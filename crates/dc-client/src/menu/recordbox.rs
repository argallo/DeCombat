//! use-windows: the record reset window of the function items 9005 (player record reset, `SubType` 5) and 9003
//! (clan record reset, `SubType` 3). A code-built 478 x 258 popup (layout section `StatResetBox`) with a title, the
//! current record line, a red warning and OK / Cancel. OK asks once more (the string section's check box, OK /
//! Cancel); its OK sends 0x330 with the unique id alone. The ok answer shows the section's success box; a player
//! reset clears the own kills, deaths and headshots and the item is deleted. Cancel closes the window with the
//! function-item cancelled box (as the rename window).
//!
//! The player window shows the own kills / deaths / headshots at once; the clan window asks for the clan's info
//! (0x3c2) and fills its wins / losses / draws from the answer.
// SPEC: PROTOCOL_2009.md §4.3 (0x330, 0x331: kinds 3 / 5 read +6 and +0xa), §5.1 (0x3c2 / 0x3c3)
// EVIDENCE: cshell+0x2445b0 (the window class, vtable VA 0x374f2a3c), cshell+0x244600 (build), cshell+0x2427e0
// (open), cshell+0x2428e0 (the clan info answer), cshell+0x244c00 (commands), cshell+0x242a10 (send),
// cshell+0x244cf0 (0x331 reader)

use dc_ui_data::pages::Align;

use super::catalog::{int, key, pair, Catalog, KeyPin};
use super::gm::{printf, Arg};
use super::popup::{BoxTag, Popup};
use super::roomtext::wrap_lines;
use super::sheet::{image_fit, place_text, ALIGN_LEFT};
use super::shop::colour_runs;
use super::usebox::{
    self, draw_button, fill, inside, measure, NameArt, BUTTON_SIZE, CANCEL_TEXT, FONT, FONT_SIZE,
    OK_TEXT, TITLE_COLOUR, WARNING_COLOUR,
};
use crate::hud::items::DrawItem;

/// The window's commands: OK 0x268, Cancel 0x267 (the rename window's).
// EVIDENCE: cshell+0x244600 (OK 0x268, Cancel 0x267, both 0x78 x 0x19)
pub const CMD_OK: i32 = usebox::CMD_OK;
pub const CMD_CANCEL: i32 = usebox::CMD_CANCEL;

/// The warning's wrap width.
// DISASM: cshell+0x244bb3 (the warning control's width 0x168)
pub const WARNING_WIDTH: i32 = 0x168;

/// The record line's base colour (white; its runs colour the numbers).
// DISASM: cshell+0x244ad7..0x244adf (the record control's colours -1)
pub const RECORD_COLOUR: u32 = 0xffff_ffff;

/// The layout section, its keys and the string sections.
// DISASM: cshell+0x2446bf / 0x2446c8 (kind 5 -> string section VA 0x374f2ac8, kind 3 -> VA 0x374f2ab8),
// cshell+0x2446cf..0x244b24 (section VA 0x374f2aa8: WindowPos, WindowRect, OKButPos, CancelButPos, TitlePos,
// CurrentRecordPos, WarningPos; str_Title, str_Warning), cshell+0x242819 (str_CurrentRecord), cshell+0x244c3a /
// 0x244c49 (str_CheckMsgBoxTitle, str_CheckMsgBoxNotic), cshell+0x244cf0 (str_UseMsgBoxTitle, str_UseMsgBoxNotic)
pub mod keys {
    use super::{key, KeyPin};
    pub use crate::menu::usebox::keys::{
        CANCEL_POS, OK_POS, TITLE, TITLE_POS, WARNING, WINDOW_POS, WINDOW_RECT,
    };
    pub const SECTION: KeyPin = key("1af961a4e824b7af", 12);
    pub const CURRENT_POS: KeyPin = key("99585eea2cc25e89", 16);
    pub const WARNING_POS: KeyPin = key("b7da6efa38e3f7d1", 10);
    pub const PLAYER_STRINGS: KeyPin = key("a68fe90ffecc8b4f", 14);
    pub const CLAN_STRINGS: KeyPin = key("7eb805710c741098", 14);
    pub const CURRENT: KeyPin = key("55b773323dbdc364", 17);
    pub const ASK_TITLE: KeyPin = key("cff8c8c64c41735f", 20);
    pub const ASK_TEXT: KeyPin = key("7509eddbe10ffa2d", 20);
    pub const DONE_TITLE: KeyPin = key("ee973ae177fc27fa", 18);
    pub const DONE_TEXT: KeyPin = key("670e34515875bdb4", 18);
}

/// The window's places (window origin, then offsets inside it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordLayout {
    pub window_at: [i32; 2],
    pub window_size: [i32; 2],
    pub ok_at: [i32; 2],
    pub cancel_at: [i32; 2],
    pub title_at: [i32; 2],
    pub record_at: [i32; 2],
    pub warning_at: [i32; 2],
}

/// DC-DEFAULT: CLI-1371 (proposed) our fallback when the ROM's layout section is missing: a 600 x 400 window on a
/// simple grid of our own round numbers (the real places are read from the ROM first).
impl Default for RecordLayout {
    fn default() -> Self {
        RecordLayout {
            window_at: [212, 184],
            window_size: [600, 400],
            ok_at: [200, 350],
            cancel_at: [320, 350],
            title_at: [10, 10],
            record_at: [20, 60],
            warning_at: [20, 150],
        }
    }
}

impl RecordLayout {
    pub fn read(cat: &Catalog) -> RecordLayout {
        let d = RecordLayout::default();
        let p = |k: &KeyPin, or: [i32; 2]| pair(cat.section_value(&keys::SECTION, k)).unwrap_or(or);
        RecordLayout {
            window_at: p(&keys::WINDOW_POS, d.window_at),
            window_size: p(&keys::WINDOW_RECT, d.window_size),
            ok_at: p(&keys::OK_POS, d.ok_at),
            cancel_at: p(&keys::CANCEL_POS, d.cancel_at),
            title_at: p(&keys::TITLE_POS, d.title_at),
            record_at: p(&keys::CURRENT_POS, d.record_at),
            warning_at: p(&keys::WARNING_POS, d.warning_at),
        }
    }

    fn abs(&self, at: [i32; 2]) -> [i32; 2] {
        [self.window_at[0] + at[0], self.window_at[1] + at[1]]
    }
}

/// The window's string ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordTexts {
    pub title: i32,
    pub warning: i32,
    pub current: i32,
    pub ask_title: i32,
    pub ask_text: i32,
    pub done_title: i32,
    pub done_text: i32,
}

impl RecordTexts {
    /// DC-DEFAULT: CLI-1371 (proposed) our fallback when the ROM's string section is missing: no text (id 0); the real
    /// ids are read from the ROM first.
    pub fn fallback(_clan: bool) -> RecordTexts {
        RecordTexts {
            title: 0,
            warning: 0,
            current: 0,
            ask_title: 0,
            ask_text: 0,
            done_title: 0,
            done_text: 0,
        }
    }

    pub fn read(cat: &Catalog, clan: bool) -> RecordTexts {
        let d = RecordTexts::fallback(clan);
        let sec = if clan {
            keys::CLAN_STRINGS
        } else {
            keys::PLAYER_STRINGS
        };
        let n = |k: &KeyPin, or: i32| int(cat.section_value(&sec, k)).unwrap_or(or);
        RecordTexts {
            title: n(&keys::TITLE, d.title),
            warning: n(&keys::WARNING, d.warning),
            current: n(&keys::CURRENT, d.current),
            ask_title: n(&keys::ASK_TITLE, d.ask_title),
            ask_text: n(&keys::ASK_TEXT, d.ask_text),
            done_title: n(&keys::DONE_TITLE, d.done_title),
            done_text: n(&keys::DONE_TEXT, d.done_text),
        }
    }
}

/// What a press did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordStep {
    None,
    /// OK: the check box (its OK sends the use).
    Ask,
    /// Cancel: the window closes with the cancelled box.
    Cancel,
}

/// The open record window.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordBox {
    pub uid: u32,
    pub clan: bool,
    /// The record the line shows (player: kills, deaths, headshots; clan: wins, losses, draws); `None` while the clan
    /// record is asked for.
    pub record: Option<[u32; 3]>,
    /// The use request waits for its answer.
    pub waiting: bool,
    pub layout: RecordLayout,
    pub texts: RecordTexts,
}

const TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};

impl RecordBox {
    /// The window for item `uid`; `own` = the own kills / deaths / headshots (the player window shows them, the clan
    /// window waits for the clan info answer).
    // EVIDENCE: cshell+0x2427e0 (kind 5: str_CurrentRecord with shell +0xfc / +0x100 / +0xd4; kind 3: the record
    // control hidden and the clan info request 0x3c2 (cshell+0x18d630)), cshell+0x2428e0 (the answer's +0x2a / +0x2e /
    // +0x32 into the form, the control shown)
    pub fn open(cat: &Catalog, uid: u32, clan: bool, own: [u32; 3]) -> RecordBox {
        RecordBox {
            uid,
            clan,
            record: (!clan).then_some(own),
            waiting: false,
            layout: RecordLayout::read(cat),
            texts: RecordTexts::read(cat, clan),
        }
    }

    /// The clan info answer (wins, losses, draws).
    pub fn clan_answer(&mut self, wins: u32, losses: u32, draws: u32) {
        if self.clan {
            self.record = Some([wins, losses, draws]);
        }
    }

    /// The record line.
    pub fn record_line(&self, cat: &Catalog) -> Option<String> {
        let [a, b, c] = self.record?;
        let form = cat.text(self.texts.current);
        Some(printf(
            &form,
            &[Arg::D(a as i32), Arg::D(b as i32), Arg::D(c as i32)],
        ))
    }

    /// A press at `p`; `None` outside the window.
    pub fn press(&mut self, p: [f32; 2]) -> Option<RecordStep> {
        let l = self.layout;
        if !inside(l.window_at, l.window_size, p) {
            return None;
        }
        if inside(l.abs(l.ok_at), BUTTON_SIZE, p) {
            return Some(self.ok());
        }
        if inside(l.abs(l.cancel_at), BUTTON_SIZE, p) {
            return Some(RecordStep::Cancel);
        }
        Some(RecordStep::None)
    }

    /// OK (or Enter): the check box, unless the use waits for its answer.
    // DC-DEFAULT: CLI-1371 (proposed) Enter runs OK and Escape Cancel (the 2009 window's keys are not traced)
    pub fn ok(&self) -> RecordStep {
        if self.waiting {
            RecordStep::None
        } else {
            RecordStep::Ask
        }
    }

    /// The check box: its OK sends the use.
    // EVIDENCE: cshell+0x244c00 (0x268: str_CheckMsgBoxNotic titled str_CheckMsgBoxTitle, OK 0x42 / Cancel 0x43; its
    // callback cshell+0x242170 runs the send vfunc +0x3c on OK only), cshell+0x242a10 (0x330 with the unique id, an
    // empty name and zeros)
    pub fn ask_box(&self, cat: &Catalog) -> Popup {
        let mut q = Popup::ask(
            cat.text(self.texts.ask_text),
            BoxTag::UseConfirm { uid: self.uid },
        );
        q.title = self.texts.ask_title;
        q.captions = Some([OK_TEXT, CANCEL_TEXT]);
        q
    }

    /// The success box.
    // EVIDENCE: cshell+0x244cf0 (ok: str_UseMsgBoxNotic titled str_UseMsgBoxTitle, OK 0x42; kind 5 clears shell +0xfc
    // / +0x100 / +0xd4; the item deleted (cshell+0x22ccb0) and the page rebuilt; refused: the reason box cshell+0x393f0)
    pub fn done_box(&self, cat: &Catalog) -> Popup {
        let mut q = Popup::ok(cat.text(self.texts.done_text), BoxTag::None);
        q.title = self.texts.done_title;
        q
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
        if let Some(line) = self.record_line(cat) {
            let [mut x, y] = l.abs(l.record_at);
            for (s, colour) in colour_runs(&line, RECORD_COLOUR) {
                out.push(text(&s, [x, y], colour));
                x += measure(cat, &s);
            }
        }
        let warn = cat.text(self.texts.warning);
        let lines = wrap_lines(&warn, WARNING_WIDTH as f32, |t| measure(cat, t) as f32);
        let at = l.abs(l.warning_at);
        for (k, line) in lines.iter().enumerate() {
            out.push(text(
                line,
                [at[0], at[1] + k as i32 * (FONT_SIZE + 2)],
                WARNING_COLOUR,
            ));
        }
        draw_button(cat, art, l.abs(l.ok_at), OK_TEXT, !self.waiting, out);
        draw_button(cat, art, l.abs(l.cancel_at), CANCEL_TEXT, true, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> Catalog {
        Catalog::fallback(None, 1024, 768)
    }

    #[test]
    fn the_player_window_shows_the_own_record_and_the_clan_window_waits() {
        let c = cat();
        let p = RecordBox::open(&c, 7, false, [10, 4, 3]);
        assert_eq!(p.record, Some([10, 4, 3]));
        assert_eq!(p.texts, RecordTexts::fallback(false));
        let mut k = RecordBox::open(&c, 8, true, [10, 4, 3]);
        assert_eq!(k.record, None);
        assert_eq!(k.record_line(&c), None);
        k.clan_answer(5, 6, 7);
        assert_eq!(k.record, Some([5, 6, 7]));
    }

    #[test]
    fn presses_hit_ok_and_cancel_and_ok_asks_first() {
        let c = cat();
        let mut b = RecordBox::open(&c, 7, false, [1, 2, 3]);
        let l = b.layout;
        let at = |q: [i32; 2]| {
            [
                (l.window_at[0] + q[0] + 5) as f32,
                (l.window_at[1] + q[1] + 5) as f32,
            ]
        };
        assert_eq!(b.press([0.0, 0.0]), None);
        assert_eq!(b.press(at(l.ok_at)), Some(RecordStep::Ask));
        assert_eq!(b.press(at(l.cancel_at)), Some(RecordStep::Cancel));
        assert_eq!(b.press(at([200, 150])), Some(RecordStep::None));
        b.waiting = true;
        assert_eq!(b.press(at(l.ok_at)), Some(RecordStep::None));
        // the string ids are ours here (the fallback has none)
        b.texts.ask_title = 4001;
        b.texts.done_title = 4002;
        let q = b.ask_box(&c);
        assert_eq!(q.tag, BoxTag::UseConfirm { uid: 7 });
        assert_eq!(q.title, 4001);
        assert_eq!(b.done_box(&c).title, 4002);
    }
}
