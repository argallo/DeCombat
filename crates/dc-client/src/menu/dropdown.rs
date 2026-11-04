//! The 2009 drop-down list of the room settings and the room creation box: a framed column of texts that opens 3
//! pixels under the control it belongs to, as wide as that control; a click on a row picks it and closes the list,
//! the command that opened it closes it again.
// EVIDENCE: cshell+0x33ca50 (the list control: 5 rows shown at most), cshell+0x33cb70 / cshell+0x2f8d00 (frame
// texture of 32 x 32, centred rows), cshell+0x33c750 (fixed width from the caller), cshell+0x33d010 (one text row per
// item, colour 0xffeeeeee), cshell+0x33c350 (row k at frame y + 4 + k x (row height + spacing), centred in the
// width), cshell+0x33c620 (frame height 4 + rows x (row height + spacing)), cshell+0x33cbb0 (a click on a shown row
// hides the list and hands the row's index in the whole list to the owner's command with the list's command id),
// cshell+0x33c520 (scrolling shows rows first..first + 5)
// EVIDENCE: cshell+0x33c960 (the scroll bar: cshell+0x345490 control, width 0xe, height = page = the frame's,
// placed at list width - 0x10), cshell+0x33ce00 (shown: range = the whole list's height cshell+0x33c690, step = row
// height + spacing), cshell+0x33cbb0 (a press on the bar goes to the bar, then first row = value / whole height x
// rows), cshell+0x33cd70 (pointer moves go to the bar, then the same), cshell+0x15f470 (the waiting room hands
// every wheel turn to its open list, wherever the pointer is)
// DISASM: cshell+0x33cd70 (first row = value / whole height x rows + 0.5, truncated), cshell+0x33d5c0 (wheel: a
// positive turn -> cshell+0x33d200, a negative one -> cshell+0x33d3d0, one row each whatever the turn's size)
// EVIDENCE: cshell+0x33d200 (one row up: at the top the bar's value goes to 0, else first - 1 and value - step),
// cshell+0x33d3d0 (one row down: at the bottom the value goes to its end, else first + 1 and value + step)
// EVIDENCE: cshell+0x15ca90 (the waiting room's list: frame and scroll textures, spacing 4, command 0xf0),
// cshell+0x138040 (the room creation box's list, the same), cshell+0x1625f0 / cshell+0x1628e0 / cshell+0x163550 /
// cshell+0x1637a0 / cshell+0x160080 (map / mode / goal / players / weapon lists under controls (6, 1) .. (6, 5):
// position = control's top-left + (0, height + 3), width = control's, font index 0xe size 0xe; only the map list
// scrolls), cshell+0x139590 (the creation box's mode list under control (1, 3), size 0xc)

use dc_ui_data::names::{read_pinned, Pin};

use super::catalog::Catalog;
use super::drawlist::DynWant;
use super::scrollbar::{self, ScrollBar};
use super::sheet::place_text;
use crate::hud::items::DrawItem;
use dc_ui_data::pages::Align;

/// Rows shown at most (the map list scrolls past them).
// EVIDENCE: cshell+0x33ca50 (+0x804 = 5)
pub const MAX_ROWS: usize = 5;
/// Gap between the rows (the owners set 4).
// EVIDENCE: cshell+0x15ca90, cshell+0x138040 (+0x7fc = 4)
pub const SPACING: i32 = 4;
/// The frame's top padding before the first row (and the height's start value).
// EVIDENCE: cshell+0x33c350 (first row at y + 4), cshell+0x33c620 (height starts at 4)
pub const TOP_PAD: i32 = 4;
/// The list opens this far under its control.
// EVIDENCE: cshell+0x163550 (list y = control y + height + 3)
pub const GAP: i32 = 3;
/// Row text colour (both colours of the row's text control).
// EVIDENCE: cshell+0x33d010 (vtable +0xb0 with 0xffeeeeee, 0xffeeeeee)
pub const ROW_COLOUR: u32 = 0xffee_eeee;
/// Font index of the rows.
// EVIDENCE: cshell+0x163550 / cshell+0x139590 (the rows' font by index 0xe)
pub const ROW_FONT: i32 = 0xe;
/// Row text size in the waiting room and in the creation box.
// EVIDENCE: cshell+0x160080 (row size 0xe), cshell+0x139590 (row size 0xc)
pub const ROOM_ROW_SIZE: i32 = 0xe;
pub const CREATE_ROW_SIZE: i32 = 0xc;
/// The scroll bar's width and its place from the list's right edge.
// EVIDENCE: cshell+0x33c960 (bar width 0xe; placed at list width - 0x10)
pub const SCROLL_WIDTH: i32 = scrollbar::WIDTH;
pub const SCROLL_FROM_RIGHT: i32 = 0x10;
/// The scroll bar sits this far under the frame's top (as high as the frame: it reaches past the frame's bottom).
// DISASM: cshell+0x33ca1a (the bar added as a child at (width - 0x10, 2) through vtable +0x138 = cshell+0x2f9360,
// which moves it to the list's x / y plus those)
pub const SCROLL_TOP: i32 = 2;

/// The list's frame texture and its scroll bar textures (back, bar, up, down).
// EVIDENCE: cshell+0x15ca90 (frame), cshell+0x1625f0 (scroll bar textures of the map list)
pub const FRAME_TEX: Pin = Pin {
    what: "drop-down list frame",
    rva: 0x3cdadc,
    sha16: "3fb07d70b5de62a4",
    len: 34,
};
/// The hover bar's texture: a frame control the list moves under the row the pointer is on.
// EVIDENCE: cshell+0x15ca90 / cshell+0x138040 (the owners hand it to cshell+0x33c0e0 with 8 x 8, mode 0),
// cshell+0x33c0e0 (a frame control, hidden until the pointer is on a row)
pub const HIGHLIGHT_TEX: Pin = Pin {
    what: "drop-down hover bar",
    rva: 0x3cdab8,
    sha16: "3622aa8b5dfe78b7",
    len: 32,
};
pub const SCROLL_TEX: [Pin; 4] = [
    Pin {
        what: "drop-down scroll back",
        rva: 0x3dcf38,
        sha16: "6c42b566e5f649d3",
        len: 36,
    },
    Pin {
        what: "drop-down scroll bar",
        rva: 0x3dcf60,
        sha16: "3c4fb214f9abf9f1",
        len: 35,
    },
    Pin {
        what: "drop-down scroll up",
        rva: 0x3dcf84,
        sha16: "bb17821c8689671c",
        len: 34,
    },
    Pin {
        what: "drop-down scroll down",
        rva: 0x3dd014,
        sha16: "6baa4244f42cface",
        len: 36,
    },
];

/// The list's textures as the user's CShell.dll names them (none without a ROM).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DropArt {
    pub frame: Option<String>,
    pub highlight: Option<String>,
    pub scroll: [Option<String>; 4],
}

impl DropArt {
    pub fn from_pe(pe: &dc_rom::PeFile) -> DropArt {
        DropArt {
            frame: read_pinned(pe, &FRAME_TEX).ok(),
            highlight: read_pinned(pe, &HIGHLIGHT_TEX).ok(),
            scroll: SCROLL_TEX.map(|p| read_pinned(pe, &p).ok()),
        }
    }
}

/// An open drop-down list.
#[derive(Debug, Clone, PartialEq)]
pub struct Dropdown {
    /// What the list holds (the draw list's tag: `map`, `mode`, `goal`, `players`, `arms`, `create_mode`).
    pub list: &'static str,
    /// The control it opened under (x, y, w, h).
    pub anchor: [i32; 4],
    pub items: Vec<String>,
    /// The value behind each row (map index, mode row, goal, players, weapon limit).
    pub values: Vec<i32>,
    pub font_size: i32,
    /// The list scrolls when it has more rows than [`MAX_ROWS`] (the map list).
    pub scroll: bool,
    /// The first row shown.
    pub first: usize,
    /// The row under the pointer (index in the whole list).
    pub hover: Option<usize>,
    /// The scroll bar of a list that scrolls (more rows than [`MAX_ROWS`]).
    pub bar: Option<ScrollBar>,
    /// The rows start at the frame's left edge plus [`LEFT_PAD`] instead of being centred.
    pub left: bool,
}

/// The left padding of a left-aligned row.
// OBSERVED: the reference stack's report box lists (rows start 5 px right of the frame)
pub const LEFT_PAD: i32 = 5;

/// Row height: the font size of the rows.
// DC-DEFAULT: CLI-501 (proposed) a row is as high as its font size (the 2009 text control's height, vtable +0x1c,
// is not traced)
pub fn row_height(font_size: i32) -> i32 {
    font_size
}

/// Where the 2009 rule puts the frame of a list under `anchor` showing `shown` rows (x, y, w, h).
pub fn rule_frame(anchor: [i32; 4], shown: usize, font_size: i32) -> [f32; 4] {
    let h = TOP_PAD + shown as i32 * (row_height(font_size) + SPACING);
    [
        anchor[0] as f32,
        (anchor[1] + anchor[3] + GAP) as f32,
        anchor[2] as f32,
        h as f32,
    ]
}

/// What the 2009 rule asks of shown row `k` of a list under `anchor` (centred in the width, top at the row's y).
pub fn rule_row(list: &str, anchor: [i32; 4], k: usize, font_size: i32) -> DynWant {
    let top = anchor[1] + anchor[3] + GAP + TOP_PAD + k as i32 * (row_height(font_size) + SPACING);
    DynWant {
        key: format!("dyn/drop/{list}/row{k}"),
        text_at: [anchor[0] as f32 + anchor[2] as f32 / 2.0, top as f32],
        text_anchor: [0.5, 0.0],
        font: Some(ROW_FONT),
        font_size: Some(font_size),
    }
}

impl Dropdown {
    pub fn new(
        list: &'static str,
        anchor: [i32; 4],
        rows: Vec<(String, i32)>,
        font_size: i32,
        scroll: bool,
    ) -> Dropdown {
        let (items, values): (Vec<String>, Vec<i32>) = rows.into_iter().unzip();
        let mut d = Dropdown {
            list,
            anchor,
            items,
            values,
            font_size,
            scroll,
            first: 0,
            hover: None,
            bar: None,
            left: false,
        };
        if d.scrolls() {
            let r = d.rect();
            let mut bar = ScrollBar::new(
                [
                    r[0] + r[2] - SCROLL_FROM_RIGHT,
                    r[1] + SCROLL_TOP,
                    SCROLL_WIDTH,
                    r[3],
                ],
                r[3],
                Default::default(),
            );
            bar.set_content(d.whole_height());
            bar.step = row_height(font_size) + SPACING;
            d.bar = Some(bar);
        }
        d
    }

    /// The height of every row of the list (the bar's range).
    // EVIDENCE: cshell+0x33c690 (4 + rows x (row height + spacing))
    fn whole_height(&self) -> i32 {
        TOP_PAD + self.items.len() as i32 * (row_height(self.font_size) + SPACING)
    }

    /// The first row from the bar's value.
    // DISASM: cshell+0x33cd70
    fn first_from_bar(&mut self) {
        let n = self.items.len();
        let whole = self.whole_height().max(1) as f32;
        if let Some(b) = self.bar.as_mut() {
            let v = b.clamp() as f32;
            let first = (v / whole * n as f32 + 0.5) as usize;
            self.first = first.min(n.saturating_sub(MAX_ROWS));
        }
    }

    /// A wheel turn (`notches` > 0: away from the user): one row up or down.
    // EVIDENCE: cshell+0x33d200 / cshell+0x33d3d0, DISASM: cshell+0x33d5c0
    pub fn wheel(&mut self, notches: i32) {
        let n = self.items.len();
        let shown = self.shown();
        let Some(b) = self.bar.as_mut() else {
            return;
        };
        if notches > 0 {
            if shown.start == 0 {
                b.value = 0;
            } else {
                self.first = shown.start - 1;
                b.value = (b.value - b.step).max(0);
            }
        } else if notches < 0 {
            if shown.end >= n {
                b.value = b.top();
            } else {
                self.first = shown.start + 1;
                b.value = (b.value + b.step).min(b.top());
            }
        }
    }

    /// A press on the scroll bar: arrow step or drag start. True when the bar took it.
    // EVIDENCE: cshell+0x33cbb0
    pub fn press_bar(&mut self, p: [f32; 2]) -> bool {
        let took = self.bar.as_mut().is_some_and(|b| b.press(p));
        if took {
            self.first_from_bar();
        }
        took
    }

    /// The pointer moved: a drag of the bar's thumb moves the rows.
    // EVIDENCE: cshell+0x33cd70
    pub fn drag_to(&mut self, p: [f32; 2]) -> bool {
        let moved = self.bar.as_mut().is_some_and(|b| b.drag_to(p));
        if moved {
            self.first_from_bar();
        }
        moved
    }

    /// The bar is being dragged.
    pub fn dragging(&self) -> bool {
        self.bar.as_ref().is_some_and(|b| b.drag)
    }

    pub fn release(&mut self) {
        if let Some(b) = self.bar.as_mut() {
            b.release();
        }
    }

    /// Rows shown now (index in the whole list).
    pub fn shown(&self) -> std::ops::Range<usize> {
        let n = self.items.len();
        if self.scroll && n > MAX_ROWS {
            let first = self.first.min(n - MAX_ROWS);
            first..first + MAX_ROWS
        } else {
            0..n
        }
    }

    fn scrolls(&self) -> bool {
        self.scroll && self.items.len() > MAX_ROWS
    }

    /// The frame (x, y, w, h).
    pub fn rect(&self) -> [i32; 4] {
        let rows = self.shown().len() as i32;
        let row = row_height(self.font_size) + SPACING;
        [
            self.anchor[0],
            self.anchor[1] + self.anchor[3] + GAP,
            self.anchor[2],
            TOP_PAD + rows * row,
        ]
    }

    pub fn contains(&self, p: [f32; 2]) -> bool {
        let r = self.rect();
        p[0] >= r[0] as f32
            && p[1] >= r[1] as f32
            && p[0] < (r[0] + r[2]) as f32
            && p[1] < (r[1] + r[3]) as f32
    }

    /// The list answers a press at `p`: on its scroll bar or on a row. Anywhere else (the frame's padding and the
    /// gaps between rows included) the press goes on to the window.
    // EVIDENCE: cshell+0x33cbb0 (scroll bar first, then the rows; otherwise 0)
    pub fn takes(&self, p: [f32; 2]) -> bool {
        let on_bar = self.scroll_rect().is_some_and(|s| {
            p[0] >= s[0] as f32
                && p[0] < (s[0] + s[2]) as f32
                && p[1] >= s[1] as f32
                && p[1] < (s[1] + s[3]) as f32
        });
        on_bar || self.row_at(p).is_some()
    }

    /// Top of shown row `k` (0 = the first shown).
    fn row_top(&self, k: usize) -> i32 {
        self.rect()[1] + TOP_PAD + k as i32 * (row_height(self.font_size) + SPACING)
    }

    /// The scroll bar's column (x, y, w, h), when the list scrolls.
    fn scroll_rect(&self) -> Option<[i32; 4]> {
        self.bar.as_ref().map(|b| b.rect)
    }

    /// The row under `p` (index in the whole list): x from the frame's left + 2 to its right - 2, y within the row's
    /// text height.
    // EVIDENCE: cshell+0x33cbb0 (x from list x + 2, width - 4; y from the row's top over its height)
    pub fn row_at(&self, p: [f32; 2]) -> Option<usize> {
        let r = self.rect();
        let right = match self.scroll_rect() {
            Some(s) => s[0],
            None => r[0] + r[2] - 2,
        };
        if p[0] < (r[0] + 2) as f32 || p[0] >= right as f32 {
            return None;
        }
        let h = row_height(self.font_size) as f32;
        self.shown().enumerate().find_map(|(k, i)| {
            let top = self.row_top(k) as f32;
            (p[1] >= top && p[1] < top + h).then_some(i)
        })
    }

    /// Where the hover bar sits (x, y, w, h): under the hovered row while it is shown, from the frame's left + 2 to
    /// the scroll bar (or to the frame's right - 2), as high as the row.
    // DISASM: cshell+0x33c200 (pointer move: per shown row, left = list x + 2, right = left + list width - bar width
    // - 4, top / bottom = the row's; inside -> the bar control sized (right - left, row height), moved to (left, top)
    // and shown, else hidden)
    pub fn highlight_rect(&self) -> Option<[i32; 4]> {
        let i = self.hover?;
        let k = self.shown().position(|j| j == i)?;
        let r = self.rect();
        let bar_w = if self.bar.is_some() { SCROLL_WIDTH } else { 0 };
        Some([
            r[0] + 2,
            self.row_top(k),
            r[2] - bar_w - 4,
            row_height(self.font_size),
        ])
    }

    /// -1 / +1 when `p` is on the scroll bar's up / down arrow.
    pub fn arrow_at(&self, p: [f32; 2]) -> Option<i32> {
        self.bar.as_ref()?.arrow_at(p)
    }

    /// The centre of row `i` (index in the whole list) while it is shown; scripted players click there.
    pub fn row_point(&self, i: usize) -> Option<[f32; 2]> {
        let k = self.shown().position(|j| j == i)?;
        let r = self.rect();
        let right = self.scroll_rect().map_or(r[0] + r[2] - 2, |s| s[0]);
        let x = (r[0] + 2 + right) as f32 / 2.0;
        let y = self.row_top(k) as f32 + row_height(self.font_size) as f32 / 2.0;
        Some([x, y])
    }

    /// The centre of the scroll bar's up (`-1`) or down (`1`) arrow, when the list scrolls.
    pub fn arrow_point(&self, dir: i32) -> Option<[f32; 2]> {
        Some(self.bar.as_ref()?.arrow_point(dir))
    }

    /// Moves the bar by `d` steps (rows), as `d` arrow presses do.
    pub fn scroll_by(&mut self, d: i32) {
        if let Some(b) = self.bar.as_mut() {
            b.value = (b.value + d * b.step).clamp(0, b.top());
        }
        self.first_from_bar();
    }

    /// Draws the list; returns the draw-list index and the shown-row number of each row text it pushed.
    pub fn draw(
        &self,
        cat: &mut Catalog,
        art: &DropArt,
        out: &mut Vec<DrawItem>,
    ) -> Vec<(usize, usize)> {
        let r = self.rect();
        // the frame: the texture in nine parts; without it an 80 % black fill
        // DC-DEFAULT: CLI-502 (proposed) without the frame texture an 80 % black fill
        let drawn = match art.frame.as_deref() {
            Some(t) => super::nine::draw(cat, t, r, [255; 4], out),
            None => 0,
        };
        if drawn == 0 {
            out.push(DrawItem::Fill {
                rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
                colour: [0, 0, 0, 204],
            });
        }
        // the hover bar under the row the pointer is on
        if let (Some(t), Some(h)) = (art.highlight.as_deref(), self.highlight_rect()) {
            super::nine::draw(cat, t, h, [255; 4], out);
        }
        if let Some(b) = &self.bar {
            let mut b = b.clone();
            b.art = art.scroll.clone();
            b.draw(cat, out);
        }
        let centre = Align {
            first: if self.left {
                super::sheet::ALIGN_LEFT
            } else {
                super::sheet::ALIGN_CENTRE
            },
            second: 0,
        };
        let mut rows = Vec::new();
        for (k, i) in self.shown().enumerate() {
            rows.push((out.len(), k));
            out.push(place_text(
                &self.items[i],
                [r[0] + if self.left { LEFT_PAD } else { 0 }, self.row_top(k)],
                [r[2] - if self.left { LEFT_PAD } else { 0 }, 0],
                centre,
                ROW_FONT,
                self.font_size,
                ROW_COLOUR,
            ));
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: usize, scroll: bool) -> Dropdown {
        Dropdown::new(
            "goal",
            [700, 300, 120, 20],
            (0..n)
                .map(|i| (format!("row {i}"), i as i32 * 10))
                .collect(),
            ROOM_ROW_SIZE,
            scroll,
        )
    }

    #[test]
    fn the_list_opens_three_pixels_under_its_control_as_wide_as_it() {
        let d = list(3, false);
        // y = 300 + 20 + 3; height = 4 + 3 x (14 + 4)
        assert_eq!(d.rect(), [700, 323, 120, 58]);
        assert_eq!(
            rule_frame(d.anchor, 3, ROOM_ROW_SIZE),
            [700.0, 323.0, 120.0, 58.0]
        );
        let w = rule_row("goal", d.anchor, 2, ROOM_ROW_SIZE);
        assert_eq!(w.text_at, [760.0, 323.0 + 4.0 + 36.0]);
        assert_eq!(w.key, "dyn/drop/goal/row2");
    }

    #[test]
    fn row_and_arrow_points_hit_what_they_name() {
        let d = list(3, false);
        for i in 0..3 {
            let p = d.row_point(i).unwrap();
            assert!(d.takes(p));
            assert_eq!(d.row_at(p), Some(i));
        }
        assert_eq!(d.arrow_point(1), None);
        let mut d = list(MAX_ROWS + 4, true);
        assert_eq!(d.row_point(MAX_ROWS + 1), None);
        assert_eq!(d.arrow_at(d.arrow_point(1).unwrap()), Some(1));
        assert_eq!(d.arrow_at(d.arrow_point(-1).unwrap()), Some(-1));
        d.scroll_by(4);
        let p = d.row_point(MAX_ROWS + 1).unwrap();
        assert_eq!(d.row_at(p), Some(MAX_ROWS + 1));
    }

    #[test]
    fn rows_are_hit_inside_the_frame_less_two_pixels() {
        let d = list(3, false);
        assert_eq!(d.row_at([702.0, 327.0]), Some(0));
        assert_eq!(d.row_at([701.0, 327.0]), None);
        assert_eq!(d.row_at([760.0, 327.0 + 18.0]), Some(1));
        // the spacing between two rows picks nothing
        assert_eq!(d.row_at([760.0, 327.0 + 15.0]), None);
        assert_eq!(d.row_at([760.0, 200.0]), None);
    }

    #[test]
    fn the_list_takes_presses_on_rows_and_its_scroll_bar_only() {
        let d = list(3, false);
        assert!(d.takes([760.0, 327.0]));
        // inside the frame: the spacing between two rows and the 2 px border are the window's
        assert!(d.contains([760.0, 327.0 + 15.0]) && !d.takes([760.0, 327.0 + 15.0]));
        assert!(d.contains([701.0, 327.0]) && !d.takes([701.0, 327.0]));
        let s = list(8, true);
        let r = s.rect();
        let mid_bar = [
            (r[0] + r[2] - SCROLL_FROM_RIGHT + 2) as f32,
            (r[1] + r[3] / 2) as f32,
        ];
        assert_eq!(s.arrow_at(mid_bar), None);
        assert!(s.takes(mid_bar), "the scroll bar's track is the list's");
    }

    #[test]
    fn only_a_scrolling_list_shows_five_rows_and_moves_by_its_arrows() {
        let fixed = list(8, false);
        assert_eq!(fixed.shown(), 0..8);
        let mut d = list(8, true);
        assert_eq!(d.shown(), 0..5);
        let r = d.rect();
        let up = [
            (r[0] + r[2] - SCROLL_FROM_RIGHT + 2) as f32,
            (r[1] + 2) as f32,
        ];
        let down = [up[0], (r[1] + r[3] - 2) as f32];
        assert_eq!(d.arrow_at(up), Some(-1));
        assert_eq!(d.arrow_at(down), Some(1));
        d.scroll_by(1);
        assert_eq!(d.shown(), 1..6);
        // the row under the pointer is the index in the whole list
        assert_eq!(d.row_at([710.0, (r[1] + 5) as f32]), Some(1));
        d.scroll_by(10);
        assert_eq!(d.shown(), 3..8);
        d.scroll_by(-10);
        assert_eq!(d.first, 0);
    }

    /// The 19-map list of the waiting room: the wheel and the thumb scroll the shown rows (item 1 of the lobby
    /// phase: "scrolling drop downs don't work").
    #[test]
    fn the_wheel_and_the_thumb_scroll_the_shown_rows() {
        let mut d = list(19, true);
        assert_eq!(d.shown(), 0..5);
        // wheel towards the user: one row per turn, whatever its size
        d.wheel(-1);
        assert_eq!(d.shown(), 1..6);
        d.wheel(-3);
        assert_eq!(d.shown(), 2..7);
        assert_eq!(d.bar.as_ref().unwrap().value, 2 * 18);
        for _ in 0..30 {
            d.wheel(-1);
        }
        assert_eq!(d.shown(), 14..19);
        let b = d.bar.clone().unwrap();
        assert_eq!(b.value, b.top());
        // the thumb sits at the track's end
        let (len, off) = b.thumb();
        assert_eq!(off, b.track() - len);
        d.wheel(1);
        assert_eq!(d.shown(), 13..18);
        for _ in 0..30 {
            d.wheel(1);
        }
        assert_eq!(d.shown(), 0..5);
        assert_eq!(d.bar.as_ref().unwrap().value, 0);
        // a press on the track jumps there and drags; the rows follow the pointer until the release
        let b = d.bar.clone().unwrap();
        let low = [b.rect[0] as f32 + 7.0, (b.rect[1] + b.rect[3] - 10) as f32];
        assert!(d.takes(low));
        assert!(d.press_bar(low));
        assert!(d.dragging());
        assert_eq!(d.shown(), 14..19);
        let mid = b.drag_point(b.top() / 2);
        assert!(d.drag_to(mid));
        let s = d.shown();
        assert!(s.start > 4 && s.start < 10, "{s:?}");
        d.release();
        assert!(!d.drag_to([low[0], 0.0]));
        assert_eq!(d.shown(), s);
        // the arrows step one row
        let mut d = list(19, true);
        assert!(d.press_bar(d.arrow_point(1).unwrap()));
        assert_eq!(d.shown(), 1..6);
    }
}
