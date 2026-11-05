//! use-items-emblem: the face window of the function items of `SubType` 6 (base character change: another character
//! of the own gender) and 7 (gender change: a character of the other gender). A code-built popup by the layout section
//! `FaceBox` with the kind's strings (`FaceChange` / `GenderChange`): the title, a line naming the choice, the own
//! character's portrait, the picked one, a row of `columns` portraits of the ROM creation table (every row, the hidden
//! ones too) with scroll buttons, OK / Cancel. A pick enables OK; OK shows the confirmation (the picked portrait, a
//! question, OK / Cancel) whose OK sends 0x330 with the face value 6000 + the picked row's face. The ok answer makes it
//! the own face (the shop pane's figure wears it at once) and removes the item.
// SPEC: PROTOCOL_2009.md §4.3 (0x330 +0x34 face; 0x331 window table rows 6, 7)
// EVIDENCE: cshell+0x246960 (the class, vtable VA 0x374f2ca4), cshell+0x247370 (build), cshell+0x246a30 (commands),
// cshell+0x246450 (a press), cshell+0x246350 (a slot press), cshell+0x246550 (the scroll), cshell+0x248360 (the list),
// cshell+0x284670 (the creation table's records); DISASM: cshell+0x24a010 (open), cshell+0x242ad0 (send),
// cshell+0x24a1d0 (the 0x331 reader)

use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::pages::Align;
use std::path::Path;

use super::catalog::{int, key, pair, Catalog, KeyPin};
use super::colorbox::WHITE;
use super::popup::wrap_runs;
use super::sheet::{image, image_fit, place_text, ALIGN_LEFT};
use super::usebox::{
    draw_button, fill, inside, measure, NameArt, BUTTON_SIZE, CANCEL_TEXT, FONT, FONT_SIZE,
    OK_TEXT, TITLE_COLOUR,
};
use crate::hud::items::DrawItem;

/// The kinds this window serves.
pub const SUB_FACE: i32 = 6;
pub const SUB_GENDER: i32 = 7;

/// The face value base of the use request.
// DISASM: cshell+0x242ae7 (the picked face + 0x1770)
pub const FACE_BASE: i32 = 0x1770;

/// The commands: OK 0x268, Cancel 0x267, the confirmation's OK 0x271 and Cancel 0x272, the scroll buttons 0x25c (next)
/// and 0x25d (previous).
// EVIDENCE: cshell+0x247370 (buttons 0x268 captioned 0x42, 0x267 captioned 0x43, 0x25d / 0x25c, the confirmation's
// 0x271 / 0x272 captioned 0x42 / 0x43), cshell+0x246a30 (0x25c / 0x25d move the first slot by 4; 0x268 with a pick shows
// the confirmation; 0x271 hides it and sends; 0x272 hides it)
pub const CMD_OK: i32 = 0x268;
pub const CMD_CANCEL: i32 = 0x267;
pub const CMD_SEND: i32 = 0x271;
pub const CMD_BACK: i32 = 0x272;

/// The slots a scroll press moves by.
// EVIDENCE: cshell+0x246a30 (0x25c: first + 4, 0x25d: first - 4)
pub const SCROLL_STEP: usize = 4;

/// The confirmation's frame (the shared popup window picture) and its buttons' places.
// EVIDENCE: cshell+0x247370 (the shared popup frame 0x1dc x 0x101 centred; OK at (0x7e, 0xcf), Cancel at (0xf6, 0xcf))
pub const CONFIRM_SIZE: [i32; 2] = [0x1dc, 0x101];
pub const CONFIRM_OK_AT: [i32; 2] = [0x7e, 0xcf];
pub const CONFIRM_CANCEL_AT: [i32; 2] = [0xf6, 0xcf];

/// The layout section and its keys.
// EVIDENCE: cshell+0x247370 (section literal "FaceBox" and the keys below; the string sections "FaceChange" (kind 6)
// and "GenderChange" (kind 7))
pub mod keys {
    use super::{key, KeyPin};
    pub use crate::menu::colorbox::keys::{
        CHOICE, CHOICE_POS, C_NOTICE, C_NOTICE_POS, C_PREVIEW_POS, C_TITLE, LIST_POS, TITLE,
    };
    pub use crate::menu::usebox::keys::{CANCEL_POS, OK_POS, TITLE_POS, WINDOW_RECT};
    pub const SECTION: KeyPin = key("985d5107fe14095f", 7);
    pub const PREVIEW_POS: KeyPin = key("a984a3db82c37e28", 10);
    pub const CURRENT_POS: KeyPin = key("3be4cbee3d45fa77", 10);
    pub const IMAGE_SIZE: KeyPin = key("6a2395ff17ae6062", 13);
    pub const SLOT_ON: KeyPin = key("ecd70cc3c3458ea9", 17);
    pub const SLOT_OFF: KeyPin = key("c9c44e0258491c76", 18);
    pub const ROWS: KeyPin = key("4135621cbbd16484", 18);
    pub const COLUMNS: KeyPin = key("d124987f88c3d907", 18);
    pub const ROW_GAP: KeyPin = key("02d59ffb2ff3357e", 23);
    pub const COLUMN_GAP: KeyPin = key("c65ad75ea9002c39", 23);
    pub const PREV_POS: KeyPin = key("7e50cf709ca2cc31", 13);
    pub const NEXT_POS: KeyPin = key("9210c9d9da470b1b", 14);
    pub const PAGE_BUTTON_SIZE: KeyPin = key("cf1bf8773633d94c", 14);
    pub const FACE_STRINGS: KeyPin = key("eca22d2aeebaeced", 10);
    pub const GENDER_STRINGS: KeyPin = key("ce0ac6b6e0fe8024", 12);
}

/// The window's places (offsets inside the window; the confirmation's inside its frame).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceLayout {
    pub window_size: [i32; 2],
    pub title_at: [i32; 2],
    pub choice_at: [i32; 2],
    pub current_at: [i32; 2],
    pub preview_at: [i32; 2],
    pub image_size: [i32; 2],
    pub list_at: [i32; 2],
    pub slot_on: String,
    pub slot_off: String,
    pub rows: i32,
    pub columns: i32,
    pub row_gap: i32,
    pub column_gap: i32,
    pub prev_at: [i32; 2],
    pub next_at: [i32; 2],
    pub page_button_size: [i32; 2],
    pub ok_at: [i32; 2],
    pub cancel_at: [i32; 2],
    pub c_preview_at: [i32; 2],
    pub c_notice_at: [i32; 2],
}

/// DC-DEFAULT: CLI-1885 (proposed) our fallback when the ROM's layout section is missing: a 600 x 400 window on a
/// simple grid of our own round numbers, no slot pictures (the real places are read from the ROM first).
impl Default for FaceLayout {
    fn default() -> Self {
        FaceLayout {
            window_size: [600, 400],
            title_at: [10, 10],
            choice_at: [40, 50],
            current_at: [40, 80],
            preview_at: [140, 80],
            image_size: [70, 70],
            list_at: [200, 200],
            slot_on: String::new(),
            slot_off: String::new(),
            rows: 1,
            columns: 4,
            row_gap: 10,
            column_gap: 10,
            prev_at: [180, 200],
            next_at: [520, 200],
            page_button_size: [20, 70],
            ok_at: [160, 340],
            cancel_at: [320, 340],
            c_preview_at: [200, 90],
            c_notice_at: [30, 50],
        }
    }
}

impl FaceLayout {
    pub fn read(cat: &Catalog) -> FaceLayout {
        let d = FaceLayout::default();
        let v = |k: &KeyPin| cat.section_value(&keys::SECTION, k);
        let p = |k: &KeyPin, or: [i32; 2]| pair(v(k)).unwrap_or(or);
        let n = |k: &KeyPin, or: i32| int(v(k)).unwrap_or(or);
        let t = |k: &KeyPin, or: String| v(k).map_or(or, |s| s.trim().to_string());
        FaceLayout {
            window_size: p(&keys::WINDOW_RECT, d.window_size),
            title_at: p(&keys::TITLE_POS, d.title_at),
            choice_at: p(&keys::CHOICE_POS, d.choice_at),
            current_at: p(&keys::CURRENT_POS, d.current_at),
            preview_at: p(&keys::PREVIEW_POS, d.preview_at),
            image_size: p(&keys::IMAGE_SIZE, d.image_size),
            list_at: p(&keys::LIST_POS, d.list_at),
            slot_on: t(&keys::SLOT_ON, d.slot_on),
            slot_off: t(&keys::SLOT_OFF, d.slot_off),
            rows: n(&keys::ROWS, d.rows).max(1),
            columns: n(&keys::COLUMNS, d.columns).max(1),
            row_gap: n(&keys::ROW_GAP, d.row_gap),
            column_gap: n(&keys::COLUMN_GAP, d.column_gap),
            prev_at: p(&keys::PREV_POS, d.prev_at),
            next_at: p(&keys::NEXT_POS, d.next_at),
            page_button_size: p(&keys::PAGE_BUTTON_SIZE, d.page_button_size),
            ok_at: p(&keys::OK_POS, d.ok_at),
            cancel_at: p(&keys::CANCEL_POS, d.cancel_at),
            c_preview_at: p(&keys::C_PREVIEW_POS, d.c_preview_at),
            c_notice_at: p(&keys::C_NOTICE_POS, d.c_notice_at),
        }
    }

    /// The window's corner: centred on the 1024 x 768 screen.
    // EVIDENCE: cshell+0x247370 (the window at ((0x400 - w) / 2, (0x300 - h) / 2); the confirmation likewise)
    pub fn window_at(&self) -> [i32; 2] {
        centred(self.window_size)
    }
}

fn centred(size: [i32; 2]) -> [i32; 2] {
    [(0x400 - size[0]) / 2, (0x300 - size[1]) / 2]
}

/// The string ids of one kind: title, the choice line, the confirmation's title and question (0 = none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaceTexts {
    pub title: i32,
    pub choice: i32,
    pub confirm_title: i32,
    pub confirm_notice: i32,
}

impl FaceTexts {
    /// DC-DEFAULT: CLI-1885 (proposed) our fallback when the ROM's string section is missing: no text (id 0); the real
    /// ids are read from the ROM first.
    pub fn fallback() -> FaceTexts {
        FaceTexts {
            title: 0,
            choice: 0,
            confirm_title: 0,
            confirm_notice: 0,
        }
    }

    pub fn read(cat: &Catalog, kind: i32) -> FaceTexts {
        let d = FaceTexts::fallback();
        let sec = if kind == SUB_GENDER {
            keys::GENDER_STRINGS
        } else {
            keys::FACE_STRINGS
        };
        let n = |k: &KeyPin, or: i32| int(cat.section_value(&sec, k)).unwrap_or(or);
        FaceTexts {
            title: n(&keys::TITLE, d.title),
            choice: n(&keys::CHOICE, d.choice),
            confirm_title: n(&keys::C_TITLE, d.confirm_title),
            confirm_notice: n(&keys::C_NOTICE, d.confirm_notice),
        }
    }
}

/// One character of the creation table: its face (the record's `FacePieceId`), its gender and its portrait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceRow {
    pub face: i32,
    pub man: bool,
    pub portrait: String,
}

/// Every row of the creation table in row order (the hidden rows too).
// OBSERVED: ROM table CREATEMODEL.TXT [Create_Model<n>] FacePieceId, IsMan, SmallFaceImg
// EVIDENCE: cshell+0x284670 (a record: FacePieceId +8, SmallFaceImg +0x3c, IsMan != 0 +0xd8, ViewType +0xdc),
// cshell+0x248360 (the face window lists records whatever their ViewType)
pub fn rows_from_doc(doc: &dc_bute::ButeDoc) -> Vec<FaceRow> {
    let clean = |v: &str| {
        v.split("//")
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .trim()
            .to_string()
    };
    let mut rows: Vec<(i32, FaceRow)> = doc
        .iter()
        .filter_map(|(name, sec)| {
            let n = super::nickname::create_row(name)?;
            let num = |k: &str| sec.get(k).and_then(|v| clean(v).parse::<i32>().ok());
            Some((
                n,
                FaceRow {
                    face: num("FacePieceId").unwrap_or(0),
                    man: num("IsMan").unwrap_or(0) != 0,
                    portrait: sec
                        .get("SmallFaceImg")
                        .map(|v| clean(v))
                        .unwrap_or_default(),
                },
            ))
        })
        .collect();
    rows.sort_by_key(|(n, _)| *n);
    rows.into_iter().map(|(_, r)| r).collect()
}

pub fn rows_from_rom(rom: Option<&Path>) -> Vec<FaceRow> {
    rom.and_then(super::nickname::create_table)
        .map_or_else(Vec::new, |d| rows_from_doc(&d))
}

/// The list of kind `kind` for the own face `own` (the face row number): kind 6 the rows of the own gender but the own
/// row, kind 7 the rows of the other gender; the own gender is the own row's (a man without one).
// DISASM: cshell+0x24a010 (the own record: the one whose face is the character's; its gender, 1 without one; its
// portrait at CurrentPos), cshell+0x248360 (kind 6: same gender and another face; kind 7: another gender)
pub fn choices(rows: &[FaceRow], kind: i32, own: i32) -> Vec<FaceRow> {
    let man = rows.iter().find(|r| r.face == own).is_none_or(|r| r.man);
    rows.iter()
        .filter(|r| {
            if kind == SUB_GENDER {
                r.man != man
            } else {
                r.man == man && r.face != own
            }
        })
        .cloned()
        .collect()
}

/// Texture literals: the window frame and the scroll buttons (left / right, enabled / disabled).
// DISASM: cshell+0x247490 (frame VA 0x374f3018), cshell+0x2478ee / 0x247918 (left A / D VA 0x374f2fa0 / 0x374f2f40),
// cshell+0x2479e3 / 0x247a0f (right A / D VA 0x374f2ed0 / 0x374f2e70)
pub const WINDOW_TEX: Pin = pin("face window", 0x3f3018, "8aa90fd68f6a6223", 55);
pub const LEFT_TEX: Pin = pin("face window left", 0x3f2fa0, "6ea0ec691199ca98", 45);
pub const LEFT_OFF_TEX: Pin = pin("face window left off", 0x3f2f40, "66a496ff0af1f9f6", 45);
pub const RIGHT_TEX: Pin = pin("face window right", 0x3f2ed0, "f4bce282df556209", 46);
pub const RIGHT_OFF_TEX: Pin = pin("face window right off", 0x3f2e70, "1f675d309c368ec0", 46);
const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
    Pin {
        what,
        rva,
        sha16,
        len,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FaceArt {
    pub window: Option<String>,
    pub confirm: Option<String>,
    /// Left and right, each (enabled, disabled).
    pub scroll: [(Option<String>, Option<String>); 2],
}

impl FaceArt {
    pub fn from_rom(rom: Option<&Path>) -> FaceArt {
        let Some(pe) = rom.and_then(|r| {
            dc_rom::PeFile::from_file(&r.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1])).ok()
        }) else {
            return FaceArt::default();
        };
        let get = |p: &Pin| read_pinned(&pe, p).ok();
        FaceArt {
            window: get(&WINDOW_TEX),
            confirm: get(&super::countrybox::WINDOW_TEX),
            scroll: [
                (get(&LEFT_TEX), get(&LEFT_OFF_TEX)),
                (get(&RIGHT_TEX), get(&RIGHT_OFF_TEX)),
            ],
        }
    }
}

/// What a press or a key did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceStep {
    None,
    /// Use the item with face value `face` (0x330 +0x34).
    Use {
        face: u32,
    },
    /// Cancel: the window closes with the cancelled box.
    Cancel,
}

/// The open face window.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceBox {
    pub kind: i32,
    pub uid: u32,
    /// The own face row and its portrait (empty without one).
    pub own: i32,
    pub own_portrait: String,
    pub choices: Vec<FaceRow>,
    /// The first choice shown.
    pub first: usize,
    /// The picked choice (an index into `choices`).
    pub picked: Option<usize>,
    pub confirm: bool,
    pub waiting: bool,
    pub layout: FaceLayout,
    pub texts: FaceTexts,
}

const TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};

impl FaceBox {
    /// The window of kind `kind` for item `uid`; `own` the own face row.
    // DISASM: cshell+0x24a010 (open: the own portrait, the preview hidden, OK off, nothing picked, the list from its
    // start, the previous button off, the next one on with more than four choices)
    pub fn open(cat: &Catalog, kind: i32, uid: u32, own: i32, rows: &[FaceRow]) -> FaceBox {
        FaceBox {
            kind,
            uid,
            own,
            own_portrait: rows
                .iter()
                .find(|r| r.face == own)
                .map(|r| r.portrait.clone())
                .unwrap_or_default(),
            choices: choices(rows, kind, own),
            first: 0,
            picked: None,
            confirm: false,
            waiting: false,
            layout: FaceLayout::read(cat),
            texts: FaceTexts::read(cat, kind),
        }
    }

    fn abs(&self, at: [i32; 2]) -> [i32; 2] {
        let w = self.layout.window_at();
        [w[0] + at[0], w[1] + at[1]]
    }

    fn c_abs(&self, at: [i32; 2]) -> [i32; 2] {
        let w = centred(CONFIRM_SIZE);
        [w[0] + at[0], w[1] + at[1]]
    }

    /// Slots shown at once.
    pub fn per_page(&self) -> usize {
        (self.layout.rows * self.layout.columns).max(1) as usize
    }

    /// Slot `k`'s corner.
    // EVIDENCE: cshell+0x247370 (the list at ListPos: FaceListSlotVerNum x FaceListSlotHorNum slots of FaceImageSize
    // with the intervals)
    pub fn slot_at(&self, k: usize) -> [i32; 2] {
        let l = &self.layout;
        let (row, col) = ((k as i32) / l.columns, (k as i32) % l.columns);
        self.abs([
            l.list_at[0] + col * (l.image_size[0] + l.column_gap),
            l.list_at[1] + row * (l.image_size[1] + l.row_gap),
        ])
    }

    fn shown(&self) -> std::ops::Range<usize> {
        let n = self.choices.len();
        self.first.min(n)..(self.first + self.per_page()).min(n)
    }

    /// The scroll buttons' state: (previous, next) enabled.
    // EVIDENCE: cshell+0x246550 (previous on past the start; next on while four more follow)
    pub fn scroll_on(&self) -> (bool, bool) {
        (
            self.first > 0,
            self.first + SCROLL_STEP < self.choices.len(),
        )
    }

    /// The picked face's value for the use request (6000 + its face row).
    pub fn picked_face(&self) -> Option<i32> {
        self.picked
            .and_then(|i| self.choices.get(i))
            .map(|r| r.face)
    }

    /// Picks choice `i`.
    // EVIDENCE: cshell+0x246450 (a slot press: the choice kept, its portrait at PreviewPos, OK on), cshell+0x246350 (the
    // on picture moves to the pressed slot)
    pub fn pick(&mut self, i: usize) {
        if i < self.choices.len() && !self.waiting {
            self.picked = Some(i);
        }
    }

    /// OK (or Enter) of the window that is up.
    // EVIDENCE: cshell+0x246a30 (0x268 with a pick: the confirmation shown with the picked portrait; 0x271: the send)
    // DC-DEFAULT: CLI-1885 (proposed) Enter runs OK of the window that is up, Escape the confirmation's Cancel (back to
    // the window), then the window's Cancel
    pub fn ok(&mut self) -> FaceStep {
        let Some(face) = self.picked_face() else {
            return FaceStep::None;
        };
        if self.waiting {
            return FaceStep::None;
        }
        if !self.confirm {
            self.confirm = true;
            return FaceStep::None;
        }
        self.confirm = false;
        self.waiting = true;
        // DISASM: cshell+0x242ad0 (the face word: the picked face + 0x1770)
        FaceStep::Use {
            face: (face + FACE_BASE) as u32,
        }
    }

    pub fn escape(&mut self) -> FaceStep {
        if self.confirm {
            if !self.waiting {
                self.confirm = false;
            }
            return FaceStep::None;
        }
        FaceStep::Cancel
    }

    /// A refused use: the window stays.
    // DISASM: cshell+0x24a351 (ok = 0: the reason box only)
    pub fn refused(&mut self) {
        self.waiting = false;
    }

    /// A press at `p`; `None` outside the window.
    pub fn press(&mut self, p: [f32; 2]) -> Option<FaceStep> {
        let l = self.layout.clone();
        if self.confirm {
            if !inside(centred(CONFIRM_SIZE), CONFIRM_SIZE, p) {
                return None;
            }
            if inside(self.c_abs(CONFIRM_OK_AT), BUTTON_SIZE, p) {
                return Some(self.ok());
            }
            if inside(self.c_abs(CONFIRM_CANCEL_AT), BUTTON_SIZE, p) {
                return Some(self.escape());
            }
            return Some(FaceStep::None);
        }
        if !inside(l.window_at(), l.window_size, p) {
            return None;
        }
        if inside(self.abs(l.cancel_at), BUTTON_SIZE, p) {
            return Some(FaceStep::Cancel);
        }
        if inside(self.abs(l.ok_at), BUTTON_SIZE, p) {
            return Some(self.ok());
        }
        let (prev, next) = self.scroll_on();
        if prev && inside(self.abs(l.prev_at), l.page_button_size, p) {
            self.first = self.first.saturating_sub(SCROLL_STEP);
            return Some(FaceStep::None);
        }
        if next && inside(self.abs(l.next_at), l.page_button_size, p) {
            self.first += SCROLL_STEP;
            return Some(FaceStep::None);
        }
        for (k, i) in self.shown().enumerate() {
            if inside(self.slot_at(k), l.image_size, p) {
                self.pick(i);
                return Some(FaceStep::None);
            }
        }
        Some(FaceStep::None)
    }

    pub fn draw(
        &self,
        cat: &mut Catalog,
        art: &FaceArt,
        buttons: &NameArt,
        out: &mut Vec<DrawItem>,
    ) {
        // EVIDENCE: cshell+0x246a30 (0x268: the confirmation shown and the window hidden; 0x271 / 0x272 the reverse)
        if self.confirm {
            self.draw_confirm(cat, art, buttons, out);
            return;
        }
        let l = &self.layout;
        let w = l.window_at();
        match art.window.as_deref() {
            Some(t) => out.push(image_fit(cat, t, w, l.window_size)),
            None => out.push(fill(w, l.window_size, [24, 24, 24, 240])),
        }
        let text = |s: &str, at: [i32; 2], colour: u32| {
            place_text(s, at, [0, 0], TL, FONT, FONT_SIZE, colour)
        };
        // EVIDENCE: cshell+0x247370 (str_Title at TitlePos in 0xffffc000, str_Choice at CurrentRecordPos)
        // DC-DEFAULT: CLI-1885 (proposed) the choice line and the confirmation's question in white
        out.push(text(
            &cat.text(self.texts.title),
            self.abs(l.title_at),
            TITLE_COLOUR,
        ));
        out.push(text(
            &cat.text(self.texts.choice),
            self.abs(l.choice_at),
            WHITE,
        ));
        if !self.own_portrait.is_empty() {
            out.push(image(
                &self.own_portrait,
                self.abs(l.current_at),
                l.image_size,
            ));
        }
        if let Some(r) = self.picked.and_then(|i| self.choices.get(i)) {
            out.push(image(&r.portrait, self.abs(l.preview_at), l.image_size));
        }
        for (k, i) in self.shown().enumerate() {
            let at = self.slot_at(k);
            if !l.slot_off.is_empty() {
                out.push(image(&l.slot_off, at, l.image_size));
            }
            out.push(image(&self.choices[i].portrait, at, l.image_size));
            if self.picked == Some(i) && !l.slot_on.is_empty() {
                out.push(image(&l.slot_on, at, l.image_size));
            }
        }
        let (prev, next) = self.scroll_on();
        for (k, (at, on)) in [(l.prev_at, prev), (l.next_at, next)]
            .into_iter()
            .enumerate()
        {
            let (a, d) = &art.scroll[k];
            let tex = if on { a } else { d };
            match tex.as_deref().or(a.as_deref()) {
                Some(t) => out.push(image_fit(cat, t, self.abs(at), l.page_button_size)),
                None => {
                    let c = if on { 90 } else { 45 };
                    out.push(fill(self.abs(at), l.page_button_size, [c, c, c, 255]));
                }
            }
        }
        let free = !self.waiting;
        draw_button(
            cat,
            buttons,
            self.abs(l.ok_at),
            OK_TEXT,
            free && self.picked.is_some(),
            out,
        );
        draw_button(cat, buttons, self.abs(l.cancel_at), CANCEL_TEXT, free, out);
    }

    // EVIDENCE: cshell+0x247370 (the confirmation: the frame, str_UseMsgBoxTitle at TitlePos in 0xffffc000, the picked
    // portrait at ConFirmBoxPreviewPos in FaceImageSize, str_UseMsgBoxNotice in white at ConFirmBoxNotice, OK / Cancel)
    fn draw_confirm(
        &self,
        cat: &mut Catalog,
        art: &FaceArt,
        buttons: &NameArt,
        out: &mut Vec<DrawItem>,
    ) {
        let l = &self.layout;
        let at = centred(CONFIRM_SIZE);
        match art.confirm.as_deref() {
            Some(t) => out.push(image_fit(cat, t, at, CONFIRM_SIZE)),
            None => out.push(fill(at, CONFIRM_SIZE, [24, 24, 24, 245])),
        }
        out.push(place_text(
            &cat.text(self.texts.confirm_title),
            self.c_abs(l.title_at),
            [0, 0],
            TL,
            FONT,
            FONT_SIZE,
            TITLE_COLOUR,
        ));
        if let Some(r) = self.picked.and_then(|i| self.choices.get(i)) {
            out.push(image(&r.portrait, self.c_abs(l.c_preview_at), l.image_size));
        }
        let notice = cat.text(self.texts.confirm_notice);
        let n_at = self.c_abs(l.c_notice_at);
        // DC-DEFAULT: CLI-1885 (proposed) the question wraps at the frame's width less twice its left offset, over 7
        // pixels a character (as CLI-1876)
        let width = (CONFIRM_SIZE[0] - 2 * l.c_notice_at[0]).max(70);
        for (k, line) in wrap_runs(&notice, (width / 7) as usize, WHITE)
            .iter()
            .enumerate()
        {
            let mut x = n_at[0];
            let y = n_at[1] + k as i32 * (FONT_SIZE + 2);
            for (s, c) in line {
                out.push(place_text(s, [x, y], [0, 0], TL, FONT, FONT_SIZE, *c));
                x += measure(cat, s);
            }
        }
        let free = !self.waiting;
        draw_button(cat, buttons, self.c_abs(CONFIRM_OK_AT), OK_TEXT, free, out);
        draw_button(
            cat,
            buttons,
            self.c_abs(CONFIRM_CANCEL_AT),
            CANCEL_TEXT,
            free,
            out,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic rows: men with faces 0, 1, 2, 3, 9, 11, women with faces 4, 5.
    fn rows() -> Vec<FaceRow> {
        [
            (0, true),
            (1, true),
            (2, true),
            (3, true),
            (4, false),
            (5, false),
            (9, true),
            (11, true),
        ]
        .into_iter()
        .map(|(face, man)| FaceRow {
            face,
            man,
            portrait: format!("p{face}.dtx"),
        })
        .collect()
    }

    fn boxed(kind: i32, own: i32) -> FaceBox {
        let c = Catalog::fallback(None, 1024, 768);
        FaceBox::open(&c, kind, 30, own, &rows())
    }

    fn mid(at: [i32; 2]) -> [f32; 2] {
        [(at[0] + 5) as f32, (at[1] + 5) as f32]
    }

    #[test]
    fn the_lists_follow_the_kind_and_the_own_gender() {
        let faces = |b: &FaceBox| b.choices.iter().map(|r| r.face).collect::<Vec<_>>();
        assert_eq!(faces(&boxed(SUB_FACE, 2)), vec![0, 1, 3, 9, 11]);
        assert_eq!(faces(&boxed(SUB_GENDER, 2)), vec![4, 5]);
        assert_eq!(faces(&boxed(SUB_FACE, 4)), vec![5]);
        assert_eq!(faces(&boxed(SUB_GENDER, 5)), vec![0, 1, 2, 3, 9, 11]);
        // without an own row the character counts as a man
        assert_eq!(faces(&boxed(SUB_GENDER, 77)), vec![4, 5]);
        assert_eq!(boxed(SUB_FACE, 2).own_portrait, "p2.dtx");
    }

    #[test]
    fn a_pick_confirms_then_sends_the_face_value() {
        let mut b = boxed(SUB_FACE, 2);
        assert_eq!(b.layout.window_at(), [212, 184]);
        let ok = mid(b.abs(b.layout.ok_at));
        assert_eq!(b.press(ok), Some(FaceStep::None));
        assert!(!b.confirm, "OK is off until a pick");
        // the scroll: five choices, four shown, next on
        assert_eq!(b.scroll_on(), (false, true));
        b.press(mid(b.abs(b.layout.next_at)));
        assert_eq!((b.first, b.scroll_on()), (4, (true, false)));
        b.press(mid(b.slot_at(0)));
        assert_eq!(b.picked_face(), Some(11));
        assert_eq!(b.press(ok), Some(FaceStep::None));
        assert!(b.confirm);
        // the confirmation's Cancel goes back, its OK sends 6000 + the face
        assert_eq!(
            b.press(mid(b.c_abs(CONFIRM_CANCEL_AT))),
            Some(FaceStep::None)
        );
        assert!(!b.confirm);
        b.ok();
        assert_eq!(
            b.press(mid(b.c_abs(CONFIRM_OK_AT))),
            Some(FaceStep::Use { face: 6011 })
        );
        assert!(b.waiting);
        b.refused();
        assert_eq!(b.escape(), FaceStep::Cancel);
        assert_eq!(b.press([0.0, 0.0]), None);
    }

    #[test]
    fn the_draw_shows_the_own_and_the_picked_portraits() {
        let mut c = Catalog::fallback(None, 1024, 768);
        let mut b = boxed(SUB_GENDER, 1);
        b.pick(1);
        let mut out = Vec::new();
        b.draw(&mut c, &FaceArt::default(), &NameArt::default(), &mut out);
        let texs: Vec<&str> = out
            .iter()
            .filter_map(|d| match d {
                DrawItem::Image { tex, .. } => Some(tex.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texs, vec!["p1.dtx", "p5.dtx", "p4.dtx", "p5.dtx"]);
    }

    #[test]
    fn the_creation_table_keeps_every_row_in_order() {
        let d = dc_bute::ButeDoc::parse(
            "[Create_Model1]\nFacePieceId=7\nIsMan=0\nSmallFaceImg=\"b.dtx\"\nViewType=1\n\
             [Create_Model0]\nFacePieceId=3\nIsMan=1\nSmallFaceImg=a.dtx // note\n[Other]\nFacePieceId=9\n",
        );
        let r = rows_from_doc(&d);
        assert_eq!(
            r,
            vec![
                FaceRow {
                    face: 3,
                    man: true,
                    portrait: "a.dtx".into()
                },
                FaceRow {
                    face: 7,
                    man: false,
                    portrait: "b.dtx".into()
                }
            ]
        );
    }
}
