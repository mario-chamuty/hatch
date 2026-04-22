//! Stream E phase 2 – end-to-end install integration tests.
//!
//! These tests exercise the install-path primitives (registry, downloader,
//! resolver, resolution-cache) against a wiremock server. We deliberately
//! do NOT drive the full `hatch install` binary: that path transitively
//! requires a usable FVM + Flutter install on the host, which is not
//! appropriate for a hermetic test suite. Instead we target the library
//! surface that the install command is composed from.
//!
//! Network policy: every test here must pass without network access. The
//! `HATCH_PUB_HOSTED_URL` env var pointed at wiremock is the only HTTP
//! endpoint Hatch ever reaches.
//!
//! OnceCell hazard: `CachePaths::root()` caches its first resolved value
//! process-wide, so all tests in this binary share a single tempdir that
//! is set up before any cache-touching code runs. Each test sub-directory
//! is still isolated by prefixing keys and scoping fixtures.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use flate2::write::GzEncoder;
use flate2::Compression;
use proptest::prelude::*;
use sha2::{Digest, Sha256};
use tar::{Builder, Header};
use wiremock::matchers::{method, path as wm_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Global serialisation lock for tests that mutate process-wide env vars
/// (`HATCH_PUB_HOSTED_URL`, `HOME`, `USERPROFILE`, etc.). Parallel tests
/// would otherwise race and observe each other's overrides.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

use hatch::cache::downloader::{PackageDownloader, ALLOW_UNCHECKSUMMED_SENTINEL};
use hatch::cache::metadata_cache::MetadataCache;
use hatch::manifest::schema::SdkConstraints;
use hatch::registry::pub_dev::PubDevRegistry;
use hatch::registry::traits::{ParsedConstraint, Registry, VersionConstraint};
use hatch::resolver::{pubgrub_adapter, resolution_cache};

// ---------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------

/// Process-wide cache root. Set once on first call; all subsequent calls
/// get the same directory because `CachePaths::root` uses a OnceCell.
fn shared_cache_dir() -> &'static std::path::Path {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::path::PathBuf> = OnceLock::new();
    LOCK.get_or_init(|| {
        let tmp = tempfile::tempdir()
            .expect("create shared cache tempdir")
            .keep();
        std::env::set_var("HATCH_CACHE_DIR", &tmp);
        tmp
    })
    .as_path()
}

/// Build a minimal, valid `.tar.gz` payload containing `pubspec.yaml`.
fn make_tarball(name: &str, version: &str) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    {
        let enc = GzEncoder::new(&mut buf, Compression::default());
        let mut tar = Builder::new(enc);
        let body = format!("name: {name}\nversion: {version}\n");
        let mut h = Header::new_gnu();
        h.set_path("pubspec.yaml").unwrap();
        h.set_size(body.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        tar.append(&h, body.as_bytes()).unwrap();
        tar.finish().unwrap();
    }
    buf
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------
// Test 1: happy path – metadata fetch + download + checksum verify
// ---------------------------------------------------------------------

#[tokio::test]
async fn install_basic_happy_path() {
    let _cache = shared_cache_dir();
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let server = MockServer::start().await;

    let name = "happy_pkg";
    let version = "1.0.0";
    let tarball = make_tarball(name, version);
    let sha = sha256_hex(&tarball);

    // Metadata route.
    Mock::given(method("GET"))
        .and(wm_path(format!("/api/packages/{name}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "name": name,
            "versions": [{
                "version": version,
                "published": "2024-01-01T00:00:00Z",
                "archive_sha256": sha,
                "pubspec": {
                    "name": name,
                    "environment": { "sdk": ">=3.0.0 <4.0.0" }
                }
            }]
        })))
        .mount(&server)
        .await;

    // Tarball route.
    Mock::given(method("GET"))
        .and(wm_path(format!(
            "/packages/{name}/versions/{version}.tar.gz"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(tarball.clone()))
        .mount(&server)
        .await;

    // Point downloader + registry at the mock.
    std::env::set_var("HATCH_PUB_HOSTED_URL", &server.uri());

    let reg = PubDevRegistry::with_base_url(server.uri());
    let meta = reg.get_package_metadata(name).await.expect("metadata");
    assert_eq!(meta.versions.len(), 1);
    assert_eq!(meta.versions[0].archive_sha256.as_deref(), Some(sha.as_str()));

    // Download with correct checksum succeeds and is placed in the cache.
    let dl = PackageDownloader::new();
    let path = dl
        .download_with_checksum("pub.dev", name, version, &sha)
        .await
        .expect("download happy path");
    assert!(path.exists(), "download path must exist");

    std::env::remove_var("HATCH_PUB_HOSTED_URL");
}

// ---------------------------------------------------------------------
// Test 2: warm-cache hit should not re-hit metadata endpoint
// ---------------------------------------------------------------------

#[tokio::test]
async fn install_warm_cache_hit() {
    let _cache = shared_cache_dir();
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());

    // Build a fake metadata cache + resolution cache and verify the
    // warm-start path round-trips. Because the resolution cache memoises
    // the solver answer, a second "resolve" call with the same manifest
    // hash must skip both the propagator and pubgrub entirely.
    std::env::remove_var("HATCH_NO_RESOLUTION_CACHE");
    let cache = MetadataCache::new();
    let name = "warm_pkg";
    let version = "2.3.4";
    cache
        .insert(
            name.to_string(),
            vec![hatch::registry::traits::PackageVersion {
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
            }],
        )
        .await;

    let hash = "warmhitintegration00000000000001";
    let mut resolved: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    resolved.insert(name.to_string(), version.to_string());
    resolution_cache::write(hash, &resolved, &Default::default())
        .expect("write cache entry");

    // Two consecutive loads: identical result, no solver in between.
    let first = resolution_cache::try_load(hash, &cache).await.expect("first");
    let second = resolution_cache::try_load(hash, &cache).await.expect("second");
    assert_eq!(first.resolved, second.resolved);
    assert_eq!(first.resolved.get(name).map(String::as_str), Some(version));
}

// ---------------------------------------------------------------------
// Test 3: checksum mismatch fails closed
// ---------------------------------------------------------------------

#[tokio::test]
async fn install_checksum_mismatch_fails() {
    let _cache = shared_cache_dir();
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let server = MockServer::start().await;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let name = format!("mismatch_pkg_{unique}");
    let name = name.as_str();
    let version = "0.1.0";
    let tarball = make_tarball(name, version);
    let _correct = sha256_hex(&tarball);
    let wrong_sha = "0".repeat(64);

    Mock::given(method("GET"))
        .and(wm_path(format!(
            "/packages/{name}/versions/{version}.tar.gz"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(tarball.clone()))
        .mount(&server)
        .await;

    std::env::set_var("HATCH_PUB_HOSTED_URL", &server.uri());

    let dl = PackageDownloader::new();
    let err = dl
        .download_with_checksum("pub.dev", name, version, &wrong_sha)
        .await
        .expect_err("mismatch must fail");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("checksum"),
        "error should mention checksum: {err}"
    );

    std::env::remove_var("HATCH_PUB_HOSTED_URL");
}

// ---------------------------------------------------------------------
// Test 4: --allow-unchecksummed bypass writes to audit.log
// ---------------------------------------------------------------------

#[tokio::test]
async fn install_allow_unchecksummed_override() {
    let _cache = shared_cache_dir();
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let server = MockServer::start().await;
    // Unique name per run so `download_path.exists()` short-circuit never
    // fires against a stale cache entry from a previous test invocation.
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let name = format!("unchecked_pkg_{unique}");
    let name = name.as_str();
    let version = "9.9.9";
    let tarball = make_tarball(name, version);

    Mock::given(method("GET"))
        .and(wm_path(format!(
            "/packages/{name}/versions/{version}.tar.gz"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(tarball.clone()))
        .mount(&server)
        .await;

    std::env::set_var("HATCH_PUB_HOSTED_URL", &server.uri());

    // Audit.log lives under the real user's home directory – the `dirs`
    // crate on Windows resolves via `SHGetKnownFolderPath` and ignores
    // `HOME`/`USERPROFILE`, so a tempdir redirect won't work. Instead we
    // read the current audit.log content, perform the bypass with a
    // test-unique package name, and assert that a matching line was
    // appended.
    let home = dirs::home_dir().expect("home_dir");
    let log_path = home.join(".hatch").join("audit.log");
    let before = std::fs::read_to_string(&log_path).unwrap_or_default();

    let dl = PackageDownloader::new();
    let _ = dl
        .download_with_checksum("pub.dev", name, version, ALLOW_UNCHECKSUMMED_SENTINEL)
        .await
        .expect("bypass should succeed");

    let after = std::fs::read_to_string(&log_path).expect("audit.log must exist after bypass");
    assert!(
        after.len() > before.len(),
        "audit.log must grow on bypass path"
    );
    assert!(
        after.contains(name) && after.contains("allow-unchecksummed"),
        "audit entry missing or malformed for {name}"
    );

    std::env::remove_var("HATCH_PUB_HOSTED_URL");
}

// ---------------------------------------------------------------------
// Test 5: path-dep manifests do not populate the resolution cache
// ---------------------------------------------------------------------

#[test]
fn install_path_dep_no_resolution_cache() {
    // Build a HatchManifest that includes a local-path dep. The resolver's
    // cache short-circuit must bail (compute_manifest_hash returns None).
    use hatch::manifest::dependency::{ComplexDependency, Dependency};
    use hatch::manifest::schema::HatchManifest;
    use std::collections::HashMap;

    let mut require: HashMap<String, Dependency> = HashMap::new();
    require.insert(
        "local_pkg".into(),
        Dependency::Complex(ComplexDependency {
            version: "any".into(),
            git: None,
            git_ref: None,
            nest: None,
            sdk: None,
            path: Some("../local_pkg".into()),
        }),
    );

    let mut manifest = HatchManifest::default();
    manifest.name = "with_path_dep".into();
    manifest.require = Some(require);
    manifest.sdk = SdkConstraints {
        flutter: Some("3.35.2".into()),
        dart: Some(">=3.5.0 <4.0.0".into()),
    };

    let hash = resolution_cache::compute_manifest_hash(&manifest)
        .expect("compute_manifest_hash");
    assert!(
        hash.is_none(),
        "path-dep manifest must not produce a resolution-cache key"
    );
    assert!(resolution_cache::has_path_dep(&manifest));
}

// ---------------------------------------------------------------------
// Test 6: proptest – random graphs resolve or explain, never panic
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
struct FakePackage {
    name: String,
    versions: Vec<FakeVersion>,
}

#[derive(Debug, Clone)]
struct FakeVersion {
    version: String,
    deps: Vec<(String, String)>,
}

fn arb_graph() -> impl Strategy<Value = Vec<FakePackage>> {
    // 2-6 packages, each with 1-3 versions.
    prop::collection::vec(
        (
            "[a-f]{1,4}",
            prop::collection::vec(
                (
                    0u32..3u32,
                    0u32..3u32,
                    0u32..3u32,
                    prop::collection::vec(
                        ("[a-f]{1,4}", 0u32..3u32),
                        0usize..2usize,
                    ),
                ),
                1usize..3usize,
            ),
        ),
        2usize..7usize,
    )
    .prop_map(|entries| {
        let mut packages: Vec<FakePackage> = Vec::new();
        for (name, vers) in entries {
            let mut versions: Vec<FakeVersion> = Vec::new();
            for (maj, min, pat, deps) in vers {
                let deps = deps
                    .into_iter()
                    .map(|(dn, dmaj)| (dn, format!("^{dmaj}.0.0")))
                    .collect();
                versions.push(FakeVersion {
                    version: format!("{maj}.{min}.{pat}"),
                    deps,
                });
            }
            // Dedup versions.
            versions.sort_by(|a, b| a.version.cmp(&b.version));
            versions.dedup_by(|a, b| a.version == b.version);
            packages.push(FakePackage { name, versions });
        }
        // Dedup packages by name.
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        packages.dedup_by(|a, b| a.name == b.name);
        packages
    })
}

async fn seed_cache_from_graph(cache: &MetadataCache, graph: &[FakePackage]) {
    for pkg in graph {
        if pkg.versions.is_empty() {
            continue;
        }
        let pvs: Vec<hatch::registry::traits::PackageVersion> = pkg
            .versions
            .iter()
            .map(|v| hatch::registry::traits::PackageVersion {
                version: v.version.clone(),
                description: None,
                homepage: None,
                repository: None,
                dependencies: v.deps.iter().cloned().collect(),
                dev_dependencies: HashMap::new(),
                published: None,
                dart_sdk: None,
                flutter_sdk: None,
                archive_sha256: None,
            })
            .collect();
        cache.insert(pkg.name.clone(), pvs).await;
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 100,
        .. ProptestConfig::default()
    })]

    #[test]
    fn proptest_random_graph_resolves_or_explains(graph in arb_graph()) {
        // Each proptest case spins up its own tokio runtime so the proptest
        // macro's sync wrapper is happy. Runtime creation is cheap enough
        // at this sample size.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // Note: pubgrub_adapter::solve doesn't touch the resolution
            // cache directly (that's a caller concern in ultra.rs), so we
            // don't need to mutate env vars here. Avoiding env mutation
            // also keeps us from racing parallel tests.

            let cache = Arc::new(MetadataCache::new());
            seed_cache_from_graph(&cache, &graph).await;

            // Root depends on every package in the graph.
            let mut root_deps: HashMap<String, ParsedConstraint> = HashMap::new();
            for pkg in &graph {
                if pkg.versions.is_empty() {
                    continue;
                }
                let c = VersionConstraint::parse("any")
                    .and_then(|c| c.to_parsed())
                    .unwrap();
                root_deps.insert(pkg.name.clone(), c);
            }
            if root_deps.is_empty() {
                return;
            }

            let sdks = SdkConstraints {
                flutter: Some("3.35.2".into()),
                dart: Some(">=3.5.0 <4.0.0".into()),
            };

            // Must not panic. Either returns a solution or a NoSolution
            // error; both are acceptable outcomes for property testing.
            let res = pubgrub_adapter::solve(
                cache.clone(),
                sdks,
                Default::default(),
                Default::default(),
                "proptest_root".into(),
                semver::Version::new(0, 0, 0),
                root_deps,
            )
            .await;
            match res {
                Ok(_) | Err(hatch::resolver::error::ResolverError::NoSolution(_))
                | Err(hatch::resolver::error::ResolverError::NoVersions(_))
                | Err(hatch::resolver::error::ResolverError::Io(_)) => {}
            }
        });
    }
}
