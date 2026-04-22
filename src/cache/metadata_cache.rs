use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use log::{debug, info, warn};

use crate::registry::traits::PackageVersion;
use crate::cache::paths::CachePaths;
use crate::resolver::version_index::VersionIndex;

/// Stats returned from a prune run.
#[derive(Debug, Default, Clone)]
pub struct PruneStats {
    pub files_removed: usize,
    pub versions_removed: usize,
    pub bytes_freed: u64,
}

/// Global metadata cache that persists between runs.
///
/// The in-memory cache holds an `Arc<Vec<PackageVersion>>` per package so
/// readers do not clone the version vector. On disk, one JSON file per
/// package sits under `~/.hatch/cache/metadata/<pkg>.json`.
pub struct MetadataCache {
    memory_cache: RwLock<HashMap<String, Arc<Vec<PackageVersion>>>>,
    /// Pre-built semver-sorted indices kept in lockstep with `memory_cache`.
    /// Rebuilt on `insert` and cleared alongside any cache invalidation.
    indices: Arc<RwLock<HashMap<String, Arc<VersionIndex>>>>,
}

impl MetadataCache {
    pub fn new() -> Self {
        Self {
            memory_cache: RwLock::new(HashMap::new()),
            indices: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load cached metadata from disk. Also runs a best-effort, gated prune
    /// (once per 24h) that drops stale per-package files and trims each
    /// surviving file to versions that are actually referenced by local
    /// lockfiles or are recent enough to still be interesting.
    pub async fn load_from_disk(&self) -> Result<()> {
        let metadata_dir = CachePaths::metadata_dir()?;
        if !metadata_dir.exists() {
            return Ok(());
        }

        // Best-effort: prune first (but only once per day).
        if let Err(e) = Self::maybe_gc(&metadata_dir).await {
            warn!("metadata cache GC failed (continuing): {e}");
        }

        let mut cache = self.memory_cache.write().await;
        let mut indices = self.indices.write().await;

        // Try to load all metadata files
        if let Ok(entries) = std::fs::read_dir(&metadata_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(contents) = std::fs::read_to_string(&path) {
                    if let Ok(metadata) = serde_json::from_str::<CachedMetadata>(&contents) {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let age = now.saturating_sub(metadata.timestamp);
                        if age < SEVEN_DAYS_SECS {
                            // Build eager index alongside the cached versions.
                            if let Ok(idx) = VersionIndex::from_metadata(&metadata.versions) {
                                indices.insert(metadata.name.clone(), Arc::new(idx));
                            }
                            cache.insert(metadata.name.clone(), Arc::new(metadata.versions));
                            debug!("Loaded cached metadata for {}", metadata.name);
                        }
                    }
                }
            }
        }

        info!("Loaded {} packages from metadata cache", cache.len());
        Ok(())
    }

    /// Save metadata to disk for persistence (static method, no self needed)
    async fn save_to_disk_static(package: &str, versions: &[PackageVersion]) -> Result<()> {
        let metadata_dir = CachePaths::metadata_dir()?;
        std::fs::create_dir_all(&metadata_dir)?;

        let metadata = CachedMetadata {
            name: package.to_string(),
            versions: versions.to_vec(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_secs(),
        };

        let path = metadata_dir.join(format!("{}.json", package));
        let json = serde_json::to_string(&metadata)?;
        std::fs::write(path, json)?;

        Ok(())
    }

    /// Save metadata to disk for persistence
    pub async fn save_to_disk(&self, package: &str, versions: &[PackageVersion]) -> Result<()> {
        Self::save_to_disk_static(package, versions).await
    }

    pub fn get_cache(&self) -> &RwLock<HashMap<String, Arc<Vec<PackageVersion>>>> {
        &self.memory_cache
    }

    /// Get package versions. Returns Arc to avoid cloning the entire Vec.
    pub async fn get(&self, package: &str) -> Option<Arc<Vec<PackageVersion>>> {
        let cache = self.memory_cache.read().await;
        cache.get(package).cloned() // Clones the Arc (cheap), not the Vec
    }

    pub async fn insert(&self, package: String, versions: Vec<PackageVersion>) {
        let arc_versions = Arc::new(versions);

        // Build the index eagerly so hot callers never have to.
        let index = VersionIndex::from_metadata(&arc_versions)
            .ok()
            .map(Arc::new);

        {
            let mut cache = self.memory_cache.write().await;
            cache.insert(package.clone(), arc_versions.clone());
        }
        if let Some(idx) = index {
            let mut indices = self.indices.write().await;
            indices.insert(package.clone(), idx);
        } else {
            // Parsing failed somehow – invalidate any stale index entry.
            let mut indices = self.indices.write().await;
            indices.remove(&package);
        }
        // Write locks dropped before spawning.

        // Save to disk in background using static method (no new cache instance)
        let package_clone = package;
        let versions_clone = arc_versions;
        tokio::spawn(async move {
            let _ = Self::save_to_disk_static(&package_clone, &versions_clone).await;
        });
    }

    pub async fn contains(&self, package: &str) -> bool {
        let cache = self.memory_cache.read().await;
        cache.contains_key(package)
    }

    /// Fetch the pre-built semver-sorted index for a package, if cached.
    pub async fn get_index(&self, name: &str) -> Option<Arc<VersionIndex>> {
        let indices = self.indices.read().await;
        indices.get(name).cloned()
    }

    // ------------------------------------------------------------------
    // Pruning
    // ------------------------------------------------------------------

    /// Check the gate file and, if a full day has passed (or the gate is
    /// missing), run a prune pass. Always updates the gate file on success.
    async fn maybe_gc(metadata_dir: &Path) -> Result<()> {
        let gate = Self::gc_gate_path()?;
        if let Ok(meta) = std::fs::metadata(&gate) {
            if let Ok(modified) = meta.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed < Duration::from_secs(SEVEN_DAYS_SECS).min(Duration::from_secs(24 * 3600)) {
                        // We gate on 24h regardless of TTL.
                        if elapsed < Duration::from_secs(24 * 3600) {
                            return Ok(());
                        }
                    }
                }
            }
        }

        debug!("Running metadata cache GC pass");
        let lockfile_refs = gather_lockfile_versions();
        let _ = prune_directory(metadata_dir, &lockfile_refs, /*aggressive=*/ false)?;

        // Touch the gate file.
        if let Some(parent) = gate.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&gate, b"");
        Ok(())
    }

    fn gc_gate_path() -> Result<PathBuf> {
        Ok(CachePaths::root()?.join(".last_metadata_gc"))
    }

    /// Run a prune pass right now, ignoring the once-per-day gate. Returns
    /// the stats so callers (the `hatch cache prune --aggressive` command)
    /// can surface them.
    pub fn prune_aggressive(&self) -> Result<PruneStats> {
        let metadata_dir = CachePaths::metadata_dir()?;
        if !metadata_dir.exists() {
            return Ok(PruneStats::default());
        }

        let lockfile_refs = gather_lockfile_versions();
        let stats = prune_directory(&metadata_dir, &lockfile_refs, /*aggressive=*/ true)?;

        // Update the gate file so a follow-up `load_from_disk` doesn't
        // immediately re-prune.
        if let Ok(gate) = Self::gc_gate_path() {
            if let Some(parent) = gate.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&gate, b"");
        }

        Ok(stats)
    }
}

// ----------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------

const SEVEN_DAYS_SECS: u64 = 7 * 24 * 3600;
const THIRTY_DAYS_SECS: u64 = 30 * 24 * 3600;

/// Collect every `<package> -> set(version)` pair referenced by any
/// lockfile in the current working directory. Walks both `hatch.lock` and
/// `pubspec.lock`. Returns `HashMap<package, HashSet<version>>`; if no
/// lockfiles are present the map is empty and callers fall back to the
/// "keep latest N" default.
fn gather_lockfile_versions() -> HashMap<String, HashSet<String>> {
    let mut out: HashMap<String, HashSet<String>> = HashMap::new();

    // hatch.lock
    let hatch_lock = std::env::current_dir()
        .ok()
        .map(|p| p.join("hatch.lock"));
    if let Some(path) = hatch_lock {
        if path.exists() {
            if let Ok(lock) = crate::lockfile::LockfileParser::parse(&path) {
                for (name, pkg) in &lock.packages {
                    out.entry(name.clone()).or_default().insert(pkg.version.clone());
                }
            }
        }
    }

    // pubspec.lock (minimal yaml walk: look for `name` + `version` under the
    // packages map).
    let pubspec_lock_path = std::env::current_dir()
        .ok()
        .map(|p| p.join("pubspec.lock"));
    if let Some(path) = pubspec_lock_path {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(serde_yaml::Value::Mapping(root)) =
                serde_yaml::from_str::<serde_yaml::Value>(&content)
            {
                if let Some(serde_yaml::Value::Mapping(packages)) =
                    root.get(serde_yaml::Value::String("packages".into()))
                {
                    for (k, v) in packages {
                        if let (serde_yaml::Value::String(name), serde_yaml::Value::Mapping(pkg)) =
                            (k, v)
                        {
                            if let Some(serde_yaml::Value::String(ver)) =
                                pkg.get(serde_yaml::Value::String("version".into()))
                            {
                                out.entry(name.clone()).or_default().insert(ver.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    out
}

fn prune_directory(
    metadata_dir: &Path,
    lockfile_refs: &HashMap<String, HashSet<String>>,
    aggressive: bool,
) -> Result<PruneStats> {
    let mut stats = PruneStats::default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let entries = match std::fs::read_dir(metadata_dir) {
        Ok(e) => e,
        Err(_) => return Ok(stats),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let size_before = contents.len() as u64;

        let mut metadata: CachedMetadata = match serde_json::from_str(&contents) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Drop file entirely if older than 7 days.
        let age = now.saturating_sub(metadata.timestamp);
        if age > SEVEN_DAYS_SECS {
            if std::fs::remove_file(&path).is_ok() {
                stats.files_removed += 1;
                stats.bytes_freed = stats.bytes_freed.saturating_add(size_before);
            }
            continue;
        }

        let referenced: HashSet<String> = lockfile_refs
            .get(&metadata.name)
            .cloned()
            .unwrap_or_default();

        let before_count = metadata.versions.len();
        let have_lockfile_info = !lockfile_refs.is_empty();

        let kept: Vec<PackageVersion> = if aggressive {
            // Aggressive: keep ONLY what lockfiles reference. If no
            // lockfiles, keep nothing – caller explicitly asked for this.
            metadata
                .versions
                .into_iter()
                .filter(|v| referenced.contains(&v.version))
                .collect()
        } else if have_lockfile_info {
            // Non-aggressive: keep lockfile refs + anything published in
            // the last 30 days.
            metadata
                .versions
                .into_iter()
                .filter(|v| {
                    if referenced.contains(&v.version) {
                        return true;
                    }
                    if let Some(pub_ts) = v
                        .published
                        .as_ref()
                        .and_then(|p| chrono::DateTime::parse_from_rfc3339(p).ok())
                    {
                        let published_secs = pub_ts.timestamp() as u64;
                        return now.saturating_sub(published_secs) <= THIRTY_DAYS_SECS;
                    }
                    false
                })
                .collect()
        } else {
            // No lockfile at all: keep everything. Truncating to a fixed
            // "latest N" window is unsafe – a project in the same dir may
            // constrain a package to an older version that would be dropped,
            // breaking resolution on the next run.
            metadata.versions
        };

        let after_count = kept.len();
        if after_count >= before_count {
            continue; // nothing to rewrite
        }

        stats.versions_removed += before_count - after_count;

        if kept.is_empty() && aggressive {
            // Drop the whole file if we emptied it.
            if std::fs::remove_file(&path).is_ok() {
                stats.files_removed += 1;
                stats.bytes_freed = stats.bytes_freed.saturating_add(size_before);
            }
            continue;
        }

        metadata.versions = kept;
        if let Ok(json) = serde_json::to_string(&metadata) {
            if std::fs::write(&path, &json).is_ok() {
                let size_after = json.len() as u64;
                if size_before > size_after {
                    stats.bytes_freed =
                        stats.bytes_freed.saturating_add(size_before - size_after);
                }
            }
        }
    }

    Ok(stats)
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
