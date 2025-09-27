use super::schema::{HatchManifest, Profile};
use crate::resolver::dependency_utils::DependencyUtils;
use std::collections::HashMap;

/// Profile and override merging utilities
pub struct ManifestMerger;

impl ManifestMerger {
    /// Merge a profile into the base manifest
    pub fn merge_profile(mut manifest: HatchManifest, profile_name: &str) -> HatchManifest {
        if let Some(profiles) = &manifest.profiles {
            if let Some(profile) = profiles.get(profile_name) {
                // Merge profile dependencies
                if let Some(profile_deps) = &profile.require {
                    if let Some(ref mut base_deps) = manifest.require {
                        base_deps.extend(profile_deps.clone());
                    } else {
                        manifest.require = Some(profile_deps.clone());
                    }
                }

                // Merge profile dev dependencies
                if let Some(profile_dev_deps) = &profile.require_dev {
                    if let Some(ref mut base_dev_deps) = manifest.require_dev {
                        base_dev_deps.extend(profile_dev_deps.clone());
                    } else {
                        manifest.require_dev = Some(profile_dev_deps.clone());
                    }
                }

                // Merge profile scripts
                if let Some(profile_scripts) = &profile.scripts {
                    if let Some(ref mut base_scripts) = manifest.scripts {
                        base_scripts.extend(profile_scripts.clone());
                    } else {
                        manifest.scripts = Some(profile_scripts.clone());
                    }
                }
            }
        }

        manifest
    }

    /// Create a flattened view of dependencies for a given profile
    pub fn flatten_dependencies(manifest: &HatchManifest, profile_name: Option<&str>) -> HashMap<String, String> {
        let mut deps = HashMap::new();

        // Start with base dependencies (convert to simple format)
        if let Some(base_deps) = &manifest.require {
            deps.extend(DependencyUtils::to_simple_deps(base_deps));
        }

        // Add profile-specific dependencies if profile is specified
        if let Some(profile_name) = profile_name {
            if let Some(profiles) = &manifest.profiles {
                if let Some(profile) = profiles.get(profile_name) {
                    if let Some(profile_deps) = &profile.require {
                        deps.extend(DependencyUtils::to_simple_deps(profile_deps));
                    }
                }
            }
        }

        deps
    }

    /// Create a flattened view of dev dependencies for a given profile
    pub fn flatten_dev_dependencies(manifest: &HatchManifest, profile_name: Option<&str>) -> HashMap<String, String> {
        let mut deps = HashMap::new();

        // Start with base dev dependencies (convert to simple format)
        if let Some(base_dev_deps) = &manifest.require_dev {
            deps.extend(DependencyUtils::to_simple_deps(base_dev_deps));
        }

        // Add profile-specific dev dependencies if profile is specified
        if let Some(profile_name) = profile_name {
            if let Some(profiles) = &manifest.profiles {
                if let Some(profile) = profiles.get(profile_name) {
                    if let Some(profile_dev_deps) = &profile.require_dev {
                        deps.extend(DependencyUtils::to_simple_deps(profile_dev_deps));
                    }
                }
            }
        }

        deps
    }

    /// Create a flattened view of all dependencies (regular + dev) for a given profile
    pub fn flatten_all_dependencies(manifest: &HatchManifest, profile_name: Option<&str>) -> HashMap<String, String> {
        let mut deps = Self::flatten_dependencies(manifest, profile_name);
        let dev_deps = Self::flatten_dev_dependencies(manifest, profile_name);
        deps.extend(dev_deps);
        deps
    }

    /// Get effective scripts for a profile (base + profile-specific)
    pub fn flatten_scripts(manifest: &HatchManifest, profile_name: Option<&str>) -> HashMap<String, serde_json::Value> {
        let mut scripts = HashMap::new();

        // Start with base scripts
        if let Some(base_scripts) = &manifest.scripts {
            scripts.extend(base_scripts.clone());
        }

        // Add profile-specific scripts if profile is specified
        if let Some(profile_name) = profile_name {
            if let Some(profiles) = &manifest.profiles {
                if let Some(profile) = profiles.get(profile_name) {
                    if let Some(profile_scripts) = &profile.scripts {
                        scripts.extend(profile_scripts.clone());
                    }
                }
            }
        }

        scripts
    }

    /// Check if a profile exists in the manifest
    pub fn has_profile(manifest: &HatchManifest, profile_name: &str) -> bool {
        if let Some(profiles) = &manifest.profiles {
            profiles.contains_key(profile_name)
        } else {
            false
        }
    }

    /// Get list of available profile names
    pub fn get_profile_names(manifest: &HatchManifest) -> Vec<String> {
        if let Some(profiles) = &manifest.profiles {
            profiles.keys().cloned().collect()
        } else {
            Vec::new()
        }
    }
}