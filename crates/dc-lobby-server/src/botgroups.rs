//! Bot groups the lobby runs (docs/BOTS.md "Bots the lobby runs"): their table, the checks of a group, the operator
//! dashboard's JSON (`GET /ops/bots`) and its writes (`POST /ops/bots/group`, `/ops/bots/delete`, `/ops/bots/start`,
//! `/ops/bots/stop`). The processes themselves are botsup.rs's.
//!
//! A write needs a JSON body, an `Origin` equal to the page's own origin and the page's token in `X-Admin-CSRF`: on the
//! loopback admin port `http://<Host>` and the lobby's per-process page token (LG-375's), on the admin web its public
//! origin and the session's token (LG-387, which adminweb.rs checks before as well).
// Provenance: ORIGINAL (DeCombat operator tooling)
// DC-DEFAULT: LG-389 groups and their checks; LG-390 the writes, their guard and the ops events; LG-391 the status

use crate::betaweb::{header, WriteGuard};
use crate::clanweb::Request;
use crate::servers::Entry;
use crate::state::{Core, Shared};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// The groups, and the loadout spec last given to each bot account (so a lobby restart does not hand it out again).
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS bot_groups (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    prefix    TEXT NOT NULL UNIQUE COLLATE NOCASE,
    count     INTEGER NOT NULL DEFAULT 1,
    server_id INTEGER NOT NULL DEFAULT 1,
    channels  TEXT NOT NULL DEFAULT '',
    room_mode TEXT NOT NULL DEFAULT 'any',
    room_arg  TEXT NOT NULL DEFAULT '',
    skill     TEXT NOT NULL DEFAULT '30-65',
    loadout   TEXT NOT NULL DEFAULT '',
    enabled   INTEGER NOT NULL DEFAULT 0,
    created   INTEGER NOT NULL DEFAULT 0,
    updated   INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS bot_loadouts (
    login TEXT PRIMARY KEY COLLATE NOCASE,
    spec  TEXT NOT NULL DEFAULT '',
    at    INTEGER NOT NULL DEFAULT 0
);
"#;

/// Bots of one group: a room holds 8, one seat stays a person's.
pub const MAX_PER_GROUP: u32 = 7;
/// The skill range of a group that names none (as `tools/vps/bots.sh`).
pub const BOT_SKILL_RANGE: &str = "30-65";
/// Longest prefix: the names `<prefix>1..7` stay well inside a character name's 16 units.
pub const MAX_PREFIX: usize = 12;
/// Longest room title part.
pub const MAX_TITLE: usize = 30;
/// Longest loadout spec.
pub const MAX_LOADOUT: usize = 200;

/// Which room the bots of a group take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoomPick {
    /// The first listed room with a person in it (`--room any --filler <prefix>`).
    Any,
    /// The room of that player, and only while they are in it (`--filler =<nick>`).
    Follow(String),
    /// The first listed room with a person whose title holds this text (`--room-title`).
    Title(String),
}

impl RoomPick {
    pub fn mode(&self) -> &'static str {
        match self {
            RoomPick::Any => "any",
            RoomPick::Follow(_) => "follow",
            RoomPick::Title(_) => "title",
        }
    }

    pub fn arg(&self) -> &str {
        match self {
            RoomPick::Any => "",
            RoomPick::Follow(s) | RoomPick::Title(s) => s,
        }
    }

    fn from_parts(mode: &str, arg: &str) -> Result<RoomPick, String> {
        let arg = arg.trim();
        match mode.trim() {
            "" | "any" => Ok(RoomPick::Any),
            "follow" => {
                if !crate::auth::valid_nick(arg) {
                    return Err("room: follow needs the player's character name (1..16 characters)".into());
                }
                Ok(RoomPick::Follow(arg.to_string()))
            }
            "title" => {
                if arg.is_empty() || arg.chars().count() > MAX_TITLE || arg.chars().any(char::is_control) {
                    return Err(format!(
                        "room: title needs a part of the room's title (1..{MAX_TITLE} characters)"
                    ));
                }
                Ok(RoomPick::Title(arg.to_string()))
            }
            other => Err(format!("room: {other:?} is not any, follow or title")),
        }
    }
}

/// One group: `count` bots named `<prefix>1..`, on entry `server_id`, bot `i` on `channels[(i - 1) % len]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub id: i64,
    pub prefix: String,
    pub count: u32,
    pub server_id: u32,
    pub channels: Vec<u32>,
    pub room: RoomPick,
    pub skill: String,
    /// `account loadout` spec given to each bot before its next start (LG-251); "" = leave the kit alone.
    pub loadout: String,
    pub enabled: bool,
}

impl Group {
    /// The bots' names, `<prefix>1..<prefix><count>`.
    pub fn names(&self) -> Vec<String> {
        (1..=self.count).map(|i| format!("{}{i}", self.prefix)).collect()
    }

    /// The channel of bot `i` (1-based) of `channels`: round-robin.
    pub fn channel_of(channels: &[u32], i: u32) -> u32 {
        if channels.is_empty() {
            return 0;
        }
        channels[((i.max(1) - 1) as usize) % channels.len()]
    }

    pub fn json(&self) -> Value {
        json!({
            "id": self.id, "prefix": self.prefix, "count": self.count, "server_id": self.server_id,
            "channels": self.channels, "room": self.room.mode(), "room_arg": self.room.arg(),
            "skill": self.skill, "loadout": self.loadout, "enabled": self.enabled,
        })
    }
}

fn channels_text(v: &[u32]) -> String {
    v.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Group> {
    let channels: String = r.get(4)?;
    let mode: String = r.get(5)?;
    let arg: String = r.get(6)?;
    Ok(Group {
        id: r.get(0)?,
        prefix: r.get(1)?,
        count: r.get::<_, i64>(2)?.clamp(0, MAX_PER_GROUP as i64) as u32,
        server_id: r.get::<_, i64>(3)?.max(0) as u32,
        channels: channels.split(',').filter_map(|c| c.trim().parse().ok()).collect(),
        room: RoomPick::from_parts(&mode, &arg).unwrap_or(RoomPick::Any),
        skill: r.get(7)?,
        loadout: r.get(8)?,
        enabled: r.get::<_, i64>(9)? != 0,
    })
}

const COLS: &str = "id, prefix, count, server_id, channels, room_mode, room_arg, skill, loadout, enabled";

/// Every group, by id.
pub fn list(c: &Connection) -> rusqlite::Result<Vec<Group>> {
    let mut q = c.prepare(&format!("SELECT {COLS} FROM bot_groups ORDER BY id"))?;
    let rows = q.query_map([], row)?;
    rows.collect()
}

#[cfg(test)]
pub fn get(c: &Connection, id: i64) -> rusqlite::Result<Option<Group>> {
    c.query_row(&format!("SELECT {COLS} FROM bot_groups WHERE id = ?1"), [id], row)
        .optional()
}

/// Inserts (`id` 0) or updates a group; returns its id.
pub fn save(c: &Connection, g: &Group, now: i64) -> rusqlite::Result<i64> {
    let p = params![
        g.prefix,
        g.count as i64,
        g.server_id as i64,
        channels_text(&g.channels),
        g.room.mode(),
        g.room.arg(),
        g.skill,
        g.loadout,
        g.enabled as i64,
        now
    ];
    if g.id == 0 {
        c.execute(
            "INSERT INTO bot_groups (prefix, count, server_id, channels, room_mode, room_arg, skill, loadout, enabled, \
             created, updated) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            p,
        )?;
        Ok(c.last_insert_rowid())
    } else {
        c.execute(
            &format!(
                "UPDATE bot_groups SET prefix = ?1, count = ?2, server_id = ?3, channels = ?4, room_mode = ?5, \
                 room_arg = ?6, skill = ?7, loadout = ?8, enabled = ?9, updated = ?10 WHERE id = {}",
                g.id
            ),
            p,
        )?;
        Ok(g.id)
    }
}

pub fn remove(c: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(c.execute("DELETE FROM bot_groups WHERE id = ?1", [id])? > 0)
}

/// The loadout spec last given to `login`'s account ("" = none).
pub fn applied_loadout(c: &Connection, login: &str) -> rusqlite::Result<String> {
    Ok(
        c.query_row("SELECT spec FROM bot_loadouts WHERE login = ?1", [login], |r| r.get(0))
            .optional()?
            .unwrap_or_default(),
    )
}

pub fn set_applied_loadout(c: &Connection, login: &str, spec: &str, now: i64) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO bot_loadouts (login, spec, at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(login) DO UPDATE SET spec = excluded.spec, at = excluded.at",
        params![login, spec, now],
    )?;
    Ok(())
}

/// The skill text of a group: a level 0..100, a range `lo-hi`, or `off` (dc-botclient `--skill`).
fn clean_skill(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(BOT_SKILL_RANGE.to_string());
    }
    if s.eq_ignore_ascii_case("off") {
        return Ok("off".into());
    }
    let level = |t: &str| {
        let t = t.trim();
        (!t.is_empty() && t.len() <= 3 && t.bytes().all(|b| b.is_ascii_digit()))
            .then(|| t.parse::<u32>().ok())
            .flatten()
            .filter(|v| *v <= 100)
    };
    let bad = || format!("skill: {s:?} is not a level 0..100, a range like 30-65, or off");
    match s.split_once('-') {
        Some((lo, hi)) => match (level(lo), level(hi)) {
            (Some(lo), Some(hi)) if lo <= hi => Ok(format!("{lo}-{hi}")),
            _ => Err(bad()),
        },
        None => level(s).map(|v| v.to_string()).ok_or_else(bad),
    }
}

/// A loadout spec as `account loadout` takes it (`<slot>:<item>[+<option>],...`, bare item numbers, or `random`);
/// whether the items exist is known only when it is given to an account (the bot's status shows the answer).
fn clean_loadout(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("random") {
        return Ok("random".into());
    }
    if s.len() > MAX_LOADOUT
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b':' | b'+' | b',' | b' '))
    {
        return Err("loadout: random, or <slot>:<item>[+<option>],... (for example 0:27)".into());
    }
    Ok(s.replace(' ', ""))
}

fn clean_prefix(s: &str) -> Result<String, String> {
    let s = s.trim();
    let b = s.as_bytes();
    let ok = (2..=MAX_PREFIX).contains(&b.len())
        && b[0].is_ascii_alphabetic()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
        && !b[b.len() - 1].is_ascii_digit();
    if !ok {
        return Err(format!(
            "prefix: 2..{MAX_PREFIX} letters, digits or _, starting with a letter and not ending in a digit"
        ));
    }
    if s.to_ascii_lowercase()
        .starts_with(&dc_wire::loginmods::POOL_NICK_PREFIX.to_ascii_lowercase())
    {
        return Err(format!(
            "prefix: {} names are the bot pool's",
            dc_wire::loginmods::POOL_NICK_PREFIX
        ));
    }
    Ok(s.to_string())
}

/// The group a write's JSON describes (`id` 0 = a new one), checked against the server list.
pub fn parse(m: &Map<String, Value>, entries: &[Entry]) -> Result<Group, String> {
    let text = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let int = |k: &str| m.get(k).and_then(Value::as_i64);
    let prefix = clean_prefix(&text("prefix"))?;
    let count = int("count").unwrap_or(0);
    if !(1..=MAX_PER_GROUP as i64).contains(&count) {
        return Err(format!("count: 1..{MAX_PER_GROUP} bots (a room holds 8)"));
    }
    let server_id = int("server_id").unwrap_or(0);
    let Some(entry) = entries.iter().find(|e| i64::from(e.id) == server_id) else {
        let known = entries
            .iter()
            .map(|e| format!("{} {}", e.id, e.name))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "server_id: {server_id} is not an entry of the server list ({known})"
        ));
    };
    let mut channels = Vec::new();
    for v in m.get("channels").and_then(Value::as_array).into_iter().flatten() {
        let Some(ch) = v.as_u64().filter(|c| *c <= u64::from(u32::MAX)).map(|c| c as u32) else {
            return Err("channels: a list of channel numbers".into());
        };
        if !entry.owns(ch) {
            return Err(format!(
                "channels: {ch} is not a channel of {} (channels {}..{})",
                entry.name,
                entry.first,
                entry.first + entry.channels.max(1) - 1
            ));
        }
        if !channels.contains(&ch) {
            channels.push(ch);
        }
    }
    if channels.is_empty() {
        return Err("channels: pick at least one channel".into());
    }
    let id = int("id").unwrap_or(0).max(0);
    let enabled = m.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    Ok(Group {
        id,
        prefix,
        count: count as u32,
        server_id: entry.id,
        channels,
        room: RoomPick::from_parts(&text("room"), &text("room_arg"))?,
        skill: clean_skill(&text("skill"))?,
        loadout: clean_loadout(&text("loadout"))?,
        enabled,
    })
}

/// The checks across groups: one prefix per group, and the bots of all enabled groups within `max_total`.
pub fn check_against(g: &Group, all: &[Group], max_total: u32) -> Result<(), String> {
    let others = all.iter().filter(|o| o.id != g.id);
    if let Some(o) = others.clone().find(|o| o.prefix.eq_ignore_ascii_case(&g.prefix)) {
        return Err(format!("prefix: group {} already uses {}", o.id, o.prefix));
    }
    let total: u32 = others.filter(|o| o.enabled).map(|o| o.count).sum::<u32>() + if g.enabled { g.count } else { 0 };
    if total > max_total {
        return Err(format!(
            "that makes {total} bots running; this lobby runs at most {max_total} (bots.managed_max_total)"
        ));
    }
    Ok(())
}

/// The channels a stored group can use today (the server list may have changed since it was saved), or why none.
pub fn usable_channels(g: &Group, entries: &[Entry]) -> Result<Vec<u32>, String> {
    let Some(e) = entries.iter().find(|e| e.id == g.server_id) else {
        return Err(format!(
            "server entry {} is not in the server list any more",
            g.server_id
        ));
    };
    let ch: Vec<u32> = g.channels.iter().copied().filter(|c| e.owns(*c)).collect();
    if ch.is_empty() {
        return Err(format!(
            "none of channels {} belongs to {} any more",
            channels_text(&g.channels),
            e.name
        ));
    }
    Ok(ch)
}

// ------------------------------------------------------------------------------------------------- HTTP

type Reply = (u16, &'static str, String);

fn jerr(status: u16, msg: &str) -> Reply {
    (
        status,
        crate::opsweb::JSON,
        json!({"ok": false, "message": msg}).to_string(),
    )
}

fn jok(v: Value) -> Reply {
    (200, crate::opsweb::JSON, v.to_string())
}

/// True for the paths of the dashboard's Bots tab.
pub fn admin_handles(path: &str) -> bool {
    let p = path.trim_end_matches('/');
    p == "/ops/bots" || p.starts_with("/ops/bots/")
}

/// LG-390: a JSON body, the page's own `Origin`, the page's token in `X-Admin-CSRF`.
fn post_guard(head: &str, origin: &str, csrf: &str) -> Result<(), Reply> {
    let ct = header(head, "content-type").unwrap_or("").to_ascii_lowercase();
    if !ct.starts_with("application/json") {
        return Err(jerr(415, "send application/json"));
    }
    if !header(head, "origin").is_some_and(|o| !origin.is_empty() && o.eq_ignore_ascii_case(origin)) {
        return Err(jerr(403, "the request does not come from the dashboard (Origin)"));
    }
    let tok = header(head, "x-admin-csrf").unwrap_or("");
    if csrf.is_empty() || !crate::hosts::token_eq(tok, csrf) {
        return Err(jerr(403, "missing or wrong page token: reload the page"));
    }
    Ok(())
}

/// One request under `/ops/bots` (the caller checked loopback, Host and the admin token, or the admin web session).
pub fn admin(shared: &Shared, req: &Request, head: &str, body: &[u8], guard: Option<&WriteGuard>) -> Reply {
    let path = req.path.trim_end_matches('/');
    match (req.method.as_str(), path) {
        ("GET", "/ops/bots") => jok(status_json(shared)),
        ("POST", "/ops/bots/group" | "/ops/bots/delete" | "/ops/bots/start" | "/ops/bots/stop") => {
            let mut g = crate::net::lock(shared);
            let st = &mut *g;
            let host = header(head, "host").unwrap_or("");
            let (origin, csrf) = match guard {
                Some(w) => (w.origin.clone(), w.csrf.clone()),
                None if host.is_empty() => return jerr(403, "the request does not come from the dashboard (Origin)"),
                None => (format!("http://{host}"), st.beta.csrf(&mut st.keys)),
            };
            if let Err(r) = post_guard(head, &origin, &csrf) {
                return r;
            }
            let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(body) else {
                return jerr(400, "send a JSON object");
            };
            write(st, path, &m, crate::clock::unix_now())
        }
        (_, "/ops/bots" | "/ops/bots/group" | "/ops/bots/delete" | "/ops/bots/start" | "/ops/bots/stop") => {
            jerr(405, "method not allowed")
        }
        _ => jerr(404, "not found"),
    }
}

fn ops_event(st: &Core, detail: &str) {
    crate::opsmon::event(st, 0, "bots", detail);
    st.note(&format!("bots: {detail}"));
}

fn describe(g: &Group) -> String {
    let room = match &g.room {
        RoomPick::Any => "any room with a person".to_string(),
        RoomPick::Follow(n) => format!("following {n}"),
        RoomPick::Title(t) => format!("rooms titled *{t}*"),
    };
    format!(
        "{} x{} on entry {} channels {}, {room}, skill {}{}{}",
        g.prefix,
        g.count,
        g.server_id,
        channels_text(&g.channels),
        g.skill,
        if g.loadout.is_empty() {
            String::new()
        } else {
            format!(", loadout {}", g.loadout)
        },
        if g.enabled { ", running" } else { ", stopped" }
    )
}

/// The write of `path` with body `m`, under the lobby's lock.
pub fn write(st: &mut Core, path: &str, m: &Map<String, Value>, now: i64) -> Reply {
    let max_total = st.opts.bots_max_total;
    let managed = st.opts.bots_managed;
    let all = match list(&st.db) {
        Ok(v) => v,
        Err(e) => return jerr(500, &format!("db: {e}")),
    };
    let off = if managed {
        ""
    } else {
        " (bots.managed is off on this lobby: nothing runs until it is on)"
    };
    if path == "/ops/bots/group" {
        let entries = crate::servers::entries(st);
        let mut g = match parse(m, &entries) {
            Ok(g) => g,
            Err(e) => return jerr(400, &e),
        };
        if g.id != 0 {
            let Some(old) = all.iter().find(|o| o.id == g.id) else {
                return jerr(404, &format!("no bot group {}", g.id));
            };
            // an edit that does not say keeps the group running or stopped
            if !m.contains_key("enabled") {
                g.enabled = old.enabled;
            }
        }
        if let Err(e) = check_against(&g, &all, max_total) {
            return jerr(409, &e);
        }
        let new = g.id == 0;
        let id = match save(&st.db, &g, now) {
            Ok(id) => id,
            Err(e) => return jerr(500, &format!("db: {e}")),
        };
        let g = Group { id, ..g };
        ops_event(
            st,
            &format!("group {id} {}: {}", if new { "added" } else { "changed" }, describe(&g)),
        );
        return jok(json!({"ok": true, "message": format!("Group {} saved{off}.", g.prefix), "group": g.json()}));
    }
    let id = m.get("id").and_then(Value::as_i64).unwrap_or(0);
    let Some(g) = all.iter().find(|g| g.id == id).cloned() else {
        return jerr(404, &format!("no bot group {id}"));
    };
    match path {
        "/ops/bots/delete" => {
            if let Err(e) = remove(&st.db, id) {
                return jerr(500, &format!("db: {e}"));
            }
            ops_event(st, &format!("group {id} {} deleted ({} bots stop)", g.prefix, g.count));
            jok(json!({"ok": true, "message": format!("Group {} deleted.", g.prefix)}))
        }
        _ => {
            let on = path == "/ops/bots/start";
            let ng = Group {
                enabled: on,
                ..g.clone()
            };
            if on {
                if let Err(e) = check_against(&ng, &all, max_total) {
                    return jerr(409, &e);
                }
            }
            if let Err(e) = save(&st.db, &ng, now) {
                return jerr(500, &format!("db: {e}"));
            }
            ops_event(
                st,
                &format!("group {id} {} {}", g.prefix, if on { "started" } else { "stopped" }),
            );
            jok(
                json!({"ok": true, "message": format!("Group {} {}{off}.", g.prefix, if on { "started" } else { "stopped" }),
                       "group": ng.json()}),
            )
        }
    }
}

/// Where a bot is in the lobby now, from its account's session.
fn lobby_place(st: &Core, name: &str) -> Value {
    let login = name.to_ascii_lowercase();
    let acc: Option<i64> = st
        .db
        .query_row("SELECT user_no FROM accounts WHERE login = ?1", [&login], |r| r.get(0))
        .optional()
        .ok()
        .flatten();
    let Some(acc) = acc else {
        return json!({"online": false, "account": false});
    };
    let s = st
        .sessions
        .values()
        .find(|s| s.account == Some(acc as u32) && s.in_lobby && !s.closing);
    let Some(s) = s else {
        return json!({"online": false, "account": true});
    };
    let room = s.room.and_then(|k| st.rooms.get(&k)).map(|r| {
        let phase = match r.state {
            crate::room::RoomPhase::Waiting => "waiting",
            crate::room::RoomPhase::Starting => "countdown",
            crate::room::RoomPhase::Playing => "playing",
        };
        json!({"id": format!("{}-{}", r.key.channel, r.key.no), "title": r.title, "phase": phase,
               "in_match": r.game.is_some() && r.state == crate::room::RoomPhase::Playing})
    });
    json!({"online": true, "account": true, "channel": s.channel, "room": room})
}

/// LG-391: `GET /ops/bots`: the server list's entries, every group with its bots, each bot's process and place.
pub fn status_json(shared: &Shared) -> Value {
    let sup = crate::net::lock(shared).bot_sup.clone();
    let procs: BTreeMap<String, crate::botsup::ProcStatus> = match &sup {
        Some(s) => crate::botsup::lock(s).status(std::time::Instant::now()),
        None => BTreeMap::new(),
    };
    let g = crate::net::lock(shared);
    let entries = crate::servers::entries(&g);
    let groups = match list(&g.db) {
        Ok(v) => v,
        Err(e) => return json!({"ok": false, "message": format!("db: {e}")}),
    };
    let mut running = 0;
    let gj: Vec<Value> = groups
        .iter()
        .map(|grp| {
            let usable = usable_channels(grp, &entries);
            let bots: Vec<Value> = grp
                .names()
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let p = procs.get(name);
                    if p.is_some_and(|p| p.pid.is_some()) {
                        running += 1;
                    }
                    let ch = usable.as_ref().map_or(0, |c| Group::channel_of(c, i as u32 + 1));
                    json!({
                        "name": name, "channel": ch,
                        "state": p.map_or(if grp.enabled && g.opts.bots_managed { "waiting" } else { "stopped" }, |p| p.state),
                        "pid": p.and_then(|p| p.pid), "restarts": p.map_or(0, |p| p.restarts),
                        "uptime_secs": p.map_or(0, |p| p.uptime_secs), "retry_in_secs": p.map_or(0, |p| p.retry_in_secs),
                        "last_exit": p.map_or(String::new(), |p| p.last_exit.clone()),
                        "note": p.map_or(String::new(), |p| p.note.clone()),
                        "lobby": lobby_place(&g, name),
                    })
                })
                .collect();
            let mut v = grp.json();
            v["entry_name"] = json!(entries.iter().find(|e| e.id == grp.server_id).map(|e| e.name.clone()));
            v["problem"] = json!(usable.err());
            v["bots"] = json!(bots);
            v
        })
        .collect();
    // bots of a group no longer listed (deleted or shrunk) that are still being stopped
    let listed: std::collections::BTreeSet<String> = groups.iter().flat_map(Group::names).collect();
    let stopping: Vec<Value> = procs
        .iter()
        .filter(|(n, _)| !listed.contains(*n))
        .map(|(n, p)| json!({"name": n, "state": p.state, "pid": p.pid}))
        .collect();
    json!({
        "ok": true,
        "managed": g.opts.bots_managed,
        "max_total": g.opts.bots_max_total,
        "per_group": MAX_PER_GROUP,
        "wanted": groups.iter().filter(|x| x.enabled).map(|x| x.count).sum::<u32>(),
        "running": running,
        "supervisor": sup.is_some(),
        "entries": entries.iter().map(|e| json!({"id": e.id, "name": e.name, "channels": e.channel_ids().collect::<Vec<_>>()}))
            .collect::<Vec<_>>(),
        "groups": gj,
        "stopping": stopping,
    })
}

#[cfg(test)]
#[path = "botgroups_tests.rs"]
pub(crate) mod tests;
