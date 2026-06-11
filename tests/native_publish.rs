//! Windows-native sign + upload for an already-built IPA (no WSL/openssl/python).
//! Ignored by default. Signs always; uploads only when UPLOAD=1.

use hatch::ios::config::AscCredentials;
use hatch::ios::{signing, upload};
use std::path::Path;

#[test]
#[ignore]
fn sign_and_maybe_upload() {
    let ipa_in = std::env::var("IPA_IN").expect("IPA_IN");
    let signed = std::env::var("SIGNED_OUT").expect("SIGNED_OUT");
    let rcodesign = std::env::var("RCODESIGN").expect("RCODESIGN");
    let p12 = std::env::var("P12").expect("P12");
    let pw = std::env::var("P12_PW").unwrap_or_else(|_| "hatch".into());
    let profile = std::env::var("PROFILE").expect("PROFILE");

    signing::sign_ipa_native(&rcodesign, &ipa_in, &p12, &pw, &profile, &signed)
        .expect("native sign");
    let n = std::fs::metadata(&signed).map(|m| m.len()).unwrap_or(0);
    println!("SIGNED: {signed} ({n} bytes)");

    if std::env::var("UPLOAD").as_deref() == Ok("1") {
        let creds = AscCredentials {
            issuer_id: std::env::var("ASC_ISSUER").expect("ASC_ISSUER"),
            key_id: std::env::var("ASC_KEY_ID").expect("ASC_KEY_ID"),
            p8_path: std::env::var("ASC_P8").expect("ASC_P8"),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(upload::upload(&creds, Path::new(&signed)))
            .expect("upload");
        println!("UPLOADED");
    } else {
        println!("(skip upload; set UPLOAD=1 to submit)");
    }
}
