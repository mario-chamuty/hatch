use anyhow::Result;
use log::{warn, info, debug};
use std::collections::HashMap;
use std::time::Instant;
use indicatif::{ProgressBar, ProgressStyle};

use crate::manifest::parser::ManifestParser;
use crate::resolver::ultra::Resolver;
use crate::fvm::installer::FvmInstaller;
use crate::cache::manager::CacheManager;
use crate::lockfile::LockfileGenerator;
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

    // Validate dependencies for conflicts
    use crate::manifest::validator::ManifestValidator;
    use anyhow::anyhow;
    if let Err(e) = ManifestValidator::validate(&manifest) {
        return Err(anyhow!("Manifest validation failed: {}", e));
    }

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
    let mut resolver = Resolver::with_manifest(&manifest).await;
    let resolved = resolver.resolve(&manifest).await?;
    let resolve_time = resolve_start.elapsed();

    if verbosity::should_show_timings() {
        println!("✅ Resolved {} total packages in {:.2}s", resolved.len(), resolve_time.as_secs_f32());
    } else {
        println!("✅ Resolved {} total packages", resolved.len());
    }

    let resolved_paths = resolver.get_resolved_paths();
    let resolved_batches = resolver.get_resolved_batches();

    // Pull archive_sha256 values out of the resolver's metadata cache so we
    // can enforce checksum-verified downloads without extra network calls.
    // Missing entries become `None` and fail closed unless
    // `--allow-unchecksummed` was passed.
    let metadata_cache_ref = resolver.metadata_cache().clone();
    let mut checksums: HashMap<String, Option<String>> = HashMap::new();
    for (name, version) in &resolved {
        if let Some(versions) = metadata_cache_ref.get(name).await {
            if let Some(v) = versions.iter().find(|v| &v.version == version) {
                checksums.insert(name.clone(), v.archive_sha256.clone());
                continue;
            }
        }
        checksums.insert(name.clone(), None);
    }

    let graph = resolver.as_graph();
    let lockfile_path = std::path::Path::new("hatch.lock");
    LockfileGenerator::generate_from_graph(&manifest, &graph, lockfile_path)?;
    println!("🔒 Generated hatch.lock");

    // ULTRA-PARALLEL DOWNLOAD STRATEGY:
    // 1. Collect ALL packages (direct + transitive) upfront
    // 2. Check cache for all of them
    // 3. Download ALL missing packages in one massive parallel batch

    println!("📥 Preparing to download packages...");
    let download_start = Instant::now();

    // Debug: Check if we have ALL resolved packages
    println!("🔍 Debug: UltraResolver resolved {} packages", resolved.len());

    let cache_manager = CacheManager::new();
    CacheManager::ensure_cache_dirs()?;

    // First pass: Collect ALL packages and check cache status
    let mut all_packages_to_check = Vec::new();
    let mut downloaded_packages = HashMap::new();
    let mut packages_to_download = Vec::new();

    // Add all resolved packages (this includes ALL transitive deps already)
    for (name, version) in &resolved {
        if name == "flutter" {
            continue;
        }

        // Handle local packages
        if let Some(local_path) = resolved_paths.get(name) {
            if verbosity::should_show_download_details() {
                println!("   {} @ {} [local] ... ✓", name, local_path.display());
            }
            downloaded_packages.insert(format!("{}@{}", name, version), local_path.clone());
            continue;
        }

        all_packages_to_check.push((name.clone(), version.clone()));
    }

    // Batch check cache status for ALL packages
    let mut already_cached = 0;
    for (name, version) in all_packages_to_check {
        let cache_path = crate::cache::paths::CachePaths::package_dir("pub.dev", &name, &version)?;
        if cache_path.exists() {
            already_cached += 1;
            downloaded_packages.insert(format!("{}@{}", name, version), cache_path);
        } else {
            packages_to_download.push((name, version));
        }
    }

    if packages_to_download.is_empty() {
        if already_cached > 0 {
            println!("📦 All {} packages already cached", already_cached);
        }
    } else {
        println!("🚀 Downloading {} packages in parallel ({} already cached)...",
                 packages_to_download.len(), already_cached);

        if verbosity::is_debug() {
            println!("🔍 PARALLEL DOWNLOAD SETUP: Creating {} download tasks", packages_to_download.len());

            // Group packages by batch for display
            let mut packages_by_batch: std::collections::BTreeMap<usize, Vec<(String, String)>> = std::collections::BTreeMap::new();
            for (name, version) in &packages_to_download {
                let batch = resolved_batches.get(name).copied().unwrap_or(999);
                packages_by_batch.entry(batch).or_insert_with(Vec::new).push((name.clone(), version.clone()));
            }

            // Display packages grouped by batch
            for (batch_num, batch_packages) in packages_by_batch {
                let batch_msg = format!("📦 BATCH {} ({} packages):", batch_num, batch_packages.len());
                println!("{}", batch_msg);
                info!("{}", batch_msg);  // Also log to file
                for (name, version) in batch_packages {
                    let pkg_msg = format!("   └── {}@{}", name, version);
                    println!("{}", pkg_msg);
                    info!("{}", pkg_msg);  // Also log to file
                }
            }
        }

        // Create ALL download tasks at once - true parallel downloading!
        // Two separate semaphores decouple the network-bound and CPU-bound
        // phases: many downloads can stream in parallel while a bounded pool
        // of extraction workers drains them as soon as they complete.
        let mut download_tasks = Vec::new();
        let download_semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(100));
        let extract_parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        let extract_semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(extract_parallelism));

        for (name, version) in packages_to_download {
            let cache_manager_clone = cache_manager.clone();
            let dl_sem = download_semaphore.clone();
            let ex_sem = extract_semaphore.clone();
            let batch_num = resolved_batches.get(&name).copied().unwrap_or(999);

            // Resolve the checksum up-front (on the main task, so any missing
            // checksum fails the whole install rather than a silent download
            // failure deep in join_all).
            let checksum_opt = checksums.get(&name).and_then(|o| o.clone());
            let checksum_arg = crate::cli::security::resolve_checksum_or_bypass(
                &name,
                &version,
                checksum_opt.as_deref(),
            )?;

            let task = tokio::spawn(async move {
                let start = std::time::Instant::now();

                // Fast path: already cached (can happen if a parallel batch
                // populated the cache between the initial check and now).
                if let Ok(Some(p)) = crate::cache::storage::PackageStorage::get_package_path(
                    "pub.dev", &name, &version,
                ) {
                    return (name, version, batch_num, Ok(p), start.elapsed());
                }

                // Phase 1: download under the network semaphore only. Drop
                // the permit before extraction so another download can start.
                let archive_path = {
                    let _permit = dl_sem.acquire().await.unwrap();
                    match cache_manager_clone
                        .download_only("pub.dev", &name, &version, &checksum_arg)
                        .await
                    {
                        Ok(p) => p,
                        Err(e) => return (name, version, batch_num, Err(e), start.elapsed()),
                    }
                };

                // Phase 2: extract under the CPU semaphore. Bounded by CPU
                // count so we do not saturate cores with blocking work.
                let result = {
                    let _permit = ex_sem.acquire().await.unwrap();
                    cache_manager_clone
                        .extract_and_cleanup("pub.dev", &name, &version, archive_path)
                        .await
                };
                (name, version, batch_num, result, start.elapsed())
            });

            download_tasks.push(task);
        }

        // Progress bar for parallel downloads
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

        if verbosity::is_debug() {
            println!("⏳ WAITING: for {} parallel download tasks to complete...", download_tasks.len());
        }

        // Wait for ALL downloads to complete in parallel
        let results = futures::future::join_all(download_tasks).await;

        if verbosity::is_debug() {
            println!("🎯 RESULTS: Processing {} download results", results.len());
        }

        let mut failed = 0;
        for result in results {
        match result {
            Ok((name, version, batch_num, Ok(path), elapsed)) => {
                if verbosity::is_debug() {
                    let success_msg = format!("✅ BATCH {} SUCCESS: {} @ {} ... ✓ ({:.2}s)", batch_num, name, version, elapsed.as_secs_f32());
                    println!("{}", success_msg);
                    debug!("{}", success_msg);  // Also log to file
                } else if verbosity::should_show_download_details() {
                    println!("   {} @ {} ... ✓ ({:.2}s)", name, version, elapsed.as_secs_f32());
                }
                if let Some(ref pb) = progress {
                    pb.inc(1);
                }
                downloaded_packages.insert(format!("{}@{}", name, version), path);
            }
            Ok((name, version, _batch_num, Err(e), _elapsed)) => {
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
    }

    let download_time = download_start.elapsed();
    if verbosity::should_show_timings() {
        println!("   ⏱️  All downloads completed in {:.2}s", download_time.as_secs_f32());
    }

    if verbosity::is_verbose() {
        println!("📝 Generating package_config.json...");
    }

    if verbosity::is_debug() {
        println!("📝 CONFIG GENERATION: Starting with {} downloaded packages", downloaded_packages.len());
        for (name_version, path) in &downloaded_packages {
            println!("   └── {} -> {}", name_version, path.display());
        }
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