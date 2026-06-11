//! End-to-end check of the native Rust pipeline on Linux. Ignored by default
//! (needs the provisioned toolchain + a prepared Flutter project). Run in WSL:
//!
//!   CARGO_TARGET_DIR=/tmp/hatch-linux-target \
//!   PROJECT_DIR=/mnt/d/.../com.scamnemesis.app \
//!   TOOLCHAIN_ROOT=/home/mchamuty/iospoc \
//!   cargo test --test native_e2e -- --ignored --nocapture

use hatch::ios::builder::BuildRequest;
use hatch::ios::native_pipeline;

#[test]
#[ignore]
fn native_build_produces_ipa() {
    let project_dir = std::env::var("PROJECT_DIR").expect("set PROJECT_DIR");
    let root = std::env::var("TOOLCHAIN_ROOT").expect("set TOOLCHAIN_ROOT");
    let bundle_id = std::env::var("BUNDLE_ID").unwrap_or_else(|_| "com.scamnemesis.app".into());
    let app_name = std::env::var("APP_NAME").unwrap_or_else(|_| "ScamNemesis".into());

    let req = BuildRequest {
        project_dir,
        app_name,
        bundle_id,
        min_os: "13.4".into(),
        short_version: "1.0.0".into(),
        build_number: "999".into(),
    };

    let out = native_pipeline::build(&req, &root).expect("native build");
    let meta = std::fs::metadata(&out.ipa_path).expect("ipa exists");
    println!("IPA: {} ({} bytes)", out.ipa_path, meta.len());
    assert!(meta.len() > 100_000, "ipa suspiciously small: {} bytes", meta.len());
}
