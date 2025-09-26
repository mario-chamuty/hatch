use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use futures::future::join_all;
use tokio::sync::{RwLock, Semaphore};
use std::sync::Arc;
use log::{debug, info, warn};
use std::time::Instant;

use crate::manifest::schema::HatchManifest;
use crate::registry::pub_dev::PubDevRegistry;
use crate::registry::traits::{PackageVersion, VersionConstraint, Registry};
use super::smart::VersionAlias;
use crate::cache::metadata_cache::MetadataCache;
use crate::cli::verbosity;

pub struct UltraResolver {
    registry: PubDevRegistry,
    metadata_cache: Arc<MetadataCache>,
    resolved: HashMap<String, String>,
    resolved_paths: HashMap<String, std::path::PathBuf>,
    resolved_batch: HashMap<String, usize>, // Track which batch/level each package belongs to
    overrides: HashMap<String, VersionAlias>,
    local_packages: HashMap<String, String>,
}

impl UltraResolver {
    pub async fn new() -> Self {
        let cache = Arc::new(MetadataCache::new());
        // Load persistent metadata cache from disk
        let _ = cache.load_from_disk().await;

        Self {
            registry: PubDevRegistry::new(),
            metadata_cache: cache,
            resolved: HashMap::new(),
            resolved_paths: HashMap::new(),
            resolved_batch: HashMap::new(),
            overrides: HashMap::new(),
            local_packages: HashMap::new(),
        }
    }

    pub async fn with_manifest(manifest: &HatchManifest) -> Self {
        let mut resolver = Self::new().await;

        // Parse override aliases
        if let Some(overrides) = &manifest.overrides {
            for (pkg_name, alias_str) in overrides {
                if let Some(alias) = VersionAlias::parse(alias_str) {
                    resolver.overrides.insert(pkg_name.clone(), alias);
                }
            }
        }

        if let Some(local_packages) = &manifest.local_packages {
            resolver.local_packages = local_packages.clone();
        }

        resolver
    }

    pub async fn resolve(&mut self, manifest: &HatchManifest) -> Result<HashMap<String, String>> {
        let total_start = Instant::now();
        info!("Starting ultra-fast dependency resolution");

        // Collect all root dependencies
        let mut all_deps = HashMap::new();

        if let Some(deps) = &manifest.require {
            all_deps.extend(deps.clone());
        }

        if let Some(dev_deps) = &manifest.require_dev {
            all_deps.extend(dev_deps.clone());
        }

        println!("🔍 Resolving {} direct dependencies...", all_deps.len());

        // Aggressively pre-fetch ALL possible metadata
        let fetch_start = Instant::now();
        self.prefetch_all_metadata(&all_deps).await?;

        if verbosity::should_show_timings() {
            println!("   ⏱️  Metadata fetch: {:.2}s", fetch_start.elapsed().as_secs_f32());
        }

        // Now resolve with all metadata already cached
        let resolve_start = Instant::now();

        // Batch 0: Direct dependencies
        for (name, constraint) in &all_deps {
            if Self::is_flutter_sdk_package(name) || self.local_packages.contains_key(name) {
                continue;
            }
            self.resolve_package(name, constraint, "root", 0).await?;
        }

        // Process ALL transitive dependencies - keep going until NO new packages are found
        let mut dependency_waves = 0;
        let mut failed_packages = HashSet::new(); // Track packages that failed to resolve

        loop {
            dependency_waves += 1;
            let mut new_packages_found = false;
            let mut queue = VecDeque::new();

            // Add all transitive deps from currently resolved packages to queue
            for (pkg_name, version) in &self.resolved.clone() {
                if let Some(metadata) = self.metadata_cache.get(pkg_name).await {
                    if let Some(version_info) = metadata.iter().find(|v| v.version == *version) {
                        for (dep_name, dep_constraint) in &version_info.dependencies {
                            // Skip Flutter SDK packages and already processed packages
                            if Self::is_flutter_sdk_package(dep_name) ||
                               self.resolved.contains_key(dep_name) ||
                               failed_packages.contains(dep_name) {
                                continue;
                            }
                            queue.push_back((dep_name.clone(), dep_constraint.clone(), pkg_name.clone()));
                        }
                    }
                }
            }

            // If no packages to process, we're done
            if queue.is_empty() {
                debug!("Transitive dependency resolution complete after {} waves", dependency_waves);
                break;
            }

            // Process this wave of dependencies
            while let Some((pkg_name, constraint_str, requester)) = queue.pop_front() {
                if self.resolved.contains_key(&pkg_name) || failed_packages.contains(&pkg_name) {
                    continue;
                }

                // Fetch metadata if we don't have it yet
                if !self.metadata_cache.contains(&pkg_name).await {
                    debug!("Fetching missing metadata for {}", pkg_name);
                    match self.registry.get_package_metadata(&pkg_name).await {
                        Ok(metadata) => {
                            self.metadata_cache.insert(pkg_name.clone(), metadata.versions).await;
                        }
                        Err(_) => {
                            // Package doesn't exist - mark as failed to prevent retries
                            failed_packages.insert(pkg_name.clone());
                            debug!("Package {} not found, marking as failed", pkg_name);
                            continue;
                        }
                    }
                }

                match self.resolve_package(&pkg_name, &constraint_str, &requester, dependency_waves).await {
                    Ok(_) => {
                        new_packages_found = true;
                    }
                    Err(_) => {
                        // Failed to resolve - mark as failed to prevent retries
                        failed_packages.insert(pkg_name.clone());
                        debug!("Failed to resolve {}, marking as failed", pkg_name);
                    }
                }
            }

            // Safety check to prevent infinite loops
            if dependency_waves > 20 {
                warn!("Stopping transitive resolution at wave {} to prevent infinite loops", dependency_waves);
                break;
            }
        }

        if verbosity::should_show_timings() {
            println!("   ⏱️  Resolution: {:.2}s", resolve_start.elapsed().as_secs_f32());
            println!("   ⏱️  Total time: {:.2}s", total_start.elapsed().as_secs_f32());
        }

        Ok(self.resolved.clone())
    }

    async fn resolve_package(&mut self, name: &str, constraint_str: &str, requester: &str, batch: usize) -> Result<()> {
        // Check for local package
        if let Some(local_path) = self.local_packages.get(name) {
            debug!("Using local package {} at {}", name, local_path);
            self.resolved.insert(name.to_string(), "local".to_string());
            self.resolved_paths.insert(name.to_string(), local_path.into());
            return Ok(());
        }

        // Check for version override
        if let Some(alias) = self.overrides.get(name) {
            self.resolved.insert(name.to_string(), alias.actual_version.clone());
            return Ok(());
        }

        // Get cached metadata
        let versions = match self.metadata_cache.get(name).await {
            Some(v) => v,
            None => {
                if requester == "root" {
                    return Err(anyhow!("Required package {} not found", name));
                } else {
                    warn!("Optional dependency {} not found", name);
                    return Ok(());
                }
            }
        };

        // Find best version
        let constraint = VersionConstraint::parse(constraint_str)?;

        // Debug: Show available versions for problematic packages
        if ["web", "mime", "file", "dio_web_adapter", "logging", "bloc", "js", "rxdart", "xdg_directories"].contains(&name) {
            println!("🐛 Debug {} constraint='{}' has {} versions: {:?}",
                name, constraint_str, versions.len(),
                versions.iter().map(|v| &v.version).collect::<Vec<_>>());

            // Also show which versions satisfy the constraint
            let satisfying: Vec<_> = versions.iter().enumerate()
                .filter(|(_, v)| constraint.satisfies(&v.version))
                .map(|(idx, v)| format!("{}({})", v.version, idx))
                .collect();
            println!("🐛 Satisfying versions: {:?}", satisfying);
        }

        let matching_version = Self::find_best_version(&versions, &constraint);

        if let Some(idx) = matching_version {
            let selected = &versions[idx];
            self.resolved.insert(name.to_string(), selected.version.clone());
            self.resolved_batch.insert(name.to_string(), batch);
            if crate::cli::verbosity::is_debug() {
                println!("📦 Resolved {} @ {} (Batch {})", name, selected.version, batch);
            }
        } else {
            if requester == "root" {
                return Err(anyhow!("No version of {} satisfies constraint {}", name, constraint_str));
            } else {
                warn!("No version of {} satisfies {} (required by {})", name, constraint_str, requester);
                // Use latest version as fallback for transitive deps
                if let Some(latest) = versions.last() {
                    self.resolved.insert(name.to_string(), latest.version.clone());
                    self.resolved_batch.insert(name.to_string(), batch);
                }
            }
        }

        Ok(())
    }

    async fn prefetch_all_metadata(&self, root_deps: &HashMap<String, String>) -> Result<()> {
        let semaphore = Arc::new(Semaphore::new(100)); // Very aggressive parallelism
        let mut all_packages = HashSet::new();
        let mut failed_packages = HashSet::new(); // Track packages that failed to fetch

        // Add root packages
        for (name, _) in root_deps {
            if !Self::is_flutter_sdk_package(name) && !self.local_packages.contains_key(name) {
                all_packages.insert(name.clone());
            }
        }

        // Fetch in expanding waves
        let mut wave = 0;
        loop {
            wave += 1;
            // Check which packages need fetching (excluding already failed ones)
            let mut to_fetch = Vec::new();
            for package in &all_packages {
                if !self.metadata_cache.contains(package).await && !failed_packages.contains(package) {
                    to_fetch.push(package.clone());
                }
            }

            if to_fetch.is_empty() {
                break;
            }

            debug!("Wave {}: fetching {} packages", wave, to_fetch.len());

            // Fetch this wave in parallel
            let mut tasks = Vec::new();
            for package in to_fetch {
                let registry = self.registry.clone();
                let cache = self.metadata_cache.clone();
                let sem = semaphore.clone();

                tasks.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();

                    match registry.get_package_metadata(&package).await {
                        Ok(metadata) => {
                            // Extract dependencies to fetch next
                            let mut deps = HashSet::new();
                            for version in &metadata.versions {
                                for (dep_name, _) in &version.dependencies {
                                    if !Self::is_flutter_sdk_package(dep_name) {
                                        deps.insert(dep_name.clone());
                                    }
                                }
                            }

                            cache.insert(package.clone(), metadata.versions).await;
                            Ok((package, deps))
                        }
                        Err(e) => {
                            debug!("Failed to fetch {}: {}", package, e);
                            Err((package, e))
                        }
                    }
                }));
            }

            // Collect new dependencies from this wave
            let results = join_all(tasks).await;
            for result in results {
                match result {
                    Ok(Ok((_, deps))) => {
                        for dep in deps {
                            all_packages.insert(dep);
                        }
                    }
                    Ok(Err((package, e))) => {
                        // Mark package as failed to avoid re-fetching
                        failed_packages.insert(package.clone());

                        // Special handling for specific known issues
                        if package == "sqflite" {
                            warn!("Package 'sqflite' has malformed metadata, skipping");
                        } else if e.to_string().contains("not found") {
                            debug!("Package '{}' not found on registry, marking as unavailable", package);
                        }
                    }
                    Err(e) => {
                        warn!("Task error during fetch: {}", e);
                    }
                }
            }

            // Only stop if we've gone too deep (preventing infinite loops)
            if wave > 20 {
                warn!("Stopping metadata fetch at wave {} with {} packages to prevent infinite loops", wave, all_packages.len());
                break;
            }
        }

        if !failed_packages.is_empty() {
            debug!("Failed to fetch {} packages: {:?}", failed_packages.len(), failed_packages);
        }

        info!("Pre-fetched metadata for {} packages in {} waves", all_packages.len() - failed_packages.len(), wave);
        Ok(())
    }

    fn find_best_version(
        versions: &[PackageVersion],
        constraint: &VersionConstraint,
    ) -> Option<usize> {
        // Find the latest version that satisfies the constraint
        let mut best_idx = None;
        for (idx, pkg_version) in versions.iter().enumerate() {
            if constraint.satisfies(&pkg_version.version) {
                best_idx = Some(idx); // Keep updating to find the latest compatible version
            }
        }
        best_idx
    }

    pub fn get_resolved(&self) -> &HashMap<String, String> {
        &self.resolved
    }

    pub fn get_resolved_paths(&self) -> &HashMap<String, std::path::PathBuf> {
        &self.resolved_paths
    }

    pub fn get_resolved_batches(&self) -> &HashMap<String, usize> {
        &self.resolved_batch
    }

    /// Check if a package is part of the Flutter SDK (not available on pub.dev)
    fn is_flutter_sdk_package(name: &str) -> bool {
        matches!(name,
            "flutter" |
            "flutter_test" |
            "flutter_web_plugins" |
            "flutter_driver" |
            "integration_test" |
            "_macros"        // Dart SDK package
        )
    }
}