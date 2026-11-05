//! A navigation grid for the bots (BOT-93), sans-IO: a lattice of positions a player cylinder can stand on, grown
//! step by step from the places the bot has stood (its spawns), every link checked against the loaded collision
//! world (`dc-sim`), A* over it with the path pulled straight where the way is clear. The greedy way-finder
//! (`crate::hunt`) stays the fallback where the grid does not reach yet. A DeCombat bot behaviour, no 2009 rule.
// DC-DEFAULT: BOT-93 (proposed)

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use dc_sim::{Collider, CollisionWorld, ConvexVolume, Cylinder, MovementParams, V3};

/// Lattice step, units.
// DC-DEFAULT: BOT-93 (proposed) 48 units (a body is 32 wide)
pub const STEP: f32 = 48.0;
/// How high a link may climb (the mover's step height, MOVEMENT_2009.md §2 STEP_HEIGHT 30).
pub const STEP_UP: f32 = 30.0;
/// How far a link may drop (a walk down stairs or a ledge the mover takes without harm).
// DC-DEFAULT: BOT-93 (proposed) 96 units
pub const STEP_DOWN: f32 = 96.0;
/// The highest walk off an edge a link takes (falls up to 300 do no damage, DSV-76).
// DC-DEFAULT: BOT-119 (proposed) 200 units
pub const FALL_DROP: f32 = 200.0;
/// The extra path cost of a fall of more than [`STEP_DOWN`].
// DC-DEFAULT: BOT-119 (proposed) 300 units
pub const FALL_COST: f32 = 300.0;
/// Sub-step of the stairway check: shorter than a stair's tread, so each sub-step rises at most one riser.
// DC-DEFAULT: BOT-112 (proposed) 8 units
pub const CLIMB_SUB: f32 = 8.0;
/// The most a stairway link may rise over one lattice step (a steeper way is a wall or a ladder).
// DC-DEFAULT: BOT-112 (proposed) 60 units over 48
pub const CLIMB_MAX: f32 = 60.0;
/// A waypoint counts as reached within this flat distance.
pub const REACHED: f32 = 28.0;
/// Probes per `expand` call (one call per session tick keeps the grid growing without stalling the bot).
// DC-DEFAULT: BOT-93 (proposed) 24
pub const BUDGET: usize = 24;
/// The eight lattice directions.
const DIRS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

type Key = (i32, i32, i32);

fn key_of(p: V3) -> Key {
    (
        (p.x / STEP).round() as i32,
        (p.y / STEP_UP).round() as i32,
        (p.z / STEP).round() as i32,
    )
}

fn flat(a: V3, b: V3) -> f32 {
    ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

fn dist(a: V3, b: V3) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

/// A drop the greedy way-finder does not walk off (higher drops hurt or kill).
// DC-DEFAULT: BOT-111 (proposed) 300 units
pub const DEADLY_DROP: f32 = 300.0;

/// Whether a body (half extents `dims`) at `p` is in danger: in a kill volume, above one (nothing but a kill volume
/// or the void below), or above a drop of more than [`DEADLY_DROP`]. The greedy way-finder's probes that end in
/// danger count as no progress, so it turns away from water and ledges.
// DC-DEFAULT: BOT-111 (proposed)
pub fn in_danger(
    world: &CollisionWorld,
    kill: &[ConvexVolume],
    dims: [f32; 3],
    p: [f32; 3],
) -> bool {
    let shape = Cylinder::from_dims(V3::new(dims[0], dims[1], dims[2]));
    let at = V3::new(p[0], p[1], p[2]);
    let low = V3::new(p[0], p[1] - 4000.0, p[2]);
    let Some(h) = world.sweep(&shape, at, low) else {
        return true;
    };
    let landing = at + (low - at) * h.t;
    at.y - landing.y > DEADLY_DROP
        || kill
            .iter()
            .any(|k| k.touches(&shape, at) || k.touches(&shape, landing))
}

#[derive(Debug, Clone)]
struct Node {
    pos: V3,
    links: Vec<Key>,
    expanded: bool,
}

/// The grid of one world for one body size.
pub struct NavGrid {
    world: Arc<CollisionWorld>,
    shape: Cylinder,
    dims: [f32; 3],
    /// BOT-111: the kill volumes the server created; no node stands where the body touches one.
    kill: Vec<ConvexVolume>,
    nodes: HashMap<Key, Node>,
    frontier: VecDeque<Key>,
    pub probes: u64,
    /// BOT-118: the cosine of the steepest floor the mover stands on (`MovementParams::standable_cos`).
    standable: f32,
    /// BOT-119: the first place seeded (the bot's spawn), a count of the grid's changes, and the nodes from which
    /// a way leads back to that place as of a change count (the others are pits: a drop leads in, nothing out).
    home: Option<Key>,
    changes: u64,
    escape: Option<(u64, HashSet<Key>)>,
}

#[derive(Copy, Clone, PartialEq)]
struct Open {
    f: f32,
    key: Key,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.total_cmp(&self.f)
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl NavGrid {
    pub fn new(world: Arc<CollisionWorld>, dims: [f32; 3]) -> Self {
        NavGrid {
            world,
            shape: Cylinder::from_dims(V3::new(dims[0], dims[1], dims[2])),
            dims,
            kill: Vec::new(),
            nodes: HashMap::new(),
            frontier: VecDeque::new(),
            probes: 0,
            standable: MovementParams::default().standable_cos,
            home: None,
            changes: 0,
            escape: None,
        }
    }

    /// The grid with the kill volumes it keeps its nodes out of (BOT-111).
    // DC-DEFAULT: BOT-111 (proposed) the bots' grid has no node where the standing body touches a kill volume the
    // server created, so a route never leads into one (water, a fall, a pit); the server kills every client in them
    pub fn with_kill_volumes(mut self, kill: Vec<ConvexVolume>) -> Self {
        self.kill = kill;
        self
    }

    /// The half extents of the body the grid was grown for.
    pub fn dims(&self) -> [f32; 3] {
        self.dims
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The standing position at the flat place of `p`, between `p.y + STEP_UP` and `p.y - STEP_DOWN`, when the
    /// body fits there. The search starts at `p.y` (then half a step lower) where the body does not fit at
    /// `p.y + STEP_UP`: a way down under a low ceiling (stairs under a bridge).
    // DC-DEFAULT: BOT-112 (proposed) the lower starts
    fn stand(&mut self, p: V3) -> Option<V3> {
        let bottom = V3::new(p.x, p.y - STEP_DOWN, p.z);
        self.probes += 1;
        let top = [STEP_UP, 0.0, -STEP_UP * 0.5]
            .into_iter()
            .map(|up| V3::new(p.x, p.y + up, p.z))
            .find(|t| !self.world.overlaps(&self.shape, *t))?;
        let hit = self.world.sweep(&self.shape, top, bottom)?;
        // BOT-118: no place on a slope the mover slides down (it is no ground to it: no walking, no jump)
        if hit.face_normal.y < self.standable {
            return None;
        }
        let y = top.y + (bottom.y - top.y) * hit.t;
        let at = V3::new(p.x, y, p.z);
        if self.kill.iter().any(|v| v.touches(&self.shape, at)) {
            return None;
        }
        (!self.world.overlaps(&self.shape, at)).then_some(at)
    }

    /// Whether the body walks from `a` to `b` (both standing positions): the raised cylinder sweeps free.
    fn walkable(&mut self, a: V3, b: V3) -> bool {
        self.lifted_clear(a, b, STEP_UP * 0.5)
    }

    /// Whether the cylinder raised by `up` sweeps free from `a` to `b` (each end unraised where the raised body
    /// does not fit).
    fn lifted_clear(&mut self, a: V3, b: V3, up: f32) -> bool {
        let lift = |p: V3| V3::new(p.x, p.y + up, p.z);
        self.probes += 1;
        let (ra, rb) = (lift(a), lift(b));
        let from = if self.world.overlaps(&self.shape, ra) {
            a
        } else {
            ra
        };
        let to = if self.world.overlaps(&self.shape, rb) {
            b
        } else {
            rb
        };
        self.world
            .sweep(&self.shape, from, to)
            .is_none_or(|h| h.t >= 0.999)
    }

    /// A stairway the lattice step misses (it rises more than [`STEP_UP`] over one [`STEP`]): from the standing
    /// position `a` toward the flat place of `q` in sub-steps of [`CLIMB_SUB`] units, each a standing place at most
    /// [`STEP_UP`] above the last (no drop) that the body raised by [`STEP_UP`] reaches; the end when the way rose
    /// more than [`STEP_UP`] and at most [`CLIMB_MAX`] in all.
    // DC-DEFAULT: BOT-112 (proposed)
    fn climb(&mut self, a: V3, q: V3) -> Option<V3> {
        let n = (flat(a, q) / CLIMB_SUB).ceil().max(1.0) as u32;
        let mut cur = a;
        for k in 1..=n {
            let t = k as f32 / n as f32;
            let p = V3::new(a.x + (q.x - a.x) * t, cur.y, a.z + (q.z - a.z) * t);
            let s = self.stand(p)?;
            if s.y < cur.y - 1.0 || !self.lifted_clear(cur, s, STEP_UP) {
                return None;
            }
            cur = s;
        }
        let rise = cur.y - a.y;
        (rise > STEP_UP && rise <= CLIMB_MAX).then_some(cur)
    }

    /// The standing place at the flat place of `q` the body walks to from the node `k` at `from`: the flat step (or
    /// a drop of at most [`STEP_DOWN`]), else a stairway up, else a stairway down (the lower place climbs back up to
    /// this one; the straight sweep meets the stair edges).
    // DC-DEFAULT: BOT-112 (proposed) stairways
    fn step_to(&mut self, from: V3, k: Key, q: V3) -> Option<V3> {
        let below = self.stand(q).filter(|at| key_of(*at) != k);
        if let Some(at) = below.filter(|at| self.walkable(from, *at)) {
            return Some(at);
        }
        if let Some(at) = self.climb(from, q) {
            return Some(at);
        }
        if let Some(at) =
            below.filter(|at| from.y - at.y > STEP_UP && self.climb(*at, from).is_some())
        {
            return Some(at);
        }
        self.walk_off(from, q).filter(|at| key_of(*at) != k)
    }

    /// BOT-119: the place the body lands on when it walks off an edge from the standing place `from` toward the flat
    /// place of `q`: the raised body moves level to above `q`, then falls freely to a standing place more than
    /// [`STEP_UP`] and at most [`FALL_DROP`] lower (out of kill volumes, on standable floor). The straight sweep of
    /// [`Self::walkable`] meets the edge of a box or a ledge, so before no link led off one: a bot on a crate top had
    /// no way down on the grid.
    // DC-DEFAULT: BOT-119 (proposed)
    fn walk_off(&mut self, from: V3, q: V3) -> Option<V3> {
        let over = V3::new(q.x, from.y, q.z);
        if !self.lifted_clear(from, over, STEP_UP * 0.5) || self.world.overlaps(&self.shape, over) {
            return None;
        }
        self.probes += 1;
        let low = V3::new(q.x, from.y - FALL_DROP, q.z);
        let hit = self.world.sweep(&self.shape, over, low)?;
        if hit.face_normal.y < self.standable {
            return None;
        }
        let at = over + (low - over) * hit.t;
        if from.y - at.y <= STEP_UP
            || self.world.overlaps(&self.shape, at)
            || self.kill.iter().any(|v| v.touches(&self.shape, at))
        {
            return None;
        }
        Some(at)
    }

    /// BOT-118: whether the body walks from the standing place `a` to the known node at `b`: a flat step or a drop
    /// (a step up of at most [`STEP_UP`]) over floor, or a stairway that ends at `b`.
    // DC-DEFAULT: BOT-118 (proposed)
    fn reaches(&mut self, a: V3, b: V3) -> bool {
        if b.y - a.y <= STEP_UP + 1.0 && self.walkable(a, b) && self.supported(a, b) {
            return true;
        }
        if self
            .climb(a, b)
            .is_some_and(|c| flat(c, b) < 1.0 && (c.y - b.y).abs() <= 2.0)
        {
            return true;
        }
        // BOT-119: or a walk off an edge that lands on it
        a.y - b.y > STEP_UP
            && self
                .walk_off(a, b)
                .is_some_and(|c| flat(c, b) < 1.0 && (c.y - b.y).abs() <= 2.0)
    }

    /// Whether the body finds a floor all along the straight line from `a` to `b` (both standing positions): a
    /// standing place at every half step, none in a kill volume. String pulling takes a shortcut only over floor, so
    /// a straight leg never crosses water or a drop that the grid went around. BOT-112: and no standing place on the
    /// way lies more than [`STEP_UP`] above the one before it (a shortcut up an edge the body cannot step).
    // DC-DEFAULT: BOT-111 (proposed), BOT-112 (proposed)
    fn supported(&mut self, a: V3, b: V3) -> bool {
        let n = (flat(a, b) / (STEP * 0.5)).ceil() as u32;
        let mut last = a.y;
        for k in 1..n {
            let t = k as f32 / n as f32;
            match self.stand(a + (b - a) * t) {
                Some(s) if s.y - last <= STEP_UP => last = s.y,
                _ => return false,
            }
        }
        b.y - last <= STEP_UP
    }

    /// Adds the place the bot stands at (a spawn, or wherever it is) as a node when it is not known yet.
    pub fn seed(&mut self, pos: [f32; 3]) -> bool {
        let p = V3::new(pos[0], pos[1], pos[2]);
        let k = key_of(p);
        if self.nodes.contains_key(&k) {
            return false;
        }
        let Some(at) = self.stand(p) else {
            return false;
        };
        self.nodes.insert(
            k,
            Node {
                pos: at,
                links: Vec::new(),
                expanded: false,
            },
        );
        self.frontier.push_back(k);
        self.home.get_or_insert(k);
        self.changes += 1;
        true
    }

    /// Grows the grid by up to `budget` probes from its frontier; returns the nodes added. With a `focus` the
    /// frontier place nearest to it is looked at first (the grid grows toward where the bot wants to go), else
    /// the oldest.
    pub fn expand(&mut self, budget: usize, focus: Option<[f32; 3]>) -> usize {
        let mut added = 0;
        let start = self.probes;
        while self.probes - start < budget as u64 {
            let k = match focus {
                Some(f) => {
                    let fp = V3::new(f[0], f[1], f[2]);
                    let best = self
                        .frontier
                        .iter()
                        .enumerate()
                        .filter_map(|(i, k)| self.nodes.get(k).map(|n| (i, dist(n.pos, fp))))
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|x| x.0);
                    match best {
                        Some(i) => self.frontier.remove(i).unwrap(),
                        None => break,
                    }
                }
                None => match self.frontier.pop_front() {
                    Some(k) => k,
                    None => break,
                },
            };
            let Some(node) = self.nodes.get(&k).cloned() else {
                continue;
            };
            if node.expanded {
                continue;
            }
            let mut links = node.links.clone();
            for (dx, dz) in DIRS {
                let q = V3::new(
                    node.pos.x + dx as f32 * STEP,
                    node.pos.y,
                    node.pos.z + dz as f32 * STEP,
                );
                // BOT-112: where the lattice place is no way, the places half a step to either side of it (a door
                // or a gap narrower than the lattice that its columns miss)
                let (sx, sz) = {
                    let l = ((dx * dx + dz * dz) as f32).sqrt();
                    (-dz as f32 / l * STEP * 0.5, dx as f32 / l * STEP * 0.5)
                };
                let mut found = self.step_to(node.pos, k, q);
                for side in [1.0f32, -1.0] {
                    if found.is_none() {
                        found = self.step_to(
                            node.pos,
                            k,
                            V3::new(q.x + side * sx, q.y, q.z + side * sz),
                        );
                    }
                }
                let Some(at) = found else {
                    continue;
                };
                let qk = key_of(at);
                if qk == k {
                    continue;
                }
                // BOT-118: the link leads to the node already known at that key, which may stand up to a lattice
                // step away from the place just probed (a half-step side probe, a stairway end): the body must reach
                // that node itself, or the link is not made (a link checked only to the probed place led bots into
                // railings and stair sides on Gray Hammer and kept them pushing there)
                let at = match self.nodes.get(&qk).map(|n| n.pos) {
                    Some(np) if dist(np, at) > 1.0 => {
                        if !self.reaches(node.pos, np) {
                            continue;
                        }
                        np
                    }
                    _ => at,
                };
                if !links.contains(&qk) {
                    links.push(qk);
                }
                // BOT-112: a link is walked both ways only when the way back climbs at most STEP_UP or is a stairway;
                // a ledge the bot drops from is not a way up
                let back = node.pos.y - at.y <= STEP_UP || self.climb(at, node.pos).is_some();
                match self.nodes.get_mut(&qk) {
                    Some(n) => {
                        if back && !n.links.contains(&k) {
                            n.links.push(k);
                        }
                    }
                    None => {
                        self.nodes.insert(
                            qk,
                            Node {
                                pos: at,
                                links: if back { vec![k] } else { Vec::new() },
                                expanded: false,
                            },
                        );
                        self.frontier.push_back(qk);
                        added += 1;
                    }
                }
            }
            if let Some(n) = self.nodes.get_mut(&k) {
                n.links = links;
                n.expanded = true;
            }
            self.changes += 1;
        }
        added
    }

    /// BOT-119: brings the set of nodes with a way back to the first seeded place up to date.
    fn refresh_escape(&mut self) {
        let Some(home) = self.home else {
            return;
        };
        if self.escape.as_ref().is_some_and(|e| e.0 == self.changes) {
            return;
        }
        let mut rev: HashMap<Key, Vec<Key>> = HashMap::new();
        for (k, n) in &self.nodes {
            for l in &n.links {
                rev.entry(*l).or_default().push(*k);
            }
        }
        let mut seen = HashSet::from([home]);
        let mut q = VecDeque::from([home]);
        while let Some(k) = q.pop_front() {
            for f in rev.get(&k).into_iter().flatten() {
                if seen.insert(*f) {
                    q.push_back(*f);
                }
            }
        }
        self.escape = Some((self.changes, seen));
    }

    /// BOT-119: whether the node at `k` lies in a pit (no way back to the first seeded place, as last computed).
    fn in_pit(&self, k: &Key) -> bool {
        self.escape.as_ref().is_some_and(|e| !e.1.contains(k))
    }

    /// The nodes within `r` units (flat) of `p` and the places their links lead to (diagnostics and tests).
    pub fn links_near(&self, p: [f32; 3], r: f32) -> Vec<([f32; 3], Vec<[f32; 3]>)> {
        let c = V3::new(p[0], p[1], p[2]);
        let mut out: Vec<([f32; 3], Vec<[f32; 3]>)> = self
            .nodes
            .values()
            .filter(|n| flat(n.pos, c) <= r)
            .map(|n| {
                let to = n
                    .links
                    .iter()
                    .filter_map(|l| self.nodes.get(l))
                    .map(|m| [m.pos.x, m.pos.y, m.pos.z])
                    .collect();
                ([n.pos.x, n.pos.y, n.pos.z], to)
            })
            .collect();
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
        out
    }

    /// Whether the frontier still has places to look at.
    pub fn growing(&self) -> bool {
        !self.frontier.is_empty()
    }

    /// The known node nearest to `p` within `within` units (3D), by key.
    fn nearest(&self, p: V3, within: f32) -> Option<Key> {
        self.nearest_where(p, within, |_| true)
    }

    /// The known node nearest to `p` within `within` units (3D) among those `allow` lets through.
    fn nearest_where(&self, p: V3, within: f32, allow: impl Fn(&Key) -> bool) -> Option<Key> {
        self.nodes
            .iter()
            .filter(|(k, _)| allow(k))
            .map(|(k, n)| (dist(n.pos, p), *k))
            .filter(|(d, _)| *d <= within)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| x.1)
    }

    /// BOT-117: a node at least `min` units (flat) from `from`, picked by `pick` among them (in key order, so the same
    /// grid and pick give the same node). `None` when no node is that far.
    // DC-DEFAULT: BOT-117 (proposed)
    pub fn far_node(&self, from: [f32; 3], min: f32, pick: u64) -> Option<[f32; 3]> {
        let f = V3::new(from[0], from[1], from[2]);
        let mut far: Vec<(Key, V3)> = self
            .nodes
            .iter()
            .filter(|(k, n)| flat(n.pos, f) >= min && !self.in_pit(k))
            .map(|(k, n)| (*k, n.pos))
            .collect();
        if far.is_empty() {
            return None;
        }
        far.sort_by_key(|x| x.0);
        let p = far[(pick % far.len() as u64) as usize].1;
        Some([p.x, p.y, p.z])
    }

    /// Whether a ray from `a` reaches `b` through the world.
    fn sees(&self, a: V3, b: V3) -> bool {
        self.world.ray(a, b).is_none_or(|h| h.t >= 0.999)
    }

    /// The known node nearest to `p` within `within` units (3D) whose centre sees `p` (a ray through the world).
    // DC-DEFAULT: BOT-112 (proposed)
    fn nearest_in_sight(
        &self,
        p: V3,
        within: f32,
        allow: impl Fn(&Key) -> bool,
    ) -> Option<Key> {
        let mut near: Vec<(f32, Key, V3)> = self
            .nodes
            .iter()
            .filter(|(k, _)| allow(k))
            .map(|(k, n)| (dist(n.pos, p), *k, n.pos))
            .filter(|(d, _, _)| *d <= within)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        near.into_iter()
            .find(|(_, _, q)| self.sees(*q, p))
            .map(|x| x.1)
    }

    /// BOT-113: gives up the link from the node the bot stands at (nearest to `from`) that leads most directly
    /// toward `toward`: the body made no way along it. `false` when the bot stands off the grid.
    // DC-DEFAULT: BOT-113 (proposed)
    pub fn ban_step(&mut self, from: [f32; 3], toward: [f32; 3]) -> bool {
        let f = V3::new(from[0], from[1], from[2]);
        let Some(k) = self.nearest(f, STEP * 1.5) else {
            return false;
        };
        let (dx, dz) = (toward[0] - f.x, toward[2] - f.z);
        let dl = (dx * dx + dz * dz).sqrt().max(1e-3);
        let here = self.nodes[&k].pos;
        let best = self.nodes[&k]
            .links
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let q = self.nodes.get(l)?.pos;
                let (lx, lz) = (q.x - here.x, q.z - here.z);
                let ll = (lx * lx + lz * lz).sqrt().max(1e-3);
                Some((i, (lx * dx + lz * dz) / (ll * dl)))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, _)) => {
                self.nodes.get_mut(&k).unwrap().links.remove(i);
                self.changes += 1;
                true
            }
            None => false,
        }
    }

    /// A path of positions from `from` to `to` over the grid (the first entry is the first waypoint, the last is
    /// the known node nearest to `to`). `None` when `from` is off the grid or nothing reachable lies near `to`.
    /// The way is pulled straight: a waypoint is dropped when the one after it is walkable from the one before.
    pub fn path(&mut self, from: [f32; 3], to: [f32; 3]) -> Option<Vec<[f32; 3]>> {
        let (f, t) = (
            V3::new(from[0], from[1], from[2]),
            V3::new(to[0], to[1], to[2]),
        );
        self.refresh_escape();
        let start = self.nearest(f, STEP * 1.5)?;
        // BOT-119: a bot out of the pits plans no way into one (a drop in, nothing out), nor ends its way there
        let avoid = !self.in_pit(&start);
        let allow = |k: &Key| !avoid || !self.in_pit(k);
        // the goal node: the known node nearest to the goal (a far goal in unknown ground gets the closest edge);
        // BOT-112: of the nodes near the goal one it can see, not the one behind the wall it stands at (that way two
        // bots each walk to their side of a wall and stand there)
        let goal = self
            .nearest_in_sight(t, STEP * 3.0, allow)
            .or_else(|| self.nearest_where(t, f32::MAX, allow))?;
        if dist(self.nodes[&goal].pos, t) > STEP * 1.5
            && dist(self.nodes[&goal].pos, t) >= dist(f, t)
        {
            return None;
        }
        let h = |k: Key| {
            let p = self.nodes[&k].pos;
            flat(p, self.nodes[&goal].pos) + (p.y - self.nodes[&goal].pos.y).abs()
        };
        let mut open = BinaryHeap::new();
        let mut g: HashMap<Key, f32> = HashMap::new();
        let mut came: HashMap<Key, Key> = HashMap::new();
        let mut closed: HashSet<Key> = HashSet::new();
        g.insert(start, 0.0);
        open.push(Open {
            f: h(start),
            key: start,
        });
        let mut found = false;
        while let Some(Open { key, .. }) = open.pop() {
            if key == goal {
                found = true;
                break;
            }
            if !closed.insert(key) {
                continue;
            }
            let (pos, links) = {
                let n = &self.nodes[&key];
                (n.pos, n.links.clone())
            };
            let gk = g[&key];
            for l in links {
                if closed.contains(&l) || !allow(&l) {
                    continue;
                }
                let Some(ln) = self.nodes.get(&l) else {
                    continue;
                };
                // BOT-119: a fall of more than STEP_DOWN costs extra (a way down a stair or a short drop wins)
                let fall = if pos.y - ln.pos.y > STEP_DOWN {
                    FALL_COST
                } else {
                    0.0
                };
                let ng = gk + dist(pos, ln.pos) + fall;
                if g.get(&l).is_none_or(|old| ng < *old) {
                    g.insert(l, ng);
                    came.insert(l, key);
                    open.push(Open {
                        f: ng + h(l),
                        key: l,
                    });
                }
            }
        }
        if !found {
            return None;
        }
        let mut keys = vec![goal];
        let mut k = goal;
        while let Some(p) = came.get(&k) {
            keys.push(*p);
            k = *p;
        }
        keys.reverse();
        let pts: Vec<V3> = keys.iter().map(|k| self.nodes[k].pos).collect();
        // string pulling
        let mut out = Vec::new();
        let mut i = 0;
        while i < pts.len() {
            let mut j = i + 1;
            while j + 1 < pts.len()
                && self.walkable(pts[i], pts[j + 1])
                && self.supported(pts[i], pts[j + 1])
            {
                j += 1;
            }
            if j < pts.len() {
                out.push(pts[j]);
            }
            i = j;
        }
        if out.is_empty() {
            out.push(pts[pts.len() - 1]);
        }
        Some(out.into_iter().map(|p| [p.x, p.y, p.z]).collect())
    }
}

/// The waypoints a bot follows (BOT-93): the current path to its goal and when it was planned.
#[derive(Debug, Default)]
pub struct Route {
    pub points: Vec<[f32; 3]>,
    pub goal: Option<[f32; 3]>,
    pub planned_at: Option<u32>,
    pub replans: u32,
    /// The flat distance to the first waypoint when the plan was made (the off-route check's reference).
    leg: f32,
    /// BOT-113: the least length of the way left (to the first waypoint, then along the waypoints) and when the bot
    /// last got it shorter, and the goal then (a goal that moves far starts the count again).
    progress: Option<(f32, u32, [f32; 3])>,
    /// BOT-113: until when the grid gives no way (the greedy way-finder explores past the grid's edge).
    greedy_until: Option<u32>,
    /// BOT-113: the links given up because the body made no way along them.
    pub stuck: u32,
}

/// The way left must get [`ROUTE_PROGRESS`] shorter within this time, or the step toward the first waypoint is
/// given up (a link the grid found that the body cannot walk: a ledge, a narrow ramp, a low beam).
// DC-DEFAULT: BOT-113 (proposed) 2500 ms
pub const ROUTE_STUCK_MS: u32 = 2500;
/// How much shorter the way left must get to count as progress.
// DC-DEFAULT: BOT-113 (proposed) 32 units
pub const ROUTE_PROGRESS: f32 = 32.0;
/// A goal that moved this far starts the progress count again (a chased player runs away).
// DC-DEFAULT: BOT-113 (proposed) 512 units
pub const PROGRESS_GOAL_MOVED: f32 = 512.0;
/// How long the greedy way-finder explores once the bot stands at the grid's end short of its goal.
// DC-DEFAULT: BOT-113 (proposed) 3000 ms
pub const EDGE_GREEDY_MS: u32 = 3000;

/// Replan this often even while on course, ms.
pub const REPLAN_MS: u32 = 800;
/// A goal that moved more than this gets a new plan.
pub const GOAL_MOVED: f32 = 128.0;
/// Off the next segment by more than this: a new plan.
pub const OFF_ROUTE: f32 = 160.0;

impl Route {
    /// The next waypoint toward `goal` from `pos` (planning or re-planning on the grid when due); `None` when the
    /// grid has no way (the caller falls back to the greedy way-finder).
    pub fn next(
        &mut self,
        nav: &mut NavGrid,
        now: u32,
        pos: [f32; 3],
        goal: [f32; 3],
    ) -> Option<[f32; 3]> {
        let p = V3::new(pos[0], pos[1], pos[2]);
        if self
            .greedy_until
            .is_some_and(|t| (now.wrapping_sub(t) as i32) < 0)
        {
            return None;
        }
        self.greedy_until = None;
        // drop the waypoints already reached
        while self
            .points
            .first()
            .is_some_and(|w| flat(p, V3::new(w[0], w[1], w[2])) <= REACHED)
        {
            self.points.remove(0);
            self.leg = self
                .points
                .first()
                .map_or(0.0, |w| flat(p, V3::new(w[0], w[1], w[2])));
        }
        let moved = self.goal.is_none_or(|g| {
            flat(
                V3::new(g[0], g[1], g[2]),
                V3::new(goal[0], goal[1], goal[2]),
            ) > GOAL_MOVED
        });
        let stale = self
            .planned_at
            .is_none_or(|t| now.wrapping_sub(t) >= REPLAN_MS);
        // off route: farther from the next waypoint than when the leg began, by a margin
        let off = self
            .points
            .first()
            .is_some_and(|w| flat(p, V3::new(w[0], w[1], w[2])) > self.leg + OFF_ROUTE);
        if moved || stale || off || self.points.is_empty() {
            match nav.path(pos, goal) {
                Some(pts) => {
                    let end = pts[pts.len() - 1];
                    // short of the goal: far from it, or behind a wall from it (the grid has not grown to its side)
                    let (e, gv) = (
                        V3::new(end[0], end[1], end[2]),
                        V3::new(goal[0], goal[1], goal[2]),
                    );
                    let short =
                        flat(e, gv) > STEP * 1.5 || (flat(e, gv) > REACHED && !nav.sees(e, gv));
                    self.points = pts;
                    self.goal = Some(goal);
                    self.planned_at = Some(now);
                    self.replans += 1;
                    while self
                        .points
                        .first()
                        .is_some_and(|w| flat(p, V3::new(w[0], w[1], w[2])) <= REACHED)
                    {
                        self.points.remove(0);
                    }
                    self.leg = self
                        .points
                        .first()
                        .map_or(0.0, |w| flat(p, V3::new(w[0], w[1], w[2])));
                    // BOT-113: at the grid's end short of the goal: the greedy way-finder explores for a while (the
                    // places it walks seed the grid further)
                    if short && self.points.is_empty() {
                        self.greedy_until = Some(now.wrapping_add(EDGE_GREEDY_MS));
                        self.progress = None;
                        return None;
                    }
                }
                None => {
                    self.points.clear();
                    self.goal = None;
                    self.planned_at = Some(now);
                    // BOT-113: no way on the grid (off it, or at its end nearest the goal): the greedy way-finder
                    // walks for a while (a plan each tick made the bot walk back to the grid's end and stand there)
                    self.greedy_until = Some(now.wrapping_add(EDGE_GREEDY_MS));
                    self.progress = None;
                    return None;
                }
            }
        }
        let wp = self.points.first().copied()?;
        // BOT-113: the way left (to the first waypoint, then along the plan) must get shorter
        let mut left = flat(p, V3::new(wp[0], wp[1], wp[2]));
        for w in self.points.windows(2) {
            left += flat(
                V3::new(w[0][0], w[0][1], w[0][2]),
                V3::new(w[1][0], w[1][1], w[1][2]),
            );
        }
        let far = |g: [f32; 3]| {
            flat(
                V3::new(g[0], g[1], g[2]),
                V3::new(goal[0], goal[1], goal[2]),
            ) > PROGRESS_GOAL_MOVED
        };
        match self.progress {
            Some((_, _, g)) if far(g) => self.progress = Some((left, now, goal)),
            Some((best, _, _)) if left < best - ROUTE_PROGRESS => {
                self.progress = Some((left, now, goal))
            }
            Some((_, at, _)) if now.wrapping_sub(at) >= ROUTE_STUCK_MS => {
                nav.ban_step(pos, wp);
                self.stuck += 1;
                self.progress = None;
                self.points.clear();
                self.planned_at = None;
                return None;
            }
            Some(_) => {}
            None => self.progress = Some((left, now, goal)),
        }
        Some(wp)
    }

    /// BOT-113: the body made no way toward the first waypoint (the session's walk check): the step toward it is given
    /// up and the next call plans again.
    // DC-DEFAULT: BOT-113 (proposed)
    pub fn give_up_step(&mut self, nav: &mut NavGrid, pos: [f32; 3]) {
        if let Some(wp) = self.points.first().copied() {
            nav.ban_step(pos, wp);
            self.stuck += 1;
        }
        self.progress = None;
        self.points.clear();
        self.planned_at = None;
    }

    pub fn clear(&mut self) {
        self.points.clear();
        self.goal = None;
        self.planned_at = None;
        self.progress = None;
        self.greedy_until = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hunt::tests::{cupped, solid_box, walled};

    fn grow(nav: &mut NavGrid, max_probes: u64, toward: Option<[f32; 3]>) {
        while nav.growing() && nav.probes < max_probes {
            nav.expand(BUDGET, toward);
        }
    }

    /// A floor at y = 0 and a slope rising toward -z over z 200..0 at `deg` degrees (x -200..200).
    fn sloped(deg: f32) -> Arc<CollisionWorld> {
        let h = 200.0 * deg.to_radians().tan();
        let v = V3::new;
        let slope = dc_sim::CollModel::from_polys(&[dc_sim::ConvexPoly::from_corners(
            vec![
                v(-200.0, h, 0.0),
                v(-200.0, 0.0, 200.0),
                v(200.0, 0.0, 200.0),
                v(200.0, h, 0.0),
            ],
            dc_sim::SURF_SOLID,
        )]);
        let models = vec![
            solid_box([-5000.0, -64.0, -5000.0], [5000.0, 0.0, 5000.0]),
            slope,
        ];
        Arc::new(CollisionWorld::from_parts(models, &[]))
    }

    /// BOT-118: no node stands on a slope the mover slides down (steeper than 45 degrees); a walkable ramp keeps its
    /// nodes. Before, a node stood wherever the body fit, so routes led over steep slopes into crevices where the
    /// body was never on the ground (no walking, no stuck jump).
    #[test]
    fn no_node_stands_on_a_slope_the_mover_slides_down() {
        for (deg, standable) in [(60.0f32, false), (30.0, true)] {
            let h = 200.0 * deg.to_radians().tan();
            let mut nav = NavGrid::new(sloped(deg), [16.0, 40.0, 16.0]);
            // over the middle of the slope (z 100: the surface at h / 2)
            assert_eq!(
                nav.seed([0.0, h / 2.0 + 41.0, 100.0]),
                standable,
                "{deg} degrees"
            );
            assert!(nav.seed([0.0, 40.0, 800.0]), "the floor");
        }
    }

    /// A floor at y = 0 with a crate (x -100..100, z 1000..1200, 100 high) and a pit (x 500..700, z 500..700, its
    /// floor 100 below, sheer sides: no way out).
    fn crate_and_pit() -> Arc<CollisionWorld> {
        let models = vec![
            solid_box([-5000.0, -200.0, -5000.0], [5000.0, 0.0, 500.0]),
            solid_box([-5000.0, -200.0, 700.0], [5000.0, 0.0, 5000.0]),
            solid_box([-5000.0, -200.0, 500.0], [500.0, 0.0, 700.0]),
            solid_box([700.0, -200.0, 500.0], [5000.0, 0.0, 700.0]),
            solid_box([500.0, -264.0, 500.0], [700.0, -100.0, 700.0]),
            solid_box([-100.0, 0.0, 1000.0], [100.0, 100.0, 1200.0]),
        ];
        Arc::new(CollisionWorld::from_parts(models, &[]))
    }

    /// BOT-119: a bot on a crate top (it got there by a jump, a fall or a spawn) has a way down on the grid: before,
    /// the straight sweep of a drop met the crate's edge and a drop of more than 96 was no link, so the top was an
    /// island and the plan kept the bot there.
    #[test]
    fn a_crate_top_has_a_way_down() {
        let mut nav = NavGrid::new(crate_and_pit(), [16.0, 40.0, 16.0]);
        assert!(nav.seed([0.0, 40.0, 0.0]), "home");
        assert!(nav.seed([0.0, 140.0, 1100.0]), "the crate top");
        grow(&mut nav, 60_000, None);
        let p = nav
            .path([0.0, 140.0, 1100.0], [0.0, 40.0, 0.0])
            .expect("a way off the crate");
        let last = p[p.len() - 1];
        assert!(last[1] < 60.0 && flat(V3::new(last[0], 0.0, last[2]), V3::ZERO) <= STEP * 1.5, "{p:?}");
    }

    /// BOT-119: no plan leads into a pit the body cannot leave (a walk off its edge leads in, nothing out), not even
    /// for a goal inside it: the plan ends at its rim.
    #[test]
    fn no_plan_leads_into_a_pit() {
        let mut nav = NavGrid::new(crate_and_pit(), [16.0, 40.0, 16.0]);
        assert!(nav.seed([0.0, 40.0, 0.0]));
        grow(&mut nav, 60_000, None);
        assert!(
            nav.nodes.values().any(|n| n.pos.y < -50.0),
            "the grid knows the pit floor"
        );
        if let Some(p) = nav.path([0.0, 40.0, 0.0], [600.0, -60.0, 600.0]) {
            assert!(p.iter().all(|w| w[1] > -50.0), "{p:?}");
        }
        // a bot that fell in still gets a plan inside it
        let inside = nav.path([600.0, -60.0, 600.0], [560.0, -60.0, 650.0]);
        assert!(inside.is_some());
    }

    #[test]
    fn the_grid_grows_over_the_floor_and_stops_at_walls() {
        let mut nav = NavGrid::new(walled(), [16.0, 40.0, 16.0]);
        assert!(nav.seed([0.0, 40.0, 0.0]));
        assert!(!nav.seed([0.0, 40.0, 0.0]), "known");
        grow(&mut nav, 30_000, Some([0.0, 40.0, 900.0]));
        assert!(nav.len() > 200, "{} nodes", nav.len());
        // no node inside the wall slab (x -400..400, z 580..620): every body of the grid stands free of it
        assert!(nav
            .nodes
            .values()
            .all(|n| !nav.world.overlaps(&nav.shape, n.pos)
                && !(n.pos.x.abs() < 400.0 && (580.0..620.0).contains(&n.pos.z))));
        // a path from the start to the far side of the wall goes around it
        let p = nav
            .path([0.0, 40.0, 0.0], [0.0, 40.0, 900.0])
            .expect("a path");
        assert!(p.len() >= 2, "{p:?}");
        assert!(
            p.iter().any(|w| w[0].abs() > 400.0),
            "around the wall: {p:?}"
        );
        let last = p[p.len() - 1];
        assert!(
            flat(
                V3::new(last[0], last[1], last[2]),
                V3::new(0.0, 40.0, 900.0)
            ) <= STEP * 1.5
        );
    }

    #[test]
    fn a_route_out_of_the_cup_leads_away_from_the_goal_first() {
        let mut nav = NavGrid::new(cupped(), [16.0, 40.0, 16.0]);
        nav.seed([0.0, 40.0, 0.0]);
        grow(&mut nav, 40_000, Some([0.0, 40.0, 1500.0]));
        let mut r = Route::default();
        let wp = r
            .next(&mut nav, 0, [0.0, 40.0, 0.0], [0.0, 40.0, 1500.0])
            .expect("a way out");
        // the first waypoint is not through the back wall (z 300): it heads out of the cup (-z) or to a side
        assert!(wp[2] < 300.0, "{wp:?}");
        assert!(r.points.len() >= 2, "{:?}", r.points);
        // the same plan is kept until it is stale or the goal moves
        let again = r.next(&mut nav, 100, [0.0, 40.0, 0.0], [0.0, 40.0, 1500.0]);
        assert_eq!(again, Some(wp));
        assert_eq!(r.replans, 1);
        let _ = r.next(
            &mut nav,
            REPLAN_MS + 100,
            [0.0, 40.0, 0.0],
            [0.0, 40.0, 1500.0],
        );
        assert_eq!(r.replans, 2);
    }

    /// BOT-111: a kill volume across the whole floor (a strip of water at z 300..400)
    /// keeps every node out of it; without it the grid grows over the strip.
    #[test]
    fn no_node_stands_in_a_kill_volume() {
        let strip =
            ConvexVolume::from_box(V3::new(-6000.0, -50.0, 300.0), V3::new(6000.0, 10.0, 400.0));
        let inside = |n: &Node| n.pos.z > 300.0 - 16.0 && n.pos.z < 400.0 + 16.0;
        let mut nav = NavGrid::new(walled(), [16.0, 40.0, 16.0]).with_kill_volumes(vec![strip]);
        nav.seed([0.0, 40.0, 0.0]);
        grow(&mut nav, 30_000, Some([0.0, 40.0, 900.0]));
        assert!(nav.len() > 100, "{} nodes", nav.len());
        assert!(!nav.nodes.values().any(inside));
        // the strip cuts the floor: the way toward the far side ends before it (no straight leg over it)
        let p = nav
            .path([0.0, 40.0, 0.0], [0.0, 40.0, 900.0])
            .unwrap_or_default();
        assert!(p.iter().all(|w| w[2] < 300.0), "{p:?}");
        let mut nav = NavGrid::new(walled(), [16.0, 40.0, 16.0]);
        nav.seed([0.0, 40.0, 0.0]);
        grow(&mut nav, 30_000, Some([0.0, 40.0, 900.0]));
        assert!(nav.nodes.values().any(inside));
    }

    /// BOT-111: two floors with a gap between them (z 300..500) and a bridge at x 800..1000: the way from one floor
    /// to the other takes the bridge; no straight leg crosses the gap (the gap has no walls to stop the sweep).
    #[test]
    fn a_straight_leg_never_crosses_a_gap() {
        use crate::hunt::tests::solid_box;
        let w = Arc::new(CollisionWorld::from_parts(
            vec![
                solid_box([-2000.0, -64.0, -2000.0], [2000.0, 0.0, 300.0]),
                solid_box([-2000.0, -64.0, 500.0], [2000.0, 0.0, 2000.0]),
                solid_box([800.0, -64.0, 300.0], [1000.0, 0.0, 500.0]),
            ],
            &[],
        ));
        let mut nav = NavGrid::new(w, [16.0, 40.0, 16.0]);
        nav.seed([0.0, 40.0, 0.0]);
        grow(&mut nav, 60_000, Some([0.0, 40.0, 900.0]));
        let p = nav
            .path([0.0, 40.0, 0.0], [0.0, 40.0, 900.0])
            .expect("a path");
        let mut from = [0.0f32, 40.0, 0.0];
        for q in &p {
            // a leg that crosses z 300..500 does it on the bridge (the body's centre within a radius of it once it
            // is off the floors' edges)
            if (from[2] < 300.0) != (q[2] < 300.0) || (from[2] > 500.0) != (q[2] > 500.0) {
                let t = |z: f32| (z - from[2]) / (q[2] - from[2]);
                for z in [320.0, 400.0, 480.0] {
                    let k = t(z);
                    if (0.0..=1.0).contains(&k) {
                        let x = from[0] + (q[0] - from[0]) * k;
                        assert!(
                            (784.0..=1016.0).contains(&x),
                            "leg {from:?} -> {q:?} over the gap: {p:?}"
                        );
                    }
                }
            }
            from = *q;
        }
    }

    /// BOT-111: the greedy way-finder's danger test: on the floor is safe; above water (a kill volume under a
    /// non-solid surface), over the void and over a high drop is not.
    #[test]
    fn danger_is_a_kill_volume_below_the_void_or_a_high_drop() {
        use crate::hunt::tests::solid_box;
        let w = CollisionWorld::from_parts(
            vec![
                solid_box([-1000.0, -64.0, -1000.0], [0.0, 0.0, 1000.0]),
                solid_box([0.0, -1064.0, -1000.0], [1000.0, -1000.0, 0.0]),
                solid_box([0.0, -864.0, 0.0], [1000.0, -800.0, 1000.0]),
            ],
            &[],
        );
        let water = vec![ConvexVolume::from_box(
            V3::new(0.0, -800.0, 0.0),
            V3::new(1000.0, -500.0, 1000.0),
        )];
        let d = [16.0, 40.0, 16.0];
        assert!(!in_danger(&w, &water, d, [-500.0, 40.0, 0.0]), "the floor");
        assert!(
            in_danger(&w, &water, d, [500.0, 40.0, 500.0]),
            "over the water"
        );
        assert!(
            in_danger(&w, &water, d, [500.0, 40.0, -500.0]),
            "a 1000-unit drop"
        );
        assert!(in_danger(&w, &water, d, [2000.0, 40.0, 0.0]), "the void");
    }

    /// BOT-112: stairs of 16-unit risers on 16-unit treads (48 up per lattice step, more than STEP_UP) up to a
    /// platform 96 high: the grid climbs them and comes back down (the lattice step alone found no place on them).
    #[test]
    fn a_stairway_steeper_than_the_lattice_step_is_climbed() {
        use crate::hunt::tests::solid_box;
        let mut models = vec![solid_box([-2000.0, -64.0, -2000.0], [2000.0, 0.0, 2000.0])];
        for k in 0..6 {
            let z = 200.0 + 16.0 * k as f32;
            models.push(solid_box(
                [-200.0, 0.0, z],
                [200.0, 16.0 * (k + 1) as f32, 700.0],
            ));
        }
        let w = Arc::new(CollisionWorld::from_parts(models, &[]));
        let top = [0.0f32, 136.0, 500.0];
        let mut nav = NavGrid::new(w, [16.0, 40.0, 16.0]);
        nav.seed([0.0, 40.0, 0.0]);
        grow(&mut nav, 40_000, Some(top));
        let up = nav.path([0.0, 40.0, 0.0], top).expect("a way up");
        let end = up[up.len() - 1];
        assert!(
            end[1] > 120.0
                && flat(
                    V3::new(end[0], end[1], end[2]),
                    V3::new(top[0], top[1], top[2])
                ) <= STEP * 1.5,
            "{up:?}"
        );
        let down = nav.path(end, [0.0, 40.0, 0.0]).expect("a way down");
        assert!(down[down.len() - 1][1] < 50.0, "{down:?}");
        // a ledge as high as the stairs (a box with no steps) is no way up
        let ledge = Arc::new(CollisionWorld::from_parts(
            vec![
                solid_box([-2000.0, -64.0, -2000.0], [2000.0, 0.0, 2000.0]),
                solid_box([-200.0, 0.0, 200.0], [200.0, 96.0, 700.0]),
            ],
            &[],
        ));
        let mut nav = NavGrid::new(ledge, [16.0, 40.0, 16.0]);
        nav.seed([0.0, 40.0, 0.0]);
        grow(&mut nav, 40_000, Some(top));
        assert!(
            nav.nodes.values().all(|n| n.pos.y < 60.0),
            "no node on the ledge"
        );
    }

    /// BOT-112: a node dropped from a ledge is no way back up: the link from the top to the floor is one-way.
    #[test]
    fn a_drop_is_walked_down_only() {
        use crate::hunt::tests::solid_box;
        let w = Arc::new(CollisionWorld::from_parts(
            vec![
                solid_box([-2000.0, -64.0, -2000.0], [2000.0, 0.0, 2000.0]),
                solid_box([-2000.0, 0.0, -2000.0], [2000.0, 80.0, 300.0]),
            ],
            &[],
        ));
        let mut nav = NavGrid::new(w, [16.0, 40.0, 16.0]);
        nav.seed([0.0, 120.0, 0.0]);
        grow(&mut nav, 40_000, Some([0.0, 40.0, 900.0]));
        assert!(
            nav.path([0.0, 120.0, 0.0], [0.0, 40.0, 900.0]).is_some(),
            "down the ledge"
        );
        let back = nav.path([0.0, 40.0, 900.0], [0.0, 120.0, 0.0]);
        assert!(
            back.as_ref().is_none_or(|p| p.iter().all(|w| w[1] < 60.0)),
            "no way back up: {back:?}"
        );
    }

    /// BOT-112 / BOT-113: a goal just behind a wall the grid has not grown around yet: the plan ends on the near
    /// side, and once the bot stands there the greedy way-finder takes over for a while instead of the bot standing
    /// at the wall; grown around, the plan's end sees the goal.
    #[test]
    fn a_plan_that_ends_behind_a_wall_from_the_goal_hands_over_to_the_greedy_finder() {
        let goal = [0.0f32, 40.0, 700.0];
        let mut nav = NavGrid::new(walled(), [16.0, 40.0, 16.0]);
        nav.seed([0.0, 40.0, 0.0]);
        grow(&mut nav, 3_000, None);
        let p = nav.path([0.0, 40.0, 0.0], goal).expect("toward the wall");
        let end = p[p.len() - 1];
        assert!(end[2] < 580.0, "{p:?}");
        let mut r = Route::default();
        assert_eq!(
            r.next(&mut nav, 0, end, goal),
            None,
            "at the plan's end: greedy"
        );
        assert_eq!(
            r.next(&mut nav, EDGE_GREEDY_MS / 2, end, goal),
            None,
            "still greedy"
        );
        // a step away from the end (the greedy finder walks): still no plan until the time is up
        let aside = [end[0] + 40.0, end[1], end[2] - 40.0];
        assert_eq!(
            r.next(&mut nav, EDGE_GREEDY_MS / 2 + 100, aside, goal),
            None,
            "the hold"
        );
        grow(&mut nav, 60_000, Some(goal));
        let p = nav.path([0.0, 40.0, 0.0], goal).expect("around the wall");
        let end = p[p.len() - 1];
        assert!(end[2] > 620.0, "{p:?}");
    }

    /// BOT-113: a bot that makes no way along its plan gives up the step toward its first waypoint and plans again.
    #[test]
    fn a_route_without_progress_gives_up_its_step() {
        let mut nav = NavGrid::new(walled(), [16.0, 40.0, 16.0]);
        nav.seed([0.0, 40.0, 0.0]);
        grow(&mut nav, 30_000, Some([0.0, 40.0, 900.0]));
        let mut r = Route::default();
        let at = [0.0f32, 40.0, 0.0];
        let goal = [0.0f32, 40.0, 900.0];
        let mut t = 0;
        while t <= ROUTE_STUCK_MS + 2 * REPLAN_MS {
            r.next(&mut nav, t, at, goal);
            t += 100;
        }
        assert!(r.stuck >= 1, "the body stood still: {} given up", r.stuck);
        let links = nav.nodes[&nav.nearest(V3::new(0.0, 40.0, 0.0), 30.0).unwrap()]
            .links
            .len();
        assert!(links < 8, "{links} links left");
    }

    /// BOT-117: a roaming goal is a known node at least the asked distance away, the same one for the same pick.
    #[test]
    fn a_far_node_is_far_and_follows_the_pick() {
        let mut nav = NavGrid::new(walled(), [16.0, 40.0, 16.0]);
        nav.seed([0.0, 40.0, 0.0]);
        grow(&mut nav, 20_000, None);
        let at = [0.0f32, 40.0, 0.0];
        let a = nav.far_node(at, 800.0, 7).expect("a far node");
        assert!(
            flat(V3::new(a[0], a[1], a[2]), V3::new(0.0, 40.0, 0.0)) >= 800.0,
            "{a:?}"
        );
        assert_eq!(nav.far_node(at, 800.0, 7), Some(a));
        assert!((0..20)
            .filter_map(|k| nav.far_node(at, 800.0, k))
            .any(|b| b != a));
        assert_eq!(nav.far_node(at, 1.0e6, 7), None);
    }

    #[test]
    fn off_the_grid_there_is_no_route() {
        let mut nav = NavGrid::new(walled(), [16.0, 40.0, 16.0]);
        nav.seed([0.0, 40.0, 0.0]);
        nav.expand(BUDGET, None);
        let mut r = Route::default();
        assert_eq!(
            r.next(&mut nav, 0, [3000.0, 40.0, 3000.0], [0.0, 40.0, 0.0]),
            None
        );
    }
}
