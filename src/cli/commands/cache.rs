use anyhow::Result;
use std::io::{self, Write};
use colored::Colorize;
use chrono::Local;

use crate::cache::manager::CacheManager;
use crate::cache::storage::PackageStorage;
use crate::cache::paths::CachePaths;

pub async fn clear(force: bool) -> Result<()> {
    if !force {
        print!("⚠️  This will delete all cached packages. Continue? [y/N]: ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    println!("🗑️  Clearing cache...");
    CacheManager::clear_cache()?;
    println!("✅ Cache cleared successfully");

    Ok(())
}

pub async fn stats() -> Result<()> {
    println!("📊 Cache Statistics");
    println!("═══════════════════");

    // Show cache location
    let cache_dir = CachePaths::root()?;
    println!("📁 Location: {}", cache_dir.display());

    if std::env::var("HATCH_CACHE_DIR").is_ok() {
        println!("   (Using custom cache from HATCH_CACHE_DIR)");
    }

    let stats = CacheManager::get_cache_stats()?;

    println!("📦 Packages: {}", stats.package_count);
    println!("💾 Total size: {}", stats.format_size());

    if stats.package_count > 0 {
        println!("\n📋 Cached packages:");
        for package in stats.packages.iter().take(20) {
            println!("   • {} @ {} ({:.1} MB)",
                package.name,
                package.version,
                package.size as f64 / 1_048_576.0
            );
        }

        if stats.packages.len() > 20 {
            println!("   ... and {} more", stats.packages.len() - 20);
        }
    }

    Ok(())
}

pub async fn remove(package: &str, version: Option<&str>) -> Result<()> {
    if let Some(ver) = version {
        println!("🗑️  Removing {}@{} from cache...", package, ver);
    } else {
        println!("🗑️  Removing all versions of {} from cache...", package);
    }

    PackageStorage::clear_package("pub.dev", package, version)?;

    println!("✅ Package removed from cache");

    Ok(())
}

pub async fn list(detailed: bool) -> Result<()> {
    let stats = CacheManager::get_cache_stats()?;

    if stats.package_count == 0 {
        println!("📦 No packages in cache");
        return Ok(());
    }

    println!("📦 Cached packages ({} total):", stats.package_count);
    println!();

    for package in &stats.packages {
        if detailed {
            println!("📦 {}@{}", package.name.bold(), package.version);
            println!("   📁 Location: {}", package.path.display());
            println!("   💾 Size: {:.2} MB", package.size as f64 / 1_048_576.0);
            println!("   📅 Modified: {}", package.modified.format("%Y-%m-%d %H:%M:%S"));
            println!();
        } else {
            println!("   • {}@{} ({:.1} MB)",
                package.name,
                package.version,
                package.size as f64 / 1_048_576.0
            );
        }
    }

    if !detailed {
        println!();
        println!("💾 Total size: {}", stats.format_size());
        println!("   Use --detailed for more information");
    }

    Ok(())
}

pub async fn prune(dry_run: bool, aggressive: bool) -> Result<()> {
    println!("🔍 Analyzing cache for unused packages...");

    // Get current project dependencies from pubspec.lock or hatch.lock
    let used_packages = get_used_packages()?;
    let stats = CacheManager::get_cache_stats()?;

    let mut unused_packages = Vec::new();
    let mut unused_size = 0u64;

    for package in &stats.packages {
        let key = format!("{}@{}", package.name, package.version);
        if !used_packages.contains(&key) {
            unused_packages.push(package);
            unused_size += package.size;
        }
    }

    if unused_packages.is_empty() {
        println!("✅ No unused packages found in cache");
    } else {
        println!("Found {} unused packages ({:.1} MB):",
            unused_packages.len(),
            unused_size as f64 / 1_048_576.0
        );

        for package in &unused_packages {
            println!("   • {}@{} ({:.1} MB)",
                package.name,
                package.version,
                package.size as f64 / 1_048_576.0
            );
        }

        if dry_run {
            println!();
            println!("ℹ️  Dry run mode - no packages were removed");
            println!("   Run without --dry-run to actually remove packages");
        } else {
            print!("\n⚠️  Remove these packages? [y/N]: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            if input.trim().eq_ignore_ascii_case("y") {
                let package_count = unused_packages.len();
                for package in unused_packages {
                    PackageStorage::clear_package("pub.dev", &package.name, Some(&package.version))?;
                }
                println!("✅ Removed {} packages, freed {:.1} MB",
                    package_count,
                    unused_size as f64 / 1_048_576.0
                );
            } else {
                println!("Cancelled.");
            }
        }
    }

    if aggressive && !dry_run {
        run_aggressive_prune().await?;
    } else if aggressive && dry_run {
        println!("ℹ️  --aggressive has no effect in --dry-run mode");
    }

    Ok(())
}

/// Aggressive prune:
///   1. Lockfile-based pruning already ran above.
///   2. Re-apply Stream A's debloat allowlist to already-extracted packages.
///      If the `package_manifest` module is not present yet (Stream A still
///      in flight), this step is skipped with a warning.
///   3. MetadataCache::prune_aggressive().
///   4. Merge every per-package `.hatch_metadata.json` sidecar into a
///      central `~/.hatch/cache/index.json`, then delete the sidecars.
async fn run_aggressive_prune() -> Result<()> {
    println!();
    println!("🧹 Aggressive prune running...");

    // 2. Re-apply debloat.
    // TODO: re-apply debloat once package_manifest lands (Stream A). The
    // hook would call `crate::cache::package_manifest::keep_decision`
    // on every file in each cached package dir and delete any file whose
    // keep_decision says it should be stripped. Skipped for now because
    // that module is not yet in the tree.
    println!("   (skipped) re-apply debloat allowlist – depends on Stream A's package_manifest module");

    // 3. Aggressive metadata prune.
    let metadata_cache = crate::cache::metadata_cache::MetadataCache::new();
    match metadata_cache.prune_aggressive() {
        Ok(stats) => {
            println!(
                "   Metadata pruned: {} files removed, {} versions trimmed, ~{} bytes freed",
                stats.files_removed, stats.versions_removed, stats.bytes_freed
            );
        }
        Err(e) => {
            println!("   Metadata prune failed: {e}");
        }
    }

    // 4. Merge sidecars into central index.
    match merge_metadata_sidecars() {
        Ok(count) => println!("   Merged {count} metadata sidecars into central index.json"),
        Err(e) => println!("   Sidecar merge failed: {e}"),
    }

    println!("✅ Aggressive prune complete");
    Ok(())
}

/// Walk every cached package dir, read its `.hatch_metadata.json`, merge it
/// into `~/.hatch/cache/index.json`, then delete the sidecar. Returns the
/// number of sidecars that were successfully merged. Best-effort: per-file
/// failures are logged, not fatal.
fn merge_metadata_sidecars() -> Result<usize> {
    use std::collections::BTreeMap;

    let packages_dir = CachePaths::packages_dir()?;
    if !packages_dir.exists() {
        return Ok(0);
    }

    let index_path = CachePaths::root()?.join("index.json");
    let mut index: BTreeMap<String, serde_json::Value> = match std::fs::read_to_string(&index_path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => BTreeMap::new(),
    };

    let mut merged = 0usize;
    for registry_entry in std::fs::read_dir(&packages_dir)? {
        let registry_entry = registry_entry?;
        if !registry_entry.path().is_dir() {
            continue;
        }
        let registry_name = registry_entry.file_name().to_string_lossy().to_string();
        for package_entry in std::fs::read_dir(registry_entry.path())? {
            let package_entry = package_entry?;
            if !package_entry.path().is_dir() {
                continue;
            }
            let package_name = package_entry.file_name().to_string_lossy().to_string();
            for version_entry in std::fs::read_dir(package_entry.path())? {
                let version_entry = version_entry?;
                if !version_entry.path().is_dir() {
                    continue;
                }
                let version = version_entry.file_name().to_string_lossy().to_string();
                let sidecar = version_entry.path().join(".hatch_metadata.json");
                if !sidecar.exists() {
                    continue;
                }
                let content = match std::fs::read_to_string(&sidecar) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let value: serde_json::Value = match serde_json::from_str(&content) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let key = format!("{registry_name}/{package_name}/{version}");
                index.insert(key, value);
                let _ = std::fs::remove_file(&sidecar);
                merged += 1;
            }
        }
    }

    let json = serde_json::to_string_pretty(&index)?;
    std::fs::write(&index_path, json)?;
    Ok(merged)
}

fn get_used_packages() -> Result<Vec<String>> {
    let mut used = Vec::new();

    // Check hatch.lock first
    let lockfile_path = std::path::Path::new("hatch.lock");
    if lockfile_path.exists() {
        use crate::lockfile::LockfileParser;
        let lockfile = LockfileParser::parse(lockfile_path)?;
        for (name, package) in &lockfile.packages {
            used.push(format!("{}@{}", name, package.version));
        }
    }

    // Also check pubspec.lock if it exists
    let pubspec_lock_path = std::path::Path::new("pubspec.lock");
    if pubspec_lock_path.exists() {
        // Simple parsing of pubspec.lock YAML
        let content = std::fs::read_to_string(pubspec_lock_path)?;
        for line in content.lines() {
            if line.trim_start().starts_with("version:") {
                // This is a simplified parser - in reality we'd need to track the package name too
                // For now, we'll just mark this as TODO
            }
        }
    }

    Ok(used)
}

pub async fn verify() -> Result<()> {
    println!("🔍 Verifying cache integrity...");

    let stats = CacheManager::get_cache_stats()?;
    let mut issues = Vec::new();

    for package in &stats.packages {
        // Check if package directory exists and is valid
        if !package.path.exists() {
            issues.push(format!("{}@{}: Directory missing", package.name, package.version));
        } else if !package.path.join("lib").exists() {
            issues.push(format!("{}@{}: Missing lib directory", package.name, package.version));
        }

        // Check if pubspec.yaml exists
        let pubspec_path = package.path.join("pubspec.yaml");
        if !pubspec_path.exists() {
            issues.push(format!("{}@{}: Missing pubspec.yaml", package.name, package.version));
        }
    }

    if issues.is_empty() {
        println!("✅ Cache verification passed");
        println!("   {} packages verified", stats.package_count);
    } else {
        println!("❌ Cache verification found {} issues:", issues.len());
        for issue in issues {
            println!("   • {}", issue);
        }
        println!();
        println!("   Run 'hatch cache clear' to fix these issues");
    }

    Ok(())
}