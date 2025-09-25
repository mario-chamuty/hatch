use anyhow::Result;
use log::info;
use crate::cli::BuildsCommands;

pub async fn execute(subcommand: BuildsCommands) -> Result<()> {
    match subcommand {
        BuildsCommands::List { count, channel } => {
            info!("Listing builds");
            println!("📊 Listing {} builds", count);
            if let Some(ch) = channel {
                println!("   Filtering by channel: {}", ch);
            }
            // TODO: Fetch and display builds from backend
            println!("   No builds found (not implemented yet)");
        }
        BuildsCommands::Get { build, output } => {
            info!("Getting build: {}", build);
            println!("📥 Downloading build: {}", build);
            if let Some(out_path) = output {
                println!("   Output: {}", out_path);
            }
            // TODO: Download build from backend
            println!("   Build download not implemented yet");
        }
    }
    
    Ok(())
}