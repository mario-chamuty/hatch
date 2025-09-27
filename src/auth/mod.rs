use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use log::{debug, info};

/// Authentication configuration for Hatch
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HatchAuth {
    /// Authentication tokens for different nests
    pub nests: Option<HashMap<String, NestAuth>>,

    /// Global pub.dev token (if needed for publishing)
    #[serde(rename = "pub-token")]
    pub pub_token: Option<String>,

    /// Git credentials
    pub git: Option<HashMap<String, GitAuth>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NestAuth {
    pub token: String,
    pub username: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitAuth {
    pub username: Option<String>,
    pub token: Option<String>,
    pub ssh_key: Option<String>,
}

pub struct AuthManager;

impl AuthManager {
    /// Load authentication configuration
    pub fn load() -> Result<HatchAuth> {
        // Try loading in order of precedence:
        // 1. hatch_auth.local.json (local overrides, gitignored)
        // 2. hatch_auth.json (project-specific)
        // 3. ~/.hatch/auth.json (user global)

        let configs = vec![
            PathBuf::from("hatch_auth.local.json"),
            PathBuf::from("hatch_auth.json"),
            Self::global_auth_path()?,
        ];

        let mut auth = HatchAuth::default();

        for config_path in configs {
            if config_path.exists() {
                debug!("Loading auth from: {}", config_path.display());
                if let Ok(content) = std::fs::read_to_string(&config_path) {
                    if let Ok(loaded_auth) = serde_json::from_str::<HatchAuth>(&content) {
                        auth = Self::merge_auth(auth, loaded_auth);
                    }
                }
            }
        }

        Ok(auth)
    }

    /// Get the global auth file path
    fn global_auth_path() -> Result<PathBuf> {
        let home = home::home_dir()
            .ok_or_else(|| anyhow!("Could not determine home directory"))?;
        Ok(home.join(".hatch").join("auth.json"))
    }

    /// Merge authentication configurations (later overrides earlier)
    fn merge_auth(mut base: HatchAuth, overlay: HatchAuth) -> HatchAuth {
        if overlay.pub_token.is_some() {
            base.pub_token = overlay.pub_token;
        }

        if let Some(overlay_nests) = overlay.nests {
            let base_nests = base.nests.get_or_insert_with(HashMap::new);
            for (name, auth) in overlay_nests {
                base_nests.insert(name, auth);
            }
        }

        if let Some(overlay_git) = overlay.git {
            let base_git = base.git.get_or_insert_with(HashMap::new);
            for (url, auth) in overlay_git {
                base_git.insert(url, auth);
            }
        }

        base
    }

    /// Get authentication for a specific nest
    pub fn get_nest_auth(nest_name: &str) -> Result<Option<NestAuth>> {
        let auth = Self::load()?;
        Ok(auth.nests.and_then(|nests| nests.get(nest_name).cloned()))
    }

    /// Save authentication to local file
    pub fn save_local_auth(auth: &HatchAuth) -> Result<()> {
        let path = PathBuf::from("hatch_auth.local.json");
        let json = serde_json::to_string_pretty(auth)?;
        std::fs::write(&path, json)?;
        info!("Saved authentication to hatch_auth.local.json");
        Ok(())
    }

    /// Add or update nest authentication
    pub fn add_nest_auth(nest_name: String, token: String, username: Option<String>) -> Result<()> {
        let mut auth = Self::load()?;
        let nests = auth.nests.get_or_insert_with(HashMap::new);

        nests.insert(nest_name.clone(), NestAuth {
            token,
            username: username.clone(),
            email: None,
        });

        Self::save_local_auth(&auth)?;
        info!("Added authentication for nest: {}", nest_name);
        Ok(())
    }

    /// Remove nest authentication
    pub fn remove_nest_auth(nest_name: &str) -> Result<()> {
        let mut auth = Self::load()?;

        if let Some(ref mut nests) = auth.nests {
            if nests.remove(nest_name).is_some() {
                Self::save_local_auth(&auth)?;
                info!("Removed authentication for nest: {}", nest_name);
            } else {
                return Err(anyhow!("No authentication found for nest: {}", nest_name));
            }
        }

        Ok(())
    }

    /// Create a sample auth file
    pub fn create_sample_auth_file(path: &Path) -> Result<()> {
        let sample = HatchAuth {
            nests: Some(HashMap::from([
                ("v2.sk".to_string(), NestAuth {
                    token: "YOUR_TOKEN_HERE".to_string(),
                    username: Some("your_username".to_string()),
                    email: Some("your_email@example.com".to_string()),
                }),
            ])),
            pub_token: Some("YOUR_PUB_TOKEN_HERE".to_string()),
            git: Some(HashMap::from([
                ("github.com".to_string(), GitAuth {
                    username: Some("your_github_username".to_string()),
                    token: Some("YOUR_GITHUB_TOKEN".to_string()),
                    ssh_key: None,
                }),
            ])),
        };

        let json = serde_json::to_string_pretty(&sample)?;
        std::fs::write(path, json)?;

        info!("Created sample auth file at: {}", path.display());
        info!("Please update it with your actual credentials");

        Ok(())
    }
}