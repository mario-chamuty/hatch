use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Package metadata from registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMetadata {
    pub name: String,
    pub description: Option<String>,
    pub latest: PackageVersion,
    pub versions: Vec<PackageVersion>,
}

/// Package version information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageVersion {
    pub version: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub dependencies: HashMap<String, String>,
    pub dev_dependencies: HashMap<String, String>,
    pub published: Option<String>,
    pub dart_sdk: Option<String>,
    pub flutter_sdk: Option<String>,
}

/// Dependency resolution result
#[derive(Debug, Clone)]
pub struct ResolvedDependency {
    pub name: String,
    pub version: String,
    pub source: DependencySource,
    pub dependencies: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DependencySource {
    Registry { url: String },
    Git { url: String, ref_name: Option<String> },
    Path { path: String },
    Sdk { sdk: String }, // flutter, dart
}

/// Registry trait for different package sources
#[async_trait]
pub trait Registry: Send + Sync {
    /// Get package metadata
    async fn get_package_metadata(&self, name: &str) -> Result<PackageMetadata>;

    /// Get specific version metadata
    async fn get_version_metadata(&self, name: &str, version: &str) -> Result<PackageVersion>;

    /// Search for packages
    async fn search_packages(&self, query: &str) -> Result<Vec<PackageMetadata>>;

    /// Check if package exists
    async fn package_exists(&self, name: &str) -> Result<bool>;

    /// Get all available versions for a package
    async fn get_available_versions(&self, name: &str) -> Result<Vec<String>>;

    /// Get registry URL
    fn registry_url(&self) -> &str;

    /// Get registry name
    fn registry_name(&self) -> &str;
}

/// Version constraint types
#[derive(Debug, Clone, PartialEq)]
pub enum VersionConstraint {
    Any,
    Exact(String),
    Range { min: Option<String>, max: Option<String> },
    Caret(String), // ^1.2.3
    Tilde(String), // ~1.2.3
    GreaterThan(String),
    GreaterThanOrEqual(String),
    LessThan(String),
    LessThanOrEqual(String),
}

impl VersionConstraint {
    /// Parse version constraint from string
    pub fn parse(constraint: &str) -> Result<Self> {
        let constraint = constraint.trim();

        if constraint.is_empty() || constraint == "*" || constraint == "any" {
            return Ok(VersionConstraint::Any);
        }

        // Handle special case of pre-release constraints like "1.15.0-nnbd"
        // These should be treated as exact versions
        if constraint.contains("-") && !constraint.contains(" ") &&
           !constraint.starts_with('>') && !constraint.starts_with('<') {
            return Ok(VersionConstraint::Exact(constraint.to_string()));
        }

        if constraint.starts_with('^') {
            return Ok(VersionConstraint::Caret(constraint[1..].to_string()));
        }

        if constraint.starts_with('~') {
            return Ok(VersionConstraint::Tilde(constraint[1..].to_string()));
        }

        if constraint.starts_with(">=") {
            return Ok(VersionConstraint::GreaterThanOrEqual(constraint[2..].trim().to_string()));
        }

        if constraint.starts_with("<=") {
            return Ok(VersionConstraint::LessThanOrEqual(constraint[2..].trim().to_string()));
        }

        if constraint.starts_with('>') {
            return Ok(VersionConstraint::GreaterThan(constraint[1..].trim().to_string()));
        }

        if constraint.starts_with('<') {
            return Ok(VersionConstraint::LessThan(constraint[1..].trim().to_string()));
        }

        // Handle constraints with spaces like ">= 1.0.0"
        let normalized = constraint.replace(">= ", ">=")
                                  .replace("<= ", "<=")
                                  .replace("> ", ">")
                                  .replace("< ", "<");

        if normalized != constraint {
            return Self::parse(&normalized);
        }

        // Handle range constraints like ">=1.0.0 <2.0.0"
        if constraint.contains(' ') {
            let parts: Vec<&str> = constraint.split_whitespace().collect();
            if parts.len() == 2 {
                // Check for impossible constraints like ">=1.15.0-nnbd <1.15.0"
                // This is impossible because 1.15.0-nnbd is > 1.15.0
                if parts[0].starts_with(">=") && parts[1].starts_with("<") {
                    let min_ver = parts[0].trim_start_matches(">=").trim();
                    let max_ver = parts[1].trim_start_matches("<").trim();

                    // If min version has pre-release and max doesn't, relax to just >= min
                    if min_ver.contains("-") && !max_ver.contains("-") {
                        if let Ok(min_parsed) = semver::Version::parse(min_ver) {
                            if let Ok(max_parsed) = semver::Version::parse(max_ver) {
                                if min_parsed >= max_parsed {
                                    // Impossible constraint, relax to any version >= base
                                    return Ok(VersionConstraint::GreaterThanOrEqual(
                                        format!("{}.{}.{}", max_parsed.major, max_parsed.minor, max_parsed.patch)
                                    ));
                                }
                            }
                        }
                    }
                }

                let min_constraint = Self::parse(parts[0])?;
                let max_constraint = Self::parse(parts[1])?;

                let min = match min_constraint {
                    VersionConstraint::GreaterThanOrEqual(v) => Some(v),
                    VersionConstraint::GreaterThan(v) => Some(v),
                    _ => None,
                };

                let max = match max_constraint {
                    VersionConstraint::LessThan(v) => Some(v),
                    VersionConstraint::LessThanOrEqual(v) => Some(v),
                    _ => None,
                };

                return Ok(VersionConstraint::Range { min, max });
            }
        }

        // Default to exact version
        Ok(VersionConstraint::Exact(constraint.to_string()))
    }

    /// Check if a version satisfies this constraint
    pub fn satisfies(&self, version: &str) -> bool {
        use semver::Version;

        let Ok(ver) = Version::parse(version) else {
            return false;
        };

        match self {
            VersionConstraint::Any => true,
            VersionConstraint::Exact(exact) => {
                Version::parse(exact).map(|v| v == ver).unwrap_or(false)
            },
            VersionConstraint::Caret(base) => {
                if let Ok(base_ver) = Version::parse(base) {
                    ver >= base_ver && ver.major == base_ver.major
                } else {
                    false
                }
            },
            VersionConstraint::Tilde(base) => {
                if let Ok(base_ver) = Version::parse(base) {
                    ver >= base_ver && ver.major == base_ver.major && ver.minor == base_ver.minor
                } else {
                    false
                }
            },
            VersionConstraint::GreaterThan(base) => {
                Version::parse(base).map(|v| ver > v).unwrap_or(false)
            },
            VersionConstraint::GreaterThanOrEqual(base) => {
                Version::parse(base).map(|v| ver >= v).unwrap_or(false)
            },
            VersionConstraint::LessThan(base) => {
                Version::parse(base).map(|v| ver < v).unwrap_or(false)
            },
            VersionConstraint::LessThanOrEqual(base) => {
                Version::parse(base).map(|v| ver <= v).unwrap_or(false)
            },
            VersionConstraint::Range { min, max } => {
                let min_ok = min.as_ref()
                    .map(|m| {
                        Version::parse(m).map(|min_ver| {
                            let result = ver >= min_ver;
                            if !result && version.starts_with("1.") {
                                eprintln!("Range check: {} >= {} = false", ver, min_ver);
                            }
                            result
                        }).unwrap_or(false)
                    })
                    .unwrap_or(true);

                let max_ok = max.as_ref()
                    .map(|m| {
                        Version::parse(m).map(|max_ver| {
                            let result = ver < max_ver;
                            if !result && version.starts_with("1.") {
                                eprintln!("Range check: {} < {} = false", ver, max_ver);
                            }
                            result
                        }).unwrap_or(false)
                    })
                    .unwrap_or(true);

                min_ok && max_ok
            },
        }
    }
}