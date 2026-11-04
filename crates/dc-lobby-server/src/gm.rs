//! Operator (GM) tools: the three request packets the 2009 chat box builds from an operator's `/` lines
//! (0x3d0 restrict, 0x3d2 lookup, 0x3d4 mute) and their replies, the mute-end push 0x3053, observer mode
//! 0x30e / 0x30f, the banned-login refusal (0x201 reason 0x00030089) and the server-side mute filter on chat.
//! Also the per-session hooks the lobby calls at login / disconnect, and the CD-64 moderator flag for game servers.
// SPEC: PROTOCOL_2009.md §6.4 (operator commands), §3.4 0x30e / 0x30f, §2.2 0x201 reason 0x89
// DC-DEFAULT: CD-07 GM flag in the account row (accounts.gm != 0); kick, ban with duration, trace, mute
// DC-DEFAULT: CD-34 every op request is answered with a result and, on failure, a reason the client knows

use crate::clock;
use crate::db;
use crate::lobby;
use crate::msg::{CHAT_SEND, LOGIN_REPLY};
use crate::room::RoomPhase;
use crate::state::{Core, Fault, HResult, Sid};
use crate::wire::{self, LoginReply};
use dc_wire::{PktReader, PktWriter};
use rusqlite::{params, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};

// message ids (numbers are 2009 wire facts; names are ours, SPEC: PROTOCOL_2009.md Appendix A)
pub const OBSERVER_SET: u16 = 0x30e;
pub const OBSERVER_SET_REPLY: u16 = 0x30f;
pub const OP_RESTRICT: u16 = 0x3d0;
pub const OP_RESTRICT_REPLY: u16 = 0x3d1;
pub const OP_LOOKUP: u16 = 0x3d2;
pub const OP_LOOKUP_REPLY: u16 = 0x3d3;
pub const OP_MUTE: u16 = 0x3d4;
pub const OP_MUTE_REPLY: u16 = 0x3d5;
pub const MUTE_UNTIL_PUSH: u16 = 0x3053;

/// 0x3d1 +0xa: the restrict target is unknown (`res 0x1809`).
// EVIDENCE: cshell+0x3e0f0
pub const REASON_RESTRICT_UNKNOWN: u16 = 0x5d;
/// 0x3d5 +0xa: the mute target is unknown (`res 0x1809`).
// EVIDENCE: cshell+0x3e300
pub const REASON_MUTE_UNKNOWN: u16 = 0x87;
/// 0x3d3 +0xa: invalid query / no such user.
// EVIDENCE: cshell+0x40dd0
pub const REASON_BAD_QUERY: u16 = 0x6e;
pub const REASON_LOOKUP_NOT_FOUND: u16 = 0x6f;
/// 0x201 reason of a banned account: the client words its box with the day count at +0x2a.
// SPEC: PROTOCOL_2009.md §2.2 0x201 reason (low word 0x89 uses +0x2a; EVIDENCE: cshell+0x42450)
pub const REASON_LOGIN_BANNED: u32 = 0x0003_0089;

/// 0x3d0 actions (+0x28): 0 lifts, 1 / 2 are the two restrict commands of the chat box.
// DC-DEFAULT: OPS-3 (proposed) 1 = kick (drop the connection), 2 = ban for `period` days, 0 = lift the ban
pub const ACTION_LIFT: u32 = 0;
pub const ACTION_KICK: u32 = 1;
pub const ACTION_BAN: u32 = 2;

/// Per-session operator state kept beside the sessions (not in the DB: it ends with the connection).
#[derive(Debug, Default)]
pub struct OpsState {
    /// Lobby sessions in observer mode (CD-43).
    pub observers: BTreeSet<Sid>,
    /// Unix times of the stored reports per reporter account, for the hourly cap (OPS-7).
    pub report_times: BTreeMap<u32, Vec<i64>>,
}

// ---------------------------------------------------------------------------------------------------------------
// layouts
// ---------------------------------------------------------------------------------------------------------------

/// Parsed 0x3d0: +6 wstr[17] target; +0x28 u32 action; +0x2c i32 period; +0x30 wstr[65] reason.
// SPEC: PROTOCOL_2009.md §6.4 0x3d0 (EVIDENCE: cshell+0x42920 -> cshell+0x18e790)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestrictReq {
    pub target: String,
    pub action: u32,
    pub period: i32,
    pub reason: String,
}

pub fn parse_restrict(p: &[u8]) -> RestrictReq {
    let mut r = PktReader::new(p);
    let target = r.at(6).wstr(17);
    let action = r.at(0x28).u32();
    let period = r.i32();
    let reason = r.at(0x30).wstr(65);
    RestrictReq {
        target,
        action,
        period,
        reason,
    }
}

/// Parsed 0x3d4: +6 wstr[17] target; +0x28 u32 on; +0x2c i32 minutes.
// SPEC: PROTOCOL_2009.md §6.4 0x3d4 (EVIDENCE: cshell+0x40790 -> cshell+0x18e910)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuteReq {
    pub target: String,
    pub on: bool,
    pub minutes: i32,
}

pub fn parse_mute(p: &[u8]) -> MuteReq {
    let mut r = PktReader::new(p);
    let target = r.at(6).wstr(17);
    let on = r.at(0x28).u32() != 0;
    let minutes = r.i32();
    MuteReq {
        target,
        on,
        minutes,
    }
}

/// Parsed 0x3d2 selector: 0 account number (u32 at +0xa), 1 login id, 2 character name (wstr[17] at +0xa).
// SPEC: PROTOCOL_2009.md §6.4 0x3d2 (EVIDENCE: cshell+0x40980 -> cshell+0x18e850)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupKey {
    Account(u32),
    Login(String),
    Nick(String),
    Invalid,
}

pub fn parse_lookup(p: &[u8]) -> LookupKey {
    let mut r = PktReader::new(p);
    match r.at(6).i32() {
        0 => LookupKey::Account(r.at(0xa).u32()),
        1 => LookupKey::Login(r.at(0xa).wstr(17)),
        2 => LookupKey::Nick(r.at(0xa).wstr(17)),
        _ => LookupKey::Invalid,
    }
}

/// `{+6 u32 result, +0xa u16 reason}` of 0x3d1 / 0x3d5 (>= 0xc bytes).
// SPEC: PROTOCOL_2009.md §6.4 0x3d1 / 0x3d5 (EVIDENCE: cshell+0x3e0f0, cshell+0x3e300)
pub fn result_reply(id: u16, ok: bool, reason: u16) -> Vec<u8> {
    let mut w = PktWriter::new(id);
    w.u32(ok as u32).u16(reason);
    w.at(0xc);
    w.into_frame()
}

/// What 0x3d3 reports about one account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LookupView {
    pub account: u32,
    pub login: String,
    pub nick: String,
    pub address: String,
    pub online: bool,
    pub server_kind: u32,
    pub server_index: i32,
    pub channel: i32,
    pub room: i32,
    pub in_match: bool,
}

/// 0x3d3 (0x90): +6 result; +0xa u16 reason; +0xe account; +0x12 wstr[17] login; +0x34 wstr[17] name;
/// +0x56 wstr[17] address; +0x78 online; +0x7c server kind; +0x80 server index; +0x84 channel; +0x88 room;
/// +0x8c in a match.
// SPEC: PROTOCOL_2009.md §6.4 0x3d3 (EVIDENCE: cshell+0x40dd0)
pub fn lookup_reply(v: Option<&LookupView>, reason: u16) -> Vec<u8> {
    let mut w = PktWriter::new(OP_LOOKUP_REPLY);
    match v {
        Some(v) => {
            w.u32(1).u16(0);
            w.at(0xe)
                .u32(v.account)
                .wstr(&v.login, 17)
                .wstr(&v.nick, 17)
                .wstr(&v.address, 17);
            w.at(0x78)
                .u32(v.online as u32)
                .u32(v.server_kind)
                .i32(v.server_index)
                .i32(v.channel)
                .i32(v.room);
            w.u32(v.in_match as u32);
        }
        None => {
            w.u32(0).u16(reason);
        }
    }
    w.at(0x90);
    w.into_frame()
}

/// 0x3053 (0xe): +6 FILETIME (local) chat-muted-until, 0 = not muted.
// SPEC: PROTOCOL_2009.md §6.4 0x3053 (EVIDENCE: cshell+0x361c0 -> cshell+0x1b1620)
pub fn mute_until_push(until: u64) -> Vec<u8> {
    let mut w = PktWriter::new(MUTE_UNTIL_PUSH);
    w.u64(until);
    w.into_frame()
}

// ---------------------------------------------------------------------------------------------------------------
// rules
// ---------------------------------------------------------------------------------------------------------------

/// The CD-07 GM flag of an account, read from the DB on every request (an operator may change it live).
pub fn is_gm(st: &Core, account: u32) -> bool {
    db::account_by_no(&st.db, account)
        .ok()
        .flatten()
        .map(|a| a.gm != 0)
        .unwrap_or(false)
}

/// 0x201 +0x26 value the 2009 client takes as the operator flag (`== 2` opens the operator chat commands, the
/// room "Kick User" entry and the operator slots of the server and channel lists).
// SPEC: PROTOCOL_2009.md §2.2 0x201 +0x26 (EVIDENCE: cshell+0x42450)
pub const OPERATOR_PRIVILEGE: i32 = 2;

/// LG-170: the 0x201 privilege of an account row: `accounts.gm` 0 = player (0), any other value = GM (2).
// DC-DEFAULT: LG-170 accounts.gm != 0 is a GM account and logs in with privilege 2
pub fn privilege_of(gm_column: i64) -> i32 {
    if gm_column != 0 {
        OPERATOR_PRIVILEGE
    } else {
        0
    }
}

/// True for a character name the 2009 client draws as a GM: it compares the first two units with "GM", case
/// sensitive (rank icon, yellow chat, local kick immunity, no vote kick).
// SPEC: PROTOCOL_2009.md §6.5 GM name prefix
pub fn gm_named(nick: &str) -> bool {
    nick.starts_with("GM")
}

/// LG-172: a name with the GM prefix is reserved for GM accounts (`account` None = a new account, not a GM).
// DC-DEFAULT: LG-172 character names beginning with "GM" only for GM accounts
pub fn name_reserved_for(st: &Core, account: Option<u32>, nick: &str) -> bool {
    // LG-318: the pool's prefix is for the pool, whose characters are made without this check
    crate::botpool::pool_named(nick) || gm_named(nick) && !account.is_some_and(|a| is_gm(st, a))
}

/// LG-173: a GM account or a GM-named character cannot be kicked from a room.
// DC-DEFAULT: LG-173 kick immunity of GM accounts and GM-named characters
pub fn kick_immune(st: &Core, account: u32) -> bool {
    is_gm(st, account) || st.character(account).is_some_and(|c| gm_named(&c.nick))
}

/// LG-175: operator slots per server-list and channel-list entry (+0x98 / +0x52).
// DC-DEFAULT: LG-175 ten operator slots per server and per channel
pub const OPERATOR_SLOTS: u32 = 10;

/// The accounts with the GM flag (one query; the set is small).
pub fn gm_accounts(st: &Core) -> BTreeSet<u32> {
    let Ok(mut q) = st
        .db
        .prepare_cached("SELECT user_no FROM accounts WHERE gm != 0")
    else {
        return BTreeSet::new();
    };
    q.query_map([], |r| r.get::<_, i64>(0))
        .map(|rows| rows.flatten().map(|n| n as u32).collect())
        .unwrap_or_default()
}

/// GM accounts among the live lobby sessions, optionally only those in channel `ch` (the operator-current fields).
pub fn gms_online(st: &Core, ch: Option<u32>) -> u32 {
    let gms = gm_accounts(st);
    if gms.is_empty() {
        return 0;
    }
    st.sessions
        .values()
        .filter(|s| s.in_lobby && !s.closing && (ch.is_none() || s.channel == ch))
        .filter(|s| s.account.is_some_and(|a| gms.contains(&a)))
        .count() as u32
}

fn lobby_account(st: &Core, sid: Sid) -> Option<u32> {
    st.sessions
        .get(&sid)
        .filter(|s| s.in_lobby && !s.closing)
        .and_then(|s| s.account)
}

/// The GM account of a lobby session, if the session belongs to one.
pub fn gm_of(st: &Core, sid: Sid) -> Option<u32> {
    lobby_account(st, sid).filter(|&a| is_gm(st, a))
}

/// Character found by the name an operator typed (CD-16 case rule).
fn target_by_nick(st: &Core, nick: &str) -> Result<Option<db::Character>, Fault> {
    let n = nick.trim();
    if n.is_empty() {
        return Ok(None);
    }
    Ok(db::character_by_nick(
        &st.db,
        n,
        st.rules.names_case_insensitive,
    )?)
}

/// Every live connection of an account (lobby and clan lobby).
fn sessions_of(st: &Core, account: u32) -> Vec<Sid> {
    st.sessions
        .values()
        .filter(|s| !s.closing && s.account == Some(account))
        .map(|s| s.sid)
        .collect()
}

/// Drops every connection of an account: it leaves its room and channel first, so the others see it go.
pub fn drop_account(st: &mut Core, account: u32) -> Result<usize, Fault> {
    let sids = sessions_of(st, account);
    for &s in &sids {
        crate::messenger::on_disconnect(st, s)?;
        lobby::leave_everything(st, s)?;
        st.ops.observers.remove(&s);
        st.close(s);
    }
    // a key handed out before the drop is no longer good
    st.tickets.retain(|_, t| t.account != account);
    Ok(sids.len())
}

/// Writes a ban of `target` (`until` unix seconds, 0 = permanent) and drops its connections; returns how many
/// were dropped. `by_no` is the GM account (0 = the dashboard), `by_name` who did it when that is not a GM account.
/// Shared by the 0x3d0 ban and the dashboard's ban (playeradmin.rs).
pub fn put_ban(
    st: &mut Core,
    target: u32,
    until: i64,
    reason: &str,
    by_no: u32,
    by_name: &str,
) -> Result<usize, Fault> {
    st.db.execute(
        "INSERT OR REPLACE INTO op_bans (user_no, until, reason, by_no, by_name, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![target as i64, until, reason, by_no as i64, by_name, clock::unix_now()],
    )?;
    drop_account(st, target)
}

/// Lifts the ban of `target`; false when it had none.
pub fn lift_ban(st: &Core, target: u32) -> rusqlite::Result<bool> {
    Ok(st
        .db
        .execute("DELETE FROM op_bans WHERE user_no = ?1", [target as i64])?
        > 0)
}

/// The length of a mute asked for `requested` minutes: the default when none (0 or less), else at most the maximum.
// DC-DEFAULT: OPS-4 (proposed)
pub fn mute_minutes(r: &crate::opscfg::OpsRules, requested: i64) -> i64 {
    if requested <= 0 {
        r.default_mute_minutes
    } else {
        requested.min(r.max_mute_minutes)
    }
}

/// Stores the mute end of `target` (`minutes` 0 lifts it) and tells its lobby sessions (0x3053); returns the end.
/// Shared by the 0x3d4 mute and the dashboard's mute (playeradmin.rs).
pub fn set_mute(st: &mut Core, target: u32, minutes: i64) -> Result<i64, Fault> {
    let until = if minutes > 0 {
        clock::now_ticks().saturating_add(minutes.saturating_mul(60 * clock::TICKS_PER_SEC))
    } else {
        0
    };
    st.db.execute(
        "UPDATE characters SET mute_until = ?2 WHERE user_no = ?1",
        params![target as i64, until],
    )?;
    let push = mute_until_push(until.max(0) as u64);
    for s in st.sids_of(target) {
        st.send(s, push.clone());
    }
    Ok(until)
}

/// One row of the operator action log (`op_actions`); `by` 0 = the dashboard.
pub fn log_action(
    st: &Core,
    by: u32,
    action: &str,
    target: u32,
    period: i64,
    reason: &str,
) -> rusqlite::Result<()> {
    st.db.execute(
        "INSERT INTO op_actions (by_no, action, target, period, reason, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![by as i64, action, target as i64, period, reason, clock::unix_now()],
    )?;
    Ok(())
}

/// Active ban of an account: `Some(until)` (unix seconds, 0 = permanent); an expired ban is removed.
pub fn active_ban(st: &Core, account: u32) -> rusqlite::Result<Option<i64>> {
    let until: Option<i64> = st
        .db
        .query_row(
            "SELECT until FROM op_bans WHERE user_no = ?1",
            [account as i64],
            |r| r.get(0),
        )
        .optional()?;
    match until {
        Some(u) if u != 0 && u <= clock::unix_now() => {
            st.db
                .execute("DELETE FROM op_bans WHERE user_no = ?1", [account as i64])?;
            Ok(None)
        }
        other => Ok(other),
    }
}

/// Whole days left of a ban, rounded up (the 0x201 +0x2a day count).
// DC-DEFAULT: OPS-3 (proposed) remaining days rounded up; a permanent ban reports gm.permanent_ban_days
pub fn ban_days_left(until: i64, now: i64, permanent_days: u32) -> u32 {
    if until == 0 {
        return permanent_days;
    }
    let left = (until - now).max(0);
    ((left + 86_399) / 86_400).clamp(1, u32::MAX as i64) as u32
}

/// Called by the auth login after the password check: a banned account gets 0x201 reason 0x00030089 with the
/// remaining days and nothing else. Returns true when the login was refused.
// DC-DEFAULT: CD-07 ban = login refused until expiry
pub fn refuse_banned_login(st: &mut Core, sid: Sid, account: u32) -> Result<bool, Fault> {
    let Some(until) = active_ban(st, account)? else {
        return Ok(false);
    };
    let days = ban_days_left(until, clock::unix_now(), st.rules.ops.permanent_ban_days);
    let r = LoginReply {
        reason: REASON_LOGIN_BANNED,
        account,
        days,
        ..Default::default()
    };
    debug_assert_eq!(LOGIN_REPLY, 0x201);
    st.send(sid, wire::login_reply(&r));
    st.note(&format!(
        "login refused: account {} is banned ({} day(s) left)",
        account, days
    ));
    Ok(true)
}

/// Active mute end (local FILETIME) of an account, 0 when not muted.
pub fn mute_end(st: &Core, account: u32) -> i64 {
    st.character(account)
        .map(|c| c.mute_until)
        .filter(|&m| m > clock::now_ticks())
        .unwrap_or(0)
}

/// `" mod=1"` for a moderator account on the game-server pipe `user` line (an optional trailing token, SPEC:
/// DSERVER_PIPE.md §3.1), so a game server can run remote console commands only for moderators.
// DC-DEFAULT: CD-64 remote console only for accounts with the GM (moderator) flag
pub fn moderator_token(st: &Core, account: u32) -> String {
    if st.rules.ops.flag_moderators_to_game && is_gm(st, account) {
        " mod=1".to_string()
    } else {
        String::new()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------------------------------------------

// DC-DEFAULT: OPS-3 (proposed) restrict actions; CD-34 reason 0x5d for an unknown target
fn on_restrict(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let req = parse_restrict(p);
    let Some(gm) = gm_of(st, sid) else {
        st.note(&format!(
            "op restrict from a non-operator connection {} ignored",
            sid
        ));
        st.send(sid, result_reply(OP_RESTRICT_REPLY, false, 0));
        return Ok(());
    };
    let Some(target) = target_by_nick(st, &req.target)? else {
        st.send(
            sid,
            result_reply(OP_RESTRICT_REPLY, false, REASON_RESTRICT_UNKNOWN),
        );
        return Ok(());
    };
    let t = target.user_no;
    match req.action {
        ACTION_LIFT => {
            lift_ban(st, t)?;
            log_action(st, gm, "unban", t, 0, &req.reason)?;
            st.note(&format!(
                "op: account {} lifted the ban of {} ({})",
                gm, target.nick, t
            ));
        }
        ACTION_KICK => {
            if sessions_of(st, t).is_empty() {
                st.send(
                    sid,
                    result_reply(OP_RESTRICT_REPLY, false, REASON_RESTRICT_UNKNOWN),
                );
                return Ok(());
            }
            let n = drop_account(st, t)?;
            log_action(st, gm, "kick", t, 0, &req.reason)?;
            st.note(&format!(
                "op: account {} kicked {} ({} connection(s)): {}",
                gm, target.nick, n, req.reason
            ));
        }
        ACTION_BAN => {
            let now = clock::unix_now();
            let days = req.period as i64;
            let until = if days <= 0 {
                0
            } else {
                now.saturating_add(days.saturating_mul(86_400))
            };
            let n = put_ban(st, t, until, &req.reason, gm, "")?;
            log_action(st, gm, "ban", t, days, &req.reason)?;
            st.note(&format!(
                "op: account {} banned {} for {} day(s) ({} connection(s) dropped): {}",
                gm, target.nick, days, n, req.reason
            ));
        }
        other => {
            st.note(&format!(
                "op restrict: unknown action {} from account {}",
                other, gm
            ));
            st.send(sid, result_reply(OP_RESTRICT_REPLY, false, 0));
            return Ok(());
        }
    }
    st.send(sid, result_reply(OP_RESTRICT_REPLY, true, 0));
    Ok(())
}

// DC-DEFAULT: CD-33 the mute end is sent to the client (0x3053) and stored for 0x303 +0xec; OPS-4 (proposed)
fn on_mute(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let req = parse_mute(p);
    let Some(gm) = gm_of(st, sid) else {
        st.note(&format!(
            "op mute from a non-operator connection {} ignored",
            sid
        ));
        st.send(sid, result_reply(OP_MUTE_REPLY, false, 0));
        return Ok(());
    };
    let Some(target) = target_by_nick(st, &req.target)? else {
        st.send(sid, result_reply(OP_MUTE_REPLY, false, REASON_MUTE_UNKNOWN));
        return Ok(());
    };
    let t = target.user_no;
    let minutes = if req.on {
        mute_minutes(&st.rules.ops, req.minutes as i64)
    } else {
        0
    };
    set_mute(st, t, minutes)?;
    log_action(
        st,
        gm,
        if req.on { "mute" } else { "unmute" },
        t,
        minutes,
        "",
    )?;
    st.send(sid, result_reply(OP_MUTE_REPLY, true, 0));
    st.note(&format!(
        "op: account {} {} {} ({} min)",
        gm,
        if req.on { "muted" } else { "unmuted" },
        target.nick,
        minutes
    ));
    Ok(())
}

/// The lookup report of one account.
// DC-DEFAULT: OPS-5 (proposed) server index 0 (this lobby is the first entry of the client's list), channel and
// room 0-based, -1 when not there; the address only when gm.lookup_shows_address
pub fn lookup_view(st: &Core, account: u32) -> Option<LookupView> {
    let acc = db::account_by_no(&st.db, account).ok().flatten()?;
    let c = st.character(account)?;
    let sess = st
        .sessions
        .values()
        .find(|s| s.in_lobby && !s.closing && s.account == Some(account));
    let mut v = LookupView {
        account,
        login: acc.login,
        nick: c.nick,
        server_index: -1,
        channel: -1,
        room: -1,
        ..Default::default()
    };
    if let Some(s) = sess {
        v.online = true;
        v.server_index = 0;
        if st.rules.ops.lookup_shows_address {
            v.address = s
                .peer
                .rsplit_once(':')
                .map(|(h, _)| h.to_string())
                .unwrap_or_else(|| s.peer.clone());
        }
        if let Some(ch) = s.channel {
            v.channel = ch as i32 - 1;
        }
        if let Some(k) = s.room {
            v.channel = k.channel as i32 - 1;
            v.room = k.no as i32;
            v.in_match = st
                .rooms
                .get(&k)
                .map(|r| r.state == RoomPhase::Playing)
                .unwrap_or(false);
        }
    }
    Some(v)
}

// SPEC: PROTOCOL_2009.md §6.4 0x3d2 / 0x3d3; CD-34 reasons 0x6e / 0x6f
fn on_lookup(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let key = parse_lookup(p);
    let Some(gm) = gm_of(st, sid) else {
        st.note(&format!(
            "op lookup from a non-operator connection {} ignored",
            sid
        ));
        st.send(sid, lookup_reply(None, 0));
        return Ok(());
    };
    let account = match &key {
        LookupKey::Account(a) => Some(*a),
        LookupKey::Login(l) => {
            db::account_by_login(&st.db, &crate::auth::normalize_login(l))?.map(|a| a.user_no)
        }
        LookupKey::Nick(n) => target_by_nick(st, n)?.map(|c| c.user_no),
        LookupKey::Invalid => {
            st.send(sid, lookup_reply(None, REASON_BAD_QUERY));
            return Ok(());
        }
    };
    match account.and_then(|a| lookup_view(st, a)) {
        Some(v) => {
            log_action(st, gm, "lookup", v.account, 0, "")?;
            st.send(sid, lookup_reply(Some(&v), 0));
        }
        None => st.send(sid, lookup_reply(None, REASON_LOOKUP_NOT_FOUND)),
    }
    Ok(())
}

// DC-DEFAULT: CD-43 only operator accounts may enter observer mode; 0x30f result
fn on_observer(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let on = PktReader::new(p).at(6).u32() != 0;
    let ok = gm_of(st, sid).is_some();
    if ok {
        if on {
            st.ops.observers.insert(sid);
        } else {
            st.ops.observers.remove(&sid);
        }
    }
    st.send(sid, wire::ok_reply(OBSERVER_SET_REPLY, ok, 0xa));
    Ok(())
}

/// Server-side mute: chat from a muted account is dropped and the sender is told its mute end again.
/// Lines that start with `!` or `/` are commands, not chat, and go on to their handlers.
// DC-DEFAULT: CD-33 a muted player's chat is dropped by the server, not only by the client
fn filter_chat(st: &mut Core, sid: Sid, p: &[u8]) -> Result<bool, Fault> {
    let Some(account) = lobby_account(st, sid) else {
        return Ok(false);
    };
    let text = wire::parse_chat(p);
    let t = text.trim_start();
    if t.starts_with('!') || t.starts_with('/') {
        return Ok(false);
    }
    let end = mute_end(st, account);
    if end == 0 {
        return Ok(false);
    }
    st.send(sid, mute_until_push(end as u64));
    st.note(&format!("chat from muted account {} dropped", account));
    Ok(true)
}

/// Lobby login hook (after 0x301): operator notices and the messenger state of the new session.
pub fn on_lobby_login(st: &mut Core, sid: Sid, account: u32) -> HResult {
    crate::notices::on_lobby_login(st, sid, account)?;
    crate::messenger::on_lobby_login(st, sid, account)?;
    Ok(())
}

/// Connection-loss hook (before the session leaves its room and channel).
pub fn on_disconnect(st: &mut Core, sid: Sid) -> HResult {
    st.ops.observers.remove(&sid);
    crate::messenger::on_disconnect(st, sid)?;
    Ok(())
}

/// GM messages and the chat mute filter; returns false when `id` is not handled here.
pub fn dispatch(st: &mut Core, sid: Sid, id: u16, p: &[u8]) -> Result<bool, Fault> {
    match id {
        OP_RESTRICT => on_restrict(st, sid, p)?,
        OP_LOOKUP => on_lookup(st, sid, p)?,
        OP_MUTE => on_mute(st, sid, p)?,
        OBSERVER_SET => on_observer(st, sid, p)?,
        CHAT_SEND => return filter_chat(st, sid, p),
        _ => return Ok(false),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::u32_at;

    fn split(f: &[u8]) -> (i32, Vec<u8>) {
        let (frames, used) = dc_wire::parse_tcp_frames(f).unwrap();
        assert_eq!(used, f.len());
        frames.into_iter().next().unwrap()
    }

    fn u16_at(p: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([p[o], p[o + 1]])
    }

    fn ws(p: &[u8], o: usize, n: usize) -> String {
        PktReader::new(p).at(o).wstr(n)
    }

    #[test]
    fn request_layouts() {
        // SPEC: PROTOCOL_2009.md §6.4: 0x3d0 / 0x3d4 are 0xb2 bytes
        let mut w = PktWriter::new(OP_RESTRICT);
        w.wstr("Bob", 17).u32(2).i32(3).wstr("spam", 65);
        let p = w.into_payload();
        assert_eq!(p.len(), 0xb2);
        assert_eq!(
            parse_restrict(&p),
            RestrictReq {
                target: "Bob".into(),
                action: 2,
                period: 3,
                reason: "spam".into()
            }
        );
        let mut w = PktWriter::new(OP_MUTE);
        w.wstr("Bob", 17).u32(1).i32(15).at(0xb2);
        let p = w.into_payload();
        assert_eq!(p.len(), 0xb2);
        assert_eq!(
            parse_mute(&p),
            MuteReq {
                target: "Bob".into(),
                on: true,
                minutes: 15
            }
        );
        // 0x3d2 (0x2c): selector + account or name
        let mut w = PktWriter::new(OP_LOOKUP);
        w.i32(0).u32(77).at(0x2c);
        assert_eq!(parse_lookup(&w.into_payload()), LookupKey::Account(77));
        let mut w = PktWriter::new(OP_LOOKUP);
        w.i32(2).wstr("Bob", 17);
        let p = w.into_payload();
        assert_eq!(p.len(), 0x2c);
        assert_eq!(parse_lookup(&p), LookupKey::Nick("Bob".into()));
        let mut w = PktWriter::new(OP_LOOKUP);
        w.i32(1).wstr("bob", 17);
        assert_eq!(
            parse_lookup(&w.into_payload()),
            LookupKey::Login("bob".into())
        );
        let mut w = PktWriter::new(OP_LOOKUP);
        w.i32(-1).at(0x2c);
        assert_eq!(parse_lookup(&w.into_payload()), LookupKey::Invalid);
    }

    #[test]
    fn reply_layouts() {
        let (id, p) = split(&result_reply(
            OP_RESTRICT_REPLY,
            false,
            REASON_RESTRICT_UNKNOWN,
        ));
        assert_eq!(
            (id, p.len(), u32_at(&p, 6), u16_at(&p, 0xa)),
            (0x3d1, 0xc, 0, 0x5d)
        );
        let (id, p) = split(&result_reply(OP_MUTE_REPLY, true, 0));
        assert_eq!((id, p.len(), u32_at(&p, 6)), (0x3d5, 0xc, 1));
        let v = LookupView {
            account: 9,
            login: "bob".into(),
            nick: "Bob".into(),
            address: "127.0.0.1".into(),
            online: true,
            server_kind: 0,
            server_index: 0,
            channel: 0,
            room: 4,
            in_match: true,
        };
        let (id, p) = split(&lookup_reply(Some(&v), 0));
        assert_eq!(
            (id, p.len(), u32_at(&p, 6), u16_at(&p, 0xa), u32_at(&p, 0xe)),
            (0x3d3, 0x90, 1, 0, 9)
        );
        assert_eq!(
            (ws(&p, 0x12, 17), ws(&p, 0x34, 17), ws(&p, 0x56, 17)),
            ("bob".into(), "Bob".into(), "127.0.0.1".into())
        );
        assert_eq!(
            (
                u32_at(&p, 0x78),
                u32_at(&p, 0x7c),
                u32_at(&p, 0x80),
                u32_at(&p, 0x84)
            ),
            (1, 0, 0, 0)
        );
        assert_eq!((u32_at(&p, 0x88), u32_at(&p, 0x8c)), (4, 1));
        let (_, p) = split(&lookup_reply(None, REASON_LOOKUP_NOT_FOUND));
        assert_eq!((p.len(), u32_at(&p, 6), u16_at(&p, 0xa)), (0x90, 0, 0x6f));
        let (id, p) = split(&mute_until_push(0x0102_0304_0506_0708));
        assert_eq!((id, p.len()), (0x3053, 0xe));
        assert_eq!(
            u64::from_le_bytes(p[6..14].try_into().unwrap()),
            0x0102_0304_0506_0708
        );
    }

    #[test]
    fn ban_days_round_up() {
        assert_eq!(ban_days_left(3 * 86_400 + 100, 100, 0), 3);
        assert_eq!(ban_days_left(3 * 86_400 + 100, 101, 0), 3);
        assert_eq!(ban_days_left(86_400 + 100, 50_000, 0), 1);
        assert_eq!(ban_days_left(10, 20, 0), 1);
        assert_eq!(ban_days_left(0, 20, 9999), 9999);
    }
}
