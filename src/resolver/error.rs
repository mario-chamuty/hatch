//! Resolver-specific error types.
//!
//! Central place for errors produced by the version-constraint propagator and
//! the pubgrub adapter. The `NoSolution` variant carries a pre-rendered,
//! pub-style explanation so CLI callers can print it verbatim.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResolverError {
    /// The constraint system is over-determined – no single assignment of
    /// package versions satisfies every requirement. The payload is a human
    /// readable explanation (pub-style "because A depends on B... and B
    /// depends on C..., version solving failed.")
    #[error("version solving failed:\n{0}")]
    NoSolution(String),

    /// A package was referenced during resolution but the registry returned
    /// no versions for it (e.g. typo, retraction, offline mode without
    /// cache).
    #[error("package '{0}' has no available versions")]
    NoVersions(String),

    /// An error surfaced from inside the pubgrub callback that was not a
    /// genuine constraint failure (usually an I/O hiccup fetching metadata
    /// for a package the solver asked about).
    #[error("resolver I/O error: {0}")]
    Io(String),
}
