#!/usr/bin/env python3
"""
mkcar.py - write a compiled Asset Catalog (Assets.car) for app icons on
Linux/Windows, with no Apple tools (no actool / CoreUI).

The .car is a big-endian BOM (Bill of Materials) container holding named
blocks (CARHEADER, KEYFORMAT) and B-trees (FACETKEYS, RENDITIONS). Layout +
exact field values reverse-engineered and validated byte-for-byte against
genuine actool output (acextract test fixtures):
  - bomutils (BOM container)            https://github.com/hogliux/bomutils
  - Timac/CARParser Car.h (CAR structs) https://github.com/Timac/CARParser

Usage:
  mkcar.py selftest [out.car]                 # synthetic self-test
  mkcar.py build <AppIcon.appiconset> <out.car>   # from a Flutter/Xcode iconset
"""

import json
import os
import struct
import sys
import zlib

# ---------------------------------------------------------------------------
# BOM container (multi-byte fields big-endian)
# ---------------------------------------------------------------------------

class Bom:
    def __init__(self):
        self.blocks = [b""]          # block 0 is the reserved null block
        self.vars = []               # (name, block_index)

    def add_block(self, data: bytes) -> int:
        self.blocks.append(data)
        return len(self.blocks) - 1

    def add_var(self, name: str, block_index: int):
        self.vars.append((name, block_index))

    def add_tree(self, name: str, pairs):
        indices = []
        for key, value in pairs:
            vidx = self.add_block(value)
            kidx = self.add_block(key)
            indices.append((vidx, kidx))
        # BOMPaths leaf (big-endian): isLeaf(u16) count(u16) forward(u32) backward(u32)
        paths = struct.pack(">HHII", 1, len(indices), 0, 0)
        for vidx, kidx in indices:
            paths += struct.pack(">II", vidx, kidx)
        paths_idx = self.add_block(paths)
        # BOMTree: 'tree' version child blockSize pathCount unknown
        tree = b"tree" + struct.pack(">IIIIB", 1, paths_idx, 4096, len(indices), 0)
        self.add_var(name, self.add_block(tree))

    def serialize(self) -> bytes:
        HEADER = 512
        body = bytearray(b"\x00" * HEADER)
        pointers = [(0, 0)]
        for data in self.blocks[1:]:
            addr = len(body)
            body += data
            body += b"\x00" * ((-len(body)) % 4)
            pointers.append((addr, len(data)))
        index_offset = len(body)
        index = struct.pack(">I", len(pointers))
        for addr, length in pointers:
            index += struct.pack(">II", addr, length)
        index += struct.pack(">I", 0)  # free-list count
        body += index
        vars_offset = len(body)
        v = struct.pack(">I", len(self.vars))
        for name, idx in self.vars:
            nb = name.encode("ascii")
            v += struct.pack(">IB", idx, len(nb)) + nb
        body += v
        header = b"BOMStore" + struct.pack(
            ">IIIIII", 1, len(pointers), index_offset, len(index), vars_offset, len(v))
        body[0:len(header)] = header
        return bytes(body)


# ---------------------------------------------------------------------------
# CAR structures (Car.h). CAR tags are byte-reversed in-file ('CTAR'->"RATC").
# ---------------------------------------------------------------------------

def fourcc(s: str) -> int:
    return struct.unpack(">I", s.encode("ascii"))[0]

def carheader(rendition_count: int) -> bytes:
    return struct.pack(
        "<IIIII128s256s16sIIII",
        fourcc("CTAR"), 374, 11, 0, rendition_count,
        b"@(#)PROGRESS:73", b"IBCocoaTouchImageCatalogTool-mkcar",
        b"\x00" * 16, 0, 0, 0, 0)

# RenditionAttributeType ids
A_ELEMENT, A_PART, A_SIZE, A_DIRECTION, A_VALUE = 1, 2, 3, 4, 6
A_DIMENSION1, A_DIMENSION2, A_STATE, A_SCALE = 8, 9, 10, 12
A_IDIOM, A_SUBTYPE, A_IDENTIFIER = 15, 16, 17
A_HSIZECLASS, A_VSIZECLASS, A_MEMCLASS, A_GFXCLASS = 20, 21, 22, 23

# actool's exact 13-token KEYFORMAT order (validated against reference)
KEY_TOKENS = [A_SCALE, A_IDIOM, A_SUBTYPE, A_GFXCLASS, A_MEMCLASS,
              A_HSIZECLASS, A_VSIZECLASS, A_IDENTIFIER, A_ELEMENT,
              A_PART, A_STATE, A_VALUE, A_DIMENSION1]

# Asset-catalog idiom string -> CoreUI idiom value (validated: phone=1, universal=0)
IDIOM = {"universal": 0, "iphone": 1, "ipad": 2, "tv": 3, "watch": 4,
         "car": 5, "mac": 6, "ios-marketing": 0}

def keyformat() -> bytes:
    return struct.pack("<III", fourcc("kfmt"), 0, len(KEY_TOKENS)) + \
        b"".join(struct.pack("<I", t) for t in KEY_TOKENS)

def rendition_key(attrs: dict) -> bytes:
    return b"".join(struct.pack("<H", attrs.get(t, 0)) for t in KEY_TOKENS)

def facet_value(identifier: int) -> bytes:
    attrs = [(A_IDENTIFIER, identifier)]
    out = struct.pack("<HHH", 0, 0, len(attrs))
    for name, value in attrs:
        out += struct.pack("<HH", name, value)
    return out

def csiheader(width, height, scale_factor, name, payload_len, pixel_format="ARGB"):
    flags = 0x8  # isOpaque (icons have no alpha)
    csimetadata = struct.pack("<IHH128s", 0, 0x3F2, 0, name.encode("ascii")[:128])
    csibitmaplist = struct.pack("<IIII", 0, 0, 0, payload_len)
    return (struct.pack("<IIIIIII", fourcc("CTSI"), 1, flags, width, height,
                        scale_factor, fourcc(pixel_format))
            + struct.pack("<I", 0) + csimetadata + csibitmaplist)

def celm_uncompressed(bgra: bytes) -> bytes:
    return struct.pack("<IIII", fourcc("CELM"), 1, 0, len(bgra)) + bgra


# ---------------------------------------------------------------------------
# Minimal PNG decoder -> premultiplied BGRA (8-bit truecolor / truecolor-alpha)
# ---------------------------------------------------------------------------

def _paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    return b if pb <= pc else c

def decode_png(data: bytes):
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos = 8
    width = height = bit_depth = color_type = None
    idat = bytearray()
    plte = None
    while pos < len(data):
        length, = struct.unpack(">I", data[pos:pos + 4])
        ctype = data[pos + 4:pos + 8]
        chunk = data[pos + 8:pos + 8 + length]
        pos += 12 + length
        if ctype == b"IHDR":
            width, height, bit_depth, color_type = struct.unpack(">IIBB", chunk[:10])
        elif ctype == b"PLTE":
            plte = chunk
        elif ctype == b"IDAT":
            idat += chunk
        elif ctype == b"IEND":
            break
    if bit_depth != 8 or color_type not in (2, 6, 3):
        raise ValueError(f"unsupported PNG (bit_depth={bit_depth}, color_type={color_type}); "
                         "need 8-bit RGB, RGBA or palette")
    channels = {2: 3, 6: 4, 3: 1}[color_type]
    raw = zlib.decompress(bytes(idat))
    stride = width * channels
    out = bytearray()
    prev = bytearray(stride)
    p = 0
    for _ in range(height):
        ftype = raw[p]; p += 1
        line = bytearray(raw[p:p + stride]); p += stride
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = prev[i]
            c = prev[i - channels] if i >= channels else 0
            if ftype == 1:
                line[i] = (line[i] + a) & 0xFF
            elif ftype == 2:
                line[i] = (line[i] + b) & 0xFF
            elif ftype == 3:
                line[i] = (line[i] + ((a + b) >> 1)) & 0xFF
            elif ftype == 4:
                line[i] = (line[i] + _paeth(a, b, c)) & 0xFF
        # -> opaque premultiplied BGRA (icons must be opaque)
        if color_type == 3:
            for x in range(width):
                idx = line[x] * 3
                r, g, b_ = plte[idx], plte[idx + 1], plte[idx + 2]
                out += bytes((b_, g, r, 0xFF))
        else:
            for x in range(width):
                r = line[x * channels]
                g = line[x * channels + 1]
                b_ = line[x * channels + 2]
                out += bytes((b_, g, r, 0xFF))
        prev = line
    return width, height, bytes(out)


# ---------------------------------------------------------------------------
# Assemble a .car
# ---------------------------------------------------------------------------

def build_car(renditions) -> bytes:
    bom = Bom()
    bom.add_var("CARHEADER", bom.add_block(carheader(len(renditions))))
    bom.add_var("KEYFORMAT", bom.add_block(keyformat()))
    identifier = 0x4D2
    bom.add_tree("FACETKEYS", [(b"AppIcon", facet_value(identifier))])
    rend_pairs = []
    for r in renditions:
        attrs = {
            A_IDENTIFIER: identifier,
            A_SCALE: r["scale"],
            A_IDIOM: r["idiom"],
            A_SUBTYPE: r.get("subtype", 0),
            A_DIMENSION1: r.get("point", 0),
        }
        key = rendition_key(attrs)
        celm = celm_uncompressed(r["bgra"])
        value = csiheader(r["width"], r["height"], r["scale"] * 100, "AppIcon", len(celm)) + celm
        rend_pairs.append((key, value))
    bom.add_tree("RENDITIONS", rend_pairs)
    return bom.serialize()


def load_appiconset(path: str):
    """Parse AppIcon.appiconset/Contents.json + PNGs into renditions."""
    contents = json.load(open(os.path.join(path, "Contents.json")))
    renditions = []
    for img in contents.get("images", []):
        fn = img.get("filename")
        if not fn:
            continue
        png = os.path.join(path, fn)
        if not os.path.exists(png):
            continue
        w, h, bgra = decode_png(open(png, "rb").read())
        scale = int(img.get("scale", "1x").rstrip("x"))
        idiom = IDIOM.get(img.get("idiom", "universal"), 0)
        # point size from "60x60" -> 60 (used to disambiguate same scale/idiom)
        size_str = img.get("size", f"{w}x{h}")
        try:
            point = int(float(size_str.split("x")[0]))
        except ValueError:
            point = w
        renditions.append({"width": w, "height": h, "scale": scale,
                           "idiom": idiom, "subtype": 0, "point": point, "bgra": bgra})
    if not renditions:
        raise SystemExit("no icon images found in " + path)
    return renditions


# ---------------------------------------------------------------------------
# Self-test reader
# ---------------------------------------------------------------------------

def read_bom(data: bytes):
    assert data[:8] == b"BOMStore", "bad magic"
    _, nblocks, ioff, ilen, voff, vlen = struct.unpack(">IIIIII", data[8:32])
    count = struct.unpack(">I", data[ioff:ioff + 4])[0]
    ptrs, p = [], ioff + 4
    for _ in range(count):
        a, l = struct.unpack(">II", data[p:p + 8]); p += 8; ptrs.append((a, l))
    vcount, p = struct.unpack(">I", data[voff:voff + 4])[0], voff + 4
    vars_ = {}
    for _ in range(vcount):
        idx, nlen = struct.unpack(">IB", data[p:p + 5]); p += 5
        vars_[data[p:p + nlen].decode("ascii")] = idx; p += nlen
    return vars_, ptrs


def selftest(out):
    px = lambda w, h: b"\xff\xff\xff\xff" * (w * h)
    rends = [
        {"width": 120, "height": 120, "scale": 2, "idiom": 1, "point": 60, "bgra": px(120, 120)},
        {"width": 180, "height": 180, "scale": 3, "idiom": 1, "point": 60, "bgra": px(180, 180)},
        {"width": 1024, "height": 1024, "scale": 1, "idiom": 0, "point": 1024, "bgra": px(1024, 1024)},
    ]
    data = build_car(rends)
    vars_, ptrs = read_bom(data)
    print(f"BOM ok: {len(ptrs)} blocks, vars={sorted(vars_)}")
    open(out, "wb").write(data)
    print(f"wrote {out} ({len(data)} bytes)")


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "selftest":
        selftest(sys.argv[2] if len(sys.argv) > 2 else "/tmp/mkcar_selftest.car")
    elif len(sys.argv) == 4 and sys.argv[1] == "build":
        rends = load_appiconset(sys.argv[2])
        data = build_car(rends)
        open(sys.argv[3], "wb").write(data)
        print(f"Assets.car: {len(rends)} renditions, {len(data)} bytes -> {sys.argv[3]}")
    else:
        print("usage: mkcar.py selftest [out.car] | build <AppIcon.appiconset> <out.car>",
              file=sys.stderr)
        sys.exit(2)
