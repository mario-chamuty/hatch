//! Minimal App Store Connect API client.
//!
//! Auth uses an ES256 JWT signed with the team's `.p8` API key (issuer id +
//! key id), exactly as Apple documents for automation. We expose the handful
//! of endpoints needed to fetch apps/devices and to mint signing certificates
//! and provisioning profiles from the CLI.

use anyhow::{anyhow, Context, Result};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::Serialize;
use serde_json::{json, Value};

use super::config::AscCredentials;

const API_BASE: &str = "https://api.appstoreconnect.apple.com";
const AUDIENCE: &str = "appstoreconnect-v1";

#[derive(Serialize)]
struct Claims {
    iss: String,
    iat: i64,
    exp: i64,
    aud: String,
}

/// Mint a short-lived ES256 bearer token from the API key. Shared by the REST
/// client and the iris content-delivery uploader (`super::upload`).
pub fn mint_token(creds: &AscCredentials) -> Result<String> {
    let pem = std::fs::read(&creds.p8_path)
        .with_context(|| format!("reading API key file {}", creds.p8_path))?;
    let key = EncodingKey::from_ec_pem(&pem)
        .context("invalid .p8 key (expected a PKCS#8 EC private key from App Store Connect)")?;

    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        iss: creds.issuer_id.clone(),
        iat: now,
        exp: now + 19 * 60, // Apple requires <= 20 minutes
        aud: AUDIENCE.to_string(),
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(creds.key_id.clone());
    header.typ = Some("JWT".to_string());
    encode(&header, &claims, &key).context("signing App Store Connect JWT")
}

pub struct AppStoreClient {
    http: reqwest::Client,
    token: String,
}

impl AppStoreClient {
    /// Build a client and mint a short-lived bearer token.
    pub fn new(creds: &AscCredentials) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::new(),
            token: mint_token(creds)?,
        })
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let url = format!("{API_BASE}{path}");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("GET {path}"))?;
        Self::parse(resp, &format!("GET {path}")).await
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let url = format!("{API_BASE}{path}");
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {path}"))?;
        Self::parse(resp, &format!("POST {path}")).await
    }

    async fn parse(resp: reqwest::Response, ctx: &str) -> Result<Value> {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "{ctx} -> HTTP {}: {}",
                status.as_u16(),
                apple_error_detail(&text)
            ));
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).with_context(|| format!("{ctx}: invalid JSON response"))
    }

    /// Validate the credentials without hard-failing on business-level blocks.
    /// A 401 means the key/issuer/.p8 are wrong; a 403 means the key is valid
    /// but the account is missing a role or a signed agreement.
    pub async fn verify(&self) -> Result<Access> {
        let url = format!("{API_BASE}/v1/apps?limit=1");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("contacting App Store Connect")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() {
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let n = v["meta"]["paging"]["total"]
                .as_u64()
                .map(|t| t as usize)
                .or_else(|| v["data"].as_array().map(|a| a.len()))
                .unwrap_or(0);
            return Ok(Access::Ok(n));
        }
        if status.as_u16() == 403 {
            return Ok(Access::Forbidden(apple_error_detail(&text)));
        }
        Err(anyhow!(
            "HTTP {}: {} (check the issuer ID, key ID and .p8 match the same key)",
            status.as_u16(),
            apple_error_detail(&text)
        ))
    }

    // --- Apps / bundle IDs ------------------------------------------------

    pub async fn list_apps(&self) -> Result<Vec<App>> {
        let v = self.get("/v1/apps?limit=200").await?;
        Ok(v["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|a| App {
                id: a["id"].as_str().unwrap_or_default().to_string(),
                name: a["attributes"]["name"].as_str().unwrap_or_default().to_string(),
                bundle_id: a["attributes"]["bundleId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                sku: a["attributes"]["sku"].as_str().unwrap_or_default().to_string(),
            })
            .collect())
    }

    pub async fn list_bundle_ids(&self) -> Result<Vec<BundleId>> {
        let v = self.get("/v1/bundleIds?limit=200&filter[platform]=IOS").await?;
        Ok(v["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|b| BundleId {
                id: b["id"].as_str().unwrap_or_default().to_string(),
                identifier: b["attributes"]["identifier"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                name: b["attributes"]["name"].as_str().unwrap_or_default().to_string(),
            })
            .collect())
    }

    /// Find an existing bundle id resource by its identifier (e.g. com.acme.app).
    pub async fn find_bundle_id(&self, identifier: &str) -> Result<Option<BundleId>> {
        Ok(self
            .list_bundle_ids()
            .await?
            .into_iter()
            .find(|b| b.identifier == identifier))
    }

    pub async fn create_bundle_id(&self, identifier: &str, name: &str) -> Result<BundleId> {
        let body = json!({
            "data": {
                "type": "bundleIds",
                "attributes": { "identifier": identifier, "name": name, "platform": "IOS" }
            }
        });
        let v = self.post("/v1/bundleIds", body).await?;
        Ok(BundleId {
            id: v["data"]["id"].as_str().unwrap_or_default().to_string(),
            identifier: identifier.to_string(),
            name: name.to_string(),
        })
    }

    // --- Devices ----------------------------------------------------------

    pub async fn list_devices(&self) -> Result<Vec<Device>> {
        let v = self.get("/v1/devices?limit=200").await?;
        Ok(v["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|d| Device {
                id: d["id"].as_str().unwrap_or_default().to_string(),
                name: d["attributes"]["name"].as_str().unwrap_or_default().to_string(),
                udid: d["attributes"]["udid"].as_str().unwrap_or_default().to_string(),
                class: d["attributes"]["deviceClass"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            })
            .collect())
    }

    pub async fn register_device(&self, name: &str, udid: &str) -> Result<Device> {
        let body = json!({
            "data": {
                "type": "devices",
                "attributes": { "name": name, "udid": udid, "platform": "IOS" }
            }
        });
        let v = self.post("/v1/devices", body).await?;
        Ok(Device {
            id: v["data"]["id"].as_str().unwrap_or_default().to_string(),
            name: name.to_string(),
            udid: udid.to_string(),
            class: String::new(),
        })
    }

    // --- Certificates -----------------------------------------------------

    pub async fn list_certificates(&self) -> Result<Vec<Certificate>> {
        let v = self.get("/v1/certificates?limit=200").await?;
        Ok(v["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(Self::cert_from_json)
            .collect())
    }

    /// Submit a CSR (PEM text) and receive a signed certificate. `cert_type`
    /// is e.g. "IOS_DEVELOPMENT" or "IOS_DISTRIBUTION".
    pub async fn create_certificate(&self, csr_pem: &str, cert_type: &str) -> Result<Certificate> {
        let body = json!({
            "data": {
                "type": "certificates",
                "attributes": { "csrContent": csr_pem, "certificateType": cert_type }
            }
        });
        let v = self.post("/v1/certificates", body).await?;
        Ok(Self::cert_from_json(&v["data"]))
    }

    fn cert_from_json(c: &Value) -> Certificate {
        Certificate {
            id: c["id"].as_str().unwrap_or_default().to_string(),
            name: c["attributes"]["name"].as_str().unwrap_or_default().to_string(),
            cert_type: c["attributes"]["certificateType"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            expiration: c["attributes"]["expirationDate"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            // base64 DER, present on create and on detail fetches
            content: c["attributes"]["certificateContent"]
                .as_str()
                .map(|s| s.to_string()),
        }
    }

    // --- Profiles ---------------------------------------------------------

    pub async fn list_profiles(&self) -> Result<Vec<Profile>> {
        let v = self.get("/v1/profiles?limit=200").await?;
        Ok(v["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(Self::profile_from_json)
            .collect())
    }

    /// Create a provisioning profile linking a bundle id, certificate(s) and
    /// (for development profiles) device(s). `profile_type` is e.g.
    /// "IOS_APP_DEVELOPMENT" or "IOS_APP_STORE".
    pub async fn create_profile(
        &self,
        name: &str,
        profile_type: &str,
        bundle_id: &str,
        certificate_ids: &[String],
        device_ids: &[String],
    ) -> Result<Profile> {
        let certs: Vec<Value> = certificate_ids
            .iter()
            .map(|id| json!({ "type": "certificates", "id": id }))
            .collect();
        let devices: Vec<Value> = device_ids
            .iter()
            .map(|id| json!({ "type": "devices", "id": id }))
            .collect();
        let mut relationships = json!({
            "bundleId": { "data": { "type": "bundleIds", "id": bundle_id } },
            "certificates": { "data": certs },
        });
        if !devices.is_empty() {
            relationships["devices"] = json!({ "data": devices });
        }
        let body = json!({
            "data": {
                "type": "profiles",
                "attributes": { "name": name, "profileType": profile_type },
                "relationships": relationships
            }
        });
        let v = self.post("/v1/profiles", body).await?;
        Ok(Self::profile_from_json(&v["data"]))
    }

    fn profile_from_json(p: &Value) -> Profile {
        Profile {
            id: p["id"].as_str().unwrap_or_default().to_string(),
            name: p["attributes"]["name"].as_str().unwrap_or_default().to_string(),
            profile_type: p["attributes"]["profileType"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            state: p["attributes"]["profileState"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            // base64 of the .mobileprovision
            content: p["attributes"]["profileContent"].as_str().map(|s| s.to_string()),
        }
    }
}

/// Result of a credential check.
pub enum Access {
    /// Authenticated and authorized; `usize` is the visible app count.
    Ok(usize),
    /// Authenticated but blocked (missing role or unsigned agreement). The
    /// string is Apple's human-readable reason.
    Forbidden(String),
}

/// Flatten Apple's structured `errors` array into a readable one-liner.
fn apple_error_detail(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| {
            v.get("errors").and_then(|e| e.as_array()).map(|errs| {
                errs.iter()
                    .map(|er| {
                        format!(
                            "{}: {}",
                            er.get("title").and_then(Value::as_str).unwrap_or("error"),
                            er.get("detail").and_then(Value::as_str).unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
        })
        .unwrap_or_else(|| text.to_string())
}

#[derive(Debug, Clone)]
pub struct App {
    pub id: String,
    pub name: String,
    pub bundle_id: String,
    pub sku: String,
}

#[derive(Debug, Clone)]
pub struct BundleId {
    pub id: String,
    pub identifier: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub udid: String,
    pub class: String,
}

#[derive(Debug, Clone)]
pub struct Certificate {
    pub id: String,
    pub name: String,
    pub cert_type: String,
    pub expiration: String,
    pub content: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub profile_type: String,
    pub state: String,
    pub content: Option<String>,
}
