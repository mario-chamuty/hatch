use anyhow::{anyhow, Result};
use serde_yaml::{Value, Mapping};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use log::{debug, info};

use crate::manifest::schema::HatchManifest;
use crate::resolver::sat::ResolvedPackage;

/// Generates pubspec.yaml from Hatch manifest and resolved dependencies
pub struct PubspecGenerator;

impl PubspecGenerator {
    /// Generate pubspec.yaml from manifest and resolved dependencies
    pub fn generate(
        manifest: &HatchManifest,
        resolved_packages: &[ResolvedPackage],
        output_path: &Path,
    ) -> Result<()> {
        info!("Generating pubspec.yaml from manifest and resolved dependencies");

        let mut pubspec = BTreeMap::new();

        // Basic project information
        pubspec.insert("name".to_string(), Value::String(manifest.name.clone()));

        if let Some(description) = &manifest.description {
            pubspec.insert("description".to_string(), Value::String(description.clone()));
        }

        if let Some(version) = &manifest.version {
            pubspec.insert("version".to_string(), Value::String(version.clone()));
        }

        // Environment (SDK constraints)
        let mut environment = BTreeMap::new();
        if let Some(dart_constraint) = &manifest.sdk.dart {
            environment.insert("sdk".to_string(), Value::String(dart_constraint.clone()));
        }
        if let Some(flutter_constraint) = &manifest.sdk.flutter {
            environment.insert("flutter".to_string(), Value::String(flutter_constraint.clone()));
        }

        if !environment.is_empty() {
            let env_mapping = environment.into_iter()
                .map(|(k, v)| (Value::String(k), v))
                .collect::<Mapping>();
            pubspec.insert("environment".to_string(), Value::Mapping(env_mapping));
        }

        // Dependencies
        let dependencies = Self::build_dependencies_section(resolved_packages, false)?;
        if !dependencies.is_empty() {
            let deps_mapping = dependencies.into_iter()
                .map(|(k, v)| (Value::String(k), v))
                .collect::<Mapping>();
            pubspec.insert("dependencies".to_string(), Value::Mapping(deps_mapping));
        }

        // Dev dependencies
        let dev_dependencies = Self::build_dependencies_section(resolved_packages, true)?;
        if !dev_dependencies.is_empty() {
            let dev_deps_mapping = dev_dependencies.into_iter()
                .map(|(k, v)| (Value::String(k), v))
                .collect::<Mapping>();
            pubspec.insert("dev_dependencies".to_string(), Value::Mapping(dev_deps_mapping));
        }

        // Flutter section
        let flutter_section = Self::build_flutter_section(manifest)?;
        if !flutter_section.is_empty() {
            let flutter_mapping = flutter_section.into_iter()
                .map(|(k, v)| (Value::String(k), v))
                .collect::<Mapping>();
            pubspec.insert("flutter".to_string(), Value::Mapping(flutter_mapping));
        }

        // Convert to YAML and write to file
        let yaml_content = serde_yaml::to_string(&pubspec)
            .map_err(|e| anyhow!("Failed to serialize pubspec to YAML: {}", e))?;

        fs::write(output_path, yaml_content)
            .map_err(|e| anyhow!("Failed to write pubspec.yaml: {}", e))?;

        info!("Generated pubspec.yaml with {} dependencies", resolved_packages.len());
        Ok(())
    }

    /// Generate pubspec.lock from resolved dependencies
    pub fn generate_lock_file(
        resolved_packages: &[ResolvedPackage],
        output_path: &Path,
    ) -> Result<()> {
        info!("Generating pubspec.lock from resolved dependencies");

        let mut lock_file = BTreeMap::new();
        lock_file.insert("sdks".to_string(), Self::build_sdks_section()?);

        let mut packages = BTreeMap::new();
        for package in resolved_packages {
            let mut package_info = BTreeMap::new();

            package_info.insert("dependency".to_string(), Value::String("direct main".to_string()));

            let mut description = BTreeMap::new();
            description.insert("name".to_string(), Value::String(package.name.clone()));
            description.insert("sha256".to_string(), Value::String("placeholder".to_string()));
            description.insert("url".to_string(), Value::String("https://pub.dev".to_string()));

            let desc_mapping = description.into_iter()
                .map(|(k, v)| (Value::String(k), v))
                .collect::<Mapping>();
            package_info.insert("description".to_string(), Value::Mapping(desc_mapping));
            package_info.insert("source".to_string(), Value::String("hosted".to_string()));
            package_info.insert("version".to_string(), Value::String(package.version.clone()));

            let pkg_info_mapping = package_info.into_iter()
                .map(|(k, v)| (Value::String(k), v))
                .collect::<Mapping>();
            packages.insert(package.name.clone(), Value::Mapping(pkg_info_mapping));
        }

        let packages_mapping = packages.into_iter()
            .map(|(k, v)| (Value::String(k), v))
            .collect::<Mapping>();
        lock_file.insert("packages".to_string(), Value::Mapping(packages_mapping));

        let yaml_content = serde_yaml::to_string(&lock_file)
            .map_err(|e| anyhow!("Failed to serialize pubspec.lock to YAML: {}", e))?;

        fs::write(output_path, yaml_content)
            .map_err(|e| anyhow!("Failed to write pubspec.lock: {}", e))?;

        info!("Generated pubspec.lock with {} packages", resolved_packages.len());
        Ok(())
    }

    fn build_dependencies_section(
        resolved_packages: &[ResolvedPackage],
        dev_only: bool,
    ) -> Result<BTreeMap<String, Value>> {
        let mut dependencies = BTreeMap::new();

        for package in resolved_packages {
            // For now, treat all dependencies as regular dependencies
            // In a real implementation, we'd need to track which are dev dependencies
            if !dev_only {
                dependencies.insert(
                    package.name.clone(),
                    Value::String(format!("^{}", package.version))
                );
            }
        }

        // Add Flutter SDK dependencies
        if !dev_only {
            let mut flutter_dep = BTreeMap::new();
            flutter_dep.insert("sdk".to_string(), Value::String("flutter".to_string()));
            let flutter_mapping = flutter_dep.into_iter()
                .map(|(k, v)| (Value::String(k), v))
                .collect::<Mapping>();
            dependencies.insert("flutter".to_string(), Value::Mapping(flutter_mapping));
        }

        Ok(dependencies)
    }

    fn build_flutter_section(manifest: &HatchManifest) -> Result<BTreeMap<String, Value>> {
        let mut flutter_section = BTreeMap::new();

        // Uses material design
        flutter_section.insert("uses-material-design".to_string(), Value::Bool(true));

        // Assets (placeholder - would be configured in manifest)
        // flutter_section.insert("assets".to_string(), Value::Sequence(vec![]));

        Ok(flutter_section)
    }

    fn build_sdks_section() -> Result<Value> {
        let mut sdks = BTreeMap::new();
        sdks.insert("dart".to_string(), Value::String(">=3.5.0 <4.0.0".to_string()));
        sdks.insert("flutter".to_string(), Value::String("3.24.2".to_string()));
        let sdks_mapping = sdks.into_iter()
            .map(|(k, v)| (Value::String(k), v))
            .collect::<Mapping>();
        Ok(Value::Mapping(sdks_mapping))
    }

    /// Update an existing pubspec.yaml with resolved dependencies
    pub fn update_existing_pubspec(
        pubspec_path: &Path,
        resolved_packages: &[ResolvedPackage],
    ) -> Result<()> {
        if !pubspec_path.exists() {
            return Err(anyhow!("pubspec.yaml does not exist at {}", pubspec_path.display()));
        }

        let content = fs::read_to_string(pubspec_path)
            .map_err(|e| anyhow!("Failed to read pubspec.yaml: {}", e))?;

        let mut pubspec: Value = serde_yaml::from_str(&content)
            .map_err(|e| anyhow!("Failed to parse existing pubspec.yaml: {}", e))?;

        // Update dependencies section
        if let Some(deps) = pubspec.get_mut("dependencies") {
            if let Some(deps_map) = deps.as_mapping_mut() {
                for package in resolved_packages {
                    // Only update if the package already exists or is a new dependency
                    deps_map.insert(
                        Value::String(package.name.clone()),
                        Value::String(format!("^{}", package.version))
                    );
                }
            }
        }

        // Write updated pubspec back
        let updated_content = serde_yaml::to_string(&pubspec)
            .map_err(|e| anyhow!("Failed to serialize updated pubspec: {}", e))?;

        fs::write(pubspec_path, updated_content)
            .map_err(|e| anyhow!("Failed to write updated pubspec.yaml: {}", e))?;

        info!("Updated existing pubspec.yaml with {} dependencies", resolved_packages.len());
        Ok(())
    }

    /// Validate that generated pubspec.yaml is valid
    pub fn validate_pubspec(pubspec_path: &Path) -> Result<()> {
        let content = fs::read_to_string(pubspec_path)
            .map_err(|e| anyhow!("Failed to read pubspec.yaml: {}", e))?;

        let pubspec: Value = serde_yaml::from_str(&content)
            .map_err(|e| anyhow!("Invalid YAML in pubspec.yaml: {}", e))?;

        // Check required fields
        if !pubspec.get("name").is_some() {
            return Err(anyhow!("pubspec.yaml is missing 'name' field"));
        }

        if !pubspec.get("environment").is_some() {
            return Err(anyhow!("pubspec.yaml is missing 'environment' field"));
        }

        debug!("pubspec.yaml validation passed");
        Ok(())
    }

    /// Extract dependency information from existing pubspec.yaml
    pub fn parse_existing_dependencies(pubspec_path: &Path) -> Result<HashMap<String, String>> {
        let content = fs::read_to_string(pubspec_path)
            .map_err(|e| anyhow!("Failed to read pubspec.yaml: {}", e))?;

        let pubspec: Value = serde_yaml::from_str(&content)
            .map_err(|e| anyhow!("Failed to parse pubspec.yaml: {}", e))?;

        let mut dependencies = HashMap::new();

        // Extract regular dependencies
        if let Some(deps) = pubspec.get("dependencies") {
            if let Some(deps_map) = deps.as_mapping() {
                for (key, value) in deps_map {
                    if let (Some(name), Some(constraint)) = (key.as_str(), value.as_str()) {
                        dependencies.insert(name.to_string(), constraint.to_string());
                    }
                }
            }
        }

        // Extract dev dependencies
        if let Some(dev_deps) = pubspec.get("dev_dependencies") {
            if let Some(dev_deps_map) = dev_deps.as_mapping() {
                for (key, value) in dev_deps_map {
                    if let (Some(name), Some(constraint)) = (key.as_str(), value.as_str()) {
                        dependencies.insert(format!("dev:{}", name), constraint.to_string());
                    }
                }
            }
        }

        Ok(dependencies)
    }
}