//! gpu-prewarm: where and when GPU pipelines and shader modules are made. A match makes every pipeline it can need
//! up front, on the frame thread, before its first frame ([`begin_play`] marks the end of that); one made later is a
//! "made in play" create, counted and logged once by what it is (a render style that only showed up mid-match, a
//! pass a new setting needs). Threads that must never make one (the model loader threads) say so with
//! [`forbid_on_this_thread`]; the model pass then hands the pipelines a load needs back to the frame thread
//! (`ModelPipelines::make_pending`). Every creation through [`crate::render_pipeline`] / [`crate::shader_module`] is
//! counted here (one relaxed atomic add; creations are load-time events), so tests can require "no pipeline made
//! during N frames of play".
//!
//! Bounds: counters are atomics; the list of in-play / forbidden-thread creates keeps [`MADE_KEPT`] entries and
//! counts the rest.
// ORIGINAL: gpu-prewarm engineering (our own resource timing; no 2009 behaviour)

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

static IN_PLAY: AtomicBool = AtomicBool::new(false);
static CREATES: AtomicU64 = AtomicU64::new(0);
static PLAY_CREATES: AtomicU64 = AtomicU64::new(0);
static FORBIDDEN_CREATES: AtomicU64 = AtomicU64::new(0);
static UNREADY_DRAWS: AtomicU64 = AtomicU64::new(0);
static MADE: Mutex<Vec<Made>> = Mutex::new(Vec::new());
static MADE_DROPPED: AtomicU64 = AtomicU64::new(0);

/// In-play / forbidden-thread creates kept for [`made`].
pub const MADE_KEPT: usize = 64;

thread_local! {
    static FORBIDDEN: Cell<bool> = const { Cell::new(false) };
    /// CLI-1933: the backend of the device this thread makes pipelines on (for the slow-pipeline line).
    static BACKEND: Cell<Option<wgpu::Backend>> = const { Cell::new(None) };
}

/// CLI-1933: called after every pipeline made (the client's frame watchdog counts it as progress: a load that makes
/// many slow pipelines is slow, not hung).
static PROGRESS: OnceLock<fn()> = OnceLock::new();
static PIPES: AtomicU64 = AtomicU64::new(0);
static PIPE_US: AtomicU64 = AtomicU64::new(0);
static PIPE_MAX_US: AtomicU64 = AtomicU64::new(0);

/// CLI-1933: a pipeline that takes longer than this to make is logged with its label, raster state and backend
/// (`DC_SLOW_PIPELINE_MS` overrides it; 0 logs every pipeline).
// DC-DEFAULT: CLI-1933 250 ms
pub const SLOW_PIPELINE_MS: u64 = 250;

/// CLI-1933: `f` runs after every pipeline made, on the thread that made it (set once per process).
pub fn set_progress_hook(f: fn()) {
    let _ = PROGRESS.set(f);
}

/// CLI-1933: the calling thread makes pipelines on a device of `backend` (named in the slow-pipeline line).
pub fn set_thread_backend(backend: wgpu::Backend) {
    let _ = BACKEND.try_with(|b| b.set(Some(backend)));
}

/// CLI-1933: the backend [`set_thread_backend`] named on this thread.
pub fn thread_backend() -> Option<wgpu::Backend> {
    BACKEND.try_with(Cell::get).ok().flatten()
}

/// CLI-1933: pipelines made so far, their summed and their longest creation time.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PipelineTimes {
    pub count: u64,
    pub total_ms: f64,
    pub max_ms: f64,
}

/// CLI-1933: starts a new longest time ([`PipelineTimes::max_ms`] covers the pipelines made from now on; the count
/// and the sum keep running).
pub fn restart_longest() {
    PIPE_MAX_US.store(0, Relaxed);
}

/// CLI-1933: the creation times of every pipeline made so far (all threads); `max_ms` since [`restart_longest`].
pub fn pipeline_times() -> PipelineTimes {
    PipelineTimes {
        count: PIPES.load(Relaxed),
        total_ms: PIPE_US.load(Relaxed) as f64 / 1000.0,
        max_ms: PIPE_MAX_US.load(Relaxed) as f64 / 1000.0,
    }
}

fn slow_ms() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("DC_SLOW_PIPELINE_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(SLOW_PIPELINE_MS)
    })
}

/// CLI-1933: the log line of a slow pipeline.
pub fn slow_line(what: &str, ms: f64, backend: Option<wgpu::Backend>) -> String {
    let backend = backend.map_or_else(|| "?".to_string(), |b| format!("{b:?}"));
    format!("dc-render: pipeline {what} compiled in {ms:.0} ms ({backend})")
}

/// CLI-1933: books one pipeline made in `took`: the times, the slow line, the progress hook.
pub(crate) fn pipeline_made(label: Option<&str>, detail: Option<&str>, took: Duration) {
    let us = took.as_micros().min(u128::from(u64::MAX)) as u64;
    PIPES.fetch_add(1, Relaxed);
    PIPE_US.fetch_add(us, Relaxed);
    PIPE_MAX_US.fetch_max(us, Relaxed);
    if us >= slow_ms().saturating_mul(1000) {
        let mut what = label.unwrap_or("(unlabelled)").to_string();
        if let Some(d) = detail {
            what.push(' ');
            what.push_str(d);
        }
        println!("{}", slow_line(&what, us as f64 / 1000.0, thread_backend()));
    }
    if let Some(f) = PROGRESS.get() {
        f();
    }
}

/// One pipeline or shader module made in play or on a thread that forbids it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    /// `pipeline` or `shader`.
    pub kind: &'static str,
    /// The label, and for a model pipeline its raster state.
    pub what: String,
    /// The creating thread's name (`?` when it has none).
    pub thread: String,
    /// It was made on a thread that forbids creates (a loader thread): a bug.
    pub forbidden: bool,
}

/// The match's GPU objects are made: creates from now on are "made in play".
pub fn begin_play() {
    IN_PLAY.store(true, Relaxed);
}

/// A load phase starts (match load, device rebuild, renderer rebuild) or the match ended.
pub fn end_play() {
    IN_PLAY.store(false, Relaxed);
}

/// The match is in play ([`begin_play`] without [`end_play`] since).
pub fn in_play() -> bool {
    IN_PLAY.load(Relaxed)
}

/// The calling thread never makes a pipeline or shader module (the model loader threads).
pub fn forbid_on_this_thread() {
    let _ = FORBIDDEN.try_with(|f| f.set(true));
}

/// The calling thread may make pipelines.
pub fn allowed_here() -> bool {
    !FORBIDDEN.try_with(Cell::get).unwrap_or(false)
}

/// Pipelines and shader modules made so far (all threads, all phases).
pub fn creates() -> u64 {
    CREATES.load(Relaxed)
}

/// Pipelines and shader modules made while in play.
pub fn play_creates() -> u64 {
    PLAY_CREATES.load(Relaxed)
}

/// Pipelines and shader modules made on a thread that forbids it (must stay 0).
pub fn forbidden_creates() -> u64 {
    FORBIDDEN_CREATES.load(Relaxed)
}

/// Model passes skipped because their pipeline was not made yet (must stay 0: the frame makes them first).
pub fn unready_draws() -> u64 {
    UNREADY_DRAWS.load(Relaxed)
}

pub(crate) fn unready_draw() {
    UNREADY_DRAWS.fetch_add(1, Relaxed);
}

/// The in-play and forbidden-thread creates kept so far (oldest first) and how many were not kept.
pub fn made() -> (Vec<Made>, u64) {
    let v = MADE.lock().map(|v| v.clone()).unwrap_or_default();
    (v, MADE_DROPPED.load(Relaxed))
}

/// Books one creation of `kind` (`pipeline`, `shader`) named `label` (`detail`: the model raster state).
pub(crate) fn note(kind: &'static str, label: Option<&str>, detail: Option<&str>) {
    CREATES.fetch_add(1, Relaxed);
    let forbidden = !allowed_here();
    let play = in_play();
    if !forbidden && !play {
        return;
    }
    if forbidden {
        FORBIDDEN_CREATES.fetch_add(1, Relaxed);
    }
    if play {
        PLAY_CREATES.fetch_add(1, Relaxed);
    }
    let mut what = label.unwrap_or("(unlabelled)").to_string();
    if let Some(d) = detail {
        what.push(' ');
        what.push_str(d);
    }
    let thread = std::thread::current().name().unwrap_or("?").to_string();
    if forbidden {
        println!("dc-render: {kind} made on a loader thread (a bug): {what} (thread {thread})");
    } else {
        println!("dc-render: {kind} made in play: {what} (thread {thread})");
    }
    let Ok(mut v) = MADE.lock() else {
        return;
    };
    if v.len() < MADE_KEPT {
        v.push(Made {
            kind,
            what,
            thread,
            forbidden,
        });
    } else {
        MADE_DROPPED.fetch_add(1, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forbidding_thread_is_seen_as_such_and_others_are_not() {
        assert!(allowed_here());
        let t = std::thread::spawn(|| {
            forbid_on_this_thread();
            allowed_here()
        });
        assert!(!t.join().unwrap());
        assert!(allowed_here());
    }

    // CLI-1933: the slow-pipeline line names what, how long and the backend; a thread knows its own backend
    #[test]
    fn a_slow_pipeline_line_names_the_state_and_the_backend() {
        assert_eq!(
            slow_line(
                "model Blend(One, Zero) style RS\\A.LTB",
                812.4,
                Some(wgpu::Backend::Dx12)
            ),
            "dc-render: pipeline model Blend(One, Zero) style RS\\A.LTB compiled in 812 ms (Dx12)"
        );
        assert_eq!(
            slow_line("gamma", 300.0, None),
            "dc-render: pipeline gamma compiled in 300 ms (?)"
        );
        set_thread_backend(wgpu::Backend::Vulkan);
        assert_eq!(thread_backend(), Some(wgpu::Backend::Vulkan));
        assert_eq!(std::thread::spawn(thread_backend).join().unwrap(), None);
    }

    static HOOKED: AtomicU64 = AtomicU64::new(0);

    fn bump() {
        HOOKED.fetch_add(1, Relaxed);
    }

    #[test]
    fn every_pipeline_made_is_timed_and_reported_as_progress() {
        set_progress_hook(bump);
        let hooked = HOOKED.load(Relaxed);
        let before = pipeline_times();
        pipeline_made(Some("t"), None, Duration::from_millis(3));
        let after = pipeline_times();
        assert!(after.count > before.count);
        assert!(after.total_ms >= before.total_ms + 3.0 - 1e-9);
        assert!(after.max_ms >= 3.0 - 1e-9);
        assert!(HOOKED.load(Relaxed) > hooked, "the progress hook ran");
    }
}
