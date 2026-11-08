//! `dc-mapkit walk`: walking a map offline with the game's own movement and collision (DC-DEFAULT: MAP-119).
//!
//! The project is built in memory and, with ROM pieces (MAP-116), composed from the user's ROM exactly as every
//! client and game server composes it at load time. On that world:
//!
//! - the collision is built as the client builds it: `dc_sim::from_world::collision_world` with
//!   `WorldFilter::default()` (doors and switches stand at their file pose, closed; `--open-doors` leaves them
//!   out), and the ladder volumes of `ladder_volumes`, refreshed every tick (dc-client `world.rs`, `player.rs`);
//! - the kill volumes are tested as the game server tests them: the closed model of each kill volume brush, a
//!   point-inside test on the body centre (dc-dserver `killvol.rs`, DSV-52), the first entry is the death;
//! - a standing body (half extents [`BODY`]) is placed on the floor under the start as the client places a spawn
//!   (`dc_sim::drop_to_floor`, SIM-2) and stepped by `dc_sim::PlayerState::step` at the client's fixed 60 Hz
//!   (CLI-MOVE-1) with a bot's commands: the yaw turned toward the next waypoint every tick, forward and run held
//!   (the client's normal pace, CLI-2; `--sprint` sprints), a jump pressed when the horizontal progress stalls for
//!   [`JUMP_AFTER`] seconds, and the walk given up after [`GIVE_UP`] seconds without progress or at the time limit;
//! - a landing is judged by the game server's fall rule (DSV-76: no damage up to 300 units, 200 at 700) and a fall
//!   that takes the whole start health is a death; a body below everything (the world's extents, every floor and
//!   every kill volume, by [`FELL_BELOW`]) fell out of the world.
//!
//! The survey casts straight down at every column of a grid from the top of the world's extents (sky faces and
//! surfaces too steep to stand on are passed through) and reports the columns with no floor at all (holes, with
//! the kill volume a fall there ends in) and the columns whose first floor puts a standing body inside a kill
//! volume, in connected areas and as a top-view picture (the flat `render --view top` of the same world).
//!
//! `--reach` ([`crate::reach`], MAP-122) finds every floor of a grid and every move between neighbouring floors with
//! the same world, body, mover, kill test and fall rule, and reports where a player who goes in cannot get out
//! (traps), which moves kill and which floors are out of reach.

use crate::build::{self, Assets};
use crate::cli::Outcome;
use crate::diag::{Code, Diagnostic, Diagnostics, Severity};
use crate::project::{write_atomic, Heights, Project};
use crate::render_sw::{self, RenderOptions, RgbaImage, Scene, View};
use dc_sim::from_world::{collision_world, ladder_volumes, selected_models, WorldFilter};
use dc_sim::{
    drop_to_floor, Collider, CollisionWorld, Commands, Container, ContainerVolumes, Cylinder,
    MovementParams, PlayerState, TickInput, BLOCKER_MODEL, SURF_SOLID, V3,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// The standing body's half extents: the box the game server sends with the spawn teleport (the ROM player
/// models' stand box).
// SPEC: MOVEMENT_2009.md §9.2, §12.4 (observed tdm1: the 0xa0 0x0020 teleport carries half extents (15, 50, 15))
pub const BODY: [f32; 3] = [15.0, 50.0, 15.0];
/// Simulation ticks per second: the client's fixed mover step.
// DC-DEFAULT: CLI-MOVE-1 (proposed), SIM-1 (proposed) 60 Hz
pub const TICK_HZ: u32 = dc_sim::DEFAULT_TICK_HZ;
/// A route is reached within this many units of its target in x / z.
// DC-DEFAULT: MAP-119
pub const REACH: f32 = 64.0;
/// Progress toward the waypoint that counts (units of horizontal distance gained); less is a stall.
// DC-DEFAULT: MAP-119
pub const PROGRESS: f32 = 16.0;
/// A stall this long (seconds, standing) presses jump, and again after each further such stretch.
// DC-DEFAULT: MAP-119
pub const JUMP_AFTER: f32 = 0.5;
/// A stall this long (seconds) ends the walk: stuck.
// DC-DEFAULT: MAP-119
pub const GIVE_UP: f32 = 3.0;
/// Ticks the jump key is held at most for one press (released at the take-off).
// DC-DEFAULT: MAP-119
pub const JUMP_HOLD_TICKS: u32 = 6;
/// One path point every this many seconds.
// DC-DEFAULT: MAP-119
pub const PATH_EVERY: f32 = 0.5;
/// The time limit of a route unless `--seconds` says otherwise.
// DC-DEFAULT: MAP-119
pub const DEFAULT_SECONDS: f32 = 60.0;
/// How far under the start a floor is looked for, after the lift of one step (the client's spawn placement).
// DC-DEFAULT: SIM-2 (proposed) lift by one step, sweep down at most 1000 units
pub const START_DROP: f32 = 1000.0;
/// The floor under a route's start is looked for from this far above `from` (half a standing body and one step).
// DC-DEFAULT: MAP-119
pub const START_LIFT: f32 =
    BODY[1] + MovementParams::DEFAULT.step_height + MovementParams::DEFAULT.step_margin;
/// A body this far below the world's extents, every collision model and every kill volume fell out of the world.
// DC-DEFAULT: MAP-119 (as dc-client's headless `fell out`: 100 below the lowest collision model)
pub const FELL_BELOW: f32 = 100.0;
/// Kill volumes are tested at least every this many units of a tick's travel (a fast fall crosses a thin slab
/// within one tick).
// DC-DEFAULT: MAP-119
pub const KILL_SAMPLE: f32 = 16.0;
/// The game server's fall rule (DSV-76): falls up to `FALL_H_LO` do no damage, then linearly up to `FALL_D_HI` at
/// `FALL_H_HI` and no more above it; the start health a lethal fall takes.
// DC-DEFAULT: DSV-76 (proposed) 300 / 700 / 0 / 200; dc-dserver game.rs `GameTuning` start health 100
pub const FALL_H_LO: f32 = 300.0;
pub const FALL_H_HI: f32 = 700.0;
pub const FALL_D_LO: f32 = 0.0;
pub const FALL_D_HI: f32 = 200.0;
pub const START_HEALTH: f32 = 100.0;
/// View pitch on a ladder (radians, positive looks down) and the band in which a waypoint counts as level (the
/// body climbs up then).
// DC-DEFAULT: BOT-61 (proposed) the bot's ladder pitch, reused
pub const LADDER_PITCH: f32 = 1.2;
pub const LADDER_LEVEL_BAND: f32 = 40.0;
/// The survey's grid spacing unless `--step` says otherwise.
// DC-DEFAULT: MAP-119
pub const SURVEY_STEP: f32 = 128.0;
/// Most columns one survey casts (a smaller step over a larger box is refused).
pub const SURVEY_MAX_COLUMNS: usize = 4_000_000;
/// Most columns listed one by one, and most areas listed, per kind.
pub const LIST_CAP: usize = 200;
pub const AREA_CAP: usize = 50;
/// Most survey areas that get a warning diagnostic of their own, per kind.
pub const DIAG_CAP: usize = 20;
/// Most casts down one column (each passes a sky face or a steep surface).
const MAX_CASTS: usize = 32;
/// Sky face surface flag (FMT_DAT85.md: the level's 0x10 polygons are the sky faces).
const SURF_SKY: u32 = crate::geom::SURF_SKY;
/// The long side of the picture unless `--size` says otherwise.
pub const PICTURE_SIDE: u32 = 1536;
/// Colours of the picture: holes, kill floors, region outlines, routes by their end.
pub const HOLE_COLOR: [u8; 3] = [235, 40, 40];
pub const KILL_COLOR: [u8; 3] = [255, 150, 20];
pub const REGION_COLOR: [u8; 3] = [90, 190, 255];
const REACHED_COLOR: [u8; 3] = [70, 230, 90];
const STUCK_COLOR: [u8; 3] = [255, 230, 40];
const TIMEOUT_COLOR: [u8; 3] = [235, 235, 235];

// ---------------------------------------------------------------------------------------------------------------
// the world

/// One kill volume: its closed model as the game server tests it.
#[derive(Debug, Clone, PartialEq)]
pub struct KillVolume {
    pub name: String,
    pub lo: V3,
    pub hi: V3,
    /// The time a player must stay inside before it dies (the volume's pending property, 0 = at once); the server
    /// kills it also when it left meanwhile, so entering is the death.
    pub pending_s: f32,
    faces: Vec<KillFace>,
}

#[derive(Debug, Clone, PartialEq)]
struct KillFace {
    normal: V3,
    dist: f32,
    corners: Vec<V3>,
}

/// A ray direction off every axis and diagonal, so a ray does not run along the edges of a box volume (the game
/// server's constant).
// DC-DEFAULT: DSV-52 (dc-dserver killvol.rs `RAY`)
const KILL_RAY: V3 = V3::new(0.000_37, 1.0, 0.000_71);

impl KillVolume {
    /// Whether `p` lies inside the closed model: inside its box, and an odd number of its polygons cross the ray
    /// from `p` (the game server's test, dc-dserver killvol.rs `VolumeShape::contains`).
    // DC-DEFAULT: DSV-52 (proposed) the closed-model point test, re-expressed here (dc-mapkit does not depend on
    // dc-dserver)
    pub fn contains(&self, p: V3) -> bool {
        if p.x < self.lo.x
            || p.y < self.lo.y
            || p.z < self.lo.z
            || p.x > self.hi.x
            || p.y > self.hi.y
            || p.z > self.hi.z
        {
            return false;
        }
        let mut crossings = 0u32;
        for f in &self.faces {
            let den = f.normal.dot(KILL_RAY);
            if den.abs() < 1e-9 || f.corners.len() < 3 {
                continue;
            }
            let t = (f.dist - f.normal.dot(p)) / den;
            if t <= 0.0 {
                continue;
            }
            let q = p + KILL_RAY * t;
            // inside the convex polygon: the same side of every edge (either winding)
            let n = f.corners.len();
            let (mut pos, mut neg) = (false, false);
            for k in 0..n {
                let a = f.corners[k];
                let b = f.corners[(k + 1) % n];
                let s = (b - a).cross(q - a).dot(f.normal);
                if s > 0.0 {
                    pos = true;
                } else if s < 0.0 {
                    neg = true;
                }
            }
            if !(pos && neg) {
                crossings += 1;
            }
        }
        crossings % 2 == 1
    }

    /// The highest point of the vertical line through (x, z) inside the volume, sampled every [`KILL_SAMPLE`]
    /// units or 8 times across its height, whichever is finer (a box volume is found at its first sample).
    fn on_column(&self, x: f32, z: f32) -> Option<f32> {
        if x < self.lo.x || x > self.hi.x || z < self.lo.z || z > self.hi.z {
            return None;
        }
        let h = self.hi.y - self.lo.y;
        let step = (h / 8.0).clamp(0.01, KILL_SAMPLE);
        let mut y = self.hi.y - step * 0.5;
        while y > self.lo.y {
            if self.contains(V3::new(x, y, z)) {
                return Some(y);
            }
            y -= step;
        }
        None
    }
}

/// The kill volumes of a parsed world: every kill-volume object whose brush model is there (dc-dserver
/// `kill_shapes`, DSV-52; the server acts on every one it creates on the clients, which are these).
pub fn kill_volumes(w: &dc_world::World<'_>) -> Vec<KillVolume> {
    let mut out = Vec::new();
    for o in w.objects_with_role(dc_world::ObjectRole::KillVolume) {
        let Some(m) = w.brush_of(o) else {
            continue;
        };
        let faces = m
            .polygons
            .iter()
            .filter_map(|p| {
                let pl = m.planes.get(p.plane as usize)?;
                Some(KillFace {
                    normal: V3::from_array(pl.normal),
                    dist: pl.dist,
                    corners: m
                        .polygon_points(p)
                        .iter()
                        .filter_map(|&i| m.points.get(i as usize).map(|&q| V3::from_array(q)))
                        .collect(),
                })
            })
            .collect();
        let pending_s = o
            .prop_named(&dc_world::names::PROP_PENDING_SEC)
            .and_then(|p| match p.value {
                dc_world::FieldValue::Real(r) if r.is_finite() && r > 0.0 => Some(r),
                _ => None,
            })
            .unwrap_or(0.0);
        out.push(KillVolume {
            name: String::from_utf8_lossy(o.name().unwrap_or_default()).into_owned(),
            lo: V3::from_array(m.box_min),
            hi: V3::from_array(m.box_max),
            pending_s,
            faces,
        });
    }
    out
}

/// What the walk and the survey run against.
pub struct WalkWorld {
    pub collision: CollisionWorld,
    pub ladders: ContainerVolumes,
    pub kills: Vec<KillVolume>,
    /// What each collision model is, for the reports (`level`, `door <name>`, the object's name).
    models: Vec<String>,
    pub extent_min: V3,
    pub extent_max: V3,
    /// The lowest point of every collision model.
    pub floor_limit: f32,
    /// Below this a body fell out of the world: [`FELL_BELOW`] under the extents, every model and every kill volume.
    pub bottom: f32,
    /// The x / z box of every solid polygon that is not a sky face (`[x0, z0, x1, z1]`): the survey's default area.
    pub ground: Option<[f32; 4]>,
    pub doors_open: bool,
}

impl WalkWorld {
    /// The walk world of a parsed world file. `open_doors` leaves the doors and switches out of the collision
    /// (in a match the game server opens a door for a player who comes near, DSV-46).
    pub fn of(w: &dc_world::World<'_>, open_doors: bool) -> WalkWorld {
        let filter = if open_doors {
            WorldFilter {
                movers: Vec::new(),
                ..WorldFilter::default()
            }
        } else {
            WorldFilter::default()
        };
        let collision = collision_world(w, &filter);
        let picked = selected_models(w, &filter);
        let mut ground: Option<[f32; 4]> = None;
        let mut models = Vec::with_capacity(picked.len());
        for &i in &picked {
            let m = &w.models[i];
            models.push(match w.object_named(m.name) {
                None => "level".to_string(),
                Some(o) => {
                    let name = String::from_utf8_lossy(o.name().unwrap_or_default()).into_owned();
                    if o.role.is_none() && filter.is_mover(o) {
                        format!("door {name}")
                    } else {
                        name
                    }
                }
            });
            for p in &m.polygons {
                let flags = m.surfaces.get(p.surface as usize).map_or(0, |s| s.flags);
                if flags & SURF_SOLID == 0 || flags & SURF_SKY != 0 {
                    continue;
                }
                for &k in m.polygon_points(p) {
                    let Some(q) = m.points.get(k as usize) else {
                        continue;
                    };
                    let g = ground.get_or_insert([q[0], q[2], q[0], q[2]]);
                    g[0] = g[0].min(q[0]);
                    g[1] = g[1].min(q[2]);
                    g[2] = g[2].max(q[0]);
                    g[3] = g[3].max(q[2]);
                }
            }
        }
        let kills = kill_volumes(w);
        let extent_min = V3::from_array(w.extent_min);
        let extent_max = V3::from_array(w.extent_max);
        let floor_limit = collision
            .models
            .iter()
            .map(|m| m.bounds().0.y)
            .fold(f32::INFINITY, f32::min);
        let floor_limit = if floor_limit.is_finite() {
            floor_limit
        } else {
            extent_min.y
        };
        let lowest = kills
            .iter()
            .map(|k| k.lo.y)
            .fold(floor_limit.min(extent_min.y), f32::min);
        WalkWorld {
            collision,
            ladders: ladder_volumes(w),
            kills,
            models,
            extent_min,
            extent_max,
            floor_limit,
            bottom: lowest - FELL_BELOW,
            ground,
            doors_open: open_doors,
        }
    }

    /// The first kill volume holding `p`.
    pub fn kill_at(&self, p: V3) -> Option<usize> {
        self.kills.iter().position(|k| k.contains(p))
    }

    /// The first kill volume a point moving from `a` to `b` enters (sampled every [`KILL_SAMPLE`] units).
    pub(crate) fn kill_between(&self, a: V3, b: V3) -> Option<(usize, V3)> {
        let n = ((b - a).len() / KILL_SAMPLE).ceil().clamp(1.0, 4096.0) as u32;
        (1..=n).find_map(|i| {
            let p = a + (b - a) * (i as f32 / n as f32);
            self.kill_at(p).map(|k| (k, p))
        })
    }

    /// What a collision hit's model is.
    fn label(&self, model: u32) -> String {
        if model == BLOCKER_MODEL {
            "invisible wall".to_string()
        } else {
            self.models
                .get(model as usize)
                .cloned()
                .unwrap_or_else(|| format!("model {model}"))
        }
    }

    /// The first floor under (x, z) from the top of the extents: the first hit on a solid surface a body can stand
    /// on that is not a sky face (those and steeper surfaces are passed through). Returns its height.
    pub fn first_floor(&self, x: f32, z: f32, standable: f32) -> Option<f32> {
        let bottom = self.extent_min.y - 1.0;
        let mut top = self.extent_max.y + 1.0;
        for _ in 0..MAX_CASTS {
            if top <= bottom {
                return None;
            }
            let from = V3::new(x, top, z);
            let to = V3::new(x, bottom, z);
            let hit = self.collision.ray_pruned(from, to)?;
            let y = top + (bottom - top) * hit.t;
            if hit.surface_flags & SURF_SKY == 0 && hit.face_normal.y >= standable {
                return Some(y);
            }
            top = y - 0.5;
        }
        None
    }
}

// ---------------------------------------------------------------------------------------------------------------
// routes

/// One route of a routes file: `{"name": ..., "from": [x, y, z], "to": [x, y, z], "via": [[x, y, z], ...]}`
/// (`via`, waypoints walked first, is optional).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    #[serde(default)]
    pub name: String,
    pub from: [f32; 3],
    pub to: [f32; 3],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<[f32; 3]>,
}

/// How each route is walked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WalkOptions {
    /// The time limit (seconds).
    pub seconds: f32,
    pub sprint: bool,
}

impl Default for WalkOptions {
    fn default() -> Self {
        WalkOptions {
            seconds: DEFAULT_SECONDS,
            sprint: false,
        }
    }
}

/// How a route ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteEnd {
    Reached,
    Died,
    Stuck,
    Timeout,
}

impl RouteEnd {
    pub fn text(self) -> &'static str {
        match self {
            RouteEnd::Reached => "reached",
            RouteEnd::Died => "died",
            RouteEnd::Stuck => "stuck",
            RouteEnd::Timeout => "timeout",
        }
    }
}

/// How a walking body died.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Death {
    /// `kill_volume`, `fall` (a lethal landing) or `fell_out` (below everything).
    pub cause: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    /// The volume's pending time; the server kills after it also when the body left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_s: Option<f32>,
    /// A lethal fall's height and damage (DSV-76).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damage: Option<f32>,
    pub at: [f32; 3],
    pub seconds: f32,
    /// The ROM piece whose region holds the point (`None`: the map's own ground).
    pub piece: Option<String>,
}

/// Where a walking body stopped making progress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stuck {
    pub at: [f32; 3],
    pub seconds: f32,
    /// What stands in the way toward the waypoint (`level`, `invisible wall`, `door <name>`, a model's object name),
    /// `None` when nothing does (a ledge, a slope, a hole the body cannot jump).
    pub blocked_by: Option<String>,
    pub piece: Option<String>,
}

/// The answer for one route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteReport {
    pub name: String,
    pub from: [f32; 3],
    pub to: [f32; 3],
    /// Where the body was placed (on the floor under `from`).
    pub start: [f32; 3],
    /// A floor was found under `from` (else the body starts in the air and falls).
    pub start_floor: bool,
    /// The standing body overlaps solid geometry at the start.
    pub start_blocked: bool,
    pub outcome: RouteEnd,
    pub reached: bool,
    pub end: [f32; 3],
    /// Horizontal distance left to the target.
    pub left: f32,
    pub seconds: f32,
    /// Horizontal distance walked.
    pub distance: f32,
    pub jumps: u32,
    /// The largest fall (top of the flight to the landing), and its damage by the server's rule.
    pub largest_fall: f32,
    pub fall_damage: f32,
    pub died: Option<Death>,
    pub stuck: Option<Stuck>,
    /// The body centre every [`PATH_EVERY`] seconds, start and end included.
    pub path: Vec<[f32; 3]>,
    /// The ROM piece whose region holds the end (`None`: the map's own ground).
    pub end_piece: Option<String>,
}

/// The game server's fall damage for a fall of `h` units (DSV-76).
pub fn fall_damage(h: f32) -> f32 {
    if !h.is_finite() || h <= FALL_H_LO {
        return 0.0;
    }
    let span = FALL_H_HI - FALL_H_LO;
    let f = if span > 0.0 {
        ((h - FALL_H_LO) / span).min(1.0)
    } else {
        1.0
    };
    FALL_D_LO + f * (FALL_D_HI - FALL_D_LO)
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn arr(v: V3) -> [f32; 3] {
    [round1(v.x), round1(v.y), round1(v.z)]
}

fn xz_dist(a: V3, b: V3) -> f32 {
    let (dx, dz) = (b.x - a.x, b.z - a.z);
    (dx * dx + dz * dz).sqrt()
}

/// Where a walk from `from` starts: on the first floor a body can stand on under a point [`START_LIFT`] above it (a
/// `from` given a little under the floor still finds it), then placed as the client places a spawn (lifted one
/// step, swept down at most [`START_DROP`], SIM-2); without such a floor, the client's placement at `from` itself.
// DC-DEFAULT: MAP-119
pub(crate) fn place(w: &WalkWorld, shape: &Cylinder, p: &MovementParams, from: V3) -> V3 {
    let top = from + V3::new(0.0, START_LIFT, 0.0);
    let floor = w
        .collision
        .ray(top, from - V3::new(0.0, START_DROP, 0.0))
        .filter(|h| h.face_normal.y >= p.standable_cos)
        .map(|h| top.y + (from.y - START_DROP - top.y) * h.t);
    let at = match floor {
        Some(y) => V3::new(from.x, y + shape.half_height + 1.0, from.z),
        None => from,
    };
    drop_to_floor(&w.collision, shape, at, p.step(), START_DROP)
}

/// Walks one route (see the module documentation for the rules).
pub fn walk_route(w: &WalkWorld, r: &Route, o: &WalkOptions) -> RouteReport {
    let p = MovementParams::DEFAULT;
    let dims = V3::from_array(BODY);
    let shape = Cylinder::from_dims(dims);
    let from = V3::from_array(r.from);
    let start = place(w, &shape, &p, from);
    let start_floor = w
        .collision
        .sweep(&shape, start, start - V3::new(0.0, 2.0, 0.0))
        .is_some();
    let start_blocked = w.collision.overlaps(&shape, start + V3::new(0.0, 0.5, 0.0));
    let mut st = PlayerState::new(start, dims, &p);
    let legs: Vec<V3> = r
        .via
        .iter()
        .chain(std::iter::once(&r.to))
        .map(|a| V3::from_array(*a))
        .collect();
    let dt = 1.0 / TICK_HZ as f32;
    let max_ticks = (o.seconds.max(0.0) * TICK_HZ as f32).ceil() as u64;
    let mut containers: Vec<Container> = Vec::new();
    let mut leg = 0usize;
    let mut mark = xz_dist(st.pos, legs[0]);
    let mut last_progress = 0.0f32;
    let mut next_jump = JUMP_AFTER;
    let mut jump_hold = 0u32;
    let mut jumps = 0u32;
    let mut top: Option<f32> = None;
    let mut largest_fall = 0.0f32;
    let mut worst_damage = 0.0f32;
    let mut distance = 0.0f32;
    let mut path = vec![arr(start)];
    let mut next_path = PATH_EVERY;
    let mut t = 0.0f32;
    let mut end = RouteEnd::Timeout;
    let mut died: Option<Death> = None;
    let mut stuck: Option<Stuck> = None;
    for tick in 0..max_ticks {
        // the waypoint: the next one once this one is reached
        let mut dist = xz_dist(st.pos, legs[leg]);
        while dist <= REACH && leg + 1 < legs.len() {
            leg += 1;
            dist = xz_dist(st.pos, legs[leg]);
            mark = dist;
            last_progress = t;
            next_jump = JUMP_AFTER;
        }
        // the target counts only standing there: a body passing over it in a fall has not arrived
        if dist <= REACH && (st.on_ladder || st.contact.is_some()) {
            end = RouteEnd::Reached;
            break;
        }
        let goal = legs[leg];
        let yaw = (goal.x - st.pos.x).atan2(goal.z - st.pos.z);
        // the containers the body touches, refreshed before the law runs (dc-client player.rs `tick`)
        w.ladders.touched(&st.shape(), st.pos, &mut containers);
        let pitch = if st.on_ladder {
            if goal.y < st.pos.y - LADDER_LEVEL_BAND {
                LADDER_PITCH
            } else {
                -LADDER_PITCH
            }
        } else {
            0.0
        };
        let input = TickInput {
            commands: Commands {
                forward: true,
                run: true,
                sprint: o.sprint,
                jump: jump_hold > 0,
                ..Commands::default()
            },
            yaw,
            pitch,
            // alive (the server's player state 1)
            ctrl_state: 1,
            containers: &containers,
            ..TickInput::default()
        };
        let before = st.pos;
        let ev = st.step(&p, &w.collision, &input, dt);
        t = (tick + 1) as f32 * dt;
        if ev.jumped {
            jumps += 1;
            jump_hold = 0;
        } else {
            jump_hold = jump_hold.saturating_sub(1);
        }
        if !st.pos.is_finite() {
            stuck = Some(Stuck {
                at: arr(before),
                seconds: round1(t),
                blocked_by: Some("the simulation left the numbers (a collision fault)".into()),
                piece: None,
            });
            st.pos = before;
            end = RouteEnd::Stuck;
            break;
        }
        distance += xz_dist(before, st.pos);
        if t >= next_path {
            path.push(arr(st.pos));
            next_path += PATH_EVERY;
        }
        // kill volumes along the tick's travel (the server's test on the body centre)
        if let Some((k, at)) = w.kill_between(before, st.pos) {
            let kv = &w.kills[k];
            died = Some(Death {
                cause: "kill_volume".into(),
                volume: Some(kv.name.clone()),
                pending_s: (kv.pending_s > 0.0).then_some(kv.pending_s),
                // the flight so far when it died falling
                height: top.map(|t0| round1(t0.max(before.y) - at.y)),
                damage: None,
                at: arr(at),
                seconds: round1(t),
                piece: None,
            });
            st.pos = at;
            end = RouteEnd::Died;
            break;
        }
        // the fall track: the highest point since the last grounded step, judged at the landing (DSV-76)
        let grounded = st.on_ladder || st.contact.is_some_and(|c| c.normal.y >= p.ground_cos);
        if grounded {
            if let Some(t0) = top.take() {
                let h = t0 - st.pos.y;
                largest_fall = largest_fall.max(h);
                let dmg = fall_damage(h);
                worst_damage = worst_damage.max(dmg);
                if dmg >= START_HEALTH {
                    died = Some(Death {
                        cause: "fall".into(),
                        volume: None,
                        pending_s: None,
                        height: Some(round1(h)),
                        damage: Some(round1(dmg)),
                        at: arr(st.pos),
                        seconds: round1(t),
                        piece: None,
                    });
                    end = RouteEnd::Died;
                    break;
                }
            }
        } else {
            let y0 = top.unwrap_or(before.y);
            top = Some(y0.max(st.pos.y));
        }
        if st.pos.y < w.bottom {
            died = Some(Death {
                cause: "fell_out".into(),
                volume: None,
                pending_s: None,
                height: top.map(|t0| round1(t0 - st.pos.y)),
                damage: None,
                at: arr(st.pos),
                seconds: round1(t),
                piece: None,
            });
            end = RouteEnd::Died;
            break;
        }
        // progress, stalls, jumps
        let dist = xz_dist(st.pos, goal);
        if mark - dist >= PROGRESS {
            mark = dist;
            last_progress = t;
            next_jump = JUMP_AFTER;
        }
        if st.contact.is_none() && !st.on_ladder {
            // a flight is not a stall
            last_progress += dt;
        }
        let stall = t - last_progress;
        if stall >= GIVE_UP {
            let ahead = V3::new(yaw.sin(), 0.0, yaw.cos()) * (shape.radius + 24.0);
            let blocked_by = w
                .collision
                .sweep(&shape, st.pos, st.pos + ahead)
                .map(|h| w.label(h.model));
            stuck = Some(Stuck {
                at: arr(st.pos),
                seconds: round1(t),
                blocked_by,
                piece: None,
            });
            end = RouteEnd::Stuck;
            break;
        }
        if stall >= next_jump {
            next_jump += JUMP_AFTER;
            if st.on_ground {
                jump_hold = JUMP_HOLD_TICKS;
            }
        }
    }
    if path.last() != Some(&arr(st.pos)) {
        path.push(arr(st.pos));
    }
    // a flight the walk ended in counts as far as it went
    if let Some(t0) = top {
        largest_fall = largest_fall.max(t0 - st.pos.y);
    }
    let target = V3::from_array(r.to);
    RouteReport {
        name: r.name.clone(),
        from: r.from,
        to: r.to,
        start: arr(start),
        start_floor,
        start_blocked,
        outcome: end,
        reached: end == RouteEnd::Reached,
        end: arr(st.pos),
        left: round1(xz_dist(st.pos, target)),
        seconds: round1(t),
        distance: round1(distance),
        jumps,
        largest_fall: round1(largest_fall),
        fall_damage: round1(worst_damage),
        died,
        stuck,
        path,
        end_piece: None,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// survey

/// What a survey column holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Column {
    /// A floor to stand on (its height).
    Floor(f32),
    /// No floor at all; a fall there ends in this kill volume (index into [`WalkWorld::kills`]) or nowhere.
    Hole(Option<usize>),
    /// The first floor (its height) puts a standing body inside this kill volume.
    KillFloor(f32, usize),
}

/// The columns of a survey, row by row (z), each row x0 to x1. The cells of `step` cover the box, centred on it,
/// so every column's centre lies inside the box.
#[derive(Debug, Clone, PartialEq)]
pub struct Survey {
    /// `[x0, z0, x1, z1]`.
    pub area: [f32; 4],
    pub step: f32,
    pub nx: usize,
    pub nz: usize,
    /// The x / z corner of cell (0, 0).
    pub origin: [f32; 2],
    pub columns: Vec<Column>,
}

impl Survey {
    /// The centre of column (i, j).
    pub fn centre(&self, i: usize, j: usize) -> (f32, f32) {
        let (a, _) = self.cell(i, j);
        (a.0 + 0.5 * self.step, a.1 + 0.5 * self.step)
    }

    /// The x / z corners of cell (i, j).
    pub fn cell(&self, i: usize, j: usize) -> ((f32, f32), (f32, f32)) {
        let a = (
            self.origin[0] + i as f32 * self.step,
            self.origin[1] + j as f32 * self.step,
        );
        (a, (a.0 + self.step, a.1 + self.step))
    }
}

/// Casts every column of `area` (`[x0, z0, x1, z1]`) at `step`.
pub fn survey(w: &WalkWorld, area: [f32; 4], step: f32) -> Result<Survey, String> {
    if !(step.is_finite() && step >= 1.0) {
        return Err(format!("the step {step} is not a number of at least 1"));
    }
    let [x0, z0, x1, z1] = area;
    if !(x0.is_finite() && z0.is_finite() && x1.is_finite() && z1.is_finite())
        || x1 <= x0
        || z1 <= z0
    {
        return Err(format!("the box {area:?} is empty"));
    }
    let nx = ((x1 - x0) / step).ceil() as usize;
    let nz = ((z1 - z0) / step).ceil() as usize;
    if nx.saturating_mul(nz) > SURVEY_MAX_COLUMNS {
        return Err(format!(
            "{nx} x {nz} columns are more than {SURVEY_MAX_COLUMNS}: take a larger step"
        ));
    }
    let standable = MovementParams::DEFAULT.standable_cos;
    let half = BODY[1];
    let mut s = Survey {
        area,
        step,
        nx,
        nz,
        origin: [
            x0 - (nx as f32 * step - (x1 - x0)) * 0.5,
            z0 - (nz as f32 * step - (z1 - z0)) * 0.5,
        ],
        columns: Vec::with_capacity(nx * nz),
    };
    for j in 0..nz {
        for i in 0..nx {
            let (x, z) = s.centre(i, j);
            let c = match w.first_floor(x, z, standable) {
                Some(y) => match w.kill_at(V3::new(x, y + half, z)) {
                    Some(k) => Column::KillFloor(y, k),
                    None => Column::Floor(y),
                },
                // the fall ends in the highest kill volume on the column
                None => Column::Hole(
                    w.kills
                        .iter()
                        .enumerate()
                        .filter_map(|(k, v)| v.on_column(x, z).map(|y| (k, y)))
                        .max_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(k, _)| k),
                ),
            };
            s.columns.push(c);
        }
    }
    Ok(s)
}

/// A connected area of one kind of column (4-neighbours).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Area {
    pub columns: usize,
    /// `[x, z]` corners of the columns' cells.
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// The kill volume the area's columns name most (a hole's fall ends in it, a kill floor lies in it).
    pub volume: Option<String>,
    /// The ROM piece whose region holds the area's centre.
    pub piece: Option<String>,
}

fn is_hole(c: &Column) -> bool {
    matches!(c, Column::Hole(_))
}

fn is_kill(c: &Column) -> bool {
    matches!(c, Column::KillFloor(..))
}

fn column_volume(c: &Column) -> Option<usize> {
    match c {
        Column::Hole(k) => *k,
        Column::KillFloor(_, k) => Some(*k),
        Column::Floor(_) => None,
    }
}

/// The connected areas of the columns `want` accepts, largest first.
pub fn areas(s: &Survey, w: &WalkWorld, want: fn(&Column) -> bool) -> Vec<Area> {
    let mut seen = vec![false; s.columns.len()];
    let mut out = Vec::new();
    for start in 0..s.columns.len() {
        if seen[start] || !want(&s.columns[start]) {
            continue;
        }
        seen[start] = true;
        let mut queue = VecDeque::from([start]);
        let (mut i0, mut j0, mut i1, mut j1) = (usize::MAX, usize::MAX, 0, 0);
        let mut n = 0;
        let mut votes: Vec<(usize, usize)> = Vec::new();
        while let Some(c) = queue.pop_front() {
            n += 1;
            let (i, j) = (c % s.nx, c / s.nx);
            i0 = i0.min(i);
            j0 = j0.min(j);
            i1 = i1.max(i);
            j1 = j1.max(j);
            if let Some(k) = column_volume(&s.columns[c]) {
                match votes.iter_mut().find(|v| v.0 == k) {
                    Some(v) => v.1 += 1,
                    None => votes.push((k, 1)),
                }
            }
            let mut next = Vec::with_capacity(4);
            if i > 0 {
                next.push(c - 1);
            }
            if i + 1 < s.nx {
                next.push(c + 1);
            }
            if j > 0 {
                next.push(c - s.nx);
            }
            if j + 1 < s.nz {
                next.push(c + s.nx);
            }
            for q in next {
                if !seen[q] && want(&s.columns[q]) {
                    seen[q] = true;
                    queue.push_back(q);
                }
            }
        }
        let volume = votes
            .iter()
            .max_by_key(|v| v.1)
            .map(|v| w.kills[v.0].name.clone());
        out.push(Area {
            columns: n,
            min: {
                let (a, _) = s.cell(i0, j0);
                [round1(a.0), round1(a.1)]
            },
            max: {
                let (_, b) = s.cell(i1, j1);
                [round1(b.0), round1(b.1)]
            },
            volume,
            piece: None,
        });
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.columns));
    out
}

// ---------------------------------------------------------------------------------------------------------------
// the picture

/// Maps a world x / z to picture pixels (x right, y down).
pub type ToPixel<'a> = dyn Fn(f32, f32) -> (f32, f32) + 'a;

/// The default picture size for a frame: the long side [`PICTURE_SIDE`], the other one by the frame's aspect.
pub fn picture_size(frame: [f32; 4]) -> (u32, u32) {
    let (w, h) = (
        (frame[2] - frame[0]).max(1.0),
        (frame[3] - frame[1]).max(1.0),
    );
    let side = PICTURE_SIDE as f32;
    if w >= h {
        (PICTURE_SIDE, ((side * h / w).round() as u32).max(64))
    } else {
        (((side * w / h).round() as u32).max(64), PICTURE_SIDE)
    }
}

/// The top view of `world` framed on `frame` (`[x0, z0, x1, z1]`), shaded by face as the flat `render --view top`
/// draws a composed world, the ROM pieces' regions outlined; `paint` then draws on it, given the mapping from world
/// x / z to pixels.
pub fn top_picture(
    world: &dc_world::World<'_>,
    p: &Project,
    frame: [f32; 4],
    size: (u32, u32),
    paint: &mut dyn FnMut(&mut RgbaImage, &ToPixel<'_>),
) -> Result<RgbaImage, Diagnostics> {
    let opts = RenderOptions {
        labels: true,
        shade_render: true,
        sun_dir: crate::geom::V3::from_array(p.settings.sun_dir),
        ..RenderOptions::default()
    };
    let mut scene = Scene::from_world(world, &opts);
    scene.markers.clear();
    scene.labels.clear();
    scene.spawns.clear();
    // the regions outlined, without the marker discs (the picture has no labels to name them)
    scene.add_pieces(p);
    scene.markers.clear();
    for b in scene.boxes.iter_mut() {
        b.2 = REGION_COLOR;
    }
    scene.extent_min.x = frame[0];
    scene.extent_min.z = frame[1];
    scene.extent_max.x = frame[2];
    scene.extent_max.z = frame[3];
    let r = render_sw::render_report(&scene, View::Top, size, &opts)?;
    let b = r.top_bounds.unwrap_or(frame);
    let mut img = r.image;
    let (w, h) = (img.width as f32, img.height as f32);
    let map = move |x: f32, z: f32| {
        (
            (x - b[0]) / (b[2] - b[0]) * w,
            (b[3] - z) / (b[3] - b[1]) * h,
        )
    };
    paint(&mut img, &map);
    Ok(img)
}

/// Blends a world rectangle (`x0, z0` .. `x1, z1`) into the picture.
pub(crate) fn fill_cell(
    img: &mut RgbaImage,
    map: &ToPixel<'_>,
    a: (f32, f32),
    b: (f32, f32),
    color: [u8; 3],
    alpha: f32,
) {
    let (p0, p1) = (map(a.0, b.1), map(b.0, a.1));
    let (px0, py0) = (p0.0.floor().max(0.0) as u32, p0.1.floor().max(0.0) as u32);
    let px1 = (p1.0.ceil().max(0.0) as u32).min(img.width);
    let py1 = (p1.1.ceil().max(0.0) as u32).min(img.height);
    for y in py0..py1 {
        for x in px0..px1 {
            img.blend(x, y, color, alpha);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the verb

/// The survey's area: a box or a ROM piece's region.
#[derive(Debug, Clone, PartialEq)]
pub enum AreaArg {
    Box([f32; 4]),
    Piece(String),
}

/// What `walk` does.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    Routes(Vec<Route>),
    Survey {
        area: Option<AreaArg>,
        step: f32,
    },
    /// The reachability analysis ([`crate::reach`]).
    Reach(crate::reach::ReachJob),
}

/// A `walk` request (the CLI verb and the MCP tool).
#[derive(Debug, Clone, PartialEq)]
pub struct WalkRequest {
    pub dir: PathBuf,
    pub rom: Option<PathBuf>,
    pub job: Job,
    pub options: WalkOptions,
    pub open_doors: bool,
    /// The picture's path: the survey writes `<dir>/build/walk/survey.png` without it (the reach analysis
    /// `reach.png`), routes draw only with it.
    pub out: Option<PathBuf>,
    pub size: Option<(u32, u32)>,
}

/// `x,y,z` (or `x z y` with spaces) as three numbers.
pub fn parse_point(s: &str) -> Result<[f32; 3], String> {
    let v = numbers(s)?;
    match v.as_slice() {
        [x, y, z] => Ok([*x, *y, *z]),
        _ => Err(format!("`{s}` is not x,y,z")),
    }
}

/// `x0,z0,x1,z1` (corners in any order) or a piece id.
pub fn parse_area(s: &str) -> Result<AreaArg, String> {
    if s.trim().starts_with("piece_") {
        return Ok(AreaArg::Piece(s.trim().to_string()));
    }
    let v = numbers(s)?;
    match v.as_slice() {
        [a, b, c, d] => Ok(AreaArg::Box([a.min(*c), b.min(*d), a.max(*c), b.max(*d)])),
        _ => Err(format!("`{s}` is not x0,z0,x1,z1 or a piece id")),
    }
}

fn numbers(s: &str) -> Result<Vec<f32>, String> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| {
            t.parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("`{t}` is not a number"))
        })
        .collect()
}

/// Parses a routes file's text.
pub fn parse_routes(text: &str) -> Result<Vec<Route>, String> {
    let routes: Vec<Route> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if routes.is_empty() {
        return Err("the list holds no route".into());
    }
    for (i, r) in routes.iter().enumerate() {
        let finite = r
            .from
            .iter()
            .chain(r.to.iter())
            .chain(r.via.iter().flatten())
            .all(|v| v.is_finite());
        if !finite {
            return Err(format!("route {i} has a coordinate that is not a number"));
        }
    }
    Ok(routes)
}

/// The ROM piece whose region holds `p` (in 3D, else in x / z).
pub fn piece_at(project: &Project, p: [f32; 3]) -> Option<String> {
    let inside = |r: &crate::project::BoxF, three: bool| {
        p[0] >= r.min[0]
            && p[0] <= r.max[0]
            && p[2] >= r.min[2]
            && p[2] <= r.max[2]
            && (!three || (p[1] >= r.min[1] && p[1] <= r.max[1]))
    };
    let regions: Vec<(&str, crate::project::BoxF)> = project
        .pieces
        .iter()
        .map(|x| (x.id.as_str(), x.world_region()))
        .collect();
    regions
        .iter()
        .find(|(_, r)| inside(r, true))
        .or_else(|| regions.iter().find(|(_, r)| inside(r, false)))
        .map(|(id, _)| id.to_string())
}

fn invalid(path: &str, msg: impl Into<String>) -> Diagnostics {
    Diagnostics::one(Diagnostic::new(Code::OpInvalid, path, msg))
}

/// `walk`: the project built (and composed with its pieces from the ROM), then the routes or the survey.
pub fn walk_project(req: &WalkRequest) -> Result<Outcome, Diagnostics> {
    let (p, h) = Project::open(&req.dir)?;
    let rom = if p.pieces.is_empty() {
        None
    } else {
        match crate::romlist::discover_rom(req.rom.as_deref()) {
            Some((r, _)) => Some(r),
            None => return Err(crate::romlist::no_rom()),
        }
    };
    let area = request_area(req, &p)?;
    // composed as `build` and `render` compose it (the same call the loaders' composition mirrors)
    let built = build::build_with(&p, &h, &Assets::new(&req.dir), &p.build, rom.as_deref())?;
    walk_built(req, &p, built, area)
}

/// [`walk_project`] on a loaded project with the reader of the ROM's worlds itself (a test hands in its own).
pub fn walk_project_with(
    req: &WalkRequest,
    p: &Project,
    h: &Heights,
    read: Option<&mut dc_world::compose::RomRead<'_>>,
) -> Result<Outcome, Diagnostics> {
    let area = request_area(req, p)?;
    let built = build::build_with_reader(p, h, &Assets::new(&req.dir), &p.build, read)?;
    walk_built(req, p, built, area)
}

/// Checks the request before the (long) build; answers the survey's or the reach analysis's box when it names one.
fn request_area(req: &WalkRequest, p: &Project) -> Result<Option<[f32; 4]>, Diagnostics> {
    let piece = |id: &str, path: &str| -> Result<crate::project::BoxF, Diagnostics> {
        match p.pieces.iter().find(|x| x.id == id) {
            Some(x) => Ok(x.world_region()),
            None => Err(Diagnostics::one(
                Diagnostic::new(Code::IdUnknown, path, format!("no piece `{id}`"))
                    .with_fix("name a piece of `info` / map.json `pieces`, or give x0,z0,x1,z1"),
            )),
        }
    };
    let (area, step) = match &req.job {
        Job::Survey { area, step } => (area, *step),
        Job::Reach(r) => {
            if let crate::reach::Seed::Piece(id) = &r.seed {
                piece(id, "--seed-piece")?;
            }
            (&r.area, r.step)
        }
        Job::Routes(r) if r.is_empty() => return Err(invalid("--routes", "no route")),
        Job::Routes(_) => return Ok(None),
    };
    if !(step.is_finite() && step >= 1.0) {
        return Err(invalid("--step", format!("{step} (at least 1)")));
    }
    match area {
        Some(AreaArg::Piece(id)) => {
            let r = piece(id, "--box")?;
            Ok(Some([r.min[0], r.min[2], r.max[0], r.max[2]]))
        }
        Some(AreaArg::Box(b)) => Ok(Some(*b)),
        None => Ok(None),
    }
}

/// The routes or the survey on a built project.
fn walk_built(
    req: &WalkRequest,
    p: &Project,
    built: build::Built,
    area_arg: Option<[f32; 4]>,
) -> Result<Outcome, Diagnostics> {
    let composed = built.composed.is_some();
    let bytes: &[u8] = match (&built.composed, p.pieces.is_empty()) {
        (Some(c), _) => c,
        (None, true) => &built.dat,
        (None, false) => {
            let mut d = Diagnostics::one(
                Diagnostic::new(
                    Code::PieceRom,
                    "pieces",
                    "the ROM pieces could not be composed, so there is no world to walk",
                )
                .with_severity(Severity::Error)
                .with_fix("check the ROM (`--rom`, CA_ROM) and the pieces' worlds (`dc-mapkit rom worlds`)"),
            );
            d.extend(built.diagnostics);
            return Err(d);
        }
    };
    let picture = matches!(req.job, Job::Survey { .. } | Job::Reach(_)) || req.out.is_some();
    let parsed = if picture {
        dc_world::World::parse(bytes)
    } else {
        dc_world::World::parse_with(bytes, dc_world::ParseOptions { render: false })
    };
    let world = parsed
        .map_err(|e| Diagnostics::one(Diagnostic::new(Code::WorldParse, "world", e.to_string())))?;
    let w = WalkWorld::of(&world, req.open_doors);
    let mut diags = Diagnostics::new();
    // the build's own errors stay visible (the world may not be what players load)
    for d in built.diagnostics.iter() {
        if d.severity == Severity::Error {
            diags.push(d.clone());
        }
    }
    let world_info = json!({
        "composed": composed,
        "bytes": bytes.len(),
        "models": w.collision.models.len(),
        "blockers": w.collision.blocker_count(),
        "kill_volumes": w.kills.len(),
        "ladders": w.ladders.len(),
        "doors_open": w.doors_open,
        "extent_min": arr(w.extent_min),
        "extent_max": arr(w.extent_max),
        "floor_limit": round1(w.floor_limit),
    });
    match &req.job {
        Job::Reach(job) => {
            let Some(area) = area_arg.or(w.ground) else {
                return Err(invalid(
                    "--box",
                    "the world has no solid floor to analyse: give --box",
                ));
            };
            crate::reach::report(req, p, &world, &w, area, job, world_info, diags)
        }
        Job::Routes(routes) => {
            let mut reports = Vec::with_capacity(routes.len());
            let mut text = Vec::new();
            for (i, r) in routes.iter().enumerate() {
                let mut rep = walk_route(&w, r, &req.options);
                if rep.name.is_empty() {
                    rep.name = format!("route {}", i + 1);
                }
                rep.end_piece = piece_at(p, rep.end);
                if let Some(d) = rep.died.as_mut() {
                    d.piece = piece_at(p, d.at);
                }
                if let Some(s) = rep.stuck.as_mut() {
                    s.piece = piece_at(p, s.at);
                }
                let path = format!("routes[{i}]");
                route_diagnostics(&rep, &path, &mut diags);
                text.push(route_text(&rep));
                reports.push(rep);
            }
            let count = |e: RouteEnd| reports.iter().filter(|r| r.outcome == e).count();
            let summary = json!({
                "routes": reports.len(),
                "reached": count(RouteEnd::Reached),
                "died": count(RouteEnd::Died),
                "stuck": count(RouteEnd::Stuck),
                "timeout": count(RouteEnd::Timeout),
            });
            let mut result = json!({
                "world": world_info,
                "body": BODY,
                "tick_hz": TICK_HZ,
                "seconds_limit": req.options.seconds,
                "sprint": req.options.sprint,
                "summary": summary,
                "routes": reports,
                "image": Value::Null,
            });
            if let Some(out) = &req.out {
                let frame = routes_frame(&reports);
                let size = req.size.unwrap_or_else(|| picture_size(frame));
                let img = top_picture(&world, p, frame, size, &mut |img, map| {
                    draw_routes(img, map, &reports)
                })?;
                write_png(out, &img)?;
                result["image"] = json!(out.display().to_string());
                result["width"] = json!(img.width);
                result["height"] = json!(img.height);
                text.push(format!("picture {}", out.display()));
            }
            let mut o = Outcome::new(result, text.join("\n"));
            o.diagnostics = diags;
            Ok(o)
        }
        Job::Survey { step, .. } => {
            let area = match area_arg.or(w.ground) {
                Some(a) => a,
                None => {
                    return Err(invalid(
                        "--box",
                        "the world has no solid floor to survey: give --box",
                    ))
                }
            };
            let s = survey(&w, area, *step).map_err(|e| invalid("--box", e))?;
            let mut holes = areas(&s, &w, is_hole);
            let mut kills = areas(&s, &w, is_kill);
            for a in holes.iter_mut().chain(kills.iter_mut()) {
                let c = [
                    (a.min[0] + a.max[0]) * 0.5,
                    0.0,
                    (a.min[1] + a.max[1]) * 0.5,
                ];
                a.piece = piece_at(p, c);
            }
            let (mut hole_list, mut kill_list) = (Vec::new(), Vec::new());
            let (mut n_holes, mut n_kills, mut n_floors) = (0usize, 0usize, 0usize);
            for j in 0..s.nz {
                for i in 0..s.nx {
                    let (x, z) = s.centre(i, j);
                    match s.columns[j * s.nx + i] {
                        Column::Floor(_) => n_floors += 1,
                        Column::Hole(k) => {
                            n_holes += 1;
                            if hole_list.len() < LIST_CAP {
                                hole_list.push(json!({
                                    "x": round1(x), "z": round1(z),
                                    "kill_below": k.map(|k| w.kills[k].name.clone()),
                                }));
                            }
                        }
                        Column::KillFloor(y, k) => {
                            n_kills += 1;
                            if kill_list.len() < LIST_CAP {
                                kill_list.push(json!({
                                    "x": round1(x), "y": round1(y), "z": round1(z),
                                    "volume": w.kills[k].name,
                                }));
                            }
                        }
                    }
                }
            }
            let out = req.out.clone().unwrap_or_else(|| {
                req.dir
                    .join(crate::project::BUILD_DIR)
                    .join("walk")
                    .join("survey.png")
            });
            let size = req.size.unwrap_or_else(|| picture_size(area));
            let img = top_picture(&world, p, area, size, &mut |img, map| {
                for j in 0..s.nz {
                    for i in 0..s.nx {
                        let color = match s.columns[j * s.nx + i] {
                            Column::Floor(_) => continue,
                            Column::Hole(_) => HOLE_COLOR,
                            Column::KillFloor(..) => KILL_COLOR,
                        };
                        let (a, b) = s.cell(i, j);
                        fill_cell(img, map, a, b, color, 0.7);
                    }
                }
                let corners = [
                    map(area[0], area[1]),
                    map(area[2], area[1]),
                    map(area[2], area[3]),
                    map(area[0], area[3]),
                ];
                for k in 0..4 {
                    render_sw::draw_line(
                        img,
                        corners[k],
                        corners[(k + 1) % 4],
                        [255, 255, 255],
                        0.9,
                    );
                }
            })?;
            write_png(&out, &img)?;
            for a in holes.iter().take(DIAG_CAP) {
                let below = match &a.volume {
                    Some(v) => format!("a fall there ends in kill volume `{v}`"),
                    None => "nothing is below: a fall there leaves the world".to_string(),
                };
                diags.push(
                    Diagnostic::new(
                        Code::WalkHole,
                        area_path(a),
                        format!(
                            "{} columns with no floor at {}{}: {below}",
                            a.columns,
                            area_text(a),
                            in_piece(&a.piece)
                        ),
                    )
                    .with_fix("lay a floor of your own across it (a slab), or wall it off"),
                );
            }
            for a in kills.iter().take(DIAG_CAP) {
                diags.push(
                    Diagnostic::new(
                        Code::WalkKillFloor,
                        area_path(a),
                        format!(
                            "{} columns whose floor puts a standing body inside kill volume `{}` at {}{}",
                            a.columns,
                            a.volume.as_deref().unwrap_or("?"),
                            area_text(a),
                            in_piece(&a.piece)
                        ),
                    )
                    .with_fix("keep players off that floor, or cut the kill volume out with `piece.cut`"),
                );
            }
            let text = format!(
                "survey of x {}..{}, z {}..{} every {} units: {} columns, {} with a floor, {} holes in {} areas, {} kill floors in {} areas; picture {}",
                area[0],
                area[2],
                area[1],
                area[3],
                s.step,
                s.columns.len(),
                n_floors,
                n_holes,
                holes.len(),
                n_kills,
                kills.len(),
                out.display()
            );
            let truncated = holes.len() > AREA_CAP
                || kills.len() > AREA_CAP
                || n_holes > LIST_CAP
                || n_kills > LIST_CAP;
            holes.truncate(AREA_CAP);
            kills.truncate(AREA_CAP);
            let result = json!({
                "world": world_info,
                "box": [round1(area[0]), round1(area[1]), round1(area[2]), round1(area[3])],
                "step": s.step,
                "grid": [s.nx, s.nz],
                "columns": s.columns.len(),
                "floors": n_floors,
                "holes": n_holes,
                "kill_floors": n_kills,
                "hole_areas": holes,
                "kill_areas": kills,
                "hole_list": hole_list,
                "kill_list": kill_list,
                "truncated": truncated,
                "image": out.display().to_string(),
                "width": img.width,
                "height": img.height,
            });
            let mut o = Outcome::new(result, text);
            o.diagnostics = diags;
            Ok(o)
        }
    }
}

pub(crate) fn write_png(out: &Path, img: &RgbaImage) -> Result<(), Diagnostics> {
    write_atomic(out, &img.png()).map_err(|e| {
        Diagnostics::one(Diagnostic::new(
            Code::ProjectIo,
            out.display().to_string(),
            e.to_string(),
        ))
    })
}

/// The frame around every route's points, grown by 800 units.
fn routes_frame(reports: &[RouteReport]) -> [f32; 4] {
    let mut f = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for r in reports {
        for q in r.path.iter().chain([&r.from, &r.to]) {
            f[0] = f[0].min(q[0]);
            f[1] = f[1].min(q[2]);
            f[2] = f[2].max(q[0]);
            f[3] = f[3].max(q[2]);
        }
    }
    [f[0] - 800.0, f[1] - 800.0, f[2] + 800.0, f[3] + 800.0]
}

fn draw_routes(img: &mut RgbaImage, map: &ToPixel<'_>, reports: &[RouteReport]) {
    let r = ((img.width.min(img.height) as f32 / 512.0 * 4.0).round() as i32).clamp(3, 10);
    for rep in reports {
        let color = match rep.outcome {
            RouteEnd::Reached => REACHED_COLOR,
            RouteEnd::Died => HOLE_COLOR,
            RouteEnd::Stuck => STUCK_COLOR,
            RouteEnd::Timeout => TIMEOUT_COLOR,
        };
        let target = map(rep.to[0], rep.to[2]);
        let (tx, ty) = (target.0.round() as i32, target.1.round() as i32);
        render_sw::draw_disc(img, tx, ty, r, [40, 40, 48], [255, 255, 255]);
        for pair in rep.path.windows(2) {
            let a = map(pair[0][0], pair[0][2]);
            let b = map(pair[1][0], pair[1][2]);
            render_sw::draw_line(img, a, b, color, 1.0);
        }
        let s = map(rep.start[0], rep.start[2]);
        render_sw::draw_disc(
            img,
            s.0.round() as i32,
            s.1.round() as i32,
            r,
            [255, 255, 255],
            [20, 20, 24],
        );
        let e = map(rep.end[0], rep.end[2]);
        render_sw::draw_disc(
            img,
            e.0.round() as i32,
            e.1.round() as i32,
            r,
            color,
            [20, 20, 24],
        );
    }
}

fn area_path(a: &Area) -> String {
    format!("survey.{}", a.piece.as_deref().unwrap_or("ground"))
}

fn area_text(a: &Area) -> String {
    format!("x {}..{}, z {}..{}", a.min[0], a.max[0], a.min[1], a.max[1])
}

fn fmt_point(p: &[f32; 3]) -> String {
    format!("({}, {}, {})", p[0], p[1], p[2])
}

fn in_piece(piece: &Option<String>) -> String {
    piece
        .as_deref()
        .map(|p| format!(" in {p}"))
        .unwrap_or_default()
}

/// One line per route for the text output.
pub fn route_text(r: &RouteReport) -> String {
    let what = match r.outcome {
        RouteEnd::Reached => format!("reached in {} s", r.seconds),
        RouteEnd::Died => match &r.died {
            Some(d) => match d.cause.as_str() {
                "kill_volume" => format!(
                    "died in kill volume `{}` at {}{} after {} s",
                    d.volume.as_deref().unwrap_or("?"),
                    fmt_point(&d.at),
                    in_piece(&d.piece),
                    d.seconds
                ),
                "fall" => format!(
                    "died of a {} unit fall at {}{} after {} s",
                    d.height.unwrap_or(0.0),
                    fmt_point(&d.at),
                    in_piece(&d.piece),
                    d.seconds
                ),
                _ => format!(
                    "fell out of the world at {}{} after {} s",
                    fmt_point(&d.at),
                    in_piece(&d.piece),
                    d.seconds
                ),
            },
            None => "died".to_string(),
        },
        RouteEnd::Stuck => match &r.stuck {
            Some(s) => format!(
                "stuck at {}{} after {} s{}",
                fmt_point(&s.at),
                in_piece(&s.piece),
                s.seconds,
                s.blocked_by
                    .as_deref()
                    .map(|b| format!(" against {b}"))
                    .unwrap_or_default()
            ),
            None => "stuck".to_string(),
        },
        RouteEnd::Timeout => format!("out of time after {} s, {} units short", r.seconds, r.left),
    };
    format!(
        "{}: {what} ({} units walked, {} jumps, largest fall {}{})",
        r.name,
        r.distance,
        r.jumps,
        r.largest_fall,
        if r.start_floor {
            ""
        } else {
            "; no floor under the start"
        }
    )
}

/// The warning diagnostics of a route that did not get there.
fn route_diagnostics(r: &RouteReport, path: &str, d: &mut Diagnostics) {
    if !r.start_floor || r.start_blocked {
        d.push(
            Diagnostic::new(
                Code::WalkStart,
                path,
                format!(
                    "`{}`: {}",
                    r.name,
                    if r.start_blocked {
                        "the standing body overlaps solid geometry at the start"
                    } else {
                        "no floor within 1000 units under the start"
                    }
                ),
            )
            .with_fix("start from a point on an open floor"),
        );
    }
    let (code, fix) = match r.outcome {
        RouteEnd::Reached => return,
        RouteEnd::Died => (
            Code::WalkDied,
            "close the hole with a floor of your own, or keep the region's kill volumes out of reach",
        ),
        RouteEnd::Stuck => (
            Code::WalkStuck,
            "cut a doorway there (`piece.cut`), lay a slab or ramp, or route around it with `via` points",
        ),
        RouteEnd::Timeout => (Code::WalkTimeout, "give the route more `--seconds`, or check its path"),
    };
    let text = route_text(r);
    d.push(Diagnostic::new(code, path, text).with_fix(fix));
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_world::compose::{self, clip::Aabb, fixture};

    /// The fixture map composed with its piece (made-up worlds of `dc_world::compose::fixture`): the piece's floor
    /// at y 0 over x 9,000 .. 10,600, its wall at x 10,500 (200 high), its kill slab at y -500 .. -400 under the
    /// region, the map's own floor at x 12,000 .. 14,000 and nothing between.
    fn composed(cuts: Vec<Aabb>) -> Vec<u8> {
        let map = fixture::map(&fixture::piece(cuts));
        let mut read = fixture::reader();
        compose::compose(&map, &mut read).unwrap().unwrap().bytes
    }

    fn walk_world(bytes: &[u8]) -> WalkWorld {
        let w = dc_world::World::parse(bytes).unwrap();
        WalkWorld::of(&w, false)
    }

    fn route(from: [f32; 3], to: [f32; 3]) -> Route {
        Route {
            name: "test".into(),
            from,
            to,
            via: Vec::new(),
        }
    }

    /// The hole the cut leaves in the piece's floor.
    fn floor_cut() -> Aabb {
        Aabb::new([9_400.0, -50.0, 400.0], [9_800.0, 50.0, 800.0])
    }

    #[test]
    fn the_kill_test_is_the_servers_closed_model_test() {
        let bytes = composed(Vec::new());
        let w = walk_world(&bytes);
        assert_eq!(
            w.kills.len(),
            1,
            "{:?}",
            w.kills.iter().map(|k| &k.name).collect::<Vec<_>>()
        );
        let pit = &w.kills[0];
        assert!(pit.name.starts_with("piece_1~pit"), "{}", pit.name);
        assert!(pit.contains(V3::new(9_500.0, -450.0, 0.0)));
        assert!(!pit.contains(V3::new(9_500.0, -350.0, 0.0)), "above it");
        // clipped to the region: nothing of it past x 10,600
        assert!(!pit.contains(V3::new(10_800.0, -450.0, 0.0)));
        let y = pit.on_column(9_500.0, 0.0).expect("under the floor");
        assert!(y < -400.0 && y > -500.0, "{y}");
        assert!(pit.on_column(11_000.0, 0.0).is_none());
    }

    #[test]
    fn walking_across_the_piece_floor_reaches_the_target() {
        let bytes = composed(Vec::new());
        let w = walk_world(&bytes);
        let r = walk_route(
            &w,
            &route([9_300.0, 60.0, -500.0], [10_300.0, 60.0, -500.0]),
            &WalkOptions::default(),
        );
        assert_eq!(r.outcome, RouteEnd::Reached, "{r:?}");
        assert!(r.reached && r.start_floor && !r.start_blocked, "{r:?}");
        assert!(
            (r.start[1] - 50.0).abs() < 1.0,
            "on the floor: {:?}",
            r.start
        );
        assert!(r.left <= REACH);
        // a run of about 936 units at the run speed (285 a second) and the start ramp
        assert!(r.seconds > 3.0 && r.seconds < 4.5, "{}", r.seconds);
        assert!(r.distance > 900.0 && r.distance < 1_000.0, "{}", r.distance);
        assert!(r.largest_fall < 5.0, "{}", r.largest_fall);
        assert!(r.died.is_none() && r.stuck.is_none());
        assert!(r.path.len() >= 7, "{:?}", r.path);
    }

    #[test]
    fn walking_off_the_floor_into_the_void_falls_out_of_the_world() {
        let bytes = composed(Vec::new());
        let w = walk_world(&bytes);
        // west past the region's face at x 9,000: no floor and no kill volume below
        let r = walk_route(
            &w,
            &route([9_300.0, 60.0, -500.0], [8_000.0, 60.0, -500.0]),
            &WalkOptions::default(),
        );
        assert_eq!(r.outcome, RouteEnd::Died, "{r:?}");
        let d = r.died.as_ref().unwrap();
        assert_eq!(d.cause, "fell_out", "{d:?}");
        assert!(d.at[0] < 9_000.0 && d.at[1] < w.bottom + 1.0, "{d:?}");
    }

    #[test]
    fn walking_into_a_hole_in_the_floor_dies_in_the_kill_volume_below() {
        let bytes = composed(vec![floor_cut()]);
        let w = walk_world(&bytes);
        let r = walk_route(
            &w,
            &route([9_200.0, 60.0, 600.0], [10_200.0, 60.0, 600.0]),
            &WalkOptions::default(),
        );
        assert_eq!(r.outcome, RouteEnd::Died, "{r:?}");
        let d = r.died.as_ref().unwrap();
        assert_eq!(d.cause, "kill_volume", "{d:?}");
        assert!(
            d.volume.as_deref().unwrap().starts_with("piece_1~pit"),
            "{d:?}"
        );
        assert!(d.at[0] > 9_400.0 && d.at[0] < 9_800.0, "{d:?}");
        assert!(d.at[1] <= -399.0, "{d:?}");
    }

    #[test]
    fn walking_into_the_wall_gets_stuck_after_jumping() {
        let bytes = composed(Vec::new());
        let w = walk_world(&bytes);
        let r = walk_route(
            &w,
            &route([10_300.0, 60.0, -500.0], [11_000.0, 60.0, -500.0]),
            &WalkOptions::default(),
        );
        assert_eq!(r.outcome, RouteEnd::Stuck, "{r:?}");
        let s = r.stuck.as_ref().unwrap();
        assert!((s.at[0] - (10_500.0 - BODY[0])).abs() < 3.0, "{s:?}");
        assert_eq!(s.blocked_by.as_deref(), Some("level"), "{s:?}");
        assert!(r.jumps >= 2, "it tried to jump: {}", r.jumps);
        // the jumps' flights are short falls, nothing that hurts
        assert!(r.largest_fall > 20.0 && r.fall_damage == 0.0, "{r:?}");
    }

    #[test]
    fn a_waypoint_leads_round_the_wall_through_a_doorway() {
        // a doorway cut through the wall at z -50 .. 50, walked through by way of a point in front of it
        let door = Aabb::new([10_400.0, 1.0, -60.0], [10_700.0, 150.0, 60.0]);
        let bytes = composed(vec![door]);
        let w = walk_world(&bytes);
        let mut r = route([10_300.0, 60.0, -500.0], [10_580.0, 60.0, 0.0]);
        r.via = vec![[10_300.0, 60.0, 0.0]];
        let rep = walk_route(&w, &r, &WalkOptions::default());
        assert_eq!(rep.outcome, RouteEnd::Reached, "{rep:?}");
        assert!(rep.end[0] > 10_500.0, "through the wall: {:?}", rep.end);
    }

    #[test]
    fn a_lethal_fall_is_judged_by_the_servers_rule() {
        assert_eq!(fall_damage(299.0), 0.0);
        assert!((fall_damage(500.0) - 100.0).abs() < 1e-3);
        assert_eq!(fall_damage(5_000.0), FALL_D_HI);
    }

    #[test]
    fn the_survey_finds_the_holes_past_the_region_and_in_the_cut() {
        let bytes = composed(vec![floor_cut()]);
        let w = walk_world(&bytes);
        let s = survey(&w, [8_800.0, -1_000.0, 12_200.0, 1_000.0], 100.0).unwrap();
        assert_eq!((s.nx, s.nz), (34, 20));
        let holes: Vec<(f32, f32, Option<usize>)> = (0..s.nz)
            .flat_map(|j| (0..s.nx).map(move |i| (i, j)))
            .filter_map(|(i, j)| match s.columns[j * s.nx + i] {
                Column::Hole(k) => {
                    let (x, z) = s.centre(i, j);
                    Some((x, z, k))
                }
                _ => None,
            })
            .collect();
        // west of the region (2 columns), the gap to the map's floor (14 columns), the cut (4 x 4), all 20 deep
        assert_eq!(holes.len(), 2 * 20 + 14 * 20 + 16, "{holes:?}");
        for (x, z, k) in &holes {
            let in_cut = *x > 9_400.0 && *x < 9_800.0 && *z > 400.0 && *z < 800.0;
            assert!(
                in_cut || *x < 9_000.0 || (*x > 10_600.0 && *x < 12_000.0),
                "{x} {z}"
            );
            assert_eq!(
                k.is_some(),
                in_cut,
                "{x} {z}: only the cut has the pit below"
            );
        }
        let a = areas(&s, &w, is_hole);
        assert_eq!(a.len(), 3, "{a:?}");
        assert_eq!(a[0].columns, 280);
        assert_eq!(
            (a[0].min, a[0].max),
            ([10_600.0, -1_000.0], [12_000.0, 1_000.0])
        );
        assert!(a[0].volume.is_none());
        assert_eq!(a[2].columns, 16);
        assert!(a[2].volume.as_deref().unwrap().starts_with("piece_1~pit"));
        assert!(areas(&s, &w, is_kill).is_empty());
        // the floors are found at their height through the sky-less composed level
        let (i, j) = (12, 3);
        assert!(matches!(s.columns[j * s.nx + i], Column::Floor(y) if y.abs() < 1.0));
    }

    #[test]
    fn points_areas_and_routes_parse() {
        assert_eq!(
            parse_point("-10965,60,2700").unwrap(),
            [-10_965.0, 60.0, 2_700.0]
        );
        assert!(parse_point("1,2").is_err());
        assert_eq!(
            parse_area("10,-5,0,5").unwrap(),
            AreaArg::Box([0.0, -5.0, 10.0, 5.0])
        );
        assert_eq!(
            parse_area("piece_10").unwrap(),
            AreaArg::Piece("piece_10".into())
        );
        let r = parse_routes(r#"[{"name": "a", "from": [0, 60, 0], "to": [1, 60, 1]}]"#).unwrap();
        assert_eq!(r[0].name, "a");
        assert!(parse_routes("[]").is_err());
        assert!(parse_routes(r#"[{"from": [0, 0, 0], "to": [1, 1, 1], "speed": 3}]"#).is_err());
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dc-mapkit-walk-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The flat template with the fixture world as a piece at the origin whose region reaches 500 units west past
    /// the world's floor (the terrain is gone there, so a hole over the kill slabs), and a kill volume brush of the
    /// map's own standing on its terrain.
    fn project_with_piece(dir: &Path) -> (Project, Heights) {
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        let made = crate::template::make(crate::template::Template::Flat, "Walk", "tests", 1);
        for file in &made.files {
            std::fs::write(dir.join(&file.path), &file.bytes).unwrap();
        }
        let (mut p, mut h) = (made.project, made.heights);
        let ops: Vec<crate::ops::Op> = serde_json::from_value(json!([
            {"op": "piece.add", "world": fixture::WORLD, "pos": [0.0, 0.0, 0.0],
             "region": {"min": [-1_500.0, -600.0, -900.0], "max": [600.0, 1_100.0, 900.0]}},
            {"op": "solid.add", "kind": "box", "pos": [1_300.0, 40.0, 1_300.0],
             "size": [400.0, 80.0, 400.0], "material": "wall", "brush": "kill_volume"}
        ]))
        .unwrap();
        crate::ops::apply(&mut p, &mut h, &ops).unwrap();
        (p, h)
    }

    fn request(dir: &Path, job: Job) -> WalkRequest {
        WalkRequest {
            dir: dir.to_path_buf(),
            rom: None,
            job,
            options: WalkOptions::default(),
            open_doors: false,
            out: None,
            size: Some((256, 256)),
        }
    }

    #[test]
    fn a_project_with_a_piece_is_walked_and_surveyed_on_the_composed_world() {
        let dir = tmp("piece");
        let (p, h) = project_with_piece(&dir);
        let pid = p.pieces[0].id.clone();
        let own = format!("{pid}~");
        let routes = vec![
            // on the piece's floor, short of its wall at x 500
            route([-700.0, 60.0, 500.0], [300.0, 60.0, 500.0]),
            // from the map's terrain east into the region: the hole west of the world's floor
            route([-1_800.0, 60.0, 500.0], [-700.0, 60.0, 500.0]),
        ];
        let mut read = fixture::reader();
        let mut req = request(&dir, Job::Routes(routes));
        req.out = Some(dir.join("build").join("routes.png"));
        let o = walk_project_with(&req, &p, &h, Some(&mut read)).unwrap();
        let r = &o.result;
        assert_eq!(r["world"]["composed"], true, "{r}");
        assert_eq!(r["summary"]["reached"], 1, "{r}");
        assert_eq!(r["summary"]["died"], 1, "{r}");
        let a = &r["routes"][0];
        assert_eq!(a["outcome"], "reached", "{a}");
        assert_eq!(a["end_piece"], pid.as_str(), "{a}");
        let b = &r["routes"][1];
        assert_eq!(b["outcome"], "died", "{b}");
        assert_eq!(b["died"]["cause"], "kill_volume", "{b}");
        assert_eq!(b["died"]["piece"], pid.as_str(), "{b}");
        // the map's own kill slab (y -128 .. -64) lies above the piece's: the fall ends there
        let v = b["died"]["volume"].as_str().unwrap();
        assert!(!v.starts_with(&own), "{v}");
        assert!(b["died"]["at"][0].as_f64().unwrap() < -1_000.0, "{b}");
        let codes: Vec<&str> = o.diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(codes, ["walk.died"], "{:?}", o.diagnostics);
        assert!(dir.join("build").join("routes.png").exists());

        let mut read = fixture::reader();
        let req = request(
            &dir,
            Job::Survey {
                area: None,
                step: 280.0,
            },
        );
        let o = walk_project_with(&req, &p, &h, Some(&mut read)).unwrap();
        let r = &o.result;
        assert!(r["holes"].as_u64().unwrap() >= 4, "{r}");
        let holes = r["hole_areas"].as_array().unwrap();
        assert_eq!(holes.len(), 1, "{r}");
        assert_eq!(holes[0]["piece"], pid.as_str());
        assert!(
            holes[0]["max"][0].as_f64().unwrap() <= -1_000.0 + 280.0,
            "{r}"
        );
        assert!(!holes[0]["volume"].as_str().unwrap().starts_with(&own));
        assert!(r["kill_floors"].as_u64().unwrap() >= 1, "{r}");
        let kills = r["kill_areas"].as_array().unwrap();
        assert!(
            kills[0]["min"][0].as_f64().unwrap() >= 1_100.0 - 280.0,
            "{r}"
        );
        assert_eq!(kills[0]["piece"], Value::Null);
        let image = PathBuf::from(r["image"].as_str().unwrap());
        assert_eq!(image, dir.join("build").join("walk").join("survey.png"));
        assert!(image.exists());
        let codes: Vec<&str> = o.diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(
            codes,
            ["walk.hole", "walk.kill_floor"],
            "{:?}",
            o.diagnostics
        );

        // a survey of one piece's region, by its id
        let mut read = fixture::reader();
        let req = request(
            &dir,
            Job::Survey {
                area: Some(AreaArg::Piece(pid.clone())),
                step: 100.0,
            },
        );
        let o = walk_project_with(&req, &p, &h, Some(&mut read)).unwrap();
        assert_eq!(o.result["box"], json!([-1_500.0, -900.0, 600.0, 900.0]));
        assert_eq!(o.result["holes"], 5 * 18, "{}", o.result);
        let mut read = fixture::reader();
        let req = request(
            &dir,
            Job::Survey {
                area: Some(AreaArg::Piece("piece_9999".into())),
                step: 100.0,
            },
        );
        let Err(e) = walk_project_with(&req, &p, &h, Some(&mut read)) else {
            panic!("an unknown piece is refused");
        };
        assert_eq!(e.items[0].code, Code::IdUnknown);

        // the reach analysis of the piece's region, seeded at its spawns, its main component written out
        let nodes_file = dir.join("build").join("nodes.json");
        let reach_job = |seed: crate::reach::Seed| {
            Job::Reach(crate::reach::ReachJob {
                area: Some(AreaArg::Piece(pid.clone())),
                step: 100.0,
                seed,
                nodes_out: Some(nodes_file.clone()),
            })
        };
        let mut read = fixture::reader();
        let req = request(&dir, reach_job(crate::reach::Seed::Piece(pid.clone())));
        let o = walk_project_with(&req, &p, &h, Some(&mut read)).unwrap();
        let r = &o.result;
        assert_eq!(r["box"], json!([-1_500.0, -900.0, 600.0, 900.0]), "{r}");
        assert_eq!(r["main"]["seed"]["piece"], pid.as_str(), "{r}");
        let main = r["main"]["nodes"].as_u64().unwrap();
        assert!(main > 50, "{r}");
        assert!(r["main"]["spawns"].as_u64().unwrap() >= 1, "{r}");
        assert_eq!(r["traps"]["nodes"], 0, "{r}");
        // the floor's open edges kill (the hole west of the world's floor, the kill slabs)
        assert!(r["deadly"]["moves"].as_u64().unwrap() > 0, "{r}");
        assert_eq!(r["pieces"][0]["piece"], pid.as_str(), "{r}");
        assert_eq!(r["pieces"][0]["main"].as_u64().unwrap(), main, "{r}");
        assert!(r["pieces"][0]["spawns_main"].as_u64().unwrap() >= 1, "{r}");
        assert_eq!(r["spawns_outside_count"], 0, "{r}");
        let v: Value = serde_json::from_slice(&std::fs::read(&nodes_file).unwrap()).unwrap();
        assert_eq!(v["step"], 100.0);
        let list = v["nodes"].as_array().unwrap();
        assert_eq!(list.len() as u64, main);
        assert!(list.iter().all(|q| q.as_array().unwrap().len() == 3));
        let image = PathBuf::from(r["image"].as_str().unwrap());
        assert_eq!(image, dir.join("build").join("walk").join("reach.png"));
        assert!(image.exists());
        let codes: Vec<&str> = o.diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert!(codes.contains(&"walk.deadly"), "{codes:?}");
        // a seed piece that is not there is refused before the build
        let mut read = fixture::reader();
        let req = request(
            &dir,
            reach_job(crate::reach::Seed::Piece("piece_9999".into())),
        );
        let Err(e) = walk_project_with(&req, &p, &h, Some(&mut read)) else {
            panic!("an unknown seed piece is refused");
        };
        assert_eq!(e.items[0].code, Code::IdUnknown);
        assert_eq!(e.items[0].path, "--seed-piece");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cli_walks_and_surveys_a_template_map() {
        let dir = tmp("cli");
        let d = dir.to_str().unwrap();
        let run = |args: &[&str]| -> (i32, Value) {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let argv: Vec<String> = std::iter::once("dc-mapkit")
                .chain(args.iter().copied())
                .map(String::from)
                .collect();
            let code = crate::cli::run_cli(&argv, &mut out, &mut err);
            let text = String::from_utf8(out).unwrap();
            (
                code,
                serde_json::from_str(text.trim()).unwrap_or(Value::Null),
            )
        };
        let (code, _) = run(&[
            "new",
            d,
            "--template",
            "flat",
            "--name",
            "Walk Cli",
            "--json",
        ]);
        assert_eq!(code, 0);
        let (code, v) = run(&[
            "walk",
            d,
            "--from",
            "-600,60,-300",
            "--to",
            "600,60,-300",
            "--json",
        ]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["ok"], true);
        assert_eq!(v["verb"], "walk");
        assert_eq!(v["result"]["routes"][0]["outcome"], "reached", "{v}");
        assert_eq!(v["result"]["world"]["composed"], false);
        assert_eq!(v["result"]["image"], Value::Null);
        // into the boundary wall: stuck, a warning and exit 0
        let routes = dir.join("routes.json");
        std::fs::write(
            &routes,
            r#"[{"name": "wall", "from": [1800, 60, 0], "to": [2600, 60, 0]}]"#,
        )
        .unwrap();
        let (code, v) = run(&[
            "walk",
            d,
            "--routes",
            routes.to_str().unwrap(),
            "--seconds",
            "20",
            "--json",
        ]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["result"]["routes"][0]["outcome"], "stuck", "{v}");
        assert_eq!(v["result"]["routes"][0]["name"], "wall");
        assert_eq!(v["diagnostics"][0]["code"], "walk.stuck", "{v}");
        let (code, v) = run(&[
            "walk", d, "--survey", "--step", "600", "--size", "128x128", "--json",
        ]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["result"]["holes"], 0, "{v}");
        assert_eq!(v["result"]["width"], 128);
        assert!(PathBuf::from(v["result"]["image"].as_str().unwrap()).exists());
        // the reach analysis: one component over the template's floor, no trap
        let nodes = dir.join("nodes.json");
        let (code, v) = run(&[
            "walk",
            d,
            "--reach",
            "--step",
            "500",
            "--size",
            "128x128",
            "--nodes",
            nodes.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(code, 0, "{v}");
        let r = &v["result"];
        assert!(r["nodes"].as_u64().unwrap() > 0, "{v}");
        assert_eq!(r["traps"]["nodes"], 0, "{v}");
        assert_eq!(r["step"], 500.0);
        assert_eq!(r["width"], 128);
        assert!(PathBuf::from(r["image"].as_str().unwrap()).ends_with("build/walk/reach.png"));
        assert!(nodes.exists());
        let (code, v) = run(&[
            "walk", d, "--reach", "--seed", "0,60,0", "--step", "500", "--json",
        ]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["result"]["main"]["seed"]["by"], "point", "{v}");
        // a seed piece the project does not have, a seed off the grid, flags of --reach without it
        let (code, v) = run(&["walk", d, "--reach", "--seed-piece", "piece_1", "--json"]);
        assert_eq!(code, 1);
        assert_eq!(v["diagnostics"][0]["code"], "id.unknown", "{v}");
        let (code, v) = run(&[
            "walk",
            d,
            "--reach",
            "--box",
            "-500,-500,500,500",
            "--seed",
            "40000,0,0",
            "--step",
            "500",
            "--json",
        ]);
        assert_eq!(code, 1);
        assert_eq!(v["diagnostics"][0]["path"], "--seed", "{v}");
        let (code, v) = run(&["walk", d, "--survey", "--nodes", "x.json", "--json"]);
        assert_eq!(code, 1);
        assert_eq!(v["diagnostics"][0]["code"], "op.invalid", "{v}");
        // no job, or two: refused
        let (code, v) = run(&["walk", d, "--json"]);
        assert_eq!(code, 1);
        assert_eq!(v["diagnostics"][0]["code"], "op.invalid");
        let (code, _) = run(&["walk", d, "--survey", "--from", "0,0,0", "--json"]);
        assert_eq!(code, 1);
        let (code, v) = run(&["walk", d, "--from", "0,0", "--to", "1,1,1", "--json"]);
        assert_eq!(code, 1);
        assert_eq!(v["diagnostics"][0]["path"], "--from");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
