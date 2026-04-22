//! Resolver conflict reporting.
//!
//! Feed the solver a hand-crafted conflict (two dependents want disjoint
//! ranges of the same transitive package) and confirm the resulting error
//! is a `ResolverError::NoSolution` whose rendered message mentions the
//! conflicting package in a "because"-style derivation.
//!
//! Offline: we seed every package that pubgrub will look up into the
//! metadata cache up-front, so the `ensure_metadata` hook is a no-op.

use std::collections::HashMap;
use std::sync::Arc;

use hatch::cache::metadata_cache::MetadataCache;
use hatch::manifest::schema::SdkConstraints;
use hatch::registry::traits::{PackageVersion, ParsedConstraint, VersionConstraint};
use hatch::resolver::error::ResolverError;
use hatch::resolver::pubgrub_adapter;

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

async fn seed(cache: &MetadataCache, name: &str, versions: Vec<PackageVersion>) {
    cache.insert(name.to_string(), versions).await;
}

fn range(c: &str) -> ParsedConstraint {
    VersionConstraint::parse(c)
        .and_then(|vc| vc.to_parsed())
        .expect("parse constraint")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disjoint_transitive_ranges_produce_no_solution() {
    // Use a unique cache dir per test process so the on-disk prune/save
    // paths don't collide with other test binaries.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    std::env::set_var("HATCH_CACHE_DIR", tmp.path());
    std::env::set_var("HATCH_NO_RESOLUTION_CACHE", "1");

    // Graph:
    //   root -> left ^1.0.0 -> shared ^1.0.0
    //   root -> right ^1.0.0 -> shared ^2.0.0
    // Shared only publishes 1.0.0 and 2.0.0; ^1.0.0 only matches 1.x and
    // ^2.0.0 only matches 2.x, so the two transitive constraints are
    // disjoint and there is NO solution.
    let cache = Arc::new(MetadataCache::new());
    seed(&cache, "left", vec![pv("1.0.0", &[("shared", "^1.0.0")])]).await;
    seed(&cache, "right", vec![pv("1.0.0", &[("shared", "^2.0.0")])]).await;
    seed(
        &cache,
        "shared",
        vec![pv("1.0.0", &[]), pv("2.0.0", &[])],
    )
    .await;

    let mut root_deps: HashMap<String, ParsedConstraint> = HashMap::new();
    root_deps.insert("left".into(), range("^1.0.0"));
    root_deps.insert("right".into(), range("^1.0.0"));

    let sdks = SdkConstraints {
        flutter: Some("3.35.2".into()),
        dart: Some(">=3.5.0 <4.0.0".into()),
    };

    let result = pubgrub_adapter::solve(
        cache.clone(),
        sdks,
        Default::default(),
        HashMap::new(),
        "conflict_root".into(),
        semver::Version::new(0, 0, 0),
        root_deps,
    )
    .await;

    let err = match result {
        Err(e) => e,
        Ok(map) => panic!("expected NoSolution, got resolution: {map:?}"),
    };

    match err {
        ResolverError::NoSolution(msg) => {
            // pubgrub's DefaultStringReporter uses "because" / "depends on"
            // phrasing. Sanity-check that the conflicting package name
            // appears and the pub-style wording is present.
            assert!(
                msg.contains("shared"),
                "expected 'shared' in conflict explanation, got:\n{msg}"
            );
            let lower = msg.to_lowercase();
            assert!(
                lower.contains("because")
                    || lower.contains("depends on")
                    || lower.contains("no versions"),
                "expected pub-style conflict prose, got:\n{msg}"
            );
        }
        other => panic!("expected ResolverError::NoSolution, got: {other:?}"),
    }
}
