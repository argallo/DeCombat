//! use-items: the country window of the function item of `SubType` 17 (the country code change). A code-built popup
//! (layout section `CountryChangeWnd`): the title, a notice with the current country, a preview row (the current flag
//! and short name, an arrow, the chosen flag and short name), a drop-down button with the chosen country's full name
//! that opens the country list, a message line and OK / Cancel. Picking the current country disables OK; OK shows the
//! confirmation (the same frame: the chosen flag and name, a question, Back / OK) whose OK sends 0x330 with the chosen
//! nation at +0x44. The ok answer sets the own nation and deletes the item; Cancel shows the function-item cancelled
//! box.
// SPEC: PROTOCOL_2009.md §4.3 (0x330 +0x44 nation; 0x331 kind 17 reads +6 / +0xa)
// EVIDENCE: cshell+0x2a9350 (the class, vtable VA 0x374f8c5c), cshell+0x2a9e30 (build), cshell+0x2a99d0 (open),
// cshell+0x2a9c00 (commands), cshell+0x2a9560 (the confirmation), cshell+0x2a9660 (a pick), cshell+0x2a9440 (send),
// cshell+0x2a94b0 (0x331 reader)

use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::pages::Align;
use std::path::Path;

use super::buybox::fill_form;
use super::catalog::{int, key, pair, Catalog, KeyPin};
use super::popup::wrap_runs;
use super::sheet::{image, image_fit, place_text, ALIGN_CENTRE, ALIGN_LEFT, ALIGN_RIGHT};
use super::usebox::{
    draw_button, fill, inside, measure, NameArt, BUTTON_SIZE, CANCEL_TEXT, FONT, FONT_SIZE, OK_TEXT,
};
use crate::hud::items::DrawItem;

/// The window's commands: OK 0x268 (the confirmation), Cancel 0x267, the confirmation's OK 0x271 (send) and Back
/// 0x272, a list row 0x276, the drop-down button 0x277.
// EVIDENCE: cshell+0x2a9e30 (buttons 0x268 / 0x267 at OKButtonPos / CancelButtonPos; the confirmation's 0x272 at
// OKButtonPos captioned 0x3e, 0x271 at CancelButtonPos captioned 0x42; the list's command 0x276; the drop-down 0x277)
pub const CMD_OK: i32 = 0x268;
pub const CMD_CANCEL: i32 = 0x267;
pub const CMD_SEND: i32 = 0x271;
pub const CMD_BACK: i32 = 0x272;

/// The strings: title, the notice form (the current full name), the list's title, the preview's title, "same
/// country", "approved", the confirmation's question, Back.
// EVIDENCE: cshell+0x2a9e30 (title 0x18a3, preview title 0x15f4, list title 0x18a5, confirmation 0x18a8, Back 0x3e),
// cshell+0x2a99d0 (notice form 0x18a4), cshell+0x2a9c00 (0x276: same as the current -> 0x18a6, else 0x18a7)
pub const TITLE_TEXT: i32 = 0x18a3;
pub const NOTICE_TEXT: i32 = 0x18a4;
pub const PICK_TITLE_TEXT: i32 = 0x18a5;
pub const PREVIEW_TEXT: i32 = 0x15f4;
pub const SAME_TEXT: i32 = 0x18a6;
pub const APPROVED_TEXT: i32 = 0x18a7;
pub const CONFIRM_TEXT: i32 = 0x18a8;
pub const BACK_TEXT: i32 = 0x3e;

/// Colours: the labels (preview and list titles) grey, the message line red, the other texts white.
// EVIDENCE: cshell+0x2a9e30 (0xff7d7d7d for the two labels, 0xffff0000 for the message line, -1 for the others)
pub const LABEL_COLOUR: u32 = 0xff7d_7d7d;
pub const ERROR_COLOUR: u32 = 0xffff_0000;
pub const WHITE: u32 = 0xffff_ffff;
/// The names next to the flags are font 0xf.
// EVIDENCE: cshell+0x2a9e30 (MyCountryNamePos, SelectCountryNamePos, ConfirmPreViewNamePos: size 0xf)
pub const NAME_SIZE: i32 = 0xf;
/// The labels are right-aligned on their point at a width of 200.
// EVIDENCE: cshell+0x2a9e30 (the two labels: width 200 through vfunc +0x140, flags 0x12 through vfunc +0x160)
pub const LABEL_WIDTH: i32 = 200;
/// The list's row height.
// EVIDENCE: cshell+0x2a9e30 (the list control's +0x804 = 0x12)
pub const ROW_HEIGHT: i32 = 0x12;

/// The layout section and its keys.
// EVIDENCE: cshell+0x2a9e30 (section literal VA 0x374d5be8 and the keys below)
pub mod keys {
    use super::{key, KeyPin};
    pub use crate::menu::usebox::keys::{TITLE_POS, WINDOW_POS, WINDOW_RECT};
    pub const SECTION: KeyPin = key("b2cd7d4e0574ab2a", 16);
    pub const STRING_WIDTH: KeyPin = key("c098e9d5e5bec77d", 16);
    pub const NOTICE_POS: KeyPin = key("820535e59c17b025", 9);
    pub const PREVIEW_TITLE_POS: KeyPin = key("2891821336d78681", 15);
    pub const PREVIEW_BG_POS: KeyPin = key("9efbc44aacdfe332", 12);
    pub const PREVIEW_BG_SIZE: KeyPin = key("bb84ad9de9c64046", 13);
    pub const MY_FLAG_POS: KeyPin = key("15308ede543dfdf8", 12);
    pub const MY_NAME_POS: KeyPin = key("7c038a1b28b3c56d", 16);
    pub const ARROW_POS: KeyPin = key("f8d206fe9d7f5829", 8);
    pub const ARROW_SIZE: KeyPin = key("de46b35ec67ffcd3", 9);
    pub const PICK_FLAG_POS: KeyPin = key("94242d4d2a12f840", 16);
    pub const PICK_NAME_POS: KeyPin = key("ede731eb293c10e3", 20);
    pub const PICK_TITLE_POS: KeyPin = key("a8dab857aa409e3a", 21);
    pub const BUTTON_POS: KeyPin = key("b8ebd3f29e9b665b", 19);
    pub const BUTTON_SIZE: KeyPin = key("1ed8602e9d79647b", 23);
    pub const FULL_NAME_POS: KeyPin = key("94730febd84a9240", 24);
    pub const LIST_POS: KeyPin = key("0222ef8ec244fec1", 14);
    pub const LIST_SIZE: KeyPin = key("56cdb4688751b8f0", 15);
    pub const ERROR_POS: KeyPin = key("8705873c1ad4408a", 11);
    pub const OK_POS: KeyPin = key("f9a8656e8e00a64a", 11);
    pub const CANCEL_POS: KeyPin = key("4f52114eebe8187f", 15);
    pub const CONFIRM_NOTICE_POS: KeyPin = key("62966eccbcc1dd7d", 16);
    pub const CONFIRM_BG_POS: KeyPin = key("c92a635ad8c0ccd0", 19);
    pub const CONFIRM_FLAG_POS: KeyPin = key("3fe47f56d08e82fd", 21);
    pub const CONFIRM_NAME_POS: KeyPin = key("2a92629030c5e603", 21);
    pub const FLAG_SIZE: KeyPin = key("662f0e51326b863c", 8);
}

/// The window's places (window origin, then offsets inside it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountryLayout {
    pub window_at: [i32; 2],
    pub window_size: [i32; 2],
    pub string_width: i32,
    pub title_at: [i32; 2],
    pub notice_at: [i32; 2],
    pub preview_title_at: [i32; 2],
    pub preview_bg_at: [i32; 2],
    pub preview_bg_size: [i32; 2],
    pub my_flag_at: [i32; 2],
    pub my_name_at: [i32; 2],
    pub arrow_at: [i32; 2],
    pub arrow_size: [i32; 2],
    pub pick_flag_at: [i32; 2],
    pub pick_name_at: [i32; 2],
    pub pick_title_at: [i32; 2],
    pub button_at: [i32; 2],
    pub button_size: [i32; 2],
    pub full_name_at: [i32; 2],
    pub list_at: [i32; 2],
    pub list_size: [i32; 2],
    pub error_at: [i32; 2],
    pub ok_at: [i32; 2],
    pub cancel_at: [i32; 2],
    pub confirm_notice_at: [i32; 2],
    pub confirm_bg_at: [i32; 2],
    pub confirm_flag_at: [i32; 2],
    pub confirm_name_at: [i32; 2],
    pub flag_size: [i32; 2],
}

/// DC-DEFAULT: CLI-1873 (proposed) our fallback when the ROM's layout section is missing: a 600 x 400 window on a
/// simple grid of our own round numbers (the real places are read from the ROM first).
impl Default for CountryLayout {
    fn default() -> Self {
        CountryLayout {
            window_at: [212, 184],
            window_size: [600, 400],
            string_width: 560,
            title_at: [10, 10],
            notice_at: [20, 40],
            preview_title_at: [20, 100],
            preview_bg_at: [100, 90],
            preview_bg_size: [400, 40],
            my_flag_at: [150, 100],
            my_name_at: [200, 100],
            arrow_at: [300, 100],
            arrow_size: [20, 10],
            pick_flag_at: [350, 100],
            pick_name_at: [400, 100],
            pick_title_at: [20, 150],
            button_at: [100, 140],
            button_size: [400, 30],
            full_name_at: [110, 145],
            list_at: [100, 170],
            list_size: [400, 150],
            error_at: [200, 330],
            ok_at: [200, 360],
            cancel_at: [320, 360],
            confirm_notice_at: [200, 60],
            confirm_bg_at: [100, 110],
            confirm_flag_at: [200, 120],
            confirm_name_at: [250, 120],
            flag_size: [20, 14],
        }
    }
}

impl CountryLayout {
    pub fn read(cat: &Catalog) -> CountryLayout {
        let d = CountryLayout::default();
        let p = |k: &KeyPin, or: [i32; 2]| pair(cat.section_value(&keys::SECTION, k)).unwrap_or(or);
        let n = |k: &KeyPin, or: i32| int(cat.section_value(&keys::SECTION, k)).unwrap_or(or);
        CountryLayout {
            window_at: p(&keys::WINDOW_POS, d.window_at),
            window_size: p(&keys::WINDOW_RECT, d.window_size),
            string_width: n(&keys::STRING_WIDTH, d.string_width),
            title_at: p(&keys::TITLE_POS, d.title_at),
            notice_at: p(&keys::NOTICE_POS, d.notice_at),
            preview_title_at: p(&keys::PREVIEW_TITLE_POS, d.preview_title_at),
            preview_bg_at: p(&keys::PREVIEW_BG_POS, d.preview_bg_at),
            preview_bg_size: p(&keys::PREVIEW_BG_SIZE, d.preview_bg_size),
            my_flag_at: p(&keys::MY_FLAG_POS, d.my_flag_at),
            my_name_at: p(&keys::MY_NAME_POS, d.my_name_at),
            arrow_at: p(&keys::ARROW_POS, d.arrow_at),
            arrow_size: p(&keys::ARROW_SIZE, d.arrow_size),
            pick_flag_at: p(&keys::PICK_FLAG_POS, d.pick_flag_at),
            pick_name_at: p(&keys::PICK_NAME_POS, d.pick_name_at),
            pick_title_at: p(&keys::PICK_TITLE_POS, d.pick_title_at),
            button_at: p(&keys::BUTTON_POS, d.button_at),
            button_size: p(&keys::BUTTON_SIZE, d.button_size),
            full_name_at: p(&keys::FULL_NAME_POS, d.full_name_at),
            list_at: p(&keys::LIST_POS, d.list_at),
            list_size: p(&keys::LIST_SIZE, d.list_size),
            error_at: p(&keys::ERROR_POS, d.error_at),
            ok_at: p(&keys::OK_POS, d.ok_at),
            cancel_at: p(&keys::CANCEL_POS, d.cancel_at),
            confirm_notice_at: p(&keys::CONFIRM_NOTICE_POS, d.confirm_notice_at),
            confirm_bg_at: p(&keys::CONFIRM_BG_POS, d.confirm_bg_at),
            confirm_flag_at: p(&keys::CONFIRM_FLAG_POS, d.confirm_flag_at),
            confirm_name_at: p(&keys::CONFIRM_NAME_POS, d.confirm_name_at),
            flag_size: p(&keys::FLAG_SIZE, d.flag_size),
        }
    }

    fn abs(&self, at: [i32; 2]) -> [i32; 2] {
        [self.window_at[0] + at[0], self.window_at[1] + at[1]]
    }

    /// The list rows that fit.
    pub fn list_rows(&self) -> usize {
        (self.list_size[1] / ROW_HEIGHT).max(1) as usize
    }
}

/// Texture literals: the popup frame, the preview background, the arrow.
// DISASM: cshell+0x2a9ea3 (frame VA 0x374f2d88), cshell+0x2a9f6b (preview background VA 0x374f8e8c), cshell+0x2aa081
// (arrow VA 0x374f8e2c)
pub const WINDOW_TEX: Pin = pin("country window", 0x3f2d88, "0f424f7d41eb452f", 40);
pub const PREVIEW_TEX: Pin = pin("country preview", 0x3f8e8c, "cc52dcc89bf21740", 52);
pub const ARROW_TEX: Pin = pin("country arrow", 0x3f8e2c, "2edfaff848582607", 47);
const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
    Pin {
        what,
        rva,
        sha16,
        len,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CountryArt {
    pub window: Option<String>,
    pub preview: Option<String>,
    pub arrow: Option<String>,
}

impl CountryArt {
    pub fn from_rom(rom: Option<&Path>) -> CountryArt {
        let Some(pe) = rom.and_then(|r| {
            dc_rom::PeFile::from_file(&r.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1])).ok()
        }) else {
            return CountryArt::default();
        };
        let get = |p: &Pin| read_pinned(&pe, p).ok();
        CountryArt {
            window: get(&WINDOW_TEX),
            preview: get(&PREVIEW_TEX),
            arrow: get(&ARROW_TEX),
        }
    }
}

/// One row of the ROM country table (`FullName`, `SimpleName`, `FlagImg`); the nation number is its index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Country {
    pub full: String,
    pub short: String,
    pub flag: String,
}

/// What a press or a key did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountryStep {
    None,
    /// The confirmation's OK: use the item with this nation (0x330 +0x44).
    Use(u32),
    /// Cancel: the window closes with the cancelled box.
    Cancel,
}

/// The open country window.
#[derive(Debug, Clone, PartialEq)]
pub struct CountryBox {
    pub uid: u32,
    /// The own nation when the window opened.
    pub current: u32,
    /// The picked nation (0 = none yet).
    pub picked: u32,
    pub list_open: bool,
    /// The first list row shown.
    pub list_top: usize,
    /// The confirmation is up.
    pub confirm: bool,
    /// The use waits for its answer.
    pub waiting: bool,
    /// The message line (0 = hidden).
    pub message: i32,
    /// The ROM table; nation `n` is row `n`, the list shows rows 1.. .
    pub countries: Vec<Country>,
    pub layout: CountryLayout,
}

const TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};
const CENTRE: Align = Align {
    first: ALIGN_CENTRE,
    second: 0,
};
const RIGHT: Align = Align {
    first: ALIGN_RIGHT,
    second: 0,
};

impl CountryBox {
    /// The window for item `uid` with the own nation `current`.
    // EVIDENCE: cshell+0x2a99d0 (open: the current nation from shell +0x444, its flag and short name in the preview,
    // the notice form with its full name, nothing picked, the message line, the list, the picked flag and name and
    // the confirmation hidden, OK disabled)
    pub fn open(cat: &Catalog, uid: u32, current: u32, countries: Vec<Country>) -> CountryBox {
        CountryBox {
            uid,
            current,
            picked: 0,
            list_open: false,
            list_top: 0,
            confirm: false,
            waiting: false,
            message: 0,
            countries,
            layout: CountryLayout::read(cat),
        }
    }

    /// The list's entries: the table rows past the first.
    // EVIDENCE: cshell+0x2a9e30 (the list is filled with the full names of rows 1 .. count - 1)
    pub fn list_len(&self) -> usize {
        self.countries.len().saturating_sub(1)
    }

    fn country(&self, n: u32) -> Option<&Country> {
        self.countries.get(n as usize)
    }

    /// OK of the main window is enabled: a country other than the current one is picked.
    pub fn ok_enabled(&self) -> bool {
        self.picked != 0 && self.picked != self.current
    }

    /// List row `row` (0-based) was picked: nation `row + 1`; the list closes and the message line says whether the
    /// pick is the current country.
    // EVIDENCE: cshell+0x2a9c00 (0x276: the list hidden, +0x28 = row + 1, cshell+0x2a9660 shows the picked flag, short
    // and full name; the same as the current -> OK disabled, 0x18a6; else OK enabled, 0x18a7; the line shown)
    pub fn pick(&mut self, row: usize) {
        if row >= self.list_len() {
            return;
        }
        self.list_open = false;
        self.picked = row as u32 + 1;
        self.message = if self.picked == self.current {
            SAME_TEXT
        } else {
            APPROVED_TEXT
        };
    }

    /// Scrolls the open list by `notches` (positive = up).
    // DC-DEFAULT: CLI-1873 (proposed) the list scrolls by the wheel, one row a notch (the 2009 list's scroll bar is not
    // drawn)
    pub fn wheel(&mut self, notches: i32) {
        if !self.list_open {
            return;
        }
        let max = self.list_len().saturating_sub(self.layout.list_rows());
        let top = self.list_top as i64 - i64::from(notches);
        self.list_top = top.clamp(0, max as i64) as usize;
    }

    /// OK (or Enter): the confirmation, or the confirmation's OK.
    // EVIDENCE: cshell+0x2a9c00 (0x268 -> cshell+0x2a9560(1): the confirmation with the picked flag and short name;
    // 0x271 -> the send vfunc +0x3c = cshell+0x2a9440: only a picked nation that is not the current one, then busy)
    // DC-DEFAULT: CLI-1873 (proposed) Enter runs OK (of the window that is up), Escape runs Back on the confirmation
    // and Cancel on the window
    pub fn ok(&mut self) -> CountryStep {
        if self.waiting || !self.ok_enabled() {
            return CountryStep::None;
        }
        if !self.confirm {
            self.confirm = true;
            self.list_open = false;
            return CountryStep::None;
        }
        self.waiting = true;
        CountryStep::Use(self.picked)
    }

    /// Escape: Back on the confirmation, else Cancel.
    pub fn escape(&mut self) -> CountryStep {
        if self.confirm {
            if !self.waiting {
                self.confirm = false;
            }
            CountryStep::None
        } else {
            CountryStep::Cancel
        }
    }

    /// A refused use: the window stays, the confirmation can be sent again.
    // EVIDENCE: cshell+0x2a94b0 (ok = 0: the reason box cshell+0x1393f0; the window is not closed)
    pub fn refused(&mut self) {
        self.waiting = false;
    }

    /// A press at `p`; `None` outside the window.
    pub fn press(&mut self, p: [f32; 2]) -> Option<CountryStep> {
        let l = self.layout;
        if self.confirm {
            if !inside(l.window_at, l.window_size, p) {
                return None;
            }
            if inside(l.abs(l.cancel_at), BUTTON_SIZE, p) {
                return Some(self.ok());
            }
            if inside(l.abs(l.ok_at), BUTTON_SIZE, p) {
                return Some(self.escape());
            }
            return Some(CountryStep::None);
        }
        // the open list reaches past the window's bottom
        if self.list_open && inside(l.abs(l.list_at), l.list_size, p) {
            let row = ((p[1] - l.abs(l.list_at)[1] as f32) / ROW_HEIGHT as f32) as usize;
            self.pick(self.list_top + row);
            return Some(CountryStep::None);
        }
        if !inside(l.window_at, l.window_size, p) {
            return None;
        }
        // EVIDENCE: cshell+0x2a9c00 (0x277: the list shown when hidden, hidden when shown)
        if inside(l.abs(l.button_at), l.button_size, p) {
            self.list_open = !self.list_open;
            return Some(CountryStep::None);
        }
        self.list_open = false;
        // DC-DEFAULT: CLI-1873 (proposed) the later-built button (Cancel) takes a press where the 0x78-wide hit areas
        // overlap
        if inside(l.abs(l.cancel_at), BUTTON_SIZE, p) {
            return Some(CountryStep::Cancel);
        }
        if inside(l.abs(l.ok_at), BUTTON_SIZE, p) {
            return Some(self.ok());
        }
        Some(CountryStep::None)
    }

    /// The notice: the form with the current country's full name.
    pub fn notice(&self, cat: &Catalog) -> String {
        let name = self.country(self.current).map_or("", |c| c.full.as_str());
        fill_form(&cat.text(NOTICE_TEXT), &[name.trim()])
    }

    fn flag(&self, n: u32, at: [i32; 2], out: &mut Vec<DrawItem>) {
        if let Some(c) = self.country(n).filter(|c| !c.flag.is_empty()) {
            out.push(image(&c.flag, at, self.layout.flag_size));
        }
    }

    fn short(&self, n: u32) -> String {
        self.country(n)
            .map_or(String::new(), |c| c.short.trim().to_string())
    }

    fn runs(
        cat: &mut Catalog,
        text: &str,
        at: [i32; 2],
        width: i32,
        centre: bool,
        out: &mut Vec<DrawItem>,
    ) {
        // DC-DEFAULT: CLI-1873 (proposed) the wrapped texts break at the width over 7 characters a line
        for (k, line) in wrap_runs(text, (width / 7).max(1) as usize, WHITE)
            .iter()
            .enumerate()
        {
            let w: i32 = line.iter().map(|(s, _)| measure(cat, s)).sum();
            let mut x = if centre { at[0] - w / 2 } else { at[0] };
            let y = at[1] + k as i32 * (FONT_SIZE + 2);
            for (s, colour) in line {
                out.push(place_text(s, [x, y], [0, 0], TL, FONT, FONT_SIZE, *colour));
                x += measure(cat, s);
            }
        }
    }

    pub fn draw(
        &self,
        cat: &mut Catalog,
        art: &CountryArt,
        buttons: &NameArt,
        out: &mut Vec<DrawItem>,
    ) {
        let l = self.layout;
        match art.window.as_deref() {
            Some(t) => out.push(image_fit(cat, t, l.window_at, l.window_size)),
            None => out.push(fill(l.window_at, l.window_size, [24, 24, 24, 240])),
        }
        let text = |s: &str, at: [i32; 2], align: Align, size: i32, colour: u32| {
            place_text(s, at, [0, 0], align, FONT, size, colour)
        };
        out.push(text(
            &cat.text(TITLE_TEXT),
            l.abs(l.title_at),
            TL,
            FONT_SIZE,
            WHITE,
        ));
        let bg =
            |at: [i32; 2], cat: &mut Catalog, out: &mut Vec<DrawItem>| match art.preview.as_deref()
            {
                Some(t) => out.push(image_fit(cat, t, at, l.preview_bg_size)),
                None => out.push(fill(at, l.preview_bg_size, [40, 40, 40, 255])),
            };
        if self.confirm {
            bg(l.abs(l.confirm_bg_at), cat, out);
            self.flag(self.picked, l.abs(l.confirm_flag_at), out);
            out.push(text(
                &self.short(self.picked),
                l.abs(l.confirm_name_at),
                TL,
                NAME_SIZE,
                WHITE,
            ));
            let q = cat.text(CONFIRM_TEXT);
            Self::runs(
                cat,
                &q,
                l.abs(l.confirm_notice_at),
                l.string_width,
                true,
                out,
            );
            draw_button(cat, buttons, l.abs(l.ok_at), BACK_TEXT, !self.waiting, out);
            draw_button(
                cat,
                buttons,
                l.abs(l.cancel_at),
                OK_TEXT,
                !self.waiting,
                out,
            );
            return;
        }
        let notice = self.notice(cat);
        Self::runs(cat, &notice, l.abs(l.notice_at), l.string_width, false, out);
        let label = |s: &str, at: [i32; 2]| {
            place_text(
                s,
                [at[0] - LABEL_WIDTH, at[1]],
                [LABEL_WIDTH, 0],
                RIGHT,
                FONT,
                FONT_SIZE,
                LABEL_COLOUR,
            )
        };
        out.push(label(&cat.text(PREVIEW_TEXT), l.abs(l.preview_title_at)));
        bg(l.abs(l.preview_bg_at), cat, out);
        self.flag(self.current, l.abs(l.my_flag_at), out);
        out.push(text(
            &self.short(self.current),
            l.abs(l.my_name_at),
            TL,
            NAME_SIZE,
            WHITE,
        ));
        if self.picked != 0 {
            if let Some(t) = art.arrow.as_deref() {
                out.push(image_fit(cat, t, l.abs(l.arrow_at), l.arrow_size));
            }
            self.flag(self.picked, l.abs(l.pick_flag_at), out);
            out.push(text(
                &self.short(self.picked),
                l.abs(l.pick_name_at),
                TL,
                NAME_SIZE,
                WHITE,
            ));
        }
        out.push(label(&cat.text(PICK_TITLE_TEXT), l.abs(l.pick_title_at)));
        // DC-DEFAULT: CLI-1873 (proposed) the drop-down button and the list are drawn as plain panels (the 2009
        // Dropdown_ forms and BOX_32_32 / BOX_8_8 frames are not read)
        out.push(fill(l.abs(l.button_at), l.button_size, [40, 40, 40, 255]));
        if let Some(c) = self.country(self.picked).filter(|_| self.picked != 0) {
            out.push(text(
                c.full.trim(),
                l.abs(l.full_name_at),
                TL,
                FONT_SIZE,
                WHITE,
            ));
        }
        if self.message != 0 && !self.list_open {
            out.push(text(
                &cat.text(self.message),
                l.abs(l.error_at),
                CENTRE,
                FONT_SIZE,
                ERROR_COLOUR,
            ));
        }
        draw_button(
            cat,
            buttons,
            l.abs(l.ok_at),
            OK_TEXT,
            self.ok_enabled() && !self.waiting,
            out,
        );
        draw_button(cat, buttons, l.abs(l.cancel_at), CANCEL_TEXT, true, out);
        if self.list_open {
            let at = l.abs(l.list_at);
            out.push(fill(at, l.list_size, [16, 16, 16, 245]));
            for (k, c) in self
                .countries
                .iter()
                .skip(1 + self.list_top)
                .take(l.list_rows())
                .enumerate()
            {
                let y = at[1] + k as i32 * ROW_HEIGHT;
                if self.picked as usize == 1 + self.list_top + k {
                    out.push(fill(
                        [at[0], y],
                        [l.list_size[0], ROW_HEIGHT],
                        [70, 70, 70, 255],
                    ));
                }
                out.push(text(
                    c.full.trim(),
                    [at[0] + 6, y + 2],
                    TL,
                    FONT_SIZE,
                    WHITE,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Vec<Country> {
        (0..30)
            .map(|n| Country {
                full: format!("Land {n}"),
                short: format!("L{n}"),
                flag: format!("flag{n}.dtx"),
            })
            .collect()
    }

    fn boxed(current: u32) -> CountryBox {
        let c = Catalog::fallback(None, 1024, 768);
        CountryBox::open(&c, 9, current, table())
    }

    #[test]
    fn our_united_states_row_follows_the_rom_rows_once() {
        use crate::menu::charinfo::add_own_countries;
        let row = |f: &str, s: &str| (f.to_string(), s.to_string(), format!("{s}.dtx"));
        let mut rows = vec![row("Land 0", "L0"), row("Land 1", "L1")];
        add_own_countries(&mut rows);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[2],
            ("United States".into(), "USA".into(), String::new())
        );
        add_own_countries(&mut rows);
        assert_eq!(rows.len(), 3, "added once");
        let mut none = Vec::new();
        add_own_countries(&mut none);
        assert!(none.is_empty(), "no table, no window rows");
    }

    fn at(b: &CountryBox, q: [i32; 2]) -> [f32; 2] {
        let l = b.layout;
        [
            (l.window_at[0] + q[0] + 5) as f32,
            (l.window_at[1] + q[1] + 5) as f32,
        ]
    }

    #[test]
    fn the_list_picks_nation_row_plus_one_and_the_current_country_keeps_ok_off() {
        let mut b = boxed(3);
        assert_eq!(b.list_len(), 29);
        assert!(!b.ok_enabled());
        let button = at(&b, b.layout.button_at);
        assert_eq!(b.press(button), Some(CountryStep::None));
        assert!(b.list_open);
        // the third row is nation 3: the current one
        let l = b.layout;
        let row = |k: i32| {
            [
                (l.window_at[0] + l.list_at[0] + 10) as f32,
                (l.window_at[1] + l.list_at[1] + k * ROW_HEIGHT + 3) as f32,
            ]
        };
        b.press(row(2));
        assert_eq!((b.picked, b.message, b.list_open), (3, SAME_TEXT, false));
        assert!(!b.ok_enabled());
        assert_eq!(b.ok(), CountryStep::None);
        assert!(!b.confirm);
        b.press(button);
        b.wheel(-3);
        assert_eq!(b.list_top, 3);
        b.press(row(1));
        assert_eq!((b.picked, b.message), (5, APPROVED_TEXT));
        assert!(b.ok_enabled());
    }

    #[test]
    fn ok_confirms_first_then_sends_and_back_returns() {
        let mut b = boxed(0);
        b.pick(6);
        let ok = at(&b, b.layout.ok_at);
        let right = at(&b, b.layout.cancel_at);
        assert_eq!(b.press(ok), Some(CountryStep::None));
        assert!(b.confirm);
        // the confirmation's left button is Back
        assert_eq!(b.press(ok), Some(CountryStep::None));
        assert!(!b.confirm);
        b.press(ok);
        assert_eq!(b.press(right), Some(CountryStep::Use(7)));
        assert!(b.waiting);
        assert_eq!(b.press(right), Some(CountryStep::None));
        b.refused();
        assert_eq!(b.ok(), CountryStep::Use(7));
    }

    #[test]
    fn cancel_and_escape_close_the_window() {
        let mut b = boxed(1);
        let cancel = at(&b, b.layout.cancel_at);
        assert_eq!(b.press(cancel), Some(CountryStep::Cancel));
        assert_eq!(b.press([0.0, 0.0]), None);
        b.pick(4);
        b.ok();
        assert_eq!(b.escape(), CountryStep::None);
        assert!(!b.confirm);
        assert_eq!(b.escape(), CountryStep::Cancel);
    }

    #[test]
    fn the_window_draws_its_preview_and_the_open_list() {
        let mut c = Catalog::fallback(None, 1024, 768);
        let mut b = boxed(2);
        b.pick(4);
        b.list_open = true;
        let mut out = Vec::new();
        b.draw(
            &mut c,
            &CountryArt::default(),
            &NameArt::default(),
            &mut out,
        );
        let images: Vec<&str> = out
            .iter()
            .filter_map(|d| match d {
                DrawItem::Image { tex, .. } => Some(tex.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(images, vec!["flag2.dtx", "flag5.dtx"]);
        let labels = out
            .iter()
            .filter(|d| matches!(d, DrawItem::Label { text, .. } if text.starts_with("Land ")))
            .count();
        assert_eq!(labels, 1 + b.layout.list_rows().min(29));
    }
}
