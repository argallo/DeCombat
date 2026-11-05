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
use std::sync::Mutex;

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
}
