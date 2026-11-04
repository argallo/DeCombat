//! Baked light for a whole map: lightmaps for its flat surfaces, vertex colours for its models and a light grid for the
//! players (DC-DEFAULT: MAP-108). The world format has lightmaps but the first bake never wrote them: a wall or a floor
//! was one colour per corner, so a pool of light or a soft shadow had nowhere to go.
//!
//! Each flat surface is cut into tiles, each tile gets a rectangle on a 256 x 256 page, and every texel adds:
//! the point lights and the sun, each from a few samples so that shadows have soft edges; one bounce of light gathered
//! from the surfaces around it, so a red wall tints the floor beside it; and the ambient light, less where things stand
//! close. Hidden cores cast shadows but only count as grey for the bounce. A model's corners take the same lights
//! without the cores (its own would shade it), and players read the grid.
//!
//! Provenance: ORIGINAL. Section kinds, texture coordinates and the texel packing follow RENDER_2009.md 5.2 and 6.1.
//! Determinism: every texel depends only on its own place and the fixed ray sets; threads only share out the pages.

use crate::geom::{Level, Poly, SURF_SOLID, V3};
use crate::project::{MapObject, WorldSettings};
use std::collections::BTreeMap;

/// A lightmap page's side: the largest the engine's buffer holds (RENDER_2009.md 6.1).
pub const PAGE: u32 = 256;
/// A tile's longest side in texels, so that it fits a page with room to spare.
const TILE_TEXELS: f32 = 200.0;
/// Rays gathered per texel for the bounce and the contact darkening.
const GATHER: usize = 20;
/// A model's ambient light against a wall's: no bounce is gathered for a corner, and without this a model reads
/// darker than the wall right behind it.
const MODEL_FILL: f32 = 2.2;
/// How far the contact darkening looks, in world units.
const CONTACT: f32 = 110.0;
/// How far a shadow ray to the sun looks.
const SUN_REACH: f32 = 8192.0;
/// The size of the disc a point light's shadow samples spread over.
const LIGHT_SIZE: f32 = 9.0;
/// Light samples per light and for the sun.
const SAMPLES: [[f32; 3]; 4] = [
    [1.0, 0.3, 0.0],
    [-1.0, -0.3, 0.2],
    [0.2, 1.0, -0.9],
    [-0.2, -1.0, 0.9],
];

/// One lamp, from a `dc.point_light`.
#[derive(Debug, Clone, Copy)]
pub struct Lamp {
    pub pos: V3,
    pub colour: [f32; 3],
    pub radius: f32,
}

impl Lamp {
    pub fn of(o: &MapObject) -> Option<Lamp> {
        use crate::project::PropValue;
        use crate::vocab_names as v;
        if o.class != v::CLASS_POINT_LIGHT {
            return None;
        }
        let radius = o.num(v::PROP_RADIUS)? as f32;
        let bright = o.num(v::PROP_BRIGHT).unwrap_or(1.0) as f32;
        let rgb = match o.props.get(v::PROP_COLOR) {
            Some(PropValue::Rgb { rgb }) => *rgb,
            _ => [255, 255, 255],
        };
        Some(Lamp {
            pos: V3::from_array(o.pos),
            colour: rgb.map(|c| c as f32 / 255.0 * bright),
            radius,
        })
    }
}

/// What the bake reads.
pub struct LightInput<'a> {
    pub settings: &'a WorldSettings,
    /// World units a lightmap texel covers.
    pub density: f32,
    /// The ambient light's strength.
    pub ambient: f32,
    /// How strong the sun is against the lamps.
    pub sun: f32,
    /// How strong the one bounce is.
    pub bounce: f32,
    pub lamps: Vec<Lamp>,
    /// A material's average colour, 0 .. 1.
    pub albedo: &'a (dyn Fn(usize) -> [f32; 3] + Sync),
    /// True for a material whose picture has holes: paint, which takes the light of the surface right behind it.
    pub paint: &'a (dyn Fn(usize) -> bool + Sync),
    /// True for a material of collision that leaves the light alone (`MaterialFlags::skin`).
    pub skin: &'a (dyn Fn(usize) -> bool + Sync),
}

/// A flat piece of a surface on a lightmap page.
#[derive(Debug, Clone)]
pub struct LitPiece {
    pub poly: Poly,
    pub page: usize,
    /// Lightmap coordinates per corner.
    pub lm_uv: Vec<[f32; 2]>,
}

/// The bake's result.
pub struct Baked {
    pub pieces: Vec<LitPiece>,
    /// RLE-packed pages (RENDER_2009.md 6.1), `PAGE` x `PAGE`.
    pub pages: Vec<Vec<u8>>,
    /// Corner colours of the polygons left vertex-lit, parallel to the input polygons (empty for a lit one).
    pub vertex_colours: Vec<Vec<u32>>,
    /// Polygons of paint with a corner that has no surface right behind it, or another surface over it.
    pub paint_off_wall: Vec<usize>,
    pub grid_dims: [u32; 3],
    pub grid_origin: V3,
    pub grid_cell: V3,
    pub grid_rgb: Vec<u8>,
}

fn axes(n: V3) -> (V3, V3) {
    let seed = if n.y.abs() > 0.7 {
        V3::new(1.0, 0.0, 0.0)
    } else {
        V3::new(0.0, 1.0, 0.0)
    };
    let u = (seed - n * seed.dot(n))
        .normalized()
        .unwrap_or(V3::new(1.0, 0.0, 0.0));
    let v = V3::new(
        n.y * u.z - n.z * u.y,
        n.z * u.x - n.x * u.z,
        n.x * u.y - n.y * u.x,
    );
    (u, v)
}

/// Keeps the part of a convex outline on the side of `axis . p <= at` (or `>=`).
fn clip(corners: &[V3], axis: V3, at: f32, below: bool) -> Vec<V3> {
    let side = |p: V3| {
        let d = p.dot(axis) - at;
        if below {
            -d
        } else {
            d
        }
    };
    let mut out = Vec::with_capacity(corners.len() + 1);
    for i in 0..corners.len() {
        let (a, b) = (corners[i], corners[(i + 1) % corners.len()]);
        let (da, db) = (side(a), side(b));
        if da >= 0.0 {
            out.push(a);
        }
        if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out
}

/// A polygon cut along a grid of `step` units on its own plane: `(column, row, piece)`.
fn tiles(p: &Poly, step: f32) -> Vec<(i32, i32, Vec<V3>)> {
    let (u, v) = axes(p.plane.n);
    let range = |axis: V3| {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for c in &p.corners {
            lo = lo.min(c.dot(axis));
            hi = hi.max(c.dot(axis));
        }
        (
            (lo / step).floor() as i32,
            ((hi - 1e-3) / step).floor() as i32,
        )
    };
    let (u0, u1) = range(u);
    let (v0, v1) = range(v);
    let mut out = Vec::new();
    for i in u0..=u1 {
        let a = clip(&p.corners, u, i as f32 * step, false);
        let a = clip(&a, u, (i + 1) as f32 * step, true);
        if a.len() < 3 {
            continue;
        }
        for j in v0..=v1 {
            let b = clip(&a, v, j as f32 * step, false);
            let b = clip(&b, v, (j + 1) as f32 * step, true);
            if b.len() >= 3 {
                out.push((i, j, b));
            }
        }
    }
    out
}

/// A see-through solid (floor paint, wall trim) is a thin box: its underside and its edges would only take up lightmap
/// room. They are not drawn at all.
fn unseen_paint(p: &Poly) -> bool {
    if p.flags & SURF_SOLID != 0 {
        return false;
    }
    if p.plane.n.y < -0.5 && p.corners.iter().all(|c| c.y < 3.0) {
        return true;
    }
    let (mut lo, mut hi) = (p.corners[0], p.corners[0]);
    for c in &p.corners {
        lo = lo.min(*c);
        hi = hi.max(*c);
    }
    let size = hi - lo;
    let mut spans = [size.x, size.y, size.z];
    spans.sort_by(f32::total_cmp);
    // a flat polygon has one span near zero; its next span tells how thin it is
    spans[1] < 3.0
}

/// One tile on a page.
struct Chart {
    n: V3,
    d: f32,
    u: V3,
    v: V3,
    /// Plane coordinates of the rectangle's corner.
    a0: f32,
    b0: f32,
    w: u32,
    h: u32,
    page: usize,
    x: u32,
    y: u32,
    pieces: Vec<usize>,
    albedo: [f32; 3],
}

// ---- rays -----------------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Tri {
    a: V3,
    e1: V3,
    e2: V3,
    /// The chart whose light it shows, or -1 for a hidden core.
    chart: i32,
    albedo: [f32; 3],
    core: bool,
}

struct Node {
    lo: V3,
    hi: V3,
    /// Leaf: `first .. first + count` of the order; inner: `left = first`, `right = count`.
    first: u32,
    count: u32,
    leaf: bool,
}

struct Bvh {
    tris: Vec<Tri>,
    order: Vec<u32>,
    nodes: Vec<Node>,
}

impl Bvh {
    fn new(tris: Vec<Tri>) -> Bvh {
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let bounds: Vec<(V3, V3)> = tris
            .iter()
            .map(|t| {
                let (b, c) = (t.a + t.e1, t.a + t.e2);
                (t.a.min(b).min(c), t.a.max(b).max(c))
            })
            .collect();
        let mut nodes = Vec::with_capacity(tris.len() * 2);
        Self::split(&mut nodes, &mut order, &bounds, 0, tris.len());
        Bvh { tris, order, nodes }
    }

    fn split(
        nodes: &mut Vec<Node>,
        order: &mut [u32],
        bounds: &[(V3, V3)],
        first: usize,
        count: usize,
    ) -> u32 {
        let (mut lo, mut hi) = (
            V3::new(f32::MAX, f32::MAX, f32::MAX),
            V3::new(f32::MIN, f32::MIN, f32::MIN),
        );
        for &i in &order[first..first + count] {
            lo = lo.min(bounds[i as usize].0);
            hi = hi.max(bounds[i as usize].1);
        }
        let me = nodes.len();
        nodes.push(Node {
            lo,
            hi,
            first: first as u32,
            count: count as u32,
            leaf: true,
        });
        if count <= 4 {
            return me as u32;
        }
        let size = hi - lo;
        let axis = if size.x >= size.y && size.x >= size.z {
            0
        } else if size.y >= size.z {
            1
        } else {
            2
        };
        let key = |b: &(V3, V3)| {
            let c = (b.0 + b.1).to_array();
            c[axis]
        };
        let slice = &mut order[first..first + count];
        slice.sort_by(|a, b| {
            key(&bounds[*a as usize])
                .total_cmp(&key(&bounds[*b as usize]))
                .then(a.cmp(b))
        });
        let half = count / 2;
        let left = Self::split(nodes, order, bounds, first, half);
        let right = Self::split(nodes, order, bounds, first + half, count - half);
        nodes[me].first = left;
        nodes[me].count = right;
        nodes[me].leaf = false;
        me as u32
    }

    fn hit_tri(t: &Tri, o: V3, d: V3, len: f32) -> Option<f32> {
        let h = V3::new(
            d.y * t.e2.z - d.z * t.e2.y,
            d.z * t.e2.x - d.x * t.e2.z,
            d.x * t.e2.y - d.y * t.e2.x,
        );
        let det = t.e1.dot(h);
        if det.abs() < 1e-7 {
            return None;
        }
        let s = o - t.a;
        let u = s.dot(h) / det;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = V3::new(
            s.y * t.e1.z - s.z * t.e1.y,
            s.z * t.e1.x - s.x * t.e1.z,
            s.x * t.e1.y - s.y * t.e1.x,
        );
        let v = d.dot(q) / det;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let at = t.e2.dot(q) / det;
        (at > 1e-3 && at < len).then_some(at)
    }

    fn box_near(n: &Node, o: V3, inv: V3, len: f32) -> bool {
        let (lo, hi, o, inv) = (
            n.lo.to_array(),
            n.hi.to_array(),
            o.to_array(),
            inv.to_array(),
        );
        let (mut t0, mut t1) = (0.0f32, len);
        for k in 0..3 {
            let (a, b) = ((lo[k] - o[k]) * inv[k], (hi[k] - o[k]) * inv[k]);
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
        t0 <= t1
    }

    /// Whether anything but (optionally) a core lies on the segment.
    fn blocked(&self, o: V3, d: V3, len: f32, skip_cores: bool) -> bool {
        let inv = V3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut stack = [0u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let n = &self.nodes[stack[top] as usize];
            if !Self::box_near(n, o, inv, len) {
                continue;
            }
            if n.leaf {
                for &i in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    let t = &self.tris[i as usize];
                    if !(skip_cores && t.core) && Self::hit_tri(t, o, d, len).is_some() {
                        return true;
                    }
                }
            } else if top + 2 <= stack.len() {
                stack[top] = n.first;
                stack[top + 1] = n.count;
                top += 2;
            }
        }
        false
    }

    /// The nearest triangle on the segment.
    fn nearest(&self, o: V3, d: V3, len: f32) -> Option<(usize, f32)> {
        let inv = V3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut stack = [0u32; 64];
        let mut top = 1;
        let mut best: Option<(usize, f32)> = None;
        while top > 0 {
            top -= 1;
            let n = &self.nodes[stack[top] as usize];
            let reach = best.map_or(len, |b| b.1);
            if !Self::box_near(n, o, inv, reach) {
                continue;
            }
            if n.leaf {
                for &i in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    let reach = best.map_or(len, |b| b.1);
                    if let Some(at) = Self::hit_tri(&self.tris[i as usize], o, d, reach) {
                        best = Some((i as usize, at));
                    }
                }
            } else if top + 2 <= stack.len() {
                stack[top] = n.first;
                stack[top + 1] = n.count;
                top += 2;
            }
        }
        best
    }
}

// ---- light ----------------------------------------------------------------------------------------------------

fn sun_towards(s: &WorldSettings) -> V3 {
    (-V3::from_array(s.sun_dir)).normalized().unwrap_or(V3::UP)
}

/// The light arriving straight from the lamps and the sun.
fn direct(
    input: &LightInput<'_>,
    bvh: &Bvh,
    p: V3,
    n: V3,
    skip_cores: bool,
    wrap: f32,
) -> [f32; 3] {
    let start = p + n * 1.5;
    let mut sum = [0f32; 3];
    let facing = |cos: f32| ((cos + wrap) / (1.0 + wrap)).max(0.0);
    for l in &input.lamps {
        let to = l.pos - start;
        let d2 = to.dot(to);
        if d2 >= l.radius * l.radius {
            continue;
        }
        let dist = d2.sqrt().max(1e-3);
        let cos = facing(to.dot(n) / dist);
        if cos <= 0.0 {
            continue;
        }
        let fade = (1.0 - d2 / (l.radius * l.radius)).powi(2);
        let mut seen = 0;
        for s in SAMPLES {
            let target = l.pos + V3::from_array(s) * LIGHT_SIZE;
            let t = target - start;
            let len = t.length();
            if len > 1e-3 && !bvh.blocked(start, t * (1.0 / len), len, skip_cores) {
                seen += 1;
            }
        }
        let k = cos * fade * seen as f32 / SAMPLES.len() as f32;
        for (c, s) in sum.iter_mut().enumerate() {
            *s += l.colour[c] * k;
        }
    }
    let sun = sun_towards(input.settings);
    let cos = facing(sun.dot(n));
    if input.sun > 0.0 && cos > 0.0 {
        let mut seen = 0;
        for s in SAMPLES {
            let d = (sun + V3::from_array(s) * 0.012)
                .normalized()
                .unwrap_or(sun);
            if !bvh.blocked(start, d, SUN_REACH, skip_cores) {
                seen += 1;
            }
        }
        let k = cos * input.sun * input.settings.sun_bright * seen as f32 / SAMPLES.len() as f32;
        for (c, s) in sum.iter_mut().enumerate() {
            *s += input.settings.sun_color[c] as f32 / 255.0 * k;
        }
    }
    sum
}

/// A fixed spread of directions around a normal, turned a little by the texel's number so that neighbours sample
/// differently and the blur smooths it out.
fn gather_dir(i: usize, turn: f32, u: V3, v: V3, n: V3) -> V3 {
    const GOLDEN: f32 = 2.399_963;
    let f = (i as f32 + 0.5) / GATHER as f32;
    let (r, h) = (f.sqrt(), (1.0 - f).sqrt());
    let a = i as f32 * GOLDEN + turn;
    u * (r * a.cos()) + v * (r * a.sin()) + n * h
}

/// The bake.
/// A tile: the plane's rounded normal and distance, and its column and row on that plane.
type TileKey = (i32, i32, i32, i32, i32, i32);
/// A polygon's part of a tile: its index and corners.
type TilePart = (usize, Vec<V3>);

pub fn bake(level: &Level, input: &LightInput<'_>) -> Baked {
    // cut the flat, drawn polygons into tiles; a model's triangles (with normals of their own) stay vertex-lit
    let step = input.density * TILE_TEXELS;
    let mut by_tile: BTreeMap<TileKey, Vec<TilePart>> = BTreeMap::new();
    for (i, p) in level.polys.iter().enumerate() {
        if p.corner_normals.is_some() || p.corners.len() < 3 || unseen_paint(p) {
            continue;
        }
        let n = p.plane.n;
        let key = |v: f32, s: f32| (v * s).round() as i32;
        for (col, row, corners) in tiles(p, step) {
            by_tile
                .entry((
                    key(n.x, 500.0),
                    key(n.y, 500.0),
                    key(n.z, 500.0),
                    key(p.plane.d, 2.0),
                    col,
                    row,
                ))
                .or_default()
                .push((i, corners));
        }
    }
    let mut pieces = Vec::new();
    let mut charts: Vec<Chart> = Vec::with_capacity(by_tile.len());
    for group in by_tile.values() {
        let first = &level.polys[group[0].0];
        let n = first.plane.n;
        let (u, v) = axes(n);
        let (mut a0, mut a1, mut b0, mut b1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        let mut members = Vec::with_capacity(group.len());
        for (i, corners) in group {
            for c in corners {
                a0 = a0.min(c.dot(u));
                a1 = a1.max(c.dot(u));
                b0 = b0.min(c.dot(v));
                b1 = b1.max(c.dot(v));
            }
            let mut poly = level.polys[*i].clone();
            poly.corners = corners.clone();
            members.push(pieces.len());
            pieces.push(LitPiece {
                poly,
                page: 0,
                lm_uv: Vec::new(),
            });
        }
        let texels = |span: f32| ((span / input.density).ceil() as u32 + 3).clamp(4, PAGE - 2);
        charts.push(Chart {
            n,
            d: first.plane.d,
            u,
            v,
            a0,
            b0,
            w: texels(a1 - a0),
            h: texels(b1 - b0),
            page: 0,
            x: 0,
            y: 0,
            pieces: members,
            albedo: (input.albedo)(first.material),
        });
    }
    // shelves on pages, the tallest first
    let mut order: Vec<usize> = (0..charts.len()).collect();
    order.sort_by_key(|i| {
        (
            std::cmp::Reverse(charts[*i].h),
            std::cmp::Reverse(charts[*i].w),
            *i,
        )
    });
    let (mut page, mut x, mut y, mut row) = (0usize, 1u32, 1u32, 0u32);
    for &i in &order {
        let (w, h) = (charts[i].w, charts[i].h);
        if x + w + 1 > PAGE {
            x = 1;
            y += row + 1;
            row = 0;
        }
        if y + h + 1 > PAGE {
            page += 1;
            x = 1;
            y = 1;
            row = 0;
        }
        charts[i].page = page;
        charts[i].x = x;
        charts[i].y = y;
        x += w + 1;
        row = row.max(h);
    }
    let page_count = if charts.is_empty() { 0 } else { page + 1 };
    // lightmap coordinates
    for ch in &charts {
        for &m in &ch.pieces {
            let piece = &mut pieces[m];
            piece.page = ch.page;
            piece.lm_uv = piece
                .poly
                .corners
                .iter()
                .map(|c| {
                    let tx = ch.x as f32 + 1.0 + (c.dot(ch.u) - ch.a0) / input.density;
                    let ty = ch.y as f32 + 1.0 + (c.dot(ch.v) - ch.b0) / input.density;
                    [(tx + 0.5) / PAGE as f32, (ty + 0.5) / PAGE as f32]
                })
                .collect();
        }
    }
    // what rays can hit
    let mut tris = Vec::new();
    let fan = |corners: &[V3], chart: i32, albedo: [f32; 3], core: bool, out: &mut Vec<Tri>| {
        for k in 1..corners.len().saturating_sub(1) {
            out.push(Tri {
                a: corners[0],
                e1: corners[k] - corners[0],
                e2: corners[k + 1] - corners[0],
                chart,
                albedo,
                core,
            });
        }
    };
    for (ci, ch) in charts.iter().enumerate() {
        for &m in &ch.pieces {
            if pieces[m].poly.flags & SURF_SOLID != 0 {
                fan(
                    &pieces[m].poly.corners,
                    ci as i32,
                    ch.albedo,
                    false,
                    &mut tris,
                );
            }
        }
    }
    for p in level.hidden.iter().filter(|p| !(input.skin)(p.material)) {
        fan(&p.corners, -1, [0.45; 3], true, &mut tris);
    }
    let bvh = Bvh::new(tris);
    // first the straight light on every chart, then the bounce from those
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(12);
    let shared = |f: &(dyn Fn(usize) -> Vec<[f32; 3]> + Sync)| -> Vec<Vec<[f32; 3]>> {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let mut done: Vec<(usize, Vec<[f32; 3]>)> = Vec::with_capacity(charts.len());
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..threads {
                handles.push(scope.spawn(|| {
                    let mut part = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= charts.len() {
                            break;
                        }
                        part.push((i, f(i)));
                    }
                    part
                }));
            }
            for h in handles {
                if let Ok(part) = h.join() {
                    done.extend(part);
                }
            }
        });
        done.sort_by_key(|d| d.0);
        done.into_iter().map(|d| d.1).collect()
    };
    let point = |ch: &Chart, tx: u32, ty: u32| {
        let a = ch.a0 + (tx as f32 - 1.0) * input.density;
        let b = ch.b0 + (ty as f32 - 1.0) * input.density;
        ch.u * a + ch.v * b + ch.n * ch.d
    };
    let lit = shared(&|i| {
        let ch = &charts[i];
        let mut out = vec![[0f32; 3]; (ch.w * ch.h) as usize];
        for ty in 0..ch.h {
            for tx in 0..ch.w {
                out[(ty * ch.w + tx) as usize] =
                    direct(input, &bvh, point(ch, tx, ty), ch.n, false, 0.0);
            }
        }
        out
    });
    let ambient = [0, 1, 2].map(|c| input.settings.ambient[c] as f32 / 255.0 * input.ambient);
    let sky = input
        .settings
        .sun_color
        .map(|c| c as f32 / 255.0 * input.sun * 0.35);
    let around = shared(&|i| {
        let ch = &charts[i];
        let mut out = vec![[0f32; 3]; (ch.w * ch.h) as usize];
        for ty in 0..ch.h {
            for tx in 0..ch.w {
                let p = point(ch, tx, ty) + ch.n * 1.5;
                let turn = ((tx.wrapping_mul(73) ^ ty.wrapping_mul(179)) % 97) as f32 * 0.0648;
                let mut sum = [0f32; 3];
                let mut close = 0;
                for r in 0..GATHER {
                    let d = gather_dir(r, turn, ch.u, ch.v, ch.n);
                    match bvh.nearest(p, d, SUN_REACH) {
                        None => {
                            for c in 0..3 {
                                sum[c] += sky[c];
                            }
                        }
                        Some((t, at)) => {
                            if at < CONTACT {
                                close += 1;
                            }
                            let hit = &bvh.tris[t];
                            let light = if hit.chart >= 0 {
                                let o = &charts[hit.chart as usize];
                                let q = p + d * at;
                                let qx = ((q.dot(o.u) - o.a0) / input.density + 1.0).round();
                                let qy = ((q.dot(o.v) - o.b0) / input.density + 1.0).round();
                                let (qx, qy) = (
                                    qx.clamp(0.0, o.w as f32 - 1.0) as u32,
                                    qy.clamp(0.0, o.h as f32 - 1.0) as u32,
                                );
                                lit[hit.chart as usize][(qy * o.w + qx) as usize]
                            } else {
                                [0.12; 3]
                            };
                            for c in 0..3 {
                                sum[c] += light[c] * hit.albedo[c];
                            }
                        }
                    }
                }
                let open = 1.0 - 0.75 * close as f32 / GATHER as f32;
                for c in 0..3 {
                    out[(ty * ch.w + tx) as usize][c] =
                        sum[c] / GATHER as f32 * input.bounce + ambient[c] * open;
                }
            }
        }
        out
    });
    // pages: straight light plus the blurred surroundings
    let mut images = vec![vec![[0u8; 3]; (PAGE * PAGE) as usize]; page_count];
    for (i, ch) in charts.iter().enumerate() {
        let (w, h) = (ch.w as i32, ch.h as i32);
        for ty in 0..h {
            for tx in 0..w {
                let mut blur = [0f32; 3];
                let mut n = 0f32;
                for dy in -2..=2i32 {
                    for dx in -2..=2i32 {
                        let (ax, ay) = (tx + dx, ty + dy);
                        if ax >= 0 && ay >= 0 && ax < w && ay < h {
                            let s = around[i][(ay * w + ax) as usize];
                            for c in 0..3 {
                                blur[c] += s[c];
                            }
                            n += 1.0;
                        }
                    }
                }
                let d = lit[i][(ty * w + tx) as usize];
                let at = ((ch.y + ty as u32) * PAGE + ch.x + tx as u32) as usize;
                for c in 0..3 {
                    images[ch.page][at][c] = texel(d[c] + blur[c] / n);
                }
            }
        }
    }
    // the models' corners and the grid
    let vertex_colours: Vec<Vec<u32>> = level
        .polys
        .iter()
        .map(|p| match &p.corner_normals {
            None => Vec::new(),
            Some(ns) => p
                .corners
                .iter()
                .zip(ns)
                .map(|(c, n)| {
                    // paint looks like part of its wall when it has the wall's own light (DC-DEFAULT: MAP-110)
                    if (input.paint)(p.material) {
                        if let Some((t, _)) = bvh.nearest(*c + *n * 0.25, *n * -1.0, 12.0) {
                            let chart = bvh.tris[t].chart;
                            if chart >= 0 {
                                let o = &charts[chart as usize];
                                let q = *c;
                                let qx = ((q.dot(o.u) - o.a0) / input.density + 1.0).round();
                                let qy = ((q.dot(o.v) - o.b0) / input.density + 1.0).round();
                                let at = (qy.clamp(0.0, o.h as f32 - 1.0) as u32 * o.w
                                    + qx.clamp(0.0, o.w as f32 - 1.0) as u32)
                                    as usize;
                                let (a, b) = (lit[chart as usize][at], around[chart as usize][at]);
                                let channel = |k: usize| texel(a[k] + b[k]) as u32;
                                return 0xFF00_0000
                                    | (channel(0) << 16)
                                    | (channel(1) << 8)
                                    | channel(2);
                            }
                        }
                    }
                    let d = direct(input, &bvh, *c, *n, true, 0.35);
                    let hemi = 0.72 + 0.28 * n.y;
                    let channel = |k: usize| {
                        // a model's picture has its contact shading baked in, so the ambient light stays whole
                        (((d[k] + ambient[k] * hemi * MODEL_FILL + sky[k] * 0.1) * 128.0)
                            .round()
                            .clamp(0.0, 255.0)) as u32
                    };
                    0xFF00_0000 | (channel(0) << 16) | (channel(1) << 8) | channel(2)
                })
                .collect(),
        })
        .collect();
    let paint_off_wall: Vec<usize> = level
        .polys
        .iter()
        .enumerate()
        .filter(|(_, p)| (input.paint)(p.material))
        .filter(|(_, p)| {
            p.corner_normals.as_ref().is_some_and(|ns| {
                let middle = p
                    .corners
                    .iter()
                    .fold(V3::ZERO, |sum, c| sum + *c * (1.0 / p.corners.len() as f32));
                p.corners.iter().zip(ns).any(|(c, n)| {
                    // a little inside the corner: a ray exactly on the seam of two triangles can slip through
                    let q = *c + (middle - *c).normalized().unwrap_or(V3::ZERO) * 2.0;
                    let behind = bvh
                        .nearest(q + *n * 0.25, *n * -1.0, 12.0)
                        .is_some_and(|(t, _)| bvh.tris[t].chart >= 0);
                    !behind || bvh.nearest(q, *n, 1.0).is_some()
                })
            })
        })
        .map(|(i, _)| i)
        .collect();
    let (grid_dims, grid_origin, grid_cell, grid_rgb) = light_grid(level, input, &bvh, ambient);
    Baked {
        paint_off_wall,
        pieces,
        pages: images.iter().map(|m| pack(m)).collect(),
        vertex_colours,
        grid_dims,
        grid_origin,
        grid_cell,
        grid_rgb,
    }
}

/// A light value as a lightmap byte: the game multiplies twice the stored value into the texture.
fn texel(v: f32) -> u8 {
    (v * 128.0).round().clamp(0.0, 255.0) as u8
}

/// The light on a player standing at each cell of a grid over the map.
fn light_grid(
    level: &Level,
    input: &LightInput<'_>,
    bvh: &Bvh,
    ambient: [f32; 3],
) -> ([u32; 3], V3, V3, Vec<u8>) {
    let (lo, hi) = (level.extent_min, level.extent_max);
    let span = hi - lo;
    let cell = 128.0f32;
    let count = |s: f32| ((s / cell).ceil() as u32 + 1).clamp(1, 96);
    let dims = [count(span.x), count(span.y).min(12), count(span.z)];
    let size = V3::new(cell, (span.y / (dims[1].max(2) - 1) as f32).max(1.0), cell);
    let mut rgb = Vec::with_capacity((dims[0] * dims[1] * dims[2] * 3) as usize);
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let p = lo + V3::new(x as f32 * size.x, y as f32 * size.y, z as f32 * size.z);
                let d = direct(input, bvh, p, V3::UP, false, 1.0);
                for c in 0..3 {
                    // never so dark that a player cannot be seen: this is what they are shot at by
                    rgb.push(
                        ((d[c] * 0.7 + ambient[c] + 0.14) * 255.0)
                            .round()
                            .clamp(0.0, 255.0) as u8,
                    );
                }
            }
        }
    }
    (dims, lo, size, rgb)
}

/// The run-length packing of a lightmap (RENDER_2009.md 6.1): a control byte `c`, `run = (c & 0x7f) + 1`; with `c & 0x80`
/// one texel repeated, else `run` literal texels.
pub fn pack(image: &[[u8; 3]]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < image.len() {
        let mut same = 1;
        while i + same < image.len() && same < 128 && image[i + same] == image[i] {
            same += 1;
        }
        if same >= 2 {
            out.push(0x80 | (same as u8 - 1));
            out.extend_from_slice(&image[i]);
            i += same;
            continue;
        }
        let start = i;
        i += 1;
        while i < image.len()
            && i - start < 128
            && !(i + 1 < image.len() && image[i] == image[i + 1])
        {
            i += 1;
        }
        out.push((i - start - 1) as u8);
        for t in &image[start..i] {
            out.extend_from_slice(t);
        }
    }
    out
}

/// The reader's unpacking, for the tests.
#[cfg(test)]
pub(crate) fn tests_unpack(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut p = data;
    while let Some((&c, rest)) = p.split_first() {
        let run = (c & 0x7f) as usize + 1;
        if c & 0x80 != 0 {
            for _ in 0..run {
                out.extend_from_slice(&rest[..3]);
            }
            p = &rest[3..];
        } else {
            out.extend_from_slice(&rest[..3 * run]);
            p = &rest[3 * run..];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Surface;

    fn square(y: f32, half: f32, up: bool, solid: bool) -> Poly {
        let s = Surface {
            flags: if solid { SURF_SOLID } else { 0 },
            ..Surface::default()
        };
        Poly::new(
            vec![
                V3::new(-half, y, -half),
                V3::new(half, y, -half),
                V3::new(half, y, half),
                V3::new(-half, y, half),
            ],
            if up { V3::UP } else { V3::UP * -1.0 },
            s,
            None,
        )
        .unwrap()
    }

    fn settings() -> WorldSettings {
        WorldSettings {
            ambient: [0, 0, 0],
            sun_bright: 0.0,
            ..WorldSettings::default()
        }
    }

    fn unpack(data: &[u8]) -> Vec<u8> {
        crate::lightmap::tests_unpack(data)
    }

    #[test]
    fn packing_reads_back_through_the_renderers_rule() {
        let mut image = vec![[9u8, 8, 7]; 300];
        for (i, t) in image.iter_mut().enumerate().take(150).skip(100) {
            *t = [i as u8, 1, 2];
        }
        let bytes = unpack(&pack(&image));
        let flat: Vec<u8> = image.iter().flatten().copied().collect();
        assert_eq!(bytes, flat);
    }

    #[test]
    fn a_skin_stops_shots_but_leaves_the_light_alone() {
        let s = settings();
        let albedo = |_: usize| [0.5f32; 3];
        let run = |is_skin: bool| {
            let mut level = Level::default();
            level.polys.push(square(0.0, 300.0, true, true));
            let mut over = square(100.0, 300.0, true, true);
            over.material = 1;
            level.hidden.push(over);
            level.update_extent();
            let skin = move |m: usize| is_skin && m == 1;
            let input = LightInput {
                settings: &s,
                density: 16.0,
                ambient: 0.0,
                sun: 0.0,
                bounce: 0.0,
                lamps: vec![Lamp {
                    pos: V3::new(0.0, 200.0, 0.0),
                    colour: [1.0; 3],
                    radius: 600.0,
                }],
                albedo: &albedo,
                paint: &|_| false,
                skin: &skin,
            };
            let b = bake(&level, &input);
            let piece = b.pieces.first().unwrap();
            let data = unpack(&b.pages[piece.page]);
            let mut lightest = 0u8;
            for uv in &piece.lm_uv {
                let x = (uv[0] * PAGE as f32) as usize;
                let y = (uv[1] * PAGE as f32) as usize;
                lightest = lightest.max(data[(y * PAGE as usize + x) * 3]);
            }
            lightest
        };
        assert!(
            run(true) > run(false),
            "under a skin the lamp reaches the floor: {} against {}",
            run(true),
            run(false)
        );
    }

    #[test]
    fn paint_past_the_edge_of_its_wall_is_reported() {
        let s = settings();
        let albedo = |_: usize| [0.5f32; 3];
        let run = |shift: f32| {
            let mut level = Level::default();
            level.polys.push(square(0.0, 300.0, true, true));
            let mut sheet = square(1.0, 40.0, true, true);
            for c in &mut sheet.corners {
                c.x += shift;
            }
            sheet.material = 1;
            sheet.corner_normals = Some(vec![sheet.plane.n; sheet.corners.len()]);
            level.polys.push(sheet);
            level.update_extent();
            let paint = |m: usize| m == 1;
            let input = LightInput {
                settings: &s,
                density: 16.0,
                ambient: 0.0,
                sun: 0.0,
                bounce: 0.0,
                lamps: Vec::new(),
                albedo: &albedo,
                paint: &paint,
                skin: &|_| false,
            };
            bake(&level, &input).paint_off_wall
        };
        assert!(run(0.0).is_empty(), "wholly on the floor");
        assert_eq!(run(290.0), vec![1], "past the floor's edge");
    }

    #[test]
    fn paint_takes_the_light_of_the_surface_behind_it() {
        let s = WorldSettings {
            ambient: [150, 150, 150],
            sun_bright: 0.0,
            ..WorldSettings::default()
        };
        let albedo = |_: usize| [0.5f32; 3];
        let run = |is_paint: bool| {
            let mut level = Level::default();
            level.polys.push(square(0.0, 300.0, true, true));
            let mut sheet = square(1.0, 40.0, true, true);
            sheet.material = 1;
            sheet.corner_normals = Some(vec![sheet.plane.n; sheet.corners.len()]);
            level.polys.push(sheet);
            level.update_extent();
            let paint = move |m: usize| is_paint && m == 1;
            let input = LightInput {
                settings: &s,
                density: 16.0,
                ambient: 1.0,
                sun: 0.0,
                bounce: 0.0,
                lamps: Vec::new(),
                albedo: &albedo,
                paint: &paint,
                skin: &|_| false,
            };
            bake(&level, &input).vertex_colours[1][0] >> 16 & 255
        };
        let (on_wall, as_model) = (run(true), run(false));
        assert!(on_wall > 0, "it has light");
        assert!(
            on_wall < as_model,
            "a model's lifted ambient is not for paint: {on_wall} against {as_model}"
        );
    }

    #[test]
    fn a_lamp_lights_the_floor_and_a_roof_between_them_shades_it() {
        let s = settings();
        let albedo = |_: usize| [0.5f32; 3];
        let run = |roof: bool| {
            let mut level = Level::default();
            level.polys.push(square(0.0, 300.0, true, true));
            if roof {
                level.polys.push(square(100.0, 200.0, true, true));
            }
            level.update_extent();
            let input = LightInput {
                settings: &s,
                density: 16.0,
                ambient: 0.0,
                sun: 0.0,
                bounce: 0.0,
                lamps: vec![Lamp {
                    pos: V3::new(0.0, 200.0, 0.0),
                    colour: [1.0; 3],
                    radius: 600.0,
                }],
                albedo: &albedo,
                paint: &|_| false,
                skin: &|_| false,
            };
            let b = bake(&level, &input);
            assert!(!b.pages.is_empty());
            let mut lightest = 0u8;
            let piece = b
                .pieces
                .iter()
                .find(|p| p.poly.corners[0].y == 0.0)
                .unwrap();
            let data = unpack(&b.pages[piece.page]);
            // the texel under the lamp's foot: near a corner of the tile that meets the middle of the floor
            for (c, uv) in piece.poly.corners.iter().zip(&piece.lm_uv) {
                if c.x.abs() + c.z.abs() < 1.0 {
                    let x = (uv[0] * PAGE as f32) as usize;
                    let y = (uv[1] * PAGE as f32) as usize;
                    lightest = data[(y * PAGE as usize + x) * 3];
                }
            }
            lightest
        };
        let (open, shaded) = (run(false), run(true));
        assert!(open > 60, "lit: {open}");
        assert!(shaded < 12, "behind the roof: {shaded}");
    }

    #[test]
    fn a_long_wall_is_cut_into_tiles_that_fit_a_page() {
        let s = settings();
        let albedo = |_: usize| [0.5f32; 3];
        let mut level = Level::default();
        level.polys.push(square(0.0, 2000.0, true, true));
        level.update_extent();
        let input = LightInput {
            settings: &s,
            density: 8.0,
            ambient: 0.5,
            sun: 0.0,
            bounce: 0.0,
            lamps: Vec::new(),
            albedo: &albedo,
            paint: &|_| false,
            skin: &|_| false,
        };
        let b = bake(&level, &input);
        assert!(b.pieces.len() > 1);
        for p in &b.pieces {
            for uv in &p.lm_uv {
                assert!(
                    uv[0] > 0.0 && uv[0] < 1.0 && uv[1] > 0.0 && uv[1] < 1.0,
                    "{uv:?}"
                );
            }
        }
    }
}
