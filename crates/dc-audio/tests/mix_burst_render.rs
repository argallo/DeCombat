//! Offline render of many simultaneous gunshot-like bursts (synthesized: decaying noise and a low thump; no sound
//! file is read) through the mixer with the output limiter (CLI-1960) off and on. Prints a table of clipped samples
//! and peak for N = 1, 4, 8, 16 shooters, and the limiter's cost per buffer. Run with `--nocapture` to see it.
// DC-DEFAULT: CLI-1960 (proposed), see DC_DEFAULTS.md

use std::sync::Arc;
use std::time::Instant;

use dc_audio::decode::Pcm;
use dc_audio::limiter::{gain_of_db, Limiter, DEFAULT_CEILING_DB};
use dc_audio::mixer::{Listener, Mixer, PlayRequest};
use dc_audio::OUTPUT_RATE;

/// One synthesized shot: 600 ms of noise with a 120 ms decay over a 60 Hz thump with a 150 ms decay, peak about 1.
fn shot(seed: u32) -> Arc<Pcm> {
    let n = (OUTPUT_RATE as f32 * 0.6) as usize;
    let mut x = seed.max(1);
    let r = OUTPUT_RATE as f32;
    let samples = (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let t = i as f32 / r;
            let noise = (x as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let thump = (t * 60.0 * std::f32::consts::TAU).sin();
            0.7 * noise * (-t / 0.120).exp() + 0.5 * thump * (-t / 0.150).exp()
        })
        .collect();
    Arc::new(Pcm {
        rate: OUTPUT_RATE,
        channels: 1,
        samples,
    })
}

#[derive(Debug, Default)]
struct Result {
    clipped: u64,
    peak: f32,
    clamped_by_mixer: u64,
    limited_frames: u64,
    max_reduction_db: f32,
}

/// `n` shooters, each firing 15 rounds at 600 a minute (one every 100 ms, start offsets on the 10 ms render grid, 30 ms apart modulo 100), the
/// first at the listener (the own gun: a local voice), the others between 100 and 900 units around it (full level inside 300).
fn render(n: usize, limiter: bool) -> Result {
    let mut m = Mixer::new();
    m.set_limiter_enabled(limiter);
    let meter = m.limiter_meter();
    m.set_camera(Some(Listener::default()));
    m.update(0.0);
    let block = 441; // 10 ms
    let total_blocks = 200; // 2 s
    let mut out = vec![0.0f32; 2 * block];
    let mut res = Result::default();
    let shots: Vec<Arc<Pcm>> = (0..4).map(|k| shot(17 + k)).collect();
    for b in 0..total_blocks {
        let t_ms = b * 10;
        for s in 0..n {
            let offset = (s * 30) % 100;
            if t_ms >= offset && t_ms < offset + 1500 && (t_ms - offset) % 100 == 0 {
                let pcm = shots[(s + t_ms / 100) % shots.len()].clone();
                let req = if s == 0 {
                    PlayRequest::local(pcm)
                } else {
                    let a = s as f32 * 2.4;
                    let d = 100.0 + (s as f32 * 90.0) % 800.0;
                    PlayRequest::positional(pcm, [a.sin() * d, 0.0, a.cos() * d], 300.0, 4000.0)
                };
                m.play(req).unwrap();
            }
        }
        m.render(&mut out, block);
        m.update(t_ms as f64 / 1000.0);
        for v in &out {
            let a = v.abs();
            res.peak = res.peak.max(a);
            if a >= 1.0 {
                res.clipped += 1;
            }
        }
        res.max_reduction_db = res.max_reduction_db.max(meter.reduction_db());
    }
    res.clamped_by_mixer = meter.clipped_samples();
    res.limited_frames = meter.limited_frames();
    res
}

#[test]
fn many_bursts_clip_without_the_limiter_and_never_with_it() {
    eprintln!("N  | off: clipped  peak (pre-clamp clips) | on: clipped  peak  limited frames  max reduction");
    let ceiling = gain_of_db(DEFAULT_CEILING_DB);
    let mut off_clips = Vec::new();
    for n in [1, 4, 8, 16] {
        let off = render(n, false);
        let on = render(n, true);
        eprintln!(
            "{n:<2} | {:>8} {:>7.3} ({:>8}) | {:>6} {:>6.3} {:>8} {:>8.1} dB",
            off.clipped,
            off.peak,
            off.clamped_by_mixer,
            on.clipped,
            on.peak,
            on.limited_frames,
            on.max_reduction_db
        );
        assert_eq!(
            on.clipped, 0,
            "N {n}: the limiter never lets a sample reach full scale"
        );
        assert_eq!(on.clamped_by_mixer, 0);
        assert!(on.peak <= ceiling + 1e-6, "N {n}: {}", on.peak);
        off_clips.push(off.clipped);
    }
    assert!(
        off_clips[3] > off_clips[0],
        "more guns clip more: {off_clips:?}"
    );
}

#[test]
fn limiter_cost_per_buffer() {
    // 512-frame stereo buffers of loud noise (the limiter busy on every frame); a debug build is far slower than a
    // release one: run `cargo test --release` for a figure worth quoting
    let mut l = Limiter::new(48_000);
    l.set_enabled(true);
    let mut x = 1u32;
    let src: Vec<f32> = (0..1024)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            ((x as f32 / u32::MAX as f32) * 2.0 - 1.0) * 3.0
        })
        .collect();
    let mut buf = src.clone();
    let rounds = 2000;
    let t = Instant::now();
    for _ in 0..rounds {
        buf.copy_from_slice(&src);
        l.process(&mut buf);
    }
    let per = t.elapsed().as_secs_f64() / f64::from(rounds);
    eprintln!(
        "limiter: {:.2} us per 512-frame buffer ({:.3} % of its 10.7 ms at 48 kHz)",
        per * 1e6,
        per / (512.0 / 48_000.0) * 100.0
    );
    assert!(buf.iter().all(|s| s.abs() < 1.0));
}
