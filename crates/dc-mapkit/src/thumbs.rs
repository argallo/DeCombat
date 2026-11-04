//! Thumbnails: a DTX (the user's ROM texture at run time, or an own texture) decoded to a small PNG
//! (MAPKIT_PLAN.md 5.2 `rom textures --thumbs`; DC-DEFAULT: MAP-55). Nothing here keeps a name or a byte of
//! the ROM: the caller hands in the bytes it read and gets PNG bytes back.
//!
//! The mip level nearest to the requested size is decoded (`dc_tex` decodes the 32-bit and DXT formats of the
//! ROM census) and box-averaged down to fit `max` on its longer side, aspect kept.

use crate::render_sw::RgbaImage;

/// Default thumbnail size (longer side, pixels).
pub const THUMB_SIZE: u32 = 64;

/// A decoded texture level as RGBA with the texture's full size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub image: RgbaImage,
    pub full_width: u32,
    pub full_height: u32,
}

/// Decode the mip level of `bytes` that best fits `max` (0 = the first level).
pub fn decode_dtx(bytes: &[u8], max: u32) -> Result<Decoded, String> {
    let f = dc_tex::DtxFile::parse(bytes).map_err(|e| e.to_string())?;
    let full_width = f.level_width(0).unwrap_or(0);
    let full_height = f.level_height(0).unwrap_or(0);
    let mut level = 0;
    if max > 0 {
        for i in 0..f.level_count() {
            let (w, h) = (
                f.level_width(i).unwrap_or(0),
                f.level_height(i).unwrap_or(0),
            );
            if w == 0 || h == 0 {
                break;
            }
            level = i;
            if w <= max && h <= max {
                break;
            }
        }
    }
    let w = f.level_width(level).unwrap_or(0);
    let h = f.level_height(level).unwrap_or(0);
    if w == 0 || h == 0 {
        return Err("texture has no pixels".into());
    }
    let data = f.try_decode_rgba8(level, 0).map_err(|e| e.to_string())?;
    if data.len() != w as usize * h as usize * 4 {
        return Err(format!("decoded {} bytes for {w} x {h}", data.len()));
    }
    Ok(Decoded {
        image: RgbaImage {
            width: w,
            height: h,
            data,
        },
        full_width,
        full_height,
    })
}

/// Fit an image into `max` on its longer side (no upscaling).
pub fn fit(img: &RgbaImage, max: u32) -> RgbaImage {
    if max == 0 || (img.width <= max && img.height <= max) {
        return img.clone();
    }
    let longer = img.width.max(img.height) as f32;
    let k = max as f32 / longer;
    img.resized(
        ((img.width as f32 * k).round() as u32).max(1),
        ((img.height as f32 * k).round() as u32).max(1),
    )
}

/// DTX bytes -> PNG bytes of at most `max` pixels on the longer side, plus the texture's full size.
pub fn dtx_thumb_png(bytes: &[u8], max: u32) -> Result<(Vec<u8>, (u32, u32)), String> {
    let d = decode_dtx(bytes, max)?;
    let small = fit(&d.image, max);
    Ok((small.png(), (d.full_width, d.full_height)))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_own_texture_round_trips_to_a_small_png() {
        // a 16 x 8 gradient written with the DTX writer, decoded and shrunk to 4 pixels wide
        let mut rgba = Vec::new();
        for y in 0..8u32 {
            for x in 0..16u32 {
                rgba.extend_from_slice(&[(x * 16) as u8, (y * 32) as u8, 7, 255]);
            }
        }
        let dtx = dc_tex::write_bgra8(16, 8, &rgba).unwrap();
        let d = decode_dtx(&dtx, 0).unwrap();
        assert_eq!((d.full_width, d.full_height), (16, 8));
        assert_eq!(d.image.get(15, 0), [240, 0, 7, 255]);
        let (png, full) = dtx_thumb_png(&dtx, 4).unwrap();
        assert_eq!(full, (16, 8));
        let back = image::load_from_memory(&png).unwrap();
        assert_eq!((back.width(), back.height()), (4, 2));
        assert!(decode_dtx(b"nope", 0).is_err());
    }

    #[test]
    fn a_sprite_header_names_its_first_frame() {
        let mut b = Vec::new();
        for w in [2u32, 10, 0, 0, 0] {
            b.extend_from_slice(&w.to_le_bytes());
        }
        b.extend_from_slice(&5u16.to_le_bytes());
        b.extend_from_slice(b"a.dtx");
        assert_eq!(sprite_first_frame(&b), Some(&b"a.dtx"[..]));
        assert_eq!(sprite_first_frame(&b[..10]), None);
        let mut zero = b.clone();
        zero[0] = 0;
        assert_eq!(sprite_first_frame(&zero), None);
    }
}
