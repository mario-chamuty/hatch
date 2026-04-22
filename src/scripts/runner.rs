use anyhow::{anyhow, Result};
use std::env;
use std::process::{Command, Stdio};
use log::info;

use super::parser::{ScriptCommand, ScriptParser};
use super::trust::{ScriptOrigin, check_script_trust, TrustDecision};
use crate::manifest::schema::HatchManifest;

pub struct ScriptRunner;

impl ScriptRunner {
    /// Run a named script from the root manifest (origin = Root – no TOFU).
    pub fn run_script(manifest: &HatchManifest, script_name: &str) -> Result<()> {
        let script = ScriptParser::get_script(manifest, script_name)
            .ok_or_else(|| anyhow!("Script '{}' not found", script_name))?;

        println!("🚀 Running script '{}'", script_name);
        if let Some(desc) = &script.description {
            println!("   {}", desc);
        }

        Self::execute_command(&script.command, &[], &ScriptOrigin::Root, script_name)
    }

    /// Run a lifecycle hook from the root manifest.
    pub fn run_hook(manifest: &HatchManifest, hook_name: &str) -> Result<()> {
        let hooks = ScriptParser::get_lifecycle_hooks(manifest);

        if let Some(command) = hooks.get(hook_name) {
            println!("🔗 Running {} hook", hook_name);
            Self::execute_command(command, &[], &ScriptOrigin::Root, hook_name)?;
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

    /// Execute a parsed [`ScriptCommand`] given its origin. `user_args` are
    /// safely appended as positional arguments (never interpolated into the
    /// script body). `origin` drives the TOFU trust check.
    pub fn execute_command(
        command: &ScriptCommand,
        user_args: &[String],
        origin: &ScriptOrigin,
        script_name: &str,
    ) -> Result<()> {
        // Reject user args containing NUL / newline in all forms. This
        // matches the guarantee that dependency_commands.rs makes and keeps
        // shell and non-shell forms symmetric.
        for a in user_args {
            if a.contains('\0') || a.contains('\r') || a.contains('\n') {
                return Err(anyhow!(
                    "Refusing to pass argument containing NUL or newline"
                ));
            }
        }

        // TOFU: dependency scripts must be approved. Root scripts bypass.
        let body_for_trust = command.display();
        match check_script_trust(origin, script_name, &body_for_trust)? {
            TrustDecision::Allow => {}
            TrustDecision::Deny => {
                return Err(anyhow!("Script '{script_name}' blocked by trust decision"));
            }
        }

        info!("Executing script '{script_name}' [{:?}]: {}", origin_kind(origin), command.display());

        let mut cmd = match command {
            ScriptCommand::Argv(argv) => {
                if argv.is_empty() {
                    return Err(anyhow!("Script '{script_name}' has an empty argv"));
                }
                let mut c = Command::new(&argv[0]);
                c.args(&argv[1..]);
                for a in user_args {
                    c.arg(a);
                }
                c
            }
            ScriptCommand::Shell(body) => {
                if cfg!(target_os = "windows") {
                    // cmd.exe – append shell-quoted args (same approach as
                    // dependency_commands.rs).
                    let mut full = body.clone();
                    for a in user_args {
                        let escaped = a.replace('"', "\"\"");
                        full.push(' ');
                        full.push('"');
                        full.push_str(&escaped);
                        full.push('"');
                    }
                    let mut c = Command::new("cmd");
                    c.arg("/C").arg(full);
                    c
                } else {
                    let mut c = Command::new("sh");
                    c.arg("-c").arg(body).arg("hatch-script");
                    for a in user_args {
                        c.arg(a);
                    }
                    c
                }
            }
        };

        cmd.stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdin(Stdio::inherit());
        cmd.env("HATCH_VERSION", env!("CARGO_PKG_VERSION"));

        if let Ok(flutter_path) = env::var("FLUTTER_ROOT") {
            let path = env::var("PATH").unwrap_or_default();
            let sep = if cfg!(target_os = "windows") { ";" } else { ":" };
            let new_path = format!("{}{sep}{}", format!("{}/bin", flutter_path), path);
            cmd.env("PATH", new_path);
        }

        let status = cmd
            .status()
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
                        if let Some((cmd, _)) = super::parser::parse_script_value(command) {
                            println!("🔗 Running {} hook for profile '{}'", hook_name, profile_name);
                            Self::execute_command(&cmd, &[], &ScriptOrigin::Root, hook_name)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn origin_kind(o: &ScriptOrigin) -> &'static str {
    match o {
        ScriptOrigin::Root => "root",
        ScriptOrigin::Dependency { .. } => "dependency",
    }
}
