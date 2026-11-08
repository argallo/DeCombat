//! Clipping for the composer: axis-aligned boxes, convex polygons against a box and outside cut boxes, render
//! vertices with their attributes, and convex volumes given as half-spaces.
//!
//! Everything here is plain `f32` arithmetic in a fixed order (no `mul_add`, no transcendental function), so the
//! game server, the client and dc-mapkit cut the same polygons into the same corners on every platform.
// DC-DEFAULT: MAP-116 (pieces are kept to their region, outside their cuts)

pub type V = [f32; 3];

/// Corners closer than this are merged after a clip.
const WELD: f32 = 1.0e-3;
/// A clipped polygon whose doubled area is below this is dropped (a sliver along a box face).
const MIN_AREA2: f32 = 1.0e-3;

pub fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
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

pub fn scale(a: V, s: f32) -> V {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn length(a: V) -> f32 {
    dot(a, a).sqrt()
}

/// An axis-aligned box, `min <= max` on every axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: V,
    pub max: V,
}

impl Aabb {
    /// The box spanned by two corners in any order.
    pub fn new(a: V, b: V) -> Aabb {
        Aabb {
            min: [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            max: [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
        }
    }

    /// The bounds of a point set, `None` when it is empty.
    pub fn of_points<'p>(pts: impl IntoIterator<Item = &'p V>) -> Option<Aabb> {
        let mut it = pts.into_iter();
        let first = *it.next()?;
        let mut b = Aabb {
            min: first,
            max: first,
        };
        for p in it {
            b.grow(*p);
        }
        Some(b)
    }

    pub fn grow(&mut self, p: V) {
        for (i, &c) in p.iter().enumerate() {
            self.min[i] = self.min[i].min(c);
            self.max[i] = self.max[i].max(c);
        }
    }

    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb {
            min: [
                self.min[0].min(o.min[0]),
                self.min[1].min(o.min[1]),
                self.min[2].min(o.min[2]),
            ],
            max: [
                self.max[0].max(o.max[0]),
                self.max[1].max(o.max[1]),
                self.max[2].max(o.max[2]),
            ],
        }
    }

    pub fn translated(&self, t: V) -> Aabb {
        Aabb {
            min: add(self.min, t),
            max: add(self.max, t),
        }
    }

    /// Every coordinate finite and `min < max` on every axis.
    pub fn is_valid(&self) -> bool {
        (0..3).all(|i| {
            self.min[i].is_finite() && self.max[i].is_finite() && self.min[i] < self.max[i]
        })
    }

    /// The point lies in the closed box.
    pub fn contains(&self, p: V) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }

    /// The other box lies in this closed box.
    pub fn contains_box(&self, o: &Aabb) -> bool {
        (0..3).all(|i| o.min[i] >= self.min[i] && o.max[i] <= self.max[i])
    }

    /// The two boxes share interior (touching faces do not count).
    pub fn overlaps(&self, o: &Aabb) -> bool {
        (0..3).all(|i| self.min[i] < o.max[i] && o.min[i] < self.max[i])
    }

    /// The two closed boxes share a point.
    pub fn touches(&self, o: &Aabb) -> bool {
        (0..3).all(|i| self.min[i] <= o.max[i] && o.min[i] <= self.max[i])
    }

    pub fn centre(&self) -> V {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    /// The intersection, `None` when the boxes share no interior.
    pub fn intersection(&self, o: &Aabb) -> Option<Aabb> {
        if !self.overlaps(o) {
            return None;
        }
        Some(Aabb {
            min: [
                self.min[0].max(o.min[0]),
                self.min[1].max(o.min[1]),
                self.min[2].max(o.min[2]),
            ],
            max: [
                self.max[0].min(o.max[0]),
                self.max[1].min(o.max[1]),
                self.max[2].min(o.max[2]),
            ],
        })
    }
}

/// What the clipper needs of a corner: its position, and the corner a fraction `t` of the way to another.
pub trait Corner: Clone {
    fn pos(&self) -> V;
    fn lerp(&self, o: &Self, t: f32) -> Self;
    /// The same corner with its position coordinate `axis` set exactly (no drift off a box face).
    fn snap(self, axis: usize, v: f32) -> Self;
}

impl Corner for V {
    fn pos(&self) -> V {
        *self
    }
    fn lerp(&self, o: &V, t: f32) -> V {
        [
            self[0] + (o[0] - self[0]) * t,
            self[1] + (o[1] - self[1]) * t,
            self[2] + (o[2] - self[2]) * t,
        ]
    }
    fn snap(mut self, axis: usize, v: f32) -> V {
        self[axis] = v;
        self
    }
}

/// Keeps the part of a convex polygon on one side of the plane `p[axis] = v`: `above` keeps `p[axis] >= v`,
/// else `p[axis] <= v`; `strict` leaves out what lies on the plane itself (the half that continues in
/// [`minus_box`], so a polygon lying in a cut face is kept once).
fn clip_axis<C: Corner>(poly: &[C], axis: usize, v: f32, above: bool, strict: bool) -> Vec<C> {
    let inside = |c: &C| {
        let x = c.pos()[axis];
        match (above, strict) {
            (true, false) => x >= v,
            (true, true) => x > v,
            (false, false) => x <= v,
            (false, true) => x < v,
        }
    };
    let n = poly.len();
    let mut out = Vec::with_capacity(n + 2);
    for i in 0..n {
        let a = &poly[i];
        let b = &poly[(i + 1) % n];
        let (ia, ib) = (inside(a), inside(b));
        if ia {
            out.push(a.clone());
        }
        if ia != ib {
            let (pa, pb) = (a.pos()[axis], b.pos()[axis]);
            let den = pb - pa;
            if den != 0.0 {
                let t = (v - pa) / den;
                out.push(a.lerp(b, t).snap(axis, v));
            }
        }
    }
    out
}

/// Drops repeated corners and slivers; `None` when fewer than three corners or no area are left.
fn tidy<C: Corner>(poly: Vec<C>) -> Option<Vec<C>> {
    let mut out: Vec<C> = Vec::with_capacity(poly.len());
    for c in poly {
        if let Some(last) = out.last() {
            if length(sub(c.pos(), last.pos())) < WELD {
                continue;
            }
        }
        out.push(c);
    }
    while out.len() > 1 && length(sub(out[0].pos(), out[out.len() - 1].pos())) < WELD {
        out.pop();
    }
    if out.len() < 3 {
        return None;
    }
    // doubled area of the fan
    let o = out[0].pos();
    let mut acc = [0.0f32; 3];
    for w in out[1..].windows(2) {
        acc = add(acc, cross(sub(w[0].pos(), o), sub(w[1].pos(), o)));
    }
    (length(acc) >= MIN_AREA2).then_some(out)
}

/// The part of a convex polygon inside the closed box.
pub fn clip_box<C: Corner>(poly: &[C], b: &Aabb) -> Option<Vec<C>> {
    let mut p: Vec<C> = poly.to_vec();
    for axis in 0..3 {
        p = clip_axis(&p, axis, b.min[axis], true, false);
        if p.len() < 3 {
            return None;
        }
        p = clip_axis(&p, axis, b.max[axis], false, false);
        if p.len() < 3 {
            return None;
        }
    }
    tidy(p)
}

/// The convex pieces of a polygon outside a box (at most six): each face of the box in turn splits off what lies
/// beyond it, and what is left inside the box at the end is dropped.
pub fn minus_box<C: Corner>(poly: &[C], cut: &Aabb) -> Vec<Vec<C>> {
    let bounds = Aabb::of_points(poly.iter().map(|c| c.pos()).collect::<Vec<_>>().iter());
    match bounds {
        Some(b) if b.overlaps(cut) => {}
        _ => return vec![poly.to_vec()],
    }
    let mut out = Vec::new();
    let mut rest: Vec<C> = poly.to_vec();
    for axis in 0..3 {
        for (v, beyond_above) in [(cut.min[axis], false), (cut.max[axis], true)] {
            let beyond = clip_axis(&rest, axis, v, beyond_above, false);
            if let Some(piece) = tidy(beyond) {
                out.push(piece);
            }
            rest = clip_axis(&rest, axis, v, !beyond_above, true);
            if rest.len() < 3 {
                return out;
            }
        }
    }
    out
}

/// The convex pieces of a polygon kept by a region and a set of cuts: inside the region, outside every cut. A
/// polygon wholly inside the region and clear of every cut comes back exactly as it was.
pub fn keep<C: Corner>(poly: &[C], region: &Aabb, cuts: &[Aabb]) -> Vec<Vec<C>> {
    let pts: Vec<V> = poly.iter().map(|c| c.pos()).collect();
    let Some(bounds) = Aabb::of_points(pts.iter()) else {
        return Vec::new();
    };
    if !region.touches(&bounds) {
        return Vec::new();
    }
    if region.contains_box(&bounds) && !cuts.iter().any(|c| c.overlaps(&bounds)) {
        return vec![poly.to_vec()];
    }
    let Some(first) = clip_box(poly, region) else {
        return Vec::new();
    };
    let mut parts = vec![first];
    for cut in cuts {
        let mut next = Vec::with_capacity(parts.len());
        for p in parts {
            next.extend(minus_box(&p, cut));
        }
        parts = next;
        if parts.is_empty() {
            break;
        }
    }
    parts
}

/// A half-space `dot(n, p) <= d` (inside).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HalfSpace {
    pub n: V,
    pub d: f32,
}

impl HalfSpace {
    fn side(&self, p: V) -> f32 {
        dot(self.n, p) - self.d
    }
}

/// The half-spaces of a box (`x <= max.x`, `x >= min.x`, ... in axis order).
pub fn box_halfspaces(b: &Aabb) -> [HalfSpace; 6] {
    let mut out = [HalfSpace {
        n: [0.0; 3],
        d: 0.0,
    }; 6];
    for axis in 0..3 {
        let mut n = [0.0f32; 3];
        n[axis] = 1.0;
        out[2 * axis] = HalfSpace { n, d: b.max[axis] };
        let mut m = [0.0f32; 3];
        m[axis] = -1.0;
        out[2 * axis + 1] = HalfSpace {
            n: m,
            d: -b.min[axis],
        };
    }
    out
}

/// Keeps the part of a convex polygon inside a half-space.
fn clip_half(poly: &[V], h: &HalfSpace) -> Vec<V> {
    let n = poly.len();
    let mut out = Vec::with_capacity(n + 2);
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let (sa, sb) = (h.side(a), h.side(b));
        let (ia, ib) = (sa <= 0.0, sb <= 0.0);
        if ia {
            out.push(a);
        }
        if ia != ib {
            let den = sa - sb;
            if den != 0.0 {
                out.push(a.lerp(&b, sa / den));
            }
        }
    }
    out
}

/// The faces of the convex volume the half-spaces bound, each as `(index of its half-space, corners
/// counter-clockwise seen from outside)`; `near` is any box around the volume (it sizes the start polygons).
/// Half-spaces that bound no face give none; an empty volume gives fewer than four faces.
pub fn volume_faces(planes: &[HalfSpace], near: &Aabb) -> Vec<(usize, Vec<V>)> {
    let c = near.centre();
    let h = length(sub(near.max, near.min)) * 2.0 + 16.0;
    let mut out = Vec::new();
    for (i, p) in planes.iter().enumerate() {
        let n = p.n;
        let len = length(n);
        if len == 0.0 {
            continue;
        }
        let n = scale(n, 1.0 / len);
        let d = p.d / len;
        // a reference axis away from the normal, then u, v with u x v = n
        let r = if n[0].abs() <= n[1].abs() && n[0].abs() <= n[2].abs() {
            [1.0, 0.0, 0.0]
        } else if n[1].abs() <= n[2].abs() {
            [0.0, 1.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        let u = cross(n, r);
        let u = scale(u, 1.0 / length(u));
        let v = cross(n, u);
        let o = sub(c, scale(n, dot(n, c) - d));
        let corner = |a: f32, b: f32| add(o, add(scale(u, a * h), scale(v, b * h)));
        let mut poly = vec![
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ];
        for (j, q) in planes.iter().enumerate() {
            if j == i {
                continue;
            }
            poly = clip_half(&poly, q);
            if poly.len() < 3 {
                break;
            }
        }
        if let Some(face) = tidy(poly) {
            out.push((i, face));
        }
    }
    out
}

/// Bounds of a volume's faces.
pub fn faces_bounds(faces: &[(usize, Vec<V>)]) -> Option<Aabb> {
    Aabb::of_points(faces.iter().flat_map(|(_, f)| f.iter()))
}

/// The convex pieces of a convex volume outside a cut box, each as its half-space list (at most six pieces).
pub fn volume_minus_box(planes: &[HalfSpace], near: &Aabb, cut: &Aabb) -> Vec<Vec<HalfSpace>> {
    let faces = volume_faces(planes, near);
    match faces_bounds(&faces) {
        Some(b) if faces.len() >= 4 && b.overlaps(cut) => {}
        Some(_) if faces.len() >= 4 => return vec![planes.to_vec()],
        _ => return Vec::new(),
    }
    let mut out = Vec::new();
    let mut rest = planes.to_vec();
    for axis in 0..3 {
        let mut pos = [0.0f32; 3];
        pos[axis] = 1.0;
        let mut neg = [0.0f32; 3];
        neg[axis] = -1.0;
        // below the cut's min face, then above its max face
        let sides = [
            (
                HalfSpace {
                    n: pos,
                    d: cut.min[axis],
                },
                HalfSpace {
                    n: neg,
                    d: -cut.min[axis],
                },
            ),
            (
                HalfSpace {
                    n: neg,
                    d: -cut.max[axis],
                },
                HalfSpace {
                    n: pos,
                    d: cut.max[axis],
                },
            ),
        ];
        for (beyond, within) in sides {
            let mut part = rest.clone();
            part.push(beyond);
            if volume_faces(&part, near).len() >= 4 {
                out.push(part);
            }
            rest.push(within);
            if volume_faces(&rest, near).len() < 4 {
                return out;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(y: f32, lo: f32, hi: f32) -> Vec<V> {
        vec![[lo, y, lo], [lo, y, hi], [hi, y, hi], [hi, y, lo]]
    }

    fn area(p: &[V]) -> f32 {
        let o = p[0];
        let mut acc = [0.0f32; 3];
        for w in p[1..].windows(2) {
            acc = add(acc, cross(sub(w[0], o), sub(w[1], o)));
        }
        length(acc) * 0.5
    }

    #[test]
    fn a_box_clips_a_polygon_to_its_inside() {
        let b = Aabb::new([0.0, -10.0, 0.0], [50.0, 10.0, 50.0]);
        let p = clip_box(&square(0.0, -100.0, 100.0), &b).unwrap();
        assert!((area(&p) - 2500.0).abs() < 1e-2);
        assert!(p.iter().all(|c| b.contains(*c)));
        assert!(clip_box(&square(20.0, -100.0, 100.0), &b).is_none());
    }

    #[test]
    fn a_cut_leaves_the_ring_around_it_and_nothing_twice() {
        let floor = square(0.0, -100.0, 100.0);
        let cut = Aabb::new([-10.0, -5.0, -10.0], [10.0, 5.0, 10.0]);
        let parts = minus_box(&floor, &cut);
        let total: f32 = parts.iter().map(|p| area(p)).sum();
        assert!((total - (40_000.0 - 400.0)).abs() < 0.1, "{total}");
        // a polygon lying in a face of the cut is kept once
        let wall = vec![
            [-10.0, -20.0, -50.0],
            [-10.0, 20.0, -50.0],
            [-10.0, 20.0, 50.0],
            [-10.0, -20.0, 50.0],
        ];
        let parts = minus_box(&wall, &cut);
        let total: f32 = parts.iter().map(|p| area(p)).sum();
        assert!((total - 4000.0).abs() < 0.1, "{total}");
        // untouched polygons come back as they are
        assert_eq!(
            minus_box(&square(50.0, 0.0, 5.0), &cut),
            vec![square(50.0, 0.0, 5.0)]
        );
    }

    #[test]
    fn keep_combines_region_and_cuts() {
        let region = Aabb::new([-50.0, -1.0, -50.0], [50.0, 1.0, 50.0]);
        let cuts = [Aabb::new([40.0, -5.0, -60.0], [60.0, 5.0, 60.0])];
        let parts = keep(&square(0.0, -100.0, 100.0), &region, &cuts);
        let total: f32 = parts.iter().map(|p| area(p)).sum();
        assert!((total - 90.0 * 100.0).abs() < 0.1, "{total}");
    }

    #[test]
    fn a_box_volume_has_six_faces_and_loses_a_slice_to_a_cut() {
        let b = Aabb::new([0.0, 0.0, 0.0], [100.0, 50.0, 80.0]);
        let hs = box_halfspaces(&b);
        let faces = volume_faces(&hs, &b);
        assert_eq!(faces.len(), 6);
        let fb = faces_bounds(&faces).unwrap();
        for i in 0..3 {
            assert!((fb.min[i] - b.min[i]).abs() < 1e-3 && (fb.max[i] - b.max[i]).abs() < 1e-3);
        }
        // every face is wound counter-clockwise seen from outside
        for (i, f) in &faces {
            let n = cross(sub(f[1], f[0]), sub(f[2], f[1]));
            assert!(dot(n, hs[*i].n) > 0.0);
        }
        let cut = Aabb::new([90.0, -10.0, -10.0], [200.0, 100.0, 100.0]);
        let parts = volume_minus_box(&hs, &b, &cut);
        assert_eq!(parts.len(), 1);
        let pb = faces_bounds(&volume_faces(&parts[0], &b)).unwrap();
        assert!((pb.max[0] - 90.0).abs() < 1e-3);
        // a cut through the middle leaves pieces around it
        let hole = Aabb::new([40.0, -10.0, 30.0], [60.0, 100.0, 50.0]);
        let parts = volume_minus_box(&hs, &b, &hole);
        assert_eq!(parts.len(), 4);
    }
}
