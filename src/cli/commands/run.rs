use anyhow::Result;
use log::info;

use crate::manifest::parser::ManifestParser;
use crate::scripts::{ScriptRunner, DependencyCommandManager};

pub async fn execute(script_name: String, args: Vec<String>) -> Result<()> {
    info!("Running script");

    // Check if this is a namespaced command (package:command)
    if script_name.contains(':') {
        return DependencyCommandManager::execute_namespaced_command(&script_name, args);
    }

    // Parse manifest for local scripts
    let manifest = ManifestParser::parse_with_overrides(
        "hatch.yaml",
        Some("hatch.local.yaml"),
    ).or_else(|_| {
        ManifestParser::parse_with_overrides(
            "hatch.json",
            Some("hatch.local.json"),
        )
    })?;

    if script_name == "list" || script_name.is_empty() {
        // List available scripts (both local and from dependencies)
        println!("=== Local Scripts ===");
        ScriptRunner::list_scripts(&manifest);

        println!("\n=== Dependency Commands ===");
        DependencyCommandManager::list_all_commands()?;
    } else {
        // Run specific script with args if provided
        if !args.is_empty() {
            // TODO: Pass args to script
            println!("Note: Script args not yet implemented for local scripts");
        }
        ScriptRunner::run_script(&manifest, &script_name)?;
    }

    Ok(())
}