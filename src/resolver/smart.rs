use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use log::{debug, info, warn};

use crate::registry::traits::{PackageVersion, VersionConstraint};
use crate::registry::pub_dev::PubDevRegistry;
use crate::registry::traits::Registry;
use crate::manifest::schema::HatchManifest;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct VersionAlias {
    pub actual_version: String,
    pub pretend_version: String,
    pub is_path: bool,
}

impl VersionAlias {
    fn parse(alias_str: &str) -> Option<Self> {
        // Parse "1.8.3 as 1.9.9" or "dev-main as 2.0.0" or "path:../my-fork as 1.5.0"
        let parts: Vec<&str> = alias_str.split(" as ").collect();
        if parts.len() != 2 {
            return None;
        }

        let actual = parts[0].trim();
        let pretend = parts[1].trim();

        // Check if it's a path
        if actual.starts_with("path:") {
            Some(VersionAlias {
                actual_version: actual[5..].to_string(),
                pretend_version: pretend.to_string(),
                is_path: true,
            })
        } else {
            Some(VersionAlias {
                actual_version: actual.to_string(),
                pretend_version: pretend.to_string(),
                is_path: false,
            })
        }
    }
}

pub struct SmartResolver {
    registry: PubDevRegistry,
    metadata_cache: HashMap<String, Vec<PackageVersion>>,
    resolved: HashMap<String, String>,
    resolved_paths: HashMap<String, PathBuf>,
    queue: VecDeque<(String, String, String)>, // (package, constraint, requester)
    processed: HashSet<String>,
    overrides: HashMap<String, VersionAlias>,
    local_packages: HashMap<String, String>,
}

impl SmartResolver {
    pub fn new() -> Self {
        Self {
            registry: PubDevRegistry::new(),
            metadata_cache: HashMap::new(),
            resolved: HashMap::new(),
            resolved_paths: HashMap::new(),
            queue: VecDeque::new(),
            processed: HashSet::new(),
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

    pub async fn resolve(
        &mut self,
        root_deps: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>> {
        info!("Starting smart dependency resolution");
        for (name, constraint) in root_deps {
            if name == "flutter" {
                continue;
            }
            self.queue.push_back((name.clone(), constraint.clone(), "root".to_string()));
        }
        while let Some((pkg_name, constraint_str, requester)) = self.queue.pop_front() {
            // Check for local package first
            if let Some(local_path) = self.local_packages.get(&pkg_name) {
                info!("Using local package {} at {}", pkg_name, local_path);
                self.resolved.insert(pkg_name.clone(), "local".to_string());
                self.resolved_paths.insert(pkg_name.clone(), PathBuf::from(local_path));
                continue;
            }

            // Check for version override/alias
            if let Some(alias) = self.overrides.get(&pkg_name) {
                if alias.is_path {
                    info!("Using path override for {} at {} (as {})", pkg_name, alias.actual_version, alias.pretend_version);
                    self.resolved.insert(pkg_name.clone(), alias.pretend_version.clone());
                    self.resolved_paths.insert(pkg_name.clone(), PathBuf::from(&alias.actual_version));
                    continue;
                } else {
                    info!("Aliasing {} version {} as {}", pkg_name, alias.actual_version, alias.pretend_version);

                    // Fetch the actual version but report it as the pretend version
                    if !self.metadata_cache.contains_key(&pkg_name) {
                        if let Ok(metadata) = self.registry.get_package_metadata(&pkg_name).await {
                            self.metadata_cache.insert(pkg_name.clone(), metadata.versions);
                        }
                    }

                    if let Some(versions) = self.metadata_cache.get(&pkg_name) {
                        // Find the actual version to use
                        let version_to_use = if alias.actual_version == "dev-main" || alias.actual_version == "latest" {
                            versions.last() // Use latest
                        } else {
                            versions.iter().find(|v| v.version == alias.actual_version)
                        };

                        if let Some(version_info) = version_to_use {
                            // Store the actual version internally (for downloading)
                            // But pretend it's the alias version for constraint checking
                            self.resolved.insert(pkg_name.clone(), version_info.version.clone());

                            // Queue its dependencies
                            for (dep_name, dep_constraint) in &version_info.dependencies {
                                if dep_name != "flutter" && !self.resolved.contains_key(dep_name) {
                                    self.queue.push_back((
                                        dep_name.clone(),
                                        dep_constraint.clone(),
                                        pkg_name.clone(),
                                    ));
                                }
                            }
                        }
                    }
                    continue;
                }
            }
            if self.resolved.contains_key(&pkg_name) {
                let resolved_version = &self.resolved[&pkg_name];
                let constraint = VersionConstraint::parse(&constraint_str)?;

                if !constraint.satisfies(resolved_version) {
                    warn!("Conflict: {} requires {} {}, but {} is already resolved",
                        requester, pkg_name, constraint_str, resolved_version);

                    continue;
                }
                continue;
            }

            if self.processed.contains(&pkg_name) {
                continue;
            }
            self.processed.insert(pkg_name.clone());

            // Skip metadata fetching for local packages
            if self.local_packages.contains_key(&pkg_name) {
                continue;
            }

            if !self.metadata_cache.contains_key(&pkg_name) {
                debug!("Fetching metadata for {}", pkg_name);
                match self.registry.get_package_metadata(&pkg_name).await {
                    Ok(metadata) => {
                        self.metadata_cache.insert(pkg_name.clone(), metadata.versions);
                    }
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

            let versions = self.metadata_cache[&pkg_name].clone();
            let constraint = VersionConstraint::parse(&constraint_str)?;

            let matching_version = Self::find_best_version(&versions, &constraint, &pkg_name, &constraint_str);

            if let Some(idx) = matching_version {
                    let version = &versions[idx];
                    info!("Resolved {} {} (required by {})", pkg_name, version.version, requester);
                    let version_str = version.version.clone();
                    let deps = version.dependencies.clone();

                    self.resolved.insert(pkg_name.clone(), version_str);

                    for (dep_name, dep_constraint) in deps {
                        if dep_name == "flutter" || self.resolved.contains_key(&dep_name) {
                            continue;
                        }
                        self.queue.push_back((
                            dep_name,
                            dep_constraint,
                            pkg_name.clone(),
                        ));
                    }
            } else {
                    if requester == "root" {
                        return Err(anyhow!(
                            "No version of {} satisfies constraint {} (required by {})",
                            pkg_name, constraint_str, requester
                        ));
                    } else {
                        warn!("No version of {} satisfies {} (required by {}), using latest",
                            pkg_name, constraint_str, requester);

                        if let Some(latest) = versions.last() {
                            let version_str = latest.version.clone();
                            let deps = latest.dependencies.clone();

                            self.resolved.insert(pkg_name.clone(), version_str);

                            for (dep_name, dep_constraint) in deps {
                                if dep_name == "flutter" || self.resolved.contains_key(&dep_name) {
                                    continue;
                                }
                                self.queue.push_back((
                                    dep_name,
                                    dep_constraint,
                                    pkg_name.clone(),
                                ));
                            }
                        }
                    }
            }
        }

        Ok(self.resolved.clone())
    }

    fn find_best_version(
        versions: &[PackageVersion],
        constraint: &VersionConstraint,
        pkg_name: &str,
        constraint_str: &str,
    ) -> Option<usize> {
        let mut matching_indices: Vec<usize> = versions
            .iter()
            .enumerate()
            .filter(|(_, v)| constraint.satisfies(&v.version))
            .map(|(i, _)| i)
            .collect();

        if matching_indices.is_empty() {
            if constraint_str.contains("nullsafety") || constraint_str.contains("-nnbd") {
                warn!("Detected pre-null-safety constraint for {}: {}", pkg_name, constraint_str);
                return versions.iter()
                    .enumerate()
                    .rev()
                    .find(|(_, v)| !v.version.contains("-"))
                    .map(|(i, _)| i);
            }

            if let VersionConstraint::Range { min: Some(min), max: Some(max) } = constraint {
                if min > max || (min.contains("-") && !max.contains("-")) {
                    warn!("Impossible constraint for {}: {} - using latest stable", pkg_name, constraint_str);
                    return versions.iter()
                    .enumerate()
                    .rev()
                    .find(|(_, v)| !v.version.contains("-"))
                    .map(|(i, _)| i);
                }
            }

            return None;
        }

        matching_indices.sort_by_key(|&i| (!versions[i].version.contains("-"), &versions[i].version));

        matching_indices.last().copied()
    }

    pub fn get_metadata_cache(&self) -> &HashMap<String, Vec<PackageVersion>> {
        &self.metadata_cache
    }

    pub fn get_resolved_paths(&self) -> &HashMap<String, PathBuf> {
        &self.resolved_paths
    }

    pub fn get_display_versions(&self) -> HashMap<String, String> {
        let mut display = HashMap::new();
        for (pkg, version) in &self.resolved {
            // Check if this is an aliased package
            if let Some(alias) = self.overrides.get(pkg) {
                if !alias.is_path {
                    // Show the pretend version
                    display.insert(pkg.clone(), alias.pretend_version.clone());
                    continue;
                }
            }
            display.insert(pkg.clone(), version.clone());
        }
        display
    }

    pub fn get_actual_versions(&self) -> HashMap<String, String> {
        // We already store the real downloadable versions in resolved
        self.resolved.clone()
    }
}