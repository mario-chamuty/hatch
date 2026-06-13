//! Native (Mac-free, no-python) Assets.car app-icon catalog via the **transplant**
//! path - the only path proven to clear App Store ingestion (build 41).
//!
//! A genuine `actool` catalog (the donor, shipped as an embedded template) is
//! copied byte-for-byte except the app-icon pixel payloads, which are replaced
//! with our artwork re-encoded with the Apple-reference LZFSE (the `lzfse` crate
//! produces byte-identical output to the `lzfse` CLI used in build 41). Because
//! the container, BOM slot table, free list, trees and keys are preserved
//! verbatim and reserialization is deterministic, the output is byte-identical to
//! the proven `temp/transplant.py` for the same donor + iconset.
//!
//! Pure Rust port of `temp/transplant.py` + the helpers it used from
//! `src/ios/tools/mkcar.py` (`load_appiconset`, `pad_rows`, `mlec_lzfse`,
//! `decode_png`). Replaces `python3` + the `lzfse` CLI in the icon path.
//!
//! Endianness: the BOM *container* is big-endian; the CoreUI structures inside
//! the blocks (CSI headers, TLVs, KLNI, MLEC) are little-endian.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

// ---- byte helpers -------------------------------------------------------

#[inline]
fn be_u32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(d[o..o + 4].try_into().unwrap())
}
#[inline]
fn le_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}
#[inline]
fn le_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(d[o..o + 2].try_into().unwrap())
}
#[inline]
fn be_u16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes(d[o..o + 2].try_into().unwrap())
}
#[inline]
fn wr_le_u32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

// ---- icon source --------------------------------------------------------

pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Decode a PNG to opaque BGRA (alpha forced to 0xFF, matching `decode_png`:
/// App Store icons must not carry alpha). Mirrors the python's B,G,R,0xFF order.
pub fn decode_png_bgra(png: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .context("decoding icon PNG")?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for px in img.pixels() {
        let [r, g, b, _a] = px.0;
        bgra.extend_from_slice(&[b, g, r, 0xFF]);
    }
    Ok((w, h, bgra))
}

/// Parse `AppIcon.appiconset/Contents.json` into a `(width,height) -> BGRA` map.
/// (The transplant only needs to look icons up by pixel dimensions.)
pub fn load_appiconset(dir: &Path) -> Result<HashMap<(u32, u32), Vec<u8>>> {
    let contents_path = dir.join("Contents.json");
    let raw = std::fs::read(&contents_path)
        .with_context(|| format!("reading {}", contents_path.display()))?;
    let contents: serde_json::Value =
        serde_json::from_slice(&raw).context("parsing Contents.json")?;
    let mut by_wh = HashMap::new();
    if let Some(images) = contents.get("images").and_then(|v| v.as_array()) {
        for img in images {
            let Some(fname) = img.get("filename").and_then(|v| v.as_str()) else {
                continue;
            };
            let png_path = dir.join(fname);
            if !png_path.exists() {
                continue;
            }
            let png = std::fs::read(&png_path)
                .with_context(|| format!("reading {}", png_path.display()))?;
            let (w, h, bgra) = decode_png_bgra(&png)?;
            by_wh.insert((w, h), bgra);
        }
    }
    if by_wh.is_empty() {
        bail!("no icon images found in {}", dir.display());
    }
    Ok(by_wh)
}

// ---- pixel + payload encoding ------------------------------------------

/// Pad each pixel row to actool's 8-px (32-byte) stride. Returns
/// `(padded_bgra, stride_px)`.
pub fn pad_rows(bgra: &[u8], width: u32, height: u32) -> (Vec<u8>, u32) {
    let stride_px = (width + 7) / 8 * 8;
    if stride_px == width {
        return (bgra.to_vec(), stride_px);
    }
    let (rb, sb) = ((width * 4) as usize, (stride_px * 4) as usize);
    let mut out = vec![0u8; sb * height as usize];
    for r in 0..height as usize {
        out[r * sb..r * sb + rb].copy_from_slice(&bgra[r * rb..(r + 1) * rb]);
    }
    (out, stride_px)
}

fn lzfse_compress(data: &[u8]) -> Vec<u8> {
    // Apple-reference LZFSE can expand slightly on incompressible input; size the
    // output buffer generously, then truncate to the encoded length.
    let mut out = vec![0u8; data.len() + data.len() / 2 + 4096];
    let n = lzfse::encode_buffer(data, &mut out)
        .expect("lzfse encode buffer too small (should not happen)");
    out.truncate(n);
    out
}

/// MLEC (CELM) payload: row-chunked, LZFSE-compressed KCBC blocks. actool's
/// chunk policy: `rows_per_chunk = max(height/3, 8192/bytes_per_row, 1)`.
pub fn mlec_lzfse(bgra: &[u8], width: u32, height: u32) -> Vec<u8> {
    let bytes_per_row = (width * 4) as usize;
    let rows_per_chunk = (height as usize / 3)
        .max(8192 / bytes_per_row.max(1))
        .max(1);
    let mut blocks: Vec<u8> = Vec::new();
    let mut nchunks = 0u32;
    let mut row = 0usize;
    let h = height as usize;
    while row < h {
        let n = rows_per_chunk.min(h - row);
        let chunk = &bgra[row * bytes_per_row..(row + n) * bytes_per_row];
        let comp = lzfse_compress(chunk);
        blocks.extend_from_slice(b"KCBC");
        blocks.extend_from_slice(&0u32.to_le_bytes());
        blocks.extend_from_slice(&0u32.to_le_bytes());
        blocks.extend_from_slice(&(n as u32).to_le_bytes());
        blocks.extend_from_slice(&(comp.len() as u32).to_le_bytes());
        blocks.extend_from_slice(&comp);
        row += n;
        nchunks += 1;
    }
    let mut out = Vec::with_capacity(16 + blocks.len());
    out.extend_from_slice(b"MLEC"); // fourcc("CELM") byte-reversed in file
    out.extend_from_slice(&3u32.to_le_bytes()); // version
    out.extend_from_slice(&4u32.to_le_bytes()); // compressionType = LZFSE
    out.extend_from_slice(&nchunks.to_le_bytes());
    out.extend_from_slice(&blocks);
    out
}

// ---- the transplant -----------------------------------------------------

const RENDITION_LEN_OFF: usize = 0xB4; // CSI header renditionLength field
const CSI_HEADER_LEN: usize = 184;
const KF_LEN: usize = 10; // KEYFORMAT token count (10-token CoreUI)

/// Splice our icon pixels into a genuine donor catalog. Returns the new `.car`
/// bytes (byte-identical to `temp/transplant.py` for the same inputs).
pub fn transplant(donor: &[u8], iconset_dir: &Path) -> Result<Vec<u8>> {
    let d = donor;
    if &d[0..8] != b"BOMStore" {
        bail!("donor is not a BOMStore container");
    }
    // BOM header (big-endian) at offset 8: ver, nblocks, ioff, ilen, voff, vlen.
    let ver = be_u32(d, 8);
    let nblocks = be_u32(d, 12);
    let ioff = be_u32(d, 16) as usize;
    let ilen = be_u32(d, 20) as usize;
    let voff = be_u32(d, 24) as usize;
    let vlen = be_u32(d, 28) as usize;

    let nslots = be_u32(d, ioff) as usize;
    let mut slots: Vec<(u32, u32)> = Vec::with_capacity(nslots);
    for i in 0..nslots {
        let p = ioff + 4 + 8 * i;
        slots.push((be_u32(d, p), be_u32(d, p + 4)));
    }
    let free_tail = d[ioff + 4 + 8 * nslots..ioff + ilen].to_vec();
    let vars_blob = d[voff..voff + vlen].to_vec();

    // blocks[i] = Some(bytes) for every non-null slot.
    let mut blocks: Vec<Option<Vec<u8>>> = vec![None; nslots];
    for (i, &(a, l)) in slots.iter().enumerate() {
        if a != 0 || l != 0 {
            blocks[i] = Some(d[a as usize..(a + l) as usize].to_vec());
        }
    }

    // vars table -> name:idx
    let mut vt: HashMap<String, usize> = HashMap::new();
    {
        let vc = be_u32(&vars_blob, 0);
        let mut p = 4usize;
        for _ in 0..vc {
            let idx = be_u32(&vars_blob, p) as usize;
            let nl = vars_blob[p + 4] as usize;
            p += 5;
            let name = String::from_utf8_lossy(&vars_blob[p..p + nl]).into_owned();
            vt.insert(name, idx);
            p += nl;
        }
    }

    let by_wh = load_appiconset(iconset_dir)?;

    // RENDITIONS tree -> its leaf block.
    let rend_idx = *vt.get("RENDITIONS").context("donor has no RENDITIONS tree")?;
    let rend_tree = blocks[rend_idx].clone().context("RENDITIONS block missing")?;
    let leaf_idx = be_u32(&rend_tree, 8) as usize;
    let leaf = blocks[leaf_idx].clone().context("RENDITIONS leaf missing")?;
    let cnt = be_u16(&leaf, 2) as usize;

    let mut replaced = 0usize;

    // Pass 1: direct (0x0C) icon images named "Icon-App...".
    for i in 0..cnt {
        let vidx = be_u32(&leaf, 12 + 8 * i) as usize;
        let cb = match &blocks[vidx] {
            Some(b) => b.clone(),
            None => continue,
        };
        if &cb[0..4] != b"ISTC" {
            continue;
        }
        let w = le_u32(&cb, 12);
        let h = le_u32(&cb, 16);
        let layout = le_u16(&cb, 36);
        let name_field = &cb[40..168];
        let name_end = name_field.iter().position(|&b| b == 0).unwrap_or(128);
        let name = String::from_utf8_lossy(&name_field[..name_end]);
        if !name.starts_with("Icon-App") || layout != 0x0C {
            continue;
        }
        let Some(src) = by_wh.get(&(w, h)) else {
            continue; // no source pixels for this size; leave donor's
        };
        let tvl = le_u32(&cb, 168) as usize;
        let stride = find_tlv_stride(&cb, tvl).unwrap_or(w * 4);
        let (padded, stride_px) = pad_rows(src, w, h);
        if stride_px * 4 != stride {
            bail!("stride mismatch for {name}: {} vs {}", stride_px * 4, stride);
        }
        let mlec = mlec_lzfse(&padded, stride_px, h);
        blocks[vidx] = Some(splice_payload(&cb, tvl, &mlec));
        replaced += 1;
    }

    // Map donor atlases (layout 0x3EC) by (scale, idiom, dim1).
    let mut atlases: HashMap<(u16, u16, u16), usize> = HashMap::new();
    for i in 0..cnt {
        let vidx = be_u32(&leaf, 12 + 8 * i) as usize;
        let kidx = be_u32(&leaf, 12 + 8 * i + 4) as usize;
        let (Some(cb), Some(kb)) = (&blocks[vidx], &blocks[kidx]) else {
            continue;
        };
        if cb.len() < 38 || &cb[0..4] != b"ISTC" {
            continue;
        }
        if le_u16(cb, 36) == 0x3EC && kb.len() >= KF_LEN * 2 {
            let scale = le_u16(kb, 2 * 2); // toks[2] = attr12 scale
            let idiom = le_u16(kb, 3 * 2); // toks[3] = attr15 idiom
            let dim1 = le_u16(kb, 6 * 2); // toks[6] = attr8  dimension1
            atlases.insert((scale, idiom, dim1), vidx);
        }
    }

    // Collect KLNI rects per atlas from the 0x3EB internal references.
    let mut atlas_rects: HashMap<(u16, u16, u16), Vec<(u32, u32, u32, u32)>> = HashMap::new();
    for i in 0..cnt {
        let vidx = be_u32(&leaf, 12 + 8 * i) as usize;
        let cb = match &blocks[vidx] {
            Some(b) => b,
            None => continue,
        };
        if cb.len() < 38 || &cb[0..4] != b"ISTC" || le_u16(cb, 36) != 0x3EB {
            continue;
        }
        let tvl = le_u32(cb, 168) as usize;
        let Some(klni) = find_tlv(cb, tvl, 0x3F2) else {
            continue;
        };
        if klni.len() < 28 || &klni[0..4] != b"KLNI" {
            continue;
        }
        let x = le_u32(klni, 8);
        let y = le_u32(klni, 12);
        let kw = le_u32(klni, 16);
        let kh = le_u32(klni, 20);
        // keyblob starts at klni[28]; pairs after a leading u16(0).
        let kb2 = &klni[28..];
        let mut pairs: HashMap<u16, u16> = HashMap::new();
        let mut q = 2usize;
        while q + 4 <= kb2.len() {
            let a = le_u16(kb2, q);
            let v = le_u16(kb2, q + 2);
            if a == 0 {
                break;
            }
            pairs.insert(a, v);
            q += 4;
        }
        let ak = (
            *pairs.get(&12).unwrap_or(&0),
            *pairs.get(&15).unwrap_or(&0),
            *pairs.get(&8).unwrap_or(&0),
        );
        atlas_rects.entry(ak).or_default().push((x, y, kw, kh));
    }

    // Rebuild each atlas bitmap with our pixels at the donor's KLNI rects.
    let mut aks: Vec<_> = atlas_rects.keys().cloned().collect();
    aks.sort();
    for ak in aks {
        let rects = &atlas_rects[&ak];
        let Some(&avidx) = atlases.get(&ak) else {
            bail!("atlas {ak:?} referenced by a KLNI but not found in donor");
        };
        let acb = blocks[avidx].clone().unwrap();
        let ah = le_u32(&acb, 16);
        let tvl = le_u32(&acb, 168) as usize;
        let stride = find_tlv_stride(&acb, tvl).context("atlas has no 0x3EF stride")?;
        let stride_px = stride / 4;
        let mut buf = vec![0u8; (stride_px * ah * 4) as usize];
        for &(x, y, w, h) in rects {
            let src = by_wh
                .get(&(w, h))
                .with_context(|| format!("atlas {ak:?} rect {w}x{h}: no source pixels"))?;
            let rb = (w * 4) as usize;
            // CoreUI atlases use a BOTTOM-LEFT origin: the KLNI `y` is the offset
            // from the bottom of the atlas, not the top. Our `src` rows are stored
            // top-down (PNG order), so the icon's top row lands at top-origin row
            // `ah - y - h`. Writing at `y` directly (top-origin) makes iOS read the
            // wrong rows and mangles the icon (top half = bottom half + a stray
            // smaller sub-icon bleeding in). Verified against the genuine donor
            // catalog: only `ah - y - h` reconstructs every sub-icon upright.
            let top = (ah - y - h) as usize;
            for r in 0..h as usize {
                let dst = ((top + r) * stride_px as usize + x as usize) * 4;
                buf[dst..dst + rb].copy_from_slice(&src[r * rb..(r + 1) * rb]);
            }
        }
        let mlec = mlec_lzfse(&buf, stride_px, ah);
        blocks[avidx] = Some(splice_payload(&acb, tvl, &mlec));
        replaced += 1;
    }

    let _ = replaced; // count is informational; callers can re-derive if needed

    // Reserialize: identical slot count/order/free-list, recomputed addresses.
    let mut body: Vec<u8> = vec![0u8; 512];
    let mut new_slots: Vec<(u32, u32)> = Vec::with_capacity(nslots);
    for i in 0..nslots {
        let (a, l) = slots[i];
        if (a != 0 || l != 0) && blocks[i].is_some() {
            let data = blocks[i].as_ref().unwrap();
            let addr = body.len() as u32;
            body.extend_from_slice(data);
            let pad = (4 - (body.len() % 4)) % 4;
            body.extend(std::iter::repeat(0u8).take(pad));
            new_slots.push((addr, data.len() as u32));
        } else {
            new_slots.push((a, l));
        }
    }
    let new_voff = body.len() as u32;
    body.extend_from_slice(&vars_blob);
    let new_ioff = body.len();
    let mut index: Vec<u8> = Vec::with_capacity(4 + 8 * nslots + free_tail.len());
    index.extend_from_slice(&(nslots as u32).to_be_bytes());
    for (a, l) in &new_slots {
        index.extend_from_slice(&a.to_be_bytes());
        index.extend_from_slice(&l.to_be_bytes());
    }
    index.extend_from_slice(&free_tail);
    let ilen_new = index.len() as u32;
    body.extend_from_slice(&index);

    // Header (big-endian) over the first 32 bytes.
    body[0..8].copy_from_slice(b"BOMStore");
    body[8..12].copy_from_slice(&ver.to_be_bytes());
    body[12..16].copy_from_slice(&nblocks.to_be_bytes());
    body[16..20].copy_from_slice(&(new_ioff as u32).to_be_bytes());
    body[20..24].copy_from_slice(&ilen_new.to_be_bytes());
    body[24..28].copy_from_slice(&new_voff.to_be_bytes());
    body[28..32].copy_from_slice(&(vlen as u32).to_be_bytes());

    Ok(body)
}

/// Replace a CSI block's payload (everything after the 184-byte header + `tvl`
/// TLV bytes) with `mlec` and update the renditionLength field.
fn splice_payload(cb: &[u8], tvl: usize, mlec: &[u8]) -> Vec<u8> {
    let keep = CSI_HEADER_LEN + tvl;
    let mut new_cb = Vec::with_capacity(keep + mlec.len());
    new_cb.extend_from_slice(&cb[..keep]);
    new_cb.extend_from_slice(mlec);
    wr_le_u32(&mut new_cb, RENDITION_LEN_OFF, mlec.len() as u32);
    new_cb
}

/// Walk a CSI block's TLV info list (starts at byte 184, `tvl` bytes long) and
/// return the value bytes of the first entry with tag `tag`.
fn find_tlv(cb: &[u8], tvl: usize, tag: u32) -> Option<&[u8]> {
    let mut tp = CSI_HEADER_LEN;
    let end = CSI_HEADER_LEN + tvl;
    while tp + 8 <= end {
        let t = le_u32(cb, tp);
        let l = le_u32(cb, tp + 4) as usize;
        if t == tag {
            return Some(&cb[tp + 8..tp + 8 + l]);
        }
        tp += 8 + l;
    }
    None
}

/// Row stride in bytes from the 0x3EF TLV entry, if present.
fn find_tlv_stride(cb: &[u8], tvl: usize) -> Option<u32> {
    find_tlv(cb, tvl, 0x3EF).map(|v| u32::from_le_bytes(v[0..4].try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_rows_8px_stride() {
        // width 60 -> stride 64; row bytes copied, tail zero-padded.
        let bgra = vec![0xABu8; 60 * 4 * 2]; // 2 rows
        let (out, stride_px) = pad_rows(&bgra, 60, 2);
        assert_eq!(stride_px, 64);
        assert_eq!(out.len(), 64 * 4 * 2);
        // first 240 bytes of each row are data, next 16 are zero.
        assert!(out[0..240].iter().all(|&b| b == 0xAB));
        assert!(out[240..256].iter().all(|&b| b == 0));
    }

    #[test]
    fn pad_rows_noop_when_aligned() {
        let bgra = vec![1u8; 64 * 4];
        let (out, stride_px) = pad_rows(&bgra, 64, 1);
        assert_eq!(stride_px, 64);
        assert_eq!(out, bgra);
    }

    #[test]
    fn mlec_framing_and_roundtrip() {
        // 120x120 opaque image -> MLEC; decode each KCBC chunk, verify pixels.
        let (w, h) = (120u32, 120u32);
        let mut bgra = Vec::new();
        for i in 0..(w * h) {
            let v = (i & 0xFF) as u8;
            bgra.extend_from_slice(&[v, v, v, 0xFF]);
        }
        let payload = mlec_lzfse(&bgra, w, h);
        assert_eq!(&payload[0..4], b"MLEC");
        assert_eq!(le_u32(&payload, 4), 3);
        assert_eq!(le_u32(&payload, 8), 4);
        let nchunks = le_u32(&payload, 12);
        assert!(nchunks >= 1);
        // walk chunks, decompress, ensure total == w*h*4
        let body = &payload[16..];
        let mut p = 0usize;
        let mut total = 0usize;
        while p + 20 <= body.len() && &body[p..p + 4] == b"KCBC" {
            let clen = le_u32(body, p + 16) as usize;
            let seg = &body[p + 20..p + 20 + clen];
            let mut out = vec![0u8; (w * h * 4) as usize];
            let n = lzfse::decode_buffer(seg, &mut out).unwrap();
            total += n;
            p += 20 + clen;
        }
        assert_eq!(total, (w * h * 4) as usize);
    }
}
