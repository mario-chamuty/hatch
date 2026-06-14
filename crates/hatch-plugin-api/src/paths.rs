//! Shared filesystem locations under `~/.hatch` used by core and plugins.

use std::path::PathBuf;

/// `~/.hatch` — the hatch config/state directory. `None` if the home directory
/// cannot be resolved.
pub fn hatch_config_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".hatch"))
}

/// `~/.hatch/plugins/bin` — where `hatch plugin install` places plugin
/// executables and where core looks first when resolving `hatch-<name>`.
pub fn plugins_bin_dir() -> Option<PathBuf> {
    hatch_config_dir().map(|d| d.join("plugins").join("bin"))
}
