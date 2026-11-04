//! View angles and mouse look with the 2009 client's defaults.
//!
//! The 2009 client's mouse path has two halves:
//! - CShell computes one axis scale for the mouse X and Y axes, `scale = sensitivity * mul + base`, and hands it
//!   to the engine's input layer with a console command (cshell+0x2a4ca0). Defaults: sensitivity 5 (the options
//!   value, an integer slider), `mul` 0.000225, `base` 0.00001, so 0.001135 per count at the default setting.
//! - CShell's view code adds the axis values it gets back to yaw and pitch, divided by `3 z + 1` for zoom level
//!   `z`, then clamps pitch (MOVEMENT_2009 §13.2).
//!
//! How the engine turns raw device counts into axis values with that scale is not read; this slice treats the
//! scale as radians per raw count ([`crate::todo::MOUSE_SCALE_IS_RADIANS_PER_COUNT`]).
// SPEC: MOVEMENT_2009.md §13.1 (ADAPTIVE_MOUSE 0, ADAPTIVE_MAX 0.4), §13.2 steps 1-6

/// Yaw and pitch in radians. Yaw turns about +Y (0 looks along +Z, positive toward +X); positive pitch looks
/// down. Same convention as `dc_sim::TickInput` and `dc_render::Camera::looking`.
// SPEC: MOVEMENT_2009.md §13.2 (controller+0x48 pitch, +0x4c yaw)
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ViewAngles {
    pub yaw: f32,
    pub pitch: f32,
}

/// Mouse-look parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookParams {
    /// The options-menu sensitivity value (integer slider in the client, kept as a float here).
    // EVIDENCE: cshell+0x21a90, cshell+0x112800 (read with default 5.0 = bits 0x40a00000, stored as an integer),
    // cshell+0x1ec900 (options profile default 5), key id 3c39cb62ad3cb2c5/16
    pub sensitivity: f32,
    /// Scale per sensitivity step.
    // DISASM: cshell+0x2a50dc (registration of handle VA 0x375b3178 with default bits 0x396bedfa = 0.000225,
    // key id 994058314cae1a83/19); EVIDENCE: cshell+0x2a4ca0 (scale = value * sensitivity + base)
    pub scale_mul: f32,
    /// Scale at sensitivity 0.
    // DISASM: cshell+0x2a50b5 (registration of handle VA 0x375b316c with default bits 0x3727c5ac = 0.00001,
    // key id 2493c1bd78dd8e67/14)
    pub scale_base: f32,
    /// Mouse look on (pitch follows the mouse). Default on.
    // EVIDENCE: cshell+0x21a90 (mouse-look variable read with default 1); SPEC: MOVEMENT_2009.md §13.2 step 5
    pub mouse_look: bool,
    /// Invert Y. Default off.
    // EVIDENCE: cshell+0x21a90 (invert variable read with default 0); SPEC: MOVEMENT_2009.md §13.2 step 5
    pub invert_y: bool,
    /// Integer zoom level `z`; mouse deltas are divided by `3 z + 1`.
    // SPEC: MOVEMENT_2009.md §13.2 step 2 (s = 1 / (3 z + 1), z = controller+0xf0)
    pub zoom_level: u32,
    /// Small-motion curve on / off and its threshold.
    // SPEC: MOVEMENT_2009.md §13.1 (ADAPTIVE_MOUSE 0, ADAPTIVE_MAX 0.4), §13.2 step 3
    pub adaptive: bool,
    pub adaptive_max: f32,
    /// Pitch limit, radians (`pi / 2 - 0.1`).
    // SPEC: MOVEMENT_2009.md §13.2 step 6 (always |pitch| <= 1.4707963)
    pub pitch_limit: f32,
    /// Largest mouse motion per frame and axis, in raw counts; larger (or non-finite) deltas are clamped (or
    /// dropped) before the sensitivity is applied. 0 switches the clamp off.
    // DC-DEFAULT: CLI-10 (proposed) +-4096 counts per frame, yaw kept wrapped; config key client.mouse_delta_clamp
    // OBSERVED: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §5 (one ~2^31-count delta pushed the 2009 client's unwrapped
    // yaw to 2.4e6 rad, where binary32 steps are 0.25 rad, and froze horizontal look for the rest of the round)
    pub max_delta_counts: f64,
}

impl LookParams {
    pub const DEFAULT: LookParams = LookParams {
        sensitivity: 5.0,
        scale_mul: 0.000225,
        scale_base: 0.00001,
        mouse_look: true,
        invert_y: false,
        zoom_level: 0,
        adaptive: false,
        adaptive_max: 0.4,
        pitch_limit: 1.470_796_3,
        max_delta_counts: MOUSE_DELTA_CLAMP,
    };

    /// The axis scale CShell hands to the engine: `sensitivity * scale_mul + scale_base`.
    // EVIDENCE: cshell+0x2a4ca0 (product of the two variables plus the base, formatted into the scale command);
    // DISASM: cshell+0x2a4e68..0x2a4e8e (fld / fmul sensitivity / fadd base)
    pub fn axis_scale(&self) -> f32 {
        self.sensitivity * self.scale_mul + self.scale_base
    }

    /// One axis of one frame's raw motion after the CLI-10 clamp: non-finite motion is dropped, the rest is kept
    /// within `+-max_delta_counts` (no clamp when that is 0 or less).
    // DC-DEFAULT: CLI-10 (proposed) per-frame delta clamp
    pub fn clamp_delta(&self, d: f64) -> f64 {
        if !d.is_finite() {
            return 0.0;
        }
        let m = self.max_delta_counts;
        if m > 0.0 && m.is_finite() {
            d.clamp(-m, m)
        } else {
            d
        }
    }

    /// Applies one frame of raw mouse motion (counts, +x right, +y toward the user) to `view`. The motion is
    /// clamped per axis first and the yaw is wrapped before the step is added (CLI-10), so one corrupt delta turns
    /// the view by at most `max_delta_counts * axis_scale()` and later small motions still turn it.
    // SPEC: MOVEMENT_2009.md §13.2 steps 2-6; DC-DEFAULT: CLI-10 (proposed), CLI-8
    pub fn apply(&self, view: &mut ViewAngles, dx: f64, dy: f64) {
        let (dx, dy) = (self.clamp_delta(dx), self.clamp_delta(dy));
        // keep the accumulator small before adding the step: at |yaw| ~ 2^21 a binary32 step is 0.25 rad
        view.yaw = wrap_angle(view.yaw);
        if !view.pitch.is_finite() {
            view.pitch = 0.0;
        }
        let k = self.axis_scale();
        let s = 1.0 / (3.0 * self.zoom_level as f32 + 1.0);
        let mut yaw_d = dx as f32 * k * s;
        let pitch_d = dy as f32 * k * s;
        if self.adaptive && yaw_d.abs() < self.adaptive_max && self.adaptive_max > 0.0 {
            let x = if crate::todo::ADAPTIVE_ARG_IS_RATIO {
                yaw_d.abs() / self.adaptive_max
            } else {
                yaw_d.abs()
            };
            yaw_d = yaw_d.signum() * adaptive_curve(x) * self.adaptive_max;
        }
        view.yaw = wrap_angle(view.yaw + yaw_d);
        if self.mouse_look {
            if self.invert_y {
                view.pitch -= pitch_d;
            } else {
                view.pitch += pitch_d;
            }
        }
        view.pitch = view.pitch.clamp(-self.pitch_limit, self.pitch_limit);
    }
}

/// Default per-frame mouse delta clamp in raw counts (CLI-10): 4096 counts are 4.6 rad at the default scale, far
/// more than a hand moves a mouse in one frame, and far less than the 2^31 glitch of capture tdm2.
// DC-DEFAULT: CLI-10 (proposed) clamp 4096
pub const MOUSE_DELTA_CLAMP: f64 = 4096.0;

impl Default for LookParams {
    fn default() -> Self {
        LookParams::DEFAULT
    }
}

/// `f(x) = (1 - sin(pi x + pi / 2)) / 2` below 0.5, else `x`.
// SPEC: MOVEMENT_2009.md §13.2 step 3 (cshell+0x19f050)
pub fn adaptive_curve(x: f32) -> f32 {
    if x < 0.5 {
        (1.0 - (std::f32::consts::PI * x + std::f32::consts::FRAC_PI_2).sin()) * 0.5
    } else {
        x
    }
}

/// Keeps yaw in [-pi, pi) so it never loses precision (the client accumulates it unbounded; a DeCombat choice
/// that only changes the representation, not the direction).
pub fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let w = (a + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI;
    if w.is_finite() {
        w
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scale_is_sensitivity_times_mul_plus_base() {
        let p = LookParams::DEFAULT;
        assert!((p.axis_scale() - 0.001_135).abs() < 1e-9);
        // the defaults round-trip to the binary32 constants of the client
        assert_eq!(p.scale_mul.to_bits(), 0x396b_edfa);
        assert_eq!(p.scale_base.to_bits(), 0x3727_c5ac);
        assert_eq!(p.sensitivity.to_bits(), 0x40a0_0000);
        assert_eq!(p.pitch_limit, std::f32::consts::FRAC_PI_2 - 0.1);
    }

    #[test]
    fn mouse_turns_right_and_looks_down_and_clamps() {
        let p = LookParams::DEFAULT;
        let mut v = ViewAngles::default();
        p.apply(&mut v, 100.0, 0.0);
        assert!((v.yaw - 0.1135).abs() < 1e-6, "{v:?}");
        p.apply(&mut v, 0.0, 100.0);
        assert!(v.pitch > 0.0, "mouse toward the user looks down");
        p.apply(&mut v, 0.0, 1e6);
        assert_eq!(v.pitch, p.pitch_limit);
        p.apply(&mut v, 0.0, -1e7);
        assert_eq!(v.pitch, -p.pitch_limit);
        let inv = LookParams {
            invert_y: true,
            ..p
        };
        let mut w = ViewAngles::default();
        inv.apply(&mut w, 0.0, 100.0);
        assert!(w.pitch < 0.0);
        let no_look = LookParams {
            mouse_look: false,
            ..p
        };
        let mut n = ViewAngles::default();
        no_look.apply(&mut n, 0.0, 100.0);
        assert_eq!(n.pitch, 0.0);
    }

    #[test]
    fn zoom_divides_and_yaw_wraps() {
        let p = LookParams {
            zoom_level: 1,
            ..LookParams::DEFAULT
        };
        let mut v = ViewAngles::default();
        p.apply(&mut v, 100.0, 0.0);
        assert!((v.yaw - 0.1135 / 4.0).abs() < 1e-6);
        let mut w = ViewAngles {
            yaw: 3.1,
            pitch: 0.0,
        };
        LookParams::DEFAULT.apply(&mut w, 100.0, 0.0);
        assert!(w.yaw < -3.0 && w.yaw >= -std::f32::consts::PI, "{w:?}");
        assert_eq!(wrap_angle(f32::NAN), 0.0);
    }

    #[test]
    fn a_huge_mouse_delta_is_clamped_and_yaw_keeps_turning() {
        // CLI-10: the tdm2 glitch was one delta of about 2^31 counts on both axes in one frame
        let p = LookParams::DEFAULT;
        assert_eq!(p.max_delta_counts, 4096.0);
        let mut v = ViewAngles {
            yaw: -1.659_809,
            pitch: 0.0,
        };
        p.apply(&mut v, 2_147_483_647.0, 2_147_483_647.0);
        assert!(v.yaw.abs() <= std::f32::consts::PI, "{v:?}");
        // the step was at most 4096 counts: the expected yaw is the wrapped start plus 4096 counts
        let expect = wrap_angle(-1.659_809 + 4096.0 * p.axis_scale());
        assert!((v.yaw - expect).abs() < 1e-4, "{v:?} vs {expect}");
        assert_eq!(v.pitch, p.pitch_limit, "pitch still clamps");
        // after the glitch a small motion still turns the view (the 2009 client could not: 0.25 rad steps)
        let before = v.yaw;
        p.apply(&mut v, 10.0, 0.0);
        assert!(
            (wrap_angle(v.yaw - before) - 10.0 * p.axis_scale()).abs() < 1e-5,
            "{before} -> {}",
            v.yaw
        );
        // negative and non-finite deltas
        let mut w = ViewAngles::default();
        p.apply(&mut w, -1e12, 0.0);
        assert!(
            (w.yaw - wrap_angle(-4096.0 * p.axis_scale())).abs() < 1e-4,
            "{w:?}"
        );
        let mut n = ViewAngles::default();
        p.apply(&mut n, f64::NAN, f64::INFINITY);
        assert_eq!(n, ViewAngles::default(), "non-finite motion is dropped");
        assert_eq!(p.clamp_delta(-5000.0), -4096.0);
        assert_eq!(p.clamp_delta(123.0), 123.0);
        // the clamp can be switched off
        let off = LookParams {
            max_delta_counts: 0.0,
            ..p
        };
        assert_eq!(off.clamp_delta(1e9), 1e9);
    }

    #[test]
    fn an_unwrapped_yaw_is_wrapped_before_the_step() {
        // a yaw already at the tdm2 value (2.4e6 rad, binary32 spacing 0.25) is folded first, so the step counts
        let p = LookParams::DEFAULT;
        let mut v = ViewAngles {
            yaw: 2_426_654.0,
            pitch: 0.2,
        };
        let folded = wrap_angle(2_426_654.0);
        p.apply(&mut v, 20.0, 0.0);
        assert!(
            (v.yaw - wrap_angle(folded + 20.0 * p.axis_scale())).abs() < 1e-5,
            "{v:?}"
        );
        assert!(v.yaw.abs() <= std::f32::consts::PI);
        // a non-finite view is repaired, not propagated
        let mut bad = ViewAngles {
            yaw: f32::INFINITY,
            pitch: f32::NAN,
        };
        p.apply(&mut bad, 1.0, 1.0);
        assert!(bad.yaw.is_finite() && bad.pitch.is_finite(), "{bad:?}");
    }

    #[test]
    fn keyboard_mouse_clamps_the_frame_sum() {
        // two events in one frame are summed first and the sum is clamped once
        use crate::input::{DeviceInput, InputSource, KeyboardMouse};
        let mut k = KeyboardMouse::new(LookParams::DEFAULT);
        let mut v = ViewAngles::default();
        k.device(DeviceInput::MouseDelta(3000.0, 0.0));
        k.device(DeviceInput::MouseDelta(3000.0, 0.0));
        k.frame(&mut v);
        assert!((v.yaw - wrap_angle(4096.0 * LookParams::DEFAULT.axis_scale())).abs() < 1e-4);
    }

    #[test]
    fn adaptive_curve_shape() {
        assert_eq!(adaptive_curve(0.0), 0.0);
        assert!(
            (adaptive_curve(0.25) - (1.0 - (0.75f32 * std::f32::consts::PI).sin()) * 0.5).abs()
                < 1e-6
        );
        assert_eq!(adaptive_curve(0.7), 0.7);
        let p = LookParams {
            adaptive: true,
            ..LookParams::DEFAULT
        };
        let mut v = ViewAngles::default();
        // a small motion is damped by the curve
        p.apply(&mut v, 10.0, 0.0);
        assert!(v.yaw > 0.0 && v.yaw < 10.0 * p.axis_scale());
    }
}
