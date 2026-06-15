//! Install (and re-install/update) plugins from a local path, a GitHub release,
//! or the tryhatch registry. All three converge on [`place_from_url`], which
//! downloads, verifies, probes the `--hatch-manifest`, and atomically drops the
//! binary into `~/.hatch/plugins/bin/`.

use anyhow::{anyhow, bail, Context, Result};
use hatch_plugin_api::paths::plugins_bin_dir;
use hatch_plugin_api::PluginManifest;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

use super::discovery::plugin_binary_name;
use super::sources::{self, InstallSource};
use super::state::{self, Source};
use super::{github, manifest, registry};

/// The Rust target triple for the current host, used to match release assets.
/// `None` on platforms we don't ship plugin binaries for.
pub fn target_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => Some("aarch64-pc-windows-msvc"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

fn require_target() -> Result<&'static str> {
    target_triple().ok_or_else(|| {
        anyhow!(
            "unsupported platform {}/{} — no prebuilt plugin binaries",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })
}

/// What a successful placement produced.
pub struct Placed {
    pub manifest: PluginManifest,
    pub path: PathBuf,
}

/// A resolved download: a URL plus optional integrity metadata.
pub struct Download {
    pub url: String,
    pub sha256: Option<String>,
    pub is_archive: bool,
}

impl Download {
    fn from_url(url: String, sha256: Option<String>) -> Self {
        let is_archive = url.ends_with(".tar.gz") || url.ends_with(".tgz");
        Self { url, sha256, is_archive }
    }
}

/// `hatch plugin install <source>` entry point.
pub async fn install(source_str: &str) -> Result<()> {
    match sources::parse(source_str)? {
        InstallSource::Local(path) => install_local(&path),
        InstallSource::Github { repo, tag } => {
            install_github(&repo, tag.as_deref(), true).await.map(|_| ())
        }
        InstallSource::Registry { name, version } => {
            install_registry(&name, version.as_deref(), true).await.map(|_| ())
        }
    }
}

/// Copy an executable from a local path (the original behavior).
pub fn install_local(src: &Path) -> Result<()> {
    if !src.is_file() {
        bail!("`{}` is not a file", src.display());
    }
    let m = manifest::query(src).ok_or_else(|| {
        anyhow!("`{}` did not respond to --hatch-manifest; not a hatch plugin", src.display())
    })?;
    warn_if_incompatible(&m);
    let dest = dest_path(&m.name)?;
    atomic_place_file(src, &dest)?;
    state::record(&m.name, &m.version, Source::Local { path: src.display().to_string() })?;
    println!("✅ installed plugin `{}` v{} -> {}", m.name, m.version, dest.display());
    Ok(())
}

/// Install from a GitHub release. Returns the placed manifest; `announce`
/// controls whether the success line is printed (suppressed for autoupdate,
/// which prints its own message).
pub async fn install_github(repo: &str, tag: Option<&str>, announce: bool) -> Result<PluginManifest> {
    let target = require_target()?;
    let release = github::fetch_release(repo, tag).await?;
    let asset = github::pick_asset(&release.assets, target)?;
    let dl = Download::from_url(asset.url.clone(), None);
    let placed = place_from_url(&dl).await?;
    state::record(
        &placed.manifest.name,
        &placed.manifest.version,
        Source::Github { repo: repo.to_string(), tag: Some(release.tag.clone()) },
    )?;
    if announce {
        println!(
            "✅ installed plugin `{}` v{} from github:{repo} ({})",
            placed.manifest.name, placed.manifest.version, release.tag
        );
    }
    Ok(placed.manifest)
}

/// Install from the tryhatch registry by plugin name.
pub async fn install_registry(
    name: &str,
    version: Option<&str>,
    announce: bool,
) -> Result<PluginManifest> {
    let target = require_target()?;
    let plugin = registry::fetch_plugin(name).await?;
    let ver = plugin.resolve_version(version)?;
    let asset = ver.asset_for(target)?;
    let dl = Download::from_url(asset.url.clone(), asset.sha256.clone());
    let placed = place_from_url(&dl).await?;
    state::record(
        &placed.manifest.name,
        &placed.manifest.version,
        Source::Registry { name: name.to_string(), base: Some(registry::base_url()) },
    )?;
    if announce {
        println!(
            "✅ installed plugin `{}` v{} from registry ({})",
            placed.manifest.name, placed.manifest.version, registry::base_url()
        );
    }
    Ok(placed.manifest)
}

/// Download -> verify -> (extract) -> probe manifest -> atomically place.
pub async fn place_from_url(dl: &Download) -> Result<Placed> {
    let dir = plugins_bin_dir().ok_or_else(|| anyhow!("could not resolve ~/.hatch/plugins/bin"))?;
    std::fs::create_dir_all(&dir)?;

    // Download into memory (plugin binaries are a few MB).
    let bytes = http_get_bytes(&dl.url).await?;

    if let Some(expected) = &dl.sha256 {
        verify_sha256(&bytes, expected)?;
    }

    let exe_bytes = if dl.is_archive { extract_binary_from_targz(&bytes)? } else { bytes };

    // Stage to a temp file in the destination dir (same volume -> atomic rename).
    let staging = dir.join(format!(".hatch-plugin-staging-{}", std::process::id()));
    std::fs::write(&staging, &exe_bytes)
        .with_context(|| format!("writing staged plugin to {}", staging.display()))?;
    make_executable(&staging)?;

    // Identify the plugin by asking it for its manifest.
    let m = match manifest::query(&staging) {
        Some(m) => m,
        None => {
            let _ = std::fs::remove_file(&staging);
            bail!("downloaded binary did not respond to --hatch-manifest; not a hatch plugin");
        }
    };
    warn_if_incompatible(&m);

    let dest = dir.join(plugin_binary_name(&m.name));
    finalize_rename(&staging, &dest)?;
    Ok(Placed { manifest: m, path: dest })
}

async fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::new();
    let mut req = client.get(url).header("User-Agent", "hatch-plugin-installer");
    // GitHub asset URLs accept the same token; harmless elsewhere.
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

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<()> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let got = hex_lower(&hasher.finalize());
    let expected = expected.trim().to_lowercase();
    if got != expected {
        bail!("sha256 mismatch: expected {expected}, got {got}");
    }
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Extract the single plugin executable from a `.tar.gz`. Prefers an entry whose
/// file name starts with `hatch-`, else the first regular file.
fn extract_binary_from_targz(bytes: &[u8]) -> Result<Vec<u8>> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    let mut fallback: Option<Vec<u8>> = None;
    for entry in archive.entries().context("reading tar archive")? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.into_owned();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        if name.starts_with("hatch-") {
            return Ok(buf);
        }
        if fallback.is_none() {
            fallback = Some(buf);
        }
    }
    fallback.ok_or_else(|| anyhow!("archive contained no plugin binary"))
}

fn dest_path(name: &str) -> Result<PathBuf> {
    let dir = plugins_bin_dir().ok_or_else(|| anyhow!("could not resolve ~/.hatch/plugins/bin"))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(plugin_binary_name(name)))
}

/// Copy a local file into place (no rename — source stays where it is).
fn atomic_place_file(src: &Path, dest: &Path) -> Result<()> {
    let staging = dest.with_extension("staging");
    std::fs::copy(src, &staging)
        .with_context(|| format!("copying {} -> {}", src.display(), staging.display()))?;
    make_executable(&staging)?;
    finalize_rename(&staging, dest)
}

/// Replace `dest` with `staging` (Windows can't rename onto an existing file).
fn finalize_rename(staging: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        let _ = std::fs::remove_file(dest);
    }
    std::fs::rename(staging, dest)
        .with_context(|| format!("installing plugin to {}", dest.display()))?;
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn warn_if_incompatible(m: &PluginManifest) {
    if !m.is_compatible() {
        eprintln!(
            "⚠️  plugin targets hatch_plugin_api {} but this hatch speaks {} — installing anyway",
            m.hatch_plugin_api,
            hatch_plugin_api::HATCH_PLUGIN_API_VERSION
        );
    }
}
