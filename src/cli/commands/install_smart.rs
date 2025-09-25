use anyhow::{anyhow, Result};
use log::{debug, info, warn};
use std::collections::HashMap;

use crate::manifest::parser::ManifestParser;
use crate::resolver::smart::SmartResolver;
use crate::resolver::sat::ResolvedPackage;
use crate::fvm::installer::FvmInstaller;
use crate::cache::manager::CacheManager;
use crate::lockfile::{LockfileParser, LockfileGenerator};
use crate::scripts::ScriptRunner;

pub async fn execute(profile: Option<String>) -> Result<()> {
    let profile_name = profile.unwrap_or_else(|| "default".to_string());
    println!("⚡ Smart install with profile '{}'", profile_name);

    println!("📄 Parsing manifest...");
    let manifest = ManifestParser::parse_with_overrides(
        "hatch.yaml",
        Some("hatch.local.yaml"),
    ).or_else(|_| {
        ManifestParser::parse_with_overrides(
            "hatch.json",
            Some("hatch.local.json"),
        )
    })?;

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

    println!("🔍 Resolving {} direct dependencies...", dependencies.len());

    let mut resolver = SmartResolver::with_manifest(&manifest);
    let resolved = resolver.resolve(&dependencies).await?;

    println!("✅ Resolved {} total packages", resolved.len());

    let resolved_packages: Vec<ResolvedPackage> = resolved
        .iter()
        .map(|(name, version)| {
            let deps = resolver
                .get_metadata_cache()
                .get(name)
                .and_then(|versions| {
                    versions.iter()
                        .find(|v| v.version == *version)
                        .map(|v| v.dependencies.clone())
                })
                .unwrap_or_default();

            ResolvedPackage {
                name: name.clone(),
                version: version.clone(),
                dependencies: deps,
                source_constraint: dependencies.get(name).unwrap_or(&"any".to_string()).clone(),
            }
        })
        .collect();

    let lockfile_path = std::path::Path::new("hatch.lock");
    LockfileGenerator::generate(&manifest, &resolved_packages, lockfile_path)?;
    println!("🔒 Generated hatch.lock");

    println!("📥 Downloading packages...");
    let cache_manager = CacheManager::new();
    CacheManager::ensure_cache_dirs()?;

    let mut downloaded_packages = HashMap::new();
    let resolved_paths = resolver.get_resolved_paths();
    let display_versions = resolver.get_display_versions();

    for (name, version) in &resolved {
        if name == "flutter" {
            continue;
        }

        // Check if it's a local/path package
        if let Some(local_path) = resolved_paths.get(name) {
            println!("   {} @ {} [local] ... ✓", name, local_path.display());
            downloaded_packages.insert(format!("{}@{}", name, version), local_path.clone());
            continue;
        }

        // Get display version for user output
        let display_version = display_versions.get(name).unwrap_or(version);
        let version_str = if display_version != version {
            format!("{} (actually {})", display_version, version)
        } else {
            version.clone()
        };

        print!("   {} @ {} ... ", name, version_str);
        match cache_manager.get_package("pub.dev", name, version).await {
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

    ScriptRunner::run_post_install(&manifest)?;
    ScriptRunner::run_profile_scripts(&manifest, &profile_name, "post-install")?;

    println!("✅ Installation complete!");
    println!("   {} packages resolved", resolved.len());
    println!("   {} packages downloaded", downloaded_packages.len());

    Ok(())
}