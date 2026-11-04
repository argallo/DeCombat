//! Background music: a two-slot player with a linear crossfade.
// SPEC: AUDIO_2009.md §8.1 (play/update/stop rules), §4.3 (music master is separate, not handled here)
//
// The original streams MP3 from disk; here a slot plays an already decoded [`Pcm`] (decode with
// `decode::decode_mp3`). Time is passed in by the caller (seconds, any monotonic origin) so the fade
// timeline is deterministic.

use std::sync::Arc;

use crate::decode::Pcm;
use crate::OUTPUT_RATE;

#[derive(Clone, Debug)]
struct Slot {
    track: Arc<Pcm>,
    pos: f64,
    looped: bool,
    gain: f32,
    finished: bool,
}

impl Slot {
    fn new(track: Arc<Pcm>, looped: bool, gain: f32) -> Self {
        Self {
            track,
            pos: 0.0,
            looped,
            gain,
            finished: false,
        }
    }

    fn render_add(&mut self, out: &mut [f32], frames: usize, out_rate: u32) {
        if self.finished {
            return;
        }
        let n = self.track.frames();
        if n == 0 {
            self.finished = true;
            return;
        }
        let step = f64::from(self.track.rate) / f64::from(out_rate.max(1));
        for f in 0..frames {
            let mut i = self.pos as usize;
            if i >= n {
                if self.looped {
                    self.pos -= n as f64 * (self.pos / n as f64).floor();
                    i = self.pos as usize;
                } else {
                    self.finished = true;
                    return;
                }
            }
            let frac = (self.pos - i as f64) as f32;
            let j = if i + 1 < n {
                i + 1
            } else if self.looped {
                0
            } else {
                i
            };
            let (l0, r0) = self.track.frame_lr(i);
            let (l1, r1) = self.track.frame_lr(j);
            out[2 * f] += (l0 + (l1 - l0) * frac) * self.gain;
            out[2 * f + 1] += (r0 + (r1 - r0) * frac) * self.gain;
            self.pos += step;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Fade {
    start_s: f64,
    ms: u32,
}

/// The current stream as logs and checks see it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BgmStatus {
    pub looped: bool,
    /// Play position in seconds of the track (a looped track wraps).
    pub pos_s: f64,
    /// The track's length in seconds.
    pub dur_s: f64,
    pub gain: f32,
    /// A crossfade is running (a `next` stream exists).
    pub crossfading: bool,
}

/// What `play` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayOutcome {
    /// Same key already playing (case-insensitive): nothing restarted.
    Ignored,
    /// New stream opened directly as the current one.
    Started,
    /// New stream opened in the `next` slot and crossfading in.
    Crossfading,
}

#[derive(Clone, Debug, Default)]
pub struct BgmPlayer {
    cur: Option<Slot>,
    next: Option<Slot>,
    fade: Option<Fade>,
    path: Option<String>,
}

impl BgmPlayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while the current stream exists and has not run out.
    pub fn is_playing(&self) -> bool {
        self.cur.as_ref().map(|s| !s.finished).unwrap_or(false)
    }

    /// The stream [`BgmPlayer::current_key`] names while it plays: the incoming one during a crossfade, else the
    /// current one (`None`: stopped or run out).
    pub fn status(&self) -> Option<BgmStatus> {
        let c = match &self.next {
            Some(n) if !n.finished => n,
            _ => self.cur.as_ref().filter(|s| !s.finished)?,
        };
        let rate = f64::from(c.track.rate.max(1));
        Some(BgmStatus {
            looped: c.looped,
            pos_s: c.pos / rate,
            dur_s: c.track.frames() as f64 / rate,
            gain: c.gain,
            crossfading: self.next.is_some(),
        })
    }

    pub fn current_key(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// `(current gain, next gain)`; for tests and UI.
    pub fn gains(&self) -> (Option<f32>, Option<f32>) {
        (
            self.cur.as_ref().map(|s| s.gain),
            self.next.as_ref().map(|s| s.gain),
        )
    }

    /// `play(path, loop, fade_seconds)`. `key` is the resource path used for the de-duplication rule.
    /// `fade_ms = trunc(fade_s * 1000)`; loop true = forever, false = once.
    /// Steps: // SPEC: AUDIO_2009.md §8.1 `play`
    pub fn play(
        &mut self,
        key: &str,
        track: Arc<Pcm>,
        looped: bool,
        fade_s: f32,
        now_s: f64,
    ) -> PlayOutcome {
        let fade_ms = (fade_s * 1000.0) as i64;
        let fade_ms = fade_ms.max(0) as u32;
        if self.is_playing() {
            if let Some(p) = &self.path {
                if dc_bute::eq_fold(p, key) {
                    return PlayOutcome::Ignored;
                }
            }
        }
        let initial = if fade_ms == 0 { 1.0 } else { 0.0 };

        let outcome = if !self.is_playing() || fade_ms == 0 {
            self.next = None;
            self.cur = Some(Slot::new(track, looped, initial));
            PlayOutcome::Started
        } else {
            self.next = Some(Slot::new(track, looped, initial));
            PlayOutcome::Crossfading
        };
        self.fade = if fade_ms > 0 {
            Some(Fade {
                start_s: now_s,
                ms: fade_ms,
            })
        } else {
            None
        };
        self.path = Some(key.to_string());
        outcome
    }

    /// Hard stop of both slots, path cleared (no fade-out). // SPEC: AUDIO_2009.md §8.1 `stop`
    pub fn stop(&mut self) {
        self.cur = None;
        self.next = None;
        self.fade = None;
        self.path = None;
    }

    /// Pause and resume are empty in the original. // SPEC: AUDIO_2009.md §8.1
    pub fn pause(&mut self) {}
    pub fn resume(&mut self) {}

    /// Per-update fade step. // SPEC: AUDIO_2009.md §8.1 "Per update"
    pub fn update(&mut self, now_s: f64) {
        let Some(f) = self.fade else {
            // no window: a fade-in that never got a window cannot exist; a stale <= 0.99 gain is snapped
            if let Some(c) = &mut self.cur {
                if self.next.is_none() && c.gain <= 0.99 {
                    c.gain = 1.0;
                }
            }
            return;
        };
        let t = ((now_s - f.start_s) * 1000.0 / f64::from(f.ms)) as f32;
        if (0.0..1.0).contains(&t) {
            if let Some(n) = &mut self.next {
                if let Some(c) = &mut self.cur {
                    c.gain = (1.0 - t).clamp(0.0, 1.0);
                }
                n.gain = t.clamp(0.0, 1.0);
            } else if let Some(c) = &mut self.cur {
                c.gain = t.clamp(0.0, 1.0);
            }
        } else if t >= 1.0 {
            if let Some(mut n) = self.next.take() {
                n.gain = 1.0;
                self.cur = Some(n);
            } else if let Some(c) = &mut self.cur {
                if c.gain <= 0.99 {
                    c.gain = 1.0;
                }
            }
            self.fade = None;
        }
    }

    /// Adds `frames` stereo frames of music (before the music master) into `out` (interleaved L/R) at the
    /// 44.1 kHz rate of the 2009 drivers.
    pub fn render_add(&mut self, out: &mut [f32], frames: usize) {
        self.render_add_at(out, frames, OUTPUT_RATE);
    }

    /// As [`BgmPlayer::render_add`] at an output rate of `out_rate` Hz (the device's own rate): each track is
    /// converted from its file rate straight to the output rate.
    pub fn render_add_at(&mut self, out: &mut [f32], frames: usize, out_rate: u32) {
        if let Some(c) = &mut self.cur {
            c.render_add(out, frames, out_rate);
        }
        if let Some(n) = &mut self.next {
            n.render_add(out, frames, out_rate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(len: usize) -> Arc<Pcm> {
        Arc::new(Pcm {
            rate: OUTPUT_RATE,
            channels: 1,
            samples: vec![0.5; len],
        })
    }

    #[test]
    fn crossfade_timeline() {
        let mut b = BgmPlayer::new();
        assert_eq!(
            b.play("A.mp3", tone(1000), true, 0.0, 0.0),
            PlayOutcome::Started
        );
        b.update(0.0);
        assert_eq!(b.gains(), (Some(1.0), None));
        assert_eq!(
            b.play("B.mp3", tone(1000), true, 2.0, 10.0),
            PlayOutcome::Crossfading
        );
        b.update(10.5); // t = 0.25
        let (c, n) = b.gains();
        assert!((c.unwrap() - 0.75).abs() < 1e-5 && (n.unwrap() - 0.25).abs() < 1e-5);
        b.update(11.0); // t = 0.5
        let (c, n) = b.gains();
        assert!((c.unwrap() - 0.5).abs() < 1e-5 && (n.unwrap() - 0.5).abs() < 1e-5);
        b.update(12.0); // window over: next replaces current at 1.0
        assert_eq!(b.gains(), (Some(1.0), None));
        assert_eq!(b.current_key(), Some("B.mp3"));
    }

    #[test]
    fn fade_in_from_silence_and_dedupe() {
        let mut b = BgmPlayer::new();
        assert_eq!(
            b.play("a.mp3", tone(1000), true, 1.0, 0.0),
            PlayOutcome::Started
        );
        assert_eq!(b.gains().0, Some(0.0));
        b.update(0.4);
        assert!((b.gains().0.unwrap() - 0.4).abs() < 1e-5);
        // same path (different case): ignored, no restart
        assert_eq!(
            b.play("A.MP3", tone(10), false, 0.0, 0.5),
            PlayOutcome::Ignored
        );
        b.update(1.5);
        assert_eq!(b.gains().0, Some(1.0));
        b.stop();
        assert!(!b.is_playing() && b.current_key().is_none());
    }

    #[test]
    fn zero_fade_replaces_immediately_and_drops_next() {
        let mut b = BgmPlayer::new();
        b.play("a", tone(100), true, 0.0, 0.0);
        b.play("b", tone(100), true, 1.0, 1.0);
        assert!(b.gains().1.is_some());
        assert_eq!(b.play("c", tone(100), true, 0.0, 1.1), PlayOutcome::Started);
        assert_eq!(b.gains(), (Some(1.0), None));
    }

    #[test]
    fn status_reports_position_length_and_loop() {
        let mut b = BgmPlayer::new();
        assert_eq!(b.status(), None);
        let n = OUTPUT_RATE as usize; // one second, mono
        b.play("a", tone(n), false, 0.0, 0.0);
        let st = b.status().unwrap();
        assert!(!st.looped && st.pos_s == 0.0 && (st.dur_s - 1.0).abs() < 1e-9);
        let mut buf = vec![0.0; 2 * n / 2];
        b.render_add(&mut buf, n / 2);
        assert!((b.status().unwrap().pos_s - 0.5).abs() < 1e-3);
        let mut buf = vec![0.0; 2 * n];
        b.render_add(&mut buf, n);
        assert_eq!(b.status(), None, "a once track that ran out has no status");
        b.play("b", tone(n), true, 0.0, 0.0);
        b.render_add(&mut buf, n);
        b.render_add(&mut buf, n);
        assert!(b.status().unwrap().looped);
        // during a crossfade the incoming track is reported
        b.play("c", tone(n / 2), false, 1.0, 0.0);
        let st = b.status().unwrap();
        assert!(st.crossfading && !st.looped && (st.dur_s - 0.5).abs() < 1e-9);
    }

    #[test]
    fn once_finishes_loop_wraps() {
        let mut once = BgmPlayer::new();
        once.play("a", tone(10), false, 0.0, 0.0);
        let mut buf = vec![0.0; 40];
        once.render_add(&mut buf, 20);
        assert!(!once.is_playing());
        assert_eq!(buf[0], 0.5);
        assert_eq!(buf[39], 0.0);
        let mut lp = BgmPlayer::new();
        lp.play("a", tone(10), true, 0.0, 0.0);
        let mut buf = vec![0.0; 40];
        lp.render_add(&mut buf, 20);
        assert!(lp.is_playing());
        assert_eq!(buf[39], 0.5);
    }

    #[test]
    fn music_rate_is_resampled() {
        let mut b = BgmPlayer::new();
        let t = Arc::new(Pcm {
            rate: 22050,
            channels: 1,
            samples: vec![0.25; 100],
        });
        b.play("a", t, false, 0.0, 0.0);
        let mut buf = vec![0.0; 2 * 150];
        b.render_add(&mut buf, 150); // 100 source frames last ~200 output frames
        assert!(b.is_playing());
        assert!((buf[2 * 100] - 0.25).abs() < 1e-5);
    }
}
