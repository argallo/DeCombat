//! The whole gun: the third-person model's shape, which has every side, with the first-person model's texture, which has the detail. The two
//! models have different texture layouts, so one texture will not go on the other. Instead the third-person model is fitted over the first-person
//! one by their outlines from the side, and each texel of a larger third-person texture looks straight along the way its face points and reads the
//! outermost first-person surface there. Looking along a line and not for the closest point keeps neighbouring texels on the same surface, so a
//! pattern stays in one piece. That lookup is worked out once per gun, and then any paint goes through it at once. A first-person model has little or
//! no far side, and what it has is dark and rough, so every face looking sideways reads the side the player sees, at the same place along the gun. That
//! also suits the third-person model, which often uses one part of its texture for both sides of a part: one texel can only hold one colour.
// DC-DEFAULT: VAR-10 (proposed; USER REQUEST 2026-11-05 "map the lower res model and still show the first person quality camo on top so that we
// have a full weapon model")

use crate::lens::SkinTri;
use crate::shape::Shape;
use dc_render::camo::Circle;
use dc_render::TextureImage;
use glam::{Mat3, Quat, Vec2, Vec3};

/// The side of the baked texture.
pub const BAKE_SIDE: u32 = 2048;
/// Below this overlap of their outlines from the side, a third-person model is not close enough to the same gun to use.
pub const MIN_OVERLAP: f32 = 0.6;
/// Above this gap between the two models' surfaces, as a share of the gun's length, they only look alike from the side. Guns measure 0.11 or less; a
/// first-person item, such as a medkit or a grenade, is built for its view alone and measures from 0.16 up.
// OBSERVED: the player's own ROM: every gun at 0.114 (M136 AT4) or less; Satellite Scanner 0.168, First Aid Kit 0.190, Turkey Leg 0.248, M67 Frag 0.249,
// G.I. Shovel 0.266, Medkit 0.528
pub const MAX_GAP: f32 = 0.15;
/// How finely the outlines are compared, in cells along the gun.
const OUTLINE_CELLS: f32 = 256.0;
/// The largest tilt between the two models that is looked for, in degrees. A third-person model is posed in a hand, nose down a little.
const MAX_TILT: f32 = 8.0;
/// A barrel leaning further than this from the measured line is something else, in degrees.
const MAX_YAW: f32 = 8.0;
/// The share of the gun's length at the muzzle taken as its barrel.
const BARREL_SHARE: f32 = 0.15;
/// How finely surfaces are looked up, in cells along the gun.
const LOOK_CELLS: f32 = 1536.0;
/// A surface turned further than this from a look along an axis is not looked up along it (the dot of its normal and the axis).
const MIN_FACING: f32 = 0.2;
/// A texel looks along each axis its face turns at least this far toward, the most-facing first.
const LOOK_FACING: f32 = 0.35;
/// A face turned at least this far to the side looks from the side first (the dot of its normal and the axis across the gun).
const SIDE_FIRST: f32 = 0.3;
/// How far around a place a surface is looked for, in cells, in rings: a model's small differences of shape do not leave holes.
const RINGS: [i32; 7] = [0, 1, 2, 4, 8, 16, 32];

/// A model's directions: toward the muzzle, up, and to its right.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Frame {
    muzzle: Vec3,
    up: Vec3,
    right: Vec3,
}

impl Frame {
    fn of(shape: &Shape) -> Frame {
        let muzzle = shape.toward_muzzle;
        Frame {
            muzzle,
            up: Vec3::Y,
            right: muzzle.cross(Vec3::Y).normalize_or_zero(),
        }
    }

    /// As this one, the other way round.
    fn turned_round(&self) -> Frame {
        Frame {
            muzzle: -self.muzzle,
            up: self.up,
            right: -self.right,
        }
    }

    /// As this one, turned level by `degrees` about its vertical.
    fn yawed(&self, degrees: f32) -> Frame {
        let (s, c) = degrees.to_radians().sin_cos();
        let muzzle = self.muzzle * c + self.right * s;
        Frame {
            muzzle,
            up: self.up,
            right: muzzle.cross(self.up).normalize_or_zero(),
        }
    }

    /// As this one, with the muzzle raised by `degrees`.
    fn tilted(&self, degrees: f32) -> Frame {
        let (s, c) = degrees.to_radians().sin_cos();
        Frame {
            muzzle: self.muzzle * c + self.up * s,
            up: self.up * c - self.muzzle * s,
            right: self.right,
        }
    }

    /// A vector's parts along the muzzle, up and right.
    fn read(&self, p: Vec3) -> [f32; 3] {
        [p.dot(self.muzzle), p.dot(self.up), p.dot(self.right)]
    }

    fn matrix(&self) -> Mat3 {
        Mat3::from_cols(self.muzzle, self.up, self.right)
    }
}

/// How the third-person model sits over the first-person one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    from: Frame,
    to: Frame,
    /// Along the gun, up and across: the scale and the shift from the third-person model's parts to the first-person model's.
    scale: [f32; 3],
    shift: [f32; 3],
    /// The first-person gun's length.
    length: f32,
    /// How well their outlines from the side overlap: what both cover, as a share of what either covers.
    pub overlap: f32,
    /// How far the third-person model's nose is down against the first-person one, in degrees.
    pub tilt: f32,
}

impl Fit {
    /// A third-person point's parts along the first-person gun: along it, up, and across.
    fn place(&self, p: Vec3) -> [f32; 3] {
        let v = self.from.read(p);
        std::array::from_fn(|k| v[k] * self.scale[k] + self.shift[k])
    }

    /// The turn that stands the third-person model the way the first-person one stands.
    pub fn align(&self) -> Quat {
        Quat::from_mat3(&(self.to.matrix() * self.from.matrix().transpose())).normalize()
    }
}

/// Visits the cells a triangle covers, with its weights at each.
fn cells_of(
    q: [Vec2; 3],
    lo: Vec2,
    cell: f32,
    size: (usize, usize),
    mut visit: impl FnMut(usize, usize, [f32; 3]),
) {
    let area = (q[1] - q[0]).perp_dot(q[2] - q[0]);
    if area.abs() < 1e-12 || !area.is_finite() {
        return;
    }
    let top = q[0].min(q[1]).min(q[2]);
    let low = q[0].max(q[1]).max(q[2]);
    let first = |v: f32| ((v / cell).floor().max(0.0)) as usize;
    let (x0, y0) = (first(top.x - lo.x), first(top.y - lo.y));
    let (x1, y1) = (
        (first(low.x - lo.x) + 1).min(size.0),
        (first(low.y - lo.y) + 1).min(size.1),
    );
    for y in y0..y1 {
        for x in x0..x1 {
            let c = lo + Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * cell;
            let w1 = (c - q[0]).perp_dot(q[2] - q[0]) / area;
            let w2 = (q[1] - q[0]).perp_dot(c - q[0]) / area;
            let w0 = 1.0 - w1 - w2;
            if w0 >= -0.02 && w1 >= -0.02 && w2 >= -0.02 {
                visit(x, y, [w0, w1, w2]);
            }
        }
    }
}

/// An outline from the side, in cells.
struct Outline {
    lo: Vec2,
    cell: f32,
    size: (usize, usize),
    on: Vec<bool>,
}

impl Outline {
    fn new(lo: Vec2, hi: Vec2, cell: f32) -> Outline {
        let size = (
            ((hi.x - lo.x) / cell).ceil() as usize + 1,
            ((hi.y - lo.y) / cell).ceil() as usize + 1,
        );
        Outline {
            lo,
            cell,
            size,
            on: vec![false; size.0 * size.1],
        }
    }

    fn draw(&mut self, q: [Vec2; 3]) {
        let (w, on) = (self.size.0, &mut self.on);
        cells_of(q, self.lo, self.cell, self.size, |x, y, _| {
            on[y * w + x] = true
        });
    }

    fn overlap(&self, other: &Outline) -> f32 {
        let (mut both, mut either) = (0u32, 0u32);
        for (a, b) in self.on.iter().zip(&other.on) {
            both += (*a && *b) as u32;
            either += (*a || *b) as u32;
        }
        both as f32 / either.max(1) as f32
    }
}

/// The third-person model's barrel line against its measured direction, in degrees, and where the barrel runs across the model.
fn barrel_line(frame: &Frame, tris: &[SkinTri]) -> Option<(f32, f32)> {
    let mids: Vec<([f32; 3], f32)> = tris
        .iter()
        .map(|t| {
            (
                frame.read((t.points[0] + t.points[1] + t.points[2]) / 3.0),
                t.area,
            )
        })
        .collect();
    let hi = mids.iter().map(|m| m.0[0]).fold(f32::MIN, f32::max);
    let lo = mids.iter().map(|m| m.0[0]).fold(f32::MAX, f32::min);
    let from = hi - (hi - lo) * BARREL_SHARE;
    let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (v, area) in mids.iter().filter(|m| m.0[0] >= from) {
        let (w, x, y) = (*area as f64, v[0] as f64, v[2] as f64);
        n += w;
        sx += w * x;
        sy += w * y;
        sxx += w * x * x;
        sxy += w * x * y;
    }
    let det = n * sxx - sx * sx;
    if n <= 0.0 {
        return None;
    }
    let slope = if det.abs() < 1e-12 {
        0.0
    } else {
        (n * sxy - sx * sy) / det
    };
    let degrees = (slope as f32).atan().to_degrees();
    Some((degrees, (sy / n) as f32))
}

/// A third-person model's triangles from the side, as `fit` would place them.
#[derive(Clone, Copy)]
struct Placing {
    tilt: f32,
    scale: [f32; 2],
    shift: [f32; 2],
}

/// Fits the third-person model over the first-person one, by their outlines from the side: the tilt first and then, from there, a few small moves
/// of every number while the overlap grows. Across the gun, where the first-person model may have one side only, the two centre lines are lined up.
pub fn fit(hh: &[SkinTri], hh_shape: &Shape, pv: &[SkinTri], pv_shape: &Shape) -> Option<Fit> {
    let ahead = fit_from(hh, Frame::of(hh_shape), pv, pv_shape);
    // which end is the muzzle is measured on each model alone, from the height of its ends. A short or a bullpup gun can read the wrong way round on one
    // of them, so the other way is tried too, and the outlines say which is right
    let round = fit_from(hh, Frame::of(hh_shape).turned_round(), pv, pv_shape);
    match (ahead, round) {
        (Some(a), Some(b)) => Some(if b.overlap > a.overlap { b } else { a }),
        (a, b) => a.or(b),
    }
}

fn fit_from(hh: &[SkinTri], level: Frame, pv: &[SkinTri], pv_shape: &Shape) -> Option<Fit> {
    if hh.is_empty() || pv.is_empty() {
        return None;
    }
    let to = Frame::of(pv_shape);
    let (yaw, _) = barrel_line(&level, hh)?;
    let level = if yaw.abs() <= MAX_YAW {
        level.yawed(yaw)
    } else {
        level
    };
    let flat = |frame: &Frame, p: Vec3| {
        let v = frame.read(p);
        Vec2::new(v[0], v[1])
    };
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for t in pv {
        for p in t.points {
            let v = flat(&to, p);
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    let length = hi.x - lo.x;
    if length <= 0.0 || !length.is_finite() {
        return None;
    }
    let cell = length / OUTLINE_CELLS;
    let margin = Vec2::splat(length * 0.1);
    let mut target = Outline::new(lo - margin, hi + margin, cell);
    for t in pv {
        target.draw(t.points.map(|p| flat(&to, p)));
    }
    let score = |place: &Placing| -> f32 {
        let frame = level.tilted(place.tilt);
        let mut drawn = Outline::new(lo - margin, hi + margin, cell);
        for t in hh {
            drawn.draw(t.points.map(|p| {
                let v = flat(&frame, p);
                Vec2::new(
                    v.x * place.scale[0] + place.shift[0],
                    v.y * place.scale[1] + place.shift[1],
                )
            }));
        }
        drawn.overlap(&target)
    };
    // ends to ends at each tilt, and the tilt that overlaps most
    let start = |tilt: f32| -> Option<Placing> {
        let frame = level.tilted(tilt);
        let (mut a, mut b) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for t in hh {
            for p in t.points {
                let v = flat(&frame, p);
                a = a.min(v);
                b = b.max(v);
            }
        }
        let scale = length / (b.x - a.x);
        (scale.is_finite() && scale > 0.0).then_some(Placing {
            tilt,
            scale: [scale, scale],
            shift: [
                lo.x - a.x * scale,
                (lo.y + hi.y) * 0.5 - (a.y + b.y) * 0.5 * scale,
            ],
        })
    };
    let mut best: Option<(Placing, f32)> = None;
    for k in 0..=(MAX_TILT * 4.0) as i32 {
        let tilt = -MAX_TILT + k as f32 * 0.5;
        if let Some(p) = start(tilt) {
            let s = score(&p);
            if best.as_ref().is_none_or(|b| s > b.1) {
                best = Some((p, s));
            }
        }
    }
    let (mut place, mut value) = best?;
    let mut steps = [0.5, 0.02, 0.02, length * 0.01, length * 0.01];
    for _ in 0..4 {
        for (k, step) in steps.iter().enumerate() {
            for sign in [1.0, -1.0] {
                for _ in 0..6 {
                    let mut p = place;
                    let d = *step * sign;
                    match k {
                        0 => p.tilt += d,
                        1 => p.scale[0] *= 1.0 + d,
                        2 => p.scale[1] *= 1.0 + d,
                        3 => p.shift[0] += d,
                        _ => p.shift[1] += d,
                    }
                    let s = score(&p);
                    if s <= value {
                        break;
                    }
                    (place, value) = (p, s);
                }
            }
        }
        steps = steps.map(|s| s * 0.5);
    }
    let from = level.tilted(place.tilt);
    let (_, hh_centre) = barrel_line(&from, hh)?;
    let pv_centre = to.read(pv_shape.across * pv_shape.centre)[2];
    let across = (place.scale[0] + place.scale[1]) * 0.5;
    Some(Fit {
        from,
        to,
        scale: [place.scale[0], place.scale[1], across],
        shift: [
            place.shift[0],
            place.shift[1],
            pv_centre - hh_centre * across,
        ],
        length,
        overlap: value,
        tilt: -place.tilt,
    })
}

/// Where one texel of the baked texture reads from: which skin, and where on it, in texels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Texel {
    pub source: u16,
    pub at: Vec2,
}

/// A worked-out lookup for one gun.
#[derive(Debug, Clone, PartialEq)]
pub struct Lookup {
    pub side: u32,
    pub texels: Vec<Option<Texel>>,
    /// The share of the third-person surface that found a place on the first-person gun.
    pub matched: f32,
    /// How far on average the found places were from the third-person surface along the look, as a share of the gun's length.
    pub gap: f32,
}

/// One first-person skin's triangles.
pub struct Source {
    pub tris: Vec<SkinTri>,
}

/// A first-person triangle as the looks read it: its corners' parts along the gun, up and across.
struct Target {
    corners: [[f32; 3]; 3],
    texels: [Vec2; 3],
    source: u16,
}

const NONE: u32 = u32::MAX;

/// The outermost surfaces along one axis: for each cell of the two other axes, the triangle furthest along the axis each way.
struct Layer {
    /// The axis looked along and the two across it.
    axis: usize,
    across: [usize; 2],
    lo: Vec2,
    cell: f32,
    size: (usize, usize),
    /// Furthest along the axis, and furthest back.
    ahead: Vec<u32>,
    back: Vec<u32>,
}

impl Layer {
    fn new(axis: usize, targets: &[Target], lo: [f32; 3], hi: [f32; 3], cell: f32) -> Layer {
        let across = match axis {
            0 => [2, 1],
            1 => [0, 2],
            _ => [0, 1],
        };
        let corner = Vec2::new(lo[across[0]], lo[across[1]]);
        let size = (
            ((hi[across[0]] - lo[across[0]]) / cell).ceil() as usize + 1,
            ((hi[across[1]] - lo[across[1]]) / cell).ceil() as usize + 1,
        );
        let count = size.0 * size.1;
        let (mut ahead, mut back) = (vec![NONE; count], vec![NONE; count]);
        let (mut most, mut least) = (vec![f32::MIN; count], vec![f32::MAX; count]);
        for (k, t) in targets.iter().enumerate() {
            let c = t.corners;
            let n = (Vec3::from(c[1]) - Vec3::from(c[0]))
                .cross(Vec3::from(c[2]) - Vec3::from(c[0]))
                .normalize_or_zero();
            if n[axis].abs() < MIN_FACING {
                continue;
            }
            let q = c.map(|p| Vec2::new(p[across[0]], p[across[1]]));
            cells_of(q, corner, cell, size, |x, y, w| {
                let d = c[0][axis] * w[0] + c[1][axis] * w[1] + c[2][axis] * w[2];
                let i = y * size.0 + x;
                if d > most[i] {
                    most[i] = d;
                    ahead[i] = k as u32;
                }
                if d < least[i] {
                    least[i] = d;
                    back[i] = k as u32;
                }
            });
        }
        Layer {
            axis,
            across,
            lo: corner,
            cell,
            size,
            ahead,
            back,
        }
    }

    /// The surface at a place, furthest along the axis or furthest back. A place that may be read from either side of the centre line is read from the
    /// other side when its own has nothing there.
    fn find(&self, p: [f32; 3], ahead: bool, mirror: Option<f32>) -> Option<(u32, Vec2)> {
        let place = Vec2::new(p[self.across[0]], p[self.across[1]]);
        let other = mirror.map(|m| {
            if self.across[0] == 2 {
                Vec2::new(2.0 * m - place.x, place.y)
            } else {
                Vec2::new(place.x, 2.0 * m - place.y)
            }
        });
        let list = if ahead { &self.ahead } else { &self.back };
        let at = |q: Vec2, dx: i32, dy: i32| -> Option<u32> {
            let v = (q - self.lo) / self.cell;
            let (x, y) = (v.x.floor() as i32 + dx, v.y.floor() as i32 + dy);
            if x < 0 || y < 0 || x >= self.size.0 as i32 || y >= self.size.1 as i32 {
                return None;
            }
            let t = list[y as usize * self.size.0 + x as usize];
            (t != NONE).then_some(t)
        };
        for r in RINGS {
            let around: &[(i32, i32)] = if r == 0 {
                &[(0, 0)]
            } else {
                &[
                    (1, 0),
                    (-1, 0),
                    (0, 1),
                    (0, -1),
                    (1, 1),
                    (-1, 1),
                    (1, -1),
                    (-1, -1),
                ]
            };
            for (dx, dy) in around {
                if let Some(t) = at(place, dx * r, dy * r) {
                    return Some((t, place));
                }
                if let Some(q) = other {
                    if let Some(t) = at(q, dx * r, dy * r) {
                        return Some((t, q));
                    }
                }
            }
        }
        None
    }
}

/// Where a place falls on a target's texture: on the triangle, or at the nearest point of it when a ring found it nearby.
fn read_target(t: &Target, layer: &Layer, place: Vec2) -> (Vec2, f32) {
    let q = t
        .corners
        .map(|p| Vec2::new(p[layer.across[0]], p[layer.across[1]]));
    let area = (q[1] - q[0]).perp_dot(q[2] - q[0]);
    let mut w = if area.abs() < 1e-12 {
        [1.0 / 3.0; 3]
    } else {
        let w1 = (place - q[0]).perp_dot(q[2] - q[0]) / area;
        let w2 = (q[1] - q[0]).perp_dot(place - q[0]) / area;
        [1.0 - w1 - w2, w1, w2]
    };
    let mut total = 0.0;
    for v in &mut w {
        *v = v.max(0.0);
        total += *v;
    }
    for v in &mut w {
        *v /= f32::max(total, 1e-6);
    }
    let at = t.texels[0] * w[0] + t.texels[1] * w[1] + t.texels[2] * w[2];
    let depth = t.corners[0][layer.axis] * w[0]
        + t.corners[1][layer.axis] * w[1]
        + t.corners[2][layer.axis] * w[2];
    (at, depth)
}

/// Fills small gaps at the edges of the texture's parts, so that smoothing across an edge does not pick up a colour from outside.
fn grow_edges(texels: &mut [Option<Texel>], side: usize, passes: usize) {
    for _ in 0..passes {
        let before = texels.to_vec();
        for y in 0..side {
            for x in 0..side {
                if before[y * side + x].is_some() {
                    continue;
                }
                let around = [
                    (x > 0).then(|| y * side + x - 1),
                    (x + 1 < side).then(|| y * side + x + 1),
                    (y > 0).then(|| (y - 1) * side + x),
                    (y + 1 < side).then(|| (y + 1) * side + x),
                ];
                if let Some(t) = around.into_iter().flatten().find_map(|i| before[i]) {
                    texels[y * side + x] = Some(t);
                }
            }
        }
    }
}

/// Which way out a closed model's triangles face: 1 when their winding points outward, -1 when it points in.
fn outward_sign(hh: &[SkinTri]) -> f32 {
    let (mut mid, mut area) = (Vec3::ZERO, 0.0);
    for t in hh {
        mid += (t.points[0] + t.points[1] + t.points[2]) / 3.0 * t.area;
        area += t.area;
    }
    let mid = mid / f32::max(area, 1e-6);
    let lean: f32 = hh
        .iter()
        .map(|t| ((t.points[0] + t.points[1] + t.points[2]) / 3.0 - mid).dot(t.normal) * t.area)
        .sum();
    if lean < 0.0 {
        -1.0
    } else {
        1.0
    }
}

/// Works out the lookup: every texel the third-person model uses, at `side` x `side`, reads the first-person surface it looks at. `hh` holds the
/// third-person triangles with their corners in texels of a `hh_size` texture.
pub fn bake_lookup(
    hh: &[SkinTri],
    hh_size: (u32, u32),
    fit: &Fit,
    pv_shape: &Shape,
    sources: &[Source],
    side: u32,
) -> Option<Lookup> {
    let mut targets = Vec::new();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (k, s) in sources.iter().enumerate() {
        for t in &s.tris {
            let corners = t.points.map(|p| fit.to.read(p));
            for c in corners {
                for a in 0..3 {
                    lo[a] = lo[a].min(c[a]);
                    hi[a] = hi[a].max(c[a]);
                }
            }
            targets.push(Target {
                corners,
                texels: t.texels,
                source: k as u16,
            });
        }
    }
    if targets.is_empty() {
        return None;
    }
    let cell = fit.length / LOOK_CELLS;
    let layers: [Layer; 3] = std::array::from_fn(|axis| Layer::new(axis, &targets, lo, hi, cell));
    let centre = fit.to.read(pv_shape.across * pv_shape.centre)[2];
    let players_side = fit.to.read(pv_shape.across)[2];
    let n = side as usize;
    let scale = Vec2::new(
        side as f32 / hh_size.0 as f32,
        side as f32 / hh_size.1 as f32,
    );
    let sign = outward_sign(hh);
    let threads = std::thread::available_parallelism()
        .map_or(4, |v| v.get())
        .clamp(1, 16);
    let mut texels: Vec<Option<Texel>> = vec![None; n * n];
    // which triangle covers each texel
    let mut cover: Vec<Option<(u32, [f32; 3])>> = vec![None; n * n];
    for (k, t) in hh.iter().enumerate() {
        let q = t.texels.map(|v| v * scale);
        cells_of(q, Vec2::ZERO, 1.0, (n, n), |x, y, w| {
            cover[y * n + x].get_or_insert((k as u32, w));
        });
    }
    let rows = n.div_ceil(threads * 4).max(1);
    let jobs: Vec<_> = texels
        .chunks_mut(rows * n)
        .zip(cover.chunks(rows * n))
        .collect();
    let queue = std::sync::Mutex::new(jobs);
    let (mut found, mut asked, mut off) = (0u64, 0u64, 0.0f64);
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..threads {
            let (queue, layers, targets) = (&queue, &layers, &targets);
            workers.push(scope.spawn(move || {
                let (mut hit, mut seen, mut distance) = (0u64, 0u64, 0.0f64);
                loop {
                    let job = match queue.lock() {
                        Ok(mut q) => q.pop(),
                        Err(_) => None,
                    };
                    let Some((out, cov)) = job else {
                        break;
                    };
                    for (slot, c) in out.iter_mut().zip(cov) {
                        let Some((k, w)) = c else {
                            continue;
                        };
                        seen += 1;
                        let t = &hh[*k as usize];
                        let p =
                            fit.place(t.points[0] * w[0] + t.points[1] * w[1] + t.points[2] * w[2]);
                        let facing = fit.from.read(t.normal * sign);
                        // from the side whenever a face turns that way at all, so a pattern stays in one piece around a bevel or a barrel; else the way
                        // it most faces. The side is also the last look, since that is the view the model was made for
                        let side_view = 2;
                        let mut order = [0, 1, 2];
                        order.sort_by(|a, b| facing[*b].abs().total_cmp(&facing[*a].abs()));
                        let sideways = facing[side_view].abs() >= SIDE_FIRST;
                        let tries = sideways
                            .then_some(side_view)
                            .into_iter()
                            .chain(
                                order
                                    .iter()
                                    .copied()
                                    .filter(|a| facing[*a].abs() >= LOOK_FACING),
                            )
                            .chain(std::iter::once(side_view));
                        let mut chosen = None;
                        for axis in tries {
                            // a look down on the gun or along it may find its surface on the far side of the centre line; a look from the side always
                            // reads the side the player sees
                            let (ahead, mirror) = if axis == side_view {
                                (players_side > 0.0, None)
                            } else {
                                (facing[axis] > 0.0, Some(centre))
                            };
                            if let Some((id, place)) = layers[axis].find(p, ahead, mirror) {
                                chosen = Some((axis, id, place));
                                break;
                            }
                        }
                        if let Some((axis, id, place)) = chosen {
                            let target = &targets[id as usize];
                            let (at, depth) = read_target(target, &layers[axis], place);
                            *slot = Some(Texel {
                                source: target.source,
                                at,
                            });
                            hit += 1;
                            distance += (depth - p[axis]).abs() as f64;
                        }
                    }
                }
                (hit, seen, distance)
            }));
        }
        for j in workers {
            if let Ok((hit, seen, distance)) = j.join() {
                found += hit;
                asked += seen;
                off += distance;
            }
        }
    });
    grow_edges(&mut texels, n, 3);
    Some(Lookup {
        side,
        texels,
        matched: found as f32 / asked.max(1) as f32,
        gap: (off / found.max(1) as f64) as f32 / fit.length.max(1e-6),
    })
}

/// Leaves a third-person lens clean. A look from its texels may land on the first-person gun's paint, as a first-person model has no front lens and its rear
/// lens need not line up, so inside each of `hh_lenses` (in texels of a `hh_size` texture) a texel keeps its look only if that lands on a first-person lens,
/// or on a skin other than `main`, which is not painted. The rest read the third-person texture's own lens.
pub fn keep_lenses(
    lookup: &mut Lookup,
    hh_lenses: &[Circle],
    hh_size: (u32, u32),
    pv_lenses: &[Circle],
    main: u16,
) {
    let n = lookup.side as usize;
    let scale = Vec2::new(
        lookup.side as f32 / hh_size.0.max(1) as f32,
        lookup.side as f32 / hh_size.1.max(1) as f32,
    );
    let inside = |c: &Circle, p: Vec2| (p.x - c.x).hypot(p.y - c.y) <= c.radius;
    for c in hh_lenses {
        let centre = Vec2::new(c.x, c.y) * scale;
        let radius = c.radius * scale.max_element();
        let x0 = (centre.x - radius).floor().max(0.0) as usize;
        let y0 = (centre.y - radius).floor().max(0.0) as usize;
        let x1 = ((centre.x + radius).ceil() as usize).min(n);
        let y1 = ((centre.y + radius).ceil() as usize).min(n);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                if (p - centre).length() > radius {
                    continue;
                }
                let slot = &mut lookup.texels[y * n + x];
                let clean = slot
                    .is_some_and(|t| t.source != main || pv_lenses.iter().any(|l| inside(l, t.at)));
                if !clean {
                    *slot = None;
                }
            }
        }
    }
}

fn sample(img: &TextureImage, at: Vec2) -> [f32; 4] {
    let Some(px) = img.levels.first() else {
        return [0.0, 0.0, 0.0, 255.0];
    };
    let (w, h) = (img.width as i32, img.height as i32);
    let (fx, fy) = (at.x - 0.5, at.y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let read = |x: i32, y: i32| -> [f32; 4] {
        let i = ((y.rem_euclid(h) * w + x.rem_euclid(w)) * 4) as usize;
        match px.get(i..i + 4) {
            Some(c) => [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32],
            None => [0.0, 0.0, 0.0, 255.0],
        }
    };
    let (x, y) = (x0 as i32, y0 as i32);
    let (a, b, c, d) = (
        read(x, y),
        read(x + 1, y),
        read(x, y + 1),
        read(x + 1, y + 1),
    );
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let low = c[k] + (d[k] - c[k]) * tx;
        top + (low - top) * ty
    })
}

/// A third-person texture at the size of the bake, so that a pattern painted on it for what found no place is as sharp as the rest.
pub fn enlarged(img: &TextureImage, side: u32) -> TextureImage {
    let stretch = Vec2::new(
        img.width as f32 / side as f32,
        img.height as f32 / side as f32,
    );
    let mut out = vec![0u8; (side * side * 4) as usize];
    for y in 0..side {
        for x in 0..side {
            let c = sample(img, Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * stretch);
            let i = ((y * side + x) * 4) as usize;
            for k in 0..4 {
                out[i + k] = c[k].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    let mut big = TextureImage::rgba(side, side, out);
    big.alpha_cutoff = img.alpha_cutoff;
    big.mip_bias = img.mip_bias;
    big
}

/// The baked texture: each texel from the first-person skins through the lookup, and what found no place from `own`, the third-person texture at
/// the same size. Its alpha stays, so that a cut-out part stays cut out.
pub fn bake(lookup: &Lookup, skins: &[Option<&TextureImage>], own: &TextureImage) -> TextureImage {
    let n = lookup.side as usize;
    let mut out = vec![0u8; n * n * 4];
    let Some(own_px) = own.levels.first().filter(|p| p.len() >= n * n * 4) else {
        return TextureImage::rgba(lookup.side, lookup.side, out);
    };
    let threads = std::thread::available_parallelism()
        .map_or(4, |v| v.get())
        .clamp(1, 16);
    let rows = n.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (r, row) in out.chunks_mut(rows * n * 4).enumerate() {
            let first = r * rows * n;
            scope.spawn(move || {
                for (k, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let at = first + k;
                    let mine = &own_px[at * 4..at * 4 + 4];
                    let colour = lookup.texels[at].and_then(|t| {
                        skins
                            .get(t.source as usize)
                            .copied()
                            .flatten()
                            .map(|s| sample(s, t.at))
                    });
                    for c in 0..3 {
                        px[c] = match colour {
                            Some(v) => v[c].round().clamp(0.0, 255.0) as u8,
                            None => mine[c],
                        };
                    }
                    px[3] = mine[3];
                }
            });
        }
    });
    let mut img = TextureImage::rgba(lookup.side, lookup.side, out);
    img.alpha_cutoff = own.alpha_cutoff;
    img.mip_bias = own.mip_bias;
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One flat quad, with its corners on the texture.
    fn quad(p: [Vec3; 4], q: [Vec2; 4]) -> Vec<SkinTri> {
        [[0, 1, 2], [0, 2, 3]]
            .map(|c: [usize; 3]| {
                let points = c.map(|i| p[i]);
                let cross = (points[1] - points[0]).cross(points[2] - points[0]);
                SkinTri {
                    points,
                    texels: c.map(|i| q[i]),
                    normal: cross.normalize(),
                    area: cross.length() * 0.5,
                }
            })
            .to_vec()
    }

    fn texture_part(x: f32, y: f32, w: f32, h: f32) -> [Vec2; 4] {
        [
            Vec2::new(x, y),
            Vec2::new(x + w, y),
            Vec2::new(x + w, y + h),
            Vec2::new(x, y + h),
        ]
    }

    /// A wall facing along x at `x`, along z from 0 to `long`, in height from 0 to `high`, with its winding facing away from the middle.
    fn wall(x: f32, long: f32, high: f32, part: [Vec2; 4]) -> Vec<SkinTri> {
        let p = [
            Vec3::new(x, 0.0, 0.0),
            Vec3::new(x, 0.0, long),
            Vec3::new(x, high, long),
            Vec3::new(x, high, 0.0),
        ];
        let mut t = quad(p, part);
        if (t[0].normal.x > 0.0) != (x > 0.0) {
            t = quad(
                [p[1], p[0], p[3], p[2]],
                [part[1], part[0], part[3], part[2]],
            );
        }
        t
    }

    /// A lid facing up at `y`.
    fn lid(y: f32, half: f32, long: f32, part: [Vec2; 4]) -> Vec<SkinTri> {
        let p = [
            Vec3::new(-half, y, 0.0),
            Vec3::new(half, y, 0.0),
            Vec3::new(half, y, long),
            Vec3::new(-half, y, long),
        ];
        let mut t = quad(p, part);
        if t[0].normal.y < 0.0 {
            t = quad(
                [p[1], p[0], p[3], p[2]],
                [part[1], part[0], part[3], part[2]],
            );
        }
        t
    }

    fn shape_of(across: Vec3, centre: f32) -> Shape {
        Shape {
            across,
            centre,
            balance: 0.3,
            spread: 0.02,
            toward_muzzle: Vec3::Z,
        }
    }

    /// The first-person gun: its near wall on the -x side and its lid, 100 long and 20 high.
    fn first_person() -> (Vec<SkinTri>, Shape) {
        let mut t = wall(-2.0, 100.0, 20.0, texture_part(0.0, 0.0, 500.0, 100.0));
        t.extend(lid(20.0, 2.0, 100.0, texture_part(0.0, 200.0, 500.0, 40.0)));
        (t, shape_of(Vec3::NEG_X, 0.0))
    }

    /// The third-person gun: both walls and the lid, half the size, each on its own part of a small texture.
    fn third_person() -> Vec<SkinTri> {
        let mut t = wall(-1.0, 50.0, 10.0, texture_part(0.0, 0.0, 64.0, 64.0));
        t.extend(wall(1.0, 50.0, 10.0, texture_part(128.0, 0.0, 64.0, 64.0)));
        t.extend(lid(10.0, 1.0, 50.0, texture_part(0.0, 128.0, 64.0, 16.0)));
        t
    }

    fn hh_shape() -> Shape {
        shape_of(Vec3::NEG_X, 0.0)
    }

    fn lookup() -> Lookup {
        let (pv, pv_shape) = first_person();
        let hh = third_person();
        let f = fit(&hh, &hh_shape(), &pv, &pv_shape).unwrap();
        bake_lookup(&hh, (256, 256), &f, &pv_shape, &[Source { tris: pv }], 256).unwrap()
    }

    fn tilted(tris: &[SkinTri], degrees: f32) -> Vec<SkinTri> {
        let turn = Quat::from_rotation_x(degrees.to_radians());
        tris.iter()
            .map(|t| SkinTri {
                points: t.points.map(|p| turn * p),
                normal: turn * t.normal,
                ..*t
            })
            .collect()
    }

    #[test]
    fn a_smaller_model_is_fitted_over_the_larger_one() {
        let (pv, pv_shape) = first_person();
        let f = fit(&third_person(), &hh_shape(), &pv, &pv_shape).unwrap();
        assert!(f.overlap > 0.9, "{f:?}");
        let [long, up, _] = f.place(Vec3::new(-1.0, 10.0, 50.0));
        assert!((long - 100.0).abs() < 1.5, "{long}");
        assert!((up - 20.0).abs() < 1.5, "{up}");
    }

    #[test]
    fn a_model_held_nose_down_is_found_and_stands_level_again() {
        let (pv, pv_shape) = first_person();
        let hh = tilted(&third_person(), 4.0);
        let f = fit(&hh, &hh_shape(), &pv, &pv_shape).unwrap();
        assert!(f.overlap > 0.85, "{f:?}");
        assert!((f.tilt.abs() - 4.0).abs() < 0.6, "{f:?}");
        let along = f.align() * (Quat::from_rotation_x(4.0_f32.to_radians()) * Vec3::Z);
        assert!(along.dot(Vec3::Z) > 0.999, "{along:?}");
    }

    #[test]
    fn a_muzzle_found_at_the_wrong_end_of_one_model_is_put_right_by_the_outlines() {
        // a gun with a tall end and a low end, so that its outline only fits one way round
        let body = |long: f32, high: f32, part: [Vec2; 4]| {
            let mut t = wall(-1.0, long, high * 0.25, part);
            let rear = quad(
                [
                    Vec3::new(-1.0, 0.0, 0.0),
                    Vec3::new(-1.0, 0.0, long * 0.3),
                    Vec3::new(-1.0, high, long * 0.3),
                    Vec3::new(-1.0, high, 0.0),
                ],
                part,
            );
            t.extend(rear);
            t
        };
        let pv = body(100.0, 20.0, texture_part(0.0, 0.0, 500.0, 100.0));
        let hh = body(50.0, 10.0, texture_part(0.0, 0.0, 64.0, 64.0));
        let wrong = Shape {
            toward_muzzle: Vec3::NEG_Z,
            ..hh_shape()
        };
        let f = fit(&hh, &wrong, &pv, &shape_of(Vec3::NEG_X, 0.0)).unwrap();
        assert!(f.overlap > 0.85, "{f:?}");
        let [long, ..] = f.place(Vec3::new(-1.0, 0.0, 50.0));
        assert!((long - 100.0).abs() < 2.0, "{long}");
    }

    #[test]
    fn the_near_side_reads_the_same_place_on_the_first_person_texture() {
        let l = lookup();
        assert!(l.matched > 0.95, "{}", l.matched);
        // a quarter of the way along the wall's part of the texture, half way up
        let t = l.texels[32 * 256 + 16].expect("it found a place");
        assert!((t.at.x - 125.0).abs() < 12.0, "{t:?}");
        assert!((t.at.y - 50.0).abs() < 12.0, "{t:?}");
    }

    #[test]
    fn the_far_side_has_its_own_geometry_and_reads_the_same_place_along_the_gun() {
        let l = lookup();
        let near = l.texels[32 * 256 + 16].expect("near");
        let far = l.texels[32 * 256 + 128 + 16].expect("the far wall is not left out");
        assert!((far.at - near.at).length() < 12.0, "{near:?} {far:?}");
    }

    #[test]
    fn both_sides_read_the_players_side_even_where_the_first_person_model_has_a_far_wall() {
        let (mut pv, pv_shape) = first_person();
        // a rough far wall on its own part of the texture
        pv.extend(wall(
            2.0,
            100.0,
            20.0,
            texture_part(0.0, 300.0, 500.0, 100.0),
        ));
        let hh = third_person();
        let f = fit(&hh, &hh_shape(), &pv, &pv_shape).unwrap();
        let l = bake_lookup(&hh, (256, 256), &f, &pv_shape, &[Source { tris: pv }], 256).unwrap();
        for at in [32 * 256 + 16, 32 * 256 + 128 + 16] {
            let t = l.texels[at].expect("it found a place");
            assert!(t.at.y < 100.0, "the wall the player sees: {t:?}");
        }
    }

    #[test]
    fn a_part_of_the_texture_used_on_both_sides_reads_the_players_side() {
        let (mut pv, pv_shape) = first_person();
        pv.extend(wall(
            2.0,
            100.0,
            20.0,
            texture_part(0.0, 300.0, 500.0, 100.0),
        ));
        // the far wall comes first, and both walls use one part of the third-person texture
        let mut hh = wall(1.0, 50.0, 10.0, texture_part(0.0, 0.0, 64.0, 64.0));
        hh.extend(wall(-1.0, 50.0, 10.0, texture_part(0.0, 0.0, 64.0, 64.0)));
        let f = fit(&hh, &hh_shape(), &pv, &pv_shape).unwrap();
        let l = bake_lookup(&hh, (256, 256), &f, &pv_shape, &[Source { tris: pv }], 256).unwrap();
        let t = l.texels[32 * 256 + 16].expect("it found a place");
        assert!(t.at.y < 100.0, "{t:?}");
    }

    #[test]
    fn a_face_looking_up_reads_the_surface_on_top_and_not_the_wall() {
        let l = lookup();
        let t = l.texels[136 * 256 + 32].expect("the lid found a place");
        assert!(t.at.y >= 200.0 && t.at.y <= 240.0, "{t:?}");
    }

    #[test]
    fn a_texel_the_model_does_not_use_reads_nothing() {
        let l = lookup();
        assert_eq!(l.texels[200 * 256 + 230], None);
    }

    #[test]
    fn a_model_that_is_not_the_same_gun_overlaps_little() {
        let (pv, pv_shape) = first_person();
        // long and low against long and high, with a block far below
        let mut hh = wall(-1.0, 50.0, 1.0, texture_part(0.0, 0.0, 64.0, 64.0));
        hh.extend(quad(
            [
                Vec3::new(-1.0, -60.0, 20.0),
                Vec3::new(-1.0, -60.0, 30.0),
                Vec3::new(-1.0, -10.0, 30.0),
                Vec3::new(-1.0, -10.0, 20.0),
            ],
            texture_part(0.0, 0.0, 64.0, 64.0),
        ));
        let f = fit(&hh, &hh_shape(), &pv, &pv_shape).unwrap();
        assert!(f.overlap < MIN_OVERLAP, "{f:?}");
    }

    #[test]
    fn a_third_person_lens_keeps_only_a_look_that_lands_on_something_unpainted() {
        let on = |source, x: f32| {
            Some(Texel {
                source,
                at: Vec2::new(x, 10.0),
            })
        };
        let mut l = Lookup {
            side: 8,
            texels: vec![on(1, 500.0); 64],
            matched: 1.0,
            gap: 0.0,
        };
        // a first-person lens at 10,10 of the texture; a third-person lens over the left half of a texture half the bake's size
        l.texels[3 * 8 + 1] = on(1, 12.0);
        l.texels[3 * 8 + 2] = on(2, 500.0);
        let circle = Circle {
            x: 1.0,
            y: 2.0,
            radius: 1.0,
        };
        let pv = Circle {
            x: 10.0,
            y: 10.0,
            radius: 5.0,
        };
        keep_lenses(&mut l, &[circle], (4, 4), &[pv], 1);
        assert_eq!(l.texels[3 * 8 + 3], None, "painted: reads its own lens");
        assert!(l.texels[3 * 8 + 1].is_some(), "on the first-person lens");
        assert!(l.texels[3 * 8 + 2].is_some(), "on an unpainted skin");
        assert!(
            l.texels[7 * 8 + 7].is_some(),
            "outside the lens nothing changes"
        );
    }

    #[test]
    fn a_bake_takes_colour_from_the_first_person_skin_and_alpha_from_its_own() {
        let l = lookup();
        let skin = TextureImage::rgba(512, 512, [200, 10, 10, 255].repeat(512 * 512));
        let own = TextureImage::rgba(64, 64, [5, 5, 200, 77].repeat(64 * 64));
        let img = bake(&l, &[Some(&skin)], &enlarged(&own, 256));
        let px = &img.levels[0];
        let at = |x: usize, y: usize| &px[(y * 256 + x) * 4..(y * 256 + x) * 4 + 4];
        assert_eq!(
            at(16, 32),
            &[200, 10, 10, 77],
            "the skin, with its own alpha"
        );
        assert_eq!(
            at(230, 200),
            &[5, 5, 200, 77],
            "what found no place keeps its own"
        );
    }
}
