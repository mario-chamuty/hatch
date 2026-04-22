//! Trust-On-First-Use (TOFU) script-trust model for dependency-authored
//! scripts.
//!
//! Scripts declared in the *root project's own manifest* bypass this check
//! entirely – the user is the author. Scripts declared in a *dependency's*
//! manifest go through an interactive prompt the first time they execute.
//!
//! Decisions persist to `~/.hatch/trust.json`.
//!
//! Granularity:
//!   * `allowed` – trusts the exact (package, version, script-body-sha256)
//!     triple. If the script body is edited in a subsequent version the
//!     trust does NOT carry over.
//!   * `always`  – trusts ALL scripts from `package` at any version. Stored
//!     under the bare package key.
//!   * `denied`  – blocks the script and every later execution until the
//!     user runs `hatch trust reset <package>`.
//!
//! Non-TTY / CI fallback: default to DENY. Set `HATCH_TRUST_SCRIPTS=all` to
//! bypass all prompts (audit-logged).

use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use log::warn;
use serde::{Deserialize, Serialize};

/// Origin of a script execution. Scripts originating from the user's own
/// repo bypass TOFU; scripts from dependencies do not.
#[derive(Debug, Clone)]
pub enum ScriptOrigin {
    Root,
    Dependency { package: String, version: String },
}

/// On-disk trust database. Values are one of `"allowed"`, `"denied"`, or
/// `"always"`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustStore {
    #[serde(flatten)]
    entries: BTreeMap<String, String>,
}

fn trust_store_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow!("Could not determine home directory"))?;
    Ok(home.join(".hatch").join("trust.json"))
}

fn audit_log_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow!("Could not determine home directory"))?;
    Ok(home.join(".hatch").join("audit.log"))
}

fn load_store() -> TrustStore {
    let Ok(path) = trust_store_path() else {
        return TrustStore::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => TrustStore::default(),
    }
}

fn save_store(store: &TrustStore) -> Result<()> {
    let path = trust_store_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(store)?;
    std::fs::write(&path, json)?;
    Ok(())
}

fn script_body_hash(body: &str) -> String {
    let hash = blake3::hash(body.as_bytes());
    // blake3 hex is 64 chars; use its hex rendering.
    hash.to_hex().to_string()
}

fn triple_key(package: &str, version: &str, body_hash: &str) -> String {
    format!("{package}@{version}@{body_hash}")
}

fn audit_line(event: &str, detail: &str) {
    let Ok(path) = audit_log_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = chrono::Utc::now().to_rfc3339();
    let line = format!("{ts}\t{event}\t{detail}\n");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Outcome of a trust check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    Allow,
    Deny,
}

/// Check whether the given script is trusted for the given origin.
///
/// * Root scripts are always allowed.
/// * Dependency scripts consult the persisted trust store and, on miss,
///   interactively prompt.
pub fn check_script_trust(
    origin: &ScriptOrigin,
    script_name: &str,
    script_body: &str,
) -> Result<TrustDecision> {
    let (package, version) = match origin {
        ScriptOrigin::Root => return Ok(TrustDecision::Allow),
        ScriptOrigin::Dependency { package, version } => (package.clone(), version.clone()),
    };

    // Environment escape: blanket bypass.
    if let Ok(val) = std::env::var("HATCH_TRUST_SCRIPTS") {
        if val.trim().eq_ignore_ascii_case("all") {
            warn!(
                "⚠️  HATCH_TRUST_SCRIPTS=all – bypassing prompt for {package}@{version}::{script_name}"
            );
            audit_line(
                "trust-env-bypass",
                &format!("{package}@{version}::{script_name}"),
            );
            return Ok(TrustDecision::Allow);
        }
    }

    let body_hash = script_body_hash(script_body);
    let triple = triple_key(&package, &version, &body_hash);

    let mut store = load_store();

    // 1. Always-trusted package?
    if let Some(v) = store.entries.get(&package) {
        match v.as_str() {
            "always" => return Ok(TrustDecision::Allow),
            "denied" => {
                return Err(anyhow!(
                    "Package '{package}' is denied. Run 'hatch trust reset {package}' to clear."
                ));
            }
            _ => {}
        }
    }

    // 2. Specific (package, version, hash) triple?
    if let Some(v) = store.entries.get(&triple) {
        match v.as_str() {
            "allowed" => return Ok(TrustDecision::Allow),
            "denied" => {
                return Err(anyhow!(
                    "Script '{script_name}' from {package}@{version} has been denied."
                ));
            }
            _ => {}
        }
    }

    // 3. Prompt – but only if stdin is a TTY. Otherwise fail closed.
    let stdin_is_tty = io::stdin().is_terminal();
    let stdout_is_tty = io::stdout().is_terminal();
    if !(stdin_is_tty && stdout_is_tty) {
        audit_line(
            "trust-deny-notty",
            &format!("{package}@{version}::{script_name}"),
        );
        return Err(anyhow!(
            "Refusing to execute script '{script_name}' from {package}@{version}: \
             non-interactive environment and no prior trust record. \
             Set HATCH_TRUST_SCRIPTS=all to bypass (audited) or run interactively \
             to approve."
        ));
    }

    println!(
        "Package '{package}' ({version}) declares a script '{script_name}'. \
         Allow execution? [y/N/always]"
    );
    print!("> ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let answer = line.trim().to_ascii_lowercase();

    let (decision, store_value, store_key) = match answer.as_str() {
        "y" | "yes" => (TrustDecision::Allow, "allowed".to_string(), triple.clone()),
        "a" | "always" => (TrustDecision::Allow, "always".to_string(), package.clone()),
        _ => (TrustDecision::Deny, "denied".to_string(), triple.clone()),
    };

    store.entries.insert(store_key, store_value.clone());
    if let Err(e) = save_store(&store) {
        warn!("Could not persist trust decision: {e}");
    }
    audit_line(
        &format!("trust-{store_value}"),
        &format!("{package}@{version}::{script_name}"),
    );

    if matches!(decision, TrustDecision::Deny) {
        return Err(anyhow!(
            "Script '{script_name}' from {package}@{version} denied by user."
        ));
    }
    Ok(decision)
}

/// Remove every trust entry pertaining to `package`. Used by the
/// (planned) `hatch trust reset` command. Returns the number of entries
/// removed.
pub fn reset_package_trust(package: &str) -> Result<usize> {
    let mut store = load_store();
    let prefix = format!("{package}@");
    let before = store.entries.len();
    store
        .entries
        .retain(|k, _| k != package && !k.starts_with(&prefix));
    let removed = before - store.entries.len();
    save_store(&store)?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_origin_always_allowed() {
        let d = check_script_trust(&ScriptOrigin::Root, "test", "echo hi").unwrap();
        assert_eq!(d, TrustDecision::Allow);
    }

    #[test]
    fn body_hash_is_stable() {
        let a = script_body_hash("echo hi");
        let b = script_body_hash("echo hi");
        assert_eq!(a, b);
        let c = script_body_hash("echo bye");
        assert_ne!(a, c);
    }
}
