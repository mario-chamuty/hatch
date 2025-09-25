use anyhow::{anyhow, Result};
use reqwest::Client;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, info};

use super::paths::CachePaths;
use std::path::PathBuf;

pub struct PackageDownloader {
    client: Client,
}

impl PackageDownloader {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    pub async fn download_from_pubdev(
        &self,
        name: &str,
        version: &str,
    ) -> Result<PathBuf> {
        info!("Downloading {}@{} from pub.dev", name, version);

        CachePaths::ensure_directories()?;

        let download_path = CachePaths::download_path(name, version)?;
        if download_path.exists() {
            debug!("Package already downloaded: {}", download_path.display());
            return Ok(download_path);
        }

        let url = format!(
            "https://pub.dartlang.org/packages/{}/versions/{}.tar.gz",
            name, version
        );

        debug!("Download URL: {}", url);

        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} Downloading {msg}")
                .unwrap()
        );
        pb.set_message(format!("{}@{}", name, version));
        pb.enable_steady_tick(std::time::Duration::from_millis(100));

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

        if total_size > 0 {
            pb.set_length(total_size);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} {msg}")
                    .unwrap()
                    .progress_chars("#>-")
            );
        }

        let mut file = File::create(&download_path).await?;
        let mut downloaded = 0u64;
        let mut stream = response.bytes_stream();

        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow!("Download error: {}", e))?;
            file.write_all(&chunk).await?;

            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
        }

        pb.finish_with_message(format!("Downloaded {}@{}", name, version));

        Ok(download_path)
    }

    pub async fn download(
        &self,
        registry: &str,
        name: &str,
        version: &str,
    ) -> Result<PathBuf> {
        match registry {
            "pub.dev" => self.download_from_pubdev(name, version).await,
            _ => Err(anyhow!("Unsupported registry: {}", registry)),
        }
    }

    pub fn get_pubdev_archive_url(name: &str, version: &str) -> String {
        format!(
            "https://pub.dartlang.org/packages/{}/versions/{}.tar.gz",
            name, version
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