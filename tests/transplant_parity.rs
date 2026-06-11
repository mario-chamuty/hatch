//! Parity check: the native Rust `assets_car::transplant` must produce a
//! catalog byte-identical to the proven `temp/transplant.py`. Ignored by default
//! (needs a local donor + iconset + a python-generated reference); run with:
//!
//!   DONOR=temp/reference_kazumi_flutter_918.car \
//!   ICONSET=".../AppIcon.appiconset" \
//!   PY_CAR=temp/py_transplant.car \
//!   cargo test --test transplant_parity -- --ignored --nocapture

use hatch::ios::assets_car;
use std::path::Path;

#[test]
#[ignore]
fn rust_transplant_matches_python() {
    let donor_path = std::env::var("DONOR").expect("set DONOR");
    let iconset = std::env::var("ICONSET").expect("set ICONSET");
    let py_car = std::env::var("PY_CAR").expect("set PY_CAR");

    let donor = std::fs::read(&donor_path).expect("read donor");
    let expected = std::fs::read(&py_car).expect("read python reference car");

    let got = assets_car::transplant(&donor, Path::new(&iconset)).expect("rust transplant");

    if got != expected {
        // Localize the first divergence for a useful failure message.
        let n = got.len().min(expected.len());
        let first = (0..n).find(|&i| got[i] != expected[i]);
        panic!(
            "rust transplant differs from python: rust={} bytes, py={} bytes, first diff at {:?}",
            got.len(),
            expected.len(),
            first
        );
    }
}
