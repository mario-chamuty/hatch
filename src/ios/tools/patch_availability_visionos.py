#!/usr/bin/env python3
"""Add the visionOS (and xros/bridgeos) platform spellings to the iPhoneOS16.5
SDK's AvailabilityInternal.h. The 16.5 SDK predates visionOS, so headers that
write `API_AVAILABLE(ios(3.0), ..., visionos(1.0))` (current Flutter plugin
headers do) fail to expand `visionos(1.0)` and clang reports `expected ','`.

Each `__API_*_PLATFORM_<plat>` macro just maps the spelling to the clang
availability-attribute form; clang then emits a benign `unknown platform`
warning (suppressed by -w) and ignores the visionOS clause. Our arm64-apple-ios
target only cares about the ios() clause, so semantics are unaffected.

Idempotent."""
import sys, os

A = sys.argv[1] if len(sys.argv) > 1 else (
    r"C:\Users\mario\iospoc-win\iossdk\iPhoneOS16.5.sdk\usr\include\AvailabilityInternal.h")

# (anchor line, lines to insert after it)
ADDS = [
    ("    #define __API_AVAILABLE_PLATFORM_ios(x) ios,introduced=x\n",
     ["    #define __API_AVAILABLE_PLATFORM_visionos(x) visionos,introduced=x\n",
      "    #define __API_AVAILABLE_PLATFORM_xros(x) xros,introduced=x\n",
      "    #define __API_AVAILABLE_PLATFORM_bridgeos(x) bridgeos,introduced=x\n"]),
    ("    #define __API_DEPRECATED_PLATFORM_ios(x,y) ios,introduced=x,deprecated=y\n",
     ["    #define __API_DEPRECATED_PLATFORM_visionos(x,y) visionos,introduced=x,deprecated=y\n",
      "    #define __API_DEPRECATED_PLATFORM_xros(x,y) xros,introduced=x,deprecated=y\n",
      "    #define __API_DEPRECATED_PLATFORM_bridgeos(x,y) bridgeos,introduced=x,deprecated=y\n"]),
    ("    #define __API_UNAVAILABLE_PLATFORM_ios ios,unavailable\n",
     ["    #define __API_UNAVAILABLE_PLATFORM_visionos visionos,unavailable\n",
      "    #define __API_UNAVAILABLE_PLATFORM_xros xros,unavailable\n",
      "    #define __API_UNAVAILABLE_PLATFORM_bridgeos bridgeos,unavailable\n"]),
]

s = open(A, encoding="utf-8", errors="surrogateescape").read()
changed = False
for anchor, adds in ADDS:
    if "_PLATFORM_visionos" in s and adds[0] in s:
        continue
    assert anchor in s, "anchor missing: %r" % anchor[:50]
    block = "".join(a for a in adds if a not in s)
    if block:
        s = s.replace(anchor, anchor + block, 1)
        changed = True
if changed:
    open(A, "w", encoding="utf-8", errors="surrogateescape", newline="").write(s)
print(("PATCHED " if changed else "already-patched ") + A)
