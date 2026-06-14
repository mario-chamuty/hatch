//! The plugin manifest: the JSON a plugin emits for `--hatch-manifest`, used by
//! core to integrate the plugin into `hatch --help` / `hatch plugin list`.

use serde::{Deserialize, Serialize};

/// Bumped only on breaking changes to the plugin contract. Core compares this
/// against a plugin's reported value and warns (does not hard-fail) on mismatch.
pub const HATCH_PLUGIN_API_VERSION: &str = "1";

/// The flag a plugin recognizes to print its manifest as JSON to stdout.
pub const MANIFEST_FLAG: &str = "--hatch-manifest";

/// Top-level description a plugin advertises to core.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin contract version the plugin was built against (see
    /// [`HATCH_PLUGIN_API_VERSION`]).
    pub hatch_plugin_api: String,
    /// The subcommand name, i.e. `hatch <name>` (without the `hatch-` prefix).
    pub name: String,
    /// Plugin version (informational, shown in `hatch plugin list`).
    pub version: String,
    /// One-line description shown next to the command in `hatch --help`.
    #[serde(default)]
    pub about: String,
    /// The plugin's own subcommands, for richer help listings.
    #[serde(default)]
    pub subcommands: Vec<SubcommandSpec>,
}

/// One subcommand of a plugin (e.g. `build`, `publish` for `hatch-ios`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubcommandSpec {
    pub name: String,
    #[serde(default)]
    pub about: String,
}

impl PluginManifest {
    /// Convenience constructor for plugins.
    pub fn new(name: impl Into<String>, version: impl Into<String>, about: impl Into<String>) -> Self {
        Self {
            hatch_plugin_api: HATCH_PLUGIN_API_VERSION.to_string(),
            name: name.into(),
            version: version.into(),
            about: about.into(),
            subcommands: Vec::new(),
        }
    }

    pub fn with_subcommands(mut self, subs: Vec<SubcommandSpec>) -> Self {
        self.subcommands = subs;
        self
    }

    /// Serialize for the `--hatch-manifest` response.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Parse a manifest emitted by a plugin.
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(s.trim())?)
    }

    /// Whether the plugin's contract version is compatible with this core.
    pub fn is_compatible(&self) -> bool {
        self.hatch_plugin_api == HATCH_PLUGIN_API_VERSION
    }
}

impl SubcommandSpec {
    pub fn new(name: impl Into<String>, about: impl Into<String>) -> Self {
        Self { name: name.into(), about: about.into() }
    }
}
