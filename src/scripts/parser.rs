use anyhow::Result;
use std::collections::HashMap;

use crate::manifest::schema::HatchManifest;

/// Command form for a script.
///
/// * `Shell` – legacy string form, executed via `sh -c` (or `cmd /C` on
///   Windows). User CLI args are appended as `$1..$N` positional params.
/// * `Argv`  – argv-array form, executed DIRECTLY via `Command::new` with no
///   shell interpretation at all. User CLI args are appended as extra argv
///   tokens. Recommended for new packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptCommand {
    Shell(String),
    Argv(Vec<String>),
}

impl ScriptCommand {
    /// Render as a human-readable single-line summary (used for logging and
    /// "command not set" error messages).
    pub fn display(&self) -> String {
        match self {
            ScriptCommand::Shell(s) => s.clone(),
            ScriptCommand::Argv(v) => v
                .iter()
                .map(|s| {
                    if s.contains(' ') {
                        format!("\"{}\"", s.replace('"', "\\\""))
                    } else {
                        s.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    /// True iff this command form bypasses the shell entirely. Callers that
    /// want to reason about shell injection can check this.
    pub fn is_argv(&self) -> bool {
        matches!(self, ScriptCommand::Argv(_))
    }
}

/// Parse a raw `serde_json::Value` from the `scripts` map into a
/// `(ScriptCommand, Option<description>)` pair. Accepts:
///   * `"echo hi"`                             – `Shell`
///   * `["flutter", "build", "apk"]`           – `Argv`
///   * `{"command": "...", "description": ...}` – either form nested under
///     `command`.
pub fn parse_script_value(value: &serde_json::Value) -> Option<(ScriptCommand, Option<String>)> {
    match value {
        serde_json::Value::String(s) => Some((ScriptCommand::Shell(s.clone()), None)),
        serde_json::Value::Array(items) => {
            let argv: Option<Vec<String>> = items
                .iter()
                .map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            argv.map(|a| (ScriptCommand::Argv(a), None))
        }
        serde_json::Value::Object(obj) => {
            let desc = obj
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let cmd_val = obj.get("command")?;
            let cmd = match cmd_val {
                serde_json::Value::String(s) => ScriptCommand::Shell(s.clone()),
                serde_json::Value::Array(items) => {
                    let argv: Option<Vec<String>> = items
                        .iter()
                        .map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                    ScriptCommand::Argv(argv?)
                }
                _ => return None,
            };
            Some((cmd, desc))
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct Script {
    pub name: String,
    pub command: ScriptCommand,
    pub description: Option<String>,
}

pub struct ScriptParser;

impl ScriptParser {
    /// Parse scripts from manifest
    pub fn parse(manifest: &HatchManifest) -> Vec<Script> {
        let mut scripts = Vec::new();

        if let Some(manifest_scripts) = &manifest.scripts {
            for (name, value) in manifest_scripts {
                if let Some((command, description)) = parse_script_value(value) {
                    scripts.push(Script {
                        name: name.clone(),
                        command,
                        description,
                    });
                }
            }
        }

        scripts
    }

    /// Get lifecycle hooks (pre-install, post-install, etc.)
    pub fn get_lifecycle_hooks(manifest: &HatchManifest) -> HashMap<String, ScriptCommand> {
        let mut hooks = HashMap::new();

        if let Some(scripts) = &manifest.scripts {
            // Standard lifecycle hooks
            let lifecycle_names = [
                "pre-install",
                "post-install",
                "pre-build",
                "post-build",
                "pre-test",
                "post-test",
                "pre-publish",
                "post-publish",
            ];

            for hook_name in lifecycle_names {
                if let Some(value) = scripts.get(hook_name) {
                    if let Some((cmd, _)) = parse_script_value(value) {
                        hooks.insert(hook_name.to_string(), cmd);
                    }
                }
            }
        }

        hooks
    }

    /// Get script by name
    pub fn get_script(manifest: &HatchManifest, name: &str) -> Option<Script> {
        let scripts = Self::parse(manifest);
        scripts.into_iter().find(|s| s.name == name)
    }

    /// List all available scripts
    pub fn list_scripts(manifest: &HatchManifest) -> Vec<(String, String)> {
        let scripts = Self::parse(manifest);
        scripts
            .into_iter()
            .map(|s| {
                let summary = s
                    .description
                    .unwrap_or_else(|| s.command.display());
                (s.name, summary)
            })
            .collect()
    }
}

// Keep `Result` in scope even if no fn uses it right now – the public API
// promise is that `Result` is available for extension.
#[allow(dead_code)]
type _ResultAlias = Result<()>;
