//! Diagnostic reports for beta players (CLI-1840..CLI-1843, project decision; our own design, no 2009 counterpart).
//!
//! When a match link is lost, the server goes away, a join fails, the frame watchdog reports a stall, the GPU device
//! is lost, or the previous run crashed, the client can send the lobby's owner a report: the client version, the
//! machine, the settings that matter, the tails of the client's logs, the link's numbers and the stall / crash files,
//! with everything private taken out ([`redact`]). Whether that happens is the player's choice ([`Consent`]): always,
//! after a question, or never; the first problem asks (`menu::diagbox`). A report that cannot go now waits in the
//! profile folder ([`queue`]) and goes at the next start. The connection test ([`probe`]) tells the player in plain
//! words how the network to each server looks. The whole design: docs/DIAGNOSTICS.md.
//!
//! Threads: triggers come from the menu thread (after a match, at a stall or GPU loss it notices, at start-up for
//! the files of an earlier run) and from the match's chat (`/report`). Gathering, packing and the upload run on a
//! worker thread of their own; the menu reads the outcome lines ([`take_notices`]).
// DC-DEFAULT: CLI-1840 (proposed, USER REQUEST 2026-11-02) what a report holds; CLI-1841 consent; CLI-1842 queue and
// upload; CLI-1843 connection test. See DC_DEFAULTS.md and docs/DIAGNOSTICS.md.

pub mod bundle;
pub mod probe;
pub mod queue;
pub mod redact;
pub mod upload;

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// The lobby's web port is its auth port + this (dc-lobby-server `--clanweb` default); the echo uses the same number
/// over UDP.
pub const WEB_PORT_OFFSET: u16 = 8080;
/// The ledger of stall / crash files already handled (sent, declined or never to be sent).
pub const SEEN_FILE: &str = "diag_seen.txt";

/// The player's choice (profile `[DeCombat] diag.consent`: 0 not chosen, 1 always, 2 ask, 3 never).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Consent {
    /// Not chosen: reports go automatically until the player says otherwise (one notice line the first time).
    #[default]
    Unset,
    Always,
    Ask,
    Never,
}

impl Consent {
    pub fn from_u8(v: u8) -> Consent {
        match v {
            1 => Consent::Always,
            2 => Consent::Ask,
            3 => Consent::Never,
            _ => Consent::Unset,
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Consent::Unset => 0,
            Consent::Always => 1,
            Consent::Ask => 2,
            Consent::Never => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Consent::Unset => "not chosen",
            Consent::Always => "always",
            Consent::Ask => "ask each time",
            Consent::Never => "never",
        }
    }

    /// `/diag always|ask|never`.
    pub fn parse(s: &str) -> Option<Consent> {
        match s.trim().to_ascii_lowercase().as_str() {
            "always" | "on" | "auto" => Some(Consent::Always),
            "ask" => Some(Consent::Ask),
            "never" | "off" => Some(Consent::Never),
            _ => None,
        }
    }
}

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    LinkLost,
    ServerGone,
    Stall,
    Crash,
    Panic,
    GpuLost,
    JoinFailed,
    /// `/report`: the player asked.
    Manual,
    /// The bug report form's "attach diagnostics".
    BugReport,
    /// A connection test's result sent on its own.
    NetTest,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::LinkLost => "link_lost",
            Kind::ServerGone => "server_gone",
            Kind::Stall => "stall",
            Kind::Crash => "crash",
            Kind::Panic => "panic",
            Kind::GpuLost => "gpu_lost",
            Kind::JoinFailed => "join_failed",
            Kind::Manual => "manual",
            Kind::BugReport => "bug_report",
            Kind::NetTest => "nettest",
        }
    }

    /// The network failures: their box offers the connection test.
    pub fn is_network(self) -> bool {
        matches!(self, Kind::LinkLost | Kind::ServerGone | Kind::JoinFailed)
    }

    /// The player asked for it: sent whatever the consent says.
    pub fn is_explicit(self) -> bool {
        matches!(self, Kind::Manual | Kind::BugReport | Kind::NetTest)
    }

    /// The first words of the box that asks (our own text; the 2009 box of the failure came before it).
    pub fn what_happened(self) -> &'static str {
        match self {
            Kind::LinkLost => "The connection to the game server was lost.",
            Kind::ServerGone => "The game server ended the connection.",
            Kind::JoinFailed => "The game could not be joined.",
            Kind::Stall => "The game stopped responding for a while.",
            Kind::Crash | Kind::Panic => "The game closed unexpectedly last time.",
            Kind::GpuLost => "The graphics driver reset during the game.",
            Kind::Manual | Kind::BugReport | Kind::NetTest => "",
        }
    }
}

/// What the menu does with a problem, by the player's choice (docs/DIAGNOSTICS.md §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// A box: send / (test the connection) / no.
    Ask { offer_test: bool },
    /// Sent at once; a network failure's box still offers the connection test.
    Send { offer_test: bool },
    /// Nothing is gathered or sent.
    Drop,
}

/// The plan for a problem of `kind` under `consent` (`off`: `DC_DIAG=off`).
// DC-DEFAULT: CLI-1841 (proposed, USER REQUEST 2026-11-02; the test button's place by project decision; amended
// USER REQUEST 2026-11-07: not chosen = automatic)
pub fn plan(consent: Consent, kind: Kind, off: bool) -> Plan {
    if off {
        return Plan::Drop;
    }
    if kind.is_explicit() {
        return Plan::Send { offer_test: false };
    }
    let offer_test = kind.is_network();
    match consent {
        Consent::Unset | Consent::Always => Plan::Send { offer_test },
        Consent::Ask => Plan::Ask { offer_test },
        Consent::Never => Plan::Drop,
    }
}

/// One problem to report.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub kind: Kind,
    pub summary: String,
    pub created: i64,
    /// 16 hex digits.
    pub id: String,
    pub note: Option<String>,
    pub match_info: Option<Value>,
    pub link: Option<Value>,
    pub nettest: Option<Value>,
    /// Stall / crash folders and panic files that go in (and are then marked handled).
    pub artifacts: Vec<PathBuf>,
    /// The lobby (auth address) the report goes to; `None`: the current one.
    pub lobby: Option<SocketAddr>,
    /// Server addresses the report may name.
    pub keep_ips: Vec<IpAddr>,
}

impl Failure {
    pub fn new(kind: Kind, summary: impl Into<String>) -> Failure {
        Failure {
            kind,
            summary: summary.into(),
            created: now(),
            id: new_id(),
            note: None,
            match_info: None,
            link: None,
            nettest: None,
            artifacts: Vec::new(),
            lobby: None,
            keep_ips: Vec::new(),
        }
    }
}

/// Unix seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A fresh report id (16 hex digits): the clock and a counter through the standard library's random hasher keys.
pub fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(N.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

/// The web port (and echo) of a lobby whose auth address is `auth`.
pub fn web_of(auth: SocketAddr) -> Option<SocketAddr> {
    auth.port()
        .checked_add(WEB_PORT_OFFSET)
        .map(|p| SocketAddr::new(auth.ip(), p))
}

/// Where the reports look for files.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The folder of `dc-client.out` (the launcher's redirect): the `--net-log` file's folder, else the platform's.
    pub logs_dir: Option<PathBuf>,
    /// `--net-log`.
    pub net_log: Option<PathBuf>,
    /// The watchdog's report folder (stall dirs, crash folders, panic files).
    pub report_dir: Option<PathBuf>,
    /// The profile folder (queue, ledger).
    pub profile: Option<PathBuf>,
    /// `DC_DIAG=off`: nothing is gathered or sent.
    pub off: bool,
}

/// The platform's log folder of the launchers: `%LOCALAPPDATA%\DeCombat\logs`, `~/Library/Logs/DeCombat`.
pub fn default_logs_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("DC_LOG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d).join("DeCombat").join("logs"));
    }
    let home = std::env::var_os("HOME").filter(|d| !d.is_empty())?;
    Some(if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library").join("Logs").join("DeCombat")
    } else {
        PathBuf::from(home).join(".local").join("state").join("decombat")
    })
}

/// A connection test running or done (the server screen's panel, the failure box).
#[derive(Debug, Clone, Default)]
pub struct ProbeView {
    pub running: bool,
    /// Targets tested so far / in all.
    pub done: usize,
    pub total: usize,
    pub results: Vec<probe::Report>,
    /// Counts every change (the screen redraws when it moves).
    pub generation: u64,
}

#[derive(Default)]
struct Global {
    /// [`configure`] ran (the client's own start; tests and tools that never call it record nothing).
    armed: bool,
    cfg: Config,
    pending: VecDeque<Failure>,
    notices: VecDeque<String>,
    gpu: Option<Value>,
    settings: Value,
    login: Option<String>,
    lobby: Option<SocketAddr>,
    keep: Vec<IpAddr>,
    probe: ProbeView,
    stalls_seen: u32,
    gpu_seen: u64,
    bug_note: Option<String>,
}

fn global() -> MutexGuard<'static, Global> {
    static G: OnceLock<Mutex<Global>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(Global::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Sets the folders (once at start; `DC_DIAG=off` is read here).
pub fn configure(mut c: Config) {
    c.off |= std::env::var("DC_DIAG").is_ok_and(|v| v.eq_ignore_ascii_case("off") || v == "0");
    let mut g = global();
    g.cfg = c;
    g.armed = true;
    g.stalls_seen = crate::watchdog::stalls();
    g.gpu_seen = crate::gpulost::losses();
}

/// Reports are switched off for this run.
pub fn off() -> bool {
    global().cfg.off
}

/// [`configure`] ran and reports are not off: problems are recorded.
pub fn armed() -> bool {
    let g = global();
    g.armed && !g.cfg.off
}

/// The window's adapter (the report's GPU and driver).
pub fn note_gpu(info: &wgpu::AdapterInfo) {
    // the `run:` line of a crash or panic report names the adapter
    dc_hangdump::crash::add_context(&format!(
        "adapter {} ({:?}, {:?}); driver {} {}",
        info.name, info.backend, info.device_type, info.driver, info.driver_info
    ));
    global().gpu = Some(json!({
        "gpu": info.name,
        "driver": info.driver,
        "driver_info": info.driver_info,
        "backend": format!("{:?}", info.backend),
        "device_type": format!("{:?}", info.device_type),
        "vendor": format!("0x{:04x}", info.vendor),
        "device": format!("0x{:04x}", info.device),
    }));
}

/// What the menu knows (called now and then by the menu): the settings that matter, the login typed, the lobby,
/// the servers' addresses.
pub fn note_context(settings: Value, login: &str, lobby: Option<SocketAddr>, servers: Vec<IpAddr>) {
    let mut g = global();
    g.settings = settings;
    g.login = (!login.trim().is_empty()).then(|| login.trim().to_string());
    g.lobby = lobby;
    g.keep = servers;
}

/// Records a problem for the menu to handle (ask, send, or drop by the consent).
pub fn failure(f: Failure) {
    let mut g = global();
    if !g.armed || g.cfg.off {
        return;
    }
    // one of a kind waiting is enough (a stall that repeats asks once)
    if g.pending.iter().any(|p| p.kind == f.kind && !f.kind.is_explicit()) {
        return;
    }
    g.pending.push_back(f);
}

/// A bug report went from the report form (lobby or match): the menu asks whether to attach diagnostics.
pub fn note_bug_report(text: &str) {
    let mut g = global();
    if g.armed && !g.cfg.off {
        g.bug_note = Some(text.chars().take(bundle::NOTE_MAX).collect());
    }
}

/// The bug report's text waiting for the question.
pub fn take_bug_report() -> Option<String> {
    global().bug_note.take()
}

/// The next problem waiting for the menu.
pub fn take_pending() -> Option<Failure> {
    global().pending.pop_front()
}

/// Lines for the player (lobby chat): what became of a report.
pub fn take_notices() -> Vec<String> {
    global().notices.drain(..).collect()
}

/// CLI-1932: a line for the player's lobby chat from outside the reports (printed by the caller).
pub fn player_notice(s: String) {
    let mut g = global();
    if g.notices.len() < 16 {
        g.notices.push_back(s);
    }
}

fn notice(s: String) {
    println!("dc-client diag: {s}");
    let mut g = global();
    if g.notices.len() < 16 {
        g.notices.push_back(s);
    }
}

/// The menu's look for problems of this run it can see by counters: a watchdog stall (the stall dir written) and a
/// GPU device loss.
pub fn poll_counters() {
    let (stalls, gpu, quiet) = {
        let g = global();
        (crate::watchdog::stalls() > g.stalls_seen, crate::gpulost::losses() > g.gpu_seen, !g.armed || g.cfg.off)
    };
    if quiet || !(stalls || gpu) {
        return;
    }
    {
        let mut g = global();
        g.stalls_seen = crate::watchdog::stalls();
        g.gpu_seen = crate::gpulost::losses();
    }
    if stalls {
        let found = unreported(std::process::id(), true);
        let mut f = Failure::new(Kind::Stall, "the frame watchdog reported a stall");
        f.artifacts = found;
        failure(f);
    }
    if gpu {
        failure(Failure::new(Kind::GpuLost, "the GPU device was lost"));
    }
}

/// The ledger of handled files.
fn seen_path() -> Option<PathBuf> {
    global().cfg.profile.as_ref().map(|p| p.join(SEEN_FILE))
}

/// The names in ledger `file`.
pub fn seen_in(file: &Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

/// Adds `paths`' names to ledger `file` (kept to the last 200 names).
pub fn mark_seen_in(file: &Path, paths: &[PathBuf]) {
    let mut v = seen_in(file);
    for p in paths {
        if let Some(n) = p.file_name().map(|n| n.to_string_lossy().into_owned()) {
            if !v.contains(&n) {
                v.push(n);
            }
        }
    }
    let skip = v.len().saturating_sub(200);
    if let Some(d) = file.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(file, v[skip..].join("\n") + "\n");
}

/// Marks `paths` handled in the profile's ledger.
pub fn mark_seen(paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    if let Some(file) = seen_path() {
        mark_seen_in(&file, paths);
    }
}

/// The pid in `stall_<pid>_<n>`, `crash_<pid>`, `panic_<pid>.txt`.
pub fn artifact_pid(name: &str) -> Option<(u32, &'static str)> {
    let (kind, rest) = if let Some(r) = name.strip_prefix("stall_") {
        ("stall", r)
    } else if let Some(r) = name.strip_prefix("crash_") {
        ("crash", r)
    } else if let Some(r) = name.strip_prefix("panic_") {
        ("panic", r.strip_suffix(".txt")?)
    } else {
        return None;
    };
    let pid = rest.split('_').next()?.parse().ok()?;
    Some((pid, kind))
}

/// The stall dirs, crash folders (with a `crash.txt`) and panic files of report folder `dir` whose names are not in
/// `done`: of process `pid` (`own`), else of every other process.
pub fn unreported_in(dir: &Path, done: &[String], pid: u32, own: bool) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let (p, kind) = artifact_pid(&name)?;
            if (p == pid) != own || done.contains(&name) {
                return None;
            }
            let path = e.path();
            // an empty crash folder is a run that ended without the filter writing (killed, or still running)
            if kind == "crash" && !path.join("crash.txt").is_file() {
                return None;
            }
            Some(path)
        })
        .collect();
    v.sort();
    v
}

/// [`unreported_in`] for the configured folders.
pub fn unreported(pid: u32, own: bool) -> Vec<PathBuf> {
    let Some(dir) = global().cfg.report_dir.clone() else {
        return Vec::new();
    };
    let done = seen_path().map(|f| seen_in(&f)).unwrap_or_default();
    unreported_in(&dir, &done, pid, own)
}

/// The problem made of an earlier run's files (`None` when there are none).
pub fn earlier_failure(found: Vec<PathBuf>) -> Option<Failure> {
    if found.is_empty() {
        return None;
    }
    let names: Vec<String> = found
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let kind = if names.iter().any(|n| n.starts_with("crash_")) {
        Kind::Crash
    } else if names.iter().any(|n| n.starts_with("panic_")) {
        Kind::Panic
    } else {
        Kind::Stall
    };
    let mut f = Failure::new(kind, format!("found after the last run: {}", names.join(", ")));
    f.artifacts = found;
    Some(f)
}

/// At start: the crash, panic and stall files an earlier run left become one problem to report.
pub fn scan_earlier_runs() {
    if !armed() {
        return;
    }
    if let Some(f) = earlier_failure(unreported(std::process::id(), false)) {
        failure(f);
    }
}

/// The version of this build (`1.2.3`, `dev` for a development build).
fn version_words() -> String {
    crate::clientver::current().map_or("dev".to_string(), |v| format!("{}.{}.{}", v[0], v[1], v[2]))
}

/// The start of the `run:` line of a crash or panic report: version, commit, target, profile (the adapter is added
/// by [`note_gpu`]).
pub fn run_context() -> String {
    format!(
        "version {}; commit {}; {} {}",
        version_words(),
        option_env!("DC_GIT_COMMIT").unwrap_or("unknown"),
        env!("DC_TARGET_TRIPLE"),
        env!("DC_BUILD_PROFILE")
    )
}

/// The report's `client` block.
pub fn client_info() -> Value {
    json!({
        "version": version_words(),
        "commit": option_env!("DC_GIT_COMMIT").unwrap_or("unknown"),
        "target": env!("DC_TARGET_TRIPLE"),
        "profile": env!("DC_BUILD_PROFILE"),
        "pid": std::process::id(),
    })
}

/// The report's `system` block (the GPU from the window's adapter).
pub fn system_info() -> Value {
    let gpu = global().gpu.clone();
    let mut v = json!({
        "os": os_words(),
        "arch": std::env::consts::ARCH,
        "cpu": cpu_words(),
        "cores": std::thread::available_parallelism().map_or(0, |n| n.get()),
        "ram_mb": ram_mb(),
    });
    if let (Some(o), Some(Value::Object(g))) = (v.as_object_mut(), gpu) {
        o.extend(g);
    }
    v
}

fn os_words() -> String {
    if let Some(s) = dc_hangdump::sysinfo::os_version() {
        return s;
    }
    #[cfg(target_os = "macos")]
    if let Some(v) = command_line("sw_vers", &["-productVersion"]) {
        return format!("macOS {v}");
    }
    std::env::consts::OS.to_string()
}

fn cpu_words() -> Option<String> {
    if let Some(s) = dc_hangdump::sysinfo::cpu_name() {
        return Some(s);
    }
    #[cfg(target_os = "macos")]
    if let Some(v) = command_line("sysctl", &["-n", "machdep.cpu.brand_string"]) {
        return Some(v);
    }
    None
}

fn ram_mb() -> Option<u64> {
    if let Some(m) = dc_hangdump::sysinfo::ram_mb() {
        return Some(m);
    }
    #[cfg(target_os = "macos")]
    if let Some(v) = command_line("sysctl", &["-n", "hw.memsize"]).and_then(|v| v.parse::<u64>().ok()) {
        return Some(v / (1024 * 1024));
    }
    None
}

#[cfg(target_os = "macos")]
fn command_line(cmd: &str, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new(cmd).args(args).output().ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (o.status.success() && !s.is_empty()).then_some(s)
}

/// The files of a report (tails of the logs, the artifacts' texts).
fn gather_files(cfg: &Config, artifacts: &[PathBuf]) -> Vec<bundle::FileText> {
    let mut out = Vec::new();
    let mut add = |name: String, path: &Path, max: u64| {
        if let Some(text) = bundle::tail(path, max) {
            out.push(bundle::FileText { name, text });
        }
    };
    if let Some(d) = &cfg.logs_dir {
        add("dc-client.out".into(), &d.join("dc-client.out"), bundle::OUT_TAIL);
        add("dc-client.prev.out".into(), &d.join("dc-client.prev.out"), bundle::OUT_TAIL);
    }
    let net = cfg
        .net_log
        .clone()
        .or_else(|| cfg.logs_dir.as_ref().map(|d| d.join("dc-client.net.jsonl")));
    if let Some(n) = net {
        add("dc-client.net.jsonl".into(), &n, bundle::NET_TAIL);
        add("dc-client.net.menu.jsonl".into(), &n.with_extension("menu.jsonl"), bundle::MENU_TAIL);
    }
    if let Some(d) = &cfg.report_dir {
        add("dc-client.watchdog.log".into(), &d.join("dc-client.watchdog.log"), bundle::WATCHDOG_TAIL);
        add("dc-client.hang.txt".into(), &d.join("dc-client.hang.txt"), bundle::DIR_FILE_TAIL);
    }
    for a in artifacts {
        for (name, path) in bundle::artifact_files(a) {
            add(name, &path, bundle::DIR_FILE_TAIL);
        }
    }
    out
}

/// The cached identity token, when the player has one (CLI-1610).
fn token() -> Option<String> {
    let a = crate::identity::current()?;
    let c = crate::identity::load(a.cache.as_ref()?, crate::identity::now())?;
    Some(c.token)
}

/// Builds the packed report of `f` (on the calling thread; reads files).
pub fn build(f: &Failure) -> (Vec<u8>, Option<SocketAddr>) {
    let (cfg, settings, login, lobby, mut keep) = {
        let g = global();
        (g.cfg.clone(), g.settings.clone(), g.login.clone(), g.lobby, g.keep.clone())
    };
    let lobby = f.lobby.or(lobby);
    keep.extend(f.keep_ips.iter().copied());
    if let Some(l) = lobby {
        keep.push(l.ip());
    }
    let mut r = redact::Redactor::from_env(keep);
    if let Some(t) = token() {
        r.add_secret(&t);
    }
    let parts = bundle::Parts {
        id: f.id.clone(),
        created: f.created,
        kind: f.kind.name(),
        summary: f.summary.clone(),
        client: client_info(),
        system: system_info(),
        settings,
        match_info: f.match_info.clone(),
        link: f.link.clone(),
        nettest: f.nettest.clone().or_else(last_nettest),
        note: f.note.clone(),
        login,
        files: gather_files(&cfg, &f.artifacts),
    };
    (bundle::pack(&parts, &r), lobby.and_then(web_of))
}

/// The last connection test's results (for a report that has none of its own).
pub fn last_nettest() -> Option<Value> {
    let g = global();
    (!g.probe.results.is_empty() && !g.probe.running)
        .then(|| Value::Array(g.probe.results.iter().map(probe::Report::to_json).collect()))
}

/// Packs `f`, queues it and sends everything queued, on a worker thread; the outcome comes back as a notice line.
pub fn send(f: Failure) {
    if off() {
        notice("reports are off for this run (DC_DIAG=off)".into());
        return;
    }
    let _ = std::thread::Builder::new()
        .name("dc-diag-send".into())
        .spawn(move || {
            let (packed, target) = build(&f);
            let Some(target) = target else {
                // the files stay unhandled: the next start offers them again
                notice("no lobby to send the diagnostic report to".into());
                return;
            };
            mark_seen(&f.artifacts);
            let qdir = global().cfg.profile.as_ref().map(|p| queue::dir(p));
            match qdir {
                Some(q) => {
                    if let Err(e) = queue::push(&q, f.created, &f.id, &target.to_string(), &packed, now()) {
                        notice(format!("the report could not be queued: {e}"));
                        report_outcome(&f.id, upload_one(target, &packed));
                        return;
                    }
                    flush_queue(&q);
                }
                None => report_outcome(&f.id, upload_one(target, &packed)),
            }
        });
}

fn upload_one(target: SocketAddr, packed: &[u8]) -> upload::Outcome {
    let trust = crate::identity::trust_for(target);
    upload::send(target, &trust, packed, token().as_deref())
}

fn report_outcome(id: &str, o: upload::Outcome) {
    match o {
        upload::Outcome::Sent(n) => notice(format!("Diagnostic report sent (number {n}). Thank you.")),
        upload::Outcome::Rejected(e) => notice(format!("The server refused the diagnostic report ({e}).")),
        upload::Outcome::Later(e) => notice(format!(
            "The diagnostic report could not be sent now ({e}); it goes at the next start."
        )),
    }
    let _ = id;
}

/// Sends what waits in the queue (oldest first); a report that cannot go now stays.
pub fn flush_queue(dir: &Path) {
    queue::trim(dir, now());
    for e in queue::list(dir) {
        let Ok(target) = e.target.parse::<SocketAddr>() else {
            e.remove();
            continue;
        };
        let Some(bytes) = e.bytes() else {
            continue;
        };
        let o = upload_one(target, &bytes);
        let later = matches!(o, upload::Outcome::Later(_));
        if !later {
            e.remove();
        }
        report_outcome(&e.id, o);
        if later {
            // the same lobby will not take the next one either
            break;
        }
    }
}

/// At start: the queued reports of earlier runs go now (worker thread).
pub fn start() {
    if !armed() {
        return;
    }
    scan_earlier_runs();
    let q = global().cfg.profile.as_ref().map(|p| queue::dir(p));
    if let Some(q) = q.filter(|q| !queue::list(q).is_empty()) {
        let _ = std::thread::Builder::new()
            .name("dc-diag-queue".into())
            .spawn(move || flush_queue(&q));
    }
}

/// The connection test's state.
pub fn probe_view() -> ProbeView {
    global().probe.clone()
}

/// Starts a connection test of `targets` (one after another on a worker thread); `false` while one runs.
pub fn start_probe(targets: Vec<probe::Target>) -> bool {
    {
        let mut g = global();
        if g.probe.running || targets.is_empty() {
            return false;
        }
        g.probe = ProbeView {
            running: true,
            done: 0,
            total: targets.len(),
            results: Vec::new(),
            generation: g.probe.generation + 1,
        };
    }
    let _ = std::thread::Builder::new()
        .name("dc-diag-probe".into())
        .spawn(move || {
            for t in targets {
                let r = probe::run(&t);
                println!("dc-client nettest: {} ({}): {}", t.name, t.addr, r.words());
                let mut g = global();
                g.probe.results.push(r);
                g.probe.done += 1;
                g.probe.generation += 1;
            }
            let mut g = global();
            g.probe.running = false;
            g.probe.generation += 1;
        });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_round_trips_and_parses() {
        for c in [Consent::Unset, Consent::Always, Consent::Ask, Consent::Never] {
            assert_eq!(Consent::from_u8(c.as_u8()), c);
        }
        assert_eq!(Consent::from_u8(9), Consent::Unset);
        assert_eq!(Consent::parse(" Always "), Some(Consent::Always));
        assert_eq!(Consent::parse("ask"), Some(Consent::Ask));
        assert_eq!(Consent::parse("off"), Some(Consent::Never));
        assert_eq!(Consent::parse("maybe"), None);
    }

    /// Nothing chosen yet: a crash, panic, GPU loss or stall goes at once, with no box and no connection test.
    #[test]
    fn an_unchosen_consent_sends_the_crash_kinds_automatically() {
        for k in [Kind::Crash, Kind::Panic, Kind::GpuLost, Kind::Stall] {
            assert_eq!(plan(Consent::Unset, k, false), Plan::Send { offer_test: false }, "{k:?}");
            assert_eq!(plan(Consent::Unset, k, true), Plan::Drop, "{k:?} with DC_DIAG=off");
        }
    }

    #[test]
    fn the_plan_follows_the_consent() {
        use Kind::*;
        assert_eq!(plan(Consent::Unset, LinkLost, false), Plan::Send { offer_test: true });
        assert_eq!(plan(Consent::Unset, Stall, false), Plan::Send { offer_test: false });
        assert_eq!(plan(Consent::Always, LinkLost, false), Plan::Send { offer_test: true });
        assert_eq!(plan(Consent::Always, Stall, false), Plan::Send { offer_test: false });
        assert_eq!(plan(Consent::Ask, JoinFailed, false), Plan::Ask { offer_test: true });
        assert_eq!(plan(Consent::Ask, Crash, false), Plan::Ask { offer_test: false });
        assert_eq!(plan(Consent::Never, ServerGone, false), Plan::Drop);
        // what the player asks for goes whatever the consent, unless reports are off for the run
        for c in [Consent::Unset, Consent::Always, Consent::Ask, Consent::Never] {
            assert_eq!(plan(c, Manual, false), Plan::Send { offer_test: false });
            assert_eq!(plan(c, BugReport, false), Plan::Send { offer_test: false });
            assert_eq!(plan(c, Manual, true), Plan::Drop);
            assert_eq!(plan(c, LinkLost, true), Plan::Drop);
        }
    }

    /// The files of an earlier run become one problem, once: the ledger keeps them from being offered again, and
    /// the own process's files and a crash folder without a crash.txt are not taken.
    #[test]
    fn earlier_runs_are_found_once() {
        let base = std::env::temp_dir().join(format!("dc-diag-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let reports = base.join("reports");
        let ledger = base.join("profile").join(SEEN_FILE);
        std::fs::create_dir_all(reports.join("crash_11")).unwrap();
        std::fs::write(reports.join("crash_11/crash.txt"), "unhandled exception").unwrap();
        std::fs::create_dir_all(reports.join("crash_12")).unwrap();
        std::fs::create_dir_all(reports.join(format!("crash_{}", std::process::id()))).unwrap();
        std::fs::create_dir_all(reports.join("stall_11_1")).unwrap();
        std::fs::write(reports.join("panic_13.txt"), "panic").unwrap();
        let me = std::process::id();
        let f = earlier_failure(unreported_in(&reports, &seen_in(&ledger), me, false)).expect("a problem");
        assert_eq!(f.kind, Kind::Crash);
        let names: Vec<String> =
            f.artifacts.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["crash_11", "panic_13.txt", "stall_11_1"]);
        mark_seen_in(&ledger, &f.artifacts);
        assert!(earlier_failure(unreported_in(&reports, &seen_in(&ledger), me, false)).is_none());
        // the own process's stall dir is found by the in-run look only
        std::fs::create_dir_all(reports.join(format!("stall_{me}_1"))).unwrap();
        assert_eq!(unreported_in(&reports, &seen_in(&ledger), me, true).len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn artifact_names_carry_their_process() {
        assert_eq!(artifact_pid("stall_1234_2"), Some((1234, "stall")));
        assert_eq!(artifact_pid("crash_77"), Some((77, "crash")));
        assert_eq!(artifact_pid("panic_5.txt"), Some((5, "panic")));
        assert_eq!(artifact_pid("panic_5.log"), None);
        assert_eq!(artifact_pid("dc-client.out"), None);
    }

    #[test]
    fn ids_are_sixteen_hex_digits_and_differ() {
        let a = new_id();
        let b = new_id();
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert_eq!(web_of("1.2.3.4:10000".parse().unwrap()), Some("1.2.3.4:18080".parse().unwrap()));
    }
}
