//! name-change: the 2009 item-use windows of the function items (item type 0x15, numbers 9000..9999). The storage
//! page's Equip (265) on a function item, a bought function item and an opened inbox function item open the window
//! of the item's ROM `SubType`; this module has the rename window (player name 4, clan name 1: a code-built 478 x 258
//! popup with the current name, a name field, Check, OK and Cancel), the kinds that are used at once (no window:
//! bonus passes, free passes, slot license, keys, respawn tokens, room badge), the 0x331 reason boxes and the
//! rename's success box.
//!
//! The rename flow: Check tests the length (4..12 player, 4..16 clan) and the current name locally, then asks the
//! lobby (0x346); a free name ("Name OK!") enables OK, which sends 0x330 with the checked name; typing again after a
//! check disables OK. The ok answer (0x331) shows the success box with the new name and the client renames itself
//! (no relogin); a refusal shows the reason table's box (0x53 = the name is in use).
// SPEC: PROTOCOL_2009.md §4.3 (0x330, 0x331 and its window table, 0x346 / 0x347), §4.5 (the reason table)
// DISASM: cshell+0x14f8a7..0x14f965 (storage command 265 on a cell of type 0x15: the inventory record, its item
// row (cshell+0x25c480); an item in use whose kind is not 0x16 opens nothing; else the check cshell+0x14cc00 and the
// item-use manager cshell+0x1db770(1, unique id, in-use flag) with the screen as owner (+0x4b0, command 0x40b))
// EVIDENCE: cshell+0x1db770 (one window per kind: 1 / 4 the name window, 10 / 12..16 / 18..21 the generic window, 22
// the megaphone), cshell+0x1db0b0 (the kind is the item row's SubType; only type 0x15 opens a window)

use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use std::path::Path;

use super::buybox::{fill_form, BUTTON_TEX};
use super::catalog::{int, key, pair, Catalog, KeyPin};
use super::popup::{text_ids, BoxTag, Popup};
use super::roomtext::wrap_lines;
use super::sheet::{image_fit, place_text, ALIGN_CENTRE, ALIGN_LEFT};
use super::shop::colour_runs;
use crate::hud::items::DrawItem;

/// The item type of the function items.
// SPEC: PROTOCOL_2009.md §4.5 (9000 -> 0x15)
pub const FUNCTION_TYPE: i32 = 0x15;

/// Function-item kinds (the item row's `SubType`).
pub const SUB_CLAN_NAME: i32 = 1;
pub const SUB_CLAN_RECORD: i32 = 3;
pub const SUB_PLAYER_NAME: i32 = 4;
pub const SUB_PLAYER_RECORD: i32 = 5;
pub const SUB_WEAPON_CHANGE: i32 = 12;
pub const SUB_COUNTRY: i32 = 17;
pub const SUB_MEGAPHONE: i32 = 22;

/// What a use of a function item of kind `sub` opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseAction {
    /// The rename window (player or clan name).
    Rename { clan: bool },
    /// No window: the use request goes at once and the answer marks the item in use.
    Direct,
    /// use-windows: the weapon change (kind 12): the 0x1876 question first, then the request with the item's
    /// `ChangeItemID` (CLI-1370).
    WeaponChange,
    /// use-windows: the record reset window (player 5, clan 3; CLI-1371).
    Record { clan: bool },
    /// use-items: the country window (kind 17; CLI-1873).
    Country,
    /// use-items: the megaphone (kind 22; CLI-1874): an item not in use is used at once (the generic window) and its
    /// ok answer opens the composer; an item in use opens the composer.
    Megaphone,
    /// use-items-sweep: the colour window (player name 8, clan name 9, reticle 11; CLI-1876).
    Colour { kind: i32 },
    /// use-items-emblem: the clan mark window (kind 2; CLI-1884).
    Mark,
    /// use-items-emblem: the face window (base character 6, gender 7; CLI-1885).
    Face { kind: i32 },
    /// A kind the 2009 manager opens no window for: the use shows nothing.
    NoWindow(i32),
}

/// The window of kind `sub`.
// EVIDENCE: cshell+0x1db770 (kinds 1, 4 -> the name window +0x10 / +0x1c; 10, 12..16, 18..21 -> the generic window),
// cshell+0x2450e0 (the generic window's open: every kind but 12 sends at once through its vfunc +0x3c; 12 asks
// 0x1876 first), cshell+0x242bc0 (the generic send: the unique id, an empty name, zeros, the change item number for
// kind 12)
// EVIDENCE: cshell+0x1db2c0 (the manager's windows: kinds 3 and 5 share the record window class cshell+0x2445b0)
// EVIDENCE: cshell+0x1db770 (kind 0x11 -> the country window +0x48 (class cshell+0x2a9350); kind 0x16 -> the generic
// window +0x5c, or with the in-use flag the composer +0x60 (class cshell+0x2a7020))
// EVIDENCE: cshell+0x1db2c0 (kinds 8, 9 and 11 share the colour window class cshell+0x246ba0)
// EVIDENCE: cshell+0x1db2c0 (kind 2: the clan mark window class cshell+0x2aacc0, slot +0x14; kinds 6 and 7 share the
// face window class cshell+0x246960)
// DC-DEFAULT: CLI-1365 (amended by CLI-1873 / CLI-1874 / CLI-1876 / CLI-1884 / CLI-1885) every kind of the 2009
// manager has its window; another kind shows nothing
pub fn action_of(sub: i32) -> UseAction {
    match sub {
        SUB_CLAN_NAME => UseAction::Rename { clan: true },
        SUB_PLAYER_NAME => UseAction::Rename { clan: false },
        SUB_CLAN_RECORD => UseAction::Record { clan: true },
        SUB_PLAYER_RECORD => UseAction::Record { clan: false },
        SUB_WEAPON_CHANGE => UseAction::WeaponChange,
        SUB_COUNTRY => UseAction::Country,
        SUB_MEGAPHONE => UseAction::Megaphone,
        8 | 9 | 11 => UseAction::Colour { kind: sub },
        2 => UseAction::Mark,
        6 | 7 => UseAction::Face { kind: sub },
        10 | 13..=16 | 18..=21 => UseAction::Direct,
        k => UseAction::NoWindow(k),
    }
}

/// The window's commands: Check (player name 0x264, clan name 0x265), Cancel 0x267, OK 0x268.
// EVIDENCE: cshell+0x243690 (kind 4: check command 0x264, kind 1: 0x265; OK 0x268, Cancel 0x267), cshell+0x2441f0
pub const CMD_CHECK_PLAYER: i32 = 0x264;
pub const CMD_CHECK_CLAN: i32 = 0x265;
pub const CMD_CANCEL: i32 = 0x267;
pub const CMD_OK: i32 = 0x268;

/// The length a name must have for Check: 4..12 units (player), 4..16 (clan).
// EVIDENCE: cshell+0x2441f0 (0x264: `< 4 || 0xc <` -> error 0, compared with the own name cshell+0x28d80; 0x265:
// `< 4 || 0x10 <`, compared with the clan name cshell+0x321a0)
pub const NAME_MIN: usize = 4;
pub const PLAYER_NAME_MAX: usize = 12;
pub const CLAN_NAME_MAX: usize = 16;

/// The captions: Check / "Name OK!" (after a free answer), OK, Cancel; the field's prompt "Enter new name".
// EVIDENCE: cshell+0x243690 (Check caption 0x17ed, OK 0x42, Cancel 0x43), cshell+0x2426d0 (a free answer: caption
// 0x17eb), cshell+0x242330 (the open: caption 0x17ed, the field's text 0x17cd), cshell+0x2425f0 / cshell+0x244480 (the
// first key or click into the field clears its text)
pub const CHECK_TEXT: i32 = 0x17ed;
pub const NAME_OK_TEXT: i32 = 0x17eb;
pub const PROMPT_TEXT: i32 = 0x17cd;
pub const OK_TEXT: i32 = 0x42;
pub const CANCEL_TEXT: i32 = 0x43;

/// Cancel's box: "You have cancelled usage of a Function Item..." titled "Function Item Use Cancelled".
// EVIDENCE: cshell+0x2433d0 (0x267: box 0x1807 titled 0x1811 with OK, its callback closes the window)
pub const CANCELLED_TEXT: i32 = 0x1807;
pub const CANCELLED_TITLE: i32 = 0x1811;

/// The layout section of the window and its keys.
// EVIDENCE: cshell+0x243690 (section literal VA 0x374f29d8: WindowPos, WindowRect, EditImgPos, CheckButPos, OKButPos,
// CancelButPos, TitlePos, CurrentNamePos, InputNamePos, ErrorPos, ErrorWidth, WraningPos, WarningWidth, EditPos,
// EditRect)
pub mod keys {
    use super::{key, KeyPin};
    pub const SECTION: KeyPin = key("6402057f76b1c0b6", 10);
    pub const WINDOW_POS: KeyPin = key("7fd8459190304736", 9);
    pub const WINDOW_RECT: KeyPin = key("d7ec185a6f8c9e9e", 10);
    pub const EDIT_IMG_POS: KeyPin = key("5ef1dd955759e11f", 10);
    pub const CHECK_POS: KeyPin = key("c86f9e4ca408ed51", 11);
    pub const OK_POS: KeyPin = key("f59a44bf674e1065", 8);
    pub const CANCEL_POS: KeyPin = key("0df3eb3dcdfa7a48", 12);
    pub const TITLE_POS: KeyPin = key("71c86b0f71532eee", 8);
    pub const CURRENT_POS: KeyPin = key("2a52f2a6d1d58e4f", 14);
    pub const INPUT_POS: KeyPin = key("217ff96a428959d9", 12);
    pub const ERROR_POS: KeyPin = key("c0de69ab33d0f432", 8);
    pub const ERROR_WIDTH: KeyPin = key("251586bf6ded6335", 10);
    pub const WARNING_POS: KeyPin = key("768b7692eb16f538", 10);
    pub const WARNING_WIDTH: KeyPin = key("f52a3f4c23c7c2cd", 12);
    pub const EDIT_POS: KeyPin = key("809f5afdcfe6a66c", 7);
    pub const EDIT_RECT: KeyPin = key("91e8449d3e7c9ffc", 8);
    /// The string sections (player name, clan name) and their keys.
    // EVIDENCE: cshell+0x243690 (kind 4 -> section VA 0x374f2a28, kind 1 -> VA 0x374f2a1c; `str_error%d` 0..4 into
    // +0x64..+0x74, str_Title, str_InputTitle, str_Warning), cshell+0x242330 (str_CurrentName), cshell+0x249d80
    // (str_MsgBoxTitle, str_MsgBoxNotic)
    pub const PLAYER_STRINGS: KeyPin = key("4f22c07401aed87f", 11);
    pub const CLAN_STRINGS: KeyPin = key("e2177cb33af22da1", 11);
    pub const ERRORS: [KeyPin; 5] = [
        key("871dff1994fdf915", 10),
        key("43d2b73cd92f7316", 10),
        key("977ef5c0d745074d", 10),
        key("e069444443145fa2", 10),
        key("0cbda5399f049003", 10),
    ];
    pub const TITLE: KeyPin = key("a76193039bdcf5c7", 9);
    pub const FIELD_LABEL: KeyPin = key("e0a7200790ee4bfd", 14);
    pub const WARNING: KeyPin = key("9ef164601c510735", 11);
    pub const CURRENT: KeyPin = key("16e6d67b7f36e2e4", 15);
    pub const BOX_TITLE: KeyPin = key("4e8c1ddb590e8f5f", 15);
    pub const BOX_TEXT: KeyPin = key("b63e9246c6fb6449", 15);
}

/// The window's places (window origin, then offsets inside it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameLayout {
    pub window_at: [i32; 2],
    pub window_size: [i32; 2],
    pub edit_img_at: [i32; 2],
    pub check_at: [i32; 2],
    pub ok_at: [i32; 2],
    pub cancel_at: [i32; 2],
    pub title_at: [i32; 2],
    pub current_at: [i32; 2],
    pub input_at: [i32; 2],
    pub error_at: [i32; 2],
    pub error_width: i32,
    pub warning_at: [i32; 2],
    pub warning_width: i32,
    pub edit_at: [i32; 2],
    pub edit_size: [i32; 2],
}

/// The buttons: 0x78 x 0x19.
// EVIDENCE: cshell+0x243690 (Check, OK and Cancel sized 0x78 x 0x19 through their vfunc +0x144)
pub const BUTTON_SIZE: [i32; 2] = [0x78, 0x19];

/// DC-DEFAULT: CLI-1366 (proposed) our fallback when the ROM's layout section is missing: a 600 x 400 window on a
/// simple grid of our own round numbers (the real places are read from the ROM first); the texts are font 0xe size 0xe.
impl Default for NameLayout {
    fn default() -> Self {
        NameLayout {
            window_at: [212, 184],
            window_size: [600, 400],
            edit_img_at: [100, 100],
            check_at: [300, 100],
            ok_at: [200, 350],
            cancel_at: [320, 350],
            title_at: [10, 10],
            current_at: [20, 50],
            input_at: [20, 100],
            error_at: [20, 200],
            error_width: 560,
            warning_at: [20, 250],
            warning_width: 560,
            edit_at: [102, 102],
            edit_size: [160, 40],
        }
    }
}

impl NameLayout {
    pub fn read(cat: &Catalog) -> NameLayout {
        let d = NameLayout::default();
        let p = |k: &KeyPin, or: [i32; 2]| pair(cat.section_value(&keys::SECTION, k)).unwrap_or(or);
        let n = |k: &KeyPin, or: i32| int(cat.section_value(&keys::SECTION, k)).unwrap_or(or);
        NameLayout {
            window_at: p(&keys::WINDOW_POS, d.window_at),
            window_size: p(&keys::WINDOW_RECT, d.window_size),
            edit_img_at: p(&keys::EDIT_IMG_POS, d.edit_img_at),
            check_at: p(&keys::CHECK_POS, d.check_at),
            ok_at: p(&keys::OK_POS, d.ok_at),
            cancel_at: p(&keys::CANCEL_POS, d.cancel_at),
            title_at: p(&keys::TITLE_POS, d.title_at),
            current_at: p(&keys::CURRENT_POS, d.current_at),
            input_at: p(&keys::INPUT_POS, d.input_at),
            error_at: p(&keys::ERROR_POS, d.error_at),
            error_width: n(&keys::ERROR_WIDTH, d.error_width),
            warning_at: p(&keys::WARNING_POS, d.warning_at),
            warning_width: n(&keys::WARNING_WIDTH, d.warning_width),
            edit_at: p(&keys::EDIT_POS, d.edit_at),
            edit_size: p(&keys::EDIT_RECT, d.edit_size),
        }
    }

    fn abs(&self, at: [i32; 2]) -> [i32; 2] {
        [self.window_at[0] + at[0], self.window_at[1] + at[1]]
    }
}

/// The window's string ids: the five messages (0 length, 1 free, 2 the instruction, 3 in use, 4 inappropriate),
/// title, field label, warning, current-name form and the success box's title and form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameTexts {
    pub errors: [i32; 5],
    pub title: i32,
    pub field_label: i32,
    pub warning: i32,
    pub current: i32,
    pub box_title: i32,
    pub box_text: i32,
}

impl NameTexts {
    /// DC-DEFAULT: CLI-1366 (proposed) our fallback when the ROM's string section is missing: no text (id 0); the real
    /// ids are read from the ROM first.
    pub fn fallback(_clan: bool) -> NameTexts {
        NameTexts {
            errors: [0; 5],
            title: 0,
            field_label: 0,
            warning: 0,
            current: 0,
            box_title: 0,
            box_text: 0,
        }
    }

    pub fn read(cat: &Catalog, clan: bool) -> NameTexts {
        let d = NameTexts::fallback(clan);
        let sec = if clan {
            keys::CLAN_STRINGS
        } else {
            keys::PLAYER_STRINGS
        };
        let n = |k: &KeyPin, or: i32| int(cat.section_value(&sec, k)).unwrap_or(or);
        let mut errors = d.errors;
        for (e, k) in errors.iter_mut().zip(keys::ERRORS.iter()) {
            *e = n(k, *e);
        }
        NameTexts {
            errors,
            title: n(&keys::TITLE, d.title),
            field_label: n(&keys::FIELD_LABEL, d.field_label),
            warning: n(&keys::WARNING, d.warning),
            current: n(&keys::CURRENT, d.current),
            box_title: n(&keys::BOX_TITLE, d.box_title),
            box_text: n(&keys::BOX_TEXT, d.box_text),
        }
    }
}

/// Texture literals of the window: the popup frame, the field bar, the buttons (normal and disabled forms).
// DISASM: cshell+0x2436ef..0x243730 (button forms VA 0x374eda10 / 0x374ed9e0 / 0x374ed9b0 / 0x374ed980),
// cshell+0x2437d6 (window VA 0x374f29e8), cshell+0x2438bb (field bar VA 0x374f2998)
pub const WINDOW_TEX: Pin = pin("name window", 0x3f29e8, "5a721112d78633ef", 38);
pub const BAR_TEX: Pin = pin("name window field bar", 0x3f2998, "2722a30c23ddf0e4", 42);
pub const BUTTON_OFF_TEX: Pin = pin("name window button off", 0x3ed980, "b204d117aee0fb98", 45);
const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
    Pin {
        what,
        rva,
        sha16,
        len,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NameArt {
    pub window: Option<String>,
    pub bar: Option<String>,
    pub button: Option<String>,
    pub button_off: Option<String>,
}

impl NameArt {
    pub fn from_rom(rom: Option<&Path>) -> NameArt {
        let Some(pe) = rom.and_then(|r| {
            dc_rom::PeFile::from_file(&r.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1])).ok()
        }) else {
            return NameArt::default();
        };
        let get = |p: &Pin| read_pinned(&pe, p).ok();
        NameArt {
            window: get(&WINDOW_TEX),
            bar: get(&BAR_TEX),
            button: get(&BUTTON_TEX),
            button_off: get(&BUTTON_OFF_TEX),
        }
    }
}

/// Text colours: title, the message line, the warning, the other texts and the captions (enabled / disabled).
// EVIDENCE: cshell+0x243690 (title 0xffffc000, message line 0xff828282, warning red (210, 5, 5))
// DC-DEFAULT: CLI-1366 (proposed) the current name, the field label and the typed name in 0xffeeeeee, captions
// 0xffeeeeee / 0xff777777 (disabled) as the other code-built windows
pub const TITLE_COLOUR: u32 = 0xffff_c000;
pub const MESSAGE_COLOUR: u32 = 0xff82_8282;
pub const WARNING_COLOUR: u32 = 0xff00_0000 | (210 << 16) | (5 << 8) | 5;
pub const TEXT_COLOUR: u32 = 0xffee_eeee;
pub const CAPTION_OFF: u32 = 0xff77_7777;
pub const FONT: i32 = 0xe;
pub const FONT_SIZE: i32 = 0xe;

/// What a press, a key or a character did to the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameStep {
    /// Nothing for the screen to do.
    None,
    /// Ask the lobby whether the name is free (0x346).
    Check(String),
    /// Use the item with the checked name (0x330).
    Use(String),
    /// Cancel: the window closes with the cancelled box.
    Cancel,
}

/// The open rename window.
#[derive(Debug, Clone, PartialEq)]
pub struct NameBox {
    pub uid: u32,
    pub clan: bool,
    /// The current name (own or clan) the window shows and compares with.
    pub current: String,
    /// The typed name.
    pub text: String,
    /// The field was clicked or typed into (its prompt is gone).
    pub touched: bool,
    /// The name the lobby answered free for (OK is enabled while the field still holds it).
    pub checked: Option<String>,
    /// A check or a use waits for its answer.
    pub waiting: bool,
    /// The message line's string id.
    pub message: i32,
    pub layout: NameLayout,
    pub texts: NameTexts,
}

const TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};
const MID: Align = Align {
    first: ALIGN_CENTRE,
    second: ALIGN_SECOND_CENTRE,
};

pub(crate) fn inside(at: [i32; 2], size: [i32; 2], p: [f32; 2]) -> bool {
    p[0] >= at[0] as f32
        && p[1] >= at[1] as f32
        && p[0] < (at[0] + size[0]) as f32
        && p[1] < (at[1] + size[1]) as f32
}

impl NameBox {
    /// The window for item `uid` (`clan`: the clan name window) showing `current`.
    // EVIDENCE: cshell+0x242330 (open: OK disabled, Cancel enabled, caption 0x17ed, the message line str_error2, the
    // field's prompt, both names cleared, "Current name" filled with the own (kind 4) or clan (kind 1) name)
    pub fn open(cat: &Catalog, uid: u32, clan: bool, current: &str) -> NameBox {
        let texts = NameTexts::read(cat, clan);
        NameBox {
            uid,
            clan,
            current: current.to_string(),
            text: String::new(),
            touched: false,
            checked: None,
            waiting: false,
            message: texts.errors[2],
            layout: NameLayout::read(cat),
            texts,
        }
    }

    pub fn max_len(&self) -> usize {
        if self.clan {
            CLAN_NAME_MAX
        } else {
            PLAYER_NAME_MAX
        }
    }

    /// OK is enabled: the field holds the name the lobby answered free for.
    pub fn ok_enabled(&self) -> bool {
        self.checked
            .as_deref()
            .is_some_and(|c| !c.is_empty() && c == self.text)
    }

    fn edited(&mut self) {
        // EVIDENCE: cshell+0x244480 (a key after a free answer: a field that no longer holds the checked name drops
        // the check, disables OK, caption back to 0x17ed, message str_error2)
        if self.checked.is_some() && !self.ok_enabled() {
            self.checked = None;
            self.message = self.texts.errors[2];
        }
    }

    /// A typed character (the field's limit is the check's upper length).
    // DC-DEFAULT: CLI-1366 (proposed) the field takes at most the check's upper length (12 / 16) and no control
    // characters
    pub fn type_char(&mut self, c: char) {
        self.touched = true;
        if c.is_control() || self.text.encode_utf16().count() >= self.max_len() {
            return;
        }
        self.text.push(c);
        self.edited();
    }

    pub fn backspace(&mut self) {
        self.touched = true;
        self.text.pop();
        self.edited();
    }

    /// Check: the local tests, then the lobby's check.
    // EVIDENCE: cshell+0x2441f0 (0x264 / 0x265: the field's text; its length outside 4..12 / 4..16 -> str_error0; the
    // current name -> str_error3; the word filter refusing it -> str_error4; else the check vfunc +0x44 =
    // cshell+0x242660, which sends 0x346)
    // DC-DEFAULT: CLI-1367 (proposed) the 2009 word filter (the object at VA 0x375bbdf4, vfunc +4) is not ported: every name
    // passes it and the lobby decides
    pub fn check(&mut self) -> NameStep {
        if self.waiting || self.ok_enabled() {
            return NameStep::None;
        }
        let n = self.text.encode_utf16().count();
        if !(NAME_MIN..=self.max_len()).contains(&n) {
            self.message = self.texts.errors[0];
            return NameStep::None;
        }
        if self.text == self.current {
            self.message = self.texts.errors[3];
            return NameStep::None;
        }
        self.waiting = true;
        NameStep::Check(self.text.clone())
    }

    /// The lobby's answer to the check.
    // EVIDENCE: cshell+0x2426d0 (ok != 0: checked, the checked name = the field, message str_error1, OK enabled,
    // caption 0x17eb; else message str_error3)
    pub fn checked_reply(&mut self, free: bool) {
        self.waiting = false;
        if free {
            self.checked = Some(self.text.clone());
            self.message = self.texts.errors[1];
        } else {
            self.checked = None;
            self.message = self.texts.errors[3];
        }
    }

    /// OK: the use request with the checked name.
    // EVIDENCE: cshell+0x2441f0 (0x268 only while checked and the field not empty -> vfunc +0x3c), cshell+0x242780
    // (0x330 with the window's unique id +0xc and the checked name +0x40)
    pub fn ok(&mut self) -> NameStep {
        if self.waiting || !self.ok_enabled() {
            return NameStep::None;
        }
        self.waiting = true;
        NameStep::Use(self.checked.clone().unwrap_or_default())
    }

    /// Enter: Check, or OK once the name is checked.
    // DC-DEFAULT: CLI-1367 (proposed) Enter runs Check, and OK once the lobby answered the name free; Escape runs
    // Cancel (the 2009 field's own keys are not traced)
    pub fn enter(&mut self) -> NameStep {
        if self.ok_enabled() {
            self.ok()
        } else {
            self.check()
        }
    }

    /// A press at `p`; `None` outside the window.
    pub fn press(&mut self, p: [f32; 2]) -> Option<NameStep> {
        let l = self.layout;
        if !inside(l.window_at, l.window_size, p) {
            return None;
        }
        if inside(l.abs(l.check_at), BUTTON_SIZE, p) {
            return Some(self.check());
        }
        if inside(l.abs(l.ok_at), BUTTON_SIZE, p) {
            return Some(self.ok());
        }
        if inside(l.abs(l.cancel_at), BUTTON_SIZE, p) {
            return Some(NameStep::Cancel);
        }
        if inside(l.abs(l.edit_at), l.edit_size, p) {
            self.touched = true;
        }
        Some(NameStep::None)
    }

    pub fn draw(&self, cat: &mut Catalog, art: &NameArt, out: &mut Vec<DrawItem>) {
        let l = self.layout;
        match art.window.as_deref() {
            Some(t) => out.push(image_fit(cat, t, l.window_at, l.window_size)),
            None => out.push(fill(l.window_at, l.window_size, [24, 24, 24, 240])),
        }
        let text = |s: &str, at: [i32; 2], colour: u32| {
            place_text(s, at, [0, 0], TL, FONT, FONT_SIZE, colour)
        };
        out.push(text(
            &cat.text(self.texts.title),
            l.abs(l.title_at),
            TITLE_COLOUR,
        ));
        // the current name: the form's `%s` filled, its colour runs drawn one after the other
        let current = fill_form(&cat.text(self.texts.current), &[&self.current]);
        let mut x = l.abs(l.current_at)[0];
        let y = l.abs(l.current_at)[1];
        for (s, colour) in colour_runs(&current, TEXT_COLOUR) {
            out.push(text(&s, [x, y], colour));
            x += measure(cat, &s);
        }
        out.push(text(
            &cat.text(self.texts.field_label),
            l.abs(l.input_at),
            TEXT_COLOUR,
        ));
        if let Some(t) = art.bar.as_deref() {
            out.push(image_fit(cat, t, l.abs(l.edit_img_at), [1024, 1024]));
        }
        let (field, colour) = if self.touched {
            (self.text.clone(), TEXT_COLOUR)
        } else {
            (cat.text(PROMPT_TEXT), MESSAGE_COLOUR)
        };
        out.push(place_text(
            &field,
            l.abs(l.edit_at),
            [l.edit_size[0], FONT_SIZE + 6],
            Align {
                first: ALIGN_LEFT,
                second: ALIGN_SECOND_CENTRE,
            },
            FONT,
            FONT_SIZE,
            colour,
        ));
        // the message line, centred on its point and wrapped at its width; the warning below
        // EVIDENCE: cshell+0x243690 (message control flags 0x14: centred on the x point; warning control left)
        let wrap = |cat: &mut Catalog, s: &str, w: i32| {
            wrap_lines(s, w as f32, |t| measure(cat, t) as f32)
        };
        let msg = cat.text(self.message);
        for (k, line) in wrap(cat, &msg, l.error_width).iter().enumerate() {
            let at = l.abs(l.error_at);
            out.push(place_text(
                line,
                [at[0], at[1] + k as i32 * (FONT_SIZE + 2)],
                [0, 0],
                Align {
                    first: ALIGN_CENTRE,
                    second: 0,
                },
                FONT,
                FONT_SIZE,
                MESSAGE_COLOUR,
            ));
        }
        let warn = cat.text(self.texts.warning);
        for (k, line) in wrap(cat, &warn, l.warning_width).iter().enumerate() {
            let at = l.abs(l.warning_at);
            out.push(text(
                line,
                [at[0], at[1] + k as i32 * (FONT_SIZE + 2)],
                WARNING_COLOUR,
            ));
        }
        let check = if self.ok_enabled() {
            NAME_OK_TEXT
        } else {
            CHECK_TEXT
        };
        for (at, id, on) in [
            (l.check_at, check, !self.waiting),
            (l.ok_at, OK_TEXT, self.ok_enabled() && !self.waiting),
            (l.cancel_at, CANCEL_TEXT, true),
        ] {
            draw_button(cat, art, l.abs(at), id, on, out);
        }
    }
}

/// use-windows: a code-built window button 0x78 x 0x19 with its caption (`on`: enabled form and colour).
pub(crate) fn draw_button(
    cat: &mut Catalog,
    art: &NameArt,
    at: [i32; 2],
    caption: i32,
    on: bool,
    out: &mut Vec<DrawItem>,
) {
    let tex = if on { &art.button } else { &art.button_off };
    match tex.as_deref().or(art.button.as_deref()) {
        Some(t) => out.push(image_fit(cat, t, at, BUTTON_SIZE)),
        None => out.push(fill(at, BUTTON_SIZE, [60, 60, 60, 255])),
    }
    // the caption centred about (0x33, 0xc) of the button, the middle of the 102-wide picture
    // DISASM: cshell+0x2439d3..0x243a00, cshell+0x243aec..0x243b16, cshell+0x243bfb..0x243c21 (Check / OK / Cancel:
    // point (0x33, 0xc), flags 4 | 0x40 -> cshell+0x33e190)
    out.push(place_text(
        &cat.text(caption),
        super::buybox::caption_point(at),
        [0, 0],
        MID,
        FONT,
        FONT_SIZE,
        if on { TEXT_COLOUR } else { CAPTION_OFF },
    ));
}

pub(crate) fn measure(cat: &mut Catalog, s: &str) -> i32 {
    cat.measure_text(FONT, FONT_SIZE, s)
        .map_or(s.chars().count() as f32 * 7.0, |m| m.0)
        .round() as i32
}

pub(crate) fn fill(at: [i32; 2], size: [i32; 2], colour: [u8; 4]) -> DrawItem {
    DrawItem::Fill {
        rect: [at[0] as f32, at[1] as f32, size[0] as f32, size[1] as f32],
        colour,
    }
}

/// use-windows: the weapon change's question ("By activating the '%s' item, your default %s weapon will be changed
/// permanently..."), its success box ("Your default %s weapon has been changed permanently to the %s...") and their
/// title ("Notice").
// EVIDENCE: cshell+0x2450e0 (kind 12: box 0x1876 with the item's name and the slot word of its change item, titled
// 0x371, OK 0x42 / Cancel 0x43), cshell+0x2452f0 (kind 12 ok: box 0x187b with the slot word, the change item's name,
// the slot word, titled 0x371, OK)
pub const WEAPON_CHANGE_ASK: i32 = 0x1876;
pub const WEAPON_CHANGED: i32 = 0x187b;
pub const NOTICE_TITLE: i32 = 0x371;

/// The slot words the weapon change's boxes name (primary, secondary, melee, throw), by equipment slot 0..3.
// DISASM: cshell+0x2451da..0x2451ef (the change item's slot 0..3 -> VA 0x374f2b4c / 0x374f2b40 / 0x374f2b38 /
// 0x374f2b30)
pub const SLOT_WORDS: [Pin; 4] = [
    pin("slot word 0", 0x3f2b4c, "986a1b7135f49861", 7),
    pin("slot word 1", 0x3f2b40, "c0f69e19ba252767", 9),
    pin("slot word 2", 0x3f2b38, "c5f5aae9769a367a", 5),
    pin("slot word 3", 0x3f2b30, "7b0456581bef4cb1", 5),
];

/// The four slot words read from the ROM (empty without it: the 2009 box keeps its word empty for another slot).
pub fn slot_words(rom: Option<&Path>) -> [String; 4] {
    let pe = rom.and_then(|r| {
        dc_rom::PeFile::from_file(&r.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1])).ok()
    });
    SLOT_WORDS.map(|p| pe.as_ref().and_then(|pe| read_pinned(pe, &p).ok()).unwrap_or_default())
}

/// The weapon change's question: OK / Cancel under the "Notice" title.
pub fn weapon_change_ask(cat: &Catalog, item: &str, slot_word: &str, uid: u32) -> Popup {
    let mut q = Popup::ask(
        fill_form(&cat.text(WEAPON_CHANGE_ASK), &[item, slot_word]),
        BoxTag::UseConfirm { uid },
    );
    q.title = NOTICE_TITLE;
    q.captions = Some([OK_TEXT, CANCEL_TEXT]);
    q
}

/// The weapon change's success box.
pub fn weapon_changed_box(cat: &Catalog, slot_word: &str, weapon: &str) -> Popup {
    let mut q = Popup::ok(
        fill_form(&cat.text(WEAPON_CHANGED), &[slot_word, weapon, slot_word]),
        BoxTag::None,
    );
    q.title = NOTICE_TITLE;
    q
}

/// The box Cancel shows.
pub fn cancelled_box(cat: &Catalog) -> Popup {
    let mut q = Popup::ok(cat.text(CANCELLED_TEXT), BoxTag::None);
    q.title = CANCELLED_TITLE;
    q
}

/// The rename's success box: the form with the new name, titled by the section.
// EVIDENCE: cshell+0x249d80 (ok: str_MsgBoxNotic filled with the checked name, titled str_MsgBoxTitle, OK 0x42)
pub fn renamed_box(cat: &Catalog, texts: &NameTexts, name: &str) -> Popup {
    let mut q = Popup::ok(fill_form(&cat.text(texts.box_text), &[name]), BoxTag::None);
    q.title = texts.box_title;
    q
}

/// The box of a refused use (`reason` of 0x331 +0xa): string and title.
// EVIDENCE: cshell+0x393f0 (title 0x346 with OK 0x42 unless stated: 0x3c 0x503, 0x4a 0x17f2, 0x4b 0x17f3, 0x4c
// 0x17f4, 0x4e 0x17f5, 0x4f 0x17f6, 0x53 0x17f7, 0x56 / 0x6b 0x1805 (title 0x181e), 0x57 0x17fc, 0x58 0x17f8, 0x5f
// 0x1800, 0x60 / 0x61 0x17f9, 0x62 0x17fa (title 0x181e), 0x65 0x18bf, 0x67 0x1873 (title 0x17e5), 0x68 0x17fb (title
// 0x181e), 0x71 0x1828 (0x1827), 0x72 0x182a (0x1829), 0x73 0x182e (0x182d), 0x8b 0x1891, 0x8f 0x1903, 0x90 0x1905;
// any other value: a server error line with the number)
pub fn reason_text(reason: u32) -> Option<(i32, i32)> {
    let e = text_ids::TITLE_ERROR;
    Some(match reason & 0xffff {
        0x3c => (0x503, e),
        0x4a => (0x17f2, e),
        0x4b => (0x17f3, e),
        0x4c => (0x17f4, e),
        0x4e => (0x17f5, e),
        0x4f => (0x17f6, e),
        0x53 => (0x17f7, e),
        0x56 | 0x6b => (0x1805, 0x181e),
        0x57 => (0x17fc, e),
        0x58 => (0x17f8, e),
        0x5f => (0x1800, e),
        0x60 | 0x61 => (0x17f9, e),
        0x62 => (0x17fa, 0x181e),
        0x65 => (0x18bf, e),
        0x67 => (0x1873, 0x17e5),
        0x68 => (0x17fb, 0x181e),
        0x71 => (0x1828, 0x1827),
        0x72 => (0x182a, 0x1829),
        0x73 => (0x182e, 0x182d),
        0x8b => (0x1891, e),
        0x8f => (0x1903, e),
        0x90 => (0x1905, e),
        _ => return None,
    })
}

// DC-DEFAULT: CLI-1368 (proposed) a reason outside the table shows our own line "Server error (<n>)" titled 0x346
pub fn reason_box(cat: &Catalog, reason: u32) -> Popup {
    let (text, title) = match reason_text(reason) {
        Some((t, title)) => (cat.text(t), title),
        None => (
            format!("Server error ({})", reason & 0xffff),
            text_ids::TITLE_ERROR,
        ),
    };
    let mut q = Popup::ok(text, BoxTag::None);
    q.title = title;
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed() -> NameBox {
        let cat = Catalog::fallback(None, 1024, 768);
        NameBox::open(&cat, 77, false, "Casey")
    }

    #[test]
    fn kinds_pick_the_2009_windows() {
        assert_eq!(action_of(4), UseAction::Rename { clan: false });
        assert_eq!(action_of(1), UseAction::Rename { clan: true });
        for k in [10, 13, 14, 15, 16, 18, 19, 20, 21] {
            assert_eq!(action_of(k), UseAction::Direct, "kind {k}");
        }
        assert_eq!(action_of(12), UseAction::WeaponChange);
        assert_eq!(action_of(3), UseAction::Record { clan: true });
        assert_eq!(action_of(5), UseAction::Record { clan: false });
        assert_eq!(action_of(17), UseAction::Country);
        assert_eq!(action_of(22), UseAction::Megaphone);
        for k in [8, 9, 11] {
            assert_eq!(action_of(k), UseAction::Colour { kind: k });
        }
        assert_eq!(action_of(2), UseAction::Mark);
        for k in [6, 7] {
            assert_eq!(action_of(k), UseAction::Face { kind: k });
        }
        for k in [0, 23, -1] {
            assert_eq!(action_of(k), UseAction::NoWindow(k), "kind {k}");
        }
    }

    #[test]
    fn check_tests_the_length_and_the_current_name_first() {
        let mut b = boxed();
        assert_eq!(b.message, b.texts.errors[2]);
        for c in "abc".chars() {
            b.type_char(c);
        }
        assert_eq!(b.check(), NameStep::None);
        assert_eq!(b.message, b.texts.errors[0]);
        b.text = "Casey".into();
        assert_eq!(b.check(), NameStep::None);
        assert_eq!(b.message, b.texts.errors[3]);
        b.text = "NewName".into();
        assert_eq!(b.check(), NameStep::Check("NewName".into()));
        assert!(b.waiting);
        // a second check waits for the answer
        assert_eq!(b.check(), NameStep::None);
    }

    #[test]
    fn the_field_takes_at_most_twelve_units_for_a_player_name() {
        let mut b = boxed();
        for c in "abcdefghijklmnop".chars() {
            b.type_char(c);
        }
        assert_eq!(b.text, "abcdefghijkl");
        let cat = Catalog::fallback(None, 1024, 768);
        let mut c = NameBox::open(&cat, 1, true, "Clan");
        for ch in "abcdefghijklmnopqr".chars() {
            c.type_char(ch);
        }
        assert_eq!(c.text.len(), 16);
    }

    #[test]
    fn ok_needs_a_free_answer_for_the_name_in_the_field() {
        let mut b = boxed();
        b.text = "NewName".into();
        assert_eq!(b.ok(), NameStep::None);
        assert_eq!(b.enter(), NameStep::Check("NewName".into()));
        b.checked_reply(true);
        assert!(b.ok_enabled());
        assert_eq!(b.message, b.texts.errors[1]);
        // typing drops the check
        b.type_char('x');
        assert!(!b.ok_enabled());
        assert_eq!(b.message, b.texts.errors[2]);
        b.backspace();
        assert_eq!(b.enter(), NameStep::Check("NewName".into()));
        b.checked_reply(true);
        assert_eq!(b.enter(), NameStep::Use("NewName".into()));
        // a taken answer
        let mut t = boxed();
        t.text = "Taken1".into();
        t.check();
        t.checked_reply(false);
        assert!(!t.ok_enabled());
        assert_eq!(t.message, t.texts.errors[3]);
    }

    #[test]
    fn presses_hit_the_layout_buttons() {
        let mut b = boxed();
        let l = b.layout;
        let at = |q: [i32; 2]| {
            [
                (l.window_at[0] + q[0] + 5) as f32,
                (l.window_at[1] + q[1] + 5) as f32,
            ]
        };
        assert_eq!(b.press([0.0, 0.0]), None);
        assert_eq!(b.press(at(l.cancel_at)), Some(NameStep::Cancel));
        assert!(!b.touched);
        assert_eq!(b.press(at(l.edit_at)), Some(NameStep::None));
        assert!(b.touched);
        b.text = "Fresh".into();
        assert_eq!(
            b.press(at(l.check_at)),
            Some(NameStep::Check("Fresh".into()))
        );
    }

    #[test]
    fn refusals_use_the_reason_table() {
        assert_eq!(reason_text(0x53), Some((0x17f7, text_ids::TITLE_ERROR)));
        assert_eq!(reason_text(0x57), Some((0x17fc, text_ids::TITLE_ERROR)));
        assert_eq!(reason_text(0x4e), Some((0x17f5, text_ids::TITLE_ERROR)));
        assert_eq!(reason_text(0x56), Some((0x1805, 0x181e)));
        assert_eq!(reason_text(0), None);
        let cat = Catalog::fallback(None, 1024, 768);
        assert!(reason_box(&cat, 0).text.contains("Server error (0)"));
    }
}
