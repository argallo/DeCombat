//! Vertex-colour baking (MAPKIT_PLAN.md 5.3 step 2, D6): the world's light is baked into the render vertices;
//! no lightmaps in v1. Kind-1 sections draw `texture * colour * 2`, so a colour byte of 0x80 is unit brightness
//! (RENDER_2009.md §5.2 as read by dc-render; MAPKIT_PLAN.md section 2).
//!
//! The rule (DC-DEFAULT: MAP-44), per polygon corner with the polygon's plane normal `n`:
//!
//! ```text
//! light_c = bake.ambient * ambient_c / 255
//!         + bake.sun * sun_bright * sun_color_c / 255 * max(0, dot(n, -sun_dir))
//!         + bake.hemisphere * (0.5 + 0.5 * n.y)
//! light_c *= 1 - bake.ao * occlusion(p)
//! byte_c  = clamp(round(light_c * 128), 0, 255)       (1.0 = 0x80 = neutral)
//! colour  = 0xFF000000 | r << 16 | g << 8 | b          (ARGB, alpha 0xFF)
//! ```
//!
//! `occlusion(p)` is a cheap terrain ambient occlusion from height differences: the mean over eight compass
//! directions at `2 * cell_size` of `clamp((terrain height there - p.y) / distance, 0, 1)`; 0 without terrain
//! or off the grid (a vertex on open flat ground or above its surroundings is unoccluded). Every term is a fixed
//! sequence of f32 operations (no transcendental), so the bytes are the same on every platform.

use crate::geom::{Poly, V3};
use crate::project::{Bake, Heights, Terrain, WorldSettings};

/// Everything the baker reads.
#[derive(Debug, Clone, Copy)]
pub struct BakeInput<'a> {
    pub settings: &'a WorldSettings,
    pub bake: &'a Bake,
    pub terrain: Option<(&'a Terrain, &'a Heights)>,
    /// What casts shadows when `bake.shadow` is above 0 (see [`Occluder::of`]).
    pub occluders: &'a [Occluder],
}

/// One polygon that casts a shadow, with its bounds for a quick refusal.
#[derive(Debug, Clone)]
pub struct Occluder {
    lo: V3,
    hi: V3,
    corners: Vec<V3>,
}

impl Occluder {
    pub fn of(p: &Poly) -> Occluder {
        let (mut lo, mut hi) = (p.corners[0], p.corners[0]);
        for c in &p.corners {
            lo = lo.min(*c);
            hi = hi.max(*c);
        }
        Occluder {
            lo,
            hi,
            corners: p.corners.clone(),
        }
    }

    /// Whether the segment from `o` along `d` (length `len`) crosses the polygon. Fixed f32 steps, no hashing.
    fn hit(&self, o: V3, d: V3, len: f32) -> bool {
        let e = (o + d * len).min(o);
        let f = (o + d * len).max(o);
        if e.x > self.hi.x
            || e.y > self.hi.y
            || e.z > self.hi.z
            || f.x < self.lo.x
            || f.y < self.lo.y
            || f.z < self.lo.z
        {
            return false;
        }
        for i in 1..self.corners.len() - 1 {
            let (a, b, c) = (self.corners[0], self.corners[i], self.corners[i + 1]);
            let (e1, e2) = (b - a, c - a);
            let h = V3::new(
                d.y * e2.z - d.z * e2.y,
                d.z * e2.x - d.x * e2.z,
                d.x * e2.y - d.y * e2.x,
            );
            let det = e1.dot(h);
            if det.abs() < 1e-6 {
                continue;
            }
            let s = o - a;
            let u = s.dot(h) / det;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = V3::new(
                s.y * e1.z - s.z * e1.y,
                s.z * e1.x - s.x * e1.z,
                s.x * e1.y - s.y * e1.x,
            );
            let v = d.dot(q) / det;
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            let t = e2.dot(q) / det;
            if t > 0.0 && t < len {
                return true;
            }
        }
        false
    }
}

/// How far a ray to the sun looks (world units), and how far the contact shadow reaches.
const SUN_REACH: f32 = 4096.0;
const CONTACT_REACH: f32 = 220.0;
/// The sun's disc: rays around its direction, so that a shadow's edge is soft.
const SUN_SPREAD: [[f32; 3]; 5] = [
    [0.0, 0.0, 0.0],
    [0.05, 0.0, 0.0],
    [-0.05, 0.0, 0.0],
    [0.0, 0.0, 0.05],
    [0.0, 0.0, -0.05],
];
/// Low rays around a corner: what is close by darkens the corner, whichever way the sun comes from.
const CONTACT_RAYS: [[f32; 3]; 8] = [
    [0.8, 0.6, 0.0],
    [-0.8, 0.6, 0.0],
    [0.0, 0.6, 0.8],
    [0.0, 0.6, -0.8],
    [0.566, 0.6, 0.566],
    [0.566, 0.6, -0.566],
    [-0.566, 0.6, 0.566],
    [-0.566, 0.6, -0.566],
];

/// `(share of the sun's disc in view, share of the contact rays blocked)` at a corner.
fn shade(input: &BakeInput<'_>, n: V3, p: V3) -> (f32, f32) {
    let start = p + n * 2.0;
    let blocked = |d: V3, len: f32| input.occluders.iter().any(|o| o.hit(start, d, len));
    let sun = sun_towards(input.settings);
    let mut lit = 0;
    for s in SUN_SPREAD {
        let d = (sun + V3::from_array(s)).normalized().unwrap_or(sun);
        if !blocked(d, SUN_REACH) {
            lit += 1;
        }
    }
    let mut near = 0;
    for r in CONTACT_RAYS {
        // a ray that points into its own surface says nothing about the things around it
        if V3::from_array(r).dot(n) > 0.0 && blocked(V3::from_array(r), CONTACT_REACH) {
            near += 1;
        }
    }
    (
        lit as f32 / SUN_SPREAD.len() as f32,
        near as f32 / CONTACT_RAYS.len() as f32,
    )
}

/// The unit vector towards the sun (`-sun_dir`), or straight up for a zero vector.
pub fn sun_towards(settings: &WorldSettings) -> V3 {
    (-V3::from_array(settings.sun_dir))
        .normalized()
        .unwrap_or(V3::UP)
}

/// Terrain ambient occlusion in [0, 1] at a world point (0 without terrain).
pub fn occlusion(terrain: Option<(&Terrain, &Heights)>, p: V3) -> f32 {
    let Some((t, h)) = terrain else {
        return 0.0;
    };
    let d = 2.0 * t.cell_size;
    if d <= 0.0 || !d.is_finite() {
        return 0.0;
    }
    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;
    const DIRS: [[f32; 2]; 8] = [
        [1.0, 0.0],
        [-1.0, 0.0],
        [0.0, 1.0],
        [0.0, -1.0],
        [S, S],
        [S, -S],
        [-S, S],
        [-S, -S],
    ];
    let mut sum = 0.0f32;
    for dir in DIRS {
        if let Some(y) = h.sample(t, p.x + dir[0] * d, p.z + dir[1] * d) {
            sum += ((y - p.y) / d).clamp(0.0, 1.0);
        }
    }
    sum / DIRS.len() as f32
}

/// One channel's light before the AO factor.
fn channel(b: &Bake, s: &WorldSettings, ambient: u8, sun: u8, n_dot_l: f32, hemi: f32) -> f32 {
    b.ambient * (ambient as f32 / 255.0)
        + b.sun * s.sun_bright * (sun as f32 / 255.0) * n_dot_l
        + b.hemisphere * hemi
}

fn byte(v: f32) -> u32 {
    (v * 128.0).round().clamp(0.0, 255.0) as u32
}

/// The ARGB colour of a corner with normal `n` at `p`.
pub fn colour_at(input: &BakeInput<'_>, n: V3, p: V3) -> u32 {
    shaded_colour(input, n, p, (1.0, 0.0))
}

/// [`colour_at`] with the result of [`shade`].
fn shaded_colour(input: &BakeInput<'_>, n: V3, p: V3, shade: (f32, f32)) -> u32 {
    let s = input.settings;
    let b = input.bake;
    let n_dot_l = n.dot(sun_towards(s)).max(0.0) * (1.0 - b.shadow * (1.0 - shade.0));
    let hemi = 0.5 + 0.5 * n.y;
    let ao = (1.0 - b.ao * occlusion(input.terrain, p)) * (1.0 - b.shadow * 0.5 * shade.1);
    let r = byte(channel(b, s, s.ambient[0], s.sun_color[0], n_dot_l, hemi) * ao);
    let g = byte(channel(b, s, s.ambient[1], s.sun_color[1], n_dot_l, hemi) * ao);
    let bl = byte(channel(b, s, s.ambient[2], s.sun_color[2], n_dot_l, hemi) * ao);
    0xFF00_0000 | (r << 16) | (g << 8) | bl
}

/// Colours of every corner of a polygon (flat normal from its plane).
pub fn bake_poly(input: &BakeInput<'_>, poly: &Poly) -> Vec<u32> {
    poly.corners
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let n = poly.corner_normals.as_ref().and_then(|n| n.get(i)).copied();
            // a model's corners keep plain light: its own simple core would shade them in patches
            if input.bake.shadow > 0.0 && n.is_none() && !input.occluders.is_empty() {
                let at = shade(input, poly.plane.n, *c);
                shaded_colour(input, poly.plane.n, *c, at)
            } else {
                colour_at(input, n.unwrap_or(poly.plane.n), *c)
            }
        })
        .collect()
}

/// Colours of every corner of every polygon.
pub fn bake_polys(input: &BakeInput<'_>, polys: &[Poly]) -> Vec<Vec<u32>> {
    polys.iter().map(|p| bake_poly(input, p)).collect()
}

#[cfg(test)]
mod shadow_tests {
    use super::*;

    #[test]
    fn a_floor_is_darker_under_a_core_and_a_model_is_left_alone() {
        let s = WorldSettings {
            sun_dir: [0.0, -1.0, 0.0],
            ..Default::default()
        };
        let b = Bake {
            lightmap: 0.0,
            shadow: 1.0,
            ..Default::default()
        };
        // a roof at height 50 over x, z in -10 .. 10
        let roof = Poly::new(
            vec![
                V3::new(-10.0, 50.0, -10.0),
                V3::new(10.0, 50.0, -10.0),
                V3::new(10.0, 50.0, 10.0),
                V3::new(-10.0, 50.0, 10.0),
            ],
            V3::new(0.0, -1.0, 0.0),
            crate::geom::Surface::default(),
            None,
        )
        .unwrap();
        let occluders = [Occluder::of(&roof)];
        let input = BakeInput {
            settings: &s,
            bake: &b,
            terrain: None,
            occluders: &occluders,
        };
        let floor = |x: f32| {
            Poly::new(
                vec![
                    V3::new(x, 0.0, 0.0),
                    V3::new(x + 1.0, 0.0, 0.0),
                    V3::new(x, 0.0, 1.0),
                ],
                V3::UP,
                crate::geom::Surface::default(),
                None,
            )
            .unwrap()
        };
        let red = |c: u32| c >> 16 & 0xFF;
        let (mut under, open) = (floor(0.0), floor(500.0));
        let dark = bake_poly(&input, &under)[0];
        let lit = bake_poly(&input, &open)[0];
        assert!(red(dark) < red(lit), "{dark:x} {lit:x}");
        under.corner_normals = Some(vec![under.plane.n; 3]);
        assert_eq!(
            bake_poly(&input, &under)[0],
            lit,
            "a model's corners keep plain light"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Plane, Surface};
    use crate::project::TerrainLayer;

    fn quad(n: V3, y: f32) -> Poly {
        let s = 10.0;
        Poly::new(
            vec![
                V3::new(-s, y, -s),
                V3::new(s, y, -s),
                V3::new(s, y, s),
                V3::new(-s, y, s),
            ],
            n,
            Surface::default(),
            None,
        )
        .unwrap()
    }

    #[test]
    fn sunlit_up_face_is_brighter_than_a_down_face_and_neutral_is_0x80() {
        let s = WorldSettings {
            ambient: [255; 3],
            sun_color: [255; 3],
            sun_dir: [0.0, -1.0, 0.0],
            ..Default::default()
        };
        let b = Bake {
            lightmap: 0.0,
            shadow: 0.0,
            ambient: 0.5,
            sun: 0.5,
            hemisphere: 0.0,
            ao: 0.0,
        };
        let input = BakeInput {
            occluders: &[],
            settings: &s,
            bake: &b,
            terrain: None,
        };
        let up = bake_poly(&input, &quad(V3::UP, 0.0));
        assert_eq!(up.len(), 4);
        // 0.5 + 0.5 * 1 = 1.0 -> 0x80 per channel, alpha 0xFF
        assert!(up.iter().all(|c| *c == 0xFF80_8080), "{up:x?}");
        let down = bake_poly(&input, &quad(-V3::UP, 0.0));
        assert!(down.iter().all(|c| *c == 0xFF40_4040), "{down:x?}");
        let flipped = Poly {
            plane: Plane { n: V3::UP, d: 0.0 },
            ..quad(V3::UP, 0.0)
        };
        assert_eq!(bake_poly(&input, &flipped), up);
    }

    #[test]
    fn terrain_occlusion_darkens_a_pit_and_leaves_a_peak_alone() {
        let t = Terrain {
            cells_x: 8,
            cells_z: 8,
            cell_size: 10.0,
            origin: [0.0, 0.0, 0.0],
            height_scale: 100.0,
            layers: vec![TerrainLayer::default()],
            ..Default::default()
        };
        let mut h = Heights::for_terrain(&t, 65535); // 100 high everywhere
        h.set(4, 4, 0); // a pit at (40, 40)
        let pit = occlusion(Some((&t, &h)), V3::new(40.0, 0.0, 40.0));
        assert!(pit > 0.9, "{pit}");
        let rim = occlusion(Some((&t, &h)), V3::new(40.0, 100.0, 40.0));
        assert_eq!(rim, 0.0);
        let peak = occlusion(Some((&t, &h)), V3::new(40.0, 500.0, 40.0));
        assert_eq!(peak, 0.0);
        assert_eq!(occlusion(None, V3::ZERO), 0.0);
        let s = WorldSettings::default();
        let b = Bake {
            ao: 1.0,
            ..Bake::default()
        };
        let lit = colour_at(
            &BakeInput {
                occluders: &[],
                settings: &s,
                bake: &b,
                terrain: Some((&t, &h)),
            },
            V3::UP,
            V3::new(40.0, 0.0, 40.0),
        );
        let open = colour_at(
            &BakeInput {
                occluders: &[],
                settings: &s,
                bake: &b,
                terrain: Some((&t, &h)),
            },
            V3::UP,
            V3::new(40.0, 100.0, 40.0),
        );
        assert!(lit & 0xFF_FFFF < open & 0xFF_FFFF);
        assert_eq!(lit >> 24, 0xFF);
    }
}
