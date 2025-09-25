use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use log::{debug, info};

use crate::registry::traits::PackageVersion;
use crate::cache::paths::CachePaths;

/// Global metadata cache that persists between runs
pub struct MetadataCache {
    memory_cache: Arc<RwLock<HashMap<String, Vec<PackageVersion>>>>,
}

impl MetadataCache {
    pub fn new() -> Self {
        Self {
            memory_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load cached metadata from disk
    pub async fn load_from_disk(&self) -> Result<()> {
        let metadata_dir = CachePaths::metadata_dir()?;
        if !metadata_dir.exists() {
            return Ok(());
        }

        let mut cache = self.memory_cache.write().await;

        // Try to load all metadata files
        if let Ok(entries) = std::fs::read_dir(&metadata_dir) {
            for entry in entries {
                if let Ok(entry) = entry {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("json") {
                        if let Ok(contents) = std::fs::read_to_string(&path) {
                            if let Ok(metadata) = serde_json::from_str::<CachedMetadata>(&contents) {
                                // Check if metadata is still fresh (24 hours)
                                let age = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)?
                                    .as_secs() - metadata.timestamp;

                                if age < 86400 { // 24 hours
                                    cache.insert(metadata.name.clone(), metadata.versions);
                                    debug!("Loaded cached metadata for {}", metadata.name);
                                }
                            }
                        }
                    }
                }
            }
        }

        info!("Loaded {} packages from metadata cache", cache.len());
        Ok(())
    }

    /// Save metadata to disk for persistence
    pub async fn save_to_disk(&self, package: &str, versions: &[PackageVersion]) -> Result<()> {
        let metadata_dir = CachePaths::metadata_dir()?;
        std::fs::create_dir_all(&metadata_dir)?;

        let metadata = CachedMetadata {
            name: package.to_string(),
            versions: versions.to_vec(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        };

        let path = metadata_dir.join(format!("{}.json", package));
        let json = serde_json::to_string(&metadata)?;
        std::fs::write(path, json)?;

        Ok(())
    }

    pub fn get_cache(&self) -> Arc<RwLock<HashMap<String, Vec<PackageVersion>>>> {
        self.memory_cache.clone()
    }

    pub async fn get(&self, package: &str) -> Option<Vec<PackageVersion>> {
        let cache = self.memory_cache.read().await;
        cache.get(package).cloned()
    }

    pub async fn insert(&self, package: String, versions: Vec<PackageVersion>) {
        let mut cache = self.memory_cache.write().await;
        cache.insert(package.clone(), versions.clone());

        // Save to disk in background
        let package_clone = package.clone();
        let versions_clone = versions.clone();
        tokio::spawn(async move {
            let cache = MetadataCache::new();
            let _ = cache.save_to_disk(&package_clone, &versions_clone).await;
        });
    }

    pub async fn contains(&self, package: &str) -> bool {
        let cache = self.memory_cache.read().await;
        cache.contains_key(package)
    }
}

#[derive(Serialize, Deserialize)]
struct CachedMetadata {
    name: String,
    versions: Vec<PackageVersion>,
    timestamp: u64,
}

impl Default for MetadataCache {
    fn default() -> Self {
        Self::new()
    }
}