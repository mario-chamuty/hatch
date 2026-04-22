//! Manifest-hash resolution cache.
//!
//! A single JSON file per (hash-of-manifest-inputs) that memoises the
//! resolver's output. On a warm run we can skip propagation and the solver
//! entirely by looking up the hash and verifying that every referenced
//! version is still present in the live metadata cache.
//!
//! Cache is intentionally skipped when the manifest contains any local-path
//! dependency: those can change freely under the resolver's feet.

use anyhow::{anyhow, Result};
use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cache::metadata_cache::MetadataCache;
use crate::cache::paths::CachePaths;
use crate::manifest::schema::HatchManifest;

pub const RESOLVER_ALGO_VERSION: u32 = 1;
pub const LOCKFILE_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_TTL_SECONDS: u64 = 86_400;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionCacheEntry {
    pub resolved: BTreeMap<String, String>,
    #[serde(default)]
    pub resolved_paths: BTreeMap<String, String>,
    pub generated_at: i64,
    pub ttl_seconds: u64,
    pub hatch_version: String,
    pub resolver_algo_version: u32,
    pub lockfile_schema_version: u32,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Environment kill-switch – useful for tests and debugging.
fn disabled() -> bool {
    std::env::var("HATCH_NO_RESOLUTION_CACHE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Detects any local path dependency in the manifest. When true the caller
/// must skip the resolution cache entirely.
pub fn has_path_dep(manifest: &HatchManifest) -> bool {
    for bag in [&manifest.require, &manifest.require_dev].iter().copied() {
        if let Some(map) = bag {
            for dep in map.values() {
                if dep.local_path().is_some() {
                    return true;
                }
            }
        }
    }
    if let Some(legacy) = &manifest.local_packages {
        if !legacy.is_empty() {
            return true;
        }
    }
    false
}

/// Compute the hash key that uniquely identifies this set of resolver
/// inputs. Returns `Ok(None)` if a path dep was detected (caller should skip
/// the cache) or `Ok(Some(hex))` otherwise.
pub fn compute_manifest_hash(manifest: &HatchManifest) -> Result<Option<String>> {
    if has_path_dep(manifest) {
        return Ok(None);
    }

    let mut lines: Vec<String> = Vec::new();

    for (label, bag) in [
        ("require", &manifest.require),
        ("require-dev", &manifest.require_dev),
    ] {
        if let Some(map) = bag {
            let mut sorted: Vec<(&String, &crate::manifest::dependency::Dependency)> =
                map.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            for (name, dep) in sorted {
                lines.push(format!("{}:{}={}", label, name, dep_key(dep)));
            }
        }
    }

    if let Some(ov) = &manifest.overrides {
        let mut sorted: Vec<(&String, &String)> = ov.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in sorted {
            lines.push(format!("override:{}={}", k, v));
        }
    }

    if let Some(dart) = &manifest.sdk.dart {
        lines.push(format!("dart={}", dart));
    }
    if let Some(flutter) = &manifest.sdk.flutter {
        lines.push(format!("flutter={}", flutter));
    }

    let pub_hosted = std::env::var("PUB_HOSTED_URL").unwrap_or_default();
    lines.push(format!("PUB_HOSTED_URL={}", pub_hosted));
    lines.push(format!("HATCH_VERSION={}", env!("CARGO_PKG_VERSION")));
    lines.push(format!("RESOLVER_ALGO_VERSION={}", RESOLVER_ALGO_VERSION));
    lines.push(format!(
        "LOCKFILE_SCHEMA_VERSION={}",
        LOCKFILE_SCHEMA_VERSION
    ));

    let joined = lines.join("\n");
    let mut hasher = Hasher::new();
    hasher.update(joined.as_bytes());
    let digest = hasher.finalize();
    let bytes = digest.as_bytes();
    // First 16 bytes, lowercase hex.
    let hex: String = bytes[..16]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    Ok(Some(hex))
}

fn dep_key(dep: &crate::manifest::dependency::Dependency) -> String {
    use crate::manifest::dependency::Dependency;
    match dep {
        Dependency::Simple(s) => s.clone(),
        Dependency::Complex(c) => {
            // Stable rendering – serialize into a deterministic shape.
            let mut parts: Vec<String> = vec![format!("version={}", c.version)];
            if let Some(g) = &c.git {
                parts.push(format!("git={}", g));
                if let Some(r) = &c.git_ref {
                    parts.push(format!("ref={}", r));
                }
            }
            if let Some(n) = &c.nest {
                parts.push(format!("nest={}", n));
            }
            if let Some(s) = &c.sdk {
                parts.push(format!("sdk={}", s));
            }
            parts.join(";")
        }
    }
}

fn cache_dir() -> Result<PathBuf> {
    Ok(CachePaths::root()?.join("resolutions"))
}

pub fn cache_path(hash: &str) -> Result<PathBuf> {
    Ok(cache_dir()?.join(format!("{}.json", hash)))
}

/// Load a cache entry if it exists, is fresh, and matches the running
/// hatch/algo versions. Also checks that each referenced version is still
/// present in the metadata cache (a retracted version invalidates the
/// entry).
pub async fn try_load(hash: &str, metadata: &MetadataCache) -> Option<ResolutionCacheEntry> {
    if disabled() {
        return None;
    }
    let path = cache_path(hash).ok()?;
    if !path.exists() {
        return None;
    }
    let contents = std::fs::read_to_string(&path).ok()?;
    let entry: ResolutionCacheEntry = serde_json::from_str(&contents).ok()?;

    if entry.hatch_version != env!("CARGO_PKG_VERSION") {
        return None;
    }
    if entry.resolver_algo_version != RESOLVER_ALGO_VERSION {
        return None;
    }
    let now = now_secs();
    if now.saturating_sub(entry.generated_at) as u64 >= entry.ttl_seconds {
        return None;
    }

    // Retraction guard: every resolved version must still appear in the
    // live metadata cache. If the version is missing the entry is stale.
    for (name, version) in &entry.resolved {
        let Some(versions) = metadata.get(name).await else {
            return None;
        };
        if !versions.iter().any(|v| &v.version == version) {
            return None;
        }
    }

    Some(entry)
}

/// Persist a cache entry for the given hash. Best-effort; I/O errors are
/// logged and swallowed.
pub fn write(
    hash: &str,
    resolved: &BTreeMap<String, String>,
    resolved_paths: &BTreeMap<String, String>,
) -> Result<()> {
    if disabled() {
        return Ok(());
    }
    let dir = cache_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow!("resolution_cache: mkdir {}: {}", dir.display(), e))?;
    let path = cache_path(hash)?;

    let entry = ResolutionCacheEntry {
        resolved: resolved.clone(),
        resolved_paths: resolved_paths.clone(),
        generated_at: now_secs(),
        ttl_seconds: DEFAULT_TTL_SECONDS,
        hatch_version: env!("CARGO_PKG_VERSION").to_string(),
        resolver_algo_version: RESOLVER_ALGO_VERSION,
        lockfile_schema_version: LOCKFILE_SCHEMA_VERSION,
    };
    let json = serde_json::to_string_pretty(&entry)
        .map_err(|e| anyhow!("resolution_cache: serialize: {}", e))?;
    std::fs::write(&path, json)
        .map_err(|e| anyhow!("resolution_cache: write {}: {}", path.display(), e))?;
    Ok(())
}
