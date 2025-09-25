use anyhow::{anyhow, Result};
use std::path::PathBuf;

use super::detector::FvmDetector;
use super::installer::FvmInstaller;

/// High-level FVM management interface
pub struct FvmManager {
    project_dir: PathBuf,
}

impl FvmManager {
    pub fn new(project_dir: PathBuf) -> Self {
        Self { project_dir }
    }

    /// Setup FVM for the project based on manifest requirements
    pub async fn setup_project_flutter(&self, required_version: &str) -> Result<()> {
        println!("Setting up Flutter {} for project", required_version);

        // Check if FVM is available
        if !FvmDetector::is_fvm_installed() {
            println!("FVM is not installed.");
            println!("   Install FVM to enable automatic Flutter version management:");
            println!("   https://fvm.app/docs/getting_started/installation");
            return Ok(());
        }

        // Display FVM status
        match FvmDetector::get_fvm_version() {
            Ok(version) => println!("FVM version: {}", version),
            Err(_) => println!("Could not determine FVM version"),
        }

        // Check if the required version is already installed and set
        if let Ok(Some(current_version)) = FvmDetector::get_project_flutter_version(&self.project_dir) {
            if current_version == required_version {
                println!("Project is already using Flutter {}", required_version);
                return Ok(());
            } else {
                println!("Project currently using Flutter {}, switching to {}",
                    current_version, required_version);
            }
        }

        // Auto-install if needed
        let installed = FvmInstaller::auto_install_from_manifest(required_version).await?;

        if installed {
            println!("Flutter {} is ready for use", required_version);
        } else {
            println!("Could not automatically setup Flutter {}", required_version);
            println!("   Run 'fvm install {}' manually", required_version);
        }

        Ok(())
    }

    /// List all installed Flutter versions
    pub fn list_versions(&self) -> Result<()> {
        if !FvmDetector::is_fvm_installed() {
            return Err(anyhow!("FVM is not installed"));
        }

        println!("Installed Flutter versions:");

        let installed_versions = FvmDetector::list_installed_versions()?;

        if installed_versions.is_empty() {
            println!("   No versions installed");
            return Ok(());
        }

        // Get current project version if available
        let current_project_version = FvmDetector::get_project_flutter_version(&self.project_dir)
            .unwrap_or(None);

        for version in &installed_versions {
            let marker = if Some(version) == current_project_version.as_ref() {
                " (current)"
            } else {
                ""
            };
            println!("   • {}{}", version, marker);
        }

        if let Some(current) = current_project_version {
            if !installed_versions.contains(&current) {
                println!("   Current project version {} is not installed", current);
            }
        } else {
            println!("   No version set for this project");
        }

        Ok(())
    }

    /// Use a specific Flutter version for the project
    pub fn use_version(&self, version: &str) -> Result<()> {
        if !FvmDetector::is_fvm_installed() {
            return Err(anyhow!("FVM is not installed"));
        }

        FvmInstaller::use_flutter_version(version)?;
        Ok(())
    }

    /// Install a Flutter version
    pub async fn install_version(&self, version: &str) -> Result<()> {
        if !FvmDetector::is_fvm_installed() {
            return Err(anyhow!("FVM is not installed"));
        }

        FvmInstaller::install_flutter_version(version).await?;
        Ok(())
    }

    /// Sync Flutter version with manifest
    pub async fn sync_with_manifest(&self, manifest_flutter_version: &str) -> Result<()> {
        println!("Syncing Flutter version with manifest");

        // Check current project configuration
        let current_version = FvmDetector::get_project_flutter_version(&self.project_dir)?;

        if let Some(current) = current_version {
            if current == manifest_flutter_version {
                println!("Project Flutter version is already in sync ({})", current);
                return Ok(());
            } else {
                println!("Updating project Flutter version: {} -> {}",
                    current, manifest_flutter_version);
            }
        } else {
            println!("Setting Flutter version for project: {}", manifest_flutter_version);
        }

        // Setup the required version
        self.setup_project_flutter(manifest_flutter_version).await?;

        Ok(())
    }

    /// Get project information
    pub fn get_project_info(&self) -> Result<ProjectFlutterInfo> {
        let fvm_installed = FvmDetector::is_fvm_installed();
        let fvm_version = if fvm_installed {
            FvmDetector::get_fvm_version().ok()
        } else {
            None
        };

        let project_version = if fvm_installed {
            FvmDetector::get_project_flutter_version(&self.project_dir)?
        } else {
            None
        };

        let has_fvm_config = FvmDetector::has_project_fvm_config(&self.project_dir);

        Ok(ProjectFlutterInfo {
            fvm_installed,
            fvm_version,
            project_version,
            has_fvm_config,
        })
    }
}

#[derive(Debug)]
pub struct ProjectFlutterInfo {
    pub fvm_installed: bool,
    pub fvm_version: Option<String>,
    pub project_version: Option<String>,
    pub has_fvm_config: bool,
}