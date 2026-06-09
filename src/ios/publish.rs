//! TestFlight / App Store upload via Apple's iTMSTransporter (a Java tool that
//! runs on Linux/Windows). Authenticated with the App Store Connect API key.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};

use super::config::AscCredentials;
use super::runner::Runner;

/// Locate iTMSTransporter inside the build environment.
pub fn locate(runner: &Runner, root: &str, configured: Option<&str>) -> Option<String> {
    let candidates = [
        configured.map(|s| s.to_string()),
        Some(format!("{root}/transporter/itms/bin/iTMSTransporter")),
        Some(format!("{root}/transporter/bin/iTMSTransporter")),
    ];
    for c in candidates.into_iter().flatten() {
        if runner
            .exec_raw(&format!("test -x \"{c}\" && echo yes || echo no"))
            .map(|o| o.stdout.trim() == "yes")
            .unwrap_or(false)
        {
            return Some(c);
        }
    }
    // Fall back to PATH.
    runner
        .exec_raw("command -v iTMSTransporter || true")
        .ok()
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Upload an `.ipa` (build-env path) to App Store Connect / TestFlight.
pub fn upload(
    runner: &Runner,
    root: &str,
    itms: &str,
    ipa_build_path: &str,
    creds: &AscCredentials,
) -> Result<()> {
    // Place the .p8 where iTMSTransporter expects API keys.
    let p8 = std::fs::read(&creds.p8_path)
        .with_context(|| format!("reading {}", creds.p8_path))?;
    let key_dir = format!("{root}/private_keys");
    let install_key = format!(
        "mkdir -p \"{dir}\"\nprintf '%s' '{b64}' | base64 -d > \"{dir}/AuthKey_{kid}.p8\"\n",
        dir = key_dir,
        b64 = STANDARD.encode(&p8),
        kid = creds.key_id,
    );
    runner.exec(&install_key).context("installing API key")?.require()?;

    let script = format!(
        r#"
export API_PRIVATE_KEYS_DIR="{key_dir}"
"{itms}" -m upload -assetFile "{ipa}" \
  -apiKey "{kid}" -apiIssuer "{iss}" \
  -WONoPause true -v informational
"#,
        key_dir = key_dir,
        itms = itms,
        ipa = ipa_build_path,
        kid = creds.key_id,
        iss = creds.issuer_id,
    );
    let out = runner.exec(&script).context("iTMSTransporter upload")?;
    for line in out.stdout.lines().chain(out.stderr.lines()) {
        if line.contains("ERROR") || line.contains("upload") || line.contains("Package") {
            println!("   {}", line.trim());
        }
    }
    if !out.ok() {
        anyhow::bail!("upload failed (exit {}):\n{}", out.status, out.stderr.trim());
    }
    Ok(())
}
