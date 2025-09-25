use anyhow::Result;
use log::info;
use std::path::PathBuf;

use crate::cli::FvmCommands;
use crate::fvm::FvmManager;
use crate::manifest::ManifestManager;

pub async fn execute(subcommand: FvmCommands) -> Result<()> {
    let project_dir = std::env::current_dir()?;
    let fvm_manager = FvmManager::new(project_dir.clone());

    match subcommand {
        FvmCommands::List => {
            info!("Listing Flutter versions");
            fvm_manager.list_versions()?;
        }
        FvmCommands::Use { version } => {
            info!("Using Flutter version: {}", version);
            fvm_manager.use_version(&version)?;
        }
        FvmCommands::Install { version } => {
            info!("Installing Flutter version: {}", version);
            fvm_manager.install_version(&version).await?;
        }
        FvmCommands::Sync => {
            info!("Syncing FVM configuration");

            // Load manifest to get required Flutter version
            let manifest_manager = ManifestManager::new(project_dir);
            match manifest_manager.load_manifest(None) {
                Ok(manifest) => {
                    if let Some(flutter_version) = &manifest.sdk.flutter {
                        fvm_manager.sync_with_manifest(flutter_version).await?;
                    } else {
                        println!("⚠️ No Flutter version specified in manifest");
                    }
                }
                Err(e) => {
                    println!("⚠️  Could not load manifest: {}", e);
                    println!("   Make sure you have a hatch.yaml or hatch.json file");
                }
            }
        }
    }

    Ok(())
}