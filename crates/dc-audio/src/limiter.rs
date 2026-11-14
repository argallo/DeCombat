//! A peak limiter on the final stereo mix, so a sum of many loud voices is turned down for a moment instead of being
//! cut flat at full scale.
//!
//! Design (ours; ordinary signal processing, no 2009 evidence: the 2009 mixer's own output stage is internal to its
//! third-party driver, SPEC AUDIO_2009.md §12 item 5 says only that the output is clipped):
//! - feed-forward, stereo-linked: one gain for both channels, from the larger of the two samples of each frame, so a
//!   limited mix keeps its stereo image;
//! - zero latency, hard knee, instant attack: when a frame would exceed the ceiling the gain drops at once to exactly
//!   `ceiling / peak`; no look-ahead, so nothing is delayed and a mix that stays at or below the ceiling is the same
//!   sample for sample as without the limiter (the gain is exactly 1.0 there; a soft knee or a look-ahead delay would
//!   both change a mix that never came near full scale);
//! - hold, then release: the gain stays down for [`HOLD_S`] after the last frame that needed it, then returns to 1.0
//!   along an exponential with time constant [`RELEASE_S`]; it snaps to exactly 1.0 when within [`UNITY_SNAP`], so
//!   the pass-through is exact again after a limited passage and no tiny value lingers (no denormals: the state is the
//!   gain, never below the ceiling over the largest finite sample);
//! - fixed state, no allocation, no lock, no logging; the coefficients depend on the output rate only.
//!
//! Telemetry for the log line and for a live meter lives in [`LimiterMeter`] (atomics, read from any thread).
// DC-DEFAULT: CLI-1960 (proposed) output limiter, see DC_DEFAULTS.md

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Default ceiling in dBFS: one decibel of headroom below full scale.
// DC-DEFAULT: CLI-1960 (proposed)
pub const DEFAULT_CEILING_DB: f32 = -1.0;
/// The ceiling can be set within this range (dBFS).
pub const CEILING_RANGE_DB: (f32, f32) = (-6.0, 0.0);
/// Whether the limiter runs when nothing set it (off: the mix is exactly as before the limiter existed).
// DC-DEFAULT: CLI-1960 (proposed) off until the owner decides; see DC_DEFAULTS.md
pub const DEFAULT_ENABLED: bool = false;
/// Time the gain stays down after the last frame above the ceiling.
// DC-DEFAULT: CLI-1960 (proposed) 10 ms
pub const HOLD_S: f32 = 0.010;
/// Release time constant: the gain recovers 63 % of its reduction in this time. Longer than the gap between two
/// shots of an automatic weapon (60..100 ms at 600..1000 rounds a minute), so a burst gets one steady reduction
/// instead of a dip per shot (pumping).
// DC-DEFAULT: CLI-1960 (proposed) 200 ms
pub const RELEASE_S: f32 = 0.200;
/// A gain within this of 1.0 is set to exactly 1.0.
pub const UNITY_SNAP: f32 = 1.0e-5;
/// The largest output the limiter writes: just under full scale, so a limited sample never reads as a clipped one.
pub const FULL_SCALE_GUARD: f32 = 1.0 - 1.0 / 32768.0;

/// Decibels of a linear gain (`-inf` guarded to -200).
pub fn db_of(gain: f32) -> f32 {
    if gain > 1.0e-10 {
        20.0 * gain.log10()
    } else {
        -200.0
    }
}
/// Linear gain of a level in decibels.
pub fn gain_of_db(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Live limiter figures, shared between the audio callback (writer) and the game or a panel (readers).
#[derive(Debug, Default)]
pub struct LimiterMeter {
    /// The largest gain reduction of the last rendered buffer, in dB (>= 0), as f32 bits.
    last_reduction_db: AtomicU32,
    /// The largest gain reduction since the last [`LimiterMeter::take_window`], dB (>= 0), as f32 bits.
    window_max_db: AtomicU32,
    /// Frames rendered with a gain below 1.0, in total.
    limited_frames: AtomicU64,
    /// Samples the final clamp changed (beyond full scale after the limiter, or with the limiter off), in total.
    clipped_samples: AtomicU64,
    /// Whether the limiter ran in the last buffer.
    active: AtomicBool,
}

impl LimiterMeter {
    /// Gain reduction of the last rendered buffer in dB (0.0 when nothing was limited).
    pub fn reduction_db(&self) -> f32 {
        f32::from_bits(self.last_reduction_db.load(Ordering::Relaxed))
    }
    /// Frames limited since the mixer was made.
    pub fn limited_frames(&self) -> u64 {
        self.limited_frames.load(Ordering::Relaxed)
    }
    /// Samples hard-clipped since the mixer was made.
    pub fn clipped_samples(&self) -> u64 {
        self.clipped_samples.load(Ordering::Relaxed)
    }
    /// Whether the limiter is switched on (as of the last buffer).
    pub fn enabled(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }
    /// The largest reduction (dB) since the previous call, and resets it (a status line's window).
    pub fn take_window(&self) -> f32 {
        f32::from_bits(self.window_max_db.swap(0f32.to_bits(), Ordering::Relaxed))
    }
    /// The largest reduction (dB) since the previous [`LimiterMeter::take_window`], without resetting it.
    pub fn window_max_db(&self) -> f32 {
        f32::from_bits(self.window_max_db.load(Ordering::Relaxed))
    }

    fn publish(&self, enabled: bool, reduction_db: f32, limited: u64, clipped: u64) {
        self.active.store(enabled, Ordering::Relaxed);
        self.last_reduction_db
            .store(reduction_db.to_bits(), Ordering::Relaxed);
        if limited > 0 {
            self.limited_frames.fetch_add(limited, Ordering::Relaxed);
        }
        if clipped > 0 {
            self.clipped_samples.fetch_add(clipped, Ordering::Relaxed);
        }
        // a lost race only drops one buffer's maximum from a status window
        let w = f32::from_bits(self.window_max_db.load(Ordering::Relaxed));
        if reduction_db > w {
            self.window_max_db
                .store(reduction_db.to_bits(), Ordering::Relaxed);
        }
    }
}

/// The limiter's state. See the module documentation.
#[derive(Debug)]
pub struct Limiter {
    enabled: bool,
    ceiling_db: f32,
    /// Linear ceiling: the threshold (frames at or below it pass unchanged while the gain is 1.0).
    ceiling: f32,
    gain: f32,
    /// `1 - gain`, the state the release decays.
    reduction: f32,
    hold_left: u32,
    hold_frames: u32,
    release_coef: f32,
    rate: u32,
    meter: Arc<LimiterMeter>,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new(crate::OUTPUT_RATE)
    }
}

impl Limiter {
    pub fn new(rate: u32) -> Self {
        let mut l = Limiter {
            enabled: DEFAULT_ENABLED,
            ceiling_db: DEFAULT_CEILING_DB,
            ceiling: gain_of_db(DEFAULT_CEILING_DB),
            gain: 1.0,
            reduction: 0.0,
            hold_left: 0,
            hold_frames: 0,
            release_coef: 0.0,
            rate: 0,
            meter: Arc::new(LimiterMeter::default()),
        };
        l.set_rate(rate);
        l
    }

    /// Recomputes the time constants for an output rate (Hz).
    pub fn set_rate(&mut self, rate: u32) {
        let rate = rate.max(1);
        if rate == self.rate {
            return;
        }
        self.rate = rate;
        self.hold_frames = (HOLD_S * rate as f32).round() as u32;
        self.release_coef = (-1.0 / (RELEASE_S * rate as f32)).exp();
    }

    /// Switches the limiter. Switching it off while it holds the gain down does not jump back to unity (a click):
    /// the gain finishes its hold and release, no new peak is limited, and once it is back at exactly 1.0 the
    /// limiter is bypassed.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Sets the ceiling in dBFS, clamped to [`CEILING_RANGE_DB`] (a NaN keeps the current one).
    pub fn set_ceiling_db(&mut self, db: f32) {
        if db.is_nan() {
            return;
        }
        self.ceiling_db = db.clamp(CEILING_RANGE_DB.0, CEILING_RANGE_DB.1);
        self.ceiling = gain_of_db(self.ceiling_db).min(1.0);
    }
    pub fn ceiling_db(&self) -> f32 {
        self.ceiling_db
    }

    /// The gain applied to the last frame (1.0: pass-through).
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// The shared meter.
    pub fn meter(&self) -> Arc<LimiterMeter> {
        self.meter.clone()
    }

    /// Limits the interleaved stereo block in place, then clamps it to [-1, 1] as the mixer always did, and
    /// publishes the buffer's figures to the meter.
    pub fn process(&mut self, lr: &mut [f32]) {
        let mut min_gain = 1.0f32;
        let mut limited = 0u64;
        // off and at unity: not one sample is touched (the clamp below is the mixer's as before)
        if self.enabled || self.gain < 1.0 {
            let target = self.ceiling.min(FULL_SCALE_GUARD);
            for fr in lr.as_chunks_mut::<2>().0 {
                if !(fr[0].is_finite() && fr[1].is_finite()) {
                    // a non-finite sample is not limitable: the state stays as it is, the frame gets the current
                    // gain (an infinity stays infinite and the clamp makes it full scale, a NaN stays a NaN, both
                    // as with the limiter off)
                    if self.gain < 1.0 {
                        fr[0] *= self.gain;
                        fr[1] *= self.gain;
                    }
                    continue;
                }
                let peak = fr[0].abs().max(fr[1].abs());
                if self.enabled && peak > self.ceiling {
                    // `peak` is finite and above the ceiling here, so `need` is finite and above 0
                    let need = target / peak;
                    if need < self.gain {
                        self.gain = need;
                        self.reduction = 1.0 - need;
                    }
                    self.hold_left = self.hold_frames;
                } else if self.gain < 1.0 {
                    if self.hold_left > 0 {
                        self.hold_left -= 1;
                    } else {
                        // the reduction `1 - gain` decays; kept as the state it would stall in f32 near 1.0
                        self.reduction *= self.release_coef;
                        if self.reduction < UNITY_SNAP {
                            self.reduction = 0.0;
                        }
                        self.gain = 1.0 - self.reduction;
                    }
                }
                if self.gain < 1.0 {
                    fr[0] *= self.gain;
                    fr[1] *= self.gain;
                    limited += 1;
                    min_gain = min_gain.min(self.gain);
                    // a gain that would let a later frame's peak through cannot happen: `need` follows the peak
                    // down at once; a frame below the ceiling times a gain below 1 stays below it
                }
            }
        }
        let mut clipped = 0u64;
        for s in lr.iter_mut() {
            if s.abs() > 1.0 {
                clipped += 1;
            }
            *s = s.clamp(-1.0, 1.0);
        }
        let red = if min_gain < 1.0 {
            -db_of(min_gain)
        } else {
            0.0
        };
        self.meter.publish(self.enabled, red, limited, clipped);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic noise-like test signal (xorshift).
    fn noise(frames: usize, peak: f32, seed: u32) -> Vec<f32> {
        let mut x = seed.max(1);
        (0..2 * frames)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                ((x as f32 / u32::MAX as f32) * 2.0 - 1.0) * peak
            })
            .collect()
    }

    #[test]
    fn below_the_ceiling_is_bit_exact() {
        for ceiling in [-6.0, -1.0, 0.0] {
            let mut l = Limiter::new(48_000);
            l.set_enabled(true);
            l.set_ceiling_db(ceiling);
            let c = gain_of_db(ceiling);
            let input = noise(48_000, c, 7);
            let mut out = input.clone();
            for chunk in out.chunks_mut(512) {
                l.process(chunk);
            }
            assert!(
                out.iter()
                    .zip(&input)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                "{ceiling} dB"
            );
            assert_eq!(l.meter().limited_frames(), 0);
            assert_eq!(l.meter().reduction_db(), 0.0);
        }
    }

    #[test]
    fn a_loud_burst_never_exceeds_the_ceiling_and_recovers_to_exact_unity() {
        let rate = 44_100;
        let mut l = Limiter::new(rate);
        l.set_enabled(true);
        assert_eq!(l.ceiling_db(), DEFAULT_CEILING_DB);
        let c = gain_of_db(DEFAULT_CEILING_DB);
        // 50 ms at peak 3.0, then quiet signal
        let mut burst = noise(rate as usize / 20, 3.0, 3);
        l.process(&mut burst);
        assert!(burst.iter().all(|s| s.abs() <= c + 1e-6));
        assert!(l.meter().limited_frames() > 0);
        let red = l.meter().reduction_db();
        assert!(red > 9.0 && red < 10.6, "{red}");
        assert_eq!(l.meter().clipped_samples(), 0);
        // the quiet tail: limited during hold and release, back to exact pass-through within ~3 s
        let quiet = noise(rate as usize * 4, 0.1, 9);
        let mut tail = quiet.clone();
        for chunk in tail.chunks_mut(441) {
            l.process(chunk);
        }
        assert_eq!(l.gain(), 1.0);
        let n = tail.len();
        assert_eq!(&tail[n - 1000..], &quiet[n - 1000..]);
        // it held for 10 ms: the first frames after the burst still at the burst's gain
        assert!((tail[0] / quiet[0] - tail[2 * 400] / quiet[2 * 400]).abs() < 1e-4);
    }

    #[test]
    fn stereo_linked_one_gain_for_both_channels() {
        let mut l = Limiter::new(44_100);
        l.set_enabled(true);
        l.set_ceiling_db(0.0);
        let mut f = [2.0, 0.5];
        l.process(&mut f);
        assert!((f[0] - FULL_SCALE_GUARD).abs() < 1e-6);
        assert!((f[1] - FULL_SCALE_GUARD / 4.0).abs() < 1e-6);
    }

    #[test]
    fn off_clips_and_counts_as_before() {
        let mut l = Limiter::new(44_100);
        assert!(!l.enabled(), "off by default");
        let mut f = [1.5, -0.5, -2.0, 1.0];
        l.process(&mut f);
        assert_eq!(f, [1.0, -0.5, -1.0, 1.0]);
        assert_eq!(l.meter().clipped_samples(), 2);
        assert_eq!(l.meter().limited_frames(), 0);
    }

    #[test]
    fn the_ceiling_clamps_to_its_range_and_works_at_any_rate() {
        let mut l = Limiter::new(44_100);
        l.set_ceiling_db(-20.0);
        assert_eq!(l.ceiling_db(), -6.0);
        l.set_ceiling_db(3.0);
        assert_eq!(l.ceiling_db(), 0.0);
        l.set_ceiling_db(f32::NAN);
        assert_eq!(l.ceiling_db(), 0.0);
        for rate in [8_000, 44_100, 48_000, 192_000] {
            let mut l = Limiter::new(rate);
            l.set_enabled(true);
            let mut b = noise(rate as usize / 10, 4.0, 5);
            l.process(&mut b);
            assert!(b.iter().all(|s| s.abs() < 1.0), "{rate}");
            // a NaN frame does not poison the gain
            let mut n = [f32::NAN, 0.5];
            l.process(&mut n);
            assert!(l.gain().is_finite() && l.gain() > 0.0);
        }
    }

    #[test]
    fn non_finite_samples_pass_as_with_the_limiter_off_and_leave_the_state() {
        let inf = f32::INFINITY;
        let cases: [[f32; 2]; 6] = [
            [inf, 0.5],
            [0.5, -inf],
            [inf, -inf],
            [f32::NAN, 0.5],
            [0.5, f32::NAN],
            [f32::NAN, f32::NAN],
        ];
        for limited_before in [false, true] {
            for c in cases {
                let mut l = Limiter::new(44_100);
                l.set_enabled(true);
                if limited_before {
                    let mut loud = [2.0, 2.0];
                    l.process(&mut loud);
                }
                let (g, r, h) = (l.gain(), l.reduction, l.hold_left);
                let mut on = c;
                l.process(&mut on);
                assert_eq!((l.gain(), l.reduction, l.hold_left), (g, r, h), "{c:?}");
                let mut off = c;
                Limiter::new(44_100).process(&mut off);
                for k in 0..2 {
                    assert_eq!(on[k].is_nan(), off[k].is_nan(), "{c:?}");
                    if off[k].is_finite() && off[k].abs() == 1.0 {
                        assert_eq!(on[k], off[k], "{c:?}: an infinity is full scale either way");
                    }
                }
                if !limited_before {
                    assert!(
                        on.iter().zip(&off).all(|(a, b)| a.to_bits() == b.to_bits()),
                        "{c:?}"
                    );
                }
                // the next ordinary frame is limited or passed as before
                let mut next = [0.25, -0.25];
                l.process(&mut next);
                assert!(next[0].is_finite() && l.gain() > 0.0);
            }
        }
    }

    #[test]
    fn switching_off_mid_reduction_releases_without_a_jump() {
        let rate = 44_100;
        let mut l = Limiter::new(rate);
        l.set_enabled(true);
        let mut loud = [3.0, 3.0];
        l.process(&mut loud);
        let g0 = l.gain();
        assert!(g0 < 0.5);
        l.set_enabled(false);
        assert!(!l.enabled());
        // a steady tone below the ceiling: the gain glides back, no step larger than a release step
        let mut prev = g0;
        let mut frames = 0;
        while l.gain() < 1.0 {
            let mut f = [0.5, 0.5];
            l.process(&mut f);
            let step = l.gain() - prev;
            assert!((0.0..1e-3).contains(&step), "step {step} at {frames}");
            assert!((f[0] - 0.5 * l.gain()).abs() < 1e-7);
            prev = l.gain();
            frames += 1;
            assert!(frames < 10 * rate, "released");
        }
        // a new peak while switched off is not limited (it clips as without the limiter), and from unity on the
        // limiter is bypassed bit for bit
        let mut over = [2.0, 0.3];
        l.process(&mut over);
        assert_eq!(over, [1.0, 0.3]);
        assert_eq!(l.gain(), 1.0);
        let mut quiet = [0.123_456, -0.654_321];
        l.process(&mut quiet);
        assert_eq!(quiet, [0.123_456, -0.654_321]);
    }

    #[test]
    fn the_window_maximum_is_taken_and_reset() {
        let mut l = Limiter::new(44_100);
        l.set_enabled(true);
        let m = l.meter();
        let mut f = [2.0, 2.0];
        l.process(&mut f);
        let w = m.take_window();
        assert!(w > 6.0, "{w}");
        assert_eq!(m.take_window(), 0.0);
        assert!(m.enabled());
    }
}
