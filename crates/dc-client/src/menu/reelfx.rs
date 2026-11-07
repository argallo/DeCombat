//! The case reel's look by rarity tier (CRATE-7): the tier colours and the textures of its effects (a soft glow, a
//! searchlight beam and a brass casing), made here: the glow and the beam as white shapes a draw item tints, the casing
//! in brass.
//!
//! Provenance: ORIGINAL.
// DC-DEFAULT: CRATE-7 (proposed; USER REQUEST 2026-11-06 "make it feel true to the game but also epic")

use std::sync::Arc;

use dc_bute::modpatch::Rarity;

use crate::hud::compose::Tex;

/// The names the reel draws its effect textures by ([`crate::hud::compose::HudAssets::texture`] makes them).
pub const GLOW: &str = "@dc:reel_glow";
pub const RAY: &str = "@dc:reel_ray";
pub const CASING: &str = "@dc:reel_casing";

/// The colour of a tier (bars, the mark's tell, the reveal), Common to Legendary.
pub fn colour(r: Rarity) -> [u8; 3] {
    match r {
        Rarity::Common => [185, 194, 207],
        Rarity::Rare => [79, 179, 255],
        Rarity::Epic => [196, 107, 255],
        Rarity::Legendary => [255, 197, 61],
    }
}

/// The dark shade of a tier (the winning cell's ground).
pub fn deep(r: Rarity) -> [u8; 3] {
    match r {
        Rarity::Common => [44, 50, 60],
        Rarity::Rare => [20, 48, 92],
        Rarity::Epic => [56, 24, 96],
        Rarity::Legendary => [92, 60, 10],
    }
}

/// The bar of an item that is no camo (a case's own items have no tier).
pub const PLAIN: [u8; 3] = [92, 106, 124];

/// How strong a tier's reveal is: 1 (Common) to 4 (Legendary).
pub fn strength(r: Rarity) -> u32 {
    r.index() as u32 + 1
}

/// The texture named `path`, when it is one of the reel's.
pub fn texture(path: &str) -> Option<Tex> {
    match path {
        GLOW => Some(shape(128, 128, |x, y| {
            let r = (x * x + y * y).sqrt();
            (1.0 - r).max(0.0).powf(2.2)
        })),
        // a wedge from its pivot at the bottom middle, widening and fading upwards
        RAY => Some(shape(64, 512, |x, y| {
            let up = (1.0 - y) / 2.0;
            let half = 0.06 + 0.94 * up;
            let edge = (1.0 - (x.abs() / half)).clamp(0.0, 1.0);
            edge.powf(1.5) * (1.0 - up).powf(1.3) * (up * 12.0).min(1.0)
        })),
        CASING => Some(casing()),
        _ => None,
    }
}

/// A spent brass casing seen from the side, 24 x 10: a brass tube lit from above with a darker rim at the base (left)
/// and an open mouth (right).
fn casing() -> Tex {
    let (w, h) = (24u32, 10u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for j in 0..h {
        for i in 0..w {
            let y = (j as f32 + 0.5) / h as f32 * 2.0 - 1.0;
            let inside = i >= 1 && i + 1 < w && y.abs() < 0.86;
            let rim = i <= 2;
            let mouth = i + 2 >= w;
            // light from above: bright near the top third, dark at the bottom
            let lit = (1.0 - (y + 0.35).abs()).clamp(0.0, 1.0);
            let base = if rim {
                0.62
            } else if mouth {
                0.7
            } else {
                0.8
            };
            let k = base * (0.45 + 0.75 * lit);
            let c = [
                (232.0 * k).min(255.0) as u8,
                (181.0 * k).min(255.0) as u8,
                (78.0 * k).min(255.0) as u8,
            ];
            rgba.extend_from_slice(&[c[0], c[1], c[2], if inside { 255 } else { 0 }]);
        }
    }
    Tex {
        width: w,
        height: h,
        rgba,
    }
}

/// A white texture of `w` x `h` whose alpha is `a(x, y)` with x and y from -1 to 1 across it.
fn shape(w: u32, h: u32, a: impl Fn(f32, f32) -> f32) -> Tex {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for j in 0..h {
        for i in 0..w {
            let x = (i as f32 + 0.5) / w as f32 * 2.0 - 1.0;
            let y = (j as f32 + 0.5) / h as f32 * 2.0 - 1.0;
            let v = (a(x, y).clamp(0.0, 1.0) * 255.0).round() as u8;
            rgba.extend_from_slice(&[255, 255, 255, v]);
        }
    }
    Tex {
        width: w,
        height: h,
        rgba,
    }
}

/// The same texture as an `Arc` (the texture cache's form).
pub fn texture_arc(path: &str) -> Option<Arc<Tex>> {
    texture(path).map(Arc::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_effect_textures_are_soft_white_shapes() {
        for name in [GLOW, RAY] {
            let t = texture(name).unwrap();
            assert_eq!(t.rgba.len(), (t.width * t.height * 4) as usize, "{name}");
            assert!(t.rgba.chunks(4).all(|p| p[..3] == [255, 255, 255]));
            let a: Vec<u8> = t.rgba.chunks(4).map(|p| p[3]).collect();
            assert!(
                a.iter().any(|&v| v > 150) && a.iter().any(|&v| v <= 2),
                "{name}"
            );
        }
        // the glow is brightest in the middle, the ring on its rim
        let g = texture(GLOW).unwrap();
        let at = |t: &Tex, x: u32, y: u32| t.rgba[((y * t.width + x) * 4 + 3) as usize];
        assert!(at(&g, 64, 64) > at(&g, 64, 100));
        // the casing is brass, opaque inside and clear around it
        let c = texture(CASING).unwrap();
        assert_eq!((c.width, c.height), (24, 10));
        let mid = &c.rgba[((5 * 24 + 12) * 4) as usize..][..4];
        assert!(
            mid[0] > mid[1] && mid[1] > mid[2] && mid[3] == 255,
            "{mid:?}"
        );
        assert_eq!(c.rgba[3], 0);
        assert!(texture("@dc:reel_none").is_none());
        assert!(Rarity::ALL
            .windows(2)
            .all(|w| strength(w[0]) < strength(w[1])));
    }
}
