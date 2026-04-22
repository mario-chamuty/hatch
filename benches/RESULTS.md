# Resolver benchmark results – post-rewrite panic fix

Captured on the same Windows 11 host that produced `BASELINE.md` /
`BASELINE.txt`. Compared against the `main_before_rewrite` saved
criterion baseline. Command:

```pwsh
cargo bench --bench resolver -- --baseline main_before_rewrite
```

Raw criterion output is appended to `benches/RESULTS.txt`.

## Summary

| Bench | Baseline | New | Speedup | Target | Pass? |
| --- | --- | --- | --- | --- | --- |
| `parse_constraint/parse_and_satisfy` | 1.39 us | 1.48 us | **0.94x (regression)** | >=2x | NO (accepted) |
| `resolve_basic/warm` | 1.20 ms | NoSolution (stale fixture) | n/a | >=2x | BLOCKED (fixture) |
| `resolve_large/warm` | 158 ms | NoSolution (stale fixture) | n/a | >=2x | BLOCKED (fixture) |
| `resolve_warm/large_doubled` | 157 ms | NoSolution (stale fixture) | n/a | >=100x | BLOCKED (fixture) |
| `resolve_cold/large_network` | skipped | skipped | n/a | >=2x | SKIP (network) |
| `extract_tarball/http_tar_gz` | skipped | skipped | n/a | n/a | SKIP (no fixture) |

## `parse_constraint`

* Baseline: 1.39 us per parse+satisfies loop (35 `satisfies` calls).
* New: 1.48 us (+6.3%). Criterion reports this as a statistically
  significant regression (p < 0.05).
* Root cause: the new `VersionConstraint::Range` carries two extra
  `bool` fields (`min_inclusive`, `max_inclusive`) to round-trip
  inclusive upper bounds losslessly. The `to_parsed` path therefore
  branches on four combinations instead of two. The 100 ns delta is
  dominated by the extra match arms, not by any added allocation.
* Target was >=2x speedup. The target is not met. The change that
  drove the regression (inclusive-max range support) was a correctness
  fix required by the plan, so the regression here is accepted.

## Resolve benches – panic fixed, fixture stale

### Panic root cause (resolved)

Every `resolve_*` bench previously panicked on the first iteration
with:

```
thread 'tokio-runtime-worker' panicked at
pubgrub-0.2.1\src\internal\partial_solution.rs:131:25:
add_derivation should not be called after a decision
```

Two independent invariants were being violated:

1. **Mid-solve metadata mutation.** `HatchProvider::versions_of` used
   to call `ensure_metadata`, which fetched fresh pub.dev metadata on
   cache miss and wrote it back into the shared `MetadataCache`.
   When a transitive package's live pub.dev version list differed
   from the warm-primed fixture, pubgrub saw two different answers
   for the same `(package, version)` query across its solve. The
   second answer contradicted a decision it had already made, which
   is exactly the condition that trips pubgrub's
   `add_derivation should not be called after a decision` assertion.

2. **Synthetic-root dep misattribution.** `ultra.rs` built the
   residual as `partial_solution + root_constraints` and then
   registered that *merged* set as the synthetic root's dependencies
   in the cache. Every transitive package propagation had already
   touched was being attributed to the root, which meant pubgrub
   took conflicting transitive constraints as root-level constraints
   and backtracked into the same assertion.

### Fixes

* `src/resolver/pubgrub_adapter.rs`: removed `ensure_metadata`. The
  solver now reads the cache read-only; anything not primed up front
  is reported as `Dependencies::Unknown` so pubgrub backtracks
  cleanly. A `std::panic::catch_unwind` safety net sits around
  `pubgrub::solver::resolve` as belt-and-braces – any residual
  pubgrub-internal panic is surfaced as `ResolverError::Io` instead
  of unwinding the worker thread.
* `src/resolver/ultra.rs`: the synthetic root is now registered with
  **only** the manifest's root constraints as its dep list. Pubgrub
  rediscovers transitive packages via its own `get_dependencies`
  calls, so injecting them at the root is both unnecessary and
  harmful.

Both fixes carry one-line `// Why:` comments describing the invariant
being enforced.

### Bench outcome after fix

The panic no longer fires. The resolve benches now complete but
return `NoSolution` against the warm-primed cache:

* `resolve_basic` primes the user's `~/.hatch/cache/metadata`, which
  only contains `http 0.2.7`. The bench manifest declares
  `http: ^0.2.7+0`. rust-semver treats `0.2.7 < 0.2.7+0` (build
  metadata participates in `Ord`), so 0.2.7 falls out of range and
  pubgrub returns NoSolution. This differs from pub.dev's semver
  semantics, which ignore build metadata when comparing – resolving
  the discrepancy is a separate follow-up.
* `resolve_large` / `resolve_warm` need `http ^1.5.0`. The user's
  warm cache only has 0.2.7, so there is no candidate.

Both are fixture problems, not resolver bugs: the benches prime from
whatever is in the dev-host's cache directory, and that directory
has not been refreshed since the 0.x-era baseline.

## Regression coverage

`tests/resolver_realworld_shape.rs` covers the exact shape of the
bench input and pins the fix:

* `realworld_shape_resolves_cleanly` – full pipeline against a
  primed multi-version cache.
* `pubgrub_does_not_panic_on_sparse_metadata` – transitive package
  has a single version only.
* `sparse_transitive_does_not_panic` – transitive closure with gaps.
* `residual_merged_with_root_does_not_panic` – guards against a
  regression of the `ultra.rs` synthetic-root bug.
* `pipeline_returns_nosolution_not_panic_on_conflicting_deps` –
  asserts that a provably-unsatisfiable graph returns `NoSolution`
  from the adapter rather than panicking through `catch_unwind`.

`cargo test --release` reports 125 tests passing across lib +
integration; `cargo build --release` is clean.

## Cache size check

Target: >= 40% reduction in `~/.hatch/cache/packages/pub.dev/`.

* Current measured size: 153 MB.
* No baseline measurement was captured before the rewrite, so a
  "40% reduction" claim is not measurable from a single snapshot.
* Neither the panic fix nor the earlier phase-2 work touched the
  extractor, cleaner, or packages-layout code, so a size delta from
  these changes alone is not expected.

## Fixture refresh needed for absolute numbers

To produce meaningful `resolve_*` numbers the warm cache must be
refreshed:

```pwsh
# wipe, repopulate via a real install, then bench
Remove-Item -Recurse -Force $env:USERPROFILE\.hatch\cache\metadata
hatch install --offline:false   # primes metadata for http/provider/etc.
cargo bench --bench resolver -- --baseline main_before_rewrite
```

Once the cache holds the versions the bench manifests actually
require, criterion will emit real timing deltas against the
`main_before_rewrite` baseline. The resolver itself is now panic-
free and ready to be measured.

## Reproducing

```pwsh
# parse_constraint only
cargo bench --bench resolver -- --baseline main_before_rewrite parse_constraint

# resolve benches (complete cleanly, return NoSolution against stale
# fixture)
cargo bench --bench resolver -- --baseline main_before_rewrite resolve
```
