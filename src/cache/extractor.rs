use anyhow::{anyhow, Result};
use std::path::Path;
use log::{debug, info};

pub struct PackageExtractor;

impl PackageExtractor {
    pub fn extract_tar_gz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
        info!(
            "Extracting {} to {}",
            archive_path.display(),
            dest_dir.display()
        );

        std::fs::create_dir_all(dest_dir)?;

        let file = std::fs::File::open(archive_path)
            .map_err(|e| anyhow!("Failed to open archive: {}", e))?;

        let decoder = flate2::read::GzDecoder::new(file);

        let mut archive = tar::Archive::new(decoder);

        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?;

            if path.components().any(|c| c == std::path::Component::ParentDir) {
                return Err(anyhow!("Archive contains path traversal: {:?}", path));
            }

            let dest_path = dest_dir.join(&path);

            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            entry.unpack(&dest_path)?;
            debug!("Extracted: {}", dest_path.display());
        }

        info!("Extraction complete: {}", dest_dir.display());
        Ok(())
    }

    pub fn extract_package(archive_path: &Path, dest_dir: &Path) -> Result<()> {
        Self::extract_tar_gz(archive_path, dest_dir)?;

        Self::verify_package_structure(dest_dir)?;

        Ok(())
    }

    fn verify_package_structure(package_dir: &Path) -> Result<()> {
        let lib_dir = package_dir.join("lib");

        if !lib_dir.exists() {
            debug!("Package has no lib/ directory: {}", package_dir.display());
        }

        let pubspec = package_dir.join("pubspec.yaml");
        if !pubspec.exists() {
            return Err(anyhow!("Invalid package: missing pubspec.yaml"));
        }

        Ok(())
    }

    pub fn cleanup_failed_extraction(dest_dir: &Path) -> Result<()> {
        if dest_dir.exists() {
            std::fs::remove_dir_all(dest_dir)?;
            debug!("Cleaned up failed extraction: {}", dest_dir.display());
        }
        Ok(())
    }
}