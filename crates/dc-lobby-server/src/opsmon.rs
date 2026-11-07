//! Operator monitoring: what happened to whom, stored so that a night's triage is a query instead of a grep.
//!
//! - **Match records** (MON-2): one `ops_matches` row per game server the lobby starts (room, map, mode, host and its
//!   version, players, bots, outcome), one `ops_match_players` row per player link from the game server's
//!   `@linksum` line (MON-1 of dc-dserver: round trip mean / p95 / max, input rate, resends, flow drops, largest
//!   datagram, longest gap, how the link ended) and one `ops_net` row per `@net` 10 s window. Remote hosts send the
//!   same lines through their agent (`event <gid> <line>`), so both links end in [`on_pipe`].
//! - **Events** (MON-3): `ops_events`, per account, the notable things the lobby saw (a refused ready, a seat ticket
//!   refusal, a start that left members behind, a dropped game link, a report, a crash diagnostics upload, ...).
//! - **Queries** for the dashboard (opsweb.rs), the alerts and the `ops` command line.
//!
//! Rows older than `monitor.retention_days` are deleted (at most once an hour). A monitoring write that fails is
//! logged and never fails the handler it rides in.
// Provenance: ORIGINAL (DeCombat operator tooling; no 2009 counterpart)
// DC-DEFAULT: MON-2 (proposed) match records; MON-3 (proposed) account events

use crate::game::Event;
use crate::state::Core;
use dc_mod_lua::JsonValue as J;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS ops_matches (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    game         INTEGER NOT NULL DEFAULT 0,
    started      INTEGER NOT NULL DEFAULT 0,
    ended        INTEGER NOT NULL DEFAULT 0,
    channel      INTEGER NOT NULL DEFAULT 0,
    room         INTEGER NOT NULL DEFAULT 0,
    map          INTEGER NOT NULL DEFAULT 0,
    map_world     TEXT NOT NULL DEFAULT '',
    map_name      TEXT NOT NULL DEFAULT '',
    mode         INTEGER NOT NULL DEFAULT 0,
    mode_key     TEXT NOT NULL DEFAULT '',
    host         TEXT NOT NULL DEFAULT 'local',
    host_version TEXT NOT NULL DEFAULT '',
    port         INTEGER NOT NULL DEFAULT 0,
    players      INTEGER NOT NULL DEFAULT 0,
    bots         INTEGER NOT NULL DEFAULT 0,
    outcome      TEXT NOT NULL DEFAULT 'running',
    winner       INTEGER NOT NULL DEFAULT 0,
    alpha        INTEGER NOT NULL DEFAULT 0,
    bravo        INTEGER NOT NULL DEFAULT 0,
    silent_drops INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ops_matches_started ON ops_matches (started);
CREATE TABLE IF NOT EXISTS ops_match_players (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    match_id     INTEGER NOT NULL,
    user_no      INTEGER NOT NULL DEFAULT 0,
    nick         TEXT NOT NULL DEFAULT '',
    bot          INTEGER NOT NULL DEFAULT 0,
    client       TEXT NOT NULL DEFAULT '',
    joined       INTEGER NOT NULL DEFAULT 0,
    left_at      INTEGER NOT NULL DEFAULT 0,
    how          TEXT NOT NULL DEFAULT '',
    code         INTEGER NOT NULL DEFAULT 0,
    has_link     INTEGER NOT NULL DEFAULT 0,
    secs         REAL NOT NULL DEFAULT 0,
    rtt_mean     REAL NOT NULL DEFAULT 0,
    rtt_p95      REAL NOT NULL DEFAULT 0,
    rtt_max      REAL NOT NULL DEFAULT 0,
    rtt_samples  INTEGER NOT NULL DEFAULT 0,
    jitter       REAL NOT NULL DEFAULT 0,
    input_rate   REAL NOT NULL DEFAULT 0,
    updates      INTEGER NOT NULL DEFAULT 0,
    gap_max      INTEGER NOT NULL DEFAULT 0,
    resends      INTEGER NOT NULL DEFAULT 0,
    reliable     INTEGER NOT NULL DEFAULT 0,
    flow_drops   INTEGER NOT NULL DEFAULT 0,
    dg_in        INTEGER NOT NULL DEFAULT 0,
    dg_out       INTEGER NOT NULL DEFAULT 0,
    max_datagram INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ops_match_players_match ON ops_match_players (match_id);
CREATE INDEX IF NOT EXISTS ops_match_players_user ON ops_match_players (user_no, joined);
CREATE TABLE IF NOT EXISTS ops_net (
    match_id  INTEGER NOT NULL,
    at        INTEGER NOT NULL DEFAULT 0,
    client    TEXT NOT NULL DEFAULT '',
    user_no   INTEGER NOT NULL DEFAULT 0,
    window_ms INTEGER NOT NULL DEFAULT 0,
    rtt       REAL NOT NULL DEFAULT 0,
    rtt_min   REAL NOT NULL DEFAULT 0,
    rtt_max   REAL NOT NULL DEFAULT 0,
    rtt_n     INTEGER NOT NULL DEFAULT 0,
    jitter    REAL NOT NULL DEFAULT 0,
    input     REAL NOT NULL DEFAULT 0,
    gap       INTEGER NOT NULL DEFAULT 0,
    resends   INTEGER NOT NULL DEFAULT 0,
    drops     INTEGER NOT NULL DEFAULT 0,
    dg_in     INTEGER NOT NULL DEFAULT 0,
    dg_out    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ops_net_match ON ops_net (match_id, at);
CREATE INDEX IF NOT EXISTS ops_net_at ON ops_net (at);
CREATE TABLE IF NOT EXISTS ops_events (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    at       INTEGER NOT NULL DEFAULT 0,
    user_no  INTEGER NOT NULL DEFAULT 0,
    nick     TEXT NOT NULL DEFAULT '',
    kind     TEXT NOT NULL DEFAULT '',
    problem  INTEGER NOT NULL DEFAULT 0,
    match_id INTEGER NOT NULL DEFAULT 0,
    detail   TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS ops_events_user ON ops_events (user_no, at);
CREATE INDEX IF NOT EXISTS ops_events_at ON ops_events (at, problem);
"#;

/// How often old rows are deleted.
const PURGE_EVERY: Duration = Duration::from_secs(3600);
/// Longest event detail kept.
const MAX_DETAIL: usize = 500;

/// The event kinds the lobby records (MON-3).
pub mod kind {
    pub const LOGIN: &str = "login";
    pub const DISCONNECT: &str = "lobby_disconnect";
    pub const READY_REFUSED: &str = "ready_refused";
    /// Entry into a room from the list, or a late join into a running match, refused (room full, join in progress off, kicked, wrong password).
    pub const JOIN_REFUSED: &str = "join_refused";
    pub const START_REFUSED: &str = "start_refused";
    pub const START_FAILED: &str = "start_failed";
    pub const LEFT_AT_START: &str = "left_at_start";
    pub const START_BLOCKED: &str = "start_blocked";
    pub const SEAT_REFUSED: &str = "seat_refused";
    pub const MATCH_JOIN: &str = "match_join";
    pub const MATCH_LEFT: &str = "match_left";
    pub const SILENT_DROP: &str = "silent_drop";
    pub const MATCH_DROP: &str = "match_drop";
    pub const GAME_CRASH: &str = "game_crash";
    pub const REPORT: &str = "report_filed";
    #[allow(dead_code)]
    pub const DIAGNOSTICS: &str = "diagnostics";
    pub const LOBBY_FAULT: &str = "lobby_fault";
    /// LG-397: an operator's change to an account from the dashboard's Players tab (playeradmin.rs).
    pub const ADMIN: &str = "admin";
}

/// True for the kinds the dashboard's "problems" list and the error-spike alert count.
pub fn is_problem(k: &str, detail: &str) -> bool {
    match k {
        // a refused entry is a rule at work, not a fault: it shows in the account events only
        kind::LOGIN | kind::MATCH_JOIN | kind::MATCH_LEFT | kind::ADMIN | kind::JOIN_REFUSED => {
            false
        }
        // a disconnect is a problem only when it cut a match short
        kind::DISCONNECT => detail.contains("in a match"),
        _ => true,
    }
}

/// Per-game bookkeeping of the monitor (in memory; the rows are in the DB).
#[derive(Debug, Default)]
struct GameMon {
    match_id: i64,
    /// Account -> unix time of its first `@join`.
    joined: BTreeMap<u32, i64>,
    /// Accounts whose `@linksum` arrived (a rejoin gives a second row).
    summarized: BTreeSet<u32>,
    silent: u32,
    closed: bool,
}

/// The monitor's state in [`Core`].
#[derive(Debug, Default)]
pub struct Monitor {
    games: BTreeMap<u64, GameMon>,
    last_purge: Option<Instant>,
}

/// The build this lobby was made from (the git short sha of build.rs, "unknown" without one).
pub fn build_sha() -> &'static str {
    let v = env!("DC_LOBBY_GIT_SHA");
    if v.is_empty() || v.chars().any(|c| c.is_whitespace() || c.is_control()) {
        "unknown"
    } else {
        v
    }
}

/// Two build ids name the same build: equal, or one a prefix of the other (a short and a full sha); "unknown" or
/// empty never mismatches.
pub fn same_build(a: &str, b: &str) -> bool {
    let known = |s: &str| !s.is_empty() && s != "unknown";
    if !known(a) || !known(b) {
        return true;
    }
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a.starts_with(&b) || b.starts_with(&a)
}

fn cut(s: &str) -> String {
    let s = crate::logs::log_text(s);
    if s.chars().count() <= MAX_DETAIL {
        s
    } else {
        s.chars().take(MAX_DETAIL).collect::<String>() + "..."
    }
}

fn nick_of(st: &Core, user: u32) -> String {
    if user == 0 {
        return String::new();
    }
    st.character(user).map(|c| c.nick).unwrap_or_default()
}

/// The match an account is in now (its room's running game), 0 when none.
fn current_match(st: &Core, user: u32) -> i64 {
    st.games
        .values()
        .filter(|g| !g.over)
        .find(|g| {
            st.rooms
                .get(&g.room)
                .is_some_and(|r| r.game == Some(g.id) && r.member(user).is_some())
        })
        .and_then(|g| st.monitor.games.get(&g.id))
        .map_or(0, |m| m.match_id)
}

fn put_event(c: &Connection, at: i64, user: u32, nick: &str, k: &str, match_id: i64, detail: &str) {
    let _ = c.execute(
        "INSERT INTO ops_events (at, user_no, nick, kind, problem, match_id, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![at, user as i64, nick, k, is_problem(k, detail) as i64, match_id, cut(detail)],
    );
}

/// Records one event of `user` (0 = no account). Never fails: a write error is dropped. The logins and lobby
/// disconnects of bot accounts are not recorded (a pool of bots would bury the players).
pub fn event(st: &Core, user: u32, k: &str, detail: &str) {
    if (k == kind::LOGIN || k == kind::DISCONNECT)
        && user != 0
        && !crate::lifecycle::is_human(st, user)
    {
        return;
    }
    let m = if user == 0 {
        0
    } else {
        current_match(st, user)
    };
    put_event(
        &st.db,
        crate::clock::unix_now(),
        user,
        &nick_of(st, user),
        k,
        m,
        detail,
    );
}

/// An event of the admin web sign-in (adminweb.rs, LG-386): sign-ins, enrolments, key changes and refusals, named by
/// key number, label and a short credential id, never a token or a cookie. A refusal is a problem.
pub fn admin_web_event(c: &Connection, problem: bool, detail: &str) {
    let _ = c.execute(
        "INSERT INTO ops_events (at, user_no, nick, kind, problem, match_id, detail) VALUES (?1, 0, '', 'admin_web', ?2, 0, ?3)",
        params![crate::clock::unix_now(), problem as i64, cut(detail)],
    );
}

/// [`event`] tied to a match.
fn match_event(st: &Core, user: u32, k: &str, match_id: i64, detail: &str) {
    put_event(
        &st.db,
        crate::clock::unix_now(),
        user,
        &nick_of(st, user),
        k,
        match_id,
        detail,
    );
}

/// Hook for client diagnostics uploads (crash or disconnect reports): `id` is the stored report's id, `what` a short
/// description. Shows in the account's timeline and in the problem list.
pub fn diagnostics_uploaded(st: &Core, user: u32, id: &str, what: &str) {
    event(
        st,
        user,
        kind::DIAGNOSTICS,
        &format!("diagnostics {id}: {what}"),
    );
}

/// A handler's rolled-back error (net.rs `report`): counted for the error-spike alert.
pub fn fault(st: &Core, what: &str) {
    put_event(
        &st.db,
        crate::clock::unix_now(),
        0,
        "",
        kind::LOBBY_FAULT,
        0,
        what,
    );
}

/// Deletes rows older than the retention (at most once per [`PURGE_EVERY`]).
pub fn maybe_purge(st: &mut Core) {
    if st
        .monitor
        .last_purge
        .is_some_and(|t| t.elapsed() < PURGE_EVERY)
    {
        return;
    }
    st.monitor.last_purge = Some(Instant::now());
    let days = st.rules.ops.monitor_retention_days;
    if let Err(e) = purge(&st.db, crate::clock::unix_now(), days) {
        st.note(&format!("monitor: purge failed: {e}"));
    }
}

/// Deletes the rows older than `days` days before `now` (0 days = keep everything).
pub fn purge(c: &Connection, now: i64, days: u32) -> rusqlite::Result<usize> {
    if days == 0 {
        return Ok(0);
    }
    let cutoff = now - days as i64 * 86_400;
    let mut n = c.execute("DELETE FROM ops_events WHERE at < ?1", [cutoff])?;
    n += c.execute("DELETE FROM ops_net WHERE at < ?1", [cutoff])?;
    n += c.execute(
        "DELETE FROM ops_match_players WHERE match_id IN (SELECT id FROM ops_matches WHERE started < ?1)",
        [cutoff],
    )?;
    n += c.execute("DELETE FROM ops_matches WHERE started < ?1", [cutoff])?;
    Ok(n)
}

/// OPS-14: `GET /ops/guns`: the weapons a loadout can name, `{id, name, slot}` (slot 0 primary, 1 secondary, 2 melee,
/// 3 throwing): the ROM item table's weapons, then the camo variants of the loaded mod sets (lobby-wide and each curated
/// entry's) in their base gun's slot, with the variant's own name. Empty without a ROM item table.
// DC-DEFAULT: OPS-14 (proposed)
pub fn guns_json(st: &Core) -> J {
    let mut guns: Vec<(i64, String, i64)> = st.rom.weapon_list();
    let contents = st
        .mods
        .iter()
        .map(|m| &m.client_content)
        .chain(st.entry_mods.values().map(|e| &e.client.client_content));
    let mut variants: Vec<(i64, String, i64)> = Vec::new();
    for v in contents.flat_map(|c| c.variants()) {
        let id = i64::from(v.row);
        let Some(slot) = st
            .rom
            .slot_of_item(i64::from(v.base))
            .filter(|s| (0..=3).contains(s))
        else {
            continue;
        };
        if guns.iter().chain(&variants).any(|g| g.0 == id) {
            continue;
        }
        let name = v
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map_or_else(|| format!("Variant {id}"), str::to_string);
        variants.push((id, name, slot));
    }
    variants.sort_by(|a, b| (a.2, &a.1, a.0).cmp(&(b.2, &b.1, b.0)));
    guns.extend(variants);
    J::Array(
        guns.into_iter()
            .map(|(id, name, slot)| {
                o(vec![
                    ("id", n(id as f64)),
                    ("name", s(name)),
                    ("slot", n(slot as f64)),
                ])
            })
            .collect(),
    )
}

/// The world and the name of a game's map as the lobby started it: the room's channel decides the map table (a
/// curated entry has its own community maps, MAP-82), a clan room takes the clan mode's world.
// DC-DEFAULT: OPS-13 (proposed) every page names a match's map from the table the lobby started it with
fn game_map(st: &Core, g: &crate::game::Game) -> (String, String) {
    room_map(st, g.room, g.map, g.mode)
}

/// [`game_map`] for a room's `(map, mode)`.
pub(crate) fn room_map(
    st: &Core,
    room: crate::room::RoomKey,
    map: u32,
    mode: u32,
) -> (String, String) {
    let table = st.maps_of_channel(room.channel);
    let world = crate::clan::world(st, room)
        .unwrap_or_else(|| table.world_for(map, mode, &st.opts.dserver_world));
    let name = table.name_of(map, &world);
    (world, name)
}

/// The `ops_matches` row of a game, made at the first need (the start, or the first pipe line).
fn ensure_match(st: &mut Core, game: u64) -> Option<i64> {
    if let Some(m) = st.monitor.games.get(&game) {
        return Some(m.match_id);
    }
    let g = st.games.get(&game)?;
    let (host, version) = match g.link.remote() {
        Some(r) => (
            r.agent.clone(),
            st.hosts
                .online
                .get(&r.agent)
                .map(|a| a.version.clone())
                .unwrap_or_default(),
        ),
        None => ("local".to_string(), build_sha().to_string()),
    };
    let (map_world, map_name) = game_map(st, g);
    let mode_key = st.rom.mode_key(g.mode);
    let r = st.db.execute(
        "INSERT INTO ops_matches (game, started, channel, room, map, map_world, map_name, mode, mode_key, host, host_version, port) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            game as i64,
            crate::clock::unix_now(),
            g.room.channel as i64,
            g.room.no as i64,
            g.map as i64,
            map_world,
            map_name,
            g.mode as i64,
            mode_key,
            host,
            version,
            g.port as i64
        ],
    );
    match r {
        Ok(_) => {
            let id = st.db.last_insert_rowid();
            st.monitor.games.insert(
                game,
                GameMon {
                    match_id: id,
                    ..Default::default()
                },
            );
            Some(id)
        }
        Err(e) => {
            st.note(&format!("monitor: match row of game {game}: {e}"));
            None
        }
    }
}

/// A game server was started for a room.
pub fn match_started(st: &mut Core, game: u64) {
    ensure_match(st, game);
}

/// One `@linksum` line, parsed (MON-1 of dc-dserver).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkSum {
    pub client: String,
    pub uid: u32,
    pub how: String,
    pub code: i64,
    pub secs: f64,
    pub rtt_mean: f64,
    pub rtt_p95: f64,
    pub rtt_max: f64,
    pub rtt_samples: i64,
    pub jitter: f64,
    pub input_rate: f64,
    pub updates: i64,
    pub gap_max: i64,
    pub resends: i64,
    pub reliable: i64,
    pub flow_drops: i64,
    pub dg_in: i64,
    pub dg_out: i64,
    pub max_datagram: i64,
}

/// `@linksum k=v ...` -> [`LinkSum`]; `None` for another line or one without `uid=`.
pub fn parse_linksum(line: &str) -> Option<LinkSum> {
    let mut t = line.split_whitespace();
    if t.next()? != "@linksum" {
        return None;
    }
    let m: BTreeMap<&str, &str> = t.filter_map(|x| x.split_once('=')).collect();
    let f = |k: &str| {
        m.get(k)
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(0.0)
    };
    let i = |k: &str| {
        m.get(k)
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0)
            .max(0)
    };
    let how: String = m
        .get("how")
        .copied()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect();
    Some(LinkSum {
        client: m
            .get("client")
            .map(|s| s.chars().take(8).collect())
            .unwrap_or_default(),
        uid: m.get("uid")?.parse().ok()?,
        how,
        code: i("code"),
        secs: f("secs"),
        rtt_mean: f("rtt"),
        rtt_p95: f("rtt_p95"),
        rtt_max: f("rtt_max"),
        rtt_samples: i("rtt_n"),
        jitter: f("jitter"),
        input_rate: f("input"),
        updates: i("updates"),
        gap_max: i("gap"),
        resends: i("resends"),
        reliable: i("rel"),
        flow_drops: i("drops"),
        dg_in: i("in"),
        dg_out: i("out"),
        max_datagram: i("maxdg"),
    })
}

/// The event kind of a link's end: normal (bye, match end), silent drop, or another drop.
fn leave_kind(how: &str) -> &'static str {
    match how {
        "bye" | "end" => kind::MATCH_LEFT,
        "silent" => kind::SILENT_DROP,
        _ => kind::MATCH_DROP,
    }
}

fn store_linksum(st: &mut Core, game: u64, match_id: i64, s: &LinkSum) {
    let now = crate::clock::unix_now();
    let joined = st
        .monitor
        .games
        .get(&game)
        .and_then(|m| m.joined.get(&s.uid).copied())
        .unwrap_or(now - s.secs as i64);
    let bot = !crate::lifecycle::is_human(st, s.uid);
    let nick = nick_of(st, s.uid);
    let r = st.db.execute(
        "INSERT INTO ops_match_players (match_id, user_no, nick, bot, client, joined, left_at, how, code, has_link, secs, \
         rtt_mean, rtt_p95, rtt_max, rtt_samples, jitter, input_rate, updates, gap_max, resends, reliable, flow_drops, \
         dg_in, dg_out, max_datagram) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
         ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
        params![
            match_id,
            s.uid as i64,
            nick,
            bot as i64,
            s.client,
            joined,
            now,
            s.how,
            s.code,
            s.secs,
            s.rtt_mean,
            s.rtt_p95,
            s.rtt_max,
            s.rtt_samples,
            s.jitter,
            s.input_rate,
            s.updates,
            s.gap_max,
            s.resends,
            s.reliable,
            s.flow_drops,
            s.dg_in,
            s.dg_out,
            s.max_datagram
        ],
    );
    if let Err(e) = r {
        st.note(&format!("monitor: player row of game {game}: {e}"));
    }
    let silent = s.how == "silent";
    if let Some(m) = st.monitor.games.get_mut(&game) {
        m.summarized.insert(s.uid);
        if silent {
            m.silent += 1;
        }
    }
    if silent {
        let _ = st.db.execute(
            "UPDATE ops_matches SET silent_drops = silent_drops + 1 WHERE id = ?1",
            [match_id],
        );
    }
    if !bot {
        let detail = format!(
            "{} after {:.0} s (code {}): rtt {:.0} ms (p95 {:.0}, max {:.0}), input {:.1}/s, longest gap {} ms, resends {}",
            s.how, s.secs, s.code, s.rtt_mean, s.rtt_p95, s.rtt_max, s.input_rate, s.gap_max, s.resends
        );
        match_event(st, s.uid, leave_kind(&s.how), match_id, &detail);
    }
}

/// One pipe line of a game server (local stdout, or a host agent's `event` line), before the lobby acts on it.
pub fn on_pipe(st: &mut Core, game: u64, line: &str, ev: &Event) {
    let interesting = matches!(
        ev,
        Event::Ready { .. }
            | Event::Join { .. }
            | Event::Net(_)
            | Event::SeatRefused(_)
            | Event::Result(_)
            | Event::Crash(_)
    ) || matches!(ev, Event::Other(t) if t.starts_with("@linksum"));
    if !interesting {
        return;
    }
    let Some(match_id) = ensure_match(st, game) else {
        return;
    };
    let now = crate::clock::unix_now();
    match ev {
        Event::Ready { port, .. } => {
            if let Ok(p) = port.parse::<i64>() {
                let _ = st.db.execute(
                    "UPDATE ops_matches SET port = ?1 WHERE id = ?2 AND port = 0",
                    params![p, match_id],
                );
            }
        }
        Event::Join {
            client,
            uid: Some(uid),
        } => {
            let first = st
                .monitor
                .games
                .get_mut(&game)
                .map(|m| {
                    let new = !m.joined.contains_key(uid);
                    m.joined.entry(*uid).or_insert(now);
                    m.summarized.remove(uid);
                    new
                })
                .unwrap_or(false);
            if crate::lifecycle::is_human(st, *uid) {
                let room = st.games.get(&game).map(|g| g.room);
                let detail = format!(
                    "{} room {} (client {client})",
                    if first { "joined" } else { "joined again" },
                    room.map(|r| format!("{}-{}", r.channel, r.no))
                        .unwrap_or_default()
                );
                match_event(st, *uid, kind::MATCH_JOIN, match_id, &detail);
            }
        }
        Event::Net(n) => {
            let uid = if n.uid != 0 {
                n.uid
            } else {
                st.games
                    .get(&game)
                    .and_then(|g| g.clients.get(&n.client).copied())
                    .unwrap_or(0)
            };
            let _ = st.db.execute(
                "INSERT INTO ops_net (match_id, at, client, user_no, window_ms, rtt, rtt_min, rtt_max, rtt_n, jitter, input, \
                 gap, resends, drops, dg_in, dg_out) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    match_id,
                    now,
                    n.client,
                    uid as i64,
                    n.window_ms as i64,
                    n.rtt_mean,
                    n.rtt_min,
                    n.rtt_max,
                    n.rtt_samples as i64,
                    n.jitter_ms,
                    n.input_rate,
                    n.longest_gap_ms as i64,
                    n.resends as i64,
                    n.flow_drops as i64,
                    n.datagrams_in as i64,
                    n.datagrams_out as i64
                ],
            );
        }
        Event::SeatRefused(uid) => {
            match_event(
                st,
                *uid,
                kind::SEAT_REFUSED,
                match_id,
                "seat refused: a client without a seat ticket (too old) asked for this account's seat",
            );
        }
        Event::Result(r) => {
            let _ = st.db.execute(
                "UPDATE ops_matches SET winner = ?1, alpha = ?2, bravo = ?3, mode_key = CASE WHEN mode_key = '' THEN ?4 ELSE mode_key END \
                 WHERE id = ?5",
                params![r.winner, r.alpha, r.bravo, r.mode, match_id],
            );
        }
        Event::Crash(text) => {
            match_event(
                st,
                0,
                kind::GAME_CRASH,
                match_id,
                &format!("game {game}: {text}"),
            );
        }
        Event::Other(_) => {
            if let Some(s) = parse_linksum(line) {
                store_linksum(st, game, match_id, &s);
            }
        }
        _ => {}
    }
}

/// The game is over (`@end`, a crash, the process gone): its row gets its outcome and player counts; players whose
/// summary never came get a row without link numbers. Runs once per game.
pub fn match_closed(st: &mut Core, game: u64, crashed: bool) {
    let Some(match_id) = ensure_match(st, game) else {
        return;
    };
    let Some(m) = st.monitor.games.get_mut(&game) else {
        return;
    };
    if m.closed {
        return;
    }
    m.closed = true;
    let joined: Vec<(u32, i64)> = m.joined.iter().map(|(&u, &t)| (u, t)).collect();
    let missing: Vec<(u32, i64)> = joined
        .iter()
        .copied()
        .filter(|(u, _)| !m.summarized.contains(u))
        .collect();
    let (ready, result) = st
        .games
        .get(&game)
        .map_or((false, false), |g| (g.ready, g.result_done));
    let outcome = if result {
        "result"
    } else if !ready {
        "start-failed"
    } else if crashed {
        "crash"
    } else {
        "no-result"
    };
    let now = crate::clock::unix_now();
    let how = if crashed { "crash" } else { "unknown" };
    for (uid, t) in missing {
        let bot = !crate::lifecycle::is_human(st, uid);
        let nick = nick_of(st, uid);
        let _ = st.db.execute(
            "INSERT INTO ops_match_players (match_id, user_no, nick, bot, joined, left_at, how, has_link) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
            params![match_id, uid as i64, nick, bot as i64, t, now, how],
        );
    }
    let humans = joined
        .iter()
        .filter(|(u, _)| crate::lifecycle::is_human(st, *u))
        .count();
    let bots = joined.len() - humans;
    let _ = st.db.execute(
        "UPDATE ops_matches SET ended = ?1, outcome = ?2, players = ?3, bots = ?4 WHERE id = ?5",
        params![now, outcome, humans as i64, bots as i64, match_id],
    );
    if outcome == "crash" || outcome == "start-failed" {
        let host = st
            .games
            .get(&game)
            .and_then(|g| g.link.remote().map(|r| r.agent.clone()))
            .unwrap_or_else(|| "local".into());
        match_event(
            st,
            0,
            kind::GAME_CRASH,
            match_id,
            &format!("game {game} on {host}: {outcome}"),
        );
    }
    let live: BTreeSet<u64> = st.games.keys().copied().collect();
    st.monitor
        .games
        .retain(|id, g| !g.closed || live.contains(id));
    maybe_purge(st);
}

// ------------------------------------------------------------------------------------------------ queries

fn o(pairs: Vec<(&str, J)>) -> J {
    J::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn s(v: impl Into<String>) -> J {
    J::String(v.into())
}

fn n(v: impl Into<f64>) -> J {
    J::Number(v.into())
}

fn r1(v: f64) -> J {
    J::Number((v * 10.0).round() / 10.0)
}

/// Link health of one 10 s window or one summary: "good", "warn" or "bad". `resends` and `flow_drops` per 10 s;
/// `silent`: nothing arrived (a window without datagrams in, a stale report, a link that ended silent). The input
/// rate and its longest gap are not used: a dead player sends no updates until the respawn (7 s gaps with bots).
// DC-DEFAULT: MON-4 (proposed) the dashboard's colours
pub fn health(
    rtt_mean: f64,
    rtt_max: f64,
    resends: f64,
    flow_drops: f64,
    silent: bool,
) -> &'static str {
    if silent || rtt_mean > 200.0 {
        "bad"
    } else if rtt_mean > 120.0 || rtt_max > 400.0 || resends > 10.0 || flow_drops > 0.0 {
        "warn"
    } else {
        "good"
    }
}

/// Seconds after which a player's newest `@net` window is stale (two and a half windows).
const STALE_SECS: f64 = 25.0;

/// `GET /ops/live`: the lobby, its hosts, rooms and the running matches with each player's newest link window.
pub fn live_json(st: &Core) -> J {
    let lobby = o(vec![
        ("build", s(build_sha())),
        ("version", s(env!("CARGO_PKG_VERSION"))),
        ("uptime_secs", n(st.started.elapsed().as_secs() as f64)),
        ("stopping", J::Bool(st.stopping)),
        ("connections", n(st.sessions.len() as f64)),
        (
            "online",
            n(st.sessions
                .values()
                .filter(|x| x.in_lobby && x.account.is_some())
                .count() as f64),
        ),
        ("lobby_logins", n(st.lobby_logins as f64)),
        ("public", J::Bool(st.opts.public)),
    ]);
    // the lobby's own machine first: the matches it starts itself (no host agent) run here
    let local_games = st
        .games
        .values()
        .filter(|g| !g.over && g.link.remote().is_none())
        .count();
    let this_box = o(vec![
        ("name", s("this server (lobby)".to_string())),
        ("online", J::Bool(true)),
        ("version", s(build_sha())),
        ("version_matches", J::Bool(true)),
        // the address the lobby gives the clients (a loopback one says nothing useful here)
        (
            "address",
            match st.opts.lobbyip.parse::<std::net::IpAddr>() {
                Ok(ip) if !ip.is_loopback() => s(st.opts.lobbyip.clone()),
                _ => J::Null,
            },
        ),
        ("online_secs", n(st.started.elapsed().as_secs() as f64)),
        ("games", n(local_games as f64)),
        ("local", J::Bool(true)),
    ]);
    let hosts = J::Array(
        std::iter::once(this_box)
            .chain(st.opts.host_agents.iter().map(|c| {
                let a = st.hosts.online.get(&c.name);
                let games = st
                    .games
                    .values()
                    .filter(|g| !g.over && g.link.remote().is_some_and(|r| r.agent == c.name))
                    .count();
                o(vec![
                    ("name", s(c.name.clone())),
                    ("online", J::Bool(a.is_some())),
                    ("version", a.map_or(J::Null, |a| s(a.version.clone()))),
                    (
                        "version_matches",
                        J::Bool(a.is_none_or(|a| same_build(&a.version, build_sha()))),
                    ),
                    ("address", a.map_or(J::Null, |a| s(a.address.clone()))),
                    (
                        "online_secs",
                        a.map_or(J::Null, |a| n(a.since.elapsed().as_secs() as f64)),
                    ),
                    ("games", n(games as f64)),
                    ("local", J::Bool(false)),
                ])
            }))
            .collect(),
    );
    let rooms = J::Array(
        st.rooms
            .iter()
            .map(|(k, r)| {
                let humans = r
                    .members
                    .iter()
                    .filter(|m| crate::lifecycle::is_human(st, m.account))
                    .count();
                o(vec![
                    ("id", s(format!("{}-{}", k.channel, k.no))),
                    ("title", s(r.title.clone())),
                    ("state", s(format!("{:?}", r.state).to_lowercase())),
                    ("mode", s(st.rom.mode_key(r.mode))),
                    ("map", n(r.map)),
                    ("players", n(humans as f64)),
                    ("bots", n((r.members.len() - humans) as f64)),
                    ("max_players", n(r.max_players)),
                    (
                        "members",
                        J::Array(
                            r.members
                                .iter()
                                .map(|m| s(nick_of(st, m.account)))
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect(),
    );
    let mut matches = Vec::new();
    for g in st.games.values().filter(|g| !g.over) {
        let mut players = Vec::new();
        for (client, &uid) in &g.clients {
            if g.left.contains(&uid) {
                continue;
            }
            let mut p = vec![
                ("account", n(uid)),
                ("nick", s(nick_of(st, uid))),
                ("client", s(client.clone())),
                ("bot", J::Bool(!crate::lifecycle::is_human(st, uid))),
            ];
            match g.net.get(client) {
                Some((at, w)) => {
                    let age = at.elapsed().as_secs_f64();
                    p.push(("report_age_secs", r1(age)));
                    p.push(("rtt", r1(w.rtt_mean)));
                    p.push(("rtt_min", r1(w.rtt_min)));
                    p.push(("rtt_max", r1(w.rtt_max)));
                    p.push(("jitter", r1(w.jitter_ms)));
                    p.push(("input", r1(w.input_rate)));
                    p.push(("gap", n(w.longest_gap_ms as f64)));
                    p.push(("resends", n(w.resends as f64)));
                    p.push(("drops", n(w.flow_drops as f64)));
                    p.push((
                        "health",
                        s(health(
                            w.rtt_mean,
                            w.rtt_max,
                            w.resends as f64,
                            w.flow_drops as f64,
                            age > STALE_SECS || w.datagrams_in == 0,
                        )),
                    ));
                }
                None => {
                    p.push(("report_age_secs", J::Null));
                    p.push(("health", s("none")));
                }
            }
            players.push(o(p));
        }
        let host = g
            .link
            .remote()
            .map_or("local".to_string(), |r| r.agent.clone());
        matches.push(o(vec![
            ("game", n(g.id as f64)),
            (
                "match",
                st.monitor
                    .games
                    .get(&g.id)
                    .map_or(J::Null, |m| n(m.match_id as f64)),
            ),
            ("room", s(format!("{}-{}", g.room.channel, g.room.no))),
            ("mode", s(st.rom.mode_key(g.mode))),
            ("map", s(game_map(st, g).1)),
            ("host", s(host)),
            ("port", n(g.port)),
            ("ready", J::Bool(g.ready)),
            ("players", J::Array(players)),
        ]));
    }
    o(vec![
        ("lobby", lobby),
        ("hosts", hosts),
        ("rooms", rooms),
        ("matches", J::Array(matches)),
    ])
}

fn event_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<J> {
    Ok(o(vec![
        ("id", n(r.get::<_, i64>(0)? as f64)),
        ("at", n(r.get::<_, i64>(1)? as f64)),
        ("account", n(r.get::<_, i64>(2)? as f64)),
        ("nick", s(r.get::<_, String>(3)?)),
        ("kind", s(r.get::<_, String>(4)?)),
        ("problem", J::Bool(r.get::<_, i64>(5)? != 0)),
        ("match", n(r.get::<_, i64>(6)? as f64)),
        ("detail", s(r.get::<_, String>(7)?)),
    ]))
}

const EVENT_COLS: &str = "id, at, user_no, nick, kind, problem, match_id, detail";

/// Recent events, newest first (`problems`: only the problem kinds).
pub fn events_json(c: &Connection, problems: bool, limit: u32) -> rusqlite::Result<J> {
    let sql = format!(
        "SELECT {EVENT_COLS} FROM ops_events {} ORDER BY id DESC LIMIT ?1",
        if problems { "WHERE problem = 1" } else { "" }
    );
    let mut q = c.prepare(&sql)?;
    let rows = q.query_map([limit.clamp(1, 2000) as i64], event_row)?;
    Ok(J::Array(rows.collect::<rusqlite::Result<Vec<_>>>()?))
}

/// Accounts whose character name or login matches `name`: the exact one (case-insensitive) first, then names
/// containing it. `(user_no, nick, login)`.
pub fn find_accounts(c: &Connection, name: &str) -> rusqlite::Result<Vec<(u32, String, String)>> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(Vec::new());
    }
    let like = format!(
        "%{}%",
        name.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let mut q = c.prepare(
        "SELECT a.user_no, COALESCE(c.nick, ''), a.login FROM accounts a LEFT JOIN characters c ON c.user_no = a.user_no \
         WHERE c.nick = ?1 COLLATE NOCASE OR a.login = ?1 COLLATE NOCASE \
         OR c.nick LIKE ?2 ESCAPE '\\' OR a.login LIKE ?2 ESCAPE '\\' \
         ORDER BY (c.nick = ?1 COLLATE NOCASE OR a.login = ?1 COLLATE NOCASE) DESC, a.user_no LIMIT 20",
    )?;
    let rows = q.query_map(params![name, like], |r| {
        Ok((r.get::<_, i64>(0)? as u32, r.get(1)?, r.get(2)?))
    })?;
    rows.collect()
}

/// `GET /ops/players`: one page of the accounts for the dashboard's player list, `total` the count after the filters.
/// `q` matches a part of the login or the character name; bots (LG-317: the bot mark, or a `...bot<digits>` login,
/// the pool seats included) are left out unless `bots`. `sort` is `seen` (last login, the default), `new` (newest
/// account) or `matches`. The whole account table is read and filtered here: the bot rule is not SQL, and a lobby's
/// accounts are thousands at most. (The route calls [`players_json_at`] with the LG-398 filter.)
#[cfg(test)]
pub fn players_json(
    c: &Connection,
    online: &BTreeSet<u32>,
    q: &str,
    bots: bool,
    sort: &str,
    offset: u32,
    limit: u32,
) -> rusqlite::Result<J> {
    let now = (crate::clock::unix_now(), crate::clock::now_ticks());
    players_json_at(c, online, q, bots, false, sort, offset, limit, now)
}

/// [`players_json`] with the LG-398 marks at `now` (unix seconds, local FILETIME ticks): every row says whether the
/// account is banned (a permanent ban or one that has not ended), disabled (LG-142) or muted; `banned_only` keeps the
/// banned ones.
#[allow(clippy::too_many_arguments)]
pub fn players_json_at(
    c: &Connection,
    online: &BTreeSet<u32>,
    q: &str,
    bots: bool,
    banned_only: bool,
    sort: &str,
    offset: u32,
    limit: u32,
    now: (i64, i64),
) -> rusqlite::Result<J> {
    struct P {
        user: u32,
        login: String,
        nick: String,
        created: i64,
        last: i64,
        matches: i64,
        kills: i64,
        deaths: i64,
        gm: bool,
        bot: bool,
        banned: bool,
        disabled: bool,
        muted: bool,
    }
    let needle = q.trim().to_lowercase();
    let mut stmt = c.prepare(
        "SELECT a.user_no, a.login, COALESCE(c.nick, ''), a.created, a.last_login, COALESCE(c.matches, 0), \
         COALESCE(c.kills, 0), COALESCE(c.deaths, 0), a.gm, a.self_bot, a.disabled, COALESCE(c.mute_until, 0), \
         b.until FROM accounts a LEFT JOIN characters c ON c.user_no = a.user_no \
         LEFT JOIN op_bans b ON b.user_no = a.user_no",
    )?;
    let mut all = stmt
        .query_map([], |r| {
            let login: String = r.get(1)?;
            let marked: i64 = r.get(9)?;
            Ok(P {
                user: r.get::<_, i64>(0)? as u32,
                bot: marked != 0 || crate::grant::is_bot_name(&login),
                login,
                nick: r.get(2)?,
                created: r.get(3)?,
                last: r.get(4)?,
                matches: r.get(5)?,
                kills: r.get(6)?,
                deaths: r.get(7)?,
                gm: r.get::<_, i64>(8)? != 0,
                disabled: r.get::<_, i64>(10)? != 0,
                muted: r.get::<_, i64>(11)? > now.1,
                banned: r
                    .get::<_, Option<i64>>(12)?
                    .is_some_and(|u| u == 0 || u > now.0),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    all.retain(|p| {
        (bots || !p.bot)
            && (!banned_only || p.banned)
            && (needle.is_empty()
                || p.login.to_lowercase().contains(&needle)
                || p.nick.to_lowercase().contains(&needle))
    });
    match sort {
        "new" => all.sort_by(|a, b| b.created.cmp(&a.created).then(b.user.cmp(&a.user))),
        "matches" => all.sort_by(|a, b| b.matches.cmp(&a.matches).then(b.last.cmp(&a.last))),
        _ => all.sort_by(|a, b| {
            online
                .contains(&b.user)
                .cmp(&online.contains(&a.user))
                .then(b.last.cmp(&a.last))
                .then(b.user.cmp(&a.user))
        }),
    }
    let total = all.len();
    let page: Vec<J> = all
        .into_iter()
        .skip(offset as usize)
        .take(limit.clamp(1, 200) as usize)
        .map(|p| {
            o(vec![
                ("account", n(p.user as f64)),
                ("online", J::Bool(online.contains(&p.user))),
                ("login", s(p.login)),
                ("nick", s(p.nick)),
                ("created", n(p.created as f64)),
                ("last_login", n(p.last as f64)),
                ("matches", n(p.matches as f64)),
                ("kills", n(p.kills as f64)),
                ("deaths", n(p.deaths as f64)),
                ("gm", J::Bool(p.gm)),
                ("bot", J::Bool(p.bot)),
                ("banned", J::Bool(p.banned)),
                ("disabled", J::Bool(p.disabled)),
                ("muted", J::Bool(p.muted)),
            ])
        })
        .collect();
    Ok(o(vec![
        ("total", n(total as f64)),
        ("offset", n(offset as f64)),
        ("players", J::Array(page)),
    ]))
}

fn player_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<J> {
    let rtt_mean: f64 = r.get(12)?;
    let rtt_max: f64 = r.get(14)?;
    let gap: i64 = r.get(19)?;
    let input: f64 = r.get(17)?;
    let resends: i64 = r.get(20)?;
    let has_link = r.get::<_, i64>(10)? != 0;
    let secs: f64 = r.get(9)?;
    let how: String = r.get(7)?;
    let flow_drops: i64 = r.get(22)?;
    let per10 = |v: f64| if secs > 10.0 { v * 10.0 / secs } else { v };
    Ok(o(vec![
        ("match", n(r.get::<_, i64>(0)? as f64)),
        ("account", n(r.get::<_, i64>(1)? as f64)),
        ("nick", s(r.get::<_, String>(2)?)),
        ("bot", J::Bool(r.get::<_, i64>(3)? != 0)),
        ("client", s(r.get::<_, String>(4)?)),
        ("joined", n(r.get::<_, i64>(5)? as f64)),
        ("left", n(r.get::<_, i64>(6)? as f64)),
        ("how", s(r.get::<_, String>(7)?)),
        ("code", n(r.get::<_, i64>(8)? as f64)),
        ("secs", r1(r.get(9)?)),
        ("has_link", J::Bool(has_link)),
        ("rtt_mean", r1(rtt_mean)),
        ("rtt_p95", r1(r.get(13)?)),
        ("rtt_max", r1(rtt_max)),
        ("rtt_samples", n(r.get::<_, i64>(15)? as f64)),
        ("jitter", r1(r.get(16)?)),
        ("input_rate", r1(input)),
        ("updates", n(r.get::<_, i64>(18)? as f64)),
        ("gap_max", n(gap as f64)),
        ("resends", n(resends as f64)),
        ("reliable", n(r.get::<_, i64>(21)? as f64)),
        ("flow_drops", n(r.get::<_, i64>(22)? as f64)),
        ("dg_in", n(r.get::<_, i64>(23)? as f64)),
        ("dg_out", n(r.get::<_, i64>(24)? as f64)),
        ("max_datagram", n(r.get::<_, i64>(25)? as f64)),
        (
            "health",
            s(if has_link {
                health(
                    rtt_mean,
                    rtt_max,
                    per10(resends as f64),
                    per10(flow_drops as f64),
                    how == "silent",
                )
            } else {
                "none"
            }),
        ),
    ]))
}

const PLAYER_COLS: &str = "match_id, user_no, nick, bot, client, joined, left_at, how, code, secs, has_link, id, \
    rtt_mean, rtt_p95, rtt_max, rtt_samples, jitter, input_rate, updates, gap_max, resends, reliable, flow_drops, dg_in, \
    dg_out, max_datagram";

const MATCH_COLS: &str =
    "id, game, started, ended, channel, room, map, map_world, mode, mode_key, host, host_version, \
    port, players, bots, outcome, winner, alpha, bravo, silent_drops, map_name";

/// A match row's map name; the rows from before the name column hold the world only.
fn name_or_world(name: String, world: &str) -> String {
    if name.is_empty() {
        world.to_string()
    } else {
        name
    }
}

fn match_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<J> {
    let world: String = r.get(7)?;
    Ok(o(vec![
        ("id", n(r.get::<_, i64>(0)? as f64)),
        ("game", n(r.get::<_, i64>(1)? as f64)),
        ("started", n(r.get::<_, i64>(2)? as f64)),
        ("ended", n(r.get::<_, i64>(3)? as f64)),
        (
            "room",
            s(format!("{}-{}", r.get::<_, i64>(4)?, r.get::<_, i64>(5)?)),
        ),
        ("map", n(r.get::<_, i64>(6)? as f64)),
        ("map_world", s(world.clone())),
        ("mode", n(r.get::<_, i64>(8)? as f64)),
        ("mode_key", s(r.get::<_, String>(9)?)),
        ("host", s(r.get::<_, String>(10)?)),
        ("host_version", s(r.get::<_, String>(11)?)),
        ("port", n(r.get::<_, i64>(12)? as f64)),
        ("players", n(r.get::<_, i64>(13)? as f64)),
        ("bots", n(r.get::<_, i64>(14)? as f64)),
        ("outcome", s(r.get::<_, String>(15)?)),
        ("winner", n(r.get::<_, i64>(16)? as f64)),
        ("alpha", n(r.get::<_, i64>(17)? as f64)),
        ("bravo", n(r.get::<_, i64>(18)? as f64)),
        ("silent_drops", n(r.get::<_, i64>(19)? as f64)),
        ("map_name", s(name_or_world(r.get(20)?, &world))),
    ]))
}

/// Recent matches, newest first.
pub fn matches_json(c: &Connection, limit: u32) -> rusqlite::Result<J> {
    let mut q = c.prepare(&format!(
        "SELECT {MATCH_COLS} FROM ops_matches ORDER BY id DESC LIMIT ?1"
    ))?;
    let rows = q.query_map([limit.clamp(1, 1000) as i64], match_row)?;
    Ok(J::Array(rows.collect::<rusqlite::Result<Vec<_>>>()?))
}

/// One match with every player's summary, every `@net` window and its events; `None` for an unknown id.
pub fn match_json(c: &Connection, id: i64) -> rusqlite::Result<Option<J>> {
    let Some(m) = c
        .query_row(
            &format!("SELECT {MATCH_COLS} FROM ops_matches WHERE id = ?1"),
            [id],
            match_row,
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut q = c.prepare(&format!(
        "SELECT {PLAYER_COLS} FROM ops_match_players WHERE match_id = ?1 ORDER BY joined, id"
    ))?;
    let players = q
        .query_map([id], player_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut q = c.prepare(
        "SELECT at, client, user_no, window_ms, rtt, rtt_min, rtt_max, rtt_n, jitter, input, gap, resends, drops, dg_in, dg_out \
         FROM ops_net WHERE match_id = ?1 ORDER BY at, client LIMIT 20000",
    )?;
    let net = q
        .query_map([id], |r| {
            let rtt: f64 = r.get(4)?;
            let rtt_max: f64 = r.get(6)?;
            let input: f64 = r.get(9)?;
            let gap: i64 = r.get(10)?;
            let resends: i64 = r.get(11)?;
            let drops: i64 = r.get(12)?;
            let dg_in: i64 = r.get(13)?;
            Ok(o(vec![
                ("at", n(r.get::<_, i64>(0)? as f64)),
                ("client", s(r.get::<_, String>(1)?)),
                ("account", n(r.get::<_, i64>(2)? as f64)),
                ("window_ms", n(r.get::<_, i64>(3)? as f64)),
                ("rtt", r1(rtt)),
                ("rtt_min", r1(r.get(5)?)),
                ("rtt_max", r1(rtt_max)),
                ("rtt_n", n(r.get::<_, i64>(7)? as f64)),
                ("jitter", r1(r.get(8)?)),
                ("input", r1(input)),
                ("gap", n(gap as f64)),
                ("resends", n(resends as f64)),
                ("drops", n(drops as f64)),
                ("dg_in", n(dg_in as f64)),
                ("dg_out", n(r.get::<_, i64>(14)? as f64)),
                (
                    "health",
                    s(health(
                        rtt,
                        rtt_max,
                        resends as f64,
                        drops as f64,
                        dg_in == 0,
                    )),
                ),
            ]))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut q = c.prepare(&format!(
        "SELECT {EVENT_COLS} FROM ops_events WHERE match_id = ?1 ORDER BY id LIMIT 2000"
    ))?;
    let events = q
        .query_map([id], event_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(o(vec![
        ("match", m),
        ("players", J::Array(players)),
        ("net", J::Array(net)),
        ("events", J::Array(events)),
    ])))
}

/// One account's timeline: its events and its matches, newest first; `None` when no account matches `name`.
pub fn timeline_json(c: &Connection, name: &str, limit: u32) -> rusqlite::Result<Option<J>> {
    let found = find_accounts(c, name)?;
    let Some(&(user, ref nick, ref login)) = found.first() else {
        return Ok(None);
    };
    let limit = limit.clamp(1, 5000) as i64;
    let mut q = c.prepare(&format!(
        "SELECT {EVENT_COLS} FROM ops_events WHERE user_no = ?1 ORDER BY id DESC LIMIT ?2"
    ))?;
    let events = q
        .query_map(params![user as i64, limit], event_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut q = c.prepare(&format!(
        "SELECT {PLAYER_COLS}, m.room, m.channel, m.map_world, m.mode_key, m.host, m.outcome, m.map_name FROM ops_match_players p \
         JOIN ops_matches m ON m.id = p.match_id WHERE p.user_no = ?1 ORDER BY p.joined DESC, p.id DESC LIMIT ?2"
    ).replace("match_id, user_no", "p.match_id, p.user_no").replace(", id,", ", p.id,"))?;
    let matches = q
        .query_map(params![user as i64, limit], |r| {
            let mut p = player_row(r)?;
            if let J::Object(m) = &mut p {
                m.insert(
                    "room".into(),
                    s(format!("{}-{}", r.get::<_, i64>(27)?, r.get::<_, i64>(26)?)),
                );
                let world: String = r.get(28)?;
                m.insert("map_name".into(), s(name_or_world(r.get(32)?, &world)));
                m.insert("map_world".into(), s(world));
                m.insert("mode_key".into(), s(r.get::<_, String>(29)?));
                m.insert("host".into(), s(r.get::<_, String>(30)?));
                m.insert("outcome".into(), s(r.get::<_, String>(31)?));
            }
            Ok(p)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let others: Vec<J> = found
        .iter()
        .skip(1)
        .map(|(u, nk, lg)| {
            o(vec![
                ("account", n(*u)),
                ("nick", s(nk.clone())),
                ("login", s(lg.clone())),
            ])
        })
        .collect();
    Ok(Some(o(vec![
        ("account", n(user)),
        ("nick", s(nick.clone())),
        ("login", s(login.clone())),
        ("events", J::Array(events)),
        ("matches", J::Array(matches)),
        ("other_matches", J::Array(others)),
    ])))
}

// ------------------------------------------------------------------------------------------------ alerts

/// Thresholds of `GET /ops/alerts` (query keys in brackets).
// DC-DEFAULT: MON-5 (proposed) alert thresholds
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// Look-back window in minutes (`window`).
    pub window_min: u32,
    /// A match with at least this many silent drops in the window (`silent`).
    pub silent_drops: u32,
    /// At least this many problem events (lobby faults included) in the window (`errors`).
    pub errors: u32,
    /// At least this many game crashes or failed starts in the window (`crashes`).
    pub crashes: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            window_min: 10,
            silent_drops: 3,
            errors: 30,
            crashes: 2,
        }
    }
}

impl Thresholds {
    /// From the query of `GET /ops/alerts` (a missing or bad value keeps the default; 0 turns a check off).
    pub fn from_query(q: &BTreeMap<String, String>) -> Thresholds {
        let d = Thresholds::default();
        let g = |k: &str, v: u32| q.get(k).and_then(|x| x.trim().parse().ok()).unwrap_or(v);
        Thresholds {
            window_min: g("window", d.window_min).clamp(1, 24 * 60),
            silent_drops: g("silent", d.silent_drops),
            errors: g("errors", d.errors),
            crashes: g("crashes", d.crashes),
        }
    }
}

/// One alert check: `(name, ok, text)`.
pub type Check = (String, bool, String);

/// The checks of the DB part (silent drops, error spike, crashes) at `now`.
pub fn db_checks(c: &Connection, now: i64, t: &Thresholds) -> rusqlite::Result<Vec<Check>> {
    let since = now - t.window_min as i64 * 60;
    let mut out = Vec::new();
    if t.silent_drops > 0 {
        let mut q = c.prepare(
            "SELECT id, channel, room, COALESCE(NULLIF(map_name, ''), map_world), silent_drops FROM ops_matches \
             WHERE silent_drops >= ?1 AND (ended = 0 OR ended >= ?2) AND started >= ?3 ORDER BY id DESC LIMIT 5",
        )?;
        let bad: Vec<String> = q
            .query_map(
                params![t.silent_drops as i64, since, since - 6 * 3600],
                |r| {
                    Ok(format!(
                        "match {} (room {}-{}, {}): {} silent drops",
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?
                    ))
                },
            )?
            .collect::<rusqlite::Result<_>>()?;
        out.push(if bad.is_empty() {
            (
                "silent_drops".into(),
                true,
                format!("no match with {} or more silent drops", t.silent_drops),
            )
        } else {
            ("silent_drops".into(), false, bad.join("; "))
        });
    }
    if t.errors > 0 {
        let (count, faults): (i64, i64) = c.query_row(
            "SELECT COUNT(*), COALESCE(SUM(kind = ?2), 0) FROM ops_events WHERE at >= ?1 AND problem = 1",
            params![since, kind::LOBBY_FAULT],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        out.push((
            "errors".into(),
            count < t.errors as i64,
            format!(
                "{count} problem events in {} min ({faults} lobby errors; limit {})",
                t.window_min, t.errors
            ),
        ));
    }
    if t.crashes > 0 {
        let crashes: i64 = c.query_row(
            "SELECT COUNT(*) FROM ops_matches WHERE ended >= ?1 AND outcome IN ('crash', 'start-failed')",
            [since],
            |r| r.get(0),
        )?;
        out.push((
            "crashes".into(),
            crashes < t.crashes as i64,
            format!(
                "{crashes} game servers crashed or failed to start in {} min (limit {})",
                t.window_min, t.crashes
            ),
        ));
    }
    Ok(out)
}

/// The checks of the live part: every configured host online and on the lobby's build.
pub fn host_checks(st: &Core) -> Vec<Check> {
    let mut out = Vec::new();
    for c in &st.opts.host_agents {
        match st.hosts.online.get(&c.name) {
            None => out.push((
                format!("host.{}", c.name),
                false,
                format!("game host {} offline", c.name),
            )),
            Some(a) => {
                out.push((
                    format!("host.{}", c.name),
                    true,
                    format!("game host {} online", c.name),
                ));
                let same = same_build(&a.version, build_sha());
                out.push((
                    format!("version.{}", c.name),
                    same,
                    if same {
                        format!(
                            "game host {} runs the lobby's build {}",
                            c.name,
                            build_sha()
                        )
                    } else {
                        format!(
                            "game host {} runs build {}, the lobby {}",
                            c.name,
                            a.version,
                            build_sha()
                        )
                    },
                ));
            }
        }
    }
    out
}

/// `GET /ops/alerts`.
pub fn alerts_json(checks: &[Check]) -> J {
    J::Array(
        checks
            .iter()
            .map(|(name, ok, text)| {
                o(vec![
                    ("name", s(name.clone())),
                    ("ok", J::Bool(*ok)),
                    ("text", s(text.clone())),
                ])
            })
            .collect(),
    )
}

// ------------------------------------------------------------------------------------------------ command line

/// `2026-11-02 21:04:05Z` for a unix time (UTC; days from the civil calendar algorithm).
pub fn when(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

fn get<'a>(v: &'a J, k: &str) -> &'a J {
    static NULL: J = J::Null;
    match v {
        J::Object(m) => m.get(k).unwrap_or(&NULL),
        _ => &NULL,
    }
}

fn gs(v: &J, k: &str) -> String {
    match get(v, k) {
        J::String(x) => x.clone(),
        J::Number(x) => format!("{x}"),
        J::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn gn(v: &J, k: &str) -> f64 {
    match get(v, k) {
        J::Number(x) => *x,
        _ => 0.0,
    }
}

fn arr<'a>(v: &'a J, k: &str) -> &'a [J] {
    match get(v, k) {
        J::Array(a) => a,
        _ => &[],
    }
}

fn player_text(p: &J) -> String {
    let link = if get(p, "has_link") == &J::Bool(true) {
        format!(
            "rtt {:.0}/{:.0}/{:.0} ms (mean/p95/max), input {:.1}/s, gap {:.0} ms, resends {:.0}/{:.0}, drops {:.0}, dg in {:.0} out {:.0} (largest {:.0} B) [{}]",
            gn(p, "rtt_mean"),
            gn(p, "rtt_p95"),
            gn(p, "rtt_max"),
            gn(p, "input_rate"),
            gn(p, "gap_max"),
            gn(p, "resends"),
            gn(p, "reliable"),
            gn(p, "flow_drops"),
            gn(p, "dg_in"),
            gn(p, "dg_out"),
            gn(p, "max_datagram"),
            gs(p, "health")
        )
    } else {
        "no link summary".to_string()
    };
    format!(
        "{} {}{} joined {}, {:.0} s, left: {} (code {:.0}); {}",
        gs(p, "nick"),
        gs(p, "account"),
        if get(p, "bot") == &J::Bool(true) {
            " (bot)"
        } else {
            ""
        },
        when(gn(p, "joined") as i64),
        gn(p, "secs"),
        gs(p, "how"),
        gn(p, "code"),
        link
    )
}

fn event_text(e: &J) -> String {
    format!(
        "{} {}{} {}{}: {}",
        when(gn(e, "at") as i64),
        if get(e, "problem") == &J::Bool(true) {
            "!"
        } else {
            " "
        },
        gs(e, "kind"),
        gs(e, "nick"),
        match gn(e, "match") as i64 {
            0 => String::new(),
            m => format!(" (match {m})"),
        },
        gs(e, "detail")
    )
}

/// `dc-lobby-server ops [--db FILE] (timeline <name> | match <id> | matches | events [--problems])`; prints text.
pub fn run_cli(args: &[String]) -> i32 {
    const USAGE: &str = "usage: dc-lobby-server ops [--db FILE] timeline <name> [--limit N]\n\
                         \x20      dc-lobby-server ops [--db FILE] match <id> [--net]\n\
                         \x20      dc-lobby-server ops [--db FILE] matches [--limit N]\n\
                         \x20      dc-lobby-server ops [--db FILE] events [--problems] [--limit N]";
    let mut db = None;
    let mut limit = 50u32;
    let mut problems = false;
    let mut net = false;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--db" => db = it.next().cloned(),
            "--limit" => limit = it.next().and_then(|v| v.parse().ok()).unwrap_or(limit),
            "--problems" => problems = true,
            "--net" => net = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            _ => rest.push(a.clone()),
        }
    }
    let path = db
        .map(std::path::PathBuf::from)
        .unwrap_or_else(crate::cli::default_db);
    if !path.is_file() {
        eprintln!(
            "dc-lobby-server ops: no DB file {} (give --db)",
            path.display()
        );
        return 2;
    }
    let c = match Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("dc-lobby-server ops: cannot open {}: {e}", path.display());
            return 2;
        }
    };
    let _ = c.busy_timeout(Duration::from_secs(5));
    let r = match rest.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["timeline", name @ ..] if !name.is_empty() => cli_timeline(&c, &name.join(" "), limit),
        ["match", id] => match id.parse::<i64>() {
            Ok(id) => cli_match(&c, id, net),
            Err(_) => Err(format!("match id {id:?} is not a number")),
        },
        ["matches"] => matches_json(&c, limit).map_err(|e| e.to_string()).map(|v| {
            if let J::Array(a) = v {
                for m in a {
                    println!(
                        "{:>6} {} room {} {} {} on {} ({}): {:.0} players, {:.0} bots, {}, {:.0} silent drops",
                        gs(&m, "id"),
                        when(gn(&m, "started") as i64),
                        gs(&m, "room"),
                        gs(&m, "mode_key"),
                        gs(&m, "map_name"),
                        gs(&m, "host"),
                        gs(&m, "host_version"),
                        gn(&m, "players"),
                        gn(&m, "bots"),
                        gs(&m, "outcome"),
                        gn(&m, "silent_drops")
                    );
                }
            }
        }),
        ["events"] => events_json(&c, problems, limit).map_err(|e| e.to_string()).map(|v| {
            if let J::Array(a) = v {
                for e in a.iter().rev() {
                    println!("{}", event_text(e));
                }
            }
        }),
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("dc-lobby-server ops: {e}");
            1
        }
    }
}

fn cli_timeline(c: &Connection, name: &str, limit: u32) -> Result<(), String> {
    let Some(t) = timeline_json(c, name, limit).map_err(|e| e.to_string())? else {
        return Err(format!("no account or character named like {name:?}"));
    };
    println!(
        "account {} login {} nick {}",
        gs(&t, "account"),
        gs(&t, "login"),
        gs(&t, "nick")
    );
    for x in arr(&t, "other_matches") {
        println!(
            "  (also matches: {} {} {})",
            gs(x, "account"),
            gs(x, "login"),
            gs(x, "nick")
        );
    }
    println!("-- matches (newest first)");
    for p in arr(&t, "matches") {
        println!(
            "match {} room {} {} {} on {} ({}): {}",
            gs(p, "match"),
            gs(p, "room"),
            gs(p, "mode_key"),
            gs(p, "map_name"),
            gs(p, "host"),
            gs(p, "outcome"),
            player_text(p)
        );
    }
    println!("-- events (oldest first)");
    for e in arr(&t, "events").iter().rev() {
        println!("{}", event_text(e));
    }
    Ok(())
}

fn cli_match(c: &Connection, id: i64, net: bool) -> Result<(), String> {
    let Some(m) = match_json(c, id).map_err(|e| e.to_string())? else {
        return Err(format!("no match {id}"));
    };
    let h = get(&m, "match");
    println!(
        "match {} (game {}) room {} {} {} on {} build {} port {}: started {}, ended {}, {}, winner {} ({}:{}), {} players, {} bots, {} silent drops",
        gs(h, "id"),
        gs(h, "game"),
        gs(h, "room"),
        gs(h, "mode_key"),
        gs(h, "map_name"),
        gs(h, "host"),
        gs(h, "host_version"),
        gs(h, "port"),
        when(gn(h, "started") as i64),
        match gn(h, "ended") as i64 {
            0 => "-".to_string(),
            t => when(t),
        },
        gs(h, "outcome"),
        gs(h, "winner"),
        gs(h, "alpha"),
        gs(h, "bravo"),
        gs(h, "players"),
        gs(h, "bots"),
        gs(h, "silent_drops")
    );
    println!("-- players");
    for p in arr(&m, "players") {
        println!("{}", player_text(p));
    }
    println!("-- events");
    for e in arr(&m, "events") {
        println!("{}", event_text(e));
    }
    let lines = arr(&m, "net");
    if net {
        println!("-- link windows (10 s)");
        for w in lines {
            println!(
                "{} client {} account {}: rtt {:.0} ms (min {:.0}, max {:.0}, {:.0} samples), jitter {:.1}, input {:.1}/s, gap {:.0} ms, resends {:.0}, drops {:.0}, in {:.0} out {:.0} [{}]",
                when(gn(w, "at") as i64),
                gs(w, "client"),
                gs(w, "account"),
                gn(w, "rtt"),
                gn(w, "rtt_min"),
                gn(w, "rtt_max"),
                gn(w, "rtt_n"),
                gn(w, "jitter"),
                gn(w, "input"),
                gn(w, "gap"),
                gn(w, "resends"),
                gn(w, "drops"),
                gn(w, "dg_in"),
                gn(w, "dg_out"),
                gs(w, "health")
            );
        }
    } else {
        println!("-- {} link windows (10 s); --net prints them", lines.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room::RoomKey;

    fn account(st: &Core, login: &str, nick: &str) -> u32 {
        let u = crate::db::create_account(&st.db, login, 0).unwrap();
        crate::db::create_character(
            &st.db,
            &crate::db::NewCharacter {
                user_no: u,
                nick,
                face: 0,
                exp: 0,
                gp: 0,
                nx: 0,
                now: 0,
            },
            true,
        )
        .unwrap();
        u
    }

    fn game(id: u64) -> crate::game::Game {
        crate::game::Game {
            id,
            room: RoomKey { channel: 1, no: 5 },
            port: 27015,
            map: 0,
            mode: 0,
            link: Default::default(),
            ready: false,
            over: false,
            players: Vec::new(),
            clients: Default::default(),
            left: Default::default(),
            result_done: false,
            entry_keys: Default::default(),
            seat_tickets: Default::default(),
            seat_told: Default::default(),
            reap_scheduled: false,
            tmp_workdir: None,
            hisec_keys: Default::default(),
            net: Default::default(),
        }
    }

    fn line(st: &mut Core, g: u64, l: &str) {
        st.scope(|st| crate::game::on_line(st, g, l)).unwrap();
    }

    #[test]
    fn public_board_uses_client_numbers_and_excludes_private_or_ended_games() {
        use crate::room::RoomPhase;
        let mut st = crate::botgroups::tests::three_entries();
        let (mut client, _) = crate::flow_tests::player(&mut st, 1, 2, "tester", "PrivateNick");
        let no = crate::flow_tests::open_room(&mut st, &mut client, "");
        let key = RoomKey { channel: 4, no };
        let mut room = st.rooms.remove(&RoomKey { channel: 1, no }).unwrap();
        room.key = key;
        room.game = Some(9);
        room.gun_game = true;
        room.state = RoomPhase::Playing;
        room.title = "PrivateTitle".into();
        st.rooms.insert(key, room);
        let mut g = game(9);
        g.room = key;
        g.ready = true;
        st.games.insert(9, g);
        let snapshot = crate::public_games::snapshot;
        let board = snapshot(&st);
        let row = &board["games"][0];
        assert_eq!(row["server"], "US West");
        assert_eq!(row["channel"], 1);
        assert_eq!(row["room"], 1);
        assert_eq!(row["mode"], "Gun Game");
        assert_eq!(row["players"], 1);
        assert_eq!(row.as_object().unwrap().len(), 8);
        assert!(!board.to_string().contains("Private"));
        let empty = |st: &Core| assert_eq!(snapshot(st)["games"], serde_json::json!([]));
        st.rooms.get_mut(&key).unwrap().password = "locked".into();
        empty(&st);
        st.rooms.get_mut(&key).unwrap().password.clear();
        st.games.get_mut(&9).unwrap().over = true;
        empty(&st);
        st.games.get_mut(&9).unwrap().over = false;
        st.rooms.get_mut(&key).unwrap().state = RoomPhase::Waiting;
        empty(&st);
        let clan_key = RoomKey {
            channel: crate::clan::CLAN_CHANNEL_BASE,
            no,
        };
        let mut room = st.rooms.remove(&key).unwrap();
        room.key = clan_key;
        room.state = RoomPhase::Playing;
        st.rooms.insert(clan_key, room);
        st.games.get_mut(&9).unwrap().room = clan_key;
        empty(&st);
    }

    fn count(c: &Connection, sql: &str) -> i64 {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn kinds(c: &Connection, sql: &str) -> Vec<String> {
        c.prepare(sql)
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    // the line dc-dserver's pipe::linksum_line writes (its own test pins the same text)
    const SUM: &str = "@linksum client=0 uid=UID how=silent code=3 secs=312.5 rtt=61 rtt_p95=95 rtt_max=410 \
                       rtt_n=1200 jitter=3.2 input=29.8 updates=9300 gap=840 resends=7 rel=2100 drops=1 in=19000 \
                       out=9400 maxdg=548";

    /// OPS-13: a match row keeps the map's name; a row from before the column shows its world; a database from before
    /// the column gets it on open.
    #[test]
    fn a_match_row_shows_its_map_name_and_an_old_database_gains_the_column() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let old: Vec<&str> = SCHEMA.lines().filter(|l| !l.contains("map_name")).collect();
        conn.execute_batch(&old.join("\n")).unwrap();
        assert!(conn.prepare("SELECT map_name FROM ops_matches").is_err());
        conn.execute(
            "INSERT INTO ops_matches (started, map_world) VALUES (1, 'Worlds/Old')",
            [],
        )
        .unwrap();
        crate::db::prepare(&conn).unwrap();
        conn.execute(
            "INSERT INTO ops_matches (started, map_world, map_name) VALUES (2, 'C:/mods/x/world.dat', 'Freight Hall')",
            [],
        )
        .unwrap();
        let v = matches_json(&conn, 10).unwrap();
        let J::Array(rows) = v else { panic!("{v:?}") };
        let names: Vec<(String, String)> = rows
            .iter()
            .map(|r| (gs(r, "map_name"), gs(r, "map_world")))
            .collect();
        assert_eq!(
            names,
            [
                (
                    "Freight Hall".to_string(),
                    "C:/mods/x/world.dat".to_string()
                ),
                ("Worlds/Old".to_string(), "Worlds/Old".to_string())
            ]
        );
    }

    #[test]
    fn linksum_lines_parse_and_bad_ones_do_not() {
        let s = parse_linksum(&SUM.replace("UID", "1006")).unwrap();
        assert_eq!(
            (s.uid, s.how.as_str(), s.code, s.client.as_str()),
            (1006, "silent", 3, "0")
        );
        assert_eq!(
            (s.secs, s.rtt_mean, s.rtt_p95, s.rtt_max),
            (312.5, 61.0, 95.0, 410.0)
        );
        assert_eq!(
            (s.rtt_samples, s.updates, s.gap_max, s.resends, s.reliable),
            (1200, 9300, 840, 7, 2100)
        );
        assert_eq!(
            (s.flow_drops, s.dg_in, s.dg_out, s.max_datagram),
            (1, 19000, 9400, 548)
        );
        assert!((s.input_rate - 29.8).abs() < 1e-9 && (s.jitter - 3.2).abs() < 1e-9);
        assert!(parse_linksum("@net client=0 uid=1").is_none());
        assert!(
            parse_linksum("@linksum client=0 how=bye").is_none(),
            "no uid"
        );
        let odd = parse_linksum("@linksum uid=5 how=<b>x rtt=-4 secs=nan").unwrap();
        assert_eq!((odd.how.as_str(), odd.rtt_mean, odd.secs), ("bx", 0.0, 0.0));
    }

    #[test]
    fn a_match_with_its_pipe_lines_is_stored_with_players_windows_and_events() {
        let mut st = crate::state::testing::state();
        let alice = account(&st, "alice", "Alice");
        let bot = account(&st, "bot1", "Bot1");
        st.games.insert(7, game(7));
        st.scope(|st| {
            match_started(st, 7);
            Ok(())
        })
        .unwrap();
        line(&mut st, 7, "@ready port=27016 world=w");
        line(&mut st, 7, &format!("@join 0 {alice} 11 0 Alice"));
        line(&mut st, 7, &format!("@join 1 {bot} 12 1 Bot1"));
        line(
            &mut st,
            7,
            &format!(
                "@net client=0 uid={alice} ms=10000 rtt=52 rtt_min=40 rtt_max=90 rtt_n=9 jitter=2.5 input=30.0 \
                 gap=60 resends=1 drops=0 in=600 out=300"
            ),
        );
        line(
            &mut st,
            7,
            "@net client=1 uid=0 ms=10000 rtt=20 rtt_min=20 rtt_max=20 rtt_n=1 jitter=0 input=30 gap=33 resends=0 \
             drops=0 in=1 out=1",
        );
        line(&mut st, 7, &SUM.replace("UID", &alice.to_string()));
        line(
            &mut st,
            7,
            "@result mode=TDM win=1 alpha=3 bravo=1 limit=3 rounds=1:0",
        );
        line(
            &mut st,
            7,
            &format!(
                "@linksum client=1 uid={bot} how=end code=0 secs=300 rtt=20 rtt_p95=25 rtt_max=30 rtt_n=100 \
                 jitter=0.5 input=30 updates=9000 gap=40 resends=0 rel=50 drops=0 in=9000 out=9000 maxdg=400"
            ),
        );
        line(&mut st, 7, "@end");
        let c = &st.db;
        let (outcome, players, bots, silent, port, winner): (String, i64, i64, i64, i64, i64) = c
            .query_row(
                "SELECT outcome, players, bots, silent_drops, port, winner FROM ops_matches WHERE game = 7",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(
            (outcome.as_str(), players, bots, silent, winner),
            ("result", 1, 1, 1, 1)
        );
        assert_eq!(
            port, 27015,
            "the start's port stays; @ready fills only a missing one"
        );
        assert_eq!(count(c, "SELECT COUNT(*) FROM ops_match_players"), 2);
        let (how, p95, maxdg, isbot): (String, f64, i64, i64) = c
            .query_row(
                "SELECT how, rtt_p95, max_datagram, bot FROM ops_match_players WHERE user_no = ?1",
                [alice],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!((how.as_str(), p95, maxdg, isbot), ("silent", 95.0, 548, 0));
        assert_eq!(count(c, "SELECT COUNT(*) FROM ops_net"), 2);
        assert_eq!(
            count(
                c,
                &format!("SELECT COUNT(*) FROM ops_net WHERE client = '1' AND user_no = {bot}")
            ),
            1,
            "a window without uid gets the account of its @join"
        );
        // the human's timeline: the join and the silent drop, both tied to the match; the bot has no events
        assert_eq!(
            kinds(
                c,
                &format!("SELECT kind FROM ops_events WHERE user_no = {alice} ORDER BY id")
            ),
            vec![kind::MATCH_JOIN, kind::SILENT_DROP]
        );
        assert_eq!(
            count(
                c,
                &format!("SELECT COUNT(*) FROM ops_events WHERE user_no = {bot}")
            ),
            0
        );
        let id = count(c, "SELECT id FROM ops_matches WHERE game = 7");
        assert_eq!(
            count(
                c,
                &format!("SELECT COUNT(*) FROM ops_events WHERE match_id = {id}")
            ),
            2
        );
        let m = match_json(c, id).unwrap().unwrap().to_json_string();
        assert!(
            m.contains("\"how\":\"silent\"") && m.contains("\"rtt_p95\":95"),
            "{m}"
        );
        let t = timeline_json(c, "alice", 50)
            .unwrap()
            .unwrap()
            .to_json_string();
        assert!(
            t.contains("\"nick\":\"Alice\"") && t.contains("silent_drop"),
            "{t}"
        );
        assert!(
            timeline_json(c, "lic", 50).unwrap().is_some(),
            "a part of the name finds it"
        );
        assert!(timeline_json(c, "nobody", 50).unwrap().is_none());
        assert!(
            timeline_json(c, "%", 50).unwrap().is_none(),
            "LIKE wildcards are plain text"
        );
        // the EOF after `@end` changes nothing
        st.scope(|st| {
            crate::game::close_game(st, 7, true);
            Ok(())
        })
        .unwrap();
        let outcome: String = st
            .db
            .query_row("SELECT outcome FROM ops_matches WHERE game = 7", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(outcome, "result");
        assert_eq!(count(&st.db, "SELECT COUNT(*) FROM ops_match_players"), 2);
    }

    #[test]
    fn a_crash_is_the_outcome_and_players_without_a_summary_get_a_row() {
        let mut st = crate::state::testing::state();
        let alice = account(&st, "alice", "Alice");
        st.games.insert(3, game(3));
        line(&mut st, 3, "@ready port=27015 world=w");
        line(&mut st, 3, &format!("@join 0 {alice} 11 0 Alice"));
        st.scope(|st| {
            crate::game::close_game(st, 3, true);
            Ok(())
        })
        .unwrap();
        let c = &st.db;
        let (outcome, players): (String, i64) = c
            .query_row("SELECT outcome, players FROM ops_matches", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((outcome.as_str(), players), ("crash", 1));
        let (how, link): (String, i64) = c
            .query_row("SELECT how, has_link FROM ops_match_players", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((how.as_str(), link), ("crash", 0));
        assert_eq!(
            count(
                c,
                "SELECT COUNT(*) FROM ops_events WHERE kind = 'game_crash' AND problem = 1"
            ),
            1
        );
        // a server that never got ready failed its start
        st.games.insert(4, game(4));
        st.scope(|st| {
            crate::game::close_game(st, 4, true);
            Ok(())
        })
        .unwrap();
        let outcome: String = st
            .db
            .query_row("SELECT outcome FROM ops_matches WHERE game = 4", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(outcome, "start-failed");
    }

    #[test]
    fn events_are_problems_by_kind_and_a_disconnect_only_in_a_match() {
        let st = crate::state::testing::state();
        let a = account(&st, "alice", "Alice");
        event(&st, a, kind::LOGIN, "lobby login");
        event(
            &st,
            a,
            kind::DISCONNECT,
            "lobby connection 4 closed (in room 1-2)",
        );
        event(
            &st,
            a,
            kind::DISCONNECT,
            "lobby connection 5 closed (in room 1-2, in a match)",
        );
        event(&st, a, kind::READY_REFUSED, "weapon limit 6");
        // a refused entry shows in the account events, not in the problems
        event(
            &st,
            a,
            kind::JOIN_REFUSED,
            "room 1-2: entry refused: kicked",
        );
        diagnostics_uploaded(&st, a, "r17", "crash report");
        fault(&st, "db: locked");
        assert_eq!(
            kinds(
                &st.db,
                "SELECT kind FROM ops_events WHERE problem = 1 ORDER BY id"
            ),
            vec![
                kind::DISCONNECT,
                kind::READY_REFUSED,
                kind::DIAGNOSTICS,
                kind::LOBBY_FAULT
            ]
        );
        let all = events_json(&st.db, false, 100).unwrap().to_json_string();
        assert!(
            all.contains("\"nick\":\"Alice\"") && all.contains("diagnostics r17"),
            "{all}"
        );
        let only = events_json(&st.db, true, 100).unwrap().to_json_string();
        assert!(!only.contains("\"kind\":\"login\""), "{only}");
        event(&st, a, kind::REPORT, &"x".repeat(2000));
        let kept: String = st
            .db
            .query_row(
                "SELECT detail FROM ops_events ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept.chars().count(), MAX_DETAIL + 3);
    }

    #[test]
    fn health_colours() {
        assert_eq!(health(40.0, 90.0, 0.0, 0.0, false), "good");
        assert_eq!(health(130.0, 200.0, 0.0, 0.0, false), "warn");
        assert_eq!(health(40.0, 450.0, 0.0, 0.0, false), "warn");
        assert_eq!(health(40.0, 90.0, 11.0, 0.0, false), "warn");
        assert_eq!(health(40.0, 90.0, 0.0, 1.0, false), "warn");
        assert_eq!(health(210.0, 300.0, 0.0, 0.0, false), "bad");
        assert_eq!(health(1.0, 2.0, 0.0, 0.0, true), "bad", "nothing arrived");
    }

    #[test]
    fn bot_logins_are_not_events() {
        let st = crate::state::testing::state();
        let b = account(&st, "bot7", "Bot7");
        event(&st, b, kind::LOGIN, "lobby login");
        event(&st, b, kind::DISCONNECT, "closed (in room 1-1, in a match)");
        event(&st, b, kind::SEAT_REFUSED, "seat");
        assert_eq!(
            kinds(&st.db, "SELECT kind FROM ops_events"),
            vec![kind::SEAT_REFUSED]
        );
    }

    #[test]
    fn retention_deletes_only_rows_older_than_the_limit() {
        let mut st = crate::state::testing::state();
        let now = 100 * 86_400;
        {
            let c = &st.db;
            for (id, t) in [(1, now - 15 * 86_400), (2, now - 13 * 86_400)] {
                c.execute(
                    "INSERT INTO ops_matches (id, started) VALUES (?1, ?2)",
                    params![id, t],
                )
                .unwrap();
                c.execute("INSERT INTO ops_match_players (match_id) VALUES (?1)", [id])
                    .unwrap();
                c.execute(
                    "INSERT INTO ops_net (match_id, at) VALUES (?1, ?2)",
                    params![id, t],
                )
                .unwrap();
                c.execute(
                    "INSERT INTO ops_events (at, kind) VALUES (?1, 'login')",
                    [t],
                )
                .unwrap();
            }
            assert_eq!(purge(c, now, 0).unwrap(), 0, "0 days keeps everything");
            assert_eq!(purge(c, now, 14).unwrap(), 4);
            for t in ["ops_matches", "ops_match_players", "ops_net", "ops_events"] {
                assert_eq!(count(c, &format!("SELECT COUNT(*) FROM {t}")), 1, "{t}");
            }
            assert_eq!(count(c, "SELECT match_id FROM ops_match_players"), 2);
        }
        // the lobby purges at most once an hour, with the configured days
        st.rules.ops.monitor_retention_days = 1;
        maybe_purge(&mut st);
        assert_eq!(
            count(&st.db, "SELECT COUNT(*) FROM ops_matches"),
            0,
            "13 days > 1 day"
        );
        st.db
            .execute("INSERT INTO ops_events (at, kind) VALUES (0, 'login')", [])
            .unwrap();
        maybe_purge(&mut st);
        assert_eq!(
            count(&st.db, "SELECT COUNT(*) FROM ops_events"),
            1,
            "not again within the hour"
        );
    }

    #[test]
    fn alert_thresholds() {
        let st = crate::state::testing::state();
        let c = &st.db;
        let now = 1_000_000;
        let t = Thresholds::default();
        let ok = |checks: &[Check], name: &str| checks.iter().find(|x| x.0 == name).map(|x| x.1);
        let checks = db_checks(c, now, &t).unwrap();
        assert_eq!(
            (
                ok(&checks, "silent_drops"),
                ok(&checks, "errors"),
                ok(&checks, "crashes")
            ),
            (Some(true), Some(true), Some(true))
        );
        // a running match with 3 silent drops; an older one with 5 that ended an hour ago, a crash
        c.execute(
            "INSERT INTO ops_matches (started, ended, silent_drops, map_world, channel, room) \
             VALUES (?1, 0, 3, 'W', 1, 2)",
            [now - 300],
        )
        .unwrap();
        c.execute(
            "INSERT INTO ops_matches (started, ended, silent_drops, outcome) VALUES (?1, ?2, 5, 'crash')",
            params![now - 7200, now - 3600],
        )
        .unwrap();
        let checks = db_checks(c, now, &t).unwrap();
        let silent = checks.iter().find(|x| x.0 == "silent_drops").unwrap();
        assert!(
            !silent.1
                && silent.2.contains("match 1 (room 1-2, W): 3 silent drops")
                && !silent.2.contains("match 2"),
            "{silent:?}"
        );
        assert_eq!(
            ok(&checks, "crashes"),
            Some(true),
            "the crash is outside the window"
        );
        let wide = Thresholds {
            window_min: 120,
            ..t
        };
        assert_eq!(
            ok(&db_checks(c, now, &wide).unwrap(), "crashes"),
            Some(true),
            "one crash < 2"
        );
        let strict = Thresholds {
            window_min: 120,
            crashes: 1,
            silent_drops: 4,
            ..t
        };
        let checks = db_checks(c, now, &strict).unwrap();
        assert_eq!(
            (ok(&checks, "crashes"), ok(&checks, "silent_drops")),
            (Some(false), Some(false))
        );
        // an error spike: 30 problem events in the window; info events do not count
        for i in 0..29 {
            put_event(c, now - i, 0, "", kind::SEAT_REFUSED, 0, "");
            put_event(c, now - i, 0, "", kind::LOGIN, 0, "");
        }
        assert_eq!(ok(&db_checks(c, now, &t).unwrap(), "errors"), Some(true));
        put_event(c, now, 0, "", kind::LOBBY_FAULT, 0, "db");
        let checks = db_checks(c, now, &t).unwrap();
        let e = checks.iter().find(|x| x.0 == "errors").unwrap();
        assert!(
            !e.1 && e.2.contains("30 problem events") && e.2.contains("1 lobby errors"),
            "{e:?}"
        );
        // 0 turns a check off; a bad value keeps the default; the window is capped at a day
        let q: BTreeMap<String, String> = [("silent", "0"), ("errors", "x"), ("window", "99999")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let off = Thresholds::from_query(&q);
        assert_eq!(
            (off.silent_drops, off.errors, off.window_min),
            (0, 30, 24 * 60)
        );
        assert!(db_checks(c, now, &off)
            .unwrap()
            .iter()
            .all(|x| x.0 != "silent_drops"));
    }

    #[test]
    fn builds_compare_by_prefix_and_unknown_never_mismatches() {
        assert!(same_build("abc1234", "abc1234def") && same_build("ABC1234", "abc1234"));
        assert!(!same_build("abc1234", "def5678"));
        assert!(same_build("unknown", "def5678") && same_build("", "x"));
        let st = crate::state::testing::state();
        assert!(
            host_checks(&st).is_empty(),
            "no hosts configured, nothing to check"
        );
    }

    #[test]
    fn the_player_list_pages_filters_and_leaves_bots_out() {
        let st = crate::state::testing::state();
        for (i, login) in ["anna", "poolbot3", "marked", "bert", "carla"]
            .iter()
            .enumerate()
        {
            st.db
                .execute(
                    "INSERT INTO accounts (login, created, last_login, self_bot) VALUES (?1, ?2, ?3, ?4)",
                    params![login, 100 + i as i64, 1000 + i as i64, (*login == "marked") as i64],
                )
                .unwrap();
        }
        let names = |v: &J| -> Vec<String> {
            let t = v.to_json_string();
            t.split("\"login\":\"")
                .skip(1)
                .map(|x| x[..x.find('"').unwrap()].to_string())
                .collect()
        };
        let none = BTreeSet::new();
        let v = players_json(&st.db, &none, "", false, "", 0, 50).unwrap();
        assert_eq!(
            names(&v),
            ["carla", "bert", "anna"],
            "bots out, last seen first"
        );
        assert!(v.to_json_string().contains("\"total\":3"));
        let v = players_json(&st.db, &none, "", true, "", 0, 50).unwrap();
        assert!(v.to_json_string().contains("\"total\":5"));
        let page = players_json(&st.db, &none, "", false, "", 1, 1).unwrap();
        assert_eq!(names(&page), ["bert"], "offset 1, limit 1");
        assert_eq!(
            names(&players_json(&st.db, &none, "ER", false, "", 0, 50).unwrap()),
            ["bert"]
        );
        let online: BTreeSet<u32> = [1u32].into_iter().collect();
        let v = players_json(&st.db, &online, "", false, "seen", 0, 50).unwrap();
        assert_eq!(names(&v)[0], "anna", "people online first");
    }

    #[test]
    fn the_live_view_lists_the_lobby_machine_as_the_first_host() {
        let st = crate::state::testing::state();
        let text = live_json(&st).to_json_string();
        let hosts = &text[text.find("\"hosts\"").expect("hosts")..];
        let first = &hosts[..hosts.find('}').expect("an entry")];
        assert!(
            first.contains("this server (lobby)") && first.contains("\"local\":true"),
            "{first}"
        );
        assert!(
            first.contains("\"online\":true") && first.contains("\"games\":0"),
            "{first}"
        );
    }

    #[test]
    fn utc_text() {
        assert_eq!(when(0), "1970-01-01 00:00:00Z");
        assert_eq!(when(951_782_400), "2000-02-29 00:00:00Z");
        assert_eq!(when(1_793_651_045), "2026-11-02 20:24:05Z");
    }
}
