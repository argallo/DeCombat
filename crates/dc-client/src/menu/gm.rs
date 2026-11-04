//! gm-client: the 2009 operator (GM) privileges of the client (CLI-1300..CLI-1306).
//!
//! The 2009 shell has two GM paths:
//! - the **operator flag** of the login reply (0x201 +0x26 == 2): slash commands in the chat box (0x3d0 / 0x3d2 /
//!   0x3d4), `/k <name>` in the waiting room, the "Kick User" entry of the member menu, the operator slots of full
//!   servers and channels, no repeat count in the chat, friend requests answered at once;
//! - the **"GM" name prefix**: the GM rank icon, the GM chat colour, local kick immunity.
//!
//! The command words and the mute confirmations are code literals of the user's CShell.dll ([`GmLiterals`]); nothing
//! of them is in this tree.
// EVIDENCE: cshell+0x42450 (0x201 +0x26 -> shell +4; value 2 -> the operator flag byte, shell +0, set), cshell+0x42920
// (slash commands), cshell+0x160c90 (`/k`), cshell+0x15dde0 (member menu), cshell+0x1604b0 (kick pre-check),
// cshell+0x160340 (kick reply), cshell+0x139d70 / cshell+0xfab60 (operator slots), cshell+0x20ed0 (repeat count),
// cshell+0xed4b0 (friend request)

use super::catalog::read_pinned_wide;
use dc_botclient::lobby::ServerEntry;
use dc_rom::PeFile;
use dc_ui_data::names::{read_pinned, Pin};
use dc_wire::{PktReader, PktWriter, OP_LOOKUP, OP_MUTE, OP_RESTRICT, ROOM_KICK};
use std::collections::VecDeque;

/// The login privilege value of an operator.
// EVIDENCE: cshell+0x42450 (`*(shell + 4) == 2` sets the operator flag)
pub const OPERATOR_PRIVILEGE: i32 = 2;

/// CLI-1300: the operator flag of a login privilege word.
pub fn is_operator(privilege: i32) -> bool {
    privilege == OPERATOR_PRIVILEGE
}

/// The name prefix of a GM character.
// EVIDENCE: cshell+0x1604b0, cshell+0x268300, cshell+0x2683f0 (wcsncmp with the two-unit literal L"GM")
pub const GM_PREFIX: &str = "GM";

/// A name with the GM prefix.
pub fn gm_named(name: &str) -> bool {
    name.starts_with(GM_PREFIX)
}

/// The operator state of the session: the flag and the requests waiting for their reply (oldest first).
// EVIDENCE: cshell+0x3e520 / cshell+0x3e5b0 (requests appended to two pending lists), cshell+0x3e0f0 / cshell+0x3e300
// (each reply takes the oldest entry of its list)
#[derive(Debug, Clone, Default)]
pub struct GmState {
    pub operator: bool,
    pub restricts: VecDeque<Restrict>,
    pub mutes: VecDeque<Mute>,
}

/// A sent restrict request (0x3d0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restrict {
    pub name: String,
    pub action: u32,
    pub period: i32,
    pub reason: String,
}

/// A sent mute request (0x3d4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mute {
    pub name: String,
    pub on: bool,
    pub minutes: i32,
}

/// The six command words, in the order the 2009 parser tries them: kick (restrict 1), ban (restrict 2), unban
/// (restrict 0), trace (lookup), punish (mute), unpunish (unmute).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpWords {
    pub kick: String,
    pub ban: String,
    pub unban: String,
    pub trace: String,
    pub punish: String,
    pub unpunish: String,
}

/// The pins of the command words (narrow literals) and the two mute confirmations (wide literals).
// EVIDENCE: cshell+0x42920 (the six strncmp pointers at VA 0x37542490 / 0x3754248c / 88 / 84 / 80 / 7c), cshell+0x3e300
// (swprintf of the wide literals VA 0x374cfb10 (unmuted: name) and VA 0x374cfb60 (muted: name, the -t number))
const WORD_PINS: [Pin; 6] = [
    Pin {
        what: "operator word 1 (restrict 1)",
        rva: 0x3d05c0,
        sha16: "0db10f2c2f332cd2",
        len: 4,
    },
    Pin {
        what: "operator word 2 (restrict 2)",
        rva: 0x3d05bc,
        sha16: "b2a96c3d3fc2b6ac",
        len: 3,
    },
    Pin {
        what: "operator word 3 (restrict 0)",
        rva: 0x3d05b4,
        sha16: "de73c984dc0d9850",
        len: 5,
    },
    Pin {
        what: "operator word 4 (lookup)",
        rva: 0x3d05ac,
        sha16: "eafe895eb8119e6e",
        len: 5,
    },
    Pin {
        what: "operator word 5 (mute)",
        rva: 0x3d05a4,
        sha16: "0d2ca3afc299c9ad",
        len: 6,
    },
    Pin {
        what: "operator word 6 (unmute)",
        rva: 0x3d0598,
        sha16: "a72432841eaf9d52",
        len: 8,
    },
];
const MUTE_FORM_PINS: [Pin; 2] = [
    Pin {
        what: "unmute confirmation (wide)",
        rva: 0x3cfb10,
        sha16: "17f33edc0069dc56",
        len: 37,
    },
    Pin {
        what: "mute confirmation (wide)",
        rva: 0x3cfb60,
        sha16: "f61c2c15fc5f8f20",
        len: 40,
    },
];

/// The operator literals of the user's CShell.dll (`None`: absent or not the 2009 build).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GmLiterals {
    pub words: Option<OpWords>,
    /// The confirmations of 0x3d5: unmuted (`%s`), muted (`%s`, `%d`).
    pub mute_forms: [Option<String>; 2],
}

impl GmLiterals {
    pub fn from_cshell(pe: &PeFile) -> GmLiterals {
        let w: Option<Vec<String>> = WORD_PINS.iter().map(|p| read_pinned(pe, p).ok()).collect();
        GmLiterals {
            words: w.map(|w| OpWords {
                kick: w[0].clone(),
                ban: w[1].clone(),
                unban: w[2].clone(),
                trace: w[3].clone(),
                punish: w[4].clone(),
                unpunish: w[5].clone(),
            }),
            mute_forms: MUTE_FORM_PINS.map(|p| read_pinned_wide(pe, &p)),
        }
    }
}

/// The lookup selector of 0x3d2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpQuery {
    /// Selector 0: by account number.
    Account(u32),
    /// Selector 1: by login id.
    Login(String),
    /// Selector 2: by character name.
    Name(String),
}

/// What an operator's slash line comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpCommand {
    /// 0x3d0.
    Restrict(Restrict),
    /// 0x3d2.
    Lookup(OpQuery),
    /// 0x3d4.
    Mute(Mute),
    /// The line is taken (not sent as chat) and nothing goes out: an unknown word or a missing name.
    Nothing,
}

/// The first quoted text from `s`, at most `cap - 1` characters (empty without a quote or with nothing after it).
// EVIDENCE: cshell+0x136160 (skip to the first '"', copy up to the next '"', the end or cap - 1 characters)
fn quoted(s: &str, cap: usize) -> String {
    let Some(i) = s.find('"') else {
        return String::new();
    };
    s[i + 1..]
        .chars()
        .take_while(|c| *c != '"')
        .take(cap.saturating_sub(1))
        .collect()
}

/// The value of a switch: the characters from `skip` past the switch's start up to a space or the end.
// EVIDENCE: cshell+0x42920 (`-p`: from the switch + 3), cshell+0x40790 (`-t`: + 3), cshell+0x40980 (`-uid`: + 5)
fn switch_value<'a>(s: &'a str, switch: &str, skip: usize) -> Option<&'a str> {
    let i = s.find(switch)?;
    let rest = s.get(i + skip..).unwrap_or("");
    Some(rest.split(' ').next().unwrap_or(""))
}

/// C `atoi`: leading blanks, a sign, then digits (0 without any).
fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, digits) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let n = digits
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .fold(0i64, |n, c| (n * 10 + i64::from(c as u8 - b'0')).min(i64::from(i32::MAX) + 1));
    let n = if neg { -n } else { n };
    n.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Name field width (characters including the end) of the operator requests.
pub const NAME_CAP: usize = 0x11;
/// Reason field width of 0x3d0.
pub const REASON_CAP: usize = 0x41;

/// CLI-1301: an operator's chat line as a command; `None` when the line does not start with `/` or its first word is
/// none of the six (it is chat). The words are tried as prefixes in the 2009 order; `words` `None` (no 2009 literals)
/// leaves every line chat.
// DC-DEFAULT: CLI-1301 (amended) the 2009 client takes an unknown slash word without sending anything; DeCombat sends
// it as chat so the lobby's own chat-text commands (/notice, /expire, /shopver, /grade; OPS-1 / OPS-2) work for an operator
// EVIDENCE: cshell+0x42920 (operator flag and a leading '/' or not a command; strncmp of the text after '/' with
// kick -> 0x3d0 action 1, ban -> 2, unban -> 0, trace -> cshell+0x40980, punish -> cshell+0x40790 on 1, unpunish ->
// on 0, anything else taken without a request; 0x3d0: the quoted name (16), `-r` -> the quoted reason after it (64),
// `-p` -> atoi of the word 3 characters on, sent only with a name), cshell+0x40980 (`-uid` -> selector 0 with atoi of
// the word 5 characters on; else `-nxid` -> selector 1 with the quoted id after it; else the quoted name -> selector
// 2; none -> nothing), cshell+0x40790 (quoted name, `-t` -> atoi of the word 3 characters on, sent only with a name)
pub fn parse_command(line: &str, words: Option<&OpWords>) -> Option<OpCommand> {
    let after = line.strip_prefix('/')?;
    let w = words?;
    let restrict = |action: u32| {
        let name = quoted(after, NAME_CAP);
        if name.is_empty() {
            return OpCommand::Nothing;
        }
        let reason = line
            .find("-r")
            .map(|i| quoted(&line[i..], REASON_CAP))
            .unwrap_or_default();
        let period = switch_value(line, "-p", 3).map_or(0, atoi);
        OpCommand::Restrict(Restrict {
            name,
            action,
            period,
            reason,
        })
    };
    let mute = |on: bool| {
        let name = quoted(line, NAME_CAP);
        if name.is_empty() {
            return OpCommand::Nothing;
        }
        let minutes = switch_value(line, "-t", 3)
            .filter(|v| !v.is_empty())
            .map_or(0, atoi);
        OpCommand::Mute(Mute { name, on, minutes })
    };
    let starts = |word: &str| !word.is_empty() && after.starts_with(word);
    if ![&w.kick, &w.ban, &w.unban, &w.trace, &w.punish, &w.unpunish]
        .into_iter()
        .any(|x| starts(x))
    {
        return None;
    }
    Some(if starts(&w.kick) {
        restrict(1)
    } else if starts(&w.ban) {
        restrict(2)
    } else if starts(&w.unban) {
        restrict(0)
    } else if starts(&w.trace) {
        if let Some(v) = switch_value(line, "-uid", 5) {
            if v.is_empty() {
                OpCommand::Nothing
            } else {
                OpCommand::Lookup(OpQuery::Account(atoi(v) as u32))
            }
        } else if let Some(i) = line.find("-nxid") {
            let id = quoted(&line[i..], NAME_CAP);
            if id.is_empty() {
                OpCommand::Nothing
            } else {
                OpCommand::Lookup(OpQuery::Login(id))
            }
        } else {
            let name = quoted(line, NAME_CAP);
            if name.is_empty() {
                OpCommand::Nothing
            } else {
                OpCommand::Lookup(OpQuery::Name(name))
            }
        }
    } else if starts(&w.punish) {
        mute(true)
    } else if starts(&w.unpunish) {
        mute(false)
    } else {
        OpCommand::Nothing
    })
}

/// 0x3d0 (0xb2 bytes).
// SPEC: PROTOCOL_2009.md §6.4 (0x3d0: +6 wstr[17] name, +0x28 u32 action, +0x2c i32 period, +0x30 wstr[65] reason)
pub fn restrict_packet(name: &str, action: u32, period: i32, reason: &str) -> PktWriter {
    let mut w = PktWriter::new(OP_RESTRICT);
    w.wstr(name, NAME_CAP)
        .u32(action)
        .i32(period)
        .wstr(reason, REASON_CAP)
        .at(0xb2);
    w
}

/// 0x3d2 (0x2c bytes).
// SPEC: PROTOCOL_2009.md §6.4 (0x3d2: +6 i32 selector; +0xa u32 account number or wstr[17])
pub fn lookup_packet(q: &OpQuery) -> PktWriter {
    let mut w = PktWriter::new(OP_LOOKUP);
    match q {
        OpQuery::Account(n) => w.i32(0).u32(*n),
        OpQuery::Login(id) => w.i32(1).wstr(id, NAME_CAP),
        OpQuery::Name(n) => w.i32(2).wstr(n, NAME_CAP),
    };
    w.at(0x2c);
    w
}

/// 0x3d4 (0xb2 bytes).
// SPEC: PROTOCOL_2009.md §6.4 (0x3d4: +6 wstr[17] name, +0x28 u32 on, +0x2c i32 minutes, +0x30 zero bytes)
pub fn mute_packet(name: &str, on: bool, minutes: i32) -> PktWriter {
    let mut w = PktWriter::new(OP_MUTE);
    w.wstr(name, NAME_CAP).u32(on as u32).i32(minutes).at(0xb2);
    w
}

/// 0x38e (0x12 bytes) with the third word 0.
// SPEC: PROTOCOL_2009.md §3.7 (0x38e: +6 u32 in-channel id, +0xa u32 character number; cs+0x18f280 leaves +0xe)
pub fn kick_packet(id: u32, char_no: u32) -> PktWriter {
    let mut w = PktWriter::new(ROOM_KICK);
    w.u32(id).u32(char_no).u32(0).at(0x12);
    w
}

/// The 0x3d3 fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LookupView {
    pub ok: bool,
    pub reason: u16,
    pub account: u32,
    pub login: String,
    pub name: String,
    pub address: String,
    pub online: bool,
    pub server_kind: u32,
    pub server: i32,
    pub channel: i32,
    pub room: i32,
    pub playing: bool,
}

/// Reads 0x3d3.
// SPEC: PROTOCOL_2009.md §6.4 (0x3d3 layout)
pub fn lookup_view(d: &[u8]) -> LookupView {
    let r = |o: usize| PktReader::from_raw(d).at(o).u32();
    let s = |o: usize| PktReader::from_raw(d).at(o).wstr(NAME_CAP);
    LookupView {
        ok: r(6) == 1,
        reason: PktReader::from_raw(d).at(0xa).u16(),
        account: r(0xe),
        login: s(0x12),
        name: s(0x34),
        address: s(0x56),
        online: r(0x78) != 0,
        server_kind: r(0x7c),
        server: r(0x80) as i32,
        channel: r(0x84) as i32,
        room: r(0x88) as i32,
        playing: r(0x8c) != 0,
    }
}

/// Reply line colours: confirmations, failures.
// EVIDENCE: cshell+0x3e0f0 / cshell+0x3e300 / cshell+0x40dd0 (0xffffff00 confirmations, 0xffff0000 failures)
pub const REPLY_COLOUR: u32 = 0xffff_ff00;
pub const FAIL_COLOUR: u32 = 0xffff_0000;
/// String ids of the restrict confirmations (actions 0, 1, 2) and of the unknown target.
// EVIDENCE: cshell+0x3e0f0 (action 0 -> 0x180a (name, reason), 1 -> 0x180b (name, reason), 2 -> 0x180c (name, period,
// reason); failure with reason 0x5d -> 0x1809 (name)), cshell+0x3e300 (failure with reason 0x87 -> 0x1809)
pub const TEXT_RESTRICT: [i32; 3] = [0x180a, 0x180b, 0x180c];
pub const TEXT_NO_TARGET: i32 = 0x1809;
pub const RESTRICT_NO_TARGET: u16 = 0x5d;
pub const MUTE_NO_TARGET: u16 = 0x87;

/// One printf argument.
#[derive(Debug, Clone, Copy)]
pub enum Arg<'a> {
    S(&'a str),
    D(i32),
}

/// A printf form with `%s` / `%d` (and `%ls`, `%i`) conversions taken in order; `%%` is a percent sign.
pub fn printf(form: &str, args: &[Arg]) -> String {
    let mut out = String::new();
    let mut args = args.iter();
    let mut it = form.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if it.peek() == Some(&'%') {
            it.next();
            out.push('%');
            continue;
        }
        while it.peek().is_some_and(|c| matches!(c, 'l' | 'h' | 'w')) {
            it.next();
        }
        match (it.next(), args.next()) {
            (Some('s' | 'S'), Some(Arg::S(s))) => out.push_str(s),
            (Some('d' | 'i' | 'u'), Some(Arg::D(n))) => out.push_str(&n.to_string()),
            (Some(_), Some(Arg::S(s))) => out.push_str(s),
            (Some(_), Some(Arg::D(n))) => out.push_str(&n.to_string()),
            (Some(k), None) => {
                out.push('%');
                out.push(k);
            }
            (None, _) => out.push('%'),
        }
    }
    out
}

/// The chat line of a 0x3d1 for the oldest pending restrict `p` (`text` = the string table).
pub fn restrict_reply_line(
    text: impl Fn(i32) -> String,
    p: &Restrict,
    ok: bool,
    reason: u16,
) -> Option<(String, u32)> {
    if !ok {
        return (reason == RESTRICT_NO_TARGET).then(|| {
            (
                printf(&text(TEXT_NO_TARGET), &[Arg::S(&p.name)]),
                FAIL_COLOUR,
            )
        });
    }
    let line = match p.action {
        0 | 1 => printf(
            &text(TEXT_RESTRICT[p.action as usize]),
            &[Arg::S(&p.name), Arg::S(&p.reason)],
        ),
        2 => printf(
            &text(TEXT_RESTRICT[2]),
            &[Arg::S(&p.name), Arg::D(p.period), Arg::S(&p.reason)],
        ),
        _ => return None,
    };
    Some((line, REPLY_COLOUR))
}

/// The chat line of a 0x3d5 for the oldest pending mute `p` (`forms` = [`GmLiterals::mute_forms`]).
// DC-DEFAULT: CLI-1301 (proposed) without the 2009 confirmation literals the line is `name` (unmuted) or
// `name: number` (muted)
pub fn mute_reply_line(
    text: impl Fn(i32) -> String,
    forms: &[Option<String>; 2],
    p: &Mute,
    ok: bool,
    reason: u16,
) -> Option<(String, u32)> {
    if !ok {
        return (reason == MUTE_NO_TARGET).then(|| {
            (
                printf(&text(TEXT_NO_TARGET), &[Arg::S(&p.name)]),
                FAIL_COLOUR,
            )
        });
    }
    let line = match (p.on, &forms[p.on as usize]) {
        (false, Some(f)) => printf(f, &[Arg::S(&p.name)]),
        (true, Some(f)) => printf(f, &[Arg::S(&p.name), Arg::D(p.minutes)]),
        (false, None) => p.name.clone(),
        (true, None) => format!("{}: {}", p.name, p.minutes),
    };
    Some((line, REPLY_COLOUR))
}

/// The two report lines of a 0x3d3, or the failure line (`None`: silent).
// EVIDENCE: cshell+0x40dd0 (result 1 -> a title line and two report lines in 0xffffff00: name, login id, account,
// address, online; then server kind, server name (the client's list by index, -1 none), channel + 1, room + 1,
// playing; result 0 with reason 0x6e / 0x6f -> one line in 0xffff0000)
// DC-DEFAULT: CLI-1301 (proposed) the report's wording is DeCombat's (the 2009 English literals are not reproduced):
// the same fields in the same two lines, no title line, the server kind as its number
pub fn lookup_lines(v: &LookupView, servers: &[ServerEntry]) -> Vec<(String, u32)> {
    if !v.ok {
        let t = match v.reason {
            0x6e => "lookup: query not valid",
            0x6f => "lookup: no such user",
            _ => return Vec::new(),
        };
        return vec![(t.to_string(), FAIL_COLOUR)];
    }
    let none = "-".to_string();
    let addr = if v.address.is_empty() {
        none.clone()
    } else {
        v.address.clone()
    };
    let server = usize::try_from(v.server)
        .ok()
        .and_then(|i| servers.get(i))
        .map_or(none.clone(), |e| e.name.clone());
    let num = |n: i32| {
        if n < 0 {
            none.clone()
        } else {
            (n + 1).to_string()
        }
    };
    vec![
        (
            format!(
                "{} / {} / #{} / {} / {}",
                v.name,
                v.login,
                v.account,
                addr,
                if v.online { "online" } else { "offline" }
            ),
            REPLY_COLOUR,
        ),
        (
            format!(
                "server {} ({}) / channel {} / room {} / {}",
                server,
                v.server_kind,
                num(v.channel),
                num(v.room),
                if v.playing { "playing" } else { "not playing" }
            ),
            REPLY_COLOUR,
        ),
    ]
}

/// The waiting room's member menu strings: add friend, profile, and Kick User.
// EVIDENCE: cshell+0x15dde0 (entries 0x13f7 and 0x573; 0x1782 while the room's +0x56b0 is set; 0x480 when the shell's
// room-badge flag +0x424 and the room's +0x564c are set; 0x480 again when the operator flag is set)
pub const TEXT_ADD_FRIEND: i32 = 0x13f7;
pub const TEXT_PROFILE: i32 = 0x573;
pub const TEXT_KICK: i32 = 0x480;

/// CLI-1303: the member menu's entries for a player who is `operator`, is the room's `host`, holds the room badge
/// (`room_badge`, 0x303 +0xe8) in a room whose host badge level is `badge`.
// DC-DEFAULT: CLI-1303 (proposed) the room's +0x564c is read as the host's badge level (0x389 +0x88, 0x3035 type 0xf)
// and is taken only while the player is the host; Kick User is listed once when both rules hold; the 0x1782 entry
// (the room's +0x56b0, command 0x7532: hand the host over) is not modelled
pub fn member_menu_entries(operator: bool, host: bool, room_badge: bool, badge: i32) -> Vec<i32> {
    let mut v = vec![TEXT_ADD_FRIEND, TEXT_PROFILE];
    if operator || (host && room_badge && badge != 0) {
        v.push(TEXT_KICK);
    }
    v
}

/// The waiting room's open member menu: its top-left, the member's in-channel id and the entries' strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberMenu {
    pub at: [i32; 2],
    pub id: u32,
    pub entries: Vec<i32>,
}

impl MemberMenu {
    /// The rectangle of entry `i` (x, y, w, h), the room list's user menu look (CLI-472).
    pub fn entry_rect(&self, i: usize) -> [i32; 4] {
        use super::lobby::{USER_MENU_ENTRY, USER_MENU_PAD};
        [
            self.at[0] + USER_MENU_PAD,
            self.at[1] + USER_MENU_PAD + i as i32 * USER_MENU_ENTRY[1],
            USER_MENU_ENTRY[0],
            USER_MENU_ENTRY[1],
        ]
    }

    /// The whole box.
    pub fn rect(&self) -> [i32; 4] {
        use super::lobby::{USER_MENU_ENTRY, USER_MENU_PAD};
        [
            self.at[0],
            self.at[1],
            USER_MENU_ENTRY[0] + 2 * USER_MENU_PAD,
            self.entries.len() as i32 * USER_MENU_ENTRY[1] + 2 * USER_MENU_PAD,
        ]
    }

    /// The entry string under `p`.
    pub fn entry_at(&self, p: [f32; 2]) -> Option<i32> {
        (0..self.entries.len())
            .find(|&i| {
                let r = self.entry_rect(i);
                p[0] >= r[0] as f32
                    && p[1] >= r[1] as f32
                    && p[0] < (r[0] + r[2]) as f32
                    && p[1] < (r[1] + r[3]) as f32
            })
            .map(|i| self.entries[i])
    }

    /// The box, the entry under the pointer lit, the entries' strings (the room list's user menu look).
    pub fn draw(
        &self,
        cat: &super::catalog::Catalog,
        hover: Option<i32>,
        out: &mut Vec<crate::hud::items::DrawItem>,
    ) {
        use crate::hud::items::DrawItem;
        let f = |r: [i32; 4]| [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32];
        out.push(DrawItem::Fill {
            rect: f(self.rect()),
            colour: [16, 16, 16, 235],
        });
        for (i, id) in self.entries.iter().enumerate() {
            let e = self.entry_rect(i);
            if hover == Some(*id) {
                out.push(DrawItem::Fill {
                    rect: f(e),
                    colour: [90, 70, 20, 255],
                });
            }
            out.push(super::sheet::place_text(
                &cat.text(*id),
                [e[0] + 6, e[1]],
                [0, e[3]],
                dc_ui_data::pages::Align {
                    first: super::sheet::ALIGN_LEFT,
                    second: dc_ui_data::pages::ALIGN_SECOND_CENTRE,
                },
                14,
                13,
                super::lobby::ROW_COLOUR,
            ));
        }
    }
}

/// The kick confirmation (`%s` = the name), the kick's success line (`%s`) and the refusals.
// EVIDENCE: cshell+0x164810 (command 0x7533: cshell+0x1604b0, then a yes / no box of 0x1848 with the member's name),
// cshell+0x160340 (0x38f ok -> 0x1849 with the stored name; reasons 0x7e -> 0x1850, 0x7f -> 0x184f, 0x80 -> 0x184e,
// 0x81 -> 0x1846, 0x82 -> 0x1845, 0x83 -> 0x1844, others 0x1847)
pub const TEXT_KICK_ASK: i32 = 0x1848;
pub const TEXT_KICKED: i32 = 0x1849;
pub const TEXT_KICK_BUSY: i32 = 0x1844;
pub const TEXT_KICK_GM: i32 = 0x1845;

/// The string of a refused 0x38f.
pub fn kick_refusal_text(reason: u32) -> i32 {
    match reason {
        0x7e => 0x1850,
        0x7f => 0x184f,
        0x80 => 0x184e,
        0x81 => 0x1846,
        0x82 => TEXT_KICK_GM,
        0x83 => TEXT_KICK_BUSY,
        _ => 0x1847,
    }
}

/// CLI-1306: the local pre-check of a kick from the member menu: a target starting or in a match is refused with
/// 0x1844, a GM-named target with 0x1845 (no request goes out); `None` lets the kick go on.
// EVIDENCE: cshell+0x1604b0 (member state 3 or 2 -> 0x1844; name starting with L"GM" -> 0x1845; both in a message box)
pub fn kick_precheck(state: i32, name: &str) -> Option<i32> {
    if state == 2 || state == 3 {
        Some(TEXT_KICK_BUSY)
    } else if gm_named(name) {
        Some(TEXT_KICK_GM)
    } else {
        None
    }
}

/// The `/k` prompt: its string and the two trigger texts.
// EVIDENCE: cshell+0x160c90 (key 0x20 with the operator flag and the field exactly L"/k " or L"/K " -> the field
// becomes string 0x576 and the prompt flag is set)
pub const TEXT_KICK_PROMPT: i32 = 0x576;
const KICK_TRIGGERS: [&str; 2] = ["/k ", "/K "];

/// The `/k` refusals: no member of that name, the member is in a match (member state 3).
// EVIDENCE: cshell+0x15fef0 (clean tree: 0x57c when no row has the name, 0x57b for a member state 3)
pub const TEXT_NO_MEMBER: i32 = 0x57c;
pub const TEXT_MEMBER_IN_MATCH: i32 = 0x57b;
pub const MEMBER_IN_MATCH: i32 = 3;

/// CLI-1302: the typed field turns into the kick prompt.
pub fn kick_trigger(operator: bool, text: &str) -> bool {
    operator && KICK_TRIGGERS.contains(&text)
}

/// CLI-1302: Backspace with the prompt up: `true` = the field is cleared and the prompt ends (the text is already
/// shorter than the prompt).
// EVIDENCE: cshell+0x160c90 (key 8 with the prompt flag: field shorter than string 0x576 -> clear, flag off, key taken)
pub fn kick_prompt_backspace(text: &str, prompt: &str) -> bool {
    text.chars().count() < prompt.chars().count()
}

/// CLI-1302: the name typed after the prompt (the first space-separated word).
// EVIDENCE: cshell+0x160c90 (key 0xd with the prompt flag: the field from the prompt's length on, wcstok by " ", the
// first token to cshell+0x15fef0; the field is cleared and the flag ends)
pub fn kick_prompt_name(text: &str, prompt: &str) -> Option<String> {
    let n = prompt.chars().count();
    let rest: String = text.chars().skip(n).collect();
    rest.split(' ')
        .find(|t| !t.is_empty())
        .map(str::to_string)
}

/// CLI-1304: a server of the list can be entered: an operator by the operator slots (open, operator in use below
/// operator capacity), anyone else by the player slots.
// EVIDENCE: cshell+0x139d70 (operator: +0x8c open and +0x9c < +0x98, no load, level, kill-ratio or paid-grade check)
// DC-DEFAULT: CLI-1304 (proposed) a server without operator slots (capacity 0) takes an operator by the player rule
pub fn server_open_to(e: &ServerEntry, operator: bool) -> bool {
    if operator && e.op_capacity > 0 {
        e.open != 0 && e.op_current < e.op_capacity
    } else {
        e.open != 0 && e.current < e.capacity
    }
}

/// CLI-1304: a channel is full for this player (operator slots for an operator).
// EVIDENCE: cshell+0xfab60 (player: capacity +0x4a <= in use +0x4e -> full; operator: +0x52 <= +0x56 -> full)
// DC-DEFAULT: CLI-1304 (proposed) a channel without operator slots (capacity 0) takes an operator by the player rule
pub fn channel_full_for(c: &super::link::ChannelRow, operator: bool) -> bool {
    if operator && c.op_capacity > 0 {
        c.op_capacity <= c.op_current
    } else {
        c.capacity <= c.current
    }
}

/// The operator events on any screen: the flag of the login, the replies as chat lines (the lobby and the waiting room
/// share the chat lines).
// EVIDENCE: cshell+0x3e0f0 / cshell+0x3e300 / cshell+0x40dd0 (a reply line goes to the current chat page through its
// vtable +0x140 with the colour; in a match to the message lines instead)
pub fn shared_event(d: &mut super::MenuData, cat: &super::catalog::Catalog, e: &super::link::LinkEvent) {
    use super::link::LinkEvent;
    let text = |id: i32| cat.text(id);
    let line = match e {
        LinkEvent::Operator(on) => {
            d.gm = GmState {
                operator: *on,
                ..GmState::default()
            };
            None
        }
        LinkEvent::OpRestrictReply { ok, reason } => d
            .gm
            .restricts
            .pop_front()
            .and_then(|p| restrict_reply_line(text, &p, *ok, *reason)),
        LinkEvent::OpMuteReply { ok, reason } => d
            .gm
            .mutes
            .pop_front()
            .and_then(|p| mute_reply_line(text, &cat.paths.gm.mute_forms, &p, *ok, *reason)),
        LinkEvent::OpLookupReply(v) => {
            for (l, c) in lookup_lines(v, &d.servers) {
                d.say_in(l, c);
            }
            None
        }
        _ => None,
    };
    if let Some((l, c)) = line {
        d.say_in(l, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words() -> OpWords {
        // synthetic words (the 2009 ones are read from the user's CShell.dll)
        OpWords {
            kick: "w1".into(),
            ban: "w2".into(),
            unban: "w3".into(),
            trace: "w4".into(),
            punish: "w5".into(),
            unpunish: "w6".into(),
        }
    }

    #[test]
    fn operator_flag_is_privilege_two() {
        assert!(is_operator(2));
        assert!(!is_operator(0));
        assert!(!is_operator(1));
        assert!(gm_named("GMcasey"));
        assert!(!gm_named("gm") && !gm_named("xGM"));
    }

    #[test]
    fn slash_lines_parse_as_the_2009_commands() {
        let w = words();
        assert_eq!(parse_command("hello", Some(&w)), None);
        assert_eq!(parse_command("/w9 x", Some(&w)), None, "an unknown word is chat");
        assert_eq!(parse_command("/notice hi", Some(&w)), None);
        assert_eq!(parse_command("/anything", None), None);
        assert_eq!(
            parse_command("/w2 \"Bob\" -p 7 -r \"cheating\"", Some(&w)),
            Some(OpCommand::Restrict(Restrict {
                name: "Bob".into(),
                action: 2,
                period: 7,
                reason: "cheating".into()
            }))
        );
        assert_eq!(
            parse_command("/w1 \"Bob\"", Some(&w)),
            Some(OpCommand::Restrict(Restrict {
                name: "Bob".into(),
                action: 1,
                period: 0,
                reason: String::new()
            }))
        );
        match parse_command("/w3 \"Bob\"", Some(&w)) {
            Some(OpCommand::Restrict(r)) => assert_eq!(r.action, 0),
            o => panic!("{o:?}"),
        }
        assert_eq!(parse_command("/w1 Bob", Some(&w)), Some(OpCommand::Nothing));
        assert_eq!(
            parse_command("/w4 -uid 1234", Some(&w)),
            Some(OpCommand::Lookup(OpQuery::Account(1234)))
        );
        assert_eq!(
            parse_command("/w4 -nxid \"login1\"", Some(&w)),
            Some(OpCommand::Lookup(OpQuery::Login("login1".into())))
        );
        assert_eq!(
            parse_command("/w4 \"Bob\"", Some(&w)),
            Some(OpCommand::Lookup(OpQuery::Name("Bob".into())))
        );
        assert_eq!(parse_command("/w4", Some(&w)), Some(OpCommand::Nothing));
        assert_eq!(
            parse_command("/w5 \"Bob\" -t 30", Some(&w)),
            Some(OpCommand::Mute(Mute {
                name: "Bob".into(),
                on: true,
                minutes: 30
            }))
        );
        assert_eq!(
            parse_command("/w6 \"Bob\"", Some(&w)),
            Some(OpCommand::Mute(Mute {
                name: "Bob".into(),
                on: false,
                minutes: 0
            }))
        );
        // names are cut at 16 characters
        match parse_command("/w1 \"ABCDEFGHIJKLMNOPQRST\"", Some(&w)) {
            Some(OpCommand::Restrict(r)) => assert_eq!(r.name, "ABCDEFGHIJKLMNOP"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn packets_have_the_spec_layout() {
        let p = restrict_packet("Bob", 2, 7, "why").payload();
        assert_eq!(p.len(), 0xb2);
        let r = |o: usize| PktReader::from_raw(&p).at(o).u32();
        assert_eq!(PktReader::from_raw(&p).at(6).wstr(17), "Bob");
        assert_eq!((r(0x28), r(0x2c)), (2, 7));
        assert_eq!(PktReader::from_raw(&p).at(0x30).wstr(65), "why");
        let p = lookup_packet(&OpQuery::Account(99)).payload();
        assert_eq!(p.len(), 0x2c);
        assert_eq!(PktReader::from_raw(&p).at(6).u32(), 0);
        assert_eq!(PktReader::from_raw(&p).at(0xa).u32(), 99);
        let p = lookup_packet(&OpQuery::Name("Al".into())).payload();
        assert_eq!(PktReader::from_raw(&p).at(6).u32(), 2);
        assert_eq!(PktReader::from_raw(&p).at(0xa).wstr(17), "Al");
        let p = mute_packet("Bob", true, 30).payload();
        assert_eq!(p.len(), 0xb2);
        assert_eq!(PktReader::from_raw(&p).at(0x28).u32(), 1);
        assert_eq!(PktReader::from_raw(&p).at(0x2c).u32(), 30);
        let p = kick_packet(5, 77).payload();
        assert_eq!(p.len(), 0x12);
        assert_eq!(PktReader::from_raw(&p).at(6).u32(), 5);
        assert_eq!(PktReader::from_raw(&p).at(0xa).u32(), 77);
    }

    #[test]
    fn replies_word_the_oldest_request() {
        let text = |id: i32| match id {
            0x180c => "%s banned %d days: %s".to_string(),
            0x1809 => "%s unknown".to_string(),
            _ => format!("[{id:x}] %s %s"),
        };
        let p = Restrict {
            name: "Bob".into(),
            action: 2,
            period: 3,
            reason: "x".into(),
        };
        assert_eq!(
            restrict_reply_line(text, &p, true, 0),
            Some(("Bob banned 3 days: x".into(), REPLY_COLOUR))
        );
        assert_eq!(
            restrict_reply_line(text, &p, false, 0x5d),
            Some(("Bob unknown".into(), FAIL_COLOUR))
        );
        assert_eq!(restrict_reply_line(text, &p, false, 1), None);
        let m = Mute {
            name: "Bob".into(),
            on: true,
            minutes: 2,
        };
        let forms = [Some("%s free".to_string()), Some("%s for %d".to_string())];
        assert_eq!(
            mute_reply_line(text, &forms, &m, true, 0).unwrap().0,
            "Bob for 2"
        );
        assert_eq!(
            mute_reply_line(text, &forms, &m, false, 0x87).unwrap().1,
            FAIL_COLOUR
        );
        let v = LookupView {
            ok: true,
            name: "Bob".into(),
            channel: 0,
            room: -1,
            server: -1,
            ..LookupView::default()
        };
        let l = lookup_lines(&v, &[]);
        assert_eq!(l.len(), 2);
        assert!(l[1].0.contains("channel 1") && l[1].0.contains("room -"));
        let v = LookupView {
            reason: 0x6f,
            ..LookupView::default()
        };
        assert_eq!(lookup_lines(&v, &[])[0].1, FAIL_COLOUR);
    }

    #[test]
    fn kick_entry_prompt_and_checks() {
        assert_eq!(member_menu_entries(false, true, false, 1).len(), 2);
        assert_eq!(member_menu_entries(false, true, true, 0).len(), 2);
        assert_eq!(member_menu_entries(false, false, true, 1).len(), 2);
        assert_eq!(
            member_menu_entries(false, true, true, 1).last(),
            Some(&TEXT_KICK)
        );
        assert_eq!(
            member_menu_entries(true, false, false, 0).last(),
            Some(&TEXT_KICK)
        );
        assert!(kick_trigger(true, "/k ") && kick_trigger(true, "/K "));
        assert!(!kick_trigger(false, "/k ") && !kick_trigger(true, "/k"));
        let prompt = "Kick: ";
        assert_eq!(
            kick_prompt_name("Kick: Bob more", prompt).as_deref(),
            Some("Bob")
        );
        assert_eq!(kick_prompt_name("Kick: ", prompt), None);
        assert!(!kick_prompt_backspace("Kick: ", prompt));
        assert!(kick_prompt_backspace("Kick:", prompt));
        assert_eq!(kick_precheck(0, "GMx"), Some(TEXT_KICK_GM));
        assert_eq!(kick_precheck(3, "Bob"), Some(TEXT_KICK_BUSY));
        assert_eq!(kick_precheck(1, "Bob"), None);
        assert_eq!(kick_refusal_text(0x82), 0x1845);
        assert_eq!(kick_refusal_text(1), 0x1847);
    }

    #[test]
    fn operator_slots_decide_entry() {
        let e = ServerEntry {
            id: 1,
            name: "s".into(),
            ip: String::new(),
            port: 0,
            open: 1,
            capacity: 10,
            current: 10,
            clan_only: 0,
            op_capacity: 2,
            op_current: 1,
        };
        assert!(!server_open_to(&e, false));
        assert!(server_open_to(&e, true));
        let e2 = ServerEntry {
            op_current: 2,
            ..e.clone()
        };
        assert!(!server_open_to(&e2, true));
        let e3 = ServerEntry {
            op_capacity: 0,
            op_current: 0,
            current: 3,
            ..e.clone()
        };
        assert!(server_open_to(&e3, true), "no operator slots: player rule");
        let c = super::super::link::ChannelRow {
            id: 1,
            name: "c".into(),
            open: 1,
            capacity: 5,
            current: 5,
            op_capacity: 1,
            op_current: 0,
        };
        assert!(channel_full_for(&c, false));
        assert!(!channel_full_for(&c, true));
    }

    #[test]
    fn printf_takes_arguments_in_order() {
        assert_eq!(
            printf("%s: %d%% %ls", &[Arg::S("a"), Arg::D(5), Arg::S("b")]),
            "a: 5% b"
        );
        assert_eq!(printf("%s %s", &[Arg::S("a")]), "a %s");
    }
}
