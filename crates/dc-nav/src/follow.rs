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
/// BOT-196: in the flight of a jump ([`JUMP_HOLD_MS`] after it) a top is reached only by a body this close under its
/// height (up on it). Before, [`TOP_LEVEL`] held in the flight too: a body whose flight topped out 3..6 under the rim of
/// a narrow ledge (Two Towers' ledge at (1771, -635, 706) beside the floor at -702, a rise of 68) had the top dropped as
/// reached, fell back to the floor, and the jump counted as no failure: the next plan jumped there again (nav_rom).
// DC-DEFAULT: BOT-196 (proposed) 2 units (`LEVEL`)
pub const FLIGHT_LEVEL: f32 = crate::LEVEL;
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
/// Waverider deck case its way, nav_rom.) BOT-162: the height cap is a jump and a step (106): a body standing a
/// little below the jump link's take-off gets up from there (Overdose's 12-unit dip before its stairs of ledges: a
/// rise of 87, nav_rom); before, a top higher than a jump got the face jump at any distance up to [`TOP_NEAR`] and with
/// no jump link onto it (rises up to 150 in the 2026-11-08 rooms, of which few got the body up).
// DC-DEFAULT: BOT-153 (proposed) 76 units, 72 units; BOT-162 (proposed) 106 units
pub const FACE_MAX_RISE: f32 = REACH;
pub const FACE_NEAR: f32 = 72.0;
/// BOT-197: no face jump from a body standing more than this above another take-off of a jump link onto the top that
/// lies nearer the top (the ground slopes down to the ledge's foot: the body walks on to the foot first and the face jump is taken there, the
/// stuck jump of [`JUMP_MS`] stays). Before, at Pump Jack's ledge outside team 1's spawn (top 114, foot 50, the ground
/// 57..61 a step back) the face jump was taken 70 units out on the slope, the body came down at the foot and jumped
/// again: two jumps a climb, 22 of a TDM room's 82 team-1 jumps (2026-11-10 room of the bot evaluation).
// DC-DEFAULT: BOT-197 (proposed) 4 units
pub const FACE_ABOVE: f32 = 4.0;
/// BOT-162: the jumps of one life are counted by the place the body jumped from (within [`PLACE_NEAR`] flat and
/// [`PLACE_RISE`] in height of the first), whatever top they were for: after [`PLACE_JUMPS`] jumps there within
/// [`PLACE_WINDOW_MS`] the place is spent for the rest of the life: no jump is taken from it again, the top ahead is
/// refused (BOT-145) and the plan goes round. Before, BOT-153 counted the jumps per top, the hunt goal moving every
/// 1..2 s gave the plan a new top beside the last one each time, and no count ran out: Overdose's stairs of ledges
/// outside team 2's spawn (8 jumps in 6 s, 250 of a TSV room's 580 jumps repeated in place), Two Towers (18 in 15 s),
/// Gray Hammer's crate (16 in 18 s). The height band keeps the tiers of a stairs of ledges apart (60..85 apart).
// DC-DEFAULT: BOT-162 (proposed) 96 units, 32 units, 5 jumps (the Warhead crate and Waverider's deck take four or five
// at one place, nav_rom; 4 cost the Warhead case its way), 30 s
pub const PLACE_NEAR: f32 = 96.0;
pub const PLACE_RISE: f32 = 32.0;
pub const PLACE_JUMPS: u32 = 5;
pub const PLACE_WINDOW_MS: u32 = 30_000;
/// BOT-162: a jump that got the body up is no failure: a body standing more than this above a place it jumped from
/// (higher than a jump's apex, 330^2 / 2200 = 49.5), within two [`PLACE_NEAR`] of it, clears that place's count (a body
/// that gets onto Warhead's crate and walks off its edge toward the walk above tries again: nav_rom).
// DC-DEFAULT: BOT-162 (proposed) 52 units
pub const UP_GAIN: f32 = 52.0;
/// BOT-162: a next point that is a top higher above the body than [`REACH`] (two jumps: a ledge on a ledge) and is no
/// ladder's top gets no jump (face or stuck): the body stands below the take-off (it fell off the step before, the
/// top's node was dropped as reached in the flight). Instead the follower plans again from where the body stands, at
/// most every [`OUT_OF_REACH_MS`] (the plan leads up the step first). Before, BOT-141's face jump took every top higher
/// than a jump as a ladder's: Waverider's deck at 158 above the body, jumped at in vain. Counting such a stop as a
/// failure at the top (BOT-145) refused the deck's ways and the walk to its case failed (nav_rom): not kept.
// DC-DEFAULT: BOT-162 (proposed) 152 units (2 x 76), 500 ms
pub const REACH: f32 = crate::JUMP_RISE * 2.0;
pub const OUT_OF_REACH_MS: u32 = 500;
/// BOT-162: no jump this soon after the last one (the rise of a jump, 330 / 1100 = 0.3 s, and a step; a body that
/// lands on a crate 69 high takes the next jump, onto the walk above, 0.5 s after the first: Warhead, nav_rom; a hold
/// of a whole flight, 1 s, cost that way).
// DC-DEFAULT: BOT-162 (proposed) 350 ms
pub const RISE_MS: u32 = 350;
/// BOT-162: a jump's take-off (a point followed by a top) counts as reached only by a body within this height of it
/// (else the body steers onto it first: a step up from a lower floor beside it). 12 units below a take-off the body
/// still got up a rise of 87 (Overdose, nav_rom); 17 below it, beside its step, it failed (2026-11-08 rooms).
// DC-DEFAULT: BOT-162 (proposed) 14 units
pub const TAKEOFF_LEVEL: f32 = 14.0;
/// BOT-186: a jump up onto a top at most [`APEX`] above the body is taken only where the body fits raised to the top's
/// height (one unit over it); elsewhere (under a slab's edge or a beam beside the ledge) the body first walks to the
/// take-off of a jump link onto that top (the node the grid's jump test raised the body at), for at most
/// [`TAKEOFF_MS`], until it stands within [`TAKEOFF_AT`] (flat) of it; with no such take-off known it does not jump
/// (the stuck count runs on to its re-plan). Before, a body that landed on Gray Hammer's crate stairs under the slab of
/// the walk above jumped at the next crate from there, struck its head (its flight topped out 37 above the take-off,
/// short of the 42 it needed) and fell back, often off the narrow crate to the floor (nav_rom). [`APEX`] is a jump's
/// apex (330^2 / 2200 = 49.5): a higher top is reached by the step at the end of the flight, the body moving on from
/// under what is over it (Two Towers' corner under the bridge: a top 62 up, nav_rom). Not kept: a walk to the take-off
/// also for a body more than 16 units off the line from the take-off to the top (more falls than it saved on Gray
/// Hammer's crate stairs), and the headroom test also 16 units ahead toward the top (more falls).
// DC-DEFAULT: BOT-186 (proposed) 1000 ms, 8 units, 50 units
pub const TAKEOFF_MS: u32 = 1000;
pub const TAKEOFF_AT: f32 = 8.0;
pub const APEX: f32 = 50.0;
/// BOT-166: a body this close (flat) to a ladder's foot, the way's next point the ladder's top, is at the ladder (the
/// foot lies a body's width and a margin beside the ladder's end, the body may hold the ladder anywhere along its
/// width).
// DC-DEFAULT: BOT-166 (proposed) one and a half lattice steps
pub const LADDER_AT: f32 = crate::STEP * 1.5;
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
/// BOT-191: failures at tops count together while they lie within [`crate::REFUSE_NEAR`] (flat) and a step in height
/// of the one counted first (the links up onto all of them go together, [`NavGrid`]'s `unlink_ups_onto`), within this
/// time of it; a jump at a top whose flight ended with the body not up there (the top still ahead, the body more than
/// [`TOP_LEVEL`] below it, [`JUMP_HOLD_MS`] after the jump or at the next jump) is such a failure. Before, the count
/// was kept per lattice node: a body at a parapet or a crate's edge jumped at the node in front of it, then at the
/// one beside it (the next plan's), and no count ran up (Rattlesnake's spawn exit parapet, Overdose's crate stack,
/// 4..5 jumps within 6 s).
// DC-DEFAULT: BOT-191 (proposed) 30 s
pub const FAIL_WINDOW_MS: u32 = 30_000;
/// BOT-196: the failures are counted at up to this many places at once (each place: the first top counted there and
/// the tops near it, BOT-191), and a goal changed or given up keeps them (a new life forgets them, as the places jumped
/// from, BOT-162). Before, one count was kept: a failure at another top started it again, and every goal given up or
/// gone for more than 1.5 s (a fight) cleared it with the way, so a body that failed at the tiers of a stairs of
/// ledges in turn (Overdose's tiers 70..80 apart outside team 2's spawn, Two Towers' beams round the pool) or fought
/// between its tries never ran a count up: 4..9 jumps at one place within 6 s, again and again.
// DC-DEFAULT: BOT-196 (proposed) 8 places
pub const FAIL_SPOTS: usize = 8;
/// BOT-196: after [`BURST_JUMPS`] jumps in a row near one another (each within [`BURST_NEAR`] of the place the body
/// would jump from now, each less than [`BURST_GAP_MS`] after the one before, the last less than that before now) the
/// next jump from there waits until [`BURST_GAP_MS`] after the last: the stuck count runs on meanwhile (its re-plan,
/// the refusal of the top, BOT-145) and the goal may move on. A ladder's top is not held. Before, a body at a stairs
/// of ledges whose tops it got onto and fell off again, or whose plan changed with the goal, jumped 4..9 times within
/// a few seconds there (Overdose's tiers outside team 2's spawn, Two Towers' beams round the pool): failures and
/// spent places (BOT-162, BOT-191) are counted per top and per place and are cleared by a jump that gets the body up.
// DC-DEFAULT: BOT-196 (proposed) 3 jumps, 96 units (3D), 6 s
pub const BURST_JUMPS: usize = 3;
pub const BURST_NEAR: f32 = 96.0;
pub const BURST_GAP_MS: u32 = 6000;

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

/// BOT-191 / BOT-196: a place failures at tops were counted at: the first top counted, how many, since when.
#[derive(Debug, Clone)]
struct FailSpot {
    at: [f32; 3],
    n: u32,
    first: u32,
}

/// BOT-162: a place the body jumped from in this life: where, the first jump counted, how many, spent for the life.
#[derive(Debug, Clone)]
struct JumpPlace {
    at: [f32; 3],
    first: u32,
    n: u32,
    spent: bool,
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
    /// BOT-162: jumps not taken from a spent place, and stops at a top out of the body's reach.
    pub place_spent: u64,
    pub out_of_reach: u64,
    /// BOT-197: jumps not taken on a ladder's rungs.
    pub rung_holds: u64,
    /// BOT-186: jumps put off for a walk to the take-off (no headroom where the body stood).
    pub takeoffs: u64,
    /// BOT-191: refusals made after jumps whose flight did not get the body up onto the top.
    pub jump_fails: u64,
    /// BOT-196: stuck counts whose jump was held after a run of jumps near one another.
    pub burst_holds: u64,
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
    /// BOT-145: the tops failures were counted at, and how many (BOT-191: tops near one count with it, since the
    /// first; BOT-196: at up to [`FAIL_SPOTS`] places at once, kept across goals).
    top_fails: Vec<FailSpot>,
    /// BOT-191: the top the body last jumped at and when (judged when the flight is over).
    pending: Option<([f32; 3], u32)>,
    /// BOT-162: the places jumped from in this life, and when a top out of reach was last met.
    places: Vec<JumpPlace>,
    /// BOT-196: the last jumps said (when, where), newest last.
    burst: Vec<(u32, [f32; 3], bool)>,
    reach_at: Option<u32>,
    /// BOT-186: the take-off the body walks to before its jump (no headroom where it stood), and until when.
    takeoff: Option<([f32; 3], u32)>,
    /// BOT-186: the top whose take-off walk ran out of time (its jump is taken from where the body stands).
    takeoff_spent: Option<[f32; 3]>,
    /// BOT-186: the height the body last jumped from, and when.
    jump_from: Option<(f32, u32)>,
    /// BOT-196: the last jump of the run a jump was last held after, and whether it is held (judged once a run).
    held_at: Option<(u32, bool)>,
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
            top_fails: Vec::new(),
            pending: None,
            places: Vec::new(),
            burst: Vec::new(),
            reach_at: None,
            takeoff: None,
            takeoff_spent: None,
            jump_from: None,
            held_at: None,
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

    /// Forgets the way and the stuck count (a new life, a new grid); the counters stay, and the places jumped from
    /// (BOT-162: a goal given up or changed keeps them; [`Self::forget_places`] at a new life), and the failures at
    /// tops (BOT-196, likewise).
    pub fn clear(&mut self) {
        let stats = std::mem::take(&mut self.stats);
        let refused = std::mem::take(&mut self.refused);
        let places = std::mem::take(&mut self.places);
        let top_fails = std::mem::take(&mut self.top_fails);
        let burst = std::mem::take(&mut self.burst);
        *self = Follower {
            stats,
            refused,
            places,
            top_fails,
            burst,
            ..Follower::default()
        };
    }

    /// BOT-162: forgets the places jumped from (a new life: the body was moved elsewhere); BOT-196: and the failures
    /// counted at tops and the last jumps.
    pub fn forget_places(&mut self) {
        self.places.clear();
        self.top_fails.clear();
        self.burst.clear();
    }

    /// BOT-162: the place the body at `pos` stands at, among those jumped from (counts older than
    /// [`PLACE_WINDOW_MS`] dropped unless spent).
    fn place_at(&mut self, now: u32, pos: [f32; 3]) -> Option<usize> {
        self.places
            .retain(|p| p.spent || now.wrapping_sub(p.first) <= PLACE_WINDOW_MS);
        self.places
            .iter()
            .position(|p| flat(p.at, pos) <= PLACE_NEAR && (p.at[1] - pos[1]).abs() <= PLACE_RISE)
    }

    /// BOT-162: whether the place the body stands at is spent (no more jumps from it in this life).
    fn place_spent(&mut self, now: u32, pos: [f32; 3]) -> bool {
        self.place_at(now, pos)
            .is_some_and(|i| self.places[i].spent || self.places[i].n >= PLACE_JUMPS)
    }

    /// BOT-162: a jump (or a stop at a top out of reach) at the place the body stands at counts there.
    fn count_place(&mut self, now: u32, pos: [f32; 3]) {
        match self.place_at(now, pos) {
            Some(i) => self.places[i].n += 1,
            None => {
                if self.places.len() >= 16 {
                    // the oldest place that is not spent goes first
                    let i = self.places.iter().position(|p| !p.spent).unwrap_or(0);
                    self.places.remove(i);
                }
                self.places.push(JumpPlace {
                    at: pos,
                    first: now,
                    n: 1,
                    spent: false,
                });
            }
        }
    }

    /// BOT-162: the next point is a top out of the body's reach by a jump (higher than [`crate::JUMP_RISE`] and no
    /// ladder's top).
    fn out_of_reach(&self, grid: &NavGrid, pos: [f32; 3]) -> bool {
        self.top
            .is_some_and(|(w, high)| high && w[1] - pos[1] > REACH && !grid.laddered_onto(w))
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
        self.refuse_top(grid, now, pos, w, force)
    }

    /// [`Self::refuse`] for the top `w` (BOT-191: also a top jumped at before, judged after the flight).
    fn refuse_top(
        &mut self,
        grid: &mut NavGrid,
        now: u32,
        pos: [f32; 3],
        w: [f32; 3],
        force: bool,
    ) -> bool {
        // BOT-191: tops near the first one counted count with it (the links up onto them go together); BOT-196: at
        // each of the places counted at in the window
        let near =
            |t: [f32; 3]| flat(t, w) <= crate::REFUSE_NEAR && (t[1] - w[1]).abs() <= crate::STEP_UP;
        self.top_fails
            .retain(|f| now.wrapping_sub(f.first) <= FAIL_WINDOW_MS);
        let i = match self.top_fails.iter().position(|f| near(f.at)) {
            Some(i) => {
                self.top_fails[i].n += 1;
                i
            }
            None => {
                if self.top_fails.len() >= FAIL_SPOTS {
                    self.top_fails.remove(0);
                }
                self.top_fails.push(FailSpot {
                    at: w,
                    n: 1,
                    first: now,
                });
                self.top_fails.len() - 1
            }
        };
        let (at, fails) = (self.top_fails[i].at, self.top_fails[i].n);
        // (a failure counted here is not judged again after the flight)
        if self.pending.is_some_and(|(t, _)| near(t)) {
            self.pending = None;
        }
        if !force && fails < TOP_FAILS {
            return false;
        }
        // (BOT-191: and onto the tops near the first one counted)
        let mut out = grid.unlink_ups_onto(w);
        if flat(at, w) >= 1.0 || (at[1] - w[1]).abs() >= 1.0 {
            out.extend(grid.unlink_ups_onto(at));
        }
        if out.is_empty() {
            return false;
        }
        if let Some(t) = self.want {
            let way = grid.route(pos, t);
            // BOT-191: a failure counted (not a top tried too long) keeps the ways up when the way round would not
            // reach the target while the way followed did (a stairway of tiers a jump high, climbed at the second or
            // third try: Waverider's deck)
            let lost = !force
                && self.whole
                && !way.is_some_and(|w| {
                    let e = w[w.len() - 1];
                    flat(e, t) <= AT_TARGET
                        && (e[1] - t[1]).abs() <= REACHED_RISE
                        && grid.in_sight(e, t)
                });
            if grid.last_way_closed() || lost {
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

    /// BOT-166: the height left to climb when the next point is a ladder's top more than a step above the body at `pos`
    /// and within [`TOP_NEAR`] of it (flat): the body climbs; 0 otherwise. Before, the stuck count measured the way
    /// left flat only, so a climb made no way, and a re-plan from the rungs (2 s) found no node near the body: a bot on
    /// Snow Valley's cellar ladder hung at its middle for the rest of the walk (nav_rom).
    fn climb_rise(&self, grid: &NavGrid, pos: [f32; 3]) -> f32 {
        self.pts
            .first()
            .filter(|w| {
                w[1] - pos[1] > crate::STEP_UP + 1.0
                    && flat(**w, pos) <= TOP_NEAR
                    && grid.laddered_onto(**w)
            })
            .map_or(0.0, |w| w[1] - pos[1])
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
        // BOT-186: in the flight of a jump a point is judged from the height the body jumped from (before: a top that
        // became the next point while the body rose toward it counted as no top, was reached in the flight and the
        // body steered on past the crate's rim)
        let in_flight = self
            .jump_from
            .is_some_and(|(_, t)| now.wrapping_sub(t) < JUMP_HOLD_MS);
        let base = match self.jump_from {
            Some((y, _)) if in_flight => pos[1].min(y),
            _ => pos[1],
        };
        let reached = |w: &[f32; 3]| {
            let high = match top {
                Some((t, h)) if t == *w => h,
                _ => w[1] - base > crate::STEP_UP + 1.0,
            };
            // BOT-196: in the flight a top is reached only by a body up on it (the step at the end of the flight
            // puts it there), not by one at the top of its flight a few units under the rim
            let rise = match (high, in_flight) {
                (true, true) => FLIGHT_LEVEL,
                (true, false) => TOP_LEVEL,
                _ => crate::STEP_UP + 1.0,
            };
            flat(*w, pos) <= REACHED && pos[1] - w[1] <= REACHED_RISE && w[1] - pos[1] <= rise
        };
        // the nodes reached are dropped, then the way ahead is pulled straight from here
        let before = self.pts.len();
        // BOT-162: a jump's take-off is reached only by a body level with it; BOT-166: a ladder's foot by a body on the
        // ladder at any height up to its top
        while self
            .pts
            .first()
            .is_some_and(|w| reached(w) && !off_takeoff(&self.pts, 0, pos))
            || at_ladder(grid, &self.pts, pos)
        {
            let w = self.pts.remove(0);
            // BOT-191: the top jumped at was reached: no failure
            if self.pending.is_some_and(|(t, _)| t == w) {
                self.pending = None;
            }
        }
        // BOT-191: a jump's flight is over and the body is not up on the top it jumped at: a failure there (the third
        // near it refuses the ways up, BOT-145)
        // (BOT-196: judged at a step that may plan; before, a call between two steps dropped it)
        if let Some((w, t)) = self.pending {
            if now.wrapping_sub(t) >= JUMP_HOLD_MS && may_plan {
                self.pending = None;
                if w[1] - pos[1] > TOP_LEVEL && self.refuse_top(grid, now, pos, w, false) {
                    self.stats.jump_fails += 1;
                }
            }
        }
        if self.pts.len() < before && self.pts.len() > 1 {
            let k = grid.shortcut(pos, &self.pts, AHEAD);
            self.pts.drain(..k);
        }
        let moved = self.goal.is_none_or(|g| flat(g, target) > TARGET_MOVED);
        // BOT-166: nor a timed re-plan during a climb (from the rungs the grid has no node near the body)
        let flight = self
            .jump_at
            .is_some_and(|t| now.wrapping_sub(t) < JUMP_HOLD_MS)
            || self.climb_rise(grid, pos) > 0.0;
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
        // BOT-162: a body up above a place it jumped from got up: that place's count is cleared
        for p in self.places.iter_mut().filter(|p| !p.spent) {
            if pos[1] - p.at[1] > UP_GAIN && flat(p.at, pos) <= PLACE_NEAR * 2.0 {
                p.n = 0;
            }
        }
        // BOT-141: whether the next point is a top, judged when it became the next one
        self.top = self.pts.first().map(|w| match top {
            Some((t, h)) if t == *w => (*w, h),
            _ => (*w, w[1] - base > crate::STEP_UP + 1.0),
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
        // BOT-162: a top near the body but out of its reach by a jump (the body stands below the take-off: it fell
        // off the step before): no jump; the follower plans again from where the body stands, and the stop counts as
        // a failure at that top (BOT-145) and at the place (a spent place refuses the top at once)
        let near_top = self.top.is_some_and(|(w, _)| flat(w, pos) <= TOP_NEAR);
        if near_top
            && !flight
            && may_plan
            && self.out_of_reach(grid, pos)
            && self
                .reach_at
                .is_none_or(|t| now.wrapping_sub(t) >= OUT_OF_REACH_MS)
        {
            self.reach_at = Some(now);
            self.plan(grid, now, pos, target);
            self.top = self
                .pts
                .first()
                .map(|w| (*w, w[1] - pos[1] > crate::STEP_UP + 1.0));
        }
        // BOT-186: the walk to the take-off of the jump ahead, while the top is the next point
        if let Some((t, until)) = self.takeoff {
            if later(until, now)
                && flat(t, pos) > TAKEOFF_AT
                && (t[1] - pos[1]).abs() <= crate::STEP_UP
                && self.top.is_some_and(|(_, high)| high)
            {
                return NavGoal::Way(t);
            }
            if !later(until, now) {
                self.takeoff_spent = self.top.map(|(w, _)| w);
            }
            self.takeoff = None;
        }
        match self.pts.first() {
            Some(w) => NavGoal::Way(rung(grid, *w, pos)),
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
    /// dropped and the next [`Self::next`] plans at once ([`Stuck::Replan`]). BOT-196: a jump is said only while
    /// `on_feet` (the body's jump would leave the ground now); else it is put off, uncounted, to a tick that is.
    pub fn stuck(&mut self, grid: &mut NavGrid, now: u32, pos: [f32; 3], on_feet: bool) -> Stuck {
        // BOT-133: a back-up is no walk along the way
        if self.backing() {
            self.last_walk = None;
            return Stuck::No;
        }
        let fresh = self
            .last_walk
            .is_none_or(|t| now.wrapping_sub(t) > WALK_GAP_MS);
        self.last_walk = Some(now);
        // BOT-166: on a ladder the height climbed is way made (the count starts again at the foot, where the height
        // to climb joins the way left)
        let rise = self.climb_rise(grid, pos);
        let left = self.left(pos) + rise;
        let moved = self.moved(pos)
            || (rise > 0.0
                && self
                    .best_pos
                    .is_some_and(|b| (b[1] - pos[1]).abs() >= MOVED));
        let climb_start = rise > 0.0 && left > self.best_left + DETOUR;
        if fresh || climb_start || (left < self.best_left - STUCK_MOVE && moved) {
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
                    // face within FACE_NEAR); BOT-162: within REACH of a body a little below the take-off
                    && ((w[1] - pos[1] >= FACE_RISE
                        && w[1] - pos[1] <= FACE_MAX_RISE
                        && flat(w, pos) <= FACE_NEAR
                        && grid.jumped_onto(w)
                        // BOT-197: not from above the jump link's take-off (a slope down to the ledge's foot)
                        && !grid.takeoff_below(w, pos, FACE_ABOVE))
                        // BOT-162: higher than a jump only a ladder's top (before: any top higher than a jump)
                        || (w[1] - pos[1] > crate::JUMP_RISE && grid.laddered_onto(w)))
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
        // BOT-162: the top of a ladder keeps its jumps (the rungs begin above the floor)
        let ladder = self.top.is_some_and(|(w, high)| {
            high && w[1] - pos[1] > crate::JUMP_RISE && grid.laddered_onto(w)
        });
        // BOT-162: no jump in the rise of the last one (at the top of its flight the body moves level between two
        // steps and passed the on-its-feet test: two to three jumps a climb, 15..200 ms apart, in the rooms)
        let in_flight = self.jump_at.is_some_and(|t| now.wrapping_sub(t) < RISE_MS);
        if in_flight {
            return Stuck::No;
        }
        if (stuck >= JUMP_MS || at_face) && !self.jumped && !spent && !ladder {
            // BOT-162: no jump at a top out of the body's reach (the follower plans again from here, `next`)
            if self.out_of_reach(grid, pos) {
                self.jumped = true;
                self.stats.out_of_reach += 1;
                return Stuck::No;
            }
            // BOT-162: no jump from a spent place: the top ahead is refused (the plan goes round); with no way round
            // the stuck count runs on to its re-plan (and the session's goal given up, BOT-145)
            if self.place_spent(now, pos) {
                self.jumped = true;
                self.stats.place_spent += 1;
                if let Some(i) = self.place_at(now, pos) {
                    self.places[i].spent = true;
                }
                self.refuse(grid, now, pos, true);
                return Stuck::No;
            }
        }
        // BOT-197: no jump on a ladder's rungs (the body more than a step above the ladder's foot): the climb goes on.
        // Before, each climb took a second jump 0.4..0.5 s after the one at the foot, on the rungs (Gray Hammer's
        // trench ladder, Two Towers' ladder by team 2's spawn: 2 stuck jumps a climb in the 2026-11-10 rooms)
        let on_rungs = ladder
            && self.top.is_some_and(|(w, _)| {
                grid.ladder_topped(w)
                    .is_some_and(|(f, _)| pos[1] > f[1] + crate::STEP_UP)
            });
        if on_rungs && (stuck >= JUMP_MS || at_face) && !self.jumped {
            self.jumped = true;
            self.stats.rung_holds += 1;
            return Stuck::No;
        }
        // BOT-186: no jump up where the body does not fit raised to the top's height: it walks to the jump link's
        // take-off first (the jump is taken from there)
        if (stuck >= JUMP_MS || at_face) && !self.jumped && !spent && !ladder {
            if let Some((w, true)) = self.top.filter(|t| self.takeoff_spent != Some(t.0)) {
                let rise = w[1] - pos[1];
                let headroom = rise > APEX || grid.fits([pos[0], w[1] + 1.0, pos[2]]);
                let t = grid.jump_takeoff(w, pos);
                match t.filter(|t| !headroom && flat(*t, pos) > TAKEOFF_AT) {
                    Some(t) => {
                        if self.takeoff.is_none() {
                            self.stats.takeoffs += 1;
                        }
                        self.takeoff = Some((t, now.wrapping_add(TAKEOFF_MS)));
                        return Stuck::No;
                    }
                    // no take-off known and no headroom: no jump (the stuck count runs on to its re-plan)
                    None if !headroom => {
                        self.jumped = true;
                        return Stuck::No;
                    }
                    None => {}
                }
            }
        }
        // BOT-196: not while the jump would not leave the ground (on a rim the body is not standing on, crouched,
        // in a flight): the jump is put off, and counts nowhere (before: 11 % of the stuck jumps of an Overdose room
        // got the body no higher, each counted as a jump at the place and the top)
        if (stuck >= JUMP_MS || at_face) && !self.jumped && !spent && on_feet {
            // BOT-191: a jump again while the last one is not judged yet: the body is not up on the top it jumped at
            // (a failure there; the third near it refuses the ways up and no jump is taken)
            if let Some((w, _)) = self.pending.take() {
                if w[1] - pos[1] > TOP_LEVEL && self.refuse_top(grid, now, pos, w, false) {
                    self.stats.jump_fails += 1;
                    self.jumped = true;
                    return Stuck::No;
                }
            }
            // BOT-196: no fourth jump in a row from about here until the last one is BURST_GAP_MS old
            // (not when the top ahead is the only way to the target: its tries go on, as BOT-191 keeps its ways)
            if !ladder && self.in_burst(now, pos) {
                let last = self.burst.last().map(|b| b.0);
                if self.held_at.map(|h| h.0) != last {
                    let hold = !self.only_way_up(grid, pos);
                    self.held_at = last.map(|t| (t, hold));
                    if hold {
                        self.stats.burst_holds += 1;
                    }
                }
                if self.held_at.is_some_and(|h| h.1) {
                    return Stuck::No;
                }
            }
            if !ladder {
                if self.burst.len() >= 8 {
                    self.burst.remove(0);
                }
                self.burst.push((now, pos, true));
            }
            // (BOT-153: every jump at a top counts toward FACE_SPENT, the stuck jump of JUMP_MS too)
            if let Some((w, true)) = self.top {
                self.face_jumps = Some((w, tries + 1));
                // BOT-191: judged when the flight is over (not at a ladder: the body climbs on)
                if !ladder {
                    self.pending = Some((w, now));
                }
            }
            // BOT-162: and toward the place's count (not at a ladder)
            if !ladder {
                self.count_place(now, pos);
            }
            self.jumped = true;
            self.jump_at = Some(now);
            self.jump_from = Some((pos[1], now));
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

    /// BOT-196: whether the top ahead is the only way to the target: the way followed reaches the target's place and
    /// a plan from `pos` without the ways up onto that top would not (the links go back at once).
    fn only_way_up(&mut self, grid: &mut NavGrid, pos: [f32; 3]) -> bool {
        let (Some((w, true)), Some(t)) = (self.top, self.want) else {
            return false;
        };
        if !self.whole {
            return false;
        }
        let out = grid.unlink_ups_onto(w);
        if out.is_empty() {
            return false;
        }
        let way = grid.route(pos, t);
        let round = way.is_some_and(|w| {
            let e = w[w.len() - 1];
            flat(e, t) <= AT_TARGET && (e[1] - t[1]).abs() <= REACHED_RISE && grid.in_sight(e, t)
        });
        for l in out {
            grid.relink(l);
        }
        !round
    }

    /// BOT-196: the jump said last was not taken (the bot's jump test refused its flight, BOT-134): it counts toward
    /// a run of jumps only with other jumps not taken (a body that tries a ledge from several places along it, its
    /// jumps there refused now and then, is not hopping; one whose every try is refused stands saying jump).
    pub fn jump_not_taken(&mut self) {
        if let Some(b) = self.burst.last_mut() {
            b.2 = false;
        }
    }

    /// BOT-196: whether the last jumps were a run near `pos` (each within [`BURST_NEAR`] of it, less than
    /// [`BURST_GAP_MS`] apart and the last less than that before `now`): [`BURST_JUMPS`] taken ones, or as many not
    /// taken in a row.
    fn in_burst(&self, now: u32, pos: [f32; 3]) -> bool {
        let d3 = |a: [f32; 3]| {
            ((a[0] - pos[0]).powi(2) + (a[1] - pos[1]).powi(2) + (a[2] - pos[2]).powi(2)).sqrt()
        };
        let mut t = now;
        let (mut taken, mut refused, mut refused_run) = (0, 0, true);
        for (at, p, took) in self.burst.iter().rev() {
            if t.wrapping_sub(*at) >= BURST_GAP_MS || d3(*p) > BURST_NEAR {
                break;
            }
            if *took {
                taken += 1;
                refused_run = false;
            } else if refused_run {
                refused += 1;
            }
            t = *at;
        }
        taken >= BURST_JUMPS || refused >= BURST_JUMPS
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

/// BOT-162: whether `pts[i]` is the take-off of a jump up (the point after it lies more than a step above it, and more
/// than a jump above the body at `pos`) and the body is not level with it ([`TAKEOFF_LEVEL`]): it has not reached it
/// (used while the way is followed; a new plan still drops the points the body stands at). Before, a body on a lower
/// floor beside the take-off (Overdose's step at 250 beside the take-off at 265 under the ledge at 335) counted it as
/// reached (24 units flat, a step in height), the ledge 85 above the body became the next point, and the face jump
/// from there failed again and again (rises 86..92, 2..8 jumps a pass, every team-2 bot). Applied to every take-off
/// (a top within a jump of the body too) and in the plan as well, it got the body onto a part of Waverider's deck the
/// grid had no way on from yet (the lattice's only whole way from there: a loop of 140 lattice steps found after 35 s;
/// growth round the body or from the goal's place did not shorten that) and the walk to the deck case failed
/// (nav_rom).
fn off_takeoff(pts: &[[f32; 3]], i: usize, pos: [f32; 3]) -> bool {
    let w = pts[i];
    pts.get(i + 1)
        .is_some_and(|n| n[1] - w[1] > crate::STEP_UP + 1.0 && n[1] - pos[1] > crate::JUMP_RISE)
        && (pos[1] - w[1]).abs() > TAKEOFF_LEVEL
}

/// BOT-166: whether the way's next point `pts[0]` is a ladder's foot with the ladder's top after it, and the body at
/// `pos` is at that ladder: within [`LADDER_AT`] (flat) of the foot, no more than [`REACHED_RISE`] below it and not
/// above the top. Such a foot is reached: the body climbs from where it holds the ladder. Before, BOT-162's take-off
/// rule took the foot for a jump's take-off (the top 790 above it) and a body that had dropped onto Snow Valley's
/// shaft ladder up to S&D site B (bobbing 25..50 above the foot, 30 from it) never reached it: the follower gave the
/// ladder up after 6 s and its way round to the site took 31 s (2026-11-08 rooms: every bomb there detonated).
/// BOT-166: where a body steers for the way's next point `w`: while `w` is a ladder's top more than a step above the
/// body and the body is within [`TOP_NEAR`] of it (flat), the ladder's middle (halfway between its foot and top places,
/// flat) at the top's height, so the body climbs in the middle of the ladder and steps off only up at the top; else `w`.
/// Before, the body steered at the top place beyond the ladder from the first rung, pressed against that side of the
/// ladder and hung under the floor slab beside a hatch (Snow Valley's cellar ladder up to S&D site A: 105 below its
/// top for the rest of the walk, nav_rom).
fn rung(grid: &NavGrid, w: [f32; 3], pos: [f32; 3]) -> [f32; 3] {
    if w[1] - pos[1] <= crate::STEP_UP || flat(w, pos) > TOP_NEAR {
        return w;
    }
    grid.ladder_topped(w)
        .map_or(w, |(f, t)| [(f[0] + t[0]) * 0.5, t[1], (f[2] + t[2]) * 0.5])
}

fn at_ladder(grid: &NavGrid, pts: &[[f32; 3]], pos: [f32; 3]) -> bool {
    let (Some(w), Some(n)) = (pts.first(), pts.get(1)) else {
        return false;
    };
    grid.ladder_ends(*w, *n).is_some_and(|(f, t)| {
        flat(f, pos) <= LADDER_AT && pos[1] >= f[1] - REACHED_RISE && pos[1] <= t[1]
    })
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
            f.stuck(grid, now, pos, true);
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
            let s = f.stuck(&mut grid, now, pos, true);
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
        assert_eq!(f.stuck(&mut grid, 3050, pos, true), Stuck::No);
        assert_eq!(f.stuck(&mut grid, 5000, pos, true), Stuck::No);
        assert_eq!(f.stuck(&mut grid, 5040, pos, true), Stuck::No);
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
        assert_eq!(f.stuck(&mut grid, 1900, pos, true), Stuck::No);
        assert_eq!(f.stuck(&mut grid, 1950, pos, true), Stuck::Jump);
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
            seen = g.stuck(&mut grid, later + k * STEP_MS, pos, true);
            if matches!(seen, Stuck::Replan | Stuck::Trap) {
                break;
            }
        }
        assert_eq!(seen, Stuck::Replan);
        assert_eq!(g.stats.refusals, 0);
    }

    /// BOT-191: a body that never gets up onto a low wall (its jumps fall back) while its goal flips between two places
    /// beyond it (a new top beside the last one at each plan) has the ways up onto those tops refused after `TOP_FAILS`
    /// failures there in all (two jumps whose flight ended below the top and a stuck re-plan) and walks round the wall's
    /// end. Before, each top kept its own count and none ran up.
    #[test]
    fn failed_jumps_at_tops_near_one_another_count_together() {
        let w = Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-400.0, -64.0, -200.0], [400.0, 0.0, 400.0]),
                solid_box([-160.0, 0.0, 100.0], [160.0, 60.0, 140.0]),
            ],
            &[],
        ));
        let mut grid = NavGrid::new(w, BODY);
        let mut f = Follower::default();
        let pos = [0.0, 40.0, 80.0];
        let goals = [[-40.0, 40.0, 220.0], [40.0, 40.0, 220.0]];
        grid.seed_grown(pos);
        while grid.growing() {
            grid.expand(2_000, None);
        }
        let mut jumps = 0;
        let mut tops = Vec::new();
        let mut now = 0;
        while f.stats.refusals == 0 && now < 20_000 {
            now += STEP_MS;
            let target = goals[((now / 1500) % 2) as usize];
            if let NavGoal::Way(w) = f.next(&mut grid, now, pos, target, true) {
                if w[1] > 90.0 && !tops.contains(&w) {
                    tops.push(w);
                }
            }
            if f.stuck(&mut grid, now, pos, true) == Stuck::Jump {
                jumps += 1;
            }
        }
        assert!(tops.len() >= 2, "the goal flips give other tops: {tops:?}");
        assert_eq!(f.stats.refusals, 1, "{:?}", f.stats);
        assert!(jumps < TOP_FAILS, "{jumps} jumps, {:?}", f.stats);
        // the way goes round the wall's end now
        let next = f.next(&mut grid, now + STEP_MS, pos, goals[0], true);
        assert!(matches!(next, NavGoal::Way(w) if w[1] < 50.0), "{next:?}");
    }

    /// BOT-196: failures at two places along a ledge (more than [`crate::REFUSE_NEAR`] apart) taken in turn count at
    /// each place, and a way cleared between them (a goal given up, a fight) keeps the counts: the third failure at
    /// one place refuses its ways up. Before, one count was kept and each failure elsewhere or each clear began it
    /// again (no refusal at all).
    #[test]
    fn failures_count_at_each_place_and_across_cleared_ways() {
        let w = Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-400.0, -64.0, -200.0], [400.0, 0.0, 400.0]),
                solid_box([-300.0, 0.0, 100.0], [300.0, 60.0, 140.0]),
            ],
            &[],
        ));
        let mut grid = NavGrid::new(w, BODY);
        let mut f = Follower::default();
        let pos = [0.0, 40.0, 80.0];
        grid.seed_grown(pos);
        while grid.growing() {
            grid.expand(2_000, None);
        }
        // a top of a jump link near each end of the ledge
        let top = |x: f32| {
            grid.links_near([x, 100.0, 120.0], 72.0)
                .into_iter()
                .map(|(p, _)| p)
                .find(|p| p[1] > 90.0 && grid.jumped_onto(*p))
                .expect("a jump link onto the ledge")
        };
        let (a, b) = (top(-150.0), top(150.0));
        assert!(flat(a, b) > crate::REFUSE_NEAR * 2.0, "{a:?} {b:?}");
        let mut refused = Vec::new();
        for (i, t) in [a, b, a, b, a].into_iter().enumerate() {
            let now = 1000 + i as u32 * 2000;
            refused.push(f.refuse_top(&mut grid, now, pos, t, false));
            // a fight between the tries: the way and the stuck count go, the failures stay
            f.clear();
        }
        assert_eq!(refused, [false, false, false, false, true], "{:?}", f.stats);
        assert_eq!(f.stats.refusals, 1);
        assert!(!f.refused.is_empty());
        // a new life forgets them
        f.forget_places();
        assert!(!f.refuse_top(&mut grid, 12_000, pos, b, false));
    }

    /// BOT-196: a body at a ledge's face that makes no way jumps at most `BURST_JUMPS` times in a row there; the next
    /// jump waits until `BURST_GAP_MS` after the last (the jumps at a ladder's top are not held), and a jump not said
    /// while the body is off its feet is said at the first tick it is on them.
    #[test]
    fn a_run_of_jumps_at_one_place_is_held_and_jumps_wait_for_the_feet() {
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
        let mut jumps = Vec::new();
        let mut now = 0;
        while now < 9_000 {
            now += STEP_MS;
            f.next(&mut grid, now, pos, target, true);
            // off its feet for the first 3 s: no jump is said
            let feet = now >= 3_000;
            if f.stuck(&mut grid, now, pos, feet) == Stuck::Jump {
                jumps.push(now);
                // (the way stays: each jump starts a stuck count again, as a walk would)
                f.last_walk = None;
            }
        }
        assert!(jumps.first().is_some_and(|t| *t == 3_000), "{jumps:?}");
        assert!(jumps.len() >= BURST_JUMPS, "{jumps:?} {:?}", f.stats);
        let run = &jumps[..BURST_JUMPS];
        if let Some(next) = jumps.get(BURST_JUMPS) {
            assert!(next - run[BURST_JUMPS - 1] >= BURST_GAP_MS, "{jumps:?}");
        }
        assert!(f.stats.burst_holds >= 1, "{:?}", f.stats);
        // far from the run, or after the gap, no hold
        assert!(f.in_burst(run[BURST_JUMPS - 1] + 100, pos));
        assert!(!f.in_burst(run[BURST_JUMPS - 1] + 100, [500.0, 40.0, 80.0]));
        assert!(!f.in_burst(run[BURST_JUMPS - 1] + BURST_GAP_MS, pos));
        // jumps the bot's jump test refused: a run of them alone is held, mixed with taken ones not
        let mut g = Follower::default();
        for t in [100, 600, 1100] {
            g.burst.push((t, pos, false));
        }
        assert!(g.in_burst(1500, pos));
        g.burst[0].2 = true;
        assert!(!g.in_burst(1500, pos));
    }

    /// BOT-162: a body that never gets up a ledge while its goal flips between two places along the ledge (a new top
    /// beside the last one at each plan) jumps at most `PLACE_JUMPS` times from its place; then the place is spent
    /// and the top ahead is refused instead.
    #[test]
    fn jumps_from_one_place_are_counted_across_tops_and_run_out() {
        let w = Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-2000.0, -64.0, -2000.0], [2000.0, 0.0, 2000.0]),
                solid_box([-2000.0, 0.0, 100.0], [2000.0, 60.0, 2000.0]),
            ],
            &[],
        ));
        let mut grid = NavGrid::new(w, BODY);
        let mut f = Follower::default();
        let pos = [0.0, 40.0, 80.0];
        let goals = [[-150.0, 100.0, 400.0], [150.0, 100.0, 400.0]];
        grid.seed_grown(pos);
        for _ in 0..80 {
            f.grow(&mut grid, pos, GROW_PROBES);
        }
        let mut jumps = 0;
        let mut tops = Vec::new();
        for s in 0..(20_000 / STEP_MS) {
            let now = s * STEP_MS;
            let target = goals[((now / 1500) % 2) as usize];
            if let NavGoal::Way(w) = f.next(&mut grid, now, pos, target, true) {
                if w[1] > 90.0 && !tops.contains(&w) {
                    tops.push(w);
                }
            }
            if f.stuck(&mut grid, now, pos, true) == Stuck::Jump {
                jumps += 1;
            }
        }
        assert!(tops.len() >= 2, "the goal flips give other tops: {tops:?}");
        assert!(jumps <= PLACE_JUMPS, "{jumps} jumps, {:?}", f.stats);
        assert!(f.stats.place_spent >= 1, "{:?}", f.stats);
        // a new life forgets the place
        f.forget_places();
        assert!(!f.place_spent(20_000, pos));
    }

    /// BOT-162: a next point that is a top out of the body's reach (more than two jumps above it: the body fell off
    /// the step before it) is no jump: the follower plans again from where the body stands, and the way then leads up
    /// the step first; the stuck jump is not taken at such a top.
    #[test]
    fn a_top_out_of_reach_is_planned_round_not_jumped_at() {
        let w = Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-2000.0, -64.0, -2000.0], [2000.0, 0.0, 2000.0]),
                // a step 60 high and 48 deep, then a ledge 130 high
                solid_box([-2000.0, 0.0, 76.0], [2000.0, 60.0, 124.0]),
                solid_box([-2000.0, 0.0, 124.0], [2000.0, 130.0, 2000.0]),
            ],
            &[],
        ));
        let mut grid = NavGrid::new(w, BODY);
        let mut f = Follower::default();
        let (floor, step) = ([0.0, 40.0, 56.0], [0.0, 100.0, 96.0]);
        let target = [0.0, 170.0, 500.0];
        grid.seed_grown(floor);
        grid.seed_grown(step);
        for _ in 0..40 {
            grid.expand_between(GROW_PROBES * GROW_BOOST, step, target);
        }
        let NavGoal::Way(top) = f.next(&mut grid, 0, step, target, true) else {
            panic!("no way from the step");
        };
        assert!(top[1] > 160.0, "from the step the ledge is next: {top:?}");
        // the body falls back below the step (the test sets it 40 lower than the floor's place, as a body in a dip
        // beside a take-off stands): the top lies 170 above it (within REPLAN_MS of the plan: no plan on the clock)
        let low = [floor[0], floor[1] - 40.0, floor[2]];
        assert!(top[1] - low[1] > REACH);
        let next = f.next(&mut grid, 500, low, target, true);
        assert!(
            matches!(next, NavGoal::Way(w) if w[1] - low[1] <= REACH),
            "{next:?}"
        );
        // with the top out of reach still ahead, the stuck jump is not taken
        let mut g = Follower {
            pts: vec![top],
            top: Some((top, true)),
            ..Default::default()
        };
        for k in 0..(JUMP_MS / STEP_MS + 2) {
            assert_ne!(
                g.stuck(&mut grid, 500 + k * STEP_MS, low, true),
                Stuck::Jump
            );
        }
        assert_eq!(g.stats.out_of_reach, 1, "{:?}", g.stats);
    }

    /// BOT-162: a jump's take-off is reached only by a body level with it: a body on a floor 17 below it, beside it,
    /// steers onto it first (the ledge above is not the next point yet); one level with it goes on to the ledge.
    #[test]
    fn a_take_off_is_reached_only_level_with_it() {
        let mut grid = NavGrid::new(walled(), BODY);
        let (takeoff, top) = ([0.0, 265.0, 0.0], [0.0, 335.0, 48.0]);
        let mut f = Follower {
            pts: vec![takeoff, top],
            ..Default::default()
        };
        let below = [10.0, 248.0, 0.0];
        assert!(
            matches!(f.next(&mut grid, 100, below, top, false), NavGoal::Way(w) if w == takeoff)
        );
        let level = [10.0, 262.0, 0.0];
        assert!(matches!(f.next(&mut grid, 150, level, top, false), NavGoal::Way(w) if w == top));
        // a point that is no take-off is reached as before
        let mut g = Follower {
            pts: vec![takeoff, [0.0, 265.0, 96.0]],
            ..Default::default()
        };
        assert!(
            matches!(g.next(&mut grid, 100, below, top, false), NavGoal::Way(w) if w[2] == 96.0)
        );
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
                    f.stuck(&mut grid, now, pos, true);
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
                let s = f.stuck(&mut grid, now, pos, true);
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
                let s = f.stuck(&mut grid, now, pos, true);
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
        assert_eq!(f.stuck(&mut grid, now + STEP_MS, pos, true), Stuck::No);
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

    /// BOT-166: a body on the rungs above a ladder's foot has reached the foot (no jump's take-off), steers at the
    /// ladder's middle at the top's height until it is up, and its climb is way made for the stuck count (no re-plan
    /// in a climb of 4 s).
    #[test]
    fn a_body_on_the_rungs_climbs_on_without_a_replan() {
        let w = Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-2000.0, -64.0, -2000.0], [2000.0, 0.0, 2000.0]),
                solid_box([-2000.0, 0.0, 200.0], [2000.0, 200.0, 2000.0]),
            ],
            &[],
        ));
        let ladder = dc_sim::ConvexVolume::from_box(
            dc_sim::V3::new(-30.0, 0.0, 190.0),
            dc_sim::V3::new(30.0, 200.0, 200.0),
        );
        let mut grid = NavGrid::new(w, BODY).with_ladders(&[ladder]);
        let (low, high) = ([0.0f32, 40.0, 0.0], [0.0f32, 240.0, 600.0]);
        grid.seed(low);
        while grid.growing() && grid.probes < 200_000 {
            grid.expand(2_000, Some(high));
        }
        let (foot, top) = grid.ladder_topped([0.0, 240.0, 230.0]).expect("the ladder");
        assert!(top[1] - foot[1] > crate::JUMP_RISE);
        let mid = [(foot[0] + top[0]) * 0.5, top[1], (foot[2] + top[2]) * 0.5];
        // on the rungs 60 above the foot: the foot is passed, the body steers at the ladder's middle
        let mut f = Follower {
            pts: vec![foot, top, high],
            goal: Some(high),
            whole: true,
            planned_at: Some(0),
            ..Default::default()
        };
        let on = [foot[0], foot[1] + 60.0, foot[2] + 20.0];
        assert_eq!(f.next(&mut grid, 100, on, high, false), NavGoal::Way(mid));
        assert_eq!(f.points()[0], top);
        // a climb of 4 s at 40 units a second: no re-plan, no jump
        for k in 0..80u32 {
            let p = [on[0], on[1] + 2.0 * k as f32, on[2]];
            let now = 100 + k * STEP_MS;
            assert!(matches!(
                f.next(&mut grid, now, p, high, false),
                NavGoal::Way(_)
            ));
            assert_eq!(f.stuck(&mut grid, now, p, true), Stuck::No, "at {k}");
        }
        // up at the top's height it steers off the ladder (at the top place or past it)
        let up = [mid[0], top[1] - 10.0, mid[2]];
        let next = f.next(&mut grid, 5000, up, high, false);
        assert!(
            matches!(next, NavGoal::Way(w) if w != mid && w[2] > mid[2]),
            "{next:?}"
        );
    }
}
