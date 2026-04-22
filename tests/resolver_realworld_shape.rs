//! Regression coverage for the resolver pipeline against real pub.dev-shaped
//! metadata.
//!
//! The bug these tests guard against: pubgrub panicked with
//!   "add_derivation should not be called after a decision"
//! when the resolver was fed metadata with many versions per package, where
//! the synthetic root's dependencies (built from `ParsedConstraint`) were
//! added to the residual AND overlapped with packages the propagator had
//! already resolved. The panic reproduced with
//! `HATCH_NO_SUBGRAPH_PARALLEL=1` and `HATCH_NO_RESOLUTION_CACHE=1`, i.e. it
//! was a bug in the propagation -> pubgrub composition path itself, not in
//! the parallel fan-out.
//!
//! These tests drive the pipeline end-to-end with synthetic metadata that
//! matches the pub.dev shape (many versions per package, typical Dart
//! constraint strings) and assert that the solver returns either a valid
//! graph or a `NoSolution` error – never a panic.

use std::collections::HashMap;

use hatch::cache::metadata_cache::MetadataCache;
use hatch::manifest::schema::SdkConstraints;
use hatch::registry::traits::{ParsedConstraint, PackageVersion, VersionConstraint};
use hatch::resolver::propagation::Propagator;
use hatch::resolver::pubgrub_adapter;

/// Build a `PackageVersion` from (version, deps) where deps is a slice of
/// `(name, constraint)` pairs.
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

fn parse(c: &str) -> ParsedConstraint {
    VersionConstraint::parse(c)
        .and_then(|x| x.to_parsed())
        .expect("parse constraint")
}

/// Seed the cache with 10 packages, each having 20 versions, each version
/// declaring 3-5 deps that form a realistic dependency graph.
async fn seed_realistic(cache: &MetadataCache) {
    // `core_utils` – leaf-like dep, 20 patch versions of 1.x.
    let mut core_utils = Vec::new();
    for patch in 0..20 {
        core_utils.push(pv(&format!("1.{}.0", patch), &[]));
    }
    cache.insert("core_utils".into(), core_utils).await;

    // `string_helpers` – depends on core_utils.
    let mut string_helpers = Vec::new();
    for patch in 0..20 {
        let core_constraint = format!("^1.{}.0", patch.min(10));
        string_helpers.push(pv(
            &format!("2.{}.0", patch),
            &[("core_utils", &core_constraint)],
        ));
    }
    cache.insert("string_helpers".into(), string_helpers).await;

    // `net_io` – depends on string_helpers + core_utils.
    let mut net_io = Vec::new();
    for patch in 0..20 {
        net_io.push(pv(
            &format!("0.{}.0", patch),
            &[
                ("string_helpers", "^2.0.0"),
                ("core_utils", ">=1.0.0 <2.0.0"),
            ],
        ));
    }
    cache.insert("net_io".into(), net_io).await;

    // `http_client` – depends on net_io.
    let mut http_client = Vec::new();
    for patch in 0..20 {
        http_client.push(pv(
            &format!("3.{}.0", patch),
            &[("net_io", "^0.5.0"), ("core_utils", "^1.0.0")],
        ));
    }
    cache.insert("http_client".into(), http_client).await;

    // `json_api` – depends on http_client.
    let mut json_api = Vec::new();
    for patch in 0..20 {
        json_api.push(pv(
            &format!("4.{}.0", patch),
            &[("http_client", "^3.0.0"), ("string_helpers", "^2.0.0")],
        ));
    }
    cache.insert("json_api".into(), json_api).await;

    // `auth_lib` – depends on http_client + core_utils.
    let mut auth_lib = Vec::new();
    for patch in 0..20 {
        auth_lib.push(pv(
            &format!("1.{}.0", patch),
            &[("http_client", "^3.0.0"), ("core_utils", "^1.0.0")],
        ));
    }
    cache.insert("auth_lib".into(), auth_lib).await;

    // `db_adapter` – depends on core_utils only.
    let mut db_adapter = Vec::new();
    for patch in 0..20 {
        db_adapter.push(pv(
            &format!("5.{}.0", patch),
            &[("core_utils", ">=1.0.0 <2.0.0")],
        ));
    }
    cache.insert("db_adapter".into(), db_adapter).await;

    // `logging` – leaf.
    let mut logging = Vec::new();
    for patch in 0..20 {
        logging.push(pv(&format!("6.{}.0", patch), &[]));
    }
    cache.insert("logging".into(), logging).await;

    // `metrics` – depends on logging + net_io.
    let mut metrics = Vec::new();
    for patch in 0..20 {
        metrics.push(pv(
            &format!("7.{}.0", patch),
            &[("logging", "^6.0.0"), ("net_io", "^0.5.0")],
        ));
    }
    cache.insert("metrics".into(), metrics).await;

    // `app_core` – top-level app package. Depends on json_api, auth_lib,
    // db_adapter, metrics – a four-way fan-out.
    let mut app_core = Vec::new();
    for patch in 0..20 {
        app_core.push(pv(
            &format!("8.{}.0", patch),
            &[
                ("json_api", "^4.0.0"),
                ("auth_lib", "^1.0.0"),
                ("db_adapter", "^5.0.0"),
                ("metrics", "^7.0.0"),
            ],
        ));
    }
    cache.insert("app_core".into(), app_core).await;
}

/// Realistic shape test: 10 packages * 20 versions, overlapping constraints,
/// multi-level transitive deps. The solver must return a valid graph.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn realworld_shape_resolves_cleanly() {
    std::env::set_var("HATCH_NO_RESOLUTION_CACHE", "1");

    let cache = std::sync::Arc::new(MetadataCache::new());
    seed_realistic(&cache).await;

    let mut root_deps = HashMap::new();
    root_deps.insert("app_core".into(), parse("^8.0.0"));
    root_deps.insert("logging".into(), parse("^6.0.0"));

    let sdks = SdkConstraints { flutter: None, dart: None };
    let overrides = HashMap::new();

    // Run propagation first – same ordering as `Resolver::resolve`.
    let propagator = Propagator::new(&cache, sdks.clone(), overrides.clone());
    let prop = propagator
        .propagate(&root_deps)
        .await
        .expect("propagate");

    // Anything left in `partial` (or any conflict) should still produce a
    // valid solve via pubgrub without panicking.
    if !prop.partial.is_empty() || !prop.conflicts.is_empty() {
        let mut residual = prop.partial.clone();
        for (name, c) in &root_deps {
            residual.entry(name.clone()).or_insert_with(|| c.clone());
        }
        let out = pubgrub_adapter::solve(
            cache.clone(),
            sdks,
            overrides,
            HashMap::new(),
            "synthetic_root".into(),
            semver::Version::new(0, 0, 0),
            residual,
        )
        .await;
        assert!(
            out.is_ok() || matches!(out, Err(hatch::resolver::error::ResolverError::NoSolution(_))),
            "solver panicked or returned unexpected error: {out:?}"
        );
    }

    // The merged result must cover app_core + all transitives.
    let mut final_resolved: HashMap<String, semver::Version> = prop.resolved.clone();
    if !prop.partial.is_empty() {
        let mut residual = prop.partial.clone();
        for (name, c) in &root_deps {
            residual.entry(name.clone()).or_insert_with(|| c.clone());
        }
        let out = pubgrub_adapter::solve(
            cache.clone(),
            SdkConstraints { flutter: None, dart: None },
            HashMap::new(),
            HashMap::new(),
            "synthetic_root".into(),
            semver::Version::new(0, 0, 0),
            residual,
        )
        .await
        .expect("pubgrub solve");
        for (k, v) in out {
            final_resolved.entry(k).or_insert(v);
        }
    }

    assert!(
        final_resolved.contains_key("app_core"),
        "expected app_core in solution: {final_resolved:?}"
    );
    assert!(
        final_resolved.contains_key("core_utils"),
        "expected core_utils (transitive) in solution"
    );
}

/// This test specifically reproduces the `add_derivation after decision`
/// panic that fired in the bench. It drives the same shape the bench did:
/// a package whose cache contains many versions, with some transitive deps
/// whose metadata is NOT cached (forcing the adapter to treat them as
/// Unknown), while the root constraint overlaps with a propagation-resolved
/// package. Prior to the fix this panicked inside pubgrub.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pubgrub_does_not_panic_on_sparse_metadata() {
    std::env::set_var("HATCH_NO_RESOLUTION_CACHE", "1");

    let cache = std::sync::Arc::new(MetadataCache::new());

    // `alpha` has 30 versions. Every version depends on `beta`, which has
    // no metadata cached at all. This matches the bench's trigger (http
    // depends on unittest, which was not in the warm cache).
    let mut alpha = Vec::new();
    for patch in 0..30 {
        alpha.push(pv(&format!("1.{}.0", patch), &[("beta", "any")]));
    }
    cache.insert("alpha".into(), alpha).await;

    // `beta` is deliberately absent. The adapter will observe
    // `Dependencies::Unknown` when pubgrub asks for its deps.

    let mut root_deps = HashMap::new();
    root_deps.insert("alpha".into(), parse("^1.0.0"));

    let mut residual = root_deps.clone();
    residual.insert("beta".into(), ParsedConstraint::any());

    let result = pubgrub_adapter::solve(
        cache.clone(),
        SdkConstraints { flutter: None, dart: None },
        HashMap::new(),
        HashMap::new(),
        "synthetic_root".into(),
        semver::Version::new(0, 0, 0),
        residual,
    )
    .await;

    // Must not panic. Either we get a solution (if beta metadata is
    // somehow filled in) or a clean NoSolution / Io error. The critical
    // assertion is the absence of the pubgrub internal panic.
    match result {
        Ok(_) => {}
        Err(hatch::resolver::error::ResolverError::NoSolution(_)) => {}
        Err(hatch::resolver::error::ResolverError::Io(msg)) => {
            assert!(
                !msg.contains("add_derivation should not be called after a decision"),
                "regression: pubgrub panicked: {msg}"
            );
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

/// Flutter-shaped regression. This is the exact trigger the bench hit: a
/// package (`http`) whose only cached version has a dep on another package
/// (`unittest`) that has a large version list where most versions carry
/// their own interesting constraint shapes. Propagation can pick http
/// easily, but the residual it hands to pubgrub includes both
/// propagation-resolved keys (because `Resolver::resolve` merges
/// `prop.partial` with `root_constraints`) and packages whose metadata the
/// propagator could not load.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sparse_transitive_does_not_panic() {
    let _ = env_logger::builder().is_test(true).try_init();
    std::env::set_var("HATCH_NO_RESOLUTION_CACHE", "1");

    let cache = std::sync::Arc::new(MetadataCache::new());

    // Mimic http: 130 versions, each depending on `unittest: any`.
    let mut http = Vec::new();
    for patch in 0..130 {
        let major = patch / 50;
        let minor = (patch % 50) / 10;
        let bug = patch % 10;
        http.push(pv(
            &format!("{}.{}.{}", major, minor, bug),
            &[("unittest", "any")],
        ));
    }
    cache.insert("http".into(), http).await;

    // Mimic provider: 30 versions of 6.x with a nested dep that also has
    // many versions.
    let mut provider = Vec::new();
    for patch in 0..30 {
        provider.push(pv(
            &format!("6.{}.0", patch),
            &[("nested", "^1.0.0"), ("collection", ">=1.15.0 <2.0.0")],
        ));
    }
    cache.insert("provider".into(), provider).await;

    // Nested: 30 versions.
    let mut nested = Vec::new();
    for patch in 0..30 {
        nested.push(pv(&format!("1.{}.0", patch), &[]));
    }
    cache.insert("nested".into(), nested).await;

    // Collection: 40 versions spanning 1.15-1.19, with a cross-dep on
    // `meta` which also has many versions.
    let mut collection = Vec::new();
    for patch in 0..40 {
        collection.push(pv(
            &format!("1.{}.{}", 15 + patch / 10, patch % 10),
            &[("meta", ">=1.3.0 <2.0.0")],
        ));
    }
    cache.insert("collection".into(), collection).await;

    // Meta: 30 versions.
    let mut meta = Vec::new();
    for patch in 0..30 {
        meta.push(pv(&format!("1.{}.0", patch), &[]));
    }
    cache.insert("meta".into(), meta).await;

    // Cupertino_icons: 30 versions.
    let mut ci = Vec::new();
    for patch in 0..30 {
        ci.push(pv(&format!("1.0.{}", patch), &[]));
    }
    cache.insert("cupertino_icons".into(), ci).await;

    // unittest intentionally NOT cached – drives the sparse-metadata path.

    let mut root_deps = HashMap::new();
    root_deps.insert("provider".into(), parse("^6.0.0"));
    root_deps.insert("cupertino_icons".into(), parse("^1.0.2"));
    root_deps.insert("http".into(), parse("^0.2.0"));

    // Replicate `Resolver::resolve`'s residual construction.
    let propagator = Propagator::new(
        &cache,
        SdkConstraints { flutter: None, dart: None },
        HashMap::new(),
    );
    let prop = propagator
        .propagate(&root_deps)
        .await
        .expect("propagate");

    let mut residual = prop.partial.clone();
    for (name, c) in &root_deps {
        residual.entry(name.clone()).or_insert_with(|| c.clone());
    }

    let out = pubgrub_adapter::solve(
        cache.clone(),
        SdkConstraints { flutter: None, dart: None },
        HashMap::new(),
        HashMap::new(),
        "synthetic_root".into(),
        semver::Version::new(0, 0, 0),
        residual,
    )
    .await;

    match out {
        Ok(_) | Err(hatch::resolver::error::ResolverError::NoSolution(_)) => {}
        Err(hatch::resolver::error::ResolverError::Io(msg)) => {
            assert!(
                !msg.contains("add_derivation should not be called after a decision"),
                "regression: pubgrub panicked internally: {msg}"
            );
        }
        Err(e) => panic!("unexpected error kind: {e:?}"),
    }
}

/// End-to-end reproduction of the bench trigger. Feeds the real
/// `UltraResolver` pipeline the basic test project shape with a manifest
/// whose only cache-primed metadata for `logging` starts at version 6.0.0
/// (matching the user's warm cache), but whose `http ^0.2.7+0` would
/// transitively demand logging < 0.12.0. The old code panicked inside
/// pubgrub; after the fix the resolver must return `NoSolution` with a
/// clean explanation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_returns_nosolution_not_panic_on_conflicting_deps() {
    let _ = env_logger::builder().is_test(true).try_init();
    std::env::set_var("HATCH_NO_RESOLUTION_CACHE", "1");
    std::env::set_var("HATCH_NO_SUBGRAPH_PARALLEL", "1");

    let cache = std::sync::Arc::new(MetadataCache::new());

    // http 0.2.7+0 depends on logging ^0.9.0 (simulated).
    cache
        .insert(
            "http".into(),
            vec![pv("0.2.7", &[("logging", "^0.9.0")])],
        )
        .await;
    // logging has only 6.x available (matches the user's warm cache).
    let mut logging = Vec::new();
    for minor in 0..20 {
        logging.push(pv(&format!("6.{}.0", minor), &[]));
    }
    cache.insert("logging".into(), logging).await;

    let mut root_deps = HashMap::new();
    root_deps.insert("http".into(), parse("^0.2.7"));

    let out = pubgrub_adapter::solve(
        cache.clone(),
        SdkConstraints { flutter: None, dart: None },
        HashMap::new(),
        HashMap::new(),
        "app_under_test".into(),
        semver::Version::new(0, 0, 0),
        root_deps,
    )
    .await;

    match out {
        Err(hatch::resolver::error::ResolverError::NoSolution(_)) => {}
        Err(hatch::resolver::error::ResolverError::Io(msg)) => {
            assert!(
                !msg.contains("add_derivation should not be called after a decision"),
                "regression: pubgrub panicked internally: {msg}"
            );
        }
        other => panic!("expected NoSolution, got: {other:?}"),
    }
}

/// Tight reproduction: propagation resolves packages, then residual is
/// constructed the same way `Resolver::resolve` does it (`partial + root`),
/// which means PROP-RESOLVED packages also appear in residual. If the
/// adapter treats that naively it trips pubgrub's decision invariant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn residual_merged_with_root_does_not_panic() {
    std::env::set_var("HATCH_NO_RESOLUTION_CACHE", "1");
    let cache = std::sync::Arc::new(MetadataCache::new());
    seed_realistic(&cache).await;

    let mut root_deps = HashMap::new();
    root_deps.insert("app_core".into(), parse("^8.0.0"));
    root_deps.insert("logging".into(), parse("^6.0.0"));
    root_deps.insert("json_api".into(), parse("^4.0.0"));

    // Force partial-like behaviour by ALSO including a package whose
    // metadata is missing. Pubgrub will try to pick a version, fail to
    // find one, and backtrack. Before the fix that backtrack path could
    // panic.
    let propagator = Propagator::new(
        &cache,
        SdkConstraints { flutter: None, dart: None },
        HashMap::new(),
    );
    let prop = propagator
        .propagate(&root_deps)
        .await
        .expect("propagate");

    // Simulate `Resolver::resolve` building the residual.
    let mut residual = prop.partial.clone();
    for (name, c) in &root_deps {
        residual.entry(name.clone()).or_insert_with(|| c.clone());
    }
    // Add a completely unknown package with any constraint.
    residual.insert("missing_pkg".into(), ParsedConstraint::any());

    let out = pubgrub_adapter::solve(
        cache.clone(),
        SdkConstraints { flutter: None, dart: None },
        HashMap::new(),
        HashMap::new(),
        "synthetic_root".into(),
        semver::Version::new(0, 0, 0),
        residual,
    )
    .await;

    match out {
        Ok(_) | Err(hatch::resolver::error::ResolverError::NoSolution(_)) => {}
        Err(hatch::resolver::error::ResolverError::Io(msg)) => {
            assert!(
                !msg.contains("add_derivation should not be called after a decision"),
                "regression: pubgrub panicked internally: {msg}"
            );
        }
        Err(e) => panic!("unexpected error kind: {e:?}"),
    }
}
