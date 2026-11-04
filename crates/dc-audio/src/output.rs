//! The output side of the mixer without a device: the null sink of headless runs, the stereo-to-device channel
//! mapping, and the callback statistics (period, latency, late callbacks, lock waits, clipping) that the device
//! layer keeps and prints, so a window run can be judged from its log. Everything here is plain code and tested
//! without opening a device; `device.rs` (feature `device`) is the cpal glue around it.
// SPEC: AUDIO_2009.md §2 (44.1 kHz drivers, OS speaker layout), §12 item 1 (device), item 5 (clip at the output)

use crate::mixer::Mixer;

/// Renders the mixer into a scratch buffer nobody hears, by the time that passed: voices advance and finish as
/// they would on a device. Used by headless runs and whenever no device is open.
#[derive(Debug, Default)]
pub struct NullSink {
    scratch: Vec<f32>,
    last_ms: Option<u32>,
    /// Remainder below one frame of the elapsed time times the rate, carried so no time is lost at high frame rates.
    carry: u64,
    /// Frames rendered so far.
    pub frames: u64,
}

/// The null sink renders at most this much time per call (a long hitch does not render minutes of audio).
pub const NULL_MAX_STEP_MS: u32 = 1000;

impl NullSink {
    /// Renders the time since the last call (`now_ms` is the caller's millisecond clock).
    pub fn pump(&mut self, mixer: &mut Mixer, now_ms: u32) {
        let elapsed = self
            .last_ms
            .map_or(0, |l| now_ms.saturating_sub(l))
            .min(NULL_MAX_STEP_MS);
        self.last_ms = Some(now_ms);
        let total = self.carry + u64::from(elapsed) * u64::from(mixer.output_rate());
        let frames = (total / 1000) as usize;
        self.carry = total % 1000;
        if frames > 0 {
            self.scratch.resize(frames * 2, 0.0);
            mixer.render(&mut self.scratch, frames);
            self.frames += frames as u64;
        }
    }
}

/// Writes the mixer's interleaved stereo `lr` into a device buffer of `channels` channels: mono gets the mean of
/// both sides, stereo L / R, more channels L / R on the first two and silence on the rest (the 2009 drivers use
/// the OS speaker layout; the engine never sends more than a stereo mix, SPEC §2). `gain` is a linear output gain.
pub fn spread_stereo<T: Copy>(
    lr: &[f32],
    out: &mut [T],
    channels: usize,
    gain: f32,
    conv: impl Fn(f32) -> T,
) {
    let channels = channels.max(1);
    let silence = conv(0.0);
    for (f, frame) in out.chunks_mut(channels).enumerate() {
        let (l, r) = match lr.get(2 * f..2 * f + 2) {
            Some(s) => (s[0] * gain, s[1] * gain),
            None => (0.0, 0.0),
        };
        if channels == 1 {
            frame[0] = conv((0.5 * (l + r)).clamp(-1.0, 1.0));
            continue;
        }
        for (c, o) in frame.iter_mut().enumerate() {
            *o = match c {
                0 => conv(l.clamp(-1.0, 1.0)),
                1 => conv(r.clamp(-1.0, 1.0)),
                _ => silence,
            };
        }
    }
}

/// f32 to signed 16-bit (round to nearest, clamped).
pub fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}
/// f32 to unsigned 16-bit (offset binary).
pub fn f32_to_u16(s: f32) -> u16 {
    (i32::from(f32_to_i16(s)) + 32768) as u16
}

/// A callback is counted late (a probable underrun: the device queue ran dry) when the time since the previous
/// callback exceeds this many periods plus [`LATE_SLACK_S`]. Two periods: a shared-mode stream queues about two.
// DC-DEFAULT: CLI-95 (proposed) late-callback threshold 2 periods + 2 ms
pub const LATE_PERIODS: f64 = 2.0;
pub const LATE_SLACK_S: f64 = 0.002;
/// A wait for the mixer lock longer than this is counted (the game thread held the mixer during a callback).
pub const LOCK_WAIT_COUNT_S: f64 = 0.001;

/// Statistics of the device callbacks. Fed by the callback, read by the game thread for the log line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputStats {
    /// Device rate (Hz).
    pub rate: u32,
    pub callbacks: u64,
    pub frames: u64,
    pub min_frames: u32,
    pub max_frames: u32,
    /// Seconds from the stream open to the first callback.
    pub first_callback_s: Option<f64>,
    /// Largest time between two callbacks.
    pub max_gap_s: f64,
    /// Callbacks after a gap above [`LATE_PERIODS`] periods + slack (probable underruns).
    pub late: u64,
    /// Latency the host reported for the last callback (time until the written block plays) and its maximum.
    pub latency_s: Option<f64>,
    pub max_latency_s: f64,
    /// Waits for the mixer lock: the longest, and how many exceeded [`LOCK_WAIT_COUNT_S`].
    pub max_lock_wait_s: f64,
    pub lock_waits: u64,
    /// Callbacks that wrote silence because the mixer lock was poisoned.
    pub silent_callbacks: u64,
    /// Output samples at full scale (the mixer clips at [-1, 1]) and the largest absolute sample.
    pub clipped: u64,
    pub peak: f32,
    /// Stream errors the host reported.
    pub errors: u64,
    pub last_error: Option<String>,
    last_t: Option<f64>,
    last_frames: u32,
}

impl OutputStats {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            ..Self::default()
        }
    }

    /// One callback at `t_s` seconds after the open that wrote `frames` frames.
    pub fn on_callback(&mut self, t_s: f64, frames: u32, latency_s: Option<f64>, lock_wait_s: f64) {
        if self.callbacks == 0 {
            self.first_callback_s = Some(t_s);
            self.min_frames = frames;
        }
        if let Some(prev) = self.last_t {
            let gap = (t_s - prev).max(0.0);
            self.max_gap_s = self.max_gap_s.max(gap);
            let period = f64::from(self.last_frames) / f64::from(self.rate.max(1));
            if period > 0.0 && gap > LATE_PERIODS * period + LATE_SLACK_S {
                self.late += 1;
            }
        }
        self.last_t = Some(t_s);
        self.last_frames = frames;
        self.callbacks += 1;
        self.frames += u64::from(frames);
        self.min_frames = self.min_frames.min(frames);
        self.max_frames = self.max_frames.max(frames);
        if let Some(l) = latency_s {
            self.latency_s = Some(l);
            self.max_latency_s = self.max_latency_s.max(l);
        }
        self.max_lock_wait_s = self.max_lock_wait_s.max(lock_wait_s);
        if lock_wait_s > LOCK_WAIT_COUNT_S {
            self.lock_waits += 1;
        }
    }

    /// Level facts of the stereo block a callback wrote.
    pub fn on_block(&mut self, lr: &[f32]) {
        for s in lr {
            let a = s.abs();
            if a > self.peak {
                self.peak = a;
            }
            if a >= 1.0 {
                self.clipped += 1;
            }
        }
    }

    pub fn on_error(&mut self, e: String) {
        self.errors += 1;
        self.last_error = Some(e);
    }

    /// Seconds of audio written.
    pub fn seconds(&self) -> f64 {
        self.frames as f64 / f64::from(self.rate.max(1))
    }

    /// One log line: `audio out: <label> | ...`.
    pub fn line(&self, label: &str) -> String {
        let ms = |s: f64| s * 1000.0;
        let period = |f: u32| ms(f64::from(f) / f64::from(self.rate.max(1)));
        format!(
            "audio out: {label} | callbacks {} audio {:.2} s period {:.1}..{:.1} ms first {} | latency {} (max {:.1} ms) | max gap {:.1} ms late {} | lock wait max {:.2} ms over-1ms {} silent {} | mix peak {:.3} clipped {} | errors {}{}",
            self.callbacks,
            self.seconds(),
            period(self.min_frames),
            period(self.max_frames),
            self.first_callback_s
                .map_or("-".into(), |s| format!("{:.1} ms", ms(s))),
            self.latency_s.map_or("-".into(), |s| format!("{:.1} ms", ms(s))),
            ms(self.max_latency_s),
            ms(self.max_gap_s),
            self.late,
            ms(self.max_lock_wait_s),
            self.lock_waits,
            self.silent_callbacks,
            self.peak,
            self.clipped,
            self.errors,
            self.last_error
                .as_deref()
                .map_or(String::new(), |e| format!(" (last: {e})")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Pcm;
    use crate::mixer::PlayRequest;
    use std::sync::Arc;

    #[test]
    fn null_sink_renders_the_elapsed_time_without_losing_fractions() {
        let mut m = Mixer::new();
        let mut n = NullSink::default();
        n.pump(&mut m, 0);
        // 1 ms steps at 44.1 kHz: 44.1 frames each; after 1000 of them exactly 44100 frames
        for t in 1..=1000 {
            n.pump(&mut m, t);
        }
        assert_eq!(n.frames, 44_100);
        // a 5 s hitch renders at most one second
        n.pump(&mut m, 6000);
        assert_eq!(n.frames, 2 * 44_100);
        // at the device rate of 48 kHz
        m.set_output_rate(48_000);
        n.pump(&mut m, 6010);
        assert_eq!(n.frames, 2 * 44_100 + 480);
    }

    #[test]
    fn null_sink_finishes_a_voice_in_its_duration_at_any_output_rate() {
        for rate in [44_100, 48_000, 96_000] {
            let mut m = Mixer::new();
            m.set_output_rate(rate);
            // 0.5 s at 22.05 kHz
            let pcm = Arc::new(Pcm {
                rate: 22_050,
                channels: 1,
                samples: vec![0.1; 11_025],
            });
            let id = m.play(PlayRequest::local(pcm)).unwrap();
            let mut n = NullSink::default();
            n.pump(&mut m, 0);
            n.pump(&mut m, 490);
            m.update(0.49);
            assert!(!m.is_finished(id), "{rate}: still playing at 0.49 s");
            n.pump(&mut m, 510);
            m.update(0.51);
            assert!(m.is_finished(id), "{rate}: finished at 0.51 s");
        }
    }

    #[test]
    fn stereo_spreads_to_mono_stereo_and_surround() {
        let lr = [0.5, -0.5, 0.25, 0.75];
        let mut mono = [0.0f32; 2];
        spread_stereo(&lr, &mut mono, 1, 1.0, |s| s);
        assert_eq!(mono, [0.0, 0.5]);
        let mut st = [0.0f32; 4];
        spread_stereo(&lr, &mut st, 2, 0.5, |s| s);
        assert_eq!(st, [0.25, -0.25, 0.125, 0.375]);
        let mut six = [9.0f32; 12];
        spread_stereo(&lr, &mut six, 6, 1.0, |s| s);
        assert_eq!(&six[..6], &[0.5, -0.5, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(&six[6..8], &[0.25, 0.75]);
        let mut i = [0i16; 4];
        spread_stereo(&[1.5, -1.0, 0.0, 0.5], &mut i, 2, 1.0, f32_to_i16);
        assert_eq!(i, [32767, -32767, 0, 16384]);
        assert_eq!(f32_to_u16(0.0), 32768);
        assert_eq!(f32_to_u16(-1.0), 1);
    }

    #[test]
    fn late_callbacks_lock_waits_and_levels_are_counted() {
        let mut s = OutputStats::new(48_000);
        // 10 ms periods (480 frames) on time
        for k in 0..10 {
            s.on_callback(0.05 + k as f64 * 0.010, 480, Some(0.010), 0.0001);
        }
        assert_eq!((s.callbacks, s.late, s.lock_waits), (10, 0, 0));
        assert_eq!(s.first_callback_s, Some(0.05));
        // a 30 ms gap (> 2 periods + 2 ms): one late callback; a 2 ms lock wait
        s.on_callback(0.05 + 9.0 * 0.010 + 0.030, 480, Some(0.012), 0.002);
        assert_eq!((s.late, s.lock_waits), (1, 1));
        assert!((s.max_gap_s - 0.030).abs() < 1e-9);
        assert!((s.max_latency_s - 0.012).abs() < 1e-9);
        s.on_block(&[0.5, -1.0, 1.0, 0.25]);
        assert_eq!((s.clipped, s.peak), (2, 1.0));
        s.on_error("device lost".into());
        let line = s.line("test 48000 Hz 2 ch");
        assert!(line.starts_with("audio out: test 48000 Hz 2 ch | callbacks 11"));
        assert!(line.contains("late 1"));
        assert!(line.contains("errors 1 (last: device lost)"));
        assert!((s.seconds() - 11.0 * 0.010).abs() < 1e-9);
    }
}
