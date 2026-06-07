//! Persistent iOS configuration: App Store Connect API credentials and
//! toolchain location. Stored at `~/.hatch/ios.json`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// App Store Connect API key (the modern, 2FA-free auth Apple recommends for
/// automation). Created at App Store Connect -> Users and Access -> Integrations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AscCredentials {
    /// Issuer ID (a UUID shown above the keys table).
    pub issuer_id: String,
    /// Key ID (the 10-char identifier of the key).
    pub key_id: String,
    /// Path (host path) to the downloaded `AuthKey_<KEYID>.p8`.
    pub p8_path: String,
}

/// Signing material produced by `hatch ios cert-create` / `profile-create`,
/// expressed as paths inside the build environment, reused by `build --sign`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SigningMaterial {
    pub p12_path: Option<String>,
    pub p12_password: Option<String>,
    pub profile_path: Option<String>,
    pub certificate_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IosConfig {
    /// App Store Connect API credentials (None until `hatch ios auth`).
    #[serde(default)]
    pub asc: Option<AscCredentials>,

    /// Cached signing material for `build --sign`.
    #[serde(default)]
    pub signing: SigningMaterial,

    /// WSL distro to run the Linux toolchain in (Windows only). Ignored on Linux.
    #[serde(default)]
    pub wsl_distro: Option<String>,

    /// Toolchain root inside the build environment (default: `$HOME/iospoc`).
    #[serde(default = "default_toolchain_root")]
    pub toolchain_root: String,

    /// Apple Developer Team ID (10 chars), used for profile/signing defaults.
    #[serde(default)]
    pub team_id: Option<String>,

    /// Default bundle identifier for builds/profiles.
    #[serde(default)]
    pub bundle_id: Option<String>,
}

fn default_toolchain_root() -> String {
    "$HOME/iospoc".to_string()
}

impl Default for IosConfig {
    fn default() -> Self {
        Self {
            asc: None,
            signing: SigningMaterial::default(),
            wsl_distro: None,
            toolchain_root: default_toolchain_root(),
            team_id: None,
            bundle_id: None,
        }
    }
}

impl IosConfig {
    pub fn config_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("could not resolve home directory")?;
        Ok(home.join(".hatch").join("ios.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// App Store Connect credentials or a clear error pointing at `hatch ios auth`.
    pub fn require_asc(&self) -> Result<&AscCredentials> {
        self.asc.as_ref().context(
            "no App Store Connect API key configured. Run:\n  \
             hatch ios auth --issuer <ISSUER_ID> --key-id <KEY_ID> --p8 <AuthKey_XXX.p8>",
        )
    }
}
