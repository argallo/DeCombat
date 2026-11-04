//! Test bench of the remote-player playout (CLI-11 / CLI-1330): a 30 Hz stream of a running player over a link with
//! a fixed delay, uniform jitter and loss, polled like the network thread (every 2 ms) and drawn like the render
//! loop (every 7 ms), measured on what the viewer sees: frozen frames, jumps and the speed error of the drawn body.

use super::Track;
#[cfg(test)]
use super::{INTERP_MIN_MS, LATE_MARGIN_MS};
use dc_botclient::objects::ClientObject;

/// Link model: one-way base delay, uniform extra delay 0..jitter, loss probability.
#[derive(Clone, Copy, Debug)]
pub struct LinkModel {
    pub base_ms: f64,
    pub jitter_ms: f64,
    pub loss: f64,
}

/// What the viewer saw.
#[derive(Clone, Copy, Debug, Default)]
pub struct Seen {
    pub frames: u32,
    /// Frames in which the body did not move although the player ran (step < 20 % of the true step).
    pub frozen: u32,
    /// Frames in which the body moved more than twice the true step.
    pub jumps: u32,
    /// Largest step beyond the true one (world units).
    pub max_jump: f32,
    /// Root mean square of the drawn speed error, in % of the run speed.
    pub rms_speed_err_pct: f32,
}

pub const SPEED: f64 = 300.0;
const TICK_MS: f64 = 1000.0 / 30.0;
const POLL_MS: u32 = 2;
const FRAME_MS: u32 = 7;

/// The true position and velocity at server time `t` ms: runs at [`SPEED`] along +x and +z alternately, 1 s each.
pub fn truth(t: f64) -> ([f32; 3], [f32; 3]) {
    let n = (t / 1000.0).floor().max(0.0) as u64;
    let rem = (t - n as f64 * 1000.0) / 1000.0 * SPEED;
    let (fx, fz) = ((n + 1) / 2, n / 2);
    let mut x = fx as f64 * SPEED;
    let mut z = fz as f64 * SPEED;
    let v = if n % 2 == 0 {
        x += rem;
        [SPEED as f32, 0.0, 0.0]
    } else {
        z += rem;
        [0.0, 0.0, SPEED as f32]
    };
    ([x as f32, 0.0, z as f32], v)
}

/// xorshift64*: deterministic noise for the link.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Runs `secs` seconds of stream over `link`; `draw` gives the drawn position of the track at local time `now`.
pub fn run(
    link: LinkModel,
    secs: u32,
    seed: u64,
    mut draw: impl FnMut(&mut Track, u32) -> Option<[f32; 3]>,
) -> (Seen, Track) {
    let mut rng = Rng(seed | 1);
    // (arrival ms, server ms) of every packet that survives the link, in arrival order
    let mut packets: Vec<(f64, f64)> = Vec::new();
    let ticks = (secs as f64 * 1000.0 / TICK_MS) as u64;
    for k in 0..ticks {
        let s = k as f64 * TICK_MS;
        let lost = rng.next() < link.loss;
        let a = s + link.base_ms + rng.next() * link.jitter_ms;
        if !lost {
            packets.push((a, s));
        }
    }
    packets.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut o = ClientObject::default();
    let mut t = Track::default();
    let mut server_time: Option<f32> = None;
    let mut next = 0usize;
    let mut seen = Seen::default();
    let mut prev: Option<[f32; 3]> = None;
    let mut err_sq = 0.0f64;
    let end = secs * 1000;
    let warmup = 2000;
    let mut now = 0u32;
    while now < end {
        if now % POLL_MS == 0 {
            let mut got = false;
            while next < packets.len() && packets[next].0 <= now as f64 {
                let (_, s) = packets[next];
                next += 1;
                let (p, v) = truth(s);
                // the object table: positions always taken, the time entry only when later (B §9.1)
                o.stream_entries += 1;
                o.pos = Some(p);
                o.vel = Some(v);
                let st = (s / 1000.0) as f32;
                o.stream_time = Some(st);
                if server_time.is_none_or(|x| st > x) {
                    server_time = Some(st);
                }
                got = true;
            }
            if got {
                t.follow_object_at(&o, now, server_time);
            }
        }
        if now % FRAME_MS == 0 {
            if let Some(d) = draw(&mut t, now) {
                if now >= warmup {
                    if let Some(q) = prev {
                        let step = ((d[0] - q[0]).powi(2) + (d[2] - q[2]).powi(2)).sqrt();
                        let want = (SPEED * FRAME_MS as f64 / 1000.0) as f32;
                        seen.frames += 1;
                        if step < 0.2 * want {
                            seen.frozen += 1;
                        }
                        if step > 2.0 * want {
                            seen.jumps += 1;
                        }
                        seen.max_jump = seen.max_jump.max(step - want);
                        let e = (step - want) as f64 / want as f64;
                        err_sq += e * e;
                    }
                }
                prev = Some(d);
            }
        }
        now += 1;
    }
    seen.rms_speed_err_pct = ((err_sq / seen.frames.max(1) as f64).sqrt() * 100.0) as f32;
    (seen, t)
}

pub const NO_JITTER: LinkModel = LinkModel {
    base_ms: 60.0,
    jitter_ms: 0.0,
    loss: 0.0,
};
pub const JITTER_30: LinkModel = LinkModel {
    base_ms: 60.0,
    jitter_ms: 30.0,
    loss: 0.0,
};
pub const LOSS_2_JITTER_30: LinkModel = LinkModel {
    base_ms: 60.0,
    jitter_ms: 30.0,
    loss: 0.02,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn ratio(n: u32, d: u32) -> f32 {
        100.0 * n as f32 / d.max(1) as f32
    }

    fn line(name: &str, s: &Seen) -> String {
        format!(
            "{name}: frozen {:.2}% jumps {:.2}% max_jump {:.1} u rms_speed_err {:.1}%",
            ratio(s.frozen, s.frames),
            ratio(s.jumps, s.frames),
            s.max_jump,
            s.rms_speed_err_pct
        )
    }

    /// CLI-11 as it was (arrival stamps, 1.5 x interval, held at the newest sample), rebuilt from the arrival times
    /// the bench hands the track: the reference for the before / after numbers.
    fn old_rule_run(l: LinkModel) -> Seen {
        let mut arr: Vec<(f64, [f32; 3])> = Vec::new();
        let mut gap = 0.0f32;
        let mut last_n = 0u64;
        run(l, 120, 7, |t, now| {
            if t.total.samples != last_n {
                last_n = t.total.samples;
                let (p, at) = (t.last?, t.last_arrival? as f64);
                if let Some(&(a, _)) = arr.last() {
                    let dt = (at - a) as f32;
                    gap = if gap <= 0.0 { dt } else { 0.9 * gap + 0.1 * dt };
                }
                arr.push((at, p));
            }
            let d = (INTERP_FACTOR_OLD * if gap > 0.0 { gap } else { 100.0 }).clamp(50.0, 250.0);
            let r = now as f64 - d as f64;
            let (&(t0, p0), &(t1, p1)) = (arr.first()?, arr.last()?);
            if r <= t0 {
                return Some(p0);
            }
            if r >= t1 {
                return Some(p1);
            }
            let k = arr.partition_point(|x| x.0 < r);
            let ((ta, pa), (tb, pb)) = (arr[k - 1], arr[k]);
            let a = ((r - ta) / (tb - ta).max(1e-3)) as f32;
            Some([
                pa[0] + (pb[0] - pa[0]) * a,
                0.0,
                pa[2] + (pb[2] - pa[2]) * a,
            ])
        })
        .0
    }
    const INTERP_FACTOR_OLD: f32 = 1.5;

    const LINKS: [(&str, LinkModel); 4] = [
        ("(a) no jitter", NO_JITTER),
        ("(b) 30 ms jitter", JITTER_30),
        ("(c) 2% loss + 30 ms jitter", LOSS_2_JITTER_30),
        (
            "(d) 5% loss + 60 ms jitter",
            LinkModel {
                base_ms: 60.0,
                jitter_ms: 60.0,
                loss: 0.05,
            },
        ),
    ];

    #[test]
    fn bench_before_and_after() {
        // `cargo test -p dc-client --lib jitter_sim -- --nocapture` prints the table
        for (name, l) in LINKS {
            let old = old_rule_run(l);
            let (new, t) = run(l, 120, 7, |t, now| t.draw(now));
            println!("BENCH before {}", line(name, &old));
            println!(
                "BENCH after  {} | late runs/min {:.1} holds/min {:.1} delay {:.0} ms jitter q95 {:.0} ms",
                line(name, &new),
                t.total.late_runs as f32 / 2.0,
                t.total.held_runs as f32 / 2.0,
                t.total.delay_sum_ms / t.total.frames.max(1) as f64,
                t.late_q
            );
        }
    }

    #[test]
    fn a_clean_link_keeps_the_cli_11_delay_and_draws_smoothly() {
        // loopback-like: 30 Hz, no jitter: the CLI-1330 floor of two intervals + 4 ms (70.7 ms), a hair over the
        // 2009 50 ms minimum
        let (s, t) = run(
            LinkModel {
                base_ms: 0.5,
                jitter_ms: 0.0,
                loss: 0.0,
            },
            20,
            3,
            |t, now| t.draw(now),
        );
        let floor = (2.0_f32 * 1000.0 / 30.0 + LATE_MARGIN_MS).max(INTERP_MIN_MS);
        assert!(
            (t.playout_delay(20_000) - floor).abs() < 1.0,
            "{} vs {floor}",
            t.playout_delay(20_000)
        );
        assert_eq!((s.frozen, s.jumps, t.total.late_frames), (0, 0, 0), "{s:?}");
        assert!(s.rms_speed_err_pct < 5.0, "{s:?}");
    }

    #[test]
    fn jitter_raises_the_delay_and_keeps_the_drawn_speed_even() {
        let (s, t) = run(JITTER_30, 60, 11, |t, now| t.draw(now));
        let d = t.total.delay_sum_ms / t.total.frames as f64;
        assert!((60.0..80.0).contains(&d), "delay {d}");
        assert!(
            ratio(s.frozen, s.frames) < 0.05 && ratio(s.jumps, s.frames) < 0.1,
            "{s:?}"
        );
        assert!(s.rms_speed_err_pct < 10.0, "{s:?}");
        let old = old_rule_run(JITTER_30);
        assert!(
            old.rms_speed_err_pct > 3.0 * s.rms_speed_err_pct,
            "{old:?} vs {s:?}"
        );
    }

    #[test]
    fn a_lost_sample_is_bridged_by_the_velocity_not_frozen() {
        let (s, t) = run(LOSS_2_JITTER_30, 60, 5, |t, now| t.draw(now));
        assert!(t.total.late_runs > 0, "losses make late frames");
        assert_eq!(t.total.held_runs, 0, "single losses stay inside the bound");
        assert!(ratio(s.frozen, s.frames) < 0.05, "{s:?}");
    }
}
