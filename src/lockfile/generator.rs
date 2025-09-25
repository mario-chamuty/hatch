use anyhow::{Result};
use std::path::Path;
use std::collections::HashMap;
use log::info;

use super::parser::{HatchLockfile, LockedPackage};
use crate::resolver::sat::ResolvedPackage;
use crate::manifest::schema::HatchManifest;

pub struct LockfileGenerator;

impl LockfileGenerator {
    pub fn generate(
        manifest: &HatchManifest,
        resolved_packages: &[ResolvedPackage],
        path: &Path,
    ) -> Result<()> {
        info!("Generating lockfile with {} packages", resolved_packages.len());

        let mut packages = HashMap::new();

        for package in resolved_packages {
            if package.name == "flutter" {
                continue;
            }

            let locked = LockedPackage {
                version: package.version.clone(),
                resolved: format!("pub.dev/{}@{}", package.name, package.version),
                integrity: None,
                dependencies: package.dependencies.clone(),
                dev: false,
                registry: "pub.dev".to_string(),
            };

            packages.insert(package.name.clone(), locked);
        }

        let lockfile = HatchLockfile {
            lockfile_version: 1,
            packages,
            flutter_version: manifest.sdk.flutter.clone(),
            dart_version: manifest.sdk.dart.clone(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            hatch_version: env!("CARGO_PKG_VERSION").to_string(),
        };

        let yaml = serde_yaml::to_string(&lockfile)?;
        std::fs::write(path, yaml)?;

        info!("Lockfile written to {}", path.display());
        Ok(())
    }

    pub fn is_up_to_date(manifest: &HatchManifest, lockfile_path: &Path) -> bool {
        if !lockfile_path.exists() {
            return false;
        }

        super::parser::LockfileParser::parse(lockfile_path).is_ok()
    }
}