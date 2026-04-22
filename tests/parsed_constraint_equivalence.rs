//! Property test: `VersionConstraint::satisfies` and
//! `VersionConstraint::to_parsed().contains()` must agree for every
//! (constraint, version) pair.
//!
//! The interval-based API is a pure optimisation: it avoids re-parsing the
//! version string on each call. If these two ever diverged it would be a
//! resolver correctness bug.

use hatch::registry::traits::VersionConstraint;
use proptest::prelude::*;
use semver::Version;

/// Generate a random semver version with small numeric components so we
/// occasionally hit the same values as the constraints.
fn any_version() -> impl Strategy<Value = Version> {
    (0u64..5, 0u64..5, 0u64..5).prop_map(|(maj, min, pat)| Version::new(maj, min, pat))
}

/// Generate a version optionally carrying a prerelease tag (e.g. `1.2.3-beta.1`).
fn any_version_with_optional_pre() -> impl Strategy<Value = Version> {
    (
        0u64..5,
        0u64..5,
        0u64..5,
        prop_oneof![Just(None), Just(Some("alpha.1".to_string())), Just(Some("beta.2".to_string())), Just(Some("rc.1".to_string()))],
    )
        .prop_map(|(maj, min, pat, pre)| {
            let mut v = Version::new(maj, min, pat);
            if let Some(p) = pre {
                v.pre = semver::Prerelease::new(&p).unwrap();
            }
            v
        })
}

/// Generate a constraint string using shapes we support: caret, tilde,
/// comparators, ranges, exact, any.
fn any_constraint_string() -> impl Strategy<Value = String> {
    let ver = (0u64..5, 0u64..5, 0u64..5)
        .prop_map(|(a, b, c)| format!("{}.{}.{}", a, b, c));
    let ver2 = ver.clone();
    let ver3 = ver.clone();
    let ver4 = ver.clone();
    let ver5 = ver.clone();
    let ver6 = ver.clone();
    let ver7 = ver.clone();
    let lo = (0u64..5, 0u64..5, 0u64..5)
        .prop_map(|(a, b, c)| format!("{}.{}.{}", a, b, c));
    let hi = (0u64..5, 0u64..5, 0u64..5)
        .prop_map(|(a, b, c)| format!("{}.{}.{}", a, b, c));

    prop_oneof![
        Just("any".to_string()),
        Just("*".to_string()),
        ver.prop_map(|v| format!("^{}", v)),
        ver2.prop_map(|v| format!("~{}", v)),
        ver3.prop_map(|v| format!(">={}", v)),
        ver4.prop_map(|v| format!(">{}", v)),
        ver5.prop_map(|v| format!("<={}", v)),
        ver6.prop_map(|v| format!("<{}", v)),
        ver7, // exact
        (lo, hi).prop_map(|(a, b)| format!(">={} <{}", a, b)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1000,
        ..ProptestConfig::default()
    })]

    #[test]
    fn satisfies_matches_contains(
        constraint_str in any_constraint_string(),
        v in any_version(),
    ) {
        let c = match VersionConstraint::parse(&constraint_str) {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };
        let parsed = match c.to_parsed() {
            Ok(p) => p,
            Err(_) => return Ok(()),
        };

        let s = c.satisfies(&v.to_string());
        let cc = parsed.contains(&v);

        prop_assert_eq!(
            s, cc,
            "disagree for constraint={:?} version={}: satisfies={} contains={}",
            constraint_str, v, s, cc
        );
    }

    #[test]
    fn contains_rejects_prerelease_when_not_allowed(
        v in any_version_with_optional_pre(),
    ) {
        // `>=0.0.0` never mentions a prerelease and therefore should reject
        // any version carrying a prerelease tag.
        let c = VersionConstraint::parse(">=0.0.0").unwrap();
        let parsed = c.to_parsed().unwrap();
        if !v.pre.is_empty() {
            prop_assert!(!parsed.contains(&v));
        }
    }
}

#[test]
fn caret_upper_bound_semantics() {
    // ^1.2.3 -> <2.0.0
    let c = VersionConstraint::Caret("1.2.3".into()).to_parsed().unwrap();
    assert!(c.contains(&Version::parse("1.2.3").unwrap()));
    assert!(c.contains(&Version::parse("1.999.999").unwrap()));
    assert!(!c.contains(&Version::parse("2.0.0").unwrap()));
    assert!(!c.contains(&Version::parse("1.2.2").unwrap()));

    // ^0.1.2 -> <0.2.0
    let c = VersionConstraint::Caret("0.1.2".into()).to_parsed().unwrap();
    assert!(c.contains(&Version::parse("0.1.2").unwrap()));
    assert!(c.contains(&Version::parse("0.1.9").unwrap()));
    assert!(!c.contains(&Version::parse("0.2.0").unwrap()));

    // ^0.0.3 -> <0.0.4
    let c = VersionConstraint::Caret("0.0.3".into()).to_parsed().unwrap();
    assert!(c.contains(&Version::parse("0.0.3").unwrap()));
    assert!(!c.contains(&Version::parse("0.0.4").unwrap()));
}

#[test]
fn tilde_semantics() {
    // ~1.2.3 -> >=1.2.3 <1.3.0
    let c = VersionConstraint::Tilde("1.2.3".into()).to_parsed().unwrap();
    assert!(c.contains(&Version::parse("1.2.3").unwrap()));
    assert!(c.contains(&Version::parse("1.2.999").unwrap()));
    assert!(!c.contains(&Version::parse("1.3.0").unwrap()));
    assert!(!c.contains(&Version::parse("1.2.2").unwrap()));
}

#[test]
fn exact_constraint_matches_exactly_one() {
    let c = VersionConstraint::Exact("1.2.3".into()).to_parsed().unwrap();
    assert!(c.contains(&Version::parse("1.2.3").unwrap()));
    assert!(!c.contains(&Version::parse("1.2.4").unwrap()));
    assert!(!c.contains(&Version::parse("1.2.2").unwrap()));
}

/// Regression: ">=1.0.0 <=2.0.0" used to lose its inclusive-max on
/// `to_parsed()` round-trip because `VersionConstraint::Range` had no slot
/// for the inclusive/exclusive flag. Stream E phase 2 fix.
#[test]
fn inclusive_max_range_round_trip() {
    let c = VersionConstraint::parse(">=1.0.0 <=2.0.0")
        .expect("parse closed range");
    let parsed = c.to_parsed().expect("to_parsed");
    assert!(
        parsed.contains(&Version::parse("2.0.0").unwrap()),
        "inclusive-max range must include 2.0.0"
    );
    assert!(parsed.contains(&Version::parse("1.0.0").unwrap()));
    assert!(parsed.contains(&Version::parse("1.5.0").unwrap()));
    assert!(!parsed.contains(&Version::parse("2.0.1").unwrap()));
    assert!(!parsed.contains(&Version::parse("0.9.9").unwrap()));
}

#[test]
fn exclusive_max_range_still_exclusive() {
    let c = VersionConstraint::parse(">=1.0.0 <2.0.0")
        .expect("parse half-open range");
    let parsed = c.to_parsed().expect("to_parsed");
    assert!(!parsed.contains(&Version::parse("2.0.0").unwrap()));
    assert!(parsed.contains(&Version::parse("1.9.9").unwrap()));
}

#[test]
fn exclusive_min_inclusive_max_range() {
    let c = VersionConstraint::parse(">1.0.0 <=2.0.0")
        .expect("parse gt-lte range");
    let parsed = c.to_parsed().expect("to_parsed");
    assert!(!parsed.contains(&Version::parse("1.0.0").unwrap()));
    assert!(parsed.contains(&Version::parse("1.0.1").unwrap()));
    assert!(parsed.contains(&Version::parse("2.0.0").unwrap()));
    assert!(!parsed.contains(&Version::parse("2.0.1").unwrap()));
}
