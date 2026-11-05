//! Keeping a window's frames coming when the window system stops delivering its redraw events.
//!
//! Both windows (the menu, the match) draw from `RedrawRequested` and ask for the next one at the end of every frame:
//! a chain that ends for good when one redraw event never arrives. On Windows the request is an internal paint
//! (`RedrawWindow` with `RDW_INTERNALPAINT` in winit 0.30), and the playtest of 2026-11-04 (build 819ca921) saw the
//! chain end in a full-screen (borderless) match on alt-tab: no frame for 30 s while the event loop ran (focus
//! events, the net log, the watchdog's pump), the last frame shown on screen; Alt+Enter (a resize, a real repaint)
//! started it again. Our match code has no path that leaves the chain without a log line, so the event was dropped
//! on the system's side (the window being minimised / restored behind the task switcher).
//!
//! The keeper notes every delivered redraw. Once none came for [`STALL_MS`] while the window is visible, the loop
//! draws the frames itself from `about_to_wait` (paced as before) and asks for a redraw with each, until a redraw
//! event arrives again. The surface is configured again before the first such frame and after the window was shown
//! or focused again (a stale surface of the full-screen window is what a resize repairs).
// DC-DEFAULT: CLI-1848 (proposed, project decision)
// PUBLIC: winit 0.30 docs (`Window::request_redraw`, `ApplicationHandler::about_to_wait`, `Window::is_minimized`);
// Win32 RedrawWindow documentation (RDW_INTERNALPAINT); wgpu 25 docs (`Surface::get_current_texture`, SurfaceError)

/// How long without a delivered redraw event before the loop draws on its own (ms). Well above a slow frame of a
/// capped frame rate and short enough to read as a hitch, not a freeze.
pub const STALL_MS: u64 = 250;

/// What the loop does about its frames after a turn of events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// Nothing: the redraw events come.
    Idle,
    /// Draw a frame now, as the redraw event would (and ask for a redraw again).
    Draw,
}

/// A change of the keeper worth one log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// The loop draws on its own from now on: no redraw event for this many ms.
    Stalled(u64),
    /// A redraw event came again after this many ms of frames drawn by the loop.
    Resumed(u64),
}

/// The state of one window's redraw chain; times are ms on any one clock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RedrawKeeper {
    /// When the last frame ended (a delivered redraw or one the loop drew), `None` before the window's first.
    last_ms: Option<u64>,
    /// When the loop started drawing on its own (`None`: the redraw events come).
    fallback_since: Option<u64>,
    /// The surface is configured again before the next frame.
    reconfigure: bool,
    /// Acquire failures in a row (the first of a run is logged).
    acquire_fails: u32,
}

impl RedrawKeeper {
    /// The window starts its frames at `now_ms` (opened, handed over, or back from a device rebuild).
    pub fn arm(&mut self, now_ms: u64) {
        self.last_ms = Some(now_ms);
        self.fallback_since = None;
    }

    /// A frame drawn for a delivered redraw event ended at `now_ms`. `Some` when that ends the loop's own frames.
    pub fn on_redraw_event(&mut self, now_ms: u64) -> Option<Note> {
        self.last_ms = Some(now_ms);
        self.fallback_since
            .take()
            .map(|t| Note::Resumed(now_ms.saturating_sub(t)))
    }

    /// A frame the loop drew itself ended at `now_ms`.
    pub fn on_loop_frame(&mut self, now_ms: u64) {
        self.last_ms = Some(now_ms);
    }

    /// The window was shown or focused again (or changed its full-screen look): the surface is configured again
    /// before the next frame.
    pub fn on_shown(&mut self) {
        self.reconfigure = true;
    }

    /// The surface is configured again before this frame (taken once).
    pub fn take_reconfigure(&mut self) -> bool {
        std::mem::take(&mut self.reconfigure)
    }

    /// The frame's surface texture was acquired (`ok`) or not; `true` when the failure starts a run (log it). A
    /// failed acquire configures the surface again (the caller does it at once) and the frame is skipped.
    pub fn on_acquire(&mut self, ok: bool) -> bool {
        if ok {
            self.acquire_fails = 0;
            return false;
        }
        self.acquire_fails = self.acquire_fails.saturating_add(1);
        self.acquire_fails == 1
    }

    /// The loop is due to look at the window (cheap: no window query): a frame is overdue or the loop draws on its
    /// own already.
    pub fn due(&self, now_ms: u64) -> bool {
        match (self.last_ms, self.fallback_since) {
            (None, _) => false,
            (Some(_), Some(_)) => true,
            (Some(last), None) => now_ms.saturating_sub(last) >= STALL_MS,
        }
    }

    /// After a turn of events at `now_ms`, with the window `visible` (not minimised, a non-zero client area):
    /// whether the loop draws a frame itself, and a note to log. A hidden window draws nothing and the loop stays in
    /// its current mode.
    pub fn poll(&mut self, now_ms: u64, visible: bool) -> (Wake, Option<Note>) {
        if !self.due(now_ms) || !visible {
            return (Wake::Idle, None);
        }
        let mut note = None;
        if self.fallback_since.is_none() {
            let gap = self.last_ms.map_or(0, |l| now_ms.saturating_sub(l));
            self.fallback_since = Some(now_ms);
            self.reconfigure = true;
            note = Some(Note::Stalled(gap));
        }
        (Wake::Draw, note)
    }

    /// Whether the loop draws the frames itself.
    pub fn drawing_from_loop(&self) -> bool {
        self.fallback_since.is_some()
    }
}

/// The log line of a keeper note (`who`: "dc-client" or "dc-client menu").
pub fn note_line(who: &str, note: Note) -> String {
    match note {
        Note::Stalled(ms) => format!(
            "{who}: no redraw event from the window system for {ms} ms: the loop draws the frames (surface configured again)"
        ),
        Note::Resumed(ms) => {
            format!("{who}: redraw events back after {ms} ms of frames drawn by the loop")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The event sequence of the 2026-11-04 log (freeze_1319): frames to ~109.9 s, then no redraw event while the
    /// window loses the focus (111.1 s), gets it back (112.1 s), loses it (116.5 s), gets it back (130.6 s).
    #[test]
    fn the_alt_tab_freeze_of_the_log_draws_again_from_the_loop() {
        let mut k = RedrawKeeper::default();
        k.arm(100);
        // ~800 fps chain
        let mut t = 100;
        while t < 109_900 {
            assert_eq!(k.on_redraw_event(t), None);
            assert_eq!(k.poll(t, true).0, Wake::Idle);
            t += 1;
        }
        // the chain ends; the task switcher has the screen (the window minimised: not visible)
        for t in (109_901..111_101).step_by(7) {
            assert_eq!(k.poll(t, false), (Wake::Idle, None));
        }
        assert!(!k.drawing_from_loop());
        // focus back at 112.133 s, the window restored: the loop draws, configuring the surface first
        k.on_shown();
        let (w, n) = k.poll(112_133, true);
        assert_eq!(w, Wake::Draw);
        assert_eq!(n, Some(Note::Stalled(112_133 - 109_899)));
        assert!(k.take_reconfigure());
        k.on_loop_frame(112_134);
        // every later turn draws too (no redraw event comes), without another note or configure
        for t in 112_135..112_200 {
            assert_eq!(k.poll(t, true), (Wake::Draw, None));
            assert!(!k.take_reconfigure());
            k.on_loop_frame(t);
        }
        // focus lost and the window minimised: nothing drawn, still in the loop's mode
        assert_eq!(k.poll(116_487, false), (Wake::Idle, None));
        assert!(k.drawing_from_loop());
        // back at 130.570 s: drawing at once, the surface configured again after the show
        k.on_shown();
        assert_eq!(k.poll(130_570, true), (Wake::Draw, None));
        assert!(k.take_reconfigure());
        k.on_loop_frame(130_571);
        // winit's redraw events come again: the chain takes over
        assert_eq!(
            k.on_redraw_event(130_572),
            Some(Note::Resumed(130_572 - 112_133))
        );
        assert_eq!(k.poll(130_573, true), (Wake::Idle, None));
        assert!(!k.drawing_from_loop());
    }

    #[test]
    fn a_running_chain_or_a_slow_frame_never_draws_from_the_loop() {
        let mut k = RedrawKeeper::default();
        k.arm(0);
        // 30 fps cap
        for i in 1..100u64 {
            assert!(!k.due(i * 33));
            k.on_redraw_event(i * 33);
        }
        // a 200 ms hitch is inside the frame: measured from its end, the next turn is not due
        k.on_redraw_event(3_500);
        assert_eq!(k.poll(3_501, true), (Wake::Idle, None));
        assert_eq!(k.poll(3_500 + STALL_MS - 1, true), (Wake::Idle, None));
        assert_eq!(k.poll(3_500 + STALL_MS, true).0, Wake::Draw);
    }

    #[test]
    fn nothing_before_the_first_frame() {
        let mut k = RedrawKeeper::default();
        assert!(!k.due(1_000_000));
        assert_eq!(k.poll(1_000_000, true), (Wake::Idle, None));
    }

    #[test]
    fn arming_again_after_a_device_rebuild_leaves_the_loop_mode() {
        let mut k = RedrawKeeper::default();
        k.arm(0);
        assert_eq!(k.poll(STALL_MS, true).0, Wake::Draw);
        assert!(k.drawing_from_loop());
        k.arm(5_000);
        assert!(!k.drawing_from_loop());
        assert_eq!(k.poll(5_001, true), (Wake::Idle, None));
    }

    #[test]
    fn a_focus_or_show_change_configures_once() {
        let mut k = RedrawKeeper::default();
        assert!(!k.take_reconfigure());
        k.on_shown();
        k.on_shown();
        assert!(k.take_reconfigure());
        assert!(!k.take_reconfigure());
    }

    #[test]
    fn only_the_first_acquire_failure_of_a_run_is_logged() {
        let mut k = RedrawKeeper::default();
        assert!(k.on_acquire(false));
        assert!(!k.on_acquire(false));
        assert!(!k.on_acquire(true));
        assert!(k.on_acquire(false));
    }
}
