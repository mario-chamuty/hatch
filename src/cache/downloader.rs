use anyhow::{anyhow, Result};
use reqwest::Client;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, info, warn};
use std::time::Instant;

use super::paths::CachePaths;
use std::path::PathBuf;
use crate::cli::verbosity;

/// Sentinel value accepted by [`PackageDownloader::download_with_checksum`] and
/// friends that indicates the caller has *intentionally* chosen to skip
/// checksum verification for this one invocation (i.e. the user passed
/// `--allow-unchecksummed` on the command line). Supplying this string is
/// logged to both the console and `~/.hatch/audit.log`. It is NOT a config
/// setting – it must be re-opted into on every invocation.
pub const ALLOW_UNCHECKSUMMED_SENTINEL: &str = "@@HATCH_ALLOW_UNCHECKSUMMED@@";

/// Base URL used for package archive downloads. Honours
/// `HATCH_PUB_HOSTED_URL` / `PUB_HOSTED_URL` so E2E tests (and mirror
/// deployments) can redirect archive fetches to a wiremock server. The
/// default `https://pub.dartlang.org` matches the historical hard-coded
/// value so production behaviour is unchanged.
fn archive_base_url() -> String {
    std::env::var("HATCH_PUB_HOSTED_URL")
        .ok()
        .or_else(|| std::env::var("PUB_HOSTED_URL").ok())
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_else(|| "https://pub.dartlang.org".to_string())
}

/// Append one line to `~/.hatch/audit.log` recording a checksum-bypass event.
/// Best-effort – failures are logged but never bubble.
pub(crate) fn audit_log_unchecksummed(package: &str, version: &str) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            warn!("audit: could not determine home directory for audit.log");
            return;
        }
    };
    let dir = home.join(".hatch");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("audit: failed to create {}: {}", dir.display(), e);
        return;
    }
    let path = dir.join("audit.log");
    let ts = chrono::Utc::now().to_rfc3339();
    let line = format!(
        "{ts}\tallow-unchecksummed\t{pkg}\t{ver}\n",
        ts = ts, pkg = package, ver = version
    );
    use std::io::Write;
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(line.as_bytes()) {
                warn!("audit: failed to write {}: {}", path.display(), e);
            }
        }
        Err(e) => warn!("audit: failed to open {}: {}", path.display(), e),
    }
}

#[derive(Clone)]
pub struct PackageDownloader {
    client: Client,
}

impl PackageDownloader {
    pub fn new() -> Self {
        // Use aggressive connection pooling for ultra-fast parallel downloads
        let client = Client::builder()
            .pool_max_idle_per_host(100)  // Increased from 50
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .timeout(std::time::Duration::from_secs(60))  // Increased timeout for larger packages
            .tcp_nodelay(true)  // Disable Nagle's algorithm for lower latency
            .build()
            .unwrap_or_else(|_| Client::new());

        Self { client }
    }

    pub async fn download_from_pubdev_with_checksum(
        &self,
        name: &str,
        version: &str,
        expected_sha256: &str,
    ) -> Result<PathBuf> {
        let start = Instant::now();
        info!("Downloading {}@{} from pub.dev", name, version);

        CachePaths::ensure_directories()?;

        let download_path = CachePaths::download_path(name, version)?;
        if download_path.exists() {
            debug!("Package already downloaded: {}", download_path.display());
            return Ok(download_path);
        }

        let url = format!(
            "{}/packages/{}/versions/{}.tar.gz",
            archive_base_url(), name, version
        );

        debug!("Download URL: {}", url);

        let pb = if verbosity::should_show_progress_bars() {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} Downloading {msg}")
                    .unwrap()
            );
            pb.set_message(format!("{}@{}", name, version));
            pb.enable_steady_tick(std::time::Duration::from_millis(100));
            Some(pb)
        } else {
            None
        };

        let response = self.client
            .get(&url)
            .header("User-Agent", "hatch-cli/1.0.0")
            .send()
            .await
            .map_err(|e| anyhow!("Failed to download package: {}", e))?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to download {}@{}: HTTP {}",
                name,
                version,
                response.status()
            ));
        }

        let total_size = response
            .content_length()
            .unwrap_or(0);

        if let Some(ref pb) = pb {
            if total_size > 0 {
                pb.set_length(total_size);
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} {msg}")
                        .unwrap()
                        .progress_chars("#>-")
                );
            }
        }

        let mut file = File::create(&download_path).await?;
        let mut downloaded = 0u64;
        let mut stream = response.bytes_stream();

        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow!("Download error: {}", e))?;
            file.write_all(&chunk).await?;

            downloaded += chunk.len() as u64;
            if let Some(ref pb) = pb {
                pb.set_position(downloaded);
            }
        }

        let elapsed = start.elapsed();
        let size_mb = downloaded as f64 / 1_048_576.0;
        let speed_mb = size_mb / elapsed.as_secs_f64();

        if let Some(pb) = pb {
            if verbosity::should_show_download_details() {
                pb.finish_with_message(format!("Downloaded {}@{} ({:.1}MB in {:.1}s, {:.1}MB/s)",
                    name, version, size_mb, elapsed.as_secs_f32(), speed_mb));
            } else {
                pb.finish_and_clear();
            }
        }

        if verbosity::is_debug() {
            debug!("Download stats for {}@{}: {:.1}MB in {:.2}s ({:.2}MB/s)",
                name, version, size_mb, elapsed.as_secs_f32(), speed_mb);
        }

        // Verify checksum (fail-closed). The sentinel value bypasses and writes
        // to audit.log with a loud warning.
        if expected_sha256 == ALLOW_UNCHECKSUMMED_SENTINEL {
            warn!(
                "⚠️  SECURITY: skipping checksum verification for {}@{} (--allow-unchecksummed)",
                name, version
            );
            audit_log_unchecksummed(name, version);
        } else {
            match self.verify_checksum(&download_path, Some(expected_sha256)).await {
                Ok(_) => {
                    info!("✓ Checksum verified for {}@{}", name, version);
                }
                Err(e) => {
                    // Delete the corrupted file
                    let _ = tokio::fs::remove_file(&download_path).await;
                    return Err(anyhow!("Checksum verification failed for {}@{}: {}", name, version, e));
                }
            }
        }

        Ok(download_path)
    }

    /// Download a package with a required checksum.
    ///
    /// `checksum` is required. If the caller genuinely wants to skip checksum
    /// verification (e.g. because the registry did not provide one and the
    /// user passed `--allow-unchecksummed` on the CLI), pass
    /// [`ALLOW_UNCHECKSUMMED_SENTINEL`]. Any other missing/empty value is
    /// rejected – there is no `Option::None` path.
    pub async fn download_with_checksum(
        &self,
        registry: &str,
        name: &str,
        version: &str,
        checksum: &str,
    ) -> Result<PathBuf> {
        if checksum.is_empty() {
            return Err(anyhow!(
                "Refusing to download {}@{}: checksum is required. \
                 Pass --allow-unchecksummed to bypass (audited).",
                name, version
            ));
        }
        match registry {
            "pub.dev" => self.download_from_pubdev_with_checksum(name, version, checksum).await,
            _ => Err(anyhow!("Unsupported registry: {}", registry)),
        }
    }

    pub fn get_pubdev_archive_url(name: &str, version: &str) -> String {
        format!(
            "{}/packages/{}/versions/{}.tar.gz",
            archive_base_url(), name, version
        )
    }

    pub async fn verify_checksum(
        &self,
        file_path: &Path,
        expected_sha256: Option<&str>,
    ) -> Result<bool> {
        if let Some(expected) = expected_sha256 {
            use sha2::{Sha256, Digest};
            use tokio::io::AsyncReadExt;

            let mut file = tokio::fs::File::open(file_path).await?;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0; 8192];

            loop {
                let bytes_read = file.read(&mut buffer).await?;
                if bytes_read == 0 {
                    break;
                }
                hasher.update(&buffer[..bytes_read]);
            }

            let result = hasher.finalize();
            let actual = format!("{:x}", result);

            if actual != expected {
                return Err(anyhow!(
                    "Checksum mismatch: expected {}, got {}",
                    expected,
                    actual
                ));
            }
        }

        Ok(true)
    }
}

impl Default for PackageDownloader {
    fn default() -> Self {
        Self::new()
    }
}