//! Top-down interval propagation.
//!
//! A pre-pass before the pubgrub solver. Given the root package's direct
//! dependency constraints and a warm [`MetadataCache`], it walks the graph
//! breadth-first: for every package whose interval was tightened in the
//! current wave, it consults the latest version in-range and intersects that
//! version's own dependency constraints into the accumulator.
//!
//! In the happy path (no conflicts, no under-constrained packages) every
//! interval collapses to a single satisfying version and we hand the solver
//! nothing at all. Pubgrub is only invoked when a package remains ambiguous
//! or we detected a conflict and want the derivation tree explained.

use anyhow::Result;
use log::{debug, trace};
use std::collections::{HashMap, HashSet};

use crate::cache::metadata_cache::MetadataCache;
use crate::manifest::schema::SdkConstraints;
use crate::registry::traits::{ParsedConstraint, VersionConstraint};

/// Packages handled by the Flutter/Dart SDK rather than pub.dev. They never
/// enter the fetch loop.
const SDK_PSEUDO_PACKAGES: &[&str] = &[
    "flutter",
    "flutter_test",
    "flutter_web_plugins",
    "flutter_driver",
    "integration_test",
    "sky_engine",
    "_macros",
];

pub fn is_sdk_pseudo_package(name: &str) -> bool {
    SDK_PSEUDO_PACKAGES.contains(&name)
}

/// Version overrides applied by the user via `dependency_overrides` /
/// `overrides` in the manifest. We treat these as pinning constraints –
/// propagation replaces whatever the graph says with the override.
pub type Overrides = HashMap<String, semver::Version>;

/// Trace of a single conflict discovered while propagating. The solver will
/// surface a proper derivation tree later; this struct is intentionally
/// lightweight and only used for debugging the pre-pass.
#[derive(Debug, Clone)]
pub struct ConflictTrace {
    pub package: String,
    pub left: String,
    pub right: String,
}

#[derive(Debug)]
pub struct PropagationResult {
    /// Packages whose interval collapsed to a single satisfying version.
    pub resolved: HashMap<String, semver::Version>,
    /// Packages whose interval did not collapse. Passed to pubgrub.
    pub partial: HashMap<String, ParsedConstraint>,
    /// Conflicts observed during the pre-pass. If non-empty the caller
    /// should treat the result as "needs pubgrub to explain" and invoke the
    /// adapter.
    pub conflicts: Vec<ConflictTrace>,
    /// Per-package resolved version -> direct dependencies at that version.
    /// Saves the install path from a second metadata lookup.
    pub resolved_deps: HashMap<String, HashMap<String, String>>,
}

pub struct Propagator<'a> {
    cache: &'a MetadataCache,
    #[allow(dead_code)]
    sdks: SdkConstraints,
    overrides: Overrides,
}

impl<'a> Propagator<'a> {
    pub fn new(cache: &'a MetadataCache, sdks: SdkConstraints, overrides: Overrides) -> Self {
        Self {
            cache,
            sdks,
            overrides,
        }
    }

    pub async fn propagate(
        &self,
        root_deps: &HashMap<String, ParsedConstraint>,
    ) -> Result<PropagationResult> {
        let mut intervals: HashMap<String, ParsedConstraint> = HashMap::new();
        let mut tightened: HashSet<String> = HashSet::new();
        let mut conflicts: Vec<ConflictTrace> = Vec::new();

        // Seed with root deps.
        for (name, c) in root_deps {
            if is_sdk_pseudo_package(name) {
                continue;
            }
            intervals.insert(name.clone(), c.clone());
            tightened.insert(name.clone());
        }

        // Apply overrides immediately: pin to an exact interval.
        for (name, ver) in &self.overrides {
            let pinned = pin_to_exact(ver);
            intervals.insert(name.clone(), pinned);
            tightened.insert(name.clone());
        }

        let mut waves = 0usize;
        while !tightened.is_empty() {
            waves += 1;
            if waves > 200 {
                debug!("Propagator bailing after {} waves (fixed-point not reached)", waves);
                break;
            }
            let this_wave: Vec<String> = tightened.drain().collect();
            trace!("Propagator wave {}: {} packages", waves, this_wave.len());

            for pkg in this_wave {
                let Some(interval) = intervals.get(&pkg).cloned() else {
                    continue;
                };
                // Skip overridden packages; their deps are honoured via the
                // override target anyway.
                if self.overrides.contains_key(&pkg) {
                    continue;
                }

                let idx = match self.cache.get_index(&pkg).await {
                    Some(i) => i,
                    None => {
                        // Metadata not cached yet – the async fetcher in
                        // `ultra.rs` will prime it before we hand over to
                        // pubgrub. Skip for now.
                        continue;
                    }
                };

                let Some(pos) = idx.latest_matching(&interval) else {
                    continue; // Conflict recorded elsewhere if interval empties.
                };
                let Some(latest) = idx.get(pos) else { continue };

                // Intersect the latest version's declared deps into the
                // accumulator.
                for (dep_name, dep_constraint) in &latest.dependencies {
                    if is_sdk_pseudo_package(dep_name) {
                        continue;
                    }
                    let parsed = match VersionConstraint::parse(dep_constraint)
                        .and_then(|c| c.to_parsed())
                    {
                        Ok(p) => p,
                        Err(_) => continue,
                    };

                    match intervals.get(dep_name) {
                        None => {
                            intervals.insert(dep_name.clone(), parsed);
                            tightened.insert(dep_name.clone());
                        }
                        Some(existing) => {
                            match existing.intersect(&parsed) {
                                Some(merged) => {
                                    if &merged != existing {
                                        intervals.insert(dep_name.clone(), merged);
                                        tightened.insert(dep_name.clone());
                                    }
                                }
                                None => {
                                    conflicts.push(ConflictTrace {
                                        package: dep_name.clone(),
                                        left: format!("{:?}", existing),
                                        right: format!("{:?}", parsed),
                                    });
                                    // Don't mutate further – let pubgrub
                                    // produce the real explanation.
                                }
                            }
                        }
                    }
                }
            }
        }

        // Now split intervals -> resolved / partial.
        let mut resolved: HashMap<String, semver::Version> = HashMap::new();
        let mut partial: HashMap<String, ParsedConstraint> = HashMap::new();
        let mut resolved_deps: HashMap<String, HashMap<String, String>> = HashMap::new();

        for (name, interval) in intervals {
            if let Some(ov) = self.overrides.get(&name) {
                resolved.insert(name.clone(), ov.clone());
                resolved_deps.insert(name, HashMap::new());
                continue;
            }
            if is_sdk_pseudo_package(&name) {
                continue;
            }
            match self.cache.get_index(&name).await {
                Some(idx) => {
                    if let Some(pos) = idx.latest_matching(&interval) {
                        if let (Some(ver), Some(raw)) = (idx.version(pos), idx.get(pos)) {
                            resolved.insert(name.clone(), ver.clone());
                            resolved_deps.insert(name, raw.dependencies.clone());
                            continue;
                        }
                    }
                    // No match -> fall through to partial so pubgrub can
                    // explain the conflict.
                    partial.insert(name, interval);
                }
                None => {
                    // Metadata not cached – hand the interval to the solver
                    // which will drive the fetcher.
                    partial.insert(name, interval);
                }
            }
        }

        Ok(PropagationResult {
            resolved,
            partial,
            conflicts,
            resolved_deps,
        })
    }
}

fn pin_to_exact(v: &semver::Version) -> ParsedConstraint {
    use std::ops::Bound;
    ParsedConstraint {
        lo: Bound::Included(v.clone()),
        hi: Bound::Included(v.clone()),
        allow_prerelease: !v.pre.is_empty(),
    }
}

