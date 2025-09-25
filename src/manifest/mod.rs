pub mod parser;
pub mod schema;
pub mod validator;
pub mod merger;

use anyhow::Result;
use std::path::PathBuf;

pub use parser::ManifestParser;
pub use schema::HatchManifest;
pub use validator::ManifestValidator;
pub use merger::ManifestMerger;

use crate::config::paths::PathResolver;

/// Manifest management and parsing
pub struct ManifestManager {
    path_resolver: PathResolver,
}

impl ManifestManager {
    pub fn new(project_dir: PathBuf) -> Self {
        Self {
            path_resolver: PathResolver::new(project_dir),
        }
    }

    /// Parse and load project manifest with profile and overrides
    pub fn load_manifest(&self, profile: Option<&str>) -> Result<HatchManifest> {
        // Find the main manifest file
        let manifest_path = self.path_resolver.find_manifest()
            .ok_or_else(|| anyhow::anyhow!("No hatch.yaml or hatch.json found"))?;

        // Check for local overrides
        let yaml_override = self.path_resolver.local_manifest_yaml_path();
        let json_override = self.path_resolver.local_manifest_json_path();

        let override_path = if yaml_override.exists() {
            Some(yaml_override)
        } else if json_override.exists() {
            Some(json_override)
        } else {
            None
        };

        // Parse manifest with overrides
        let mut manifest = ManifestParser::parse_with_overrides(manifest_path, override_path)?;

        // Apply profile if specified
        if let Some(profile_name) = profile {
            if ManifestMerger::has_profile(&manifest, profile_name) {
                manifest = ManifestMerger::merge_profile(manifest, profile_name);
            } else {
                return Err(anyhow::anyhow!("Profile '{}' not found", profile_name));
            }
        }

        Ok(manifest)
    }

    /// Create a new manifest file
    pub fn create_manifest(&self, name: &str, format: ManifestFormat) -> Result<()> {
        let content = match format {
            ManifestFormat::Yaml => {
                let mut template = ManifestParser::create_default_yaml();
                template = template.replace("my_flutter_app", name);
                template
            },
            ManifestFormat::Json => {
                let mut template = ManifestParser::create_default_json();
                template = template.replace("my_flutter_app", name);
                template
            }
        };

        let path = match format {
            ManifestFormat::Yaml => self.path_resolver.manifest_yaml_path(),
            ManifestFormat::Json => self.path_resolver.manifest_json_path(),
        };

        std::fs::write(&path, content)?;
        println!("✅ Created manifest: {}", path.display());

        Ok(())
    }

    /// Validate current manifest
    pub fn validate_manifest(&self, profile: Option<&str>) -> Result<Vec<String>> {
        let manifest = self.load_manifest(profile)?;
        ManifestValidator::validate(&manifest)
    }
}

#[derive(Debug, Clone)]
pub enum ManifestFormat {
    Yaml,
    Json,
}