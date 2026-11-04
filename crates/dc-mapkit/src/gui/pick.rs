//! Picking for the editor viewport, with no egui or GPU types: a ray against the level polygons
//! ([`crate::build::level_of`]: terrain, solids, imports), against one solid's faces (so a click names the solid
//! AND the face the asset browser assigns to), and the nearest projected object marker in screen space.
// DC-DEFAULT: MAP-64 (click to select: markers first within a pixel radius, then the nearest solid face)

use super::camera::{cross, dot, sub, Ray, V};
use crate::geom::{self, Poly};
use crate::project::Solid;

/// Plane-side tolerance of the inside test, world units.
pub const EPS: f32 = 0.05;

/// A hit on a polygon list.
#[derive(Debug, Clone, PartialEq)]
pub struct PolyHit {
    /// Distance along the ray.
    pub t: f32,
    pub point: V,
    /// Index into the list that was searched.
    pub index: usize,
}

/// A hit on a solid.
#[derive(Debug, Clone, PartialEq)]
pub struct SolidHit {
    pub id: String,
    /// The face name of [`geom::solid_faces`] (`top`, `north`, `side_3`, ...).
    pub face: String,
    pub t: f32,
    pub point: V,
}

fn v(p: geom::V3) -> V {
    [p.x, p.y, p.z]
}

/// Distance along `ray` to the convex polygon `corners` (any winding) lying on the plane with normal `n`, or
/// `None` when the ray misses it, runs parallel or the polygon is behind the eye.
pub fn ray_convex(ray: &Ray, corners: &[V], n: V) -> Option<f32> {
    if corners.len() < 3 {
        return None;
    }
    let denom = dot(n, ray.dir);
    if denom.abs() < 1e-9 {
        return None;
    }
    let d = dot(n, corners[0]);
    let t = (d - dot(n, ray.origin)) / denom;
    if t <= 0.0 {
        return None;
    }
    let p = ray.at(t);
    // inside a convex polygon: every edge turns the same way around p
    let mut sign = 0.0f32;
    for i in 0..corners.len() {
        let a = corners[i];
        let b = corners[(i + 1) % corners.len()];
        let s = dot(cross(sub(b, a), sub(p, a)), n);
        if s.abs() <= EPS {
            continue;
        }
        if sign == 0.0 {
            sign = s.signum();
        } else if s.signum() != sign {
            return None;
        }
    }
    Some(t)
}

/// The nearest polygon of `polys` along the ray (both sides of every polygon count: the terrain is one-sided
/// but a camera under it still wants to pick).
pub fn pick_polys(polys: &[Poly], ray: &Ray) -> Option<PolyHit> {
    let mut best: Option<PolyHit> = None;
    for (i, poly) in polys.iter().enumerate() {
        let corners: Vec<V> = poly.corners.iter().map(|c| v(*c)).collect();
        if let Some(t) = ray_convex(ray, &corners, v(poly.plane.n)) {
            if best.as_ref().is_none_or(|b| t < b.t) {
                best = Some(PolyHit {
                    t,
                    point: ray.at(t),
                    index: i,
                });
            }
        }
    }
    best
}

/// The nearest face of any solid along the ray.
pub fn pick_solids(solids: &[Solid], ray: &Ray) -> Option<SolidHit> {
    let mut best: Option<SolidHit> = None;
    for s in solids {
        let Ok(faces) = geom::solid_faces(s) else {
            continue;
        };
        for f in faces {
            let corners: Vec<V> = f.corners.iter().map(|c| v(*c)).collect();
            if let Some(t) = ray_convex(ray, &corners, v(f.outward)) {
                if best.as_ref().is_none_or(|b| t < b.t) {
                    best = Some(SolidHit {
                        id: s.id.clone(),
                        face: f.name.clone(),
                        t,
                        point: ray.at(t),
                    });
                }
            }
        }
    }
    best
}

/// The index of the marker nearest to `at` within `radius` pixels (the nearest wins; a tie keeps the first).
pub fn pick_marker(markers: &[[f32; 2]], at: [f32; 2], radius: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, m) in markers.iter().enumerate() {
        let d2 = (m[0] - at[0]).powi(2) + (m[1] - at[1]).powi(2);
        if d2 <= radius * radius && best.is_none_or(|(_, b)| d2 < b) {
            best = Some((i, d2));
        }
    }
    best.map(|(i, _)| i)
}

/// Axis-aligned world box of a solid (its rotated corners' bounds), for the selection outline.
pub fn solid_box(s: &Solid) -> Option<(V, V)> {
    let faces = geom::solid_faces(s).ok()?;
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for f in &faces {
        for c in &f.corners {
            let c = v(*c);
            for a in 0..3 {
                lo[a] = lo[a].min(c[a]);
                hi[a] = hi[a].max(c[a]);
            }
        }
    }
    (lo[0] <= hi[0]).then_some((lo, hi))
}

/// The 12 edges of a box.
pub fn box_edges(lo: V, hi: V) -> [(V, V); 12] {
    let c = |x: bool, y: bool, z: bool| -> V {
        [
            if x { hi[0] } else { lo[0] },
            if y { hi[1] } else { lo[1] },
            if z { hi[2] } else { lo[2] },
        ]
    };
    [
        (c(false, false, false), c(true, false, false)),
        (c(false, false, true), c(true, false, true)),
        (c(false, true, false), c(true, true, false)),
        (c(false, true, true), c(true, true, true)),
        (c(false, false, false), c(false, false, true)),
        (c(true, false, false), c(true, false, true)),
        (c(false, true, false), c(false, true, true)),
        (c(true, true, false), c(true, true, true)),
        (c(false, false, false), c(false, true, false)),
        (c(true, false, false), c(true, true, false)),
        (c(false, false, true), c(false, true, true)),
        (c(true, false, true), c(true, true, true)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{level_of, Assets};
    use crate::gui::camera::FlyCamera;
    use crate::project::SolidKind;
    use crate::template::{make, Template};

    fn down_at(x: f32, z: f32) -> Ray {
        Ray {
            origin: [x, 5000.0, z],
            dir: [0.0, -1.0, 0.0],
        }
    }

    #[test]
    fn a_ray_meets_a_convex_polygon_of_either_winding_and_misses_outside() {
        let quad = [
            [0.0, 0.0, 0.0],
            [100.0, 0.0, 0.0],
            [100.0, 0.0, 100.0],
            [0.0, 0.0, 100.0],
        ];
        let up = [0.0, 1.0, 0.0];
        let t = ray_convex(&down_at(50.0, 50.0), &quad, up).unwrap();
        assert!((t - 5000.0).abs() < 1e-3);
        let mut rev = quad;
        rev.reverse();
        assert!(ray_convex(&down_at(50.0, 50.0), &rev, up).is_some());
        assert!(ray_convex(&down_at(150.0, 50.0), &quad, up).is_none());
        assert!(ray_convex(&down_at(50.0, 50.0), &quad, [0.0, -1.0, 0.0]).is_some());
        // parallel and behind
        let flat = Ray {
            origin: [50.0, 10.0, -50.0],
            dir: [0.0, 0.0, 1.0],
        };
        assert!(ray_convex(&flat, &quad, up).is_none());
        let away = Ray {
            origin: [50.0, 10.0, 50.0],
            dir: [0.0, 1.0, 0.0],
        };
        assert!(ray_convex(&away, &quad, up).is_none());
    }

    #[test]
    fn picking_the_starter_level_finds_the_floor_and_a_solid_face() {
        let made = make(Template::Starter, "pick", "", 7);
        let dir = std::env::temp_dir().join(format!("mapkit-pick-{}", std::process::id()));
        let level = level_of(&made.project, &made.heights, &Assets::new(&dir)).unwrap();
        assert!(!level.polys.is_empty());
        // straight down at a spawn: the floor under it is where ground snapping put it (STAND_OFFSET above)
        let spawn = made
            .project
            .objects
            .iter()
            .find(|o| o.class == crate::vocab_names::CLASS_PLAYER_SPAWN)
            .unwrap();
        let hit = pick_polys(&level.polys, &down_at(spawn.pos[0], spawn.pos[2])).expect("floor");
        assert!(
            (hit.point[1] - (spawn.pos[1] - crate::project::STAND_OFFSET)).abs() < 1.0,
            "floor {} under spawn {:?}",
            hit.point[1],
            spawn.pos
        );
        // the camera's own ray through the pixel of that floor point hits the same point
        let cam = FlyCamera::default();
        let (px, _) = cam.project(hit.point, 1000.0, 700.0).unwrap();
        let ray = cam.unproject(px[0], px[1], 1000.0, 700.0);
        let again = pick_polys(&level.polys, &ray).expect("hit");
        assert!(
            crate::gui::camera::len(sub(again.point, hit.point)) < 2.0,
            "{:?} vs {:?}",
            again.point,
            hit.point
        );
        // a level solid: its top face from above
        let solid = made
            .project
            .solids
            .iter()
            .find(|s| matches!(s.kind, SolidKind::Box) && s.brush.is_none() && s.size[1] < 300.0)
            .expect("a box");
        let hit = pick_solids(&made.project.solids, &down_at(solid.pos[0], solid.pos[2])).unwrap();
        // the sky lid is above everything else; whatever is hit first is a top face
        assert_eq!(hit.face, "top");
        let only = [solid.clone()];
        let hit = pick_solids(&only, &down_at(solid.pos[0], solid.pos[2])).unwrap();
        assert_eq!(hit.id, solid.id);
        assert!((hit.point[1] - (solid.pos[1] + solid.size[1] * 0.5)).abs() < 1e-2);
        let (lo, hi) = solid_box(solid).unwrap();
        assert!(lo[1] < hi[1]);
        assert_eq!(box_edges(lo, hi).len(), 12);
    }

    #[test]
    fn the_nearest_marker_within_the_radius_wins() {
        let markers = [[10.0, 10.0], [50.0, 50.0], [52.0, 50.0]];
        assert_eq!(pick_marker(&markers, [51.5, 50.0], 12.0), Some(2));
        assert_eq!(pick_marker(&markers, [12.0, 12.0], 12.0), Some(0));
        assert_eq!(pick_marker(&markers, [200.0, 200.0], 12.0), None);
        assert_eq!(pick_marker(&[], [0.0, 0.0], 12.0), None);
    }
}
