//! Shared tooling of the lobby process tests: the lobby binary on ephemeral loopback ports with a temporary DB,
//! scripted 2009-protocol TCP clients (frames through `dc-wire`), the stand-in dedicated server compiled from
//! `fake_dserver.rs`, the location of the `dc-dserver` binary, and ROM expectations through `dc-bute`.
//! Every process a test starts is stopped by its PID (child handle or `taskkill /PID`) when the guard drops.
// HARNESS: lobby_harness (C, login, lobby: the same message sequence and the same asserts)
#![allow(dead_code)]

use dc_wire::tcp::{parse_tcp_frames, PktWriter};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------------------------------------ helpers

pub fn u32_at(d: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}

pub fn u16_at(d: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([d[off], d[off + 1]])
}

/// UTF-16LE text of a fixed `wstr` field (stops at the first NUL).
pub fn wstr_at(d: &[u8], off: usize, chars: usize) -> String {
    let mut units = Vec::new();
    for i in 0..chars {
        let o = off + 2 * i;
        if o + 1 >= d.len() {
            break;
        }
        let u = u16_at(d, o);
        if u == 0 {
            break;
        }
        units.push(u);
    }
    String::from_utf16_lossy(&units)
}

/// A free TCP port on 127.0.0.1 (bind 0, read, close).
pub fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A free UDP port on 127.0.0.1 (bind 0, read, close).
pub fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Distinct free UDP ports.
pub fn free_udp_ports(n: usize) -> Vec<u16> {
    let socks: Vec<UdpSocket> = (0..n)
        .map(|_| UdpSocket::bind("127.0.0.1:0").unwrap())
        .collect();
    socks
        .iter()
        .map(|s| s.local_addr().unwrap().port())
        .collect()
}

/// True when a UDP bind on 127.0.0.1 succeeds.
pub fn udp_free(port: u16) -> bool {
    UdpSocket::bind(("127.0.0.1", port)).is_ok()
}

pub fn scratch_dir(tag: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = base.join(format!("lobby-{tag}-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Stops a process by PID (Windows `taskkill /F /PID`); never by image name.
pub fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
}

/// True while a process with this PID runs.
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        out.contains(&format!("\"{pid}\""))
    }
    #[cfg(not(windows))]
    {
        // `kill -0` asks the kernel without a signal; macOS has no /proc
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

pub fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    loop {
        if f() {
            return true;
        }
        if Instant::now() > end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ------------------------------------------------------------------------------------------------ binaries

pub fn lobby_exe() -> PathBuf {
    if let Some(p) = std::env::var_os("DC_LOBBY_EXE") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_BIN_EXE_dc-lobby-server"))
}

fn exe_name(stem: &str) -> String {
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}

/// The `dc-dserver` binary: `DC_DSERVER_EXE`, else the one next to the lobby binary (cargo puts every workspace
/// binary in the same profile directory), else it is built with the same cargo into the same target directory.
pub fn dserver_exe() -> PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| {
        if let Ok(p) = std::env::var("DC_DSERVER_EXE") {
            if !p.trim().is_empty() {
                return PathBuf::from(p);
            }
        }
        let sibling = lobby_exe().with_file_name(exe_name("dc-dserver"));
        if sibling.is_file() {
            return sibling;
        }
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "-p", "dc-dserver", "--bin", "dc-dserver"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .expect("run cargo build -p dc-dserver");
        assert!(status.success(), "cargo build -p dc-dserver failed");
        assert!(
            sibling.is_file(),
            "dc-dserver not found at {}",
            sibling.display()
        );
        sibling
    })
    .clone()
}

/// The stand-in server, compiled from `fake_dserver.rs` with the toolchain's `rustc` once per source hash.
pub fn fake_dserver_exe() -> PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| {
        let src = include_str!("fake_dserver.rs");
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in src.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake-dserver");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join(exe_name(&format!("fake_dserver_{h:016x}")));
        if out.is_file() {
            return out;
        }
        let rustc = std::env::var("CARGO")
            .ok()
            .map(|c| Path::new(&c).with_file_name(exe_name("rustc")))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("rustc"));
        let tmp_dir = dir.join(format!("build-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let src_path = tmp_dir.join("fake_dserver.rs");
        std::fs::write(&src_path, src).unwrap();
        let tmp_out = tmp_dir.join(exe_name("fake_dserver"));
        let o = Command::new(&rustc)
            .args(["--edition", "2021", "-C", "debuginfo=0", "-o"])
            .arg(&tmp_out)
            .arg(&src_path)
            .output()
            .expect("run rustc");
        assert!(
            o.status.success(),
            "rustc fake_dserver.rs failed: {}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        // another test binary may have won the race; either copy is the same program
        if std::fs::rename(&tmp_out, &out).is_err() && !out.is_file() {
            return tmp_out;
        }
        out
    })
    .clone()
}

/// What the stand-in recorded: one entry per launch (pid, arguments, control lines).
#[derive(Debug, Clone)]
pub struct FakeLaunch {
    pub pid: u32,
    pub args: Vec<String>,
    pub stdin: Vec<String>,
}

impl FakeLaunch {
    pub fn arg(&self, flag: &str) -> Option<&str> {
        self.args
            .iter()
            .position(|a| a == flag)
            .and_then(|i| self.args.get(i + 1))
            .map(String::as_str)
    }
    pub fn users(&self) -> Vec<Vec<String>> {
        self.stdin
            .iter()
            .filter(|l| l.starts_with("user "))
            .map(|l| l.split_whitespace().map(String::from).collect())
            .collect()
    }
    pub fn room_line(&self) -> Option<&str> {
        self.stdin
            .iter()
            .find(|l| l.starts_with("room"))
            .map(String::as_str)
    }
}

pub fn fake_launches(dir: &Path) -> Vec<FakeLaunch> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return v;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(pid) = name
            .strip_prefix("launch_")
            .and_then(|s| s.strip_suffix(".txt"))
            .and_then(|s| s.parse().ok())
        else {
            continue;
        };
        let args = std::fs::read_to_string(e.path())
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect();
        let stdin = std::fs::read_to_string(dir.join(format!("stdin_{pid}.txt")))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect();
        v.push(FakeLaunch { pid, args, stdin });
    }
    v
}

// ------------------------------------------------------------------------------------------------ the lobby

pub struct LobbyOpts {
    /// `--dserver-exe` (None: no `--dserver`).
    pub dserver_exe: Option<PathBuf>,
    pub dserver_world: Option<String>,
    pub dserver_idle: Option<u32>,
    pub gameport: u16,
    /// Written as `<db dir>/lobby.json`.
    pub lobby_json: Option<String>,
    /// Environment for the lobby (inherited by its game servers).
    pub env: Vec<(String, String)>,
    pub extra: Vec<String>,
    /// Do not pass `--dserver-logdir`: the lobby makes its own per-room working directory under the temp dir.
    pub no_logdir: bool,
}

impl Default for LobbyOpts {
    fn default() -> Self {
        LobbyOpts {
            dserver_exe: None,
            dserver_world: None,
            dserver_idle: None,
            gameport: free_udp_port(),
            lobby_json: None,
            env: Vec::new(),
            extra: Vec::new(),
            no_logdir: false,
        }
    }
}

pub struct Lobby {
    pub child: Option<Child>,
    pub port: u16,
    pub lport: u16,
    /// The clan web port (`--clanweb`): `/server_list.json` and the mod packages.
    pub web: u16,
    pub gameport: u16,
    pub dir: PathBuf,
    pub db: PathBuf,
    pub log: PathBuf,
    /// Directory the stand-in records into (`DC_FAKE_DIR`).
    pub fake_dir: PathBuf,
    /// PIDs to stop on drop (game servers the test learned about).
    pub extra_pids: std::cell::RefCell<BTreeSet<u32>>,
}

impl Drop for Lobby {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        for l in fake_launches(&self.fake_dir) {
            self.extra_pids.borrow_mut().insert(l.pid);
        }
        for pid in self.game_pids() {
            self.extra_pids.borrow_mut().insert(pid);
        }
        for pid in self.extra_pids.borrow().iter() {
            if pid_alive(*pid) {
                kill_pid(*pid);
            }
        }
        // keep the logs and the DB of a failed test for inspection; remove them otherwise
        if !std::thread::panicking() && std::env::var_os("DC_KEEP_TEST_DIRS").is_none() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

impl Lobby {
    pub fn start(tag: &str, o: LobbyOpts) -> Lobby {
        let dir = scratch_dir(tag);
        let db = dir.join("lobby.db");
        let log = dir.join("server.log");
        let fake_dir = dir.join("fake");
        std::fs::create_dir_all(&fake_dir).unwrap();
        // A small bot pool unless the test sets one: the default 100 pool characters per lobby, times the lobbies the
        // tests of one binary start at once, stalled a shared CI runner's disk for over 120 s (linux servers lane).
        let json = match &o.lobby_json {
            Some(j) if j.contains("bots.pool_size") => j.clone(),
            Some(j) => match j.trim().strip_prefix('{') {
                Some(rest) if rest.trim() == "}" => r#"{"bots.pool_size": 4}"#.to_string(),
                Some(rest) => format!(r#"{{"bots.pool_size": 4, {rest}"#),
                None => j.clone(),
            },
            None => r#"{"bots.pool_size": 4}"#.to_string(),
        };
        std::fs::write(dir.join("lobby.json"), json).unwrap();
        // every listener on its own port; the clan lobby must not be lobbyport + 1 of another listener
        let (port, lport, cport, web) = loop {
            let p = (
                free_tcp_port(),
                free_tcp_port(),
                free_tcp_port(),
                free_tcp_port(),
            );
            let set: BTreeSet<u16> = [p.0, p.1, p.2, p.3, p.1.wrapping_add(1)]
                .into_iter()
                .collect();
            if set.len() == 5 {
                break p;
            }
        };
        let mut cmd = Command::new(lobby_exe());
        cmd.args([
            "--port",
            &port.to_string(),
            "--lobbyport",
            &lport.to_string(),
        ])
        .args([
            "--clanport",
            &cport.to_string(),
            "--clanweb",
            &web.to_string(),
        ])
        .arg("--db")
        .arg(&db)
        .args([
            "--countdown",
            "0.05",
            "--start-nx",
            "5000",
            "--gameport",
            &o.gameport.to_string(),
        ])
        .arg("--log")
        .arg(&log);
        if let Some(exe) = &o.dserver_exe {
            cmd.arg("--dserver").arg("--dserver-exe").arg(exe);
            if !o.no_logdir {
                cmd.arg("--dserver-logdir").arg(&dir);
            }
            cmd.arg("--dserver-verbose");
        }
        if let Some(w) = &o.dserver_world {
            cmd.args(["--dserver-world", w]);
        }
        if let Some(i) = o.dserver_idle {
            cmd.args(["--dserver-idle", &i.to_string()]);
        }
        cmd.args(&o.extra);
        cmd.env("DC_FAKE_DIR", &fake_dir);
        for (k, v) in &o.env {
            cmd.env(k, v);
        }
        cmd.current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = cmd.spawn().expect("spawn dc-lobby-server");
        let l = Lobby {
            child: Some(child),
            port,
            lport,
            web,
            gameport: o.gameport,
            dir,
            db,
            log,
            fake_dir,
            extra_pids: Default::default(),
        };
        // 120 s: a shared CI runner starting several debug lobbies at once (one per test) took over 30 s to listen
        let up = wait_until(Duration::from_secs(120), || {
            TcpStream::connect(("127.0.0.1", lport)).is_ok()
                && TcpStream::connect(("127.0.0.1", port)).is_ok()
        });
        assert!(up, "lobby did not listen; log:\n{}", l.log_text());
        l
    }

    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    pub fn log_has(&self, needle: &str) -> bool {
        self.log_text().contains(needle)
    }

    pub fn wait_log(&self, needle: &str, timeout: Duration) -> bool {
        wait_until(timeout, || self.log_has(needle))
    }

    /// PIDs of the game servers the lobby reported starting (`game N: room C-R server pid P ...`).
    pub fn game_pids(&self) -> Vec<u32> {
        self.log_text()
            .lines()
            .filter_map(|l| {
                let i = l.find(" server pid ")?;
                l[i + 12..].split_whitespace().next()?.parse().ok()
            })
            .collect()
    }

    pub fn set_fake_mode(&self, mode: &str) {
        std::fs::write(self.fake_dir.join("mode"), mode).unwrap();
    }

    pub fn fake(&self) -> Vec<FakeLaunch> {
        fake_launches(&self.fake_dir)
    }

    pub fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.db).unwrap()
    }

    pub fn user_no(&self, login: &str) -> u32 {
        self.db()
            .query_row(
                "SELECT user_no FROM accounts WHERE login=?1",
                [login],
                |r| r.get::<_, i64>(0),
            )
            .unwrap() as u32
    }

    /// (exp, gp, kills, deaths, wins, losses, headshots, matches) of a character.
    pub fn character(&self, user_no: u32) -> (i64, i64, i64, i64, i64, i64, i64, i64) {
        self.db()
            .query_row(
                "SELECT exp, gp, kills, deaths, wins, losses, headshots, matches FROM characters WHERE user_no=?1",
                [user_no as i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
            )
            .unwrap()
    }

    pub fn match_rows(&self) -> i64 {
        self.db()
            .query_row("SELECT COUNT(*) FROM match_log", [], |r| r.get(0))
            .unwrap()
    }

    /// Text of every per-room game-server log in the work directory.
    pub fn dserver_logs(&self) -> String {
        let mut s = String::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            let mut names: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().starts_with("dserver_"))
                        .unwrap_or(false)
                })
                .collect();
            names.sort();
            for p in names {
                s.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
            }
        }
        s
    }
}

// ------------------------------------------------------------------------------------------------ TCP client

pub struct Client {
    pub name: String,
    s: TcpStream,
    buf: Vec<u8>,
    pub got: Vec<(i32, Vec<u8>)>,
}

/// Builds a frame body: `fill` writes the fields after the inner header.
pub fn frame(id: u16, fill: impl FnOnce(&mut PktWriter)) -> Vec<u8> {
    let mut w = PktWriter::new(id);
    fill(&mut w);
    w.to_frame()
}

impl Client {
    pub fn connect(port: u16, name: &str) -> Client {
        let s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        s.set_nodelay(true).ok();
        Client {
            name: name.into(),
            s,
            buf: Vec::new(),
            got: Vec::new(),
        }
    }

    pub fn send_frame(&mut self, f: &[u8]) {
        self.s.write_all(f).unwrap();
    }

    pub fn send(&mut self, id: u16, fill: impl FnOnce(&mut PktWriter)) {
        let f = frame(id, fill);
        self.send_frame(&f);
    }

    fn pump(&mut self) -> bool {
        let mut b = [0u8; 65536];
        match self.s.read(&mut b) {
            Ok(0) => false,
            Ok(n) => {
                self.buf.extend_from_slice(&b[..n]);
                let (frames, used) = parse_tcp_frames(&self.buf).expect("frames");
                self.buf.drain(..used);
                self.got.extend(frames);
                true
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                true
            }
            Err(_) => false,
        }
    }

    /// The first frame `id` matching `pred`; the frames before it are dropped (like the harness's `recv_until`).
    pub fn recv_where(&mut self, id: u16, t: Duration, pred: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        let end = Instant::now() + t;
        loop {
            if let Some(i) = self
                .got
                .iter()
                .position(|(f, d)| *f == id as i32 && pred(d))
            {
                let d = self.got[i].1.clone();
                self.got.drain(..=i);
                return d;
            }
            if Instant::now() > end || !self.pump() {
                let have: Vec<String> = self.got.iter().map(|(f, _)| format!("0x{f:x}")).collect();
                panic!("{}: no 0x{id:x} within {t:?}; have {have:?}", self.name);
            }
        }
    }

    pub fn recv_until(&mut self, id: u16, t: Duration) -> Vec<u8> {
        self.recv_where(id, t, |_| true)
    }

    pub fn recv(&mut self, id: u16) -> Vec<u8> {
        self.recv_until(id, Duration::from_secs(5))
    }

    /// Reads for `t` and reports whether a frame `id` arrived (kept in `got`).
    pub fn saw_within(&mut self, id: u16, t: Duration) -> bool {
        let end = Instant::now() + t;
        while Instant::now() < end {
            if self.got.iter().any(|(f, _)| *f == id as i32) {
                return true;
            }
            if !self.pump() {
                break;
            }
        }
        self.got.iter().any(|(f, _)| *f == id as i32)
    }
}

/// Auth server: version check, login (new account: nickname check + character create), server list, server
/// select; returns the lobby connection and the session key.
// HARNESS: lobby_harness::login
pub fn login(l: &Lobby, user: &str, nick: &str) -> (Client, Vec<u8>) {
    login_with(l, user, nick, "")
}

/// [`login`] with a suffix on the login-name field: the mod-set and content hashes a modded lobby requires
/// (`dc_wire::loginmods::suffix`, LG-271).
pub fn login_with(l: &Lobby, user: &str, nick: &str, name_suffix: &str) -> (Client, Vec<u8>) {
    let mut a = Client::connect(l.port, user);
    a.send(0x206, |w| {
        w.pad(32);
    });
    assert_eq!(u32_at(&a.recv(0x207), 6), 1);
    a.send(0x200, |w| {
        w.wstr(&format!("LOCAL:{user}{name_suffix}"), 4096)
            .wstr("pw", 17)
            .wstr("00-11-22-33-44-55", 20)
            .i32(0);
    });
    let d = a.recv(0x201);
    if u32_at(&d, 6) == 0 {
        assert_eq!(u32_at(&d, 0xa), 0x0003001e);
        a.send(0x204, |w| {
            w.wstr(nick, 17);
        });
        assert_eq!(u32_at(&a.recv(0x205), 6), 1);
        a.send(0x202, |w| {
            w.i32(6002).wstr(nick, 17);
        });
        assert_eq!(u32_at(&a.recv(0x203), 6), 1);
    }
    let sl = a.recv(0x2000);
    assert_eq!(sl.len(), 0xf16);
    assert_eq!(
        u32_at(&sl, 0x16 + 0x88),
        l.lport as u32,
        "lobby port in the server list"
    );
    a.send(0x20a, |w| {
        w.u32(1);
    });
    let d = a.recv(0x20b);
    assert_eq!(u32_at(&d, 6), 1);
    let key = d[0xe..0x1e].to_vec();
    drop(a);
    (Client::connect(l.lport, user), key)
}

/// Lobby: character login, channel list, character info, equipment, inventory, channel enter, lobby list.
// HARNESS: lobby_harness::lobby
pub fn enter_lobby(b: &mut Client, key: &[u8], user_no: u32) -> String {
    let k = key.to_vec();
    b.send(0x300, |w| {
        w.raw(&k).u32(user_no);
    });
    assert_eq!(u32_at(&b.recv(0x301), 6), 1, "char login refused");
    b.send(0x310, |w| {
        w.u32(0).u32(0);
    });
    let d = b.recv(0x311);
    assert!(d.len() == 0x10a && u32_at(&d, 0x16) > 0);
    b.send(0x342, |_| {});
    b.recv(0x343);
    b.send(0x302, |_| {});
    let d = b.recv(0x303);
    assert!(d.len() == 0x108 && u32_at(&d, 6) == 1);
    let nick = wstr_at(&d, 0x34, 17);
    b.send(0x322, |_| {});
    b.recv(0x323);
    b.send(0x320, |_| {});
    b.recv(0x321);
    b.send(0x372, |w| {
        w.u32(user_no).u32(1).wstr("Channel 1", 32);
    });
    assert_eq!(u32_at(&b.recv(0x373), 6), 1);
    b.send(0x380, |_| {});
    b.recv(0x381);
    b.send(0x370, |_| {});
    b.recv(0x371);
    nick
}

/// Logs in and enters channel 1; returns the client and its account number.
pub fn player(l: &Lobby, user: &str, nick: &str) -> (Client, u32) {
    player_with(l, user, nick, "")
}

/// [`player`] on a modded lobby: `name_suffix` carries the hashes (see [`login_with`]).
pub fn player_with(l: &Lobby, user: &str, nick: &str, name_suffix: &str) -> (Client, u32) {
    let (mut c, key) = login_with(l, user, nick, name_suffix);
    let n = l.user_no(user);
    let got = enter_lobby(&mut c, &key, n);
    assert_eq!(got, nick);
    (c, n)
}

/// 0x382 room create (max 8 players, no password, `mode`): returns the room number.
pub fn open_room(c: &mut Client, title: &str, mode: u32) -> u32 {
    c.send(0x382, |w| {
        w.i32(8).wstr(title, 33).wstr("", 9).i32(mode as i32);
    });
    let d = c.recv(0x383);
    assert_eq!(u32_at(&d, 6), 1, "room create refused");
    u32_at(&d, 0xe)
}

pub fn join_room(c: &mut Client, no: u32) {
    c.send(0x384, |w| {
        w.u32(no).wstr("", 9).wstr("", 65);
    });
    assert_eq!(u32_at(&c.recv(0x385), 6), 1, "room enter refused");
}

/// 0x38c room setting change (host, waiting room).
pub fn set_room(c: &mut Client, field: u32, value: u32) {
    c.send(0x38c, |w| {
        w.u32(0).u32(field).u32(value);
    });
    assert_eq!(
        u32_at(&c.recv(0x38d), 6),
        1,
        "setting {field} = {value} refused"
    );
}

/// 0x392 with state 1 (guest ready) or 2 (host start); the reply must be ok.
pub fn ready(c: &mut Client, state: u32) {
    c.send(0x392, |w| {
        w.u32(0).u32(state);
    });
    assert_eq!(
        u32_at(&c.recv(0x393), 6),
        1,
        "{}: ready/start {state} refused",
        c.name
    );
}

/// 0x394: back in the waiting room.
pub fn back_to_room(c: &mut Client) {
    c.send(0x394, |w| {
        w.u32(0);
    });
}

/// Waits for the game-start packet: (ip, port).
pub fn game_address(c: &mut Client, t: Duration) -> (String, u32) {
    let d = c.recv_until(0x3043, t);
    (wstr_at(&d, 6, 17), u32_at(&d, 0x28))
}

/// Room field ids of the 0x38c / 0x3035 pair.
// SPEC: PROTOCOL_2009.md §3.7 (0x38c / 0x3035 field numbers)
pub const FIELD_MAP: u32 = 3;
pub const FIELD_MODE: u32 = 4;
pub const FIELD_STATE: u32 = 7;

/// Waits for the room-state update to waiting (0x3035 field 7 value 0).
pub fn room_waiting(c: &mut Client, t: Duration) {
    c.recv_where(0x3035, t, |d| {
        u32_at(d, 0xa) == FIELD_STATE && u32_at(d, 0xe) == 0
    });
}

// ------------------------------------------------------------------------------------------------ ROM

pub struct Rom {
    pub dir: PathBuf,
    pub vfs: dc_bute::RezVfs,
    pub game_room: dc_bute::GameRoomData,
}

impl Rom {
    /// The user's ROM (CA_ROM) with a readable room table, or None (the caller skips).
    pub fn open() -> Option<Rom> {
        let dir = dc_rom::test_rom()?;
        let vfs = dc_bute::RezVfs::new(&dir);
        let game_room = dc_bute::GameRoomData::from_vfs(&vfs).ok()?;
        if game_room.maps().is_empty() || game_room.modes(false).is_empty() {
            return None;
        }
        Some(Rom {
            dir,
            vfs,
            game_room,
        })
    }

    pub fn mode_index(&self, key: &str) -> Option<u32> {
        self.game_room
            .modes(false)
            .iter()
            .position(|m| m.get("Key").map(|k| k == key).unwrap_or(false))
            .map(|i| i as u32)
    }

    pub fn default_map(&self, mode: u32) -> u32 {
        self.game_room.default_map(mode as usize, false).max(0) as u32
    }

    /// `[MapExp]` / `[MapMoney]` `level_correction` of the room's GMS (1.0 when absent).
    // DC-DEFAULT: CD-03 / CD-04 map factor from the ROM .gms
    pub fn factors(&self, map: u32, mode: u32) -> (f64, f64) {
        let (_m, g) = dc_bute::gms_file_with_vfs(
            &self.vfs,
            &self.game_room,
            map as usize,
            mode as usize,
            false,
        );
        (
            g.get_f64("MapExp", "level_correction", 1.0),
            g.get_f64("MapMoney", "level_correction", 1.0),
        )
    }

    pub fn world(&self, map: u32, mode: u32) -> String {
        dc_bute::world_for_room(
            &self.dir,
            &self.game_room,
            map as usize,
            mode as usize,
            false,
            None,
        )
    }

    /// The `room k=v ...` line the lobby should forward for these settings.
    pub fn room_line(&self, map: u32, mode: u32, goal: u32, ff: u32) -> String {
        let cfg = dc_bute::RoomConfig {
            map: map as usize,
            mode: mode as usize,
            clan: false,
            goal: goal as i32,
            ff: ff as i32,
            ..dc_bute::RoomConfig::default()
        };
        dc_bute::room_line_with_vfs(
            &self.vfs,
            &self.game_room,
            &cfg,
            &dc_bute::RoomOptions::default(),
        )
    }
}

/// Expected (exp, gp) of one match per the DeCombat rule.
// DC-DEFAULT: CD-03 (exp: finish 10, kill 4, win +25 %, objective 15), CD-04 (gp: finish 20, kill 6, win +25 %,
// objective 25), CD-54 (proposed: the lobby computes, reported numbers are advisory; round half away from zero)
pub fn expected_reward(kills: i64, won: bool, team_kills: i64, factors: (f64, f64)) -> (i64, i64) {
    let one = |finish: f64, kill: f64, win_pct: f64, f: f64| {
        let mut raw = finish + kill * kills as f64;
        if won {
            raw *= 1.0 + win_pct / 100.0;
        }
        raw -= kill * team_kills as f64;
        let f = if f.is_finite() && f > 0.0 { f } else { 1.0 };
        (raw * f).round().max(0.0) as i64
    };
    (
        one(10.0, 4.0, 25.0, factors.0),
        one(20.0, 6.0, 25.0, factors.1),
    )
}

/// Starting GP of a new character (the lobby's `--start-gp` default).
// SPEC: LOBBY_HARNESS.md §1 (--start-gp default 10000)
pub const START_GP: i64 = 10000;
