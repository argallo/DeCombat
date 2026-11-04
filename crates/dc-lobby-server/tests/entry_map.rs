//! A community map on a featured server entry, end to end over the 2009 protocol (MAP-80 .. MAP-84): the real
//! `dc-lobby-server` binary, vanilla, one `[[lobby.servers]]` entry "Vanilla++" with the committed starter map as its
//! curated set; the stand-in game server records its launch. A login claims the entry's hashes (0x3f11), picks it,
//! opens a room and picks the map. Locally the start carries the world flags; on a remote host the REAL `dc-host`
//! binary over its pinned TLS control stream gets the pack lines, unpacks them and adds the flags itself.
//! No ROM: the map sits after dc_bute's synthetic rows (index 12).
// DC-DEFAULT: MAP-81, MAP-82, MAP-83, MAP-84; SPEC: HOST_AGENT.md section 3

mod support;
use dc_wire::entrymods;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use support::*;

const T: Duration = Duration::from_secs(20);
const TOKEN: &str = "0123456789abcdef01234567";
const ANNOUNCE: &str = "@mod:starter_map/assets/maps/starter_map.dat";

struct Fixture {
    dir: PathBuf,
    mods: PathBuf,
    sha: String,
    version: String,
    size: usize,
}

/// The committed starter map packed into `<mods>/<sha256>.zip`.
fn fixture(tag: &str) -> Fixture {
    let dir = scratch_dir(tag);
    let mods = dir.join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/starter_map");
    let (bytes, e) = dc_modkit::pack_mod_bytes(&src).expect("the example packs");
    std::fs::write(mods.join(format!("{}.zip", e.sha256)), &bytes).unwrap();
    Fixture {
        dir,
        mods,
        sha: e.sha256,
        version: e.version.to_string(),
        size: bytes.len(),
    }
}

/// These cases run without a ROM whatever the shell says: the map's index is then 12.
fn no_rom() -> Vec<(String, String)> {
    vec![("CA_ROM".to_string(), String::new())]
}

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// The vanilla lobby's config: the one entry on `host`, and `extra` after it.
fn write_config(f: &Fixture, host: &str, extra: &str) -> PathBuf {
    let cfg = f.dir.join("server.toml");
    std::fs::write(
        &cfg,
        format!(
            "[paths]\nmods_dir = \"{}\"\n[[lobby.servers]]\nname = \"Vanilla++\"\ncapacity = 100\nhost = \"{host}\"\n\
             mods = [{{ id = \"starter_map\", version = \"{}\", sha256 = \"{}\" }}]\n{extra}",
            toml_path(&f.mods),
            f.version,
            f.sha
        ),
    )
    .unwrap();
    cfg
}

fn http_get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("web port");
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

/// The auth steps with the entry's claim: what our client does at a pick (CLI-1643); the lobby session on channel 1.
fn player_on_entry(l: &Lobby, user: &str, nick: &str) -> (Client, u32) {
    let mut a = Client::connect(l.port, user);
    a.send(0x206, |w| {
        w.pad(32);
    });
    assert_eq!(u32_at(&a.recv(0x207), 6), 1);
    a.send(0x200, |w| {
        w.wstr(&format!("LOCAL:{user}"), 4096)
            .wstr("pw", 17)
            .wstr("00-11-22-33-44-55", 20)
            .i32(0);
    });
    let d = a.recv(0x201);
    if u32_at(&d, 6) == 0 {
        a.send(0x204, |w| {
            w.wstr(nick, 17);
        });
        assert_eq!(u32_at(&a.recv(0x205), 6), 1);
        a.send(0x202, |w| {
            w.i32(6002).wstr(nick, 17);
        });
        assert_eq!(u32_at(&a.recv(0x203), 6), 1);
    }
    let push = entrymods::parse_push(&a.recv(entrymods::PUSH));
    assert_eq!(push.len(), 1, "{push:?}");
    assert_eq!(push[0].entry, 1);
    a.recv(0x2000);
    a.send_frame(&entrymods::claim_message(&push[0]).into_frame());
    a.send(0x20a, |w| {
        w.u32(1);
    });
    let d = a.recv(0x20b);
    assert_eq!(u32_at(&d, 6), 1, "the pick with the claim");
    let key = d[0xe..0x1e].to_vec();
    drop(a);
    let mut c = Client::connect(l.lport, user);
    let n = l.user_no(user);
    assert_eq!(enter_lobby(&mut c, &key, n), nick);
    (c, n)
}

fn example_world() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/mods/starter_map/assets/maps/starter_map.dat")
}

/// Two players on the entry, a room of `mode` on map `map`, the start: the game's address.
fn start_on_map(l: &Lobby, tag: &str, mode: u32, map: u32) -> (String, u32) {
    let (mut a, _) = player_on_entry(l, &format!("{tag}a"), &format!("Host {tag}"));
    let (mut b, _) = player_on_entry(l, &format!("{tag}b"), &format!("Guest {tag}"));
    let no = open_room(&mut a, "Starter room", mode);
    set_room(&mut a, FIELD_MAP, map);
    join_room(&mut b, no);
    ready(&mut b, 1);
    ready(&mut a, 2);
    game_address(&mut a, T)
}

fn assert_entry_doc(l: &Lobby, f: &Fixture) {
    let doc = http_get(l.web, "/entry_mods/1.json");
    assert!(doc.contains(r#""community_map_base": 12"#), "{doc}");
    assert!(
        doc.contains(r#""maps": ["#) && doc.contains(r#""id": "starter_map""#),
        "{doc}"
    );
    assert!(doc.contains(&format!(r#""world": "{ANNOUNCE}""#)), "{doc}");
    assert!(doc.contains(&f.sha), "{doc}");
}

/// Every value of `flag`, in order.
fn values<'a>(launch: &'a FakeLaunch, flag: &str) -> Vec<&'a str> {
    launch
        .args
        .iter()
        .enumerate()
        .filter(|(_, a)| a.as_str() == flag)
        .filter_map(|(i, _)| launch.args.get(i + 1).map(String::as_str))
        .collect()
}

#[test]
fn a_local_entry_offers_its_map_and_starts_it_with_the_world_flags() {
    let f = fixture("entry-map-local-fixture");
    let cfg = write_config(&f, "", "");
    let l = Lobby::start(
        "entry-map-local",
        LobbyOpts {
            dserver_exe: Some(fake_dserver_exe()),
            extra: vec!["--config".into(), cfg.display().to_string()],
            env: no_rom(),
            ..Default::default()
        },
    );
    assert!(
        l.wait_log(
            "maps: entry 1: 1 community map(s) after the 12 ROM row(s): 12=starter_map",
            T
        ),
        "{}",
        l.log_text()
    );
    assert_entry_doc(&l, &f);
    // the lobby's own set stays vanilla
    let list = http_get(l.web, "/server_list.json");
    assert!(list.contains(r#""policy": "vanilla""#), "{list}");
    let (ip, _) = start_on_map(&l, "loc", 0, 12);
    assert_eq!(ip, "127.0.0.1");
    let launches = l.fake();
    assert_eq!(launches.len(), 1, "{launches:?}");
    let launch = &launches[0];
    let world = PathBuf::from(launch.arg("-worldfile").expect("-worldfile"));
    assert!(
        world.is_absolute() && world.is_file(),
        "{}",
        world.display()
    );
    assert!(
        world.ends_with(
            Path::new("entry_mods/cache")
                .join(&f.sha)
                .join("assets/maps/starter_map.dat")
        ),
        "{}",
        world.display()
    );
    assert_eq!(launch.arg("-announce"), Some(ANNOUNCE), "{:?}", launch.args);
    assert_eq!(
        values(launch, "-worldobjects").last(),
        Some(&"world"),
        "{:?}",
        launch.args
    );
    assert!(
        l.log_has(&format!(
            "(community map starter_map, announced as {ANNOUNCE})"
        )),
        "{}",
        l.log_text()
    );
    drop(l);
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// The real agent, killed with this guard.
struct Agent(Child);

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The `dc-host` binary of this tree: cargo brings it up to date first (a sibling left by an earlier build may be old).
fn dc_host_exe() -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "-p", "dc-host", "--bin", "dc-host"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("run cargo build -p dc-host");
    assert!(status.success(), "cargo build -p dc-host failed");
    let exe = lobby_exe().with_file_name(format!("dc-host{}", std::env::consts::EXE_SUFFIX));
    assert!(exe.is_file(), "no dc-host at {}", exe.display());
    exe
}

/// Four UDP ports in a row that are free now. Scanned in 20000..40000 from a start that differs per run: the
/// OS's own picks come from the ephemeral range, which Windows hands out in order from 49152 up, so asking it for a
/// port can stay above 60000 for the whole loop.
fn udp_range() -> (u16, u16) {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        ^ std::process::id().wrapping_mul(2_654_435_761);
    let start = 20000 + (seed % 5000) as u16 * 4;
    for i in 0..5000u16 {
        let lo = 20000 + (start - 20000 + i * 4) % 20000;
        if (lo..lo + 4).all(udp_free) {
            return (lo, lo + 3);
        }
    }
    panic!("no free UDP range");
}

/// A lobby whose one entry plays on agent `us-west`, and that agent's folder and config.
struct Remote {
    f: Fixture,
    l: Lobby,
    host_dir: PathBuf,
    toml: PathBuf,
    ports: (u16, u16),
    exe: PathBuf,
}

impl Remote {
    fn agent(&self) -> Agent {
        Agent(
            Command::new(&self.exe)
                .arg("--config")
                .arg(&self.toml)
                .env("DC_FAKE_DIR", &self.l.fake_dir)
                .env_remove("CA_ROM")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn dc-host"),
        )
    }
}

/// `game_server` is the exe on both sides; `rom` goes to the lobby's environment and to the agent's `rom`.
fn remote(tag: &str, game_server: PathBuf, rom: Option<&Path>) -> Remote {
    let f = fixture(&format!("{tag}-fixture"));
    let hosts_port = free_tcp_port();
    let cfg = write_config(
        &f,
        "us-west",
        &format!(
            "[hosts]\nport = {hosts_port}\n[[hosts.agents]]\nname = \"us-west\"\ntoken = \"{TOKEN}\"\npublic_address = \"127.0.0.1\"\n"
        ),
    );
    let l = Lobby::start(
        tag,
        LobbyOpts {
            dserver_exe: Some(game_server.clone()),
            extra: vec![
                "--config".into(),
                cfg.display().to_string(),
                "--dserver-args".into(),
                "-worldobjects off -tickhz 30".into(),
            ],
            env: match rom {
                Some(r) => vec![("CA_ROM".to_string(), r.display().to_string())],
                None => no_rom(),
            },
            ..Default::default()
        },
    );
    assert!(l.wait_log("--server-fingerprint ", T), "{}", l.log_text());
    let log = l.log_text();
    let fp = log
        .lines()
        .find_map(|t| t.split("--server-fingerprint ").nth(1))
        .expect("the fingerprint line")
        .trim()
        .to_string();
    let host_dir = f.dir.join("host");
    let logs = host_dir.join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let ports = udp_range();
    let toml = host_dir.join("host.toml");
    let rom_line = rom.map_or(String::new(), |r| format!("rom = \"{}\"\n", toml_path(r)));
    std::fs::write(
        &toml,
        format!(
            "name = \"us-west\"\nlobby = \"127.0.0.1:{hosts_port}\"\nfingerprint = \"{fp}\"\ntoken = \"{TOKEN}\"\n\
             dserver_exe = \"{}\"\nport_range = \"{}-{}\"\nbind_ip = \"127.0.0.1\"\nlog_dir = \"{}\"\n{rom_line}",
            toml_path(&game_server),
            ports.0,
            ports.1,
            toml_path(&logs),
        ),
    )
    .unwrap();
    Remote {
        f,
        l,
        host_dir,
        toml,
        ports,
        exe: dc_host_exe(),
    }
}

/// THE PROOF OF MAP-84 on one machine: real lobby, real agent, real TLS pin, real control stream, real pack bytes (the
/// 3.7 MiB example is hundreds of `pack` lines). Only the game server is the stand-in and the network is loopback.
#[test]
fn a_remote_entry_gets_the_pack_through_the_real_agent_which_adds_the_flags_and_keeps_it() {
    let r = remote("entry-map-remote", fake_dserver_exe(), None);
    let (f, l, ports) = (&r.f, &r.l, r.ports);
    l.set_fake_mode("record");
    assert_entry_doc(l, f);
    let agent = r.agent();
    assert!(l.wait_log("host us-west online", T), "{}", l.log_text());

    let (ip, port) = start_on_map(l, "rem", 0, 12);
    assert_eq!(ip, "127.0.0.1");
    assert!(
        (u32::from(ports.0)..=u32::from(ports.1)).contains(&port),
        "{port} {ports:?}"
    );
    assert!(
        l.log_has(&format!(
            "host us-west: map pack {} ({} bytes) sent",
            f.sha, f.size
        )),
        "{}",
        l.log_text()
    );
    let launches = l.fake();
    assert_eq!(launches.len(), 1, "{launches:?}");
    let launch = &launches[0];
    let unpacked = r.host_dir.join("packs").join(&f.sha);
    let world = std::path::absolute(unpacked.join("assets/maps/starter_map.dat")).unwrap();
    assert_eq!(
        launch.arg("-worldfile"),
        Some(world.to_string_lossy().as_ref()),
        "{:?}",
        launch.args
    );
    assert_eq!(
        launch.arg("-world"),
        Some(world.to_string_lossy().as_ref()),
        "{:?}",
        launch.args
    );
    assert_eq!(launch.arg("-announce"), Some(ANNOUNCE), "{:?}", launch.args);
    assert_eq!(
        values(launch, "-worldobjects"),
        ["world"],
        "the lobby's own is dropped: {:?}",
        launch.args
    );
    assert_eq!(launch.arg("-tickhz"), Some("30"), "{:?}", launch.args);
    assert_eq!(
        std::fs::read(&world).unwrap(),
        std::fs::read(example_world()).unwrap(),
        "the world arrived byte for byte"
    );
    assert!(r
        .host_dir
        .join("packs")
        .join(format!("{}.zip", f.sha))
        .is_file());
    assert!(
        wait_until(T, || l.fake()[0].room_line().is_some()),
        "the room line reached the stand-in"
    );

    // an agent restart: it keeps the pack and says so, and the next start sends no byte of it
    drop(agent);
    assert!(l.wait_log("host us-west offline", T), "{}", l.log_text());
    let _agent = r.agent();
    assert!(
        l.wait_log(&format!("host us-west: has map pack {}", f.sha), T),
        "{}",
        l.log_text()
    );
    let (_, port2) = start_on_map(l, "two", 0, 12);
    assert!(
        (u32::from(ports.0)..=u32::from(ports.1)).contains(&port2),
        "{port2} {ports:?}"
    );
    assert!(wait_until(T, || l.fake().len() == 2), "{:?}", l.fake());
    assert_eq!(
        l.log_text()
            .matches(&format!("map pack {} ({} bytes) sent", f.sha, f.size))
            .count(),
        1,
        "{}",
        l.log_text()
    );
    let dir = f.dir.clone();
    drop(r);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ROM-gated: the REAL `dc-dserver` under the real agent loads the world from the unpacked pack and says ready, so the
/// lobby sends the game-start packet. No client plays the match here: that is crates/dc-client/tests/vanilla_plus.rs.
#[test]
fn the_real_game_server_under_the_real_agent_loads_the_world_of_the_pack() {
    let Some(rom) = Rom::open() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let tdm = rom.mode_index("TDM").expect("a TDM row");
    let r = remote("entry-map-real", dserver_exe(), Some(&rom.dir));
    let (f, l) = (&r.f, &r.l);
    let doc = http_get(l.web, "/entry_mods/1.json");
    let base: u32 = doc
        .split("\"community_map_base\": ")
        .nth(1)
        .and_then(|t| t.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| panic!("{doc}"));
    assert!(doc.contains(r#""id": "starter_map""#), "{doc}");
    let _agent = r.agent();
    assert!(l.wait_log("host us-west online", T), "{}", l.log_text());
    let (ip, port) = start_on_map(l, "real", tdm, base);
    assert_eq!(ip, "127.0.0.1");
    assert!(
        (u32::from(r.ports.0)..=u32::from(r.ports.1)).contains(&port),
        "{port}"
    );
    assert!(
        l.log_has(&format!(
            "host us-west: map pack {} ({} bytes) sent",
            f.sha, f.size
        )),
        "{}",
        l.log_text()
    );
    // the game server's own log, in the agent's log folder: the world it loaded and the name it announces
    let logs = r.host_dir.join("logs");
    let read = || {
        std::fs::read_dir(&logs)
            .map(|rd| {
                rd.flatten()
                    .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    let announced = format!("announced as {ANNOUNCE:?}");
    assert!(wait_until(T, || read().contains(&announced)), "{}", read());
    let size = std::fs::metadata(example_world()).unwrap().len();
    assert!(
        read().contains(&format!("world starter_map ({size} bytes")),
        "{}",
        read()
    );
    assert!(!l.log_has("could not start the server"), "{}", l.log_text());
    let dir = f.dir.clone();
    drop(r);
    let _ = std::fs::remove_dir_all(&dir);
}
