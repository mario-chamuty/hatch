use anyhow::{anyhow, Result};
use std::process::Command;
use indicatif::{ProgressBar, ProgressStyle};
use log::{info, warn};

use super::detector::FvmDetector;

/// FVM Flutter version installer
pub struct FvmInstaller;

impl FvmInstaller {
    /// Install a specific Flutter version via FVM
    pub async fn install_flutter_version(version: &str) -> Result<()> {
        info!("Installing Flutter version {} via FVM", version);

        if !FvmDetector::is_fvm_installed() {
            return Err(anyhow!("FVM is not installed. Please install FVM first:"));
        }

        // Check if version is already installed
        if FvmDetector::is_flutter_version_installed(version)? {
            println!("Flutter {} is already installed", version);
            return Ok(());
        }

        println!("Installing Flutter version {}", version);
        println!("   This may take a few minutes...");

        // Create a progress bar
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap()
        );
        pb.set_message(format!("Installing Flutter {}", version));
        pb.enable_steady_tick(std::time::Duration::from_millis(100));

        // Execute FVM install command
        let version_owned = version.to_string();
        let output = tokio::task::spawn_blocking(move || {
            Command::new("fvm")
                .args(&["install", &version_owned])
                .output()
        }).await??;

        pb.finish_and_clear();

        if output.status.success() {
            println!("Flutter {} installed successfully", version);

            // Verify installation
            if FvmDetector::is_flutter_version_installed(version)? {
                println!("Installation verified");
            } else {
                warn!("Installation may not have completed properly");
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to install Flutter {}: {}", version, stderr));
        }

        Ok(())
    }

    /// Remove a Flutter version installed via FVM
    pub fn remove_flutter_version(version: &str) -> Result<()> {
        info!("Removing Flutter version {} via FVM", version);

        if !FvmDetector::is_fvm_installed() {
            return Err(anyhow!("FVM is not installed"));
        }

        println!("Removing Flutter version {}", version);

        let output = Command::new("fvm")
            .args(&["remove", version])
            .output()
            .map_err(|e| anyhow!("Failed to execute FVM: {}", e))?;

        if output.status.success() {
            println!("Flutter {} removed successfully", version);
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to remove Flutter {}: {}", version, stderr));
        }

        Ok(())
    }

    /// Set Flutter version for current project via FVM
    pub fn use_flutter_version(version: &str) -> Result<()> {
        info!("Setting Flutter version {} for current project", version);

        if !FvmDetector::is_fvm_installed() {
            return Err(anyhow!("FVM is not installed"));
        }

        // Check if version is installed
        if !FvmDetector::is_flutter_version_installed(version)? {
            return Err(anyhow!(
                "Flutter version {} is not installed. Run 'hatch fvm install {}' first",
                version, version
            ));
        }

        println!("Setting Flutter version {} for this project", version);

        let output = Command::new("fvm")
            .args(&["use", version])
            .output()
            .map_err(|e| anyhow!("Failed to execute FVM: {}", e))?;

        if output.status.success() {
            println!("Project now using Flutter {}", version);
            println!("   Use 'fvm flutter' to run Flutter commands");
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to set Flutter version {}: {}", version, stderr));
        }

        Ok(())
    }

    /// Get list of available Flutter versions from FVM
    pub fn list_available_versions() -> Result<Vec<String>> {
        if !FvmDetector::is_fvm_installed() {
            return Err(anyhow!("FVM is not installed"));
        }

        let output = Command::new("fvm")
            .args(&["releases"])
            .output()
            .map_err(|e| anyhow!("Failed to execute FVM: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to list available versions: {}", stderr));
        }

        let stdout = String::from_utf8(output.stdout)?;
        let mut versions = Vec::new();

        // Parse the output to extract version numbers
        for line in stdout.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with("Releases available on Flutter SDK:") {
                // Extract version number from line like "3.24.2" or "3.24.2 (stable)"
                if let Some(version) = line.split_whitespace().next() {
                    if version.chars().next().unwrap_or('_').is_numeric() {
                        versions.push(version.to_string());
                    }
                }
            }
        }

        Ok(versions)
    }

    /// Auto-install Flutter version from manifest
    pub async fn auto_install_from_manifest(flutter_version: &str) -> Result<bool> {
        if !FvmDetector::is_fvm_installed() {
            println!("FVM is not installed. Flutter version management will not be available.");
            return Ok(false);
        }

        if FvmDetector::is_flutter_version_installed(flutter_version)? {
            println!("Flutter {} is already available", flutter_version);
            return Ok(true);
        }

        println!("Manifest requires Flutter {}", flutter_version);
        println!("Auto-installing Flutter {} via FVM...", flutter_version);

        match Self::install_flutter_version(flutter_version).await {
            Ok(_) => {
                Self::use_flutter_version(flutter_version)?;
                Ok(true)
            },
            Err(e) => {
                warn!("Failed to auto-install Flutter {}: {}", flutter_version, e);
                Ok(false)
            }
        }
    }
}