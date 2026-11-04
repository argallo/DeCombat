//! World query: the swept-cylinder test against BSP world models and blocker polygons.
//!
//! [`Collider`] is the interface the movement code uses; [`CollisionWorld`] implements it for static world
//! geometry (BSP models of FMT_DAT85.md §5.2 plus the blocker soup of §9.1). With the `world` feature it is
//! built from a parsed `dc_world::World` (see [`crate::from_world`]); [`CollisionWorld::from_parts`] builds
//! one from convex polygons with [`build_bsp`] (synthetic tests, authoring tools).
// SPEC: MOVEMENT_2009.md §9.6 (engine+0x78280), §9.8 (engine+0x77db0), §9.9 (engine+0x77530)

extern crate alloc;
use alloc::vec::Vec;

use crate::shape::Cylinder;
use crate::todo;
use crate::vec3::V3;

/// Surface flag bit 0: the polygon takes part in the sweep.
// SPEC: MOVEMENT_2009.md §9.8 (tested only when the polygon's surface flags have bit 0 set)
pub const SURF_SOLID: u32 = 0x0000_0001;
/// Surface flag bit 0x400000: the stair move does not step up after hitting this surface.
// SPEC: MOVEMENT_2009.md §9.5 (flag "no step" when the hit polygon's surface flags have bit 0x400000)
pub const SURF_NO_STEP: u32 = 0x0040_0000;

/// Model index used in a [`Hit`] that came from the blocker soup.
pub const BLOCKER_MODEL: u32 = u32::MAX;

/// Result of a sweep that hit something.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Fraction of `to - from` that is free, after the back-off.
    pub t: f32,
    /// Normal of the contact (the face plane, or the side / cap / vertex normal of an edge contact): the
    /// slide loop clips the move against it.
    pub normal: V3,
    /// Plane normal of the hit polygon itself. The stair move's landing test and the ground probe judge the
    /// surface by it, and the standing contact carries it (the engine stores the polygon, whose plane the
    /// integrator and the mover read).
    // EVIDENCE: engine+0x78c50 (landing: poly->plane normal y), engine+0x79210 (probe: poly->plane normal y)
    pub face_normal: V3,
    /// Surface flags of the hit polygon.
    pub surface_flags: u32,
    /// World model index ([`BLOCKER_MODEL`] for blockers) and polygon index inside it.
    pub model: u32,
    pub poly: u32,
}

/// The world query the movement code needs.
pub trait Collider {
    /// Sweeps `shape` from `from` to `to`. `None` means the whole move is free.
    fn sweep(&self, shape: &Cylinder, from: V3, to: V3) -> Option<Hit>;
    /// Whether `shape` placed at `at` intersects solid geometry (the "box must fit" test).
    fn overlaps(&self, shape: &Cylinder, at: V3) -> bool;
}

/// A collider with no geometry (free space).
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyWorld;

impl Collider for EmptyWorld {
    fn sweep(&self, _: &Cylinder, _: V3, _: V3) -> Option<Hit> {
        None
    }
    fn overlaps(&self, _: &Cylinder, _: V3) -> bool {
        false
    }
}

/// A convex polygon for building geometry: plane `dot(normal, x) = dist`, corners counter-clockwise seen from
/// the front (OBSERVED over the 21 ROM worlds: `cross(v1 - v0, v2 - v1)` points along the normal for
/// 964,510 of 964,512 polygons).
#[derive(Debug, Clone, PartialEq)]
pub struct ConvexPoly {
    pub normal: V3,
    pub dist: f32,
    pub verts: Vec<V3>,
    pub flags: u32,
}

impl ConvexPoly {
    /// From corners (counter-clockwise seen from the front); the plane is derived from the first three.
    pub fn from_corners(verts: Vec<V3>, flags: u32) -> ConvexPoly {
        let n = if verts.len() >= 3 {
            (verts[1] - verts[0])
                .cross(verts[2] - verts[1])
                .normalized()
        } else {
            V3::ZERO
        };
        let dist = if verts.is_empty() {
            0.0
        } else {
            n.dot(verts[0])
        };
        ConvexPoly {
            normal: n,
            dist,
            verts,
            flags,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Face {
    normal: V3,
    dist: f32,
    first: u32,
    count: u32,
    flags: u32,
    center: V3,
    radius: f32,
    sloped: bool,
}

/// Polygons with precomputed outward edge planes.
#[derive(Debug, Clone, Default, PartialEq)]
struct PolySet {
    faces: Vec<Face>,
    verts: Vec<V3>,
    /// Per corner `i`: outward unit normal and distance of the plane through edge `i -> i+1`
    /// perpendicular to the face (`normalize(cross(edge, n))`).
    edges: Vec<(V3, f32)>,
}

impl PolySet {
    fn push(&mut self, normal: V3, dist: f32, corners: &[V3], flags: u32) {
        let first = self.verts.len() as u32;
        let k = corners.len();
        let mut center = V3::ZERO;
        for &c in corners {
            center += c;
        }
        if k > 0 {
            center *= 1.0 / k as f32;
        }
        let mut radius = 0.0f32;
        for &c in corners {
            let r = (c - center).len();
            if r > radius {
                radius = r;
            }
        }
        let usable = k >= 3 && normal.len_sq() > 0.5;
        for i in 0..k {
            let a = corners[i];
            let b = corners[(i + 1) % k];
            let mut en = (b - a).cross(normal).normalized();
            let mut ed = en.dot(a);
            // orient outward (away from the centroid) whatever the winding
            if en.dot(center) - ed > 0.0 {
                en = -en;
                ed = -ed;
            }
            self.verts.push(a);
            self.edges.push((en, ed));
        }
        let ny = crate::fmath::abs(normal.y);
        self.faces.push(Face {
            normal,
            dist,
            first,
            count: k as u32,
            flags: if usable { flags } else { 0 },
            center,
            radius,
            // SPEC: MOVEMENT_2009.md §9.9 step 4 (sloped polygon: 0.0001 <= |n.y| < 0.9999)
            sloped: (0.0001..0.9999).contains(&ny),
        });
    }

    fn bounds(&self) -> (V3, V3) {
        let mut lo = V3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut hi = V3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for v in &self.verts {
            lo = V3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
            hi = V3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
        }
        (lo, hi)
    }
}

const END: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Node {
    face: u32,
    neg: u32,
    pos: u32,
}

/// One world model: polygons plus the BSP over them.
#[derive(Debug, Clone, PartialEq)]
pub struct CollModel {
    set: PolySet,
    nodes: Vec<Node>,
    root: u32,
    lo: V3,
    hi: V3,
}

/// Reference to a child in a node list: a node index, or `None` for either sentinel.
pub type Child = Option<u32>;

impl CollModel {
    /// From faces and nodes `(face, negative child, positive child)`.
    pub fn new(faces: &[ConvexPoly], nodes: &[(u32, Child, Child)], root: Child) -> CollModel {
        let mut set = PolySet::default();
        for f in faces {
            set.push(f.normal, f.dist, &f.verts, f.flags);
        }
        let nf = set.faces.len() as u32;
        let nn = nodes.len() as u32;
        let link = |c: Child| match c {
            Some(i) if i < nn => i,
            _ => END,
        };
        // a node whose face index is out of range ends its path (the walk treats it like a sentinel)
        let nodes = nodes
            .iter()
            .map(|&(face, neg, pos)| Node {
                face: if face < nf { face } else { END },
                neg: link(neg),
                pos: link(pos),
            })
            .collect::<Vec<_>>();
        let root = link(root);
        let (lo, hi) = set.bounds();
        CollModel {
            set,
            nodes,
            root,
            lo,
            hi,
        }
    }

    /// Builds the BSP with [`build_bsp`] and wraps it.
    pub fn from_polys(faces: &[ConvexPoly]) -> CollModel {
        let (nodes, root) = build_bsp(faces);
        CollModel::new(faces, &nodes, root)
    }

    pub fn face_count(&self) -> usize {
        self.set.faces.len()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Axis-aligned bounds of the model's corners.
    pub fn bounds(&self) -> (V3, V3) {
        (self.lo, self.hi)
    }
}

/// Builds a BSP over convex polygons: each node's split plane is its polygon's plane; polygons that straddle
/// a split plane are referenced from both subtrees (the ROM's BSPs do the same: they hold about three nodes
/// per polygon). Returns nodes `(face, negative child, positive child)` and the root.
pub fn build_bsp(faces: &[ConvexPoly]) -> (Vec<(u32, Child, Child)>, Child) {
    fn rec(
        faces: &[ConvexPoly],
        list: &[u32],
        out: &mut Vec<(u32, Child, Child)>,
        depth: u32,
    ) -> Child {
        let (&first, rest) = list.split_first()?;
        let idx = out.len() as u32;
        out.push((first, None, None));
        let split = &faces[first as usize];
        let mut neg = Vec::new();
        let mut pos = Vec::new();
        for &o in rest {
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for v in &faces[o as usize].verts {
                let d = split.normal.dot(*v) - split.dist;
                lo = lo.min(d);
                hi = hi.max(d);
            }
            const EPS: f32 = 0.01;
            if hi > EPS {
                pos.push(o);
            }
            if lo < -EPS {
                neg.push(o);
            }
            if hi <= EPS && lo >= -EPS {
                pos.push(o); // coplanar
            }
        }
        if depth > 4096 {
            return Some(idx);
        }
        let n = rec(faces, &neg, out, depth + 1);
        let p = rec(faces, &pos, out, depth + 1);
        out[idx as usize].1 = n;
        out[idx as usize].2 = p;
        Some(idx)
    }
    let list: Vec<u32> = (0..faces.len() as u32).collect();
    let mut out = Vec::new();
    let root = rec(faces, &list, &mut out, 0);
    (out, root)
}

/// Static collision geometry: world models and the blocker soup.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollisionWorld {
    pub models: Vec<CollModel>,
    blockers: PolySet,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Pos,
    Neg,
    Touch,
}

/// Point classification of the BSP walk.
// SPEC: MOVEMENT_2009.md §9.8 (distance > Rb -> positive, < -Rb -> negative, else positive when > off,
// touching when >= -off, negative otherwise)
fn classify(d: f32, rb: f32, off: f32) -> Side {
    if d > rb {
        Side::Pos
    } else if d < -rb {
        Side::Neg
    } else if d > off {
        Side::Pos
    } else if d >= -off {
        Side::Touch
    } else {
        Side::Neg
    }
}

/// Small explicit stack for the BSP walk (spills to the heap only when deeper than 64).
struct WalkStack {
    inline: [u32; 64],
    len: usize,
    spill: Vec<u32>,
}

impl WalkStack {
    fn new() -> Self {
        WalkStack {
            inline: [0; 64],
            len: 0,
            spill: Vec::new(),
        }
    }
    fn push(&mut self, v: u32) {
        if self.len < self.inline.len() {
            self.inline[self.len] = v;
            self.len += 1;
        } else {
            self.spill.push(v);
        }
    }
    fn pop(&mut self) -> Option<u32> {
        if let Some(v) = self.spill.pop() {
            return Some(v);
        }
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(self.inline[self.len])
    }
}

/// Used only by the static overlap test for polygons with more corners than its clip buffers hold (the sweep
/// follows the 2009 edge rule of [`edge_sweep`]).
///
/// The half-spaces that every point of the shape's centre must satisfy for the shape to touch face `f`
/// (each is a necessary condition, so their intersection is a conservative stand-in for the exact Minkowski
/// sum): `dot(m, centre) - md <= sup`. Per edge: the in-plane edge plane (`normalize(cross(edge, n))`), the
/// vertical plane through the edge (support = radius), and for a sloped face and a non-vertical edge the
/// plane through the edge along the shape's support offset `k` (the §9.9 step 4 plane).
// SPEC: MOVEMENT_2009.md §9.9 step 4 (edge planes; sloped variant normalize(cross(edge, k)))
fn edge_constraints(
    set: &PolySet,
    f: &Face,
    shape: &Cylinder,
    mut each: impl FnMut(V3, f32, f32) -> bool,
) -> bool {
    let k = shape.support_offset(f.normal);
    for i in f.first..f.first + f.count {
        let (en, ed) = set.edges[i as usize];
        let a = set.verts[i as usize];
        if !each(en, ed, shape.support(en)) {
            return false;
        }
        let next = if i + 1 == f.first + f.count {
            f.first
        } else {
            i + 1
        };
        let edge = set.verts[next as usize] - a;
        let el = edge.len();
        // vertical plane containing the edge (the polygon lies on its inner side); skipped when it is the
        // face plane itself or the edge is vertical
        let mut hv = edge.cross(V3::UP).normalized();
        let side = hv.dot(en);
        if !hv.is_zero() && crate::fmath::abs(side) > 1e-4 {
            if side < 0.0 {
                hv = -hv;
            }
            if !each(hv, hv.dot(a), shape.radius) {
                return false;
            }
        }
        if f.sloped && !k.is_zero() && el > 0.0 && crate::fmath::abs(edge.y) <= 0.9999 * el {
            let mut mk = edge.cross(k).normalized();
            if mk.dot(en) < 0.0 {
                mk = -mk;
            }
            if !mk.is_zero() && !each(mk, mk.dot(a), shape.support(mk)) {
                return false;
            }
        }
    }
    true
}

/// Largest finite binary32: the "never" / "always" time of the 2009 interval code (the constants at engine VA
/// 0x675b6c / 0x675b68).
// DISASM: engine+0x7683d (movss xmm7, [0x675b68]; movss xmm7, [0x675b6c])
const NEVER: f32 = f32::MAX;

/// Sweep of a point against an infinite vertical cylinder of radius `r` around the axis through `c`; the hit
/// time is the first root in [0, 1) (0 when the segment starts inside), the normal is horizontal and radial.
// EVIDENCE: engine+0x75520 (quadratic in x / z, tangent case, start-inside case, normal (p - c).xz)
fn axis_sweep(c: V3, s: V3, e: V3, r: f32) -> Option<(f32, V3)> {
    let (fx, fz) = (s.x - c.x, s.z - c.z);
    let (dx, dz) = (e.x - s.x, e.z - s.z);
    let a = dz * dz + dx * dx;
    let c0 = fz * fz + fx * fx;
    let radial = |px: f32, pz: f32| V3::new(px - c.x, 0.0, pz - c.z).normalized();
    if a >= 0.0001 {
        let b = fz * dz + fx * dx;
        let disc = b * b - (c0 - r * r) * a;
        let mut t;
        if disc <= 0.0 {
            if disc < 0.0 {
                return None;
            }
            t = -(b / a);
            if t < 0.0 {
                return None;
            }
        } else {
            let root = crate::fmath::sqrt(disc);
            let t0 = (-b - root) * (1.0 / a);
            let t1 = (root - b) * (1.0 / a);
            if t0 < 0.0 && t1 < 0.0 {
                return None;
            }
            t = if t0 < t1 { t0 } else { t1 };
        }
        if t >= 0.0 {
            if t >= 1.0 {
                return None;
            }
        } else {
            t = 0.0;
        }
        Some((t, radial(dx * t + s.x, dz * t + s.z)))
    } else if c0 <= r * r {
        Some((0.0, radial(s.x, s.z)))
    } else {
        None
    }
}

/// Sweep of a point against a finite vertical cylinder (axis through `base`, from `base.y` up by `height`):
/// the side when the hit lies strictly inside the height band, else the cap the segment crosses (normal
/// straight up or down).
// EVIDENCE: engine+0x75f40 (band test, then the cap: the bottom one first when moving up, the top one first
// when moving down, the other one when the first is not crossed within [0, 1); strict radius test)
fn vertical_cylinder_sweep(base: V3, height: f32, s: V3, e: V3, r: f32) -> Option<(f32, V3)> {
    let y0 = base.y;
    let y1 = height + y0;
    let lo = if e.y < s.y { e.y } else { s.y };
    let hi = if s.y < e.y { e.y } else { s.y };
    if !(lo <= y1 && y0 <= hi) {
        return None;
    }
    let side = axis_sweep(base, s, e, r)?;
    let y = (e.y - s.y) * side.0 + s.y;
    if !(y <= y0 || y1 <= y) {
        return Some(side);
    }
    if s.y == e.y {
        return None;
    }
    let up = s.y < e.y;
    let (mut cap, mut from_below) = if up { (y0, true) } else { (y1, false) };
    let mut t = (cap - s.y) / (e.y - s.y);
    if !(0.0..1.0).contains(&t) {
        (cap, from_below) = if up { (y1, false) } else { (y0, true) };
        t = (cap - s.y) / (e.y - s.y);
        if !(0.0..1.0).contains(&t) {
            return None;
        }
    }
    let _ = cap;
    let dz = ((e.z - s.z) * t + s.z) - base.z;
    let dx = ((e.x - s.x) * t + s.x) - base.x;
    if dz * dz + dx * dx < r * r {
        let ny = if from_below { -1.0 } else { 1.0 };
        Some((t, V3::new(0.0, ny, 0.0)))
    } else {
        None
    }
}

/// `normalize(cross(cross(e, v), e))`: the part of `v` perpendicular to the edge direction `e`, as a unit
/// vector (zero for a zero vector).
// EVIDENCE: engine+0x76120 tail (normal of a rim contact: two cross products with the edge, then engine+0x753c0)
fn perpendicular_to_edge(e: V3, v: V3) -> V3 {
    e.cross(v).cross(e).normalized()
}

/// Exit / entry bounds of the interval code of [`edge_sweep`].
fn exit_at(t: f32, hi: &mut f32) {
    if t <= *hi {
        *hi = t;
    }
}

fn enter_at(t: f32, lo: &mut f32) {
    if *lo <= t {
        *lo = t;
    }
}

/// Time of a slab bound `offset(t) = bound` with `offset(t) = off0 + rate t` (the 2009 form
/// `(off0 - bound) * (-1 / rate)`); for `rate == 0` "always" (`NEVER`) when `inside`, else "never"
/// (`-NEVER`).
fn bound_time(off0: f32, bound: f32, rate: f32, inside: bool) -> f32 {
    if rate == 0.0 {
        if inside {
            NEVER
        } else {
            -NEVER
        }
    } else {
        (off0 - bound) * (-1.0 / rate)
    }
}

/// Swept cylinder against one polygon edge `a -> a + e` of the face with plane `(n, dist)`: the cylinder's
/// side against the edge, its caps against the edge, the edge's end vertex, or (when the cylinder already
/// encloses part of the edge at the start) the start itself. Lowers `t_best` and sets `normal` when it finds
/// an earlier contact. Returns false only when the edge cannot be touched at all (the polygon is then missed
/// unless another edge refines the time).
// EVIDENCE: engine+0x76120 (called by engine+0x77530 for every edge plane the contact point lies outside of)
// DISASM: engine+0x76871..0x76878 (for a move parallel to the band plane the lower band bound compares the
// along-edge offset, as written in the binary), engine+0x769e3..0x769f9 (the band side is chosen by
// `e.z n.x - e.x n.z <= -0.001`, EDX = the face plane)
#[allow(clippy::too_many_arguments)]
fn edge_sweep(
    n: V3,
    ndist: f32,
    a: V3,
    e: V3,
    s: V3,
    d: V3,
    shape: &Cylinder,
    t_best: &mut f32,
    normal: &mut V3,
) -> bool {
    use crate::fmath::abs;
    let r = shape.radius;
    let h = shape.half_height;
    let end = s + d;
    // a vertical edge: the cylinder of radius r around it, grown by h at both ends
    if abs(e.x) < 0.001 && abs(e.z) < 0.001 {
        let mut by = a.y;
        if e.y < 0.0 {
            by += e.y;
        }
        let base = V3::new(a.x, by - h, a.z);
        return match vertical_cylinder_sweep(base, h + h + abs(e.y), s, end, r) {
            None => false,
            Some((t, nn)) => {
                if t < *t_best {
                    *t_best = t;
                    *normal = nn;
                }
                true
            }
        };
    }
    // w: horizontal, perpendicular to the edge, pointing out of the polygon (the face normal for a wall)
    let mut w = if abs(n.y) <= 0.001 {
        V3::new(n.x, 0.0, n.z)
    } else {
        let sg = if n.y <= 0.0 { -1.0 } else { 1.0 };
        let w = V3::new(-(sg * e.z), 0.0, e.x * sg);
        if e.cross(n).dot(w) <= 0.0 {
            -w
        } else {
            w
        }
    };
    w = w * (1.0 / w.len());
    let wa = a.dot(w);
    let sw = s.dot(w) - wa;
    let dw = d.dot(w);
    let t_w = bound_time(sw, r, dw, sw < r);
    // u: the edge's horizontal direction; the span along it, grown by r
    let u = V3::new(e.x, 0.0, e.z) * (1.0 / crate::fmath::sqrt(e.x * e.x + e.z * e.z));
    let ua = a.dot(u);
    let su = s.dot(u) - ua;
    let du = d.dot(u);
    let inv_du = -1.0 / du;
    let t_u0 = bound_time(su, -r, du, -r < su);
    let span = e.dot(u);
    let t_u1 = bound_time(su, r + span, du, su < r + span);
    // m: normal of the plane through the edge and w (pointing up); the band around it, grown by the support
    let mut m = e.cross(w);
    m = m * (1.0 / m.len());
    let mut ma = a.dot(m);
    if m.y < 0.0 {
        m = -m;
        ma = -ma;
    }
    let band = h * m.y;
    let dm = d.dot(m);
    let rim = abs(m.dot(u)) * r;
    let sup_m = band + rim;
    let sm = s.dot(m) - ma;
    let inv_dm = -1.0 / dm;
    let t_m0 = bound_time(sm, sup_m, dm, sm < sup_m);
    // DISASM: engine+0x76871..0x76878 (the parallel case compares the along-edge offset su)
    let mut t_m1 = bound_time(sm, -sup_m, dm, -sup_m < su);
    // the face slab
    let sn = s.dot(n) - ndist;
    let dn = d.dot(n);
    let sup_n = shape.support(n);
    let t_n0 = bound_time(sn, sup_n, dn, sn < sup_n);
    let t_n1 = bound_time(sn, -sup_n, dn, -sup_n < sn);
    // intersect the constraints
    let mut lo = -NEVER;
    let mut hi = NEVER;
    if 0.0 <= dw {
        if t_w <= NEVER {
            hi = t_w;
        }
    } else if -NEVER <= t_w {
        lo = t_w;
    }
    if du <= 0.0 {
        exit_at(t_u0, &mut hi);
    } else {
        enter_at(t_u0, &mut lo);
    }
    if 0.0 <= du {
        exit_at(t_u1, &mut hi);
    } else {
        enter_at(t_u1, &mut lo);
    }
    if e.z * n.x - e.x * n.z <= -0.001 {
        if dm <= 0.0 {
            exit_at(t_m1, &mut hi);
        } else {
            enter_at(t_m1, &mut lo);
        }
    } else if 0.0 <= dm {
        exit_at(t_m0, &mut hi);
    } else {
        enter_at(t_m0, &mut lo);
    }
    if 0.0 <= dn {
        exit_at(t_n0, &mut hi);
    } else {
        enter_at(t_n0, &mut lo);
    }
    if dn <= 0.0 {
        exit_at(t_n1, &mut hi);
    } else {
        enter_at(t_n1, &mut lo);
    }
    if hi <= lo || 1.0 <= lo {
        return false;
    }
    // entry into the edge's span [0, span] and into the band |offset along m| <= h m.y
    let (mut in_span0, in_span1);
    if du == 0.0 {
        in_span0 = t_u0;
        let mut x = t_u1;
        if t_u1 == NEVER {
            if 0.0 < su && su < span {
                x = -NEVER;
            }
            in_span0 = -x;
        }
        in_span1 = x;
    } else {
        let q = r * inv_du;
        in_span0 = t_u0 - q;
        in_span1 = q + t_u1;
    }
    let mut in_band;
    if dm == 0.0 {
        in_band = t_m0;
        if t_m1 == NEVER {
            if sm < band && -band < sm {
                t_m1 = -NEVER;
            }
            in_band = -t_m1;
        }
    } else {
        t_m1 -= inv_dm * rim;
        in_band = inv_dm * rim + t_m0;
    }
    if du <= 0.0 {
        in_span0 = in_span1;
    }
    if 0.0 <= dm {
        in_band = t_m1;
    }
    // the cylinder's side against the edge
    if in_span0 <= t_w && in_band <= t_w && dw < 0.0 && 0.0 <= t_w {
        let c = s + d * t_w;
        let cm = c.dot(m) - ma;
        let cu = c.dot(u) - ua;
        if cm <= band && -band <= cm && cu <= span && 0.0 <= cu {
            if t_w < *t_best {
                *t_best = t_w;
                *normal = w;
            }
            return true;
        }
    }
    // the end vertex the move comes from
    let mut found = false;
    let (toward_a, rate) = if du == 0.0 {
        (0.0 < su, su)
    } else {
        (du < 0.0, du)
    };
    let v = if toward_a || rate == 0.0 { a + e } else { a };
    let hv = h + 0.001;
    if let Some((t, nn)) = vertical_cylinder_sweep(V3::new(v.x, v.y - hv, v.z), hv * 2.0, s, end, r)
    {
        if t < *t_best {
            *t_best = t;
            *normal = nn;
        }
        found = true;
    }
    // a cap of the cylinder against the edge
    let above = sm > 0.0;
    let cap_y = (if above { h } else { -h }) + a.y;
    if e.y == 0.0 {
        if d.y != 0.0 {
            let mut t = s.y - cap_y;
            if t != 0.0 {
                t = -(t / d.y);
            }
            if t <= 0.0 {
                t = 0.0;
            }
            let c = s + d * t;
            let cu = c.dot(u) - ua;
            if abs(c.dot(w) - wa) < r && 0.0 < cu && cu < span {
                if *t_best <= t {
                    return true;
                }
                *t_best = t;
                *normal = V3::new(0.0, if above { 1.0 } else { -1.0 }, 0.0);
                return true;
            }
        }
    } else {
        // shear the problem so that the sloped edge becomes vertical
        let kx = e.x * (1.0 / e.y);
        let kz = (1.0 / e.y) * e.z;
        let lift = s.y - cap_y;
        let p = V3::new(s.x - lift * kx, s.y, s.z - lift * kz);
        let q = p + V3::new(d.x - d.y * kx, d.y, d.z - d.y * kz);
        let low = if e.y < 0.0 { e.y } else { 0.0 };
        let base = V3::new(a.x, low + cap_y, a.z);
        if let Some((t, _)) = vertical_cylinder_sweep(base, abs(e.y), p, q, r) {
            if *t_best <= t {
                return true;
            }
            *t_best = t;
            let c = s + d * t;
            let k = (c - a).dot(e) / e.len_sq();
            let on_edge = a + e * k;
            *normal = perpendicular_to_edge(e, c - on_edge);
            return true;
        }
    }
    if found {
        return true;
    }
    // the edge already reaches into the cylinder at the start: the edge parameter where it enters is taken as
    // the time (as written in the binary)
    let base = V3::new(s.x, s.y - h, s.z);
    match vertical_cylinder_sweep(base, h + h, a, a + e, r) {
        None => false,
        Some((te, _)) => {
            if *t_best <= te {
                return true;
            }
            *t_best = te;
            let k = ((s - a).dot(e) / e.len_sq()).clamp(0.0, 1.0);
            let on_edge = a + e * k;
            *normal = perpendicular_to_edge(e, s - on_edge);
            true
        }
    }
}

/// Swept cylinder against one polygon. `d = e - s`; returns the hit time (fraction of `d`) when below
/// `best`, and the hit normal (the face normal, or the normal of the edge / vertex / cap contact).
// SPEC: MOVEMENT_2009.md §9.9 steps 1-5
// EVIDENCE: engine+0x77530 (front side only; slab entry / exit; a move parallel to the face never hits it;
// edge planes normalize(cross(edge, n)), or normalize(cross(edge, k)) for a sloped face and a non-vertical
// edge, edges walked from the last corner; a contact point outside an edge plane by more than that plane's
// support misses, by less calls the edge sweep engine+0x76120; when no edge lowered the exit time the face
// is hit at its entry time unless an edge could not be touched; recorded when below best and moving into the
// final normal, at max(t, 0))
fn face_sweep(
    set: &PolySet,
    fi: u32,
    shape: &Cylinder,
    s: V3,
    d: V3,
    best: f32,
) -> Option<(f32, V3)> {
    let f = &set.faces[fi as usize];
    // step 1: bounding-sphere pre-filter (exact closest point, todo::SPHERE_PREFILTER_EXACT)
    let c = f.center - s;
    let dd = d.len_sq();
    let mut u = if dd > 0.0 { c.dot(d) / dd } else { 0.0 };
    u = u.clamp(0.0, 1.0);
    let q = c - d * u;
    let reach = f.radius + shape.bound;
    if q.len_sq() > reach * reach {
        return None;
    }
    // step 2: only from the front side
    let n = f.normal;
    let ds = n.dot(s) - f.dist;
    if ds < 0.0 {
        return None;
    }
    // step 3: slab entry / exit; a move parallel to the face never hits it
    let off = shape.support(n);
    let vn = n.dot(d);
    if vn == 0.0 {
        return None;
    }
    let mut t_in = (ds - off) * (-1.0 / vn);
    let t_out = (ds + off) * (-1.0 / vn);
    if t_in >= best || t_out < 0.0 {
        return None;
    }
    if t_in < 0.0 {
        t_in = 0.0;
    }
    // step 4: contact point against the edge planes
    let p = s + d * t_in;
    let k = shape.support_offset(n);
    let corners = &set.verts[f.first as usize..(f.first + f.count) as usize];
    let &last = corners.last()?;
    let mut t_hit = t_out;
    let mut nn = n;
    let mut missed = false;
    let mut prev = last;
    for &cur in corners {
        let edge = cur - prev;
        let vertical = crate::fmath::abs(edge.x) <= 0.001 && crate::fmath::abs(edge.z) <= 0.001;
        // a zero-size shape (rays) has no support offset: its edge planes are the in-plane ones (the 2009
        // body always has r, h > 0; its cross(edge, 0) would be NaN)
        // DC-DEFAULT: SIM-8 (proposed) a zero-size sweep bounds a sloped face by its in-plane edge planes
        let m = if f.sloped && !vertical && !k.is_zero() {
            edge.cross(k).normalized()
        } else {
            edge.cross(n).normalized()
        };
        let outside = m.dot(p) - m.dot(cur);
        if 0.0 < outside {
            if shape.support(m) < outside {
                return None;
            }
            if !edge_sweep(n, f.dist, prev, edge, s, d, shape, &mut t_hit, &mut nn) {
                missed = true;
            }
        }
        prev = cur;
    }
    if t_hit == t_out {
        if missed {
            return None;
        }
        t_hit = t_in;
    }
    // step 5: record when earlier than best and moving into the final normal
    if t_hit < best && d.dot(nn) < 0.0 {
        Some((if t_hit > 0.0 { t_hit } else { 0.0 }, nn))
    } else {
        None
    }
}

/// Exact static overlap of a vertical cylinder at `at` with one polygon, shrunk by a small tolerance so that
/// resting contacts 0.1 units away do not count: clip the polygon to the cylinder's height band, project the
/// rest onto XZ (a convex polygon) and compare its distance from the axis with the radius. Both sides of
/// the polygon count (a box that crosses a polygon does not fit, whichever way it faces).
const CLIP_CAP: usize = 260;

fn face_overlap(set: &PolySet, fi: u32, shape: &Cylinder, at: V3) -> bool {
    const TOL: f32 = 0.01;
    let f = &set.faces[fi as usize];
    if f.flags & SURF_SOLID == 0 {
        return false;
    }
    let lo = at.y - (shape.half_height - TOL);
    let hi = at.y + (shape.half_height - TOL);
    let r = shape.radius - TOL;
    let ds = f.normal.dot(at) - f.dist;
    if crate::fmath::abs(ds) >= shape.support(f.normal) - TOL || r < 0.0 {
        return false;
    }
    // Sutherland-Hodgman against y >= lo and y <= hi, in fixed buffers (a convex polygon gains at most one
    // corner per clip; the file format stores at most 255 corners, larger hand-built polygons are rejected
    // by the conservative half-space test instead)
    let corners = &set.verts[f.first as usize..(f.first + f.count) as usize];
    if corners.len() > CLIP_CAP - 2 {
        return edge_constraints(set, f, shape, |m, md, sup| m.dot(at) - md < sup - TOL);
    }
    let mut a = [V3::ZERO; CLIP_CAP];
    let mut b = [V3::ZERO; CLIP_CAP];
    a[..corners.len()].copy_from_slice(corners);
    let mut na = corners.len();
    for (keep_above, level) in [(true, lo), (false, hi)] {
        let inside = |p: V3| {
            if keep_above {
                p.y >= level
            } else {
                p.y <= level
            }
        };
        let mut nb = 0;
        for i in 0..na {
            let p = a[i];
            let q = a[(i + 1) % na];
            let (pi, qi) = (inside(p), inside(q));
            if pi {
                b[nb] = p;
                nb += 1;
            }
            if pi != qi {
                let t = (level - p.y) / (q.y - p.y);
                b[nb] = p + (q - p) * t;
                nb += 1;
            }
        }
        core::mem::swap(&mut a, &mut b);
        na = nb;
        if na == 0 {
            return false;
        }
    }
    let a = &a[..na];
    // 2D distance from the axis to the projected convex polygon
    let (cx, cz) = (at.x, at.z);
    let n = a.len();
    let mut sign = 0i32;
    let mut inside = n >= 3;
    let mut best = f32::INFINITY;
    for i in 0..n {
        let p = a[i];
        let q = a[(i + 1) % n];
        let (ex, ez) = (q.x - p.x, q.z - p.z);
        let (wx, wz) = (cx - p.x, cz - p.z);
        let cr = ex * wz - ez * wx;
        let s = if cr > 0.0 {
            1
        } else if cr < 0.0 {
            -1
        } else {
            0
        };
        if s != 0 {
            if sign == 0 {
                sign = s;
            } else if s != sign {
                inside = false;
            }
        }
        let el = ex * ex + ez * ez;
        let t = if el > 0.0 {
            ((wx * ex + wz * ez) / el).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (dx, dz) = (wx - ex * t, wz - ez * t);
        let d2 = dx * dx + dz * dz;
        if d2 < best {
            best = d2;
        }
    }
    if inside && sign != 0 {
        return true;
    }
    best < r * r
}

impl CollModel {
    /// BSP sweep of one world model; returns `(t, normal, face)`.
    // SPEC: MOVEMENT_2009.md §9.8 (engine+0x77db0)
    fn sweep(&self, shape: &Cylinder, s: V3, e: V3) -> Option<(f32, V3, u32)> {
        let d = e - s;
        let len = d.len();
        let mid = (s + e) * 0.5;
        let rb = shape.bound;
        let mut best = 1.0f32;
        let mut found = None;
        let mut stack = WalkStack::new();
        let mut node = self.root;
        loop {
            if node == END {
                match stack.pop() {
                    Some(n) => node = n,
                    None => break,
                }
                continue;
            }
            let nd = self.nodes[node as usize];
            let Some(f) = self.set.faces.get(nd.face as usize) else {
                node = END;
                continue;
            };
            let n = f.normal;
            let c = n.dot(mid) - f.dist;
            if c > len + rb {
                node = nd.pos;
                continue;
            }
            if c < -(len + rb) {
                node = nd.neg;
                continue;
            }
            let off = shape.support(n);
            let ds = n.dot(s) - f.dist;
            let de = n.dot(e) - f.dist;
            let cs = classify(ds, rb, off);
            let ce = classify(de, rb, off);
            if cs == ce && cs != Side::Touch {
                node = if cs == Side::Pos { nd.pos } else { nd.neg };
                continue;
            }
            let s_front = cs == Side::Pos || cs == Side::Touch;
            if s_front && f.flags & SURF_SOLID != 0 && n.dot(d) < -0.0001 * len {
                if let Some((t, nn)) = face_sweep(&self.set, nd.face, shape, s, d, best) {
                    best = t;
                    found = Some((t, nn, nd.face));
                }
            }
            let (first, other) = if ds > -0.001 {
                (nd.pos, nd.neg)
            } else {
                (nd.neg, nd.pos)
            };
            if other != END {
                stack.push(other);
            }
            node = first;
        }
        found
    }

    fn overlaps(&self, shape: &Cylinder, at: V3) -> bool {
        let rb = shape.bound;
        let mut stack = WalkStack::new();
        let mut node = self.root;
        loop {
            if node == END {
                match stack.pop() {
                    Some(n) => node = n,
                    None => return false,
                }
                continue;
            }
            let nd = self.nodes[node as usize];
            let Some(f) = self.set.faces.get(nd.face as usize) else {
                node = END;
                continue;
            };
            let ds = f.normal.dot(at) - f.dist;
            match classify(ds, rb, shape.support(f.normal)) {
                Side::Pos => node = nd.pos,
                Side::Neg => node = nd.neg,
                Side::Touch => {
                    if face_overlap(&self.set, nd.face, shape, at) {
                        return true;
                    }
                    if nd.neg != END {
                        stack.push(nd.neg);
                    }
                    node = nd.pos;
                }
            }
        }
    }

    fn touches_box(&self, lo: V3, hi: V3) -> bool {
        !(hi.x < self.lo.x
            || hi.y < self.lo.y
            || hi.z < self.lo.z
            || lo.x > self.hi.x
            || lo.y > self.hi.y
            || lo.z > self.hi.z)
    }

    /// The surface flags of a face.
    pub fn face_flags(&self, face: u32) -> u32 {
        self.set.faces.get(face as usize).map_or(0, |f| f.flags)
    }

    /// The plane normal of a face.
    pub fn face_normal(&self, face: u32) -> V3 {
        self.set
            .faces
            .get(face as usize)
            .map_or(V3::ZERO, |f| f.normal)
    }
}

impl CollisionWorld {
    /// From world models and blocker polygons.
    pub fn from_parts(models: Vec<CollModel>, blockers: &[ConvexPoly]) -> CollisionWorld {
        let mut set = PolySet::default();
        for b in blockers {
            set.push(b.normal, b.dist, &b.verts, b.flags | SURF_SOLID);
        }
        CollisionWorld {
            models,
            blockers: set,
        }
    }

    pub fn blocker_count(&self) -> usize {
        self.blockers.faces.len()
    }

    /// A ray (zero-size shape) from `from` to `to`.
    pub fn ray(&self, from: V3, to: V3) -> Option<Hit> {
        self.sweep(&Cylinder::POINT, from, to)
    }
}

impl Collider for CollisionWorld {
    // SPEC: MOVEMENT_2009.md §9.6 (two stages: blocker polygons, then objects with the 0.1 back-off)
    fn sweep(&self, shape: &Cylinder, p: V3, e: V3) -> Option<Hit> {
        let d = e - p;
        let len = d.len();
        if len.is_nan() || len <= 0.0 {
            return None;
        }
        let mut t = 1.0f32;
        let mut hit: Option<Hit> = None;
        // stage 1: blocker polygons (todo::BLOCKERS_AS_CYLINDER_FACES), skipped for tiny moves
        if todo::BLOCKERS_AS_CYLINDER_FACES && len >= 0.0001 && !self.blockers.faces.is_empty() {
            let mid = (p + e) * 0.5;
            let reach = len * 0.5 + shape.bound;
            let mut best = 1.0f32;
            let mut bf = None;
            for (i, f) in self.blockers.faces.iter().enumerate() {
                let r = reach + f.radius;
                if (f.center - mid).len_sq() > r * r {
                    continue;
                }
                if let Some((tb, nn)) = face_sweep(&self.blockers, i as u32, shape, p, d, best) {
                    best = tb;
                    bf = Some((i as u32, nn));
                }
            }
            if let Some((i, nn)) = bf {
                // the nearest hit distance minus 0.001 becomes t |E - P|
                let mut tb = (best * len - 0.001) / len;
                if tb < 0.0 {
                    tb = 0.0;
                }
                t = tb;
                hit = Some(Hit {
                    t,
                    normal: nn,
                    face_normal: self.blockers.faces[i as usize].normal,
                    surface_flags: self.blockers.faces[i as usize].flags,
                    model: BLOCKER_MODEL,
                    poly: i,
                });
            }
        }
        // stage 2: world models on the (possibly shortened) segment, each hit backed off 0.1 along N
        let u = d * (1.0 / len);
        for (mi, m) in self.models.iter().enumerate() {
            if t <= 0.0 {
                break;
            }
            let cur = p + d * t;
            let pad = shape.bound + 0.5;
            let lo =
                V3::new(p.x.min(cur.x), p.y.min(cur.y), p.z.min(cur.z)) - V3::new(pad, pad, pad);
            let hi =
                V3::new(p.x.max(cur.x), p.y.max(cur.y), p.z.max(cur.z)) + V3::new(pad, pad, pad);
            if !m.touches_box(lo, hi) {
                continue;
            }
            if let Some((tp, nn, face)) = m.sweep(shape, p, cur) {
                t *= tp;
                let q = -nn.dot(u);
                if q > 0.001 {
                    let back = (0.1 / q) / len;
                    t -= back.clamp(0.0, t);
                }
                hit = Some(Hit {
                    t,
                    normal: nn,
                    face_normal: m.face_normal(face),
                    surface_flags: m.face_flags(face),
                    model: mi as u32,
                    poly: face,
                });
            }
        }
        hit.map(|h| Hit { t, ..h })
    }

    fn overlaps(&self, shape: &Cylinder, at: V3) -> bool {
        let r = V3::new(shape.bound, shape.bound, shape.bound);
        for m in &self.models {
            if m.touches_box(at - r, at + r) && m.overlaps(shape, at) {
                return true;
            }
        }
        (0..self.blockers.faces.len() as u32).any(|i| face_overlap(&self.blockers, i, shape, at))
    }
}

// ---- character bodies: the object stage's box test (w1-bodies) ----

/// Model index used in a [`Hit`] that came from a character body of a [`BodySet`]; the hit's `poly` is the body's
/// `id`.
pub const BODY_MODEL: u32 = u32::MAX - 1;

/// Most bodies a [`BodySet`] holds. The set is a fixed array, so rebuilding it every tick never allocates; bodies
/// pushed beyond the capacity are ignored (in push order, so the result stays deterministic).
// DC-DEFAULT: SIM-14 (proposed) capacity 32 (more than the player slots of any 2009 room)
pub const MAX_BODIES: usize = 32;

/// One character body for the object stage: an axis-aligned box (centre, half extents) as the engine keeps it for
/// every non-world-model object (object +0xc8 position, +0xb8 half extents).
// EVIDENCE: engine+0x781c0 (other objects: the box test with object +0xc8 and +0xb8)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    /// The caller's key (the remote's object id); returned as [`Hit::poly`].
    pub id: u32,
    pub centre: V3,
    pub half: V3,
}

const NO_BODY: Body = Body {
    id: 0,
    centre: V3::new(0.0, 0.0, 0.0),
    half: V3::new(0.0, 0.0, 0.0),
};

/// The character bodies the own mover collides with this tick: bounded ([`MAX_BODIES`]), in push order, rebuilt by
/// [`BodySet::clear`] + [`BodySet::push`] without allocation.
#[derive(Debug, Clone, PartialEq)]
pub struct BodySet {
    items: [Body; MAX_BODIES],
    len: usize,
}

impl Default for BodySet {
    fn default() -> Self {
        BodySet {
            items: [NO_BODY; MAX_BODIES],
            len: 0,
        }
    }
}

impl BodySet {
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Adds a body; false (ignored) when the set is full or the box is not finite and positive.
    pub fn push(&mut self, body: Body) -> bool {
        let ok = body.centre.is_finite()
            && body.half.is_finite()
            && body.half.x > 0.0
            && body.half.y > 0.0
            && body.half.z > 0.0;
        if !ok || self.len >= MAX_BODIES {
            return false;
        }
        self.items[self.len] = body;
        self.len += 1;
        true
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[Body] {
        &self.items[..self.len]
    }
}

/// The corner axis of the box nearest to `s` when `s` lies outside the box's own x or z extent, swept as a vertical
/// cylinder of the mover's radius (`axis_sweep`); `None` when `s` is inside both extents or nothing is hit.
// EVIDENCE: engine+0x757b0 (|s.x - c.x| >= dx or |s.z - c.z| >= dz: corner (c.x +- dx, 0, c.z +- dz) on the side of
// `s`, `>= 0` choosing +, then engine+0x75520 with the mover's radius)
fn body_corner_sweep(b: &Body, s: V3, e: V3, r: f32) -> Option<(f32, V3)> {
    let rx = s.x - b.centre.x;
    let rz = s.z - b.centre.z;
    if !(b.half.x <= crate::fmath::abs(rx) || b.half.z <= crate::fmath::abs(rz)) {
        return None;
    }
    let cx = if rx >= 0.0 {
        b.centre.x + b.half.x
    } else {
        b.centre.x - b.half.x
    };
    let cz = if rz >= 0.0 {
        b.centre.z + b.half.z
    } else {
        b.centre.z - b.half.z
    };
    axis_sweep(V3::new(cx, 0.0, cz), s, e, r)
}

/// Sweep of the mover's centre from `s` to `e` against one body: the body's box grown by the mover's radius in x / z
/// and by its half height in y, slab by slab; a hit on an x / z face beyond the box's own extent in the other
/// horizontal axis is refined against the rounded corner. A start inside the grown box blocks only a move towards
/// the body's centre (time 0, normal against the move) unless the start lies in a corner region outside the rounded
/// corner. Result: the time in [0, 1) and the contact normal.
// EVIDENCE: engine+0x75860 (grown box: x / z by max(r, r) = r, y by h; per axis the low / high slab plane and its
// time, -1 when the axis has no motion; all three inside: `dot(s - c, d) >= 0` no hit, else the corner sweep, else
// no hit in a corner region (|s.x - c.x| >= dx and |s.z - c.z| >= dz), else time 0 with normal -d/|d|; otherwise the
// largest slab time, a hit only for 0 < t < 1 with the other two coordinates inside the grown box; the y slab or a
// point within dx (x) or dz (z) of the centre is a face hit with the axis normal (+1 when the plane lies above the
// centre's coordinate, else -1), else the corner sweep from the face point, its time mapped to `t + (1 - t) t'`)
fn body_sweep(b: &Body, shape: &Cylinder, s: V3, e: V3) -> Option<(f32, V3)> {
    let r = shape.radius;
    let grow = [r, shape.half_height, r];
    let c = b.centre.to_array();
    let half = b.half.to_array();
    let sa = s.to_array();
    let d = (e - s).to_array();
    let mut lo = [0.0f32; 3];
    let mut hi = [0.0f32; 3];
    for i in 0..3 {
        lo[i] = c[i] - (half[i] + grow[i]);
        hi[i] = c[i] + half[i] + grow[i];
    }
    let mut tc = [-1.0f32; 3];
    let mut at = [0.0f32; 3];
    let mut inside = true;
    for i in 0..3 {
        let plane = if sa[i] < lo[i] {
            lo[i]
        } else if hi[i] < sa[i] {
            hi[i]
        } else {
            continue;
        };
        inside = false;
        at[i] = plane;
        if d[i] != 0.0 {
            tc[i] = (plane - sa[i]) / d[i];
        }
    }
    if inside {
        let rel = s - b.centre;
        let dv = e - s;
        if rel.y * dv.y + rel.z * dv.z + rel.x * dv.x >= 0.0 {
            return None;
        }
        if let Some(h) = body_corner_sweep(b, s, e, r) {
            return Some(h);
        }
        if b.half.x <= crate::fmath::abs(rel.x) && b.half.z <= crate::fmath::abs(rel.z) {
            return None;
        }
        let n = V3::new(-dv.x, -dv.y, -dv.z).normalized();
        return Some((0.0, n));
    }
    let mut k = if tc[0] < tc[1] { 1 } else { 0 };
    if tc[k] < tc[2] {
        k = 2;
    }
    let tk = tc[k];
    if !(tk > 0.0 && tk < 1.0) {
        return None;
    }
    for i in 0..3 {
        if i == k {
            continue;
        }
        let p = d[i] * tk + sa[i];
        at[i] = p;
        if p < lo[i] || hi[i] < p {
            return None;
        }
    }
    let face = k == 1
        || crate::fmath::abs(at[0] - c[0]) <= half[0]
        || crate::fmath::abs(at[2] - c[2]) <= half[2];
    if face {
        let sign = if at[k] <= c[k] { -1.0 } else { 1.0 };
        let mut n = [0.0f32; 3];
        n[k] = sign;
        return Some((tk, V3::from_array(n)));
    }
    let (t2, n) = body_corner_sweep(b, V3::from_array(at), e, r)?;
    Some(((1.0 - tk) * t2 + tk, n))
}

/// Whether `shape` placed at `at` intersects the body: the cylinder against the box with the rounded corners of
/// [`body_sweep`] (the time-0 contact of a move from `at`).
// DC-DEFAULT: SIM-15 (proposed) the static fit test of a body (the 2009 box fit engine+0x73be0 grows the box by
// moves, which run the same object stage: PLAUSIBLE that bodies take part)
fn body_overlap(b: &Body, shape: &Cylinder, at: V3) -> bool {
    let rel = at - b.centre;
    let ax = crate::fmath::abs(rel.x);
    let az = crate::fmath::abs(rel.z);
    let ay = crate::fmath::abs(rel.y);
    let r = shape.radius;
    if !(ay < b.half.y + shape.half_height && ax < b.half.x + r && az < b.half.z + r) {
        return false;
    }
    if ax <= b.half.x || az <= b.half.z {
        return true;
    }
    let (ex, ez) = (ax - b.half.x, az - b.half.z);
    ex * ex + ez * ez < r * r
}

/// A collider plus the character bodies of a [`BodySet`]: the world's sweep first, then each body on the part of the
/// segment that is still free, with the object stage's back-off (the contact stays 0.1 units away from the face
/// along the normal).
// SPEC: MOVEMENT_2009.md §9.6 stage 2 (`t *= t'`, then with `q = -dot(N, u) > 0.001`: `t -= clamp((0.1 / q) / |E -
// P|, 0, t)`)
// EVIDENCE: engine+0x745e0 (candidate filter: not the mover, solid by its flag word (engine+0x70650), two objects
// that both carry second-flag 0x200 skipped; the own mover's second flag word is 0x8, MOVEMENT_2009.md §9.2, so the
// own body meets every solid character)
// DC-DEFAULT: SIM-13 (proposed) the own mover collides with the visible remote characters' boxes
pub struct WithBodies<'a, W: Collider + ?Sized> {
    pub world: &'a W,
    pub bodies: &'a BodySet,
}

impl<W: Collider + ?Sized> Collider for WithBodies<'_, W> {
    fn sweep(&self, shape: &Cylinder, p: V3, e: V3) -> Option<Hit> {
        let first = self.world.sweep(shape, p, e);
        if self.bodies.is_empty() {
            return first;
        }
        let d = e - p;
        let len = d.len();
        if len.is_nan() || len <= 0.0 {
            return first;
        }
        let mut t = first.map_or(1.0, |h| h.t);
        let mut hit = first;
        let u = d * (1.0 / len);
        for b in self.bodies.as_slice() {
            if t <= 0.0 {
                break;
            }
            let cur = p + d * t;
            if let Some((tb, n)) = body_sweep(b, shape, p, cur) {
                t *= tb;
                let q = -n.dot(u);
                if q > 0.001 {
                    let back = (0.1 / q) / len;
                    t -= back.clamp(0.0, t);
                }
                hit = Some(Hit {
                    t,
                    normal: n,
                    face_normal: n,
                    surface_flags: SURF_SOLID,
                    model: BODY_MODEL,
                    poly: b.id,
                });
            }
        }
        hit.map(|h| Hit { t, ..h })
    }

    fn overlaps(&self, shape: &Cylinder, at: V3) -> bool {
        self.world.overlaps(shape, at)
            || self
                .bodies
                .as_slice()
                .iter()
                .any(|b| body_overlap(b, shape, at))
    }
}

// ---- client-fps: the pruned point ray (engineering, no rule of its own) ----

/// How far (world units) a part of the segment must lie on one side of a split plane before the other side is left
/// out for it: well above the float error of the plane distances, far below any polygon spacing.
const PRUNE_MARGIN: f32 = 0.01;

impl CollModel {
    /// [`CollModel::sweep`] of a point (a ray) that walks each subtree only with the part of the segment that lies
    /// in it. Every node carries the parameter interval of the segment inside the half-spaces above it (each widened
    /// by [`PRUNE_MARGIN`]), cut at the nearest hit found so far (`best`): a node whose interval lies on one side of
    /// its plane by more than the margin descends that side only, without its own face (whose hit would lie outside
    /// the interval); a child whose part of the interval is empty is not visited. The faces still tested are tested
    /// in the sweep's order with the sweep's test, and a face left out could only be hit outside its node's region
    /// or at or beyond `best` (the sweep keeps a hit only when nearer than `best`): a polygon that crosses a split
    /// plane is referenced from both subtrees, so its hit point inside the other region is still found there. The
    /// equality with the sweep is checked by tests (random box worlds, every ROM world).
    fn point_sweep_pruned(&self, s: V3, e: V3) -> Option<(f32, V3, u32)> {
        let shape = &Cylinder::POINT;
        let d = e - s;
        let len = d.len();
        let mid = (s + e) * 0.5;
        let rb = shape.bound;
        let mut best = 1.0f32;
        let mut found = None;
        // nodes still to visit with their segment interval
        let mut stack: Vec<(u32, f32, f32)> = Vec::with_capacity(64);
        let (mut node, mut t0, mut t1) = (self.root, 0.0f32, 1.0f32);
        loop {
            if node == END || t0 > t1.min(best) {
                match stack.pop() {
                    Some((n, a, b)) => {
                        node = n;
                        t0 = a;
                        t1 = b;
                    }
                    None => break,
                }
                continue;
            }
            let nd = self.nodes[node as usize];
            let Some(f) = self.set.faces.get(nd.face as usize) else {
                node = END;
                continue;
            };
            let n = f.normal;
            let c = n.dot(mid) - f.dist;
            if c > len + rb {
                node = nd.pos;
                continue;
            }
            if c < -(len + rb) {
                node = nd.neg;
                continue;
            }
            let off = shape.support(n);
            let ds = n.dot(s) - f.dist;
            let de = n.dot(e) - f.dist;
            let cs = classify(ds, rb, off);
            let ce = classify(de, rb, off);
            if cs == ce && cs != Side::Touch {
                node = if cs == Side::Pos { nd.pos } else { nd.neg };
                continue;
            }
            // the interval's ends against this plane (distance linear along the segment)
            let k = de - ds;
            let d0 = ds + k * t0;
            let d1 = ds + k * t1.min(best);
            if d0 > PRUNE_MARGIN && d1 > PRUNE_MARGIN {
                node = nd.pos;
                continue;
            }
            if d0 < -PRUNE_MARGIN && d1 < -PRUNE_MARGIN {
                node = nd.neg;
                continue;
            }
            let s_front = cs == Side::Pos || cs == Side::Touch;
            if s_front && f.flags & SURF_SOLID != 0 && n.dot(d) < -0.0001 * len {
                if let Some((t, nn)) = face_sweep(&self.set, nd.face, shape, s, d, best) {
                    best = t;
                    found = Some((t, nn, nd.face));
                }
            }
            // each side's part of the interval: where the distance is above -margin (pos) or below +margin (neg)
            let part = |pos: bool| -> (f32, f32) {
                if k == 0.0 {
                    return (t0, t1);
                }
                let at = |v: f32| (v - ds) / k;
                match (pos, k > 0.0) {
                    (true, true) => (t0.max(at(-PRUNE_MARGIN)), t1),
                    (true, false) => (t0, t1.min(at(-PRUNE_MARGIN))),
                    (false, true) => (t0, t1.min(at(PRUNE_MARGIN))),
                    (false, false) => (t0.max(at(PRUNE_MARGIN)), t1),
                }
            };
            let (first, other, first_pos) = if ds > -0.001 {
                (nd.pos, nd.neg, true)
            } else {
                (nd.neg, nd.pos, false)
            };
            if other != END {
                let (a, b) = part(!first_pos);
                stack.push((other, a, b));
            }
            let (a, b) = part(first_pos);
            node = first;
            t0 = a;
            t1 = b;
        }
        found
    }
}

impl CollisionWorld {
    /// [`CollisionWorld::ray`] with the pruned walk of each world model: the same [`Hit`] (fraction, normals, flags,
    /// model, polygon) for every ray, found faster for long rays (client-fps; the sun test of the client's model
    /// lighting). A world with blocker polygons takes [`CollisionWorld::ray`] itself.
    pub fn ray_pruned(&self, from: V3, to: V3) -> Option<Hit> {
        if !self.blockers.faces.is_empty() {
            return self.ray(from, to);
        }
        let shape = &Cylinder::POINT;
        let (p, e) = (from, to);
        let d = e - p;
        let len = d.len();
        if len.is_nan() || len <= 0.0 {
            return None;
        }
        let mut t = 1.0f32;
        let mut hit: Option<Hit> = None;
        // stage 2 of the sweep as written there (no blockers here): each model on the shortened segment
        let u = d * (1.0 / len);
        for (mi, m) in self.models.iter().enumerate() {
            if t <= 0.0 {
                break;
            }
            let cur = p + d * t;
            let pad = shape.bound + 0.5;
            let lo =
                V3::new(p.x.min(cur.x), p.y.min(cur.y), p.z.min(cur.z)) - V3::new(pad, pad, pad);
            let hi =
                V3::new(p.x.max(cur.x), p.y.max(cur.y), p.z.max(cur.z)) + V3::new(pad, pad, pad);
            if !m.touches_box(lo, hi) {
                continue;
            }
            if let Some((tp, nn, face)) = m.point_sweep_pruned(p, cur) {
                t *= tp;
                let q = -nn.dot(u);
                if q > 0.001 {
                    let back = (0.1 / q) / len;
                    t -= back.clamp(0.0, t);
                }
                hit = Some(Hit {
                    t,
                    normal: nn,
                    face_normal: m.face_normal(face),
                    surface_flags: m.face_flags(face),
                    model: mi as u32,
                    poly: face,
                });
            }
        }
        hit.map(|h| Hit { t, ..h })
    }
}

#[cfg(test)]
mod pruned_ray_tests {
    use super::*;
    use alloc::vec;

    /// A tiny deterministic generator (xorshift) for the random worlds and rays.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }

        fn range(&mut self, a: f32, b: f32) -> f32 {
            a + (b - a) * self.next()
        }
    }

    /// The six outward faces of a box, every face flagged solid, the top ones also with `top_flags`.
    fn box_faces(lo: V3, hi: V3, top_flags: u32) -> Vec<ConvexPoly> {
        let c = |x: f32, y: f32, z: f32| V3::new(x, y, z);
        let (a, b) = (lo, hi);
        vec![
            ConvexPoly::from_corners(
                vec![
                    c(a.x, b.y, a.z),
                    c(a.x, b.y, b.z),
                    c(b.x, b.y, b.z),
                    c(b.x, b.y, a.z),
                ],
                SURF_SOLID | top_flags,
            ),
            ConvexPoly::from_corners(
                vec![
                    c(a.x, a.y, a.z),
                    c(b.x, a.y, a.z),
                    c(b.x, a.y, b.z),
                    c(a.x, a.y, b.z),
                ],
                SURF_SOLID,
            ),
            ConvexPoly::from_corners(
                vec![
                    c(a.x, a.y, a.z),
                    c(a.x, a.y, b.z),
                    c(a.x, b.y, b.z),
                    c(a.x, b.y, a.z),
                ],
                SURF_SOLID,
            ),
            ConvexPoly::from_corners(
                vec![
                    c(b.x, a.y, a.z),
                    c(b.x, b.y, a.z),
                    c(b.x, b.y, b.z),
                    c(b.x, a.y, b.z),
                ],
                SURF_SOLID,
            ),
            ConvexPoly::from_corners(
                vec![
                    c(a.x, a.y, a.z),
                    c(a.x, b.y, a.z),
                    c(b.x, b.y, a.z),
                    c(b.x, a.y, a.z),
                ],
                SURF_SOLID,
            ),
            ConvexPoly::from_corners(
                vec![
                    c(a.x, a.y, b.z),
                    c(b.x, a.y, b.z),
                    c(b.x, b.y, b.z),
                    c(a.x, b.y, b.z),
                ],
                SURF_SOLID,
            ),
        ]
    }

    #[test]
    fn pruned_rays_equal_the_sweep_in_random_box_worlds() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut pruned_hits = 0;
        for world in 0..12 {
            // one model of many boxes (a level), plus a few separate models (props), some boxes touching
            let mut level = Vec::new();
            for _ in 0..60 {
                let lo = V3::new(
                    rng.range(-2000.0, 2000.0),
                    rng.range(-200.0, 400.0),
                    rng.range(-2000.0, 2000.0),
                );
                let size = V3::new(
                    rng.range(4.0, 500.0),
                    rng.range(4.0, 300.0),
                    rng.range(4.0, 500.0),
                );
                // snap some boxes to a grid so faces share planes and edges (ties between faces)
                let lo = if world % 2 == 0 {
                    V3::new(
                        (lo.x / 64.0).round() * 64.0,
                        (lo.y / 64.0).round() * 64.0,
                        (lo.z / 64.0).round() * 64.0,
                    )
                } else {
                    lo
                };
                let flags = if rng.next() < 0.3 { 0x10 } else { 0 };
                level.extend(box_faces(lo, lo + size, flags));
            }
            let mut models = vec![CollModel::from_polys(&level)];
            for _ in 0..4 {
                let lo = V3::new(
                    rng.range(-1500.0, 1500.0),
                    rng.range(-100.0, 200.0),
                    rng.range(-1500.0, 1500.0),
                );
                models.push(CollModel::from_polys(&box_faces(
                    lo,
                    lo + V3::new(64.0, 64.0, 64.0),
                    0,
                )));
            }
            let w = CollisionWorld::from_parts(models, &[]);
            for _ in 0..3000 {
                let from = V3::new(
                    rng.range(-2500.0, 2500.0),
                    rng.range(-300.0, 800.0),
                    rng.range(-2500.0, 2500.0),
                );
                let dir = V3::new(
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                );
                let reach = if rng.next() < 0.5 {
                    8000.0
                } else {
                    rng.range(1.0, 600.0)
                };
                let to = from + dir * reach;
                let a = w.ray(from, to);
                let b = w.ray_pruned(from, to);
                assert_eq!(a, b, "world {world} ray {from:?} -> {to:?}");
                if a.is_some() {
                    pruned_hits += 1;
                }
            }
        }
        assert!(
            pruned_hits > 1000,
            "too few hits to mean anything: {pruned_hits}"
        );
    }

    #[test]
    fn pruned_ray_edge_cases() {
        let w = CollisionWorld::from_parts(
            vec![CollModel::from_polys(&box_faces(
                V3::new(0.0, 0.0, 0.0),
                V3::new(10.0, 10.0, 10.0),
                0x10,
            ))],
            &[],
        );
        let p = V3::new(5.0, 50.0, 5.0);
        // zero length, NaN, straight down onto the top face, starting inside
        assert_eq!(w.ray_pruned(p, p), w.ray(p, p));
        let nan = V3::new(f32::NAN, 0.0, 0.0);
        assert_eq!(w.ray_pruned(nan, p), None);
        let down = w
            .ray_pruned(p, p - V3::new(0.0, 100.0, 0.0))
            .expect("top face");
        assert_eq!(Some(down), w.ray(p, p - V3::new(0.0, 100.0, 0.0)));
        assert_ne!(down.surface_flags & 0x10, 0);
        let inside = V3::new(5.0, 5.0, 5.0);
        assert_eq!(
            w.ray_pruned(inside, inside + V3::new(100.0, 3.0, 1.0)),
            w.ray(inside, inside + V3::new(100.0, 3.0, 1.0))
        );
        // a world with blockers takes the sweep itself
        let wb = CollisionWorld::from_parts(
            vec![],
            &box_faces(V3::new(0.0, 0.0, 0.0), V3::new(10.0, 10.0, 10.0), 0),
        );
        assert_eq!(
            wb.ray_pruned(p, p - V3::new(0.0, 100.0, 0.0)),
            wb.ray(p, p - V3::new(0.0, 100.0, 0.0))
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// The 2009 polygon sweep against a sampled first-overlap time, for random triangles and frame-sized moves
    /// (up to about 5 units) that go into the front side of the face from a free start. The rule is not exact
    /// by design: the edge planes are judged at the slab entry only, and an edge tests only the vertex at the
    /// end the move enters the edge's span from, so a contact reached past the near vertex while leaving the
    /// span is not seen (rare). Phantom contacts (a hit with no overlap at all) are the failure the old
    /// polytope clip had; the 2009 rule has none beyond the sampling resolution.
    // EVIDENCE: engine+0x77530, engine+0x76120 (vertex choice: A when the move runs along +u, A + e otherwise,
    // DISASM engine+0x76d03..0x76d25)
    #[test]
    fn polygon_sweep_agrees_with_sampled_overlap() {
        let mut seed = 0x9e37_79b9u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        let shape = Cylinder::new(15.0, 50.0);
        let (mut cases, mut agree, mut phantom) = (0u32, 0u32, 0u32);
        for _ in 0..30000 {
            let v: Vec<V3> = (0..3)
                .map(|_| V3::new(rnd() * 60.0, rnd() * 80.0, rnd() * 60.0))
                .collect();
            let poly = ConvexPoly::from_corners(v, SURF_SOLID);
            if poly.normal.len_sq() < 0.5 {
                continue;
            }
            let mut set = PolySet::default();
            set.push(poly.normal, poly.dist, &poly.verts, poly.flags);
            let s = V3::new(rnd() * 50.0, rnd() * 100.0, rnd() * 50.0);
            let d = V3::new(rnd() * 5.0, rnd() * 2.0, rnd() * 5.0);
            if face_overlap(&set, 0, &shape, s)
                || poly.normal.dot(s) - poly.dist < 0.0
                || poly.normal.dot(d) >= 0.0
            {
                continue;
            }
            cases += 1;
            let n = 400;
            let first = (0..=n)
                .map(|i| i as f32 / n as f32)
                .find(|&t| face_overlap(&set, 0, &shape, s + d * t));
            let got = face_sweep(&set, 0, &shape, s, d, 1.0).map(|x| x.0);
            match (first, got) {
                (None, None) => agree += 1,
                (Some(a), Some(b)) if (a - b).abs() * d.len() < 0.2 => agree += 1,
                (None, Some(t)) if t < 0.95 => phantom += 1,
                _ => {}
            }
        }
        assert!(cases > 5000, "{cases}");
        assert_eq!(phantom, 0, "contacts without any overlap");
        assert!(
            agree as f32 >= 0.995 * cases as f32,
            "{agree} of {cases} agree"
        );
    }

    /// The old conservative edge clip pushed a sloped face's edge planes out by their full support (up to the
    /// half height), so a body passing beside a thin slanted panel stopped tens of units away from it.
    #[test]
    fn no_contact_beside_a_slanted_panel() {
        // a narrow panel leaning over the path, lowest corner 98 units up, 20 units to the side
        let panel = ConvexPoly::from_corners(
            vec![
                V3::new(30.0, 132.0, 10.0),
                V3::new(0.0, 98.0, 11.0),
                V3::new(-3.0, 138.0, 9.0),
            ],
            SURF_SOLID,
        );
        let w = world(&[floor(), panel]);
        let shape = Cylinder::new(15.0, 50.0);
        // standing 20 units beside the panel's lowest corner and walking past it
        let from = V3::new(-20.0, 50.1, -20.0);
        let to = V3::new(-20.0, 50.1, 20.0);
        assert!(w.sweep(&shape, from, to).is_none());
    }

    /// The cylinder's side meets a box's vertical corner edge: the contact normal is the radial direction from
    /// the edge (rounded corner), so the slide goes around the corner instead of stopping on it.
    #[test]
    fn box_corner_is_rounded() {
        // a 100-unit box with its corner at the origin, occupying x > 0, z > 0
        let side_x = ConvexPoly::from_corners(
            vec![
                V3::new(0.0, 0.0, 100.0),
                V3::new(0.0, 100.0, 100.0),
                V3::new(0.0, 100.0, 0.0),
                V3::new(0.0, 0.0, 0.0),
            ],
            SURF_SOLID,
        );
        let side_z = ConvexPoly::from_corners(
            vec![
                V3::new(0.0, 100.0, 0.0),
                V3::new(100.0, 100.0, 0.0),
                V3::new(100.0, 0.0, 0.0),
                V3::new(0.0, 0.0, 0.0),
            ],
            SURF_SOLID,
        );
        assert!(side_x.normal.x < -0.99 && side_z.normal.z < -0.99);
        let w = world(&[floor(), side_x, side_z]);
        let shape = Cylinder::new(15.0, 50.0);
        // moving diagonally into the corner, offset so that only the rounded corner region is reached
        let from = V3::new(-20.0, 50.1, -30.0);
        let to = V3::new(0.0, 50.1, -10.0);
        let h = w.sweep(&shape, from, to).expect("corner contact");
        let at = from + (to - from) * h.t;
        let r = (at.x * at.x + at.z * at.z).sqrt();
        assert!((r - 15.1).abs() < 0.2, "contact {at:?} r {r}");
        // the normal points from the corner edge to the contact centre
        let radial = V3::new(at.x, 0.0, at.z).normalized();
        assert!(h.normal.dot(radial) > 0.99, "{:?} vs {radial:?}", h.normal);
        assert_eq!(h.face_normal.y, 0.0);
    }

    /// Rays (zero-size shape) against a sloped face: the support offset is zero, so the in-plane edge planes
    /// bound the polygon (no hit beside it).
    #[test]
    fn ray_misses_beside_a_sloped_face() {
        let ramp = ConvexPoly::from_corners(
            vec![
                V3::new(0.0, 0.0, 0.0),
                V3::new(0.0, 10.0, 20.0),
                V3::new(10.0, 10.0, 20.0),
                V3::new(10.0, 0.0, 0.0),
            ],
            SURF_SOLID,
        );
        let w = world(&[ramp]);
        assert!(w
            .ray(V3::new(5.0, 20.0, 10.0), V3::new(5.0, -20.0, 10.0))
            .is_some());
        assert!(w
            .ray(V3::new(50.0, 20.0, 10.0), V3::new(50.0, -20.0, 10.0))
            .is_none());
    }

    fn quad(c: [V3; 4]) -> ConvexPoly {
        ConvexPoly::from_corners(c.to_vec(), SURF_SOLID)
    }

    /// Floor at y = 0 spanning [-500, 500]^2, facing up.
    fn floor() -> ConvexPoly {
        quad([
            V3::new(-500.0, 0.0, -500.0),
            V3::new(-500.0, 0.0, 500.0),
            V3::new(500.0, 0.0, 500.0),
            V3::new(500.0, 0.0, -500.0),
        ])
    }

    /// Wall at z = 100 facing -z.
    fn wall() -> ConvexPoly {
        quad([
            V3::new(-500.0, 0.0, 100.0),
            V3::new(-500.0, 300.0, 100.0),
            V3::new(500.0, 300.0, 100.0),
            V3::new(500.0, 0.0, 100.0),
        ])
    }

    fn world(polys: &[ConvexPoly]) -> CollisionWorld {
        CollisionWorld::from_parts(vec![CollModel::from_polys(polys)], &[])
    }

    #[test]
    fn corners_ccw_give_the_front_normal() {
        let f = floor();
        assert_eq!(f.normal, V3::UP);
        assert_eq!(f.dist, 0.0);
        let w = wall();
        assert!(
            (w.normal - V3::new(0.0, 0.0, -1.0)).len() < 1e-6,
            "{:?}",
            w.normal
        );
    }

    #[test]
    fn falling_onto_the_floor_stops_point_one_above_the_slab() {
        let w = world(&[floor()]);
        let shape = Cylinder::new(16.0, 40.0);
        let from = V3::new(0.0, 100.0, 0.0);
        let to = V3::new(0.0, 0.0, 0.0);
        let h = w.sweep(&shape, from, to).expect("hit");
        let stop = from + (to - from) * h.t;
        assert_eq!(h.normal, V3::UP);
        // contact at y = 40, backed off 0.1 along the normal
        assert!((stop.y - 40.1).abs() < 1e-3, "{stop:?}");
    }

    #[test]
    fn moving_parallel_above_the_floor_is_free() {
        let w = world(&[floor()]);
        let shape = Cylinder::new(16.0, 40.0);
        assert!(w
            .sweep(&shape, V3::new(0.0, 40.1, 0.0), V3::new(50.0, 40.1, 0.0))
            .is_none());
    }

    #[test]
    fn walking_into_a_wall_stops_radius_away() {
        let w = world(&[floor(), wall()]);
        let shape = Cylinder::new(16.0, 40.0);
        let from = V3::new(0.0, 40.1, 0.0);
        let to = V3::new(0.0, 40.1, 200.0);
        let h = w.sweep(&shape, from, to).expect("hit");
        let stop = from + (to - from) * h.t;
        assert!((stop.z - (100.0 - 16.0 - 0.1)).abs() < 1e-3, "{stop:?}");
        assert!((h.normal - V3::new(0.0, 0.0, -1.0)).len() < 1e-6);
    }

    #[test]
    fn back_faces_do_not_collide() {
        let w = world(&[wall()]);
        let shape = Cylinder::new(16.0, 40.0);
        // from behind the wall going out through its back
        assert!(w
            .sweep(&shape, V3::new(0.0, 40.0, 200.0), V3::new(0.0, 40.0, 0.0))
            .is_none());
    }

    #[test]
    fn floor_edge_supports_until_the_rim_passes() {
        // a small platform [0, 100] x [0, 100] at y = 0
        let p = quad([
            V3::new(0.0, 0.0, 0.0),
            V3::new(0.0, 0.0, 100.0),
            V3::new(100.0, 0.0, 100.0),
            V3::new(100.0, 0.0, 0.0),
        ]);
        let w = world(&[p]);
        let shape = Cylinder::new(16.0, 40.0);
        // centre 10 units beyond the edge: the rim (r = 16) still overlaps -> hit
        assert!(w
            .sweep(
                &shape,
                V3::new(110.0, 41.0, 50.0),
                V3::new(110.0, 39.0, 50.0)
            )
            .is_some());
        // centre 20 units beyond: free
        assert!(w
            .sweep(
                &shape,
                V3::new(120.0, 41.0, 50.0),
                V3::new(120.0, 39.0, 50.0)
            )
            .is_none());
    }

    #[test]
    fn overlap_test() {
        let w = world(&[floor(), wall()]);
        let shape = Cylinder::new(16.0, 40.0);
        assert!(!w.overlaps(&shape, V3::new(0.0, 40.1, 0.0)));
        assert!(w.overlaps(&shape, V3::new(0.0, 30.0, 0.0)));
        assert!(w.overlaps(&shape, V3::new(0.0, 100.0, 95.0)));
        assert!(!w.overlaps(&shape, V3::new(0.0, 100.0, 80.0)));
    }

    #[test]
    fn bsp_builder_duplicates_straddlers() {
        let polys = [floor(), wall()];
        let (nodes, root) = build_bsp(&polys);
        assert_eq!(root, Some(0));
        // floor first; the wall lies on its positive side
        assert_eq!(nodes[0].0, 0);
        assert!(nodes[0].2.is_some() && nodes[0].1.is_none());
    }

    #[test]
    fn ray_hits_and_misses() {
        let w = world(&[floor()]);
        let h = w
            .ray(V3::new(0.0, 10.0, 0.0), V3::new(0.0, -10.0, 0.0))
            .expect("ray hit");
        assert!(h.t < 0.5);
        assert!(w
            .ray(V3::new(0.0, 10.0, 0.0), V3::new(0.0, 5.0, 0.0))
            .is_none());
        assert!(w
            .ray(V3::new(600.0, 10.0, 0.0), V3::new(600.0, -10.0, 0.0))
            .is_none());
    }

    // ---- character bodies (w1-bodies) ----

    /// A made-up standing body (no ROM numbers).
    fn body_at(x: f32, z: f32) -> Body {
        Body {
            id: 7,
            centre: V3::new(x, 40.0, z),
            half: V3::new(16.0, 40.0, 16.0),
        }
    }

    fn bodies(list: &[Body]) -> BodySet {
        let mut s = BodySet::default();
        for b in list {
            assert!(s.push(*b));
        }
        s
    }

    #[test]
    fn walking_into_a_body_stops_at_the_grown_box_with_the_back_off() {
        let set = bodies(&[body_at(0.0, 100.0)]);
        let w = WithBodies {
            world: &EmptyWorld,
            bodies: &set,
        };
        let shape = Cylinder::new(16.0, 40.0);
        let from = V3::new(0.0, 40.0, 0.0);
        let to = V3::new(0.0, 40.0, 200.0);
        let h = w.sweep(&shape, from, to).expect("blocked");
        let stop = from + (to - from) * h.t;
        // face plane at 100 - 16 - 16 = 68, kept 0.1 away along the normal
        assert!((stop.z - 67.9).abs() < 1e-3, "{stop:?}");
        assert_eq!(h.normal, V3::new(0.0, 0.0, -1.0));
        assert_eq!((h.model, h.poly), (BODY_MODEL, 7));
        // beside it (beyond the radius plus the box) the move is free
        assert!(w
            .sweep(&shape, V3::new(40.0, 40.0, 0.0), V3::new(40.0, 40.0, 200.0))
            .is_none());
        // the world's own hit still wins when it is nearer
        let wall = world(&[wall()]);
        let w2 = WithBodies {
            world: &wall,
            bodies: &bodies(&[body_at(0.0, 150.0)]),
        };
        let h = w2.sweep(&shape, from, to).expect("wall");
        assert_ne!(h.model, BODY_MODEL);
    }

    #[test]
    fn a_body_corner_is_rounded_by_the_radius() {
        let b = body_at(0.0, 0.0);
        let shape = Cylinder::new(16.0, 40.0);
        // a diagonal move at the corner (16, 16): the contact is the radius away from the corner axis
        let s = V3::new(60.0, 40.0, 60.0);
        let e = V3::new(0.0, 40.0, 0.0);
        let (t, n) = body_sweep(&b, &shape, s, e).expect("corner");
        let at = s + (e - s) * t;
        let dist = ((at.x - 16.0).powi(2) + (at.z - 16.0).powi(2)).sqrt();
        assert!((dist - 16.0).abs() < 1e-2, "{at:?} {dist}");
        assert!(n.x > 0.7 && n.z > 0.7 && n.y == 0.0, "{n:?}");
        // passing the corner within the square but outside the rounded corner: no hit
        // (the line x + z = 61 enters the grown square, whose corner has x + z = 64, but passes 20.5 units from
        // the corner axis)
        let s = V3::new(50.0, 40.0, 11.0);
        let e = V3::new(11.0, 40.0, 50.0);
        let grown_square_hit = body_sweep(&b, &shape, s, e);
        assert!(grown_square_hit.is_none(), "{grown_square_hit:?}");
    }

    #[test]
    fn an_overlapping_start_blocks_only_moves_towards_the_centre() {
        let b = body_at(0.0, 0.0);
        let shape = Cylinder::new(16.0, 40.0);
        let s = V3::new(0.0, 40.0, 20.0);
        // away: free
        assert!(body_sweep(&b, &shape, s, s + V3::new(0.0, 0.0, 5.0)).is_none());
        // towards, from beyond the box's own z extent: the 2009 rule sweeps the nearest corner axis first (here it
        // is met at 0.8 of the move)
        let (t, _) = body_sweep(&b, &shape, s, s + V3::new(0.0, 0.0, -5.0)).unwrap();
        assert!((t - 0.8).abs() < 1e-3, "{t}");
        // towards, from inside the box's own column: time 0, normal against the move
        let deep = V3::new(0.0, 40.0, 10.0);
        let (t, n) = body_sweep(&b, &shape, deep, deep + V3::new(0.0, 0.0, -5.0)).unwrap();
        assert_eq!(t, 0.0);
        assert_eq!(n, V3::new(0.0, 0.0, 1.0));
        assert!(body_overlap(&b, &shape, s));
        assert!(!body_overlap(&b, &shape, V3::new(0.0, 40.0, 33.0)));
        // the corner region outside the rounded corner does not overlap
        assert!(!body_overlap(&b, &shape, V3::new(29.0, 40.0, 29.0)));
        assert!(body_overlap(&b, &shape, V3::new(26.0, 40.0, 26.0)));
        // above the head: free
        assert!(!body_overlap(&b, &shape, V3::new(0.0, 121.0, 0.0)));
    }

    #[test]
    fn falling_onto_a_head_lands_on_the_top_face() {
        let set = bodies(&[body_at(0.0, 0.0)]);
        let w = WithBodies {
            world: &EmptyWorld,
            bodies: &set,
        };
        let shape = Cylinder::new(16.0, 40.0);
        let from = V3::new(5.0, 300.0, 5.0);
        let to = V3::new(5.0, 0.0, 5.0);
        let h = w.sweep(&shape, from, to).expect("head");
        let stop = from + (to - from) * h.t;
        // top 80 plus the half height 40, 0.1 above
        assert!((stop.y - 120.1).abs() < 1e-3, "{stop:?}");
        assert_eq!(h.face_normal, V3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn walking_into_a_body_at_an_angle_slides_along_it() {
        let set = bodies(&[body_at(0.0, 100.0)]);
        let w = WithBodies {
            world: &EmptyWorld,
            bodies: &set,
        };
        let shape = Cylinder::new(16.0, 40.0);
        let p0 = V3::new(0.0, 40.0, 60.0);
        // into the -Z face, mostly forward, a little sideways
        let e0 = V3::new(10.0, 40.0, 90.0);
        let out = crate::motion::slide_move(&w, &shape, p0, e0);
        assert!(out.pos.z < 68.0, "stays out of the box {:?}", out.pos);
        assert!(out.pos.x > 9.0, "keeps the sideways part {:?}", out.pos);
    }

    #[test]
    fn the_body_set_is_bounded_and_reused() {
        let mut s = BodySet::default();
        for i in 0..(MAX_BODIES as u32 + 5) {
            let ok = s.push(Body {
                id: i,
                ..body_at(i as f32 * 100.0, 0.0)
            });
            assert_eq!(ok, (i as usize) < MAX_BODIES);
        }
        assert_eq!(s.len(), MAX_BODIES);
        assert!(!s.push(Body {
            half: V3::new(0.0, 1.0, 1.0),
            ..body_at(0.0, 0.0)
        }));
        s.clear();
        assert!(s.is_empty());
        assert!(!s.push(Body {
            centre: V3::new(f32::NAN, 0.0, 0.0),
            ..body_at(0.0, 0.0)
        }));
        assert!(s.push(body_at(0.0, 0.0)));
        assert_eq!(s.as_slice()[0].id, 7);
    }
}
