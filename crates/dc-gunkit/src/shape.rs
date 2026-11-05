//! The shape of a gun's first-person model, measured from its triangles: which way it points, where its centre line runs, and whether
//! it has only the side the player sees. A model built that way is hollow from the far side, and the studio closes it with a reflection
//! of itself across that centre line.
// DC-DEFAULT: VAR-6 (proposed; USER REPORT 2026-11-05 "Still not fully rendering the other side")

use crate::lens::posed_point;
use dc_render::ModelMesh;
use glam::{Mat4, Vec3};
use std::collections::{HashMap, HashSet};

/// One triangle in model space: its middle, its size, and the way it faces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tri {
    pub at: Vec3,
    pub area: f32,
    pub facing: Vec3,
}

/// What was measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    /// The gun's sideways direction: a unit vector across it, level, pointing to the side with more surface.
    pub across: Vec3,
    /// How far along `across` the centre line is from the model's origin.
    pub centre: f32,
    /// The surface facing the emptier side as a share of the surface facing the fuller side: 1 is a whole gun, near 0 is one side only.
    pub balance: f32,
    /// How far the surface sits from the centre line on average, as a share of the gun's length. A gun hugs its centre line; loose parts,
    /// or a line in the wrong place, show here.
    pub spread: f32,
    /// A unit vector along the gun, level, toward the muzzle.
    pub toward_muzzle: Vec3,
}

/// A model whose emptier side has less than this share of the surface may have holes worth closing. Only the triangles with nothing across from
/// them are reflected (`reflected_mesh`), so a model that is whole but for a part loses nothing by being tried.
// OBSERVED: the player's own ROM: first-person models run from 0.1 (FAMAS), 0.29 (G36E, L96A1) and 0.47 (M16A3) up to 1.0
pub const ONE_SIDED_BELOW: f32 = 0.9;

/// A model whose surface sits further than this from its centre line, as a share of its length, is not reflected: a bad reflection is worse
/// than none.
// OBSERVED: the player's own ROM: every long gun and pistol measures 0.05 or less. Then comes a gap, and from 0.09 up are items with loose parts
// or no barrel to find (Heartbeat Sensor, First Aid Kit, Claymore, Anaconda's loose cylinder, Machete)
pub const MAX_SPREAD: f32 = 0.07;

impl Shape {
    /// Whether the model has only one side, and is shaped so that its reflection will close it.
    pub fn one_sided(&self) -> bool {
        self.balance < ONE_SIDED_BELOW && self.spread <= MAX_SPREAD
    }

    /// The reflection across the centre line, as an instance's own terms: the turn to add, the scale, and the offset in model space.
    pub fn reflection(&self) -> (glam::Quat, Vec3, Vec3) {
        // a reflection across a plane turned by t about the vertical is a turn by 2t after a reflection across x
        let t = self.across.z.atan2(self.across.x);
        let turn = glam::Quat::from_rotation_y(-2.0 * t);
        (
            turn,
            Vec3::new(-1.0, 1.0, 1.0),
            self.across * (2.0 * self.centre),
        )
    }
}

/// Where a point goes under the reflection (what [`Shape::reflection`] draws).
pub fn reflect(shape: &Shape, p: Vec3) -> Vec3 {
    p + shape.across * (2.0 * (shape.centre - p.dot(shape.across)))
}

/// The triangles of a model's first level of detail in its current pose.
pub fn posed_triangles(mesh: &dc_render::ModelMesh, bones: &[Mat4]) -> Vec<Tri> {
    let posed = |i: u32| -> Option<(Vec3, Vec3)> {
        let v = mesh.vertices.get(i as usize)?;
        let mut m = Mat4::ZERO;
        for k in 0..4 {
            if v.weights[k] != 0.0 {
                m += *bones.get(v.bones[k] as usize).unwrap_or(&Mat4::IDENTITY) * v.weights[k];
            }
        }
        Some((
            m.transform_point3(Vec3::from(v.pos)),
            m.transform_vector3(Vec3::from(v.normal)),
        ))
    };
    let mut out = Vec::new();
    for d in mesh.draws.iter().filter(|d| d.lod == 0) {
        let first = d.first_index as usize;
        let Some(list) = mesh.indices.get(first..first + d.index_count as usize) else {
            continue;
        };
        for t in list.as_chunks::<3>().0 {
            let (Some(a), Some(b), Some(c)) = (posed(t[0]), posed(t[1]), posed(t[2])) else {
                continue;
            };
            let area = (b.0 - a.0).cross(c.0 - a.0).length() * 0.5;
            let at = (a.0 + b.0 + c.0) / 3.0;
            if area > 0.0 && at.is_finite() {
                out.push(Tri {
                    at,
                    area,
                    facing: (a.1 + b.1 + c.1).normalize_or_zero(),
                });
            }
        }
    }
    out
}

/// The level direction a set of triangles is longest in, weighted by surface, and how many times longer it is than it is wide.
fn long_axis(tris: &[Tri]) -> Option<(Vec3, f32)> {
    let total: f32 = tris.iter().map(|t| t.area).sum();
    if total <= 0.0 {
        return None;
    }
    let (mx, mz) = tris.iter().fold((0.0, 0.0), |(x, z), t| {
        (x + t.at.x * t.area / total, z + t.at.z * t.area / total)
    });
    let (mut xx, mut xz, mut zz) = (0.0, 0.0, 0.0);
    for t in tris {
        let (dx, dz) = (t.at.x - mx, t.at.z - mz);
        xx += dx * dx * t.area;
        xz += dx * dz * t.area;
        zz += dz * dz * t.area;
    }
    let angle = 0.5 * (2.0 * xz).atan2(xx - zz);
    let axis = Vec3::new(angle.cos(), 0.0, angle.sin());
    let side = Vec3::new(-axis.z, 0.0, axis.x);
    let spread = |d: Vec3| xx * d.x * d.x + 2.0 * xz * d.x * d.z + zz * d.z * d.z;
    Some((axis, (spread(axis) / spread(side).max(1e-9)).sqrt()))
}

/// The share of a gun's length taken as its barrel when finding which way it points.
const BARREL_SHARE: f32 = 0.15;
/// A section at least this many times longer than it is wide is a barrel to trust.
const BARREL_MIN_RATIO: f32 = 3.0;
/// A barrel runs along the gun: a section whose line is further than this from the gun's own (about 20 degrees) is something else, such as a
/// bipod folded across, and would turn the whole gun sideways.
const BARREL_AGREES: f32 = 0.94;

/// A gun's direction, from a measured one. First-person models are authored along an axis, and their idle poses turn them by about a degree
/// at most, so a direction further from an axis than this is a bipod or a stubby shape misleading the measure.
// OBSERVED: the player's own ROM: G36E, M16A3 and FAMAS measure 0.00 degrees off an axis in the idle pose, and 102 of the 129 reflected
// models under 1 degree
pub const MAX_TILT_DEGREES: f32 = 1.5;

fn settled(long: Vec3) -> Vec3 {
    let axis = if long.x.abs() >= long.z.abs() {
        Vec3::new(long.x.signum(), 0.0, 0.0)
    } else {
        Vec3::new(0.0, 0.0, long.z.signum())
    };
    let off = long.dot(axis).clamp(-1.0, 1.0).acos().to_degrees();
    if off <= MAX_TILT_DEGREES {
        long
    } else {
        axis
    }
}

/// Measures a model. `None`: it has no surface to measure.
pub fn measure(tris: &[Tri]) -> Option<Shape> {
    // Which way the gun points comes from its barrel. The whole model will not do, because a one-sided model's surface sits to one side
    // of its barrel, and neither will the way its faces point, because first-person faces lean toward the player: both tilt the line,
    // and a reflection across a tilted line splays the two sides apart. A barrel is long and thin whichever side of it was modelled.
    let (rough, _) = long_axis(tris)?;
    let span = |axis: Vec3| {
        tris.iter().fold((f32::MAX, f32::MIN), |(lo, hi), t| {
            let v = t.at.dot(axis);
            (lo.min(v), hi.max(v))
        })
    };
    let (lo, hi) = span(rough);
    let length = hi - lo;
    if length <= 0.0 {
        return None;
    }
    // the muzzle is the shorter end. Height tells the ends apart, not width: a one-sided stock can be a flat plate with no width at all
    let height_at = |near: f32| {
        let (low, high) = tris
            .iter()
            .filter(|t| (t.at.dot(rough) - near).abs() <= length * 0.08)
            .fold((f32::MAX, f32::MIN), |(a, b), t| {
                (a.min(t.at.y), b.max(t.at.y))
            });
        high - low
    };
    let muzzle = if height_at(lo) <= height_at(hi) {
        lo
    } else {
        hi
    };
    let barrel: Vec<Tri> = tris
        .iter()
        .filter(|t| (t.at.dot(rough) - muzzle).abs() <= length * BARREL_SHARE)
        .copied()
        .collect();
    let long = settled(
        long_axis(&barrel)
            .filter(|(axis, ratio)| {
                *ratio >= BARREL_MIN_RATIO && axis.dot(rough).abs() >= BARREL_AGREES
            })
            .map_or(rough, |(axis, _)| axis),
    );
    // the barrel's line may come back pointing the other way, and the muzzle was found along the rough one
    let long = if long.dot(rough) < 0.0 { -long } else { long };
    let mut across = Vec3::new(-long.z, 0.0, long.x);
    // the barrel is round: the middle of its width at the muzzle is the centre line
    let tip = if muzzle == lo {
        span(long).0
    } else {
        span(long).1
    };
    let (m0, m1) = tris
        .iter()
        .filter(|t| (t.at.dot(long) - tip).abs() <= length * 0.08)
        .fold((f32::MAX, f32::MIN), |(a, b), t| {
            let v = t.at.dot(across);
            (a.min(v), b.max(v))
        });
    let mut centre = (m0 + m1) * 0.5;
    let (mut toward, mut away) = (0.0, 0.0);
    for t in tris {
        let f = t.facing.dot(across);
        if f > 0.0 {
            toward += t.area * f;
        } else {
            away -= t.area * f;
        }
    }
    let (full, empty): (f32, f32) = if toward >= away {
        (toward, away)
    } else {
        across = -across;
        centre = -centre;
        (away, toward)
    };
    if full <= 0.0 {
        return None;
    }
    let area: f32 = tris.iter().map(|t| t.area).sum();
    let spread = tris
        .iter()
        .map(|t| (t.at.dot(across) - centre).abs() * t.area)
        .sum::<f32>()
        / (area.max(1e-6) * length);
    Some(Shape {
        across,
        centre,
        balance: empty / full,
        spread,
        toward_muzzle: if muzzle == lo { -long } else { long },
    })
}

/// A copy of a mesh with only the first level of detail's triangles that `keep` keeps. Other levels are left as they are.
pub fn retain_triangles(mesh: &ModelMesh, mut keep: impl FnMut([u32; 3]) -> bool) -> ModelMesh {
    let mut out = mesh.clone();
    out.indices.clear();
    for (d, new) in mesh.draws.iter().zip(out.draws.iter_mut()) {
        let first = d.first_index as usize;
        let list = mesh
            .indices
            .get(first..first + d.index_count as usize)
            .unwrap_or_default();
        new.first_index = out.indices.len() as u32;
        for t in list.as_chunks::<3>().0 {
            if d.lod != 0 || keep(*t) {
                out.indices.extend_from_slice(t);
            }
        }
        new.index_count = out.indices.len() as u32 - new.first_index;
    }
    out
}

/// The corners of a mesh's first level of detail in the current pose, as triangles.
fn posed_corners(mesh: &ModelMesh, bones: &[Mat4]) -> Vec<([u32; 3], [Vec3; 3])> {
    let mut out = Vec::new();
    for d in mesh.draws.iter().filter(|d| d.lod == 0) {
        let first = d.first_index as usize;
        let list = mesh
            .indices
            .get(first..first + d.index_count as usize)
            .unwrap_or_default();
        for t in list.as_chunks::<3>().0 {
            let (Some(a), Some(b), Some(c)) = (
                posed_point(mesh, t[0], bones),
                posed_point(mesh, t[1], bones),
                posed_point(mesh, t[2], bones),
            ) else {
                continue;
            };
            out.push((*t, [a, b, c]));
        }
    }
    out
}

/// The bounds of a mesh's first level of detail in the current pose.
pub fn mesh_bounds(mesh: &ModelMesh, bones: &[Mat4]) -> Option<(Vec3, Vec3)> {
    let mut out: Option<(Vec3, Vec3)> = None;
    for (_, points) in posed_corners(mesh, bones) {
        for p in points {
            out = Some(match out {
                None => (p, p),
                Some((a, b)) => (a.min(p), b.max(p)),
            });
        }
    }
    out
}

/// How far a part may sit from the rest of the gun, as a share of the gun's size, and still be part of it.
const LOOSE_GAP: f32 = 0.1;
/// More of the surface than this being loose means the model is not a gun with a stray part: it is left whole.
const LOOSE_MAX_SHARE: f32 = 0.35;

/// A mesh without its loose parts: islands of triangles far from the rest of the gun, such as a revolver's speed loader, which waits out of
/// the player's sight until a reload brings it in.
// DC-DEFAULT: VAR-6 (proposed; USER REPORT 2026-11-05 the Anaconda's floating cylinder)
pub fn without_loose_parts(mesh: &ModelMesh, bones: &[Mat4]) -> ModelMesh {
    let tris = posed_corners(mesh, bones);
    if tris.is_empty() {
        return mesh.clone();
    }
    let mut parent: Vec<usize> = (0..tris.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut seen: HashMap<[i32; 3], usize> = HashMap::new();
    for (k, (_, points)) in tris.iter().enumerate() {
        for p in points {
            let q = *p * 2000.0;
            let key = [q.x.round() as i32, q.y.round() as i32, q.z.round() as i32];
            match seen.get(&key) {
                Some(&other) => {
                    let (a, b) = (find(&mut parent, k), find(&mut parent, other));
                    parent[a] = b;
                }
                None => {
                    seen.insert(key, k);
                }
            }
        }
    }
    struct Island {
        lo: Vec3,
        hi: Vec3,
        area: f32,
        joined: bool,
    }
    let mut islands: HashMap<usize, Island> = HashMap::new();
    for (k, (_, [a, b, c])) in tris.iter().enumerate() {
        let root = find(&mut parent, k);
        let e = islands.entry(root).or_insert(Island {
            lo: *a,
            hi: *a,
            area: 0.0,
            joined: false,
        });
        e.lo = e.lo.min(*a).min(*b).min(*c);
        e.hi = e.hi.max(*a).max(*b).max(*c);
        e.area += (*b - *a).cross(*c - *a).length() * 0.5;
    }
    let total: f32 = islands.values().map(|i| i.area).sum();
    let Some(main) = islands
        .iter()
        .max_by(|a, b| a.1.area.total_cmp(&b.1.area))
        .map(|(k, _)| *k)
    else {
        return mesh.clone();
    };
    // the gun grows from its largest island, part by part, so that a long chain of small parts stays together
    let (mut lo, mut hi) = (islands[&main].lo, islands[&main].hi);
    if let Some(m) = islands.get_mut(&main) {
        m.joined = true;
    }
    loop {
        let reach = (hi - lo).max_element() * LOOSE_GAP;
        let mut grew = false;
        for i in islands.values_mut().filter(|i| !i.joined) {
            let gap = (i.lo - hi).max(lo - i.hi).max(Vec3::ZERO).length();
            if gap <= reach {
                i.joined = true;
                lo = lo.min(i.lo);
                hi = hi.max(i.hi);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let loose: f32 = islands.values().filter(|i| !i.joined).map(|i| i.area).sum();
    if loose <= 0.0 || loose > total * LOOSE_MAX_SHARE {
        return mesh.clone();
    }
    let keep: Vec<bool> = (0..tris.len())
        .map(|k| {
            let root = find(&mut parent, k);
            islands[&root].joined
        })
        .collect();
    let mut k = 0;
    let mut next = 0;
    retain_triangles(mesh, |t| {
        // the triangles come in the order `posed_corners` read them
        while next < tris.len() && tris[next].0 != t {
            next += 1;
        }
        k = next;
        next += 1;
        keep.get(k).copied().unwrap_or(true)
    })
}

/// How finely the far side of a gun is mapped to see what is already there, as a share of its length.
const COVER_CELLS: f32 = 128.0;
/// A reflection adding less than this share of the surface closes nothing worth drawing.
const MIN_REFLECTED_SHARE: f32 = 0.03;

/// The cells on the gun's side view that a triangle covers.
fn cells_of(points: &[Vec3; 3], long: Vec3, cell: f32) -> Vec<(i32, i32)> {
    let uv = |p: Vec3| (p.dot(long) / cell, p.y / cell);
    let [a, b, c] = points.map(uv);
    let edge = |p: (f32, f32), q: (f32, f32)| (p.0 - q.0).abs().max((p.1 - q.1).abs());
    let steps = (edge(a, b).max(edge(b, c)).max(edge(c, a)).ceil() as i32 + 1).clamp(1, 64);
    let mut out = Vec::new();
    for i in 0..=steps {
        for j in 0..=(steps - i) {
            let (s, t) = (i as f32 / steps as f32, j as f32 / steps as f32);
            let r = 1.0 - s - t;
            let (u, v) = (a.0 * s + b.0 * t + c.0 * r, a.1 * s + b.1 * t + c.1 * r);
            out.push((u.floor() as i32, v.floor() as i32));
        }
    }
    out
}

/// The triangles a reflection should draw: those on the side the model has, with nothing on the far side across from them. A part that is already
/// whole, such as a magazine, has its far side there, and reflecting it would put two surfaces in one place, which flicker. Faces lying on the
/// centre line would land on themselves, and those crossing it are already on both sides. `None`: nothing worth reflecting.
// DC-DEFAULT: VAR-6 (proposed; USER REPORT 2026-11-05 "there's flickering and missing texture on magazines")
pub fn reflected_mesh(mesh: &ModelMesh, bones: &[Mat4], shape: &Shape) -> Option<ModelMesh> {
    let tris = posed_corners(mesh, bones);
    let long = Vec3::new(shape.across.z, 0.0, -shape.across.x);
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for (_, points) in &tris {
        for p in points {
            lo = lo.min(p.dot(long));
            hi = hi.max(p.dot(long));
        }
    }
    let length = hi - lo;
    if length <= 0.0 {
        return None;
    }
    let cell = length / COVER_CELLS;
    let margin = length * 0.004;
    let side = |p: Vec3| p.dot(shape.across) - shape.centre;
    let mut far: HashSet<(i32, i32)> = HashSet::new();
    for (_, points) in &tris {
        let mid = (points[0] + points[1] + points[2]) / 3.0;
        if side(mid) < -margin {
            for (u, v) in cells_of(points, long, cell) {
                for du in -1..=1 {
                    for dv in -1..=1 {
                        far.insert((u + du, v + dv));
                    }
                }
            }
        }
    }
    let area = |p: &[Vec3; 3]| (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
    let total: f32 = tris.iter().map(|(_, p)| area(p)).sum();
    let mut kept_area = 0.0;
    let keep: HashSet<[u32; 3]> = tris
        .iter()
        .filter(|(_, p)| {
            let s = p.map(side);
            if s.iter().copied().fold(f32::MAX, f32::min) < -margin
                || s.iter().copied().fold(f32::MIN, f32::max) <= margin
            {
                return false;
            }
            let cells = cells_of(p, long, cell);
            let covered = cells.iter().filter(|c| far.contains(c)).count();
            let open = covered * 2 < cells.len();
            if open {
                kept_area += area(p);
            }
            open
        })
        .map(|(t, _)| *t)
        .collect();
    if kept_area < total * MIN_REFLECTED_SHARE {
        return None;
    }
    Some(retain_triangles(mesh, |t| keep.contains(&t)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A long flat wall along z, facing one way, with a round barrel at its front on the centre line.
    fn shell(facing_x: f32, wall_x: f32) -> Vec<Tri> {
        let mut out = Vec::new();
        for k in 0..40 {
            for y in [-6.0, 6.0] {
                out.push(Tri {
                    at: Vec3::new(wall_x, y, k as f32),
                    area: 10.0,
                    facing: Vec3::new(facing_x, 0.0, 0.0),
                });
            }
        }
        for z in 0..16 {
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let n = Vec3::new(a.cos(), a.sin(), 0.0);
                out.push(Tri {
                    at: n * 0.5 + Vec3::new(0.0, 0.0, 85.0 + z as f32),
                    area: 0.5,
                    facing: n,
                });
            }
        }
        out
    }

    #[test]
    fn a_one_sided_model_is_seen_and_its_centre_is_the_barrel() {
        let s = measure(&shell(-1.0, -3.0)).unwrap();
        assert!(s.one_sided(), "{s:?}");
        assert!(s.balance < 0.1, "{s:?}");
        assert!(s.centre.abs() < 0.6, "{s:?}");
        assert!(
            s.across.x < -0.9,
            "it points to the side with the surface: {s:?}"
        );
    }

    #[test]
    fn a_whole_gun_is_left_alone() {
        let mut both = shell(-1.0, -3.0);
        both.extend(shell(1.0, 3.0));
        let s = measure(&both).unwrap();
        assert!(!s.one_sided(), "{s:?}");
        assert!(s.balance > 0.9, "{s:?}");
    }

    #[test]
    fn the_reflection_puts_a_wall_on_the_far_side_of_the_centre_line() {
        let s = measure(&shell(-1.0, -3.0)).unwrap();
        let p = Vec3::new(-3.0, 2.0, 20.0);
        let q = reflect(&s, p);
        assert!(
            (q.x - 3.0).abs() < 0.6,
            "the wall lands across the barrel: {q:?}"
        );
        assert_eq!(q.y, p.y);
        assert!((q.z - p.z).abs() < 0.1, "{q:?}");
        assert!(
            (reflect(&s, q) - p).length() < 1e-4,
            "twice is where it began"
        );
    }

    #[test]
    fn the_instance_terms_are_the_same_reflection_on_a_turned_gun() {
        for degrees in [0.0_f32, 7.0, -12.0, 90.0] {
            let turn = glam::Quat::from_rotation_y(degrees.to_radians());
            let across = turn * Vec3::X;
            let s = Shape {
                across,
                centre: 1.5,
                balance: 0.0,
                spread: 0.0,
                toward_muzzle: Vec3::Z,
            };
            let (r, scale, offset) = s.reflection();
            for p in [Vec3::new(1.0, 2.0, 3.0), Vec3::new(-4.0, 0.5, 9.0)] {
                let got = r * (p * scale) + offset;
                let want = reflect(&s, p);
                assert!((got - want).length() < 1e-3, "{degrees}: {got:?} {want:?}");
            }
        }
    }

    fn turned(degrees: f32) -> Vec<Tri> {
        let turn = glam::Quat::from_rotation_y(degrees.to_radians());
        shell(-1.0, -3.0)
            .into_iter()
            .map(|t| Tri {
                at: turn * t.at,
                area: t.area,
                facing: turn * t.facing,
            })
            .collect()
    }

    #[test]
    fn a_small_turn_from_the_idle_pose_is_followed_with_no_wedge_between_the_walls() {
        for degrees in [0.0_f32, 0.8, -1.2] {
            let turn = glam::Quat::from_rotation_y(degrees.to_radians());
            let s = measure(&turned(degrees)).unwrap();
            let want = turn * Vec3::NEG_X;
            assert!(s.across.dot(want) > 0.9999, "{degrees}: {s:?}");
            let width = |z: f32| {
                let p = turn * Vec3::new(-3.0, 0.0, z);
                (reflect(&s, p) - p).length()
            };
            // a measured barrel is good to about 0.2 degrees: a sliver across the 39 units of this wall, against a gun 6 wide
            assert!((width(1.0) - width(39.0)).abs() < 0.5, "{degrees}: {s:?}");
        }
    }

    #[test]
    fn a_large_reading_settles_on_the_axis_the_model_is_built_along() {
        for degrees in [6.0_f32, -15.0] {
            let s = measure(&turned(degrees)).unwrap();
            assert!(s.across.x.abs() > 0.99999, "{degrees}: {s:?}");
        }
        let along_x = Vec3::new(0.3, 0.0, -0.05).normalize();
        assert_eq!(settled(along_x), Vec3::X);
        let nearly = Vec3::new(0.999_9, 0.0, 0.014).normalize();
        assert_eq!(settled(nearly), nearly);
        let along_z = Vec3::new(0.2, 0.0, -0.9).normalize();
        assert_eq!(settled(along_z), Vec3::NEG_Z);
    }

    #[test]
    fn faces_leaning_toward_the_player_do_not_tilt_the_centre_line() {
        // first-person walls lean toward the viewer, and the model's surface sits to one side of the barrel
        let mut tris = shell(-1.0, -3.0);
        for t in &mut tris {
            if t.facing.x < -0.9 {
                t.facing = Vec3::new(-0.8, 0.0, -0.6);
            }
        }
        let s = measure(&tris).unwrap();
        assert!(s.across.x.abs() > 0.9999, "{s:?}");
        assert!(s.centre.abs() < 0.6, "{s:?}");
    }

    #[test]
    fn a_model_with_loose_parts_far_from_its_centre_line_is_not_reflected() {
        let mut tris = shell(-1.0, -3.0);
        assert!(measure(&tris).unwrap().one_sided());
        for t in &mut tris {
            if t.area > 1.0 {
                t.at.x = -40.0;
            }
        }
        let s = measure(&tris).unwrap();
        assert!(s.balance < ONE_SIDED_BELOW, "{s:?}");
        assert!(s.spread > MAX_SPREAD, "{s:?}");
        assert!(!s.one_sided(), "{s:?}");
    }

    /// A mesh from quads given by corners: each quad two triangles.
    fn mesh_of(quads: &[[[f32; 3]; 4]]) -> ModelMesh {
        let mut mesh = ModelMesh::default();
        for q in quads {
            let at = mesh.vertices.len() as u32;
            for p in q {
                mesh.vertices.push(dc_render::ModelVertex {
                    pos: *p,
                    normal: [0.0, 0.0, 1.0],
                    uv: [0.0, 0.0],
                    bones: [0; 4],
                    weights: [1.0, 0.0, 0.0, 0.0],
                });
            }
            mesh.indices
                .extend_from_slice(&[at, at + 1, at + 2, at, at + 2, at + 3]);
        }
        mesh.draws.push(dc_render::PieceDraw {
            piece: 0,
            lod: 0,
            first_index: 0,
            index_count: mesh.indices.len() as u32,
            material: 0,
            slots: [0; 4],
            slot_count: 1,
            style: 0,
            priority: 0,
            kind: 0,
        });
        mesh
    }

    fn wall(x: f32, z0: f32, z1: f32, y0: f32, y1: f32) -> [[f32; 3]; 4] {
        [[x, y0, z0], [x, y0, z1], [x, y1, z1], [x, y1, z0]]
    }

    const NO_POSE: [Mat4; 0] = [];

    fn side_on() -> Shape {
        Shape {
            across: Vec3::NEG_X,
            centre: 0.0,
            balance: 0.3,
            spread: 0.02,
            toward_muzzle: Vec3::Z,
        }
    }

    #[test]
    fn a_wall_with_nothing_across_from_it_is_reflected_and_a_closed_part_is_not() {
        // the receiver's wall on the -x side only, and a magazine with both of its walls
        let mesh = mesh_of(&[
            wall(-3.0, 0.0, 60.0, 5.0, 15.0),
            wall(-1.0, 70.0, 90.0, -20.0, 0.0),
            wall(1.0, 70.0, 90.0, -20.0, 0.0),
        ]);
        let r = reflected_mesh(&mesh, &NO_POSE, &side_on()).expect("the wall needs closing");
        assert_eq!(r.indices.len(), 6, "only the receiver's two triangles");
        assert_eq!(&r.indices[..6], &[0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn a_whole_model_has_nothing_to_reflect() {
        let mesh = mesh_of(&[
            wall(-3.0, 0.0, 60.0, 5.0, 15.0),
            wall(3.0, 0.0, 60.0, 5.0, 15.0),
        ]);
        assert!(reflected_mesh(&mesh, &NO_POSE, &side_on()).is_none());
    }

    #[test]
    fn a_face_on_the_centre_line_or_across_it_is_not_reflected_onto_itself() {
        let on_line = wall(0.0, 0.0, 60.0, 5.0, 15.0);
        let across = [
            [-2.0, 15.0, 0.0],
            [2.0, 15.0, 0.0],
            [2.0, 15.0, 60.0],
            [-2.0, 15.0, 60.0],
        ];
        let mesh = mesh_of(&[on_line, across]);
        assert!(reflected_mesh(&mesh, &NO_POSE, &side_on()).is_none());
    }

    #[test]
    fn a_loose_part_is_left_out_and_small_parts_near_the_gun_stay() {
        let mesh = mesh_of(&[
            wall(-3.0, 0.0, 60.0, 5.0, 15.0),
            // a small part just off the gun: a trigger
            wall(-3.0, 30.0, 32.0, 2.0, 4.0),
            // far below: a speed loader
            wall(-3.0, 30.0, 34.0, -80.0, -76.0),
        ]);
        let m = without_loose_parts(&mesh, &NO_POSE);
        assert_eq!(m.indices.len(), 12, "the wall and the trigger");
        let (lo, _) = mesh_bounds(&m, &NO_POSE).unwrap();
        assert!(lo.y > 0.0, "{lo:?}");
    }

    #[test]
    fn a_model_of_two_equal_parts_is_not_cut_in_half() {
        let mesh = mesh_of(&[
            wall(-3.0, 0.0, 10.0, 0.0, 10.0),
            wall(-3.0, 500.0, 510.0, 0.0, 10.0),
        ]);
        assert_eq!(without_loose_parts(&mesh, &NO_POSE).indices.len(), 12);
    }

    #[test]
    fn a_part_lying_across_the_muzzle_does_not_turn_the_gun_sideways() {
        let mut tris = shell(-1.0, -3.0);
        // long thin bars across the gun at its front, as a folded bipod is, which outweigh the barrel there
        for k in 0..40 {
            tris.push(Tri {
                at: Vec3::new(k as f32 - 20.0, 1.0, 95.0),
                area: 4.0,
                facing: Vec3::Y,
            });
        }
        let s = measure(&tris).unwrap();
        assert!(s.across.x.abs() > 0.9, "the gun still runs along z: {s:?}");
    }

    #[test]
    fn the_muzzle_is_named_by_its_direction() {
        let s = measure(&shell(-1.0, -3.0)).unwrap();
        assert!(
            s.toward_muzzle.z > 0.99,
            "the barrel of the test gun is at +z: {s:?}"
        );
    }

    #[test]
    fn nothing_to_measure_says_so() {
        assert_eq!(measure(&[]), None);
    }

    #[test]
    fn triangles_are_read_from_the_first_level_only() {
        let v = |p: [f32; 3]| dc_render::ModelVertex {
            pos: p,
            normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            bones: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
        };
        let draw = |lod, first| dc_render::PieceDraw {
            piece: 0,
            lod,
            first_index: first,
            index_count: 3,
            material: 0,
            slots: [0; 4],
            slot_count: 1,
            style: 0,
            priority: 0,
            kind: 0,
        };
        let mesh = dc_render::ModelMesh {
            vertices: vec![v([0.0, 0.0, 0.0]), v([2.0, 0.0, 0.0]), v([0.0, 2.0, 0.0])],
            indices: vec![0, 1, 2, 0, 1, 2],
            draws: vec![draw(0, 0), draw(1, 3)],
            ..Default::default()
        };
        let moved = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let t = posed_triangles(&mesh, &[moved]);
        assert_eq!(t.len(), 1);
        assert!((t[0].area - 2.0).abs() < 1e-5);
        assert!(t[0].at.x > 10.0);
        assert_eq!(t[0].facing, Vec3::Z);
    }
}
