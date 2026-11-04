//! Static lightmaps of kind-2 world sections: run-length unpacking and conversion to RGBA8.

/// Largest lightmap the engine's scratch buffer holds (0x30000 bytes = 256 x 256 x 3).
// SPEC: RENDER_2009.md §6.1 (0x30000-byte scratch buffer, 256 x 256 maximum)
pub const MAX_LIGHTMAP_BYTES: usize = 0x30000;

/// Byte order of one stored lightmap texel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TexelOrder {
    /// Bytes B, G, R in memory: the first reading of the upload converter's channel masks (R 0xff0000, G 0x00ff00,
    /// B 0x0000ff over a 24-bit source, engine+0x183f40). Kept for differential screenshots: the 2009 frames
    /// contradict it (see [`TexelOrder::Rgb`]).
    // SPEC: RENDER_2009.md §6.1 (engine+0x183f40 masks)
    Bgr,
    /// Bytes R, G, B in memory: the first stored byte is red, as in the light grid of the same packing
    /// (RENDER_2009.md §6.3 / Appendix M5).
    // OBSERVED: capture 20261025_202429_sbs4-real/driver/rest_1.png (2009 client, JUNK_FLEA spawn
    // view) against the same view drawn by dc-render (reports/client-render2): with R, G, B the tinted lightmap areas
    // (the sun patch on the far floor, the blue-lit pillar, the ceiling) match within 1..4 % per channel after the
    // gamma ramp, with B, G, R they are off by up to 12 % in opposite directions for red and blue
    // DC-DEFAULT: REN-20 (proposed), see the staged rows (resolves RENDER_2009.md §16 item 3 by observation; the
    // converter table engine+0x82b90 / VA 0x6cd360 is not decoded)
    #[default]
    Rgb,
}

/// Unpacks a lightmap payload to `w * h * 3` bytes. Returns `None` unless the payload expands to exactly that
/// size (OBSERVED: all 6,274 ROM payloads do) or when the size exceeds the engine's buffer.
///
/// Packing: repeat until the input is consumed: control byte `c`, `run = (c & 0x7f) + 1`; with `c & 0x80` one
/// 3-byte texel repeated `run` times, else `run` literal 3-byte texels.
// SPEC: RENDER_2009.md §6.1 (engine+0x4fbb0), FMT_DAT85.md Addendum (lightmap RLE = light-grid RLE, §8)
pub fn decode_lightmap(payload: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let total = (w as usize).checked_mul(h as usize)?.checked_mul(3)?;
    if total == 0 || total > MAX_LIGHTMAP_BYTES {
        return None;
    }
    let mut out = Vec::with_capacity(total);
    let mut p = payload;
    while let Some((&ctl, rest)) = p.split_first() {
        let run = (ctl & 0x7f) as usize + 1;
        if ctl & 0x80 != 0 {
            let texel = rest.get(..3)?;
            if out.len() + 3 * run > total {
                return None;
            }
            for _ in 0..run {
                out.extend_from_slice(texel);
            }
            p = &rest[3..];
        } else {
            let lit = rest.get(..3 * run)?;
            if out.len() + lit.len() > total {
                return None;
            }
            out.extend_from_slice(lit);
            p = &rest[3 * run..];
        }
    }
    (out.len() == total).then_some(out)
}

/// Converts unpacked texels to RGBA8 with alpha 255 (the lightmap slot has no alpha channel).
// SPEC: RENDER_2009.md §10.1 (DeCombat: lightmaps -> Bgra8Unorm with alpha 255)
pub fn lightmap_rgba(texels: &[u8], order: TexelOrder) -> Vec<u8> {
    let mut out = Vec::with_capacity(texels.len() / 3 * 4);
    for t in texels.as_chunks::<3>().0 {
        match order {
            TexelOrder::Bgr => out.extend_from_slice(&[t[2], t[1], t[0], 255]),
            TexelOrder::Rgb => out.extend_from_slice(&[t[0], t[1], t[2], 255]),
        }
    }
    out
}

/// Packs `w * h` RGB texels the way the decoder expects (runs of equal texels, else literals). Used by the
/// synthetic tests and by our own world writer.
pub fn pack_lightmap(texels: &[u8]) -> Vec<u8> {
    dc_world::synth::pack_light_grid(texels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_exact_size() {
        let mut t = Vec::new();
        for i in 0..64u32 {
            let v = if i < 20 { 7 } else { (i * 3) as u8 };
            t.extend_from_slice(&[v, v.wrapping_add(1), v.wrapping_add(2)]);
        }
        let p = pack_lightmap(&t);
        assert!(p.len() < t.len());
        assert_eq!(decode_lightmap(&p, 8, 8).as_deref(), Some(&t[..]));
        // wrong declared size: too small and too large both refuse
        assert_eq!(decode_lightmap(&p, 4, 8), None);
        assert_eq!(decode_lightmap(&p, 16, 8), None);
    }

    #[test]
    fn hostile_payloads_do_not_panic() {
        assert_eq!(decode_lightmap(&[], 1, 1), None);
        assert_eq!(decode_lightmap(&[0x80], 1, 1), None);
        assert_eq!(decode_lightmap(&[0x05, 1, 2], 2, 1), None);
        assert_eq!(decode_lightmap(&[0xff, 1, 2, 3], 1024, 1024), None);
        assert_eq!(decode_lightmap(&[0x80, 1, 2, 3], 0, 1), None);
    }

    #[test]
    fn default_order_is_red_first() {
        assert_eq!(TexelOrder::default(), TexelOrder::Rgb);
        assert_eq!(
            lightmap_rgba(&[200, 100, 50], TexelOrder::default()),
            vec![200, 100, 50, 255]
        );
    }

    #[test]
    fn texel_order() {
        assert_eq!(
            lightmap_rgba(&[1, 2, 3], TexelOrder::Bgr),
            vec![3, 2, 1, 255]
        );
        assert_eq!(
            lightmap_rgba(&[1, 2, 3], TexelOrder::Rgb),
            vec![1, 2, 3, 255]
        );
    }
}
