//! `hatch plugin` — manage external `hatch-<name>` subcommand plugins.

use anyhow::{anyhow, Result};
use hatch_plugin_api::paths::plugins_bin_dir;

use crate::cli::PluginCommands;
use crate::plugin::{discovery, install, manifest, registry, state, update};

pub async fn execute(cmd: PluginCommands) -> Result<()> {
    match cmd {
        PluginCommands::List => list(),
        PluginCommands::Info { name } => info(&name),
        PluginCommands::Install { source } => install::install(&source).await,
        PluginCommands::Update { name, all } => update_cmd(name, all).await,
        PluginCommands::Search { query } => search(query.as_deref()).await,
        PluginCommands::Remove { name } => remove(&name),
    }
}

fn list() -> Result<()> {
    let plugins = discovery::list_plugins();
    if plugins.is_empty() {
        println!("No plugins installed. Install one with `hatch plugin install <source>`.");
        println!("  <source> = a local path, a GitHub repo (owner/repo), or a registry name.");
        return Ok(());
    }
    let st = state::load();
    println!("{:<12} {:<9} {:<22} {}", "NAME", "VERSION", "SOURCE", "DESCRIPTION");
    for (name, path) in &plugins {
        let (ver, about) = match manifest::query(path) {
            Some(m) => (m.version, m.about),
            None => ("?".to_string(), String::new()),
        };
        let source = st.plugins.get(name).map(|p| p.source.label()).unwrap_or_else(|| "-".into());
        println!("{name:<12} {ver:<9} {source:<22} {about}");
    }
    Ok(())
}

fn info(name: &str) -> Result<()> {
    let path = discovery::find_plugin(name)
        .ok_or_else(|| anyhow!("plugin `{name}` not found on PATH or in ~/.hatch/plugins/bin"))?;
    println!("{name}  ({})", path.display());
    match manifest::query(&path) {
        Some(m) => {
            println!("  version: {}", m.version);
            println!(
                "  api:     {}{}",
                m.hatch_plugin_api,
                if m.is_compatible() { "" } else { "  (INCOMPATIBLE with this hatch)" }
            );
            if let Some(entry) = state::load().plugins.get(name) {
                println!("  source:  {}", entry.source.label());
                println!(
                    "  auto-update: {}",
                    if entry.auto_update && entry.source.is_remote() { "on" } else { "off" }
                );
            }
            if !m.about.is_empty() {
                println!("  {}", m.about);
            }
            if !m.subcommands.is_empty() {
                println!("  subcommands:");
                for s in m.subcommands {
                    println!("    {:<16} {}", s.name, s.about);
                }
            }
        }
        None => println!("  (no manifest — not a hatch-aware plugin?)"),
    }
    Ok(())
}

async fn update_cmd(name: Option<String>, all: bool) -> Result<()> {
    match (name, all) {
        (Some(n), _) => update::update_one(&n).await,
        (None, true) => update::update_all().await,
        (None, false) => update::update_all().await,
    }
}

async fn search(query: Option<&str>) -> Result<()> {
    let index = registry::fetch_index().await?;
    let installed = discovery::list_plugins();
    let q = query.map(|s| s.to_lowercase());
    let mut matched = 0usize;
    println!("{:<12} {:<9} {:<10} {}", "NAME", "LATEST", "STATUS", "DESCRIPTION");
    for e in &index.plugins {
        if let Some(q) = &q {
            let hay = format!(
                "{} {} {}",
                e.name,
                e.title.clone().unwrap_or_default(),
                e.summary.clone().unwrap_or_default()
            )
            .to_lowercase();
            if !hay.contains(q) {
                continue;
            }
        }
        matched += 1;
        let latest = e.latest_version.clone().unwrap_or_else(|| "-".into());
        let status = if installed.contains_key(&e.name) { "installed" } else { "" };
        let desc = e.summary.clone().or_else(|| e.title.clone()).unwrap_or_default();
        println!("{:<12} {latest:<9} {status:<10} {desc}", e.name);
    }
    if matched == 0 {
        println!("(no matching plugins in the registry at {})", registry::base_url());
    } else {
        println!("\nInstall with `hatch plugin install <name>`.");
    }
    Ok(())
}

fn remove(name: &str) -> Result<()> {
    let dir = plugins_bin_dir().ok_or_else(|| anyhow!("could not resolve ~/.hatch/plugins/bin"))?;
    let dest = dir.join(discovery::plugin_binary_name(name));
    if !dest.is_file() {
        // Still forget any stale state entry.
        let _ = state::forget(name);
        anyhow::bail!("plugin `{name}` is not installed in {}", dir.display());
    }
    std::fs::remove_file(&dest)?;
    let _ = state::forget(name);
    println!("🗑️  removed plugin `{name}`");
    Ok(())
}
