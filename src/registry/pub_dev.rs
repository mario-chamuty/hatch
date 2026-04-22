use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use log::{debug, info, warn};

use super::traits::{Registry, PackageMetadata, PackageVersion};

#[derive(Clone)]
pub struct PubDevRegistry {
    client: Client,
    base_url: String,
}

impl PubDevRegistry {
    /// Default registry URL used when no override is in effect.
    pub const DEFAULT_BASE_URL: &'static str = "https://pub.dev";

    pub fn new() -> Self {
        // Create client with connection pooling and HTTP/2 for faster parallel requests
        let client = Client::builder()
            .pool_max_idle_per_host(50)
            .pool_idle_timeout(std::time::Duration::from_secs(60))
            .timeout(std::time::Duration::from_secs(15))
            .http2_adaptive_window(true)
            .build()
            .unwrap_or_else(|_| Client::new());

        // `HATCH_PUB_HOSTED_URL` is the test/E2E override. `PUB_HOSTED_URL`
        // is honoured too so users can point Hatch at a mirror (e.g. a
        // corporate proxy) without code changes.
        let base_url = std::env::var("HATCH_PUB_HOSTED_URL")
            .ok()
            .or_else(|| std::env::var("PUB_HOSTED_URL").ok())
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| Self::DEFAULT_BASE_URL.to_string());

        Self {
            client,
            base_url,
        }
    }

    /// Construct a registry that always targets the given base URL, ignoring
    /// any `HATCH_PUB_HOSTED_URL` / `PUB_HOSTED_URL` environment variables.
    /// Primarily used by integration tests that spin up a wiremock server.
    pub fn with_base_url(url: String) -> Self {
        let client = Client::builder()
            .pool_max_idle_per_host(50)
            .pool_idle_timeout(std::time::Duration::from_secs(60))
            .timeout(std::time::Duration::from_secs(15))
            .http2_adaptive_window(true)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            base_url: url.trim_end_matches('/').to_string(),
        }
    }

    fn value_to_string(value: &Value) -> Option<String> {
        match value {
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
    }

    fn value_to_constraint(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            Value::Object(obj) => {
                if let Some(Value::String(sdk)) = obj.get("sdk") {
                    format!("{{\"sdk\": \"{}\"}}", sdk)
                } else if let Some(Value::String(version)) = obj.get("version") {
                    version.clone()
                } else {
                    "any".to_string()
                }
            },
            _ => "any".to_string(),
        }
    }

    pub fn with_url(url: String) -> Self {
        Self {
            client: Client::new(),
            base_url: url,
        }
    }
}

/// Error returned when a pub.dev version-detail response is missing the
/// `archive_sha256` field. Surfaced as its own variant so callers can
/// recognise the "missing checksum" case distinctly from "network failure".
#[derive(Debug, thiserror::Error)]
#[error("registry did not provide archive_sha256 for {name}@{version}")]
pub struct MissingChecksum {
    pub name: String,
    pub version: String,
}

#[async_trait]
impl Registry for PubDevRegistry {
    async fn get_package_metadata(&self, name: &str) -> Result<PackageMetadata> {
        debug!("Fetching package metadata for: {}", name);

        let url = format!("{}/api/packages/{}", self.base_url, name);
        let response = self.client
            .get(&url)
            .header("User-Agent", "hatch-cli/1.0.0")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("Package '{}' not found on pub.dev", name));
        }

        let pub_package: PubDevPackage = response.json().await?;

        let mut versions = Vec::new();
        for version_info in &pub_package.versions {
            if let Some(pubspec) = &version_info.pubspec {
                // `archive_sha256` MUST be populated on the version-list
                // endpoint according to pub.dev's API docs. If it is not,
                // surface it as a distinct error so callers (the install
                // command) can decide whether to bail or ride with
                // `--allow-unchecksummed`.
                let archive_sha256 = match version_info.archive_sha256.as_ref() {
                    Some(s) if !s.is_empty() => Some(s.clone()),
                    _ => {
                        warn!(
                            "pub.dev /api/packages/{} version {} is missing archive_sha256",
                            name, version_info.version
                        );
                        None
                    }
                };

                let version = PackageVersion {
                    version: version_info.version.clone(),
                    description: pubspec.description.clone(),
                    homepage: pubspec.homepage.clone(),
                    repository: pubspec.repository.clone(),
                    dependencies: pubspec.dependencies.as_ref()
                        .map(|deps| deps.iter()
                            .map(|(k, v)| (k.clone(), Self::value_to_constraint(v)))
                            .collect())
                        .unwrap_or_default(),
                    dev_dependencies: pubspec.dev_dependencies.as_ref()
                        .map(|deps| deps.iter()
                            .map(|(k, v)| (k.clone(), Self::value_to_constraint(v)))
                            .collect())
                        .unwrap_or_default(),
                    published: Some(version_info.published.clone()),
                    dart_sdk: pubspec.environment.as_ref()
                        .and_then(|env| env.get("sdk"))
                        .and_then(Self::value_to_string),
                    flutter_sdk: pubspec.environment.as_ref()
                        .and_then(|env| env.get("flutter"))
                        .and_then(Self::value_to_string),
                    archive_sha256,
                };
                versions.push(version);
            }
        }

        // pub.dev returns versions oldest-first, so last() is the latest
        let latest = versions.last()
            .ok_or_else(|| anyhow!("No versions found for package '{}'", name))?
            .clone();

        Ok(PackageMetadata {
            name: pub_package.name,
            description: latest.description.clone(),
            latest,
            versions,
        })
    }

    async fn get_version_metadata(&self, name: &str, version: &str) -> Result<PackageVersion> {
        debug!("Fetching version metadata for: {}@{}", name, version);

        let url = format!("{}/api/packages/{}/versions/{}", self.base_url, name, version);
        let response = self.client
            .get(&url)
            .header("User-Agent", "hatch-cli/1.0.0")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("Version '{}@{}' not found on pub.dev", name, version));
        }

        let version_info: PubDevVersionInfo = response.json().await?;

        if let Some(pubspec) = &version_info.pubspec {
            Ok(PackageVersion {
                version: version_info.version.clone(),
                description: pubspec.description.clone(),
                homepage: pubspec.homepage.clone(),
                repository: pubspec.repository.clone(),
                dependencies: pubspec.dependencies.as_ref()
                    .map(|deps| deps.iter()
                        .map(|(k, v)| (k.clone(), Self::value_to_constraint(v)))
                        .collect())
                    .unwrap_or_default(),
                dev_dependencies: pubspec.dev_dependencies.as_ref()
                    .map(|deps| deps.iter()
                        .map(|(k, v)| (k.clone(), Self::value_to_constraint(v)))
                        .collect())
                    .unwrap_or_default(),
                published: Some(version_info.published),
                dart_sdk: pubspec.environment.as_ref()
                    .and_then(|env| env.get("sdk"))
                    .and_then(Self::value_to_string),
                flutter_sdk: pubspec.environment.as_ref()
                    .and_then(|env| env.get("flutter"))
                    .and_then(Self::value_to_string),
                archive_sha256: version_info.archive_sha256,
            })
        } else {
            Err(anyhow!("Invalid version data for {}@{}", name, version))
        }
    }

    async fn search_packages(&self, query: &str) -> Result<Vec<PackageMetadata>> {
        debug!("Searching packages with query: {}", query);

        let url = format!("{}/api/search?q={}", self.base_url, query);
        let response = self.client
            .get(&url)
            .header("User-Agent", "hatch-cli/1.0.0")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("Search failed"));
        }

        let search_result: PubDevSearchResult = response.json().await?;

        let mut results = Vec::new();
        for package in search_result.packages {
            let latest = PackageVersion {
                version: package.latest.version,
                description: package.latest.pubspec.description.clone(),
                homepage: package.latest.pubspec.homepage.clone(),
                repository: package.latest.pubspec.repository.clone(),
                dependencies: package.latest.pubspec.dependencies.as_ref()
                    .map(|deps| deps.iter()
                        .map(|(k, v)| (k.clone(), Self::value_to_constraint(v)))
                        .collect())
                    .unwrap_or_default(),
                dev_dependencies: package.latest.pubspec.dev_dependencies.as_ref()
                    .map(|deps| deps.iter()
                        .map(|(k, v)| (k.clone(), Self::value_to_constraint(v)))
                        .collect())
                    .unwrap_or_default(),
                published: None,
                dart_sdk: package.latest.pubspec.environment.as_ref()
                    .and_then(|env| env.get("sdk"))
                    .and_then(Self::value_to_string),
                flutter_sdk: package.latest.pubspec.environment.as_ref()
                    .and_then(|env| env.get("flutter"))
                    .and_then(Self::value_to_string),
                archive_sha256: None, // Search results don't include checksums
            };

            results.push(PackageMetadata {
                name: package.package.clone(),
                description: latest.description.clone(),
                latest: latest.clone(),
                versions: vec![latest],
            });
        }

        Ok(results)
    }

    async fn package_exists(&self, name: &str) -> Result<bool> {
        debug!("Checking if package exists: {}", name);

        let url = format!("{}/api/packages/{}", self.base_url, name);
        let response = self.client
            .head(&url)
            .header("User-Agent", "hatch-cli/1.0.0")
            .send()
            .await?;

        Ok(response.status().is_success())
    }

    async fn get_available_versions(&self, name: &str) -> Result<Vec<String>> {
        debug!("Getting available versions for: {}", name);

        let metadata = self.get_package_metadata(name).await?;
        let versions: Vec<String> = metadata.versions
            .into_iter()
            .map(|v| v.version)
            .collect();

        Ok(versions)
    }

    fn registry_url(&self) -> &str {
        &self.base_url
    }

    fn registry_name(&self) -> &str {
        "pub.dev"
    }
}

#[derive(Debug, Deserialize)]
struct PubDevPackage {
    name: String,
    versions: Vec<PubDevVersionInfo>,
}

#[derive(Debug, Deserialize)]
struct PubDevVersionInfo {
    version: String,
    published: String,
    pubspec: Option<PubDevPubspec>,
    archive_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PubDevPubspec {
    name: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    repository: Option<String>,
    dependencies: Option<HashMap<String, Value>>,
    dev_dependencies: Option<HashMap<String, Value>>,
    environment: Option<HashMap<String, Value>>,
}

#[derive(Debug, Deserialize)]
struct PubDevSearchResult {
    packages: Vec<PubDevSearchPackage>,
}

#[derive(Debug, Deserialize)]
struct PubDevSearchPackage {
    package: String,
    latest: PubDevSearchLatest,
}

#[derive(Debug, Deserialize)]
struct PubDevSearchLatest {
    version: String,
    pubspec: PubDevPubspec,
}

impl Default for PubDevRegistry {
    fn default() -> Self {
        Self::new()
    }
}