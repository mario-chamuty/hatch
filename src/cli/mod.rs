use clap::{Parser, Subcommand};

pub mod commands;
pub mod args;
pub mod verbosity;
pub mod security;

#[derive(Parser)]
#[command(name = "hatch")]
#[command(about = "Next-Gen Dependency and Build Manager for Flutter")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(long_about = "Hatch - The unified toolchain Flutter has been missing.\nSmarter dependency management, automatic builds, and team-wide version sync.\n\nEnvironment variables:\n  HATCH_CACHE_DIR - Custom cache directory path (useful for CI/CD)\n  HATCH_API_KEY   - API key for backend services")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Increase verbosity (use -v, -vv, or -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Use a specific profile
    #[arg(long, global = true)]
    pub profile: Option<String>,

    /// Path to the project directory
    #[arg(long, global = true)]
    pub project_dir: Option<String>,

    /// SECURITY: allow downloading packages that the registry did not
    /// publish a checksum for. This is per-invocation only (NEVER a
    /// config setting) and is recorded in `~/.hatch/audit.log`.
    #[arg(long, global = true)]
    pub allow_unchecksummed: bool,
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
    /// Clear the package cache
    Cache {
        #[command(subcommand)]
        subcommand: CacheCommands,
    },
    /// Build & sign iOS apps without a Mac, and manage App Store Connect
    Ios {
        #[command(subcommand)]
        subcommand: IosCommands,
    },
}

/// iOS build + signing + App Store Connect management.
#[derive(Subcommand)]
pub enum IosCommands {
    /// Check the iOS build toolchain health
    Doctor,
    /// Configure App Store Connect API credentials
    Auth {
        /// Browser-assisted setup: open the API Keys page and auto-import the
        /// downloaded AuthKey_*.p8 (prompts for the issuer ID).
        #[arg(long)]
        web: bool,
        /// Issuer ID (UUID from App Store Connect -> Integrations)
        #[arg(long)]
        issuer: Option<String>,
        /// Key ID (10-char key identifier)
        #[arg(long)]
        key_id: Option<String>,
        /// Path to the AuthKey_<KEYID>.p8 file
        #[arg(long)]
        p8: Option<String>,
        /// Apple Developer Team ID (optional)
        #[arg(long)]
        team_id: Option<String>,
    },
    /// Download the Flutter engine artifacts for the toolchain
    Setup {
        /// Flutter version (defaults to the project's pinned FVM version)
        #[arg(long)]
        flutter: Option<String>,
    },
    /// Build an .ipa from the current project (no Mac required)
    Build {
        /// Bundle identifier (defaults to config / com.example.<name>)
        #[arg(long)]
        bundle_id: Option<String>,
        /// App display name (defaults to pubspec name)
        #[arg(long)]
        name: Option<String>,
        /// Sign the .ipa with the configured certificate + profile
        #[arg(long)]
        sign: bool,
        /// Use a distribution (App Store) profile instead of development
        #[arg(long)]
        distribution: bool,
    },
    /// Upload a signed .ipa to TestFlight / App Store Connect
    Publish {
        /// Path to the .ipa (defaults to the latest signed build in the project)
        #[arg(long)]
        ipa: Option<String>,
    },
    /// Validate a signed .ipa for App Store upload (no upload, no credentials)
    Validate {
        /// Path to the .ipa (defaults to the latest signed build in the project)
        #[arg(long)]
        ipa: Option<String>,
    },
    /// List apps registered in App Store Connect
    Apps,
    /// List registered test devices
    Devices,
    /// Register a test device
    DeviceAdd {
        /// Device name
        #[arg(long)]
        name: String,
        /// Device UDID
        #[arg(long)]
        udid: String,
    },
    /// List signing certificates
    Certs,
    /// Generate a signing certificate (key + CSR -> ASC -> .p12)
    CertCreate {
        /// Create a distribution certificate instead of development
        #[arg(long)]
        distribution: bool,
        /// Password for the generated .p12 (default: "hatch")
        #[arg(long)]
        password: Option<String>,
    },
    /// List provisioning profiles
    Profiles,
    /// Create a provisioning profile for a bundle id
    ProfileCreate {
        /// Profile name
        #[arg(long)]
        name: String,
        /// Bundle identifier the profile is for
        #[arg(long)]
        bundle_id: String,
        /// Create an App Store distribution profile
        #[arg(long)]
        distribution: bool,
        /// Device UDIDs to include (development profiles); repeatable
        #[arg(long = "device")]
        device: Vec<String>,
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

#[derive(Subcommand)]
pub enum CacheCommands {
    /// Clear all cached packages
    Clear {
        /// Force clear without confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Show cache statistics
    Stats,
    /// Clear specific package from cache
    Remove {
        /// Package name
        package: String,
        /// Package version (optional)
        version: Option<String>,
    },
    /// List all cached packages
    List {
        /// Show detailed information
        #[arg(short, long)]
        detailed: bool,
    },
    /// Remove unused packages from cache
    Prune {
        /// Dry run - show what would be removed without removing
        #[arg(long)]
        dry_run: bool,
        /// Also aggressively prune metadata, re-apply debloat to already-extracted
        /// packages, and relocate .hatch_metadata.json sidecars to a central
        /// ~/.hatch/cache/index.json. Lockfile-only.
        #[arg(long)]
        aggressive: bool,
    },
    /// Verify cache integrity
    Verify,
}