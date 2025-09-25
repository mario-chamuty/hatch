use anyhow::{anyhow, Context, Result};
use std::path::Path;
use regex::Regex;

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

    pub fn parse_json(content: &str) -> Result<HatchManifest> {
        // Strip // comments to support JSON with comments
        let content = Self::strip_json_comments(content);

        serde_json::from_str(&content)
            .with_context(|| "Failed to parse JSON manifest")
    }

    fn strip_json_comments(content: &str) -> String {
        let re = Regex::new(r"//[^
]*").unwrap();
        let content = re.replace_all(content, "");

        // Also remove /* */ style comments
        let re = Regex::new(r"/\*[^*]*\*+(?:[^/*][^*]*\*+)*/").unwrap();
        re.replace_all(&content, "").to_string()
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