//! Frame profiler of the headless and window loops (our own tooling; off unless asked for, and then nothing but a
//! flag test per lap site).
//!
//! Switched on by `dc-client --profile <json>` (or the environment variable `DC_PROF=<json>`). In profile mode the
//! headless loop renders EVERY frame into its offscreen target, saves no PNG, keeps `inflight` frames in flight
//! (like a swap chain; 0 = wait for the GPU after every frame) and composes the HUD only when it is due, as the
//! window does. The window loop books one record per redraw. At the end it prints and writes the JSON summary.
//!
//! Options (environment, all optional): `DC_PROF_INFLIGHT=N` (default 2), `DC_PROF_WARMUP=N` frames left out of the
//! statistics (default 30; the hitch list and the frame-time histogram keep them), `DC_PROF_READBACK=1` read every
//! frame back, `DC_PROF_PNG=1` also save the PNGs, `DC_PROF_TS=0` no GPU timestamps (default on when the adapter has
//! them: per-pass times on the normal in-flight path, see [`dc_render::perf`]), `DC_PROF_HUDFORCE=1` compose the HUD
//! on every frame (the old headless behaviour), `DC_PROF_FRAMES=<csv>` one line per frame (warm-up included):
//! stages, per-stage allocations, every counter, the frame's events and the GPU pass times,
//! `DC_PROF_MAX_FRAMES=N` frames kept for the statistics and the CSV (default 20000; later frames still count into
//! the frame-time histogram, the high-water marks and the hitch list), `DC_PROF_SCENARIO=<name>` a label for the
//! report (the stress runs set it), `DC_PROF_TAIL=<csv>` a flight recorder: the last [`TAIL_FRAMES`] frames in the
//! `DC_PROF_FRAMES` columns plus the wall clock, rewritten by a thread of its own every [`TAIL_EVERY_MS`] ms, so the
//! frames before a GPU reset or a hang are on disk even when the frame thread never returns (gpu-reset).
//!
//! perf-g1 (docs/RENDER_PERF_GATES.md G1): the summary gives p50 / p95 / p99 / max per stage, per counter and per GPU
//! pass, the high-water mark of every ledger row (`ledger`), the load phase (`load`: time, uploads, creates,
//! I/O / parse / decode / pack time), the runtime creates after the level load (`violations`), the hitch list
//! (`hitches`: every frame over [`HITCH_MS`] with its stages, counters and events, bounded) and the machine's CPU load
//! during the run (`cpu`). With the cargo feature `prof-alloc` the binary counts heap allocations per stage of the
//! frame thread ([`dc_render::perf::CountingAlloc`]), so "0 hot-loop allocations" is a number in the report.
// ORIGINAL: our own measurement tooling (render-profile, render-core, client-fps, perf-g1); no 2009 behaviour

use crate::frametimes::FrameTimes;
use dc_render::perf;
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

/// Heap allocations and bytes of every thread (counted only by [`dc_render::perf::CountingAlloc`]; 0 otherwise).
pub use dc_render::perf::{ALLOCS, ALLOC_BYTES};
/// Overlay counters (always kept: one relaxed add per upload / per batch).
pub static OVERLAY_UPLOAD_BYTES: AtomicU64 = AtomicU64::new(0);
pub static OVERLAY_QUADS: AtomicU64 = AtomicU64::new(0);
pub static OVERLAY_BUILDS: AtomicU64 = AtomicU64::new(0);
/// dc-render command counters (always kept: [`count_frame`], one relaxed add each per rendered frame; render-core).
pub static DRAW_CALLS: AtomicU64 = AtomicU64::new(0);
pub static BIND_SETS: AtomicU64 = AtomicU64::new(0);
pub static PIPELINE_SETS: AtomicU64 = AtomicU64::new(0);
pub static WORLD_DRAWS: AtomicU64 = AtomicU64::new(0);
pub static WORLD_CULLED: AtomicU64 = AtomicU64::new(0);
pub static MODEL_UNIFORM_BYTES: AtomicU64 = AtomicU64::new(0);
pub static MODEL_INSTANCES: AtomicU64 = AtomicU64::new(0);
/// Model uploads finished, inline or handed back by the loader threads (always counted; the profiler's per-frame
/// `model_loads` is dc-render's [`perf::Counter::ModelLoads`], booked at the same sites).
pub static MODEL_LOADS: AtomicU64 = AtomicU64::new(0);
/// World triangles drawn (perf-g1).
pub static WORLD_TRIS: AtomicU64 = AtomicU64::new(0);
/// Effect quads drawn (perf-g1; the particle / sprite load of a frame).
pub static FX_QUADS: AtomicU64 = AtomicU64::new(0);

/// Heap allocations made by the calling thread so far (counted only by [`dc_render::perf::CountingAlloc`]).
pub fn thread_allocs() -> u64 {
    dc_render::perf::thread_allocs()
}

/// Books dc-render's counters of one frame's world passes (render-core).
pub fn count_frame(r: &dc_render::FrameReport) {
    let add = |c: &AtomicU64, v: u64| {
        c.fetch_add(v, Ordering::Relaxed);
    };
    add(&DRAW_CALLS, r.draw_calls);
    add(&BIND_SETS, r.bind_sets);
    add(&PIPELINE_SETS, r.pipeline_sets);
    add(&WORLD_DRAWS, r.world_draws);
    add(&WORLD_CULLED, r.world_culled);
    add(&MODEL_UNIFORM_BYTES, r.uniform_bytes);
    add(&MODEL_INSTANCES, r.models + r.view_models);
    add(&WORLD_TRIS, r.world_tris);
}

/// The stages of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Input = 0,
    Net,
    Sim,
    Arms,
    HudUpdate,
    Sound,
    Panel,
    HudCompose,
    HudUpload,
    Models,
    Drops,
    ViewWeapon,
    Encode,
    Submit,
    GpuWait,
    Readback,
    Png,
    /// Window: the swap-chain image acquire.
    Acquire,
    /// Window: present.
    Present,
    /// Window: the event loop between two redraws.
    Loop,
    /// The effect player's frame (groups, particles, tracers).
    Fx,
    /// Window only (client-fps): the HUD's per-frame inputs (case marks, name marks, radar).
    HudMarks,
    /// Window only (client-fps): the HUD display list built when it is due (`hud_compose` is then the quad build
    /// and its upload).
    HudItems,
}

pub const STAGES: usize = 23;
pub const STAGE_NAMES: [&str; STAGES] = [
    "input",
    "net",
    "sim",
    "arms",
    "hud_update",
    "sound",
    "panel",
    "hud_compose",
    "hud_upload",
    "models_remotes",
    "drops",
    "view_weapon",
    "encode",
    "submit",
    "gpu_wait",
    "readback",
    "png",
    "acquire",
    "present",
    "loop",
    "fx",
    "hud_marks",
    "hud_items",
];

/// Events of a frame the loop notes (inputs applied), for the hitch attribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    /// HUD inputs (server messages turned into HUD events).
    HudEvents = 0,
    /// Weapon-effect events (shots, records, placed effects).
    FxEvents,
    /// Sound cues.
    SoundCues,
    /// Server corrections applied.
    Corrections,
    /// Window events handled since the last redraw.
    WindowEvents,
    /// Simulation ticks run.
    Ticks,
}

pub const NOTES: usize = 6;
pub const NOTE_NAMES: [&str; NOTES] = [
    "hud_events",
    "fx_events",
    "sound_cues",
    "corrections",
    "window_events",
    "ticks",
];

/// GPU passes with a column of their own (any other name is booked to `other`).
pub const GPU_PASSES: [&str; 8] = [
    "sky", "world", "boxes", "fx", "hud", "panel", "gamma", "other",
];

/// Profile settings.
#[derive(Debug, Clone)]
pub struct Config {
    pub out: Option<PathBuf>,
    pub inflight: usize,
    pub warmup: usize,
    pub readback: bool,
    pub png: bool,
    pub timestamps: bool,
    pub hud_force: bool,
    pub max_frames: usize,
    pub scenario: String,
}

impl Config {
    /// The settings from the environment, with `out` as the report path (`None`: `DC_PROF`, or off).
    pub fn from_env(out: Option<PathBuf>) -> Config {
        let flag = |k: &str| std::env::var(k).is_ok_and(|v| v == "1");
        let num = |k: &str, d: usize| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d)
        };
        Config {
            out: out.or_else(|| std::env::var_os("DC_PROF").map(PathBuf::from)),
            inflight: num("DC_PROF_INFLIGHT", 2),
            warmup: num("DC_PROF_WARMUP", 30),
            readback: flag("DC_PROF_READBACK"),
            png: flag("DC_PROF_PNG"),
            timestamps: std::env::var("DC_PROF_TS").map_or(true, |v| v != "0"),
            hud_force: flag("DC_PROF_HUDFORCE"),
            // DC_PROF_MAX_RECORDS: client-fps's name of the same bound
            max_frames: num("DC_PROF_MAX_FRAMES", num("DC_PROF_MAX_RECORDS", 20_000))
                .clamp(1, 2_000_000),
            scenario: std::env::var("DC_PROF_SCENARIO").unwrap_or_default(),
        }
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();
static SCENARIO: OnceLock<String> = OnceLock::new();

/// Names the run's scenario in the report (the stress runs; the first name wins; `DC_PROF_SCENARIO` otherwise).
pub fn set_scenario(name: &str) {
    let _ = SCENARIO.set(name.to_string());
}
static ENABLED_AT: OnceLock<Instant> = OnceLock::new();

fn init(c: Config) -> Config {
    if c.out.is_some() {
        let _ = ENABLED_AT.set(Instant::now());
        perf::enable(c.timestamps);
    }
    c
}

/// Turns the profiler on with a report path (call before the run and before any GPU device is created, so the
/// device asks for timestamps; the first setting wins).
pub fn enable(out: PathBuf) {
    let _ = CONFIG.set(init(Config::from_env(Some(out))));
}

/// The settings (from the environment when [`enable`] was not called).
pub fn config() -> &'static Config {
    CONFIG.get_or_init(|| init(Config::from_env(None)))
}

/// The profiler is on.
pub fn on() -> bool {
    config().out.is_some()
}

#[derive(Clone, Default)]
struct Current {
    ms: [f64; STAGES],
    allocs: [u64; STAGES],
    notes: [u64; NOTES],
}

thread_local! {
    static CURRENT: RefCell<Current> = RefCell::new(Current::default());
}

/// Adds `ms` to stage `s` of the current frame.
pub fn add(s: Stage, ms: f64) {
    CURRENT.with(|c| c.borrow_mut().ms[s as usize] += ms);
}

/// Notes `n` events of kind `k` in the current frame (nothing while the profiler is off).
pub fn note(k: Note, n: usize) {
    if n > 0 {
        crate::watchdog::event(k as u8, n as u64);
    }
    if n > 0 && on() {
        CURRENT.with(|c| c.borrow_mut().notes[k as usize] += n as u64);
    }
}

/// A lap clock: `None` when the profiler is off (no clock is read then).
#[derive(Clone, Copy, Debug)]
pub struct Laps(Option<(Instant, u64)>);

impl Laps {
    pub fn start() -> Laps {
        Laps(on().then(|| (Instant::now(), thread_allocs())))
    }

    /// Time (and the calling thread's allocations) since the last lap go to `s`.
    pub fn lap(&mut self, s: Stage) {
        crate::watchdog::stage(s as usize);
        if let Some((t, a)) = self.0 {
            let now = Instant::now();
            let an = thread_allocs();
            CURRENT.with(|c| {
                let mut c = c.borrow_mut();
                c.ms[s as usize] += (now - t).as_secs_f64() * 1000.0;
                c.allocs[s as usize] += an.wrapping_sub(a);
            });
            self.0 = Some((now, an));
        }
    }

    /// Restarts the clock without booking the time.
    pub fn reset(&mut self) {
        if self.0.is_some() {
            self.0 = Some((Instant::now(), thread_allocs()));
        }
    }
}

const OWN_COUNTERS: usize = 14;
const OWN_COUNTER_NAMES: [&str; OWN_COUNTERS] = [
    "allocs",
    "alloc_bytes",
    "overlay_upload_bytes",
    "overlay_quads",
    "overlay_builds",
    "draw_calls",
    "bind_group_sets",
    "pipeline_sets",
    "world_draws",
    "world_culled",
    "model_uniform_bytes",
    "model_instances",
    "world_tris",
    "fx_quads",
];
/// A counting allocator is installed (the binary's `prof-alloc` feature, or a test's own): allocations were seen.
fn allocs_counted() -> bool {
    ALLOCS.load(Ordering::Relaxed) > 0
}

/// Every counter of a record: this crate's, then dc-render's [`perf::COUNTER_NAMES`].
pub const COUNTERS: usize = OWN_COUNTERS + perf::COUNTERS;

/// The name of counter `i` of a record.
pub fn counter_name(i: usize) -> &'static str {
    if i < OWN_COUNTERS {
        OWN_COUNTER_NAMES[i]
    } else {
        perf::COUNTER_NAMES[i - OWN_COUNTERS]
    }
}

fn counter_index(name: &str) -> usize {
    (0..COUNTERS)
        .find(|&i| counter_name(i) == name)
        .unwrap_or(0)
}

fn counters() -> [u64; COUNTERS] {
    let r = |c: &AtomicU64| c.load(Ordering::Relaxed);
    let own = [
        r(&ALLOCS),
        r(&ALLOC_BYTES),
        r(&OVERLAY_UPLOAD_BYTES),
        r(&OVERLAY_QUADS),
        r(&OVERLAY_BUILDS),
        r(&DRAW_CALLS),
        r(&BIND_SETS),
        r(&PIPELINE_SETS),
        r(&WORLD_DRAWS),
        r(&WORLD_CULLED),
        r(&MODEL_UNIFORM_BYTES),
        r(&MODEL_INSTANCES),
        r(&WORLD_TRIS),
        r(&FX_QUADS),
    ];
    let pc = perf::counters();
    std::array::from_fn(|i| {
        if i < OWN_COUNTERS {
            own[i]
        } else {
            pc[i - OWN_COUNTERS]
        }
    })
}

/// Frames the `DC_PROF_TAIL` flight recorder keeps (about 2 s at 1000 fps).
pub const TAIL_FRAMES: usize = 4096;
/// How often the flight recorder's thread rewrites its file.
pub const TAIL_EVERY_MS: u64 = 500;

/// The flight recorder's ring: (wall clock in ms since the Unix epoch, record), oldest first.
type TailRing = std::collections::VecDeque<(u64, Record)>;

struct Tail {
    path: PathBuf,
    ring: std::sync::Mutex<TailRing>,
}

static TAIL: OnceLock<Option<std::sync::Arc<Tail>>> = OnceLock::new();

/// The flight recorder (`DC_PROF_TAIL`, profile mode only); its writer thread starts with the first use.
fn tail() -> Option<&'static std::sync::Arc<Tail>> {
    TAIL.get_or_init(|| {
        let path = PathBuf::from(std::env::var_os("DC_PROF_TAIL")?);
        if !on() {
            return None;
        }
        let t = std::sync::Arc::new(Tail {
            path,
            ring: std::sync::Mutex::new(TailRing::with_capacity(TAIL_FRAMES)),
        });
        let w = t.clone();
        let spawned = std::thread::Builder::new()
            .name("dc-prof-tail".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(TAIL_EVERY_MS));
                w.write();
            });
        if let Err(e) = spawned {
            eprintln!("dc-prof: tail thread: {e}");
        }
        Some(t)
    })
    .as_ref()
}

impl Tail {
    /// Writes the ring (a copy taken under the lock) to a side file and moves it over the old one.
    fn write(&self) {
        let rows: Vec<(u64, Record)> = match self.ring.lock() {
            Ok(r) => r.iter().cloned().collect(),
            Err(p) => p.into_inner().iter().cloned().collect(),
        };
        let mut csv = String::from("unix_ms,");
        csv.push_str(&csv_head());
        for (ms, r) in &rows {
            csv.push_str(&ms.to_string());
            csv.push(',');
            csv.push_str(&csv_row(r));
        }
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, csv).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}

/// Writes the flight recorder's file now (the window's lost-device path: the frame thread does not come back to it).
pub fn tail_flush() {
    if let Some(t) = TAIL.get().and_then(|t| t.as_ref()) {
        t.write();
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The `DC_PROF_FRAMES` header line (with its newline).
fn csv_head() -> String {
    let mut head = vec!["frame".to_string(), "total".into()];
    head.extend(STAGE_NAMES.iter().map(|s| s.to_string()));
    head.extend(STAGE_NAMES.iter().map(|s| format!("a_{s}")));
    head.extend((0..COUNTERS).map(|i| counter_name(i).to_string()));
    head.extend(NOTE_NAMES.iter().map(|s| format!("n_{s}")));
    head.extend(GPU_PASSES.iter().map(|s| format!("gpu_{s}")));
    head.push("gpu_span".into());
    head.push("remotes".into());
    let mut csv = head.join(",");
    csv.push('\n');
    csv
}

/// One `DC_PROF_FRAMES` line (with its newline).
fn csv_row(r: &Record) -> String {
    let f = |v: f32| {
        if v.is_finite() {
            format!("{v:.4}")
        } else {
            String::new()
        }
    };
    let mut row = vec![r.frame.to_string(), f(r.total)];
    row.extend(r.ms.iter().map(|v| f(*v)));
    row.extend(r.allocs.iter().map(|v| v.to_string()));
    row.extend(r.counts.iter().map(|v| v.to_string()));
    row.extend(r.notes.iter().map(|v| v.to_string()));
    row.extend(r.gpu.iter().map(|v| f(*v)));
    row.push(f(r.gpu_span));
    row.push(r.remotes.to_string());
    let mut line = row.join(",");
    line.push('\n');
    line
}

#[derive(Clone)]
struct Record {
    frame: u64,
    ms: [f32; STAGES],
    allocs: [u32; STAGES],
    total: f32,
    counts: [u64; COUNTERS],
    notes: [u32; NOTES],
    /// GPU pass times (NaN: not timed).
    gpu: [f32; GPU_PASSES.len()],
    gpu_span: f32,
    remotes: u16,
}

/// Frames above this many ms are hitches (the async model loads' target).
pub const HITCH_MS: f64 = 5.0;
/// Frames above this many ms miss 60 fps.
pub const SLOW_MS: f64 = 1000.0 / 60.0;
/// Hitch frames kept with their whole record: the worst of the run (the others are counted in `not_kept`).
pub const HITCHES_KEPT: usize = 128;
/// Process memory is read every this many frames, the GPU allocator's report every [`GPU_MEM_EVERY`] frames
/// (outside the frame's own time).
pub const MEM_EVERY: u64 = 30;
pub const GPU_MEM_EVERY: u64 = 300;

#[derive(Default, Clone)]
struct LoadPhase {
    started: Option<Instant>,
    base: Option<[u64; COUNTERS]>,
    ms: f64,
    counts: Option<[u64; COUNTERS]>,
    gpu_level: Option<(u64, u64)>,
    rss: Option<(u64, u64, u64)>,
}

/// Collects one record per frame (bounded: see `DC_PROF_MAX_FRAMES`).
pub struct Recorder {
    records: Vec<Record>,
    max_records: usize,
    dropped_records: u64,
    hitches: Vec<Record>,
    hitches_dropped: u64,
    times: FrameTimes,
    start: Instant,
    allocs_at_start: u64,
    base: [u64; COUNTERS],
    run_start: Instant,
    cpu_start: Option<(u64, u64, u64)>,
    next_frame: u64,
    first_frame_ms: Option<(f64, f64)>,
    load: LoadPhase,
    mem_peak: (u64, u64),
    gpu_peak: (u64, u64),
    gpu_end: Option<(u64, u64)>,
    stream: Vec<perf::StreamLatency>,
    stream_open: usize,
    stream_lost: u64,
    pub note: String,
    /// Adapter description and target size, for the report.
    pub adapter: String,
    pub size: (u32, u32),
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

/// Nearest-rank statistics of a list: (mean, p50, p95, p99, max, min).
fn stat(mut v: Vec<f64>) -> (f64, f64, f64, f64, f64, f64) {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    let q = |p: f64| v[((p * n as f64).ceil() as usize).clamp(1, n) - 1];
    (mean, q(0.5), q(0.95), q(0.99), v[n - 1], v[0])
}

fn stat_json(v: Vec<f64>, digits: usize) -> Value {
    let (mean, p50, p95, p99, max, min) = stat(v);
    let r = |x: f64| {
        let f = 10f64.powi(digits as i32);
        (x * f).round() / f
    };
    json!({"mean": r(mean), "median": r(p50), "p50": r(p50), "p95": r(p95), "p99": r(p99), "max": r(max), "min": r(min)})
}

impl Recorder {
    pub fn new() -> Recorder {
        let max_records = config().max_frames;
        Recorder {
            records: if on() {
                Vec::with_capacity(max_records)
            } else {
                Vec::new()
            },
            max_records,
            dropped_records: 0,
            hitches: Vec::new(),
            hitches_dropped: 0,
            times: FrameTimes::default(),
            start: Instant::now(),
            allocs_at_start: thread_allocs(),
            base: counters(),
            run_start: Instant::now(),
            cpu_start: perf::cpu_times(),
            next_frame: 0,
            first_frame_ms: None,
            load: LoadPhase::default(),
            mem_peak: (0, 0),
            gpu_peak: (0, 0),
            gpu_end: None,
            stream: Vec::new(),
            stream_open: 0,
            stream_lost: 0,
            note: String::new(),
            adapter: String::new(),
            size: (0, 0),
        }
    }

    /// Room for `n` frame records up front (client-fps: a growing list would copy itself inside a frame and show
    /// up as a hitch of its own), never beyond the `DC_PROF_MAX_FRAMES` bound.
    pub fn reserve(&mut self, n: usize) {
        let room = self.max_records.saturating_sub(self.records.len());
        self.records.reserve(n.min(room));
    }

    /// Frames not kept because the record list was full (they still count in the histogram and the hitch list).
    pub fn dropped(&self) -> u64 {
        self.dropped_records
    }

    /// The level load starts: counters from here to [`Recorder::load_end`] are the load phase.
    pub fn load_begin(&mut self) {
        perf::set_level_ready(false);
        self.load.started = Some(Instant::now());
        self.load.base = Some(counters());
    }

    /// The level is loaded: its time, counters, GPU memory (`device`'s allocator) and working set are kept, and
    /// from now on a pipeline or sampler create is a runtime create.
    pub fn load_end(&mut self, device: Option<&wgpu::Device>) {
        if let (Some(t), Some(b)) = (self.load.started, self.load.base) {
            self.load.ms = t.elapsed().as_secs_f64() * 1000.0;
            let now = counters();
            self.load.counts = Some(std::array::from_fn(|i| now[i].wrapping_sub(b[i])));
        }
        self.load.gpu_level = device.and_then(perf::gpu_memory);
        if let Some(g) = self.load.gpu_level {
            self.gpu_peak = (self.gpu_peak.0.max(g.0), self.gpu_peak.1.max(g.1));
        }
        self.load.rss = perf::process_memory();
        perf::set_level_ready(true);
    }

    /// [`Recorder::begin`] with the frame's time counted from `since` (the window: the end of the previous redraw).
    pub fn begin_at(&mut self, device: Option<&wgpu::Device>, since: Instant) -> u64 {
        let n = self.begin(device);
        self.start = since;
        n
    }

    /// Starts frame `n` (its number is returned): collects finished GPU timestamps of earlier frames (`device`), and
    /// starts this frame's GPU timer.
    pub fn begin(&mut self, device: Option<&wgpu::Device>) -> u64 {
        if let Some(d) = device {
            perf::gpu_collect(d);
        }
        self.take_gpu();
        let n = self.next_frame;
        self.next_frame += 1;
        perf::set_frame(n);
        perf::gpu_frame_begin(n);
        CURRENT.with(|c| *c.borrow_mut() = Current::default());
        self.base = counters();
        self.allocs_at_start = thread_allocs();
        self.start = Instant::now();
        n
    }

    fn take_gpu(&mut self) {
        let first = self.records.first().map_or(0, |r| r.frame);
        for g in perf::gpu_take() {
            let mut gpu = [f32::NAN; GPU_PASSES.len()];
            for (name, ms) in &g.passes {
                let k = GPU_PASSES
                    .iter()
                    .position(|p| p == name)
                    .unwrap_or(GPU_PASSES.len() - 1);
                gpu[k] = if gpu[k].is_nan() {
                    *ms as f32
                } else {
                    gpu[k] + *ms as f32
                };
            }
            let i = g.frame.wrapping_sub(first) as usize;
            if let Some(r) = self.records.get_mut(i).filter(|r| r.frame == g.frame) {
                r.gpu = gpu;
                r.gpu_span = g.span_ms as f32;
            }
            if let Some(h) = self.hitches.iter_mut().find(|h| h.frame == g.frame) {
                h.gpu = gpu;
                h.gpu_span = g.span_ms as f32;
            }
            if let Some(t) = tail() {
                if let Ok(mut ring) = t.ring.lock() {
                    if let Some((_, r)) = ring.iter_mut().rev().find(|(_, r)| r.frame == g.frame) {
                        r.gpu = gpu;
                        r.gpu_span = g.span_ms as f32;
                    }
                }
            }
        }
        let (done, open, lost) = perf::stream_take();
        let room = 65_536usize.saturating_sub(self.stream.len());
        self.stream_lost += lost + done.len().saturating_sub(room) as u64;
        self.stream.extend(done.into_iter().take(room));
        self.stream_open = open;
    }

    /// Ends the current frame (`device`: for the periodic GPU memory reading, made after the frame's time is taken).
    pub fn end_with(&mut self, remotes: usize, device: Option<&wgpu::Device>) {
        let total = self.start.elapsed().as_secs_f64() * 1000.0;
        let now = counters();
        let counts: [u64; COUNTERS] = std::array::from_fn(|i| now[i].wrapping_sub(self.base[i]));
        let c = CURRENT.with(|c| c.borrow().clone());
        let rec = Record {
            frame: self.next_frame.saturating_sub(1),
            ms: c.ms.map(|v| v as f32),
            allocs: c.allocs.map(|v| v.min(u32::MAX as u64) as u32),
            total: total as f32,
            counts,
            notes: c.notes.map(|v| v.min(u32::MAX as u64) as u32),
            gpu: [f32::NAN; GPU_PASSES.len()],
            gpu_span: f32::NAN,
            remotes: remotes.min(u16::MAX as usize) as u16,
        };
        self.times.push_ms(total);
        if self.first_frame_ms.is_none() {
            let from_load = self
                .load
                .started
                .map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
            let from_start = ENABLED_AT
                .get()
                .map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
            self.first_frame_ms = Some((from_load, from_start));
        }
        // the worst HITCHES_KEPT hitches of the run are kept whole (a later, longer hitch replaces the shortest)
        if total > HITCH_MS {
            if self.hitches.len() < HITCHES_KEPT {
                self.hitches.push(rec.clone());
            } else {
                self.hitches_dropped += 1;
                if let Some((i, h)) = self
                    .hitches
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.total.total_cmp(&b.1.total))
                {
                    if h.total < rec.total {
                        self.hitches[i] = rec.clone();
                    }
                }
            }
        }
        if let Some(t) = tail() {
            if let Ok(mut ring) = t.ring.lock() {
                if ring.len() >= TAIL_FRAMES {
                    ring.pop_front();
                }
                ring.push_back((unix_ms(), rec.clone()));
            }
        }
        if self.records.len() < self.max_records {
            self.records.push(rec);
        } else {
            self.dropped_records += 1;
        }
        let n = self.next_frame;
        if n % MEM_EVERY == 1 {
            if let Some((ws, peak, private)) = perf::process_memory() {
                self.mem_peak = (
                    self.mem_peak.0.max(ws.max(peak)),
                    self.mem_peak.1.max(private),
                );
            }
        }
        let every = if perf::GPU_MEMORY_CHEAP {
            MEM_EVERY
        } else {
            GPU_MEM_EVERY
        };
        if n % every == 1 {
            if let Some(g) = device.and_then(perf::gpu_memory) {
                self.gpu_peak = (self.gpu_peak.0.max(g.0), self.gpu_peak.1.max(g.1));
            }
        }
    }

    /// [`Recorder::end_with`] without a device.
    pub fn end(&mut self, remotes: usize) {
        self.end_with(remotes, None);
    }

    /// Prints the summary and writes the JSON report (and the CSV); returns the JSON text. `device`: one last GPU
    /// collection and memory reading.
    pub fn finish_with(&mut self, device: Option<&wgpu::Device>) -> String {
        if let Some(d) = device {
            let _ = d.poll(wgpu::PollType::Wait);
            perf::gpu_collect(d);
            self.gpu_end = perf::gpu_memory(d);
            if let Some(g) = self.gpu_end {
                self.gpu_peak = (self.gpu_peak.0.max(g.0), self.gpu_peak.1.max(g.1));
            }
        }
        self.take_gpu();
        let v = self.summary();
        let text = serde_json::to_string_pretty(&v).unwrap_or_default();
        self.write_frames();
        tail_flush();
        if let Some(p) = &config().out {
            if let Err(e) = std::fs::write(p, &text) {
                eprintln!("dc-prof: {}: {e}", p.display());
            }
        }
        text
    }

    /// [`Recorder::finish_with`] without a device.
    pub fn finish(&mut self) -> String {
        self.finish_with(None)
    }

    fn summary(&self) -> Value {
        let cfg = config();
        let wall = self.run_start.elapsed().as_secs_f64();
        let skip = cfg.warmup.min(self.records.len().saturating_sub(1));
        let r = &self.records[skip..];
        let col = |f: &dyn Fn(&Record) -> f64| r.iter().map(f).collect::<Vec<f64>>();
        let total = stat(col(&|x| x.total as f64));
        println!(
            "dc-prof: {} frames ({skip} warm-up left out): frame mean {:.4} ms, p50 {:.4}, p95 {:.4}, p99 {:.4}, max {:.4} = {:.0} fps mean",
            r.len(),
            total.0,
            total.1,
            total.2,
            total.3,
            total.4,
            if total.0 > 0.0 { 1000.0 / total.0 } else { 0.0 }
        );
        let mut stages = Map::new();
        for (i, name) in STAGE_NAMES.iter().enumerate() {
            let s = stat_json(col(&|x| x.ms[i] as f64), 4);
            let mut o = s.as_object().cloned().unwrap_or_default();
            let a = stat(col(&|x| x.allocs[i] as f64));
            o.insert("allocs_p99".into(), json!(a.3));
            o.insert("allocs_max".into(), json!(a.4));
            if s["max"].as_f64().unwrap_or(0.0) > 0.0 || a.4 > 0.0 {
                println!(
                    "  {name:14} p50 {:8.4} ms  p95 {:8.4}  p99 {:8.4}  max {:8.4}  allocs p99 {}",
                    s["p50"], s["p95"], s["p99"], s["max"], a.3
                );
            }
            stages.insert((*name).into(), Value::Object(o));
        }
        let mut counts = Map::new();
        for i in 0..COUNTERS {
            let mut o = stat_json(col(&|x| x.counts[i] as f64), 1)
                .as_object()
                .cloned()
                .unwrap_or_default();
            o.insert(
                "total".into(),
                json!(r.iter().map(|x| x.counts[i]).sum::<u64>()),
            );
            counts.insert(counter_name(i).into(), Value::Object(o));
        }
        let mut notes = Map::new();
        for (i, name) in NOTE_NAMES.iter().enumerate() {
            notes.insert((*name).into(), stat_json(col(&|x| x.notes[i] as f64), 1));
        }
        let hot = stat(col(&|x| x.allocs.iter().map(|&a| a as f64).sum()));
        let gpu_frames = r.iter().filter(|x| x.gpu_span.is_finite()).count();
        let mut gpu_passes = Map::new();
        for (k, name) in GPU_PASSES.iter().enumerate() {
            let v = col(&|x| x.gpu[k] as f64);
            if v.iter().any(|x| x.is_finite()) {
                gpu_passes.insert((*name).into(), stat_json(v, 4));
            }
        }
        let gpu_span = stat_json(col(&|x| x.gpu_span as f64), 4);
        let gs = perf::gpu_stats();
        if gpu_frames > 0 {
            println!(
                "  GPU ({gpu_frames} frames timed): span p50 {} ms p95 {} p99 {} max {}",
                gpu_span["p50"], gpu_span["p95"], gpu_span["p99"], gpu_span["max"]
            );
        }
        let (violations, violations_dropped) = perf::take_violations();
        let cpu = match (self.cpu_start, perf::cpu_times()) {
            (Some(a), Some(b)) => {
                let (load, own) = perf::cpu_load(a, b);
                json!({"machine_load_pct": (load * 1000.0).round() / 10.0, "process_pct_of_machine": (own * 1000.0).round() / 10.0,
                       "process_cpu_s": ((b.2 - a.2) as f64 / 1e7 * 1000.0).round() / 1000.0,
                       "cores": std::thread::available_parallelism().map_or(0, |n| n.get())})
            }
            _ => Value::Null,
        };
        println!("  cpu: {cpu}");
        let ft = &self.times;
        let ledger = self.ledger(r, cfg.inflight);
        let load = self.load_json();
        let streaming = {
            let ms = stat_json(self.stream.iter().map(|l| l.ms).collect(), 3);
            let fr = stat_json(self.stream.iter().map(|l| l.frames as f64).collect(), 1);
            json!({"completed": self.stream.len(), "open_at_end": self.stream_open, "lost": self.stream_lost,
                   "latency_ms": ms, "latency_frames": fr})
        };
        let mut kept: Vec<&Record> = self.hitches.iter().collect();
        kept.sort_by_key(|r| r.frame);
        let hitches: Vec<Value> = kept.into_iter().map(record_json).collect();
        let gauges: Map<String, Value> = perf::GAUGE_NAMES
            .iter()
            .zip(perf::gauges())
            .map(|(n, (now, high))| ((*n).to_string(), json!({"now": now, "high": high})))
            .collect();
        let mem_end = perf::process_memory();
        json!({
            "schema": "dc-prof/2",
            "note": self.note,
            "scenario": SCENARIO.get().cloned().unwrap_or_else(|| cfg.scenario.clone()),
            "adapter": self.adapter,
            "size": [self.size.0, self.size.1],
            "target": std::env::consts::ARCH,
            "debug_build": cfg!(debug_assertions),
            "prof_alloc": allocs_counted(),
            "build_commit": option_env!("DC_BUILD_COMMIT").unwrap_or(""),
            "frames": r.len(),
            "frames_run": self.next_frame,
            "records_dropped": self.dropped_records,
            "warmup": skip,
            "inflight": cfg.inflight,
            "readback": cfg.readback,
            "wall_s": (wall * 1000.0).round() / 1000.0,
            "frame_ms": stat_json(col(&|x| x.total as f64), 4),
            "fps_mean": if total.0 > 0.0 { (10000.0 / total.0).round() / 10.0 } else { 0.0 },
            "fps_median": if total.1 > 0.0 { (10000.0 / total.1).round() / 10.0 } else { 0.0 },
            "all_frames": {
                "frames": ft.frames(), "p50": ft.quantile_ms(0.5), "p95": ft.quantile_ms(0.95),
                "p99": ft.quantile_ms(0.99), "max_ms": ft.worst_ms(),
                "over_hitch": ft.over_hitch(), "hitch_ms": HITCH_MS,
                "over_slow": ft.over_slow(), "slow_ms": SLOW_MS,
            },
            "first_frame_ms": self.first_frame_ms.map(|(l, s)| json!({"from_load_begin": l, "from_profiler_start": s})),
            "stages": stages,
            "counters": counts,
            "events": notes,
            "hot_loop_allocs_per_frame": {"p50": hot.1, "p95": hot.2, "p99": hot.3, "max": hot.4,
                                          "counted": allocs_counted()},
            "gpu": {"available": gs.available, "frames_timed": gpu_frames, "skipped": gs.skipped,
                    "pass_overflow": gs.overflow, "dropped": gs.dropped, "failed": gs.failed,
                    "span_ms": gpu_span, "passes": gpu_passes},
            "gauges": gauges,
            "memory": {
                "rss_end": mem_end.map(|m| m.0), "rss_peak": mem_end.map(|m| m.1.max(self.mem_peak.0)),
                "private_end": mem_end.map(|m| m.2), "private_peak_sampled": self.mem_peak.1,
                "gpu_allocated_level": self.load.gpu_level.map(|g| g.0),
                "gpu_reserved_level": self.load.gpu_level.map(|g| g.1),
                "gpu_allocated_end": self.gpu_end.map(|g| g.0),
                "gpu_allocated_peak_sampled": self.gpu_peak.0, "gpu_reserved_peak_sampled": self.gpu_peak.1,
            },
            "load": load,
            "streaming": streaming,
            "violations": {"runtime_creates": violations, "not_kept": violations_dropped},
            "hitches": {"kept": hitches, "not_kept": self.hitches_dropped, "hitch_ms": HITCH_MS},
            "ledger": ledger,
            "cpu": cpu,
        })
    }

    fn load_json(&self) -> Value {
        let Some(c) = self.load.counts else {
            return Value::Null;
        };
        let g = |n: &str| c[counter_index(n)];
        json!({
            "ms": (self.load.ms * 1000.0).round() / 1000.0,
            "upload_bytes": g("upload_bytes"), "upload_ops": g("upload_ops"),
            "buffer_bytes": g("buffer_bytes"), "texture_bytes": g("texture_bytes"),
            "pipeline_creates": g("pipeline_creates"), "sampler_creates": g("sampler_creates"),
            "io_ms": g("io_ns") as f64 / 1e6, "io_bytes": g("io_bytes"),
            "parse_ms": g("parse_ns") as f64 / 1e6, "decode_ms": g("decode_ns") as f64 / 1e6,
            "decode_bytes": g("decode_bytes"), "pack_ms": g("pack_ns") as f64 / 1e6,
            "rss": self.load.rss.map(|m| m.0),
            "gpu_allocated": self.load.gpu_level.map(|g| g.0),
            "gpu_reserved": self.load.gpu_level.map(|g| g.1),
        })
    }

    /// The high-water marks per ledger row of RENDER_PERF_GATES.md (see tools/perf_report.py for the table).
    fn ledger(&self, r: &[Record], inflight: usize) -> Value {
        let c = |n: &str| {
            let i = counter_index(n);
            r.iter().map(|x| x.counts[i] as f64).collect::<Vec<f64>>()
        };
        let q = |v: Vec<f64>| stat(v);
        let gauges = perf::gauges();
        let window = inflight + 1;
        let roll = |v: &[f64]| -> Vec<f64> {
            v.windows(window.min(v.len()).max(1))
                .map(|w| w.iter().sum())
                .collect()
        };
        let up = c("upload_bytes");
        let ops = c("upload_ops");
        let staging = q(roll(&up));
        let staging_ops = q(roll(&ops));
        let up_s = q(up);
        let ops_s = q(ops);
        let dyn_s = q(c("buffer_write_bytes"));
        let bones = q(c("bones_uploaded"));
        let quads = q(c("overlay_quads"));
        let fxq = q(c("fx_quads"));
        let draws = q(r
            .iter()
            .map(|x| {
                (x.counts[counter_index("draw_calls")] + x.counts[counter_index("client_draws")])
                    as f64
            })
            .collect());
        let binds = q(c("bind_group_sets"));
        let pipes = q(c("pipeline_sets"));
        let frame = q(r.iter().map(|x| x.total as f64).collect());
        let encode = q(r
            .iter()
            .map(|x| (x.ms[Stage::Encode as usize] + x.ms[Stage::Submit as usize]) as f64)
            .collect());
        let hot = q(r
            .iter()
            .map(|x| x.allocs.iter().map(|&a| a as f64).sum())
            .collect());
        let gpu = q(r.iter().map(|x| x.gpu_span as f64).collect());
        let load_c = self.load.counts.unwrap_or([0; COUNTERS]);
        let lc = |n: &str| load_c[counter_index(n)];
        // every runtime create of the run, warm-up frames included (the first frames are where they happen)
        let runtime_pipes = perf::counters()[perf::Counter::RuntimePipelineCreates as usize];
        let runtime_samplers = perf::counters()[perf::Counter::RuntimeSamplerCreates as usize];
        let stream_ms = q(self.stream.iter().map(|l| l.ms).collect());
        let stream_fr = q(self.stream.iter().map(|l| l.frames as f64).collect());
        let load_frames: Vec<f64> = r
            .iter()
            .filter(|x| {
                x.counts[counter_index("model_loads")] + x.counts[counter_index("texture_loads")]
                    > 0
            })
            .map(|x| x.total as f64)
            .collect();
        let worst_upload_frame = load_frames.iter().copied().fold(0.0, f64::max);
        let mem_end = perf::process_memory();
        json!({
            "system_ram": {
                "rss_peak_bytes": mem_end.map(|m| m.1.max(self.mem_peak.0)),
                "decoded_image_pool_high_bytes": gauges[perf::Gauge::DecodedImageBytes as usize].1,
                "loader_jobs_high": gauges[perf::Gauge::LoaderJobs as usize].1,
            },
            "gpu_static": {
                "gpu_allocated_at_level_bytes": self.load.gpu_level.map(|g| g.0),
                "created_during_load_bytes": lc("buffer_bytes") + lc("texture_bytes"),
                "gpu_allocated_peak_bytes": self.gpu_peak.0,
            },
            "gpu_dynamic_frame": {
                "buffer_write_bytes_p95": dyn_s.2, "buffer_write_bytes_max": dyn_s.4,
                "bones_per_frame_max": bones.4, "hud_quads_max": quads.4, "fx_quads_max": fxq.4,
            },
            "staging_in_flight": {
                "window_frames": window,
                "bytes_p95": staging.2, "bytes_max": staging.4, "ops_max": staging_ops.4,
            },
            "upload_traffic": {
                "bytes_p95": up_s.2, "bytes_p99": up_s.3, "bytes_max": up_s.4,
                "ops_p95": ops_s.2, "ops_max": ops_s.4,
            },
            "cpu_frame": {
                "frame_ms_p95": frame.2, "frame_ms_p99": frame.3, "frame_ms_max": frame.4,
                "encode_submit_ms_p95": encode.2, "encode_submit_ms_p99": encode.3,
                "hot_loop_allocs_max": hot.4, "allocs_counted": allocs_counted(),
                "loader_jobs_high": gauges[perf::Gauge::LoaderJobs as usize].1,
            },
            "gpu_frame": {
                "span_ms_p95": gpu.2, "span_ms_p99": gpu.3, "span_ms_max": gpu.4,
                "timed_frames": r.iter().filter(|x| x.gpu_span.is_finite()).count(),
            },
            "pipelines": {
                "created_at_load": lc("pipeline_creates"), "load_ms": self.load.ms,
                "runtime_creates": runtime_pipes, "runtime_sampler_creates": runtime_samplers,
            },
            "draw_bind": {
                "draws_p95": draws.2, "draws_max": draws.4,
                "bind_sets_p95": binds.2, "bind_sets_max": binds.4,
                "pipeline_sets_max": pipes.4,
                "dynamic_offsets_max": 0,
            },
            "streaming": {
                "time_to_resident_frames_p95": stream_fr.2, "time_to_resident_frames_max": stream_fr.4,
                "time_to_resident_ms_p95": stream_ms.2,
                "worst_frame_with_load_ms": worst_upload_frame,
            },
        })
    }

    /// `DC_PROF_FRAMES=<csv>`: one line per kept frame.
    fn write_frames(&self) {
        let Some(path) = std::env::var_os("DC_PROF_FRAMES") else {
            return;
        };
        let mut csv = csv_head();
        for r in &self.records {
            csv.push_str(&csv_row(r));
        }
        if let Err(e) = std::fs::write(&path, csv) {
            eprintln!("dc-prof: {}: {e}", std::path::Path::new(&path).display());
        }
    }
}

/// A frame's record as the hitch list shows it: every stage over 0.05 ms, every non-zero counter and event, the GPU
/// passes.
fn record_json(r: &Record) -> Value {
    let stages: Map<String, Value> = STAGE_NAMES
        .iter()
        .zip(r.ms)
        .filter(|(_, v)| *v > 0.05)
        .map(|(n, v)| {
            (
                (*n).to_string(),
                json!((v as f64 * 1000.0).round() / 1000.0),
            )
        })
        .collect();
    let counts: Map<String, Value> = (0..COUNTERS)
        .filter(|&i| r.counts[i] > 0)
        .map(|i| (counter_name(i).to_string(), json!(r.counts[i])))
        .collect();
    let events: Map<String, Value> = NOTE_NAMES
        .iter()
        .zip(r.notes)
        .filter(|(_, v)| *v > 0)
        .map(|(n, v)| ((*n).to_string(), json!(v)))
        .collect();
    let gpu: Map<String, Value> = GPU_PASSES
        .iter()
        .zip(r.gpu)
        .filter(|(_, v)| v.is_finite())
        .map(|(n, v)| {
            (
                (*n).to_string(),
                json!((v as f64 * 10000.0).round() / 10000.0),
            )
        })
        .collect();
    // the likely cause: the largest stage, and the load events of the frame
    let top = STAGE_NAMES
        .iter()
        .zip(r.ms)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(n, _)| *n)
        .unwrap_or("");
    let causes: Vec<&str> = [
        "model_loads",
        "texture_loads",
        "sound_loads",
        "runtime_pipeline_creates",
        "runtime_sampler_creates",
        "buffer_regrows",
    ]
    .into_iter()
    .filter(|n| r.counts[counter_index(n)] > 0)
    .collect();
    json!({"frame": r.frame, "ms": (r.total as f64 * 1000.0).round() / 1000.0, "top_stage": top,
           "load_events": causes, "stages": stages, "counters": counts, "events": events, "gpu": gpu})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn laps_without_profile_read_no_clock() {
        // tests never enable the profiler: a lap clock is inert
        if on() {
            return;
        }
        let mut l = Laps::start();
        assert!(l.0.is_none());
        l.lap(Stage::Encode);
        assert!(l.0.is_none());
    }

    #[test]
    fn stats_are_nearest_rank() {
        let (mean, p50, p95, p99, max, min) = stat(vec![4.0, 1.0, 3.0, 2.0]);
        assert_eq!(
            (mean, p50, p95, p99, max, min),
            (2.5, 2.0, 4.0, 4.0, 4.0, 1.0)
        );
        assert_eq!(stat(Vec::new()).0, 0.0);
        // NaN (an untimed GPU frame) is left out
        assert_eq!(stat(vec![f64::NAN, 1.0]).4, 1.0);
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        let s = stat(v);
        assert_eq!((s.1, s.2, s.3, s.4), (50.0, 95.0, 99.0, 100.0));
    }

    #[test]
    fn counter_names_are_unique_and_complete() {
        let names: std::collections::BTreeSet<&str> = (0..COUNTERS).map(counter_name).collect();
        assert_eq!(names.len(), COUNTERS);
        assert_eq!(counter_index("upload_bytes"), OWN_COUNTERS);
        assert_eq!(STAGE_NAMES.len(), STAGES);
        assert_eq!(NOTE_NAMES.len(), NOTES);
    }

    #[test]
    fn a_frames_row_has_one_value_per_header_column() {
        let r = Record {
            frame: 3,
            ms: [0.5; STAGES],
            allocs: [1; STAGES],
            total: 2.0,
            counts: [4; COUNTERS],
            notes: [0; NOTES],
            gpu: [f32::NAN; GPU_PASSES.len()],
            gpu_span: 1.25,
            remotes: 2,
        };
        let head = csv_head();
        let row = csv_row(&r);
        assert!(head.ends_with('\n') && row.ends_with('\n'));
        assert_eq!(head.split(',').count(), row.split(',').count());
        assert!(row.starts_with("3,2.0000,"));
    }

    #[test]
    fn a_hitch_record_names_its_cause() {
        let mut r = Record {
            frame: 7,
            ms: [0.0; STAGES],
            allocs: [0; STAGES],
            total: 12.0,
            counts: [0; COUNTERS],
            notes: [0; NOTES],
            gpu: [f32::NAN; GPU_PASSES.len()],
            gpu_span: f32::NAN,
            remotes: 0,
        };
        r.ms[Stage::Models as usize] = 11.0;
        r.counts[counter_index("model_loads")] = 2;
        r.notes[Note::FxEvents as usize] = 3;
        let j = record_json(&r);
        assert_eq!(j["top_stage"], "models_remotes");
        assert_eq!(j["load_events"][0], "model_loads");
        assert_eq!(j["events"]["fx_events"], 3);
        assert!(j["gpu"].as_object().is_some_and(|o| o.is_empty()));
    }
}
