//! use-items-emblem: the clan mark window of the function items of `SubType` 2 (clan emblem). A code-built popup by
//! the layout section `ClanMark` in two steps: first an emblem, then a background, each picked from the ROM clan mark
//! table (`clan::ClanMarks`) on seven category tabs (tab 0 lists every entry of a category 1..6) in pages of
//! `rows x columns` slots with page links `<`, `1..5`, `>`. The clan's mark now and the picked mark are shown side by
//! side. Step 1 has Cancel / Next, step 2 Back / OK / Check: Check asks the lobby whether the clan may take the pair
//! (0x34c), an ok answer enables OK, OK shows the confirmation (the mark, a question, Back / OK) whose OK sends 0x330
//! with the pair. The ok answer marks the item in use and makes the pair the own clan's mark.
// SPEC: PROTOCOL_2009.md §4.3 (0x330 +0x2c emblem, +0x30 background; 0x331 window table row 2), §5.1 (0x34c / 0x34d)
// EVIDENCE: cshell+0x2aacc0 (the class, vtable VA 0x374f8ecc), cshell+0x2ac5c0 (build), cshell+0x2ae470 (commands),
// cshell+0x2ae340 (open), cshell+0x2adf70 (a press), cshell+0x2ae130 (the steps), cshell+0x2ac1b0 (a tab),
// cshell+0x2ac060 (the page links), cshell+0x2ab220 (the 0x331 reader), cshell+0x2ab3a0 (the 0x34d reader); DISASM:
// cshell+0x2aada0 (the check)

use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use std::path::Path;

use super::catalog::{int, key, pair, Catalog, KeyPin};
use super::clan::ClanMarks;
use super::colorbox::{PAGE_COLOUR, PAGE_LINKS, PAGE_OFF_COLOUR, PAGE_SIZE, WHITE};
use super::popup::{text_ids, BoxTag, Popup};
use super::roomtext::wrap_lines;
use super::sheet::{image, image_fit, place_text, ALIGN_CENTRE, ALIGN_LEFT};
use super::usebox::{
    draw_button, fill, inside, measure, NameArt, BUTTON_SIZE, CANCEL_TEXT, FONT, FONT_SIZE,
    TITLE_COLOUR,
};
use crate::hud::items::DrawItem;

/// The kind this window serves.
pub const SUB_CLAN_MARK: i32 = 2;

/// The commands: OK (the confirmation) 0x25e, Next 0x25f, Back 0x260, Cancel 0x261, the confirmation's OK 0x262 and
/// Back 0x263, Check 0x266, the next and previous page group 0x25c / 0x25d.
// EVIDENCE: cshell+0x2ac5c0 (buttons 0x261 Cancel, 0x25f Next, 0x260 Back, 0x25e OK, 0x266 Check, 0x263 / 0x262 in the
// confirmation, the links 0x25d `<` and 0x25c `>`), cshell+0x2ae470 (the commands)
pub const CMD_OK: i32 = 0x25e;
pub const CMD_NEXT: i32 = 0x25f;
pub const CMD_BACK: i32 = 0x260;
pub const CMD_CANCEL: i32 = 0x261;
pub const CMD_SEND: i32 = 0x262;
pub const CMD_MSG_BACK: i32 = 0x263;
pub const CMD_CHECK: i32 = 0x266;

/// The captions: Next and OK 0x17b5, Back 0x3e, Cancel 0x43, Check 0x17ed; the confirmation's OK 0x17cf.
// EVIDENCE: cshell+0x2ac5c0 (the button captions; the confirmation: Back 0x3e, OK 0x17cf)
pub const NEXT_TEXT: i32 = 0x17b5;
pub const BACK_TEXT: i32 = 0x3e;
pub const CHECK_TEXT: i32 = 0x17ed;
pub const SEND_TEXT: i32 = 0x17cf;

/// The texts: the title (0xffffc000), the two lines of each step (the first in 0xffaaaaaa, the second white and wrapped
/// at 0x14a), the confirmation's question and title.
// EVIDENCE: cshell+0x2ac5c0 (title 0x17b4 at strTitlePos in 0xffffc000; the confirmation's 0x17d0 at (0x1e, MsgStrPos
// y) 0x14a wide and 0x17e1 at (0xd, 6)), cshell+0x2ae130 (step 1: 0x179c / 0x179d, step 2: 0x179e / 0x179f on the
// StrSelectPos / StrImgTitlePos texts)
// EVIDENCE: cshell+0x2ac5c0 (the StrSelectPos text in 0xffaaaaaa; the StrImgTitlePos
// text in white, 0x14a wide)
pub const TITLE_TEXT: i32 = 0x17b4;
pub const SELECT_COLOUR: u32 = 0xffaa_aaaa;
pub const IMG_TITLE_WIDTH: i32 = 0x14a;
pub const STEP_TEXTS: [[i32; 2]; 2] = [[0x179c, 0x179d], [0x179e, 0x179f]];
pub const MSG_TEXT: i32 = 0x17d0;
pub const MSG_TITLE: i32 = 0x17e1;
pub const MSG_TEXT_AT: [i32; 2] = [0x1e, 0];
pub const MSG_TEXT_WIDTH: i32 = 0x14a;
pub const MSG_TITLE_AT: [i32; 2] = [0xd, 6];

/// The boxes: Next without an emblem 0x179b, Check without a background 0x179a, the clan's own mark 0x17f4 (titled
/// 0x346 with OK); an ok check 0x1804 titled 0x1812; an ok use 0x1798 titled 0x17e8.
// EVIDENCE: cshell+0x2ae470 (0x25f: 0x179b; 0x266: 0x179a, the pair of shell +0x428 / +0x42c -> 0x17f4, all titled
// 0x346), cshell+0x2ab220 (ok: 0x1798 titled 0x17e8, its OK closes the window), cshell+0x2ab3a0 (an ok check: 0x1804
// titled 0x1812)
pub const NO_EMBLEM_TEXT: i32 = 0x179b;
pub const NO_BACKGROUND_TEXT: i32 = 0x179a;
pub const OWN_MARK_TEXT: i32 = 0x17f4;
pub const CHECKED_TEXT: i32 = 0x1804;
pub const CHECKED_TITLE: i32 = 0x1812;
pub const DONE_TEXT: i32 = 0x1798;
pub const DONE_TITLE: i32 = 0x17e8;

/// The captions of the seven tabs of each step (emblems, backgrounds).
// EVIDENCE: cshell+0x280a90 (the mark table's tab caption ids: emblems 0x17a5, 0x17a6, 0x17a7, 0x17a8, 0x17a9, 0x17b1,
// 0x17b2; backgrounds 0x17a0, 0x17a2, 0x17a1, 0x17a3, 0x17af, 0x17a4, 0x17b0), cshell+0x27fbd0 (tab k's caption),
// cshell+0x2ac5c0 (seven tabs per step)
pub const TAB_TEXTS: [[i32; 7]; 2] = [
    [0x17a5, 0x17a6, 0x17a7, 0x17a8, 0x17a9, 0x17b1, 0x17b2],
    [0x17a0, 0x17a2, 0x17a1, 0x17a3, 0x17af, 0x17a4, 0x17b0],
];
pub const TABS: usize = 7;

/// The mark pictures' size on the window and the confirmation.
// EVIDENCE: cshell+0x2ac5c0 (MarkNowPos, MarkViewPos and MsgMarkPos controls 0x40 x 0x40 at scale 1.0)
pub const MARK_SIZE: [i32; 2] = [0x40, 0x40];

/// The layout section and its keys.
// EVIDENCE: cshell+0x2ac5c0 (section literal "ClanMark" and the keys below)
pub mod keys {
    use super::{key, KeyPin};
    pub use crate::menu::colorbox::keys::{NEXT_POS, NUM_POS, PAGE_GAP, PREV_POS};
    pub const SECTION: KeyPin = key("61ae1a37629e8e1f", 8);
    pub const MAIN_TEX: KeyPin = key("5d1fa97b9265ad6f", 11);
    pub const MAIN_SIZE: KeyPin = key("b093d3ac77dafbc7", 15);
    pub const TITLE_POS: KeyPin = key("00e9b913fb078c5e", 11);
    pub const SELECT_POS: KeyPin = key("504450455e177b65", 12);
    pub const IMG_TITLE_POS: KeyPin = key("e8f4a97d76cbaac1", 14);
    pub const NOW_POS: KeyPin = key("87c02aad8fd2bb90", 10);
    pub const VIEW_POS: KeyPin = key("87179a34959f4373", 11);
    pub const NEXT_BUT_POS: KeyPin = key("25d1227ad2917892", 10);
    pub const CANCEL_BUT_POS: KeyPin = key("006f2c1cf4035134", 12);
    pub const BACK_BUT_POS: KeyPin = key("7aabf56b198a530b", 10);
    pub const OK_BUT_POS: KeyPin = key("594cdb53cfa3eaed", 8);
    pub const CHECK_BUT_POS: KeyPin = key("3e4320b96ac5a75a", 8);
    pub const LIST_POS: KeyPin = key("f5844c7adbc3bf4d", 10);
    pub const CHILD_POS: KeyPin = key("11b370b668ff9c10", 11);
    pub const TAB_POS: KeyPin = key("7c06a1f1e16a1a17", 9);
    pub const TAB_NORMAL: KeyPin = key("abf37880d090ff28", 16);
    pub const TAB_SELECT: KeyPin = key("91c8105f4f73e428", 16);
    pub const TAB_SIZE: KeyPin = key("373ebbc7ea496147", 14);
    pub const MSG_TEX: KeyPin = key("b73829d79db30a2f", 10);
    pub const MSG_SIZE: KeyPin = key("a2e48c3c15f8c27a", 14);
    pub const MSG_STR_POS: KeyPin = key("e7b19ac9f894136e", 9);
    pub const MSG_MARK_POS: KeyPin = key("7eccd90b952c933f", 10);
    pub const MSG_BACK_POS: KeyPin = key("210d461362d344ee", 13);
    pub const MSG_OK_POS: KeyPin = key("4d6c581afaa0e24b", 11);
    pub const SLOT_OFF: KeyPin = key("e2db10288009b59d", 18);
    pub const SLOT_ON: KeyPin = key("6b5f6ea49bae0c4a", 17);
    pub const SLOT_SIZE: KeyPin = key("bb2914b94a2728a1", 16);
    pub const ROWS: KeyPin = key("4501f63a0c30091b", 18);
    pub const COLUMNS: KeyPin = key("220428c148c2dc36", 18);
    pub const ROW_GAP: KeyPin = key("e393dfbecabafe49", 23);
    pub const COLUMN_GAP: KeyPin = key("6523a71def1c2f10", 23);
}

/// The window's places (offsets inside the window; the confirmation's inside its frame) and pictures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkLayout {
    pub window_tex: String,
    pub window_size: [i32; 2],
    pub title_at: [i32; 2],
    pub select_at: [i32; 2],
    pub img_title_at: [i32; 2],
    pub now_at: [i32; 2],
    pub view_at: [i32; 2],
    pub next_at: [i32; 2],
    pub cancel_at: [i32; 2],
    pub back_at: [i32; 2],
    pub ok_at: [i32; 2],
    pub check_at: [i32; 2],
    pub list_at: [i32; 2],
    pub child_at: [i32; 2],
    pub tab_at: [i32; 2],
    pub tab_normal: String,
    pub tab_select: String,
    pub tab_size: [i32; 2],
    pub prev_at: [i32; 2],
    pub num_at: [i32; 2],
    pub next_page_at: [i32; 2],
    pub page_gap: i32,
    pub slot_off: String,
    pub slot_on: String,
    pub slot_size: [i32; 2],
    pub rows: i32,
    pub columns: i32,
    pub row_gap: i32,
    pub column_gap: i32,
    pub msg_tex: String,
    pub msg_size: [i32; 2],
    pub msg_str_at: [i32; 2],
    pub msg_mark_at: [i32; 2],
    pub msg_back_at: [i32; 2],
    pub msg_ok_at: [i32; 2],
}

/// DC-DEFAULT: CLI-1884 (proposed) our fallback when the ROM's layout section is missing: a 600 x 400 window on a
/// simple grid of our own round numbers, no pictures (the real places are read from the ROM first).
impl Default for MarkLayout {
    fn default() -> Self {
        MarkLayout {
            window_tex: String::new(),
            window_size: [600, 400],
            title_at: [10, 10],
            select_at: [40, 110],
            img_title_at: [40, 40],
            now_at: [420, 40],
            view_at: [500, 40],
            next_at: [320, 370],
            cancel_at: [160, 370],
            back_at: [100, 370],
            ok_at: [240, 370],
            check_at: [380, 370],
            list_at: [40, 170],
            child_at: [40, 140],
            tab_at: [0, 0],
            tab_normal: String::new(),
            tab_select: String::new(),
            tab_size: [60, 20],
            prev_at: [200, 345],
            num_at: [230, 345],
            next_page_at: [360, 345],
            page_gap: 20,
            slot_off: String::new(),
            slot_on: String::new(),
            slot_size: [50, 50],
            rows: 3,
            columns: 8,
            row_gap: 10,
            column_gap: 10,
            msg_tex: String::new(),
            msg_size: [500, 300],
            msg_str_at: [30, 60],
            msg_mark_at: [400, 100],
            msg_back_at: [100, 240],
            msg_ok_at: [260, 240],
        }
    }
}

impl MarkLayout {
    pub fn read(cat: &Catalog) -> MarkLayout {
        let d = MarkLayout::default();
        let v = |k: &KeyPin| cat.section_value(&keys::SECTION, k);
        let p = |k: &KeyPin, or: [i32; 2]| pair(v(k)).unwrap_or(or);
        let n = |k: &KeyPin, or: i32| int(v(k)).unwrap_or(or);
        let t = |k: &KeyPin, or: String| v(k).map_or(or, |s| s.trim().to_string());
        MarkLayout {
            window_tex: t(&keys::MAIN_TEX, d.window_tex),
            window_size: p(&keys::MAIN_SIZE, d.window_size),
            title_at: p(&keys::TITLE_POS, d.title_at),
            select_at: p(&keys::SELECT_POS, d.select_at),
            img_title_at: p(&keys::IMG_TITLE_POS, d.img_title_at),
            now_at: p(&keys::NOW_POS, d.now_at),
            view_at: p(&keys::VIEW_POS, d.view_at),
            next_at: p(&keys::NEXT_BUT_POS, d.next_at),
            cancel_at: p(&keys::CANCEL_BUT_POS, d.cancel_at),
            back_at: p(&keys::BACK_BUT_POS, d.back_at),
            ok_at: p(&keys::OK_BUT_POS, d.ok_at),
            check_at: p(&keys::CHECK_BUT_POS, d.check_at),
            list_at: p(&keys::LIST_POS, d.list_at),
            child_at: p(&keys::CHILD_POS, d.child_at),
            tab_at: p(&keys::TAB_POS, d.tab_at),
            tab_normal: t(&keys::TAB_NORMAL, d.tab_normal),
            tab_select: t(&keys::TAB_SELECT, d.tab_select),
            tab_size: p(&keys::TAB_SIZE, d.tab_size),
            prev_at: p(&keys::PREV_POS, d.prev_at),
            num_at: p(&keys::NUM_POS, d.num_at),
            next_page_at: p(&keys::NEXT_POS, d.next_page_at),
            page_gap: n(&keys::PAGE_GAP, d.page_gap),
            slot_off: t(&keys::SLOT_OFF, d.slot_off),
            slot_on: t(&keys::SLOT_ON, d.slot_on),
            slot_size: p(&keys::SLOT_SIZE, d.slot_size),
            rows: n(&keys::ROWS, d.rows).max(1),
            columns: n(&keys::COLUMNS, d.columns).max(1),
            row_gap: n(&keys::ROW_GAP, d.row_gap),
            column_gap: n(&keys::COLUMN_GAP, d.column_gap),
            msg_tex: t(&keys::MSG_TEX, d.msg_tex),
            msg_size: p(&keys::MSG_SIZE, d.msg_size),
            msg_str_at: p(&keys::MSG_STR_POS, d.msg_str_at),
            msg_mark_at: p(&keys::MSG_MARK_POS, d.msg_mark_at),
            msg_back_at: p(&keys::MSG_BACK_POS, d.msg_back_at),
            msg_ok_at: p(&keys::MSG_OK_POS, d.msg_ok_at),
        }
    }

    /// The window's corner: centred on the 1024 x 768 screen (the confirmation likewise).
    // EVIDENCE: cshell+0x2ac5c0 (the window at ((0x400 - w) / 2, (0x300 - h) / 2); the confirmation likewise)
    pub fn window_at(&self) -> [i32; 2] {
        centred(self.window_size)
    }

    pub fn msg_at(&self) -> [i32; 2] {
        centred(self.msg_size)
    }
}

fn centred(size: [i32; 2]) -> [i32; 2] {
    [(0x400 - size[0]) / 2, (0x300 - size[1]) / 2]
}

/// The empty mark frame shown without a clan mark.
// DISASM: cshell+0x2acca5 (the MarkNowPos image VA 0x374f91b8)
pub const FRAME_TEX: Pin = Pin {
    what: "clan mark window empty frame",
    rva: 0x3f91b8,
    sha16: "0c3ac30b06f810fb",
    len: 25,
};

/// The empty frame picture (`None` without the ROM).
pub fn frame_art(rom: Option<&Path>) -> Option<String> {
    let pe =
        dc_rom::PeFile::from_file(&rom?.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1])).ok()?;
    read_pinned(&pe, &FRAME_TEX).ok()
}

/// What a press or a key did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkStep {
    None,
    /// Ask the lobby whether the clan may take the pair (0x34c).
    Check {
        emblem: u32,
        background: u32,
    },
    /// Use the item with the pair (0x330).
    Use {
        emblem: u32,
        background: u32,
    },
    /// A box with text `text` titled 0x346 (the window stays).
    Notice(i32),
    /// Cancel: the window closes with the cancelled box.
    Cancel,
}

/// The open clan mark window.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkBox {
    pub uid: u32,
    /// 0: the emblem step, 1: the background step.
    pub step: usize,
    pub tab: usize,
    /// The page shown (0-based).
    pub page: usize,
    /// The picked emblem and background (indices of the mark table, 0 = none).
    pub emblem: u32,
    pub background: u32,
    /// The own clan's mark now (0 = none).
    pub own: (u32, u32),
    /// An ok check enabled OK (a new background pick disables it again).
    pub checked: bool,
    pub confirm: bool,
    pub waiting: bool,
    pub marks: ClanMarks,
    pub frame: Option<String>,
    pub layout: MarkLayout,
}

const TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};
const MID: Align = Align {
    first: ALIGN_CENTRE,
    second: ALIGN_SECOND_CENTRE,
};

impl MarkBox {
    /// The window for item `uid`; `own` the clan's mark now.
    // EVIDENCE: cshell+0x2ae340 (open: the clan's mark from shell +0x428 / +0x42c shown at MarkNowPos when both are
    // >= 1, else the empty frame; the picked mark cleared; the emblem step), cshell+0x2ae130 (a step starts on tab 0),
    // cshell+0x2ac1b0 (a tab starts on its first page)
    pub fn open(
        cat: &Catalog,
        uid: u32,
        own: (u32, u32),
        marks: ClanMarks,
        frame: Option<String>,
    ) -> MarkBox {
        MarkBox {
            uid,
            step: 0,
            tab: 0,
            page: 0,
            emblem: 0,
            background: 0,
            own,
            checked: false,
            confirm: false,
            waiting: false,
            marks,
            frame,
            layout: MarkLayout::read(cat),
        }
    }

    fn abs(&self, at: [i32; 2]) -> [i32; 2] {
        let w = self.layout.window_at();
        [w[0] + at[0], w[1] + at[1]]
    }

    fn m_abs(&self, at: [i32; 2]) -> [i32; 2] {
        let w = self.layout.msg_at();
        [w[0] + at[0], w[1] + at[1]]
    }

    /// The table indices on the step's tab: tab 0 every entry of a category 1..6, tab k the entries of category k.
    // EVIDENCE: cshell+0x280a90 (category -1: no entry; 0: the look-up list only; k >= 1: tab 0 and tab k, in index
    // order), cshell+0x2ac1b0 (the tab's list)
    pub fn entries(&self) -> Vec<u32> {
        let cats = if self.step == 0 {
            &self.marks.emblem_cats
        } else {
            &self.marks.background_cats
        };
        cats.iter()
            .enumerate()
            .filter(|&(_, &c)| {
                if self.tab == 0 {
                    c >= 1
                } else {
                    c == self.tab as i32
                }
            })
            .map(|(i, _)| i as u32)
            .collect()
    }

    /// Slots on a page.
    // EVIDENCE: cshell+0x2ac060 (a page holds 0x18 entries: MarkListSlotVerNum x MarkListSlotHorNum)
    pub fn per_page(&self) -> usize {
        (self.layout.rows * self.layout.columns).max(1) as usize
    }

    pub fn pages(&self) -> usize {
        self.entries().len().div_ceil(self.per_page()).max(1)
    }

    /// Slot `k` of the page: its corner.
    pub fn slot_at(&self, k: usize) -> [i32; 2] {
        let l = &self.layout;
        let (row, col) = ((k as i32) / l.columns, (k as i32) % l.columns);
        self.abs([
            l.list_at[0] + col * (l.slot_size[0] + l.column_gap),
            l.list_at[1] + row * (l.slot_size[1] + l.row_gap),
        ])
    }

    /// Tab `k`'s corner.
    // EVIDENCE: cshell+0x2ac5c0 (the tab panel at ChildWndPos, its tabs from ChkTabPos every TabTextureSize width)
    pub fn tab_rect(&self, k: usize) -> [i32; 2] {
        let l = &self.layout;
        self.abs([
            l.child_at[0] + l.tab_at[0] + k as i32 * l.tab_size[0],
            l.child_at[1] + l.tab_at[1],
        ])
    }

    /// The entries on the page shown.
    fn shown(&self) -> Vec<u32> {
        let all = self.entries();
        let first = (self.page * self.per_page()).min(all.len());
        let last = (first + self.per_page()).min(all.len());
        all[first..last].to_vec()
    }

    fn page_link_at(&self, k: usize) -> [i32; 2] {
        let l = &self.layout;
        self.abs([l.num_at[0] + k as i32 * l.page_gap, l.num_at[1]])
    }

    /// The first page of the group of links shown.
    // EVIDENCE: cshell+0x2ac060 (five links from the group's first page; a link past the last page disabled; `<`
    // disabled on the first group, `>` disabled when the last page is in the group), cshell+0x2ae470 (0x25c / 0x25d
    // move the group by five pages)
    fn group(&self) -> usize {
        self.page / PAGE_LINKS * PAGE_LINKS
    }

    /// The buttons of the step: (command, place, caption, enabled).
    // EVIDENCE: cshell+0x2ae130 (step 1: Cancel and Next; step 2: Back, OK (disabled) and Check), cshell+0x2adf70 (a
    // background pick enables Check and disables OK), cshell+0x2ab3a0 (an ok check enables OK and disables Check)
    pub fn buttons(&self) -> Vec<(i32, [i32; 2], i32, bool)> {
        let l = &self.layout;
        let free = !self.waiting;
        if self.step == 0 {
            vec![
                (CMD_CANCEL, l.cancel_at, CANCEL_TEXT, free),
                (CMD_NEXT, l.next_at, NEXT_TEXT, free),
            ]
        } else {
            vec![
                (CMD_BACK, l.back_at, BACK_TEXT, free),
                (CMD_OK, l.ok_at, NEXT_TEXT, free && self.checked),
                (CMD_CHECK, l.check_at, CHECK_TEXT, free && !self.checked),
            ]
        }
    }

    /// Picks table index `i` for the step.
    // EVIDENCE: cshell+0x2adf70 (a slot press: step 1 the emblem, step 2 the background (Check on, OK off), the
    // picked mark's layer set)
    pub fn pick(&mut self, i: u32) {
        if self.waiting {
            return;
        }
        if self.step == 0 {
            self.emblem = i;
        } else {
            self.background = i;
            self.checked = false;
        }
    }

    /// The step's tab `k`.
    pub fn choose_tab(&mut self, k: usize) {
        if k < TABS {
            self.tab = k;
            self.page = 0;
        }
    }

    /// Step `s`: the emblem step clears the background.
    // EVIDENCE: cshell+0x2ae130 (step 1: the background pick and its layer cleared; either step on tab 0)
    fn go_step(&mut self, s: usize) {
        self.step = s;
        self.tab = 0;
        self.page = 0;
        self.checked = false;
        if s == 0 {
            self.background = 0;
        }
    }

    /// A command of the window.
    // EVIDENCE: cshell+0x2ae470 (0x25f: an emblem -> step 2, else 0x179b; 0x260: step 1; 0x261: Cancel; 0x266: no
    // background -> 0x179a, the clan's own pair -> 0x17f4, else the check; 0x25e: with a background the confirmation;
    // 0x262: the send; 0x263: the confirmation hidden)
    pub fn command(&mut self, cmd: i32) -> MarkStep {
        if self.waiting {
            return MarkStep::None;
        }
        match cmd {
            CMD_NEXT if self.emblem == 0 => MarkStep::Notice(NO_EMBLEM_TEXT),
            CMD_NEXT => {
                self.go_step(1);
                MarkStep::None
            }
            CMD_BACK => {
                self.go_step(0);
                MarkStep::None
            }
            CMD_CANCEL => MarkStep::Cancel,
            CMD_CHECK if self.background == 0 => MarkStep::Notice(NO_BACKGROUND_TEXT),
            CMD_CHECK if (self.emblem, self.background) == self.own => {
                MarkStep::Notice(OWN_MARK_TEXT)
            }
            CMD_CHECK => {
                self.waiting = true;
                MarkStep::Check {
                    emblem: self.emblem,
                    background: self.background,
                }
            }
            CMD_OK if self.background > 0 && self.checked => {
                self.confirm = true;
                MarkStep::None
            }
            CMD_SEND if self.confirm => {
                self.confirm = false;
                self.waiting = true;
                MarkStep::Use {
                    emblem: self.emblem,
                    background: self.background,
                }
            }
            CMD_MSG_BACK => {
                self.confirm = false;
                MarkStep::None
            }
            _ => MarkStep::None,
        }
    }

    /// The check's answer (0x34d): ok enables OK.
    // EVIDENCE: cshell+0x2ab3a0 (ok: box 0x1804 titled 0x1812, OK on, Check off; refused: the reason box)
    pub fn checked_reply(&mut self, ok: bool) {
        self.waiting = false;
        self.checked = ok;
    }

    /// A refused use: the window stays.
    pub fn refused(&mut self) {
        self.waiting = false;
    }

    /// Enter: Next / Check / OK of the step, the confirmation's OK.
    // DC-DEFAULT: CLI-1884 (proposed) Enter runs the default button of what is up (Next, Check before a check, OK
    // after it, the confirmation's OK); Escape runs the confirmation's Back, then Back, then Cancel
    pub fn enter(&mut self) -> MarkStep {
        if self.confirm {
            return self.command(CMD_SEND);
        }
        match (self.step, self.checked) {
            (0, _) => self.command(CMD_NEXT),
            (_, false) => self.command(CMD_CHECK),
            _ => self.command(CMD_OK),
        }
    }

    pub fn escape(&mut self) -> MarkStep {
        if self.confirm {
            return self.command(CMD_MSG_BACK);
        }
        if self.step == 1 {
            return self.command(CMD_BACK);
        }
        self.command(CMD_CANCEL)
    }

    /// A press at `p`; `None` outside the window.
    pub fn press(&mut self, p: [f32; 2]) -> Option<MarkStep> {
        let l = self.layout.clone();
        if self.confirm {
            if !inside(l.msg_at(), l.msg_size, p) {
                return None;
            }
            if inside(self.m_abs(l.msg_ok_at), BUTTON_SIZE, p) {
                return Some(self.command(CMD_SEND));
            }
            if inside(self.m_abs(l.msg_back_at), BUTTON_SIZE, p) {
                return Some(self.command(CMD_MSG_BACK));
            }
            return Some(MarkStep::None);
        }
        if !inside(l.window_at(), l.window_size, p) {
            return None;
        }
        for (cmd, at, _, on) in self.buttons() {
            if on && inside(self.abs(at), BUTTON_SIZE, p) {
                return Some(self.command(cmd));
            }
        }
        for k in 0..TABS {
            if inside(self.tab_rect(k), l.tab_size, p) {
                self.choose_tab(k);
                return Some(MarkStep::None);
            }
        }
        for (k, i) in self.shown().into_iter().enumerate() {
            if inside(self.slot_at(k), l.slot_size, p) {
                self.pick(i);
                return Some(MarkStep::None);
            }
        }
        // DC-DEFAULT: CLI-1884 (proposed) a page link takes a press in a 20 x 20 square at its point (as CLI-1876)
        let link = [PAGE_SIZE, PAGE_SIZE];
        let first = self.group();
        for k in 0..PAGE_LINKS {
            if first + k < self.pages() && inside(self.page_link_at(k), link, p) {
                self.page = first + k;
                return Some(MarkStep::None);
            }
        }
        if first > 0 && inside(self.abs(l.prev_at), link, p) {
            self.page = first - PAGE_LINKS;
        } else if first + PAGE_LINKS < self.pages() && inside(self.abs(l.next_page_at), link, p) {
            self.page = first + PAGE_LINKS;
        }
        Some(MarkStep::None)
    }

    /// The picture of table index `i` of the step (`None` without one).
    fn picture(&self, step: usize, i: u32) -> Option<&str> {
        let list = if step == 0 {
            &self.marks.emblems
        } else {
            &self.marks.backgrounds
        };
        list.get(i as usize).and_then(|p| p.as_deref())
    }

    pub fn draw(&self, cat: &mut Catalog, buttons: &NameArt, out: &mut Vec<DrawItem>) {
        let l = &self.layout;
        let text = |s: &str, at: [i32; 2], align: Align, colour: u32| {
            place_text(s, at, [0, 0], align, FONT, FONT_SIZE, colour)
        };
        let w = l.window_at();
        if l.window_tex.is_empty() {
            out.push(fill(w, l.window_size, [24, 24, 24, 240]));
        } else {
            out.push(image_fit(cat, &l.window_tex, w, l.window_size));
        }
        out.push(text(
            &cat.text(TITLE_TEXT),
            self.abs(l.title_at),
            TL,
            TITLE_COLOUR,
        ));
        let [select, img_title] = STEP_TEXTS[self.step];
        out.push(text(
            &cat.text(select),
            self.abs(l.select_at),
            TL,
            SELECT_COLOUR,
        ));
        let long = cat.text(img_title);
        let at = self.abs(l.img_title_at);
        for (k, line) in wrap_lines(&long, IMG_TITLE_WIDTH as f32, |t| measure(cat, t) as f32)
            .iter()
            .enumerate()
        {
            out.push(text(
                line,
                [at[0], at[1] + k as i32 * (FONT_SIZE + 2)],
                TL,
                WHITE,
            ));
        }
        // the tabs of the step
        // DC-DEFAULT: CLI-1884 (proposed) the tab captions in white
        for (k, &caption) in TAB_TEXTS[self.step].iter().enumerate() {
            let at = self.tab_rect(k);
            let tex = if k == self.tab {
                &l.tab_select
            } else {
                &l.tab_normal
            };
            if tex.is_empty() {
                let c = if k == self.tab { 70 } else { 40 };
                out.push(fill(at, l.tab_size, [c, c, c, 255]));
            } else {
                out.push(image_fit(cat, tex, at, l.tab_size));
            }
            out.push(text(
                &cat.text(caption),
                [at[0] + l.tab_size[0] / 2, at[1] + l.tab_size[1] / 2],
                MID,
                WHITE,
            ));
        }
        // the slots: the off picture, the entry's art (the 51 x 51 corner of its texture) inside it, the on picture on
        // the picked entry
        // DC-DEFAULT: CLI-1884 (proposed) a slot shows its own layer only, its art 1 pixel in from the slot's corner;
        // the on picture marks the picked entry (hover is not tracked)
        let picked = if self.step == 0 {
            self.emblem
        } else {
            self.background
        };
        for (k, i) in self.shown().into_iter().enumerate() {
            let at = self.slot_at(k);
            if !l.slot_off.is_empty() {
                out.push(image(&l.slot_off, at, l.slot_size));
            }
            let r = [at[0] + 1, at[1] + 1, l.slot_size[0] - 2, l.slot_size[1] - 2];
            let pic = self.picture(self.step, i);
            if self.step == 0 {
                super::clanpage::mark_cropped(None, pic, r, out);
            } else {
                super::clanpage::mark_cropped(pic, None, r, out);
            }
            if i == picked && !l.slot_on.is_empty() {
                out.push(image(&l.slot_on, at, l.slot_size));
            }
        }
        // the page links
        let first = self.group();
        let last = self.pages() - 1;
        let off = |on: bool| if on { PAGE_COLOUR } else { PAGE_OFF_COLOUR };
        let small =
            |s: &str, at: [i32; 2], c: u32| place_text(s, at, [0, 0], TL, FONT, PAGE_SIZE, c);
        out.push(small("<", self.abs(l.prev_at), off(first > 0)));
        for k in 0..PAGE_LINKS {
            let n = first + k;
            let c = if n == self.page {
                WHITE
            } else {
                off(n <= last)
            };
            out.push(small(&(n + 1).to_string(), self.page_link_at(k), c));
        }
        out.push(small(
            ">",
            self.abs(l.next_page_at),
            off(first + PAGE_LINKS <= last),
        ));
        // the clan's mark now and the picked mark
        let now = self.abs(l.now_at);
        let now_r = [now[0], now[1], MARK_SIZE[0], MARK_SIZE[1]];
        match self.marks.textures(self.own.0, self.own.1) {
            Some((e, b)) => super::clanpage::mark(cat, Some(b), Some(e), now_r, out),
            None => {
                if let Some(f) = self.frame.as_deref() {
                    out.push(image_fit(cat, f, now, MARK_SIZE));
                }
            }
        }
        let view = self.abs(l.view_at);
        super::clanpage::mark(
            cat,
            self.picture(1, self.background),
            self.picture(0, self.emblem),
            [view[0], view[1], MARK_SIZE[0], MARK_SIZE[1]],
            out,
        );
        for (_, at, caption, on) in self.buttons() {
            draw_button(cat, buttons, self.abs(at), caption, on, out);
        }
        if self.confirm {
            self.draw_confirm(cat, buttons, out);
        }
    }

    // EVIDENCE: cshell+0x2ac5c0 (the confirmation: MsgTexture centred, 0x17e1 at (0xd, 6), 0x17d0 at (0x1e, MsgStrPos
    // y) 0x14a wide, the mark at MsgMarkPos, Back 0x3e and OK 0x17cf), cshell+0x2ae470 (0x25e: both layers of the
    // picked mark set, the confirmation shown)
    fn draw_confirm(&self, cat: &mut Catalog, buttons: &NameArt, out: &mut Vec<DrawItem>) {
        let l = &self.layout;
        let at = l.msg_at();
        if l.msg_tex.is_empty() {
            out.push(fill(at, l.msg_size, [24, 24, 24, 245]));
        } else {
            out.push(image_fit(cat, &l.msg_tex, at, l.msg_size));
        }
        out.push(place_text(
            &cat.text(MSG_TITLE),
            self.m_abs(MSG_TITLE_AT),
            [0, 0],
            TL,
            FONT,
            FONT_SIZE,
            WHITE,
        ));
        let notice = cat.text(MSG_TEXT);
        let lines = wrap_lines(&notice, MSG_TEXT_WIDTH as f32, |t| measure(cat, t) as f32);
        for (k, line) in lines.iter().enumerate() {
            let y = l.msg_str_at[1] + k as i32 * (FONT_SIZE + 2);
            out.push(place_text(
                line,
                self.m_abs([MSG_TEXT_AT[0], y]),
                [0, 0],
                TL,
                FONT,
                FONT_SIZE,
                WHITE,
            ));
        }
        let m = self.m_abs(l.msg_mark_at);
        super::clanpage::mark(
            cat,
            self.picture(1, self.background),
            self.picture(0, self.emblem),
            [m[0], m[1], MARK_SIZE[0], MARK_SIZE[1]],
            out,
        );
        let free = !self.waiting;
        draw_button(
            cat,
            buttons,
            self.m_abs(l.msg_back_at),
            BACK_TEXT,
            free,
            out,
        );
        draw_button(cat, buttons, self.m_abs(l.msg_ok_at), SEND_TEXT, free, out);
    }
}

/// A box of the window: `text` titled 0x346 with OK.
pub fn notice_box(cat: &Catalog, text: i32) -> Popup {
    let mut q = Popup::ok(cat.text(text), BoxTag::None);
    q.title = text_ids::TITLE_ERROR;
    q
}

/// The ok check's box.
pub fn checked_box(cat: &Catalog) -> Popup {
    let mut q = Popup::ok(cat.text(CHECKED_TEXT), BoxTag::None);
    q.title = CHECKED_TITLE;
    q
}

/// The ok use's box.
pub fn done_box(cat: &Catalog) -> Popup {
    let mut q = Popup::ok(cat.text(DONE_TEXT), BoxTag::None);
    q.title = DONE_TITLE;
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic table: emblems 1..=40 in categories 1 / 2 alternating (index 7 category 0, 9 none), backgrounds
    /// 1..=5 in category 3.
    fn marks() -> ClanMarks {
        let mut m = ClanMarks::default();
        for i in 0..=40u32 {
            let cat = match i {
                0 | 9 => -1,
                7 => 0,
                _ => 1 + (i % 2) as i32,
            };
            m.emblems.push((cat >= 0).then(|| format!("e{i}.dtx")));
            m.emblem_cats.push(cat);
        }
        for i in 0..=5u32 {
            m.backgrounds.push((i > 0).then(|| format!("b{i}.dtx")));
            m.background_cats.push(if i > 0 { 3 } else { -1 });
        }
        m
    }

    fn boxed(own: (u32, u32)) -> MarkBox {
        let c = Catalog::fallback(None, 1024, 768);
        MarkBox::open(&c, 50, own, marks(), Some("frame.dtx".into()))
    }

    fn mid(at: [i32; 2]) -> [f32; 2] {
        [(at[0] + 5) as f32, (at[1] + 5) as f32]
    }

    #[test]
    fn tabs_list_the_categories_and_tab_zero_lists_every_shown_one() {
        let mut b = boxed((0, 0));
        // 40 entries less the category 0 and the empty one
        assert_eq!(b.entries().len(), 38);
        assert!(!b.entries().contains(&7) && !b.entries().contains(&9));
        assert_eq!((b.per_page(), b.pages()), (24, 2));
        b.press(mid(b.tab_rect(2)));
        assert_eq!(b.tab, 2);
        assert!(b.entries().iter().all(|i| i % 2 == 1));
        assert_eq!(b.entries().len(), 18);
        // the background step lists its own table
        b.emblem = 3;
        b.command(CMD_NEXT);
        assert_eq!((b.step, b.tab, b.entries()), (1, 0, vec![1, 2, 3, 4, 5]));
    }

    #[test]
    fn the_steps_check_and_confirm_before_the_send() {
        let mut b = boxed((4, 2));
        assert_eq!(b.layout.window_at(), [212, 184]);
        // Next without an emblem shows the box
        let next = mid(b.abs(b.layout.next_at));
        assert_eq!(b.press(next), Some(MarkStep::Notice(NO_EMBLEM_TEXT)));
        let slot = mid(b.slot_at(3));
        b.press(slot);
        assert_eq!(b.emblem, 4);
        assert_eq!(b.press(next), Some(MarkStep::None));
        assert_eq!(b.step, 1);
        // Check without a background, then with the clan's own pair, then a new pair
        let check = mid(b.abs(b.layout.check_at));
        assert_eq!(b.press(check), Some(MarkStep::Notice(NO_BACKGROUND_TEXT)));
        b.pick(2);
        assert_eq!(b.press(check), Some(MarkStep::Notice(OWN_MARK_TEXT)));
        b.pick(5);
        assert_eq!(
            b.press(check),
            Some(MarkStep::Check {
                emblem: 4,
                background: 5
            })
        );
        assert!(b.waiting);
        b.checked_reply(true);
        // OK shows the confirmation; its Back hides it; its OK sends
        let ok = mid(b.abs(b.layout.ok_at));
        assert_eq!(b.press(ok), Some(MarkStep::None));
        assert!(b.confirm);
        assert_eq!(
            b.press(mid(b.m_abs(b.layout.msg_back_at))),
            Some(MarkStep::None)
        );
        assert!(!b.confirm);
        b.press(ok);
        assert_eq!(
            b.press(mid(b.m_abs(b.layout.msg_ok_at))),
            Some(MarkStep::Use {
                emblem: 4,
                background: 5
            })
        );
        assert!(b.waiting);
        b.refused();
        // a new background pick needs a new check; Back clears the background
        b.pick(1);
        assert!(!b.checked);
        assert_eq!(b.command(CMD_OK), MarkStep::None);
        assert!(!b.confirm);
        b.escape();
        assert_eq!((b.step, b.background, b.emblem), (0, 0, 4));
        assert_eq!(b.escape(), MarkStep::Cancel);
        assert_eq!(b.press([0.0, 0.0]), None);
    }

    #[test]
    fn page_links_move_within_the_tab() {
        let mut b = boxed((0, 0));
        b.press(mid(b.page_link_at(1)));
        assert_eq!(b.page, 1);
        assert_eq!(b.shown().len(), 38 - 24);
        // a link past the last page does nothing
        b.press(mid(b.page_link_at(3)));
        assert_eq!(b.page, 1);
        b.choose_tab(1);
        assert_eq!(b.page, 0);
    }

    #[test]
    fn the_draw_shows_the_own_mark_or_the_frame_and_the_pick() {
        let mut c = Catalog::fallback(None, 1024, 768);
        let tex = |out: &[DrawItem]| -> Vec<String> {
            out.iter()
                .filter_map(|d| match d {
                    DrawItem::Image { tex, .. } => Some(tex.clone()),
                    _ => None,
                })
                .collect()
        };
        let b = boxed((0, 0));
        let mut out = Vec::new();
        b.draw(&mut c, &NameArt::default(), &mut out);
        let t = tex(&out);
        assert!(t.contains(&"frame.dtx".to_string()));
        assert_eq!(t.iter().filter(|s| s.starts_with('e')).count(), 24);
        let mut b = boxed((3, 2));
        b.pick(5);
        let mut out = Vec::new();
        b.draw(&mut c, &NameArt::default(), &mut out);
        let t = tex(&out);
        assert!(!t.contains(&"frame.dtx".to_string()));
        assert!(t.contains(&"e3.dtx".to_string()) && t.contains(&"b2.dtx".to_string()));
        // the picked emblem appears in its slot and in the preview
        assert_eq!(t.iter().filter(|s| *s == "e5.dtx").count(), 2);
    }
}
