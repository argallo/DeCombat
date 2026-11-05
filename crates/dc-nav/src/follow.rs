//! Following a way over the grid (FT-S11's rules, shared by the bots, BOT-121): the grid grows between the follower
//! and its target while no whole way is known, a plan is the A* route of [`NavGrid::route`] (a partial way to the
//! reachable node nearest to an unreachable target), the lattice nodes ahead are pulled straight as they are
//! reached, and the way is planned again when it is old, when the target moved and when the follower is stuck: a
//! follower that wants to move and whose way left gets no shorter jumps, then plans again without the link it was
//! on. A follower on an island of the grid (no way leads to the target from its part of the grid, all of it grown)
//! is told so. A DeCombat behaviour, no 2009 rule.
// DC-DEFAULT: FT-S11 (proposed); BOT-121 (proposed)

use crate::NavGrid;

/// Grid probes per navigation step while a follower has no whole way to its target.
// DC-DEFAULT: FT-S11 (proposed) 120 probes per 50 ms step
pub const GROW_PROBES: usize = 120;
/// A navigation step: growth and plans at most this often per follower.
// DC-DEFAULT: FT-S11 (proposed) 50 ms (the co-op AI's step)
pub const STEP_MS: u32 = 50;
/// A way is planned again this often while it is followed, and when the target moved this far from where it was.
// DC-DEFAULT: FT-S11 (proposed) 2000 ms, 128 units
pub const REPLAN_MS: u32 = 2000;
pub const TARGET_MOVED: f32 = 128.0;
/// After a plan that found no way, the next one waits this long (the grid grows meanwhile); a follower on an island
/// looks again after `ISLAND_RETRY_MS` (a door may open).
// DC-DEFAULT: FT-S11 (proposed) 500 ms, 2000 ms
pub const RETRY_MS: u32 = 500;
pub const ISLAND_RETRY_MS: u32 = 2000;
/// How many of the next lattice nodes a shortcut may skip, and when a node counts as reached (flat distance, and at
/// most `REACHED_RISE` above or below).
// DC-DEFAULT: FT-S11 (proposed) 6 nodes, 24 units, 90 units
pub const AHEAD: usize = 6;
pub const REACHED: f32 = 24.0;
pub const REACHED_RISE: f32 = 90.0;
/// The end of a way counts as the target's place within this flat distance, at most `REACHED_RISE` above or below
/// it and in sight of it (a place right under a target on a floor above, or behind the wall from it, is not its
/// place).
// DC-DEFAULT: FT-S11 (proposed) 72 units (one and a half lattice steps); BOT-121 (proposed) the height band, the sight
pub const AT_TARGET: f32 = 72.0;
/// Stuck: the follower wants to move and its way left gets no `STUCK_MOVE` units shorter: after `JUMP_MS` it jumps,
/// after `STUCK_REPLAN_MS` it plans again without the link it was on. A walk asked for after more than `WALK_GAP_MS`
/// without one starts the count again (it stood, fought, waited at a door). A new plan longer than the least way
/// left by `DETOUR` (a way round) starts it again too.
// DC-DEFAULT: FT-S11 (proposed) 16 units, 1500 ms, 3000 ms, 250 ms, 48 units
pub const STUCK_MOVE: f32 = 16.0;
pub const JUMP_MS: u32 = 1500;
pub const STUCK_REPLAN_MS: u32 = 3000;
pub const WALK_GAP_MS: u32 = 250;
pub const DETOUR: f32 = 48.0;
/// BOT-121: the way left counts as shorter only when the follower moved this far (flat) since the count began (a
/// body that creeps along a wall while new plans keep the way left getting shorter is stuck too).
// DC-DEFAULT: BOT-121 (proposed) 16 units (`STUCK_MOVE`)
pub const MOVED: f32 = STUCK_MOVE;

/// Where a follower goes next.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NavGoal {
    /// The next point of its way (or the target itself at the end of a whole way).
    Way([f32; 3]),
    /// No way leads from where it stands to the target on this world (a pen, a vantage island).
    Hold,
    /// The grid gives no way yet (the follower stands off the grid, or no plan was made yet).
    Steer,
}

/// What the stuck check of a walk came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stuck {
    /// The way left got shorter in time (or the count started again).
    No,
    /// No way made for [`JUMP_MS`]: the follower jumps (once per count).
    Jump,
    /// No way made for [`STUCK_REPLAN_MS`]: the link toward the first point was given up and the next call plans.
    Replan,
}

/// Counters of a follower (tests, logs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FollowStats {
    /// Plans made, those that reached the target's place, those that found no way, those that found the follower on
    /// an island.
    pub plans: u64,
    pub whole: u64,
    pub failed: u64,
    pub islands: u64,
    /// Stuck jumps and re-plans after being stuck.
    pub jumps: u64,
    pub stuck_replans: u64,
    /// The most one plan and one growth took (microseconds).
    pub plan_max_us: u64,
    pub grow_max_us: u64,
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

fn later(t: u32, now: u32) -> bool {
    (t.wrapping_sub(now) as i32) > 0
}

/// The way of one follower and its stuck count.
#[derive(Debug, Clone)]
pub struct Follower {
    pts: Vec<[f32; 3]>,
    /// The target the way was planned to and whether it reaches the target's place.
    goal: Option<[f32; 3]>,
    whole: bool,
    /// No way leads to the target from the follower's part of the grid (all of it grown).
    island: bool,
    /// The last plan found a way (whole or partial).
    found: bool,
    planned_at: Option<u32>,
    retry_at: Option<u32>,
    replan: bool,
    /// The target asked for last (the grid grows toward it).
    want: Option<[f32; 3]>,
    /// The least way left so far, when it last got `STUCK_MOVE` shorter and where the follower stood then, the last
    /// walk asked for, the jump.
    best_left: f32,
    best_at: u32,
    best_pos: Option<[f32; 3]>,
    last_walk: Option<u32>,
    jumped: bool,
    pub stats: FollowStats,
}

impl Default for Follower {
    fn default() -> Self {
        Follower {
            pts: Vec::new(),
            goal: None,
            whole: false,
            island: false,
            found: false,
            planned_at: None,
            retry_at: None,
            replan: false,
            want: None,
            best_left: f32::INFINITY,
            best_at: 0,
            best_pos: None,
            last_walk: None,
            jumped: false,
            stats: FollowStats::default(),
        }
    }
}

impl Follower {
    /// The points of the way ahead (the first is the next one).
    pub fn points(&self) -> &[[f32; 3]] {
        &self.pts
    }

    /// Whether the way planned reaches the target's place.
    pub fn whole(&self) -> bool {
        self.whole
    }

    /// Whether the last plan found the follower on an island (no way to the target on this world).
    pub fn island(&self) -> bool {
        self.island
    }

    /// Whether the follower stands at the end of a way that does not reach the target (the last plan's way led
    /// nearer, all of it is walked, and it is no whole way): the target is out of the grid's reach from here, or the
    /// grid has not grown to it yet.
    pub fn at_end(&self) -> bool {
        self.found && self.pts.is_empty() && !self.whole
    }

    /// Forgets the way and the stuck count (a new life, a new grid); the counters stay.
    pub fn clear(&mut self) {
        let stats = std::mem::take(&mut self.stats);
        *self = Follower {
            stats,
            ..Follower::default()
        };
    }

    /// Whether the follower at `pos` moved [`MOVED`] units (flat) from where the stuck count began (BOT-121: a way
    /// that got shorter only because it was planned again, or the target came nearer, is no progress of the body).
    fn moved(&self, pos: [f32; 3]) -> bool {
        self.best_pos.is_none_or(|b| flat(b, pos) >= MOVED)
    }

    /// The way left from `pos`: to the first point, then along the points (the target when there are none).
    fn left(&self, pos: [f32; 3]) -> f32 {
        match self.pts.first() {
            Some(w) => flat(pos, *w) + self.pts.windows(2).map(|p| flat(p[0], p[1])).sum::<f32>(),
            None => self.want.map_or(0.0, |t| flat(pos, t)),
        }
    }

    /// One growth step: the place `pos` joins the grid (linked at once), and while the way does not reach the target
    /// asked for last, up to `budget` probes grow the grid between the two. Returns the nodes added.
    pub fn grow(&mut self, grid: &mut NavGrid, pos: [f32; 3], budget: usize) -> usize {
        let mut added = usize::from(grid.seed_grown(pos));
        if let Some(t) = self.want.filter(|_| !self.whole && grid.growing()) {
            let t0 = std::time::Instant::now();
            added += grid.expand_between(budget, pos, t);
            let us = t0.elapsed().as_micros() as u64;
            self.stats.grow_max_us = self.stats.grow_max_us.max(us);
        }
        added
    }

    /// Where the follower at `pos` goes next on its way to `target` at `now` (ms), planning when a plan is due and
    /// `may_plan` (the caller's budget of plans).
    pub fn next(
        &mut self,
        grid: &mut NavGrid,
        now: u32,
        pos: [f32; 3],
        target: [f32; 3],
        may_plan: bool,
    ) -> NavGoal {
        self.want = Some(target);
        let reached =
            |w: &[f32; 3]| flat(*w, pos) <= REACHED && (w[1] - pos[1]).abs() <= REACHED_RISE;
        // the nodes reached are dropped, then the way ahead is pulled straight from here
        let before = self.pts.len();
        while self.pts.first().is_some_and(reached) {
            self.pts.remove(0);
        }
        if self.pts.len() < before && self.pts.len() > 1 {
            let k = grid.shortcut(pos, &self.pts, AHEAD);
            self.pts.drain(..k);
        }
        let moved = self.goal.is_none_or(|g| flat(g, target) > TARGET_MOVED);
        let due = (self.pts.is_empty() && !self.whole)
            || moved
            || self.replan
            || self
                .planned_at
                .is_none_or(|t| now.wrapping_sub(t) >= REPLAN_MS);
        let waiting = self.retry_at.is_some_and(|t| later(t, now));
        if due && !waiting && may_plan {
            self.plan(grid, now, pos, target);
        }
        match self.pts.first() {
            Some(w) => NavGoal::Way(*w),
            // at the end of a whole way: the target itself
            None if self.whole => NavGoal::Way(target),
            None if self.island => NavGoal::Hold,
            None => NavGoal::Steer,
        }
    }

    fn plan(&mut self, grid: &mut NavGrid, now: u32, pos: [f32; 3], target: [f32; 3]) {
        self.stats.plans += 1;
        self.planned_at = Some(now);
        self.replan = false;
        self.goal = Some(target);
        let t0 = std::time::Instant::now();
        let way = grid.route(pos, target);
        let us = t0.elapsed().as_micros() as u64;
        self.stats.plan_max_us = self.stats.plan_max_us.max(us);
        self.island = grid.last_way_closed();
        if self.island {
            self.stats.islands += 1;
        }
        let wait = if self.island {
            ISLAND_RETRY_MS
        } else {
            RETRY_MS
        };
        self.found = way.is_some();
        match way {
            Some(mut pts) => {
                let end = pts[pts.len() - 1];
                self.whole = flat(end, target) <= AT_TARGET
                    && (end[1] - target[1]).abs() <= REACHED_RISE
                    && grid.in_sight(end, target);
                if self.whole {
                    self.stats.whole += 1;
                }
                let k = grid.shortcut(pos, &pts, AHEAD);
                pts.drain(..k);
                pts.retain(|w| flat(*w, pos) > REACHED || (w[1] - pos[1]).abs() > REACHED_RISE);
                self.pts = pts;
                // a way that ends where the follower stands short of the target: the next plan waits
                self.retry_at = (self.island || (self.pts.is_empty() && !self.whole))
                    .then(|| now.wrapping_add(wait));
                // a longer way than the one followed (a detour) starts the stuck count again, only when the follower
                // moved since the count began (BOT-121: a target that moves gives longer and shorter ways all the time,
                // and a body that stands still is stuck all the same)
                let left = self.left(pos);
                if left > self.best_left + DETOUR && self.moved(pos) {
                    self.best_left = left;
                    self.best_at = now;
                    self.best_pos = Some(pos);
                }
            }
            None => {
                self.stats.failed += 1;
                self.pts.clear();
                self.whole = false;
                self.retry_at = Some(now.wrapping_add(wait));
            }
        }
    }

    /// The stuck check of a walk of the follower at `pos` along its way at `now` (call it on each tick it walks):
    /// the way left must get [`STUCK_MOVE`] shorter. After [`JUMP_MS`] without that it says [`Stuck::Jump`] (once),
    /// after [`STUCK_REPLAN_MS`] the link from where it stands toward the first point is given up, the way is
    /// dropped and the next [`Self::next`] plans at once ([`Stuck::Replan`]).
    pub fn stuck(&mut self, grid: &mut NavGrid, now: u32, pos: [f32; 3]) -> Stuck {
        let fresh = self
            .last_walk
            .is_none_or(|t| now.wrapping_sub(t) > WALK_GAP_MS);
        self.last_walk = Some(now);
        let left = self.left(pos);
        if fresh || (left < self.best_left - STUCK_MOVE && self.moved(pos)) {
            self.best_left = left;
            self.best_at = now;
            self.best_pos = Some(pos);
            self.jumped = false;
        }
        let stuck = now.wrapping_sub(self.best_at);
        if stuck >= STUCK_REPLAN_MS {
            if let Some(w) = self.pts.first().copied() {
                grid.ban_step(pos, w);
            }
            self.pts.clear();
            self.whole = false;
            self.replan = true;
            self.retry_at = None;
            self.best_left = f32::INFINITY;
            self.best_at = now;
            self.jumped = false;
            self.stats.stuck_replans += 1;
            return Stuck::Replan;
        }
        if stuck >= JUMP_MS && !self.jumped {
            self.jumped = true;
            self.stats.jumps += 1;
            return Stuck::Jump;
        }
        Stuck::No
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testworld::{cupped, solid_box, walled};
    use std::sync::Arc;

    const BODY: [f32; 3] = [16.0, 40.0, 16.0];

    /// Walks a follower over `grid` with straight strides of `stride` units per 50 ms step (the grid's own walk,
    /// as a kinematic body), up to `steps`; returns where it ended and the steps taken.
    fn follow(
        grid: &mut NavGrid,
        f: &mut Follower,
        from: [f32; 3],
        target: [f32; 3],
        steps: u32,
    ) -> ([f32; 3], u32) {
        let mut pos = from;
        for s in 0..steps {
            let now = s * STEP_MS;
            f.grow(grid, pos, GROW_PROBES);
            let w = match f.next(grid, now, pos, target, true) {
                NavGoal::Way(w) => w,
                _ => continue,
            };
            if flat(pos, target) <= REACHED {
                return (pos, s);
            }
            f.stuck(grid, now, pos);
            if let Some(p) = grid.walk(pos, w, 16.0, crate::STEP_UP) {
                pos = p;
            }
        }
        (pos, steps)
    }

    /// The way round a wall between the follower and its target is found from a partial way first (the grid grows
    /// between the two as the follower goes), and the follower gets there.
    #[test]
    fn a_follower_goes_round_a_wall_to_its_target() {
        let mut grid = NavGrid::new(walled(), BODY);
        let mut f = Follower::default();
        let (end, steps) = follow(
            &mut grid,
            &mut f,
            [0.0, 40.0, 0.0],
            [0.0, 40.0, 1200.0],
            2000,
        );
        assert!(
            flat(end, [0.0, 40.0, 1200.0]) <= REACHED,
            "ended at {end:?}"
        );
        assert!(steps < 2000);
        assert!(f.stats.whole >= 1, "{:?}", f.stats);
    }

    /// Out of a cup open away from the target: the follower walks out of its open side and round.
    #[test]
    fn a_follower_leaves_a_cup_open_away_from_the_target() {
        let mut grid = NavGrid::new(cupped(), BODY);
        let mut f = Follower::default();
        let (end, _) = follow(
            &mut grid,
            &mut f,
            [0.0, 40.0, 0.0],
            [0.0, 40.0, 1500.0],
            3000,
        );
        assert!(
            flat(end, [0.0, 40.0, 1500.0]) <= REACHED,
            "ended at {end:?}"
        );
    }

    /// A follower that makes no way jumps after 1.5 s (once), then gives up the link and plans again after 3 s.
    #[test]
    fn a_follower_that_makes_no_way_jumps_then_plans_again() {
        let mut grid = NavGrid::new(walled(), BODY);
        let mut f = Follower::default();
        let (pos, target) = ([0.0, 40.0, 0.0], [0.0, 40.0, 400.0]);
        grid.seed_grown(pos);
        for _ in 0..40 {
            f.grow(&mut grid, pos, GROW_PROBES);
        }
        assert!(matches!(
            f.next(&mut grid, 0, pos, target, true),
            NavGoal::Way(_)
        ));
        let mut seen = Vec::new();
        for now in (0..=3000).step_by(50) {
            let s = f.stuck(&mut grid, now, pos);
            if s != Stuck::No {
                seen.push((now, s));
            }
        }
        assert_eq!(seen, vec![(1500, Stuck::Jump), (3000, Stuck::Replan)]);
        assert!(f.points().is_empty());
        // the next call plans at once
        let plans = f.stats.plans;
        f.next(&mut grid, 3050, pos, target, true);
        assert_eq!(f.stats.plans, plans + 1);
        // a walk asked for after a pause starts the count again
        assert_eq!(f.stuck(&mut grid, 3050, pos), Stuck::No);
        assert_eq!(f.stuck(&mut grid, 5000, pos), Stuck::No);
        assert_eq!(f.stuck(&mut grid, 5040, pos), Stuck::No);
    }

    /// A follower in a pen (a closed box round it) is told it is on an island once its part of the grid is grown,
    /// and its way ends inside the pen.
    #[test]
    fn a_follower_in_a_pen_is_on_an_island() {
        let models = vec![
            solid_box([-5000.0, -64.0, -5000.0], [5000.0, 0.0, 5000.0]),
            solid_box([-200.0, 0.0, -200.0], [-160.0, 300.0, 200.0]),
            solid_box([160.0, 0.0, -200.0], [200.0, 300.0, 200.0]),
            solid_box([-200.0, 0.0, -200.0], [200.0, 300.0, -160.0]),
            solid_box([-200.0, 0.0, 160.0], [200.0, 300.0, 200.0]),
        ];
        let world = Arc::new(dc_sim::CollisionWorld::from_parts(models, &[]));
        let mut grid = NavGrid::new(world, BODY);
        let mut f = Follower::default();
        let (pos, target) = ([0.0, 40.0, 0.0], [0.0, 40.0, 1000.0]);
        for s in 0..200u32 {
            f.grow(&mut grid, pos, GROW_PROBES);
            f.next(&mut grid, s * STEP_MS, pos, target, true);
        }
        assert!(f.island(), "{:?}", f.stats);
        assert!(!f.whole());
        assert!(!f.at_end(), "the way to the pen's side is not walked yet");
        assert!(
            f.points().iter().all(|p| p[2].abs() < 160.0),
            "{:?}",
            f.points()
        );
    }

    /// A target on top of a box the body cannot climb: the way is partial, it ends at the box's foot, and there the
    /// follower says it stands at the end of a way short of the target (the bots hold there instead of pushing into
    /// the box, BOT-121).
    #[test]
    fn a_target_on_a_high_box_ends_the_way_at_its_foot() {
        let models = vec![
            solid_box([-5000.0, -64.0, -5000.0], [5000.0, 0.0, 5000.0]),
            solid_box([-100.0, 0.0, 900.0], [100.0, 250.0, 1100.0]),
        ];
        let world = Arc::new(dc_sim::CollisionWorld::from_parts(models, &[]));
        let mut grid = NavGrid::new(world, BODY);
        let mut f = Follower::default();
        let target = [0.0, 290.0, 1000.0];
        let mut pos = [0.0, 40.0, 0.0];
        for s in 0..2000u32 {
            let now = s * STEP_MS;
            f.grow(&mut grid, pos, GROW_PROBES);
            match f.next(&mut grid, now, pos, target, true) {
                NavGoal::Way(w) => {
                    f.stuck(&mut grid, now, pos);
                    if let Some(p) = grid.walk(pos, w, 16.0, crate::STEP_UP) {
                        pos = p;
                    }
                }
                _ if f.at_end() => break,
                _ => {}
            }
        }
        assert!(f.at_end(), "{:?} at {pos:?}", f.stats);
        assert!(!f.whole());
        assert!(flat(pos, target) < 160.0 && pos[1] < 60.0, "at {pos:?}");
    }

    /// A target on a floor above the follower that no way leads up to: the way ends right under it, which is not the
    /// target's place (before, the flat distance alone made that a whole way, and bots of two teams stood on two
    /// floors under each other's flag carriers).
    #[test]
    fn a_way_that_ends_under_a_target_on_a_floor_above_is_not_whole() {
        let models = vec![
            solid_box([-5000.0, -64.0, -5000.0], [5000.0, 0.0, 5000.0]),
            solid_box([-300.0, 200.0, 700.0], [300.0, 216.0, 1300.0]),
        ];
        let world = Arc::new(dc_sim::CollisionWorld::from_parts(models, &[]));
        let mut grid = NavGrid::new(world, BODY);
        let mut f = Follower::default();
        let target = [0.0, 256.0, 1000.0];
        let mut pos = [0.0, 40.0, 0.0];
        for s in 0..2000u32 {
            let now = s * STEP_MS;
            f.grow(&mut grid, pos, GROW_PROBES);
            match f.next(&mut grid, now, pos, target, true) {
                NavGoal::Way(w) => {
                    if flat(pos, target) < 30.0 {
                        break;
                    }
                    if let Some(p) = grid.walk(pos, w, 16.0, crate::STEP_UP) {
                        pos = p;
                    }
                }
                _ if f.at_end() => break,
                _ => {}
            }
        }
        assert!(!f.whole(), "a whole way to {target:?} from {pos:?}");
        assert!(pos[1] < 60.0, "at {pos:?}");
    }

    /// A follower that stands still while its target moves (away: each new plan a longer way; nearer: each one
    /// shorter) is stuck all the same: it jumps and plans again in time.
    #[test]
    fn a_follower_that_stands_still_is_stuck_while_its_target_moves() {
        let runs: [fn(f32) -> [f32; 3]; 2] = [
            |i| [i * 12.0, 40.0, 400.0],
            |i| [0.0, 40.0, 1200.0 - i * 12.0],
        ];
        for target_at in runs {
            let mut grid = NavGrid::new(walled(), BODY);
            let mut f = Follower::default();
            let pos = [0.0, 40.0, 0.0];
            for _ in 0..40 {
                grid.seed_grown(pos);
                grid.expand(crate::BUDGET * 4, Some([0.0, 40.0, 400.0]));
            }
            let mut seen = Vec::new();
            for (i, now) in (0..=3000u32).step_by(50).enumerate() {
                f.next(&mut grid, now, pos, target_at(i as f32), true);
                let s = f.stuck(&mut grid, now, pos);
                if s != Stuck::No {
                    seen.push(s);
                }
            }
            assert!(seen.contains(&Stuck::Jump), "{seen:?} {:?}", f.stats);
            assert!(seen.contains(&Stuck::Replan), "{seen:?} {:?}", f.stats);
        }
    }

    /// A target right behind a wall that no way leads round: the way ends at the wall on this side, which is not the
    /// target's place (before, two flag carriers stood on the two sides of a wall on Gray Hammer, each "at" the other).
    #[test]
    fn a_way_that_ends_behind_a_wall_from_the_target_is_not_whole() {
        let models = vec![
            solid_box([-5000.0, -64.0, -5000.0], [5000.0, 0.0, 5000.0]),
            // a closed room round the target, its near wall 24 thick
            solid_box([-300.0, 0.0, 500.0], [300.0, 300.0, 524.0]),
            solid_box([-300.0, 0.0, 1000.0], [300.0, 300.0, 1024.0]),
            solid_box([-324.0, 0.0, 500.0], [-300.0, 300.0, 1024.0]),
            solid_box([300.0, 0.0, 500.0], [324.0, 300.0, 1024.0]),
        ];
        let world = Arc::new(dc_sim::CollisionWorld::from_parts(models, &[]));
        let mut grid = NavGrid::new(world, BODY);
        let mut f = Follower::default();
        let target = [0.0, 40.0, 540.0];
        let mut pos = [0.0, 40.0, 0.0];
        for s in 0..3000u32 {
            let now = s * STEP_MS;
            f.grow(&mut grid, pos, GROW_PROBES);
            match f.next(&mut grid, now, pos, target, true) {
                NavGoal::Way(w) => {
                    if f.points().is_empty() {
                        break;
                    }
                    if let Some(p) = grid.walk(pos, w, 16.0, crate::STEP_UP) {
                        pos = p;
                    }
                }
                _ if f.at_end() => break,
                _ => {}
            }
        }
        assert!(!f.whole(), "a whole way to {target:?} from {pos:?}");
        assert!(pos[2] < 500.0, "at {pos:?}");
    }
}
