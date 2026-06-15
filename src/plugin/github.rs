//! Minimal GitHub Releases client for installing/updating plugins.
//!
//! Resolves a repo's latest (or tagged) release and the download URL of the
//! asset matching the current platform. Authenticates with `GITHUB_TOKEN` /
//! `GH_TOKEN` when present (private repos + higher rate limit), works
//! unauthenticated otherwise.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
    browser_download_url: String,
}

fn token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .ok()
        .or_else(|| std::env::var("GH_TOKEN").ok())
        .filter(|t| !t.is_empty())
}

/// Fetch the latest release (or a specific tag) of `owner/repo`.
pub async fn fetch_release(repo: &str, tag: Option<&str>) -> Result<Release> {
    let url = match tag {
        Some(t) => format!("https://api.github.com/repos/{repo}/releases/tags/{t}"),
        None => format!("https://api.github.com/repos/{repo}/releases/latest"),
    };

    let client = reqwest::Client::new();
    let mut req = client
        .get(&url)
        .header("User-Agent", "hatch-plugin-installer")
        .header("Accept", "application/vnd.github+json");
    if let Some(tok) = token() {
        req = req.header("Authorization", format!("Bearer {tok}"));
    }

    let resp = req.send().await.with_context(|| format!("GET {url}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        bail!(
            "no {} release found for `{repo}` (is the repo public and does it have releases?)",
            tag.map(|t| format!("`{t}`")).unwrap_or_else(|| "latest".into())
        );
    }
    if !resp.status().is_success() {
        bail!("GitHub API returned {} for {url}", resp.status());
    }

    let raw: RawRelease = resp.json().await.context("parsing GitHub release JSON")?;
    let version = raw.tag_name.trim_start_matches('v').to_string();
    Ok(Release {
        tag: raw.tag_name,
        version,
        assets: raw
            .assets
            .into_iter()
            .map(|a| Asset { name: a.name, url: a.browser_download_url })
            .collect(),
    })
}

/// Pick the release asset for `target` (the platform triple). Prefers an asset
/// whose name contains the triple; falls back to the single asset if a release
/// ships exactly one. Errors listing candidates when ambiguous.
pub fn pick_asset<'a>(assets: &'a [Asset], target: &str) -> Result<&'a Asset> {
    if let Some(a) = assets.iter().find(|a| a.name.contains(target)) {
        return Ok(a);
    }
    if assets.len() == 1 {
        return Ok(&assets[0]);
    }
    if assets.is_empty() {
        bail!("release has no downloadable assets");
    }
    let names: Vec<&str> = assets.iter().map(|a| a.name.as_str()).collect();
    Err(anyhow!(
        "no asset matches this platform ({target}); available assets: {}",
        names.join(", ")
    ))
}
