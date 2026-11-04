//! Loopback test of shooter mode against our own dedicated server binary: `dc-dserver` (slice 2 handles `UP_FIRE`
//! 0x82, damage and kills) runs on an ephemeral UDP port with a synthetic world; a walking shooter bot and a
//! standing target bot join over real UDP on opposite teams. The shooter selects its weapon from the server's
//! grants, reports hits on the target's object, the server kills the target (0x6d to both, 0x67 state 2 to the
//! victim), the target respawns on the server's own timer (no respawn request: the server never armed the
//! client's wait) and the shooter takes it as a target again.
//!
//! The shooter fires the 2009 cone (`dc-cone`) from its counter seeded by the server's 0x7d `own_value`
//! (`-combat shot_seed`, set near the table size so the reads wrap): the server's shot check (`cone_check=log`) must
//! count every pellet `cone_ok` (0 off, 0 na) in its `session cone:` line; with the ROM (`CA_ROM`, the weapon rows and
//! the random table on both sides) the pellets are tilted and the server's expected `stat` is exact.
//!
//! Load proof: nothing here waits for a number of seconds. Every bot runs until the event the test is about happened
//! (`run_game_until`; the wall-clock number is only a ceiling that turns into a failure with a state dump), the join
//! limit of the bots and the server's idle limit are long, timing claims are read from the server's own clock lines
//! (its respawn timer) or from the bots' session clock, and a failing test prints both bots' state, the server's
//! lines and log, and the longest scheduling stall this process saw (`LoadProbe`). A hang is cut by a watchdog.
//!
//! The server binary is found next to this test's target directory (built by `cargo test --workspace` or `cargo
//! build -p dc-dserver`), or through `DC_DSERVER_EXE`; without it the test is skipped with a message.
// SPEC: DSERVER_PIPE.md §2 (launch line), §3.1 (user line: team 0 = Alpha, 1 = Bravo), §4 (@ready)
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1, Appendix A steps A8-A11
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.5 (0x7d seeds the counter), AC-6 (the server's check)

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game_until;
use dc_botclient::session::{BotSession, GameConfig};
use dc_botclient::shooter::ShooterConfig;
use serde_json::Value;

/// How long the server may take to print `@ready` (world and table load) on a machine at full load.
const READY_CEILING: Duration = Duration::from_secs(120);
/// A bot's ceiling for the wait on its stop event. Reaching it is a failure that prints the state.
const BOT_CEILING: Duration = Duration::from_secs(150);
/// The join limit of the bots (the 2009 client's is 10 s; the server of a starved machine answers later).
const JOIN_LIMIT_MS: u32 = 120_000;
/// A test that has not ended after this long is hung: the watchdog kills the servers and ends the process.
const WATCHDOG_S: u64 = 480;

fn dserver_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DC_DSERVER_EXE") {
        return Some(PathBuf::from(p)).filter(|p| p.exists());
    }
    let me = std::env::current_exe().ok()?;
    let dir = me.parent()?.parent()?;
    let p = dir.join(format!("dc-dserver{}", std::env::consts::EXE_SUFFIX));
    p.exists().then_some(p)
}

/// Dedicated-server processes of this test process (the watchdog kills them when a test hangs).
static SERVER_PIDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static UNIQUE: AtomicUsize = AtomicUsize::new(0);

fn kill_pid(pid: u32) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
    } else {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
    }
}

/// Ends the test process (exit 101) and the servers when the test it guards runs longer than `WATCHDOG_S`.
struct Watchdog {
    done: Arc<AtomicBool>,
}

impl Watchdog {
    fn arm(test: &'static str) -> Watchdog {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        std::thread::spawn(move || {
            let end = Instant::now() + Duration::from_secs(WATCHDOG_S);
            while Instant::now() < end {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if flag.load(Ordering::Relaxed) {
                return;
            }
            eprintln!("WATCHDOG: {test} still running after {WATCHDOG_S} s; killing the servers");
            for pid in SERVER_PIDS.lock().map(|g| g.clone()).unwrap_or_default() {
                kill_pid(pid);
            }
            std::process::exit(101);
        });
        Watchdog { done }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
    }
}

/// The longest overshoot of a 5 ms sleep seen on this process while the test ran: a measure of how long the machine
/// kept a thread of this process from running. Timing bounds that depend on the server's own clock widen by it.
struct LoadProbe {
    stop: Arc<AtomicBool>,
    worst_ms: Arc<AtomicU32>,
}

impl LoadProbe {
    fn start() -> LoadProbe {
        let stop = Arc::new(AtomicBool::new(false));
        let worst_ms = Arc::new(AtomicU32::new(0));
        let (s, w) = (stop.clone(), worst_ms.clone());
        std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                let t = Instant::now();
                std::thread::sleep(Duration::from_millis(5));
                let over = t.elapsed().as_millis().saturating_sub(5) as u32;
                w.fetch_max(over, Ordering::Relaxed);
            }
        });
        LoadProbe { stop, worst_ms }
    }

    fn worst_ms(&self) -> u32 {
        self.worst_ms.load(Ordering::Relaxed)
    }
}

impl Drop for LoadProbe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The spawned server; killed when dropped.
struct Server {
    child: Option<Child>,
    /// Kept open for the session: the server ends when its stdin closes.
    _stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    seen: Vec<String>,
    /// The server's own `-log` file (every line stamped with the server's clock).
    log: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Server {
    fn spawn(exe: &PathBuf, args: &[String], users: &[&str], log: PathBuf) -> Server {
        let mut child = Command::new(exe)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn dc-dserver");
        if let Ok(mut g) = SERVER_PIDS.lock() {
            g.push(child.id());
        }
        let out = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    return;
                }
            }
        });
        let mut stdin = child.stdin.take().unwrap();
        for u in users {
            writeln!(stdin, "{u}").unwrap();
        }
        stdin.flush().unwrap();
        Server {
            child: Some(child),
            _stdin: Some(stdin),
            lines: rx,
            seen: Vec::new(),
            log,
        }
    }

    fn wait(&mut self, prefix: &str, t: Duration) -> Option<String> {
        let end = Instant::now() + t;
        loop {
            if let Some(l) = self.seen.iter().find(|l| l.starts_with(prefix)) {
                return Some(l.clone());
            }
            let left = end.checked_duration_since(Instant::now())?;
            match self.lines.recv_timeout(left.min(Duration::from_millis(50))) {
                Ok(l) => self.seen.push(l),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    fn drain(&mut self) {
        while let Ok(l) = self.lines.try_recv() {
            self.seen.push(l);
        }
    }

    /// `quit` on the control channel (DSERVER_PIPE.md §3.4), then the session's closing lines up to `@end`.
    fn quit(&mut self) {
        if let Some(s) = self._stdin.as_mut() {
            let _ = writeln!(s, "quit");
            let _ = s.flush();
        }
        let _ = self.wait("@end", Duration::from_secs(30));
        self.drain();
    }

    /// What the server printed and logged, for a failure message.
    fn state(&mut self) -> String {
        self.drain();
        let log = std::fs::read_to_string(&self.log).unwrap_or_default();
        let tail: Vec<&str> = log.lines().rev().take(60).collect();
        format!(
            "server stdout {:?}\nserver log (last {} lines, server clock ms first):\n{}",
            self.seen,
            tail.len(),
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        )
    }

    /// The server's own time of its respawn timer for the first death: the `N` of `respawned by the server N ms
    /// after the death (no request)`.
    fn server_respawn_ms(&self) -> Option<u64> {
        self.seen.iter().find_map(|l| {
            let rest = l.split("respawned by the server ").nth(1)?;
            let (n, tail) = rest.split_once(" ms after the death")?;
            tail.contains("(no request)")
                .then(|| n.trim().parse().ok())
                .flatten()
        })
    }
}

/// The counters of a `cone:` line (`cone: N pellets, A ok, B off (...), C na, D unchecked; E without row bounds;
/// stat skew F; mode log`): (pellets, ok, off, na, unchecked, unbounded, skew).
fn cone_counts(line: &str) -> Option<[u64; 7]> {
    let n = |key: &str| -> Option<u64> {
        let i = line.find(key)?;
        line[..i].split_whitespace().last()?.parse().ok()
    };
    let skew = line
        .split("stat skew ")
        .nth(1)?
        .split(';')
        .next()?
        .trim()
        .parse()
        .ok()?;
    Some([
        n(" pellets")?,
        n(" ok,")?,
        n(" off (")?,
        n(" na,")?,
        n(" unchecked")?,
        n(" without row")?,
        skew,
    ])
}

/// A made-up world file (no ROM content): a box from (-512, -64, -512) to (512, 256, 512) and one model. One file
/// per call, so tests running side by side never rewrite a file another server is reading.
fn synthetic_world() -> PathBuf {
    use dc_world::synth::{SynthModel, SynthWorld};
    let dir = std::env::temp_dir().join(format!(
        "dc-botclient-shooter-{}-{}",
        std::process::id(),
        UNIQUE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("synth.dat");
    let bytes = SynthWorld {
        info: b"botclient shooter synthetic".to_vec(),
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
    std::fs::write(&p, bytes).unwrap();
    p
}

fn events<'a>(log: &'a [Value], kind: &str) -> Vec<&'a Value> {
    log.iter().filter(|e| e["ev"] == kind).collect()
}

/// A bot's run: its final state, its event log, and whether its stop event happened (false = the ceiling).
struct Run {
    nick: String,
    bot: BotSession,
    log: Vec<Value>,
    reached: bool,
    /// Session clock of the end of the loop, ms since the socket opened.
    ended_ms: u32,
}

impl Run {
    /// One line on the join (always printed, shown with `--nocapture` or when a test fails): when the join reply came,
    /// after how many asks, and the bot's socket counters.
    fn join_line(&self) -> String {
        let joined = self.bot.obs.milestones.iter().find(|m| m.0 == "joined");
        let io = self.log.iter().find(|e| e["ev"] == "udp_io");
        format!(
            "join {}: joined at {:?} ms after {} asks, udp_io {}",
            self.nick,
            joined.map(|m| m.1),
            self.bot.obs.join_asks,
            io.map(|e| e.to_string()).unwrap_or_default()
        )
    }

    /// The bot's state for a failure message.
    fn state(&self) -> String {
        let mut kinds = std::collections::BTreeMap::<String, usize>::new();
        for e in &self.log {
            *kinds
                .entry(e["ev"].as_str().unwrap_or("?").to_string())
                .or_default() += 1;
        }
        let key: Vec<String> = self
            .log
            .iter()
            .filter(|e| {
                matches!(
                    e["ev"].as_str(),
                    Some(
                        "join_failed"
                            | "server_gone"
                            | "fatal"
                            | "joined"
                            | "timeline_start"
                            | "died"
                            | "respawned"
                            | "udp_io"
                    )
                )
            })
            .map(|e| e.to_string())
            .collect();
        format!(
            "bot {}: stop event reached {}, loop ended at {} ms, stage {:?}, end {:?}, join asks {}, fatal {:?}, \
             milestones {:?}, life states {:?}, event kinds {:?}, key events {:?}",
            self.nick,
            self.reached,
            self.ended_ms,
            self.bot.stage(),
            self.bot.obs.end,
            self.bot.obs.join_asks,
            self.bot.obs.fatal,
            self.bot.obs.milestones,
            self.bot.obs.life_states,
            kinds,
            key
        )
    }
}

/// Prints the whole state of a test when it fails (the guard drops while the test panics).
struct FailDump(String);

impl Drop for FailDump {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("---- state at the failure ----\n{}", self.0);
        }
    }
}

fn fail_dump(srv: &mut Server, runs: &[&Run], probe: &LoadProbe) -> FailDump {
    let mut t = format!(
        "longest scheduling stall seen by this process: {} ms\n",
        probe.worst_ms()
    );
    for r in runs {
        t.push_str(&r.state());
        t.push('\n');
        println!("{}", r.join_line());
    }
    t.push_str(&srv.state());
    let log = std::fs::read_to_string(&srv.log).unwrap_or_default();
    for l in log.lines().filter(|l| l.contains("dc-dserver: udp")) {
        println!("{l}");
    }
    FailDump(t)
}

/// What one bot is and does.
struct BotSpec {
    nick: &'static str,
    motion: MotionKind,
    shooter: Option<ShooterConfig>,
    timeline: Option<String>,
    /// Stay in the game server after the result screen (the arena's `--stay-after-result`); timeline bots only.
    stay_after_result: bool,
    /// Seconds to wait before the bot starts.
    delay_s: u64,
}

impl BotSpec {
    fn new(nick: &'static str, motion: MotionKind) -> BotSpec {
        BotSpec {
            nick,
            motion,
            shooter: None,
            timeline: None,
            stay_after_result: true,
            delay_s: 0,
        }
    }
}

/// Runs a bot on its own thread until `stop` says so (checked at most every 50 ms of the session clock).
fn spawn_bot<F>(
    port: u16,
    world: PathBuf,
    spec: BotSpec,
    mut stop: F,
) -> std::thread::JoinHandle<Run>
where
    F: FnMut(&BotSession, &Recorder, u32) -> bool + Send + 'static,
{
    std::thread::spawn(move || {
        if spec.delay_s > 0 {
            std::thread::sleep(Duration::from_secs(spec.delay_s));
        }
        let nick = spec.nick;
        let mut cfg = GameConfig::new(nick);
        cfg.world_file = Some(world);
        cfg.hello_tag = nick.to_ascii_lowercase();
        cfg.motion = spec.motion;
        cfg.load_ms = 300;
        cfg.shooter = spec.shooter;
        cfg.join_timeout_ms = JOIN_LIMIT_MS;
        if let Some(t) = &spec.timeline {
            cfg.timeline = Some(dc_botclient::timeline::parse(t, nick).unwrap());
            cfg.leave_after_result = !spec.stay_after_result;
        }
        let rec = Recorder::null(nick).keep_in_memory();
        let mut bot = BotSession::new(cfg, None, rec.clone());
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let mut reached = false;
        let mut last_check: Option<u32> = None;
        let mut ended_ms = 0u32;
        let mut check = |b: &BotSession, t: u32| {
            ended_ms = t;
            if last_check.is_some_and(|l| t.wrapping_sub(l) < 50) {
                return false;
            }
            last_check = Some(t);
            let s = stop(b, &rec, t);
            reached |= s;
            s
        };
        run_game_until(&mut bot, server, BOT_CEILING, &rec, &mut check).expect("udp run");
        Run {
            nick: nick.to_string(),
            bot,
            log: rec.memory(),
            reached,
            ended_ms,
        }
    })
}

/// The command line of the test server: a synthetic world, a long idle limit (the default 30 s counts from the
/// server's start, and a loaded machine can take longer than that to bring in the first client), a log file.
fn server_args(world: &Path, log: &Path, extra: &[&str]) -> Vec<String> {
    let w = world.to_string_lossy().into_owned();
    let l = log.to_string_lossy().into_owned();
    let mut args: Vec<String> = [
        "-port",
        "0",
        "-worldfile",
        w.as_str(),
        "-announce",
        "Worlds\\Synth.dat",
        "-idle",
        "300",
        "-loadout",
        "3,4",
        "-nomovecheck",
        "-log",
        l.as_str(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

fn ready_server(exe: &PathBuf, world: &Path, extra: &[&str], users: &[&str]) -> (Server, u16) {
    let log = world.with_file_name("server.log");
    let args = server_args(world, &log, extra);
    let mut srv = Server::spawn(exe, &args, users, log);
    let l = srv
        .wait("@ready", READY_CEILING)
        .unwrap_or_else(|| panic!("no @ready in {READY_CEILING:?}; {}", srv.state()));
    let port: u16 = l
        .split_whitespace()
        .find_map(|t| t.strip_prefix("port="))
        .and_then(|p| p.parse().ok())
        .expect("port in @ready");
    (srv, port)
}

/// Both-flags gate: each side sets its own flag when its condition holds, both stop when both are set.
#[derive(Clone, Default)]
struct Gate {
    a: Arc<AtomicBool>,
    b: Arc<AtomicBool>,
    /// Set by the bot that decides the end (after its own settle time); the other one stops on it.
    done: Arc<AtomicBool>,
}

impl Gate {
    fn both(&self) -> bool {
        self.a.load(Ordering::Relaxed) && self.b.load(Ordering::Relaxed)
    }
}

/// Session-clock settle: `true` once `settle_ms` of the loop's clock passed since `since` was first set.
fn settled(since: &mut Option<u32>, now: u32, settle_ms: u32) -> bool {
    let s = *since.get_or_insert(now);
    now.wrapping_sub(s) >= settle_ms
}

#[test]
fn shooter_kills_a_standing_bot_which_respawns_on_the_server_timer() {
    round(None);
}

/// The same round with the human-like skill shooter: the aim error turns the sent rotation, the spread byte grows
/// with the bursts, and every pellet still follows the cone through the server's check.
#[test]
fn skill_shooter_pellets_pass_the_server_cone_check() {
    round(Some(dc_botclient::skill::Skill::new(
        dc_botclient::skill::STANDARD_LEVEL,
    )));
}

fn round(skill: Option<dc_botclient::skill::Skill>) {
    let Some(exe) = dserver_exe() else {
        println!("skipped: dc-dserver binary not built (cargo build -p dc-dserver, or set DC_DSERVER_EXE)");
        return;
    };
    let _wd = Watchdog::arm("round");
    let probe = LoadProbe::start();
    let world = synthetic_world();
    let (mut srv, port) = ready_server(
        &exe,
        &world,
        &["-combat", "cone_check=log,shot_seed=9993"],
        &["user 1 1 1 0 0 Shooter", "user 2 2 2 1 0 Target"],
    );
    // the ROM's weapon rows and random table, as `dc-botclient --rom` loads them (the server reads the same ROM)
    let rom = dc_rez::dc_rom::test_rom();
    let sc = ShooterConfig {
        shot_period_ms: 150,
        rearm_ms: 9000,
        rom: rom.as_ref().map(|r| {
            std::sync::Arc::new(dc_botclient::shooter::RomWeapons::open(r).expect("ROM weapons"))
        }),
        table: Some(std::sync::Arc::new(dc_cone::ShotTable::open(
            rom.as_deref(),
        ))),
        skill,
        ..ShooterConfig::default()
    };
    // the round is over for both bots once the victim was back on its feet (its own `respawned` event) and the
    // shooter took it up as a target again
    let gate = Gate::default();
    let (g1, g2) = (gate.clone(), gate.clone());
    let mut shooter_spec = BotSpec::new("Shooter", MotionKind::Circle);
    shooter_spec.shooter = Some(sc);
    // The shooter fires again the moment it has the victim back as a target. Quitting in the middle of a burst would
    // leave the last fire message queued but not delivered, and the server's pellet count (compared below with the
    // messages the bot sent) would be one short. So the end waits for 1 s of session clock without a new fire message.
    let (mut fired, mut quiet_since) = (0u32, 0u32);
    let a = spawn_bot(port, world.clone(), shooter_spec, move |b, _, t| {
        let Some(sh) = b.shooter.as_ref() else {
            return false;
        };
        if sh.stats.target_ups > 0 {
            g1.a.store(true, Ordering::Relaxed);
        }
        if sh.stats.fire_msgs != fired {
            fired = sh.stats.fire_msgs;
            quiet_since = t;
        }
        if g1.both() && t.wrapping_sub(quiet_since) >= 1000 {
            g1.done.store(true, Ordering::Relaxed);
        }
        g1.done.load(Ordering::Relaxed)
    });
    let b = spawn_bot(
        port,
        world.clone(),
        BotSpec::new("Target", MotionKind::Stand),
        move |_, rec, _| {
            if rec.count("respawned") > 0 {
                g2.b.store(true, Ordering::Relaxed);
            }
            g2.done.load(Ordering::Relaxed)
        },
    );
    let (shooter_run, target_run) = (a.join().unwrap(), b.join().unwrap());
    srv.quit();
    let _dump = fail_dump(&mut srv, &[&shooter_run, &target_run], &probe);
    assert!(
        shooter_run.reached && target_run.reached,
        "the kill, the respawn and the shooter's re-arm did not all happen within {BOT_CEILING:?}"
    );
    let (shooter, slog) = (&shooter_run.bot, &shooter_run.log);
    let (target, tlog) = (&target_run.bot, &target_run.log);
    let st = shooter.shooter.as_ref().expect("shooter state").stats;
    println!("shooter stats {st:?}");
    println!(
        "kills seen by the target {:?}, by the shooter {:?}",
        target.obs.kills, shooter.obs.kills
    );
    println!(
        "target life states {:?}; server lines {:?}",
        target.obs.life_states,
        srv.seen
            .iter()
            .filter(|l| l.contains("killed") || l.contains("spawned"))
            .collect::<Vec<_>>()
    );
    // the select pair and the shots went out in the 2009 forms
    let sel = events(slog, "weapon_select");
    assert!(!sel.is_empty(), "no weapon_select");
    assert_eq!(sel[0]["source"], "grant", "{:?}", sel[0]);
    assert_eq!(sel[0]["weapon"], 3, "the drawn grant (0xb3, -loadout 3,4)");
    assert!(
        shooter
            .obs
            .c2s_shell_counts
            .get(&0x7f)
            .copied()
            .unwrap_or(0)
            >= 1
    );
    assert!(
        shooter
            .obs
            .c2s_shell_counts
            .get(&0x76)
            .copied()
            .unwrap_or(0)
            >= 1
    );
    assert!(st.shots > 0 && st.fire_msgs >= st.shots, "{st:?}");
    // the server killed the target: 0x6d reached both, naming the shooter's pid as the killer
    let tpid = target.obs.client_id.map(|c| c.0 as u8).expect("target pid");
    let spid = shooter
        .obs
        .client_id
        .map(|c| c.0 as u8)
        .expect("shooter pid");
    assert!(
        target.obs.kills.contains(&(tpid, spid)),
        "target saw {:?} (pids target {tpid} shooter {spid})",
        target.obs.kills
    );
    assert!(shooter.obs.kills.contains(&(tpid, spid)));
    assert!(st.kills_by_me >= 1 && st.target_downs >= 1, "{st:?}");
    assert!(!events(tlog, "damage").is_empty(), "the victim got 0x74");
    // the victim died and respawned on the server's own timer, without asking (no armed wait, no fallback)
    let died = events(tlog, "died");
    let back = events(tlog, "respawned");
    assert!(
        !died.is_empty(),
        "victim never died: {:?}",
        target.obs.life_states
    );
    assert!(
        !back.is_empty(),
        "victim never respawned: {:?}",
        target.obs.life_states
    );
    // the timer is the server's: its own line gives the time from the death to the respawn on the server's clock
    // (7000 ms plus the lateness of its next pass). A bot's clock cannot judge this: a starved bot reads the death
    // late and the respawn on time. The bound widens by the longest stall this process saw.
    let on_server = srv.server_respawn_ms().unwrap_or_else(|| {
        panic!("no 'respawned by the server .. (no request)' line from the server")
    });
    let slack = 2_000 + 2 * u64::from(probe.worst_ms());
    assert!(
        (7000..=7000 + slack).contains(&on_server),
        "the server respawned the victim {on_server} ms after the death (timer 7000 ms, allowed lateness {slack} ms)"
    );
    assert_eq!(
        target.obs.c2s_shell_counts.get(&0x89),
        None,
        "no respawn request"
    );
    // the shooter took the target up again once it came back
    assert!(
        !events(slog, "target_up").is_empty() || st.target_ups > 0,
        "the shooter never re-armed: {st:?}"
    );
    assert!(shooter.obs.fatal.is_empty() && target.obs.fatal.is_empty());
    // the first kill came within a few seconds of the first shot
    let first_shot = slog.iter().find_map(|e| {
        (e["ev"] == "shot" || e["ev"] == "fire")
            .then(|| e["ms"].as_u64())
            .flatten()
    });
    let target_died = tlog
        .iter()
        .find(|e| e["ev"] == "died")
        .and_then(|e| e["ms"].as_u64());
    println!(
        "first shot at {first_shot:?} ms, the target died at {target_died:?} ms (target clock)"
    );
    // every bot pellet followed the 2009 cone through the server's check
    let line = srv
        .seen
        .iter()
        .find(|l| l.contains("session cone:"))
        .cloned()
        .unwrap_or_else(|| panic!("no session cone line; server said {:?}", srv.seen));
    println!("{line}");
    println!(
        "shooter: {} hit claims, {} miss reports, counter {}, table {}",
        st.hit_claims,
        st.miss_reports,
        shooter.shooter.as_ref().unwrap().shot_counter(),
        shooter.shooter.as_ref().unwrap().shot_table().source
    );
    let [pellets, ok, off, na, unchecked, unbounded, skew] =
        cone_counts(&line).unwrap_or_else(|| panic!("cone line {line:?}"));
    assert_eq!(pellets, u64::from(st.fire_msgs), "{line}");
    assert_eq!((off, na, unchecked), (0, 0, 0), "{line}");
    assert_eq!(ok, pellets, "{line}");
    if rom.is_some() {
        // the rows on both sides: every pellet checked against its row, the counter seeded by 0x7d and exact
        assert_eq!((unbounded, skew), (0, 0), "{line}");
        assert!(st.miss_reports + st.hit_claims == st.fire_msgs, "{st:?}");
        // two reads per pellet from the server's own_value, with the wrap at the table size: the cone drew
        let sh = shooter.shooter.as_ref().unwrap();
        let mut c = 9993;
        for _ in 0..st.fire_msgs {
            c = dc_cone::draw_pair(sh.shot_table(), c).2;
        }
        assert_eq!(sh.shot_counter(), c);
        assert_eq!(sh.shot_table().source, "rom");
    }
}

/// A timeline holds the shooter's fire, then lets exactly two shots go (`fire 2`), against the real server.
#[test]
fn timeline_holds_fire_then_fires_a_counted_burst() {
    let Some(exe) = dserver_exe() else {
        println!("skipped: dc-dserver binary not built (cargo build -p dc-dserver, or set DC_DSERVER_EXE)");
        return;
    };
    let _wd = Watchdog::arm("timeline_holds_fire_then_fires_a_counted_burst");
    let probe = LoadProbe::start();
    let world = synthetic_world();
    let (mut srv, port) = ready_server(
        &exe,
        &world,
        &[],
        &["user 1 1 1 0 0 Shooter", "user 2 2 2 1 0 Target"],
    );
    let gate = Gate::default();
    let (g1, g2) = (gate.clone(), gate.clone());
    let mut spec = BotSpec::new("Shooter", MotionKind::Stand);
    spec.shooter = Some(ShooterConfig::default());
    spec.timeline = Some(
        "[Shooter]\n@0 hold\n@2500 target target\n@2500 fire 2\n@3000 throw\n@3000 mark burst\n"
            .into(),
    );
    // done when the script ran out and both shots left; then 2 s more for a third shot that must not come
    let mut after = None;
    let a = spawn_bot(port, world.clone(), spec, move |b, _, t| {
        let done = b.timeline.as_ref().is_some_and(|p| p.remaining() == 0)
            && b.shooter.as_ref().is_some_and(|s| s.stats.shots >= 2);
        if done && settled(&mut after, t, 2000) {
            g1.a.store(true, Ordering::Relaxed);
        }
        g1.both()
    });
    // the target stays in the game until the shooter is done
    let b = spawn_bot(
        port,
        world.clone(),
        BotSpec::new("Target", MotionKind::Stand),
        move |_, _, _| {
            g2.b.store(true, Ordering::Relaxed);
            g2.both()
        },
    );
    let (shooter_run, target_run) = (a.join().unwrap(), b.join().unwrap());
    srv.quit();
    let _dump = fail_dump(&mut srv, &[&shooter_run, &target_run], &probe);
    assert!(
        shooter_run.reached,
        "the script and its two shots did not finish within {BOT_CEILING:?}"
    );
    let (shooter, slog) = (&shooter_run.bot, &shooter_run.log);
    let st = shooter.shooter.as_ref().expect("shooter state").stats;
    let verbs: Vec<_> = events(slog, "timeline")
        .iter()
        .map(|e| e["verb"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        verbs,
        ["hold", "target", "fire", "throw", "mark"],
        "{verbs:?}"
    );
    assert_eq!(st.shots, 2, "exactly the counted burst: {st:?}");
    assert_eq!(events(slog, "fire").len(), 2);
    assert_eq!(events(slog, "timeline_hold").len(), 1);
    // the first shot left after the hold ended (2.5 s on the timeline clock)
    let first_fire = events(slog, "fire")[0]["ms"].as_u64().unwrap();
    let fire_step = events(slog, "timeline")[2]["ms"].as_u64().unwrap();
    assert!(
        first_fire >= fire_step,
        "shot at {first_fire} before the fire step at {fire_step}"
    );
    assert!(shooter
        .timeline
        .as_ref()
        .is_some_and(|t| t.remaining() == 0));
}

/// `sync <nick>`: the timeline clock waits until the named player is spawned (here it starts 2 s later), and the bot
/// holds fire until then.
#[test]
fn timeline_sync_waits_for_the_named_player() {
    let Some(exe) = dserver_exe() else {
        println!("skipped: dc-dserver binary not built (cargo build -p dc-dserver, or set DC_DSERVER_EXE)");
        return;
    };
    let _wd = Watchdog::arm("timeline_sync_waits_for_the_named_player");
    let probe = LoadProbe::start();
    let world = synthetic_world();
    let (mut srv, port) = ready_server(
        &exe,
        &world,
        &[],
        &["user 1 1 1 0 0 Waiter", "user 2 2 2 1 0 Late"],
    );
    let gate = Gate::default();
    let (g1, g2) = (gate.clone(), gate.clone());
    let mut spec = BotSpec::new("Waiter", MotionKind::Stand);
    spec.shooter = Some(ShooterConfig::default());
    // the give-up time is long: on a loaded machine the late player can take many seconds to spawn
    spec.timeline = Some("[Waiter]\nsync late 120000\n@0 mark go\n".into());
    let t0 = dc_botclient::record::wall_secs();
    let mut after = None;
    let a = spawn_bot(port, world.clone(), spec, move |b, _, t| {
        if b.timeline.as_ref().is_some_and(|p| p.remaining() == 0) && settled(&mut after, t, 1000) {
            g1.a.store(true, Ordering::Relaxed);
        }
        g1.both()
    });
    let mut late = BotSpec::new("Late", MotionKind::Stand);
    late.delay_s = 2;
    let b = spawn_bot(port, world.clone(), late, move |_, _, _| {
        g2.b.store(true, Ordering::Relaxed);
        g2.both()
    });
    let (waiter_run, late_run) = (a.join().unwrap(), b.join().unwrap());
    srv.quit();
    let _dump = fail_dump(&mut srv, &[&waiter_run, &late_run], &probe);
    assert!(
        waiter_run.reached,
        "the waiter's timeline did not run within {BOT_CEILING:?}"
    );
    let (waiter, wlog) = (&waiter_run.bot, &waiter_run.log);
    let start = events(wlog, "timeline_start");
    assert_eq!(start.len(), 1, "{start:?}");
    assert_eq!(start[0]["sync"], true, "{start:?}");
    assert_eq!(start[0]["gave_up"], false, "{start:?}");
    let at = start[0]["ms"].as_u64().unwrap();
    // The late player did not even start before t0 + 2 s (wall clock), so a clock that started before that did
    // not wait for it. (The waiter's own ms cannot judge: its clock starts when its thread is first scheduled.)
    let started_wall = start[0]["wall"].as_f64().unwrap();
    assert!(
        started_wall >= t0 + 2.0,
        "the clock started {:.3} s after the test began, before the late player could join (at {at} ms)",
        started_wall - t0
    );
    let mark = events(wlog, "timeline");
    assert_eq!(mark.len(), 1);
    assert!(mark[0]["ms"].as_u64().unwrap() >= at);
    assert!(
        events(wlog, "fire")
            .iter()
            .all(|e| e["ms"].as_u64().unwrap() >= at),
        "no shot before the sync"
    );
    assert!(waiter.timeline.as_ref().is_some_and(|t| t.remaining() == 0));
}

/// BOT-44: after the round result (goal 1 reached by the first kill) the shooter never fires again, also not when
/// its re-acquire timer (9 s) brings the dead target back.
#[test]
fn no_shot_after_the_round_result() {
    let Some(exe) = dserver_exe() else {
        println!("skipped: dc-dserver binary not built (cargo build -p dc-dserver, or set DC_DSERVER_EXE)");
        return;
    };
    let _wd = Watchdog::arm("no_shot_after_the_round_result");
    let probe = LoadProbe::start();
    let world = synthetic_world();
    let (mut srv, port) = ready_server(
        &exe,
        &world,
        &["-goal", "1", "-damage", "200", "-linger", "30"],
        &["user 1 1 1 0 0 Shooter", "user 2 2 2 1 0 Target"],
    );
    let gate = Gate::default();
    let (g1, g2) = (gate.clone(), gate.clone());
    let mut spec = BotSpec::new("Shooter", MotionKind::Stand);
    spec.shooter = Some(ShooterConfig::default());
    spec.timeline = Some("[Shooter]\n@0 target target\n@0 fire\n".into());
    // done 10 s of the loop's clock after the round result was seen: past the 9 s re-acquire timer of the dead target
    let mut seen_at = None;
    let a = spawn_bot(port, world.clone(), spec, move |b, _, t| {
        if b.obs.reached("round_result") && settled(&mut seen_at, t, 10_000) {
            g1.a.store(true, Ordering::Relaxed);
        }
        g1.both()
    });
    let b = spawn_bot(
        port,
        world.clone(),
        BotSpec::new("Target", MotionKind::Stand),
        move |_, _, _| {
            g2.b.store(true, Ordering::Relaxed);
            g2.both()
        },
    );
    let (shooter_run, target_run) = (a.join().unwrap(), b.join().unwrap());
    srv.quit();
    let _dump = fail_dump(&mut srv, &[&shooter_run, &target_run], &probe);
    let slog = &shooter_run.log;
    let over = events(slog, "milestone")
        .iter()
        .find(|e| e["step"] == "round_result")
        .and_then(|e| e["ms"].as_u64())
        .unwrap_or_else(|| {
            panic!(
                "no round result within {BOT_CEILING:?}; fire events {:?}",
                events(slog, "fire")
            )
        });
    assert!(
        shooter_run.reached,
        "the run ended before 10 s after the round result"
    );
    let fires = events(slog, "fire");
    assert!(!fires.is_empty(), "the kill needs a shot");
    let late: Vec<_> = fires
        .iter()
        .filter(|e| e["ms"].as_u64().unwrap() > over)
        .collect();
    assert!(
        late.is_empty(),
        "shots after the round result at {over} ms: {late:?}"
    );
    // the bot is still connected well past the re-acquire timer of the dead target
    let last = slog.iter().filter_map(|e| e["ms"].as_u64()).max().unwrap();
    assert!(
        last > over + 9_500,
        "run ended at {last} ms, round over at {over} ms"
    );
}

/// BOT-45: a timeline `approach <nick>` logs `approach_reached` once the bot is within the stop distance.
#[test]
fn approach_reached_is_logged_once() {
    let Some(exe) = dserver_exe() else {
        println!("skipped: dc-dserver binary not built (cargo build -p dc-dserver, or set DC_DSERVER_EXE)");
        return;
    };
    let _wd = Watchdog::arm("approach_reached_is_logged_once");
    let probe = LoadProbe::start();
    let world = synthetic_world();
    let (mut srv, port) = ready_server(
        &exe,
        &world,
        &[],
        &["user 1 1 1 0 0 Walker", "user 2 2 2 0 0 Mate"],
    );
    let gate = Gate::default();
    let (g1, g2) = (gate.clone(), gate.clone());
    let mut spec = BotSpec::new("Walker", MotionKind::Hunt);
    spec.timeline = Some("[Walker]\n@500 approach mate\n".into());
    // done 2 s of the loop's clock after the first arrival: a second `approach_reached` would show by then
    let mut at = None;
    let a = spawn_bot(port, world.clone(), spec, move |_, rec, t| {
        if rec.count("approach_reached") > 0 && settled(&mut at, t, 2000) {
            g1.a.store(true, Ordering::Relaxed);
        }
        g1.both()
    });
    let b = spawn_bot(
        port,
        world.clone(),
        BotSpec::new("Mate", MotionKind::Stand),
        move |_, _, _| {
            g2.b.store(true, Ordering::Relaxed);
            g2.both()
        },
    );
    let (walker_run, mate_run) = (a.join().unwrap(), b.join().unwrap());
    srv.quit();
    let _dump = fail_dump(&mut srv, &[&walker_run, &mate_run], &probe);
    assert!(
        walker_run.reached,
        "the walker never reached the mate within {BOT_CEILING:?}"
    );
    let wlog = &walker_run.log;
    let got = events(wlog, "approach_reached");
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["nick"], "mate");
    assert!(got[0]["dist"].as_f64().unwrap() <= 96.0, "{got:?}");
    let step = events(wlog, "timeline")[0]["ms"].as_u64().unwrap();
    assert!(got[0]["ms"].as_u64().unwrap() >= step);
}

/// BOT-46: `goto x y z r` walks to within r of the point, logs `goto_reached` once, and the bot stays there.
#[test]
fn goto_with_a_radius_is_reached_and_logged() {
    let Some(exe) = dserver_exe() else {
        println!("skipped: dc-dserver binary not built (cargo build -p dc-dserver, or set DC_DSERVER_EXE)");
        return;
    };
    let _wd = Watchdog::arm("goto_with_a_radius_is_reached_and_logged");
    let probe = LoadProbe::start();
    let world = synthetic_world();
    let (mut srv, port) = ready_server(
        &exe,
        &world,
        &[],
        &["user 1 1 1 0 0 Walker", "user 2 2 2 1 0 Other"],
    );
    let gate = Gate::default();
    let (g1, g2) = (gate.clone(), gate.clone());
    let mut spec = BotSpec::new("Walker", MotionKind::Hunt);
    spec.shooter = Some(ShooterConfig::default());
    spec.timeline = Some("[Walker]\n@0 target other\n@0 fire\n@300 goto 120 50 -90 10\n".into());
    // done 1.5 s of the loop's clock after the arrival: the bot must still stand at the point then
    let mut at = None;
    let a = spawn_bot(port, world.clone(), spec, move |_, rec, t| {
        if rec.count("goto_reached") > 0 && settled(&mut at, t, 1500) {
            g1.a.store(true, Ordering::Relaxed);
        }
        g1.both()
    });
    let b = spawn_bot(
        port,
        world.clone(),
        BotSpec::new("Other", MotionKind::Stand),
        move |_, _, _| {
            g2.b.store(true, Ordering::Relaxed);
            g2.both()
        },
    );
    let (walker_run, other_run) = (a.join().unwrap(), b.join().unwrap());
    srv.quit();
    let _dump = fail_dump(&mut srv, &[&walker_run, &other_run], &probe);
    assert!(
        walker_run.reached,
        "the walker never reached its point within {BOT_CEILING:?}"
    );
    let (walker, wlog) = (&walker_run.bot, &walker_run.log);
    let got = events(wlog, "goto_reached");
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0]["dist"].as_f64().unwrap() <= 10.0, "{got:?}");
    // no shot on the way to the point (target and fire were set before the goto step)
    let arrived = got[0]["ms"].as_u64().unwrap();
    let goto_at = events(wlog, "timeline")
        .iter()
        .find(|e| e["verb"] == "goto")
        .and_then(|e| e["ms"].as_u64())
        .expect("goto step");
    let early: Vec<_> = events(wlog, "fire")
        .into_iter()
        .filter(|e| (goto_at..arrived).contains(&e["ms"].as_u64().unwrap()))
        .collect();
    assert!(
        arrived > goto_at + 200,
        "the walk took {} ms",
        arrived - goto_at
    );
    assert!(
        early.is_empty(),
        "shots before the arrival at {arrived} ms: {early:?}"
    );
    // still there at the end: within the radius plus one walking step
    let p = walker.body().expect("body").pos();
    let d = ((p[0] - 120.0).powi(2) + (p[2] + 90.0).powi(2)).sqrt();
    assert!(d <= 25.0, "the bot left the point: {p:?} ({d} units)");
}
