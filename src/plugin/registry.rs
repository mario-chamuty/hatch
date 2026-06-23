//! Client for the tryhatch curated plugin registry (Composer/Packagist-style).
//!
//! The registry publishes two JSON documents that form the CLI contract:
//! - `{base}/registry/plugins.json` — the index (names + summaries).
//! - `{base}/registry/plugins/{name}.json` — one plugin with its versions and
//!   per-target download assets (with `sha256` for verification).
//!
//! The base URL defaults to `https://tryhatch.dev` and is overridable via the
//! `HATCH_REGISTRY` environment variable.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

const DEFAULT_BASE: &str = "https://tryhatch.dev";

/// The registry base URL (`HATCH_REGISTRY` override, else the default).
pub fn base_url() -> String {
    std::env::var("HATCH_REGISTRY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryIndex {
    #[serde(default)]
    pub plugins: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub latest_version: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryPlugin {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub versions: Vec<RegistryVersion>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryVersion {
    pub version: String,
    #[serde(default)]
    pub release_tag: Option<String>,
    #[serde(default)]
    pub yanked: bool,
    #[serde(default)]
    pub assets: Vec<RegistryAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryAsset {
    pub target: String,
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

impl RegistryPlugin {
    /// Newest non-yanked version, or a specific `version` if requested.
    pub fn resolve_version(&self, want: Option<&str>) -> Result<&RegistryVersion> {
        match want {
            Some(v) => self
                .versions
                .iter()
                .find(|rv| rv.version == v)
                .with_context(|| format!("registry has no version `{v}` for `{}`", self.name)),
            None => self
                .versions
                .iter()
                .find(|rv| !rv.yanked)
                .or_else(|| self.versions.first())
                .with_context(|| format!("registry lists no versions for `{}`", self.name)),
        }
    }
}

impl RegistryVersion {
    /// The asset for `target` (the platform triple).
    pub fn asset_for<'a>(&'a self, target: &str) -> Result<&'a RegistryAsset> {
        self.assets
            .iter()
            .find(|a| a.target == target)
            .with_context(|| format!("no `{target}` build published for version {}", self.version))
    }
}

async fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header("User-Agent", "hatch-plugin-installer")
        .header("Accept", "application/json")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        bail!("registry returned {} for {url}", resp.status());
    }
    resp.json::<T>().await.with_context(|| format!("parsing JSON from {url}"))
}

/// Fetch the registry index (`/registry/plugins.json`).
pub async fn fetch_index() -> Result<RegistryIndex> {
    get_json(&format!("{}/registry/plugins.json", base_url())).await
}

/// Fetch one plugin's full record (`/registry/plugins/{name}.json`).
pub async fn fetch_plugin(name: &str) -> Result<RegistryPlugin> {
    get_json(&format!("{}/registry/plugins/{name}.json", base_url())).await
}
