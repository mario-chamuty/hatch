//! Cross-platform command runner for the iOS build/sign toolchain.
//!
//! The whole iOS toolchain (gen_snapshot, clang/ld64, openssl, zsign) is a
//! Linux toolchain. On Windows we run it inside WSL; on Linux we run it
//! directly. Every command is delivered as a bash script over stdin
//! (`bash -s`) so we never have to fight argv quoting, and large blobs are
//! transported base64-encoded inside the script itself.

use anyhow::{anyhow, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

/// Where/how to execute the Linux toolchain.
#[derive(Debug, Clone)]
pub struct Runner {
    /// WSL distro name on Windows (e.g. "Ubuntu"). `None` => run bash natively.
    pub distro: Option<String>,
}

/// Result of a captured command.
pub struct Captured {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Captured {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
    /// stdout trimmed, or an error carrying stderr when the command failed.
    pub fn require(self) -> Result<String> {
        if self.ok() {
            Ok(self.stdout.trim().to_string())
        } else {
            Err(anyhow!(
                "command failed (exit {}):\n{}",
                self.status,
                self.stderr.trim()
            ))
        }
    }
}

impl Runner {
    /// Build a runner appropriate for the host OS.
    pub fn new(distro: Option<String>) -> Self {
        if cfg!(windows) {
            Self {
                distro: Some(distro.unwrap_or_else(|| "Ubuntu".to_string())),
            }
        } else {
            Self { distro: None }
        }
    }

    fn command(&self) -> Command {
        match &self.distro {
            Some(d) => {
                let mut c = Command::new("wsl");
                c.args(["-d", d, "-e", "bash", "-s"]);
                c
            }
            None => {
                let mut c = Command::new("bash");
                c.arg("-s");
                c
            }
        }
    }

    /// Run a bash script, capturing stdout/stderr. The script is fed on stdin
    /// with `set -euo pipefail` prepended unless `raw` is set.
    pub fn exec(&self, script: &str) -> Result<Captured> {
        self.exec_inner(script, false)
    }

    /// Like [`exec`] but does not inject `set -euo pipefail` (used for probes
    /// where individual command failure is expected and inspected).
    pub fn exec_raw(&self, script: &str) -> Result<Captured> {
        self.exec_inner(script, true)
    }

    fn exec_inner(&self, script: &str, raw: bool) -> Result<Captured> {
        let mut cmd = self.command();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .context("failed to spawn build shell (is WSL/bash available?)")?;

        let full = if raw {
            script.to_string()
        } else {
            format!("set -euo pipefail\n{script}\n")
        };
        child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("no stdin"))?
            .write_all(full.as_bytes())
            .context("failed to write script to shell")?;

        let out = child.wait_with_output().context("shell did not complete")?;
        Ok(Captured {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// Check that the runner can actually reach a Linux shell.
    pub fn probe(&self) -> Result<String> {
        self.exec("uname -a").and_then(Captured::require)
    }
}

/// Translate a host path to a path visible inside the build environment.
/// On Windows, `D:\a\b` becomes `/mnt/d/a/b` (the WSL drvfs mount). On Linux
/// the path is returned unchanged.
pub fn to_build_path(host_path: &str) -> String {
    if cfg!(windows) {
        let p = host_path.replace('\\', "/");
        // Match a leading drive letter like "D:/..."
        let bytes = p.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            let drive = (bytes[0] as char).to_ascii_lowercase();
            let rest = &p[2..];
            return format!("/mnt/{drive}{rest}");
        }
        p
    } else {
        host_path.to_string()
    }
}
