//! A featured server entry over loopback (LG-313): the real lobby with a fixture pack on its third entry and the client's
//! link: a vanilla login, the pick of the featured entry downloads, verifies and loads the pack (no consent prompt, a
//! consent list is set), a move to a plain entry unloads it, a move back loads it from the cache. The entry document's
//! map records as the menu's rows on that entry (MAP-86, MAP-87) and the scripted player's entry pick (MAP-88). No
//! window, no ROM.

mod support;

use dc_client::menu::link::{Link, LinkCmd, LinkEvent, LinkOptions};
use dc_client::menu::pilot::{Pilot, PilotPlan, PilotRoom};
use dc_client::menu::{Go, Menu, MenuOptions};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use support::*;

fn pack(dir: &Path) -> dc_modkit::ModSetEntry {
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("screenshots")).unwrap();
    std::fs::write(
        src.join("screenshots/main.png"),
        b"\x89PNG\r\n\x1a\nfixture",
    )
    .unwrap();
    std::fs::write(
        src.join("manifest.json"),
        r#"{"id": "fixture_pack", "name": "Fixture Pack", "version": "1.0.0", "entry": "main.lua", "kind": "shared",
           "description": "A test fixture pack for the loopback flow.", "author": "tests",
           "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons"]}"#,
    )
    .unwrap();
    std::fs::write(
        src.join("main.lua"),
        "return function(mod) mod.content.weapons:patch(145, {AmmoDamage = 41}) end",
    )
    .unwrap();
    let (bytes, e) = dc_modkit::pack_mod_bytes(&src).unwrap();
    let mods = dir.join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(mods.join(format!("{}.zip", e.sha256)), &bytes).unwrap();
    e
}

/// Polls until `want` matches; panics on a refusal or on time.
fn until(link: &Link, what: &str, want: impl Fn(&LinkEvent) -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while Instant::now() < end {
        for e in link.poll() {
            if want(&e) {
                return;
            }
            if let LinkEvent::EntryRefused(_)
            | LinkEvent::MoveRefused
            | LinkEvent::ServerRefused(_)
            | LinkEvent::LoginRefused { .. } = e
            {
                panic!("{what}: {e:?}");
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn picking_a_featured_entry_loads_its_pack_and_a_plain_entry_unloads_it() {
    let _watchdog = Watchdog::arm("entry_mods_flow", Duration::from_secs(120));
    let dir = scratch("entry_mods");
    let e = pack(&dir);
    let (auth, lobby, clan) = loop {
        let (a, b, c) = (free_tcp_port(), free_tcp_port(), free_tcp_port());
        if a < 50000
            && a != b
            && b != c
            && a != c
            && std::net::TcpListener::bind(("127.0.0.1", a + 8080)).is_ok()
        {
            break (a, b, c);
        }
    };
    let cfg = dir.join("server.toml");
    let mods_dir = dir.join("mods").display().to_string().replace('\\', "/");
    std::fs::write(
        &cfg,
        format!(
            r#"[server]
auth_port = {auth}
lobby_port = {lobby}
clan_port = {clan}
http_port = {}
[paths]
mods_dir = "{mods_dir}"
[mods]
policy = "vanilla"
[[lobby.servers]]
name = "US Central"
capacity = 50
[[lobby.servers]]
name = "US West"
capacity = 50
[[lobby.servers]]
name = "Vanilla++"
capacity = 50
mods = [{{ id = "{}", version = "{}", sha256 = "{}" }}]
"#,
            auth + 8080,
            e.id,
            e.version,
            e.sha256
        ),
    )
    .unwrap();
    let log = dir.join("lobby.log");
    let mut cmd = Command::new(lobby_exe());
    cmd.arg("--config")
        .arg(&cfg)
        .arg("--db")
        .arg(dir.join("lobby.db"))
        .arg("--log")
        .arg(&log)
        .current_dir(&dir);
    let _lobby = Proc::spawn(cmd);
    let addr: SocketAddr = format!("127.0.0.1:{auth}").parse().unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    while std::net::TcpStream::connect(addr).is_err() {
        assert!(Instant::now() < end, "the lobby did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut o = LinkOptions::new("entry-mods");
    o.mods.data_dir = Some(dir.join("client"));
    o.mods.timeout = Duration::from_secs(5);
    let link = Link::start(o);
    // a consent list that does not name the pack: a curated set must still not pause
    link.send(LinkCmd::ModPrefs {
        trusted: Vec::new(),
        local_enabled: Vec::new(),
    });
    link.send(LinkCmd::Login {
        auth: addr,
        user: "LOCAL:entryflow".into(),
        password: "pw".into(),
        nick: "EntryFlow".into(),
    });
    until(&link, "the server list", |e| {
        matches!(e, LinkEvent::Servers(_))
    });
    assert_eq!(link.join_report().recipe, None, "the login is vanilla");
    let empty = dc_mod_lua::EMPTY_SET_HASH;
    assert_eq!(link.join_report().hash.as_deref(), Some(empty));

    link.send(LinkCmd::PickServer(3));
    until(&link, "the featured entry's channels", |e| {
        matches!(e, LinkEvent::Channels(_))
    });
    let r = link.join_report();
    assert!(r.curated);
    assert_ne!(r.hash.as_deref(), Some(empty));
    assert!(
        !r.content.is_empty(),
        "the pack's weapon patch reaches the match's table"
    );
    assert_eq!(
        r.recipe.as_ref().map(|c| c.policy),
        Some(dc_mod_lua::ModPolicy::Curated {
            client_visual_allowed: false
        })
    );
    assert_eq!(r.recipe.unwrap().mods[0].id, "fixture_pack");
    assert!(dir
        .join("client/mods/by-hash")
        .join(format!("{}.zip", e.sha256))
        .is_file());

    link.send(LinkCmd::MoveServer(1));
    until(&link, "the plain entry's channels", |e| {
        matches!(e, LinkEvent::Channels(_))
    });
    let r = link.join_report();
    assert!(
        !r.curated && r.content.is_empty() && r.recipe.is_none(),
        "{r:?}"
    );
    assert_eq!(r.hash.as_deref(), Some(empty));

    link.send(LinkCmd::MoveServer(3));
    until(&link, "the featured entry again", |e| {
        matches!(e, LinkEvent::Channels(_))
    });
    assert!(link.join_report().curated);
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("curated pack(s)"), "{text}");
    assert!(!text.contains("mismatch"), "{text}");
}

/// A pack with a world file, for the `@mod:` path (the lobby needs it on the entry; the fake web port serves it).
fn map_pack(dir: &Path) -> (dc_modkit::ModSetEntry, Vec<u8>) {
    let src = dir.join("map-src");
    std::fs::create_dir_all(src.join("screenshots")).unwrap();
    std::fs::create_dir_all(src.join("assets/maps")).unwrap();
    std::fs::write(
        src.join("screenshots/main.png"),
        b"\x89PNG\r\n\x1a\nfixture",
    )
    .unwrap();
    std::fs::write(src.join("assets/maps/fixture_map.dat"), WORLD_BYTES).unwrap();
    std::fs::write(
        src.join("manifest.json"),
        r#"{"id": "fixture_maps", "name": "Fixture Maps", "version": "1.0.0", "entry": "main.lua", "kind": "shared",
           "description": "A test fixture pack with a world file.", "author": "tests",
           "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "assets"]}"#,
    )
    .unwrap();
    std::fs::write(
        src.join("main.lua"),
        "return function(mod) mod.content.weapons:patch(145, {AmmoDamage = 41}) end",
    )
    .unwrap();
    let (bytes, e) = dc_modkit::pack_mod_bytes(&src).unwrap();
    let mods = dir.join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(mods.join(format!("{}.zip", e.sha256)), &bytes).unwrap();
    (e, bytes)
}

const WORLD_BYTES: &[u8] = b"fixture world bytes";
const WORLD: &str = "@mod:fixture_maps/assets/maps/fixture_map.dat";
const BASE: i32 = 19;

/// A web port on `port` answering GETs from `routes` in plain HTTP (the client's connector takes plain on loopback; a
/// TLS hello is closed at once): the entry documents the lobby slice publishes, independent of this tree's lobby.
fn web_front(port: u16, routes: BTreeMap<String, Vec<u8>>) {
    let l = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut first = [0u8; 1];
            if s.peek(&mut first).ok() != Some(1) || first[0] == 0x16 {
                continue;
            }
            let mut buf = Vec::new();
            let mut c = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut c) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&c[..n]),
                }
            }
            let head = String::from_utf8_lossy(&buf).into_owned();
            let path = head.split_whitespace().nth(1).unwrap_or("/");
            let (status, body) = match routes.get(path) {
                Some(b) => (200, b.clone()),
                None => (404, Vec::new()),
            };
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = s.write_all(&body);
        }
    });
}

/// Runs `pilot` on `m` until `done`; panics with the screens and the open box on time.
fn walk(m: &mut Menu, pilot: &mut Pilot, what: &str, done: impl Fn(&Menu, &Pilot) -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        m.update();
        for i in pilot.next(m) {
            m.input(i);
        }
        let _ = m.take_outcome();
        if done(m, pilot) {
            return;
        }
        assert!(
            Instant::now() < end,
            "timed out waiting for {what}: screens {:?}, box {:?}, warnings {:?}",
            m.trail,
            m.popup().map(|p| p.text.clone()),
            pilot.warnings
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn plan() -> PilotPlan {
    PilotPlan {
        password: "pw".into(),
        room: PilotRoom::Create {
            title: "entry".into(),
            mode: None,
            map: None,
            start_after: 1,
        },
        matches: 1,
        shots: None,
        timeout: Duration::from_secs(60),
    }
}

/// The server screen again, and the pilot's pick of `entry` from it up to the room list.
fn enter_from_the_lobby(m: &mut Menu, entry: u32) {
    m.open(Go::Servers);
    // the screen's list request (0x3b0) holds it for a moment
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(300) {
        m.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut pilot = Pilot::new(plan()).on_entry(entry);
    walk(m, &mut pilot, "the room list", |m, _| {
        m.screen_name() == "lobby"
    });
    assert_eq!(pilot.gave_up(), None);
}

/// MAP-86 / MAP-87 / MAP-88: the pilot picks the entry by id; the entry document's records become the Catalog's rows at
/// the document's base; a `@mod:` announce resolves through the entry set's unpacked folder; a failed move gets the
/// rows of the entry still played back; a plain entry clears them; a move back shows them again. No ROM; the documents
/// come from a fake web port, so this holds whatever this tree's lobby publishes.
#[test]
fn the_entry_documents_maps_are_the_catalogs_rows_on_that_entry_only() {
    let _watchdog = Watchdog::arm("entry_mods_flow maps", Duration::from_secs(180));
    let dir = scratch("entry_maps");
    let (e, zip) = map_pack(&dir);
    let (auth, lobby, clan, http) = loop {
        let (a, b, c, h) = (
            free_tcp_port(),
            free_tcp_port(),
            free_tcp_port(),
            free_tcp_port(),
        );
        if a < 50000
            && [b, c, h].iter().all(|p| *p != a && *p != a + 8080)
            && b != c
            && c != h
            && b != h
            && std::net::TcpListener::bind(("127.0.0.1", a + 8080)).is_ok()
        {
            break (a, b, c, h);
        }
    };
    let set = dc_client::mods::ServerModSet::from_fragment(&serde_json::json!({
        "policy": "modded", "client_visual_allowed": false,
        "mods": [{"id": e.id, "version": e.version.to_string(), "sha256": e.sha256, "kind": e.kind.as_str(),
                  "requires_client": e.requires_client, "capabilities": e.capabilities}],
    }))
    .unwrap();
    let fragment = set.to_fragment();
    let row = serde_json::json!({
        "id": "fixture_map", "name": "Fixture Map", "description": "On the entry.", "world": WORLD, "modes": ["TDM"],
        "goals": [["TDM", 50]], "round_seconds": null, "picture": null, "loading": null, "radar": null,
        "players": null, "mod_id": e.id, "patched_by": [],
    });
    let doc = |maps: serde_json::Value| {
        serde_json::json!({"curated": true, "mod_set": fragment, "mod_set_hash": set.hash(),
                           "community_map_base": BASE, "maps": maps})
        .to_string()
        .into_bytes()
    };
    let mut routes = BTreeMap::new();
    routes.insert(
        "/server_list.json".to_string(),
        br#"{"name": "t"}"#.to_vec(),
    );
    routes.insert(
        "/entry_mods/2.json".to_string(),
        doc(serde_json::json!([row])),
    );
    // the third entry's document has a row no client can read: a move there fails
    routes.insert(
        "/entry_mods/3.json".to_string(),
        doc(serde_json::json!([{"id": "Bad Key"}])),
    );
    routes.insert(format!("/mods/{}.zip", e.sha256), zip);
    web_front(auth + 8080, routes);

    let cfg = dir.join("server.toml");
    let mods_dir = dir.join("mods").display().to_string().replace('\\', "/");
    let line = format!(
        r#"mods = [{{ id = "{}", version = "{}", sha256 = "{}" }}]"#,
        e.id, e.version, e.sha256
    );
    std::fs::write(
        &cfg,
        format!(
            r#"[server]
auth_port = {auth}
lobby_port = {lobby}
clan_port = {clan}
http_port = {http}
[paths]
mods_dir = "{mods_dir}"
[mods]
policy = "vanilla"
[[lobby.servers]]
name = "US Central"
capacity = 50
[[lobby.servers]]
name = "Vanilla++"
capacity = 50
{line}
[[lobby.servers]]
name = "Broken"
capacity = 50
{line}
"#
        ),
    )
    .unwrap();
    let log = dir.join("lobby.log");
    let mut cmd = Command::new(lobby_exe());
    cmd.arg("--config")
        .arg(&cfg)
        .arg("--db")
        .arg(dir.join("lobby.db"))
        .arg("--log")
        .arg(&log)
        .current_dir(&dir);
    let _lobby = Proc::spawn(cmd);
    let addr: SocketAddr = format!("127.0.0.1:{auth}").parse().unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    while std::net::TcpStream::connect(addr).is_err() {
        assert!(Instant::now() < end, "the lobby did not start");
        std::thread::sleep(Duration::from_millis(50));
    }

    let mut o = LinkOptions::new("entry-maps");
    o.mods.data_dir = Some(dir.join("client"));
    o.mods.timeout = Duration::from_secs(5);
    o.auto_character = true;
    let mut m = Menu::new(MenuOptions {
        auth: addr,
        user: "entrymaps".into(),
        link: o,
        rom: None,
        font: Some((dc_client::testkit::synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.set_idle_rules(false);

    // the pilot enters the featured entry by its id (the screen's own highlight is the first row)
    let mut pilot = Pilot::new(plan()).on_entry(2);
    walk(
        &mut m,
        &mut pilot,
        "the featured entry's room list",
        |m, _| m.screen_name() == "lobby",
    );
    assert_eq!(pilot.gave_up(), None);
    let on_entry = |m: &Menu| {
        assert_eq!(
            m.cat.first_community_index(),
            BASE as usize,
            "the document's base"
        );
        assert_eq!(m.cat.community_maps().len(), 1);
        assert_eq!(
            m.cat.community_map(BASE).map(|r| r.id.as_str()),
            Some("fixture_map")
        );
        assert_eq!(m.cat.map_label(BASE), "Fixture Map");
        assert!(m.cat.community_map(0).is_none());
        let recipe = m
            .mod_recipe()
            .expect("the entry's recipe reaches the match");
        assert_eq!(
            recipe.policy,
            dc_mod_lua::ModPolicy::Curated {
                client_visual_allowed: false
            }
        );
        recipe
    };
    let recipe = on_entry(&m);

    // the announce of the entry's game server: through the entry set's unpacked folder, by the recipe as by the
    // installed layer (the same `mod_world_for` as the match's loader)
    let unpacked = dir.join("client/mods/unpacked").join(&e.sha256);
    let by_recipe = dc_client::modassets::ModAssets::from_recipe(&recipe)
        .resolve(WORLD)
        .unwrap();
    assert_eq!(by_recipe, unpacked.join("assets/maps/fixture_map.dat"));
    assert!(
        dc_client::modassets::active().is_some(),
        "the menu's view of the entry set"
    );
    let world = dc_client::net::mod_world_for(WORLD, Some(&recipe), None)
        .map_err(|e| e.to_string())
        .unwrap();
    assert_eq!(world.bytes, WORLD_BYTES);
    assert_eq!(world.name, WORLD);

    // a move to the entry with the bad document fails, and the entry still played has its rows back
    m.open(Go::Servers);
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(300) {
        m.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut pilot = Pilot::new(plan()).on_entry(3);
    walk(&mut m, &mut pilot, "the refused move", |_, p| {
        p.gave_up().is_some()
    });
    let why = pilot.gave_up().unwrap();
    assert!(
        why.contains("--pilot-entry 3") && why.contains("map record 0"),
        "{why}"
    );
    m.open(Go::Lobby);
    on_entry(&m);

    // a plain entry: the rows and the mod view are gone
    enter_from_the_lobby(&mut m, 1);
    assert!(m.cat.community_maps().is_empty());
    assert_eq!(
        m.cat.first_community_index(),
        0,
        "no base: this client's ROM rows (none)"
    );
    assert!(m.cat.community_map(BASE).is_none());
    assert!(m.mod_recipe().is_none());
    assert!(dc_client::modassets::active().is_none());

    // and back: the rows again, the pack from the cache
    enter_from_the_lobby(&mut m, 2);
    on_entry(&m);

    // an id the list lacks is the pilot's reason, not another entry
    m.open(Go::Servers);
    let mut pilot = Pilot::new(plan()).on_entry(9);
    walk(&mut m, &mut pilot, "the missing entry", |_, p| {
        p.gave_up().is_some()
    });
    assert!(pilot.gave_up().unwrap().contains("no such entry"));
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(!text.contains("mismatch"), "{text}");
}

/// MAP-88: `dc-mapkit play` looks for the flag in the usage text, spelled exactly so; the flag takes a number and
/// belongs to the menu's scripted player.
#[test]
fn the_usage_names_the_pilot_entry_flag() {
    let run = |args: &[&str]| {
        let o = Command::new(env!("CARGO_BIN_EXE_dc-client"))
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stderr).into_owned()
    };
    assert!(run(&["--help"]).contains("--pilot-entry"));
    assert!(run(&["--menu", "lobby", "--pilot-entry", "first"])
        .contains("--pilot-entry: a server entry id"));
    assert!(run(&["--pilot-entry", "1"]).contains("--pilot-entry needs --menu lobby"));
}
