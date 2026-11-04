//! World textures: DTX files from the user's ROM decoded to RGBA8 mip chains, plus the traits the bucket
//! selector needs.

use crate::bucket::TextureTraits;
use crate::source::TextureIndex;
use dc_tex::{DtxFile, PixelFormat};
use std::collections::HashMap;

/// A decoded texture ready for upload: RGBA8 levels, largest first.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureImage {
    pub width: u32,
    pub height: u32,
    /// `levels[i]` is `max(1, width >> i) * max(1, height >> i) * 4` bytes.
    pub levels: Vec<Vec<u8>>,
    /// Alpha-test reference from the DTX command, when the texture carries one.
    pub alpha_cutoff: Option<u8>,
    /// LOD bias from the DTX command (0 when absent).
    pub mip_bias: f32,
    /// A camo effect drawn over this texture (never from a DTX file).
    pub fx: Option<std::sync::Arc<TextureFx>>,
}

/// A camo effect on a model's texture (DC_DEFAULTS.md VAR-4): `tile` drifts and pulses over the model, where `mask` lets it.
// DC-DEFAULT: VAR-4 (proposed) our own effect; the 2009 renderer has none like it
#[derive(Debug, Clone, PartialEq)]
pub struct TextureFx {
    pub tile: TextureImage,
    pub mask: TextureImage,
    /// How many times the tile repeats across the texture.
    pub tiles: f32,
    /// Drift, in repeats per second.
    pub speed: [f32; 2],
    /// Lowest and highest gain.
    pub gain: [f32; 2],
    /// Seconds per pulse (0: steady at the highest gain).
    pub seconds: f32,
}

impl TextureImage {
    /// A single-level image from RGBA8 bytes (synthetic scenes, fallbacks).
    pub fn rgba(width: u32, height: u32, rgba: Vec<u8>) -> TextureImage {
        TextureImage {
            width,
            height,
            levels: vec![rgba],
            alpha_cutoff: None,
            mip_bias: 0.0,
            fx: None,
        }
    }

    /// A debug stand-in for a texture that resolves to nothing: an 8x8 magenta checker so gaps are visible on
    /// screen. World sections use [`TextureImage::unbound`] instead (what the 2009 renderer samples).
    pub fn missing() -> TextureImage {
        let mut px = Vec::with_capacity(8 * 8 * 4);
        for y in 0..8 {
            for x in 0..8 {
                if (x / 2 + y / 2) % 2 == 0 {
                    px.extend_from_slice(&[255, 0, 255, 255]);
                } else {
                    px.extend_from_slice(&[24, 24, 24, 255]);
                }
            }
        }
        TextureImage::rgba(8, 8, px)
    }

    /// What a world section samples on a stage whose texture did not load: opaque black 1x1.
    ///
    /// The section loader keeps a null slot for a name that does not load (it only logs), and the per-batch bind
    /// unbinds the stage for a null slot (SetTexture(stage, NULL)); the engine's own default-texture fallback
    /// (`textures\default_texture.dtx`, engine+0x19c40) belongs to another texture list and that file is in no
    /// ROM archive anyway. A Direct3D 9 sampler with no texture bound reads (0, 0, 0, 1) (PUBLIC: Wine's d3d9
    /// visual test `unbound_sampler_test`, run against Windows drivers). Only the two-texture blend buckets 10 and
    /// 20 still sample such a stage: a missing first texture takes the bucket selector's no-texture path
    /// ([`RomTextures::traits`]).
    // DISASM: engine+0x1818e0 (section texture slots = texture manager result, null + "Error loading texture" log)
    // DISASM: engine+0x1020b0 (null texture or failed upload -> device SetTexture(stage, NULL))
    pub fn unbound() -> TextureImage {
        TextureImage::rgba(1, 1, vec![0, 0, 0, 255])
    }

    /// Plain white 1x1 (unbound second stage).
    pub fn white() -> TextureImage {
        TextureImage::rgba(1, 1, vec![255; 4])
    }

    /// Adds the box-filtered levels down to 1x1 that a single-level image lacks. Off by default in the viewer:
    /// the 2009 uploader creates only the stored levels (FMT_DTX.md §6 rule 3).
    pub fn generate_mips(&mut self) {
        let Some(last) = self.levels.len().checked_sub(1) else {
            return;
        };
        let mut w = level_dim(self.width, last as u32);
        let mut h = level_dim(self.height, last as u32);
        while w > 1 || h > 1 {
            let i = self.levels.len() - 1;
            if self.levels[i].len() != (w * h * 4) as usize {
                break; // chain inconsistent; leave it
            }
            let nw = (w / 2).max(1);
            let nh = (h / 2).max(1);
            let src = &self.levels[i];
            let mut out = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    for c in 0..4 {
                        let mut sum = 0u32;
                        let mut n = 0u32;
                        for dy in 0..2 {
                            for dx in 0..2 {
                                let sx = (2 * x + dx).min(w - 1);
                                let sy = (2 * y + dy).min(h - 1);
                                sum += src[((sy * w + sx) * 4 + c) as usize] as u32;
                                n += 1;
                            }
                        }
                        out[((y * nw + x) * 4 + c) as usize] = ((sum + n / 2) / n) as u8;
                    }
                }
            }
            self.levels.push(out);
            w = nw;
            h = nh;
        }
    }
}

fn level_dim(d: u32, i: u32) -> u32 {
    (d >> i).max(1)
}

/// Bucket traits of a parsed DTX: the command mode and flag 0x1.
// SPEC: RENDER_2009.md §5.2 (`mode` = DTX command mode, `hi` = DTX flag 0x1), FMT_DTX.md §7
pub fn traits_of(dtx: &DtxFile) -> TextureTraits {
    let mode = dtx
        .commands()
        .ok()
        .and_then(|c| c.material.map(|m| m.kind.mode()))
        .unwrap_or(0);
    TextureTraits {
        present: true,
        mode,
        hi: dtx.header().flags & 0x1 != 0,
    }
}

/// Decodes the levels the 2009 uploader would create: start at `upload_skip` (0 for block formats, which go
/// up natively), at most `upload_levels` (0 meaning 8) levels, each decoded to RGBA8. Levels whose size does
/// not follow the `max(1, side >> i)` chain wgpu requires end the chain.
// SPEC: FMT_DTX.md §6 rules 1 and 3 (first level, level count), RENDER_2009.md §10.1 (decode to RGBA8 when
// block formats are not uploaded natively)
pub fn decode_dtx(dtx: &DtxFile) -> Option<TextureImage> {
    let _decode = crate::perf::span(crate::perf::Counter::DecodeNs);
    let img = decode_dtx_levels(dtx)?;
    crate::perf::add(
        crate::perf::Counter::DecodeBytes,
        img.levels.iter().map(|l| l.len() as u64).sum(),
    );
    Some(img)
}

fn decode_dtx_levels(dtx: &DtxFile) -> Option<TextureImage> {
    let h = dtx.header();
    let block = matches!(
        dtx.format(),
        PixelFormat::Dxt1 | PixelFormat::Dxt3 | PixelFormat::Dxt5
    );
    let n = dtx.level_count();
    if n == 0 {
        return None;
    }
    let first = if block {
        0
    } else {
        (h.upload_skip as usize).min(n - 1)
    };
    let count = (h.effective_upload_levels() as usize).min(n - first).max(1);
    let w0 = dtx.level_width(first)?;
    let h0 = dtx.level_height(first)?;
    if w0 == 0 || h0 == 0 {
        return None;
    }
    let mut levels = Vec::with_capacity(count);
    for (k, li) in (first..first + count).enumerate() {
        let (w, hh) = (dtx.level_width(li)?, dtx.level_height(li)?);
        if w != level_dim(w0, k as u32) || hh != level_dim(h0, k as u32) || w == 0 || hh == 0 {
            break;
        }
        let px = dtx.try_decode_rgba8(li, 0).ok()?;
        levels.push(px);
    }
    if levels.is_empty() {
        return None;
    }
    let cmd = dtx.commands().ok();
    Some(TextureImage {
        width: w0,
        height: h0,
        levels,
        alpha_cutoff: cmd
            .as_ref()
            .and_then(|c| c.alpha_test_value)
            .map(|v| v.clamp(0, 255) as u8),
        mip_bias: cmd.and_then(|c| c.mip_bias).unwrap_or(0.0),
        fx: None,
    })
}

/// True for names whose last four characters are `.spr`, case-insensitive.
// SPEC: FMT_SPR_LTC_FX.md §2.1 (4-char suffix test for world surface textures)
pub fn is_sprite_name(name: &[u8]) -> bool {
    name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".spr")
}

/// The texture path of frame 0 of a sprite file, `None` for a file the engine would reject.
// SPEC: FMT_SPR_LTC_FX.md §2.2 (five u32 header words, then per frame u16 length + path; length > 1000 rejects)
pub fn sprite_first_frame(b: &[u8]) -> Option<&[u8]> {
    let word = |o: usize| {
        b.get(o..o + 4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
    };
    let frames = word(0)?;
    let fps = word(4)?;
    if frames == 0 || fps == 0 {
        return None;
    }
    let len = b.get(20..22).map(|w| u16::from_le_bytes([w[0], w[1]]))? as usize;
    if len == 0 || len > 1000 {
        return None;
    }
    b.get(22..22 + len)
}

/// Parsed textures of the user's ROM, cached by name.
#[derive(Debug)]
pub struct RomTextures<'i> {
    index: &'i TextureIndex,
    cache: HashMap<Vec<u8>, Option<DtxFile>>,
    pub missing: u64,
    pub unreadable: u64,
}

impl<'i> RomTextures<'i> {
    pub fn new(index: &'i TextureIndex) -> Self {
        RomTextures {
            index,
            cache: HashMap::new(),
            missing: 0,
            unreadable: 0,
        }
    }

    /// The parsed file, `None` when the name does not resolve or does not parse. A world texture name ending
    /// in `.spr` is an animated sprite: its first frame is used (the playback cursor starts on frame 0 of
    /// animation 0; animation over time is not run in this slice).
    // SPEC: FMT_SPR_LTC_FX.md §2.1 (world surface textures ending in `.spr` load as sprites, engine+0x1818e0 ->
    // engine+0x1786a0), §2.4 (cursor init engine+0xa72f0: frame 0)
    pub fn get(&mut self, name: &[u8]) -> Option<&DtxFile> {
        if !self.cache.contains_key(name) {
            let frame;
            let target: &[u8] = if is_sprite_name(name) {
                frame = match self.index.read(name) {
                    Ok(Some(b)) => sprite_first_frame(&b).map(|f| f.to_vec()),
                    _ => None,
                };
                match &frame {
                    Some(f) => f,
                    None => {
                        self.unreadable += 1;
                        self.cache.insert(name.to_vec(), None);
                        return None;
                    }
                }
            } else {
                name
            };
            let parsed = match self.index.read(target) {
                Ok(Some(bytes)) => match DtxFile::parse(&bytes) {
                    Ok(d) => Some(d),
                    Err(_) => {
                        self.unreadable += 1;
                        None
                    }
                },
                Ok(None) => {
                    self.missing += 1;
                    None
                }
                Err(_) => {
                    self.unreadable += 1;
                    None
                }
            };
            self.cache.insert(name.to_vec(), parsed);
        }
        self.cache.get(name).and_then(|d| d.as_ref())
    }

    /// Bucket traits for a section texture. A name that does not resolve or parse leaves the section's first
    /// texture slot null, and the bucket selector tests that slot: vertex-lit sections then draw flat (bucket 0),
    /// lightmapped texture sections are not drawn (RENDER_2009.md §5.2 "no texture").
    // DISASM: engine+0x1818e0 (slot 0 = texture manager result, 0 when the load fails)
    // DISASM: engine+0x136798 (the selector reads slot 0; null -> the no-texture branches)
    pub fn traits(&mut self, name: &[u8]) -> TextureTraits {
        match self.get(name) {
            Some(d) => traits_of(d),
            None => TextureTraits {
                present: false,
                mode: 0,
                hi: false,
            },
        }
    }

    /// Decoded image, or `None` when unresolvable (the caller substitutes [`TextureImage::missing`]).
    pub fn image(&mut self, name: &[u8]) -> Option<TextureImage> {
        self.get(name).and_then(decode_dtx)
    }

    /// Drops a parsed file once it has been uploaded.
    pub fn release(&mut self, name: &[u8]) {
        if let Some(slot) = self.cache.get_mut(name) {
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_tex::{DtxHeader, HEADER_LEN};

    fn bgra_file(w: u16, h: u16, levels: u16, flags: u32, cmd: &str) -> Vec<u8> {
        let mut hd = DtxHeader {
            width: w,
            height: h,
            level_count: levels,
            flags,
            ..DtxHeader::default()
        };
        hd.set_command(cmd);
        let mut b = hd.to_bytes().to_vec();
        assert_eq!(b.len(), HEADER_LEN);
        for i in 0..levels {
            let n = ((w as usize) >> i) * ((h as usize) >> i);
            for p in 0..n {
                b.extend_from_slice(&[i as u8, p as u8, 7, 200]);
            }
        }
        b
    }

    #[test]
    fn an_unresolved_name_is_a_null_slot() {
        let index = TextureIndex::default();
        let mut tex = RomTextures::new(&index);
        let t = tex.traits(b"textures\nowhere.dtx");
        assert!(!t.present, "no texture: the selector's no-texture path");
        assert_eq!(tex.missing, 1);
        assert!(tex.image(b"textures\nowhere.dtx").is_none());
        let u = TextureImage::unbound();
        assert_eq!((u.width, u.height, u.levels.len()), (1, 1, 1));
        assert_eq!(
            u.levels[0],
            vec![0, 0, 0, 255],
            "unbound stage reads opaque black"
        );
    }

    #[test]
    fn decode_levels_and_traits() {
        let f = DtxFile::parse(&bgra_file(8, 4, 3, 0x9, "AlphaRef 96")).unwrap();
        let img = decode_dtx(&f).unwrap();
        assert_eq!((img.width, img.height, img.levels.len()), (8, 4, 3));
        assert_eq!(img.levels[2].len(), 2 * 4);
        // stored B,G,R,A -> R,G,B,A
        assert_eq!(&img.levels[1][..4], &[7, 0, 1, 200]);
        assert_eq!(img.alpha_cutoff, Some(96));
        let t = traits_of(&f);
        assert!(t.hi && t.present && t.mode == 0);
    }

    #[test]
    fn sprite_frame_zero() {
        let mut b = Vec::new();
        for w in [2u32, 10, 1, 0, 0] {
            b.extend_from_slice(&w.to_le_bytes());
        }
        for f in [&b"a/one.dtx"[..], b"a/two.dtx"] {
            b.extend_from_slice(&(f.len() as u16).to_le_bytes());
            b.extend_from_slice(f);
        }
        assert_eq!(sprite_first_frame(&b), Some(&b"a/one.dtx"[..]));
        assert!(
            is_sprite_name(b"x/y.SPR") && !is_sprite_name(b".spr") && !is_sprite_name(b"y.dtx")
        );
        let mut bad = b.clone();
        bad[20..22].copy_from_slice(&1001u16.to_le_bytes());
        assert_eq!(sprite_first_frame(&bad), None);
        assert_eq!(sprite_first_frame(&b[..10]), None);
    }

    #[test]
    fn generated_mips_reach_one_texel() {
        let mut img = TextureImage::rgba(4, 2, vec![100; 4 * 2 * 4]);
        img.generate_mips();
        let dims: Vec<usize> = img.levels.iter().map(|l| l.len() / 4).collect();
        assert_eq!(dims, vec![8, 2, 1]);
        assert!(img.levels.iter().all(|l| l.iter().all(|&v| v == 100)));
    }
}
