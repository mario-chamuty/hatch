#!/usr/bin/env python3
"""
mkcar.py - write a compiled Asset Catalog (Assets.car) for app icons on
Linux/Windows, with no Apple tools (no actool / CoreUI).

The .car is a big-endian BOM (Bill of Materials) container holding named
blocks (CARHEADER, KEYFORMAT) and B-trees (FACETKEYS, RENDITIONS). The exact
field values, key ordering and rendition encoding below were reverse-engineered
byte-for-byte from genuine actool output (UTM-Remote.app/Assets.car, CoreUI-374)
and cross-checked against:
  - bomutils (BOM container)            https://github.com/hogliux/bomutils
  - Timac/CARParser Car.h (CAR structs) https://github.com/Timac/CARParser

Modern App Store ingestion parses each rendition with CoreUI, so the image
payload must match actool's real format: a CTSI (csiheader) + a TLV info list
+ an MLEC (CELM) payload whose pixels are LZFSE-compressed and wrapped in
row-chunked "KCBC" blocks. An uncompressed CELM or a hand-guessed BOM key
scheme is rejected with ITMS-90596 ("asset catalog can't be processed").

A modern "single size" app icon is one 1024x1024 source; the store generates
the 120/180/... derivatives server-side. We emit, per idiom (iphone, ipad):
  - an AppIcon multi-size descriptor rendition (layout 0x3F2, an "SISM" table)
  - the 1024x1024 "Icon.png" image rendition (layout 0x0C, LZFSE)
plus a single "AppIcon" facet that resolves CFBundleIconName -> the renditions.

Usage:
  mkcar.py selftest [out.car]                      # synthetic self-test
  mkcar.py build <AppIcon.appiconset> <out.car>    # from a Flutter/Xcode iconset
"""

import json
import os
import struct
import subprocess
import sys
import tempfile
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
        # CoreUI walks the tree as a sorted B-tree; keep keys in memcmp order.
        pairs = sorted(pairs, key=lambda kv: kv[0])
        indices = []
        for key, value in pairs:
            vidx = self.add_block(value)
            kidx = self.add_block(key)
            indices.append((vidx, kidx))
        self._emit_tree(name, indices)

    def add_inline_tree(self, name: str, pairs):
        """A tree whose leaf key is an inline uint32 rather than a key block
        (CoreUI uses this for BITMAPKEYS: entry = (valueBlockIdx, identifier))."""
        pairs = sorted(pairs, key=lambda kv: kv[0])
        indices = []
        for ident, value in pairs:
            vidx = self.add_block(value)
            indices.append((vidx, ident))   # second slot is the literal id, not a block
        self._emit_tree(name, indices)

    BLOCK_SIZE = 4096

    def _emit_tree(self, name: str, indices):
        # BOMPaths leaf (big-endian): isLeaf(u16) count(u16) forward(u32) backward(u32)
        paths = struct.pack(">HHII", 1, len(indices), 0, 0)
        for vidx, second in indices:
            paths += struct.pack(">II", vidx, second)
        # Genuine BOM leaf nodes are allocated at the tree's blockSize and
        # zero-padded - CoreUI reads each node as a fixed blockSize block. A
        # tightly-sized node parses fine in lenient decoders (iineva) but is
        # rejected by CoreUI ingestion (ITMS-90596). Pad to >= blockSize.
        if len(paths) < self.BLOCK_SIZE:
            paths += b"\x00" * (self.BLOCK_SIZE - len(paths))
        paths_idx = self.add_block(paths)
        # BOMTree: 'tree' version child blockSize pathCount unknown(u8) + the two
        # trailing fields (0xffffffff, 0) real CoreUI trees carry (bomutils' 21-
        # byte struct omits them; CoreUI reads them, so a 21-byte header makes it
        # read past the block into garbage). Verified vs a real catalog (29 B).
        tree = (b"tree"
                + struct.pack(">IIIIB", 1, paths_idx, self.BLOCK_SIZE, len(indices), 0)
                + struct.pack(">II", 0xFFFFFFFF, 0))
        self.add_var(name, self.add_block(tree))

    def serialize(self) -> bytes:
        HEADER = 512
        body = bytearray(b"\x00" * HEADER)
        pointers = [(0, 0)]                      # entry 0 is always the null block
        for data in self.blocks[1:]:
            addr = len(body)
            body += data
            body += b"\x00" * ((-len(body)) % 4)
            pointers.append((addr, len(data)))
        # The BOM vars table comes BEFORE the block-index table.
        vars_offset = len(body)
        v = struct.pack(">I", len(self.vars))
        for name, idx in self.vars:
            nb = name.encode("ascii")
            v += struct.pack(">IB", idx, len(nb)) + nb
        body += v
        # The block-index table MUST be last: the BOM invariant is
        # indexOffset + indexLength == file length (libbom relies on it).
        index_offset = len(body)
        index = struct.pack(">I", len(pointers))     # numberOfBlockTablePointers (incl. null)
        for addr, length in pointers:
            index += struct.pack(">II", addr, length)
        index += struct.pack(">I", 0)                # trailing free-list count
        body += index
        # header.numberOfBlocks counts NON-null blocks only (excludes entry 0).
        non_null = sum(1 for _, length in pointers if length != 0)
        header = b"BOMStore" + struct.pack(
            ">IIIIII", 1, non_null, index_offset, len(index), vars_offset, len(v))
        body[0:len(header)] = header
        return bytes(body)


# ---------------------------------------------------------------------------
# CAR structures (Car.h). CAR tags are byte-reversed in-file ('CTSI'->"ISTC").
# fourcc() reads big-endian; packing with "<I" writes the reversed bytes.
# ---------------------------------------------------------------------------

def fourcc(s: str) -> int:
    return struct.unpack(">I", s.encode("ascii"))[0]

def carheader(rendition_count: int) -> bytes:
    # coreuiVersion / storageVersion MUST be current: App Store ingestion
    # rejects a stale catalog ("rebuild with the latest GM Xcode", ITMS-90596).
    # A genuine Xcode 26 catalog reports coreuiVersion 970 / storageVersion 17
    # with these exact version strings and a zero UUID (validated vs real CAR).
    # Tail schemaVersion / colorSpaceID / keySemantics MUST be (2,1,2).
    uuid = b"\x00" * 16
    return struct.pack(
        "<IIIII128s256s16sIIII",
        fourcc("CTAR"), 970, 17, 0, rendition_count,
        b"@(#)PROGRAM:CoreUI  PROJECT:CoreUI-970.1",
        b"Xcode 26.0 (17A321) via AssetCatalogSimulatorAgent",
        uuid, 0, 2, 1, 2)

def extended_metadata() -> bytes:
    """EXTENDED_METADATA 'META' block (1028 bytes). Every real CAR carries it;
    its absence makes CoreUI reject the catalog. Tag 'META' is stored literally
    (not byte-reversed). Values mirror a genuine Xcode 26 catalog."""
    def s(v, n):
        return v.encode("ascii")[:n].ljust(n, b"\x00")
    return (b"META"
            + s("", 256)                              # thinningArguments
            + s("14.0", 256)                          # deploymentPlatformVersion
            + s("ios", 256)                           # deploymentPlatform
            + s("@(#)PROGRAM:CoreThemeDefinition  PROJECT:CoreThemeDefinition-652"
                "  [IIO-2773.0.1.2]", 256))           # authoringTool

# APPEARANCEKEYS maps appearance names -> ids; the KEYFORMAT's ThemeAppearance
# token (7) resolves against this. We declare ONLY the appearances actually used
# by renditions - every hatch icon rendition is appearance 0 (UIAppearanceAny),
# matching a real simple actool catalog (iineva). Declaring unused Dark/Light/
# Tintable keys diverges from real output and is a candidate ITMS-90596 cause.
APPEARANCE_KEYS = [
    (b"UIAppearanceAny", 0x00),
]

# BITMAPKEYS: per-identifier 56-byte bitmap descriptor, keyed by the inline
# rendition identifier. Verbatim from a genuine catalog for the AppIcon id.
BITMAPKEYS_DESCRIPTOR = bytes.fromhex(
    "01000000000000002c0000000a000000ffffffff010000000e000000"
    "0600000001000100ff03000001000000ffffffffffffffffffffffff")

# RenditionAttributeType ids (Car.h)
A_ELEMENT, A_PART, A_SIZE, A_DIRECTION, A_VALUE = 1, 2, 3, 4, 6
A_APPEARANCE, A_DIMENSION1, A_DIMENSION2, A_STATE = 7, 8, 9, 10
A_SCALE, A_UNKNOWN13, A_IDIOM, A_SUBTYPE, A_IDENTIFIER = 12, 13, 15, 16, 17

# actool's exact 10-token KEYFORMAT order (validated against reference CAR).
KEY_TOKENS = [A_APPEARANCE, A_UNKNOWN13, A_SCALE, A_IDIOM, A_SUBTYPE,
              A_DIMENSION2, A_DIMENSION1, A_IDENTIFIER, A_ELEMENT, A_PART]

# Asset-catalog idiom string -> CoreUI idiom value
IDIOM = {"universal": 0, "iphone": 1, "ipad": 2, "tv": 3, "watch": 4,
         "car": 5, "mac": 6, "ios-marketing": 0}

# Identity triple actool assigns to the "AppIcon" facet (any consistent values
# work; we reuse the reference's so the catalog matches real output exactly).
APPICON_ELEMENT = 85
APPICON_PART_IMAGE = 220   # the image renditions (Icon.png)
APPICON_PART_META = 218    # the multi-size descriptor rendition (0x3F2)
APPICON_IDENTIFIER = 6849
DIM2_1024 = 9              # dimension2 slot index for the 1024 marketing size
# Subtype actool stamps on the iPhone primary (home-screen, 60pt) icon variant.
IPHONE_PRIMARY_SUBTYPE = 1792   # 0x700

# Packed-atlas ("ZZZZPackedAsset") element/part. Genuine actool routes the
# per-size app-icon bitmaps through shared 0x3EC atlas images keyed by
# (Scale, Idiom[, Dimension1]) with Element=9 Part=181 and NO identifier; each
# logical size is a 0x3EB InternalReference (KLNI) into that atlas. Emitting
# every size as a bare 0x0C direct image (which CoreUI never produces) is the
# ITMS-90596 cause - confirmed against a real Flutter catalog (Kazumi, CoreUI
# 918) where the primary 120/180 sizes are atlas refs, not direct images.
A_PACKED_ELEMENT = 9
A_PACKED_PART = 181
PACK_BORDER = 2                 # actool's 2px gutter/border around packed icons
MARKETING_IDIOM = 6            # idiom actool stamps on the 1024 marketing icon
LEGACY_POINTS = {50, 57, 72}  # pre-iOS7 sizes modern actool drops

def keyformat() -> bytes:
    return struct.pack("<III", fourcc("kfmt"), 0, len(KEY_TOKENS)) + \
        b"".join(struct.pack("<I", t) for t in KEY_TOKENS)

def rendition_key(attrs) -> bytes:
    return b"".join(struct.pack("<H", attrs.get(t, 0)) for t in KEY_TOKENS)

def facet_value(element, part, identifier) -> bytes:
    attrs = [(A_ELEMENT, element), (A_PART, part), (A_IDENTIFIER, identifier)]
    out = struct.pack("<HHH", 0, 0, len(attrs))
    for name, value in attrs:
        out += struct.pack("<HH", name, value)
    return out

def csiheader(width, height, scale_factor, name, layout, tvl_len,
              payload_len, pixel_format=0, color_space=1, flags=0) -> bytes:
    # csimetadata: modtime(0) layout(u16) zero(u16) name[128]
    csimetadata = struct.pack("<IHH128s", 0, layout, 0,
                              name.encode("ascii")[:128])
    # csibitmaplist: tvlLength, unknown(=1), zero, renditionLength
    csibitmaplist = struct.pack("<IIII", tvl_len, 1, 0, payload_len)
    pf = fourcc(pixel_format) if isinstance(pixel_format, str) else pixel_format
    return (struct.pack("<IIIIIII", fourcc("CTSI"), 1, flags, width, height,
                        scale_factor, pf)
            + struct.pack("<I", color_space)          # colorSpaceID:4 (1 = sRGB)
            + csimetadata + csibitmaplist)


# ---------------------------------------------------------------------------
# TLV info lists (verbatim from actool, with row-stride patched per image)
# ---------------------------------------------------------------------------

def _tlv(tag, value):
    return struct.pack("<II", tag, len(value)) + value

def image_tvl(width, height) -> bytes:
    """104-byte TLV info list preceding a layout-0x0C image rendition. These
    entries are PER-RENDITION: 0x3E9 and 0x3EB embed the bitmap's width/height,
    0x3EF is the row stride. (CoreUI validates them against the rendition;
    leniency-only decoders skip the TLV, which is why a verbatim copy passed
    local checks but failed ingestion.)"""
    return (
        _tlv(0x3E9, struct.pack("<IIIII", 1, 0, 0, width, height)) +
        _tlv(0x3EB, struct.pack("<IIIIIII", 1, 0, 0, 0, 0, width, height)) +
        _tlv(0x3EC, struct.pack("<ff", 0.0, 1.0)) +
        _tlv(0x3EE, struct.pack("<I", 1)) +
        _tlv(0x3EF, struct.pack("<I", width * 4))   # bytes per row
    )

def meta_tvl() -> bytes:
    """28-byte TLV info list preceding the 0x3F2 multi-size descriptor."""
    return (
        _tlv(0x3EC, bytes.fromhex("0000000000000000")) +
        _tlv(0x3EE, struct.pack("<I", 1))
    )

def atlas_tvl(stride_bytes) -> bytes:
    """TLV preceding a 0x3EC ZZZZPackedAsset atlas. Unlike an image TLV the
    0x3E9 entry carries zero width/height (the atlas is addressed by KLNI
    sub-rects, not as a single image) and there is no 0x3EB entry; 0x3EF is the
    padded row stride in bytes."""
    return (
        _tlv(0x3E9, struct.pack("<IIIII", 1, 0, 0, 0, 0)) +
        _tlv(0x3EC, struct.pack("<ff", 0.0, 1.0)) +
        _tlv(0x3EE, struct.pack("<I", 1)) +
        _tlv(0x3EF, struct.pack("<I", stride_bytes))
    )

def klni(x, y, w, h, atlas_attrs) -> bytes:
    """0x3F2 'KLNI' (internal-link) descriptor inside a 0x3EB reference: the
    icon's pixels live at (x, y, w, h) in the atlas identified by atlas_attrs
    (a list of (attribute_id, value) pairs, ASCENDING by id, for the atlas's
    non-zero key tokens: Element=9, Part=181, [Dimension1], Scale, Idiom).
    Byte layout reverse-engineered from a real catalog: a leading u16(0), the
    (attr,val) pairs, then a u16(0) terminator form the key blob; the blob is
    framed by u16(12) u16(keyLen) ... and a trailing u16(0) pad."""
    blob = (struct.pack("<H", 0)
            + b"".join(struct.pack("<HH", a, v) for a, v in atlas_attrs)
            + struct.pack("<H", 0))
    return (b"KLNI" + struct.pack("<IIIII", 0, x, y, w, h)
            + struct.pack("<HH", 12, len(blob)) + blob + struct.pack("<H", 0))

def ref_tvl(width, height, klni_blob) -> bytes:
    """TLV preceding a 0x3EB InternalReference (rendition has no payload). Same
    0x3E9/0x3EB image entries as a direct image, plus the 0x3F2 KLNI link; no
    0x3EF stride (the reference owns no pixels)."""
    return (
        _tlv(0x3E9, struct.pack("<IIIII", 1, 0, 0, width, height)) +
        _tlv(0x3EB, struct.pack("<IIIIIII", 1, 0, 0, 0, 0, width, height)) +
        _tlv(0x3F2, klni_blob) +
        _tlv(0x3EC, struct.pack("<ff", 0.0, 1.0)) +
        _tlv(0x3EE, struct.pack("<I", 1))
    )


# ---------------------------------------------------------------------------
# Rendition payloads
# ---------------------------------------------------------------------------

def lzfse_compress(data: bytes) -> bytes:
    with tempfile.NamedTemporaryFile(delete=False) as ti:
        ti.write(data); src = ti.name
    dst = src + ".lz"
    try:
        subprocess.run(["lzfse", "-encode", "-i", src, "-o", dst],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        return open(dst, "rb").read()
    finally:
        for f in (src, dst):
            try:
                os.remove(f)
            except OSError:
                pass

def mlec_lzfse(bgra: bytes, width: int, height: int) -> bytes:
    """MLEC (CELM) payload: row-chunked, LZFSE-compressed KCBC blocks.

    actool caps each chunk at ~1.4 MB of uncompressed pixels, so for a 1024px
    image that is 341 rows/chunk (341*1024*4 = 1396736). Each chunk is an
    independent LZFSE stream wrapped as: 'KCBC' u32(0) u32(0) u32(rows) u32(len).
    """
    bytes_per_row = width * 4
    rows_per_chunk = max(1, 1396736 // bytes_per_row)
    blocks = b""
    nchunks = 0
    row = 0
    while row < height:
        n = min(rows_per_chunk, height - row)
        chunk = bgra[row * bytes_per_row:(row + n) * bytes_per_row]
        comp = lzfse_compress(chunk)
        blocks += b"KCBC" + struct.pack("<IIII", 0, 0, n, len(comp)) + comp
        row += n
        nchunks += 1
    # MLEC header: tag, version(3), compressionType(4 = LZFSE), chunkCount
    return struct.pack("<IIII", fourcc("CELM"), 3, 4, nchunks) + blocks

def msis_payload(sizes) -> bytes:
    """0x3F2 multi-size descriptor: 'SISM' u32(ver=1) u32(count) [w,h,dim2]*.
    Like other CAR tags it is byte-reversed in-file, so the logical fourcc is
    'MSIS' (which fourcc()+'<I' writes as the bytes 'SISM')."""
    out = struct.pack("<III", fourcc("MSIS"), 1, len(sizes))
    for w, h, dim2 in sizes:
        out += struct.pack("<III", w, h, dim2)
    return out


# ---------------------------------------------------------------------------
# Minimal PNG decoder -> opaque BGRA (8-bit truecolor / truecolor-alpha / palette)
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
        # -> opaque BGRA (the App Store 1024 icon must not carry alpha)
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


def _nearest_resize_bgra(bgra, sw, sh, dw, dh):
    """Nearest-neighbour resize of a BGRA buffer (only used if no 1024 source)."""
    out = bytearray(dw * dh * 4)
    for y in range(dh):
        sy = y * sh // dh
        srow = sy * sw * 4
        drow = y * dw * 4
        for x in range(dw):
            sx = x * sw // dw
            si = srow + sx * 4
            di = drow + x * 4
            out[di:di + 4] = bgra[si:si + 4]
    return bytes(out)


# ---------------------------------------------------------------------------
# Assemble a .car (single-size 1024 app icon)
# ---------------------------------------------------------------------------

# Idioms whose per-size icons are packed into atlases (iPhone, iPad). The 1024
# marketing icon is handled separately under MARKETING_IDIOM.
PACKED_IDIOMS = [(IDIOM["iphone"], "iphone"), (IDIOM["ipad"], "ipad")]


def _direct_image(point_or_none, scale, idiom_val, dim2, im, subtype=0):
    """Build a 0x0C direct-image rendition (key, blob) for an icon dict."""
    attrs = {A_SCALE: scale, A_IDIOM: idiom_val, A_DIMENSION2: dim2,
             A_IDENTIFIER: APPICON_IDENTIFIER, A_ELEMENT: APPICON_ELEMENT,
             A_PART: APPICON_PART_IMAGE}
    if subtype:
        attrs[A_SUBTYPE] = subtype
    key = rendition_key(attrs)
    mlec = mlec_lzfse(im["bgra"], im["width"], im["height"])
    tvl = image_tvl(im["width"], im["height"])
    hdr = csiheader(im["width"], im["height"], scale * 100, im["name"], 0x0C,
                    len(tvl), len(mlec), pixel_format="ARGB", color_space=1)
    return key, hdr + tvl + mlec


def _descriptor(idiom_val, sizes, tvl_meta, subtype=0):
    """Build a 0x3F2 AppIcon multi-size descriptor (key, blob). sizes is a list
    of (point, dim2); SISM lists (point, point, dim2)."""
    sism = msis_payload([(int(p), int(p), d) for p, d in sizes])
    hdr = csiheader(0, 0, 0, "AppIcon", 0x3F2, len(tvl_meta), len(sism),
                    pixel_format=0, color_space=0)
    attrs = {A_SCALE: 1, A_IDIOM: idiom_val, A_IDENTIFIER: APPICON_IDENTIFIER,
             A_ELEMENT: APPICON_ELEMENT, A_PART: APPICON_PART_META}
    if subtype:
        attrs[A_SUBTYPE] = subtype
    return rendition_key(attrs), hdr + tvl_meta + sism


def _atlas(scale, idiom_val, group, dim2):
    """Pack a same-(scale,idiom) icon group into one 0x3EC ZZZZPackedAsset atlas
    and emit it plus a 0x3EB InternalReference per icon. Single-row shelf pack
    with a 2px gutter; the atlas row stride is padded to a multiple of 8 px
    (matching actool's 32-byte row alignment). Returns a list of (key, blob)."""
    placements = []                       # (im, px, py)
    x = PACK_BORDER
    max_h = max(im["height"] for im in group)
    for im in group:
        placements.append((im, x, PACK_BORDER))
        x += im["width"] + PACK_BORDER
    content_w, content_h = x, PACK_BORDER + max_h + PACK_BORDER
    stride_px = (content_w + 7) // 8 * 8
    buf = bytearray(stride_px * content_h * 4)
    for im, px, py in placements:
        row_bytes = im["width"] * 4
        for r in range(im["height"]):
            dst = ((py + r) * stride_px + px) * 4
            buf[dst:dst + row_bytes] = im["bgra"][r * row_bytes:(r + 1) * row_bytes]

    out = []
    atlas_attrs = [(A_ELEMENT, A_PACKED_ELEMENT), (A_PART, A_PACKED_PART),
                   (A_SCALE, scale), (A_IDIOM, idiom_val)]
    mlec = mlec_lzfse(bytes(buf), stride_px, content_h)
    atvl = atlas_tvl(stride_px * 4)
    ahdr = csiheader(content_w, content_h, scale * 100,
                     "ZZZZPackedAsset-%d.1.0-gamut0" % scale, 0x3EC,
                     len(atvl), len(mlec), pixel_format="ARGB", color_space=1)
    atlas_key = rendition_key({A_SCALE: scale, A_IDIOM: idiom_val,
                               A_ELEMENT: A_PACKED_ELEMENT,
                               A_PART: A_PACKED_PART})
    out.append((atlas_key, ahdr + atvl + mlec))
    for im, px, py in placements:
        kblob = klni(px, py, im["width"], im["height"], atlas_attrs)
        rtvl = ref_tvl(im["width"], im["height"], kblob)
        rhdr = csiheader(im["width"], im["height"], im["scale"] * 100,
                         im["name"], 0x3EB, len(rtvl), 0,
                         pixel_format="ARGB", color_space=1)
        rkey = rendition_key({A_SCALE: im["scale"], A_IDIOM: idiom_val,
                              A_DIMENSION2: dim2[im["point"]],
                              A_IDENTIFIER: APPICON_IDENTIFIER,
                              A_ELEMENT: APPICON_ELEMENT,
                              A_PART: APPICON_PART_IMAGE})
        out.append((rkey, rhdr + rtvl))
    return out


def build_car(images) -> bytes:
    """images: list of {idiom_str, scale, point, width, height, bgra, name}.
    Emits a flat app-icon catalog in the structure genuine actool produces:
      * per idiom (iPhone, iPad): a 0x3F2 multi-size descriptor; same-scale
        sizes packed into a 0x3EC ZZZZPackedAsset atlas referenced by 0x3EB
        KLNI links (a singleton scale group stays a 0x0C direct image, as
        actool does);
      * the 1024 marketing icon as a 0x0C image under idiom 6 + its descriptor;
      * the iPhone primary (60pt@3x -> 90pt@2x, Subtype=1792) as a 0x0C image +
        its descriptor.
    Dimension2 is a global per-point-size slot, consistent across descriptors,
    references and direct images. Legacy (pre-iOS7) 50/57/72pt sizes are dropped
    to match modern actool output."""
    imgs = [im for im in images if round(im["point"]) not in LEGACY_POINTS]
    size_pts = sorted({im["point"] for im in imgs
                       if im["idiom_str"] != "ios-marketing"})
    dim2 = {p: i + 1 for i, p in enumerate(size_pts)}
    dim2[90.0] = len(size_pts) + 1            # iPhone-primary subtype slot
    dim2[1024.0] = len(size_pts) + 2          # marketing slot
    tvl_meta = meta_tvl()
    bom = Bom()
    rend_pairs = []

    for idiom_val, idiom_name in PACKED_IDIOMS:
        ims = [im for im in imgs if im["idiom_str"] == idiom_name]
        if not ims:
            continue
        pts = sorted({im["point"] for im in ims})
        rend_pairs.append(_descriptor(idiom_val,
                                      [(p, dim2[p]) for p in pts], tvl_meta))
        for scale in sorted({im["scale"] for im in ims}):
            group = [im for im in ims if im["scale"] == scale]
            if len(group) == 1:               # actool emits singletons direct
                im = group[0]
                rend_pairs.append(_direct_image(im["point"], scale, idiom_val,
                                                dim2[im["point"]], im))
            else:
                rend_pairs.extend(_atlas(scale, idiom_val, group, dim2))

    marketing = next((im for im in imgs
                      if im["idiom_str"] == "ios-marketing"), None)
    if marketing is not None:
        rend_pairs.append(_descriptor(MARKETING_IDIOM,
                                      [(1024.0, dim2[1024.0])], tvl_meta))
        rend_pairs.append(_direct_image(1024.0, 1, MARKETING_IDIOM,
                                        dim2[1024.0], marketing))

    primary = next((im for im in imgs if im["idiom_str"] == "iphone"
                    and round(im["point"]) == 60 and im["scale"] == 3), None)
    if primary is not None:
        rend_pairs.append(_descriptor(IDIOM["iphone"], [(90.0, dim2[90.0])],
                                      tvl_meta, subtype=IPHONE_PRIMARY_SUBTYPE))
        rend_pairs.append(_direct_image(90.0, 2, IDIOM["iphone"], dim2[90.0],
                                        primary, subtype=IPHONE_PRIMARY_SUBTYPE))

    bom.add_var("CARHEADER", bom.add_block(carheader(len(rend_pairs))))
    bom.add_var("KEYFORMAT", bom.add_block(keyformat()))
    bom.add_tree("FACETKEYS", [(b"AppIcon",
                                facet_value(APPICON_ELEMENT, APPICON_PART_IMAGE,
                                            APPICON_IDENTIFIER))])
    bom.add_tree("APPEARANCEKEYS",
                 [(name, struct.pack("<H", v)) for name, v in APPEARANCE_KEYS])
    bom.add_var("EXTENDED_METADATA", bom.add_block(extended_metadata()))
    bom.add_inline_tree("BITMAPKEYS", [(APPICON_IDENTIFIER, BITMAPKEYS_DESCRIPTOR)])
    bom.add_tree("RENDITIONS", rend_pairs)
    return bom.serialize()


def load_appiconset(path: str):
    """Parse AppIcon.appiconset/Contents.json into a list of icon images."""
    contents = json.load(open(os.path.join(path, "Contents.json")))
    images = []
    for img in contents.get("images", []):
        fn = img.get("filename")
        if not fn:
            continue
        png = os.path.join(path, fn)
        if not os.path.exists(png):
            continue
        w, h, bgra = decode_png(open(png, "rb").read())
        scale = int(img.get("scale", "1x").rstrip("x"))
        size_str = img.get("size", "%gx%g" % (w, w))
        try:
            point = float(size_str.split("x")[0])
        except ValueError:
            point = w / scale
        images.append({"idiom_str": img.get("idiom", "universal"),
                       "scale": scale, "point": point,
                       "width": w, "height": h, "bgra": bgra,
                       "name": os.path.splitext(fn)[0] + ".png"})
    if not images:
        raise SystemExit("no icon images found in " + path)
    return images


# ---------------------------------------------------------------------------
# Self-test reader / round-trip validator
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


def validate_car(data: bytes):
    """Parse our own output back: BOM ok, renditions decode, LZFSE round-trips."""
    vars_, ptrs = read_bom(data)
    assert "CARHEADER" in vars_ and "RENDITIONS" in vars_ and "FACETKEYS" in vars_
    blk = lambda i: data[ptrs[i][0]:ptrs[i][0] + ptrs[i][1]]
    tree = blk(vars_["RENDITIONS"])
    _, child, _, pc, _ = struct.unpack(">IIIIB", tree[4:21])
    paths = blk(child)
    isLeaf, cnt = struct.unpack(">HH", paths[:4])
    images = 0
    for i in range(cnt):
        vidx, kidx = struct.unpack_from(">II", paths, 12 + i * 8)
        v = blk(vidx)
        layout = struct.unpack_from("<H", v, 36)[0]
        if layout != 0x0C:
            continue
        w, h = struct.unpack_from("<II", v, 12)
        tvl_len = struct.unpack_from("<I", v, 168)[0]
        ro = 184 + tvl_len
        assert v[ro:ro + 4] == b"MLEC", "rendition not MLEC"
        # walk KCBC blocks, decompress, verify total pixel count
        body = v[ro + 16:]
        total = 0
        p = 0
        while p + 20 <= len(body) and body[p:p + 4] == b"KCBC":
            _, _, rows, clen = struct.unpack_from("<IIII", body, p + 4)
            seg = body[p + 20:p + 20 + clen]
            with tempfile.NamedTemporaryFile(delete=False) as ti:
                ti.write(seg); s = ti.name
            o = s + ".o"
            subprocess.run(["lzfse", "-decode", "-i", s, "-o", o], check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            total += len(open(o, "rb").read())
            os.remove(s); os.remove(o)
            p += 20 + clen
        assert total == w * h * 4, f"pixels {total} != {w*h*4}"
        images += 1
    assert images >= 1, "no image renditions"
    return images


def selftest(out):
    px = lambda w: bytes((0x20, 0x40, 0x80, 0xFF)) * (w * w)
    images = [
        {"idiom_str": "iphone", "scale": 2, "point": 60.0, "width": 120,
         "height": 120, "bgra": px(120), "name": "icon60@2x.png"},
        {"idiom_str": "iphone", "scale": 3, "point": 60.0, "width": 180,
         "height": 180, "bgra": px(180), "name": "icon60@3x.png"},
        {"idiom_str": "ios-marketing", "scale": 1, "point": 1024.0, "width": 1024,
         "height": 1024, "bgra": px(1024), "name": "Icon.png"},
    ]
    data = build_car(images)
    vars_, ptrs = read_bom(data)
    imgs = validate_car(data)
    print(f"BOM ok: {len(ptrs)} blocks, vars={sorted(vars_)}, image renditions={imgs}")
    open(out, "wb").write(data)
    print(f"wrote {out} ({len(data)} bytes)")


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "selftest":
        selftest(sys.argv[2] if len(sys.argv) > 2 else "/tmp/mkcar_selftest.car")
    elif len(sys.argv) == 4 and sys.argv[1] == "build":
        images = load_appiconset(sys.argv[2])
        data = build_car(images)
        n = validate_car(data)
        open(sys.argv[3], "wb").write(data)
        print(f"Assets.car: {n} icon renditions, {len(data)} bytes -> {sys.argv[3]}")
    else:
        print("usage: mkcar.py selftest [out.car] | build <AppIcon.appiconset> <out.car>",
              file=sys.stderr)
        sys.exit(2)
