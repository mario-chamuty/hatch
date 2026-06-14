//! FVM detection now lives in the shared `hatch-plugin-api` crate so core and
//! build plugins share one implementation. Re-exported here to keep existing
//! `crate::fvm::detector::FvmDetector` call sites unchanged.

pub use hatch_plugin_api::FvmDetector;
