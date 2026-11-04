//! A community map end to end over the 2009 protocol: the real `dc-lobby-server` binary with a `server.toml`
//! whose declared set holds a map mod of ours (MAP-35..MAP-39), the stand-in game server recording its launch.
//! No ROM: the map sits after dc_bute's synthetic 0x311 fallback rows (index 12), a room picks it, the start
//! passes `-worldfile <absolute mod path> -announce @mod:... -worldobjects world` and the record's round time,
//! and `GET /status` lists the map.
// DC-DEFAULT: MAP-35, MAP-36, MAP-37, MAP-39; SPEC: DSERVER_PIPE.md §2 (launch), §3.2 (room line)

mod support;
use std::path::Path;
use std::time::Duration;
use support::*;

const T: Duration = Duration::from_secs(15);

/// A map mod folder of ours: shared, one record (TDM / BOMB, TDM goal 80, 300 s), the placeholder world.
fn write_map_mod(root: &Path, id: &str) -> std::path::PathBuf {
    let m = root.join(id);
    std::fs::create_dir_all(m.join("assets").join("maps")).unwrap();
    std::fs::create_dir_all(m.join("screenshots")).unwrap();
    std::fs::write(
        m.join("manifest.json"),
        format!(
            r#"{{"id": "{id}", "name": "{id}", "version": "1.0.0", "entry": "main.lua", "kind": "shared",
       "description": "A test fixture map mod for the lobby's e2e tests.", "author": "tests",
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
        modes = {{ "TDM", "BOMB" }},
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

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

fn http_get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("admin port");
    s.set_read_timeout(Some(T)).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

#[test]
fn a_declared_map_mod_is_offered_picked_and_launched_with_the_world_flags() {
    // the fixture lives next to the lobby's folder so that the paths of this test are its own
    let fixture = scratch_dir("community-map-fixture");
    let mods = fixture.join("mods");
    let folder = write_map_mod(&mods, "valley");
    let (_, entry) = dc_modkit::pack_mod_bytes(&folder).expect("the fixture packs");
    let cfg = fixture.join("server.toml");
    std::fs::write(
        &cfg,
        format!(
            "[paths]\nmods_dir = \"{}\"\n[mods]\npolicy = \"modded\"\n[[mods.set]]\nid = \"valley\"\nversion = \"1.0.0\"\nsha256 = \"{}\"\n",
            toml_path(&mods),
            entry.sha256
        ),
    )
    .unwrap();
    let admin = free_tcp_port();
    let l = Lobby::start(
        "community-map",
        LobbyOpts {
            dserver_exe: Some(fake_dserver_exe()),
            extra: vec![
                "--config".into(),
                cfg.display().to_string(),
                "--admin".into(),
                admin.to_string(),
                "--dserver-args".into(),
                "-worldobjects off".into(),
            ],
            // no ROM whatever the shell says: the indices below are the synthetic rows'
            env: vec![("CA_ROM".to_string(), String::new())],
            ..Default::default()
        },
    );
    assert!(
        l.wait_log(
            "maps: 1 community map(s) after the 12 ROM row(s): 12=valley",
            T
        ),
        "log:\n{}",
        l.log_text()
    );
    assert!(
        l.log_has("community maps 1"),
        "the start-up line counts it:\n{}",
        l.log_text()
    );
    // GET /status lists the map by key with its index and owning mod
    let status = http_get(admin, "/status");
    assert!(
        status.contains(r#""community_maps""#)
            && status.contains(r#""key": "valley""#)
            && status.contains(r#""index": 12"#),
        "{status}"
    );
    // the published server list names the base index the client decodes community indices from (MAP-32 /
    // MAP-35: the no-ROM lobby's 12 round-trips to a client with any ROM)
    let list = http_get(l.web, "/server_list.json");
    assert!(
        list.contains(r#""community_map_base": 12"#),
        "server list:\n{list}"
    );
    // a room on mode 0 picks the map; the pick echoes the record's goal (the logins carry the set's hash, LG-271)
    let set_hash = dc_rom::sha256_hex(format!("valley@1.0.0:{}\n", entry.sha256).as_bytes());
    let suffix = dc_wire::loginmods::suffix(&set_hash, dc_wire::loginmods::NO_CONTENT);
    let (mut a, _) = player_with(&l, "alice", "Alice", &suffix);
    let (mut b, _) = player_with(&l, "bob", "Bob", &suffix);
    let no = open_room(&mut a, "Valley room", 0);
    set_room(&mut a, FIELD_MAP, 12);
    join_room(&mut b, no);
    ready(&mut b, 1);
    ready(&mut a, 2);
    let (ip, _) = game_address(&mut a, T);
    assert_eq!(ip, "127.0.0.1");
    let f = l.fake();
    assert_eq!(f.len(), 1, "{f:?}");
    let launch = &f[0];
    let world = std::path::absolute(folder.join("assets").join("maps").join("valley.dat")).unwrap();
    assert_eq!(
        launch.arg("-worldfile"),
        Some(world.to_string_lossy().as_ref()),
        "{:?}",
        launch.args
    );
    assert_eq!(
        launch.arg("-announce"),
        Some("@mod:valley/assets/maps/valley.dat"),
        "{:?}",
        launch.args
    );
    let objects: Vec<&str> = launch
        .args
        .iter()
        .enumerate()
        .filter(|(_, a)| a.as_str() == "-worldobjects")
        .filter_map(|(i, _)| launch.args.get(i + 1).map(String::as_str))
        .collect();
    assert_eq!(
        objects,
        vec!["off", "world"],
        "after --dserver-args: {:?}",
        launch.args
    );
    // the record's round time rides the line; without a ROM there are no mode rows to name the room's mode, so
    // the record's TDM goal cannot apply and the goal stays the default (the in-process test covers the goal)
    let line = launch.room_line().expect("the room line").to_string();
    assert!(
        line.contains(" time=300") && line.contains("goal=100"),
        "{line}"
    );
    assert!(
        l.log_has("(community map valley, announced as @mod:valley/assets/maps/valley.dat)"),
        "{}",
        l.log_text()
    );
    drop(l);
    let _ = std::fs::remove_dir_all(&fixture);
}
