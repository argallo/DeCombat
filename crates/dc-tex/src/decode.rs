//! Level decoding to RGBA8.
//!
//! DXT1/3/5 follow the public S3TC/BC1-3 block definitions; code 3 follows SPEC: FMT_DTX.md §3 (memory
//! order B,G,R,A).

use crate::{DtxError, PixelFormat};

pub(crate) fn decode_level(
    fmt: PixelFormat,
    w: u32,
    h: u32,
    data: &[u8],
    strict: bool,
) -> Result<Vec<u8>, DtxError> {
    let px = (w as usize)
        .checked_mul(h as usize)
        .ok_or(DtxError::Overflow)?;
    let out_len = px.checked_mul(4).ok_or(DtxError::Overflow)?;
    match fmt {
        PixelFormat::Bgra8 => {
            if data.len() < out_len && strict {
                return Err(DtxError::ShortLevel {
                    need: out_len,
                    have: data.len(),
                });
            }
            let mut out = vec![0u8; out_len];
            for (o, i) in out
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(data.as_chunks::<4>().0)
            {
                o.copy_from_slice(&[i[2], i[1], i[0], i[3]]);
            }
            Ok(out)
        }
        PixelFormat::Dxt1 | PixelFormat::Dxt3 | PixelFormat::Dxt5 => {
            let block = if fmt == PixelFormat::Dxt1 { 8usize } else { 16 };
            let bw = (w as usize).div_ceil(4);
            let bh = (h as usize).div_ceil(4);
            let need = bw
                .checked_mul(bh)
                .and_then(|n| n.checked_mul(block))
                .ok_or(DtxError::Overflow)?;
            if strict && data.len() < need {
                return Err(DtxError::ShortLevel {
                    need,
                    have: data.len(),
                });
            }
            let mut out = vec![0u8; out_len];
            let stride = w as usize * 4;
            for by in 0..bh {
                for bx in 0..bw {
                    let off = (by * bw + bx) * block;
                    let Some(chunk) = data.get(off..off + block) else {
                        continue; // lenient: missing block stays zero (transparent black)
                    };
                    let texels = match fmt {
                        // DXT1 alpha comes from the colour block itself.
                        PixelFormat::Dxt1 => decode_colour(&chunk[0..8], true),
                        PixelFormat::Dxt3 => {
                            let mut t = decode_colour(&chunk[8..16], false);
                            for (i, px) in t.iter_mut().enumerate() {
                                let byte = chunk[i / 2];
                                let nib = if i % 2 == 0 { byte & 0xF } else { byte >> 4 };
                                px[3] = nib * 17;
                            }
                            t
                        }
                        _ => {
                            let mut t = decode_colour(&chunk[8..16], false);
                            let alphas = decode_alpha5(&chunk[0..8]);
                            for (px, a) in t.iter_mut().zip(alphas) {
                                px[3] = a;
                            }
                            t
                        }
                    };
                    for ty in 0..4 {
                        let y = by * 4 + ty;
                        if y >= h as usize {
                            break;
                        }
                        for tx in 0..4 {
                            let x = bx * 4 + tx;
                            if x >= w as usize {
                                break;
                            }
                            let o = y * stride + x * 4;
                            out[o..o + 4].copy_from_slice(&texels[ty * 4 + tx]);
                        }
                    }
                }
            }
            Ok(out)
        }
        other => Err(DtxError::UnsupportedDecode(other.code())),
    }
}

fn expand565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u8;
    let g = ((c >> 5) & 63) as u8;
    let b = (c & 31) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

fn mix(a: u8, b: u8, wa: u32, wb: u32, div: u32) -> u8 {
    ((a as u32 * wa + b as u32 * wb) / div) as u8
}

/// Decodes the 8-byte colour block into 16 RGBA texels (row-major). `dxt1` enables the 3-colour mode
/// with a transparent fourth entry when `c0 <= c1`.
fn decode_colour(b: &[u8], dxt1: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let e0 = expand565(c0);
    let e1 = expand565(c1);
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [e0[0], e0[1], e0[2], 255];
    pal[1] = [e1[0], e1[1], e1[2], 255];
    if c0 > c1 || !dxt1 {
        for k in 0..3 {
            pal[2][k] = mix(e0[k], e1[k], 2, 1, 3);
            pal[3][k] = mix(e0[k], e1[k], 1, 2, 3);
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for k in 0..3 {
            pal[2][k] = mix(e0[k], e1[k], 1, 1, 2);
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let mut out = [[0u8; 4]; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[((bits >> (2 * i)) & 3) as usize];
    }
    out
}

/// Decodes the 8-byte interpolated-alpha block into 16 alpha values.
fn decode_alpha5(b: &[u8]) -> [u8; 16] {
    let a0 = b[0] as u32;
    let a1 = b[1] as u32;
    let mut pal = [0u8; 8];
    pal[0] = a0 as u8;
    pal[1] = a1 as u8;
    if a0 > a1 {
        for i in 1..7u32 {
            pal[(i + 1) as usize] = (((7 - i) * a0 + i * a1) / 7) as u8;
        }
    } else {
        for i in 1..5u32 {
            pal[(i + 1) as usize] = (((5 - i) * a0 + i * a1) / 5) as u8;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (i, v) in b[2..8].iter().enumerate() {
        bits |= (*v as u64) << (8 * i);
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[((bits >> (3 * i)) & 7) as usize];
    }
    out
}
