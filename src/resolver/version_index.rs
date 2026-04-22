//! Pre-parsed, semver-sorted index over a package's available versions.
//!
//! Built once per package (after metadata arrives from the registry) and then
//! used by the resolver for bulk queries: "which versions satisfy this
//! constraint?" or "what's the latest matching?". The underlying membership is
//! a [`BitVec`] so set algebra (intersection / union) is cheap even for
//! packages with hundreds of versions.

use anyhow::Result;
use bitvec::prelude::*;
use log::debug;
use std::collections::HashMap;

use crate::registry::traits::{PackageVersion, ParsedConstraint};

/// Sorted, pre-parsed index over a package's versions.
pub struct VersionIndex {
    /// Parsed `semver::Version` values, ascending order.
    versions: Vec<semver::Version>,
    /// Raw [`PackageVersion`] entries in the same order as `versions`, so
    /// callers can roundtrip to the metadata (sha256, dependency map, etc.).
    raw: Vec<PackageVersion>,
    /// Fast lookup from the original version string to its index position.
    by_string: HashMap<String, usize>,
}

impl VersionIndex {
    /// Build an index from raw registry metadata. Versions that fail to parse
    /// are logged at `debug!` and dropped (pub.dev occasionally ships garbage
    /// entries – we refuse to fail the whole resolution over it).
    pub fn from_metadata(versions: &[PackageVersion]) -> Result<Self> {
        let mut pairs: Vec<(semver::Version, PackageVersion)> = Vec::with_capacity(versions.len());
        for pv in versions {
            match semver::Version::parse(&pv.version) {
                Ok(v) => pairs.push((v, pv.clone())),
                Err(e) => {
                    debug!(
                        "VersionIndex::from_metadata: skipping unparseable version '{}': {}",
                        pv.version, e
                    );
                }
            }
        }

        pairs.sort_by(|a, b| a.0.cmp(&b.0));

        let mut versions_vec = Vec::with_capacity(pairs.len());
        let mut raw = Vec::with_capacity(pairs.len());
        let mut by_string = HashMap::with_capacity(pairs.len());

        for (idx, (ver, pv)) in pairs.into_iter().enumerate() {
            by_string.insert(pv.version.clone(), idx);
            versions_vec.push(ver);
            raw.push(pv);
        }

        Ok(Self {
            versions: versions_vec,
            raw,
            by_string,
        })
    }

    pub fn len(&self) -> usize {
        self.versions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }

    /// Return a [`VersionSet`] with a bit set for every version satisfying
    /// `c`. The returned bitvec has length `self.len()`.
    pub fn matching(&self, c: &ParsedConstraint) -> VersionSet {
        let mut bits = bitvec![0; self.versions.len()];
        for (i, v) in self.versions.iter().enumerate() {
            if c.contains(v) {
                bits.set(i, true);
            }
        }
        VersionSet(bits)
    }

    /// Return the index of the highest version satisfying `c`. Uses binary
    /// search on the `hi` bound to skip past versions that are definitely too
    /// new, then walks downward until a version passes [`ParsedConstraint::contains`]
    /// (which handles the prerelease filter).
    pub fn latest_matching(&self, c: &ParsedConstraint) -> Option<usize> {
        if self.versions.is_empty() {
            return None;
        }

        use std::ops::Bound;

        // `partition_point` returns the first index where the predicate is
        // false. Applied here with "v <= hi", it yields `end` such that
        // `self.versions[..end]` are candidates (<= hi) and versions at
        // `end..` are above the ceiling.
        let end = match &c.hi {
            Bound::Unbounded => self.versions.len(),
            Bound::Included(hi) => self.versions.partition_point(|v| v <= hi),
            Bound::Excluded(hi) => self.versions.partition_point(|v| v < hi),
        };

        // Walk downward from `end` looking for the first version that
        // actually matches (prerelease / lo bound filters still apply).
        let mut i = end;
        while i > 0 {
            i -= 1;
            if c.contains(&self.versions[i]) {
                return Some(i);
            }
        }
        None
    }

    pub fn get(&self, idx: usize) -> Option<&PackageVersion> {
        self.raw.get(idx)
    }

    pub fn version(&self, idx: usize) -> Option<&semver::Version> {
        self.versions.get(idx)
    }

    pub fn index_of(&self, v_str: &str) -> Option<usize> {
        self.by_string.get(v_str).copied()
    }

    pub fn iter_versions(&self) -> impl Iterator<Item = (usize, &semver::Version)> {
        self.versions.iter().enumerate()
    }
}

/// Bit-packed set of version indices within a [`VersionIndex`].
pub struct VersionSet(BitVec);

impl VersionSet {
    pub fn and(&self, other: &Self) -> Self {
        debug_assert_eq!(self.0.len(), other.0.len(), "VersionSet length mismatch");
        let mut out = self.0.clone();
        out &= other.0.clone();
        VersionSet(out)
    }

    pub fn or(&self, other: &Self) -> Self {
        debug_assert_eq!(self.0.len(), other.0.len(), "VersionSet length mismatch");
        let mut out = self.0.clone();
        out |= other.0.clone();
        VersionSet(out)
    }

    /// Rightmost set bit – i.e. the highest matching index.
    pub fn highest(&self) -> Option<usize> {
        self.0.last_one()
    }

    /// Leftmost set bit – i.e. the lowest matching index.
    pub fn lowest(&self) -> Option<usize> {
        self.0.first_one()
    }

    pub fn is_empty(&self) -> bool {
        self.0.not_any()
    }

    pub fn count(&self) -> usize {
        self.0.count_ones()
    }

    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter_ones()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::traits::VersionConstraint;
    use std::collections::HashMap;

    fn pv(version: &str) -> PackageVersion {
        PackageVersion {
            version: version.to_string(),
            description: None,
            homepage: None,
            repository: None,
            dependencies: HashMap::new(),
            dev_dependencies: HashMap::new(),
            published: None,
            dart_sdk: None,
            flutter_sdk: None,
            archive_sha256: None,
        }
    }

    fn idx_of_ten() -> VersionIndex {
        let versions: Vec<PackageVersion> = [
            "1.0.0", "1.1.0", "1.2.0", "1.2.1", "1.3.0", "1.5.0", "2.0.0", "2.1.0", "2.1.1",
            "3.0.0",
        ]
        .iter()
        .map(|s| pv(s))
        .collect();
        VersionIndex::from_metadata(&versions).unwrap()
    }

    #[test]
    fn matching_any_sets_all_bits() {
        let idx = idx_of_ten();
        let any = ParsedConstraint::any();
        let set = idx.matching(&any);
        assert_eq!(set.count(), 10);
        assert_eq!(set.highest(), Some(9));
        assert_eq!(set.lowest(), Some(0));
    }

    #[test]
    fn latest_matching_caret() {
        let idx = idx_of_ten();
        let c = VersionConstraint::Caret("1.2.0".to_string())
            .to_parsed()
            .unwrap();
        let found = idx.latest_matching(&c).unwrap();
        // Highest <2.0.0 and >=1.2.0 is 1.5.0 at idx 5.
        assert_eq!(idx.version(found).unwrap().to_string(), "1.5.0");
    }

    #[test]
    fn intersect_overlapping_returns_some() {
        let a = VersionConstraint::parse(">=1.0.0 <3.0.0").unwrap().to_parsed().unwrap();
        let b = VersionConstraint::parse(">=2.0.0 <4.0.0").unwrap().to_parsed().unwrap();
        let c = a.intersect(&b).expect("should overlap");
        // The intersection should contain 2.5.0, not 1.5.0 or 3.5.0.
        assert!(c.contains(&semver::Version::parse("2.5.0").unwrap()));
        assert!(!c.contains(&semver::Version::parse("1.5.0").unwrap()));
        assert!(!c.contains(&semver::Version::parse("3.5.0").unwrap()));
    }

    #[test]
    fn intersect_disjoint_returns_none() {
        let a = VersionConstraint::parse(">=1.0.0 <2.0.0").unwrap().to_parsed().unwrap();
        let b = VersionConstraint::parse(">=3.0.0 <4.0.0").unwrap().to_parsed().unwrap();
        assert!(a.intersect(&b).is_none());
    }

    #[test]
    fn caret_rejects_prerelease_of_next_major() {
        let c = VersionConstraint::Caret("1.0.0".to_string()).to_parsed().unwrap();
        assert!(!c.contains(&semver::Version::parse("2.0.0-alpha.1").unwrap()));
        assert!(c.contains(&semver::Version::parse("1.5.0").unwrap()));
    }

    #[test]
    fn latest_matching_skips_prereleases_by_default() {
        // Index with a pre-release at the top.
        let versions: Vec<PackageVersion> =
            ["1.0.0", "1.1.0", "2.0.0-beta.1"].iter().map(|s| pv(s)).collect();
        let idx = VersionIndex::from_metadata(&versions).unwrap();
        let c = VersionConstraint::parse(">=1.0.0").unwrap().to_parsed().unwrap();
        let found = idx.latest_matching(&c).unwrap();
        assert_eq!(idx.version(found).unwrap().to_string(), "1.1.0");
    }

    #[test]
    fn skips_unparseable_versions() {
        let versions = vec![pv("1.0.0"), pv("not-a-version"), pv("2.0.0")];
        let idx = VersionIndex::from_metadata(&versions).unwrap();
        assert_eq!(idx.len(), 2);
    }

    #[test]
    fn exact_constraint_matches_only_that_version() {
        let idx = idx_of_ten();
        let c = VersionConstraint::Exact("1.2.0".to_string()).to_parsed().unwrap();
        let set = idx.matching(&c);
        assert_eq!(set.count(), 1);
        let i = set.highest().unwrap();
        assert_eq!(idx.version(i).unwrap().to_string(), "1.2.0");
    }
}
