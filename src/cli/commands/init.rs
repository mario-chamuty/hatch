use anyhow::Result;
use log::info;
use std::path::PathBuf;

use crate::manifest::{ManifestManager, ManifestFormat};
use crate::utils::Utilities;

pub async fn execute(name: Option<String>, path: Option<String>) -> Result<()> {
    info!("Initializing new Hatch project");

    let project_path = PathBuf::from(path.unwrap_or_else(|| ".".to_string()));
    let project_name = name.unwrap_or_else(|| {
        Utilities::current_dir_name().unwrap_or_else(|| "my_flutter_app".to_string())
    });

    println!("🚀 Initializing Hatch project '{}' in '{}'", project_name, project_path.display());

    // Create manifest manager
    let manifest_manager = ManifestManager::new(project_path.clone());

    // Check if manifest already exists
    if project_path.join("hatch.json").exists() {
        println!("⚠️  Manifest file already exists. Use --force to overwrite.");
        return Ok(());
    }

    // Create hatch.json manifest
    manifest_manager.create_manifest(&project_name, ManifestFormat::Json)?;

    // Check if this is already a Flutter project
    let pubspec_path = project_path.join("pubspec.yaml");
    if !pubspec_path.exists() {
        println!("💡 This doesn't appear to be a Flutter project.");
        println!("   Run 'flutter create {}' to create a new Flutter project first.", project_name);
    }

    println!("✅ Hatch project '{}' initialized successfully", project_name);
    println!();
    println!("Next steps:");
    println!("  1. Edit hatch.json to configure dependencies");
    println!("  2. Run 'hatch install' to install dependencies");
    println!("  3. Run 'hatch --help' for more commands");

    Ok(())
}