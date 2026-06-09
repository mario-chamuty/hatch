#!/usr/bin/env python3
"""
mkcar.py - write a minimal compiled Asset Catalog (Assets.car) for app icons,
on Linux/Windows, with no Apple tools (no actool / CoreUI).

The .car is a big-endian BOM (Bill of Materials) container holding named
blocks (CARHEADER, KEYFORMAT, EXTENDED_METADATA) and B-trees (FACETKEYS,
RENDITIONS). Layout reverse-engineered from:
  - bomutils (BOM container)            https://github.com/hogliux/bomutils
  - Timac/CARParser Car.h (CAR structs) https://github.com/Timac/CARParser
  - blog.timac.org .car format writeup

Status: structural writer + self round-trip verifier. Apple-validation tuning
(rendition flags, key-token order) is iterated against a reference actool .car.
"""

import struct
import sys
import zlib

# ---------------------------------------------------------------------------
# BOM container (all multi-byte fields big-endian)
# ---------------------------------------------------------------------------

class Bom:
    """Accumulates data blocks and named variables, then serializes a BOM."""

    def __init__(self):
        # block index 0 is the reserved null block
        self.blocks = [b""]
        self.vars = []  # list of (name, block_index)

    def add_block(self, data: bytes) -> int:
        self.blocks.append(data)
        return len(self.blocks) - 1

    def add_var(self, name: str, block_index: int):
        self.vars.append((name, block_index))

    def add_tree(self, name: str, pairs):
        """Add a single-leaf BOM tree variable from a list of (key, value) byte
        pairs. Each key/value becomes its own block; a leaf 'paths' node
        references them; a 'tree' block points at the leaf."""
        indices = []
        for key, value in pairs:
            vidx = self.add_block(value)
            kidx = self.add_block(key)
            indices.append((vidx, kidx))  # BOMPathIndices: (value, key)

        # BOMPaths leaf: isLeaf(u16) count(u16) forward(u32) backward(u32)
        #                then count * (index0 u32, index1 u32)
        paths = struct.pack(">HHII", 1, len(indices), 0, 0)
        for vidx, kidx in indices:
            paths += struct.pack(">II", vidx, kidx)
        paths_idx = self.add_block(paths)

        # BOMTree: 'tree' version(u32=1) child(u32) blockSize(u32=4096)
        #          pathCount(u32) unknown3(u8)
        tree = b"tree" + struct.pack(">IIIIB", 1, paths_idx, 4096, len(indices), 0)
        tree_idx = self.add_block(tree)
        self.add_var(name, tree_idx)

    def serialize(self) -> bytes:
        HEADER = 512  # actool pads the header region to 512 bytes
        body = bytearray(b"\x00" * HEADER)

        # Lay out each block; record (address, length). Block 0 is null.
        pointers = [(0, 0)]
        for data in self.blocks[1:]:
            addr = len(body)
            body += data
            # 4-byte align the next block
            pad = (-len(body)) % 4
            body += b"\x00" * pad
            pointers.append((addr, len(data)))

        # Block table (index): count, then BOMPointer{addr,len} per block,
        # plus a trailing empty free-list (count=0) as actool emits.
        index_offset = len(body)
        index = struct.pack(">I", len(pointers))
        for addr, length in pointers:
            index += struct.pack(">II", addr, length)
        index += struct.pack(">I", 0)  # free-list count
        body += index
        index_length = len(index)

        # Vars: count, then each BOMVar{index u32, length u8, name}
        vars_offset = len(body)
        v = struct.pack(">I", len(self.vars))
        for name, idx in self.vars:
            nb = name.encode("ascii")
            v += struct.pack(">IB", idx, len(nb)) + nb
        body += v
        vars_length = len(v)

        header = (
            b"BOMStore"
            + struct.pack(">IIIIII",
                          1,                # version
                          len(pointers),    # numberOfBlocks
                          index_offset,
                          index_length,
                          vars_offset,
                          vars_length)
        )
        body[0:len(header)] = header
        return bytes(body)


# ---------------------------------------------------------------------------
# CAR structures (Car.h)
# ---------------------------------------------------------------------------

def fourcc(s: str) -> int:
    # CAR tags are the C multi-char constant (big-endian char order) stored as
    # a host little-endian uint32 -> the bytes appear reversed in the file
    # (e.g. 'CTAR' -> "RATC", 'CTSI' -> "ISTC", 'ARGB' -> "BGRA"), exactly as
    # actool writes them.
    return struct.unpack(">I", s.encode("ascii"))[0]

def carheader(rendition_count: int) -> bytes:
    return struct.pack(
        "<IIIII128s256s16sIIII",
        fourcc("CTAR"),
        374,                 # coreuiVersion (matches actool reference)
        11,                  # storageVersion
        0,                   # storageTimestamp
        rendition_count,
        b"@(#)PROGRESS:73",  # mainVersionString
        b"IBCocoaTouchImageCatalogTool-mkcar",  # versionString
        b"\x00" * 16,        # uuid
        0, 0, 0, 0,          # associatedChecksum, schemaVersion, colorSpaceID, keySemantics
    )

def extended_metadata() -> bytes:
    return struct.pack(
        "<I256s256s256s256s",
        fourcc("META"),
        b"",
        b"15.0",
        b"ios",
        b"hatch-mkcar",
    )

# KEYFORMAT attribute type ids (Car.h RenditionAttributeType)
A_SCALE = 12
A_IDIOM = 15
A_SUBTYPE = 16
A_IDENTIFIER = 17
A_ELEMENT = 1
A_PART = 2
A_VALUE = 6
A_DIMENSION1 = 8
A_DIMENSION2 = 9
A_STATE = 10
A_DIRECTION = 4
A_SIZE = 3

# Exact 13-token KEYFORMAT order observed in actool output (RenditionAttributeType
# ids): Scale, Idiom, Subtype, GraphicsFeatureSetClass, MemoryLevelClass,
# HorizontalSizeClass, VerticalSizeClass, Identifier, Element, Part, State,
# Value, Dimension1.
A_HSIZECLASS = 20
A_VSIZECLASS = 21
A_MEMCLASS = 22
A_GFXCLASS = 23
KEY_TOKENS = [A_SCALE, A_IDIOM, A_SUBTYPE, A_GFXCLASS, A_MEMCLASS,
              A_HSIZECLASS, A_VSIZECLASS, A_IDENTIFIER, A_ELEMENT,
              A_PART, A_STATE, A_VALUE, A_DIMENSION1]

def keyformat() -> bytes:
    head = struct.pack("<III", fourcc("kfmt"), 0, len(KEY_TOKENS))
    return head + b"".join(struct.pack("<I", t) for t in KEY_TOKENS)

def rendition_key(attrs: dict) -> bytes:
    """Encode a rendition key as one uint16 per KEY_TOKENS attribute."""
    return b"".join(struct.pack("<H", attrs.get(t, 0)) for t in KEY_TOKENS)

def facet_value(identifier: int) -> bytes:
    # renditionkeytoken: cursorHotSpot{x,y}(u16), numberOfAttributes(u16),
    #                    then renditionAttribute{name u16, value u16}
    attrs = [(A_IDENTIFIER, identifier)]
    out = struct.pack("<HHH", 0, 0, len(attrs))
    for name, value in attrs:
        out += struct.pack("<HH", name, value)
    return out

def csiheader(width, height, scale, name, payload_len, pixel_format="ARGB"):
    # renditionFlags: bit0 isOpaque etc. 0 is fine for icons.
    flags = 0
    csimetadata = struct.pack("<IHH128s", 0, 0x3F2, 0, name.encode("ascii")[:128])
    csibitmaplist = struct.pack("<IIII", 0, 0, 0, payload_len)  # tlvLength=0
    return struct.pack(
        "<IIIIIII",
        fourcc("CTSI"),
        1,                       # version
        flags,
        width, height,
        scale,                   # 100/200/300
        fourcc(pixel_format),
    ) + struct.pack("<I", 0) + csimetadata + csibitmaplist  # colorSpace u32 then meta

def celm_uncompressed(bgra: bytes) -> bytes:
    # CUIThemePixelRendition: 'CELM' version compressionType rawDataLength data
    return struct.pack("<IIII", fourcc("CELM"), 1, 0, len(bgra)) + bgra


# ---------------------------------------------------------------------------
# Assemble a .car from a list of icon renditions
# ---------------------------------------------------------------------------

def build_car(renditions) -> bytes:
    """renditions: list of dicts {width,height,scale,idiom,subtype,bgra}."""
    bom = Bom()
    bom.add_var("CARHEADER", bom.add_block(carheader(len(renditions))))
    bom.add_var("KEYFORMAT", bom.add_block(keyformat()))

    # FACETKEYS: one facet "AppIcon" -> identifier
    identifier = 0x4D2  # arbitrary stable id
    bom.add_tree("FACETKEYS", [(b"AppIcon", facet_value(identifier))])

    # RENDITIONS: one entry per icon size/scale
    rend_pairs = []
    for r in renditions:
        attrs = {
            A_IDENTIFIER: identifier,
            A_SCALE: r["scale"] // 100,
            A_IDIOM: r["idiom"],
            A_SUBTYPE: r.get("subtype", 0),
            A_PART: 0, A_ELEMENT: 0,
        }
        key = rendition_key(attrs)
        value = csiheader(r["width"], r["height"], r["scale"], "AppIcon",
                          len(celm_uncompressed(r["bgra"]))) \
                + celm_uncompressed(r["bgra"])
        rend_pairs.append((key, value))
    bom.add_tree("RENDITIONS", rend_pairs)

    return bom.serialize()


# ---------------------------------------------------------------------------
# Self round-trip verifier (reads the BOM container we just wrote)
# ---------------------------------------------------------------------------

def read_bom(data: bytes):
    assert data[:8] == b"BOMStore", "bad magic"
    version, nblocks, ioff, ilen, voff, vlen = struct.unpack(">IIIIII", data[8:32])
    count = struct.unpack(">I", data[ioff:ioff + 4])[0]
    ptrs = []
    p = ioff + 4
    for _ in range(count):
        a, l = struct.unpack(">II", data[p:p + 8]); p += 8
        ptrs.append((a, l))
    vcount = struct.unpack(">I", data[voff:voff + 4])[0]
    p = voff + 4
    vars_ = {}
    for _ in range(vcount):
        idx, nlen = struct.unpack(">IB", data[p:p + 5]); p += 5
        name = data[p:p + nlen].decode("ascii"); p += nlen
        vars_[name] = idx
    def block(name):
        a, l = ptrs[vars_[name]]
        return data[a:a + l]
    return vars_, ptrs, block


def selftest():
    # 1x1 white pixel BGRA for each of a couple of sizes
    def px(w, h):
        return b"\xff\xff\xff\xff" * (w * h)
    renditions = [
        {"width": 120, "height": 120, "scale": 200, "idiom": 1, "subtype": 0, "bgra": px(120, 120)},
        {"width": 180, "height": 180, "scale": 300, "idiom": 1, "subtype": 0, "bgra": px(180, 180)},
        {"width": 1024, "height": 1024, "scale": 100, "idiom": 0, "subtype": 0, "bgra": px(1024, 1024)},
    ]
    data = build_car(renditions)
    vars_, ptrs, block = read_bom(data)
    print(f"BOM ok: {len(ptrs)} blocks, vars={sorted(vars_)}")
    ch = block("CARHEADER")
    tag, = struct.unpack("<I", ch[:4])
    print(f"CARHEADER tag={'CTAR' if tag == fourcc('CTAR') else hex(tag)} "
          f"renditionCount={struct.unpack('<I', ch[16:20])[0]}")
    assert tag == fourcc("CTAR")
    assert "FACETKEYS" in vars_ and "RENDITIONS" in vars_
    out = sys.argv[2] if len(sys.argv) > 2 else "/tmp/mkcar_selftest.car"
    open(out, "wb").write(data)
    print(f"wrote {out} ({len(data)} bytes)")


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "selftest":
        selftest()
    else:
        print("usage: mkcar.py selftest [out.car]", file=sys.stderr)
        sys.exit(2)
