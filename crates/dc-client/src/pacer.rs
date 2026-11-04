//! Presentation frame pacer (CLI-890, user rule 2026-10-28): the window presents at most `cap` frames per second.
//! A sleep-then-spin wait to the next frame deadline in the redraw loop; the simulation tick and the net threads are
//! not touched (frame-rate independence stands, this caps presentation only).
//!
//! Defaults (DC_DEFAULTS.md CLI-890): 1000 fps in a match, `min(360, monitor refresh)` in the front end / menus
//! (360 when winit does not know the refresh rate). `--fps-cap-match N` / `--fps-cap-menu N` override, 0 = uncapped.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Default match cap, frames per second.
pub const MATCH_CAP: u32 = 1000;
/// Upper bound of the menu cap, frames per second.
pub const MENU_CAP_MAX: u32 = 360;
const UNSET: u32 = u32::MAX;

static MATCH_FLAG: AtomicU32 = AtomicU32::new(UNSET);
static MENU_FLAG: AtomicU32 = AtomicU32::new(UNSET);
/// CLI-1595: the Graphic tab's frame-rate setting (0 = unlimited, i.e. the CLI-890 defaults); a flag wins over it.
static SETTING_CAP: AtomicU32 = AtomicU32::new(0);

/// The options page's frame-rate setting (`settings::publish`): 0 = unlimited.
pub fn set_setting_cap(cap: u32) {
    SETTING_CAP.store(cap, Ordering::Relaxed);
}

/// The frame-rate setting in force (0 = unlimited, `settings::FPS_VSYNC` = the display's rate).
pub fn setting_cap() -> u32 {
    SETTING_CAP.load(Ordering::Relaxed)
}

/// The cap the setting asks the pacer for: the vertical-sync choice paces nothing (the blank does).
fn setting_pace() -> u32 {
    match setting_cap() {
        crate::settings::FPS_VSYNC => 0,
        v => v,
    }
}

/// `--fps-cap-match N`.
pub fn set_match_flag(cap: u32) {
    MATCH_FLAG.store(cap.min(UNSET - 1), Ordering::Relaxed);
}

/// `--fps-cap-menu N`.
pub fn set_menu_flag(cap: u32) {
    MENU_FLAG.store(cap.min(UNSET - 1), Ordering::Relaxed);
}

/// The match cap in force (0 = uncapped): the `--fps-cap-match` flag, else the frame-rate setting (CLI-1595),
/// else the CLI-890 default.
pub fn match_cap() -> u32 {
    match MATCH_FLAG.load(Ordering::Relaxed) {
        UNSET => match setting_pace() {
            0 => MATCH_CAP,
            v => v,
        },
        v => v,
    }
}

/// The menu cap in force for a monitor refresh rate in millihertz (0 = uncapped): the `--fps-cap-menu` flag,
/// else the frame-rate setting (CLI-1595) bounded by the CLI-890 menu default, else that default.
pub fn menu_cap(refresh_mhz: Option<u32>) -> u32 {
    match MENU_FLAG.load(Ordering::Relaxed) {
        UNSET => {
            let d = default_menu_cap(refresh_mhz);
            match setting_pace() {
                0 => d,
                v => v.min(d),
            }
        }
        v => v,
    }
}

/// `min(360, refresh)`; 360 when the refresh rate is unknown or zero.
pub fn default_menu_cap(refresh_mhz: Option<u32>) -> u32 {
    match refresh_mhz {
        Some(m) if m >= 1000 => ((m + 500) / 1000).min(MENU_CAP_MAX),
        _ => MENU_CAP_MAX,
    }
}

/// The frame interval of a cap (`None` = uncapped).
pub fn interval(cap: u32) -> Option<Duration> {
    (cap > 0).then(|| Duration::from_nanos(1_000_000_000 / cap as u64))
}

/// The deadline after `prev`: one interval later, but never in the past (a late frame is not made up with a burst).
pub fn next_deadline(prev: Instant, interval: Duration, now: Instant) -> Instant {
    (prev + interval).max(now)
}

/// The pacer of one window.
#[derive(Debug, Default)]
pub struct Pacer {
    next: Option<Instant>,
}

impl Pacer {
    /// Waits until the frame deadline for `cap` (0 = uncapped: returns at once), then sets the next one. Sleeps
    /// to ~1.5 ms before the deadline, spins the rest.
    pub fn wait(&mut self, cap: u32) {
        let Some(iv) = interval(cap) else {
            self.next = None;
            return;
        };
        let now = Instant::now();
        let target = match self.next {
            Some(t) => t,
            None => now,
        };
        let spin = Duration::from_micros(1500);
        if target > now + spin {
            std::thread::sleep(target - now - spin);
        }
        while Instant::now() < target {
            std::hint::spin_loop();
        }
        self.next = Some(next_deadline(target, iv, Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_default_is_min_of_360_and_refresh() {
        assert_eq!(default_menu_cap(Some(60_000)), 60);
        assert_eq!(default_menu_cap(Some(59_940)), 60);
        assert_eq!(default_menu_cap(Some(144_000)), 144);
        assert_eq!(default_menu_cap(Some(480_000)), 360);
        assert_eq!(default_menu_cap(None), 360);
        assert_eq!(default_menu_cap(Some(0)), 360);
    }

    /// CLI-1595: the setting caps both windows under the flags; unlimited is the CLI-890 default.
    #[test]
    fn the_frame_rate_setting_sits_under_the_flags() {
        set_setting_cap(0);
        assert_eq!(match_cap(), MATCH_CAP);
        assert_eq!(menu_cap(Some(120_000)), 120);
        set_setting_cap(60);
        assert_eq!(match_cap(), 60);
        assert_eq!(menu_cap(Some(120_000)), 60);
        set_setting_cap(240);
        assert_eq!(match_cap(), 240);
        assert_eq!(menu_cap(Some(120_000)), 120, "the menu never goes above its own default");
        set_setting_cap(crate::settings::FPS_VSYNC);
        assert_eq!(match_cap(), MATCH_CAP, "vertical sync paces nothing here");
        assert!(crate::settings::fps_vsync());
        set_setting_cap(0);
    }

    #[test]
    fn interval_and_deadline_arithmetic() {
        assert_eq!(interval(0), None);
        assert_eq!(interval(1000), Some(Duration::from_millis(1)));
        let iv = Duration::from_millis(1);
        let t0 = Instant::now();
        assert_eq!(next_deadline(t0, iv, t0), t0 + iv);
        // late by 10 ms: the next deadline is now, not a burst of catch-up frames
        let late = t0 + Duration::from_millis(11);
        assert_eq!(next_deadline(t0, iv, late), late);
    }

    #[test]
    fn cap_holds_over_a_loop() {
        let mut p = Pacer::default();
        let t = Instant::now();
        for _ in 0..201 {
            p.wait(500);
        }
        let el = t.elapsed();
        // 200 intervals of 2 ms: never faster than the cap
        assert!(el >= Duration::from_millis(399), "{el:?}");
        assert!(el < Duration::from_millis(700), "{el:?}");
        let t = Instant::now();
        for _ in 0..1000 {
            p.wait(0);
        }
        assert!(t.elapsed() < Duration::from_millis(50));
    }
}
