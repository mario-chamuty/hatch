use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::collections::HashMap;
use log::{debug, info, warn};

use super::downloader::{PackageDownloader, ALLOW_UNCHECKSUMMED_SENTINEL};
use super::storage::PackageStorage;
use super::paths::CachePaths;

#[derive(Clone)]
pub struct CacheManager {
    downloader: PackageDownloader,
}

impl CacheManager {
    pub fn new() -> Self {
        Self {
            downloader: PackageDownloader::new(),
        }
    }

    /// Get a package, requiring a checksum. Pass
    /// [`ALLOW_UNCHECKSUMMED_SENTINEL`] to explicitly bypass.
    pub async fn get_package_with_checksum(
        &self,
        registry: &str,
        name: &str,
        version: &str,
        checksum: &str,
    ) -> Result<PathBuf> {
        if let Some(cached_path) = PackageStorage::get_package_path(registry, name, version)? {
            info!("Using cached package: {}@{}", name, version);
            return Ok(cached_path);
        }

        let caller = std::panic::Location::caller();
        warn!("🚨 SEQUENTIAL DOWNLOAD: Package {}@{} not cached, downloading... (This should not happen in parallel mode!)", name, version);

        if crate::cli::verbosity::is_debug() {
            println!("🔍 SEQUENTIAL DOWNLOAD TRACE: {}@{}", name, version);
            println!("   └── Called from: {}:{}:{}", caller.file(), caller.line(), caller.column());
            println!("   └── Reason: Package not in cache, falling back to individual download");
            println!("   └── Context: This indicates the parallel download batch missed this package");
        }

        self.download_and_cache_with_checksum(registry, name, version, checksum).await
    }

    pub async fn get_package_parallel_with_checksum(
        &self,
        registry: &str,
        name: &str,
        version: &str,
        checksum: &str,
    ) -> Result<PathBuf> {
        if let Some(cached_path) = PackageStorage::get_package_path(registry, name, version)? {
            if crate::cli::verbosity::is_debug() {
                println!("✓ PARALLEL CACHE HIT: {}@{} (already cached)", name, version);
            } else {
                debug!("Using cached package: {}@{}", name, version);
            }
            return Ok(cached_path);
        }

        if crate::cli::verbosity::is_debug() {
            println!("📥 PARALLEL DOWNLOAD: {}@{} (downloading in parallel batch)", name, version);
        } else {
            debug!("Downloading {}@{} as part of parallel batch", name, version);
        }

        self.download_and_cache_with_checksum(registry, name, version, checksum).await
    }

    async fn download_and_cache_with_checksum(
        &self,
        registry: &str,
        name: &str,
        version: &str,
        checksum: &str,
    ) -> Result<PathBuf> {
        let download_start = std::time::Instant::now();

        if crate::cli::verbosity::is_debug() {
            println!("⬇️  DOWNLOADING: {}@{} from {}", name, version, registry);
        }

        let archive_path = self.downloader.download_with_checksum(registry, name, version, checksum).await?;
        let download_time = download_start.elapsed();

        if crate::cli::verbosity::is_debug() {
            println!("📦 EXTRACTING: {}@{} ({:.2}s download)", name, version, download_time.as_secs_f32());
        }

        // Move extraction to a blocking task pool to avoid blocking the async runtime
        let registry_str = registry.to_string();
        let name_str = name.to_string();
        let version_str = version.to_string();
        let archive_path_clone = archive_path.clone();
        let extract_start = std::time::Instant::now();
        // Stream B: synchronous cleanup (do not reorder with Stream A's debloat invocation)
        let store_result = tokio::task::spawn_blocking(move || {
            PackageStorage::store_package(&registry_str, &name_str, &version_str, &archive_path_clone)
        }).await?;

        let cached_path = match store_result {
            Ok(p) => p,
            Err(e) => {
                // Extraction failed – tear down any partial package dir AND
                // the tarball so we never leave half-written state behind.
                if let Ok(pkg_dir) = CachePaths::package_dir(registry, name, version) {
                    if pkg_dir.exists() {
                        if let Err(rm) = tokio::fs::remove_dir_all(&pkg_dir).await {
                            warn!("Failed to remove partial package dir {}: {}", pkg_dir.display(), rm);
                        }
                    }
                }
                if archive_path.exists() {
                    if let Err(rm) = tokio::fs::remove_file(&archive_path).await {
                        warn!("Failed to remove tarball {} after extraction error: {}", archive_path.display(), rm);
                    }
                }
                return Err(e);
            }
        };
        let extract_time = extract_start.elapsed();

        if crate::cli::verbosity::is_debug() {
            println!("✅ COMPLETED: {}@{} ({:.2}s extract, {:.2}s total)",
                name, version, extract_time.as_secs_f32(),
                (download_time + extract_time).as_secs_f32());
        }

        // Stream B: synchronous cleanup on success – block until the tarball
        // is gone (or we've logged the failure). We previously fire-and-forgot
        // this with `tokio::spawn`, which meant a short-lived CLI could exit
        // before the cleanup ran, leaving stale downloads on disk.
        if let Err(e) = tokio::fs::remove_file(&archive_path).await {
            warn!("Failed to clean up download file {}: {}", archive_path.display(), e);
        }

        Ok(cached_path)
    }

    /// Batch fetch packages. Checksums are required – callers that genuinely
    /// want to skip verification must pass [`ALLOW_UNCHECKSUMMED_SENTINEL`]
    /// as the checksum for that entry.
    pub async fn get_packages(
        &self,
        packages: Vec<(String, String, String, String)>, // (registry, name, version, checksum)
    ) -> Result<HashMap<String, PathBuf>> {
        let mut results = HashMap::new();

        for (registry, name, version, checksum) in packages {
            let key = format!("{}@{}", name, version);
            match self.get_package_with_checksum(&registry, &name, &version, &checksum).await {
                Ok(path) => {
                    results.insert(key, path);
                }
                Err(e) => {
                    warn!("Failed to get package {}@{}: {}", name, version, e);
                }
            }
        }

        Ok(results)
    }

    pub fn ensure_cache_dirs() -> Result<()> {
        CachePaths::ensure_directories()
    }

    pub fn generate_package_config(
        &self,
        packages: &HashMap<String, PathBuf>,
        project_name: &str,
    ) -> Result<()> {
        info!("Generating package_config.json");

        let dart_tool_dir = std::path::Path::new(".dart_tool");
        std::fs::create_dir_all(dart_tool_dir)?;

        let mut config = serde_json::json!({
            "configVersion": 2,
            "packages": [],
            "generated": chrono::Utc::now().to_rfc3339(),
            "generator": "hatch",
            "generatorVersion": "1.0.0"
        });

        let packages_array = config["packages"].as_array_mut()
            .ok_or_else(|| anyhow!("Failed to get packages array"))?;

        packages_array.push(serde_json::json!({
            "name": project_name,
            "rootUri": "..",
            "packageUri": "lib/",
            "languageVersion": "3.5"
        }));

        for (name_version, path) in packages {
            let parts: Vec<&str> = name_version.split('@').collect();
            if parts.len() != 2 {
                continue;
            }

            let name = parts[0];
            let lib_path = path.join("lib");

            let abs_path = path.canonicalize()
                .unwrap_or_else(|_| path.clone());

            let root_uri = Self::path_to_file_uri(&abs_path);

            packages_array.push(serde_json::json!({
                "name": name,
                "rootUri": root_uri,
                "packageUri": if lib_path.exists() { "lib/" } else { "" },
                "languageVersion": "3.5"
            }));
        }

        let config_path = dart_tool_dir.join("package_config.json");
        let json = serde_json::to_string_pretty(&config)?;
        std::fs::write(&config_path, json)?;

        info!("Generated package_config.json with {} packages", packages.len());
        Ok(())
    }

    pub fn generate_packages_file(
        &self,
        packages: &HashMap<String, PathBuf>,
        project_name: &str,
    ) -> Result<()> {
        info!("Generating .packages file");

        let mut lines = Vec::new();

        lines.push(format!("# Generated by hatch on {}", chrono::Utc::now()));
        lines.push("# This file is deprecated. Tools should instead use .dart_tool/package_config.json".to_string());

        lines.push(format!("{}:lib/", project_name));

        for (name_version, path) in packages {
            let parts: Vec<&str> = name_version.split('@').collect();
            if parts.len() != 2 {
                continue;
            }

            let name = parts[0];
            let lib_path = path.join("lib");

            if lib_path.exists() {
                let abs_path = lib_path.canonicalize()
                    .unwrap_or_else(|_| lib_path.clone());

                let base_uri = Self::path_to_file_uri(&abs_path);
                let uri = format!("{}/", base_uri);

                lines.push(format!("{}:{}", name, uri));
            }
        }

        let packages_content = lines.join("\n");
        std::fs::write(".packages", packages_content)?;

        info!("Generated .packages file with {} packages", packages.len());
        Ok(())
    }

    /// Convert a path to a file:// URI with minimal allocations
    fn path_to_file_uri(path: &std::path::Path) -> String {
        if cfg!(windows) {
            let path_str = path.display().to_string().replace('\\', "/");
            // Remove UNC prefix if present (\\?\  -> //?/ after replacement)
            if let Some(stripped) = path_str.strip_prefix("//?/") {
                format!("file:///{}", stripped)
            } else {
                format!("file:///{}", path_str)
            }
        } else {
            format!("file://{}", path.display())
        }
    }

    pub fn clear_cache() -> Result<()> {
        PackageStorage::clear_cache()
    }

    pub fn clear_package(registry: &str, name: &str, version: Option<&str>) -> Result<()> {
        PackageStorage::clear_package(registry, name, version)
    }

    pub fn get_cache_stats() -> Result<CacheStats> {
        let packages = PackageStorage::list_cached_packages()?;
        let total_size = PackageStorage::get_cache_size()?;

        Ok(CacheStats {
            package_count: packages.len(),
            total_size,
            packages,
        })
    }
}

impl Default for CacheManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct CacheStats {
    pub package_count: usize,
    pub total_size: u64,
    pub packages: Vec<super::storage::CachedPackageInfo>,
}

impl CacheStats {
    pub fn format_size(&self) -> String {
        let size = self.total_size as f64;
        let units = ["B", "KB", "MB", "GB"];
        let mut unit_idx = 0;
        let mut formatted = size;

        while formatted >= 1024.0 && unit_idx < units.len() - 1 {
            formatted /= 1024.0;
            unit_idx += 1;
        }

        format!("{:.2} {}", formatted, units[unit_idx])
    }
}