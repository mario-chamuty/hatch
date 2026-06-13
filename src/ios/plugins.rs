//! Native iOS plugin support (Workstream B): compile every Flutter plugin's
//! Swift/ObjC sources to arm64-apple-ios objects and surface the staged public
//! headers + clang module maps so the GeneratedPluginRegistrant compiles and the
//! Runner links them in. Mac-free, Windows-native.
//!
//! INTERIM: the per-plugin compile is driven by the proven `build_plugins.py`
//! (embedded below) plus three idempotent SDK-completion patch scripts. These
//! reproduce, exactly, the recipe proven by hand on ScamNemesis (18/18 plugins
//! incl. Firebase + the multi-target in_app_purchase_storekit). The pure-Rust
//! port of the compile loop is the remaining B5b/B7 tail; the LINK and the
//! registrant compile are already done natively in `native_pipeline::stage4`.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use super::exec::Exec;

const BUILD_PLUGINS_PY: &str = include_str!("tools/build_plugins.py");
const PATCH_STOREKIT_PY: &str = include_str!("tools/patch_storekit_signature.py");
const PATCH_AVAIL_PY: &str = include_str!("tools/patch_availability_visionos.py");
const PATCH_OVERLAYS_PY: &str = include_str!("tools/patch_sdk_swift_overlays.py");

/// Genuine Swift back-deploy compat `.a` + `libclang_rt.ios.a`, force-loaded at
/// link from the Xcode-extract iphoneos toolchain. Overridable for portability.
pub fn compat_lib_dir() -> String {
    std::env::var("HATCH_IOS_SWIFT_COMPAT_DIR").unwrap_or_else(|_|
        r"D:\dartwin\xcode\extract\Xcode.app\Contents\Developer\Toolchains\XcodeDefault.xctoolchain\usr\lib\swift\iphoneos".into())
}
pub fn clang_rt_dir() -> String {
    std::env::var("HATCH_IOS_CLANG_RT_DIR").unwrap_or_else(|_|
        r"D:\dartwin\xcode\extract\Xcode.app\Contents\Developer\Toolchains\XcodeDefault.xctoolchain\usr\lib\clang\21\lib\darwin".into())
}
/// Gathered Firebase static xcframework slices (-F). Overridable.
pub fn fb_framework_dir(root: &str) -> String {
    std::env::var("HATCH_IOS_FB_DIR").unwrap_or_else(|_| format!("{root}/dl/fbframeworks"))
}

pub struct Plugins {
    pub has: bool,
    /// Compiled plugin object files (arm64 Mach-O).
    pub objects: Vec<String>,
    /// `-I` dir of staged public headers (`inc/<plugin>/*.h`).
    pub inc_dir: String,
    /// dir of synthesized clang module maps (`mod/<plugin>/module.modulemap`).
    pub mod_dir: String,
    /// ObjC prefix header (Foundation+UIKit) used compiling the registrant.
    pub prefix_h: String,
    /// real per-plugin header search dirs (so `<plugin/Header.h>` + root-relative
    /// sub-imports resolve when compiling the registrant).
    pub include_dirs: Vec<String>,
    /// `-fmodule-map-file=` args exposing each Swift plugin's clang module.
    pub swift_modmaps: Vec<String>,
    /// Firebase static framework dir (-F), or empty if no firebase plugin.
    pub fb_dir: String,
    /// every framework name under `fb_dir` to link explicitly (`-framework X`);
    /// `@import` autolink alone does not cover the whole static closure.
    pub fb_frameworks: Vec<String>,
}

impl Plugins {
    fn none() -> Self {
        Plugins { has: false, objects: vec![], inc_dir: String::new(), mod_dir: String::new(),
            prefix_h: String::new(), include_dirs: vec![], swift_modmaps: vec![], fb_dir: String::new(),
            fb_frameworks: vec![] }
    }
}

fn has_ios_plugins(project: &str) -> bool {
    let p = format!("{project}/.flutter-plugins-dependencies");
    let Ok(raw) = fs::read_to_string(&p) else { return false };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else { return false };
    v.get("plugins").and_then(|p| p.get("ios")).and_then(|i| i.as_array())
        .map(|a| !a.is_empty()).unwrap_or(false)
}

/// Compile all of the project's iOS plugins. `sdk` is the SDK root (patched
/// idempotently for Swift linking). Returns [`Plugins::none`] when the project
/// has no native iOS plugins (the plugin-less fast path is unchanged).
pub fn compile(project: &str, sdk: &str, root: &str, work: &str) -> Result<Plugins> {
    if !has_ios_plugins(project) {
        println!(">> no native iOS plugins - skipping plugin compile");
        return Ok(Plugins::none());
    }
    println!("== plugins: compiling native iOS plugin sources ==");
    let tools = format!("{work}/plugtools");
    fs::create_dir_all(&tools)?;
    for (name, src) in [
        ("build_plugins.py", BUILD_PLUGINS_PY),
        ("patch_storekit_signature.py", PATCH_STOREKIT_PY),
        ("patch_availability_visionos.py", PATCH_AVAIL_PY),
        ("patch_sdk_swift_overlays.py", PATCH_OVERLAYS_PY),
    ] {
        fs::write(format!("{tools}/{name}"), src)?;
    }
    // 1. idempotently complete the SDK for Swift linking (StoreKit 17.4/18.0 APIs,
    //    visionOS availability macros, Swift C-overlay .tbd stubs).
    for script in ["patch_storekit_signature.py", "patch_availability_visionos.py", "patch_sdk_swift_overlays.py"] {
        Exec::run("python", [format!("{tools}/{script}").as_str(), sdk])?
            .require(&format!("plugin SDK patch ({script})"))?;
    }
    // 2. compile every plugin (Swift/ObjC, multi-target, firebase) -> objects +
    //    staged headers/modulemaps. build_plugins.py knows the toolchain paths.
    let out = format!("{work}/plugout");
    fs::create_dir_all(&out)?;
    Exec::run("python", [format!("{tools}/build_plugins.py").as_str(), project, &out])?
        .require("plugin compile (build_plugins.py)")?;

    // 3. collect outputs.
    let obj_dir = format!("{out}/obj");
    let mut objects = vec![];
    for e in fs::read_dir(&obj_dir).with_context(|| format!("no plugin objects at {obj_dir}"))?.flatten() {
        let p = e.path();
        if p.extension().map(|x| x == "o").unwrap_or(false) {
            objects.push(p.to_string_lossy().into_owned());
        }
    }
    objects.sort();
    if objects.is_empty() {
        anyhow::bail!("plugin compile produced no objects");
    }
    let inc_dir = format!("{out}/inc");
    let mod_dir = format!("{out}/mod");
    let prefix_h = format!("{out}/objc_prefix.h");
    let include_dirs = plugin_include_dirs(project);
    let mut swift_modmaps = vec![];
    for mm in glob_modmaps(&mod_dir) {
        swift_modmaps.push(format!("-fmodule-map-file={mm}"));
    }
    let fb_dir = fb_framework_dir(root);
    let (fb_dir, fb_frameworks) = if Path::new(&fb_dir).exists() && uses_firebase(project) {
        let mut names = vec![];
        for e in fs::read_dir(&fb_dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "framework").unwrap_or(false) {
                if let Some(stem) = p.file_stem() { names.push(stem.to_string_lossy().into_owned()); }
            }
        }
        names.sort();
        (fb_dir, names)
    } else { (String::new(), vec![]) };
    println!(">> {} plugin objects compiled", objects.len());
    Ok(Plugins { has: true, objects, inc_dir, mod_dir, prefix_h, include_dirs, swift_modmaps, fb_dir, fb_frameworks })
}

fn uses_firebase(project: &str) -> bool {
    let p = format!("{project}/.flutter-plugins-dependencies");
    fs::read_to_string(&p).map(|s| s.contains("firebase")).unwrap_or(false)
}

fn glob_modmaps(mod_dir: &str) -> Vec<String> {
    let mut out = vec![];
    // mod/<plugin>/module.modulemap (Swift plugins) + mod/<name>.modulemap (SPM ObjC siblings)
    if let Ok(rd) = fs::read_dir(mod_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let mm = p.join("module.modulemap");
                if mm.exists() { out.push(mm.to_string_lossy().into_owned()); }
            } else if p.extension().map(|x| x == "modulemap").unwrap_or(false) {
                out.push(p.to_string_lossy().into_owned());
            }
        }
    }
    out
}

/// Real header search dirs for every plugin (mirrors link_app.py) so the
/// registrant's `<plugin/Header.h>` imports + their root-relative sub-imports
/// resolve against the original tree.
fn plugin_include_dirs(project: &str) -> Vec<String> {
    let mut dirs: Vec<String> = vec![];
    let p = format!("{project}/.flutter-plugins-dependencies");
    let Ok(raw) = fs::read_to_string(&p) else { return dirs };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else { return dirs };
    let Some(plugins) = v.get("plugins").and_then(|p| p.get("ios")).and_then(|i| i.as_array()) else { return dirs };
    for pl in plugins {
        let (Some(name), Some(path)) = (pl.get("name").and_then(|n| n.as_str()),
                                        pl.get("path").and_then(|p| p.as_str())) else { continue };
        let path = path.trim_end_matches(['\\', '/']);
        for sub in ["ios", "darwin"] {
            let base = format!("{path}/{sub}");
            if !Path::new(&base).exists() { continue; }
            walk_header_dirs(Path::new(&base), name, &mut dirs);
        }
    }
    dirs.sort(); dirs.dedup();
    dirs.retain(|d| Path::new(d).is_dir());
    dirs
}

fn walk_header_dirs(dir: &Path, name: &str, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let low = dir.to_string_lossy().to_lowercase();
    if ["example", "tests", ".symlinks", "macos"].iter().any(|x| low.contains(x)) { return; }
    let mut has_h = false;
    let mut subdirs = vec![];
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { subdirs.push(p); }
        else if p.extension().map(|x| x == "h").unwrap_or(false) { has_h = true; }
    }
    let bn = dir.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    if has_h { out.push(dir.to_string_lossy().into_owned()); }
    if bn == name || bn == "include" {
        if let Some(par) = dir.parent() { out.push(par.to_string_lossy().into_owned()); }
    }
    if bn == "Sources" {
        for sd in &subdirs { out.push(sd.to_string_lossy().into_owned()); }
    }
    for sd in subdirs { walk_header_dirs(&sd, name, out); }
}
