//! Process-wide security toggles driven by CLI flags.
//!
//! These must NEVER be persisted to config – they are re-opted into on every
//! invocation. See `--allow-unchecksummed` in `cli::Cli`.

use std::sync::atomic::{AtomicBool, Ordering};

static ALLOW_UNCHECKSUMMED: AtomicBool = AtomicBool::new(false);

/// Record (or clear) the `--allow-unchecksummed` flag for the running process.
pub fn set_allow_unchecksummed(allow: bool) {
    ALLOW_UNCHECKSUMMED.store(allow, Ordering::SeqCst);
}

/// True iff the user passed `--allow-unchecksummed` on the command line.
pub fn allow_unchecksummed() -> bool {
    ALLOW_UNCHECKSUMMED.load(Ordering::SeqCst)
}

/// Resolve an `Option<&str>` checksum from a registry into the string the
/// cache/downloader expects.
///
/// * `Some(sha)` – use as-is.
/// * `None` + `--allow-unchecksummed` passed – returns the bypass sentinel.
/// * `None` without the flag – `Err(_)`, install must fail closed.
pub fn resolve_checksum_or_bypass(
    name: &str,
    version: &str,
    archive_sha256: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(sha) = archive_sha256 {
        if !sha.is_empty() {
            return Ok(sha.to_string());
        }
    }
    if allow_unchecksummed() {
        Ok(crate::cache::downloader::ALLOW_UNCHECKSUMMED_SENTINEL.to_string())
    } else {
        Err(anyhow::anyhow!(
            "Registry did not supply a checksum for {name}@{version}. \
             Refusing to install. Re-run with --allow-unchecksummed to \
             bypass (the event will be recorded in ~/.hatch/audit.log)."
        ))
    }
}
