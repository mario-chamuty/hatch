use anyhow::{anyhow, Result};
use semver::Version;
use std::cmp::Ordering;

/// Version utility functions for dependency resolution
pub struct VersionUtils;

impl VersionUtils {
    /// Compare two version strings using semver
    pub fn compare(v1: &str, v2: &str) -> Result<Ordering> {
        let ver1 = Version::parse(v1)
            .map_err(|e| anyhow!("Invalid version '{}': {}", v1, e))?;
        let ver2 = Version::parse(v2)
            .map_err(|e| anyhow!("Invalid version '{}': {}", v2, e))?;

        Ok(ver1.cmp(&ver2))
    }

    /// Get the latest version from a list of version strings
    pub fn get_latest(versions: &[String]) -> Result<Option<String>> {
        if versions.is_empty() {
            return Ok(None);
        }

        let mut parsed_versions: Vec<(Version, String)> = Vec::new();

        for version_str in versions {
            if let Ok(version) = Version::parse(version_str) {
                parsed_versions.push((version, version_str.clone()));
            }
        }

        if parsed_versions.is_empty() {
            return Ok(None);
        }

        parsed_versions.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(Some(parsed_versions[0].1.clone()))
    }

    /// Sort versions in descending order (newest first)
    pub fn sort_versions(versions: &mut Vec<String>) -> Result<()> {
        let mut parsed_versions: Vec<(Version, String)> = Vec::new();

        for version_str in versions.iter() {
            if let Ok(version) = Version::parse(version_str) {
                parsed_versions.push((version, version_str.clone()));
            }
        }

        parsed_versions.sort_by(|a, b| b.0.cmp(&a.0));
        *versions = parsed_versions.into_iter().map(|(_, s)| s).collect();
        Ok(())
    }

    /// Check if a version is a pre-release
    pub fn is_prerelease(version: &str) -> bool {
        if let Ok(ver) = Version::parse(version) {
            !ver.pre.is_empty()
        } else {
            false
        }
    }

    /// Check if a version satisfies a caret constraint (^1.2.3)
    pub fn satisfies_caret(version: &str, base: &str) -> bool {
        let Ok(ver) = Version::parse(version) else { return false; };
        let Ok(base_ver) = Version::parse(base) else { return false; };

        ver >= base_ver && ver.major == base_ver.major
    }

    /// Check if a version satisfies a tilde constraint (~1.2.3)
    pub fn satisfies_tilde(version: &str, base: &str) -> bool {
        let Ok(ver) = Version::parse(version) else { return false; };
        let Ok(base_ver) = Version::parse(base) else { return false; };

        ver >= base_ver && ver.major == base_ver.major && ver.minor == base_ver.minor
    }

    /// Parse and normalize version string
    pub fn normalize_version(version: &str) -> Result<String> {
        let parsed = Version::parse(version)
            .map_err(|e| anyhow!("Invalid version '{}': {}", version, e))?;
        Ok(parsed.to_string())
    }

    /// Extract major.minor.patch from version string
    pub fn get_major_minor_patch(version: &str) -> Result<(u64, u64, u64)> {
        let parsed = Version::parse(version)
            .map_err(|e| anyhow!("Invalid version '{}': {}", version, e))?;
        Ok((parsed.major, parsed.minor, parsed.patch))
    }

    /// Check if version is stable (no prerelease/build metadata)
    pub fn is_stable(version: &str) -> bool {
        if let Ok(ver) = Version::parse(version) {
            ver.pre.is_empty() && ver.build.is_empty()
        } else {
            false
        }
    }

    /// Get the next major version
    pub fn next_major(version: &str) -> Result<String> {
        let mut ver = Version::parse(version)
            .map_err(|e| anyhow!("Invalid version '{}': {}", version, e))?;
        ver.major += 1;
        ver.minor = 0;
        ver.patch = 0;
        ver.pre = semver::Prerelease::EMPTY;
        ver.build = semver::BuildMetadata::EMPTY;
        Ok(ver.to_string())
    }

    /// Get the next minor version
    pub fn next_minor(version: &str) -> Result<String> {
        let mut ver = Version::parse(version)
            .map_err(|e| anyhow!("Invalid version '{}': {}", version, e))?;
        ver.minor += 1;
        ver.patch = 0;
        ver.pre = semver::Prerelease::EMPTY;
        ver.build = semver::BuildMetadata::EMPTY;
        Ok(ver.to_string())
    }

    /// Get the next patch version
    pub fn next_patch(version: &str) -> Result<String> {
        let mut ver = Version::parse(version)
            .map_err(|e| anyhow!("Invalid version '{}': {}", version, e))?;
        ver.patch += 1;
        ver.pre = semver::Prerelease::EMPTY;
        ver.build = semver::BuildMetadata::EMPTY;
        Ok(ver.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparison() {
        assert_eq!(VersionUtils::compare("1.0.0", "2.0.0").unwrap(), Ordering::Less);
        assert_eq!(VersionUtils::compare("2.0.0", "1.0.0").unwrap(), Ordering::Greater);
        assert_eq!(VersionUtils::compare("1.0.0", "1.0.0").unwrap(), Ordering::Equal);
    }

    #[test]
    fn test_get_latest() {
        let versions = vec!["1.0.0".to_string(), "1.2.0".to_string(), "1.1.0".to_string()];
        let latest = VersionUtils::get_latest(&versions).unwrap();
        assert_eq!(latest, Some("1.2.0".to_string()));
    }

    #[test]
    fn test_caret_constraint() {
        assert!(VersionUtils::satisfies_caret("1.2.0", "1.0.0"));
        assert!(VersionUtils::satisfies_caret("1.9.9", "1.0.0"));
        assert!(!VersionUtils::satisfies_caret("2.0.0", "1.0.0"));
        assert!(!VersionUtils::satisfies_caret("0.9.0", "1.0.0"));
    }

    #[test]
    fn test_tilde_constraint() {
        assert!(VersionUtils::satisfies_tilde("1.2.5", "1.2.0"));
        assert!(!VersionUtils::satisfies_tilde("1.3.0", "1.2.0"));
        assert!(!VersionUtils::satisfies_tilde("2.2.0", "1.2.0"));
    }

    #[test]
    fn test_is_prerelease() {
        assert!(VersionUtils::is_prerelease("1.0.0-alpha"));
        assert!(VersionUtils::is_prerelease("1.0.0-beta.1"));
        assert!(!VersionUtils::is_prerelease("1.0.0"));
    }

    #[test]
    fn test_next_versions() {
        assert_eq!(VersionUtils::next_major("1.2.3").unwrap(), "2.0.0");
        assert_eq!(VersionUtils::next_minor("1.2.3").unwrap(), "1.3.0");
        assert_eq!(VersionUtils::next_patch("1.2.3").unwrap(), "1.2.4");
    }
}