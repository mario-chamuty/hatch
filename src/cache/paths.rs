use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use dirs;

/// Manages all cache-related paths
pub struct CachePaths;

impl CachePaths {
    /// Get the root cache directory (~/.hatch/cache)
    pub fn root() -> Result<PathBuf> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow!("Could not determine home directory"))?;
        Ok(home.join(".hatch").join("cache"))
    }

    /// Get packages directory (~/.hatch/cache/packages)
    pub fn packages_dir() -> Result<PathBuf> {
        Ok(Self::root()?.join("packages"))
    }

    /// Get downloads directory (~/.hatch/cache/downloads)
    pub fn downloads_dir() -> Result<PathBuf> {
        Ok(Self::root()?.join("downloads"))
    }

    /// Get metadata directory (~/.hatch/cache/metadata)
    pub fn metadata_dir() -> Result<PathBuf> {
        Ok(Self::root()?.join("metadata"))
    }

    /// Get package directory for a specific package and version
    /// e.g., ~/.hatch/cache/packages/pub.dev/http/1.1.0
    pub fn package_dir(registry: &str, name: &str, version: &str) -> Result<PathBuf> {
        Ok(Self::packages_dir()?
            .join(registry)
            .join(name)
            .join(version))
    }

    /// Get download path for a package tarball
    pub fn download_path(name: &str, version: &str) -> Result<PathBuf> {
        Ok(Self::downloads_dir()?
            .join(format!("{}-{}.tar.gz", name, version)))
    }

    /// Get metadata path for a package
    pub fn metadata_path(registry: &str, name: &str) -> Result<PathBuf> {
        Ok(Self::metadata_dir()?
            .join(registry)
            .join(format!("{}.json", name)))
    }

    /// Ensure all cache directories exist
    pub fn ensure_directories() -> Result<()> {
        std::fs::create_dir_all(Self::packages_dir()?)?;
        std::fs::create_dir_all(Self::downloads_dir()?)?;
        std::fs::create_dir_all(Self::metadata_dir()?)?;
        Ok(())
    }

    /// Check if a package is cached
    pub fn is_package_cached(registry: &str, name: &str, version: &str) -> Result<bool> {
        let package_path = Self::package_dir(registry, name, version)?;
        Ok(package_path.exists())
    }
}