//! Vanilla++ end to end (MAPKIT_PLAN.md D9 / P5, the 2026-11-01 decision that community maps ride the featured entry;
//! DC_DEFAULTS.md MAP-72): the REAL `dc-lobby-server` binary on loopback, the real `dc-dserver` next to it and this
//! crate's menu + headless client in process, driven by the menu pilot, with `examples/mods/starter_map` packed here
//! with `dc_modkit::pack_mod_bytes` into a temporary mods folder (as an operator drops the zip into `mods/`).
//!
//! Two shapes, each with a ROM-free and a ROM-gated test. The main case is the ENTRY path, what `dc-mapkit play` runs
//! by default: a VANILLA lobby with one `[[lobby.servers]]` entry "Vanilla++" whose curated set is the map; the entry
//! document lists the map, the pilot picks the entry (the set loads at the pick, CLI-1643) and creates the room. The
//! MODDED path (`dc-mapkit play --modded`): a modded lobby with the map as its lobby-wide set; the login carries the
//! LG-271 suffix and the link reads the set from the web port.
//!
//! ROM-free: as far as the lobby start, the listing, the login with the set and the client's catalog holding the map
//! record (without a ROM the lobby has no mode rows, so the map has no 0x311 bits, MAP-35, and the room cannot be
//! created on it). ROM-gated (the menu pilot clicks the ROM layout, the room needs the ROM's mode rows): plays the match
//! and asserts the client loaded the `@mod:` world (no 0x29, `textures missing 0`, frames rendered, a spawn used), the
//! lobby's launch line and a clean end (`@leave`, `@end`). `dc-mapkit play --headless` drives the same binaries the
//! same way (MAP-69, MAP-89).
// SPEC: DSERVER_PIPE.md §2 (launch line: -worldfile, -announce), §3.2 (room line)

mod support;

use dc_botclient::run::RoomChoice;
use dc_client::hud::HudChoice;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::pilot::{
    headless_runner, run as run_pilot, Pilot, PilotPlan, PilotRoom, RoomNames,
};
use dc_client::menu::{Menu, MenuOptions};
use dc_client::testkit::synth_font;
use dc_client::{Connect, HeadlessOptions, NetOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use support::*;

const LOGIN: &str = "vplus";
const MAP_KEY: &str = "starter_map";
const WORLD: &str = "@mod:starter_map/assets/maps/starter_map.dat";
/// The first `[[lobby.servers]]` entry of a lobby is entry 1 (LG-304); the entry's name is `[server] name`.
const ENTRY_ID: u32 = 1;
const ENTRY_NAME: &str = "Vanilla++";
/// The community base of a lobby without a ROM: after dc_bute's synthetic 0x311 rows 0..12 (MAP-35).
const ROMLESS_BASE: i32 = 12;
/// Starts of a lobby before a lost port ends the test.
const PORT_TRIES: u32 = 5;

/// Where the map's set is declared: the shape of the lobby under test.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// `dc-mapkit play`: a vanilla lobby, one featured entry with the map as its curated set (MAP-89).
    Entry,
    /// `dc-mapkit play --modded`: a modded lobby with the map as its lobby-wide set (MAP-68).
    Modded,
}

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/starter_map")
}

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

fn tcp_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn udp_free(port: u16) -> bool {
    UdpSocket::bind(("127.0.0.1", port)).is_ok()
}

/// An auth port whose derived lobby (+2), clan (+3) and web (+8080: the client derives the mod download port
/// from the auth port) ports are free too.
fn auth_port() -> u16 {
    loop {
        let a = free_tcp_port();
        if a < 50_000 && tcp_free(a + 2) && tcp_free(a + 3) && tcp_free(a + 8080) {
            return a;
        }
    }
}

fn game_port() -> u16 {
    loop {
        let g = free_udp_port();
        if g < 60_000 && (g + 1..=g + 8).all(udp_free) {
            return g;
        }
    }
}

fn http(port: u16, method: &str, path: &str) -> String {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_secs(5)) else {
        return String::new();
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

/// The lobby instance of one test: the real binary, its folder, its log; stopped when dropped.
struct Lobby {
    child: Option<Child>,
    auth: u16,
    admin: u16,
    dir: PathBuf,
    log: PathBuf,
    sha256: String,
}

impl Lobby {
    /// Packs the example into `<dir>/mods/<sha>.zip`, writes `server.toml` (the [`Shape`], loopback, free ports; `[paths]
    /// rom` when a ROM is given) and starts `dc-lobby-server --config` with the real `dc-dserver`.
    fn start(dir: &Path, rom: Option<&Path>, shape: Shape) -> Lobby {
        let mods = dir.join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let (zip, entry) = dc_modkit::pack_mod_bytes(&example_dir()).expect("the example packs");
        std::fs::write(mods.join(format!("{}.zip", entry.sha256)), &zip).unwrap();
        // Ports are picked by binding and releasing, so the tests of this process start their lobbies one at a time; a
        // client's connections still take ephemeral ports, so a lobby that loses a port starts again on new ones.
        static STARTS: Mutex<()> = Mutex::new(());
        let _one_at_a_time = STARTS.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..PORT_TRIES {
            let mut l = Lobby::spawn(dir, rom, shape, &entry);
            if l.wait_ready() {
                return l;
            }
        }
        panic!("the lobby lost a port {PORT_TRIES} times");
    }

    /// One start on freshly picked ports.
    fn spawn(
        dir: &Path,
        rom: Option<&Path>,
        shape: Shape,
        entry: &dc_modkit::ModSetEntry,
    ) -> Lobby {
        let mods = dir.join("mods");
        let auth = auth_port();
        let mut admin = free_tcp_port();
        // a freed port comes back quickly on Windows: not one of the lobby's own
        while (auth..auth + 16).contains(&admin) {
            admin = free_tcp_port();
        }
        let game = game_port();
        let mut toml = format!(
            "[server]\nname = \"{}\"\nbind = \"127.0.0.1\"\nadvertise_ip = \"127.0.0.1\"\n\
             auth_port = {auth}\nlobby_port = {}\nclan_port = {}\nhttp_port = {}\nhttp_enabled = true\ngame_port = {game}\n\
             [paths]\ndb = \"{}\"\nlog_dir = \"{}\"\nmods_dir = \"{}\"\n",
            match shape {
                Shape::Entry => ENTRY_NAME,
                Shape::Modded => "vanilla++ test",
            },
            auth + 2,
            auth + 3,
            auth + 8080,
            toml_path(&dir.join("data").join("ca.db")),
            toml_path(dir),
            toml_path(&mods),
        );
        if let Some(r) = rom {
            toml.push_str(&format!("rom = \"{}\"\n", toml_path(r)));
        }
        toml.push_str(&format!(
            "[dserver]\nport_range = \"{}-{}\"\ntick_hz = 30\n[auth]\nauto_create = true\n",
            game + 1,
            game + 8,
        ));
        toml.push_str(&match shape {
            Shape::Entry => format!(
                "[mods]\npolicy = \"vanilla\"\n[[lobby.servers]]\nname = \"{ENTRY_NAME}\"\ncapacity = 16\n\
                 mods = [\n  {{ id = \"{}\", version = \"{}\", sha256 = \"{}\" }},\n]\n",
                entry.id, entry.version, entry.sha256
            ),
            Shape::Modded => format!(
                "[mods]\npolicy = \"modded\"\n[[mods.set]]\nid = \"{}\"\nversion = \"{}\"\nsha256 = \"{}\"\n",
                entry.id, entry.version, entry.sha256
            ),
        });
        let cfg = dir.join("server.toml");
        std::fs::write(&cfg, toml).unwrap();
        let log = dir.join("lobby.log");
        let _ = std::fs::remove_file(&log);
        let out = std::fs::File::create(dir.join("lobby.out")).unwrap();
        let mut cmd = Command::new(lobby_exe());
        cmd.arg("--config")
            .arg(&cfg)
            .arg("--dserver")
            .arg("--dserver-exe")
            .arg(dserver_exe())
            .arg("--dserver-logdir")
            .arg(dir)
            .arg("--dserver-verbose")
            .args(["--dserver-idle", "5"])
            .arg("--admin")
            .arg(format!("127.0.0.1:{admin}"))
            .arg("--log")
            .arg(&log)
            .arg("--autochar")
            .args(["--stop-grace", "10"])
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out.try_clone().unwrap()))
            .stderr(Stdio::from(out));
        match rom {
            Some(r) => {
                cmd.env("CA_ROM", r);
            }
            None => {
                // the ROM-free test stays ROM-free whatever the environment says
                cmd.env_remove("CA_ROM");
            }
        }
        let child = cmd.spawn().expect("spawn dc-lobby-server");
        Lobby {
            child: Some(child),
            auth,
            admin,
            dir: dir.to_path_buf(),
            log,
            sha256: entry.sha256.clone(),
        }
    }

    /// Waits for the auth port and the admin endpoint's answer (it opens last). False when the lobby lost a port; any
    /// other failure to come up ends the test.
    fn wait_ready(&mut self) -> bool {
        let end = Instant::now() + Duration::from_secs(30);
        while !(TcpStream::connect(("127.0.0.1", self.auth)).is_ok()
            && self.status().contains("community_maps"))
        {
            let out = std::fs::read_to_string(self.dir.join("lobby.out")).unwrap_or_default();
            let exited = self
                .child
                .as_mut()
                .is_some_and(|c| matches!(c.try_wait(), Ok(Some(_))));
            if exited && out.contains("Address already in use") {
                return false;
            }
            assert!(
                !exited && Instant::now() < end,
                "the lobby did not listen; log:\n{}\nout:\n{out}",
                self.log_text(),
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn wait_log(&self, needle: &str, t: Duration) -> bool {
        let end = Instant::now() + t;
        while !self.log_text().contains(needle) {
            if Instant::now() > end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }

    fn status(&self) -> String {
        http(self.admin, "GET", "/status")
    }

    /// The map's community index from `GET /status`.
    fn map_index(&self) -> Option<i32> {
        let status = self.status();
        let v: serde_json::Value = serde_json::from_str(status.split("\r\n\r\n").last()?).ok()?;
        v["community_maps"]
            .as_array()?
            .iter()
            .find(|m| m["key"] == MAP_KEY)
            .and_then(|m| m["index"].as_i64())
            .map(|i| i as i32)
    }

    /// The entry's set document (`GET /entry_mods/1.json` on the web port), once it holds a map; `None` when the
    /// lobby offers none within `t`.
    fn entry_doc(&self, t: Duration) -> Option<serde_json::Value> {
        let end = Instant::now() + t;
        loop {
            let reply = http(
                self.auth + 8080,
                "GET",
                &format!("/entry_mods/{ENTRY_ID}.json"),
            );
            let doc = reply
                .split("\r\n\r\n")
                .last()
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                .filter(|v| v.get("maps").is_some());
            if doc.is_some() || Instant::now() > end {
                return doc;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The graceful stop through the admin endpoint, then the child handle.
    fn stop(&mut self) {
        let _ = http(self.admin, "POST", "/stop");
        if let Some(mut c) = self.child.take() {
            let end = Instant::now() + Duration::from_secs(15);
            while Instant::now() < end {
                if let Ok(Some(_)) = c.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Lobby {
    fn drop(&mut self) {
        self.stop();
        for pid in logged_pids(&self.log_text()) {
            kill_pid(pid);
        }
    }
}

fn menu(auth: u16, rom: Option<&Path>, data: &Path) -> Menu {
    let mut link = LinkOptions::new("vanilla-plus");
    link.mods.data_dir = Some(data.to_path_buf());
    link.auto_character = true;
    Menu::new(MenuOptions {
        auth: SocketAddr::from(([127, 0, 0, 1], auth)),
        user: LOGIN.into(),
        link,
        rom: rom.map(Path::to_path_buf),
        font: if rom.is_some() {
            None
        } else {
            Some((synth_font(), 0))
        },
        width: 1024,
        height: 768,
    })
}

/// Drives the menu until a lobby-side screen with the map's record in the catalog; the boxes seen on the way.
fn walk_to_the_record(m: &mut Menu, pilot: &mut Pilot, lobby: &Lobby) -> Vec<String> {
    let start = Instant::now();
    let mut boxes: Vec<String> = Vec::new();
    loop {
        m.update();
        if let Some(p) = m.popup() {
            if boxes.last() != Some(&p.text) {
                boxes.push(p.text.clone());
            }
        }
        for i in pilot.next(m) {
            m.input(i);
        }
        let _ = m.take_outcome();
        let has_map = m.cat.community_maps().iter().any(|r| r.id == MAP_KEY);
        if has_map && matches!(m.screen_name(), "lobby" | "create" | "room") {
            println!("screens {:?}; boxes {boxes:?}", m.trail);
            return boxes;
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "no lobby screen with the map record within 60 s: screens {:?}, boxes {boxes:?}, lobby log:\n{}",
            m.trail,
            lobby.log_text()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The pilot's plan of one match. The menu pilot's servers arm enters the row the screen picks by itself, the first
/// non-clan entry below 95 % (on the entry shape, the lobby's one entry).
fn pilot_plan(room: PilotRoom, timeout: Duration) -> PilotPlan {
    PilotPlan {
        password: LOGIN.into(),
        room,
        matches: 1,
        shots: None,
        timeout,
    }
}

/// The ROM-free half of the entry proof: the entry document lists the map, the client picks the entry, the set loads
/// at the pick and the waiting room's catalog holds the map at the entry's base.
#[test]
fn entry_lists_the_starter_map_and_the_client_loads_it_at_the_pick() {
    let dir = scratch("vplus-entry-list");
    let wd = Watchdog::arm("vanilla++ entry listing", Duration::from_secs(180));
    let mut lobby = Lobby::start(&dir, None, Shape::Entry);
    wd.watch_pid(lobby.child.as_ref().map(Child::id));
    wd.watch_log(&lobby.log);
    // the entry document (contract b): the set and the map records, the same base as a modded lobby's
    let doc = lobby.entry_doc(Duration::from_secs(20)).unwrap_or_else(|| {
        panic!(
            "GET /entry_mods/{ENTRY_ID}.json holds no maps; log:\n{}",
            lobby.log_text()
        )
    });
    assert_eq!(doc["name"], ENTRY_NAME, "{doc}");
    assert_eq!(doc["curated"], true, "{doc}");
    assert_eq!(doc["community_map_base"], ROMLESS_BASE, "{doc}");
    let maps = doc["maps"].as_array().expect("the records");
    assert_eq!(maps.len(), 1, "{doc}");
    assert_eq!(maps[0]["id"], MAP_KEY, "{doc}");
    assert_eq!(maps[0]["mod_id"], MAP_KEY, "{doc}");
    assert_eq!(maps[0]["world"], WORLD, "{doc}");
    assert!(
        !lobby.log_text().contains("left out"),
        "the pack loads on the entry:\n{}",
        lobby.log_text()
    );

    // the client's own menu link: a vanilla login, the pick loads the set from the web port (no consent, CLI-1643)
    let mut m = menu(lobby.auth, None, &dir.join("client"));
    m.set_idle_rules(false);
    let mut pilot = Pilot::new(pilot_plan(
        PilotRoom::Create {
            title: "vanilla++".into(),
            mode: None,
            map: Some(ROMLESS_BASE),
            start_after: 1,
        },
        Duration::from_secs(60),
    ))
    .on_entry(ENTRY_ID);
    walk_to_the_record(&mut m, &mut pilot, &lobby);
    let rec = m
        .cat
        .community_maps()
        .iter()
        .find(|r| r.id == MAP_KEY)
        .expect("the record")
        .clone();
    assert_eq!(rec.world, WORLD);
    assert_eq!(rec.mod_id, MAP_KEY);
    assert!(rec.modes.iter().any(|x| x == "TDM"), "{:?}", rec.modes);
    // the entry's base reached the catalogue and the index round-trips (contract e)
    assert_eq!(
        m.cat.first_community_index(),
        ROMLESS_BASE as usize,
        "the entry document's community_map_base reached the catalogue"
    );
    assert_eq!(
        m.cat.community_map(ROMLESS_BASE).map(|r| r.id.as_str()),
        Some(MAP_KEY)
    );
    // the pack reached the client's cache under the hash the entry names
    let unpacked = dir
        .join("client")
        .join("mods")
        .join("unpacked")
        .join(&lobby.sha256);
    assert!(
        unpacked.join("manifest.json").is_file(),
        "the entry's pack unpacked at {}",
        unpacked.display()
    );
    assert!(
        !lobby.log_text().contains("mod set mismatch"),
        "the entry's claim matched its set:\n{}",
        lobby.log_text()
    );
    // the entry-scoped 0x311 table arrived (the synthetic rows); no bits for the map without mode rows (MAP-35)
    assert!(!m.data.masks.is_empty(), "room settings received");
    assert!(
        !m.data
            .masks
            .iter()
            .any(|(map, _)| *map as i32 == ROMLESS_BASE),
        "no 0x311 pair for the community map without a ROM: {:?}",
        m.data.masks
    );
    drop(m);
    lobby.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The ROM-free half of the modded proof: the lobby lists the map, the client logs in with the set and holds the record.
#[test]
fn modded_lobby_lists_the_starter_map_and_the_client_logs_in_with_its_set() {
    let dir = scratch("vplus-list");
    let wd = Watchdog::arm("vanilla++ listing", Duration::from_secs(180));
    let mut lobby = Lobby::start(&dir, None, Shape::Modded);
    wd.watch_pid(lobby.child.as_ref().map(Child::id));
    wd.watch_log(&lobby.log);
    assert!(
        lobby.wait_log("maps: 1 community map(s)", Duration::from_secs(20)),
        "{}",
        lobby.log_text()
    );
    assert!(
        lobby.wait_log("community maps 1", Duration::from_secs(10)),
        "the start-up line counts it:\n{}",
        lobby.log_text()
    );
    // without a ROM dc_bute's synthetic 0x311 rows (0..12) come first: the map sits at index 12 (MAP-35)
    let status = lobby.status();
    assert!(
        status.contains(r#""key": "starter_map""#),
        "GET /status lists the map: {status}"
    );
    assert_eq!(lobby.map_index(), Some(12), "{status}");
    assert!(
        status.contains(&format!("\"mod\": \"{MAP_KEY}\"")),
        "{status}"
    );

    // the client's own menu link: the set is read from the web port, the package downloaded and hash-checked
    // (sha256 of the packed example), the login carries the LG-271 suffix, the lobby screen gets the record
    let mut m = menu(lobby.auth, None, &dir.join("client"));
    m.set_idle_rules(false);
    let mut pilot = Pilot::new(PilotPlan {
        password: LOGIN.into(),
        room: PilotRoom::Create {
            title: "vanilla++".into(),
            mode: None,
            map: Some(12),
            start_after: 1,
        },
        matches: 1,
        shots: None,
        timeout: Duration::from_secs(60),
    });
    walk_to_the_record(&mut m, &mut pilot, &lobby);
    let rec = m
        .cat
        .community_maps()
        .iter()
        .find(|r| r.id == MAP_KEY)
        .expect("the record")
        .clone();
    assert_eq!(rec.world, WORLD);
    assert_eq!(rec.mod_id, MAP_KEY);
    assert!(rec.modes.iter().any(|x| x == "TDM"), "{:?}", rec.modes);
    assert_eq!(m.cat.rom_map_count(), 0, "no ROM rows in this client");
    // MAP-32 / MAP-35: the lobby published its base (12: dc_bute's synthetic rows on a lobby without a ROM) in
    // the server list, so the client places the map at the lobby's index, not at its own row count
    assert_eq!(
        m.cat.first_community_index(),
        12,
        "the server list's community_map_base reached the catalogue"
    );
    assert_eq!(
        m.cat.community_map(12).map(|r| r.id.as_str()),
        Some(MAP_KEY),
        "the lobby's index 12 round-trips"
    );
    assert!(m.cat.community_map(0).is_none());
    // the package reached the client's cache under the hash the set names
    let unpacked = dir
        .join("client")
        .join("mods")
        .join("unpacked")
        .join(&lobby.sha256);
    assert!(
        unpacked.join("manifest.json").is_file(),
        "the set's package unpacked at {}",
        unpacked.display()
    );
    assert!(
        !lobby.log_text().contains("mod set mismatch"),
        "the login carried the set's hash:\n{}",
        lobby.log_text()
    );
    // the 0x311 table arrived (the synthetic rows); the map has no bits without mode rows (MAP-35)
    assert!(!m.data.masks.is_empty(), "room settings received");
    assert!(
        !m.data.masks.iter().any(|(map, _)| *map == 12),
        "no 0x311 pair for the community map without a ROM: {:?}",
        m.data.masks
    );
    drop(m);
    lobby.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The number after `label ` in a text.
fn number_after(text: &str, label: &str) -> Option<u32> {
    let i = text.find(label)? + label.len();
    text[i..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// The match both ROM-gated cases play: a TDM room on the community map, ~5 s headless, a clean end. The lobby is up and
/// lists the map at `index`.
fn play_the_match(lobby: &Lobby, rom: &Path, dir: &Path, index: i32, world_under: &str) {
    let names = dc_bute::GameRoomData::from_rom(rom)
        .map(|g| RoomNames::from_rows(&g.maps(), &g.modes(false)))
        .expect("the ROM room table");
    assert_eq!(
        index as usize,
        names.maps.len(),
        "after the ROM's rows (MAP-35)"
    );
    let tdm = names.mode("TDM").expect("a TDM row");

    let mut m = menu(lobby.auth, Some(rom), &dir.join("client"));
    let pilot = Pilot::new(pilot_plan(
        PilotRoom::Create {
            title: "vanilla++".into(),
            mode: Some(tdm),
            map: Some(index),
            start_after: 1,
        },
        Duration::from_secs(150),
    ));
    // the entry shape picks the entry by id, as `dc-mapkit play` does with `--pilot-entry`
    let mut pilot = if world_under.contains("entry_mods") {
        pilot.on_entry(ENTRY_ID)
    } else {
        pilot
    };
    let mut base = NetOptions::new(
        Connect::Lobby {
            auth: SocketAddr::from(([127, 0, 0, 1], lobby.auth)),
            room: RoomChoice::New {
                max_players: 8,
                title: "vanilla++".into(),
                mode: tdm,
                start_after: 1,
            },
            password: String::new(),
            channel: None,
        },
        LOGIN,
    );
    base.rom = Some(rom.to_path_buf());
    base.log = Some(dir.join("client_events.jsonl"));
    let shots = dir.join("shots");
    std::fs::create_dir_all(&shots).unwrap();
    let hopts = HeadlessOptions {
        // ~5 s of match at the wall clock, then the client leaves
        frames: 300,
        frame_us: 16_667,
        width: 640,
        height: 480,
        out: Some(shots.clone()),
        prefix: "vplus_".into(),
        save_every: u32::MAX,
        font_rom: Some(rom.to_path_buf()),
        realtime: true,
        stop_after_over: Some(60),
        hud: HudChoice::Rom(rom.to_path_buf()),
        debug_panel: false,
        sound: dc_client::sound::SoundOptions {
            enabled: false,
            log: None,
        },
        ..HeadlessOptions::default()
    };
    let mut lines = Vec::new();
    let report = {
        let mut play = headless_runner(&base, LOGIN.into(), &hopts, &mut lines);
        run_pilot(&mut m, &mut pilot, &mut play)
    };
    println!("screens {}", report.trail.join(" -> "));
    for l in &lines {
        println!("  {l}");
    }
    println!("boxes {:?}; end {}", report.boxes, report.end);

    // 1. one match was played and the pilot came back to the waiting room
    assert_eq!(report.matches, 1, "{report:?}");
    assert_eq!(report.end, "matches played");
    assert!(
        report.trail.iter().any(|s| s == "match")
            && report.trail.last().map(String::as_str) == Some("room"),
        "{:?}",
        report.trail
    );
    assert_eq!(m.data.room.map, index, "the room is on the community map");
    // 2. the client loaded the `@mod:` world through the set (no 0x29: a world failure is the error form of the
    //    line), every own texture resolved, frames rendered, the server's spawn used
    let line = lines.first().expect("the match line");
    assert!(
        line.contains(&format!("world Some(\"{WORLD}\")")),
        "the @mod: world: {line}"
    );
    assert!(line.contains(WORLD), "{line}");
    assert!(line.contains("textures missing 0"), "{line}");
    let frames = number_after(line, "\n  frames ").or_else(|| number_after(line, " frames "));
    assert!(frames.is_some_and(|f| f > 0), "frames rendered: {line}");
    assert!(line.contains("spawn Some("), "{line}");
    assert!(
        line.contains("end Some("),
        "a clean end of the session: {line}"
    );
    assert!(
        !line.contains("(phase "),
        "the match ran (the error form names the phase): {line}"
    );
    // 3. the lobby's launch line names the unpacked world and the announce
    let log = lobby.log_text();
    let launch = log
        .lines()
        .find(|l| l.contains("announced as @mod:"))
        .unwrap_or_else(|| panic!("no launch line:\n{log}"));
    assert!(
        launch.contains(&format!("(community map {MAP_KEY}, announced as {WORLD})")),
        "{launch}"
    );
    assert!(
        launch.replace('\\', "/").contains(world_under)
            && launch.replace('\\', "/").contains("assets/maps/starter_map.dat"),
        "the world file under {world_under}: {launch}"
    );
    let dserver_world = log
        .lines()
        .find(|l| l.contains("dc-dserver: world starter_map"))
        .unwrap_or_else(|| panic!("no game-server world line:\n{log}"));
    assert!(
        dserver_world.contains(&format!("announced as \"{WORLD}\"")),
        "{dserver_world}"
    );
    // 4. the match ended cleanly: the client left, the game server reported its end
    assert!(
        lobby.wait_log(": @leave client", Duration::from_secs(20)),
        "{}",
        lobby.log_text()
    );
    assert!(
        lobby.wait_log(": @end", Duration::from_secs(30)),
        "{}",
        lobby.log_text()
    );
    assert!(!lobby.log_text().contains("@crash"), "{}", lobby.log_text());
    drop(m);
}

/// The stop after a match, the log kept for the assertion on it.
fn stop_and_check(lobby: &mut Lobby, dir: &Path) {
    lobby.stop();
    assert!(
        lobby.log_text().contains("stopped"),
        "the graceful stop:\n{}",
        lobby.log_text()
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The whole entry proof, with the user's ROM: the pilot picks the entry, a room on the entry's community map, the
/// match played headless, the world from the entry's unpacked pack (contract d, e), a clean end.
#[test]
fn entry_room_on_the_starter_map_plays_end_to_end_with_the_rom() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        println!("CA_ROM not set: skipped");
        return;
    };
    let dir = scratch("vplus-entry-play");
    let wd = Watchdog::arm("vanilla++ entry match", Duration::from_secs(300));
    let mut lobby = Lobby::start(&dir, Some(&rom), Shape::Entry);
    wd.watch_pid(lobby.child.as_ref().map(Child::id));
    wd.watch_log(&lobby.log);
    let doc = lobby.entry_doc(Duration::from_secs(30)).unwrap_or_else(|| {
        panic!(
            "GET /entry_mods/{ENTRY_ID}.json holds no maps; log:\n{}",
            lobby.log_text()
        )
    });
    let index = doc["community_map_base"]
        .as_i64()
        .expect("the entry's base") as i32;
    play_the_match(&lobby, &rom, &dir, index, "entry_mods/cache");
    stop_and_check(&mut lobby, &dir);
}

/// The whole modded proof, with the user's ROM: a room on the community map of the lobby-wide set.
#[test]
fn modded_room_on_the_starter_map_plays_end_to_end_with_the_rom() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        println!("CA_ROM not set: skipped");
        return;
    };
    let dir = scratch("vplus-play");
    let wd = Watchdog::arm("vanilla++ match", Duration::from_secs(300));
    let mut lobby = Lobby::start(&dir, Some(&rom), Shape::Modded);
    wd.watch_pid(lobby.child.as_ref().map(Child::id));
    wd.watch_log(&lobby.log);
    assert!(
        lobby.wait_log("maps: 1 community map(s)", Duration::from_secs(30)),
        "{}",
        lobby.log_text()
    );
    let index = lobby.map_index().expect("the map's index from GET /status");
    play_the_match(&lobby, &rom, &dir, index, "mod_cache");
    stop_and_check(&mut lobby, &dir);
}
