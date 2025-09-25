use anyhow::Result;
use std::collections::HashMap;

use crate::manifest::schema::HatchManifest;

#[derive(Debug, Clone)]
pub struct Script {
    pub name: String,
    pub command: String,
    pub description: Option<String>,
}

pub struct ScriptParser;

impl ScriptParser {
    /// Parse scripts from manifest
    pub fn parse(manifest: &HatchManifest) -> Vec<Script> {
        let mut scripts = Vec::new();

        if let Some(manifest_scripts) = &manifest.scripts {
            for (name, value) in manifest_scripts {
                let (command, description) = match value {
                    serde_json::Value::String(cmd) => (cmd.clone(), None),
                    serde_json::Value::Object(obj) => {
                        let cmd = obj.get("command")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let desc = obj.get("description")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        (cmd, desc)
                    }
                    _ => continue,
                };

                scripts.push(Script {
                    name: name.clone(),
                    command,
                    description,
                });
            }
        }

        scripts
    }

    /// Get lifecycle hooks (pre-install, post-install, etc.)
    pub fn get_lifecycle_hooks(manifest: &HatchManifest) -> HashMap<String, String> {
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
                    if let Some(cmd) = value.as_str() {
                        hooks.insert(hook_name.to_string(), cmd.to_string());
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
        scripts.into_iter()
            .map(|s| (s.name, s.description.unwrap_or_else(|| s.command.clone())))
            .collect()
    }
}