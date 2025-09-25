use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use log::{debug, info};

use super::traits::{Registry, PackageMetadata, PackageVersion};

pub struct PubDevRegistry {
    client: Client,
    base_url: String,
}

impl PubDevRegistry {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: "https://pub.dev".to_string(),
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
                        .cloned(),
                    flutter_sdk: pubspec.environment.as_ref()
                        .and_then(|env| env.get("flutter"))
                        .cloned(),
                };
                versions.push(version);
            }
        }

        let latest = versions.first()
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
                version: version_info.version,
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
                    .cloned(),
                flutter_sdk: pubspec.environment.as_ref()
                    .and_then(|env| env.get("flutter"))
                    .cloned(),
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
                    .cloned(),
                flutter_sdk: package.latest.pubspec.environment.as_ref()
                    .and_then(|env| env.get("flutter"))
                    .cloned(),
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
}

#[derive(Debug, Deserialize)]
struct PubDevPubspec {
    name: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    repository: Option<String>,
    dependencies: Option<HashMap<String, Value>>,
    dev_dependencies: Option<HashMap<String, Value>>,
    environment: Option<HashMap<String, String>>,
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