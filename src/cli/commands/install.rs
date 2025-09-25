use anyhow::{anyhow, Result};
use log::{debug, info, warn};
use std::collections::{HashMap, HashSet};

use crate::manifest::parser::ManifestParser;
use crate::registry::pub_dev::PubDevRegistry;
use crate::registry::traits::{Registry, PackageVersion};
use crate::resolver::sat::SatSolver;
use crate::fvm::installer::FvmInstaller;
use crate::cache::manager::CacheManager;
use crate::lockfile::{LockfileParser, LockfileGenerator};
use crate::scripts::ScriptRunner;

pub async fn execute(profile: Option<String>) -> Result<()> {
    super::install_smart::execute(profile).await
}

pub async fn execute_with_options(profile: Option<String>, _relax_constraints: bool, _force: bool) -> Result<()> {
    super::install_smart::execute(profile).await
}

pub async fn _execute_with_options_old(profile: Option<String>, relax_constraints: bool, force: bool) -> Result<()> {
    let lockfile_path = std::path::Path::new("hatch.lock");
    if lockfile_path.exists() {
        if let Ok(lockfile) = LockfileParser::parse(lockfile_path) {
            // Quick check if we can use the lockfile
            let manifest = ManifestParser::parse_with_overrides(
                "hatch.yaml",
                Some("hatch.local.yaml"),
            ).or_else(|_| {
                ManifestParser::parse_with_overrides(
                    "hatch.json",
                    Some("hatch.local.json"),
                )
            })?;

            if lockfile.flutter_version == manifest.sdk.flutter &&
               lockfile.dart_version == manifest.sdk.dart {
                println!("📋 Using existing hatch.lock for faster install");
            }
        }
    }

    execute_full(profile, relax_constraints, force).await
}

pub async fn execute_full(profile: Option<String>, relax_constraints: bool, force: bool) -> Result<()> {
    info!("Installing dependencies");

    let profile_name = profile.unwrap_or_else(|| "default".to_string());
    println!("📦 Installing dependencies with profile '{}'", profile_name);

    // Step 1: Parse hatch.yaml/json manifest
    println!("📄 Parsing manifest...");
    let manifest = ManifestParser::parse_with_overrides(
        "hatch.yaml",
        Some("hatch.local.yaml"),
    ).or_else(|_| {
        ManifestParser::parse_with_overrides(
            "hatch.json",
            Some("hatch.local.json"),
        )
    }).map_err(|e| anyhow!("Failed to parse manifest: {}", e))?;

    ScriptRunner::run_pre_install(&manifest)?;
    ScriptRunner::run_profile_scripts(&manifest, &profile_name, "pre-install")?;

    if let Some(flutter_version) = &manifest.sdk.flutter {
        println!("🎯 Setting up Flutter {}...", flutter_version);

        match FvmInstaller::auto_install_from_manifest(flutter_version).await {
            Ok(true) => println!("✅ Flutter {} is ready", flutter_version),
            Ok(false) => warn!("⚠️ Flutter version management not available"),
            Err(e) => warn!("⚠️ Failed to setup Flutter {}: {}", flutter_version, e),
        }
    }

    let mut dependencies = HashMap::new();

    if let Some(require) = &manifest.require {
        dependencies.extend(require.clone());
    }

    if profile_name == "dev" || profile_name == "development" {
        if let Some(require_dev) = &manifest.require_dev {
            dependencies.extend(require_dev.clone());
        }
    }

    if let Some(profiles) = &manifest.profiles {
        if let Some(profile) = profiles.get(&profile_name) {
            if let Some(profile_deps) = &profile.require {
                dependencies.extend(profile_deps.clone());
            }
        }
    }

    if dependencies.is_empty() {
        println!("ℹ️ No dependencies to install");
        return Ok(());
    }

    println!("🔍 Resolving {} dependencies...", dependencies.len());

    let registry = PubDevRegistry::new();
    let mut available_versions = HashMap::new();
    let mut to_fetch = dependencies.keys().cloned().collect::<Vec<_>>();
    let mut fetched = std::collections::HashSet::new();

    let mut iteration = 0;
    while !to_fetch.is_empty() && iteration < 10 {
        iteration += 1;
        let current_batch = to_fetch.clone();
        to_fetch.clear();

        for dep_name in current_batch {
            if fetched.contains(&dep_name) || dep_name == "flutter" {
                continue;
            }
            fetched.insert(dep_name.clone());

            println!("   Fetching versions for {}", dep_name);

            match registry.get_package_metadata(&dep_name).await {
                Ok(metadata) => {
                    if let Some(latest_version) = metadata.versions.last() {
                        for (trans_dep, _) in &latest_version.dependencies {
                            if !fetched.contains(trans_dep) && trans_dep != "flutter" {
                                if !trans_dep.contains("test") && !trans_dep.contains("mock") {
                                    to_fetch.push(trans_dep.clone());
                                }
                            }
                        }
                    }
                    available_versions.insert(dep_name.clone(), metadata.versions);
                }
                Err(e) => {
                    if dependencies.contains_key(&dep_name) {
                        warn!("Failed to fetch required package {}: {}", dep_name, e);
                    } else {
                        debug!("Skipping optional/transitive package {}: {}", dep_name, e);
                    }
                }
            }
        }
    }

    println!("🧮 Solving dependency constraints...");

    for (name, versions) in &available_versions {
        if name == "path" || name == "collection" {
            println!("   DEBUG: Package {} has {} versions available", name, versions.len());
            if !versions.is_empty() {
                println!("      First version: {}", versions[0].version);
                println!("      Last version: {}", versions.last().unwrap().version);
            }
        }
    }

    let mut solver = SatSolver::new();

    let resolution_result = solver.resolve_dependencies(&dependencies, &available_versions)
        .map_err(|e| anyhow!("Dependency resolution failed: {}", e))?;

    if !resolution_result.conflicts.is_empty() {
        println!("❌ Dependency conflicts detected:");
        for conflict in &resolution_result.conflicts {
            println!("   {} - {}", conflict.package, conflict.reason);
        }
        return Err(anyhow!("Cannot resolve dependency conflicts"));
    }

    println!("✅ Resolved {} packages", resolution_result.resolved_packages.len());

    let lockfile_path = std::path::Path::new("hatch.lock");
    LockfileGenerator::generate(&manifest, &resolution_result.resolved_packages, lockfile_path)?;
    println!("🔒 Generated hatch.lock");

    println!("📥 Downloading packages...");
    let cache_manager = CacheManager::new();

    CacheManager::ensure_cache_dirs()?;

    let mut packages_to_download = Vec::new();
    let mut downloaded_packages = HashMap::new();

    for package in &resolution_result.resolved_packages {
        if package.name == "flutter" {
            continue;
        }
        packages_to_download.push(("pub.dev".to_string(), package.name.clone(), package.version.clone()));
    }

    for (registry, name, version) in packages_to_download {
        print!("   Downloading {}@{}... ", name, version);
        match cache_manager.get_package(&registry, &name, &version).await {
            Ok(path) => {
                println!("✓");
                downloaded_packages.insert(format!("{}@{}", name, version), path);
            }
            Err(e) => {
                println!("✗");
                warn!("Failed to download {}@{}: {}", name, version, e);
            }
        }
    }

    println!("📝 Generating package_config.json...");
    cache_manager.generate_package_config(&downloaded_packages, &manifest.name)?;

    cache_manager.generate_packages_file(&downloaded_packages, &manifest.name)?;

    for warning in &resolution_result.warnings {
        warn!("⚠️ {}", warning);
    }

    ScriptRunner::run_post_install(&manifest)?;
    ScriptRunner::run_profile_scripts(&manifest, &profile_name, "post-install")?;

    println!("✅ Dependencies installed successfully");
    println!("   {} packages resolved", resolution_result.resolved_packages.len());
    println!("   {} packages downloaded", downloaded_packages.len());
    if !resolution_result.warnings.is_empty() {
        println!("   {} warnings", resolution_result.warnings.len());
    }

    Ok(())
}