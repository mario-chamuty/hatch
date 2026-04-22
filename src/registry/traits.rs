use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Bound;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_sha256: Option<String>,
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
    /// Bounded range. Legacy callers built via struct literal get an
    /// exclusive max; use the `range_*` helpers below when constructing
    /// new instances so inclusive-max semantics round-trip.
    Range {
        min: Option<String>,
        max: Option<String>,
        /// Whether the lower bound is inclusive (`>=`) vs exclusive (`>`).
        #[doc(hidden)]
        min_inclusive: bool,
        /// Whether the upper bound is inclusive (`<=`) vs exclusive (`<`).
        /// Pre-existing bug fix: ">=1.0.0 <=2.0.0" used to lose the
        /// inclusive-max on round-trip because the variant had no slot
        /// for this flag.
        #[doc(hidden)]
        max_inclusive: bool,
    },
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

        // IMPORTANT: Handle range constraints FIRST, before single-prefix checks.
        // ">=1.0.0 <2.0.0" must be parsed as a Range, not as GreaterThanOrEqual("1.0.0 <2.0.0").
        if constraint.contains(' ') {
            // Normalize spaces around operators: ">= 1.0.0" -> ">=1.0.0"
            let normalized = constraint.replace(">= ", ">=")
                                      .replace("<= ", "<=")
                                      .replace("> ", ">")
                                      .replace("< ", "<");

            let parts: Vec<&str> = normalized.split_whitespace().collect();
            if parts.len() == 2 {
                // Check for impossible constraints like ">=1.15.0-nnbd <1.15.0"
                if parts[0].starts_with(">=") && parts[1].starts_with("<") {
                    let min_ver = parts[0].trim_start_matches(">=").trim();
                    let max_ver = parts[1].trim_start_matches("<").trim();

                    // If min version has pre-release and max doesn't, relax to just >= min
                    if min_ver.contains("-") && !max_ver.contains("-") {
                        if let Ok(min_parsed) = semver::Version::parse(min_ver) {
                            if let Ok(max_parsed) = semver::Version::parse(max_ver) {
                                if min_parsed >= max_parsed {
                                    return Ok(VersionConstraint::GreaterThanOrEqual(
                                        format!("{}.{}.{}", max_parsed.major, max_parsed.minor, max_parsed.patch)
                                    ));
                                }
                            }
                        }
                    }
                }

                // Parse each part individually (they won't contain spaces, so no recursion issues)
                let min_constraint = Self::parse(parts[0])?;
                let max_constraint = Self::parse(parts[1])?;

                let (min, min_inclusive) = match min_constraint {
                    VersionConstraint::GreaterThanOrEqual(v) => (Some(v), true),
                    VersionConstraint::GreaterThan(v) => (Some(v), false),
                    _ => (None, true),
                };

                let (max, max_inclusive) = match max_constraint {
                    VersionConstraint::LessThan(v) => (Some(v), false),
                    VersionConstraint::LessThanOrEqual(v) => (Some(v), true),
                    _ => (None, false),
                };

                return Ok(VersionConstraint::Range {
                    min,
                    max,
                    min_inclusive,
                    max_inclusive,
                });
            }
            // If more than 2 parts, fall through to single-prefix parsing
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

        // Default to exact version
        Ok(VersionConstraint::Exact(constraint.to_string()))
    }

    /// Check if a version satisfies this constraint.
    ///
    /// Delegates to the interval representation so the interactive `satisfies`
    /// path and the batch `ParsedConstraint::contains` path cannot drift.
    pub fn satisfies(&self, version: &str) -> bool {
        let Ok(ver) = semver::Version::parse(version) else {
            return false;
        };
        match self.to_parsed() {
            Ok(p) => p.contains(&ver),
            Err(_) => false,
        }
    }

    /// Lift a `VersionConstraint` to an interval representation suitable for
    /// bulk queries. This is the single-step parse performed by the resolver
    /// so downstream hot loops never re-parse version strings.
    ///
    /// Dart/pub caret semantics: `^1.2.3` -> `>=1.2.3, <2.0.0`. For 0.x bases,
    /// the minor component is treated as breaking: `^0.1.2` -> `>=0.1.2, <0.2.0`,
    /// `^0.0.1` -> `>=0.0.1, <0.0.2`.
    pub fn to_parsed(&self) -> Result<ParsedConstraint> {
        use semver::Version;

        match self {
            VersionConstraint::Any => Ok(ParsedConstraint::any()),
            VersionConstraint::Exact(v) => {
                let ver = Version::parse(v)
                    .map_err(|e| anyhow!("invalid exact version '{}': {}", v, e))?;
                let allow_pre = !ver.pre.is_empty();
                Ok(ParsedConstraint {
                    lo: Bound::Included(ver.clone()),
                    hi: Bound::Included(ver),
                    allow_prerelease: allow_pre,
                })
            }
            VersionConstraint::Caret(base) => {
                let base_ver = Version::parse(base)
                    .map_err(|e| anyhow!("invalid caret base '{}': {}", base, e))?;
                let hi = caret_upper_bound(&base_ver);
                let allow_pre = !base_ver.pre.is_empty();
                Ok(ParsedConstraint {
                    lo: Bound::Included(base_ver),
                    hi: Bound::Excluded(hi),
                    allow_prerelease: allow_pre,
                })
            }
            VersionConstraint::Tilde(base) => {
                let base_ver = Version::parse(base)
                    .map_err(|e| anyhow!("invalid tilde base '{}': {}", base, e))?;
                // ~1.2.3 -> >=1.2.3, <1.3.0
                let hi = Version::new(base_ver.major, base_ver.minor + 1, 0);
                let allow_pre = !base_ver.pre.is_empty();
                Ok(ParsedConstraint {
                    lo: Bound::Included(base_ver),
                    hi: Bound::Excluded(hi),
                    allow_prerelease: allow_pre,
                })
            }
            VersionConstraint::GreaterThan(base) => {
                let ver = Version::parse(base)
                    .map_err(|e| anyhow!("invalid '>' base '{}': {}", base, e))?;
                let allow_pre = !ver.pre.is_empty();
                Ok(ParsedConstraint {
                    lo: Bound::Excluded(ver),
                    hi: Bound::Unbounded,
                    allow_prerelease: allow_pre,
                })
            }
            VersionConstraint::GreaterThanOrEqual(base) => {
                let ver = Version::parse(base)
                    .map_err(|e| anyhow!("invalid '>=' base '{}': {}", base, e))?;
                let allow_pre = !ver.pre.is_empty();
                Ok(ParsedConstraint {
                    lo: Bound::Included(ver),
                    hi: Bound::Unbounded,
                    allow_prerelease: allow_pre,
                })
            }
            VersionConstraint::LessThan(base) => {
                let ver = Version::parse(base)
                    .map_err(|e| anyhow!("invalid '<' base '{}': {}", base, e))?;
                Ok(ParsedConstraint {
                    lo: Bound::Unbounded,
                    hi: Bound::Excluded(ver),
                    allow_prerelease: false,
                })
            }
            VersionConstraint::LessThanOrEqual(base) => {
                let ver = Version::parse(base)
                    .map_err(|e| anyhow!("invalid '<=' base '{}': {}", base, e))?;
                let allow_pre = !ver.pre.is_empty();
                Ok(ParsedConstraint {
                    lo: Bound::Unbounded,
                    hi: Bound::Included(ver),
                    allow_prerelease: allow_pre,
                })
            }
            VersionConstraint::Range {
                min,
                max,
                min_inclusive,
                max_inclusive,
            } => {
                let (lo, lo_pre) = match min {
                    Some(m) => {
                        let v = Version::parse(m)
                            .map_err(|e| anyhow!("invalid range min '{}': {}", m, e))?;
                        let has_pre = !v.pre.is_empty();
                        let b = if *min_inclusive {
                            Bound::Included(v)
                        } else {
                            Bound::Excluded(v)
                        };
                        (b, has_pre)
                    }
                    None => (Bound::Unbounded, false),
                };
                let hi = match max {
                    Some(m) => {
                        let v = Version::parse(m)
                            .map_err(|e| anyhow!("invalid range max '{}': {}", m, e))?;
                        if *max_inclusive {
                            Bound::Included(v)
                        } else {
                            Bound::Excluded(v)
                        }
                    }
                    None => Bound::Unbounded,
                };
                Ok(ParsedConstraint {
                    lo,
                    hi,
                    allow_prerelease: lo_pre,
                })
            }
        }
    }
}

/// Compute the pub-style caret upper bound for `^<base>`.
///
/// - base major >= 1: bump major, zero minor/patch
/// - base major == 0, minor >= 1: bump minor, zero patch
/// - base 0.0.x: bump patch
fn caret_upper_bound(base: &semver::Version) -> semver::Version {
    if base.major >= 1 {
        semver::Version::new(base.major + 1, 0, 0)
    } else if base.minor >= 1 {
        semver::Version::new(0, base.minor + 1, 0)
    } else {
        semver::Version::new(0, 0, base.patch + 1)
    }
}

/// Interval representation of a version constraint.
///
/// Produced once by [`VersionConstraint::to_parsed`] and then used in hot
/// loops (`VersionIndex::matching`, `VersionIndex::latest_matching`) to avoid
/// re-parsing version strings on every comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedConstraint {
    pub lo: Bound<semver::Version>,
    pub hi: Bound<semver::Version>,
    /// Whether versions carrying a prerelease tag are acceptable. Pub-style
    /// semantics: a prerelease version is rejected unless the constraint's
    /// lower bound itself specified a prerelease.
    pub allow_prerelease: bool,
}

impl ParsedConstraint {
    /// Unbounded "accept anything" constraint (equivalent to `*` / `any`).
    pub fn any() -> Self {
        Self {
            lo: Bound::Unbounded,
            hi: Bound::Unbounded,
            allow_prerelease: false,
        }
    }

    /// Returns `true` when `v` is inside the interval and passes the
    /// prerelease filter.
    pub fn contains(&self, v: &semver::Version) -> bool {
        // Prerelease filter: if the version has a pre tag and we are not
        // explicitly allowing prereleases, reject. (Mirrors pub/cargo.)
        if !v.pre.is_empty() && !self.allow_prerelease {
            return false;
        }

        let lo_ok = match &self.lo {
            Bound::Unbounded => true,
            Bound::Included(lo) => v >= lo,
            Bound::Excluded(lo) => v > lo,
        };
        if !lo_ok {
            return false;
        }

        let hi_ok = match &self.hi {
            Bound::Unbounded => true,
            Bound::Included(hi) => v <= hi,
            Bound::Excluded(hi) => v < hi,
        };
        hi_ok
    }

    /// Intersect two constraints. Returns `None` when the overlap is empty.
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let lo = intersect_lo(&self.lo, &other.lo);
        let hi = intersect_hi(&self.hi, &other.hi);

        if !interval_non_empty(&lo, &hi) {
            return None;
        }

        Some(Self {
            lo,
            hi,
            allow_prerelease: self.allow_prerelease || other.allow_prerelease,
        })
    }
}

fn intersect_lo(
    a: &Bound<semver::Version>,
    b: &Bound<semver::Version>,
) -> Bound<semver::Version> {
    match (a, b) {
        (Bound::Unbounded, x) | (x, Bound::Unbounded) => x.clone(),
        (Bound::Included(va), Bound::Included(vb)) => {
            if va >= vb {
                Bound::Included(va.clone())
            } else {
                Bound::Included(vb.clone())
            }
        }
        (Bound::Excluded(va), Bound::Excluded(vb)) => {
            if va >= vb {
                Bound::Excluded(va.clone())
            } else {
                Bound::Excluded(vb.clone())
            }
        }
        (Bound::Included(vi), Bound::Excluded(ve))
        | (Bound::Excluded(ve), Bound::Included(vi)) => {
            if ve >= vi {
                Bound::Excluded(ve.clone())
            } else {
                Bound::Included(vi.clone())
            }
        }
    }
}

fn intersect_hi(
    a: &Bound<semver::Version>,
    b: &Bound<semver::Version>,
) -> Bound<semver::Version> {
    match (a, b) {
        (Bound::Unbounded, x) | (x, Bound::Unbounded) => x.clone(),
        (Bound::Included(va), Bound::Included(vb)) => {
            if va <= vb {
                Bound::Included(va.clone())
            } else {
                Bound::Included(vb.clone())
            }
        }
        (Bound::Excluded(va), Bound::Excluded(vb)) => {
            if va <= vb {
                Bound::Excluded(va.clone())
            } else {
                Bound::Excluded(vb.clone())
            }
        }
        (Bound::Included(vi), Bound::Excluded(ve))
        | (Bound::Excluded(ve), Bound::Included(vi)) => {
            if ve <= vi {
                Bound::Excluded(ve.clone())
            } else {
                Bound::Included(vi.clone())
            }
        }
    }
}

fn interval_non_empty(
    lo: &Bound<semver::Version>,
    hi: &Bound<semver::Version>,
) -> bool {
    match (lo, hi) {
        (Bound::Unbounded, _) | (_, Bound::Unbounded) => true,
        (Bound::Included(a), Bound::Included(b)) => a <= b,
        (Bound::Included(a), Bound::Excluded(b)) => a < b,
        (Bound::Excluded(a), Bound::Included(b)) => a < b,
        (Bound::Excluded(a), Bound::Excluded(b)) => a < b,
    }
}