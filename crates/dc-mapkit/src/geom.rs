//! The intermediate geometry between the project and the world file (MAPKIT_PLAN.md 5.3 step 1, D4).
//!
//! The build stage (next slice) turns a [`Level`] into BSP models and render data; this module produces it:
//! convex solids ([`solid_faces`] / [`solid_polys`]), the terrain grid ([`terrain_polys`]), OBJ meshes
//! ([`parse_obj`] / [`obj_polys`]) and the per-polygon surface data.
//!
//! Conventions (MAPKIT_PLAN.md section 2, FMT_DAT85.md): polygons are convex, wound counter-clockwise seen from
//! the front (the outside), the plane is `dot(n, p) = d` with a unit normal, and the flags are `SURF_SOLID` (0x1),
//! `SURF_SKY` (0x10) and `SURF_NO_STEP` (0x400000); `surface` is the footstep surface id written into `texflags`.
//!
//! Determinism: nothing here calls a libm transcendental. Rotations use [`sincos_deg`] (exact at multiples of 90
//! degrees, an own f64 series elsewhere); cylinders take their unit circle from the project (computed once at
//! authoring time, [`unit_circle`]); normals use `sqrt`, which IEEE 754 rounds correctly on every platform.

use crate::project::{BrushRole, Heights, Solid, SolidKind, Terrain};
use std::ops::{Add, Mul, Neg, Sub};

pub const SURF_SOLID: u32 = 0x1;
pub const SURF_SKY: u32 = 0x10;
pub const SURF_NO_STEP: u32 = 0x40_0000;

/// A world-space vector.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl V3 {
    pub const ZERO: V3 = V3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const UP: V3 = V3 {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };

    pub const fn new(x: f32, y: f32, z: f32) -> V3 {
        V3 { x, y, z }
    }
    pub fn from_array(a: [f32; 3]) -> V3 {
        V3::new(a[0], a[1], a[2])
    }
    pub fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
    pub fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    /// Unit vector, or `None` for a (near) zero vector.
    pub fn normalized(self) -> Option<V3> {
        let l = self.length();
        if l > 1e-12 {
            Some(self * (1.0 / l))
        } else {
            None
        }
    }
    pub fn min(self, o: V3) -> V3 {
        V3::new(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }
    pub fn max(self, o: V3) -> V3 {
        V3::new(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for V3 {
    type Output = V3;
    fn mul(self, s: f32) -> V3 {
        V3::new(self.x * s, self.y * s, self.z * s)
    }
}
impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        V3::new(-self.x, -self.y, -self.z)
    }
}

/// `dot(n, p) = d`, unit `n`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub n: V3,
    pub d: f32,
}

impl Plane {
    /// The plane of a polygon from its corners (Newell's method over every edge, so a slightly bent polygon
    /// still gets the best-fit normal); `None` when the corners are collinear or fewer than three.
    pub fn from_corners(corners: &[V3]) -> Option<Plane> {
        if corners.len() < 3 {
            return None;
        }
        let mut n = V3::ZERO;
        let mut c = V3::ZERO;
        for i in 0..corners.len() {
            let a = corners[i];
            let b = corners[(i + 1) % corners.len()];
            n.x += (a.y - b.y) * (a.z + b.z);
            n.y += (a.z - b.z) * (a.x + b.x);
            n.z += (a.x - b.x) * (a.y + b.y);
            c = c + a;
        }
        let n = n.normalized()?;
        let c = c * (1.0 / corners.len() as f32);
        Some(Plane { n, d: n.dot(c) })
    }

    /// Signed distance of `p` (positive in front).
    pub fn dist(&self, p: V3) -> f32 {
        self.n.dot(p) - self.d
    }

    pub fn flipped(&self) -> Plane {
        Plane {
            n: -self.n,
            d: -self.d,
        }
    }
}

/// Per-face surface data a generator stamps on its polygons.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    pub material: usize,
    pub flags: u32,
    pub surface: u16,
    pub uv_scale: f32,
    /// See `Material::fit`.
    pub fit: Option<[f32; 4]>,
}

/// A texture region fitted across one face: `u = p . du + cu`, `v = p . dv + cv` (MAP-97).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvFrame {
    pub du: V3,
    pub dv: V3,
    pub cu: f32,
    pub cv: f32,
}

impl UvFrame {
    /// The frame that maps `region` once across `corners`, with u along `right` and v along `down`.
    pub fn fit(corners: &[V3], right: V3, down: V3, region: [f32; 4]) -> Option<UvFrame> {
        let span = |axis: V3| {
            corners.iter().fold((f32::MAX, f32::MIN), |(lo, hi), c| {
                let d = c.dot(axis);
                (lo.min(d), hi.max(d))
            })
        };
        let (u_lo, u_hi) = span(right);
        let (v_lo, v_hi) = span(down);
        if u_hi - u_lo < 1e-3 || v_hi - v_lo < 1e-3 {
            return None;
        }
        let (su, sv) = (
            (region[2] - region[0]) / (u_hi - u_lo),
            (region[3] - region[1]) / (v_hi - v_lo),
        );
        Some(UvFrame {
            du: right * su,
            dv: down * sv,
            cu: region[0] - u_lo * su,
            cv: region[1] - v_lo * sv,
        })
    }
}

impl Default for Surface {
    fn default() -> Self {
        Surface {
            material: 0,
            flags: SURF_SOLID,
            surface: 0,
            uv_scale: 128.0,
            fit: None,
        }
    }
}

/// A convex world-space polygon with its surface data.
#[derive(Debug, Clone, PartialEq)]
pub struct Poly {
    pub corners: Vec<V3>,
    pub plane: Plane,
    pub material: usize,
    pub flags: u32,
    pub surface: u16,
    pub uv_scale: f32,
    pub uv_frame: Option<UvFrame>,
    /// The light at each corner follows this normal instead of the plane's: an imported model's smooth shading (MAP-104).
    pub corner_normals: Option<Vec<V3>>,
    /// The exact texture coordinates per corner (an imported model): no frame could hold these, the long thin
    /// triangles of a simplified model round too badly (DC-DEFAULT: MAP-104).
    pub corner_uvs: Option<Vec<[f32; 2]>>,
    pub group: Option<String>,
}

impl Poly {
    /// Build a polygon from corners, orienting the winding so the normal points along `outward` (CCW from the
    /// front). `None` for a degenerate outline.
    pub fn new(
        mut corners: Vec<V3>,
        outward: V3,
        s: Surface,
        group: Option<String>,
    ) -> Option<Poly> {
        let mut plane = Plane::from_corners(&corners)?;
        if plane.n.dot(outward) < 0.0 {
            corners.reverse();
            plane = plane.flipped();
        }
        Some(Poly {
            corners,
            plane,
            material: s.material,
            flags: s.flags,
            surface: s.surface,
            uv_scale: s.uv_scale,
            uv_frame: None,
            corner_normals: None,
            corner_uvs: None,
            group,
        })
    }

    pub fn centroid(&self) -> V3 {
        let mut c = V3::ZERO;
        for p in &self.corners {
            c = c + *p;
        }
        c * (1.0 / self.corners.len().max(1) as f32)
    }

    /// Planar (every corner within `eps` of the plane), convex and CCW about the plane normal.
    pub fn check(&self, eps: f32) -> Result<(), String> {
        let n = self.corners.len();
        if n < 3 {
            return Err(format!("{n} corners"));
        }
        let len = self.plane.n.length();
        if (len - 1.0).abs() > 1e-4 {
            return Err(format!("normal length {len}"));
        }
        for (i, c) in self.corners.iter().enumerate() {
            let d = self.plane.dist(*c);
            if d.abs() > eps {
                return Err(format!("corner {i} is {d} off the plane"));
            }
        }
        for i in 0..n {
            let a = self.corners[i];
            let b = self.corners[(i + 1) % n];
            let c = self.corners[(i + 2) % n];
            let turn = (b - a).cross(c - b).dot(self.plane.n);
            if turn < -eps {
                return Err(format!(
                    "clockwise or concave turn at corner {}",
                    (i + 1) % n
                ));
            }
        }
        Ok(())
    }

    /// UV of a corner by the dominant axis of the normal (MAPKIT_PLAN.md 5.3 step 1: "uv projection by face
    /// axis"): a mostly-horizontal face maps (x, z), a face along x maps (z, y), else (x, y); one tile per
    /// `uv_scale` units.
    pub fn uv(&self, p: V3) -> [f32; 2] {
        match &self.uv_frame {
            Some(f) => [p.dot(f.du) + f.cu, p.dot(f.dv) + f.cv],
            None => uv_project(p, self.plane.n, self.uv_scale),
        }
    }
}

/// See [`Poly::uv`].
pub fn uv_project(p: V3, n: V3, scale: f32) -> [f32; 2] {
    let s = if scale > 0.0 { 1.0 / scale } else { 1.0 };
    let (ax, ay, az) = (n.x.abs(), n.y.abs(), n.z.abs());
    if ay >= ax && ay >= az {
        [p.x * s, p.z * s]
    } else if ax >= az {
        [p.z * s, p.y * s]
    } else {
        [p.x * s, p.y * s]
    }
}

/// A brush model: a closed convex set of polygons an object of `role.class()` names; `origin` = the object's pos.
#[derive(Debug, Clone, PartialEq)]
pub struct Brush {
    pub name: String,
    pub role: BrushRole,
    pub origin: V3,
    pub polys: Vec<Poly>,
}

/// The whole map's geometry: level polygons (terrain, plain solids, imports), brushes and the sky faces
/// (level polygons carrying `SURF_SKY`, kept apart because render data excludes them, MAPKIT_PLAN.md 5.3 step 4).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Level {
    pub polys: Vec<Poly>,
    pub brushes: Vec<Brush>,
    pub sky_faces: Vec<Poly>,
    /// Where each of `polys` / `sky_faces` came from (parallel to them): a solid id, an import id or
    /// `terrain`; empty for a polygon pushed with [`Level::push`]. The build names them in `bsp.budget`.
    pub poly_sources: Vec<String>,
    pub sky_sources: Vec<String>,
    /// Polygons of hidden materials: in the collision model, in no render world (DC-DEFAULT: MAP-104).
    pub hidden: Vec<Poly>,
    pub hidden_sources: Vec<String>,
    /// Faces of blocker solids: the world's blocker section, players only, never drawn (DC-DEFAULT: MAP-124).
    pub blockers: Vec<Poly>,
    pub extent_min: V3,
    pub extent_max: V3,
}

impl Level {
    /// Recompute `extent_min/max` over every polygon (level, sky, hidden, blockers and brushes).
    pub fn update_extent(&mut self) {
        let mut first = true;
        let mut lo = V3::ZERO;
        let mut hi = V3::ZERO;
        let all = self
            .polys
            .iter()
            .chain(self.sky_faces.iter())
            .chain(self.hidden.iter())
            .chain(self.blockers.iter())
            .chain(self.brushes.iter().flat_map(|b| b.polys.iter()));
        for p in all {
            for c in &p.corners {
                if first {
                    lo = *c;
                    hi = *c;
                    first = false;
                } else {
                    lo = lo.min(*c);
                    hi = hi.max(*c);
                }
            }
        }
        self.extent_min = lo;
        self.extent_max = hi;
    }

    /// Add a level polygon, routing sky faces to `sky_faces` (source `""`).
    pub fn push(&mut self, p: Poly) {
        self.push_from(p, "");
    }

    /// Add a level polygon that came from `source` (a solid id, an import id, `terrain`), routing sky faces
    /// to `sky_faces`.
    pub fn push_from(&mut self, p: Poly, source: &str) {
        if p.flags & SURF_SKY != 0 {
            self.sky_faces.push(p);
            self.sky_sources.push(source.to_string());
        } else {
            self.polys.push(p);
            self.poly_sources.push(source.to_string());
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// deterministic trigonometry

/// (sin, cos) of an angle in degrees without libm: exact at multiples of 90, else an own f64 Taylor series over
/// the angle reduced to [-45, 45] degrees (error below 1e-15, far under f32 resolution). Same bits on every
/// platform, so a rotated solid builds identically everywhere (DC-DEFAULT: MAP-10).
pub fn sincos_deg(deg: f32) -> (f32, f32) {
    let mut a = deg as f64 % 360.0;
    if a < 0.0 {
        a += 360.0;
    }
    // quadrant and the exact cases
    let qr = ((a + 45.0) / 90.0).floor();
    let r = a - 90.0 * qr; // in [-45, 45)
    let q = (qr as i32).rem_euclid(4);
    let (s, c) = if r == 0.0 {
        (0.0, 1.0)
    } else {
        let x = r * (std::f64::consts::PI / 180.0);
        series_sincos(x)
    };
    let (s, c) = match q {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    };
    (s as f32, c as f32)
}

/// Taylor series of sin and cos for |x| <= pi/4 (Horner form, fixed term count: every evaluation is the same
/// sequence of IEEE operations).
fn series_sincos(x: f64) -> (f64, f64) {
    let x2 = x * x;
    // sin: x - x^3/3! + x^5/5! - ... up to x^19/19!
    let mut s = -1.0 / 121_645_100_408_832_000.0; // -1/19!
    let sin_coeffs = [
        -1.0 / 355_687_428_096_000.0, // 1/17!
        1.0 / 1_307_674_368_000.0,    // 1/15!
        -1.0 / 6_227_020_800.0,       // 1/13!
        1.0 / 39_916_800.0,           // 1/11!
        -1.0 / 362_880.0,             // 1/9!
        1.0 / 5_040.0,                // 1/7!
        -1.0 / 120.0,                 // 1/5!
        1.0 / 6.0,                    // 1/3!
        -1.0,
    ];
    for k in sin_coeffs {
        s = s * x2 - k;
    }
    let s = s * x;
    // cos: 1 - x^2/2! + x^4/4! - ... up to x^18/18!
    let mut c = -1.0 / 6_402_373_705_728_000.0; // -1/18!
    let cos_coeffs = [
        -1.0 / 20_922_789_888_000.0, // 1/16!
        1.0 / 87_178_291_200.0,      // 1/14!
        -1.0 / 479_001_600.0,        // 1/12!
        1.0 / 3_628_800.0,           // 1/10!
        -1.0 / 40_320.0,             // 1/8!
        1.0 / 720.0,                 // 1/6!
        -1.0 / 24.0,                 // 1/4!
        1.0 / 2.0,                   // 1/2!
        -1.0,
    ];
    for k in cos_coeffs {
        c = c * x2 - k;
    }
    (s, c)
}

/// The unit circle a cylinder stores: `sides` points `(cos, sin)` starting at (1, 0), computed once at authoring
/// time with the platform's f64 sin / cos and then frozen in map.json (so the build never recomputes them).
pub fn unit_circle(sides: u32) -> Vec<[f32; 2]> {
    let n = sides.max(3);
    (0..n)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / n as f64;
            [a.cos() as f32, a.sin() as f32]
        })
        .collect()
}

/// Position, yaw and uniform scale: local -> world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xform {
    pub pos: V3,
    pub rot_y_deg: f32,
    pub scale: f32,
}

impl Xform {
    pub fn new(pos: [f32; 3], rot_y_deg: f32, scale: f32) -> Xform {
        Xform {
            pos: V3::from_array(pos),
            rot_y_deg,
            scale,
        }
    }

    pub fn apply(&self, p: V3) -> V3 {
        let (s, c) = sincos_deg(self.rot_y_deg);
        let p = p * self.scale;
        V3::new(p.x * c + p.z * s, p.y, -p.x * s + p.z * c) + self.pos
    }

    /// Rotate a direction (no translation, no scale).
    pub fn rotate(&self, v: V3) -> V3 {
        let (s, c) = sincos_deg(self.rot_y_deg);
        V3::new(v.x * c + v.z * s, v.y, -v.x * s + v.z * c)
    }

    /// World -> local (the inverse).
    pub fn unapply(&self, p: V3) -> V3 {
        let (s, c) = sincos_deg(self.rot_y_deg);
        let p = p - self.pos;
        let r = V3::new(p.x * c - p.z * s, p.y, p.x * s + p.z * c);
        if self.scale != 0.0 {
            r * (1.0 / self.scale)
        } else {
            r
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// solids

/// One face of a solid before surfaces are assigned: its name (`top`, `bottom`, `north` (+z), `south` (-z),
/// `east` (+x), `west` (-x), `back`, `slope`, `side_<i>`) and its world-space corners, plus the outward direction.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub name: String,
    pub corners: Vec<V3>,
    pub outward: V3,
}

/// The faces of a solid in world space; `Err` for a malformed kind (circle length, non-convex outline).
pub fn solid_faces(s: &Solid) -> Result<Vec<Face>, String> {
    let x = Xform::new(s.pos, s.rot_y_deg, 1.0);
    let h = V3::new(s.size[0] / 2.0, s.size[1] / 2.0, s.size[2] / 2.0);
    if h.x <= 0.0 || h.y <= 0.0 || h.z <= 0.0 {
        return Err(format!("size {:?} is not positive", s.size));
    }
    let mut out = Vec::new();
    let face = |out: &mut Vec<Face>, name: &str, local: [V3; 4], dir: V3| {
        out.push(Face {
            name: name.into(),
            corners: local.iter().map(|p| x.apply(*p)).collect(),
            outward: x.rotate(dir),
        });
    };
    let p = |sx: f32, sy: f32, sz: f32| V3::new(sx * h.x, sy * h.y, sz * h.z);
    match &s.kind {
        SolidKind::Box => {
            face(
                &mut out,
                "top",
                [
                    p(-1., 1., -1.),
                    p(1., 1., -1.),
                    p(1., 1., 1.),
                    p(-1., 1., 1.),
                ],
                V3::UP,
            );
            face(
                &mut out,
                "bottom",
                [
                    p(-1., -1., -1.),
                    p(1., -1., -1.),
                    p(1., -1., 1.),
                    p(-1., -1., 1.),
                ],
                -V3::UP,
            );
            face(
                &mut out,
                "north",
                [
                    p(-1., -1., 1.),
                    p(1., -1., 1.),
                    p(1., 1., 1.),
                    p(-1., 1., 1.),
                ],
                V3::new(0., 0., 1.),
            );
            face(
                &mut out,
                "south",
                [
                    p(-1., -1., -1.),
                    p(1., -1., -1.),
                    p(1., 1., -1.),
                    p(-1., 1., -1.),
                ],
                V3::new(0., 0., -1.),
            );
            face(
                &mut out,
                "east",
                [
                    p(1., -1., -1.),
                    p(1., -1., 1.),
                    p(1., 1., 1.),
                    p(1., 1., -1.),
                ],
                V3::new(1., 0., 0.),
            );
            face(
                &mut out,
                "west",
                [
                    p(-1., -1., -1.),
                    p(-1., -1., 1.),
                    p(-1., 1., 1.),
                    p(-1., 1., -1.),
                ],
                V3::new(-1., 0., 0.),
            );
        }
        SolidKind::Wedge => {
            face(
                &mut out,
                "bottom",
                [
                    p(-1., -1., -1.),
                    p(1., -1., -1.),
                    p(1., -1., 1.),
                    p(-1., -1., 1.),
                ],
                -V3::UP,
            );
            face(
                &mut out,
                "back",
                [
                    p(-1., -1., 1.),
                    p(1., -1., 1.),
                    p(1., 1., 1.),
                    p(-1., 1., 1.),
                ],
                V3::new(0., 0., 1.),
            );
            face(
                &mut out,
                "slope",
                [
                    p(-1., -1., -1.),
                    p(1., -1., -1.),
                    p(1., 1., 1.),
                    p(-1., 1., 1.),
                ],
                V3::new(0., h.z, -h.y),
            );
            out.push(Face {
                name: "west".into(),
                corners: vec![
                    x.apply(p(-1., -1., -1.)),
                    x.apply(p(-1., -1., 1.)),
                    x.apply(p(-1., 1., 1.)),
                ],
                outward: x.rotate(V3::new(-1., 0., 0.)),
            });
            out.push(Face {
                name: "east".into(),
                corners: vec![
                    x.apply(p(1., -1., -1.)),
                    x.apply(p(1., -1., 1.)),
                    x.apply(p(1., 1., 1.)),
                ],
                outward: x.rotate(V3::new(1., 0., 0.)),
            });
        }
        SolidKind::Cylinder { sides, circle } => {
            if *sides < 3 || circle.len() != *sides as usize {
                return Err(format!(
                    "cylinder with {sides} sides has {} circle points",
                    circle.len()
                ));
            }
            let pts: Vec<[f32; 2]> = circle.iter().map(|c| [c[0] * h.x, c[1] * h.z]).collect();
            extrude(&mut out, &x, &pts, h.y)?;
        }
        SolidKind::Prism { points } => {
            extrude(&mut out, &x, points, h.y)?;
        }
    }
    Ok(out)
}

/// Top, bottom and `side_<i>` faces of a convex outline in local x / z extruded from -hy to +hy.
fn extrude(out: &mut Vec<Face>, x: &Xform, pts: &[[f32; 2]], hy: f32) -> Result<(), String> {
    let n = pts.len();
    if n < 3 {
        return Err(format!("outline has {n} points"));
    }
    // orientation and convexity in the x/z plane
    let mut pos = 0;
    let mut neg = 0;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let c = pts[(i + 2) % n];
        let t = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if t > 1e-6 {
            pos += 1;
        } else if t < -1e-6 {
            neg += 1;
        }
    }
    if pos > 0 && neg > 0 {
        return Err("outline is not convex".into());
    }
    if pos == 0 && neg == 0 {
        return Err("outline is degenerate".into());
    }
    let mut cx = 0.0;
    let mut cz = 0.0;
    for p in pts {
        cx += p[0];
        cz += p[1];
    }
    let (cx, cz) = (cx / n as f32, cz / n as f32);
    let ring: Vec<V3> = pts.iter().map(|p| V3::new(p[0], 0.0, p[1])).collect();
    out.push(Face {
        name: "top".into(),
        corners: ring
            .iter()
            .map(|p| x.apply(*p + V3::new(0., hy, 0.)))
            .collect(),
        outward: x.rotate(V3::UP),
    });
    out.push(Face {
        name: "bottom".into(),
        corners: ring
            .iter()
            .map(|p| x.apply(*p - V3::new(0., hy, 0.)))
            .collect(),
        outward: x.rotate(-V3::UP),
    });
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        let mid = (a + b) * 0.5;
        let dir = V3::new(mid.x - cx, 0.0, mid.z - cz);
        out.push(Face {
            name: format!("side_{i}"),
            corners: vec![
                x.apply(a - V3::new(0., hy, 0.)),
                x.apply(b - V3::new(0., hy, 0.)),
                x.apply(b + V3::new(0., hy, 0.)),
                x.apply(a + V3::new(0., hy, 0.)),
            ],
            outward: x.rotate(dir),
        });
    }
    Ok(())
}

/// The material id a face uses: the `faces` override by exact name, then `side` for any `side_<i>`, then the
/// solid's material.
pub fn face_material<'a>(s: &'a Solid, face: &str) -> &'a str {
    if let Some(m) = s.faces.get(face) {
        return m;
    }
    if face.starts_with("side_") {
        if let Some(m) = s.faces.get("side") {
            return m;
        }
    }
    &s.material
}

/// The tiling frame of one face of a yawed solid (DC-DEFAULT: MAP-99): the same projection as [`uv_project`], taken
/// along the solid's own axes and anchored at its centre, so the texture turns with the solid instead of lying
/// slanted across it. An unturned solid keeps the world projection (`None`), so its bytes do not change.
fn tile_frame(s: &Solid, p: &Poly) -> Option<UvFrame> {
    if s.rot_y_deg.rem_euclid(360.0) == 0.0 {
        return None;
    }
    let scale = if p.uv_scale > 0.0 {
        1.0 / p.uv_scale
    } else {
        1.0
    };
    let (sin, cos) = s.rot_y_deg.to_radians().sin_cos();
    let (x, z) = (V3::new(cos, 0.0, -sin), V3::new(sin, 0.0, cos));
    let n = p.plane.n;
    let (ax, ay, az) = (n.dot(x).abs(), n.y.abs(), n.dot(z).abs());
    let (u, v) = if ay >= ax && ay >= az {
        (x, z)
    } else if ax >= az {
        (z, V3::UP)
    } else {
        (x, V3::UP)
    };
    let centre = V3::from_array(s.pos);
    let (du, dv) = (u * scale, v * scale);
    Some(UvFrame {
        du,
        dv,
        cu: -centre.dot(du),
        cv: -centre.dot(dv),
    })
}

/// The fitted frame of one face, as seen from outside the solid: u runs to the viewer's right, v runs down. A top
/// or bottom face has no "down", so its u follows the solid's own local +x, whichever way the solid is turned. A
/// sky model is seen from inside, so its u runs the other way.
fn fit_frame(s: &Solid, p: &Poly, region: [f32; 4]) -> Option<UvFrame> {
    let n = p.plane.n;
    let (sin, cos) = s.rot_y_deg.to_radians().sin_cos();
    let mut right = if n.y.abs() > 0.9 {
        V3::new(cos, 0.0, -sin)
    } else {
        n.cross(V3::UP).normalized()?
    };
    if s.brush == Some(BrushRole::Sky) {
        right = -right;
    }
    let down = if n.y.abs() > 0.9 {
        n.cross(V3::new(cos, 0.0, -sin))
    } else {
        n.cross(n.cross(V3::UP).normalized()?)
    };
    UvFrame::fit(&p.corners, right, down, region)
}

/// The polygons of a solid; `surface_of(material id)` supplies the surface data (index, flags, surface id,
/// tile). Degenerate faces are skipped (a wedge with a zero slope cannot happen after the size check).
pub fn solid_polys(s: &Solid, surface_of: &dyn Fn(&str) -> Surface) -> Result<Vec<Poly>, String> {
    let faces = solid_faces(s)?;
    let mut out = Vec::with_capacity(faces.len());
    for f in faces {
        let surf = surface_of(face_material(s, &f.name));
        if let Some(mut p) = Poly::new(f.corners, f.outward, surf, s.group.clone()) {
            p.uv_frame = match surf.fit {
                Some(region) => fit_frame(s, &p, region),
                None => tile_frame(s, &p),
            };
            out.push(p);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------------
// terrain

/// Two triangles per cell, normals up; `surface_of(cell_x, cell_z)` picks the surface (paint layer).
pub fn terrain_polys(
    t: &Terrain,
    h: &Heights,
    surface_of: &dyn Fn(u32, u32) -> Surface,
) -> Vec<Poly> {
    let mut out = Vec::with_capacity((t.cells_x * t.cells_z * 2) as usize);
    if h.is_empty() || h.cols != t.cols() || h.rows != t.rows() {
        return out;
    }
    let at = |x: usize, z: usize| {
        V3::new(
            t.origin[0] + x as f32 * t.cell_size,
            t.raw_to_y(h.get(x, z)),
            t.origin[2] + z as f32 * t.cell_size,
        )
    };
    for z in 0..t.cells_z as usize {
        for x in 0..t.cells_x as usize {
            let a = at(x, z);
            let b = at(x + 1, z);
            let c = at(x + 1, z + 1);
            let d = at(x, z + 1);
            let s = surface_of(x as u32, z as u32);
            if let Some(p) = Poly::new(vec![a, d, c], V3::UP, s, None) {
                out.push(p);
            }
            if let Some(p) = Poly::new(vec![a, c, b], V3::UP, s, None) {
                out.push(p);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// OBJ

/// A parsed Wavefront OBJ: positions, optional uvs / normals, faces as 0-based position index lists.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObjMesh {
    pub positions: Vec<V3>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<V3>,
    pub faces: Vec<Vec<usize>>,
    /// For each face its `vt` indices, or nothing when the file gives none.
    pub face_uvs: Vec<Vec<usize>>,
    /// For each face its `vn` indices, or nothing when the file gives none.
    pub face_normals: Vec<Vec<usize>>,
}

impl ObjMesh {
    /// Fan triangulation of every face.
    pub fn triangles(&self) -> Vec<[usize; 3]> {
        let mut out = Vec::new();
        for f in &self.faces {
            for i in 1..f.len().saturating_sub(1) {
                out.push([f[0], f[i], f[i + 1]]);
            }
        }
        out
    }
}

/// Parse `v`, `vt`, `vn` and `f` lines (`f v`, `f v/vt`, `f v//vn`, `f v/vt/vn`, negative indices relative to
/// the end); every other line is ignored (materials, groups, smoothing). Errors name the line.
pub fn parse_obj(text: &str) -> Result<ObjMesh, String> {
    let mut m = ObjMesh::default();
    for (ln, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let kw = it.next().unwrap_or("");
        let nums = |it: std::str::SplitWhitespace, n: usize| -> Result<Vec<f32>, String> {
            let v: Result<Vec<f32>, _> = it.take(n).map(|s| s.parse::<f32>()).collect();
            let v = v.map_err(|e| format!("line {}: {e}", ln + 1))?;
            if v.len() < n {
                return Err(format!("line {}: expected {n} numbers", ln + 1));
            }
            Ok(v)
        };
        match kw {
            "v" => {
                let v = nums(it, 3)?;
                m.positions.push(V3::new(v[0], v[1], v[2]));
            }
            "vt" => {
                let v = nums(it, 2)?;
                m.uvs.push([v[0], v[1]]);
            }
            "vn" => {
                let v = nums(it, 3)?;
                m.normals.push(V3::new(v[0], v[1], v[2]));
            }
            "f" => {
                let mut face = Vec::new();
                let mut face_uv = Vec::new();
                let mut face_n = Vec::new();
                for tok in it {
                    let mut parts = tok.split('/');
                    let idx_s = parts.next().unwrap_or("");
                    let idx: i64 = idx_s
                        .parse()
                        .map_err(|_| format!("line {}: bad index `{tok}`", ln + 1))?;
                    let n = m.positions.len() as i64;
                    let zero = if idx > 0 {
                        idx - 1
                    } else if idx < 0 {
                        n + idx
                    } else {
                        return Err(format!("line {}: index 0", ln + 1));
                    };
                    if zero < 0 || zero >= n {
                        return Err(format!(
                            "line {}: index {idx} out of range (have {n} positions)",
                            ln + 1
                        ));
                    }
                    face.push(zero as usize);
                    let extra = |text: Option<&str>, have: usize| -> Option<usize> {
                        let i: i64 = text.filter(|t| !t.is_empty())?.parse().ok()?;
                        let z = if i > 0 { i - 1 } else { have as i64 + i };
                        (z >= 0 && (z as usize) < have).then_some(z as usize)
                    };
                    if let Some(i) = extra(parts.next(), m.uvs.len()) {
                        face_uv.push(i);
                    }
                    if let Some(i) = extra(parts.next(), m.normals.len()) {
                        face_n.push(i);
                    }
                }
                if face.len() < 3 {
                    return Err(format!("line {}: face with {} corners", ln + 1, face.len()));
                }
                // a face keeps its uvs or normals only when every corner names one
                m.face_uvs.push(if face_uv.len() == face.len() {
                    face_uv
                } else {
                    Vec::new()
                });
                m.face_normals.push(if face_n.len() == face.len() {
                    face_n
                } else {
                    Vec::new()
                });
                m.faces.push(face);
            }
            _ => {}
        }
    }
    Ok(m)
}

/// Write a mesh as OBJ text (positions and faces, 1-based).
pub fn write_obj(m: &ObjMesh) -> String {
    let mut s = String::from("# dc-mapkit\n");
    for p in &m.positions {
        s.push_str(&format!("v {} {} {}\n", p.x, p.y, p.z));
    }
    for t in &m.uvs {
        s.push_str(&format!("vt {} {}\n", t[0], t[1]));
    }
    for n in &m.normals {
        s.push_str(&format!("vn {} {} {}\n", n.x, n.y, n.z));
    }
    for (k, f) in m.faces.iter().enumerate() {
        s.push('f');
        let uv = m.face_uvs.get(k).filter(|v| v.len() == f.len());
        let nor = m.face_normals.get(k).filter(|v| v.len() == f.len());
        for (c, i) in f.iter().enumerate() {
            s.push_str(&format!(" {}", i + 1));
            match (uv, nor) {
                (Some(u), Some(n)) => s.push_str(&format!("/{}/{}", u[c] + 1, n[c] + 1)),
                (Some(u), None) => s.push_str(&format!("/{}", u[c] + 1)),
                (None, Some(n)) => s.push_str(&format!("//{}", n[c] + 1)),
                (None, None) => {}
            }
        }
        s.push('\n');
    }
    s
}

/// Triangles of a mesh as polygons (winding as authored: the normal follows the face order; a degenerate
/// triangle is skipped and counted).
pub fn obj_polys(m: &ObjMesh, x: &Xform, s: Surface, group: Option<String>) -> (Vec<Poly>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0;
    for (k, f) in m.faces.iter().enumerate() {
        let uvs = m.face_uvs.get(k).filter(|v| v.len() == f.len());
        let normals = m.face_normals.get(k).filter(|v| v.len() == f.len());
        for i in 1..f.len().saturating_sub(1) {
            let at = [0, i, i + 1];
            let c: Vec<V3> = at.iter().map(|j| x.apply(m.positions[f[*j]])).collect();
            let Some(plane) = Plane::from_corners(&c) else {
                skipped += 1;
                continue;
            };
            out.push(Poly {
                uv_frame: None,
                corner_uvs: uvs.map(|u| {
                    at.iter()
                        .map(|j| {
                            let t = m.uvs[u[*j]];
                            // OBJ's v runs up, the texture's down
                            [t[0], 1.0 - t[1]]
                        })
                        .collect()
                }),
                corner_normals: normals
                    .map(|n| at.iter().map(|j| x.rotate(m.normals[n[*j]])).collect()),
                corners: c,
                plane,
                material: s.material,
                flags: s.flags,
                surface: s.surface,
                uv_scale: s.uv_scale,
                group: group.clone(),
            });
        }
    }
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Solid;

    #[test]
    fn an_obj_keeps_its_own_uvs_and_normals_through_the_transform() {
        let m = parse_obj(
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nvn 0 0 1\nf 1/1/1 2/2/1 3/3/1\n",
        )
        .unwrap();
        let x = Xform::new([100.0, 0.0, 50.0], 90.0, 200.0);
        let (polys, skipped) = obj_polys(&m, &x, Surface::default(), None);
        assert_eq!((polys.len(), skipped), (1, 0));
        let p = &polys[0];
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
        let uv = p.corner_uvs.as_ref().unwrap();
        assert!(close(uv[0], [0.0, 1.0]), "v runs down in the texture");
        assert!(close(uv[1], [1.0, 1.0]));
        assert!(close(uv[2], [0.0, 0.0]));
        let n = p.corner_normals.as_ref().unwrap()[0];
        assert!(
            (n.x - 1.0).abs() < 1e-4,
            "the normal turns with the model: {n:?}"
        );
        // written and read back, the same
        let again = parse_obj(&write_obj(&m)).unwrap();
        assert_eq!(
            (again.face_uvs.clone(), again.face_normals.clone()),
            (m.face_uvs.clone(), m.face_normals.clone())
        );
    }

    #[test]
    fn an_obj_without_uvs_is_projected_as_before() {
        let m = parse_obj("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        let (polys, _) = obj_polys(
            &m,
            &Xform::new([0.0; 3], 0.0, 1.0),
            Surface::default(),
            None,
        );
        assert!(polys[0].corner_uvs.is_none() && polys[0].corner_normals.is_none());
    }

    fn surf(_: &str) -> Surface {
        Surface::default()
    }

    fn check_solid(s: &Solid, expect: usize) {
        let polys = solid_polys(s, &surf).unwrap();
        assert_eq!(polys.len(), expect, "{:?}", s.kind);
        // an interior point of the convex solid: the mean of every corner (the box centre lies ON a wedge's slope)
        let mut inside = V3::ZERO;
        let mut n = 0.0;
        for p in &polys {
            for c in &p.corners {
                inside = inside + *c;
                n += 1.0;
            }
        }
        let inside = inside * (1.0 / n);
        for p in &polys {
            p.check(1e-3)
                .unwrap_or_else(|e| panic!("{:?} face: {e}", s.kind));
            // outward: the plane normal points away from the solid's interior
            assert!(
                p.plane.dist(inside) < 0.0,
                "{:?}: interior point in front of a face",
                s.kind
            );
            // independent CCW check: the cross of the first two edges follows the normal
            let n2 = (p.corners[1] - p.corners[0]).cross(p.corners[2] - p.corners[0]);
            assert!(n2.dot(p.plane.n) > 0.0);
        }
    }

    #[test]
    fn every_solid_kind_yields_planar_convex_ccw_outward_polygons() {
        let base = Solid {
            pos: [100.0, 50.0, -30.0],
            size: [64.0, 96.0, 128.0],
            ..Default::default()
        };
        for rot in [0.0, 33.0, 90.0, 180.0, -45.0] {
            let mut s = base.clone();
            s.rot_y_deg = rot;
            s.kind = SolidKind::Box;
            check_solid(&s, 6);
            s.kind = SolidKind::Wedge;
            check_solid(&s, 5);
            s.kind = SolidKind::Cylinder {
                sides: 8,
                circle: unit_circle(8),
            };
            check_solid(&s, 10);
            s.kind = SolidKind::Prism {
                points: vec![
                    [-40.0, -40.0],
                    [40.0, -40.0],
                    [60.0, 0.0],
                    [40.0, 40.0],
                    [-40.0, 40.0],
                ],
            };
            check_solid(&s, 7);
            // clockwise outline is accepted too (the winding is fixed per face)
            s.kind = SolidKind::Prism {
                points: vec![[-40.0, 40.0], [40.0, 40.0], [40.0, -40.0], [-40.0, -40.0]],
            };
            check_solid(&s, 6);
        }
    }

    #[test]
    fn malformed_solids_are_refused() {
        let mut s = Solid {
            size: [0.0, 1.0, 1.0],
            ..Default::default()
        };
        assert!(solid_faces(&s).is_err());
        s.size = [1.0; 3];
        s.kind = SolidKind::Cylinder {
            sides: 6,
            circle: unit_circle(5),
        };
        assert!(solid_faces(&s).is_err());
        s.kind = SolidKind::Prism {
            points: vec![
                [0.0, 0.0],
                [10.0, 0.0],
                [10.0, 10.0],
                [2.0, 2.0],
                [0.0, 10.0],
            ],
        };
        assert!(solid_faces(&s).unwrap_err().contains("convex"));
        s.kind = SolidKind::Prism {
            points: vec![[0.0, 0.0], [10.0, 0.0]],
        };
        assert!(solid_faces(&s).is_err());
    }

    #[test]
    fn box_faces_are_named_and_rotation_is_exact_at_right_angles() {
        let s = Solid {
            size: [10.0, 20.0, 30.0],
            rot_y_deg: 90.0,
            ..Default::default()
        };
        let faces = solid_faces(&s).unwrap();
        let names: Vec<&str> = faces.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["top", "bottom", "north", "south", "east", "west"]);
        // a 90 degree yaw maps the local +z face exactly onto +x or -x (no rounding)
        let north = &faces[2];
        assert_eq!(north.outward.y, 0.0);
        assert!(
            north.outward.x.abs() == 1.0 && north.outward.z == 0.0,
            "{:?}",
            north.outward
        );
        let top = &faces[0];
        assert!(top.corners.iter().all(|c| c.y == 10.0));
        let mut s2 = s.clone();
        s2.faces.insert("top".into(), "a".into());
        s2.faces.insert("side".into(), "b".into());
        assert_eq!(face_material(&s2, "top"), "a");
        assert_eq!(face_material(&s2, "side_3"), "b");
        assert_eq!(face_material(&s2, "east"), "ground");
    }

    #[test]
    fn sincos_matches_std_and_is_exact_at_quadrants() {
        for deg in [0.0, 90.0, 180.0, 270.0, 360.0, -90.0, -180.0, 450.0] {
            let (s, c) = sincos_deg(deg);
            assert!(s == 0.0 || s.abs() == 1.0, "{deg}: {s}");
            assert!(c == 0.0 || c.abs() == 1.0, "{deg}: {c}");
            assert_eq!(s * s + c * c, 1.0);
        }
        let mut deg = -720.0f32;
        while deg <= 720.0 {
            let (s, c) = sincos_deg(deg);
            let r = (deg as f64).to_radians();
            assert!((s as f64 - r.sin()).abs() < 1e-6, "{deg}");
            assert!((c as f64 - r.cos()).abs() < 1e-6, "{deg}");
            deg += 7.3;
        }
    }

    #[test]
    fn xform_round_trips() {
        let x = Xform::new([5.0, -2.0, 9.0], 37.0, 2.0);
        let p = V3::new(1.0, 2.0, 3.0);
        let q = x.unapply(x.apply(p));
        assert!((q - p).length() < 1e-4);
    }

    #[test]
    fn uv_projection_follows_the_dominant_axis() {
        let p = V3::new(10.0, 20.0, 30.0);
        assert_eq!(uv_project(p, V3::UP, 10.0), [1.0, 3.0]);
        assert_eq!(uv_project(p, V3::new(1.0, 0.1, 0.0), 10.0), [3.0, 2.0]);
        assert_eq!(uv_project(p, V3::new(0.0, 0.1, -1.0), 10.0), [1.0, 2.0]);
        assert_eq!(uv_project(p, V3::UP, 0.0), [10.0, 30.0]);
    }

    #[test]
    fn terrain_triangulation_counts_and_normals_up() {
        let t = Terrain {
            cells_x: 4,
            cells_z: 3,
            cell_size: 16.0,
            origin: [0.0, 0.0, 0.0],
            height_scale: 100.0,
            ..Default::default()
        };
        let mut h = Heights::for_terrain(&t, 0);
        for z in 0..t.rows() {
            for x in 0..t.cols() {
                h.set(x, z, ((x * 7 + z * 13) * 1000 % 65535) as u16);
            }
        }
        let polys = terrain_polys(&t, &h, &|_, _| Surface::default());
        assert_eq!(polys.len(), 24);
        for p in &polys {
            assert_eq!(p.corners.len(), 3);
            p.check(1e-3).unwrap();
            assert!(p.plane.n.y > 0.0);
        }
        let empty = terrain_polys(&t, &Heights::none(), &|_, _| Surface::default());
        assert!(empty.is_empty());
    }

    #[test]
    fn obj_round_trip_and_triangulation() {
        let text = "# comment\nv 0 0 0\nv 1 0 0\nv 1 0 1\nv 0 0 1\nvt 0 0\nvn 0 1 0\nusemtl x\nf 1/1/1 2/1/1 3/1/1 4/1/1\nf -4 -3 -2\n";
        let m = parse_obj(text).unwrap();
        assert_eq!(m.positions.len(), 4);
        assert_eq!(m.faces, vec![vec![0, 1, 2, 3], vec![0, 1, 2]]);
        assert_eq!(m.triangles(), vec![[0, 1, 2], [0, 2, 3], [0, 1, 2]]);
        let back = parse_obj(&write_obj(&m)).unwrap();
        assert_eq!(back, m);
        let (polys, skipped) = obj_polys(
            &m,
            &Xform::new([0.0; 3], 0.0, 2.0),
            Surface::default(),
            None,
        );
        assert_eq!(polys.len(), 3);
        assert_eq!(skipped, 0);
        assert_eq!(polys[0].corners[1], V3::new(2.0, 0.0, 0.0));
        assert!(parse_obj("f 1 2 3").is_err());
        assert!(parse_obj("v 1 2\n").is_err());
        assert!(parse_obj("v 0 0 0\nv 1 0 0\nv 2 0 0\nf 1 2 0\n").is_err());
        let degenerate = parse_obj("v 0 0 0\nv 1 0 0\nv 2 0 0\nf 1 2 3\n").unwrap();
        let (polys, skipped) = obj_polys(
            &degenerate,
            &Xform::new([0.0; 3], 0.0, 1.0),
            Surface::default(),
            None,
        );
        assert!(polys.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn level_routes_sky_faces_and_tracks_extent() {
        let mut l = Level::default();
        let s = Solid {
            size: [10.0; 3],
            ..Default::default()
        };
        let mut polys = solid_polys(&s, &surf).unwrap();
        polys[0].flags |= SURF_SKY;
        for p in polys {
            l.push(p);
        }
        assert_eq!(l.sky_faces.len(), 1);
        assert_eq!(l.polys.len(), 5);
        l.update_extent();
        assert_eq!(l.extent_min, V3::new(-5.0, -5.0, -5.0));
        assert_eq!(l.extent_max, V3::new(5.0, 5.0, 5.0));
    }
}
