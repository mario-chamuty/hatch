use anyhow::Result;
use clap::Parser;
use log::info;

mod cli;
mod config;
mod manifest;
mod resolver;
mod registry;
mod fvm;
mod generator;
mod backend;
mod submodules;
mod utils;
mod pubspec;
mod cache;
mod lockfile;
mod scripts;
mod branding;

use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    info!("Starting Hatch CLI v{}", env!("CARGO_PKG_VERSION"));

    match cli.command {
        Commands::Init { name, path } => {
            cli::commands::init::execute(name, path).await
        }
        Commands::Install { profile, relax_constraints, force } => {
            cli::commands::install::execute_with_options(profile, relax_constraints, force).await
        }
        Commands::Update { packages } => {
            cli::commands::update::execute(packages).await
        }
        Commands::Add { package, version, dev } => {
            cli::commands::add::execute(package, version, dev).await
        }
        Commands::Remove { package } => {
            cli::commands::remove::execute(package).await
        }
        Commands::Why { package } => {
            cli::commands::why::execute(package).await
        }
        Commands::Profile { name } => {
            cli::commands::profile::execute(name).await
        }
        Commands::Sub { subcommand } => {
            cli::commands::sub::execute(subcommand).await
        }
        Commands::Fvm { subcommand } => {
            cli::commands::fvm::execute(subcommand).await
        }
        Commands::Run { script, args } => {
            cli::commands::run::execute(script, args).await
        }
        Commands::Build { platform, profile, channel } => {
            cli::commands::build::execute(platform, profile, channel).await
        }
        Commands::Publish { platform, channel, message } => {
            cli::commands::publish::execute(platform, channel, message).await
        }
        Commands::Builds { subcommand } => {
            cli::commands::builds::execute(subcommand).await
        }
        Commands::Version => {
            cli::commands::version::execute().await
        }
        Commands::Migrate { pubspec, output } => {
            cli::commands::migrate::execute(pubspec, output).await
        }
        Commands::SdkUpdate { flutter, dart, check } => {
            if check {
                cli::commands::sdk_update::execute_check().await
            } else {
                cli::commands::sdk_update::execute(flutter, dart).await
            }
        }
    }
}