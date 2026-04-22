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

    // Automatically run install after update
    println!("\n📦 Installing updated dependencies...");
    super::install_smart::execute(None).await?;

    Ok(())
}

async fn update_all_packages(
    manifest: &mut crate::manifest::schema::HatchManifest,
    registry: &PubDevRegistry,
) -> Result<()> {
    let mut updated = Vec::new();

    // Update regular dependencies
    if let Some(deps) = &mut manifest.require {
        for (name, dep) in deps.iter_mut() {
            // Skip local and git dependencies
            if dep.is_local() || dep.is_git() {
                continue;
            }

            let current_version = dep.version();
            if let Ok(new_version) = get_latest_matching_version(registry, name, current_version).await {
                if new_version != current_version {
                    println!("   {} {} → {}", name, current_version.yellow(), new_version.green());
                    // Update the version in the dependency
                    match dep {
                        crate::manifest::Dependency::Simple(ref mut v) => *v = new_version.clone(),
                        crate::manifest::Dependency::Complex(ref mut c) => c.version = new_version.clone(),
                    }
                    updated.push(name.clone());
                }
            }
        }
    }

    // Update dev dependencies
    if let Some(dev_deps) = &mut manifest.require_dev {
        for (name, dep) in dev_deps.iter_mut() {
            // Skip local and git dependencies
            if dep.is_local() || dep.is_git() {
                continue;
            }

            let current_version = dep.version();
            if let Ok(new_version) = get_latest_matching_version(registry, name, current_version).await {
                if new_version != current_version {
                    println!("   {} {} → {} (dev)", name, current_version.yellow(), new_version.green());
                    // Update the version in the dependency
                    match dep {
                        crate::manifest::Dependency::Simple(ref mut v) => *v = new_version.clone(),
                        crate::manifest::Dependency::Complex(ref mut c) => c.version = new_version.clone(),
                    }
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
            if let Some(dep) = deps.get_mut(&package) {
                // Skip local and git dependencies
                if dep.is_local() || dep.is_git() {
                    println!("   {} is a local/git dependency - skipping", package);
                    continue;
                }

                found = true;
                let current_version = dep.version();
                if let Ok(new_version) = get_latest_matching_version(registry, &package, current_version).await {
                    if new_version != current_version {
                        println!("   {} {} → {}", package, current_version.yellow(), new_version.green());
                        // Update the version in the dependency
                        match dep {
                            crate::manifest::Dependency::Simple(ref mut v) => *v = new_version.clone(),
                            crate::manifest::Dependency::Complex(ref mut c) => c.version = new_version.clone(),
                        }
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
                if let Some(dep) = dev_deps.get_mut(&package) {
                    // Skip local and git dependencies
                    if dep.is_local() || dep.is_git() {
                        println!("   {} is a local/git dependency - skipping", package);
                        continue;
                    }

                    found = true;
                    let current_version = dep.version();
                    if let Ok(new_version) = get_latest_matching_version(registry, &package, current_version).await {
                        if new_version != current_version {
                            println!("   {} {} → {} (dev)", package, current_version.yellow(), new_version.green());
                            // Update the version in the dependency
                            match dep {
                                crate::manifest::Dependency::Simple(ref mut v) => *v = new_version.clone(),
                                crate::manifest::Dependency::Complex(ref mut c) => c.version = new_version.clone(),
                            }
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