//! Community maps in the lobby (MAPKIT_PLAN.md 6.4; MAP-35..MAP-39): a fixture map mod of ours (no ROM) fills
//! the map table after the ROM rows, 0x311 carries its index with the bits of its modes, a room picks it and
//! starts a game server with the world flags, a remote host refuses it, and a record that cannot be served
//! stops a declared lobby. In-process flows over fake connections (the flow_tests pattern).

use crate::flow_tests::{enter, open_room, player, u32s};
use crate::modhost::ModClient;
use crate::msg::*;
use crate::room::{RoomKey, RoomPhase};
use crate::state::testing::*;
use crate::state::Core;
use dc_bute::{ButeDoc, GameRoomData};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Synthetic room table of ours: one ROM map; plain mode rows of classes 1 (TDM), 2 (BOMB) and 6 (FFA).
pub(crate) const TABLE: &str = r"[Map0]
Enable=TRUE
GMS_AA=a
GMS_BB=b
GMS_CC=c
[Mode0]
Key=AA
GameType=1
DefaultGoal=100
[Mode1]
Key=BB
GameType=2
DefaultGoal=5
[Mode2]
Key=CC
GameType=6
DefaultGoal=30
";

fn with_table() -> Core {
    let mut st = state();
    Arc::get_mut(&mut st.rom).unwrap().game_room = GameRoomData::from_doc(ButeDoc::parse(TABLE));
    st
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc-lobby-maps-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A map mod folder of ours: `kind`, one record (`id` = the mod id) with `modes`, a goal for TDM, a round time,
/// the placeholder world under `assets/maps/` and a screenshot.
pub(crate) fn write_map_mod(root: &Path, id: &str, kind: &str, modes: &str) -> PathBuf {
    let m = root.join(id);
    std::fs::create_dir_all(m.join("assets").join("maps")).unwrap();
    std::fs::create_dir_all(m.join("screenshots")).unwrap();
    std::fs::write(
        m.join("manifest.json"),
        format!(
            r#"{{"id": "{id}", "name": "{id}", "version": "1.0.0", "entry": "main.lua", "kind": "{kind}",
       "description": "A test fixture map mod for the lobby's unit tests.", "author": "tests",
       "screenshots": ["screenshots/main.png"], "capabilities": ["content.maps", "assets"]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        m.join("main.lua"),
        format!(
            r#"return function(mod)
    mod.content.maps:register({{
        id = "{id}",
        name = "fixture {id}",
        world = mod.assets:path("assets/maps/{id}.dat"),
        modes = {{ {modes} }},
        goals = {{ TDM = 80 }},
        round_seconds = 300,
    }})
end"#
        ),
    )
    .unwrap();
    std::fs::write(
        m.join("assets").join("maps").join(format!("{id}.dat")),
        dc_modkit::placeholder_world(),
    )
    .unwrap();
    std::fs::write(
        m.join("screenshots").join("main.png"),
        dc_modkit::placeholder_png(),
    )
    .unwrap();
    m
}

/// The (map, mask) pairs of a 0x311 payload.
pub(crate) fn pairs(d: &[u8]) -> Vec<(u32, u32)> {
    let n = u32_at(d, 0x16) as usize;
    (0..n)
        .map(|i| (u32_at(d, 0x1a + 8 * i), u32_at(d, 0x1e + 8 * i)))
        .collect()
}

/// A lobby over the synthetic table with the fixture mod loaded (folder discovery) and its maps installed.
fn lobby_with_map(tag: &str) -> (Core, PathBuf, Vec<String>) {
    let dir = scratch(tag);
    let mods = dir.join("mods");
    write_map_mod(&mods, "valley", "shared", r#""TDM", "BOMB""#);
    let c =
        ModClient::start(&mods, Arc::new(crate::logs::Logger::none()), None).expect("mod thread");
    assert_eq!(c.loaded, vec!["valley".to_string()]);
    assert_eq!(c.maps.len(), 1, "{:?}", c.maps);
    let mut st = with_table();
    let maps = c.maps.clone();
    st.mods = Some(c);
    let notes = st.install_community_maps(maps);
    (st, dir, notes)
}

/// MAP-35 / MAP-39: the record arrives as plain data with an absolute world path; the table appends it after the
/// ROM row; 0x311 carries its index with the TDM and BOMB bits; the status document lists it by key.
#[test]
fn a_shared_map_mod_joins_the_table_after_the_rom_rows_with_the_bits_of_its_modes() {
    let (st, dir, notes) = lobby_with_map("table");
    let m = &st.community_maps[0];
    assert_eq!(m.key(), "valley");
    assert_eq!(m.record.world, "@mod:valley/assets/maps/valley.dat");
    assert!(m.world_path.is_absolute(), "{}", m.world_path.display());
    assert!(
        m.world_path
            .ends_with(Path::new("assets").join("maps").join("valley.dat"))
            && m.world_path.is_file(),
        "{}",
        m.world_path.display()
    );
    assert_eq!(st.maps().first_community_index(), 1);
    assert_eq!(st.maps().map_count(), 2);
    assert!(
        notes
            .iter()
            .any(|n| n.contains("1 community map(s) after the 1 ROM row(s): 1=valley")),
        "{notes:?}"
    );
    let reply = crate::lobby::room_settings_reply(&st, None);
    let (frames, _) = dc_wire::parse_tcp_frames(&reply).unwrap();
    let p = &frames[0].1;
    assert_eq!(p.len(), 0x10a);
    assert_eq!(
        pairs(p),
        vec![(0, 0b111), (1, 0b011)],
        "ROM map, then the community map with TDM | BOMB"
    );
    let status = crate::lifecycle::status_json(&st);
    assert!(
        status.contains(r#""community_maps""#)
            && status.contains(r#""key": "valley""#)
            && status.contains(r#""mod": "valley""#),
        "{status}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// MAP-36 / MAP-37: a room created on TDM picks the community map (its record goal becomes the room's goal),
/// a mode change to BOMB keeps it (goal = the ROM mode default, no record goal), a change to FFA drops it (the
/// record lists no FFA), and Start launches a game server whose argv carries `-worldfile <absolute>`,
/// `-announce @mod:...` and `-worldobjects world` after `--dserver-args`, with the record's time on the room line.
#[test]
fn a_tdm_room_picks_the_community_map_and_starts_a_server_with_the_world_flags() {
    let (mut st, dir, _) = lobby_with_map("start");
    let logfile = dir.join("lobby.log");
    st.log = Arc::new(crate::logs::Logger::open(Some(&logfile), None).0);
    let (mut a, _) = player(&mut st, 1, 2, "mapa", "MapHost");
    let (mut b, _) = player(&mut st, 3, 4, "mapb", "MapGuest");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    assert_eq!(
        (st.rooms[&key].mode, st.rooms[&key].map, st.rooms[&key].goal),
        (0, 0, 100)
    );
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    b.got.clear();
    // the host picks the community map (index 1): accepted, the record's TDM goal is echoed to the room as its goal
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MAP, 1]));
    assert_eq!(
        u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6),
        1,
        "the pick is accepted"
    );
    assert_eq!((st.rooms[&key].map, st.rooms[&key].goal), (1, 80));
    let ups: Vec<(u32, u32)> = b
        .all(ROOM_UPDATE_PUSH)
        .iter()
        .map(|d| (u32_at(d, 0xa), u32_at(d, 0xe)))
        .collect();
    assert!(
        ups.contains(&(field::MAP, 1)) && ups.contains(&(field::GOAL, 80)),
        "{ups:?}"
    );
    // an index past the table is refused
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MAP, 2]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 0);
    // BOMB: the map plays it, so it stays; the goal is the ROM row's default (no record goal for BOMB)
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MODE, 1]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    assert_eq!((st.rooms[&key].map, st.rooms[&key].goal), (1, 5));
    // FFA: not in the record, the room falls back to the mode's default ROM map
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MODE, 2]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    assert_eq!((st.rooms[&key].map, st.rooms[&key].goal), (0, 30));
    // back on TDM with the community map, Start
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MODE, 0]));
    a.take(ROOM_SETTING_CHANGE_REPLY);
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MAP, 1]));
    a.take(ROOM_SETTING_CHANGE_REPLY);
    a.got.clear();
    if !cfg!(unix) {
        // the stand-in below is a shell script; on Windows the launch is covered by tests/community_map.rs
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    // a stand-in server: records its arguments and its first control line, then exits
    let args_file = dir.join("args.txt");
    let line_file = dir.join("line.txt");
    let script = dir.join("capture.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nread l\nprintf '%s\\n' \"$l\" > '{}'\nexit 0\n",
            args_file.display(),
            line_file.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    st.opts.dserver = true;
    st.opts.dserver_exe = script;
    st.opts.dserver_world = "ignored-for-community-maps".into();
    st.opts.dserver_args = "-worldobjects off".into();
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(
        u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6),
        1,
        "start accepted"
    );
    assert_eq!(st.rooms[&key].state, RoomPhase::Starting);
    let game = st.rooms[&key].game.expect("a game");
    // the stand-in exits at once; its records are complete when it is gone
    if let Some(c) = st.games.get_mut(&game).and_then(|g| g.link.child_mut()) {
        let _ = c.wait();
    }
    let args: Vec<String> = std::fs::read_to_string(&args_file)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    let after = |flag: &str| -> Vec<&str> {
        args.iter()
            .enumerate()
            .filter(|(_, a)| a.as_str() == flag)
            .filter_map(|(i, _)| args.get(i + 1).map(String::as_str))
            .collect()
    };
    let world = st.community_maps[0]
        .world_path
        .to_string_lossy()
        .into_owned();
    assert_eq!(after("-worldfile"), vec![world.as_str()], "{args:?}");
    assert_eq!(after("-world"), vec![world.as_str()], "{args:?}");
    assert_eq!(
        after("-announce"),
        vec!["@mod:valley/assets/maps/valley.dat"],
        "{args:?}"
    );
    assert_eq!(
        after("-worldobjects"),
        vec!["off", "world"],
        "the launch flag comes after --dserver-args: {args:?}"
    );
    let line = std::fs::read_to_string(&line_file).unwrap();
    assert!(
        line.starts_with("room ") && line.contains(" time=300") && line.contains("goal=80"),
        "{line}"
    );
    let log = std::fs::read_to_string(&logfile).unwrap_or_default();
    assert!(
        log.contains("(community map valley, announced as @mod:valley/assets/maps/valley.dat)"),
        "{log}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// MAP-36 / D12: a channel whose matches run on a remote game host does not start a community map room; the
/// room stays waiting and the log names the reason.
#[test]
fn a_remote_host_channel_refuses_a_community_map_room_and_keeps_it_waiting() {
    let (mut st, dir, _) = lobby_with_map("remote");
    let logfile = dir.join("lobby.log");
    st.log = Arc::new(crate::logs::Logger::open(Some(&logfile), None).0);
    st.opts.dserver = true;
    st.opts.dserver_world = "world".into();
    st.opts.servers = vec![dc_config::LobbyServerCfg {
        name: "West".into(),
        capacity: 500,
        host: "us-west".into(),
        channels: 3,
        lobby: String::new(),
        fingerprint: String::new(),
        policy: String::new(),
        region: String::new(),
        features: Vec::new(),
        mods: Vec::new(),
        client_visual_allowed: false,
        client_fov: None,
    }];
    let (mut a, _) = player(&mut st, 1, 2, "mapr", "Remote");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MAP, 1]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    let r = a.take(ROOM_USER_FLAG_REPLY);
    assert_eq!(u32_at(&r, 6), 0, "start refused");
    assert_eq!(st.rooms[&key].state, RoomPhase::Waiting);
    assert!(st.rooms[&key].game.is_none());
    let log = std::fs::read_to_string(&logfile).unwrap_or_default();
    assert!(
        log.contains("game server start failed: community map \"valley\" (mod 'valley') cannot start on remote game host us-west"),
        "{log}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// MAP-39: a declared set whose map record cannot be served does not start the lobby: a map in a mod clients do
/// not receive (`server_rules`), or a record with a mode that is no mode name (refused by the host at
/// `:register`, so the mod itself does not load). Folder discovery (no server.toml) drops such maps instead.
#[test]
fn a_map_the_clients_cannot_get_or_with_a_bad_mode_stops_a_declared_lobby() {
    let dir = scratch("refuse");
    let rules = write_map_mod(&dir.join("a"), "rules_map", "server_rules", r#""TDM""#);
    let bad = write_map_mod(&dir.join("b"), "bad_mode", "shared", r#""TDM", "ZOMBIE""#);
    let log = Arc::new(crate::logs::Logger::none());
    let declared = |id: &str, d: &Path| crate::modset::Resolved {
        id: id.into(),
        dir: d.to_path_buf(),
    };
    let e = ModClient::start_declared(vec![declared("rules_map", &rules)], Arc::clone(&log), None)
        .err()
        .expect("a server_rules map mod is refused");
    assert!(
        e.contains("mod maps:") && e.contains("map \"rules_map\"") && e.contains("must be shared"),
        "{e}"
    );
    let e = ModClient::start_declared(vec![declared("bad_mode", &bad)], Arc::clone(&log), None)
        .err()
        .expect("a record with a bad mode refuses the mod");
    assert!(e.contains("did not load") && e.contains("ZOMBIE"), "{e}");
    // discovery: the rules map is dropped, the lobby runs without it
    let c = ModClient::start(&dir.join("a"), Arc::clone(&log), None).unwrap();
    assert_eq!(c.loaded, vec!["rules_map".to_string()]);
    assert!(c.maps.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// MAP-38: the 0x311 version is derived from the mod-set hash on a modded lobby and stays 1 on a vanilla one.
#[test]
fn the_room_settings_version_follows_the_mod_set_hash() {
    let mut st = state();
    assert_eq!(
        st.room_settings_version(),
        crate::lobby::ROOM_SETTINGS_VERSION
    );
    st.login_mods.set = "0a1b2c3d".repeat(8);
    assert_eq!(st.room_settings_version(), 0x0a1b_2c3d);
    let reply = crate::lobby::room_settings_reply(&st, None);
    let (frames, _) = dc_wire::parse_tcp_frames(&reply).unwrap();
    assert_eq!(u32_at(&frames[0].1, 0x12), 0x0a1b_2c3d);
    st.login_mods.set = dc_wire::loginmods::EMPTY_SET_HASH.to_string();
    assert_eq!(st.room_settings_version(), 1);
}

/// Without a ROM the community rows come after dc_bute's synthetic 0x311 fallback (indices 0..12) and a room
/// may still pick one (no mode rows, so no bits: the pick is the host's, the start works).
#[test]
fn without_a_rom_the_community_map_sits_after_the_fallback_rows() {
    let dir = scratch("norom");
    let mods = dir.join("mods");
    write_map_mod(&mods, "valley", "shared", r#""TDM""#);
    let c = ModClient::start(&mods, Arc::new(crate::logs::Logger::none()), None).unwrap();
    let mut st = state();
    let maps = c.maps.clone();
    st.mods = Some(c);
    st.install_community_maps(maps);
    assert_eq!(st.maps().first_community_index(), 12);
    assert_eq!(st.maps().map_count(), 13);
    let (mut a, _) = player(&mut st, 1, 2, "mapn", "NoRom");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MAP, 12]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    assert_eq!(st.rooms[&key].map, 12);
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MAP, 13]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// C8 / MAP-35, MAP-36 (D12): a room on a clan channel picks from the ROM rows only (a community index is
/// refused by `on_setting`), and a clan room that sits on a community index anyway does not start: the launch
/// is refused with a log reason and the room stays waiting.
#[test]
fn a_clan_room_neither_picks_nor_starts_a_community_map() {
    let (mut st, dir, _) = lobby_with_map("clan");
    st.opts.dserver = true;
    let (mut a, _) = player(&mut st, 1, 2, "mapc", "Clan");
    let no = open_room(&mut st, &mut a, "");
    // the room moves to a clan channel (the clan dispatch leaves 0x38c to `room::on_setting`)
    let old = RoomKey { channel: 1, no };
    let key = RoomKey {
        channel: crate::clan::CLAN_CHANNEL_BASE + 1,
        no,
    };
    let mut room = st.rooms.remove(&old).unwrap();
    room.key = key;
    st.rooms.insert(key, room);
    let s = st.sessions.get_mut(&a.sid).unwrap();
    s.room = Some(key);
    s.channel = Some(key.channel);
    // the community index (1) is past the clan room's bound (the ROM's one row); the ROM row is fine
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MAP, 1]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 0);
    assert_eq!(st.rooms[&key].map, 0);
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MAP, 0]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    // a plain room on the same lobby still picks it
    let (mut b, _) = player(&mut st, 3, 4, "mapp", "Plain");
    let plain = RoomKey {
        channel: 1,
        no: open_room(&mut st, &mut b, ""),
    };
    b.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, field::MAP, 1]));
    assert_eq!(u32_at(&b.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    assert_eq!(st.rooms[&plain].map, 1);
    // a clan room on the community index anyway: the start is refused before anything is launched
    st.rooms.get_mut(&key).unwrap().map = 1;
    let e = crate::game::spawn(&mut st, key).unwrap_err();
    assert!(
        e.contains("community map \"valley\" (mod 'valley') cannot start in a clan room"),
        "{e}"
    );
    assert_eq!(st.rooms[&key].state, RoomPhase::Waiting);
    assert!(st.rooms[&key].game.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
