//! `dc-mapkit walk --reach`: where on a map a player can get to and back from, which moves kill, and where a player
//! who goes in can never get out (DC-DEFAULT: MAP-122).
//!
//! It runs on the walk world of [`crate::walk`]: the project built in memory and composed with its ROM pieces, the
//! client's collision and ladders, the game server's kill volumes and fall rule.
//!
//! - **Nodes.** On a grid of `step` units over the box, each column is cast down from the top of the world's extents
//!   again and again; sky faces and surfaces too steep to stand on are passed. The standing body ([`walk::BODY`]) is
//!   put on every floor hit: swept down from just over the floor, or from one step over it where that overlaps (the
//!   edge of a stair). A floor the body fits on is a node at the body's resting height. A node whose standing body
//!   is inside a kill volume is deadly.
//! - **Moves.** From every node toward each of its 8 neighbour columns. Two nodes on one horizontal plane are joined
//!   both ways without a simulation when the floor lies on that plane under every [`FLAT_SAMPLE`] units of the line
//!   between them, the standing body lifted by one step sweeps clear from one to the other, no kill volume is on the
//!   way and no ladder is near (the flat walk: the body would walk straight there). Every other move is simulated
//!   with the client's mover (`dc_sim::PlayerState::step` at 60 Hz): the body starts on the node running toward the
//!   neighbour at the run speed, forward and run held; it jumps when it gains less than [`STALL_UNITS`] along the
//!   move in [`STALL_TICKS`] ticks on the ground, and gives up when that happens again after the jump. The move ends
//!   on the first node it stands on in the neighbour column, on any node it stands on after a drop of more than
//!   [`DROP`], off a ladder or outside the 3 x 3 columns round the start, after [`MOVE_SECONDS`] on the ground
//!   ([`FALL_SECONDS`] while it falls, [`LADDER_SECONDS`] after a ladder), or in a death: a kill volume entered (the
//!   server's test, every 16 units of travel), a landing the server's fall rule makes lethal, or a body below
//!   everything. A move that drops or dies is tried again as a running jump from the start (a gap jumped), a move
//!   that touched a ladder again climbing down. Where the body ends is the node of the column it stands in, by the
//!   height of its feet (a drop lands on the node actually under it).
//! - **Analysis.** The strongly connected components of the moves (deadly nodes left out). The main component holds
//!   the `--seed` point, or the largest component holding a spawn of `--seed-piece`, or else the most player spawns
//!   (a tie: the larger). A node the main component reaches that cannot get back to it is a trap; a node it does not
//!   reach is an island. Moves from the main component that kill are the deadly moves. Traps, deadly moves and
//!   islands are joined into areas along the moves between their nodes.

use crate::cli::Outcome;
use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::project::{write_atomic, Project};
use crate::render_sw::{self, RgbaImage};
use crate::walk::{self, WalkRequest, WalkWorld, BODY, TICK_HZ};
use dc_sim::{
    bits, Collider, Commands, Container, Cylinder, MovementParams, PlayerState, TickInput, V3,
};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// The reach grid's spacing unless `--step` says otherwise.
// DC-DEFAULT: MAP-122
pub const REACH_STEP: f32 = 48.0;
/// Most nodes one analysis keeps (a smaller step over a larger box is refused).
// DC-DEFAULT: MAP-122
pub const MAX_NODES: usize = 8_000_000;
/// Most casts down one column (each passes a floor, a sky face or a steep surface).
// DC-DEFAULT: MAP-122
const NODE_CASTS: usize = 64;
/// The standing body is first put this far over a floor hit (then swept down onto it).
// DC-DEFAULT: MAP-122
const STAND_LIFT: f32 = 0.6;
/// A move's simulation ends after this long on the ground, this long in all while the body has been falling, and
/// this long in all after it touched a ladder (seconds).
// DC-DEFAULT: MAP-122
pub const MOVE_SECONDS: f32 = 1.5;
pub const FALL_SECONDS: f32 = 4.0;
pub const LADDER_SECONDS: f32 = 8.0;
/// A body on the ground stands on a node of its column when its feet are within this many units of the node's
/// floor (two nodes of one column are a standing body apart).
// DC-DEFAULT: MAP-122
pub const NODE_MATCH: f32 = 40.0;
/// A landing after a fall of more than this many units ends the move wherever the body stands.
// DC-DEFAULT: MAP-122
pub const DROP: f32 = 40.0;
/// The flat walk: floor heights within `FLAT_DY` of each other, floor normals at least `FLAT_NY` up, the floor
/// sampled every `FLAT_SAMPLE` units of the line (narrower than the body, so a gap it would fall into is seen).
// DC-DEFAULT: MAP-122
pub const FLAT_DY: f32 = 0.5;
pub const FLAT_NY: f32 = 0.999;
pub const FLAT_SAMPLE: f32 = 12.0;
/// A ladder volume this near a column (x / z, units) keeps its moves simulated.
// DC-DEFAULT: MAP-122
const LADDER_NEAR: f32 = 64.0;
/// The stall that presses jump, and after the jump ends the move: less than `STALL_UNITS` gained along the move in
/// `STALL_TICKS` ticks standing.
// DC-DEFAULT: MAP-122
pub const STALL_TICKS: usize = 4;
pub const STALL_UNITS: f32 = 2.0;
/// Most entry moves listed per trap area, sample moves per deadly area, spawns listed outside the main component.
// DC-DEFAULT: MAP-122
pub const ENTRY_CAP: usize = 20;
pub const SAMPLE_CAP: usize = 10;
pub const SPAWN_CAP: usize = 200;
/// Nodes per work item of the threads.
const CHUNK: usize = 256;
/// Colours of the picture: main, trap, deadly, island nodes, entry moves.
pub const MAIN_COLOR: [u8; 3] = [60, 200, 90];
pub const TRAP_COLOR: [u8; 3] = walk::HOLE_COLOR;
pub const DEADLY_COLOR: [u8; 3] = walk::KILL_COLOR;
pub const ISLAND_COLOR: [u8; 3] = [150, 150, 158];
pub const ENTRY_COLOR: [u8; 3] = [255, 235, 40];

/// The eight neighbour directions as (x step, z step): east, north-east, north, north-west, west, south-west,
/// south, south-east; `d` and `(d + 4) % 8` are opposite.
pub const DIRS: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

// ---------------------------------------------------------------------------------------------------------------
// the grid and its nodes

/// The columns over a box: cells of `step` centred on it, so every column's centre lies inside the box (as the
/// survey lays them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    /// `[x0, z0, x1, z1]`.
    pub area: [f32; 4],
    pub step: f32,
    pub nx: usize,
    pub nz: usize,
    /// The x / z corner of column 0.
    pub origin: [f32; 2],
}

impl Grid {
    pub fn new(area: [f32; 4], step: f32) -> Result<Grid, String> {
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
        if nx.saturating_mul(nz) > walk::SURVEY_MAX_COLUMNS {
            return Err(format!(
                "{nx} x {nz} columns are more than {}: take a larger step",
                walk::SURVEY_MAX_COLUMNS
            ));
        }
        Ok(Grid {
            area,
            step,
            nx,
            nz,
            origin: [
                x0 - (nx as f32 * step - (x1 - x0)) * 0.5,
                z0 - (nz as f32 * step - (z1 - z0)) * 0.5,
            ],
        })
    }

    pub fn columns(&self) -> usize {
        self.nx * self.nz
    }

    pub fn ij(&self, c: usize) -> (usize, usize) {
        (c % self.nx, c / self.nx)
    }

    /// The x / z centre of column `c`.
    pub fn centre(&self, c: usize) -> (f32, f32) {
        let (i, j) = self.ij(c);
        (
            self.origin[0] + (i as f32 + 0.5) * self.step,
            self.origin[1] + (j as f32 + 0.5) * self.step,
        )
    }

    /// The x / z corners of column `c`'s cell.
    pub fn cell(&self, c: usize) -> ((f32, f32), (f32, f32)) {
        let (i, j) = self.ij(c);
        let a = (
            self.origin[0] + i as f32 * self.step,
            self.origin[1] + j as f32 * self.step,
        );
        (a, (a.0 + self.step, a.1 + self.step))
    }

    /// The column whose cell holds (x, z).
    pub fn column_at(&self, x: f32, z: f32) -> Option<usize> {
        let fi = ((x - self.origin[0]) / self.step).floor();
        let fj = ((z - self.origin[1]) / self.step).floor();
        if !(fi >= 0.0 && fj >= 0.0 && fi < self.nx as f32 && fj < self.nz as f32) {
            return None;
        }
        Some(fj as usize * self.nx + fi as usize)
    }

    /// The neighbour of column `c` in direction `d` ([`DIRS`]).
    pub fn neighbour(&self, c: usize, d: usize) -> Option<usize> {
        let (i, j) = self.ij(c);
        let (di, dj) = DIRS[d];
        let (ni, nj) = (i as i64 + di as i64, j as i64 + dj as i64);
        if ni < 0 || nj < 0 || ni >= self.nx as i64 || nj >= self.nz as i64 {
            return None;
        }
        Some(nj as usize * self.nx + ni as usize)
    }
}

/// One floor a standing body fits on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub col: u32,
    /// The standing body's resting height less its half height.
    pub floor: f32,
    /// On a horizontal floor (the flat walk may join it).
    pub flat: bool,
    /// The kill volume (index into [`WalkWorld::kills`]) the standing body is inside: a deadly node.
    pub kill: Option<u32>,
}

/// Every node of a grid, column by column, each column's from the top down.
#[derive(Debug, Clone, PartialEq)]
pub struct Nodes {
    pub grid: Grid,
    pub list: Vec<Node>,
    /// The nodes of column `c` are `list[start[c]..start[c + 1]]`.
    start: Vec<u32>,
}

impl Nodes {
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn of_column(&self, c: usize) -> std::ops::Range<usize> {
        self.start[c] as usize..self.start[c + 1] as usize
    }

    /// The node's point: its column's centre on its floor.
    pub fn pos(&self, n: usize) -> V3 {
        let nd = self.list[n];
        let (x, z) = self.grid.centre(nd.col as usize);
        V3::new(x, nd.floor, z)
    }

    /// The standing body's centre on the node.
    pub fn rest(&self, n: usize) -> V3 {
        self.pos(n) + V3::new(0.0, BODY[1], 0.0)
    }

    /// The node of column `c` whose floor is nearest `feet`, within [`NODE_MATCH`].
    pub fn match_feet(&self, c: usize, feet: f32) -> Option<u32> {
        self.of_column(c)
            .map(|n| (n, (self.list[n].floor - feet).abs()))
            .filter(|&(_, d)| d <= NODE_MATCH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(n, _)| n as u32)
    }

    /// The node a standing body centred at `p` stands on.
    pub fn at(&self, p: V3) -> Option<u32> {
        let c = self.grid.column_at(p.x, p.z)?;
        self.match_feet(c, p.y - BODY[1])
    }
}

/// Runs `f(0) .. f(count - 1)` on `threads` scoped threads and returns the answers in order.
fn parallel<T: Send>(count: usize, threads: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let mut parts: Vec<(usize, T)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads.clamp(1, count.max(1)))
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        if k >= count {
                            break;
                        }
                        out.push((k, f(k)));
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a reach worker panicked"))
            .collect()
    });
    parts.sort_by_key(|p| p.0);
    parts.into_iter().map(|p| p.1).collect()
}

/// The number of threads the analysis runs on: every core.
pub fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 64)
}

/// Where a standing body comes to rest on the floor hit at (x, y, z): swept down from [`STAND_LIFT`] over it, or
/// from one step over it when that start overlaps (a stair's edge, where the body stands on the step above); `None`
/// when it fits on neither. Returns the floor the body stands on (its centre less its half height).
fn stand_on(
    w: &WalkWorld,
    shape: &Cylinder,
    p: &MovementParams,
    x: f32,
    y: f32,
    z: f32,
) -> Option<f32> {
    let half = shape.half_height;
    for lift in [STAND_LIFT, p.step()] {
        let start = V3::new(x, y + half + lift, z);
        if w.collision.overlaps(shape, start) {
            continue;
        }
        let end = V3::new(x, y + half - 2.0, z);
        let Some(h) = w.collision.sweep(shape, start, end) else {
            continue;
        };
        let rest = start + (end - start) * h.t;
        if w.collision.overlaps(shape, rest + V3::new(0.0, 0.5, 0.0)) {
            continue;
        }
        return Some(rest.y - half);
    }
    None
}

/// The floors of one column a standing body fits on, from the top down: `(floor, flat)`.
fn column_floors(
    w: &WalkWorld,
    shape: &Cylinder,
    p: &MovementParams,
    x: f32,
    z: f32,
    out: &mut Vec<(f32, bool)>,
) {
    out.clear();
    let bottom = w.bottom;
    let mut top = w.extent_max.y + 1.0;
    for _ in 0..NODE_CASTS {
        if top <= bottom {
            break;
        }
        let from = V3::new(x, top, z);
        let to = V3::new(x, bottom, z);
        let Some(hit) = w.collision.ray_pruned(from, to) else {
            break;
        };
        let y = top + (bottom - top) * hit.t;
        top = y - 0.5;
        if hit.surface_flags & crate::geom::SURF_SKY != 0 || hit.face_normal.y < p.standable_cos {
            continue;
        }
        let Some(floor) = stand_on(w, shape, p, x, y, z) else {
            continue;
        };
        if out.iter().any(|&(f, _)| (f - floor).abs() < 1.0) {
            continue;
        }
        let flat = hit.face_normal.y >= FLAT_NY && (floor - y).abs() <= FLAT_DY;
        out.push((floor, flat));
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
}

/// Every node of `grid`.
pub fn find_nodes(w: &WalkWorld, grid: Grid, threads: usize) -> Result<Nodes, String> {
    let shape = Cylinder::from_dims(V3::from_array(BODY));
    let p = MovementParams::DEFAULT;
    let rows = parallel(grid.nz, threads, |j| {
        let mut row: Vec<Node> = Vec::new();
        let mut floors = Vec::new();
        for i in 0..grid.nx {
            let c = j * grid.nx + i;
            let (x, z) = grid.centre(c);
            column_floors(w, &shape, &p, x, z, &mut floors);
            for &(floor, flat) in &floors {
                let rest = V3::new(x, floor + BODY[1], z);
                row.push(Node {
                    col: c as u32,
                    floor,
                    flat,
                    kill: w.kill_at(rest).map(|k| k as u32),
                });
            }
        }
        row
    });
    let total: usize = rows.iter().map(Vec::len).sum();
    if total > MAX_NODES {
        return Err(format!(
            "{total} floors are more than {MAX_NODES}: take a larger step or a smaller box"
        ));
    }
    let mut list = Vec::with_capacity(total);
    for r in rows {
        list.extend(r);
    }
    let mut start = vec![0u32; grid.columns() + 1];
    for n in &list {
        start[n.col as usize + 1] += 1;
    }
    for c in 0..grid.columns() {
        start[c + 1] += start[c];
    }
    Ok(Nodes { grid, list, start })
}

// ---------------------------------------------------------------------------------------------------------------
// moves

/// How a move was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MoveKind {
    /// The flat walk (no simulation).
    Walk,
    /// Simulated running, with a jump when blocked.
    Run,
    /// Simulated running jump from the start.
    Jump,
    /// Simulated, climbing down a ladder.
    Climb,
}

impl MoveKind {
    pub fn text(self) -> &'static str {
        match self {
            MoveKind::Walk => "walk",
            MoveKind::Run => "run",
            MoveKind::Jump => "jump",
            MoveKind::Climb => "climb",
        }
    }
}

/// A move from one node that ends standing on another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Move {
    pub from: u32,
    pub to: u32,
    pub kind: MoveKind,
    /// The largest fall on the way (top of a flight to its landing).
    pub fall: f32,
}

/// What killed a body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cause {
    /// Entered this kill volume (index into [`WalkWorld::kills`]).
    KillVolume(u32),
    /// A landing the fall rule makes lethal, after a fall of this height.
    Fall(f32),
    /// Below the world's extents, every floor and every kill volume.
    FellOut,
}

/// A move from a node that kills.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeadlyMove {
    pub from: u32,
    pub dir: u8,
    pub kind: MoveKind,
    pub cause: Cause,
    /// Where the body died.
    pub at: V3,
    /// The fall so far.
    pub fall: f32,
}

/// What the move search did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoveStats {
    /// Moves found by the flat walk.
    pub flat_walks: usize,
    /// Moves simulated, by how they started.
    pub runs: usize,
    pub jumps: usize,
    pub climbs: usize,
    /// Simulated moves that ended on no node (blocked, out of time, outside the box or on ground without a node).
    pub nowhere: usize,
    /// Mover steps run.
    pub ticks: u64,
}

/// Every move of a grid's nodes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Moves {
    /// Sorted by `from`, then `to`; one per pair (the first kind found).
    pub moves: Vec<Move>,
    /// Sorted by `from`, then direction and kind.
    pub deadly: Vec<DeadlyMove>,
    pub stats: MoveStats,
}

/// How a simulated move starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    Run,
    Jump,
    LadderDown,
}

/// How a simulated move ended.
#[derive(Debug, Clone, Copy, PartialEq)]
enum End {
    At { node: u32, fall: f32 },
    Died { cause: Cause, at: V3, fall: f32 },
    Nowhere,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Sim {
    end: End,
    /// It fell more than [`DROP`] or died: worth a running jump.
    dropped: bool,
    /// It touched a ladder.
    ladder: bool,
    ticks: u32,
}

/// The columns a ladder volume comes near (their moves stay simulated: the mover climbs there).
fn ladder_columns(w: &WalkWorld, g: &Grid) -> Vec<bool> {
    let mut out = vec![false; g.columns()];
    for (v, _) in &w.ladders.volumes {
        let (lo, hi) = (v.lo, v.hi);
        for (c, near) in out.iter_mut().enumerate() {
            let (x, z) = g.centre(c);
            if x >= lo.x - LADDER_NEAR
                && x <= hi.x + LADDER_NEAR
                && z >= lo.z - LADDER_NEAR
                && z <= hi.z + LADDER_NEAR
            {
                *near = true;
            }
        }
    }
    out
}

/// Whether the body walks straight from node `a` to node `b` and from `b` to `a` on one horizontal plane (see the
/// module notes). The two ways are swept apart: a collision face stops only a body moving against its front, so a
/// wall can be one way.
fn flat_walk(
    w: &WalkWorld,
    nodes: &Nodes,
    ladder: &[bool],
    shape: &Cylinder,
    a: usize,
    b: usize,
) -> (bool, bool) {
    let (na, nb) = (nodes.list[a], nodes.list[b]);
    if ladder[na.col as usize] || ladder[nb.col as usize] {
        return (false, false);
    }
    let (ra, rb) = (nodes.rest(a), nodes.rest(b));
    let y = na.floor;
    let len = ((rb.x - ra.x).powi(2) + (rb.z - ra.z).powi(2)).sqrt();
    let k = (len / FLAT_SAMPLE).ceil().max(1.0) as usize;
    for s in 1..k {
        let q = ra + (rb - ra) * (s as f32 / k as f32);
        let from = V3::new(q.x, y + BODY[1], q.z);
        let to = V3::new(q.x, y - 2.0, q.z);
        let Some(h) = w.collision.ray_pruned(from, to) else {
            return (false, false);
        };
        let hy = from.y + (to.y - from.y) * h.t;
        // the node's floor is the body's rest, a tenth over the face (the sweep's back-off)
        if (hy - y).abs() > FLAT_DY + 0.2
            || h.face_normal.y < FLAT_NY
            || h.surface_flags & crate::geom::SURF_SKY != 0
        {
            return (false, false);
        }
    }
    if w.kill_between(ra, rb).is_some() {
        return (false, false);
    }
    let up = V3::new(0.0, MovementParams::DEFAULT.step(), 0.0);
    let clear = |p: V3, q: V3| {
        !w.collision.overlaps(shape, p + up) && w.collision.sweep(shape, p + up, q + up).is_none()
    };
    (clear(ra, rb), clear(rb, ra))
}

/// The node a body standing at `p` stands on: the node of its column by the height of its feet, else the nearest
/// node at that height in the columns round it that the body, one step up, sweeps to in a straight line (a body by
/// a wall stands where its column's centre is inside the wall).
fn standing_node(w: &WalkWorld, nodes: &Nodes, shape: &Cylinder, p: V3) -> Option<u32> {
    if let Some(n) = nodes.at(p) {
        return Some(n);
    }
    let c = nodes.grid.column_at(p.x, p.z)?;
    let feet = p.y - BODY[1];
    let mut near: Vec<(f32, u32)> = (0..8)
        .filter_map(|d| nodes.grid.neighbour(c, d))
        .filter_map(|nc| nodes.match_feet(nc, feet))
        .map(|n| {
            let r = nodes.rest(n as usize);
            ((r.x - p.x).powi(2) + (r.z - p.z).powi(2), n)
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let up = V3::new(0.0, MovementParams::DEFAULT.step(), 0.0);
    near.into_iter().map(|(_, n)| n).find(|&n| {
        w.collision
            .sweep(shape, p + up, nodes.rest(n as usize) + up)
            .is_none()
    })
}

/// Simulates one move from node `from` toward its neighbour column in direction `dir` (see the module notes).
fn simulate(
    w: &WalkWorld,
    nodes: &Nodes,
    from: u32,
    dir: usize,
    how: Start,
    containers: &mut Vec<Container>,
) -> Sim {
    let p = MovementParams::DEFAULT;
    let dims = V3::from_array(BODY);
    let shape = Cylinder::from_dims(dims);
    let g = &nodes.grid;
    let a = nodes.list[from as usize];
    let start = nodes.rest(from as usize);
    let (di, dj) = DIRS[dir];
    let d = V3::new(di as f32, 0.0, dj as f32).normalized();
    let towards = g.neighbour(a.col as usize, dir);
    let (ai, aj) = g.ij(a.col as usize);
    // yaw 0 looks along +z (walk_route's steering)
    let yaw = d.x.atan2(d.z);
    let dt = 1.0 / TICK_HZ as f32;
    let ground_ticks = (MOVE_SECONDS * TICK_HZ as f32).round() as u32;
    let fall_ticks = (FALL_SECONDS * TICK_HZ as f32).round() as u32;
    let ladder_ticks = (LADDER_SECONDS * TICK_HZ as f32).round() as u32;
    let mut st = PlayerState::new(start, dims, &p);
    // settle on the floor: one step without commands
    w.ladders.touched(&st.shape(), st.pos, containers);
    st.step(
        &p,
        &w.collision,
        &TickInput {
            yaw,
            ctrl_state: 1,
            containers,
            ..TickInput::default()
        },
        dt,
    );
    // a player who has been running this way: the run speed along the move, forward and run held before
    // DC-DEFAULT: MAP-122
    st.vel = V3::new(d.x * p.run_forward, st.vel.y, d.z * p.run_forward);
    st.controls = bits::FWD | bits::RUN | bits::ANY_MOVE;
    st.ramp_fwd = p.max_accel;
    let mut sim = Sim {
        end: End::Nowhere,
        dropped: false,
        ladder: false,
        ticks: 1,
    };
    let mut jump_hold = if how == Start::Jump {
        walk::JUMP_HOLD_TICKS
    } else {
        0
    };
    let mut jumped = false;
    let mut top: Option<f32> = None;
    let mut fall = 0.0f32;
    let mut airborne = false;
    let mut was_on_ladder = false;
    let mut along_at = [0.0f32; STALL_TICKS];
    let mut tick = 0u32;
    loop {
        tick += 1;
        sim.ticks += 1;
        w.ladders.touched(&st.shape(), st.pos, containers);
        let pitch = if st.on_ladder {
            if how == Start::LadderDown {
                walk::LADDER_PITCH
            } else {
                -walk::LADDER_PITCH
            }
        } else {
            0.0
        };
        let input = TickInput {
            commands: Commands {
                forward: true,
                run: true,
                jump: jump_hold > 0,
                ..Commands::default()
            },
            yaw,
            pitch,
            ctrl_state: 1,
            containers,
            ..TickInput::default()
        };
        let before = st.pos;
        let ev = st.step(&p, &w.collision, &input, dt);
        if ev.jumped {
            jumped = true;
            jump_hold = 0;
        } else {
            jump_hold = jump_hold.saturating_sub(1);
        }
        if !st.pos.is_finite() {
            return sim;
        }
        if let Some((k, at)) = w.kill_between(before, st.pos) {
            sim.end = End::Died {
                cause: Cause::KillVolume(k as u32),
                at,
                fall: top.map_or(0.0, |t0| (t0.max(before.y) - at.y).max(0.0)),
            };
            sim.dropped = true;
            return sim;
        }
        // the fall track, judged at the landing (DSV-76), as walk_route keeps it
        let grounded = st.on_ladder || st.contact.is_some_and(|c| c.normal.y >= p.ground_cos);
        let mut landed = false;
        if grounded {
            if let Some(t0) = top.take() {
                let h = t0 - st.pos.y;
                fall = fall.max(h);
                if walk::fall_damage(h) >= walk::START_HEALTH {
                    sim.end = End::Died {
                        cause: Cause::Fall(h),
                        at: st.pos,
                        fall,
                    };
                    sim.dropped = true;
                    return sim;
                }
                if h > DROP {
                    landed = true;
                    sim.dropped = true;
                }
            }
        } else {
            top = Some(top.unwrap_or(before.y).max(st.pos.y));
            airborne = true;
        }
        if st.pos.y < w.bottom {
            sim.end = End::Died {
                cause: Cause::FellOut,
                at: st.pos,
                fall: top.map_or(0.0, |t0| (t0 - st.pos.y).max(0.0)),
            };
            sim.dropped = true;
            return sim;
        }
        sim.ladder |= st.on_ladder;
        let left_ladder = was_on_ladder && !st.on_ladder;
        was_on_ladder = st.on_ladder;
        // standing on a node: the end when it is the neighbour's, after a drop, off a ladder or far from the start
        let standing = grounded && !st.on_ladder;
        if standing {
            if let Some(c) = g.column_at(st.pos.x, st.pos.z) {
                let (ci, cj) = g.ij(c);
                let far = ci.abs_diff(ai) > 1 || cj.abs_diff(aj) > 1;
                let n = match nodes.match_feet(c, st.pos.y - BODY[1]) {
                    Some(n) => Some(n),
                    // landed where its column has no node of that height (by a wall): the nearest one
                    None if landed || left_ladder => standing_node(w, nodes, &shape, st.pos),
                    None => None,
                };
                if let Some(n) = n {
                    if n != from && (Some(c) == towards || landed || left_ladder || far) {
                        sim.end = End::At { node: n, fall };
                        return sim;
                    }
                }
            }
        }
        // a stall standing presses jump; a stall after the jump ends the move
        let along = (st.pos.x - start.x) * d.x + (st.pos.z - start.z) * d.z;
        let slot = tick as usize % STALL_TICKS;
        let gained = along - along_at[slot];
        along_at[slot] = along;
        if standing && tick as usize > STALL_TICKS && gained < STALL_UNITS && jump_hold == 0 {
            if !jumped {
                if st.on_ground {
                    jump_hold = walk::JUMP_HOLD_TICKS;
                }
            } else {
                break;
            }
        }
        let limit = if sim.ladder {
            ladder_ticks
        } else if airborne {
            fall_ticks
        } else {
            ground_ticks
        };
        if tick >= limit {
            break;
        }
    }
    // the end: standing on some node other than the start
    if st.contact.is_some_and(|c| c.normal.y >= p.ground_cos) && !st.on_ladder {
        if let Some(n) = standing_node(w, nodes, &shape, st.pos) {
            if n != from {
                sim.end = End::At { node: n, fall };
            }
        }
    }
    sim
}

/// A flat walk found between two nodes: `b` lies in direction `dir` of `a`; `ab` and `ba` say which ways are clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FlatPair {
    a: u32,
    dir: u8,
    b: u32,
    ab: bool,
    ba: bool,
}

/// The moves of every node.
pub fn find_moves(w: &WalkWorld, nodes: &Nodes, threads: usize) -> Moves {
    let n = nodes.len();
    let chunks = n.div_ceil(CHUNK);
    let ladder = ladder_columns(w, &nodes.grid);
    let shape = Cylinder::from_dims(V3::from_array(BODY));
    // 1: the flat walks, each pair looked at once (and each way swept)
    let pairs: Vec<Vec<FlatPair>> = parallel(chunks, threads, |k| {
        let mut out = Vec::new();
        for a in k * CHUNK..n.min((k + 1) * CHUNK) {
            let na = nodes.list[a];
            if na.kill.is_some() || !na.flat {
                continue;
            }
            for d in 0..4 {
                let Some(bc) = nodes.grid.neighbour(na.col as usize, d) else {
                    continue;
                };
                for b in nodes.of_column(bc) {
                    let nb = nodes.list[b];
                    if nb.kill.is_some() || !nb.flat || (nb.floor - na.floor).abs() > FLAT_DY {
                        continue;
                    }
                    let (ab, ba) = flat_walk(w, nodes, &ladder, &shape, a, b);
                    if ab || ba {
                        out.push(FlatPair {
                            a: a as u32,
                            dir: d as u8,
                            b: b as u32,
                            ab,
                            ba,
                        });
                    }
                }
            }
        }
        out
    });
    let mut stats = MoveStats::default();
    let mut resolved = vec![0u8; n];
    let mut moves = Vec::new();
    for FlatPair {
        a,
        dir: d,
        b,
        ab,
        ba,
    } in pairs.into_iter().flatten()
    {
        let walk = |from: u32, to: u32| Move {
            from,
            to,
            kind: MoveKind::Walk,
            fall: 0.0,
        };
        if ab {
            resolved[a as usize] |= 1 << d;
            moves.push(walk(a, b));
            stats.flat_walks += 1;
        }
        if ba {
            resolved[b as usize] |= 1 << ((d + 4) % 8);
            moves.push(walk(b, a));
            stats.flat_walks += 1;
        }
    }
    // 2: every other move, simulated
    let sims: Vec<(Vec<Move>, Vec<DeadlyMove>, MoveStats)> = parallel(chunks, threads, |k| {
        let mut out = (Vec::new(), Vec::new(), MoveStats::default());
        let mut containers = Vec::new();
        let (first, end) = (k * CHUNK, n.min((k + 1) * CHUNK));
        for (a, &done) in resolved.iter().enumerate().take(end).skip(first) {
            if nodes.list[a].kill.is_some() {
                continue;
            }
            for d in 0..8 {
                if done & (1 << d) != 0 {
                    continue;
                }
                let run = simulate(w, nodes, a as u32, d, Start::Run, &mut containers);
                out.2.runs += 1;
                record(nodes, a as u32, d, MoveKind::Run, &run, &mut out);
                if run.dropped {
                    let jump = simulate(w, nodes, a as u32, d, Start::Jump, &mut containers);
                    out.2.jumps += 1;
                    record(nodes, a as u32, d, MoveKind::Jump, &jump, &mut out);
                }
                if run.ladder {
                    let down = simulate(w, nodes, a as u32, d, Start::LadderDown, &mut containers);
                    out.2.climbs += 1;
                    record(nodes, a as u32, d, MoveKind::Climb, &down, &mut out);
                }
            }
        }
        out
    });
    let mut deadly = Vec::new();
    for (m, dm, s) in sims {
        moves.extend(m);
        deadly.extend(dm);
        stats.runs += s.runs;
        stats.jumps += s.jumps;
        stats.climbs += s.climbs;
        stats.nowhere += s.nowhere;
        stats.ticks += s.ticks;
    }
    moves.sort_by_key(|m| (m.from, m.to, m.kind));
    moves.dedup_by(|b, a| a.from == b.from && a.to == b.to);
    deadly.sort_by_key(|m| (m.from, m.dir, m.kind));
    Moves {
        moves,
        deadly,
        stats,
    }
}

/// Files one simulated move's end.
fn record(
    nodes: &Nodes,
    from: u32,
    dir: usize,
    kind: MoveKind,
    sim: &Sim,
    out: &mut (Vec<Move>, Vec<DeadlyMove>, MoveStats),
) {
    out.2.ticks += sim.ticks as u64;
    match sim.end {
        End::At { node, fall } => match nodes.list[node as usize].kill {
            // standing at the node's centre is inside a kill volume: as deadly as entering it
            Some(k) => out.1.push(DeadlyMove {
                from,
                dir: dir as u8,
                kind,
                cause: Cause::KillVolume(k),
                at: nodes.rest(node as usize),
                fall,
            }),
            None => out.0.push(Move {
                from,
                to: node,
                kind,
                fall,
            }),
        },
        End::Died { cause, at, fall } => out.1.push(DeadlyMove {
            from,
            dir: dir as u8,
            kind,
            cause,
            at,
            fall,
        }),
        End::Nowhere => out.2.nowhere += 1,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the analysis

/// What a node is to the main component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// In the main component.
    Main,
    /// Reached from the main component, with no way back.
    Trap,
    /// Not reached from the main component.
    Island,
    /// Its standing body is inside a kill volume.
    Deadly,
}

impl Class {
    pub fn text(self) -> &'static str {
        match self {
            Class::Main => "main",
            Class::Trap => "trap",
            Class::Island => "island",
            Class::Deadly => "deadly",
        }
    }
}

/// Adjacency lists (compressed rows) of the moves, forward or backward.
fn adjacency(n: usize, moves: &[Move], backward: bool) -> (Vec<u32>, Vec<u32>) {
    let mut start = vec![0u32; n + 1];
    for m in moves {
        let a = if backward { m.to } else { m.from };
        start[a as usize + 1] += 1;
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut adj = vec![0u32; moves.len()];
    for m in moves {
        let (a, b) = if backward {
            (m.to, m.from)
        } else {
            (m.from, m.to)
        };
        adj[fill[a as usize] as usize] = b;
        fill[a as usize] += 1;
    }
    (start, adj)
}

/// The strongly connected components of the moves (Tarjan's, without recursion): a component per node, `u32::MAX`
/// for the nodes `skip` leaves out, and the number of components.
pub fn components(n: usize, moves: &[Move], skip: &dyn Fn(usize) -> bool) -> (Vec<u32>, u32) {
    const NONE: u32 = u32::MAX;
    let (start, adj) = adjacency(n, moves, false);
    let mut index = vec![NONE; n];
    let mut low = vec![0u32; n];
    let mut on_stack = vec![false; n];
    let mut comp = vec![NONE; n];
    let mut stack: Vec<u32> = Vec::new();
    let mut call: Vec<(u32, u32)> = Vec::new();
    let mut next = 0u32;
    let mut count = 0u32;
    for s in 0..n {
        if index[s] != NONE || skip(s) {
            continue;
        }
        index[s] = next;
        low[s] = next;
        next += 1;
        stack.push(s as u32);
        on_stack[s] = true;
        call.push((s as u32, start[s]));
        while let Some(&(v, e)) = call.last() {
            let v = v as usize;
            if e < start[v + 1] {
                call.last_mut().expect("frame").1 += 1;
                let u = adj[e as usize] as usize;
                if skip(u) {
                    continue;
                }
                if index[u] == NONE {
                    index[u] = next;
                    low[u] = next;
                    next += 1;
                    stack.push(u as u32);
                    on_stack[u] = true;
                    call.push((u as u32, start[u]));
                } else if on_stack[u] {
                    low[v] = low[v].min(index[u]);
                }
            } else {
                call.pop();
                if let Some(&(parent, _)) = call.last() {
                    let parent = parent as usize;
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    loop {
                        let x = stack.pop().expect("tarjan stack") as usize;
                        on_stack[x] = false;
                        comp[x] = count;
                        if x == v {
                            break;
                        }
                    }
                    count += 1;
                }
            }
        }
    }
    (comp, count)
}

/// Every node reached from `seeds` along the moves (or against them, `backward`).
fn reached(n: usize, moves: &[Move], seeds: &[u32], backward: bool) -> Vec<bool> {
    let (start, adj) = adjacency(n, moves, backward);
    let mut seen = vec![false; n];
    let mut queue: Vec<u32> = Vec::new();
    for &s in seeds {
        if !seen[s as usize] {
            seen[s as usize] = true;
            queue.push(s);
        }
    }
    while let Some(v) = queue.pop() {
        for &u in &adj[start[v as usize] as usize..start[v as usize + 1] as usize] {
            if !seen[u as usize] {
                seen[u as usize] = true;
                queue.push(u);
            }
        }
    }
    seen
}

/// How the main component is chosen.
#[derive(Debug, Clone, PartialEq)]
pub enum Seed {
    /// The component holding the most player spawns (a tie: the larger; no spawn: the largest).
    Spawns,
    /// The component of the node under this point.
    Point([f32; 3]),
    /// The largest component holding a player spawn of this piece (`<id>~...`).
    Piece(String),
}

/// A player spawn of the composed world.
#[derive(Debug, Clone, PartialEq)]
pub struct Spawn {
    pub name: String,
    pub pos: [f32; 3],
}

/// The player spawns of a parsed world.
pub fn spawns(world: &dc_world::World<'_>) -> Vec<Spawn> {
    world
        .objects_with_role(dc_world::ObjectRole::PlayerSpawn)
        .filter_map(|o| {
            Some(Spawn {
                name: String::from_utf8_lossy(o.name().unwrap_or_default()).into_owned(),
                pos: o.position()?,
            })
        })
        .collect()
}

/// The node a body put at `p` stands on: placed as a walk's start is (the floor under a point over it, then the
/// client's spawn placement), then [`standing_node`].
pub fn node_of_point(w: &WalkWorld, nodes: &Nodes, p: [f32; 3]) -> Option<u32> {
    let mp = MovementParams::DEFAULT;
    let shape = Cylinder::from_dims(V3::from_array(BODY));
    let body = walk::place(w, &shape, &mp, V3::from_array(p));
    standing_node(w, nodes, &shape, body)
}

/// Chooses the main component from the component of each node and the size of each component.
pub type Pick<'a> = dyn Fn(&[u32], &[u32]) -> Option<u32> + 'a;

/// The classes of every node and the main component.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// The strongly connected component of each node (`u32::MAX`: a deadly node).
    pub comp: Vec<u32>,
    pub comp_size: Vec<u32>,
    pub main: Option<u32>,
    pub class: Vec<Class>,
    /// The node can get to the main component.
    pub reaches_main: Vec<bool>,
}

/// The analysis with the main component chosen by `pick` from the components (and their sizes).
pub fn analyse(nodes: &Nodes, moves: &Moves, pick: &Pick<'_>) -> Analysis {
    let n = nodes.len();
    let deadly = |i: usize| nodes.list[i].kill.is_some();
    let (comp, count) = components(n, &moves.moves, &deadly);
    let mut comp_size = vec![0u32; count as usize];
    for &c in &comp {
        if c != u32::MAX {
            comp_size[c as usize] += 1;
        }
    }
    let main = pick(&comp, &comp_size);
    let seeds: Vec<u32> = match main {
        Some(m) => (0..n as u32).filter(|&i| comp[i as usize] == m).collect(),
        None => Vec::new(),
    };
    let from_main = reached(n, &moves.moves, &seeds, false);
    let reaches_main = reached(n, &moves.moves, &seeds, true);
    let class = (0..n)
        .map(|i| {
            if deadly(i) {
                Class::Deadly
            } else if main == Some(comp[i]) {
                Class::Main
            } else if from_main[i] {
                Class::Trap
            } else {
                Class::Island
            }
        })
        .collect();
    Analysis {
        comp,
        comp_size,
        main,
        class,
        reaches_main,
    }
}

/// The component holding the most of `spawn_nodes` (a tie: the larger; none: the largest).
pub fn pick_most(spawn_nodes: &[u32], comp: &[u32], size: &[u32]) -> Option<u32> {
    let mut votes: Vec<(u32, u32)> = Vec::new();
    for &s in spawn_nodes {
        let c = comp[s as usize];
        if c == u32::MAX {
            continue;
        }
        match votes.iter_mut().find(|v| v.0 == c) {
            Some(v) => v.1 += 1,
            None => votes.push((c, 1)),
        }
    }
    if votes.is_empty() {
        return pick_largest(&(0..size.len() as u32).collect::<Vec<_>>(), size);
    }
    votes
        .iter()
        .max_by(|a, b| {
            a.1.cmp(&b.1)
                .then(size[a.0 as usize].cmp(&size[b.0 as usize]))
                .then(b.0.cmp(&a.0))
        })
        .map(|v| v.0)
}

/// The largest of the components `cands` (a tie: the first found).
pub fn pick_largest(cands: &[u32], size: &[u32]) -> Option<u32> {
    cands
        .iter()
        .copied()
        .filter(|&c| c != u32::MAX)
        .max_by(|&a, &b| size[a as usize].cmp(&size[b as usize]).then(b.cmp(&a)))
}

// ---------------------------------------------------------------------------------------------------------------
// areas

/// Union-find over node indices.
struct Dsu(Vec<u32>);

impl Dsu {
    fn new(n: usize) -> Dsu {
        Dsu((0..n as u32).collect())
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.0[x as usize] != x {
            let p = self.0[x as usize];
            self.0[x as usize] = self.0[p as usize];
            x = p;
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.0[hi as usize] = lo;
        }
    }
}

/// A connected area of nodes of one kind.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeArea {
    /// The nodes, in index order.
    pub nodes: Vec<u32>,
    /// `[x, z]` corners of the nodes' cells.
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// The lowest and the highest floor.
    pub floor: [f32; 2],
}

/// The areas the nodes `member` accepts make, joined along the moves between them, largest first.
pub fn node_areas(nodes: &Nodes, moves: &[Move], member: &dyn Fn(usize) -> bool) -> Vec<NodeArea> {
    let mut dsu = Dsu::new(nodes.len());
    for m in moves {
        if member(m.from as usize) && member(m.to as usize) {
            dsu.union(m.from, m.to);
        }
    }
    collect_areas(nodes, &mut dsu, member)
}

/// The areas of the deadly nodes (no moves join them): nodes of neighbouring columns inside the same kill volume
/// whose floors are at most a step of the grid apart.
pub fn kill_areas(nodes: &Nodes) -> Vec<NodeArea> {
    let mut dsu = Dsu::new(nodes.len());
    for a in 0..nodes.len() {
        let na = nodes.list[a];
        let Some(k) = na.kill else {
            continue;
        };
        for d in 0..4 {
            let Some(c) = nodes.grid.neighbour(na.col as usize, d) else {
                continue;
            };
            for b in nodes.of_column(c) {
                let nb = nodes.list[b];
                if nb.kill == Some(k) && (nb.floor - na.floor).abs() <= nodes.grid.step {
                    dsu.union(a as u32, b as u32);
                }
            }
        }
    }
    collect_areas(nodes, &mut dsu, &|i| nodes.list[i].kill.is_some())
}

fn collect_areas(nodes: &Nodes, dsu: &mut Dsu, member: &dyn Fn(usize) -> bool) -> Vec<NodeArea> {
    let n = nodes.len();
    let mut index: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    let mut out: Vec<NodeArea> = Vec::new();
    for i in 0..n {
        if !member(i) {
            continue;
        }
        let r = dsu.find(i as u32);
        let k = *index.entry(r).or_insert_with(|| {
            out.push(NodeArea {
                nodes: Vec::new(),
                min: [f32::INFINITY; 2],
                max: [f32::NEG_INFINITY; 2],
                floor: [f32::INFINITY, f32::NEG_INFINITY],
            });
            out.len() - 1
        });
        let a = &mut out[k];
        a.nodes.push(i as u32);
        let (lo, hi) = nodes.grid.cell(nodes.list[i].col as usize);
        a.min = [a.min[0].min(lo.0), a.min[1].min(lo.1)];
        a.max = [a.max[0].max(hi.0), a.max[1].max(hi.1)];
        let f = nodes.list[i].floor;
        a.floor = [a.floor[0].min(f), a.floor[1].max(f)];
    }
    out.sort_by(|a, b| {
        b.nodes
            .len()
            .cmp(&a.nodes.len())
            .then(a.nodes[0].cmp(&b.nodes[0]))
    });
    out
}

// ---------------------------------------------------------------------------------------------------------------
// the report

/// A `walk --reach` request's own part.
#[derive(Debug, Clone, PartialEq)]
pub struct ReachJob {
    pub area: Option<walk::AreaArg>,
    pub step: f32,
    pub seed: Seed,
    /// Where to write the main component's nodes (`--nodes`).
    pub nodes_out: Option<PathBuf>,
}

/// To 0.1 unit, as the nearest f64 (so the JSON reads `47.2`, not the f32's `47.20000076293945`).
fn r1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

fn pt(v: V3) -> [f64; 3] {
    [r1(v.x), r1(v.y), r1(v.z)]
}

/// The piece regions of a project, for the per-node lookup ([`walk::piece_at`]'s rule: in 3D, else in x / z).
struct Pieces(Vec<(String, crate::project::BoxF)>);

impl Pieces {
    fn of(p: &Project) -> Pieces {
        Pieces(
            p.pieces
                .iter()
                .map(|x| (x.id.clone(), x.world_region()))
                .collect(),
        )
    }

    /// The index of the piece whose region holds `q`, `len()` for the map's own ground.
    fn at(&self, q: V3) -> usize {
        let inside = |r: &crate::project::BoxF, three: bool| {
            q.x >= r.min[0]
                && q.x <= r.max[0]
                && q.z >= r.min[2]
                && q.z <= r.max[2]
                && (!three || (q.y >= r.min[1] && q.y <= r.max[1]))
        };
        self.0
            .iter()
            .position(|(_, r)| inside(r, true))
            .or_else(|| self.0.iter().position(|(_, r)| inside(r, false)))
            .unwrap_or(self.0.len())
    }

    fn name(&self, k: usize) -> Option<String> {
        self.0.get(k).map(|(id, _)| id.clone())
    }
}

/// The piece most of `members` lie in.
fn area_piece(piece_of: &[usize], members: &[u32], pieces: &Pieces) -> Option<String> {
    let mut votes = vec![0usize; pieces.0.len() + 1];
    for &m in members {
        votes[piece_of[m as usize]] += 1;
    }
    let best = (0..votes.len()).max_by(|&a, &b| votes[a].cmp(&votes[b]).then(b.cmp(&a)))?;
    pieces.name(best)
}

fn cause_text(w: &WalkWorld, c: &Cause) -> (&'static str, Value) {
    match c {
        Cause::KillVolume(k) => ("kill_volume", json!(w.kills[*k as usize].name)),
        Cause::Fall(h) => ("fall", json!(r1(*h))),
        Cause::FellOut => ("fell_out", Value::Null),
    }
}

/// `walk --reach` on a walk world: the nodes, the moves, the analysis, the report, the picture and the nodes file.
#[allow(clippy::too_many_arguments)]
pub fn report(
    req: &WalkRequest,
    p: &Project,
    world: &dc_world::World<'_>,
    w: &WalkWorld,
    area: [f32; 4],
    job: &ReachJob,
    world_info: Value,
    mut diags: Diagnostics,
) -> Result<Outcome, Diagnostics> {
    let invalid =
        |path: &str, msg: String| Diagnostics::one(Diagnostic::new(Code::OpInvalid, path, msg));
    let t0 = Instant::now();
    let grid = Grid::new(area, job.step).map_err(|e| invalid("--box", e))?;
    let threads = threads();
    let nodes = find_nodes(w, grid, threads).map_err(|e| invalid("--step", e))?;
    let t_nodes = t0.elapsed().as_secs_f32();
    let moves = find_moves(w, &nodes, threads);
    let t_moves = t0.elapsed().as_secs_f32() - t_nodes;
    let t2 = Instant::now();
    // the player spawns inside the box (the others stand on no floor of the grid)
    let all_spawns: Vec<Spawn> = spawns(world)
        .into_iter()
        .filter(|s| {
            s.pos[0] >= area[0] && s.pos[0] <= area[2] && s.pos[2] >= area[1] && s.pos[2] <= area[3]
        })
        .collect();
    let spawn_nodes: Vec<Option<u32>> = all_spawns
        .iter()
        .map(|s| node_of_point(w, &nodes, s.pos))
        .collect();
    let seed_nodes: Vec<u32> = match &job.seed {
        Seed::Spawns => spawn_nodes.iter().flatten().copied().collect(),
        Seed::Point(q) => match node_of_point(w, &nodes, *q) {
            Some(n) => vec![n],
            None => {
                return Err(invalid(
                    "--seed",
                    format!(
                        "no floor of the grid under ({}, {}, {}): give a point on a floor inside the box",
                        q[0], q[1], q[2]
                    ),
                ))
            }
        },
        Seed::Piece(id) => {
            let own = format!("{id}~");
            let found: Vec<u32> = all_spawns
                .iter()
                .zip(&spawn_nodes)
                .filter(|(s, _)| s.name.starts_with(&own))
                .filter_map(|(_, n)| *n)
                .collect();
            if found.is_empty() {
                return Err(invalid(
                    "--seed-piece",
                    format!("no player spawn of `{id}` stands on a floor inside the box (its spawns are named `{own}...`)"),
                ));
            }
            found
        }
    };
    let pick = |comp: &[u32], size: &[u32]| -> Option<u32> {
        match &job.seed {
            Seed::Spawns => pick_most(&seed_nodes, comp, size),
            Seed::Point(_) => seed_nodes
                .first()
                .map(|&n| comp[n as usize])
                .filter(|&c| c != u32::MAX),
            Seed::Piece(_) => pick_largest(
                &seed_nodes
                    .iter()
                    .map(|&n| comp[n as usize])
                    .collect::<Vec<_>>(),
                size,
            ),
        }
    };
    let an = analyse(&nodes, &moves, &pick);
    let pieces = Pieces::of(p);
    let piece_of: Vec<usize> = (0..nodes.len()).map(|i| pieces.at(nodes.pos(i))).collect();
    let class = &an.class;
    let is = |k: Class| move |i: usize| class[i] == k;
    // deadly moves count from main-component nodes only
    let deadly: Vec<&DeadlyMove> = moves
        .deadly
        .iter()
        .filter(|m| class[m.from as usize] == Class::Main)
        .collect();
    let mut deadly_from = vec![false; nodes.len()];
    for m in &deadly {
        deadly_from[m.from as usize] = true;
    }
    let trap_areas = node_areas(&nodes, &moves.moves, &is(Class::Trap));
    let island_areas = node_areas(&nodes, &moves.moves, &is(Class::Island));
    let deadly_areas = node_areas(&nodes, &moves.moves, &|i| deadly_from[i]);
    let kill_areas = kill_areas(&nodes);
    // the area of each trap node, for its entries
    let mut trap_area_of = vec![u32::MAX; nodes.len()];
    for (k, a) in trap_areas.iter().enumerate() {
        for &m in &a.nodes {
            trap_area_of[m as usize] = k as u32;
        }
    }
    let mut entries: Vec<Vec<&Move>> = vec![Vec::new(); trap_areas.len()];
    for m in &moves.moves {
        if class[m.from as usize] == Class::Main && class[m.to as usize] == Class::Trap {
            entries[trap_area_of[m.to as usize] as usize].push(m);
        }
    }
    let move_json = |m: &Move| {
        json!({
            "from": pt(nodes.pos(m.from as usize)),
            "to": pt(nodes.pos(m.to as usize)),
            "kind": m.kind.text(),
            "fall": r1(m.fall),
        })
    };
    let count = |k: Class| class.iter().filter(|&&c| c == k).count();
    let (n_main, n_traps, n_islands, n_deadly_nodes) = (
        count(Class::Main),
        count(Class::Trap),
        count(Class::Island),
        count(Class::Deadly),
    );
    let traps_json: Vec<Value> = trap_areas
        .iter()
        .zip(&entries)
        .take(walk::AREA_CAP)
        .map(|(a, e)| {
            json!({
                "nodes": a.nodes.len(),
                "min": [r1(a.min[0]), r1(a.min[1])],
                "max": [r1(a.max[0]), r1(a.max[1])],
                "floor": [r1(a.floor[0]), r1(a.floor[1])],
                "piece": area_piece(&piece_of, &a.nodes, &pieces),
                "entry_moves": e.len(),
                "entries": e.iter().take(ENTRY_CAP).map(|m| move_json(m)).collect::<Vec<_>>(),
            })
        })
        .collect();
    // the deadly moves of each deadly area
    let mut deadly_area_of = vec![u32::MAX; nodes.len()];
    for (k, a) in deadly_areas.iter().enumerate() {
        for &m in &a.nodes {
            deadly_area_of[m as usize] = k as u32;
        }
    }
    let mut by_area: Vec<Vec<&DeadlyMove>> = vec![Vec::new(); deadly_areas.len()];
    for &m in &deadly {
        by_area[deadly_area_of[m.from as usize] as usize].push(m);
    }
    let deadly_json: Vec<Value> = deadly_areas
        .iter()
        .zip(&by_area)
        .take(walk::AREA_CAP)
        .map(|(a, ms)| {
            let mut volumes: Vec<(String, usize)> = Vec::new();
            let (mut falls, mut fall_max, mut fell_out) = (0usize, 0.0f32, 0usize);
            for m in ms {
                match m.cause {
                    Cause::KillVolume(k) => {
                        let name = &w.kills[k as usize].name;
                        match volumes.iter_mut().find(|v| &v.0 == name) {
                            Some(v) => v.1 += 1,
                            None => volumes.push((name.clone(), 1)),
                        }
                    }
                    Cause::Fall(h) => {
                        falls += 1;
                        fall_max = fall_max.max(h);
                    }
                    Cause::FellOut => fell_out += 1,
                }
            }
            volumes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            json!({
                "moves": ms.len(),
                "nodes": a.nodes.len(),
                "min": [r1(a.min[0]), r1(a.min[1])],
                "max": [r1(a.max[0]), r1(a.max[1])],
                "floor": [r1(a.floor[0]), r1(a.floor[1])],
                "piece": area_piece(&piece_of, &a.nodes, &pieces),
                "kill_volumes": volumes.iter().map(|v| json!({"name": v.0, "moves": v.1})).collect::<Vec<_>>(),
                "falls": falls,
                "fall_max": r1(fall_max),
                "fell_out": fell_out,
                "samples": ms.iter().take(SAMPLE_CAP).map(|m| {
                    let (cause, what) = cause_text(w, &m.cause);
                    json!({
                        "from": pt(nodes.pos(m.from as usize)),
                        "at": pt(m.at),
                        "kind": m.kind.text(),
                        "cause": cause,
                        "detail": what,
                        "fall": r1(m.fall),
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    let spawn_index: Vec<(usize, Option<u32>)> = spawn_nodes.iter().copied().enumerate().collect();
    let island_json: Vec<Value> = island_areas
        .iter()
        .take(walk::AREA_CAP)
        .map(|a| {
            let first = a.nodes[0];
            let here: Vec<&str> = spawn_index
                .iter()
                .filter(|(_, n)| n.is_some_and(|n| a.nodes.binary_search(&n).is_ok()))
                .map(|(i, _)| all_spawns[*i].name.as_str())
                .collect();
            json!({
                "nodes": a.nodes.len(),
                "min": [r1(a.min[0]), r1(a.min[1])],
                "max": [r1(a.max[0]), r1(a.max[1])],
                "floor": [r1(a.floor[0]), r1(a.floor[1])],
                "piece": area_piece(&piece_of, &a.nodes, &pieces),
                "reaches_main": an.reaches_main[first as usize],
                "spawns": here,
            })
        })
        .collect();
    let kill_json: Vec<Value> = kill_areas
        .iter()
        .take(walk::AREA_CAP)
        .map(|a| {
            let k = nodes.list[a.nodes[0] as usize].kill.unwrap_or(0) as usize;
            json!({
                "nodes": a.nodes.len(),
                "min": [r1(a.min[0]), r1(a.min[1])],
                "max": [r1(a.max[0]), r1(a.max[1])],
                "floor": [r1(a.floor[0]), r1(a.floor[1])],
                "volume": w.kills.get(k).map(|v| v.name.clone()),
                "piece": area_piece(&piece_of, &a.nodes, &pieces),
            })
        })
        .collect();
    // the spawns outside the main component
    let mut outside = Vec::new();
    for (s, n) in all_spawns.iter().zip(&spawn_nodes) {
        let (where_, reaches) = match n {
            Some(n) if class[*n as usize] == Class::Main => continue,
            Some(n) => (class[*n as usize].text(), an.reaches_main[*n as usize]),
            None => ("no_node", false),
        };
        outside.push(json!({
            "name": s.name,
            "pos": [r1(s.pos[0]), r1(s.pos[1]), r1(s.pos[2])],
            "node": n.map(|n| pt(nodes.pos(n as usize))),
            "class": where_,
            "reaches_main": reaches,
            "piece": walk::piece_at(p, s.pos),
        }));
    }
    let spawns_in_main = spawn_nodes
        .iter()
        .flatten()
        .filter(|&&n| class[n as usize] == Class::Main)
        .count();
    // per piece
    let mut table = vec![[0usize; 8]; pieces.0.len() + 1];
    for (sp, n) in all_spawns.iter().zip(&spawn_nodes) {
        let row = &mut table[pieces.at(V3::from_array(sp.pos))];
        row[6] += 1;
        if n.is_some_and(|n| class[n as usize] == Class::Main) {
            row[7] += 1;
        }
    }
    for i in 0..nodes.len() {
        let row = &mut table[piece_of[i]];
        row[0] += 1;
        row[match class[i] {
            Class::Main => 1,
            Class::Trap => 2,
            Class::Island => 3,
            Class::Deadly => 4,
        }] += 1;
    }
    for m in &deadly {
        table[piece_of[m.from as usize]][5] += 1;
    }
    let pieces_json: Vec<Value> = table
        .iter()
        .enumerate()
        .filter(|(_, r)| r[0] > 0 || r[6] > 0)
        .map(|(k, r)| {
            json!({
                "piece": pieces.name(k),
                "nodes": r[0], "main": r[1], "traps": r[2], "islands": r[3],
                "deadly_nodes": r[4], "deadly_moves": r[5],
                "spawns": r[6], "spawns_main": r[7],
            })
        })
        .collect();
    let t_analysis = t2.elapsed().as_secs_f32();
    // the main component's nodes, every deadly move from them and every move into a trap, for planners (a rail
    // across each of those moves, a corridor's end on one of these nodes)
    let mut nodes_file = Value::Null;
    if let Some(path) = &job.nodes_out {
        let list: Vec<[f64; 3]> = (0..nodes.len())
            .filter(|&i| class[i] == Class::Main)
            .map(|i| pt(nodes.pos(i)))
            .collect();
        let deadly_list: Vec<Value> = deadly
            .iter()
            .map(|m| {
                let (cause, detail) = cause_text(w, &m.cause);
                let (dx, dz) = DIRS[m.dir as usize];
                json!({
                    "from": pt(nodes.pos(m.from as usize)),
                    "dir": [dx, dz],
                    "kind": m.kind.text(),
                    "cause": cause,
                    "detail": detail,
                    "at": pt(m.at),
                })
            })
            .collect();
        let entry_list: Vec<Value> = entries.iter().flatten().map(|m| move_json(m)).collect();
        let body = serde_json::to_vec(&json!({
            "step": job.step,
            "nodes": list,
            "deadly": deadly_list,
            "entries": entry_list,
        }))
        .expect("json");
        write_atomic(path, &body).map_err(|e| {
            Diagnostics::one(Diagnostic::new(
                Code::ProjectIo,
                path.display().to_string(),
                e.to_string(),
            ))
        })?;
        nodes_file = json!(path.display().to_string());
    }
    // the picture
    let t3 = Instant::now();
    let out = req.out.clone().unwrap_or_else(|| {
        req.dir
            .join(crate::project::BUILD_DIR)
            .join("walk")
            .join("reach.png")
    });
    let size = req.size.unwrap_or_else(|| walk::picture_size(area));
    let img = walk::top_picture(world, p, area, size, &mut |img, map| {
        paint(img, map, &nodes, class, &deadly_from, &entries, area)
    })?;
    walk::write_png(&out, &img)?;
    let t_picture = t3.elapsed().as_secs_f32();
    // diagnostics
    for (a, e) in trap_areas.iter().zip(&entries).take(walk::DIAG_CAP) {
        let piece = area_piece(&piece_of, &a.nodes, &pieces);
        let first = e
            .first()
            .map(|m| {
                let (f, t) = (pt(nodes.pos(m.from as usize)), pt(nodes.pos(m.to as usize)));
                format!(
                    "; the first of {} entries goes from ({}, {}, {}) to ({}, {}, {})",
                    e.len(),
                    f[0],
                    f[1],
                    f[2],
                    t[0],
                    t[1],
                    t[2]
                )
            })
            .unwrap_or_default();
        diags.push(
            Diagnostic::new(
                Code::WalkTrap,
                format!("reach.{}", piece.as_deref().unwrap_or("ground")),
                format!(
                    "{} floors at x {}..{}, z {}..{} (floor {}..{}){}: a player who gets there cannot get back{first}",
                    a.nodes.len(),
                    r1(a.min[0]),
                    r1(a.max[0]),
                    r1(a.min[1]),
                    r1(a.max[1]),
                    r1(a.floor[0]),
                    r1(a.floor[1]),
                    piece.as_deref().map(|p| format!(" in {p}")).unwrap_or_default(),
                ),
            )
            .with_fix("block the entries (a wall, a railing) or give it a way back (a ramp, a ladder, a doorway)"),
        );
    }
    for (a, ms) in deadly_areas.iter().zip(&by_area).take(walk::DIAG_CAP) {
        let piece = area_piece(&piece_of, &a.nodes, &pieces);
        let worst = ms
            .iter()
            .map(|m| match m.cause {
                Cause::KillVolume(k) => format!("kill volume `{}`", w.kills[k as usize].name),
                Cause::Fall(h) => format!("a fall of {}", r1(h)),
                Cause::FellOut => "a fall out of the world".to_string(),
            })
            .next()
            .unwrap_or_default();
        diags.push(
            Diagnostic::new(
                Code::WalkDeadly,
                format!("reach.{}", piece.as_deref().unwrap_or("ground")),
                format!(
                    "{} moves from {} floors at x {}..{}, z {}..{} (floor {}..{}){} kill, the first by {worst}",
                    ms.len(),
                    a.nodes.len(),
                    r1(a.min[0]),
                    r1(a.max[0]),
                    r1(a.min[1]),
                    r1(a.max[1]),
                    r1(a.floor[0]),
                    r1(a.floor[1]),
                    piece.as_deref().map(|p| format!(" in {p}")).unwrap_or_default(),
                ),
            )
            .with_fix("rail the ledge, floor the hole, or keep the kill volume out of reach"),
        );
    }
    for o in outside.iter().take(walk::DIAG_CAP) {
        diags.push(
            Diagnostic::new(
                Code::WalkSpawnOutside,
                format!("reach.spawn.{}", o["name"].as_str().unwrap_or("?")),
                format!(
                    "player spawn `{}` at {} is outside the main component ({}, {})",
                    o["name"].as_str().unwrap_or("?"),
                    o["pos"],
                    o["class"].as_str().unwrap_or("?"),
                    if o["reaches_main"] == true {
                        "it can walk into the main component"
                    } else {
                        "it cannot get to the main component"
                    }
                ),
            )
            .with_fix("move the spawn onto the main area, or join its area to it"),
        );
    }
    let text = format!(
        "reach of x {}..{}, z {}..{} every {} units: {} nodes ({} deadly), {} moves ({} flat walks, {} simulated), main {} nodes ({} spawns), {} trap nodes in {} areas, {} deadly moves in {} areas, {} island nodes, {} spawns outside main; picture {}",
        area[0],
        area[2],
        area[1],
        area[3],
        job.step,
        nodes.len(),
        n_deadly_nodes,
        moves.moves.len(),
        moves.stats.flat_walks,
        moves.stats.runs + moves.stats.jumps + moves.stats.climbs,
        n_main,
        spawns_in_main,
        n_traps,
        trap_areas.len(),
        deadly.len(),
        deadly_areas.len(),
        n_islands,
        outside.len(),
        out.display()
    );
    let spawns_outside = outside.len();
    let truncated = trap_areas.len() > walk::AREA_CAP
        || deadly_areas.len() > walk::AREA_CAP
        || island_areas.len() > walk::AREA_CAP
        || kill_areas.len() > walk::AREA_CAP
        || outside.len() > SPAWN_CAP;
    outside.truncate(SPAWN_CAP);
    let seed = match &job.seed {
        Seed::Spawns => json!({"by": "spawns"}),
        Seed::Point(q) => json!({"by": "point", "point": q}),
        Seed::Piece(id) => json!({"by": "piece", "piece": id}),
    };
    let s = &moves.stats;
    let result = json!({
        "world": world_info,
        "box": [r1(area[0]), r1(area[1]), r1(area[2]), r1(area[3])],
        "step": job.step,
        "grid": [grid.nx, grid.nz],
        "columns": grid.columns(),
        "nodes": nodes.len(),
        "deadly_nodes": n_deadly_nodes,
        "moves": moves.moves.len(),
        "simulation": {
            "flat_walks": s.flat_walks,
            "runs": s.runs,
            "jumps": s.jumps,
            "climbs": s.climbs,
            "nowhere": s.nowhere,
            "ticks": s.ticks,
            "threads": threads,
        },
        "seconds": {
            "nodes": r1(t_nodes),
            "moves": r1(t_moves),
            "analysis": r1(t_analysis),
            "picture": r1(t_picture),
        },
        "components": an.comp_size.len(),
        "main": {
            "seed": seed,
            "nodes": n_main,
            "spawns": spawns_in_main,
        },
        "spawns": all_spawns.len(),
        "traps": {"nodes": n_traps, "areas": trap_areas.len(), "list": traps_json},
        "deadly": {"moves": deadly.len(), "areas": deadly_areas.len(), "list": deadly_json},
        "kill_nodes": {"nodes": n_deadly_nodes, "areas": kill_areas.len(), "list": kill_json},
        "islands": {"nodes": n_islands, "areas": island_areas.len(), "list": island_json},
        "spawns_outside_count": spawns_outside,
        "spawns_outside": outside,
        "pieces": pieces_json,
        "nodes_file": nodes_file,
        "nodes_written": if job.nodes_out.is_some() { json!(n_main) } else { Value::Null },
        "truncated": truncated,
        "image": out.display().to_string(),
        "width": img.width,
        "height": img.height,
    });
    let mut o = Outcome::new(result, text);
    o.diagnostics = diags;
    Ok(o)
}

/// Draws the classes on the top view: per column the most telling class of its nodes (a trap over a deadly move's
/// start over the main component over an island over a node inside a kill volume, drawn fainter: kill volumes often
/// fill the space under a play area), the entry moves into traps as lines with a disc at their start, and the box.
fn paint(
    img: &mut RgbaImage,
    map: &walk::ToPixel<'_>,
    nodes: &Nodes,
    class: &[Class],
    deadly_from: &[bool],
    entries: &[Vec<&Move>],
    area: [f32; 4],
) {
    let g = &nodes.grid;
    for c in 0..g.columns() {
        let mut best: Option<(u8, [u8; 3], f32)> = None;
        for n in nodes.of_column(c) {
            let k = match class[n] {
                Class::Trap => (4, TRAP_COLOR, 0.7),
                Class::Main if deadly_from[n] => (3, DEADLY_COLOR, 0.75),
                Class::Main => (2, MAIN_COLOR, 0.6),
                Class::Island => (1, ISLAND_COLOR, 0.6),
                Class::Deadly => (0, DEADLY_COLOR, 0.3),
            };
            if best.is_none_or(|b| k.0 > b.0) {
                best = Some(k);
            }
        }
        if let Some((_, color, alpha)) = best {
            let (a, b) = g.cell(c);
            walk::fill_cell(img, map, a, b, color, alpha);
        }
    }
    let r = ((img.width.min(img.height) as f32 / 512.0).round() as i32).clamp(1, 4);
    for m in entries.iter().flatten() {
        let a = nodes.pos(m.from as usize);
        let b = nodes.pos(m.to as usize);
        let (pa, pb) = (map(a.x, a.z), map(b.x, b.z));
        render_sw::draw_line(img, pa, pb, ENTRY_COLOR, 1.0);
        render_sw::draw_disc(
            img,
            pa.0.round() as i32,
            pa.1.round() as i32,
            r,
            ENTRY_COLOR,
            ENTRY_COLOR,
        );
    }
    let corners = [
        map(area[0], area[1]),
        map(area[2], area[1]),
        map(area[2], area[3]),
        map(area[0], area[3]),
    ];
    for k in 0..4 {
        render_sw::draw_line(img, corners[k], corners[(k + 1) % 4], [255, 255, 255], 0.9);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_world::compose::clip::Aabb;
    use dc_world::compose::{self, fixture};
    use dc_world::synth::{SynthObject, SynthRenderWorld, SynthWorld};

    /// A made-up world of solid boxes (their outward faces on the fixture's naive BSP), kill volume boxes and
    /// player spawns. Every name and number is our own.
    fn boxes_world(solids: &[Aabb], kills: &[Aabb], spawns: &[[f32; 3]]) -> Vec<u8> {
        let mut faces = Vec::new();
        for b in solids {
            faces.extend(fixture::box_faces(b, 1));
        }
        let mut models = vec![fixture::model("", 0, [0.0; 3], &faces)];
        let names: [&[u8]; 3] = [b"name", b"pos", b"rot"];
        let mut objects = Vec::new();
        for (i, k) in kills.iter().enumerate() {
            let name = format!("drop{i}");
            models.push(fixture::model(
                &name,
                2,
                k.centre(),
                &fixture::box_faces(k, 0),
            ));
            objects.push(SynthObject::point(
                b"dc.kill_volume",
                names,
                name.as_bytes(),
                k.centre(),
                [0.0; 3],
            ));
        }
        for (i, s) in spawns.iter().enumerate() {
            objects.push(SynthObject::dc(
                "dc.player_spawn",
                &format!("start{i}"),
                *s,
                [0.0; 3],
                &[],
            ));
        }
        let all = Aabb::of_points(faces.iter().flat_map(|f| f.corners.iter())).unwrap();
        let lo = [all.min[0] - 100.0, all.min[1] - 100.0, all.min[2] - 100.0];
        let hi = [all.max[0] + 100.0, all.max[1] + 100.0, all.max[2] + 100.0];
        SynthWorld {
            info: b"reach test".to_vec(),
            extent_min: lo,
            extent_max: hi,
            tree_shape: vec![false],
            models,
            objects,
            grid_origin: lo,
            grid_cell: [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
            grid_dims: [1, 1, 1],
            grid_rgb: vec![40, 40, 40],
            render: SynthRenderWorld {
                blocks: vec![fixture::block(&faces, b"test/stone")],
                models: Vec::new(),
            },
            ..SynthWorld::default()
        }
        .build()
    }

    fn walk_world(bytes: &[u8]) -> (WalkWorld, Vec<Spawn>) {
        let w = dc_world::World::parse(bytes).unwrap();
        (WalkWorld::of(&w, false), spawns(&w))
    }

    /// The nodes, the moves and the analysis of `area` at `step`, the main component by the spawns.
    fn reach(
        w: &WalkWorld,
        spawns: &[Spawn],
        area: [f32; 4],
        step: f32,
    ) -> (Nodes, Moves, Analysis) {
        let nodes = find_nodes(w, Grid::new(area, step).unwrap(), 4).unwrap();
        let moves = find_moves(w, &nodes, 4);
        let seeds: Vec<u32> = spawns
            .iter()
            .filter_map(|s| node_of_point(w, &nodes, s.pos))
            .collect();
        assert!(!seeds.is_empty(), "the spawns stand on nodes");
        let an = analyse(&nodes, &moves, &|c, s| pick_most(&seeds, c, s));
        (nodes, moves, an)
    }

    /// The inside of the test rooms (walls round it, outside the box).
    const ROOM: [f32; 4] = [-600.0, -600.0, 600.0, 600.0];

    /// Walls 100 thick round x / z -600 .. 600, from y -300 up to `top`.
    fn walls(top: f32) -> Vec<Aabb> {
        vec![
            Aabb::new([-700.0, -300.0, -700.0], [-600.0, top, 700.0]),
            Aabb::new([600.0, -300.0, -700.0], [700.0, top, 700.0]),
            Aabb::new([-600.0, -300.0, -700.0], [600.0, top, -600.0]),
            Aabb::new([-600.0, -300.0, 600.0], [600.0, top, 700.0]),
        ]
    }

    #[test]
    fn the_grid_lays_its_columns_as_the_survey_does() {
        let g = Grid::new([0.0, 0.0, 100.0, 50.0], 30.0).unwrap();
        assert_eq!((g.nx, g.nz), (4, 2));
        assert_eq!(g.origin, [-10.0, -5.0]);
        assert_eq!(g.centre(0), (5.0, 10.0));
        assert_eq!(g.column_at(5.0, 10.0), Some(0));
        assert_eq!(g.column_at(81.0, 54.0), Some(7));
        assert_eq!(g.column_at(-11.0, 0.0), None);
        assert_eq!(g.neighbour(0, 0), Some(1));
        assert_eq!(g.neighbour(0, 1), Some(5));
        assert_eq!(g.neighbour(0, 4), None);
        assert_eq!(g.neighbour(7, 6), Some(3));
        assert!(Grid::new([0.0, 0.0, 0.0, 10.0], 8.0).is_err());
        assert!(Grid::new([0.0, 0.0, 10.0, 10.0], 0.5).is_err());
    }

    #[test]
    fn components_are_strongly_connected_and_the_pick_follows_the_spawns() {
        // 0 <-> 1 -> 2 <-> 3, 4 alone, 5 -> 0 (5 is left out as deadly)
        let m = |from: u32, to: u32| Move {
            from,
            to,
            kind: MoveKind::Walk,
            fall: 0.0,
        };
        let moves = [m(0, 1), m(1, 0), m(1, 2), m(2, 3), m(3, 2), m(5, 0)];
        let (comp, count) = components(6, &moves, &|i| i == 5);
        assert_eq!(count, 3);
        assert_eq!(comp[0], comp[1]);
        assert_eq!(comp[2], comp[3]);
        assert_ne!(comp[0], comp[2]);
        assert_ne!(comp[4], comp[0]);
        assert_eq!(comp[5], u32::MAX);
        let mut size = vec![0u32; count as usize];
        for &c in comp.iter().filter(|&&c| c != u32::MAX) {
            size[c as usize] += 1;
        }
        // two spawns on 2 and 3 beat one on 0; a tie goes to the larger; no spawn: the largest
        assert_eq!(pick_most(&[0, 2, 3], &comp, &size), Some(comp[2]));
        assert_eq!(pick_most(&[4, 0], &comp, &size), Some(comp[0]));
        assert!([comp[0], comp[2]].contains(&pick_most(&[], &comp, &size).unwrap()));
        assert_eq!(pick_largest(&[comp[4], comp[2]], &size), Some(comp[2]));
        let from_main = reached(6, &moves, &[0, 1], false);
        assert_eq!(from_main, [true, true, true, true, false, false]);
        let to_main = reached(6, &moves, &[0, 1], true);
        assert_eq!(to_main, [true, true, false, false, false, true]);
    }

    #[test]
    fn a_pit_with_one_way_in_is_a_trap_with_that_entry() {
        // a floor at y 0 round a pit 200 x 200 and 200 deep (a drop that does not hurt, a side no jump climbs),
        // fenced 150 high round its rim but for a gap 100 wide on its west side
        let mut solids = walls(300.0);
        solids.extend([
            Aabb::new([-600.0, -300.0, -600.0], [-100.0, 0.0, 600.0]),
            Aabb::new([100.0, -300.0, -600.0], [600.0, 0.0, 600.0]),
            Aabb::new([-100.0, -300.0, -600.0], [100.0, 0.0, -100.0]),
            Aabb::new([-100.0, -300.0, 100.0], [100.0, 0.0, 600.0]),
            Aabb::new([-100.0, -300.0, -100.0], [100.0, -200.0, 100.0]),
            Aabb::new([-120.0, 0.0, 100.0], [120.0, 150.0, 120.0]),
            Aabb::new([-120.0, 0.0, -120.0], [120.0, 150.0, -100.0]),
            Aabb::new([100.0, 0.0, -100.0], [120.0, 150.0, 100.0]),
            Aabb::new([-120.0, 0.0, -100.0], [-100.0, 150.0, -50.0]),
            Aabb::new([-120.0, 0.0, 50.0], [-100.0, 150.0, 100.0]),
        ]);
        let (w, spawns) = walk_world(&boxes_world(&solids, &[], &[[400.0, 10.0, 400.0]]));
        let (nodes, moves, an) = reach(&w, &spawns, ROOM, 48.0);
        let traps = node_areas(&nodes, &moves.moves, &|i| an.class[i] == Class::Trap);
        assert_eq!(traps.len(), 1, "{traps:?}");
        let t = &traps[0];
        assert!(t.nodes.len() >= 4, "{t:?}");
        assert!(
            t.floor[0] > -201.0 && t.floor[1] < -199.0,
            "the pit's floor: {t:?}"
        );
        assert!(
            t.min[0] >= -120.0 && t.max[0] <= 120.0 && t.min[1] >= -120.0 && t.max[1] <= 120.0,
            "{t:?}"
        );
        // every way in starts on the floor at the gap
        let entries: Vec<&Move> = moves
            .moves
            .iter()
            .filter(|m| {
                an.class[m.from as usize] == Class::Main && an.class[m.to as usize] == Class::Trap
            })
            .collect();
        assert!(!entries.is_empty());
        for m in &entries {
            let a = nodes.pos(m.from as usize);
            assert!(
                a.x < -100.0 && a.x > -200.0 && a.z.abs() < 100.0 && a.y.abs() < 1.0,
                "{a:?} {m:?}"
            );
            assert!(m.fall > 150.0 && m.fall < 260.0, "{m:?}");
        }
        // no way out of the pit, and nothing kills in the room
        assert!(moves
            .moves
            .iter()
            .all(|m| an.class[m.from as usize] != Class::Trap
                || an.class[m.to as usize] == Class::Trap));
        assert!(moves.deadly.is_empty(), "{:?}", moves.deadly);
        // the floor round the pit is one component; the fence tops are islands
        for (i, n) in nodes.list.iter().enumerate() {
            if n.floor.abs() < 1.0 {
                assert_eq!(an.class[i], Class::Main, "{:?}", nodes.pos(i));
            } else if n.floor > 100.0 {
                assert_eq!(an.class[i], Class::Island, "{:?}", nodes.pos(i));
            }
        }
    }

    #[test]
    fn a_ledge_over_a_700_drop_is_deadly() {
        // a platform 700 high over the west half of a room
        let mut solids = walls(1_000.0);
        solids.extend([
            Aabb::new([-600.0, -300.0, -600.0], [600.0, 0.0, 600.0]),
            Aabb::new([-600.0, 0.0, -600.0], [0.0, 700.0, 600.0]),
        ]);
        let (w, spawns) = walk_world(&boxes_world(&solids, &[], &[[-300.0, 710.0, 0.0]]));
        let (nodes, moves, an) = reach(&w, &spawns, ROOM, 50.0);
        for (i, n) in nodes.list.iter().enumerate() {
            let want = if n.floor > 600.0 {
                Class::Main
            } else {
                Class::Island
            };
            assert_eq!(an.class[i], want, "{:?}", nodes.pos(i));
        }
        let deadly: Vec<&DeadlyMove> = moves
            .deadly
            .iter()
            .filter(|m| an.class[m.from as usize] == Class::Main)
            .collect();
        // every node of the column along the edge (24) kills, running off and jumping off; so do the two by the
        // corners one column further in, whose diagonal runs slide along a wall and off the edge
        let edge: std::collections::BTreeSet<u32> = deadly
            .iter()
            .map(|m| m.from)
            .filter(|&n| (nodes.pos(n as usize).x + 25.0).abs() < 1.0)
            .collect();
        assert_eq!(edge.len(), 24, "{deadly:?}");
        for m in &deadly {
            let x = nodes.pos(m.from as usize).x;
            assert!(
                (x + 25.0).abs() < 1.0 || (x + 75.0).abs() < 1.0,
                "by the edge: {m:?}"
            );
            match m.cause {
                Cause::Fall(h) => assert!(h > 690.0 && h < 760.0, "{m:?}"),
                c => panic!("not a fall: {c:?}"),
            }
            assert!(m.at.x > 0.0 && m.at.y < 60.0, "landed below: {m:?}");
        }
        assert!(an.class.iter().all(|c| *c != Class::Trap));
    }

    #[test]
    fn a_step_of_20_units_is_walked_both_ways() {
        let mut solids = walls(300.0);
        solids.extend([
            Aabb::new([-600.0, -300.0, -600.0], [0.0, 0.0, 600.0]),
            Aabb::new([0.0, -300.0, -600.0], [600.0, 20.0, 600.0]),
        ]);
        let (w, spawns) = walk_world(&boxes_world(&solids, &[], &[[-300.0, 10.0, 0.0]]));
        let (nodes, moves, an) = reach(&w, &spawns, ROOM, 50.0);
        assert_eq!(nodes.len(), 24 * 24);
        assert!(an.class.iter().all(|c| *c == Class::Main), "one component");
        assert_eq!(an.comp_size.len(), 1);
        assert!(moves.deadly.is_empty());
        // up and down across the step: simulated runs
        let at = |x: f32, z: f32| nodes.at(V3::new(x, 0.0, z) + V3::new(0.0, BODY[1], 0.0));
        let low = nodes.at(V3::new(-25.0, BODY[1], 25.0)).unwrap();
        let high = nodes.at(V3::new(25.0, 20.0 + BODY[1], 25.0)).unwrap();
        assert!(at(-25.0, 25.0).is_some());
        let find = |a: u32, b: u32| moves.moves.iter().find(|m| m.from == a && m.to == b);
        assert_eq!(find(low, high).map(|m| m.kind), Some(MoveKind::Run));
        assert_eq!(find(high, low).map(|m| m.kind), Some(MoveKind::Run));
        // the flat floors are joined without a simulation
        assert!(moves.stats.flat_walks > 2_000, "{:?}", moves.stats);
    }

    #[test]
    fn a_cut_doorway_joins_the_rooms_either_side_of_a_wall() {
        // the fixture piece's floor at y 0 over x 9,000 .. 10,600 with its wall at x 10,500 and the spawn at
        // x 10,000: east of the wall is out of reach until a doorway is cut through it
        let composed = |cuts: Vec<Aabb>| {
            let map = fixture::map(&fixture::piece(cuts));
            let mut read = fixture::reader();
            compose::compose(&map, &mut read).unwrap().unwrap().bytes
        };
        let area = [9_000.0, -1_000.0, 10_600.0, 1_000.0];
        let east = |nodes: &Nodes, an: &Analysis| -> (usize, usize) {
            let mut n = (0, 0);
            for i in 0..nodes.len() {
                let q = nodes.pos(i);
                if q.x > 10_500.0 && q.y.abs() < 1.0 {
                    n.0 += 1;
                    if an.class[i] == Class::Main {
                        n.1 += 1;
                    }
                }
            }
            n
        };
        let (w, spawns) = walk_world(&composed(Vec::new()));
        let (nodes, _, an) = reach(&w, &spawns, area, 50.0);
        let (count, main) = east(&nodes, &an);
        assert_eq!(count, 2 * 40, "two columns east of the wall");
        assert_eq!(main, 0, "walled off");
        // the doorway (and a hole cut in the floor west of the wall)
        let door = Aabb::new([10_400.0, 1.0, -60.0], [10_700.0, 150.0, 60.0]);
        let hole = Aabb::new([9_400.0, -50.0, 400.0], [9_800.0, 50.0, 800.0]);
        let (w, spawns) = walk_world(&composed(vec![door, hole]));
        let (nodes, moves, an) = reach(&w, &spawns, area, 50.0);
        let (count, main) = east(&nodes, &an);
        assert_eq!((count, main), (2 * 40, 2 * 40), "through the doorway");
        // the floor's edges kill: a running body flies past the kill slab under the region and out of the world,
        // and a body running into the hole dies in the slab
        let mut by_hole = 0;
        for m in moves
            .deadly
            .iter()
            .filter(|m| an.class[m.from as usize] == Class::Main)
        {
            let a = nodes.pos(m.from as usize);
            let near_hole = a.x > 9_300.0 && a.x < 9_900.0 && a.z > 300.0 && a.z < 900.0;
            match m.cause {
                Cause::KillVolume(k) => {
                    assert!(near_hole, "{m:?}");
                    assert!(w.kills[k as usize].name.starts_with("piece_1~pit"), "{m:?}");
                    by_hole += 1;
                }
                Cause::FellOut => assert!(!near_hole, "{m:?}"),
                Cause::Fall(_) => panic!("no floor under the region to land on: {m:?}"),
            }
        }
        assert!(by_hole >= 16, "{by_hole}");
    }
}
