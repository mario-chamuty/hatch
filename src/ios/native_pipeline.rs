//! Host-native Rust orchestration of the Flutter -> iOS `.ipa` build, a faithful
//! reimplementation of `pipeline.sh` over [`Exec`] + the ported Rust components
//! ([`macho`], [`fixups`], [`assets_car`]). No bash, no python, no `lzfse`/`mkcar`
//! CLI - the only externally-invoked binaries are the cross-compiler toolchain
//! (gen_snapshot, clang/ld64.lld, vtool/install_name_tool), which are host-native
//! by design.
//!
//! It differs from `pipeline.sh` in one deliberate way: stage 5 emits the
//! **transplant** catalog (the proven build-41 path) directly via
//! [`assets_car::transplant`], instead of the from-scratch mkcar catalog that
//! passes validation but wedges processing. This is the tech-debt productization.
//!
//! On Linux every path here is a real filesystem path, so file generation is
//! `std::fs` and tool calls are direct `Command`s. The bash `pipeline.sh` remains
//! the default (see `builder::build`); this path is opt-in until verified
//! end-to-end, then becomes the default on Linux (and, after the Windows-native
//! toolchain lands, on Windows too).

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::assets_car;
use super::builder::{BuildOutput, BuildRequest};
use super::exec::Exec;
use super::fixups;
use super::macho;

/// Genuine actool donor catalog (CoreUI 918). The transplant copies it verbatim
/// except the app-icon pixel payloads. flutter_launcher_icons always emits the
/// same 25-entry iconset, so the donor's rendition rects always match ours.
const DONOR: &[u8] = include_bytes!("assets/donor_appicon.car");

// SDK version stamped into LC_BUILD_VERSION + the Info.plist DT* keys (ITMS-90725).
const SDK_VER: &str = "26.0";
const SDK_BUILD: &str = "23A340";
const XCODE_BUILD: &str = "17A324";

/// Resolved toolchain layout (all real paths on the host), host-aware. On Linux
/// it drives the cctools/ld64 cross toolchain + downloaded engine dart-sdk; on
/// Windows it drives stock LLVM (clang/ld64.lld) + the windows-x64 gen_snapshot +
/// fvm's host dart frontend (no cctools - vtool/install_name_tool have no Windows
/// build, so the one vtool use is replaced by `macho::set_ios_build_version`).
struct Tools {
    windows: bool,
    gs: String,
    aotrt: String,
    fes: String,
    platform: String,
    flutter_fw: String,
    clang: String,
    install_name_tool: Option<String>,
    vtool: Option<String>,
    sdk: String,
    lld: Option<String>,
    /// arch/target selector for clang: `-arch arm64` (cctools) vs
    /// `-target arm64-apple-ios<minos>` (stock LLVM).
    arch: Vec<String>,
    /// `PATH` value that puts the cctools bin first (Linux only; the clang
    /// wrapper resolves `arm-apple-darwin11-ld` from `PATH`). None on Windows.
    path_env: Option<String>,
}

impl Tools {
    fn resolve(root: &str, minos: &str) -> Result<Self> {
        let windows = cfg!(windows);
        let eng = format!("{root}/engine");
        let sdk = resolve_sdk(root)?;
        // Dart frontend: HATCH_DART_SDK_BIN (e.g. fvm's dart-sdk/bin) overrides
        // the in-toolchain engine/dart-sdk/bin.
        let dartbin = std::env::var("HATCH_DART_SDK_BIN")
            .unwrap_or_else(|_| format!("{eng}/dart-sdk/bin"));
        let exe = if windows { ".exe" } else { "" };
        let aotrt = format!("{dartbin}/dartaotruntime{exe}");
        let fes = format!("{dartbin}/snapshots/frontend_server_aot.dart.snapshot");
        let platform = format!("{eng}/flutter_patched_sdk_product/platform_strong.dill");
        let flutter_fw =
            format!("{eng}/ios-release/Flutter.xcframework/ios-arm64/Flutter.framework");

        if windows {
            let llvm = format!("{root}/llvm/bin");
            Ok(Self {
                windows,
                gs: format!("{eng}/gs-win/gen_snapshot.exe"),
                aotrt, fes, platform, flutter_fw,
                clang: format!("{llvm}/clang.exe"),
                install_name_tool: None,
                vtool: None,
                sdk,
                lld: Some(format!("{llvm}/ld64.lld.exe")),
                arch: vec!["-target".into(), format!("arm64-apple-ios{minos}")],
                path_env: None,
            })
        } else {
            let tc = format!("{root}/cctools-port/usage_examples/ios_toolchain/target/bin");
            let base_path = std::env::var("PATH").unwrap_or_default();
            Ok(Self {
                windows,
                gs: format!("{eng}/gs-linux/gen_snapshot"),
                aotrt, fes, platform, flutter_fw,
                clang: format!("{tc}/arm-apple-darwin11-clang"),
                install_name_tool: Some(format!("{tc}/arm-apple-darwin11-install_name_tool")),
                vtool: Some(format!("{tc}/arm-apple-darwin11-vtool")),
                sdk,
                lld: which("ld64.lld-18").or_else(|| which("ld64.lld")),
                arch: vec!["-arch".into(), "arm64".into()],
                path_env: Some(format!("{tc}:{base_path}")),
            })
        }
    }
}

/// Build an unsigned `.ipa` natively. `root` must be a real (expanded) path.
pub fn build(req: &BuildRequest, root: &str) -> Result<BuildOutput> {
    let minos = req.min_os.as_str();
    let t = Tools::resolve(root, minos)?;
    let safe = sanitize(&req.app_name);

    let work = format!("{root}/work/{safe}");
    let out = format!("{root}/out");
    let app = format!("{out}/Payload/Runner.app");
    let app_fw_dir = format!("{app}/Frameworks/App.framework");
    let assets_dir = format!("{app_fw_dir}/flutter_assets");

    // Clean + scaffold. flutter_assets lives INSIDE App.framework (the
    // io.flutter.flutter.app bundle), exactly like a real Xcode-built app.
    let _ = fs::remove_dir_all(&work);
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&work)?;
    fs::create_dir_all(&assets_dir)?;

    stage1_aot_kernel(&t, req, &work)?;
    stage2_app_framework(&t, &work, &app, minos)?;
    stage3_flutter_framework(&t, &app, minos)?;
    // Compile native plugin sources (Swift/ObjC) before the Runner link so the
    // Runner can register + link them. Returns Plugins::none for plugin-less apps.
    let plugins = super::plugins::compile(&req.project_dir, &t.sdk, root, &work)?;
    stage4_runner(&t, req, &work, &app, minos, &plugins)?;
    let icon_plist = stage5_assets(&t, req, &app, &assets_dir)?;
    stage5_info_plist(req, &app, minos, &icon_plist)?;
    let ipa_path = stage6_package(&out, &safe)?;

    Ok(BuildOutput { ipa_path })
}

// --- stage 1 -------------------------------------------------------------

fn stage1_aot_kernel(t: &Tools, req: &BuildRequest, work: &str) -> Result<()> {
    println!("== 1/6 AOT kernel ==");
    let pkg_src = format!("{}/.dart_tool/package_config.json", req.project_dir);
    if !Path::new(&pkg_src).exists() {
        bail!("missing {pkg_src} - run 'hatch install' first");
    }
    // The file:///C:/ -> /mnt/c rewrite is only needed when a Linux frontend
    // consumes a Windows-resolved config. On a Windows host the dart frontend is
    // native and the C:/ URIs are already correct - leave them untouched.
    let raw = fs::read_to_string(&pkg_src)?;
    let content = if t.windows { raw } else { fixups::rewrite_package_config(&raw)? };
    let pkg = format!("{work}/package_config.json");
    fs::write(&pkg, content)?;

    // The dart frontend resolves URI-context args (--platform, --sdk-root,
    // --packages, entrypoint) via Uri.parse, which on Windows misreads an
    // absolute `C:/...` path as a URI with scheme `c:`. Pass those as proper
    // `file:///C:/...` URIs on Windows. --output-dill is a plain output path.
    // --sdk-root is run through Uri.file() (frontend_server._ensureFolderPath),
    // so it must be a plain path, NOT a file:// URI. A forward-slash drive path
    // (C:/Users/.../) is what Uri.file accepts on Windows.
    let sdk_root = format!("{}/", parent_str(&t.platform));
    let platform = dart_uri(t, &t.platform);
    let packages = dart_uri(t, &pkg);
    let dill = format!("{work}/app.aot.dill");
    let main = dart_uri(t, &format!("{}/lib/main.dart", req.project_dir));
    // --output-dill is run through `Uri.file()`, which needs a native Windows
    // path (backslashes), not a forward-slash path or a file:// URI.
    let out_dill = win_native(t, &dill);
    let mut args: Vec<String> = vec![
        t.fes.clone(),
        "--sdk-root".into(), sdk_root,
        "--platform".into(), platform,
        "--target=flutter".into(), "--aot".into(), "--tfa".into(),
        "-Ddart.vm.product=true".into(),
        "--packages".into(), packages,
        "--output-dill".into(), out_dill,
    ];
    // Dart-side federated-plugin registrant. `flutter build` generates this and
    // feeds it to the frontend so each plugin's platform implementation registers
    // itself (e.g. PathProviderPlatform.instance = PathProviderFoundation()).
    // Without it, federated plugins fall back to their DEFAULT MethodChannel
    // (the legacy `plugins.flutter.io/<x>` channel) which the modern Pigeon-based
    // native plugin does NOT implement -> MissingPluginException at runtime ->
    // e.g. getApplicationDocumentsDirectory throws -> Hive.initFlutter() fails ->
    // runApp() is never reached -> black screen. Wire it exactly as flutter does.
    let registrant = format!(
        "{}/.dart_tool/flutter_build/dart_plugin_registrant.dart",
        req.project_dir
    );
    if Path::new(&registrant).exists() {
        let reg_uri = dart_uri(t, &registrant);
        args.push("--source".into());
        args.push(reg_uri.clone());
        args.push("--source".into());
        args.push("package:flutter/src/dart_plugin_registrant.dart".into());
        args.push(format!("-Dflutter.dart_plugin_registrant={reg_uri}"));
    } else {
        eprintln!(">> WARN: no dart_plugin_registrant.dart - federated plugins \
                   may MissingPluginException at runtime (run 'fvm flutter pub get')");
    }
    args.push(main);
    Exec::run(&t.aotrt, str_refs(&args))?.require("AOT kernel (frontend_server)")?;
    Ok(())
}

// --- stage 2 -------------------------------------------------------------

fn stage2_app_framework(
    t: &Tools,
    work: &str,
    app: &str,
    minos: &str,
) -> Result<()> {
    println!("== 2/6 gen_snapshot -> assembly -> App.framework ==");
    let appfw = format!("{app}/Frameworks/App.framework/App");
    let dill = format!("{work}/app.aot.dill");

    if let Some(lld) = &t.lld {
        let asm = format!("{work}/snapshot.S");
        Exec::run(
            &t.gs,
            ["--snapshot_kind=app-aot-assembly", "--strip",
             &format!("--assembly={asm}"), &dill],
        )?
        .require("gen_snapshot (assembly)")?;

        // ELF -> Mach-O directive fixups (the sed transform in pipeline.sh).
        let macho_asm = format!("{work}/snapshot.macho.S");
        fs::write(&macho_asm, elf_to_macho_asm(&fs::read_to_string(&asm)?))?;

        let obj = format!("{work}/snapshot.o");
        let mut a = t.arch.clone();
        a.extend(["-isysroot".into(), t.sdk.clone(), "-c".into(), macho_asm, "-o".into(), obj.clone()]);
        run_clang(t, "assemble snapshot", &str_refs(&a))?;

        let mut b = t.arch.clone();
        b.extend([
            "-isysroot".into(), t.sdk.clone(), "-dynamiclib".into(),
            format!("-fuse-ld={lld}"), "-Wl,-fixup_chains".into(),
            format!("-Wl,-platform_version,ios,{minos},{SDK_VER}"),
            "-install_name".into(), "@rpath/App.framework/App".into(), obj, "-o".into(), appfw.clone(),
        ]);
        run_clang(t, "link App.framework", &str_refs(&b))?;
        patch_macho(&appfw, |d| macho::fix_linker_identity(d))?;
    } else {
        eprintln!(">> WARN: ld64.lld not found; App.framework will not pass App Store ingestion");
        Exec::run(&t.gs, ["--snapshot_kind=app-aot-macho-dylib",
            &format!("--macho={appfw}"), &dill])?
            .require("gen_snapshot (macho-dylib)")?;
        if let Some(int) = &t.install_name_tool {
            run_with_path(t, "install_name", int,
                &["-id", "@rpath/App.framework/App", &appfw])?;
        }
        if let Some(vtool) = &t.vtool {
            let _ = run_with_path(t, "vtool", vtool,
                &["-arch", "arm64", "-set-build-version", "ios", minos, SDK_VER,
                  "-tool", "ld", "1217", "-replace", "-output", &appfw, &appfw]);
        }
        patch_macho_slice(&appfw, |d| macho::clear_dwarf_exec_bit(d))?;
    }

    fs::write(
        format!("{app}/Frameworks/App.framework/Info.plist"),
        app_framework_plist(minos),
    )?;
    Ok(())
}

// --- stage 3 -------------------------------------------------------------

fn stage3_flutter_framework(t: &Tools, app: &str, minos: &str) -> Result<()> {
    println!("== 3/6 Flutter.framework ==");
    let dst = format!("{app}/Frameworks/Flutter.framework");
    copy_dir_all(Path::new(&t.flutter_fw), Path::new(&dst))?;
    let _ = fs::remove_dir_all(format!("{dst}/_CodeSignature"));
    let fbin = format!("{dst}/Flutter");
    // Raise the engine binary's LC_BUILD_VERSION minos/sdk to match the patched
    // plist (ITMS-90208). On Linux use cctools vtool (proven); on Windows there
    // is no vtool, so do the same edit natively via macho::set_ios_build_version.
    match &t.vtool {
        Some(vtool) => {
            let _ = run_with_path(t, "vtool (Flutter)", vtool,
                &["-arch", "arm64", "-set-build-version", "ios", minos, SDK_VER,
                  "-tool", "ld", "1217", "-replace", "-output", &fbin, &fbin]);
        }
        None => patch_macho_slice(&fbin, |d| macho::set_ios_build_version(d, minos, SDK_VER))?,
    }
    fixups::patch_flutter_framework_plist(Path::new(&format!("{dst}/Info.plist")), minos)?;
    Ok(())
}

// --- stage 4 -------------------------------------------------------------

fn stage4_runner(
    t: &Tools, req: &BuildRequest, work: &str, app: &str, minos: &str,
    plugins: &super::plugins::Plugins,
) -> Result<()> {
    println!("== 4/6 Runner (cross clang + ld64) ==");
    let rdir = format!("{work}/Runner");
    fs::create_dir_all(&rdir)?;
    fs::write(format!("{rdir}/main.m"), RUNNER_MAIN_M)?;
    fs::write(format!("{rdir}/AppDelegate.h"), RUNNER_APPDELEGATE_H)?;
    // With plugins the AppDelegate additionally calls GeneratedPluginRegistrant
    // (after the FlutterViewController is the window's rootVC, so FlutterAppDelegate
    // routes each registrar to the running engine's messenger).
    fs::write(format!("{rdir}/AppDelegate.m"),
        if plugins.has { RUNNER_APPDELEGATE_PLUGINS_M } else { RUNNER_APPDELEGATE_M })?;

    let frameworks = format!("{app}/Frameworks");
    let headers = format!("{}/Headers", t.flutter_fw);
    // CFLAGS as separate argv entries. On Windows the deployment target rides on
    // `-target arm64-apple-ios<minos>` (in t.arch); on Linux clang takes
    // `-arch arm64` + an explicit `-miphoneos-version-min`.
    let mut cflags: Vec<String> = t.arch.clone();
    cflags.extend(["-isysroot".into(), t.sdk.clone()]);
    if !t.windows {
        cflags.push(format!("-miphoneos-version-min={minos}"));
    }
    cflags.extend([
        "-fobjc-arc".into(), "-fmodules".into(),
        format!("-I{headers}"), format!("-F{frameworks}"),
    ]);

    // Plugin-aware compile flags for AppDelegate + the GeneratedPluginRegistrant:
    // staged public headers (-I inc), clang module maps (Swift plugins), the real
    // per-plugin header dirs, the ObjC prefix header, and the firebase -F.
    let mut plugin_cflags: Vec<String> = vec![];
    let mut objs: Vec<String> = vec![format!("{work}/main.o"), format!("{work}/AppDelegate.o")];
    if plugins.has {
        plugin_cflags.push("-w".into());
        plugin_cflags.push(format!("-fmodules-cache-path={work}/cmc_runner"));
        plugin_cflags.push(format!("-include")); plugin_cflags.push(plugins.prefix_h.clone());
        plugin_cflags.push(format!("-I{}", plugins.inc_dir));
        plugin_cflags.push(format!("-I{}", plugins.mod_dir));
        plugin_cflags.push(format!("-I{}/ios/Runner", req.project_dir)); // GeneratedPluginRegistrant.h
        for d in &plugins.include_dirs { plugin_cflags.push(format!("-I{d}")); }
        plugin_cflags.extend(plugins.swift_modmaps.clone());
        if !plugins.fb_dir.is_empty() { plugin_cflags.push(format!("-F{}", plugins.fb_dir)); }
        // resource-dir for the clang module compiles (Swift -Swift.h umbrella etc.)
        // is implicit via the toolchain clang; nothing extra needed.
    }

    // main.m (no plugin headers needed); AppDelegate.m + registrant.m (plugin headers).
    {
        let mut a = cflags.clone();
        a.extend(["-c".into(), format!("{rdir}/main.m"), "-o".into(), format!("{work}/main.o")]);
        run_clang(t, "compile Runner main", &str_refs(&a))?;
    }
    {
        let mut a = cflags.clone();
        a.extend(plugin_cflags.clone());
        a.extend(["-c".into(), format!("{rdir}/AppDelegate.m"), "-o".into(), format!("{work}/AppDelegate.o")]);
        run_clang(t, "compile Runner AppDelegate", &str_refs(&a))?;
    }
    if plugins.has {
        let reg = format!("{}/ios/Runner/GeneratedPluginRegistrant.m", req.project_dir);
        if Path::new(&reg).exists() {
            let regdir = format!("{}/ios/Runner", req.project_dir);
            let mut a = cflags.clone();
            a.extend(plugin_cflags.clone());
            a.push(format!("-I{regdir}"));
            a.extend(["-c".into(), reg, "-o".into(), format!("{work}/registrant.o")]);
            run_clang(t, "compile GeneratedPluginRegistrant", &str_refs(&a))?;
            objs.push(format!("{work}/registrant.o"));
        } else {
            bail!("plugins present but no GeneratedPluginRegistrant.m at {reg}");
        }
        objs.extend(plugins.objects.clone());
    }

    // Plugin link flags: firebase -F, swift runtime stubs (SDK), genuine compat
    // .a + clang_rt force-loaded, plus -lc++/-lz/-lsqlite3 the plugins use.
    let mut plugin_link: Vec<String> = vec![];
    if plugins.has {
        if !plugins.fb_dir.is_empty() {
            plugin_link.push("-F".into()); plugin_link.push(plugins.fb_dir.clone());
            // @import autolink does not cover the whole static Firebase closure
            // (FIRApp/FIRCrashlytics/FIRMessaging/GUL...); link each explicitly.
            for fw in &plugins.fb_frameworks {
                plugin_link.push("-framework".into()); plugin_link.push(fw.clone());
            }
        }
        plugin_link.extend([
            // Load ObjC class+category symbols from the static plugin/Firebase
            // frameworks. Without -ObjC, categories in static libs are dropped,
            // so e.g. Firebase's NSData (de)compression category used by the
            // heartbeat payload (`Data.zipped()`) is missing -> `+[NSData ...]:
            // unrecognized selector` SIGABRT when the (daily) heartbeat fires.
            "-Xlinker".into(), "-ObjC".into(),
            "-lc++".into(), "-lz".into(), "-lsqlite3".into(),
            "-L".into(), format!("{}/usr/lib/swift", t.sdk),
            "-L".into(), super::plugins::compat_lib_dir(),
            "-L".into(), super::plugins::clang_rt_dir(), "-lclang_rt.ios".into(),
            "-Xlinker".into(), "-rpath".into(), "-Xlinker".into(), "/usr/lib/swift".into(),
        ]);
    }

    let runner_bin = format!("{app}/Runner");
    if let Some(lld) = &t.lld {
        println!(">> linking Runner with ld64.lld (chained fixups, sdk {SDK_VER}){}",
            if plugins.has { format!(", +{} plugin objects", plugins.objects.len()) } else { String::new() });
        let mut a = t.arch.clone();
        a.extend([
            "-isysroot".into(), t.sdk.clone(),
            format!("-fuse-ld={lld}"), "-Wl,-fixup_chains".into(),
            format!("-Wl,-platform_version,ios,{minos},{SDK_VER}"),
        ]);
        a.extend(objs.clone());
        a.extend([
            "-F".into(), frameworks.clone(),
            "-framework".into(), "Flutter".into(), "-framework".into(), "UIKit".into(),
            "-framework".into(), "Foundation".into(),
        ]);
        a.extend(plugin_link.clone());
        a.extend([
            "-Xlinker".into(), "-rpath".into(), "-Xlinker".into(),
            "@executable_path/Frameworks".into(), "-o".into(), runner_bin.clone(),
        ]);
        run_clang(t, "link Runner", &str_refs(&a))?;
        patch_macho(&runner_bin, |d| macho::fix_linker_identity(d))?;
    } else {
        let mut a = t.arch.clone();
        a.extend([
            "-isysroot".into(), t.sdk.clone(), format!("-miphoneos-version-min={minos}"),
        ]);
        a.extend(objs.clone());
        a.extend([
            "-F".into(), frameworks.clone(),
            "-framework".into(), "Flutter".into(), "-framework".into(), "UIKit".into(),
            "-framework".into(), "Foundation".into(),
        ]);
        a.extend(plugin_link.clone());
        a.extend([
            "-Xlinker".into(), "-rpath".into(), "-Xlinker".into(),
            "@executable_path/Frameworks".into(), "-o".into(), runner_bin.clone(),
        ]);
        run_clang(t, "link Runner", &str_refs(&a))?;
        if let Some(vtool) = &t.vtool {
            let raised = format!("{runner_bin}.v");
            if run_with_path(t, "vtool (Runner)", vtool,
                &["-arch", "arm64", "-set-build-version", "ios", minos, SDK_VER,
                  "-tool", "ld", "1217", "-replace", "-output", &raised, &runner_bin]).is_ok()
            {
                fs::rename(&raised, &runner_bin)?;
            }
        }
    }
    Ok(())
}

// --- stage 5 -------------------------------------------------------------

/// Returns the CFBundleIcons plist snippet (empty if no catalog was produced).
fn stage5_assets(t: &Tools, req: &BuildRequest, app: &str, assets_dir: &str) -> Result<String> {
    println!("== 5/6 bundle assets + Assets.car + Info.plist ==");
    // Xcode strips Headers/ + Modules/ when embedding a framework (done only now;
    // stage 4 compiled against the embedded Headers).
    let ffw = format!("{app}/Frameworks/Flutter.framework");
    let _ = fs::remove_dir_all(format!("{ffw}/Headers"));
    let _ = fs::remove_dir_all(format!("{ffw}/Modules"));

    let built_assets = format!("{}/build/flutter_assets", req.project_dir);
    if Path::new(&built_assets).exists() {
        copy_dir_all(Path::new(&built_assets), Path::new(assets_dir))?;
        let _ = fs::remove_file(format!("{assets_dir}/kernel_blob.bin"));
    } else {
        fs::write(format!("{assets_dir}/AssetManifest.json"), "{}")?;
        fs::write(format!("{assets_dir}/FontManifest.json"), "[]")?;
    }

    // Firebase iOS config (the GoogleService-Info.plist counterpart of Android's
    // google-services.json). Firebase.initializeApp uses explicit DefaultFirebaseOptions,
    // but bundling the plist matches a standard iOS Firebase app and lets
    // FirebaseMessaging/APNs resolve its app config at the bundle root.
    let gsi = format!("{}/ios/Runner/GoogleService-Info.plist", req.project_dir);
    if Path::new(&gsi).exists() {
        fs::copy(&gsi, format!("{app}/GoogleService-Info.plist"))?;
        println!(">> bundled GoogleService-Info.plist");
    }

    // App-icon catalog: the PROVEN transplant path (genuine container + our
    // pixels), generated natively. No actool, no python, no Mac.
    let mut icon_plist = String::new();
    let iconset = format!("{}/ios/Runner/Assets.xcassets/AppIcon.appiconset", req.project_dir);
    if Path::new(&iconset).exists() {
        let car = assets_car::transplant(DONOR, Path::new(&iconset))
            .context("generating Assets.car (transplant)")?;
        fs::write(format!("{app}/Assets.car"), car)?;
        println!(">> Assets.car generated from AppIcon.appiconset (native transplant, no actool)");
        // actool also emits the primary icon as a loose PNG at the bundle root
        // (AppIcon60x60@2x.png = the 120x120 ITMS-90022 checks for).
        let icon120 = format!("{iconset}/Icon-App-60x60@2x.png");
        if Path::new(&icon120).exists() {
            fs::copy(&icon120, format!("{app}/AppIcon60x60@2x.png"))?;
            println!(">> AppIcon60x60@2x.png (120x120) emitted at bundle root");
        }
        icon_plist = ICON_PLIST_SNIPPET.to_string();
    }
    let _ = t; // tools unused in this stage; kept for signature symmetry

    // PkgInfo: 8-byte type+creator every Xcode bundle carries.
    fs::write(format!("{app}/PkgInfo"), b"APPL????")?;
    Ok(icon_plist)
}

fn stage5_info_plist(req: &BuildRequest, app: &str, minos: &str, icon_plist: &str) -> Result<()> {
    let generated = info_plist(
        &req.bundle_id, &req.app_name, &req.short_version, &req.build_number, minos, icon_plist,
    );
    let dst = format!("{app}/Info.plist");
    // Merge the app's real ios/Runner/Info.plist (usage descriptions, query
    // schemes, background modes, ATS, launch screen, etc.) - required for both
    // runtime (permission plugins crash without purpose strings) and App Store
    // ingestion (ITMS-90683). hatch's structural keys always win; storyboard
    // keys are dropped (we create the FlutterViewController in code).
    let app_plist = format!("{}/ios/Runner/Info.plist", req.project_dir);
    match merge_info_plist(&generated, &app_plist, req) {
        Ok(merged) => { fs::write(&dst, merged)?; }
        Err(e) => {
            eprintln!(">> WARN: Info.plist merge failed ({e:#}); using generated plist only");
            fs::write(&dst, generated)?;
        }
    }
    Ok(())
}

/// Keys hatch controls (its generated value always wins over the app's).
const HATCH_OWNED_PLIST_KEYS: &[&str] = &[
    "CFBundleExecutable", "CFBundleIdentifier", "CFBundleName", "CFBundleDisplayName",
    "CFBundleVersion", "CFBundleShortVersionString", "CFBundlePackageType", "CFBundleSignature",
    "CFBundleInfoDictionaryVersion", "CFBundleDevelopmentRegion", "MinimumOSVersion",
    "CFBundleSupportedPlatforms", "UIRequiredDeviceCapabilities", "DTPlatformName",
    "DTPlatformVersion", "DTSDKName", "DTSDKBuild", "DTPlatformBuild", "DTXcode", "DTXcodeBuild",
    "DTCompiler", "BuildMachineOSBuild", "CFBundleIcons", "CFBundleIconName", "UIDeviceFamily",
    "LSRequiresIPhoneOS",
];
/// Storyboard keys dropped (hatch builds the FlutterViewController in code).
const DROP_PLIST_KEYS: &[&str] = &["UIMainStoryboardFile", "UILaunchStoryboardName"];

fn merge_info_plist(generated: &str, app_plist_path: &str, req: &BuildRequest) -> Result<Vec<u8>> {
    use plist::Value;
    let mut base = Value::from_reader_xml(std::io::Cursor::new(generated.as_bytes()))
        .context("parsing generated plist")?;
    let base_dict = base.as_dictionary_mut().context("generated plist is not a dict")?;
    if !Path::new(app_plist_path).exists() {
        // no app plist - just re-serialize the generated dict
        let mut buf = Vec::new();
        plist::to_writer_xml(&mut buf, &base)?;
        return Ok(buf);
    }
    let app_val = Value::from_file(app_plist_path)
        .with_context(|| format!("parsing {app_plist_path}"))?;
    let app_dict = app_val.as_dictionary().context("app plist is not a dict")?;
    // Xcode build-setting substitutions (Mac-free; no xcodebuild to expand them).
    let subs: Vec<(&str, &str)> = vec![
        ("$(PRODUCT_NAME)", &req.app_name),
        ("${PRODUCT_NAME}", &req.app_name),
        ("$(FLUTTER_BUILD_NAME)", &req.short_version),
        ("$(FLUTTER_BUILD_NUMBER)", &req.build_number),
        ("$(PRODUCT_BUNDLE_IDENTIFIER)", &req.bundle_id),
        ("$(DEVELOPMENT_LANGUAGE)", "en"),
        ("$(EXECUTABLE_NAME)", "Runner"),
        ("$(PRODUCT_BUNDLE_PACKAGE_TYPE)", "APPL"),
    ];
    for (k, v) in app_dict {
        if HATCH_OWNED_PLIST_KEYS.contains(&k.as_str()) || DROP_PLIST_KEYS.contains(&k.as_str()) {
            continue;
        }
        let mut v = v.clone();
        subst_plist(&mut v, &subs);
        base_dict.insert(k.clone(), v);
    }
    let mut buf = Vec::new();
    plist::to_writer_xml(&mut buf, &base)?;
    Ok(buf)
}

/// Recursively apply `$(VAR)` string substitutions throughout a plist value.
fn subst_plist(v: &mut plist::Value, subs: &[(&str, &str)]) {
    use plist::Value;
    match v {
        Value::String(s) => {
            for (from, to) in subs {
                if s.contains(from) { *s = s.replace(from, to); }
            }
        }
        Value::Array(a) => for x in a.iter_mut() { subst_plist(x, subs); },
        Value::Dictionary(d) => for (_, x) in d.iter_mut() { subst_plist(x, subs); },
        _ => {}
    }
}

// --- stage 6 -------------------------------------------------------------

fn stage6_package(out: &str, safe: &str) -> Result<String> {
    println!("== 6/6 package .ipa ==");
    let ipa = format!("{out}/{safe}.ipa");
    let _ = fs::remove_file(&ipa);
    zip_dir(Path::new(out), "Payload", Path::new(&ipa))
        .with_context(|| format!("packaging {ipa}"))?;
    println!(">> built {ipa}");
    Ok(ipa)
}

// --- helpers -------------------------------------------------------------

fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect()
}

fn parent_str(p: &str) -> String {
    Path::new(p).parent().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default()
}

fn str_refs(v: &[String]) -> Vec<&str> {
    v.iter().map(|s| s.as_str()).collect()
}

/// On Windows, turn an absolute path into a `file:///C:/...` URI so the dart
/// frontend's `Uri.parse` does not mistake the drive letter for a URI scheme.
/// On Linux the dart tools accept plain paths, so pass through unchanged.
fn dart_uri(t: &Tools, path: &str) -> String {
    if t.windows {
        format!("file:///{}", path.replace('\\', "/"))
    } else {
        path.to_string()
    }
}

/// On Windows, convert a forward-slash path to a native backslash path (for args
/// consumed via `Uri.file()`); pass through on Linux.
fn win_native(t: &Tools, path: &str) -> String {
    if t.windows {
        path.replace('/', "\\")
    } else {
        path.to_string()
    }
}

fn run_clang(t: &Tools, what: &str, args: &[&str]) -> Result<()> {
    let clang = t.clang.clone();
    run_with_path(t, what, &clang, args)
}

fn run_with_path(t: &Tools, what: &str, prog: &str, args: &[&str]) -> Result<()> {
    // cctools clang resolves its linker from PATH (Linux); stock LLVM does not.
    let env: Vec<(&str, &str)> = match &t.path_env {
        Some(p) => vec![("PATH", p.as_str())],
        None => vec![],
    };
    Exec::run_full(prog, args.iter(), None, &env)?
        .require(what)
        .map(|_| ())
}

fn patch_macho<F: Fn(&mut Vec<u8>) -> Result<()>>(path: &str, f: F) -> Result<()> {
    let mut d = fs::read(path).with_context(|| format!("reading {path}"))?;
    f(&mut d)?;
    fs::write(path, &d).with_context(|| format!("writing {path}"))?;
    Ok(())
}

fn patch_macho_slice<F: Fn(&mut [u8]) -> Result<()>>(path: &str, f: F) -> Result<()> {
    let mut d = fs::read(path).with_context(|| format!("reading {path}"))?;
    f(&mut d)?;
    fs::write(path, &d).with_context(|| format!("writing {path}"))?;
    Ok(())
}

/// Find the first `iPhoneOS*.sdk` under `<root>/iossdk`.
fn resolve_sdk(root: &str) -> Result<String> {
    let dir = format!("{root}/iossdk");
    let entries = fs::read_dir(&dir).with_context(|| format!("no iOS SDK dir at {dir}"))?;
    let mut found: Option<String> = None;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("iPhoneOS") && name.ends_with(".sdk") {
            let p = e.path().to_string_lossy().into_owned();
            found = Some(match found {
                Some(prev) if prev <= p => prev,
                _ => p,
            });
        }
    }
    found.with_context(|| format!("no iPhoneOS*.sdk found under {dir}"))
}

/// Search `PATH` for an executable named `name`.
fn which(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand.to_string_lossy().into_owned());
        }
    }
    None
}

/// Apply the three `sed` ELF->Mach-O directive fixups from pipeline.sh:
///  * drop lines whose first token is `.size` / `.type` (ELF symbol metadata)
///  * drop the `.section .note.GNU-stack` line
///  * replace a `.section .rodata...` line with `.const` (__TEXT,__const)
fn elf_to_macho_asm(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let trimmed = line.trim_start();
        let mut toks = trimmed.split_whitespace();
        let first = toks.next().unwrap_or("");
        if first == ".size" || first == ".type" {
            continue;
        }
        if first == ".section" {
            let second = toks.next().unwrap_or("");
            if second.starts_with(".note.GNU-stack") {
                continue;
            }
            if second.starts_with(".rodata") {
                out.push_str(".const\n");
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Recursively copy a directory tree, preserving symlinks. (iOS frameworks are
/// flat, but be robust.)
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if ft.is_symlink() {
            let target = fs::read_link(&from)?;
            symlink_any(&target, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_any(target: &Path, link: &Path) -> Result<()> {
    let _ = fs::remove_file(link);
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

#[cfg(windows)]
fn symlink_any(target: &Path, link: &Path) -> Result<()> {
    // Frameworks shouldn't contain symlinks; fall back to a copy if one appears.
    let _ = fs::remove_file(link);
    fs::copy(target, link).map(|_| ()).or_else(|_| {
        std::os::windows::fs::symlink_file(target, link).map_err(Into::into)
    })
}

/// Zip `<base>/<sub>` into `out`, with entries named `<sub>/...` (matching
/// `cd base && zip -r out sub`). Mach-O files get exec permission.
fn zip_dir(base: &Path, sub: &str, out: &Path) -> Result<String> {
    use zip::write::SimpleFileOptions;
    let file = fs::File::create(out)?;
    let mut zw = zip::ZipWriter::new(file);
    let root = base.join(sub);
    let mut stack: Vec<PathBuf> = vec![root.clone()];
    // Deterministic order: walk depth-first, sorting entries.
    while let Some(dir) = stack.pop() {
        let rel = format!("{}/", path_rel(base, &dir));
        zw.add_directory(rel, SimpleFileOptions::default())?;
        let mut entries: Vec<_> = fs::read_dir(&dir)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let ft = e.file_type()?;
            if ft.is_dir() {
                stack.push(p);
            } else {
                let data = fs::read(&p)?;
                let exec = data.len() >= 4 && data[..4] == [0xCF, 0xFA, 0xED, 0xFE];
                let mode = if exec { 0o755 } else { 0o644 };
                let opts = SimpleFileOptions::default().unix_permissions(mode);
                zw.start_file(path_rel(base, &p), opts)?;
                use std::io::Write;
                zw.write_all(&data)?;
            }
        }
    }
    zw.finish()?;
    Ok(out.to_string_lossy().into_owned())
}

fn path_rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

// --- embedded sources + plists ------------------------------------------

const RUNNER_MAIN_M: &str = r#"#import <UIKit/UIKit.h>
#import "AppDelegate.h"
int main(int argc, char * argv[]) {
  @autoreleasepool { return UIApplicationMain(argc, argv, nil, NSStringFromClass([AppDelegate class])); }
}
"#;

const RUNNER_APPDELEGATE_H: &str = r#"#import <UIKit/UIKit.h>
#import <Flutter/Flutter.h>
@interface AppDelegate : FlutterAppDelegate
@end
"#;

const RUNNER_APPDELEGATE_M: &str = r#"#import "AppDelegate.h"
@implementation AppDelegate
- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
  self.window = [[UIWindow alloc] initWithFrame:[UIScreen mainScreen].bounds];
  FlutterViewController *flutterViewController =
      [[FlutterViewController alloc] initWithProject:nil nibName:nil bundle:nil];
  self.window.rootViewController = flutterViewController;
  [self.window makeKeyAndVisible];
  return [super application:application didFinishLaunchingWithOptions:launchOptions];
}
@end
"#;

// AppDelegate that registers the Flutter plugins. The FlutterViewController is
// the window's rootViewController BEFORE registration, so FlutterAppDelegate's
// plugin registry routes each plugin's registrar to that engine's messenger.
const RUNNER_APPDELEGATE_PLUGINS_M: &str = r#"#import "AppDelegate.h"
#import "GeneratedPluginRegistrant.h"
@implementation AppDelegate {
  FlutterEngine *_engine;
}
- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
  // Explicit engine so plugin registration lands on the SAME messenger that runs
  // the Dart entrypoint. Registering against the AppDelegate while using an
  // implicit FlutterViewController engine puts plugin channels on a different
  // messenger -> Firebase.initializeApp()/method channels hang -> black screen.
  _engine = [[FlutterEngine alloc] initWithName:@"io.flutter" project:nil
                          allowHeadlessExecution:YES];
  [_engine run];
  [GeneratedPluginRegistrant registerWithRegistry:_engine];
  self.window = [[UIWindow alloc] initWithFrame:[UIScreen mainScreen].bounds];
  FlutterViewController *flutterViewController =
      [[FlutterViewController alloc] initWithEngine:_engine nibName:nil bundle:nil];
  self.window.rootViewController = flutterViewController;
  [self.window makeKeyAndVisible];
  return [super application:application didFinishLaunchingWithOptions:launchOptions];
}
@end
"#;

const ICON_PLIST_SNIPPET: &str = r#"  <key>CFBundleIconName</key><string>AppIcon</string>
  <key>CFBundleIcons</key><dict>
    <key>CFBundlePrimaryIcon</key><dict>
      <key>CFBundleIconFiles</key><array><string>AppIcon60x60</string></array>
      <key>CFBundleIconName</key><string>AppIcon</string>
    </dict>
  </dict>"#;

fn app_framework_plist(minos: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>App</string>
  <key>CFBundleIdentifier</key><string>io.flutter.flutter.app</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>App</string>
  <key>CFBundlePackageType</key><string>FMWK</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleSignature</key><string>????</string>
  <key>CFBundleVersion</key><string>1.0</string>
  <key>MinimumOSVersion</key><string>{minos}</string>
</dict></plist>
"#
    )
}

fn info_plist(
    bundle_id: &str,
    app_name: &str,
    short_version: &str,
    build_number: &str,
    minos: &str,
    icon_plist: &str,
) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>Runner</string>
  <key>CFBundleIdentifier</key><string>{bundle_id}</string>
  <key>CFBundleName</key><string>{app_name}</string>
  <key>CFBundleDisplayName</key><string>{app_name}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleSignature</key><string>????</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>{short_version}</string>
  <key>CFBundleVersion</key><string>{build_number}</string>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>ITSAppUsesNonExemptEncryption</key><false/>
  <key>MinimumOSVersion</key><string>{minos}</string>
  <key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array>
  <key>UIRequiredDeviceCapabilities</key><array><string>arm64</string></array>
  <key>DTPlatformName</key><string>iphoneos</string>
  <key>DTPlatformVersion</key><string>{SDK_VER}</string>
  <key>DTSDKName</key><string>iphoneos{SDK_VER}</string>
  <key>DTSDKBuild</key><string>{SDK_BUILD}</string>
  <key>DTPlatformBuild</key><string>{SDK_BUILD}</string>
  <key>DTXcode</key><string>2600</string>
  <key>DTXcodeBuild</key><string>{XCODE_BUILD}</string>
  <key>DTCompiler</key><string>com.apple.compilers.llvm.clang.1_0</string>
  <key>BuildMachineOSBuild</key><string>25A354</string>
{icon_plist}
  <key>UIDeviceFamily</key><array><integer>1</integer></array>
  <key>UILaunchScreen</key><dict/>
  <key>UISupportedInterfaceOrientations</key>
  <array><string>UIInterfaceOrientationPortrait</string></array>
</dict></plist>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elf_to_macho_directive_fixups() {
        let src = "\
\t.text
\t.globl foo
\t.type foo, @function
foo:
\t.size foo, .-foo
\t.section .rodata.cst16,\"aM\",@progbits,16
\t.section .note.GNU-stack,\"\",@progbits
\t.quad 0
";
        let out = elf_to_macho_asm(src);
        assert!(!out.contains(".type"), "{out}");
        assert!(!out.contains(".size"), "{out}");
        assert!(!out.contains(".note.GNU-stack"), "{out}");
        assert!(out.contains(".const"), "{out}");
        assert!(!out.contains(".rodata"), "{out}");
        assert!(out.contains(".globl foo"));
        assert!(out.contains("\t.quad 0"));
    }

    #[test]
    fn sanitize_replaces_nonalnum() {
        assert_eq!(sanitize("My App!.v2"), "My_App__v2");
    }
}
