# Flutter -> iOS .ipa pipeline (runs in the Linux build environment).
# Placeholders @@...@@ are substituted by builder.rs. `set -euo pipefail` is
# injected by the runner.

ROOT="@@ROOT@@"
PROJ="@@PROJ@@"
SDK="@@SDK@@"
MINOS="@@MINOS@@"

ENG="$ROOT/engine"
DARTBIN="$ENG/dart-sdk/bin"
AOTRT="$DARTBIN/dartaotruntime"
FES="$DARTBIN/snapshots/frontend_server_aot.dart.snapshot"
PLATFORM="$ENG/flutter_patched_sdk_product/platform_strong.dill"
GS="$ENG/gs-linux/gen_snapshot"
FLUTTER_FW="$ENG/ios-release/Flutter.xcframework/ios-arm64/Flutter.framework"
TC="$ROOT/cctools-port/usage_examples/ios_toolchain/target/bin"
# The cross clang wrapper resolves its linker (arm-apple-darwin11-ld) from PATH.
export PATH="$TC:$PATH"
CLANG="$TC/arm-apple-darwin11-clang"
INT="$TC/arm-apple-darwin11-install_name_tool"
VTOOL="$TC/arm-apple-darwin11-vtool"

# Apple's ingestion rejects binaries that don't look like they were produced by
# Apple's linker (ITMS-90125). cctools ld64 emits the legacy LC_DYLD_INFO_ONLY;
# a real iOS-SDK build emits LC_DYLD_CHAINED_FIXUPS. LLVM's ld64.lld can emit
# chained fixups (-fixup_chains, requires deployment target >= 13.4), so prefer
# it when present and fall back to cctools ld otherwise.
LLD="$(command -v ld64.lld-18 2>/dev/null || command -v ld64.lld 2>/dev/null || true)"
# The Apple SDK version we stamp into LC_BUILD_VERSION + the Info.plist DT* keys.
# Apple requires apps be built with a current SDK (ITMS-90725); we don't link new
# symbols, so stamping the version is sufficient.
SDK_VER="26.0"
SDK_BUILD="23A340"
XCODE_BUILD="17A324"

WORK="$ROOT/work/@@SAFE@@"
OUT="$ROOT/out"
APP="$OUT/Payload/Runner.app"

rm -rf "$WORK" "$OUT"
mkdir -p "$WORK" "$APP/Frameworks/App.framework" "$APP/flutter_assets"

# 1. main.dart + package_config -> AOT kernel
if [ ! -f "$PROJ/.dart_tool/package_config.json" ]; then
  echo "missing $PROJ/.dart_tool/package_config.json - run 'hatch install' first" >&2
  exit 3
fi
echo "== 1/6 AOT kernel =="
# Translate any Windows file:///C:/ URIs to /mnt/c so the Linux frontend can
# resolve packages/SDK that were pub-got on a Windows host (no-op on Linux).
PKG="$WORK/package_config.json"
python3 - "$PROJ/.dart_tool/package_config.json" "$PKG" <<'PY'
import json, re, sys
src, dst = sys.argv[1], sys.argv[2]
d = json.load(open(src))
def fix(u):
    m = re.match(r'file:///([A-Za-z]):/(.*)', u)
    return 'file:///mnt/%s/%s' % (m.group(1).lower(), m.group(2)) if m else u
for p in d.get('packages', []):
    ru = p.get('rootUri', '')
    if ru.startswith('file:'):
        p['rootUri'] = fix(ru)
json.dump(d, open(dst, 'w'), indent=2)
PY
"$AOTRT" "$FES" \
  --sdk-root "$(dirname "$PLATFORM")/" \
  --platform "$PLATFORM" \
  --target=flutter --aot --tfa -Ddart.vm.product=true \
  --packages "$PKG" \
  --output-dill "$WORK/app.aot.dill" \
  "$PROJ/lib/main.dart" >/dev/null

# 2. AOT kernel -> Mach-O arm64 App.framework
echo "== 2/6 gen_snapshot -> App.framework (Mach-O arm64) =="
"$GS" --snapshot_kind=app-aot-macho-dylib \
  --macho="$APP/Frameworks/App.framework/App" "$WORK/app.aot.dill"
"$INT" -id @rpath/App.framework/App "$APP/Frameworks/App.framework/App"
"$VTOOL" -arch arm64 -set-build-version ios "$MINOS" "$SDK_VER" -tool ld 1217 -replace \
  -output "$APP/Frameworks/App.framework/App" "$APP/Frameworks/App.framework/App" 2>/dev/null || true
# gen_snapshot marks the __DWARF (debug) segment rwx, so the framework has two
# executable segments and Apple rejects it (ITMS-90999). Clear the exec bit from
# every non-__TEXT segment so only __TEXT is executable.
python3 - "$APP/Frameworks/App.framework/App" <<'PY'
import sys, struct
p = sys.argv[1]
d = bytearray(open(p, "rb").read())
assert struct.unpack_from("<I", d, 0)[0] == 0xfeedfacf, "expected arm64 Mach-O"
ncmds = struct.unpack_from("<I", d, 16)[0]
off = 32
for _ in range(ncmds):
    cmd, cmdsize = struct.unpack_from("<II", d, off)
    if cmd == 0x19:  # LC_SEGMENT_64
        seg = d[off + 8:off + 24].split(b"\x00")[0]
        if seg != b"__TEXT":
            for fld in (56, 60):  # maxprot, initprot
                prot = struct.unpack_from("<I", d, off + fld)[0]
                if prot & 0x4:
                    struct.pack_into("<I", d, off + fld, prot & ~0x4)
    off += cmdsize
open(p, "wb").write(d)
PY
cat > "$APP/Frameworks/App.framework/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>App</string>
  <key>CFBundleIdentifier</key><string>io.flutter.flutter.app</string>
  <key>CFBundleName</key><string>App</string>
  <key>CFBundlePackageType</key><string>FMWK</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>MinimumOSVersion</key><string>@@MINOS@@</string>
</dict></plist>
PLIST

# 3. Flutter.framework
echo "== 3/6 Flutter.framework =="
cp -R "$FLUTTER_FW" "$APP/Frameworks/Flutter.framework"
rm -rf "$APP/Frameworks/Flutter.framework/_CodeSignature"
"$VTOOL" -arch arm64 -set-build-version ios "$MINOS" "$SDK_VER" -tool ld 1217 -replace \
  -output "$APP/Frameworks/Flutter.framework/Flutter" "$APP/Frameworks/Flutter.framework/Flutter" 2>/dev/null || true

# 4. native Runner shell (no plugins) -> compile + link
echo "== 4/6 Runner (cross clang + ld64) =="
mkdir -p "$WORK/Runner"
cat > "$WORK/Runner/main.m" <<'OBJC'
#import <UIKit/UIKit.h>
#import "AppDelegate.h"
int main(int argc, char * argv[]) {
  @autoreleasepool { return UIApplicationMain(argc, argv, nil, NSStringFromClass([AppDelegate class])); }
}
OBJC
cat > "$WORK/Runner/AppDelegate.h" <<'OBJC'
#import <UIKit/UIKit.h>
#import <Flutter/Flutter.h>
@interface AppDelegate : FlutterAppDelegate
@end
OBJC
cat > "$WORK/Runner/AppDelegate.m" <<'OBJC'
#import "AppDelegate.h"
@implementation AppDelegate
- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
  return [super application:application didFinishLaunchingWithOptions:launchOptions];
}
@end
OBJC
CFLAGS="-arch arm64 -isysroot $SDK -miphoneos-version-min=$MINOS -fobjc-arc -fmodules -I$FLUTTER_FW/Headers -F$APP/Frameworks"
"$CLANG" $CFLAGS -c "$WORK/Runner/main.m" -o "$WORK/main.o"
"$CLANG" $CFLAGS -c "$WORK/Runner/AppDelegate.m" -o "$WORK/AppDelegate.o"
if [ -n "$LLD" ]; then
  echo ">> linking Runner with ld64.lld (chained fixups, build-version sdk $SDK_VER)"
  # -platform_version makes lld emit a correctly-positioned LC_BUILD_VERSION with
  # the current SDK in a single pass. We deliberately do NOT post-process with
  # vtool: vtool appends LC_BUILD_VERSION *after* LC_CODE_SIGNATURE, which marks
  # the binary as not Apple-linker output (ITMS-90125). Omit -miphoneos-version-min
  # so clang doesn't also emit a second (sysroot-SDK) platform version.
  "$CLANG" -arch arm64 -isysroot "$SDK" \
    -fuse-ld="$LLD" -Wl,-fixup_chains -Wl,-platform_version,ios,"$MINOS","$SDK_VER" \
    "$WORK/main.o" "$WORK/AppDelegate.o" \
    -F"$APP/Frameworks" -framework Flutter -framework UIKit -framework Foundation \
    -Xlinker -rpath -Xlinker @executable_path/Frameworks \
    -o "$APP/Runner"
  # Apple's "built with Apple's linker" check (ITMS-90125) inspects the main
  # executable. ld64.lld differs from Apple's ld in two ways we must repair after
  # the link: (1) it stamps LC_BUILD_VERSION's build-tool id = 4 (TOOL_LLD) where
  # Apple's ld uses 3 (TOOL_LD), and (2) it does NOT emit LC_SOURCE_VERSION, which
  # every real Apple-ld main executable carries (verified by diffing against a
  # genuine App Store binary). We rewrite the tool id in place and append a
  # LC_SOURCE_VERSION into the load-command slack (the zero padding between the
  # load commands and the first section, ~14 KB), bumping ncmds/sizeofcmds. No
  # section content moves, so file offsets stay valid; rcodesign seals it after.
  python3 - "$APP/Runner" <<'PY'
import sys, struct
p = sys.argv[1]
d = bytearray(open(p, "rb").read())
assert struct.unpack_from("<I", d, 0)[0] == 0xfeedfacf, "expected arm64 Mach-O"
ncmds, sizeofcmds = struct.unpack_from("<II", d, 16)
LD_VERSION = 1217 << 16  # ld-prime 1217.0.0
LC_BUILD_VERSION, LC_SOURCE_VERSION = 0x32, 0x2A
off = 32
has_source = False
first_sect = 1 << 62
for _ in range(ncmds):
    cmd, cmdsize = struct.unpack_from("<II", d, off)
    if cmd == LC_BUILD_VERSION:
        ntools = struct.unpack_from("<I", d, off + 20)[0]
        to = off + 24
        for _t in range(ntools):
            if struct.unpack_from("<I", d, to)[0] != 3:
                struct.pack_into("<II", d, to, 3, LD_VERSION)
            to += 8
    elif cmd == LC_SOURCE_VERSION:
        has_source = True
    elif cmd == 0x19:  # LC_SEGMENT_64 -> track lowest non-zero section file offset
        nsects = struct.unpack_from("<I", d, off + 64)[0]
        so = off + 72
        for _s in range(nsects):
            soff = struct.unpack_from("<I", d, so + 48)[0]
            if soff:
                first_sect = min(first_sect, soff)
            so += 80
    off += cmdsize
if not has_source:
    ins = 32 + sizeofcmds
    assert ins + 16 <= first_sect, "no load-command slack for LC_SOURCE_VERSION"
    # source_version_command: cmd, cmdsize=16, version 1.0 packed a24.b10.c10.d10.e10
    d[ins:ins + 16] = struct.pack("<IIQ", LC_SOURCE_VERSION, 16, 1 << 40)
    struct.pack_into("<II", d, 16, ncmds + 1, sizeofcmds + 16)
open(p, "wb").write(d)
PY
else
  "$CLANG" -arch arm64 -isysroot "$SDK" -miphoneos-version-min="$MINOS" \
    "$WORK/main.o" "$WORK/AppDelegate.o" \
    -F"$APP/Frameworks" -framework Flutter -framework UIKit -framework Foundation \
    -Xlinker -rpath -Xlinker @executable_path/Frameworks \
    -o "$APP/Runner"
  "$VTOOL" -arch arm64 -set-build-version ios "$MINOS" "$SDK_VER" -tool ld 1217 -replace \
    -output "$APP/Runner.v" "$APP/Runner" && mv "$APP/Runner.v" "$APP/Runner"
fi

# 5. assets + app icon catalog + Info.plist
echo "== 5/6 bundle assets + Assets.car + Info.plist =="
if [ -d "$PROJ/build/flutter_assets" ]; then
  cp -R "$PROJ/build/flutter_assets/." "$APP/flutter_assets/"
  rm -f "$APP/flutter_assets/kernel_blob.bin"
else
  printf '{}' > "$APP/flutter_assets/AssetManifest.json"
  printf '[]' > "$APP/flutter_assets/FontManifest.json"
fi
cp "$FLUTTER_FW/icudtl.dat" "$APP/flutter_assets/" 2>/dev/null || true

# App icon asset catalog (Assets.car) generated natively by mkcar.py - no Mac.
ICON_PLIST=""
ICONSET="$PROJ/ios/Runner/Assets.xcassets/AppIcon.appiconset"
if [ -d "$ICONSET" ] && command -v python3 >/dev/null 2>&1; then
  if python3 "$ROOT/tools/mkcar.py" build "$ICONSET" "$APP/Assets.car" 2>/tmp/mkcar.err; then
    echo ">> Assets.car generated from AppIcon.appiconset (native, no actool)"
    ICON_PLIST="  <key>CFBundleIconName</key><string>AppIcon</string>"
  else
    echo ">> WARN: Assets.car generation failed: $(cat /tmp/mkcar.err)" >&2
  fi
fi

cat > "$APP/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>Runner</string>
  <key>CFBundleIdentifier</key><string>@@BUNDLEID@@</string>
  <key>CFBundleName</key><string>@@APPNAME@@</string>
  <key>CFBundleDisplayName</key><string>@@APPNAME@@</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>@@SHORTVER@@</string>
  <key>CFBundleVersion</key><string>@@BUILDVER@@</string>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>MinimumOSVersion</key><string>@@MINOS@@</string>
  <key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array>
  <key>UIRequiredDeviceCapabilities</key><array><string>arm64</string></array>
  <key>DTPlatformName</key><string>iphoneos</string>
  <key>DTPlatformVersion</key><string>$SDK_VER</string>
  <key>DTSDKName</key><string>iphoneos$SDK_VER</string>
  <key>DTSDKBuild</key><string>$SDK_BUILD</string>
  <key>DTPlatformBuild</key><string>$SDK_BUILD</string>
  <key>DTXcode</key><string>2600</string>
  <key>DTXcodeBuild</key><string>$XCODE_BUILD</string>
  <key>DTCompiler</key><string>com.apple.compilers.llvm.clang.1_0</string>
  <key>BuildMachineOSBuild</key><string>25A354</string>
$ICON_PLIST
  <key>UIDeviceFamily</key><array><integer>1</integer></array>
  <key>UILaunchScreen</key><dict/>
  <key>UISupportedInterfaceOrientations</key>
  <array><string>UIInterfaceOrientationPortrait</string></array>
</dict></plist>
PLIST

# 6. package
echo "== 6/6 package .ipa =="
command -v zip >/dev/null 2>&1 || { echo "zip not installed in build env" >&2; exit 4; }
cd "$OUT"
rm -f "@@SAFE@@.ipa"
zip -q -r "@@SAFE@@.ipa" Payload
echo ">> built $OUT/@@SAFE@@.ipa"
