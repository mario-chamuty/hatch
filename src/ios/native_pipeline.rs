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

/// Resolved toolchain layout (all real paths on the host).
struct Tools {
    gs: String,
    aotrt: String,
    fes: String,
    platform: String,
    flutter_fw: String,
    clang: String,
    install_name_tool: String,
    vtool: String,
    sdk: String,
    lld: Option<String>,
    /// `PATH` value that puts the cctools bin first (the clang wrapper resolves
    /// `arm-apple-darwin11-ld` from `PATH`).
    path_env: String,
}

impl Tools {
    fn resolve(root: &str) -> Result<Self> {
        let eng = format!("{root}/engine");
        let dartbin = format!("{eng}/dart-sdk/bin");
        let tc = format!("{root}/cctools-port/usage_examples/ios_toolchain/target/bin");
        let sdk = resolve_sdk(root)?;
        let lld = which("ld64.lld-18").or_else(|| which("ld64.lld"));
        let base_path = std::env::var("PATH").unwrap_or_default();
        Ok(Self {
            gs: format!("{eng}/gs-linux/gen_snapshot"),
            aotrt: format!("{dartbin}/dartaotruntime"),
            fes: format!("{dartbin}/snapshots/frontend_server_aot.dart.snapshot"),
            platform: format!("{eng}/flutter_patched_sdk_product/platform_strong.dill"),
            flutter_fw: format!(
                "{eng}/ios-release/Flutter.xcframework/ios-arm64/Flutter.framework"
            ),
            clang: format!("{tc}/arm-apple-darwin11-clang"),
            install_name_tool: format!("{tc}/arm-apple-darwin11-install_name_tool"),
            vtool: format!("{tc}/arm-apple-darwin11-vtool"),
            sdk,
            lld,
            path_env: format!("{tc}:{base_path}"),
        })
    }
}

/// Build an unsigned `.ipa` natively. `root` must be a real (expanded) path.
pub fn build(req: &BuildRequest, root: &str) -> Result<BuildOutput> {
    let t = Tools::resolve(root)?;
    let minos = req.min_os.as_str();
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
    stage2_app_framework(&t, req, &work, &app, minos)?;
    stage3_flutter_framework(&t, &app, minos)?;
    stage4_runner(&t, &work, &app, minos)?;
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
    let fixed = fixups::rewrite_package_config(&fs::read_to_string(&pkg_src)?)?;
    let pkg = format!("{work}/package_config.json");
    fs::write(&pkg, fixed)?;

    let sdk_root = format!("{}/", parent_str(&t.platform));
    let dill = format!("{work}/app.aot.dill");
    let main = format!("{}/lib/main.dart", req.project_dir);
    let args = [
        t.fes.as_str(),
        "--sdk-root", &sdk_root,
        "--platform", &t.platform,
        "--target=flutter", "--aot", "--tfa", "-Ddart.vm.product=true",
        "--packages", &pkg,
        "--output-dill", &dill,
        &main,
    ];
    Exec::run(&t.aotrt, args)?.require("AOT kernel (frontend_server)")?;
    Ok(())
}

// --- stage 2 -------------------------------------------------------------

fn stage2_app_framework(
    t: &Tools,
    req: &BuildRequest,
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
        run_clang(t, "assemble snapshot",
            &["-arch", "arm64", "-isysroot", &t.sdk, "-c", &macho_asm, "-o", &obj])?;
        run_clang(t, "link App.framework",
            &["-arch", "arm64", "-isysroot", &t.sdk, "-dynamiclib",
              &format!("-fuse-ld={lld}"), "-Wl,-fixup_chains",
              &format!("-Wl,-platform_version,ios,{minos},{SDK_VER}"),
              "-install_name", "@rpath/App.framework/App", &obj, "-o", &appfw])?;
        patch_macho(&appfw, |d| macho::fix_linker_identity(d))?;
    } else {
        eprintln!(">> WARN: ld64.lld not found; App.framework will not pass App Store ingestion");
        Exec::run(&t.gs, ["--snapshot_kind=app-aot-macho-dylib",
            &format!("--macho={appfw}"), &dill])?
            .require("gen_snapshot (macho-dylib)")?;
        run_with_path(t, "install_name", &t.install_name_tool,
            &["-id", "@rpath/App.framework/App", &appfw])?;
        // best-effort vtool build-version (|| true in bash)
        let _ = run_with_path(t, "vtool", &t.vtool,
            &["-arch", "arm64", "-set-build-version", "ios", minos, SDK_VER,
              "-tool", "ld", "1217", "-replace", "-output", &appfw, &appfw]);
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
    // best-effort raise of the engine binary's minos (|| true in bash); the plist
    // patch below is what actually clears ITMS-90208.
    let _ = run_with_path(t, "vtool (Flutter)", &t.vtool,
        &["-arch", "arm64", "-set-build-version", "ios", minos, SDK_VER,
          "-tool", "ld", "1217", "-replace", "-output", &fbin, &fbin]);
    fixups::patch_flutter_framework_plist(Path::new(&format!("{dst}/Info.plist")), minos)?;
    Ok(())
}

// --- stage 4 -------------------------------------------------------------

fn stage4_runner(t: &Tools, work: &str, app: &str, minos: &str) -> Result<()> {
    println!("== 4/6 Runner (cross clang + ld64) ==");
    let rdir = format!("{work}/Runner");
    fs::create_dir_all(&rdir)?;
    fs::write(format!("{rdir}/main.m"), RUNNER_MAIN_M)?;
    fs::write(format!("{rdir}/AppDelegate.h"), RUNNER_APPDELEGATE_H)?;
    fs::write(format!("{rdir}/AppDelegate.m"), RUNNER_APPDELEGATE_M)?;

    let frameworks = format!("{app}/Frameworks");
    let headers = format!("{}/Headers", t.flutter_fw);
    // CFLAGS as separate argv entries.
    let cflags: Vec<String> = vec![
        "-arch".into(), "arm64".into(),
        "-isysroot".into(), t.sdk.clone(),
        format!("-miphoneos-version-min={minos}"),
        "-fobjc-arc".into(), "-fmodules".into(),
        format!("-I{headers}"),
        format!("-F{frameworks}"),
    ];
    for (src, obj) in [("main.m", "main.o"), ("AppDelegate.m", "AppDelegate.o")] {
        let mut a: Vec<String> = cflags.clone();
        a.extend(["-c".into(), format!("{rdir}/{src}"), "-o".into(), format!("{work}/{obj}")]);
        run_with_path(t, "compile Runner", &t.clang, &a.iter().map(|s| s.as_str()).collect::<Vec<_>>())?;
    }

    let runner_bin = format!("{app}/Runner");
    let main_o = format!("{work}/main.o");
    let appdel_o = format!("{work}/AppDelegate.o");
    if let Some(lld) = &t.lld {
        println!(">> linking Runner with ld64.lld (chained fixups, sdk {SDK_VER})");
        run_clang(t, "link Runner",
            &["-arch", "arm64", "-isysroot", &t.sdk,
              &format!("-fuse-ld={lld}"), "-Wl,-fixup_chains",
              &format!("-Wl,-platform_version,ios,{minos},{SDK_VER}"),
              &main_o, &appdel_o,
              "-F", &frameworks, "-framework", "Flutter", "-framework", "UIKit",
              "-framework", "Foundation",
              "-Xlinker", "-rpath", "-Xlinker", "@executable_path/Frameworks",
              "-o", &runner_bin])?;
        patch_macho(&runner_bin, |d| macho::fix_linker_identity(d))?;
    } else {
        run_clang(t, "link Runner",
            &["-arch", "arm64", "-isysroot", &t.sdk, &format!("-miphoneos-version-min={minos}"),
              &main_o, &appdel_o,
              "-F", &frameworks, "-framework", "Flutter", "-framework", "UIKit",
              "-framework", "Foundation",
              "-Xlinker", "-rpath", "-Xlinker", "@executable_path/Frameworks",
              "-o", &runner_bin])?;
        let raised = format!("{runner_bin}.v");
        if run_with_path(t, "vtool (Runner)", &t.vtool,
            &["-arch", "arm64", "-set-build-version", "ios", minos, SDK_VER,
              "-tool", "ld", "1217", "-replace", "-output", &raised, &runner_bin]).is_ok()
        {
            fs::rename(&raised, &runner_bin)?;
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
    let plist = info_plist(
        &req.bundle_id, &req.app_name, &req.short_version, &req.build_number, minos, icon_plist,
    );
    fs::write(format!("{app}/Info.plist"), plist)?;
    Ok(())
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

fn run_clang(t: &Tools, what: &str, args: &[&str]) -> Result<()> {
    run_with_path(t, what, &t.clang, args)
}

fn run_with_path(t: &Tools, what: &str, prog: &str, args: &[&str]) -> Result<()> {
    Exec::run_full(prog, args.iter(), None, &[("PATH", t.path_env.as_str())])?
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
