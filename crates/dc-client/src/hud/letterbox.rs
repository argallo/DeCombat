//! The letterbox: two black bars, top and bottom, that grow in while a world camera shot of the letterbox kind is
//! live (the round-end camera shot, CLI-240) and shrink away after it.
//!
//! The 2009 rule (DISASM, CShell):
//! - The world camera's create blob carries a kind byte (the second blob byte, camera +0x3c, cshell+0x216887); every
//!   frame a live camera hands it to cshell+0x1f50b0 (cshell+0x1f8aa2..0x1f8b22), which sets the interface's letterbox
//!   flag (+0x532c) when it is exactly 1.
//! - The interface draw (cshell+0x1b6120, called first in the in-game interface frame cshell+0x1b73be, before the HUD)
//!   keeps a fade clock: on the flag's rising edge the bars run in over `LetterBoxFadeInTime`, on its falling edge out
//!   over `LetterBoxFadeOutTime` (console values, default 1.0 s each, cshell+0x1b7ff0); the progress is
//!   `1 - left / fade_in` going in and `left / fade_out` going out, measured on the engine clock (frame-rate
//!   independent). At progress 0 nothing is drawn.
//! - Each bar is `trunc(screen_height * progress * (1 - LetterBoxPercent) * 0.5)` pixels high (cshell+0x1b62fc..
//!   0x1b63b3; the factor is 0 unless 0.01 < LetterBoxPercent < 1) over the full width, from the top edge and up to
//!   the bottom edge, with the surface alpha set to the progress (engine call +0x198 before the two draws).
//! - `LetterBoxPercent` is read from the client attribute file's camera section (cshell+0x1b2aa0).
// DISASM: cshell+0x1b6120, cshell+0x1b62fc..0x1b645f, cshell+0x1b7ff0 (console defaults), cshell+0x1f50b0,
// cshell+0x1f8aa2..0x1f8b22, cshell+0x216887
// OBSERVED: ROM table key CLIENTBUTES.TXT [Camera] LetterBoxPercent (0.66 in the ROM)
// DC-DEFAULT: CLI-610 (proposed) the bars go out when no letterbox camera is live (the 2009 clear path through the
// camera exit, cshell+0x1f847b, was not traced)

use super::items::DrawItem;

/// The ROM key of the bar size (the share of the screen left between the bars).
// OBSERVED: ROM table key CLIENTBUTES.TXT [Camera] LetterBoxPercent
pub const PERCENT_SECTION: &str = "Camera";
pub const PERCENT_KEY: &str = "LetterBoxPercent";
/// Without the ROM value: the ROM's own 0.66.
// DC-DEFAULT: CLI-610 (proposed) fallback = the ROM value
pub const DEFAULT_PERCENT: f32 = 0.66;
/// Fade times (console defaults 1.0 s).
// DISASM: cshell+0x1b7ff0 (`LetterBoxFadeInTime` / `LetterBoxFadeOutTime` registered with 1.0)
pub const FADE_IN_MS: u32 = 1000;
pub const FADE_OUT_MS: u32 = 1000;
/// The blob kind byte of a letterbox camera.
// DISASM: cshell+0x1f50b8 (`param == 1`)
pub const LETTERBOX_KIND: u8 = 1;

/// The fade clock of the bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LetterboxClock {
    pub on: bool,
    /// Clock time the running fade ends (`None`: no edge seen yet).
    pub end_ms: Option<u32>,
}

impl LetterboxClock {
    /// Feeds the frame's flag at `now` (the session clock, ms).
    pub fn set(&mut self, on: bool, now: u32) {
        if on != self.on {
            self.on = on;
            self.end_ms = Some(now.wrapping_add(if on { FADE_IN_MS } else { FADE_OUT_MS }));
        }
    }

    /// The bar progress 0..=1 at `now`.
    pub fn progress(&self, now: u32) -> f32 {
        let Some(end) = self.end_ms else {
            return 0.0;
        };
        // wrapping clock: a fade end in the past reads as 0 ms left
        let left = end.wrapping_sub(now);
        let left = if left > u32::MAX / 2 { 0 } else { left } as f32;
        if self.on {
            if FADE_IN_MS == 0 {
                1.0
            } else {
                (1.0 - left / FADE_IN_MS as f32).clamp(0.0, 1.0)
            }
        } else if FADE_OUT_MS == 0 {
            0.0
        } else {
            (left / FADE_OUT_MS as f32).clamp(0.0, 1.0)
        }
    }
}

/// The height of one bar in target pixels.
pub fn bar_px(screen_h: u32, progress: f32, percent: f32) -> u32 {
    let share = if 0.01 < percent && percent < 1.0 {
        (1.0 - percent) * 0.5
    } else {
        0.0
    };
    (screen_h as f32 * progress * share).max(0.0) as u32
}

/// The two bars as HUD items (virtual coordinates; `sy` = target pixels per virtual pixel vertically), none at
/// progress 0.
pub fn push_bars(items: &mut Vec<DrawItem>, screen_h: u32, sy: f32, progress: f32, percent: f32) {
    if progress <= 0.0 {
        return;
    }
    let px = bar_px(screen_h, progress, percent);
    if px == 0 || sy <= 0.0 {
        return;
    }
    let (h, full_h) = (px as f32 / sy, screen_h as f32 / sy);
    let alpha = (progress * 255.0).round().clamp(0.0, 255.0) as u8;
    let colour = [0, 0, 0, alpha];
    let w = dc_ui_data::VIRTUAL_WIDTH as f32;
    items.push(DrawItem::Fill {
        rect: [0.0, 0.0, w, h],
        colour,
    });
    items.push(DrawItem::Fill {
        rect: [0.0, full_h - h, w, h],
        colour,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bars_grow_in_over_the_fade_time_at_any_frame_rate() {
        // the progress is a function of the clock alone: 30 fps and 1000 fps sample the same curve
        for step in [33u32, 1] {
            let mut c = LetterboxClock::default();
            let mut seen = Vec::new();
            let mut t = 5_000;
            while t <= 7_000 {
                c.set(t >= 5_000, t);
                seen.push((t, bar_px(768, c.progress(t), 0.66)));
                t += step;
            }
            let at = |ms: u32| seen.iter().rev().find(|(t, _)| *t <= ms).unwrap().1;
            assert_eq!(at(5_000), 0, "starts at nothing");
            // 768 * 0.5 * 0.17 = 65.28 -> 65
            assert_eq!(bar_px(768, 0.5, 0.66), 65);
            assert!(
                (at(5_500) as i32 - 65).abs() <= 1,
                "half way at half the fade: {}",
                at(5_500)
            );
            assert_eq!(at(6_050), 130, "full bar (768 * 0.17) after 1 s");
            assert_eq!(at(7_000), 130);
            assert!(
                seen.windows(2).all(|w| w[0].1 <= w[1].1),
                "never shrinks while on"
            );
        }
    }

    #[test]
    fn the_bars_shrink_out_after_the_camera_and_vanish() {
        let mut c = LetterboxClock::default();
        c.set(true, 0);
        c.set(true, 2_000);
        c.set(false, 2_000);
        assert_eq!(c.progress(2_000), 1.0);
        assert!((c.progress(2_250) - 0.75).abs() < 1e-6);
        assert_eq!(c.progress(3_000), 0.0);
        let mut items = Vec::new();
        push_bars(&mut items, 768, 1.0, c.progress(3_500), 0.66);
        assert!(items.is_empty(), "nothing drawn at 0");
        push_bars(&mut items, 768, 1.0, 1.0, 0.66);
        match (&items[0], &items[1]) {
            (DrawItem::Fill { rect: a, colour }, DrawItem::Fill { rect: b, .. }) => {
                assert_eq!(*a, [0.0, 0.0, 1024.0, 130.0]);
                assert_eq!(*b, [0.0, 638.0, 1024.0, 130.0]);
                assert_eq!(*colour, [0, 0, 0, 255]);
            }
            other => panic!("{other:?}"),
        }
        // a percent outside (0.01, 1) draws no bars
        assert_eq!(bar_px(768, 1.0, 1.0), 0);
        assert_eq!(bar_px(768, 1.0, 0.0), 0);
    }
}
