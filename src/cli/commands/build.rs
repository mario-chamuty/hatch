use anyhow::{anyhow, Result};
use log::info;
use std::process::Command;

use crate::manifest::parser::ManifestParser;
use crate::fvm::detector::FvmDetector;

pub async fn execute(platform: String, profile: Option<String>, channel: Option<String>) -> Result<()> {
    info!("Building for platform: {}", platform);

    let build_profile = profile.unwrap_or_else(|| "release".to_string());

    // Parse manifest for scripts and config
    let manifest = ManifestParser::parse_with_overrides("hatch.json", Some("hatch.local.json"))?;

    // Run install first to ensure deps are up to date
    println!("📦 Ensuring dependencies are installed...");
    super::install_smart::execute(None).await?;

    // Map platform to Flutter build target
    let flutter_target = match platform.as_str() {
        "android" | "apk" => "apk",
        "appbundle" | "aab" => "appbundle",
        "ios" => "ios",
        "web" => "web",
        "windows" => "windows",
        "macos" => "macos",
        "linux" => "linux",
        "ipa" => "ipa",
        other => return Err(anyhow!(
            "Unknown platform '{}'. Supported: android, apk, appbundle, aab, ios, ipa, web, windows, macos, linux",
            other
        )),
    };

    // Determine flutter command (fvm or direct)
    let project_dir = std::env::current_dir()?;
    let use_fvm = FvmDetector::is_fvm_installed() && FvmDetector::has_project_fvm_config(&project_dir);
    let (cmd, base_args) = if use_fvm {
        ("fvm", vec!["flutter", "build", flutter_target])
    } else {
        ("flutter", vec!["build", flutter_target])
    };

    // Build args
    let mut args = base_args;

    // Add profile flag
    match build_profile.as_str() {
        "release" => args.push("--release"),
        "debug" => args.push("--debug"),
        "profile" => args.push("--profile"),
        other => {
            println!("⚠️  Unknown profile '{}', using --release", other);
            args.push("--release");
        }
    }

    println!("🔨 Building {} ({})...", platform, build_profile);
    println!("   Running: {} {}", cmd, args.join(" "));

    let status = Command::new(cmd)
        .args(&args)
        .status()?;

    if status.success() {
        println!("✅ Build complete!");
    } else {
        return Err(anyhow!("Build failed with exit code: {}", status.code().unwrap_or(-1)));
    }

    Ok(())
}
