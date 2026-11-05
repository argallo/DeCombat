//! The 2009 scroll bar control shared by the drop-down lists and the layout lists: a value in pixels from 0 to
//! (range - page), an up arrow at the top, a down arrow at the bottom, a back between them and a thumb whose length
//! and place follow page / range. A press on an arrow moves the value by one step; a press on the back starts a drag
//! that puts the thumb's middle under the pointer until the button is released.
// EVIDENCE: cshell+0x345490 (the control: up arrow +0x104, down arrow +0x108, back +0xfc, thumb +0x100, drag flag
// +0x110, step +0x114, page +0x128, range +0x130, value +0x138), cshell+0x345980 (built with width 0xe, height and
// page; range = page, value 0), cshell+0x345750 (content height -> range = max(content or 1, page)),
// cshell+0x345710 (value clamped to 0 .. range - page), cshell+0x3458d0 / cshell+0x345900 (value minus / plus a
// step, clamped), cshell+0x345ec0 (press: up arrow -> value - step, down arrow -> value + step, back -> drag on and
// the value taken from the pointer), cshell+0x3455c0 (the drag ends)
// DISASM: cshell+0x345980 (the back at (x, y + 9) of (w, h - 18), the thumb at (x + 3, y + 12) of w - 6, the up
// arrow at (x + 1, y), the down arrow at (x + 1, y + h - 7); scale +0x2c = 1.0 from cshell+0x2ec320)
// DISASM: cshell+0x345ce0 (track = h - 24; thumb length = min(page x track / range + 0.5, track); thumb offset =
// value x track / range + 0.5, clamped to 0 .. track - length; drawn back, thumb, up, down)
// DISASM: cshell+0x345690 (drag: value = (pointer y - y - 12 - g / 2) x range / track, g = track x page / range
// truncated, cshell+0x345750)

use super::catalog::Catalog;
use crate::hud::items::DrawItem;

/// The bar's width.
// EVIDENCE: cshell+0x33c960, cshell+0x2f00f0 (both owners build the bar 0xe wide)
pub const WIDTH: i32 = 0xe;
/// Track = bar height less this (the two arrow ends).
// DISASM: cshell+0x345ce0 (h - 0x18)
pub const ENDS: i32 = 0x18;
/// The thumb's top offset from the bar's top and its inset from the bar's sides.
// DISASM: cshell+0x345980 (thumb at (x + 3, y + 12), width w - 6)
pub const THUMB_TOP: i32 = 12;
pub const THUMB_INSET: i32 = 3;
/// The back's top offset (it is 2 x this shorter than the bar).
// DISASM: cshell+0x345980 (back at y + 7 + 2, height h - 2 x (7 + 2))
pub const BACK_TOP: i32 = 9;
/// The arrows: one pixel in from the bar's left, the down arrow this far above the bar's bottom.
// DISASM: cshell+0x345980 (up at (x + 1, y), down at (x + 1, y + h - 7))
pub const ARROW_H: i32 = 7;

/// A scroll bar.
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollBar {
    /// x, y, w, h (virtual pixels).
    pub rect: [i32; 4],
    pub page: i32,
    pub range: i32,
    pub value: i32,
    /// One press of an arrow (and one wheel step of a list) moves the value this far.
    pub step: i32,
    pub drag: bool,
    /// Textures: back, thumb, up, down.
    pub art: [Option<String>; 4],
    /// A layout list's bar that its screen draws after the list's rows (the sheet's own pass leaves it out).
    // EVIDENCE: cshell+0x2f0f00 (a list draws its shown rows, then its bar +0x228 last, over them)
    pub over_rows: bool,
}

impl ScrollBar {
    pub fn new(rect: [i32; 4], page: i32, art: [Option<String>; 4]) -> ScrollBar {
        ScrollBar {
            rect,
            page,
            range: page,
            value: 0,
            step: 0,
            drag: false,
            art,
            over_rows: false,
        }
    }

    /// The content height of the owner: the range becomes max(content or 1, page); the value stays in range.
    // EVIDENCE: cshell+0x345750
    pub fn set_content(&mut self, content: i32) {
        let c = if content == 0 { 1 } else { content };
        self.range = c.max(self.page);
        self.clamp();
    }

    /// The largest value.
    pub fn top(&self) -> i32 {
        (self.range - self.page).max(0)
    }

    // EVIDENCE: cshell+0x345710
    pub fn clamp(&mut self) -> i32 {
        self.value = self.value.clamp(0, self.top());
        self.value
    }

    pub fn track(&self) -> i32 {
        self.rect[3] - ENDS
    }

    /// Thumb length and offset in the track.
    pub fn thumb(&self) -> (i32, i32) {
        let track = self.track();
        let r = self.range.max(1) as f32;
        let len = ((self.page as f32 * track as f32 / r + 0.5) as i32).min(track);
        let value = self.value.clamp(0, self.top());
        let off = ((value as f32 * track as f32 / r + 0.5) as i32)
            .max(0)
            .min(track - len);
        (len, off)
    }

    // DC-DEFAULT: CLI-701 (proposed) an arrow takes presses over (bar width - 2) x 7 at its place (the 2009 arrow is
    // a button whose own size, its texture's, is not traced)
    fn high_arrow_rect(&self) -> [i32; 4] {
        let [x, y, w, _] = self.rect;
        [x + 1, y, w - 2, ARROW_H]
    }

    fn low_arrow_rect(&self) -> [i32; 4] {
        let [x, y, w, h] = self.rect;
        [x + 1, y + h - ARROW_H, w - 2, ARROW_H]
    }

    fn back_rect(&self) -> [i32; 4] {
        let [x, y, w, h] = self.rect;
        [x, y + BACK_TOP, w, h - 2 * BACK_TOP]
    }

    /// The thumb's rectangle.
    pub fn thumb_rect(&self) -> [i32; 4] {
        let [x, y, w, _] = self.rect;
        let (len, off) = self.thumb();
        [
            x + THUMB_INSET,
            y + THUMB_TOP + off,
            w - 2 * THUMB_INSET,
            len,
        ]
    }

    pub fn contains(&self, p: [f32; 2]) -> bool {
        inside(self.rect, p)
    }

    /// The centre of the up (`-1`) or down (`1`) arrow (scripted players press there).
    pub fn arrow_point(&self, dir: i32) -> [f32; 2] {
        let r = if dir < 0 {
            self.high_arrow_rect()
        } else {
            self.low_arrow_rect()
        };
        [
            r[0] as f32 + r[2] as f32 / 2.0,
            r[1] as f32 + r[3] as f32 / 2.0,
        ]
    }

    /// -1 / +1 when `p` is on the up / down arrow.
    pub fn arrow_at(&self, p: [f32; 2]) -> Option<i32> {
        if inside(self.high_arrow_rect(), p) {
            Some(-1)
        } else if inside(self.low_arrow_rect(), p) {
            Some(1)
        } else {
            None
        }
    }

    /// The value that puts the thumb's middle at pointer height `py`.
    // DISASM: cshell+0x345690
    fn value_at(&self, py: f32) -> i32 {
        let track = self.track();
        if track <= 0 {
            return 0;
        }
        let g = (track as f32 * self.page as f32 / self.range.max(1) as f32) as i32;
        let v = (py - self.rect[1] as f32 - (ENDS / 2) as f32 - g as f32 * 0.5) * self.range as f32
            / track as f32;
        v as i32
    }

    /// A press at `p`: true when the bar took it (arrow step, or the back: drag on, value from the pointer).
    // EVIDENCE: cshell+0x345ec0
    pub fn press(&mut self, p: [f32; 2]) -> bool {
        if !self.contains(p) {
            return false;
        }
        match self.arrow_at(p) {
            Some(-1) => self.value = (self.value - self.step).max(0),
            Some(_) => self.value = (self.value + self.step).min(self.top()),
            None if inside(self.back_rect(), p) => {
                self.drag = true;
                self.value = self.value_at(p[1]);
                self.clamp();
            }
            None => {}
        }
        true
    }

    /// The pointer moved: while dragging, the value follows it. True when the value may have changed.
    // DISASM: cshell+0x345690 (only while the drag flag is set)
    pub fn drag_to(&mut self, p: [f32; 2]) -> bool {
        if !self.drag {
            return false;
        }
        self.value = self.value_at(p[1]);
        self.clamp();
        true
    }

    // EVIDENCE: cshell+0x3455c0 (drag flag cleared)
    pub fn release(&mut self) {
        self.drag = false;
    }

    /// The pointer y that a drag must reach to put the value at `value` (scripted players and tests).
    pub fn drag_point(&self, value: i32) -> [f32; 2] {
        let track = self.track();
        let g = (track as f32 * self.page as f32 / self.range.max(1) as f32) as i32;
        let y = self.rect[1] as f32
            + (ENDS / 2) as f32
            + g as f32 * 0.5
            + (value as f32 + 0.5) * track as f32 / self.range.max(1) as f32;
        [self.rect[0] as f32 + self.rect[2] as f32 / 2.0, y]
    }

    /// Draws back, thumb, up arrow, down arrow (each when its texture is known).
    pub fn draw(&self, cat: &mut Catalog, out: &mut Vec<DrawItem>) {
        let [x, y, w, h] = self.rect;
        let thumb = self.thumb_rect();
        let push = |cat: &mut Catalog,
                    out: &mut Vec<DrawItem>,
                    i: usize,
                    at: [i32; 2],
                    size: Option<[i32; 2]>| {
            let Some(t) = self.art[i].as_deref() else {
                return;
            };
            let Some((tw, th)) = cat.tex_size(t) else {
                return;
            };
            let size = size.unwrap_or([tw as i32, th as i32]);
            out.push(DrawItem::Image {
                tex: t.to_string(),
                src: [tw as f32, th as f32],
                dst: [at[0] as f32, at[1] as f32, size[0] as f32, size[1] as f32],
                tint: [255; 4],
                turn: None,
            });
        };
        // the back and the thumb are frame controls: their textures in nine parts
        // EVIDENCE: cshell+0x345980 (back +0xfc and thumb +0x100 built by cshell+0x343860 with mode 0)
        if let Some(t) = self.art[0].as_deref() {
            super::nine::draw(
                cat,
                t,
                [x, y + BACK_TOP, w, h - 2 * BACK_TOP],
                [255; 4],
                out,
            );
        }
        if let Some(t) = self.art[1].as_deref() {
            super::nine::draw(cat, t, thumb, [255; 4], out);
        }
        push(cat, out, 2, [x + 1, y], None);
        push(cat, out, 3, [x + 1, y + h - ARROW_H], None);
    }
}

fn inside(r: [i32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] as f32
        && p[1] >= r[1] as f32
        && p[0] < (r[0] + r[2]) as f32
        && p[1] < (r[1] + r[3]) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> ScrollBar {
        // a list of 20 rows of 18 px (360) in a 94 px page
        let mut b = ScrollBar::new([500, 100, WIDTH, 94], 94, Default::default());
        b.step = 18;
        b.set_content(360);
        b
    }

    #[test]
    fn thumb_follows_page_and_value() {
        let mut b = bar();
        // track 70; length 94 x 70 / 360 + 0.5 = 18
        assert_eq!(b.thumb(), (18, 0));
        b.value = b.top();
        assert_eq!(b.value, 266);
        // 266 x 70 / 360 + 0.5 = 52 = track - length
        assert_eq!(b.thumb(), (18, 52));
        // content shorter than the page: a full-length thumb
        b.set_content(40);
        assert_eq!(b.value, 0);
        assert_eq!(b.thumb(), (70, 0));
    }

    #[test]
    fn arrows_step_and_the_back_drags() {
        let mut b = bar();
        assert!(b.press(b.arrow_point(1)));
        assert_eq!(b.value, 18);
        assert!(b.press(b.arrow_point(-1)));
        assert!(b.press(b.arrow_point(-1)));
        assert_eq!(b.value, 0);
        // a press low on the back jumps there and drags
        assert!(b.press([505.0, 180.0]));
        assert!(b.drag && b.value > 200, "{}", b.value);
        assert!(b.drag_to([505.0, 400.0]));
        assert_eq!(b.value, b.top());
        assert!(b.drag_to(b.drag_point(100)));
        assert_eq!(b.value, 100);
        b.release();
        assert!(!b.drag_to([505.0, 100.0]));
        assert!(!b.press([400.0, 150.0]));
    }
}
