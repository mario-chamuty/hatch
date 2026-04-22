use std::collections::HashMap;
use crate::manifest::{Dependency, HatchManifest};

pub struct DependencyUtils;

impl DependencyUtils {
    /// Extract local packages from manifest dependencies
    pub fn extract_local_packages(manifest: &HatchManifest) -> HashMap<String, String> {
        let mut local_packages = HashMap::new();

        // Extract from require
        if let Some(deps) = &manifest.require {
            for (name, dep) in deps {
                if let Some(path) = dep.local_path() {
                    local_packages.insert(name.clone(), path.to_string());
                }
            }
        }

        // Extract from require-dev
        if let Some(deps) = &manifest.require_dev {
            for (name, dep) in deps {
                if let Some(path) = dep.local_path() {
                    local_packages.insert(name.clone(), path.to_string());
                }
            }
        }

        // Also check deprecated local-packages field for backward compatibility
        if let Some(legacy_local) = &manifest.local_packages {
            local_packages.extend(legacy_local.clone());
        }

        local_packages
    }

    /// Convert Dependency map to simple String map for version constraints
    /// This filters out local packages and returns only registry dependencies
    pub fn extract_registry_deps(deps: &HashMap<String, Dependency>) -> HashMap<String, String> {
        let mut registry_deps = HashMap::new();

        for (name, dep) in deps {
            // Skip local path dependencies
            if dep.is_local() {
                continue;
            }

            // Skip git dependencies
            if dep.is_git() {
                continue;
            }

            // Skip SDK dependencies
            if dep.is_sdk() {
                continue;
            }

            // Add registry dependencies (pub.dev or nest)
            registry_deps.insert(name.clone(), dep.version().to_string());
        }

        registry_deps
    }

    /// Get all dependencies as simple version map (for backward compatibility)
    pub fn to_simple_deps(deps: &HashMap<String, Dependency>) -> HashMap<String, String> {
        deps.iter()
            .map(|(name, dep)| (name.clone(), dep.version().to_string()))
            .collect()
    }

    /// Extract dependencies that should come from a specific nest
    pub fn extract_nest_deps(deps: &HashMap<String, Dependency>, nest_name: &str) -> HashMap<String, String> {
        let mut nest_deps = HashMap::new();

        for (name, dep) in deps {
            if let Some(nest) = dep.nest_name() {
                if nest == nest_name {
                    nest_deps.insert(name.clone(), dep.version().to_string());
                }
            }
        }

        nest_deps
    }

    /// Extract Git dependencies (supports both Complex and Simple "git:url" shorthand)
    pub fn extract_git_deps(deps: &HashMap<String, Dependency>) -> Vec<(String, String, Option<String>)> {
        let mut git_deps = Vec::new();

        for (name, dep) in deps {
            if dep.is_git() {
                if let Some(url) = dep.git_url() {
                    git_deps.push((
                        name.clone(),
                        url.to_string(),
                        dep.git_ref().map(|s| s.to_string()),
                    ));
                }
            }
        }

        git_deps
    }

    /// Extract SDK dependencies (supports both Complex and Simple "sdk:flutter" shorthand)
    pub fn extract_sdk_deps(deps: &HashMap<String, Dependency>) -> Vec<(String, String)> {
        let mut sdk_deps = Vec::new();

        for (name, dep) in deps {
            if dep.is_sdk() {
                if let Some(sdk) = dep.sdk_name() {
                    sdk_deps.push((name.clone(), sdk.to_string()));
                }
            }
        }

        sdk_deps
    }
}