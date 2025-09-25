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

        for (name, constraint) in &all_deps {
            if name == "flutter" || self.local_packages.contains_key(name) {
                continue;
            }
            self.resolve_package(name, constraint, "root").await?;
        }

        // Process transitive dependencies
        let mut processed = HashSet::new();
        let mut queue = VecDeque::new();

        // Add initial transitive deps to queue
        for (pkg_name, version) in &self.resolved.clone() {
            if let Some(metadata) = self.metadata_cache.get(pkg_name).await {
                if let Some(version_info) = metadata.iter().find(|v| v.version == *version) {
                    for (dep_name, dep_constraint) in &version_info.dependencies {
                        if !self.resolved.contains_key(dep_name) && !processed.contains(dep_name) {
                            queue.push_back((dep_name.clone(), dep_constraint.clone(), pkg_name.clone()));
                        }
                    }
                }
            }
        }

        while let Some((pkg_name, constraint_str, requester)) = queue.pop_front() {
            if self.resolved.contains_key(&pkg_name) || processed.contains(&pkg_name) {
                continue;
            }
            processed.insert(pkg_name.clone());

            if let Ok(_) = self.resolve_package(&pkg_name, &constraint_str, &requester).await {
                // Add new transitive deps
                if let Some(metadata) = self.metadata_cache.get(&pkg_name).await {
                    if let Some(resolved_version) = self.resolved.get(&pkg_name) {
                        if let Some(version_info) = metadata.iter().find(|v| v.version == *resolved_version) {
                            for (dep_name, dep_constraint) in &version_info.dependencies {
                                if !self.resolved.contains_key(dep_name) && !processed.contains(dep_name) {
                                    queue.push_back((dep_name.clone(), dep_constraint.clone(), pkg_name.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }

        if verbosity::should_show_timings() {
            println!("   ⏱️  Resolution: {:.2}s", resolve_start.elapsed().as_secs_f32());
            println!("   ⏱️  Total time: {:.2}s", total_start.elapsed().as_secs_f32());
        }

        Ok(self.resolved.clone())
    }

    async fn resolve_package(&mut self, name: &str, constraint_str: &str, requester: &str) -> Result<()> {
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
        let matching_version = Self::find_best_version(&versions, &constraint);

        if let Some(idx) = matching_version {
            let selected = &versions[idx];
            self.resolved.insert(name.to_string(), selected.version.clone());
        } else {
            if requester == "root" {
                return Err(anyhow!("No version of {} satisfies constraint {}", name, constraint_str));
            } else {
                warn!("No version of {} satisfies {} (required by {})", name, constraint_str, requester);
                // Use latest version as fallback for transitive deps
                if let Some(latest) = versions.last() {
                    self.resolved.insert(name.to_string(), latest.version.clone());
                }
            }
        }

        Ok(())
    }

    async fn prefetch_all_metadata(&self, root_deps: &HashMap<String, String>) -> Result<()> {
        let semaphore = Arc::new(Semaphore::new(100)); // Very aggressive parallelism
        let mut all_packages = HashSet::new();

        // Add root packages
        for (name, _) in root_deps {
            if name != "flutter" && !self.local_packages.contains_key(name) {
                all_packages.insert(name.clone());
            }
        }

        // Fetch in expanding waves
        let mut wave = 0;
        loop {
            wave += 1;
            // Check which packages need fetching
            let mut to_fetch = Vec::new();
            for package in &all_packages {
                if !self.metadata_cache.contains(package).await {
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
                                    if dep_name != "flutter" {
                                        deps.insert(dep_name.clone());
                                    }
                                }
                            }

                            cache.insert(package.clone(), metadata.versions).await;
                            Ok(deps)
                        }
                        Err(e) => {
                            debug!("Failed to fetch {}: {}", package, e);
                            Err(e)
                        }
                    }
                }));
            }

            // Collect new dependencies from this wave
            let results = join_all(tasks).await;
            for result in results {
                if let Ok(Ok(deps)) = result {
                    for dep in deps {
                        all_packages.insert(dep);
                    }
                }
            }

            // Limit waves to prevent infinite loops
            if wave > 5 {
                debug!("Stopping at wave {} with {} packages", wave, all_packages.len());
                break;
            }
        }

        info!("Pre-fetched metadata for {} packages in {} waves", all_packages.len(), wave);
        Ok(())
    }

    fn find_best_version(
        versions: &[PackageVersion],
        constraint: &VersionConstraint,
    ) -> Option<usize> {
        // First try exact match
        for (idx, pkg_version) in versions.iter().enumerate().rev() {
            if constraint.satisfies(&pkg_version.version) {
                return Some(idx);
            }
        }

        // No match found
        None
    }

    pub fn get_resolved(&self) -> &HashMap<String, String> {
        &self.resolved
    }

    pub fn get_resolved_paths(&self) -> &HashMap<String, std::path::PathBuf> {
        &self.resolved_paths
    }
}