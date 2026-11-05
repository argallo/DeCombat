//! server.toml wiring, end to end: the real `dc-lobby-server` binary is started on ephemeral loopback ports
//! with a config file, and the effect is read back through the 2009 protocol (server list, room settings),
//! through the server-list data file, and through `doctor`.
//!
//! Hermetic: `CA_ROM` and every `DC_*` variable are removed from the child's environment (except the ones a test
//! sets), the DB and logs live in a per-test temp folder, every started process is stopped by PID.
// SPEC: LOBBY_HARNESS.md §1 (flags), PROTOCOL_2009.md §1.2 (frames), §2.2 (login, server list), §4 (rooms)

use dc_wire::tcp::{read_tcp_frame, FrameHeader8};
use dc_wire::PktWriter;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_dc-lobby-server");

/// A free loopback TCP port in 20000..40000 (a clock- and pid-seeded start, then the first that binds): port 0 hands
/// out the dynamic range above 49152, parts of which Windows reserves, so the server could not take it back (CI).
/// A call counter joins the seed: the tests run on threads, the clock is coarse on Windows (the same reading for every
/// thread of one tick), and two tests that drew the same start handed the same port to two servers, the second of
/// which exited before it listened (CI, windows-i686).
fn free_port() -> u16 {
    static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        ^ std::process::id().wrapping_mul(2_654_435_761)
        ^ call.wrapping_mul(40_503);
    let start = (seed % 20000) as u16;
    for i in 0..20000u16 {
        let port = 20000 + (start.wrapping_add(i * 7)) % 20000;
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("no free TCP port in 20000..40000")
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc_cfgwire_{}_{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

fn command(args: &[String], env: &[(&str, &str)]) -> Command {
    let mut c = Command::new(EXE);
    c.args(args).env_remove("CA_ROM").env_remove("DC_CONFIG");
    for (k, _) in std::env::vars() {
        if k.starts_with("DC_") && k != "DC_TARGET" {
            c.env_remove(k);
        }
    }
    for (k, v) in env {
        c.env(k, v);
    }
    c
}

/// A running server; stopped by PID on drop.
struct Server {
    child: Child,
}

/// Removes a test folder on drop (declared first in a test, so it is dropped after the servers).
struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(args: &[&str], env: &[(&str, &str)], auth_port: u16) -> Server {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let child = command(&args, env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    let mut s = Server { child };
    // a loaded machine may start the process slowly: the wait is long (90 s) and ends early, with the exit status,
    // when the process is gone; what the tests assert afterwards is unchanged
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(90) {
        if TcpStream::connect(("127.0.0.1", auth_port)).is_ok() {
            return s;
        }
        if let Ok(Some(status)) = s.child.try_wait() {
            panic!("server exited before it listened on {auth_port}: {status}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server did not listen on {auth_port} within 90 s");
}

struct Conn(TcpStream);

impl Conn {
    fn open(port: u16) -> Conn {
        let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        Conn(s)
    }

    fn send(&mut self, id: u16, fill: impl FnOnce(&mut PktWriter)) {
        let mut w = PktWriter::new(id);
        fill(&mut w);
        let p = w.into_payload();
        let mut f = FrameHeader8 {
            id: id as i32,
            size: p.len() as i32,
        }
        .to_bytes()
        .to_vec();
        f.extend_from_slice(&p);
        self.0.write_all(&f).unwrap();
    }

    /// Next payload with frame id `id`, skipping others.
    fn recv(&mut self, id: u16) -> Vec<u8> {
        loop {
            let (fid, p) = read_tcp_frame(&mut self.0)
                .expect("read")
                .expect("connection closed");
            if fid as u16 == id {
                return p;
            }
        }
    }
}

fn u32_at(p: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]])
}

fn wstr_at(p: &[u8], o: usize, n: usize) -> String {
    let u: Vec<u16> = (0..n)
        .map(|i| u16::from_le_bytes([p[o + 2 * i], p[o + 2 * i + 1]]))
        .take_while(|c| *c != 0)
        .collect();
    String::from_utf16_lossy(&u)
}

/// Auth login with a new account + character; returns the server-list entry 0 `(ip, port, capacity)`, the
/// account number and the auth connection.
fn login(auth_port: u16, name: &str) -> ((String, u32, u32), u32, Conn) {
    let mut a = Conn::open(auth_port);
    a.send(0x200, |w| {
        w.wstr(&format!("LOCAL:{name}"), 4096)
            .wstr("pw", 17)
            .wstr("00-11", 20)
            .i32(0);
    });
    let r = a.recv(0x201);
    let account = u32_at(&r, 0x12);
    if u32_at(&r, 6) == 0 {
        a.send(0x202, |w| {
            w.i32(6002).wstr(name, 17);
        });
        assert_eq!(u32_at(&a.recv(0x203), 6), 1, "character created");
    }
    let list = a.recv(0x2000);
    // entry 0 starts at +0x16: name wstr[33] at +4, ip wstr[33] at +0x46, port at +0x88, capacity at +0x90
    let e = 0x16;
    let entry = (
        wstr_at(&list, e + 0x46, 33),
        u32_at(&list, e + 0x88),
        u32_at(&list, e + 0x90),
    );
    (entry, account, a)
}

/// Lobby session in channel 1, with the room created and its max-player setting probed.
fn probe_max_players(auth_port: u16, lobby_port: u16, probes: &[u32]) -> Vec<bool> {
    let (_, account, mut a) = login(auth_port, "probe");
    a.send(0x20a, |w| {
        w.u32(1);
    });
    let r = a.recv(0x20b);
    let key = r[0xe..0x1e].to_vec();
    let mut l = Conn::open(lobby_port);
    l.send(0x300, |w| {
        w.bytes(&key).u32(account);
    });
    assert_eq!(u32_at(&l.recv(0x301), 6), 1, "lobby login");
    l.send(0x372, |w| {
        w.u32(account).u32(1).wstr("Channel 1", 32);
    });
    assert_eq!(u32_at(&l.recv(0x373), 6), 1, "channel enter");
    l.send(0x382, |w| {
        w.i32(8).wstr("Room", 33).wstr("", 9).i32(0);
    });
    assert_eq!(u32_at(&l.recv(0x383), 6), 1, "room created");
    probes
        .iter()
        .map(|v| {
            l.send(0x38c, |w| {
                w.u32(0).u32(6).u32(*v);
            });
            u32_at(&l.recv(0x38d), 6) == 1
        })
        .collect()
}

#[test]
fn toml_rule_override_reaches_the_running_server() {
    let dir = temp("startup");
    let _guard = Dir(dir.clone());
    let (auth, lobby, clan, web) = (free_port(), free_port(), free_port(), free_port());
    let cfg = dir.join("server.toml");
    std::fs::write(
        &cfg,
        format!(
            "[server]\nname = \"Wiring Test\"\nauth_port = {auth}\nlobby_port = {lobby}\nclan_port = {clan}\nhttp_port = {web}\n\
             advertise_ip = \"10.9.8.7\"\n[room]\nmax_players_cap = 4\n[[lobby.servers]]\nname = \"One\"\ncapacity = 777\n"
        ),
    )
    .unwrap();
    let db = dir.join("ca.db");
    let _s = start(
        &[
            "--config",
            &cfg.display().to_string(),
            "--db",
            &db.display().to_string(),
        ],
        &[],
        auth,
    );
    let ((ip, port, cap), _, _a) = login(auth, "wire1");
    assert_eq!(
        (ip.as_str(), port, cap),
        ("10.9.8.7", lobby as u32, 777),
        "advertise_ip, lobby_port and lobby.servers[0].capacity from server.toml"
    );
    // room.max_players_cap = 4 from server.toml: the built-in cap is 16, so a 6 would be accepted without it
    assert_eq!(probe_max_players(auth, lobby, &[6, 4]), vec![false, true]);
}

#[test]
fn precedence_flag_then_env_then_toml_then_lobby_json_then_builtin() {
    let dir = temp("prec");
    let _guard = Dir(dir.clone());
    let (auth, lobby_cli, lobby_toml, clan, web) = (
        free_port(),
        free_port(),
        free_port(),
        free_port(),
        free_port(),
    );
    // lobby.json sits next to the DB: cap 3 and a key the toml does not mention
    std::fs::write(dir.join("lobby.json"), r#"{"room.max_players_cap": 3}"#).unwrap();
    let cfg = dir.join("server.toml");
    std::fs::write(
        &cfg,
        format!(
            "[server]\nauth_port = {auth}\nlobby_port = {lobby_toml}\nclan_port = {clan}\nhttp_port = {web}\nadvertise_ip = \"10.0.0.1\"\n\
             [room]\nmax_players_cap = 4\n"
        ),
    )
    .unwrap();
    let db = dir.join("ca.db");
    let cfgs = cfg.display().to_string();
    let dbs = db.display().to_string();
    let lobby_cli_s = lobby_cli.to_string();
    // flag (--lobbyport) beats toml (lobby_port); env (advertise_ip, max_players_cap) beats toml;
    // toml beats lobby.json (cap 4 would be 3); the DB path flag beats the default
    let s = start(
        &["--config", &cfgs, "--db", &dbs, "--lobbyport", &lobby_cli_s],
        &[
            ("DC_SERVER__ADVERTISE_IP", "10.0.0.2"),
            ("DC_ROOM__MAX_PLAYERS_CAP", "5"),
        ],
        auth,
    );
    let ((ip, port, cap), _, _a) = login(auth, "prec1");
    assert_eq!(
        (ip.as_str(), port, cap),
        ("10.0.0.2", lobby_cli as u32, 1000),
        "env > toml for the address, flag > toml for the port, built-in capacity"
    );
    assert_eq!(
        probe_max_players(auth, lobby_cli, &[6, 5]),
        vec![false, true],
        "env 5 > toml 4 > lobby.json 3"
    );
    drop(s);
    // same files, no env: toml (4) beats lobby.json (3)
    let auth2 = free_port();
    let cfg2 = dir.join("server2.toml");
    let (l2, c2, w2) = (free_port(), free_port(), free_port());
    std::fs::write(
        &cfg2,
        format!("[server]\nauth_port = {auth2}\nlobby_port = {l2}\nclan_port = {c2}\nhttp_port = {w2}\n[room]\nmax_players_cap = 4\n"),
    )
    .unwrap();
    let _s2 = start(
        &["--config", &cfg2.display().to_string(), "--db", &dbs],
        &[],
        auth2,
    );
    assert_eq!(
        probe_max_players(auth2, l2, &[5, 4]),
        vec![false, true],
        "toml 4 > lobby.json 3"
    );
}

/// LG-316: the client's report frame over a real lobby socket, stored and handed to the mail (dry run).
#[test]
fn a_report_over_the_socket_is_stored_and_mailed() {
    let dir = temp("report");
    let _guard = Dir(dir.clone());
    let (auth, lobby, clan, web) = (free_port(), free_port(), free_port(), free_port());
    let db = dir.join("ca.db");
    let _s = start(
        &[
            "--port",
            &auth.to_string(),
            "--lobbyport",
            &lobby.to_string(),
            "--clanport",
            &clan.to_string(),
            "--clanweb",
            &web.to_string(),
            "--db",
            db.to_str().unwrap(),
            "--mail-dry-run",
            "--mail-reports-to",
            "owner@test.invalid",
        ],
        &[],
        auth,
    );
    let (_, account, mut a) = login(auth, "repo");
    a.send(0x20a, |w| {
        w.u32(1);
    });
    let r = a.recv(0x20b);
    let key = r[0xe..0x1e].to_vec();
    let mut l = Conn::open(lobby);
    l.send(0x300, |w| {
        w.bytes(&key).u32(account);
    });
    assert_eq!(u32_at(&l.recv(0x301), 6), 1, "lobby login");
    let p = dc_wire::report::packet("", "the shop freezes", 1, 2).into_payload();
    let mut f = FrameHeader8 {
        id: 0x312,
        size: p.len() as i32,
    }
    .to_bytes()
    .to_vec();
    f.extend_from_slice(&p);
    l.0.write_all(&f).unwrap();
    let c = rusqlite::Connection::open(&db).unwrap();
    let t = Instant::now();
    loop {
        let row: Option<(String, i64, i64, i64, String)> = c
            .query_row(
                "SELECT text, category, sub, mailed, reporter_nick FROM reports",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .ok();
        if let Some(row) = row.clone().filter(|r| r.3 == 1) {
            assert_eq!(row, ("the shop freezes".into(), 1, 2, 1, "repo".into()));
            return;
        }
        assert!(
            t.elapsed() < Duration::from_secs(40),
            "no mailed row: {row:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn lobby_json_alone_and_no_config_behave_as_before() {
    let dir = temp("legacy");
    let _guard = Dir(dir.clone());
    let (auth, lobby, clan, web) = (free_port(), free_port(), free_port(), free_port());
    std::fs::write(dir.join("lobby.json"), r#"{"room.max_players_cap": 3}"#).unwrap();
    let db = dir.join("ca.db");
    let _s = start(
        &[
            "--port",
            &auth.to_string(),
            "--lobbyport",
            &lobby.to_string(),
            "--clanport",
            &clan.to_string(),
            "--clanweb",
            &web.to_string(),
            "--db",
            &db.display().to_string(),
        ],
        &[],
        auth,
    );
    let ((ip, port, cap), _, _a) = login(auth, "old1");
    assert_eq!((ip.as_str(), port, cap), ("127.0.0.1", lobby as u32, 1000));
    assert_eq!(
        probe_max_players(auth, lobby, &[4, 3]),
        vec![false, true],
        "lobby.json still works without a config"
    );
}

// ------------------------------------------------------------------ mods

fn write_mod(dir: &Path, id: &str) {
    let m = dir.join(id);
    std::fs::create_dir_all(&m).unwrap();
    std::fs::write(
        m.join("manifest.json"),
        format!(
            r#"{{"id": "{id}", "name": "Wiring Mod", "version": "1.0.0", "entry": "main.lua",
       "kind": "server_rules", "description": "A test fixture mod for the unit tests.", "author": "tests", "capabilities": ["hooks.lobby"]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        m.join("main.lua"),
        "return function(mod)\n    mod.hooks:wrap(\"lobby.calc_exp_gp\", function(base, ctx)\n        return base(ctx)\n    end)\nend\n",
    )
    .unwrap();
}

fn wait_for(path: &Path, needle: &str) -> String {
    let t = Instant::now();
    loop {
        let s = std::fs::read_to_string(path).unwrap_or_default();
        // DC test tooling: a long wait (60 s) that ends as soon as the line is there
        if s.contains(needle) || t.elapsed() > Duration::from_secs(60) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn vanilla_policy_refuses_mods_and_publishes_the_empty_set_hash() {
    let dir = temp("vanilla");
    let _guard = Dir(dir.clone());
    let (auth, lobby, clan, web) = (free_port(), free_port(), free_port(), free_port());
    let mods = dir.join("mods");
    write_mod(&mods, "wiring_mod");
    let cfg = dir.join("server.toml");
    std::fs::write(
        &cfg,
        format!(
            "[server]\nauth_port = {auth}\nlobby_port = {lobby}\nclan_port = {clan}\nhttp_port = {web}\n[paths]\nmods_dir = \"{}\"\n[mods]\npolicy = \"vanilla\"\n",
            toml_path(&mods)
        ),
    )
    .unwrap();
    let log = dir.join("lobby.log");
    let _s = start(
        &[
            "--config",
            &cfg.display().to_string(),
            "--db",
            &dir.join("ca.db").display().to_string(),
            "--log",
            &log.display().to_string(),
        ],
        &[],
        auth,
    );
    let text = wait_for(&log, "mod-set hash");
    assert!(
        text.contains("refused") && text.contains("vanilla"),
        "vanilla policy refuses the load:\n{text}"
    );
    assert!(text.contains("mods: 0 loaded"), "{text}");
    let list = std::fs::read_to_string(dir.join("server_list.json")).expect("server-list data");
    assert!(
        list.contains("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        "{list}"
    );
    assert!(list.contains("\"policy\": \"vanilla\""), "{list}");
}

#[test]
fn modded_policy_loads_the_set_and_publishes_its_hash() {
    let dir = temp("modded");
    let _guard = Dir(dir.clone());
    let (auth, lobby, clan, web) = (free_port(), free_port(), free_port(), free_port());
    let mods = dir.join("mods");
    write_mod(&mods, "wiring_mod");
    // LG-270: a folder that is not declared stays unloaded
    write_mod(&mods, "undeclared_mod");
    let (_, entry) = dc_modkit::pack_mod_bytes(&mods.join("wiring_mod"))
        .unwrap_or_else(|e| panic!("pack: {e:?}"));
    let cfg = dir.join("server.toml");
    std::fs::write(
        &cfg,
        format!(
            "[server]\nauth_port = {auth}\nlobby_port = {lobby}\nclan_port = {clan}\nhttp_port = {web}\n[paths]\nmods_dir = \"{}\"\n\
             [mods]\npolicy = \"modded\"\n[[mods.set]]\nid = \"wiring_mod\"\nversion = \"1.0.0\"\nsha256 = \"{}\"\n",
            toml_path(&mods),
            entry.sha256
        ),
    )
    .unwrap();
    let log = dir.join("lobby.log");
    let _s = start(
        &[
            "--config",
            &cfg.display().to_string(),
            "--db",
            &dir.join("ca.db").display().to_string(),
            "--log",
            &log.display().to_string(),
        ],
        &[],
        auth,
    );
    let text = wait_for(&log, "loaded in the declared order");
    assert!(
        text.contains("mods: 1 loaded in the declared order: [\"wiring_mod\"]"),
        "{text}"
    );
    assert!(!text.contains("ERROR"), "{text}");
    let want = dc_rom::sha256_hex(format!("wiring_mod@1.0.0:{}\n", entry.sha256).as_bytes());
    let list = std::fs::read_to_string(dir.join("server_list.json")).expect("server-list data");
    assert!(list.contains(&want), "hash {want} not in\n{list}");
    assert!(
        list.contains("\"policy\": \"modded\"") && list.contains("\"kind\": \"server_rules\""),
        "{list}"
    );
}

// ------------------------------------------------------------------ doctor

fn run_doctor(args: &[String], env: &[(&str, &str)]) -> (i32, String) {
    let mut c = command(args, env);
    let out = c.stdin(Stdio::null()).output().expect("run doctor");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn doctor_args(cfg: &Path, db: &Path, ports: (u16, u16, u16, u16)) -> Vec<String> {
    vec![
        "doctor".into(),
        "--config".into(),
        cfg.display().to_string(),
        "--db".into(),
        db.display().to_string(),
        "--port".into(),
        ports.0.to_string(),
        "--lobbyport".into(),
        ports.1.to_string(),
        "--clanport".into(),
        ports.2.to_string(),
        "--clanweb".into(),
        ports.3.to_string(),
    ]
}

#[test]
fn doctor_prints_the_table_and_exits_zero_on_a_good_setup() {
    let dir = temp("doctor_ok");
    let _guard = Dir(dir.clone());
    let ports = (free_port(), free_port(), free_port(), free_port());
    let mods = dir.join("mods");
    write_mod(&mods, "wiring_mod");
    let (_, entry) = dc_modkit::pack_mod_bytes(&mods.join("wiring_mod"))
        .unwrap_or_else(|e| panic!("pack: {e:?}"));
    let cfg = dir.join("server.toml");
    std::fs::write(
        &cfg,
        format!(
            "[paths]\nmods_dir = \"{}\"\n[mods]\npolicy = \"modded\"\n[[mods.set]]\nid = \"wiring_mod\"\nversion = \"1.0.0\"\nsha256 = \"{}\"\n",
            toml_path(&mods),
            entry.sha256
        ),
    )
    .unwrap();
    let rom = dc_rom::test_rom();
    let rom_s = rom.as_ref().map(|p| p.display().to_string());
    let env: Vec<(&str, &str)> = rom_s.iter().map(|s| ("CA_ROM", s.as_str())).collect();
    let (code, out) = run_doctor(&doctor_args(&cfg, &dir.join("ca.db"), ports), &env);
    assert_eq!(code, 0, "{out}");
    for needle in [
        "CHECK",
        "STAT",
        "DETAIL",
        "config",
        "lobby.json",
        "port auth",
        "port lobby",
        "rom",
        "database",
        "mod wiring_mod",
        "mod set",
        "doctor:",
    ] {
        assert!(out.contains(needle), "missing '{needle}' in\n{out}");
    }
    assert!(out.contains("0 failures"), "{out}");
    assert!(
        out.contains(&dc_rom::sha256_hex(
            format!("wiring_mod@1.0.0:{}\n", entry.sha256).as_bytes()
        )),
        "mod-set hash in the table:\n{out}"
    );
    assert!(!dir.join("ca.db").exists(), "doctor creates no database");
    if rom.is_some() {
        assert!(out.contains("build"), "ROM identified:\n{out}");
    } else {
        println!("skipped ROM identification: CA_ROM unset");
    }
}

#[test]
fn doctor_exits_one_and_names_the_failures() {
    let dir = temp("doctor_bad");
    let _guard = Dir(dir.clone());
    let ports = (free_port(), free_port(), free_port(), free_port());
    let mods = dir.join("mods");
    std::fs::create_dir_all(mods.join("broken")).unwrap();
    std::fs::write(
        mods.join("broken").join("manifest.json"),
        "{\"id\": \"broken\"}",
    )
    .unwrap();
    let cfg = dir.join("server.toml");
    // modded with a declared mod that the folder cannot back, a bad enum value, and a broken mod folder
    std::fs::write(
        &cfg,
        format!(
            "[paths]\nmods_dir = \"{}\"\n[gm]\ntest_commands = \"nobody\"\n[mods]\npolicy = \"modded\"\n[[mods.set]]\nid = \"ghost\"\nversion = \"1.0.0\"\nsha256 = \"{}\"\n",
            toml_path(&mods),
            "ab".repeat(32)
        ),
    )
    .unwrap();
    // an unusable DB file as well
    std::fs::write(dir.join("ca.db"), b"definitely not a sqlite database file").unwrap();
    let (code, out) = run_doctor(&doctor_args(&cfg, &dir.join("ca.db"), ports), &[]);
    assert_eq!(code, 1, "{out}");
    for needle in [
        "FAIL",
        "gm.test_commands",
        "database",
        "mod set",
        "ghost",
        "mod broken",
    ] {
        assert!(out.contains(needle), "missing '{needle}' in\n{out}");
    }
    // a config that does not parse at all is a FAIL row too, not a crash
    let bad = dir.join("bad.toml");
    std::fs::write(&bad, "[server\n").unwrap();
    let (code, out) = run_doctor(&doctor_args(&bad, &dir.join("x.db"), ports), &[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("config") && out.contains("FAIL"), "{out}");
}

#[test]
fn a_bad_config_stops_the_server_with_status_two() {
    let dir = temp("badcfg");
    let _guard = Dir(dir.clone());
    let cfg = dir.join("server.toml");
    std::fs::write(&cfg, "[room]\nmax_players_cap = 999\n").unwrap();
    let args = vec![
        "--config".to_string(),
        cfg.display().to_string(),
        "--db".to_string(),
        dir.join("ca.db").display().to_string(),
    ];
    let out = command(&args, &[]).stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("room.max_players_cap"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_declared_mod_with_the_wrong_hash_stops_the_server() {
    // LG-270: the lobby refuses to start rather than run a set its clients could not join
    let dir = temp("badhash");
    let _guard = Dir(dir.clone());
    let mods = dir.join("mods");
    write_mod(&mods, "wiring_mod");
    let cfg = dir.join("server.toml");
    std::fs::write(
        &cfg,
        format!(
            // no [server] ports: the start stops before any listener binds
            "[paths]\nmods_dir = \"{}\"\n\
             [mods]\npolicy = \"modded\"\n[[mods.set]]\nid = \"wiring_mod\"\nversion = \"1.0.0\"\nsha256 = \"{}\"\n",
            toml_path(&mods),
            "0".repeat(64)
        ),
    )
    .unwrap();
    let args = vec![
        "--config".to_string(),
        cfg.display().to_string(),
        "--db".to_string(),
        dir.join("ca.db").display().to_string(),
    ];
    let out = command(&args, &[]).stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("LG-270") && err.contains("packs to"), "{err}");
}

/// MOD-25: a vanilla lobby with a catalogue stays vanilla: the empty set hash, nothing loaded; the optional mod is only
/// served.
#[test]
fn a_vanilla_lobby_with_a_catalogue_keeps_the_empty_set_hash() {
    let dir = temp("vanilla_catalog");
    let _guard = Dir(dir.clone());
    let (auth, lobby, clan, web) = (free_port(), free_port(), free_port(), free_port());
    let mods = dir.join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/drawn_crosshair");
    let (bytes, e) = dc_modkit::pack_mod_bytes(&src).unwrap();
    std::fs::write(mods.join(format!("{}.zip", e.sha256)), &bytes).unwrap();
    let cfg = dir.join("server.toml");
    std::fs::write(
        &cfg,
        format!(
            "[server]\nauth_port = {auth}\n\
             lobby_port = {lobby}\nclan_port = {clan}\nhttp_port = {web}\n[paths]\nmods_dir = \"{}\"\n[mods]\npolicy = \"vanilla\"\n[[mods.catalog]]\nid = \"{}\"\nversion = \"{}\"\nsha256 = \"{}\"\n",
            toml_path(&mods), e.id, e.version, e.sha256
        ),
    )
    .unwrap();
    let log = dir.join("lobby.log");
    let _s = start(
        &[
            "--config",
            &cfg.display().to_string(),
            "--db",
            &dir.join("ca.db").display().to_string(),
            "--log",
            &log.display().to_string(),
        ],
        &[],
        auth,
    );
    let text = wait_for(&log, "mod-set hash");
    assert!(
        !text.contains("mods: catalog:"),
        "the entry is offered:\n{text}"
    );
    let list = std::fs::read_to_string(dir.join("server_list.json")).expect("server-list data");
    assert!(
        list.contains("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        "{list}"
    );
    assert!(
        list.contains("\"policy\": \"vanilla\"") && !list.contains("drawn_crosshair"),
        "{list}"
    );
}
