//! Code-signing material management. All crypto runs in the build environment
//! via openssl; the resulting `.p12` / `.mobileprovision` live next to the
//! toolchain where `zsign` consumes them. Hatch never needs host openssl.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};

use super::exec::Exec;
use super::runner::Runner;

/// Generate a fresh 2048-bit RSA private key and a CSR for it. Returns the CSR
/// in PEM form (to POST to App Store Connect) and the path to the private key
/// inside the build environment.
pub fn generate_csr(runner: &Runner, root: &str, common_name: &str) -> Result<(String, String)> {
    let dir = format!("{root}/signing");
    let key_path = format!("{dir}/key.pem");
    let script = format!(
        r#"
mkdir -p "{dir}"
openssl genrsa -out "{key_path}" 2048 >/dev/null 2>&1
openssl req -new -key "{key_path}" -subj "/CN={cn}/O=Hatch/C=US" 2>/dev/null
"#,
        dir = dir,
        key_path = key_path,
        cn = common_name,
    );
    let csr_pem = runner
        .exec(&script)
        .context("generating signing key + CSR")?
        .require()?;
    Ok((csr_pem, key_path))
}

/// Combine the previously generated private key with a certificate returned by
/// App Store Connect (base64 DER) into a password-protected `.p12`. Returns the
/// `.p12` path inside the build environment.
pub fn assemble_p12(
    runner: &Runner,
    root: &str,
    cert_der_b64: &str,
    p12_password: &str,
) -> Result<String> {
    let dir = format!("{root}/signing");
    let key_path = format!("{dir}/key.pem");
    let p12_path = format!("{dir}/cert.p12");
    // Strip whitespace/newlines Apple may include in the base64 blob.
    let clean: String = cert_der_b64.split_whitespace().collect();
    let script = format!(
        r#"
cd "{dir}"
printf '%s' "{b64}" | base64 -d > cert.der
openssl x509 -inform DER -in cert.der -out cert.pem
# -legacy keeps the .p12 readable by zsign's/older OpenSSL; harmless on 3.x.
openssl pkcs12 -export -legacy -inkey "{key_path}" -in cert.pem \
  -out "{p12_path}" -passout pass:"{pw}" 2>/dev/null \
  || openssl pkcs12 -export -inkey "{key_path}" -in cert.pem \
       -out "{p12_path}" -passout pass:"{pw}"
echo "{p12_path}"
"#,
        dir = dir,
        b64 = clean,
        key_path = key_path,
        p12_path = p12_path,
        pw = p12_password,
    );
    runner.exec(&script).context("assembling .p12")?.require()
}

/// Write a provisioning profile (base64 of the `.mobileprovision`) into the
/// build environment and return its path.
pub fn install_profile(runner: &Runner, root: &str, profile_b64: &str, name: &str) -> Result<String> {
    let dir = format!("{root}/signing");
    let safe: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
    let path = format!("{dir}/{safe}.mobileprovision");
    let clean: String = profile_b64.split_whitespace().collect();
    let script = format!(
        r#"
mkdir -p "{dir}"
printf '%s' "{b64}" | base64 -d > "{path}"
echo "{path}"
"#,
        dir = dir,
        b64 = clean,
        path = path,
    );
    runner.exec(&script).context("installing profile")?.require()
}

/// Sign an `.ipa` to `out` with `rcodesign` (the `apple-codesign` project) using
/// the given p12 + provisioning profile.
///
/// rcodesign faithfully reimplements Apple's `codesign` and produces signatures
/// the App Store / notarization accept Mac-free - unlike zsign, whose
/// sideloading-grade signatures Apple's ingestion rejects with ITMS-90034 even
/// when every structural check passes (see the `zsign-execseg-itms90034` notes).
///
/// Flow: unpack the IPA, embed the profile, derive the entitlements XML from the
/// profile, sign the `.app` bundle (rcodesign recursively signs nested
/// frameworks and auto-includes the Apple WWDR + Root CAs in the CMS chain),
/// then repackage. The signing certificate, team ID and exec-segment flags are
/// all set correctly by rcodesign from the leaf in the p12.
pub fn sign_ipa(
    runner: &Runner,
    root: &str,
    ipa_path: &str,
    p12_path: &str,
    p12_password: &str,
    profile_path: &str,
    out_path: &str,
) -> Result<()> {
    let rcodesign = format!("{root}/rcodesign/rcodesign");
    // Pass ONLY the p12 (leaf + key). rcodesign auto-registers the Apple CAs and
    // sets the team ID from the leaf; passing the chain via --pem-file makes it
    // mis-select the WWDR intermediate as the signing cert.
    let script = format!(
        r#"
set -e
WORK=$(mktemp -d)
cd "$WORK"
unzip -oq "{ipa}"
APP=$(ls -d Payload/*.app | head -1)
cp "{prof}" "$APP/embedded.mobileprovision"
openssl smime -inform DER -verify -noverify -in "$APP/embedded.mobileprovision" 2>/dev/null > prof.plist
python3 -c "import plistlib,sys;plistlib.dump(plistlib.load(open('prof.plist','rb'))['Entitlements'],open('ent.plist','wb'))"
"{rcodesign}" sign --p12-file "{p12}" --p12-password "{pw}" --entitlements-xml-file ent.plist "$APP"
rm -f "{out}"
( cd "$WORK" && zip -qX -r "{out}" Payload )
"#,
        rcodesign = rcodesign,
        p12 = p12_path,
        pw = p12_password,
        prof = profile_path,
        out = out_path,
        ipa = ipa_path,
    );
    runner.exec(&script).context("rcodesign signing")?.require()?;
    Ok(())
}

/// Convenience: decode a base64 blob (used by callers that want to persist
/// Apple-returned artifacts on the host too).
#[allow(dead_code)]
pub fn decode_b64(b64: &str) -> Result<Vec<u8>> {
    let clean: String = b64.split_whitespace().collect();
    STANDARD.decode(clean.as_bytes()).context("decoding base64 blob")
}

/// Extract the `Entitlements` dict from a `.mobileprovision` and write it as a
/// standalone XML plist. The profile is a CMS SignedData whose content is the
/// provisioning plist in cleartext, so we slice out the `<?xml ... </plist>`
/// span and parse it - no openssl/CMS needed.
pub fn entitlements_from_profile(profile_path: &std::path::Path, out_xml: &std::path::Path) -> Result<()> {
    let data = std::fs::read(profile_path)
        .with_context(|| format!("reading {}", profile_path.display()))?;
    let start = find_sub(&data, b"<?xml").context("no plist in provisioning profile")?;
    let end = find_sub(&data, b"</plist>").context("truncated plist in profile")? + b"</plist>".len();
    let value = plist::Value::from_reader_xml(std::io::Cursor::new(&data[start..end]))
        .context("parsing profile plist")?;
    let ent = value
        .as_dictionary()
        .and_then(|d| d.get("Entitlements"))
        .context("profile has no Entitlements")?;
    ent.to_file_xml(out_xml)
        .with_context(|| format!("writing {}", out_xml.display()))?;
    Ok(())
}

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Sign an unsigned `.ipa` to `out` entirely host-natively (no bash/openssl/
/// python): unpack via the zip crate, embed the profile, derive entitlements
/// from it, sign the `.app` with `rcodesign`, repack. Used on the Windows-native
/// path; `rcodesign` is the cross-platform `apple-codesign` binary.
pub fn sign_ipa_native(
    rcodesign: &str,
    ipa_path: &str,
    p12_path: &str,
    p12_password: &str,
    profile_path: &str,
    out_path: &str,
) -> Result<()> {
    use std::path::Path;
    let work = std::env::temp_dir().join(format!("hatch-sign-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;

    // Unzip the IPA.
    let f = std::fs::File::open(ipa_path).with_context(|| format!("opening {ipa_path}"))?;
    let mut zip = zip::ZipArchive::new(f).context("reading IPA zip")?;
    for i in 0..zip.len() {
        let mut e = zip.by_index(i)?;
        let Some(rel) = e.enclosed_name() else { continue };
        let dst = work.join(rel);
        if e.is_dir() {
            std::fs::create_dir_all(&dst)?;
        } else {
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut o = std::fs::File::create(&dst)?;
            std::io::copy(&mut e, &mut o)?;
        }
    }

    // Locate Payload/<App>.app.
    let payload = work.join("Payload");
    let app = std::fs::read_dir(&payload)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().map(|x| x == "app").unwrap_or(false))
        .context("no .app in Payload")?;

    // Embed the profile + derive entitlements. rcodesign's --entitlements-xml-file
    // uses `[scope:]path` syntax, so an absolute Windows path (C:\...) is misread
    // as scope "C". Write ent.plist into the work dir and pass it RELATIVE, with
    // the work dir as the cwd, so the path carries no drive colon.
    std::fs::copy(profile_path, app.join("embedded.mobileprovision"))?;
    entitlements_from_profile(Path::new(profile_path), &work.join("ent.plist"))?;

    // Sign the bundle (rcodesign recurses into nested frameworks).
    let out = Exec::run_full(
        rcodesign,
        [
            "sign",
            "--p12-file", p12_path,
            "--p12-password", p12_password,
            "--entitlements-xml-file", "ent.plist",
            &app.to_string_lossy(),
        ],
        Some(&work),
        &[],
    )?;
    if !out.ok() {
        anyhow::bail!("rcodesign failed (exit {}):\n{}", out.status, out.stderr.trim());
    }

    // Repackage the IPA (Mach-O files get the exec bit).
    let _ = std::fs::remove_file(out_path);
    zip_payload(&work, Path::new(out_path))?;
    Ok(())
}

/// Zip `<work>/Payload` into `out` with entries named `Payload/...`, marking
/// Mach-O files executable.
fn zip_payload(work: &std::path::Path, out: &std::path::Path) -> Result<()> {
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(out)?;
    let mut zw = zip::ZipWriter::new(file);
    let base = work;
    let mut stack = vec![work.join("Payload")];
    while let Some(dir) = stack.pop() {
        let rel = format!("{}/", rel_slash(base, &dir));
        zw.add_directory(rel, SimpleFileOptions::default())?;
        let mut entries: Vec<_> = std::fs::read_dir(&dir)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if e.file_type()?.is_dir() {
                stack.push(p);
            } else {
                let data = std::fs::read(&p)?;
                let exec = data.len() >= 4 && data[..4] == [0xCF, 0xFA, 0xED, 0xFE];
                let opts = SimpleFileOptions::default()
                    .unix_permissions(if exec { 0o755 } else { 0o644 });
                zw.start_file(rel_slash(base, &p), opts)?;
                use std::io::Write;
                zw.write_all(&data)?;
            }
        }
    }
    zw.finish()?;
    Ok(())
}

fn rel_slash(base: &std::path::Path, p: &std::path::Path) -> String {
    p.strip_prefix(base).unwrap_or(p).to_string_lossy().replace('\\', "/")
}
