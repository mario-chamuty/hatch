use anyhow::{anyhow, Context, Result};
use std::path::Path;
use once_cell::sync::Lazy;
use regex::Regex;

/// Pre-compiled regex for block comment stripping (avoids recompilation on every call)
static BLOCK_COMMENT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"/\*[^*]*\*+(?:[^/*][^*]*\*+)*/").unwrap()
});

use super::schema::HatchManifest;

pub struct ManifestParser;

impl ManifestParser {
    pub fn parse_from_file<P: AsRef<Path>>(path: P) -> Result<HatchManifest> {
        let path = path.as_ref();

        // Only support hatch.json
        if !path.file_name().map_or(false, |n| n == "hatch.json") {
            return Err(anyhow!("Manifest must be named 'hatch.json'. YAML format is no longer supported."));
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read manifest file: {}", path.display()))?;

        Self::parse_json(&content)
    }

    /// Parse a JSON file directly (for dependency command discovery)
    pub fn parse_json_file<P: AsRef<Path>>(path: P) -> Result<HatchManifest> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read file: {}", path.display()))?;
        Self::parse_json(&content)
    }

    /// Parse a YAML file directly (for backward compatibility in dependencies)
    pub fn parse_yaml_file<P: AsRef<Path>>(path: P) -> Result<HatchManifest> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read file: {}", path.display()))?;

        // Parse as YAML and convert to HatchManifest
        serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse YAML file: {}", path.display()))
    }

    pub fn parse_json(content: &str) -> Result<HatchManifest> {
        // Strip // comments to support JSON with comments
        let content = Self::strip_json_comments(content);

        // Parse JSON
        let mut value: serde_json::Value = serde_json::from_str(&content)
            .with_context(|| "Failed to parse JSON manifest")?;

        // Expand environment variables
        crate::utils::env::expand_json_env_vars(&mut value);

        // Convert to manifest
        serde_json::from_value(value)
            .with_context(|| "Failed to deserialize manifest after env var expansion")
    }

    fn strip_json_comments(content: &str) -> String {
        // Process line by line to avoid matching URLs
        let lines: Vec<String> = content.lines().map(|line| {
            // Find // but not in strings or URLs
            if let Some(pos) = line.find("//") {
                // Check if this is likely a URL (preceded by : or /)
                if pos > 0 {
                    let prev_char = line.chars().nth(pos - 1);
                    if prev_char == Some(':') || prev_char == Some('/') {
                        // It's likely a URL, keep the whole line
                        line.to_string()
                    } else {
                        // It's a comment, strip from // onwards
                        line[..pos].to_string()
                    }
                } else {
                    // Comment at start of line
                    String::new()
                }
            } else {
                line.to_string()
            }
        }).collect();

        let content = lines.join("\n");

        // Also remove /* */ style comments (using pre-compiled regex)
        BLOCK_COMMENT_RE.replace_all(&content, "").to_string()
    }

    pub fn parse_with_overrides<P: AsRef<Path>>(
        manifest_path: P,
        local_override_path: Option<P>,
    ) -> Result<HatchManifest> {
        let mut manifest = Self::parse_from_file(manifest_path)?;

        // Apply local overrides if they exist
        if let Some(override_path) = local_override_path {
            let override_path = override_path.as_ref();
            if override_path.exists() {
                let override_manifest = Self::parse_from_file(override_path)
                    .with_context(|| "Failed to parse local override manifest")?;
                manifest = Self::merge_manifests(manifest, override_manifest);
            }
        }

        // Validate the final manifest
        manifest.validate()?;

        Ok(manifest)
    }

    /// Merge base manifest with override manifest
    pub fn merge_manifests(mut base: HatchManifest, override_manifest: HatchManifest) -> HatchManifest {
        // Override basic fields
        if override_manifest.description.is_some() {
            base.description = override_manifest.description;
        }
        if override_manifest.version.is_some() {
            base.version = override_manifest.version;
        }

        // Merge dependencies
        if let Some(override_deps) = override_manifest.require {
            if let Some(ref mut base_deps) = base.require {
                base_deps.extend(override_deps);
            } else {
                base.require = Some(override_deps);
            }
        }

        // Merge dev dependencies
        if let Some(override_dev_deps) = override_manifest.require_dev {
            if let Some(ref mut base_dev_deps) = base.require_dev {
                base_dev_deps.extend(override_dev_deps);
            } else {
                base.require_dev = Some(override_dev_deps);
            }
        }

        // Merge repositories
        if let Some(override_repos) = override_manifest.repositories {
            if let Some(ref mut base_repos) = base.repositories {
                base_repos.extend(override_repos);
            } else {
                base.repositories = Some(override_repos);
            }
        }

        // Merge profiles
        if let Some(override_profiles) = override_manifest.profiles {
            if let Some(ref mut base_profiles) = base.profiles {
                base_profiles.extend(override_profiles);
            } else {
                base.profiles = Some(override_profiles);
            }
        }

        // Merge scripts
        if let Some(override_scripts) = override_manifest.scripts {
            if let Some(ref mut base_scripts) = base.scripts {
                base_scripts.extend(override_scripts);
            } else {
                base.scripts = Some(override_scripts);
            }
        }

        // Override build config entirely
        if override_manifest.build.is_some() {
            base.build = override_manifest.build;
        }

        base
    }

    pub fn create_default_json() -> String {
        r#"{
  "name": "my_flutter_app",
  "description": "A new Flutter app",
  "version": "1.0.0",
  "sdk": {
    "flutter": "3.24.2",
    "dart": ">=3.5.0 <4.0.0"
  },
  // Core dependencies
  "require": {
    // Add your dependencies here
  },

  // Development dependencies
  "require-dev": {
    // Add dev dependencies here
  }
}"#.to_string()
    }
}