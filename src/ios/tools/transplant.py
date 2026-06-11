#!/usr/bin/env python3
"""Pixel transplant: take a GENUINE actool Assets.car (Kazumi, CoreUI 918) and
replace only the app-icon pixel payloads with our own artwork, preserving every
container byte (BOM slot table incl. free list, block order, trees, keys,
CARHEADER) except block addresses/lengths and the spliced payloads.

Works because our appiconset has exactly the same 25 standard entries
(flutter_launcher_icons output) as the donor's: same sizes, keys, dim2 scheme.

The from-scratch mkcar catalog passes Apple's *validation* but silently WEDGES
the *processing* stage (ITMS-90596). This transplant is the proven-VALID path
(App Store build 41). mkcar.py lives next to this file in the deployed tools dir.

Usage: transplant.py <donor.car> <AppIcon.appiconset> <out.car>
"""
import importlib.util, os, struct, sys

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("mkcar", os.path.join(HERE, "mkcar.py"))
mkcar = importlib.util.module_from_spec(spec); spec.loader.exec_module(mkcar)

donor_path, iconset, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
d = open(donor_path, "rb").read()

# ---- parse BOM container, keeping the slot table VERBATIM ----
ver, nblocks, ioff, ilen, voff, vlen = struct.unpack_from(">IIIIII", d, 8)
nslots = struct.unpack_from(">I", d, ioff)[0]
slots = [list(struct.unpack_from(">II", d, ioff + 4 + 8 * i)) for i in range(nslots)]
free_tail = d[ioff + 4 + 8 * nslots: ioff + ilen]      # free-list bytes, verbatim
vars_blob = d[voff:voff + vlen]
blocks = {}                                             # idx -> bytes
for i, (a, l) in enumerate(slots):
    if a or l:
        blocks[i] = d[a:a + l]

# vars table -> name:idx
vt = {}
vc = struct.unpack_from(">I", vars_blob, 0)[0]; p = 4
for _ in range(vc):
    idx, nl = struct.unpack_from(">IB", vars_blob, p); p += 5
    vt[vars_blob[p:p + nl].decode()] = idx; p += nl

# ---- load our icon pixels, indexed by (width, height) ----
imgs = mkcar.load_appiconset(iconset)
by_wh = {}
for im in imgs:
    by_wh[(im["width"], im["height"])] = im["bgra"]
print("our icons:", sorted(by_wh))

# ---- find icon renditions in the donor RENDITIONS tree, splice pixels ----
def tree_leaf(idx):
    tb = blocks[idx]
    child = struct.unpack_from(">I", tb, 8)[0]
    return child

leaf_idx = tree_leaf(vt["RENDITIONS"])
leaf = bytearray(blocks[leaf_idx])
cnt = struct.unpack_from(">H", leaf, 2)[0]
replaced = 0
for i in range(cnt):
    vidx, kidx = struct.unpack_from(">II", leaf, 12 + 8 * i)
    cb = blocks[vidx]
    if cb[:4] != b"ISTC":
        continue
    w, h = struct.unpack_from("<II", cb, 12)
    layout = struct.unpack_from("<H", cb, 36)[0]
    name = cb[40:168].split(b"\0")[0].decode("latin1")
    if not name.startswith("Icon-App"):
        continue                       # leave donor launch images untouched
    if layout not in (0x0C,):          # packed sizes live in atlases (below)
        continue
    if (w, h) not in by_wh:
        print("  !! no source pixels for %dx%d (%s) - keeping donor" % (w, h, name))
        continue
    tvl = struct.unpack_from("<I", cb, 168)[0]
    stride = w * 4
    tp = 184
    while tp + 8 <= 184 + tvl:
        t, l = struct.unpack_from("<II", cb, tp)
        if t == 0x3EF:
            stride = struct.unpack_from("<I", cb, tp + 8)[0]
        tp += 8 + l
    padded, stride_px = mkcar.pad_rows(by_wh[(w, h)], w, h)
    assert stride_px * 4 == stride, "stride mismatch %d vs %d" % (stride_px * 4, stride)
    mlec = mkcar.mlec_lzfse(padded, stride_px, h)
    new_cb = bytearray(cb[:184 + tvl]) + mlec
    struct.pack_into("<I", new_cb, 0xB4, len(mlec))     # renditionLength
    blocks[vidx] = bytes(new_cb)
    replaced += 1
    print("  spliced %-28s %4dx%-4d mlec=%d bytes" % (name, w, h, len(mlec)))

# ---- rebuild the packed atlases with our pixels at the donor's KLNI rects ----
atlas_rects = {}    # atlas key tuple -> list of (x,y,w,h)

# map donor atlas blocks: key -> vidx (scan renditions tree again)
atlases = {}
for i in range(cnt):
    vidx, kidx = struct.unpack_from(">II", leaf, 12 + 8 * i)
    cb = blocks[vidx]; kb = blocks[kidx]
    if cb[:4] != b"ISTC":
        continue
    layout = struct.unpack_from("<H", cb, 36)[0]
    toks = struct.unpack_from("<10H", kb, 0)
    if layout == 0x3EC:
        # key: (scale, idiom, dim1)
        atlases[(toks[2], toks[3], toks[6])] = vidx

for i in range(cnt):
    vidx, kidx = struct.unpack_from(">II", leaf, 12 + 8 * i)
    cb = blocks[vidx]
    if cb[:4] != b"ISTC" or struct.unpack_from("<H", cb, 36)[0] != 0x3EB:
        continue
    w, h = struct.unpack_from("<II", cb, 12)
    tvl = struct.unpack_from("<I", cb, 168)[0]
    tp = 184; klni = None
    while tp + 8 <= 184 + tvl:
        t, l = struct.unpack_from("<II", cb, tp)
        if t == 0x3F2:
            klni = cb[tp + 8:tp + 8 + l]
        tp += 8 + l
    if not klni or klni[:4] != b"KLNI":
        continue
    _, x, y, kw, kh = struct.unpack_from("<IIIII", klni, 4)
    kb2 = klni[28:]
    pairs = {}
    q = 2
    while q + 4 <= len(kb2):
        a, v = struct.unpack_from("<HH", kb2, q)
        if a == 0:
            break
        pairs[a] = v; q += 4
    ak = (pairs.get(12), pairs.get(15), pairs.get(8, 0))
    atlas_rects.setdefault(ak, []).append((x, y, kw, kh))

for ak, rects in sorted(atlas_rects.items()):
    avidx = atlases.get(ak)
    if avidx is None:
        print("  !! atlas %s not found" % (ak,)); continue
    acb = blocks[avidx]
    aw, ah = struct.unpack_from("<II", acb, 12)
    tvl = struct.unpack_from("<I", acb, 168)[0]
    stride = None
    tp = 184
    while tp + 8 <= 184 + tvl:
        t, l = struct.unpack_from("<II", acb, tp)
        if t == 0x3EF:
            stride = struct.unpack_from("<I", acb, tp + 8)[0]
        tp += 8 + l
    stride_px = stride // 4
    buf = bytearray(stride_px * ah * 4)
    ok = True
    for (x, y, w, h) in rects:
        src = by_wh.get((w, h))
        if src is None:
            print("  !! atlas %s rect %dx%d: no source pixels" % (ak, w, h)); ok = False; break
        rb = w * 4
        for r in range(h):
            dst = ((y + r) * stride_px + x) * 4
            buf[dst:dst + rb] = src[r * rb:(r + 1) * rb]
    if not ok:
        continue
    mlec = mkcar.mlec_lzfse(bytes(buf), stride_px, ah)
    new_cb = bytearray(acb[:184 + tvl]) + mlec
    struct.pack_into("<I", new_cb, 0xB4, len(mlec))
    blocks[avidx] = bytes(new_cb)
    replaced += 1
    print("  rebuilt atlas %s (%dx%d, %d rects) mlec=%d" % (ak, aw, ah, len(rects), len(mlec)))

print("replaced %d payloads" % replaced)

# ---- re-serialize: identical slot count/order/free-list, new addresses ----
HEADER = 512
body = bytearray(b"\x00" * HEADER)
new_slots = []
for i in range(nslots):
    a, l = slots[i]
    if i in blocks and (a or l):
        data = blocks[i]
        addr = len(body)
        body += data
        body += b"\x00" * ((-len(body)) % 4)
        new_slots.append((addr, len(data)))
    else:
        new_slots.append((a, l))      # null/free slots verbatim (0,0)
new_voff = len(body)
body += vars_blob
new_ioff = len(body)
index = struct.pack(">I", nslots)
for a, l in new_slots:
    index += struct.pack(">II", a, l)
index += free_tail
body += index
hdr = b"BOMStore" + struct.pack(">IIIIII", ver, nblocks, new_ioff, len(index), new_voff, vlen)
body[0:len(hdr)] = hdr
open(out_path, "wb").write(bytes(body))
print("wrote %s (%d bytes; donor %d)" % (out_path, len(body), len(d)))
