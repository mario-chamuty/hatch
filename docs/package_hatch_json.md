# Per-package `.hatch.json`

Pub packages can ship an optional `.hatch.json` at their root to steer Hatch's
debloat extractor. It is parsed on extraction and used to add to (not replace)
the built-in keep/strip rules.

## Schema

```json
{
  "hatch_package_version": 1,
  "keep":  ["lib/generated/**", "assets/icons/**"],
  "strip": ["example/**", "tool/scratch/**"]
}
```

- `hatch_package_version` (int) – schema version. Current value is `1`.
- `keep` (array of glob patterns) – additional paths to keep on top of the
  built-in keep set.
- `strip` (array of glob patterns) – additional paths to drop on top of the
  built-in strip set.

Patterns use the [`glob::Pattern`](https://docs.rs/glob) syntax (`*`, `**`,
`?`, `[abc]`). Invalid patterns are logged and ignored – they never panic the
extractor.

## Precedence

The decision for each archive entry is evaluated in this order, highest to
lowest:

1. **Built-in keep (always wins).** Manifests (`pubspec.yaml`,
   `pubspec.lock`, `hatch.json`, `hatch.yaml`, `.hatch.json`), root-level
   `README*` / `LICENSE*` / `LICENCE*` / `CHANGELOG*`, and everything under
   `lib/`, `bin/`, `tool/`. These cannot be stripped by any rule, including
   the package's own `.hatch.json`. This is defence in depth: a malicious or
   buggy package cannot nuke its own API surface.
2. **`hatch_keep` globs** from `.hatch.json`.
3. **Conditional keep from `pubspec.yaml`.** Entries under `flutter.assets`
   (file or directory prefix), `flutter.fonts[*].fonts[*].asset`, and any
   platform directory the package declares in
   `flutter.plugin.platforms.<android|ios|linux|macos|windows|web>`.
4. **Built-in strip.** `example/**`, `examples/**`, `test/**`, `tests/**`,
   `doc/**`, `docs/**`, `.git/**`, `.github/**`, `.idea/**`, `.vscode/**`,
   `screenshots/**`, `.DS_Store`, `Thumbs.db`, `*.psd`, and any root-level
   dotfile other than the handful of allow-listed manifests.
5. **`hatch_strip` globs** from `.hatch.json`.
6. **Default: keep.**

## Disabling the filter

Set the environment variable `HATCH_DEBLOAT=0` (or `false`) to disable the
filter entirely for a single invocation – Hatch will extract every entry the
underlying security checks accept. Use this only to diagnose a bug that you
suspect was caused by an over-aggressive strip.

## Example

A package that ships a large `third_party/` tree but wants Hatch to keep its
`lib/generated/**` code drops this at its root:

```json
{
  "hatch_package_version": 1,
  "keep":  ["lib/generated/**"],
  "strip": ["third_party/**"]
}
```

On a 101-file synthetic package with 50 files under `lib/` and 50 under
`example/` and `test/`, the default built-in rules alone deliver a 49.5 %
file-count reduction.
