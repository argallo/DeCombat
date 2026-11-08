//! The game servers: one `dserver_exe` child per `start` (HOST_AGENT.md §3, §5, §6; DSERVER_PIPE.md §2).
//!
//! Per server three threads: the stdin writer (lines of `line <gid>`; dropping its channel closes stdin), the stdout
//! reader (each line becomes `event <gid> <text>`; at EOF it waits for the exit and sends `exited`), and the stderr
//! reader (the agent's own log). Lines go to the lobby through the [`Hub`]; while no control connection is up they
//! are dropped (§4: nothing is buffered).

use crate::config::HostConfig;
use crate::content::Contents;
use crate::log;
use crate::packs::Packs;
use crate::ports;
use crate::proto::{self, out, LineBuf, Piece, StartReq};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// The grace between closing a game server's stdin and the kill (DSERVER_PIPE.md §2).
// DC-DEFAULT: HA-1 5 s
pub const QUIT_GRACE: Duration = Duration::from_secs(5);

/// How often a reaper looks whether a server whose stdout ended has exited.
const REAP_POLL: Duration = Duration::from_millis(100);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where the agent's lines go: the current control connection, or nowhere.
#[derive(Default)]
pub struct Hub {
    tx: Mutex<Option<Sender<String>>>,
}

impl Hub {
    /// Sends one line to the lobby when a connection is attached (dropped otherwise).
    pub fn send(&self, line: String) {
        if let Some(tx) = lock(&self.tx).as_ref() {
            let _ = tx.send(line);
        }
    }

    pub fn attach(&self, tx: Sender<String>) {
        *lock(&self.tx) = Some(tx);
    }

    pub fn detach(&self) {
        *lock(&self.tx) = None;
    }
}

struct Game {
    /// Unique per spawn (a later server of the same gid is not killed by an earlier quit's timer).
    serial: u64,
    port: u16,
    pid: u32,
    child: Child,
    /// None once `quit` closed stdin.
    stdin: Option<Sender<String>>,
}

/// The running game servers of this agent.
pub struct Games {
    cfg: HostConfig,
    hub: Arc<Hub>,
    /// MAP-84: the map packs on this machine.
    pub packs: Packs,
    /// LG-431: the weapon content documents on this machine.
    pub contents: Contents,
    /// HA-2: the server build the lobby offers, on its way to tools/host/update.sh.
    pub incoming: crate::update::Incoming,
    map: Mutex<BTreeMap<u64, Game>>,
    serial: AtomicU64,
}

impl Games {
    pub fn new(cfg: HostConfig, hub: Arc<Hub>) -> Arc<Games> {
        let packs = Packs::new(&cfg.log_dir);
        let contents = Contents::new(&cfg.log_dir);
        let incoming = crate::update::Incoming::new(&cfg.log_dir);
        Arc::new(Games {
            cfg,
            hub,
            packs,
            contents,
            incoming,
            map: Mutex::new(BTreeMap::new()),
            serial: AtomicU64::new(1),
        })
    }

    /// (gid, port, pid) of every running server, in gid order.
    pub fn running(&self) -> Vec<(u64, u16, u32)> {
        lock(&self.map)
            .iter()
            .map(|(g, s)| (*g, s.port, s.pid))
            .collect()
    }

    /// `start <gid> <json>`: the answer line (`started` or `failed`). `world` is a ROM world, unless the request
    /// names a `world_pack` (a featured entry's community map, DC_DEFAULTS.md MAP-83): the world is then that file of
    /// the unpacked pack.
    pub fn start(self: &Arc<Self>, gid: u64, json: &str) -> String {
        let mut map = lock(&self.map);
        if let Some(g) = map.get(&gid) {
            return out::started(gid, g.port, g.pid);
        }
        let req = match StartReq::parse(json) {
            Ok(r) => r,
            Err(e) => {
                log!("game {gid}: {e}");
                return out::failed(gid, &e);
            }
        };
        let ours: Vec<u16> = map.values().map(|g| g.port).collect();
        let ip = self.cfg.bind_ip;
        let Some(port) = ports::pick(self.cfg.port_lo, self.cfg.port_hi, req.port, &ours, &|p| {
            ports::udp_free(ip, p)
        }) else {
            log!(
                "game {gid}: no free UDP port in {}-{}",
                self.cfg.port_lo,
                self.cfg.port_hi
            );
            return out::failed(gid, "no free port");
        };
        match self.spawn(gid, &req, port) {
            Ok(g) => {
                let line = out::started(gid, g.port, g.pid);
                map.insert(gid, g);
                line
            }
            Err(e) => {
                log!("game {gid}: {e}");
                out::failed(gid, &e)
            }
        }
    }

    fn spawn(self: &Arc<Self>, gid: u64, req: &StartReq, port: u16) -> Result<Game, String> {
        let c = &self.cfg;
        if !c.dserver_exe.is_file() {
            return Err(format!("exe missing: {}", c.dserver_exe.display()));
        }
        std::fs::create_dir_all(&c.log_dir)
            .map_err(|e| format!("log_dir {}: {e}", c.log_dir.display()))?;
        let log_file = c.log_dir.join(req.log_name(gid));
        // MAP-83: `failed <gid> pack <sha>: ...` tells the lobby it does not hold the pack (MAP-84)
        let pack_world = match &req.world_pack {
            Some(p) => Some(
                self.packs
                    .world(p, &req.world_file)
                    .map_err(|e| format!("pack {}: {e}", p.sha256))?,
            ),
            None => None,
        };
        let mut argv = proto::dserver_argv(
            req,
            port,
            &log_file,
            &c.bind_ip.to_string(),
            &c.dserver_args,
            pack_world.as_deref(),
        );
        // LG-313: a featured entry's weapon content reaches this game server alone
        if !req.content.is_empty() {
            let hash = &req.content_hash;
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("bad content hash".to_string());
            }
            let file = c.log_dir.join(format!("content-{hash}.json"));
            std::fs::write(&file, &req.content)
                .map_err(|e| format!("content {}: {e}", file.display()))?;
            argv.push("-content".into());
            argv.push(file.into_os_string());
        } else if !req.content_hash.is_empty() {
            // LG-431: `failed <gid> content <hash>: ...` tells the lobby it does not hold the document
            let file = self
                .contents
                .path(&req.content_hash)
                .map_err(|e| format!("content {}: {e}", req.content_hash))?;
            argv.push("-content".into());
            argv.push(file.into_os_string());
        }
        // no shell: the program and each argument are argv entries of their own
        let mut cmd = Command::new(&c.dserver_exe);
        cmd.args(&argv);
        if let Some(rom) = &c.rom {
            cmd.env("CA_ROM", rom);
        }
        // its own process group: a signal to the agent's group does not reach it; the kill takes its whole group
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
        }
        cmd.current_dir(&c.log_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("spawn error: {e}"))?;
        let pid = child.id();
        let serial = self.serial.fetch_add(1, Ordering::SeqCst);
        log!(
            "game {gid}: pid {pid} port {port} world {} log {}",
            req.world,
            log_file.display()
        );
        let (tx, rx) = mpsc::channel::<String>();
        if let Some(mut stdin) = child.stdin.take() {
            std::thread::spawn(move || {
                for l in rx {
                    let mut b = l.into_bytes();
                    b.push(b'\n');
                    if stdin.write_all(&b).and_then(|_| stdin.flush()).is_err() {
                        break;
                    }
                }
                // the channel or the pipe ended: dropping `stdin` closes it
            });
        }
        if let Some(mut stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                let mut lb = LineBuf::default();
                let mut buf = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    for p in lb.push(&buf[..n]) {
                        match p {
                            Piece::Line(l) => log!("game {gid} stderr: {l}"),
                            Piece::TooLong(n) => {
                                log!("game {gid} stderr: a line of {n} bytes dropped")
                            }
                        }
                    }
                }
            });
        }
        if let Some(mut stdout) = child.stdout.take() {
            let me = Arc::clone(self);
            std::thread::spawn(move || {
                let mut lb = LineBuf::default();
                let mut buf = [0u8; 8192];
                while let Ok(n) = stdout.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    for p in lb.push(&buf[..n]) {
                        match p {
                            Piece::Line(l) => {
                                let e = out::event(gid, &l);
                                if e.len() > proto::MAX_LINE {
                                    log!("game {gid}: an output line of {} bytes dropped (over 16 KiB)", l.len());
                                } else {
                                    me.hub.send(e);
                                }
                            }
                            Piece::TooLong(n) => {
                                log!(
                                    "game {gid}: an output line of {n} bytes dropped (over 16 KiB)"
                                )
                            }
                        }
                    }
                }
                me.reap(gid, serial);
            });
        }
        Ok(Game {
            serial,
            port,
            pid,
            child,
            stdin: Some(tx),
        })
    }

    /// Waits for the exit of the server `serial` of `gid` (its stdout has ended), then sends `exited`.
    fn reap(&self, gid: u64, serial: u64) {
        loop {
            {
                let mut map = lock(&self.map);
                let Some(g) = map.get_mut(&gid).filter(|g| g.serial == serial) else {
                    return;
                };
                let code = match g.child.try_wait() {
                    Ok(Some(st)) => Some(st.code()),
                    Ok(None) => None,
                    Err(e) => {
                        log!("game {gid}: wait: {e}");
                        Some(None)
                    }
                };
                if let Some(code) = code {
                    map.remove(&gid);
                    drop(map);
                    log!("game {gid}: exited code {}", code.unwrap_or(-1));
                    self.hub.send(out::exited(gid, code));
                    return;
                }
            }
            std::thread::sleep(REAP_POLL);
        }
    }

    /// `line <gid> <text>`: one stdin line; false when no such server (or its stdin is closed).
    pub fn line(&self, gid: u64, text: &str) -> bool {
        match lock(&self.map).get(&gid).and_then(|g| g.stdin.as_ref()) {
            Some(tx) => tx.send(text.to_string()).is_ok(),
            None => false,
        }
    }

    /// `quit <gid>`: closes stdin; the server is killed when it still runs after [`QUIT_GRACE`].
    pub fn quit(self: &Arc<Self>, gid: u64) -> bool {
        self.quit_after(gid, QUIT_GRACE)
    }

    fn quit_after(self: &Arc<Self>, gid: u64, grace: Duration) -> bool {
        let serial = {
            let mut map = lock(&self.map);
            let Some(g) = map.get_mut(&gid) else {
                return false;
            };
            g.stdin = None;
            g.serial
        };
        let me = Arc::clone(self);
        std::thread::spawn(move || {
            std::thread::sleep(grace);
            let mut map = lock(&me.map);
            if let Some(g) = map.get_mut(&gid).filter(|g| g.serial == serial) {
                if let Ok(None) = g.child.try_wait() {
                    log!(
                        "game {gid}: still running {} s after quit: killed",
                        grace.as_secs()
                    );
                    kill(&mut g.child);
                }
            }
        });
        true
    }
}

/// Kills the child and, on unix, its whole process group (it is the group's leader).
fn kill(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: kill(2) with a negative pid signals that process group; no memory is touched.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
}
