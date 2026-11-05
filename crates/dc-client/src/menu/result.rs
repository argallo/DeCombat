//! Result screen (screen 7): opens after the match when the client left the game server (its focus is the moment
//! of the lobby location 0xb, [`crate::matchexit`]). Team 1's list on the left, team 2's on the right (name,
//! kills, deaths, mission, experience per player), the team scores above them, the own experience and GP
//! below; the exit button, or the screen itself 60 s later, goes back to the waiting room.
//!
//! Everything it draws is the ROM's result page (looked up by the name the 2009 build loads) and the background
//! tiles named in CShell's code; the data are the match's ([`crate::matchexit::MatchResult`]).
// DISASM: cshell+0x132350 (build: page by name, background tiler 0, 0, 3, 4; window 0 lists 0x19 and 0x1a),
// cshell+0x137dd0 (focus: windows 2 / 3 by the clan flag, the lobby location 0xb), cshell+0x132b80 (window 1 for
// mode class 5, window 0 otherwise), cshell+0x1343b0 (60000 ms after the rows were filled: command 6),
// cshell+0x132400 (command 6 -> waiting room, 0x3b -> room leave 0x386)
// OBSERVED: the result page of the user's ROM read at run time (window 0: mark image, score digits, header texts,
// two lists; window 2: own summary values; the exit button with command 6)

use super::catalog::Catalog;
use super::drawlist::{anchor_of, DynNotes, DynWant};
use super::link::{LinkCmd, LinkEvent, ProgressApplied};
use super::popup::{BoxTag, Popup};
use super::sheet::{
    list_row, place_text, Look, Part, Sheet, Tiles, ALIGN_CENTRE, ALIGN_LEFT, ALIGN_RIGHT,
};
use super::{pages, Ctx, Go, MenuData, MenuKey, Screen};
use crate::hud::board::LevelTable;
use crate::hud::items::{markup_segments, DrawItem};
use crate::matchexit::{
    level_progress_text, MatchResult, ResultRow, RESULT_AUTO_EXIT_MS, RESULT_CMD_EXIT,
    RESULT_CMD_LEAVE_ROOM,
};
use dc_ui_data::names::{matches_pin, read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::{PageTag, UiScreen, VirtualScaler, WidgetKind};
use std::path::Path;
use std::time::{Duration, Instant};

/// Windows of the result page.
pub const WINDOW_TEAMS: i32 = 0;
pub const WINDOW_COOP: i32 = 1;
pub const WINDOW_SUMMARY: i32 = 2;
pub const WINDOW_CLAN_SUMMARY: i32 = 3;
pub const WINDOW_LEVEL_UP: i32 = 4;
/// Screen-level controls: the abused-game notice image, the exit button.
pub const ABUSED_IMAGE: i32 = 5;
/// The two lists of window 0 (left, right).
// EVIDENCE: cshell+0x132350 (window 0 controls 0x19 and 0x1a)
pub const LIST_LEFT: i32 = 0x19;
pub const LIST_RIGHT: i32 = 0x1a;
/// Window 0: the mark image over the lists.
pub const MARK_IMAGE: i32 = 0;
/// Window 0: score digit images (hundreds, tens, ones) of the left and the right team; mode, goal and map images.
pub const SCORE_LEFT: [i32; 3] = [1, 2, 3];
pub const SCORE_RIGHT: [i32; 3] = [4, 5, 6];
pub const MODE_IMAGES: [i32; 4] = [7, 8, 9, 10];
/// Window 0: the header texts whose places are the columns (name, kills, deaths, mission, experience) of the left
/// and the right list.
pub const COLUMNS_LEFT: [i32; 5] = [12, 14, 15, 16, 17];
pub const COLUMNS_RIGHT: [i32; 5] = [19, 21, 22, 23, 24];
/// Window 2: the own rank icon and level text, the ID, experience and GP values, the bonus marks.
pub const SUMMARY_RANK_ICON: i32 = 0;
pub const SUMMARY_LEVEL: i32 = 9;
pub const SUMMARY_ID: i32 = 10;
pub const SUMMARY_EXP: i32 = 11;
pub const SUMMARY_GP: i32 = 12;
pub const SUMMARY_MARKS: [i32; 4] = [1, 2, 3, 4];
/// Window 4 (level up): the digit images of the new level's bonus (thousands, hundreds, tens, ones), the level's
/// icon and its text.
// EVIDENCE: cshell+0x136e40 (window 4 controls 2..5 take `<digit form>` of each digit, control 7 the level's icon of
// cshell+0x2682b0, control 8 string 0x156f with the level name; window 4 hidden when the bonus is below 1)
pub const LEVEL_UP_DIGITS: [i32; 4] = [2, 3, 4, 5];
pub const LEVEL_UP_ICON: i32 = 7;
pub const LEVEL_UP_TEXT: i32 = 8;
/// String ids: the level-up window's text, the rank-up box of 0x3004, the waiting room's rank-up notice.
// EVIDENCE: cshell+0x136e40 (0x156f), cshell+0x40580 / cshell+0x164020 (0x185f), cshell+0x15fb50 (0x487)
pub const LEVEL_UP_TEXT_ID: i32 = 0x156f;
pub const RANK_UP_BOX_ID: i32 = 0x185f;
pub const RANK_UP_NOTICE_ID: i32 = 0x487;
/// The interface sound the result screen plays when its level-up window opens (the M11 cue table).
// EVIDENCE: cshell+0x136e40 (screen +0x12c4 = 0xd), cshell+0x1343b0 (the next frame passes it to cshell+0x1b6470,
// the interface sound dispatcher cshell+0x1b3960)
pub const LEVEL_UP_SOUND: i32 = 0xd;
/// The bonus run of the MY SCORE values.
// EVIDENCE: cshell+0x136410 (`%s(+%s / ^0xffd49c00^+%s^)`: the bonus part in this colour)
pub const BONUS_COLOUR: u32 = 0xffd4_9c00;
/// The level-up mark of a result row.
// EVIDENCE: cshell+0x130220 (the mark image is 0x1c x 0xf when the row's level-up flag is set)
pub const LEVEL_UP_MARK_SIZE: [i32; 2] = [28, 15];
/// Height of one result row (the list's spacing comes from the page).
// EVIDENCE: cshell+0x130120 (the result row widget is 459 x 29; client-scoreboard, was the CLI-272 guess 28)
pub const ROW_HEIGHT: i32 = 29;
/// Text colours of the result rows: the others' rows grey, the own row orange.
// EVIDENCE: cshell+0x130120 (row text colour 0xffaaaaaa), cshell+0x130220 (the own row, found by name: colour
// 0xffff9900 and the row bar)
pub const ROW_COLOUR: u32 = 0xffaa_aaaa;
pub const OWN_ROW_COLOUR: u32 = 0xffff_9900;
/// The level icon of a result row.
// EVIDENCE: cshell+0x130220 (the level icon image is 0x1e x 0x16, its texture the icon picker's for the row's level)
pub const ROW_ICON_SIZE: [i32; 2] = [30, 22];
/// Font record and size of the result row texts.
// EVIDENCE: cshell+0x130220 (every row string control: font lookup cshell+0x1c07b0 with 0xe, size the row's
// +0x15c), cshell+0x130120 (+0x15c = 0xc)
pub const ROW_FONT: i32 = 14;
pub const ROW_TEXT_SIZE: i32 = 12;
/// Alignments of the row texts: the name left / top, the place number and the numbers centred / top.
// EVIDENCE: cshell+0x130220 (name flags 0x11, place and numbers 0x14)
pub const ROW_NAME_ALIGN: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};
pub const ROW_NUMBER_ALIGN: Align = Align {
    first: ALIGN_CENTRE,
    second: 0,
};

/// What the 2009 result row asks of its place number (with `index_at`) and its five column texts (with
/// `columns_at`) at row position `at`: each at the row plus its section-7 offset, [`ROW_NAME_ALIGN`] for the name,
/// [`ROW_NUMBER_ALIGN`] for the others, font [`ROW_FONT`], size [`ROW_TEXT_SIZE`]. Index 0 is the place number, 1..=5
/// the columns (name, kills, deaths, mission, experience). The draw trace's copy of the rule, apart from the drawing.
// EVIDENCE: cshell+0x1315c0 (controls at the row position plus the section-7 offsets), cshell+0x130220 (fonts,
// sizes and flags above)
// DC-DEFAULT: CLI-539 (proposed) layout keys `dyn/result<list>_<row>/<style key sha>`
pub fn result_row_wants(
    index_at: Option<[i32; 2]>,
    columns_at: Option<[[i32; 2]; 5]>,
    list: i32,
    row: usize,
    at: [i32; 2],
) -> [Option<DynWant>; 6] {
    let want = |pin: &crate::menu::catalog::KeyPin, p: [i32; 2], align: Align| DynWant {
        key: format!("dyn/result{list}_{row}/{}", pin.sha16),
        text_at: [(at[0] + p[0]) as f32, (at[1] + p[1]) as f32],
        text_anchor: anchor_of(align),
        font: Some(ROW_FONT),
        font_size: Some(ROW_TEXT_SIZE),
    };
    let mut out: [Option<DynWant>; 6] = Default::default();
    out[0] = index_at.map(|p| want(&row_keys::INDEX, p, ROW_NUMBER_ALIGN));
    if let Some(cols) = columns_at {
        for (k, p) in cols.iter().enumerate() {
            let align = if k == 0 {
                ROW_NAME_ALIGN
            } else {
                ROW_NUMBER_ALIGN
            };
            out[k + 1] = Some(want(&row_keys::COLUMNS[k], *p, align));
        }
    }
    out
}

/// The own row's bar image size (the row widget's size).
// EVIDENCE: cshell+0x130120 (0x1cb x 0x1d), cshell+0x130220 (the bar image takes that size)
pub const ROW_BAR_SIZE: [i32; 2] = [459, 29];
/// Offsets of the place number and the level icon inside a result row: keys of the result screen's layout section.
// EVIDENCE: cshell+0x1315c0 (the row places its controls at the row position plus these section-7 pairs,
// cshell+0x1c8430)
pub mod row_keys {
    use crate::menu::catalog::{key, KeyPin};
    pub const INDEX: KeyPin = key("cd706a6f94bf5e6f", 11);
    pub const LEVEL: KeyPin = key("c6b5fa04e858bcd4", 11);
    /// The own row's bar texture and its offset.
    pub const BAR: KeyPin = key("85820e6558c79123", 6);
    pub const BAR_AT: KeyPin = key("76c686497eb38036", 9);
    /// The level-up mark.
    // DISASM: cshell+0x131a80..0x131aaf (the key read into the mark's offset pair), cshell+0x131d02 (the mark placed
    // at the row position plus that pair)
    pub const LEVEL_UP: KeyPin = key("08c85622d7b0f389", 13);
    /// Name, kills, deaths, mission, experience.
    pub const COLUMNS: [KeyPin; 5] = [
        key("dee342d99fea1e50", 10),
        key("7b1c1cc5863b4ab0", 10),
        key("3a4e78f951c48334", 11),
        key("3b724e92f6cbfa0d", 13),
        key("2a6fd780409e207d", 9),
    ];
}

/// A number with thousands separators; below 1 it is `0`.
// EVIDENCE: cshell+0x136410 (`%d%s` for the leading group, `,%03d%s` for the others; a value below 1 prints `0`)
pub fn grouped(v: i64) -> String {
    if v < 1 {
        return "0".into();
    }
    let digits = v.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The MY SCORE value of experience or GP: the total after the match with the match's own gain in brackets, and
/// the bonus part after a slash in the bonus colour (`^0x<argb>^text^` markup, [`markup_segments`]).
// EVIDENCE: cshell+0x136410 (`total(+gain)`, with a bonus `%s(+%s / ^0xffd49c00^+%s^)`; total = the stored account
// value + gain + bonus), cshell+0x137720 (gain = the 0x65 gain minus its bonus part)
// w1-result: CLI-383 retired, the bonus run is drawn in its 2009 colour
pub fn summary_text(stored: u32, gain: u32, bonus: u32) -> String {
    let total = i64::from(stored) + i64::from(gain);
    let own = i64::from(gain) - i64::from(bonus);
    if bonus == 0 {
        format!("{}(+{})", grouped(total), grouped(own))
    } else {
        format!(
            "{}(+{} / ^0x{BONUS_COLOUR:08x}^+{}^)",
            grouped(total),
            grouped(own),
            grouped(i64::from(bonus))
        )
    }
}

/// A client printf form with its first string conversion (`%s`, `%ls`, `%S`, ...) replaced by `s` and its first
/// integer conversion (`%d`, `%i`, `%u`) by `n`; `%%` reads `%`.
pub fn fill_text(form: &str, s: &str, n: Option<i64>) -> String {
    let mut out = String::new();
    let (mut s_done, mut n_done) = (false, n.is_none());
    let mut it = form.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // flags, width and length modifiers, then the conversion
        let mut j = i + 1;
        let rest = &form[j..];
        let spec_len = rest
            .find(|ch: char| !(ch.is_ascii_digit() || "-+ #.lhIwL".contains(ch)))
            .unwrap_or(rest.len());
        j += spec_len;
        let conv = form[j..].chars().next();
        let consumed = match conv {
            Some('%') if spec_len == 0 => {
                out.push('%');
                true
            }
            Some('s' | 'S') if !s_done => {
                out.push_str(s);
                s_done = true;
                true
            }
            Some('d' | 'i' | 'u') if !n_done => {
                out.push_str(&n.unwrap_or(0).to_string());
                n_done = true;
                true
            }
            _ => false,
        };
        if consumed {
            let end = j + conv.map_or(0, char::len_utf8);
            while it.peek().is_some_and(|(k, _)| *k < end) {
                it.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Pinned literals of the result screen and the level table read for it.
// EVIDENCE: cshell+0x130220 (the row's level-up mark texture), cshell+0x136e40 (the level-up window's digit form),
// cshell+0x268810 (the level section and its name and bonus columns: column objects +8 and +0x18 of the level
// table, read by cshell+0x267e60 and cshell+0x267ec0)
const ART_PINS: [Pin; 5] = [
    Pin {
        what: "result row level-up mark",
        rva: 0x3e2b6c,
        sha16: "680e9bb90d131832",
        len: 40,
    },
    Pin {
        what: "level-up window digit form",
        rva: 0x3e3228,
        sha16: "553fbd796836ab3b",
        len: 33,
    },
    Pin {
        what: "level section",
        rva: 0x3f4a68,
        sha16: "9782640c0625e02f",
        len: 8,
    },
    Pin {
        what: "level name column key",
        rva: 0x3f4a50,
        sha16: "571e47573f9bb9a0",
        len: 9,
    },
    Pin {
        what: "level bonus column key",
        rva: 0x3f4a3c,
        sha16: "d587622d277c4be4",
        len: 5,
    },
];

/// What the result screen and the rank-up boxes take from the user's ROM beyond the page: the level-up mark, the
/// level-up window's digit form, and the level names and bonus values per level.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResultArt {
    pub level_up_mark: Option<String>,
    pub digit_form: Option<String>,
    pub names: Vec<String>,
    pub bonus: Vec<i64>,
}

impl ResultArt {
    pub fn from_rom(rom: &Path) -> Option<ResultArt> {
        let pe = dc_rom::PeFile::from_file(&rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]))
            .ok()?;
        let lit: Vec<Option<String>> = ART_PINS.iter().map(|p| read_pinned(&pe, p).ok()).collect();
        let mut art = ResultArt {
            level_up_mark: lit[0].clone(),
            digit_form: lit[1].clone(),
            ..ResultArt::default()
        };
        if let (Some(sec), Some(name_key), Some(bonus_key)) = (&lit[2], &lit[3], &lit[4]) {
            let vfs = dc_rez::RezVfs::new(rom);
            if let Some(doc) =
                dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", crate::hud::rank::LEVEL_ENTRY)
            {
                if let Some(m) = doc.get(sec) {
                    let column = |key: &str| -> Vec<String> {
                        (0..1024)
                            .map_while(|i| m.get(&format!("{key}{i}")))
                            .map(|v| v.trim().trim_matches('"').trim().to_string())
                            .collect()
                    };
                    art.names = column(name_key);
                    art.bonus = column(bonus_key).iter().map(|v| atoi(v)).collect();
                }
            }
        }
        Some(art)
    }

    /// The name of `level`; a level past the table reads row 0.
    // EVIDENCE: cshell+0x267e60 (`if (count <= level) level = 0`)
    pub fn name(&self, level: u32) -> &str {
        let i = if (level as usize) < self.names.len() {
            level as usize
        } else {
            0
        };
        self.names.get(i).map_or("", String::as_str)
    }

    /// The bonus value of `level` (`atoi`); a level past the table reads row 0.
    // EVIDENCE: cshell+0x267ec0
    pub fn bonus(&self, level: u32) -> i64 {
        let i = if (level as usize) < self.bonus.len() {
            level as usize
        } else {
            0
        };
        self.bonus.get(i).copied().unwrap_or(0)
    }
}

/// C `atoi`: optional sign and leading digits, 0 without digits.
fn atoi(v: &str) -> i64 {
    let t = v.trim_start();
    let (neg, rest) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let n: i64 = digits.parse().unwrap_or(0);
    if neg {
        -n
    } else {
        n
    }
}

/// The four digits the level-up window shows for `v` (thousands, hundreds, tens, ones; the first may pass 9).
// EVIDENCE: cshell+0x136e40 (v / 1000, v % 1000 / 100, (v - v / 100 x 100) / 10, the rest)
pub fn level_up_digits(v: i64) -> [i64; 4] {
    let hundreds = v / 100;
    let tens = (v - hundreds * 100) / 10;
    [
        v / 1000,
        v % 1000 / 100,
        tens,
        v - (tens + hundreds * 10) * 10,
    ]
}

/// The boxes a progress push that took effect opens, in their 2009 order: when the new experience is in a higher
/// level than the stored one, the popup flag's box 0x185f with the new level's name, then the waiting room's
/// notice 0x487 with the name and the level's bonus. Integrator hook: the menu pushes these on
/// [`LinkEvent::Progress`].
// EVIDENCE: cshell+0x40580 and cshell+0x164020 (cshell+0x267f10 of the stored experience below that of the new
// one: rank-up flag shell +0xe8 = 1; with the popup flag `swprintf(string 0x185f, level name)` in an OK box titled
// 0x371), cshell+0x15fb50 (called by the waiting room's frame cshell+0x1608b0: with the flag set, an OK box of
// `sprintf(string 0x487, level name, level bonus)` for the stored experience, then the flag is cleared; DISASM
// cshell+0x15fc15..0x15fc41: string 0x487 is loaded plain by cshell+0x1a0230, name and bonus are sprintf's)
// DC-DEFAULT: CLI-387 (proposed) the notice 0x487 opens with the push's own box, whatever screen is up (2009 waits
// for the next waiting-room frame; after a match both appear in the waiting room either way)
pub fn rank_up_texts(
    levels: &LevelTable,
    art: &ResultArt,
    p: &ProgressApplied,
    text: impl Fn(i32) -> String,
) -> Vec<String> {
    let new = levels.level_of(p.exp);
    if levels.level_of(p.exp_before) >= new {
        return Vec::new();
    }
    let name = art.name(new);
    let mut out = Vec::new();
    if p.popup {
        out.push(fill_text(&text(RANK_UP_BOX_ID), name, None));
    }
    out.push(fill_text(
        &text(RANK_UP_NOTICE_ID),
        name,
        Some(art.bonus(new)),
    ));
    out
}

/// [`rank_up_texts`] as notice boxes, the tables read from the catalog's ROM (none without a ROM).
pub fn rank_up_boxes(cat: &Catalog, p: &ProgressApplied) -> Vec<Popup> {
    let Some(rom) = cat.rom.as_deref() else {
        return Vec::new();
    };
    let (Some(levels), Some(art)) = (LevelTable::from_rom(rom), ResultArt::from_rom(rom)) else {
        return Vec::new();
    };
    rank_up_texts(&levels, &art, p, |id| cat.text(id))
        .into_iter()
        .map(|t| Popup::ok(t, BoxTag::None))
        .collect()
}

/// The page of the result screen: the one whose name is the pinned literal, else the result tag.
/// The scoreboard's win, lose and draw header textures of the HUD layout (set 0).
// EVIDENCE: cshell+0x522c0 / cshell+0x52390 / cshell+0xc1a50 (win / lose / draw header names, HUD set 0)
fn header_textures(cat: &Catalog) -> Option<[Option<String>; 3]> {
    use dc_ui_data::{HudKey, HudKeys, HudView};
    let keys = HudKeys::from_rom(cat.rom.as_deref()?).ok()?;
    let v = HudView::new(cat.layout.as_ref()?, &keys, 0);
    let t = |k| v.text(k).map(str::to_string);
    Some([
        t(HudKey::ResultWinTex),
        t(HudKey::ResultLoseTex),
        t(HudKey::ResultDrawTex),
    ])
}

fn result_tag(cat: &Catalog) -> PageTag {
    let by_name = cat.paths.result_page.as_ref().and_then(|_| {
        let pin = super::catalog::pins::RESULT_PAGE;
        cat.pages
            .pages
            .iter()
            .find(|p| {
                p.screen_name
                    .as_deref()
                    .is_some_and(|n| matches_pin(n, pin.sha16, pin.len))
            })
            .map(|p| p.tag)
    });
    by_name.unwrap_or(pages::RESULT)
}

/// The digit image of `d` from a digit image name (`<prefix>0.dtx` form).
// DC-DEFAULT: CLI-272 (proposed) score digits: the page's digit image name with its digit replaced; leading zeros
// are drawn
pub fn digit_texture(zero: &str, d: u32) -> Option<String> {
    let lower = zero.to_ascii_lowercase();
    let cut = lower.rfind(".dtx")?;
    let digit_at = cut.checked_sub(1)?;
    zero[digit_at..cut]
        .chars()
        .all(|c| c.is_ascii_digit())
        .then(|| format!("{}{}{}", &zero[..digit_at], d % 10, &zero[cut..]))
}

pub struct ResultScreen {
    sheet: Sheet,
    result: MatchResult,
    /// The level table (rank icons by experience) and the row offsets of the place number and the level icon.
    levels: Option<crate::hud::board::LevelTable>,
    index_at: Option<[i32; 2]>,
    level_at: Option<[i32; 2]>,
    /// Row offsets of name, kills, deaths, mission and experience (all five, or the header columns are used).
    columns_at: Option<[[i32; 2]; 5]>,
    /// The own row's bar texture and offset.
    bar: Option<(String, [i32; 2])>,
    /// w1-result: the ROM's level names / bonus values, the level-up mark and its row offset.
    art: Option<ResultArt>,
    level_up_at: Option<[i32; 2]>,
    /// The summary values drawn as coloured runs (control id, markup text).
    runs: Vec<(i32, String)>,
    /// The interface sound to play (the level-up cue), for the menu's sound hook.
    pub sound: Option<i32>,
    opened: Instant,
    /// Test hook: the screen's clock runs this much ahead.
    pub clock_ahead: Duration,
    exit_sent: bool,
}

impl ResultScreen {
    pub fn new(cat: &Catalog, data: &MenuData) -> ResultScreen {
        let tag = result_tag(cat);
        let mut sheet = Sheet::build(&cat.pages, &[tag], cat);
        if let Some(prefix) = &cat.paths.result_back {
            sheet.tiles = Tiles::new([0, 0], 3, 4, prefix);
        }
        let result = data.result.clone().unwrap_or_default();
        let coop = result.mode_class == Some(5);
        sheet.show_window(WINDOW_TEAMS, !coop);
        sheet.show_window(WINDOW_COOP, coop);
        // the non-clan summary (the clan server's form has the clan experience column)
        sheet.show_window(WINDOW_SUMMARY, true);
        sheet.show_window(WINDOW_CLAN_SUMMARY, false);
        // DC-DEFAULT: CLI-272 (amended, proposed) no abused-game notice, no mode / goal / map images (picked per
        // mode class in cshell+0x132b80 from the game client's map and mode state, not yet traced to its sources),
        // no bonus marks in the summary (no item or internet-cafe bonus); the level-up window opens below when the
        // own level changed
        sheet.show_window(WINDOW_LEVEL_UP, false);
        sheet.show(-1, ABUSED_IMAGE, WidgetKind::Image, false);
        for id in MODE_IMAGES {
            sheet.show(WINDOW_TEAMS, id, WidgetKind::Image, false);
        }
        for id in SUMMARY_MARKS {
            sheet.show(WINDOW_SUMMARY, id, WidgetKind::Image, false);
        }
        sheet.set_text(WINDOW_SUMMARY, SUMMARY_LEVEL, "");
        let own = result.own_row().cloned();
        let nick = own
            .as_ref()
            .map(|r| r.name.clone())
            .unwrap_or_else(|| data.profile.nick.clone());
        sheet.set_text(WINDOW_SUMMARY, SUMMARY_ID, &nick);
        // client-scoreboard: the totals with the gains, from the stored account values (the lobby's 0x3004 of the
        // match is parked until the waiting room, so the profile still holds the values before the match)
        // SPEC: PROTOCOL_2009.md §3 0x3004 (in a match or on the result screen the values are parked and applied
        // later)
        let (exp_text, gp_text) = summary_texts(&result, data.profile.exp, data.profile.gp);
        // w1-result: a value with a bonus run is drawn as coloured runs by the screen (the sheet's text is one colour)
        let mut runs = Vec::new();
        for (id, t) in [(SUMMARY_EXP, exp_text), (SUMMARY_GP, gp_text)] {
            if t.contains("^0x") {
                sheet.set_text(WINDOW_SUMMARY, id, "");
                runs.push((id, t));
            } else {
                sheet.set_text(WINDOW_SUMMARY, id, &t);
            }
        }
        let levels = cat.rom.as_deref().and_then(LevelTable::from_rom);
        let art = cat.rom.as_deref().and_then(ResultArt::from_rom);
        let after = data
            .profile
            .exp
            .saturating_add(own.as_ref().map_or(0, |r| r.exp_gain));
        // w1-result: the level text, the progress through the level of the total after the match
        // EVIDENCE: cshell+0x136920 (`%3.2f %%` set on the summary's level text)
        if let Some(t) = &levels {
            sheet.set_text(
                WINDOW_SUMMARY,
                SUMMARY_LEVEL,
                &level_progress_text(t, after),
            );
        }
        // w1-result: the level-up window when the own level changed in the match
        let mut sound = None;
        if let (Some(t), Some(a), Some(r)) = (&levels, &art, &own) {
            if r.levelled_up(t) {
                sound = level_up_window(&mut sheet, t, a, after, |id| cat.text(id));
            }
        }
        // the own rank icon: the icon of the total experience after the match
        // EVIDENCE: cshell+0x136920 (cshell+0x2683d0 of the stored experience + gain + bonus, set on the summary's
        // image)
        let icon = levels.as_ref().and_then(|t| {
            let total = data
                .profile
                .exp
                .saturating_add(own.as_ref().map_or(0, |r| r.exp_gain));
            t.icon_of_exp(total, &nick).map(str::to_string)
        });
        let mut icon_shown = false;
        if let (Some(t), Some(p)) = (
            icon,
            sheet.part_mut(WINDOW_SUMMARY, SUMMARY_RANK_ICON, WidgetKind::Image),
        ) {
            if let Look::Panel { tex, .. } = &mut p.look {
                *tex = t;
                icon_shown = true;
            }
        }
        sheet.show(
            WINDOW_SUMMARY,
            SUMMARY_RANK_ICON,
            WidgetKind::Image,
            icon_shown,
        );
        let index_at = super::catalog::pair(cat.style(UiScreen::Screen7, &row_keys::INDEX));
        let level_at = super::catalog::pair(cat.style(UiScreen::Screen7, &row_keys::LEVEL));
        let bar = cat
            .style(UiScreen::Screen7, &row_keys::BAR)
            .map(|t| t.trim().trim_matches('"').trim().to_string())
            .filter(|t| !t.is_empty())
            .zip(super::catalog::pair(
                cat.style(UiScreen::Screen7, &row_keys::BAR_AT),
            ));
        let columns_at = row_keys::COLUMNS
            .iter()
            .map(|k| super::catalog::pair(cat.style(UiScreen::Screen7, k)))
            .collect::<Option<Vec<_>>>()
            .and_then(|v| <[[i32; 2]; 5]>::try_from(v).ok());
        // the mark over the lists: the scoreboard's header of the own result (the page's own image stays without
        // a ROM header)
        // DC-DEFAULT: CLI-272 (proposed) the result page's mark image shows the HUD's win / lose / draw header by the
        // own team (the 2009 pick of that image in cshell+0x132b80 is not traced)
        if let Some([win, lose, draw]) = header_textures(cat) {
            let pick = match result.own_won() {
                Some(true) => win,
                Some(false) => lose,
                None => draw,
            };
            if let (Some(t), Some(p)) = (
                pick,
                sheet.part_mut(WINDOW_TEAMS, MARK_IMAGE, WidgetKind::Image),
            ) {
                if let Look::Panel { tex, .. } = &mut p.look {
                    *tex = t;
                }
            }
        }
        // the team score digits (window 0 controls 1..6) are hidden in the single-list classes (One Man Army and the
        // other free-for-all forms): no team pair
        // EVIDENCE: cshell+0x132b80 (classes 6 / 9 / 0xb: window 0 controls 1..6 shown false; the team classes show
        // them and hide 7..10) (CLI-781)
        let single = result.one_ranking();
        for id in SCORE_LEFT.iter().chain(SCORE_RIGHT.iter()) {
            sheet.show(WINDOW_TEAMS, *id, WidgetKind::Image, !single);
        }
        let (left, right) = result.sides();
        for (ids, team) in [(SCORE_LEFT, left), (SCORE_RIGHT, right)] {
            let v = result.score_of(team).min(999);
            for (k, id) in ids.iter().enumerate() {
                let d = v / 10u32.pow(2 - k as u32) % 10;
                if let Some(p) = sheet.part_mut(WINDOW_TEAMS, *id, WidgetKind::Image) {
                    if let Look::Panel { tex, .. } = &mut p.look {
                        if let Some(t) = digit_texture(tex, d) {
                            *tex = t;
                        }
                    }
                }
            }
        }
        let level_up_at = super::catalog::pair(cat.style(UiScreen::Screen7, &row_keys::LEVEL_UP));
        // the 60 s count from the moment the 2009 screen filled its rows (the 0xb leave), when the session gave it
        // DISASM: cshell+0x1343b0 (stamp in the frame after the fill flag), cshell+0x137dd0 (fill at the focus)
        let opened = result.filled_at.unwrap_or_else(Instant::now);
        ResultScreen {
            sheet,
            result,
            levels,
            index_at,
            level_at,
            columns_at,
            bar,
            art,
            level_up_at,
            runs,
            sound,
            opened,
            clock_ahead: Duration::ZERO,
            exit_sent: false,
        }
    }

    pub fn result(&self) -> &MatchResult {
        &self.result
    }

    /// The screen's clock: time since it opened (plus the test hook).
    pub fn elapsed(&self) -> Duration {
        self.opened.elapsed() + self.clock_ahead
    }

    /// The 60 s of the screen ran out.
    pub fn exit_due(elapsed: Duration) -> bool {
        elapsed.as_millis() >= RESULT_AUTO_EXIT_MS as u128
    }

    fn exit(&mut self, ctx: &mut Ctx) {
        if self.exit_sent {
            return;
        }
        self.exit_sent = true;
        ctx.data.result = None;
        ctx.send(LinkCmd::BackToRoom);
        *ctx.go = Some(Go::Room);
    }

    /// Whether the level-up window is open (tests, gallery).
    pub fn level_up_shown(&self) -> bool {
        self.sheet
            .parts
            .iter()
            .any(|p| p.kind == WidgetKind::Window && p.layer == WINDOW_LEVEL_UP && p.shown)
    }

    /// The text of a control of window `layer` (tests).
    /// Whether image control `id` of window `layer` is shown (`None`: no such control).
    pub fn image_shown(&self, layer: i32, id: i32) -> Option<bool> {
        self.sheet
            .part(layer, id, WidgetKind::Image)
            .map(|p| p.shown)
    }

    pub fn text_of(&self, layer: i32, id: i32) -> Option<&str> {
        self.sheet
            .part(layer, id, WidgetKind::Text)
            .map(|p| p.text.as_str())
    }

    fn list(&self, id: i32) -> Option<&Part> {
        self.sheet.part(WINDOW_TEAMS, id, WidgetKind::List)
    }

    fn draw_rows(
        &self,
        (list, list_id): (&Part, i32),
        columns: [i32; 5],
        rows: &[&ResultRow],
        first_place: usize,
        out: &mut Vec<DrawItem>,
        mut notes: Option<&mut DynNotes>,
    ) -> usize {
        let own = self.result.own_pid;
        let mut drawn = 0;
        for (i, r) in rows.iter().enumerate() {
            let Some(at) = list_row(list, ROW_HEIGHT, i, 0) else {
                break;
            };
            drawn = i + 1;
            let wants = notes
                .as_ref()
                .map(|_| result_row_wants(self.index_at, self.columns_at, list_id, i, at));
            let mut note = |out: &Vec<DrawItem>, k: usize| {
                if let (Some(n), Some(Some(w))) =
                    (notes.as_deref_mut(), wants.as_ref().map(|w| w[k].as_ref()))
                {
                    n.push((out.len() - 1, w.clone()));
                }
            };
            let colour = if Some(r.pid) == own {
                OWN_ROW_COLOUR
            } else {
                ROW_COLOUR
            };
            // the own row's bar under its texts
            // EVIDENCE: cshell+0x130220 (the own row, by name, gets the section-7 bar texture at the row size),
            // cshell+0x1315c0 (placed at the bar offset)
            if let (true, Some((tex, off))) = (Some(r.pid) == own, &self.bar) {
                out.push(DrawItem::Image {
                    tex: tex.clone(),
                    src: [ROW_BAR_SIZE[0] as f32, ROW_BAR_SIZE[1] as f32],
                    dst: [
                        (at[0] + off[0]) as f32,
                        (at[1] + off[1]) as f32,
                        ROW_BAR_SIZE[0] as f32,
                        ROW_BAR_SIZE[1] as f32,
                    ],
                    tint: [255; 4],
                    turn: None,
                });
            }
            // the place number, centred on its offset point
            // EVIDENCE: cshell+0x130220 (`%d` of the row index, flags 0x14 = centred, top, font 0xe, size 0xc),
            // cshell+0x134b80 (index from 1 per list)
            if let Some(off) = self.index_at {
                out.push(place_text(
                    &(first_place + i + 1).to_string(),
                    [at[0] + off[0], at[1] + off[1]],
                    [0, 0],
                    ROW_NUMBER_ALIGN,
                    ROW_FONT,
                    ROW_TEXT_SIZE,
                    colour,
                ));
                note(out, 0);
            }
            // w1-result: the level-up mark of a player whose level changed in the match
            // EVIDENCE: cshell+0x137720 (the row flag), cshell+0x130220 (mark image 28 x 15), cshell+0x131d02 (at
            // the row position plus the mark's section-7 offset)
            if let (Some(off), Some(t), Some(tex)) = (
                self.level_up_at,
                self.levels.as_ref(),
                self.art.as_ref().and_then(|a| a.level_up_mark.as_ref()),
            ) {
                if r.levelled_up(t) {
                    out.push(DrawItem::Image {
                        tex: tex.clone(),
                        src: [LEVEL_UP_MARK_SIZE[0] as f32, LEVEL_UP_MARK_SIZE[1] as f32],
                        dst: [
                            (at[0] + off[0]) as f32,
                            (at[1] + off[1]) as f32,
                            LEVEL_UP_MARK_SIZE[0] as f32,
                            LEVEL_UP_MARK_SIZE[1] as f32,
                        ],
                        tint: [255; 4],
                        turn: None,
                    });
                }
            }
            // the level icon of the experience after the match
            // EVIDENCE: cshell+0x137720 (row level = the level of the 0x65 experience before + gain,
            // cshell+0x267f10), cshell+0x130220 (icon picker cshell+0x268300, 30 x 22)
            if let (Some(off), Some(t)) = (self.level_at, self.levels.as_ref()) {
                if let Some(tex) = t.icon_of_exp(r.exp_before.saturating_add(r.exp_gain), &r.name) {
                    out.push(DrawItem::Image {
                        tex: tex.to_string(),
                        src: [ROW_ICON_SIZE[0] as f32, ROW_ICON_SIZE[1] as f32],
                        dst: [
                            (at[0] + off[0]) as f32,
                            (at[1] + off[1]) as f32,
                            ROW_ICON_SIZE[0] as f32,
                            ROW_ICON_SIZE[1] as f32,
                        ],
                        tint: [255; 4],
                        turn: None,
                    });
                }
            }
            let values = [
                r.name.clone(),
                r.kills.to_string(),
                r.deaths.to_string(),
                // CLI-1161 the mission column: v64, or captures / returns in the flag classes (cshell+0x130220)
                crate::hud::board::mission_values_text(r.mission, self.result.mode_class),
                r.exp_gain.to_string(),
            ];
            for (k, (col, text)) in columns.iter().zip(values).enumerate() {
                let Some(h) = self.sheet.part(WINDOW_TEAMS, *col, WidgetKind::Text) else {
                    continue;
                };
                let Look::Text {
                    align, font, size, ..
                } = h.look
                else {
                    continue;
                };
                // client-scoreboard: the row's own offsets: the name left, the numbers centred, tops on the points
                // EVIDENCE: cshell+0x1315c0 (controls at the row position plus the section-7 offsets),
                // cshell+0x130220 (name flags 0x11 = left, top; kills, deaths, experience 0x14 = centred, top; font
                // record 0xe at size 0xc: CLI-384 retired by w2-gaterows, the menu's font table has record 0xe)
                if let Some(offs) = self.columns_at {
                    let off = offs[k];
                    out.push(place_text(
                        &text,
                        [at[0] + off[0], at[1] + off[1]],
                        [0, 0],
                        if k == 0 {
                            ROW_NAME_ALIGN
                        } else {
                            ROW_NUMBER_ALIGN
                        },
                        ROW_FONT,
                        ROW_TEXT_SIZE,
                        colour,
                    ));
                    note(out, k + 1);
                    continue;
                }
                out.push(place_text(
                    &text,
                    [h.pos[0], at[1]],
                    [0, ROW_HEIGHT],
                    Align {
                        first: align.first,
                        second: ALIGN_SECOND_CENTRE,
                    },
                    font,
                    size,
                    colour,
                ));
            }
        }
        drawn
    }
}

/// The MY SCORE experience and GP texts of `result` with the stored account values.
pub fn summary_texts(result: &MatchResult, exp: u32, gp: u32) -> (String, String) {
    let own = result.own_row();
    (
        summary_text(
            exp,
            own.map_or(0, |r| r.exp_gain),
            own.map_or(0, |r| r.exp_bonus),
        ),
        summary_text(
            gp,
            own.map_or(0, |r| r.gp_gain),
            own.map_or(0, |r| r.gp_bonus),
        ),
    )
}

impl ResultScreen {
    /// The interface sound the screen plays when it opens: the level-up sound, else the win or the lose sound of the
    /// own result.
    // EVIDENCE: cshell+0x1326b0 (team boards: own team the winner -> 0xf, else 0x10; co-op: complete 0xf, fail /
    // timeout 0x10; each only when no sound is set yet), cshell+0x136e40 (level up: 0xd), cshell+0x1343b0 (played once
    // on the next update)
    pub fn entry_sound(&self) -> i32 {
        self.sound
            .unwrap_or(if self.result.own_won() == Some(true) {
                super::frontfx::SOUND_RESULT_WIN
            } else {
                super::frontfx::SOUND_RESULT_LOSE
            })
    }
}

impl Screen for ResultScreen {
    fn name(&self) -> &'static str {
        "result"
    }

    fn enter(&mut self, ctx: &mut Ctx) {
        ctx.sound(self.entry_sound());
        // CLI-1880: the active bonus passes lose the match's use
        super::storage::Wares::cached(ctx.cat.rom.as_deref())
            .count_down_passes(&mut ctx.data.store);
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        // DC-DEFAULT: CLI-273 (proposed) Enter and Escape act as the exit button on the result screen
        if matches!(k, MenuKey::Enter | MenuKey::Escape) {
            self.exit(ctx);
        }
    }

    fn click(&mut self, cmd: i32, _part: usize, ctx: &mut Ctx) {
        match cmd {
            RESULT_CMD_EXIT => self.exit(ctx),
            RESULT_CMD_LEAVE_ROOM => {
                // w1-result: the link ends the parking of 0x3004 on this leave (M2); the 60 s exit no longer runs
                if self.exit_sent {
                    return;
                }
                self.exit_sent = true;
                ctx.data.result = None;
                ctx.send(LinkCmd::QuitRoom);
            }
            _ => {}
        }
    }

    fn event(&mut self, e: &LinkEvent, ctx: &mut Ctx) {
        if let LinkEvent::LeftRoom = e {
            *ctx.go = Some(Go::Lobby);
        }
    }

    fn due(&mut self) -> Option<i32> {
        (!self.exit_sent && ResultScreen::exit_due(self.elapsed())).then_some(RESULT_CMD_EXIT)
    }

    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, secs: f32, out: &mut Vec<DrawItem>) {
        self.draw_all(cat, data, secs, out, None);
    }

    fn draw_extra_noted(
        &self,
        cat: &mut Catalog,
        data: &MenuData,
        secs: f32,
        out: &mut Vec<DrawItem>,
        notes: &mut DynNotes,
    ) {
        self.draw_all(cat, data, secs, out, Some(notes));
    }
}

impl ResultScreen {
    /// The summary runs and the team rows; with `notes` the rows note their 2009 rule ([`result_row_wants`]).
    fn draw_all(
        &self,
        cat: &mut Catalog,
        _data: &MenuData,
        _secs: f32,
        out: &mut Vec<DrawItem>,
        mut notes: Option<&mut DynNotes>,
    ) {
        if self.sheet_window_shown(WINDOW_SUMMARY) {
            self.draw_runs(cat, out);
        }
        let (left, right) = self.result.sides();
        let single = self.result.one_ranking();
        // a single list: the ranking fills the left list and runs on in the right one, places counting on
        // EVIDENCE: cshell+0x137720 (one sorted list), cshell+0x134b80 (the second list's index starts at the first
        // list's count)
        // DC-DEFAULT: CLI-688 (proposed) the rows the left list cannot hold go on in the right list
        let ranking = self.result.ranking();
        let mut placed = 0;
        for (id, team, cols) in [
            (LIST_LEFT, left, COLUMNS_LEFT),
            (LIST_RIGHT, right, COLUMNS_RIGHT),
        ] {
            if !self.sheet_window_shown(WINDOW_TEAMS) {
                continue;
            }
            if let Some(list) = self.list(id) {
                if single {
                    let rest = &ranking[placed.min(ranking.len())..];
                    placed +=
                        self.draw_rows((list, id), cols, rest, placed, out, notes.as_deref_mut());
                } else {
                    let rows = self.result.rows_of(team);
                    self.draw_rows((list, id), cols, &rows, 0, out, notes.as_deref_mut());
                }
            }
        }
        // without a ROM page: a plain list so the screen still says something
        if self.sheet.parts.is_empty() {
            let mut y = 120;
            let lists = if single {
                vec![ranking.clone()]
            } else {
                vec![self.result.rows_of(left), self.result.rows_of(right)]
            };
            for list in lists {
                for r in list {
                    out.push(place_text(
                        &format!(
                            "team {} {} {} / {} exp +{} gp +{}",
                            r.team, r.name, r.kills, r.deaths, r.exp_gain, r.gp_gain
                        ),
                        [80, y],
                        [0, 0],
                        Align::default(),
                        0,
                        16,
                        0xffff_ffff,
                    ));
                    y += 24;
                }
            }
        }
    }
}

/// Opens window 4 for a level-up to the level of `exp` (the stored experience + the match's gain): the level's
/// bonus as four digit images, the level's icon and text 0x156f with its name in white. Returns the level-up sound,
/// or `None` when the window stays closed (a bonus below 1).
// EVIDENCE: cshell+0x136e40
fn level_up_window(
    sheet: &mut Sheet,
    levels: &LevelTable,
    art: &ResultArt,
    exp: u32,
    text: impl Fn(i32) -> String,
) -> Option<i32> {
    let level = levels.level_of(exp);
    let bonus = art.bonus(level);
    if bonus < 1 {
        return None;
    }
    if let Some(form) = &art.digit_form {
        for (id, d) in LEVEL_UP_DIGITS.iter().zip(level_up_digits(bonus)) {
            if let Some(p) = sheet.part_mut(WINDOW_LEVEL_UP, *id, WidgetKind::Image) {
                if let Look::Panel { tex, .. } = &mut p.look {
                    *tex = form.replacen("%d", &d.to_string(), 1);
                }
            }
        }
    }
    // the level's icon: the level texture list at the level (cshell+0x2682b0: none past the list)
    let icon = levels
        .small
        .get(level as usize)
        .filter(|t| !t.is_empty())
        .cloned();
    let has_icon = icon.is_some();
    if let (Some(t), Some(p)) = (
        icon,
        sheet.part_mut(WINDOW_LEVEL_UP, LEVEL_UP_ICON, WidgetKind::Image),
    ) {
        if let Look::Panel { tex, .. } = &mut p.look {
            *tex = t;
        }
    }
    if !has_icon {
        sheet.show(WINDOW_LEVEL_UP, LEVEL_UP_ICON, WidgetKind::Image, false);
    }
    if let Some(p) = sheet.part_mut(WINDOW_LEVEL_UP, LEVEL_UP_TEXT, WidgetKind::Text) {
        // a FormatMessage form with the name as insert 1 (the other two boxes are printf forms)
        // EVIDENCE: cshell+0x1a00b0 (LoadStringA, then FormatMessageA with the caller's arguments)
        p.text = dc_ui_data::strings::format_message(
            &text(LEVEL_UP_TEXT_ID),
            &[dc_ui_data::FmtArg::Str(art.name(level))],
        );
        if let Look::Text { colour, align, .. } = &mut p.look {
            // flags 0x11 (left, top), colour 0xffffffff
            *colour = 0xffff_ffff;
            *align = Align {
                first: ALIGN_LEFT,
                second: 0,
            };
        }
    }
    sheet.show_window(WINDOW_LEVEL_UP, true);
    Some(LEVEL_UP_SOUND)
}

impl ResultScreen {
    /// The summary values with a bonus run, run by run at their control's place and alignment.
    fn draw_runs(&self, cat: &mut Catalog, out: &mut Vec<DrawItem>) {
        let (w, h) = cat.size();
        let sx = VirtualScaler::new(w, h).sx.max(f32::EPSILON);
        for (id, markup) in &self.runs {
            let Some(p) = self.sheet.part(WINDOW_SUMMARY, *id, WidgetKind::Text) else {
                continue;
            };
            let Look::Text {
                align,
                font,
                size,
                colour,
            } = p.look
            else {
                continue;
            };
            let base = super::catalog::argb(colour);
            let segs = markup_segments(markup, base);
            let atlas = cat
                .assets
                .fonts
                .atlas_sized(font, (size > 0).then_some(size));
            let width = |t: &str| -> f32 {
                atlas
                    .as_ref()
                    .map_or(t.chars().count() as f32 * size.max(8) as f32 * 0.5, |a| {
                        a.measure(t) / sx
                    })
            };
            let total: f32 = segs.iter().map(|(_, t)| width(t)).sum();
            let ax = match align.first {
                ALIGN_CENTRE => 0.5,
                ALIGN_RIGHT => 1.0,
                _ => 0.0,
            };
            let mut x = p.pos[0] as f32 + ax * (p.size[0] as f32 - total);
            for (c, t) in &segs {
                let argb = u32::from_be_bytes([c[3], c[0], c[1], c[2]]);
                out.push(place_text(
                    t,
                    [x.round() as i32, p.pos[1]],
                    [0, p.size[1]],
                    Align {
                        first: ALIGN_LEFT,
                        second: align.second,
                    },
                    font,
                    size,
                    argb,
                ));
                x += width(t);
            }
        }
    }

    fn sheet_window_shown(&self, layer: i32) -> bool {
        self.sheet
            .parts
            .iter()
            .rev()
            .find(|p| p.kind == WidgetKind::Window && p.layer == layer)
            .is_none_or(|p| p.shown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ROM-free: the result row rule puts the place number and the five columns at the row plus their section-7
    /// offsets, the name left / top, the others centred / top, font 14 size 12; missing offsets give no want.
    #[test]
    fn result_row_wants_follow_the_rule() {
        let cols = [[10, 1], [200, 2], [250, 3], [300, 4], [350, 5]];
        let w = result_row_wants(Some([-5, 0]), Some(cols), 1, 4, [20, 40]);
        let idx = w[0].as_ref().unwrap();
        assert_eq!(idx.key, format!("dyn/result1_4/{}", row_keys::INDEX.sha16));
        assert_eq!(idx.text_at, [15.0, 40.0]);
        assert_eq!(idx.text_anchor, [0.5, 0.0]);
        for k in 0..5 {
            let c = w[k + 1].as_ref().unwrap();
            assert_eq!(
                c.key,
                format!("dyn/result1_4/{}", row_keys::COLUMNS[k].sha16)
            );
            assert_eq!(
                c.text_at,
                [(20 + cols[k][0]) as f32, (40 + cols[k][1]) as f32]
            );
            assert_eq!(c.text_anchor, if k == 0 { [0.0, 0.0] } else { [0.5, 0.0] });
        }
        for c in w.iter().flatten() {
            assert_eq!((c.font, c.font_size), (Some(14), Some(12)));
        }
        let none = result_row_wants(None, None, 0, 0, [0, 0]);
        assert!(none.iter().all(Option::is_none));
    }

    #[test]
    fn digit_images_and_the_exit_timer() {
        assert_eq!(
            digit_texture("A\\B\\score_0.dtx", 7).as_deref(),
            Some("A\\B\\score_7.dtx")
        );
        assert_eq!(digit_texture("A\\plain.dtx", 7), None);
        assert!(!ResultScreen::exit_due(Duration::from_millis(59_999)));
        assert!(ResultScreen::exit_due(Duration::from_millis(60_000)));
    }

    #[test]
    fn the_screen_without_a_rom_lists_the_rows_and_exits_by_its_timer() {
        let cat = Catalog::fallback(None, 1024, 768);
        let data = MenuData {
            result: Some(MatchResult::sample()),
            ..MenuData::default()
        };
        let mut s = ResultScreen::new(&cat, &data);
        assert_eq!(s.due(), None);
        s.clock_ahead = Duration::from_millis(RESULT_AUTO_EXIT_MS);
        assert_eq!(s.due(), Some(RESULT_CMD_EXIT));
        let mut cat = cat;
        let mut out = Vec::new();
        s.draw_extra(&mut cat, &data, 0.0, &mut out);
        assert_eq!(out.len(), 4, "one line per player");
    }

    #[test]
    fn summary_shows_the_total_with_the_gain_and_the_bonus() {
        // client-scoreboard: 2009 `total(+gain)` / `total(+gain / +bonus)` with thousands separators
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(-5), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(225_043), "225,043");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(summary_text(225_000, 43, 0), "225,043(+43)");
        assert_eq!(
            summary_text(500_000, 34, 4),
            "500,034(+30 / ^0xffd49c00^+4^)"
        );
        assert_eq!(summary_text(0, 0, 0), "0(+0)");
        let mut r = MatchResult::sample();
        r.rows[0].exp_bonus = 3;
        assert_eq!(
            summary_texts(&r, 1000, 2000),
            (
                "1,043(+40 / ^0xffd49c00^+3^)".to_string(),
                "2,034(+34)".to_string()
            )
        );
    }

    fn table(bounds: &[u32]) -> LevelTable {
        LevelTable {
            bounds: bounds.to_vec(),
            small: vec!["i0".into(), "i1".into(), "i2".into()],
        }
    }

    fn art() -> ResultArt {
        ResultArt {
            level_up_mark: Some("mark".into()),
            digit_form: Some("n%d.x".into()),
            names: vec!["Zero".into(), "One".into(), "Two".into()],
            bonus: vec![0, 1234, -1],
        }
    }

    #[test]
    fn level_up_digits_and_text_forms() {
        assert_eq!(level_up_digits(1234), [1, 2, 3, 4]);
        assert_eq!(level_up_digits(50), [0, 0, 5, 0]);
        assert_eq!(level_up_digits(12_345), [12, 3, 4, 5]);
        assert_eq!(fill_text("up to %s!", "One", None), "up to One!");
        assert_eq!(fill_text("%s gets %d", "One", Some(7)), "One gets 7");
        assert_eq!(fill_text("now %ls!.", "One", None), "now One!.");
        assert_eq!(fill_text("%S at %3d%%", "One", Some(5)), "One at 5%");
        assert_eq!(fill_text("%d %s %d", "x", Some(1)), "1 x %d");
        assert_eq!(atoi(" -12x"), -12);
        assert_eq!(atoi("\"3\""), 0, "quotes are trimmed before");
        let a = art();
        assert_eq!(
            (a.name(1), a.name(9)),
            ("One", "Zero"),
            "past the table: row 0"
        );
        assert_eq!((a.bonus(1), a.bonus(9)), (1234, 0));
    }

    #[test]
    fn rank_up_boxes_follow_the_popup_flag_and_the_level_change() {
        // w1-result: cshell+0x40580 / 0x164020 (box 0x185f with the popup flag), cshell+0x15fb50 (notice 0x487)
        let t = table(&[100, 300, 600]);
        let a = art();
        let text = |id: i32| match id {
            RANK_UP_BOX_ID => "now %s".to_string(),
            RANK_UP_NOTICE_ID => "%s: %d GP".to_string(),
            _ => String::new(),
        };
        let p = |before, exp, popup| ProgressApplied {
            exp_before: before,
            exp,
            gp: 1,
            popup,
        };
        assert_eq!(
            rank_up_texts(&t, &a, &p(90, 150, true), text),
            ["now One", "One: 1234 GP"]
        );
        assert_eq!(
            rank_up_texts(&t, &a, &p(90, 150, false), text),
            ["One: 1234 GP"]
        );
        assert!(
            rank_up_texts(&t, &a, &p(110, 150, true), text).is_empty(),
            "same level"
        );
        assert!(
            rank_up_texts(&t, &a, &p(150, 90, true), text).is_empty(),
            "down"
        );
    }

    #[test]
    fn the_level_up_window_opens_only_with_a_bonus() {
        // without a ROM page the window parts do not exist: the decision and the sound remain
        let mut sheet = Sheet::default();
        let t = table(&[100, 300, 600]);
        assert_eq!(
            level_up_window(&mut sheet, &t, &art(), 150, |_| "%s".into()),
            Some(LEVEL_UP_SOUND)
        );
        assert_eq!(
            level_up_window(&mut sheet, &t, &art(), 50, |_| "%s".into()),
            None
        );
        assert_eq!(
            level_up_window(&mut sheet, &t, &art(), 400, |_| "%s".into()),
            None
        );
    }

    #[test]
    fn leaving_the_room_stops_the_exit_timer_and_the_timer_counts_from_the_fill() {
        let cat = Catalog::fallback(None, 1024, 768);
        let mut data = MenuData {
            result: Some(MatchResult::sample()),
            ..MenuData::default()
        };
        let mut s = ResultScreen::new(&cat, &data);
        let link = super::super::link::Link::start(super::super::link::LinkOptions::new("t"));
        let (mut popups, mut go, mut quit, mut sounds) = (Vec::new(), None, false, Vec::new());
        let mut ctx = Ctx {
            cat: &cat,
            data: &mut data,
            link: &link,
            popups: &mut popups,
            go: &mut go,
            quit: &mut quit,
            sounds: &mut sounds,
        };
        s.click(RESULT_CMD_LEAVE_ROOM, usize::MAX, &mut ctx);
        assert!(ctx.data.result.is_none());
        s.clock_ahead = Duration::from_millis(RESULT_AUTO_EXIT_MS);
        assert_eq!(s.due(), None, "no back-to-room after the leave");
        assert_eq!(*ctx.go, None, "the lobby opens on the leave reply");
        // filled 61 s ago: due at once
        let Some(old) = Instant::now().checked_sub(Duration::from_secs(61)) else {
            return;
        };
        let data = MenuData {
            result: Some(MatchResult::sample().filled(old)),
            ..MenuData::default()
        };
        let mut s = ResultScreen::new(&cat, &data);
        assert_eq!(s.due(), Some(RESULT_CMD_EXIT));
    }
}
