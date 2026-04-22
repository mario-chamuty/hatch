# Resolver baseline – `main_before_rewrite`

Captured by Stream E baseline phase (pre Stream C/D resolver rewrite).
Saved baseline name: `main_before_rewrite`.

All numbers come from `cargo bench --bench resolver -- --save-baseline main_before_rewrite`
run against the current resolver on Windows 11, release profile (`lto = true`,
`codegen-units = 1`). Raw criterion output lives in `benches/BASELINE.txt`;
the per-bench JSON estimates are at
`target/criterion/<group>/<bench>/main_before_rewrite/estimates.json`.

## Results

| Bench | Mean | Std dev | Notes |
| --- | --- | --- | --- |
| `resolve_basic/warm` | 1.20 ms | 54.0 us | 3 direct deps (`examples/basic/test_project`), metadata cache warm on disk, fresh `UltraResolver` per iteration. |
| `resolve_large/warm` | 158 ms | 3.31 ms | 46 direct deps (`examples/large_app`), metadata cache warm on disk, fresh `UltraResolver` per iteration. |
| `resolve_warm/large_doubled` | 157 ms | 1.19 ms | Same fresh-resolver pattern but calls `.resolve()` twice per iteration. Near-identical to single-call warm because `self.resolved` short-circuits every package on the second pass – the overhead dominates and a per-manifest resolution cache would collapse this into ~0. |
| `parse_constraint/parse_and_satisfy` | 1.39 us | 16.9 ns | Parses 7 constraint shapes (`^`, `~`, range, `any`, exact, etc.) and evaluates each against 5 version probes (35 `satisfies` calls per iteration, i.e. ~40 ns per call). |
| `resolve_cold/large_network` | **skipped** | – | Gated by `HATCH_BENCH_NETWORK=1`. Wipes `HATCH_CACHE_DIR/metadata` per iteration and re-fetches from pub.dev. Skipped by default so the suite does not hammer the public registry on every `cargo bench` run. |
| `extract_tarball/http_tar_gz` | **skipped** | – | No `http-*.tar.gz` present in either `~/.hatch/cache/downloads` or the bench scratch cache. Run `hatch install` against a project that depends on `http` to seed a tarball, then re-run the bench. |

### Speedup targets this baseline enables

* **Cold resolve** (Stream D): >=2x speedup over `resolve_cold/large_network`.
  Baseline not yet captured – run on a network-enabled machine with
  `HATCH_BENCH_NETWORK=1` before comparing.
* **Warm resolve** (Stream D manifest-hash cache): >=100x speedup over
  `resolve_large/warm` (~158 ms) -> target ~1.6 ms or better for the
  second call on an unchanged manifest.
* **Constraint satisfies** (Stream C rewrite of `VersionConstraint`):
  current 40 ns per `satisfies` call is the floor to beat once
  `semver::Version::parse` is no longer re-invoked on every call.

## Methodology notes

* All benches construct a fresh `UltraResolver` each iteration via
  `UltraResolver::with_manifest(&manifest).await`. This is the only stable
  public constructor on the current resolver.
* `HATCH_CACHE_DIR` is redirected to `target/tmp/hatch_bench/cache_shared`
  before any bench runs so we never touch the user's real cache during
  measurement. `CachePaths::root` caches its value in a `OnceCell`, so this
  redirect only takes effect if the bench sets it before the first resolver
  call; the bench helpers do exactly that.
* Warm benches prime the bench cache directory from
  `~/.hatch/cache/metadata` via a best-effort copy before the measurement
  loop starts. A missing source cache makes the first iteration do a
  network round-trip, which inflates the mean; re-running once on a
  primed machine produces the stable numbers reported here.
* The `examples/large_app/hatch.json` manifest pins `"flutter": "stable"`,
  which the current `ManifestValidator` rejects. The bench helper rewrites
  this to `3.35.2` in-memory before parsing (no change on disk).

## API gaps that will affect Streams C / D / later-E

These are places where the current resolver API made the bench awkward
or impossible to write as specified. Later streams should address them:

1. **No in-memory cache clear API.** `MetadataCache` exposes `new`,
   `load_from_disk`, `insert`, `get`, `contains` and `save_to_disk`, but no
   `clear` or `drop_entry`. `bench_resolve_cold` therefore works around
   this by wiping the on-disk cache directory and constructing a fresh
   `MetadataCache` per iteration. Stream D should add a `clear()` method
   so cold-path benches can test a pure "fresh in-memory cache" without
   deleting disk state.
2. **No manifest-hash resolution cache.** The current resolver always
   re-walks the entire dependency graph – `resolve_warm/large_doubled`
   demonstrates this: calling `.resolve()` a second time on an identical
   manifest takes the same ~158 ms as the first. Stream D's plan to add
   a manifest-hash-keyed result cache is where the >=100x warm speedup
   is expected to come from.
3. **`VersionConstraint::satisfies` re-parses on every call.** Every call
   to `satisfies` runs `semver::Version::parse(constraint_string)` and
   `Version::parse(version_string)`. The 40 ns per-call figure is
   dominated by this. Stream C's rewrite should intern parsed versions.
4. **No `clear_cache` on `UltraResolver`.** The resolver owns `resolved`,
   `resolved_paths`, and `resolved_batch` `HashMap`s. There is no way to
   reset these short of dropping the resolver, which is fine for the
   current benches but would force any later "reuse resolver across
   manifests" bench to hand-reconstruct.
5. **No public "pure resolution" hook.** `UltraResolver::resolve` also
   handles SDK deps, git deps, and side effects like printing progress
   via `println!` and `eprintln!`. The bench measures the whole thing,
   so the baseline numbers include the cost of those `println!` calls.
   Stream D's extraction of a pure-resolution core would let later
   benches subtract that overhead.
6. **`CachePaths::root` is a one-shot `OnceCell`.** Once pinned, the
   cache path is fixed for the process lifetime. Cold benches would
   like to swap cache dirs between iterations but can't. Workaround is
   to wipe the contents of the shared dir instead.
7. **No hatch-lib test harness helper for "seed cache with N packages"**.
   Stream C/D benches would benefit from a helper like
   `hatch::test_support::seed_cache(&[("http", "1.1.0"), ...])` so each
   bench starts from a deterministic cache state without reaching into
   the user's home directory.

## Reproducing

```pwsh
# With HATCH binary already built in release:
cargo bench --bench resolver -- --save-baseline main_before_rewrite

# Network-dependent cold bench (do not run in CI without consent – hits pub.dev):
$env:HATCH_BENCH_NETWORK = "1"; cargo bench --bench resolver -- resolve_cold
```

After Streams C / D land, compare with:

```pwsh
cargo bench --bench resolver -- --baseline main_before_rewrite
```
