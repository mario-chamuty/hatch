//! Persistent record of installed plugins and their update source.
//!
//! Lives at `~/.hatch/plugins/registry.json`. Tracks where each plugin came
//! from (local path / GitHub / tryhatch registry) so `hatch plugin update` and
//! the autoupdate-on-dispatch hook know how to fetch a newer build. Plugins
//! installed from a bare local path have no remote source and are never
//! auto-updated.

use anyhow::{Context, Result};
use hatch_plugin_api::paths::plugins_bin_dir;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn default_true() -> bool {
    true
}
fn default_interval() -> u64 {
    24
}

/// Where a plugin was installed from — drives autoupdate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    /// Installed from a local file; not auto-updatable.
    Local { path: String },
    /// Installed from a GitHub repo (`owner/repo`), pinned to a release tag.
    Github {
        repo: String,
        #[serde(default)]
        tag: Option<String>,
    },
    /// Installed from a tryhatch registry by plugin name.
    Registry {
        name: String,
        #[serde(default)]
        base: Option<String>,
    },
}

impl Source {
    pub fn is_remote(&self) -> bool {
        !matches!(self, Source::Local { .. })
    }

    /// Short human label for `hatch plugin list`'s SOURCE column.
    pub fn label(&self) -> String {
        match self {
            Source::Local { .. } => "local".to_string(),
            Source::Github { repo, .. } => format!("github:{repo}"),
            Source::Registry { name, .. } => format!("registry:{name}"),
        }
    }
}

/// One installed plugin's bookkeeping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    pub version: String,
    pub source: Source,
    #[serde(default)]
    pub installed_at: Option<String>,
    #[serde(default)]
    pub last_check: Option<String>,
    #[serde(default = "default_true")]
    pub auto_update: bool,
}

/// Global plugin settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_true")]
    pub auto_update: bool,
    #[serde(default = "default_interval")]
    pub check_interval_hours: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto_update: true, check_interval_hours: 24 }
    }
}

/// The whole `registry.json` document.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginState {
    #[serde(default)]
    pub plugins: BTreeMap<String, InstalledPlugin>,
    #[serde(default)]
    pub settings: Settings,
}

/// `~/.hatch/plugins` (parent of the `bin/` dir).
fn plugins_dir() -> Option<PathBuf> {
    plugins_bin_dir().and_then(|b| b.parent().map(|p| p.to_path_buf()))
}

/// Path to `~/.hatch/plugins/registry.json`.
pub fn state_path() -> Option<PathBuf> {
    plugins_dir().map(|d| d.join("registry.json"))
}

/// Load the state document, returning an empty default if it does not exist.
pub fn load() -> PluginState {
    let Some(path) = state_path() else {
        return PluginState::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return PluginState::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Persist the state document (pretty-printed).
pub fn save(state: &PluginState) -> Result<()> {
    let path = state_path().context("could not resolve ~/.hatch/plugins/registry.json")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(state)?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Record (or replace) a plugin entry and persist.
pub fn record(name: &str, version: &str, source: Source) -> Result<()> {
    let mut state = load();
    let now = chrono::Utc::now().to_rfc3339();
    let auto_update = state
        .plugins
        .get(name)
        .map(|p| p.auto_update)
        .unwrap_or(true);
    state.plugins.insert(
        name.to_string(),
        InstalledPlugin {
            name: name.to_string(),
            version: version.to_string(),
            source,
            installed_at: Some(now.clone()),
            last_check: Some(now),
            auto_update,
        },
    );
    save(&state)
}

/// Update just the `last_check` timestamp for a plugin (no version change).
pub fn touch_check(name: &str) -> Result<()> {
    let mut state = load();
    if let Some(p) = state.plugins.get_mut(name) {
        p.last_check = Some(chrono::Utc::now().to_rfc3339());
        save(&state)?;
    }
    Ok(())
}

/// Drop a plugin entry (on `hatch plugin remove`).
pub fn forget(name: &str) -> Result<()> {
    let mut state = load();
    if state.plugins.remove(name).is_some() {
        save(&state)?;
    }
    Ok(())
}
