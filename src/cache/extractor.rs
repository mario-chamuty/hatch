use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use log::{debug, info, warn};
use std::io::Read;

pub struct PackageExtractor;

impl PackageExtractor {
    /// Convert path to Windows extended-length path if needed
    #[cfg(windows)]
    fn to_extended_path(path: &Path) -> PathBuf {
        let path_str = path.to_string_lossy();

        // Already extended path
        if path_str.starts_with("\\\\?\\") {
            return path.to_path_buf();
        }

        // Convert to absolute path if relative
        let abs_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_default()
                .join(path)
        };

        // Add extended-length prefix for absolute paths
        let abs_str = abs_path.to_string_lossy().replace('/', "\\");
        PathBuf::from(format!("\\\\?\\{}", abs_str))
    }

    #[cfg(not(windows))]
    fn to_extended_path(path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    pub fn extract_tar_gz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
        if crate::cli::verbosity::is_ultra_verbose() {
            info!(
                "Extracting {} to {}",
                archive_path.display(),
                dest_dir.display()
            );
        } else {
            debug!(
                "Extracting {} to {}",
                archive_path.display(),
                dest_dir.display()
            );
        }

        // Use extended path on Windows for the destination directory
        let dest_dir = Self::to_extended_path(dest_dir);
        std::fs::create_dir_all(&dest_dir)?;

        let file = std::fs::File::open(archive_path)
            .map_err(|e| anyhow!("Failed to open archive: {}", e))?;

        let decoder = flate2::read::GzDecoder::new(file);

        let mut archive = tar::Archive::new(decoder);
        archive.set_preserve_permissions(false);
        archive.set_preserve_mtime(false);

        let mut entries = archive.entries()?;
        let mut long_name: Option<String> = None;

        while let Some(entry) = entries.next() {
            let mut entry = entry?;
            let header = entry.header();

            // Get the entry path
            let mut path = entry.path()?.to_path_buf();
            let path_str = path.to_string_lossy();

            // Check if this is a GNU long link entry
            if path_str.contains("@LongLink") || path_str == "././@LongLink" {
                // Read the long filename from the entry content
                let mut long_name_bytes = Vec::new();
                entry.read_to_end(&mut long_name_bytes)?;

                // Remove null terminator and convert to string
                if let Some(null_pos) = long_name_bytes.iter().position(|&b| b == 0) {
                    long_name_bytes.truncate(null_pos);
                }

                long_name = Some(String::from_utf8_lossy(&long_name_bytes).into_owned());
                if crate::cli::verbosity::is_ultra_verbose() {
                    debug!("Found long name: {:?}", long_name);
                }
                continue;
            }

            // Skip pax headers
            if path_str == "pax_global_header" || path_str.starts_with("PaxHeader") {
                continue;
            }

            // Use the long name if we have one
            if let Some(ref name) = long_name {
                path = PathBuf::from(name);
                long_name = None; // Reset for next entry
                if crate::cli::verbosity::is_ultra_verbose() {
                    debug!("Using long name for extraction: {}", path.display());
                }
            }

            // Security check
            if path.components().any(|c| c == std::path::Component::ParentDir) {
                return Err(anyhow!("Archive contains path traversal: {:?}", path));
            }

            // Build destination path
            let dest_path = dest_dir.join(&path);

            // Use extended paths on Windows for very long paths
            #[cfg(windows)]
            let dest_path = Self::to_extended_path(&dest_path);

            // Create parent directories
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Extract based on entry type
            let entry_type = header.entry_type();
            if entry_type.is_file() {
                // Read content and write to file
                let mut content = Vec::new();
                entry.read_to_end(&mut content)?;

                std::fs::write(&dest_path, content)
                    .map_err(|e| anyhow!("Failed to write {:?}: {}", dest_path, e))?;

                if crate::cli::verbosity::is_ultra_verbose() {
                    debug!("Extracted file: {}", path.display());
                }
            } else if entry_type.is_dir() {
                std::fs::create_dir_all(&dest_path)?;
                if crate::cli::verbosity::is_ultra_verbose() {
                    debug!("Created directory: {}", path.display());
                }
            } else {
                debug!("Skipping special entry type: {:?} for {}", entry_type, path.display());
            }
        }

        if crate::cli::verbosity::is_ultra_verbose() {
            info!("Extraction complete: {}", dest_dir.display());
        } else {
            debug!("Extraction complete: {}", dest_dir.display());
        }
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

    #[cfg(windows)]
    fn move_directory_contents(src: &Path, dst: &Path) -> Result<()> {
        use std::fs;

        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let src_path = entry.path();

            // Use extended path for destination
            let dst_path = Self::to_extended_path(&dst.join(&file_name));

            if src_path.is_dir() {
                fs::create_dir_all(&dst_path)?;
                Self::move_directory_contents(&src_path, &dst_path)?;
            } else {
                // Try to create parent directory
                if let Some(parent) = dst_path.parent() {
                    fs::create_dir_all(parent)?;
                }

                // Copy file (move might fail across drives)
                if let Err(e) = fs::copy(&src_path, &dst_path) {
                    warn!("Failed to copy {}: {}", src_path.display(), e);
                    // Continue with other files
                } else {
                    // Remove source after successful copy
                    let _ = fs::remove_file(&src_path);
                }
            }
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