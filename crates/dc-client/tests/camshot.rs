//! CLI-240 headless: while the source reports a live world camera the frame is drawn from it (the eye and the view
//! follow the camera object's path frame by frame, its field of view replaces the client's) and a camera whose
//! movement byte is 0 holds the body (the player keeps pressing forward and does not move); the own view and the
//! movement return the frame the camera goes off. No ROM, no window; rendering is off.
// DISASM: cshell+0x1f83d0 (the live camera drives the view), cshell+0x1f8502 / cshell+0x2001c0 (movement byte 0
// zeroes the body's velocity), cshell+0x1f2ca0 (the own view returns)

use dc_client::camshot::CamShotView;
use dc_client::testkit::synth_source;
use dc_client::world::{FrameState, GpuTarget, LoadedWorld};
use dc_client::{run_headless, ClientError, HeadlessOptions, InputSource, ViewAngles, WorldSource};
use dc_sim::Commands;

const SHOT: std::ops::Range<u32> = 30..90;

/// The camera's path: a dolly along +X with a slow turn.
fn path(k: u32) -> ([f32; 3], f32) {
    (
        [100.0 + 2.0 * k as f32, 200.0, -50.0],
        0.5 + 0.01 * k as f32,
    )
}

struct Scripted {
    inner: dc_client::world::SynthSource,
    frame: u32,
}

impl WorldSource for Scripted {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError> {
        self.inner.load(gpu)
    }
    fn frame_state(&mut self) -> FrameState {
        let f = self.frame;
        self.frame += 1;
        let mut fs = FrameState {
            life_state: Some(1),
            ..FrameState::default()
        };
        if SHOT.contains(&f) {
            let (pos, yaw) = path(f - SHOT.start);
            let h = yaw * 0.5;
            fs.cam_shot = Some(CamShotView {
                obj: 300,
                pos,
                rot: Some([0.0, h.sin(), 0.0, h.cos()]),
                allow_move: false,
                listener: true,
                fov: [70.0, 55.0],
            });
        }
        fs
    }
}

/// Always forward.
struct Forward;

impl InputSource for Forward {
    fn frame(&mut self, _view: &mut ViewAngles) {}
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands {
            forward: true,
            ..Commands::default()
        }
    }
    fn label(&self) -> &'static str {
        "forward"
    }
}

#[test]
fn world_camera_follows_its_path_and_holds_the_body() {
    let opts = HeadlessOptions {
        frames: 140,
        frame_us: 16_667,
        width: 64,
        height: 48,
        out: None,
        render: false,
        trace: true,
        ..HeadlessOptions::default()
    };
    let mut src = Scripted {
        inner: synth_source(),
        frame: 0,
    };
    let mut input = Forward;
    let r = run_headless(&mut src, Some(&mut input as &mut dyn InputSource), &opts)
        .expect("headless run");
    assert_eq!(r.cam_shot_frames, SHOT.end - SHOT.start);
    assert_eq!(r.cam_shot_marks.len(), (SHOT.end - SHOT.start) as usize);
    for (i, (f, eye, yaw, pitch, held)) in r.cam_shot_marks.iter().enumerate() {
        assert_eq!(
            *f,
            SHOT.start + i as u32,
            "the shot frames are the camera's frames"
        );
        let (pos, want_yaw) = path(i as u32);
        assert_eq!(*eye, pos, "frame {f}: the eye is the camera's position");
        assert!(
            (yaw - want_yaw).abs() < 1e-4,
            "frame {f}: yaw {yaw} vs {want_yaw}"
        );
        assert!(pitch.abs() < 1e-4);
        assert!(*held, "frame {f}: the movement byte 0 holds the body");
    }
    // the body: it walks before the shot, stands while the camera is live (after the first tick of the shot), walks
    // again after it (one tick per frame at 60 Hz)
    let at = |tick: u64| {
        r.trace
            .iter()
            .find(|t| t.tick == tick)
            .map(|t| t.pos)
            .expect("traced tick")
    };
    let moved = |a: u64, b: u64| (at(b) - at(a)).len_xz();
    let (s, e) = (SHOT.start as u64, SHOT.end as u64);
    assert!(moved(5, s - 1) > 20.0, "walks before: {}", moved(5, s - 1));
    assert!(
        moved(s + 2, e - 1) < 0.5,
        "held during the shot: {}",
        moved(s + 2, e - 1)
    );
    assert!(
        moved(e + 2, 138) > 20.0,
        "walks after: {}",
        moved(e + 2, 138)
    );
}

/// CLI-610: a letterbox camera (blob kind byte 1) runs the bars in over the 2009 fade time (1 s) to
/// `height * (1 - 0.66) / 2` each, measured on the session clock: 30 fps and 250 fps give the same heights at the same
/// clock times; after the camera they run out over 1 s and vanish. Fallback HUD (no ROM), GPU target.
// DISASM: cshell+0x1b6120 (fade clock, bar height), cshell+0x1f50b0 (kind byte 1 -> letterbox)
#[test]
fn letterbox_bars_grow_in_over_one_second_at_any_frame_rate() {
    struct Clocked {
        inner: dc_client::world::SynthSource,
        frame: u32,
        frame_ms: u32,
    }
    impl WorldSource for Clocked {
        fn describe(&self) -> String {
            self.inner.describe()
        }
        fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError> {
            self.inner.load(gpu)
        }
        fn frame_state(&mut self) -> FrameState {
            let t = self.frame * self.frame_ms;
            self.frame += 1;
            let mut fs = FrameState {
                life_state: Some(1),
                clock_ms: Some(t),
                ..FrameState::default()
            };
            // the end camera is live from 1.0 s to 3.0 s of the clock
            if (1000..3000).contains(&t) {
                fs.cam_shot = Some(CamShotView {
                    obj: 300,
                    pos: [0.0, 100.0, 0.0],
                    rot: None,
                    allow_move: false,
                    listener: true,
                    fov: [70.0, 55.0],
                });
            }
            fs
        }
    }
    let mut curves = Vec::new();
    for frame_ms in [33u32, 4] {
        let opts = HeadlessOptions {
            frames: 4500 / frame_ms,
            frame_us: frame_ms as u64 * 1000,
            width: 640,
            height: 480,
            out: None,
            render: true,
            hud: dc_client::hud::HudChoice::Fallback,
            ..HeadlessOptions::default()
        };
        let mut src = Clocked {
            inner: synth_source(),
            frame: 0,
            frame_ms,
        };
        let r = match run_headless(&mut src, None, &opts) {
            Ok(r) => r,
            Err(e) => {
                println!("skipped: no GPU target ({e})");
                return;
            }
        };
        if r.adapter.is_none() {
            println!("no GPU adapter: the HUD checks are skipped");
            return;
        }
        let marks = r.letterbox_marks;
        assert!(!marks.is_empty(), "the HUD ran");
        // bar height at clock time t: the last change at or before t
        let at = |t: u32| marks.iter().rev().find(|m| m.1 <= t).map_or(0, |m| m.2);
        let full = (480.0f32 * (1.0 - 0.66) * 0.5) as u32; // 81
        assert_eq!(
            at(990),
            0,
            "{frame_ms} ms frames: no bars before the camera"
        );
        let mid = at(1500) as i32;
        assert!(
            (mid - full as i32 / 2).abs() <= 3,
            "{frame_ms} ms: half way at 0.5 s: {mid}"
        );
        assert_eq!(at(2100), full, "{frame_ms} ms: full after the 1 s fade");
        assert_eq!(at(2900), full);
        let out = at(3500) as i32;
        assert!(
            (out - full as i32 / 2).abs() <= 3,
            "{frame_ms} ms: half way out: {out}"
        );
        assert_eq!(at(4200), 0, "{frame_ms} ms: gone 1 s after the camera");
        // every frame's bar is the clock's curve from the first frame with (without) the camera: 1 s in, 1 s out,
        // whatever the frame rate
        let first = |ms: u32| ms.div_ceil(frame_ms) * frame_ms;
        let (on, off) = (first(1000), first(3000));
        for &(f, t, px) in &marks {
            let p = if t < on {
                0.0
            } else if t < off {
                ((t - on) as f32 / 1000.0).min(1.0)
            } else {
                (1.0 - (t - off) as f32 / 1000.0).max(0.0)
            };
            let want = (480.0 * p * (1.0 - 0.66f32) * 0.5) as i32;
            assert!(
                (px as i32 - want).abs() <= 1,
                "{frame_ms} ms frames: frame {f} at {t} ms: {px} px, the curve gives {want}"
            );
        }
        curves.push(marks.len());
    }
    assert!(
        curves[1] > curves[0],
        "the faster run samples more heights: {curves:?}"
    );
}
