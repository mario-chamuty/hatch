//! Native Mach-O load-command patchers (pure Rust port of the inline `python3`
//! snippets in `pipeline.sh`).
//!
//! Two operations on a linked arm64 Mach-O, both in-place and size-preserving
//! except the optional `LC_SOURCE_VERSION` insertion (which only grows into
//! existing load-command slack, never moving section content):
//!
//!  * [`fix_linker_identity`] - make an `ld64.lld`-linked binary look like Apple's
//!    `ld` output (ITMS-90125): rewrite every `LC_BUILD_VERSION` build-tool id
//!    from 4 (`TOOL_LLD`) to 3 (`TOOL_LD`), and append an `LC_SOURCE_VERSION`
//!    when absent (lld omits it; every real Apple-ld binary carries one).
//!  * [`clear_dwarf_exec_bit`] - drop the rwx exec bit (ITMS-90999) from every
//!    non-`__TEXT` segment, used only on the bare-snapshot fallback path.

use anyhow::{bail, Result};

const MH_MAGIC_64: u32 = 0xfeed_facf;
const LC_BUILD_VERSION: u32 = 0x32;
const LC_SOURCE_VERSION: u32 = 0x2A;
const LC_SEGMENT_64: u32 = 0x19;

/// ld-prime version 1217.0.0, packed into the high 16 bits as actool/ld do.
const TOOL_LD: u32 = 3;
const LD_VERSION: u32 = 1217 << 16;
/// source version 1.0, packed a24.b10.c10.d10.e10 -> a == 1.
const SOURCE_VERSION_1_0: u64 = 1 << 40;

#[inline]
fn rd_u32(d: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(d[off..off + 4].try_into().unwrap())
}

#[inline]
fn wr_u32(d: &mut [u8], off: usize, v: u32) {
    d[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn check_magic(d: &[u8]) -> Result<()> {
    if d.len() < 32 || rd_u32(d, 0) != MH_MAGIC_64 {
        bail!("expected a 64-bit little-endian (arm64) Mach-O");
    }
    Ok(())
}

/// Patch `d` so an lld-linked binary passes Apple's "built with Apple's linker"
/// check. Mirrors `fix_linker_identity` in `pipeline.sh` byte-for-byte.
pub fn fix_linker_identity(d: &mut Vec<u8>) -> Result<()> {
    check_magic(d)?;
    let ncmds = rd_u32(d, 16);
    let sizeofcmds = rd_u32(d, 20) as usize;

    let mut off = 32usize;
    let mut has_source = false;
    let mut first_sect = usize::MAX;

    for _ in 0..ncmds {
        let cmd = rd_u32(d, off);
        let cmdsize = rd_u32(d, off + 4) as usize;
        match cmd {
            LC_BUILD_VERSION => {
                let ntools = rd_u32(d, off + 20);
                let mut to = off + 24;
                for _ in 0..ntools {
                    if rd_u32(d, to) != TOOL_LD {
                        wr_u32(d, to, TOOL_LD);
                        wr_u32(d, to + 4, LD_VERSION);
                    }
                    to += 8;
                }
            }
            LC_SOURCE_VERSION => has_source = true,
            LC_SEGMENT_64 => {
                let nsects = rd_u32(d, off + 64);
                let mut so = off + 72;
                for _ in 0..nsects {
                    let soff = rd_u32(d, so + 48) as usize;
                    if soff != 0 {
                        first_sect = first_sect.min(soff);
                    }
                    so += 80;
                }
            }
            _ => {}
        }
        off += cmdsize;
    }

    // Insert LC_SOURCE_VERSION into the load-command slack iff it fits before the
    // first section's file content (so no section bytes move).
    if !has_source && 32 + sizeofcmds + 16 <= first_sect {
        let ins = 32 + sizeofcmds;
        wr_u32(d, ins, LC_SOURCE_VERSION);
        wr_u32(d, ins + 4, 16);
        d[ins + 8..ins + 16].copy_from_slice(&SOURCE_VERSION_1_0.to_le_bytes());
        wr_u32(d, 16, ncmds + 1);
        wr_u32(d, 20, (sizeofcmds + 16) as u32);
    }
    Ok(())
}

/// Parse a `"13.4"` / `"26.0"` version string into the packed `xxxx.yy.zz`
/// (a16.b8.c8) form used by `LC_BUILD_VERSION`.
fn pack_version(v: &str) -> u32 {
    let mut it = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let a = it.next().unwrap_or(0);
    let b = it.next().unwrap_or(0);
    let c = it.next().unwrap_or(0);
    (a << 16) | (b << 8) | c
}

/// Set the iOS `LC_BUILD_VERSION` minos + sdk (and stamp `tool = ld 1217`) on a
/// linked Mach-O. This is the pure-Rust replacement for the cctools `vtool
/// -set-build-version ios <minos> <sdk> -tool ld 1217` call used on the Flutter
/// engine binary (ITMS-90208 alignment) - cctools/vtool has no Windows build.
pub fn set_ios_build_version(d: &mut [u8], minos: &str, sdk: &str) -> Result<()> {
    check_magic(d)?;
    let minos_p = pack_version(minos);
    let sdk_p = pack_version(sdk);
    let ncmds = rd_u32(d, 16);
    let mut off = 32usize;
    for _ in 0..ncmds {
        let cmd = rd_u32(d, off);
        let cmdsize = rd_u32(d, off + 4) as usize;
        if cmd == LC_BUILD_VERSION {
            // platform(off+8) minos(off+12) sdk(off+16) ntools(off+20)
            wr_u32(d, off + 12, minos_p);
            wr_u32(d, off + 16, sdk_p);
            let ntools = rd_u32(d, off + 20);
            let mut to = off + 24;
            for _ in 0..ntools {
                wr_u32(d, to, TOOL_LD);
                wr_u32(d, to + 4, LD_VERSION);
                to += 8;
            }
        }
        off += cmdsize;
    }
    Ok(())
}

/// Clear the executable protection bit on every non-`__TEXT` segment (ITMS-90999
/// on the bare-snapshot path). Mirrors the inline python in `pipeline.sh`.
pub fn clear_dwarf_exec_bit(d: &mut [u8]) -> Result<()> {
    check_magic(d)?;
    let ncmds = rd_u32(d, 16);
    let mut off = 32usize;
    for _ in 0..ncmds {
        let cmd = rd_u32(d, off);
        let cmdsize = rd_u32(d, off + 4) as usize;
        if cmd == LC_SEGMENT_64 {
            let seg = &d[off + 8..off + 24];
            let name_end = seg.iter().position(|&b| b == 0).unwrap_or(seg.len());
            if &seg[..name_end] != b"__TEXT" {
                for fld in [56usize, 60] {
                    let prot = rd_u32(d, off + fld);
                    if prot & 0x4 != 0 {
                        wr_u32(d, off + fld, prot & !0x4);
                    }
                }
            }
        }
        off += cmdsize;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal arm64 Mach-O: header + one LC_BUILD_VERSION (tool=4) +
    /// one LC_SEGMENT_64 (__TEXT, one section at file offset `sect_off`), padded
    /// out so the header sits before the section content.
    fn synth(tool: u32, sect_off: u32) -> Vec<u8> {
        let mut bv = Vec::new();
        bv.extend_from_slice(&LC_BUILD_VERSION.to_le_bytes());
        // cmdsize = 24-byte base (cmd,cmdsize,platform,minos,sdk,ntools) + 8 per tool
        bv.extend_from_slice(&32u32.to_le_bytes()); // cmdsize
        bv.extend_from_slice(&2u32.to_le_bytes()); // platform ios
        bv.extend_from_slice(&(13u32 << 16).to_le_bytes()); // minos
        bv.extend_from_slice(&(26u32 << 16).to_le_bytes()); // sdk
        bv.extend_from_slice(&1u32.to_le_bytes()); // ntools
        bv.extend_from_slice(&tool.to_le_bytes()); // tool id
        bv.extend_from_slice(&0u32.to_le_bytes()); // tool version

        let mut seg = Vec::new();
        let seg_cmdsize = 72u32 + 80; // segment + 1 section
        seg.extend_from_slice(&LC_SEGMENT_64.to_le_bytes());
        seg.extend_from_slice(&seg_cmdsize.to_le_bytes());
        let mut name = *b"__TEXT\0\0\0\0\0\0\0\0\0\0";
        seg.extend_from_slice(&name);
        seg.extend_from_slice(&[0u8; 32]); // vmaddr,vmsize,fileoff,filesize
        seg.extend_from_slice(&7u32.to_le_bytes()); // maxprot rwx
        seg.extend_from_slice(&5u32.to_le_bytes()); // initprot r-x
        seg.extend_from_slice(&1u32.to_le_bytes()); // nsects
        seg.extend_from_slice(&0u32.to_le_bytes()); // flags
        // one section_64: name(32) + addr,size(16) + offset(4) ...
        let mut sect = vec![0u8; 80];
        name = *b"__text\0\0\0\0\0\0\0\0\0\0";
        sect[0..16].copy_from_slice(&name);
        sect[48..52].copy_from_slice(&sect_off.to_le_bytes());
        seg.extend_from_slice(&sect);

        let ncmds = 2u32;
        let sizeofcmds = (bv.len() + seg.len()) as u32;
        let mut d = Vec::new();
        d.extend_from_slice(&MH_MAGIC_64.to_le_bytes());
        d.extend_from_slice(&0x0100000Cu32.to_le_bytes()); // cputype arm64
        d.extend_from_slice(&0u32.to_le_bytes()); // cpusubtype
        d.extend_from_slice(&6u32.to_le_bytes()); // filetype
        d.extend_from_slice(&ncmds.to_le_bytes());
        d.extend_from_slice(&sizeofcmds.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes()); // flags
        d.extend_from_slice(&0u32.to_le_bytes()); // reserved
        d.extend_from_slice(&bv);
        d.extend_from_slice(&seg);
        d.resize(sect_off as usize + 16, 0); // section content room
        d
    }

    #[test]
    fn rewrites_tool_id_4_to_3() {
        let mut d = synth(4, 4096);
        fix_linker_identity(&mut d).unwrap();
        // LC_BUILD_VERSION starts at off 32; tool id at 32+24, version at +28.
        assert_eq!(rd_u32(&d, 32 + 24), TOOL_LD);
        assert_eq!(rd_u32(&d, 32 + 28), LD_VERSION);
    }

    #[test]
    fn inserts_source_version_when_absent() {
        let mut d = synth(4, 4096);
        let ncmds_before = rd_u32(&d, 16);
        let size_before = rd_u32(&d, 20);
        fix_linker_identity(&mut d).unwrap();
        assert_eq!(rd_u32(&d, 16), ncmds_before + 1);
        assert_eq!(rd_u32(&d, 20), size_before + 16);
        // The new command sits right after the old command block.
        let ins = 32 + size_before as usize;
        assert_eq!(rd_u32(&d, ins), LC_SOURCE_VERSION);
        assert_eq!(rd_u32(&d, ins + 4), 16);
    }

    #[test]
    fn does_not_insert_when_no_slack() {
        // Put the first section's file offset only 8 bytes past the load
        // commands - too tight for the 16-byte LC_SOURCE_VERSION.
        let mut d = synth(4, 4096);
        let sizeofcmds = rd_u32(&d, 20) as usize;
        let near = (32 + sizeofcmds + 8) as u32;
        // section_64 offset field: header(32) + bv(32) + seg header(72) + 48.
        const SECT_OFF_FIELD: usize = 32 + 32 + 72 + 48;
        wr_u32(&mut d, SECT_OFF_FIELD, near);
        let ncmds_before = rd_u32(&d, 16);
        fix_linker_identity(&mut d).unwrap();
        assert_eq!(rd_u32(&d, 16), ncmds_before, "must not insert without slack");
    }

    #[test]
    fn idempotent_when_tool_already_3() {
        let mut d = synth(3, 4096);
        let mut d2 = d.clone();
        fix_linker_identity(&mut d).unwrap();
        // tool stays 3; source version still gets added once.
        assert_eq!(rd_u32(&d, 32 + 24), TOOL_LD);
        fix_linker_identity(&mut d2).unwrap();
        fix_linker_identity(&mut d2).unwrap();
        // running twice must not add two source-version commands
        assert_eq!(rd_u32(&d2, 16), rd_u32(&d, 16));
    }
}
