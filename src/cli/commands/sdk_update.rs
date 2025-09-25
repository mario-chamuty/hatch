use anyhow::{Context, Result};
use colored::Colorize;
use std::process::Command;
use std::path::PathBuf;

use crate::manifest::parser::ManifestParser;
use crate::fvm::manager::FvmManager;

pub async fn execute(flutter_version: Option<String>, dart_version: Option<String>) -> Result<()> {
    println!("🎯 {} SDK versions", "Updating".cyan().bold());

    let mut manifest = ManifestParser::parse_with_overrides("hatch.json", Some("hatch.local.json"))?;

    let mut updated = false;
    let flutter_was_updated;

    // Update Flutter SDK
    if let Some(version) = flutter_version.clone() {
        flutter_was_updated = true;
        let old_version = manifest.sdk.flutter.clone().unwrap_or_else(|| "none".to_string());

        if version == "latest" || version == "stable" {
            // Get latest stable Flutter version
            let latest = get_latest_flutter_version().await?;
            manifest.sdk.flutter = Some(latest.clone());
            println!("   Flutter: {} → {}", old_version.yellow(), latest.green());
        } else {
            // Use specific version
            manifest.sdk.flutter = Some(version.clone());
            println!("   Flutter: {} → {}", old_version.yellow(), version.green());
        }
        updated = true;
    } else {
        flutter_was_updated = false;
    }

    // Update Dart SDK
    if let Some(version) = dart_version {
        let old_version = manifest.sdk.dart.clone().unwrap_or_else(|| "none".to_string());

        if version == "latest" {
            // Get latest Dart version constraint
            let latest = ">=3.0.0 <4.0.0".to_string();
            manifest.sdk.dart = Some(latest.clone());
            println!("   Dart: {} → {}", old_version.yellow(), latest.green());
        } else {
            // Use specific version constraint
            manifest.sdk.dart = Some(version.clone());
            println!("   Dart: {} → {}", old_version.yellow(), version.green());
        }
        updated = true;
    }

    // If no specific versions provided, show current and available updates
    if !updated {
        println!("\n📊 Current SDK versions:");

        if let Some(flutter) = &manifest.sdk.flutter {
            println!("   Flutter: {}", flutter.green());

            // Check for newer version
            let latest = get_latest_flutter_version().await?;
            if latest != *flutter && !flutter.contains("stable") && !flutter.contains("beta") {
                println!("   {} Latest stable: {}", "↑".cyan(), latest.cyan());
                println!("\n   Run {} to update", format!("hatch sdk-update --flutter {}", latest).cyan());
            }
        } else {
            println!("   Flutter: {}", "not set".yellow());
        }

        if let Some(dart) = &manifest.sdk.dart {
            println!("   Dart: {}", dart.green());
        } else {
            println!("   Dart: {}", "not set".yellow());
        }

        return Ok(());
    }

    // Save updated manifest
    let manifest_content = serde_json::to_string_pretty(&manifest)?;
    std::fs::write("hatch.json", manifest_content)?;

    println!("\n✅ {} updated successfully!", "SDK versions".green().bold());

    // Install/setup the new Flutter version if needed
    if flutter_was_updated {
        if let Some(flutter) = &manifest.sdk.flutter {
            println!("\n🔧 Setting up Flutter {}...", flutter);
            let fvm_manager = FvmManager::new(PathBuf::from("."));

            if !flutter.contains("stable") && !flutter.contains("beta") && !flutter.contains("dev") {
                // It's a specific version, use FVM
                match fvm_manager.setup_project_flutter(flutter).await {
                    Ok(_) => println!("✅ Flutter {} is ready", flutter),
                    Err(e) => println!("⚠️  Could not setup Flutter {}: {}", flutter, e),
                }
            }
        }
    }

    println!("\n💡 Run {} to install dependencies with the new SDK", "hatch install".cyan());

    Ok(())
}

async fn get_latest_flutter_version() -> Result<String> {
    // Try to get from FVM first
    let output = Command::new("fvm")
        .args(&["releases", "--json"])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse FVM releases JSON to find latest stable
            if let Ok(releases) = serde_json::from_str::<serde_json::Value>(&stdout) {
                if let Some(channels) = releases.get("channels") {
                    if let Some(stable) = channels.get("stable") {
                        if let Some(version) = stable.as_str() {
                            return Ok(version.to_string());
                        }
                    }
                }
            }
        }
    }

    // Fallback: try flutter channels command
    let output = Command::new("flutter")
        .args(&["channel"])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse flutter channel output to find stable version
            for line in stdout.lines() {
                if line.contains("* stable") {
                    // Extract version from line like "* stable    channel/stable   3.16.9   ..."
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 {
                        // Find the version number (starts with a digit)
                        for part in &parts[2..] {
                            if part.chars().next().map_or(false, |c| c.is_numeric()) {
                                return Ok(part.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Default to stable channel if we can't determine specific version
    Ok("stable".to_string())
}

pub async fn execute_check() -> Result<()> {
    println!("🔍 {} for SDK updates", "Checking".cyan().bold());

    let manifest = ManifestParser::parse_with_overrides("hatch.json", Some("hatch.local.json"))?;

    println!("\n📊 Current SDK configuration:");

    // Check Flutter
    if let Some(flutter) = &manifest.sdk.flutter {
        println!("   Flutter: {}", flutter.green());

        if !flutter.contains("stable") && !flutter.contains("beta") && !flutter.contains("dev") {
            // It's a specific version, check for updates
            let latest = get_latest_flutter_version().await?;

            if latest != *flutter {
                println!("   {} Update available: {}", "↑".cyan(), latest.cyan());
                println!("\n   Run {} to update", format!("hatch sdk-update --flutter {}", latest).cyan());
            } else {
                println!("   ✓ Already on latest stable version");
            }
        } else {
            println!("   ✓ Using {} channel", flutter);
        }
    } else {
        println!("   Flutter: {} (not configured)", "none".yellow());
        println!("   Run {} to set Flutter version", "hatch sdk-update --flutter stable".cyan());
    }

    // Check Dart
    if let Some(dart) = &manifest.sdk.dart {
        println!("   Dart: {}", dart.green());

        // Check if it's an old constraint
        if dart.contains("2.") || dart.starts_with("<3") {
            println!("   {} Consider updating to Dart 3+", "↑".yellow());
            println!("   Run {}", "hatch sdk-update --dart \">=3.0.0 <4.0.0\"".cyan());
        } else {
            println!("   ✓ Using modern Dart version");
        }
    } else {
        println!("   Dart: {} (not configured)", "none".yellow());
        println!("   Run {} to set Dart constraint", "hatch sdk-update --dart \">=3.0.0 <4.0.0\"".cyan());
    }

    Ok(())
}