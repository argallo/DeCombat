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
/// BOT-141: a follower with no way ahead (at the end of a partial way, or no way found yet) grows the grid this many
/// times as fast: a way that first leads far away from the target (Waverider: an intel case on a deck 220 above the
/// floor under it, reached round by the far stairs) was found after 145 s of standing at the end of the partial way.
// DC-DEFAULT: BOT-141 (proposed) 8 times (960 probes, about 5 ms per 50 ms step, only while the bot has no way)
pub const GROW_BOOST: usize = 8;
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
/// BOT-141: a point that lay more than a step above the body when it became the next one (the top of a jump or a
/// ladder) counts as reached only once the body stands within this height of it (before: within a step, so a body
/// in the flight of its jump had reached the top of the crate it was jumping onto and steered on).
// DC-DEFAULT: BOT-141 (proposed) 8 units
pub const TOP_LEVEL: f32 = 8.0;
/// BOT-141: for this long after the follower said jump, the way it jumps along is not planned again on the clock (a
/// plan from the body in flight began elsewhere, the body steered off the crate's edge and fell back).
// DC-DEFAULT: BOT-141 (proposed) 1000 ms (a jump's flight, 2 x 330 / 1100 = 0.6 s, and the step onto the edge)
pub const JUMP_HOLD_MS: u32 = 1000;
/// BOT-141: the follower within [`TOP_NEAR`] (flat) of a next point that is a top (a jump up) says jump as soon as a
/// walk step gets it less than [`TOP_GAIN`] units (flat) nearer to it (the body met the ledge's face) instead of
/// after [`JUMP_MS`] without way: on a crate top the body slid along the face of the ledge above and walked off the
/// crate before the jump. (On its feet only: its height the same as a step before.)
// DC-DEFAULT: BOT-141 (proposed) 96 units (the two lattice steps of a jump link), 4 units (a free walk step of 50 ms
// at 285 units/s is 14)
pub const TOP_NEAR: f32 = 96.0;
pub const TOP_GAIN: f32 = 4.0;
/// BOT-141: at most this many such jumps at the face of one top (then the stuck jump of [`JUMP_MS`]; BOT-153: then
/// the refusal, [`FACE_SPENT`]).
// DC-DEFAULT: BOT-141 (proposed) 2
pub const FACE_JUMPS: u32 = 2;
/// BOT-153: after this many jumps at one top (the face jumps and the stuck jump of [`JUMP_MS`]) the body back at its
/// face refuses the top (BOT-145) instead of jumping again.
// DC-DEFAULT: BOT-153 (proposed) 5 (the Warhead crate a Spy Hunt case lies on takes four jumps at the rifle's run
// speed, nav_rom)
pub const FACE_SPENT: u32 = 5;
/// BOT-141: the face jump only where a jump link of the grid leads onto the next point and it stands at least this
/// high above the body (a real ledge, not a stair or a ramp: 270 of 342 jumps of a Gray Hammer CTF room were face
/// jumps on stairs and ramps before), or where the next point stands higher than any jump (a ladder's top: Waverider's
/// deck ladder, whose rungs begin above the floor, is taken with a jump).
// DC-DEFAULT: BOT-141 (proposed) 40 units
pub const FACE_RISE: f32 = 40.0;
/// BOT-153: the face jump onto a ledge only when its top is no higher than a jump (BOT-141: 76) and no farther than
/// [`FACE_NEAR`] (flat): of the face jumps of four 2026-11-08 rooms, those with a rise of 45..75 failed (the body no
/// higher 0.4..2 s later) 55 % of the time, 80 % beyond 72 units from the top. (A cap of 72 in height cost the
/// Waverider deck case its way, nav_rom.)
// DC-DEFAULT: BOT-153 (proposed) 76 units, 72 units
pub const FACE_MAX_RISE: f32 = 76.0;
pub const FACE_NEAR: f32 = 72.0;
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
/// BOT-133: a trap: a second stuck re-plan (or watchdog kick) within `TRAP_NEAR` units (flat) of the one before and
/// within `TRAP_WINDOW_MS` of it gives up every link of the node the follower stands at (and of the node at the place
/// before), and the follower first backs up `BACK_MIN` units along its trail (the places it passed, kept every
/// `TRAIL_STEP` units, the last `TRAIL_KEEP`) for at most `BACK_MS` before it plans again.
// DC-DEFAULT: BOT-133 (proposed) 48 units, 15000 ms, 64 units, 32 units, 8 places, 800 ms
pub const TRAP_NEAR: f32 = 48.0;
pub const TRAP_WINDOW_MS: u32 = 15_000;
pub const BACK_MIN: f32 = 64.0;
pub const TRAIL_STEP: f32 = 32.0;
pub const TRAIL_KEEP: usize = 8;
pub const BACK_MS: u32 = 800;
/// BOT-145: a way up (a jump, a ladder, a stairway) the body did not take (the follower's stuck re-plan or a watchdog kick came while the next point
/// was its top, or the bot's jump test refused the flight there) is taken out of the grid for this long: every link
/// up onto that top, so the plan goes round however often the goal changes (the hunt goal moves every 2 s; before,
/// BOT-113 gave up the link from the node nearest the body, which under a ledge is not the jump's take-off, and each
/// new plan led to the same face again: Snow Valley's rock, Two Towers' and Rattlesnake's ledges, 24..146 s).
// DC-DEFAULT: BOT-145 (proposed) 120 s
pub const REFUSE_MS: u32 = 120_000;
/// BOT-145: a top that was the follower's next point for [`TOP_TRY_MS`] in all, or became it [`TOP_TRIES`] times,
/// within [`TOP_WINDOW_MS`] while the body never got up onto it is refused too: a body that paces between the foot of
/// a stairway or a jump and the face under its top (each plan of 2 s leads back to the foot) moves enough for every
/// stuck count (Two Towers under the ledge of team 2's spawn, 65 units to and fro; Snow Valley's rock), and one that
/// gets onto the first crate of a stack and walks off it before the second jump (Gray Hammer (596, 125, 767) ->
/// (548, 175, 695), every 5.5 s at the rifle's run speed) comes back to the same top again and again. Next points
/// asked for within [`TOP_STEP_GAP_MS`] of each other count as one stay.
// DC-DEFAULT: BOT-145 (proposed) 5000 ms (a face jump is taken within 1.5 s, a stairway of six lattice steps walked in
// about 1 s), 3 times, 20 s, 250 ms
pub const TOP_TRY_MS: u32 = 5000;
pub const TOP_TRIES: u32 = 4;
pub const TOP_WINDOW_MS: u32 = 20_000;
pub const TOP_STEP_GAP_MS: u32 = 250;
/// BOT-145: a stuck re-plan, a watchdog kick or a refused jump at a top refuses it at the second time at the same top
/// (a crate a face jump gets onto at the second try, BOT-141, stays a way).
// DC-DEFAULT: BOT-145 (proposed) 2
pub const TOP_FAILS: u32 = 3;

/// A grid node's lattice key.
type Lattice = (i32, i32, i32);

/// BOT-145: a top being tried: where, since when, when it was the next point last, for how long in all, how often.
#[derive(Debug, Clone)]
struct TopTry {
    at: [f32; 3],
    first: u32,
    last: u32,
    ms: u32,
    tries: u32,
}

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
    /// BOT-133: a re-plan at the place of the one before (a trap): every link of the node there was given up and the
    /// follower backs up along its trail first.
    Trap,
}

/// Counters of a follower (tests, logs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FollowStats {
    /// BOT-145: tops refused (the links up onto them taken out for [`REFUSE_MS`]), and refusals not made because the
    /// follower would have been left on an island.
    pub refusals: u64,
    pub refusals_kept: u64,
    /// BOT-153: tops refused after their face jumps were spent.
    pub face_refusals: u64,
    /// Plans made, those that reached the target's place, those that found no way, those that found the follower on
    /// an island.
    pub plans: u64,
    pub whole: u64,
    pub failed: u64,
    pub islands: u64,
    /// Stuck jumps and re-plans after being stuck.
    pub jumps: u64,
    pub stuck_replans: u64,
    /// BOT-133: traps (nodes given up whole), back-ups begun, watchdog kicks.
    pub traps: u64,
    pub backups: u64,
    pub kicks: u64,
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
    /// BOT-133: the places passed (newest last), the last re-plan or kick (when, where), the back-up in force (the
    /// place backed up to, until when).
    trail: Vec<[f32; 3]>,
    last_trap: Option<(u32, [f32; 3])>,
    back: Option<([f32; 3], u32)>,
    /// BOT-141: the next point and whether it lay more than a step above the body when it became the next one.
    top: Option<([f32; 3], bool)>,
    /// BOT-141: when the follower last said jump.
    jump_at: Option<u32>,
    /// BOT-141: where the body stood at the last walk step, and the top the follower last jumped at its face for
    /// (how often).
    walk_pos: Option<[f32; 3]>,
    face_jumps: Option<([f32; 3], u32)>,
    /// BOT-145: the jump links taken out of the grid (from, to) and until when.
    refused: Vec<(Lattice, Lattice, u32)>,
    /// BOT-145: the top being tried (a next point more than a step above the body), since when, and when it was the
    /// next point last.
    tops: Vec<TopTry>,
    /// BOT-145: the last top a failure was counted at, and how many.
    top_fails: Option<([f32; 3], u32)>,
    pub stats: FollowStats,
    /// BOT-141: why the follower said jump last (for the session's event): at a ledge's face (else after the
    /// stuck time), the next point's height over the body, its flat distance, and whether it was a top.
    pub last_jump: Option<JumpWhy>,
}

/// BOT-141: why a follower said jump.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JumpWhy {
    pub face: bool,
    pub rise: f32,
    pub flat: f32,
    pub top: bool,
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
            trail: Vec::new(),
            last_trap: None,
            back: None,
            top: None,
            jump_at: None,
            walk_pos: None,
            face_jumps: None,
            refused: Vec::new(),
            tops: Vec::new(),
            top_fails: None,
            stats: FollowStats::default(),
            last_jump: None,
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

    /// BOT-141: the way left from `pos` to `target` along the way planned to it: to the first point, along the points,
    /// then from the last one to the target (3D: a partial way that ends under a case on a ledge is not near it).
    /// `None` when the way was planned to another place, or no way is planned (none ahead and no whole way).
    pub fn way_left(&self, pos: [f32; 3], target: [f32; 3]) -> Option<f32> {
        self.goal.filter(|g| flat(*g, target) <= TARGET_MOVED)?;
        let d3 = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        match (self.pts.first(), self.pts.last()) {
            (Some(w), Some(l)) => Some(
                flat(pos, *w)
                    + self.pts.windows(2).map(|p| flat(p[0], p[1])).sum::<f32>()
                    + d3(*l, target),
            ),
            _ if self.whole => Some(d3(pos, target)),
            _ => None,
        }
    }

    /// BOT-141: whether the follower, asked for `target`, has no way ahead yet while its grid may still grow one (no
    /// point ahead, no whole way, not on an island).
    pub fn waiting_for_way(&self, target: [f32; 3]) -> bool {
        self.want.is_some_and(|t| flat(t, target) <= TARGET_MOVED)
            && self.pts.is_empty()
            && !self.whole
            && !self.island
    }

    /// Forgets the way and the stuck count (a new life, a new grid); the counters stay.
    pub fn clear(&mut self) {
        let stats = std::mem::take(&mut self.stats);
        let refused = std::mem::take(&mut self.refused);
        *self = Follower {
            stats,
            refused,
            ..Follower::default()
        };
    }

    /// BOT-145: forgets the jump links taken out (a new grid: they belong to the old one).
    pub fn forget_refused(&mut self) {
        self.refused.clear();
    }

    /// BOT-145: the body at `pos` did not get up onto the next point (its jump test refused the flight, or it made no
    /// way there): every link up onto that top is taken out of the grid for [`REFUSE_MS`] and the next call plans at
    /// once: at the second such failure at the same top ([`TOP_FAILS`]), or at once with `force` (the top was tried
    /// for [`TOP_TRY_MS`]). Not when that leaves the body on an island of the grid (no way to the target at all from
    /// there: the refused way up is its only way out, kept as before). `false` when nothing changed.
    pub fn refuse(&mut self, grid: &mut NavGrid, now: u32, pos: [f32; 3], force: bool) -> bool {
        let Some((w, true)) = self.top else {
            return false;
        };
        let fails = match self.top_fails {
            Some((t, n)) if flat(t, w) < 1.0 && (t[1] - w[1]).abs() < 1.0 => n + 1,
            _ => 1,
        };
        self.top_fails = Some((w, fails));
        if !force && fails < TOP_FAILS {
            return false;
        }
        let out = grid.unlink_ups_onto(w);
        if out.is_empty() {
            return false;
        }
        if let Some(t) = self.want {
            grid.route(pos, t);
            if grid.last_way_closed() {
                for l in out {
                    grid.relink(l);
                }
                self.stats.refusals_kept += 1;
                return false;
            }
        }
        self.stats.refusals += 1;
        let until = now.wrapping_add(REFUSE_MS).max(1);
        self.refused
            .extend(out.into_iter().map(|(a, b)| (a, b, until)));
        self.pts.clear();
        self.whole = false;
        self.replan = true;
        self.retry_at = None;
        self.top = None;
        true
    }

    /// BOT-145: the jump links whose time is out go back into the grid.
    fn restore(&mut self, grid: &mut NavGrid, now: u32) {
        if self.refused.iter().all(|r| later(r.2, now)) {
            return;
        }
        let mut keep = Vec::new();
        for r in std::mem::take(&mut self.refused) {
            if later(r.2, now) {
                keep.push(r);
            } else {
                grid.relink((r.0, r.1));
            }
        }
        self.refused = keep;
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
            let budget = if self.pts.is_empty() {
                budget * GROW_BOOST
            } else {
                budget
            };
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
        self.restore(grid, now);
        // BOT-133: the trail of places passed, and a back-up in force before the next plan
        if self
            .trail
            .last()
            .is_none_or(|l| flat(*l, pos) >= TRAIL_STEP)
        {
            if self.trail.len() >= TRAIL_KEEP {
                self.trail.remove(0);
            }
            self.trail.push(pos);
        }
        if let Some((to, until)) = self.back {
            if later(until, now) && flat(to, pos) > REACHED {
                return NavGoal::Way(to);
            }
            self.back = None;
            // the stuck count starts again from where the back-up ended
            self.last_walk = None;
            self.replan = true;
            self.retry_at = None;
        }
        // BOT-139: a point more than a step above the body (the top of a jump) is reached only once the body is up;
        // BOT-141: up on it, not in the flight below its height
        let top = self.top;
        let reached = |w: &[f32; 3]| {
            let high = match top {
                Some((t, h)) if t == *w => h,
                _ => w[1] - pos[1] > crate::STEP_UP + 1.0,
            };
            let rise = if high {
                TOP_LEVEL
            } else {
                crate::STEP_UP + 1.0
            };
            flat(*w, pos) <= REACHED && pos[1] - w[1] <= REACHED_RISE && w[1] - pos[1] <= rise
        };
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
        let flight = self
            .jump_at
            .is_some_and(|t| now.wrapping_sub(t) < JUMP_HOLD_MS);
        let due = (self.pts.is_empty() && !self.whole)
            || moved
            || self.replan
            || (!flight
                && self
                    .planned_at
                    .is_none_or(|t| now.wrapping_sub(t) >= REPLAN_MS));
        let waiting = self.retry_at.is_some_and(|t| later(t, now));
        if due && !waiting && may_plan {
            self.plan(grid, now, pos, target);
        }
        // BOT-141: whether the next point is a top, judged when it became the next one
        self.top = self.pts.first().map(|w| match top {
            Some((t, h)) if t == *w => (*w, h),
            _ => (*w, w[1] - pos[1] > crate::STEP_UP + 1.0),
        });
        // BOT-145: a top tried for too long, or too often, without the body getting up is refused (the plan goes
        // round)
        if let Some((w, true)) = self.top {
            self.tops
                .retain(|t| now.wrapping_sub(t.first) <= TOP_WINDOW_MS);
            let same = |t: &TopTry| flat(t.at, w) < 1.0 && (t.at[1] - w[1]).abs() < 1.0;
            if w[1] - pos[1] <= TOP_LEVEL {
                self.tops.retain(|t| !same(t));
            } else {
                match self.tops.iter_mut().find(|t| same(t)) {
                    Some(t) => {
                        let dt = now.wrapping_sub(t.last);
                        if dt <= TOP_STEP_GAP_MS {
                            t.ms += dt;
                        } else {
                            t.tries += 1;
                        }
                        t.last = now;
                    }
                    None => {
                        if self.tops.len() >= 8 {
                            self.tops.remove(0);
                        }
                        self.tops.push(TopTry {
                            at: w,
                            first: now,
                            last: now,
                            ms: 0,
                            tries: 1,
                        });
                    }
                }
                let worn = self
                    .tops
                    .iter()
                    .any(|t| same(t) && (t.ms >= TOP_TRY_MS || t.tries >= TOP_TRIES));
                if worn && may_plan {
                    self.tops.retain(|t| !same(t));
                    if self.refuse(grid, now, pos, true) {
                        self.plan(grid, now, pos, target);
                        self.top = self
                            .pts
                            .first()
                            .map(|w| (*w, w[1] - pos[1] > crate::STEP_UP + 1.0));
                    }
                }
            }
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
        // BOT-133: a back-up is no walk along the way
        if self.backing() {
            self.last_walk = None;
            return Stuck::No;
        }
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
            // BOT-145: at the face of a jump up the jump links onto its top go; else the link toward the next point
            let refused = self.refuse(grid, now, pos, false);
            if let Some(w) = self.pts.first().copied().filter(|_| !refused) {
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
            if self.trap(grid, now, pos) {
                return Stuck::Trap;
            }
            return Stuck::Replan;
        }
        let last = self.walk_pos.replace(pos).filter(|_| !fresh);
        let tries = self
            .face_jumps
            .filter(|f| self.top.is_some_and(|t| t.0 == f.0))
            .map_or(0, |f| f.1);
        let facing = self.top.is_some_and(|(w, high)| {
            high && flat(w, pos) <= TOP_NEAR
                    && last.is_some_and(|l| {
                        // (on its feet: a body in flight rises or falls between two steps)
                        flat(l, w) - flat(pos, w) < TOP_GAIN && (l[1] - pos[1]).abs() < crate::LEVEL
                    })
                    // a ledge a jump link leads onto, or a top higher than any jump (a ladder's: the body gets
                    // onto the rungs above the floor with a jump)
                    // BOT-153: a ledge only within a standing jump's reach (a rise of at most FACE_MAX_RISE, the
                    // face within FACE_NEAR)
                    && ((w[1] - pos[1] >= FACE_RISE
                        && w[1] - pos[1] <= FACE_MAX_RISE
                        && flat(w, pos) <= FACE_NEAR
                        && grid.jumped_onto(w))
                        || w[1] - pos[1] > crate::JUMP_RISE)
        });
        let at_face = tries < FACE_JUMPS && facing;
        // BOT-153: a body back at the face of a top it jumped at FACE_SPENT times and never got onto refuses the
        // top at once (the plan goes round) instead of jumping again (before: up to the third stuck failure, BOT-145)
        // (a ladder's top, higher than any jump, keeps its jumps: the rungs begin above the floor)
        let spent = tries >= FACE_SPENT
            && self.top.is_some_and(|(w, high)| {
                high && flat(w, pos) <= TOP_NEAR
                    && w[1] - pos[1] > TOP_LEVEL
                    && w[1] - pos[1] <= crate::JUMP_RISE
            });
        if spent && (stuck >= JUMP_MS || facing) {
            self.face_jumps = None;
            if self.refuse(grid, now, pos, true) {
                self.stats.face_refusals += 1;
                return Stuck::No;
            }
        }
        if (stuck >= JUMP_MS || at_face) && !self.jumped && !spent {
            // (BOT-153: every jump at a top counts toward FACE_SPENT, the stuck jump of JUMP_MS too)
            if let Some((w, true)) = self.top {
                self.face_jumps = Some((w, tries + 1));
            }
            self.jumped = true;
            self.jump_at = Some(now);
            self.stats.jumps += 1;
            let next = self.pts.first().copied();
            self.last_jump = Some(JumpWhy {
                face: at_face,
                rise: next.map_or(0.0, |w| w[1] - pos[1]),
                flat: next.map_or(0.0, |w| flat(w, pos)),
                top: self.top.is_some_and(|t| t.1),
            });
            return Stuck::Jump;
        }
        Stuck::No
    }

    /// BOT-145: the place to walk back to along the way the body came (the newest place passed at least
    /// [`BACK_MIN`] from `pos`, else the oldest), `None` with no such place.
    pub fn trail_back(&self, pos: [f32; 3]) -> Option<[f32; 3]> {
        self.trail
            .iter()
            .rev()
            .find(|q| flat(**q, pos) >= BACK_MIN)
            .or_else(|| self.trail.first().filter(|q| flat(**q, pos) > REACHED))
            .copied()
    }

    /// BOT-133: whether a back-up is in force (the follower walks back along its trail).
    pub fn backing(&self) -> bool {
        self.back.is_some()
    }

    /// BOT-133: the body made no way although the bot wants to walk (a watchdog outside the follower's own stuck
    /// count, which a change of steering keeps starting again): the way is dropped and the next [`Self::next`] plans
    /// at once; at the place of the last re-plan or kick it is a trap ([`Stuck::Trap`]'s rule). `true` on a trap.
    pub fn kick(&mut self, grid: &mut NavGrid, now: u32, pos: [f32; 3]) -> bool {
        self.stats.kicks += 1;
        // BOT-145: a body that made no way at the face of a jump up did not take it
        self.refuse(grid, now, pos, false);
        self.pts.clear();
        self.whole = false;
        self.replan = true;
        self.retry_at = None;
        self.best_left = f32::INFINITY;
        self.best_at = now;
        self.last_walk = None;
        self.jumped = false;
        self.trap(grid, now, pos)
    }

    /// BOT-133: a re-plan or kick at `pos`: when the one before was within [`TRAP_NEAR`] and [`TRAP_WINDOW_MS`],
    /// every link of the node nearest `pos` (and of the node nearest the place before) is given up, so no plan leaves
    /// from there or passes through it (the grid keeps that), and a back-up along the trail begins. Before, one link per
    /// 3 s was given up and the nodes beside it kept offering parallel links into the same edge.
    fn trap(&mut self, grid: &mut NavGrid, now: u32, pos: [f32; 3]) -> bool {
        let before = self.last_trap.replace((now, pos));
        let Some((t, p)) = before else {
            return false;
        };
        if now.wrapping_sub(t) > TRAP_WINDOW_MS || flat(p, pos) > TRAP_NEAR {
            return false;
        }
        self.stats.traps += 1;
        ban_node(grid, pos);
        ban_node(grid, p);
        self.last_trap = None;
        let to = self
            .trail
            .iter()
            .rev()
            .find(|q| flat(**q, pos) >= BACK_MIN)
            .or_else(|| self.trail.first().filter(|q| flat(**q, pos) > REACHED))
            .copied();
        if let Some(to) = to {
            self.stats.backups += 1;
            self.back = Some((to, now.wrapping_add(BACK_MS)));
        }
        true
    }
}

/// BOT-133: gives up every link of the node nearest `pos` (within the grid's reach of a standing place); the links
/// given up.
fn ban_node(grid: &mut NavGrid, pos: [f32; 3]) -> usize {
    let toward = [pos[0] + 1.0, pos[1], pos[2]];
    let mut n = 0;
    while n < 64 && grid.ban_step(pos, toward) {
        n += 1;
    }
    n
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

    /// BOT-141: at the face of a ledge whose top is the next point the follower says jump at the first walk step
    /// that gets no nearer (not after 1.5 s), plans nothing on the clock during the flight, and the top counts as
    /// reached only once the body is up on it (a body a step below it, in its flight, has not reached it).
    #[test]
    fn a_follower_at_a_ledge_jumps_at_once_and_reaches_the_top_only_up_there() {
        let w = Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-2000.0, -64.0, -2000.0], [2000.0, 0.0, 2000.0]),
                solid_box([-2000.0, 0.0, 100.0], [2000.0, 60.0, 2000.0]),
            ],
            &[],
        ));
        let mut grid = NavGrid::new(w, BODY);
        let mut f = Follower::default();
        let (pos, target) = ([0.0, 40.0, 80.0], [0.0, 100.0, 400.0]);
        grid.seed_grown(pos);
        for _ in 0..60 {
            f.grow(&mut grid, pos, GROW_PROBES);
        }
        let NavGoal::Way(top) = f.next(&mut grid, 0, pos, target, true) else {
            panic!("no way");
        };
        assert!(top[1] > 90.0, "the next point is on the ledge: {top:?}");
        assert_eq!(f.stuck(&mut grid, 1900, pos), Stuck::No);
        assert_eq!(f.stuck(&mut grid, 1950, pos), Stuck::Jump);
        let plans = f.stats.plans;
        let air = [top[0], top[1] - 20.0, top[2]];
        assert!(matches!(f.next(&mut grid, 2100, air, target, true), NavGoal::Way(w) if w == top));
        assert_eq!(f.stats.plans, plans, "no plan in the flight");
        let up = [top[0], top[1], top[2]];
        assert!(!matches!(f.next(&mut grid, 2150, up, target, true), NavGoal::Way(w) if w == top));
    }

    /// BOT-145: a body that stays under the face of a ledge (it never gets up) has the ways up onto that top (and the
    /// tops beside it) refused after `TOP_TRY_MS`: the next point is no longer up there; the links come back after
    /// `REFUSE_MS`. A first stuck re-plan at the face keeps the top.
    #[test]
    fn a_top_the_body_does_not_get_onto_is_refused_for_a_while() {
        let w = Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-2000.0, -64.0, -2000.0], [2000.0, 0.0, 2000.0]),
                solid_box([-2000.0, 0.0, 100.0], [2000.0, 60.0, 2000.0]),
            ],
            &[],
        ));
        let mut grid = NavGrid::new(w, BODY);
        let mut f = Follower::default();
        let (pos, target) = ([0.0, 40.0, 80.0], [0.0, 100.0, 400.0]);
        grid.seed_grown(pos);
        for _ in 0..60 {
            f.grow(&mut grid, pos, GROW_PROBES);
        }
        let NavGoal::Way(top) = f.next(&mut grid, 0, pos, target, true) else {
            panic!("no way");
        };
        assert!(top[1] > 90.0, "the next point is on the ledge: {top:?}");
        let mut now = 0;
        while now < TOP_TRY_MS && f.stats.refusals == 0 {
            now += STEP_MS;
            f.next(&mut grid, now, pos, target, true);
        }
        assert_eq!(f.stats.refusals, 1, "refused after {now} ms");
        assert!(now >= TOP_TRY_MS, "{now}");
        let next = f.next(&mut grid, now + STEP_MS, pos, target, true);
        assert!(
            !matches!(next, NavGoal::Way(w) if flat(w, top) <= crate::REFUSE_NEAR && w[1] > 90.0),
            "{next:?}"
        );
        // the refusal lasts; then the ways up come back
        let later = now + REFUSE_MS + STEP_MS;
        f.clear();
        assert!(
            matches!(f.next(&mut grid, later, pos, target, true), NavGoal::Way(w) if w[1] > 90.0),
            "the ledge is a way again"
        );
        // a first stuck re-plan with the next point up there keeps the top (a face jump may get on at the next try)
        let mut g = Follower::default();
        let NavGoal::Way(_) = g.next(&mut grid, later, pos, target, true) else {
            panic!("no way");
        };
        let mut seen = Stuck::No;
        for k in 0..=(STUCK_REPLAN_MS / STEP_MS) {
            seen = g.stuck(&mut grid, later + k * STEP_MS, pos);
            if matches!(seen, Stuck::Replan | Stuck::Trap) {
                break;
            }
        }
        assert_eq!(seen, Stuck::Replan);
        assert_eq!(g.stats.refusals, 0);
    }

    /// BOT-145: the way back along the trail is the newest place passed at least `BACK_MIN` away.
    #[test]
    fn the_trail_back_is_the_newest_place_far_enough() {
        let mut grid = NavGrid::new(walled(), BODY);
        let mut f = Follower::default();
        let t = [0.0, 40.0, 1200.0];
        for (k, z) in [0.0f32, 40.0, 80.0, 120.0, 160.0].iter().enumerate() {
            f.next(&mut grid, k as u32 * STEP_MS, [0.0, 40.0, *z], t, false);
        }
        assert_eq!(f.trail_back([0.0, 40.0, 160.0]), Some([0.0, 40.0, 80.0]));
        assert_eq!(f.trail_back([0.0, 40.0, 40.0]), Some([0.0, 40.0, 160.0]));
    }

    /// BOT-141: the way left along the planned way to a target, `None` for another target.
    #[test]
    fn the_way_left_follows_the_planned_way() {
        let mut grid = NavGrid::new(walled(), BODY);
        let mut f = Follower::default();
        let (from, target) = ([0.0, 40.0, 0.0], [0.0, 40.0, 1200.0]);
        let mut pos = from;
        let mut left = None;
        for s in 0..400u32 {
            f.grow(&mut grid, pos, GROW_PROBES);
            if let NavGoal::Way(w) = f.next(&mut grid, s * STEP_MS, pos, target, true) {
                left = f.way_left(pos, target).filter(|_| f.whole());
                if left.is_some() {
                    break;
                }
                pos = grid.walk(pos, w, 16.0, crate::STEP_UP).unwrap_or(pos);
            }
        }
        let left = left.expect("a whole way");
        // (the wall stands across the straight line: the way round is longer)
        assert!(
            left > flat(pos, target) + 100.0,
            "round the wall: {left} vs {}",
            flat(pos, target)
        );
        assert_eq!(f.way_left(pos, [3000.0, 40.0, 0.0]), None);
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

    /// BOT-133: a follower whose body stays in one place re-plans after 3 s; the second re-plan there is a trap:
    /// every link of the node it stands at is given up, it backs up along its trail (at least 64 units) and then plans
    /// again from there.
    #[test]
    fn a_second_replan_at_the_same_place_gives_up_the_node_and_backs_up() {
        let mut grid = NavGrid::new(walled(), BODY);
        let mut f = Follower::default();
        let target = [0.0, 40.0, 400.0];
        // the follower came up from z -240 to z 0
        let mut now = 0;
        for i in 0..=10 {
            let pos = [0.0, 40.0, -240.0 + 24.0 * i as f32];
            f.grow(&mut grid, pos, GROW_PROBES);
            f.next(&mut grid, now, pos, target, true);
            now += STEP_MS;
        }
        let pos = [0.0, 40.0, 0.0];
        for _ in 0..40 {
            f.grow(&mut grid, pos, GROW_PROBES);
        }
        let mut seen = Vec::new();
        let start = now;
        while now <= start + 6500 {
            if let NavGoal::Way(_) = f.next(&mut grid, now, pos, target, true) {
                let s = f.stuck(&mut grid, now, pos);
                if s != Stuck::No {
                    seen.push(s);
                }
            }
            if f.backing() {
                break;
            }
            now += STEP_MS;
        }
        assert_eq!(
            seen,
            vec![Stuck::Jump, Stuck::Replan, Stuck::Jump, Stuck::Trap]
        );
        assert_eq!(f.stats.traps, 1);
        let node = grid
            .links_near(pos, 30.0)
            .into_iter()
            .min_by(|a, b| flat(a.0, pos).total_cmp(&flat(b.0, pos)))
            .expect("a node at the place");
        assert!(node.1.is_empty(), "links left: {:?}", node.1);
        // the back-up: a place on the trail at least 64 units back, while it lasts no plan and no stuck count
        let plans = f.stats.plans;
        let NavGoal::Way(back) = f.next(&mut grid, now + STEP_MS, pos, target, true) else {
            panic!("no back-up");
        };
        assert!(flat(back, pos) >= BACK_MIN && back[2] < 0.0, "{back:?}");
        assert_eq!(f.stuck(&mut grid, now + STEP_MS, pos), Stuck::No);
        assert_eq!(f.stats.plans, plans);
        // reached (or run out): a plan from where it stands
        f.next(&mut grid, now + 2 * STEP_MS, back, target, true);
        assert!(!f.backing());
        assert_eq!(f.stats.plans, plans + 1);
    }

    /// BOT-134: the watchdog's kick drops the way (the next call plans at once); a second kick at the same place is a
    /// trap, one far away is not.
    #[test]
    fn a_kick_replans_and_a_second_kick_at_the_same_place_is_a_trap() {
        let mut grid = NavGrid::new(walled(), BODY);
        let mut f = Follower::default();
        let (pos, target) = ([0.0, 40.0, 0.0], [0.0, 40.0, 400.0]);
        for _ in 0..40 {
            f.grow(&mut grid, pos, GROW_PROBES);
        }
        f.next(&mut grid, 0, pos, target, true);
        let plans = f.stats.plans;
        assert!(!f.kick(&mut grid, 100, pos));
        assert!(f.points().is_empty());
        f.next(&mut grid, 150, pos, target, true);
        assert_eq!(f.stats.plans, plans + 1);
        // far away: no trap
        assert!(!f.kick(&mut grid, 3200, [300.0, 40.0, 0.0]));
        // near the last one, in time: a trap
        assert!(f.kick(&mut grid, 6300, [320.0, 40.0, 10.0]));
        assert_eq!(f.stats.traps, 1);
        // long after: no trap
        assert!(!f.kick(&mut grid, 30_000, [320.0, 40.0, 10.0]));
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
