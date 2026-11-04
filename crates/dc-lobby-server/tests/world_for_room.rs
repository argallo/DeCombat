//! The world-for-room suite against the real lobby binary: the `-world` a room's game server is launched with
//! comes from the room's map and mode through the ROM room table (the map's GMS for the mode names the world),
//! and `--dserver-world` overrides it. A stand-in server records its launch line and control lines; the real
//! `dc-dserver` then loads the chosen world. ROM-gated (CA_ROM, room table, world archive); values are compared
//! with what `dc-bute` computes from the same ROM, never with copied ROM data.
// HARNESS: lobby_harness::test_world_for_room (map Showdown + mode QRT -> the QRT warehouse world;
// map 0 + mode TDM -> a Snow world); SPEC: DSERVER_PIPE.md §2 (`-world` derived from the room map), §3.2 (room line);
// SPEC: LOBBY_HARNESS.md row 15 (world mapping through the ROM world archive)

mod support;
use std::time::Duration;
use support::*;

const T: Duration = Duration::from_secs(15);

/// lobby.json that opens the playable-mode gate (DC-DEFAULT MG-1 keeps QRT out by default).
const OPEN_MODES: &str = r#"{"modes.playable": ["*"]}"#;

fn rom_with_worlds() -> Option<Rom> {
    let rom = Rom::open()?;
    if dc_bute::world_names(&rom.dir).is_empty() {
        return None;
    }
    Some(rom)
}

/// The map the harness calls Showdown (by name or key, case-insensitive).
fn showdown(rom: &Rom) -> Option<u32> {
    rom.game_room
        .maps()
        .iter()
        .position(|m| {
            m.get("Name")
                .map(|s| s.to_lowercase() == "showdown")
                .unwrap_or(false)
                || m.get("Key")
                    .map(|s| s.to_lowercase() == "showdown")
                    .unwrap_or(false)
        })
        .map(|i| i as u32)
}

/// Starts the host's room alone and returns the stand-in's record of the NEW launch once its server got the room
/// line and the roster.
fn start_and_record(l: &Lobby, host: &mut Client) -> FakeLaunch {
    let known: Vec<u32> = l.fake().iter().map(|x| x.pid).collect();
    ready(host, 2);
    let (_ip, port) = game_address(host, T);
    assert_eq!(port, l.gameport as u32);
    let mut rec = None;
    assert!(
        wait_until(T, || {
            rec = l.fake().into_iter().find(|x| {
                !known.contains(&x.pid) && x.room_line().is_some() && !x.users().is_empty()
            });
            rec.is_some()
        }),
        "no launch recorded; log:
{}",
        l.log_text()
    );
    rec.unwrap()
}

/// The host leaves the match: the lobby quits the server and the room waits again.
fn end_match(l: &Lobby, host: &mut Client) {
    back_to_room(host);
    room_waiting(host, T);
    let n = l.game_pids().len();
    assert!(
        l.wait_log("server exited", T) || n == 0,
        "log:\n{}",
        l.log_text()
    );
}

#[test]
fn world_follows_the_room_map_and_mode() {
    let Some(rom) = rom_with_worlds() else {
        println!("skipped: CA_ROM unset or room table / world archive unavailable");
        return;
    };
    let (Some(qrt), Some(tdm), Some(sd)) =
        (rom.mode_index("QRT"), rom.mode_index("TDM"), showdown(&rom))
    else {
        panic!("the ROM room table has no QRT / TDM mode or no Showdown map");
    };
    let l = Lobby::start(
        "world-room",
        LobbyOpts {
            // the QRT room needs the playable-mode gate (MG-1) open
            lobby_json: Some(OPEN_MODES.into()),
            dserver_exe: Some(fake_dserver_exe()),
            ..Default::default()
        },
    );
    l.set_fake_mode("record");
    let (mut a, alice) = player(&l, "alice", "Alice");

    // map Showdown + mode QRT
    let no = open_room(&mut a, "World room", qrt);
    set_room(&mut a, FIELD_MAP, sd);
    let rec = start_and_record(&l, &mut a);
    let world = rec.arg("-world").expect("-world").to_string();
    assert_eq!(
        world,
        rom.world(sd, qrt),
        "the dc-bute world of (map, mode)"
    );
    assert_eq!(
        world.to_lowercase(),
        "worlds\\warehouse_qrt",
        "harness expectation"
    );
    assert_eq!(rec.arg("-port"), Some(l.gameport.to_string().as_str()));
    assert_eq!(rec.arg("-guid"), Some("ca"));
    assert_eq!(rec.arg("-idle"), Some("60"));
    // the room line precedes the roster; the roster has the host with team 0 (Alpha) and the nick
    assert!(rec.stdin[0].starts_with("room"), "{:?}", rec.stdin);
    let users = rec.users();
    assert_eq!(users.len(), 1, "{users:?}");
    assert_eq!(users[0][1], alice.to_string());
    assert_eq!(users[0][6], "Alice");
    end_match(&l, &mut a);

    // map 0 + mode TDM in the same room
    set_room(&mut a, FIELD_MODE, tdm);
    set_room(&mut a, FIELD_MAP, 0);
    let rec2 = start_and_record(&l, &mut a);
    assert_ne!(rec2.pid, rec.pid);
    let world2 = rec2.arg("-world").unwrap().to_string();
    assert_eq!(world2, rom.world(0, tdm));
    assert!(
        world2.to_lowercase().starts_with("worlds\\snow"),
        "{world2}"
    );
    end_match(&l, &mut a);
    let _ = no;
}

#[test]
fn the_room_line_matches_the_rom_settings() {
    let Some(rom) = rom_with_worlds() else {
        println!("skipped: CA_ROM unset or room table / world archive unavailable");
        return;
    };
    let tdm = rom.mode_index("TDM").expect("TDM mode");
    let l = Lobby::start(
        "world-line",
        LobbyOpts {
            dserver_exe: Some(fake_dserver_exe()),
            ..Default::default()
        },
    );
    l.set_fake_mode("record");
    let (mut a, _) = player(&l, "alice", "Alice");
    open_room(&mut a, "Line room", tdm);
    let map = rom.default_map(tdm);
    let rec = start_and_record(&l, &mut a);
    let line = rec.room_line().unwrap().to_string();
    // goal / ff of a new room are the lobby's defaults (CD-25 goal from the ROM room table, ff off); the line must
    // equal what dc-bute builds for those settings
    let goal = rom.game_room.default_goal(tdm as usize, false).max(0) as u32;
    assert_eq!(
        line,
        rom.room_line(map, tdm, goal, 0),
        "forwarded room line"
    );
    assert!(line.contains("key=TDM"), "{line}");
    // DC-DEFAULT: GMD-2 (clarification) the ROM's TDM mode file has no time limit, so the class fallback applies
    // (every room has a time limit; project decision)
    let time = line
        .split_whitespace()
        .find_map(|t| t.strip_prefix("time="))
        .and_then(|t| t.parse::<u32>().ok());
    assert_eq!(time, Some(600), "{line}");
    end_match(&l, &mut a);
}

#[test]
fn dserver_world_overrides_the_room_map() {
    let l = Lobby::start(
        "world-forced",
        LobbyOpts {
            dserver_exe: Some(fake_dserver_exe()),
            dserver_world: Some("Worlds\\X".into()),
            ..Default::default()
        },
    );
    l.set_fake_mode("record");
    let (mut a, _) = player(&l, "alice", "Alice");
    open_room(&mut a, "Forced room", 0);
    let rec = start_and_record(&l, &mut a);
    assert_eq!(rec.arg("-world"), Some("Worlds\\X"));
    end_match(&l, &mut a);
}

#[test]
fn the_real_dserver_loads_the_chosen_world() {
    let Some(rom) = rom_with_worlds() else {
        println!("skipped: CA_ROM unset or room table / world archive unavailable");
        return;
    };
    let (Some(qrt), Some(sd)) = (rom.mode_index("QRT"), showdown(&rom)) else {
        panic!("no QRT mode or Showdown map");
    };
    let l = Lobby::start(
        "world-real",
        LobbyOpts {
            // the QRT room needs the playable-mode gate (MG-1) open
            lobby_json: Some(OPEN_MODES.into()),
            dserver_exe: Some(dserver_exe()),
            ..Default::default()
        },
    );
    let (mut a, _) = player(&l, "alice", "Alice");
    open_room(&mut a, "Real world", qrt);
    set_room(&mut a, FIELD_MAP, sd);
    ready(&mut a, 2);
    // the address push only follows `@ready`, i.e. a world the server could load
    let (_ip, port) = game_address(&mut a, T);
    assert_eq!(port, l.gameport as u32);
    assert!(l.wait_log("@ready port=", T), "log:\n{}", l.log_text());
    let dlog = l.dserver_logs();
    assert!(dlog.contains("dc-dserver: world "), "dserver log:\n{dlog}");
    assert!(!dlog.contains("dc-dserver: error"), "dserver log:\n{dlog}");
    let expected = rom.world(sd, qrt);
    assert!(
        l.log_has(&format!("world {expected}")),
        "lobby log:\n{}",
        l.log_text()
    );
    end_match(&l, &mut a);
}
