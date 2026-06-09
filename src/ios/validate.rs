//! Pre-flight validation of a signed `.ipa` *before* it is uploaded to App
//! Store Connect. Apple rejects bad uploads late, asynchronously, and with
//! cryptic ITMS codes (an upload round-trip + minutes of processing wasted), so
//! we re-derive the same checks Apple's ingestion runs and fail fast on the
//! host with an actionable message.
//!
//! The headline check is the executable-segment flags of the main binary's
//! CodeDirectory: a distribution binary must carry `CS_EXECSEG_MAIN_BINARY` and
//! must NOT carry `CS_EXECSEG_ALLOW_UNSIGNED` (a debug flag). zsign historically
//! stamped `ALLOW_UNSIGNED` onto every binary whose entitlements merely *mention*
//! `get-task-allow` (App Store entitlements always carry it as `<false/>`), which
//! is exactly what triggers ITMS-90034 "not signed using an Apple submission
//! certificate". See the `zsign-execseg-itms90034` notes.

use anyhow::{anyhow, Context, Result};
use std::io::Read;
use std::path::Path;

// Mach-O / code-signing magic numbers and flags.
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_CIGAM: u32 = 0xbeba_feca;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const LC_CODE_SIGNATURE: u32 = 0x1d;
const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
const CSMAGIC_CODEDIRECTORY: u32 = 0xfade_0c02;
const CD_SUPPORTS_EXEC_SEG: u32 = 0x0002_0400;
const CS_EXECSEG_MAIN_BINARY: u64 = 0x1;
const CS_EXECSEG_ALLOW_UNSIGNED: u64 = 0x10;

// --- little/big-endian readers (all bounds-checked) -----------------------

fn u32_le(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn u32_be(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}
fn u64_be(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8)
        .map(|s| u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

struct CdInfo {
    version: u32,
    exec_seg_flags: Option<u64>,
}

struct MachOSig {
    signed: bool,
    cds: Vec<CdInfo>,
}

/// Parse the code-signature SuperBlob at `base` and return each CodeDirectory's
/// version + exec-segment flags.
fn parse_superblob(d: &[u8], base: usize) -> Vec<CdInfo> {
    let mut out = Vec::new();
    if u32_be(d, base) != Some(CSMAGIC_EMBEDDED_SIGNATURE) {
        return out;
    }
    let count = u32_be(d, base + 8).unwrap_or(0);
    for i in 0..count as usize {
        let io = base + 12 + i * 8;
        let Some(rel) = u32_be(d, io + 4) else { break };
        let boff = base + rel as usize;
        if u32_be(d, boff) == Some(CSMAGIC_CODEDIRECTORY) {
            let version = u32_be(d, boff + 8).unwrap_or(0);
            let exec_seg_flags = if version >= CD_SUPPORTS_EXEC_SEG {
                u64_be(d, boff + 80)
            } else {
                None
            };
            out.push(CdInfo { version, exec_seg_flags });
        }
    }
    out
}

/// Parse a thin 64-bit Mach-O image: is it signed, and its CodeDirectories.
fn parse_thin(d: &[u8]) -> Option<MachOSig> {
    if u32_le(d, 0)? != MH_MAGIC_64 {
        return None;
    }
    let ncmds = u32_le(d, 16)?;
    let mut off = 32usize;
    let mut signed = false;
    let mut cds = Vec::new();
    for _ in 0..ncmds {
        let cmd = u32_le(d, off)?;
        let sz = u32_le(d, off + 4)? as usize;
        if sz == 0 {
            break;
        }
        if cmd == LC_CODE_SIGNATURE {
            signed = true;
            if let Some(dataoff) = u32_le(d, off + 8) {
                cds = parse_superblob(d, dataoff as usize);
            }
        }
        off += sz;
    }
    Some(MachOSig { signed, cds })
}

/// Parse a Mach-O (thin or fat); returns `None` if the bytes aren't Mach-O.
fn parse_macho(d: &[u8]) -> Option<MachOSig> {
    let head = u32_be(d, 0)?;
    if head == FAT_MAGIC || head == FAT_CIGAM {
        let nfat = u32_be(d, 4)?;
        for i in 0..nfat as usize {
            let ao = 8 + i * 20; // fat_arch: cputype,cpusubtype,offset,size,align
            let off = u32_be(d, ao + 8)? as usize;
            let size = u32_be(d, ao + 12)? as usize;
            if let Some(slice) = d.get(off..off + size) {
                if let Some(r) = parse_thin(slice) {
                    return Some(r);
                }
            }
        }
        return None;
    }
    parse_thin(d)
}

/// Pull the embedded XML plist out of a DER `.mobileprovision` (a PKCS#7 blob).
fn extract_provision_plist(der: &[u8]) -> Option<Vec<u8>> {
    let find = |needle: &[u8]| -> Option<usize> {
        der.windows(needle.len()).position(|w| w == needle)
    };
    let start = find(b"<?xml")?;
    let end = find(b"</plist>")? + b"</plist>".len();
    der.get(start..end).map(|s| s.to_vec())
}

/// Run all pre-flight checks against the signed `.ipa`. Prints a checklist;
/// returns `Err` (aborting the upload) if any fatal problem is found.
pub fn preflight(path: &Path) -> Result<()> {
    println!("🔍 Pre-flight validating {}…", path.display());
    let mut fatals: Vec<String> = Vec::new();
    let mut warns: Vec<String> = Vec::new();

    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("the .ipa is not a valid zip archive")?;

    // Locate the app bundle + its Info.plist + provisioning profile.
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
    let app_dir = info_name.trim_end_matches("/Info.plist").to_string();

    // --- Info.plist: bundle id, executable name, build-environment keys ----
    let info: plist::Value = {
        let mut f = zip.by_name(&info_name)?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        plist::from_bytes(&buf).context("parsing the app's Info.plist")?
    };
    let idict = info.as_dictionary().context("Info.plist is not a dictionary")?;
    let sget = |k: &str| idict.get(k).and_then(|v| v.as_string()).map(|s| s.to_string());
    let bundle_id = sget("CFBundleIdentifier").context("Info.plist missing CFBundleIdentifier")?;
    let exec_name = sget("CFBundleExecutable").context("Info.plist missing CFBundleExecutable")?;
    for k in ["CFBundleVersion", "CFBundleShortVersionString", "MinimumOSVersion"] {
        if idict.get(k).is_none() {
            fatals.push(format!("Info.plist is missing required key `{k}`"));
        }
    }
    if idict.get("CFBundleSupportedPlatforms").is_none() {
        warns.push("Info.plist has no CFBundleSupportedPlatforms".into());
    }

    // --- provisioning profile: must be an App Store distribution profile ---
    let prov_name = prov_name
        .context("the .ipa has no embedded.mobileprovision - sign it first (hatch ios build --sign)")?;
    let prov_der = {
        let mut f = zip.by_name(&prov_name)?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        buf
    };
    let mut app_identifier: Option<String> = None;
    match extract_provision_plist(&prov_der).and_then(|p| plist::from_bytes::<plist::Value>(&p).ok()) {
        Some(pv) => {
            let pd = pv.as_dictionary();
            if let Some(pd) = pd {
                if pd.get("ProvisionedDevices").is_some() {
                    fatals.push(
                        "embedded profile is an Ad Hoc / Development profile (it lists \
                         ProvisionedDevices). App Store uploads need a distribution profile \
                         with no devices - create one with `hatch ios profile-create --distribution`."
                            .into(),
                    );
                }
                if pd.get("ProvisionsAllDevices").and_then(|v| v.as_boolean()) == Some(true) {
                    fatals.push(
                        "embedded profile is an Enterprise (in-house) profile, which the App \
                         Store will not accept."
                            .into(),
                    );
                }
                if let Some(ent) = pd.get("Entitlements").and_then(|v| v.as_dictionary()) {
                    if ent.get("get-task-allow").and_then(|v| v.as_boolean()) == Some(true) {
                        fatals.push(
                            "profile entitlements set get-task-allow=true (a development build). \
                             App Store distribution requires get-task-allow=false."
                                .into(),
                        );
                    }
                    app_identifier = ent
                        .get("application-identifier")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string());
                }
            }
        }
        None => warns.push("could not decode embedded.mobileprovision for inspection".into()),
    }

    // bundle id must match the profile's application-identifier (TEAMID.<id>),
    // honouring a trailing wildcard.
    if let Some(appid) = &app_identifier {
        if let Some((_team, pat)) = appid.split_once('.') {
            let matches = if let Some(prefix) = pat.strip_suffix('*') {
                bundle_id.starts_with(prefix)
            } else {
                pat == bundle_id
            };
            if !matches {
                fatals.push(format!(
                    "bundle id `{bundle_id}` does not match the provisioning profile's \
                     app id `{pat}`. Re-sign with a profile for `{bundle_id}`."
                ));
            }
        }
    }

    // --- signatures: every Mach-O signed; main binary's exec-seg flags sane --
    let main_exec_path = format!("{app_dir}/{exec_name}");
    let names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    let mut checked_main = false;
    for name in &names {
        if name.ends_with('/') {
            continue;
        }
        let mut data = Vec::new();
        {
            let mut f = match zip.by_name(name) {
                Ok(f) => f,
                Err(_) => continue,
            };
            if f.read_to_end(&mut data).is_err() {
                continue;
            }
        }
        let Some(sig) = parse_macho(&data) else { continue };
        let is_main = name == &main_exec_path;
        if !sig.signed {
            fatals.push(format!("Mach-O `{name}` is not code-signed"));
            continue;
        }
        if is_main {
            checked_main = true;
            if sig.cds.is_empty() {
                fatals.push(format!("main executable `{name}` has no CodeDirectory"));
            }
            // Aggregate the exec-seg flags across the (SHA1 + SHA256) directories
            // so we report each problem once rather than per-CodeDirectory.
            let with_flags: Vec<u64> = sig.cds.iter().filter_map(|c| c.exec_seg_flags).collect();
            if with_flags.is_empty() && !sig.cds.is_empty() {
                let v = sig.cds[0].version;
                warns.push(format!(
                    "main executable CodeDirectory is an old version (0x{v:x}) without \
                     executable-segment flags"
                ));
            }
            if with_flags.iter().any(|f| f & CS_EXECSEG_ALLOW_UNSIGNED != 0) {
                fatals.push(
                    "main executable carries CS_EXECSEG_ALLOW_UNSIGNED (a debug flag); Apple \
                     will reject this with ITMS-90034. Your zsign is the buggy build that stamps \
                     ALLOW_UNSIGNED on every binary - rebuild zsign with the get-task-allow value \
                     fix and re-sign."
                        .into(),
                );
            }
            if !with_flags.is_empty() && with_flags.iter().any(|f| f & CS_EXECSEG_MAIN_BINARY == 0) {
                fatals.push(
                    "main executable is missing CS_EXECSEG_MAIN_BINARY on its executable \
                     segment; Apple requires this on the main binary."
                        .into(),
                );
            }
        }
    }
    if !checked_main {
        warns.push(format!(
            "could not locate/parse the main executable `{main_exec_path}`"
        ));
    }

    // --- report -----------------------------------------------------------
    for w in &warns {
        println!("   ⚠️  {w}");
    }
    if fatals.is_empty() {
        println!("   ✅ signature, profile and bundle metadata look valid for App Store upload.");
        Ok(())
    } else {
        for f in &fatals {
            println!("   ❌ {f}");
        }
        Err(anyhow!(
            "pre-flight validation failed with {} blocking problem(s); not uploading.",
            fatals.len()
        ))
    }
}
