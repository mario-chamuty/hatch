//! Update installed plugins from their remembered source, plus the throttled
//! autoupdate hook invoked on every plugin dispatch.

use anyhow::{anyhow, bail, Result};
use std::time::Duration;

use super::install;
use super::registry;
use super::state::{self, InstalledPlugin, Source};
use super::{github, manifest};

/// Resolve the latest available version string for a plugin's source.
async fn latest_version(source: &Source) -> Result<String> {
    match source {
        Source::Local { .. } => bail!("local plugins have no remote source"),
        Source::Github { repo, .. } => Ok(github::fetch_release(repo, None).await?.version),
        Source::Registry { name, .. } => {
            let plugin = registry::fetch_plugin(name).await?;
            Ok(plugin.resolve_version(None)?.version.clone())
        }
    }
}

/// Re-install a plugin from its source (used by explicit + auto update).
async fn reinstall(source: &Source) -> Result<String> {
    let m = match source {
        Source::Local { .. } => bail!("local plugins cannot be auto-updated"),
        // Pass `tag: None` so update follows the latest release, not the pinned one.
        Source::Github { repo, .. } => install::install_github(repo, None, false).await?,
        Source::Registry { name, .. } => install::install_registry(name, None, false).await?,
    };
    Ok(m.version)
}

/// True if `latest` is a strictly newer version than `installed`. Falls back to
/// a plain inequality when either side isn't valid semver.
fn is_newer(latest: &str, installed: &str) -> bool {
    match (semver::Version::parse(latest), semver::Version::parse(installed)) {
        (Ok(l), Ok(i)) => l > i,
        _ => latest != installed,
    }
}

/// `hatch plugin update <name>` — update one plugin.
pub async fn update_one(name: &str) -> Result<()> {
    let st = state::load();
    let entry = st
        .plugins
        .get(name)
        .cloned()
        .ok_or_else(|| anyhow!("plugin `{name}` was not installed by hatch (no recorded source)"))?;
    update_entry(&entry, true).await
}

/// `hatch plugin update --all` — update every plugin with a remote source.
pub async fn update_all() -> Result<()> {
    let st = state::load();
    let remotes: Vec<InstalledPlugin> =
        st.plugins.values().filter(|p| p.source.is_remote()).cloned().collect();
    if remotes.is_empty() {
        println!("No updatable plugins (none were installed from GitHub or the registry).");
        return Ok(());
    }
    for entry in remotes {
        if let Err(e) = update_entry(&entry, true).await {
            eprintln!("⚠️  {}: {e:#}", entry.name);
        }
    }
    Ok(())
}

/// Check a single entry and, if a newer version exists, install it.
/// `verbose` prints "already up to date" lines for explicit updates.
async fn update_entry(entry: &InstalledPlugin, verbose: bool) -> Result<()> {
    if !entry.source.is_remote() {
        if verbose {
            println!("`{}` was installed from a local path — nothing to update.", entry.name);
        }
        return Ok(());
    }
    let latest = latest_version(&entry.source).await?;
    if !is_newer(&latest, &entry.version) {
        let _ = state::touch_check(&entry.name);
        if verbose {
            println!("`{}` is already up to date (v{}).", entry.name, entry.version);
        }
        return Ok(());
    }
    let from = entry.version.clone();
    let installed = reinstall(&entry.source).await?;
    println!("🔄 plugin `{}` updated v{from} -> v{installed}", entry.name);
    Ok(())
}

/// Autoupdate hook called from dispatch before exec-ing a plugin. Throttled to
/// the configured interval and entirely best-effort: any failure (offline, rate
/// limit, parse error, timeout) is swallowed so the user's command still runs.
pub async fn maybe_auto_update(name: &str) {
    if std::env::var_os("HATCH_NO_PLUGIN_UPDATE").is_some() {
        return;
    }
    let st = state::load();
    if !st.settings.auto_update {
        return;
    }
    let Some(entry) = st.plugins.get(name).cloned() else {
        return; // not hatch-managed; leave it alone
    };
    if !entry.auto_update || !entry.source.is_remote() {
        return;
    }
    if !due_for_check(&entry, st.settings.check_interval_hours) {
        return;
    }

    // Bound the whole check+update so a slow network never stalls dispatch.
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        if let Err(e) = update_entry(&entry, false).await {
            log::debug!("autoupdate check for `{name}` failed: {e:#}");
            let _ = state::touch_check(name);
        }
    })
    .await;
}

/// Whether enough time has elapsed since the last update check.
fn due_for_check(entry: &InstalledPlugin, interval_hours: u64) -> bool {
    let Some(last) = &entry.last_check else {
        return true;
    };
    let Ok(last) = chrono::DateTime::parse_from_rfc3339(last) else {
        return true;
    };
    let elapsed = chrono::Utc::now().signed_duration_since(last.with_timezone(&chrono::Utc));
    elapsed.num_hours() >= interval_hours as i64
}

/// Best-effort lookup of the currently-installed version (for `info`).
pub fn installed_version(bin: &std::path::Path) -> Option<String> {
    manifest::query(bin).map(|m| m.version)
}
