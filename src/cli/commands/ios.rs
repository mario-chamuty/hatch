//! `hatch ios ...` command handlers: doctor, auth, setup, build, and App Store
//! Connect resource management (apps, devices, certificates, profiles).

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

use crate::cli::IosCommands;
use crate::ios::appstore::AppStoreClient;
use crate::ios::builder::{self, BuildRequest};
use crate::ios::config::{AscCredentials, IosConfig};
use crate::ios::publish as publisher;
use crate::ios::runner::to_build_path;
use crate::ios::signing;
use crate::ios::toolchain::Toolchain;
use crate::ios::{runner_for, Runner};

pub async fn execute(cmd: IosCommands) -> Result<()> {
    match cmd {
        IosCommands::Doctor => doctor().await,
        IosCommands::Auth { web, issuer, key_id, p8, team_id } => {
            auth(web, issuer, key_id, p8, team_id).await
        }
        IosCommands::Setup { flutter } => setup(flutter).await,
        IosCommands::Build { bundle_id, name, sign, distribution } => {
            build(bundle_id, name, sign, distribution).await
        }
        IosCommands::Publish { ipa } => publish(ipa).await,
        IosCommands::Apps => apps().await,
        IosCommands::Devices => devices().await,
        IosCommands::DeviceAdd { name, udid } => device_add(name, udid).await,
        IosCommands::Certs => certs().await,
        IosCommands::CertCreate { distribution, password } => cert_create(distribution, password).await,
        IosCommands::Profiles => profiles().await,
        IosCommands::ProfileCreate { name, bundle_id, distribution, device } => {
            profile_create(name, bundle_id, distribution, device).await
        }
    }
}

// --- doctor / setup -------------------------------------------------------

async fn doctor() -> Result<()> {
    let cfg = IosConfig::load()?;
    let runner = runner_for(&cfg);
    let tc = Toolchain::new(&runner, &cfg.toolchain_root);
    println!("🩺 Hatch iOS toolchain ({})", describe_runner(&runner));
    let checks = tc.doctor()?;
    let mut all_ok = true;
    for c in &checks {
        let mark = if c.ok { "✅" } else { "❌" };
        if !c.ok {
            all_ok = false;
        }
        println!("  {mark} {:<26} {}", c.name, c.detail);
    }
    println!();
    match &cfg.asc {
        Some(a) => println!("  ✅ App Store Connect       issuer {}…, key {}", short(&a.issuer_id), a.key_id),
        None => println!("  ⚠️  App Store Connect       not configured (run `hatch ios auth`)"),
    }
    if all_ok {
        println!("\nToolchain looks good. Build with: hatch ios build");
    } else {
        println!("\nSome tools are missing. Provision them with: hatch ios setup");
    }
    Ok(())
}

async fn setup(flutter: Option<String>) -> Result<()> {
    let cfg = IosConfig::load()?;
    let runner = runner_for(&cfg);
    let tc = Toolchain::new(&runner, &cfg.toolchain_root);
    let project = std::env::current_dir()?;
    let version = flutter
        .or_else(|| resolve_flutter_version(&project))
        .context("could not determine Flutter version; pass --flutter <version>")?;
    let hash = resolve_engine_hash(&version)
        .with_context(|| format!("resolving engine hash for Flutter {version}"))?;
    println!("📦 Provisioning engine artifacts for Flutter {version} (engine {})…", short(&hash));
    tc.ensure_engine(&hash)?;
    println!("✅ Engine artifacts ready.");
    println!("\nNote: the cross toolchain (cctools/ld64), zsign and the iOS SDK are");
    println!("provisioned once out-of-band; run `hatch ios doctor` to verify all parts.");
    Ok(())
}

// --- auth -----------------------------------------------------------------

const ASC_KEYS_URL: &str = "https://appstoreconnect.apple.com/access/integrations/api";

async fn auth(
    web: bool,
    issuer: Option<String>,
    key_id: Option<String>,
    p8: Option<String>,
    team_id: Option<String>,
) -> Result<()> {
    // Resolve the three credential parts, browser-assisted when requested or
    // when they weren't all supplied on the command line.
    let (issuer, key_id, p8) = if web || issuer.is_none() || key_id.is_none() || p8.is_none() {
        web_assisted(issuer, key_id, p8)?
    } else {
        (issuer.unwrap(), key_id.unwrap(), p8.unwrap())
    };

    let p8_abs = std::fs::canonicalize(&p8)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(p8);
    let mut cfg = IosConfig::load()?;
    cfg.asc = Some(AscCredentials { issuer_id: issuer, key_id, p8_path: p8_abs });
    if let Some(t) = team_id {
        cfg.team_id = Some(t);
    }
    let client = AppStoreClient::new(cfg.asc.as_ref().unwrap())?;
    print!("🔑 Verifying credentials… ");
    use std::io::Write;
    std::io::stdout().flush().ok();
    match client.verify().await {
        Ok(crate::ios::appstore::Access::Ok(n)) => {
            println!("ok ({n} apps visible)");
            cfg.save()?;
            println!("✅ Saved to {}", IosConfig::config_path()?.display());
        }
        Ok(crate::ios::appstore::Access::Forbidden(detail)) => {
            // The key itself is valid (authenticated) – save it – but the
            // account has a business-level block we should explain.
            cfg.save()?;
            println!("key valid, but access is blocked");
            println!("✅ Saved to {}", IosConfig::config_path()?.display());
            println!("\n⚠️  Apple returned 403: {detail}");
            if detail.to_lowercase().contains("agreement") {
                println!("\nThis is NOT a credential problem – a legal agreement needs signing.");
                println!("The Account Holder must accept the pending agreement:");
                println!("  • Apple Developer Program License Agreement:");
                println!("      https://developer.apple.com/account  → review/accept the banner");
                println!("  • Paid Applications Agreement (needed for App Store / sales APIs):");
                println!("      App Store Connect → Business → Agreements");
                println!("\nCertificate/profile/device commands may still work now; try:");
                println!("  hatch ios certs");
            } else {
                println!("\nGive the API key more access (App Manager or Admin) in");
                println!("App Store Connect → Users and Access → Integrations.");
            }
        }
        Err(e) => return Err(e.context("App Store Connect rejected the credentials")),
    }
    Ok(())
}

/// Browser-assisted credential capture: open Apple's API Keys page, then
/// auto-detect the downloaded `AuthKey_<KEYID>.p8` and ask for the issuer ID.
fn web_assisted(
    issuer: Option<String>,
    key_id: Option<String>,
    p8: Option<String>,
) -> Result<(String, String, String)> {
    println!("🌐 Apple has no OAuth login for App Store Connect, so this is the");
    println!("   next best thing: a one-time API key (it never expires, no 2FA).\n");
    println!("Opening the API Keys page in your browser:\n  {ASC_KEYS_URL}");
    open_browser(ASC_KEYS_URL);
    println!(
        "\nIn the browser:\n  \
         1. Click the + (Generate API Key), give it 'App Manager' access.\n  \
         2. Download the AuthKey_XXXXXXXXXX.p8 (you can only download it once).\n  \
         3. Copy the Issuer ID shown above the keys table.\n"
    );

    // p8: explicit flag wins; else auto-detect the newest AuthKey_*.p8 in Downloads.
    let (p8_path, detected_key_id) = match p8 {
        Some(p) => (p, None),
        None => {
            prompt("Press Enter once the .p8 has finished downloading…")?;
            match newest_auth_key() {
                Some((path, kid)) => {
                    println!("📄 Found {}", path);
                    (path, Some(kid))
                }
                None => {
                    let p = prompt("Couldn't find it in Downloads. Paste the full path to the .p8:")?;
                    (p, None)
                }
            }
        }
    };

    let key_id = match key_id.or(detected_key_id) {
        Some(k) => k,
        None => prompt("Key ID (10 chars):")?,
    };
    let issuer = match issuer {
        Some(i) => i,
        None => prompt("Issuer ID (UUID from the top of the page):")?,
    };
    Ok((issuer, key_id, p8_path))
}

/// Find the most recently modified `AuthKey_<KEYID>.p8` in the Downloads folder.
fn newest_auth_key() -> Option<(String, String)> {
    let dl = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))?;
    let re = regex::Regex::new(r"(?i)^AuthKey_([A-Z0-9]+)\.p8$").ok()?;
    let mut best: Option<(std::time::SystemTime, String, String)> = None;
    for entry in std::fs::read_dir(&dl).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(c) = re.captures(&name) {
            let kid = c.get(1).unwrap().as_str().to_string();
            let mtime = entry.metadata().and_then(|m| m.modified()).ok()?;
            if best.as_ref().map(|(t, _, _)| mtime > *t).unwrap_or(true) {
                best = Some((mtime, entry.path().to_string_lossy().into_owned(), kid));
            }
        }
    }
    best.map(|(_, path, kid)| (path, kid))
}

/// Open a URL in the user's default browser (best effort, cross-platform).
fn open_browser(url: &str) {
    let r = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if r.is_err() {
        println!("(could not open a browser automatically; visit the URL above)");
    }
}

/// Print a prompt and read a trimmed line from stdin.
fn prompt(msg: &str) -> Result<String> {
    use std::io::{self, Write};
    print!("{msg} ");
    io::stdout().flush().ok();
    let mut s = String::new();
    io::stdin().read_line(&mut s).context("reading input")?;
    Ok(s.trim().to_string())
}

// --- build ----------------------------------------------------------------

async fn build(bundle_id: Option<String>, name: Option<String>, sign: bool, _distribution: bool) -> Result<()> {
    let cfg = IosConfig::load()?;
    let runner = runner_for(&cfg);
    let tc = Toolchain::new(&runner, &cfg.toolchain_root);
    let project = std::env::current_dir()?;

    // 1. dependencies (generates pubspec + .dart_tool/package_config.json)
    println!("📦 Ensuring dependencies are installed…");
    ensure_dependencies(&project).await?;

    // 1b. asset bundle (real flutter_assets via fvm flutter / flutter)
    generate_assets(&project);

    // 2. metadata
    let app_name = name
        .or_else(|| read_pubspec_name(&project))
        .unwrap_or_else(|| "App".to_string());
    let bundle = bundle_id
        .or_else(|| read_ios_bundle_id(&project))
        .or_else(|| cfg.bundle_id.clone())
        .unwrap_or_else(|| format!("com.example.{}", sanitize(&app_name)));

    // 3. engine artifacts for the project's Flutter version
    let version = resolve_flutter_version(&project)
        .context("could not determine Flutter version (no .fvmrc/.fvm config)")?;
    let hash = resolve_engine_hash(&version)?;
    println!("🧩 Flutter {version} (engine {})", short(&hash));
    tc.ensure_engine(&hash)?;

    // 4. build
    let req = BuildRequest {
        project_dir: to_build_path(&project.to_string_lossy()),
        app_name: app_name.clone(),
        bundle_id: bundle.clone(),
        min_os: "13.0".to_string(),
    };
    println!("🔨 Building iOS app '{app_name}' ({bundle})…");
    let out = builder::build(&runner, &tc, &req)?;

    // 5. optional signing
    let final_build_path = if sign {
        let mat = &cfg.signing;
        let p12 = mat.p12_path.as_deref().context(
            "no signing certificate configured. Run `hatch ios cert-create` then `hatch ios profile-create`.",
        )?;
        let profile = mat.profile_path.as_deref().context(
            "no provisioning profile configured. Run `hatch ios profile-create`.",
        )?;
        let pw = mat.p12_password.as_deref().unwrap_or("");
        let signed = format!("{}/out/{}-signed.ipa", tc.root, sanitize(&app_name));
        println!("✍️  Signing with zsign…");
        signing::sign_ipa(&runner, &tc.root, &out.ipa_path, p12, pw, profile, &signed)?;
        signed
    } else {
        out.ipa_path.clone()
    };

    // 6. copy artifact back to the project (host-visible)
    let host_out = project.join("build").join("ios").join("hatch");
    std::fs::create_dir_all(&host_out).ok();
    let dest = host_out.join(format!(
        "{}{}.ipa",
        sanitize(&app_name),
        if sign { "-signed" } else { "" }
    ));
    let dest_build = to_build_path(&dest.to_string_lossy());
    runner
        .exec(&format!("cp \"{}\" \"{}\"", final_build_path, dest_build))?
        .require()?;

    println!("\n✅ {} IPA: {}", if sign { "Signed" } else { "Unsigned" }, dest.display());
    if !sign {
        println!("   Sign later with: hatch ios build --sign  (after cert-create + profile-create)");
    }
    Ok(())
}

// --- publish (TestFlight upload) ------------------------------------------

async fn publish(ipa: Option<String>) -> Result<()> {
    let cfg = IosConfig::load()?;
    let creds = cfg.require_asc()?.clone();
    let runner = runner_for(&cfg);

    // Pre-flight: the target app must already exist in App Store Connect. Apple
    // has no API to create an app record, and uploading to a non-existent app is
    // rejected late and cryptically, so check up front and guide the user.
    if let Some(bid) = std::env::current_dir().ok().and_then(|p| read_ios_bundle_id(&p)) {
        match AppStoreClient::new(&creds) {
            Ok(client) => match client.list_apps().await {
                Ok(apps) if !apps.iter().any(|a| a.bundle_id == bid) => anyhow::bail!(
                    "no app with bundle id `{bid}` exists in App Store Connect.\n\
                     Apple's API cannot create app records - create it once in the web UI:\n  \
                     App Store Connect -> Apps -> (+) -> New App -> pick bundle id `{bid}`,\n  \
                     set a name + SKU, then re-run `hatch ios publish`."
                ),
                Ok(_) => {}
                Err(e) => eprintln!("⚠️  could not verify the app exists ({e}); continuing"),
            },
            Err(e) => eprintln!("⚠️  could not verify the app exists ({e}); continuing"),
        }
    }

    // Resolve the .ipa: explicit flag, else newest in build/ios/hatch.
    let ipa_build = match ipa {
        Some(p) => to_build_path(&p),
        None => {
            let dir = std::env::current_dir()?.join("build").join("ios").join("hatch");
            let newest = std::fs::read_dir(&dir)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map(|e| e == "ipa").unwrap_or(false))
                .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok());
            let p = newest.context(
                "no .ipa found in build/ios/hatch. Build one first: hatch ios build --sign",
            )?;
            to_build_path(&p.to_string_lossy())
        }
    };
    println!("📤 Uploading {ipa_build} to TestFlight…");

    let itms = publisher::ensure_installed(&runner, &cfg.toolchain_root, cfg.itms_transporter.as_deref())?;
    publisher::upload(&runner, &cfg.toolchain_root, &itms, &ipa_build, &creds)?;
    println!("\n✅ Uploaded. The build will appear in App Store Connect → TestFlight after processing.");
    Ok(())
}

// --- App Store Connect resource commands ----------------------------------

fn client() -> Result<(IosConfig, AppStoreClient)> {
    let cfg = IosConfig::load()?;
    let creds = cfg.require_asc()?;
    let client = AppStoreClient::new(creds)?;
    Ok((cfg, client))
}

async fn apps() -> Result<()> {
    let (_, c) = client()?;
    let apps = c.list_apps().await?;
    if apps.is_empty() {
        println!("No apps found.");
        return Ok(());
    }
    println!("{:<34} {:<28} {}", "BUNDLE ID", "NAME", "SKU");
    for a in apps {
        println!("{:<34} {:<28} {}", a.bundle_id, a.name, a.sku);
    }
    Ok(())
}

async fn devices() -> Result<()> {
    let (_, c) = client()?;
    let devs = c.list_devices().await?;
    if devs.is_empty() {
        println!("No registered devices.");
        return Ok(());
    }
    println!("{:<24} {:<14} {}", "NAME", "CLASS", "UDID");
    for d in devs {
        println!("{:<24} {:<14} {}", d.name, d.class, d.udid);
    }
    Ok(())
}

async fn device_add(name: String, udid: String) -> Result<()> {
    let (_, c) = client()?;
    let d = c.register_device(&name, &udid).await?;
    println!("✅ Registered device '{}' ({})", d.name, d.id);
    Ok(())
}

async fn certs() -> Result<()> {
    let (_, c) = client()?;
    let list = c.list_certificates().await?;
    if list.is_empty() {
        println!("No certificates.");
        return Ok(());
    }
    println!("{:<40} {:<22} {}", "NAME", "TYPE", "EXPIRES");
    for cert in list {
        println!("{:<40} {:<22} {}", cert.name, cert.cert_type, cert.expiration);
    }
    Ok(())
}

async fn cert_create(distribution: bool, password: Option<String>) -> Result<()> {
    let (mut cfg, c) = client()?;
    let runner = runner_for(&cfg);
    let cert_type = if distribution { "IOS_DISTRIBUTION" } else { "IOS_DEVELOPMENT" };
    let pw = password.unwrap_or_else(|| "hatch".to_string());

    println!("🔐 Generating private key + CSR…");
    let (csr_pem, _key) = signing::generate_csr(&runner, &cfg.toolchain_root, "Hatch iOS Signing")?;
    println!("📡 Requesting {cert_type} certificate from App Store Connect…");
    let cert = c.create_certificate(&csr_pem, cert_type).await?;
    let content = cert
        .content
        .clone()
        .context("App Store Connect did not return certificate content")?;
    println!("🧩 Assembling .p12…");
    let p12 = signing::assemble_p12(&runner, &cfg.toolchain_root, &content, &pw)?;

    cfg.signing.p12_path = Some(p12.clone());
    cfg.signing.p12_password = Some(pw);
    cfg.signing.certificate_id = Some(cert.id.clone());
    cfg.save()?;
    println!("✅ Certificate '{}' ready.\n   p12: {}", cert.name, p12);
    println!("   Next: hatch ios profile-create --bundle-id <id> --name <profile>");
    Ok(())
}

async fn profiles() -> Result<()> {
    let (_, c) = client()?;
    let list = c.list_profiles().await?;
    if list.is_empty() {
        println!("No profiles.");
        return Ok(());
    }
    println!("{:<40} {:<22} {}", "NAME", "TYPE", "STATE");
    for p in list {
        println!("{:<40} {:<22} {}", p.name, p.profile_type, p.state);
    }
    Ok(())
}

async fn profile_create(
    name: String,
    bundle_id: String,
    distribution: bool,
    devices: Vec<String>,
) -> Result<()> {
    let (mut cfg, c) = client()?;
    let runner = runner_for(&cfg);

    let cert_id = cfg
        .signing
        .certificate_id
        .clone()
        .context("no certificate yet. Run `hatch ios cert-create` first.")?;

    // Resolve (or create) the bundle id resource.
    let bid = match c.find_bundle_id(&bundle_id).await? {
        Some(b) => b,
        None => {
            println!("➕ Registering bundle id {bundle_id}…");
            c.create_bundle_id(&bundle_id, &name).await?
        }
    };

    // Map device UDIDs -> resource ids for development profiles.
    let device_ids = if distribution || devices.is_empty() {
        Vec::new()
    } else {
        let all = c.list_devices().await?;
        devices
            .iter()
            .filter_map(|udid| all.iter().find(|d| &d.udid == udid).map(|d| d.id.clone()))
            .collect()
    };

    let ptype = if distribution { "IOS_APP_STORE" } else { "IOS_APP_DEVELOPMENT" };
    println!("📡 Creating {ptype} profile '{name}'…");
    let profile = c
        .create_profile(&name, ptype, &bid.id, &[cert_id], &device_ids)
        .await?;
    let content = profile
        .content
        .clone()
        .context("App Store Connect did not return profile content")?;
    let path = signing::install_profile(&runner, &cfg.toolchain_root, &content, &name)?;

    cfg.signing.profile_path = Some(path.clone());
    cfg.bundle_id = Some(bundle_id);
    cfg.save()?;
    println!("✅ Profile '{}' ready.\n   profile: {}", profile.name, path);
    println!("   Build a signed app with: hatch ios build --sign");
    Ok(())
}

// --- helpers --------------------------------------------------------------

fn describe_runner(r: &Runner) -> String {
    match &r.distro {
        Some(d) => format!("WSL: {d}"),
        None => "native Linux".to_string(),
    }
}

fn short(s: &str) -> String {
    s.chars().take(8).collect()
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

/// Ensure `.dart_tool/package_config.json` exists. Uses Hatch's resolver when
/// the project is Hatch-managed, otherwise falls back to `flutter pub get`.
async fn ensure_dependencies(project: &Path) -> Result<()> {
    let hatch_managed = project.join("hatch.json").exists() || project.join("hatch.yaml").exists();
    if hatch_managed {
        return crate::cli::commands::install_smart::execute(None).await;
    }
    if project.join(".dart_tool").join("package_config.json").exists() {
        return Ok(()); // already resolved
    }
    if !project.join("pubspec.yaml").exists() {
        return Err(anyhow!("not a Flutter project (no pubspec.yaml / hatch.json)"));
    }
    // Plain Flutter project: resolve with flutter pub get (via fvm when available).
    use crate::fvm::detector::FvmDetector;
    let use_fvm =
        FvmDetector::is_fvm_installed() && FvmDetector::has_project_fvm_config(&project.to_path_buf());
    let (cmd, args): (&str, Vec<&str>) = if use_fvm {
        ("fvm", vec!["flutter", "pub", "get"])
    } else {
        ("flutter", vec!["pub", "get"])
    };
    let status = std::process::Command::new(cmd)
        .args(&args)
        .current_dir(project)
        .status()
        .with_context(|| format!("running `{cmd} {}`", args.join(" ")))?;
    if !status.success() {
        return Err(anyhow!("`{cmd} pub get` failed"));
    }
    Ok(())
}

/// Build the Flutter asset bundle (`build/flutter_assets`) so the pipeline ships
/// real assets/fonts. Runs via `fvm flutter` when the project pins an FVM
/// version, else plain `flutter`. Non-fatal: the pipeline falls back to minimal
/// manifests if this can't run (e.g. Flutter not on PATH).
fn generate_assets(project: &Path) {
    use crate::fvm::detector::FvmDetector;
    let use_fvm =
        FvmDetector::is_fvm_installed() && FvmDetector::has_project_fvm_config(&project.to_path_buf());
    let (cmd, args): (&str, Vec<&str>) = if use_fvm {
        ("fvm", vec!["flutter", "build", "bundle"])
    } else {
        ("flutter", vec!["build", "bundle"])
    };
    println!(
        "🎨 Building Flutter asset bundle ({})…",
        if use_fvm { "fvm flutter" } else { "flutter" }
    );
    match std::process::Command::new(cmd)
        .args(&args)
        .current_dir(project)
        .status()
    {
        Ok(s) if s.success() => {}
        Ok(_) => eprintln!(
            "⚠️  `{cmd} build bundle` failed; continuing with minimal asset manifests"
        ),
        Err(e) => eprintln!(
            "⚠️  could not run `{cmd} build bundle` ({e}); continuing with minimal asset manifests"
        ),
    }
}

/// Read the app's real bundle identifier from the iOS Xcode project
/// (`ios/Runner.xcodeproj/project.pbxproj`). Picks the most common
/// `PRODUCT_BUNDLE_IDENTIFIER`, ignoring test/extension targets
/// (`.RunnerTests`) and unresolved `$(...)` variable references. This is the
/// project's own truth, so it beats the global config default.
fn read_ios_bundle_id(project: &Path) -> Option<String> {
    let pbx = project
        .join("ios")
        .join("Runner.xcodeproj")
        .join("project.pbxproj");
    let text = std::fs::read_to_string(&pbx).ok()?;
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("PRODUCT_BUNDLE_IDENTIFIER") else {
            continue;
        };
        let val = rest
            .trim_start_matches(|c: char| c == ' ' || c == '=')
            .trim()
            .trim_end_matches(';')
            .trim()
            .trim_matches('"')
            .to_string();
        if val.is_empty() || val.contains("$(") || val.ends_with(".RunnerTests") {
            continue;
        }
        *counts.entry(val).or_insert(0) += 1;
    }
    counts.into_iter().max_by_key(|(_, c)| *c).map(|(k, _)| k)
}

fn read_pubspec_name(project: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project.join("pubspec.yaml")).ok()?;
    let val: serde_yaml::Value = serde_yaml::from_str(&text).ok()?;
    val.get("name")?.as_str().map(|s| s.to_string())
}

/// Read the pinned Flutter version from fvm config in the project.
fn resolve_flutter_version(project: &Path) -> Option<String> {
    // Newer FVM: .fvmrc = { "flutter": "3.35.2" }
    if let Ok(text) = std::fs::read_to_string(project.join(".fvmrc")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(s) = v.get("flutter").and_then(|x| x.as_str()) {
                return Some(s.to_string());
            }
        }
    }
    // Older FVM: .fvm/fvm_config.json = { "flutterSdkVersion": "3.35.2" }
    if let Ok(text) = std::fs::read_to_string(project.join(".fvm").join("fvm_config.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(s) = v.get("flutterSdkVersion").and_then(|x| x.as_str()) {
                return Some(s.to_string());
            }
        }
    }
    // Fallback: .fvm/version (plain text)
    std::fs::read_to_string(project.join(".fvm").join("version"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Resolve the engine commit hash for a Flutter version by reading the FVM
/// install's `bin/internal/engine.version`.
fn resolve_engine_hash(version: &str) -> Result<String> {
    let home = dirs::home_dir().context("no home directory")?;
    let candidates = [
        home.join("fvm").join("versions").join(version),
        home.join(".fvm").join("versions").join(version),
        PathBuf::from("/opt/fvm/versions").join(version),
    ];
    for base in candidates {
        let f = base.join("bin").join("internal").join("engine.version");
        if let Ok(h) = std::fs::read_to_string(&f) {
            let h = h.trim().to_string();
            if !h.is_empty() {
                return Ok(h);
            }
        }
    }
    Err(anyhow!(
        "could not find engine.version for Flutter {version}. Is it installed via FVM? \
         Looked under ~/fvm/versions/{version}/bin/internal/engine.version"
    ))
}
