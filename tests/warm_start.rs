//! Warm-start behaviour for the resolver.
//!
//! The "warm" path has two layers:
//!
//! 1. Resolution-cache hit – a memoised answer short-circuits the whole
//!    solver when the manifest hash has not changed.
//! 2. Lockfile-biased pubgrub – when the resolution cache cannot be used
//!    (e.g. first run after a manifest tweak), the solver is still biased
//!    toward previously-locked versions via `HatchProvider::locked`.
//!
//! Both are covered here without network access: we pre-populate the
//! metadata cache so `HatchProvider::ensure_metadata` never needs to hit
//! the registry. `CachePaths::root()` uses a process-wide `OnceCell`, so
//! every test in this binary shares a single `HATCH_CACHE_DIR`; we fold
//! the two behaviours into a single test to keep that dir stable.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use hatch::cache::metadata_cache::MetadataCache;
use hatch::manifest::schema::SdkConstraints;
use hatch::registry::traits::{PackageVersion, ParsedConstraint, VersionConstraint};
use hatch::resolver::pubgrub_adapter;
use hatch::resolver::resolution_cache;

fn pv(version: &str, deps: &[(&str, &str)]) -> PackageVersion {
    PackageVersion {
        version: version.to_string(),
        description: None,
        homepage: None,
        repository: None,
        dependencies: deps
            .iter()
            .map(|(n, c)| (n.to_string(), c.to_string()))
            .collect(),
        dev_dependencies: HashMap::new(),
        published: None,
        dart_sdk: None,
        flutter_sdk: None,
        archive_sha256: None,
    }
}

fn range(constraint: &str) -> ParsedConstraint {
    VersionConstraint::parse(constraint)
        .and_then(|c| c.to_parsed())
        .expect("parse constraint")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warm_start_paths_short_circuit_and_bias() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    std::env::set_var("HATCH_CACHE_DIR", tmp.path());
    std::env::remove_var("HATCH_NO_RESOLUTION_CACHE");

    // ------------------------------------------------------------------
    // Layer 1: resolution-cache hit short-circuits the solver entirely.
    // ------------------------------------------------------------------
    let cache = MetadataCache::new();
    cache
        .insert("alpha".to_string(), vec![pv("1.2.3", &[])])
        .await;

    let hash = "warmstartprobe0000000000000000ab";
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    resolved.insert("alpha".to_string(), "1.2.3".to_string());
    let paths: BTreeMap<String, String> = BTreeMap::new();
    resolution_cache::write(hash, &resolved, &paths).expect("write");

    let first = resolution_cache::try_load(hash, &cache)
        .await
        .expect("first try_load should succeed");
    assert_eq!(
        first.resolved.get("alpha").map(String::as_str),
        Some("1.2.3")
    );

    // Second load is identical: warm, no solver invoked, no new writes.
    let before_mtime = std::fs::metadata(
        resolution_cache::cache_path(hash).expect("cache_path"),
    )
    .and_then(|m| m.modified())
    .expect("mtime");
    let second = resolution_cache::try_load(hash, &cache)
        .await
        .expect("second try_load");
    let after_mtime = std::fs::metadata(
        resolution_cache::cache_path(hash).expect("cache_path"),
    )
    .and_then(|m| m.modified())
    .expect("mtime");
    assert_eq!(first.resolved, second.resolved);
    assert_eq!(first.generated_at, second.generated_at);
    assert_eq!(before_mtime, after_mtime, "try_load must not rewrite the file");

    // ------------------------------------------------------------------
    // Layer 2: pubgrub honours the locked version when picking a version.
    // ------------------------------------------------------------------
    // Disable the resolution cache for this leg so we force the solver to
    // actually run instead of hitting the memo we just wrote.
    std::env::set_var("HATCH_NO_RESOLUTION_CACHE", "1");

    let cache_arc = Arc::new(MetadataCache::new());
    cache_arc
        .insert(
            "beta".to_string(),
            vec![pv("1.0.0", &[]), pv("1.1.0", &[]), pv("1.2.0", &[])],
        )
        .await;

    let mut root_deps: HashMap<String, ParsedConstraint> = HashMap::new();
    root_deps.insert("beta".into(), range("^1.0.0"));

    let mut locked: HashMap<String, semver::Version> = HashMap::new();
    locked.insert("beta".into(), semver::Version::new(1, 1, 0));

    let sdks = SdkConstraints {
        flutter: Some("3.35.2".into()),
        dart: Some(">=3.5.0 <4.0.0".into()),
    };

    let out = pubgrub_adapter::solve(
        cache_arc.clone(),
        sdks,
        Default::default(),
        locked,
        "warm_root".into(),
        semver::Version::new(0, 0, 0),
        root_deps,
    )
    .await
    .expect("solve");

    let picked = out.get("beta").cloned().expect("beta in solution");
    assert_eq!(
        picked.to_string(),
        "1.1.0",
        "solver must honour the warm-start lock, picked {picked}"
    );

    std::env::remove_var("HATCH_NO_RESOLUTION_CACHE");
}
