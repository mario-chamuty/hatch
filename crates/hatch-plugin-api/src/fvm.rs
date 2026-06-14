//! FVM (Flutter Version Management) detection. Shared by core and by build
//! plugins that need to resolve the project's pinned Flutter version. Moved out
//! of `hatch` core so there is a single implementation across repos.

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::Command;

/// FVM installation detector and utilities
pub struct FvmDetector;

impl FvmDetector {
    /// Check if FVM is installed on the system
    pub fn is_fvm_installed() -> bool {
        Command::new("fvm")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// Get FVM installation path
    pub fn get_fvm_path() -> Result<PathBuf> {
        // Try to find FVM executable
        if let Ok(output) = Command::new("where").arg("fvm").output() {
            if output.status.success() {
                let path_str = String::from_utf8(output.stdout)?;
                let path = PathBuf::from(path_str.trim());
                return Ok(path.parent().unwrap_or(&path).to_path_buf());
            }
        }

        // Try alternative methods for Unix-like systems
        if let Ok(output) = Command::new("which").arg("fvm").output() {
            if output.status.success() {
                let path_str = String::from_utf8(output.stdout)?;
                let path = PathBuf::from(path_str.trim());
                return Ok(path.parent().unwrap_or(&path).to_path_buf());
            }
        }

        Err(anyhow!("FVM not found in PATH"))
    }

    /// Get FVM cache directory where Flutter versions are stored
    pub fn get_fvm_cache_dir() -> Result<PathBuf> {
        // Try to get from FVM config
        if let Ok(output) = Command::new("fvm")
            .args(["config", "--cache-path"])
            .output()
        {
            if output.status.success() {
                let path_str = String::from_utf8(output.stdout)?;
                return Ok(PathBuf::from(path_str.trim()));
            }
        }

        // Fallback to default cache locations
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow!("Could not determine home directory"))?;

        // Try common FVM cache locations
        let possible_paths = vec![
            home.join("fvm").join("versions"),
            home.join(".fvm").join("versions"),
            home.join("AppData").join("Local").join("fvm").join("versions"), // Windows
        ];

        for path in possible_paths {
            if path.exists() {
                return Ok(path);
            }
        }

        Err(anyhow!("FVM cache directory not found"))
    }

    /// Check if a specific Flutter version is installed via FVM
    pub fn is_flutter_version_installed(version: &str) -> Result<bool> {
        let cache_dir = Self::get_fvm_cache_dir()?;
        let version_dir = cache_dir.join(version);
        Ok(version_dir.exists() && version_dir.join("bin").join("flutter").exists())
    }

    /// List all Flutter versions installed via FVM
    pub fn list_installed_versions() -> Result<Vec<String>> {
        let cache_dir = Self::get_fvm_cache_dir()?;

        if !cache_dir.exists() {
            return Ok(Vec::new());
        }

        let mut versions = Vec::new();

        for entry in std::fs::read_dir(cache_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(version_name) = entry.file_name().to_str() {
                    // Verify it's a valid Flutter installation
                    let flutter_bin = entry.path().join("bin").join("flutter");
                    if flutter_bin.exists() {
                        versions.push(version_name.to_string());
                    }
                }
            }
        }

        versions.sort();
        Ok(versions)
    }

    /// Get FVM version information
    pub fn get_fvm_version() -> Result<String> {
        let output = Command::new("fvm")
            .arg("--version")
            .output()
            .map_err(|_| anyhow!("Failed to execute FVM"))?;

        if !output.status.success() {
            return Err(anyhow!("FVM command failed"));
        }

        let version_str = String::from_utf8(output.stdout)?;
        Ok(version_str.trim().to_string())
    }

    /// Check if project has FVM configuration
    pub fn has_project_fvm_config(project_dir: &PathBuf) -> bool {
        project_dir.join(".fvmrc").exists() ||
        project_dir.join(".fvm").join("fvm_config.json").exists()
    }

    /// Get project's configured Flutter version from FVM
    pub fn get_project_flutter_version(project_dir: &PathBuf) -> Result<Option<String>> {
        // Check for .fvmrc file
        let fvmrc_path = project_dir.join(".fvmrc");
        if fvmrc_path.exists() {
            let content = std::fs::read_to_string(&fvmrc_path)?;
            return Ok(Some(content.trim().to_string()));
        }

        // Check for FVM config JSON
        let config_path = project_dir.join(".fvm").join("fvm_config.json");
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(version) = config.get("flutterSdkVersion").and_then(|v| v.as_str()) {
                    return Ok(Some(version.to_string()));
                }
            }
        }

        Ok(None)
    }
}
