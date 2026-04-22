//! Resolution-cache retraction guard.
//!
//! The cache persists (manifest_hash -> resolved versions). If one of those
//! resolved versions later disappears from the metadata cache (pub.dev
//! retraction, manual cache edit, etc.) the entry MUST be treated as stale
//! and `try_load` must return `None`.
//!
//! Scenario 1: retraction invalidates the cache entry.
//! Scenario 2: HATCH_NO_RESOLUTION_CACHE=1 short-circuits both ends.
//!
//! `CachePaths::root()` uses a process-wide `OnceCell`, so every test in
//! this binary must share a single `HATCH_CACHE_DIR`. We gate everything
//! behind a single `#[tokio::test]` to keep the cache dir stable.

use std::collections::{BTreeMap, HashMap};

use hatch::cache::metadata_cache::MetadataCache;
use hatch::registry::traits::PackageVersion;
use hatch::resolver::resolution_cache;

fn pv(version: &str) -> PackageVersion {
    PackageVersion {
        version: version.to_string(),
        description: None,
        homepage: None,
        repository: None,
        dependencies: HashMap::new(),
        dev_dependencies: HashMap::new(),
        published: None,
        dart_sdk: None,
        flutter_sdk: None,
        archive_sha256: None,
    }
}

#[tokio::test]
async fn retraction_and_kill_switch_behaviours() {
    // Single shared tempdir – must outlive every assertion in this binary.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    std::env::set_var("HATCH_CACHE_DIR", tmp.path());

    // ------------------------------------------------------------------
    // Part 1 – retraction guard.
    // ------------------------------------------------------------------
    std::env::remove_var("HATCH_NO_RESOLUTION_CACHE");
    let cache = MetadataCache::new();
    cache
        .insert("foo".to_string(), vec![pv("1.0.0"), pv("1.1.0")])
        .await;

    let hash = "retractionprobe000000000000000a0";
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    resolved.insert("foo".to_string(), "1.1.0".to_string());
    let paths: BTreeMap<String, String> = BTreeMap::new();
    resolution_cache::write(hash, &resolved, &paths).expect("write cache entry");

    let file = resolution_cache::cache_path(hash).expect("cache_path");
    assert!(file.exists(), "cache file should have been written");

    let entry = resolution_cache::try_load(hash, &cache).await;
    assert!(entry.is_some(), "fresh entry should load");

    // Simulate pub.dev retracting 1.1.0 – only 1.0.0 survives.
    cache.insert("foo".to_string(), vec![pv("1.0.0")]).await;
    let entry_after = resolution_cache::try_load(hash, &cache).await;
    assert!(
        entry_after.is_none(),
        "retracted version must invalidate the cache entry"
    );

    // The file is NOT deleted – guard only refuses to load it. A future
    // resolver run overwrites with a fresh answer.
    assert!(file.exists());

    // ------------------------------------------------------------------
    // Part 2 – HATCH_NO_RESOLUTION_CACHE=1 kill switch.
    // ------------------------------------------------------------------
    std::env::set_var("HATCH_NO_RESOLUTION_CACHE", "1");

    let hash2 = "disableprobe0000000000000000000b";
    let mut resolved2: BTreeMap<String, String> = BTreeMap::new();
    resolved2.insert("foo".to_string(), "1.0.0".to_string());
    resolution_cache::write(hash2, &resolved2, &paths).expect("write is a no-op");
    let file2 = resolution_cache::cache_path(hash2).expect("cache_path");
    assert!(
        !file2.exists(),
        "HATCH_NO_RESOLUTION_CACHE must suppress writes"
    );
    let none = resolution_cache::try_load(hash2, &cache).await;
    assert!(none.is_none(), "HATCH_NO_RESOLUTION_CACHE must suppress loads");

    // And even a previously written entry is ignored under the kill switch.
    let still_none = resolution_cache::try_load(hash, &cache).await;
    assert!(
        still_none.is_none(),
        "kill switch ignores pre-existing entries too"
    );

    std::env::remove_var("HATCH_NO_RESOLUTION_CACHE");
}
