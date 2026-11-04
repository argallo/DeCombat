//! Frame-time distribution of a whole window run, in bounded memory (client-fps, our own tooling).
//!
//! The exit report of a window run gives the mean frame rate and the shortest / longest frame
//! ([`crate::FpsMeter`]); a mean hides hitches. [`FrameTimes`] keeps every frame of the run in a histogram of
//! logarithmic buckets (each bucket 1 % wider than the one before, from 1 µs up), so a session of hours costs a few
//! kilobytes and the median, p95 and p99 come back within 1 % of the true frame time. The worst frame is kept
//! exactly.
// DC-DEFAULT: CLI-390 (proposed) the exit report adds the frame-time percentiles and hitch counts, see DC_DEFAULTS.md

/// Bucket width ratio: each bucket is 1 % wider than the one before.
const RATIO: f64 = 1.01;
/// The lower edge of bucket 0 in ms (1 µs); shorter frames count into bucket 0.
const FLOOR_MS: f64 = 0.001;
/// Buckets: 1 µs * 1.01^2100 is about 1.2e6 ms (20 minutes); longer frames count into the last bucket.
const BUCKETS: usize = 2100;

/// Frames above this many ms are counted as hitches in the report (the async model loads' target, as the profiler).
pub const HITCH_MS: f64 = 5.0;
/// Frames above this many ms are counted as below 60 fps.
pub const SLOW_MS: f64 = 1000.0 / 60.0;

/// Frame times of a run: a log-bucket histogram plus the exact worst frame and the hitch counts.
#[derive(Debug, Clone)]
pub struct FrameTimes {
    counts: Vec<u32>,
    frames: u64,
    worst_ms: f64,
    over_hitch: u64,
    over_slow: u64,
}

impl Default for FrameTimes {
    fn default() -> Self {
        FrameTimes {
            counts: vec![0; BUCKETS],
            frames: 0,
            worst_ms: 0.0,
            over_hitch: 0,
            over_slow: 0,
        }
    }
}

fn bucket_of(ms: f64) -> usize {
    if ms.is_nan() || ms <= FLOOR_MS {
        return 0;
    }
    let b = ((ms / FLOOR_MS).ln() / RATIO.ln()).floor();
    (b as usize).min(BUCKETS - 1)
}

/// The middle of bucket `b` in ms (geometric middle of its edges).
fn bucket_ms(b: usize) -> f64 {
    FLOOR_MS * RATIO.powf(b as f64 + 0.5)
}

impl FrameTimes {
    /// Records one frame of `ms` milliseconds (a non-finite or negative time counts as 0).
    pub fn push_ms(&mut self, ms: f64) {
        let ms = if ms.is_finite() { ms.max(0.0) } else { 0.0 };
        let b = bucket_of(ms);
        self.counts[b] = self.counts[b].saturating_add(1);
        self.frames += 1;
        self.worst_ms = self.worst_ms.max(ms);
        if ms > HITCH_MS {
            self.over_hitch += 1;
        }
        if ms > SLOW_MS {
            self.over_slow += 1;
        }
    }

    /// Frames recorded.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The longest frame (exact), ms.
    pub fn worst_ms(&self) -> f64 {
        self.worst_ms
    }

    /// Frames longer than [`HITCH_MS`].
    pub fn over_hitch(&self) -> u64 {
        self.over_hitch
    }

    /// Frames longer than [`SLOW_MS`].
    pub fn over_slow(&self) -> u64 {
        self.over_slow
    }

    /// The frame time below which the fraction `q` (0..=1) of the frames lie, ms (0 without frames). Within 1 % of
    /// the true value; never above the worst frame.
    pub fn quantile_ms(&self, q: f64) -> f64 {
        if self.frames == 0 {
            return 0.0;
        }
        let q = if q.is_finite() {
            q.clamp(0.0, 1.0)
        } else {
            1.0
        };
        // the rank of the frame asked for, 1-based (q = 0.5 of 4 frames = the 2nd)
        let rank = ((q * self.frames as f64).ceil() as u64).clamp(1, self.frames);
        let mut seen = 0u64;
        for (b, &c) in self.counts.iter().enumerate() {
            seen += c as u64;
            if seen >= rank {
                return bucket_ms(b).min(self.worst_ms);
            }
        }
        self.worst_ms
    }

    /// One report line: median, p95, p99, worst and the hitch counts.
    pub fn line(&self) -> String {
        format!(
            "frame times: median {:.3} ms, p95 {:.3} ms, p99 {:.3} ms, worst {:.2} ms; {} frames over {} ms, {} over {:.1} ms",
            self.quantile_ms(0.5),
            self.quantile_ms(0.95),
            self.quantile_ms(0.99),
            self.worst_ms,
            self.over_hitch,
            HITCH_MS,
            self.over_slow,
            SLOW_MS
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_run_reports_zero() {
        let t = FrameTimes::default();
        assert_eq!(t.frames(), 0);
        assert_eq!(t.quantile_ms(0.5), 0.0);
        assert_eq!(t.worst_ms(), 0.0);
    }

    #[test]
    fn quantiles_are_within_one_percent() {
        let mut t = FrameTimes::default();
        // 1000 frames of 1.000..1.999 ms, then 10 of 50 ms
        for i in 0..1000 {
            t.push_ms(1.0 + i as f64 / 1000.0);
        }
        for _ in 0..10 {
            t.push_ms(50.0);
        }
        let med = t.quantile_ms(0.5);
        assert!((med - 1.505).abs() / 1.505 < 0.011, "median {med}");
        let p95 = t.quantile_ms(0.95);
        assert!((p95 - 1.96).abs() / 1.96 < 0.011, "p95 {p95}");
        // the top 1 % are the 50 ms frames, never above the worst
        let p99 = t.quantile_ms(0.995);
        assert!(p99 <= 50.0 && p99 > 49.0, "p99.5 {p99}");
        assert_eq!(t.worst_ms(), 50.0);
        assert_eq!(t.over_hitch(), 10);
        assert_eq!(t.over_slow(), 10);
        assert_eq!(t.frames(), 1010);
    }

    #[test]
    fn odd_times_are_clamped() {
        let mut t = FrameTimes::default();
        t.push_ms(f64::NAN);
        t.push_ms(-3.0);
        t.push_ms(0.0);
        t.push_ms(1e12);
        assert_eq!(t.frames(), 4);
        assert!(t.quantile_ms(0.25) < 0.002);
        assert_eq!(t.worst_ms(), 1e12);
        assert!(t.quantile_ms(1.0) > 1e5);
        assert!(t.line().contains("worst"));
    }
}
