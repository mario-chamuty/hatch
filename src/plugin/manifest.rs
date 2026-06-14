//! Query a plugin's self-description via `hatch-<name> --hatch-manifest`.

use hatch_plugin_api::{manifest::MANIFEST_FLAG, PluginManifest};
use std::path::Path;
use std::process::Command;

/// Run `<bin> --hatch-manifest` and parse the JSON. Returns `None` if the plugin
/// doesn't support the probe or emits invalid JSON (older / foreign plugins).
pub fn query(bin: &Path) -> Option<PluginManifest> {
    let out = Command::new(bin).arg(MANIFEST_FLAG).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    PluginManifest::from_json(&text).ok()
}
