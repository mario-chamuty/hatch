use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use super::dependency::Dependency;

/// Main Hatch manifest structure (supports both YAML and JSON)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HatchManifest {
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub sdk: SdkConstraints,
    pub require: Option<HashMap<String, Dependency>>,
    #[serde(rename = "require-dev")]
    pub require_dev: Option<HashMap<String, Dependency>>,
    pub profiles: Option<HashMap<String, Profile>>,
    pub submodules: Option<Vec<String>>,
    pub repositories: Option<Vec<Repository>>,
    pub nests: Option<Vec<Nest>>,
    pub build: Option<BuildConfig>,
    pub scripts: Option<HashMap<String, serde_json::Value>>,
    pub overrides: Option<HashMap<String, String>>,
    #[serde(rename = "disable-pub")]
    pub disable_pub: Option<bool>,
    #[serde(rename = "prefer-newest-from")]
    pub prefer_newest_from: Option<String>, // "nest" or "pub"
    #[serde(rename = "local-packages")]
    pub local_packages: Option<HashMap<String, String>>, // Deprecated, kept for backward compat
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkConstraints {
    pub flutter: Option<String>,
    pub dart: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub require: Option<HashMap<String, Dependency>>,
    #[serde(rename = "require-dev")]
    pub require_dev: Option<HashMap<String, Dependency>>,
    pub scripts: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nest {
    pub name: String,
    pub url: String,
    pub auth: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    #[serde(rename = "type")]
    pub repo_type: String,
    pub url: String,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    pub versioning: Option<VersioningConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersioningConfig {
    pub url: String,
    pub apikey: String,
    pub project: String,
}

impl Default for HatchManifest {
    fn default() -> Self {
        Self {
            name: "my_flutter_app".to_string(),
            description: Some("A new Flutter app".to_string()),
            version: Some("1.0.0".to_string()),
            sdk: SdkConstraints {
                flutter: Some("3.24.2".to_string()),
                dart: Some(">=3.5.0 <4.0.0".to_string()),
            },
            require: Some(HashMap::new()),
            require_dev: Some(HashMap::new()),
            profiles: None,
            submodules: None,
            repositories: None,
            nests: None,
            build: None,
            scripts: None,
            overrides: None,
            disable_pub: None,
            prefer_newest_from: None,
            local_packages: None,
        }
    }
}

impl HatchManifest {
    /// Get all dependencies for a specific profile
    pub fn get_dependencies(&self, profile_name: Option<&str>) -> HashMap<String, String> {
        use crate::resolver::dependency_utils::DependencyUtils;
        let mut deps = HashMap::new();

        // Add base dependencies
        if let Some(require) = &self.require {
            deps.extend(DependencyUtils::to_simple_deps(require));
        }

        // Add profile-specific dependencies
        if let Some(profile_name) = profile_name {
            if let Some(profiles) = &self.profiles {
                if let Some(profile) = profiles.get(profile_name) {
                    if let Some(profile_deps) = &profile.require {
                        deps.extend(DependencyUtils::to_simple_deps(profile_deps));
                    }
                }
            }
        }

        deps
    }

    /// Get all dev dependencies for a specific profile
    pub fn get_dev_dependencies(&self, profile_name: Option<&str>) -> HashMap<String, String> {
        use crate::resolver::dependency_utils::DependencyUtils;
        let mut deps = HashMap::new();

        // Add base dev dependencies
        if let Some(require_dev) = &self.require_dev {
            deps.extend(DependencyUtils::to_simple_deps(require_dev));
        }

        // Add profile-specific dev dependencies
        if let Some(profile_name) = profile_name {
            if let Some(profiles) = &self.profiles {
                if let Some(profile) = profiles.get(profile_name) {
                    if let Some(profile_dev_deps) = &profile.require_dev {
                        deps.extend(DependencyUtils::to_simple_deps(profile_dev_deps));
                    }
                }
            }
        }

        deps
    }

    /// Get scripts for a specific profile
    pub fn get_scripts(&self, profile_name: Option<&str>) -> HashMap<String, serde_json::Value> {
        let mut scripts = HashMap::new();

        // Add base scripts
        if let Some(base_scripts) = &self.scripts {
            scripts.extend(base_scripts.clone());
        }

        // Add profile-specific scripts
        if let Some(profile_name) = profile_name {
            if let Some(profiles) = &self.profiles {
                if let Some(profile) = profiles.get(profile_name) {
                    if let Some(profile_scripts) = &profile.scripts {
                        scripts.extend(profile_scripts.clone());
                    }
                }
            }
        }

        scripts
    }

    /// Validate the manifest structure
    pub fn validate(&self) -> anyhow::Result<()> {
        use anyhow::{anyhow, Context};

        // Validate name
        if self.name.trim().is_empty() {
            return Err(anyhow!("Project name cannot be empty"));
        }

        // Validate Flutter version format
        if let Some(flutter) = &self.sdk.flutter {
            if flutter.trim().is_empty() {
                return Err(anyhow!("Flutter SDK version cannot be empty"));
            }
        } else {
            return Err(anyhow!("Flutter SDK version is required"));
        }

        // Validate Dart version format
        if let Some(dart) = &self.sdk.dart {
            if dart.trim().is_empty() {
                return Err(anyhow!("Dart SDK version cannot be empty"));
            }
        } else {
            return Err(anyhow!("Dart SDK version is required"));
        }

        // Validate repository configurations
        if let Some(repositories) = &self.repositories {
            for repo in repositories {
                match repo.repo_type.as_str() {
                    "pub" | "hatch" | "path" => {},
                    _ => return Err(anyhow!("Invalid repository type: {}", repo.repo_type)),
                }

                if repo.url.trim().is_empty() {
                    return Err(anyhow!("Repository URL cannot be empty"));
                }
            }
        }

        Ok(())
    }
}