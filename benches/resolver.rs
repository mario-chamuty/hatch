//! Stream E baseline benchmark suite.
//!
//! Captures baseline numbers for the CURRENT resolver (pre-Stream-C/D rewrite)
//! so later work can prove >=2x cold-resolve and >=100x warm-resolve speedups.
//!
//! The suite drives the resolver through its direct API (`UltraResolver`,
//! `VersionConstraint`, `PackageExtractor`) via the `hatch` lib target that
//! exists as `src/lib.rs`. Benches are sized with small sample counts
//! (criterion's floor of 10) because each iteration performs real resolver
//! work and the goal here is a stable BASELINE number, not a competitive
//! microbench.
//!
//! ### Variants
//!
//! * `resolve_basic` – warm resolve of the ~3-dep `examples/basic/test_project`
//!   manifest. Metadata cache is primed on-disk from the user's real
//!   `HATCH_CACHE_DIR`; a fresh `UltraResolver` is constructed every iteration.
//! * `resolve_large` – same but for the ~46-dep `examples/large_app` manifest.
//! * `resolve_warm` – runs `resolve` twice per iteration so the second call
//!   sees a populated in-memory resolver state. This is the closest we can
//!   get to measuring what a manifest-hash-keyed resolution cache (Stream D)
//!   would replace.
//! * `resolve_cold` – wipes `HATCH_CACHE_DIR` before every sample. Gated
//!   behind `HATCH_BENCH_NETWORK=1` because it hits pub.dev.
//! * `parse_constraint` – `VersionConstraint::parse` + `satisfies` over a
//!   representative set of constraint shapes.
//! * `extract_tarball` – `PackageExtractor::extract_tar_gz` against a cached
//!   `http-1.1.0.tar.gz` if one is present in the user's cache; skipped
//!   otherwise (no downloads during benching).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

use hatch::cache::extractor::PackageExtractor;
use hatch::cache::metadata_cache::MetadataCache;
use hatch::cache::paths::CachePaths;
use hatch::manifest::parser::ManifestParser;
use hatch::manifest::schema::HatchManifest;
use hatch::registry::traits::VersionConstraint;
use hatch::resolver::ultra::UltraResolver;

// =====================================================================
// Fixture helpers
// =====================================================================

/// Fixture tree for bench-scoped caches. Kept under `target/` so it is
/// wiped by `cargo clean` and never pollutes the user's home directory.
fn bench_fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("tmp")
        .join("hatch_bench")
}

fn basic_manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("basic")
        .join("test_project")
        .join("hatch.json")
}

fn large_manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("large_app")
        .join("hatch.json")
}

/// Read the manifest at `path` and rewrite the placeholder
/// `"flutter": "stable"` SDK pin to a concrete version so the validator
/// doesn't reject it. The bench doesn't actually talk to FVM, but the
/// resolver's upstream call path runs `ManifestValidator::validate`.
fn load_manifest(path: &Path) -> HatchManifest {
    let raw = fs::read_to_string(path).expect("read manifest");
    let patched = raw.replace("\"flutter\": \"stable\"", "\"flutter\": \"3.35.2\"");
    ManifestParser::parse_json(&patched).expect("parse manifest")
}

/// Lay out a dedicated HATCH_CACHE_DIR for a bench variant and return it.
/// Also sets the `HATCH_CACHE_DIR` environment variable so `CachePaths`
/// picks it up. This must be called before any code that touches the
/// cache path `OnceCell`, which is why every bench function calls it
/// eagerly at entry.
///
/// NOTE: `CachePaths::root` caches the first value it sees in a `OnceCell`.
/// That means the FIRST bench to call this pins the cache dir for the
/// entire process, and subsequent calls are silently ignored. We work
/// around this by:
/// 1. Sharing a single cache dir across all in-process benches, seeded
///    from the user's real cache for warm benches, and
/// 2. Leaving cold/network-dependent benches in their own process-level
///    environment (set before criterion starts) or skipping them.
fn bench_cache_dir() -> PathBuf {
    let dir = bench_fixture_root().join("cache_shared");
    fs::create_dir_all(&dir).expect("create bench cache dir");

    // Only set on first call – after that the OnceCell has already resolved.
    env::set_var("HATCH_CACHE_DIR", &dir);
    dir
}

/// Copy the user's real metadata cache (if any) into the bench cache dir
/// so warm benches don't immediately miss every lookup. Best-effort; a
/// missing source cache simply means the first bench iteration does a
/// network round-trip.
fn warm_prime_metadata_cache(dst_cache: &Path) {
    // Probe the real cache *before* overriding HATCH_CACHE_DIR – otherwise
    // CachePaths::root will already be pinned to our bench dir.
    let user_home = dirs::home_dir();
    let user_metadata = match user_home {
        Some(h) => h.join(".hatch").join("cache").join("metadata"),
        None => return,
    };
    if !user_metadata.exists() {
        return;
    }
    let dst_metadata = dst_cache.join("metadata");
    if let Err(e) = fs::create_dir_all(&dst_metadata) {
        eprintln!("warm prime: create dst failed: {e}");
        return;
    }

    // Copy every .json file from the user's metadata dir. Shallow copy only;
    // the current layout is flat but we walk subdirs defensively in case a
    // registry-scoped subdir shows up later.
    fn copy_json(src: &Path, dst: &Path) -> std::io::Result<()> {
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            if ft.is_dir() {
                fs::create_dir_all(&to)?;
                copy_json(&from, &to)?;
            } else if ft.is_file() {
                if from.extension().and_then(|s| s.to_str()) == Some("json") {
                    fs::copy(&from, &to)?;
                }
            }
        }
        Ok(())
    }
    let _ = copy_json(&user_metadata, &dst_metadata);
}

fn tokio_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

// =====================================================================
// Resolve benches
// =====================================================================

/// Warm resolution of the small manifest. "Warm" = metadata cache is
/// primed on-disk before the bench starts; a fresh `UltraResolver` is
/// constructed every iteration.
fn bench_resolve_basic(c: &mut Criterion) {
    let manifest_path = basic_manifest_path();
    if !manifest_path.exists() {
        eprintln!("SKIP bench_resolve_basic: {} not found", manifest_path.display());
        return;
    }
    let cache_dir = bench_cache_dir();
    warm_prime_metadata_cache(&cache_dir);

    let manifest = load_manifest(&manifest_path);
    let rt = tokio_runtime();

    // Prime the on-disk cache at least once by running a full resolve.
    // Subsequent iterations then hit warm metadata.
    rt.block_on(async {
        let mut r = UltraResolver::with_manifest(&manifest).await;
        let _ = r.resolve(&manifest).await;
    });

    let mut group = c.benchmark_group("resolve_basic");
    group.sample_size(10);
    group.bench_function("warm", |b| {
        b.to_async(&rt).iter(|| async {
            let mut r = UltraResolver::with_manifest(&manifest).await;
            r.resolve(&manifest).await.expect("resolve basic")
        });
    });
    group.finish();
}

/// Warm resolution of the ~46-direct-dep manifest.
fn bench_resolve_large(c: &mut Criterion) {
    let manifest_path = large_manifest_path();
    if !manifest_path.exists() {
        eprintln!("SKIP bench_resolve_large: {} not found", manifest_path.display());
        return;
    }
    let cache_dir = bench_cache_dir();
    warm_prime_metadata_cache(&cache_dir);

    let manifest = load_manifest(&manifest_path);
    let rt = tokio_runtime();

    // Prime.
    rt.block_on(async {
        let mut r = UltraResolver::with_manifest(&manifest).await;
        let _ = r.resolve(&manifest).await;
    });

    let mut group = c.benchmark_group("resolve_large");
    group.sample_size(10);
    group.bench_function("warm", |b| {
        b.to_async(&rt).iter(|| async {
            let mut r = UltraResolver::with_manifest(&manifest).await;
            r.resolve(&manifest).await.expect("resolve large")
        });
    });
    group.finish();
}

/// Doubled warm: same resolver instance, `.resolve()` twice. The second
/// call finds every package in `self.resolved` already and should be the
/// fastest path the current resolver can do.
fn bench_resolve_warm(c: &mut Criterion) {
    let manifest_path = large_manifest_path();
    if !manifest_path.exists() {
        eprintln!("SKIP bench_resolve_warm: {} not found", manifest_path.display());
        return;
    }
    let cache_dir = bench_cache_dir();
    warm_prime_metadata_cache(&cache_dir);

    let manifest = load_manifest(&manifest_path);
    let rt = tokio_runtime();

    // Prime.
    rt.block_on(async {
        let mut r = UltraResolver::with_manifest(&manifest).await;
        let _ = r.resolve(&manifest).await;
    });

    let mut group = c.benchmark_group("resolve_warm");
    group.sample_size(10);
    group.bench_function("large_doubled", |b| {
        b.to_async(&rt).iter(|| async {
            let mut r = UltraResolver::with_manifest(&manifest).await;
            r.resolve(&manifest).await.expect("first resolve");
            r.resolve(&manifest).await.expect("second resolve")
        });
    });
    group.finish();
}

/// Cold resolution: force the metadata cache empty before every iteration.
/// Gated by `HATCH_BENCH_NETWORK=1` because this hits pub.dev.
fn bench_resolve_cold(c: &mut Criterion) {
    if env::var("HATCH_BENCH_NETWORK").ok().as_deref() != Some("1") {
        eprintln!(
            "SKIP bench_resolve_cold: set HATCH_BENCH_NETWORK=1 to run network-dependent benches"
        );
        return;
    }
    let manifest_path = large_manifest_path();
    if !manifest_path.exists() {
        eprintln!("SKIP bench_resolve_cold: {} not found", manifest_path.display());
        return;
    }
    let cache_dir = bench_cache_dir();

    let manifest = load_manifest(&manifest_path);
    let rt = tokio_runtime();

    let mut group = c.benchmark_group("resolve_cold");
    group.sample_size(10);
    group.sampling_mode(criterion::SamplingMode::Flat);
    group.bench_function("large_network", |b| {
        b.to_async(&rt).iter_batched(
            || {
                // Setup (sync): wipe the on-disk metadata directory so the
                // next resolver sees zero cached packages.
                let md = cache_dir.join("metadata");
                let _ = fs::remove_dir_all(&md);
                let _ = fs::create_dir_all(&md);
            },
            |_| async {
                let mut r = UltraResolver::with_manifest(&manifest).await;
                r.resolve(&manifest).await.expect("cold resolve")
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

// =====================================================================
// Micro-benches: constraint parse+satisfy
// =====================================================================

fn bench_parse_constraint(c: &mut Criterion) {
    // Representative constraint shapes pulled from the large_app manifest
    // and a couple of Dart-ecosystem oddities (range, exact, any).
    let constraints: &[&str] = &[
        "^1.2.3",
        "~1.0.0",
        ">=1.0.0 <2.0.0",
        "any",
        "1.5.0",
        ">=2.17.0 <4.0.0",
        "^0.20.2",
    ];
    // Versions we'll test each constraint against. Chosen to exercise both
    // the satisfying and non-satisfying branches of every constraint shape.
    let probes: &[&str] = &["1.0.0", "1.5.0", "2.0.0", "0.20.3", "3.1.0"];

    let mut group = c.benchmark_group("parse_constraint");
    group.sample_size(50);
    group.bench_function("parse_and_satisfy", |b| {
        b.iter(|| {
            let mut hits = 0u64;
            for cs in constraints {
                // `parse` is the cheap step; `satisfies` is the one Stream C
                // will rewrite. Bundle them so the numbers track realistic
                // resolver hot-path usage.
                let parsed = VersionConstraint::parse(cs).expect("parse constraint");
                for v in probes {
                    if parsed.satisfies(v) {
                        hits += 1;
                    }
                }
            }
            hits
        });
    });
    group.finish();
}

// =====================================================================
// Tarball extract micro-bench
// =====================================================================

/// Benchmark `PackageExtractor::extract_tar_gz` against a cached http-*.tar.gz.
/// Searches the user's real downloads dir plus the bench cache dir for
/// any tarball matching `http-*.tar.gz`. If none found, skips.
fn bench_extract_tarball(c: &mut Criterion) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        let downloads = home.join(".hatch").join("cache").join("downloads");
        if let Ok(entries) = fs::read_dir(&downloads) {
            for entry in entries.flatten() {
                let p = entry.path();
                let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if fname.starts_with("http-") && fname.ends_with(".tar.gz") {
                    candidates.push(p);
                }
            }
        }
    }
    let bench_downloads = bench_fixture_root().join("cache_shared").join("downloads");
    if let Ok(entries) = fs::read_dir(&bench_downloads) {
        for entry in entries.flatten() {
            let p = entry.path();
            let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if fname.starts_with("http-") && fname.ends_with(".tar.gz") {
                candidates.push(p);
            }
        }
    }

    if candidates.is_empty() {
        eprintln!(
            "SKIP bench_extract_tarball: no http-*.tar.gz found in ~/.hatch/cache/downloads or bench cache. \
             Run `hatch install` against a project that uses http to seed it."
        );
        return;
    }
    let archive = candidates.remove(0);
    let scratch = bench_fixture_root().join("extract_scratch");

    let mut group = c.benchmark_group("extract_tarball");
    group.sample_size(10);
    group.bench_function("http_tar_gz", |b| {
        b.iter_batched(
            || {
                let _ = fs::remove_dir_all(&scratch);
                fs::create_dir_all(&scratch).expect("create scratch");
                scratch.clone()
            },
            |dest| {
                PackageExtractor::extract_tar_gz(&archive, &dest).expect("extract tarball");
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

// =====================================================================
// Silence warnings about unused imports when benches skip
// =====================================================================
#[allow(dead_code)]
fn _keep_imports_live() {
    let _: Option<Arc<MetadataCache>> = None;
    let _ = CachePaths::root();
}

criterion_group!(
    benches,
    bench_resolve_basic,
    bench_resolve_large,
    bench_resolve_warm,
    bench_resolve_cold,
    bench_parse_constraint,
    bench_extract_tarball,
);
criterion_main!(benches);
