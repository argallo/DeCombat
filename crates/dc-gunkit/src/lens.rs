//! What the model says about its texture: which texels end up where on the gun, and where the lenses are. A lens is a small flat round group
//! of faces looking along the barrel, away from the muzzle's own end, and it maps to a circle on the texture. So a camo can leave it clean on
//! any gun, without anyone finding the circle by hand.
// DC-DEFAULT: VAR-9 (proposed; USER REQUEST 2026-11-05 "not adding the texture over the scope lenses like we did properly on the galaxy and
// toxic skins")

use dc_render::camo::Circle;
use dc_render::ModelMesh;
use glam::{Mat4, Vec2, Vec3};
use std::collections::HashMap;

/// A vertex in the current pose.
pub fn posed_point(mesh: &ModelMesh, i: u32, bones: &[Mat4]) -> Option<Vec3> {
    let v = mesh.vertices.get(i as usize)?;
    let mut m = Mat4::ZERO;
    for k in 0..4 {
        if v.weights[k] != 0.0 {
            m += *bones.get(v.bones[k] as usize).unwrap_or(&Mat4::IDENTITY) * v.weights[k];
        }
    }
    let p = m.transform_point3(Vec3::from(v.pos));
    p.is_finite().then_some(p)
}

/// A triangle with what the texture needs to know of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinTri {
    pub points: [Vec3; 3],
    /// Where its corners are on the texture, in texels.
    pub texels: [Vec2; 3],
    pub normal: Vec3,
    pub area: f32,
}

/// The triangles drawn with one skin slot, at the first level of detail.
pub fn skin_triangles(
    mesh: &ModelMesh,
    bones: &[Mat4],
    slot: u32,
    size: (u32, u32),
) -> Vec<SkinTri> {
    let (w, h) = (size.0 as f32, size.1 as f32);
    let mut out = Vec::new();
    for d in mesh
        .draws
        .iter()
        .filter(|d| d.lod == 0 && d.material == slot)
    {
        let first = d.first_index as usize;
        let Some(list) = mesh.indices.get(first..first + d.index_count as usize) else {
            continue;
        };
        for t in list.as_chunks::<3>().0 {
            let corner = |i: u32| -> Option<(Vec3, Vec2)> {
                let p = posed_point(mesh, i, bones)?;
                Some((p, Vec2::from(mesh.vertices.get(i as usize)?.uv)))
            };
            let (Some(mut a), Some(mut b), Some(mut c)) =
                (corner(t[0]), corner(t[1]), corner(t[2]))
            else {
                continue;
            };
            // a texture repeats, so a triangle may lie in any repeat of it. It moves as one: a corner alone at exactly 1 is the texture's far edge, and
            // sent to 0 it would stretch the triangle across the whole texture
            let tile = a.1.min(b.1).min(c.1).floor();
            for q in [&mut a, &mut b, &mut c] {
                q.1 = (q.1 - tile) * Vec2::new(w, h);
            }
            let cross = (b.0 - a.0).cross(c.0 - a.0);
            let area = cross.length() * 0.5;
            if area > 0.0 {
                out.push(SkinTri {
                    points: [a.0, b.0, c.0],
                    texels: [a.1, b.1, c.1],
                    normal: cross.normalize(),
                    area,
                });
            }
        }
    }
    out
}

/// Groups of triangles joined at a corner, on the model and on the texture alike: the parts the texture is cut into.
pub fn parts(tris: &[SkinTri]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..tris.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    // a corner is the same corner when it is at the same place on the model and on the texture
    let mut seen: HashMap<[i32; 5], usize> = HashMap::new();
    for (k, t) in tris.iter().enumerate() {
        for c in 0..3 {
            let (p, q) = (t.points[c] * 2000.0, t.texels[c] * 4.0);
            let key = [
                p.x.round() as i32,
                p.y.round() as i32,
                p.z.round() as i32,
                q.x.round() as i32,
                q.y.round() as i32,
            ];
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
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for k in 0..tris.len() {
        let root = find(&mut parent, k);
        groups.entry(root).or_default().push(k);
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().collect();
    out.sort_by_key(|g| g[0]);
    out
}

/// A lens found on the texture, and how round it measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lens {
    pub circle: Circle,
    /// 1 is a perfect circle on the texture.
    pub roundness: f32,
    /// How high on the gun it sits: 0 at the bottom, 1 at the top.
    pub height: f32,
}

/// The fewest triangles a round face has: fewer is a square or a hexagonal bolt head.
const LENS_MIN_TRIS: usize = 5;
/// How squarely a lens looks along the barrel.
const LENS_FACING: f32 = 0.8;
/// How round a lens's outline must be.
const LENS_MIN_ROUNDNESS: f32 = 0.8;
/// The largest lens radius, as a share of the gun's length.
const LENS_MAX_RADIUS: f32 = 0.08;
/// The share of the gun's length at the muzzle whose round faces are the muzzle itself.
const MUZZLE_ZONE: f32 = 0.06;

/// Finds the lenses of a model: round flat parts looking along the barrel, away from the muzzle.
pub fn find_lenses(tris: &[SkinTri], long: Vec3) -> Vec<Lens> {
    let along = |p: Vec3| p.dot(long);
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let (mut low_y, mut high_y) = (f32::MAX, f32::MIN);
    for t in tris {
        for p in t.points {
            lo = lo.min(along(p));
            hi = hi.max(along(p));
            low_y = low_y.min(p.y);
            high_y = high_y.max(p.y);
        }
    }
    let length = hi - lo;
    if length <= 0.0 {
        return Vec::new();
    }
    // only the faces looking along the barrel can be a lens: joined on their own, a lens is not fused to the scope's tube around it
    let facing: Vec<usize> = (0..tris.len())
        .filter(|&k| tris[k].normal.dot(long).abs() >= LENS_FACING)
        .collect();
    let subset: Vec<SkinTri> = facing.iter().map(|&k| tris[k]).collect();
    let mut found: Vec<Lens> = Vec::new();
    for group in parts(&subset) {
        if group.len() < LENS_MIN_TRIS {
            continue;
        }
        let points: Vec<Vec3> = group.iter().flat_map(|&k| subset[k].points).collect();
        let texels: Vec<Vec2> = group.iter().flat_map(|&k| subset[k].texels).collect();
        let n = points.len() as f32;
        let mid = points.iter().copied().sum::<Vec3>() / n;
        // not on the muzzle's own end, and above the middle of the gun, where a sight is
        let from_ends = (along(mid) - lo).min(hi - along(mid));
        if from_ends < length * MUZZLE_ZONE * 0.5 {
            continue;
        }
        if mid.y < (low_y + high_y) * 0.5 {
            continue;
        }
        let (tmin, tmax) = texels.iter().fold(
            (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
            |(a, b), t| (a.min(*t), b.max(*t)),
        );
        let centre = (tmin + tmax) * 0.5;
        let size = tmax - tmin;
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        // on the model it is a small disc
        let radius_model = points
            .iter()
            .map(|p| (*p - mid).length())
            .fold(0.0, f32::max);
        if radius_model > length * LENS_MAX_RADIUS {
            continue;
        }
        // on the texture the outline's points are all about the same distance from the middle
        let radius = size.max_element() * 0.5;
        let far: Vec<f32> = texels
            .iter()
            .map(|t| (*t - centre).length())
            .filter(|d| *d > radius * 0.5)
            .collect();
        if far.len() < 5 {
            continue;
        }
        let mean = far.iter().sum::<f32>() / far.len() as f32;
        let wander = far.iter().map(|d| (d - mean).abs()).sum::<f32>() / far.len() as f32;
        let aspect = size.min_element() / size.max_element();
        let roundness = aspect.min(1.0 - wander / mean.max(1e-3));
        if roundness < LENS_MIN_ROUNDNESS {
            continue;
        }
        // a texel or two of margin keeps the paint off the edge
        let circle = Circle {
            x: centre.x,
            y: centre.y,
            radius: mean + 2.0,
        };
        let lens = Lens {
            circle,
            roundness,
            height: (mid.y - low_y) / (high_y - low_y).max(1e-6),
        };
        // front and back of a scope often share one circle of the texture
        let same = found.iter().position(|l| {
            let d = (l.circle.x - circle.x).hypot(l.circle.y - circle.y);
            d < l.circle.radius.max(circle.radius)
        });
        match same {
            Some(i) if found[i].circle.radius >= circle.radius => {}
            Some(i) => found[i] = lens,
            None => found.push(lens),
        }
    }
    found.sort_by(|a, b| b.circle.radius.total_cmp(&a.circle.radius));
    found
}

/// The share of a gun's length that a scope's end may run, at least and at most.
const CAP_SIZE: (f32, f32) = (0.012, 0.09);
/// How square a lens's face is on the model, at least.
const CAP_SQUARE: f32 = 0.6;
/// How high on the gun a lens is, at least. A third-person model is posed nose down, so one end of a scope sits lower than the other.
const CAP_MIN_HEIGHT: f32 = 0.6;
/// How far from either end of the gun a lens is, at least, as a share of its length: what is further out is a muzzle or a stock.
const CAP_FROM_ENDS: f32 = 0.1;
/// The size of a lens's picture on the texture, at least, as a share of its width, and how square that picture is. An artist gives a lens far more of
/// the texture than so small a face would get.
// OBSERVED: the player's own ROM, on 256 textures: AUG 30x35, Dragunov 33x38, L85A1 MOD 30x34, M24 48x56, MSG-90 36x39, SR25 29x29, G36E 35x35, L96A1 45x46,
// against 7x13 to 20x15 for the mounts and rings around them
const CAP_PICTURE: f32 = 0.1;
const CAP_PICTURE_SQUARE: f32 = 0.7;
/// How square a short sight's window is on the model, at least: a reflex sight's is wide.
const WINDOW_SQUARE: f32 = 0.35;
/// How far apart a short sight's windows are along the gun, as a share of its length.
const WINDOW_RUN: (f32, f32) = (0.02, 0.2);
/// How alike in size a short sight's windows are, at least.
const WINDOW_ALIKE: f32 = 0.6;
/// How far below the highest window another may be, as a share of the gun's height.
const WINDOW_BAND: f32 = 0.2;
/// How far a short sight's windows may be off one line along the gun, as a share of its length.
const WINDOW_IN_LINE: f32 = 0.03;
/// The smallest circle kept, in texels.
const CAP_MIN_RADIUS: f32 = 3.0;

/// One flat group of faces looking along the barrel.
#[derive(Debug, Clone, Copy)]
struct Cap {
    /// Which way along the barrel it looks.
    ahead: bool,
    /// Along the gun, and across it, as shares of its length.
    at: f32,
    across: f32,
    /// Its height on the gun, 0 to 1, and its larger side as a share of the gun's length.
    height: f32,
    size: f32,
    /// Its smaller side against its larger, on the model and on the texture.
    square: f32,
    picture_square: f32,
    /// Its smaller side on the texture, as a share of the texture's width.
    picture: f32,
    lens: Lens,
}

/// The lenses of a low model, whose lens is a square or a hexagon of two to four triangles and cannot be told by being round. It is told by its picture: a large
/// square part of the texture on a small face. A reflex sight has no such picture, so on a model with one skin, which has its sight on that skin, its two windows are
/// taken instead: the highest two wide faces, close together and looking opposite ways. A model with more skins has its sight on one of its own, which is not painted.
/// Only for a gun the game says has a sight. `size` is the texture's.
// DC-DEFAULT: VAR-9 (proposed; USER REPORT 2026-11-05 the sights of the third-person M24 and M416 CQB painted over)
pub fn scope_caps(tris: &[SkinTri], long: Vec3, size: (u32, u32), one_skin: bool) -> Vec<Lens> {
    let along = |p: Vec3| p.dot(long);
    let side = Vec3::new(-long.z, 0.0, long.x);
    let (mut lo, mut hi, mut low_y, mut high_y) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for t in tris {
        for p in t.points {
            lo = lo.min(along(p));
            hi = hi.max(along(p));
            low_y = low_y.min(p.y);
            high_y = high_y.max(p.y);
        }
    }
    let length = hi - lo;
    if length <= 0.0 || size.0 == 0 || size.1 == 0 {
        return Vec::new();
    }
    let subset: Vec<SkinTri> = tris
        .iter()
        .filter(|t| t.normal.dot(long).abs() >= LENS_FACING)
        .copied()
        .collect();
    let mut caps: Vec<Cap> = Vec::new();
    for group in parts(&subset) {
        if group.len() < 2 {
            continue;
        }
        let points: Vec<Vec3> = group.iter().flat_map(|&k| subset[k].points).collect();
        let texels: Vec<Vec2> = group.iter().flat_map(|&k| subset[k].texels).collect();
        let mid = points.iter().copied().sum::<Vec3>() / points.len() as f32;
        let (mut s0, mut s1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &points {
            s0 = s0.min(p.dot(side));
            s1 = s1.max(p.dot(side));
            y0 = y0.min(p.y);
            y1 = y1.max(p.y);
        }
        let (wide, high) = (s1 - s0, y1 - y0);
        let face = wide.max(high) / length;
        let at = (along(mid) - lo) / length;
        if !(CAP_SIZE.0..=CAP_SIZE.1).contains(&face) || at.min(1.0 - at) < CAP_FROM_ENDS {
            continue;
        }
        let (tmin, tmax) = texels.iter().fold(
            (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
            |(a, b), t| (a.min(*t), b.max(*t)),
        );
        let centre = (tmin + tmax) * 0.5;
        let height = (mid.y - low_y) / (high_y - low_y).max(1e-6);
        let span = tmax - tmin;
        caps.push(Cap {
            ahead: subset[group[0]].normal.dot(long) > 0.0,
            at,
            across: mid.dot(side) / length,
            height,
            size: face,
            square: wide.min(high) / wide.max(high).max(1e-6),
            picture_square: span.min_element() / span.max_element().max(1e-6),
            picture: span.min_element() / size.0 as f32,
            lens: Lens {
                circle: Circle {
                    x: centre.x,
                    y: centre.y,
                    radius: (span.max_element() * 0.5 + 1.5).max(CAP_MIN_RADIUS),
                },
                roundness: 0.0,
                height,
            },
        });
    }
    let mut found: Vec<Lens> = caps
        .iter()
        .filter(|c| {
            c.square >= CAP_SQUARE
                && c.height >= CAP_MIN_HEIGHT
                && c.picture >= CAP_PICTURE
                && c.picture_square >= CAP_PICTURE_SQUARE
        })
        .map(|c| c.lens)
        .collect();
    if found.is_empty() && one_skin {
        let fit: Vec<Cap> = caps
            .iter()
            .filter(|c| c.square >= WINDOW_SQUARE)
            .copied()
            .collect();
        let top = fit.iter().map(|c| c.height).fold(f32::MIN, f32::max);
        let mut best: Option<(Cap, Cap, f32)> = None;
        for a in fit
            .iter()
            .filter(|c| c.ahead && c.height >= top - WINDOW_BAND)
        {
            for b in fit
                .iter()
                .filter(|c| !c.ahead && c.height >= top - WINDOW_BAND)
            {
                let apart = (a.at - b.at).abs();
                if !(WINDOW_RUN.0..=WINDOW_RUN.1).contains(&apart)
                    || a.size.min(b.size) / a.size.max(b.size) < WINDOW_ALIKE
                    || (a.across - b.across).abs() > WINDOW_IN_LINE
                {
                    continue;
                }
                let high = a.height + b.height;
                if best.as_ref().is_none_or(|x| high > x.2) {
                    best = Some((*a, *b, high));
                }
            }
        }
        if let Some((a, b, _)) = best {
            found.push(a.lens);
            found.push(b.lens);
        }
    }
    // the two ends of a scope often share a picture, and a picture may run over the texture's edge, which repeats
    let mut out: Vec<Lens> = Vec::new();
    for lens in found {
        let (w, h) = (size.0 as f32, size.1 as f32);
        let mut wrapped = lens;
        wrapped.circle.x = lens.circle.x.rem_euclid(w);
        wrapped.circle.y = lens.circle.y.rem_euclid(h);
        for l in [wrapped, lens] {
            let c = l.circle;
            let on_texture = c.x + c.radius > 0.0
                && c.y + c.radius > 0.0
                && c.x - c.radius < w
                && c.y - c.radius < h;
            let known = out.iter().any(|o| {
                (o.circle.x - c.x).hypot(o.circle.y - c.y) < o.circle.radius.max(c.radius)
            });
            if on_texture && !known {
                out.push(l);
            }
        }
    }
    out
}

/// How many skins a model's first level of detail draws with.
pub fn skins_used(mesh: &ModelMesh) -> usize {
    let mut seen: Vec<u32> = mesh
        .draws
        .iter()
        .filter(|d| d.lod == 0 && d.index_count > 0)
        .map(|d| d.material)
        .collect();
    seen.sort_unstable();
    seen.dedup();
    seen.len()
}

/// How high a scope's lens sits on the gun, at least.
// OBSERVED: the player's own ROM: the hand-found lens of the G36E is at 0.94 and the L96A1's at 0.86, PSG-1 0.90, Dragunov 0.83, L115A2 0.84;
// the G36E's next round face is at 0.73
const SCOPE_MIN_HEIGHT: f32 = 0.8;
/// How much smaller than the largest a second lens may be: the far end of the same scope, and not a screw on it.
const SCOPE_MIN_SHARE: f32 = 0.5;

/// The lenses of a gun the game says has a sight: its round faces need telling apart from a screw, a knob or the end of a stock, which are
/// round too and lie anywhere. A gun without a sight has none, whatever is round on it: looks alone do not tell a lens, by measure, as a real
/// one on the L96A1 and a stock's end on the AK-47 are drawn much alike.
pub fn scope_lenses(found: &[Lens], has_scope: bool) -> Vec<Lens> {
    if !has_scope {
        return Vec::new();
    }
    let high: Vec<Lens> = found
        .iter()
        .filter(|l| l.height >= SCOPE_MIN_HEIGHT)
        .copied()
        .collect();
    let largest = high.iter().map(|l| l.circle.radius).fold(0.0, f32::max);
    high.into_iter()
        .filter(|l| l.circle.radius >= largest * SCOPE_MIN_SHARE)
        .collect()
}

#[cfg(test)]
mod scope_cap_tests {
    use super::*;

    /// A flat rectangle looking along z, at `z`, `wide` by `high`, centred at height `y`.
    fn cap(z: f32, y: f32, wide: f32, high: f32, texel: Vec2, ahead: bool) -> Vec<SkinTri> {
        let p = [
            Vec3::new(-wide / 2.0, y - high / 2.0, z),
            Vec3::new(wide / 2.0, y - high / 2.0, z),
            Vec3::new(wide / 2.0, y + high / 2.0, z),
            Vec3::new(-wide / 2.0, y + high / 2.0, z),
        ];
        let q = [
            texel,
            texel + Vec2::new(10.0, 0.0),
            texel + Vec2::new(10.0, 10.0),
            texel + Vec2::new(0.0, 10.0),
        ];
        let n = if ahead { Vec3::Z } else { Vec3::NEG_Z };
        [[0usize, 1, 2], [0, 2, 3]]
            .map(|c| SkinTri {
                points: c.map(|i| p[i]),
                texels: c.map(|i| q[i]),
                normal: n,
                area: wide * high / 2.0,
            })
            .to_vec()
    }

    /// Something long and low that gives a gun its length and height, with no faces looking along it.
    fn body() -> Vec<SkinTri> {
        vec![SkinTri {
            points: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 100.0),
                Vec3::new(1.0, 20.0, 50.0),
            ],
            texels: [Vec2::ZERO, Vec2::X, Vec2::Y],
            normal: Vec3::X,
            area: 1000.0,
        }]
    }

    const TEXTURE: (u32, u32) = (256, 256);

    /// As `cap`, with its picture `side` texels square.
    fn pictured(
        z: f32,
        y: f32,
        wide: f32,
        high: f32,
        at: Vec2,
        side: f32,
        ahead: bool,
    ) -> Vec<SkinTri> {
        let mut t = cap(z, y, wide, high, at, ahead);
        for k in &mut t {
            for q in &mut k.texels {
                *q = at + (*q - at) * (side / 10.0);
            }
        }
        t
    }

    #[test]
    fn a_lens_is_a_large_picture_on_a_small_high_face_and_a_mount_is_not() {
        let mut t = body();
        t.extend(pictured(
            30.0,
            18.0,
            4.0,
            4.0,
            Vec2::new(10.0, 10.0),
            32.0,
            false,
        ));
        // a mount's ring beside it, with a small picture
        t.extend(pictured(
            38.0,
            18.0,
            4.0,
            4.0,
            Vec2::new(90.0, 10.0),
            12.0,
            true,
        ));
        let found = scope_caps(&t, Vec3::Z, TEXTURE, true);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0].circle.x - 26.0).abs() < 1.0, "{found:?}");
        assert!(found[0].circle.radius >= 16.0, "{found:?}");
    }

    #[test]
    fn a_large_picture_low_on_the_gun_or_at_its_end_is_not_a_lens() {
        let mut t = body();
        t.extend(pictured(
            50.0,
            4.0,
            4.0,
            4.0,
            Vec2::new(10.0, 10.0),
            32.0,
            false,
        ));
        t.extend(pictured(
            97.0,
            18.0,
            4.0,
            4.0,
            Vec2::new(90.0, 10.0),
            32.0,
            true,
        ));
        assert!(scope_caps(&t, Vec3::Z, TEXTURE, false).is_empty());
    }

    #[test]
    fn two_ends_sharing_one_picture_are_one_circle() {
        let mut t = body();
        t.extend(pictured(
            30.0,
            18.0,
            4.0,
            4.0,
            Vec2::new(10.0, 10.0),
            32.0,
            false,
        ));
        t.extend(pictured(
            60.0,
            18.0,
            5.0,
            5.0,
            Vec2::new(10.0, 10.0),
            32.0,
            true,
        ));
        assert_eq!(scope_caps(&t, Vec3::Z, TEXTURE, true).len(), 1);
    }

    #[test]
    fn a_picture_over_the_textures_edge_is_kept_on_both_sides_of_it() {
        let mut t = body();
        t.extend(pictured(
            30.0,
            18.0,
            4.0,
            4.0,
            Vec2::new(100.0, 240.0),
            32.0,
            false,
        ));
        let found = scope_caps(&t, Vec3::Z, TEXTURE, true);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found
                .iter()
                .any(|l| (l.circle.y - 256.0).abs() < 1.0 || (l.circle.y - 0.0).abs() < 1.0),
            "{found:?}"
        );
    }

    #[test]
    fn a_reflex_sights_two_windows_are_taken_only_on_a_model_with_one_skin() {
        let mut t = body();
        t.extend(cap(50.0, 19.0, 6.0, 3.0, Vec2::new(10.0, 10.0), false));
        t.extend(cap(55.0, 19.0, 6.0, 3.5, Vec2::new(50.0, 10.0), true));
        assert_eq!(scope_caps(&t, Vec3::Z, TEXTURE, true).len(), 2);
        assert!(
            scope_caps(&t, Vec3::Z, TEXTURE, false).is_empty(),
            "with a skin of its own the sight is not painted"
        );
    }

    #[test]
    fn two_small_faces_off_one_line_are_no_sight() {
        let mut t = body();
        t.extend(cap(50.0, 19.0, 6.0, 3.0, Vec2::new(10.0, 10.0), false));
        let mut other = cap(55.0, 19.0, 6.0, 3.0, Vec2::new(50.0, 10.0), true);
        for k in &mut other {
            for p in &mut k.points {
                p.x += 12.0;
            }
        }
        t.extend(other);
        assert!(scope_caps(&t, Vec3::Z, TEXTURE, true).is_empty());
    }

    fn one_triangle(uv: [[f32; 2]; 3]) -> ModelMesh {
        let v = |pos: [f32; 3], uv: [f32; 2]| dc_render::ModelVertex {
            pos,
            normal: [0.0, 0.0, 1.0],
            uv,
            bones: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
        };
        ModelMesh {
            vertices: vec![
                v([0.0, 0.0, 0.0], uv[0]),
                v([1.0, 0.0, 0.0], uv[1]),
                v([0.0, 1.0, 0.0], uv[2]),
            ],
            indices: vec![0, 1, 2],
            draws: vec![dc_render::PieceDraw {
                piece: 0,
                lod: 0,
                first_index: 0,
                index_count: 3,
                material: 0,
                slots: [0; 4],
                slot_count: 1,
                style: 0,
                priority: 0,
                kind: 0,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_corner_at_the_textures_far_edge_stays_there_and_a_repeat_moves_as_one() {
        let t = skin_triangles(
            &one_triangle([[0.9, 0.9], [1.0, 0.9], [1.0, 1.0]]),
            &[],
            0,
            (100, 100),
        );
        assert!(
            (t[0].texels[2] - Vec2::new(100.0, 100.0)).length() < 1e-3,
            "{:?}",
            t[0].texels
        );
        let t = skin_triangles(
            &one_triangle([[2.1, -0.9], [2.2, -0.9], [2.2, -0.8]]),
            &[],
            0,
            (100, 100),
        );
        assert!(
            (t[0].texels[0] - Vec2::new(10.0, 10.0)).length() < 1e-3,
            "{:?}",
            t[0].texels
        );
        assert!(
            (t[0].texels[2] - Vec2::new(20.0, 20.0)).length() < 1e-3,
            "{:?}",
            t[0].texels
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_gun_with_a_scope_has_lenses_and_only_its_large_high_round_faces() {
        let lens = |radius: f32, height: f32| Lens {
            circle: Circle {
                x: radius,
                y: 0.0,
                radius,
            },
            roundness: 1.0,
            height,
        };
        let found = [
            lens(80.0, 0.94),
            lens(79.0, 0.73),
            lens(50.0, 0.93),
            lens(13.0, 0.9),
        ];
        let kept = scope_lenses(&found, true);
        let radii: Vec<f32> = kept.iter().map(|l| l.circle.radius).collect();
        assert_eq!(radii, vec![80.0, 50.0]);
        assert!(scope_lenses(&found, false).is_empty());
        assert!(scope_lenses(&[], true).is_empty());
    }

    /// A round fan of `n` triangles at `at`, looking along z, drawn on the texture round `texel`.
    fn disc(at: Vec3, radius: f32, texel: Vec2, texel_radius: f32, n: usize) -> Vec<SkinTri> {
        let ring = |k: usize| {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            (a.cos(), a.sin())
        };
        (0..n)
            .map(|k| {
                let ((c0, s0), (c1, s1)) = (ring(k), ring(k + 1));
                let points = [
                    at,
                    at + Vec3::new(c0, s0, 0.0) * radius,
                    at + Vec3::new(c1, s1, 0.0) * radius,
                ];
                let texels = [
                    texel,
                    texel + Vec2::new(c0, s0) * texel_radius,
                    texel + Vec2::new(c1, s1) * texel_radius,
                ];
                let cross = (points[1] - points[0]).cross(points[2] - points[0]);
                SkinTri {
                    points,
                    texels,
                    normal: cross.normalize(),
                    area: cross.length() * 0.5,
                }
            })
            .collect()
    }

    /// A long body along z, from 0 to 100, low down, so a gun has a length and a height.
    fn body() -> Vec<SkinTri> {
        let quad = |a: Vec3, b: Vec3, c: Vec3| {
            let cross = (b - a).cross(c - a);
            SkinTri {
                points: [a, b, c],
                texels: [
                    Vec2::new(1.0, 1.0),
                    Vec2::new(50.0, 1.0),
                    Vec2::new(1.0, 50.0),
                ],
                normal: cross.normalize(),
                area: cross.length() * 0.5,
            }
        };
        vec![
            quad(
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, 100.0),
                Vec3::new(0.0, 10.0, 0.0),
            ),
            quad(
                Vec3::new(0.0, 10.0, 100.0),
                Vec3::new(0.0, 0.0, 100.0),
                Vec3::new(0.0, 10.0, 0.0),
            ),
        ]
    }

    #[test]
    fn a_scope_lens_is_found_where_its_texture_is() {
        let mut tris = body();
        tris.extend(disc(
            Vec3::new(0.0, 9.0, 40.0),
            2.0,
            Vec2::new(900.0, 120.0),
            80.0,
            12,
        ));
        let found = find_lenses(&tris, Vec3::Z);
        assert_eq!(found.len(), 1, "{found:?}");
        let c = found[0].circle;
        assert!(
            (c.x - 900.0).abs() < 2.0 && (c.y - 120.0).abs() < 2.0,
            "{c:?}"
        );
        assert!((c.radius - 82.0).abs() < 3.0, "{c:?}");
        assert!(found[0].roundness > 0.9);
    }

    #[test]
    fn the_muzzle_a_low_part_a_bolt_head_and_a_square_are_not_lenses() {
        for (name, disc_tris) in [
            (
                "muzzle",
                disc(
                    Vec3::new(0.0, 9.0, 99.9),
                    2.0,
                    Vec2::new(500.0, 500.0),
                    40.0,
                    12,
                ),
            ),
            (
                "low",
                disc(
                    Vec3::new(0.0, 1.0, 40.0),
                    2.0,
                    Vec2::new(500.0, 500.0),
                    40.0,
                    12,
                ),
            ),
            (
                "few",
                disc(
                    Vec3::new(0.0, 9.0, 40.0),
                    2.0,
                    Vec2::new(500.0, 500.0),
                    40.0,
                    4,
                ),
            ),
            (
                "huge",
                disc(
                    Vec3::new(0.0, 9.0, 40.0),
                    30.0,
                    Vec2::new(500.0, 500.0),
                    40.0,
                    12,
                ),
            ),
        ] {
            let mut tris = body();
            tris.extend(disc_tris);
            assert!(find_lenses(&tris, Vec3::Z).is_empty(), "{name}");
        }
    }

    #[test]
    fn a_lens_stretched_on_the_texture_is_not_round() {
        let mut d = disc(
            Vec3::new(0.0, 9.0, 40.0),
            2.0,
            Vec2::new(500.0, 500.0),
            40.0,
            12,
        );
        for t in &mut d {
            for q in &mut t.texels {
                q.x = 500.0 + (q.x - 500.0) * 3.0;
            }
        }
        let mut tris = body();
        tris.extend(d);
        assert!(find_lenses(&tris, Vec3::Z).is_empty());
    }

    #[test]
    fn front_and_back_of_a_scope_sharing_a_circle_count_once() {
        let mut tris = body();
        tris.extend(disc(
            Vec3::new(0.0, 9.0, 30.0),
            2.0,
            Vec2::new(300.0, 300.0),
            60.0,
            12,
        ));
        tris.extend(disc(
            Vec3::new(0.0, 9.0, 50.0),
            2.0,
            Vec2::new(300.0, 300.0),
            60.0,
            12,
        ));
        tris.extend(disc(
            Vec3::new(0.0, 9.5, 60.0),
            1.0,
            Vec2::new(700.0, 300.0),
            30.0,
            12,
        ));
        let found = find_lenses(&tris, Vec3::Z);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].circle.radius > found[1].circle.radius);
    }

    #[test]
    fn parts_join_at_corners_on_the_model_and_the_texture() {
        let tris = disc(Vec3::ZERO, 1.0, Vec2::new(10.0, 10.0), 5.0, 6);
        assert_eq!(parts(&tris).len(), 1);
        let mut two = tris.clone();
        two.extend(disc(
            Vec3::new(50.0, 0.0, 0.0),
            1.0,
            Vec2::new(90.0, 90.0),
            5.0,
            6,
        ));
        assert_eq!(parts(&two).len(), 2);
        // the same place on the model, but cut apart on the texture
        let mut cut = tris.clone();
        cut.extend(disc(Vec3::ZERO, 1.0, Vec2::new(90.0, 90.0), 5.0, 6));
        assert_eq!(parts(&cut).len(), 2);
    }
}
