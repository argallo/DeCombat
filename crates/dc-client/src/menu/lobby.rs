//! Lobby screen (room list): the channel's rooms (number, lock, title, map, mode, players, state), the channel's
//! users, the own character panel and the chat panel. Enter / double click joins the selected room (a locked room
//! asks for its password first), quick start joins the first joinable room, create opens the room creation
//! screen, change channel / exit leave the channel (0x374) for the channel screen.
// SPEC: PROTOCOL_2009.md §3.6 (0x380 / 0x381, 0x370 / 0x371, pushes 0x3021 / 0x3022 / 0x3031 / 0x3032 / 0x3035),
// §3.7 (0x384 / 0x385: reason 0x2a -> res 0x1013, 0x8a -> res 0x14be, other -> res 0xfb1)
// EVIDENCE: cshell+0x129e00 (commands: 204 create -> screen 8, 206 quick start -> cshell+0x129ac0, 207 enter: no
// room -> box 0xfb1, locked -> password box, else 0x384 with an empty password; 67 exit), cshell+0x11ed10 (row
// element positions), cshell+0x128940 (row bar and lock textures, state strings 0x161c / 0x445 / 0x161b),
// cshell+0x120d90 (user row bar and rank textures, own row 0xfffead00), cshell+0x11fa00 (user row positions)
// EVIDENCE: cshell+0x1211b0 (build: "map_list_page", list 13 of window 5), cshell+0x1219b0 (list 13 of window 5,
// list 1 of window 6), cshell+0x11ea70 (a room row draws its bar only with its current-item flag, slot 0x8c, that
// the list sets through cshell+0x2f1400), cshell+0x128a96 (index `%03d`), cshell+0x128c88 (players `%d / %d`)
// DC-DEFAULT: CLI-1692 (proposed) the clicked room keeps its bar while the pointer is on another row
// EVIDENCE: cshell+0x1295a0 (screen focus: room list request, location 0, windows 8 and 9 hidden)

use super::catalog::{int, ints, key, pair, rgb_word, Catalog, KeyPin};
use super::drawlist::{anchor_of, DynNotes, DynWant};
use super::link::{CharExtras, LinkCmd, LinkEvent, ProfileView, RoomRow, Wait};
use super::popup::{text_ids, BoxTag, Popup};
use super::result::grouped;
use super::sheet::Look;
use super::sheet::{
    image, list_capacity, list_hit, list_row, place_text, Part, Sheet, ALIGN_CENTRE, ALIGN_LEFT,
};
use super::textbox::{
    self, ChatScreen, CHAT_COLOUR, CHAT_PAGE_FONT, CHAT_PAGE_SIZE, CMD_CHAT_SEND,
};
use super::{build_sheet, pages, Ctx, Go, MenuData, MenuKey, Screen, DOUBLE_CLICK_MS};
use crate::hud::board::LevelTable;
use crate::hud::items::DrawItem;
use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::{UiScreen, WidgetKind};
use std::path::Path;
use std::time::Instant;

/// Keys of the room / user rows in the room list screen's layout section.
// EVIDENCE: cshell+0x128940 (bar width, height, texture, lock size and texture), cshell+0x11ed10 (positions),
// cshell+0x120d90 (user bar texture / size, rank texture / size), cshell+0x11fa00 (user name, bar and rank places)
pub mod row_keys {
    use super::{key, KeyPin};
    pub const BAR: KeyPin = key("85820e6558c79123", 6);
    pub const WIDTH: KeyPin = key("4a0a89e74659ea35", 8);
    pub const HEIGHT: KeyPin = key("7b15cfe7354d7747", 9);
    pub const LOCK: KeyPin = key("328a126db8df742b", 7);
    pub const LOCK_SIZE: KeyPin = key("5cbccd7dc54423b2", 8);
    pub const LOCK_AT: KeyPin = key("5d6ab19b72b09539", 7);
    pub const INDEX_AT: KeyPin = key("65fb375b13800675", 8);
    pub const NAME_AT: KeyPin = key("d5698f80e06d3478", 7);
    pub const MAP_AT: KeyPin = key("6a415ec0f936cf60", 10);
    pub const MODE_AT: KeyPin = key("fd1ba057d8c11ba4", 11);
    pub const MEMBERS_AT: KeyPin = key("a0cf5c39f7e2cd24", 9);
    pub const STATE_AT: KeyPin = key("d81c20d304555303", 8);
    pub const USER_BAR: KeyPin = key("f061f09a01030234", 10);
    pub const USER_WIDTH: KeyPin = key("25e3f9b36561027e", 12);
    pub const USER_HEIGHT: KeyPin = key("372dfe6c0a525444", 13);
    pub const USER_BADGE: KeyPin = key("3f8971586189ae81", 11);
    pub const USER_BADGE_SIZE: KeyPin = key("f0c2612262eb9d40", 12);
    pub const USER_BAR_AT: KeyPin = key("7b2c49f7767948ac", 10);
    pub const USER_NAME_AT: KeyPin = key("b889d37b25e349dc", 11);
    pub const USER_BADGE_AT: KeyPin = key("61220609b19059a5", 11);
    pub const USER_MARK_AT: KeyPin = key("6213fef0efd87357", 15);
    pub const USER_FLAG_AT: KeyPin = key("250f7f64b1959ce2", 15);
    /// The country window's `FlagSize`: the size of a flag control, also the user row's (cshell+0x120d90).
    pub const FLAG_SECTION: KeyPin = key("b2cd7d4e0574ab2a", 16);
    pub const FLAG_SIZE: KeyPin = key("662f0e51326b863c", 8);
}

/// Room state strings: in game (2, 3), starting (1), waiting (other).
// EVIDENCE: cshell+0x128940
pub const STATE_IN_GAME: i32 = 0x161c;
pub const STATE_STARTING: i32 = 0x445;
pub const STATE_WAITING: i32 = 0x161b;
/// Row text colours: selected, other; the own user row.
// EVIDENCE: cshell+0x120d90 (own row 0xfffead00)
pub const OWN_COLOUR: u32 = 0xfffe_ad00;
// DC-DEFAULT: CLI-171 (proposed) room / user row text 0xffdddddd, the selected room row 0xfffead00
pub const ROW_COLOUR: u32 = 0xffdd_dddd;

pub fn state_text(state: i32) -> i32 {
    match state {
        2 | 3 => STATE_IN_GAME,
        1 => STATE_STARTING,
        _ => STATE_WAITING,
    }
}

/// Window and control ids.
pub const ROOM_WINDOW: i32 = 5;
pub const ROOM_LIST: i32 = 13;
pub const CHANNEL_TEXT: i32 = 5;
pub const USER_WINDOW: i32 = 6;
pub const USER_LIST: i32 = 1;
/// The user list window's text 0: the 2009 room list never writes it (it keeps the layout's own text) and never
/// hides it: a shown text control without a string, so nothing is drawn there.
// EVIDENCE: cshell+0x121880, cshell+0x1219b0, cshell+0x1220e0, cshell+0x122230, cshell+0x1222b0, cshell+0x1224a0
// (every window-6 lookup of the room list screen names control 1, the user list); cshell+0x1211b0 (the screen's
// build: no window-6 lookup at all)
// SPEC: FMT_CRES_UI.md §6.3 (a text record has no visibility field; only the edit box record has one)
// OBSERVED: lobbyfx4/real/R10_lobby.png (2009: nothing drawn at the record's place 805, 95)
pub const USER_COUNT: i32 = 0;
pub const CHAR_WINDOW: i32 = 7;
pub const TOOLTIP_WINDOW: i32 = 8;
pub const DISPLAY_WINDOW: i32 = 9;
pub const CHAT_PANEL: i32 = 2;
pub const CHAT_LIST: i32 = 4;
/// The chat page's Enter button and its three chat lists (window [`CHAT_PANEL`]).
pub const CHAT_ENTER: i32 = 0;
pub const CHAT_LISTS: [i32; 3] = [4, 5, 6];
/// Where the room list screen puts the chat's Enter button, its caption (inside the button) and the chat lists'
/// scroll bars (x): see [`lobby_chat`].
// EVIDENCE: cshell+0x1211b0 (0x1c5, 0x2a9; 0x1e, 0xc; 0x208)
pub const LOBBY_ENTER_AT: [i32; 2] = [0x1c5, 0x2a9];
pub const LOBBY_ENTER_CAPTION_AT: [i32; 2] = [0x1e, 0xc];
pub const LOBBY_CHAT_SCROLL_X: i32 = 0x208;
/// The colour the character info's prepaid, credit and GP numbers are given (all four state colours).
// EVIDENCE: cshell+0x126fa0 (vtable +0xb0 with 0xffffffff x 4 after texts 9, 11, 13)
pub const CHAR_NUMBER_COLOUR: u32 = 0xffff_ffff;
/// Command numbers.
pub const CMD_EXIT: i32 = 67;
pub const CMD_CREATE: i32 = 204;
pub const CMD_QUICK: i32 = 206;
pub const CMD_ENTER: i32 = 207;
pub const CMD_CHANGE_CHANNEL: i32 = 20008;
pub const CMD_TOP_EXIT: i32 = 303;

/// The user menu: its two entries' strings (whisper, profile) and the command its entries send.
// EVIDENCE: cshell+0x1211b0 (the user menu: font 0xe, entries string 0x13f7 and 0x573, command 0xf0 with the
// entry's index), cshell+0x129e00 (command 0xf0: entry 0 -> the selected user's name to the chat's whisper target;
// entry 1 -> command 0x3a, the profile request 0x378 with the selected user's in-channel id)
pub const USER_MENU_TEXTS: [i32; 2] = [0x13f7, 0x573];
pub const USER_MENU_WHISPER: usize = 0;
pub const USER_MENU_PROFILE: usize = 1;
/// The user menu's look: entry size and padding.
// DC-DEFAULT: CLI-472 (staged) the user menu is a dark box of 120 x 22 px entries with 4 px padding, the entry under
// the pointer lit; the 2009 nine-slice skin (cshell+0x1211b0, box images 32 / 8) is not drawn
pub const USER_MENU_ENTRY: [i32; 2] = [120, 22];
pub const USER_MENU_PAD: i32 = 4;

/// The open user menu: its top-left, the user's row and in-channel id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserMenu {
    pub at: [i32; 2],
    pub row: usize,
    pub id: u32,
}

impl UserMenu {
    /// The rectangle of entry `i` (x, y, w, h).
    pub fn entry_rect(&self, i: usize) -> [i32; 4] {
        [
            self.at[0] + USER_MENU_PAD,
            self.at[1] + USER_MENU_PAD + i as i32 * USER_MENU_ENTRY[1],
            USER_MENU_ENTRY[0],
            USER_MENU_ENTRY[1],
        ]
    }

    /// The whole box.
    pub fn rect(&self) -> [i32; 4] {
        let n = USER_MENU_TEXTS.len() as i32;
        [
            self.at[0],
            self.at[1],
            USER_MENU_ENTRY[0] + 2 * USER_MENU_PAD,
            n * USER_MENU_ENTRY[1] + 2 * USER_MENU_PAD,
        ]
    }

    pub fn entry_at(&self, p: [f32; 2]) -> Option<usize> {
        (0..USER_MENU_TEXTS.len()).find(|&i| inside(self.entry_rect(i), p))
    }
}

fn inside(r: [i32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] as f32
        && p[1] >= r[1] as f32
        && p[0] < (r[0] + r[2]) as f32
        && p[1] < (r[1] + r[3]) as f32
}

/// The grade table's file in the attribute archive (one section per grade).
// OBSERVED: ROM archive ATTRIBUTES.rez (entry of the used-NX grade table; the path literal is in the same
// function as EVIDENCE: cshell+0x285980)
const GRADE_ENTRY: &str = "ATTRIBUTES\\USEDNXLEVEL.TXT";

/// Pinned literals of the grade table reader and of the level table's short-name column.
// EVIDENCE: cshell+0x2856d0 (grade sections `<prefix><n>` with the icon, name and name colour keys),
// cshell+0x268810 (the level section's last column, the short level name)
const PANEL_PINS: [Pin; 6] = [
    Pin {
        what: "level section",
        rva: 0x3f4a68,
        sha16: "9782640c0625e02f",
        len: 8,
    },
    Pin {
        what: "short level name key",
        rva: 0x3f4a20,
        sha16: "93b2cc756dc7faaf",
        len: 15,
    },
    Pin {
        what: "grade section prefix",
        rva: 0x3f68d0,
        sha16: "cfd1d2ba65d5011f",
        len: 11,
    },
    Pin {
        what: "grade icon key",
        rva: 0x3f68c0,
        sha16: "a5b610db96764323",
        len: 12,
    },
    Pin {
        what: "grade name key",
        rva: 0x3f4a50,
        sha16: "571e47573f9bb9a0",
        len: 9,
    },
    Pin {
        what: "grade name colour key",
        rva: 0x3f6888,
        sha16: "9231dd0636e20ca4",
        len: 14,
    },
];

/// One row of the grade table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GradeRow {
    pub name: String,
    pub icon: String,
    /// Opaque `0xFFRRGGBB`.
    pub colour: u32,
}

/// What the Character Info panel and the rank badges take from the user's ROM: the level table (experience bounds
/// and small icons), the short level names and the grade table. Nothing of it is stored in the tree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PanelArt {
    pub levels: LevelTable,
    pub short_names: Vec<String>,
    pub grades: Vec<GradeRow>,
}

impl PanelArt {
    /// [`PanelArt::from_rom`], read once per ROM folder and process (the room list and the room creation box both
    /// open it; the read costs a few milliseconds).
    pub fn cached(rom: &Path) -> Option<PanelArt> {
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::sync::{Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<PanelArt>>>> = OnceLock::new();
        let mut c = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        c.entry(rom.to_path_buf())
            .or_insert_with(|| PanelArt::from_rom(rom))
            .clone()
    }

    pub fn from_rom(rom: &Path) -> Option<PanelArt> {
        let levels = LevelTable::from_rom(rom)?;
        let pe = dc_rom::PeFile::from_file(&rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]))
            .ok()?;
        let lit: Vec<Option<String>> = PANEL_PINS
            .iter()
            .map(|p| read_pinned(&pe, p).ok())
            .collect();
        let vfs = dc_rez::RezVfs::new(rom);
        let mut art = PanelArt {
            levels,
            ..PanelArt::default()
        };
        if let (Some(sec), Some(key)) = (&lit[0], &lit[1]) {
            if let Some(doc) =
                dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", crate::hud::rank::LEVEL_ENTRY)
            {
                if let Some(m) = doc.get(sec) {
                    art.short_names = (0..1024)
                        .map_while(|i| m.get(&format!("{key}{i}")))
                        .map(|v| v.trim().trim_matches('"').trim().to_string())
                        .collect();
                }
            }
        }
        if let (Some(prefix), Some(icon), Some(name), Some(colour)) =
            (&lit[2], &lit[3], &lit[4], &lit[5])
        {
            if let Some(doc) = dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", GRADE_ENTRY) {
                // numbered sections from 0 up to the first missing one
                art.grades = (0..1024)
                    .map_while(|i| doc.get(&format!("{prefix}{i}")))
                    .map(|m| {
                        let text = |k: &str| {
                            m.get(k)
                                .map(|v| v.trim().trim_matches('"').trim().to_string())
                                .unwrap_or_default()
                        };
                        let c = ints(&text(colour));
                        GradeRow {
                            name: text(name),
                            icon: text(icon),
                            colour: if c.len() >= 3 {
                                rgb_word([f64::from(c[0]), f64::from(c[1]), f64::from(c[2])])
                            } else {
                                0xffff_ffff
                            },
                        }
                    })
                    .collect();
            }
        }
        Some(art)
    }

    /// The short name of the level of `exp`; an index past the list reads row 0.
    // EVIDENCE: cshell+0x268140 (`if (count <= level) level = 0`, the list at the level section's +0x2c)
    pub fn short_name(&self, exp: u32) -> &str {
        let level = self.levels.level_of(exp) as usize;
        let i = if level < self.short_names.len() {
            level
        } else {
            0
        };
        self.short_names.get(i).map_or("", String::as_str)
    }

    /// `part/span (pct %)` of the level of `exp`.
    // EVIDENCE: cshell+0x126fa0 (`%d/%d (%3.2f %%)`: part = cshell+0x268040, span = cshell+0x267ff0, the per cent is
    // part x 100 / span, 0 when the span is 0)
    pub fn progress(&self, exp: u32) -> String {
        let (part, span) = level_span(&self.levels, exp);
        let pct = if span == 0 {
            0.0
        } else {
            f64::from(part) * 100.0 / f64::from(span)
        };
        format!("{part}/{span} ({pct:3.2} %)")
    }

    /// The experience above the level below and the span of the level of `exp` (the two numbers of
    /// [`PanelArt::progress`]).
    pub fn part_span(&self, exp: u32) -> (u32, u32) {
        level_span(&self.levels, exp)
    }

    /// The grade row of `grade` (none past the table).
    // EVIDENCE: cshell+0x285330 / 0x285370 / 0x2853b0 (colour, name and icon of the row; an index past the table
    // gives white, an empty name and an empty icon)
    pub fn grade(&self, grade: u32) -> Option<&GradeRow> {
        self.grades.get(grade as usize)
    }
}

/// The part of the experience above the level below and the span of the level.
// EVIDENCE: cshell+0x268040 (the experience minus the bound of the level below; the whole experience at level 0),
// cshell+0x267ff0 (the first bound at level 0, else the bound difference; 0 at the level count)
fn level_span(levels: &LevelTable, exp: u32) -> (u32, u32) {
    let b = &levels.bounds;
    let level = levels.level_of(exp) as usize;
    if level >= b.len() {
        (0, 0)
    } else if level == 0 {
        (exp, b[0])
    } else {
        (
            exp.wrapping_sub(b[level - 1]),
            b[level].wrapping_sub(b[level - 1]),
        )
    }
}

/// The experience of the room member or channel user `id`: a room member's from its slot record (+0x42, the own
/// slot too, as 2009), else the own one from the profile, else the channel user record's (+0x2a). The rank word of
/// the records (+8) is never read for a rank.
// EVIDENCE: cshell+0x1657e0 / cshell+0x2683d0 (room member rank icon = icon of the level of slot +0x42),
// cshell+0x120d90 (user row: +0x2a); SPEC: PROTOCOL_2009.md §3.7 table 3.7b, §3.6 table 3.6b
pub fn exp_of(data: &MenuData, id: u32) -> Option<u32> {
    if let Some(m) = data.members.get(&id) {
        return Some(m.exp);
    }
    if id != 0 && id == data.profile.my_id {
        return Some(data.profile.exp);
    }
    data.users.iter().find(|u| u.id == id).map(|u| u.exp)
}

/// Room and user row looks.
#[derive(Debug, Clone, PartialEq)]
pub struct RowStyle {
    pub bar: Option<String>,
    pub size: [i32; 2],
    pub lock: Option<String>,
    pub lock_size: [i32; 2],
    pub lock_at: [i32; 2],
    pub index_at: [i32; 2],
    pub name_at: [i32; 2],
    pub map_at: [i32; 2],
    pub mode_at: [i32; 2],
    pub members_at: [i32; 2],
    pub state_at: [i32; 2],
    pub user_bar: Option<String>,
    pub user_size: [i32; 2],
    pub user_badge: Option<String>,
    pub user_badge_size: [i32; 2],
    pub user_bar_at: [i32; 2],
    pub user_name_at: [i32; 2],
    pub user_badge_at: [i32; 2],
    pub user_mark_at: [i32; 2],
    pub user_flag_at: [i32; 2],
    /// `FlagSize` of the country window: the size every flag control is built with.
    pub flag_size: [i32; 2],
}

impl RowStyle {
    // DC-DEFAULT: CLI-171 (proposed) row fallbacks when a key is missing: 690 x 31 room rows, 212 x 31 user rows
    pub fn load(cat: &Catalog) -> RowStyle {
        use row_keys as k;
        let s = |key: &KeyPin| cat.style(UiScreen::Screen15, key);
        let p = |key: &KeyPin| pair(s(key)).unwrap_or([0, 0]);
        RowStyle {
            bar: s(&k::BAR).map(str::to_string),
            size: [
                int(s(&k::WIDTH)).unwrap_or(690),
                int(s(&k::HEIGHT)).unwrap_or(31),
            ],
            lock: s(&k::LOCK).map(str::to_string),
            lock_size: p(&k::LOCK_SIZE),
            lock_at: p(&k::LOCK_AT),
            index_at: p(&k::INDEX_AT),
            name_at: p(&k::NAME_AT),
            map_at: p(&k::MAP_AT),
            mode_at: p(&k::MODE_AT),
            members_at: p(&k::MEMBERS_AT),
            state_at: p(&k::STATE_AT),
            user_bar: s(&k::USER_BAR).map(str::to_string),
            user_size: [
                int(s(&k::USER_WIDTH)).unwrap_or(212),
                int(s(&k::USER_HEIGHT)).unwrap_or(31),
            ],
            user_badge: s(&k::USER_BADGE).map(str::to_string),
            user_badge_size: p(&k::USER_BADGE_SIZE),
            user_bar_at: p(&k::USER_BAR_AT),
            user_name_at: p(&k::USER_NAME_AT),
            user_badge_at: p(&k::USER_BADGE_AT),
            user_mark_at: p(&k::USER_MARK_AT),
            user_flag_at: p(&k::USER_FLAG_AT),
            flag_size: pair(cat.section_value(&k::FLAG_SECTION, &k::FLAG_SIZE)).unwrap_or([0, 0]),
        }
    }
}

fn add(a: [i32; 2], b: [i32; 2]) -> [i32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

const CENTRE: Align = Align {
    first: ALIGN_CENTRE,
    second: ALIGN_SECOND_CENTRE,
};
const LEFT_MID: Align = Align {
    first: ALIGN_LEFT,
    second: ALIGN_SECOND_CENTRE,
};

/// The room row's two font records: the number, players, mode and state texts use the first, the title and map
/// texts the second.
// EVIDENCE: cshell+0x128940 (font lookups cshell+0x1c07b0 with 9 and 3; index, members, mode and state controls
// get the first, name and map the second)
pub const ROOM_ROW_FONTS: [i32; 2] = [9, 3];
/// Size of every room row text.
// EVIDENCE: cshell+0x128940 (all six string controls created with size 0xe)
pub const ROOM_ROW_SIZE: i32 = 14;
/// The room number as the room list shows it: three digits, one more than the room's number, no dot.
// EVIDENCE: cshell+0x128940 / cshell+0x127fb0 (wide form at cshell+0x3d5998 with the room record's first field + 1;
// the forms read from the binary: that one `%03d`, the waiting room's cshell+0x3e60a0 `%03d.`)
// DISASM: cshell+0x3d5998 (the wide format literal)
// OBSERVED: lobbyfx4/real/R11_lobbylist.png (2009: the first room shows `001`, no dot)
pub fn room_list_number(number: u32) -> String {
    format!("{:03}", u64::from(number) + 1)
}

/// The players text of a room row (`%d / %d`).
// EVIDENCE: cshell+0x128940 (wide form at cshell+0x3def6c with the players and the maximum)
// OBSERVED: lobbyfx4/real/R11_lobbylist.png (`1 / 16`)
pub fn room_list_players(players: i32, max: i32) -> String {
    format!("{players} / {max}")
}

/// What the 2009 room row asks of its six texts at row position `at`, in draw order (number, title, map, mode,
/// players, state): each at the row plus its style position; the title left / vertically centred, the others
/// centred both ways; fonts [`ROOM_ROW_FONTS`], size [`ROOM_ROW_SIZE`]. The draw trace's own copy of the rule,
/// apart from [`draw_rooms`].
// EVIDENCE: cshell+0x11ed10 (children moved to the row position plus IndexPos / NamePos / MapNamePos / ModeNamePos
// / MemberPos / StatePos), cshell+0x128940 (create calls: index font 9 flags 0x44, name font 3 flags 0x41, map font
// 3 flags 0x44, members font 9 flags 0x44, mode font 9 flags 0x44, state font 9 flags 0x44, all size 0xe)
// DC-DEFAULT: CLI-539 (proposed) layout keys `dyn/room<row>/<style key sha>`
pub fn room_row_wants(st: &RowStyle, row: usize, at: [i32; 2]) -> [DynWant; 6] {
    use row_keys as k;
    let want = |pin: &KeyPin, p: [i32; 2], align: Align, font: i32| DynWant {
        key: format!("dyn/room{row}/{}", pin.sha16),
        text_at: [(at[0] + p[0]) as f32, (at[1] + p[1]) as f32],
        text_anchor: anchor_of(align),
        font: Some(font),
        font_size: Some(ROOM_ROW_SIZE),
    };
    let [a, b] = ROOM_ROW_FONTS;
    [
        want(&k::INDEX_AT, st.index_at, CENTRE, a),
        want(&k::NAME_AT, st.name_at, LEFT_MID, b),
        want(&k::MAP_AT, st.map_at, CENTRE, b),
        want(&k::MODE_AT, st.mode_at, CENTRE, a),
        want(&k::MEMBERS_AT, st.members_at, CENTRE, a),
        want(&k::STATE_AT, st.state_at, CENTRE, a),
    ]
}

/// Draws the room rows of `rooms` into list `list` (`selected` / `hover` row indices); with `notes` also notes what
/// their texts ask for ([`room_row_wants`]).
#[allow(clippy::too_many_arguments)]
pub fn draw_rooms(
    st: &RowStyle,
    cat: &mut Catalog,
    list: &Part,
    rooms: &[RoomRow],
    selected: Option<usize>,
    hover: Option<usize>,
    out: &mut Vec<DrawItem>,
    notes: Option<&mut DynNotes>,
) {
    draw_rooms_named(
        st,
        cat,
        list,
        rooms,
        &Default::default(),
        selected,
        hover,
        out,
        notes,
    )
}

/// [`draw_rooms`] with the channel's Gun Game rooms (LG-305): their mode column says so.
#[allow(clippy::too_many_arguments)]
pub fn draw_rooms_named(
    st: &RowStyle,
    cat: &mut Catalog,
    list: &Part,
    rooms: &[RoomRow],
    gun_rooms: &std::collections::BTreeSet<u32>,
    selected: Option<usize>,
    hover: Option<usize>,
    out: &mut Vec<DrawItem>,
    mut notes: Option<&mut DynNotes>,
) {
    let [font_a, font_b] = ROOM_ROW_FONTS;
    for (i, r) in rooms.iter().enumerate() {
        // rows scrolled above the list or below its end are not drawn
        let Some(at) = list_row(list, st.size[1], i, 0) else {
            continue;
        };
        let wants = notes.as_ref().map(|_| room_row_wants(st, i, at));
        let mut note = |out: &Vec<DrawItem>, k: usize| {
            if let (Some(n), Some(w)) = (notes.as_deref_mut(), wants.as_ref()) {
                n.push((out.len() - 1, w[k].clone()));
            }
        };
        let sel = selected == Some(i);
        if sel || hover == Some(i) {
            if let Some(t) = &st.bar {
                out.push(image(t, at, st.size));
            }
        }
        let c = if sel { OWN_COLOUR } else { ROW_COLOUR };
        let text = |out: &mut Vec<DrawItem>, t: &str, p: [i32; 2], align: Align, font: i32| {
            out.push(place_text(
                t,
                add(at, p),
                [0, 0],
                align,
                font,
                ROOM_ROW_SIZE,
                c,
            ));
        };
        text(
            out,
            &room_list_number(r.number),
            st.index_at,
            CENTRE,
            font_a,
        );
        note(out, 0);
        if r.locked {
            if let Some(t) = &st.lock {
                out.push(image(t, add(at, st.lock_at), st.lock_size));
            }
        }
        text(out, &r.title, st.name_at, LEFT_MID, font_b);
        note(out, 1);
        let map = cat.map_label(r.map);
        text(out, &map, st.map_at, CENTRE, font_b);
        note(out, 2);
        let mode = if gun_rooms.contains(&r.number) {
            dc_wire::gungame::MODE_NAME.to_string()
        } else {
            cat.mode_name(r.mode)
        };
        text(out, &mode, st.mode_at, CENTRE, font_a);
        note(out, 3);
        let players = room_list_players(r.players, r.max_players);
        text(out, &players, st.members_at, CENTRE, font_a);
        note(out, 4);
        let state = cat.text(state_text(r.state));
        text(out, &state, st.state_at, CENTRE, font_a);
        note(out, 5);
    }
}

/// Draws the last chat lines into the chat list of the chat panel, in the chat page's font, size and colour.
/// A line kept with its own colour (the waiting room's countdown, GM lines, warnings) draws in that colour.
// EVIDENCE: cshell+0x1f850 (the chat page's font index 4, size 0xd), cshell+0x1ba40 (the page's text colour
// 0xffffffff), cshell+0x1c8d0 (each line built with the page's font and size)
// DC-DEFAULT: CLI-165 (proposed) chat lines 18 px apart, newest at the bottom
// lobby-fixes10: with its scroll bar (the record's scroll flag) the list shows the rows the bar's value reaches
// (it follows the newest line, the wheel looks back); without one the newest lines
pub fn draw_chat(sheet: &Sheet, data: &MenuData, out: &mut Vec<DrawItem>) {
    let Some(list) = sheet.part(CHAT_PANEL, CHAT_LIST, WidgetKind::List) else {
        return;
    };
    let view = data.chat_view();
    let lines = &view;
    let scrolled = matches!(list.look, Look::List { bar: Some(_), .. });
    let n = list_capacity(list, CHAT_ROW).min(6);
    let start = if scrolled {
        0
    } else {
        lines.len().saturating_sub(n)
    };
    for (k, (line, colour)) in lines.iter().enumerate().skip(start) {
        let at = if scrolled {
            match list_row(list, CHAT_ROW, k, 0) {
                Some(at) => at,
                None => continue,
            }
        } else {
            [
                list.pos[0],
                list.pos[1] + (k - start) as i32 * (CHAT_ROW + 2),
            ]
        };
        let c = match *colour {
            0 => CHAT_COLOUR,
            c => c,
        };
        out.push(place_text(
            line,
            [at[0] + 4, at[1]],
            [0, 0],
            Align::default(),
            CHAT_PAGE_FONT,
            CHAT_PAGE_SIZE,
            c,
        ));
    }
}

pub struct LobbyScreen {
    sheet: Sheet,
    style: RowStyle,
    selected: Option<usize>,
    hover: Option<usize>,
    last_click: Option<(usize, Instant)>,
    busy: Wait,
    art: Option<PanelArt>,
    extras: CharExtras,
    /// NX credit and prepaid balance (0x345); both read 0 until the first reply.
    balance: (u32, u32),
    /// lobby-nav: the screen the channel leave goes to (change server: the server screen, change channel: the
    /// channel screen).
    leave_to: Go,
    /// The selected and the pointed user rows, and the open user menu (with the entry under the pointer).
    user_selected: Option<usize>,
    user_hover: Option<usize>,
    user_menu: Option<UserMenu>,
    menu_hover: Option<usize>,
    /// The open chat combo list.
    drop: Option<super::dropdown::Dropdown>,
}

impl LobbyScreen {
    pub fn new(cat: &Catalog, data: &MenuData) -> LobbyScreen {
        let mut sheet = lobby_sheet(cat);
        textbox::add_chat_edit(&mut sheet, ChatScreen::Lobby);
        let mut s = LobbyScreen {
            sheet,
            style: RowStyle::load(cat),
            selected: None,
            hover: None,
            last_click: None,
            drop: None,
            busy: Wait::default(),
            art: cat.rom.as_deref().and_then(PanelArt::cached),
            extras: CharExtras::default(),
            balance: (0, 0),
            leave_to: Go::Channels,
            user_selected: None,
            user_hover: None,
            user_menu: None,
            menu_hover: None,
        };
        s.fill_char(cat, data);
        s
    }

    /// The open user menu.
    pub fn user_menu(&self) -> Option<UserMenu> {
        self.user_menu
    }

    /// The selected user row.
    pub fn user_selected(&self) -> Option<usize> {
        self.user_selected
    }

    fn user_list(&self) -> Option<&Part> {
        self.sheet.part(USER_WINDOW, USER_LIST, WidgetKind::List)
    }

    fn user_hit(&self, p: [f32; 2], n: usize) -> Option<usize> {
        self.user_list()
            .and_then(|l| list_hit(l, self.style.user_size[0], self.style.user_size[1], n, 0, p))
    }

    /// A press on a user row: the row is selected and the user menu opens at the pointer (a user without an account
    /// number gets no menu).
    // EVIDENCE: cshell+0x121880 (a press on the user list selects the row and calls cshell+0x120250: the menu at the
    // pointer, shown unless the row's account number is 0; a press elsewhere hides it)
    fn open_user_menu(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        let Some(i) = self.user_hit(p, ctx.data.users.len()) else {
            return false;
        };
        self.user_selected = Some(i);
        let u = &ctx.data.users[i];
        self.user_menu = (u.account != 0).then_some(UserMenu {
            at: [p[0] as i32, p[1] as i32],
            row: i,
            id: u.id,
        });
        self.menu_hover = None;
        true
    }

    /// A press while the user menu is open: an entry runs, any press closes the menu; true = the press is taken.
    // EVIDENCE: cshell+0x129e00 (command 0xf0: entry 1 -> command 0x3a -> 0x378 with the user's in-channel id,
    // cshell+0x18dd70)
    // DC-DEFAULT: CLI-474 (staged) the whisper entry only closes the menu (no chat typing or messenger yet)
    fn menu_press(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        let Some(m) = self.user_menu.take() else {
            return false;
        };
        match m.entry_at(p) {
            Some(USER_MENU_PROFILE) => {
                ctx.send(LinkCmd::Profile(m.id));
                true
            }
            // w3-clan: entry 0 hands the user's name to the messenger's add-friend call (string 0x13f7)
            // EVIDENCE: cshell+0x129e00 (command 0xf0 entry 0: the selected user's name through the messenger object's
            // vtable +0xac)
            // DC-DEFAULT: CLI-565 (proposed) the add goes out as the relay's `!nm add` (amends CLI-474)
            Some(USER_MENU_WHISPER) => {
                if let Some(u) = ctx.data.users.get(m.row) {
                    ctx.send(LinkCmd::Messenger(super::messenger::add_command(&u.name)));
                }
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    /// The character data the link delivered before this screen existed (0x303 extras, grade, balance).
    pub fn apply_char_data(
        &mut self,
        cat: &Catalog,
        data: &MenuData,
        extras: &CharExtras,
        balance: (u32, u32),
    ) {
        self.extras = extras.clone();
        self.balance = balance;
        self.fill_char(cat, data);
    }

    fn list(&self) -> Option<&Part> {
        self.sheet.part(ROOM_WINDOW, ROOM_LIST, WidgetKind::List)
    }

    fn fill_char(&mut self, cat: &Catalog, data: &MenuData) {
        fill_char_panel(
            &mut self.sheet,
            cat,
            data,
            self.art.as_ref(),
            &self.extras,
            self.balance,
        );
    }

    fn join(&mut self, ctx: &mut Ctx, row: Option<RoomRow>) {
        if self.busy.busy() {
            return;
        }
        let Some(r) = row else {
            ctx.notice(text_ids::CANNOT_ENTER);
            return;
        };
        if r.locked {
            let t = ctx.cat.text(0x341);
            ctx.popups.push(Popup::password(t, r.number));
            return;
        }
        ctx.send(LinkCmd::JoinRoom {
            number: r.number,
            password: String::new(),
        });
        self.busy.start();
    }
}

fn set_char_image(sheet: &mut Sheet, id: i32, tex: &str) {
    if let Some(p) = sheet.part_mut(CHAR_WINDOW, id, WidgetKind::Image) {
        if let Look::Panel { tex: t, .. } = &mut p.look {
            *t = tex.to_string();
        }
    }
}

/// The Character Info panel: nickname, rank icon, level and progress, clan, NX prepaid and credit, GP, grade.
/// Shared by the room list and the room creation box drawn over it (the 2009 keeps the panel filled under the box).
// EVIDENCE: cshell+0x126fa0 (window 7: text 3 name, image 0 rank icon of the experience, text 4 short level
// name, text 5 `%d/%d (%3.2f %%)`, text 7 clan name or string 0x41a, image 1 the empty mark frame, text 9
// prepaid (shell +0x11c), text 11 credit (+0x108), text 13 GP (+0x104), text 15 grade name in the grade colour,
// image 2 grade icon; numbers with thousands separators, cshell+0x1b3ed0)
// EVIDENCE: cshell+0x126fa0 (texts 9, 11 and 13: after the number the colour setter, vtable +0xb0, gets
// 0xffffffff for all four colours; text 15: the grade row's colour, cshell+0x285330)
// OBSERVED: lobbyfx4/real/R10_lobby.png (2009: the prepaid, credit and GP numbers are drawn
// 255 / 255 / 255 where their records say 170; the grade name "Common" is drawn 180 where its record says 179)
// OBSERVED: lobbyfx4/real/R1_create.png (2009: the Character Info values stay drawn under the room
// creation box)
pub fn fill_char_panel(
    sheet: &mut Sheet,
    cat: &Catalog,
    data: &MenuData,
    art: Option<&PanelArt>,
    extras: &CharExtras,
    balance: (u32, u32),
) {
    let p = &data.profile;
    sheet.set_text(CHAR_WINDOW, 3, &p.nick);
    sheet.set_text(CHAR_WINDOW, 13, &grouped(i64::from(p.gp)));
    sheet.set_text(CHAR_WINDOW, 9, &grouped(i64::from(balance.1)));
    sheet.set_text(CHAR_WINDOW, 11, &grouped(i64::from(balance.0)));
    for id in [9, 11, 13] {
        sheet.set_colour(CHAR_WINDOW, id, CHAR_NUMBER_COLOUR);
    }
    // clan: the name, or "no clan" (string 0x41a) without one
    let clan = if extras.clan_id != 0 {
        extras.clan_label.clone()
    } else {
        cat.text(0x41a)
    };
    sheet.set_text(CHAR_WINDOW, 7, &clan);
    // the empty mark frame shows unless a mark is set (the mark itself is not drawn yet: open item)
    let marked = extras.clan_id != 0 && extras.clan_mark != 0 && extras.clan_mark_ground != 0;
    sheet.show(CHAR_WINDOW, 1, WidgetKind::Image, !marked);
    let Some(art) = art else {
        return;
    };
    sheet.set_text(CHAR_WINDOW, 4, art.short_name(p.exp));
    sheet.set_text(CHAR_WINDOW, 5, &art.progress(p.exp));
    if let Some(icon) = art.levels.icon_of_exp(p.exp, &p.nick) {
        set_char_image(sheet, 0, icon);
    }
    let row = art.grade(extras.grade);
    sheet.set_text(CHAR_WINDOW, 15, row.map_or("", |r| r.name.as_str()));
    sheet.set_colour(CHAR_WINDOW, 15, row.map_or(0xffff_ffff, |r| r.colour));
    set_char_image(sheet, 2, row.map_or("", |r| r.icon.as_str()));
}

/// The lobby pages of the room list screen.
pub fn lobby_sheet(cat: &Catalog) -> Sheet {
    let mut sheet = build_sheet(cat, &[pages::LOBBY_COMMON, pages::CHAT, pages::ROOM_LIST]);
    lobby_hides(&mut sheet);
    lobby_chat(&mut sheet, cat);
    mark_frame_size(&mut sheet);
    sheet
}

/// The Character Info panel's empty mark frame (image 1 of window 7) is shrunk to the mark's 26 x 26 at its place.
// DISASM: cshell+0x121663..0x121693 (room list build: control 1 of window 7 -> size 0x1a x 0x1a through
// cshell+0x3430a0, both scales 0x3ed2d2d3 (0.41) through vtable +0x144, hidden; cshell+0x126fa0 shows it without a
// clan or a mark)
pub fn mark_frame_size(sheet: &mut Sheet) {
    if let Some(p) = sheet.part_mut(CHAR_WINDOW, super::clan::CHAR_MARK_FRAME, WidgetKind::Image) {
        let [w, h] = p.size.map(|v| v.max(1) as u32);
        p.size = super::clan::MARK_SIZE;
        if let Look::Panel { tex, stretch } = &mut p.look {
            // the record's texels (its own rectangle of the texture) over the new size
            *tex = crate::hud::compose::part_of(tex, [0, 0, w, h]);
            *stretch = true;
        }
    }
}

/// The room list screen narrows the chat page (whose own places fit the waiting room) to the left of the character
/// info window: the chat lists' scroll bars move to x 520 and the Enter button gets the screen's own smaller
/// textures, caption position and place.
// EVIDENCE: cshell+0x1211b0 (room list build: lists 4, 5, 6 of chat window 2 -> each list's scroll bar (list +0x228)
// to (0x208, y) through cshell+0x2f0310 (scroll bar vtable +0x20); control 0 of window 2 -> four textures (vtable
// +0x164, catalog::pins::LOBBY_ENTER), caption position (0x1e, 0xc) (vtable +0x14c), place (0x1c5, 0x2a9) (vtable
// +0x20))
// OBSERVED: lobbyfx4/real/R10_lobby.png (2009: the Enter button drawn x 453..512, lit 459..505,
// y 682..704, its caption centred near x 482; the chat scroll bar near x 520; the character info window from x 549)
// DC-DEFAULT: CLI-467 (proposed) the scroll bars keep the record's y offset (the 2009 y comes from the list's vtable
// +0x24 result, not read); the four textures are used through their common prefix with the menu's state letters
// w2-lobbychat: the chat window takes the room list's own narrow panel (`textbox::chat_panel`); the run-time chat
// edit box is built by the room list itself (`textbox::add_chat_edit`), not by the room creation box over it
pub fn lobby_chat(sheet: &mut Sheet, cat: &Catalog) {
    textbox::chat_panel(sheet, cat, ChatScreen::Lobby);
    let prefix = cat.paths.lobby_enter_prefix();
    for p in sheet.parts.iter_mut().filter(|p| p.page == pages::CHAT) {
        match p.kind {
            WidgetKind::List if p.layer == CHAT_PANEL && CHAT_LISTS.contains(&p.id) => {
                if let Look::List { scroll_at, .. } = &mut p.look {
                    scroll_at[0] = LOBBY_CHAT_SCROLL_X - p.pos[0];
                }
            }
            WidgetKind::Button if p.layer == CHAT_PANEL && p.id == CHAT_ENTER => {
                p.pos = LOBBY_ENTER_AT;
                p.text_at = LOBBY_ENTER_CAPTION_AT;
                p.placed = true;
                if let Look::Button {
                    prefix: tex,
                    caption,
                    ..
                } = &mut p.look
                {
                    if let Some(c) = caption {
                        c.at = LOBBY_ENTER_CAPTION_AT;
                    }
                    if let Some(pre) = &prefix {
                        *tex = pre.clone();
                    }
                }
            }
            _ => {}
        }
    }
}

/// What the room list screen hides: the room tooltip, the display window, the character panel's clan mark, the
/// chat page's clan tab (no clan chat).
// DC-DEFAULT: CLI-171 (proposed) the chat page's clan tab is hidden (dc-client has no clan chat)
pub fn lobby_hides(sheet: &mut Sheet) {
    sheet.show_window(TOOLTIP_WINDOW, false);
    sheet.show_window(DISPLAY_WINDOW, false);
    sheet.show(CHAR_WINDOW, 1, WidgetKind::Image, false);
    for p in sheet.parts.iter_mut() {
        if p.page == pages::CHAT && p.parent.is_none() && p.kind == WidgetKind::Button {
            p.shown = false;
        }
    }
}

/// A chat line's height in the chat list (17 px apart with the list's 2 px spacing).
// OBSERVED: lobby10/cpp tuncpp_bb00_chat_end.png / tuncpp_bb01_chat_back1.png (the reference stack's lobby chat: six lines
// 17 px apart at the list's end, one line back per wheel notch)
pub const CHAT_ROW: i32 = 15;

impl Screen for LobbyScreen {
    fn list_follow(&self) -> Vec<(i32, i32)> {
        vec![(CHAT_PANEL, CHAT_LIST)]
    }
    fn list_rows(&self, data: &MenuData) -> Vec<(i32, i32, usize, i32)> {
        vec![
            (CHAT_PANEL, CHAT_LIST, data.chat_view().len(), CHAT_ROW),
            (ROOM_WINDOW, ROOM_LIST, data.rooms.len(), self.style.size[1]),
            (
                USER_WINDOW,
                USER_LIST,
                data.users.len(),
                self.style.user_size[1],
            ),
        ]
    }
    fn name(&self) -> &'static str {
        "lobby"
    }
    fn dropdown(&self) -> Option<&super::dropdown::Dropdown> {
        self.drop.as_ref()
    }
    fn dropdown_mut(&mut self) -> Option<&mut super::dropdown::Dropdown> {
        self.drop.as_mut()
    }
    fn close_dropdown(&mut self) {
        self.drop = None;
    }
    fn dropdown_pick(&mut self, list: &'static str, value: i32, _index: usize, ctx: &mut Ctx) {
        super::chatdrop::pick(&mut self.sheet, ctx.cat, ctx.data, list, value);
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    fn enter(&mut self, ctx: &mut Ctx) {
        ctx.send(LinkCmd::Rooms);
    }

    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        let n = ctx.data.rooms.len();
        match k {
            // Enter sends the chat field (cshell+0x205e0: key 0xd -> command 0x75); with an empty field it joins
            // the selected room (CLI-483)
            MenuKey::Enter if !textbox::typed_chat(&self.sheet).is_empty() => {
                self.click(CMD_CHAT_SEND, usize::MAX, ctx)
            }
            MenuKey::Enter => self.click(CMD_ENTER, usize::MAX, ctx),
            // the window delivers Space as a key: a space in the chat box (CLI-483)
            MenuKey::Space => {
                self.sheet.type_char(' ');
            }
            MenuKey::Escape => self.click(CMD_TOP_EXIT, usize::MAX, ctx),
            // CLI-1173: F2 / F3 set the chat target (cshell+0x205e0)
            MenuKey::F2 | MenuKey::F3 => {
                super::chatdrop::key_target(&mut self.sheet, ctx.cat, ctx.data, k);
            }
            MenuKey::Down if n > 0 => {
                self.selected = Some(self.selected.map_or(0, |s| (s + 1).min(n - 1)))
            }
            MenuKey::Up if n > 0 => {
                self.selected = Some(self.selected.map_or(0, |s| s.saturating_sub(1)))
            }
            _ => {}
        }
    }

    fn click(&mut self, command: i32, _part: usize, ctx: &mut Ctx) {
        match command {
            CMD_CHAT_SEND => {
                textbox::submit_chat(&mut self.sheet, ctx);
            }
            CMD_ENTER => {
                let row = self.selected.and_then(|i| ctx.data.rooms.get(i)).cloned();
                self.join(ctx, row);
            }
            CMD_QUICK => {
                // the first room that is not full, not in state 3 and not locked, then the enter command
                // EVIDENCE: cshell+0x129ac0 (players != max, state != 3, lock flag clear -> command 0xcf)
                let row = ctx
                    .data
                    .rooms
                    .iter()
                    .find(|r| !r.locked && r.players != r.max_players && r.state != 3)
                    .cloned();
                self.join(ctx, row);
            }
            CMD_CREATE => *ctx.go = Some(Go::Create),
            // change server (67) and change channel (20008) both leave the channel; the leave's answer opens the
            // server screen or the channel screen
            // EVIDENCE: cshell+0x129e00 (0x43 clears the screen's +0x5581, 0x4e28 sets it, both send 0x374),
            // cshell+0x35010 (0x375: +0x5581 clear -> screen 0xb, set -> 0xc)
            CMD_EXIT | CMD_CHANGE_CHANNEL => {
                self.leave_to = if command == CMD_EXIT {
                    Go::Servers
                } else {
                    Go::Channels
                };
                ctx.send(LinkCmd::LeaveChannel);
            }
            CMD_TOP_EXIT => {
                let t = ctx.cat.text(text_ids::ASK_QUIT);
                ctx.popups.push(Popup::ask(t, BoxTag::Quit));
            }
            // w3-clan: the lobby bar's clan button opens the clan screen, its messenger button the messenger window
            // EVIDENCE: cshell+0x1bf20 (0xa8 -> screen 0x1c; 0x136 -> cshell+0x12b350)
            super::clan::BAR_CLAN_CMD => *ctx.go = Some(Go::Clan),
            super::messenger::BAR_FRIENDS_CMD => ctx.data.friends.toggle_asked = true,
            // w3-shop: the bar's Shop / Storage, Black Market, Modification and Inbox
            c => {
                super::shop::bar_command(c, Go::Lobby, ctx);
            }
        }
    }

    fn press(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        if self.menu_press(p, ctx) {
            return true;
        }
        // the chat combos: a press shows the list of the combo under it, or hides it when shown
        // EVIDENCE: cshell+0x1bf20 (0x274 / 0x278 toggle their lists)
        let was = self.drop.take().map(|d| d.list);
        if let Some(cmd) = super::chatdrop::combo_at(&self.sheet, p) {
            let list = if cmd == super::chatdrop::CMD_FILTER {
                super::chatdrop::LIST_FILTER
            } else {
                super::chatdrop::LIST_TARGET
            };
            if was != Some(list) {
                self.drop = super::chatdrop::open(&self.sheet, ctx.cat, cmd);
            }
            return true;
        }
        // DC-DEFAULT: CLI-471 (staged) a left press on a user row opens the user menu as the 2009 handler of the
        // screen's mouse slot 0x50 does (cshell+0x121880; that slot is the other button, PLAUSIBLE)
        if self.open_user_menu(p, ctx) {
            return true;
        }
        let Some(list) = self.list().cloned() else {
            return false;
        };
        let n = ctx.data.rooms.len();
        let Some(i) = list_hit(&list, self.style.size[0], self.style.size[1], n, 0, p) else {
            return false;
        };
        let double = self
            .last_click
            .is_some_and(|(j, t)| j == i && t.elapsed().as_millis() < DOUBLE_CLICK_MS);
        self.last_click = if double {
            None
        } else {
            Some((i, Instant::now()))
        };
        self.selected = Some(i);
        if double {
            self.click(CMD_ENTER, usize::MAX, ctx);
        }
        true
    }

    fn press_alt(&mut self, p: [f32; 2], ctx: &mut Ctx) {
        self.user_menu = None;
        self.open_user_menu(p, ctx);
    }

    fn hover(&mut self, p: [f32; 2], data: &MenuData) {
        self.hover = self.list().and_then(|l| {
            list_hit(
                l,
                self.style.size[0],
                self.style.size[1],
                data.rooms.len(),
                0,
                p,
            )
        });
        self.user_hover = self.user_hit(p, data.users.len());
        self.menu_hover = self.user_menu.and_then(|m| m.entry_at(p));
    }

    fn popup_done(&mut self, p: &Popup, yes: bool, ctx: &mut Ctx) {
        match (p.tag, yes) {
            (BoxTag::RoomPassword(number), true) if !p.input.is_empty() => {
                ctx.send(LinkCmd::JoinRoom {
                    number,
                    password: p.input.clone(),
                });
                self.busy.start();
            }
            // w2-lobbychat: Yes on an invitation enters its room with the room's password (0x384)
            // SPEC: PROTOCOL_2009.md §3.7 (0x3023: Yes -> 0x384)
            (BoxTag::Invite(number), true) if !self.busy.busy() => {
                ctx.send(LinkCmd::JoinRoom {
                    number,
                    password: p.input.clone(),
                });
                self.busy.start();
            }
            _ => {}
        }
    }

    fn event(&mut self, e: &LinkEvent, ctx: &mut Ctx) {
        match e {
            LinkEvent::Rooms(v) => {
                if self.selected.is_some_and(|s| s >= v.len()) {
                    self.selected = None;
                }
            }
            LinkEvent::Users(_)
            | LinkEvent::UserAdded(_)
            | LinkEvent::UserRemoved(_)
            | LinkEvent::Profile(_) => {
                // a changed user list drops a selection past its end
                let n = ctx.data.users.len();
                if self.user_selected.is_some_and(|s| s >= n) {
                    self.user_selected = None;
                    self.user_menu = None;
                }
                self.fill_char(ctx.cat, ctx.data);
            }
            LinkEvent::CharInfo(x) => {
                self.extras = x.clone();
                self.fill_char(ctx.cat, ctx.data);
            }
            LinkEvent::Grade(g) => {
                self.extras.grade = *g;
                self.fill_char(ctx.cat, ctx.data);
            }
            LinkEvent::Balance { credit, prepaid } => {
                self.balance = (*credit, *prepaid);
                self.fill_char(ctx.cat, ctx.data);
            }
            LinkEvent::RoomEntered { .. } => {
                self.busy.stop();
                *ctx.go = Some(Go::Room);
            }
            LinkEvent::EnterRefused(reason) => {
                self.busy.stop();
                let id = match reason {
                    0x2a => text_ids::WRONG_PASSWORD,
                    0x8a => text_ids::KICKED_HERE,
                    _ => text_ids::CANNOT_ENTER,
                };
                ctx.notice(id);
            }
            LinkEvent::ChannelLeft => *ctx.go = Some(self.leave_to),
            // CLI-931: the reply opens the Character Info box (cshell+0x35cf0 fills the box object, whose build is
            // cshell+0x2412c0), not a message box
            LinkEvent::ProfileInfo(p) => {
                ctx.data.char_view = Some(p.clone());
                *ctx.go = Some(Go::CharInfo);
            }
            // w2-lobbychat: an invitation shows on the room list only (cshell+0x39110)
            LinkEvent::Invite {
                number,
                host,
                password,
            } => {
                // CLI-1141: Block Invitations shows no invitation box
                if !ctx.data.settings.block_invites() {
                    ctx.popups.push(Popup::invite(
                        ctx.cat,
                        *number,
                        host,
                        password,
                        Instant::now(),
                    ));
                }
            }
            _ => {}
        }
    }

    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, _secs: f32, out: &mut Vec<DrawItem>) {
        self.draw_lists(cat, data, out, None);
    }

    fn draw_extra_noted(
        &self,
        cat: &mut Catalog,
        data: &MenuData,
        _secs: f32,
        out: &mut Vec<DrawItem>,
        notes: &mut DynNotes,
    ) {
        self.draw_lists(cat, data, out, Some(notes));
    }
}

impl LobbyScreen {
    /// The room and user rows, the chat lines and the user menu; with `notes` the rows note their 2009 rules.
    fn draw_lists(
        &self,
        cat: &mut Catalog,
        data: &MenuData,
        out: &mut Vec<DrawItem>,
        mut notes: Option<&mut DynNotes>,
    ) {
        if let Some(list) = self.list() {
            draw_rooms_named(
                &self.style,
                cat,
                list,
                &data.rooms,
                &data.gun_rooms,
                self.selected,
                self.hover,
                out,
                notes.as_deref_mut(),
            );
        }
        draw_users(
            &self.style,
            &self.sheet,
            data,
            self.art.as_ref().map(|a| &a.levels),
            self.user_hover,
            out,
            notes,
        );
        if let Some(rom) = cat.rom.clone() {
            let lits = super::clan::ClanLits::cached(&rom);
            draw_user_marks(&self.style, &self.sheet, data, &lits.marks, out);
            let countries = super::charinfo::Tables::cached(&rom).countries;
            draw_user_flags(
                &self.style,
                &self.sheet,
                data,
                &mut |nation| {
                    let flag = &countries.get(nation as usize)?.2;
                    let t = cat.assets.texture(flag)?;
                    Some((flag.clone(), [t.width as f32, t.height as f32]))
                },
                out,
            );
        }
        draw_chat(&self.sheet, data, out);
        textbox::draw_ticker(cat, &data.ticker, Instant::now(), out);
        // w3-clan (CLI-389): the clan mark at the Character Info panel's empty frame
        super::clan::draw_panel_mark(cat, &self.sheet, data, out);
        if let Some(m) = self.user_menu {
            let r = m.rect();
            out.push(DrawItem::Fill {
                rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
                colour: [16, 16, 16, 235],
            });
            for (i, id) in USER_MENU_TEXTS.iter().enumerate() {
                let e = m.entry_rect(i);
                if self.menu_hover == Some(i) {
                    out.push(DrawItem::Fill {
                        rect: [e[0] as f32, e[1] as f32, e[2] as f32, e[3] as f32],
                        colour: [90, 70, 20, 255],
                    });
                }
                out.push(place_text(
                    &cat.text(*id),
                    [e[0] + 6, e[1]],
                    [0, e[3]],
                    LEFT_MID,
                    14,
                    13,
                    ROW_COLOUR,
                ));
            }
        }
    }
}

/// The channel's users (own row in the own colour; the row bar under every row; the pointed row's name in the own
/// colour).
///
/// Each row's badge is the rank icon of the user's experience (the level table read at run time); without a table
/// or an icon the layout's one badge stays.
// EVIDENCE: cshell+0x120d90 (user row bar texture and size at the bar position), cshell+0x11f990 (the row always
// draws its bar), cshell+0x11fc10 (the pointer on the row: name 0xfffead00, else the row's colour)
// OBSERVED: lobbyfx4/real/R10_lobby.png (2009: the user row with its bar)
// EVIDENCE: cshell+0x120d90 (user row rank texture), cshell+0x2683d0 / 0x268300 (the icon of the level of the
// experience; the operator mark picks the last icon), SPEC: PROTOCOL_2009.md §3.6 table 3.6b (+0x2a exp)
pub fn draw_users(
    st: &RowStyle,
    sheet: &Sheet,
    data: &MenuData,
    levels: Option<&LevelTable>,
    hover: Option<usize>,
    out: &mut Vec<DrawItem>,
    mut notes: Option<&mut DynNotes>,
) {
    let Some(list) = sheet.part(USER_WINDOW, USER_LIST, WidgetKind::List) else {
        return;
    };
    for (i, u) in data.users.iter().enumerate() {
        let Some(at) = list_row(list, st.user_size[1], i, 0) else {
            continue;
        };
        if let Some(t) = &st.user_bar {
            out.push(image(t, add(at, st.user_bar_at), st.user_size));
        }
        let rank = levels.and_then(|l| l.icon_of_exp(u.exp, &u.name));
        if let Some(t) = rank.or(st.user_badge.as_deref()) {
            out.push(image(t, add(at, st.user_badge_at), st.user_badge_size));
        }
        let c = if u.id == data.profile.my_id || hover == Some(i) {
            OWN_COLOUR
        } else {
            ROW_COLOUR
        };
        out.push(place_text(
            &u.name,
            add(at, st.user_name_at),
            [0, 0],
            LEFT_MID,
            USER_ROW_FONT,
            USER_ROW_SIZE,
            c,
        ));
        if let Some(n) = notes.as_deref_mut() {
            n.push((out.len() - 1, user_row_want(st, i, at)));
        }
    }
}

/// The clan mark of each user row that has a clan and both mark layers: 21 x 21 art (the mark control's 51-texel
/// rectangle at the scale 21 / 51) at the row's UserClanMarkPos.
// EVIDENCE: cshell+0x11fa00 (UserClanMarkPos moves the mark control with the row), cshell+0x120d90 (the user row's
// mark control: scale 0x3ed2d2d3 x 0x3ed2d2d3, size 0x1a x 0x1a, layers shown when both ids are set), SPEC:
// PROTOCOL_2009.md §3.6 table 3.6b (+0x2e clan, +0x32 emblem, +0x36 background)
// CLI-940: the rows drew no clan mark at all
pub fn draw_user_marks(
    st: &RowStyle,
    sheet: &Sheet,
    data: &MenuData,
    marks: &super::clan::ClanMarks,
    out: &mut Vec<DrawItem>,
) {
    let Some(list) = sheet.part(USER_WINDOW, USER_LIST, WidgetKind::List) else {
        return;
    };
    for (i, u) in data.users.iter().enumerate() {
        if u.clan == 0 || u.mark == 0 || u.ground == 0 {
            continue;
        }
        let Some(at) = list_row(list, st.user_size[1], i, 0) else {
            continue;
        };
        let pos = add(at, st.user_mark_at);
        let r = super::clan::MARK_PANEL_DRAWN;
        if let Some((e, b)) = marks.textures(u.mark, u.ground) {
            super::clanpage::mark_cropped(Some(b), Some(e), [pos[0], pos[1], r[0], r[1]], out);
        }
    }
}

/// The nation flag of each user row with a nation: the country row's `FlagImg` texture at the row plus the style's
/// NationalFlagPos, in the country window's FlagSize (the texture's own size when the layout has none). `flag_of`
/// gives the texture name and its size for a nation index.
// EVIDENCE: cshell+0x11fa00 (NationalFlagPos moves the flag control with the row, in the order bar, name, rank, clan
// mark, flag, PC room), cshell+0x120d90 (the row's flag control: texture of the row's nation, size
// CountryChangeWnd / FlagSize, shown for a non-zero nation through cshell+0x11f5f0), SPEC: PROTOCOL_2009.md §3.6
// table 3.6b (+0x3e nation index), CLI-961 (CountryInfoTable `FlagImg` of row `Country<nation>`)
// CLI-1024: the rows drew no flag
pub fn draw_user_flags(
    st: &RowStyle,
    sheet: &Sheet,
    data: &MenuData,
    flag_of: &mut dyn FnMut(u32) -> Option<(String, [f32; 2])>,
    out: &mut Vec<DrawItem>,
) {
    let Some(list) = sheet.part(USER_WINDOW, USER_LIST, WidgetKind::List) else {
        return;
    };
    for (i, u) in data.users.iter().enumerate() {
        if u.nation == 0 {
            continue;
        }
        let Some((tex, src)) = flag_of(u.nation) else {
            continue;
        };
        let Some(at) = list_row(list, st.user_size[1], i, 0) else {
            continue;
        };
        let pos = add(at, st.user_flag_at);
        let size = if st.flag_size[0] > 0 && st.flag_size[1] > 0 {
            [st.flag_size[0] as f32, st.flag_size[1] as f32]
        } else {
            src
        };
        out.push(DrawItem::Image {
            tex,
            src,
            dst: [pos[0] as f32, pos[1] as f32, size[0], size[1]],
            tint: [255; 4],
            turn: None,
        });
    }
}

/// Font record and size of a user row's name.
// EVIDENCE: cshell+0x120d90 (name control: font lookup cshell+0x1c07b0 with 3, size 0xd, flags 0x41)
pub const USER_ROW_FONT: i32 = 3;
pub const USER_ROW_SIZE: i32 = 13;

/// What the 2009 user row asks of its name at row position `at`: the row plus the name's style position, left /
/// vertically centred, font [`USER_ROW_FONT`], size [`USER_ROW_SIZE`] (the draw trace's copy of the rule, apart from
/// [`draw_users`]).
// EVIDENCE: cshell+0x11fa00 (children at the row position plus UserBarPos / UserNamePos / UserRankPos / ...),
// cshell+0x120d90 (the name control: font 3, size 0xd, flags 0x41)
// DC-DEFAULT: CLI-539 (proposed) layout key `dyn/user<row>/<style key sha>`
pub fn user_row_want(st: &RowStyle, row: usize, at: [i32; 2]) -> DynWant {
    let p = st.user_name_at;
    DynWant {
        key: format!("dyn/user{row}/{}", row_keys::USER_NAME_AT.sha16),
        text_at: [(at[0] + p[0]) as f32, (at[1] + p[1]) as f32],
        text_anchor: anchor_of(LEFT_MID),
        font: Some(USER_ROW_FONT),
        font_size: Some(USER_ROW_SIZE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art() -> PanelArt {
        PanelArt {
            levels: LevelTable {
                bounds: vec![100, 300, 600],
                small: vec!["i0".into(), "i1".into(), "i2".into()],
            },
            short_names: vec!["TRN".into(), "RCT".into(), "PVT".into()],
            grades: vec![GradeRow {
                name: "Common".into(),
                icon: "g0".into(),
                colour: 0xffb4_b4b4,
            }],
        }
    }

    #[test]
    fn progress_text_is_part_over_span_and_percent() {
        let a = art();
        assert_eq!(a.progress(50), "50/100 (50.00 %)");
        assert_eq!(a.progress(100), "0/200 (0.00 %)");
        assert_eq!(a.progress(200), "100/200 (50.00 %)");
        // past the last bound the level wraps to row 0 (cshell+0x267f10)
        assert_eq!(a.progress(1000), "1000/100 (1000.00 %)");
        assert_eq!(a.short_name(250), "RCT");
        assert_eq!(a.short_name(5000), "TRN");
    }

    #[test]
    fn grade_rows_and_the_operator_badge() {
        let a = art();
        assert_eq!(a.grade(0).map(|g| g.icon.as_str()), Some("g0"));
        assert!(a.grade(1).is_none());
        assert_eq!(a.levels.icon_of_exp(250, "x"), Some("i1"));
        assert_eq!(a.levels.icon_of_exp(250, "GM_x"), Some("i2"));
    }

    #[test]
    fn exp_of_reads_the_profile_for_the_own_id_and_the_user_list_for_others() {
        let mut d = MenuData::default();
        d.profile.my_id = 5;
        d.profile.exp = 77;
        d.users.push(super::super::link::UserRow {
            account: 1,
            id: 9,
            name: "n".into(),
            exp: 12,
            ..Default::default()
        });
        assert_eq!(exp_of(&d, 5), Some(77));
        assert_eq!(exp_of(&d, 9), Some(12));
        assert_eq!(exp_of(&d, 4), None);
        assert_eq!(exp_of(&d, 0), None);
        // a room member: the slot record's exp, ahead of the profile and the user list
        for (id, exp) in [(5, 300), (9, 400)] {
            d.members.insert(
                id,
                dc_botclient::lobby::RoomMember {
                    char_no: 1,
                    name: "m".into(),
                    team: 1,
                    state: 0,
                    exp,
                },
            );
        }
        assert_eq!((exp_of(&d, 5), exp_of(&d, 9)), (Some(300), Some(400)));
    }

    #[test]
    fn room_state_strings() {
        assert_eq!(state_text(0), STATE_WAITING);
        assert_eq!(state_text(1), STATE_STARTING);
        assert_eq!(state_text(2), STATE_IN_GAME);
        assert_eq!(state_text(3), STATE_IN_GAME);
    }
}
