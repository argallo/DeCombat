//! use-items-sweep: the colour window of the function items of `SubType` 8 (player name colour), 9 (clan name colour)
//! and 11 (custom reticle). A code-built popup (layout section `ColorChangeBox`): the title, a line naming the
//! choice, a palette of the ROM colour table's swatches on pages of `rows x columns` slots (page links `<`, `1..5`,
//! `>`), a preview (kinds 8 / 9: the own or clan name in the picked colour; kind 11: the reticle in use, an arrow and
//! this item's reticle in the picked colour) and OK / Cancel. A pick enables OK; OK sends 0x330 with the colour index
//! (kinds 8 / 9) or first shows the confirmation (kind 11: the reticle on a black square, a question, OK / Cancel).
//! The ok answer marks the item in use with the colour as its function word.
// SPEC: PROTOCOL_2009.md §4.3 (0x330 +0x38 colour, +0x3c used unique id; 0x331 window table row 8, 9, 11)
// EVIDENCE: cshell+0x246ba0 (the class, vtable VA 0x374f2cf4), cshell+0x248660 (build), cshell+0x249a60 (commands),
// cshell+0x246ca0 (a press: the palette pick), cshell+0x246de0 (a page), cshell+0x2470c0 (the slot grid),
// cshell+0x244f30 (the in-use question), cshell+0x242b80 (send); DISASM: cshell+0x24a780 (open), cshell+0x24a3a0
// (0x331 reader)

use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::pages::Align;
use std::path::Path;

use super::catalog::{argb, int, key, pair, Catalog, KeyPin};
use super::popup::{wrap_runs, BoxTag, Popup};
use super::sheet::{image, image_fit, place_text, ALIGN_CENTRE, ALIGN_LEFT};
use super::usebox::{
    draw_button, fill, inside, measure, NameArt, BUTTON_SIZE, CANCEL_TEXT, FONT, FONT_SIZE,
    NOTICE_TITLE, OK_TEXT, TITLE_COLOUR,
};
use crate::hud::items::DrawItem;

/// The kinds this window serves.
pub const SUB_NAME_COLOUR: i32 = 8;
pub const SUB_CLAN_COLOUR: i32 = 9;
pub const SUB_RETICLE: i32 = 11;

/// The commands: OK 0x268, Cancel 0x267, the confirmation's OK 0x271 and Cancel 0x272, the next and previous page
/// 0x25c / 0x25d.
// EVIDENCE: cshell+0x248660 (buttons 0x268 / 0x267, the confirmation's 0x271 captioned 0x42 and 0x272 captioned 0x43,
// the page links 0x25d `<` and 0x25c `>`), cshell+0x249a60 (0x25c / 0x25d move by five pages' first index)
pub const CMD_OK: i32 = 0x268;
pub const CMD_CANCEL: i32 = 0x267;
pub const CMD_SEND: i32 = 0x271;
pub const CMD_BACK: i32 = 0x272;

/// The in-use question: the text (kinds 8 / 9; kind 11 with the same item number; kind 11 with another), its title and
/// the captions Extend / New Color.
// EVIDENCE: cshell+0x244f30 (another item of the kind in use: box 0x183b, titled 0x371; the same item number: kind 11
// 0x1865, captions 0x183d / 0x183c; another number: kind 11 0x1866, OK / Cancel)
pub const IN_USE_TEXT: i32 = 0x183b;
pub const RETICLE_SAME_TEXT: i32 = 0x1865;
pub const RETICLE_OTHER_TEXT: i32 = 0x1866;
pub const EXTEND_TEXT: i32 = 0x183d;
pub const NEW_COLOUR_TEXT: i32 = 0x183c;

/// The preview name's font size and the page links' size and colours.
// EVIDENCE: cshell+0x248660 (PreViewPos text: size 0x10, flags 0x44, black until a pick; page links size 0x14,
// colours 0xffaaaaaa / 0xff666666)
pub const PREVIEW_SIZE: i32 = 0x10;
pub const PAGE_SIZE: i32 = 0x14;
pub const PAGE_COLOUR: u32 = 0xffaa_aaaa;
pub const PAGE_OFF_COLOUR: u32 = 0xff66_6666;
pub const WHITE: u32 = 0xffff_ffff;
/// The page links shown at most.
// EVIDENCE: cshell+0x248660 (five number links), cshell+0x246de0 (links past the last page hidden)
pub const PAGE_LINKS: usize = 5;
/// The reticle shown "before" without one in use: the plain cross in green.
// DISASM: cshell+0x24a840 (0xff00ff00), cshell+0x24a8b7 (the Type0 picture)
pub const NO_RETICLE_COLOUR: u32 = 0xff00_ff00;
/// The confirmation's frame (the shared popup window picture).
// EVIDENCE: cshell+0x248660 (Popup_Window 0x1dc x 0x101, centred)
pub const CONFIRM_SIZE: [i32; 2] = [0x1dc, 0x101];

/// The layout section and its keys.
// DISASM: cshell+0x248759..0x2499a0 (section literal VA 0x374f32f8 and the keys below)
pub mod keys {
    use super::{key, KeyPin};
    pub use crate::menu::usebox::keys::{CANCEL_POS, OK_POS, TITLE_POS, WINDOW_RECT};
    pub const SECTION: KeyPin = key("2d4e4540eb8fb1e5", 14);
    pub const CHOICE_POS: KeyPin = key("99585eea2cc25e89", 16);
    pub const PREVIEW_POS: KeyPin = key("ad83d1bf4cb1c856", 10);
    pub const LIST_POS: KeyPin = key("c3ed93d51de457f8", 7);
    pub const SLOT_SIZE: KeyPin = key("1b4f12bbf23f0c8c", 14);
    pub const SLOT_ON: KeyPin = key("a3e879a1ca5eb6f9", 18);
    pub const ROWS: KeyPin = key("5d21f18c41a77f26", 19);
    pub const COLUMNS: KeyPin = key("3155dc2e8c7b3baa", 19);
    pub const RETICLE_ROWS: KeyPin = key("01d171aaacbe19aa", 23);
    pub const ROW_GAP: KeyPin = key("17ba48d0354154ab", 24);
    pub const COLUMN_GAP: KeyPin = key("b4590eebf8222572", 24);
    pub const PREV_POS: KeyPin = key("981f76439a2b85ef", 13);
    pub const NUM_POS: KeyPin = key("e3a96227e1ce07c4", 13);
    pub const NEXT_POS: KeyPin = key("969828caf073f20f", 14);
    pub const PAGE_GAP: KeyPin = key("ef92549941c0fe99", 10);
    pub const RETICLE_LIST_POS: KeyPin = key("a1f5541c155e0487", 16);
    pub const ARROW_POS: KeyPin = key("99609d530bc18479", 12);
    pub const ARROW_SIZE: KeyPin = key("bf4736743a39c1ee", 13);
    pub const BEFORE_POS: KeyPin = key("83ef0a7ee90af99d", 15);
    pub const BEFORE_SIZE: KeyPin = key("74f496b0aa0cc01f", 16);
    pub const AFTER_POS: KeyPin = key("7e37826266978db6", 15);
    pub const AFTER_SIZE: KeyPin = key("e834ab06320b45da", 16);
    pub const C_PREVIEW_POS: KeyPin = key("474ddd755c03caa3", 20);
    pub const C_NOTICE_POS: KeyPin = key("73c4fc94eb3f2642", 16);
    pub const C_OK_POS: KeyPin = key("c9311b6b4477867b", 15);
    pub const C_CANCEL_POS: KeyPin = key("0ab02ee60f2be542", 19);
    /// The string sections (player name colour, clan name colour, reticle) and their keys.
    // DISASM: cshell+0x248727..0x248740 (kind 8 -> VA 0x374f3308, 9 -> VA 0x374f331c, 11 -> VA 0x374f3330)
    pub const NAME_STRINGS: KeyPin = key("a1aec0be291b0775", 19);
    pub const CLAN_STRINGS: KeyPin = key("233af8f4e9f44056", 19);
    pub const RETICLE_STRINGS: KeyPin = key("8a51e0afb98c1f4d", 20);
    pub const TITLE: KeyPin = key("a76193039bdcf5c7", 9);
    pub const CHOICE: KeyPin = key("3ccd665186bf915e", 10);
    pub const C_TITLE: KeyPin = key("ee973ae177fc27fa", 18);
    pub const C_NOTICE: KeyPin = key("746c9e8f97a0a1b4", 19);
}

/// The window's places (offsets inside the window, the confirmation's inside its frame).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColourLayout {
    pub window_size: [i32; 2],
    pub ok_at: [i32; 2],
    pub cancel_at: [i32; 2],
    pub title_at: [i32; 2],
    pub choice_at: [i32; 2],
    pub preview_at: [i32; 2],
    pub list_at: [i32; 2],
    pub reticle_list_at: [i32; 2],
    pub slot_size: [i32; 2],
    pub slot_on: String,
    pub rows: i32,
    pub columns: i32,
    pub reticle_rows: i32,
    pub row_gap: i32,
    pub column_gap: i32,
    pub prev_at: [i32; 2],
    pub num_at: [i32; 2],
    pub next_at: [i32; 2],
    pub page_gap: i32,
    pub arrow_at: [i32; 2],
    pub arrow_size: [i32; 2],
    pub before_at: [i32; 2],
    pub before_size: [i32; 2],
    pub after_at: [i32; 2],
    pub after_size: [i32; 2],
    pub c_preview_at: [i32; 2],
    pub c_notice_at: [i32; 2],
    pub c_ok_at: [i32; 2],
    pub c_cancel_at: [i32; 2],
}

/// DC-DEFAULT: CLI-1876 (proposed) our fallback when the ROM's layout section is missing: a 600 x 400 window on a
/// simple grid of our own round numbers (the real places are read from the ROM first; the on-slot picture stays empty
/// without it).
impl Default for ColourLayout {
    fn default() -> Self {
        ColourLayout {
            window_size: [600, 400],
            ok_at: [200, 350],
            cancel_at: [320, 350],
            title_at: [10, 10],
            choice_at: [40, 60],
            preview_at: [450, 100],
            list_at: [40, 180],
            reticle_list_at: [40, 240],
            slot_size: [40, 40],
            slot_on: String::new(),
            rows: 2,
            columns: 8,
            reticle_rows: 1,
            row_gap: 10,
            column_gap: 10,
            prev_at: [200, 320],
            num_at: [260, 320],
            next_at: [320, 320],
            page_gap: 20,
            arrow_at: [420, 110],
            arrow_size: [30, 20],
            before_at: [350, 80],
            before_size: [60, 60],
            after_at: [480, 80],
            after_size: [60, 60],
            c_preview_at: [200, 100],
            c_notice_at: [30, 50],
            c_ok_at: [100, 200],
            c_cancel_at: [250, 200],
        }
    }
}

impl ColourLayout {
    pub fn read(cat: &Catalog) -> ColourLayout {
        let d = ColourLayout::default();
        let v = |k: &KeyPin| cat.section_value(&keys::SECTION, k);
        let p = |k: &KeyPin, or: [i32; 2]| pair(v(k)).unwrap_or(or);
        let n = |k: &KeyPin, or: i32| int(v(k)).unwrap_or(or);
        ColourLayout {
            window_size: p(&keys::WINDOW_RECT, d.window_size),
            ok_at: p(&keys::OK_POS, d.ok_at),
            cancel_at: p(&keys::CANCEL_POS, d.cancel_at),
            title_at: p(&keys::TITLE_POS, d.title_at),
            choice_at: p(&keys::CHOICE_POS, d.choice_at),
            preview_at: p(&keys::PREVIEW_POS, d.preview_at),
            list_at: p(&keys::LIST_POS, d.list_at),
            reticle_list_at: p(&keys::RETICLE_LIST_POS, d.reticle_list_at),
            slot_size: p(&keys::SLOT_SIZE, d.slot_size),
            slot_on: v(&keys::SLOT_ON).map_or(d.slot_on, |s| s.trim().to_string()),
            rows: n(&keys::ROWS, d.rows).max(1),
            columns: n(&keys::COLUMNS, d.columns).max(1),
            reticle_rows: n(&keys::RETICLE_ROWS, d.reticle_rows).max(1),
            row_gap: n(&keys::ROW_GAP, d.row_gap),
            column_gap: n(&keys::COLUMN_GAP, d.column_gap),
            prev_at: p(&keys::PREV_POS, d.prev_at),
            num_at: p(&keys::NUM_POS, d.num_at),
            next_at: p(&keys::NEXT_POS, d.next_at),
            page_gap: n(&keys::PAGE_GAP, d.page_gap),
            arrow_at: p(&keys::ARROW_POS, d.arrow_at),
            arrow_size: p(&keys::ARROW_SIZE, d.arrow_size),
            before_at: p(&keys::BEFORE_POS, d.before_at),
            before_size: p(&keys::BEFORE_SIZE, d.before_size),
            after_at: p(&keys::AFTER_POS, d.after_at),
            after_size: p(&keys::AFTER_SIZE, d.after_size),
            c_preview_at: p(&keys::C_PREVIEW_POS, d.c_preview_at),
            c_notice_at: p(&keys::C_NOTICE_POS, d.c_notice_at),
            c_ok_at: p(&keys::C_OK_POS, d.c_ok_at),
            c_cancel_at: p(&keys::C_CANCEL_POS, d.c_cancel_at),
        }
    }

    /// The window's corner: centred on the 1024 x 768 screen.
    // EVIDENCE: cshell+0x248660 (the window placed at ((0x400 - w) / 2, (0x300 - h) / 2); the confirmation likewise)
    pub fn window_at(&self) -> [i32; 2] {
        centred(self.window_size)
    }
}

fn centred(size: [i32; 2]) -> [i32; 2] {
    [(0x400 - size[0]) / 2, (0x300 - size[1]) / 2]
}

/// The string ids of one kind: title, the choice line, the confirmation's title and question (0 = none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColourTexts {
    pub title: i32,
    pub choice: i32,
    pub confirm_title: i32,
    pub confirm_notice: i32,
}

impl ColourTexts {
    /// DC-DEFAULT: CLI-1876 (proposed) our fallback when the ROM's string section is missing: no text (id 0) for every
    /// kind; the real ids are read from the ROM first.
    pub fn fallback(_kind: i32) -> ColourTexts {
        ColourTexts {
            title: 0,
            choice: 0,
            confirm_title: 0,
            confirm_notice: 0,
        }
    }

    pub fn read(cat: &Catalog, kind: i32) -> ColourTexts {
        let d = ColourTexts::fallback(kind);
        let sec = match kind {
            SUB_CLAN_COLOUR => keys::CLAN_STRINGS,
            SUB_RETICLE => keys::RETICLE_STRINGS,
            _ => keys::NAME_STRINGS,
        };
        let n = |k: &KeyPin, or: i32| int(cat.section_value(&sec, k)).unwrap_or(or);
        ColourTexts {
            title: n(&keys::TITLE, d.title),
            choice: n(&keys::CHOICE, d.choice),
            confirm_title: n(&keys::C_TITLE, d.confirm_title),
            confirm_notice: n(&keys::C_NOTICE, d.confirm_notice),
        }
    }
}

/// One colour of a ROM colour table: `0xAARRGGBB` and its swatch picture.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Swatch {
    pub argb: u32,
    pub tex: String,
}

/// The ROM colour tables (name colours, reticle colours); colour index `n` is entry `n - 1`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColourTables {
    pub names: Vec<Swatch>,
    pub reticles: Vec<Swatch>,
}

impl ColourTables {
    /// Reads `ColorMax` entries `Color<n>` (`<r, g, b>`, opaque) and `Color<n>Texture` of the two tables.
    // DISASM: cshell+0x1dd97e..0x1ddab3 (per entry: `Color%d` -> 0xff000000 | r << 16 | g << 8 | b, each truncated to
    // a byte; `Color%dTexture` the swatch picture), cshell+0x1dcda0 / cshell+0x1dce50 (index n -> entry n - 1, 0
    // outside)
    pub fn from_doc(d: &dc_bute::ButeDoc) -> ColourTables {
        let table = |sec: &str| -> Vec<Swatch> {
            let n = d.get_int(sec, "ColorMax", 0).clamp(0, 256);
            (1..=n)
                .map(|k| {
                    let rgb = d
                        .get(sec)
                        .and_then(|m| m.get(&format!("Color{k}")))
                        .map(|v| super::catalog::ints(v))
                        .unwrap_or_default();
                    let byte = |i: usize| rgb.get(i).copied().unwrap_or(0).clamp(0, 255) as u32;
                    let tex = d
                        .get(sec)
                        .and_then(|m| m.get(&format!("Color{k}Texture")))
                        .map(|v| v.trim().to_string())
                        .unwrap_or_default();
                    Swatch {
                        argb: 0xff00_0000 | byte(0) << 16 | byte(1) << 8 | byte(2),
                        tex,
                    }
                })
                .collect()
        };
        ColourTables {
            names: table("NameColorTable"),
            reticles: table("CrosshairColorTable"),
        }
    }

    pub fn from_rom(rom: Option<&Path>) -> ColourTables {
        let Some(rom) = rom else {
            return ColourTables::default();
        };
        let vfs = dc_rez::RezVfs::new(rom);
        dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\COLORTABLE.TXT")
            .map(|d| ColourTables::from_doc(&d))
            .unwrap_or_default()
    }

    /// The swatches of kind `kind`'s window.
    pub fn of(&self, kind: i32) -> &[Swatch] {
        if kind == SUB_RETICLE {
            &self.reticles
        } else {
            &self.names
        }
    }

    /// The reticle colour of index `n` (`None` outside the table).
    pub fn reticle_argb(&self, n: u32) -> Option<u32> {
        (n as usize)
            .checked_sub(1)
            .and_then(|i| self.reticles.get(i))
            .map(|s| s.argb)
    }
}

/// Texture literals: the window frame, the arrow, the plain reticle picture, the confirmation's frame and its black
/// square.
// DISASM: cshell+0x24878d (frame VA 0x374f32c0), cshell+0x2491b4 (arrow VA 0x374f3160), cshell+0x249290 (Type0 VA
// 0x374f30f0), cshell+0x249459 (confirmation frame VA 0x374f2d88), cshell+0x24953c (black square VA 0x374f3098)
pub const WINDOW_TEX: Pin = pin("colour window", 0x3f32c0, "a2e4858649d1d246", 54);
pub const ARROW_TEX: Pin = pin("colour arrow", 0x3f3160, "0bd4d931cdad9a4c", 50);
pub const PLAIN_TEX: Pin = pin("plain reticle", 0x3f30f0, "912c3aa3b553cf65", 74);
pub const BLACK_TEX: Pin = pin("colour black square", 0x3f3098, "6aa07ef573bd5766", 48);
const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
    Pin {
        what,
        rva,
        sha16,
        len,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColourArt {
    pub window: Option<String>,
    pub arrow: Option<String>,
    pub plain: Option<String>,
    pub confirm: Option<String>,
    pub black: Option<String>,
}

impl ColourArt {
    pub fn from_rom(rom: Option<&Path>) -> ColourArt {
        let Some(pe) = rom.and_then(|r| {
            dc_rom::PeFile::from_file(&r.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1])).ok()
        }) else {
            return ColourArt::default();
        };
        let get = |p: &Pin| read_pinned(&pe, p).ok();
        ColourArt {
            window: get(&WINDOW_TEX),
            arrow: get(&ARROW_TEX),
            plain: get(&PLAIN_TEX),
            confirm: get(&super::countrybox::WINDOW_TEX),
            black: get(&BLACK_TEX),
        }
    }
}

/// What a press or a key did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourStep {
    None,
    /// Use the item with colour index `colour`, replacing the item `used` (0x330 +0x38 / +0x3c).
    Use {
        colour: u32,
        used: u32,
    },
    /// Cancel: the window closes with the cancelled box.
    Cancel,
}

/// Another item of the same kind in use when the window opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InUse {
    pub uid: u32,
    /// Its colour index (the function word); the window starts with it chosen.
    pub colour: u32,
    /// It has this item's item number.
    pub same_item: bool,
}

/// The open colour window.
#[derive(Debug, Clone, PartialEq)]
pub struct ColourBox {
    pub kind: i32,
    pub uid: u32,
    /// The chosen colour index (0 = none).
    pub picked: u32,
    /// OK takes a press (after a pick on the palette).
    pub ok_on: bool,
    /// The page shown (0-based).
    pub page: usize,
    pub confirm: bool,
    pub waiting: bool,
    pub in_use: Option<InUse>,
    pub swatches: Vec<Swatch>,
    /// Kinds 8 / 9: the own or the clan name the preview colours.
    pub name: String,
    /// Kind 11: the reticle in use (its picture and colour) and this item's picture.
    pub before: (String, u32),
    pub after: String,
    pub layout: ColourLayout,
    pub texts: ColourTexts,
}

const TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};
const CENTRE: Align = Align {
    first: ALIGN_CENTRE,
    second: 0,
};

impl ColourBox {
    /// The window of kind `kind` for item `uid`; `name` the preview name (8 / 9), `after` this item's reticle picture
    /// and `before` the reticle in use (11).
    // DISASM: cshell+0x24a780 (open: nothing picked, OK disabled, the preview name hidden; kind 8 the own name, 9 the
    // clan name; kind 11 the in-use reticle's picture in its colour, else the Type0 picture in green, and this item's
    // picture for the "after" preview and the confirmation, hidden until a pick; page 0)
    // EVIDENCE: cshell+0x244f30 (another item of the kind in use: its unique id kept for the send, its colour chosen)
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        cat: &Catalog,
        kind: i32,
        uid: u32,
        swatches: Vec<Swatch>,
        name: &str,
        before: (String, u32),
        after: &str,
        in_use: Option<InUse>,
    ) -> ColourBox {
        ColourBox {
            kind,
            uid,
            picked: in_use.map_or(0, |u| u.colour),
            ok_on: false,
            page: 0,
            confirm: false,
            waiting: false,
            in_use,
            swatches,
            name: name.to_string(),
            before,
            after: after.to_string(),
            layout: ColourLayout::read(cat),
            texts: ColourTexts::read(cat, kind),
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

    fn rows(&self) -> i32 {
        if self.kind == SUB_RETICLE {
            self.layout.reticle_rows
        } else {
            self.layout.rows
        }
    }

    /// Slots on a page.
    // EVIDENCE: cshell+0x2470c0 (rows x columns slots), cshell+0x248660 (kind 11: CrosshairListSlotVerNum rows at
    // CrosshairListPos)
    pub fn per_page(&self) -> usize {
        (self.rows() * self.layout.columns).max(1) as usize
    }

    pub fn pages(&self) -> usize {
        self.swatches.len().div_ceil(self.per_page()).max(1)
    }

    /// Slot `k` of the page: its corner.
    // EVIDENCE: cshell+0x2470c0 (slot (row, column) at (column x (width + column gap), row x (height + row gap)))
    pub fn slot_at(&self, k: usize) -> [i32; 2] {
        let l = &self.layout;
        let list = if self.kind == SUB_RETICLE {
            l.reticle_list_at
        } else {
            l.list_at
        };
        let (row, col) = ((k as i32) / l.columns, (k as i32) % l.columns);
        self.abs([
            list[0] + col * (l.slot_size[0] + l.column_gap),
            list[1] + row * (l.slot_size[1] + l.row_gap),
        ])
    }

    /// The swatch indices (0-based) on the page shown.
    fn shown(&self) -> std::ops::Range<usize> {
        let first = self.page * self.per_page();
        first.min(self.swatches.len())..(first + self.per_page()).min(self.swatches.len())
    }

    /// Picks colour index `n` (1-based).
    // EVIDENCE: cshell+0x246ca0 (a slot press: the index -> window +0x24, the preview shown in its colour, OK enabled)
    pub fn pick(&mut self, n: u32) {
        if n == 0 || n as usize > self.swatches.len() || self.waiting {
            return;
        }
        self.picked = n;
        self.ok_on = true;
    }

    /// The picked colour (`0xAARRGGBB`; 0 = none).
    pub fn picked_argb(&self) -> u32 {
        (self.picked as usize)
            .checked_sub(1)
            .and_then(|i| self.swatches.get(i))
            .map_or(0, |s| s.argb)
    }

    fn page_link_at(&self, k: usize) -> [i32; 2] {
        let l = &self.layout;
        self.abs([l.num_at[0] + k as i32 * l.page_gap, l.num_at[1]])
    }

    /// The first page link's page and the links shown.
    // EVIDENCE: cshell+0x246de0 (five links from the group of the page shown; a link past the last page hidden; `<`
    // shown past the first group, `>` before the last)
    fn links(&self) -> (usize, usize) {
        let first = self.page / PAGE_LINKS * PAGE_LINKS;
        (first, (self.pages() - first).min(PAGE_LINKS))
    }

    /// OK (or Enter) of the window that is up.
    // EVIDENCE: cshell+0x249a60 (0x268 with a colour: kind 11 shows the confirmation, the others send; 0x271: the
    // confirmation hidden, the send), cshell+0x242b80 (the send: the colour and the in-use item's unique id)
    // DC-DEFAULT: CLI-1876 (proposed) Enter runs OK of the window that is up, Escape runs Cancel of the confirmation
    // (back to the window) and of the window
    pub fn ok(&mut self) -> ColourStep {
        if self.waiting || !self.ok_on || self.picked == 0 {
            return ColourStep::None;
        }
        if self.kind == SUB_RETICLE && !self.confirm {
            self.confirm = true;
            return ColourStep::None;
        }
        self.confirm = false;
        self.send()
    }

    /// The use with the chosen colour (also the in-use question's Extend).
    pub fn send(&mut self) -> ColourStep {
        if self.picked == 0 {
            return ColourStep::None;
        }
        self.waiting = true;
        ColourStep::Use {
            colour: self.picked,
            used: self.in_use.map_or(0, |u| u.uid),
        }
    }

    pub fn escape(&mut self) -> ColourStep {
        if self.confirm {
            if !self.waiting {
                self.confirm = false;
            }
            return ColourStep::None;
        }
        ColourStep::Cancel
    }

    /// A refused use: the window stays.
    // DISASM: cshell+0x24a573 (ok = 0: the reason box only)
    pub fn refused(&mut self) {
        self.waiting = false;
    }

    /// A press at `p`; `None` outside the window.
    pub fn press(&mut self, p: [f32; 2]) -> Option<ColourStep> {
        let l = self.layout.clone();
        if self.confirm {
            if !inside(centred(CONFIRM_SIZE), CONFIRM_SIZE, p) {
                return None;
            }
            if inside(self.c_abs(l.c_ok_at), BUTTON_SIZE, p) {
                return Some(self.ok());
            }
            if inside(self.c_abs(l.c_cancel_at), BUTTON_SIZE, p) {
                return Some(self.escape());
            }
            return Some(ColourStep::None);
        }
        if !inside(l.window_at(), l.window_size, p) {
            return None;
        }
        if inside(self.abs(l.cancel_at), BUTTON_SIZE, p) {
            return Some(ColourStep::Cancel);
        }
        if inside(self.abs(l.ok_at), BUTTON_SIZE, p) {
            return Some(self.ok());
        }
        for (k, i) in self.shown().enumerate() {
            if inside(self.slot_at(k), l.slot_size, p) {
                self.pick(i as u32 + 1);
                return Some(ColourStep::None);
            }
        }
        // DC-DEFAULT: CLI-1876 (proposed) a page link takes a press in a 20 x 20 square at its point
        let link = [PAGE_SIZE, PAGE_SIZE];
        let (first, n) = self.links();
        for k in 0..n {
            if inside(self.page_link_at(k), link, p) {
                self.page = first + k;
                return Some(ColourStep::None);
            }
        }
        if first > 0 && inside(self.abs(l.prev_at), link, p) {
            self.page = first - PAGE_LINKS;
        } else if first + PAGE_LINKS < self.pages() && inside(self.abs(l.next_at), link, p) {
            self.page = first + PAGE_LINKS;
        }
        Some(ColourStep::None)
    }

    pub fn draw(
        &self,
        cat: &mut Catalog,
        art: &ColourArt,
        buttons: &NameArt,
        out: &mut Vec<DrawItem>,
    ) {
        let l = &self.layout;
        let text = |s: &str, at: [i32; 2], align: Align, size: i32, colour: u32| {
            place_text(s, at, [0, 0], align, FONT, size, colour)
        };
        let tinted = |tex: &str, at: [i32; 2], size: [i32; 2], c: u32| {
            let mut it = image(tex, at, size);
            if let DrawItem::Image { tint, .. } = &mut it {
                *tint = argb(c);
            }
            it
        };
        let w = l.window_at();
        match art.window.as_deref() {
            Some(t) => out.push(image_fit(cat, t, w, l.window_size)),
            None => out.push(fill(w, l.window_size, [24, 24, 24, 240])),
        }
        out.push(text(
            &cat.text(self.texts.title),
            self.abs(l.title_at),
            TL,
            FONT_SIZE,
            TITLE_COLOUR,
        ));
        out.push(text(
            &cat.text(self.texts.choice),
            self.abs(l.choice_at),
            TL,
            FONT_SIZE,
            WHITE,
        ));
        // the palette
        for (k, i) in self.shown().enumerate() {
            let at = self.slot_at(k);
            let s = &self.swatches[i];
            if s.tex.is_empty() {
                let c = argb(s.argb);
                out.push(fill(at, l.slot_size, c));
            } else {
                out.push(image(&s.tex, at, l.slot_size));
            }
            // DC-DEFAULT: CLI-1876 (proposed) the on-slot picture marks the chosen slot (hover is not tracked)
            if self.picked as usize == i + 1 && !l.slot_on.is_empty() {
                out.push(image(&l.slot_on, at, l.slot_size));
            }
        }
        // the page links
        let (first, n) = self.links();
        if first > 0 {
            out.push(text("<", self.abs(l.prev_at), TL, PAGE_SIZE, PAGE_COLOUR));
        }
        for k in 0..n {
            let c = if first + k == self.page {
                WHITE
            } else {
                PAGE_COLOUR
            };
            out.push(text(
                &(first + k + 1).to_string(),
                self.page_link_at(k),
                TL,
                PAGE_SIZE,
                c,
            ));
        }
        if first + PAGE_LINKS < self.pages() {
            out.push(text(">", self.abs(l.next_at), TL, PAGE_SIZE, PAGE_COLOUR));
        }
        // the preview
        if self.kind == SUB_RETICLE {
            let (tex, c) = &self.before;
            if !tex.is_empty() {
                out.push(tinted(tex, self.abs(l.before_at), l.before_size, *c));
            }
            if let Some(t) = art.arrow.as_deref() {
                out.push(image_fit(cat, t, self.abs(l.arrow_at), l.arrow_size));
            }
            if self.ok_on && !self.after.is_empty() {
                out.push(tinted(
                    &self.after,
                    self.abs(l.after_at),
                    l.after_size,
                    self.picked_argb(),
                ));
            }
        } else if self.ok_on {
            out.push(text(
                &self.name,
                self.abs(l.preview_at),
                CENTRE,
                PREVIEW_SIZE,
                self.picked_argb(),
            ));
        }
        draw_button(
            cat,
            buttons,
            self.abs(l.ok_at),
            OK_TEXT,
            self.ok_on && !self.waiting,
            out,
        );
        draw_button(cat, buttons, self.abs(l.cancel_at), CANCEL_TEXT, true, out);
        if self.confirm {
            self.draw_confirm(cat, art, buttons, out);
        }
    }

    // EVIDENCE: cshell+0x248660 (the confirmation: frame, title str_UseMsgBoxTitle in 0xffffc000 at TitlePos, the black
    // square and the reticle 1 pixel up-left of ConFirmBoxPreviewPos at ColorImageSize, str_UseMsgBoxNotice in white at
    // ConFirmBoxNotice), cshell+0x249a60 (0x268, kind 11: the confirmation's picture in the picked colour)
    fn draw_confirm(
        &self,
        cat: &mut Catalog,
        art: &ColourArt,
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
        let sq = self.c_abs(l.c_preview_at);
        match art.black.as_deref() {
            Some(t) => out.push(image(t, sq, l.slot_size)),
            None => out.push(fill(sq, l.slot_size, [0, 0, 0, 255])),
        }
        if !self.after.is_empty() {
            let mut it = image(&self.after, [sq[0] - 1, sq[1] - 1], l.slot_size);
            if let DrawItem::Image { tint, .. } = &mut it {
                *tint = argb(self.picked_argb());
            }
            out.push(it);
        }
        let notice = cat.text(self.texts.confirm_notice);
        let n_at = self.c_abs(l.c_notice_at);
        // DC-DEFAULT: CLI-1876 (proposed) the question wraps at the frame's width less twice its left offset, over 7
        // pixels a character
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
        draw_button(
            cat,
            buttons,
            self.c_abs(l.c_ok_at),
            OK_TEXT,
            !self.waiting,
            out,
        );
        draw_button(
            cat,
            buttons,
            self.c_abs(l.c_cancel_at),
            CANCEL_TEXT,
            !self.waiting,
            out,
        );
    }
}

/// The reticle of the match: the in-use reticle item's shape bits and colour (`0xAARRGGBB`, 0 outside the table);
/// `None` without one.
// EVIDENCE: cshell+0x1630a0 (the match entry: the in-use function record of kind 11 (cshell+0x227070) -> the mark's
// colour from its colour index (cshell+0x1dce50) and its shape from the item's ShapeType (cshell+0x25ac80,
// cshell+0x851f0); none: shape 0 and colour 0, the weapon's)
// DC-DEFAULT: CLI-1878 (proposed) the first in-use record of SubType 11 in the inventory
pub fn match_reticle(rom: Option<&Path>, store: &super::storage::Store) -> Option<(u16, u32)> {
    let wares = super::storage::Wares::cached(rom);
    let (rec, art) = store.items.iter().filter(|r| r.in_use).find_map(|r| {
        wares
            .art_of(r.item_no as i32)
            .filter(|a| a.sub_type == SUB_RETICLE)
            .map(|a| (r, a))
    })?;
    let argb = ColourTables::from_rom(rom)
        .reticle_argb(rec.function)
        .unwrap_or(0);
    Some((crate::spread::mark::shape_of(art.shape), argb))
}

/// The in-use question (CLI-1877): Extend / New Color / Cancel for the same item number, OK / Cancel for another.
// EVIDENCE: cshell+0x244f30 (the boxes and their captions), cshell+0x2431c0 (the same-number box: its first button sends
// at once, its last cancels), cshell+0x2431f0 (the other box: Cancel cancels the window, OK leaves it open)
pub fn in_use_box(cat: &Catalog, kind: i32, u: &InUse, uid: u32) -> Popup {
    let text = match (kind, u.same_item) {
        (SUB_RETICLE, true) => RETICLE_SAME_TEXT,
        (SUB_RETICLE, false) => RETICLE_OTHER_TEXT,
        _ => IN_USE_TEXT,
    };
    let tag = BoxTag::ColourInUse { uid };
    let mut q = if u.same_item {
        let (a, b, c) = (
            cat.text(EXTEND_TEXT),
            cat.text(NEW_COLOUR_TEXT),
            cat.text(CANCEL_TEXT),
        );
        Popup::choice(cat.text(text), tag, &[a.as_str(), b.as_str(), c.as_str()])
    } else {
        let mut q = Popup::ask(cat.text(text), tag);
        q.captions = Some([OK_TEXT, CANCEL_TEXT]);
        q
    };
    q.title = NOTICE_TITLE;
    q
}

/// What the in-use question's answer does: `Some(true)` sends at once (Extend), `Some(false)` cancels the window,
/// `None` leaves it open for a pick.
// DC-DEFAULT: CLI-1877 (proposed) New Color (the second of three buttons) leaves the window open; Escape cancels
pub fn in_use_answer(same_item: bool, chosen: Option<usize>, yes: bool) -> Option<bool> {
    if same_item {
        match chosen {
            Some(0) => Some(true),
            Some(1) => None,
            _ => Some(false),
        }
    } else if yes {
        None
    } else {
        Some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swatches(n: usize) -> Vec<Swatch> {
        (0..n)
            .map(|i| Swatch {
                argb: 0xff00_0000 | i as u32,
                tex: format!("sw{i}.dtx"),
            })
            .collect()
    }

    fn boxed(kind: i32, n: usize, in_use: Option<InUse>) -> ColourBox {
        let c = Catalog::fallback(None, 1024, 768);
        ColourBox::open(
            &c,
            kind,
            40,
            swatches(n),
            "Alice",
            ("before.dtx".into(), NO_RETICLE_COLOUR),
            "after.dtx",
            in_use,
        )
    }

    fn mid(at: [i32; 2]) -> [f32; 2] {
        [(at[0] + 5) as f32, (at[1] + 5) as f32]
    }

    #[test]
    fn the_name_window_sends_the_picked_index_at_once() {
        let mut b = boxed(SUB_NAME_COLOUR, 16, None);
        assert_eq!((b.per_page(), b.pages()), (16, 1));
        assert_eq!(b.layout.window_at(), [212, 184]);
        let ok = mid(b.abs(b.layout.ok_at));
        // OK is off until a pick
        assert_eq!(b.press(ok), Some(ColourStep::None));
        let slot = mid(b.slot_at(9));
        assert_eq!(b.press(slot), Some(ColourStep::None));
        assert_eq!((b.picked, b.ok_on), (10, true));
        assert_eq!(b.picked_argb(), 0xff00_0009);
        assert_eq!(
            b.press(ok),
            Some(ColourStep::Use {
                colour: 10,
                used: 0
            })
        );
        assert!(b.waiting);
        assert_eq!(b.press(ok), Some(ColourStep::None));
        b.refused();
        assert_eq!(
            b.ok(),
            ColourStep::Use {
                colour: 10,
                used: 0
            }
        );
        let cancel = mid(b.abs(b.layout.cancel_at));
        assert_eq!(b.press(cancel), Some(ColourStep::Cancel));
        assert_eq!(b.press([0.0, 0.0]), None);
    }

    #[test]
    fn the_reticle_window_confirms_first_and_carries_the_reticle_in_use() {
        let u = InUse {
            uid: 33,
            colour: 3,
            same_item: false,
        };
        let mut b = boxed(SUB_RETICLE, 8, Some(u));
        assert_eq!(
            (b.per_page(), b.pages(), b.picked, b.ok_on),
            (8, 1, 3, false)
        );
        // the second row's place has no slot on a reticle page
        assert_eq!(b.slot_at(0)[1], b.abs(b.layout.reticle_list_at)[1]);
        b.pick(5);
        assert_eq!(b.ok(), ColourStep::None);
        assert!(b.confirm);
        // the confirmation's Cancel goes back, its OK sends
        let back = mid(b.c_abs(b.layout.c_cancel_at));
        assert_eq!(b.press(back), Some(ColourStep::None));
        assert!(!b.confirm);
        b.ok();
        let ok = mid(b.c_abs(b.layout.c_ok_at));
        assert_eq!(
            b.press(ok),
            Some(ColourStep::Use {
                colour: 5,
                used: 33
            })
        );
        // the in-use question's Extend sends the colour in use
        let mut b = boxed(
            SUB_RETICLE,
            8,
            Some(InUse {
                same_item: true,
                ..u
            }),
        );
        assert_eq!(
            b.send(),
            ColourStep::Use {
                colour: 3,
                used: 33
            }
        );
    }

    #[test]
    fn pages_and_links_follow_the_table_length() {
        let mut b = boxed(SUB_NAME_COLOUR, 40, None);
        assert_eq!(b.pages(), 3);
        assert_eq!(b.links(), (0, 3));
        b.press(mid(b.page_link_at(2)));
        assert_eq!(b.page, 2);
        assert_eq!(b.shown(), 32..40);
        b.press(mid(b.slot_at(1)));
        assert_eq!(b.picked, 34);
        let mut out = Vec::new();
        let mut c = Catalog::fallback(None, 1024, 768);
        b.draw(&mut c, &ColourArt::default(), &NameArt::default(), &mut out);
        let texs: Vec<&str> = out
            .iter()
            .filter_map(|d| match d {
                DrawItem::Image { tex, .. } if tex.starts_with("sw") => Some(tex.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texs.len(), 8);
        assert!(out
            .iter()
            .any(|d| matches!(d, DrawItem::Label { text, .. } if text == "Alice")));
    }

    #[test]
    fn the_in_use_answers() {
        assert_eq!(in_use_answer(true, Some(0), true), Some(true));
        assert_eq!(in_use_answer(true, Some(1), false), None);
        assert_eq!(in_use_answer(true, Some(2), false), Some(false));
        assert_eq!(in_use_answer(true, None, false), Some(false));
        assert_eq!(in_use_answer(false, Some(0), true), None);
        assert_eq!(in_use_answer(false, Some(1), false), Some(false));
    }

    #[test]
    fn the_colour_table_reads_opaque_entries() {
        let d = dc_bute::ButeDoc::parse(
            "[NameColorTable]\nColorMax=2\nColor1=<010.0, 20.0, 30.0>\nColor1Texture=a.dtx\nColor2=<200.0, 100.0, 50.0>\n\
             [CrosshairColorTable]\nColorMax=1\nColor1=<18.0, 52.0, 86.0>\nColor1Texture=c.dtx\n",
        );
        let t = ColourTables::from_doc(&d);
        assert_eq!(t.names.len(), 2);
        assert_eq!(
            (t.names[0].argb, t.names[0].tex.as_str()),
            (0xff0a_141e, "a.dtx")
        );
        assert_eq!(t.names[1].argb, 0xffc8_6432);
        assert_eq!(t.reticle_argb(1), Some(0xff12_3456));
        assert_eq!((t.reticle_argb(0), t.reticle_argb(2)), (None, None));
    }
}
