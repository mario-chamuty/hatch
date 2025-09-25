use anyhow::{anyhow, Result};
use log::info;
use std::collections::HashMap;

use crate::manifest::parser::ManifestParser;
use crate::registry::pub_dev::PubDevRegistry;
use crate::registry::traits::Registry;

pub async fn execute(package: String, version: Option<String>, dev: bool) -> Result<()> {
    info!("Adding dependency: {}", package);

    let dep_type = if dev { "dev" } else { "regular" };
    let version_str = version.as_deref().unwrap_or("latest");

    println!("📎 Adding {} dependency '{}@{}'", dep_type, package, version_str);

    let mut manifest = ManifestParser::parse_with_overrides(
        "hatch.json",
        Some("hatch.local.json"),
    )?;

    let registry = PubDevRegistry::new();
    let package_exists = registry.package_exists(&package).await?;

    if !package_exists {
        return Err(anyhow!("Package '{}' not found on pub.dev", package));
    }

    let constraint = if let Some(ver) = version {
        if ver == "latest" {
            match registry.get_package_metadata(&package).await {
                Ok(metadata) => format!("^{}", metadata.latest.version),
                Err(_) => "^1.0.0".to_string(),
            }
        } else {
            format!("^{}", ver)
        }
    } else {
        match registry.get_package_metadata(&package).await {
            Ok(metadata) => format!("^{}", metadata.latest.version),
            Err(_) => "^1.0.0".to_string(), // Fallback
        }
    };

    if dev {
        if manifest.require_dev.is_none() {
            manifest.require_dev = Some(HashMap::new());
        }
        manifest.require_dev.as_mut().unwrap().insert(package.clone(), constraint);
    } else {
        if manifest.require.is_none() {
            manifest.require = Some(HashMap::new());
        }
        manifest.require.as_mut().unwrap().insert(package.clone(), constraint);
    }

    let manifest_content = serde_json::to_string_pretty(&manifest)?;
    std::fs::write("hatch.json", manifest_content)?;

    println!("✅ Dependency '{}' added successfully", package);

    Ok(())
}