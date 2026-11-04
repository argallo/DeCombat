//! The sky pass: the sky camera (its eye placed inside a sky view box by the main eye's position inside the
//! world box), and the screen rectangle the sky portals cover.
//!
//! Evidence: RENDER_2009.md §4.4 and §5.3, plus the decompiler output of the two routines named there:
//! - engine+0x193ea0 builds the sky camera from the main camera matrix (camera context +0x3c) with its
//!   translation replaced by the context's second position (+0x35c..+0x364), near 0.01 (immediate 0x3c23d70a),
//!   far `sky_far_z`, and a per-axis scale of `sky_scale`; it draws the sky objects (engine+0x1b4d50), restores
//!   the main camera and clears depth only;
//! - engine+0xfb8c0 (camera builder, tail) computes that second position: `t = (eye - world_min) /
//!   (world_max - world_min)` per axis for a scene of mode 1 (0.5 otherwise), with the world box read at +0x20 /
//!   +0x2c of the shared world-BSP interface (the same offsets the world loader stores the world extents at,
//!   FMT_DAT85.md §4 items 3-4), then `sky_eye = box_min + (box_max - box_min) * t` with the box read from
//!   scene description +0x60..+0x74: floats 6..11 of the 12-float block at +0x48, which is the 48-byte block of
//!   `S2C_SKY_TABLE` (UDP_SESSION_2009_A.md §4.8: the client copies it to client state +0x9cc; RENDER_2009.md
//!   §3.3: scene +0x48 is copied from client state +0x9cc).

use crate::camera::Camera;
use glam::{Mat4, Vec3, Vec4};

/// Near plane of the sky camera.
// EVIDENCE: engine+0x193ea0 (immediate 0x3c23d70a = 0.01); SPEC: RENDER_2009.md §4.4
pub const SKY_NEAR: f32 = 0.01;
/// Defaults of `sky_far_z` and `sky_scale`.
// SPEC: RENDER_2009.md §4.4, §13 (sky_far_z 10000.0, sky_scale 1.0)
pub const SKY_FAR_Z: f32 = 10000.0;
pub const SKY_SCALE: f32 = 1.0;
/// The sky pass runs when the portal bounds are wider and taller than this (pixels).
// SPEC: RENDER_2009.md §5.1 step 3 (bounds wider and taller than 0.9 pixel)
pub const SKY_MIN_EXTENT: f32 = 0.9;

/// The 48-byte block of `S2C_SKY_TABLE` read as 12 little-endian floats. Floats 6..11 are the sky view box
/// (min, max) the sky eye is placed in (EVIDENCE engine+0xfb8c0, see the module notes); floats 0..5 are not read
/// by the client's camera code (the 2009 host initialises them to -10 and +10 per axis and the second triple
/// pair to 0: engine+0x9c5e0; meaning UNPROVEN).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyBlock {
    pub todo_first: [f32; 6],
    pub view_min: [f32; 3],
    pub view_max: [f32; 3],
}

impl SkyBlock {
    pub fn from_bytes(b: &[u8; 48]) -> SkyBlock {
        let f = |i: usize| f32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        SkyBlock {
            todo_first: [f(0), f(1), f(2), f(3), f(4), f(5)],
            view_min: [f(6), f(7), f(8)],
            view_max: [f(9), f(10), f(11)],
        }
    }

    pub fn to_bytes(&self) -> [u8; 48] {
        let mut out = [0u8; 48];
        let all = self
            .todo_first
            .iter()
            .chain(&self.view_min)
            .chain(&self.view_max);
        for (i, v) in all.enumerate() {
            out[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }
}

/// Settings of the sky pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkySettings {
    /// The sky view box (floats 6..11 of the sky block). `None`: DeCombat default, a box of zero size at the
    /// centre of the sky geometry (no sky parallax), used when no server sky block is at hand (the viewer).
    pub view_box: Option<([f32; 3], [f32; 3])>,
    /// `sky_scale`, applied to the sky geometry around the sky eye (DeCombat reading of the per-axis scale the
    /// camera builder gets; only the default 1.0 is exercised).
    pub scale: f32,
    /// `sky_far_z`.
    pub far_z: f32,
    /// Scene of mode 1 (the game scene): the eye moves the sky eye; otherwise the box centre is used.
    pub mode1: bool,
}

impl Default for SkySettings {
    fn default() -> Self {
        SkySettings {
            view_box: None,
            scale: SKY_SCALE,
            far_z: SKY_FAR_Z,
            mode1: true,
        }
    }
}

impl SkySettings {
    /// Settings from a server sky block.
    pub fn from_block(b: &SkyBlock) -> SkySettings {
        SkySettings {
            view_box: Some((b.view_min, b.view_max)),
            ..SkySettings::default()
        }
    }
}

/// The sky eye: the main eye's relative position inside the world box, applied to the sky view box.
// EVIDENCE: engine+0xfb8c0 tail (t per axis; 0.5 for scenes not of mode 1)
pub fn sky_eye(
    eye: Vec3,
    world: Option<([f32; 3], [f32; 3])>,
    view_box: ([f32; 3], [f32; 3]),
    mode1: bool,
) -> Vec3 {
    let (vmin, vmax) = (Vec3::from(view_box.0), Vec3::from(view_box.1));
    let t = match (mode1, world) {
        (true, Some((lo, hi))) => {
            let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
            let span = hi - lo;
            let f = |e: f32, l: f32, s: f32| if s.abs() > 1e-6 { (e - l) / s } else { 0.5 };
            Vec3::new(
                f(eye.x, lo.x, span.x),
                f(eye.y, lo.y, span.y),
                f(eye.z, lo.z, span.z),
            )
        }
        _ => Vec3::splat(0.5),
    };
    vmin + (vmax - vmin) * t
}

/// The sky camera of one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyView {
    pub eye: Vec3,
    pub view_projection: Mat4,
}

/// Builds the sky camera: the main camera's rotation and fields of view, the sky eye, near [`SKY_NEAR`], far
/// `far_z`, the sky geometry scaled by `scale` around the eye.
// SPEC: RENDER_2009.md §4.4; EVIDENCE: engine+0x193ea0, engine+0xfb8c0
pub fn sky_camera(
    cam: &Camera,
    world_extents: Option<([f32; 3], [f32; 3])>,
    sky_bounds: Option<([f32; 3], [f32; 3])>,
    s: &SkySettings,
) -> SkyView {
    let view_box = s.view_box.unwrap_or_else(|| {
        let c = sky_bounds
            .map(|(lo, hi)| (Vec3::from(lo) + Vec3::from(hi)) * 0.5)
            .unwrap_or(Vec3::ZERO)
            .to_array();
        (c, c)
    });
    let eye = sky_eye(cam.pos, world_extents, view_box, s.mode1);
    let n = SKY_NEAR;
    let f = if s.far_z.is_finite() && s.far_z > n * 2.0 {
        s.far_z
    } else {
        SKY_FAR_Z
    };
    let tx = (cam.fov_x * 0.5).tan();
    let ty = (cam.fov_y * 0.5).tan();
    let a = f / (f - n);
    // SPEC: RENDER_2009.md §4.1 step 3 (the same projection form), with the sky near / far of §4.4
    let proj = Mat4::from_cols(
        Vec4::new(1.0 / tx, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0 / ty, 0.0, 0.0),
        Vec4::new(0.0, 0.0, a, 1.0),
        Vec4::new(0.0, 0.0, -n * a, 0.0),
    );
    let scale = if s.scale.is_finite() && s.scale > 0.0 {
        s.scale
    } else {
        1.0
    };
    let view = Mat4::from_quat(cam.rot).transpose()
        * Mat4::from_scale(Vec3::splat(scale))
        * Mat4::from_translation(-eye);
    SkyView {
        eye,
        view_projection: proj * view,
    }
}

/// Clips a clip-space polygon to `z >= 0` (in front of the near plane; the projection maps the near plane to
/// z = 0, RENDER_2009.md §4.1 step 3).
fn clip_near(poly: &[Vec4]) -> Vec<Vec4> {
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let (ia, ib) = (a.z >= 0.0, b.z >= 0.0);
        if ia {
            out.push(a);
        }
        if ia != ib {
            let t = a.z / (a.z - b.z);
            out.push(a + (b - a) * t);
        }
    }
    out
}

/// Screen rectangle `[x0, y0, x1, y1]` (pixels, x1 > x0, y1 > y0) covered by the parts of the sky portals in
/// front of the camera, clamped to the target; `None` when it is not wider and taller than 0.9 pixel.
///
/// The engine accumulates the bounds of the portals of the visible render blocks (engine+0x17cdc0); without the
/// block visibility walk every portal in front of the camera counts here (DeCombat: a larger rectangle can only
/// draw sky where the world covers it afterwards).
// SPEC: RENDER_2009.md §5.1 step 3, §5.3
pub fn portal_screen_rect(
    view_projection: Mat4,
    portals: &[Vec<[f32; 3]>],
    width: u32,
    height: u32,
) -> Option<[u32; 4]> {
    let (w, h) = (width as f32, height as f32);
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    let mut any = false;
    for p in portals {
        if p.len() < 3 {
            continue;
        }
        let clip: Vec<Vec4> = p
            .iter()
            .map(|v| view_projection * Vec3::from(*v).extend(1.0))
            .collect();
        for c in clip_near(&clip) {
            if c.w <= 1e-6 {
                continue;
            }
            let x = (c.x / c.w * 0.5 + 0.5) * w;
            let y = (0.5 - 0.5 * c.y / c.w) * h;
            let s = Vec3::new(x, y, 0.0);
            lo = lo.min(s);
            hi = hi.max(s);
            any = true;
        }
    }
    if !any {
        return None;
    }
    let x0 = lo.x.clamp(0.0, w);
    let x1 = hi.x.clamp(0.0, w);
    let y0 = lo.y.clamp(0.0, h);
    let y1 = hi.y.clamp(0.0, h);
    if x1 - x0 <= SKY_MIN_EXTENT || y1 - y0 <= SKY_MIN_EXTENT {
        return None;
    }
    let r = [
        x0.floor() as u32,
        y0.floor() as u32,
        (x1.ceil() as u32).min(width),
        (y1.ceil() as u32).min(height),
    ];
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eye_follows_the_world_box() {
        let world = Some(([0.0, 0.0, 0.0], [100.0, 10.0, 200.0]));
        let e = sky_eye(
            Vec3::new(25.0, 5.0, 200.0),
            world,
            ([1000.0, 0.0, 0.0], [1100.0, 10.0, 10.0]),
            true,
        );
        assert!(e.abs_diff_eq(Vec3::new(1025.0, 5.0, 10.0), 1e-4), "{e}");
        // not a mode-1 scene: the box centre
        let e = sky_eye(Vec3::ZERO, world, ([0.0; 3], [10.0; 3]), false);
        assert_eq!(e, Vec3::splat(5.0));
        // a flat world axis: centre on that axis
        let e = sky_eye(
            Vec3::ZERO,
            Some(([0.0; 3], [0.0, 1.0, 1.0])),
            ([0.0; 3], [10.0; 3]),
            true,
        );
        assert_eq!(e.x, 5.0);
    }

    #[test]
    fn block_round_trip() {
        let b = SkyBlock {
            todo_first: [-10.0, -10.0, -10.0, 10.0, 10.0, 10.0],
            view_min: [1.0, 2.0, 3.0],
            view_max: [4.0, 5.0, 6.0],
        };
        assert_eq!(SkyBlock::from_bytes(&b.to_bytes()), b);
    }

    #[test]
    fn portal_rect_clips_behind_the_camera() {
        let cam = Camera::looking(Vec3::ZERO, 0.0, 0.0, 90f32.to_radians(), 90f32.to_radians());
        let vp = cam.view_projection();
        // a quad straight ahead covering the middle half of the screen
        let ahead = vec![
            [-50.0, -50.0, 100.0],
            [50.0, -50.0, 100.0],
            [50.0, 50.0, 100.0],
            [-50.0, 50.0, 100.0],
        ];
        assert_eq!(
            portal_screen_rect(vp, &[ahead], 100, 100),
            Some([25, 25, 75, 75])
        );
        // entirely behind: nothing
        let behind = vec![[-5.0, -5.0, -10.0], [5.0, -5.0, -10.0], [0.0, 5.0, -10.0]];
        assert_eq!(portal_screen_rect(vp, &[behind], 100, 100), None);
        // crossing the near plane: clipped, clamped to the screen
        let crossing = vec![
            [-1.0, 0.0, -10.0],
            [1.0, 0.0, -10.0],
            [1.0, 50.0, 100.0],
            [-1.0, 50.0, 100.0],
        ];
        let r = portal_screen_rect(vp, &[crossing], 100, 100).unwrap();
        assert!(r[1] == 0 && r[3] <= 100, "{r:?}");
    }

    #[test]
    fn sky_camera_keeps_rotation_and_moves_the_eye() {
        let cam = Camera::looking(Vec3::new(50.0, 0.0, 0.0), 0.3, 0.0, 1.5, 1.2);
        let s = SkySettings::default();
        let v = sky_camera(
            &cam,
            Some(([0.0; 3], [100.0; 3])),
            Some(([1000.0; 3], [1200.0; 3])),
            &s,
        );
        assert!(v.eye.abs_diff_eq(Vec3::splat(1100.0), 1e-3));
        // a point ahead of the sky eye along the camera's forward lands at the screen centre
        let p = v.eye + cam.forward() * 500.0;
        let c = v.view_projection * p.extend(1.0);
        assert!((c.x / c.w).abs() < 1e-4 && (c.y / c.w).abs() < 1e-4);
        let z = c.z / c.w;
        assert!(z > 0.0 && z < 1.0);
    }
}
