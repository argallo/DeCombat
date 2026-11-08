//! The screen flash of `GS_PLAYER_STATUS` (0x66) kind 0x0c: the flashbang's white screen.
//!
//! The server names the flash's length in seconds and its direction (`fade`: from full white down, else the
//! inverse ramp up to white). Every frame the white level is read from the time through an eased curve and the drawn
//! weapon's two flash numbers (`WhiteScreenDecl`, `CapturedDecl` of its weapon row; the binary's defaults for a row
//! without them); the screen is covered with white at that level.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.2 row 0x66 kind 0x0c (`[kind][fade_out:u8][unused:u8][flash_secs:f32]`,
// cshell+0x49aa8..0x49b03)
// EVIDENCE: cshell+0x47bb0 (start: length = flash_secs x 1000.0 truncated, start time now, the fade flag), cshell+0x485a0
// (per frame, below), cshell+0x19f050 (the ease), cshell+0x17bd20 (weapon row loader: `WhiteScreenDecl` -> row +0x318,
// `CapturedDecl` -> row +0x31c), cshell+0x17f7b0 (row defaults +0x318 := 2.0, +0x31c := 1.0)

use super::items::DrawItem;
use dc_ui_data::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};

/// The flash numbers of a weapon row without its own (`WhiteScreenDecl`, `CapturedDecl`).
// EVIDENCE: cshell+0x17f7b0 (the weapon row constructor: row +0x318 := 0x40000000, row +0x31c := 0x3f800000)
pub const WHITE_DECL_DEFAULT: f32 = 2.0;
pub const CAPTURED_DECL_DEFAULT: f32 = 1.0;

/// The drawn weapon's flash numbers: how fast the white goes (`white`, the curve's multiplier) and the factor of the
/// captured-picture layer (`captured`; a fading flash ends once the white times it reaches 0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlashDecl {
    pub white: f32,
    pub captured: f32,
}

impl Default for FlashDecl {
    fn default() -> Self {
        FlashDecl {
            white: WHITE_DECL_DEFAULT,
            captured: CAPTURED_DECL_DEFAULT,
        }
    }
}

impl FlashDecl {
    /// The numbers of a weapon row (`None`: the row has no such key, the default applies).
    pub fn of_row(white: Option<f32>, captured: Option<f32>) -> FlashDecl {
        let d = FlashDecl::default();
        FlashDecl {
            white: white.filter(|v| v.is_finite()).unwrap_or(d.white),
            captured: captured.filter(|v| v.is_finite()).unwrap_or(d.captured),
        }
    }
}

/// The 2009 ease of the white curve: a half cosine below one half (0 to 0.5), the identity above.
// EVIDENCE: cshell+0x19f050 (p < 0.5: (1 - sin(p pi + pi / 2)) x 0.5, else p)
pub fn ease(p: f32) -> f32 {
    if p < 0.5 {
        (1.0 - (p * std::f32::consts::PI + std::f32::consts::FRAC_PI_2).sin()) * 0.5
    } else {
        p
    }
}

/// One running screen flash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Whiteout {
    pub start_ms: u32,
    pub length_ms: u32,
    /// From full white down (the server's non-zero `fade_out`); else the inverse ramp.
    pub fade: bool,
}

impl Whiteout {
    /// The flash of a 0x66 kind 0x0c at `now` (`None` for a length that is not a positive number).
    // EVIDENCE: cshell+0x47c10..0x47c25 (flash_secs x 1000.0 through the truncating float-to-int)
    pub fn start(now: u32, secs: f32, fade: bool) -> Option<Whiteout> {
        let ms = secs * 1000.0;
        if !(ms.is_finite() && ms >= 1.0) {
            return None;
        }
        Some(Whiteout {
            start_ms: now,
            length_ms: ms.min(u32::MAX as f32) as u32,
            fade,
        })
    }

    /// The white level (0..=1) at `now` with the drawn weapon's numbers; `None` once the flash is over.
    // EVIDENCE: cshell+0x4869c..0x486be (over once the time since the start passes the length), cshell+0x486c3..0x4872f
    // (f = the remaining time / length with the fade flag, else the elapsed time / length, clamped 0..1),
    // cshell+0x4872f..0x48799 (level = 1 - ease(1 - f) x row +0x318, clamped 0..1), cshell+0x48809..0x4883c (a fading
    // flash whose level x row +0x31c is 0 or less ends)
    pub fn level(&self, now: u32, decl: FlashDecl) -> Option<f32> {
        let elapsed = now.wrapping_sub(self.start_ms);
        if elapsed > self.length_ms {
            return None;
        }
        let x = if self.fade {
            self.length_ms - elapsed
        } else {
            elapsed
        };
        let f = (x as f32 / self.length_ms.max(1) as f32).clamp(0.0, 1.0);
        let w = (1.0 - ease(1.0 - f) * decl.white).clamp(0.0, 1.0);
        let shown = w * decl.captured;
        if self.fade && (shown.is_nan() || shown <= 0.0) {
            return None;
        }
        Some(w)
    }
}

/// The white's alpha byte at level `w` (truncated, as the 2009 colour word is formed).
// EVIDENCE: cshell+0x487db..0x48806 (level x 255.0 through the truncating float-to-int, alpha of the white colour
// word, drawn only when not zero)
pub fn alpha(w: f32) -> u8 {
    (w.clamp(0.0, 1.0) * 255.0) as u8
}

/// Pushes the white over the whole screen at level `w` (nothing at alpha 0).
// DC-DEFAULT: CLI-1926 (proposed) the white covers the world, the HUD and the kill banners; the network lines and
// the boxes stay over it; the captured-picture layer the 2009 frame also draws (alpha level x `CapturedDecl`, cshell+0x487a5..
// 0x487d8) is not ported (our HUD painter has no copy of the frame)
pub fn push_items(out: &mut Vec<DrawItem>, w: f32) {
    let a = alpha(w);
    if a == 0 {
        return;
    }
    out.push(DrawItem::Fill {
        rect: [0.0, 0.0, VIRTUAL_WIDTH as f32, VIRTUAL_HEIGHT as f32],
        colour: [255, 255, 255, a],
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ease_is_a_half_cosine_then_the_identity() {
        assert!(ease(0.0).abs() < 1e-6);
        assert!((ease(0.25) - (1.0 - std::f32::consts::FRAC_1_SQRT_2) * 0.5).abs() < 1e-5);
        assert!((ease(0.4999) - 0.5).abs() < 1e-3, "continuous at one half");
        assert_eq!(ease(0.5), 0.5);
        assert_eq!(ease(0.8), 0.8);
    }

    #[test]
    fn a_fading_flash_starts_full_white_and_eases_out_by_half_its_length_with_the_default_row() {
        // 4 s, fading, a weapon row without its own numbers (curve x 2): white at the start, eased down, clear at
        // 2 s (half the length), where the flash ends
        let f = Whiteout::start(1000, 4.0, true).expect("a flash");
        assert_eq!(f.length_ms, 4000);
        let d = FlashDecl::default();
        let at = |ms: u32| f.level(1000 + ms, d);
        assert_eq!(at(0), Some(1.0));
        let mut last = 1.0;
        for ms in (100..2000).step_by(100) {
            let w = at(ms).expect("still running");
            assert!(w < last && w > 0.0, "{ms} ms: {w} after {last}");
            last = w;
        }
        // 1 s in: 1 - 2 x ease(0.25)
        let w = at(1000).unwrap();
        assert!((w - (1.0 - 2.0 * ease(0.25))).abs() < 1e-5, "{w}");
        assert_eq!(alpha(at(0).unwrap()), 255);
        assert_eq!(at(2000), None, "clear at half the length: over");
        assert_eq!(at(3000), None);
        assert_eq!(at(4001), None);
    }

    #[test]
    fn the_thrown_weapons_own_numbers_clear_the_white_sooner() {
        // a row with its own numbers (curve x 4): clear where ease = 1 / 4, a third of the length
        let f = Whiteout::start(0, 3.0, true).unwrap();
        let own = FlashDecl::of_row(Some(4.0), Some(0.5));
        assert!(f.level(900, own).is_some_and(|w| w > 0.0));
        assert_eq!(f.level(1001, own), None);
        // the default row is still white then
        assert!(f.level(1001, FlashDecl::default()).is_some_and(|w| w > 0.2));
    }

    #[test]
    fn the_inverse_ramp_rises_to_white_and_ends_at_its_length() {
        let f = Whiteout::start(0, 2.0, false).unwrap();
        let d = FlashDecl::default();
        assert_eq!(f.level(0, d), Some(0.0), "clear at first");
        assert_eq!(f.level(500, d), Some(0.0));
        let late = f.level(1900, d).unwrap();
        assert!(late > 0.8, "{late}");
        assert_eq!(f.level(2000, d), Some(1.0));
        assert_eq!(f.level(2001, d), None);
    }

    #[test]
    fn no_flash_for_a_length_that_is_not_a_positive_number() {
        assert_eq!(Whiteout::start(0, 0.0, true), None);
        assert_eq!(Whiteout::start(0, -1.0, true), None);
        assert_eq!(Whiteout::start(0, f32::NAN, true), None);
    }

    #[test]
    fn the_white_is_one_full_screen_fill() {
        let mut out = Vec::new();
        push_items(&mut out, 0.0);
        assert!(out.is_empty());
        push_items(&mut out, 0.5);
        assert_eq!(
            out,
            vec![DrawItem::Fill {
                rect: [0.0, 0.0, VIRTUAL_WIDTH as f32, VIRTUAL_HEIGHT as f32],
                colour: [255, 255, 255, 127],
            }]
        );
    }
}
