//! Pubgrub-backed solver for version resolution.
//!
//! Implements `pubgrub::solver::DependencyProvider` against our async
//! [`MetadataCache`]. The top-level entry point (`solve`) is async and must
//! be called from outside a tokio current-thread context – it drives pubgrub
//! inside `tokio::task::spawn_blocking` while forwarding metadata lookups
//! back onto the caller's `Handle` via `block_on`.
//!
//! Versions are modelled by the [`SemverVersion`] newtype: pubgrub's
//! `Version` trait requires `Ord + bump() + lowest()`, none of which map
//! cleanly onto semver's prerelease ordering semantics. The wrapper uses
//! lexicographic `Ord` from `semver::Version` (which already orders
//! `1.0.0-alpha < 1.0.0`) and implements `bump` by incrementing the build
//! counter – pubgrub never asks us to materialise the bumped value.

use anyhow::Result;
use log::debug;
use pubgrub::range::Range;
use pubgrub::report::{DefaultStringReporter, Reporter};
use pubgrub::solver::{Dependencies, DependencyProvider};
use pubgrub::version::Version as PubgrubVersion;
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::error::Error as StdError;
use std::fmt;
use std::ops::Bound;
use std::sync::Arc;

use crate::cache::metadata_cache::MetadataCache;
use crate::manifest::schema::SdkConstraints;
use crate::registry::traits::{ParsedConstraint, VersionConstraint};
use crate::resolver::error::ResolverError;
use crate::resolver::propagation::{is_sdk_pseudo_package, Overrides};

/// A version package name plus optional SDK marker. Pubgrub keys on the
/// name only; we keep `String` throughout.
pub type PkgName = String;

/// Newtype around `semver::Version` so we can implement pubgrub's
/// [`Version`] trait without touching the upstream crate.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SemverVersion(pub semver::Version);

impl Ord for SemverVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for SemverVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for SemverVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PubgrubVersion for SemverVersion {
    fn lowest() -> Self {
        SemverVersion(semver::Version::new(0, 0, 0))
    }
    fn bump(&self) -> Self {
        // Pubgrub uses bump() to exclude a single point. For semver we bump
        // the patch; the wrapper is monotone so this satisfies the trait.
        let mut next = self.0.clone();
        next.patch += 1;
        next.pre = semver::Prerelease::EMPTY;
        next.build = semver::BuildMetadata::EMPTY;
        SemverVersion(next)
    }
}

/// Convert a [`ParsedConstraint`] interval into a pubgrub [`Range`].
pub fn to_range(c: &ParsedConstraint) -> Range<SemverVersion> {
    let lo_opt: Option<SemverVersion> = match &c.lo {
        Bound::Unbounded => None,
        Bound::Included(v) => Some(SemverVersion(v.clone())),
        Bound::Excluded(v) => Some(SemverVersion(v.clone()).bump()),
    };
    let hi_opt: Option<SemverVersion> = match &c.hi {
        Bound::Unbounded => None,
        Bound::Excluded(v) => Some(SemverVersion(v.clone())),
        Bound::Included(v) => Some(SemverVersion(v.clone()).bump()),
    };
    match (lo_opt, hi_opt) {
        (None, None) => Range::any(),
        (Some(lo), None) => Range::higher_than(lo),
        (None, Some(hi)) => Range::strictly_lower_than(hi),
        (Some(lo), Some(hi)) => {
            if lo < hi {
                Range::between(lo, hi)
            } else {
                Range::none()
            }
        }
    }
}

/// Dependency provider backing the pubgrub solver.
pub struct HatchProvider {
    pub cache: Arc<MetadataCache>,
    pub sdks: SdkConstraints,
    pub overrides: Overrides,
    pub locked: HashMap<String, semver::Version>,
    pub handle: tokio::runtime::Handle,
}

impl HatchProvider {
    // Why: pubgrub's `add_derivation` panics (partial_solution.rs:131) if
    // `get_dependencies`/`versions_of` return state inconsistent with an
    // earlier decision. Mutating the metadata cache mid-solve – by fetching
    // fresh pub.dev metadata whose transitive constraints contradict a
    // package the solver already decided on – trips that invariant. The
    // caller (`Resolver::resolve` via `incremental_fetch`) primes the cache
    // up front; anything the solver then discovers beyond that closure is a
    // dead end and must be reported as `Dependencies::Unknown` so pubgrub
    // backtracks cleanly instead of receiving surprise constraints on
    // already-decided packages.
    fn versions_of(&self, pkg: &str) -> Vec<semver::Version> {
        let handle = self.handle.clone();
        let cache = self.cache.clone();
        let pkg = pkg.to_string();
        handle.block_on(async move {
            let Some(idx) = cache.get_index(&pkg).await else {
                return Vec::new();
            };
            let mut out: Vec<semver::Version> = Vec::with_capacity(idx.len());
            for i in 0..idx.len() {
                if let Some(v) = idx.version(i) {
                    out.push(v.clone());
                }
            }
            out
        })
    }

    fn deps_of(
        &self,
        pkg: &str,
        version: &semver::Version,
    ) -> Option<HashMap<String, String>> {
        let handle = self.handle.clone();
        let cache = self.cache.clone();
        let pkg = pkg.to_string();
        let wanted = version.to_string();
        handle.block_on(async move {
            let versions = cache.get(&pkg).await?;
            versions
                .iter()
                .find(|v| v.version == wanted)
                .map(|v| v.dependencies.clone())
        })
    }
}

impl DependencyProvider<PkgName, SemverVersion> for HatchProvider {
    fn choose_package_version<T: Borrow<PkgName>, U: Borrow<Range<SemverVersion>>>(
        &self,
        potential_packages: impl Iterator<Item = (T, U)>,
    ) -> Result<(T, Option<SemverVersion>), Box<dyn StdError>> {
        // Fail-fast heuristic: pick the package with the fewest candidates
        // that still match its current range.
        let mut best: Option<(T, Option<SemverVersion>, usize)> = None;

        for (pkg_borrow, range_borrow) in potential_packages {
            let pkg = pkg_borrow.borrow().clone();
            let range = range_borrow.borrow();

            // SDK pseudo packages: synthesise a single version so pubgrub
            // sees them as pinned.
            if is_sdk_pseudo_package(&pkg) {
                let pinned = SemverVersion(semver::Version::new(0, 0, 0));
                return Ok((pkg_borrow, Some(pinned)));
            }

            let versions = self.versions_of(&pkg);
            let in_range: Vec<SemverVersion> = versions
                .into_iter()
                .map(SemverVersion)
                .filter(|v| range.contains(v))
                .collect();

            let candidate_count = in_range.len();

            // Warm-start bias: prefer the locked version if it is still in
            // range.
            let locked_match = self
                .locked
                .get(&pkg)
                .map(|v| SemverVersion(v.clone()))
                .filter(|v| range.contains(v));

            let chosen = locked_match
                .or_else(|| in_range.last().cloned());

            let replace = match &best {
                None => true,
                Some((_, _, c)) => candidate_count < *c,
            };
            if replace {
                best = Some((pkg_borrow, chosen, candidate_count));
            }
        }

        let (pkg, ver, _count) = best
            .ok_or_else(|| Box::<dyn StdError>::from("no potential packages"))?;
        Ok((pkg, ver))
    }

    fn get_dependencies(
        &self,
        package: &PkgName,
        version: &SemverVersion,
    ) -> Result<Dependencies<PkgName, SemverVersion>, Box<dyn StdError>> {
        // SDK pseudo packages have no transitive deps.
        if is_sdk_pseudo_package(package) {
            return Ok(Dependencies::Known(BTreeMap::new().into_iter().collect()));
        }

        let Some(deps_raw) = self.deps_of(package, &version.0) else {
            return Ok(Dependencies::Unknown);
        };

        let mut out: BTreeMap<PkgName, Range<SemverVersion>> = BTreeMap::new();
        for (dep_name, dep_constraint) in &deps_raw {
            if is_sdk_pseudo_package(dep_name) {
                // Model SDK as trivially satisfied.
                out.insert(dep_name.clone(), Range::any());
                continue;
            }
            let parsed = match VersionConstraint::parse(dep_constraint)
                .and_then(|c| c.to_parsed())
            {
                Ok(p) => p,
                Err(_) => continue,
            };
            out.insert(dep_name.clone(), to_range(&parsed));
        }
        Ok(Dependencies::Known(out.into_iter().collect()))
    }
}

/// Format a pubgrub derivation tree into a pub-style explanation.
pub fn format_conflict(
    mut tree: pubgrub::report::DerivationTree<PkgName, SemverVersion>,
) -> String {
    tree.collapse_no_versions();
    DefaultStringReporter::report(&tree)
}

/// Resolve a manifest. Spawns pubgrub inside `spawn_blocking` so the sync
/// solver can call back into async metadata lookups without deadlocking the
/// current-thread runtime.
pub async fn solve(
    cache: Arc<MetadataCache>,
    sdks: SdkConstraints,
    overrides: Overrides,
    locked: HashMap<String, semver::Version>,
    root_name: String,
    root_version: semver::Version,
    root_deps: HashMap<String, ParsedConstraint>,
) -> Result<HashMap<String, semver::Version>, ResolverError> {
    let handle = tokio::runtime::Handle::current();

    // Register a synthetic root in the cache so pubgrub can call
    // get_dependencies on it.
    let root_pkg_version = crate::registry::traits::PackageVersion {
        version: root_version.to_string(),
        description: None,
        homepage: None,
        repository: None,
        dependencies: root_deps
            .iter()
            .map(|(k, v)| (k.clone(), parsed_to_display(v)))
            .collect(),
        dev_dependencies: std::collections::HashMap::new(),
        published: None,
        dart_sdk: None,
        flutter_sdk: None,
        archive_sha256: None,
    };
    cache
        .insert(root_name.clone(), vec![root_pkg_version])
        .await;

    let provider = HatchProvider {
        cache: cache.clone(),
        sdks,
        overrides,
        locked,
        handle: handle.clone(),
    };

    let root_name_clone = root_name.clone();
    let root_ver_clone = root_version.clone();

    // pubgrub::error::PubGrubError contains a `Box<dyn StdError>` which is
    // not `Send` in pubgrub 0.2; we can't return it across a
    // `spawn_blocking` boundary. Fold the result into a Send-friendly shape
    // inside the blocking task.
    enum SolveOutcome {
        Ok(HashMap<String, semver::Version>),
        NoSolution(String),
        Other(String),
    }

    let outcome = tokio::task::spawn_blocking(move || {
        let root_name_inner = root_name_clone.clone();
        // Catch panics from pubgrub 0.2 internals so our callers see a clean
        // Io error instead of a worker-thread panic propagating up through
        // tokio. This is a safety net for the `add_derivation should not be
        // called after a decision` invariant; our input normalisation should
        // prevent it from firing but we don't want the whole resolver to
        // crash if it does.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pubgrub::solver::resolve(
                &provider,
                root_name_clone,
                SemverVersion(root_ver_clone),
            )
        }));
        let resolve_result = match result {
            Ok(r) => r,
            Err(panic) => {
                let msg = if let Some(s) = panic.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = panic.downcast_ref::<&str>() {
                    s.to_string()
                } else {
                    "unknown pubgrub panic".to_string()
                };
                return SolveOutcome::Other(format!("pubgrub panic: {msg}"));
            }
        };
        match resolve_result {
            Ok(solution) => {
                let mut out = HashMap::new();
                for (pkg, ver) in solution {
                    if pkg == root_name_inner {
                        continue;
                    }
                    if is_sdk_pseudo_package(&pkg) {
                        continue;
                    }
                    out.insert(pkg, ver.0);
                }
                SolveOutcome::Ok(out)
            }
            Err(pubgrub::error::PubGrubError::NoSolution(tree)) => {
                SolveOutcome::NoSolution(format_conflict(tree))
            }
            Err(e) => SolveOutcome::Other(format!("{e}")),
        }
    })
    .await
    .map_err(|e| ResolverError::Io(format!("pubgrub task join failed: {e}")))?;

    match outcome {
        SolveOutcome::Ok(map) => Ok(map),
        SolveOutcome::NoSolution(msg) => Err(ResolverError::NoSolution(msg)),
        SolveOutcome::Other(msg) => {
            debug!("pubgrub error: {msg}");
            Err(ResolverError::Io(msg))
        }
    }
}

fn parsed_to_display(c: &ParsedConstraint) -> String {
    // Lossless-ish display purely for cache round-tripping. We don't parse
    // this back on the propagation path – it's only stored so the synthetic
    // root has plausible deps in the cache.
    let lo = match &c.lo {
        Bound::Unbounded => String::new(),
        Bound::Included(v) => format!(">={}", v),
        Bound::Excluded(v) => format!(">{}", v),
    };
    let hi = match &c.hi {
        Bound::Unbounded => String::new(),
        Bound::Included(v) => format!("<={}", v),
        Bound::Excluded(v) => format!("<{}", v),
    };
    match (lo.is_empty(), hi.is_empty()) {
        (true, true) => "any".to_string(),
        (false, true) => lo,
        (true, false) => hi,
        (false, false) => format!("{} {}", lo, hi),
    }
}
