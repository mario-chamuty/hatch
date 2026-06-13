#!/usr/bin/env python3
"""Prove the plugin LINK (Workstream B7): compile GeneratedPluginRegistrant.m +
a Runner that registers plugins, then link Runner + all 72 plugin objects +
Firebase static frameworks, letting -fmodules autolink (LC_LINKER_OPTION) pull
the system + Firebase frameworks. Uses the EXACT clang/ld64.lld the Rust
native_pipeline uses, so the recipe transfers 1:1.

Usage: python link_app.py <project> <plugout_dir>
"""
import os, sys, glob, subprocess, shutil

PROJECT = sys.argv[1]
OUT = sys.argv[2]
TC   = r"C:\Library\Developer\Toolchains\unknown-Asserts-development.xctoolchain\usr\bin"
LLVM = r"C:\Users\mario\iospoc-win\llvm\bin"
SDK  = r"C:\Users\mario\iospoc-win\iossdk\iPhoneOS16.5.sdk"
FFW  = r"C:\Users\mario\iospoc-win\engine\ios-release\Flutter.xcframework\ios-arm64"
FBFW = r"C:\Users\mario\iospoc-win\dl\fbframeworks"
CLANG = os.path.join(LLVM, "clang.exe")
RESCLANG = glob.glob(os.path.join(TC, r"..\lib\clang\*"))
RESCLANG = os.path.abspath(RESCLANG[0]) if RESCLANG else ""
TARGET = "arm64-apple-ios13.0"
INC = os.path.join(OUT, "inc")
MOD = os.path.join(OUT, "mod")
OBJ = os.path.join(OUT, "obj")
PREFIX = os.path.join(OUT, "objc_prefix.h")
LINKDIR = os.path.join(OUT, "link")
os.makedirs(LINKDIR, exist_ok=True)

RDIR = os.path.join(PROJECT, "ios", "Runner")

# Swift plugin module maps (each MOD/<name>/module.modulemap) -> -fmodule-map-file
swift_modmaps = []
for mm in glob.glob(os.path.join(MOD, "*", "module.modulemap")):
    swift_modmaps += ["-fmodule-map-file=" + mm]
# also expose the SPM ObjC sibling module maps (MOD/<name>.modulemap)
for mm in glob.glob(os.path.join(MOD, "*.modulemap")):
    swift_modmaps += ["-fmodule-map-file=" + mm]

import json
def plugin_include_dirs(project):
    """Real header search dirs for every plugin, so the registrant's
    `<plugin/Header.h>` imports (and their root-relative sub-imports, e.g.
    sqflite's `include/sqflite_darwin/...`) resolve against the original tree."""
    dirs = []
    d = json.load(open(os.path.join(project, ".flutter-plugins-dependencies")))
    for pl in d["plugins"]["ios"]:
        name = pl["name"]; path = pl["path"].rstrip("\\/")
        for sub in ("ios", "darwin"):
            base = os.path.join(path, sub)
            if not os.path.isdir(base):
                continue
            for r, _, files in os.walk(base):
                low = r.lower()
                if any(x in low for x in ("example", "tests", ".symlinks", "macos")):
                    continue
                b = os.path.basename(r)
                if any(f.endswith(".h") for f in files):
                    dirs.append(r)
                # parent of a dir named <plugin> or 'include' -> <plugin/H.h>
                if b == name or b == "include":
                    dirs.append(os.path.dirname(r))
                # SPM target source root (parent of include/) so root-relative
                # "include/<plugin>/X.h" imports resolve
                if b == "include":
                    dirs.append(os.path.dirname(r))
                if b == "Sources":
                    for sd in os.listdir(r):
                        dirs.append(os.path.join(r, sd))
    # de-dup, keep order
    seen = set(); out = []
    for x in dirs:
        if x not in seen and os.path.isdir(x):
            seen.add(x); out.append(x)
    return out

PLUGIN_INCS = []
for _d in plugin_include_dirs(PROJECT):
    PLUGIN_INCS += ["-I", _d]

def compile_objc(src, obj, extra):
    cmd = [CLANG, "-target", TARGET, "-isysroot", SDK, "-resource-dir", RESCLANG,
           "-fobjc-arc", "-fmodules", "-fmodules-cache-path=" + os.path.join(OUT, "cmc_runner"),
           "-include", PREFIX, "-w",
           "-I", os.path.join(FFW, "Headers"),
           "-I", INC, "-I", MOD,
           "-F", FFW, "-F", FBFW] + PLUGIN_INCS + swift_modmaps + extra + ["-c", src, "-o", obj]
    print(">> compile", os.path.basename(src))
    return subprocess.run(cmd).returncode

# --- write AppDelegate that registers plugins ---
appdel_h = '#import <Flutter/Flutter.h>\n#import <UIKit/UIKit.h>\n@interface AppDelegate : FlutterAppDelegate\n@end\n'
appdel_m = (
    '#import "AppDelegate.h"\n'
    '#import "GeneratedPluginRegistrant.h"\n'
    '@implementation AppDelegate\n'
    '- (BOOL)application:(UIApplication *)application didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {\n'
    '  [GeneratedPluginRegistrant registerWithRegistry:self];\n'
    '  return [super application:application didFinishLaunchingWithOptions:launchOptions];\n'
    '}\n@end\n'
)
main_m = (
    '#import <UIKit/UIKit.h>\n#import "AppDelegate.h"\n'
    'int main(int argc, char * argv[]) {\n'
    '  @autoreleasepool { return UIApplicationMain(argc, argv, nil, NSStringFromClass([AppDelegate class])); }\n'
    '}\n'
)
open(os.path.join(LINKDIR, "AppDelegate.h"), "w").write(appdel_h)
open(os.path.join(LINKDIR, "AppDelegate.m"), "w").write(appdel_m)
open(os.path.join(LINKDIR, "main.m"), "w").write(main_m)

rc = 0
# registrant.o (from the project's GeneratedPluginRegistrant.m)
reg_m = os.path.join(RDIR, "GeneratedPluginRegistrant.m")
rc |= compile_objc(reg_m, os.path.join(LINKDIR, "registrant.o"),
                   ["-I", RDIR])
rc |= compile_objc(os.path.join(LINKDIR, "AppDelegate.m"),
                   os.path.join(LINKDIR, "AppDelegate.o"), ["-I", RDIR])
rc |= compile_objc(os.path.join(LINKDIR, "main.m"),
                   os.path.join(LINKDIR, "main.o"), [])
if rc:
    print("!! compile failed"); sys.exit(1)

# --- link ---
plugin_objs = sorted(glob.glob(os.path.join(OBJ, "*.o")))
runner = os.path.join(LINKDIR, "Runner")
fb_frameworks = []
for fw in glob.glob(os.path.join(FBFW, "*.framework")):
    fb_frameworks += ["-framework", os.path.splitext(os.path.basename(fw))[0]]
link = [CLANG, "-target", TARGET, "-isysroot", SDK,
        "-fuse-ld=" + os.path.join(LLVM, "ld64.lld.exe"),
        "-Wl,-fixup_chains", "-Wl,--error-limit=0", "-Wl,-platform_version,ios,13.0,26.0",
        os.path.join(LINKDIR, "main.o"), os.path.join(LINKDIR, "AppDelegate.o"),
        os.path.join(LINKDIR, "registrant.o")] + plugin_objs + [
        "-F", FFW, "-F", FBFW,
        "-framework", "Flutter", "-framework", "UIKit", "-framework", "Foundation",
        ] + fb_frameworks + [
        "-lc++", "-lz", "-lsqlite3",
        # Swift runtime: link the SDK .tbd stubs; the dylibs live in the OS on
        # iOS 12.2+ so rpath /usr/lib/swift resolves them at runtime.
        "-L", os.path.join(SDK, "usr", "lib", "swift"),
        # genuine Swift back-deployment compatibility static archives (force-loaded
        # via Firebase's autolink hints) from the Xcode-extract iphoneos toolchain
        "-L", r"D:\dartwin\xcode\extract\Xcode.app\Contents\Developer\Toolchains\XcodeDefault.xctoolchain\usr\lib\swift\iphoneos",
        # compiler-rt builtins (__isPlatformVersionAtLeast for @available checks)
        "-L", r"D:\dartwin\xcode\extract\Xcode.app\Contents\Developer\Toolchains\XcodeDefault.xctoolchain\usr\lib\clang\21\lib\darwin",
        "-lclang_rt.ios",
        "-Xlinker", "-rpath", "-Xlinker", "/usr/lib/swift",
        "-Xlinker", "-rpath", "-Xlinker", "@executable_path/Frameworks",
        "-o", runner]
print(">> linking Runner with %d plugin objects + %d firebase frameworks" %
      (len(plugin_objs), len(fb_frameworks) // 2))
p = subprocess.run(link, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
sys.stdout.write(p.stdout.decode("utf-8", "replace"))
print("LINK rc =", p.returncode)
if p.returncode == 0:
    print("Runner linked:", runner, os.path.getsize(runner), "bytes")
