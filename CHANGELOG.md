# Changelog

## 0.1.1

### Fixed

- Default plugin registry URL corrected from `tryhatch.com` (a parked domain)
  to `tryhatch.dev`, so `hatch plugin install`/`search` work out of the box.
  Override with the `HATCH_REGISTRY` environment variable.

## Unreleased

### Security

- **Checksum verification is now required for every package download.** The
  registry must supply an `archive_sha256` or the install aborts. Pass
  `--allow-unchecksummed` (per-invocation only, never a config setting) to
  bypass; every bypass is recorded in `~/.hatch/audit.log`.
- **Trust-On-First-Use (TOFU) for dependency scripts.** The first time a
  dependency-declared script executes, Hatch prompts interactively. The
  decision is persisted in `~/.hatch/trust.json`. Non-interactive
  environments default to DENY; set `HATCH_TRUST_SCRIPTS=all` to bypass (the
  bypass is audit-logged). Scripts declared in the root project's own
  manifest bypass TOFU – those are authored by the user.
- **Synchronous tarball cleanup.** The fire-and-forget `tokio::spawn` that
  previously cleaned up downloaded tarballs has been replaced with a
  synchronous call on the success path. On extraction failure, both the
  partial package directory and the tarball are removed.
- **Metadata cache pruning.** On load, once per 24 hours, per-package
  metadata files older than 7 days are dropped and surviving files are
  trimmed to versions referenced by local lockfiles plus anything published
  in the last 30 days (fallback: keep latest 20 when no lockfile is
  present). `hatch cache prune --aggressive` runs a stricter pass that
  keeps only lockfile-referenced versions and merges per-package
  `.hatch_metadata.json` sidecars into a central
  `~/.hatch/cache/index.json`.

### Features

- **Mac-free iOS builds.** `hatch ios build` compiles, links, signs (via
  `rcodesign`), and uploads an iOS app to App Store Connect / TestFlight with no
  macOS, Xcode, or `actool` involved. The fully native Rust pipeline is the
  default on Linux; a Windows-native path (no WSL) is in progress.
- **Scripts may now be declared as an argv array** (e.g.
  `["flutter", "build", "apk"]`), which bypasses the shell entirely.
  Recommended for new packages; the string form is still supported and
  continues to execute via `sh -c` + positional arguments. User CLI args
  append as additional argv tokens in both forms.
- `hatch cache prune --aggressive` re-applies Flutter-aware debloat
  rules to already-extracted packages (once Stream A's debloat module is
  merged), prunes metadata aggressively, and merges the per-package
  metadata sidecars into a central index.
- **Resolver: independent-subgraph parallelism.** After interval
  propagation, the residual dependency graph is partitioned into
  connected components via `petgraph` BFS and each component is solved
  by pubgrub on its own rayon thread. Set `HATCH_NO_SUBGRAPH_PARALLEL=1`
  to roll back to the single-solver path.
- **Archive + metadata base URL override.** Both the registry and the
  tarball downloader honour `HATCH_PUB_HOSTED_URL` (preferred) and
  `PUB_HOSTED_URL` (for pub-compat). When set, every HTTP request Hatch
  makes during install is redirected to that base. Useful for mirrors,
  wiremock-backed E2E tests, and air-gapped relay setups.
- **`VersionConstraint::Range` round-trips inclusive upper bounds.**
  Previously `>=x <=y` collapsed to `>=x <y+1`; the parser now carries
  `min_inclusive`/`max_inclusive` flags so `satisfies` and `to_parsed`
  preserve the original semantics.
