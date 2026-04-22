use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use log::{debug, info, warn};

use super::paths::CachePaths;
use super::extractor::PackageExtractor;

/// Metadata about a cached package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPackageInfo {
    pub name: String,
    pub version: String,
    pub registry: String,
    pub cached_at: chrono::DateTime<chrono::Utc>,
    pub size: u64,
    pub dependencies: HashMap<String, String>,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub modified: chrono::DateTime<chrono::Local>,
}

/// Manages package storage in the cache
pub struct PackageStorage;

impl PackageStorage {
    /// Store a package in the cache
    pub fn store_package(
        registry: &str,
        name: &str,
        version: &str,
        archive_path: &Path,
    ) -> Result<PathBuf> {
        if crate::cli::verbosity::is_ultra_verbose() {
            info!("Storing package {}@{} from {}", name, version, registry);
        } else {
            debug!("Storing package {}@{} from {}", name, version, registry);
        }

        // Get destination directory
        let package_dir = CachePaths::package_dir(registry, name, version)?;

        // Check if already cached
        if package_dir.exists() {
            debug!("Package already cached: {}", package_dir.display());
            return Ok(package_dir);
        }

        // Extract package
        match PackageExtractor::extract_package(archive_path, &package_dir) {
            Ok(_) => {
                if crate::cli::verbosity::is_ultra_verbose() {
                    info!("Stored package at: {}", package_dir.display());
                }

                // Write metadata
                Self::write_metadata(registry, name, version, &package_dir)?;

                Ok(package_dir)
            }
            Err(e) => {
                // Clean up on failure
                PackageExtractor::cleanup_failed_extraction(&package_dir)?;
                Err(e)
            }
        }
    }

    /// Get path to cached package
    pub fn get_package_path(
        registry: &str,
        name: &str,
        version: &str,
    ) -> Result<Option<PathBuf>> {
        let package_dir = CachePaths::package_dir(registry, name, version)?;

        if package_dir.exists() {
            // Verify package integrity
            if Self::verify_package(&package_dir)? {
                Ok(Some(package_dir))
            } else {
                warn!("Package {}@{} is corrupted, removing", name, version);
                std::fs::remove_dir_all(&package_dir)?;
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    /// List all cached packages
    pub fn list_cached_packages() -> Result<Vec<CachedPackageInfo>> {
        let packages_dir = CachePaths::packages_dir()?;
        let mut packages = Vec::new();

        if !packages_dir.exists() {
            return Ok(packages);
        }

        // Iterate through registries
        for registry_entry in std::fs::read_dir(&packages_dir)? {
            let registry_entry = registry_entry?;
            if !registry_entry.path().is_dir() {
                continue;
            }

            let registry_name = registry_entry.file_name().to_string_lossy().to_string();

            // Iterate through packages
            for package_entry in std::fs::read_dir(registry_entry.path())? {
                let package_entry = package_entry?;
                if !package_entry.path().is_dir() {
                    continue;
                }

                let package_name = package_entry.file_name().to_string_lossy().to_string();

                // Iterate through versions
                for version_entry in std::fs::read_dir(package_entry.path())? {
                    let version_entry = version_entry?;
                    if !version_entry.path().is_dir() {
                        continue;
                    }

                    let version = version_entry.file_name().to_string_lossy().to_string();

                    // Read metadata if exists
                    if let Ok(info) = Self::read_metadata(&registry_name, &package_name, &version) {
                        packages.push(info);
                    }
                }
            }
        }

        Ok(packages)
    }

    /// Clear the entire cache
    pub fn clear_cache() -> Result<()> {
        let cache_root = CachePaths::root()?;
        if cache_root.exists() {
            std::fs::remove_dir_all(&cache_root)?;
            info!("Cache cleared");
        }
        Ok(())
    }

    /// Clear a specific package from cache
    pub fn clear_package(registry: &str, name: &str, version: Option<&str>) -> Result<()> {
        if let Some(ver) = version {
            // Clear specific version
            let package_dir = CachePaths::package_dir(registry, name, ver)?;
            if package_dir.exists() {
                std::fs::remove_dir_all(&package_dir)?;
                info!("Cleared {}@{} from cache", name, ver);
            }
        } else {
            // Clear all versions of the package
            let packages_dir = CachePaths::packages_dir()?;
            let package_base = packages_dir.join(registry).join(name);
            if package_base.exists() {
                std::fs::remove_dir_all(&package_base)?;
                info!("Cleared all versions of {} from cache", name);
            }
        }
        Ok(())
    }

    /// Calculate cache size
    pub fn get_cache_size() -> Result<u64> {
        let cache_root = CachePaths::root()?;
        if !cache_root.exists() {
            return Ok(0);
        }

        let mut total_size = 0;
        Self::calculate_dir_size(&cache_root, &mut total_size)?;
        Ok(total_size)
    }

    /// Verify package integrity
    fn verify_package(package_dir: &Path) -> Result<bool> {
        // Check for pubspec.yaml
        let pubspec = package_dir.join("pubspec.yaml");
        Ok(pubspec.exists())
    }

    /// Write package metadata
    fn write_metadata(registry: &str, name: &str, version: &str, package_dir: &Path) -> Result<()> {
        let metadata = CachedPackageInfo {
            name: name.to_string(),
            version: version.to_string(),
            registry: registry.to_string(),
            cached_at: chrono::Utc::now(),
            size: Self::calculate_package_size(package_dir)?,
            dependencies: Self::extract_dependencies(package_dir)?,
            path: package_dir.to_path_buf(),
            modified: chrono::Local::now(),
        };

        let metadata_path = package_dir.join(".hatch_metadata.json");
        let json = serde_json::to_string_pretty(&metadata)?;
        std::fs::write(metadata_path, json)?;

        Ok(())
    }

    /// Read package metadata. Looks at the per-package sidecar first, and
    /// falls back to the central `~/.hatch/cache/index.json` (populated by
    /// `hatch cache prune --aggressive`).
    fn read_metadata(registry: &str, name: &str, version: &str) -> Result<CachedPackageInfo> {
        let package_dir = CachePaths::package_dir(registry, name, version)?;
        let metadata_path = package_dir.join(".hatch_metadata.json");

        let (content, mtime_src) = if metadata_path.exists() {
            let c = std::fs::read_to_string(&metadata_path)?;
            (c, Some(metadata_path.clone()))
        } else {
            // Central index fallback.
            let index_path = CachePaths::root()?.join("index.json");
            let index_content = std::fs::read_to_string(&index_path)?;
            let parsed: serde_json::Value = serde_json::from_str(&index_content)?;
            let key = format!("{registry}/{name}/{version}");
            let entry = parsed
                .get(&key)
                .ok_or_else(|| anyhow!("metadata missing for {key} in index.json"))?;
            (entry.to_string(), Some(index_path))
        };

        let mut metadata: CachedPackageInfo = serde_json::from_str(&content)?;

        // Add the path and modified time
        metadata.path = package_dir;
        metadata.modified = match mtime_src.as_ref().and_then(|p| std::fs::metadata(p).ok()) {
            Some(meta) => meta
                .modified()
                .map(chrono::DateTime::from)
                .unwrap_or_else(|_| chrono::Local::now()),
            None => chrono::Local::now(),
        };

        Ok(metadata)
    }

    /// Calculate directory size recursively
    fn calculate_dir_size(dir: &Path, total: &mut u64) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                Self::calculate_dir_size(&path, total)?;
            } else {
                *total += entry.metadata()?.len();
            }
        }
        Ok(())
    }

    /// Calculate package size
    fn calculate_package_size(package_dir: &Path) -> Result<u64> {
        let mut size = 0;
        Self::calculate_dir_size(package_dir, &mut size)?;
        Ok(size)
    }

    /// Extract dependencies from pubspec.yaml
    fn extract_dependencies(package_dir: &Path) -> Result<HashMap<String, String>> {
        let pubspec_path = package_dir.join("pubspec.yaml");

        if !pubspec_path.exists() {
            return Ok(HashMap::new());
        }

        let content = std::fs::read_to_string(pubspec_path)?;
        let yaml: serde_yaml::Value = serde_yaml::from_str(&content)?;

        let mut deps = HashMap::new();

        if let serde_yaml::Value::Mapping(root) = yaml {
            if let Some(serde_yaml::Value::Mapping(dependencies)) = root.get("dependencies") {
                for (key, value) in dependencies {
                    if let serde_yaml::Value::String(name) = key {
                        let version = match value {
                            serde_yaml::Value::String(v) => v.clone(),
                            serde_yaml::Value::Mapping(m) => {
                                if let Some(serde_yaml::Value::String(v)) = m.get("version") {
                                    v.clone()
                                } else {
                                    "any".to_string()
                                }
                            }
                            _ => "any".to_string(),
                        };
                        deps.insert(name.clone(), version);
                    }
                }
            }
        }

        Ok(deps)
    }
}