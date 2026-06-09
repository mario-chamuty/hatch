//! TestFlight / App Store upload via Apple's iTMSTransporter (a Java tool that
//! runs on Linux/Windows). Authenticated with the App Store Connect API key.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};

use super::config::AscCredentials;
use super::runner::Runner;

/// Apple's official command-line Transporter installer for Linux (a ~140 MB
/// self-extracting Makeself archive that bundles its own JRE). The payload is
/// fully relocatable, so we extract just the `itms/` tree into the toolchain
/// root rather than running the sudo `/usr/local/itms` installer.
const TRANSPORTER_URL: &str = "https://itunesconnect.apple.com/WebObjects/iTunesConnect.woa/ra/resources/download/public/Transporter__Linux/bin";

/// Locate iTMSTransporter, downloading + installing it into the toolchain root
/// if it isn't present yet. Returns its path inside the build environment.
pub fn ensure_installed(runner: &Runner, root: &str, configured: Option<&str>) -> Result<String> {
    if let Some(found) = locate(runner, root, configured) {
        return Ok(found);
    }
    println!("⬇️  iTMSTransporter not found - installing Apple's Linux uploader (~140 MB)…");
    // The Makeself wrapper uses a bashism, so it must run under bash (not dash).
    // Its post-extract cleanup re-execs with `exec -e` and prints a harmless
    // error; we ignore the installer's exit status and validate by running the
    // extracted binary instead.
    let script = format!(
        r#"
T="{root}/transporter"
ITMS="$T/itms/bin/iTMSTransporter"
mkdir -p "$T"
cd "$T"
curl -fL --retry 2 -o installer.sh "{url}"
rm -rf _payload
bash installer.sh --accept --noexec --keep --target "$T/_payload" >/dev/null 2>&1 || true
if [ ! -d "$T/_payload/itms" ]; then
  echo "transporter payload missing after extraction" >&2; exit 1
fi
rm -rf "$T/itms"
mv "$T/_payload/itms" "$T/itms"
rm -rf "$T/_payload" installer.sh
chmod +x "$ITMS"
"$ITMS" -version >/dev/null 2>&1 || {{ echo "installed iTMSTransporter failed to run" >&2; exit 1; }}
echo "$ITMS"
"#,
        root = root,
        url = TRANSPORTER_URL,
    );
    let out = runner.exec(&script).context("installing iTMSTransporter")?;
    if !out.ok() {
        anyhow::bail!("iTMSTransporter install failed:\n{}", out.stderr.trim());
    }
    locate(runner, root, configured)
        .context("iTMSTransporter still not found after install")
}

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
    // iTMSTransporter searches a fixed set of directories for AuthKey_<KEYID>.p8
    // (it does NOT honor API_PRIVATE_KEYS_DIR reliably). Install the key into the
    // canonical `~/.appstoreconnect/private_keys` and a local `./private_keys`,
    // and run from a working dir that contains the latter, covering every path
    // the tool probes.
    let p8 = std::fs::read(&creds.p8_path)
        .with_context(|| format!("reading {}", creds.p8_path))?;
    let work = format!("{root}/upload");
    let b64 = STANDARD.encode(&p8);
    let install_key = format!(
        r#"
mkdir -p "$HOME/.appstoreconnect/private_keys" "{work}/private_keys"
printf '%s' '{b64}' | base64 -d > "$HOME/.appstoreconnect/private_keys/AuthKey_{kid}.p8"
cp "$HOME/.appstoreconnect/private_keys/AuthKey_{kid}.p8" "{work}/private_keys/AuthKey_{kid}.p8"
chmod 600 "$HOME/.appstoreconnect/private_keys/AuthKey_{kid}.p8" "{work}/private_keys/AuthKey_{kid}.p8"
"#,
        work = work,
        b64 = b64,
        kid = creds.key_id,
    );
    runner.exec(&install_key).context("installing API key")?.require()?;

    let script = format!(
        r#"
cd "{work}"
export API_PRIVATE_KEYS_DIR="{work}/private_keys"
"{itms}" -m upload -assetFile "{ipa}" \
  -apiKey "{kid}" -apiIssuer "{iss}" \
  -WONoPause true -v informational
"#,
        work = work,
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
