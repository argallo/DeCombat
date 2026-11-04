//! Frame watchdog (fix-hang, extended by gate T1): a thread that looks at the frame thread's progress every
//! [`POLL_MS`]. Progress is a new frame, a pass of the event loop ([`pump`]) or a change of [`Phase`]. When there is
//! none for the phase's budget ([`Phase::budget_ms`]: 2 s in the menu and in play, 15 s while loading and while the
//! window rebuilds a lost GPU device, 10 s in teardown) it writes a STALL DIR `stall_<pid>_<n>` into the report folder:
//! - `stall.json`: pid, frame thread, phase, stalled ms, frame, last profiler stage, the threads inside a wait
//!   (from the breadcrumbs, `dc_hangdump::crumb`), the files of the dir;
//! - `threads.txt`: every thread's breadcrumbs (name, the wait it is in, the newest crumbs), the last [`EVENTS`]
//!   frame events, then every thread's stack (suspended only while copied, symbolized from the PDB);
//! - `events_tail.jsonl`: the last lines of the `--events` stream (when one is written);
//! - `dump1.dmp`: a minidump (thread info and indirectly referenced memory).
//!
//! When the stall lasts [`FINAL_EXTRA_MS`] past the budget (8 s in play) it writes `dump2.dmp` and
//! `threads2.txt` and ABORTS the process (`DC_WATCHDOG_ABORT=0` keeps it running). A lost GPU device
//! (crate::gpulost) whose frame did not come back is checked on every stall tick: a stall dir without minidump
//! (`stall.json` kind `gpu_lost_no_recovery`, `threads.txt`), the profiler's flight recorder, then exit code 3. Exit 3
//! is a failed run (the frame did not recover); only the window leaving the match by itself is a recovery.
//! The first stall dir is also copied to `dc-client.hang.txt` (the fix-hang report name).
//!
//! Locks: the watchdog thread takes no lock: the frame thread id, the phase and the counters are atomics; the report
//! folder, the pre-opened log file (`dc-client.watchdog.log`, opened by [`arm`]) and the events path are set once.
//! A report allocates (the stack sampler allocates its buffers before it suspends a thread). Files are written before
//! anything goes to stderr (a full pipe or the stderr lock must not block the report).
//!
//! Cost on the frame thread: one relaxed atomic add per frame and per loop pass, one relaxed store per profiler
//! lap. On by default in the windows; `DC_WATCHDOG=0` turns it off, `DC_WATCHDOG=1` also arms it in headless runs;
//! `DC_WATCHDOG_DIR` picks the report folder (default: the sound log's folder, else the working directory).
//!
//! Fault knob (gate T1): `DC_FAULT=deadlock_test` makes the frame thread and a helper thread `dc-fault-deadlock`
//! lock two mutexes in opposite order [`FAULT_AT_DEFAULT_S`] s after the first frame (`DC_FAULT_AT=<s>`): a real
//! deadlock whose stall dir names both threads and both waits.
//! Honoured only with `DC_AGENT=1` (the gate's launchers set it) or in a debug build.
// ORIGINAL: our own diagnostics tooling (fix-hang, gate T1); no 2009 behaviour
// DC-DEFAULT: CLI-461 the window's frame watchdog (amended by CLI-GATE-T1-1)
// DC-DEFAULT: CLI-GATE-T1-1 phase budgets, stall dir, second dump and abort, panic / exception reports
// PUBLIC: Microsoft Win32 docs (SuspendThread, GetThreadContext, StackWalk64, SymFromAddr, SymGetLineFromAddr64,
// MiniDumpWriteDump, CreateToolhelp32Snapshot, GetThreadDescription)

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{
    AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering::Relaxed,
};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How often the watchdog looks at the counters.
pub const POLL_MS: u64 = 500;
/// The stall that ends the process lasts this long past the phase's budget (8 s in play).
pub const FINAL_EXTRA_MS: u64 = 6000;
/// Frame events kept for the report.
pub const EVENTS: usize = 32;
/// Stall dirs written per process at most (a stall that recovers can come back).
pub const MAX_STALL_DIRS: u32 = 3;
/// A poll that came this much later than asked means the whole process (or the machine) was stopped: no stall.
const SUSPENDED_MS: u64 = 2000;
/// `DC_FAULT=deadlock_test` starts this long after the first frame unless `DC_FAULT_AT` says otherwise.
pub const FAULT_AT_DEFAULT_S: f64 = 5.0;

/// What the frame thread is doing; each phase has its own stall budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    /// Not watched (before the first window, between steps that do not draw).
    Idle = 0,
    Menu = 1,
    Loading = 2,
    Play = 3,
    Teardown = 4,
    /// A headless match loop.
    Headless = 5,
    /// The window rebuilds its GPU device inside the match (crate::gpulost, gpu-recover): the loading budget.
    Recovering = 6,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Menu => "menu",
            Phase::Loading => "loading",
            Phase::Play => "play",
            Phase::Teardown => "teardown",
            Phase::Headless => "headless",
            Phase::Recovering => "recovering",
        }
    }

    fn from_u8(v: u8) -> Phase {
        match v {
            1 => Phase::Menu,
            2 => Phase::Loading,
            3 => Phase::Play,
            4 => Phase::Teardown,
            5 => Phase::Headless,
            6 => Phase::Recovering,
            _ => Phase::Idle,
        }
    }

    /// The stall budget (ms); `None` = not watched.
    pub fn budget_ms(self) -> Option<u64> {
        match self {
            Phase::Idle => None,
            Phase::Menu | Phase::Play | Phase::Headless => Some(2000),
            Phase::Loading | Phase::Recovering => Some(15_000),
            Phase::Teardown => Some(10_000),
        }
    }
}

static FRAMES: AtomicU64 = AtomicU64::new(0);
static PUMPS: AtomicU64 = AtomicU64::new(0);
static BUSY: AtomicU64 = AtomicU64::new(0);
static PHASE: AtomicU8 = AtomicU8::new(0);
static PHASE_SEQ: AtomicU64 = AtomicU64::new(0);
static STAGE: AtomicUsize = AtomicUsize::new(usize::MAX);
static RING: [AtomicU64; EVENTS] = [const { AtomicU64::new(0) }; EVENTS];
static RING_POS: AtomicUsize = AtomicUsize::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static THREAD_ON: AtomicBool = AtomicBool::new(false);
static FRAME_TID: AtomicU32 = AtomicU32::new(0);
static STALLS: AtomicU32 = AtomicU32::new(0);
/// The stall the watchdog sees now (ms; 0 = none), for the heartbeat.
static STALLED_MS: AtomicU64 = AtomicU64::new(0);
static DIR: OnceLock<PathBuf> = OnceLock::new();
static LOG: OnceLock<std::fs::File> = OnceLock::new();
static EVENT_LOG: OnceLock<PathBuf> = OnceLock::new();
/// `DC_WATCHDOG_SELFTEST=<seconds>`: the frame thread sleeps [`SELFTEST_SLEEP_MS`] once at that window time (the
/// watchdog's own proof run).
static SELFTEST_AT_MS: AtomicU64 = AtomicU64::new(u64::MAX);
pub const SELFTEST_SLEEP_MS: u64 = 4500;
/// `DC_FAULT=deadlock_test`: the window time (ms) of the deadlock; `u64::MAX` = none (or done).
static FAULT_AT_MS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Event codes 0..=5 are the profiler's notes ([`crate::prof::NOTE_NAMES`]); these follow.
pub const EV_LIFE_STATE: u8 = 16;
pub const EV_REMOTES: u8 = 17;
pub const EV_DEAD_VIEW: u8 = 18;
pub const EV_FRAME_MS: u8 = 19;

fn ev_label(code: u8) -> String {
    match code {
        c if (c as usize) < crate::prof::NOTES => crate::prof::NOTE_NAMES[c as usize].to_string(),
        EV_LIFE_STATE => "life_state".into(),
        EV_REMOTES => "remotes".into(),
        EV_DEAD_VIEW => "death_view".into(),
        EV_FRAME_MS => "frame_ms".into(),
        c => format!("event{c}"),
    }
}

/// One frame went by (the window's redraw, a headless frame).
#[inline]
pub fn frame() {
    FRAMES.fetch_add(1, Relaxed);
}

/// One pass of the event loop went by (a window that does not draw now, e.g. minimized, is not stalled).
#[inline]
pub fn pump() {
    PUMPS.fetch_add(1, Relaxed);
}

/// A slow step on the frame thread finished (an inline ROM read or decode): progress, not a stall. A frame that
/// works through many such steps is slow but not hung, so it must not end the process (fix-first-match-drop: the
/// first sight of seven remote players read seven footstep rigs in one frame).
// DC-DEFAULT: CLI-1701 (proposed, project decision) a finished inline load step counts as progress
#[inline]
pub fn busy() {
    BUSY.fetch_add(1, Relaxed);
}

/// The frame thread enters `p` (a change restarts the stall clock).
#[inline]
pub fn phase(p: Phase) {
    if PHASE.swap(p as u8, Relaxed) != p as u8 {
        PHASE_SEQ.fetch_add(1, Relaxed);
    }
}

/// The phase now.
pub fn current_phase() -> Phase {
    Phase::from_u8(PHASE.load(Relaxed))
}

/// Frames counted so far.
pub fn frames() -> u64 {
    FRAMES.load(Relaxed)
}

/// Event-loop passes counted so far.
pub fn pumps() -> u64 {
    PUMPS.load(Relaxed)
}

/// Stall dirs written so far.
pub fn stalls() -> u32 {
    STALLS.load(Relaxed)
}

/// The stall going on now (ms), 0 when none.
pub fn stalled_ms() -> u64 {
    STALLED_MS.load(Relaxed)
}

/// Whether the watchdog watches now.
pub fn armed() -> bool {
    ARMED.load(Relaxed)
}

/// The frame thread finished stage `s` (the profiler's lap; `Stage as usize`).
#[inline]
pub fn stage(s: usize) {
    STAGE.store(s, Relaxed);
}

/// Notes a frame event (`value` is clipped to 24 bits); nothing while no window is armed.
#[inline]
pub fn event(code: u8, value: u64) {
    if !ARMED.load(Relaxed) {
        return;
    }
    let f = FRAMES.load(Relaxed) & 0xffff_ffff;
    let i = RING_POS.fetch_add(1, Relaxed) % EVENTS;
    RING[i].store(
        (f << 32) | ((code as u64) << 24) | value.min(0xff_ffff),
        Relaxed,
    );
}

fn env_flag(name: &str) -> Option<bool> {
    match std::env::var(name).ok().as_deref().map(str::trim) {
        Some("0" | "off" | "false" | "no") => Some(false),
        Some("1" | "on" | "true" | "yes") => Some(true),
        _ => None,
    }
}

/// Whether the watchdog runs in the windows (`DC_WATCHDOG=0` / `off` / `false` turns it off).
pub fn enabled() -> bool {
    env_flag("DC_WATCHDOG") != Some(false)
}

/// Whether the watchdog runs in a headless match: only when asked (`DC_WATCHDOG=1`) or for a fault test.
pub fn enabled_headless() -> bool {
    env_flag("DC_WATCHDOG") == Some(true) || fault_name().is_some()
}

fn abort_on_final() -> bool {
    env_flag("DC_WATCHDOG_ABORT") != Some(false)
}

/// The report folder: `DC_WATCHDOG_DIR`, else `dir`, else the working directory. Set once per process.
pub fn report_dir(dir: Option<PathBuf>) -> &'static Path {
    DIR.get_or_init(|| {
        std::env::var_os("DC_WATCHDOG_DIR")
            .map(PathBuf::from)
            .or(dir)
            .unwrap_or_else(|| PathBuf::from("."))
    })
}

/// The `--events` stream, for the stall dir's event tail.
pub fn set_event_log(path: &Path) {
    let _ = EVENT_LOG.set(path.to_path_buf());
}

/// Watches the calling thread (the frame thread) from now on; starts the watchdog thread on first use. `dir` is the
/// report folder's default (see [`report_dir`]). Also installs the panic report and the exception filter.
pub fn arm(dir: Option<PathBuf>) {
    let d = report_dir(dir).to_path_buf();
    install_crash_reports(&d);
    if LOG.get().is_none() {
        let _ = std::fs::create_dir_all(&d);
        if let Ok(f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(d.join("dc-client.watchdog.log"))
        {
            let _ = LOG.set(f);
        }
    }
    FRAME_TID.store(dc_hangdump::current_thread_id(), Relaxed);
    if let Some(at) = std::env::var("DC_WATCHDOG_SELFTEST")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
    {
        SELFTEST_AT_MS.store((at * 1000.0) as u64, Relaxed);
    }
    if fault_name() == Some("deadlock_test") && FAULT_AT_MS.load(Relaxed) == u64::MAX {
        let at = std::env::var("DC_FAULT_AT")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(FAULT_AT_DEFAULT_S);
        FAULT_AT_MS.store((at * 1000.0) as u64, Relaxed);
    }
    // DC_WATCHDOG=0: no watchdog thread (the reports and the fault knobs above stay: a hang without the in-process
    // watchdog, as an old build has, is what the external watchdog gate/hang.py must catch)
    if !enabled() {
        return;
    }
    ARMED.store(true, Relaxed);
    if !THREAD_ON.swap(true, Relaxed) {
        let spawned = std::thread::Builder::new()
            .name("dc-watchdog".into())
            .spawn(watch);
        if spawned.is_err() {
            THREAD_ON.store(false, Relaxed);
        }
    }
}

/// The self-test stall: at the window time `now_ms` given by `DC_WATCHDOG_SELFTEST` the calling (frame) thread
/// sleeps [`SELFTEST_SLEEP_MS`], once. One relaxed load otherwise.
#[inline]
pub fn selftest(now_ms: u64) {
    if now_ms >= SELFTEST_AT_MS.load(Relaxed) && SELFTEST_AT_MS.swap(u64::MAX, Relaxed) != u64::MAX
    {
        eprintln!(
            "dc-client watchdog: self-test stall of {SELFTEST_SLEEP_MS} ms on the frame thread"
        );
        std::thread::sleep(Duration::from_millis(SELFTEST_SLEEP_MS));
    }
    if now_ms >= FAULT_AT_MS.load(Relaxed) && FAULT_AT_MS.swap(u64::MAX, Relaxed) != u64::MAX {
        deadlock_test();
    }
}

/// The fault knob: only for test runs (`DC_AGENT=1`, set by the gate's launchers) or debug builds, so a stray
/// environment variable cannot deadlock a user's client.
pub(crate) fn fault_name() -> Option<&'static str> {
    static F: OnceLock<Option<String>> = OnceLock::new();
    F.get_or_init(|| {
        let allowed = cfg!(debug_assertions)
            || std::env::var("DC_AGENT").ok().as_deref().map(str::trim) == Some("1");
        if !allowed {
            return None;
        }
        std::env::var("DC_FAULT")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
    .as_deref()
}

/// `DC_FAULT=deadlock_test`: the calling (frame) thread and a helper take two locks in opposite order. Never returns.
fn deadlock_test() {
    use std::sync::{Arc, Barrier, Mutex};
    eprintln!("dc-client watchdog: DC_FAULT=deadlock_test: the frame thread deadlocks with dc-fault-deadlock now");
    let a = Arc::new(Mutex::new(0u32));
    let b = Arc::new(Mutex::new(0u32));
    let both = Arc::new(Barrier::new(2));
    let (a2, b2, both2) = (a.clone(), b.clone(), both.clone());
    let _ = std::thread::Builder::new()
        .name("dc-fault-deadlock".into())
        .spawn(move || {
            let _hb = dc_hangdump::crumb_wait!("fault: helper locks B", b2.lock());
            both2.wait();
            let _ha = dc_hangdump::crumb_wait!("fault: helper waits for A", a2.lock());
        });
    let _ga = dc_hangdump::crumb_wait!("fault: frame thread locks A", a.lock());
    both.wait();
    let _gb = dc_hangdump::crumb_wait!("fault: frame thread waits for B", b.lock());
    unreachable!("the deadlock test returned");
}

/// Stops watching (the window closed and nothing else watches the frame thread).
pub fn disarm() {
    ARMED.store(false, Relaxed);
    STALLED_MS.store(0, Relaxed);
}

/// What the stall test found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stall {
    /// The budget is over: write the stall dir (once per stall).
    First(u64),
    /// The stall lasted [`FINAL_EXTRA_MS`] past the budget: the second dump, then the end of the process.
    Final(u64),
}

/// The stall test of the watchdog thread, separate from the clock for the tests.
#[derive(Debug, Default)]
pub struct Monitor {
    last: Option<(u64, u64)>,
    level: u8,
}

impl Monitor {
    /// Feeds the progress key seen at `now_ms` with the phase's budget; reports a stall once per level.
    /// `budget` None (not armed, an unwatched phase) forgets the history.
    pub fn check(&mut self, budget: Option<u64>, progress: u64, now_ms: u64) -> Option<Stall> {
        let Some(budget) = budget else {
            self.last = None;
            self.level = 0;
            return None;
        };
        match self.last {
            Some((p, at)) if p == progress => {
                let stalled = now_ms.saturating_sub(at);
                if self.level < 2 && stalled >= budget + FINAL_EXTRA_MS {
                    self.level = 2;
                    Some(Stall::Final(stalled))
                } else if self.level < 1 && stalled >= budget {
                    self.level = 1;
                    Some(Stall::First(stalled))
                } else {
                    None
                }
            }
            _ => {
                self.last = Some((progress, now_ms));
                self.level = 0;
                None
            }
        }
    }

    /// The stall going on (ms), 0 when none.
    pub fn stalled(&self, now_ms: u64) -> u64 {
        match self.last {
            Some((_, at)) if self.level > 0 => now_ms.saturating_sub(at),
            _ => 0,
        }
    }

    /// Forgets the history (the process was suspended: the time between two polls is no stall).
    pub fn reset(&mut self) {
        self.last = None;
        self.level = 0;
    }
}

fn progress() -> u64 {
    FRAMES
        .load(Relaxed)
        .wrapping_add(PUMPS.load(Relaxed))
        .wrapping_add(BUSY.load(Relaxed) << 20)
        .wrapping_add(PHASE_SEQ.load(Relaxed) << 40)
}

fn budget() -> Option<u64> {
    if !ARMED.load(Relaxed) {
        return None;
    }
    current_phase().budget_ms()
}

fn watch() {
    let t0 = Instant::now();
    let mut m = Monitor::default();
    let mut last_poll = 0u64;
    let mut dir: Option<PathBuf> = None;
    loop {
        std::thread::sleep(Duration::from_millis(POLL_MS));
        let now = t0.elapsed().as_millis() as u64;
        if now.saturating_sub(last_poll) > POLL_MS + SUSPENDED_MS {
            m.reset();
        }
        last_poll = now;
        let found = m.check(budget(), progress(), now);
        STALLED_MS.store(m.stalled(now), Relaxed);
        let Some(s) = found else {
            continue;
        };
        // a lost GPU device whose unwinding did not come back (crate::gpulost): the frame did NOT recover. The
        // process ends with code 3 instead of a window that does not answer, every stall tick (even after an earlier
        // report), after a stall dir WITHOUT a minidump (the in-process dump could stall on the lost device) and the
        // profiler's flight recorder (DC_PROF_TAIL); exit 3 is a failed run for the gate, never a recovery
        // DC-DEFAULT: CLI-460, CLI-GATE-T1-1
        if crate::gpulost::is_lost() {
            let ms = match s {
                Stall::First(ms) | Stall::Final(ms) => ms,
            };
            let n = STALLS.fetch_add(1, Relaxed) + 1;
            if n <= MAX_STALL_DIRS {
                first_report(ms, n, Report::LostDevice);
            }
            flush_tail_bounded();
            if crate::net::last_gasp() {
                log_line("left the match: a plain bye went to the game server");
            }
            log_line("the GPU device was lost and the frame did not recover: exiting (code 3)");
            eprintln!("dc-client watchdog: the GPU device was lost and the frame did not recover: exiting (code 3)");
            std::process::exit(3);
        }
        match s {
            Stall::First(ms) => {
                let n = STALLS.fetch_add(1, Relaxed) + 1;
                if n <= MAX_STALL_DIRS {
                    dir = Some(first_report(ms, n, Report::Stall));
                } else {
                    // a 4th stall must not write its final files into the 3rd stall's dir
                    dir = None;
                    log_line(&format!(
                        "stall of {ms} ms ({}): no more stall dirs",
                        current_phase().name()
                    ));
                }
            }
            Stall::Final(ms) => {
                let d = dir.clone();
                final_report(ms, d.as_deref());
                if abort_on_final() {
                    // fix-first-match-drop: the game server hears the leave at once instead of 20 s of silence
                    // DC-DEFAULT: CLI-1702 (proposed, project decision) a plain bye before the watchdog ends the process
                    if crate::net::last_gasp() {
                        log_line("left the match: a plain bye went to the game server");
                        eprintln!("dc-client watchdog: the game froze, leaving the match (bye sent to the game server)");
                    }
                    log_line("aborting the process (the stall did not end)");
                    eprintln!("dc-client watchdog: the frame thread is still stalled after {ms} ms: aborting");
                    std::process::abort();
                }
            }
        }
    }
}

fn log_line(text: &str) {
    if let Some(mut f) = LOG.get() {
        let _ = writeln!(
            f,
            "[{} ms] dc-client watchdog: {text}",
            dc_hangdump::crumb::now_ms()
        );
    }
}

/// The ring's events, oldest first, as text.
fn events_text() -> Vec<String> {
    let pos = RING_POS.load(Relaxed);
    let n = pos.min(EVENTS);
    (0..n)
        .map(|k| RING[(pos - n + k) % EVENTS].load(Relaxed))
        .map(|v| {
            let frame = v >> 32;
            let code = ((v >> 24) & 0xff) as u8;
            let value = v & 0xff_ffff;
            format!("  frame {frame} {} {value}", ev_label(code))
        })
        .collect()
}

/// The last `n` lines of a text file (the event stream is flushed per line).
fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(64 * 1024);
    let _ = f.seek(SeekFrom::Start(from));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if from > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let k = lines.len().saturating_sub(n);
    lines.split_off(k)
}

/// The threads inside a wait, as JSON.
fn waits_json(threads: &[dc_hangdump::crumb::Thread], now: u64) -> serde_json::Value {
    serde_json::Value::Array(
        threads
            .iter()
            .filter_map(|t| {
                t.waiting().map(|e| {
                    serde_json::json!({
                        "tid": t.tid, "thread": t.name, "wait": e.what,
                        "for_ms": now.saturating_sub(e.at_ms), "at": format!("{}:{}", e.file, e.line),
                    })
                })
            })
            .collect(),
    )
}

/// The crumbs, the frame events and every stack, as the text of `threads.txt`.
fn threads_text(stalled_ms: u64, tid: u32) -> (String, serde_json::Value, Vec<serde_json::Value>) {
    let now = dc_hangdump::crumb::now_ms();
    let crumbs = dc_hangdump::crumb::snapshot();
    let st = STAGE.load(Relaxed);
    let stage = crate::prof::STAGE_NAMES
        .get(st)
        .copied()
        .unwrap_or("none yet");
    let mut out = vec![
        format!(
            "dc-client watchdog: the frame thread (tid {tid}) made no progress for {stalled_ms} ms in phase {} after frame {}",
            current_phase().name(),
            FRAMES.load(Relaxed)
        ),
        format!("  last finished profiler stage: {stage} (the stall is in the stage after it)"),
        "breadcrumbs (per thread, newest first):".into(),
    ];
    out.extend(dc_hangdump::crumb::lines(&crumbs, now));
    out.push(format!("last {EVENTS} frame events (oldest first):"));
    out.extend(events_text());
    out.push("thread stacks (the frame thread first):".into());
    out.extend(dc_hangdump::thread_stacks(tid));
    let threads: Vec<serde_json::Value> = crumbs
        .iter()
        .map(|t| {
            serde_json::json!({
                "tid": t.tid, "name": t.name,
                "waiting": t.waiting().map(|e| e.what),
                "last": t.crumbs.last().map(|e| e.what),
            })
        })
        .collect();
    (out.join("\n") + "\n", waits_json(&crumbs, now), threads)
}

/// What a stall dir reports.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Report {
    /// The frame thread stopped: minidump included.
    Stall,
    /// The GPU device was lost and the frame did not come back: no minidump (it could stall on the lost device).
    LostDevice,
}

/// The profiler's flight recorder (`DC_PROF_TAIL`) written now, from a helper thread waited for at most 1 s (its
/// writer takes the ring's lock; the watchdog thread itself takes none).
fn flush_tail_bounded() {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("dc-watchdog-tail".into())
        .spawn(move || {
            crate::prof::tail_flush();
            let _ = tx.send(());
        });
    if spawned.is_ok() {
        let _ = rx.recv_timeout(Duration::from_secs(1));
    }
}

fn first_report(stalled_ms: u64, n: u32, kind: Report) -> PathBuf {
    let tid = FRAME_TID.load(Relaxed);
    let base = DIR.get().cloned().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join(format!("stall_{}_{n}", std::process::id()));
    log_line(&format!(
        "stall of {stalled_ms} ms in phase {}: writing {}",
        current_phase().name(),
        dir.display()
    ));
    let _ = std::fs::create_dir_all(&dir);
    let (text, waits, threads) = threads_text(stalled_ms, tid);
    let _ = std::fs::write(dir.join("threads.txt"), &text);
    if n == 1 {
        let _ = std::fs::write(base.join("dc-client.hang.txt"), &text);
    }
    let tail = EVENT_LOG
        .get()
        .map(|p| tail_lines(p, 64))
        .unwrap_or_default();
    if !tail.is_empty() {
        let _ = std::fs::write(dir.join("events_tail.jsonl"), tail.join("\n") + "\n");
    }
    let dump = match kind {
        Report::LostDevice => serde_json::Value::from("skipped: GPU device lost"),
        Report::Stall => match dc_hangdump::write_minidump(&dir.join("dump1.dmp")) {
            Ok(()) => serde_json::Value::from("dump1.dmp"),
            Err(e) => serde_json::Value::from(format!("no minidump: {e}")),
        },
    };
    let stall = serde_json::json!({
        "kind": match kind { Report::Stall => "stall", Report::LostDevice => "gpu_lost_no_recovery" },
        "exit_code": (kind == Report::LostDevice).then_some(3),
        "pid": std::process::id(),
        "frame_tid": tid,
        "phase": current_phase().name(),
        "budget_ms": current_phase().budget_ms(),
        "stalled_ms": stalled_ms,
        "frame": FRAMES.load(Relaxed),
        "stage": crate::prof::STAGE_NAMES.get(STAGE.load(Relaxed)).copied(),
        "waiting": waits,
        "threads": threads,
        "dump": dump,
        "events_tail": !tail.is_empty(),
        "final": false,
    });
    let _ = std::fs::write(
        dir.join("stall.json"),
        serde_json::to_string_pretty(&stall).unwrap_or_default(),
    );
    log_line(&format!("stall dir {} written", dir.display()));
    eprintln!(
        "dc-client watchdog: the frame thread made no progress for {stalled_ms} ms ({}): stall dir {}",
        current_phase().name(),
        dir.display()
    );
    dir
}

fn final_report(stalled_ms: u64, dir: Option<&Path>) {
    let Some(dir) = dir else {
        return;
    };
    let tid = FRAME_TID.load(Relaxed);
    let (text, waits, _) = threads_text(stalled_ms, tid);
    let _ = std::fs::write(dir.join("threads2.txt"), &text);
    let dump = dc_hangdump::write_minidump(&dir.join("dump2.dmp")).is_ok();
    let path = dir.join("stall.json");
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(o) = v.as_object_mut() {
        o.insert("final".into(), true.into());
        o.insert("final_stalled_ms".into(), stalled_ms.into());
        o.insert("final_waiting".into(), waits);
        o.insert("dump2".into(), dump.into());
        o.insert("aborted".into(), abort_on_final().into());
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default());
    log_line(&format!("second dump in {}", dir.display()));
}

/// The panic report and the exception filter, once per process: a panic (other than the GPU-loss unwinding of
/// crate::gpulost) writes `panic_<pid>.txt` (message, place, thread, every thread's breadcrumbs) into `dir` before
/// the previous hook runs; an unhandled SEH exception writes `crash_<pid>/` (crash.txt, crash.dmp).
pub fn install_crash_reports(dir: &Path) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        dc_hangdump::crash::install(dir.to_path_buf());
        let dir = dir.to_path_buf();
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // after a device loss wgpu's errors on the lost device panic, and gpulost::guard turns them into the
            // recovery (GpuReset): no panic report for those
            if !info.payload().is::<crate::gpulost::DeviceLostUnwind>()
                && !crate::gpulost::is_lost()
            {
                let msg = info
                    .payload()
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| info.payload().downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "(no message)".into());
                let place = info
                    .location()
                    .map(|l| format!("{}:{}", l.file(), l.line()))
                    .unwrap_or_default();
                let thread = std::thread::current();
                let now = dc_hangdump::crumb::now_ms();
                let mut text = vec![format!(
                    "dc-client panic on thread \"{}\" at {place}: {msg}",
                    thread.name().unwrap_or("?")
                )];
                text.push("breadcrumbs (per thread, newest first):".into());
                text.extend(dc_hangdump::crumb::lines(
                    &dc_hangdump::crumb::snapshot(),
                    now,
                ));
                text.push(format!(
                    "backtrace:\n{}",
                    std::backtrace::Backtrace::force_capture()
                ));
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(
                    dir.join(format!("panic_{}.txt", std::process::id())),
                    text.join("\n") + "\n",
                );
            }
            prev(info);
        }));
    });
}

/// The stacks of every other thread, with `tid` first (tests, tools).
pub fn stacks_of(tid: u32) -> Vec<String> {
    dc_hangdump::thread_stacks(tid)
}

/// The calling thread's id (tests, tools).
pub fn thread_id() -> u32 {
    dc_hangdump::current_thread_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stall_is_reported_after_the_budget_then_once_more_at_the_final_mark() {
        let mut m = Monitor::default();
        let b = Some(2000);
        assert_eq!(m.check(b, 10, 0), None);
        assert_eq!(m.check(b, 10, 1999), None);
        assert_eq!(m.check(b, 10, 2000), Some(Stall::First(2000)));
        assert_eq!(m.check(b, 10, 2500), None);
        assert_eq!(m.stalled(2500), 2500);
        assert_eq!(m.check(b, 10, 8000), Some(Stall::Final(8000)));
        assert_eq!(m.check(b, 10, 9000), None);
        // progress restarts the clock and the levels
        assert_eq!(m.check(b, 11, 9500), None);
        assert_eq!(m.stalled(9500), 0);
        assert_eq!(m.check(b, 11, 11_400), None);
        assert_eq!(m.check(b, 11, 11_600), Some(Stall::First(2100)));
        // an unwatched phase is no stall, and watching again starts a new history
        assert_eq!(m.check(None, 11, 20_000), None);
        assert_eq!(m.check(b, 11, 20_500), None);
        assert_eq!(m.check(b, 11, 22_000), None);
    }

    #[test]
    fn each_phase_has_its_budget_and_a_phase_change_is_progress() {
        assert_eq!(Phase::Play.budget_ms(), Some(2000));
        assert_eq!(Phase::Menu.budget_ms(), Some(2000));
        assert_eq!(Phase::Loading.budget_ms(), Some(15_000));
        assert_eq!(Phase::Teardown.budget_ms(), Some(10_000));
        assert_eq!(Phase::Idle.budget_ms(), None);
        let mut m = Monitor::default();
        let before = progress();
        phase(Phase::Loading);
        phase(Phase::Play);
        assert_ne!(progress(), before);
        assert_eq!(m.check(Some(15_000), 1, 0), None);
        assert_eq!(m.check(Some(15_000), 1, 14_000), None);
        assert_eq!(m.check(Some(15_000), 1, 15_000), Some(Stall::First(15_000)));
        assert_eq!(m.check(Some(15_000), 1, 21_000), Some(Stall::Final(21_000)));
        phase(Phase::Idle);
    }

    #[test]
    fn events_keep_the_last_ring_full_oldest_first() {
        ARMED.store(true, Relaxed);
        for i in 0..(EVENTS as u64 + 5) {
            event(EV_REMOTES, i);
        }
        let t = events_text();
        assert_eq!(t.len(), EVENTS);
        assert!(t[0].ends_with("remotes 5"), "{}", t[0]);
        assert!(t[EVENTS - 1].ends_with(&format!("remotes {}", EVENTS + 4)));
        ARMED.store(false, Relaxed);
    }

    #[test]
    fn the_event_tail_is_the_last_lines_of_the_stream() {
        let dir = std::env::temp_dir().join(format!("dc-wd-tail-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("events.jsonl");
        let text: String = (0..100).map(|i| format!("{{\"seq\":{i}}}\n")).collect();
        std::fs::write(&p, text).unwrap();
        let t = tail_lines(&p, 3);
        assert_eq!(t, vec!["{\"seq\":97}", "{\"seq\":98}", "{\"seq\":99}"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
