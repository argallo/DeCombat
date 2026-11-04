//! Render performance telemetry: the G1 counters of `docs/RENDER_PERF_GATES.md` (perf-g1, our own tooling).
//!
//! Off by default. [`enable`] (called by dc-client's profiler) switches it on for the whole process; while it is off
//! every function here costs one relaxed atomic load and calls straight through to wgpu.
//!
//! What it keeps:
//! - **Counters** ([`Counter`]): process-wide monotonic sums (relaxed atomics, every thread). The profiler books the
//!   difference over each frame, so a loader thread's work lands in the frame during which it ran. Upload bytes and
//!   copy operations are counted at ONE accounting point: every `queue.write_buffer` / `write_texture` and every
//!   buffer or texture created with initial contents goes through [`write_buffer`], [`write_texture`],
//!   [`buffer_init`] or [`texture_with_data`]. Resource creation goes through [`buffer`], [`texture`],
//!   [`render_pipeline`] and [`sampler`]. CPU time of the load work (I/O, parse, decode, pack) is summed with
//!   [`span`].
//! - **Gauges** ([`Gauge`]): current values with a high-water mark (the decoded-texture pools, the loader queue).
//! - **Level phase** ([`set_level_ready`]): a pipeline or sampler created after the level was declared ready is a
//!   RUNTIME create (the ledger's "0 runtime creates"); it is counted and its label kept in a bounded list
//!   ([`take_violations`]).
//! - **GPU timestamps** ([`GpuTimer`]): per-pass begin / end timestamps on the normal multi-frame-in-flight path.
//!   Each frame owns one slot of a ring of [`TIMER_RING`]; the slot's query results are resolved and copied in the
//!   frame's own encoder, mapped after its submit and read [`gpu_collect`] calls later (a frame or two). A frame
//!   whose slot is still waiting is not timed (counted in [`GpuTimerStats::skipped`]); passes beyond
//!   [`MAX_TIMED_PASSES`] in one frame are not timed (counted in [`GpuTimerStats::overflow`]). Needs the adapter's
//!   `TIMESTAMP_QUERY`; [`GpuTimerStats::available`] says whether the device has it.
//!
//! Bounds (every container here is fixed-size or capped): counters and gauges are fixed arrays; the violation list
//! keeps [`VIOLATIONS_KEPT`] labels and counts the rest; the timer keeps [`TIMER_RESULTS_KEPT`] uncollected results
//! and drops the oldest beyond that (counted).
// ORIGINAL: perf-g1 measurement tooling (docs/RENDER_PERF_GATES.md G1); no 2009 behaviour

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use wgpu::util::DeviceExt;

static ENABLED: AtomicBool = AtomicBool::new(false);
static WANT_TIMESTAMPS: AtomicBool = AtomicBool::new(false);
static LEVEL_READY: AtomicBool = AtomicBool::new(false);

/// Switches the telemetry on for the process (`timestamps`: request `TIMESTAMP_QUERY` on devices created from now on
/// and time the passes). It is never switched off again.
pub fn enable(timestamps: bool) {
    WANT_TIMESTAMPS.store(timestamps, Relaxed);
    ENABLED.store(true, Relaxed);
}

/// The telemetry is on.
#[inline]
pub fn on() -> bool {
    ENABLED.load(Relaxed)
}

/// Devices created now should ask for GPU timestamps.
pub fn timestamps_wanted() -> bool {
    on() && WANT_TIMESTAMPS.load(Relaxed) && !thread_untimed()
}

thread_local! {
    static UNTIMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// CLI-1095: marks the calling thread as one that renders on a device of its own (the menu's character view): its
/// devices install no timer and its passes take no timestamp writes. The timer and its query set belong to the
/// window's device; a second device installing its timer replaced the window's, and a pass on one device writing
/// into the other device's query set is invalid, so the shop figure's render failed whenever the profiler ran.
pub fn untimed_thread() {
    let _ = UNTIMED.try_with(|u| u.set(true));
}

/// The calling thread renders without timestamps ([`untimed_thread`]).
pub fn thread_untimed() -> bool {
    UNTIMED.try_with(std::cell::Cell::get).unwrap_or(false)
}

/// The monotonic counters (sums since process start).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Counter {
    /// Bytes handed to the queue: buffer and texture writes and the initial contents of created resources.
    UploadBytes = 0,
    /// Copy operations: one per write and per created-with-contents resource.
    UploadOps,
    /// The texture part of [`Counter::UploadBytes`].
    UploadTextureBytes,
    BufferCreates,
    /// Bytes of the buffers created (their descriptor size).
    BufferBytes,
    TextureCreates,
    /// Bytes of the textures created (format block size x texels over every mip and layer; an estimate of the
    /// driver's allocation).
    TextureBytes,
    PipelineCreates,
    /// Pipelines created after [`set_level_ready`]: the ledger's runtime creates.
    RuntimePipelineCreates,
    SamplerCreates,
    RuntimeSamplerCreates,
    /// A growable buffer replaced by a larger one inside a frame (effect vertices, overlay instances, debug boxes).
    BufferRegrows,
    /// Bone matrices written to bone palettes.
    BonesUploaded,
    /// CPU time of file reads of the load work, ns.
    IoNs,
    IoBytes,
    /// CPU time of parsing model / effect / world files, ns.
    ParseNs,
    /// CPU time of texture decoding (DTX to RGBA8), ns.
    DecodeNs,
    /// RGBA8 bytes decoded.
    DecodeBytes,
    /// CPU time of packing GPU-format data (vertex conversion, upload preparation), ns.
    PackNs,
    /// Model uploads finished (characters, drops, effect models, view weapons).
    ModelLoads,
    /// Textures decoded and created after the level load (effect sprites, HUD atlas pages).
    TextureLoads,
    /// Sound files read and decoded.
    SoundLoads,
    /// Effect groups started.
    FxSpawns,
    /// Draws recorded outside dc-render's world pass (effects, HUD, debug panel, boxes, gamma).
    ClientDraws,
    /// The `write_buffer` part of [`Counter::UploadBytes`]: per-frame dynamic data (uniforms, bone palettes, effect
    /// vertices, HUD quads).
    BufferWriteBytes,
}

/// Number of [`Counter`]s.
pub const COUNTERS: usize = 25;
/// Report names of the counters, in [`Counter`] order.
pub const COUNTER_NAMES: [&str; COUNTERS] = [
    "upload_bytes",
    "upload_ops",
    "upload_texture_bytes",
    "buffer_creates",
    "buffer_bytes",
    "texture_creates",
    "texture_bytes",
    "pipeline_creates",
    "runtime_pipeline_creates",
    "sampler_creates",
    "runtime_sampler_creates",
    "buffer_regrows",
    "bones_uploaded",
    "io_ns",
    "io_bytes",
    "parse_ns",
    "decode_ns",
    "decode_bytes",
    "pack_ns",
    "model_loads",
    "texture_loads",
    "sound_loads",
    "fx_spawns",
    "client_draws",
    "buffer_write_bytes",
];

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU64 = AtomicU64::new(0);
static COUNTS: [AtomicU64; COUNTERS] = [ZERO; COUNTERS];

/// Adds `v` to counter `c` (nothing while the telemetry is off).
#[inline]
pub fn add(c: Counter, v: u64) {
    if on() {
        COUNTS[c as usize].fetch_add(v, Relaxed);
    }
}

/// The counters now.
pub fn counters() -> [u64; COUNTERS] {
    std::array::from_fn(|i| COUNTS[i].load(Relaxed))
}

/// Gauges: a current value and its high-water mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Gauge {
    /// RGBA8 bytes held by the decoded-image caches (model loader, effect sprites); nothing is evicted today.
    DecodedImageBytes = 0,
    /// Model and texture jobs queued or running on the loader threads.
    LoaderJobs,
    /// CPU bytes of model data kept for posed bounds (the LOD0 vertex copies of the uploaded models; templates are
    /// never evicted, so this only grows).
    ModelCpuBytes,
}

/// Number of [`Gauge`]s.
pub const GAUGES: usize = 3;
/// Report names of the gauges.
pub const GAUGE_NAMES: [&str; GAUGES] = ["decoded_image_bytes", "loader_jobs", "model_cpu_bytes"];

#[allow(clippy::declare_interior_mutable_const)]
const IZERO: AtomicI64 = AtomicI64::new(0);
static GAUGE_NOW: [AtomicI64; GAUGES] = [IZERO; GAUGES];
static GAUGE_HIGH: [AtomicI64; GAUGES] = [IZERO; GAUGES];

/// Moves gauge `g` by `delta` and raises its high-water mark (nothing while off).
#[inline]
pub fn gauge(g: Gauge, delta: i64) {
    if on() {
        let now = GAUGE_NOW[g as usize].fetch_add(delta, Relaxed) + delta;
        GAUGE_HIGH[g as usize].fetch_max(now, Relaxed);
    }
}

/// The gauges now: (current, high-water) per gauge.
pub fn gauges() -> [(i64, i64); GAUGES] {
    std::array::from_fn(|i| (GAUGE_NOW[i].load(Relaxed), GAUGE_HIGH[i].load(Relaxed)))
}

/// A CPU-time span booked to a nanosecond counter when dropped (no clock is read while the telemetry is off).
#[must_use]
pub struct Span(Option<(Instant, Counter)>);

/// Starts a span of `c` (one of the `*_ns` counters).
#[inline]
pub fn span(c: Counter) -> Span {
    Span(on().then(|| (Instant::now(), c)))
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((t, c)) = self.0 {
            let ns = t.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            COUNTS[c as usize].fetch_add(ns, Relaxed);
        }
    }
}

/// Labels of runtime creates kept for the report.
pub const VIOLATIONS_KEPT: usize = 64;
static VIOLATIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static VIOLATIONS_DROPPED: AtomicU64 = AtomicU64::new(0);

/// Declares the level loaded (`true`): pipelines and samplers created from now on are runtime creates. `false`
/// starts a new load phase.
pub fn set_level_ready(ready: bool) {
    LEVEL_READY.store(ready, Relaxed);
}

/// The level was declared ready.
pub fn level_ready() -> bool {
    LEVEL_READY.load(Relaxed)
}

fn violation(kind: &str, label: Option<&str>) {
    let Ok(mut v) = VIOLATIONS.lock() else {
        return;
    };
    if v.len() < VIOLATIONS_KEPT {
        v.push(format!("{kind} {}", label.unwrap_or("(unlabelled)")));
    } else {
        VIOLATIONS_DROPPED.fetch_add(1, Relaxed);
    }
}

/// The runtime creates seen so far (kind and label) and how many were not kept; clears the list.
pub fn take_violations() -> (Vec<String>, u64) {
    let v = VIOLATIONS
        .lock()
        .map(|mut v| std::mem::take(&mut *v))
        .unwrap_or_default();
    (v, VIOLATIONS_DROPPED.swap(0, Relaxed))
}

// ---- heap allocations ----

/// Heap allocations and bytes of every thread (counted only while [`CountingAlloc`] is the global allocator).
pub static ALLOCS: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static THREAD_ALLOCS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Heap allocations made by the calling thread so far (counted only by [`CountingAlloc`]).
pub fn thread_allocs() -> u64 {
    THREAD_ALLOCS.try_with(std::cell::Cell::get).unwrap_or(0)
}

/// A counting global allocator over the system allocator: every allocation and reallocation adds to [`ALLOCS`],
/// [`ALLOC_BYTES`] and the calling thread's count ([`thread_allocs`]), so a profiler can book allocations per stage
/// of its own thread ("0 hot-loop allocations" as a number). dc-client's binary installs it with its cargo feature
/// `prof-alloc`; a test binary installs it with `#[global_allocator]`.
pub struct CountingAlloc;

impl CountingAlloc {
    #[inline]
    fn count(bytes: usize) {
        ALLOCS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(bytes as u64, Relaxed);
        // a thread being torn down has no counter left: its allocations count only globally
        let _ = THREAD_ALLOCS.try_with(|c| c.set(c.get() + 1));
    }
}

// SAFETY: every call goes to `System` with the caller's arguments unchanged; the counters are atomics and a
// const-initialised thread-local `Cell` (no allocation, no destructor), so counting never re-enters the allocator.
unsafe impl std::alloc::GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        Self::count(l.size());
        // SAFETY: the caller's layout, unchanged
        unsafe { std::alloc::System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: std::alloc::Layout) -> *mut u8 {
        Self::count(l.size());
        // SAFETY: the caller's layout, unchanged
        unsafe { std::alloc::System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        // SAFETY: `p` came from `System` through this allocator with this layout
        unsafe { std::alloc::System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, n: usize) -> *mut u8 {
        Self::count(n);
        // SAFETY: as `dealloc`; `n` is the caller's new size
        unsafe { std::alloc::System.realloc(p, l, n) }
    }
}

// ---- the accounting point of GPU uploads and creates ----

/// `queue.write_buffer`, counted.
#[inline]
pub fn write_buffer(queue: &wgpu::Queue, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
    if on() {
        COUNTS[Counter::UploadBytes as usize].fetch_add(data.len() as u64, Relaxed);
        COUNTS[Counter::BufferWriteBytes as usize].fetch_add(data.len() as u64, Relaxed);
        COUNTS[Counter::UploadOps as usize].fetch_add(1, Relaxed);
    }
    queue.write_buffer(buffer, offset, data);
}

/// `queue.write_texture`, counted.
#[inline]
pub fn write_texture(
    queue: &wgpu::Queue,
    texture: wgpu::TexelCopyTextureInfo<'_>,
    data: &[u8],
    layout: wgpu::TexelCopyBufferLayout,
    size: wgpu::Extent3d,
) {
    if on() {
        let n = data.len() as u64;
        COUNTS[Counter::UploadBytes as usize].fetch_add(n, Relaxed);
        COUNTS[Counter::UploadTextureBytes as usize].fetch_add(n, Relaxed);
        COUNTS[Counter::UploadOps as usize].fetch_add(1, Relaxed);
    }
    queue.write_texture(texture, data, layout, size);
}

/// `device.create_buffer`, counted.
pub fn buffer(device: &wgpu::Device, desc: &wgpu::BufferDescriptor<'_>) -> wgpu::Buffer {
    if on() {
        COUNTS[Counter::BufferCreates as usize].fetch_add(1, Relaxed);
        COUNTS[Counter::BufferBytes as usize].fetch_add(desc.size, Relaxed);
    }
    device.create_buffer(desc)
}

/// `device.create_buffer_init`, counted as a create and an upload of its contents.
pub fn buffer_init(
    device: &wgpu::Device,
    desc: &wgpu::util::BufferInitDescriptor<'_>,
) -> wgpu::Buffer {
    if on() {
        let n = desc.contents.len() as u64;
        COUNTS[Counter::BufferCreates as usize].fetch_add(1, Relaxed);
        COUNTS[Counter::BufferBytes as usize].fetch_add(n, Relaxed);
        COUNTS[Counter::UploadBytes as usize].fetch_add(n, Relaxed);
        COUNTS[Counter::UploadOps as usize].fetch_add(1, Relaxed);
    }
    device.create_buffer_init(desc)
}

/// A growable buffer was replaced by a larger one (counted; call next to the [`buffer`] create).
#[inline]
pub fn regrow() {
    add(Counter::BufferRegrows, 1);
}

/// Bytes a texture of `desc` occupies (block size times texels over every mip, layer and sample; 4 bytes per texel
/// for formats without a single block size).
pub fn texture_bytes(desc: &wgpu::TextureDescriptor<'_>) -> u64 {
    let f = desc.format;
    let (bw, bh) = f.block_dimensions();
    let block = f.block_copy_size(None).unwrap_or(4) as u64;
    let layers = desc.size.depth_or_array_layers.max(1) as u64;
    let mut total = 0u64;
    for mip in 0..desc.mip_level_count.max(1) {
        let w = (desc.size.width >> mip).max(1);
        let h = (desc.size.height >> mip).max(1);
        let bx = w.div_ceil(bw) as u64;
        let by = h.div_ceil(bh) as u64;
        total += bx * by * block * layers;
    }
    total * desc.sample_count.max(1) as u64
}

/// `device.create_texture`, counted.
pub fn texture(device: &wgpu::Device, desc: &wgpu::TextureDescriptor<'_>) -> wgpu::Texture {
    if on() {
        COUNTS[Counter::TextureCreates as usize].fetch_add(1, Relaxed);
        COUNTS[Counter::TextureBytes as usize].fetch_add(texture_bytes(desc), Relaxed);
    }
    device.create_texture(desc)
}

/// `device.create_texture_with_data`, counted as a create and an upload.
pub fn texture_with_data(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    desc: &wgpu::TextureDescriptor<'_>,
    order: wgpu::util::TextureDataOrder,
    data: &[u8],
) -> wgpu::Texture {
    if on() {
        let n = data.len() as u64;
        COUNTS[Counter::TextureCreates as usize].fetch_add(1, Relaxed);
        COUNTS[Counter::TextureBytes as usize].fetch_add(texture_bytes(desc), Relaxed);
        COUNTS[Counter::UploadBytes as usize].fetch_add(n, Relaxed);
        COUNTS[Counter::UploadTextureBytes as usize].fetch_add(n, Relaxed);
        COUNTS[Counter::UploadOps as usize].fetch_add(1, Relaxed);
    }
    device.create_texture_with_data(queue, desc, order, data)
}

/// `device.create_render_pipeline`, counted; after [`set_level_ready`] also a runtime create with its label kept.
pub fn render_pipeline(
    device: &wgpu::Device,
    desc: &wgpu::RenderPipelineDescriptor<'_>,
) -> wgpu::RenderPipeline {
    if on() {
        COUNTS[Counter::PipelineCreates as usize].fetch_add(1, Relaxed);
        if level_ready() {
            COUNTS[Counter::RuntimePipelineCreates as usize].fetch_add(1, Relaxed);
            violation("pipeline", desc.label);
        }
    }
    device.create_render_pipeline(desc)
}

/// `device.create_sampler`, counted like [`render_pipeline`].
pub fn sampler(device: &wgpu::Device, desc: &wgpu::SamplerDescriptor<'_>) -> wgpu::Sampler {
    if on() {
        COUNTS[Counter::SamplerCreates as usize].fetch_add(1, Relaxed);
        if level_ready() {
            COUNTS[Counter::RuntimeSamplerCreates as usize].fetch_add(1, Relaxed);
            violation("sampler", desc.label);
        }
    }
    device.create_sampler(desc)
}

/// GPU memory of a device: (allocated, reserved) bytes. With the cargo feature `gpu-counters` (wgpu's own memory
/// counters, every backend, cheap) the buffer + texture memory the device holds now; otherwise the allocator's report
/// where the backend keeps one (DX12; it walks every allocation: not for every frame). `None`: neither is available.
pub fn gpu_memory(device: &wgpu::Device) -> Option<(u64, u64)> {
    if cfg!(feature = "gpu-counters") {
        let c = device.get_internal_counters().hal;
        let m = (c.buffer_memory.read() + c.texture_memory.read()).max(0) as u64;
        if m > 0 {
            return Some((m, m));
        }
    }
    device
        .generate_allocator_report()
        .map(|r| (r.total_allocated_bytes, r.total_reserved_bytes))
}

/// [`gpu_memory`] is cheap enough to read often (the `gpu-counters` feature).
pub const GPU_MEMORY_CHEAP: bool = cfg!(feature = "gpu-counters");

/// The process's memory: (working set, peak working set, private commit) bytes; `None` off Windows or on failure.
pub fn process_memory() -> Option<(u64, u64, u64)> {
    os::process_memory()
}

/// CPU times now: (machine idle, machine busy, this process's user + kernel), all in 100 ns units; `None` off
/// Windows. Two readings give the machine's load over the interval ([`cpu_load`]).
pub fn cpu_times() -> Option<(u64, u64, u64)> {
    os::cpu_times()
}

/// The machine's CPU load between two [`cpu_times`] readings (0..1, all cores) and the process's share of the
/// machine (0..1).
pub fn cpu_load(a: (u64, u64, u64), b: (u64, u64, u64)) -> (f64, f64) {
    let idle = b.0.saturating_sub(a.0) as f64;
    let busy = b.1.saturating_sub(a.1) as f64;
    let own = b.2.saturating_sub(a.2) as f64;
    let total = idle + busy;
    if total <= 0.0 {
        return (0.0, 0.0);
    }
    (busy / total, (own / total).min(1.0))
}

// ---- streaming: request to completion ----

static FRAME: AtomicU64 = AtomicU64::new(0);

/// The profiler's current frame number (stamps streaming requests and completions).
pub fn set_frame(n: u64) {
    FRAME.store(n, Relaxed);
}

/// Streaming requests tracked at once; more are not tracked (counted).
pub const STREAM_TRACKED: usize = 4096;
/// Completed latencies kept until taken; more are dropped (counted).
pub const STREAM_KEPT: usize = 4096;

/// Request-to-completion latency of one streamed resource.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamLatency {
    pub ms: f64,
    pub frames: u64,
}

#[derive(Default)]
struct Streams {
    open: std::collections::HashMap<u64, (Instant, u64)>,
    done: Vec<StreamLatency>,
    untracked: u64,
    dropped: u64,
}

static STREAMS: Mutex<Option<Streams>> = Mutex::new(None);

/// A resource `id` was requested (a model upload key's hash); a second request of an open id keeps the first time.
pub fn stream_request(id: u64) {
    if !on() {
        return;
    }
    let Ok(mut g) = STREAMS.lock() else { return };
    let s = g.get_or_insert_with(Streams::default);
    if s.open.contains_key(&id) {
        return;
    }
    if s.open.len() >= STREAM_TRACKED {
        s.untracked += 1;
        return;
    }
    s.open.insert(id, (Instant::now(), FRAME.load(Relaxed)));
}

/// Resource `id` is resident (uploaded or failed).
pub fn stream_done(id: u64) {
    if !on() {
        return;
    }
    let Ok(mut g) = STREAMS.lock() else { return };
    let s = g.get_or_insert_with(Streams::default);
    let Some((t, f)) = s.open.remove(&id) else {
        return;
    };
    if s.done.len() >= STREAM_KEPT {
        s.dropped += 1;
        return;
    }
    s.done.push(StreamLatency {
        ms: t.elapsed().as_secs_f64() * 1000.0,
        frames: FRAME.load(Relaxed).saturating_sub(f),
    });
}

/// The latencies completed since the last call, the requests still open, and the requests not tracked or dropped.
pub fn stream_take() -> (Vec<StreamLatency>, usize, u64) {
    let Ok(mut g) = STREAMS.lock() else {
        return (Vec::new(), 0, 0);
    };
    let s = g.get_or_insert_with(Streams::default);
    let lost = s.untracked + s.dropped;
    (std::mem::take(&mut s.done), s.open.len(), lost)
}

#[cfg(windows)]
mod os {
    // Public Win32 API (psapi / kernel32 K32GetProcessMemoryInfo); layout of PROCESS_MEMORY_COUNTERS_EX from the
    // public documentation.
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_rss: usize,
        rss: usize,
        quota_peak_paged_pool: usize,
        quota_paged_pool: usize,
        quota_peak_non_paged_pool: usize,
        quota_non_paged_pool: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }

    pub fn process_memory() -> Option<(u64, u64, u64)> {
        let mut c = Counters {
            cb: std::mem::size_of::<Counters>() as u32,
            ..Counters::default()
        };
        // SAFETY: the pseudo handle of the own process and a correctly sized, writable struct of the documented
        // layout; the call only writes into it.
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        (ok != 0).then_some((c.rss as u64, c.peak_rss as u64, c.private_usage as u64))
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn get(self) -> u64 {
            (u64::from(self.high) << 32) | u64::from(self.low)
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }

    pub fn cpu_times() -> Option<(u64, u64, u64)> {
        let mut t = [FileTime::default(); 7];
        let [idle, kernel, user, created, exited, pk, pu] = &mut t;
        // SAFETY: writable FILETIME-layout outputs; the own process's pseudo handle
        let ok = unsafe {
            GetSystemTimes(idle, kernel, user) != 0
                && GetProcessTimes(GetCurrentProcess(), created, exited, pk, pu) != 0
        };
        // the system kernel time includes the idle time
        ok.then(|| {
            let busy = (t[1].get() + t[2].get()).saturating_sub(t[0].get());
            (t[0].get(), busy, t[5].get() + t[6].get())
        })
    }
}

#[cfg(not(windows))]
mod os {
    pub fn process_memory() -> Option<(u64, u64, u64)> {
        None
    }

    pub fn cpu_times() -> Option<(u64, u64, u64)> {
        None
    }
}

// ---- GPU pass timestamps ----

/// Frames whose timestamps can be outstanding at once.
pub const TIMER_RING: usize = 4;
/// Passes timed per frame (two queries each).
pub const MAX_TIMED_PASSES: usize = 16;
/// Collected results kept until the profiler takes them.
pub const TIMER_RESULTS_KEPT: usize = 64;
const SLOT_QUERIES: u32 = 2 * MAX_TIMED_PASSES as u32;
/// Bytes of one slot's results (256: the resolve offset alignment).
const SLOT_BYTES: u64 = SLOT_QUERIES as u64 * 8;

const FREE: u8 = 0;
const RECORDING: u8 = 1;
const RESOLVED: u8 = 2;
const MAPPING: u8 = 3;
const MAPPED: u8 = 4;
const MAP_FAILED: u8 = 5;

struct Slot {
    readback: wgpu::Buffer,
    state: Arc<AtomicU8>,
    frame: u64,
    passes: Vec<&'static str>,
}

/// The GPU times of one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuFrame {
    /// The frame number given to [`gpu_frame_begin`].
    pub frame: u64,
    /// Each timed pass in recording order: name and ms from its begin to its end.
    pub passes: Vec<(&'static str, f64)>,
    /// From the first pass's begin to the last pass's end, ms.
    pub span_ms: f64,
}

/// Timer statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuTimerStats {
    /// The device has a timer (`TIMESTAMP_QUERY` granted).
    pub available: bool,
    /// Frames not timed because their slot was still waiting for an older frame's results.
    pub skipped: u64,
    /// Passes not timed (more than [`MAX_TIMED_PASSES`] in a frame).
    pub overflow: u64,
    /// Results dropped because nobody took them ([`TIMER_RESULTS_KEPT`]).
    pub dropped: u64,
    /// Read-backs that failed to map.
    pub failed: u64,
    /// Frames timed and collected.
    pub collected: u64,
}

/// Per-pass GPU timestamps with a ring of read-back slots (see the module text).
pub struct GpuTimer {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: Vec<Slot>,
    period_ns: f64,
    current: Option<usize>,
    results: VecDeque<GpuFrame>,
    stats: GpuTimerStats,
}

static TIMER: Mutex<Option<GpuTimer>> = Mutex::new(None);

/// Installs the timer for a device that has `TIMESTAMP_QUERY` (the latest device wins; called by
/// [`crate::GpuContext`] when [`timestamps_wanted`]).
pub fn install_timer(device: &wgpu::Device, queue: &wgpu::Queue) {
    let set = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("perf timestamps"),
        ty: wgpu::QueryType::Timestamp,
        count: SLOT_QUERIES * TIMER_RING as u32,
    });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("perf timestamp resolve"),
        size: SLOT_BYTES * TIMER_RING as u64,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let slots = (0..TIMER_RING)
        .map(|_| Slot {
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("perf timestamp readback"),
                size: SLOT_BYTES,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            state: Arc::new(AtomicU8::new(FREE)),
            frame: 0,
            passes: Vec::with_capacity(MAX_TIMED_PASSES),
        })
        .collect();
    let timer = GpuTimer {
        set,
        resolve,
        slots,
        period_ns: queue.get_timestamp_period() as f64,
        current: None,
        results: VecDeque::with_capacity(TIMER_RESULTS_KEPT),
        stats: GpuTimerStats {
            available: true,
            ..GpuTimerStats::default()
        },
    };
    // a device loss can unwind out of a call that held the lock (gpu-recover): the new device's timer clears the
    // poison; the old device's timer is never dropped (its read-back buffers belong to the lost device)
    TIMER.clear_poison();
    let mut t = TIMER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old) = t.replace(timer) {
        std::mem::forget(old);
    }
}

/// Starts timing frame `frame` (nothing without a timer; a frame whose slot is busy is not timed).
pub fn gpu_frame_begin(frame: u64) {
    if thread_untimed() {
        return;
    }
    let Ok(mut g) = TIMER.lock() else { return };
    let Some(t) = g.as_mut() else { return };
    let i = (frame % TIMER_RING as u64) as usize;
    let s = &mut t.slots[i];
    if s.state.load(Relaxed) != FREE {
        t.stats.skipped += 1;
        t.current = None;
        return;
    }
    s.state.store(RECORDING, Relaxed);
    s.frame = frame;
    s.passes.clear();
    t.current = Some(i);
}

/// A pass's timestamp writes (hold it while the pass is recorded; `None`: not timed).
pub struct PassStamp {
    set: wgpu::QuerySet,
    begin: u32,
}

impl PassStamp {
    /// The descriptor field of the pass.
    pub fn writes(&self) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some(self.begin),
            end_of_pass_write_index: Some(self.begin + 1),
        }
    }
}

/// The timestamp writes for the next pass of the current frame, named `name` in the results.
pub fn pass_stamp(name: &'static str) -> Option<PassStamp> {
    if !on() || thread_untimed() {
        return None;
    }
    let mut g = TIMER.lock().ok()?;
    let t = g.as_mut()?;
    let i = t.current?;
    let s = &mut t.slots[i];
    if s.passes.len() >= MAX_TIMED_PASSES {
        t.stats.overflow += 1;
        return None;
    }
    let begin = i as u32 * SLOT_QUERIES + 2 * s.passes.len() as u32;
    s.passes.push(name);
    Some(PassStamp {
        set: t.set.clone(),
        begin,
    })
}

/// Records the current frame's resolve and copy into `encoder` (call after its last timed pass, before `finish`).
pub fn gpu_frame_end(encoder: &mut wgpu::CommandEncoder) {
    if thread_untimed() {
        return;
    }
    let Ok(mut g) = TIMER.lock() else { return };
    let Some(t) = g.as_mut() else { return };
    let Some(i) = t.current.take() else { return };
    let s = &t.slots[i];
    let n = 2 * s.passes.len() as u32;
    if n == 0 {
        s.state.store(FREE, Relaxed);
        return;
    }
    let base = i as u32 * SLOT_QUERIES;
    encoder.resolve_query_set(&t.set, base..base + n, &t.resolve, i as u64 * SLOT_BYTES);
    encoder.copy_buffer_to_buffer(
        &t.resolve,
        i as u64 * SLOT_BYTES,
        &s.readback,
        0,
        n as u64 * 8,
    );
    s.state.store(RESOLVED, Relaxed);
}

/// Asks for the read-back of every frame resolved since the last call (call right after the submit).
pub fn gpu_frame_submitted() {
    if thread_untimed() {
        return;
    }
    let Ok(g) = TIMER.lock() else { return };
    let Some(t) = g.as_ref() else { return };
    for s in &t.slots {
        if s.state.load(Relaxed) == RESOLVED {
            s.state.store(MAPPING, Relaxed);
            let st = s.state.clone();
            s.readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    st.store(if r.is_ok() { MAPPED } else { MAP_FAILED }, Relaxed);
                });
        }
    }
}

/// Polls `device` (without waiting) and moves every finished read-back into the result list.
pub fn gpu_collect(device: &wgpu::Device) {
    let Ok(mut g) = TIMER.lock() else { return };
    let Some(t) = g.as_mut() else { return };
    if t.slots.iter().any(|s| s.state.load(Relaxed) == MAPPING) {
        let _ = device.poll(wgpu::PollType::Poll);
    }
    for s in &mut t.slots {
        match s.state.load(Relaxed) {
            MAPPED => {
                let n = 2 * s.passes.len();
                let ticks: Vec<u64> = {
                    let data = s.readback.slice(..).get_mapped_range();
                    let (words, _) = data.as_chunks::<8>();
                    words
                        .iter()
                        .take(n)
                        .map(|c| u64::from_le_bytes(*c))
                        .collect()
                };
                s.readback.unmap();
                let ms = |a: u64, b: u64| b.saturating_sub(a) as f64 * t.period_ns / 1e6;
                let passes = s
                    .passes
                    .iter()
                    .enumerate()
                    .map(|(k, name)| (*name, ms(ticks[2 * k], ticks[2 * k + 1])))
                    .collect();
                let first = ticks.iter().step_by(2).copied().min().unwrap_or(0);
                let last = ticks.iter().skip(1).step_by(2).copied().max().unwrap_or(0);
                if t.results.len() >= TIMER_RESULTS_KEPT {
                    t.results.pop_front();
                    t.stats.dropped += 1;
                }
                t.results.push_back(GpuFrame {
                    frame: s.frame,
                    passes,
                    span_ms: ms(first, last),
                });
                t.stats.collected += 1;
                s.state.store(FREE, Relaxed);
            }
            MAP_FAILED => {
                s.readback.unmap();
                t.stats.failed += 1;
                s.state.store(FREE, Relaxed);
            }
            _ => {}
        }
    }
}

/// Takes the collected results (oldest first).
pub fn gpu_take() -> Vec<GpuFrame> {
    TIMER
        .lock()
        .ok()
        .and_then(|mut g| g.as_mut().map(|t| t.results.drain(..).collect()))
        .unwrap_or_default()
}

/// The timer's statistics (`available` false without a timer).
pub fn gpu_stats() -> GpuTimerStats {
    TIMER
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|t| t.stats))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_bytes_cover_mips_and_blocks() {
        let d = |format, w, h, mips| wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        assert_eq!(
            texture_bytes(&d(wgpu::TextureFormat::Rgba8Unorm, 4, 4, 1)),
            64
        );
        // 4x4 + 2x2 + 1x1 texels of 4 bytes
        assert_eq!(
            texture_bytes(&d(wgpu::TextureFormat::Rgba8Unorm, 4, 4, 3)),
            84
        );
        // BC1: 8 bytes per 4x4 block, a 1x1 mip still takes a block
        assert_eq!(
            texture_bytes(&d(wgpu::TextureFormat::Bc1RgbaUnorm, 8, 8, 2)),
            32 + 8
        );
    }

    #[test]
    fn counters_and_gauges_move_only_when_on() {
        // the telemetry is process-wide: this test switches it on, so it only checks monotonic movement
        let before = counters()[Counter::IoBytes as usize];
        enable(false);
        add(Counter::IoBytes, 5);
        assert!(counters()[Counter::IoBytes as usize] >= before + 5);
        gauge(Gauge::LoaderJobs, 3);
        gauge(Gauge::LoaderJobs, -3);
        let (_, high) = gauges()[Gauge::LoaderJobs as usize];
        assert!(high >= 3);
        {
            let _s = span(Counter::ParseNs);
        }
        assert!(timestamps_wanted() || !WANT_TIMESTAMPS.load(Relaxed));
    }

    #[test]
    fn violations_are_bounded() {
        for i in 0..(VIOLATIONS_KEPT + 5) {
            violation("pipeline", Some(&format!("p{i}")));
        }
        let (v, dropped) = take_violations();
        assert!(v.len() <= VIOLATIONS_KEPT);
        assert!(dropped >= 5);
        assert!(take_violations().0.is_empty());
    }

    #[test]
    fn cpu_load_of_two_readings() {
        assert_eq!(cpu_load((10, 10, 0), (40, 40, 15)), (0.5, 0.25));
        assert_eq!(cpu_load((1, 1, 1), (1, 1, 1)), (0.0, 0.0));
        if cfg!(windows) {
            let a = cpu_times().expect("cpu times");
            let b = cpu_times().expect("cpu times");
            let (load, own) = cpu_load(a, b);
            assert!((0.0..=1.0).contains(&load) && (0.0..=1.0).contains(&own));
        }
    }

    #[test]
    fn streams_measure_request_to_done() {
        enable(false);
        set_frame(10);
        stream_request(0xfeed_0001);
        stream_request(0xfeed_0001);
        set_frame(13);
        stream_done(0xfeed_0001);
        stream_done(0xfeed_0002);
        let (done, _, _) = stream_take();
        assert!(done.iter().any(|l| l.frames == 3), "{done:?}");
    }

    #[test]
    fn process_memory_reads_on_windows() {
        if cfg!(windows) {
            let (ws, peak, _) = process_memory().expect("working set");
            assert!(ws > 0 && peak >= ws);
        }
    }
}
