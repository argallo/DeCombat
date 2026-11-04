//! The fly camera of the editor viewport and its projection maths, with no egui or GPU types so the round trips
//! are unit-tested headlessly. The conventions are dc-render's (`dc_render::Camera::looking`): left-handed
//! axes, +Y up, +Z forward; yaw turns about +Y (yaw 0 looks along +Z, positive yaw toward +X); positive pitch
//! looks down; rotation = Ry(yaw) * Rx(pitch); the projection is `x' = x / tx`, `y' = y / ty`, `w' = z` with
//! `ty = tan(fov_y / 2)` and `tx = ty * aspect` (dc-render's `fov_x_for`), so a marker drawn at the pixel this
//! module computes lands on the point the viewport renders.
// DC-DEFAULT: MAP-62 (the GUI viewport: fly camera, right-drag look, WASD/QE, scroll speed)

/// A world point or direction.
pub type V = [f32; 3];

/// A ray from the eye through a pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: V,
    /// Unit length.
    pub dir: V,
}

pub fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn scale(a: V, k: f32) -> V {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn len(a: V) -> f32 {
    dot(a, a).sqrt()
}

pub fn norm(a: V) -> V {
    let l = len(a);
    if l > 1e-12 {
        scale(a, 1.0 / l)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// The lowest / highest pitch (radians): just short of straight down / up, so the basis never degenerates.
pub const PITCH_LIMIT: f32 = 1.55;
/// Radians of yaw / pitch per pixel of a right-drag.
pub const LOOK_RATE: f32 = 0.004;
/// Speed bounds (world units per second) and the factor one scroll notch applies.
pub const SPEED_MIN: f32 = 20.0;
pub const SPEED_MAX: f32 = 20_000.0;
pub const SPEED_STEP: f32 = 1.25;

/// The fly camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlyCamera {
    pub pos: V,
    /// Radians about +Y; 0 looks along +Z.
    pub yaw: f32,
    /// Radians; positive looks down.
    pub pitch: f32,
    /// Vertical field of view, degrees.
    pub fov_y_deg: f32,
    /// World units per second of WASD / QE flight.
    pub speed: f32,
    /// The near plane: points nearer than this are not projected.
    pub near: f32,
    /// The far plane handed to the renderer.
    pub far: f32,
}

impl Default for FlyCamera {
    /// A view of the 4096-unit templates from above one side, looking at the centre.
    fn default() -> Self {
        FlyCamera {
            pos: [0.0, 900.0, -2600.0],
            yaw: 0.0,
            pitch: 0.32,
            fov_y_deg: 60.0,
            speed: 800.0,
            near: 1.0,
            far: 30_000.0,
        }
    }
}

impl FlyCamera {
    /// Right, up and forward unit vectors of the camera frame in world space (rotation = Ry(yaw) * Rx(pitch)).
    pub fn basis(&self) -> (V, V, V) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let right = [cy, 0.0, -sy];
        let up = [sy * sp, cp, cy * sp];
        let forward = [sy * cp, -sp, cy * cp];
        (right, up, forward)
    }

    /// Forward direction.
    pub fn forward(&self) -> V {
        self.basis().2
    }

    /// Turn by a mouse delta in pixels (right-drag look).
    pub fn look(&mut self, dx_px: f32, dy_px: f32) {
        self.yaw = (self.yaw + dx_px * LOOK_RATE + std::f32::consts::PI)
            .rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        self.pitch = (self.pitch + dy_px * LOOK_RATE).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Fly: `fwd`, `right`, `up` in -1..1 (WASD / QE) for `dt` seconds. Forward flight follows the view direction
    /// (pitch included), `up` is the world's +Y.
    pub fn fly(&mut self, fwd: f32, right: f32, up: f32, dt: f32) {
        let (r, _, f) = self.basis();
        let mut d = add(scale(f, fwd), scale(r, right));
        d[1] += up;
        if len(d) > 1e-6 {
            self.pos = add(self.pos, scale(norm(d), self.speed * dt));
        }
    }

    /// One scroll notch up (`steps` > 0) speeds up, down slows down.
    pub fn scale_speed(&mut self, steps: f32) {
        self.speed = (self.speed * SPEED_STEP.powf(steps)).clamp(SPEED_MIN, SPEED_MAX);
    }

    /// `(tan(fov_x / 2), tan(fov_y / 2))` for a `w x h` viewport.
    pub fn tan_half(&self, w: f32, h: f32) -> (f32, f32) {
        let ty = (self.fov_y_deg.to_radians() * 0.5).tan();
        let aspect = w.max(1.0) / h.max(1.0);
        (ty * aspect, ty)
    }

    /// Eye-space coordinates of a world point (x right, y up, z forward).
    pub fn eye(&self, world: V) -> V {
        let (r, u, f) = self.basis();
        let d = sub(world, self.pos);
        [dot(r, d), dot(u, d), dot(f, d)]
    }

    /// The pixel (x right, y down, origin top-left) a world point lands on in a `w x h` viewport, with its
    /// eye-space depth; `None` when the point is on or behind the near plane.
    pub fn project(&self, world: V, w: f32, h: f32) -> Option<([f32; 2], f32)> {
        let e = self.eye(world);
        if e[2] <= self.near {
            return None;
        }
        let (tx, ty) = self.tan_half(w, h);
        let nx = e[0] / (tx * e[2]);
        let ny = e[1] / (ty * e[2]);
        Some(([(nx + 1.0) * 0.5 * w, (1.0 - ny) * 0.5 * h], e[2]))
    }

    /// The ray through a pixel of a `w x h` viewport.
    pub fn unproject(&self, px: f32, py: f32, w: f32, h: f32) -> Ray {
        let (tx, ty) = self.tan_half(w, h);
        let nx = px / w.max(1.0) * 2.0 - 1.0;
        let ny = 1.0 - py / h.max(1.0) * 2.0;
        let (r, u, f) = self.basis();
        let d = add(add(scale(r, nx * tx), scale(u, ny * ty)), f);
        Ray {
            origin: self.pos,
            dir: norm(d),
        }
    }

    /// Point the camera at `target` from its current position.
    pub fn look_at(&mut self, target: V) {
        let d = sub(target, self.pos);
        let flat = (d[0] * d[0] + d[2] * d[2]).sqrt();
        self.yaw = d[0].atan2(d[2]);
        self.pitch = (-d[1]).atan2(flat).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Place the camera to see a box (`lo..hi`): just inside its -z edge, above its top, looking at the centre
    /// of its floor (the templates' walls are low next to their span, so this looks over them into the map).
    pub fn frame_box(&mut self, lo: V, hi: V) {
        let c = [(lo[0] + hi[0]) * 0.5, lo[1], (lo[2] + hi[2]) * 0.5];
        let span = len(sub(hi, lo)).max(1.0);
        self.pos = [c[0], hi[1] + span * 0.12, lo[2] + span * 0.04];
        self.look_at(c);
    }
}

impl Ray {
    /// The point `t` along the ray.
    pub fn at(&self, t: f32) -> V {
        add(self.origin, scale(self.dir, t))
    }

    /// Where the ray meets the horizontal plane `y = y0` ahead of the eye (the ground-plane drag of a selected
    /// object: the object keeps its height and follows the pointer on its own plane).
    pub fn hit_plane_y(&self, y0: f32) -> Option<V> {
        if self.dir[1].abs() < 1e-6 {
            return None;
        }
        let t = (y0 - self.origin[1]) / self.dir[1];
        (t > 0.0).then(|| self.at(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V, b: V, eps: f32) -> bool {
        sub(a, b).iter().all(|c| c.abs() <= eps)
    }

    #[test]
    fn the_basis_is_orthonormal_and_follows_the_conventions() {
        let mut cam = FlyCamera {
            yaw: 0.0,
            pitch: 0.0,
            ..Default::default()
        };
        let (r, u, f) = cam.basis();
        assert!(close(r, [1.0, 0.0, 0.0], 1e-6));
        assert!(close(u, [0.0, 1.0, 0.0], 1e-6));
        assert!(close(f, [0.0, 0.0, 1.0], 1e-6));
        // yaw 90 looks along +x; a positive pitch looks down
        cam.yaw = std::f32::consts::FRAC_PI_2;
        assert!(close(cam.forward(), [1.0, 0.0, 0.0], 1e-6));
        cam.pitch = 0.5;
        assert!(cam.forward()[1] < 0.0);
        for (a, b) in [(r, u), (u, f), (r, f)] {
            assert!(dot(a, b).abs() < 1e-6);
        }
        let (r, u, f) = cam.basis();
        for v in [r, u, f] {
            assert!((len(v) - 1.0).abs() < 1e-6);
        }
        assert!(
            dot(cross(r, u), f) > 0.99,
            "left-handed frame: right x up = forward"
        );
    }

    #[test]
    fn project_and_unproject_round_trip() {
        let cam = FlyCamera {
            pos: [100.0, 300.0, -800.0],
            yaw: 0.7,
            pitch: 0.25,
            ..Default::default()
        };
        let (w, h) = (1280.0, 720.0);
        for p in [
            [0.0, 0.0, 0.0],
            [500.0, 40.0, 900.0],
            [-700.0, 200.0, 300.0],
            [1500.0, -100.0, 2000.0],
        ] {
            let (px, depth) = cam.project(p, w, h).expect("in front");
            assert!(depth > 0.0);
            let ray = cam.unproject(px[0], px[1], w, h);
            // the ray through the pixel passes through the point
            let t = dot(sub(p, ray.origin), ray.dir);
            assert!(
                close(ray.at(t), p, 0.05),
                "{p:?} -> {px:?} -> {:?}",
                ray.at(t)
            );
            // and re-projecting the ray's point gives the pixel back
            let (back, _) = cam.project(ray.at(t), w, h).unwrap();
            assert!((back[0] - px[0]).abs() < 0.01 && (back[1] - px[1]).abs() < 0.01);
        }
        // the centre pixel is the forward direction
        let centre = cam.unproject(w / 2.0, h / 2.0, w, h);
        assert!(close(centre.dir, cam.forward(), 1e-4));
        // a point behind the eye does not project
        let behind = sub(cam.pos, scale(cam.forward(), 100.0));
        assert!(cam.project(behind, w, h).is_none());
    }

    #[test]
    fn ground_plane_drag_keeps_the_height_and_follows_the_pointer() {
        let cam = FlyCamera::default();
        let (w, h) = (1024.0, 768.0);
        let obj = [120.0, 40.0, -300.0];
        let (px, _) = cam.project(obj, w, h).unwrap();
        // dragging to another pixel moves the object on its own plane y = 40
        let ray = cam.unproject(px[0] + 80.0, px[1] + 30.0, w, h);
        let moved = ray.hit_plane_y(obj[1]).expect("the plane is ahead");
        assert!((moved[1] - 40.0).abs() < 1e-3);
        assert!(moved[0] > obj[0], "right on screen is +x here (yaw 0)");
        assert!(moved[2] < obj[2], "down on screen is nearer (-z)");
        let (back, _) = cam.project(moved, w, h).unwrap();
        assert!((back[0] - px[0] - 80.0).abs() < 0.05 && (back[1] - px[1] - 30.0).abs() < 0.05);
        // a ray that never meets the plane (pointing up, plane below) answers None
        let up = Ray {
            origin: [0.0, 100.0, 0.0],
            dir: [0.0, 1.0, 0.0],
        };
        assert!(up.hit_plane_y(0.0).is_none());
        let flat = Ray {
            origin: [0.0, 100.0, 0.0],
            dir: [1.0, 0.0, 0.0],
        };
        assert!(flat.hit_plane_y(0.0).is_none());
    }

    #[test]
    fn flight_look_and_speed_are_bounded() {
        let mut cam = FlyCamera {
            yaw: 0.0,
            pitch: 0.0,
            ..Default::default()
        };
        let start = cam.pos;
        cam.fly(1.0, 0.0, 0.0, 0.5);
        assert!((cam.pos[2] - start[2] - cam.speed * 0.5).abs() < 1e-3);
        cam.fly(0.0, 0.0, 1.0, 1.0);
        assert!((cam.pos[1] - start[1] - cam.speed).abs() < 1e-3);
        cam.look(0.0, 10_000.0);
        assert!((cam.pitch - PITCH_LIMIT).abs() < 1e-6);
        cam.look(10_000.0, 0.0);
        assert!(cam.yaw.abs() <= std::f32::consts::PI + 1e-3);
        cam.scale_speed(1000.0);
        assert_eq!(cam.speed, SPEED_MAX);
        cam.scale_speed(-1000.0);
        assert_eq!(cam.speed, SPEED_MIN);
        cam.frame_box([-2048.0, 0.0, -2048.0], [2048.0, 700.0, 2048.0]);
        assert!(close(
            norm(sub([0.0, 0.0, 0.0], cam.pos)),
            cam.forward(),
            1e-3
        ));
        assert!(
            cam.pos[1] > 700.0 && cam.pos[2] > -2048.0,
            "above the box, inside its -z edge"
        );
    }
}
