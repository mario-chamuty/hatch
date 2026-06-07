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

use super::runner::Runner;
use super::toolchain::Toolchain;

pub struct BuildRequest {
    /// Project directory, as a path inside the build environment.
    pub project_dir: String,
    pub app_name: String,
    pub bundle_id: String,
    pub min_os: String,
}

pub struct BuildOutput {
    /// Unsigned `.ipa` path inside the build environment.
    pub ipa_path: String,
}

const PIPELINE: &str = include_str!("pipeline.sh");

pub fn build(runner: &Runner, tc: &Toolchain, req: &BuildRequest) -> Result<BuildOutput> {
    let sdk = tc.ios_sdk()?;
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
