//! Line of sight and a simple way-finder for the shooter, sans-IO, on the collision world the bot loaded from the
//! user's ROM (`dc-sim`).
//!
//! The 2009 client traces its shot against the world: a target behind a wall is never reported as hit. The
//! reference traces too (capture botduel1: every report through a wall came back as a single impact record ending at
//! the wall, no damage). So the shooter fires only along a clear line, and a hunting bot walks toward its target
//! until the line is clear: each decision probes the bot's own body a short way ahead in the goal direction and
//! in directions turned further and further to one side, takes the first free one, and flips the side when it
//! made no progress for a while (a "feeler" way-finder; DeCombat bot behaviour, not a 2009 rule).
// SPEC: MOVEMENT_2009.md §9 (collision shape and sweep, through dc-sim); GAME_MESSAGES_2009_C2S.md §4.1 (the
// client's own trace result is what 0x82 reports)
// OBSERVED: capture 20261025_010543_botduel1 (reference impact records end at the wall)

use dc_net_engine::time::{later, since};
use dc_net_engine::Millis;
use std::collections::BTreeMap;

use dc_sim::{Collider, Cylinder, V3};

/// Half extents of the probe used for the line of sight (a thin ray).
pub const RAY_DIMS: [f32; 3] = [1.0, 1.0, 1.0];
/// The way-finder: every [`DECIDE_MS`] it tries [`DIRECTIONS`] headings on a copy of the body for [`PROBE_SECS`]
/// and takes the one whose end point is closest to the goal, plus a penalty for every earlier decision taken in
/// that end point's [`CELL`] (so a dead end fills up and the bot walks out of it); headings that barely move are
/// skipped.
// DC-DEFAULT: BOT-17 (proposed) way-finder: 12 headings, 0.5 s probes, a decision every 400 ms, 64-unit cells,
// 40 units of penalty per visit, a heading is usable when the probe moves at least 30 units, a new heading must
// score 64 units better than the current one
pub const DIRECTIONS: usize = 12;
pub const PROBE_SECS: f32 = 0.5;
pub const PROBE_FREE: f32 = 30.0;
pub const DECIDE_MS: u32 = 400;
pub const CELL: f32 = 64.0;
pub const VISIT_PENALTY: f32 = 40.0;
/// A goal this far from the last one starts a fresh search (the visited places are forgotten).
pub const NEW_GOAL_DIST: f32 = 512.0;
/// A new heading must score this much better than the current one.
pub const HYSTERESIS: f32 = 64.0;
/// Stuck detection: a bot that moved less than [`STUCK_PROGRESS`] in [`STUCK_MS`], or whose distance to the goal did
/// not drop by [`STALL_GAIN`] in [`STALL_MS`] (sliding along a wall the goal lies behind), is stuck: it flips its
/// side and follows the obstacle on that side ("sidestep") for up to [`SIDESTEP_MS`], until it is [`STALL_GAIN`]
/// closer to the goal than where it got stuck.
// DC-DEFAULT: BOT-52 (proposed) stuck = under 24 units in 1 s or no 64-unit gain toward the goal in 3 s; then a
// wall-following sidestep for at most 8 s on the other side than the last time
pub const STUCK_MS: u32 = 1000;
pub const STUCK_PROGRESS: f32 = 24.0;
pub const STALL_MS: u32 = 3000;
pub const STALL_GAIN: f32 = 64.0;
pub const SIDESTEP_MS: u32 = 8000;
/// Penalty per unit a heading's probe falls short of the freest heading's (a heading into a wall scores worse
/// than one along it).
// DC-DEFAULT: BOT-52 (proposed) 0.5 per unit short of the freest probe
pub const BLOCKED_WEIGHT: f32 = 0.5;
/// While sidestepping, a heading is free when its probe moves this share of the freest probe.
pub const SIDESTEP_FREE: f32 = 0.7;

/// True when the straight line from `from` to `to` meets no world geometry.
pub fn line_clear(world: &dyn Collider, from: [f32; 3], to: [f32; 3]) -> bool {
    let shape = Cylinder::from_dims(V3::from_array(RAY_DIMS));
    world
        .sweep(&shape, V3::from_array(from), V3::from_array(to))
        .is_none_or(|h| h.t >= 0.999)
}

/// Yaw of the horizontal direction from `a` to `b` (the +Z-forward convention of the aim).
pub(crate) fn yaw_to(a: [f32; 3], b: [f32; 3]) -> f32 {
    (b[0] - a[0]).atan2(b[2] - a[2])
}

fn flat_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn cell(p: [f32; 3]) -> (i32, i32, i32) {
    (
        (p[0] / CELL).floor() as i32,
        (p[1] / (2.0 * CELL)).floor() as i32,
        (p[2] / CELL).floor() as i32,
    )
}

/// The way-finder state.
#[derive(Debug, Clone, Default)]
pub struct Hunter {
    yaw: Option<f32>,
    next_decision: Millis,
    visits: BTreeMap<(i32, i32, i32), u32>,
    progress_at: Option<(Millis, [f32; 3])>,
    /// Best distance to the goal so far and when it was reached (the stall check).
    best: Option<(Millis, f32)>,
    /// Sidestep in progress: until when, and the goal distance where it started.
    sidestep: Option<(Millis, f32)>,
    /// The side the obstacle is followed on (+1 / -1), flipped at every stuck.
    side: f32,
    /// Stuck detections (reported as `hunt_stuck`); each one starts a sidestep.
    pub flips: u32,
    pub decisions: u32,
}

impl Hunter {
    /// Stops walking (the target is in sight, or there is none).
    pub fn stop(&mut self) {
        self.yaw = None;
        self.progress_at = None;
        self.best = None;
        self.sidestep = None;
    }

    /// True while the hunter follows an obstacle after it got stuck.
    pub fn sidestepping(&self) -> bool {
        self.sidestep.is_some()
    }

    /// Forgets the visited cells (a new life or a new goal).
    pub fn reset(&mut self) {
        self.stop();
        self.visits.clear();
    }

    /// The walking direction toward `goal` for a body at `pos`. `probe(yaw)` is where the body would be after
    /// [`PROBE_SECS`] in that direction (`None` = no world: walk straight).
    pub fn steer(
        &mut self,
        now: Millis,
        pos: [f32; 3],
        goal: [f32; 3],
        probe: Option<&dyn Fn(f32) -> [f32; 3]>,
    ) -> Option<f32> {
        let direct = yaw_to(pos, goal);
        let Some(probe) = probe else {
            self.yaw = Some(direct);
            return self.yaw;
        };
        if self.yaw.is_some() && later(self.next_decision, now) {
            return self.yaw;
        }
        self.next_decision = now.wrapping_add(DECIDE_MS);
        self.decisions += 1;
        *self.visits.entry(cell(pos)).or_default() += 1;
        let mut stuck = false;
        match self.progress_at {
            Some((t, p)) if since(now, t) >= STUCK_MS => {
                stuck |= flat_dist(p, pos) < STUCK_PROGRESS;
                self.progress_at = Some((now, pos));
            }
            None => self.progress_at = Some((now, pos)),
            _ => {}
        }
        let d = flat_dist(pos, goal);
        if let Some((until, d0)) = self.sidestep {
            if !later(until, now) || d < d0 - STALL_GAIN {
                self.sidestep = None;
                self.best = None;
            }
        }
        // the stall check runs only while heading for the goal (a sidestep may lead away from it)
        if self.sidestep.is_none() {
            match self.best {
                Some((_, b)) if d < b - STALL_GAIN => self.best = Some((now, d)),
                Some((t, _)) if since(now, t) >= STALL_MS => stuck = true,
                None => self.best = Some((now, d)),
                _ => {}
            }
        }
        if stuck {
            self.flips += 1;
            self.side = if self.side > 0.0 { -1.0 } else { 1.0 };
            self.sidestep = Some((now.wrapping_add(SIDESTEP_MS), d));
            self.best = None;
        }
        let step = std::f32::consts::TAU / DIRECTIONS as f32;
        // every heading's probe: (yaw, end point, distance moved)
        let base = if self.sidestep.is_some() {
            // the hand on the obstacle: from a quarter turn toward the followed side, turning away from it
            (self.yaw.unwrap_or(direct) + self.side * std::f32::consts::FRAC_PI_2)
                .rem_euclid(std::f32::consts::TAU)
        } else {
            direct
        };
        let sign = if self.sidestep.is_some() {
            -self.side
        } else {
            1.0
        };
        let cands: Vec<(f32, [f32; 3], f32)> = (0..DIRECTIONS)
            .map(|k| {
                let y = base + sign * step * k as f32;
                let e = probe(y);
                (y, e, flat_dist(e, pos))
            })
            .collect();
        let reach = cands.iter().map(|c| c.2).fold(0.0f32, f32::max);
        if self.sidestep.is_some() {
            let free = (SIDESTEP_FREE * reach).max(PROBE_FREE);
            self.yaw = Some(
                cands
                    .iter()
                    .find(|c| c.2 >= free)
                    .map_or(direct + std::f32::consts::PI, |c| c.0),
            );
            return self.yaw;
        }
        let score = |e: [f32; 3], moved: f32| {
            let v = self.visits.get(&cell(e)).copied().unwrap_or(0) as f32;
            flat_dist(e, goal) + VISIT_PENALTY * v + BLOCKED_WEIGHT * (reach - moved)
        };
        let best = cands
            .iter()
            .filter(|c| c.2 >= PROBE_FREE)
            .map(|c| (score(c.1, c.2), c.0))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        // keep the current heading unless another is clearly better (no flapping between two headings)
        let keep = self.yaw.and_then(|y| {
            let e = probe(y);
            let moved = flat_dist(e, pos);
            (moved >= PROBE_FREE).then(|| (score(e, moved), y))
        });
        let pick = match (keep, best) {
            (Some(k), Some(b)) if k.0 <= b.0 + HYSTERESIS => Some(k),
            (_, b) => b.or(keep),
        };
        self.yaw = Some(pick.map_or(direct + std::f32::consts::PI, |b| b.1));
        self.yaw
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::motion::{Body, MotionKind};
    use dc_sim::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID};
    use std::sync::Arc;

    /// A closed box (6 outward faces) from `lo` to `hi`.
    pub(crate) fn solid_box(lo: [f32; 3], hi: [f32; 3]) -> CollModel {
        let p = |x: f32, y: f32, z: f32| V3::new(x, y, z);
        let (a, b) = (lo, hi);
        let faces = vec![
            // -x, +x
            ConvexPoly::from_corners(
                vec![
                    p(a[0], a[1], a[2]),
                    p(a[0], a[1], b[2]),
                    p(a[0], b[1], b[2]),
                    p(a[0], b[1], a[2]),
                ],
                SURF_SOLID,
            ),
            ConvexPoly::from_corners(
                vec![
                    p(b[0], a[1], a[2]),
                    p(b[0], b[1], a[2]),
                    p(b[0], b[1], b[2]),
                    p(b[0], a[1], b[2]),
                ],
                SURF_SOLID,
            ),
            // -z, +z
            ConvexPoly::from_corners(
                vec![
                    p(a[0], a[1], a[2]),
                    p(a[0], b[1], a[2]),
                    p(b[0], b[1], a[2]),
                    p(b[0], a[1], a[2]),
                ],
                SURF_SOLID,
            ),
            ConvexPoly::from_corners(
                vec![
                    p(a[0], a[1], b[2]),
                    p(b[0], a[1], b[2]),
                    p(b[0], b[1], b[2]),
                    p(a[0], b[1], b[2]),
                ],
                SURF_SOLID,
            ),
            // -y, +y
            ConvexPoly::from_corners(
                vec![
                    p(a[0], a[1], a[2]),
                    p(b[0], a[1], a[2]),
                    p(b[0], a[1], b[2]),
                    p(a[0], a[1], b[2]),
                ],
                SURF_SOLID,
            ),
            ConvexPoly::from_corners(
                vec![
                    p(a[0], b[1], a[2]),
                    p(a[0], b[1], b[2]),
                    p(b[0], b[1], b[2]),
                    p(b[0], b[1], a[2]),
                ],
                SURF_SOLID,
            ),
        ];
        CollModel::from_polys(&faces)
    }

    /// A floor at y = 0 and a wall across the straight line from (0, 40, 0) to (0, 40, 1200).
    pub(crate) fn walled() -> Arc<CollisionWorld> {
        let models = vec![
            solid_box([-5000.0, -64.0, -5000.0], [5000.0, 0.0, 5000.0]),
            solid_box([-400.0, 0.0, 580.0], [400.0, 300.0, 620.0]),
        ];
        Arc::new(CollisionWorld::from_parts(models, &[]))
    }

    /// A floor and a cup open toward -z around the start (0, 40, 0): a back wall at z 300..340 across x -500..500 and
    /// side walls from z -500 to 340 at x = +-500. The goal (0, 40, 1500) lies behind the back wall, so the way out
    /// starts away from the goal.
    pub(crate) fn cupped() -> Arc<CollisionWorld> {
        let models = vec![
            solid_box([-5000.0, -64.0, -5000.0], [5000.0, 0.0, 5000.0]),
            solid_box([-540.0, 0.0, 300.0], [540.0, 300.0, 340.0]),
            solid_box([-540.0, 0.0, -500.0], [-500.0, 300.0, 340.0]),
            solid_box([500.0, 0.0, -500.0], [540.0, 300.0, 340.0]),
        ];
        Arc::new(CollisionWorld::from_parts(models, &[]))
    }

    /// Hunts from `start` toward `goal` on `w` for `secs` (60 Hz body, the session's order: steer, then advance);
    /// returns the time of the first clear line from the eye to the goal (if any), the end position and the hunter.
    pub(crate) fn hunt_run(
        w: &Arc<CollisionWorld>,
        start: [f32; 3],
        goal: [f32; 3],
        secs: u32,
    ) -> (Option<Millis>, [f32; 3], Hunter) {
        let mut b = Body::new(
            start,
            [16.0, 40.0, 16.0],
            0.0,
            MotionKind::Hunt,
            60,
            Some(w.clone()),
        );
        assert!(b.rom_world, "spawn found the floor");
        let mut h = Hunter::default();
        for step in 0..(60 * secs) {
            let now = (step * 1000 / 60) as Millis;
            let eye = [b.pos()[0], b.pos()[1] + 26.0, b.pos()[2]];
            if line_clear(&**w, eye, [goal[0], goal[1] + 20.0, goal[2]]) {
                return (Some(now), b.pos(), h);
            }
            let probe = |y: f32| b.probe_walk(y, PROBE_SECS);
            b.steer = h.steer(now, b.pos(), goal, Some(&probe));
            if std::env::var("HUNT_DEBUG").is_ok() && step % 24 == 0 {
                println!(
                    "t {now} pos {:?} steer {:?} flips {}",
                    b.pos(),
                    b.steer,
                    h.flips
                );
            }
            b.advance(1_000_000 / 60);
        }
        (None, b.pos(), h)
    }

    #[test]
    fn a_hunter_in_a_cup_backs_out_and_finds_the_goal() {
        let w = cupped();
        let goal = [0.0, 40.0, 1500.0];
        let (seen, end, h) = hunt_run(&w, [0.0, 40.0, 0.0], goal, 20);
        println!(
            "cup: seen {seen:?} end {end:?} dist {:.0} decisions {} flips {}",
            flat_dist(end, goal),
            h.decisions,
            h.flips
        );
        assert!(
            seen.is_some(),
            "no line of sight within 20 s; ended at {end:?}"
        );
    }

    #[test]
    fn no_progress_for_a_second_starts_a_sidestep() {
        // a body that never moves although every probe says it could (pressed into something the probe misses)
        let pos = [0.0, 40.0, 0.0];
        let goal = [0.0, 40.0, 1000.0];
        let probe = |y: f32| [pos[0] + 150.0 * y.sin(), pos[1], pos[2] + 150.0 * y.cos()];
        let mut h = Hunter::default();
        let mut first = None;
        for tick in 0..120u32 {
            let now = (tick * 1000 / 60) as Millis;
            h.steer(now, pos, goal, Some(&probe));
            if h.sidestepping() && first.is_none() {
                first = Some(now);
            }
        }
        let at = first.expect("a sidestep within 2 s");
        assert!(
            (STUCK_MS..STUCK_MS + 2 * DECIDE_MS).contains(&at),
            "sidestep at {at} ms"
        );
        assert_eq!(h.flips, 1);
    }

    #[test]
    fn line_of_sight_is_blocked_by_the_wall() {
        let w = walled();
        assert!(!line_clear(&*w, [0.0, 70.0, 0.0], [0.0, 70.0, 1200.0]));
        assert!(line_clear(&*w, [0.0, 70.0, 0.0], [0.0, 70.0, 500.0]));
        assert!(line_clear(&*w, [600.0, 70.0, 0.0], [600.0, 70.0, 1200.0]));
    }

    #[test]
    fn the_hunter_walks_around_the_wall_until_the_goal_is_in_sight() {
        let w = walled();
        let goal = [0.0, 40.0, 1200.0];
        let mut b = Body::new(
            [0.0, 40.0, 0.0],
            [16.0, 40.0, 16.0],
            0.0,
            MotionKind::Hunt,
            60,
            Some(w.clone()),
        );
        assert!(b.rom_world, "spawn found the floor");
        let mut h = Hunter::default();
        let mut seen_at = None;
        for step in 0..(60 * 40) {
            let now = (step * 1000 / 60) as Millis;
            let eye = [b.pos()[0], b.pos()[1] + 26.0, b.pos()[2]];
            if line_clear(&*w, eye, [goal[0], goal[1] + 20.0, goal[2]]) {
                seen_at = Some((now, b.pos()));
                h.stop();
                break;
            }
            let probe = |y: f32| b.probe_walk(y, PROBE_SECS);
            b.steer = h.steer(now, b.pos(), goal, Some(&probe));
            b.advance(1_000_000 / 60);
            if std::env::var("HUNT_DEBUG").is_ok() && step % 30 == 0 {
                println!(
                    "t {now} pos {:?} steer {:?} stuck {}",
                    b.pos(),
                    b.steer,
                    h.flips
                );
            }
        }
        let (t, p) = seen_at.expect("the hunter found a line of sight within 40 s");
        println!(
            "line of sight at {t} ms from {p:?}, {} decisions, {} flips",
            h.decisions, h.flips
        );
        assert!(
            p[0].abs() > 380.0 || p[2] > 640.0,
            "it went round the wall: {p:?} at {t} ms"
        );
    }
}
