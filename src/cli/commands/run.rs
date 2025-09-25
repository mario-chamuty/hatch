use anyhow::Result;
use log::info;

use crate::manifest::parser::ManifestParser;
use crate::scripts::ScriptRunner;

pub async fn execute(script_name: String, args: Vec<String>) -> Result<()> {
    info!("Running script");

    // Parse manifest
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
        // List available scripts
        ScriptRunner::list_scripts(&manifest);
    } else {
        // Run specific script with args if provided
        if !args.is_empty() {
            // TODO: Pass args to script
            println!("Note: Script args not yet implemented");
        }
        ScriptRunner::run_script(&manifest, &script_name)?;
    }

    Ok(())
}