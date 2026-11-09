//! The bots the lobby runs itself (docs/BOTS.md "Bots the lobby runs"): with `bots.managed` a thread keeps, for every
//! enabled bot group of the dashboard (botgroups.rs), exactly its number of `dc-botclient` child processes running.
//!
//! Each bot logs in on this lobby through loopback by its name (no account and no password needed there, LG-32 /
//! LG-187), with the flags `tools/vps/bots.sh` gave it: the server ROM, the entry packs' cache, `--matches 0`, the
//! filler rule of its group's prefix (or the followed player), and its group's entry and channel (round-robin over the
//! group's channels). Its output goes to `<log dir>/bots/<name>.out`, its events to `<name>.events.jsonl`.
//!
//! A bot whose settings change restarts; one that ends starts again 10 s later, longer while it keeps ending fast. Its
//! stdin is a pipe (`--parent-pipe`): when the lobby ends, however it ends, the pipe closes and the bot exits, so none
//! is left behind. A graceful stop, a group stopped, deleted or made smaller close the pipe and kill what has not ended
//! 2 s later.
// Provenance: ORIGINAL (DeCombat operator tooling)
// DC-DEFAULT: LG-388 the supervisor, its flags, restarts and stop

use crate::botgroups::{self, Group, RoomPick, Spot, Steer};
use crate::state::{Core, Shared};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub type Handle = Arc<Mutex<Supervisor>>;

/// The supervisor's lock (a panic elsewhere never leaves the children unmanaged).
pub fn lock(h: &Handle) -> MutexGuard<'_, Supervisor> {
    h.lock().unwrap_or_else(|e| e.into_inner())
}

/// A bot that should run, with the full command line it runs with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub name: String,
    pub group: i64,
    pub channel: u32,
    pub args: Vec<String>,
}

/// Starts one bot process.
pub trait Launch: Send {
    fn launch(&self, name: &str, args: &[String]) -> std::io::Result<Child>;
}

/// A bot log above this size is started over (one older copy is kept as `<file>.1`).
const LOG_MAX: u64 = 5 * 1024 * 1024;

/// The real launcher: `dc-botclient` next to the lobby's own executable, output into `dir`.
pub struct BotExe {
    pub exe: PathBuf,
    pub dir: PathBuf,
}

/// What an ended bot shows: a clean exit is the bot's normal end (its hour ran, BOT-158), not an error.
fn ended_text(s: std::process::ExitStatus) -> String {
    if s.success() {
        "ran its hour".to_string()
    } else {
        crate::net::exit_text(s)
    }
}

fn rotate(p: &Path) {
    if std::fs::metadata(p).is_ok_and(|m| m.len() > LOG_MAX) {
        let mut old = p.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::rename(p, PathBuf::from(old));
    }
}

/// A child in a process group of its own, so an operator's Ctrl+C reaches the lobby only and the lobby ends its bots
/// (as the game servers, LG-130).
pub fn own_group(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
}

impl Launch for BotExe {
    fn launch(&self, name: &str, args: &[String]) -> std::io::Result<Child> {
        std::fs::create_dir_all(&self.dir)?;
        let out = self.dir.join(format!("{name}.out"));
        rotate(&out);
        rotate(&self.dir.join(format!("{name}.events.jsonl")));
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out)?;
        let err = log.try_clone()?;
        let mut cmd = Command::new(&self.exe);
        cmd.args(args).stdin(Stdio::piped()).stdout(log).stderr(err);
        own_group(&mut cmd);
        cmd.spawn()
    }
}

/// The supervisor's clock.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// The wait before a start again after an exit.
    pub restart_min: Duration,
    /// The longest wait while a bot keeps ending fast.
    pub restart_max: Duration,
    /// A bot that ran this long ended normally: its next wait is `restart_min` again.
    pub ran_well: Duration,
    /// After its pipe is closed, the time a bot has to end before it is killed.
    pub stop_grace: Duration,
    /// Between two starts (the lobby refuses connections above three TLS handshakes in progress, LG-223).
    pub stagger: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            restart_min: Duration::from_secs(10),
            restart_max: Duration::from_secs(300),
            ran_well: Duration::from_secs(120),
            stop_grace: Duration::from_secs(2),
            stagger: Duration::from_secs(2),
        }
    }
}

struct Proc {
    want: Want,
    child: Option<Child>,
    started: Instant,
    next_start: Instant,
    fast_exits: u32,
    restarts: u32,
    last_exit: String,
    note: String,
}

/// One bot's process as the dashboard shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcStatus {
    /// `running`, `starting` (due now), `backoff` (waiting after an exit) or `stopping`.
    pub state: &'static str,
    pub pid: Option<u32>,
    pub restarts: u32,
    pub uptime_secs: u64,
    pub retry_in_secs: u64,
    pub last_exit: String,
    /// The loadout's state for this bot.
    pub note: String,
    pub channel: u32,
    pub group: i64,
}

/// The bot processes of one lobby.
pub struct Supervisor {
    launch: Box<dyn Launch>,
    timing: Timing,
    procs: BTreeMap<String, Proc>,
    /// Ended on purpose: pipe closed, killed at the deadline.
    dying: Vec<(String, Child, Instant)>,
    stopped: bool,
    last_start: Option<Instant>,
}

impl Supervisor {
    pub fn new(launch: Box<dyn Launch>, timing: Timing) -> Supervisor {
        Supervisor {
            launch,
            timing,
            procs: BTreeMap::new(),
            dying: Vec::new(),
            stopped: false,
            last_start: None,
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    fn retire(&mut self, name: &str, mut c: Child, now: Instant) {
        drop(c.stdin.take());
        self.dying
            .push((name.to_string(), c, now + self.timing.stop_grace));
    }

    fn reap(&mut self, now: Instant) {
        self.dying.retain_mut(|(_, c, until)| {
            if matches!(c.try_wait(), Ok(Some(_))) {
                return false;
            }
            if now >= *until {
                let _ = c.kill();
                let _ = c.wait();
                return false;
            }
            true
        });
    }

    /// The wait after the `n`-th fast exit in a row (0: after a good run).
    fn wait_after(&self, fast_exits: u32) -> Duration {
        let t = &self.timing;
        let mut w = t.restart_min;
        for _ in 1..fast_exits.min(16) {
            w = (w * 2).min(t.restart_max);
        }
        w.min(t.restart_max.max(t.restart_min))
    }

    /// Makes the running bots those of `wants`: stops the ones not wanted any more, restarts the ones whose command
    /// line changed, notes exits, starts what is due. Returns lines for the lobby log.
    pub fn reconcile(&mut self, wants: Vec<Want>, now: Instant) -> Vec<String> {
        let mut lines = Vec::new();
        if self.stopped {
            return lines;
        }
        self.reap(now);
        let names: BTreeSet<String> = wants.iter().map(|w| w.name.clone()).collect();
        let gone: Vec<String> = self
            .procs
            .keys()
            .filter(|n| !names.contains(*n))
            .cloned()
            .collect();
        for n in gone {
            if let Some(mut p) = self.procs.remove(&n) {
                if let Some(c) = p.child.take() {
                    lines.push(format!("{n} stops (pid {})", c.id()));
                    self.retire(&n, c, now);
                }
            }
        }
        for w in wants {
            let name = w.name.clone();
            match self.procs.get_mut(&name) {
                Some(p) if p.want.args == w.args => p.want = w,
                Some(p) => {
                    p.want = w;
                    p.next_start = now;
                    p.fast_exits = 0;
                    if let Some(c) = p.child.take() {
                        lines.push(format!(
                            "{name} restarts for its new settings (pid {})",
                            c.id()
                        ));
                        self.retire(&name, c, now);
                    }
                }
                None => {
                    self.procs.insert(
                        name,
                        Proc {
                            want: w,
                            child: None,
                            started: now,
                            next_start: now,
                            fast_exits: 0,
                            restarts: 0,
                            last_exit: String::new(),
                            note: String::new(),
                        },
                    );
                }
            }
        }
        // exits
        let ran_well = self.timing.ran_well;
        let mut waits = Vec::new();
        for (n, p) in self.procs.iter_mut() {
            let Some(c) = p.child.as_mut() else { continue };
            let text = match c.try_wait() {
                Ok(None) => continue,
                Ok(Some(s)) => ended_text(s),
                Err(e) => format!("lost: {e}"),
            };
            p.child = None;
            p.restarts += 1;
            p.fast_exits = if now.duration_since(p.started) >= ran_well {
                0
            } else {
                p.fast_exits + 1
            };
            p.last_exit = text;
            waits.push((n.clone(), p.fast_exits));
        }
        for (n, fast) in waits {
            let w = self.wait_after(fast);
            if let Some(p) = self.procs.get_mut(&n) {
                p.next_start = now + w;
                lines.push(format!(
                    "{n} ended ({}), starts again in {} s",
                    p.last_exit,
                    w.as_secs()
                ));
            }
        }
        // starts: one name never runs twice, so a restarted bot waits for its old process to end
        let busy: BTreeSet<String> = self.dying.iter().map(|(n, _, _)| n.clone()).collect();
        let due: Vec<String> = self
            .procs
            .iter()
            .filter(|(n, p)| p.child.is_none() && now >= p.next_start && !busy.contains(*n))
            .map(|(n, _)| n.clone())
            .collect();
        for n in due {
            if self
                .last_start
                .is_some_and(|t| now < t + self.timing.stagger)
            {
                break;
            }
            let args = self.procs[&n].want.args.clone();
            let r = self.launch.launch(&n, &args);
            let fail_wait = self.wait_after(self.procs[&n].fast_exits + 1);
            let p = self.procs.get_mut(&n).expect("listed");
            match r {
                Ok(c) => {
                    lines.push(format!(
                        "{n} started (pid {}, channel {})",
                        c.id(),
                        p.want.channel
                    ));
                    p.child = Some(c);
                    p.started = now;
                    self.last_start = Some(now);
                }
                Err(e) => {
                    p.fast_exits += 1;
                    p.last_exit = format!("cannot start: {e}");
                    p.next_start = now + fail_wait;
                    lines.push(format!("{n}: {}", p.last_exit));
                }
            }
        }
        lines
    }

    /// Ends bot `name`'s process now (it starts again at once with the same flags); false when it was not running.
    pub fn restart(&mut self, name: &str, now: Instant) -> bool {
        let Some(p) = self.procs.get_mut(name) else {
            return false;
        };
        p.next_start = now;
        match p.child.take() {
            Some(c) => {
                self.retire(name, c, now);
                true
            }
            None => false,
        }
    }

    pub fn set_note(&mut self, name: &str, note: String) {
        if let Some(p) = self.procs.get_mut(name) {
            p.note = note;
        }
    }

    pub fn status(&self, now: Instant) -> BTreeMap<String, ProcStatus> {
        let mut out = BTreeMap::new();
        for (n, p) in &self.procs {
            let state = if p.child.is_some() {
                "running"
            } else if now < p.next_start {
                "backoff"
            } else {
                "starting"
            };
            out.insert(
                n.clone(),
                ProcStatus {
                    state,
                    pid: p.child.as_ref().map(Child::id),
                    restarts: p.restarts,
                    uptime_secs: if p.child.is_some() {
                        now.duration_since(p.started).as_secs()
                    } else {
                        0
                    },
                    retry_in_secs: p.next_start.saturating_duration_since(now).as_secs(),
                    last_exit: p.last_exit.clone(),
                    note: p.note.clone(),
                    channel: p.want.channel,
                    group: p.want.group,
                },
            );
        }
        for (n, c, _) in &self.dying {
            if out.get(n).is_none_or(|s| s.pid.is_none()) {
                let mut s = out.remove(n).unwrap_or(ProcStatus {
                    state: "stopping",
                    pid: None,
                    restarts: 0,
                    uptime_secs: 0,
                    retry_in_secs: 0,
                    last_exit: String::new(),
                    note: String::new(),
                    channel: 0,
                    group: 0,
                });
                s.state = "stopping";
                s.pid = Some(c.id());
                out.insert(n.clone(), s);
            }
        }
        out
    }

    /// The graceful stop: no more starts; every pipe is closed, what has not ended after the grace is killed. Returns
    /// the number of processes that were running.
    pub fn stop_all(&mut self) -> usize {
        self.stopped = true;
        let now = Instant::now();
        let names: Vec<String> = self.procs.keys().cloned().collect();
        for n in names {
            if let Some(c) = self.procs.get_mut(&n).and_then(|p| p.child.take()) {
                self.retire(&n, c, now);
            }
        }
        self.procs.clear();
        let n = self.dying.len();
        let until = now + self.timing.stop_grace;
        while !self.dying.is_empty() && Instant::now() < until {
            self.reap(Instant::now());
            std::thread::sleep(Duration::from_millis(25));
        }
        for (_, c, _) in self.dying.iter_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.dying.clear();
        n
    }
}

impl Drop for Supervisor {
    /// Never leaves a child behind (a test, or a lobby that ends without the graceful stop).
    fn drop(&mut self) {
        for p in self.procs.values_mut() {
            if let Some(c) = p.child.as_mut() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        for (_, c, _) in self.dying.iter_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

// ------------------------------------------------------------------------------------------------- the lobby's side

/// What every bot's command line takes from the lobby.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// This lobby's auth port through loopback.
    pub lobby: String,
    pub rom: Option<PathBuf>,
    /// The featured entries' unpacked packs (BOT-101).
    pub mod_cache: Option<PathBuf>,
    /// LG-271: the mod-set and content hash of a modded lobby's logins (None: vanilla).
    pub mods: Option<(String, String)>,
    /// The bots' log folder.
    pub dir: PathBuf,
}

/// `127.0.0.1:<port>` unless the lobby listens on one address only.
pub fn lobby_addr(bind: &str, port: u16) -> String {
    match bind.trim().parse::<std::net::IpAddr>() {
        Ok(ip) if !ip.is_unspecified() && ip.is_ipv6() => format!("[{ip}]:{port}"),
        Ok(ip) if !ip.is_unspecified() => format!("{ip}:{port}"),
        _ => format!("127.0.0.1:{port}"),
    }
}

/// The bots' log folder: `bots` beside the lobby's log, else beside its DB.
pub fn bots_dir(st: &Core) -> PathBuf {
    st.opts
        .log
        .as_ref()
        .and_then(|l| l.parent())
        .or_else(|| st.opts.db.parent())
        .map_or_else(|| PathBuf::from("bots"), |d| d.join("bots"))
}

pub fn facts(st: &Core, dir: &Path) -> Facts {
    let cache = st
        .opts
        .db
        .parent()
        .map(|d| d.join("entry_mods").join("cache"))
        .filter(|d| d.is_dir());
    let m = &st.login_mods;
    Facts {
        lobby: lobby_addr(&st.opts.bind, st.opts.port),
        rom: st.rom.dir.clone(),
        mod_cache: cache,
        mods: (*m != crate::state::ExpectedMods::default())
            .then(|| (m.set.clone(), m.content.clone())),
        dir: dir.to_path_buf(),
    }
}

/// One game session per start, as `bots.sh` (`DC_BOT_SESSION_S`): the bot ends after it and starts again.
pub const SESSION_SECS: u32 = 3600;

/// The flags of bot `name` of group `g` on `channel` (the flags `tools/vps/bots.sh` passed, plus the parent pipe).
pub fn bot_args(f: &Facts, g: &Group, name: &str, channel: u32) -> Vec<String> {
    let session = SESSION_SECS.to_string();
    let mut a: Vec<String> = [
        "--parent-pipe",
        "--lobby",
        &f.lobby,
        "--name",
        name,
        "--room",
        "any",
        "--duration",
        &session,
        "--match-wait",
        &session,
        "--shooter",
        "--hunt",
        "--respawn-after",
        "0",
        "--skill",
        &g.skill,
        "--matches",
        "0",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut add = |k: &str, v: String| {
        a.push(k.to_string());
        a.push(v);
    };
    match &g.room {
        // BOT-102: the filler rule with one name: only where that player sits
        RoomPick::Follow(n) => add("--filler", format!("={n}")),
        // BOT-84: a member without the group's prefix is a person
        RoomPick::Any | RoomPick::Title(_) => add("--filler", g.prefix.clone()),
    }
    if let RoomPick::Title(t) = &g.room {
        add("--room-title", t.clone());
    }
    add("--server-id", g.server_id.to_string());
    add("--channel", channel.to_string());
    if let Some(r) = &f.rom {
        add("--rom", r.display().to_string());
    }
    if let Some(c) = &f.mod_cache {
        add("--mod-cache", c.display().to_string());
    }
    if let Some((set, content)) = &f.mods {
        add("--mod-set-hash", set.clone());
        add("--content-hash", content.clone());
    }
    add(
        "--log",
        f.dir
            .join(format!("{name}.events.jsonl"))
            .display()
            .to_string(),
    );
    // BOT-157: the bot moves its own events file to `.1` at this size while it runs (the lobby does it at a start)
    add("--log-max", LOG_MAX.to_string());
    add(
        "--known-servers",
        f.dir.join("known_servers.txt").display().to_string(),
    );
    a
}

/// The bots of the enabled groups (none on a lobby that does not manage bots or is stopping), within the total cap
/// in group order (a cap lowered below what is stored leaves the later groups' bots out). A following group in
/// `spots` sits where its steering put it (OPS-15), on that entry's channels.
pub fn wants(st: &Core, groups: &[Group], f: &Facts, spots: &BTreeMap<i64, Spot>) -> Vec<Want> {
    if !st.opts.bots_managed || st.stopping {
        return Vec::new();
    }
    let entries = crate::servers::entries(st);
    let mut left = st.opts.bots_max_total;
    let mut out = Vec::new();
    for g in groups.iter().filter(|g| g.enabled) {
        let Ok(mut chs) = botgroups::usable_channels(g, &entries) else {
            continue;
        };
        let moved;
        let g = match spots.get(&g.id) {
            Some(s) => {
                chs = s.channels.clone();
                moved = Group {
                    server_id: s.entry,
                    ..g.clone()
                };
                &moved
            }
            None => g,
        };
        for (i, name) in g.names().into_iter().enumerate() {
            if left == 0 {
                return out;
            }
            left -= 1;
            let channel = Group::channel_of(&chs, i as u32 + 1);
            out.push(Want {
                args: bot_args(f, g, &name, channel),
                name,
                group: g.id,
                channel,
            });
        }
    }
    out
}

/// OPS-15: the steering of the following groups, kept by the supervisor thread between its rounds.
#[derive(Default)]
pub struct Follows {
    steer: HashMap<i64, Steer>,
}

impl Follows {
    /// The spot of each enabled following group this round (a move at most every `FOLLOW_MOVE_GAP`), and a log line
    /// per move. A group whose configured entry or channels change starts again from them.
    pub fn update(
        &mut self,
        st: &Core,
        groups: &[Group],
        now: Instant,
    ) -> (BTreeMap<i64, Spot>, Vec<String>) {
        let entries = crate::servers::entries(st);
        let mut spots = BTreeMap::new();
        let mut lines = Vec::new();
        let mut keep = BTreeSet::new();
        for g in groups.iter().filter(|g| g.enabled) {
            let RoomPick::Follow(nick) = &g.room else {
                continue;
            };
            let Ok(chs) = botgroups::usable_channels(g, &entries) else {
                continue;
            };
            let home = Spot {
                entry: g.server_id,
                channels: chs,
            };
            keep.insert(g.id);
            let s = self
                .steer
                .entry(g.id)
                .or_insert_with(|| Steer::new(home.clone()));
            if s.home != home {
                *s = Steer::new(home);
            }
            let place = botgroups::place_of_player(st, &entries, nick);
            if s.step(&place, now) {
                let to = entries
                    .iter()
                    .find(|e| e.id == s.now.entry)
                    .map_or_else(|| format!("entry {}", s.now.entry), |e| e.name.clone());
                lines.push(format!(
                    "group {} {} follows {nick} to {to} channel {}",
                    g.id,
                    g.prefix,
                    botgroups::channels_text(&s.now.channels)
                ));
            }
            spots.insert(g.id, s.now.clone());
        }
        self.steer.retain(|id, _| keep.contains(id));
        (spots, lines)
    }
}

/// The groups' loadouts (LG-251's `account loadout`), given to each bot account once per spec: after its first login
/// made the account, or at once when it has one. The bot then restarts so the kit counts.
#[derive(Default)]
pub struct Loadouts {
    done: HashMap<String, String>,
    /// login -> (spec, the group as it was when refused, why, when): retried after `RETRY_REFUSED`, or at once
    /// when the group is edited.
    failed: HashMap<String, (String, String, String, Instant)>,
    /// login -> when the database is read for it next (a bot waiting for its first login or a refused kit).
    next_check: HashMap<String, Instant>,
    /// logins with a lobby session at the last round (a new session is checked at once).
    online: BTreeSet<String>,
}

/// How long a refused loadout stays refused before the next try.
pub const RETRY_REFUSED: Duration = Duration::from_secs(600);
/// How often the database is read for a bot that has not got its kit yet (a lobby session appearing counts at once).
pub const LOADOUT_POLL: Duration = Duration::from_secs(10);

impl Loadouts {
    /// Returns the bots to restart and each wanted bot's loadout note.
    pub fn apply(
        &mut self,
        st: &Core,
        groups: &[Group],
        wants: &[Want],
    ) -> (Vec<String>, Vec<(String, String)>) {
        self.apply_at(st, groups, wants, Instant::now())
    }

    /// `apply` at the time `now`.
    pub fn apply_at(
        &mut self,
        st: &Core,
        groups: &[Group],
        wants: &[Want],
        now: Instant,
    ) -> (Vec<String>, Vec<(String, String)>) {
        let mut restart = Vec::new();
        let mut notes = Vec::new();
        let by_id: BTreeMap<i64, &Group> = groups.iter().map(|g| (g.id, g)).collect();
        for w in wants {
            let Some(g) = by_id.get(&w.group) else {
                continue;
            };
            let spec = g.loadout.as_str();
            if spec.is_empty() {
                notes.push((w.name.clone(), String::new()));
                continue;
            }
            // LG-438: the status names the vest the spec gives
            let shown = if spec.contains(':') {
                format!("{spec}{}", st.rom.vest_note(spec))
            } else {
                spec.to_string()
            };
            let login = w.name.to_ascii_lowercase();
            if self.done.get(&login).is_some_and(|s| s == spec) {
                notes.push((w.name.clone(), format!("loadout {shown}")));
                continue;
            }
            // a refusal stands until the group is edited or the retry time has passed
            let shape = format!("{g:?}");
            if let Some((s, gs, why, at)) = self.failed.get(&login) {
                if s == spec && *gs == shape && now < *at + RETRY_REFUSED {
                    notes.push((w.name.clone(), format!("loadout {s} refused: {why}")));
                    continue;
                }
                self.failed.remove(&login);
            }
            // the database is read when the bot's session appears, else every LOADOUT_POLL
            let online = botgroups::lobby_place(st, &w.name)["online"] == true;
            let fresh = if online {
                self.online.insert(login.clone())
            } else {
                self.online.remove(&login);
                false
            };
            if !fresh && self.next_check.get(&login).is_some_and(|t| now < *t) {
                notes.push((
                    w.name.clone(),
                    format!("loadout {shown} after the first login"),
                ));
                continue;
            }
            self.next_check.insert(login.clone(), now + LOADOUT_POLL);
            if botgroups::applied_loadout(&st.db, &login).is_ok_and(|s| s == spec) {
                self.done.insert(login, spec.to_string());
                notes.push((w.name.clone(), format!("loadout {shown}")));
                continue;
            }
            let has_char = crate::db::account_by_login(&st.db, &login)
                .ok()
                .flatten()
                .and_then(|a| crate::db::character(&st.db, a.user_no).ok().flatten())
                .is_some();
            if !has_char {
                notes.push((
                    w.name.clone(),
                    format!("loadout {shown} after the first login"),
                ));
                continue;
            }
            match crate::accounts::equip_loadout(&st.db, &st.rom, &login, spec, None) {
                Ok(line) => {
                    let _ = botgroups::set_applied_loadout(
                        &st.db,
                        &login,
                        spec,
                        crate::clock::unix_now(),
                    );
                    self.done.insert(login, spec.to_string());
                    st.note(&format!("bots: {line}"));
                    notes.push((w.name.clone(), format!("loadout {shown}")));
                    restart.push(w.name.clone());
                }
                Err(e) => {
                    st.note(&format!("bots: loadout {spec} for {}: {e}", w.name));
                    notes.push((w.name.clone(), format!("loadout {shown} refused: {e}")));
                    self.failed.insert(login, (spec.to_string(), shape, e, now));
                }
            }
        }
        (restart, notes)
    }
}

/// One round of the supervisor thread; false once the lobby stops.
pub fn tick(
    shared: &Shared,
    sup: &Handle,
    lo: &mut Loadouts,
    fo: &mut Follows,
    dir: &Path,
) -> bool {
    let now = Instant::now();
    let (wants, restart, notes, log) = {
        let g = crate::net::lock(shared);
        if g.stopping {
            return false;
        }
        let groups = match botgroups::list(&g.db) {
            Ok(v) => v,
            // a failed read changes nothing: the next round tries again
            Err(e) => {
                g.note(&format!("bots: reading the groups failed: {e}"));
                return true;
            }
        };
        let f = facts(&g, dir);
        let (spots, moves) = fo.update(&g, &groups, now);
        let w = wants(&g, &groups, &f, &spots);
        let (r, n) = lo.apply(&g, &groups, &w);
        for m in &moves {
            g.note(&format!("bots: {m}"));
        }
        (w, r, n, Arc::clone(&g.log))
    };
    let mut s = lock(sup);
    if s.is_stopped() {
        return false;
    }
    let mut lines = s.reconcile(wants, now);
    for n in restart {
        if s.restart(&n, now) {
            lines.push(format!("{n} restarts for its loadout"));
        }
    }
    for (n, t) in notes {
        s.set_note(&n, t);
    }
    drop(s);
    for l in lines {
        log.line(&format!("bots: {l}"));
    }
    true
}

/// `dc-botclient` beside this executable.
pub fn bot_exe() -> PathBuf {
    let name = format!("dc-botclient{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe().map_or_else(|_| PathBuf::from(&name), |p| p.with_file_name(&name))
}

/// Starts the supervisor thread on a lobby with `bots.managed` (nothing otherwise).
pub fn spawn(shared: &Shared) {
    let dir = {
        let g = crate::net::lock(shared);
        if !g.opts.bots_managed {
            return;
        }
        bots_dir(&g)
    };
    let exe = bot_exe();
    {
        let g = crate::net::lock(shared);
        g.note(&format!(
            "bots: managed by this lobby (bots.managed): {}{}, logs in {}, at most {} in all",
            exe.display(),
            if exe.is_file() {
                ""
            } else {
                " (MISSING: no bot can start)"
            },
            dir.display(),
            g.opts.bots_max_total
        ));
    }
    let sup: Handle = Arc::new(Mutex::new(Supervisor::new(
        Box::new(BotExe {
            exe,
            dir: dir.clone(),
        }),
        Timing::default(),
    )));
    crate::net::lock(shared).bot_sup = Some(Arc::clone(&sup));
    let shared = Arc::clone(shared);
    let _ = std::thread::Builder::new()
        .name("bots".into())
        .spawn(move || {
            let mut lo = Loadouts::default();
            let mut fo = Follows::default();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if !tick(&shared, &sup, &mut lo, &mut fo, &dir) {
                    break;
                }
            }
        });
}

/// The graceful stop's part (LG-130): every managed bot ends before the matches do.
pub fn shutdown(shared: &Shared) {
    let sup = crate::net::lock(shared).bot_sup.clone();
    if let Some(s) = sup {
        let n = lock(&s).stop_all();
        if n > 0 {
            crate::net::lock(shared).note(&format!("stopping: {n} managed bot(s) ended"));
        }
    }
}

#[cfg(test)]
#[path = "botsup_tests.rs"]
mod tests;
