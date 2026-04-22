use anyhow::{anyhow, Result};
use std::path::{Component, Path, PathBuf};
use log::{debug, info, warn};
use std::io::Read;

pub struct PackageExtractor;

/// Hard caps to defend against archive bombs.
///
/// Values chosen after the security review:
/// - `MAX_ENTRIES`: icon/font packages legitimately ship ~10k files; 50_000 gives headroom
///   without letting a zip-bomb exhaust inodes.
/// - `MAX_FILE_SIZE`: 500 MB per extracted file. Nothing legitimate on pub.dev is that big.
/// - `MAX_TOTAL_SIZE`: 1 GB cumulative decompressed per tarball. Largest real tarballs
///   extract to ~200 MB; 5× headroom.
pub(crate) const MAX_ENTRIES: usize = 50_000;
pub(crate) const MAX_FILE_SIZE: u64 = 500 * 1024 * 1024;
pub(crate) const MAX_TOTAL_SIZE: u64 = 1024 * 1024 * 1024;
pub(crate) const MAX_LONGLINK_BYTES: usize = 4096;

/// Validate a path taken from a tar archive entry before joining it with the extraction root.
///
/// Must reject:
/// - absolute paths (`PathBuf::join` silently discards the base when the joined path is absolute)
/// - any `..` component (parent-dir escape)
/// - any Windows `Prefix` component (drive letter / UNC / extended-length)
/// - any `RootDir` component (POSIX absolute root)
/// - empty paths
/// - overly long paths (defence in depth)
pub(crate) fn validate_archive_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(anyhow!("Archive contains empty path"));
    }

    if path.is_absolute() {
        return Err(anyhow!(
            "Archive contains absolute path (blocked): {:?}",
            path
        ));
    }

    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(anyhow!("Archive contains path traversal: {:?}", path));
            }
            Component::Prefix(_) => {
                return Err(anyhow!(
                    "Archive contains Windows path prefix (blocked): {:?}",
                    path
                ));
            }
            Component::RootDir => {
                return Err(anyhow!("Archive contains root component (blocked): {:?}", path));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }

    // Defence in depth: reject absurdly long paths that could trip Windows MAX_PATH
    // in code paths that don't convert to extended-length form.
    if path.as_os_str().len() > 4096 {
        return Err(anyhow!(
            "Archive contains path exceeding 4096 bytes: {}",
            path.display()
        ));
    }

    Ok(())
}

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

        // Archive-bomb accounting. These counters are the single source of truth; every
        // bail-out path must return an error before any write to disk.
        let mut entry_count: usize = 0;
        let mut cumulative_bytes: u64 = 0;

        while let Some(entry) = entries.next() {
            let mut entry = entry?;

            entry_count += 1;
            if entry_count > MAX_ENTRIES {
                return Err(anyhow!(
                    "Archive exceeds maximum entry count ({}): possible archive bomb",
                    MAX_ENTRIES
                ));
            }

            let header = entry.header().clone();

            // Get the entry path. `entry.path()` itself sanitizes `..` on some tar-crate
            // versions but we re-validate below to be sure.
            let mut path = entry.path()?.to_path_buf();
            let path_str = path.to_string_lossy();

            // GNU @LongLink: the "path" returned by the tar crate is a placeholder; the
            // real filename lives in this entry's body. An attacker controls those bytes.
            // Validate immediately, never defer.
            if path_str.contains("@LongLink") || path_str == "././@LongLink" {
                let declared_size = header.size().unwrap_or(0);
                if declared_size as usize > MAX_LONGLINK_BYTES {
                    return Err(anyhow!(
                        "Archive @LongLink entry declares {} bytes (max {})",
                        declared_size,
                        MAX_LONGLINK_BYTES
                    ));
                }

                let mut long_name_bytes = Vec::with_capacity(declared_size as usize);
                // Cap the read so a header-declared-small-but-body-huge mismatch can't DoS us.
                entry
                    .by_ref()
                    .take(MAX_LONGLINK_BYTES as u64 + 1)
                    .read_to_end(&mut long_name_bytes)?;

                if long_name_bytes.len() > MAX_LONGLINK_BYTES {
                    return Err(anyhow!(
                        "Archive @LongLink content exceeds {} bytes",
                        MAX_LONGLINK_BYTES
                    ));
                }

                // Remove null terminator.
                if let Some(null_pos) = long_name_bytes.iter().position(|&b| b == 0) {
                    long_name_bytes.truncate(null_pos);
                }

                let candidate = String::from_utf8_lossy(&long_name_bytes).into_owned();
                let candidate_path = PathBuf::from(&candidate);
                validate_archive_path(&candidate_path)?;

                long_name = Some(candidate);
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

            // Unified validation: this catches both the @LongLink-override and the
            // tar-crate-sanitized cases. Cheap, and the single source of truth.
            validate_archive_path(&path)?;

            // Build destination path
            let dest_path = dest_dir.join(&path);

            // Use extended paths on Windows for very long paths
            #[cfg(windows)]
            let dest_path = Self::to_extended_path(&dest_path);

            // Extract based on entry type.
            let entry_type = header.entry_type();
            if entry_type.is_file() {
                let declared_size = header.size().unwrap_or(0);
                if declared_size > MAX_FILE_SIZE {
                    return Err(anyhow!(
                        "Archive contains file exceeding {} bytes: {} ({} bytes)",
                        MAX_FILE_SIZE,
                        path.display(),
                        declared_size
                    ));
                }
                cumulative_bytes = cumulative_bytes.saturating_add(declared_size);
                if cumulative_bytes > MAX_TOTAL_SIZE {
                    return Err(anyhow!(
                        "Archive cumulative decompressed size exceeds {} bytes: possible archive bomb",
                        MAX_TOTAL_SIZE
                    ));
                }

                // Create parent directories just before writing so we never leave empty
                // dirs on the error path.
                if let Some(parent) = dest_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                // Read a bounded amount: defend against header-size-vs-body-size mismatches
                // where the body streams more bytes than the header claims.
                let limit = MAX_FILE_SIZE.saturating_add(1);
                let mut content = Vec::with_capacity(declared_size as usize);
                let n = entry.by_ref().take(limit).read_to_end(&mut content)?;
                if (n as u64) > MAX_FILE_SIZE {
                    return Err(anyhow!(
                        "Archive file body exceeds {} bytes: {}",
                        MAX_FILE_SIZE,
                        path.display()
                    ));
                }

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
                // Explicitly reject everything else. Silently skipping was defensible but
                // fragile: any future refactor that turns this branch into a symlink-write
                // re-opens the class of RCE. Hard-error instead.
                return Err(anyhow!(
                    "Archive contains disallowed entry type {:?} for {} (only regular files and directories are permitted)",
                    entry_type,
                    path.display()
                ));
            }
        }

        if crate::cli::verbosity::is_ultra_verbose() {
            info!(
                "Extraction complete: {} ({} entries, {} bytes)",
                dest_dir.display(),
                entry_count,
                cumulative_bytes
            );
        } else {
            debug!("Extraction complete: {}", dest_dir.display());
        }
        Ok(())
    }

    pub fn extract_package(archive_path: &Path, dest_dir: &Path) -> Result<()> {
        // The current default is the debloated extraction path. It internally
        // respects `HATCH_DEBLOAT=0` for rollback. The whole-archive path is
        // retained as `extract_tar_gz` and is used only by callers (e.g. tests)
        // that want the raw behaviour.
        let _stats = Self::extract_package_debloated(archive_path, dest_dir)?;
        Self::verify_package_structure(dest_dir)?;
        Ok(())
    }

    /// Flutter-aware debloated extraction. Returns per-package extraction
    /// statistics (bytes kept/stripped, entries kept/stripped).
    ///
    /// Reads the tarball fully into memory. Pub.dev packages are capped at
    /// 100 MB by convention and the extractor rejects anything larger via
    /// `MAX_TOTAL_SIZE`, so the RAM footprint is bounded.
    pub fn extract_package_debloated(
        archive_path: &Path,
        dest_dir: &Path,
    ) -> Result<super::package_manifest::DebloatStats> {
        if crate::cli::verbosity::is_ultra_verbose() {
            info!(
                "Extracting (debloated) {} to {}",
                archive_path.display(),
                dest_dir.display()
            );
        } else {
            debug!(
                "Extracting (debloated) {} to {}",
                archive_path.display(),
                dest_dir.display()
            );
        }

        let dest_dir = Self::to_extended_path(dest_dir);

        // Defence in depth: refuse to read absurdly large on-disk archives
        // into memory. pub.dev enforces a 100 MB limit; give 2x headroom.
        let metadata = std::fs::metadata(archive_path)
            .map_err(|e| anyhow!("Failed to stat archive {:?}: {}", archive_path, e))?;
        const MAX_TARBALL_BYTES: u64 = 200 * 1024 * 1024;
        if metadata.len() > MAX_TARBALL_BYTES {
            return Err(anyhow!(
                "Tarball {} is {} bytes, exceeds the {} byte limit",
                archive_path.display(),
                metadata.len(),
                MAX_TARBALL_BYTES
            ));
        }

        let bytes = std::fs::read(archive_path)
            .map_err(|e| anyhow!("Failed to read archive {:?}: {}", archive_path, e))?;

        let stats = super::package_manifest::extract_with_debloat(&bytes, &dest_dir)?;

        if crate::cli::verbosity::is_ultra_verbose() {
            info!(
                "Debloat: kept {} entries ({} bytes), stripped {} entries ({} bytes) in {}",
                stats.entries_kept,
                stats.bytes_kept,
                stats.entries_stripped,
                stats.bytes_stripped,
                dest_dir.display()
            );
        } else {
            debug!(
                "Debloat: kept {}/{} entries ({} bytes stripped)",
                stats.entries_kept,
                stats.entries_kept + stats.entries_stripped,
                stats.bytes_stripped
            );
        }

        Ok(stats)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_absolute_paths() {
        let err = validate_archive_path(Path::new("/etc/passwd")).unwrap_err();
        let msg = err.to_string().to_lowercase();
        // On POSIX `/etc/passwd` is flagged as absolute; on Windows the leading
        // `/` is a `RootDir` component (the path is not strictly absolute
        // without a drive prefix). Either rejection message is acceptable.
        assert!(
            msg.contains("absolute") || msg.contains("root") || msg.contains("blocked"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn rejects_parent_traversal() {
        let err =
            validate_archive_path(Path::new("../../etc/passwd")).unwrap_err();
        assert!(err.to_string().contains("traversal"));
    }

    #[test]
    fn rejects_parent_traversal_nested() {
        let err =
            validate_archive_path(Path::new("lib/../../../secret")).unwrap_err();
        assert!(err.to_string().contains("traversal"));
    }

    #[test]
    #[cfg(windows)]
    fn rejects_windows_drive_prefix() {
        let err =
            validate_archive_path(Path::new("C:\\Windows\\system32")).unwrap_err();
        let msg = err.to_string();
        // On Windows this is detected either as absolute or as a Prefix component.
        assert!(msg.contains("absolute") || msg.contains("prefix"));
    }

    #[test]
    fn accepts_normal_relative_path() {
        validate_archive_path(Path::new("lib/src/foo.dart")).unwrap();
        validate_archive_path(Path::new("pubspec.yaml")).unwrap();
        validate_archive_path(Path::new("./lib/foo.dart")).unwrap();
    }

    #[test]
    fn rejects_empty_path() {
        let err = validate_archive_path(Path::new("")).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn rejects_overlong_path() {
        let long = "a/".repeat(3000);
        let err = validate_archive_path(Path::new(&long)).unwrap_err();
        assert!(err.to_string().contains("4096"));
    }
}
