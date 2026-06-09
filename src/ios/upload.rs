//! Upload a signed `.ipa` to TestFlight using ONLY the App Store Connect API
//! key - no Mac, no app-specific password, no iTMSTransporter.
//!
//! iTMSTransporter can't deliver an IPA from Linux (its `swinfo` analysis is
//! macOS-only), and the popular `ios-uploader` authenticates the iris
//! content-delivery API with an Apple-ID + app-specific password. By decompiling
//! Transporter we found the API-key path it uses internally: the iris build
//! delivery API at the *non-provider* content-delivery URL, authenticated with a
//! plain Bearer JWT plus the `x-iris-client-audience: itmsTransporter` header.
//! See the `ios-testflight-upload-apikey` notes for the full reverse-engineering.

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use md5::{Digest, Md5};
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::config::AscCredentials;

const IRIS: &str = "https://contentdelivery.itunes.apple.com/MZContentDeliveryService/iris/v1";
const API: &str = "https://api.appstoreconnect.apple.com";
const IRIS_AUDIENCE: &str = "itmsTransporter";

/// Everything we need about the `.ipa`, read from the archive itself.
struct IpaInfo {
    bundle_id: String,
    short_version: String,
    version: String,
    /// The `.app` directory name inside `Payload/` (e.g. `Runner.app`).
    bundle_path: String,
    /// Raw `embedded.mobileprovision` bytes (the upload must declare it).
    provision: Vec<u8>,
    size: u64,
    file_name: String,
    md5_hex: String,
}

fn md5_hex(bytes: &[u8]) -> String {
    let mut h = Md5::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Read bundle metadata + the provisioning profile out of the IPA, and compute
/// the whole-file MD5 (streamed so we don't hold the whole IPA in memory twice).
fn read_ipa(path: &Path) -> Result<IpaInfo> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let size = file.metadata()?.len();
    let mut zip = zip::ZipArchive::new(file).context("reading .ipa (not a valid zip)")?;

    // Find the app's Info.plist (shortest `Payload/*.app/Info.plist`) and the
    // embedded provisioning profile.
    let mut info_name = None;
    let mut prov_name = None;
    for name in zip.file_names() {
        if name.starts_with("Payload/") && name.ends_with(".app/Info.plist") {
            if info_name.as_ref().map(|n: &String| name.len() < n.len()).unwrap_or(true) {
                info_name = Some(name.to_string());
            }
        }
        if name.starts_with("Payload/") && name.ends_with(".app/embedded.mobileprovision") {
            prov_name = Some(name.to_string());
        }
    }
    let info_name = info_name.context("no Payload/*.app/Info.plist in the .ipa")?;
    let app_dir = info_name.trim_end_matches("/Info.plist");
    let bundle_path = app_dir.rsplit('/').next().unwrap_or("Runner.app").to_string();

    let info: plist::Value = {
        let mut f = zip.by_name(&info_name)?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        plist::from_bytes(&buf).context("parsing the app's Info.plist")?
    };
    let dict = info.as_dictionary().context("Info.plist is not a dictionary")?;
    let get = |k: &str| -> Result<String> {
        dict.get(k)
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Info.plist missing {k}"))
    };

    let provision = match prov_name {
        Some(n) => {
            let mut f = zip.by_name(&n)?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            buf
        }
        None => {
            return Err(anyhow!(
                "the .ipa has no embedded.mobileprovision - sign it first (hatch ios build --sign)"
            ))
        }
    };

    // Stream the whole file for its MD5.
    let mut h = Md5::new();
    let mut rf = std::fs::File::open(path)?;
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let n = rf.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        h.update(&chunk[..n]);
    }
    let md5_hex = h.finalize().iter().map(|b| format!("{b:02x}")).collect();

    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().replace([':', ' '], "_"))
        .unwrap_or_else(|| "app.ipa".to_string());

    Ok(IpaInfo {
        bundle_id: get("CFBundleIdentifier")?,
        short_version: get("CFBundleShortVersionString")?,
        version: get("CFBundleVersion")?,
        bundle_path,
        provision,
        size,
        file_name,
        md5_hex,
    })
}

/// Build the binary `AppStoreInfo.plist` Apple's delivery API expects, derived
/// from the IPA (matches the structure `swinfo` would emit on macOS).
fn build_appstoreinfo(ipa: &IpaInfo) -> Result<Vec<u8>> {
    use plist::Value as P;
    let bundle = P::Dictionary({
        let mut d = plist::Dictionary::new();
        d.insert("CFBundleShortVersionString".into(), P::String(ipa.short_version.clone()));
        d.insert("CFBundleVersion".into(), P::String(ipa.version.clone()));
        d.insert("bundle-identifier".into(), P::String(ipa.bundle_id.clone()));
        d.insert("bundle-path".into(), P::String(ipa.bundle_path.clone()));
        d.insert("bundles".into(), P::Array(vec![]));
        d.insert("icons".into(), P::Array(vec![]));
        d.insert("platform-display-name".into(), P::String("iOS App".into()));
        d.insert("platform-id".into(), P::Integer(1i64.into()));
        d
    });
    let prov_file = P::Dictionary({
        let mut d = plist::Dictionary::new();
        d.insert("file-size".into(), P::Integer((ipa.provision.len() as i64).into()));
        d.insert("file-type".into(), P::String("NSFileTypeRegular".into()));
        d.insert("file-data".into(), P::String(STANDARD.encode(&ipa.provision)));
        d.insert("uti".into(), P::String("com.apple.mobileprovision".into()));
        d.insert("path".into(), P::String(format!("{}/embedded.mobileprovision", ipa.bundle_path)));
        d
    });
    let package = P::Dictionary({
        let mut d = plist::Dictionary::new();
        d.insert("bundles".into(), P::Array(vec![bundle]));
        d.insert("files".into(), P::Array(vec![prov_file]));
        d
    });
    let root = P::Dictionary({
        let mut meta = plist::Dictionary::new();
        meta.insert("archive-bytes".into(), P::Integer((ipa.size as i64).into()));
        meta.insert("file-name".into(), P::String(ipa.file_name.clone()));
        meta.insert("packages".into(), P::Array(vec![package]));
        let mut d = plist::Dictionary::new();
        d.insert("product-metadata".into(), P::Dictionary(meta));
        d
    });
    let mut out = Vec::new();
    plist::to_writer_binary(&mut out, &root).context("serializing AppStoreInfo.plist")?;
    Ok(out)
}

struct Iris {
    http: reqwest::Client,
    token: String,
}

impl Iris {
    fn new(token: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()?;
        Ok(Self { http, token })
    }

    /// One iris content-delivery request (non-provider path + audience header).
    async fn call(&self, method: reqwest::Method, path: &str, data: Option<Value>) -> Result<(u16, Value)> {
        let url = format!("{IRIS}/{path}");
        let mut req = self
            .http
            .request(method, &url)
            .bearer_auth(&self.token)
            .header("x-iris-client-audience", IRIS_AUDIENCE)
            .header("Accept", "application/json")
            .header("User-Agent", "iTMSTransporter/4.2.0");
        if let Some(d) = data {
            req = req.json(&json!({ "data": d }));
        }
        let resp = req.send().await.with_context(|| format!("iris {path}"))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        let val = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        Ok((status, val))
    }
}

fn iris_error(ctx: &str, status: u16, body: &Value) -> anyhow::Error {
    let detail = body
        .get("errors")
        .and_then(|e| e.get(0))
        .and_then(|e| e.get("detail").or_else(|| e.get("title")))
        .and_then(|d| d.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| body.to_string());
    anyhow!("{ctx} -> HTTP {status}: {detail}")
}

/// Run a delivery file's `uploadOperations`, PUT-ing each slice. `read_slice`
/// returns the bytes for `[offset, offset+length)` of the asset.
async fn run_operations(
    http: &reqwest::Client,
    ops: &[Value],
    mut read_slice: impl FnMut(u64, u64) -> Result<Vec<u8>>,
) -> Result<()> {
    for op in ops {
        let method = op.get("method").and_then(|m| m.as_str()).unwrap_or("PUT");
        let url = op.get("url").and_then(|u| u.as_str()).context("upload op missing url")?;
        let offset = op.get("offset").and_then(|o| o.as_u64()).unwrap_or(0);
        let length = op.get("length").and_then(|l| l.as_u64()).unwrap_or(0);
        let body = read_slice(offset, length)?;

        let mut req = http
            .request(reqwest::Method::from_bytes(method.as_bytes())?, url)
            .header("User-Agent", "iTMSTransporter/4.2.0");
        if let Some(hs) = op.get("requestHeaders").and_then(|h| h.as_array()) {
            for h in hs {
                if let (Some(n), Some(v)) = (
                    h.get("name").and_then(|n| n.as_str()),
                    h.get("value").and_then(|v| v.as_str()),
                ) {
                    req = req.header(n, v);
                }
            }
        }
        let resp = req.body(body).send().await.context("uploading chunk")?;
        if !resp.status().is_success() {
            anyhow::bail!("chunk upload failed: HTTP {}", resp.status().as_u16());
        }
    }
    Ok(())
}

/// Register a delivery file and return `(deliveryId, uploadOperations)`.
async fn register_file(
    iris: &Iris,
    build_id: &str,
    asset_type: &str,
    file_name: &str,
    file_size: u64,
    checksum: &str,
    uti: &str,
) -> Result<(String, Vec<Value>)> {
    let (st, body) = iris
        .call(
            reqwest::Method::POST,
            "buildDeliveryFiles",
            Some(json!({
                "type": "buildDeliveryFiles",
                "attributes": {
                    "assetType": asset_type,
                    "fileName": file_name,
                    "fileSize": file_size,
                    "sourceFileChecksum": checksum,
                    "uti": uti,
                },
                "relationships": { "build": { "data": { "id": build_id, "type": "builds" } } },
            })),
        )
        .await?;
    if st != 201 {
        return Err(iris_error(&format!("register {asset_type}"), st, &body));
    }
    let id = body["data"]["id"].as_str().context("no delivery file id")?.to_string();
    let ops = body["data"]["attributes"]["uploadOperations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok((id, ops))
}

/// Upload `ipa_path` to TestFlight for the app matching its bundle id.
pub async fn upload(creds: &AscCredentials, ipa_path: &Path) -> Result<()> {
    let token = super::appstore::mint_token(creds)?;
    let iris = Iris::new(token.clone())?;

    println!("🔎 Reading {}…", ipa_path.display());
    let ipa = read_ipa(ipa_path)?;
    println!(
        "   {} {} ({}) - {:.1} MB",
        ipa.bundle_id,
        ipa.short_version,
        ipa.version,
        ipa.size as f64 / 1.0e6
    );

    // 1. Resolve the app's numeric Apple ID via the public API.
    let http = reqwest::Client::new();
    let apps: Value = http
        .get(format!("{API}/v1/apps?filter[bundleId]={}", ipa.bundle_id))
        .bearer_auth(&token)
        .send()
        .await
        .context("looking up app")?
        .json()
        .await
        .context("parsing app lookup")?;
    let apple_id = apps["data"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|a| a["id"].as_str())
        .ok_or_else(|| anyhow!("no App Store Connect app for bundle id {}", ipa.bundle_id))?
        .to_string();

    // 2. Find an existing un-uploaded build for this version, else create one.
    let (st, body) = iris
        .call(
            reqwest::Method::GET,
            &format!("builds?filter[app]={apple_id}&filter[version]={}", ipa.version),
            None,
        )
        .await?;
    if st != 200 {
        return Err(iris_error("list builds", st, &body));
    }
    let mut build_id = None;
    if let Some(arr) = body["data"].as_array() {
        for b in arr {
            if !b["attributes"]["uploadedDate"].is_null() {
                anyhow::bail!(
                    "version {} (build {}) is already uploaded for this app",
                    ipa.version,
                    ipa.short_version
                );
            }
            build_id = b["id"].as_str().map(|s| s.to_string());
            break;
        }
    }
    let build_id = match build_id {
        Some(id) => id,
        None => {
            let (st, body) = iris
                .call(
                    reqwest::Method::POST,
                    "builds",
                    Some(json!({
                        "type": "builds",
                        "attributes": {
                            "cfBundleShortVersionString": ipa.short_version,
                            "cfBundleVersion": ipa.version,
                            "platform": "IOS",
                        },
                        "relationships": { "app": { "data": { "id": apple_id, "type": "apps" } } },
                    })),
                )
                .await?;
            if st != 201 {
                return Err(iris_error("create build", st, &body));
            }
            body["data"]["id"].as_str().context("no build id")?.to_string()
        }
    };

    // 3. Build the AppStoreInfo.plist and register both delivery files.
    let asi = build_appstoreinfo(&ipa)?;
    let asi_md5 = md5_hex(&asi);
    let (desc_id, desc_ops) = register_file(
        &iris,
        &build_id,
        "ASSET_DESCRIPTION",
        "AppStoreInfo.plist",
        asi.len() as u64,
        &asi_md5,
        "com.apple.binary-property-list",
    )
    .await?;
    let (asset_id, asset_ops) = register_file(
        &iris,
        &build_id,
        "ASSET",
        &ipa.file_name,
        ipa.size,
        &ipa.md5_hex,
        "com.apple.ipa",
    )
    .await?;

    // 4. Upload the description (in memory) and the IPA (streamed from disk).
    println!("📤 Uploading asset description…");
    run_operations(&iris.http, &desc_ops, |off, len| {
        let (a, b) = (off as usize, (off + len) as usize);
        Ok(asi.get(a..b.min(asi.len())).unwrap_or(&[]).to_vec())
    })
    .await?;

    println!("📤 Uploading IPA ({} chunk(s))…", asset_ops.len());
    let mut file = std::fs::File::open(ipa_path)?;
    run_operations(&iris.http, &asset_ops, |off, len| {
        file.seek(SeekFrom::Start(off))?;
        let mut buf = vec![0u8; len as usize];
        file.read_exact(&mut buf)?;
        Ok(buf)
    })
    .await?;

    // 5. Commit both files; the build flips to PROCESSING.
    for id in [&desc_id, &asset_id] {
        let (st, body) = iris
            .call(
                reqwest::Method::PATCH,
                &format!("buildDeliveryFiles/{id}"),
                Some(json!({ "type": "buildDeliveryFiles", "id": id, "attributes": { "uploaded": true } })),
            )
            .await?;
        if st != 200 {
            return Err(iris_error("commit upload", st, &body));
        }
    }

    Ok(())
}
