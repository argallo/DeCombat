//! The link meter behind the network overlay (`/network`, CLI-1570..1574): a ring of 250 ms buckets over the last
//! 5 s, fed by the network thread with what the client already has (datagram sizes, the transport's counters and
//! round-trip samples, the stream's time entries, the playout counters of the remote tracks). Fixed size, no
//! allocation; never written to the event log.
// DC-DEFAULT: CLI-1571 (proposed) what the overlay measures and over which windows

/// One bucket's length and the ring's length (5 s).
pub const BUCKET_MS: u32 = 250;
pub const BUCKETS: usize = 20;
pub const WINDOW_MS: u32 = BUCKET_MS * BUCKETS as u32;
/// The per-second rates are taken over the last full second (four whole buckets).
pub const RATE_BUCKETS: usize = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Bucket {
    dg_in: u32,
    dg_out: u32,
    bytes_in: u64,
    bytes_out: u64,
    rtt_n: u32,
    rtt_sum: f64,
    rtt_sq: f64,
    rtt_min: f32,
    rtt_max: f32,
    seq_span: u64,
    seq_gaps: u64,
    rel_out: u64,
    resends: u64,
    upd_n: u32,
    upd_gap_sum: f64,
    upd_gap_max: u32,
    late: u64,
    held: u64,
}

/// The transport counters the meter takes deltas of (monotone over one link).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkTotals {
    pub rel_out: u64,
    pub resends: u64,
    pub seq_span: u64,
    pub seq_gaps: u64,
    /// Round-trip samples taken so far.
    pub rtt_total: u64,
}

/// What the overlay shows of the meter at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MeterView {
    /// Send-to-ack round trips of the last 5 s: mean, min, max (ms), and their standard deviation (jitter).
    pub rtt_mean: Option<f32>,
    pub rtt_min: Option<f32>,
    pub rtt_max: Option<f32>,
    pub jitter: Option<f32>,
    pub rtt_samples: u32,
    /// Loss over the last 5 s (%): in = reliable block numbers skipped / numbers advanced, out = resends / reliable
    /// blocks sent. `None` while the window holds no block.
    pub loss_in: Option<f32>,
    pub loss_out: Option<f32>,
    /// Datagrams and kbit (UDP payload) per second over the last whole second.
    pub pps_in: f32,
    pub pps_out: f32,
    pub kbps_in: f32,
    pub kbps_out: f32,
    /// Gaps between server updates (stream time entries) over 5 s: mean and max (ms).
    pub tick_gap_mean: Option<f32>,
    pub tick_gap_max: Option<u32>,
    /// Since the newest server update (ms).
    pub since_update: Option<u32>,
    /// Remote-player frames drawn past the newest sample (late) and past the extrapolation bound (held), 5 s.
    pub late_frames: u64,
    pub held_frames: u64,
}

/// The ring and the baselines of the counters it takes deltas of.
#[derive(Debug, Clone, Default)]
pub struct NetMeter {
    ring: [Bucket; BUCKETS],
    cur: usize,
    cur_start: Option<u32>,
    /// Whole buckets behind the current one (at most `BUCKETS - 1`).
    whole: usize,
    last_update: Option<u32>,
    link_seen: Option<LinkTotals>,
    entries_seen: Option<u64>,
    playout_seen: Option<(u64, u64)>,
}

fn delta(now: u64, seen: u64) -> u64 {
    now.saturating_sub(seen)
}

impl NetMeter {
    /// Moves the ring to `now` (clears the buckets passed).
    fn advance(&mut self, now: u32) -> &mut Bucket {
        let start = *self.cur_start.get_or_insert(now);
        let passed = now.wrapping_sub(start);
        if passed < u32::MAX / 2 && passed >= BUCKET_MS {
            let steps = (passed / BUCKET_MS) as usize;
            if steps >= BUCKETS {
                self.ring = [Bucket::default(); BUCKETS];
                self.cur_start = Some(now);
                self.whole = BUCKETS - 1;
            } else {
                for _ in 0..steps {
                    self.cur = (self.cur + 1) % BUCKETS;
                    self.ring[self.cur] = Bucket::default();
                }
                self.cur_start = Some(start.wrapping_add(steps as u32 * BUCKET_MS));
                self.whole = (self.whole + steps).min(BUCKETS - 1);
            }
        }
        &mut self.ring[self.cur]
    }

    /// Datagrams sent (`n` of `bytes` UDP payload bytes in all).
    pub fn sent(&mut self, now: u32, n: u32, bytes: u64) {
        let b = self.advance(now);
        b.dg_out += n;
        b.bytes_out += bytes;
    }

    /// One datagram received.
    pub fn received(&mut self, now: u32, bytes: usize) {
        let b = self.advance(now);
        b.dg_in += 1;
        b.bytes_in += bytes as u64;
    }

    /// The transport's counters now and its newest round-trip sample: the deltas since the last call go into the
    /// current bucket (the first call only sets the baseline; a counter that went back means a new link).
    pub fn link(&mut self, now: u32, t: LinkTotals, newest_rtt: Option<f32>) {
        let seen = match self.link_seen {
            Some(s) if t.rel_out >= s.rel_out && t.rtt_total >= s.rtt_total => s,
            _ => {
                self.link_seen = Some(t);
                return;
            }
        };
        if seen == t {
            return;
        }
        self.link_seen = Some(t);
        let b = self.advance(now);
        b.rel_out += delta(t.rel_out, seen.rel_out);
        b.resends += delta(t.resends, seen.resends);
        b.seq_span += delta(t.seq_span, seen.seq_span);
        b.seq_gaps += delta(t.seq_gaps, seen.seq_gaps);
        if t.rtt_total > seen.rtt_total {
            if let Some(r) = newest_rtt.filter(|r| r.is_finite() && *r >= 0.0) {
                self.rtt(now, r);
            }
        }
    }

    /// One round-trip sample (ms).
    pub fn rtt(&mut self, now: u32, ms: f32) {
        let b = self.advance(now);
        if b.rtt_n == 0 {
            b.rtt_min = ms;
            b.rtt_max = ms;
        }
        b.rtt_n += 1;
        b.rtt_sum += ms as f64;
        b.rtt_sq += (ms as f64) * (ms as f64);
        b.rtt_min = b.rtt_min.min(ms);
        b.rtt_max = b.rtt_max.max(ms);
    }

    /// The session's count of stream time entries: a rise is one server update arriving now.
    pub fn time_entries(&mut self, now: u32, total: u64) {
        let seen = self.entries_seen.replace(total);
        if seen.is_none_or(|s| total <= s) {
            return;
        }
        self.update_arrived(now);
    }

    /// One server update arrived at `now`.
    pub fn update_arrived(&mut self, now: u32) {
        let prev = self.last_update.replace(now);
        let b = self.advance(now);
        if let Some(p) = prev {
            let gap = now.wrapping_sub(p);
            if gap < u32::MAX / 2 {
                b.upd_n += 1;
                b.upd_gap_sum += gap as f64;
                b.upd_gap_max = b.upd_gap_max.max(gap);
            }
        }
    }

    /// The remote tracks' late / held frame counts now (summed over the tracks).
    pub fn playout(&mut self, now: u32, late: u64, held: u64) {
        let seen = self.playout_seen.replace((late, held));
        let Some((sl, sh)) = seen else {
            return;
        };
        if (sl, sh) == (late, held) {
            return;
        }
        let b = self.advance(now);
        b.late += delta(late, sl);
        b.held += delta(held, sh);
    }

    /// The overlay's numbers at `now`.
    pub fn view(&mut self, now: u32) -> MeterView {
        self.advance(now);
        let mut v = MeterView::default();
        let mut sum = Bucket::default();
        let mut have_rtt = false;
        for b in &self.ring {
            if b.rtt_n > 0 {
                if !have_rtt {
                    sum.rtt_min = b.rtt_min;
                    sum.rtt_max = b.rtt_max;
                    have_rtt = true;
                }
                sum.rtt_min = sum.rtt_min.min(b.rtt_min);
                sum.rtt_max = sum.rtt_max.max(b.rtt_max);
            }
            sum.rtt_n += b.rtt_n;
            sum.rtt_sum += b.rtt_sum;
            sum.rtt_sq += b.rtt_sq;
            sum.seq_span += b.seq_span;
            sum.seq_gaps += b.seq_gaps;
            sum.rel_out += b.rel_out;
            sum.resends += b.resends;
            sum.upd_n += b.upd_n;
            sum.upd_gap_sum += b.upd_gap_sum;
            sum.upd_gap_max = sum.upd_gap_max.max(b.upd_gap_max);
            sum.late += b.late;
            sum.held += b.held;
        }
        if sum.rtt_n > 0 {
            let n = sum.rtt_n as f64;
            let mean = sum.rtt_sum / n;
            v.rtt_mean = Some(mean as f32);
            v.rtt_min = Some(sum.rtt_min);
            v.rtt_max = Some(sum.rtt_max);
            v.jitter = Some((sum.rtt_sq / n - mean * mean).max(0.0).sqrt() as f32);
        }
        v.rtt_samples = sum.rtt_n;
        let pct = |a: u64, b: u64| (b > 0).then(|| (a as f32 * 100.0 / b as f32).min(100.0));
        v.loss_in = pct(sum.seq_gaps, sum.seq_span);
        v.loss_out = pct(sum.resends, sum.rel_out);
        if sum.upd_n > 0 {
            v.tick_gap_mean = Some((sum.upd_gap_sum / sum.upd_n as f64) as f32);
            v.tick_gap_max = Some(sum.upd_gap_max);
        }
        v.since_update = self.last_update.map(|t| now.wrapping_sub(t));
        v.late_frames = sum.late;
        v.held_frames = sum.held;
        // the rates: the last whole buckets (up to one second) behind the current one
        let k = self.whole.min(RATE_BUCKETS);
        if k > 0 {
            let mut r = Bucket::default();
            for i in 1..=k {
                let b = &self.ring[(self.cur + BUCKETS - i) % BUCKETS];
                r.dg_in += b.dg_in;
                r.dg_out += b.dg_out;
                r.bytes_in += b.bytes_in;
                r.bytes_out += b.bytes_out;
            }
            let secs = k as f32 * BUCKET_MS as f32 / 1000.0;
            v.pps_in = r.dg_in as f32 / secs;
            v.pps_out = r.dg_out as f32 / secs;
            v.kbps_in = r.bytes_in as f32 * 8.0 / 1000.0 / secs;
            v.kbps_out = r.bytes_out as f32 * 8.0 / 1000.0 / secs;
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totals(rel_out: u64, resends: u64, span: u64, gaps: u64, rtt: u64) -> LinkTotals {
        LinkTotals {
            rel_out,
            resends,
            seq_span: span,
            seq_gaps: gaps,
            rtt_total: rtt,
        }
    }

    #[test]
    fn rates_are_per_second_over_the_last_whole_second() {
        let mut m = NetMeter::default();
        // 30 datagrams of 100 bytes in and 15 of 60 bytes out per second, for 3 s
        for ms in 0..3000u32 {
            if ms % 33 == 0 && ms < 2970 {
                m.received(ms, 100);
            }
            if ms % 66 == 0 && ms < 2970 {
                m.sent(ms, 1, 60);
            }
        }
        let v = m.view(3000);
        assert!((v.pps_in - 30.0).abs() <= 1.0, "{}", v.pps_in);
        assert!((v.kbps_in - 24.0).abs() <= 1.0, "{}", v.kbps_in);
        assert!((v.pps_out - 15.0).abs() <= 1.0, "{}", v.pps_out);
        assert!((v.kbps_out - 7.2).abs() <= 0.5, "{}", v.kbps_out);
    }

    #[test]
    fn loss_in_and_out_come_from_the_link_counters_of_the_last_five_seconds() {
        let mut m = NetMeter::default();
        m.link(0, totals(0, 0, 0, 0, 0), None);
        // 200 blocks out, 10 resent; 100 numbers in, 4 skipped
        m.link(1000, totals(200, 10, 100, 4, 0), None);
        let v = m.view(1000);
        assert_eq!(v.loss_out, Some(5.0));
        assert_eq!(v.loss_in, Some(4.0));
        // 5 s later the window is empty again
        let v = m.view(7000);
        assert_eq!((v.loss_in, v.loss_out), (None, None));
        // a new link (counters went back) only sets the baseline
        m.link(7100, totals(3, 0, 1, 0, 0), None);
        m.link(7200, totals(5, 1, 3, 1, 0), None);
        let v = m.view(7200);
        assert_eq!(v.loss_out, Some(50.0));
        assert_eq!(v.loss_in, Some(50.0));
    }

    #[test]
    fn ping_and_jitter_over_five_seconds() {
        let mut m = NetMeter::default();
        m.link(0, totals(0, 0, 0, 0, 0), None);
        let samples = [50.0f32, 70.0, 50.0, 70.0];
        for (i, s) in samples.iter().enumerate() {
            m.link(100 + i as u32 * 300, totals(0, 0, 0, 0, i as u64 + 1), Some(*s));
        }
        // no new sample: the newest is not counted twice
        m.link(1500, totals(0, 0, 0, 0, 4), Some(70.0));
        let v = m.view(1500);
        assert_eq!(v.rtt_samples, 4);
        assert_eq!(v.rtt_mean, Some(60.0));
        assert_eq!((v.rtt_min, v.rtt_max), (Some(50.0), Some(70.0)));
        assert_eq!(v.jitter, Some(10.0));
        // older than 5 s: gone
        let v = m.view(6200);
        assert_eq!(v.rtt_mean, None);
    }

    #[test]
    fn server_update_gaps_and_age() {
        let mut m = NetMeter::default();
        m.time_entries(0, 1);
        for i in 1..=30u64 {
            m.time_entries(i as u32 * 33, i + 1);
        }
        m.time_entries(1100, 40);
        let v = m.view(1150);
        assert_eq!(v.tick_gap_max, Some(110));
        let mean = v.tick_gap_mean.unwrap();
        // the first count only sets the baseline: 30 gaps from 33 ms to 1100 ms
        assert!((mean - 1067.0 / 30.0).abs() < 0.01, "{mean}");
        assert_eq!(v.since_update, Some(50));
        // an unchanged count is no update
        m.time_entries(1200, 40);
        assert_eq!(m.view(1200).since_update, Some(100));
    }

    #[test]
    fn playout_counters_are_deltas() {
        let mut m = NetMeter::default();
        m.playout(0, 10, 2);
        m.playout(500, 13, 2);
        m.playout(900, 20, 5);
        let v = m.view(1000);
        assert_eq!((v.late_frames, v.held_frames), (10, 3));
        // a track went away (the sum fell): no negative counts
        m.playout(1100, 4, 0);
        m.playout(1200, 6, 1);
        let v = m.view(1200);
        assert_eq!((v.late_frames, v.held_frames), (12, 4));
    }
}
