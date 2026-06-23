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
mod auth;
mod git;
mod plugin;

use cli::{Cli, Commands, verbosity, CacheCommands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set verbosity level
    verbosity::set_verbosity(cli.verbose);

    // Propagate --allow-unchecksummed to the process-wide security flag.
    cli::security::set_allow_unchecksummed(cli.allow_unchecksummed);

    // Initialize logger based on verbosity
    let log_level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        3 => "trace",  // -vvv (debug mode)
        _ => "trace",  // -vvvv+ (ultra-verbose)
    };

    // For -vvv mode, create a logfile in addition to console output
    if cli.verbose >= 3 {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::sync::Mutex;
        use chrono::Utc;

        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let log_filename = format!("hatch_debug_{}.log", timestamp);

        // Create the log file once and wrap it in a Mutex for thread-safe access
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_filename)
            .expect("Failed to create log file");
        let log_file = Mutex::new(log_file);

        let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level));

        // Create custom logger that writes to both console and file
        builder.format(move |buf, record| {
            use std::io::Write as IoWrite;

            let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let level = record.level();
            let target = record.target();
            let message = record.args();

            // Write to buffer (console output)
            writeln!(buf, "[{} {} {}] {}", timestamp, level, target, message)?;

            // Write to logfile (thread-safe)
            if let Ok(mut file) = log_file.lock() {
                let _ = writeln!(file, "[{} {} {}] {}", timestamp, level, target, message);
            }

            Ok(())
        });

        builder.init();

        println!("🔍 DEBUG MODE: Logging detailed information to {}", log_filename);
    } else {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level)).init();
    }

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
        Commands::Cache { subcommand } => {
            match subcommand {
                CacheCommands::Clear { force } => cli::commands::cache::clear(force).await,
                CacheCommands::Stats => cli::commands::cache::stats().await,
                CacheCommands::Remove { package, version } => {
                    cli::commands::cache::remove(&package, version.as_deref()).await
                }
                CacheCommands::List { detailed } => cli::commands::cache::list(detailed).await,
                CacheCommands::Prune { dry_run, aggressive } => cli::commands::cache::prune(dry_run, aggressive).await,
                CacheCommands::Verify => cli::commands::cache::verify().await,
            }
        }
        Commands::Plugin { subcommand } => {
            cli::commands::plugin::execute(subcommand).await
        }
        Commands::SelfUpdate { check, force, tag } => {
            cli::commands::selfupdate::execute(check, force, tag).await
        }
        Commands::External(argv) => {
            plugin::dispatch(argv).await
        }
    }
}