//! Clans: membership (create, join, leave, kick, notice), the clan messages 0x3c0..0x3c9, the clan checks
//! 0x348..0x34d, the clan pushes 0x3061..0x3064, the clan lobby behind the clan-only server entry (channels, rooms
//! with two clan sides and a start minimum) and the clan match bookkeeping.
//!
//! The web pages that drive create / join / leave / manage are in `clanweb.rs` (CD-47); the clan item windows
//! (clan name, clan mark, clan record, clan colour) stay in `itemuse.rs` (CD-80) and use the same tables.
// SPEC: PROTOCOL_2009.md §5 (clan messages, clan web pages); the layouts the spec lists as LIVE-only are proven in
// the staged addendum lobby-clan_PROTOCOL_2009.md (0x3c3 / 0x3c5 / 0x3c7)

use crate::game::{MatchResult, PlayerResult};
use crate::itemuse::reason;
use crate::logs::Listener;
use crate::msg::*;
use crate::rewards::{self, RewardCtx};
use crate::room::RoomKey;
use crate::shop::account_of;
use crate::shopmsg::CLAN_INFO_STALE_PUSH;
use crate::state::{Core, Fault, HResult, Sid};
use crate::wire::{self, ChannelEntry};
use dc_bute::ClanRoomPolicy;
use dc_mod_lua::JsonValue;
use dc_wire::{PktReader, PktWriter};
use rusqlite::{params, Connection, OptionalExtension};

// ---------------------------------------------------------------------------------------------------------------
// message ids (2009 wire facts; names are this project's, SPEC: PROTOCOL_2009.md §5.1 and Appendix A)
// ---------------------------------------------------------------------------------------------------------------

pub const CLAN_LOGIN: u16 = 0x3c0;
pub const CLAN_LOGIN_REPLY: u16 = 0x3c1;
pub const CLAN_INFO: u16 = 0x3c2;
pub const CLAN_INFO_REPLY: u16 = 0x3c3;
pub const CLAN_MEMBERS: u16 = 0x3c4;
pub const CLAN_MEMBERS_REPLY: u16 = 0x3c5;
pub const CLAN_NOTICE: u16 = 0x3c6;
pub const CLAN_NOTICE_REPLY: u16 = 0x3c7;
pub const CLAN_NOTICE_SET: u16 = 0x3c8;
pub const CLAN_NOTICE_SET_REPLY: u16 = 0x3c9;
pub const CLAN_LEADER_CHECK: u16 = 0x348;
pub const CLAN_LEADER_CHECK_REPLY: u16 = 0x349;
pub const CLAN_NAME_CHECK: u16 = 0x34a;
pub const CLAN_NAME_CHECK_REPLY: u16 = 0x34b;
pub const CLAN_MARK_CHECK: u16 = 0x34c;
pub const CLAN_MARK_CHECK_REPLY: u16 = 0x34d;
pub const CLAN_MEMBERS_STALE_PUSH: u16 = 0x3062;
pub const CLAN_NOTICE_STALE_PUSH: u16 = 0x3063;
pub const CLAN_REMOVED_PUSH: u16 = 0x3064;

/// Member role 5 = clan leader: the member list marks the leader by it (EVIDENCE: cshell+0x111990 compares the
/// row role with 5 and 0xf), 0x3064 branches on it (EVIDENCE: cshell+0x42df0 case 0x3064), and 0x303 +0xac carries
/// it (SPEC: PROTOCOL_2009.md table 3.3a).
pub const ROLE_LEADER: i64 = 5;
/// Role of an ordinary member.
// DC-DEFAULT: CLN-7 (proposed) ordinary members carry role 1 (the client only tests for 5 / 0xf)
pub const ROLE_MEMBER: i64 = 1;

/// Channels of the clan lobby are kept apart from the normal lobby's channels (1..=64, `lobby.channels` is clamped
/// to 64): the clan lobby's channel n is stored as `CLAN_CHANNEL_BASE + n`, so its rooms and lobby pushes never mix
/// with the normal lobby's.
pub const CLAN_CHANNEL_BASE: u32 = 100;

/// Longest clan name and notice in UTF-16 units (the wire fields are wstr[17] and wstr[121]).
// SPEC: PROTOCOL_2009.md §5.1 0x34a (wstr[17]), 0x3c8 (wstr[121])
pub const NAME_UNITS: usize = 16;
pub const NOTICE_UNITS: usize = 120;

/// Most member rows one 0x3c5 can carry within the 0x8000-byte frame limit (SPEC: PROTOCOL_2009.md §1.2).
pub const MAX_MEMBER_ROWS: usize = (0x8000 - 0x12) / 0x32;

// ---------------------------------------------------------------------------------------------------------------
// rules (DC defaults, config keys `clan.*` in lobby.json)
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ClanRules {
    // DC-DEFAULT: CLN-1 (proposed) only clan members may enter a channel of the clan lobby
    pub channel_members_only: bool,
    // DC-DEFAULT: CLN-3 (proposed) a clan match starts only with two clans and at least N members per side
    pub start_min_per_side: u32,
    // DC-DEFAULT: CLN-5 (proposed) clan level thresholds (cumulative clan exp of levels 1..6; the ROM has no table)
    pub level_steps: Vec<i64>,
    // DC-DEFAULT: CLN-6 (proposed) member limit per clan level
    pub member_limits: Vec<u32>,
    // DC-DEFAULT: CLN-8 (proposed) web page actions need the account's lobby session online
    pub web_requires_session: bool,
    // DC-DEFAULT: CLN-9 (proposed) `!clan ...` chat commands (create / join / leave / kick)
    pub chat_commands: bool,
    // DC-DEFAULT: CD-11 clan room scoring policy (mode weights, fallback basic / winner points)
    pub policy: ClanRoomPolicy,
}

impl Default for ClanRules {
    fn default() -> Self {
        Self {
            channel_members_only: true,
            start_min_per_side: 2,
            level_steps: vec![0, 2_000, 8_000, 25_000, 70_000, 180_000],
            member_limits: vec![20, 40, 60, 80, 100, 120],
            web_requires_session: true,
            chat_commands: true,
            policy: ClanRoomPolicy::default(),
        }
    }
}

fn json_num(v: &JsonValue) -> Option<f64> {
    match v {
        JsonValue::Number(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}

impl ClanRules {
    pub fn owns(key: &str) -> bool {
        key.starts_with("clan.")
    }

    /// Applies one `clan.*` override; false for an unknown key or a bad value.
    pub fn apply(&mut self, key: &str, v: &JsonValue) -> bool {
        let n = json_num(v);
        let b = match v {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        };
        let nums = || -> Option<Vec<f64>> {
            match v {
                JsonValue::Array(a) if !a.is_empty() && a.len() <= 6 => {
                    a.iter().map(json_num).collect()
                }
                _ => None,
            }
        };
        if let Some(mode) = key.strip_prefix("clan.mode_weight.") {
            let Some(w) = n.filter(|w| *w >= 0.0) else {
                return false;
            };
            if mode.is_empty() {
                return false;
            }
            self.policy
                .mode_weights
                .retain(|(k, _)| !dc_bute::eq_fold(k, mode));
            self.policy.mode_weights.push((mode.to_string(), w));
            return true;
        }
        match key {
            "clan.channel_members_only" => b.map(|x| self.channel_members_only = x).is_some(),
            "clan.start_min_per_side" => n
                .map(|x| self.start_min_per_side = x.clamp(1.0, 8.0) as u32)
                .is_some(),
            "clan.web_requires_session" => b.map(|x| self.web_requires_session = x).is_some(),
            "clan.chat_commands" => b.map(|x| self.chat_commands = x).is_some(),
            "clan.basic_point_default" => n
                .map(|x| self.policy.basic_point_default = x as i32)
                .is_some(),
            "clan.winner_point_default" => n
                .map(|x| self.policy.winner_point_default = x as i32)
                .is_some(),
            "clan.level_steps" => match nums() {
                // cumulative thresholds: level 1 starts at 0, never decreasing
                Some(t) if t[0] == 0.0 && t.windows(2).all(|w| w[0] <= w[1]) => {
                    self.level_steps = t.iter().map(|x| *x as i64).collect();
                    true
                }
                _ => false,
            },
            "clan.member_limits" => match nums() {
                Some(t) if t.iter().all(|x| *x >= 1.0 && *x <= MAX_MEMBER_ROWS as f64) => {
                    self.member_limits = t.iter().map(|x| *x as u32).collect();
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Clan level 1.. for an amount of clan exp.
    pub fn level_of(&self, exp: i64) -> u32 {
        (self.level_steps.iter().filter(|t| exp >= **t).count() as u32).max(1)
    }

    pub fn member_limit(&self, level: u32) -> u32 {
        let i = (level.max(1) as usize - 1).min(self.member_limits.len().saturating_sub(1));
        self.member_limits.get(i).copied().unwrap_or(1)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// tables (the `clans` table and the character clan columns are in db.rs; these are the clan module's own)
// ---------------------------------------------------------------------------------------------------------------

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS clan_records (
    clan_uid   INTEGER PRIMARY KEY,
    wins       INTEGER NOT NULL DEFAULT 0,
    losses     INTEGER NOT NULL DEFAULT 0,
    draws      INTEGER NOT NULL DEFAULT 0,
    exp        INTEGER NOT NULL DEFAULT 0,
    points     INTEGER NOT NULL DEFAULT 0,
    prev_rank  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS clan_joins (
    user_no   INTEGER PRIMARY KEY,
    clan_uid  INTEGER NOT NULL,
    joined    INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS clan_rooms (
    channel  INTEGER NOT NULL,
    room     INTEGER NOT NULL,
    home     INTEGER NOT NULL,
    away     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (channel, room)
);
CREATE TABLE IF NOT EXISTS clan_matches (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    ended        INTEGER NOT NULL DEFAULT 0,
    channel      INTEGER NOT NULL DEFAULT 0,
    room         INTEGER NOT NULL DEFAULT 0,
    home         INTEGER NOT NULL DEFAULT 0,
    away         INTEGER NOT NULL DEFAULT 0,
    winner_clan  INTEGER NOT NULL DEFAULT 0,
    home_exp     INTEGER NOT NULL DEFAULT 0,
    away_exp     INTEGER NOT NULL DEFAULT 0,
    home_points  INTEGER NOT NULL DEFAULT 0,
    away_points  INTEGER NOT NULL DEFAULT 0
);
"#;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClanRow {
    pub uid: i64,
    pub name: String,
    pub master: u32,
    pub emblem: i64,
    pub background: i64,
    pub notice: String,
}

fn clan_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ClanRow> {
    Ok(ClanRow {
        uid: r.get(0)?,
        name: r.get(1)?,
        master: r.get::<_, i64>(2)? as u32,
        emblem: r.get(3)?,
        background: r.get(4)?,
        notice: r.get(5)?,
    })
}

const CLAN_COLS: &str = "clan_uid, name, master, emblem, background, notice";

pub fn clan_by_uid(c: &Connection, uid: i64) -> rusqlite::Result<Option<ClanRow>> {
    c.query_row(
        &format!("SELECT {CLAN_COLS} FROM clans WHERE clan_uid = ?1"),
        [uid],
        clan_row,
    )
    .optional()
}

pub fn clan_by_name(c: &Connection, name: &str) -> rusqlite::Result<Option<ClanRow>> {
    c.query_row(
        &format!("SELECT {CLAN_COLS} FROM clans WHERE name = ?1 COLLATE NOCASE"),
        [name.trim()],
        clan_row,
    )
    .optional()
}

/// Every clan, best rank first.
pub fn all_clans(c: &Connection) -> rusqlite::Result<Vec<ClanRow>> {
    let mut q = c.prepare(
        "SELECT k.clan_uid, k.name, k.master, k.emblem, k.background, k.notice FROM clans k \
         LEFT JOIN clan_records r ON r.clan_uid = k.clan_uid ORDER BY COALESCE(r.exp, 0) DESC, k.clan_uid",
    )?;
    let rows = q.query_map([], clan_row)?;
    rows.collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Member {
    pub user_no: u32,
    pub nick: String,
    pub role: i64,
    pub exp: i64,
}

/// Members of a clan: the leader first, then by join order.
pub fn members(c: &Connection, uid: i64) -> rusqlite::Result<Vec<Member>> {
    let mut q = c.prepare(
        "SELECT ch.user_no, ch.nick, ch.clan_type, ch.exp FROM characters ch \
         LEFT JOIN clan_joins j ON j.user_no = ch.user_no WHERE ch.clan_uid = ?1 \
         ORDER BY ch.clan_type = 5 DESC, COALESCE(j.joined, 0), ch.user_no",
    )?;
    let rows = q.query_map([uid], |r| {
        Ok(Member {
            user_no: r.get::<_, i64>(0)? as u32,
            nick: r.get(1)?,
            role: r.get(2)?,
            exp: r.get(3)?,
        })
    })?;
    rows.collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub wins: i64,
    pub losses: i64,
    pub draws: i64,
    pub exp: i64,
    pub points: i64,
    pub prev_rank: i64,
}

pub fn stats(c: &Connection, uid: i64) -> rusqlite::Result<Stats> {
    Ok(c.query_row(
        "SELECT wins, losses, draws, exp, points, prev_rank FROM clan_records WHERE clan_uid = ?1",
        [uid],
        |r| {
            Ok(Stats {
                wins: r.get(0)?,
                losses: r.get(1)?,
                draws: r.get(2)?,
                exp: r.get(3)?,
                points: r.get(4)?,
                prev_rank: r.get(5)?,
            })
        },
    )
    .optional()?
    .unwrap_or_default())
}

/// Rank 1.. by clan exp, ties to the older clan (lower number).
// DC-DEFAULT: CLN-11 (proposed) clans are ranked by clan exp, ties by age; the previous rank is the rank at server start
pub fn rank(c: &Connection, uid: i64) -> rusqlite::Result<u32> {
    let exp = stats(c, uid)?.exp;
    let better: i64 = c.query_row(
        "SELECT COUNT(*) FROM clans k LEFT JOIN clan_records r ON r.clan_uid = k.clan_uid \
         WHERE COALESCE(r.exp, 0) > ?1 OR (COALESCE(r.exp, 0) = ?1 AND k.clan_uid < ?2)",
        params![exp, uid],
        |r| r.get(0),
    )?;
    Ok(better as u32 + 1)
}

/// The clan an account belongs to (None when the clan row is gone).
pub fn clan_of(st: &Core, account: u32) -> rusqlite::Result<Option<ClanRow>> {
    let uid: Option<i64> = st
        .db
        .query_row(
            "SELECT clan_uid FROM characters WHERE user_no = ?1",
            [account as i64],
            |r| r.get(0),
        )
        .optional()?;
    match uid {
        Some(u) if u != 0 => clan_by_uid(&st.db, u),
        _ => Ok(None),
    }
}

/// The 0x3c3 clan record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClanSheet {
    pub uid: u32,
    pub member_limit: u32,
    pub members: u32,
    pub prev_rank: u32,
    pub rank: u32,
    pub level: u32,
    pub mark: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub exp: u32,
    pub points: u32,
    pub name: String,
    pub leader: String,
}

fn c32(v: i64) -> u32 {
    v.clamp(0, u32::MAX as i64) as u32
}

pub fn record(st: &Core, clan: &ClanRow) -> rusqlite::Result<ClanSheet> {
    let s = stats(&st.db, clan.uid)?;
    let m = members(&st.db, clan.uid)?;
    let level = st.rules.clan.level_of(s.exp);
    let leader = m
        .iter()
        .find(|x| x.user_no == clan.master)
        .map(|x| x.nick.clone())
        .unwrap_or_default();
    Ok(ClanSheet {
        uid: c32(clan.uid),
        member_limit: st.rules.clan.member_limit(level),
        members: m.len() as u32,
        prev_rank: c32(s.prev_rank),
        rank: rank(&st.db, clan.uid)?,
        level,
        // DC-DEFAULT: CLN-12 (proposed) the record's mark word carries the emblem number (its use is unproven)
        mark: c32(clan.emblem),
        wins: c32(s.wins),
        losses: c32(s.losses),
        draws: c32(s.draws),
        exp: c32(s.exp),
        points: c32(s.points),
        name: clan.name.clone(),
        leader,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// layouts
// ---------------------------------------------------------------------------------------------------------------

/// 0x3c3 (0x82): +6 u32 result; +0xa u32 0; +0xe twelve u32 (clan number, member limit, members, previous rank,
/// rank, level, mark, wins, losses, draws, exp, points); +0x3e wstr[17] clan name; +0x60 wstr[17] leader name.
// EVIDENCE: cshell+0x35750 (+6 result, +0x36 / +0x3a to the shell state), cshell+0x106030 (twelve u32 from +0xe,
// the name copied from +0x3e); SPEC: PROTOCOL_2009.md §5.1 0x3c3 (size 0x82, leader name LIVE)
pub fn info_reply(r: Option<&ClanSheet>) -> Vec<u8> {
    let mut w = PktWriter::new(CLAN_INFO_REPLY);
    match r {
        Some(r) => {
            w.u32(1).u32(0);
            for v in [
                r.uid,
                r.member_limit,
                r.members,
                r.prev_rank,
                r.rank,
                r.level,
                r.mark,
                r.wins,
                r.losses,
                r.draws,
                r.exp,
                r.points,
            ] {
                w.u32(v);
            }
            w.wstr(&r.name, 17).wstr(&r.leader, 17);
        }
        None => {
            w.u32(0);
        }
    }
    w.at(0x82);
    w.into_frame()
}

/// One 0x3c5 row (0x32 bytes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberRow {
    pub char_no: u32,
    pub role: u32,
    pub exp: u32,
    pub name: String,
    pub online: u32,
}

/// 0x3c5 (0x12 + 0x32 n): +6 u32 result; +0xe u32 count; rows from +0x12: +0 character number, +4 role, +8 exp,
/// +0xc wstr[17] name, +0x2e u32 online.
// EVIDENCE: cshell+0x35820 -> cshell+0x111990 (count at +0xe, rows of 0x32 from +0x12, row +0 / +4 / +8, name copied
// from row +0xc, row +0x2e)
pub fn members_reply(ok: bool, rows: &[MemberRow]) -> Vec<u8> {
    let rows = if ok {
        &rows[..rows.len().min(MAX_MEMBER_ROWS)]
    } else {
        &[][..]
    };
    let mut w = PktWriter::new(CLAN_MEMBERS_REPLY);
    w.u32(ok as u32).u32(0).u32(rows.len() as u32);
    for r in rows {
        let base = w.len();
        w.u32(r.char_no)
            .u32(r.role)
            .u32(r.exp)
            .wstr(&r.name, 17)
            .u32(r.online);
        debug_assert_eq!(w.len() - base, 0x32);
    }
    w.at(w.len().max(0x12));
    w.into_frame()
}

/// 0x3c7 (0x100): +6 u32 result; +0xe wstr[121] notice.
// EVIDENCE: cshell+0x35880 -> cshell+0x105210 (+6 result, notice string from +0xe)
pub fn notice_reply(notice: Option<&str>) -> Vec<u8> {
    let mut w = PktWriter::new(CLAN_NOTICE_REPLY);
    w.u32(notice.is_some() as u32)
        .at(0xe)
        .wstr(notice.unwrap_or(""), 121);
    w.at(0x100);
    w.into_frame()
}

/// `{+6 u32 ok}` replies (0x3c1, 0x3c9), 0xa bytes.
// EVIDENCE: cshell+0x356f0 (0x3c1), cshell+0x358e0 (0x3c9): only +6 is read
pub fn result_reply(id: u16, ok: bool) -> Vec<u8> {
    wire::ok_reply(id, ok, 0xa)
}

/// `{+6 u32 ok; +0xa u32 reason}` check replies (0x349, 0x34b, 0x34d), 0xe bytes.
// SPEC: PROTOCOL_2009.md §5.1 0x349 / 0x34b / 0x34d
pub fn check_reply(id: u16, ok: bool, why: u32) -> Vec<u8> {
    let mut w = PktWriter::new(id);
    w.u32(ok as u32).u32(if ok { 0 } else { why });
    w.into_frame()
}

/// Header-only push (0x3061 .. 0x3064).
// SPEC: PROTOCOL_2009.md §5.1 (0x3061 .. 0x3064 carry no fields)
pub fn push(id: u16) -> Vec<u8> {
    PktWriter::new(id).into_frame()
}

/// 0x3c8 +6 wstr[121]; 0x348 / 0x34a +6 wstr[17]; 0x34c +6 emblem, +0xa background.
// EVIDENCE: cshell+0x18d870 (0x3c8), cshell+0x18e4f0 (0x348), cshell+0x18e5d0 (0x34a), cshell+0x18e6b0 (0x34c)
pub fn parse_notice(p: &[u8]) -> String {
    PktReader::new(p).at(6).wstr(121)
}

pub fn parse_name(p: &[u8]) -> String {
    PktReader::new(p).at(6).wstr(17)
}

pub fn parse_mark(p: &[u8]) -> (u32, u32) {
    let mut r = PktReader::new(p);
    let e = r.at(6).u32();
    let b = r.u32();
    (e, b)
}

// ---------------------------------------------------------------------------------------------------------------
// membership operations (shared by the web pages, the chat commands and the tests)
// ---------------------------------------------------------------------------------------------------------------

/// Why an operation was refused (the text goes into the web page or the chat reply).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    NoCharacter,
    NotOnline,
    InClan(String),
    NoClan,
    NotLeader,
    BadName,
    NameTaken,
    MarkInUse,
    NoSuchClan,
    Full,
    NoSuchMember,
}

impl Refusal {
    pub fn text(&self) -> String {
        match self {
            Refusal::NoCharacter => "this account has no character yet".into(),
            Refusal::NotOnline => "log in to the game first".into(),
            Refusal::InClan(n) => format!("you are already in clan {}", n),
            Refusal::NoClan => "you are not in a clan".into(),
            Refusal::NotLeader => "only the clan leader may do that".into(),
            Refusal::BadName => format!("a clan name has 1 to {} characters", NAME_UNITS),
            Refusal::NameTaken => "that clan name is taken".into(),
            Refusal::MarkInUse => "another clan uses that mark".into(),
            Refusal::NoSuchClan => "no such clan".into(),
            Refusal::Full => "the clan is full".into(),
            Refusal::NoSuchMember => "no such member".into(),
        }
    }
}

/// Outcome of an operation: the inner `Err` is a refusal, the outer one a DB error (the scope rolls back).
pub type Act<T> = rusqlite::Result<Result<T, Refusal>>;

/// A clan name the wire can carry: 1..16 UTF-16 units, no control characters, trimmed.
// DC-DEFAULT: CD-32 names unique per server (case-insensitive, `clans.name COLLATE NOCASE`); CLN-10 (proposed) 1..16
// units, no control characters
pub fn valid_name(n: &str) -> bool {
    let len = n.encode_utf16().count();
    (1..=NAME_UNITS).contains(&len) && n.trim() == n && !n.chars().any(|c| c.is_control())
}

pub fn name_taken(c: &Connection, name: &str, except: i64) -> rusqlite::Result<bool> {
    let n: i64 = c.query_row(
        "SELECT COUNT(*) FROM clans WHERE name = ?1 COLLATE NOCASE AND clan_uid != ?2",
        params![name.trim(), except],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// CD-32: one emblem + background pair per clan. The pair (0, 0) is "no mark yet" and may repeat.
// DC-DEFAULT: CD-32 one emblem + background pair per clan; CLN-10 (proposed) (0, 0) = no mark, exempt
pub fn mark_taken(
    c: &Connection,
    emblem: i64,
    background: i64,
    except: i64,
) -> rusqlite::Result<bool> {
    if emblem == 0 && background == 0 {
        return Ok(false);
    }
    let n: i64 = c.query_row(
        "SELECT COUNT(*) FROM clans WHERE emblem = ?1 AND background = ?2 AND clan_uid != ?3",
        params![emblem, background, except],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn send_to(st: &mut Core, account: u32, frame: &[u8]) {
    for s in st.sids_of(account) {
        st.send(s, frame.to_vec());
    }
}

/// Unprompted 0x303 so the client refreshes its own clan name, role and mark (SPEC: PROTOCOL_2009.md §3.3 0x303).
pub fn push_sheet(st: &mut Core, account: u32) {
    if let Some(c) = st.character(account) {
        let f = wire::char_sheet_reply(&st.char_sheet(&c));
        send_to(st, account, &f);
    }
    refresh_user_row(st, account);
}

/// The character's lobby user row again (0x3021; the client replaces the row of the same id), so every client in
/// the channel sees the new clan and mark without a re-entry. A character in a room has no lobby row.
// SPEC: PROTOCOL_2009.md section 3.6 0x3021 (+0x2e clan, +0x32 emblem, +0x36 background)
// DC-DEFAULT: CLI-1020 (proposed) the row is re-sent when the character's clan data changes
fn refresh_user_row(st: &mut Core, account: u32) {
    let channels: Vec<u32> = st
        .sids_of(account)
        .into_iter()
        .filter_map(|s| st.sessions.get(&s))
        .filter(|s| s.room.is_none())
        .filter_map(|s| s.channel)
        .collect();
    for ch in channels {
        let rec = wire::lobby_user_add_push(&st.lobby_user(account));
        st.push_lobby(ch, &rec);
    }
}

/// Pushes header-only frames to the online members of a clan, except one account.
pub fn push_members(
    st: &mut Core,
    clan: i64,
    except: Option<u32>,
    ids: &[u16],
) -> rusqlite::Result<()> {
    let m = members(&st.db, clan)?;
    for x in m.iter().filter(|x| Some(x.user_no) != except) {
        for &id in ids {
            send_to(st, x.user_no, &push(id));
        }
    }
    Ok(())
}

fn set_member(st: &Core, account: u32, clan: &ClanRow, role: i64) -> rusqlite::Result<()> {
    st.db.execute(
        "UPDATE characters SET clan_uid = ?2, clan_name = ?3, clan_type = ?4 WHERE user_no = ?1",
        params![account as i64, clan.uid, clan.name, role],
    )?;
    // join order: a growing sequence number (the hand-over rule picks the longest-standing member)
    let seq: i64 = st.db.query_row(
        "SELECT COALESCE(MAX(joined), 0) + 1 FROM clan_joins",
        [],
        |r| r.get(0),
    )?;
    st.db.execute(
        "INSERT INTO clan_joins (user_no, clan_uid, joined) VALUES (?1, ?2, ?3) \
         ON CONFLICT(user_no) DO UPDATE SET clan_uid = excluded.clan_uid, joined = excluded.joined",
        params![account as i64, clan.uid, seq],
    )?;
    Ok(())
}

fn clear_member(st: &Core, account: u32) -> rusqlite::Result<()> {
    st.db.execute(
        "UPDATE characters SET clan_uid = 0, clan_name = '', clan_type = 0 WHERE user_no = ?1",
        [account as i64],
    )?;
    st.db.execute(
        "DELETE FROM clan_joins WHERE user_no = ?1",
        [account as i64],
    )?;
    Ok(())
}

/// The character and its current clan (None when it has none or the clan row is gone).
fn me(st: &Core, account: u32) -> Act<(crate::db::Character, Option<ClanRow>)> {
    let Some(c) = st.character(account) else {
        return Ok(Err(Refusal::NoCharacter));
    };
    let clan = if c.clan_uid != 0 {
        clan_by_uid(&st.db, c.clan_uid)?
    } else {
        None
    };
    Ok(Ok((c, clan)))
}

/// Creates a clan led by `account`.
// DC-DEFAULT: CD-32 (unique name, one mark pair per clan), CD-47 (web create), CLN-7 (proposed) the creator leads it
pub fn create(
    st: &mut Core,
    account: u32,
    name: &str,
    emblem: i64,
    background: i64,
) -> Act<ClanRow> {
    let (_c, cur) = match me(st, account)? {
        Ok(v) => v,
        Err(r) => return Ok(Err(r)),
    };
    if let Some(k) = cur {
        return Ok(Err(Refusal::InClan(k.name)));
    }
    let name = name.trim();
    if !valid_name(name) {
        return Ok(Err(Refusal::BadName));
    }
    if name_taken(&st.db, name, 0)? {
        return Ok(Err(Refusal::NameTaken));
    }
    let (emblem, background) = (emblem.max(0), background.max(0));
    if mark_taken(&st.db, emblem, background, 0)? {
        return Ok(Err(Refusal::MarkInUse));
    }
    st.db.execute(
        "INSERT INTO clans (name, master, emblem, background, notice, created) VALUES (?1, ?2, ?3, ?4, '', ?5)",
        params![name, account as i64, emblem, background, crate::clock::unix_now()],
    )?;
    let uid = st.db.last_insert_rowid();
    st.db.execute(
        "INSERT OR REPLACE INTO clan_records (clan_uid) VALUES (?1)",
        [uid],
    )?;
    let clan = clan_by_uid(&st.db, uid)?.unwrap_or_default();
    set_member(st, account, &clan, ROLE_LEADER)?;
    // the creator's client refreshes its clan data (0x303) and, on the clan screen, the clan tabs
    push_sheet(st, account);
    send_to(st, account, &push(CLAN_INFO_STALE_PUSH));
    send_to(st, account, &push(CLAN_MEMBERS_STALE_PUSH));
    st.note(&format!(
        "clan {} '{}' created by account {}",
        uid, clan.name, account
    ));
    Ok(Ok(clan))
}

/// `account` joins clan `uid` as an ordinary member.
// DC-DEFAULT: CD-47 (web join), CLN-6 (proposed) member limit by clan level
pub fn join(st: &mut Core, account: u32, uid: i64) -> Act<ClanRow> {
    let (_c, cur) = match me(st, account)? {
        Ok(v) => v,
        Err(r) => return Ok(Err(r)),
    };
    if let Some(k) = cur {
        return Ok(Err(Refusal::InClan(k.name)));
    }
    let Some(clan) = clan_by_uid(&st.db, uid)? else {
        return Ok(Err(Refusal::NoSuchClan));
    };
    let rec = record(st, &clan)?;
    if rec.members >= rec.member_limit {
        return Ok(Err(Refusal::Full));
    }
    set_member(st, account, &clan, ROLE_MEMBER)?;
    push_sheet(st, account);
    send_to(st, account, &push(CLAN_INFO_STALE_PUSH));
    send_to(st, account, &push(CLAN_MEMBERS_STALE_PUSH));
    push_members(
        st,
        clan.uid,
        Some(account),
        &[CLAN_INFO_STALE_PUSH, CLAN_MEMBERS_STALE_PUSH],
    )?;
    st.note(&format!(
        "account {} joined clan {} '{}'",
        account, clan.uid, clan.name
    ));
    Ok(Ok(clan))
}

/// What a leave did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Left {
    /// An ordinary member left.
    Member(ClanRow),
    /// The leader left; the leadership went to this account.
    HandedOver(ClanRow, u32),
    /// The last member (the leader) left; the clan is gone.
    Disbanded(ClanRow),
}

impl Left {
    pub fn clan(&self) -> &ClanRow {
        match self {
            Left::Member(c) | Left::HandedOver(c, _) | Left::Disbanded(c) => c,
        }
    }
}

fn removed(st: &mut Core, account: u32) {
    // 0x3064 first, then the refreshed 0x303 (HARNESS: lobby_harness::lobby3 "0x3064 + 0x303 came first")
    send_to(st, account, &push(CLAN_REMOVED_PUSH));
    push_sheet(st, account);
}

/// `account` leaves its clan. A leader with members hands the clan to the longest-standing member; a leader
/// alone disbands it.
// DC-DEFAULT: CD-47 a leave makes the lobby push 0x3064 to that user; CLN-7 (proposed) leadership hand-over / disband
pub fn leave(st: &mut Core, account: u32) -> Act<Left> {
    let (c, cur) = match me(st, account)? {
        Ok(v) => v,
        Err(r) => return Ok(Err(r)),
    };
    let Some(clan) = cur else {
        if c.clan_uid != 0 {
            // a dangling clan number (the clan row is gone): clean it up
            clear_member(st, account)?;
            push_sheet(st, account);
        }
        return Ok(Err(Refusal::NoClan));
    };
    let others: Vec<Member> = members(&st.db, clan.uid)?
        .into_iter()
        .filter(|m| m.user_no != account)
        .collect();
    clear_member(st, account)?;
    let out = if clan.master == account || c.clan_role == ROLE_LEADER {
        // `members` lists the leader first, then by join order: the first other member stood longest
        match others.first() {
            Some(next) => {
                let next_no = next.user_no;
                st.db.execute(
                    "UPDATE clans SET master = ?2 WHERE clan_uid = ?1",
                    params![clan.uid, next_no as i64],
                )?;
                st.db.execute(
                    "UPDATE characters SET clan_type = ?2 WHERE user_no = ?1",
                    params![next_no as i64, ROLE_LEADER],
                )?;
                push_sheet(st, next_no);
                Left::HandedOver(clan.clone(), next_no)
            }
            None => {
                disband_rows(st, clan.uid)?;
                Left::Disbanded(clan.clone())
            }
        }
    } else {
        Left::Member(clan.clone())
    };
    removed(st, account);
    if !matches!(out, Left::Disbanded(_)) {
        push_members(
            st,
            clan.uid,
            None,
            &[CLAN_INFO_STALE_PUSH, CLAN_MEMBERS_STALE_PUSH],
        )?;
    }
    st.note(&format!(
        "account {} left clan {} '{}' ({:?})",
        account, clan.uid, clan.name, out
    ));
    Ok(Ok(out))
}

fn disband_rows(st: &Core, uid: i64) -> rusqlite::Result<()> {
    st.db.execute(
        "UPDATE characters SET clan_uid = 0, clan_name = '', clan_type = 0 WHERE clan_uid = ?1",
        [uid],
    )?;
    st.db
        .execute("DELETE FROM clan_joins WHERE clan_uid = ?1", [uid])?;
    st.db
        .execute("DELETE FROM clan_records WHERE clan_uid = ?1", [uid])?;
    st.db
        .execute("DELETE FROM clan_colours WHERE clan_uid = ?1", [uid])?;
    st.db
        .execute("DELETE FROM clans WHERE clan_uid = ?1", [uid])?;
    Ok(())
}

/// The leader removes a member.
// DC-DEFAULT: CLN-7 (proposed) only the leader removes members; the removed member gets 0x3064 like a leave (CD-47)
pub fn kick(st: &mut Core, leader: u32, target: u32) -> Act<(ClanRow, String)> {
    let (_c, cur) = match me(st, leader)? {
        Ok(v) => v,
        Err(r) => return Ok(Err(r)),
    };
    let Some(clan) = cur else {
        return Ok(Err(Refusal::NoClan));
    };
    if clan.master != leader {
        return Ok(Err(Refusal::NotLeader));
    }
    let Some(t) = members(&st.db, clan.uid)?
        .into_iter()
        .find(|m| m.user_no == target && m.user_no != leader)
    else {
        return Ok(Err(Refusal::NoSuchMember));
    };
    clear_member(st, target)?;
    removed(st, target);
    push_members(
        st,
        clan.uid,
        None,
        &[CLAN_INFO_STALE_PUSH, CLAN_MEMBERS_STALE_PUSH],
    )?;
    st.note(&format!(
        "clan {}: account {} removed by the leader {}",
        clan.uid, target, leader
    ));
    Ok(Ok((clan, t.nick)))
}

/// The leader sets the clan notice (0x3c8 and the web page); the other members get 0x3063.
// DC-DEFAULT: CLN-7 (proposed) only the leader edits the notice; at most 120 units (the wire field)
pub fn set_notice(st: &mut Core, account: u32, text: &str) -> Act<ClanRow> {
    let (_c, cur) = match me(st, account)? {
        Ok(v) => v,
        Err(r) => return Ok(Err(r)),
    };
    let Some(clan) = cur else {
        return Ok(Err(Refusal::NoClan));
    };
    if clan.master != account {
        return Ok(Err(Refusal::NotLeader));
    }
    let units: Vec<u16> = text.encode_utf16().take(NOTICE_UNITS).collect();
    let text = String::from_utf16_lossy(&units);
    st.db.execute(
        "UPDATE clans SET notice = ?2 WHERE clan_uid = ?1",
        params![clan.uid, text],
    )?;
    push_members(st, clan.uid, Some(account), &[CLAN_NOTICE_STALE_PUSH])?;
    Ok(Ok(clan))
}

// ---------------------------------------------------------------------------------------------------------------
// clan messages
// ---------------------------------------------------------------------------------------------------------------

fn on_clan_login(st: &mut Core, sid: Sid) -> HResult {
    let ok = match account_of(st, sid) {
        Some(a) => clan_of(st, a)?.is_some(),
        None => false,
    };
    st.send(sid, result_reply(CLAN_LOGIN_REPLY, ok));
    Ok(())
}

fn on_info(st: &mut Core, sid: Sid) -> HResult {
    let clan = match account_of(st, sid) {
        Some(a) => clan_of(st, a)?,
        None => None,
    };
    let rec = match &clan {
        Some(k) => Some(record(st, k)?),
        None => None,
    };
    st.send(sid, info_reply(rec.as_ref()));
    Ok(())
}

pub fn member_rows(st: &Core, clan: i64) -> rusqlite::Result<Vec<MemberRow>> {
    Ok(members(&st.db, clan)?
        .into_iter()
        .map(|m| MemberRow {
            char_no: m.user_no,
            role: c32(m.role),
            exp: c32(m.exp),
            online: !st.sids_of(m.user_no).is_empty() as u32,
            name: m.nick,
        })
        .collect())
}

fn on_members(st: &mut Core, sid: Sid) -> HResult {
    let clan = match account_of(st, sid) {
        Some(a) => clan_of(st, a)?,
        None => None,
    };
    let f = match clan {
        Some(k) => members_reply(true, &member_rows(st, k.uid)?),
        None => members_reply(false, &[]),
    };
    st.send(sid, f);
    Ok(())
}

fn on_notice(st: &mut Core, sid: Sid) -> HResult {
    let clan = match account_of(st, sid) {
        Some(a) => clan_of(st, a)?,
        None => None,
    };
    st.send(sid, notice_reply(clan.as_ref().map(|k| k.notice.as_str())));
    Ok(())
}

fn on_notice_set(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let text = parse_notice(p);
    let ok = match account_of(st, sid) {
        Some(a) => set_notice(st, a, &text)?.is_ok(),
        None => false,
    };
    st.send(sid, result_reply(CLAN_NOTICE_SET_REPLY, ok));
    Ok(())
}

/// 0x348 -> 0x349: is the named character a clan leader (the gift window, for products that hold a clan item).
// DC-DEFAULT: CD-73 (proposed) a clan item gift needs a clan-leader receiver: unknown name 0x56, not a leader 0x68
fn on_leader_check(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let name = parse_name(p);
    let found = crate::db::character_by_nick(&st.db, name.trim(), st.rules.names_case_insensitive)?;
    let f = match found {
        None => check_reply(CLAN_LEADER_CHECK_REPLY, false, 0x56),
        Some(c) => {
            let leads = c.clan_uid != 0
                && clan_by_uid(&st.db, c.clan_uid)?
                    .map(|k| k.master == c.user_no)
                    .unwrap_or(false);
            check_reply(CLAN_LEADER_CHECK_REPLY, leads, 0x68)
        }
    };
    st.send(sid, f);
    Ok(())
}

/// 0x34a -> 0x34b: is a clan name free (the clan-name item window).
// DC-DEFAULT: CD-32 names unique per server: taken 0x4e; CLN-10 (proposed) invalid 0x57
fn on_name_check(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let name = parse_name(p);
    let name = name.trim();
    // the leader may keep the own clan's name (a case change through the rename item)
    let own = match account_of(st, sid) {
        Some(a) => clan_of(st, a)?
            .filter(|k| k.master == a)
            .map(|k| k.uid)
            .unwrap_or(0),
        None => 0,
    };
    let f = if !valid_name(name) {
        check_reply(CLAN_NAME_CHECK_REPLY, false, reason::TOO_LONG)
    } else if name_taken(&st.db, name, own)? {
        check_reply(CLAN_NAME_CHECK_REPLY, false, reason::CLAN_NAME_TAKEN)
    } else {
        check_reply(CLAN_NAME_CHECK_REPLY, true, 0)
    };
    st.send(sid, f);
    Ok(())
}

/// 0x34c -> 0x34d: may the sender's clan take this emblem + background (the clan-mark item window).
// DC-DEFAULT: CD-32 only the leader may change the mark; one pair per clan: 0x4a no clan, 0x4b not leader, 0x4c in use
fn on_mark_check(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (emblem, background) = parse_mark(p);
    let clan = match account_of(st, sid) {
        Some(a) => clan_of(st, a)?.map(|k| (k, a)),
        None => None,
    };
    let f = match clan {
        None => check_reply(CLAN_MARK_CHECK_REPLY, false, reason::NO_CLAN),
        Some((k, a)) if k.master != a => {
            check_reply(CLAN_MARK_CHECK_REPLY, false, reason::NOT_LEADER)
        }
        Some((k, _)) if mark_taken(&st.db, emblem as i64, background as i64, k.uid)? => {
            check_reply(CLAN_MARK_CHECK_REPLY, false, reason::MARK_IN_USE)
        }
        Some(_) => check_reply(CLAN_MARK_CHECK_REPLY, true, 0),
    };
    st.send(sid, f);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// chat commands (`!clan create <name> [emblem background]`, `!clan join <name>`, `!clan leave`, `!clan kick <nick>`)
// ---------------------------------------------------------------------------------------------------------------

/// A line from the server to one user only (0x3051 with sender 0).
pub fn tell(st: &mut Core, sid: Sid, text: &str) {
    st.send(sid, wire::chat_push(0, "", 0, text));
}

/// Handles a `!clan` chat line; false when the line is ordinary chat.
// DC-DEFAULT: CLN-9 (proposed) chat commands for the operations of the clan web pages (HARNESS:
// lobby_harness::lobby3 / ::test_persist_restart drive clans through them)
fn bang_command(st: &mut Core, sid: Sid, p: &[u8]) -> Result<bool, Fault> {
    if !st.rules.clan.chat_commands {
        return Ok(false);
    }
    let text = wire::parse_chat(p);
    let mut words = text.split_whitespace();
    if !words
        .next()
        .map(|w| w.eq_ignore_ascii_case("!clan"))
        .unwrap_or(false)
    {
        return Ok(false);
    }
    let Some(account) = account_of(st, sid) else {
        return Ok(true);
    };
    let verb = words.next().unwrap_or("").to_ascii_lowercase();
    let rest: Vec<&str> = words.collect();
    let msg = match verb.as_str() {
        "create" => {
            // a trailing pair of numbers is the emblem and background
            let nums: Vec<i64> = rest.iter().rev().take(2).filter_map(|w| w.parse().ok()).collect();
            let (name_words, emblem, bg) = if nums.len() == 2 && rest.len() > 2 {
                (&rest[..rest.len() - 2], nums[1], nums[0])
            } else {
                (&rest[..], 0, 0)
            };
            let name = name_words.join(" ");
            match create(st, account, &name, emblem, bg)? {
                Ok(k) => format!("Clan {} created.", k.name),
                Err(r) => format!("Cannot create the clan: {}.", r.text()),
            }
        }
        "join" => {
            let name = rest.join(" ");
            match clan_by_name(&st.db, &name)? {
                Some(k) => match join(st, account, k.uid)? {
                    Ok(k) => format!("You joined {}.", k.name),
                    Err(r) => format!("Cannot join: {}.", r.text()),
                },
                None => format!("Cannot join: {}.", Refusal::NoSuchClan.text()),
            }
        }
        "leave" => match leave(st, account)? {
            Ok(l) => format!("You left {}.", l.clan().name),
            Err(r) => format!("Cannot leave: {}.", r.text()),
        },
        "kick" => {
            let nick = rest.join(" ");
            match crate::db::character_by_nick(&st.db, &nick, st.rules.names_case_insensitive)? {
                Some(t) => match kick(st, account, t.user_no)? {
                    Ok((_, n)) => format!("{} was removed from the clan.", n),
                    Err(r) => format!("Cannot remove: {}.", r.text()),
                },
                None => format!("Cannot remove: {}.", Refusal::NoSuchMember.text()),
            }
        }
        _ => "Clan commands: !clan create <name> [emblem background], !clan join <name>, !clan leave, !clan kick <name>".into(),
    };
    tell(st, sid, &msg);
    Ok(true)
}

// ---------------------------------------------------------------------------------------------------------------
// the clan lobby (the clan-only server entry, SPEC: PROTOCOL_2009.md §3.2 +0xbc)
// ---------------------------------------------------------------------------------------------------------------

pub fn is_clan_session(st: &Core, sid: Sid) -> bool {
    st.sessions
        .get(&sid)
        .map(|s| s.listener == Listener::Clan)
        .unwrap_or(false)
}

pub fn is_clan_channel(ch: u32) -> bool {
    ch > CLAN_CHANNEL_BASE && ch <= CLAN_CHANNEL_BASE + 64
}

fn clan_channel_of(st: &Core, sid: Sid) -> Option<u32> {
    st.sessions
        .get(&sid)
        .and_then(|s| s.channel)
        .filter(|c| is_clan_channel(*c))
}

fn users_in(st: &Core, ch: u32) -> u32 {
    st.sessions
        .values()
        .filter(|s| s.in_lobby && !s.closing && s.channel == Some(ch))
        .count() as u32
}

/// 0x377 for the clan lobby: the same channel numbers as the lobby, counted on the clan lobby's own channels.
// DC-DEFAULT: CD-21 channels from lobby.channels
pub fn clan_channels(st: &Core) -> Vec<ChannelEntry> {
    let cap = if st.opts.channel_max > 0 {
        st.opts.channel_max.min(u32::MAX as i64) as u32
    } else {
        200
    };
    (1..=st.rules.channels)
        .map(|id| ChannelEntry {
            id,
            name: format!("Clan Channel {}", id),
            open: true,
            capacity: cap,
            current: users_in(st, CLAN_CHANNEL_BASE + id),
            op_capacity: crate::gm::OPERATOR_SLOTS,
            op_current: crate::gm::gms_online(st, Some(CLAN_CHANNEL_BASE + id)),
        })
        .collect()
}

// DC-DEFAULT: CLN-1 (proposed) the clan lobby admits clan members only (0x373 ok = 0 otherwise)
fn on_clan_channel_enter(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (_acc, ch) = wire::parse_channel_enter(p);
    let Some(account) = account_of(st, sid) else {
        st.send(sid, wire::channel_enter_reply(false, 0));
        return Ok(());
    };
    let real = CLAN_CHANNEL_BASE + ch;
    let current = st.sessions.get(&sid).and_then(|s| s.channel);
    let cap = clan_channels(st).first().map(|c| c.capacity).unwrap_or(200);
    let exists = (1..=st.rules.channels).contains(&ch);
    let full = current != Some(real) && users_in(st, real) >= cap;
    let member = clan_of(st, account)?.is_some();
    // DC-DEFAULT: LG-175 a GM enters the clan lobby without a clan (the 2009 client lets an operator onto a
    // clan-only server) and a full clan channel while its operator slots are not used up
    let gm = crate::gm::is_gm(st, account);
    let full = full && !(gm && crate::gm::gms_online(st, Some(real)) < crate::gm::OPERATOR_SLOTS);
    if !exists || full || (st.rules.clan.channel_members_only && !member && !gm) {
        st.send(sid, wire::channel_enter_reply(false, 0));
        return Ok(());
    }
    if current.is_some() {
        crate::lobby::leave_everything(st, sid)?;
    }
    if let Some(s) = st.sessions.get_mut(&sid) {
        s.channel = Some(real);
    }
    // DC-DEFAULT: CD-22 in-channel id = account number
    st.send(sid, wire::channel_enter_reply(true, account));
    let rec = wire::lobby_user_add_push(&st.lobby_user(account));
    let others: Vec<Sid> = st
        .lobby_audience(real)
        .into_iter()
        .filter(|&s| s != sid)
        .collect();
    st.send_all(&others, &rec);
    st.note(&format!("account {} entered clan channel {}", account, ch));
    Ok(())
}

/// 0x311 on the clan lobby: the map / mode masks of the ROM clan mode table (`[ClanMode<n>]`). Community maps
/// are not offered in clan rooms (MAPKIT_PLAN.md D12; MAP-35): the clan table stays the ROM's.
// SPEC: PROTOCOL_2009.md §3.5 0x311 (exactly 0x10a bytes, <= 30 pairs); DC-DEFAULT: CD-46 masks from the ROM room
// table, CLN-2 (proposed) the clan lobby uses the clan mode table
pub fn clan_room_settings_reply(st: &Core) -> Vec<u8> {
    // DC-DEFAULT: GMD-26 only the playable modes keep their bits
    let masks = crate::lobby::filter_masks(
        st,
        st.rom.game_room.map_mode_masks(true),
        true,
        &crate::lobby::policy(),
    );
    let n = masks.len().min(crate::lobby::ROOM_SETTINGS_MAX_PAIRS);
    let mut w = PktWriter::new(ROOM_SETTINGS_REPLY);
    // MAP-38: the same version the plain lobby announces (one value per mod set)
    w.u32(1)
        .u32(0)
        .u32(crate::lobby::ROOM_SETTINGS_KEY)
        .u32(st.room_settings_version())
        .u32(n as u32);
    for &(map, mask) in &masks[..n] {
        w.u32(map as u32).u32(mask);
    }
    w.at(0x10a);
    w.into_frame()
}

// ---------------------------------------------------------------------------------------------------------------
// clan rooms: two sides (home = the creator's clan, away = the first other clan to enter)
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sides {
    pub home: i64,
    pub away: i64,
}

pub fn sides(st: &Core, key: RoomKey) -> rusqlite::Result<Option<Sides>> {
    if !st.rooms.contains_key(&key) {
        return Ok(None);
    }
    st.db
        .query_row(
            "SELECT home, away FROM clan_rooms WHERE channel = ?1 AND room = ?2",
            params![key.channel as i64, key.no as i64],
            |r| {
                Ok(Sides {
                    home: r.get(0)?,
                    away: r.get(1)?,
                })
            },
        )
        .optional()
}

fn set_sides(st: &Core, key: RoomKey, s: Sides) -> rusqlite::Result<()> {
    st.db.execute(
        "INSERT INTO clan_rooms (channel, room, home, away) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(channel, room) DO UPDATE SET home = excluded.home, away = excluded.away",
        params![key.channel as i64, key.no as i64, s.home, s.away],
    )?;
    Ok(())
}

/// Room title on clan servers: two wstr[16] team names (home, away) in the wstr[33] title field.
// SPEC: PROTOCOL_2009.md §3.6 room_record +4 ("On clan servers: two wstr[16] team names (home, away)")
pub fn clan_title(home: &str, away: &str) -> String {
    let mut units: Vec<u16> = home.encode_utf16().take(15).collect();
    units.resize(16, 0);
    units.extend(away.encode_utf16().take(15));
    String::from_utf16_lossy(&units)
}

fn name_of_clan(st: &Core, uid: i64) -> String {
    if uid == 0 {
        return String::new();
    }
    clan_by_uid(&st.db, uid)
        .ok()
        .flatten()
        .map(|k| k.name)
        .unwrap_or_default()
}

/// Re-sends a room's lobby row (remove + add: the client appends 0x3031 without de-duplication, so the old row
/// goes first; SPEC: PROTOCOL_2009.md §3.6 0x3031 / 0x3032).
fn refresh_lobby_row(st: &mut Core, key: RoomKey) {
    let Some(row) = st.rooms.get(&key).map(|r| r.row()) else {
        return;
    };
    st.push_lobby(key.channel, &wire::u32_push(LOBBY_ROOM_REMOVE_PUSH, key.no));
    st.push_lobby(key.channel, &wire::lobby_room_add_push(&row));
}

fn retitle(st: &mut Core, key: RoomKey, s: Sides) {
    let t = clan_title(&name_of_clan(st, s.home), &name_of_clan(st, s.away));
    if let Some(r) = st.rooms.get_mut(&key) {
        r.title = t;
    }
    refresh_lobby_row(st, key);
}

fn member_clan(st: &Core, account: u32) -> i64 {
    st.character(account).map(|c| c.clan_uid).unwrap_or(0)
}

/// Drops the away clan when none of its members is left in the room.
fn normalize(st: &mut Core, key: RoomKey, s: Sides) -> Sides {
    if s.away == 0 {
        return s;
    }
    let accounts: Vec<u32> = st
        .rooms
        .get(&key)
        .map(|r| r.members.iter().map(|m| m.account).collect())
        .unwrap_or_default();
    if accounts.iter().any(|&a| member_clan(st, a) == s.away) {
        return s;
    }
    Sides {
        home: s.home,
        away: 0,
    }
}

// DC-DEFAULT: CLN-2 (proposed) a clan room belongs to its creator's clan (home, team 1); the map and goal are the
// clan mode's defaults from the ROM clan mode table (CD-25 for clan modes)
fn on_clan_room_create(st: &mut Core, sid: Sid, p: &[u8]) -> Result<bool, Fault> {
    let Some(account) = account_of(st, sid) else {
        return Ok(false);
    };
    let Some(home) = clan_of(st, account)? else {
        st.send(sid, wire::room_create_reply(false, 0, 0, 0));
        return Ok(true);
    };
    let before: Vec<RoomKey> = st.rooms.keys().copied().collect();
    crate::room::dispatch(st, sid, ROOM_CREATE, p)?;
    let Some(key) = st
        .sessions
        .get(&sid)
        .and_then(|s| s.room)
        .filter(|k| !before.contains(k))
    else {
        return Ok(true);
    };
    let modes = st.rom.game_room.modes(true).len();
    if let Some(r) = st.rooms.get(&key) {
        let mode = r.mode as usize;
        if mode < modes {
            let map = st.rom.game_room.default_map(mode, true).max(0) as u32;
            let goal = st.rom.game_room.default_goal(mode, true).max(0) as u32;
            if let Some(r) = st.rooms.get_mut(&key) {
                r.map = map;
                r.goal = goal;
            }
        }
    }
    let s = Sides {
        home: home.uid,
        away: 0,
    };
    set_sides(st, key, s)?;
    retitle(st, key, s);
    Ok(true)
}

// DC-DEFAULT: CLN-2 (proposed) entering a clan room: the home clan joins team 1, the first other clan becomes the
// away side (team 2), a third clan and players without a clan are refused (0x385 generic reason); a side holds at
// most half the room
fn on_clan_room_enter(st: &mut Core, sid: Sid, p: &[u8]) -> Result<bool, Fault> {
    let (no, _pw) = wire::parse_room_enter(p);
    let (Some(account), Some(ch)) = (account_of(st, sid), clan_channel_of(st, sid)) else {
        return Ok(false);
    };
    let key = RoomKey {
        channel: ch,
        no: no.max(0) as u32,
    };
    let Some(stored) = sides(st, key)? else {
        return Ok(false);
    };
    if st.sessions.get(&sid).and_then(|x| x.room) == Some(key) {
        return Ok(false);
    }
    let s = normalize(st, key, stored);
    let mine = member_clan(st, account);
    let team = if mine == 0 {
        None
    } else if mine == s.home {
        Some(1)
    } else if s.away == 0 || mine == s.away {
        Some(2)
    } else {
        None
    };
    let full = match (team, st.rooms.get(&key)) {
        (Some(t), Some(r)) => r.count_team(t) >= (r.max_players.div_ceil(2) as usize).max(1),
        _ => false,
    };
    let Some(team) = team.filter(|_| !full) else {
        st.send(sid, wire::room_enter_reply(false, wire::REASON_GENERIC, 0));
        return Ok(true);
    };
    crate::room::dispatch(st, sid, ROOM_ENTER, p)?;
    if st.sessions.get(&sid).and_then(|x| x.room) != Some(key) {
        return Ok(true);
    }
    let moved = match st.rooms.get_mut(&key).and_then(|r| r.member_mut(account)) {
        Some(m) if m.team != team => {
            m.team = team;
            true
        }
        _ => false,
    };
    if moved {
        st.push_room(key, &wire::room_team_push(account, team));
    }
    let new = if team == 2 && s.away == 0 {
        Sides {
            home: s.home,
            away: mine,
        }
    } else {
        s
    };
    if new != stored {
        set_sides(st, key, new)?;
        retitle(st, key, new);
    }
    Ok(true)
}

/// Members per side of a clan room.
fn side_counts(st: &Core, key: RoomKey) -> (usize, usize) {
    st.rooms
        .get(&key)
        .map(|r| (r.count_team(1), r.count_team(2)))
        .unwrap_or((0, 0))
}

// DC-DEFAULT: CLN-3 (proposed) the host's Start in a clan room needs an away clan and clan.start_min_per_side
// members on each side (0x393 ok = 0 otherwise); teams are fixed by clan (0x391 ok = 0)
fn on_clan_room_flag(st: &mut Core, sid: Sid, p: &[u8]) -> Result<bool, Fault> {
    let (kind, value) = wire::parse_user_flag(p);
    let (Some(account), Some(key)) = (
        account_of(st, sid),
        st.sessions.get(&sid).and_then(|s| s.room),
    ) else {
        return Ok(false);
    };
    if kind != 0 || value != 2 {
        return Ok(false);
    }
    let Some(s) = sides(st, key)? else {
        return Ok(false);
    };
    let waiting_host = st
        .rooms
        .get(&key)
        .map(|r| r.host == account && r.state == crate::room::RoomPhase::Waiting)
        .unwrap_or(false);
    if !waiting_host {
        return Ok(false);
    }
    // DC-DEFAULT: GMD-26 a clan room of an unplayable mode never starts
    if crate::lobby::start_refused(st, key) {
        crate::lobby::refuse_start(st, sid, key);
        return Ok(true);
    }
    let s = normalize(st, key, s);
    let (home_n, away_n) = side_counts(st, key);
    let min = st.rules.clan.start_min_per_side.max(1) as usize;
    if s.away == 0 || home_n < min || away_n < min {
        st.send(sid, wire::user_flag_reply(false, 0, 0));
        st.note(&format!(
            "clan room {}-{}: start refused (sides {} / {}, need {} each)",
            key.channel, key.no, home_n, away_n, min
        ));
        return Ok(true);
    }
    Ok(false)
}

fn in_clan_room(st: &Core, sid: Sid) -> rusqlite::Result<bool> {
    match st.sessions.get(&sid).and_then(|s| s.room) {
        Some(k) => Ok(sides(st, k)?.is_some()),
        None => Ok(false),
    }
}

/// The `room key=...` line of a clan room: the clan flag, both clan numbers and the CD-11 policy.
// SPEC: DSERVER_PIPE.md §3.2; DC-DEFAULT: CD-11 ClanRoomPolicy
pub fn room_line(st: &Core, key: RoomKey) -> Option<String> {
    let s = sides(st, key).ok().flatten()?;
    let r = st.rooms.get(&key)?;
    let vfs = st.rom.vfs.as_ref()?;
    let cfg = dc_bute::RoomConfig {
        map: r.map as usize,
        mode: r.mode as usize,
        clan: true,
        goal: r.goal as i32,
        ff: r.ff_on as i32,
        arms: r.weapon_limit as i32,
        home: c32(s.home),
        visitor: c32(s.away),
        home_team: 1,
        ..dc_bute::RoomConfig::default()
    };
    let opts = dc_bute::RoomOptions {
        clan_policy: st.rules.clan.policy.clone(),
        // DC-DEFAULT: GMD-2 (clarification) the clan rows take the same round-time fallback
        round_clock: st.rules.round_clock.clone(),
        hisec_prefix: None,
    };
    Some(dc_bute::room_line_with_vfs(
        vfs,
        &st.rom.game_room,
        &cfg,
        &opts,
    ))
}

/// World of a clan room from the clan mode table (`--dserver-world` still wins).
pub fn world(st: &Core, key: RoomKey) -> Option<String> {
    sides(st, key).ok().flatten()?;
    if !st.opts.dserver_world.is_empty() {
        return Some(st.opts.dserver_world.clone());
    }
    let r = st.rooms.get(&key)?;
    let dir = st.rom.dir.as_ref()?;
    Some(dc_bute::world_for_room(
        dir,
        &st.rom.game_room,
        r.map as usize,
        r.mode as usize,
        true,
        None,
    ))
}

// ---------------------------------------------------------------------------------------------------------------
// clan match bookkeeping
// ---------------------------------------------------------------------------------------------------------------

/// Clan points of one side from the ROM `.gms` of the clan mode (`[MapExp]` / `[MapMoney]` `clan_basicpoint` and
/// `clan_winnerpoint`), the CD-11 fallbacks when absent, times the mode weight.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClanPoints {
    pub exp_basic: i32,
    pub exp_winner: i32,
    pub money_basic: i32,
    pub money_winner: i32,
    pub weight: f64,
}

fn gms_i32(g: &dc_bute::ButeDoc, sec: &str, key: &str, dflt: i32) -> i32 {
    g.get(sec)
        .and_then(|s| s.get(key))
        .and_then(|v| v.trim().trim_end_matches(['f', 'F']).parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .map(|v| v as i32)
        .unwrap_or(dflt)
}

// DC-DEFAULT: CD-11 values present in the ROM always win; fallback basic / winner points; weight per mode key
pub fn clan_points(st: &Core, map: u32, mode: u32) -> ClanPoints {
    let p = &st.rules.clan.policy;
    let (bp, wp) = (p.basic_point_default, p.winner_point_default);
    let Some(vfs) = &st.rom.vfs else {
        return ClanPoints {
            exp_basic: bp,
            exp_winner: wp,
            money_basic: bp,
            money_winner: wp,
            weight: p.weight_for("CDM"),
        };
    };
    let (md, g) =
        dc_bute::gms_file_with_vfs(vfs, &st.rom.game_room, map as usize, mode as usize, true);
    let key = md.get("Key").map(|s| s.as_str()).unwrap_or("CDM");
    ClanPoints {
        exp_basic: gms_i32(&g, "MapExp", "clan_basicpoint", bp),
        exp_winner: gms_i32(&g, "MapExp", "clan_winnerpoint", wp),
        money_basic: gms_i32(&g, "MapMoney", "clan_basicpoint", bp),
        money_winner: gms_i32(&g, "MapMoney", "clan_winnerpoint", wp),
        weight: p.weight_for(key),
    }
}

/// What one side gains from a clan match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SideGain {
    pub exp: i64,
    pub points: i64,
}

/// Clan exp = the side's ROM clan exp points (basic, + winner for the winner) times the mode weight, plus the match
/// exp its members earned through `lobby.calc_exp_gp`; clan points = the side's ROM clan money points times the
/// weight.
// DC-DEFAULT: CLN-4 (proposed) clan match bookkeeping over CD-11 and the lobby.calc_exp_gp results
pub fn side_gain(pts: &ClanPoints, won: bool, member_exp: i64) -> SideGain {
    let w = if pts.weight.is_finite() && pts.weight >= 0.0 {
        pts.weight
    } else {
        1.0
    };
    let e = pts.exp_basic as f64 + if won { pts.exp_winner as f64 } else { 0.0 };
    let m = pts.money_basic as f64 + if won { pts.money_winner as f64 } else { 0.0 };
    SideGain {
        exp: ((e * w).round() as i64).max(0) + member_exp.max(0),
        points: ((m * w).round() as i64).max(0),
    }
}

/// Called by `game::finish` after the players' results are stored: updates both clans' records of a clan room.
// DC-DEFAULT: CLN-4 (proposed): win / loss / draw per clan by the @result winner team (1 home, 2 away, 0 draw)
pub fn on_match_result(
    st: &mut Core,
    key: RoomKey,
    map: u32,
    mode: u32,
    r: &MatchResult,
    players: &[PlayerResult],
) -> HResult {
    let Some(s) = st
        .db
        .query_row(
            "SELECT home, away FROM clan_rooms WHERE channel = ?1 AND room = ?2",
            params![key.channel as i64, key.no as i64],
            |x| {
                Ok(Sides {
                    home: x.get(0)?,
                    away: x.get(1)?,
                })
            },
        )
        .optional()?
    else {
        return Ok(());
    };
    if s.home == 0 || s.away == 0 {
        return Ok(());
    }
    let (exp_factor, gp_factor) = st.rom.map_factors(map, mode);
    let mode_key = if r.mode.is_empty() {
        st.rom.mode_key(mode)
    } else {
        r.mode.clone()
    };
    let mut member_exp = [0i64; 2];
    for p in players {
        if !(1..=2).contains(&p.team) {
            continue;
        }
        let ctx = RewardCtx {
            account: p.uid,
            kills: p.kills,
            deaths: p.deaths,
            headshots: p.headshots,
            team_kills: p.team_kills,
            team: p.team,
            winner: r.winner,
            player_win: p.win,
            agent_win: p.agent,
            objective: 0,
            map,
            mode,
            mode_key: mode_key.clone(),
            exp_factor,
            gp_factor,
            reported_exp: p.exp,
            reported_gp: p.gp,
        };
        member_exp[(p.team - 1) as usize] += rewards::calc(st, &ctx).0;
    }
    let pts = clan_points(st, map, mode);
    let outcome = |team: i64| -> (i64, i64, i64) {
        match r.winner {
            0 => (0, 0, 1),
            w if w == team => (1, 0, 0),
            _ => (0, 1, 0),
        }
    };
    let mut gains = [SideGain::default(); 2];
    for (i, uid) in [s.home, s.away].into_iter().enumerate() {
        let team = i as i64 + 1;
        let (w, l, d) = outcome(team);
        let g = side_gain(&pts, w == 1, member_exp[i]);
        gains[i] = g;
        st.db.execute(
            "INSERT OR IGNORE INTO clan_records (clan_uid) VALUES (?1)",
            [uid],
        )?;
        st.db.execute(
            "UPDATE clan_records SET wins = wins + ?2, losses = losses + ?3, draws = draws + ?4, exp = exp + ?5, \
             points = points + ?6 WHERE clan_uid = ?1",
            params![uid, w, l, d, g.exp, g.points],
        )?;
    }
    let winner_clan = match r.winner {
        1 => s.home,
        2 => s.away,
        _ => 0,
    };
    st.db.execute(
        "INSERT INTO clan_matches (ended, channel, room, home, away, winner_clan, home_exp, away_exp, home_points, \
         away_points) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            crate::clock::unix_now(),
            key.channel as i64,
            key.no as i64,
            s.home,
            s.away,
            winner_clan,
            gains[0].exp,
            gains[1].exp,
            gains[0].points,
            gains[1].points
        ],
    )?;
    st.note(&format!(
        "clan match {}-{}: clans {} vs {}, winner clan {}, gains {:?}",
        key.channel, key.no, s.home, s.away, winner_clan, gains
    ));
    Ok(())
}

/// Start-up step 7 (SPEC: LOBBY_HARNESS.md §2): rooms are gone, so their clan sides are dropped; every clan's
/// current rank becomes its previous rank.
// DC-DEFAULT: CLN-11 (proposed) previous rank = rank at server start
pub fn startup(st: &mut Core) -> HResult {
    st.db.execute("DELETE FROM clan_rooms", [])?;
    st.db.execute(
        "INSERT OR IGNORE INTO clan_records (clan_uid) SELECT clan_uid FROM clans",
        [],
    )?;
    let clans = all_clans(&st.db)?;
    for (i, k) in clans.iter().enumerate() {
        st.db.execute(
            "UPDATE clan_records SET prev_rank = ?2 WHERE clan_uid = ?1",
            params![k.uid, i as i64 + 1],
        )?;
    }
    st.note(&format!("clans: {} ranked by clan exp", clans.len()));
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// dispatch
// ---------------------------------------------------------------------------------------------------------------

/// Clan messages and the clan lobby's own rules; false when `id` is left to the other modules.
pub fn dispatch(st: &mut Core, sid: Sid, id: u16, p: &[u8]) -> Result<bool, Fault> {
    match id {
        CLAN_LOGIN => on_clan_login(st, sid)?,
        CLAN_INFO => on_info(st, sid)?,
        CLAN_MEMBERS => on_members(st, sid)?,
        CLAN_NOTICE => on_notice(st, sid)?,
        CLAN_NOTICE_SET => on_notice_set(st, sid, p)?,
        CLAN_LEADER_CHECK => on_leader_check(st, sid, p)?,
        CLAN_NAME_CHECK => on_name_check(st, sid, p)?,
        CLAN_MARK_CHECK => on_mark_check(st, sid, p)?,
        CHAT_SEND => return bang_command(st, sid, p),
        CHANNEL_LIST if is_clan_session(st, sid) => {
            let ch = clan_channels(st);
            st.send(sid, wire::channel_list_reply(&ch));
        }
        CHANNEL_ENTER if is_clan_session(st, sid) => on_clan_channel_enter(st, sid, p)?,
        ROOM_SETTINGS_FETCH if is_clan_session(st, sid) => {
            let f = clan_room_settings_reply(st);
            st.send(sid, f);
        }
        ROOM_CREATE if clan_channel_of(st, sid).is_some() => {
            return on_clan_room_create(st, sid, p)
        }
        ROOM_ENTER if clan_channel_of(st, sid).is_some() => return on_clan_room_enter(st, sid, p),
        TEAM_CHANGE if in_clan_room(st, sid)? => st.send(sid, wire::team_change_reply(false, 0)),
        ROOM_USER_FLAG if in_clan_room(st, sid)? => return on_clan_room_flag(st, sid, p),
        _ => return Ok(false),
    }
    Ok(true)
}

#[cfg(test)]
#[path = "clan_tests.rs"]
mod tests;
