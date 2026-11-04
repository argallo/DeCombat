//! The bot seats: accounts made in advance on this lobby's DB that a stock bot takes at login, and the rule that keeps
//! a seat open for people in a room with bots (docs/BOTS.md).
//!
//! A seat is held while a connection is logged in on its account and, between the auth and the lobby connection, while
//! its session key is valid. Nothing else holds one, so a bot that crashes frees its seat with its connection, and a
//! lobby restart frees them all (the state is in memory, the accounts are in the DB).
// DC-DEFAULT: LG-318, LG-319 (proposed, USER DECISION 2026-11-02), see DC_DEFAULTS.md

use crate::db;
use crate::msg::*;
use crate::room::RoomKey;
use crate::state::{Core, HResult, Sid};
use crate::wire::{self, LoginReply};
use dc_wire::loginmods::{POOL_NICK_PREFIX, REASON_NO_POOL, REASON_POOL_ADDRESS, REASON_POOL_FULL};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// One seat in use.
#[derive(Clone, Debug)]
struct Seat {
    ip: String,
    /// The session key's end, between the auth and the lobby connection.
    hold_until: Option<Instant>,
}

/// The pool of this lobby.
#[derive(Default, Debug)]
pub struct Pool {
    /// Every account made for the pool, also those a smaller `bots.pool_size` no longer offers.
    all: BTreeSet<u32>,
    /// The accounts on offer, in account order.
    offered: Vec<u32>,
    seats: BTreeMap<u32, Seat>,
}

impl Pool {
    /// True for an account of the pool. Such an account never logs in with a password.
    pub fn has(&self, account: u32) -> bool {
        self.all.contains(&account)
    }
}

/// LG-396: a deleted account leaves the pool (its seat is no longer offered; the next start makes a new one to keep
/// `bots.pool_size`).
pub fn forget(st: &mut Core, account: u32) {
    st.pool.all.remove(&account);
    st.pool.offered.retain(|&a| a != account);
    st.pool.seats.remove(&account);
}

/// True for a name of the pool's prefix, any ASCII case.
pub fn pool_named(name: &str) -> bool {
    name.get(..POOL_NICK_PREFIX.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(POOL_NICK_PREFIX))
}

/// Makes the missing accounts of `bots.pool_size` and reads the pool. Run at every start; a name somebody else holds is
/// passed over, so the pool still gets its size.
pub fn ensure(st: &mut Core) -> HResult {
    let mut all: Vec<u32> = {
        let mut q = st
            .db
            .prepare("SELECT user_no FROM accounts WHERE pool_bot = 1 ORDER BY user_no")?;
        let rows: Vec<i64> = q
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, rusqlite::Error>>()?;
        rows.into_iter().map(|n| n as u32).collect()
    };
    let size = st.rules.bot_pool_size as usize;
    let ci = st.rules.names_case_insensitive;
    let mut n = 0u32;
    let mut made = 0usize;
    while all.len() < size {
        n += 1;
        let nick = format!("{POOL_NICK_PREFIX}{n}");
        let login = nick.to_ascii_lowercase();
        if db::account_by_login(&st.db, &login)?.is_some()
            || db::character_by_nick(&st.db, &nick, ci)?.is_some()
        {
            continue;
        }
        let no = db::create_account_from(
            &st.db,
            &login,
            crate::clock::unix_now(),
            st.rules.first_account_no,
        )?;
        st.db.execute(
            "UPDATE accounts SET pool_bot = 1, self_bot = 1 WHERE user_no = ?1",
            [no as i64],
        )?;
        crate::auth::create_character(st, no, &nick, 0)?;
        all.push(no);
        made += 1;
    }
    all.sort_unstable();
    st.pool.offered = all.iter().copied().take(size).collect();
    st.pool.all = all.into_iter().collect();
    if size > 0 {
        st.note(&format!(
            "bot pool: {} seat(s), {} made now",
            st.pool.offered.len(),
            made
        ));
    }
    Ok(())
}

fn sweep(st: &mut Core) {
    let now = Instant::now();
    let held: BTreeSet<u32> = st
        .sessions
        .values()
        .filter(|s| !s.closing)
        .filter_map(|s| s.account)
        .collect();
    st.pool
        .seats
        .retain(|a, s| held.contains(a) || s.hold_until.is_some_and(|t| t > now));
}

/// The seats in use now and on offer (`GET /status`).
pub fn counts(st: &Core) -> (usize, usize) {
    let now = Instant::now();
    let used = st
        .pool
        .seats
        .iter()
        .filter(|(a, s)| {
            s.hold_until.is_some_and(|t| t > now)
                || st
                    .sessions
                    .values()
                    .any(|x| !x.closing && x.account == Some(**a))
        })
        .count();
    (used, st.pool.offered.len())
}

/// The login of a stock bot that names no account: it gets a free seat, or a refusal saying why.
pub fn seat_login(st: &mut Core, sid: Sid) -> HResult {
    sweep(st);
    let peer = st
        .sessions
        .get(&sid)
        .map(|s| s.peer.clone())
        .unwrap_or_default();
    let ip = crate::limits::peer_ip(&peer);
    let refuse = |st: &mut Core, reason: u32, why: &str| {
        st.note(&format!("bot seat refused for {ip}: {why}"));
        st.send(
            sid,
            wire::login_reply(&LoginReply {
                reason,
                ..Default::default()
            }),
        );
    };
    if st.pool.offered.is_empty() {
        refuse(st, REASON_NO_POOL, "no pool on this lobby");
        return Ok(());
    }
    let cap = st.rules.bot_pool_per_ip as usize;
    let mine = st.pool.seats.values().filter(|s| s.ip == ip).count();
    if !crate::announce::is_loopback_peer(&peer) && mine >= cap {
        refuse(st, REASON_POOL_ADDRESS, "the address holds its limit");
        return Ok(());
    }
    let mut free = None;
    for a in st.pool.offered.clone() {
        if st.pool.seats.contains_key(&a) || crate::gm::active_ban(st, a)?.is_some() {
            continue;
        }
        match db::account_by_no(&st.db, a)? {
            Some(acc) if !acc.disabled => {
                free = Some(acc);
                break;
            }
            _ => {}
        }
    }
    let Some(acc) = free else {
        refuse(st, REASON_POOL_FULL, "the pool is full");
        return Ok(());
    };
    st.pool.seats.insert(
        acc.user_no,
        Seat {
            ip: ip.clone(),
            hold_until: None,
        },
    );
    st.note(&format!("bot seat {} ({}) to {ip}", acc.user_no, acc.login));
    let login = acc.login.clone();
    crate::auth::finish_login(st, sid, &acc, &login, None)
}

/// A session key for a seat: the seat waits for the lobby connection as long as the key is valid.
pub fn on_ticket(st: &mut Core, account: u32, expires: Instant) {
    if let Some(s) = st.pool.seats.get_mut(&account) {
        s.hold_until = Some(expires);
    }
}

/// The lobby login of a seat: the connection holds it from now on, so its end frees the seat at once; no other key of
/// the account stays valid.
pub fn on_lobby_login(st: &mut Core, sid: Sid, account: u32) {
    if !st.pool.has(account) {
        return;
    }
    st.tickets.retain(|_, t| t.account != account);
    let peer = st
        .sessions
        .get(&sid)
        .map(|s| s.peer.clone())
        .unwrap_or_default();
    st.pool.seats.insert(
        account,
        Seat {
            ip: crate::limits::peer_ip(&peer),
            hold_until: None,
        },
    );
}

/// What a seat may ask the lobby: the way into a room and a match, nothing else. No shop, trade, clan, mail, friends,
/// chat, room making or team change.
fn seat_may_send(id: u16) -> bool {
    matches!(
        id,
        FILE_CHECK
            | LOGIN_REQUEST
            | SERVER_PICK
            | dc_wire::entrymods::CLAIM
            | LOBBY_LOGIN
            | ITEM_SYNC
            | CHAR_SHEET_FETCH
            | EQUIPMENT
            | INVENTORY
            | BALANCE
            | ROOM_SETTINGS_FETCH
            | CHANNEL_LIST
            | CHANNEL_ENTER
            | CHAN_EXIT
            | CHANNEL_USERS
            | ROOM_LIST
            | LOCATION
            | LOGOUT
            | SERVER_LIST
            | ROOM_ENTER
            | ROOM_LEAVE
            | ROOM_INFO
            | ROOM_USERS
            | ROOM_USER_FLAG
            | MATCH_EXIT
    )
}

/// True when `sid` holds a seat and `id` is not for seats: the message is dropped, unanswered.
pub fn message_refused(st: &Core, sid: Sid, id: u16) -> bool {
    let refused = st
        .sessions
        .get(&sid)
        .and_then(|s| s.account)
        .is_some_and(|a| st.pool.has(a))
        && !seat_may_send(id);
    if refused {
        st.note(&format!(
            "connection {sid}: message 0x{id:x} refused for a bot seat"
        ));
    }
    refused
}

/// People online on this lobby: bots do not count.
pub fn people_online(st: &Core) -> u32 {
    st.sessions
        .values()
        .filter(|s| s.in_lobby && !s.closing)
        .filter_map(|s| s.account)
        .filter(|a| crate::lifecycle::is_human(st, *a))
        .count() as u32
}

/// `bots.keep_seat_online_players` is met: a room with bots keeps a seat open.
fn keeping(st: &Core) -> bool {
    let at = st.rules.bot_keep_seat_online;
    at > 0 && people_online(st) >= at
}

fn rule_covers(st: &Core, key: RoomKey) -> bool {
    st.rules.bot_keep_seat_online > 0 && !crate::clan::is_clan_channel(key.channel)
}

/// The bot that leaves first: a bot that is not the host, on the larger team, the newest. A GM is never asked to leave.
/// The host only when no other bot is there; the normal hand-over then gives the room to a person.
fn yielder(st: &Core, key: RoomKey) -> Option<(u32, Sid)> {
    let r = st.rooms.get(&key)?;
    r.members
        .iter()
        .filter(|m| {
            !crate::lifecycle::is_human(st, m.account) && !crate::gm::kick_immune(st, m.account)
        })
        .max_by_key(|m| (m.account != r.host, r.count_team(m.team), m.seq))
        .map(|m| (m.account, m.sid))
}

/// True when a person may still come into this full room, because a bot makes way.
pub fn seat_can_open(st: &Core, key: RoomKey) -> bool {
    rule_covers(st, key) && yielder(st, key).is_some()
}

/// True when a bot may not come in: it would take the last free seat of a room while the rule is met.
pub fn last_seat_kept(st: &Core, key: RoomKey) -> bool {
    rule_covers(st, key)
        && keeping(st)
        && st
            .rooms
            .get(&key)
            .is_some_and(|r| r.members.len() as u32 + 1 >= r.max_players)
}

/// One bot leaves the room with the normal pushes; the other members see a plain leave and the bot alone its removal,
/// so it knows to look elsewhere. False when no bot can leave.
pub fn yield_one(st: &mut Core, key: RoomKey) -> Result<bool, crate::state::Fault> {
    let Some((account, sid)) = yielder(st, key) else {
        return Ok(false);
    };
    st.send(
        sid,
        wire::room_user_remove_push(account, wire::REMOVE_KICKED),
    );
    let before = st.rooms.get(&key).map_or(0, |r| r.members.len());
    crate::room::remove_member(st, sid, wire::REMOVE_LEFT, true)?;
    if st.rooms.get(&key).map_or(0, |r| r.members.len()) >= before {
        // a stale member record: nothing left, so the caller's loop ends
        return Ok(false);
    }
    st.note(&format!(
        "room {}-{}: bot account {account} left to keep a seat for people",
        key.channel, key.no
    ));
    Ok(true)
}

/// After a person came in: a room now full gives up a bot while the rule is met.
pub fn after_person_joined(st: &mut Core, key: RoomKey) -> HResult {
    let full = st
        .rooms
        .get(&key)
        .is_some_and(|r| r.members.len() as u32 >= r.max_players);
    if full && rule_covers(st, key) && keeping(st) {
        yield_one(st, key)?;
    }
    Ok(())
}
