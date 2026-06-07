//! iOS support: build a Flutter `.ipa` on Windows/Linux with no macOS, and
//! manage signing material + App Store Connect resources from the CLI.
//!
//! See `linux-flutter-ios-build-poc` notes: the pipeline uses Flutter's stock
//! Linux `gen_snapshot --snapshot_kind=app-aot-macho-dylib` to emit the iOS
//! `App.framework`, a cctools/ld64 cross toolchain for the native Runner, and
//! `zsign` for signing.

pub mod appstore;
pub mod builder;
pub mod config;
pub mod runner;
pub mod signing;
pub mod toolchain;

pub use config::IosConfig;
pub use runner::Runner;

/// Build a [`Runner`] from saved config (respects the configured WSL distro).
pub fn runner_for(cfg: &IosConfig) -> Runner {
    Runner::new(cfg.wsl_distro.clone())
}
