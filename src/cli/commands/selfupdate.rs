//! `hatch selfupdate` — replace the running hatch binary with a hatch GitHub
//! release build (the latest, or a specific `--tag`).
//!
//! Reuses the plugin GitHub-releases client ([`crate::plugin::github`]) to
//! resolve the release and its per-platform asset, downloads + verifies it
//! against the release's `SHA256SUMS`, extracts the `hatch` executable from the
//! `.tar.gz`/`.zip`, and atomically swaps the *running* binary via
//! `self_replace` (which handles the Windows can't-delete-a-running-exe dance).

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

use crate::plugin::github;

/// `owner/repo` that publishes hatch release binaries. Overridable via
/// `HATCH_RELEASE_REPO` (forks / testing).
fn release_repo() -> String {
    std::env::var("HATCH_RELEASE_REPO")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "mario-chamuty/hatch".to_string())
}

/// Release-asset target triple for the current host. NOTE: Linux ships **musl**
/// static builds (per the release matrix) — these differ from the plugin
/// installer's `-gnu` triples, so this can't reuse `plugin::install::target_triple`.
fn release_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

pub async fn execute(check: bool, force: bool, tag: Option<String>) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let repo = release_repo();

    let release = github::fetch_release(&repo, tag.as_deref())
        .await
        .with_context(|| format!("resolving hatch release from github:{repo}"))?;

    let newer = is_newer(&release.version, current);

    if check {
        if newer {
            println!("⬆️  update available: v{current} -> v{} ({})", release.version, release.tag);
        } else {
            println!("✅ hatch is up to date (v{current}); latest is v{}", release.version);
        }
        return Ok(());
    }

    // Only the "latest" path short-circuits on up-to-date; an explicit --tag or
    // --force always (re)installs.
    if !newer && tag.is_none() && !force {
        println!("✅ hatch is already up to date (v{current})");
        println!("   run with --force to reinstall, or --tag <vX.Y.Z> for a specific release");
        return Ok(());
    }

    let target = release_target().ok_or_else(|| {
        anyhow!(
            "unsupported platform {}/{} — no prebuilt hatch release",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let asset = github::pick_asset(&release.assets, target)
        .with_context(|| format!("no asset for {target} in release {}", release.tag))?;

    println!("⬇️  downloading hatch v{} ({})...", release.version, asset.name);
    let bytes = http_get_bytes(&asset.url).await?;

    // Integrity: verify against the release-attached SHA256SUMS when present.
    match release.assets.iter().find(|a| a.name == "SHA256SUMS") {
        Some(sums) => match http_get_bytes(&sums.url).await {
            Ok(raw) => verify_against_sums(&bytes, &asset.name, &raw)?,
            Err(e) => eprintln!("⚠️  could not fetch SHA256SUMS ({e}); skipping integrity check"),
        },
        None => eprintln!("⚠️  release has no SHA256SUMS; skipping integrity check"),
    }

    let exe = extract_hatch_binary(&bytes, &asset.name)?;

    // Stage to a temp file, then atomically swap the currently-running binary.
    let staging =
        std::env::temp_dir().join(format!("hatch-selfupdate-{}.tmp", std::process::id()));
    std::fs::write(&staging, &exe)
        .with_context(|| format!("writing staged binary to {}", staging.display()))?;

    self_replace::self_replace(&staging).with_context(|| {
        let where_ = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "the hatch executable".into());
        format!("replacing {where_} (insufficient permissions? try elevated/sudo)")
    })?;
    let _ = std::fs::remove_file(&staging);

    println!("✅ updated hatch v{current} -> v{} ({})", release.version, release.tag);
    Ok(())
}

/// Best-effort "candidate is newer than current". Uses semver; falls back to a
/// plain string inequality if either side isn't valid semver.
fn is_newer(candidate: &str, current: &str) -> bool {
    let cand = candidate.trim_start_matches('v');
    match (semver::Version::parse(cand), semver::Version::parse(current)) {
        (Ok(c), Ok(cur)) => c > cur,
        _ => cand != current,
    }
}

async fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::new();
    let mut req = client.get(url).header("User-Agent", "hatch-selfupdate");
    if let Ok(tok) = std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
        if !tok.is_empty() {
            req = req.header("Authorization", format!("Bearer {tok}"));
        }
    }
    let resp = req.send().await.with_context(|| format!("downloading {url}"))?;
    if !resp.status().is_success() {
        bail!("download failed: {} for {url}", resp.status());
    }
    Ok(resp.bytes().await.context("reading download body")?.to_vec())
}

/// Verify `bytes` against the `<sha256>  <name>` line for `asset_name` in a
/// `sha256sum`-format `SHA256SUMS` document.
fn verify_against_sums(bytes: &[u8], asset_name: &str, sums: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(sums);
    let expected = text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        // sha256sum prefixes binary-mode names with '*'; some tools prefix './'.
        let name = parts.next()?.trim_start_matches('*').trim_start_matches("./");
        (name == asset_name).then(|| hash.to_string())
    });

    let Some(expected) = expected else {
        eprintln!("⚠️  {asset_name} not listed in SHA256SUMS; skipping integrity check");
        return Ok(());
    };

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let got = hex_lower(&hasher.finalize());
    if got != expected.to_lowercase() {
        bail!("sha256 mismatch for {asset_name}: expected {expected}, got {got}");
    }
    println!("🔒 verified sha256 ✓");
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Dispatch to the right archive extractor based on the asset extension.
fn extract_hatch_binary(bytes: &[u8], asset_name: &str) -> Result<Vec<u8>> {
    if asset_name.ends_with(".zip") {
        extract_from_zip(bytes)
    } else {
        extract_from_targz(bytes)
    }
}

/// The release archives wrap the binary in a `hatch-<ver>-<target>/` directory;
/// match on the file name regardless of parent path.
fn is_hatch_bin(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some("hatch") | Some("hatch.exe")
    )
}

fn extract_from_targz(bytes: &[u8]) -> Result<Vec<u8>> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().context("reading tar archive")? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.into_owned();
        if is_hatch_bin(&path) {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    bail!("archive did not contain a `hatch` binary");
}

fn extract_from_zip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut zip =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).context("reading zip archive")?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        if !f.is_file() {
            continue;
        }
        let name = f.name().to_string();
        if is_hatch_bin(Path::new(&name)) {
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    bail!("zip did not contain a `hatch.exe` binary");
}
