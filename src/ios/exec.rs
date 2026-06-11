//! Native per-command execution for the Rust iOS pipeline.
//!
//! Unlike [`crate::ios::runner::Runner`] - which ships a whole bash script to a
//! shell, possibly tunnelled through WSL - `Exec` runs a single program with an
//! explicit argv + env directly via [`std::process::Command`]. It is host-native
//! by design: on Linux it drives the Linux toolchain; once a Windows-native
//! toolchain exists it will drive that. No shell, no argv quoting, no WSL tunnel
//! - removing those is the entire point of the rewrite.

use anyhow::{bail, Context, Result};
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Captured result of a single command.
#[derive(Debug)]
pub struct Output {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == 0
    }

    /// stdout as lossy UTF-8 (most tools we drive emit text).
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Trimmed stdout text, or an error carrying stderr when the command failed.
    pub fn require(self, what: &str) -> Result<String> {
        if self.ok() {
            Ok(self.stdout_text().trim().to_string())
        } else {
            bail!(
                "{what} failed (exit {}):\n{}",
                self.status,
                self.stderr.trim()
            )
        }
    }
}

/// How to spawn toolchain commands.
#[derive(Debug, Clone, Default)]
pub struct Exec;

impl Exec {
    /// Run `program` with `args`, capturing stdout (bytes) + stderr (text).
    pub fn run<I, S>(program: &str, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self::run_full(program, args, None, &[])
    }

    /// Run `program` with `args`, an optional working directory, and extra env.
    pub fn run_full<I, S>(
        program: &str,
        args: I,
        cwd: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(program);
        cmd.args(args);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd
            .output()
            .with_context(|| format!("failed to spawn `{program}`"))?;
        Ok(Output {
            status: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// Run a command and fail (with stderr) on a non-zero exit. Returns trimmed
    /// stdout for the rare callers that want it; most ignore the value.
    pub fn check<I, S>(what: &str, program: &str, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self::run(program, args)?.require(what)
    }

    /// Like [`check`] but with a working directory + env.
    pub fn check_in<I, S>(
        what: &str,
        program: &str,
        args: I,
        cwd: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self::run_full(program, args, cwd, env)?.require(what)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn run_captures_stdout_and_status() {
        let out = Exec::run("/bin/sh", ["-c", "printf hello"]).unwrap();
        assert!(out.ok());
        assert_eq!(out.stdout_text(), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn check_bails_on_failure_with_stderr() {
        let err = Exec::check("probe", "/bin/sh", ["-c", "echo boom >&2; exit 7"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("exit 7"), "{err}");
        assert!(err.contains("boom"), "{err}");
    }

    #[test]
    fn missing_program_is_a_context_error() {
        let err = Exec::run("definitely-not-a-real-program-xyz", ["x"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("definitely-not-a-real-program-xyz"), "{err}");
    }
}
