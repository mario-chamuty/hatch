use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use log::{debug, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedPackage {
    pub version: String,
    pub resolved: String,
    pub integrity: Option<String>,
    pub dependencies: HashMap<String, String>,
    pub dev: bool,
    #[serde(default)]
    pub registry: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HatchLockfile {
    pub lockfile_version: u32,
    pub packages: HashMap<String, LockedPackage>,
    pub flutter_version: Option<String>,
    pub dart_version: Option<String>,
    pub generated_at: String,
    pub hatch_version: String,
}

impl Default for HatchLockfile {
    fn default() -> Self {
        Self {
            lockfile_version: 1,
            packages: HashMap::new(),
            flutter_version: None,
            dart_version: None,
            generated_at: chrono::Utc::now().to_rfc3339(),
            hatch_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

pub struct LockfileParser;

impl LockfileParser {
    pub fn parse(path: &Path) -> Result<HatchLockfile> {
        info!("Parsing lockfile: {}", path.display());

        if !path.exists() {
            debug!("Lockfile doesn't exist, returning empty lockfile");
            return Ok(HatchLockfile::default());
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("Failed to read lockfile: {}", e))?;

        let lockfile: HatchLockfile = serde_yaml::from_str(&content)
            .map_err(|e| anyhow!("Failed to parse lockfile: {}", e))?;

        if lockfile.lockfile_version > 1 {
            return Err(anyhow!(
                "Lockfile version {} is not supported by this version of hatch",
                lockfile.lockfile_version
            ));
        }

        info!("Loaded {} locked packages", lockfile.packages.len());
        Ok(lockfile)
    }

    pub fn has_package(lockfile: &HatchLockfile, name: &str, version: &str) -> bool {
        lockfile.packages
            .get(name)
            .map(|pkg| pkg.version == version)
            .unwrap_or(false)
    }

    pub fn get_packages(lockfile: &HatchLockfile) -> Vec<(String, String)> {
        lockfile.packages
            .iter()
            .map(|(name, pkg)| (name.clone(), pkg.version.clone()))
            .collect()
    }

    pub fn merge(existing: &HatchLockfile, new: HatchLockfile) -> HatchLockfile {
        let mut merged = new;

        for (name, existing_pkg) in &existing.packages {
            if let Some(new_pkg) = merged.packages.get_mut(name) {
                if new_pkg.version == existing_pkg.version && existing_pkg.integrity.is_some() {
                    new_pkg.integrity = existing_pkg.integrity.clone();
                }
            }
        }

        merged
    }
}