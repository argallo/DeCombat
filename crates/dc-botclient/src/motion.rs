//! The bot's body: a `dc-sim` player that stands at its spawn or walks a small circle, stepped at a fixed rate,
//! and turned into the fields of the player update 0x64 (body angles, control word, movement trailer).
//!
//! Collision: the ROM world's collision geometry when the world was loaded from the user's ROM, else a flat floor
//! under the spawn point (BOT-2, proposed).
// SPEC: MOVEMENT_2009.md §12.2 (what the client sends), §12.4 (teleport 0x0020: move_seq, position, box)

use dc_sim::{
    bits, CollModel, Collider, CollisionWorld, Commands, Container, ContainerVolumes, ConvexPoly,
    FixedStep, MovementParams, PlayerState, TickInput, SURF_SOLID, V3,
};

use std::sync::Arc;

use crate::shell::{MoveReport, MoveTrailer};

/// Standing half extents used until the server sends a box (0xa0 flag 0x0020). Made up for the bot, not a ROM
/// value (the 2009 client reads its own from the player model, MOVEMENT_2009.md §9.2).
// DC-DEFAULT: BOT-1 (proposed) bot standing box 16 x 40 x 16 half extents until the server's teleport box
pub const BOT_STAND_DIMS: [f32; 3] = [16.0, 40.0, 16.0];
/// One full turn of the walking circle takes this long (with the walk speed this gives the circle's size).
// DC-DEFAULT: BOT-4 (proposed) walking circle: forward held, yaw turning one full turn per 8 s
pub const CIRCLE_TURN_SECONDS: f32 = 8.0;
/// How far below the spawn point a floor is searched in the loaded world before the flat floor is used.
// DC-DEFAULT: BOT-2 (proposed) flat floor when the loaded world has no floor within 2000 units below the spawn
pub const FLOOR_SEARCH: f32 = 2000.0;
/// View pitch (radians) a walking bot holds on a ladder: up (negative) towards a goal above, down towards one
/// below. On a ladder the acceleration follows the full view direction, so the pitch is what climbs.
// SPEC: MOVEMENT_2009.md §11.4 (look up to climb), §6.4 (on a ladder Fdir = F unnormalised)
// DC-DEFAULT: BOT-61 (proposed) the bot climbs a ladder it walks onto: pitch 1.2 rad towards its goal's height
pub const LADDER_PITCH: f32 = 1.2;
/// A goal less than this far above or below the body centre counts as level: the bot climbs up (a ladder leads
/// somewhere; the top is a floor it can walk on).
// DC-DEFAULT: BOT-61 (proposed)
pub const LADDER_LEVEL_BAND: f32 = 40.0;
/// "Standing on nothing" value of the trailer's standing ids (-1 / -1).
// SPEC: MOVEMENT_2009.md §12.2 ([standing:u64] the standing ids or -1 / -1)
pub const STANDING_NONE: u64 = u64::MAX;

/// What the body does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MotionKind {
    Stand,
    /// Walk forward while turning at a constant rate.
    Circle,
    /// Stand, or walk in the direction [`Body::steer`] gives (the shooter's way-finder, `crate::hunt`).
    Hunt,
}

/// The bot's simulated body.
#[derive(Clone)]
pub struct Body {
    params: MovementParams,
    state: PlayerState,
    clock: FixedStep,
    /// Pitch, yaw, roll (radians).
    pub angles: [f32; 3],
    kind: MotionKind,
    world: Arc<CollisionWorld>,
    /// The ROM world is used (false: flat floor).
    pub rom_world: bool,
    pub ticks: u64,
    /// Walking direction (yaw) set by the way-finder; `None` = stand (Hunt) or the motion's own walk.
    pub steer: Option<f32>,
    /// Paused (BOT-59): stand where it is, no walk, no turn, whatever the motion kind.
    pub frozen: bool,
    /// The world's ladder volumes (none: the body never climbs).
    // DC-DEFAULT: BOT-61 (proposed)
    pub ladders: Option<Arc<ContainerVolumes>>,
    /// Height of the way-finder's goal (the ladder pitch, [`LADDER_PITCH`]); `None` = climb up.
    pub goal_y: Option<f32>,
    /// BOT-92: the evasive move of this tick while the bot engages (strafe, step back, jump, crouch relative
    /// to the look yaw); `None` = none. Applied only while the way-finder does not steer (a walk keeps its line),
    /// the jump also while walking.
    pub evade: Option<crate::evade::Move>,
    touched: Vec<Container>,
}

/// A flat floor at height `y` (half a million units across).
// DC-DEFAULT: BOT-2 (proposed) flat floor under the spawn point when no ROM world was loaded
pub fn flat_floor(y: f32) -> CollisionWorld {
    let e = 500_000.0;
    let c = [
        V3::new(-e, y, -e),
        V3::new(-e, y, e),
        V3::new(e, y, e),
        V3::new(e, y, -e),
    ];
    CollisionWorld::from_parts(
        vec![CollModel::from_polys(&[ConvexPoly::from_corners(
            c.to_vec(),
            SURF_SOLID,
        )])],
        &[],
    )
}

impl Body {
    /// A body at `spawn` (the object's position, the box centre) with half extents `dims`, stepped at `hz`.
    pub fn new(
        spawn: [f32; 3],
        dims: [f32; 3],
        yaw: f32,
        kind: MotionKind,
        hz: u32,
        rom_world: Option<Arc<CollisionWorld>>,
    ) -> Body {
        let params = MovementParams::default();
        let d = V3::from_array(dims);
        let shape = dc_sim::Cylinder::from_dims(d);
        let at = V3::from_array(spawn);
        // a spawn point may sit at floor level: search from half a box above it (from the point itself when that
        // start overlaps geometry). A world without a floor under the spawn point (no geometry there, or none at
        // all) gets the flat floor.
        let above = at + V3::new(0.0, dims[1], 0.0);
        let landing = |w: &CollisionWorld| -> Option<V3> {
            let start = if w.overlaps(&shape, above) { at } else { above };
            let end = start - V3::new(0.0, FLOOR_SEARCH, 0.0);
            w.sweep(&shape, start, end)
                .map(|h| start + (end - start) * h.t)
        };
        let (world, pos, rom) = match rom_world.map(|w| (landing(&w), w)) {
            Some((Some(p), w)) => (w, p, true),
            _ => (Arc::new(flat_floor(spawn[1] - dims[1])), at, false),
        };
        let state = PlayerState::new(pos, d, &params);
        Body {
            params,
            state,
            clock: FixedStep::new(hz),
            angles: [0.0, yaw, 0.0],
            kind,
            world,
            rom_world: rom,
            ticks: 0,
            steer: None,
            frozen: false,
            ladders: None,
            goal_y: None,
            evade: None,
            touched: Vec::new(),
        }
    }

    /// BOT-110: the collision set changed (the server's objects switched world models on or off): the body moves
    /// on the new set from now on, and a standing contact with a model in `gone` is dropped so the body falls.
    pub fn set_world(&mut self, world: Arc<CollisionWorld>, gone: &[u32]) {
        if !self.rom_world {
            return;
        }
        self.world = world;
        self.state.detach_from_models(gone);
    }

    /// BOT-92: the evasive move in force (none while frozen).
    fn evading(&self) -> Option<crate::evade::Move> {
        (!self.frozen).then_some(self.evade).flatten()
    }

    fn input(&self) -> TickInput<'static> {
        let walk = !self.frozen && (self.kind == MotionKind::Circle || self.steer.is_some());
        let ev = self.evading();
        // the strafe, the step back and the crouch hold the ground while nothing steers; the jump goes either way
        let side = ev.filter(|_| self.steer.is_none());
        TickInput {
            commands: Commands {
                forward: walk,
                backward: side.is_some_and(|e| e.back),
                left: side.is_some_and(|e| e.strafe < 0 && !e.back),
                right: side.is_some_and(|e| e.strafe > 0 && !e.back),
                jump: ev.is_some_and(|e| e.jump),
                crouch: side.is_some_and(|e| e.crouch),
                // the way-finder runs, as players do (control word RUN 0x200, OBSERVED tdm4 0xa01 while walking)
                run: (self.kind == MotionKind::Hunt && self.steer.is_some()) || side.is_some(),
                ..Commands::default()
            },
            yaw: match side {
                Some(e) => e.yaw,
                None => self.steer.unwrap_or(self.angles[1]),
            },
            pitch: if walk && self.state.on_ladder {
                self.ladder_pitch()
            } else {
                self.angles[0]
            },
            ctrl_state: 1,
            ..TickInput::default()
        }
    }

    /// The walking pitch on a ladder: up unless the goal is clearly below the body.
    // DC-DEFAULT: BOT-61 (proposed)
    fn ladder_pitch(&self) -> f32 {
        match self.goal_y {
            Some(g) if g < self.state.pos.y - LADDER_LEVEL_BAND => LADDER_PITCH,
            _ => -LADDER_PITCH,
        }
    }

    /// Advances by `elapsed_us` of wall time (whole fixed steps only).
    pub fn advance(&mut self, elapsed_us: u64) {
        let n = self.clock.advance(elapsed_us);
        let dt = self.clock.dt();
        for _ in 0..n {
            if self.kind == MotionKind::Circle && !self.frozen {
                self.angles[1] = (self.angles[1]
                    + std::f32::consts::TAU * dt / CIRCLE_TURN_SECONDS)
                    % std::f32::consts::TAU;
            }
            let mut input = self.input();
            if let Some(l) = &self.ladders {
                l.touched(&self.state.shape(), self.state.pos, &mut self.touched);
            }
            input.containers = &self.touched;
            self.state.step(&self.params, &*self.world, &input, dt);
            self.ticks += 1;
        }
    }

    /// A server teleport (0xa0 flag 0x0020): new `move_seq`, position and box.
    // SPEC: MOVEMENT_2009.md §12.4 (flag 0x0020)
    pub fn teleport(&mut self, seq: u8, pos: [f32; 3], dims: [f32; 3]) {
        self.state.move_seq = seq;
        self.state.pos = V3::from_array(pos);
        self.state.vel = V3::ZERO;
        self.state.acc = V3::ZERO;
        if dims.iter().all(|d| *d > 0.0 && d.is_finite()) {
            self.state.dims = V3::from_array(dims);
            self.state.stand_dims = V3::from_array(dims);
        }
        if !self.rom_world {
            self.world = Arc::new(flat_floor(pos[1] - self.state.dims.y));
        }
    }

    pub fn pos(&self) -> [f32; 3] {
        let p = self.state.pos;
        [p.x, p.y, p.z]
    }

    pub fn vel(&self) -> [f32; 3] {
        let v = self.state.vel;
        [v.x, v.y, v.z]
    }

    /// On a ladder (the movement's ladder contact).
    pub fn on_ladder(&self) -> bool {
        self.state.on_ladder
    }

    pub fn on_ground(&self) -> bool {
        self.state.on_ground
    }

    /// The control word the movement used last (mover+0x24: RUN, ANY_MOVE, ...).
    pub fn controls(&self) -> u32 {
        self.state.controls
    }

    /// In the air after a jump, from the take-off to the next real contact (dc-sim's whole-flight state, not
    /// mover+0x80: that flag ends one step after the take-off, `PlayerState::jump_flag`).
    // DC-DEFAULT: SIM-20 (proposed) the fire cone reads the whole-flight state
    pub fn jumping(&self) -> bool {
        self.state.jumping
    }

    /// The aim rotation built from pitch and yaw (the shared simulation's camera convention, PLAUSIBLE for the
    /// 2009 Euler builder), as `quat128` components x, y, z, w.
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (aim_q: rotation built from local_player+0x48, +0x4c, +0x50)
    // SPEC: MOVEMENT_2009.md §13.2 step 7 (rotation order PLAUSIBLY yaw about +Y, pitch about +X)
    pub fn aim_quat(&self) -> [f32; 4] {
        let q = dc_sim::Quat::from_yaw_pitch(self.angles[1], self.angles[0]);
        [q.x, q.y, q.z, q.w]
    }

    /// Where the body would be after running in direction `yaw` for `secs` seconds, simulated on a copy (the
    /// way-finder's probe: the same movement code, so a free direction is one the body can really take).
    pub fn probe_walk(&self, yaw: f32, secs: f32) -> [f32; 3] {
        let mut b = self.clone();
        b.kind = MotionKind::Hunt;
        b.frozen = false;
        b.steer = Some(yaw);
        // whole steps one at a time (the fixed-step clock catches up only a few steps per call)
        let dt_us = (1e6 / self.clock.hz().max(1) as f32).ceil() as u64;
        let n = (secs.max(0.0) * self.clock.hz() as f32).round() as u32;
        for _ in 0..n {
            b.advance(dt_us);
        }
        b.pos()
    }

    /// Current half extents of the body box.
    pub fn dims(&self) -> [f32; 3] {
        let d = self.state.dims;
        [d.x, d.y, d.z]
    }

    pub fn move_seq(&self) -> u8 {
        self.state.move_seq
    }

    /// The player-update fields at client time `time` (seconds).
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (control word = mover+0x24; trailer of MOVEMENT_2009.md §12.2)
    pub fn update(&self, time: f32) -> MoveReport {
        let mut control = self.state.controls;
        if !self.frozen && (self.kind == MotionKind::Circle || self.steer.is_some()) {
            control |= bits::FWD;
        }
        // BOT-92: the evasive move's own control bits
        if let Some(e) = self.evading() {
            if self.steer.is_none() {
                if e.back {
                    control |= bits::BACK;
                } else if e.strafe < 0 {
                    control |= bits::LEFT;
                } else if e.strafe > 0 {
                    control |= bits::RIGHT;
                }
                if e.crouch {
                    control |= bits::CROUCH;
                }
                control |= bits::RUN;
            }
            if e.jump {
                control |= bits::JUMP;
            }
        }
        MoveReport {
            angles: self.angles,
            // DC-DEFAULT: BOT-3 (proposed) profile, look and aux words the bot does not have are sent as 0
            aux: [0; 6],
            aim: None,
            control,
            trailer: MoveTrailer {
                move_seq: self.state.move_seq,
                pos: self.pos(),
                vel: self.vel(),
                time,
                on_ground: self.state.on_ground as u8,
                surface: 0,
                standing: STANDING_NONE,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frozen_body_stands_still_and_sends_no_walk_control() {
        let mut b = Body::new(
            [10.0, 100.0, 20.0],
            BOT_STAND_DIMS,
            0.0,
            MotionKind::Circle,
            60,
            None,
        );
        b.advance(1_000_000);
        assert!(b.update(0.0).control & bits::FWD != 0);
        b.frozen = true;
        for _ in 0..300 {
            b.advance(16_000); // the walk's momentum runs out
        }
        let walked = b.pos();
        let yaw = b.angles[1];
        for _ in 0..120 {
            b.advance(16_000);
        }
        let p = b.pos();
        assert!(
            (p[0] - walked[0]).abs() < 1e-3 && (p[2] - walked[2]).abs() < 1e-3,
            "{walked:?} {p:?}"
        );
        assert_eq!(b.angles[1], yaw);
        assert_eq!(b.update(0.0).control & bits::FWD, 0);
        b.frozen = false;
        assert!(b.update(0.0).control & bits::FWD != 0);
    }

    #[test]
    fn standing_body_rests_on_the_flat_floor() {
        let mut b = Body::new(
            [10.0, 100.0, 20.0],
            BOT_STAND_DIMS,
            0.0,
            MotionKind::Stand,
            60,
            None,
        );
        b.advance(2_000_000);
        let p = b.pos();
        assert!(b.on_ground(), "{p:?}");
        assert!((p[1] - 100.0).abs() < 1.0, "{p:?}");
        assert!((p[0] - 10.0).abs() < 1e-3 && (p[2] - 20.0).abs() < 1e-3);
    }

    #[test]
    fn walking_body_moves_on_a_circle() {
        let mut b = Body::new(
            [0.0, 40.0, 0.0],
            BOT_STAND_DIMS,
            0.0,
            MotionKind::Circle,
            60,
            None,
        );
        let mut far = 0.0f32;
        let mut pts = Vec::new();
        for _ in 0..(8 * 60) {
            b.advance(1_000_000 / 60);
            let p = b.pos();
            far = far.max((p[0] * p[0] + p[2] * p[2]).sqrt());
            pts.push(p);
        }
        assert!(far > 20.0, "it walked ({far})");
        // after one full turn the body is back near the start
        let last = pts.last().unwrap();
        assert!(
            (last[0].powi(2) + last[2].powi(2)).sqrt() < far,
            "{last:?} far {far}"
        );
        let u = b.update(1.0);
        assert_eq!(u.control & bits::FWD, bits::FWD);
        assert!(u.trailer.pos.iter().all(|c| c.is_finite()));
    }

    #[test]
    fn teleport_sets_seq_position_and_box() {
        let mut b = Body::new(
            [0.0, 40.0, 0.0],
            BOT_STAND_DIMS,
            0.0,
            MotionKind::Stand,
            60,
            None,
        );
        b.teleport(7, [100.0, 200.0, 300.0], [20.0, 45.0, 20.0]);
        b.advance(500_000);
        assert_eq!(b.move_seq(), 7);
        let p = b.pos();
        assert!(
            (p[1] - 200.0).abs() < 1.0 && (p[0] - 100.0).abs() < 1e-3,
            "{p:?}"
        );
    }

    /// ROM-gated (skipped without CA_ROM): a hunting bot that walks onto a WARHEAD ladder with its goal above
    /// climbs it and stands on the level at its top within 6 s (playtest report: "the bot is stuck halfway up the
    /// ladder": one ladder rises from a pit whose wall the body cannot step up). Every ladder is approached from each
    /// side whose floor lies below the ladder's top; at least one side of every such ladder must climb, and without
    /// the ladder volumes the body stays on its floor.
    // DC-DEFAULT: BOT-61 (proposed)
    #[test]
    fn rom_a_bot_climbs_every_warhead_ladder_to_the_top() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let worlds = crate::romworld::RomWorlds::open(&rom).expect("world archive");
        let listed = worlds
            .resolve(b"WARHEAD.DAT")
            .expect("WARHEAD in the archive");
        let w = worlds.load(&listed).expect("WARHEAD loads");
        let dims = [15.0f32, 50.0, 15.0];
        let shape = dc_sim::Cylinder::from_dims(V3::from_array(dims));
        // (start floor, highest feet reached) of a 6 s walk into the ladder from `side`
        let climb =
            |v: &dc_sim::ConvexVolume, side: (f32, f32), ladders: bool| -> Option<(f32, f32)> {
                let mid = (v.lo + v.hi) * 0.5;
                let half = (v.hi - v.lo) * 0.5;
                let at = [
                    mid.x + side.0 * (half.x + dims[0] + 4.0),
                    v.hi.y,
                    mid.z + side.1 * (half.z + dims[2] + 4.0),
                ];
                if w.collision.overlaps(&shape, V3::from_array(at)) {
                    return None;
                }
                let mut b = Body::new(
                    at,
                    dims,
                    0.0,
                    MotionKind::Hunt,
                    60,
                    Some(w.collision.clone()),
                );
                let floor = b.pos()[1] - dims[1];
                // a start on a floor inside the ladder's height (not on the level at its top, not far below it)
                if !b.rom_world || floor > v.hi.y - 40.0 || floor < v.lo.y - 60.0 {
                    return None;
                }
                if ladders {
                    b.ladders = Some(w.ladders.clone());
                }
                let yaw = (mid.x - at[0]).atan2(mid.z - at[2]);
                b.goal_y = Some(v.hi.y + 100.0);
                let mut feet = f32::NEG_INFINITY;
                for _ in 0..360 {
                    b.steer = Some(yaw);
                    b.advance(1_000_000 / 60);
                    feet = feet.max(b.pos()[1] - dims[1]);
                }
                Some((floor, feet))
            };
        let mut tried = 0;
        for (v, _) in &w.ladders.volumes {
            let sides = [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)];
            let runs: Vec<_> = sides
                .iter()
                .filter_map(|&s| climb(v, s, true).map(|r| (s, r)))
                .collect();
            if runs.is_empty() {
                continue;
            }
            tried += 1;
            let up = runs.iter().find(|(_, (_, feet))| *feet >= v.hi.y - 40.0);
            let Some((side, (floor, _))) = up else {
                panic!("ladder {:?}..{:?}: no side climbed: {runs:?}", v.lo, v.hi);
            };
            let (_, below) = climb(v, *side, false).expect("same start");
            assert!(
                below < floor + 31.0,
                "without ladders the body rose from {floor} to {below}"
            );
        }
        assert_eq!(
            tried,
            w.ladders.volumes.len(),
            "a start beside every ladder"
        );
    }
}
