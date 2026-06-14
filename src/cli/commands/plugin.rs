//! `hatch plugin` — manage external `hatch-<name>` subcommand plugins.

use anyhow::{anyhow, bail, Context, Result};
use hatch_plugin_api::paths::plugins_bin_dir;

use crate::cli::PluginCommands;
use crate::plugin::{discovery, manifest};

pub async fn execute(cmd: PluginCommands) -> Result<()> {
    match cmd {
        PluginCommands::List => list(),
        PluginCommands::Info { name } => info(&name),
        PluginCommands::Install { source } => install(&source),
        PluginCommands::Remove { name } => remove(&name),
    }
}

fn list() -> Result<()> {
    let plugins = discovery::list_plugins();
    if plugins.is_empty() {
        println!("No plugins installed. Install one with `hatch plugin install <path>`.");
        return Ok(());
    }
    println!("{:<14} {:<9} {}", "NAME", "VERSION", "DESCRIPTION");
    for (name, path) in &plugins {
        let (ver, about) = match manifest::query(path) {
            Some(m) => (m.version, m.about),
            None => ("?".to_string(), String::new()),
        };
        println!("{name:<14} {ver:<9} {about}");
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

fn install(source: &str) -> Result<()> {
    let src = std::path::Path::new(source);
    if !src.is_file() {
        bail!("`{source}` is not a file");
    }
    // Identify the plugin by asking it for its manifest.
    let m = manifest::query(src)
        .ok_or_else(|| anyhow!("`{source}` did not respond to --hatch-manifest; not a hatch plugin"))?;
    if !m.is_compatible() {
        eprintln!(
            "⚠️  plugin targets hatch_plugin_api {} but this hatch speaks {} — installing anyway",
            m.hatch_plugin_api,
            hatch_plugin_api::HATCH_PLUGIN_API_VERSION
        );
    }
    let dir = plugins_bin_dir().ok_or_else(|| anyhow!("could not resolve ~/.hatch/plugins/bin"))?;
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(discovery::plugin_binary_name(&m.name));
    std::fs::copy(src, &dest)
        .with_context(|| format!("copying {} -> {}", src.display(), dest.display()))?;
    println!("✅ installed plugin `{}` v{} -> {}", m.name, m.version, dest.display());
    Ok(())
}

fn remove(name: &str) -> Result<()> {
    let dir = plugins_bin_dir().ok_or_else(|| anyhow!("could not resolve ~/.hatch/plugins/bin"))?;
    let dest = dir.join(discovery::plugin_binary_name(name));
    if !dest.is_file() {
        bail!("plugin `{name}` is not installed in {}", dir.display());
    }
    std::fs::remove_file(&dest)?;
    println!("🗑️  removed plugin `{name}`");
    Ok(())
}
