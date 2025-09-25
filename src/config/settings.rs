use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Runtime settings and configuration (stored in memory, derived from manifest)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSettings {
    pub flutter: FlutterSettings,
    pub registries: HashMap<String, String>,
    pub build: Option<BuildSettings>,
    pub cache: CacheSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlutterSettings {
    pub version: Option<String>,
    pub fvm_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildSettings {
    pub backend_url: Option<String>,
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSettings {
    pub ttl: u64,
    pub max_size: String,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        let mut registries = HashMap::new();
        registries.insert("pub".to_string(), "https://pub.dev".to_string());
        registries.insert("hatch".to_string(), "https://hatch.dev".to_string());

        Self {
            flutter: FlutterSettings::default(),
            registries,
            build: None,
            cache: CacheSettings::default(),
        }
    }
}

impl Default for FlutterSettings {
    fn default() -> Self {
        Self {
            version: None,
            fvm_path: None,
        }
    }
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self {
            ttl: 3600,
            max_size: "100MB".to_string(),
        }
    }
}

impl RuntimeSettings {
    /// Create runtime settings from manifest data
    pub fn from_manifest(
        flutter_version: Option<String>,
        build_config: Option<BuildSettings>,
        registries: Option<HashMap<String, String>>,
    ) -> Self {
        let mut settings = Self::default();
        settings.flutter.version = flutter_version;
        settings.build = build_config;

        if let Some(reg) = registries {
            settings.registries.extend(reg);
        }

        settings
    }
}