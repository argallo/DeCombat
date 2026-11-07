//! The fall's screen-edge flash: a red radial vignette, clear in the middle of the screen and strongest at the
//! edges and corners, drawn as one full-screen image of a small radial-alpha texture made at load (the HUD painter
//! has only rectangles and textures; a stack of rectangle frames made the effect a square band).
//!
//! A fall has no bearing, so the 2009 directional arcs (a small ring about the crosshair) cannot show it; the
//! user asked for the screen edges instead, then for a circle rather than a square, and for it dimmer.

use super::compose::Tex;
use super::items::DrawItem;
use dc_ui_data::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};

/// The name the HUD's texture cache serves [`texture`] under (nothing in the ROM or a mod answers to it).
pub const TEXTURE: &str = "@dc:fall_vignette";

/// The texture is this many texels square, stretched over the whole screen: a circle in the texture is the ellipse
/// that touches the four screen edges at their midpoints. 256 keeps the alpha ramp (about 15 texels wide since the
/// clear radius went to 0.88) free of visible steps at 1080p.
pub const TEXELS: u32 = 256;

/// The vignette is clear inside this radius and fully tinted from [`FULL_RADIUS`] outward, measured in half
/// screens from the centre (1.0 is the middle of an edge, the corners lie at 1.41) and eased between the two.
// DC-DEFAULT: CLI-1847 (proposed; USER REPORT 2026-11-05 "too bright and as a square but should be a circle")
// clear inside 0.88 (0.80 until the owner's 2026-11-06 "show less red on screen when you fall"), full strength
// from 1.0 (the edges and corners), smoothstep between
pub const CLEAR_RADIUS: f32 = 0.88;
pub const FULL_RADIUS: f32 = 1.0;

/// The tint colour (the fallback damage arc's red) and its alpha at the edges and corners at full strength.
// DC-DEFAULT: CLI-1847 (proposed) red 220 / 30 / 20, peak alpha 100 of 255 (the first band's 200, halved)
pub const COLOUR: [u8; 3] = [220, 30, 20];
pub const PEAK_ALPHA: f32 = 90.0;

/// The texture's alpha (0..=1) at `radius` half screens from the centre.
pub fn profile(radius: f32) -> f32 {
    let t = ((radius - CLEAR_RADIUS) / (FULL_RADIUS - CLEAR_RADIUS)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The radial-alpha texture: white, alpha by [`profile`] of the distance from the centre.
pub fn texture() -> Tex {
    let n = TEXELS as usize;
    let mut rgba = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (
                (x as f32 + 0.5) / n as f32 * 2.0 - 1.0,
                (y as f32 + 0.5) / n as f32 * 2.0 - 1.0,
            );
            let a = (profile((u * u + v * v).sqrt()) * 255.0).round() as u8;
            rgba.extend_from_slice(&[255, 255, 255, a]);
        }
    }
    Tex {
        width: TEXELS,
        height: TEXELS,
        rgba,
    }
}

/// The tint alpha at `strength` (0..=1).
pub fn tint_alpha(strength: f32) -> u8 {
    (PEAK_ALPHA * strength.clamp(0.0, 1.0)).round() as u8
}

/// Pushes the vignette at `strength` (nothing at 0).
pub fn push_items(out: &mut Vec<DrawItem>, strength: f32, width: u32, height: u32) {
    let a = tint_alpha(strength);
    if strength.is_nan() || strength <= 0.0 || a == 0 || width == 0 || height == 0 {
        return;
    }
    out.push(DrawItem::Image {
        tex: TEXTURE.to_string(),
        src: [TEXELS as f32, TEXELS as f32],
        dst: [0.0, 0.0, VIRTUAL_WIDTH as f32, VIRTUAL_HEIGHT as f32],
        tint: [COLOUR[0], COLOUR[1], COLOUR[2], a],
        turn: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::compose::{compose, HudAssets, HudFonts, NoTextures};

    fn images(strength: f32) -> Vec<DrawItem> {
        let mut out = Vec::new();
        push_items(&mut out, strength, 1920, 1080);
        out
    }

    fn alpha_at(t: &Tex, u: f32, v: f32) -> u8 {
        let n = TEXELS as f32;
        let (x, y) = (((u + 1.0) / 2.0 * n) as u32, ((v + 1.0) / 2.0 * n) as u32);
        t.rgba[((y.min(TEXELS - 1) * TEXELS + x.min(TEXELS - 1)) * 4 + 3) as usize]
    }

    #[test]
    fn nothing_at_zero_strength() {
        for s in [0.0, -1.0, f32::NAN, 0.001] {
            assert!(images(s).is_empty(), "{s}");
        }
    }

    #[test]
    fn it_is_one_dimmer_tint_that_scales_with_the_strength() {
        let full = images(1.0);
        let [DrawItem::Image { tex, tint, dst, .. }] = full.as_slice() else {
            panic!("one image");
        };
        assert_eq!(tex, TEXTURE);
        assert_eq!(tint[..3], COLOUR);
        assert_eq!(tint[3] as f32, PEAK_ALPHA);
        assert!(tint[3] <= 110, "about half of the first band's 200");
        assert_eq!(*dst, [0.0, 0.0, 1024.0, 768.0], "the whole screen");
        let half = images(0.5);
        let [DrawItem::Image { tint, .. }] = half.as_slice() else {
            panic!("one image");
        };
        assert_eq!(tint[3], 45);
    }

    #[test]
    fn the_texture_is_clear_in_the_middle_and_full_on_a_circle_at_the_edges_and_corners() {
        let t = texture();
        assert_eq!((t.width, t.height), (TEXELS, TEXELS));
        assert_eq!(t.rgba.len(), (TEXELS * TEXELS * 4) as usize);
        assert!(t.rgba.chunks(4).all(|p| p[..3] == [255, 255, 255]));
        assert_eq!(alpha_at(&t, 0.0, 0.0), 0);
        // clear all the way to the clear radius, in every direction
        for (u, v) in [(0.5, 0.0), (0.0, -0.5), (0.35, 0.35), (-0.38, 0.38)] {
            assert_eq!(alpha_at(&t, u, v), 0, "({u}, {v})");
        }
        // full strength in every corner, nearly full at the middle of every edge (the edge is the full radius)
        for (u, v) in [(0.99, 0.99), (-0.99, 0.99)] {
            assert_eq!(alpha_at(&t, u, v), 255, "({u}, {v})");
        }
        for (u, v) in [(-0.99, 0.0), (0.99, 0.0), (0.0, -0.99), (0.0, 0.99)] {
            assert!(
                alpha_at(&t, u, v) >= 245,
                "({u}, {v}): {}",
                alpha_at(&t, u, v)
            );
        }
        // a circle, not a square: the same radius gives the same alpha on the axis and on the diagonal
        let on_axis = alpha_at(&t, 0.95, 0.0);
        let on_diagonal = alpha_at(&t, 0.6718, 0.6718);
        assert!(on_axis > 60 && on_axis < 200, "{on_axis}");
        assert!(
            on_axis.abs_diff(on_diagonal) <= 20,
            "{on_axis} vs {on_diagonal}"
        );
        // and it rises monotonically outward
        let ramp: Vec<u8> = (0..60)
            .map(|k| alpha_at(&t, 0.45 + k as f32 * 0.0085, 0.0))
            .collect();
        assert!(ramp.windows(2).all(|p| p[0] <= p[1]), "{ramp:?}");
    }

    /// The CPU compositor (the GPU overlay draws from the same primitives) at a 4:3 and a 16:9 target: the centre is
    /// untouched, the middle of every edge is tinted red below the peak alpha, a corner at least as much as an edge,
    /// and a point a quarter of the way in is clear.
    #[test]
    fn a_composed_frame_tints_the_edges_and_corners_below_the_peak_and_leaves_the_middle() {
        for (w, h) in [(640u32, 480u32), (1280, 720)] {
            let fonts = HudFonts::new(None, 16.0, None, h as f32 / 768.0);
            let mut assets = HudAssets::new(Box::new(NoTextures), fonts);
            let img = compose(&images(1.0), &mut assets, w, h);
            assert!(assets.missing.is_empty(), "{:?}", assets.missing);
            assert_eq!(img.pixel(w / 2, h / 2)[3], 0, "{w}x{h}: centre untouched");
            let edges = [(0, h / 2), (w - 1, h / 2), (w / 2, 0), (w / 2, h - 1)];
            let corners = [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)];
            for (x, y) in edges.into_iter().chain(corners) {
                let p = img.pixel(x, y);
                assert!(
                    p[3] >= 80 && p[3] as f32 <= PEAK_ALPHA,
                    "{w}x{h}: ({x}, {y}) {p:?}"
                );
                assert!(p[0] > p[1] + 100 && p[0] > p[2] + 100, "red: {p:?}");
            }
            let weakest_corner = corners.iter().map(|&(x, y)| img.pixel(x, y)[3]).min();
            let strongest_edge = edges.iter().map(|&(x, y)| img.pixel(x, y)[3]).max();
            assert!(
                weakest_corner >= strongest_edge,
                "{w}x{h}: {weakest_corner:?} < {strongest_edge:?}"
            );
            // fades inward: clear a quarter of the way in, partial on the way
            assert_eq!(img.pixel(w / 4, h / 2)[3], 0, "{w}x{h}");
            let mid = img.pixel((w as f32 * 0.05) as u32, h / 2)[3];
            assert!(mid > 0 && (mid as f32) < PEAK_ALPHA, "{w}x{h}: {mid}");
        }
    }
}
