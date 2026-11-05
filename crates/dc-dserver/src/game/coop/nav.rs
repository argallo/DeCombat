//! Co-op AI navigation (FT-S11): the AI plan their way to their target over the navigation grid of `dc-nav` (the
//! bots' grid, BOT-93 / BOT-118 / BOT-119: a lattice probed with the body's own cylinder, links only where the body
//! walks, steps or drops) and move with that body over the level model (the server's shot-trace world; the blocker
//! polygons are player clips and do not stop the AI), so an AI never stands inside geometry. The grid is grown on demand, between an AI that has no whole way
//! yet and its target, a fixed number of probes per AI step; each AI step opens a fixed number of plans. An AI whose
//! position does not change while it wants to move jumps (a higher step for a moment), then plans again without the
//! link it was stuck on. A world the AI does not stand on (no floor below them, the synthetic test worlds) keeps the
//! steering of FT-S5.
// DC-DEFAULT: FT-S11 (proposed; replaces FT-S5's "no graph search" and refines FT-6 for slice 1)

use std::sync::Arc;

use dc_nav::NavGrid;
use dc_net_engine::time::{later, since};
use dc_net_engine::Millis;
use dc_sim::{CollisionWorld, ConvexVolume, V3};

use super::{rotate_y, AiChar, AiState, Game};
use crate::killvol::VolumeShape;

/// Grid probes per AI step (50 ms) while an AI has no whole way to its target.
// DC-DEFAULT: FT-S11 (proposed) 300 probes (about 1 ms on the ROM mission world, release build)
pub const GROW_PROBES: usize = 120;
/// Plans (A* searches) per AI step for all AI of the room together; the others keep their way one step more.
// DC-DEFAULT: FT-S11 (proposed) 2 per 50 ms step (FT-6 proposed 4 per room tick)
pub const PLANS_PER_STEP: u32 = 2;
/// A way is planned again this often while it is followed, and when the target moved this far from where it was.
// DC-DEFAULT: FT-S11 (proposed) 2000 ms, 128 units (FT-6)
pub const REPLAN_MS: u32 = 2000;
pub const TARGET_MOVED: f32 = 128.0;
/// After a plan that found no way, the next one waits this long (the grid grows meanwhile); an AI on an island
/// looks again after `ISLAND_RETRY_MS` (a door may open).
// DC-DEFAULT: FT-S11 (proposed) 500 ms, 2000 ms
pub const RETRY_MS: u32 = 500;
pub const ISLAND_RETRY_MS: u32 = 2000;
/// How many of the next lattice nodes a shortcut may skip, and when a node counts as reached (flat distance).
// DC-DEFAULT: FT-S11 (proposed) 6 nodes, 24 units
pub const AHEAD: usize = 6;
pub const REACHED: f32 = 24.0;
/// The end of a way counts as the target's place within this flat distance.
// DC-DEFAULT: FT-S11 (proposed) 72 units (one and a half lattice steps)
pub const AT_TARGET: f32 = 72.0;
/// Stuck: the AI wants to move and its way left gets no `STUCK_MOVE` units shorter: after `JUMP_MS` it jumps (for
/// `JUMP_FOR_MS` its step may rise `JUMP_RISE` more), after `STUCK_REPLAN_MS` it plans again without the link it was
/// on. Every direction blocked for `BLOCKED_STRIDES` strides in a row on a way: the link is given up at once.
// DC-DEFAULT: FT-S11 (proposed) 16 units, 1500 ms, 600 ms, 40 units, 3000 ms
pub const STUCK_MOVE: f32 = 16.0;
pub const JUMP_MS: u32 = 1500;
pub const JUMP_FOR_MS: u32 = 600;
pub const JUMP_RISE: f32 = 40.0;
pub const STUCK_REPLAN_MS: u32 = 3000;
// DC-DEFAULT: FT-S11 (proposed) 3 strides
pub const BLOCKED_STRIDES: u32 = 3;
/// How long an AI that passes a face from its back is not taken out of the geometry.
// DC-DEFAULT: FT-S11 (proposed) 500 ms
pub const TRANSIT_MS: u32 = 500;
/// How far an AI inside geometry looks for a standing place.
// DC-DEFAULT: FT-S11 (proposed) 96 units
pub const FREE_REACH: f32 = 96.0;

/// Counters of the AI navigation of a room (tests, logs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NavStats {
    /// Plans made, those that reached the target's place, those that found no way, those that found the AI on an
    /// island.
    pub plans: u64,
    pub whole: u64,
    pub failed: u64,
    pub islands: u64,
    /// Stuck jumps, re-plans after being stuck, links given up when blocked, AI moved out of geometry.
    pub jumps: u64,
    pub stuck_replans: u64,
    pub blocked_bans: u64,
    pub freed: u64,
    /// Strides along a link through a face passed from its back.
    pub transits: u64,
    /// Grid size and probes so far.
    pub nodes: usize,
    pub probes: u64,
    /// Wall time spent in the navigation: in all, and the most in one AI step (microseconds).
    pub busy_us: u64,
    pub step_max_us: u64,
    pub steps: u64,
    /// The most one plan, one growth and one stride took (microseconds).
    pub plan_max_us: u64,
    pub grow_max_us: u64,
    pub walk_max_us: u64,
}

/// Where an AI goes next.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NavGoal {
    /// The next point of its way (or the target itself at the end of a whole way).
    Way([f32; 3]),
    /// No way leads from where it stands to the target on this world (a pen, a vantage island): it holds its place.
    Hold,
    /// The grid gives no way yet: the caller steers by sight and the AI nodes.
    Steer,
}

/// What an AI's stride on the navigation world came to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NavMove {
    /// No navigation world, or the AI does not stand on it: the caller steers as before.
    Off,
    /// The AI's new standing place and the direction it walked.
    Moved([f32; 3], [f32; 3]),
    /// Every direction is blocked this step.
    Blocked,
}

/// The way of one AI slot.
#[derive(Debug, Clone, Default)]
struct AiRoute {
    /// The spawn time of the AI the way belongs to (a reused slot starts afresh).
    life: Option<Millis>,
    pts: Vec<[f32; 3]>,
    /// The target the way was planned to and whether it reaches the target's place.
    goal: Option<[f32; 3]>,
    whole: bool,
    /// No way leads to the target from the AI's part of the grid (all of it grown).
    island: bool,
    planned_at: Millis,
    retry_at: Option<Millis>,
    replan: bool,
    /// The target asked for last (the grid grows toward it).
    want: Option<[f32; 3]>,
    /// Stuck check: the least way left so far and when it last got `STUCK_MOVE` shorter, the last stride asked for,
    /// the jump, the strides in a row every direction was blocked.
    best_left: f32,
    best_at: Millis,
    last_walk: Option<Millis>,
    jumped: bool,
    jump_until: Option<Millis>,
    blocked: u32,
    /// Passing a face from its back on a link the sweep allows (a one-way clip, an AI room's exit) until then.
    transit_until: Option<Millis>,
}

impl AiRoute {
    fn fresh(life: Millis, now: Millis) -> AiRoute {
        AiRoute {
            life: Some(life),
            best_left: f32::INFINITY,
            best_at: now,
            ..AiRoute::default()
        }
    }

    /// The way left from `pos`: to the first point, then along the points (the target when there are none).
    fn left(&self, pos: [f32; 3]) -> f32 {
        match self.pts.first() {
            Some(w) => flat(pos, *w) + self.pts.windows(2).map(|p| flat(p[0], p[1])).sum::<f32>(),
            None => self.want.map_or(0.0, |t| flat(pos, t)),
        }
    }
}

/// The room's grid and the AI's ways.
struct CoopNav {
    grid: NavGrid,
    routes: Vec<AiRoute>,
    plans_left: u32,
    turn: usize,
    stats: NavStats,
    step_us: u64,
}

/// The navigation world of a room and, once the first AI walks, its grid.
pub struct NavSlot {
    trace: Option<Arc<CollisionWorld>>,
    kill: Vec<ConvexVolume>,
    nav: Option<CoopNav>,
    /// The grid could not be made (no world): no more tries.
    none: bool,
}

/// A kill volume's closed model as the convex region inside its face planes (oriented by the mean corner).
// DC-DEFAULT: FT-S11 (proposed) as dc-sim `brush_volume` builds it for the bots (BOT-111)
fn kill_region(v: &VolumeShape) -> Option<ConvexVolume> {
    let corners: Vec<[f32; 3]> = v
        .faces
        .iter()
        .flat_map(|f| f.corners.iter().copied())
        .collect();
    if corners.is_empty() {
        return None;
    }
    let n = corners.len() as f32;
    let mid = V3::new(
        corners.iter().map(|c| c[0]).sum::<f32>() / n,
        corners.iter().map(|c| c[1]).sum::<f32>() / n,
        corners.iter().map(|c| c[2]).sum::<f32>() / n,
    );
    let planes = v
        .faces
        .iter()
        .map(|f| {
            let (nrm, d) = (V3::from_array(f.normal), f.dist);
            if nrm.dot(mid) - d > 0.0 {
                (-nrm, -d)
            } else {
                (nrm, d)
            }
        })
        .collect();
    Some(ConvexVolume::new(
        planes,
        V3::from_array(v.lo),
        V3::from_array(v.hi),
    ))
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

impl NavSlot {
    pub fn new(world: &crate::world::WorldInfo) -> NavSlot {
        NavSlot {
            trace: world.trace.clone(),
            kill: world.kill_shapes.iter().filter_map(kill_region).collect(),
            nav: None,
            none: false,
        }
    }

    /// The geometry the AI walk in: the level model. The blocker polygons are left out: they are player clips (an AI
    /// room of the wave map is closed by them alone, and its AI come out through them).
    // OBSERVED: ROM wave map (its AI rooms' exits; with the clips as geometry the AI planned no way out, without them
    // all twelve AI of the first waves reached the humans, tests/coop_nav_rom.rs)
    // DC-DEFAULT: FT-S11 (proposed)
    fn world(&self) -> Option<Arc<CollisionWorld>> {
        self.trace.clone()
    }
}

impl Game {
    /// The counters of the AI navigation (`None` before the first AI walked or without a world).
    pub fn coop_nav_stats(&self) -> Option<NavStats> {
        self.coop_nav.nav.as_ref().map(|n| {
            let mut s = n.stats.clone();
            s.nodes = n.grid.len();
            s.probes = n.grid.probes;
            s
        })
    }

    /// Replaces the geometry the AI walk in (tests), without kill volumes.
    pub fn set_nav_world(&mut self, w: Option<CollisionWorld>) {
        self.coop_nav.trace = w.map(Arc::new);
        self.coop_nav.kill.clear();
        self.coop_nav.nav = None;
        self.coop_nav.none = false;
    }

    /// Once per AI step before the AI move: makes the grid on the first step (for the body of the first living AI),
    /// seeds it where the living AI stand, grows it between one AI without a whole way and its target (in turn), and
    /// opens the step's plans.
    pub(super) fn ai_nav_begin(&mut self, now: Millis) {
        let t0 = std::time::Instant::now();
        let fallback = self.cfg.player_dims;
        let living: Vec<(usize, [f32; 3], [f32; 3])> = self.coop.as_ref().map_or(Vec::new(), |c| {
            c.ais
                .iter()
                .filter(|a| a.state == AiState::Alive)
                .map(|a| (a.slot, a.pos, a.dims(fallback)))
                .collect()
        });
        if !self.cfg.coop.nav {
            return;
        }
        if self.coop_nav.nav.is_none() && !self.coop_nav.none {
            let Some(&(_, _, dims)) = living.first() else {
                return;
            };
            match self.coop_nav.world() {
                Some(w) => {
                    let grid = NavGrid::new(w, dims)
                        .with_kill_volumes(self.coop_nav.kill.clone())
                        .without_pits();
                    self.coop_nav.nav = Some(CoopNav {
                        grid,
                        routes: Vec::new(),
                        plans_left: 0,
                        turn: 0,
                        stats: NavStats::default(),
                        step_us: 0,
                    });
                }
                None => self.coop_nav.none = true,
            }
        }
        let Some(nav) = self.coop_nav.nav.as_mut() else {
            return;
        };
        nav.plans_left = PLANS_PER_STEP;
        nav.stats.steps += 1;
        // never inside geometry: a living AI that does not fit where it stands (placed or spawned in a wall, frozen
        // there) is set at the nearest standing place
        let mut freed = Vec::new();
        for &(slot, p, _) in &living {
            let transit = nav
                .routes
                .get(slot)
                .is_some_and(|r| r.transit_until.is_some_and(|t| later(t, now)));
            // (an AI held for want of room looks again only when its wait is over)
            let held = nav.routes.get(slot).is_some_and(|r| {
                r.island && r.pts.is_empty() && r.retry_at.is_some_and(|t| later(t, now))
            });
            if !transit && !held && !nav.grid.fits(p) {
                if let Some(q) = nav.grid.free_place(p, FREE_REACH) {
                    nav.stats.freed += 1;
                    freed.push((slot, q));
                }
            }
        }
        for &(slot, p, _) in &living {
            let p = freed.iter().find(|f| f.0 == slot).map_or(p, |f| f.1);
            nav.grid.seed_grown(p);
        }
        // the AI whose way does not reach its target yet, in turn
        let short: Vec<([f32; 3], [f32; 3])> = living
            .iter()
            .filter_map(|&(slot, p, _)| {
                let r = nav.routes.get(slot)?;
                (!r.whole).then_some(())?;
                Some((p, r.want?))
            })
            .collect();
        if !short.is_empty() && nav.grid.growing() {
            let (p, t) = short[nav.turn % short.len()];
            nav.turn = nav.turn.wrapping_add(1);
            let tg = std::time::Instant::now();
            nav.grid.expand_between(GROW_PROBES, p, t);
            let us = tg.elapsed().as_micros() as u64;
            nav.stats.grow_max_us = nav.stats.grow_max_us.max(us);
        }
        nav.step_us = t0.elapsed().as_micros() as u64;
        if let Some(c) = self.coop.as_mut() {
            for (slot, q) in freed {
                if let Some(a) = c.ais.get_mut(slot) {
                    a.pos = q;
                }
            }
        }
    }

    /// Once per AI step after the AI moved: the step's time into the counters.
    pub(super) fn ai_nav_end(&mut self) {
        if let Some(nav) = self.coop_nav.nav.as_mut() {
            nav.stats.busy_us += nav.step_us;
            nav.stats.step_max_us = nav.stats.step_max_us.max(nav.step_us);
            nav.step_us = 0;
        }
    }

    /// Whether AI slot `slot` holds its place because no way leads from it to its target (tests, logs).
    pub fn coop_nav_island(&self, slot: usize) -> bool {
        self.coop_nav
            .nav
            .as_ref()
            .and_then(|n| n.routes.get(slot))
            .is_some_and(|r| r.island && r.pts.is_empty())
    }

    /// Where AI `a` goes next on its way to `target` (planning when due and a plan of the step is left).
    pub(super) fn ai_nav_waypoint(&mut self, a: &AiChar, target: [f32; 3], now: Millis) -> NavGoal {
        let t0 = std::time::Instant::now();
        let Some(nav) = self.coop_nav.nav.as_mut() else {
            return NavGoal::Steer;
        };
        if nav.routes.len() <= a.slot {
            nav.routes.resize_with(a.slot + 1, AiRoute::default);
        }
        let r = &mut nav.routes[a.slot];
        if r.life != Some(a.spawned_at) {
            *r = AiRoute::fresh(a.spawned_at, now);
        }
        r.want = Some(target);
        let pos = a.pos;
        // the nodes reached are dropped, then the way ahead is pulled straight from here
        let before = r.pts.len();
        while r
            .pts
            .first()
            .is_some_and(|w| flat(*w, pos) <= REACHED && (w[1] - pos[1]).abs() <= 90.0)
        {
            r.pts.remove(0);
        }
        if r.pts.len() < before && r.pts.len() > 1 {
            let k = nav.grid.shortcut(pos, &r.pts, AHEAD);
            r.pts.drain(..k);
        }
        let moved = r.goal.is_none_or(|g| flat(g, target) > TARGET_MOVED);
        let due = (r.pts.is_empty() && !r.whole)
            || moved
            || r.replan
            || since(now, r.planned_at) >= REPLAN_MS;
        let waiting = r.retry_at.is_some_and(|t| later(t, now));
        if due && !waiting && nav.plans_left > 0 {
            nav.plans_left -= 1;
            nav.stats.plans += 1;
            r.planned_at = now;
            r.replan = false;
            r.goal = Some(target);
            let tp = std::time::Instant::now();
            let way = nav.grid.route(pos, target);
            let us = tp.elapsed().as_micros() as u64;
            nav.stats.plan_max_us = nav.stats.plan_max_us.max(us);
            r.island = nav.grid.last_way_closed();
            if r.island {
                nav.stats.islands += 1;
            }
            match way {
                Some(mut pts) => {
                    let end = pts[pts.len() - 1];
                    r.whole = flat(end, target) <= AT_TARGET;
                    if r.whole {
                        nav.stats.whole += 1;
                    }
                    let k = nav.grid.shortcut(pos, &pts, AHEAD);
                    pts.drain(..k);
                    pts.retain(|w| flat(*w, pos) > REACHED || (w[1] - pos[1]).abs() > 90.0);
                    r.pts = pts;
                    // a way that ends where the AI stands short of the target: the next plan waits
                    r.retry_at = (r.island || (r.pts.is_empty() && !r.whole)).then(|| {
                        now.wrapping_add(if r.island { ISLAND_RETRY_MS } else { RETRY_MS })
                    });
                    // a longer way than the one followed (a detour) starts the stuck check again
                    let left = r.left(pos);
                    if left > r.best_left + 48.0 {
                        r.best_left = left;
                        r.best_at = now;
                    }
                }
                None => {
                    nav.stats.failed += 1;
                    r.pts.clear();
                    r.whole = false;
                    let wait = if r.island { ISLAND_RETRY_MS } else { RETRY_MS };
                    r.retry_at = Some(now.wrapping_add(wait));
                }
            }
        }
        let out = match r.pts.first() {
            Some(w) => NavGoal::Way(*w),
            // at the end of a whole way: the target itself
            None if r.whole => NavGoal::Way(target),
            None if r.island => NavGoal::Hold,
            None => NavGoal::Steer,
        };
        nav.step_us += t0.elapsed().as_micros() as u64;
        out
    }

    /// One stride of AI `a` toward `goal` with the body over the navigation world: straight, else turned by 45 or 90
    /// degrees; an AI inside geometry is first moved to the nearest standing place. Keeps the stuck check (a jump,
    /// then a new plan without the link the AI was on).
    pub(super) fn ai_nav_walk(
        &mut self,
        a: &AiChar,
        goal: [f32; 3],
        stride: f32,
        now: Millis,
    ) -> NavMove {
        let t0 = std::time::Instant::now();
        let out = self.ai_nav_walk_inner(a, goal, stride, now);
        if let Some(nav) = self.coop_nav.nav.as_mut() {
            let us = t0.elapsed().as_micros() as u64;
            nav.step_us += us;
            nav.stats.walk_max_us = nav.stats.walk_max_us.max(us);
        }
        out
    }

    fn ai_nav_walk_inner(
        &mut self,
        a: &AiChar,
        goal: [f32; 3],
        stride: f32,
        now: Millis,
    ) -> NavMove {
        let Some(nav) = self.coop_nav.nav.as_mut() else {
            return NavMove::Off;
        };
        if nav.routes.len() <= a.slot {
            nav.routes.resize_with(a.slot + 1, AiRoute::default);
        }
        let pos = a.pos;
        let Some(dir) = super::norm2([goal[0] - pos[0], 0.0, goal[2] - pos[2]]) else {
            return NavMove::Blocked;
        };
        // never inside geometry: an AI that does not fit where it is stands at the nearest free place (not while it
        // passes a face from its back)
        let transit = nav.routes[a.slot]
            .transit_until
            .is_some_and(|t| later(t, now));
        if !transit && !nav.grid.fits(pos) {
            return match nav.grid.free_place(pos, FREE_REACH) {
                Some(p) => {
                    nav.stats.freed += 1;
                    NavMove::Moved(p, dir)
                }
                // no room to stand near it (an AI the file keeps in a box smaller than its body): it holds its place
                None => {
                    let r = &mut nav.routes[a.slot];
                    r.pts.clear();
                    r.whole = false;
                    r.island = true;
                    r.retry_at = Some(now.wrapping_add(ISLAND_RETRY_MS));
                    NavMove::Blocked
                }
            };
        }
        let r = &mut nav.routes[a.slot];
        if r.life != Some(a.spawned_at) {
            *r = AiRoute::fresh(a.spawned_at, now);
        }
        // the stuck check runs while the AI keeps wanting to move: its way left must get shorter
        let fresh = r.last_walk.is_none_or(|t| since(now, t) > 250);
        r.last_walk = Some(now);
        let left = r.left(pos);
        if fresh || left < r.best_left - STUCK_MOVE {
            r.best_left = left;
            r.best_at = now;
            r.jumped = false;
        }
        let stuck = since(now, r.best_at);
        if stuck >= JUMP_MS && !r.jumped {
            r.jumped = true;
            r.jump_until = Some(now.wrapping_add(JUMP_FOR_MS));
            nav.stats.jumps += 1;
        }
        if stuck >= STUCK_REPLAN_MS {
            if let Some(w) = r.pts.first().copied() {
                nav.grid.ban_step(pos, w);
            }
            r.pts.clear();
            r.whole = false;
            r.replan = true;
            r.retry_at = None;
            r.best_left = f32::INFINITY;
            r.best_at = now;
            r.jumped = false;
            nav.stats.stuck_replans += 1;
        }
        let rise = if r.jump_until.is_some_and(|t| later(t, now)) {
            dc_nav::STEP_UP + JUMP_RISE
        } else {
            dc_nav::STEP_UP
        };
        let reach = stride.max(1.0);
        for turn in [0.0f32, 0.785, -0.785, 1.571, -1.571] {
            let d = rotate_y(dir, turn);
            let toward = [pos[0] + d[0] * reach, pos[1], pos[2] + d[2] * reach];
            if let Some(p) = nav.grid.walk(pos, toward, stride, rise) {
                nav.routes[a.slot].blocked = 0;
                return NavMove::Moved(p, d);
            }
        }
        // a link the sweep allows although the body does not fit on the way (a face passed from its back: the mover
        // goes through it, as a player does): the AI goes along it in strides
        if let Some(w) = nav.routes[a.slot].pts.first().copied() {
            let d = flat(pos, w);
            if d > 1.0 && nav.grid.clear_way(pos, w) {
                let t = (stride / d).min(1.0);
                let p = [0, 1, 2].map(|k| pos[k] + (w[k] - pos[k]) * t);
                let r = &mut nav.routes[a.slot];
                r.blocked = 0;
                r.transit_until = Some(now.wrapping_add(TRANSIT_MS));
                nav.stats.transits += 1;
                return NavMove::Moved(p, dir);
            }
        }
        // no floor below the AI at all: the world is not one it walks on
        if !transit && nav.grid.free_place(pos, 0.0).is_none() {
            return NavMove::Off;
        }
        // blocked on a way: the link the grid has and the body cannot walk is given up, a new plan follows
        let r = &mut nav.routes[a.slot];
        r.blocked += 1;
        if r.blocked >= BLOCKED_STRIDES {
            if let Some(w) = r.pts.first().copied() {
                nav.grid.ban_step(pos, w);
                r.pts.clear();
                r.whole = false;
                r.replan = true;
                r.retry_at = None;
                nav.stats.blocked_bans += 1;
            }
            r.blocked = 0;
        }
        NavMove::Blocked
    }
}
