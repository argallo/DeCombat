//! Process tooling of the network tests: our `dc-dserver` (and `dc-lobby-server`) binaries on ephemeral loopback
//! ports, their stdout lines, and the made-up world file of `dc_client::testkit`. Every process a test starts is
//! stopped by its own child handle when the guard drops.
// SPEC: DSERVER_PIPE.md §2 (launch line), §3.1 (user line), §4 (events)
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn exe_name(stem: &str) -> String {
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}

/// A workspace binary: `env` when set, else the one next to this test's profile directory, built first with the
/// cargo that runs the test (same target directory and target) so that it is never a stale one.
pub fn workspace_bin(stem: &str, env: &str, package: &str) -> PathBuf {
    if let Ok(p) = std::env::var(env) {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    // <target>/<profile>/deps/<test>.exe -> <target>/<profile>/<stem>.exe
    let me = std::env::current_exe().expect("test exe");
    let dir = me.parent().and_then(Path::parent).expect("profile dir");
    let p = dir.join(exe_name(stem));
    ensure_fresh(stem, package, dir);
    assert!(p.is_file(), "{stem} not found at {}", p.display());
    p
}

/// Builds `package`'s binary `stem` once per test process (a no-op when it is current). `cargo test -p dc-client`
/// does not rebuild the binaries of other packages: a binary left in the target directory by an earlier build
/// (before a command line option the test passes existed) would be run as it is, and a bot that exits on its
/// unknown option makes a test fail minutes later with nothing to see.
fn ensure_fresh(stem: &str, package: &str, profile_dir: &Path) {
    static DONE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut done = DONE.lock().unwrap_or_else(|e| e.into_inner());
    if done.iter().any(|d| d == stem) {
        return;
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args(["build", "-p", package, "--bin", stem]);
    if profile_dir.file_name().is_some_and(|n| n == "release") {
        cmd.arg("--release");
    }
    let status = cmd
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("run cargo build");
    assert!(
        status.success(),
        "cargo build -p {package} --bin {stem} failed (a running copy of the binary blocks the rebuild?)"
    );
    done.push(stem.to_string());
}

pub fn dserver_exe() -> PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| workspace_bin("dc-dserver", "DC_DSERVER_EXE", "dc-dserver"))
        .clone()
}

pub fn lobby_exe() -> PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| workspace_bin("dc-lobby-server", "DC_LOBBY_EXE", "dc-lobby-server"))
        .clone()
}

/// A scratch directory under cargo's per-target temporary directory.
pub fn scratch(tag: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("client-net-{tag}-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The made-up world of `dc_client::testkit` written to `dir` (no ROM content).
pub fn synth_world_file(dir: &Path) -> PathBuf {
    let p = dir.join("synth.dat");
    std::fs::write(&p, dc_client::testkit::synth_world_bytes()).unwrap();
    p
}

/// A child process with its stdout lines; killed when dropped.
pub struct Proc {
    pub child: Option<Child>,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    pub seen: Vec<String>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Proc {
    pub fn spawn(cmd: Command) -> Proc {
        Proc::spawn_with_stderr(cmd, Stdio::null())
    }

    /// Like [`Proc::spawn`] with the process's stderr sent to `stderr` (a log file, for a test that must be able
    /// to say why a process ended).
    pub fn spawn_with_stderr(mut cmd: Command, stderr: Stdio) -> Proc {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .expect("spawn");
        let out = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines() {
                match l {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });
        Proc {
            stdin: child.stdin.take(),
            child: Some(child),
            lines: rx,
            seen: Vec::new(),
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Sends one control line. A process that is gone (closed pipe) fails the test with its exit status and its
    /// output instead of a broken-pipe panic.
    pub fn line(&mut self, l: &str) {
        if let Err(e) = self.try_line(l) {
            self.drain();
            let status = self
                .child
                .as_mut()
                .and_then(|c| c.try_wait().ok().flatten());
            panic!(
                "control line {l:?} not delivered ({e}); the process exit status {status:?}; it said {:?}",
                self.seen
            );
        }
    }

    /// Sends one control line; the write error when the process closed its input (exited).
    pub fn try_line(&mut self, l: &str) -> std::io::Result<()> {
        let s = self.stdin.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin already closed")
        })?;
        writeln!(s, "{l}")?;
        s.flush()
    }

    /// Asks the process to stop (`quit`). A process that already ended by itself (its round over, idle) is fine:
    /// the closed pipe is noted, not a failure.
    pub fn quit(&mut self) {
        if let Err(e) = self.try_line("quit") {
            println!("  (quit not sent: {e}; the process had already ended)");
        }
    }

    pub fn drain(&mut self) {
        while let Ok(l) = self.lines.try_recv() {
            self.seen.push(l);
        }
    }

    pub fn wait_line(&mut self, prefix: &str, t: Duration) -> Option<String> {
        let end = Instant::now() + t;
        loop {
            if let Some(l) = self.seen.iter().find(|l| l.starts_with(prefix)) {
                return Some(l.clone());
            }
            let left = end.checked_duration_since(Instant::now())?;
            match self.lines.recv_timeout(left.min(Duration::from_millis(50))) {
                Ok(l) => self.seen.push(l),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return self.seen.iter().find(|l| l.starts_with(prefix)).cloned()
                }
            }
        }
    }

    pub fn has(&self, part: &str) -> bool {
        self.seen.iter().any(|l| l.contains(part))
    }

    /// Waits up to `t` for the process to exit by itself; kills it otherwise. Returns its exit code.
    pub fn finish(&mut self, t: Duration) -> Option<i32> {
        self.stdin.take();
        let mut c = self.child.take()?;
        let end = Instant::now() + t;
        while Instant::now() < end {
            if let Ok(Some(s)) = c.try_wait() {
                return s.code();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = c.kill();
        let _ = c.wait();
        None
    }
}

/// A hard time limit for a test that drives other processes over the network: when the guard is still armed after
/// `limit`, it prints what it was waiting for, kills the processes it watches (by PID: the test's servers, and the
/// game servers a lobby log names) and ends the test process with exit code 101, so that a lost datagram or a dead
/// peer can never hang the run. Dropping the guard disarms it.
pub struct Watchdog {
    armed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pids: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
    logs: std::sync::Arc<std::sync::Mutex<Vec<PathBuf>>>,
}

impl Watchdog {
    pub fn arm(what: &str, limit: Duration) -> Watchdog {
        use std::sync::atomic::Ordering;
        let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let pids = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u32>::new()));
        let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::<PathBuf>::new()));
        let (a, p, l, what) = (armed.clone(), pids.clone(), logs.clone(), what.to_string());
        std::thread::spawn(move || {
            let end = Instant::now() + limit;
            while Instant::now() < end {
                if !a.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if !a.load(Ordering::Relaxed) {
                return;
            }
            let mut all: Vec<u32> = p.lock().map(|v| v.clone()).unwrap_or_default();
            for log in l.lock().map(|v| v.clone()).unwrap_or_default() {
                let text = std::fs::read_to_string(&log).unwrap_or_default();
                all.extend(logged_pids(&text));
            }
            let msg = format!(
                "WATCHDOG: {what} still running after {} s (hard limit); killing {all:?} and failing the test",
                limit.as_secs()
            );
            println!("{msg}");
            eprintln!("{msg}");
            for pid in all {
                kill_pid(pid);
            }
            std::process::exit(101);
        });
        Watchdog { armed, pids, logs }
    }

    /// Kill this process too when the limit runs out.
    pub fn watch_pid(&self, pid: Option<u32>) {
        if let (Some(pid), Ok(mut v)) = (pid, self.pids.lock()) {
            v.push(pid);
        }
    }

    /// Kill the game servers this lobby log names (`... server pid P ...`) too.
    pub fn watch_log(&self, log: &Path) {
        if let Ok(mut v) = self.logs.lock() {
            v.push(log.to_path_buf());
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.armed
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// PIDs of the game servers a lobby log reports starting (`... server pid P ...`).
pub fn logged_pids(log: &str) -> Vec<u32> {
    log.lines()
        .filter_map(|l| {
            let i = l.find(" server pid ")?;
            l[i + 12..].split_whitespace().next()?.parse().ok()
        })
        .collect()
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

/// `dc-dserver` on an ephemeral loopback port with the made-up world; returns the process and its UDP port.
pub fn start_dserver(world: &Path, extra: &[&str]) -> (Proc, u16) {
    let mut cmd = Command::new(dserver_exe());
    cmd.args(["-port", "0", "-worldfile"])
        .arg(world)
        .args(["-announce", "Worlds\\Synth.dat", "-idle", "30"])
        .args(extra);
    let mut p = Proc::spawn(cmd);
    let l = p
        .wait_line("@ready", Duration::from_secs(20))
        .unwrap_or_else(|| panic!("no @ready; server said {:?}", p.seen));
    let port = l
        .split_whitespace()
        .find_map(|t| t.strip_prefix("port="))
        .and_then(|p| p.parse().ok())
        .expect("port in @ready");
    (p, port)
}

/// The band the free-port helpers hand out from: below every OS ephemeral range (Linux 32768+, Windows and macOS
/// 49152+) and clear of the lobby's fallback range (29901-29999) and the usual game ports (27xxx). A port from
/// `bind 0` that was closed is back in the ephemeral pool, where any socket bound to port 0 can take it before the
/// lobby or game server the test starts binds it (2026-11-09: a lobby test's stand-in game server failed with
/// os error 10048 that way on the i686 CI lane). The same band, marker folder and rules as the lobby crate's test
/// support, so both suites share one machine-wide reservation.
const PORT_BAND_LO: u16 = 21_000;
const PORT_BAND_LEN: u32 = 6_000;
/// A reservation older than this is free again (a test server never keeps a port that long).
const PORT_RESERVATION_TTL: Duration = Duration::from_secs(600);

/// Claims `port` across every test process of the machine: a marker file made with `create_new` in a shared temp
/// folder; a stale marker is taken over by an atomic rename, so two processes never both win it.
fn reserve_port(port: u16) -> bool {
    let dir = std::env::temp_dir().join("dc-test-ports");
    let _ = std::fs::create_dir_all(&dir);
    let f = dir.join(port.to_string());
    let claim = |f: &Path| {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(f)
            .is_ok()
    };
    if claim(&f) {
        return true;
    }
    let stale = f
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > PORT_RESERVATION_TTL);
    if !stale {
        return false;
    }
    let moved = dir.join(format!("{port}.stale-{}", std::process::id()));
    if std::fs::rename(&f, &moved).is_err() {
        return false; // another process took it over first
    }
    let _ = std::fs::remove_file(&moved);
    claim(&f)
}

/// The next reserved band port for which `free` holds, together with the port above it (a lobby listens on its
/// lobby port + 1 too, and the listener loops refuse a set in which that one is another listener's): the band steps
/// by 2 from a random even offset per process, so a process never hands the same port out twice.
fn band_port(free: impl Fn(u16) -> bool) -> u16 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(u32::MAX);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let seed = (std::process::id() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ nanos;
    let start = (seed % PORT_BAND_LEN as u64) as u32 & !1;
    let _ = NEXT.compare_exchange(u32::MAX, start, Ordering::SeqCst, Ordering::SeqCst);
    for _ in 0..PORT_BAND_LEN / 2 {
        let n = NEXT.fetch_add(2, Ordering::SeqCst);
        let p = PORT_BAND_LO + (n % PORT_BAND_LEN) as u16;
        if reserve_port(p) && reserve_port(p + 1) && free(p) && free(p + 1) {
            return p;
        }
    }
    panic!(
        "no free port in {PORT_BAND_LO}..{}",
        PORT_BAND_LO as u32 + PORT_BAND_LEN
    );
}

/// A free TCP port on 127.0.0.1 from the test band (see [`PORT_BAND_LO`]).
pub fn free_tcp_port() -> u16 {
    band_port(|p| std::net::TcpListener::bind(("127.0.0.1", p)).is_ok())
}

/// A free UDP port on 127.0.0.1 from the test band (see [`PORT_BAND_LO`]).
pub fn free_udp_port() -> u16 {
    band_port(|p| std::net::UdpSocket::bind(("127.0.0.1", p)).is_ok())
}
