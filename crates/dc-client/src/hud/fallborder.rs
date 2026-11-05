//! The fall's screen-edge flash: a soft red band along all four edges of the screen, strongest at the edge and
//! fading inward, drawn from the HUD's own filled rectangles (no new pipeline, no texture).
//!
//! A fall has no bearing, so the 2009 directional arcs (a small ring about the crosshair) cannot show it; the
//! user asked for the screen edges instead.

use super::items::DrawItem;
use dc_ui_data::{VirtualScaler, VIRTUAL_HEIGHT, VIRTUAL_WIDTH};

/// How deep the band reaches in from each edge, as a fraction of the screen height (the same pixels on all four
/// sides, so the left and right bands are as deep as the top and bottom).
// DC-DEFAULT: CLI-1847 (proposed; USER DECISION 2026-11-04 "the damage circle when you take fall damage is too small,
// it's in the middle of the screen rather than the border") 10 % of the screen height
pub const DEPTH: f32 = 0.10;

/// The band is this many stacked frames of one alpha each (the fall-off is a quadratic over them); 16 is smooth
/// enough at 1080p (a step is 7 pixels, the alpha difference between neighbours under 6 of 255).
pub const STEPS: usize = 16;

/// The band colour (the fallback damage arc's red) and its alpha at the screen edge at full strength.
// DC-DEFAULT: CLI-1847 (proposed) red 220 / 30 / 20, edge alpha 200
pub const COLOUR: [u8; 3] = [220, 30, 20];
pub const EDGE_ALPHA: f32 = 200.0;

/// The alpha of the frame `step` (0 = outermost) at `strength` (0..=1).
pub fn step_alpha(step: usize, strength: f32) -> u8 {
    let t = (step as f32 + 0.5) / STEPS as f32;
    (EDGE_ALPHA * strength.clamp(0.0, 1.0) * (1.0 - t) * (1.0 - t)).round() as u8
}

/// Pushes the band at `strength` for a `width` x `height` target (nothing at 0).
pub fn push_items(out: &mut Vec<DrawItem>, strength: f32, width: u32, height: u32) {
    if !(strength > 0.0) || width == 0 || height == 0 {
        return;
    }
    let (w, h) = (VIRTUAL_WIDTH as f32, VIRTUAL_HEIGHT as f32);
    let sc = VirtualScaler::new(width, height);
    let dy = DEPTH * h;
    let dx = dy * sc.sy / sc.sx;
    let inset = |k: usize| (dx * k as f32 / STEPS as f32, dy * k as f32 / STEPS as f32);
    for k in 0..STEPS {
        let a = step_alpha(k, strength);
        if a == 0 {
            continue;
        }
        let colour = [COLOUR[0], COLOUR[1], COLOUR[2], a];
        let ((ax, ay), (bx, by)) = (inset(k), inset(k + 1));
        // the frame between inset a and inset b: top and bottom full width, left and right between them
        for rect in [
            [ax, ay, w - 2.0 * ax, by - ay],
            [ax, h - by, w - 2.0 * ax, by - ay],
            [ax, by, bx - ax, h - 2.0 * by],
            [w - bx, by, bx - ax, h - 2.0 * by],
        ] {
            out.push(DrawItem::Fill { rect, colour });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fills(strength: f32, w: u32, h: u32) -> Vec<([f32; 4], [u8; 4])> {
        let mut out = Vec::new();
        push_items(&mut out, strength, w, h);
        out.into_iter()
            .filter_map(|i| match i {
                DrawItem::Fill { rect, colour } => Some((rect, colour)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn nothing_at_zero_strength() {
        assert!(fills(0.0, 1920, 1080).is_empty());
        assert!(fills(-1.0, 1920, 1080).is_empty());
        assert!(fills(f32::NAN, 1920, 1080).is_empty());
    }

    #[test]
    fn the_band_is_strongest_at_the_edge_and_fades_inward_with_the_strength() {
        let full = fills(1.0, 1024, 768);
        // a step whose alpha rounds to 0 is not pushed
        assert!(
            full.len() % 4 == 0 && full.len() >= (STEPS - 2) * 4,
            "{}",
            full.len()
        );
        assert_eq!(full[0].1[3], step_alpha(0, 1.0));
        assert!(step_alpha(0, 1.0) as f32 > EDGE_ALPHA * 0.85);
        let alphas: Vec<u8> = (0..STEPS).map(|k| step_alpha(k, 1.0)).collect();
        assert!(alphas.windows(2).all(|p| p[0] >= p[1]), "{alphas:?}");
        let half = fills(0.5, 1024, 768);
        assert!(half[0].1[3] < full[0].1[3] && half[0].1[3] > 0);
    }

    #[test]
    fn the_band_is_a_tenth_of_the_height_deep_on_every_edge_in_screen_pixels() {
        // 16:9: the virtual-to-pixel scales differ, the depths in pixels do not
        let (w, h) = (1920u32, 1080u32);
        let sc = VirtualScaler::new(w, h);
        let items = fills(1.0, w, h);
        let (mut top, mut left) = (0.0f32, 0.0f32);
        for (r, _) in &items {
            // top frames start at y < 384 and are wider than tall; side frames are taller than wide
            if r[2] > r[3] && r[1] < VIRTUAL_HEIGHT as f32 / 2.0 {
                top = top.max(r[1] + r[3]);
            }
            if r[3] > r[2] && r[0] < VIRTUAL_WIDTH as f32 / 2.0 {
                left = left.max(r[0] + r[2]);
            }
        }
        // the last visible step ends within two steps of the full depth, and never beyond it
        let want = 0.10 * h as f32;
        for d in [top * sc.sy, left * sc.sx] {
            assert!(d <= want + 1.0 && d >= want * 0.85, "{d}");
        }
        assert!(
            (top * sc.sy - left * sc.sx).abs() < 1.0,
            "same depth on all sides"
        );
    }
}
