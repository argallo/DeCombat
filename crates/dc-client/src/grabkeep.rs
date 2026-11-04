//! Keeping the match window's cursor grab after the window comes back (alt-tab, restore from the taskbar).
//!
//! The grab is a cursor clip the operating system holds for the window. On Windows the clip is taken when the
//! window reports its focus, which on a return through alt-tab can come while the borderless window is still being
//! restored (its client rectangle still the minimised one) or before the task switcher lets go; the clip set then is
//! stale or dropped again, and nothing takes it later because the window still counts the cursor as captured. The
//! playtest report: "alt-tab back into the game sometimes doesn't take the cursor back until Alt+Enter twice" (the
//! look change releases and takes the grab again at the new size).
//!
//! The keeper says when the window asserts its grab again: shortly after every capture, after a size or show change,
//! and then at a slow pace while the window is focused and captured. Asserting an unchanged grab costs nothing on
//! Windows (winit compares the clip it wants with the one in force and only then calls the system).
// DC-DEFAULT: CLI-1721 (proposed, project decision)
// PUBLIC: Win32 ClipCursor documentation (the cursor clip is a shared, system-wide resource); winit 0.30
// `set_cursor_grab` on Windows (clip refreshed only when the window's cursor flags change, and only while it is the
// active window)

/// The first re-assertion after a capture or a size / show change (ms).
pub const SETTLE_MS: u64 = 150;
/// The pace of the re-assertion while captured and focused (ms).
pub const PERIOD_MS: u64 = 500;
/// How long after the focus came back a refused capture is tried again (ms).
pub const RETRY_WINDOW_MS: u64 = 3000;

/// When the window's grab is asserted again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GrabKeeper {
    next_ms: Option<u64>,
    /// The focus came back: until this time a refused capture is tried again, with the time of the next try.
    retry: Option<(u64, u64)>,
}

impl GrabKeeper {
    /// The grab was taken at `now_ms`.
    pub fn on_capture(&mut self, now_ms: u64) {
        self.next_ms = Some(now_ms.saturating_add(SETTLE_MS));
        self.retry = None;
    }

    /// The window got the focus back at `now_ms` and wants the cursor: a capture refused now (the window still being
    /// restored) is tried again for [`RETRY_WINDOW_MS`].
    pub fn on_focus(&mut self, now_ms: u64) {
        self.retry = Some((
            now_ms.saturating_add(RETRY_WINDOW_MS),
            now_ms.saturating_add(SETTLE_MS),
        ));
    }

    /// Whether the capture is tried again now: inside the retry window after [`GrabKeeper::on_focus`], while the
    /// window is `focused` and not `captured`, every [`SETTLE_MS`].
    pub fn retry_capture(&mut self, now_ms: u64, captured: bool, focused: bool) -> bool {
        let Some((until, next)) = self.retry else {
            return false;
        };
        if captured || !focused || now_ms > until {
            self.retry = None;
            return false;
        }
        if now_ms < next {
            return false;
        }
        self.retry = Some((until, now_ms.saturating_add(SETTLE_MS)));
        true
    }

    /// The grab was let go: nothing to keep (and no capture retried).
    pub fn on_release(&mut self) {
        self.next_ms = None;
        self.retry = None;
    }

    /// The window's size, place or visibility changed at `now_ms`: assert the grab again soon (at the latest
    /// [`SETTLE_MS`] from now).
    pub fn on_change(&mut self, now_ms: u64) {
        let soon = now_ms.saturating_add(SETTLE_MS);
        self.next_ms = Some(self.next_ms.map_or(soon, |t| t.min(soon)));
    }

    /// Whether the grab is asserted again now (`true` at most once per due time). Only while the window is
    /// `focused` and has the cursor `captured`; otherwise the keeper waits for the next capture.
    pub fn due(&mut self, now_ms: u64, captured: bool, focused: bool) -> bool {
        if !(captured && focused) {
            return false;
        }
        match self.next_ms {
            Some(t) if now_ms >= t => {
                self.next_ms = Some(now_ms.saturating_add(PERIOD_MS));
                true
            }
            Some(_) => false,
            // captured without a capture seen (the window's own state): start the pace
            None => {
                self.next_ms = Some(now_ms.saturating_add(SETTLE_MS));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The alt-tab return: focus comes back and the capture is taken; the grab is asserted again after the settle
    /// time and then every half second, never while released or unfocused.
    #[test]
    fn the_grab_is_asserted_after_a_capture_and_then_at_a_pace() {
        let mut k = GrabKeeper::default();
        assert!(!k.due(0, false, true), "nothing before a capture");
        k.on_capture(1000);
        assert!(!k.due(1000, true, true));
        assert!(!k.due(1149, true, true));
        assert!(k.due(1150, true, true), "after the settle time");
        assert!(!k.due(1151, true, true), "once per due time");
        assert!(!k.due(1649, true, true));
        assert!(k.due(1650, true, true), "then at the pace");
        // focus lost: nothing, the release clears the keeper
        assert!(!k.due(5000, true, false));
        k.on_release();
        assert!(!k.due(6000, false, true));
        // back: a new capture starts again
        k.on_capture(7000);
        assert!(k.due(7150, true, true));
    }

    /// A size or show change (the borderless window restored from the taskbar) brings the next assertion forward.
    #[test]
    fn a_window_change_brings_the_assertion_forward() {
        let mut k = GrabKeeper::default();
        k.on_capture(0);
        assert!(k.due(150, true, true));
        // next due at 650; a restore at 200 moves it to 350
        k.on_change(200);
        assert!(!k.due(349, true, true));
        assert!(k.due(350, true, true));
        // a change never pushes a nearer time back
        k.on_change(400);
        k.on_change(500);
        assert!(k.due(550, true, true));
    }

    /// A capture refused at the focus return (the window still minimised) is tried again for a while; a capture,
    /// a release or the focus going away ends the retries.
    #[test]
    fn a_refused_capture_is_tried_again_after_the_focus_returns() {
        let mut k = GrabKeeper::default();
        assert!(!k.retry_capture(0, false, true), "no focus return seen");
        k.on_focus(1000);
        assert!(!k.retry_capture(1100, false, true));
        assert!(k.retry_capture(1150, false, true));
        assert!(!k.retry_capture(1200, false, true));
        assert!(k.retry_capture(1300, false, true));
        k.on_capture(1310);
        assert!(!k.retry_capture(1500, false, true), "taken: no more retries");
        k.on_focus(5000);
        assert!(!k.retry_capture(5200, false, false), "focus gone again");
        assert!(!k.retry_capture(5400, false, true));
        k.on_focus(9000);
        assert!(!k.retry_capture(12001, false, true), "the window closed");
    }

    /// A window that counts the cursor as captured without the keeper having seen the capture starts the pace.
    #[test]
    fn an_unseen_capture_starts_the_pace() {
        let mut k = GrabKeeper::default();
        assert!(!k.due(10, true, true));
        assert!(k.due(160, true, true));
    }
}
