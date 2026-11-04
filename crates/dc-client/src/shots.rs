//! What a shot ray of the own player meets in the world: the level, every world model the server created in a state
//! rays stop at (crates, drums, mode props), with the surface of the polygon it hit.
//!
//! The 2009 client traces each pellet with one engine segment query (cshell+0x23a910): objects included, objects
//! without the ray-hit (0x1000) and solid (0x2000) flags skipped, the hit polygon returned. The engine tests every
//! object on the segment whose first flag word has 0x1000 or 0x2000 (engine+0x40920, query bit 2 set), hands it to
//! the shell's filter (cshell+0x235220: the own character never, characters by their nodes, every other object
//! yes), and traces a world model (object kinds 2 and 9) through its own BSP with a polygon result (engine+0x40650 ->
//! +0x40190 with the polygon variant engine+0x407a0), any other object by its box without a polygon. The shell's
//! impact step (cshell+0x238eb0) names a hit object for the 0x82 only when the hit has NO polygon and the object
//! carries user flag 0x01000000 (the characters): a crate or drum hit is a world-model polygon hit, sent in the
//! 623-bit form without an object (as the real client did for 33 shots into crates in tdm1), and its impact takes
//! the polygon's surface (cshell+0x37f430). So the server, not the client, decides that a breakable was hit (from
//! the ray it receives); the client only stops the pellet there and draws the impact.
//!
//! [`ShotWorld`] joins the collision set (where the movement code's breakables and plain world models are, with
//! the server's flags applied by [`crate::objects::WorldModels::apply`]) and the shot-only models of
//! [`crate::objects::WorldModels::shot_world`] (linked models a ray stops at that the body does not collide with).
// SPEC: WEAPON_FIRE_2009.md §5.3 (segment, impact routine, miss record); GAME_MESSAGES_2009_C2S.md §4.1.1 (world hits
// carry has_hit 0)
// EVIDENCE: cshell+0x23a910 (query: segment, flags 7, the shell filter, the hit to cshell+0x238eb0, a miss to the
// impact record with surface 0x6e); cshell+0x238eb0 (the hit object only without a polygon and with user flag
// 0x01000000; the surface from the polygon, cshell+0x37f430); cshell+0x235220 (filter)
// DISASM: engine+0x10410 (client interface member +0x64 -> the world interface's segment query), engine+0x40970
// (query flags: bit 0 objects, bit 1 skip objects without 0x1000 / 0x2000, bit 2 polygon result), engine+0x40920
// (per object: flag word & 0x3000), engine+0x40650 (filter, then kinds 2 / 9 through the BSP, others by box)
// OBSERVED: capture 20261025_000014_tdm1 (the real 2009 client's rifle shots at the crates
// at 229-270 s: all 623-bit, no hit object; 8,209 0x82 of 112 captures: every hit object a character)
// DC-DEFAULT: CLI-250 (proposed) the shot set; CLI-251 (proposed) a moved model is not traced at its file position

use dc_sim::{Collider, CollisionWorld, Cylinder, Hit, BLOCKER_MODEL, V3};

/// The first flag-word bits that make the 2009 engine test an object on a shot segment (ray-hit, solid).
// DISASM: engine+0x40920 (`test ch, 0x30` on the object's flag word)
pub const RAY_FLAGS: u16 = 0x3000;
/// The ray-hit flag alone.
pub const FLAG_RAYHIT: u16 = 0x1000;

/// The collision set and the shot-only models as one collider for the shot trace. Hits on the shot-only models
/// carry model indices after the collision set's (`main.models.len() + k`).
///
/// It holds no character boxes on purpose: the movement's body set (`dc_sim::collide::BodySet`, which the own
/// mover collides with) is not a shot target, because the shell's segment filter tests characters by their nodes,
/// not by their boxes ([`crate::hitnodes`]).
// EVIDENCE: cshell+0x235220 (filter: the own character never, characters by their nodes)
#[derive(Debug, Clone, Copy)]
pub struct ShotWorld<'a> {
    pub main: &'a CollisionWorld,
    pub extra: Option<&'a CollisionWorld>,
}

impl<'a> ShotWorld<'a> {
    /// The collision set alone.
    pub fn of(main: &'a CollisionWorld) -> ShotWorld<'a> {
        ShotWorld { main, extra: None }
    }
}

impl Collider for ShotWorld<'_> {
    fn sweep(&self, shape: &Cylinder, from: V3, to: V3) -> Option<Hit> {
        let a = self.main.sweep(shape, from, to);
        let base = self.main.models.len() as u32;
        let b = self
            .extra
            .filter(|e| !e.models.is_empty())
            .and_then(|e| e.sweep(shape, from, to))
            .map(|mut h| {
                if h.model != BLOCKER_MODEL {
                    h.model += base;
                }
                h
            });
        match (a, b) {
            (Some(x), Some(y)) => Some(if y.t < x.t { y } else { x }),
            (x, y) => x.or(y),
        }
    }

    fn overlaps(&self, shape: &Cylinder, at: V3) -> bool {
        self.main.overlaps(shape, at) || self.extra.is_some_and(|e| e.overlaps(shape, at))
    }
}

/// The first polygon a unit ray meets within `range` (player-only blockers do not stop shots): the distance and the
/// hit.
// SPEC: FMT_DAT85.md §9.1 (blockers are player clip geometry); MOVEMENT_2009.md §9.6 (ray through the world)
pub fn first_hit(
    world: &dyn Collider,
    origin: [f32; 3],
    dir: [f32; 3],
    range: f32,
) -> Option<(f32, Hit)> {
    let d = V3::from_array(dir);
    let mut from = V3::from_array(origin);
    let to = from + d * range;
    let mut travelled = 0.0f32;
    for _ in 0..8 {
        let seg = (to - from).len();
        if seg.is_nan() || seg <= 1e-3 {
            return None;
        }
        let h = world.sweep(&Cylinder::POINT, from, to)?;
        let dist = h.t.clamp(0.0, 1.0) * seg;
        if h.model != BLOCKER_MODEL {
            return Some((travelled + dist, h));
        }
        let step = dist + 1.0;
        travelled += step;
        from += d * step;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_sim::{CollModel, ConvexPoly, SURF_SOLID};

    /// A wall facing -Z at `z`.
    fn wall(z: f32) -> CollModel {
        let p = ConvexPoly::from_corners(
            vec![
                V3::new(-100.0, -100.0, z),
                V3::new(-100.0, 100.0, z),
                V3::new(100.0, 100.0, z),
                V3::new(100.0, -100.0, z),
            ],
            SURF_SOLID,
        );
        CollModel::from_polys(&[p])
    }

    #[test]
    fn the_nearer_of_both_sets_wins_and_extra_models_are_numbered_after_the_main_ones() {
        let main = CollisionWorld::from_parts(vec![wall(300.0)], &[]);
        let extra = CollisionWorld::from_parts(vec![wall(500.0), wall(200.0)], &[]);
        let o = [0.0, 0.0, 0.0];
        let fwd = [0.0, 0.0, 1.0];
        let (d, h) = first_hit(&ShotWorld::of(&main), o, fwd, 3000.0).unwrap();
        assert!((d - 300.0).abs() < 0.5, "{d}");
        assert_eq!(h.model, 0);
        let both = ShotWorld {
            main: &main,
            extra: Some(&extra),
        };
        let (d, h) = first_hit(&both, o, fwd, 3000.0).unwrap();
        assert!((d - 200.0).abs() < 0.5, "{d}");
        assert_eq!(
            h.model, 2,
            "the second extra model after the one main model"
        );
        assert!(first_hit(&both, o, fwd, 150.0).is_none());
        assert!(first_hit(&both, o, [0.0, 0.0, -1.0], 3000.0).is_none());
    }
}
