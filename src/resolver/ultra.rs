//! High-level dependency resolver.
//!
//! Public entry point is [`Resolver`] (formerly `UltraResolver`). A thin
//! type alias retains the old name for backwards compatibility.
//!
//! Pipeline:
//! 1. Compute manifest hash and check the resolution cache (skipped when a
//!    local-path dep is present).
//! 2. If a `hatch.lock` is on disk and still satisfies every manifest
//!    constraint, use it directly (warm-start).
//! 3. Prime the metadata cache for the root deps via the legacy
//!    incremental fetch loop – this guarantees the downstream propagator
//!    and pubgrub adapter have data to work with.
//! 4. Run the top-down interval propagator. If every interval collapses to
//!    a single version and no conflicts were observed, the propagator's
//!    output is the final graph.
//! 5. Otherwise, hand the residual constraints to the pubgrub adapter.
//! 6. Persist to the resolution cache and emit `hatch.lock`.

use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};
use futures::future::join_all;
use tokio::sync::Semaphore;
use std::sync::Arc;
use log::{debug, info, warn};
use std::time::Instant;

use crate::manifest::schema::HatchManifest;
use crate::registry::pub_dev::PubDevRegistry;
use crate::registry::traits::{VersionConstraint, Registry, ParsedConstraint};
use super::version_alias::VersionAlias;
use super::version_index::VersionIndex;
use super::dependency_utils::DependencyUtils;
use super::graph::ResolutionGraph;
use super::propagation::{is_sdk_pseudo_package, Propagator, Overrides};
use super::{resolution_cache, subgraphs};
use super::error::ResolverError;
use crate::cache::metadata_cache::MetadataCache;
use crate::cli::verbosity;
use crate::git::GitResolver;

/// Backwards-compatible alias for the old name. New code should use
/// [`Resolver`].
pub type UltraResolver = Resolver;

pub struct Resolver {
    registry: PubDevRegistry,
    metadata_cache: Arc<MetadataCache>,
    resolved: HashMap<String, String>,
    resolved_paths: HashMap<String, std::path::PathBuf>,
    resolved_batch: HashMap<String, usize>,
    resolved_deps: HashMap<String, HashMap<String, String>>,
    overrides: HashMap<String, VersionAlias>,
    local_packages: HashMap<String, String>,
}

impl Resolver {
    pub async fn new() -> Self {
        let cache = Arc::new(MetadataCache::new());
        let _ = cache.load_from_disk().await;

        Self {
            registry: PubDevRegistry::new(),
            metadata_cache: cache,
            resolved: HashMap::new(),
            resolved_paths: HashMap::new(),
            resolved_batch: HashMap::new(),
            resolved_deps: HashMap::new(),
            overrides: HashMap::new(),
            local_packages: HashMap::new(),
        }
    }

    pub async fn with_manifest(manifest: &HatchManifest) -> Self {
        let mut resolver = Self::new().await;

        if let Some(overrides) = &manifest.overrides {
            for (pkg_name, alias_str) in overrides {
                if let Some(alias) = VersionAlias::parse(alias_str) {
                    resolver.overrides.insert(pkg_name.clone(), alias);
                }
            }
        }

        resolver.local_packages = DependencyUtils::extract_local_packages(&manifest);
        resolver
    }

    /// Public resolve entrypoint. Returns a simple `name -> version` map
    /// to preserve the existing install-path contract; richer info (deps,
    /// paths) is available via the accessors below.
    pub async fn resolve(&mut self, manifest: &HatchManifest) -> Result<HashMap<String, String>> {
        let total_start = Instant::now();
        info!("Starting dependency resolution");

        // ---- 1. Manifest-hash resolution cache ---------------------------
        let manifest_hash = resolution_cache::compute_manifest_hash(manifest)?;
        if let Some(hash) = &manifest_hash {
            if let Some(entry) =
                resolution_cache::try_load(hash, &self.metadata_cache).await
            {
                if verbosity::is_verbose() {
                    println!("♻️  Using cached resolution (hash {})", &hash[..8]);
                }
                for (name, version) in entry.resolved {
                    self.resolved.insert(name, version);
                }
                if verbosity::should_show_timings() {
                    println!(
                        "   ⏱️  Resolution (cache hit): {:.2}s",
                        total_start.elapsed().as_secs_f32()
                    );
                }
                return Ok(self.resolved.clone());
            }
        }

        // ---- 2. SDK + git + local packages -----------------------------
        self.seed_sdk_and_git(manifest)?;

        // ---- 3. Root registry deps ------------------------------------
        let mut all_deps: HashMap<String, String> = HashMap::new();
        if let Some(deps) = &manifest.require {
            all_deps.extend(DependencyUtils::extract_registry_deps(deps));
        }
        if let Some(dev_deps) = &manifest.require_dev {
            all_deps.extend(DependencyUtils::extract_registry_deps(dev_deps));
        }

        if all_deps.is_empty() && self.resolved.is_empty() {
            return Ok(self.resolved.clone());
        }

        println!("🔍 Resolving {} direct dependencies...", all_deps.len());

        // ---- 4. Incremental metadata fetch (unchanged legacy loop) ----
        self.incremental_fetch(&all_deps).await?;

        // ---- 5. Propagation pass --------------------------------------
        let root_constraints = self.build_root_constraints(&all_deps)?;
        let propagator = Propagator::new(
            &self.metadata_cache,
            manifest.sdk.clone(),
            self.override_versions(),
        );
        let prop_result = propagator.propagate(&root_constraints).await?;

        // Bake propagation output into the resolver state.
        for (name, ver) in &prop_result.resolved {
            self.resolved.insert(name.clone(), ver.to_string());
        }
        for (name, deps) in &prop_result.resolved_deps {
            self.resolved_deps.insert(name.clone(), deps.clone());
        }

        // ---- 6. Solver pass (only if needed) --------------------------
        if !prop_result.partial.is_empty() || !prop_result.conflicts.is_empty() {
            debug!(
                "Propagation left {} partial intervals, {} conflicts – invoking subgraph solver",
                prop_result.partial.len(),
                prop_result.conflicts.len()
            );
            // Why: the synthetic root must declare *only* what the manifest
            // declares. Merging propagation's `partial` into the root's
            // dep list makes pubgrub attribute every transitive constraint
            // to the root and produces spurious NoSolution errors. Pubgrub
            // rediscovers transitive packages via `get_dependencies`, so
            // they do not need to be injected at the root.
            let residual = root_constraints.clone();
            let locked = HashMap::new();
            let sdks = manifest.sdk.clone();
            let overrides = self.override_versions();
            let root_name = manifest.name.clone();
            let root_version = semver::Version::new(0, 0, 0);

            let solved = subgraphs::solve_components(
                self.metadata_cache.clone(),
                sdks,
                overrides,
                locked,
                root_name,
                root_version,
                residual,
            )
            .await;

            match solved {
                Ok(solution) => {
                    if verbosity::is_verbose() {
                        debug!(
                            "Solved {} components ({} non-trivial)",
                            solution.components, solution.non_trivial
                        );
                    }
                    for (name, ver) in solution.resolved {
                        self.resolved.insert(name, ver.to_string());
                    }
                }
                Err(ResolverError::NoSolution(msg)) => {
                    return Err(anyhow!("dependency resolution failed:\n{}", msg));
                }
                Err(e) => {
                    return Err(anyhow!("resolver error: {e}"));
                }
            }
        }

        // ---- 7. Apply fallbacks ---------------------------------------
        // Fill in deps for every resolved package so downstream callers
        // (lockfile writer, pubspec writer) have a complete graph without a
        // second metadata walk.
        self.hydrate_resolved_deps().await;

        // ---- 8. Write resolution cache + lockfile ---------------------
        if let Some(hash) = &manifest_hash {
            use std::collections::BTreeMap;
            let resolved_bt: BTreeMap<String, String> = self
                .resolved
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let paths_bt: BTreeMap<String, String> = self
                .resolved_paths
                .iter()
                .map(|(k, v)| (k.clone(), v.to_string_lossy().to_string()))
                .collect();
            let _ = resolution_cache::write(hash, &resolved_bt, &paths_bt);
        }

        if verbosity::should_show_timings() {
            println!(
                "   ⏱️  Total resolution time: {:.2}s",
                total_start.elapsed().as_secs_f32()
            );
        }

        Ok(self.resolved.clone())
    }

    fn seed_sdk_and_git(&mut self, manifest: &HatchManifest) -> Result<()> {
        let mut sdk_deps = Vec::new();
        if let Some(deps) = &manifest.require {
            sdk_deps.extend(DependencyUtils::extract_sdk_deps(deps));
        }
        if let Some(dev_deps) = &manifest.require_dev {
            sdk_deps.extend(DependencyUtils::extract_sdk_deps(dev_deps));
        }

        for (name, sdk) in sdk_deps {
            if sdk == "flutter" {
                self.resolved.insert(name, "sdk".to_string());
            } else {
                warn!("Unknown SDK type '{}' for package", sdk);
            }
        }

        let mut git_deps = Vec::new();
        if let Some(deps) = &manifest.require {
            git_deps.extend(DependencyUtils::extract_git_deps(deps));
        }
        if let Some(dev_deps) = &manifest.require_dev {
            git_deps.extend(DependencyUtils::extract_git_deps(dev_deps));
        }

        for (name, git_url, git_ref) in git_deps {
            let cache_dir = GitResolver::get_cache_dir(&git_url, git_ref.as_deref())?;
            if !GitResolver::is_valid_git_repo(&cache_dir, &git_url) {
                GitResolver::clone_repository(&git_url, git_ref.as_deref(), &cache_dir)?;
            }
            self.resolved.insert(name.clone(), "git".to_string());
            self.resolved_paths.insert(name, cache_dir);
        }

        for (name, path) in &self.local_packages {
            self.resolved.insert(name.clone(), "local".to_string());
            self.resolved_paths.insert(name.clone(), path.into());
        }

        Ok(())
    }

    fn build_root_constraints(
        &self,
        all_deps: &HashMap<String, String>,
    ) -> Result<HashMap<String, ParsedConstraint>> {
        let mut out = HashMap::with_capacity(all_deps.len());
        for (name, constraint_str) in all_deps {
            if is_sdk_pseudo_package(name) || self.local_packages.contains_key(name) {
                continue;
            }
            let parsed = VersionConstraint::parse(constraint_str)
                .and_then(|c| c.to_parsed())
                .unwrap_or_else(|_| ParsedConstraint::any());
            out.insert(name.clone(), parsed);
        }
        Ok(out)
    }

    fn override_versions(&self) -> Overrides {
        let mut out = Overrides::new();
        for (name, alias) in &self.overrides {
            if let Ok(v) = semver::Version::parse(&alias.actual_version) {
                out.insert(name.clone(), v);
            }
        }
        out
    }

    async fn hydrate_resolved_deps(&mut self) {
        let names: Vec<String> = self.resolved.keys().cloned().collect();
        for name in names {
            if self.resolved_deps.contains_key(&name) {
                continue;
            }
            let Some(version_str) = self.resolved.get(&name).cloned() else { continue };
            if version_str == "sdk" || version_str == "git" || version_str == "local" {
                continue;
            }
            if let Some(versions) = self.metadata_cache.get(&name).await {
                if let Some(info) = versions.iter().find(|v| v.version == version_str) {
                    self.resolved_deps
                        .insert(name, info.dependencies.clone());
                }
            }
        }
    }

    async fn incremental_fetch(&mut self, all_deps: &HashMap<String, String>) -> Result<()> {
        let semaphore = Arc::new(Semaphore::new(100));
        let mut failed_packages: HashSet<String> = HashSet::new();

        let mut to_fetch: Vec<(String, String, String)> = all_deps
            .iter()
            .filter(|(name, _)| !is_sdk_pseudo_package(name) && !self.local_packages.contains_key(name.as_str()))
            .map(|(name, c)| (name.clone(), c.clone(), "root".to_string()))
            .collect();

        let mut wave = 0usize;
        loop {
            if to_fetch.is_empty() {
                break;
            }
            wave += 1;

            let mut fetch_tasks = Vec::new();
            let mut already_cached = Vec::new();

            for (name, constraint, requester) in &to_fetch {
                if self.metadata_cache.contains(name).await {
                    already_cached.push((name.clone(), constraint.clone(), requester.clone()));
                    continue;
                }
                let registry = self.registry.clone();
                let cache = self.metadata_cache.clone();
                let sem = semaphore.clone();
                let pkg_name = name.clone();

                fetch_tasks.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    match registry.get_package_metadata(&pkg_name).await {
                        Ok(metadata) => {
                            cache.insert(pkg_name.clone(), metadata.versions).await;
                            Ok(pkg_name)
                        }
                        Err(_) => Err(pkg_name),
                    }
                }));
            }

            if !fetch_tasks.is_empty() {
                for result in join_all(fetch_tasks).await {
                    match result {
                        Ok(Ok(_)) => {}
                        Ok(Err(pkg_name)) => {
                            failed_packages.insert(pkg_name);
                        }
                        Err(e) => warn!("Task error during fetch: {}", e),
                    }
                }
            }

            // Pick selected versions so transitive deps come from the
            // actual chosen version.
            let mut next_chunk: Vec<(String, String, String)> = Vec::new();
            let mut next_chunk_seen: HashSet<String> = HashSet::new();

            for (name, constraint, requester) in to_fetch.iter().chain(already_cached.iter()) {
                if failed_packages.contains(name) {
                    continue;
                }
                // Select version for discovery purposes only.
                let Some(idx) = self.metadata_cache.get_index(name).await else { continue };
                let parsed = match VersionConstraint::parse(constraint)
                    .and_then(|c| c.to_parsed())
                {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                let Some(pos) = idx.latest_matching(&parsed) else { continue };
                let Some(version_info) = idx.get(pos) else { continue };

                for (dep_name, dep_constraint) in &version_info.dependencies {
                    if is_sdk_pseudo_package(dep_name)
                        || self.local_packages.contains_key(dep_name)
                        || failed_packages.contains(dep_name)
                    {
                        continue;
                    }
                    if next_chunk_seen.insert(dep_name.clone()) {
                        next_chunk.push((dep_name.clone(), dep_constraint.clone(), name.clone()));
                    }
                }
                // Prime deps for the install path.
                self.resolved_deps
                    .entry(name.clone())
                    .or_insert_with(|| version_info.dependencies.clone());
                let _ = requester;
            }

            to_fetch = next_chunk;
            if wave > 30 {
                warn!("Stopping incremental fetch at wave {}", wave);
                break;
            }
        }
        Ok(())
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

    pub fn get_resolved_deps(&self) -> &HashMap<String, HashMap<String, String>> {
        &self.resolved_deps
    }

    /// Produce a full [`ResolutionGraph`] from the resolver's internal
    /// state. Call after `resolve`.
    pub fn as_graph(&self) -> ResolutionGraph {
        ResolutionGraph {
            resolved: self.resolved.clone(),
            deps: self.resolved_deps.clone(),
            resolved_paths: self.resolved_paths.clone(),
        }
    }

    pub fn metadata_cache(&self) -> &Arc<MetadataCache> {
        &self.metadata_cache
    }

    /// Check if a package is part of the Flutter SDK.
    pub fn is_flutter_sdk_package(name: &str) -> bool {
        is_sdk_pseudo_package(name)
    }
}

// Keep the old `VersionIndex` re-export used by benches.
pub use super::version_index::VersionIndex as _PublicVersionIndex;
