//! The hatch plugin subsystem.
//!
//! Plugins are standalone executables named `hatch-<name>` (the git/cargo
//! model). When the core CLI sees an unknown subcommand `hatch <name> ...` it
//! resolves `hatch-<name>` (in `~/.hatch/plugins/bin` first, then `PATH`) and
//! execs it with the remaining argv ([`dispatch`]). Plugins describe themselves
//! via `hatch-<name> --hatch-manifest` ([`manifest`]) for `hatch plugin list`.

pub mod discovery;
pub mod dispatch;
pub mod github;
pub mod install;
pub mod manifest;
pub mod registry;
pub mod sources;
pub mod state;
pub mod update;

pub use dispatch::dispatch;
