//! The fill floor of a piece (MAP-120): where a world has no floor in the part of its region outside its play
//! area, the composer lays the map's own ground there, so a player who walks out of a world (or into its region
//! from the map's ground) stands on something instead of falling out of the world.
//!
//! A grid of [`super::FILL_CELL`] units covers the region; a cell is sampled at five points, and a point is covered
//! when an upward-facing solid polygon of the world's level (not a sky face) lies under it, at most [`COVER_UP`]
//! over the fill height (a floor a player there stands on; a hill or a roof with nothing under it covers nothing).
//! A cell whose points are not all covered is filled (in a closed piece only outside the play area, which its
//! blockers guard; in an open piece everywhere, as players can reach it); the cells of a row are joined into strips
//! and the footprint of every cut at the fill's height is taken out of them exactly. The floor lies [`SINK`] units under the fill height, so a world floor at that height is drawn
//! over it.
//!
//! Where the region cuts off a floor higher than that, its edge gets a cap: an earth wall from the edge down to the
//! fill floor, facing out of the region (nothing of it inside a cut), so a hill ends in a bank, not an open shell a
//! player walks into. A cut's own faces get none: a doorway opens into the world's floor, often under a floor that
//! runs on over it. All of this is worked out from the player's own ROM at
//! load time, like the rest of the piece; the map carries only the height and the look.
// DC-DEFAULT: MAP-120

use super::clip::{self, Aabb, V};
use super::tree::{SURF_SKY, SURF_SOLID};
use super::Fill;
use crate::synth::{SynthBlock, SynthSection};
use crate::{BspModel, RENDER_VERTEX_BYTES};

/// An x/z rectangle: `[x0, z0, x1, z1]`.
pub type Rect = [f32; 4];

/// The fill floor lies this far under its height (a world floor at the same height is drawn over it).
pub const SINK: f32 = 2.0;
/// A polygon whose normal points up at least this much is a floor.
const FLOOR_UP: f32 = 0.2;
/// A floor covers a sample when it lies at most this far over the fill height there (a player's step).
pub const COVER_UP: f32 = 30.0;
/// A floor wholly higher than this over the fill height gets no cap: it is a roof or the lid of a world's sky box,
/// not ground, and a bank down from it would stand as a wall of earth over everything round the region.
pub const CAP_TOP: f32 = 800.0;
/// The light of a cap against the fill's (a wall gets less of the sky than a floor).
const CAP_SHADE: f32 = 0.8;
/// The five sample points of a cell, as fractions of the cell.
/// The most fill samples a region gets (a 65,536-unit region at the 128-unit cell is 1.3 million).
const MAX_FILL_SAMPLES: usize = 1 << 23;
const SAMPLES: [(f32, f32); 5] = [
    (0.5, 0.5),
    (0.15, 0.15),
    (0.85, 0.15),
    (0.15, 0.85),
    (0.85, 0.85),
];

/// Whether (x, z) lies in the x/z outline of a convex polygon (either winding).
fn inside_xz(c: &[V], x: f32, z: f32) -> bool {
    let n = c.len();
    let (mut pos, mut neg) = (false, false);
    for k in 0..n {
        let a = c[k];
        let b = c[(k + 1) % n];
        let cr = (b[0] - a[0]) * (z - a[2]) - (b[2] - a[2]) * (x - a[0]);
        if cr > 1.0e-3 {
            pos = true;
        } else if cr < -1.0e-3 {
            neg = true;
        }
        if pos && neg {
            return false;
        }
    }
    true
}

/// The strips of the region to fill: the world's models that stand for ground, moved by `t` (with whether a face
/// must be solid to count: the level's must, a world model's need not); `play` is the play area left alone (a closed
/// piece's: its blockers keep players in it; `None` for an open piece, whose bare columns inside the play area are
/// filled too), `y` the fill height.
pub fn rects(
    models: &[(&BspModel<'_>, bool)],
    t: V,
    region: &Aabb,
    play: Option<&Aabb>,
    cuts: &[Aabb],
    y: f32,
    cell: f32,
) -> Vec<Rect> {
    let nx = (((region.max[0] - region.min[0]) / cell).ceil() as usize).max(1);
    let nz = (((region.max[2] - region.min[2]) / cell).ceil() as usize).max(1);
    // a record's region is bounded before it gets here (MAX_REGION_EXTENT); this guards the allocation itself
    if nx.saturating_mul(nz).saturating_mul(SAMPLES.len()) > MAX_FILL_SAMPLES {
        return Vec::new();
    }
    let mut covered = vec![false; nx * nz * SAMPLES.len()];
    let cell_of = |v: f32, lo: f32, n: usize| -> usize {
        (((v - lo) / cell).floor().max(0.0) as usize).min(n - 1)
    };
    for &(level, solid_only) in models {
        for p in &level.polygons {
            let Some(s) = level.surfaces.get(p.surface as usize) else {
                continue;
            };
            if (solid_only && s.flags & SURF_SOLID == 0) || s.flags & SURF_SKY != 0 {
                continue;
            }
            let Some(pl) = level.planes.get(p.plane as usize) else {
                continue;
            };
            if pl.normal[1] < FLOOR_UP {
                continue;
            }
            let corners: Vec<V> = level
                .polygon_points(p)
                .iter()
                .filter_map(|&i| level.points.get(i as usize).map(|&q| clip::add(q, t)))
                .collect();
            if corners.len() < 3 {
                continue;
            }
            let Some(b) = Aabb::of_points(corners.iter()) else {
                continue;
            };
            if b.max[1] < region.min[1]
                || b.min[1] > region.max[1]
                || b.min[1] > y + COVER_UP
                || b.max[0] < region.min[0]
                || b.min[0] > region.max[0]
                || b.max[2] < region.min[2]
                || b.min[2] > region.max[2]
            {
                continue;
            }
            let n = pl.normal;
            let c0 = corners[0];
            // the face's height over (x, z), on its plane
            let height = |x: f32, z: f32| c0[1] - (n[0] * (x - c0[0]) + n[2] * (z - c0[2])) / n[1];
            let (i0, i1) = (
                cell_of(b.min[0], region.min[0], nx),
                cell_of(b.max[0], region.min[0], nx),
            );
            let (j0, j1) = (
                cell_of(b.min[2], region.min[2], nz),
                cell_of(b.max[2], region.min[2], nz),
            );
            for j in j0..=j1 {
                for i in i0..=i1 {
                    for (k, (fx, fz)) in SAMPLES.iter().enumerate() {
                        let idx = (j * nx + i) * SAMPLES.len() + k;
                        if covered[idx] {
                            continue;
                        }
                        let x = region.min[0] + (i as f32 + fx) * cell;
                        let z = region.min[2] + (j as f32 + fz) * cell;
                        if inside_xz(&corners, x, z) && height(x, z) <= y + COVER_UP {
                            covered[idx] = true;
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for j in 0..nz {
        let z0 = region.min[2] + j as f32 * cell;
        let z1 = (z0 + cell).min(region.max[2]);
        let cz = (z0 + z1) * 0.5;
        let mut run: Option<usize> = None;
        for i in 0..=nx {
            let here = i < nx && {
                let x0 = region.min[0] + i as f32 * cell;
                let cx = (x0 + (x0 + cell).min(region.max[0])) * 0.5;
                let in_play = play.is_some_and(|play| {
                    cx >= play.min[0] && cx <= play.max[0] && cz >= play.min[2] && cz <= play.max[2]
                });
                let bare = (0..SAMPLES.len()).any(|k| !covered[(j * nx + i) * SAMPLES.len() + k]);
                !in_play && bare
            };
            match (here, run) {
                (true, None) => run = Some(i),
                (false, Some(i0)) => {
                    let x0 = region.min[0] + i0 as f32 * cell;
                    let x1 = (region.min[0] + i as f32 * cell).min(region.max[0]);
                    out.push([x0, z0, x1, z1]);
                    run = None;
                }
                _ => {}
            }
        }
    }
    // the cuts at the fill's height take their footprint out exactly (a whole cell left bare beside a doorway is a
    // hole a player falls through)
    for c in cuts
        .iter()
        .filter(|c| y >= c.min[1] - 64.0 && y <= c.max[1])
    {
        out = out.into_iter().flat_map(|r| minus_rect(r, c)).collect();
    }
    out
}

/// A strip less a box's x/z footprint: up to four strips.
fn minus_rect(r: Rect, c: &Aabb) -> Vec<Rect> {
    let (cx0, cz0, cx1, cz1) = (c.min[0], c.min[2], c.max[0], c.max[2]);
    if cx1 <= r[0] || cx0 >= r[2] || cz1 <= r[1] || cz0 >= r[3] {
        return vec![r];
    }
    let mut out = Vec::new();
    if cz0 > r[1] {
        out.push([r[0], r[1], r[2], cz0]);
    }
    if cz1 < r[3] {
        out.push([r[0], cz1, r[2], r[3]]);
    }
    let (zlo, zhi) = (cz0.max(r[1]), cz1.min(r[3]));
    if cx0 > r[0] {
        out.push([r[0], zlo, cx0, zhi]);
    }
    if cx1 < r[2] {
        out.push([cx1, zlo, r[2], zhi]);
    }
    out
}

/// The corners of a strip at fill height `y`, counter-clockwise seen from above (the face points up).
pub fn corners(r: &Rect, y: f32) -> Vec<V> {
    let y = y - SINK;
    vec![
        [r[0], y, r[1]],
        [r[0], y, r[3]],
        [r[2], y, r[3]],
        [r[2], y, r[1]],
    ]
}

/// A cap: a wall under the cut-off edge of a floor, down to the fill floor; `normal` faces the side cut away.
#[derive(Debug, Clone, PartialEq)]
pub struct Cap {
    pub corners: Vec<V>,
    pub normal: V,
}

/// Where a polygon edge `a`-`b` lies on a side of the region: the edge and the normal of the face cut away (out of the
/// region); `None` for an edge inside. A cut's own faces get no cap: a doorway opens into the world's floor, often
/// under a floor that runs on over it, and a corridor brings its own walls.
fn cut_side(a: V, b: V, region: &Aabb) -> Option<(V, V, V)> {
    const E: f32 = 0.01;
    let on = |axis: usize, v: f32| (a[axis] - v).abs() < E && (b[axis] - v).abs() < E;
    for axis in [0usize, 2] {
        let mut n = [0.0f32; 3];
        if on(axis, region.min[axis]) {
            n[axis] = -1.0;
            return Some((a, b, n));
        }
        if on(axis, region.max[axis]) {
            n[axis] = 1.0;
            return Some((a, b, n));
        }
    }
    None
}

/// The caps of a piece's level moved by `t`: each edge of an upward solid face (sky faces left out), kept to the
/// region outside the cuts, that lies on a side of the region and somewhere rises more than [`COVER_UP`] over the
/// fill height `y` gets a wall from the edge down to the fill floor (nothing of it inside a cut).
pub fn caps(level: &BspModel<'_>, t: V, region: &Aabb, cuts: &[Aabb], y: f32) -> Vec<Cap> {
    let bottom = y - SINK;
    let mut out = Vec::new();
    for p in &level.polygons {
        let Some(s) = level.surfaces.get(p.surface as usize) else {
            continue;
        };
        if s.flags & SURF_SOLID == 0 || s.flags & SURF_SKY != 0 {
            continue;
        }
        let Some(pl) = level.planes.get(p.plane as usize) else {
            continue;
        };
        if pl.normal[1] < FLOOR_UP {
            continue;
        }
        let corners: Vec<V> = level
            .polygon_points(p)
            .iter()
            .filter_map(|&i| level.points.get(i as usize).map(|&q| clip::add(q, t)))
            .collect();
        if corners.len() < 3 {
            continue;
        }
        let Some(b) = Aabb::of_points(corners.iter()) else {
            continue;
        };
        // a face wholly inside the region has no edge on its sides; one wholly low needs no cap, one wholly high
        // is a roof
        if b.max[1] <= y + COVER_UP
            || b.min[1] > y + CAP_TOP
            || !region.touches(&b)
            || (region.min[0] < b.min[0]
                && b.max[0] < region.max[0]
                && region.min[2] < b.min[2]
                && b.max[2] < region.max[2])
        {
            continue;
        }
        for part in clip::keep(&corners, region, cuts) {
            let n = part.len();
            for k in 0..n {
                let (a, c) = (part[k], part[(k + 1) % n]);
                if a[1].max(c[1]) <= y + COVER_UP {
                    continue;
                }
                let Some((a, c, normal)) = cut_side(a, c, region) else {
                    continue;
                };
                let mut q: Vec<V> = vec![a, c];
                for e in [c, a] {
                    if e[1] > bottom + 0.01 {
                        q.push([e[0], bottom, e[2]]);
                    }
                }
                if q.len() < 3 {
                    continue;
                }
                // wind it to face `normal` (front: cross(b - a, c - a) along the normal)
                let f = clip::cross(clip::sub(q[1], q[0]), clip::sub(q[2], q[0]));
                if clip::dot(f, normal) < 0.0 {
                    q.reverse();
                }
                // nothing of a cap stands in a cut (a doorway under a floor that runs on over it stays open)
                let mut parts = vec![q];
                for cut in cuts {
                    parts = parts
                        .into_iter()
                        .flat_map(|p| clip::minus_box(&p, cut))
                        .collect();
                }
                out.extend(
                    parts
                        .into_iter()
                        .filter(|p| p.len() >= 3)
                        .map(|corners| Cap { corners, normal }),
                );
            }
        }
    }
    out
}

fn vertex(p: V, uv: [f32; 2], argb: u32, n: V) -> [u8; RENDER_VERTEX_BYTES] {
    let mut v = [0u8; RENDER_VERTEX_BYTES];
    let mut put = |o: usize, x: f32| v[o..o + 4].copy_from_slice(&x.to_le_bytes());
    for (k, &x) in p.iter().enumerate() {
        put(4 * k, x);
    }
    put(12, uv[0]);
    put(16, uv[1]);
    // the normal, then two unit vectors across it (along x and z for a floor; along the wall and down for a cap)
    let (u, w) = if n[1].abs() > 0.5 {
        ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0])
    } else {
        ([n[2], 0.0, -n[0]], [0.0, -1.0, 0.0])
    };
    for (k, x) in n.iter().chain(u.iter()).chain(w.iter()).enumerate() {
        put(32 + 4 * k, *x);
    }
    v[28..32].copy_from_slice(&argb.to_le_bytes());
    v
}

/// The render block of the strips and caps: one kind-1 section of the fill texture, the fill colour on every
/// corner (a little darker on a cap), the texture tiled by world position (across a floor; along and down a cap).
// SPEC: FMT_DAT85.md §10.2 (block, section, 68-byte vertex; OBSERVED field split, Appendix Z D9)
pub fn block(rects: &[Rect], caps: &[Cap], f: &Fill) -> SynthBlock {
    let ch = |c: f32| (c.clamp(0.0, 255.0) + 0.5) as u32;
    let argb = |k: f32| {
        0xFF00_0000 | ch(f.color[0] * k) << 16 | ch(f.color[1] * k) << 8 | ch(f.color[2] * k)
    };
    let mut b = SynthBlock::default();
    let mut bounds: Option<Aabb> = None;
    let mut grow = |p: V| match &mut bounds {
        Some(bb) => bb.grow(p),
        None => bounds = Some(Aabb { min: p, max: p }),
    };
    for r in rects {
        let base = b.vertices.len() as u32;
        for p in corners(r, f.y) {
            b.vertices.push(vertex(
                p,
                [p[0] / f.tile, p[2] / f.tile],
                argb(1.0),
                [0.0, 1.0, 0.0],
            ));
            grow(p);
        }
        b.triangles.push([base, base + 1, base + 2, 0]);
        b.triangles.push([base, base + 2, base + 3, 0]);
    }
    for c in caps {
        let base = b.vertices.len() as u32;
        // along the wall: z for a wall facing x, x for one facing z
        let along = if c.normal[0].abs() > 0.5 { 2 } else { 0 };
        for &p in &c.corners {
            b.vertices.push(vertex(
                p,
                [p[along] / f.tile, -p[1] / f.tile],
                argb(CAP_SHADE),
                c.normal,
            ));
            grow(p);
        }
        for k in 1..c.corners.len() as u32 - 1 {
            b.triangles.push([base, base + k, base + k + 1, 0]);
        }
    }
    b.sections.push(SynthSection {
        textures: [f.texture.as_bytes().to_vec(), Vec::new()],
        shader_kind: 1,
        triangles: b.triangles.len() as u32,
        ..SynthSection::default()
    });
    if let Some(bb) = bounds {
        b.center = bb.centre();
        b.half = clip::scale(clip::sub(bb.max, bb.min), 0.5);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_takes_its_footprint_out_of_a_strip() {
        let r = [0.0, 0.0, 100.0, 100.0];
        let area = |v: &[Rect]| v.iter().map(|q| (q[2] - q[0]) * (q[3] - q[1])).sum::<f32>();
        // through the middle: four strips round a 20 x 20 hole
        let c = Aabb::new([40.0, -10.0, 40.0], [60.0, 10.0, 60.0]);
        let got = minus_rect(r, &c);
        assert_eq!(got.len(), 4);
        assert!((area(&got) - (10_000.0 - 400.0)).abs() < 1e-3);
        // across an edge: the part outside stays whole
        let c = Aabb::new([-50.0, -10.0, 30.0], [50.0, 10.0, 70.0]);
        assert!((area(&minus_rect(r, &c)) - (10_000.0 - 50.0 * 40.0)).abs() < 1e-3);
        // apart: untouched
        let c = Aabb::new([200.0, -10.0, 0.0], [300.0, 10.0, 100.0]);
        assert_eq!(minus_rect(r, &c), vec![r]);
        // over all of it: nothing left
        let c = Aabb::new([-1.0, -10.0, -1.0], [101.0, 10.0, 101.0]);
        assert!(minus_rect(r, &c).is_empty());
    }
}
