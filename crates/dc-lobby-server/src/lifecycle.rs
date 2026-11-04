//! Server life: the stop request (Ctrl+C, console close, SIGTERM, `POST /stop`), the graceful stop itself, the
//! close of a room on the operator's or the empty-room rule's behalf, and the status document of the admin
//! endpoint.
//!
//! DC-DEFAULT: LG-130 (proposed) graceful stop: refuse new connections, end every running match through the
//! game server's `quit` line, wait up to `--stop-grace` seconds for the game servers, kill what remains, close the
//! client connections, checkpoint and close the DB, log "stopped", exit 0.
// SPEC: DSERVER_PIPE.md §3.4 (`quit`: the server ends the match, prints `@end` and exits)

use crate::game;
use crate::room::{RoomKey, RoomPhase};
use crate::state::{Core, Shared};
use dc_mod_lua::JsonValue;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);

/// Asks the main thread to run the graceful stop (async-signal safe: one atomic store).
pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
}

pub fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Installs the Ctrl+C / Ctrl+Break / console-close handler (Windows) or the SIGINT / SIGTERM handler (unix).
pub fn install_stop_handler() -> Result<(), String> {
    platform::install()
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::BOOL;
    use windows_sys::Win32::System::Console::{
        SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT,
        CTRL_SHUTDOWN_EVENT,
    };

    unsafe extern "system" fn handler(kind: u32) -> BOOL {
        match kind {
            CTRL_C_EVENT | CTRL_BREAK_EVENT => {
                super::request_stop();
                1
            }
            // the process ends when the handler returns: hold it until the stop has run (Windows allows a few
            // seconds for a closing console)
            CTRL_CLOSE_EVENT | CTRL_SHUTDOWN_EVENT => {
                super::request_stop();
                let t = std::time::Instant::now();
                while !super::DONE.load(std::sync::atomic::Ordering::SeqCst)
                    && t.elapsed() < std::time::Duration::from_secs(30)
                {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                1
            }
            // a log-off of some user is not a reason to stop a server (it may run as a service)
            _ => 0,
        }
    }

    pub fn install() -> Result<(), String> {
        // SAFETY: registers a plain function pointer; the handler only touches atomics and sleeps.
        let ok = unsafe { SetConsoleCtrlHandler(Some(handler), 1) };
        if ok == 0 {
            Err(format!(
                "SetConsoleCtrlHandler failed: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(unix)]
mod platform {
    extern "C" fn on_signal(_: libc::c_int) {
        super::request_stop();
    }

    pub fn install() -> Result<(), String> {
        let f = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        // SAFETY: the handler only performs one atomic store, which is async-signal safe.
        unsafe {
            libc::signal(libc::SIGINT, f);
            libc::signal(libc::SIGTERM, f);
        }
        Ok(())
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    pub fn install() -> Result<(), String> {
        Err("no stop handler on this platform".into())
    }
}

/// Blocks until a stop is requested.
pub fn wait_for_stop() {
    while !stop_requested() {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Ends the match of a room (the game server gets `quit`) and closes the room: every member goes back to the
/// lobby with the kicked notice, the room leaves the room list, the game port is free once the server exits.
pub fn close_room_now(st: &mut Core, key: RoomKey, why: &str) -> crate::state::HResult {
    let Some(r) = st.rooms.get(&key) else {
        return Ok(());
    };
    st.note(&format!(
        "room {}-{}: closing ({})",
        key.channel, key.no, why
    ));
    if r.state != RoomPhase::Waiting || r.game.is_some() {
        game::end_match(st, key, true);
    }
    let sids: Vec<_> = st
        .rooms
        .get(&key)
        .map(|r| r.members.iter().map(|m| m.sid).collect())
        .unwrap_or_default();
    for sid in sids {
        crate::room::remove_member(st, sid, crate::wire::REMOVE_KICKED, true)?;
    }
    // a room whose members were all gone already (or whose remove failed) still goes
    crate::room::close_room(st, key);
    Ok(())
}

/// The graceful stop (LG-130). Never returns: the process exits with status 0.
pub fn shutdown(shared: &Shared, grace_secs: f64) -> ! {
    // LG-388: the managed bots end first (they would only reconnect)
    crate::botsup::shutdown(shared);
    let grace = Duration::from_secs_f64(grace_secs.clamp(0.0, 3600.0));
    {
        let mut g = crate::net::lock(shared);
        g.stopping = true;
        g.note(&format!(
            "stopping: no new connections; ending {} running match(es), up to {:.0} s for {} game server(s)",
            g.rooms.values().filter(|r| r.game.is_some()).count(),
            grace.as_secs_f64(),
            g.games.len()
        ));
        let r = g.scope(|st| {
            let keys: Vec<RoomKey> = st
                .rooms
                .iter()
                .filter(|(_, r)| r.game.is_some() || r.state != RoomPhase::Waiting)
                .map(|(k, _)| *k)
                .collect();
            for k in keys {
                game::end_match(st, k, true);
            }
            // servers no room points at any more (match over, not reaped yet)
            let rest: Vec<u64> = st.games.keys().copied().collect();
            for id in rest {
                game::quit_game(st, id);
            }
            Ok(())
        });
        if let Err(e) = r {
            g.note(&format!("stopping: ending the matches failed: {}", e));
        }
        crate::net::run_deferred(shared, &mut g);
    }
    let until = Instant::now() + grace;
    loop {
        let left = crate::net::lock(shared).games.len();
        if left == 0 || Instant::now() >= until {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut g = crate::net::lock(shared);
    let ids: Vec<u64> = g.games.keys().copied().collect();
    for id in ids {
        let gm = g.games.get_mut(&id).expect("listed");
        match gm.link.child_mut() {
            Some(c) => {
                let pid = c.id();
                let _ = c.kill();
                let _ = c.wait();
                g.note(&format!(
                    "stopping: game {} (pid {}) did not exit in time, killed",
                    id, pid
                ));
            }
            None => g.note(&format!("stopping: game {} still exiting", id)),
        }
    }
    let r = g.scope(|st| {
        let sids: Vec<_> = st.sessions.keys().copied().collect();
        for sid in sids {
            st.close(sid);
        }
        Ok(())
    });
    if let Err(e) = r {
        g.note(&format!("stopping: closing the connections failed: {}", e));
    }
    // the writer threads send what is queued and shut the sockets
    drop(g);
    std::thread::sleep(Duration::from_millis(200));
    let mut g = crate::net::lock(shared);
    match g.db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    }) {
        Ok((busy, log, done)) => g.note(&format!(
            "stopping: DB checkpoint (busy {}, wal frames {}, copied {})",
            busy, log, done
        )),
        Err(e) => g.note(&format!("stopping: DB checkpoint failed: {}", e)),
    }
    if let Ok(mem) = rusqlite::Connection::open_in_memory() {
        let db = std::mem::replace(&mut g.db, mem);
        if let Err((_, e)) = db.close() {
            g.note(&format!("stopping: DB close failed: {}", e));
        }
    }
    g.note("stopped");
    DONE.store(true, Ordering::SeqCst);
    // exit while holding the lock: no other thread touches the state after "stopped"
    std::process::exit(0)
}

fn num(v: impl Into<f64>) -> JsonValue {
    JsonValue::Number(v.into())
}

fn text(v: impl Into<String>) -> JsonValue {
    JsonValue::String(v.into())
}

/// True when the account is a player, not a test bot: not named `Bot<digits>` (LG-9), not a
/// `--test-bot-account`, and never logged in by a dc-botclient that said so (LG-317).
pub fn is_human(st: &Core, account: u32) -> bool {
    let login = match crate::db::account_by_no(&st.db, account) {
        Ok(Some(a)) => a.login,
        _ => return true,
    };
    let declared = st
        .db
        .query_row(
            "SELECT self_bot FROM accounts WHERE user_no = ?1",
            [account as i64],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        != 0;
    !(declared
        || crate::grant::is_bot_name(&login)
        || st
            .opts
            .test_bot_accounts
            .iter()
            .any(|b| crate::auth::normalize_login(b) == login))
}

fn phase_name(p: RoomPhase) -> &'static str {
    match p {
        RoomPhase::Waiting => "waiting",
        RoomPhase::Starting => "countdown",
        RoomPhase::Playing => "playing",
    }
}

/// `GET /status`: one JSON document.
pub fn status_json(st: &Core) -> String {
    let mut o = BTreeMap::new();
    o.insert(
        "uptime_secs".to_string(),
        num(st.started.elapsed().as_secs_f64().floor()),
    );
    o.insert("stopping".to_string(), JsonValue::Bool(st.stopping));
    o.insert("lobby_logins".to_string(), num(st.lobby_logins as f64));
    o.insert("connections".to_string(), num(st.sessions.len() as f64));
    o.insert(
        "online".to_string(),
        num(st
            .sessions
            .values()
            .filter(|s| s.in_lobby && s.account.is_some())
            .count() as f64),
    );
    let (seats_used, seats) = crate::botpool::counts(st);
    o.insert("bot_seats".to_string(), num(seats as f64));
    o.insert("bot_seats_used".to_string(), num(seats_used as f64));
    let db = std::path::absolute(&st.opts.db).unwrap_or_else(|_| st.opts.db.clone());
    o.insert("db".to_string(), text(db.display().to_string()));
    // LG-300: the remote game hosts, only on a lobby that has some
    if !st.opts.host_agents.is_empty() {
        o.insert("hosts".to_string(), crate::hosts::status_json(st));
    }
    o.insert(
        "rom".to_string(),
        st.rom
            .dir
            .as_ref()
            .map(|d| text(d.display().to_string()))
            .unwrap_or(JsonValue::Null),
    );
    let mut addr = BTreeMap::new();
    addr.insert("bind".to_string(), text(st.opts.bind.clone()));
    addr.insert("lobby_ip".to_string(), text(st.opts.lobbyip.clone()));
    addr.insert("game_ip".to_string(), text(st.opts.gameip.clone()));
    addr.insert("auth_port".to_string(), num(st.opts.port));
    addr.insert("lobby_port".to_string(), num(st.opts.lobbyport));
    addr.insert("clan_port".to_string(), num(st.opts.clanport));
    addr.insert("public".to_string(), JsonValue::Bool(st.opts.public));
    o.insert("address".to_string(), JsonValue::Object(addr));
    let mut rooms = Vec::new();
    for (k, r) in &st.rooms {
        let mut ro = BTreeMap::new();
        ro.insert("id".to_string(), text(format!("{}-{}", k.channel, k.no)));
        ro.insert("title".to_string(), text(r.title.clone()));
        ro.insert("state".to_string(), text(phase_name(r.state)));
        ro.insert("mode".to_string(), text(st.rom.mode_key(r.mode)));
        ro.insert("mode_index".to_string(), num(r.mode));
        ro.insert("map_index".to_string(), num(r.map));
        let humans = r.members.iter().filter(|m| is_human(st, m.account)).count();
        ro.insert("players".to_string(), num(humans as f64));
        ro.insert("bots".to_string(), num((r.members.len() - humans) as f64));
        ro.insert("max_players".to_string(), num(r.max_players));
        let g = r.game.and_then(|id| st.games.get(&id));
        ro.insert(
            "game".to_string(),
            match g {
                Some(g) => {
                    let mut go = BTreeMap::new();
                    go.insert("id".to_string(), num(g.id as f64));
                    go.insert("port".to_string(), num(g.port));
                    go.insert("ready".to_string(), JsonValue::Bool(g.ready));
                    go.insert(
                        "pid".to_string(),
                        g.link.pid().map(num).unwrap_or(JsonValue::Null),
                    );
                    JsonValue::Object(go)
                }
                None => JsonValue::Null,
            },
        );
        rooms.push(JsonValue::Object(ro));
    }
    o.insert("rooms".to_string(), JsonValue::Array(rooms));
    o.insert("game_servers".to_string(), num(st.games.len() as f64));
    // MAP-35: the community maps of the loaded set, by key, with their table index and owning mod
    let maps = st.maps();
    let community: Vec<JsonValue> = maps
        .community_indices()
        .into_iter()
        .zip(st.community_maps.iter())
        .map(|((index, key), m)| {
            let mut mo = BTreeMap::new();
            mo.insert("key".to_string(), text(key.to_string()));
            mo.insert("index".to_string(), num(index));
            mo.insert("name".to_string(), text(m.record.name.clone()));
            mo.insert("mod".to_string(), text(m.record.mod_id.clone()));
            mo.insert(
                "modes".to_string(),
                JsonValue::Array(m.record.modes.iter().map(|s| text(s.clone())).collect()),
            );
            JsonValue::Object(mo)
        })
        .collect();
    o.insert("community_maps".to_string(), JsonValue::Array(community));
    JsonValue::Object(o).to_json_pretty()
}

/// `GET /net`: every identified player of every running game with the newest link window its game server reported
/// (`@net`, every 10 s) and the age of the player's lobby connection.
// DC-DEFAULT: LG-185 (proposed) the operator's network view
pub fn net_json(st: &Core) -> String {
    let r1 = |v: f64| num((v * 10.0).round() / 10.0);
    let mut players = Vec::new();
    let mut games = 0u32;
    for g in st.games.values().filter(|g| !g.over) {
        games += 1;
        for (client, &uid) in &g.clients {
            if g.left.contains(&uid) {
                continue;
            }
            let mut o = BTreeMap::new();
            o.insert("game".to_string(), num(g.id as f64));
            o.insert(
                "room".to_string(),
                text(format!("{}-{}", g.room.channel, g.room.no)),
            );
            o.insert("client".to_string(), text(client.clone()));
            o.insert("account".to_string(), num(uid));
            o.insert(
                "nick".to_string(),
                st.character(uid)
                    .map(|c| text(c.nick))
                    .unwrap_or(JsonValue::Null),
            );
            let tcp = st
                .sessions
                .values()
                .filter(|s| s.account == Some(uid))
                .max_by_key(|s| (s.in_lobby, s.since))
                .map(|s| num(s.since.elapsed().as_secs_f64().floor()));
            o.insert("tcp_age_secs".to_string(), tcp.unwrap_or(JsonValue::Null));
            match g.net.get(client) {
                Some((at, n)) => {
                    o.insert(
                        "report_age_secs".to_string(),
                        r1(at.elapsed().as_secs_f64()),
                    );
                    o.insert("window_secs".to_string(), r1(n.window_ms as f64 / 1000.0));
                    let mut rtt = BTreeMap::new();
                    rtt.insert("mean".to_string(), r1(n.rtt_mean));
                    rtt.insert("min".to_string(), r1(n.rtt_min));
                    rtt.insert("max".to_string(), r1(n.rtt_max));
                    rtt.insert("samples".to_string(), num(n.rtt_samples as f64));
                    o.insert("rtt_ms".to_string(), JsonValue::Object(rtt));
                    o.insert("input_jitter_ms".to_string(), r1(n.jitter_ms));
                    o.insert("input_rate".to_string(), r1(n.input_rate));
                    o.insert("longest_gap_ms".to_string(), num(n.longest_gap_ms as f64));
                    o.insert("resends".to_string(), num(n.resends as f64));
                    o.insert("flow_drops".to_string(), num(n.flow_drops as f64));
                    o.insert("datagrams_in".to_string(), num(n.datagrams_in as f64));
                    o.insert("datagrams_out".to_string(), num(n.datagrams_out as f64));
                }
                None => {
                    o.insert("report_age_secs".to_string(), JsonValue::Null);
                }
            }
            players.push(JsonValue::Object(o));
        }
    }
    let mut o = BTreeMap::new();
    o.insert("games".to_string(), num(games));
    o.insert("players".to_string(), JsonValue::Array(players));
    JsonValue::Object(o).to_json_pretty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lists_the_basics_on_an_empty_server() {
        let st = crate::state::testing::state();
        let s = status_json(&st);
        for k in [
            "\"uptime_secs\"",
            "\"rooms\"",
            "\"lobby_logins\"",
            "\"db\"",
            "\"address\"",
        ] {
            assert!(s.contains(k), "{k} missing in {s}");
        }
    }

    #[test]
    fn bot_names_and_test_bot_accounts_are_not_human() {
        let mut st = crate::state::testing::state();
        st.opts.test_bot_accounts = vec!["Helper".into()];
        let now = 0;
        let a = crate::db::create_account(&st.db, "alice", now).unwrap();
        let b = crate::db::create_account(&st.db, "bot3", now).unwrap();
        let h = crate::db::create_account(&st.db, &crate::auth::normalize_login("Helper"), now)
            .unwrap();
        assert!(is_human(&st, a));
        assert!(!is_human(&st, b));
        assert!(!is_human(&st, h));
    }
}
