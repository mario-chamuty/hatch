//! Dispatch an unknown `hatch <name> ...` invocation to the `hatch-<name>`
//! plugin executable.

use anyhow::{bail, Result};
use std::ffi::OsString;
use std::process::Command;

use super::discovery::{find_plugin, list_plugins, plugin_binary_name};

/// `argv` is the external-subcommand capture: `argv[0]` is the subcommand name
/// (the plugin), `argv[1..]` are the args to forward. Execs the plugin with
/// inherited stdio and exits the process with the plugin's exit code.
pub async fn dispatch(argv: Vec<OsString>) -> Result<()> {
    let mut it = argv.into_iter();
    let name_os = it.next().unwrap_or_default();
    let name = name_os.to_string_lossy().to_string();
    let rest: Vec<OsString> = it.collect();

    // Best-effort, throttled autoupdate before we hand off. Runs only for
    // hatch-managed plugins with a remote source, and never blocks or fails the
    // command (see `update::maybe_auto_update`).
    super::update::maybe_auto_update(&name).await;

    let Some(bin) = find_plugin(&name) else {
        let available = list_plugins();
        let mut msg = format!(
            "unknown command `{name}` — no built-in subcommand and no plugin `{}` on PATH or in ~/.hatch/plugins/bin",
            plugin_binary_name(&name)
        );
        if !available.is_empty() {
            let names: Vec<&str> = available.keys().map(|s| s.as_str()).collect();
            msg.push_str(&format!("\ninstalled plugins: {}", names.join(", ")));
        }
        msg.push_str("\nInstall one with `hatch plugin install <path>`.");
        bail!(msg);
    };

    let status = Command::new(&bin)
        .args(&rest)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run plugin {}: {e}", bin.display()))?;

    // Mirror the plugin's exit code so scripts/CI see the real result.
    std::process::exit(status.code().unwrap_or(1));
}
