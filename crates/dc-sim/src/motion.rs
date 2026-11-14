//! Moving the body through the world: slide loop, stair stepping, the move-with-collision driver and the
//! ground probe that refreshes the standing contact.
// SPEC: MOVEMENT_2009.md §9.3-§9.5, §9.7, §10.1

use crate::collide::{Collider, Hit, SURF_NO_STEP};
use crate::fmath::abs;
use crate::shape::Cylinder;
use crate::vec3::V3;

/// The surface the body stands on (the engine's standing contact: object +0x164 / polygon +0x168).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    pub normal: V3,
    pub surface_flags: u32,
    pub model: u32,
    pub poly: u32,
}

/// The standing contact is the hit polygon; its normal is the polygon's plane normal (what the integrator and
/// the mover's ground state read), not the edge / cap normal of the contact.
// EVIDENCE: engine+0x79210 (the contact stored is the object and polygon of the probe hit)
// SPEC: MOVEMENT_2009.md §8.3 (n = plane normal of the standing polygon), §10.2 (standing plane normal y)
impl From<Hit> for Contact {
    fn from(h: Hit) -> Contact {
        Contact {
            normal: h.face_normal,
            surface_flags: h.surface_flags,
            model: h.model,
            poly: h.poly,
        }
    }
}

/// Result of a slide or stair move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveOut {
    pub pos: V3,
    /// Whether any sweep of the move hit something.
    pub hit: bool,
    /// Whether a hit surface carried the "no step" flag.
    pub no_step: bool,
}

/// Slide loop: sweep, clip against the first plane, then slide along the crease of a second plane; nudge
/// out when stuck on the same plane without progress.
// SPEC: MOVEMENT_2009.md §9.5 (engine+0x78650)
pub fn slide_move<W: Collider + ?Sized>(w: &W, shape: &Cylinder, p0: V3, e0: V3) -> MoveOut {
    let d0 = e0 - p0;
    let l0 = d0.len();
    let mut remaining = l0;
    let mut p = p0;
    let mut e = e0;
    let mut planes = 0i32;
    let mut tsum = 0.0f32;
    let mut last_n = V3::ZERO;
    let mut n0 = V3::ZERO;
    let mut out = MoveOut {
        pos: e0,
        hit: false,
        no_step: false,
    };
    loop {
        let Some(h) = w.sweep(shape, p, e) else {
            out.pos = e;
            return out;
        };
        out.hit = true;
        if h.surface_flags & SURF_NO_STEP != 0 {
            out.no_step = true;
        }
        let mut d = e - p;
        let c = p + d * h.t;
        if planes == 0 || (h.normal - last_n).len_sq() > 1e-6 || h.t != 0.0 || tsum != 0.0 {
            last_n = h.normal;
            if planes == 0 {
                n0 = h.normal;
                let dn = n0.dot(d);
                if dn < 0.0 {
                    // clip against the first plane with a 0.1 % push-out
                    d += n0 * abs(dn * 1.001);
                }
            } else {
                if planes != 1 || abs(h.normal.dot(n0)) >= 0.999 {
                    out.pos = c;
                    return out;
                }
                let crease = n0.cross(h.normal).normalized();
                d = crease * crease.dot(d);
            }
            d *= 1.0 - h.t;
            tsum += h.t;
            if d.len_sq() < 0.0001 || planes == 4 || (e - p0).dot(d0) < 0.0 {
                out.pos = c;
                return out;
            }
            p = c;
            e = c + d;
        } else {
            // same plane again with no progress: nudge out along its normal
            let nudge = l0 * 0.25;
            if remaining < nudge {
                out.pos = c;
                return out;
            }
            remaining -= nudge;
            p += h.normal * nudge;
            e += h.normal * nudge;
            planes -= 1;
        }
        planes += 1;
        if planes > 4 {
            out.pos = e;
            return out;
        }
    }
}

/// Stair variant of the slide: no walking up steep slopes, and a raised retry that steps over obstacles up
/// to `step` high.
// SPEC: MOVEMENT_2009.md §9.7 (step = STEP_HEIGHT + 0.5)
// EVIDENCE: engine+0x78c50 (slope projection when the probe below hits a surface with 0 < n.y < 0.7071 that
// the move does not leave and the projected move still climbs; the landing is judged by the hit polygon's
// plane normal; a steep landing after a free raised slide is retried once along the slope's horizontal uphill
// direction from the raised start, and taken when that probe is free or lands flat)
pub fn stair_move<W: Collider + ?Sized>(
    w: &W,
    shape: &Cylinder,
    p0: V3,
    e0: V3,
    standing: bool,
    step: f32,
    standable: f32,
) -> MoveOut {
    let h = shape.half_height;
    let r = shape.radius;
    let mut d = e0 - p0;
    // 1. on the ground: project a climb into a steep slope onto the slope's level line
    if standing {
        let low = if e0.y < p0.y { e0.y } else { p0.y };
        let down = V3::new(p0.x, low - h * 0.1, p0.z);
        if let Some(hit) = w.sweep(shape, p0, down) {
            let n = hit.normal;
            let dn = n.dot(d);
            if dn <= 0.0 && n.y < standable && 0.0 < n.y && 0.0 < d.y - dn * n.y {
                let level = V3::new(n.z, 0.0, -n.x).normalized();
                d = level * level.dot(d);
            }
        }
    }
    // 2. plain slide, then the raised retry
    let r1 = slide_move(w, shape, p0, p0 + d);
    if !r1.hit || r1.no_step {
        return r1;
    }
    let up = p0 + V3::new(0.0, step, 0.0);
    if w.sweep(shape, p0, up).is_some() {
        return r1;
    }
    let r2 = slide_move(w, shape, up, up + d);
    let low_end = V3::new(r2.pos.x, r2.pos.y - step, r2.pos.z);
    let below = w.sweep(shape, r2.pos, low_end);
    let landing = match below {
        None => low_end,
        Some(hh) => V3::new(r2.pos.x, (low_end.y - r2.pos.y) * hh.t + r2.pos.y, r2.pos.z),
    };
    let take = match below {
        None => true,
        Some(hh) if hh.face_normal.y >= standable => true,
        Some(hh) if !r2.hit => {
            // steep landing: one probe from the raised start along the slope's horizontal uphill direction
            let nf = hh.face_normal;
            let uphill = V3::new(-(nf.y * nf.x), 0.0, -(nf.z * nf.y)).normalized();
            let reach = (1.0 - hh.normal.y * hh.normal.y) * step * 2.0 + 0.1;
            let dist = if 0.5 * r <= reach { reach } else { 0.5 * r };
            match w.sweep(shape, up, r2.pos + uphill * dist) {
                None => true,
                Some(ph) => ph.face_normal.y >= standable,
            }
        }
        Some(_) => false,
    };
    if take {
        let mut prog1 = (r1.pos - p0).dot(d);
        if prog1 < 0.0 {
            prog1 = 0.0;
        }
        if (landing - p0).dot(d) > prog1 * 1.01 {
            return MoveOut {
                pos: landing,
                hit: true,
                no_step: r1.no_step || r2.no_step,
            };
        }
    }
    r1
}

/// Depth of the ground probe as a fraction of the half height (one unit in the last place above 0.02).
// DISASM: engine+0x79225 (mulss [0x67b3ac], bits 0x3ca3d70b)
const PROBE_DEPTH: f32 = f32::from_bits(0x3ca3_d70b);
/// Radius share of the second ground probe's reach (bits 0x3de147af).
// DISASM: engine+0x79210 (constant at VA 0x67b3a8)
const PROBE_RIM: f32 = f32::from_bits(0x3de1_47af);

/// Ground probe after a move: refreshes or clears the standing contact.
// SPEC: MOVEMENT_2009.md §10.1
// EVIDENCE: engine+0x79210 (sweep down by 0.02 h; a polygon whose plane normal y is below 0.7071 under a body
// that stood on a standable polygon: second probe from step height along the horizontal uphill direction,
// nothing there keeps the old contact, a hit becomes the contact; otherwise a contact normal y below 0.001
// clears, anything else becomes the contact)
pub fn ground_probe<W: Collider + ?Sized>(
    w: &W,
    shape: &Cylinder,
    pos: V3,
    contact: &mut Option<Contact>,
    step: f32,
    standable: f32,
) {
    let h = shape.half_height;
    let r = shape.radius;
    let depth = h * PROBE_DEPTH;
    let Some(hit) = w.sweep(shape, pos, V3::new(pos.x, pos.y - depth, pos.z)) else {
        *contact = None;
        return;
    };
    if hit.face_normal.y < standable && contact.is_some_and(|c| c.normal.y >= standable) {
        let nf = hit.face_normal;
        let uphill = V3::new(-(nf.y * nf.x), 0.0, -(nf.z * nf.y)).normalized();
        let reach =
            (1.0 - hit.normal.y * hit.normal.y) * (hit.t * depth + step) * 2.0 + r * PROBE_RIM;
        let dist = if 0.5 * r <= reach { reach } else { 0.5 * r };
        let from = V3::new(pos.x, step + pos.y, pos.z);
        match w.sweep(shape, from, from + uphill * dist) {
            None => {} // nothing there: keep the old contact
            Some(h2) => *contact = Some(Contact::from(h2)),
        }
        return;
    }
    if hit.normal.y < 0.001 {
        *contact = None;
        return;
    }
    *contact = Some(Contact::from(hit));
}

/// Move with collision from `start` to `target` (teleport: no collision test), then refresh the standing
/// contact. Returns the final position.
// SPEC: MOVEMENT_2009.md §9.3 (move_to, flags 1 / 9), §9.4 (engine+0x796e0)
#[allow(clippy::too_many_arguments)]
pub fn sweep_move<W: Collider + ?Sized>(
    w: &W,
    shape: &Cylinder,
    start: V3,
    target: V3,
    teleport: bool,
    contact: &mut Option<Contact>,
    step: f32,
    standable: f32,
) -> V3 {
    let mut result = if teleport || target == start {
        target
    } else if target.x != start.x || target.z != start.z {
        // the player body has the stair-stepping flag (0x40000)
        stair_move(w, shape, start, target, contact.is_some(), step, standable).pos
    } else {
        slide_move(w, shape, start, target).pos
    };
    if result.has_nan() {
        result = start;
    }
    let moved_down = target.y < start.y;
    if !moved_down || result.y > target.y {
        ground_probe(w, shape, result, contact, step, standable);
    } else {
        // move flag bit 0: nothing below after a free downward move
        *contact = None;
    }
    result
}

/// Places a standing box on the floor under a spawn point: lift by `lift` (clears a start that is embedded
/// up to that depth; the lift is skipped when the lifted box would overlap), then sweep down at most
/// `lift + max_drop`. Returns the resting centre, or `at` unchanged when nothing is below.
///
/// The 2009 client does not decide spawn placement: the server teleports the body with the 0xa0 / 0x0020
/// correction. OBSERVED over the ROM (counts only): with the stand box read from the user's player models,
/// 56 of the 1,560 spawn points overlap geometry when the centre is put exactly at the point (some spawn
/// points are closer to the floor than the box's half height); after this placement 12 remain (spawn
/// points that touch thin panels). PROPOSED DC default (staged, not approved): spawn =
/// `drop_to_floor(spawn, lift = step, 1000)`.
// SPEC: MOVEMENT_2009.md §12.4 flag 0x0020 (the server sets position and box)
pub fn drop_to_floor<W: Collider + ?Sized>(
    w: &W,
    shape: &Cylinder,
    at: V3,
    lift: f32,
    max_drop: f32,
) -> V3 {
    let raised = at + V3::new(0.0, lift, 0.0);
    let start = if w.overlaps(shape, raised) {
        at
    } else {
        raised
    };
    let end = start - V3::new(0.0, (start.y - at.y) + max_drop, 0.0);
    match w.sweep(shape, start, end) {
        Some(h) => start + (end - start) * h.t,
        None => at,
    }
}

/// Whether a body at `at` is wedged: it overlaps solid geometry and a 1-unit move is blocked at once along all six
/// axes. Under the 2009 face rules (front sides only, MOVEMENT_2009.md §9.8 / §9.9) a body that overlaps the world
/// can usually still move away; a wedged one cannot, and nothing in the 2009 rules ever frees it.
// DC-DEFAULT: SIM-28 (proposed) the wedge test and its escape
pub fn wedged<W: Collider + ?Sized>(w: &W, shape: &Cylinder, at: V3) -> bool {
    if !w.overlaps_world(shape, at) {
        return false;
    }
    const AXES: [V3; 6] = [
        V3::new(1.0, 0.0, 0.0),
        V3::new(-1.0, 0.0, 0.0),
        V3::new(0.0, 1.0, 0.0),
        V3::new(0.0, -1.0, 0.0),
        V3::new(0.0, 0.0, 1.0),
        V3::new(0.0, 0.0, -1.0),
    ];
    AXES.iter().all(|d| {
        w.sweep_world(shape, at, at + *d)
            .is_some_and(|h| h.t == 0.0)
    })
}

/// The escape spot of a wedged body: along 26 fixed directions (the 6 axes, then the 12 edge diagonals, then the 8
/// corner diagonals, each list in a fixed order), at whole-unit distances from 1 to `max` (nearest distance first),
/// the first spot where the body fits and a 1-unit probe swept from the body's centre to it meets no solid face from
/// its front side. So the body never goes through a wall (a wall's front side stops the probe); it may leave the brush
/// it is in, whose faces it only meets from behind. The probe has a size because a zero-size line slips through the
/// seam where two walls meet (a closed room's corner, unit test `wedge_in_a_closed_room_stays_put`). `None` when no
/// such spot is within `max` (the body stays). The spot must also have a floor below it within [`ESCAPE_FLOOR`] (a
/// ray straight down meets a solid face from its front side), so a body is not freed into the void outside the map
/// or under a one-sided floor. World geometry only: other players' bodies are not looked at.
// DC-DEFAULT: SIM-28 (proposed) 26 directions, 1-unit distance steps, a 1-unit probe free of front faces, a floor
// below within ESCAPE_FLOOR
/// How far below an escape spot a floor must be (SIM-28), in units.
// DC-DEFAULT: SIM-28 (proposed) 2048 units (deeper than any drop to a floor on the 21 ROM worlds' playable areas is
// not needed: the check only tells a floor from the void)
pub const ESCAPE_FLOOR: f32 = 2048.0;

pub fn escape_wedge<W: Collider + ?Sized>(w: &W, shape: &Cylinder, at: V3, max: u32) -> Option<V3> {
    const A: f32 = 0.707_106_77; // 1/sqrt(2)
    const B: f32 = 0.577_350_26; // 1/sqrt(3)
    const DIRS: [V3; 26] = [
        V3::new(1.0, 0.0, 0.0),
        V3::new(-1.0, 0.0, 0.0),
        V3::new(0.0, 1.0, 0.0),
        V3::new(0.0, -1.0, 0.0),
        V3::new(0.0, 0.0, 1.0),
        V3::new(0.0, 0.0, -1.0),
        V3::new(A, A, 0.0),
        V3::new(A, -A, 0.0),
        V3::new(-A, A, 0.0),
        V3::new(-A, -A, 0.0),
        V3::new(A, 0.0, A),
        V3::new(A, 0.0, -A),
        V3::new(-A, 0.0, A),
        V3::new(-A, 0.0, -A),
        V3::new(0.0, A, A),
        V3::new(0.0, A, -A),
        V3::new(0.0, -A, A),
        V3::new(0.0, -A, -A),
        V3::new(B, B, B),
        V3::new(B, B, -B),
        V3::new(B, -B, B),
        V3::new(B, -B, -B),
        V3::new(-B, B, B),
        V3::new(-B, B, -B),
        V3::new(-B, -B, B),
        V3::new(-B, -B, -B),
    ];
    // a cylinder 1 unit across and 1 unit high (bound = sqrt(0.5^2 + 0.5^2))
    const PROBE: Cylinder = Cylinder {
        radius: 0.5,
        half_height: 0.5,
        bound: 0.707_106_77,
    };
    for dist in 1..=max {
        let k = dist as f32;
        for d in &DIRS {
            let c = at + *d * k;
            if !w.overlaps_world(shape, c)
                && w.sweep_world(&PROBE, at, c).is_none()
                && w.sweep_world(&Cylinder::POINT, c, c - V3::new(0.0, ESCAPE_FLOOR, 0.0))
                    .is_some()
            {
                return Some(c);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collide::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID};
    extern crate alloc;
    use alloc::vec;

    fn quad(c: [V3; 4]) -> ConvexPoly {
        ConvexPoly::from_corners(c.to_vec(), SURF_SOLID)
    }

    fn box_top(x0: f32, x1: f32, z0: f32, z1: f32, y: f32) -> ConvexPoly {
        quad([
            V3::new(x0, y, z0),
            V3::new(x0, y, z1),
            V3::new(x1, y, z1),
            V3::new(x1, y, z0),
        ])
    }

    /// Wall in the plane z = z0 facing -z, spanning x and y ranges.
    fn wall_neg_z(x0: f32, x1: f32, y0: f32, y1: f32, z0: f32) -> ConvexPoly {
        quad([
            V3::new(x0, y0, z0),
            V3::new(x0, y1, z0),
            V3::new(x1, y1, z0),
            V3::new(x1, y0, z0),
        ])
    }

    fn world(p: &[ConvexPoly]) -> CollisionWorld {
        CollisionWorld::from_parts(vec![CollModel::from_polys(p)], &[])
    }

    /// A quad whose front faces `toward` (the corner order is flipped when needed).
    fn facing(c: [V3; 4], toward: V3) -> ConvexPoly {
        let q = quad(c);
        if q.normal.dot(toward) > 0.0 {
            q
        } else {
            quad([c[3], c[2], c[1], c[0]])
        }
    }

    /// The six faces of the box `lo..hi`, every front side facing into the box (a closed room).
    fn room(lo: V3, hi: V3) -> Vec<ConvexPoly> {
        let (a, b) = (lo, hi);
        vec![
            facing(
                [
                    V3::new(a.x, a.y, a.z),
                    V3::new(a.x, b.y, a.z),
                    V3::new(a.x, b.y, b.z),
                    V3::new(a.x, a.y, b.z),
                ],
                V3::new(1.0, 0.0, 0.0),
            ),
            facing(
                [
                    V3::new(b.x, a.y, a.z),
                    V3::new(b.x, b.y, a.z),
                    V3::new(b.x, b.y, b.z),
                    V3::new(b.x, a.y, b.z),
                ],
                V3::new(-1.0, 0.0, 0.0),
            ),
            facing(
                [
                    V3::new(a.x, a.y, a.z),
                    V3::new(b.x, a.y, a.z),
                    V3::new(b.x, a.y, b.z),
                    V3::new(a.x, a.y, b.z),
                ],
                V3::new(0.0, 1.0, 0.0),
            ),
            facing(
                [
                    V3::new(a.x, b.y, a.z),
                    V3::new(b.x, b.y, a.z),
                    V3::new(b.x, b.y, b.z),
                    V3::new(a.x, b.y, b.z),
                ],
                V3::new(0.0, -1.0, 0.0),
            ),
            facing(
                [
                    V3::new(a.x, a.y, a.z),
                    V3::new(b.x, a.y, a.z),
                    V3::new(b.x, b.y, a.z),
                    V3::new(a.x, b.y, a.z),
                ],
                V3::new(0.0, 0.0, 1.0),
            ),
            facing(
                [
                    V3::new(a.x, a.y, b.z),
                    V3::new(b.x, a.y, b.z),
                    V3::new(b.x, b.y, b.z),
                    V3::new(a.x, b.y, b.z),
                ],
                V3::new(0.0, 0.0, -1.0),
            ),
        ]
    }

    /// SIM-28: a body in a closed room smaller than itself is wedged; every way out crosses a wall's front side, so
    /// the escape finds nothing and the body stays (never through a wall).
    #[test]
    fn wedge_in_a_closed_room_stays_put() {
        let w = world(&room(
            V3::new(-10.0, -10.0, -10.0),
            V3::new(10.0, 10.0, 10.0),
        ));
        assert!(w.overlaps(&SHAPE, V3::ZERO));
        assert!(wedged(&w, &SHAPE, V3::ZERO));
        assert_eq!(escape_wedge(&w, &SHAPE, V3::ZERO, 96), None);
    }

    /// SIM-28: no geometry, or a wall the body overlaps but can back away from, is no wedge.
    #[test]
    fn wedge_needs_every_axis_blocked() {
        assert!(!wedged(&world(&[]), &SHAPE, V3::ZERO));
        // a wall at z = 5 facing -z: the body (radius 16) overlaps it, but can still move away along -z
        let w = world(&[wall_neg_z(-100.0, 100.0, -100.0, 100.0, 5.0)]);
        assert!(w.overlaps(&SHAPE, V3::ZERO));
        assert!(!wedged(&w, &SHAPE, V3::ZERO));
    }

    /// SIM-28: the escape takes the nearest fitting spot, in the fixed direction order, along a clear centre line.
    #[test]
    fn wedge_escape_takes_the_nearest_clear_spot() {
        // the room's +x wall is missing: the body walks out of the open side, 30 units along +x is the first fit; a
        // floor far below outside makes the exit a real place, not the void
        let mut faces = room(V3::new(-10.0, -10.0, -10.0), V3::new(10.0, 10.0, 10.0));
        faces.remove(1);
        faces.push(box_top(-500.0, 500.0, -500.0, 500.0, -300.0));
        let w = world(&faces);
        let to = escape_wedge(&w, &SHAPE, V3::ZERO, 96).expect("out of the open side");
        assert!(!w.overlaps(&SHAPE, to));
        assert!(w.sweep(&Cylinder::POINT, V3::ZERO, to).is_none());
        // and the same open room is no wedge for a body that can walk out of it
        assert!(!wedged(&w, &SHAPE, V3::ZERO));
        assert_eq!(
            (to.y, to.z),
            (0.0, 0.0),
            "straight out along the first free axis: {to:?}"
        );
        assert!(to.x > 0.0 && to.x <= 30.0, "{to:?}");
    }

    /// SIM-28: the same open side with nothing below it is the void: no escape there, the body stays.
    #[test]
    fn wedge_escape_needs_a_floor_below() {
        let mut faces = room(V3::new(-10.0, -10.0, -10.0), V3::new(10.0, 10.0, 10.0));
        faces.remove(1);
        let w = world(&faces);
        assert_eq!(escape_wedge(&w, &SHAPE, V3::ZERO, 96), None);
    }

    /// SIM-28: another player's body is not world geometry: overlapping it is never a wedge.
    #[test]
    fn wedge_ignores_other_players() {
        use crate::collide::{Body, BodySet, WithBodies};
        let empty = world(&[]);
        let mut bodies = BodySet::default();
        assert!(bodies.push(Body {
            id: 7,
            centre: V3::ZERO,
            half: V3::new(16.0, 40.0, 16.0),
        }));
        let w = WithBodies {
            world: &empty,
            bodies: &bodies,
        };
        assert!(
            w.overlaps(&SHAPE, V3::ZERO),
            "the body is in the other player's box"
        );
        assert!(!wedged(&w, &SHAPE, V3::ZERO));
    }

    const STANDABLE: f32 = crate::params::MovementParams::DEFAULT.standable_cos;

    const SHAPE: Cylinder = Cylinder {
        radius: 16.0,
        half_height: 40.0,
        bound: 43.081_32,
    };

    #[test]
    fn slide_along_a_wall_keeps_the_tangential_part() {
        let w = world(&[
            box_top(-500.0, 500.0, -500.0, 500.0, 0.0),
            wall_neg_z(-500.0, 500.0, 0.0, 300.0, 100.0),
        ]);
        let p0 = V3::new(0.0, 40.1, 80.0);
        let r = slide_move(&w, &SHAPE, p0, p0 + V3::new(50.0, 0.0, 50.0));
        assert!(r.hit);
        assert!(r.pos.z <= 84.0, "{:?}", r.pos);
        assert!(r.pos.x > 45.0, "slid along x: {:?}", r.pos);
    }

    #[test]
    fn step_up_a_20_unit_ledge_but_not_a_40_unit_one() {
        for (height, climbs) in [(20.0f32, true), (30.0, true), (31.0, false), (40.0, false)] {
            let floor = box_top(-500.0, 500.0, -500.0, 500.0, 0.0);
            let riser = wall_neg_z(-500.0, 500.0, 0.0, height, 100.0);
            let top = box_top(-500.0, 500.0, 100.0, 500.0, height);
            let w = world(&[floor, riser, top]);
            let mut contact = Some(Contact {
                normal: V3::UP,
                surface_flags: SURF_SOLID,
                model: 0,
                poly: 0,
            });
            let mut pos = V3::new(0.0, 40.1, 60.0);
            for _ in 0..20 {
                let target = pos + V3::new(0.0, 0.0, 5.0);
                pos = sweep_move(
                    &w,
                    &SHAPE,
                    pos,
                    target,
                    false,
                    &mut contact,
                    30.5,
                    STANDABLE,
                );
                if contact.is_none() {
                    // fall back onto whatever is below
                    pos = sweep_move(
                        &w,
                        &SHAPE,
                        pos,
                        pos - V3::new(0.0, 5.0, 0.0),
                        false,
                        &mut contact,
                        30.5,
                        STANDABLE,
                    );
                }
            }
            let on_top = pos.z > 100.0 && pos.y > height + 39.0;
            assert_eq!(on_top, climbs, "height {height}: {pos:?}");
        }
    }

    #[test]
    fn ground_probe_finds_and_loses_the_floor() {
        let w = world(&[box_top(-100.0, 100.0, -100.0, 100.0, 0.0)]);
        let mut c = None;
        ground_probe(&w, &SHAPE, V3::new(0.0, 40.1, 0.0), &mut c, 30.5, STANDABLE);
        assert_eq!(c.map(|c| c.normal), Some(V3::UP));
        ground_probe(&w, &SHAPE, V3::new(0.0, 45.0, 0.0), &mut c, 30.5, STANDABLE);
        assert_eq!(c, None, "0.02 h = 0.8 below finds nothing from 5 units up");
    }

    /// Standing with the rim on the side edge of a standable ramp: the probe meets the edge (cap contact,
    /// normal straight up) but the contact carries the ramp's plane normal, as the engine stores the polygon.
    // EVIDENCE: engine+0x79210 (the standing contact is the hit polygon), engine+0x76120 (cap contact normal)
    #[test]
    fn standing_contact_carries_the_polygon_plane() {
        // a ramp rising along +z (30 degrees), x in [0, 100]
        let rise = 100.0 * 0.577_350_3;
        let ramp = quad([
            V3::new(0.0, 0.0, 0.0),
            V3::new(0.0, rise, 100.0),
            V3::new(100.0, rise, 100.0),
            V3::new(100.0, 0.0, 0.0),
        ]);
        let n = ramp.normal;
        assert!(n.y > 0.8 && n.z < 0.0, "{n:?}");
        let w = world(&[ramp]);
        // centre 10 units beyond the ramp's x = 0 side edge at z = 50: the disc reaches over the edge up to
        // z = 50 + sqrt(16^2 - 10^2), where the edge is highest; the rim rests 0.05 above that point
        let reach = (16.0f32 * 16.0 - 10.0 * 10.0).sqrt();
        let y = (50.0 + reach) / 100.0 * rise + SHAPE.half_height + 0.05;
        assert!(!w.overlaps(&SHAPE, V3::new(-10.0, y, 50.0)));
        let mut c = None;
        ground_probe(&w, &SHAPE, V3::new(-10.0, y, 50.0), &mut c, 30.5, STANDABLE);
        let c = c.expect("standing on the edge");
        assert!((c.normal - n).len() < 1e-6, "{:?} vs {n:?}", c.normal);
    }

    #[test]
    fn drop_to_floor_clears_a_shallow_embedding() {
        let w = world(&[box_top(-100.0, 100.0, -100.0, 100.0, 0.0)]);
        // centre 36 above the floor with h = 40: 4 units embedded
        let p = drop_to_floor(&w, &SHAPE, V3::new(0.0, 36.0, 0.0), 30.5, 1000.0);
        assert!((p.y - 40.1).abs() < 1e-3, "{p:?}");
        assert!(!w.overlaps(&SHAPE, p));
        // from high above it drops to the floor too
        let p = drop_to_floor(&w, &SHAPE, V3::new(0.0, 300.0, 0.0), 30.5, 1000.0);
        assert!((p.y - 40.1).abs() < 1e-3, "{p:?}");
    }

    #[test]
    fn teleport_ignores_walls() {
        let w = world(&[wall_neg_z(-500.0, 500.0, 0.0, 300.0, 100.0)]);
        let mut c = None;
        let p = sweep_move(
            &w,
            &SHAPE,
            V3::new(0.0, 50.0, 0.0),
            V3::new(0.0, 50.0, 200.0),
            true,
            &mut c,
            30.5,
            STANDABLE,
        );
        assert_eq!(p, V3::new(0.0, 50.0, 200.0));
    }
}
