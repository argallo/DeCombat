//! Per-frame pieces shared by the headless and the window loop: the camera from the player's eye, the frame
//! rate meter and the debug overlay text.

use crate::look::ViewAngles;
use crate::player::LocalPlayer;
use dc_render::{fov_pair_2009, Camera};
use glam::Vec3;
use std::collections::VecDeque;

/// View settings of the client.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewSettings {
    /// Vertical field of view in degrees (the game's `FovY` variable, default 75). The horizontal one follows from
    /// the aspect ratio of the target: the vertical angle stays and the picture widens.
    // EVIDENCE: cshell+0x1f3cb0 (FovX = 2 atan(width / height * tan(FovY / 2))), cshell+0x1f6fb0 (default FovY 75);
    // SPEC: MOVEMENT_2009.md §13.5, RENDER_2009.md §3.3 / §4.1; DC-DEFAULT: CLI-212 (see DC_DEFAULTS.md)
    pub fov_y_deg: f32,
    /// Far plane (world units).
    // SPEC: RENDER_2009.md §4.1 (far_z default 12000)
    pub far_z: f32,
}

impl Default for ViewSettings {
    fn default() -> Self {
        ViewSettings {
            fov_y_deg: dc_render::FOV_Y_DEFAULT_DEG,
            far_z: dc_render::DEFAULT_FAR_Z,
        }
    }
}

impl ViewSettings {
    /// Settings whose horizontal field is `fov_x_deg` degrees at a 4:3 target (the `--fov` option); a value of
    /// zero or less keeps the 2009 default.
    // DC-DEFAULT: CLI-212 (see DC_DEFAULTS.md)
    pub fn with_reference_fov_x(fov_x_deg: f32) -> ViewSettings {
        let mut v = ViewSettings::default();
        if fov_x_deg > 0.0 {
            v.fov_y_deg = dc_render::fov_y_deg_for_reference_x(fov_x_deg);
        }
        v
    }

    /// The render camera at `eye` looking along `view`, for a `w x h` target.
    // EVIDENCE: cshell+0x1f3cb0 (the horizontal field is derived from the vertical one and the target aspect)
    pub fn camera(&self, eye: dc_sim::V3, view: &ViewAngles, w: u32, h: u32) -> Camera {
        let (fov_x, fov_y) = fov_pair_2009(self.fov_y_deg, w, h);
        let mut c = Camera::looking(
            Vec3::new(eye.x, eye.y, eye.z),
            view.yaw,
            view.pitch,
            fov_x,
            fov_y,
        );
        c.far_z = self.far_z;
        c
    }
}

/// Frame rate over the last frames (a sliding window of frame times).
#[derive(Debug, Clone, Default)]
pub struct FpsMeter {
    times: VecDeque<f64>,
    sum: f64,
    pub frames: u64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub total_s: f64,
}

impl FpsMeter {
    const WINDOW: usize = 120;

    /// Records one frame of `dt` seconds.
    pub fn push(&mut self, dt: f64) {
        if self.frames == 0 {
            self.min_ms = f64::INFINITY;
        }
        self.frames += 1;
        self.total_s += dt;
        let ms = dt * 1000.0;
        self.min_ms = self.min_ms.min(ms);
        self.max_ms = self.max_ms.max(ms);
        self.times.push_back(dt);
        self.sum += dt;
        if self.times.len() > Self::WINDOW {
            self.sum -= self.times.pop_front().unwrap_or(0.0);
        }
    }

    /// Frames per second over the window.
    pub fn fps(&self) -> f64 {
        if self.sum > 0.0 {
            self.times.len() as f64 / self.sum
        } else {
            0.0
        }
    }

    /// Frames per second over the whole run.
    pub fn mean_fps(&self) -> f64 {
        if self.total_s > 0.0 {
            self.frames as f64 / self.total_s
        } else {
            0.0
        }
    }
}

/// Applies a server view set (shell 0x6a): kind 0 sets pitch and yaw (roll is not simulated), kind 1 yaw only.
/// Pitch is kept inside the mouse-look limit.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.2 (kind 0: pitch, yaw, roll := angles; kind 1: yaw only); MOVEMENT_2009.md
// §12.5, §13.2 step 6 (pitch limit)
pub fn apply_view_set(view: &mut ViewAngles, kind: u8, angles: [f32; 3]) {
    if !angles.iter().all(|a| a.is_finite()) {
        return;
    }
    if kind != 1 {
        let lim = crate::look::LookParams::DEFAULT.pitch_limit;
        view.pitch = angles[0].clamp(-lim, lim);
    }
    view.yaw = crate::look::wrap_angle(angles[1]);
}

/// The debug boxes for this frame's remote players (team colours, CLI-13) and the number skipped as non-finite.
pub fn remote_boxes(
    remotes: &[crate::world::RemoteBody],
) -> (Vec<crate::debug_draw::BoxInstance>, u32) {
    let mut bad = 0u32;
    let boxes = remotes
        .iter()
        .filter(|r| {
            let ok = r.pos.iter().chain(r.half.iter()).all(|v| v.is_finite());
            bad += (!ok) as u32;
            ok
        })
        .map(|r| crate::debug_draw::BoxInstance {
            centre: r.pos,
            half: r.half,
            colour: crate::net::team_colour(r.team),
        })
        .collect();
    (boxes, bad)
}

/// The debug overlay lines: position, velocity, frame rate, support state, tick.
pub fn overlay_lines(
    world: &str,
    player: &LocalPlayer,
    view: &ViewAngles,
    fps: f64,
    input: &str,
) -> Vec<String> {
    let s = &player.state;
    let p = player.render_pos();
    vec![
        format!("{world}  [{input}]"),
        format!("pos {:9.1} {:9.1} {:9.1}", p.x, p.y, p.z),
        format!(
            "vel {:9.1} {:9.1} {:9.1}  |xz| {:6.1}",
            s.vel.x,
            s.vel.y,
            s.vel.z,
            s.vel.len_xz()
        ),
        format!(
            "yaw {:7.1}  pitch {:6.1}  fps {:7.1}",
            view.yaw.to_degrees(),
            view.pitch.to_degrees(),
            fps
        ),
        format!(
            "support {}  ctl {:#07x}",
            player.support_label(),
            s.controls
        ),
        format!(
            "tick {} @ {} Hz  alpha {:.2}",
            player.ticks(),
            player.tick_hz(),
            player.alpha()
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_meter_window_and_extremes() {
        let mut m = FpsMeter::default();
        for _ in 0..10 {
            m.push(0.01);
        }
        assert!((m.fps() - 100.0).abs() < 1e-6);
        m.push(0.02);
        assert!((m.max_ms - 20.0).abs() < 1e-9 && (m.min_ms - 10.0).abs() < 1e-9);
        assert_eq!(m.frames, 11);
        for _ in 0..500 {
            m.push(0.005);
        }
        assert!((m.fps() - 200.0).abs() < 1e-3);
    }

    #[test]
    fn camera_looks_along_the_view() {
        let v = ViewSettings::default();
        let c = v.camera(
            dc_sim::V3::new(1.0, 2.0, 3.0),
            &ViewAngles {
                yaw: std::f32::consts::FRAC_PI_2,
                pitch: 0.0,
            },
            800,
            600,
        );
        assert!(c.forward().abs_diff_eq(Vec3::X, 1e-5));
        // 75 degrees vertical at 4:3 widens to 91.3 degrees horizontally
        assert!((c.fov_y - 75f32.to_radians()).abs() < 1e-6);
        assert!((c.fov_x.to_degrees() - 91.31).abs() < 0.02);
        assert!(c.fov_y < c.fov_x);
    }

    #[test]
    fn camera_keeps_the_vertical_field_and_widens_with_the_aspect() {
        let v = ViewSettings::default();
        let at = |w, h| v.camera(dc_sim::V3::new(0.0, 0.0, 0.0), &ViewAngles::default(), w, h);
        let (a43, a169, a1610, a219) = (
            at(1024, 768),
            at(2560, 1440),
            at(1920, 1200),
            at(3440, 1440),
        );
        for c in [&a43, &a169, &a1610, &a219] {
            assert!((c.fov_y - 75f32.to_radians()).abs() < 1e-6);
        }
        assert!(a43.fov_x < a1610.fov_x && a1610.fov_x < a169.fov_x && a169.fov_x < a219.fov_x);
        // the projection has square pixels: x scale / y scale = height / width
        for (c, w, h) in [
            (&a43, 1024.0, 768.0),
            (&a169, 2560.0, 1440.0),
            (&a219, 3440.0, 1440.0),
        ] {
            let p = c.projection();
            assert!((p.x_axis.x / p.y_axis.y - h / w).abs() < 1e-4);
        }
    }

    #[test]
    fn reference_fov_option_maps_to_the_vertical_field() {
        assert_eq!(
            ViewSettings::with_reference_fov_x(0.0),
            ViewSettings::default()
        );
        let v = ViewSettings::with_reference_fov_x(90.0);
        let c = v.camera(
            dc_sim::V3::new(0.0, 0.0, 0.0),
            &ViewAngles::default(),
            640,
            480,
        );
        assert!((c.fov_x.to_degrees() - 90.0).abs() < 0.01);
        let c = v.camera(
            dc_sim::V3::new(0.0, 0.0, 0.0),
            &ViewAngles::default(),
            1920,
            1080,
        );
        assert!((c.fov_x.to_degrees() - 106.26).abs() < 0.01);
    }
}
