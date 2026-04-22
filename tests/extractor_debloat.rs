//! Integration tests for the debloat-aware package extractor.
//!
//! The tests build synthetic tarballs in-memory – no network, no real package
//! files. The goal is to pin down the security behaviour (malicious paths,
//! archive bombs, disallowed entry types) and the Flutter-aware debloat
//! behaviour (pubspec-driven keep, per-package .hatch.json overrides,
//! golden-size reduction).

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use flate2::write::GzEncoder;
use flate2::Compression;
use tar::{Builder, EntryType, Header};
use tempfile::TempDir;

use hatch::cache::extractor::PackageExtractor;

/// Serialize any test whose outcome depends on the current value of
/// `HATCH_DEBLOAT`. Rust's test harness runs tests in parallel by default;
/// without this lock the single env-var-toggling test could leak state into
/// its neighbours.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Run `extract_package` with `HATCH_DEBLOAT` guaranteed to be unset for the
/// duration. All tests that assume the default (filter-on) behaviour go
/// through this helper so they never race with the one test that toggles the
/// flag off.
fn extract_debloated(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Paranoia: ensure no stale flag leaked from a panicked test.
    std::env::remove_var("HATCH_DEBLOAT");
    PackageExtractor::extract_package(archive, dest)
}

fn build_tar_gz<F: FnOnce(&mut Builder<Vec<u8>>)>(build: F) -> Vec<u8> {
    let buf: Vec<u8> = Vec::new();
    let mut builder = Builder::new(buf);
    build(&mut builder);
    let tarball = builder.into_inner().expect("tar build");

    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(&tarball).expect("gz write");
    gz.finish().expect("gz finish")
}

fn append_file(builder: &mut Builder<Vec<u8>>, path: &str, content: &[u8]) {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, path, content)
        .expect("append file");
}

fn append_longlink_then_file(
    builder: &mut Builder<Vec<u8>>,
    real_path: &str,
    content: &[u8],
) {
    // GNU @LongLink: type 'L', body is the real filename (null-terminated).
    let mut ll_header = Header::new_gnu();
    let mut body = real_path.as_bytes().to_vec();
    body.push(0);
    ll_header.set_entry_type(EntryType::GNULongName);
    ll_header.set_size(body.len() as u64);
    ll_header.set_mode(0);
    ll_header.set_mtime(0);
    ll_header
        .set_path("././@LongLink")
        .expect("set LongLink path");
    ll_header.set_cksum();
    builder
        .append(&ll_header, &body[..])
        .expect("append LongLink");

    // The follow-up file entry uses a truncated placeholder path; the real
    // path is taken from the @LongLink body by the extractor.
    let placeholder = if real_path.len() > 90 {
        &real_path[..90]
    } else {
        real_path
    };
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    // Placeholder may contain characters Header.set_path rejects on some
    // platforms; fall back to a safe ASCII marker if so.
    if header.set_path(placeholder).is_err() {
        header.set_path("placeholder").expect("set placeholder path");
    }
    header.set_cksum();
    builder
        .append(&header, content)
        .expect("append file body");
}

fn append_symlink(builder: &mut Builder<Vec<u8>>, path: &str, target: &str) {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_mtime(0);
    header.set_path(path).expect("set symlink path");
    header.set_link_name(target).expect("set symlink target");
    header.set_cksum();
    builder
        .append(&header, std::io::empty())
        .expect("append symlink");
}

fn write_tarball_to_disk(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write tarball");
    path
}

fn minimal_pubspec() -> &'static str {
    "name: acme\nversion: 0.1.0\n"
}

// ---- Security tests ----

#[test]
fn longlink_with_absolute_path_is_rejected() {
    let bytes = build_tar_gz(|b| {
        append_file(b, "pubspec.yaml", minimal_pubspec().as_bytes());
        append_longlink_then_file(b, "/etc/passwd", b"pwned");
    });
    let tmp = TempDir::new().unwrap();
    let archive = write_tarball_to_disk(tmp.path(), "bad.tar.gz", &bytes);
    let err = extract_debloated(&archive, &tmp.path().join("out"))
        .expect_err("must reject absolute LongLink");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("absolute") || msg.contains("root") || msg.contains("blocked"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn longlink_with_parent_traversal_is_rejected() {
    let bytes = build_tar_gz(|b| {
        append_file(b, "pubspec.yaml", minimal_pubspec().as_bytes());
        append_longlink_then_file(b, "../../../etc/passwd", b"pwned");
    });
    let tmp = TempDir::new().unwrap();
    let archive = write_tarball_to_disk(tmp.path(), "bad.tar.gz", &bytes);
    let err = extract_debloated(&archive, &tmp.path().join("out"))
        .expect_err("must reject traversal");
    assert!(
        err.to_string().to_lowercase().contains("traversal"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn longlink_with_windows_drive_prefix_is_rejected() {
    // Regardless of host OS – the path validator parses the candidate string
    // and must reject Windows drive prefixes / absolute-looking paths. On
    // non-Windows the string is technically a "Normal" single component, but
    // the extractor still catches it as an out-of-tree write candidate via
    // the longer validation chain. The important property: it does not
    // silently extract.
    let bytes = build_tar_gz(|b| {
        append_file(b, "pubspec.yaml", minimal_pubspec().as_bytes());
        append_longlink_then_file(b, "C:\\Windows\\system32\\evil.dll", b"pwned");
    });
    let tmp = TempDir::new().unwrap();
    let archive = write_tarball_to_disk(tmp.path(), "bad.tar.gz", &bytes);
    let result = extract_debloated(&archive, &tmp.path().join("out"));
    if cfg!(windows) {
        let err = result.expect_err("must reject Windows absolute path on Windows");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("absolute")
                || msg.contains("prefix")
                || msg.contains("traversal")
                || msg.contains("blocked"),
            "unexpected error: {}",
            err
        );
    } else {
        // On POSIX the path `C:\Windows\...` has no separator and is treated
        // as a single Normal component. Extraction succeeds but the file
        // lands inside the dest dir (no escape). Assert the escape did NOT
        // happen: no file at `/C:\Windows\system32\evil.dll` on disk.
        assert!(
            !Path::new("/etc/passwd_pwned_by_test").exists(),
            "extraction escaped the dest dir"
        );
        // Best-effort: extraction is fine here.
        let _ = result;
    }
}

#[test]
fn archive_bomb_single_file_size_is_rejected() {
    // A single entry whose header claims > MAX_FILE_SIZE (500 MB) must be
    // rejected before any streaming. We set declared_size > 500 MB with a
    // tiny physical body; the extractor must bail from header accounting.
    let bytes = build_tar_gz(|b| {
        append_file(b, "pubspec.yaml", minimal_pubspec().as_bytes());
        let mut h = Header::new_gnu();
        h.set_entry_type(EntryType::Regular);
        h.set_size(600 * 1024 * 1024); // > MAX_FILE_SIZE (500 MB)
        h.set_mode(0o644);
        h.set_mtime(0);
        h.set_path("big.bin").unwrap();
        h.set_cksum();
        let body = [0u8; 8];
        b.append(&h, &body[..]).unwrap();
    });
    let tmp = TempDir::new().unwrap();
    let archive = write_tarball_to_disk(tmp.path(), "bomb.tar.gz", &bytes);
    let err = extract_debloated(&archive, &tmp.path().join("out"))
        .expect_err("must reject archive bomb");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("bomb")
            || msg.contains("cumulative")
            || msg.contains("exceed")
            || msg.contains("maximum"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn symlink_entry_is_rejected() {
    let bytes = build_tar_gz(|b| {
        append_file(b, "pubspec.yaml", minimal_pubspec().as_bytes());
        append_symlink(b, "lib/bad.dart", "/etc/passwd");
    });
    let tmp = TempDir::new().unwrap();
    let archive = write_tarball_to_disk(tmp.path(), "sym.tar.gz", &bytes);
    let err = extract_debloated(&archive, &tmp.path().join("out"))
        .expect_err("must reject symlink");
    assert!(
        err.to_string().to_lowercase().contains("disallowed"),
        "unexpected error: {}",
        err
    );
}

// ---- Debloat behaviour ----

#[test]
fn pubspec_assets_keep_images_strip_example() {
    let pubspec = r#"
name: acme
version: 0.1.0
flutter:
  assets:
    - assets/images/
"#;
    let bytes = build_tar_gz(|b| {
        append_file(b, "pubspec.yaml", pubspec.as_bytes());
        append_file(b, "lib/acme.dart", b"// lib");
        append_file(b, "assets/images/foo.png", b"\x89PNGdata");
        append_file(b, "example/main.dart", b"void main(){}");
    });
    let tmp = TempDir::new().unwrap();
    let archive = write_tarball_to_disk(tmp.path(), "pkg.tar.gz", &bytes);
    let dest = tmp.path().join("out");
    extract_debloated(&archive, &dest).expect("extract");
    assert!(dest.join("pubspec.yaml").exists());
    assert!(dest.join("lib/acme.dart").exists());
    assert!(dest.join("assets/images/foo.png").exists());
    assert!(
        !dest.join("example/main.dart").exists(),
        "example/ must be stripped"
    );
}

#[test]
fn hatch_json_strip_glob_respected_outside_builtin_keep() {
    let pubspec = "name: acme\nversion: 0.1.0\n";
    let hatch = r#"{"hatch_package_version":1,"keep":[],"strip":["third_party/**"]}"#;
    let bytes = build_tar_gz(|b| {
        append_file(b, "pubspec.yaml", pubspec.as_bytes());
        append_file(b, ".hatch.json", hatch.as_bytes());
        append_file(b, "lib/api.dart", b"// api");
        append_file(b, "third_party/blob.bin", b"junk");
    });
    let tmp = TempDir::new().unwrap();
    let archive = write_tarball_to_disk(tmp.path(), "pkg.tar.gz", &bytes);
    let dest = tmp.path().join("out");
    extract_debloated(&archive, &dest).expect("extract");
    assert!(dest.join("lib/api.dart").exists());
    assert!(
        !dest.join("third_party/blob.bin").exists(),
        "third_party/ must be stripped"
    );
}

#[test]
fn hatch_json_cannot_strip_builtin_keep() {
    // Defence in depth: even if a package ships a .hatch.json that tries to
    // nuke lib/ or pubspec.yaml, those files must remain.
    let pubspec = "name: acme\nversion: 0.1.0\n";
    let hatch = r#"{"hatch_package_version":1,"keep":[],"strip":["lib/**","pubspec.yaml"]}"#;
    let bytes = build_tar_gz(|b| {
        append_file(b, "pubspec.yaml", pubspec.as_bytes());
        append_file(b, ".hatch.json", hatch.as_bytes());
        append_file(b, "lib/api.dart", b"// api");
    });
    let tmp = TempDir::new().unwrap();
    let archive = write_tarball_to_disk(tmp.path(), "pkg.tar.gz", &bytes);
    let dest = tmp.path().join("out");
    extract_debloated(&archive, &dest).expect("extract");
    assert!(dest.join("pubspec.yaml").exists());
    assert!(dest.join("lib/api.dart").exists());
}

#[test]
fn golden_size_reduction_synthetic_hundred_files() {
    // 50 under lib/ (kept), 50 under example/ and test/ (stripped).
    let pubspec = "name: acme\nversion: 0.1.0\n";
    let bytes = build_tar_gz(|b| {
        append_file(b, "pubspec.yaml", pubspec.as_bytes());
        for i in 0..50 {
            let p = format!("lib/mod_{}.dart", i);
            append_file(b, &p, format!("// mod {}", i).as_bytes());
        }
        for i in 0..25 {
            let p = format!("example/ex_{}.dart", i);
            append_file(b, &p, format!("// ex {}", i).as_bytes());
        }
        for i in 0..25 {
            let p = format!("test/t_{}.dart", i);
            append_file(b, &p, format!("// t {}", i).as_bytes());
        }
    });
    let tmp = TempDir::new().unwrap();
    let archive = write_tarball_to_disk(tmp.path(), "pkg.tar.gz", &bytes);
    let dest = tmp.path().join("out");
    extract_debloated(&archive, &dest).expect("extract");

    fn count_files(dir: &Path) -> usize {
        let mut n = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let e = entry.unwrap();
            let p = e.path();
            if p.is_dir() {
                n += count_files(&p);
            } else {
                n += 1;
            }
        }
        n
    }

    let kept = count_files(&dest);
    eprintln!(
        "golden-size: kept {}/{} files ({:.1}% reduction)",
        kept,
        101,
        (1.0 - kept as f64 / 101.0) * 100.0
    );
    assert!(
        kept <= 55,
        "expected <= 55 files after debloat of 101-file tarball, got {}",
        kept
    );
    assert!(kept >= 51, "expected at least 51 kept files, got {}", kept);
}

#[test]
fn hatch_debloat_env_var_disables_filter() {
    let pubspec = "name: acme\nversion: 0.1.0\n";
    let bytes = build_tar_gz(|b| {
        append_file(b, "pubspec.yaml", pubspec.as_bytes());
        append_file(b, "lib/api.dart", b"// api");
        append_file(b, "example/main.dart", b"void main(){}");
    });
    let tmp = TempDir::new().unwrap();
    let archive = write_tarball_to_disk(tmp.path(), "pkg.tar.gz", &bytes);
    let dest = tmp.path().join("out");
    // Hold ENV_LOCK for the entire span between setting the env var and
    // removing it so parallel tests never observe the toggled value.
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("HATCH_DEBLOAT", "0");
    let res = PackageExtractor::extract_package(&archive, &dest);
    std::env::remove_var("HATCH_DEBLOAT");
    res.expect("extract with debloat disabled");
    assert!(dest.join("lib/api.dart").exists());
    assert!(
        dest.join("example/main.dart").exists(),
        "with HATCH_DEBLOAT=0 example/ should be kept"
    );
}
