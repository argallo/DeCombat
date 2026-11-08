//! A start that did not reach its first frames on the saved renderer starts on the other one next time (CLI-1932).
//!
//! Why: a renderer can take the whole process down before the player sees anything (a driver fault inside device or
//! pipeline creation, a wgpu error on one backend only): field reports 2026-11-06 (Vulkan crashed at present on a
//! GTX 960, DirectX 12 worked) and 2026-11-07 (DirectX 12 crashed on an Intel integrated GPU, Vulkan worked). Such
//! a crash happened again at every start until the player found the Renderer row by hand.
//!
//! How: before the window opens, a double-click start (`DC_PACK_START`) writes `renderer-start.txt` into the logs
//! folder naming the saved backend; the window clears it after [`CLEAR_AFTER_FRAMES`] presented frames, and an
//! orderly end of the window clears it too. A marker still there at the next start, naming the saved backend, means
//! that start died early: this start uses the other backend, saves it as the Renderer choice (the player can pick
//! the old one again in the options) and tells the player in the lobby chat. `--adapter` wins: nothing is read,
//! switched or written then.
// ORIGINAL: CLI-1932 our own start guard around the backend choice (CLI-1852); no 2009 behaviour
// DC-DEFAULT: CLI-1932 (proposed), see DC_DEFAULTS.md

use crate::settings::Renderer;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// The marker's file name in the logs folder.
pub const MARKER_FILE: &str = "renderer-start.txt";
/// Presented frames after which the start counts as finished and the marker goes.
// DC-DEFAULT: CLI-1932 300 frames (a few seconds of the menu or the match)
pub const CLEAR_AFTER_FRAMES: u64 = 300;
/// CLI-1949 (amends CLI-1932): and at least this long since the window start. A menu at its frame cap passes 300
/// frames in well under a second, so a crash a few seconds into the start (report 2026-11-08: an overlay's Vulkan
/// layer at present) never switched the backend; both conditions must hold, so a stalled present does not clear it
/// early either.
pub const CLEAR_AFTER_MS: u64 = 10_000;

/// The marker file of one folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartMarker {
    path: PathBuf,
}

impl StartMarker {
    /// The marker in `dir` (the logs folder).
    pub fn in_dir(dir: &Path) -> StartMarker {
        StartMarker {
            path: dir.join(MARKER_FILE),
        }
    }

    /// The backend an earlier start left in the marker (`None`: no marker, or not one of ours).
    pub fn stale(&self) -> Option<Renderer> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        text.trim()
            .strip_prefix("renderer ")
            .and_then(Renderer::from_name)
    }

    /// Notes that a start on `r` begins.
    pub fn write(&self, r: Renderer) -> std::io::Result<()> {
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&self.path, format!("renderer {}\n", r.name()))
    }

    /// The start finished: the marker goes (a missing file is fine).
    pub fn clear(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// What a start does with the saved renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartPlan {
    /// The backend this start asks for first.
    pub renderer: Renderer,
    /// The renderer was switched from the saved one (save it, tell the player).
    pub switched: bool,
}

/// The plan of a start: an `explicit` `--adapter` keeps the saved value untouched; a marker naming the saved
/// backend switches to the other one; anything else (no marker, or a marker of another backend: the player changed
/// the row since) keeps the saved one.
pub fn plan(stale: Option<Renderer>, saved: Renderer, explicit: bool) -> StartPlan {
    let switched = !explicit && stale == Some(saved);
    StartPlan {
        renderer: if switched { saved.other() } else { saved },
        switched,
    }
}

/// The player's line about a switch (lobby chat and `dc-client.out`).
pub fn switch_notice(from: Renderer, to: Renderer) -> String {
    format!(
        "The game did not finish starting on {} last time; using {} this start. Options > Graphics changes it.",
        from.label(),
        to.label()
    )
}

/// One start through `marker`: reads the earlier marker, plans, writes the new marker (not when `explicit`).
/// Returns the plan and the notice of a switch.
pub fn start_with(
    marker: &StartMarker,
    saved: Renderer,
    explicit: bool,
) -> (StartPlan, Option<String>) {
    if explicit {
        return (plan(None, saved, true), None);
    }
    let p = plan(marker.stale(), saved, false);
    let notice = p.switched.then(|| switch_notice(saved, p.renderer));
    if let Err(e) = marker.write(p.renderer) {
        println!("dc-client: renderer start marker not written ({e})");
    }
    (p, notice)
}

static ARMED: Mutex<Option<StartMarker>> = Mutex::new(None);
static PRESENTED: AtomicU64 = AtomicU64::new(0);
/// CLI-1949: when the window run started (`None` before [`at_window_start`]).
static STARTED: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// CLI-1949: the marker may go once `frames` were presented and `elapsed_ms` passed since the window start.
pub fn clear_due(frames: u64, elapsed_ms: u64) -> bool {
    frames >= CLEAR_AFTER_FRAMES && elapsed_ms >= CLEAR_AFTER_MS
}

/// The start of a window run with the saved renderer `saved` (no `--adapter`): only a double-click start
/// (`DC_PACK_START`, its logs folder) keeps a marker; any other run keeps `saved` and writes nothing. A switch is
/// printed, queued for the lobby chat and handed to `persist` (the Renderer row).
pub fn at_window_start(saved: Renderer, persist: impl FnOnce(Renderer)) -> Renderer {
    let pack = std::env::var_os(crate::selfupdate::PACK_START_ENV).is_some_and(|v| !v.is_empty());
    let Some(dir) = crate::diag::default_logs_dir().filter(|_| pack) else {
        return saved;
    };
    let marker = StartMarker::in_dir(&dir);
    let (p, notice) = start_with(&marker, saved, false);
    if let Some(n) = notice {
        println!("dc-client: {n}");
        crate::diag::player_notice(n);
        persist(p.renderer);
    }
    if let Ok(mut a) = ARMED.lock() {
        *a = Some(marker);
    }
    if let Ok(mut s) = STARTED.lock() {
        *s = Some(std::time::Instant::now());
    }
    PRESENTED.store(0, Ordering::Relaxed);
    p.renderer
}

/// A window presented a frame: from the [`CLEAR_AFTER_FRAMES`]-th on, once [`CLEAR_AFTER_MS`] passed since the
/// window start, the marker goes (one atomic add per frame otherwise; nothing once it is gone).
pub fn presented() {
    let n = PRESENTED.fetch_add(1, Ordering::Relaxed) + 1;
    if n < CLEAR_AFTER_FRAMES || n % 16 != 0 {
        return;
    }
    let armed = ARMED.lock().is_ok_and(|a| a.is_some());
    if !armed {
        return;
    }
    let elapsed = STARTED
        .lock()
        .ok()
        .and_then(|s| s.map(|t| t.elapsed().as_millis() as u64))
        .unwrap_or(u64::MAX);
    if clear_due(n, elapsed) {
        finished();
    }
}

/// The start finished (enough frames, or the window ended in order): the marker goes.
pub fn finished() {
    if let Some(m) = ARMED.lock().ok().and_then(|mut a| a.take()) {
        m.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI-1949: frames alone do not clear the marker, time alone does not either.
    #[test]
    fn the_marker_clears_after_both_the_frames_and_the_seconds() {
        assert!(!clear_due(CLEAR_AFTER_FRAMES, 0));
        assert!(!clear_due(CLEAR_AFTER_FRAMES - 1, CLEAR_AFTER_MS));
        assert!(!clear_due(100_000, CLEAR_AFTER_MS - 1));
        assert!(clear_due(CLEAR_AFTER_FRAMES, CLEAR_AFTER_MS));
        assert!(clear_due(100_000, 60_000));
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-startguard-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn the_marker_is_written_read_and_cleared() {
        let d = dir("rw");
        let m = StartMarker::in_dir(&d);
        assert_eq!(m.stale(), None);
        m.write(Renderer::Dx12).unwrap();
        assert_eq!(m.stale(), Some(Renderer::Dx12));
        m.write(Renderer::Vulkan).unwrap();
        assert_eq!(m.stale(), Some(Renderer::Vulkan));
        m.clear();
        assert_eq!(m.stale(), None);
        m.clear();
        std::fs::write(d.join(MARKER_FILE), "something else").unwrap();
        assert_eq!(m.stale(), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_marker_left_by_the_saved_renderer_switches_to_the_other() {
        let d = dir("stale");
        let m = StartMarker::in_dir(&d);
        // a clean first start: the saved renderer, marker written
        let (p, n) = start_with(&m, Renderer::Dx12, false);
        assert_eq!((p.renderer, p.switched, n), (Renderer::Dx12, false, None));
        assert_eq!(m.stale(), Some(Renderer::Dx12));
        // that start died before its frames: the next one uses Vulkan and says so
        let (p, n) = start_with(&m, Renderer::Dx12, false);
        assert_eq!((p.renderer, p.switched), (Renderer::Vulkan, true));
        assert_eq!(
            n.as_deref(),
            Some(
                "The game did not finish starting on DirectX 12 last time; using Vulkan this start. \
                 Options > Graphics changes it."
            )
        );
        assert_eq!(m.stale(), Some(Renderer::Vulkan));
        // the Vulkan start finished: the start after it keeps the (now saved) Vulkan
        m.clear();
        let (p, n) = start_with(&m, Renderer::Vulkan, false);
        assert_eq!((p.renderer, p.switched, n), (Renderer::Vulkan, false, None));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_marker_of_another_backend_keeps_the_saved_choice() {
        // the player picked DirectX 12 again in the options after a Vulkan start died
        assert_eq!(
            plan(Some(Renderer::Vulkan), Renderer::Dx12, false),
            StartPlan {
                renderer: Renderer::Dx12,
                switched: false
            }
        );
        assert!(!plan(None, Renderer::Vulkan, false).switched);
        assert_eq!(
            plan(Some(Renderer::Vulkan), Renderer::Vulkan, false).renderer,
            Renderer::Dx12
        );
    }

    #[test]
    fn an_explicit_adapter_wins_and_touches_no_marker() {
        let d = dir("explicit");
        let m = StartMarker::in_dir(&d);
        m.write(Renderer::Dx12).unwrap();
        let (p, n) = start_with(&m, Renderer::Dx12, true);
        assert_eq!((p.renderer, p.switched, n), (Renderer::Dx12, false, None));
        assert_eq!(
            m.stale(),
            Some(Renderer::Dx12),
            "the marker stays as it was"
        );
        let d2 = dir("explicit-none");
        let m2 = StartMarker::in_dir(&d2);
        let _ = start_with(&m2, Renderer::Vulkan, true);
        assert_eq!(m2.stale(), None, "nothing written");
        let _ = std::fs::remove_dir_all(&d);
    }
}
