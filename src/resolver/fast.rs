use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use futures::future::join_all;
use tokio::sync::RwLock;
use std::sync::Arc;
use log::{debug, info, warn};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Instant;

use crate::manifest::schema::HatchManifest;
use crate::registry::pub_dev::PubDevRegistry;
use crate::registry::traits::{PackageVersion, VersionConstraint, Registry};
use super::smart::VersionAlias;
use crate::cli::verbosity;

pub struct FastResolver {
    registry: PubDevRegistry,
    metadata_cache: Arc<RwLock<HashMap<String, Vec<PackageVersion>>>>,
    resolved: HashMap<String, String>,
    resolved_paths: HashMap<String, std::path::PathBuf>,
    overrides: HashMap<String, VersionAlias>,
    local_packages: HashMap<String, String>,
}

impl FastResolver {
    pub fn new() -> Self {
        Self {
            registry: PubDevRegistry::new(),
            metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            resolved: HashMap::new(),
            resolved_paths: HashMap::new(),
            overrides: HashMap::new(),
            local_packages: HashMap::new(),
        }
    }

    pub fn with_manifest(manifest: &HatchManifest) -> Self {
        let mut resolver = Self::new();

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
        info!("Starting fast dependency resolution");

        // Collect all root dependencies
        let mut all_deps = HashMap::new();

        if let Some(deps) = &manifest.require {
            all_deps.extend(deps.clone());
        }

        if let Some(dev_deps) = &manifest.require_dev {
            all_deps.extend(dev_deps.clone());
        }

        // Pre-fetch metadata for all root dependencies in parallel
        println!("📊 Fetching package metadata...");
        let fetch_start = Instant::now();
        self.prefetch_metadata(&all_deps).await?;
        println!("   ⏱️  Metadata fetch: {:.2}s", fetch_start.elapsed().as_secs_f32());

        // Now resolve with cached metadata
        let resolve_start = Instant::now();

        let progress = if verbosity::should_show_progress_bars() && !all_deps.is_empty() {
            let pb = ProgressBar::new(all_deps.len() as u64);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("   {bar:40.cyan/blue} {pos}/{len} packages")
                    .unwrap()
                    .progress_chars("=>-"),
            );
            Some(pb)
        } else {
            None
        };

        let mut queue = VecDeque::new();
        for (name, constraint) in &all_deps {
            if name == "flutter" {
                continue;
            }
            queue.push_back((name.clone(), constraint.clone(), "root".to_string()));
        }

        let mut processed = HashSet::new();

        while let Some((pkg_name, constraint_str, requester)) = queue.pop_front() {
            // Check for local package first
            if let Some(local_path) = self.local_packages.get(&pkg_name) {
                debug!("Using local package {} at {}", pkg_name, local_path);
                self.resolved.insert(pkg_name.clone(), "local".to_string());
                self.resolved_paths.insert(pkg_name.clone(), local_path.into());
                if let Some(ref pb) = progress {
                    pb.inc(1);
                }
                continue;
            }

            // Check if already resolved
            if self.resolved.contains_key(&pkg_name) {
                if let Some(ref pb) = progress {
                    pb.inc(1);
                }
                continue;
            }

            if processed.contains(&pkg_name) {
                continue;
            }
            processed.insert(pkg_name.clone());

            // Get cached metadata
            let cache = self.metadata_cache.read().await;
            let versions = match cache.get(&pkg_name) {
                Some(v) => v.clone(),
                None => {
                    drop(cache);
                    // Fetch on-demand if not in cache
                    match self.fetch_single_metadata(&pkg_name).await {
                        Ok(versions) => versions,
                        Err(e) => {
                            if requester == "root" {
                                return Err(anyhow!("Required package {} not found: {}", pkg_name, e));
                            } else {
                                warn!("Optional dependency {} not found: {}", pkg_name, e);
                                continue;
                            }
                        }
                    }
                }
            };

            // Find best version
            let matching_version = Self::find_best_version(&versions, &constraint_str, &pkg_name);

            if let Some(idx) = matching_version {
                let selected = &versions[idx];
                let selected_version = if let Some(alias) = self.overrides.get(&pkg_name) {
                    alias.actual_version.clone()
                } else {
                    selected.version.clone()
                };

                self.resolved.insert(pkg_name.clone(), selected_version.clone());

                // Queue transitive dependencies (but don't fetch metadata yet)
                for (dep_name, dep_constraint) in &selected.dependencies {
                    if !self.resolved.contains_key(dep_name) && !processed.contains(dep_name) {
                        queue.push_back((dep_name.clone(), dep_constraint.clone(), pkg_name.clone()));
                    }
                }
            } else {
                if requester == "root" {
                    return Err(anyhow!("No version of {} satisfies constraint {}", pkg_name, constraint_str));
                } else {
                    warn!("No version of {} satisfies constraint {} (required by {})",
                        pkg_name, constraint_str, requester);
                }
            }

            if let Some(ref pb) = progress {
                pb.inc(1);
            }
        }

        if let Some(pb) = progress {
            pb.finish_and_clear();
        }

        if verbosity::should_show_timings() {
            println!("   ⏱️  Resolution: {:.2}s", resolve_start.elapsed().as_secs_f32());
            println!("   ⏱️  Total resolve time: {:.2}s", total_start.elapsed().as_secs_f32());
        }

        Ok(self.resolved.clone())
    }

    async fn prefetch_metadata(&self, packages: &HashMap<String, String>) -> Result<()> {
        let start = Instant::now();
        let mut fetch_tasks = Vec::new();
        let cache = self.metadata_cache.clone();

        for (name, _) in packages {
            if name == "flutter" || self.local_packages.contains_key(name) {
                continue;
            }

            let pkg_name = name.clone();
            let registry = self.registry.clone();
            let cache_clone = cache.clone();

            fetch_tasks.push(tokio::spawn(async move {
                match registry.get_package_metadata(&pkg_name).await {
                    Ok(metadata) => {
                        let mut cache_write = cache_clone.write().await;
                        cache_write.insert(pkg_name.clone(), metadata.versions);
                        Ok(())
                    }
                    Err(e) => {
                        warn!("Failed to fetch metadata for {}: {}", pkg_name, e);
                        Err(e)
                    }
                }
            }));
        }

        // Wait for all fetches to complete
        let results = join_all(fetch_tasks).await;

        let successful = results.iter().filter(|r| matches!(r, Ok(Ok(_)))).count();
        let failed = results.len() - successful;

        debug!("Fetched {} packages metadata in {:.2}s ({} succeeded, {} failed)",
            packages.len(), start.elapsed().as_secs_f32(), successful, failed);

        // Check for critical failures
        for result in results {
            if let Ok(Err(e)) = result {
                debug!("Metadata fetch error: {}", e);
            }
        }

        Ok(())
    }

    async fn fetch_single_metadata(&self, package: &str) -> Result<Vec<PackageVersion>> {
        let metadata = self.registry.get_package_metadata(package).await?;
        let mut cache = self.metadata_cache.write().await;
        cache.insert(package.to_string(), metadata.versions.clone());
        Ok(metadata.versions)
    }

    fn find_best_version(
        versions: &[PackageVersion],
        constraint_str: &str,
        pkg_name: &str,
    ) -> Option<usize> {
        let constraint = VersionConstraint::parse(constraint_str).ok()?;

        // First try exact match
        for (idx, pkg_version) in versions.iter().enumerate() {
            if constraint.satisfies(&pkg_version.version) {
                return Some(idx);
            }
        }

        // If no exact match, try relaxing for non-root dependencies
        if constraint_str.contains("nullsafety") || constraint_str.contains("-nnbd") {
            warn!("Detected pre-null-safety constraint for {}: {}", pkg_name, constraint_str);
            return versions.iter()
                .enumerate()
                .rev()
                .find(|(_, v)| !v.version.contains("-"))
                .map(|(i, _)| i);
        }

        // For impossible ranges, use latest stable
        if let VersionConstraint::Range { min: Some(min), max: Some(max) } = &constraint {
            if min > max || (min.contains("-") && !max.contains("-")) {
                warn!("Impossible constraint for {}: {} - using latest stable", pkg_name, constraint_str);
                return versions.iter()
                    .enumerate()
                    .rev()
                    .find(|(_, v)| !v.version.contains("-"))
                    .map(|(i, _)| i);
            }
        }

        None
    }

    pub fn get_metadata_cache(&self) -> Arc<RwLock<HashMap<String, Vec<PackageVersion>>>> {
        self.metadata_cache.clone()
    }

    pub fn get_resolved(&self) -> &HashMap<String, String> {
        &self.resolved
    }

    pub fn get_resolved_paths(&self) -> &HashMap<String, std::path::PathBuf> {
        &self.resolved_paths
    }
}