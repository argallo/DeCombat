//! use-items: the megaphone composer of the function item of `SubType` 22. A code-built popup (layout section
//! `MegaphoneWnd`): the title, a notice, the message field with its caret and "n/65" counter, a grid of the ROM name
//! colour swatches (a pick colours the characters typed after it), a preview line (the own name and the message in its
//! colours), a message line that stands in for the preview title for five seconds, and OK / Cancel. OK asks the lobby
//! for the queue state (0x36e); the answer (0x36f) opens the confirmation (the players waiting, the minutes until the
//! message airs, the uses left; OK / Back / Cancel) whose OK sends the message (0x36c). The answer to the send (0x36d)
//! sets the uses left (0: the item is gone) and closes the composer.
// SPEC: PROTOCOL_2009.md §4.3 (0x36c..0x36f)
// EVIDENCE: cshell+0x2a7020 (the class, vtable VA 0x374f8994), cshell+0x2a7720 (build), cshell+0x2a8870 (open),
// cshell+0x2a8d60 (commands), cshell+0x2a85b0 (the confirmation), cshell+0x2a5be0 (send), cshell+0x2a5c90 (0x36d
// reader), cshell+0x2a69c0 (the message line's five seconds), cshell+0x2a9150 (a press on the colour grid),
// cshell+0x2a90d0 (the pick), cshell+0x2a5d60 (the colour runs); DISASM: cshell+0x2a8990 (0x36f reader)

use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::pages::Align;
use std::path::Path;
use std::time::{Duration, Instant};

use super::catalog::{argb, int, key, pair, Catalog, KeyPin};
use super::colorbox::Swatch;
use super::gm::{printf, Arg};
use super::link::{QueueReply, MEGAPHONE_MAX};
use super::popup::wrap_runs;
use super::sheet::{
    caret_on, fitting_tail, image, image_fit, place_text, ALIGN_CENTRE, ALIGN_LEFT, CARET_PERIOD,
    CARET_WIDTH,
};
use super::textbox::CHAT_CARET_COLOUR;
use super::usebox::{
    draw_button, fill, inside, measure, NameArt, BUTTON_SIZE, CANCEL_TEXT, FONT, FONT_SIZE, OK_TEXT,
};
use crate::hud::items::DrawItem;

/// The strings: title, the notice, the preview title, the confirmation's question, its wait and uses forms, and the
/// message lines (empty message, too many characters, words not allowed).
// EVIDENCE: cshell+0x2a7720 (title 0x18fa, notice 0x18fb, preview title 0x18fc, confirmation 0x18ff, Back 0x3e),
// cshell+0x2a85b0 (wait form 0x1900 with the players waiting and the minutes, uses form 0x1901), cshell+0x2a8d60
// (0x268 with an empty field -> 0x18fe; 0x280 -> 0x1902; the word filter -> 0x18fd)
pub const TITLE_TEXT: i32 = 0x18fa;
pub const NOTICE_TEXT: i32 = 0x18fb;
pub const PREVIEW_TEXT: i32 = 0x18fc;
pub const CONFIRM_TEXT: i32 = 0x18ff;
pub const WAIT_TEXT: i32 = 0x1900;
pub const USES_TEXT: i32 = 0x1901;
pub const EMPTY_TEXT: i32 = 0x18fe;
pub const TOO_LONG_TEXT: i32 = 0x1902;
pub const BACK_TEXT: i32 = 0x3e;

/// Colours: the preview title, the counter, the confirmation's lines, the message line.
// EVIDENCE: cshell+0x2a7720 (preview title 0xfffdff9b, counter 0xffaaaaaa, wait and uses lines 0xffd7d7d7, message
// line 0xffff0000)
pub const PREVIEW_COLOUR: u32 = 0xfffd_ff9b;
pub const COUNT_COLOUR: u32 = 0xffaa_aaaa;
pub const INFO_COLOUR: u32 = 0xffd7_d7d7;
pub const ERROR_COLOUR: u32 = 0xffff_0000;
pub const WHITE: u32 = 0xffff_ffff;

/// The message line gives the preview title back after five seconds.
// EVIDENCE: cshell+0x2a69c0 (more than 5000 ms after the line was set: the preview title shown, the line hidden)
pub const MESSAGE_TIME: Duration = Duration::from_millis(5000);

/// The colour index of a character no pick coloured: the field gives every typed character this index (outside the
/// colour table, so the receiver shows it in its default colour); the runs keep its low half.
// EVIDENCE: cshell+0x2a65f0 (the inserted characters' indices filled with 0x63636363), cshell+0x2a5d60 (the runs take
// each index as a u16)
pub const NO_COLOUR: u16 = 0x6363;

/// The colour grid's slot size (the swatch and the pick mark).
// EVIDENCE: cshell+0x2a7720 (cshell+0x245fb0(0x28, 0x28) after the grid's set-up), cshell+0x245fb0 (both pictures sized
// 0x28 x 0x28, slot (row, column) at (column x (0x28 + column gap), row x (0x28 + row gap)) from the grid's place)
pub const SLOT_SIZE: [i32; 2] = [0x28, 0x28];

/// The layout section and its keys.
// DISASM: cshell+0x2a77ac..0x2a84ba (section literal VA 0x374f8c28 and the keys below)
pub mod keys {
    use super::{key, KeyPin};
    pub use crate::menu::colorbox::keys::{COLUMNS, COLUMN_GAP, ROWS, ROW_GAP};
    pub use crate::menu::countrybox::keys::{
        CANCEL_POS, ERROR_POS, NOTICE_POS, OK_POS, PREVIEW_TITLE_POS, STRING_WIDTH,
    };
    /// The colour grid's place.
    // EVIDENCE: cshell+0x2a7720 (the grid's place key, literal VA 0x374f8b5c; rows, columns, row and column gaps: the
    // colour window's key literals VA 0x374f321c / 0x374f3208 / 0x374f31ec / 0x374f31d0)
    pub const LIST_POS: KeyPin = key("e22dc0eaaf3e67f0", 13);
    pub use crate::menu::usebox::keys::{TITLE_POS, WINDOW_POS, WINDOW_RECT};
    pub const SECTION: KeyPin = key("f000c11afa184952", 12);
    pub const EDIT_POS: KeyPin = key("92be354668d77623", 12);
    pub const EDIT_WIDTH: KeyPin = key("de3a4d98d6bd347f", 14);
    pub const PREVIEW_POS: KeyPin = key("ad83d1bf4cb1c856", 10);
    pub const PREVIEW_SIZE: KeyPin = key("dccdef2830c9ec8e", 11);
    pub const COUNT_POS: KeyPin = key("510b6cdabf587f3e", 14);
    pub const C_STRING_WIDTH: KeyPin = key("9636679468bf0986", 24);
    pub const C_WINDOW_POS: KeyPin = key("c1c137e6cf109661", 17);
    pub const C_WINDOW_RECT: KeyPin = key("7c4dc0b29774fc2a", 18);
    pub const C_OK_POS: KeyPin = key("4edb45b34a3d50ee", 19);
    pub const C_BACK_POS: KeyPin = key("e3eafce9f41eaa9e", 21);
    pub const C_CANCEL_POS: KeyPin = key("e4de0e725ba8099d", 23);
    pub const C_TITLE_POS: KeyPin = key("4e2dc5e2c93f9f0f", 16);
    pub const C_NOTICE_POS: KeyPin = key("a1e694b8be96362d", 17);
    pub const C_AD_POS: KeyPin = key("17b1c5be605e7343", 16);
    pub const C_WAIT_POS: KeyPin = key("8667e1414583f533", 19);
    pub const C_REMAIN_POS: KeyPin = key("a95d381220fa6a64", 22);
}

/// The places of the composer and of its confirmation (each window's origin, then offsets inside it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MegaLayout {
    pub window_at: [i32; 2],
    pub window_size: [i32; 2],
    pub string_width: i32,
    pub ok_at: [i32; 2],
    pub cancel_at: [i32; 2],
    pub title_at: [i32; 2],
    pub notice_at: [i32; 2],
    pub edit_at: [i32; 2],
    pub edit_width: i32,
    pub preview_title_at: [i32; 2],
    pub error_at: [i32; 2],
    pub preview_at: [i32; 2],
    pub preview_size: [i32; 2],
    pub count_at: [i32; 2],
    pub list_at: [i32; 2],
    pub rows: i32,
    pub columns: i32,
    pub row_gap: i32,
    pub column_gap: i32,
    pub c_window_at: [i32; 2],
    pub c_window_size: [i32; 2],
    pub c_string_width: i32,
    pub c_ok_at: [i32; 2],
    pub c_back_at: [i32; 2],
    pub c_cancel_at: [i32; 2],
    pub c_title_at: [i32; 2],
    pub c_notice_at: [i32; 2],
    pub c_ad_at: [i32; 2],
    pub c_wait_at: [i32; 2],
    pub c_remain_at: [i32; 2],
}

/// DC-DEFAULT: CLI-1874 (proposed) our fallback when the ROM's layout section is missing: a 600 x 400 window on a
/// simple grid of our own round numbers (the real places are read from the ROM first).
impl Default for MegaLayout {
    fn default() -> Self {
        MegaLayout {
            window_at: [212, 184],
            window_size: [600, 400],
            string_width: 560,
            ok_at: [200, 360],
            cancel_at: [320, 360],
            title_at: [10, 10],
            notice_at: [20, 40],
            edit_at: [20, 90],
            edit_width: 400,
            preview_title_at: [20, 300],
            error_at: [20, 300],
            preview_at: [20, 330],
            preview_size: [560, 20],
            count_at: [500, 150],
            list_at: [20, 180],
            rows: 2,
            columns: 10,
            row_gap: 5,
            column_gap: 5,
            c_window_at: [212, 184],
            c_window_size: [600, 400],
            c_string_width: 560,
            c_ok_at: [100, 350],
            c_back_at: [250, 350],
            c_cancel_at: [400, 350],
            c_title_at: [10, 10],
            c_notice_at: [20, 40],
            c_ad_at: [20, 100],
            c_wait_at: [20, 200],
            c_remain_at: [20, 250],
        }
    }
}

impl MegaLayout {
    pub fn read(cat: &Catalog) -> MegaLayout {
        let d = MegaLayout::default();
        let p = |k: &KeyPin, or: [i32; 2]| pair(cat.section_value(&keys::SECTION, k)).unwrap_or(or);
        let n = |k: &KeyPin, or: i32| int(cat.section_value(&keys::SECTION, k)).unwrap_or(or);
        MegaLayout {
            window_at: p(&keys::WINDOW_POS, d.window_at),
            window_size: p(&keys::WINDOW_RECT, d.window_size),
            string_width: n(&keys::STRING_WIDTH, d.string_width),
            ok_at: p(&keys::OK_POS, d.ok_at),
            cancel_at: p(&keys::CANCEL_POS, d.cancel_at),
            title_at: p(&keys::TITLE_POS, d.title_at),
            notice_at: p(&keys::NOTICE_POS, d.notice_at),
            edit_at: p(&keys::EDIT_POS, d.edit_at),
            edit_width: n(&keys::EDIT_WIDTH, d.edit_width),
            preview_title_at: p(&keys::PREVIEW_TITLE_POS, d.preview_title_at),
            error_at: p(&keys::ERROR_POS, d.error_at),
            preview_at: p(&keys::PREVIEW_POS, d.preview_at),
            preview_size: p(&keys::PREVIEW_SIZE, d.preview_size),
            count_at: p(&keys::COUNT_POS, d.count_at),
            list_at: p(&keys::LIST_POS, d.list_at),
            rows: n(&keys::ROWS, d.rows),
            columns: n(&keys::COLUMNS, d.columns),
            row_gap: n(&keys::ROW_GAP, d.row_gap),
            column_gap: n(&keys::COLUMN_GAP, d.column_gap),
            c_window_at: p(&keys::C_WINDOW_POS, d.c_window_at),
            c_window_size: p(&keys::C_WINDOW_RECT, d.c_window_size),
            c_string_width: n(&keys::C_STRING_WIDTH, d.c_string_width),
            c_ok_at: p(&keys::C_OK_POS, d.c_ok_at),
            c_back_at: p(&keys::C_BACK_POS, d.c_back_at),
            c_cancel_at: p(&keys::C_CANCEL_POS, d.c_cancel_at),
            c_title_at: p(&keys::C_TITLE_POS, d.c_title_at),
            c_notice_at: p(&keys::C_NOTICE_POS, d.c_notice_at),
            c_ad_at: p(&keys::C_AD_POS, d.c_ad_at),
            c_wait_at: p(&keys::C_WAIT_POS, d.c_wait_at),
            c_remain_at: p(&keys::C_REMAIN_POS, d.c_remain_at),
        }
    }

    fn abs(&self, at: [i32; 2]) -> [i32; 2] {
        [self.window_at[0] + at[0], self.window_at[1] + at[1]]
    }

    fn c_abs(&self, at: [i32; 2]) -> [i32; 2] {
        [self.c_window_at[0] + at[0], self.c_window_at[1] + at[1]]
    }
}

/// Texture literals: the composer's and the confirmation's frames, the colour grid's pick mark.
// DISASM: cshell+0x2a77c1 (composer VA 0x374f8bec), cshell+0x2a8047 (confirmation VA 0x374f8ad0)
// EVIDENCE: cshell+0x2a7720 (the grid's mark, literal VA 0x374f8b28), cshell+0x246350 (shown on the picked slot)
pub const WINDOW_TEX: Pin = pin("megaphone window", 0x3f8bec, "3ce3fd5ddeee62f2", 59);
pub const CONFIRM_TEX: Pin = pin("megaphone confirmation", 0x3f8ad0, "8b668f288f80d566", 58);
pub const SLOT_ON_TEX: Pin = pin("megaphone colour mark", 0x3f8b28, "d5d05fc614493c17", 48);
const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
    Pin {
        what,
        rva,
        sha16,
        len,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MegaArt {
    pub window: Option<String>,
    pub confirm: Option<String>,
    pub slot_on: Option<String>,
}

impl MegaArt {
    pub fn from_rom(rom: Option<&Path>) -> MegaArt {
        let Some(pe) = rom.and_then(|r| {
            dc_rom::PeFile::from_file(&r.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1])).ok()
        }) else {
            return MegaArt::default();
        };
        let get = |p: &Pin| read_pinned(&pe, p).ok();
        MegaArt {
            window: get(&WINDOW_TEX),
            confirm: get(&CONFIRM_TEX),
            slot_on: get(&SLOT_ON_TEX),
        }
    }
}

/// What a press or a key did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MegaStep {
    None,
    /// Ask for the queue state (0x36e).
    Queue,
    /// Broadcast the message (0x36c).
    Send(String),
    /// Cancel: the composer closes with the cancelled box.
    Cancel,
}

/// The confirmation's numbers: the players waiting and the minutes until the message airs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wait {
    pub players: u32,
    pub minutes: i32,
}

/// The wait the confirmation shows: the orders between the one on air and the last queued, at the seconds per message,
/// in whole minutes plus one.
// DISASM: cshell+0x2a89ad..0x2a89de ((+0x16 - +0x12) * +0xe * 1000 ms / 60000, truncated, + 1; with +0x1e)
pub fn wait_of(r: &QueueReply) -> Wait {
    let ms = i64::from(r.last.wrapping_sub(r.on_air) as i32) * i64::from(r.secs) * 1000;
    Wait {
        players: r.waiting,
        minutes: (ms / 60_000 + 1) as i32,
    }
}

/// The open composer.
#[derive(Debug, Clone, PartialEq)]
pub struct MegaBox {
    pub uid: u32,
    /// The uses the item has left (the confirmation shows them).
    pub uses: u32,
    /// The own name (the preview line).
    pub name: String,
    pub text: String,
    /// The colour index of each character of `text` (a missing entry: [`NO_COLOUR`]).
    pub colours: Vec<u16>,
    /// The colour index typed characters get (the last pick; [`NO_COLOUR`] before one).
    pub pen: u16,
    /// The ROM name colour table (index `n` is entry `n - 1`).
    pub swatches: Vec<Swatch>,
    /// The grid slot picked last (its mark is shown).
    pub picked_slot: Option<usize>,
    /// The confirmation is up with its numbers.
    pub confirm: Option<Wait>,
    /// A queue question or a send waits for its answer.
    pub waiting: bool,
    /// The message line and when it was set.
    pub message: Option<(i32, Instant)>,
    pub layout: MegaLayout,
}

const TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};

impl MegaBox {
    /// The composer for megaphone `uid` with `uses` left.
    // EVIDENCE: cshell+0x2a8870 (open: the confirmation hidden, the uses from the item's record +0x14, the counter
    // "0/65", the field cleared)
    pub fn open(cat: &Catalog, uid: u32, uses: u32, name: &str, swatches: Vec<Swatch>) -> MegaBox {
        MegaBox {
            uid,
            uses,
            name: name.to_string(),
            text: String::new(),
            colours: Vec::new(),
            pen: NO_COLOUR,
            swatches,
            picked_slot: None,
            confirm: None,
            waiting: false,
            message: None,
            layout: MegaLayout::read(cat),
        }
    }

    fn say(&mut self, id: i32) {
        self.message = Some((id, Instant::now()));
    }

    /// The message line shown now (it lasts [`MESSAGE_TIME`]).
    pub fn message_now(&self, now: Instant) -> Option<i32> {
        self.message
            .filter(|(_, t)| now.saturating_duration_since(*t) <= MESSAGE_TIME)
            .map(|(id, _)| id)
    }

    /// A typed character; the field holds at most 65 characters, one more shows the too-long line.
    // EVIDENCE: cshell+0x2a8d60 (command 0x280 with 1 from the field: the line 0x1902), cshell+0x2a7720 (the field
    // reports through command 0x280, cshell+0x2f5530), cshell+0x2a5be0 (0x41 characters go out)
    // DC-DEFAULT: CLI-1874 (proposed) the field takes no control characters (a space is a character: the menu window
    // delivers it as the Space key, the shop types it here); the typed character gets the colour of the last pick
    // (the 2009 field gives it none and colours a selected range on a pick; ours has no selection); the word filter
    // (0x18fd) is not ported: the lobby decides
    pub fn type_char(&mut self, c: char) {
        if self.confirm.is_some() || c.is_control() {
            return;
        }
        let n = self.text.chars().count();
        if n >= MEGAPHONE_MAX {
            self.say(TOO_LONG_TEXT);
            return;
        }
        self.colours.resize(n, NO_COLOUR);
        self.text.push(c);
        self.colours.push(self.pen);
    }

    pub fn backspace(&mut self) {
        if self.confirm.is_none() {
            self.text.pop();
            self.colours.truncate(self.text.chars().count());
        }
    }

    /// The colour index of character `i`.
    pub fn colour_of(&self, i: usize) -> u16 {
        self.colours.get(i).copied().unwrap_or(NO_COLOUR)
    }

    /// The colour runs of the message: {colour index, characters} for each stretch of one index.
    // EVIDENCE: cshell+0x2a5d60 (the per-character indices of +0x234 folded into {index, length} u16 pairs at +0x24,
    // a new pair where the index changes), cshell+0x2a5be0 (+0x24 goes out with the text)
    pub fn colour_runs(&self) -> Vec<(u16, u16)> {
        let mut out: Vec<(u16, u16)> = Vec::new();
        for i in 0..self.text.chars().count() {
            let c = self.colour_of(i);
            match out.last_mut() {
                Some((k, len)) if *k == c => *len += 1,
                _ => out.push((c, 1)),
            }
        }
        out
    }

    /// The colour of colour index `n` (white outside the table, the preview's and the field's default).
    // EVIDENCE: cshell+0x1dcf00 (0 < n <= count: entry n - 1, else 0 = the control's default colour), cshell+0x2a7720
    // (the preview line 0xffffffff)
    pub fn argb_of(&self, n: u16) -> u32 {
        (n as usize)
            .checked_sub(1)
            .and_then(|i| self.swatches.get(i))
            .map_or(WHITE, |s| s.argb)
    }

    /// The grid's slots: the table's entries that fit its rows x columns.
    // EVIDENCE: cshell+0x2a7720 (the grid filled from the name colour table, cshell+0x1ddfa0), cshell+0x246220 (as
    // many slots as entries, at most rows x columns; the rest hidden)
    pub fn slots(&self) -> usize {
        let l = &self.layout;
        self.swatches
            .len()
            .min((l.rows.max(0) * l.columns.max(0)) as usize)
    }

    /// The colour index slot `k` shows: the table's entries from the last one backwards.
    // EVIDENCE: cshell+0x246220 (slot k gets entry count - 1 - k)
    pub fn slot_colour(&self, k: usize) -> Option<u16> {
        (k < self.slots()).then(|| (self.swatches.len() - k) as u16)
    }

    /// Slot `k`'s corner.
    // EVIDENCE: cshell+0x2470c0 (rows x columns slots, row by row), cshell+0x245fb0 (see [`SLOT_SIZE`])
    pub fn slot_at(&self, k: usize) -> [i32; 2] {
        let l = &self.layout;
        let cols = l.columns.max(1);
        let (row, col) = (k as i32 / cols, k as i32 % cols);
        l.abs([
            l.list_at[0] + col * (SLOT_SIZE[0] + l.column_gap),
            l.list_at[1] + row * (SLOT_SIZE[1] + l.row_gap),
        ])
    }

    /// A pick of slot `k`: its colour becomes the pen and the slot shows the mark.
    // EVIDENCE: cshell+0x246350 (the pressed slot's mark shown, the previous one's hidden, its index returned),
    // cshell+0x2a90d0 (the index kept at +0x340 and given to the field's selected characters)
    pub fn pick_slot(&mut self, k: usize) {
        if let Some(n) = self.slot_colour(k) {
            self.pen = n;
            self.picked_slot = Some(k);
        }
    }

    /// OK: an empty field shows its line, else the queue question.
    // EVIDENCE: cshell+0x2a8d60 (0x268: no characters -> the line 0x18fe; else 0x36e through cshell+0x28f5d0, busy)
    pub fn ok(&mut self) -> MegaStep {
        if self.waiting {
            return MegaStep::None;
        }
        if self.confirm.is_some() {
            return self.send();
        }
        if self.text.trim().is_empty() {
            self.say(EMPTY_TEXT);
            return MegaStep::None;
        }
        self.waiting = true;
        MegaStep::Queue
    }

    /// The confirmation's OK: the message goes out.
    // EVIDENCE: cshell+0x2a8d60 (0x271 -> the send vfunc +0x3c = cshell+0x2a5be0: a non-empty text and a unique id ->
    // 0x36c, busy)
    fn send(&mut self) -> MegaStep {
        if self.text.is_empty() || self.uid == 0 {
            return MegaStep::None;
        }
        self.waiting = true;
        MegaStep::Send(self.text.clone())
    }

    /// The queue state: ok opens the confirmation; refused (`false` back) the caller shows the reason box.
    // DISASM: cshell+0x2a8990 (ok: cshell+0x2a85b0(1, waiting, minutes); else the reason box cshell+0x1393f0 with +0xa)
    pub fn queued(&mut self, r: &QueueReply) -> bool {
        self.waiting = false;
        if r.ok {
            self.confirm = Some(wait_of(r));
        }
        r.ok
    }

    /// Escape: Back on the confirmation, else Cancel.
    // DC-DEFAULT: CLI-1874 (proposed) Enter runs OK of the window that is up, Escape Back on the confirmation and
    // Cancel on the composer
    pub fn escape(&mut self) -> MegaStep {
        if self.confirm.is_some() {
            if !self.waiting {
                self.confirm = None;
            }
            MegaStep::None
        } else {
            MegaStep::Cancel
        }
    }

    /// A press at `p`; `None` outside the window that is up.
    // EVIDENCE: cshell+0x2a8d60 (0x272 hides the confirmation; 0x267 runs the base window's cancel)
    pub fn press(&mut self, p: [f32; 2]) -> Option<MegaStep> {
        let l = self.layout;
        if self.confirm.is_some() {
            if !inside(l.c_window_at, l.c_window_size, p) {
                return None;
            }
            // the 0x78-wide hit areas overlap by 8 pixels: the later button (Back over OK, Cancel over Back) wins
            // DC-DEFAULT: CLI-1874 (proposed) the later-built button takes a press in the overlap
            if inside(l.c_abs(l.c_cancel_at), BUTTON_SIZE, p) {
                return Some(MegaStep::Cancel);
            }
            if inside(l.c_abs(l.c_back_at), BUTTON_SIZE, p) {
                return Some(self.escape());
            }
            if inside(l.c_abs(l.c_ok_at), BUTTON_SIZE, p) {
                return Some(self.ok());
            }
            return Some(MegaStep::None);
        }
        if !inside(l.window_at, l.window_size, p) {
            return None;
        }
        if inside(l.abs(l.ok_at), BUTTON_SIZE, p) {
            return Some(self.ok());
        }
        if inside(l.abs(l.cancel_at), BUTTON_SIZE, p) {
            return Some(MegaStep::Cancel);
        }
        // EVIDENCE: cshell+0x2a9150 (a press the grid takes: the slot's colour index, a positive one picked)
        if let Some(k) = (0..self.slots()).find(|&k| inside(self.slot_at(k), SLOT_SIZE, p)) {
            self.pick_slot(k);
        }
        Some(MegaStep::None)
    }

    /// The preview line: the own name, " : " and the message.
    // EVIDENCE: cshell+0x2a85b0 (the own name, the literal VA 0x374cd9c4 " : ", the message)
    pub fn preview(&self) -> String {
        format!("{} : {}", self.name, self.text)
    }

    /// The message from character `from` on, cut into stretches of one colour.
    fn coloured(&self, from: usize) -> Vec<(String, u32)> {
        let mut out: Vec<(String, u32)> = Vec::new();
        for (i, ch) in self.text.chars().enumerate().skip(from) {
            let c = self.argb_of(self.colour_of(i));
            match out.last_mut() {
                Some((s, k)) if *k == c => s.push(ch),
                _ => out.push((ch.to_string(), c)),
            }
        }
        out
    }

    /// Stretches of text one after the other from `at`; the x after the last.
    fn draw_line(
        cat: &mut Catalog,
        parts: &[(String, u32)],
        at: [i32; 2],
        out: &mut Vec<DrawItem>,
    ) -> i32 {
        let mut x = at[0];
        for (s, c) in parts {
            out.push(place_text(s, [x, at[1]], [0, 0], TL, FONT, FONT_SIZE, *c));
            x += measure(cat, s);
        }
        x
    }

    /// The preview line's stretches: the own name and " : " in white, then the message in its colours.
    // DC-DEFAULT: CLI-1874 (proposed) the name part is white (the 2009 confirmation gives it the own name colour of
    // shell +0x434, cshell+0x2a6560; not ported)
    fn preview_parts(&self) -> Vec<(String, u32)> {
        let mut parts = vec![(format!("{} : ", self.name), WHITE)];
        parts.extend(self.coloured(0));
        parts
    }

    fn runs(
        cat: &mut Catalog,
        text: &str,
        at: [i32; 2],
        width: i32,
        colour: u32,
        out: &mut Vec<DrawItem>,
    ) {
        // DC-DEFAULT: CLI-1874 (proposed) the wrapped texts break at the width over 7 characters a line, centred on
        // their point
        for (k, line) in wrap_runs(text, (width / 7).max(1) as usize, colour)
            .iter()
            .enumerate()
        {
            let w: i32 = line.iter().map(|(s, _)| measure(cat, s)).sum();
            let mut x = at[0] - w / 2;
            let y = at[1] + k as i32 * (FONT_SIZE + 2);
            for (s, c) in line {
                out.push(place_text(s, [x, y], [0, 0], TL, FONT, FONT_SIZE, *c));
                x += measure(cat, s);
            }
        }
    }

    pub fn draw(
        &self,
        cat: &mut Catalog,
        art: &MegaArt,
        buttons: &NameArt,
        secs: f32,
        out: &mut Vec<DrawItem>,
    ) {
        let l = self.layout;
        let text = |s: &str, at: [i32; 2], align: u32, colour: u32| {
            let a = Align {
                first: align,
                second: 0,
            };
            place_text(s, at, [0, 0], a, FONT, FONT_SIZE, colour)
        };
        if let Some(w) = self.confirm {
            match art.confirm.as_deref() {
                Some(t) => out.push(image_fit(cat, t, l.c_window_at, l.c_window_size)),
                None => out.push(fill(l.c_window_at, l.c_window_size, [24, 24, 24, 240])),
            }
            out.push(text(
                &cat.text(TITLE_TEXT),
                l.c_abs(l.c_title_at),
                ALIGN_LEFT,
                WHITE,
            ));
            let q = cat.text(CONFIRM_TEXT);
            Self::runs(
                cat,
                &q,
                l.c_abs(l.c_notice_at),
                l.c_string_width,
                WHITE,
                out,
            );
            // EVIDENCE: cshell+0x2a85b0 (the confirmation's line takes the message's colour runs, cshell+0x2a6560)
            let parts = self.preview_parts();
            Self::draw_line(cat, &parts, l.c_abs(l.c_ad_at), out);
            let wait = printf(
                &cat.text(WAIT_TEXT),
                &[Arg::D(w.players as i32), Arg::D(w.minutes)],
            );
            Self::runs(
                cat,
                &wait,
                l.c_abs(l.c_wait_at),
                l.c_string_width,
                INFO_COLOUR,
                out,
            );
            let uses = printf(&cat.text(USES_TEXT), &[Arg::D(self.uses as i32)]);
            Self::runs(
                cat,
                &uses,
                l.c_abs(l.c_remain_at),
                l.c_string_width,
                INFO_COLOUR,
                out,
            );
            let on = !self.waiting;
            draw_button(cat, buttons, l.c_abs(l.c_ok_at), OK_TEXT, on, out);
            draw_button(cat, buttons, l.c_abs(l.c_back_at), BACK_TEXT, on, out);
            draw_button(cat, buttons, l.c_abs(l.c_cancel_at), CANCEL_TEXT, true, out);
            return;
        }
        match art.window.as_deref() {
            Some(t) => out.push(image_fit(cat, t, l.window_at, l.window_size)),
            None => out.push(fill(l.window_at, l.window_size, [24, 24, 24, 240])),
        }
        out.push(text(
            &cat.text(TITLE_TEXT),
            l.abs(l.title_at),
            ALIGN_LEFT,
            WHITE,
        ));
        let notice = cat.text(NOTICE_TEXT);
        Self::runs(cat, &notice, l.abs(l.notice_at), l.string_width, WHITE, out);
        // the field: the tail of the message that fits its width in the characters' colours, the caret after it
        // EVIDENCE: cshell+0x2a7720 (the field at InputEditPos, InputEditWidth wide, caret on through cshell+0x2f79c0
        // with the period 0.333; the caret colour stays the edit control's default, cshell+0x2f74c0), cshell+0x2a8bd0
        // (the field shows the colour runs too)
        // DC-DEFAULT: CLI-1874 (proposed) the caret sits after the last character (no caret movement in our field)
        let width = (l.edit_width - CARET_WIDTH) as f32;
        let shown = {
            let mut m = |t: &str| measure(cat, t) as f32;
            fitting_tail(&self.text, width, &mut m).chars().count()
        };
        let skip = self.text.chars().count() - shown;
        let parts = self.coloured(skip);
        let edit = l.abs(l.edit_at);
        let end = Self::draw_line(cat, &parts, edit, out);
        if caret_on(secs, CARET_PERIOD) {
            out.push(DrawItem::Fill {
                rect: [
                    end as f32,
                    edit[1] as f32,
                    CARET_WIDTH as f32,
                    FONT_SIZE as f32,
                ],
                colour: argb(CHAT_CARET_COLOUR),
            });
        }
        // the colour grid: the swatch pictures (a plain square of the colour without one), the mark on the pick
        for k in 0..self.slots() {
            let at = self.slot_at(k);
            let n = self.slot_colour(k).unwrap_or(0) as usize;
            match self.swatches.get(n.wrapping_sub(1)) {
                Some(s) if !s.tex.is_empty() => out.push(image(&s.tex, at, SLOT_SIZE)),
                Some(s) => out.push(fill(at, SLOT_SIZE, argb(s.argb))),
                None => {}
            }
            if self.picked_slot == Some(k) {
                if let Some(t) = art.slot_on.as_deref() {
                    out.push(image(t, at, SLOT_SIZE));
                }
            }
        }
        // EVIDENCE: cshell+0x2a8870 (the counter form VA 0x374d9820 "%d/%d" with the length and 0x41)
        let count = format!("{}/{}", self.text.chars().count(), MEGAPHONE_MAX);
        out.push(text(&count, l.abs(l.count_at), ALIGN_LEFT, COUNT_COLOUR));
        match self.message_now(Instant::now()) {
            Some(id) => {
                let m = cat.text(id);
                Self::runs(
                    cat,
                    &m,
                    l.abs(l.error_at),
                    l.string_width,
                    ERROR_COLOUR,
                    out,
                );
            }
            None => out.push(text(
                &cat.text(PREVIEW_TEXT),
                l.abs(l.preview_title_at),
                ALIGN_CENTRE,
                PREVIEW_COLOUR,
            )),
        }
        // the preview line centred in its rectangle, the message in its colours
        // EVIDENCE: cshell+0x2a7720 (PreViewPos / PreViewSize, white), cshell+0x2a8a40 (the runs applied on each change)
        let pv = l.abs(l.preview_at);
        let parts = self.preview_parts();
        let w: i32 = parts.iter().map(|(s, _)| measure(cat, s)).sum();
        let x = pv[0] + (l.preview_size[0] - w) / 2;
        Self::draw_line(cat, &parts, [x, pv[1]], out);
        draw_button(cat, buttons, l.abs(l.ok_at), OK_TEXT, !self.waiting, out);
        draw_button(cat, buttons, l.abs(l.cancel_at), CANCEL_TEXT, true, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three swatches of our own: red, green, blue (no pictures).
    fn swatches() -> Vec<Swatch> {
        [0xffff_0000, 0xff00_ff00, 0xff00_00ff]
            .iter()
            .map(|&argb| Swatch {
                argb,
                tex: String::new(),
            })
            .collect()
    }

    fn boxed() -> MegaBox {
        let c = Catalog::fallback(None, 1024, 768);
        MegaBox::open(&c, 11, 3, "Alice", swatches())
    }

    fn mid(at: [i32; 2]) -> [f32; 2] {
        [(at[0] + 5) as f32, (at[1] + 5) as f32]
    }

    #[test]
    fn a_space_is_typed_and_a_pick_colours_what_follows() {
        let mut b = boxed();
        for c in "hi all".chars() {
            b.type_char(c);
        }
        assert_eq!(b.text, "hi all");
        assert_eq!(b.colour_runs(), vec![(NO_COLOUR, 6)]);
        // the grid shows the table from its last entry: slot 0 is index 3 (blue), slot 2 index 1 (red)
        assert_eq!(b.slots(), 3);
        assert_eq!(
            (0..4).map(|k| b.slot_colour(k)).collect::<Vec<_>>(),
            vec![Some(3), Some(2), Some(1), None]
        );
        assert_eq!(b.press(mid(b.slot_at(2))), Some(MegaStep::None));
        assert_eq!((b.pen, b.picked_slot), (1, Some(2)));
        for c in " go".chars() {
            b.type_char(c);
        }
        assert_eq!(b.text, "hi all go");
        assert_eq!(b.colour_runs(), vec![(NO_COLOUR, 6), (1, 3)]);
        assert_eq!(b.argb_of(1), 0xffff_0000);
        assert_eq!(b.argb_of(NO_COLOUR), WHITE);
        b.backspace();
        assert_eq!(b.colour_runs(), vec![(NO_COLOUR, 6), (1, 2)]);
        // a text set without colours counts as uncoloured
        b.text = "abc".into();
        b.colours.clear();
        assert_eq!(b.colour_runs(), vec![(NO_COLOUR, 3)]);
    }

    #[test]
    fn the_field_draws_a_blinking_caret_after_the_text_and_the_coloured_preview() {
        let mut c = Catalog::fallback(None, 1024, 768);
        let mut b = boxed();
        b.type_char('a');
        b.pick_slot(1);
        b.type_char('b');
        let caret = |out: &Vec<DrawItem>| {
            out.iter().any(|d| {
                matches!(d, DrawItem::Fill { rect, colour }
                    if rect[2] == CARET_WIDTH as f32 && *colour == argb(CHAT_CARET_COLOUR))
            })
        };
        let mut out = Vec::new();
        b.draw(
            &mut c,
            &MegaArt::default(),
            &NameArt::default(),
            0.1,
            &mut out,
        );
        assert!(caret(&out), "the caret is on in the first half period");
        let green = |out: &Vec<DrawItem>| {
            out.iter()
                .filter(|d| {
                    matches!(d, DrawItem::Label { text, style, .. }
                        if text == "b" && style.colour == argb(0xff00_ff00))
                })
                .count()
        };
        // the field and the preview both draw "b" in the picked green
        assert_eq!(green(&out), 2);
        // the three swatches
        let squares = out
            .iter()
            .filter(|d| matches!(d, DrawItem::Fill { rect, .. } if rect[2] == SLOT_SIZE[0] as f32))
            .count();
        assert_eq!(squares, 3);
        let mut out = Vec::new();
        b.draw(
            &mut c,
            &MegaArt::default(),
            &NameArt::default(),
            0.5,
            &mut out,
        );
        assert!(!caret(&out), "off in the second half");
    }

    #[test]
    fn an_empty_field_says_so_and_a_message_asks_for_the_queue() {
        let mut b = boxed();
        assert_eq!(b.ok(), MegaStep::None);
        assert_eq!(b.message_now(Instant::now()), Some(EMPTY_TEXT));
        assert_eq!(b.message_now(Instant::now() + Duration::from_secs(6)), None);
        for c in "hi all".chars() {
            b.type_char(c);
        }
        assert_eq!(b.ok(), MegaStep::Queue);
        assert!(b.waiting);
        assert_eq!(b.ok(), MegaStep::None);
    }

    #[test]
    fn the_field_holds_65_characters() {
        let mut b = boxed();
        for _ in 0..70 {
            b.type_char('x');
        }
        assert_eq!(b.text.chars().count(), MEGAPHONE_MAX);
        assert_eq!(b.message_now(Instant::now()), Some(TOO_LONG_TEXT));
        b.type_char('\n');
        b.backspace();
        assert_eq!(b.text.chars().count(), MEGAPHONE_MAX - 1);
    }

    #[test]
    fn the_queue_answer_opens_the_confirmation_whose_ok_sends_and_back_returns() {
        let mut b = boxed();
        b.text = "sale".into();
        b.ok();
        let r = QueueReply {
            ok: true,
            reason: 0,
            secs: 30,
            on_air: 4,
            last: 9,
            waiting: 5,
        };
        assert!(b.queued(&r));
        // 5 orders x 30 s = 150 s: 2 whole minutes + 1
        assert_eq!(
            b.confirm,
            Some(Wait {
                players: 5,
                minutes: 3
            })
        );
        let l = b.layout;
        let at = |q: [i32; 2]| {
            [
                (l.c_window_at[0] + q[0] + 5) as f32,
                (l.c_window_at[1] + q[1] + 5) as f32,
            ]
        };
        assert_eq!(b.press(at(l.c_back_at)), Some(MegaStep::None));
        assert_eq!(b.confirm, None);
        b.ok();
        b.queued(&r);
        assert_eq!(b.press(at(l.c_ok_at)), Some(MegaStep::Send("sale".into())));
        assert_eq!(b.press(at(l.c_cancel_at)), Some(MegaStep::Cancel));
        // a refused queue question keeps the composer
        let mut b = boxed();
        b.text = "x".into();
        b.ok();
        assert!(!b.queued(&QueueReply {
            ok: false,
            reason: 0x8f,
            ..Default::default()
        }));
        assert_eq!((b.confirm, b.waiting), (None, false));
    }

    #[test]
    fn an_empty_queue_waits_one_minute() {
        let w = wait_of(&QueueReply {
            ok: true,
            secs: 30,
            on_air: 6,
            last: 6,
            ..Default::default()
        });
        assert_eq!(
            w,
            Wait {
                players: 0,
                minutes: 1
            }
        );
    }

    #[test]
    fn both_windows_draw() {
        let mut c = Catalog::fallback(None, 1024, 768);
        let mut b = boxed();
        b.text = "hello".into();
        let mut out = Vec::new();
        b.draw(
            &mut c,
            &MegaArt::default(),
            &NameArt::default(),
            0.0,
            &mut out,
        );
        let has = |out: &Vec<DrawItem>, s: &str| {
            out.iter()
                .any(|d| matches!(d, DrawItem::Label { text, .. } if text == s))
        };
        assert!(has(&out, "5/65"));
        assert!(has(&out, "Alice : "));
        assert!(has(&out, "hello"));
        b.confirm = Some(Wait {
            players: 1,
            minutes: 2,
        });
        let mut out = Vec::new();
        b.draw(
            &mut c,
            &MegaArt::default(),
            &NameArt::default(),
            0.0,
            &mut out,
        );
        assert!(has(&out, "Alice : "));
        assert!(!has(&out, "5/65"));
    }
}
