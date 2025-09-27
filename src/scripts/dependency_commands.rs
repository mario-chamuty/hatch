use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use log::{debug, info};

use crate::manifest::parser::ManifestParser;
use crate::manifest::schema::HatchManifest;
use crate::cache::paths::CachePaths;

/// Represents a command available from a dependency package
#[derive(Debug, Clone)]
pub struct DependencyCommand {
    pub package_name: String,
    pub command_name: String,
    pub description: Option<String>,
    pub package_path: PathBuf,
}

/// Manages commands from dependency packages
pub struct DependencyCommandManager;

impl DependencyCommandManager {
    /// Load the current project's manifest
    fn load_current_manifest() -> Result<HatchManifest> {
        ManifestParser::parse_with_overrides(
            "hatch.yaml",
            Some("hatch.local.yaml"),
        ).or_else(|_| {
            ManifestParser::parse_with_overrides(
                "hatch.json",
                Some("hatch.local.json"),
            )
        })
    }
    /// Discover all commands from installed dependencies
    pub fn discover_commands() -> Result<HashMap<String, Vec<DependencyCommand>>> {
        let mut commands_by_package = HashMap::new();

        // First check local packages from the manifest (using new dependency format)
        if let Ok(manifest) = Self::load_current_manifest() {
            // Check dependencies for local path packages
            if let Some(deps) = &manifest.require {
                for (package_name, dep) in deps {
                    if let Some(local_path) = dep.local_path() {
                        let full_path = std::env::current_dir()?.join(local_path);
                        if let Some(commands) = Self::extract_commands_from_package(&full_path, package_name)? {
                            if !commands.is_empty() {
                                info!("Found {} commands in local package {}", commands.len(), package_name);
                                commands_by_package.insert(package_name.clone(), commands);
                            }
                        }
                    }
                }
            }

            // Also check require-dev
            if let Some(deps) = &manifest.require_dev {
                for (package_name, dep) in deps {
                    if let Some(local_path) = dep.local_path() {
                        let full_path = std::env::current_dir()?.join(local_path);
                        if let Some(commands) = Self::extract_commands_from_package(&full_path, package_name)? {
                            if !commands.is_empty() {
                                info!("Found {} commands in local dev package {}", commands.len(), package_name);
                                commands_by_package.insert(package_name.clone(), commands);
                            }
                        }
                    }
                }
            }

            // Check deprecated local-packages field for backward compatibility
            if let Some(local_packages) = &manifest.local_packages {
                for (package_name, local_path) in local_packages {
                    let full_path = std::env::current_dir()?.join(local_path);
                    if let Some(commands) = Self::extract_commands_from_package(&full_path, package_name)? {
                        if !commands.is_empty() {
                            info!("Found {} commands in local package {}", commands.len(), package_name);
                            commands_by_package.insert(package_name.clone(), commands);
                        }
                    }
                }
            }
        }

        // Then check cached packages
        let packages_dir = CachePaths::packages_dir()?;

        if !packages_dir.exists() {
            return Ok(commands_by_package);
        }

        debug!("Discovering commands from dependencies in: {}", packages_dir.display());

        // Iterate through all cached packages
        for registry_entry in std::fs::read_dir(&packages_dir)? {
            let registry_entry = registry_entry?;
            if !registry_entry.path().is_dir() {
                continue;
            }

            // Iterate through packages in this registry
            for package_entry in std::fs::read_dir(registry_entry.path())? {
                let package_entry = package_entry?;
                if !package_entry.path().is_dir() {
                    continue;
                }

                let package_name = package_entry.file_name().to_string_lossy().to_string();

                // Check each version of the package
                for version_entry in std::fs::read_dir(package_entry.path())? {
                    let version_entry = version_entry?;
                    let version_path = version_entry.path();

                    if !version_path.is_dir() {
                        continue;
                    }

                    // Look for hatch.json or hatch.yaml in the package
                    if let Some(commands) = Self::extract_commands_from_package(&version_path, &package_name)? {
                        if !commands.is_empty() {
                            info!("Found {} commands in package {}", commands.len(), package_name);
                            commands_by_package.insert(package_name.clone(), commands);
                            break; // Use the first version with commands
                        }
                    }
                }
            }
        }

        Ok(commands_by_package)
    }

    /// Extract commands from a specific package directory
    fn extract_commands_from_package(
        package_path: &Path,
        package_name: &str,
    ) -> Result<Option<Vec<DependencyCommand>>> {
        let mut commands = Vec::new();

        // Try hatch.json first
        let hatch_json = package_path.join("hatch.json");
        let hatch_yaml = package_path.join("hatch.yaml");

        let manifest = if hatch_json.exists() {
            match ManifestParser::parse_json_file(&hatch_json) {
                Ok(m) => Some(m),
                Err(e) => {
                    debug!("Failed to parse {}: {}", hatch_json.display(), e);
                    None
                }
            }
        } else if hatch_yaml.exists() {
            match ManifestParser::parse_yaml_file(&hatch_yaml) {
                Ok(m) => Some(m),
                Err(e) => {
                    debug!("Failed to parse {}: {}", hatch_yaml.display(), e);
                    None
                }
            }
        } else {
            None
        };

        if let Some(manifest) = manifest {
            // Extract scripts/commands from the manifest
            if let Some(scripts) = &manifest.scripts {
                for (cmd_name, script_value) in scripts {
                    let (command, description) = match script_value {
                        serde_json::Value::String(_) => {
                            (cmd_name.clone(), None)
                        }
                        serde_json::Value::Object(obj) => {
                            let desc = obj.get("description")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            (cmd_name.clone(), desc)
                        }
                        _ => continue,
                    };

                    commands.push(DependencyCommand {
                        package_name: package_name.to_string(),
                        command_name: command,
                        description,
                        package_path: package_path.to_path_buf(),
                    });
                }
            }

            return Ok(Some(commands));
        }

        Ok(None)
    }

    /// Execute a namespaced command (package:command)
    pub fn execute_namespaced_command(
        namespaced_command: &str,
        args: Vec<String>,
    ) -> Result<()> {
        // Parse the namespace
        let parts: Vec<&str> = namespaced_command.split(':').collect();
        if parts.len() != 2 {
            return Err(anyhow!(
                "Invalid namespaced command format. Use 'package:command'"
            ));
        }

        let package_name = parts[0];
        let command_name = parts[1];

        // Discover all available commands
        let commands_by_package = Self::discover_commands()?;

        // Find the specific command
        let commands = commands_by_package.get(package_name)
            .ok_or_else(|| anyhow!("Package '{}' not found or has no commands", package_name))?;

        let command = commands.iter()
            .find(|c| c.command_name == command_name)
            .ok_or_else(|| anyhow!("Command '{}' not found in package '{}'", command_name, package_name))?;

        // Execute the command in the package directory
        Self::run_command_in_package(command, args)
    }

    /// Run a command in the context of a package directory
    fn run_command_in_package(command: &DependencyCommand, args: Vec<String>) -> Result<()> {
        println!("🚀 Running {}:{}", command.package_name, command.command_name);
        if let Some(desc) = &command.description {
            println!("   {}", desc);
        }

        // Load the package manifest to get the actual command
        let hatch_json = command.package_path.join("hatch.json");
        let hatch_yaml = command.package_path.join("hatch.yaml");

        let manifest = if hatch_json.exists() {
            ManifestParser::parse_json_file(&hatch_json)?
        } else if hatch_yaml.exists() {
            ManifestParser::parse_yaml_file(&hatch_yaml)?
        } else {
            return Err(anyhow!("No hatch manifest found in package"));
        };

        // Get the command from scripts
        if let Some(scripts) = &manifest.scripts {
            if let Some(script_value) = scripts.get(&command.command_name) {
                let cmd_string = match script_value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Object(obj) => {
                        obj.get("command")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| anyhow!("Invalid script format"))?
                            .to_string()
                    }
                    _ => return Err(anyhow!("Invalid script format")),
                };

                // Append args if provided
                let full_command = if args.is_empty() {
                    cmd_string
                } else {
                    format!("{} {}", cmd_string, args.join(" "))
                };

                // Execute the command in the package directory
                Self::execute_command_in_dir(&full_command, &command.package_path)
            } else {
                Err(anyhow!("Command not found in manifest"))
            }
        } else {
            Err(anyhow!("No scripts defined in package manifest"))
        }
    }

    /// Execute a shell command in a specific directory
    fn execute_command_in_dir(command: &str, dir: &Path) -> Result<()> {
        use std::process::{Command, Stdio};

        info!("Executing in {}: {}", dir.display(), command);

        let (shell, shell_arg) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let mut cmd = Command::new(shell);
        cmd.arg(shell_arg)
           .arg(command)
           .current_dir(dir)  // Run in package directory
           .stdout(Stdio::inherit())
           .stderr(Stdio::inherit())
           .stdin(Stdio::inherit());

        let status = cmd.status()
            .map_err(|e| anyhow!("Failed to execute command: {}", e))?;

        if !status.success() {
            return Err(anyhow!(
                "Command failed with exit code: {}",
                status.code().unwrap_or(-1)
            ));
        }

        Ok(())
    }

    /// List all available dependency commands
    pub fn list_all_commands() -> Result<()> {
        let commands_by_package = Self::discover_commands()?;

        if commands_by_package.is_empty() {
            println!("No commands found in installed dependencies");
            return Ok(());
        }

        println!("Available dependency commands:");
        println!();

        for (package_name, commands) in &commands_by_package {
            println!("📦 {}:", package_name);
            for cmd in commands {
                let desc = cmd.description.as_deref().unwrap_or("No description");
                println!("    {}:{} - {}", package_name, cmd.command_name, desc);
            }
            println!();
        }

        Ok(())
    }
}