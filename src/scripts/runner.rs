use anyhow::{anyhow, Result};
use std::process::{Command, Stdio};
use std::env;
use log::{debug, info};

use super::parser::{Script, ScriptParser};
use crate::manifest::schema::HatchManifest;

pub struct ScriptRunner;

impl ScriptRunner {
    pub fn run_script(manifest: &HatchManifest, script_name: &str) -> Result<()> {
        let script = ScriptParser::get_script(manifest, script_name)
            .ok_or_else(|| anyhow!("Script '{}' not found", script_name))?;

        println!("🚀 Running script '{}'", script_name);
        if let Some(desc) = &script.description {
            println!("   {}", desc);
        }

        Self::execute_command(&script.command)
    }

    pub fn run_hook(manifest: &HatchManifest, hook_name: &str) -> Result<()> {
        let hooks = ScriptParser::get_lifecycle_hooks(manifest);

        if let Some(command) = hooks.get(hook_name) {
            println!("🔗 Running {} hook", hook_name);
            Self::execute_command(command)?;
        }

        Ok(())
    }

    pub fn run_pre_install(manifest: &HatchManifest) -> Result<()> {
        Self::run_hook(manifest, "pre-install")
    }

    pub fn run_post_install(manifest: &HatchManifest) -> Result<()> {
        Self::run_hook(manifest, "post-install")
    }

    pub fn run_pre_build(manifest: &HatchManifest) -> Result<()> {
        Self::run_hook(manifest, "pre-build")
    }

    pub fn run_post_build(manifest: &HatchManifest) -> Result<()> {
        Self::run_hook(manifest, "post-build")
    }

    fn execute_command(command: &str) -> Result<()> {
        info!("Executing: {}", command);

        let (shell, shell_arg) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let mut cmd = Command::new(shell);
        cmd.arg(shell_arg)
           .arg(command)
           .stdout(Stdio::inherit())
           .stderr(Stdio::inherit())
           .stdin(Stdio::inherit());

        cmd.env("HATCH_VERSION", env!("CARGO_PKG_VERSION"));

        if let Ok(flutter_path) = env::var("FLUTTER_ROOT") {
            let path = env::var("PATH").unwrap_or_default();
            let new_path = format!("{}/bin;{}", flutter_path, path);
            cmd.env("PATH", new_path);
        }

        let status = cmd.status()
            .map_err(|e| anyhow!("Failed to execute command: {}", e))?;

        if !status.success() {
            return Err(anyhow!(
                "Script failed with exit code: {}",
                status.code().unwrap_or(-1)
            ));
        }

        Ok(())
    }

    pub fn list_scripts(manifest: &HatchManifest) {
        let scripts = ScriptParser::list_scripts(manifest);

        if scripts.is_empty() {
            println!("No scripts defined in manifest");
            return;
        }

        println!("Available scripts:");
        for (name, desc) in scripts {
            println!("  {} - {}", name, desc);
        }

        let hooks = ScriptParser::get_lifecycle_hooks(manifest);
        if !hooks.is_empty() {
            println!("\nLifecycle hooks:");
            for (hook, _) in hooks {
                println!("  {}", hook);
            }
        }
    }

    pub fn run_scripts(manifest: &HatchManifest, script_names: Vec<String>) -> Result<()> {
        for script_name in script_names {
            Self::run_script(manifest, &script_name)?;
        }
        Ok(())
    }

    pub fn run_profile_scripts(
        manifest: &HatchManifest,
        profile_name: &str,
        hook_name: &str,
    ) -> Result<()> {
        if let Some(profiles) = &manifest.profiles {
            if let Some(profile) = profiles.get(profile_name) {
                if let Some(scripts) = &profile.scripts {
                    if let Some(command) = scripts.get(hook_name) {
                        if let Some(cmd_str) = command.as_str() {
                            println!("🔗 Running {} hook for profile '{}'", hook_name, profile_name);
                            Self::execute_command(cmd_str)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}