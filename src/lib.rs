//! Hatch library crate.
//!
//! Exists so integration tests under `tests/` can reach into the extractor
//! and debloat code paths. The crate is still primarily a binary (`hatch`);
//! only a narrow, intentionally-exposed surface lives here.

pub mod cache;
pub mod cli;

// The following modules are referenced from `cli` and must be compiled into
// the same crate. They intentionally stay `pub` only because the module
// graph already treats them as such from `main.rs`.
pub mod config;
pub mod manifest;
pub mod resolver;
pub mod registry;
pub mod fvm;
pub mod generator;
pub mod backend;
pub mod submodules;
pub mod utils;
pub mod pubspec;
pub mod lockfile;
pub mod scripts;
pub mod branding;
pub mod auth;
pub mod git;
pub mod plugin;
