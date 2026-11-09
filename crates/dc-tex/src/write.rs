//! A writer for our own textures: one level of 32-bit texels in the container of FMT_DTX.md, so a mod can ship an
//! original picture the client's reader opens (CLI-1612). Provenance: ORIGINAL, from the spec's header layout.
// SPEC: FMT_DTX.md §2 (header), §4.1 (level sizes), §3 (format 3: B, G, R, A bytes per texel)

use crate::header::DtxHeader;

/// A single-level 32-bit DTX of `width` x `height` RGBA texels (`rgba` row-major, 4 bytes per texel). `None` when the
/// sizes do not match or are 0.
pub fn write_bgra8(width: u16, height: u16, rgba: &[u8]) -> Option<Vec<u8>> {
    let n = (width as usize).checked_mul(height as usize)?;
    if n == 0 || rgba.len() != n.checked_mul(4)? {
        return None;
    }
    let header = DtxHeader {
        width,
        height,
        level_count: 1,
        ..DtxHeader::default()
    };
    let mut out = header.to_bytes().to_vec();
    out.reserve(rgba.len());
    for px in rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    Some(out)
}

/// Levels a chain keeps: down to the smallest side of 4, the last whole DXT block (FMT_DTX.md §3 stores no rounding
/// below that), and at most what the uploader takes (8).
fn chain_levels(width: u32, height: u32) -> u16 {
    let mut n = 1u16;
    let (mut w, mut h) = (width, height);
    while w.min(h) >= 8 && n < 8 {
        w /= 2;
        h /= 2;
        n += 1;
    }
    n
}

/// A box-filtered half of an RGBA picture.
fn half(w: u32, h: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    let at = |x: u32, y: u32, c: usize| {
        rgba[((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize + c] as u32
    };
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let sum = at(x * 2, y * 2, c)
                    + at(x * 2 + 1, y * 2, c)
                    + at(x * 2, y * 2 + 1, c)
                    + at(x * 2 + 1, y * 2 + 1, c);
                out[((y * nw + x) * 4) as usize + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    (nw, nh, out)
}

/// One DXT1 block from 16 RGBA texels: end points along the colours' widest diagonal, four-colour mode.
// SPEC: the public S3TC / BC1 block definition (as decode.rs reads it)
fn dxt1_block(px: &[[u8; 3]; 16]) -> [u8; 8] {
    let (mut lo, mut hi) = ([255u8; 3], [0u8; 3]);
    for p in px {
        for c in 0..3 {
            lo[c] = lo[c].min(p[c]);
            hi[c] = hi[c].max(p[c]);
        }
    }
    // the bounding box has two diagonals on each pair of axes: pick the signs by how the colours lean
    let mid = [0, 1, 2].map(|c| (lo[c] as i32 + hi[c] as i32) / 2);
    let lean = |a: usize, b: usize| {
        px.iter()
            .map(|p| (p[a] as i32 - mid[a]) * (p[b] as i32 - mid[b]))
            .sum::<i32>()
    };
    let (mut c0, mut c1) = (hi, lo);
    if lean(0, 1) < 0 {
        std::mem::swap(&mut c0[1], &mut c1[1]);
    }
    if lean(0, 2) < 0 {
        std::mem::swap(&mut c0[2], &mut c1[2]);
    }
    let pack = |c: [u8; 3]| -> u16 {
        ((c[0] as u16 >> 3) << 11) | ((c[1] as u16 >> 2) << 5) | (c[2] as u16 >> 3)
    };
    let expand = |v: u16| -> [i32; 3] {
        let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
        [
            (r as i32 * 255 + 15) / 31,
            (g as i32 * 255 + 31) / 63,
            (b as i32 * 255 + 15) / 31,
        ]
    };
    let (mut e0, mut e1) = (pack(c0), pack(c1));
    if e0 < e1 {
        std::mem::swap(&mut e0, &mut e1);
    }
    let mut out = [0u8; 8];
    out[0..2].copy_from_slice(&e0.to_le_bytes());
    out[2..4].copy_from_slice(&e1.to_le_bytes());
    if e0 == e1 {
        return out; // one colour: every index 0
    }
    let (a, b) = (expand(e0), expand(e1));
    let palette = [
        a,
        b,
        [0, 1, 2].map(|c| (2 * a[c] + b[c]) / 3),
        [0, 1, 2].map(|c| (a[c] + 2 * b[c]) / 3),
    ];
    let mut bits = 0u32;
    for (i, p) in px.iter().enumerate() {
        let mut best = (i32::MAX, 0u32);
        for (k, q) in palette.iter().enumerate() {
            let d: i32 = (0..3).map(|c| (p[c] as i32 - q[c]).pow(2)).sum();
            if d < best.0 {
                best = (d, k as u32);
            }
        }
        bits |= best.1 << (i * 2);
    }
    out[4..8].copy_from_slice(&bits.to_le_bytes());
    out
}

/// A whole opaque picture as DXT1 with a mip chain, a sixth of the 32-bit size with mips included, and without the
/// shimmer a floor shows at a distance on one level. `None` for a size that is not a power of two of at least 4
/// (the block format and the chain need those), or for wrong data.
pub fn write_dxt1_mipped(width: u16, height: u16, rgba: &[u8]) -> Option<Vec<u8>> {
    let (w, h) = (width as u32, height as u32);
    if rgba.len() != (w as usize).checked_mul(h as usize)?.checked_mul(4)?
        || !w.is_power_of_two()
        || !h.is_power_of_two()
        || w.min(h) < 4
    {
        return None;
    }
    let levels = chain_levels(w, h);
    let header = DtxHeader {
        width,
        height,
        level_count: levels,
        format: crate::PixelFormat::Dxt1.code(),
        ..DtxHeader::default()
    };
    let mut out = header.to_bytes().to_vec();
    let (mut lw, mut lh, mut pixels) = (w, h, rgba.to_vec());
    for level in 0..levels {
        for by in (0..lh).step_by(4) {
            for bx in (0..lw).step_by(4) {
                let mut px = [[0u8; 3]; 16];
                for y in 0..4 {
                    for x in 0..4 {
                        let i = (((by + y) * lw + bx + x) * 4) as usize;
                        px[(y * 4 + x) as usize] = [pixels[i], pixels[i + 1], pixels[i + 2]];
                    }
                }
                out.extend_from_slice(&dxt1_block(&px));
            }
        }
        if level + 1 < levels {
            let (nw, nh, next) = half(lw, lh, &pixels);
            lw = nw;
            lh = nh;
            pixels = next;
        }
    }
    Some(out)
}

/// One DXT1 block of a cut-out picture: three colours from the opaque texels and index 3 for the holes (the block
/// definition's `c0 <= c1` mode). A block with no holes goes the ordinary way.
// SPEC: the public S3TC / BC1 block definition (as decode.rs reads it)
fn dxt1_cutout_block(px: &[[u8; 4]; 16], cut: u8) -> [u8; 8] {
    let holes = px.iter().filter(|p| p[3] < cut).count();
    if holes == 0 {
        return dxt1_block(&px.map(|p| [p[0], p[1], p[2]]));
    }
    let pack = |c: [u8; 3]| -> u16 {
        ((c[0] as u16 >> 3) << 11) | ((c[1] as u16 >> 2) << 5) | (c[2] as u16 >> 3)
    };
    let expand = |v: u16| -> [i32; 3] {
        let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
        [
            (r as i32 * 255 + 15) / 31,
            (g as i32 * 255 + 31) / 63,
            (b as i32 * 255 + 15) / 31,
        ]
    };
    let mut bits = 0u32;
    let (mut e0, mut e1) = (0u16, 0u16);
    if holes < 16 {
        let (mut lo, mut hi) = ([255u8; 3], [0u8; 3]);
        for p in px.iter().filter(|p| p[3] >= cut) {
            for c in 0..3 {
                lo[c] = lo[c].min(p[c]);
                hi[c] = hi[c].max(p[c]);
            }
        }
        (e0, e1) = (pack(lo), pack(hi));
        if e0 > e1 {
            std::mem::swap(&mut e0, &mut e1);
        }
    }
    let (a, b) = (expand(e0), expand(e1));
    let palette = [a, b, [0, 1, 2].map(|c| (a[c] + b[c]) / 2)];
    for (i, p) in px.iter().enumerate() {
        let index = if p[3] < cut {
            3
        } else {
            let mut best = (i32::MAX, 0u32);
            for (k, q) in palette.iter().enumerate() {
                let d: i32 = (0..3).map(|c| (p[c] as i32 - q[c]).pow(2)).sum();
                if d < best.0 {
                    best = (d, k as u32);
                }
            }
            best.1
        };
        bits |= index << (i * 2);
    }
    let mut out = [0u8; 8];
    out[0..2].copy_from_slice(&e0.to_le_bytes());
    out[2..4].copy_from_slice(&e1.to_le_bytes());
    out[4..8].copy_from_slice(&bits.to_le_bytes());
    out
}

/// A picture with holes, as DXT1 with a mip chain and the alpha reference the renderer tests against (FMT_DTX.md 7):
/// paint on a wall, a sign's outline. Each smaller level takes its colour from the opaque texels only, so that the
/// holes' colour does not creep in at a distance. `None` as for [`write_dxt1_mipped`].
pub fn write_dxt1_cutout(width: u16, height: u16, rgba: &[u8]) -> Option<Vec<u8>> {
    const CUT: u8 = 128;
    let (w, h) = (width as u32, height as u32);
    if rgba.len() != (w as usize).checked_mul(h as usize)?.checked_mul(4)?
        || !w.is_power_of_two()
        || !h.is_power_of_two()
        || w.min(h) < 4
    {
        return None;
    }
    let levels = chain_levels(w, h);
    let mut header = DtxHeader {
        width,
        height,
        level_count: levels,
        format: crate::PixelFormat::Dxt1.code(),
        ..DtxHeader::default()
    };
    header.set_command(&format!("AlphaRef {CUT}"));
    let mut out = header.to_bytes().to_vec();
    let (mut lw, mut lh, mut pixels) = (w, h, rgba.to_vec());
    for level in 0..levels {
        for by in (0..lh).step_by(4) {
            for bx in (0..lw).step_by(4) {
                let mut px = [[0u8; 4]; 16];
                for y in 0..4 {
                    for x in 0..4 {
                        let i = (((by + y) * lw + bx + x) * 4) as usize;
                        px[(y * 4 + x) as usize] =
                            [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]];
                    }
                }
                out.extend_from_slice(&dxt1_cutout_block(&px, CUT));
            }
        }
        if level + 1 < levels {
            let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
            let mut next = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    let (mut sum, mut seen) = ([0u32; 3], 0u32);
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let i = (((y * 2 + dy).min(lh - 1) * lw + (x * 2 + dx).min(lw - 1)) * 4)
                            as usize;
                        if pixels[i + 3] >= CUT {
                            for c in 0..3 {
                                sum[c] += pixels[i + c] as u32;
                            }
                            seen += 1;
                        }
                    }
                    let at = ((y * nw + x) * 4) as usize;
                    // no pixel seen: the texel keeps what it had
                    for c in 0..3 {
                        if let Some(v) = sum[c].checked_div(seen) {
                            next[at + c] = v as u8;
                        }
                    }
                    // half covered stays: thin strokes last further away
                    next[at + 3] = if seen >= 2 { 255 } else { 0 };
                }
            }
            lw = nw;
            lh = nh;
            pixels = next;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DtxFile;

    #[test]
    fn a_dxt1_chain_reads_back_close_to_the_picture() {
        // a gradient: every level should decode near its own average brightness
        let mut rgba = Vec::new();
        for y in 0..32u32 {
            for x in 0..32u32 {
                rgba.extend_from_slice(&[(x * 8) as u8, (y * 8) as u8, 64, 255]);
            }
        }
        let bytes = write_dxt1_mipped(32, 32, &rgba).unwrap();
        let f = DtxFile::parse(&bytes).unwrap();
        assert_eq!(f.format(), crate::PixelFormat::Dxt1);
        assert_eq!(f.level_count(), 4, "32, 16, 8 and 4");
        assert_eq!(f.level_width(3), Some(4));
        let decoded = f.decode_rgba8(0);
        assert_eq!(decoded.len(), rgba.len());
        let worst = decoded
            .iter()
            .zip(&rgba)
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap();
        assert!(worst <= 24, "block error {worst}");
    }

    #[test]
    fn a_cut_out_keeps_its_holes_and_says_so() {
        // the left half paint, the right half a hole
        let mut rgba = Vec::new();
        for _y in 0..8 {
            for x in 0..8 {
                rgba.extend_from_slice(&if x < 4 {
                    [200, 40, 40, 255]
                } else {
                    [0, 0, 0, 0]
                });
            }
        }
        let bytes = write_dxt1_cutout(8, 8, &rgba).unwrap();
        let dtx = crate::DtxFile::parse(&bytes).unwrap();
        assert_eq!(dtx.commands().unwrap().alpha_test_value, Some(128));
        let out = dtx.decode_rgba8(0);
        let at = |x: usize, c: usize| out[x * 4 + c];
        assert_eq!(at(1, 3), 255);
        assert!(at(1, 0) > 180 && at(1, 1) < 60, "the paint's colour");
        assert_eq!(at(6, 3), 0, "the hole");
        assert!(write_dxt1_cutout(6, 8, &[0; 6 * 8 * 4]).is_none());
    }

    #[test]
    fn dxt1_needs_a_power_of_two_of_at_least_four() {
        assert!(write_dxt1_mipped(6, 8, &[0; 6 * 8 * 4]).is_none());
        assert!(write_dxt1_mipped(2, 2, &[0; 2 * 2 * 4]).is_none());
        assert!(write_dxt1_mipped(4, 4, &[0; 4]).is_none());
        assert!(write_dxt1_mipped(4, 4, &[0; 4 * 4 * 4]).is_some());
    }

    #[test]
    fn a_written_texture_reads_back() {
        let rgba: Vec<u8> = (0..4 * 4 * 2).map(|i| i as u8).collect();
        let bytes = write_bgra8(4, 2, &rgba).unwrap();
        let f = DtxFile::parse(&bytes).unwrap();
        assert_eq!((f.header().width, f.header().height), (4, 2));
        assert_eq!(f.trailing(), 0);
        assert!(write_bgra8(4, 2, &rgba[..8]).is_none());
        assert!(write_bgra8(0, 2, &[]).is_none());
    }
}
