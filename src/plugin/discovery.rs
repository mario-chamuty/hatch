//! Locate `hatch-<name>` plugin executables.

use hatch_plugin_api::paths::plugins_bin_dir;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The executable file name for a plugin, e.g. `hatch-ios` / `hatch-ios.exe`.
pub fn plugin_binary_name(name: &str) -> String {
    let exe = std::env::consts::EXE_SUFFIX; // "" on unix, ".exe" on windows
    format!("hatch-{name}{exe}")
}

/// Resolve the path to the `hatch-<name>` executable. Search order:
/// `~/.hatch/plugins/bin`, then each entry of `PATH`.
pub fn find_plugin(name: &str) -> Option<PathBuf> {
    let file = plugin_binary_name(name);

    if let Some(dir) = plugins_bin_dir() {
        let cand = dir.join(&file);
        if cand.is_file() {
            return Some(cand);
        }
    }

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join(&file);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }

    None
}

/// Discover all installed plugins as `name -> path`. Plugins in
/// `~/.hatch/plugins/bin` take precedence over identically-named ones on `PATH`.
pub fn list_plugins() -> BTreeMap<String, PathBuf> {
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();

    // PATH first, so the plugins dir (scanned last) wins on name collisions.
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(d) = plugins_bin_dir() {
        dirs.push(d);
    }

    let exe = std::env::consts::EXE_SUFFIX;
    for dir in dirs {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let fname = entry.file_name();
            let fname = fname.to_string_lossy();
            // strip optional .exe, require the `hatch-` prefix, exclude the core
            // binary itself (`hatch`).
            let stem = fname.strip_suffix(exe).unwrap_or(&fname);
            if let Some(name) = stem.strip_prefix("hatch-") {
                if !name.is_empty() && entry.path().is_file() {
                    found.insert(name.to_string(), entry.path());
                }
            }
        }
    }

    found
}
