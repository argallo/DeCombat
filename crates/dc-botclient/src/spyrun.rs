//! Spy Hunt for a hunting bot (mode class 9): where the intel cases lie (type-0x17 kind-3 objects of the object
//! stream), where the uplink is (the type-0x37 zone of kind 8 and class 9), who holds how many cases (server 0xcb
//! sub 0 `[value][team][pid]` for every player) of how many (sub 1), whether the agent's stage runs (sub 5), and the
//! walk goal: the uplink while the bot holds every case; the agent while another player is the agent; else the case
//! the team's spread gives the bot (cases it made no way to are left alone for a while), and without one the spy
//! holding the most cases. A DeCombat bot behaviour, not a 2009 rule.
// SPEC: GAME_MODES_2009.md §4.3 (zone record order, kind 8), §4.4 (kind 3), §5.6 (0xcb sub 0 `[value][team][pid]`,
// sub 1 `[total][dropped][spies]`); GAME_MESSAGES_2009_S2C.md §8.2 (type-0x17 blob head)
// DC-DEFAULT: BOT-75 (proposed), BOT-127 (proposed), BOT-137 (proposed), BOT-140 (proposed), BOT-151 (proposed),
// BOT-152 (proposed), BOT-155 (proposed), BOT-163 (proposed), BOT-164 (proposed)

use crate::objects::ObjectTable;
use dc_wire::udp::BitWriter;
use std::collections::BTreeMap;

/// `GS_BOUNTY_MODE`.
pub const GS_BOUNTY_MODE: u8 = 0xcb;
/// Shell type of a pickup blob, the kind of an intel case; the zone type and the use-target kind.
pub const PICKUP_TYPE: u8 = 0x17;
pub const PICKUP_KIND_INTEL: u32 = 3;
pub const ZONE_TYPE: u8 = 0x37;
pub const ZONE_KIND_USE_TARGET: u32 = 8;
pub const SPY_CLASS: u8 = 9;
/// The team of a spy (and the agent) in 0xcb sub 0, and of a hunter.
pub const SPY_TEAM: u8 = 2;
pub const HUNTER_TEAM: u8 = 1;
/// BOT-127: a spy out of sight is chased to where he was last seen (within [`LAST_SEEN_MS`]) until the bot gets
/// within [`LAST_SEEN_REACHED`] units (flat) of that place.
pub const LAST_SEEN_MS: u32 = 30_000;
pub const LAST_SEEN_REACHED: f32 = 200.0;

/// Height differences count this many times in the choice of a case.
// DC-DEFAULT: BOT-75 (proposed)
pub const HEIGHT_WEIGHT: f32 = 6.0;

/// BOT-127: a case the bot gets no [`STALL_GAIN`] units (flat) closer to in [`STALL_MS`], or stands within [`NEAR`]
/// units of for [`NEAR_MS`] without taking it, is out of its reach for [`BLOCK_MS`]; a case it has not walked to for
/// [`FORGET_MS`] starts its count afresh.
// DC-DEFAULT: BOT-127 (proposed)
pub const STALL_GAIN: f32 = 64.0;
pub const STALL_MS: u32 = 10_000;
pub const NEAR: f32 = 160.0;
pub const NEAR_MS: u32 = 5_000;
pub const BLOCK_MS: u32 = 60_000;
pub const FORGET_MS: u32 = 3_000;
/// BOT-141: the stall rule measures the way left along the bot's planned way (a detour round by stairs gets no
/// nearer in a straight line for long stretches), and the near rule counts only where the bot stands within
/// [`NEAR_RISE`] in height (a bot under a case on a crate stood "near" it); while the bot's grid is still growing a
/// way to the case the count waits, at most [`WALK_MAX_MS`] in all for one case; a new way more than
/// [`STALL_GAIN`] longer than the least so far starts the count afresh.
// DC-DEFAULT: BOT-141 (proposed) 96 units (DSV-231's touch height), 90 s
pub const NEAR_RISE: f32 = 96.0;
pub const WALK_MAX_MS: u32 = 90_000;
/// BOT-127: the case the bot walks to costs this share of its distance in the spread (it keeps its case while
/// another player is about as near).
pub const KEEP: f32 = 0.8;

/// BOT-137: a case is a goal only where the bot's grid (grown in full) has a node within [`REACH`] units (flat) and
/// [`REACH_RISE`] in height of it (the grid's climb), out of danger (in a kill volume, over the void).
// DC-DEFAULT: BOT-137 (proposed)
pub const REACH: f32 = 128.0;
pub const REACH_RISE: f32 = 60.0;
/// BOT-137: a case with no floor this far below its point lies over the void (tested with a body of this half
/// size).
pub const FLOOR_BELOW: f32 = 400.0;
pub const FLOOR_PROBE: f32 = 4.0;
/// BOT-137: a holder of `n` cases costs 1 / (1 + n) of its distance in the spread, so loose cases go to the
/// players who already hold some (the set grows in one hand instead of a new holder per case).
pub const HOLDER_PULL: f32 = 1.0;
/// BOT-137 (amended by BOT-140): a holder of [`HOLDER_CAUTIOUS`] cases or more (not yet the agent) whose nearest
/// hunter is within [`HOLDER_KEEP_AWAY`] units (flat) falls back from him, unless its case lies within
/// [`HOLDER_CASE_NEAR`] or that hunter is farther from the case than the holder (not in its way); a holder of fewer
/// collects and fights.
// DC-DEFAULT: BOT-137 (proposed), BOT-140 (proposed)
pub const HOLDER_KEEP_AWAY: f32 = 400.0;
pub const HOLDER_CASE_NEAR: f32 = 600.0;
pub const HOLDER_CAUTIOUS: u32 = 3;
/// BOT-140: a holder (not the agent) is chased by at most this many hunters, the nearest ones.
// DC-DEFAULT: BOT-140 (proposed)
pub const CHASERS: usize = 2;

/// BOT-151: the uplink box opens this long after the agent appears (the world's agent trigger unlocks it with a
/// delay: 40.0 s on Sand Hog, Waverider and Power Surge in the server logs of `out_boteval/matrix1`); a bot standing
/// in it earlier uploads nothing and draws every hunter.
// DC-DEFAULT: BOT-151 (proposed)
pub const STATION_OPEN_MS: u32 = 40_000;
/// BOT-151: the agent leaves its waiting place for the uplink when the way there (flat distance x [`AGENT_WAY`] at
/// [`AGENT_SPEED`] units a second) ends [`AGENT_EARLY_MS`] before the box opens.
pub const AGENT_SPEED: f32 = 219.0;
pub const AGENT_WAY: f32 = 1.4;
pub const AGENT_EARLY_MS: u32 = 3_000;
/// BOT-151: the agent waits at a grid place [`LURK_MIN`]..[`LURK_MAX`] units (flat) from the uplink, within
/// [`LURK_RISE`] of its height, the one farthest from the hunters it knows of (capped at [`LURK_SAFE`]) less
/// [`LURK_WALK`] of the walk to it, of the places it reaches before every hunter; kept at least [`LURK_HOLD_MS`],
/// picked again when a hunter comes within [`LURK_THREAT`] of it.
pub const LURK_MIN: f32 = 700.0;
pub const LURK_MAX: f32 = 2000.0;
pub const LURK_RISE: f32 = 300.0;
pub const LURK_SAFE: f32 = 2000.0;
pub const LURK_WALK: f32 = 0.3;
pub const LURK_HOLD_MS: u32 = 2_000;
pub const LURK_THREAT: f32 = 600.0;
/// BOT-151 (amends BOT-127): the agent is chased by the [`AGENT_CHASERS`] hunters nearest his known place; the next
/// [`GUARDS`] (by player id) hold the uplink's approaches: grid places [`GUARD_MIN`]..[`GUARD_MAX`] units (flat)
/// from it, within [`GUARD_RISE`] of its height, one per angle round it (the first toward where the agent was when
/// the bot first knew of him); the others play on (roam and fight, BOT-140).
pub const AGENT_CHASERS: usize = 2;
pub const GUARDS: usize = 2;
pub const GUARD_MIN: f32 = 350.0;
pub const GUARD_MAX: f32 = 900.0;
pub const GUARD_RISE: f32 = 250.0;
pub const GUARD_RING: f32 = 600.0;
/// BOT-152: the last case waits while a hunter is within [`FIFTH_HUNTER`] of it, unless another holder is within
/// [`FIFTH_CONTEST`] of it, at most [`FIFTH_PATIENCE_MS`]; meanwhile the holder stands off at a
/// grid place [`FIFTH_RING`] (min, max, height) from the case picked like the agent's waiting place.
// DC-DEFAULT: BOT-152 (proposed)
pub const FIFTH_HUNTER: f32 = 450.0;
pub const FIFTH_CONTEST: f32 = 400.0;
pub const FIFTH_PATIENCE_MS: u32 = 10_000;
pub const FIFTH_RING: (f32, f32, f32) = (300.0, 900.0, 200.0);
/// BOT-155: a chase the bot makes no headway in (its body moved less than [`CHASE_GAIN`] units, flat, in
/// [`CHASE_STALL_MS`], or it had no grid way for [`CHASE_NO_WAY_MS`]) is given up: that player is not chased by this
/// bot for [`CHASE_DROP_MS`] (it takes another holder, a case or roams).
// DC-DEFAULT: BOT-155 (proposed)
pub const CHASE_GAIN: f32 = 64.0;
pub const CHASE_STALL_MS: u32 = 10_000;
pub const CHASE_NO_WAY_MS: u32 = 8_000;
pub const CHASE_DROP_MS: u32 = 30_000;

/// BOT-163: a case place at which the bot counted [`FAIL_MAX`] failed attempts is out of its reach for the rest of
/// the match (its count survives the bot's deaths; it starts afresh once no case lies there any more: somebody took
/// it). A failed attempt: a walk to it that stalls (BOT-127 / BOT-141), the bot's death while it walked to it within
/// [`FAIL_NEAR`] units (flat) of it and more than [`UNDER_RISE`] below it, the death of another holder one case short
/// (he loses his cases) as near under it (his place seen within [`FAIL_SEEN_MS`]), a player (the bot under the case it
/// walks to, or another holder one case short) standing [`UNDER_FLAT`] units (flat) or nearer
/// under it, more than [`UNDER_RISE`] below its point, for [`UNDER_MS`] (the server's "across from intel but off in
/// height"); at most one count per place every [`FAIL_GAP_MS`], one per player standing under it every
/// [`UNDER_GAP_MS`]. Before, a case a grid node lay near but no way led onto (Gray Hammer's deck case at (2251, 304,
/// -2030)) drew 16 holders of 4 to their deaths under it in 220 s: each died inside the stall time, and the stall's
/// block lasted 60 s.
// DC-DEFAULT: BOT-163 (proposed)
pub const FAIL_MAX: u32 = 3;
pub const FAIL_NEAR: f32 = 400.0;
pub const FAIL_GAP_MS: u32 = 3_000;
pub const UNDER_FLAT: f32 = 128.0;
pub const UNDER_RISE: f32 = 96.0;
pub const UNDER_MS: u32 = 3_000;
pub const UNDER_GAP_MS: u32 = 10_000;
pub const FAIL_SEEN_MS: u32 = 3_000;
/// BOT-163: a case place that passed the reach test only because the bot's grid was still growing is tested again
/// after this long.
pub const RECHECK_MS: u32 = 5_000;

/// Why a case is no goal for a bot (BOT-137).
pub const NO_REACH: &str = "reach";
pub const IN_DANGER: &str = "danger";
pub const NO_FLOOR: &str = "void";

/// BOT-137: whether a bot can take the case lying at `c` (a standing player's centre over the case's floor,
/// DSV-391): `None` when it can, else why not: [`IN_DANGER`] when the case's point lies in a kill volume (it fell
/// into the water: the server lays it on the pool floor inside the fall volume), [`NO_FLOOR`] over no floor within
/// [`FLOOR_BELOW`] (the void); [`NO_REACH`] when the bot's grid is grown in full and has no node within [`REACH`]
/// (flat) and [`REACH_RISE`] (height) of it (a case on a post top or a roof no way leads to; a grid still growing
/// may not have got there yet and is given the benefit). Points, not the bot's box: the case is taken from 56 units
/// across (DSV-231), so a case by a wall or a water edge is a goal (a box test refused those). BOT-163: the session
/// tests a pass while the grid grows again later ([`SpyRun::note_reach`]).
// DC-DEFAULT: BOT-137 (proposed), BOT-163 (proposed)
pub fn case_reach(
    nav: &mut crate::nav::NavGrid,
    world: &dc_sim::CollisionWorld,
    kill: &[dc_sim::ConvexVolume],
    c: [f32; 3],
) -> Option<&'static str> {
    use dc_sim::{Collider, Cylinder, V3};
    let point = Cylinder::from_dims(V3::new(0.01, 0.01, 0.01));
    let at = V3::new(c[0], c[1] + 1.0, c[2]);
    if kill.iter().any(|k| k.touches(&point, at)) {
        return Some(IN_DANGER);
    }
    // (a thin sweep, not a ray: a ray slips through the seams between floor polygons)
    let thin = Cylinder::from_dims(V3::new(FLOOR_PROBE, FLOOR_PROBE, FLOOR_PROBE));
    let top = V3::new(c[0], c[1] + FLOOR_PROBE + 2.0, c[2]);
    let floor = world
        .sweep(&thin, top, V3::new(c[0], c[1] - FLOOR_BELOW, c[2]))
        .is_some_and(|h| h.t < 0.999);
    if !floor {
        return Some(NO_FLOOR);
    }
    let fits = |s: [f32; 3]| flat(s, c) <= REACH && (s[1] - c[1]).abs() <= REACH_RISE;
    if nav.growing() || nav.is_empty() {
        return None;
    }
    let node = nav.links_near(c, REACH).iter().any(|n| fits(n.0));
    (!node).then_some(NO_REACH)
}

/// BOT-137: the place of the hunter a holder (`held` of `total` cases, not the agent) at `at` falls back from: the
/// nearest of `hunters` within [`HOLDER_KEEP_AWAY`], unless the bot's `goal` is a case within [`HOLDER_CASE_NEAR`]
/// (it takes it first). `None`: the holder walks its goal. BOT-140: only a holder of [`HOLDER_CAUTIOUS`] or more
/// falls back, and from a hunter on its way to a case only when he stands nearer that case than the holder (a
/// fleeing holder walked away from the cases and died anyway: 45 of 53 holder deaths of a Warhead room came while
/// it fell back, a holder of four never got to the fifth case).
// DC-DEFAULT: BOT-137 (proposed), BOT-140 (proposed)
pub fn keep_away_from(
    held: u32,
    total: u32,
    goal: Option<&SpyGoal>,
    at: [f32; 3],
    hunters: &[[f32; 3]],
) -> Option<[f32; 3]> {
    if held < HOLDER_CAUTIOUS || held >= total {
        return None;
    }
    let case = match goal {
        Some(SpyGoal::Case(c)) if flat(*c, at) <= HOLDER_CASE_NEAR => return None,
        Some(SpyGoal::Case(c)) => Some(*c),
        _ => None,
    };
    hunters
        .iter()
        .map(|h| (*h, flat(*h, at)))
        .filter(|h| h.1 <= HOLDER_KEEP_AWAY)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|h| h.0)
        .filter(|h| case.is_none_or(|c| flat(*h, c) < flat(at, c)))
}

/// What the bot knows of the mode.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpyRun {
    /// Cases the bot holds (its own sub 0 `value`, 0 once it is a hunter again).
    pub held: u32,
    /// Every case of the room (sub 1 `total`).
    pub total: u32,
    /// Players that hold cases (sub 0 with the spy team) and how many.
    pub holders: BTreeMap<u32, u32>,
    /// The agent's stage runs (sub 5 phase 1 until phase 0).
    pub agent_stage: bool,
    /// A case that just counted as out of reach (taken by the session for its event).
    pub just_blocked: Option<[f32; 3]>,
    walks: Vec<CaseWalk>,
    blocked: Vec<([f32; 3], u32)>,
    /// BOT-137: case places checked for reach and the ones out of it (with why), kept while the case lies there.
    checked: Vec<[f32; 3]>,
    unreachable: Vec<([f32; 3], &'static str)>,
    current: Option<[f32; 3]>,
    /// Where each other player was last seen and when.
    last_seen: BTreeMap<u32, ([f32; 3], u32)>,
    /// BOT-141: the bot's way to the place it walked to last (the session's follower), for the stall rule.
    pub way: Option<SpyWay>,
    /// BOT-151: every other player of the room (the session's roster), for the guards' order.
    pub roster: Vec<u32>,
    /// BOT-151: when the bot first knew of the agent in this stage, and the first guard's angle round the uplink.
    stage_at: Option<u32>,
    guard_base: Option<f32>,
    /// BOT-155: players this bot gave its chase of up, until when.
    dropped: BTreeMap<u32, u32>,
    /// BOT-163: failed attempts per case place this match; places that passed the reach test only provisionally (the
    /// grid grew) and when; a case place given up that the session has not yet told of.
    fails: Vec<CaseFails>,
    provisional: Vec<([f32; 3], u32)>,
    pub just_given_up: Option<[f32; 3]>,
    lost_holders: Vec<([f32; 3], u32)>,
}

/// BOT-163: the failed attempts at one case place: how many, when the last one counted, and the players standing
/// under it (pid, since when, when last counted).
#[derive(Debug, Clone, PartialEq)]
struct CaseFails {
    at: [f32; 3],
    count: u32,
    last: Option<u32>,
    under: Vec<(u32, u32, Option<u32>)>,
}

/// BOT-141: what the bot's follower knows of its way to `target`: the way left along it, or that the grid is still
/// growing one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpyWay {
    pub target: [f32; 3],
    pub left: Option<f32>,
    pub growing: bool,
}

/// The bot's way to one case: where it lies, the nearest the bot got and when, since when it stands near, when the
/// bot last walked to it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CaseWalk {
    at: [f32; 3],
    best: f32,
    since: u32,
    near_since: Option<u32>,
    seen: u32,
    /// BOT-141: when the walk began, and how its distance is measured (0 flat, 1 along the way, 2 the way grows).
    first: u32,
    kind: u8,
}

/// Where the bot walks in Spy Hunt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpyGoal {
    /// The upload point (the bot holds every case, or holds some and has nothing else to do).
    Uplink([f32; 3]),
    /// The case of the bot's spread.
    Case([f32; 3]),
    /// A player to chase: the agent, else the spy holding the most cases.
    Chase(u32, [f32; 3]),
    /// BOT-151: the bot holds the approach number `k` of the uplink at `u`; `base` is the first approach's angle.
    Guard(usize, [f32; 3], f32),
}

/// BOT-151: what a hunter does while another player is the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentJob {
    Chase,
    Guard(usize),
    Free,
}

/// BOT-151: the job of hunter `own` at `at` while there is an agent: the [`AGENT_CHASERS`] hunters (`own` and the
/// `hunters` with a known place) nearest the agent's place `agent_at` (ties by pid) chase him; of the rest the
/// first [`GUARDS`] by player id among `roster` (every hunter of the room the bot knows of, place or not; `own`
/// counts even when missing) guard the uplink; the others are free. Without a known agent place nobody chases.
// DC-DEFAULT: BOT-151 (proposed)
pub fn agent_job(
    own: u32,
    at: [f32; 3],
    hunters: &[(u32, [f32; 3])],
    roster: &[u32],
    agent_at: Option<[f32; 3]>,
) -> AgentJob {
    let mut chasers: Vec<u32> = Vec::new();
    if let Some(ap) = agent_at {
        let mut all: Vec<(f32, u32)> = std::iter::once((own, at))
            .chain(hunters.iter().copied().filter(|h| h.0 != own))
            .map(|(p, pos)| (flat(pos, ap), p))
            .collect();
        all.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        chasers = all.iter().take(AGENT_CHASERS).map(|x| x.1).collect();
        if chasers.contains(&own) {
            return AgentJob::Chase;
        }
    }
    let mut rest: Vec<u32> = roster
        .iter()
        .copied()
        .chain(std::iter::once(own))
        .filter(|p| !chasers.contains(p))
        .collect();
    rest.sort_unstable();
    rest.dedup();
    match rest.iter().position(|p| *p == own) {
        Some(i) if i < GUARDS => AgentJob::Guard(i),
        _ => AgentJob::Free,
    }
}

/// BOT-151: the agent's waiting place while the uplink is shut: of the grid `nodes`, those [`LURK_MIN`]..
/// [`LURK_MAX`] (flat) from the uplink `u` and within [`LURK_RISE`] of its height, the one whose nearest known hunter
/// (`hunters`, capped at [`LURK_SAFE`]) is farthest, less [`LURK_WALK`] of the bot's walk there from `at`. Only
/// places the bot is nearer to than every hunter count (it does not run past a hunter to get there), unless there
/// are none.
// DC-DEFAULT: BOT-151 (proposed)
pub fn lurk_place(
    nodes: &[[f32; 3]],
    u: [f32; 3],
    at: [f32; 3],
    hunters: &[[f32; 3]],
) -> Option<[f32; 3]> {
    safe_place(nodes, u, (LURK_MIN, LURK_MAX, LURK_RISE), at, hunters)
}

/// BOT-151 / BOT-152: of the grid `nodes` `ring.0..ring.1` units (flat) from `c` and within `ring.2` of its height,
/// the place [`lurk_place`] describes.
pub fn safe_place(
    nodes: &[[f32; 3]],
    c: [f32; 3],
    ring: (f32, f32, f32),
    at: [f32; 3],
    hunters: &[[f32; 3]],
) -> Option<[f32; 3]> {
    let ring: Vec<[f32; 3]> = nodes
        .iter()
        .copied()
        .filter(|n| {
            let d = flat(*n, c);
            (ring.0..=ring.1).contains(&d) && (n[1] - c[1]).abs() <= ring.2
        })
        .collect();
    let first: Vec<[f32; 3]> = ring
        .iter()
        .copied()
        .filter(|n| hunters.iter().all(|h| flat(*h, *n) > flat(at, *n)))
        .collect();
    let pick = if first.is_empty() { ring } else { first };
    pick.iter()
        .map(|n| {
            let safe = hunters
                .iter()
                .map(|h| flat(*h, *n))
                .fold(LURK_SAFE, f32::min);
            (safe - LURK_WALK * flat(*n, at), *n)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|x| x.1)
}

/// BOT-152: a holder one case short of every case (the 5th case makes it the agent, and the hunters come) does not
/// take the last case `c` while a hunter (`hunters`, known places) stands within [`FIFTH_HUNTER`] units (flat) of the
/// case, unless another holder (`holders`) is within [`FIFTH_CONTEST`] of the case (he would take it first) or the
/// holder has waited [`FIFTH_PATIENCE_MS`] (`waited`). True: it waits. (A first cut also waited for hunters near the
/// holder, 600 units / 30 s, then 450 / 15 s: stage 2 came later, 3 of 8 rooms within 4 min against master's 5.)
// DC-DEFAULT: BOT-152 (proposed)
pub fn fifth_waits(
    held: u32,
    total: u32,
    c: [f32; 3],
    hunters: &[[f32; 3]],
    holders: &[[f32; 3]],
    waited: u32,
) -> bool {
    if total < 2 || held + 1 != total || waited >= FIFTH_PATIENCE_MS {
        return false;
    }
    if holders.iter().any(|h| flat(*h, c) <= FIFTH_CONTEST) {
        return false;
    }
    hunters.iter().any(|h| flat(*h, c) <= FIFTH_HUNTER)
}

/// BOT-151: the place of guard `k` of the uplink `u`: of the grid `nodes` [`GUARD_MIN`]..[`GUARD_MAX`] (flat) from
/// it and within [`GUARD_RISE`] of its height, the one whose angle round the uplink lies nearest `base + k` x the
/// angle between two guards (ties: nearer [`GUARD_RING`]).
// DC-DEFAULT: BOT-151 (proposed)
pub fn guard_place(nodes: &[[f32; 3]], u: [f32; 3], base: f32, k: usize) -> Option<[f32; 3]> {
    let want = base + k as f32 * std::f32::consts::TAU / GUARDS as f32;
    let off = |n: &[f32; 3]| {
        let a = (n[2] - u[2]).atan2(n[0] - u[0]);
        let d = (a - want).rem_euclid(std::f32::consts::TAU);
        d.min(std::f32::consts::TAU - d) + (flat(*n, u) - GUARD_RING).abs() / 10_000.0
    };
    nodes
        .iter()
        .filter(|n| {
            let d = flat(**n, u);
            (GUARD_MIN..=GUARD_MAX).contains(&d) && (n[1] - u[1]).abs() <= GUARD_RISE
        })
        .min_by(|a, b| off(a).total_cmp(&off(b)))
        .copied()
}

/// BOT-164: the player a chaser fights from where it stands: the one its `goal` chases, when he is the shooter's
/// target in clear sight (`fighting`, the target's pid while the shooter has a clear line to it). Like the TDM hunt
/// (no hunt walk while the target is in sight) the chaser stops closing in and fights; it walks its chase again once
/// he is out of sight. Before, the chase goal was the holder's own place while in sight: 30..52 % of Spy Hunt hits
/// landed under 20 units (TDM 0..1 %).
// DC-DEFAULT: BOT-164 (proposed)
pub fn chase_engages(goal: Option<&SpyGoal>, fighting: Option<u32>) -> Option<u32> {
    match goal {
        Some(SpyGoal::Chase(pid, _)) if fighting == Some(*pid) => Some(*pid),
        _ => None,
    }
}

/// BOT-173 (amends BOT-151): a guard of the uplink takes, of the band nodes whose angle lies within [`COVER_ARC`]
/// of the best one [`guard_place`] finds (at most [`COVER_TESTS`] of them, [`crate::cover::TEST_SPREAD`] apart,
/// nearest that angle first), the one with the best cover score (`cover`, [`crate::cover::score`]) less its angle
/// off (radians); before, the node nearest the angle whatever stood round it.
// DC-DEFAULT: BOT-173 (proposed) 0.35 rad (20 degrees), 16 nodes
pub const COVER_ARC: f32 = 0.35;
pub const COVER_TESTS: usize = 16;

pub fn guard_place_scored(
    nodes: &[[f32; 3]],
    u: [f32; 3],
    base: f32,
    k: usize,
    mut cover: impl FnMut([f32; 3]) -> f32,
) -> Option<[f32; 3]> {
    let best = guard_place(nodes, u, base, k)?;
    let want = base + k as f32 * std::f32::consts::TAU / GUARDS as f32;
    let off = |n: [f32; 3]| {
        let d = (angle_of(u, n) - want).rem_euclid(std::f32::consts::TAU);
        d.min(std::f32::consts::TAU - d)
    };
    let limit = off(best) + COVER_ARC;
    let mut near: Vec<[f32; 3]> = nodes
        .iter()
        .copied()
        .filter(|n| {
            let d = flat(*n, u);
            (GUARD_MIN..=GUARD_MAX).contains(&d) && (n[1] - u[1]).abs() <= GUARD_RISE && off(*n) <= limit
        })
        .collect();
    near.sort_by(|a, b| off(*a).total_cmp(&off(*b)));
    let mut tested: Vec<[f32; 3]> = vec![best];
    for n in near {
        if tested.len() >= COVER_TESTS {
            break;
        }
        if tested.iter().all(|t| flat(*t, n) >= crate::cover::TEST_SPREAD) {
            tested.push(n);
        }
    }
    let mut out = (best, f32::MIN);
    for n in tested {
        let s = cover(n) - off(n);
        if s > out.1 {
            out = (n, s);
        }
    }
    Some(out.0)
}

/// BOT-151: the angle (round the vertical) from `u` to `p`.
pub fn angle_of(u: [f32; 3], p: [f32; 3]) -> f32 {
    (p[2] - u[2]).atan2(p[0] - u[0])
}

impl SpyGoal {
    pub fn pos(&self) -> [f32; 3] {
        match *self {
            SpyGoal::Uplink(p) | SpyGoal::Case(p) | SpyGoal::Chase(_, p) => p,
            SpyGoal::Guard(_, u, _) => u,
        }
    }

    /// The `track` event's goal kind.
    pub fn kind(&self) -> &'static str {
        match self {
            SpyGoal::Uplink(_) | SpyGoal::Case(_) => "spy",
            SpyGoal::Chase(..) => "spy_chase",
            SpyGoal::Guard(..) => "spy_guard",
        }
    }
}

/// Whether a blob is an intel case.
pub fn is_intel(blob: &BitWriter) -> bool {
    let mut r = blob.reader();
    if r.remaining_bits() < 8 + 3 + 8 + 32 || r.read_u8() != PICKUP_TYPE {
        return false;
    }
    r.read_bits(3);
    r.read_zstr8();
    r.remaining_bits() >= 32 && r.read_u32() == PICKUP_KIND_INTEL
}

/// Whether a blob is the uplink zone (kind 8, class 9).
pub fn is_uplink(blob: &BitWriter) -> bool {
    let mut r = blob.reader();
    if r.remaining_bits() < 8 + 64 + 1 + 72 + 32 + 96 || r.read_u8() != ZONE_TYPE {
        return false;
    }
    r.skip_bits(64 + 1 + 72 + 8 + 8);
    let class = r.read_u8();
    r.skip_bits(8 + 32);
    let kind = r.read_u32();
    kind == ZONE_KIND_USE_TARGET && class == SPY_CLASS
}

fn places(table: &ObjectTable, f: fn(&BitWriter) -> bool) -> Vec<[f32; 3]> {
    let mut v: Vec<(u16, [f32; 3])> = table
        .objects
        .iter()
        .filter(|(_, o)| o.blob.as_ref().is_some_and(f))
        .map(|(id, o)| (*id, o.pos.unwrap_or(o.create_pos)))
        .collect();
    v.sort_by_key(|x| x.0);
    v.into_iter().map(|x| x.1).collect()
}

/// The 0xcb body (after the shell id): `Some((0, value, pid, team))` for sub 0, `Some((1, total, _, _))` for sub 1,
/// `Some((5, phase, _, _))` for sub 5.
pub fn read_bounty(body: &BitWriter) -> Option<(u8, u32, u32, u8)> {
    let mut r = body.reader();
    if r.remaining_bits() < 8 {
        return None;
    }
    match r.read_u8() {
        0 if r.remaining_bits() >= 32 + 8 + 32 => {
            let v = r.read_u32();
            let team = r.read_u8();
            Some((0, v, r.read_u32(), team))
        }
        1 if r.remaining_bits() >= 32 => Some((1, r.read_u32(), u32::MAX, 0)),
        5 if r.remaining_bits() >= 8 => Some((5, u32::from(r.read_u8()), u32::MAX, 0)),
        _ => None,
    }
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

/// The spread's cost: a case on another level costs more than one across the floor (the way-finder rarely finds
/// the stairs).
fn cost(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2) + (HEIGHT_WEIGHT * (a[1] - b[1])).powi(2)).sqrt()
}

fn same(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1.0)
}

/// BOT-127: the team's spread: every player (the bot `own` at `at` and the `others`) and every case form pairs, the
/// cheapest pair takes its player and case first (ties by pid, then case order), and so on; the bot's case is the
/// one it got. Every bot sees the same players and cases and so gets a case of its own while there are enough. The
/// bot pairs only with the cases of `allowed` (the ones not out of its reach); its `current` case costs [`KEEP`] of
/// its distance. BOT-137: a player holding `held(pid)` cases costs 1 / (1 + [`HOLDER_PULL`] x held) of his distance.
pub fn spread(
    cases: &[[f32; 3]],
    allowed: impl Fn(&[f32; 3]) -> bool,
    own: u32,
    at: [f32; 3],
    others: &[(u32, [f32; 3])],
    current: Option<[f32; 3]>,
    held: impl Fn(u32) -> u32,
) -> Option<[f32; 3]> {
    let pull = |pid: u32| 1.0 / (1.0 + HOLDER_PULL * held(pid) as f32);
    let mut pairs: Vec<(f32, u32, usize)> = Vec::new();
    for (j, c) in cases.iter().enumerate() {
        if allowed(c) {
            let k = if current.is_some_and(|g| same(g, *c)) {
                KEEP
            } else {
                1.0
            };
            pairs.push((cost(at, *c) * k * pull(own), own, j));
        }
        for (pid, p) in others.iter().filter(|o| o.0 != own) {
            pairs.push((cost(*p, *c) * pull(*pid), *pid, j));
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut taken_p: Vec<u32> = Vec::new();
    let mut taken_c: Vec<usize> = Vec::new();
    for (_, pid, j) in pairs {
        if taken_p.contains(&pid) || taken_c.contains(&j) {
            continue;
        }
        if pid == own {
            return Some(cases[j]);
        }
        taken_p.push(pid);
        taken_c.push(j);
    }
    None
}

/// BOT-140: the holder the hunter `own` at `at` chases, if any: every hunter (`own` and the `hunters`, players
/// holding no case) and every holder with a known place (`holders`: pid, place, cases held) form pairs costing
/// the flat distance x 1 / (1 + cases), the cheapest pair goes first (ties by hunter pid, then holder pid), each
/// hunter takes one holder and each holder at most [`CHASERS`] hunters. Every bot sees about the same places, so
/// the nearest two chase a holder and the rest of the hunters play on (before, all seven hunters chased the one
/// holder with the most cases and he died 7..11 s after a seizure).
// DC-DEFAULT: BOT-140 (proposed)
pub fn chase_of(
    own: u32,
    at: [f32; 3],
    hunters: &[(u32, [f32; 3])],
    holders: &[(u32, [f32; 3], u32)],
) -> Option<(u32, [f32; 3])> {
    let mut pairs: Vec<(f32, u32, u32, [f32; 3])> = Vec::new();
    let me = std::iter::once((own, at)).chain(hunters.iter().copied().filter(|h| h.0 != own));
    for (h, hp) in me {
        for (s, sp, n) in holders.iter().filter(|s| s.0 != h) {
            pairs.push((flat(hp, *sp) / (1.0 + *n as f32), h, *s, *sp));
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut done: Vec<u32> = Vec::new();
    let mut count: BTreeMap<u32, usize> = BTreeMap::new();
    for (_, h, s, sp) in pairs {
        if done.contains(&h) || count.get(&s).copied().unwrap_or(0) >= CHASERS {
            continue;
        }
        if h == own {
            return Some((s, sp));
        }
        done.push(h);
        *count.entry(s).or_default() += 1;
    }
    None
}

impl SpyRun {
    /// One 0xcb body for the bot of client id `own`.
    pub fn on_bounty(&mut self, body: &BitWriter, own: Option<u32>) {
        match read_bounty(body) {
            Some((0, v, pid, team)) => {
                // BOT-127: who holds cases (a spy, team 2); a hunter again holds none
                if team == SPY_TEAM && v > 0 {
                    self.holders.insert(pid, v);
                } else if self
                    .holders
                    .remove(&pid)
                    .is_some_and(|n| n + 1 >= self.total)
                    && Some(pid) != own
                {
                    // BOT-163: another holder one case short lost his cases (he died): checked in goal()
                    if let Some(l) = self.last_seen.get(&pid) {
                        self.lost_holders.push(*l);
                    }
                }
                if Some(pid) == own {
                    self.held = v;
                }
            }
            Some((1, total, ..)) => self.total = total,
            Some((5, 1, ..)) => self.agent_stage = true,
            // stage 2 ends: the agent's cases go back to the map
            Some((5, 0, ..)) => {
                self.held = 0;
                self.holders.clear();
                self.agent_stage = false;
                self.stage_at = None;
                self.guard_base = None;
            }
            _ => {}
        }
    }

    /// The team of player `pid` by the cases he holds: a spy (or the agent) is team 2, everyone else a hunter.
    pub fn team_of(&self, pid: u32) -> u8 {
        if self.holders.contains_key(&pid) {
            SPY_TEAM
        } else {
            HUNTER_TEAM
        }
    }

    /// The place of player `pid`: where he is now (in `others`), else where he was last seen (not yet visited).
    fn place_of(&self, pid: u32, others: &[(u32, [f32; 3])]) -> Option<[f32; 3]> {
        others
            .iter()
            .find(|o| o.0 == pid)
            .map(|o| o.1)
            .or_else(|| self.last_seen.get(&pid).map(|l| l.0))
    }

    /// The uplink's place (the first uplink zone of the object table).
    pub fn uplink(&self, table: &ObjectTable) -> Option<[f32; 3]> {
        places(table, is_uplink).into_iter().next()
    }

    /// The agent: the player holding every case.
    pub fn agent(&self) -> Option<u32> {
        (self.total > 0)
            .then(|| self.holders.iter().find(|(_, v)| **v >= self.total))
            .flatten()
            .map(|(p, _)| *p)
    }

    /// BOT-155: the bot gives its chase of player `pid` up until `now` + [`CHASE_DROP_MS`].
    pub fn drop_chase(&mut self, pid: u32, now: u32) {
        self.dropped.insert(pid, now.wrapping_add(CHASE_DROP_MS));
    }

    /// BOT-155: whether the bot gave its chase of `pid` up (until the time set by [`Self::drop_chase`]).
    pub fn chase_dropped(&self, pid: u32, now: u32) -> bool {
        self.dropped
            .get(&pid)
            .is_some_and(|until| (until.wrapping_sub(now) as i32) > 0)
    }

    /// BOT-151: how long the agent's stage has run at `now` (since the bot first knew of the agent).
    pub fn stage_ms(&self, now: u32) -> Option<u32> {
        self.stage_at.map(|t| now.wrapping_sub(t))
    }

    /// BOT-151: whether the agent at `at` sets off for the uplink at `u` now: its walk there (`way`, the length
    /// of its grid way when it has one, else the flat distance x [`AGENT_WAY`]) ends [`AGENT_EARLY_MS`] before the
    /// box opens ([`STATION_OPEN_MS`]), or later.
    pub fn agent_sets_off(&self, now: u32, at: [f32; 3], u: [f32; 3], way: Option<f32>) -> bool {
        let d = way.unwrap_or(flat(at, u) * AGENT_WAY).max(flat(at, u));
        let walk = (d / AGENT_SPEED * 1000.0) as u32;
        self.stage_ms(now).is_none_or(|s| {
            s.saturating_add(walk).saturating_add(AGENT_EARLY_MS) >= STATION_OPEN_MS
        })
    }

    /// BOT-151: the places of the hunters the bot knows of: where they are (`others` holding no case), else where
    /// they were last seen (within [`LAST_SEEN_MS`]).
    pub fn hunter_places(&self, others: &[(u32, [f32; 3])]) -> Vec<[f32; 3]> {
        let mut v: Vec<[f32; 3]> = others
            .iter()
            .filter(|o| !self.holders.contains_key(&o.0))
            .map(|o| o.1)
            .collect();
        v.extend(
            self.last_seen
                .iter()
                .filter(|(p, _)| {
                    !self.holders.contains_key(p) && !others.iter().any(|o| o.0 == **p)
                })
                .map(|(_, l)| l.0),
        );
        v
    }

    /// The case the bot walked to last (BOT-141: the session tells [`Self::way`] the follower's way to it).
    pub fn current_case(&self) -> Option<[f32; 3]> {
        self.current
    }

    /// Whether the case at `c` is out of the bot's reach at `now` (BOT-127's stall, BOT-137's reach and danger).
    pub fn is_blocked(&self, c: &[f32; 3], now: u32) -> bool {
        self.blocked
            .iter()
            .any(|(b, at)| same(*b, *c) && now.wrapping_sub(*at) < BLOCK_MS)
            || self.unreachable.iter().any(|(b, _)| same(*b, *c))
            || self.given_up(c)
    }

    /// BOT-163: whether the bot gave the case place `c` up for the match ([`FAIL_MAX`] failed attempts).
    pub fn given_up(&self, c: &[f32; 3]) -> bool {
        self.fail_count(c) >= FAIL_MAX
    }

    /// BOT-163: the failed attempts counted at the case place `c`.
    pub fn fail_count(&self, c: &[f32; 3]) -> u32 {
        self.fails
            .iter()
            .find(|f| same(f.at, *c))
            .map_or(0, |f| f.count)
    }

    fn fails_of(&mut self, c: [f32; 3]) -> &mut CaseFails {
        let i = match self.fails.iter().position(|f| same(f.at, c)) {
            Some(i) => i,
            None => {
                self.fails.push(CaseFails {
                    at: c,
                    count: 0,
                    last: None,
                    under: Vec::new(),
                });
                self.fails.len() - 1
            }
        };
        &mut self.fails[i]
    }

    /// BOT-163: one failed attempt at the case place `c` at `now` (none within [`FAIL_GAP_MS`] of the last one).
    pub fn fail(&mut self, c: [f32; 3], now: u32) {
        let f = self.fails_of(c);
        if f.count >= FAIL_MAX || f.last.is_some_and(|t| now.wrapping_sub(t) < FAIL_GAP_MS) {
            return;
        }
        f.count += 1;
        f.last = Some(now);
        if f.count >= FAIL_MAX {
            self.just_given_up = Some(c);
            if self.current.is_some_and(|x| same(x, c)) {
                self.current = None;
            }
        }
    }

    /// BOT-163: the bot died at `at` at `now`: a failed attempt at the case it walked to when that lay within
    /// [`FAIL_NEAR`] (flat) and more than [`UNDER_RISE`] above it.
    pub fn on_died(&mut self, at: [f32; 3], now: u32) {
        if let Some(c) = self
            .current
            .filter(|c| flat(*c, at) <= FAIL_NEAR && c[1] - at[1] > UNDER_RISE)
        {
            self.fail(c, now);
        }
    }

    /// BOT-163: the players standing under the lying `cases` (`players`: the holders of cases, pid and place; `own`:
    /// the bot, its place and the case it walks to): one standing under a case for [`UNDER_MS`] is a failed attempt
    /// at it; the bot counts only under the case it walks to. A place no case lies at any more (somebody took it)
    /// starts its count afresh.
    fn watch_under(
        &mut self,
        cases: &[[f32; 3]],
        players: &[(u32, [f32; 3])],
        own: Option<(u32, [f32; 3], [f32; 3])>,
        now: u32,
    ) {
        self.fails.retain(|f| cases.iter().any(|c| same(*c, f.at)));
        for c in cases {
            let mine = own.filter(|o| same(o.2, *c)).map(|o| (o.0, o.1));
            let under: Vec<u32> = players
                .iter()
                .copied()
                .chain(mine)
                .filter(|(_, p)| flat(*p, *c) <= UNDER_FLAT && c[1] - p[1] > UNDER_RISE)
                .map(|(pid, _)| pid)
                .collect();
            if under.is_empty() && !self.fails.iter().any(|f| same(f.at, *c)) {
                continue;
            }
            let f = self.fails_of(*c);
            // (a player no longer under it starts afresh when he comes back; when he last counted is kept)
            f.under.retain(|u| under.contains(&u.0) || u.2.is_some());
            for u in f.under.iter_mut().filter(|u| !under.contains(&u.0)) {
                u.1 = u32::MAX;
            }
            let mut count = false;
            for pid in under {
                match f.under.iter_mut().find(|u| u.0 == pid) {
                    Some(u) if u.1 == u32::MAX => u.1 = now,
                    Some(u) => {
                        let gap = u.2.is_none_or(|t| now.wrapping_sub(t) >= UNDER_GAP_MS);
                        if now.wrapping_sub(u.1) >= UNDER_MS && gap {
                            u.2 = Some(now);
                            u.1 = now;
                            count = true;
                        }
                    }
                    None => f.under.push((pid, now, None)),
                }
            }
            if count {
                self.fail(*c, now);
            }
        }
    }

    /// BOT-137: the places of the lying cases not yet checked for reach (the session checks each once,
    /// [`case_reach`], and tells [`Self::note_reach`]); a case that moves (taken, dropped, back at a spawn point)
    /// lies at a new place and is checked again.
    pub fn unchecked(&mut self, table: &ObjectTable, now: u32) -> Vec<[f32; 3]> {
        let cases = places(table, is_intel);
        // BOT-163: a provisional pass (the grid was growing) is tested again after a while
        let due: Vec<[f32; 3]> = self
            .provisional
            .iter()
            .filter(|(_, t)| now.wrapping_sub(*t) >= RECHECK_MS)
            .map(|x| x.0)
            .collect();
        self.provisional.retain(|(c, t)| {
            now.wrapping_sub(*t) < RECHECK_MS && cases.iter().any(|x| same(*x, *c))
        });
        self.checked.retain(|c| {
            cases.iter().any(|x| same(*x, *c)) && !due.iter().any(|d| same(*d, *c))
        });
        self.unreachable
            .retain(|(c, _)| cases.iter().any(|x| same(*x, *c)));
        cases
            .into_iter()
            .filter(|c| !self.checked.iter().any(|x| same(*x, *c)))
            .collect()
    }

    /// BOT-137: the reach check of the case at `c`: `why` it is out of reach, `None` when the bot can take it.
    /// BOT-163: a pass while the grid still grows (`provisional`) is tested again after [`RECHECK_MS`].
    pub fn note_reach(&mut self, c: [f32; 3], why: Option<&'static str>, provisional: bool, now: u32) {
        self.checked.push(c);
        if provisional && why.is_none() {
            self.provisional.push((c, now));
        }
        if let Some(w) = why {
            self.unreachable.push((c, w));
        }
    }

    /// The walk goal of the bot (client id `own`) at `at` at `now`; `others` are the other living players with a
    /// known place (client id, place).
    pub fn goal(
        &mut self,
        table: &ObjectTable,
        own: Option<u32>,
        at: [f32; 3],
        others: &[(u32, [f32; 3])],
        now: u32,
    ) -> Option<SpyGoal> {
        self.blocked
            .retain(|(_, t)| now.wrapping_sub(*t) < BLOCK_MS);
        self.walks.retain(|w| now.wrapping_sub(w.seen) <= FORGET_MS);
        for (pid, p) in others {
            self.last_seen.insert(*pid, (*p, now));
        }
        self.last_seen.retain(|pid, (p, t)| {
            others.iter().any(|o| o.0 == *pid)
                || (now.wrapping_sub(*t) < LAST_SEEN_MS && flat(*p, at) > LAST_SEEN_REACHED)
        });
        let uplink = || {
            places(table, is_uplink)
                .into_iter()
                .next()
                .map(SpyGoal::Uplink)
        };
        // BOT-151: the stage's clock runs from when the bot first knows of the agent
        match self.agent() {
            Some(_) => {
                self.stage_at.get_or_insert(now);
            }
            None => {
                self.stage_at = None;
                self.guard_base = None;
            }
        }
        if self.total > 0 && self.held >= self.total {
            self.current = None;
            return uplink();
        }
        // BOT-127 (amended by BOT-151): the nearest hunters chase the agent, the next ones guard the uplink
        if let Some(a) = self.agent().filter(|a| Some(*a) != own) {
            self.current = None;
            let me = own.unwrap_or(u32::MAX);
            let ap = self.place_of(a, others);
            let hunters: Vec<(u32, [f32; 3])> = others
                .iter()
                .copied()
                .filter(|o| o.0 != a && !self.holders.contains_key(&o.0))
                .collect();
            let roster: Vec<u32> = self
                .roster
                .iter()
                .copied()
                .filter(|p| *p != a && !self.holders.contains_key(p))
                .collect();
            return match agent_job(me, at, &hunters, &roster, ap) {
                // BOT-155: a chase given up: the bot plays on
                AgentJob::Chase if self.chase_dropped(a, now) => None,
                AgentJob::Chase => ap.map(|p| SpyGoal::Chase(a, p)),
                AgentJob::Guard(k) => {
                    let u = places(table, is_uplink).into_iter().next()?;
                    let base = *self
                        .guard_base
                        .get_or_insert_with(|| ap.map_or(0.0, |p| angle_of(u, p)));
                    Some(SpyGoal::Guard(k, u, base))
                }
                AgentJob::Free => None,
            };
        }
        let cases = places(table, is_intel);
        let me = own.unwrap_or(u32::MAX);
        // BOT-163: holders one case short standing under a case count as failed attempts at it, and so does the bot
        // under the case it walks to (not while its grid grows a way there)
        let players: Vec<(u32, [f32; 3])> = others
            .iter()
            .copied()
            .filter(|o| {
                o.0 != me
                    && self
                        .holders
                        .get(&o.0)
                        .is_some_and(|n| n + 1 >= self.total)
            })
            .collect();
        let growing = self.way.is_some_and(|w| w.growing);
        let own_case = self.current.filter(|_| !growing).map(|c| (me, at, c));
        self.watch_under(&cases, &players, own_case, now);
        // BOT-163: another holder who died under a case (within FAIL_NEAR of it, more than UNDER_RISE below it, his
        // place seen within FAIL_SEEN_MS) failed at it
        for (p, t) in std::mem::take(&mut self.lost_holders) {
            if now.wrapping_sub(t) > FAIL_SEEN_MS {
                continue;
            }
            let under = cases
                .iter()
                .filter(|c| flat(**c, p) <= FAIL_NEAR && c[1] - p[1] > UNDER_RISE)
                .min_by(|a, b| flat(**a, p).total_cmp(&flat(**b, p)))
                .copied();
            if let Some(c) = under {
                self.fail(c, now);
            }
        }
        let pick = spread(
            &cases,
            |c| !self.is_blocked(c, now),
            me,
            at,
            others,
            self.current,
            |pid| {
                if pid == me {
                    self.held
                } else {
                    self.holders.get(&pid).copied().unwrap_or(0)
                }
            },
        );
        self.current = pick;
        if let Some(c) = pick {
            self.walk(c, at, now);
            return Some(SpyGoal::Case(c));
        }
        // BOT-140: a hunter without a case chases a holder only as one of his nearest two; else it plays on (the
        // session's hunt and roam)
        if self.held == 0 {
            let hunters: Vec<(u32, [f32; 3])> = others
                .iter()
                .copied()
                .filter(|o| !self.holders.contains_key(&o.0))
                .collect();
            let holders: Vec<(u32, [f32; 3], u32)> = self
                .holders
                .iter()
                // BOT-155: not a holder whose chase the bot gave up
                .filter(|(p, _)| Some(**p) != own && !self.chase_dropped(**p, now))
                .filter_map(|(p, n)| self.place_of(*p, others).map(|pos| (*p, pos, *n)))
                .collect();
            return chase_of(me, at, &hunters, &holders).map(|(p, pos)| SpyGoal::Chase(p, pos));
        }
        // a spy without a case of its own: the spy with the most cases (the nearest of them)
        let chase = self
            .holders
            .iter()
            .filter(|(p, _)| Some(**p) != own && !self.chase_dropped(**p, now))
            .filter_map(|(p, n)| self.place_of(*p, others).map(|pos| (*n, *p, pos)))
            .max_by(|a, b| a.0.cmp(&b.0).then(flat(b.2, at).total_cmp(&flat(a.2, at))));
        if let Some((_, p, pos)) = chase {
            return Some(SpyGoal::Chase(p, pos));
        }
        // a spy with nothing to take or chase waits at the upload point
        if self.held > 0 {
            return uplink();
        }
        None
    }

    /// BOT-127: one walk step toward case `c`; a case the bot makes no way to (or stands next to without taking it)
    /// is out of its reach for [`BLOCK_MS`].
    fn walk(&mut self, c: [f32; 3], at: [f32; 3], now: u32) {
        // BOT-141: along the bot's way when it has one to this case, else straight; the count waits while the grid
        // grows a way
        let way = self.way.filter(|w| flat(w.target, c) <= REACH);
        let (d, kind) = match way {
            Some(SpyWay { left: Some(l), .. }) => (l, 1),
            Some(SpyWay { growing: true, .. }) => (flat(c, at), 2),
            _ => (flat(c, at), 0),
        };
        let near = if kind == 1 {
            d <= NEAR
        } else {
            flat(c, at) <= NEAR && (c[1] - at[1]).abs() <= NEAR_RISE
        };
        let i = match self.walks.iter().position(|w| same(w.at, c)) {
            Some(i) => i,
            None => {
                self.walks.push(CaseWalk {
                    at: c,
                    best: d,
                    since: now,
                    near_since: None,
                    seen: now,
                    first: now,
                    kind,
                });
                self.walks.len() - 1
            }
        };
        let w = &mut self.walks[i];
        w.seen = now;
        if kind != w.kind || kind == 2 || (kind == 1 && d > w.best + STALL_GAIN * 2.0) {
            // a new measure, a way still growing, a new and longer way: the count starts afresh
            w.kind = kind;
            w.best = d;
            w.since = now;
        } else if d < w.best - STALL_GAIN {
            w.best = d;
            w.since = now;
        }
        if near {
            w.near_since.get_or_insert(now);
        } else {
            w.near_since = None;
        }
        let stalled = now.wrapping_sub(w.since) >= STALL_MS
            || w.near_since.is_some_and(|t| now.wrapping_sub(t) >= NEAR_MS)
            || now.wrapping_sub(w.first) >= WALK_MAX_MS;
        if stalled {
            self.walks.remove(i);
            self.blocked.push((c, now));
            self.current = None;
            self.just_blocked = Some(c);
            // BOT-163: and a failed attempt for the match's count
            self.fail(c, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::ClientObject;

    fn intel() -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(PICKUP_TYPE)
            .write_bit(false)
            .write_bit(false)
            .write_bit(true)
            .write_zstr8(b"")
            .write_u32(PICKUP_KIND_INTEL)
            .write_bit(false);
        w
    }

    fn uplink() -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(ZONE_TYPE)
            .write_u32(0)
            .write_u32(0)
            .write_bit(false);
        dc_wire::udp::Vec72::encode([10.0; 3]).write(&mut w);
        w.write_u8(0xff)
            .write_u8(2)
            .write_u8(SPY_CLASS)
            .write_u8(0)
            .write_u32(0)
            .write_u32(ZONE_KIND_USE_TARGET)
            .write_u32(15.0f32.to_bits())
            .write_u8(0)
            .write_u32(0)
            .write_zstr8(b"u")
            .write_zstr8(b"");
        w
    }

    fn table(items: &[(u16, BitWriter, [f32; 3])]) -> ObjectTable {
        let mut t = ObjectTable::default();
        for (id, b, p) in items {
            let mut o = ClientObject::default();
            o.blob = Some(b.clone());
            o.pos = Some(*p);
            t.objects.insert(*id, o);
        }
        t
    }

    fn sub0(v: u32, team: u8, pid: u32) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(0).write_u32(v).write_u8(team).write_u32(pid);
        w
    }

    fn sub1(total: u32) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(1).write_u32(total).write_u32(0).write_u32(0);
        w
    }

    /// BOT-75: the bot walks to the nearest intel case, and to the uplink once it holds every case.
    #[test]
    fn a_spy_bot_walks_to_the_nearest_intel_then_to_the_uplink() {
        let t = table(&[
            (10, intel(), [900.0, 0.0, 0.0]),
            (11, intel(), [-200.0, 0.0, 0.0]),
            (12, uplink(), [5000.0, 0.0, 0.0]),
        ]);
        let mut s = SpyRun::default();
        assert_eq!(
            s.goal(&t, Some(4), [0.0; 3], &[], 0),
            Some(SpyGoal::Case([-200.0, 0.0, 0.0]))
        );
        s.on_bounty(&sub1(2), Some(4));
        s.on_bounty(&sub0(2, 2, 4), Some(4));
        assert_eq!(s.held, 2);
        assert_eq!(
            s.goal(&t, Some(4), [0.0; 3], &[], 100),
            Some(SpyGoal::Uplink([5000.0, 0.0, 0.0]))
        );
        // another player's message changes nothing of the own count; stage 1 again drops the count
        s.on_bounty(&sub0(0, 1, 5), Some(4));
        assert_eq!(s.held, 2);
        let mut w = BitWriter::new();
        w.write_u8(5).write_u8(0);
        s.on_bounty(&w, Some(4));
        assert_eq!(s.held, 0);
        assert!(s.holders.is_empty());
    }

    /// BOT-127: a case the bot makes no way to in 10 s is left alone for 60 s (it takes the other one meanwhile);
    /// one it stands next to for 5 s without taking it too.
    #[test]
    fn a_case_the_bot_gets_no_closer_to_is_out_of_reach_for_a_minute() {
        let near = [300.0, -100.0, 0.0];
        let far = [-1500.0, 0.0, 0.0];
        let t = table(&[(10, intel(), near), (11, intel(), far)]);
        let mut s = SpyRun::default();
        let at = [0.0; 3];
        for ms in (0..10_000).step_by(500) {
            assert_eq!(
                s.goal(&t, Some(1), at, &[], ms),
                Some(SpyGoal::Case(near)),
                "{ms}"
            );
        }
        assert_eq!(
            s.goal(&t, Some(1), at, &[], 10_000),
            Some(SpyGoal::Case(near))
        );
        assert_eq!(s.just_blocked.take(), Some(near));
        assert_eq!(
            s.goal(&t, Some(1), at, &[], 10_500),
            Some(SpyGoal::Case(far))
        );
        assert!(s.is_blocked(&near, 69_000));
        // a minute later the near one is tried again
        assert_eq!(
            s.goal(&t, Some(1), at, &[], 70_500),
            Some(SpyGoal::Case(near))
        );
        // walking toward a case keeps it
        let only_far = table(&[(11, intel(), far)]);
        let mut s = SpyRun::default();
        for (i, ms) in (0..30_000u32).step_by(1000).enumerate() {
            let x = -(i as f32) * 50.0;
            let g = s.goal(&only_far, Some(1), [x, 0.0, 0.0], &[], ms);
            assert_eq!(g.map(|g| g.pos()), Some(far), "{ms}");
        }
        // standing beside a case it cannot take
        let mut s = SpyRun::default();
        let beside = [200.0, -100.0, 0.0];
        for ms in (0..5_000).step_by(500) {
            s.goal(&t, Some(1), beside, &[], ms);
            assert!(s.just_blocked.is_none(), "{ms}");
        }
        s.goal(&t, Some(1), beside, &[], 5_000);
        assert_eq!(s.just_blocked, Some(near));
    }

    /// BOT-141: the stall rule follows the bot's way: a case on a long way round (the bot gets no nearer in a straight
    /// line) is kept while the way left gets shorter; a bot under a case on a crate does not stand "near" it; while
    /// the grid still grows a way the count waits, at most 90 s.
    #[test]
    fn a_case_on_a_long_way_round_is_kept_while_the_way_gets_shorter() {
        let case = [0.0, 136.0, 0.0];
        let t = table(&[(10, intel(), case)]);
        let mut s = SpyRun::default();
        let at = [600.0, 0.0, 0.0];
        for (i, ms) in (0..40_000u32).step_by(500).enumerate() {
            s.way = Some(SpyWay {
                target: case,
                left: Some(5000.0 - 60.0 * i as f32),
                growing: false,
            });
            assert_eq!(s.goal(&t, Some(1), at, &[], ms), Some(SpyGoal::Case(case)));
            assert!(s.just_blocked.is_none(), "{ms}");
        }
        // under the case (136 below it): no "near" block after 5 s; the straight count blocks it after 10 s
        let mut s = SpyRun::default();
        let under = [20.0, 0.0, 0.0];
        for ms in (0..10_000).step_by(500) {
            s.goal(&t, Some(1), under, &[], ms);
            assert!(s.just_blocked.is_none(), "{ms}");
        }
        s.goal(&t, Some(1), under, &[], 10_000);
        assert_eq!(s.just_blocked.take(), Some(case));
        // the grid still grows a way: the count waits, 90 s at most
        let mut s = SpyRun::default();
        for ms in (0..90_000).step_by(500) {
            s.way = Some(SpyWay {
                target: case,
                left: None,
                growing: true,
            });
            s.goal(&t, Some(1), under, &[], ms);
            assert!(s.just_blocked.is_none(), "{ms}");
        }
        s.goal(&t, Some(1), under, &[], 90_000);
        assert_eq!(s.just_blocked, Some(case));
    }

    /// BOT-127: the bots spread over the cases (each its nearest free one by the same rule), a bot left without a
    /// case chases the spy with the most cases, and every hunter chases the agent.
    #[test]
    fn the_team_spreads_over_the_cases_and_chases_spies_and_the_agent() {
        let a = [1000.0, 0.0, 0.0];
        let b = [-1000.0, 0.0, 0.0];
        let t = table(&[
            (10, intel(), a),
            (11, intel(), b),
            (12, uplink(), [0.0, 0.0, 3000.0]),
        ]);
        // bots 1 and 2 stand near case a, bot 3 far off: 1 takes a, 2 takes b, 3 has none
        let pos = [
            (1u32, [900.0, 0.0, 0.0]),
            (2, [800.0, 0.0, 0.0]),
            (3, [0.0, 0.0, 2500.0]),
        ];
        let goal_of = |s: &mut SpyRun, me: u32| {
            let at = pos.iter().find(|p| p.0 == me).unwrap().1;
            let others: Vec<(u32, [f32; 3])> = pos.iter().copied().filter(|p| p.0 != me).collect();
            s.goal(&t, Some(me), at, &others, 0)
        };
        let mut s = SpyRun::default();
        assert_eq!(goal_of(&mut s, 1), Some(SpyGoal::Case(a)));
        let mut s = SpyRun::default();
        assert_eq!(goal_of(&mut s, 2), Some(SpyGoal::Case(b)));
        let mut s = SpyRun::default();
        assert_eq!(goal_of(&mut s, 3), None);
        // with a spy about, the bot without a case chases him (the one holding the most)
        s.on_bounty(&sub1(5), Some(3));
        s.on_bounty(&sub0(1, 2, 2), Some(3));
        s.on_bounty(&sub0(2, 2, 1), Some(3));
        assert_eq!(
            goal_of(&mut s, 3),
            Some(SpyGoal::Chase(1, [900.0, 0.0, 0.0]))
        );
        // the spy who became a hunter again is no longer chased
        s.on_bounty(&sub0(0, 1, 1), Some(3));
        assert_eq!(
            goal_of(&mut s, 3),
            Some(SpyGoal::Chase(2, [800.0, 0.0, 0.0]))
        );
        // the agent: every hunter chases him, he walks to the uplink
        s.on_bounty(&sub0(5, 2, 2), Some(3));
        assert_eq!(s.agent(), Some(2));
        assert_eq!(
            goal_of(&mut s, 3),
            Some(SpyGoal::Chase(2, [800.0, 0.0, 0.0]))
        );
        let mut agent = SpyRun::default();
        agent.on_bounty(&sub1(5), Some(2));
        agent.on_bounty(&sub0(5, 2, 2), Some(2));
        assert_eq!(
            goal_of(&mut agent, 2),
            Some(SpyGoal::Uplink([0.0, 0.0, 3000.0]))
        );
    }

    /// BOT-137: a case in the water (a kill volume under the floor's level) or with no standing place near it is no
    /// goal while it lies there; one on the floor is; a case that moves is checked again at its new place.
    #[test]
    fn a_case_in_danger_or_out_of_reach_is_no_goal_while_it_lies_there() {
        use crate::hunt::tests::solid_box;
        use dc_sim::{ConvexVolume, V3};
        use std::sync::Arc;
        // a floor x -1000..0 at y 0; beyond x 0 water: a pool floor at y -800 under a kill volume -800..-500
        let w = Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-1000.0, -64.0, -1000.0], [0.0, 0.0, 1000.0]),
                solid_box([0.0, -864.0, -1000.0], [1000.0, -800.0, 1000.0]),
            ],
            &[],
        ));
        let water = vec![ConvexVolume::from_box(
            V3::new(0.0, -800.0, -1000.0),
            V3::new(1000.0, -500.0, 1000.0),
        )];
        let dims = [16.0, 50.0, 16.0];
        let mut nav = crate::nav::NavGrid::new(w.clone(), dims).with_kill_volumes(water.clone());
        let floor = [-500.0, 50.0, 0.0];
        let wet = [500.0, -750.0, 0.0];
        let air = [-500.0, 900.0, 0.0];
        assert_eq!(case_reach(&mut nav, &w, &water, floor), None);
        assert_eq!(case_reach(&mut nav, &w, &water, wet), Some(IN_DANGER));
        // a case on the floor at the water's edge is a goal (it is taken from 56 units across)
        assert_eq!(case_reach(&mut nav, &w, &water, [-10.0, 50.0, 0.0]), None);
        // a case high over the floor (no floor within 400 below its point)
        assert_eq!(case_reach(&mut nav, &w, &water, air), Some(NO_FLOOR));
        // a case on a post top 400 high: a body stands there, but the grid grown in full has no way up
        let post = std::sync::Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-1000.0, -64.0, -1000.0], [0.0, 0.0, 1000.0]),
                solid_box([-820.0, 0.0, -20.0], [-780.0, 400.0, 20.0]),
            ],
            &[],
        ));
        let top = [-800.0, 450.0, 0.0];
        let mut grid = crate::nav::NavGrid::new(post.clone(), dims);
        assert!(grid.seed(floor));
        assert_eq!(
            case_reach(&mut grid, &post, &[], top),
            None,
            "still growing"
        );
        while grid.growing() {
            grid.expand(2_000, None);
        }
        assert_eq!(case_reach(&mut grid, &post, &[], top), Some(NO_REACH));
        assert_eq!(case_reach(&mut grid, &post, &[], floor), None);
        // the session checks each place once and the spread leaves the bad one alone
        let t = table(&[(10, intel(), wet), (11, intel(), floor)]);
        let mut s = SpyRun::default();
        let fresh = s.unchecked(&t, 0);
        assert_eq!(fresh, vec![wet, floor]);
        for c in fresh {
            let why = case_reach(&mut nav, &w, &water, c);
            s.note_reach(c, why, false, 0);
        }
        assert!(s.unchecked(&t, 0).is_empty());
        assert!(s.is_blocked(&wet, 0) && !s.is_blocked(&floor, 0));
        // the bot stands right by the wet case: it still walks to the floor one
        let at = [450.0, 50.0, 0.0];
        assert_eq!(s.goal(&t, Some(1), at, &[], 0), Some(SpyGoal::Case(floor)));
        // the wet case went back to a spawn point on the floor: it is checked again and taken
        let back = [-200.0, 50.0, 300.0];
        let t = table(&[(12, intel(), back), (11, intel(), floor)]);
        assert_eq!(s.unchecked(&t, 0), vec![back]);
        s.note_reach(back, None, false, 0);
        assert!(!s.is_blocked(&wet, 0) && !s.is_blocked(&back, 0));
        // BOT-163: a pass while the grid grew is tested again 5 s later, a final one never
        s.note_reach([0.0; 3], None, true, 100);
        let t = table(&[(13, intel(), [0.0; 3]), (11, intel(), floor)]);
        assert!(s.unchecked(&t, 100 + RECHECK_MS - 1).is_empty());
        assert_eq!(s.unchecked(&t, 100 + RECHECK_MS), vec![[0.0; 3]]);
    }

    /// BOT-163: failed attempts at a case place add up over the bot's lives (a death by the case it walks to, a
    /// stall, a holder standing under it for 3 s); at 3 the place is out of its reach for the match and the bot takes
    /// another case; once no case lies there (taken) the count starts afresh.
    #[test]
    fn a_case_that_defeats_three_attempts_is_given_up_for_the_match() {
        let deck = [0.0, 300.0, 0.0];
        let other = [3000.0, 0.0, 0.0];
        let t = table(&[(10, intel(), deck), (11, intel(), other)]);
        let mut s = SpyRun::default();
        let under = [30.0, 60.0, 0.0];
        // the bot walks to the deck case and dies under it, twice; the count survives its deaths
        assert_eq!(s.goal(&t, Some(1), under, &[], 0), Some(SpyGoal::Case(deck)));
        s.on_died(under, 1_000);
        // (a second count within 3 s of the first is the same attempt)
        s.on_died(under, 2_000);
        assert_eq!(s.fail_count(&deck), 1);
        assert_eq!(s.goal(&t, Some(1), [500.0, 0.0, 0.0], &[], 20_000), Some(SpyGoal::Case(deck)));
        s.on_died([300.0, 60.0, 0.0], 21_000);
        assert_eq!(s.fail_count(&deck), 2);
        // far from the case a death is no attempt at it
        s.on_died([900.0, 60.0, 0.0], 40_000);
        assert_eq!(s.fail_count(&deck), 2);
        assert!(!s.is_blocked(&deck, 40_000));
        // a holder (player 5 holds a case) stands under it for 3 s: the third attempt
        s.on_bounty(&sub0(1, 2, 5), Some(1));
        let others = [(5u32, [10.0, 60.0, 10.0])];
        let at = [1500.0, 0.0, 0.0];
        s.goal(&t, Some(1), at, &others, 50_000);
        s.goal(&t, Some(1), at, &others, 52_000);
        assert_eq!(s.fail_count(&deck), 2);
        s.goal(&t, Some(1), at, &others, 53_000);
        assert_eq!(s.fail_count(&deck), 3);
        assert_eq!(s.just_given_up.take(), Some(deck));
        // out of its reach for the match: long after BOT-127's minute it still takes the other case
        assert!(s.is_blocked(&deck, 500_000));
        assert_eq!(s.goal(&t, Some(1), [0.0, 60.0, 0.0], &[], 500_000), Some(SpyGoal::Case(other)));
        // somebody took the deck case: when a case lies there again it is a goal again
        let gone = table(&[(11, intel(), other)]);
        s.goal(&gone, Some(1), at, &[], 501_000);
        assert!(!s.is_blocked(&deck, 502_000));
        assert_eq!(s.fail_count(&deck), 0);
        // a hunter (no case) under it is no attempt; the bot under a case it does not walk to neither
        let mut h = SpyRun::default();
        let near = [(6u32, [10.0, 60.0, 10.0])];
        for k in 0..10u32 {
            h.goal(&t, Some(1), [2950.0, 0.0, 0.0], &near, k * 1_000);
        }
        assert_eq!(h.fail_count(&deck), 0);
        // another holder loses his cases (he died) under the deck case: an attempt; one by a floor case: none
        let away = [2950.0, 0.0, 0.0];
        // (a holder of 1 of 5 is not one case short: his death there is no attempt)
        let mut n = SpyRun::default();
        n.on_bounty(&sub1(5), Some(1));
        n.on_bounty(&sub0(1, 2, 5), Some(1));
        n.goal(&t, Some(1), away, &[(5, [200.0, 60.0, 0.0])], 1_000);
        n.on_bounty(&sub0(0, 1, 5), Some(1));
        n.goal(&t, Some(1), away, &[], 1_500);
        assert_eq!(n.fail_count(&deck), 0);
        let mut d = SpyRun::default();
        d.on_bounty(&sub1(2), Some(1));
        d.on_bounty(&sub0(1, 2, 5), Some(1));
        d.on_bounty(&sub0(2, 2, 6), Some(1));
        d.goal(&t, Some(1), away, &[(5, [200.0, 60.0, 0.0]), (6, [2800.0, 0.0, 0.0])], 1_000);
        d.on_bounty(&sub0(0, 1, 5), Some(1));
        d.on_bounty(&sub0(0, 1, 6), Some(1));
        d.goal(&t, Some(1), away, &[], 1_500);
        assert_eq!(d.fail_count(&deck), 1);
        assert_eq!(d.fail_count(&other), 0);
    }

    /// BOT-164: a chaser fights the player it chases from where it stands while he is its target in clear sight;
    /// not another target, not a case walk.
    #[test]
    fn a_chaser_with_the_chased_player_in_sight_stands_and_fights() {
        let chase = SpyGoal::Chase(4, [100.0, 0.0, 0.0]);
        assert_eq!(chase_engages(Some(&chase), Some(4)), Some(4));
        assert_eq!(chase_engages(Some(&chase), Some(5)), None);
        assert_eq!(chase_engages(Some(&chase), None), None);
        assert_eq!(chase_engages(Some(&SpyGoal::Case([0.0; 3])), Some(4)), None);
        assert_eq!(chase_engages(None, Some(4)), None);
    }

    /// BOT-137: loose cases go to the players who hold some (a holder of n costs 1 / (1 + n) of his distance), and a
    /// holder falls back from a hunter within 400 units unless its case lies within 300; the agent and a bot without
    /// cases do not.
    #[test]
    fn loose_cases_go_to_holders_and_a_holder_keeps_away_from_hunters() {
        let c = [0.0, 0.0, 0.0];
        let hunter = (1u32, [300.0, 0.0, 0.0]);
        let holder = (2u32, [-800.0, 0.0, 0.0]);
        let held = |p: u32| if p == 2 { 2 } else { 0 };
        // the hunter is nearer, the holder of two takes it (800 / 3 < 300)
        assert_eq!(
            spread(&[c], |_| true, 1, hunter.1, &[holder], None, held),
            None
        );
        assert_eq!(
            spread(&[c], |_| true, 2, holder.1, &[hunter], None, held),
            Some(c)
        );
        // with nobody holding, the nearer one takes it
        assert_eq!(
            spread(&[c], |_| true, 1, hunter.1, &[holder], None, |_| 0),
            Some(c)
        );
        let at = [0.0, 0.0, 1000.0];
        let near = [[0.0, 0.0, 700.0], [0.0, 0.0, 2000.0]];
        assert_eq!(
            keep_away_from(3, 5, None, at, &near),
            Some([0.0, 0.0, 700.0])
        );
        assert_eq!(
            keep_away_from(4, 5, Some(&SpyGoal::Chase(3, [0.0; 3])), at, &near),
            Some([0.0, 0.0, 700.0])
        );
        // a far case with the hunter in its way (nearer the case than the holder): it falls back
        assert_eq!(
            keep_away_from(3, 5, Some(&SpyGoal::Case([0.0, 0.0, 0.0])), at, &near),
            Some([0.0, 0.0, 700.0])
        );
        // a far case the other way from the hunter: it runs for it
        assert_eq!(
            keep_away_from(3, 5, Some(&SpyGoal::Case([0.0, 0.0, 1800.0])), at, &near),
            None
        );
        // its case is near (BOT-140: within 600): it takes it first
        assert_eq!(
            keep_away_from(3, 5, Some(&SpyGoal::Case([500.0, 0.0, 1000.0])), at, &near),
            None
        );
        // BOT-140: a holder of fewer than three collects and fights; no case held, the agent, no hunter near
        assert_eq!(keep_away_from(2, 5, None, at, &near), None);
        assert_eq!(keep_away_from(0, 5, None, at, &near), None);
        assert_eq!(keep_away_from(5, 5, None, at, &near), None);
        assert_eq!(keep_away_from(3, 5, None, at, &near[1..]), None);
    }

    /// BOT-140: only the two hunters nearest a holder chase him, the others play on; a bigger holder draws hunters
    /// from farther; a second holder gets the next two; a spy without a case still chases the biggest holder.
    #[test]
    fn only_the_two_nearest_hunters_chase_a_holder() {
        let t = table(&[(12, uplink(), [0.0, 0.0, 3000.0])]);
        let holder = (9u32, [0.0, 0.0, 0.0]);
        let hunters = [
            (1u32, [100.0, 0.0, 0.0]),
            (2, [0.0, 0.0, 300.0]),
            (3, [-600.0, 0.0, 0.0]),
            (4, [0.0, 0.0, -900.0]),
        ];
        let goal_of = |me: u32, held: &[(u32, u32)]| {
            let mut s = SpyRun::default();
            s.on_bounty(&sub1(5), Some(me));
            for (p, n) in held {
                s.on_bounty(&sub0(*n, SPY_TEAM, *p), Some(me));
            }
            let all: Vec<(u32, [f32; 3])> = hunters.iter().copied().chain([holder]).collect();
            let at = all.iter().find(|p| p.0 == me).unwrap().1;
            let others: Vec<(u32, [f32; 3])> = all.iter().copied().filter(|p| p.0 != me).collect();
            s.goal(&t, Some(me), at, &others, 0)
        };
        let one = [(9u32, 2u32)];
        assert_eq!(goal_of(1, &one), Some(SpyGoal::Chase(9, holder.1)));
        assert_eq!(goal_of(2, &one), Some(SpyGoal::Chase(9, holder.1)));
        assert_eq!(goal_of(3, &one), None);
        assert_eq!(goal_of(4, &one), None);
        // the pairing itself: a second holder by hunter 4 takes him (and hunter 3, the nearer of the free ones)
        let holders = [(9u32, holder.1, 2u32), (8, [0.0, 0.0, -1000.0], 1)];
        let rest: Vec<(u32, [f32; 3])> = hunters.to_vec();
        let pick = |me: u32| {
            let at = hunters.iter().find(|h| h.0 == me).unwrap().1;
            chase_of(me, at, &rest, &holders).map(|c| c.0)
        };
        assert_eq!(pick(1), Some(9));
        assert_eq!(pick(2), Some(9));
        assert_eq!(pick(4), Some(8));
        assert_eq!(pick(3), Some(8));
        // a bigger holder far off draws the hunters before a small one near them
        let holders = [(9u32, [0.0, 0.0, 0.0], 1u32), (8, [0.0, 0.0, 900.0], 4)];
        assert_eq!(
            chase_of(2, [0.0, 0.0, 300.0], &[], &holders).map(|c| c.0),
            Some(8)
        );
        // a spy without a case of its own still goes after the holder of the most cases
        let mut spy = SpyRun::default();
        spy.on_bounty(&sub1(5), Some(3));
        spy.on_bounty(&sub0(1, SPY_TEAM, 3), Some(3));
        spy.on_bounty(&sub0(3, SPY_TEAM, 9), Some(3));
        assert_eq!(
            spy.goal(&t, Some(3), [-600.0, 0.0, 0.0], &[holder], 0),
            Some(SpyGoal::Chase(9, holder.1))
        );
    }

    /// BOT-127: a hunter does not shoot hunters (DSV-453), and a spy out of sight is chased to where he was last
    /// seen until the bot gets there or 30 s pass.
    #[test]
    fn a_spy_out_of_sight_is_chased_to_where_he_was_last_seen() {
        let t = table(&[(12, uplink(), [0.0, 0.0, 3000.0])]);
        let mut s = SpyRun::default();
        s.on_bounty(&sub1(5), Some(3));
        s.on_bounty(&sub0(2, 2, 1), Some(3));
        assert_eq!(s.team_of(1), SPY_TEAM);
        assert_eq!(s.team_of(4), HUNTER_TEAM);
        let spy = [900.0, 0.0, 0.0];
        assert_eq!(
            s.goal(&t, Some(3), [0.0; 3], &[(1, spy)], 0),
            Some(SpyGoal::Chase(1, spy))
        );
        assert_eq!(
            s.goal(&t, Some(3), [0.0; 3], &[], 1000),
            Some(SpyGoal::Chase(1, spy))
        );
        assert_eq!(s.goal(&t, Some(3), [850.0, 0.0, 0.0], &[], 2000), None);
        s.goal(&t, Some(3), [0.0; 3], &[(1, spy)], 3000);
        assert_eq!(
            s.goal(&t, Some(3), [0.0; 3], &[], 32_000),
            Some(SpyGoal::Chase(1, spy))
        );
        assert_eq!(s.goal(&t, Some(3), [0.0; 3], &[], 33_000), None);
    }

    /// BOT-155: a chase the bot gave up leaves that holder alone for 30 s (it takes another or plays on).
    #[test]
    fn a_chase_given_up_leaves_that_holder_alone_for_a_while() {
        let t = table(&[(12, uplink(), [0.0, 0.0, 3000.0])]);
        let mut s = SpyRun::default();
        s.on_bounty(&sub1(5), Some(3));
        s.on_bounty(&sub0(2, 2, 1), Some(3));
        s.on_bounty(&sub0(1, 2, 2), Some(3));
        let spies = [(1, [900.0, 0.0, 0.0]), (2, [0.0, 0.0, 1500.0])];
        assert_eq!(
            s.goal(&t, Some(3), [0.0; 3], &spies, 0),
            Some(SpyGoal::Chase(1, spies[0].1))
        );
        s.drop_chase(1, 1000);
        assert!(s.chase_dropped(1, 30_999));
        assert_eq!(
            s.goal(&t, Some(3), [0.0; 3], &spies, 1000),
            Some(SpyGoal::Chase(2, spies[1].1))
        );
        assert_eq!(
            s.goal(&t, Some(3), [0.0; 3], &spies, 31_000),
            Some(SpyGoal::Chase(1, spies[0].1))
        );
    }

    /// BOT-152: a holder of 4 of 5 waits for the last case while a hunter is near it (not for one near only the
    /// holder), unless another holder is about to take it or he waited 10 s; the waiting place lies off the case,
    /// away from the hunter.
    #[test]
    fn a_holder_one_short_waits_for_the_last_case_while_hunters_are_near() {
        let c = [0.0, 0.0, 0.0];
        let at = [1000.0, 0.0, 0.0];
        let near_case = [[300.0, 0.0, 300.0]];
        let near_me = [[1400.0, 0.0, 0.0]];
        let far = [[-3000.0, 0.0, 0.0]];
        assert!(fifth_waits(4, 5, c, &near_case, &[], 0));
        assert!(!fifth_waits(4, 5, c, &near_me, &[], 0));
        assert!(!fifth_waits(4, 5, c, &far, &[], 0));
        assert!(!fifth_waits(4, 5, c, &near_case, &[], 10_000));
        assert!(fifth_waits(4, 5, c, &near_case, &[], 9_000));
        assert!(!fifth_waits(4, 5, c, &near_case, &[[200.0, 0.0, 0.0]], 0));
        // a holder of 3 and the agent do not wait
        assert!(!fifth_waits(3, 5, c, &near_case, &[], 0));
        assert!(!fifth_waits(5, 5, c, &near_case, &[], 0));
        // the place off the case: within the ring, reached before the hunter
        let nodes = [
            [500.0, 0.0, 0.0],
            [-500.0, 0.0, 0.0],
            [100.0, 0.0, 0.0],
            [0.0, 0.0, 600.0],
        ];
        assert_eq!(
            safe_place(&nodes, c, FIFTH_RING, at, &[[-400.0, 0.0, 100.0]]),
            Some([500.0, 0.0, 0.0])
        );
    }

    /// BOT-151: while there is an agent the two hunters nearest him chase him, the next two by player id guard the
    /// uplink, the rest play on; the agent waits away from the hunters and sets off so as to reach the uplink as it
    /// opens.
    #[test]
    fn the_nearest_hunters_chase_the_agent_the_next_guard_the_uplink_and_the_agent_waits() {
        let up = [0.0, 0.0, 3000.0];
        let t = table(&[(12, uplink(), up)]);
        let agent_at = [0.0, 0.0, 0.0];
        // hunters 1..7 at 100 x pid from the agent; the agent is player 8
        let pos: Vec<(u32, [f32; 3])> = (1..=7u32)
            .map(|p| (p, [100.0 * p as f32, 0.0, 0.0]))
            .collect();
        let roster: Vec<u32> = (1..=8).collect();
        let job = |me: u32| {
            let at = pos.iter().find(|p| p.0 == me).unwrap().1;
            let hunters: Vec<(u32, [f32; 3])> = pos.iter().copied().filter(|p| p.0 != me).collect();
            let r: Vec<u32> = roster
                .iter()
                .copied()
                .filter(|p| *p != 8 && *p != me)
                .collect();
            agent_job(me, at, &hunters, &r, Some(agent_at))
        };
        assert_eq!(job(1), AgentJob::Chase);
        assert_eq!(job(2), AgentJob::Chase);
        assert_eq!(job(3), AgentJob::Guard(0));
        assert_eq!(job(4), AgentJob::Guard(1));
        assert_eq!(job(5), AgentJob::Free);
        // the agent's place unknown: nobody chases, the first two guard
        assert_eq!(
            agent_job(1, [0.0; 3], &[], &[2, 3], None),
            AgentJob::Guard(0)
        );
        // through the goal: hunter 4 guards approach 1 of the uplink
        let mut s = SpyRun::default();
        s.on_bounty(&sub1(5), Some(4));
        s.on_bounty(&sub0(5, 2, 8), Some(4));
        s.roster = roster.iter().copied().filter(|p| *p != 4).collect();
        let others: Vec<(u32, [f32; 3])> = pos
            .iter()
            .copied()
            .filter(|p| p.0 != 4)
            .chain(std::iter::once((8, agent_at)))
            .collect();
        match s.goal(&t, Some(4), [400.0, 0.0, 0.0], &others, 1000) {
            Some(SpyGoal::Guard(1, u, _)) => assert_eq!(u, up),
            g => panic!("{g:?}"),
        }
        assert_eq!(s.stage_ms(11_000), Some(10_000));
        // the guard's place: the grid node nearest its angle round the uplink, within the ring
        let nodes = [
            [0.0, 0.0, 3600.0],
            [600.0, 0.0, 3000.0],
            [-600.0, 0.0, 3000.0],
            [0.0, 0.0, 3100.0],
            [0.0, 900.0, 2400.0],
        ];
        let base = angle_of(up, agent_at);
        // two guards half a turn apart: the second faces away from where the agent was
        assert_eq!(guard_place(&nodes, up, base, 1), Some([0.0, 0.0, 3600.0]));
        assert_eq!(
            guard_place(&nodes[..3], up, 0.0, 0),
            Some([600.0, 0.0, 3000.0])
        );
        assert_eq!(
            guard_place(&nodes[..3], up, std::f32::consts::FRAC_PI_2, 0),
            Some([0.0, 0.0, 3600.0])
        );
        assert_eq!(guard_place(&nodes[3..], up, 0.0, 0), None);
        // the agent's waiting place: in the ring round the uplink, away from the hunter
        let hunter = [700.0, 0.0, 3000.0];
        let lurk = [
            [800.0, 0.0, 3000.0],
            [-800.0, 0.0, 3000.0],
            [-100.0, 0.0, 3000.0],
            [-2500.0, 0.0, 3000.0],
        ];
        assert_eq!(
            lurk_place(&lurk, up, [0.0, 0.0, 2000.0], &[hunter]),
            Some([-800.0, 0.0, 3000.0])
        );
        assert_eq!(lurk_place(&lurk[2..], up, [0.0; 3], &[hunter]), None);
        // the agent 1314 units off sets off with 30.6 s gone (8.4 s walk, 3 s early), not before
        let mut a = SpyRun::default();
        a.on_bounty(&sub1(5), Some(8));
        a.on_bounty(&sub0(5, 2, 8), Some(8));
        assert_eq!(
            a.goal(&t, Some(8), [0.0, 0.0, 1686.0], &[], 0),
            Some(SpyGoal::Uplink(up))
        );
        assert!(!a.agent_sets_off(28_000, [0.0, 0.0, 1686.0], up, None));
        assert!(a.agent_sets_off(29_000, [0.0, 0.0, 1686.0], up, None));
        assert!(a.agent_sets_off(40_000, [0.0, 0.0, 2900.0], up, None));
        // a way round twice as long as the flat distance x 1.4 sets it off earlier
        assert!(a.agent_sets_off(21_000, [0.0, 0.0, 1686.0], up, Some(3679.0)));
        assert!(!a.agent_sets_off(19_000, [0.0, 0.0, 1686.0], up, Some(3679.0)));
        // the stage ends: the clock is gone
        let mut end = BitWriter::new();
        end.write_u8(5).write_u8(0);
        a.on_bounty(&end, Some(8));
        assert_eq!(a.stage_ms(50_000), None);
    }
}
