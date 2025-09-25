use clap::{Parser, Subcommand};

pub mod commands;
pub mod args;

#[derive(Parser)]
#[command(name = "hatch")]
#[command(about = "Next-Gen Dependency and Build Manager for Flutter")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(long_about = "Hatch - The unified toolchain Flutter has been missing.\nSmarter dependency management, automatic builds, and team-wide version sync.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Suppress output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Use a specific profile
    #[arg(long, global = true)]
    pub profile: Option<String>,

    /// Path to the project directory
    #[arg(long, global = true)]
    pub project_dir: Option<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new Hatch project
    Init {
        /// Project name
        name: Option<String>,
        /// Project path
        #[arg(short, long)]
        path: Option<String>,
    },
    /// Install dependencies and setup project
    Install {
        /// Profile to use for installation
        #[arg(short, long)]
        profile: Option<String>,
        /// Allow relaxed version constraints if exact matches fail
        #[arg(long)]
        relax_constraints: bool,
        /// Force refresh of all packages
        #[arg(short, long)]
        force: bool,
    },
    /// Update dependencies
    Update {
        /// Specific packages to update
        packages: Vec<String>,
    },
    /// Add a new dependency
    Add {
        /// Package name
        package: String,
        /// Package version constraint
        version: Option<String>,
        /// Add as dev dependency
        #[arg(short, long)]
        dev: bool,
    },
    /// Remove a dependency
    Remove {
        /// Package name
        package: String,
    },
    /// Explain why a package is installed
    Why {
        /// Package name
        package: String,
    },
    /// Switch to or show profile
    Profile {
        /// Profile name
        name: Option<String>,
    },
    /// Manage submodules
    Sub {
        #[command(subcommand)]
        subcommand: SubCommands,
    },
    /// Manage Flutter versions with FVM
    Fvm {
        #[command(subcommand)]
        subcommand: FvmCommands,
    },
    /// Run a script
    Run {
        /// Script name
        script: String,
        /// Additional arguments
        args: Vec<String>,
    },
    /// Build the project
    Build {
        /// Target platform
        platform: String,
        /// Build profile
        #[arg(short, long)]
        profile: Option<String>,
        /// Release channel
        #[arg(short, long)]
        channel: Option<String>,
    },
    /// Publish build artifacts
    Publish {
        /// Target platform
        platform: String,
        /// Release channel
        #[arg(short, long)]
        channel: Option<String>,
        /// Release message
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Manage published builds
    Builds {
        #[command(subcommand)]
        subcommand: BuildsCommands,
    },
    /// Show version information
    Version,
    /// Migrate from pubspec.yaml to hatch.json
    Migrate {
        /// Path to pubspec.yaml (defaults to ./pubspec.yaml)
        #[arg(short, long)]
        pubspec: Option<String>,
        /// Output path for hatch.json (defaults to ./hatch.json)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Update Flutter and Dart SDK versions
    SdkUpdate {
        /// Flutter version to update to (e.g., "3.16.9", "stable", "latest")
        #[arg(short, long)]
        flutter: Option<String>,
        /// Dart SDK constraint to update to (e.g., ">=3.0.0 <4.0.0")
        #[arg(short, long)]
        dart: Option<String>,
        /// Check for available updates without modifying
        #[arg(short, long)]
        check: bool,
    },
}

#[derive(Subcommand)]
pub enum SubCommands {
    /// List submodules
    List,
    /// Add a submodule
    Add {
        /// Path to submodule
        path: String,
    },
    /// Remove a submodule
    Remove {
        /// Path to submodule
        path: String,
    },
}

#[derive(Subcommand)]
pub enum FvmCommands {
    /// List available Flutter versions
    List,
    /// Use a specific Flutter version
    Use {
        /// Flutter version
        version: String,
    },
    /// Install a Flutter version
    Install {
        /// Flutter version
        version: String,
    },
    /// Sync FVM configuration
    Sync,
}

#[derive(Subcommand)]
pub enum BuildsCommands {
    /// List published builds
    List {
        /// Number of builds to show
        #[arg(short, long, default_value = "10")]
        count: usize,
        /// Channel to filter by
        #[arg(short = 'c', long)]
        channel: Option<String>,
    },
    /// Get/download a build
    Get {
        /// Build number or 'latest'
        build: String,
        /// Output directory
        #[arg(short, long)]
        output: Option<String>,
    },
}