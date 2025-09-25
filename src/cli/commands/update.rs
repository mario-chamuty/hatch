use anyhow::{Context, Result};
use colored::Colorize;
use std::collections::HashMap;
use log::info;

use crate::manifest::parser::ManifestParser;
use crate::registry::pub_dev::PubDevRegistry;
use crate::registry::traits::Registry;

pub async fn execute(packages: Vec<String>) -> Result<()> {
    info!("Updating dependencies");

    let mut manifest = ManifestParser::parse_with_overrides("hatch.json", Some("hatch.local.json"))?;
    let registry = PubDevRegistry::new();

    if packages.is_empty() {
        println!("🔄 {} all dependencies", "Updating".cyan().bold());
        update_all_packages(&mut manifest, &registry).await?;
    } else {
        println!("🔄 {} specific packages: {}", "Updating".cyan().bold(), packages.join(", "));
        update_specific_packages(&mut manifest, &registry, packages).await?;
    }

    // Save updated manifest
    let manifest_content = serde_json::to_string_pretty(&manifest)?;
    std::fs::write("hatch.json", manifest_content)?;

    println!("\n✅ {} complete!", "Update".green().bold());
    println!("\n💡 Run {} to install the updated dependencies", "hatch install".cyan());

    Ok(())
}

async fn update_all_packages(
    manifest: &mut crate::manifest::schema::HatchManifest,
    registry: &PubDevRegistry,
) -> Result<()> {
    let mut updated = Vec::new();

    // Update regular dependencies
    if let Some(deps) = &mut manifest.require {
        for (name, constraint) in deps.iter_mut() {
            if let Ok(new_version) = get_latest_matching_version(registry, name, constraint).await {
                if new_version != *constraint {
                    println!("   {} {} → {}", name, constraint.yellow(), new_version.green());
                    *constraint = new_version.clone();
                    updated.push(name.clone());
                }
            }
        }
    }

    // Update dev dependencies
    if let Some(dev_deps) = &mut manifest.require_dev {
        for (name, constraint) in dev_deps.iter_mut() {
            if let Ok(new_version) = get_latest_matching_version(registry, name, constraint).await {
                if new_version != *constraint {
                    println!("   {} {} → {} (dev)", name, constraint.yellow(), new_version.green());
                    *constraint = new_version.clone();
                    updated.push(name.clone());
                }
            }
        }
    }

    if updated.is_empty() {
        println!("   All packages are already up to date");
    } else {
        println!("   Updated {} packages", updated.len());
    }

    Ok(())
}

async fn update_specific_packages(
    manifest: &mut crate::manifest::schema::HatchManifest,
    registry: &PubDevRegistry,
    packages: Vec<String>,
) -> Result<()> {
    let mut updated = Vec::new();
    let mut not_found = Vec::new();

    for package in packages {
        let mut found = false;

        // Check regular dependencies
        if let Some(deps) = &mut manifest.require {
            if let Some(constraint) = deps.get_mut(&package) {
                found = true;
                if let Ok(new_version) = get_latest_matching_version(registry, &package, constraint).await {
                    if new_version != *constraint {
                        println!("   {} {} → {}", package, constraint.yellow(), new_version.green());
                        *constraint = new_version.clone();
                        updated.push(package.clone());
                    } else {
                        println!("   {} is already up to date", package);
                    }
                }
            }
        }

        // Check dev dependencies
        if !found {
            if let Some(dev_deps) = &mut manifest.require_dev {
                if let Some(constraint) = dev_deps.get_mut(&package) {
                    found = true;
                    if let Ok(new_version) = get_latest_matching_version(registry, &package, constraint).await {
                        if new_version != *constraint {
                            println!("   {} {} → {} (dev)", package, constraint.yellow(), new_version.green());
                            *constraint = new_version.clone();
                            updated.push(package.clone());
                        } else {
                            println!("   {} is already up to date", package);
                        }
                    }
                }
            }
        }

        if !found {
            not_found.push(package);
        }
    }

    if !not_found.is_empty() {
        println!("⚠️  Packages not found in manifest: {}", not_found.join(", ").yellow());
    }

    Ok(())
}

async fn get_latest_matching_version(
    registry: &PubDevRegistry,
    package: &str,
    current_constraint: &str,
) -> Result<String> {
    // Parse the current constraint to understand what type it is
    if current_constraint.starts_with("^") {
        // Caret constraint - get latest compatible version
        let base_version = current_constraint.trim_start_matches('^');
        let metadata = registry.get_package_metadata(package).await?;

        // Get latest version that's compatible
        let latest = &metadata.latest.version;
        Ok(format!("^{}", latest))
    } else if current_constraint.starts_with("~") {
        // Tilde constraint - get latest patch version
        let base_version = current_constraint.trim_start_matches('~');
        let metadata = registry.get_package_metadata(package).await?;

        // Get latest compatible patch version
        let latest = &metadata.latest.version;
        Ok(format!("~{}", latest))
    } else if current_constraint.contains("git:") || current_constraint.contains("path:") {
        // Git or path dependency - don't update
        Ok(current_constraint.to_string())
    } else {
        // Exact version or range - get latest version
        let metadata = registry.get_package_metadata(package).await?;
        let latest = &metadata.latest.version;

        // Keep the same constraint type if it's a range
        if current_constraint.contains('<') || current_constraint.contains('>') {
            Ok(current_constraint.to_string())
        } else {
            Ok(format!("^{}", latest))
        }
    }
}