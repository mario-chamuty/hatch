use anyhow::{anyhow, Result};
use regex::Regex;
use super::schema::HatchManifest;

/// Manifest validation utilities
pub struct ManifestValidator;

impl ManifestValidator {
    /// Validate a complete manifest
    pub fn validate(manifest: &HatchManifest) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        // Validate project name
        Self::validate_project_name(&manifest.name)?;

        // Validate SDK constraints
        if let Some(flutter) = &manifest.sdk.flutter {
            Self::validate_flutter_version(flutter)?;
        }
        if let Some(dart) = &manifest.sdk.dart {
            Self::validate_dart_version(dart)?;
        }

        // Validate dependencies
        if let Some(deps) = &manifest.require {
            for (name, constraint) in deps {
                Self::validate_package_name(name)?;
                Self::validate_version_constraint(constraint)?;
            }
        }

        if let Some(dev_deps) = &manifest.require_dev {
            for (name, constraint) in dev_deps {
                Self::validate_package_name(name)?;
                Self::validate_version_constraint(constraint)?;
            }
        }

        // Check for common warnings
        if manifest.description.is_none() {
            warnings.push("No description provided".to_string());
        }

        if manifest.version.is_none() {
            warnings.push("No version specified".to_string());
        }

        Ok(warnings)
    }

    /// Validate project name format
    pub fn validate_project_name(name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(anyhow!("Project name cannot be empty"));
        }

        let re = Regex::new(r"^[a-z][a-z0-9_]*$").unwrap();
        if !re.is_match(name) {
            return Err(anyhow!(
                "Invalid project name '{}'. Must be lowercase, start with letter, contain only letters, numbers, and underscores",
                name
            ));
        }

        if name.len() > 50 {
            return Err(anyhow!("Project name too long (max 50 characters)"));
        }

        Ok(())
    }

    /// Validate package name format
    pub fn validate_package_name(name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(anyhow!("Package name cannot be empty"));
        }

        // Allow Flutter SDK references
        if name == "flutter" || name == "flutter_test" {
            return Ok(());
        }

        let re = Regex::new(r"^[a-z][a-z0-9_]*$").unwrap();
        if !re.is_match(name) {
            return Err(anyhow!(
                "Invalid package name '{}'. Must be lowercase, start with letter, contain only letters, numbers, and underscores",
                name
            ));
        }

        Ok(())
    }

    /// Validate Flutter version constraint
    pub fn validate_flutter_version(version: &str) -> Result<()> {
        if version.trim().is_empty() {
            return Err(anyhow!("Flutter version cannot be empty"));
        }

        // Basic semantic version pattern
        let re = Regex::new(r"^\d+\.\d+\.\d+$").unwrap();
        if !re.is_match(version) {
            return Err(anyhow!(
                "Invalid Flutter version '{}'. Expected format: X.Y.Z (e.g., 3.24.2)",
                version
            ));
        }

        Ok(())
    }

    /// Validate Dart version constraint
    pub fn validate_dart_version(constraint: &str) -> Result<()> {
        if constraint.trim().is_empty() {
            return Err(anyhow!("Dart version constraint cannot be empty"));
        }

        // Basic validation - should support ranges like ">=3.5.0 <4.0.0"
        let re = Regex::new(r"^[>=<\s\d\.]+$").unwrap();
        if !re.is_match(constraint) {
            return Err(anyhow!(
                "Invalid Dart version constraint '{}'. Expected format: >=X.Y.Z <A.B.C",
                constraint
            ));
        }

        Ok(())
    }

    /// Validate package version constraint
    pub fn validate_version_constraint(constraint: &str) -> Result<()> {
        if constraint.trim().is_empty() {
            return Err(anyhow!("Version constraint cannot be empty"));
        }

        // Handle Flutter SDK reference
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(constraint) {
            if let Some(obj) = value.as_object() {
                if obj.contains_key("sdk") {
                    return Ok(());
                }
            }
        }

        // Basic semantic version constraint validation
        let re = Regex::new(r"^[\^~>=<\s\d\.]+$").unwrap();
        if !re.is_match(constraint) {
            return Err(anyhow!(
                "Invalid version constraint '{}'. Expected format: ^X.Y.Z, ~X.Y.Z, >=X.Y.Z, etc.",
                constraint
            ));
        }

        Ok(())
    }

    /// Check for dependency conflicts
    pub fn check_conflicts(manifest: &HatchManifest) -> Vec<String> {
        let mut conflicts = Vec::new();

        if let (Some(deps), Some(dev_deps)) = (&manifest.require, &manifest.require_dev) {
            for (name, version) in deps {
                if let Some(dev_version) = dev_deps.get(name) {
                    if version != dev_version {
                        conflicts.push(format!(
                            "Conflicting versions for '{}': require={}, require-dev={}",
                            name, version, dev_version
                        ));
                    }
                }
            }
        }

        conflicts
    }
}