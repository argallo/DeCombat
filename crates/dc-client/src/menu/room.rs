//! Waiting room: the two team lists (rank icon, host mark, name, ready state), the room's number and title, the
//! settings panel (map image and name, mode, goal, players, the three room options), the chat panel; Start (host)
//! or Ready (guest), team change, exit. The host changes the settings with the panel's buttons (0x38c). The
//! lobby's start sequence (0x3044 countdown, 0x3043 address) hands the lobby connection to the game session; after
//! the match the screen opens again.
// SPEC: PROTOCOL_2009.md §3.7 (0x388 / 0x389, 0x38a / 0x38b, pushes 0x3033 / 0x3034 / 0x3035 / 0x3041 / 0x3042,
// 0x392 / 0x393: kind 3 reason 10 -> res 0x41b, 11 -> res 0x444; 0x390 / 0x391; 0x38c / 0x38d; 0x386 / 0x387;
// 0x3034 own id: reason 1 -> res 0x1851, 2 -> res 0x565), §7 (0x3044: "starting in (5 - count) s", res 0x57a)
// EVIDENCE: cshell+0x164810 (commands: 13 start (host) / ready, 107 ready, 59 exit, 110 team change: own team 1 ->
// 0x390 team 2, team 2 -> team 1; 234 / 235 mid-match join on / off and 236 friendly fire on as 0x38c types 9
// and 10; 241 / 243 / 245 / 247 / 600 open the map / mode / goal / players / weapon lists of the host), cshell+0x1657e0
// (member row textures and sizes), cshell+0x163d00 (state strings 0x368 / 0x445 / 0x161c)

use super::catalog::{int, key, pair, Catalog, KeyPin, RoomOption};
use super::drawlist::{anchor_of, DynNotes, DynWant};
use super::dropdown::{self, Dropdown};
use super::link::{LinkCmd, LinkEvent};
use super::lobby::{draw_chat, exp_of, CHAT_LIST, CHAT_PANEL};
use super::popup::{text_ids, BoxTag};
use super::roomtext;
use super::sheet::{image, list_row, place_text, Sheet, ALIGN_CENTRE, ALIGN_LEFT};
use super::textbox::{self, ChatScreen, CMD_CHAT_SEND};
use super::{build_sheet, pages, Ctx, Go, MenuData, MenuKey, Screen};
use crate::hud::board::LevelTable;
use crate::hud::items::DrawItem;
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::{UiScreen, WidgetKind};
use std::time::{Duration, Instant};

/// Keys of the member rows in the waiting room's layout section.
// EVIDENCE: cshell+0x1657e0 (row textures and sizes); the position keys are read from the same section
// DISASM: cshell+0x15c660 (the row's place: the row at (x, y), then each control at the row plus its section-9
// position: the bar (+0xfc) at BAR_AT, the master badge (+0x104) at MASTER_AT, the name (+0x118) at NAME_AT, ...)
pub mod row_keys {
    use super::{key, KeyPin};
    pub const BAR_AT: KeyPin = key("3fda9cef6f0fcca6", 10);
    pub const ALPHA_BAR: KeyPin = key("62ac5b5dd91bdf80", 9);
    pub const BRAVO_BAR: KeyPin = key("5a001bc96ea30170", 9);
    pub const BAR_WIDTH: KeyPin = key("5b71eaf3392b6db0", 12);
    pub const BAR_HEIGHT: KeyPin = key("3e38010ae7ad716f", 13);
    pub const LEVEL: KeyPin = key("8e527d072ba143b5", 8);
    pub const LEVEL_WIDTH: KeyPin = key("1fbf4f074d0ea285", 13);
    pub const LEVEL_HEIGHT: KeyPin = key("706351a09892a091", 14);
    pub const LEVEL_AT: KeyPin = key("5d8672f5f1ee1498", 11);
    pub const MASTER: KeyPin = key("216f7a5f00dbea9e", 10);
    pub const MASTER_WIDTH: KeyPin = key("93a7c00d3e27ce92", 14);
    pub const MASTER_HEIGHT: KeyPin = key("f97e25bf9883bf21", 15);
    pub const MASTER_AT: KeyPin = key("0be00a179db322b5", 12);
    pub const NAME_AT: KeyPin = key("d5698f80e06d3478", 7);
    pub const STATE_AT: KeyPin = key("d81c20d304555303", 8);
    /// CLI-1895: the clan name text (+0x11c) and the clan mark (+0x114) of the row.
    // DISASM: cshell+0x15c83d (+0x11c placed at the row plus the clan name key, VA 0x374e591c),
    // cshell+0x15c894 (+0x114 at the row plus the clan mark key, VA 0x374e5928)
    pub const CLAN_NAME_AT: KeyPin = key("75c076cfb5a247c4", 11);
    pub const CLAN_MARK_AT: KeyPin = key("187fff8f806656b4", 11);
}

/// Member state strings: ready, starting, in game; with state 0 the sub-states shopping and away.
// EVIDENCE: cshell+0x163d00
/// The start-failed box (the watchdog).
pub const START_FAILED: i32 = 0x473;
pub const STATE_READY: i32 = 0x368;
pub const STATE_STARTING: i32 = 0x445;
pub const STATE_IN_GAME: i32 = 0x161c;
pub const SUB_SHOPPING: i32 = 0x456;
pub const SUB_AWAY: i32 = 0x457;

/// The waiting room's idle rules: the away flag after a minute without input, the room left two minutes after the
/// away flag went out (three minutes after the last input).
// EVIDENCE: cshell+0x1608b0 (waiting room update: more than 60000 ms since the last input -> 0x392 (1, 2) once,
// the away byte set and the last-input time restarted through the same engine slot the input handlers call;
// then more than 120000 ms since that time -> 0x386 once, i.e. 180 s after the last real input),
// cshell+0x15e970 / 0x15ec10 / 0x15f470 / 0x15f540 / 0x160b00 (the room's input handlers: away set -> 0x392 (1, 0); the input time restarts)
// DC-DEFAULT: CLI-486 (proposed) the idle clock starts when the waiting room opens and every menu input restarts it;
// scripted drivers (pilot, step files) run without the idle rules
pub const IDLE_AWAY: Duration = Duration::from_secs(60);
pub const IDLE_LEAVE: Duration = Duration::from_secs(120);
/// The screen's own timer commands (no button has them).
pub const CMD_IDLE_AWAY: i32 = -0x392;
pub const CMD_IDLE_LEAVE: i32 = -0x386;

/// Window and control ids.
pub const TEAM_WINDOW: i32 = 5;
pub const ROOM_NUMBER: i32 = 9;
pub const ROOM_CAPTION: i32 = 10;
pub const START_PART: i32 = 1;
pub const FORCE_OUT_BUTTON: i32 = 3;
/// CLI-1905: the Change password button (team window control 8, texture `ChangePwd_`, command 20): shown to the host
/// only; it opens the lock box, whose OK sends the typed password as the room's.
// EVIDENCE: cshell+0x15db30 (the host refresh shows (5, 1), (5, 3) and (5, 8) for the host, (5, 7) for the others),
// cshell+0x164810 (command 0x14 -> cshell+0x15fc80, the box; command 0x53 from the box's OK, cshell+0x15baf0 -> the
// typed text to cshell+0x18a9c0 as type 1; Cancel posts 7)
pub const CHANGE_PASSWORD_BUTTON: i32 = 8;
pub const CMD_CHANGE_PASSWORD: i32 = 20;
/// The team change buttons: 5 (Team_change_, arrows to Alpha) and 6 (Team_change_Bravo_, arrows to Bravo); the team
/// labels 0x15 (Alpha) and 0x16 (Bravo), check buttons whose "on" image marks the own team.
// EVIDENCE: cshell+0x15d9d0 (own team 1: button 5 hidden, button 6 shown, label 0x15 on, 0x16 off; team 2: button 5
// shown, 6 hidden, 0x16 on; team 0xff: button 5 shown, both labels off; not in room states 2, 3, 5, 7)
// OBSERVED: lobbyfx4/real/R20_room.png (2009, own team Alpha: Change Teams with the right arrows lit,
// the ALPHA label's bars lit)
pub const TEAM_ALPHA_BUTTON: i32 = 5;
pub const TEAM_BRAVO_BUTTON: i32 = 6;
pub const TEAM_ALPHA_LABEL: i32 = 0x15;
pub const TEAM_BRAVO_LABEL: i32 = 0x16;
/// The own name's colour in the member rows.
// EVIDENCE: cshell+0x1657e0 (the row's name equal to the own name and no colour level: 0xff61ffff)
// OBSERVED: lobbyfx4/real/R20_room.png (2009: the own name cyan)
pub const OWN_NAME_COLOUR: u32 = 0xff61_ffff;

/// CLI-1887: the colour of a member row's name: the colour of the record's name colour number (a name colour item)
/// when it has one, else [`OWN_NAME_COLOUR`] on the own row and the row colour `row` on the others.
// EVIDENCE: cshell+0x15c110 (a number < 1 -> the row's own colour +0x134, else the colour table's entry, onto the
// name control), cshell+0x166310 / 0x166660 / 0x166c90 / 0x1619c0 (the member refreshes set the name through that
// rule from the record), cshell+0x1657e0 (the own row's +0x134 is 0xff61ffff only without a colour number); SPEC:
// PROTOCOL_2009.md §3.7 table 3.7b (+0xac name colour id)
// DC-DEFAULT: CLI-1887 (proposed) a number past the table keeps the row's plain colour (2009: colour word 0)
pub fn member_name_colour(own: bool, row: u32, item: Option<u32>) -> u32 {
    match item {
        Some(c) => c,
        None if own => OWN_NAME_COLOUR,
        None => row,
    }
}

/// CLI-1895: the clan name's colour on a member row without a clan colour number.
// EVIDENCE: cshell+0x1657e0 (the row's clan text +0x11c: its own colour +0x138 is 0xff9898ff when the clan colour
// argument is < 1), DISASM cshell+0x165d0a..0x165d11
pub const CLAN_NAME_COLOUR: u32 = 0xff98_98ff;

/// CLI-1895: the colour of a member row's clan name: the clan colour number's colour when it has one, else
/// [`CLAN_NAME_COLOUR`].
// EVIDENCE: cshell+0x166310 (record +0xb0 >= 1 -> the colour table's entry through cshell+0x1dcda0 onto the clan
// text +0x11c, else the row's +0x138), DISASM cshell+0x16656b..0x16659c
pub fn member_clan_colour(item: Option<u32>) -> u32 {
    item.unwrap_or(CLAN_NAME_COLOUR)
}
pub const READY_BUTTON: i32 = 7;
pub const TEAM_A_ROWS: i32 = 19;
pub const TEAM_B_ROWS: i32 = 20;
pub const SETTINGS_WINDOW: i32 = 6;
pub const MAP_PICTURE: i32 = 0;
pub const MAP_LABEL: i32 = 10;
pub const MODE_NAME: i32 = 14;
pub const GOAL_LABEL: i32 = 15;
pub const DIFFICULTY_LABEL: i32 = 16;
pub const GOAL: i32 = 17;
pub const PLAYERS: i32 = 19;
pub const ARMS: i32 = 21;
/// The map description text (its record carries a placeholder the room replaces).
pub const MAP_DESCRIPTION: i32 = 11;
/// Option check buttons: (on, off) pairs for mid-match join, friendly fire, kill cam.
pub const OPTION_CHECKS: [(i32, i32); 3] = [(31, 32), (33, 34), (35, 36)];
/// Commands.
pub const CMD_START: i32 = 13;
pub const CMD_READY: i32 = 107;
pub const CMD_EXIT: i32 = 59;
pub const CMD_TEAM: i32 = 110;
pub const CMD_MAP: i32 = 241;
pub const CMD_MODE: i32 = 243;
pub const CMD_GOAL: i32 = 245;
pub const CMD_PLAYERS: i32 = 247;
/// The weapon limit list.
pub const CMD_ARMS: i32 = 600;
/// CLI-1899: the Invite button (a room invite to every player of the channel lobby, 0x396).
// EVIDENCE: cshell+0x164810 (command 0x7531 -> cshell+0x15fd30; no host test)
pub const CMD_INVITE: i32 = 30001;
/// CLI-1899: the invite's boxes: sent (on a clan server its own string), the room full, a clan room whose two sides
/// are both taken, and the wait between two invites (`%1` the seconds left).
// EVIDENCE: cshell+0x39060 (0x397 ok -> 0x509, 0x50d with shell +0x12c set), cshell+0x15fd30 (0x50b, 0x50e),
// cshell+0x38f20 (0x50a through FormatMessage with 10 minus the seconds counted)
pub const INVITE_SENT: i32 = 0x509;
pub const INVITE_SENT_CLAN: i32 = 0x50d;
pub const INVITE_ROOM_FULL: i32 = 0x50b;
pub const INVITE_CLAN_SIDES_TAKEN: i32 = 0x50e;
pub const INVITE_WAIT: i32 = 0x50a;
/// CLI-1899: seconds between two invites.
// EVIDENCE: cshell+0x38f20 (timer 0x2712 of 1000 ms armed at the send), cshell+0x42df0 (case 0x2712: the tenth tick
// allows the next invite and stops the timer)
pub const INVITE_GAP_SECS: u64 = 10;

/// CLI-1899: what the Invite button does: `Ok(())` sends 0x396, `Err(box)` shows that string instead.
/// `clan` = the joined server entry is clan-only (2009's shell +0x12c, set from the entry's +0xbc at the server
/// choice), `teams` = the members on team 1 and on team 2, `others` = members on neither, `sent` = when the last
/// invite went out (cleared by a failed reply).
// EVIDENCE: cshell+0x15fd30 (no clan flag: both team lists together below the room's player limit +0x5590, else
// 0x50b; clan flag: either team list empty, else 0x50e), cshell+0x38f20 (the wait flag clear -> 0x50a, else the send),
// cshell+0x13dff0 / cshell+0x13e200 (shell +0x12c = the chosen server entry's +0xbc != 0)
pub fn invite_check(
    clan: bool,
    teams: [usize; 2],
    others: usize,
    max_players: i32,
    sent: Option<Instant>,
    now: Instant,
) -> Result<(), InviteRefusal> {
    if clan {
        if teams[0] != 0 && teams[1] != 0 {
            return Err(InviteRefusal::Box(INVITE_CLAN_SIDES_TAKEN));
        }
    } else if (teams[0] + teams[1] + others) as i64 >= i64::from(max_players) {
        return Err(InviteRefusal::Box(INVITE_ROOM_FULL));
    }
    if let Some(at) = sent {
        let counted = now.saturating_duration_since(at).as_secs();
        if counted < INVITE_GAP_SECS {
            return Err(InviteRefusal::Wait((INVITE_GAP_SECS - counted) as i32));
        }
    }
    Ok(())
}

/// CLI-1899: why the Invite button sent nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InviteRefusal {
    /// A plain box of this string.
    Box(i32),
    /// The wait box ([`INVITE_WAIT`]) with these seconds left.
    Wait(i32),
}
/// The controls the setting lists open under: map, mode, goal, players, weapon limit.
// EVIDENCE: cshell+0x1625f0 (6, 1), cshell+0x1628e0 (6, 2), cshell+0x163550 (6, 3), cshell+0x1637a0 (6, 4),
// cshell+0x160080 (6, 5)
pub const LIST_ANCHORS: [(&str, i32); 5] = [
    ("map", 1),
    ("mode", 2),
    ("goal", 3),
    ("players", 4),
    ("arms", 5),
];
/// Option commands (on, off) and their 0x38c field types.
pub const OPTION_COMMANDS: [(i32, i32, u32); 3] = [(234, 235, 9), (236, 237, 0xa), (601, 602, 0xd)];
/// The mode row flag of each option pair (mid-match join, friendly fire, kill cam).
pub const OPTION_KINDS: [RoomOption; 3] = [
    RoomOption::MidJoin,
    RoomOption::FriendlyFire,
    RoomOption::KillCam,
];
/// The ON / OFF texts beside each option's check buttons (they carry the option commands too).
pub const OPTION_TEXTS: [(i32, i32); 3] = [(23, 24), (26, 27), (29, 30)];
/// The weapon limit's list button.
pub const ARMS_BUTTON: i32 = 10;

/// CLI-1897: the host may change room option `opt` in the room's mode (the mode row's flag of the joined server's
/// mode table: the clan rows on a clan server).
// EVIDENCE: cshell+0x1677f0 (each option's controls enabled = the row's flag and the host flag +0x56b0)
pub fn option_open(cat: &Catalog, data: &MenuData, opt: RoomOption) -> bool {
    data.host() && cat.mode_option_open(data.room.mode, opt)
}
/// 0x38c field types of the settings.
pub const SET_MAP: u32 = 3;
pub const SET_MODE: u32 = 4;
pub const SET_MAX_PLAYERS: u32 = 6;
pub const SET_GOAL: u32 = 8;
/// LG-274 / CLI-1626: the Hacks row of a modded room (our own controls, built at run time; no 2009 counterpart).
/// Its bar, dropdown button, label and value sit one settings row (28 px, the ROM rows' pitch) below the weapon
/// limit; the settings window's art is drawn in three pieces so its Game Settings section grows by that row and its
/// Map Selection section shrinks by it (a band under the map description is left out), and the map picture and the
/// description move down with it, the description cut to [`HACKS_DESCRIPTION_LINES`].
// DC-DEFAULT: CLI-1626 (proposed, project decision "steal some vertical space from the map selection")
pub mod hacks_row {
    /// Control ids in the settings window (the ROM records end at 36).
    pub const BAR: i32 = 40;
    pub const BUTTON: i32 = 41;
    pub const LABEL: i32 = 42;
    pub const VALUE: i32 = 43;
    /// The two lower pieces of the window art.
    pub const ART_MIDDLE: i32 = 44;
    pub const ART_LOWER: i32 = 45;
    /// The row's command (our own number).
    pub const CMD: i32 = 0x3f04;
    /// The row pitch (the ROM setting rows are 28 px apart: Mode 114, Goal 142, Players 170, Arms 198).
    pub const PITCH: i32 = 28;
    /// Window-relative art rows: the top piece ends here (just under the weapon-limit bar); the band above it is
    /// drawn once more under it.
    pub const SPLIT: i32 = 144;
    /// Window-relative art row where the left-out band starts (under the map description, above the map name
    /// bar); it is [`PITCH`] rows high.
    pub const CUT: i32 = 430;
}
/// The map description's lines under a Hacks row (six fit without it).
pub const HACKS_DESCRIPTION_LINES: i32 = 4;

/// The row label and the choices' names (DeCombat strings, not ROM text).
pub const HACKS_LABEL_TEXT: &str = "Hacks";
/// CLI-1641: the Arms list's value of the Possession row (no ROM limit has it).
pub const POSSESSION_ROW: i32 = -101;

/// CLI-1640: the row's label on a Hardcore entry.
pub const HARDCORE_LABEL_TEXT: &str = "Hardcore";

/// CLI-1640: the name of a Hardcore setting.
pub fn hardcore_label(on: bool) -> &'static str {
    if on {
        "On"
    } else {
        "Off"
    }
}

/// The name of a Hacks setting (`dc_wire::roomhacks` bits).
pub fn hacks_label(value: u32) -> &'static str {
    use dc_wire::roomhacks::{NO_RECOIL, WALLHACKS};
    match value {
        v if v == WALLHACKS => "Wallhacks",
        v if v == WALLHACKS | NO_RECOIL => "Wallhacks + no-recoil",
        v if v == NO_RECOIL => "No-recoil only",
        _ => "Off",
    }
}

/// 0x38c field type of the weapon limit.
// EVIDENCE: cshell+0x1614f0 (the weapon list's pick: 0x38c type 0xe with the row's index)
pub const SET_WEAPONS: u32 = 0xe;
/// 0x393 kind and reasons with their own box.
pub const FLAG_KIND_START: i16 = 3;
pub const REASON_FINAL_ROUND: u16 = 10;
pub const REASON_NEED_PLAYERS: u16 = 11;
pub const REASON_JOIN_FAILED: u16 = 0x8e;
pub use super::frontfx::COUNTDOWN_FROM;
/// The chat line of a host change during the start countdown (red).
// DISASM: cshell+0x168680 (string 0x577)
pub const HOST_CHANGED_WHILE_STARTING: i32 = 0x577;

/// Member row look.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberStyle {
    pub bars: [Option<String>; 2],
    pub bar: [i32; 2],
    /// The bar's place in its row (the 2009 layout: five pixels above the row, where the list's slot art starts).
    pub bar_at: [i32; 2],
    pub level: Option<String>,
    pub level_size: [i32; 2],
    pub level_at: [i32; 2],
    pub master: Option<String>,
    pub master_size: [i32; 2],
    pub master_at: [i32; 2],
    pub name_at: [i32; 2],
    pub state_at: [i32; 2],
    /// CLI-1895: the clan name and clan mark places (none without the layout key: not drawn).
    pub clan_name_at: Option<[i32; 2]>,
    pub clan_mark_at: Option<[i32; 2]>,
}

impl MemberStyle {
    // DC-DEFAULT: CLI-173 (proposed) member row fallbacks: 338 x 28 rows, bar at (0, -5), name at (44, 9), state at
    // (299, 9)
    pub fn load(cat: &Catalog) -> MemberStyle {
        use row_keys as k;
        let s = |key: &KeyPin| cat.style(UiScreen::Screen9, key);
        let p = |key: &KeyPin, d: [i32; 2]| pair(s(key)).unwrap_or(d);
        let i = |key: &KeyPin, d: i32| int(s(key)).unwrap_or(d);
        MemberStyle {
            bars: [
                s(&k::ALPHA_BAR).map(str::to_string),
                s(&k::BRAVO_BAR).map(str::to_string),
            ],
            bar: [i(&k::BAR_WIDTH, 338), i(&k::BAR_HEIGHT, 28)],
            bar_at: p(&k::BAR_AT, [0, -5]),
            level: s(&k::LEVEL).map(str::to_string),
            level_size: [i(&k::LEVEL_WIDTH, 30), i(&k::LEVEL_HEIGHT, 22)],
            level_at: p(&k::LEVEL_AT, [10, 0]),
            master: s(&k::MASTER).map(str::to_string),
            master_size: [i(&k::MASTER_WIDTH, 42), i(&k::MASTER_HEIGHT, 10)],
            master_at: p(&k::MASTER_AT, [0, -5]),
            name_at: p(&k::NAME_AT, [44, 9]),
            state_at: p(&k::STATE_AT, [299, 9]),
            clan_name_at: pair(s(&k::CLAN_NAME_AT)),
            clan_mark_at: pair(s(&k::CLAN_MARK_AT)),
        }
    }
}

/// Font record and size of the member row texts.
// EVIDENCE: cshell+0x1657e0 (font lookup cshell+0x1c07b0 with 0xe; name, clan and state controls created with that
// font and size 0xc)
// DISASM: cshell+0x16596f..0x165987 (the 0xe lookup kept at [esp + 0x10]), cshell+0x165b29 / 0x165c26 / 0x165cc7
// (that value pushed as the font of the three create calls)
// OBSERVED: lobbyfx4/real/R20_room.png (2009: the member name is smaller than the column titles)
pub const MEMBER_FONT: i32 = 14;
pub const MEMBER_TEXT_SIZE: i32 = 12;
/// Alignments of the member name (left, vertically centred) and its state (centred both ways).
// EVIDENCE: cshell+0x1657e0 (name control +0x118 flags 0x41, state control +0x120 flags 0x44; the state text set by
// cshell+0x163d00)
pub const MEMBER_NAME_ALIGN: Align = Align {
    first: ALIGN_LEFT,
    second: ALIGN_SECOND_CENTRE,
};
pub const MEMBER_STATE_ALIGN: Align = Align {
    first: ALIGN_CENTRE,
    second: ALIGN_SECOND_CENTRE,
};

/// What the 2009 member row asks of its name and state texts at row position `at` (team 1 or 2, row `row`): each
/// at the row plus its style position, [`MEMBER_NAME_ALIGN`] / [`MEMBER_STATE_ALIGN`], font [`MEMBER_FONT`], size
/// [`MEMBER_TEXT_SIZE`]. The draw trace's copy of the rule, apart from the drawing.
// EVIDENCE: cshell+0x1657e0 (controls and their flags), cshell+0x163d00 (the state text)
// DC-DEFAULT: CLI-539 (proposed) layout keys `dyn/member<team>_<row>/<style key sha>`; the name and state positions
// are the section's name / state keys as the drawing reads them (CLI-173)
pub fn member_row_wants(st: &MemberStyle, team: i32, row: usize, at: [i32; 2]) -> [DynWant; 2] {
    let want = |pin: &KeyPin, p: [i32; 2], align: Align| DynWant {
        key: format!("dyn/member{team}_{row}/{}", pin.sha16),
        text_at: [(at[0] + p[0]) as f32, (at[1] + p[1]) as f32],
        text_anchor: anchor_of(align),
        font: Some(MEMBER_FONT),
        font_size: Some(MEMBER_TEXT_SIZE),
    };
    [
        want(&row_keys::NAME_AT, st.name_at, MEMBER_NAME_ALIGN),
        want(&row_keys::STATE_AT, st.state_at, MEMBER_STATE_ALIGN),
    ]
}

/// Mode class of the free-for-all rows (GAME_MODES_2009.md section 2).
pub const FFA_CLASS: u8 = 6;
/// Mode class of the Quarantine rows: everybody joins as a human, the server picks the infected.
pub const QRT_CLASS: u8 = 11;
/// Mode class of the Spy Hunt rows: everybody joins as a hunter, the spies are made in the match.
pub const SPY_CLASS: u8 = 9;

/// The waiting room of this mode class lists its members in one list without team buttons.
// DC-DEFAULT: LG-10 (proposed; amended 2026-10-31) free for all; LG-95 (proposed, playtest report) Quarantine too: the
// lobby's team values mean nothing there (the server makes everyone human and picks the infected each round,
// DSV-210); Spy Hunt too (USER REQUEST 2026-10-31 "in the lobby spyhunt should not be teams but rather list
// players like one man army": the server makes everyone a hunter, DSV-236, and the lobby refuses team changes in
// class 9, CLI-782 / cshell+0x137720)
pub fn one_list_class(class: Option<u8>) -> bool {
    matches!(class, Some(FFA_CLASS) | Some(SPY_CLASS) | Some(QRT_CLASS))
}

/// Where each member of the waiting room sits: `(id, column, row)`, column 1 = the left list, 2 = the right list.
/// Team modes: the member's team picks the column (team 2 right, anything else left), rows in id order. Free for all
/// (`ffa`): one list in id order that fills the left column (`left_rows` rows) and goes on in the right one; the
/// lobby's team values are not shown, and the room's master (`master`, the host) is the first row.
// SPEC: GAME_MODES_2009.md §5.4 (FFA: no teams visible; the result and spectator screens use one ranking list)
// DC-DEFAULT: LG-10 (proposed) the FFA waiting room shows one member list over both columns and no team button
// DC-DEFAULT: CLI-932 (user, round 10) in the FFA list the master's row is the first one, the others keep their
// order (no 2009 rule found: the room's member list is the slot list of cshell+0x163d00)
pub fn member_slots(
    members: impl Iterator<Item = (u32, i32)>,
    ffa: bool,
    left_rows: usize,
    master: u32,
) -> Vec<(u32, i32, usize)> {
    let mut count = [0usize; 2];
    let mut members: Vec<(u32, i32)> = members.collect();
    if ffa {
        // a stable sort on "not the master": the master moves to the front, the rest keep their order
        members.sort_by_key(|(id, _)| *id != master);
    }
    members
        .into_iter()
        .enumerate()
        .map(|(i, (id, team))| {
            if ffa {
                if i < left_rows {
                    (id, 1, i)
                } else {
                    (id, 2, i - left_rows)
                }
            } else {
                let col = if team == 2 { 2 } else { 1 };
                let row = count[(col - 1) as usize];
                count[(col - 1) as usize] += 1;
                (id, col, row)
            }
        })
        .collect()
}

/// The state string of a member: ready / starting / in game, else (state 0) shopping / away by the sub-state.
// EVIDENCE: cshell+0x163d00 (state 1 0x368, 2 0x445, 3 0x161c; state 0 with sub-state 1 0x456, 2 0x457)
pub fn member_state_text(state: i32, sub: i32) -> Option<i32> {
    match (state, sub) {
        (1, _) => Some(STATE_READY),
        (2, _) => Some(STATE_STARTING),
        (3, _) => Some(STATE_IN_GAME),
        (0, 1) => Some(SUB_SHOPPING),
        (0, 2) => Some(SUB_AWAY),
        _ => None,
    }
}

/// The room's bar commands that mark the member shopping when they leave the room for their screen: shop (0x3d),
/// 0x8e, clan (0xa8), modification (0xb8). The black market and inbox commands (622 / 619) are not among them.
// EVIDENCE: cshell+0x164810 (case 0x3d / 0x8e / 0xa8 / 0xb8: cshell+0x15d610 when the own ready flag shell +0x1c is
// set, then 0x392 (1, 1) at cshell+0x1648cb; any other command clears the room's +0x587c)
pub const SHOPPING_COMMANDS: [i32; 4] = [0x3d, 0x8e, 0xa8, 0xb8];

/// A bar command of the room: the shop-family screen it opens, and for the commands of [`SHOPPING_COMMANDS`] the
/// 2009 shopping flag first: a ready member is made not ready (0x392 (0, 0)), then 0x392 (1, 1). The other members
/// then show the member's state text "shopping" (sub-state 1, `res 0x456`) until he is back in the room.
// EVIDENCE: cshell+0x15d610 (own ready flag toggled off, 0x392 (0, flag) at cshell+0x15d66d), cshell+0x164810
// DC-DEFAULT: CLI-1560 (proposed) the flag goes out only when the command opens a screen here (DeCombat's room bar
// opens shop, black market, inbox and modification; 0x8e and the clan command open nothing from the room yet)
fn leave_for_bar(command: i32, ctx: &mut Ctx) {
    let ready = Room::own_state(ctx.data) == 1 && !ctx.data.host();
    if !super::shop::bar_command(command, Go::Room, ctx) {
        return;
    }
    if SHOPPING_COMMANDS.contains(&command) {
        if ready {
            ctx.send(LinkCmd::Ready(false));
        }
        ctx.send(LinkCmd::Shopping(true));
    }
}

fn add(a: [i32; 2], b: [i32; 2]) -> [i32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

pub struct Room {
    sheet: Sheet,
    style: MemberStyle,
    levels: Option<LevelTable>,
    /// The idle rules run, the last input, the away flag sent, the idle leave sent.
    idle_rules: bool,
    last_input: Instant,
    away: bool,
    idle_left: bool,
    /// The open setting list (one at a time).
    drop: Option<Dropdown>,
    /// gm-client: the open member menu and its entry under the pointer, the `/k` prompt flag, the last kick's
    /// target name (the 0x38f confirmation names it).
    member_menu: Option<super::gm::MemberMenu>,
    member_hover: Option<i32>,
    kick_prompt: bool,
    kick_target: String,
    /// CLI-1310: the start countdown holds the player in the room ([`start_lock_after`]).
    start_lock: bool,
    /// CLI-1314: when the last countdown tick of a starting member arrived; armed until the room plays, the lock
    /// is dropped or the watchdog fires ([`START_WATCHDOG`]).
    start_watch: Option<Instant>,
    /// CLI-1626: the settings window's art and the record places the Hacks row moves (`None` without the window).
    hacks_layout: Option<HacksLayout>,
}

/// CLI-1626: what the Hacks row changes, kept to put the vanilla layout back.
struct HacksLayout {
    /// The settings window's art.
    art: String,
    art_width: i32,
    /// The record places of the map picture and the map description.
    picture: [i32; 2],
    description: [i32; 2],
}

/// The waiting room's start lock after lobby event `e` (`locked` before it; `own_state` = the own member state
/// after it, 0x3042 values). A countdown tick (0x3044) locks a player whose own state is 2 (starting: the host
/// and every player who was ready at the start); the room's state push (0x3035 type 7) to in game (2) unlocks.
/// A player who was not ready at the start never gets state 2 and stays unlocked (he stays in the room).
// DISASM: cshell+0x15f2b0 (0x3044: own record +0x20 == 2 -> room byte +0x56f4 := 1), cshell+0x1673a0 (0x3042:
// own +0x20 := state when the id is the own one), cshell+0x168680 / cshell+0x134830 (0x3035 type 7: value 2 ->
// +0x56f4 := 0); cshell+0x1608b0 (+0x56f4 := 0 also on the start-failure path, not reproduced: CLI-1311)
// DC-DEFAULT: CLI-1311 (proposed) a room state push other than 1 (starting) also unlocks, so a start the lobby
// takes back (state 0) never leaves the room locked (the 2009 client clears the byte only on 2 or its 15 s
// start watchdog)
pub fn start_lock_after(locked: bool, e: &LinkEvent, own_state: i32) -> bool {
    match e {
        LinkEvent::Countdown(_) if own_state == 2 => true,
        LinkEvent::RoomSetting { kind: 7, value, .. } if *value != 1 => false,
        _ => locked,
    }
}

/// CLI-1314: the start watchdog. A starting member's last countdown tick arms it; when no match address (0x3043) has
/// come [`START_WATCHDOG`] later the client shows box 0x473, sends 0x394 {1} and unlocks the room.
// DISASM: cshell+0x15f2b0 (each 0x3044 tick with own state 2: now -> +0x58bc, +0x58c0 := 1), cshell+0x1608b0 (room
// update: +0x58c0 set and GetTickCount - +0x58bc > 15000 -> box 0x473, CharInfo +0x198 / +0x199 := 1, +0x58c0 := 0;
// the next update sends 0x394 {+0x199} through cshell+0x18b010 and clears +0x198 / +0x199 / +0x56f4)
// DC-DEFAULT: CLI-1314 (proposed) the watchdog is also disarmed by a room state push other than 1 / 2 (the lobby took
// the start back); the 2009 room clears it only by firing or by leaving the screen with the match address
pub const START_WATCHDOG: Duration = Duration::from_secs(15);
/// The screen's own timer command for the watchdog.
pub const CMD_START_WATCHDOG: i32 = -0x394;

/// Room commands that still run while the start lock holds: the chat line and the idle timers.
// DISASM: cshell+0x160c90 (the room's key handler has no lock test: chat typing and Enter go on), cshell+0x1608b0
// (the idle timers run in the room update, unlocked)
pub fn runs_while_locked(command: i32) -> bool {
    matches!(command, CMD_CHAT_SEND | CMD_IDLE_AWAY | CMD_IDLE_LEAVE | CMD_START_WATCHDOG)
}

impl Room {
    pub fn new(cat: &Catalog, data: &MenuData) -> Room {
        let mut sheet = build_sheet(cat, &[pages::LOBBY_COMMON, pages::CHAT, pages::WAITING]);
        sheet.show(TEAM_WINDOW, FORCE_OUT_BUTTON, WidgetKind::Button, false);
        sheet.show(SETTINGS_WINDOW, DIFFICULTY_LABEL, WidgetKind::Text, false);
        // the page's controls on the screen (character info button and its texts) are not used here
        for p in sheet
            .parts
            .iter_mut()
            .filter(|p| p.parent.is_none() && p.kind != WidgetKind::Window)
        {
            // and the chat page's clan tab (CLI-171)
            if p.page == pages::WAITING || (p.page == pages::CHAT && p.kind == WidgetKind::Button) {
                p.shown = false;
            }
        }
        // w2-lobbychat: the chat window's wide panel and the chat box (cshell+0x1bb70, screen 9)
        textbox::chat_panel(&mut sheet, cat, ChatScreen::Room);
        textbox::add_chat_edit(&mut sheet, ChatScreen::Room);
        let hacks_layout = add_hacks_row(&mut sheet);
        let mut r = Room {
            sheet,
            style: MemberStyle::load(cat),
            levels: cat.rom.as_deref().and_then(LevelTable::from_rom),
            idle_rules: data.idle_rules,
            last_input: Instant::now(),
            away: false,
            idle_left: false,
            drop: None,
            member_menu: None,
            member_hover: None,
            kick_prompt: false,
            kick_target: String::new(),
            start_lock: false,
            start_watch: None,
            hacks_layout,
        };
        r.sync(cat, data);
        r
    }

    /// CLI-1626: shows the Hacks row (and the layout around it) when the lobby offers a choice, the vanilla
    /// layout otherwise.
    fn sync_hacks(&mut self, data: &MenuData) {
        use super::sheet::Look;
        use hacks_row::*;
        let Some(l) = &self.hacks_layout else {
            return;
        };
        // CLI-1640: the same row serves Hardcore on an entry that offers it (a set with Hacks keeps its Hacks row)
        let hardcore_row = data.room.hacks_offered == 0 && data.room.hardcore_offered;
        let on = data.room.hacks_offered != 0 || hardcore_row;
        let d = if on { PITCH } else { 0 };
        let s = &mut self.sheet;
        if let Some(w) = s.window_index(SETTINGS_WINDOW) {
            let tex = if on {
                crate::hud::compose::part_of(&l.art, [0, 0, l.art_width as u32, SPLIT as u32])
            } else {
                l.art.clone()
            };
            s.parts[w].look = Look::Panel {
                tex,
                stretch: false,
            };
        }
        for (id, kind) in [
            (BAR, WidgetKind::Image),
            (BUTTON, WidgetKind::Button),
            (LABEL, WidgetKind::Text),
            (VALUE, WidgetKind::Text),
            (ART_MIDDLE, WidgetKind::Image),
            (ART_LOWER, WidgetKind::Image),
        ] {
            s.show(SETTINGS_WINDOW, id, kind, on);
        }
        if hardcore_row {
            s.set_text(SETTINGS_WINDOW, LABEL, HARDCORE_LABEL_TEXT);
            s.set_text(SETTINGS_WINDOW, VALUE, hardcore_label(data.room.hardcore));
        } else {
            s.set_text(SETTINGS_WINDOW, LABEL, HACKS_LABEL_TEXT);
            s.set_text(SETTINGS_WINDOW, VALUE, hacks_label(data.room.hacks));
        }
        if let Some(p) = s.part_mut(SETTINGS_WINDOW, MAP_PICTURE, WidgetKind::Image) {
            p.pos = [l.picture[0], l.picture[1] + d];
            p.placed = on;
        }
        if let Some(p) = s.part_mut(SETTINGS_WINDOW, MAP_DESCRIPTION, WidgetKind::Text) {
            p.pos = [l.description[0], l.description[1] + d];
            p.placed = on;
        }
        s.set_wrap_max(
            SETTINGS_WINDOW,
            MAP_DESCRIPTION,
            if on { HACKS_DESCRIPTION_LINES } else { 0 },
        );
    }

    /// The idle rule's command at `now`: the away flag after [`IDLE_AWAY`] without input, the room left
    /// [`IDLE_LEAVE`] after the idle clock restarted with the away flag ([`Room::mark_away`]) (each once).
    pub fn idle_due(&self, now: Instant) -> Option<i32> {
        if !self.idle_rules || self.idle_left {
            return None;
        }
        let idle = now.saturating_duration_since(self.last_input);
        if idle > IDLE_LEAVE {
            Some(CMD_IDLE_LEAVE)
        } else if idle > IDLE_AWAY && !self.away {
            Some(CMD_IDLE_AWAY)
        } else {
            None
        }
    }

    /// When the last input came (tests move it back).
    /// The away flag went out at `at`: like the 2009 room update the idle clock restarts with it, so the leave
    /// follows [`IDLE_LEAVE`] later.
    pub fn mark_away(&mut self, at: Instant) {
        self.away = true;
        self.last_input = at;
    }

    pub fn set_last_input(&mut self, at: Instant) {
        self.last_input = at;
    }

    /// CLI-1314: the start watchdog has run out at `now`.
    pub fn start_watch_due(&self, now: Instant) -> bool {
        self.start_watch
            .is_some_and(|t| now.saturating_duration_since(t) > START_WATCHDOG)
    }

    /// CLI-1314 (tests): the watchdog's clock moved back by `by`.
    pub fn age_start_watch(&mut self, by: Duration) {
        if let Some(t) = self.start_watch.as_mut() {
            *t = t.checked_sub(by).unwrap_or(*t);
        }
    }

    /// CLI-1310: the start countdown holds the player in the room.
    pub fn start_locked(&self) -> bool {
        self.start_lock
    }

    fn own_team(data: &MenuData) -> i32 {
        data.members.get(&data.profile.my_id).map_or(1, |m| m.team)
    }

    /// The room's mode row shows one member list and no team buttons (free for all, Quarantine).
    fn is_ffa(cat: &Catalog, data: &MenuData) -> bool {
        one_list_class(cat.mode_class(data.room.mode))
    }

    fn own_state(data: &MenuData) -> i32 {
        data.members.get(&data.profile.my_id).map_or(0, |m| m.state)
    }

    /// Texts, buttons and checks from the room's data.
    fn sync(&mut self, cat: &Catalog, data: &MenuData) {
        let host = data.host();
        let room = &data.room;
        self.sheet.set_text(
            TEAM_WINDOW,
            ROOM_NUMBER,
            &roomtext::room_number_label(room.number),
        );
        self.sheet.set_text(TEAM_WINDOW, ROOM_CAPTION, &room.title);
        self.sheet
            .show(TEAM_WINDOW, START_PART, WidgetKind::Button, host);
        self.sheet
            .show(TEAM_WINDOW, READY_BUTTON, WidgetKind::Button, !host);
        self.sheet.show(
            TEAM_WINDOW,
            CHANGE_PASSWORD_BUTTON,
            WidgetKind::Button,
            host,
        );
        let team = Room::own_team(data);
        // EVIDENCE: cshell+0x15d9d0 (team 1 shows the button towards Bravo, team 2 the one towards Alpha; the own
        // team's label on)
        // DC-DEFAULT: LG-10 (proposed) no team button in a free-for-all room
        let ffa = Room::is_ffa(cat, data);
        self.sheet.show(
            TEAM_WINDOW,
            TEAM_ALPHA_BUTTON,
            WidgetKind::Button,
            !ffa && team != 1,
        );
        self.sheet.show(
            TEAM_WINDOW,
            TEAM_BRAVO_BUTTON,
            WidgetKind::Button,
            !ffa && team == 1,
        );
        for (label, t) in [(TEAM_ALPHA_LABEL, 1), (TEAM_BRAVO_LABEL, 2)] {
            if let Some(p) = self
                .sheet
                .part_mut(TEAM_WINDOW, label, WidgetKind::CheckButton)
            {
                p.checked = !ffa && team == t;
            }
        }
        self.sheet
            .set_text(SETTINGS_WINDOW, MAP_LABEL, &cat.map_label(room.map));
        // the waiting room's values: mode full name, goal and players by the mode row, weapon limit, map text
        // EVIDENCE: cshell+0x167e90 ((6, 14) full name, (6, 17) cshell+0x2623f0 goal, (6, 19) cshell+0x262510 players
        // of the maximum, (6, 21) weapon limit string, (6, 11) cshell+0x263a80 description wrapped at 0xeb)
        // OBSERVED: lobbyfx4/real/R20_room.png (2009: goal string with the goal, players as two halves,
        // the weapon rule string, the map text on six lines)
        self.sheet
            .set_text(
                SETTINGS_WINDOW,
                MODE_NAME,
                // LG-305: a Gun Game room's name is ours; goal and maps are the free-for-all row's
                &if room.gun_game {
                    dc_wire::gungame::MODE_NAME.to_string()
                } else {
                    cat.mode_full_name(room.mode)
                },
            );
        self.sheet.set_text(
            SETTINGS_WINDOW,
            GOAL,
            &roomtext::goal_label(cat, room.mode, room.goal),
        );
        self.sheet.set_text(
            SETTINGS_WINDOW,
            PLAYERS,
            &roomtext::player_count_label(cat, room.mode, room.max_players),
        );
        self.sheet.set_text(
            SETTINGS_WINDOW,
            ARMS,
            // LG-307: the Possession row is ours
            &if room.possession {
                dc_wire::possession::ARMS_NAME.to_string()
            } else {
                roomtext::weapon_rule_label(cat, room.weapons)
            },
        );
        self.sheet.set_text(
            SETTINGS_WINDOW,
            MAP_DESCRIPTION,
            &cat.map_description(room.map, room.mode),
        );
        self.sheet.set_wrap(
            SETTINGS_WINDOW,
            MAP_DESCRIPTION,
            roomtext::DESCRIPTION_WRAP_MEASURE,
        );
        self.sheet.set_wrap_pitch(
            SETTINGS_WINDOW,
            MAP_DESCRIPTION,
            roomtext::DESCRIPTION_PITCH,
        );
        if let Some(img) = cat.map_picture(room.map) {
            if let Some(p) = self
                .sheet
                .part_mut(SETTINGS_WINDOW, MAP_PICTURE, WidgetKind::Image)
            {
                p.look = super::sheet::Look::Panel {
                    tex: img,
                    stretch: false,
                };
            }
        }
        self.sync_hacks(data);
        self.sync_option_locks(cat, data);
        for (k, (on, off)) in OPTION_CHECKS.iter().enumerate() {
            let set = room.options[k] != 0;
            if let Some(p) = self
                .sheet
                .part_mut(SETTINGS_WINDOW, *on, WidgetKind::CheckButton)
            {
                p.checked = set;
            }
            if let Some(p) = self
                .sheet
                .part_mut(SETTINGS_WINDOW, *off, WidgetKind::CheckButton)
            {
                p.checked = !set;
            }
        }
    }

    /// CLI-1897: the option controls are enabled for the host where the room's mode row lets the option change:
    /// the check buttons and their ON / OFF texts per option, the weapon limit's list button.
    // EVIDENCE: cshell+0x1677f0 (the mode row's weapon flag (cshell+0x264ad0) and the host flag enable (6, 10); the
    // mid-match join flag (cshell+0x264c30) (6, 31), (6, 32), (6, 23), (6, 24); friendly fire (cshell+0x264d90)
    // (6, 33), (6, 34), (6, 26), (6, 27); kill cam (cshell+0x264ef0) (6, 35), (6, 36), (6, 29), (6, 30); vtable
    // +0x98 = the enabled field)
    fn sync_option_locks(&mut self, cat: &Catalog, data: &MenuData) {
        for (k, opt) in OPTION_KINDS.iter().enumerate() {
            let open = option_open(cat, data, *opt);
            let (on, off) = OPTION_CHECKS[k];
            let (on_text, off_text) = OPTION_TEXTS[k];
            for (id, kind) in [
                (on, WidgetKind::CheckButton),
                (off, WidgetKind::CheckButton),
                (on_text, WidgetKind::Text),
                (off_text, WidgetKind::Text),
            ] {
                if let Some(p) = self.sheet.part_mut(SETTINGS_WINDOW, id, kind) {
                    p.enabled = open;
                }
            }
        }
        let arms = option_open(cat, data, RoomOption::Weapons);
        if let Some(p) = self
            .sheet
            .part_mut(SETTINGS_WINDOW, ARMS_BUTTON, WidgetKind::Button)
        {
            p.enabled = arms;
        }
    }

    /// The rows of the host's setting list `list` (text, value) from the ROM room table's mode row.
    // EVIDENCE: cshell+0x1625f0 (maps of the room's mode, map names), cshell+0x1628e0 (modes, full names),
    // cshell+0x163550 (goals from the row's smallest to its largest by its step, goal strings), cshell+0x1637a0
    // (player counts the same way, player strings), cshell+0x160080 (the weapon limit strings in their order)
    // DC-DEFAULT: CLI-506 (proposed) the weapon list ends at its last string (the 2009 loop runs one row past the
    // list, a row whose string id is -1)
    pub fn list_rows(cat: &Catalog, data: &MenuData, list: &str) -> Vec<(String, i32)> {
        let room = &data.room;
        match list {
            "map" => {
                let mode_bit = 1u32.checked_shl(room.mode.max(0) as u32).unwrap_or(0);
                data.masks
                    .iter()
                    .filter(|(_, m)| m & mode_bit != 0)
                    .map(|(map, _)| (cat.map_label(*map as i32), *map as i32))
                    .collect()
            }
            "mode" => {
                let mut rows: Vec<(String, i32)> =
                    super::create::offered_modes(&data.masks, cat.mode_count())
                        .into_iter()
                        .map(|m| (cat.mode_full_name(m), m))
                        .collect();
                // DC-DEFAULT: CLI-1638 (proposed) the same row as the creation box's
                if data.gun_offered && super::create::free_for_all_row(cat, data).is_some() {
                    use super::create::{row_name, GUN_GAME_ROW};
                    rows.push((row_name(cat, GUN_GAME_ROW), GUN_GAME_ROW));
                }
                rows
            }
            "goal" => {
                let Some((lo, hi, step)) = goal_range(cat, room.mode) else {
                    return Vec::new();
                };
                (lo..=hi)
                    .step_by(step.max(1) as usize)
                    .map(|g| (roomtext::goal_label(cat, room.mode, g), g))
                    .collect()
            }
            "players" => {
                let (min, max, step) = cat.mode_players(room.mode).unwrap_or((2, 16, 2));
                (min..=max)
                    .step_by(step.max(1) as usize)
                    .map(|n| (roomtext::player_count_label(cat, room.mode, n), n))
                    .collect()
            }
            "arms" => {
                let mut rows: Vec<(String, i32)> = (0..roomtext::ARMS_TEXTS.len() as i32)
                    .map(|i| (roomtext::weapon_rule_label(cat, i), i))
                    .collect();
                // DC-DEFAULT: CLI-1641 (proposed) the Possession row: a featured entry's, not in Fireteam, Spy Hunt,
                // Quarantine or a Gun Game room
                if room.possession_offered
                    && !room.gun_game
                    && cat
                        .mode_class(room.mode)
                        .is_some_and(|c| dc_wire::possession::class_allows(u32::from(c)))
                {
                    rows.push((dc_wire::possession::ARMS_NAME.to_string(), POSSESSION_ROW));
                }
                rows
            }
            // CLI-1626: the choices the lobby's set offers (LG-274)
            "hardcore" => [false, true]
                .into_iter()
                .map(|v| (hardcore_label(v).to_string(), v as i32))
                .collect(),
            "hacks" => dc_wire::roomhacks::choices(room.hacks_offered)
                .into_iter()
                .map(|v| (hacks_label(v).to_string(), v as i32))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Opens the host's setting list `list` under its control.
    fn open_list(&mut self, cat: &Catalog, data: &MenuData, list: &'static str) {
        let Some(id) = LIST_ANCHORS
            .iter()
            .find(|(l, _)| *l == list)
            .map(|(_, id)| *id)
            .or(matches!(list, "hacks" | "hardcore").then_some(hacks_row::BAR))
        else {
            return;
        };
        let Some(anchor) = self.sheet.anchor_rect(SETTINGS_WINDOW, id) else {
            return;
        };
        let rows = Room::list_rows(cat, data, list);
        // EVIDENCE: cshell+0x1625f0 (+0x808 = 1: the map list scrolls), the others set 0
        self.drop = Some(Dropdown::new(
            list,
            anchor,
            rows,
            dropdown::ROOM_ROW_SIZE,
            list == "map",
        ));
    }
}

/// CLI-1626: adds the Hacks row's controls and the two lower pieces of the settings window's art to the waiting
/// room's sheet, hidden (`Room::sync_hacks` shows them). Copies of the weapon-limit row's records one row lower;
/// the art pieces right after the window, so they draw under its record controls. `None` without the window (no
/// ROM layout).
fn add_hacks_row(sheet: &mut Sheet) -> Option<HacksLayout> {
    use super::sheet::{Look, RUNTIME_REC};
    use hacks_row::*;
    let w = sheet.window_index(SETTINGS_WINDOW)?;
    let window = sheet.parts[w].clone();
    let Look::Panel { tex: art, .. } = &window.look else {
        return None;
    };
    let art = art.clone();
    let row = |id: i32, kind: WidgetKind| sheet.part(SETTINGS_WINDOW, id, kind).cloned();
    let (bar, button, label, value) = (
        row(5, WidgetKind::Image)?,
        row(10, WidgetKind::Button)?,
        row(20, WidgetKind::Text)?,
        row(ARMS, WidgetKind::Text)?,
    );
    let picture = row(MAP_PICTURE, WidgetKind::Image)?.pos;
    let description = row(MAP_DESCRIPTION, WidgetKind::Text)?.pos;
    let size = window.size;
    let lower = |p: &super::sheet::Part, id: i32| {
        let mut q = p.clone();
        q.id = id;
        q.rec = RUNTIME_REC;
        q.pos[1] += PITCH;
        q.shown = false;
        q.placed = true;
        q.parent = Some(w);
        q
    };
    let piece = |id: i32, from: i32, to: i32, at: i32| {
        let mut q = window.clone();
        q.kind = WidgetKind::Image;
        q.layer = SETTINGS_WINDOW;
        q.id = id;
        q.command = 0;
        q.rec = RUNTIME_REC;
        q.parent = Some(w);
        q.shown = false;
        q.placed = true;
        q.pos = [window.pos[0], window.pos[1] + at];
        q.size = [size[0], to - from];
        q.look = Look::Panel {
            tex: crate::hud::compose::part_of(
                &art,
                [0, from as u32, size[0] as u32, (to - from) as u32],
            ),
            stretch: false,
        };
        q
    };
    let mut button = lower(&button, BUTTON);
    button.command = CMD;
    let mut label = lower(&label, LABEL);
    label.text = HACKS_LABEL_TEXT.to_string();
    // the Arms value's record, one row lower: centred about the same x like every settings field
    let value = lower(&value, VALUE);
    let parts = vec![
        // window rows SPLIT - PITCH .. CUT under the top piece, then CUT + PITCH .. the end at their own place
        piece(ART_MIDDLE, SPLIT - PITCH, CUT, SPLIT),
        piece(ART_LOWER, CUT + PITCH, size[1], CUT + PITCH),
        lower(&bar, BAR),
        button,
        label,
        value,
    ];
    sheet.insert_parts(w + 1, parts);
    Some(HacksLayout {
        art,
        art_width: size[0],
        picture,
        description,
    })
}

impl Room {
    /// The weapon-limit check before Ready / Start: when the equipped weapons fail the room's limit, the box with
    /// the limit's two strings opens and true comes back (nothing is sent).
    // EVIDENCE: cshell+0x164810 (command 0xd: the check before the start / ready / join request), cshell+0x160f70
    // (the check and its box), cshell+0x160190 (the box: both strings, one after the other)
    // CLI-1132: the box is a Yes / No box (kind 1) with the notice title 0x371, the limit's two strings on two lines and
    // the buttons 0x177e ("Go to Shop") / 0x43; Yes sends the bar command 0x3d (61, the
    // shop), No only closes it
    // DISASM: cshell+0x160190 (the box spec: kind 1, title 0x371, captions 0x177e / 0x43, sound select 1, callback
    // cshell+0x15bb40: result and screen set -> screen vtable +4 (0x3d, 0, 0))
    fn arms_box(ctx: &mut Ctx) -> bool {
        Self::arms_box_for(ctx, ctx.data.room.weapons)
    }

    /// [`Room::arms_box`] for weapon limit `limit`.
    fn arms_box_for(ctx: &mut Ctx, limit: i32) -> bool {
        let wares = super::storage::Wares::cached(ctx.cat.rom.as_deref());
        let refusal = arms_refusal(limit, &ctx.data.store.equipped, |n| wares.kind_of(n));
        let Some((what, todo)) = refusal else {
            return false;
        };
        let t = format!("{}\n{}", ctx.cat.text(what), ctx.cat.text(todo));
        Self::limit_box(ctx, t);
        true
    }

    /// CLI-1899: the Invite button: 0x396 (header only) and the wait armed, or the box of [`invite_check`].
    // SPEC: PROTOCOL_2009.md §3.7 (0x396 header only)
    // EVIDENCE: cshell+0x15fd30, cshell+0x38f20 -> cshell+0x18d960 (the 6-byte 0x396)
    fn invite(ctx: &mut Ctx) {
        let clan = ctx.data.server.as_ref().is_some_and(|e| e.clan_only != 0);
        let mut teams = [0usize; 2];
        let mut others = 0;
        for m in ctx.data.members.values() {
            match m.team {
                1 => teams[0] += 1,
                2 => teams[1] += 1,
                _ => others += 1,
            }
        }
        let now = Instant::now();
        let max = ctx.data.room.max_players;
        match invite_check(clan, teams, others, max, ctx.data.invite_sent, now) {
            Ok(()) => {
                ctx.data.invite_sent = Some(now);
                ctx.send(LinkCmd::InviteAll);
            }
            Err(InviteRefusal::Box(id)) => ctx.notice(id),
            Err(InviteRefusal::Wait(secs)) => {
                let t = roomtext::format_message(&ctx.cat.text(INVITE_WAIT), secs);
                ctx.popups.push(super::popup::Popup::ok(t, BoxTag::None));
            }
        }
    }

    fn limit_box(ctx: &mut Ctx, text: String) {
        ctx.popups
            .push(super::popup::Popup::ask(text, super::popup::BoxTag::LimitBox).limit_buttons());
    }

    /// LG-342: the lobby refused a Ready / Start / late join by the room's weapon limit (our 0x393 reason): the 2009
    /// weapon-limit box of the room's limit (CLI-1132: its two ROM strings, Go to Shop / Cancel), whatever this
    /// client's own check found; a limit without 2009 strings gets our line with the limit's ROM name.
    // DC-DEFAULT: LG-342 (proposed, project decision) the box on the lobby's weapon-limit refusal
    fn refused_by_limit(ctx: &mut Ctx) {
        let limit = ctx.data.room.weapons;
        let text = match u32::try_from(limit)
            .ok()
            .and_then(dc_bute::arms::refusal_texts)
        {
            Some((what, todo)) => format!("{}\n{}", ctx.cat.text(what), ctx.cat.text(todo)),
            None => limit_refusal_line(&roomtext::weapon_rule_label(ctx.cat, limit)),
        };
        Self::limit_box(ctx, text);
    }
}

/// LG-342: our line for a weapon-limit refusal whose limit has no 2009 strings (`label`: the limit's ROM name).
pub fn limit_refusal_line(label: &str) -> String {
    format!("Your equipped weapons don't fit this room's weapon limit: {label}")
}

/// The text ids of the weapon-limit box when the equipped weapons (`equipped`: the 0x323 slots, item numbers, -1
/// empty) fail `limit`; `None` when they pass, the limit has no box, or no equipped weapon's type is known (no ROM).
// SPEC: DSERVER_PIPE.md §12.A (slots 0..3 weapons, 10..13 backpack weapons)
// EVIDENCE: cshell+0x226dd0 (the equipped weapons of both weapon lists)
pub fn arms_refusal(
    limit: i32,
    equipped: &[i32; 15],
    kind: impl Fn(i32) -> Option<i32>,
) -> Option<(i32, i32)> {
    let limit = u32::try_from(limit).ok()?;
    let texts = dc_bute::arms::refusal_texts(limit)?;
    let types: Vec<i32> = equipped
        .iter()
        .enumerate()
        .filter(|(slot, n)| matches!(slot, 0..=3 | 10..=13) && (0..1000).contains(*n))
        .filter_map(|(_, &n)| kind(n))
        .collect();
    if types.is_empty() && equipped.iter().any(|n| (0..1000).contains(n)) {
        return None;
    }
    (!dc_bute::arms::entry_ok(limit, types)).then_some(texts)
}

/// Goal limits of a mode row (min, max, step) from the ROM room table.
// OBSERVED: ROM room table mode rows (smallest goal, largest goal, step; read at run time)
fn goal_range(cat: &Catalog, mode: i32) -> Option<(i32, i32, i32)> {
    const MIN: KeyPin = key("953d7ffedaf99d1f", 7);
    const MAX: KeyPin = key("38d1fd00a6a7821a", 7);
    const STEP: KeyPin = key("c18d05b36ef40782", 7);
    let row = cat.mode_row(mode)?;
    let get = |k: &KeyPin| {
        super::catalog::find_key(row.iter(), k).and_then(|v| v.trim().parse::<i32>().ok())
    };
    Some((get(&MIN)?, get(&MAX)?, get(&STEP).unwrap_or(1).max(1)))
}

impl Screen for Room {
    fn popup_done(&mut self, p: &super::popup::Popup, yes: bool, ctx: &mut Ctx) {
        // EVIDENCE: cshell+0x15bb40 (the weapon-limit box's callback sends command 0x3d, the bar's shop command)
        if p.tag == super::popup::BoxTag::LimitBox && yes {
            leave_for_bar(super::shop::CMD_BAR_SHOP, ctx);
        }
        // CLI-1303: the kick question's Yes sends the kick
        if let (super::popup::BoxTag::Kick { id, char_no }, true) = (p.tag, yes) {
            ctx.send(LinkCmd::Kick { id, char_no });
        }
        // CLI-1905: the lock box's OK sends the typed password (empty: the room loses its password); Cancel nothing
        // EVIDENCE: cshell+0x15baf0 (OK -> command 0x53 with the text), cshell+0x164810 (0x53: no length test)
        // DC-DEFAULT: CLI-1905 (proposed) only the host's OK is sent (the lobby refuses a guest's change anyway)
        if (p.tag, yes) == (super::popup::BoxTag::LockRoom, true) && ctx.data.host() {
            ctx.send(LinkCmd::RoomPassword {
                mode: ctx.data.room.mode,
                password: p.input.clone(),
            });
        }
    }
    fn list_follow(&self) -> Vec<(i32, i32)> {
        vec![(CHAT_PANEL, CHAT_LIST)]
    }
    fn list_rows(&self, data: &MenuData) -> Vec<(i32, i32, usize, i32)> {
        vec![(
            CHAT_PANEL,
            CHAT_LIST,
            data.chat_view().len(),
            super::lobby::CHAT_ROW,
        )]
    }
    fn name(&self) -> &'static str {
        "room"
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    fn input_locked(&self) -> bool {
        self.start_lock
    }

    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        match k {
            // CLI-1310: no leave by Escape while the start countdown holds the room (Exit is locked, see click)
            MenuKey::Escape => self.click(CMD_EXIT, usize::MAX, ctx),
            // cshell+0x205e0: key 0xd -> command 0x75 (the chat send)
            MenuKey::Enter if self.kick_prompt => self.kick_by_prompt(ctx),
            MenuKey::Enter => self.click(CMD_CHAT_SEND, usize::MAX, ctx),
            // the window delivers Space as a key: a space in the chat box (CLI-483)
            MenuKey::Space => {
                self.sheet.type_char(' ');
                // CLI-1302: an operator's `/k ` turns into the kick prompt
                if super::gm::kick_trigger(ctx.data.gm.operator, textbox::typed_chat(&self.sheet)) {
                    let prompt = ctx.cat.text(super::gm::TEXT_KICK_PROMPT);
                    if let Some(i) = textbox::chat_box(&self.sheet) {
                        self.sheet.parts[i].text = prompt;
                        self.kick_prompt = true;
                    }
                }
            }
            // CLI-1173: F2 / F3 set the chat target (cshell+0x205e0)
            MenuKey::F2 | MenuKey::F3 => {
                super::chatdrop::key_target(&mut self.sheet, ctx.cat, ctx.data, k);
            }
            _ => {}
        }
    }

    fn input_seen(&mut self, ctx: &mut Ctx) {
        self.last_input = Instant::now();
        if self.away {
            self.away = false;
            ctx.send(LinkCmd::Away(false));
        }
    }

    fn age_start_watch(&mut self, by: Duration) {
        Room::age_start_watch(self, by);
    }

    fn due(&mut self) -> Option<i32> {
        let now = Instant::now();
        if self.start_watch_due(now) {
            return Some(CMD_START_WATCHDOG);
        }
        self.idle_due(now)
    }

    fn click(&mut self, command: i32, _part: usize, ctx: &mut Ctx) {
        // CLI-1310: while the start countdown holds the room only the chat and the idle timers act (no exit,
        // ready / start, team, setting list or bar command)
        // DISASM: cshell+0x15e970 / cshell+0x15ec10 / cshell+0x160b00 (the room's pointer handlers return at once
        // while byte +0x56f4 is set: no control gets the click)
        if self.start_lock && !runs_while_locked(command) {
            return;
        }
        let host = ctx.data.host();
        match command {
            CMD_CHAT_SEND => {
                textbox::submit_chat(&mut self.sheet, ctx);
            }
            CMD_IDLE_AWAY => {
                self.mark_away(Instant::now());
                ctx.send(LinkCmd::Away(true));
            }
            // CLI-1314: no match address in time: box 0x473, the failure notice 0x394 {1}, the room is free again
            CMD_START_WATCHDOG => {
                self.start_watch = None;
                self.start_lock = false;
                ctx.notice(START_FAILED);
                ctx.send(LinkCmd::StartFailed);
            }
            CMD_IDLE_LEAVE => {
                self.idle_left = true;
                ctx.send(LinkCmd::QuitRoom);
            }
            CMD_START if host => {
                if !Room::arms_box(ctx) {
                    ctx.send(LinkCmd::Start);
                }
            }
            CMD_START | CMD_READY => {
                let ready = Room::own_state(ctx.data) == 1;
                if ready || !Room::arms_box(ctx) {
                    ctx.send(LinkCmd::Ready(!ready));
                }
            }
            CMD_EXIT => ctx.send(LinkCmd::QuitRoom),
            CMD_TEAM if !Room::is_ffa(ctx.cat, ctx.data) => {
                let to = if Room::own_team(ctx.data) == 1 { 2 } else { 1 };
                ctx.send(LinkCmd::Team(to));
            }
            // CLI-1899: Invite, for every member (the host test of the setting buttons is not on this command)
            CMD_INVITE => Room::invite(ctx),
            // CLI-1905: the host's Change password button opens the lock box
            // EVIDENCE: cshell+0x15fc80
            CMD_CHANGE_PASSWORD if host => {
                let text = ctx.cat.text(super::popup::LOCK_ROOM_TEXT);
                ctx.popups.push(super::popup::Popup::lock_room(text));
            }
            // the host's setting buttons open their list; any of them closes the open one
            // EVIDENCE: cshell+0x164810 (commands 0xf1 / 0xf3 through cshell+0x1639e0, 0xf5 / 0xf7 / 600: not the
            // host -> nothing; the list shown -> hidden; else the list of kind 6..10 is built and shown)
            // CLI-1897: a mode row that locks the weapon limit opens no list
            // EVIDENCE: cshell+0x15e970 (the Arms field's click sends 600 only with the row's weapon flag)
            CMD_ARMS
                if !ctx
                    .cat
                    .mode_option_open(ctx.data.room.mode, RoomOption::Weapons) => {}
            CMD_MAP | CMD_MODE | CMD_GOAL | CMD_PLAYERS | CMD_ARMS if host => {
                let was_open = self.drop.take().is_some();
                if !was_open {
                    let list = match command {
                        CMD_MAP => "map",
                        CMD_MODE => "mode",
                        CMD_GOAL => "goal",
                        CMD_PLAYERS => "players",
                        _ => "arms",
                    };
                    self.open_list(ctx.cat, ctx.data, list);
                }
            }
            // CLI-1626: the Hacks list like the other setting lists (the host's, only with a choice offered)
            hacks_row::CMD if host && ctx.data.room.hacks_offered != 0 => {
                let was_open = self.drop.take().is_some();
                if !was_open {
                    self.open_list(ctx.cat, ctx.data, "hacks");
                }
            }
            hacks_row::CMD if host && ctx.data.room.hardcore_offered => {
                let was_open = self.drop.take().is_some();
                if !was_open {
                    self.open_list(ctx.cat, ctx.data, "hardcore");
                }
            }
            hacks_row::CMD => {}
            // w3-shop: the bar's Shop / Storage, Black Market, Modification and Inbox (Exit comes back here)
            c => leave_for_bar(c, ctx),
        }
    }

    /// The chat combos: a press shows the list of the combo under it, or hides it when shown.
    // EVIDENCE: cshell+0x164810 (commands the waiting room does not handle go to the base lobby handler, cshell+0x1bf20:
    // 0x274 / 0x278 toggle the filter / target lists, 0x275 / 0x279 set them), cshell+0x1e880 (the lists' rows)
    // CLI-941: the combos were drawn but a press did nothing here
    /// CLI-1302: Backspace into the `/k` prompt clears the field and ends the prompt.
    fn backspace(&mut self, ctx: &mut Ctx) -> bool {
        if !self.kick_prompt {
            return false;
        }
        let prompt = ctx.cat.text(super::gm::TEXT_KICK_PROMPT);
        if !super::gm::kick_prompt_backspace(textbox::typed_chat(&self.sheet), &prompt) {
            return false;
        }
        if let Some(i) = textbox::chat_box(&self.sheet) {
            self.sheet.parts[i].text.clear();
        }
        self.kick_prompt = false;
        true
    }

    fn press_alt(&mut self, p: [f32; 2], ctx: &mut Ctx) {
        self.member_menu = None;
        self.open_member_menu(p, ctx);
    }

    fn hover(&mut self, p: [f32; 2], _data: &MenuData) {
        self.member_hover = self.member_menu.as_ref().and_then(|m| m.entry_at(p));
    }

    fn press(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        // CLI-1303: the open member menu takes the press (an entry runs; any press closes it)
        if let Some(m) = self.member_menu.take() {
            if let Some(entry) = m.entry_at(p) {
                self.member_entry(entry, m.id, ctx);
                return true;
            }
        }
        let Some(cmd) = super::chatdrop::combo_at(&self.sheet, p) else {
            return self.open_member_menu(p, ctx);
        };
        let list = if cmd == super::chatdrop::CMD_FILTER {
            super::chatdrop::LIST_FILTER
        } else {
            super::chatdrop::LIST_TARGET
        };
        let was = self.drop.take().map(|d| d.list);
        if was != Some(list) {
            self.drop = super::chatdrop::open(&self.sheet, ctx.cat, cmd);
        }
        true
    }

    fn dropdown(&self) -> Option<&Dropdown> {
        self.drop.as_ref()
    }

    fn dropdown_mut(&mut self) -> Option<&mut Dropdown> {
        self.drop.as_mut()
    }

    fn close_dropdown(&mut self) {
        self.drop = None;
    }

    /// A row of the setting list: the room's field changes when the value differs.
    // EVIDENCE: cshell+0x1614f0 (map -> 0x38c type 3, mode -> type 4, goal -> type 8, players -> type 6, weapon limit
    // -> type 0xe with the row's index; nothing when the value is the room's)
    fn dropdown_pick(&mut self, list: &'static str, value: i32, _index: usize, ctx: &mut Ctx) {
        // CLI-941: the chat combos' rows set the filter / target and the combo's caption
        if super::chatdrop::is_chat_list(list) {
            super::chatdrop::pick(&mut self.sheet, ctx.cat, ctx.data, list, value);
            return;
        }
        let room = &ctx.data.room;
        // LG-274: the Hacks pick goes as our request 0x3f04 (the lobby answers every member with 0x3f03)
        // LG-306: the Hardcore pick goes as our request 0x3f09
        if list == "hardcore" {
            if (value != 0) != room.hardcore && ctx.data.host() {
                ctx.send(LinkCmd::RoomHardcore(value != 0));
            }
            return;
        }
        if list == "hacks" {
            if value as u32 != room.hacks && ctx.data.host() {
                ctx.send(LinkCmd::RoomHacks(value.max(0) as u32));
            }
            return;
        }
        // LG-307: the Possession row and the ROM rows are one exclusive choice
        if list == "arms" && ctx.data.host() {
            if value == POSSESSION_ROW {
                if !room.possession {
                    ctx.send(LinkCmd::RoomPossession(true));
                }
                return;
            }
            if room.possession && value == room.weapons {
                ctx.send(LinkCmd::RoomPossession(false));
                return;
            }
        }
        // LG-305: the Gun Game row asks with our request (after moving to the free-for-all row when needed); the
        // free-for-all row itself turns it off; any other mode change ends it in the lobby
        if list == "mode" && ctx.data.host() {
            use super::create::{free_for_all_row, GUN_GAME_ROW};
            let base = free_for_all_row(ctx.cat, ctx.data);
            if value == GUN_GAME_ROW {
                let (current, on) = (room.mode, room.gun_game);
                if !on {
                    if let Some(b) = base.filter(|b| *b != current) {
                        ctx.send(LinkCmd::Setting {
                            mode: current,
                            kind: SET_MODE,
                            value: b.max(0) as u32,
                        });
                    }
                    ctx.send(LinkCmd::RoomGunGame(true));
                }
                return;
            }
            if room.gun_game && value == room.mode {
                ctx.send(LinkCmd::RoomGunGame(false));
                return;
            }
        }
        let (kind, now) = match list {
            "map" => (SET_MAP, room.map),
            "mode" => (SET_MODE, room.mode),
            "goal" => (SET_GOAL, room.goal),
            "players" => (SET_MAX_PLAYERS, room.max_players),
            "arms" => (SET_WEAPONS, room.weapons),
            _ => return,
        };
        if value != now && ctx.data.host() {
            ctx.send(LinkCmd::Setting {
                mode: ctx.data.room.mode,
                kind,
                value: value.max(0) as u32,
            });
        }
    }

    fn check(&mut self, command: i32, _part: usize, _on: bool, ctx: &mut Ctx) {
        if let Some((k, (on, _, kind))) = OPTION_COMMANDS
            .iter()
            .enumerate()
            .find(|(_, (a, b, _))| *a == command || *b == command)
        {
            // CLI-1897: a locked option sends nothing (its checks are disabled, the click only shows the value again)
            if option_open(ctx.cat, ctx.data, OPTION_KINDS[k]) {
                ctx.send(LinkCmd::Setting {
                    mode: ctx.data.room.mode,
                    kind: *kind,
                    value: (command == *on) as u32,
                });
            }
        }
        // the checks show the room's value until the lobby confirms a change
        let data = &*ctx.data;
        self.sync(ctx.cat, data);
    }

    fn event(&mut self, e: &LinkEvent, ctx: &mut Ctx) {
        let was_locked = self.start_lock;
        self.start_lock = start_lock_after(was_locked, e, Room::own_state(ctx.data));
        // CLI-1314: every tick of a starting member restarts the watchdog; the room in game (2) does not disarm it
        // (the match address is what ends the wait), a room the lobby took back (state 0) does
        match e {
            LinkEvent::Countdown(_) if self.start_lock => self.start_watch = Some(Instant::now()),
            LinkEvent::RoomSetting { kind: 7, value, .. } if !matches!(*value, 1 | 2) => {
                self.start_watch = None
            }
            _ => {}
        }
        if self.start_lock && !was_locked {
            self.drop = None;
        }
        match e {
            // CLI-1312: a host change while the start countdown holds the room: the red line `res 0x577`
            // DISASM: cshell+0x168680 / cshell+0x134830 (0x3035 type 2 with byte +0x56f4 set: line 0x577 in
            // 0xffff0000; the ready toggle after it needs own +0x1c, which the starting state 2 cleared, cshell+0x1673a0)
            LinkEvent::RoomSetting { kind: 2, .. } if self.start_lock => {
                let t = ctx.cat.text(HOST_CHANGED_WHILE_STARTING);
                ctx.data.say_in(t, super::frontfx::COUNTDOWN_COLOUR);
            }
            LinkEvent::FlagReply {
                ok: false,
                reason: dc_wire::flagrefusal::REASON_WEAPON_LIMIT,
                ..
            } => Room::refused_by_limit(ctx),
            LinkEvent::FlagReply {
                ok: false,
                reason,
                kind,
            } => {
                let id = match (*kind, *reason) {
                    (FLAG_KIND_START, REASON_FINAL_ROUND) => text_ids::LAST_ROUND_BUSY,
                    (_, REASON_JOIN_FAILED) => text_ids::TIMEOUT,
                    _ => text_ids::NEED_PLAYERS,
                };
                ctx.notice(id);
            }
            // LG-342: the host's new weapon limit fails this member's equipped weapons: the weapon-limit box now,
            // not at the next Ready
            // DC-DEFAULT: LG-342 (proposed, project decision) the 2009 client checked only on Ready / Start
            LinkEvent::RoomSetting { kind, value, .. } if *kind == SET_WEAPONS && !ctx.data.host() => {
                Room::arms_box_for(ctx, *value as i32);
            }
            LinkEvent::TeamReply { ok: false, .. } => ctx.notice(0x564),
            LinkEvent::SettingReply { ok: false } => ctx.notice(text_ids::GAME_RUNNING),
            // the countdown's red line and its sound come from the menu (`Menu::update`, `frontfx`)
            LinkEvent::LeftRoom => *ctx.go = Some(Go::Lobby),
            // CLI-1303 (amended): the Character Info entry of the member menu asks for the profile; the reply opens
            // the Character Info box over the waiting room as it does over the room list (CLI-931)
            // EVIDENCE: cshell+0x15dde0 (the 0x573 entry of the member menu), cshell+0x35cf0 (the reply fills the box)
            LinkEvent::ProfileInfo(p) => {
                ctx.data.char_view = Some(p.clone());
                *ctx.go = Some(Go::CharInfo);
            }
            // CLI-1303: the kick's answer
            // EVIDENCE: cshell+0x160340 (ok -> 0x1849 with the kicked name; else the reason's string; a message box)
            LinkEvent::KickReply { ok, reason } => {
                let t = if *ok {
                    super::gm::printf(
                        &ctx.cat.text(super::gm::TEXT_KICKED),
                        &[super::gm::Arg::S(&self.kick_target)],
                    )
                } else {
                    ctx.cat.text(super::gm::kick_refusal_text(*reason))
                };
                ctx.popups.push(super::popup::Popup::ok(t, BoxTag::None));
            }
            // CLI-1899: the invite's answer: ok -> its box; a refusal shows nothing and allows the next invite at once
            // EVIDENCE: cshell+0x39060 (+6 != 0 -> 0x509, or 0x50d with shell +0x12c set; 0 -> the wait flag set, a log
            // line, no box)
            // DC-DEFAULT: CLI-1899 (proposed) the answer is read while the waiting room is open (2009: shell-wide)
            LinkEvent::InviteAllReply { ok: true } => {
                let clan = ctx.data.server.as_ref().is_some_and(|e| e.clan_only != 0);
                ctx.notice(if clan { INVITE_SENT_CLAN } else { INVITE_SENT });
            }
            LinkEvent::InviteAllReply { ok: false } => ctx.data.invite_sent = None,
            LinkEvent::MemberRemoved { id, reason } if *id == ctx.data.profile.my_id => {
                match reason {
                    1 => ctx.error(text_ids::KICKED_MOD, BoxTag::None),
                    2 => ctx.error(text_ids::KICKED_GM, BoxTag::None),
                    _ => {}
                }
                *ctx.go = Some(Go::Lobby);
            }
            _ => {}
        }
        if !ctx.data.host()
            && !self
                .drop
                .as_ref()
                .is_some_and(|d| super::chatdrop::is_chat_list(d.list))
        {
            self.drop = None;
        }
        // an open list whose rows the room change altered (the mode: map / goal / players rows) is rebuilt
        // DC-DEFAULT: CLI-506 (proposed) rows follow the room's current settings while the list is open
        let stale = self
            .drop
            .as_ref()
            .filter(|d| !super::chatdrop::is_chat_list(d.list))
            .and_then(|d| {
                let fresh = Room::list_rows(ctx.cat, ctx.data, d.list);
                (fresh.len() != d.values.len()
                    || fresh.iter().zip(&d.values).any(|((_, v), w)| v != w))
                .then_some(d.list)
            });
        if let Some(list) = stale {
            self.open_list(ctx.cat, ctx.data, list);
        }
        let data = &*ctx.data;
        self.sync(ctx.cat, data);
    }

    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, _secs: f32, out: &mut Vec<DrawItem>) {
        self.draw_members(cat, data, out, None);
        if let Some(m) = &self.member_menu {
            m.draw(cat, self.member_hover, out);
        }
    }

    fn draw_extra_noted(
        &self,
        cat: &mut Catalog,
        data: &MenuData,
        _secs: f32,
        out: &mut Vec<DrawItem>,
        notes: &mut DynNotes,
    ) {
        self.draw_members(cat, data, out, Some(notes));
        if let Some(m) = &self.member_menu {
            m.draw(cat, self.member_hover, out);
        }
    }
}

impl Room {
    /// The member whose row is under `p` (in-channel id).
    fn member_at(&self, cat: &Catalog, data: &MenuData, p: [f32; 2]) -> Option<u32> {
        let st = &self.style;
        let lists = [
            self.sheet.part(TEAM_WINDOW, TEAM_A_ROWS, WidgetKind::List),
            self.sheet.part(TEAM_WINDOW, TEAM_B_ROWS, WidgetKind::List),
        ];
        let left_rows = lists[0].map_or(0, |l| {
            (0..data.members.len())
                .take_while(|&i| list_row(l, st.bar[1], i, 0).is_some())
                .count()
        });
        let slots = member_slots(
            data.members.iter().map(|(id, m)| (*id, m.team)),
            Room::is_ffa(cat, data),
            left_rows,
            data.room.host_id,
        );
        slots.into_iter().find_map(|(id, team, i)| {
            let list = lists.get((team - 1) as usize).copied().flatten()?;
            let at = list_row(list, st.bar[1], i, 0)?;
            let inside = p[0] >= at[0] as f32
                && p[1] >= at[1] as f32
                && p[0] < (at[0] + st.bar[0]) as f32
                && p[1] < (at[1] + st.bar[1]) as f32;
            inside.then_some(id)
        })
    }

    /// CLI-1303: a press on another member's row opens the member menu at the pointer (the own row has none).
    // EVIDENCE: cshell+0x15f860 (the member menu shown at the pointer for the member; cshell+0x15dde0 fills it)
    // DC-DEFAULT: CLI-1303 (proposed) a left or right press on a member row opens it, as the room list's user menu
    // (CLI-471); not on the own row
    fn open_member_menu(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        let Some(id) = self.member_at(ctx.cat, ctx.data, p) else {
            return false;
        };
        if id == ctx.data.profile.my_id {
            return false;
        }
        let d = &*ctx.data;
        self.member_menu = Some(super::gm::MemberMenu {
            at: [p[0] as i32, p[1] as i32],
            id,
            entries: super::gm::member_menu_entries(
                d.gm.operator,
                d.host(),
                d.char_extras.room_badge,
                d.room.badge,
            ),
        });
        self.member_hover = None;
        true
    }

    /// A member menu entry for member `id`.
    // EVIDENCE: cshell+0x164810 (command 0x7533, the Kick User entry: cshell+0x1604b0, then the 0x1848 yes / no box
    // with the member's name), cshell+0x129e00 (the user menu's add-friend and profile entries)
    fn member_entry(&mut self, entry: i32, id: u32, ctx: &mut Ctx) {
        let Some(m) = ctx.data.members.get(&id).cloned() else {
            return;
        };
        match entry {
            super::gm::TEXT_PROFILE => ctx.send(LinkCmd::Profile(id)),
            super::gm::TEXT_ADD_FRIEND => {
                ctx.send(LinkCmd::Messenger(super::messenger::add_command(&m.name)))
            }
            super::gm::TEXT_KICK => {
                // CLI-1306: a member starting or in a match, or a GM-named one, is refused here; nothing is sent
                if let Some(t) = super::gm::kick_precheck(m.state, &m.name) {
                    ctx.notice(t);
                    return;
                }
                self.kick_target = m.name.clone();
                let t = super::gm::printf(
                    &ctx.cat.text(super::gm::TEXT_KICK_ASK),
                    &[super::gm::Arg::S(&m.name)],
                );
                ctx.popups.push(super::popup::Popup::ask(
                    t,
                    BoxTag::Kick {
                        id,
                        char_no: m.char_no as u32,
                    },
                ));
            }
            _ => {}
        }
    }

    /// CLI-1302: Enter with the `/k` prompt up: the first word after the prompt names the member to kick; the field
    /// clears and the prompt ends. No member of that name: the red line 0x57c; a member in a match: 0x57b; else the
    /// room kick 0x38e with the member's in-channel id and character number.
    // EVIDENCE: cshell+0x15fef0 (clean tree: both team lists searched with wcscmp of the name; none -> string 0x57c
    // through the chat page's vtable +0x140 in 0xffff0000; member state 3 -> 0x57b the same way; else
    // cshell+0x18f280 with the member's ids, the 0x38e sender)
    fn kick_by_prompt(&mut self, ctx: &mut Ctx) {
        let prompt = ctx.cat.text(super::gm::TEXT_KICK_PROMPT);
        let name = super::gm::kick_prompt_name(textbox::typed_chat(&self.sheet), &prompt);
        if let Some(i) = textbox::chat_box(&self.sheet) {
            self.sheet.parts[i].text.clear();
        }
        self.kick_prompt = false;
        let Some(name) = name else {
            return;
        };
        let found = ctx
            .data
            .members
            .iter()
            .find(|(_, m)| m.name == name)
            .map(|(id, m)| (*id, m.char_no as u32, m.state));
        match found {
            None => {
                let t = ctx.cat.text(super::gm::TEXT_NO_MEMBER);
                ctx.data.say_in(t, super::gm::FAIL_COLOUR);
            }
            Some((_, _, super::gm::MEMBER_IN_MATCH)) => {
                let t = ctx.cat.text(super::gm::TEXT_MEMBER_IN_MATCH);
                ctx.data.say_in(t, super::gm::FAIL_COLOUR);
            }
            Some((id, char_no, _)) => {
                self.kick_target = name;
                ctx.send(LinkCmd::Kick { id, char_no });
            }
        }
    }
}

impl Room {
    /// The member rows of both teams and the chat lines; with `notes` the rows note their 2009 rule
    /// ([`member_row_wants`]).
    fn draw_members(
        &self,
        cat: &mut Catalog,
        data: &MenuData,
        out: &mut Vec<DrawItem>,
        mut notes: Option<&mut DynNotes>,
    ) {
        let st = &self.style;
        let ffa = Room::is_ffa(cat, data);
        let lists = [
            self.sheet.part(TEAM_WINDOW, TEAM_A_ROWS, WidgetKind::List),
            self.sheet.part(TEAM_WINDOW, TEAM_B_ROWS, WidgetKind::List),
        ];
        let left_rows = lists[0].map_or(0, |l| {
            (0..data.members.len())
                .take_while(|&i| list_row(l, st.bar[1], i, 0).is_some())
                .count()
        });
        let slots = member_slots(
            data.members.iter().map(|(id, m)| (*id, m.team)),
            ffa,
            left_rows,
            data.room.host_id,
        );
        // the clan mark table, read only when a member's mark is shown (both ids set, cshell+0x15beb0)
        let marks = data
            .member_clan
            .values()
            .any(|k| k.id != 0 && k.mark != 0 && k.ground != 0)
            .then(|| cat.rom.clone())
            .flatten()
            .map(|r| super::clan::ClanLits::cached(&r).marks);
        for (id, team, i) in slots {
            let Some(m) = data.members.get(&id) else {
                continue;
            };
            let id = &id;
            {
                let Some(list) = lists[(team - 1) as usize] else {
                    continue;
                };
                let Some(at) = list_row(list, st.bar[1], i, 0) else {
                    continue;
                };
                let wants = notes.as_ref().map(|_| member_row_wants(st, team, i, at));
                let mut note = |out: &Vec<DrawItem>, k: usize| {
                    if let (Some(n), Some(w)) = (notes.as_deref_mut(), wants.as_ref()) {
                        n.push((out.len() - 1, w[k].clone()));
                    }
                };
                if *id == data.profile.my_id {
                    // DC-DEFAULT: LG-10 (proposed) the own row in a free-for-all room uses the first team's bar
                    let bar = if ffa { 0 } else { (team - 1) as usize };
                    // the bar at the row plus its own position, not at the row
                    // DISASM: cshell+0x15c660 (bar control +0xfc placed at the row plus BAR_AT)
                    if let Some(t) = &st.bars[bar] {
                        out.push(image(t, add(at, st.bar_at), st.bar));
                    }
                }
                // the rank icon of the member's experience; the layout's badge when the experience or the level
                // table is unknown
                // EVIDENCE: cshell+0x1657e0 (member row rank texture), cshell+0x2683d0 (icon of the level of the
                // experience), SPEC: PROTOCOL_2009.md §3.7 table 3.7b (+0x42 exp)
                let rank = self
                    .levels
                    .as_ref()
                    .zip(exp_of(data, *id))
                    .and_then(|(l, e)| l.icon_of_exp(e, &m.name));
                if let Some(t) = rank.or(st.level.as_deref()) {
                    out.push(image(t, add(at, st.level_at), st.level_size));
                }
                if *id == data.room.host_id {
                    if let Some(t) = &st.master {
                        out.push(image(t, add(at, st.master_at), st.master_size));
                    }
                }
                let c = if *id == data.profile.my_id {
                    super::lobby::OWN_COLOUR
                } else {
                    super::lobby::ROW_COLOUR
                };
                let item = data
                    .member_colour
                    .get(id)
                    .and_then(|&(n, _)| super::charinfo::table_colour(cat.rom.as_deref(), n));
                let name_colour = member_name_colour(*id == data.profile.my_id, c, item);
                out.push(place_text(
                    &m.name,
                    add(at, st.name_at),
                    [0, 0],
                    MEMBER_NAME_ALIGN,
                    MEMBER_FONT,
                    MEMBER_TEXT_SIZE,
                    name_colour,
                ));
                note(out, 0);
                // CLI-1895: the clan's mark and name after the member's name, for a member in a clan
                // EVIDENCE: cshell+0x166310 (clan name -> cshell+0x15bc00 text +0x11c; clan number, emblem and
                // background -> cshell+0x15beb0: mark +0x114 hidden for clan 0, its layers shown when both ids are
                // set), cshell+0x1657e0 (the mark control 26 x 26 at scale 0x3ed2d2d3; the clan text font 0xe size
                // 0xc, flags 0x41 like the name)
                if let Some(k) = data.member_clan.get(id).filter(|k| k.id != 0) {
                    if let (Some(p), Some(m)) = (st.clan_mark_at, marks.as_ref()) {
                        if let Some((e, b)) = m.textures(k.mark, k.ground) {
                            let pos = add(at, p);
                            let r = super::clan::MARK_PANEL_DRAWN;
                            super::clanpage::mark_cropped(
                                Some(b),
                                Some(e),
                                [pos[0], pos[1], r[0], r[1]],
                                out,
                            );
                        }
                    }
                    if let Some(p) = st.clan_name_at.filter(|_| !k.name.is_empty()) {
                        let item = data.member_colour.get(id).and_then(|&(_, n)| {
                            super::charinfo::table_colour(cat.rom.as_deref(), n)
                        });
                        out.push(place_text(
                            &k.name,
                            add(at, p),
                            [0, 0],
                            MEMBER_NAME_ALIGN,
                            MEMBER_FONT,
                            MEMBER_TEXT_SIZE,
                            member_clan_colour(item),
                        ));
                    }
                }
                let sub = data.member_sub.get(id).copied().unwrap_or(0);
                if let Some(s) = member_state_text(m.state, sub) {
                    let t = cat.text(s);
                    out.push(place_text(
                        &t,
                        add(at, st.state_at),
                        [0, 0],
                        MEMBER_STATE_ALIGN,
                        MEMBER_FONT,
                        MEMBER_TEXT_SIZE,
                        c,
                    ));
                    note(out, 1);
                }
            }
        }
        let _ = (CHAT_PANEL, CHAT_LIST);
        draw_chat(&self.sheet, data, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI-1887: a member's colour number's colour beats the own cyan and the row colour.
    #[test]
    fn member_clan_name_takes_the_clan_colour_or_the_rows_own() {
        assert_eq!(member_clan_colour(Some(0xff12_3456)), 0xff12_3456);
        assert_eq!(member_clan_colour(None), CLAN_NAME_COLOUR);
    }

    #[test]
    fn member_rows_take_the_name_colour_items_colour() {
        let (row, c) = (0xffdd_dddd, 0xff12_3456);
        assert_eq!(member_name_colour(true, row, Some(c)), c);
        assert_eq!(member_name_colour(false, row, Some(c)), c);
        assert_eq!(member_name_colour(true, row, None), OWN_NAME_COLOUR);
        assert_eq!(member_name_colour(false, row, None), row);
    }

    /// CLI-980: a "Sniper Only" room's Ready / Start opens the 2009 box when no sniper rifle is equipped.
    #[test]
    fn sniper_only_without_a_sniper_rifle_opens_the_weapon_limit_box() {
        // synthetic weapon types: 10 assault rifle (0), 20 pistol (4), 30 sniper rifle (2), 40 knife (7)
        let kind = |n: i32| match n {
            10 => Some(0),
            20 => Some(4),
            30 => Some(2),
            40 => Some(7),
            _ => None,
        };
        let mut eq = [-1; 15];
        eq[0] = 10;
        eq[1] = 20;
        eq[2] = 40;
        assert_eq!(arms_refusal(6, &eq, kind), Some((0x1795, 0x1796)));
        assert_eq!(arms_refusal(0, &eq, kind), None);
        assert_eq!(
            arms_refusal(1, &eq, kind),
            None,
            "a pistol for Pistols Only"
        );
        eq[10] = 30;
        assert_eq!(
            arms_refusal(6, &eq, kind),
            None,
            "a backpack sniper rifle counts"
        );
        // no type known (no ROM): nothing to check
        assert_eq!(arms_refusal(6, &[5; 15], |_| None), None);
    }

    /// CLI-1310 / CLI-1311: the start lock's state machine.
    #[test]
    fn start_lock_follows_the_countdown_and_the_room_state() {
        let tick = LinkEvent::Countdown(0);
        let state = |v| LinkEvent::RoomSetting {
            kind: 7,
            value: v,
            text: String::new(),
        };
        // the host and a player ready at the start get state 2 before the first tick: locked by the tick
        assert!(start_lock_after(false, &tick, 2));
        // a player who was not ready (state 0) or only ready (1) stays unlocked
        assert!(!start_lock_after(false, &tick, 0));
        assert!(!start_lock_after(false, &tick, 1));
        // the room's starting state alone does not lock (2009 locks on the tick)
        assert!(!start_lock_after(false, &state(1), 2));
        // later ticks keep it; a host change or a member push does not end it
        assert!(start_lock_after(true, &LinkEvent::Countdown(3), 2));
        let host = LinkEvent::RoomSetting {
            kind: 2,
            value: 9,
            text: String::new(),
        };
        assert!(start_lock_after(true, &host, 2));
        assert!(start_lock_after(true, &state(1), 2));
        // the room in game ends it (2009), a room back to waiting too (CLI-1311)
        assert!(!start_lock_after(true, &state(2), 3));
        assert!(!start_lock_after(true, &state(0), 0));
    }

    #[test]
    fn only_the_chat_and_the_idle_timers_run_while_locked() {
        for c in [CMD_CHAT_SEND, CMD_IDLE_AWAY, CMD_IDLE_LEAVE] {
            assert!(runs_while_locked(c), "{c}");
        }
        for c in [
            CMD_EXIT,
            CMD_START,
            CMD_READY,
            CMD_TEAM,
            CMD_MAP,
            CMD_MODE,
            CMD_GOAL,
            CMD_PLAYERS,
            CMD_ARMS,
            0x3d,
        ] {
            assert!(!runs_while_locked(c), "{c}");
        }
    }

    #[test]
    fn member_states() {
        assert_eq!(member_state_text(0, 0), None);
        assert_eq!(member_state_text(1, 0), Some(STATE_READY));
        assert_eq!(member_state_text(3, 2), Some(STATE_IN_GAME));
        // the sub-states show only while the member waits (m62)
        assert_eq!(member_state_text(0, 1), Some(SUB_SHOPPING));
        assert_eq!(member_state_text(0, 2), Some(SUB_AWAY));
        assert_eq!(member_state_text(1, 2), Some(STATE_READY));
    }

    #[test]
    fn idle_rules_flag_away_after_a_minute_and_leave_three_minutes_after_the_last_input() {
        let cat = Catalog::fallback(None, 1024, 768);
        let data = MenuData {
            idle_rules: true,
            ..MenuData::default()
        };
        let mut r = Room::new(&cat, &data);
        let t0 = Instant::now();
        r.set_last_input(t0);
        assert_eq!(r.idle_due(t0 + Duration::from_secs(59)), None);
        assert_eq!(
            r.idle_due(t0 + Duration::from_secs(61)),
            Some(CMD_IDLE_AWAY)
        );
        // the away flag goes out at 61 s and restarts the idle clock (cshell+0x1608b0)
        r.mark_away(t0 + Duration::from_secs(61));
        assert_eq!(r.idle_due(t0 + Duration::from_secs(90)), None, "once");
        assert_eq!(
            r.idle_due(t0 + Duration::from_secs(121)),
            None,
            "no leave two minutes after the last input"
        );
        assert_eq!(r.idle_due(t0 + Duration::from_secs(180)), None);
        assert_eq!(
            r.idle_due(t0 + Duration::from_secs(182)),
            Some(CMD_IDLE_LEAVE)
        );
        r.idle_left = true;
        assert_eq!(r.idle_due(t0 + Duration::from_secs(300)), None, "once");
        // scripted drivers: no idle rules
        let off = Room::new(&cat, &MenuData::default());
        assert_eq!(
            off.idle_due(Instant::now() + Duration::from_secs(300)),
            None
        );
    }

    /// LG-10: a free-for-all room lists every member once in id order over both columns, whatever the lobby's team
    /// values; a team room splits by team.
    #[test]
    fn ffa_room_lists_members_in_one_list_over_both_columns() {
        let members = [(1001, 1), (1002, 1), (1003, 2), (1004, 1), (1005, 2)];
        let ffa = member_slots(members.iter().copied(), true, 3, 0);
        assert_eq!(
            ffa,
            vec![
                (1001, 1, 0),
                (1002, 1, 1),
                (1003, 1, 2),
                (1004, 2, 0),
                (1005, 2, 1)
            ]
        );
        let tdm = member_slots(members.iter().copied(), false, 3, 1003);
        assert_eq!(
            tdm,
            vec![
                (1001, 1, 0),
                (1002, 1, 1),
                (1003, 2, 0),
                (1004, 1, 2),
                (1005, 2, 1)
            ]
        );
    }

    /// CLI-932: the master is the first row of a free-for-all list, whichever slot it joined in.
    #[test]
    fn quarantine_and_free_for_all_rooms_use_one_list() {
        assert!(one_list_class(Some(6)));
        assert!(
            one_list_class(Some(11)),
            "Quarantine: no teams in the waiting room"
        );
        assert!(one_list_class(Some(9)), "Spy Hunt: one list like One Man Army");
        assert!(!one_list_class(Some(1)));
        assert!(!one_list_class(Some(7)));
        assert!(!one_list_class(None));
    }

    #[test]
    fn ffa_master_joining_second_is_listed_first() {
        let members = [(1001, 1), (1002, 1), (1003, 2)];
        let ffa = member_slots(members.iter().copied(), true, 2, 1002);
        assert_eq!(ffa, vec![(1002, 1, 0), (1001, 1, 1), (1003, 2, 0)]);
        // a team room keeps its columns
        let tdm = member_slots(members.iter().copied(), false, 2, 1002);
        assert_eq!(tdm, vec![(1001, 1, 0), (1002, 1, 1), (1003, 2, 0)]);
    }

    /// ROM-free: the member row rule puts the name and state at the row plus their style offsets, name left /
    /// centred, state centred both ways, font 14 size 12, keyed by team, row and the style key.
    #[test]
    fn member_row_wants_follow_the_rule() {
        let st = MemberStyle {
            bars: [None, None],
            bar: [338, 28],
            level: None,
            level_size: [30, 22],
            level_at: [10, 0],
            master: None,
            master_size: [42, 10],
            master_at: [0, -5],
            bar_at: [0, -5],
            name_at: [44, 9],
            state_at: [299, 9],
            clan_name_at: None,
            clan_mark_at: None,
        };
        let [name, state] = member_row_wants(&st, 2, 3, [100, 200]);
        assert_eq!(
            name.key,
            format!("dyn/member2_3/{}", row_keys::NAME_AT.sha16)
        );
        assert_eq!(
            state.key,
            format!("dyn/member2_3/{}", row_keys::STATE_AT.sha16)
        );
        assert_eq!(name.text_at, [144.0, 209.0]);
        assert_eq!(state.text_at, [399.0, 209.0]);
        assert_eq!(name.text_anchor, [0.0, 0.5]);
        assert_eq!(state.text_anchor, [0.5, 0.5]);
        for w in [&name, &state] {
            assert_eq!(w.font, Some(MEMBER_FONT));
            assert_eq!(w.font_size, Some(MEMBER_TEXT_SIZE));
        }
        assert_eq!((MEMBER_FONT, MEMBER_TEXT_SIZE), (14, 12));
    }
}
