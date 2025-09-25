use anyhow::{anyhow, Result};
use log::{debug, info, warn};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use futures::future::join_all;

use crate::manifest::parser::ManifestParser;
use crate::registry::pub_dev::PubDevRegistry;
use crate::registry::traits::{Registry, PackageVersion};
use crate::resolver::sat::SatSolver;
use crate::fvm::installer::FvmInstaller;
use crate::cache::manager::CacheManager;
use crate::lockfile::{LockfileParser, LockfileGenerator};

/// Lightning-fast install with parallel operations
pub async fn execute_fast(profile: Option<String>) -> Result<()> {
    info!("Installing dependencies (fast mode)");

    let profile_name = profile.unwrap_or_else(|| "default".to_string());
    println!("⚡ Fast install with profile '{}'", profile_name);

    // Parse manifest
    let manifest = ManifestParser::parse_with_overrides(
        "hatch.yaml",
        Some("hatch.local.yaml"),
    ).or_else(|_| {
        ManifestParser::parse_with_overrides(
            "hatch.json",
            Some("hatch.local.json"),
        )
    })?;

    // Check lockfile for fast path
    let lockfile_path = Path::new("hatch.lock");
    let existing_lockfile = LockfileParser::parse(lockfile_path).ok();

    // Quick validation: if lockfile exists and manifest hasn't changed, use it
    if let Some(ref lockfile) = existing_lockfile {
        if is_lockfile_valid(&manifest, lockfile) {
            println!("📋 Using cached resolution from hatch.lock");
            return install_from_lockfile(lockfile, &manifest).await;
        }
    }

    // Setup FVM if needed (async)
    let fvm_handle = if let Some(flutter_version) = &manifest.sdk.flutter {
        let version = flutter_version.clone();
        Some(tokio::spawn(async move {
            match FvmInstaller::auto_install_from_manifest(&version).await {
                Ok(true) => println!("✅ Flutter {} ready", version),
                Ok(false) => warn!("⚠️ Flutter version management not available"),
                Err(e) => warn!("⚠️ Failed to setup Flutter {}: {}", version, e),
            }
        }))
    } else {
        None
    };

    // Collect dependencies
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

    println!("🚀 Resolving {} dependencies...", dependencies.len());

    // Parallel fetch of all package metadata
    let registry = Arc::new(PubDevRegistry::new());
    let available_versions = fetch_all_metadata_parallel(
        &dependencies,
        registry.clone(),
        &existing_lockfile,
    ).await?;

    // Resolve dependencies
    println!("🧮 Solving constraints...");
    let mut solver = SatSolver::new();
    let resolution_result = solver.resolve_dependencies(&dependencies, &available_versions)?;

    if !resolution_result.conflicts.is_empty() {
        for conflict in &resolution_result.conflicts {
            println!("❌ Conflict: {} - {}", conflict.package, conflict.reason);
        }
        return Err(anyhow!("Cannot resolve dependency conflicts"));
    }

    // Generate lockfile
    LockfileGenerator::generate(&manifest, &resolution_result.resolved_packages, lockfile_path)?;

    // Parallel download of all packages
    println!("📥 Downloading {} packages in parallel...", resolution_result.resolved_packages.len());
    let cache_manager = Arc::new(CacheManager::new());

    download_packages_parallel(
        &resolution_result.resolved_packages,
        cache_manager.clone(),
    ).await?;

    // Generate package_config.json
    println!("📝 Generating package_config.json...");
    let mut downloaded_packages = HashMap::new();
    for package in &resolution_result.resolved_packages {
        if package.name != "flutter" {
            let path = crate::cache::paths::CachePaths::package_dir(
                "pub.dev",
                &package.name,
                &package.version,
            )?;
            downloaded_packages.insert(
                format!("{}@{}", package.name, package.version),
                path,
            );
        }
    }

    cache_manager.generate_package_config(&downloaded_packages, &manifest.name)?;
    cache_manager.generate_packages_file(&downloaded_packages, &manifest.name)?;

    // Wait for FVM if needed
    if let Some(handle) = fvm_handle {
        let _ = handle.await;
    }

    println!("✅ Installation complete ({} packages)", downloaded_packages.len());
    Ok(())
}

/// Check if lockfile is still valid for the manifest
fn is_lockfile_valid(manifest: &HatchManifest, lockfile: &crate::lockfile::parser::HatchLockfile) -> bool {
    // Check Flutter/Dart versions match
    if manifest.sdk.flutter != lockfile.flutter_version {
        return false;
    }
    if manifest.sdk.dart != lockfile.dart_version {
        return false;
    }

    // Check if all required packages are in lockfile
    if let Some(require) = &manifest.require {
        for (name, _) in require {
            if !lockfile.packages.contains_key(name) && name != "flutter" {
                return false;
            }
        }
    }

    true
}

/// Install directly from lockfile (super fast path)
async fn install_from_lockfile(
    lockfile: &crate::lockfile::parser::HatchLockfile,
    manifest: &HatchManifest,
) -> Result<()> {
    let cache_manager = Arc::new(CacheManager::new());
    CacheManager::ensure_cache_dirs()?;

    // Parallel download of all locked packages
    let packages: Vec<_> = lockfile.packages
        .iter()
        .map(|(name, pkg)| (name.clone(), pkg.version.clone(), pkg.registry.clone()))
        .collect();

    println!("📥 Downloading {} packages from lock...", packages.len());

    let semaphore = Arc::new(Semaphore::new(10)); // Limit concurrent downloads
    let downloads = packages.iter().map(|(name, version, registry)| {
        let cache_manager = cache_manager.clone();
        let semaphore = semaphore.clone();
        let name = name.clone();
        let version = version.clone();
        let registry = registry.clone();

        async move {
            let _permit = semaphore.acquire().await.unwrap();
            match cache_manager.get_package(&registry, &name, &version).await {
                Ok(path) => {
                    println!("   ✓ {}", name);
                    Ok((format!("{}@{}", name, version), path))
                }
                Err(e) => {
                    println!("   ✗ {}: {}", name, e);
                    Err(e)
                }
            }
        }
    });

    let results = join_all(downloads).await;
    let mut downloaded_packages = HashMap::new();

    for result in results {
        if let Ok((key, path)) = result {
            downloaded_packages.insert(key, path);
        }
    }

    // Generate configs
    cache_manager.generate_package_config(&downloaded_packages, &manifest.name)?;
    cache_manager.generate_packages_file(&downloaded_packages, &manifest.name)?;

    println!("✅ Restored {} packages from lock", downloaded_packages.len());
    Ok(())
}

/// Fetch all package metadata in parallel
async fn fetch_all_metadata_parallel(
    dependencies: &HashMap<String, String>,
    registry: Arc<PubDevRegistry>,
    existing_lockfile: &Option<crate::lockfile::parser::HatchLockfile>,
) -> Result<HashMap<String, Vec<PackageVersion>>> {
    let mut to_fetch = dependencies.keys().cloned().collect::<Vec<_>>();
    let fetched = Arc::new(Mutex::new(HashSet::new()));
    let available_versions = Arc::new(Mutex::new(HashMap::new()));
    let semaphore = Arc::new(Semaphore::new(20)); // Allow 20 concurrent fetches

    let mut iteration = 0;
    while !to_fetch.is_empty() {
        iteration += 1;
        println!("   Round {}: fetching {} packages", iteration, to_fetch.len());

        let futures = to_fetch.iter().map(|dep_name| {
            let registry = registry.clone();
            let fetched = fetched.clone();
            let available_versions = available_versions.clone();
            let dep_name = dep_name.clone();
            let semaphore = semaphore.clone();
            let existing_lockfile = existing_lockfile.clone();

            async move {
                // Skip if already fetched or is Flutter SDK
                {
                    let fetched_guard = fetched.lock().await;
                    if fetched_guard.contains(&dep_name) || dep_name == "flutter" {
                        return Ok(Vec::new());
                    }
                }

                // Check if we can use lockfile data for this package
                if let Some(ref lockfile) = existing_lockfile {
                    if let Some(locked_pkg) = lockfile.packages.get(&dep_name) {
                        // Trust lockfile for transitive deps
                        debug!("Using lockfile data for {}", dep_name);
                        let mut fetched_guard = fetched.lock().await;
                        fetched_guard.insert(dep_name.clone());

                        // Create minimal version info from lockfile
                        let version = PackageVersion {
                            version: locked_pkg.version.clone(),
                            published: chrono::Utc::now(),
                            dependencies: locked_pkg.dependencies.clone(),
                        };

                        let mut versions_guard = available_versions.lock().await;
                        versions_guard.insert(dep_name.clone(), vec![version]);

                        return Ok(locked_pkg.dependencies.keys().cloned().collect());
                    }
                }

                let _permit = semaphore.acquire().await.unwrap();

                match registry.get_package_metadata(&dep_name).await {
                    Ok(metadata) => {
                        let mut transitive = Vec::new();
                        for version in &metadata.versions {
                            for (trans_dep, _) in &version.dependencies {
                                if trans_dep != "flutter" {
                                    transitive.push(trans_dep.clone());
                                }
                            }
                        }

                        let mut fetched_guard = fetched.lock().await;
                        fetched_guard.insert(dep_name.clone());

                        let mut versions_guard = available_versions.lock().await;
                        versions_guard.insert(dep_name, metadata.versions);

                        Ok(transitive)
                    }
                    Err(e) => {
                        warn!("Failed to fetch {}: {}", dep_name, e);
                        Ok(Vec::new())
                    }
                }
            }
        });

        let results = join_all(futures).await;
        to_fetch.clear();

        for result in results {
            if let Ok(transitive) = result {
                for dep in transitive {
                    let fetched_guard = fetched.lock().await;
                    if !fetched_guard.contains(&dep) {
                        to_fetch.push(dep);
                    }
                }
            }
        }
    }

    let versions = available_versions.lock().await;
    Ok(versions.clone())
}

/// Download packages in parallel
async fn download_packages_parallel(
    packages: &[crate::resolver::sat::ResolvedPackage],
    cache_manager: Arc<CacheManager>,
) -> Result<()> {
    let semaphore = Arc::new(Semaphore::new(10)); // Limit to 10 concurrent downloads

    let downloads = packages.iter()
        .filter(|p| p.name != "flutter")
        .map(|package| {
            let cache_manager = cache_manager.clone();
            let semaphore = semaphore.clone();
            let name = package.name.clone();
            let version = package.version.clone();

            async move {
                let _permit = semaphore.acquire().await.unwrap();
                print!("   {} @ {} ... ", name, version);

                match cache_manager.get_package("pub.dev", &name, &version).await {
                    Ok(_) => {
                        println!("✓");
                        Ok(())
                    }
                    Err(e) => {
                        println!("✗");
                        Err(e)
                    }
                }
            }
        });

    let results = join_all(downloads).await;

    for result in results {
        result?;
    }

    Ok(())
}