use anyhow::Result;
use log::warn;
use std::collections::HashMap;
use std::time::Instant;
use indicatif::{ProgressBar, ProgressStyle};

use crate::manifest::parser::ManifestParser;
use crate::resolver::ultra::UltraResolver;
use crate::resolver::sat::ResolvedPackage;
use crate::fvm::installer::FvmInstaller;
use crate::cache::manager::CacheManager;
use crate::lockfile::{LockfileGenerator};
use crate::scripts::ScriptRunner;
use crate::cli::verbosity;

pub async fn execute(profile: Option<String>) -> Result<()> {
    let total_start = Instant::now();
    let profile_name = profile.unwrap_or_else(|| "default".to_string());
    println!("⚡ Smart install with profile '{}'", profile_name);

    if verbosity::is_verbose() {
        println!("📄 Parsing manifest...");
    }
    let manifest = ManifestParser::parse_with_overrides(
        "hatch.json",
        Some("hatch.local.json"),
    )?;

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

    let resolve_start = Instant::now();
    let mut resolver = UltraResolver::with_manifest(&manifest).await;
    let resolved = resolver.resolve(&manifest).await?;
    let resolve_time = resolve_start.elapsed();

    if verbosity::should_show_timings() {
        println!("✅ Resolved {} total packages in {:.2}s", resolved.len(), resolve_time.as_secs_f32());
    } else {
        println!("✅ Resolved {} total packages", resolved.len());
    }

    let resolved_paths = resolver.get_resolved_paths();

    let resolved_packages: Vec<ResolvedPackage> = resolved
        .iter()
        .map(|(name, version)| {
            let deps = HashMap::new(); // Dependencies will be resolved transitively

            ResolvedPackage {
                name: name.clone(),
                version: version.clone(),
                dependencies: deps,
                source_constraint: manifest.require.as_ref()
                    .and_then(|r| r.get(name))
                    .or_else(|| manifest.require_dev.as_ref().and_then(|r| r.get(name)))
                    .unwrap_or(&"any".to_string())
                    .clone(),
            }
        })
        .collect();

    let lockfile_path = std::path::Path::new("hatch.lock");
    LockfileGenerator::generate(&manifest, &resolved_packages, lockfile_path)?;
    println!("🔒 Generated hatch.lock");

    // Pre-check cached packages to avoid unnecessary work
    let cache_manager = CacheManager::new();
    CacheManager::ensure_cache_dirs()?;

    let mut downloaded_packages = HashMap::new();
    let mut packages_to_download = Vec::new();
    let mut already_cached = 0;

    for (name, version) in &resolved {
        if name == "flutter" {
            continue;
        }

        // Skip local packages
        if resolved_paths.contains_key(name) {
            continue;
        }

        // Check if already cached
        let cache_path = crate::cache::paths::CachePaths::package_dir("pub.dev", name, version)?;
        if cache_path.exists() {
            already_cached += 1;
            downloaded_packages.insert(format!("{}@{}", name, version), cache_path);
        } else {
            packages_to_download.push((name.clone(), version.clone()));
        }
    }

    if packages_to_download.is_empty() && already_cached > 0 {
        println!("📦 All {} packages already cached", already_cached);
    } else if packages_to_download.len() > 0 {
        println!("📥 Downloading {} packages ({} already cached)...", packages_to_download.len(), already_cached);
    }

    let download_start = Instant::now();

    // Collect packages to download
    let mut download_tasks = Vec::new();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(100)); // 100 parallel downloads for ultra-fast downloading

    // Process local packages
    for (name, version) in &resolved {
        if let Some(local_path) = resolved_paths.get(name) {
            if verbosity::should_show_download_details() {
                println!("   {} @ {} [local] ... ✓", name, local_path.display());
            }
            downloaded_packages.insert(format!("{}@{}", name, version), local_path.clone());
        }
    }

    // Download only packages that aren't cached
    for (name, version) in packages_to_download {
        let cache_manager_clone = cache_manager.clone();
        let sem_clone = semaphore.clone();

        let task = tokio::spawn(async move {
            let _permit = sem_clone.acquire().await.unwrap();
            let result = cache_manager_clone.get_package("pub.dev", &name, &version).await;
            (name, version, result)
        });

        download_tasks.push(task);
    }

    // Create progress bar for downloads
    let total_downloads = download_tasks.len();
    let progress = if verbosity::should_show_progress_bars() && total_downloads > 0 {
        let pb = ProgressBar::new(total_downloads as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("   {bar:40.cyan/blue} {pos}/{len} packages")
                .unwrap()
                .progress_chars("=>-"),
        );
        Some(pb)
    } else {
        None
    };

    // Wait for all downloads
    let results = futures::future::join_all(download_tasks).await;
    let download_time = download_start.elapsed();

    let mut failed = 0;
    for result in results {
        match result {
            Ok((name, version, Ok(path))) => {
                if verbosity::should_show_download_details() {
                    println!("   {} @ {} ... ✓", name, version);
                }
                if let Some(ref pb) = progress {
                    pb.inc(1);
                }
                downloaded_packages.insert(format!("{}@{}", name, version), path);
            }
            Ok((name, version, Err(e))) => {
                if verbosity::should_show_download_details() {
                    println!("   {} @ {} ... ✗", name, version);
                }
                warn!("Failed to download {}@{}: {}", name, version, e);
                if let Some(ref pb) = progress {
                    pb.inc(1);
                }
                failed += 1;
            }
            Err(e) => {
                warn!("Task failed: {}", e);
                if let Some(ref pb) = progress {
                    pb.inc(1);
                }
                failed += 1;
            }
        }
    }

    if let Some(pb) = progress {
        pb.finish_and_clear();
    }

    if failed > 0 {
        println!("⚠️  {} packages failed to download", failed);
    }

    if verbosity::should_show_timings() {
        println!("   ⏱️  Downloads completed in {:.2}s", download_time.as_secs_f32());
    }

    if verbosity::is_verbose() {
        println!("📝 Generating package_config.json...");
    }
    let config_start = Instant::now();
    cache_manager.generate_package_config(&downloaded_packages, &manifest.name)?;
    cache_manager.generate_packages_file(&downloaded_packages, &manifest.name)?;
    if verbosity::should_show_timings() {
        println!("   ⏱️  Config generation: {:.2}s", config_start.elapsed().as_secs_f32());
    }

    ScriptRunner::run_post_install(&manifest)?;
    ScriptRunner::run_profile_scripts(&manifest, &profile_name, "post-install")?;

    let total_time = total_start.elapsed();
    println!("✅ Installation complete!");
    if verbosity::is_verbose() {
        println!("   {} packages resolved", resolved.len());
        println!("   {} packages downloaded", downloaded_packages.len());
    }
    if verbosity::should_show_timings() {
        println!("   ⏱️  Total time: {:.2}s", total_time.as_secs_f32());
    }

    Ok(())
}