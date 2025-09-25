use anyhow::Result;
use std::io::{self, Write};
use colored::Colorize;

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