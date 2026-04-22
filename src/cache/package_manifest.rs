//! Flutter-aware debloat allowlist, layered on top of the archive extractor.
//!
//! This module implements a two-pass extraction pipeline:
//!
//! 1. Pass 1 reads the archive once, buffers a small whitelist of manifest-like
//!    files into memory (respecting all of the existing extractor security
//!    checks), and parses them.
//! 2. Pass 2 re-opens the archive bytes, and extracts only the entries that
//!    `keep_decision` approves based on pubspec hints and the per-package
//!    `.hatch.json` file.
//!
//! Security properties:
//! - Never bypasses the checks in `extractor.rs` (path validation, special
//!   entry rejection, `MAX_ENTRIES`, `MAX_FILE_SIZE`, `MAX_TOTAL_SIZE`,
//!   `MAX_LONGLINK_BYTES`). Pass 2 shares the same hard caps.
//! - A package cannot strip a built-in-kept file. Built-in keep wins before
//!   `hatch_strip` is considered.
//! - All manifest parsing is best-effort. Malformed pubspec or `.hatch.json`
//!   degrades gracefully to the default keep/strip set.

use anyhow::{anyhow, Result};
use flate2::read::GzDecoder;
use glob::Pattern;
use log::{debug, warn};
use serde::Deserialize;
use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use super::extractor::{
    validate_archive_path, MAX_ENTRIES, MAX_FILE_SIZE, MAX_LONGLINK_BYTES, MAX_TOTAL_SIZE,
};

/// Cap on any single manifest-like file we buffer in memory during Pass 1.
/// Legitimate pubspec/README/LICENSE/CHANGELOG files are never this large; if
/// one is, treat it as an attack surface and reject the archive.
const MAX_MANIFEST_BYTES: u64 = 5 * 1024 * 1024;

/// Statistics from a debloat extraction pass.
#[derive(Debug, Default, Clone, Copy)]
pub struct DebloatStats {
    pub entries_kept: usize,
    pub entries_stripped: usize,
    pub bytes_kept: u64,
    pub bytes_stripped: u64,
}

/// Parsed hints from the package's `pubspec.yaml`.
#[derive(Debug, Default, Clone)]
pub struct PubspecHints {
    /// Entries from `flutter.assets`. Each entry may be a file path or a
    /// directory (ends with `/`).
    pub assets: Vec<String>,
    /// Entries from `flutter.fonts[*].fonts[*].asset`.
    pub font_assets: Vec<String>,
    /// Declared platform directories under `flutter.plugin.platforms.*`.
    /// Only platforms that the package actually declares appear here.
    pub platform_dirs: HashSet<String>,
}

/// Parsed per-package `.hatch.json` override file.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct HatchPackageOverrides {
    #[serde(default)]
    pub hatch_package_version: u32,
    #[serde(default)]
    pub keep: Vec<String>,
    #[serde(default)]
    pub strip: Vec<String>,
}

/// Parse `flutter.assets`, `flutter.fonts[*].fonts[*].asset`, and
/// `flutter.plugin.platforms.<name>` out of a pubspec.yaml.
pub fn parse_pubspec_hints(pubspec_yaml: &str) -> PubspecHints {
    let mut hints = PubspecHints::default();

    let value: serde_yaml::Value = match serde_yaml::from_str(pubspec_yaml) {
        Ok(v) => v,
        Err(e) => {
            debug!("pubspec.yaml did not parse cleanly, using defaults: {}", e);
            return hints;
        }
    };

    let flutter = match value.get("flutter") {
        Some(f) => f,
        None => return hints,
    };

    if let Some(assets) = flutter.get("assets").and_then(|v| v.as_sequence()) {
        for entry in assets {
            if let Some(s) = entry.as_str() {
                hints.assets.push(normalise_asset(s));
            }
        }
    }

    if let Some(fonts) = flutter.get("fonts").and_then(|v| v.as_sequence()) {
        for family in fonts {
            if let Some(inner) = family.get("fonts").and_then(|v| v.as_sequence()) {
                for font in inner {
                    if let Some(asset) = font.get("asset").and_then(|v| v.as_str()) {
                        hints.font_assets.push(normalise_asset(asset));
                    }
                }
            }
        }
    }

    if let Some(platforms) = flutter
        .get("plugin")
        .and_then(|p| p.get("platforms"))
        .and_then(|p| p.as_mapping())
    {
        for (k, _) in platforms {
            if let Some(name) = k.as_str() {
                // Only whitelist known platform dirs to avoid a malicious
                // pubspec adding `../../` as a "platform".
                match name {
                    "android" | "ios" | "linux" | "macos" | "windows" | "web" => {
                        hints.platform_dirs.insert(name.to_string());
                    }
                    other => {
                        debug!("Ignoring unknown plugin platform: {}", other);
                    }
                }
            }
        }
    }

    hints
}

/// Parse the optional per-package `.hatch.json` override file.
pub fn parse_hatch_overrides(hatch_json: &str) -> HatchPackageOverrides {
    match serde_json::from_str::<HatchPackageOverrides>(hatch_json) {
        Ok(o) => o,
        Err(e) => {
            warn!(".hatch.json did not parse cleanly, ignoring overrides: {}", e);
            HatchPackageOverrides::default()
        }
    }
}

fn normalise_asset(s: &str) -> String {
    // Strip a leading `./`.
    let s = s.strip_prefix("./").unwrap_or(s);
    s.replace('\\', "/")
}

fn normalise_rel(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Test whether a path is "under" an asset entry. The entry may be a file or
/// a directory (trailing slash, or no extension-ish suffix interpreted as a
/// prefix-dir by Flutter).
fn path_matches_asset(rel: &str, entry: &str) -> bool {
    if entry.is_empty() {
        return false;
    }
    if rel == entry {
        return true;
    }
    // Directory entry.
    if entry.ends_with('/') {
        return rel.starts_with(entry);
    }
    // A bare `assets/images` in flutter.assets is treated as a directory prefix
    // by the Flutter tool when the path resolves to a directory.
    let with_slash = format!("{}/", entry);
    rel.starts_with(&with_slash)
}

fn matches_any_glob(rel: &str, patterns: &[Pattern]) -> bool {
    patterns.iter().any(|p| p.matches(rel))
}

fn compile_patterns(raw: &[String]) -> Vec<Pattern> {
    raw.iter()
        .filter_map(|p| match Pattern::new(p) {
            Ok(pat) => Some(pat),
            Err(e) => {
                warn!("Invalid glob in .hatch.json: {:?} ({})", p, e);
                None
            }
        })
        .collect()
}

/// Root-level dotfiles that are NEVER kept even though they're technically at
/// the root. Everything else at the root starting with `.` is stripped except
/// the explicit allowlist below.
fn is_allowed_root_dotfile(rel: &str) -> bool {
    matches!(
        rel,
        ".hatch.json"
            // ".packages" is legacy Dart but harmless to keep when present.
            | ".packages"
    )
}

fn has_root_dotfile_prefix(rel: &str) -> bool {
    // Any file starting with `.` whose path has no slash is a root-level
    // dotfile. Inside subdirs we let the strip rules decide (e.g. `.github`).
    !rel.contains('/') && rel.starts_with('.')
}

/// Core keep/strip decision.
///
/// Precedence (highest first):
/// 1. Built-in keep: manifests, READMEs, licences, changelogs, `lib/`,
///    `bin/`, `tool/`. Overrides `strip` entirely.
/// 2. Per-package `hatch_keep` globs.
/// 3. Conditional keep from pubspec hints: `flutter.assets`, font assets,
///    declared `flutter.plugin.platforms.*` dirs.
/// 4. Built-in strip: `example/**`, `examples/**`, `test/**`, `tests/**`,
///    `doc/**`, `docs/**`, `.git/**`, `.github/**`, `.idea/**`, `.vscode/**`,
///    `screenshots/**`, `.DS_Store`, `Thumbs.db`, `*.psd`, root dotfiles
///    other than the allowlist.
/// 5. Per-package `hatch_strip` globs.
/// 6. Default: keep.
pub fn keep_decision(
    rel_path: &Path,
    pubspec: &PubspecHints,
    hatch_keep: &[Pattern],
    hatch_strip: &[Pattern],
) -> bool {
    let rel = normalise_rel(rel_path);
    let rel = rel.trim_start_matches("./").to_string();

    // --- 1. Built-in keep (always, cannot be stripped) ---
    if is_builtin_keep(&rel) {
        return true;
    }

    // --- 2. Per-package explicit keep ---
    if matches_any_glob(&rel, hatch_keep) {
        return true;
    }

    // --- 3. Conditional keep from pubspec ---
    if pubspec.assets.iter().any(|a| path_matches_asset(&rel, a)) {
        return true;
    }
    if pubspec.font_assets.iter().any(|a| path_matches_asset(&rel, a)) {
        return true;
    }
    for platform in &pubspec.platform_dirs {
        let prefix = format!("{}/", platform);
        if rel == *platform || rel.starts_with(&prefix) {
            return true;
        }
    }

    // --- 4. Built-in strip ---
    if is_builtin_strip(&rel) {
        return false;
    }

    // --- 5. Per-package explicit strip ---
    if matches_any_glob(&rel, hatch_strip) {
        return false;
    }

    // --- 6. Default keep ---
    true
}

fn is_builtin_keep(rel: &str) -> bool {
    // Exact-match manifests at root.
    match rel {
        "pubspec.yaml"
        | "pubspec.lock"
        | "hatch.json"
        | "hatch.yaml"
        | ".hatch.json" => return true,
        _ => {}
    }

    // Root docs: README*, LICENSE*, LICENCE*, CHANGELOG* (no slash -> root).
    if !rel.contains('/') {
        let upper: String = rel.chars().map(|c| c.to_ascii_uppercase()).collect();
        if upper.starts_with("README")
            || upper.starts_with("LICENSE")
            || upper.starts_with("LICENCE")
            || upper.starts_with("CHANGELOG")
        {
            return true;
        }
    }

    // Top-level source dirs.
    for prefix in ["lib/", "bin/", "tool/"] {
        if rel == prefix.trim_end_matches('/') || rel.starts_with(prefix) {
            return true;
        }
    }

    false
}

fn is_builtin_strip(rel: &str) -> bool {
    // Exact files anywhere: handled as suffix or exact basename.
    if rel.ends_with("/.DS_Store") || rel == ".DS_Store" {
        return true;
    }
    if rel.ends_with("/Thumbs.db") || rel == "Thumbs.db" {
        return true;
    }
    if rel.ends_with(".psd") {
        return true;
    }

    // Directory prefixes.
    const STRIP_DIRS: &[&str] = &[
        "example/",
        "examples/",
        "test/",
        "tests/",
        "doc/",
        "docs/",
        ".git/",
        ".github/",
        ".idea/",
        ".vscode/",
        "screenshots/",
    ];
    for d in STRIP_DIRS {
        let bare = d.trim_end_matches('/');
        if rel == bare || rel.starts_with(d) {
            return true;
        }
    }

    // Root-level dotfiles other than the explicit allowlist.
    if has_root_dotfile_prefix(rel) && !is_allowed_root_dotfile(rel) {
        return true;
    }

    false
}

/// Check if debloat is disabled via env var. Controlled via `HATCH_DEBLOAT=0`.
pub fn debloat_enabled() -> bool {
    match std::env::var("HATCH_DEBLOAT") {
        Ok(v) => v != "0" && !v.eq_ignore_ascii_case("false"),
        Err(_) => true,
    }
}

/// Two-pass extraction. Runs the full extractor security checks on both passes.
pub fn extract_with_debloat(tarball_bytes: &[u8], dest: &Path) -> Result<DebloatStats> {
    std::fs::create_dir_all(dest)?;

    // Pass 1: buffer manifest-like files.
    let (pubspec_src, hatch_src) = pass1_collect_manifests(tarball_bytes)?;
    let pubspec_hints = pubspec_src
        .as_deref()
        .map(parse_pubspec_hints)
        .unwrap_or_default();
    let overrides = hatch_src
        .as_deref()
        .map(parse_hatch_overrides)
        .unwrap_or_default();

    let keep_patterns = compile_patterns(&overrides.keep);
    let strip_patterns = compile_patterns(&overrides.strip);

    let filter_enabled = debloat_enabled();
    if !filter_enabled {
        debug!("HATCH_DEBLOAT disabled, extracting everything");
    }

    // Pass 2: extract with filter.
    pass2_extract(
        tarball_bytes,
        dest,
        &pubspec_hints,
        &keep_patterns,
        &strip_patterns,
        filter_enabled,
    )
}

fn pass1_collect_manifests(
    tarball_bytes: &[u8],
) -> Result<(Option<String>, Option<String>)> {
    let decoder = GzDecoder::new(Cursor::new(tarball_bytes));
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(false);
    archive.set_preserve_mtime(false);

    let mut entries = archive.entries()?;
    let mut long_name: Option<String> = None;

    let mut pubspec: Option<String> = None;
    let mut hatch: Option<String> = None;

    let mut entry_count: usize = 0;
    let mut cumulative_bytes: u64 = 0;

    while let Some(entry) = entries.next() {
        let mut entry = entry?;
        entry_count += 1;
        if entry_count > MAX_ENTRIES {
            return Err(anyhow!(
                "Archive exceeds maximum entry count ({}): possible archive bomb",
                MAX_ENTRIES
            ));
        }

        let header = entry.header().clone();
        let mut path = entry.path()?.to_path_buf();
        let path_str = path.to_string_lossy();

        if path_str.contains("@LongLink") || path_str == "././@LongLink" {
            let declared = header.size().unwrap_or(0);
            if declared as usize > MAX_LONGLINK_BYTES {
                return Err(anyhow!(
                    "Archive @LongLink entry declares {} bytes (max {})",
                    declared,
                    MAX_LONGLINK_BYTES
                ));
            }
            let mut buf = Vec::with_capacity(declared as usize);
            entry
                .by_ref()
                .take(MAX_LONGLINK_BYTES as u64 + 1)
                .read_to_end(&mut buf)?;
            if buf.len() > MAX_LONGLINK_BYTES {
                return Err(anyhow!(
                    "Archive @LongLink content exceeds {} bytes",
                    MAX_LONGLINK_BYTES
                ));
            }
            if let Some(pos) = buf.iter().position(|&b| b == 0) {
                buf.truncate(pos);
            }
            let candidate = String::from_utf8_lossy(&buf).into_owned();
            validate_archive_path(Path::new(&candidate))?;
            long_name = Some(candidate);
            continue;
        }

        if path_str == "pax_global_header" || path_str.starts_with("PaxHeader") {
            continue;
        }

        if let Some(name) = long_name.take() {
            path = PathBuf::from(name);
        }

        validate_archive_path(&path)?;

        let entry_type = header.entry_type();
        if entry_type.is_dir() {
            continue;
        }
        if !entry_type.is_file() {
            // Reject early, same rule as the main extractor.
            return Err(anyhow!(
                "Archive contains disallowed entry type {:?} for {}",
                entry_type,
                path.display()
            ));
        }

        let declared = header.size().unwrap_or(0);
        if declared > MAX_FILE_SIZE {
            return Err(anyhow!(
                "Archive contains file exceeding {} bytes: {}",
                MAX_FILE_SIZE,
                path.display()
            ));
        }
        cumulative_bytes = cumulative_bytes.saturating_add(declared);
        if cumulative_bytes > MAX_TOTAL_SIZE {
            return Err(anyhow!(
                "Archive cumulative decompressed size exceeds {} bytes: possible archive bomb",
                MAX_TOTAL_SIZE
            ));
        }

        let rel = normalise_rel(&path);
        let rel = rel.trim_start_matches("./").to_string();

        let want = is_pass1_target(&rel);
        if !want {
            continue;
        }

        if declared > MAX_MANIFEST_BYTES {
            return Err(anyhow!(
                "Manifest-like file {} exceeds {} bytes (paranoia cap)",
                rel,
                MAX_MANIFEST_BYTES
            ));
        }

        let mut body = Vec::with_capacity(declared as usize);
        let limit = MAX_MANIFEST_BYTES + 1;
        let n = entry.by_ref().take(limit).read_to_end(&mut body)?;
        if (n as u64) > MAX_MANIFEST_BYTES {
            return Err(anyhow!(
                "Manifest-like file {} body exceeds {} bytes",
                rel,
                MAX_MANIFEST_BYTES
            ));
        }

        // Only pubspec.yaml and .hatch.json are parsed; others are read so we
        // don't bias a subsequent size-bomb check, but they're discarded here.
        let text = match String::from_utf8(body) {
            Ok(s) => s,
            Err(_) => continue,
        };
        match rel.as_str() {
            "pubspec.yaml" => pubspec = Some(text),
            ".hatch.json" => hatch = Some(text),
            _ => {}
        }
    }

    Ok((pubspec, hatch))
}

fn is_pass1_target(rel: &str) -> bool {
    // Only root-level manifest-like files.
    if rel.contains('/') {
        return false;
    }
    if rel == "pubspec.yaml" || rel == ".hatch.json" {
        return true;
    }
    let upper: String = rel.chars().map(|c| c.to_ascii_uppercase()).collect();
    upper.starts_with("README") || upper.starts_with("LICENSE") || upper.starts_with("LICENCE")
        || upper.starts_with("CHANGELOG")
}

fn pass2_extract(
    tarball_bytes: &[u8],
    dest: &Path,
    pubspec: &PubspecHints,
    keep_patterns: &[Pattern],
    strip_patterns: &[Pattern],
    filter_enabled: bool,
) -> Result<DebloatStats> {
    let decoder = GzDecoder::new(Cursor::new(tarball_bytes));
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(false);
    archive.set_preserve_mtime(false);

    let mut entries = archive.entries()?;
    let mut long_name: Option<String> = None;

    let mut stats = DebloatStats::default();
    let mut entry_count: usize = 0;
    let mut cumulative_bytes: u64 = 0;

    while let Some(entry) = entries.next() {
        let mut entry = entry?;
        entry_count += 1;
        if entry_count > MAX_ENTRIES {
            return Err(anyhow!(
                "Archive exceeds maximum entry count ({}): possible archive bomb",
                MAX_ENTRIES
            ));
        }

        let header = entry.header().clone();
        let mut path = entry.path()?.to_path_buf();
        let path_str = path.to_string_lossy();

        if path_str.contains("@LongLink") || path_str == "././@LongLink" {
            let declared = header.size().unwrap_or(0);
            if declared as usize > MAX_LONGLINK_BYTES {
                return Err(anyhow!(
                    "Archive @LongLink entry declares {} bytes (max {})",
                    declared,
                    MAX_LONGLINK_BYTES
                ));
            }
            let mut buf = Vec::with_capacity(declared as usize);
            entry
                .by_ref()
                .take(MAX_LONGLINK_BYTES as u64 + 1)
                .read_to_end(&mut buf)?;
            if buf.len() > MAX_LONGLINK_BYTES {
                return Err(anyhow!(
                    "Archive @LongLink content exceeds {} bytes",
                    MAX_LONGLINK_BYTES
                ));
            }
            if let Some(pos) = buf.iter().position(|&b| b == 0) {
                buf.truncate(pos);
            }
            let candidate = String::from_utf8_lossy(&buf).into_owned();
            validate_archive_path(Path::new(&candidate))?;
            long_name = Some(candidate);
            continue;
        }

        if path_str == "pax_global_header" || path_str.starts_with("PaxHeader") {
            continue;
        }

        if let Some(name) = long_name.take() {
            path = PathBuf::from(name);
        }

        validate_archive_path(&path)?;

        let entry_type = header.entry_type();
        if entry_type.is_dir() {
            // Directories are created lazily as we write kept files; don't
            // pre-create stripped ones.
            continue;
        }
        if !entry_type.is_file() {
            return Err(anyhow!(
                "Archive contains disallowed entry type {:?} for {} (only regular files and directories are permitted)",
                entry_type,
                path.display()
            ));
        }

        let declared = header.size().unwrap_or(0);
        if declared > MAX_FILE_SIZE {
            return Err(anyhow!(
                "Archive contains file exceeding {} bytes: {}",
                MAX_FILE_SIZE,
                path.display()
            ));
        }
        cumulative_bytes = cumulative_bytes.saturating_add(declared);
        if cumulative_bytes > MAX_TOTAL_SIZE {
            return Err(anyhow!(
                "Archive cumulative decompressed size exceeds {} bytes: possible archive bomb",
                MAX_TOTAL_SIZE
            ));
        }

        let keep = if filter_enabled {
            keep_decision(&path, pubspec, keep_patterns, strip_patterns)
        } else {
            true
        };

        if !keep {
            // Drain the body so the tar reader advances past this entry, but
            // do not write anything.
            let limit = MAX_FILE_SIZE.saturating_add(1);
            let mut sink = std::io::sink();
            let n = std::io::copy(&mut entry.by_ref().take(limit), &mut sink)?;
            if n > MAX_FILE_SIZE {
                return Err(anyhow!(
                    "Archive file body exceeds {} bytes: {}",
                    MAX_FILE_SIZE,
                    path.display()
                ));
            }
            stats.entries_stripped += 1;
            stats.bytes_stripped = stats.bytes_stripped.saturating_add(n);
            continue;
        }

        // Kept: read bounded, write to disk.
        let dest_path = dest.join(&path);
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let limit = MAX_FILE_SIZE.saturating_add(1);
        let mut content = Vec::with_capacity(declared as usize);
        let n = entry.by_ref().take(limit).read_to_end(&mut content)?;
        if (n as u64) > MAX_FILE_SIZE {
            return Err(anyhow!(
                "Archive file body exceeds {} bytes: {}",
                MAX_FILE_SIZE,
                path.display()
            ));
        }

        std::fs::write(&dest_path, &content)
            .map_err(|e| anyhow!("Failed to write {:?}: {}", dest_path, e))?;

        stats.entries_kept += 1;
        stats.bytes_kept = stats.bytes_kept.saturating_add(n as u64);
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_keep_covers_manifests() {
        let p = PubspecHints::default();
        assert!(keep_decision(Path::new("pubspec.yaml"), &p, &[], &[]));
        assert!(keep_decision(Path::new("pubspec.lock"), &p, &[], &[]));
        assert!(keep_decision(Path::new("README.md"), &p, &[], &[]));
        assert!(keep_decision(Path::new("LICENSE"), &p, &[], &[]));
        assert!(keep_decision(Path::new("LICENCE.txt"), &p, &[], &[]));
        assert!(keep_decision(Path::new("CHANGELOG.md"), &p, &[], &[]));
        assert!(keep_decision(Path::new("lib/foo.dart"), &p, &[], &[]));
        assert!(keep_decision(Path::new("bin/run.dart"), &p, &[], &[]));
        assert!(keep_decision(Path::new("tool/gen.dart"), &p, &[], &[]));
    }

    #[test]
    fn builtin_keep_wins_over_strip() {
        let p = PubspecHints::default();
        let strip = compile_patterns(&["lib/**".to_string(), "pubspec.yaml".to_string()]);
        // Cannot strip built-in keep.
        assert!(keep_decision(Path::new("pubspec.yaml"), &p, &[], &strip));
        assert!(keep_decision(Path::new("lib/foo.dart"), &p, &[], &strip));
    }

    #[test]
    fn builtin_strip_removes_noise() {
        let p = PubspecHints::default();
        assert!(!keep_decision(Path::new("example/main.dart"), &p, &[], &[]));
        assert!(!keep_decision(Path::new("examples/demo.dart"), &p, &[], &[]));
        assert!(!keep_decision(Path::new("test/foo_test.dart"), &p, &[], &[]));
        assert!(!keep_decision(Path::new("doc/api.md"), &p, &[], &[]));
        assert!(!keep_decision(Path::new("docs/guide.md"), &p, &[], &[]));
        assert!(!keep_decision(Path::new(".git/HEAD"), &p, &[], &[]));
        assert!(!keep_decision(Path::new(".github/workflows/ci.yml"), &p, &[], &[]));
        assert!(!keep_decision(Path::new(".idea/workspace.xml"), &p, &[], &[]));
        assert!(!keep_decision(Path::new(".vscode/settings.json"), &p, &[], &[]));
        assert!(!keep_decision(Path::new("screenshots/hero.png"), &p, &[], &[]));
        assert!(!keep_decision(Path::new(".DS_Store"), &p, &[], &[]));
        assert!(!keep_decision(Path::new("sub/.DS_Store"), &p, &[], &[]));
        assert!(!keep_decision(Path::new("art/icon.psd"), &p, &[], &[]));
        assert!(!keep_decision(Path::new(".travis.yml"), &p, &[], &[]));
    }

    #[test]
    fn asset_dir_entry_keeps_contents() {
        let mut p = PubspecHints::default();
        p.assets.push("assets/images/".to_string());
        assert!(keep_decision(Path::new("assets/images/foo.png"), &p, &[], &[]));
        // Non-asset files outside the built-in strip list default to KEEP –
        // they need an explicit strip pattern to be dropped.
        let strip = compile_patterns(&["assets/videos/**".to_string()]);
        assert!(!keep_decision(Path::new("assets/videos/foo.mp4"), &p, &[], &strip));
    }

    #[test]
    fn asset_bare_entry_matches_as_prefix() {
        let mut p = PubspecHints::default();
        p.assets.push("assets/images".to_string());
        assert!(keep_decision(Path::new("assets/images/foo.png"), &p, &[], &[]));
    }

    #[test]
    fn font_assets_are_kept() {
        let mut p = PubspecHints::default();
        p.font_assets.push("fonts/Roboto-Regular.ttf".to_string());
        assert!(keep_decision(Path::new("fonts/Roboto-Regular.ttf"), &p, &[], &[]));
    }

    #[test]
    fn declared_platform_dirs_are_kept() {
        let mut p = PubspecHints::default();
        p.platform_dirs.insert("android".to_string());
        assert!(keep_decision(Path::new("android/build.gradle"), &p, &[], &[]));
        // iOS was not declared -> not conditionally kept.
        assert!(keep_decision(Path::new("ios/Runner.xcodeproj/project.pbxproj"), &p, &[], &[]));
        // Note: ios isn't in builtin strip set either, so default-keep applies.
        // That's fine: the conditional keep only matters when there's a
        // competing strip rule.
    }

    #[test]
    fn hatch_strip_removes_subtrees_not_builtin_kept() {
        let p = PubspecHints::default();
        let strip = compile_patterns(&["lib/generated/**".to_string()]);
        // Built-in keep for lib/ wins.
        assert!(keep_decision(Path::new("lib/generated/types.dart"), &p, &[], &strip));
        // But paths outside lib/bin/tool, strip works.
        let strip2 = compile_patterns(&["third_party/**".to_string()]);
        assert!(!keep_decision(Path::new("third_party/blob.bin"), &p, &[], &strip2));
    }

    #[test]
    fn hatch_keep_wins_over_builtin_strip() {
        let p = PubspecHints::default();
        let keep = compile_patterns(&["example/keep_me.txt".to_string()]);
        assert!(keep_decision(Path::new("example/keep_me.txt"), &p, &keep, &[]));
        assert!(!keep_decision(Path::new("example/drop_me.txt"), &p, &keep, &[]));
    }

    #[test]
    fn parse_pubspec_hints_reads_assets_fonts_platforms() {
        let yaml = r#"
name: foo
flutter:
  assets:
    - assets/images/
    - assets/data.json
  fonts:
    - family: Roboto
      fonts:
        - asset: fonts/Roboto-Regular.ttf
        - asset: fonts/Roboto-Bold.ttf
  plugin:
    platforms:
      android:
        package: com.example
      ios:
        pluginClass: FooPlugin
"#;
        let hints = parse_pubspec_hints(yaml);
        assert_eq!(hints.assets, vec!["assets/images/", "assets/data.json"]);
        assert_eq!(
            hints.font_assets,
            vec!["fonts/Roboto-Regular.ttf", "fonts/Roboto-Bold.ttf"]
        );
        assert!(hints.platform_dirs.contains("android"));
        assert!(hints.platform_dirs.contains("ios"));
        assert!(!hints.platform_dirs.contains("web"));
    }

    #[test]
    fn parse_hatch_overrides_accepts_schema() {
        let json = r#"{"hatch_package_version":1,"keep":["lib/**"],"strip":["example/**"]}"#;
        let o = parse_hatch_overrides(json);
        assert_eq!(o.hatch_package_version, 1);
        assert_eq!(o.keep, vec!["lib/**"]);
        assert_eq!(o.strip, vec!["example/**"]);
    }

    #[test]
    fn parse_hatch_overrides_tolerates_garbage() {
        let o = parse_hatch_overrides("not json");
        assert_eq!(o.keep.len(), 0);
        assert_eq!(o.strip.len(), 0);
    }
}
