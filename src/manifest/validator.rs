use anyhow::{anyhow, Result};
use regex::Regex;
use std::collections::HashMap;
use super::schema::HatchManifest;
use super::Dependency;

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

        // Collect available nest names
        let available_nests: Vec<String> = manifest.nests
            .as_ref()
            .map(|nests| nests.iter().map(|n| n.name.clone()).collect())
            .unwrap_or_default();

        // Validate dependencies
        if let Some(deps) = &manifest.require {
            Self::validate_dependency_map(deps, &available_nests)?;
        }

        if let Some(dev_deps) = &manifest.require_dev {
            Self::validate_dependency_map(dev_deps, &available_nests)?;
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

        // Handle special cases
        if constraint == "any" || constraint == "*" {
            return Ok(());
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
        // Allow digits, dots, operators, spaces, wildcards, and pre-release/build suffixes (-alpha, +build)
        let re = Regex::new(r"^[\^~>=<\s\d\.\*a-zA-Z\-\+]+$").unwrap();
        if !re.is_match(constraint) {
            return Err(anyhow!(
                "Invalid version constraint '{}'. Expected format: ^X.Y.Z, ~X.Y.Z, >=X.Y.Z, any, *, etc.",
                constraint
            ));
        }

        Ok(())
    }

    /// Validate a map of dependencies
    fn validate_dependency_map(
        deps: &HashMap<String, Dependency>,
        available_nests: &[String],
    ) -> Result<()> {
        for (name, dep) in deps {
            Self::validate_package_name(name)?;

            // Skip version constraint validation for non-registry deps (git, path, sdk)
            if !dep.is_git() && !dep.is_local() && !dep.is_sdk() {
                Self::validate_version_constraint(dep.version())?;
            }

            // Validate dependency sources for conflicts
            if let Err(e) = dep.validate(name) {
                return Err(anyhow!(e));
            }

            // Validate nest references
            if let Dependency::Complex(complex) = dep {
                if let Err(e) = complex.validate_nest(name, available_nests) {
                    return Err(anyhow!(e));
                }
            }
        }
        Ok(())
    }

    /// Check for dependency conflicts
    pub fn check_conflicts(manifest: &HatchManifest) -> Vec<String> {
        let mut conflicts = Vec::new();

        if let (Some(deps), Some(dev_deps)) = (&manifest.require, &manifest.require_dev) {
            for (name, dep) in deps {
                if let Some(dev_dep) = dev_deps.get(name) {
                    if dep.version() != dev_dep.version() {
                        conflicts.push(format!(
                            "Conflicting versions for '{}': require={}, require-dev={}",
                            name, dep.version(), dev_dep.version()
                        ));
                    }
                }
            }
        }

        conflicts
    }
}