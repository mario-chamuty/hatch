//! Code-signing material management. All crypto runs in the build environment
//! via openssl; the resulting `.p12` / `.mobileprovision` live next to the
//! toolchain where `zsign` consumes them. Hatch never needs host openssl.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};

use super::runner::Runner;

/// Generate a fresh 2048-bit RSA private key and a CSR for it. Returns the CSR
/// in PEM form (to POST to App Store Connect) and the path to the private key
/// inside the build environment.
pub fn generate_csr(runner: &Runner, root: &str, common_name: &str) -> Result<(String, String)> {
    let dir = format!("{root}/signing");
    let key_path = format!("{dir}/key.pem");
    let script = format!(
        r#"
mkdir -p "{dir}"
openssl genrsa -out "{key_path}" 2048 >/dev/null 2>&1
openssl req -new -key "{key_path}" -subj "/CN={cn}/O=Hatch/C=US" 2>/dev/null
"#,
        dir = dir,
        key_path = key_path,
        cn = common_name,
    );
    let csr_pem = runner
        .exec(&script)
        .context("generating signing key + CSR")?
        .require()?;
    Ok((csr_pem, key_path))
}

/// Combine the previously generated private key with a certificate returned by
/// App Store Connect (base64 DER) into a password-protected `.p12`. Returns the
/// `.p12` path inside the build environment.
pub fn assemble_p12(
    runner: &Runner,
    root: &str,
    cert_der_b64: &str,
    p12_password: &str,
) -> Result<String> {
    let dir = format!("{root}/signing");
    let key_path = format!("{dir}/key.pem");
    let p12_path = format!("{dir}/cert.p12");
    // Strip whitespace/newlines Apple may include in the base64 blob.
    let clean: String = cert_der_b64.split_whitespace().collect();
    let script = format!(
        r#"
cd "{dir}"
printf '%s' "{b64}" | base64 -d > cert.der
openssl x509 -inform DER -in cert.der -out cert.pem
# -legacy keeps the .p12 readable by zsign's/older OpenSSL; harmless on 3.x.
openssl pkcs12 -export -legacy -inkey "{key_path}" -in cert.pem \
  -out "{p12_path}" -passout pass:"{pw}" 2>/dev/null \
  || openssl pkcs12 -export -inkey "{key_path}" -in cert.pem \
       -out "{p12_path}" -passout pass:"{pw}"
echo "{p12_path}"
"#,
        dir = dir,
        b64 = clean,
        key_path = key_path,
        p12_path = p12_path,
        pw = p12_password,
    );
    runner.exec(&script).context("assembling .p12")?.require()
}

/// Write a provisioning profile (base64 of the `.mobileprovision`) into the
/// build environment and return its path.
pub fn install_profile(runner: &Runner, root: &str, profile_b64: &str, name: &str) -> Result<String> {
    let dir = format!("{root}/signing");
    let safe: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
    let path = format!("{dir}/{safe}.mobileprovision");
    let clean: String = profile_b64.split_whitespace().collect();
    let script = format!(
        r#"
mkdir -p "{dir}"
printf '%s' "{b64}" | base64 -d > "{path}"
echo "{path}"
"#,
        dir = dir,
        b64 = clean,
        path = path,
    );
    runner.exec(&script).context("installing profile")?.require()
}

/// Sign an `.ipa` in place (or to `out`) with zsign using the given p12 + profile.
pub fn sign_ipa(
    runner: &Runner,
    root: &str,
    ipa_path: &str,
    p12_path: &str,
    p12_password: &str,
    profile_path: &str,
    out_path: &str,
) -> Result<()> {
    let zsign = format!("{root}/zsign/bin/zsign");
    let script = format!(
        r#"
"{zsign}" -k "{p12}" -p "{pw}" -m "{prof}" -o "{out}" "{ipa}"
"#,
        zsign = zsign,
        p12 = p12_path,
        pw = p12_password,
        prof = profile_path,
        out = out_path,
        ipa = ipa_path,
    );
    runner.exec(&script).context("zsign signing")?.require()?;
    Ok(())
}

/// Convenience: decode a base64 blob (used by callers that want to persist
/// Apple-returned artifacts on the host too).
#[allow(dead_code)]
pub fn decode_b64(b64: &str) -> Result<Vec<u8>> {
    let clean: String = b64.split_whitespace().collect();
    STANDARD.decode(clean.as_bytes()).context("decoding base64 blob")
}
