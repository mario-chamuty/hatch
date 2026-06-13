#!/usr/bin/env python3
"""Mac-free Windows-native Flutter iOS plugin compiler.
Compiles each plugin's Swift/ObjC sources to arm64-apple-ios objects, and stages
public headers (for `#import <plugin/H.h>`) + module maps (for `@import plugin`)
so the Flutter GeneratedPluginRegistrant compiles and links.
Run under git-bash python with Windows paths."""
import json, os, sys, glob, subprocess, shutil

PROJECT = sys.argv[1]
OUT = sys.argv[2]
ONLY = set(sys.argv[3:]) if len(sys.argv) > 3 else None

TC   = r"C:\Library\Developer\Toolchains\unknown-Asserts-development.xctoolchain\usr\bin"
RT   = r"C:\Program Files\Swift\runtime-development\usr\bin"
SDK  = r"C:\Users\mario\iospoc-win\iossdk\iPhoneOS16.5.sdk"
FFW  = r"C:\Users\mario\iospoc-win\engine\ios-release\Flutter.xcframework\ios-arm64"
RESCLANG = glob.glob(os.path.join(TC, r"..\lib\clang\*"))
RESCLANG = os.path.abspath(RESCLANG[0]) if RESCLANG else ""
SWLIB = os.path.abspath(os.path.join(TC, r"..\lib\swift"))
FBFW = r"C:\Users\mario\iospoc-win\dl\fbframeworks"  # gathered Firebase xcframework slices
TARGET = "arm64-apple-ios13.0"
SWIFTC = os.path.join(TC, "swiftc.exe")
CLANG  = os.path.join(TC, "clang.exe")
os.environ["PATH"] = TC + ";" + RT + ";" + os.environ.get("PATH", "")

PREFIX = None  # set in main(): an ObjC prefix header importing Foundation+UIKit
INC = os.path.join(OUT, "inc")          # staged public headers: inc/<plugin>/*.h
MOD = os.path.join(OUT, "mod")          # staged module maps:   mod/<plugin>/module.modulemap
OBJ = os.path.join(OUT, "obj")          # compiled objects
for d in (INC, MOD, OBJ):
    os.makedirs(d, exist_ok=True)

def run(cmd, logf):
    with open(logf, "wb") as f:
        p = subprocess.run(cmd, stdout=f, stderr=subprocess.STDOUT)
    return p.returncode

# Minimal, behavior-preserving source shims for plugins that stock compnerd 5.9
# mis-diagnoses (newer Apple Swift accepts them). Applied to a staged COPY only.
SHIMS = {
    "local_auth_darwin": [(
        "LocalAuthPlugin.swift",
        "      ) { [weak self] (success: Bool, error: Error?) in\n"
        "        DispatchQueue.main.async {\n"
        "          self?.handleAuthReply(",
        "      ) { [weak self] (success: Bool, error: Error?) in\n"
        "        guard let self = self else { return }\n"
        "        DispatchQueue.main.async {\n"
        "          self.handleAuthReply(",
    )],
}

def apply_shims(name, sw):
    if name not in SHIMS:
        return sw
    pdir = os.path.join(OUT, "patched", name)
    os.makedirs(pdir, exist_ok=True)
    out = []
    rules = {b: (o, n) for (b, o, n) in SHIMS[name]}
    for f in sw:
        b = os.path.basename(f)
        if b in rules:
            o, n = rules[b]
            s = open(f, encoding="utf-8", errors="surrogateescape").read()
            if o not in s:
                print("    !! shim miss for %s/%s" % (name, b))
            s = s.replace(o, n)
            pf = os.path.join(pdir, b)
            open(pf, "w", encoding="utf-8", errors="surrogateescape", newline="").write(s)
            out.append(pf)
        else:
            out.append(f)
    return out

def sources(path):
    sw, oc, hdrdirs, hdrs = [], [], set(), []
    for sub in ("ios", "darwin"):
        base = os.path.join(path, sub)
        if not os.path.isdir(base):
            continue
        for r, dirs, files in os.walk(base):
            low = r.lower()
            if any(x in low for x in ("example", "tests", ".symlinks", "macos")):
                continue
            for f in files:
                fp = os.path.join(r, f)
                if f.endswith(".swift") and f.lower() != "package.swift":
                    sw.append(fp)
                elif f.endswith((".m", ".mm")):
                    oc.append(fp)
                elif f.endswith(".h"):
                    hdrs.append(fp); hdrdirs.add(r)
    return sw, oc, sorted(hdrdirs), hdrs

def spm_target_dirs(path):
    """Return target source dirs for an SPM-layout plugin: every immediate
    child of a `Sources/` dir that actually contains compilable sources.
    A plugin with >1 such dir is a multi-target package (e.g. a Swift module +
    a sibling ObjC module it imports) and needs per-module compilation."""
    tdirs = []
    for sub in ("ios", "darwin"):
        base = os.path.join(path, sub)
        if not os.path.isdir(base):
            continue
        for r, dirs, files in os.walk(base):
            low = r.lower()
            if any(x in low for x in ("example", "tests", ".symlinks", "macos")):
                dirs[:] = []
                continue
            if os.path.basename(r) == "Sources":
                for d in sorted(dirs):
                    td = os.path.join(r, d)
                    has_src = any(f.endswith((".swift", ".m", ".mm"))
                                  for _, _, fs in os.walk(td) for f in fs)
                    if has_src:
                        tdirs.append(td)
                dirs[:] = []  # don't descend further; targets handled here
    return tdirs

def target_sources(tdir):
    sw, oc, hdrs, hdrdirs = [], [], [], set()
    for r, dirs, files in os.walk(tdir):
        if any(x in r.lower() for x in ("example", "tests", ".symlinks", "macos")):
            continue
        for f in files:
            fp = os.path.join(r, f)
            if f.endswith(".swift") and f.lower() != "package.swift":
                sw.append(fp)
            elif f.endswith((".m", ".mm")):
                oc.append(fp)
            elif f.endswith(".h"):
                hdrs.append(fp); hdrdirs.add(r)
    return sw, oc, hdrs, sorted(hdrdirs)

def compile_multitarget(name, path, tdirs, log):
    """Compile an SPM multi-target plugin module-by-module: ObjC targets first
    (each becomes a clang module a Swift sibling can `import`), then Swift
    targets, wired to those ObjC modules. General for any plugin whose Package
    splits a Swift module from an ObjC module it depends on."""
    open(log, "wb").close()
    targets = []
    for td in tdirs:
        sw, oc, hdrs, hdrdirs = target_sources(td)
        targets.append({"name": os.path.basename(td), "dir": td,
                        "sw": sw, "oc": oc, "hdrs": hdrs, "hdrdirs": hdrdirs})
    objc_targets = [t for t in targets if t["oc"] and not t["sw"]]
    swift_targets = [t for t in targets if t["sw"]]
    objs = []
    swift_xcc = []          # -Xcc flags exposing the ObjC clang modules to Swift
    for t in objc_targets:
        tname = t["name"]
        # publicHeadersPath convention: include/<tname>
        pub_dir = os.path.join(t["dir"], "include", tname)
        if not os.path.isdir(pub_dir):
            pub_dir = t["dir"]
        inc_parent = os.path.dirname(pub_dir)   # so <tname/Header.h> angle imports resolve
        stage_objc_headers(tname, glob.glob(os.path.join(pub_dir, "*.h")))
        tinc = ["-I", inc_parent, "-I", pub_dir, "-I", INC, "-I", t["dir"]]
        # compile each .m
        ok_t = True
        for m in t["oc"]:
            o = os.path.join(OBJ, name + "." + tname + "." +
                             os.path.splitext(os.path.basename(m))[0] + ".o")
            cmd = [CLANG, "-target", TARGET, "-isysroot", SDK,
                   "-resource-dir", RESCLANG, "-fobjc-arc", "-fmodules",
                   "-fmodules-cache-path=" + os.path.join(OUT, "cmc_" + tname),
                   "-include", PREFIX, "-w", "-F", FFW] + tinc + ["-c", m, "-o", o]
            p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            with open(log, "ab") as lf:
                lf.write(("\n=== %s/%s rc=%d ===\n" % (tname, os.path.basename(m), p.returncode)).encode())
                lf.write(p.stdout or b"")
            if p.returncode == 0:
                objs.append(o)
            else:
                ok_t = False
        if not ok_t:
            return False, objs
        # synthesize a clang module map so a Swift sibling can `import <tname>`
        mm = os.path.join(MOD, tname + ".modulemap")
        with open(mm, "w") as f:
            f.write('module %s {\n  umbrella "%s"\n  export *\n}\n'
                    % (tname, pub_dir.replace("\\", "/")))
        swift_xcc += ["-Xcc", "-fmodule-map-file=" + mm,
                      "-Xcc", "-I", "-Xcc", inc_parent,
                      "-Xcc", "-I", "-Xcc", INC]
    for t in swift_targets:
        tname = t["name"]
        swift_hdr = os.path.join(MOD, tname + "-Swift.h")
        o = os.path.join(OBJ, name + "." + tname + ".swift.o")
        cmd = [SWIFTC, "-target", TARGET, "-sdk", SDK, "-wmo",
               "-resource-dir", SWLIB, "-Xcc", "-resource-dir", "-Xcc", RESCLANG,
               "-F", FFW, "-Xcc", "-F", "-Xcc", FFW,
               "-module-cache-path", os.path.join(OUT, "mc_" + tname),
               "-emit-object", "-emit-module",
               "-emit-module-path", os.path.join(MOD, tname + ".swiftmodule"),
               "-emit-objc-header", "-emit-objc-header-path", swift_hdr,
               "-module-name", tname, "-parse-as-library",
               "-swift-version", "5", "-strict-concurrency=minimal"] + \
              swift_xcc + ["-o", o] + t["sw"]
        rc = run_append(cmd, log, "%s (swift)" % tname)
        if rc != 0:
            return False, objs
        objs.append(o)
        mm_dir = os.path.join(MOD, tname)
        os.makedirs(mm_dir, exist_ok=True)
        shutil.copy(swift_hdr, os.path.join(mm_dir, tname + "-Swift.h"))
        with open(os.path.join(mm_dir, "module.modulemap"), "w") as f:
            f.write('module %s {\n  header "%s-Swift.h"\n  export *\n}\n' % (tname, tname))
    return True, objs

def run_append(cmd, logf, tag):
    p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    with open(logf, "ab") as lf:
        lf.write(("\n=== %s rc=%d ===\n" % (tag, p.returncode)).encode())
        lf.write(p.stdout or b"")
    return p.returncode

def plugin_version(path):
    base = os.path.basename(path)
    if "-" in base:
        return base.rsplit("-", 1)[1]
    return "0.0.1"

def podspec_defines(path, version):
    """Parse GCC_PREPROCESSOR_DEFINITIONS from the plugin podspec -> -D args."""
    import re
    specs = glob.glob(os.path.join(path, "**", "*.podspec"), recursive=True)
    if not specs:
        return []
    s = open(specs[0], encoding="utf-8", errors="replace").read()
    m = re.search(r"GCC_PREPROCESSOR_DEFINITIONS['\"]?\s*=>\s*\"(.*)\",?\s*$", s, re.M)
    if not m:
        return []
    defs = m.group(1).replace("\\", "")   # collapse \\\" escaping -> "
    defs = defs.replace("#{library_version}", version).replace("#{s.version}", version)
    defs = defs.replace("$(inherited)", "").strip()
    out = []
    # split on spaces that are not inside quotes
    cur = ""; q = False
    toks = []
    for ch in defs:
        if ch == '"':
            q = not q; cur += ch
        elif ch == " " and not q:
            if cur:
                toks.append(cur); cur = ""
        else:
            cur += ch
    if cur:
        toks.append(cur)
    for t in toks:
        if t:
            out += ["-D" + t]
    return out

def stage_objc_headers(name, hdrs):
    """Copy public .h so `#import <name/Header.h>` resolves via -I INC."""
    dst = os.path.join(INC, name)
    os.makedirs(dst, exist_ok=True)
    for h in hdrs:
        shutil.copy(h, os.path.join(dst, os.path.basename(h)))

def main():
    global PREFIX
    PREFIX = os.path.join(OUT, "objc_prefix.h")
    with open(PREFIX, "w") as f:
        f.write("#ifdef __OBJC__\n#import <Foundation/Foundation.h>\n#import <UIKit/UIKit.h>\n#endif\n")
    d = json.load(open(os.path.join(PROJECT, ".flutter-plugins-dependencies")))
    results = {}
    for pl in d["plugins"]["ios"]:
        name = pl["name"]
        if ONLY and name not in ONLY:
            continue
        path = pl["path"].rstrip("\\/")
        # Multi-target SPM plugin (e.g. Swift module + sibling ObjC module):
        # compile each module separately, wiring Swift to the ObjC clang modules.
        tdirs = spm_target_dirs(path)
        if len(tdirs) > 1:
            log = os.path.join(OUT, name + ".log")
            ok, objs = compile_multitarget(name, path, tdirs, log)
            results[name] = (ok, len(objs))
            print("[%s] %s  objs=%d  (multi-target: %d modules)%s" %
                  ("OK " if ok else "FAIL", name, len(objs), len(tdirs),
                   "" if ok else "  -> see " + log))
            continue
        sw, oc, hdrdirs, hdrs = sources(path)
        sw = apply_shims(name, sw)
        objs = []
        ok = True
        log = os.path.join(OUT, name + ".log")
        # header search args: every dir containing .h, plus include parents so
        # `<name/Header.h>` resolves (SPM include/<name> -> -I include).
        incargs = []
        for hd in hdrdirs:
            incargs += ["-I", hd]
            # if hd ends with /include/<name>, also add /include
            parent = os.path.dirname(hd)
            if os.path.basename(hd) == name and os.path.basename(parent) == "include":
                incargs += ["-I", parent]
            if os.path.basename(hd) == "include":
                incargs += ["-I", hd]
        swift_hdr = None
        # ---- Swift ----
        if sw:
            swift_hdr = os.path.join(MOD, name + "-Swift.h")
            os.makedirs(MOD, exist_ok=True)
            o = os.path.join(OBJ, name + ".swift.o")
            cmd = [SWIFTC, "-target", TARGET, "-sdk", SDK, "-wmo",
                   "-resource-dir", SWLIB, "-Xcc", "-resource-dir", "-Xcc", RESCLANG,
                   "-F", FFW, "-module-cache-path", os.path.join(OUT, "mc_" + name),
                   "-emit-object", "-emit-module",
                   "-emit-module-path", os.path.join(MOD, name + ".swiftmodule"),
                   "-emit-objc-header", "-emit-objc-header-path", swift_hdr,
                   "-module-name", name, "-parse-as-library",
                   "-swift-version", "5", "-strict-concurrency=minimal",
                   "-o", o] + sw
            rc = run(cmd, log)
            if rc == 0:
                objs.append(o)
                # stage module map so `@import name` works (umbrella = -Swift.h)
                mm_dir = os.path.join(MOD, name)
                os.makedirs(mm_dir, exist_ok=True)
                shutil.copy(swift_hdr, os.path.join(mm_dir, name + "-Swift.h"))
                with open(os.path.join(mm_dir, "module.modulemap"), "w") as f:
                    f.write('module %s {\n  header "%s-Swift.h"\n  export *\n}\n' % (name, name))
            else:
                ok = False
        # ---- ObjC ----
        if oc and ok:
            stage_objc_headers(name, hdrs)
            extra = list(incargs)
            extra += ["-I", INC]  # cross-plugin public headers (<otherplugin/H.h>)
            extra += podspec_defines(path, plugin_version(path))  # -DLIBRARY_NAME=... etc
            if name.startswith("firebase"):  # Firebase prebuilt xcframework headers
                extra += ["-F", FBFW]
            if swift_hdr:  # mixed: let .m find the generated Swift header
                extra += ["-I", MOD, "-include", swift_hdr]
            for m in oc:
                o = os.path.join(OBJ, name + "." + os.path.splitext(os.path.basename(m))[0] + ".o")
                cmd = [CLANG, "-target", TARGET, "-isysroot", SDK,
                       "-resource-dir", RESCLANG, "-fobjc-arc", "-fmodules",
                       "-fmodules-cache-path=" + os.path.join(OUT, "cmc_" + name),
                       "-include", PREFIX,
                       # -w: stock compnerd clang's TextDiagnostic renderer asserts
                       # (StartColNo<=EndColNo, Invalid range!) while PRINTING certain
                       # benign warnings (deprecated decl / mismatched param types in
                       # firebase_core FLTFirebaseCorePlugin.m). The compile itself is
                       # clean; killing warning rendering dodges the renderer crash.
                       "-w",
                       "-F", FFW] + extra + ["-c", m, "-o", o]
                p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
                with open(log, "ab") as lf:
                    lf.write(("\n=== %s rc=%d ===\n" % (os.path.basename(m), p.returncode)).encode())
                    lf.write(p.stdout or b"")
                if p.returncode == 0:
                    objs.append(o)
                else:
                    ok = False
        results[name] = (ok, len(objs))
        print("[%s] %s  objs=%d  (swift=%d objc=%d)%s" %
              ("OK " if ok else "FAIL", name, len(objs), len(sw), len(oc),
               "" if ok else "  -> see " + log))
    print("\n=== SUMMARY: %d/%d plugins compiled ===" %
          (sum(1 for v in results.values() if v[0]), len(results)))
    bad = [k for k, v in results.items() if not v[0]]
    if bad:
        print("FAILED:", bad)

main()
