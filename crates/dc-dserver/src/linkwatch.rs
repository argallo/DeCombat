//! Link statistics of one client for the server log: every [`REPORT_MS`] one line (round trip, reliable resends,
//! flow drops, the rate and the arrival jitter of the client's player updates) and one summary when the link closes.
//! The same window goes to the lobby as the `@net` pipe line (DSV-411). Diagnostics only: nothing here changes
//! what is sent.
// DC-DEFAULT: CLI-1331 (proposed) dserver link line per client every 10 s and a summary at the link's end
// DC-DEFAULT: DSV-412 (proposed) the window's round trip: mean / min / max of the raw samples taken in it

use dc_net_engine::time::since;
use dc_net_engine::{LinkStats, Millis};

/// Cadence of the per-client link line.
pub const REPORT_MS: u32 = 10_000;
/// Gain of the interarrival jitter estimate (RFC 3550 §6.4.1 form: `J += (|D| - J) / 16`).
const JITTER_GAIN: f32 = 1.0 / 16.0;
/// A transit difference beyond this (ms) is a clock jump of the client (teleport, pause), not jitter.
const JITTER_CLAMP_MS: f32 = 1_000.0;

/// One report window of a link (what the log line and the `@net` pipe line carry).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LinkWindow {
    /// Window length (ms).
    pub ms: u32,
    /// The link's round-trip estimate at the window's end (the transport's 16-sample mean).
    pub rtt_ms: f32,
    /// Raw round-trip samples taken in the window: count, mean, min, max (`None` without a sample).
    pub rtt_samples: u64,
    pub rtt_mean: Option<f32>,
    pub rtt_min: Option<f32>,
    pub rtt_max: Option<f32>,
    /// Player updates per second, their interarrival jitter (ms) and the longest gap (ms).
    pub input_rate: f32,
    pub jitter_ms: f32,
    pub max_gap_ms: u32,
    pub resends: u64,
    pub flow_drops: u64,
    pub datagrams_in: u64,
    pub datagrams_out: u64,
    /// Largest datagram the server sent on the link so far (UDP payload bytes; NET-10 keeps it at 548 or less).
    pub max_datagram_bytes: u32,
}

impl LinkWindow {
    /// The server log line (`client N: ` is the caller's prefix).
    pub fn log_line(&self) -> String {
        let spread = match (self.rtt_mean, self.rtt_min, self.rtt_max) {
            (Some(m), Some(lo), Some(hi)) => format!(
                " (window mean {m:.0}, min {lo:.0}, max {hi:.0}, {} samples)",
                self.rtt_samples
            ),
            _ => String::new(),
        };
        format!(
            "link {} s: rtt {:.0} ms{spread}, input {:.1}/s (jitter {:.1} ms, longest gap {} ms), resends {}, flow drops {}, datagrams in {} out {} (largest {} B)",
            self.ms / 1000,
            self.rtt_ms,
            self.input_rate,
            self.jitter_ms,
            self.max_gap_ms,
            self.resends,
            self.flow_drops,
            self.datagrams_in,
            self.datagrams_out,
            self.max_datagram_bytes,
        )
    }

    /// The window's round trip for a reader: the samples' mean / min / max, else the estimate three times.
    pub fn rtt_triple(&self) -> (f32, f32, f32) {
        match (self.rtt_mean, self.rtt_min, self.rtt_max) {
            (Some(m), Some(lo), Some(hi)) => (m, lo, hi),
            _ => (self.rtt_ms, self.rtt_ms, self.rtt_ms),
        }
    }
}

/// What the server measured on one client's link.
#[derive(Debug, Clone, Default)]
pub struct LinkWatch {
    started: Option<Millis>,
    report_at: Millis,
    /// Player updates: total, at the last report; arrival (server ms) and client time (s) of the newest.
    updates: u64,
    updates_at_report: u64,
    last_arrival: Option<Millis>,
    last_client_s: Option<f32>,
    /// Interarrival jitter of the updates (ms) and the longest gap between two of them in the window / overall.
    pub jitter_ms: f32,
    max_gap_window: u32,
    max_gap: u32,
    /// The newest link counters and those of the last report.
    stats: LinkStats,
    stats_at_report: LinkStats,
    rtt_sum: f64,
    rtt_n: u64,
    rtt_min: f32,
    rtt_max: f32,
    /// Raw round-trip samples of the current window: the transport's sample count last seen, sum, n, min, max.
    seen_samples: u64,
    win_sum: f64,
    win_n: u64,
    win_min: f32,
    win_max: f32,
    /// Every raw round-trip sample of the link's life: sum, count, max and a histogram for the percentile.
    all_sum: f64,
    all_n: u64,
    all_max: f32,
    hist: RttHistogram,
}

/// Bucket width (ms) of the round-trip histogram and its bucket count (the last bucket holds everything above).
const HIST_STEP_MS: f32 = 5.0;
const HIST_BUCKETS: usize = 400;

/// Raw round-trip samples in 5 ms buckets up to 2 s: the 95th percentile of the link's life without keeping samples.
// DC-DEFAULT: MON-1 (proposed) the per-player link summary to the lobby
#[derive(Debug, Clone, Default)]
struct RttHistogram {
    counts: Vec<u32>,
}

impl RttHistogram {
    fn add(&mut self, ms: f32) {
        if self.counts.is_empty() {
            self.counts = vec![0; HIST_BUCKETS];
        }
        let i = ((ms.max(0.0) / HIST_STEP_MS) as usize).min(HIST_BUCKETS - 1);
        self.counts[i] = self.counts[i].saturating_add(1);
    }

    /// The upper edge (ms) of the bucket that holds the `q` quantile; `None` without samples.
    fn quantile(&self, q: f64) -> Option<f32> {
        let total: u64 = self.counts.iter().map(|&c| c as u64).sum();
        if total == 0 {
            return None;
        }
        let want = ((total as f64) * q).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (i, &c) in self.counts.iter().enumerate() {
            seen += c as u64;
            if seen >= want {
                return Some((i as f32 + 1.0) * HIST_STEP_MS);
            }
        }
        None
    }
}

/// What the server measured on one link over its whole life (the `@linksum` pipe line, MON-1).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LinkTotals {
    /// Seconds from the link's first tick to `now`.
    pub secs: f32,
    /// Round trip (ms) over the raw samples (mean, 95th percentile to 5 ms, max); the transport estimate's mean
    /// stands in for all three when the link took no raw sample.
    pub rtt_mean: f32,
    pub rtt_p95: f32,
    pub rtt_max: f32,
    pub rtt_samples: u64,
    pub updates: u64,
    pub input_rate: f32,
    pub jitter_ms: f32,
    pub max_gap_ms: u32,
    pub resends: u64,
    pub reliable_blocks: u64,
    pub flow_drops: u64,
    pub datagrams_in: u64,
    pub datagrams_out: u64,
    pub max_datagram_bytes: u32,
}

impl LinkWatch {
    /// One player update arrived at `now`, stamped `client_s` (the update's client time, seconds).
    pub fn on_update(&mut self, now: Millis, client_s: f32) {
        self.updates += 1;
        if let (Some(a), Some(c)) = (self.last_arrival, self.last_client_s) {
            let gap = since(now, a);
            self.max_gap_window = self.max_gap_window.max(gap);
            self.max_gap = self.max_gap.max(gap);
            if client_s.is_finite() && c.is_finite() {
                let d = gap as f32 - (client_s - c) * 1000.0;
                if d.abs() < JITTER_CLAMP_MS {
                    self.jitter_ms += (d.abs() - self.jitter_ms) * JITTER_GAIN;
                }
            }
        }
        self.last_arrival = Some(now);
        self.last_client_s = Some(client_s);
    }

    /// Every tick: the link's counters, its round-trip estimate and its newest raw sample (`(samples taken so
    /// far, newest)`, `dc_net_engine::Latency::newest`); returns the window when a report is due.
    pub fn tick(
        &mut self,
        now: Millis,
        stats: LinkStats,
        rtt_ms: f32,
        newest: (u64, Option<f32>),
    ) -> Option<LinkWindow> {
        let start = *self.started.get_or_insert(now);
        if self.report_at == 0 {
            self.report_at = start;
        }
        self.stats = stats;
        if rtt_ms.is_finite() && rtt_ms >= 0.0 {
            self.rtt_sum += rtt_ms as f64;
            self.rtt_n += 1;
            self.rtt_min = if self.rtt_n == 1 {
                rtt_ms
            } else {
                self.rtt_min.min(rtt_ms)
            };
            self.rtt_max = self.rtt_max.max(rtt_ms);
        }
        if let (total, Some(v)) = newest {
            if total != self.seen_samples && v.is_finite() && v >= 0.0 {
                self.seen_samples = total;
                self.win_sum += v as f64;
                self.win_n += 1;
                self.win_min = if self.win_n == 1 {
                    v
                } else {
                    self.win_min.min(v)
                };
                self.win_max = if self.win_n == 1 {
                    v
                } else {
                    self.win_max.max(v)
                };
                self.all_sum += v as f64;
                self.all_n += 1;
                self.all_max = self.all_max.max(v);
                self.hist.add(v);
            }
        }
        let w = since(now, self.report_at);
        if w < REPORT_MS {
            return None;
        }
        let (s, s0) = (self.stats, self.stats_at_report);
        let has = self.win_n > 0;
        let win = LinkWindow {
            ms: w,
            rtt_ms,
            rtt_samples: self.win_n,
            rtt_mean: has.then(|| (self.win_sum / self.win_n as f64) as f32),
            rtt_min: has.then_some(self.win_min),
            rtt_max: has.then_some(self.win_max),
            input_rate: (self.updates - self.updates_at_report) as f32 * 1000.0 / w as f32,
            jitter_ms: self.jitter_ms,
            max_gap_ms: self.max_gap_window,
            resends: s.resends - s0.resends,
            flow_drops: s.flow_drops - s0.flow_drops,
            datagrams_in: s.datagrams_in - s0.datagrams_in,
            datagrams_out: s.datagrams_out - s0.datagrams_out,
            max_datagram_bytes: s.max_datagram_bytes,
        };
        self.report_at = now;
        self.updates_at_report = self.updates;
        self.stats_at_report = s;
        self.max_gap_window = 0;
        self.win_sum = 0.0;
        self.win_n = 0;
        Some(win)
    }

    /// The link's totals at `now` (from the counters of the last tick).
    // DC-DEFAULT: MON-1 (proposed)
    pub fn totals(&self, now: Millis) -> LinkTotals {
        let secs = self.started.map_or(0, |s| since(now, s)) as f32 / 1000.0;
        let s = self.stats;
        let est = (self.rtt_n > 0).then(|| (self.rtt_sum / self.rtt_n as f64) as f32);
        let (mean, p95, max) = if self.all_n > 0 {
            (
                (self.all_sum / self.all_n as f64) as f32,
                self.hist
                    .quantile(0.95)
                    .unwrap_or(self.all_max)
                    .min(self.all_max.max(0.0)),
                self.all_max,
            )
        } else {
            let e = est.unwrap_or(0.0);
            (e, e, est.map_or(0.0, |_| self.rtt_max))
        };
        LinkTotals {
            secs,
            rtt_mean: mean,
            rtt_p95: p95,
            rtt_max: max,
            rtt_samples: self.all_n,
            updates: self.updates,
            input_rate: if secs > 0.0 {
                self.updates as f32 / secs
            } else {
                0.0
            },
            jitter_ms: self.jitter_ms,
            max_gap_ms: self.max_gap,
            resends: s.resends,
            reliable_blocks: s.rel_blocks_out,
            flow_drops: s.flow_drops,
            datagrams_in: s.datagrams_in,
            datagrams_out: s.datagrams_out,
            max_datagram_bytes: s.max_datagram_bytes,
        }
    }

    /// The summary at the link's end (from the counters of the last tick).
    pub fn summary(&self, now: Millis) -> String {
        let secs = self.started.map_or(0, |s| since(now, s)) as f32 / 1000.0;
        let s = self.stats;
        let rtt = if self.rtt_n > 0 {
            format!(
                "mean {:.0} ms (min {:.0}, max {:.0})",
                self.rtt_sum / self.rtt_n as f64,
                self.rtt_min,
                self.rtt_max
            )
        } else {
            "n/a".to_string()
        };
        format!(
            "link summary {:.0} s: rtt {rtt}, input {} updates ({:.1}/s, jitter {:.1} ms, \
             longest gap {} ms), resends {} of {} reliable blocks, flow drops {}, datagrams in {} out {} (largest {} B)",
            secs,
            self.updates,
            if secs > 0.0 {
                self.updates as f32 / secs
            } else {
                0.0
            },
            self.jitter_ms,
            self.max_gap,
            s.resends,
            s.rel_blocks_out,
            s.flow_drops,
            s.datagrams_in,
            s.datagrams_out,
            s.max_datagram_bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_updates_have_no_jitter_and_jittered_ones_do() {
        let mut w = LinkWatch::default();
        for k in 0..200u32 {
            w.on_update(1000 + k * 16, k as f32 * 0.016);
        }
        assert!(w.jitter_ms < 0.5, "{}", w.jitter_ms);
        let mut j = LinkWatch::default();
        for k in 0..200u32 {
            // every other update 20 ms late
            j.on_update(1000 + k * 16 + (k % 2) * 20, k as f32 * 0.016);
        }
        assert!((15.0..=20.0).contains(&j.jitter_ms), "{}", j.jitter_ms);
        // a client clock jump is not jitter
        let before = j.jitter_ms;
        j.on_update(5000, 999.0);
        assert_eq!(j.jitter_ms, before);
    }

    #[test]
    fn one_line_every_10_s_with_the_window_counts_and_a_summary() {
        let mut w = LinkWatch::default();
        let mut st = LinkStats::default();
        assert!(w.tick(100, st, 80.0, (0, None)).is_none());
        for k in 0..300u32 {
            w.on_update(100 + k * 33, k as f32 * 0.033);
        }
        st.resends = 3;
        st.datagrams_in = 600;
        let line = w
            .tick(10_100, st, 90.0, (0, None))
            .expect("a line after 10 s")
            .log_line();
        assert!(
            line.contains("rtt 90 ms,") && line.contains("input 30.0/s"),
            "{line}"
        );
        assert!(
            line.contains("resends 3") && line.contains("datagrams in 600"),
            "{line}"
        );
        assert!(w.tick(10_200, st, 90.0, (0, None)).is_none());
        let line = w.tick(20_100, st, 90.0, (0, None)).unwrap().log_line();
        assert!(
            line.contains("resends 0") && line.contains("input 0.0/s"),
            "{line}"
        );
        let s = w.summary(20_100);
        assert!(s.contains("300 updates") && s.contains("resends 3"), "{s}");
    }

    #[test]
    fn the_window_round_trip_counts_each_raw_sample_once() {
        let mut w = LinkWatch::default();
        let st = LinkStats::default();
        // samples 1..=3 seen at ticks; sample 2 is seen on two ticks and counted once
        w.tick(0, st, 50.0, (1, Some(40.0)));
        w.tick(100, st, 50.0, (2, Some(120.0)));
        w.tick(200, st, 50.0, (2, Some(120.0)));
        w.tick(300, st, 50.0, (3, Some(50.0)));
        let win = w.tick(10_000, st, 70.0, (3, Some(50.0))).unwrap();
        assert_eq!(win.rtt_samples, 3);
        assert_eq!(win.rtt_triple(), (70.0, 40.0, 120.0));
        assert!(
            win.log_line()
                .contains("rtt 70 ms (window mean 70, min 40, max 120, 3 samples),"),
            "{}",
            win.log_line()
        );
        // the next window starts empty: the estimate stands in for the triple
        let win = w.tick(20_000, st, 66.0, (3, Some(50.0))).unwrap();
        assert_eq!((win.rtt_samples, win.rtt_mean), (0, None));
        assert_eq!(win.rtt_triple(), (66.0, 66.0, 66.0));
    }

    #[test]
    fn totals_carry_the_raw_samples_mean_p95_and_max() {
        let mut w = LinkWatch::default();
        let mut st = LinkStats::default();
        // 100 raw samples: 95 of 40 ms, 5 of 300 ms; the p95 bucket is the one of 40 ms
        for k in 0..100u64 {
            let v = if k < 95 { 40.0 } else { 300.0 };
            w.tick(k as u32 * 10, st, 50.0, (k + 1, Some(v)));
        }
        for k in 0..200u32 {
            w.on_update(k * 50, k as f32 * 0.05);
        }
        st.resends = 4;
        st.rel_blocks_out = 80;
        st.datagrams_in = 900;
        st.datagrams_out = 450;
        st.max_datagram_bytes = 512;
        w.tick(10_000, st, 50.0, (100, Some(300.0)));
        let t = w.totals(10_000);
        assert_eq!(t.rtt_samples, 100);
        assert!((t.rtt_mean - 53.0).abs() < 0.01, "{t:?}");
        assert_eq!(t.rtt_p95, 45.0, "upper edge of the 40 ms bucket");
        assert_eq!(t.rtt_max, 300.0);
        assert_eq!((t.updates, t.max_gap_ms), (200, 50));
        assert!((t.input_rate - 20.0).abs() < 0.01, "{t:?}");
        assert_eq!(
            (
                t.resends,
                t.reliable_blocks,
                t.datagrams_in,
                t.datagrams_out
            ),
            (4, 80, 900, 450)
        );
        assert_eq!(t.max_datagram_bytes, 512);
        assert_eq!(t.secs, 10.0);
        // 6 % slow samples move the p95 into the slow bucket
        let mut w = LinkWatch::default();
        for k in 0..100u64 {
            let v = if k < 94 { 40.0 } else { 300.0 };
            w.tick(k as u32, LinkStats::default(), 50.0, (k + 1, Some(v)));
        }
        assert_eq!(w.totals(100).rtt_p95, 300.0, "capped at the max");
        // no raw sample: the estimate's mean for all three
        let mut w = LinkWatch::default();
        w.tick(0, LinkStats::default(), 30.0, (0, None));
        w.tick(10, LinkStats::default(), 50.0, (0, None));
        let t = w.totals(10);
        assert_eq!((t.rtt_mean, t.rtt_p95, t.rtt_samples), (40.0, 40.0, 0));
        assert_eq!(LinkWatch::default().totals(0).rtt_max, 0.0);
    }

    #[test]
    fn a_summary_without_a_round_trip_prints_na() {
        let mut w = LinkWatch::default();
        w.on_update(100, 0.0);
        let s = w.summary(5_000);
        assert!(s.contains("rtt n/a,") && !s.contains("min 0"), "{s}");
        w.tick(100, LinkStats::default(), 40.0, (0, None));
        let s = w.summary(5_000);
        assert!(s.contains("rtt mean 40 ms (min 40, max 40)"), "{s}");
    }
}
