//! The processes of a run: a local lobby (which starts one game server per room), or a game server alone on a
//! synthetic world (direct run), and one `dc-botclient` process per bot. Everything on loopback, headless, killed
//! when dropped.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The binaries a run needs.
#[derive(Debug, Clone)]
pub struct Bins {
    pub lobby: PathBuf,
    pub dserver: PathBuf,
    pub bot: PathBuf,
}

fn exe(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

impl Bins {
    /// The binaries in `dir`, else next to this program.
    pub fn find(dir: Option<&Path>) -> Result<Bins, String> {
        let dir = match dir {
            Some(d) => d.to_path_buf(),
            None => std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .ok_or("cannot find this program's folder")?,
        };
        let b = Bins {
            lobby: exe(&dir, "dc-lobby-server"),
            dserver: exe(&dir, "dc-dserver"),
            bot: exe(&dir, "dc-botclient"),
        };
        for p in [&b.dserver, &b.bot] {
            if !p.is_file() {
                return Err(format!(
                    "{} not found (build it: cargo build --release -p dc-dserver -p dc-botclient -p dc-lobby-server -p dc-netlab, or pass --bin-dir)",
                    p.display()
                ));
            }
        }
        Ok(b)
    }
}

/// `count` free loopback TCP + UDP ports in a row from `from` on.
pub fn free_ports(from: u16, count: u16) -> Option<u16> {
    let mut base = from;
    while base < 60_000 {
        let ok = (0..count).all(|i| {
            TcpListener::bind(("127.0.0.1", base + i)).is_ok()
                && UdpSocket::bind(("127.0.0.1", base + i)).is_ok()
        });
        if ok {
            return Some(base);
        }
        base += count + 7;
    }
    None
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// A local lobby with game servers on demand (the ROM run).
pub struct Lobby {
    child: Child,
    pub auth_port: u16,
    pub dserver_dir: PathBuf,
}

impl Drop for Lobby {
    fn drop(&mut self) {
        kill(&mut self.child);
    }
}

impl Lobby {
    /// Starts `dc-lobby-server` on loopback ports from `base` with a fresh database in `dir`; waits until its auth
    /// port answers. The game servers it starts log into `dir/dserver`.
    pub fn start(
        bins: &Bins,
        rom: &Path,
        dir: &Path,
        base: u16,
        dserver_args: &str,
    ) -> Result<Lobby, String> {
        if !bins.lobby.is_file() {
            return Err(format!("{} not found", bins.lobby.display()));
        }
        let dserver_dir = dir.join("dserver");
        std::fs::create_dir_all(&dserver_dir).map_err(|e| e.to_string())?;
        let out = File::create(dir.join("lobby.out")).map_err(|e| e.to_string())?;
        let mut args: Vec<String> = vec![
            "--port".into(),
            base.to_string(),
            "--lobbyport".into(),
            (base + 1).to_string(),
            "--clanport".into(),
            (base + 2).to_string(),
            "--clanweb".into(),
            "0".into(),
            "--db".into(),
            dir.join("lobby.db").to_string_lossy().into_owned(),
            "--log".into(),
            dir.join("lobby.log").to_string_lossy().into_owned(),
            "--autochar".into(),
            "--gameport".into(),
            (base + 10).to_string(),
            "--countdown".into(),
            "0.2".into(),
            "--dserver".into(),
            "--dserver-exe".into(),
            bins.dserver.to_string_lossy().into_owned(),
            "--dserver-logdir".into(),
            dserver_dir.to_string_lossy().into_owned(),
        ];
        if !dserver_args.is_empty() {
            args.push(format!("--dserver-args={dserver_args}"));
        }
        let child = Command::new(&bins.lobby)
            .args(&args)
            .env("CA_ROM", rom)
            .stdin(Stdio::piped())
            .stdout(out.try_clone().map_err(|e| e.to_string())?)
            .stderr(out)
            .spawn()
            .map_err(|e| format!("start {}: {e}", bins.lobby.display()))?;
        let mut lobby = Lobby {
            child,
            auth_port: base,
            dserver_dir,
        };
        let end = Instant::now() + Duration::from_secs(90);
        loop {
            if TcpStream::connect_timeout(
                &format!("127.0.0.1:{base}").parse().expect("address"),
                Duration::from_millis(300),
            )
            .is_ok()
            {
                return Ok(lobby);
            }
            if let Ok(Some(st)) = lobby.child.try_wait() {
                return Err(format!(
                    "the lobby ended at start ({st}); see {}",
                    dir.join("lobby.out").display()
                ));
            }
            if Instant::now() > end {
                return Err("the lobby did not open its auth port within 90 s".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// A game server on a synthetic world, joined directly by the bots (the run without a ROM).
pub struct Direct {
    child: Child,
    stdin: Option<ChildStdin>,
    pub port: u16,
    pub log: PathBuf,
}

impl Drop for Direct {
    fn drop(&mut self) {
        // closing stdin ends the server the normal way (its links get their byes); kill only a hung one
        if let Some(mut s) = self.stdin.take() {
            let _ = writeln!(s, "quit");
        }
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        kill(&mut self.child);
    }
}

/// A made-up world (no ROM content): a box from (-512, -64, -512) to (512, 256, 512) with one model.
pub fn synthetic_world(dir: &Path) -> Result<PathBuf, String> {
    use dc_world::synth::{SynthModel, SynthWorld};
    let p = dir.join("synth.dat");
    let bytes = SynthWorld {
        info: b"netlab synthetic".to_vec(),
        extent_min: [-512.0, -64.0, -512.0],
        extent_max: [512.0, 256.0, 512.0],
        tree_shape: vec![false],
        models: vec![SynthModel {
            name: b"main".to_vec(),
            root: -1,
            ..SynthModel::default()
        }],
        ..SynthWorld::default()
    }
    .build();
    std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
    Ok(p)
}

impl Direct {
    /// Starts `dc-dserver` on the synthetic world with the roster `users` (`(nick, team 0|1)`); waits for `@ready`.
    pub fn start(
        bins: &Bins,
        dir: &Path,
        users: &[(String, u8)],
        extra: &[String],
    ) -> Result<Direct, String> {
        let world = synthetic_world(dir)?;
        let log = dir.join("dserver_direct.log");
        let mut args: Vec<String> = ["-port", "0", "-worldfile"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        args.push(world.to_string_lossy().into_owned());
        for a in [
            "-announce",
            "Worlds\\Synth.dat",
            "-idle",
            "900",
            "-loadout",
            "3,4",
            "-nomovecheck",
            "-log",
        ] {
            args.push(a.into());
        }
        args.push(log.to_string_lossy().into_owned());
        args.extend(extra.iter().cloned());
        let err = File::create(dir.join("dserver.err")).map_err(|e| e.to_string())?;
        let mut child = Command::new(&bins.dserver)
            .args(&args)
            .env_remove("CA_ROM")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(err)
            .spawn()
            .map_err(|e| format!("start {}: {e}", bins.dserver.display()))?;
        let out = child.stdout.take().expect("piped stdout");
        let (tx, rx) = mpsc::channel();
        let pipe_log = dir.join("dserver.pipe");
        std::thread::spawn(move || {
            let mut f = File::create(pipe_log).ok();
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(f) = f.as_mut() {
                    let _ = writeln!(f, "{l}");
                }
                let _ = tx.send(l);
            }
        });
        let mut stdin = child.stdin.take().expect("piped stdin");
        for (i, (nick, team)) in users.iter().enumerate() {
            let n = i + 1;
            writeln!(stdin, "user {n} {n} {n} {team} 0 {nick}").map_err(|e| e.to_string())?;
        }
        writeln!(stdin, "room key=TDM goal=1000").map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())?;
        let end = Instant::now() + Duration::from_secs(120);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                kill(&mut child);
                return Err("the game server sent no @ready within 120 s".into());
            }
            match rx.recv_timeout(left) {
                Ok(l) if l.starts_with("@ready") => {
                    let port = l
                        .split_whitespace()
                        .find_map(|t| t.strip_prefix("port="))
                        .and_then(|p| p.parse().ok())
                        .ok_or_else(|| format!("no port in {l:?}"))?;
                    return Ok(Direct {
                        child,
                        stdin: Some(stdin),
                        port,
                        log,
                    });
                }
                Ok(_) => {}
                Err(_) => {
                    kill(&mut child);
                    return Err("the game server ended before @ready".into());
                }
            }
        }
    }
}

/// One bot process.
pub struct BotProc {
    pub nick: String,
    pub net: String,
    pub child: Option<Child>,
    pub events: PathBuf,
    pub report: PathBuf,
    pub stdout: PathBuf,
    pub exit: Option<i32>,
}

impl Drop for BotProc {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            kill(&mut c);
        }
    }
}

impl BotProc {
    /// Starts `dc-botclient` with `args` (the log, report and network options are added here).
    pub fn spawn(
        bins: &Bins,
        dir: &Path,
        nick: &str,
        net: &str,
        seed: u64,
        args: &[String],
    ) -> Result<BotProc, String> {
        let events = dir.join(format!("{nick}.events.jsonl"));
        let report = dir.join(format!("{nick}.report.json"));
        let stdout = dir.join(format!("{nick}.out"));
        let out = File::create(&stdout).map_err(|e| e.to_string())?;
        let mut all: Vec<String> = args.to_vec();
        for (k, v) in [
            ("--name", nick.to_string()),
            ("--log", events.to_string_lossy().into_owned()),
            ("--report", report.to_string_lossy().into_owned()),
            ("--net", net.to_string()),
            ("--loss-seed", seed.to_string()),
        ] {
            all.push(k.into());
            all.push(v);
        }
        let child = Command::new(&bins.bot)
            .args(&all)
            .stdin(Stdio::null())
            .stdout(out.try_clone().map_err(|e| e.to_string())?)
            .stderr(out)
            .spawn()
            .map_err(|e| format!("start {}: {e}", bins.bot.display()))?;
        Ok(BotProc {
            nick: nick.to_string(),
            net: net.to_string(),
            child: Some(child),
            events,
            report,
            stdout,
            exit: None,
        })
    }

    /// True once the process ended (its exit code kept).
    pub fn done(&mut self) -> bool {
        match self.child.as_mut().map(|c| c.try_wait()) {
            Some(Ok(Some(st))) => {
                self.exit = st.code();
                self.child = None;
                true
            }
            Some(_) => false,
            None => true,
        }
    }

    pub fn kill(&mut self) {
        if let Some(mut c) = self.child.take() {
            kill(&mut c);
            self.exit = Some(-9);
        }
    }
}
