use anyhow::{anyhow, Result};
use log::info;
use std::collections::HashMap;

use crate::manifest::parser::ManifestParser;
use crate::registry::pub_dev::PubDevRegistry;
use crate::registry::traits::Registry;
use crate::resolver::graph::DependencyGraph;

pub async fn execute(package: String) -> Result<()> {
    info!("Explaining dependency: {}", package);

    println!("🔍 Why is '{}' installed?", package);

    // Step 1: Load manifest
    let manifest = ManifestParser::parse_with_overrides(
        "hatch.yaml",
        Some("hatch.local.yaml"),
    ).or_else(|_| {
        ManifestParser::parse_with_overrides(
            "hatch.json",
            Some("hatch.local.json"),
        )
    })?;

    // Step 2: Check if it's a direct dependency
    let mut found_direct = false;
    let mut constraint = String::new();

    if let Some(deps) = &manifest.require {
        if let Some(c) = deps.get(&package) {
            println!("📦 '{}' is a direct dependency", package);
            println!("   Constraint: {}", c);
            found_direct = true;
            constraint = c.clone();
        }
    }

    if let Some(dev_deps) = &manifest.require_dev {
        if let Some(c) = dev_deps.get(&package) {
            println!("🔧 '{}' is a dev dependency", package);
            println!("   Constraint: {}", c);
            found_direct = true;
            constraint = c.clone();
        }
    }

    if !found_direct {
        println!("🔍 Searching for '{}' as a transitive dependency...", package);

        // Step 3: Build dependency graph to trace paths
        let registry = PubDevRegistry::new();
        let mut dependencies = HashMap::new();

        // Collect all direct dependencies
        if let Some(deps) = &manifest.require {
            dependencies.extend(deps.clone());
        }
        if let Some(dev_deps) = &manifest.require_dev {
            dependencies.extend(dev_deps.clone());
        }

        // Build minimal graph to find paths
        let mut available_versions = HashMap::new();
        for (dep_name, _) in &dependencies {
            if dep_name == "flutter" {
                continue;
            }

            match registry.get_package_metadata(dep_name).await {
                Ok(metadata) => {
                    available_versions.insert(dep_name.clone(), metadata.versions);
                }
                Err(e) => {
                    println!("   ⚠️ Failed to fetch {}: {}", dep_name, e);
                }
            }
        }

        // Use dependency graph to find path
        let graph = DependencyGraph::new();
        let root_packages: Vec<String> = dependencies.keys().cloned().collect();

        if let Some(path) = graph.find_dependency_path(&package, &root_packages) {
            println!("📈 Found dependency path:");
            for (i, step) in path.iter().enumerate() {
                if i == 0 {
                    println!("   {} (root)", step);
                } else {
                    println!("   └── {}", step);
                }
            }
        } else {
            println!("❓ Package '{}' not found in dependency tree", package);
            println!("   This could mean:");
            println!("   • It's not actually installed");
            println!("   • It's a system/SDK dependency");
            println!("   • The dependency graph is incomplete");
        }
    }

    // Step 4: Show additional package info if available
    let registry = PubDevRegistry::new();
    match registry.get_package_metadata(&package).await {
        Ok(metadata) => {
            println!("\n📋 Package information:");
            println!("   Latest version: {}", metadata.latest.version);
            if let Some(description) = &metadata.description {
                println!("   Description: {}", description);
            }
            if !metadata.latest.dependencies.is_empty() {
                println!("   Dependencies: {}", metadata.latest.dependencies.len());
            }
        }
        Err(e) => {
            println!("   ⚠️ Could not fetch package info: {}", e);
        }
    }

    Ok(())
}