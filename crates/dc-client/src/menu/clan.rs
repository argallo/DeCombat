//! w3-clan: the clan screen (2009 screen state 0x1c): the clan's record (name, leader, members, rank, level,
//! wins / draws / losses, experience), its notice, the member list (rank badge, role, name, online state), the clan
//! chat, the leader's notice box and the clan web page buttons; and the clan mark (emblem over background) the lobby's
//! Character Info panel and this screen draw.
//!
//! The screen opens from the lobby bar's clan button (command 0xa8) and asks for the record (0x3c2); each answer
//! asks for the next list as the 2009 screen does (0x3c3 -> 0x3c4, 0x3c5 -> 0x3c6). Its exit (command 0x137) goes
//! back to the lobby.
// SPEC: PROTOCOL_2009.md §5.1 (0x3c2..0x3c9, 0x3061..0x3064), §5.2 (clan web pages), §3.4 (0x308 option set)
// EVIDENCE: cshell+0x1bf20 (lobby bar command 0xa8 -> screen 0x1c), cshell+0x104960 (the screen's focus: windows 5, 6
// and 7 hidden, manage / create buttons by the own clan role, war button disabled, 0x3c2), cshell+0x1103d0 (the
// screen's commands), cshell+0x106030 (0x3c3 reader -> cshell+0x105280 fill -> 0x3c4), cshell+0x111990 (0x3c5
// reader: own role, leader controls, leader name, rows -> 0x3c6), cshell+0x105210 (0x3c7 reader -> cshell+0x103630)

use super::catalog::Catalog;
use super::clanpage::{self, CreateHit, CreatePage, LeavePage, ListKind, ListPage, PageHit};
use super::link::{
    ClanInfoView, ClanMemberRow, LinkCmd, LinkEvent, CLAN_ROLE_LEADER, CLAN_ROLE_OFFICER,
};
use super::lobby::{PanelArt, RowStyle};
use super::popup::{BoxTag, Popup};
use super::result::grouped;
use super::sheet::{image, list_capacity, list_row, place_text, Look, Part, Sheet};
use super::{build_sheet, pages, Ctx, Go, MenuData, MenuKey, Screen};
use crate::hud::items::DrawItem;
use dc_ui_data::names::{matches_pin, read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::WidgetKind;
use std::path::Path;

/// Window and control ids of the clan page.
// EVIDENCE: cshell+0x105280 (window 2: text 8 name, 0xb members, 0xc rank, 0xe level, 0xd record, 0xf experience,
// image 0 the empty mark frame, control 0x10 the mark), cshell+0x1035b0 (text 9 the leader's name), cshell+0x103630
// (text 10 the notice, wrapped at 0xdc), cshell+0x206840 (window 4 list 0 the members), cshell+0x104960 (windows 5,
// 6, 7; buttons 3 manage, 4 create, 5 find, 6 war find of window 2; chat button 0 of window 3)
pub const INFO_WINDOW: i32 = 2;
pub const TALK_WINDOW: i32 = 3;
pub const MEMBER_WINDOW: i32 = 4;
pub const WEB_WINDOW: i32 = 5;
pub const WAR_WINDOW: i32 = 6;
pub const NOTICE_WINDOW: i32 = 7;
pub const NAME_TEXT: i32 = 8;
pub const LEADER_TEXT: i32 = 9;
pub const NOTICE_LABEL: i32 = 10;
pub const MEMBERS_TEXT: i32 = 0xb;
pub const RANK_TEXT: i32 = 0xc;
pub const RECORD_TEXT: i32 = 0xd;
pub const LEVEL_LABEL: i32 = 0xe;
pub const EXP_TEXT: i32 = 0xf;
pub const MARK_FRAME: i32 = 0;
/// The clanless page's name and value strings.
// EVIDENCE: cshell+0x10c5c0
pub const TEXT_UNAFFILIATED: i32 = 0x41a;
pub const TEXT_NO_VALUE: i32 = 0x37e;
pub const MANAGE_BUTTON: i32 = 3;
pub const CREATE_BUTTON: i32 = 4;
pub const FIND_BUTTON: i32 = 5;
pub const WAR_BUTTON: i32 = 6;
pub const MEMBER_LIST: i32 = 0;
pub const TALK_FIELD: i32 = 3;
pub const CHAT_LIST: i32 = 5;
pub const WEB_TITLE: i32 = 1;
/// The notice width the screen wraps the notice text at.
// EVIDENCE: cshell+0x103630 (vtable +0x140 with 0xdc before the text)
pub const NOTICE_WRAP: i32 = 0xdc;

/// Command numbers of the clan page and of the lobby bars.
// EVIDENCE: cshell+0x1103d0 (0x137 exit: screen 9 when in a room, else 0xf; 0x71 the invite option; 0x75 the chat;
// 10000 manage, 10001 find, 10004 create, 10009 leave: the web window with their titles; 10002 / 10003 the war
// window; 10005 the web window's close; 10006 notice box (leader roles only, else box 0x4cb); 10007 notice OK (0x3c8);
// 10008 notice close), cshell+0x1bf20 (0xa8 clan, 0x136 messenger)
pub const BAR_CLAN_CMD: i32 = 0xa8;
pub const CMD_EXIT: i32 = 0x137;
pub const CMD_INVITE_OPTION: i32 = 0x71;
pub const CMD_CHAT_SEND: i32 = 0x75;
pub const CMD_MANAGE: i32 = 10000;
pub const CMD_FIND: i32 = 10001;
pub const CMD_WAR: i32 = 10002;
pub const CMD_WAR_CLOSE: i32 = 10003;
pub const CMD_CREATE: i32 = 10004;
pub const CMD_WEB_CLOSE: i32 = 10005;
pub const CMD_NOTICE: i32 = 10006;
pub const CMD_NOTICE_OK: i32 = 10007;
pub const CMD_NOTICE_CLOSE: i32 = 10008;
pub const CMD_LEAVE: i32 = 10009;

/// The web window's title string of each web page button.
// EVIDENCE: cshell+0x1103d0 (10000 -> 0x4fe, 10004 -> 0x4ff, 10001 -> 0x501, 10009 -> 0x500, through cshell+0x2a0420
// into cshell+0x203280)
pub const WEB_TITLES: [(i32, i32); 4] = [
    (CMD_MANAGE, 0x4fe),
    (CMD_CREATE, 0x4ff),
    (CMD_FIND, 0x501),
    (CMD_LEAVE, 0x500),
];
/// The box of a notice edit without a leader role, the one of a leader removed from the clan.
// EVIDENCE: cshell+0x1103d0 (10006: role not 5 / 0xf -> 0x4cb); SPEC: PROTOCOL_2009.md §5.1 0x3064 (role 5: 0x502
// on a normal server)
pub const TEXT_NOTICE_REFUSED: i32 = 0x4cb;
pub const TEXT_CLAN_REMOVED: i32 = 0x502;
/// Level names: string 0x4a5 for level 1 up to 0x4aa for level 6 (the level table's entries 1..6).
// DISASM: cshell+0x1053e4..0x1053f3 (the level's entry of the table at cshell VA 0x374de8e8 to the string reader);
// the table's words 1..6 are 0x4a5..0x4aa
pub const LEVEL_NAME_FIRST: i32 = 0x4a5;
pub const LEVEL_COUNT: u32 = 6;
/// The record line string (`wins, draws, losses, win per cent`).
// DISASM: cshell+0x105444..0x1054ea (string 0x58d with wins, draws, losses and wins x 100 / (wins + losses + draws),
// 0 when there is no match, the float truncated by cshell+0x3a5f00)
pub const RECORD_TEXT_ID: i32 = 0x58d;
/// The role strings of the member rows and the online strings.
// EVIDENCE: cshell+0x1045f0 (5 -> 0x1485, 0xf -> 0x58e, else 0x58f; online 0x590 in 0xffaaaaaa, offline 0x591 in
// 0xff5e5e5e), cshell+0x102e10
pub const ROLE_LEADER_TEXT: i32 = 0x1485;
pub const ROLE_OFFICER_TEXT: i32 = 0x58e;
pub const ROLE_MEMBER_TEXT: i32 = 0x58f;
pub const ONLINE_TEXT: i32 = 0x590;
pub const OFFLINE_TEXT: i32 = 0x591;
pub const ONLINE_COLOUR: u32 = 0xffaa_aaaa;
pub const OFFLINE_COLOUR: u32 = 0xff5e_5e5e;
pub const ROLE_COLOUR: u32 = 0xff5e_5e5e;
pub const NAME_COLOUR: u32 = 0xffaa_aaaa;
/// The member row: size and the places of its parts (role, badge, name, online state) from the row's top-left;
/// texts of size 14 left aligned and vertically centred (0x41).
// EVIDENCE: cshell+0x1045f0 (row 0x155 x 0x1f; texts size 0xe, alignment 0x41), cshell+0x1030b0 (name +0x76 / +0xe,
// role +10 / +0xe, online +0xf5 / +0xe, badge +0x49 / +5)
pub const MEMBER_ROW: [i32; 2] = [0x155, 0x1f];
pub const MEMBER_ROLE_AT: [i32; 2] = [10, 0xe];
pub const MEMBER_BADGE_AT: [i32; 2] = [0x49, 5];
pub const MEMBER_NAME_AT: [i32; 2] = [0x76, 0xe];
pub const MEMBER_ONLINE_AT: [i32; 2] = [0xf5, 0xe];
pub const MEMBER_TEXT_SIZE: i32 = 0xe;
/// The clan experience a level ends at: the experience text shows `<exp> / <first experience of the next level>`.
// EVIDENCE: cshell+0x103950 (levels 1..6: first experience 0, 0x2e0, 0x93c, 0x1ec4, 0x8134, 0x229b7; any other: 0),
// DISASM: cshell+0x105512..0x10554b (the level + 1 asked, its first experience shown)
pub const LEVEL_FIRST_EXP: [u32; 6] = [0, 0x2e0, 0x93c, 0x1ec4, 0x8134, 0x229b7];

/// The clan mark control: 26 x 26 at 2 / 2 from the empty mark frame of the lobby's Character Info panel.
// EVIDENCE: cshell+0x1211b0 (the mark control of window 7: size 0x1a x 0x1a, added at (0x37, 0x4e), the frame image 1
// of the page sits at (0x35, 0x4c)); cshell+0x343cc0 / 0x343d50 (emblem and background layers shown when their id and
// texture are set)
pub const MARK_SIZE: [i32; 2] = [0x1a, 0x1a];
pub const MARK_FROM_FRAME: [i32; 2] = [2, 2];
/// The mark texture's art: its top-left 51 x 51 texels (the texture is 64 x 64), shown on the lobby panel at the scale
/// 0x3ed2d2d3 (21 / 51), so 21 x 21, inside the 26 x 26 control.
// CLI-911: both layers were stretched whole (the padding included) over 26 x 26 / the 51 x 51 frame: a smaller mark
// pushed up and left in its frame
pub const MARK_TEXELS: [i32; 2] = [51, 51];
pub const MARK_PANEL_DRAWN: [i32; 2] = [21, 21];

/// The lobby's Character Info window and its empty mark frame (image 1).
pub const CHAR_WINDOW: i32 = 7;
pub const CHAR_MARK_FRAME: i32 = 1;

// ---------------------------------------------------------------------------------------------------------------
// pinned literals of the user's CShell.dll (read at run time; nothing of them is in the tree)
// ---------------------------------------------------------------------------------------------------------------

const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
    Pin {
        what,
        rva,
        sha16,
        len,
    }
}

/// Wide format strings of the screen: members `n / max`, level `name (Lv n)`, experience `a / b Exp`.
// EVIDENCE: cshell+0x105280 (pushed before swprintf: VA 0x374def6c, 0x374def58, 0x374def40)
pub const MEMBERS_FORM: Pin = pin("clan members form", 0x3def6c, "e08ff54fb4f9023d", 7);
pub const LEVEL_FORM: Pin = pin("clan level form", 0x3def58, "aadd66c8c837bd34", 9);
pub const EXP_FORM: Pin = pin("clan experience form", 0x3def40, "f70bb8b67c2303da", 11);
/// The rank's ordinal forms (1st, 2nd, 3rd, other).
// EVIDENCE: cshell+0x103770 (n % 10 == 1 and n % 100 != 11 -> first form, 2 / 12 -> second, 3 / 13 -> third, else
// the fourth)
pub const RANK_FORMS: [Pin; 4] = [
    pin("rank form st", 0x3dee38, "affeb3f3c98b4539", 4),
    pin("rank form nd", 0x3dee2c, "809993d533488c51", 4),
    pin("rank form rd", 0x3dee20, "2dbe68dc1df67e40", 4),
    pin("rank form th", 0x3dee14, "cdcd229f4de7f7b0", 4),
];
/// The clan mark table: its file and keys.
// EVIDENCE: cshell+0x2811e0 (the file), cshell+0x280a90 (sections `Update<n>` from 0 while present; per section the
// last emblem and background index, both folders, then per index a category (-1 = none) and a texture file, joined
// as `<folder>\<file>`), cshell+0x280570 (look-up: index >= 1 and below the count, none for an entry without a path)
pub const MARK_FILE: Pin = pin("clan mark table file", 0x3f6260, "08e034e9a275cf80", 23);
pub const MARK_KEYS: [Pin; 10] = [
    pin("clan mark section", 0x3f630c, "8faae89d0d0fb092", 8),
    pin("clan mark last emblem", 0x3f6300, "1ec1a54bb47190d9", 11),
    pin(
        "clan mark last background",
        0x3f62f0,
        "a728c705d9899673",
        15,
    ),
    pin("clan mark emblem folder", 0x3f62e0, "6f46d48e08b96117", 13),
    pin(
        "clan mark background folder",
        0x3f62cc,
        "033129e509bf3861",
        17,
    ),
    pin(
        "clan mark emblem category",
        0x3f62bc,
        "e13bfbac809f222c",
        12,
    ),
    pin("clan mark emblem texture", 0x3f62b0, "0ef144d2133ff99f", 11),
    pin(
        "clan mark background category",
        0x3f6298,
        "51bfcc587821371f",
        12,
    ),
    pin(
        "clan mark background texture",
        0x3f628c,
        "3b32cfa59eef32a8",
        11,
    ),
    pin("clan mark path join", 0x3f62a8, "c7a09c5dcba8ae69", 5),
];

/// Fills a C format string: `%d` / `%s` take the next argument in turn, `%%` is a per cent sign; anything else stays.
pub fn c_format(form: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut it = form.chars().peekable();
    let mut next = args.iter();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.peek().copied() {
            Some('%') => {
                it.next();
                out.push('%');
            }
            Some('d') | Some('s') => {
                it.next();
                if let Some(a) = next.next() {
                    out.push_str(a);
                }
            }
            _ => out.push('%'),
        }
    }
    out
}

/// What the clan screen reads from the user's CShell.dll and attribute archive.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClanLits {
    pub members: Option<String>,
    pub level: Option<String>,
    pub exp: Option<String>,
    pub rank: [Option<String>; 4],
    pub marks: ClanMarks,
}

impl ClanLits {
    /// Read once per ROM folder and process.
    pub fn cached(rom: &Path) -> ClanLits {
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::sync::{Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, ClanLits>>> = OnceLock::new();
        let mut c = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        c.entry(rom.to_path_buf())
            .or_insert_with(|| ClanLits::from_rom(rom))
            .clone()
    }

    pub fn from_rom(rom: &Path) -> ClanLits {
        let mut out = ClanLits::default();
        let Ok(pe) =
            dc_rom::PeFile::from_file(&rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]))
        else {
            return out;
        };
        let wide = |p: &Pin| {
            pe.wstr(p.rva)
                .ok()
                .filter(|s| matches_pin(s, p.sha16, p.len))
        };
        out.members = wide(&MEMBERS_FORM);
        out.level = wide(&LEVEL_FORM);
        out.exp = wide(&EXP_FORM);
        for (i, p) in RANK_FORMS.iter().enumerate() {
            out.rank[i] = wide(p);
        }
        out.marks = ClanMarks::from_pe(rom, &pe);
        out
    }

    /// `n / max` (the plain form without the ROM).
    pub fn members_text(&self, n: u32, max: u32) -> String {
        match &self.members {
            Some(f) => c_format(f, &[n.to_string(), max.to_string()]),
            None => format!("{n} / {max}"),
        }
    }

    /// The rank with its ordinal form.
    // EVIDENCE: cshell+0x103770
    pub fn rank_text(&self, n: u32) -> String {
        let form = match (n % 10, n % 100) {
            (1, r) if r != 11 => 0,
            (2, r) if r != 12 => 1,
            (3, r) if r != 13 => 2,
            _ => 3,
        };
        match &self.rank[form] {
            Some(f) => c_format(f, &[n.to_string()]),
            None => n.to_string(),
        }
    }

    /// `name (Lv n)` with the level's name string.
    pub fn level_line(&self, cat: &Catalog, level: u32) -> String {
        let name = if (1..=LEVEL_COUNT).contains(&level) {
            cat.text(LEVEL_NAME_FIRST + level as i32 - 1)
        } else {
            String::new()
        };
        match &self.level {
            Some(f) => c_format(f, &[name, level.to_string()]),
            None => format!("{name} ({level})"),
        }
    }

    /// `<exp> / <first experience of the next level>` with thousands separators.
    pub fn exp_text(&self, exp: u32, level: u32) -> String {
        let next = level_first_exp(level + 1);
        let (a, b) = (grouped(i64::from(exp)), grouped(i64::from(next)));
        match &self.exp {
            Some(f) => c_format(f, &[a, b]),
            None => format!("{a} / {b}"),
        }
    }
}

/// The first clan experience of `level` (0 outside 1..6).
// EVIDENCE: cshell+0x103950
pub fn level_first_exp(level: u32) -> u32 {
    if (1..=LEVEL_COUNT).contains(&level) {
        LEVEL_FIRST_EXP[level as usize - 1]
    } else {
        0
    }
}

/// The record line: wins, draws, losses and the win per cent (string 0x58d).
pub fn record_text(cat: &Catalog, v: &ClanInfoView) -> String {
    let all = u64::from(v.wins) + u64::from(v.losses) + u64::from(v.draws);
    let pct = if all == 0 {
        0
    } else {
        (f64::from(v.wins) * 100.0 / all as f64) as u64
    };
    let form = cat.text(RECORD_TEXT_ID);
    let args = [
        v.wins.to_string(),
        v.draws.to_string(),
        v.losses.to_string(),
        pct.to_string(),
    ];
    if form.contains('%') {
        c_format(&form, &args)
    } else {
        // DC-DEFAULT: CLI-566 (proposed) without the ROM string the record line is `W-D-L (pct%)`
        format!("{}-{}-{} ({}%)", args[0], args[1], args[2], args[3])
    }
}

/// The clan mark table: emblem and background textures by index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClanMarks {
    /// Index -> texture path (`None`: category -1, no mark).
    pub emblems: Vec<Option<String>>,
    pub backgrounds: Vec<Option<String>>,
    /// use-items-emblem: index -> the entry's category (-1 none, 0 not on a tab, 1..6 the tab), for the mark item
    /// window's tabs.
    // EVIDENCE: cshell+0x280a90 (per entry `<kind>Category<n>`: -1 an empty entry, 0 only in the look-up list, k >= 1
    // in tab 0 (all) and tab k), cshell+0x27fb90 (a tab's list)
    pub emblem_cats: Vec<i32>,
    pub background_cats: Vec<i32>,
}

impl ClanMarks {
    fn from_pe(rom: &Path, pe: &dc_rom::PeFile) -> ClanMarks {
        let mut out = ClanMarks::default();
        let Ok(file) = read_pinned(pe, &MARK_FILE) else {
            return out;
        };
        let keys: Vec<String> = MARK_KEYS
            .iter()
            .filter_map(|p| read_pinned(pe, p).ok())
            .collect();
        if keys.len() != MARK_KEYS.len() {
            return out;
        }
        let vfs = dc_rez::RezVfs::new(rom);
        let Some(doc) = dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", &file) else {
            return out;
        };
        let clean = |v: &str| v.trim().trim_matches('"').trim().to_string();
        let join = |folder: &str, file: &str| c_format(&keys[9], &[folder.into(), file.into()]);
        let (mut e0, mut b0) = (0usize, 0usize);
        for n in 0..64 {
            let sec = c_format(&keys[0], &[n.to_string()]);
            let Some(m) = doc.get(&sec) else {
                break;
            };
            let get = |k: &str| m.get(k).map(|v| clean(v)).unwrap_or_default();
            let last = |k: &str| get(k).parse::<i64>().unwrap_or(-1);
            let (e_end, b_end) = (last(&keys[1]), last(&keys[2]));
            let (e_dir, b_dir) = (get(&keys[3]), get(&keys[4]));
            let read = |list: &mut Vec<Option<String>>,
                        cats: &mut Vec<i32>,
                        from: usize,
                        end: i64,
                        dir: &str,
                        cat_key: &str,
                        tex_key: &str|
             -> usize {
                let end = (end + 1).clamp(0, 4096) as usize;
                for i in from..end {
                    let category = get(&c_format(cat_key, &[i.to_string()]));
                    let tex = get(&c_format(tex_key, &[i.to_string()]));
                    let path = (category != "-1" && !tex.is_empty()).then(|| join(dir, &tex));
                    if list.len() <= i {
                        list.resize(i + 1, None);
                        cats.resize(i + 1, -1);
                    }
                    cats[i] = if path.is_some() {
                        category.parse().unwrap_or(0)
                    } else {
                        -1
                    };
                    list[i] = path;
                }
                end.max(from)
            };
            e0 = read(
                &mut out.emblems,
                &mut out.emblem_cats,
                e0,
                e_end,
                &e_dir,
                &keys[5],
                &keys[6],
            );
            b0 = read(
                &mut out.backgrounds,
                &mut out.background_cats,
                b0,
                b_end,
                &b_dir,
                &keys[7],
                &keys[8],
            );
        }
        out
    }

    /// The emblem and background textures of a mark (`None` unless both exist).
    // EVIDENCE: cshell+0x280570 (index >= 1, below the count, an entry with a path)
    pub fn textures(&self, emblem: u32, background: u32) -> Option<(&str, &str)> {
        Some((self.emblem(emblem)?, self.background(background)?))
    }

    /// The texture of one layer: emblem `i` / background `i` (`None` below 1, past the list or for an empty entry).
    // EVIDENCE: cshell+0x280570 (index >= 1, below the count, an entry with a path)
    pub fn emblem(&self, i: u32) -> Option<&str> {
        Self::pick(&self.emblems, i)
    }

    pub fn background(&self, i: u32) -> Option<&str> {
        Self::pick(&self.backgrounds, i)
    }

    fn pick(v: &[Option<String>], i: u32) -> Option<&str> {
        if i < 1 {
            return None;
        }
        v.get(i as usize).and_then(|p| p.as_deref())
    }
}

/// The mark drawn at `frame`'s place: the background, then the emblem over it, each stretched over the mark size.
// EVIDENCE: cshell+0x126fa0 / cshell+0x105280 (both layers set, the frame hidden); DC-DEFAULT: CLI-561 (proposed) the
// background is drawn under the emblem
pub fn mark_items(
    _cat: &mut Catalog,
    marks: &ClanMarks,
    frame_pos: [i32; 2],
    emblem: u32,
    background: u32,
    out: &mut Vec<DrawItem>,
) -> bool {
    let Some((e, b)) = marks.textures(emblem, background) else {
        return false;
    };
    let at = [
        frame_pos[0] + MARK_FROM_FRAME[0],
        frame_pos[1] + MARK_FROM_FRAME[1],
    ];
    let r = [at[0], at[1], MARK_PANEL_DRAWN[0], MARK_PANEL_DRAWN[1]];
    super::clanpage::mark_cropped(Some(b), Some(e), r, out);
    true
}

/// The lobby panel's mark (CLI-389): with a clan and both mark ids the frame is hidden and the mark drawn at its place.
pub fn draw_panel_mark(cat: &mut Catalog, sheet: &Sheet, data: &MenuData, out: &mut Vec<DrawItem>) {
    let x = &data.char_extras;
    if x.clan_id == 0 || x.clan_mark == 0 || x.clan_mark_ground == 0 {
        return;
    }
    let Some(frame) = sheet.part(CHAR_WINDOW, CHAR_MARK_FRAME, WidgetKind::Image) else {
        return;
    };
    let pos = frame.pos;
    let Some(rom) = cat.rom.clone() else {
        return;
    };
    let lits = ClanLits::cached(&rom);
    mark_items(cat, &lits.marks, pos, x.clan_mark, x.clan_mark_ground, out);
}

// ---------------------------------------------------------------------------------------------------------------
// shared clan data
// ---------------------------------------------------------------------------------------------------------------

/// What the menu keeps of the clan between screens (the replies may arrive on any screen).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClanState {
    pub info: Option<ClanInfoView>,
    pub members: Vec<ClanMemberRow>,
    pub notice: String,
    /// Clan chat lines (name, text), the last [`CLAN_CHAT_KEEP`].
    pub chat: Vec<(String, String)>,
}

/// How many clan chat lines are kept.
// DC-DEFAULT: CLI-562 (proposed) the clan chat keeps its last 64 lines, newest at the bottom
pub const CLAN_CHAT_KEEP: usize = 64;

impl ClanState {
    pub fn say(&mut self, name: &str, text: &str) {
        self.chat.push((name.to_string(), text.to_string()));
        let n = self.chat.len();
        if n > CLAN_CHAT_KEEP {
            self.chat.drain(..n - CLAN_CHAT_KEEP);
        }
    }
}

/// The clan parts of the events every screen sees.
pub fn shared_event(d: &mut MenuData, e: &LinkEvent) {
    match e {
        LinkEvent::ClanInfo(v) => d.clan.info = Some(v.clone()),
        LinkEvent::ClanMembers(v) => {
            // the own row's role is the own clan role
            // EVIDENCE: cshell+0x111990 (the row of the own character number -> shell +0x128)
            if let Some(me) = v.iter().find(|m| m.char_no == d.profile.char_no) {
                d.char_extras.clan_role = me.role;
            }
            d.clan.members = v.clone();
        }
        LinkEvent::ClanNotice(t) => d.clan.notice = t.clone(),
        LinkEvent::CharInfo(x) if x.clan_id == 0 => {
            d.clan.info = None;
            d.clan.members.clear();
            d.clan.notice.clear();
        }
        // DeCombat's clan chat lines (`clan`, name, text) of the messenger relay
        LinkEvent::Messenger(f) if f.first().map(String::as_str) == Some(CLAN_LINE) => {
            let get = |i: usize| f.get(i).map_or("", String::as_str);
            d.clan.say(get(1), get(2));
        }
        _ => {}
    }
}

/// The relay's clan chat kind and the command that sends one.
// DC-DEFAULT: CLI-563 (proposed) the clan chat goes through the DeCombat messenger relay: `!nm clan<TAB>text` sent,
// `clan<TAB>name<TAB>text` lines to every online member (the 2009 client sends it through its external messenger)
pub const CLAN_LINE: &str = "clan";

pub fn clan_chat_command(text: &str) -> String {
    format!("!nm clan\t{text}")
}

// ---------------------------------------------------------------------------------------------------------------
// the screen
// ---------------------------------------------------------------------------------------------------------------

/// The notice box's text field (window 7): a run-time edit control of the 2009 screen; here a part of the sheet with
/// the room of the notice background image.
// EVIDENCE: cshell+0x1103d0 (10007 reads the field through cshell+0x3452d0 into a 0x800-byte buffer, then 0x3c8),
// cshell+0x203370 (the box shows with the field reset, cshell+0x3453e0); SPEC: PROTOCOL_2009.md §5.1 0x3c8 (wstr[121])
// DC-DEFAULT: CLI-564 (proposed, amended by CLI-841) the field is the notice image's rectangle, its text wrapped
// inside it (the reference stack's multi-line box), 80 characters (the reference stack's counter; the 0x3c8 buffer holds 121), font 14
// size 14, white, starting with the current notice; the edit part itself is not drawn (the screen draws the wrapped
// lines and the "n / 80" counter), it only takes the keys
pub const NOTICE_EDIT_AT: [i32; 2] = [8, 3];
/// The width the notice field wraps at (OBSERVED: the reference stack breaks its lines at about 358 px).
pub const NOTICE_WRAP_W: f32 = 358.0;
pub const NOTICE_MAX: usize = 80;
/// The notice box's counter (window 7 text 3: "n / 80", right-aligned at the image's right edge).
pub const NOTICE_COUNT: i32 = 3;
/// The notice field's text colour (white) and the line pitch of its wrapped lines.
pub const NOTICE_COLOUR: u32 = 0xffff_ffff;
pub const NOTICE_LINE: i32 = 15;
/// The notice image of window 7.
pub const NOTICE_IMAGE: i32 = 0;

/// The page the web window shows.
#[derive(Debug, Clone, PartialEq)]
pub enum WebPage {
    None,
    Create(CreatePage),
    List(ListPage),
    Leave(LeavePage),
}

pub struct ClanScreen {
    sheet: Sheet,
    lits: ClanLits,
    style: RowStyle,
    art: Option<PanelArt>,
    /// The sheet part of the notice field.
    notice_field: Option<usize>,
    /// The invite option of the chat window (the shell flag bit 2: invites refused).
    invite_off: bool,
    /// clan-functions: the page the web window shows (our own pages, CLI-791) and the create page's name field.
    pub web: WebPage,
    create_field: Option<usize>,
    /// CLI-1896: the find page's search field.
    search_field: Option<usize>,
    back: Go,
}

impl ClanScreen {
    pub fn new(cat: &Catalog, data: &MenuData) -> ClanScreen {
        let mut sheet = build_sheet(cat, &[pages::LOBBY_COMMON, pages::CLAN]);
        // CLI-1260: the lobby bar stays live while a modal window of the page is up
        sheet.modal_free = Some(pages::LOBBY_COMMON);
        let notice_field = add_notice_edit(&mut sheet);
        let create_field = clanpage::add_name_field(&mut sheet, WEB_WINDOW, pages::CLAN);
        let search_field = clanpage::add_search_field(&mut sheet, WEB_WINDOW, pages::CLAN);
        let mut s = ClanScreen {
            sheet,
            lits: cat.rom.as_deref().map(ClanLits::cached).unwrap_or_default(),
            style: RowStyle::load(cat),
            art: cat.rom.as_deref().and_then(PanelArt::cached),
            notice_field,
            invite_off: false,
            web: WebPage::None,
            create_field,
            search_field,
            back: Go::Lobby,
        };
        s.sheet.set_wrap(INFO_WINDOW, NOTICE_LABEL, NOTICE_WRAP);
        s.reset_windows(data);
        // what the menu kept from earlier replies shows at once (the requests of the focus refresh it)
        s.fill(cat, data);
        if !data.clan.members.is_empty() {
            s.fill_members(data);
        }
        if !data.clan.notice.is_empty() {
            s.sheet
                .set_text(INFO_WINDOW, NOTICE_LABEL, &data.clan.notice);
        }
        s
    }

    /// The screen's focus: the web, war and notice windows hidden, the manage / create buttons by the own role, the
    /// war button disabled.
    // EVIDENCE: cshell+0x104960
    fn reset_windows(&mut self, data: &MenuData) {
        for w in [WEB_WINDOW, WAR_WINDOW, NOTICE_WINDOW] {
            self.sheet.show_window(w, false);
        }
        self.set_manage(in_clan(data));
        for (id, on) in [
            (WAR_BUTTON, false),
            (CREATE_BUTTON, true),
            (MANAGE_BUTTON, true),
            (FIND_BUTTON, true),
        ] {
            self.enable(id, on);
        }
    }

    /// The manage button (with a clan) or the create button (without).
    // EVIDENCE: cshell+0x1032e0 (one shown and enabled, the other hidden and disabled)
    fn set_manage(&mut self, manage: bool) {
        self.sheet
            .show(INFO_WINDOW, CREATE_BUTTON, WidgetKind::Button, !manage);
        self.sheet
            .show(INFO_WINDOW, MANAGE_BUTTON, WidgetKind::Button, manage);
        self.enable(CREATE_BUTTON, !manage);
        self.enable(MANAGE_BUTTON, manage);
    }

    fn enable(&mut self, id: i32, on: bool) {
        if let Some(p) = self.sheet.part_mut(INFO_WINDOW, id, WidgetKind::Button) {
            p.enabled = on;
        }
    }

    /// The record's texts; without a record the clanless page: the name "(Unaffiliated)" (0x41a), texts 9 .. 0xf
    /// (leader, notice, members, rank, record, level, experience) "-" (0x37e).
    // EVIDENCE: cshell+0x105280 (the record), cshell+0x10c5c0 (the clanless page: text (2, 8) string 0x41a, texts
    // (2, 9) .. (2, 0xf) string 0x37e, the mark control hidden, the frame image shown)
    // DC-DEFAULT: CLI-746 the clanless texts show whenever the screen holds no clan record (the 2009 calls of
    // cshell+0x10c5c0 seen are the clan-removed paths; the reference stack picture shows them on open for a clanless account)
    fn fill(&mut self, cat: &Catalog, data: &MenuData) {
        let s = &mut self.sheet;
        let Some(v) = &data.clan.info else {
            s.set_text(INFO_WINDOW, NAME_TEXT, &cat.text(TEXT_UNAFFILIATED));
            let dash = cat.text(TEXT_NO_VALUE);
            for id in LEADER_TEXT..=EXP_TEXT {
                s.set_text(INFO_WINDOW, id, &dash);
            }
            self.fill_mark_frame(data);
            return;
        };
        s.set_text(INFO_WINDOW, NAME_TEXT, &v.name);
        s.set_text(
            INFO_WINDOW,
            MEMBERS_TEXT,
            &self.lits.members_text(v.members, v.max_members),
        );
        s.set_text(INFO_WINDOW, RANK_TEXT, &self.lits.rank_text(v.rank));
        s.set_text(
            INFO_WINDOW,
            LEVEL_LABEL,
            &self.lits.level_line(cat, v.level),
        );
        s.set_text(INFO_WINDOW, RECORD_TEXT, &record_text(cat, v));
        s.set_text(INFO_WINDOW, EXP_TEXT, &self.lits.exp_text(v.exp, v.level));
        self.fill_mark_frame(data);
    }

    /// The empty frame shows unless the mark is drawn.
    fn fill_mark_frame(&mut self, data: &MenuData) {
        let x = &data.char_extras;
        let marked = x.clan_mark != 0
            && x.clan_mark_ground != 0
            && self
                .lits
                .marks
                .textures(x.clan_mark, x.clan_mark_ground)
                .is_some();
        self.sheet
            .show(INFO_WINDOW, MARK_FRAME, WidgetKind::Image, !marked);
    }

    /// The member list's texts: the leader's name, the manage controls of the own role.
    // EVIDENCE: cshell+0x111990, cshell+0x1035b0
    fn fill_members(&mut self, data: &MenuData) {
        let leader = data
            .clan
            .members
            .iter()
            .find(|m| m.role == CLAN_ROLE_LEADER)
            .map_or("", |m| m.name.as_str())
            .to_string();
        self.sheet.set_text(INFO_WINDOW, LEADER_TEXT, &leader);
        if let Some(me) = data
            .clan
            .members
            .iter()
            .find(|m| m.char_no == data.profile.char_no)
        {
            if matches!(me.role, CLAN_ROLE_LEADER | CLAN_ROLE_OFFICER) {
                self.set_manage(true);
            } else if data.char_extras.clan_id == 0 {
                self.set_manage(false);
            } else {
                self.set_manage(true);
                self.enable(MANAGE_BUTTON, false);
            }
        }
    }

    fn typed_talk(&self) -> String {
        self.sheet
            .part(TALK_WINDOW, TALK_FIELD, WidgetKind::EditBox)
            .map(|p| p.text.trim().to_string())
            .unwrap_or_default()
    }

    fn send_chat(&mut self, ctx: &mut Ctx) {
        let t = self.typed_talk();
        if t.is_empty() {
            return;
        }
        ctx.send(LinkCmd::Messenger(clan_chat_command(&t)));
        if let Some(p) = self
            .sheet
            .part_mut(TALK_WINDOW, TALK_FIELD, WidgetKind::EditBox)
        {
            p.text.clear();
        }
    }

    fn open_notice(&mut self, data: &MenuData) {
        self.sheet.show_window(NOTICE_WINDOW, true);
        // CLI-841: modal like the web window (the reference stack's box takes the clicks over the clan screen)
        self.sheet.modal = Some(NOTICE_WINDOW);
        if let Some(i) = self.notice_field {
            self.sheet.parts[i].text = data.clan.notice.chars().take(NOTICE_MAX).collect();
        }
        self.sheet.focus(self.notice_field);
    }

    fn close_notice(&mut self) {
        self.sheet.show_window(NOTICE_WINDOW, false);
        self.sheet.modal = None;
        if self.sheet.focused() == self.notice_field {
            self.sheet.focus(None);
        }
    }

    fn window_shown(&self, w: i32) -> bool {
        self.sheet
            .part(w, w, WidgetKind::Window)
            .is_some_and(|p| p.shown)
    }

    /// The notice field's typed text.
    pub fn notice_input(&self) -> Option<&str> {
        self.notice_field.map(|i| self.sheet.parts[i].text.as_str())
    }
}

/// Adds the notice box's field to the sheet (under window 7, at the notice image).
fn add_notice_edit(sheet: &mut Sheet) -> Option<usize> {
    let w = sheet.parts.iter().position(|p| {
        p.kind == WidgetKind::Window && p.id == NOTICE_WINDOW && p.page == pages::CLAN
    })?;
    let img = sheet.part(NOTICE_WINDOW, NOTICE_IMAGE, WidgetKind::Image)?;
    let at = [
        img.pos[0] + NOTICE_EDIT_AT[0],
        img.pos[1] + NOTICE_EDIT_AT[1],
    ];
    let size = [
        img.size[0] - 2 * NOTICE_EDIT_AT[0],
        img.size[1] - 2 * NOTICE_EDIT_AT[1],
    ];
    let part = Part {
        page: pages::CLAN,
        kind: WidgetKind::EditBox,
        layer: NOTICE_WINDOW,
        id: -1,
        command: 0,
        pos: at,
        size,
        look: Look::Edit {
            font: 14,
            size: 14,
            max_len: NOTICE_MAX,
            caret_colour: 0xffff_ffff,
            caret_period: 0.333,
            // w2-lobbychat's fields (merge): clipped to the box, the default text colour
            clip: size[0],
            text_colour: NOTICE_COLOUR,
        },
        parent: Some(w),
        // CLI-841: not drawn by the sheet (the screen draws the wrapped lines), still the focus target
        shown: false,
        enabled: true,
        text: String::new(),
        checked: false,
        masked: false,
        hover: false,
        down: false,
        selected: false,
        focus: false,
        hit: Some([at[0], at[1], size[0], size[1]]),
        text_at: [0, 0],
        rec: usize::MAX,
        wrap: 0,
        wrap_pitch: 0,
        wrap_max: 0,
        colour_set: false,
        placed: true,
        blink: 0.0,
    };
    sheet.parts.push(part);
    Some(sheet.parts.len() - 1)
}

impl Screen for ClanScreen {
    fn list_follow(&self) -> Vec<(i32, i32)> {
        vec![(TALK_WINDOW, CHAT_LIST)]
    }
    fn list_rows(&self, data: &MenuData) -> Vec<(i32, i32, usize, i32)> {
        vec![
            (
                MEMBER_WINDOW,
                MEMBER_LIST,
                data.clan.members.len(),
                MEMBER_ROW[1],
            ),
            (
                TALK_WINDOW,
                CHAT_LIST,
                data.clan.chat.len(),
                super::lobby::CHAT_ROW,
            ),
        ]
    }
    fn name(&self) -> &'static str {
        "clan"
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    fn enter(&mut self, ctx: &mut Ctx) {
        // EVIDENCE: cshell+0x104960 (cshell+0x18d630: 0x3c2)
        ctx.send(LinkCmd::ClanInfo);
    }

    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        match k {
            MenuKey::Enter if self.window_shown(NOTICE_WINDOW) => {
                self.click(CMD_NOTICE_OK, usize::MAX, ctx)
            }
            MenuKey::Enter if matches!(self.web, WebPage::Create(_)) => self.send_create(ctx),
            // CLI-1896: Enter on the find page searches for the field's text
            MenuKey::Enter if self.window_shown(WEB_WINDOW) && self.web_field_focused() => {
                self.search(ctx)
            }
            MenuKey::Enter => self.send_chat(ctx),
            // the menu window delivers Space as a key (CLI-483): a space in the focused field of the screen (the
            // chat line, the notice box, the create page's name, the find page's search)
            // DC-DEFAULT: CLI-1895 (proposed; OWNER REPORT 2026-11-05 "clan announcements don't allow for spaces")
            MenuKey::Space => {
                self.sheet.type_char(' ');
            }
            // DC-DEFAULT: CLI-567 (proposed) Escape closes the open notice / web / war window, else leaves the screen
            // as its exit button does
            MenuKey::Escape => {
                if self.window_shown(NOTICE_WINDOW) {
                    self.close_notice();
                } else if self.window_shown(WEB_WINDOW) {
                    self.close_web();
                } else if self.window_shown(WAR_WINDOW) {
                    self.sheet.show_window(WAR_WINDOW, false);
                } else {
                    self.click(CMD_EXIT, usize::MAX, ctx);
                }
            }
            _ => {}
        }
    }

    fn click(&mut self, command: i32, _part: usize, ctx: &mut Ctx) {
        match command {
            CMD_EXIT => *ctx.go = Some(self.back),
            // the invite option (shell flag bit 2): 0x308 index 1 with the new flag
            // EVIDENCE: cshell+0x1103d0 (0x71: flag = bit 2 clear, 0x308 (1, flag), the bit follows)
            CMD_INVITE_OPTION => {
                let flag = u32::from(!self.invite_off);
                ctx.send(LinkCmd::OptionSet { index: 1, flag });
                self.invite_off = !self.invite_off;
            }
            CMD_CHAT_SEND => self.send_chat(ctx),
            CMD_MANAGE | CMD_CREATE | CMD_FIND | CMD_LEAVE => self.open_page(command, ctx),
            CMD_WEB_CLOSE => self.close_web(),
            CMD_WAR | CMD_WAR_CLOSE => {
                let on = !self.window_shown(WAR_WINDOW);
                self.sheet.show_window(WAR_WINDOW, on);
            }
            CMD_NOTICE => {
                let role = ctx.data.char_extras.clan_role;
                if matches!(role, CLAN_ROLE_LEADER | CLAN_ROLE_OFFICER) {
                    self.open_notice(ctx.data);
                } else {
                    let t = ctx.cat.text(TEXT_NOTICE_REFUSED);
                    ctx.popups.push(Popup::ok(t, BoxTag::None));
                }
            }
            CMD_NOTICE_OK => {
                let t = self.notice_input().unwrap_or_default().trim().to_string();
                ctx.send(LinkCmd::ClanNoticeSet(t));
                self.close_notice();
            }
            CMD_NOTICE_CLOSE => self.close_notice(),
            // CLI-1260: the lobby bar on the clan screen works as on the lobby and the shop family's pages (user round
            // 14: Shop and the other bar buttons did nothing until Escape left the screen; the top-right X too)
            // EVIDENCE: cshell+0x1bf20 (the bar's commands are the common handler's, the same on every page)
            super::shop::CMD_BAR_QUIT => {
                let t = ctx.cat.text(super::popup::text_ids::ASK_QUIT);
                ctx.popups.push(Popup::ask(t, BoxTag::Quit));
            }
            c => {
                super::shop::bar_command(c, self.back, ctx);
            }
        }
    }

    fn press(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        if !self.window_shown(WEB_WINDOW) {
            return false;
        }
        let me = ctx.data.profile.char_no;
        match &mut self.web {
            WebPage::None => false,
            WebPage::Create(c) => {
                let Some(h) = c.hit(p) else {
                    return false;
                };
                match h {
                    CreateHit::Field => self.sheet.focus(self.create_field),
                    CreateHit::Create => self.send_create(ctx),
                    _ => c.press(&self.lits.marks, h),
                }
                true
            }
            WebPage::List(l) => {
                let Some(h) = l.hit(p, me) else {
                    return false;
                };
                let user = ctx.data.user.clone();
                let target = match h {
                    PageHit::Field => {
                        self.sheet.focus(self.search_field);
                        return true;
                    }
                    // CLI-1901: the manage page's Invite sends the field's name as the find page's Search sends its text
                    PageHit::Search | PageHit::Invite => {
                        self.search(ctx);
                        return true;
                    }
                    PageHit::Refresh => Some(l.ask(&user)),
                    PageHit::Row(i) if !l.busy => l.row_request(i, &user),
                    PageHit::Accept(i) if !l.busy => l.answer_request(i, true, &user),
                    PageHit::Decline(i) if !l.busy => l.answer_request(i, false, &user),
                    PageHit::Second => {
                        self.open_page(CMD_CREATE, ctx);
                        return true;
                    }
                    _ => None,
                };
                if let Some(t) = target {
                    l.busy = true;
                    l.message = None;
                    if !web_get(ctx, t) {
                        l.busy = false;
                        l.message = Some(clanpage::T_UNREACHABLE.into());
                    }
                }
                true
            }
            WebPage::Leave(l) => {
                let Some(h) = l.hit(p) else {
                    return false;
                };
                match h {
                    PageHit::Leave if !l.busy => {
                        l.busy = true;
                        l.message = None;
                        if !web_get(ctx, LeavePage::request(&ctx.data.user)) {
                            l.busy = false;
                            l.message = Some(clanpage::T_UNREACHABLE.into());
                        }
                    }
                    PageHit::Cancel => self.close_web(),
                    _ => {}
                }
                true
            }
        }
    }

    fn hover(&mut self, p: [f32; 2], data: &MenuData) {
        let me = data.profile.char_no;
        match &mut self.web {
            WebPage::Create(c) => c.hover = c.hit(p),
            WebPage::List(l) => l.hover = l.hit(p, me),
            WebPage::Leave(l) => l.hover = l.hit(p),
            WebPage::None => {}
        }
    }

    fn event(&mut self, e: &LinkEvent, ctx: &mut Ctx) {
        match e {
            LinkEvent::ClanWeb { target, body } if target.starts_with("/clan/create") => {
                self.created(body.as_deref(), ctx)
            }
            LinkEvent::ClanWeb { target, body } => self.web_answer(target, body.as_deref(), ctx),
            // EVIDENCE: cshell+0x106030 (the record, then 0x3c4)
            LinkEvent::ClanInfo(_) => {
                self.fill(ctx.cat, ctx.data);
                ctx.send(LinkCmd::ClanMembers);
            }
            // EVIDENCE: cshell+0x111990 (the rows, then 0x3c6)
            LinkEvent::ClanMembers(_) => {
                self.fill_members(ctx.data);
                ctx.send(LinkCmd::ClanNotice);
            }
            LinkEvent::ClanNotice(t) => self.sheet.set_text(INFO_WINDOW, NOTICE_LABEL, t),
            // EVIDENCE: cshell+0x35980 (0x3062 on the clan screen: 0x3c4), cshell+0x35940 (0x3063: 0x3c6)
            LinkEvent::ClanStale(id) if *id == dc_wire::CLAN_MEMBERS_STALE_PUSH => {
                ctx.send(LinkCmd::ClanMembers)
            }
            LinkEvent::ClanStale(_) => ctx.send(LinkCmd::ClanNotice),
            // SPEC: PROTOCOL_2009.md §5.1 0x3064 (role 5 on the clan screen: box 0x502 on a normal server)
            LinkEvent::ClanRemoved => {
                if ctx.data.char_extras.clan_role == CLAN_ROLE_LEADER {
                    let t = ctx.cat.text(TEXT_CLAN_REMOVED);
                    ctx.popups.push(Popup::ok(t, BoxTag::None));
                }
            }
            LinkEvent::CharInfo(_) => {
                self.reset_buttons_only(ctx.data);
                self.fill(ctx.cat, ctx.data);
                if ctx.data.char_extras.clan_id != 0 {
                    ctx.send(LinkCmd::ClanInfo);
                }
            }
            _ => {}
        }
    }

    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, secs: f32, out: &mut Vec<DrawItem>) {
        // the mark at the frame's place
        if let Some(frame) = self.sheet.part(INFO_WINDOW, MARK_FRAME, WidgetKind::Image) {
            let x = &data.char_extras;
            // clan-functions: on this screen the mark covers the frame's rectangle (51 x 51), not the lobby's
            // 26 x 26
            // DC-DEFAULT: CLI-792 (proposed) the size of the clan screen's mark control (2, 0x10) is not traced (it
            // is not one of the page's controls and its maker was not found); OBSERVED: the reference stack draws the mark
            // over the whole frame
            if x.clan_id != 0 {
                if let Some((e, b)) = self.lits.marks.textures(x.clan_mark, x.clan_mark_ground) {
                    let r = [frame.pos[0], frame.pos[1], frame.size[0], frame.size[1]];
                    clanpage::mark_cropped(Some(b), Some(e), r, out);
                }
            }
        }
        self.draw_members(cat, data, out);
        self.draw_chat(data, out);
        if self.window_shown(NOTICE_WINDOW) {
            // CLI-841: the notice window over the chat lines too, then its field's lines and counter
            self.sheet.draw_layer(NOTICE_WINDOW, cat, secs, out);
            self.draw_notice_field(cat, secs, out);
        }
        if self.window_shown(WEB_WINDOW) {
            // clan-modal: the web window is the last window of the page, so the member rows and chat lines (which
            // the 2009 lists draw inside their own windows) never cover it; it takes every press while it is up
            // EVIDENCE: ROM clan page layout (window 5 after windows 3 talk and 4 member in creation order),
            // cshell+0x1103d0 (10000 / 10001 / 10004 / 10009 open window 5)
            // DC-DEFAULT: CLI-840 (proposed) modal: presses outside window 5 hit nothing (2009 modality is not traced)
            self.sheet.draw_layer(WEB_WINDOW, cat, secs, out);
            match &self.web {
                WebPage::Create(c) => c.draw(cat, &self.sheet, &self.lits.marks, secs, out),
                WebPage::List(l) => {
                    l.draw(data.profile.char_no, out);
                    l.draw_search(&self.sheet, secs, out);
                }
                WebPage::Leave(l) => l.draw(out),
                WebPage::None => {}
            }
        }
    }
}

/// `text` cut into lines of at most `width` pixels, breaking between characters (a line may end in a space).
fn wrap_chars(text: &str, width: f32, measure: &mut dyn FnMut(&str) -> f32) -> Vec<String> {
    let mut lines = vec![String::new()];
    for c in text.chars() {
        let cur = lines.last_mut().unwrap();
        cur.push(c);
        if measure(cur) > width && cur.chars().count() > 1 {
            cur.pop();
            lines.push(c.to_string());
        }
    }
    lines
}

/// Whether the character is in a clan by the screen's own state: a leader role, or a clan number from 0x303.
// EVIDENCE: cshell+0x104960 (roles 5 / 0xf at shell +0x128, else a non-zero clan number at shell +0x124: the manage
// button shown, the create button hidden through cshell+0x1032e0)
fn in_clan(data: &MenuData) -> bool {
    let x = &data.char_extras;
    matches!(x.clan_role, CLAN_ROLE_LEADER | CLAN_ROLE_OFFICER) || x.clan_id != 0
}

/// Sends a clan page request; false without a page address.
fn web_get(ctx: &mut Ctx, target: String) -> bool {
    match clanpage::web_addr(ctx.data.auth) {
        Some(addr) => {
            ctx.send(LinkCmd::ClanWeb { addr, target });
            true
        }
        None => false,
    }
}

impl ClanScreen {
    /// The notice field's text wrapped into the notice image's rectangle with the caret after it, and the counter.
    // DC-DEFAULT: CLI-841 (proposed) wrapped at the field's width by characters (OBSERVED in the reference stack: a multi-line box
    // and "n / 80" at its bottom right)
    fn draw_notice_field(&self, cat: &mut Catalog, secs: f32, out: &mut Vec<DrawItem>) {
        let Some(f) = self.notice_field.map(|i| &self.sheet.parts[i]) else {
            return;
        };
        let Look::Edit {
            font,
            size,
            caret_colour,
            caret_period,
            text_colour,
            ..
        } = f.look
        else {
            return;
        };
        let lines = wrap_chars(&f.text, NOTICE_WRAP_W, &mut |t| {
            cat.measure_text(font, size, t).map_or(0.0, |m| m.0)
        });
        let rows = (f.size[1] / NOTICE_LINE).max(1) as usize;
        for (k, l) in lines.iter().take(rows).enumerate() {
            if !l.is_empty() {
                out.push(place_text(
                    l,
                    [f.pos[0], f.pos[1] + k as i32 * NOTICE_LINE],
                    [0, 0],
                    Align::default(),
                    font,
                    size,
                    text_colour,
                ));
            }
        }
        if f.focus && super::sheet::caret_on(secs, caret_period) {
            let k = (lines.len().saturating_sub(1)).min(rows - 1);
            let w = lines.get(k).map_or(0.0, |l| {
                cat.measure_text(font, size, l).map_or(0.0, |m| m.0)
            });
            out.push(DrawItem::Fill {
                rect: [
                    f.pos[0] as f32 + w,
                    (f.pos[1] + k as i32 * NOTICE_LINE) as f32,
                    super::sheet::CARET_WIDTH as f32,
                    size as f32,
                ],
                colour: super::catalog::argb(caret_colour),
            });
        }
        if let Some(c) = self
            .sheet
            .part(NOTICE_WINDOW, NOTICE_COUNT, WidgetKind::Text)
        {
            if let Look::Text {
                align,
                font,
                size,
                colour,
            } = c.look
            {
                let t = format!("{} / {}", f.text.chars().count(), NOTICE_MAX);
                out.push(place_text(&t, c.pos, c.size, align, font, size, colour));
            }
        }
    }

    /// A web page button: the web window with the page's 2009 title and our page in it (CLI-791 / CLI-793).
    // DC-DEFAULT: CLI-568 (amended by CLI-791) DeCombat has no embedded browser: the web window opens with its title
    // and the client draws our own page in it
    fn open_page(&mut self, command: i32, ctx: &mut Ctx) {
        // the 2009 screen never sends 10004 for a member (its create button is hidden, cshell+0x104960); ours drops a
        // create command that still arrives for one (find page, steps, a stale press)
        // DC-DEFAULT: CLI-1889 (proposed) no create page for a character in a clan
        if command == CMD_CREATE && in_clan(ctx.data) {
            return;
        }
        let title = WEB_TITLES
            .iter()
            .find(|(c, _)| *c == command)
            .map_or(0, |(_, t)| *t);
        let t = ctx.cat.text(title);
        self.sheet.set_text(WEB_WINDOW, WEB_TITLE, &t);
        self.sheet.show_window(WEB_WINDOW, true);
        self.sheet.modal = Some(WEB_WINDOW);
        if self.web_field_focused() {
            self.sheet.focus(None);
        }
        self.web = match command {
            CMD_CREATE => {
                if let Some(i) = self.create_field {
                    self.sheet.parts[i].text.clear();
                }
                self.sheet.focus(self.create_field);
                WebPage::Create(CreatePage::new(self.create_field))
            }
            CMD_FIND | CMD_MANAGE => {
                let kind = if command == CMD_FIND {
                    ListKind::Search
                } else {
                    ListKind::Manage
                };
                let mut l = ListPage::new(kind);
                l.member = in_clan(ctx.data);
                // CLI-1896: the search field starts empty and takes the keyboard; CLI-1901: on the manage page the
                // same field is the leader's invitation entry (empty, the keyboard on a click)
                if let Some(i) = self.search_field {
                    self.sheet.parts[i].text.clear();
                }
                l.field = self.search_field;
                if kind == ListKind::Search {
                    self.sheet.focus(self.search_field);
                }
                l.busy = true;
                if !web_get(ctx, l.ask(&ctx.data.user)) {
                    l.busy = false;
                    l.message = Some(clanpage::T_UNREACHABLE.into());
                }
                WebPage::List(l)
            }
            _ => {
                let name = ctx
                    .data
                    .clan
                    .info
                    .as_ref()
                    .map(|v| v.name.clone())
                    .unwrap_or_else(|| ctx.data.char_extras.clan_label.clone());
                WebPage::Leave(LeavePage::new(&name))
            }
        };
    }

    /// The answer of a list / leave page request: the list's rows and line; a done join or leave closes the window
    /// with the server's line in a box (the server's pushes refill the screen).
    fn web_answer(&mut self, target: &str, body: Option<&str>, ctx: &mut Ctx) {
        let done = body
            .and_then(|b| clanpage::parse_reply(b).msg)
            .filter(|(ok, _)| *ok);
        let closes = target.starts_with("/clan/join") || target.starts_with("/clan/leave");
        match &mut self.web {
            // CLI-1896: the answer of a list request that a newer one replaced is dropped (the requests run on their
            // own threads: an older search's answer may come last)
            WebPage::List(l) if l.stale(target) => {}
            WebPage::List(l) if !target.starts_with("/clan/leave") => match body {
                Some(b) => {
                    if let (true, Some((_, m))) = (closes, &done) {
                        let m = m.clone();
                        self.close_web();
                        ctx.popups.push(Popup::ok(m, BoxTag::None));
                    } else {
                        l.answer(b);
                        // CLI-1901: a sent invitation empties the entry
                        if target.starts_with("/clan/invite") && done.is_some() {
                            if let Some(i) = self.search_field {
                                self.sheet.parts[i].text.clear();
                            }
                        }
                    }
                }
                None => {
                    l.busy = false;
                    l.message = Some(clanpage::T_UNREACHABLE.into());
                }
            },
            WebPage::Leave(l) if target.starts_with("/clan/leave") => {
                l.busy = false;
                match (body, done) {
                    (Some(_), Some((_, m))) => {
                        self.close_web();
                        ctx.popups.push(Popup::ok(m, BoxTag::None));
                    }
                    (Some(b), None) => {
                        l.message = clanpage::parse_reply(b)
                            .msg
                            .map(|(_, m)| m)
                            .or(Some(clanpage::T_UNREACHABLE.into()))
                    }
                    (None, _) => l.message = Some(clanpage::T_UNREACHABLE.into()),
                }
            }
            _ => {}
        }
    }
    /// The web window closes with its page; the name field gives up the focus.
    fn close_web(&mut self) {
        self.sheet.show_window(WEB_WINDOW, false);
        self.sheet.modal = None;
        self.web = WebPage::None;
        if self.web_field_focused() {
            self.sheet.focus(None);
        }
    }

    /// Whether the keyboard is in one of the web window's own fields (the create page's name, the find page's
    /// search).
    fn web_field_focused(&self) -> bool {
        let f = self.sheet.focused();
        f.is_some() && (f == self.create_field || f == self.search_field)
    }

    /// CLI-1896: the find page's search: the field's text becomes the page's search and the list is asked again
    /// with it (the lobby filters by `q`; the page keeps the same match for an older lobby's whole list).
    fn search(&mut self, ctx: &mut Ctx) {
        let WebPage::List(l) = &mut self.web else {
            return;
        };
        // CLI-1901: the leader's manage page invites the typed name
        if l.offers_invite() {
            if l.busy {
                return;
            }
            if let Some(t) = l.invite_request(&self.sheet, &ctx.data.user) {
                l.busy = true;
                l.message = None;
                if !web_get(ctx, t) {
                    l.busy = false;
                    l.message = Some(clanpage::T_UNREACHABLE.into());
                }
            }
            return;
        }
        // a search while the list is still coming asks again: the later answer is the one shown
        if l.kind != ListKind::Search {
            return;
        }
        l.take_query(&self.sheet);
        l.busy = true;
        l.message = None;
        if !web_get(ctx, l.ask(&ctx.data.user)) {
            l.busy = false;
            l.message = Some(clanpage::T_UNREACHABLE.into());
        }
    }

    /// The create page's request (the button or Enter with an acceptable name).
    fn send_create(&mut self, ctx: &mut Ctx) {
        let WebPage::Create(c) = &mut self.web else {
            return;
        };
        if c.busy || !c.name_ok(&self.sheet) {
            return;
        }
        match clanpage::web_addr(ctx.data.auth) {
            Some(addr) => {
                let target = c.request(&self.sheet, &ctx.data.user);
                c.busy = true;
                c.message = None;
                ctx.send(LinkCmd::ClanWeb { addr, target });
            }
            None => c.message = Some(clanpage::T_UNREACHABLE.into()),
        }
    }

    /// The create answer: done -> the window closes and a box shows the server's line (the server pushes 0x303,
    /// 0x3061 and 0x3062, which refill the screen); refused or unreachable -> the line under the grids.
    fn created(&mut self, body: Option<&str>, ctx: &mut Ctx) {
        let WebPage::Create(c) = &mut self.web else {
            return;
        };
        c.busy = false;
        let Some(body) = body else {
            c.message = Some(clanpage::T_UNREACHABLE.into());
            return;
        };
        match clanpage::parse_reply(body).msg {
            Some((true, m)) => {
                self.close_web();
                ctx.popups.push(Popup::ok(m, BoxTag::None));
            }
            Some((false, m)) => c.message = Some(m),
            None => c.message = Some(clanpage::T_UNREACHABLE.into()),
        }
    }

    /// The buttons again after the character's clan changed (0x303).
    fn reset_buttons_only(&mut self, data: &MenuData) {
        let member = in_clan(data);
        self.set_manage(member);
        if let WebPage::List(l) = &mut self.web {
            l.member = member;
        }
    }

    /// The member rows: badge, role, name and online state.
    // EVIDENCE: cshell+0x1045f0, cshell+0x1030b0, cshell+0x206840 (one row per member in the list's order)
    fn draw_members(&self, cat: &mut Catalog, data: &MenuData, out: &mut Vec<DrawItem>) {
        let Some(list) = self
            .sheet
            .part(MEMBER_WINDOW, MEMBER_LIST, WidgetKind::List)
            .filter(|_| self.window_shown(MEMBER_WINDOW))
        else {
            return;
        };
        let left_mid = Align {
            first: super::sheet::ALIGN_LEFT,
            second: ALIGN_SECOND_CENTRE,
        };
        for (i, m) in data.clan.members.iter().enumerate() {
            // rows scrolled above the list's top or below its bottom are not drawn
            let Some(at) = list_row(list, MEMBER_ROW[1], i, 0) else {
                continue;
            };
            let add = |o: [i32; 2]| [at[0] + o[0], at[1] + o[1]];
            let role = match m.role {
                CLAN_ROLE_LEADER => ROLE_LEADER_TEXT,
                CLAN_ROLE_OFFICER => ROLE_OFFICER_TEXT,
                _ => ROLE_MEMBER_TEXT,
            };
            out.push(place_text(
                &cat.text(role),
                add(MEMBER_ROLE_AT),
                [0, 0],
                left_mid,
                14,
                MEMBER_TEXT_SIZE,
                ROLE_COLOUR,
            ));
            let badge = self
                .art
                .as_ref()
                .and_then(|a| a.levels.icon_of_exp(m.exp, &m.name))
                .or(self.style.user_badge.as_deref());
            if let Some(t) = badge {
                out.push(image(t, add(MEMBER_BADGE_AT), self.style.user_badge_size));
            }
            out.push(place_text(
                &m.name,
                add(MEMBER_NAME_AT),
                [0, 0],
                left_mid,
                14,
                MEMBER_TEXT_SIZE,
                NAME_COLOUR,
            ));
            let (t, c) = if m.online {
                (ONLINE_TEXT, ONLINE_COLOUR)
            } else {
                (OFFLINE_TEXT, OFFLINE_COLOUR)
            };
            out.push(place_text(
                &cat.text(t),
                add(MEMBER_ONLINE_AT),
                [0, 0],
                left_mid,
                14,
                MEMBER_TEXT_SIZE,
                c,
            ));
        }
    }

    /// The last clan chat lines in the chat window's list, newest at the bottom.
    // DC-DEFAULT: CLI-562 (proposed) lines `name : text` in font 14 size 13, 18 px apart (the lobby chat's look)
    fn draw_chat(&self, data: &MenuData, out: &mut Vec<DrawItem>) {
        let Some(list) = self
            .sheet
            .part(TALK_WINDOW, CHAT_LIST, WidgetKind::List)
            .filter(|_| self.window_shown(TALK_WINDOW))
        else {
            return;
        };
        let lines = &data.clan.chat;
        let scrolled = matches!(list.look, Look::List { bar: Some(_), .. });
        let n = list_capacity(list, 16).min(lines.len());
        let start = lines.len() - n;
        let rows = match list.look {
            Look::List { height, .. } if height > 0 => (height / 18).max(1) as usize,
            _ => n,
        };
        let start = if scrolled {
            0
        } else {
            start.max(lines.len().saturating_sub(rows))
        };
        for (k, (name, text)) in lines.iter().enumerate().skip(start) {
            // lobby-fixes10: a scrolled list shows the rows its bar reaches (it follows the newest line)
            let at = if scrolled {
                match list_row(list, super::lobby::CHAT_ROW, k, 0) {
                    Some(at) => at,
                    None => continue,
                }
            } else {
                [list.pos[0], list.pos[1] + (k - start) as i32 * 18]
            };
            out.push(place_text(
                &format!("{name} : {text}"),
                [at[0] + 4, at[1]],
                [0, 0],
                Align::default(),
                14,
                13,
                0xffee_eeee,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_format_fills_in_turn() {
        assert_eq!(
            c_format(
                "%dW-%dD-%dL (%d%%)",
                &["3".into(), "1".into(), "2".into(), "50".into()]
            ),
            "3W-1D-2L (50%)"
        );
        assert_eq!(c_format("%s (Lv%d)", &["A".into(), "2".into()]), "A (Lv2)");
        assert_eq!(c_format("%d / %d", &["1".into()]), "1 / ");
    }

    #[test]
    fn rank_ordinals_skip_the_teens() {
        let l = ClanLits {
            rank: [
                Some("%dst".into()),
                Some("%dnd".into()),
                Some("%drd".into()),
                Some("%dth".into()),
            ],
            ..ClanLits::default()
        };
        let got: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101, 111]
            .iter()
            .map(|n| l.rank_text(*n))
            .collect();
        assert_eq!(
            got,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "23rd",
                "101st", "111th"
            ]
        );
    }

    #[test]
    fn experience_shows_the_next_level_start() {
        let l = ClanLits::default();
        assert_eq!(l.exp_text(1500, 2), "1,500 / 2,364");
        // past the last level the next start is 0
        assert_eq!(l.exp_text(200000, 6), "200,000 / 0");
        assert_eq!(level_first_exp(1), 0);
        assert_eq!(level_first_exp(7), 0);
    }

    #[test]
    fn mark_look_up_needs_both_layers_and_skips_index_0() {
        let m = ClanMarks {
            emblems: vec![None, Some("e1".into()), None],
            backgrounds: vec![None, Some("b1".into())],
            ..Default::default()
        };
        assert_eq!(m.textures(1, 1), Some(("e1", "b1")));
        assert_eq!(m.textures(0, 1), None);
        assert_eq!(m.textures(2, 1), None);
        assert_eq!(m.textures(1, 5), None);
    }

    #[test]
    fn clan_lines_of_the_relay_go_to_the_clan_chat() {
        let mut d = MenuData::default();
        shared_event(
            &mut d,
            &LinkEvent::Messenger(vec!["clan".into(), "Ann".into(), "hi".into()]),
        );
        shared_event(
            &mut d,
            &LinkEvent::Messenger(vec!["whisper".into(), "Bo".into(), "x".into()]),
        );
        assert_eq!(d.clan.chat, vec![("Ann".to_string(), "hi".to_string())]);
        assert_eq!(clan_chat_command("yo"), "!nm clan\tyo");
    }

    /// CLI-1895 (owner report "clan announcements don't allow for spaces"): the window delivers Space as a key; the
    /// clan screen types it into its focused field: the notice box, the chat line, the create page's name and the
    /// find page's search (CLI-1896), whose Enter asks the list with the search. ROM-gated (the screen's sheet).
    #[test]
    fn rom_space_types_a_space_in_every_clan_field() {
        let Ok(rom) = dc_rom::resolve_rom() else {
            return;
        };
        let cat = Catalog::from_rom(&rom, 1024, 768).unwrap();
        let mut data = MenuData::default();
        data.char_extras.clan_id = 7;
        data.char_extras.clan_role = CLAN_ROLE_LEADER;
        data.auth = Some("127.0.0.1:9".parse().unwrap());
        data.user = "Alice".into();
        let mut s = ClanScreen::new(&cat, &data);
        let link = super::super::link::Link::detached();
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
        let typed = |s: &mut ClanScreen, ctx: &mut Ctx, t: &str| {
            for c in t.chars() {
                if c == ' ' {
                    s.key(MenuKey::Space, ctx);
                } else {
                    s.sheet.type_char(c);
                }
            }
        };
        // the notice box
        s.click(CMD_NOTICE, usize::MAX, &mut ctx);
        typed(&mut s, &mut ctx, "war at nine");
        assert_eq!(s.notice_input(), Some("war at nine"));
        s.click(CMD_NOTICE_OK, usize::MAX, &mut ctx);
        let sent = link.take_sent();
        assert!(
            sent.iter()
                .any(|c| matches!(c, LinkCmd::ClanNoticeSet(t) if t == "war at nine")),
            "{sent:?}"
        );
        // the chat line
        let chat = s.sheet.parts.iter().position(|p| {
            p.layer == TALK_WINDOW && p.id == TALK_FIELD && p.kind == WidgetKind::EditBox
        });
        s.sheet.focus(chat);
        typed(&mut s, &mut ctx, "hi all");
        assert_eq!(s.typed_talk(), "hi all");
        // the find page: the search field has the keyboard, Enter searches with the typed name
        s.click(CMD_FIND, usize::MAX, &mut ctx);
        let _ = link.take_sent();
        typed(&mut s, &mut ctx, "red fox");
        s.key(MenuKey::Enter, &mut ctx);
        let sent = link.take_sent();
        assert!(
            sent.iter()
                .any(|c| matches!(c, LinkCmd::ClanWeb { target, .. }
                if target.contains("url=guild/list") && target.ends_with("&q=red%20fox"))),
            "{sent:?}"
        );
        let WebPage::List(l) = &s.web else {
            panic!("the find page");
        };
        assert_eq!(l.query, "red fox");
    }
}
