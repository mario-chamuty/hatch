//! The Flutter -> iOS `.ipa` build pipeline, executed entirely on Linux.
//!
//! Stages (all in the build environment):
//!   1. AOT kernel  : frontend_server (--target=flutter --aot) over the app's
//!                    package_config + lib/main.dart
//!   2. App.dylib   : gen_snapshot --snapshot_kind=app-aot-macho-dylib
//!   3. Runner      : cross-clang/ld64 compile of the native shell
//!   4. .app/.ipa   : assemble App.framework + Flutter.framework + assets, zip
//!
//! v1 targets plugin-free apps (no CocoaPods/Swift Runner). Custom launch
//! storyboards / asset catalogs (ibtool/actool) are out of scope and replaced
//! with a programmatic launch screen + minimal flutter_assets.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};

use super::runner::Runner;
use super::toolchain::Toolchain;

pub struct BuildRequest {
    /// Project directory, as a path inside the build environment.
    pub project_dir: String,
    pub app_name: String,
    pub bundle_id: String,
    pub min_os: String,
    /// `CFBundleShortVersionString` (marketing version, e.g. `1.0.0`).
    pub short_version: String,
    /// `CFBundleVersion` (build number, numeric, e.g. `18`).
    pub build_number: String,
}

pub struct BuildOutput {
    /// Unsigned `.ipa` path inside the build environment.
    pub ipa_path: String,
}

const PIPELINE: &str = include_str!("pipeline.sh");
const MKCAR: &str = include_str!("tools/mkcar.py");
const TRANSPLANT: &str = include_str!("tools/transplant.py");
// Genuine actool catalog container (Kazumi, CoreUI 918) the transplant splices
// our icon pixels into - the proven-VALID Assets.car path (App Store build 41).
const DONOR: &[u8] = include_bytes!("assets/donor_appicon.car");

/// Expand a configured toolchain root (`$HOME/iospoc`, `~/iospoc`) to a real
/// filesystem path for the native pipeline. The bash path leaves expansion to
/// the shell; the native path has no shell.
fn expand_root(root: &str) -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    if let Some(rest) = root.strip_prefix("$HOME") {
        format!("{home}{rest}")
    } else if let Some(rest) = root.strip_prefix('~') {
        format!("{home}{rest}")
    } else {
        root.to_string()
    }
}

pub fn build(runner: &Runner, tc: &Toolchain, req: &BuildRequest) -> Result<BuildOutput> {
    // Native Rust orchestration (no bash/python/lzfse/mkcar) is the DEFAULT on
    // Linux - it is verified end-to-end (Assets.car byte-identical to the proven
    // transplant; binaries pass the ITMS linker-identity/minos checks) and is
    // strictly better than the bash path, which still emits the processing-
    // wedging from-scratch mkcar catalog. Windows keeps the bash `pipeline.sh`
    // (via WSL) until the Windows-native toolchain lands (it cannot exec the
    // Linux ELF toolchain directly). Overrides: HATCH_IOS_BASH forces bash,
    // HATCH_IOS_NATIVE forces native. See `native_pipeline`.
    let use_native = if std::env::var_os("HATCH_IOS_BASH").is_some() {
        false
    } else if std::env::var_os("HATCH_IOS_NATIVE").is_some() {
        true
    } else {
        !cfg!(windows)
    };
    if use_native {
        let root = expand_root(&tc.root);
        return super::native_pipeline::build(req, &root);
    }

    let sdk = tc.ios_sdk()?;

    // Ship the Assets.car tooling into the build environment: mkcar.py (writer),
    // transplant.py (splices our pixels into a genuine actool container - the
    // proven-VALID path), and the donor catalog it needs. The donor is ~1.7 MB,
    // so it is written only when missing or size-mismatched.
    let setup = format!(
        "mkdir -p \"{root}/tools\"\n\
         printf '%s' '{mkcar}' | base64 -d > \"{root}/tools/mkcar.py\"\n\
         printf '%s' '{transplant}' | base64 -d > \"{root}/tools/transplant.py\"\n\
         DONOR=\"{root}/tools/donor_appicon.car\"\n\
         if [ ! -f \"$DONOR\" ] || [ \"$(stat -c%s \"$DONOR\" 2>/dev/null || echo 0)\" != \"{donor_len}\" ]; then\n\
           printf '%s' '{donor}' | base64 -d > \"$DONOR\"\n\
         fi\n",
        root = tc.root,
        mkcar = STANDARD.encode(MKCAR),
        transplant = STANDARD.encode(TRANSPLANT),
        donor = STANDARD.encode(DONOR),
        donor_len = DONOR.len(),
    );
    runner
        .exec(&setup)
        .context("installing Assets.car tooling")?
        .require()?;
    let safe: String = req
        .app_name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let ipa_path = format!("{}/out/{}.ipa", tc.root, safe);

    let script = PIPELINE
        .replace("@@ROOT@@", &tc.root)
        .replace("@@PROJ@@", &req.project_dir)
        .replace("@@SDK@@", &sdk)
        .replace("@@APPNAME@@", &req.app_name)
        .replace("@@BUNDLEID@@", &req.bundle_id)
        .replace("@@MINOS@@", &req.min_os)
        .replace("@@SHORTVER@@", &req.short_version)
        .replace("@@BUILDVER@@", &req.build_number)
        .replace("@@SAFE@@", &safe);

    let out = runner.exec(&script).context("iOS build pipeline")?;
    if !out.ok() {
        anyhow::bail!(
            "iOS build failed (exit {}):\n--- stderr ---\n{}\n--- stdout ---\n{}",
            out.status,
            out.stderr.trim(),
            out.stdout.trim()
        );
    }
    // Surface pipeline progress to the user.
    for line in out.stdout.lines() {
        if line.starts_with("== ") || line.starts_with(">>") {
            println!("   {}", line.trim());
        }
    }
    Ok(BuildOutput { ipa_path })
}
