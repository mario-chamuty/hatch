//! Resolves and provisions the Linux iOS toolchain inside the build
//! environment, and reports its health (`hatch ios doctor`).

use anyhow::{Context, Result};

use super::runner::Runner;

const ENGINE_BASE: &str = "https://storage.googleapis.com/flutter_infra_release/flutter";

/// Logical locations of every tool, expressed as paths *inside* the build env.
pub struct Toolchain<'a> {
    pub runner: &'a Runner,
    pub root: String,
}

pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl<'a> Toolchain<'a> {
    pub fn new(runner: &'a Runner, root: &str) -> Self {
        Self {
            runner,
            root: root.to_string(),
        }
    }

    pub fn gen_snapshot(&self) -> String {
        format!("{}/engine/gs-linux/gen_snapshot", self.root)
    }
    pub fn dart_sdk(&self) -> String {
        format!("{}/engine/dart-sdk", self.root)
    }
    pub fn platform_dill(&self) -> String {
        format!("{}/engine/flutter_patched_sdk_product/platform_strong.dill", self.root)
    }
    pub fn flutter_framework(&self) -> String {
        format!(
            "{}/engine/ios-release/Flutter.xcframework/ios-arm64/Flutter.framework",
            self.root
        )
    }
    pub fn cctools_bin(&self) -> String {
        format!(
            "{}/cctools-port/usage_examples/ios_toolchain/target/bin",
            self.root
        )
    }
    pub fn zsign(&self) -> String {
        format!("{}/zsign/bin/zsign", self.root)
    }
    pub fn rcodesign(&self) -> String {
        format!("{}/rcodesign/rcodesign", self.root)
    }

    /// Resolve the iOS SDK path (first iPhoneOS*.sdk under iossdk/).
    pub fn ios_sdk(&self) -> Result<String> {
        let out = self
            .runner
            .exec(&format!(
                "ls -d {}/iossdk/iPhoneOS*.sdk 2>/dev/null | head -1 || true",
                self.root
            ))?
            .require()?;
        if out.is_empty() {
            anyhow::bail!("no iPhoneOS SDK found under {}/iossdk", self.root);
        }
        Ok(out)
    }

    /// Run all health checks.
    pub fn doctor(&self) -> Result<Vec<Check>> {
        let clang = format!("{}/arm-apple-darwin11-clang", self.cctools_bin());
        let ld = format!("{}/arm-apple-darwin11-ld", self.cctools_bin());
        let items = [
            ("gen_snapshot (Dart AOT)", self.gen_snapshot()),
            ("dart-sdk (frontend)", self.dart_sdk()),
            ("flutter platform dill", self.platform_dill()),
            ("Flutter.framework (arm64)", self.flutter_framework()),
            ("cross clang", clang),
            ("ld64 linker", ld),
            ("rcodesign (App Store signer)", self.rcodesign()),
        ];
        let mut checks = Vec::new();

        // Shell reachability first.
        match self.runner.probe() {
            Ok(u) => checks.push(Check {
                name: "build shell".into(),
                ok: true,
                detail: u.lines().next().unwrap_or("").to_string(),
            }),
            Err(e) => {
                checks.push(Check {
                    name: "build shell".into(),
                    ok: false,
                    detail: e.to_string(),
                });
                return Ok(checks); // nothing else is reachable
            }
        }

        for (name, path) in items {
            let exists = self
                .runner
                .exec_raw(&format!("test -e \"{path}\" && echo yes || echo no"))
                .map(|c| c.stdout.trim() == "yes")
                .unwrap_or(false);
            checks.push(Check {
                name: name.to_string(),
                ok: exists,
                detail: path,
            });
        }

        // iOS SDK (globbed) + openssl presence.
        match self.ios_sdk() {
            Ok(p) => checks.push(Check { name: "iOS SDK".into(), ok: true, detail: p }),
            Err(e) => checks.push(Check { name: "iOS SDK".into(), ok: false, detail: e.to_string() }),
        }
        let openssl = self
            .runner
            .exec_raw("command -v openssl || true")
            .map(|c| c.stdout.trim().to_string())
            .unwrap_or_default();
        checks.push(Check {
            name: "openssl".into(),
            ok: !openssl.is_empty(),
            detail: if openssl.is_empty() { "missing".into() } else { openssl },
        });

        Ok(checks)
    }

    /// Download the Flutter engine artifacts for `engine_hash` if not already
    /// present for that hash. Pulls the Linux gen_snapshot, the iOS
    /// Flutter.xcframework, the flutter platform dill and the Dart SDK.
    pub fn ensure_engine(&self, engine_hash: &str) -> Result<()> {
        let script = format!(
            r#"
H="{hash}"
ROOT="{root}"
ENG="$ROOT/engine"
BASE="{base}/$H"
mkdir -p "$ENG"
stamp="$ENG/.engine_hash"
if [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$H" ] \
   && [ -x "$ENG/gs-linux/gen_snapshot" ] \
   && [ -d "$ENG/ios-release/Flutter.xcframework" ]; then
  echo "engine $H already present"; exit 0
fi
cd "$ENG"
echo "fetching Linux gen_snapshot ..."
curl -fsSL -o gs.zip "$BASE/android-arm64-release/linux-x64.zip"
rm -rf gs-linux && mkdir gs-linux && (cd gs-linux && unzip -oq ../gs.zip)
chmod +x gs-linux/gen_snapshot
echo "fetching iOS Flutter.xcframework ..."
curl -fsSL -o ios-release.zip "$BASE/ios-release/artifacts.zip"
rm -rf ios-release && mkdir ios-release && (cd ios-release && unzip -oq ../ios-release.zip)
echo "fetching flutter platform dill ..."
curl -fsSL -o fps.zip "$BASE/flutter_patched_sdk_product.zip"
rm -rf flutter_patched_sdk_product && unzip -oq fps.zip
# Always refresh the Dart SDK alongside gen_snapshot: the two must come from the
# SAME engine hash or their kernel binary formats disagree (gen_snapshot rejects
# the frontend's .dill with "Invalid kernel binary format version"). A stale
# dart-sdk left over from a previous hash is exactly that mismatch.
echo "fetching Dart SDK (linux-x64) ..."
rm -rf dart-sdk
curl -fsSL -o dart-sdk.zip "$BASE/dart-sdk-linux-x64.zip"
unzip -oq dart-sdk.zip
echo "$H" > "$stamp"
echo "engine $H ready"
"#,
            hash = engine_hash,
            root = self.root,
            base = ENGINE_BASE,
        );
        let out = self.runner.exec(&script).context("downloading engine artifacts")?;
        if !out.ok() {
            anyhow::bail!("engine provisioning failed:\n{}", out.stderr.trim());
        }
        Ok(())
    }
}
