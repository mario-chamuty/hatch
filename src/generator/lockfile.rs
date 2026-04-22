//! Thin re-export for API stability. The real lockfile writer lives in
//! [`crate::lockfile::generator`] so every caller funnels through the same
//! schema as the reader.

pub use crate::lockfile::generator::LockfileGenerator;
