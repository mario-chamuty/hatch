//! Shared contract between the `hatch` core CLI and its external subcommand
//! plugins (e.g. `hatch-ios`).
//!
//! A plugin is a standalone executable named `hatch-<name>` discoverable on
//! `PATH` or in `~/.hatch/plugins/bin/`. Core dispatches `hatch <name> ...` to
//! it by exec-ing `hatch-<name> ...` with the remaining argv. For help
//! integration a plugin answers `hatch-<name> --hatch-manifest` with the JSON
//! [`PluginManifest`] so core can list it under `hatch --help`.
//!
//! This crate is intentionally tiny and dependency-light so every plugin can
//! depend on it cheaply. It also hosts cross-cutting helpers that build plugins
//! commonly need ([`FvmDetector`], [`paths`]).

pub mod fvm;
pub mod manifest;
pub mod paths;

pub use fvm::FvmDetector;
pub use manifest::{PluginManifest, SubcommandSpec, HATCH_PLUGIN_API_VERSION};
