//! Main camera: view and projection as the 2009 scene path builds them, in wgpu's [0, 1] depth convention.

use glam::{Mat4, Quat, Vec3, Vec4};

/// Near plane of the main camera (immediate operand in the scene setup).
// SPEC: RENDER_2009.md §4.1 (n = 7.0, engine+0xfd610)
pub const NEAR_Z: f32 = 7.0;

/// Default far plane and its clamp range.
// SPEC: RENDER_2009.md §4.1 (far_z default 12000.0, clamped to [3.0, 500000.0] by engine+0xfb8c0), §13
pub const DEFAULT_FAR_Z: f32 = 12000.0;
pub const FAR_Z_MIN: f32 = 3.0;
pub const FAR_Z_MAX: f32 = 500000.0;

/// Viewport depth range of the world and of every normal object; the player-view pass uses [0.0, 0.1].
// SPEC: RENDER_2009.md §4.3 (world and normal objects draw with viewport depth [0.1, 1.0])
pub const WORLD_DEPTH_RANGE: (f32, f32) = (0.1, 1.0);
pub const PLAYER_VIEW_DEPTH_RANGE: (f32, f32) = (0.0, 0.1);

/// Player-view ("really close") camera: the projection eye-attached objects (first-person weapon and hands) are
/// drawn with. VIEW is the identity for them (the game positions them in eye space, +X right, +Y up, +Z forward)
/// and the viewport depth range is [`PLAYER_VIEW_DEPTH_RANGE`], so they win the depth test against the world
/// without a depth clear.
// SPEC: RENDER_2009.md §4.2 (left-handed field-of-view perspective, fy = pv_fov_deg, zn = pv_near, zf = pv_far,
// aspect = width / height of the camera context; VIEW = identity), §4.3 ([0.0, 0.1])
// DISASM: engine+0x1b4870 (saves the viewport, sets the same rectangle with MinZ 0.0 and MaxZ = the f32 at
// VA 0x675b78 (0.1); D3DX field-of-view LH perspective with fy = f32 at VA 0x6d3a04 times the f32 at VA 0x676914
// (pi/180), aspect = f32 at VA 0x7e5a34 / f32 at VA 0x7e5a38, zn = f32 at VA 0x6d39d4, zf = f32 at VA 0x6d39ec;
// saves PROJECTION and VIEW (device slots 0xb4 = GetTransform 3 / 2), sets PROJECTION and an identity VIEW built
// from the f32 1.0 at VA 0x672988 (slot 0xb0 = SetTransform 3 / 2)); engine+0x1b46b0 restores viewport and both
// matrices. Default values: the console-variable records at VA 0x6d39c4.. hold 0.1, 7.0, 50.0 as both the stored
// default and the current value (ROM read, 2026-10-25).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerViewSettings {
    /// Vertical field of view in degrees (`pv_fov_deg`).
    pub fov_y_deg: f32,
    /// Near and far planes in eye-space units (`pv_near`, `pv_far`).
    pub near: f32,
    pub far: f32,
}

/// The engine's player-view defaults: 50 degrees, near 0.1, far 7.0.
// SPEC: RENDER_2009.md §4.2 (defaults 50.0, 0.1, 7.0); DISASM: engine+0x1b4870 (VA 0x6d3a04 / 0x6d39d4 / 0x6d39ec)
pub const PLAYER_VIEW_DEFAULT: PlayerViewSettings = PlayerViewSettings {
    fov_y_deg: 50.0,
    near: 0.1,
    far: 7.0,
};

impl Default for PlayerViewSettings {
    fn default() -> Self {
        PLAYER_VIEW_DEFAULT
    }
}

impl PlayerViewSettings {
    /// Projection for a target of `width x height` pixels (the camera context's aspect), with the public D3DX
    /// left-handed formula: `ys = 1 / tan(fy / 2)`, `xs = ys / aspect`, `z' = zf/(zf-zn) z - zn zf/(zf-zn)`,
    /// `w' = z`. Depth lands in [0, 1] before the viewport maps it into [`PLAYER_VIEW_DEPTH_RANGE`].
    // SPEC: RENDER_2009.md §4.2 (PUBLIC D3DX formula)
    pub fn projection(&self, width: u32, height: u32) -> Mat4 {
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let fy = self.fov_y_deg.clamp(1.0, 179.0).to_radians();
        let ys = 1.0 / (fy * 0.5).tan();
        let xs = ys / aspect;
        let n = self.near.max(1e-4);
        let f = self.far.max(n * 1.001);
        let a = f / (f - n);
        Mat4::from_cols(
            Vec4::new(xs, 0.0, 0.0, 0.0),
            Vec4::new(0.0, ys, 0.0, 0.0),
            Vec4::new(0.0, 0.0, a, 1.0),
            Vec4::new(0.0, 0.0, -n * a, 0.0),
        )
    }

    /// Window depth of an eye-space depth `z` after the projection and the viewport depth range `range`.
    pub fn window_depth(&self, z: f32, range: (f32, f32)) -> f32 {
        let c = self.projection(1, 1) * Vec4::new(0.0, 0.0, z, 1.0);
        range.0 + (range.1 - range.0) * (c.z / c.w)
    }
}

/// Scene camera inputs (scene description fields 0x90..0xb3).
// SPEC: RENDER_2009.md §3.3 (scene description: fov_x, fov_y in radians, position, rotation quaternion)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub pos: Vec3,
    pub rot: Quat,
    /// Horizontal field of view, radians.
    pub fov_x: f32,
    /// Vertical field of view, radians. Independent of `fov_x`: the engine never derives one from the other.
    pub fov_y: f32,
    pub far_z: f32,
    /// The part of the full view drawn into the target, in normalised device units (x0, y0, x1, y1; -1..1, y
    /// up): [`FULL_WINDOW`] for the whole view; a smaller window renders only that part of the frustum
    /// (an off-centre projection), e.g. a menu pane into a pane-sized target.
    pub window: [f32; 4],
}

/// The whole view ([`Camera::window`]).
pub const FULL_WINDOW: [f32; 4] = [-1.0, -1.0, 1.0, 1.0];

impl Camera {
    /// Camera at `pos` looking along yaw / pitch (radians). Axes are left-handed: +X right, +Y up, +Z forward;
    /// yaw turns about +Y (yaw 0 looks along +Z, positive yaw toward +X) and positive pitch looks down.
    ///
    /// The axis signs follow the engine's quaternion-to-matrix routine (engine+0x5f6d0: the standard form, so a
    /// rotation by `a` about +Y maps +Z to `(sin a, 0, cos a)`). The composition order of the euler routine
    /// (engine+0x5b6c0, FPU code decompiler did not decode) is UNPROVEN; yaw is applied after pitch here.
    pub fn looking(pos: Vec3, yaw: f32, pitch: f32, fov_x: f32, fov_y: f32) -> Camera {
        Camera {
            pos,
            rot: Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch),
            fov_x,
            fov_y,
            far_z: DEFAULT_FAR_Z,
            window: FULL_WINDOW,
        }
    }

    /// Camera from an object's euler property (three radians: about X, about Y, about Z), with the same
    /// UNPROVEN composition as [`Camera::looking`] plus roll applied first.
    // SPEC: FMT_DAT85.md §6 (property code 7: three radians, euler-to-quaternion engine+0x5b6c0)
    pub fn from_euler(pos: Vec3, euler: [f32; 3], fov_x: f32, fov_y: f32) -> Camera {
        Camera {
            pos,
            rot: Quat::from_rotation_y(euler[1])
                * Quat::from_rotation_x(euler[0])
                * Quat::from_rotation_z(euler[2]),
            fov_x,
            fov_y,
            far_z: DEFAULT_FAR_Z,
            window: FULL_WINDOW,
        }
    }

    /// Forward direction (+Z of the camera frame) in world space.
    pub fn forward(&self) -> Vec3 {
        self.rot * Vec3::Z
    }

    pub fn near(&self) -> f32 {
        NEAR_Z
    }

    /// Far plane after the engine's clamp.
    // SPEC: RENDER_2009.md §4.1 (clamp [3.0, 500000.0])
    pub fn far(&self) -> f32 {
        if self.far_z.is_nan() {
            return DEFAULT_FAR_Z;
        }
        self.far_z.clamp(FAR_Z_MIN, FAR_Z_MAX)
    }

    /// View: the rotation transposed times a translation by `-pos`.
    // SPEC: RENDER_2009.md §4.1 step 2 (rotation from q, transposed, times translation by -p)
    pub fn view(&self) -> Mat4 {
        Mat4::from_quat(self.rot).transpose() * Mat4::from_translation(-self.pos)
    }

    /// Projection: `x' = x / tx`, `y' = y / ty`, `z' = f/(f-n) z - n f/(f-n)`, `w' = z`, with `tx = tan(fov_x/2)`,
    /// `ty = tan(fov_y/2)`; no off-centre shear on the scene path. Depth lands in [0, 1], which wgpu shares with
    /// Direct3D, so no remapping is needed.
    // SPEC: RENDER_2009.md §4.1 steps 1 and 3
    pub fn projection(&self) -> Mat4 {
        let full = self.full_projection();
        let [x0, y0, x1, y1] = self.window;
        if self.window == FULL_WINDOW || x1 <= x0 || y1 <= y0 {
            return full;
        }
        // the window [x0, x1] x [y0, y1] of the full view stretched to the target: an off-centre frustum
        // PUBLIC: off-centre perspective projection (a sub-rectangle of the view volume mapped to clip space)
        let sx = 2.0 / (x1 - x0);
        let sy = 2.0 / (y1 - y0);
        let window = Mat4::from_cols(
            Vec4::new(sx, 0.0, 0.0, 0.0),
            Vec4::new(0.0, sy, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(-(x0 + x1) / (x1 - x0), -(y0 + y1) / (y1 - y0), 0.0, 1.0),
        );
        window * full
    }

    /// [`Camera::projection`] of the whole view (no window).
    fn full_projection(&self) -> Mat4 {
        let n = self.near();
        let f = self.far();
        let tx = (self.fov_x * 0.5).tan();
        let ty = (self.fov_y * 0.5).tan();
        let a = f / (f - n);
        Mat4::from_cols(
            Vec4::new(1.0 / tx, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0 / ty, 0.0, 0.0),
            Vec4::new(0.0, 0.0, a, 1.0),
            Vec4::new(0.0, 0.0, -n * a, 0.0),
        )
    }

    pub fn view_projection(&self) -> Mat4 {
        self.projection() * self.view()
    }

    /// World from eye space (the inverse of [`Camera::view`]): where an eye-attached object stands in the world.
    pub fn world_from_eye(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.rot, self.pos)
    }

    /// Window depth of a point at eye-space depth `z` (straight ahead) with the world viewport range.
    pub fn window_depth(&self, z: f32) -> f32 {
        let c = self.projection() * Vec4::new(0.0, 0.0, z, 1.0);
        WORLD_DEPTH_RANGE.0 + (WORLD_DEPTH_RANGE.1 - WORLD_DEPTH_RANGE.0) * (c.z / c.w)
    }

    /// The eight frustum corners in world space: near `(+-n tx, +-n ty, n)` then far `(+-f tx, +-f ty, f)`.
    // SPEC: RENDER_2009.md §4.1 step 5
    pub fn frustum_corners(&self) -> [Vec3; 8] {
        let tx = (self.fov_x * 0.5).tan();
        let ty = (self.fov_y * 0.5).tan();
        let inv = Mat4::from_translation(self.pos) * Mat4::from_quat(self.rot);
        let mut out = [Vec3::ZERO; 8];
        let mut i = 0;
        for d in [self.near(), self.far()] {
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                out[i] = inv.transform_point3(Vec3::new(sx * d * tx, sy * d * ty, d));
                i += 1;
            }
        }
        out
    }
}

/// The six clip planes of a view-projection matrix in wgpu's (Direct3D's) clip convention, `-w <= x, y <= w`,
/// `0 <= z <= w`: left, right, bottom, top, near, far. Each plane `(n, d)` is normalised (`|n| = 1`) and a point
/// `p` is on its inner side when `n . p + d >= 0`, so the value is a distance in world units. Engineering helper
/// for view-frustum culling (rows of the matrix added / subtracted: the public Gribb-Hartmann construction).
pub fn frustum_planes(view_projection: Mat4) -> [Vec4; 6] {
    let m = view_projection;
    let row = |i: usize| Vec4::new(m.x_axis[i], m.y_axis[i], m.z_axis[i], m.w_axis[i]);
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2].map(|p| {
        let l = p.truncate().length();
        if l > 0.0 && l.is_finite() {
            p / l
        } else {
            p
        }
    })
}

/// True when the box `lo..hi` lies entirely on the outer side of one of `planes` by more than `margin` world
/// units (then no point of it, and so no triangle inside it, can reach the view volume).
pub fn box_outside(planes: &[Vec4; 6], lo: Vec3, hi: Vec3, margin: f32) -> bool {
    let c = (lo + hi) * 0.5;
    let h = (hi - lo) * 0.5;
    planes.iter().any(|p| {
        let n = p.truncate();
        n.dot(c) + n.abs().dot(h) + p.w < -margin
    })
}

/// Vertical field of view that keeps square pixels for a horizontal one on a `w x h` target. The viewer picks
/// the pair this way; the engine itself takes both from the game (CShell side, not specified).
pub fn fov_y_for(fov_x: f32, w: u32, h: u32) -> f32 {
    2.0 * ((fov_x * 0.5).tan() * h as f32 / w.max(1) as f32).atan()
}

/// The game's default vertical field of view, degrees (console variable `FovY`, registered with the default 75).
// EVIDENCE: cshell+0x1f6fb0 (registers `FovY` with the immediate 0x42960000 = 75.0 and `FovX` with 0x42b40000 = 90.0);
// SPEC: MOVEMENT_2009.md §13.5 (FOV_X 90, FOV_Y 75 defaults)
pub const FOV_Y_DEFAULT_DEG: f32 = 75.0;

/// Horizontal field of view the 2009 game derives from its vertical one and the aspect ratio of the target: the
/// vertical angle is fixed (the `FovY` variable) and the horizontal one widens with the aspect,
/// `fov_x = 2 atan(aspect * tan(fov_y / 2))`, angles in radians, `aspect = width / height`. The game stores the
/// result back into its `FovX` variable and passes `(FovX, FovY)` to the engine unchanged.
// EVIDENCE: cshell+0x1f3cb0 (the camera reset: fptan of FovY * pi / 360, times width / height, fpatan with 1, scaled
// to degrees and written to the `FovX` variable by cshell+0x1a4c20; then the camera fov call with FovX and FovY in
// radians); the reset runs from cshell+0x1f6fb0 and from the routine at cshell+0x22fa50. OBSERVED 2026-10-26: the
// JUNK_FLEA spawn frame of the 2009 client at 1920x1080 correlates best with a horizontal field of 107.3 to 107.8
// degrees, this rule gives 107.5 for 75 degrees vertical (a fixed 90 degrees horizontal scores 0.27 on the same
// edge correlation, a 4:3-derived 106.26 scores 0.78, 107.5 scores 0.89).
pub fn fov_x_for(fov_y: f32, w: u32, h: u32) -> f32 {
    let aspect = w.max(1) as f32 / h.max(1) as f32;
    2.0 * (aspect * (fov_y * 0.5).tan()).atan()
}

/// The `(fov_x, fov_y)` pair (radians) the 2009 game hands to the engine for a `w x h` target and a vertical field
/// of `fov_y_deg` degrees (default [`FOV_Y_DEFAULT_DEG`]); see [`fov_x_for`].
// EVIDENCE: cshell+0x1f3cb0
pub fn fov_pair_2009(fov_y_deg: f32, w: u32, h: u32) -> (f32, f32) {
    let fov_y = fov_y_deg.clamp(1.0, 170.0).to_radians();
    (fov_x_for(fov_y, w, h).min(179f32.to_radians()), fov_y)
}

/// The vertical field (degrees) whose horizontal one is `fov_x_deg` at a 4:3 target: how a horizontal value given
/// on a command line maps to the 2009 rule, which is defined by its vertical angle.
// DC-DEFAULT: CLI-212 (see DC_DEFAULTS.md): the `--fov` option of dc-client keeps meaning "horizontal degrees at 4:3"
pub fn fov_y_deg_for_reference_x(fov_x_deg: f32) -> f32 {
    let x = fov_x_deg.clamp(1.0, 170.0).to_radians();
    (2.0 * ((x * 0.5).tan() * 0.75).atan()).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_maps_its_part_of_the_view_to_the_whole_target() {
        let full = Camera::looking(Vec3::ZERO, 0.0, 0.0, 1.0, 0.8);
        let mut part = full;
        part.window = [-1.0, 0.0, 0.0, 1.0]; // the top-left quarter
        let ndc = |c: &Camera, p: Vec3| {
            let v = c.view_projection() * p.extend(1.0);
            (v.x / v.w, v.y / v.w, v.z / v.w)
        };
        // a point at the quarter's centre in the full view lands at the target's centre
        let tx = (0.5f32).tan();
        let ty = (0.4f32).tan();
        let p = Vec3::new(-0.5 * tx * 100.0, 0.5 * ty * 100.0, 100.0);
        let (fx, fy, fz) = ndc(&full, p);
        assert!(close(fx, -0.5) && close(fy, 0.5));
        let (x, y, z) = ndc(&part, p);
        assert!(close(x, 0.0) && close(y, 0.0), "{x} {y}");
        assert!(close(z, fz), "depth unchanged");
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4 * (1.0 + a.abs().max(b.abs()))
    }

    #[test]
    fn frustum_planes_hold_the_corners_and_reject_outside_boxes() {
        let c = Camera::looking(
            Vec3::new(100.0, 50.0, -30.0),
            0.7,
            -0.2,
            90f32.to_radians(),
            70f32.to_radians(),
        );
        let planes = frustum_planes(c.view_projection());
        // every corner of the frustum lies on the inner side of every plane (within rounding)
        for p in c.frustum_corners() {
            for pl in &planes {
                assert!(
                    pl.truncate().dot(p) + pl.w > -0.05 * (1.0 + p.length() * 1e-3),
                    "{p} {pl}"
                );
            }
        }
        // a box around a point straight ahead is inside; boxes behind, beyond far and far to the side are out
        let f = c.forward();
        let at = |d: f32| c.pos + f * d;
        let e = Vec3::splat(5.0);
        assert!(!box_outside(&planes, at(100.0) - e, at(100.0) + e, 1.0));
        assert!(box_outside(&planes, at(-100.0) - e, at(-100.0) + e, 1.0));
        assert!(box_outside(&planes, at(20000.0) - e, at(20000.0) + e, 1.0));
        let side = c.rot * Vec3::X;
        let s = at(100.0) + side * 1000.0;
        assert!(box_outside(&planes, s - e, s + e, 1.0));
        // a huge box around the eye is never out
        assert!(!box_outside(
            &planes,
            c.pos - Vec3::splat(1e4),
            c.pos + Vec3::splat(1e4),
            1.0
        ));
    }

    #[test]
    fn near_and_far_map_to_zero_and_one() {
        let c = Camera::looking(Vec3::ZERO, 0.0, 0.0, 1.5, 1.2);
        let p = c.view_projection();
        let near = p * Vec4::new(0.0, 0.0, NEAR_Z, 1.0);
        let far = p * Vec4::new(0.0, 0.0, DEFAULT_FAR_Z, 1.0);
        assert!(close(near.z / near.w, 0.0));
        assert!(close(far.z / far.w, 1.0));
    }

    #[test]
    fn fov_edges_hit_ndc_edges() {
        let fx = 90f32.to_radians();
        let fy = fov_y_for(fx, 640, 480);
        let c = Camera::looking(Vec3::ZERO, 0.0, 0.0, fx, fy);
        let p = c.view_projection();
        let v = p * Vec4::new(100.0, 75.0, 100.0, 1.0);
        assert!(close(v.x / v.w, 1.0));
        assert!(close(v.y / v.w, 1.0));
    }

    #[test]
    fn yaw_and_pitch_axes() {
        let c = Camera::looking(Vec3::ZERO, std::f32::consts::FRAC_PI_2, 0.0, 1.0, 1.0);
        assert!(c.forward().abs_diff_eq(Vec3::X, 1e-5));
        let c = Camera::looking(Vec3::ZERO, 0.0, 0.3, 1.0, 1.0);
        assert!(c.forward().y < 0.0, "positive pitch looks down");
        // a point straight ahead stays centred after translation
        let c = Camera::looking(Vec3::new(10.0, 20.0, 30.0), 0.7, 0.2, 1.0, 1.0);
        let ahead = c.pos + c.forward() * 500.0;
        let v = c.view_projection() * ahead.extend(1.0);
        assert!(close(v.x / v.w + 1.0, 1.0) && close(v.y / v.w + 1.0, 1.0));
    }

    #[test]
    fn player_view_projection_matches_the_d3dx_form() {
        let pv = PLAYER_VIEW_DEFAULT;
        let p = pv.projection(640, 480);
        let ys = 1.0 / 25f32.to_radians().tan();
        assert!(close(p.y_axis.y, ys));
        assert!(close(p.x_axis.x, ys / (640.0 / 480.0)));
        // near and far land on 0 and 1; the top edge of the 50-degree field hits NDC y = 1
        for (z, want) in [(0.1, 0.0), (7.0, 1.0)] {
            let c = p * Vec4::new(0.0, 0.0, z, 1.0);
            assert!(close(c.z / c.w, want), "z {z}");
        }
        let c = p * Vec4::new(0.0, 2.0 * 25f32.to_radians().tan(), 2.0, 1.0);
        assert!(close(c.y / c.w, 1.0));
    }

    #[test]
    fn depth_split_puts_the_view_model_in_front_of_the_world() {
        // a view-model point 3 units ahead and the world just past its near plane (8 units)
        let pv = PLAYER_VIEW_DEFAULT;
        let world = Camera::looking(Vec3::ZERO, 0.0, 0.0, 1.5, 1.2);
        let v = pv.window_depth(3.0, PLAYER_VIEW_DEPTH_RANGE);
        let w = world.window_depth(8.0);
        assert!(v <= 0.1 && w >= 0.1 && v < w, "view {v} world {w}");
        // with the full [0, 1] range the world would win: the split is what keeps the weapon in front
        assert!(pv.window_depth(3.0, (0.0, 1.0)) > w);
        let pos = Vec3::new(1.0, 2.0, 3.0);
        let c = Camera::looking(pos, 0.4, 0.2, 1.0, 1.0);
        let m = c.view() * c.world_from_eye();
        assert!(m.abs_diff_eq(Mat4::IDENTITY, 1e-5));
    }

    #[test]
    fn far_clamp() {
        let mut c = Camera::looking(Vec3::ZERO, 0.0, 0.0, 1.0, 1.0);
        c.far_z = 1.0;
        assert_eq!(c.far(), FAR_Z_MIN);
        c.far_z = 1e9;
        assert_eq!(c.far(), FAR_Z_MAX);
        c.far_z = f32::NAN;
        assert_eq!(c.far(), DEFAULT_FAR_Z);
    }

    #[test]
    fn frustum_corners_lie_on_planes() {
        let c = Camera::looking(Vec3::new(1.0, 2.0, 3.0), 0.4, -0.1, 1.2, 0.9);
        let p = c.view_projection();
        for (i, k) in c.frustum_corners().iter().enumerate() {
            let v = p * k.extend(1.0);
            let z = v.z / v.w;
            assert!(close(z, if i < 4 { 0.0 } else { 1.0 }), "corner {i}: {z}");
            assert!(close((v.x / v.w).abs(), 1.0));
        }
    }

    #[test]
    fn fov_rule_2009_widens_the_horizontal_field_with_the_aspect() {
        let deg = |w, h| fov_pair_2009(FOV_Y_DEFAULT_DEG, w, h).0.to_degrees();
        // 75 degrees vertical: 4:3, 16:10, 16:9, 21:9 (the vertical angle never changes)
        assert!(
            (deg(1024, 768) - 91.31).abs() < 0.02,
            "4:3 {}",
            deg(1024, 768)
        );
        assert!(
            (deg(1920, 1200) - 101.67).abs() < 0.02,
            "16:10 {}",
            deg(1920, 1200)
        );
        assert!(
            (deg(2560, 1440) - 107.51).abs() < 0.02,
            "16:9 {}",
            deg(2560, 1440)
        );
        assert!(
            (deg(3440, 1440) - 122.77).abs() < 0.02,
            "21:9 {}",
            deg(3440, 1440)
        );
        for (w, h) in [(1024, 768), (1920, 1080), (3440, 1440)] {
            assert!((fov_pair_2009(75.0, w, h).1.to_degrees() - 75.0).abs() < 1e-4);
        }
        // the viewer helper is the exact inverse
        let (x, y) = fov_pair_2009(75.0, 1920, 1080);
        assert!((fov_y_for(x, 1920, 1080) - y).abs() < 1e-5);
        // a 90 degree reference at 4:3 widens to 106.26 at 16:9
        let y = fov_y_deg_for_reference_x(90.0);
        assert!((fov_pair_2009(y, 1920, 1080).0.to_degrees() - 106.26).abs() < 0.01);
        assert!((fov_pair_2009(y, 640, 480).0.to_degrees() - 90.0).abs() < 0.01);
        // extreme aspects stay below 180 degrees
        assert!(fov_pair_2009(170.0, 7680, 1080).0 < std::f32::consts::PI);
    }
}
