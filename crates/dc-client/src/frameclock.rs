//! The frame clock (CLI-1576): the time a frame is simulated and drawn for.
//!
//! With presentation waiting for the vertical blank (Fifo) the frames show at the display's cadence, one per
//! blank, but the CPU starts them unevenly: the swap chain blocks a frame until a drawable is free and then lets
//! the next one start at once (raw frame times of 0.7 ms and 16 ms around a 8.3 ms period were measured on a
//! 120 Hz panel). A body simulated and interpolated at those CPU times and shown at the even blanks moves by
//! uneven steps: the own body jitters, a remote runner looks doubled. The frame clock predicts the presentation
//! time instead: one display period after the previous frame's, bled toward the wall clock by [`BLEED`] of the
//! error each frame so it never drifts, and snapped to the wall clock after a hitch of more than
//! [`SNAP_PERIODS`] periods. Without blank-synced presentation the wall clock is the frame clock, as before.
// DC-DEFAULT: CLI-1576 (proposed) the render time of a blank-synced frame is its predicted presentation time

use std::time::{Duration, Instant};

/// The share of the wall-clock error taken back each frame.
pub const BLEED: f64 = 0.1;
/// An error of more than this many periods (a hitch, a pause) snaps the clock to the wall clock.
pub const SNAP_PERIODS: f64 = 1.5;

/// The clock of one window.
#[derive(Debug, Default)]
pub struct FrameClock {
    t: Option<Instant>,
    /// Raw CPU frames shorter than half a period / longer than 1.5 periods (the unevenness the clock hides).
    pub short: u64,
    pub long: u64,
}

impl FrameClock {
    /// The render time of the frame that starts at `now` on the wall clock. `period` is the display period when
    /// presentation waits for the blank, else `None` (the frame clock is the wall clock).
    pub fn next(&mut self, now: Instant, period: Option<Duration>) -> Instant {
        let Some(p) = period else {
            self.t = Some(now);
            return now;
        };
        let t = match self.t {
            None => now,
            Some(prev) => {
                let t = prev + p;
                let err = signed_secs(now, t);
                if err.abs() > SNAP_PERIODS * p.as_secs_f64() {
                    now
                } else {
                    shift(t, err * BLEED)
                }
            }
        };
        self.t = Some(t);
        t
    }

    /// Counts a raw CPU frame time against the period.
    pub fn note_raw(&mut self, dt: Duration, period: Option<Duration>) {
        if let Some(p) = period {
            let (d, p) = (dt.as_secs_f64(), p.as_secs_f64());
            if d < 0.5 * p {
                self.short += 1;
            } else if d > 1.5 * p {
                self.long += 1;
            }
        }
    }
}

/// `a - b` in seconds, negative when `a` is before `b`.
pub fn signed_secs(a: Instant, b: Instant) -> f64 {
    if a >= b {
        (a - b).as_secs_f64()
    } else {
        -(b - a).as_secs_f64()
    }
}

fn shift(t: Instant, secs: f64) -> Instant {
    if secs >= 0.0 {
        t + Duration::from_secs_f64(secs)
    } else {
        t - Duration::from_secs_f64(-secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Duration = Duration::from_micros(8333);

    /// CPU frames alternating 0.7 / 16 ms around an 8.3 ms period come out as even 8.3 ms render steps.
    #[test]
    fn uneven_cpu_frames_become_even_render_steps() {
        let mut c = FrameClock::default();
        let t0 = Instant::now();
        let mut now = t0;
        let mut prev = c.next(now, Some(P));
        let mut steps = Vec::new();
        for i in 0..200 {
            now += if i % 2 == 0 {
                Duration::from_micros(700)
            } else {
                Duration::from_micros(15966)
            };
            let r = c.next(now, Some(P));
            steps.push(signed_secs(r, prev) * 1000.0);
            prev = r;
        }
        let late: Vec<f64> = steps[20..].to_vec();
        let (lo, hi) = late.iter().fold((f64::MAX, f64::MIN), |(l, h), &s| (l.min(s), h.max(s)));
        assert!(lo > 7.0 && hi < 9.7, "render steps {lo:.2}..{hi:.2} ms");
        // and the clock stays locked to the wall clock (no drift)
        assert!(signed_secs(prev, now).abs() < 0.010, "{}", signed_secs(prev, now));
        assert_eq!((c.short, c.long), (0, 0), "note_raw was not called");
    }

    /// A pause (a world load, a hitch) snaps the clock to the wall clock instead of catching up for seconds.
    #[test]
    fn a_long_pause_snaps_to_the_wall_clock() {
        let mut c = FrameClock::default();
        let t0 = Instant::now();
        c.next(t0, Some(P));
        let now = t0 + Duration::from_millis(200);
        assert_eq!(c.next(now, Some(P)), now);
        // one dropped blank (an error of one period) is bled back, not snapped
        let now2 = now + 2 * P;
        let r = c.next(now2, Some(P));
        let err = signed_secs(now2, r) * 1000.0;
        assert!(err > 7.0 && err < 7.6, "{err}");
    }

    /// Without blank-synced presentation the frame clock is the wall clock.
    #[test]
    fn without_a_period_the_wall_clock_stands() {
        let mut c = FrameClock::default();
        let t0 = Instant::now();
        c.next(t0, None);
        let now = t0 + Duration::from_micros(700);
        assert_eq!(c.next(now, None), now);
        c.note_raw(Duration::from_micros(700), Some(P));
        c.note_raw(Duration::from_micros(16000), Some(P));
        c.note_raw(Duration::from_micros(8300), Some(P));
        assert_eq!((c.short, c.long), (1, 1));
    }
}
