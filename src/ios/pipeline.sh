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
"$VTOOL" -arch arm64 -set-build-version ios "$MINOS" "$MINOS" -replace \
  -output "$APP/Frameworks/App.framework/App" "$APP/Frameworks/App.framework/App" 2>/dev/null || true
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
"$CLANG" -arch arm64 -isysroot "$SDK" -miphoneos-version-min="$MINOS" \
  "$WORK/main.o" "$WORK/AppDelegate.o" \
  -F"$APP/Frameworks" -framework Flutter -framework UIKit -framework Foundation \
  -Xlinker -rpath -Xlinker @executable_path/Frameworks \
  -o "$APP/Runner"

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
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>MinimumOSVersion</key><string>@@MINOS@@</string>
  <key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array>
  <key>DTPlatformName</key><string>iphoneos</string>
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
