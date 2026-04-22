//! Manifest-facing view of `hatch.lock`.
//!
//! Bridges the canonical on-disk schema (see [`crate::lockfile::parser`]) to
//! the resolver-facing types expected by warm-start and downstream
//! consumers. Provides `HatchLock::load` and `find_nearest` helpers.

use anyhow::{anyhow, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::lockfile::parser::{HatchLockfile, LockfileParser};

#[derive(Debug, Clone)]
pub struct HatchLock {
    pub schema_version: u32,
    pub generated_at: String,
    pub packages: BTreeMap<String, LockedPackage>,
    pub flutter_version: Option<String>,
    pub dart_version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LockedPackage {
    pub version: String,
    pub source: String,
    pub checksum: Option<String>,
    pub dependencies: BTreeMap<String, String>,
}

impl HatchLock {
    pub fn from_parsed(raw: HatchLockfile) -> Self {
        let mut packages = BTreeMap::new();
        for (name, pkg) in raw.packages {
            let deps: BTreeMap<String, String> =
                pkg.dependencies.into_iter().collect();
            let locked = LockedPackage {
                version: pkg.version,
                source: if pkg.registry.is_empty() {
                    "pub.dev".to_string()
                } else {
                    pkg.registry
                },
                checksum: pkg.integrity,
                dependencies: deps,
            };
            packages.insert(name, locked);
        }
        HatchLock {
            schema_version: raw.lockfile_version,
            generated_at: raw.generated_at,
            packages,
            flutter_version: raw.flutter_version,
            dart_version: raw.dart_version,
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let raw = LockfileParser::parse(path)
            .map_err(|e| anyhow!("failed to parse hatch.lock: {}", e))?;
        Ok(Self::from_parsed(raw))
    }
}

/// Walk up from `cwd` looking for the nearest `hatch.lock`.
pub fn find_nearest(cwd: &Path) -> Result<Option<PathBuf>> {
    LockfileParser::find_nearest(cwd)
}
