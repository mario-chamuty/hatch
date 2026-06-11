//! Native ports of the inline `python3` JSON/plist fixups from `pipeline.sh`.
//!
//!  * [`rewrite_package_config`] - translate Windows `file:///C:/...` package
//!    URIs to `/mnt/c/...` so a Linux frontend can resolve packages that were
//!    `pub get`-ed on a Windows host. A no-op on natively-resolved configs.
//!  * [`patch_flutter_framework_plist`] - set `MinimumOSVersion` to match the
//!    vtool-raised binary minos (ITMS-90208) and repair the `b'...'` bytes-repr
//!    `ClangVersion` wart baked into the prepared engine artifact.

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::Path;

static WIN_URI: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^file:///([A-Za-z]):/(.*)$").unwrap());

/// Rewrite `file:///C:/foo` -> `file:///mnt/c/foo` in every package `rootUri`.
/// Returns pretty-printed JSON (2-space indent, matching the old python output).
pub fn rewrite_package_config(src: &str) -> Result<String> {
    let mut doc: serde_json::Value =
        serde_json::from_str(src).context("parsing package_config.json")?;
    if let Some(pkgs) = doc.get_mut("packages").and_then(|p| p.as_array_mut()) {
        for pkg in pkgs {
            let Some(ru) = pkg.get("rootUri").and_then(|u| u.as_str()) else {
                continue;
            };
            if !ru.starts_with("file:") {
                continue;
            }
            if let Some(c) = WIN_URI.captures(ru) {
                let drive = c[1].to_ascii_lowercase();
                let rest = &c[2];
                let fixed = format!("file:///mnt/{drive}/{rest}");
                pkg["rootUri"] = serde_json::Value::String(fixed);
            }
        }
    }
    serde_json::to_string_pretty(&doc).context("serializing package_config.json")
}

/// Patch a Flutter.framework `Info.plist` in place: force `MinimumOSVersion`
/// and repair a `b'...'`-wrapped `ClangVersion`. Reads xml or binary, writes
/// binary (matching the old `plistlib.FMT_BINARY` output).
pub fn patch_flutter_framework_plist(path: &Path, minos: &str) -> Result<()> {
    let mut value = plist::Value::from_file(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let dict = value
        .as_dictionary_mut()
        .context("Flutter.framework Info.plist is not a dict")?;

    dict.insert(
        "MinimumOSVersion".into(),
        plist::Value::String(minos.to_string()),
    );

    if let Some(cv) = dict.get("ClangVersion").and_then(|v| v.as_string()) {
        if cv.starts_with("b'") && cv.ends_with('\'') && cv.len() >= 3 {
            let fixed = cv[2..cv.len() - 1].to_string();
            dict.insert("ClangVersion".into(), plist::Value::String(fixed));
        }
    }

    value
        .to_file_binary(path)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_windows_drive_uris() {
        let src = r#"{
  "configVersion": 2,
  "packages": [
    {"name": "a", "rootUri": "file:///C:/dev/a"},
    {"name": "b", "rootUri": "file:///D:/x/y/z"},
    {"name": "rel", "rootUri": "../foo"},
    {"name": "lin", "rootUri": "file:///home/u/p"}
  ]
}"#;
        let out = rewrite_package_config(src).unwrap();
        assert!(out.contains("file:///mnt/c/dev/a"), "{out}");
        assert!(out.contains("file:///mnt/d/x/y/z"), "{out}");
        // relative + already-unix URIs untouched
        assert!(out.contains("\"../foo\""), "{out}");
        assert!(out.contains("file:///home/u/p"), "{out}");
    }

    #[test]
    fn linux_config_is_unchanged_semantically() {
        let src = r#"{"packages":[{"name":"a","rootUri":"file:///home/u/a"}]}"#;
        let out = rewrite_package_config(src).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["packages"][0]["rootUri"], "file:///home/u/a");
    }

    #[test]
    fn plist_minos_and_clangversion_repair() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Info.plist");
        let mut dict = plist::Dictionary::new();
        dict.insert("MinimumOSVersion".into(), plist::Value::String("13.0".into()));
        dict.insert(
            "ClangVersion".into(),
            plist::Value::String("b'Apple clang 17.0'".into()),
        );
        dict.insert("CFBundleName".into(), plist::Value::String("Flutter".into()));
        plist::Value::Dictionary(dict).to_file_xml(&p).unwrap();

        patch_flutter_framework_plist(&p, "13.4").unwrap();

        let v = plist::Value::from_file(&p).unwrap();
        let d = v.as_dictionary().unwrap();
        assert_eq!(d["MinimumOSVersion"].as_string(), Some("13.4"));
        assert_eq!(d["ClangVersion"].as_string(), Some("Apple clang 17.0"));
        assert_eq!(d["CFBundleName"].as_string(), Some("Flutter"));
    }
}
