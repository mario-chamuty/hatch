#!/usr/bin/env python3
"""Complete the iPhoneOS16.5 SDK for LINKING Swift code (Firebase's prebuilt
Swift frameworks + our Swift plugins) Mac-free:

  1. Generate arm64-ios .tbd stubs for the Swift C-overlay dylibs the SDK lacks
     (swiftXPC, swift_time, swiftunistd, ...). These dylibs ship in the OS on
     iOS 12.2+; the stub only needs to resolve the `__swift_FORCE_LOAD_$_<name>`
     anchor at link time (dyld loads the real OS dylib at runtime via the
     install-name + the /usr/lib/swift rpath). The Xcode-extract tbds are
     arm64e-only, so we emit our own arm64-ios stubs.

  2. (compat libs are static .a force-loaded from the Xcode-extract toolchain;
     handled in the link via -L, not here.)

Idempotent. Writes into <SDK>/usr/lib/swift/.
"""
import os, sys

SDK = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\mario\iospoc-win\iossdk\iPhoneOS16.5.sdk"
SWDIR = os.path.join(SDK, "usr", "lib", "swift")

# Swift C-overlay dylibs in the OS but missing from this SDK's stubs. Each maps
# to FORCE_LOAD symbol `__swift_FORCE_LOAD_$_<name>` and install-name
# /usr/lib/swift/libswift<...>.dylib. (name == libname without the "lib" prefix.)
OVERLAYS = [
    "swiftXPC", "swift_time", "swift_errno", "swift_math", "swift_signal",
    "swift_stdio", "swiftsys_time", "swiftunistd", "swift_Builtin_float",
    # reexport targets some overlays pull; harmless to stub too
    "swift_DarwinFoundation1", "swift_DarwinFoundation2", "swift_DarwinFoundation3",
]

TBD = """--- !tapi-tbd
tbd-version:     4
targets:         [ arm64-ios ]
install-name:    '/usr/lib/swift/lib{name}.dylib'
exports:
  - targets:         [ arm64-ios ]
    symbols:         [ '__swift_FORCE_LOAD_$_{name}' ]
...
"""

n = 0
for name in OVERLAYS:
    p = os.path.join(SWDIR, "lib%s.tbd" % name)
    if os.path.exists(p):
        continue
    open(p, "w", newline="\n").write(TBD.format(name=name))
    n += 1
print("wrote %d overlay tbd stubs into %s" % (n, SWDIR))
