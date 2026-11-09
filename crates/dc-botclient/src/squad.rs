//! Team jobs in the bomb modes (BOT-122), sans-IO: every bot of a team works out the same job list from the team's
//! roster (the player ids in order, turned by the round number), so no server help and no talk between the bots is
//! needed. Attackers: a lead who walks straight to the round's site, one or two escorts who trail him at a distance,
//! flankers who walk a side way (a waypoint off the straight line) and, with enough of them, one who goes for the
//! other site. Defenders: split over the sites, an odd one out rotating to the site with fewer defenders standing.
//! A bot moving to the same place as a teammate ahead of it keeps a spacing, and the jobs leave the spawn one after
//! another. A DeCombat bot behaviour, no 2009 rule.
// DC-DEFAULT: BOT-122 (proposed)

use crate::roles::{Objective, BOMB_GUARD, RESUME_MARGIN, SITE_HOLD};

/// A bot's job in a bomb round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// Attacker: straight to the round's site and plants there.
    Lead,
    /// Attacker: trails the lead (the first living one of lead and escorts) at [`ESCORT_NEAR`]..[`ESCORT_FAR`].
    Escort(u8),
    /// Attacker: to the round's site by a side way (the `n`th flank waypoint), plants when it gets there first.
    Flank(u8),
    /// Attacker: goes for the other site and plants there if nobody stops it.
    Feint,
    /// Defender: holds the site of this index (in plant point order).
    Hold(u8),
    /// Defender: holds the site with fewer living holders, between the sites in turn while they are even.
    Rotate,
}

/// A teammate following another one to the same place waits while it is closer than this to the one ahead (and
/// walks on once it is [`SPACING_RESUME`] away): a burst or a grenade aimed at one bot does not reach the next, and
/// the next is still near enough to see and cover the first (at the bots' run speed about one second behind it).
// DC-DEFAULT: BOT-122 (proposed) 256 units (8 body widths; the bomb guard radius of BOT-103), resume at 320
pub const SPACING: f32 = 256.0;
pub const SPACING_RESUME: f32 = 320.0;
/// A wait behind a teammate that is still on its way (not at the objective) lasts at most this long; then the
/// spacing is off for [`SPACING_RELEASE_MS`] and the bot passes (the one ahead may be stuck).
// DC-DEFAULT: BOT-122 (proposed) 4 s wait, 3 s release
pub const SPACING_WAIT_MAX_MS: u32 = 4000;
pub const SPACING_RELEASE_MS: u32 = 3000;
/// An escort walks toward the lead while it is farther than [`ESCORT_FAR`] and stops once within [`ESCORT_NEAR`].
// DC-DEFAULT: BOT-122 (proposed) 320..448 units
pub const ESCORT_NEAR: f32 = 320.0;
pub const ESCORT_FAR: f32 = 448.0;
/// A flank waypoint counts as passed this close, once the bot is nearer the site than the waypoint is, or when the
/// bot gets no [`VIA_GAIN`] units closer to it in [`VIA_STALL_MS`].
// DC-DEFAULT: BOT-122 (proposed) 192 units; 64 units in 6 s
pub const VIA_REACH: f32 = 192.0;
pub const VIA_GAIN: f32 = 64.0;
pub const VIA_STALL_MS: u32 = 6000;
/// No flank waypoint when the site is nearer than this to where the bot started the round.
// DC-DEFAULT: BOT-122 (proposed) 640 units
pub const FLANK_MIN_LEG: f32 = 640.0;
/// A rotating defender between sites with even holders changes site this often.
// DC-DEFAULT: BOT-122 (proposed) 25 s
pub const ROTATE_MS: u32 = 25_000;
/// A bot this close to its plant or defuse point never waits (it is at the point; the hold is bombrun's).
// DC-DEFAULT: BOT-122 (proposed) twice the plant reach of BOT-70
pub const NO_WAIT_NEAR: f32 = 2.0 * crate::bombrun::PLANT_REACH;
/// A bot whose last order is older than this starts the round again (it was dead, paused or between rounds).
pub const GAP_MS: u32 = 3000;
/// After the plant this many living defenders nearest the bomb (flat distance to its defuse point, ties the lower
/// player id) run straight to it and hold the defuse, with no spacing wait; the rest cover them, standing within
/// [`BOMB_GUARD`] of the bomb. A defuser that dies hands the job to the next nearest.
// DC-DEFAULT: BOT-124 (proposed) 2 (one holds, one beside him takes over when he falls or the hold is refused)
pub const DEFUSERS: usize = 2;
/// BOT-129: two teammates whose distances to the shared point differ by no more than this are level: the lower
/// player id is ahead, so the two never each wait for the other (before, streamed places a little old let each see
/// the other ahead and both stood).
// DC-DEFAULT: BOT-129 (proposed) 64 units
pub const SPACING_TIE: f32 = 64.0;

/// BOT-136: a defender holds an area, not a point: its posts are up to [`POSTS_MAX`] nodes of its way grid
/// [`POST_MIN`]..[`POST_MAX`] units (flat) from the site, at most [`POST_RISE`] higher or lower, at least
/// [`POST_SPREAD`] apart, those that see the site first (a node that does not see it only while fewer than two do).
// DC-DEFAULT: BOT-136 (proposed) 2..4 posts 128..480 units from the site, 192 apart, 160 up or down
pub const POST_MIN: f32 = 128.0;
pub const POST_MAX: f32 = 480.0;
pub const POST_IDEAL: f32 = 320.0;
pub const POST_RISE: f32 = 160.0;
pub const POST_SPREAD: f32 = 192.0;
pub const POSTS_MAX: usize = 4;
/// At most this many candidate nodes are tested for a sight line to the site.
pub const POST_SIGHT_TESTS: usize = 48;
/// The bot stands within this of its post (BOT-103's stand rule) and counts as there within [`POST_ARRIVE`].
pub const POST_STAND: f32 = 48.0;
pub const POST_ARRIVE: f32 = 112.0;
/// A defender stays at a post for 6..20 s (per bot, round and move), then moves to the next one; a post it does
/// not reach in [`POST_REACH_MS`] is passed over.
// DC-DEFAULT: BOT-136 (proposed) 15 s to reach a post; BOT-173 (proposed, amends BOT-136's 10..25 s) 6..20 s dwell
// (the guards' random dwell, crate::cover)
pub const POST_DWELL_MIN_MS: u32 = crate::cover::DWELL_MIN_MS;
pub const POST_DWELL_SPAN_MS: u32 = crate::cover::DWELL_SPAN_MS;
pub const POST_REACH_MS: u32 = 15_000;
/// At a post the bot peeks: every 5..9 s a step to a node 64..200 units from the post (for at most 2.5 s), then
/// back; and at two of three posts it crouches from 1.5 s after it got there, for 3..5 s.
// DC-DEFAULT: BOT-136 (proposed)
pub const PEEK_GAP_MIN_MS: u32 = 5000;
pub const PEEK_GAP_SPAN_MS: u32 = 4000;
pub const PEEK_MS: u32 = 2500;
pub const PEEK_STEP: f32 = 112.0;
pub const PEEK_NEAR: f32 = 64.0;
pub const PEEK_FAR: f32 = 200.0;
pub const CROUCH_AFTER_MS: u32 = 1500;
pub const CROUCH_MIN_MS: u32 = 3000;
pub const CROUCH_SPAN_MS: u32 = 2000;
/// A teammate within this of a post holds it: the bot picks another.
pub const POST_TAKEN: f32 = 128.0;
/// A walk to a post keeps no spacing once the bot is this close to the site (the posts are apart by themselves).
pub const POST_AREA: f32 = 640.0;
/// A quiet round: the bot saw no enemy for this long (or since its round start). Then the last living holder of a
/// site that another living holder keeps looks at the other site's approach (its post nearest the bot's own site)
/// for [`CHECK_DWELL_MS`] and comes back; never more often than once per quiet stretch, at most [`CHECK_MAX_MS`].
// DC-DEFAULT: BOT-136 (proposed) 45 s quiet, 6 s look, 30 s at most
pub const QUIET_MS: u32 = 45_000;
pub const CHECK_DWELL_MS: u32 = 6000;
pub const CHECK_MAX_MS: u32 = 30_000;
/// A waiting attacker (escort band, spacing) shifts: every 5..10 s a step to a node 48..192 units from where its
/// wait began, for at most 2.5 s.
// DC-DEFAULT: BOT-136 (proposed)
pub const SHIFT_GAP_MIN_MS: u32 = 5000;
pub const SHIFT_GAP_SPAN_MS: u32 = 5000;
pub const SHIFT_MS: u32 = 2500;
pub const SHIFT_STEP: f32 = 112.0;
pub const SHIFT_NEAR: f32 = 48.0;
pub const SHIFT_FAR: f32 = 192.0;
/// BOT-136 (bot evaluation 2026-11-07, bomb blasts): after the plant the attacking guards stand out of the blast,
/// on a ring [`BLAST_CLEAR`]..[`BLAST_RING_MAX`] units (flat) from the bomb with a sight line to it, at most
/// [`BLAST_RING_RISE`] higher or lower; they move between those places only while more than [`RING_STILL_MS`] of
/// the fuse is left. A defender that cannot finish a defuse before the blast (less than the defuse time and
/// [`FLEE_MARGIN_MS`] left, and no own hold that ends in time) lets go and runs to the nearest of those places.
// DC-DEFAULT: BOT-136 (proposed) the server blast radius 500 (GMD-9) plus 150; ring to 1100 units, 6 places,
// 300 up or down; defuse 7 s (GMD-6) + 1 s; no ring moves in the last 12 s
pub const BLAST_RADIUS: f32 = 500.0;
pub const BLAST_CLEAR: f32 = BLAST_RADIUS + 150.0;
pub const BLAST_RING_MAX: f32 = 1100.0;
pub const BLAST_RING_RISE: f32 = 300.0;
pub const BLAST_POSTS_MAX: usize = 6;
pub const DEFUSE_MS: u32 = 7000;
pub const FLEE_MARGIN_MS: u32 = 1000;
pub const RING_STILL_MS: u32 = 12_000;
/// The key of a post area around the planted bomb (site index + this).
const BOMB_KEY: u32 = 100;
const CHECK_KEY: u32 = 200;
/// The keys of the blast ring: the attackers' guard places and a defender's run from the blast.
const RING_KEY: u32 = 300;
const FLEE_KEY: u32 = 400;
/// BOT-142: the key of the attackers' guard places round the planted bomb.
const GUARD_KEY: u32 = 500;
/// BOT-142 (bot evaluation 2026-11-08, "after the plant the attackers look like bots": of 27 plants 16 defused,
/// the guards 600..1100 units out firing 0..3 shots): after the plant the attackers guard the bomb from
/// [`GUARD_MIN`]..[`GUARD_MAX`] units (flat), at most [`GUARD_RISE`] up or down, only from places with a sight line
/// from the eye to a crouched defuser's head ([`DEFUSER_HEAD`] over the defuse point), up to [`GUARDS_MAX`] places
/// [`GUARD_SPREAD`] apart (two or three angles), no peeks; with [`LEAVE_MS`] of the fuse left they leave for the
/// blast ring (BOT-136). A defender seen at the bomb (a streamed living enemy within [`DEFUSER_NEAR`] flat and
/// `bombrun::PLANT_RISE` up or down of the defuse point) is a defuse: while more than [`CONTEST_MIN_MS`] of the
/// fuse is left the nearest living attacker pushes him (walks to his place) and the others hold their guard
/// places facing him, also past [`LEAVE_MS`].
// DC-DEFAULT: BOT-142 (proposed) guard 250..500 units, 160 up or down, 3 places 256 apart, crouched head 48 over
// the defuse point (a standing defuser's body centre is 52 over it on Gray Hammer A), leave at 10 s, contest to
// 4 s, a defuser within 64 units
pub const GUARD_MIN: f32 = 250.0;
pub const GUARD_MAX: f32 = 500.0;
pub const GUARD_RISE: f32 = 160.0;
pub const GUARD_SPREAD: f32 = 256.0;
pub const GUARDS_MAX: usize = 3;
pub const DEFUSER_HEAD: f32 = 48.0;
pub const LEAVE_MS: u32 = 10_000;
pub const CONTEST_MIN_MS: u32 = 4000;
pub const DEFUSER_NEAR: f32 = 64.0;
/// BOT-142: the pusher stands this close to the defuser.
pub const PUSH_STAND: f32 = 48.0;
/// BOT-142: on the way out of the blast (late in the fuse) a place not reached in this time while the bot is still
/// within [`BLAST_CLEAR`] of the bomb is given up for the next one round the ring (on Gray Hammer A a guard stuck
/// on the way to the place nearest it died in the blast twice).
// DC-DEFAULT: BOT-142 (proposed) 4 s
pub const LEAVE_SWITCH_MS: u32 = 4000;

/// A well-mixed 64-bit number from `x` (SplitMix64), for the per-bot timings.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn seed(pid: u8, round: u32, key: u32, k: u32) -> u64 {
    mix(u64::from(pid) | u64::from(round) << 8 | u64::from(key) << 24 | u64::from(k) << 40)
}

/// BOT-136: how long the bot stays at its `k`th post of a round (10..25 s).
pub fn post_dwell_ms(pid: u8, round: u32, key: u32, k: u32) -> u32 {
    POST_DWELL_MIN_MS + (seed(pid, round, key, k) % u64::from(POST_DWELL_SPAN_MS + 1)) as u32
}

/// BOT-136: the posts round `site` among the grid's `nodes` (`sees`: node to site), in the order of their angle
/// round the site (the next post is a neighbour). See [`POST_MIN`].
// DC-DEFAULT: BOT-136 (proposed)
pub fn post_set(
    site: [f32; 3],
    nodes: &[[f32; 3]],
    sees: impl FnMut([f32; 3], [f32; 3]) -> bool,
) -> Vec<[f32; 3]> {
    post_ring(site, nodes, Ring::SITE, sees)
}

/// BOT-136: the bounds of a ring of posts round a place.
#[derive(Debug, Clone, Copy)]
pub struct Ring {
    pub min: f32,
    pub max: f32,
    pub ideal: f32,
    pub rise: f32,
    pub count: usize,
    /// Posts at least this far apart.
    pub spread: f32,
    /// Posts without a sight line are taken while fewer than this many see the centre.
    pub blind: usize,
}

impl Ring {
    /// BOT-173: the ring's band for the cover score.
    pub fn band(&self) -> crate::cover::Band {
        crate::cover::Band {
            min: self.min,
            max: self.max,
            ideal: self.ideal,
            rise: self.rise,
        }
    }

    /// The posts round a site (see [`POST_MIN`]).
    pub const SITE: Ring = Ring {
        min: POST_MIN,
        max: POST_MAX,
        ideal: POST_IDEAL,
        rise: POST_RISE,
        count: POSTS_MAX,
        spread: POST_SPREAD,
        blind: 2,
    };
    /// The places out of the planted bomb's blast (see [`BLAST_CLEAR`]).
    pub const BLAST: Ring = Ring {
        min: BLAST_CLEAR,
        max: BLAST_RING_MAX,
        ideal: 0.5 * (BLAST_CLEAR + BLAST_RING_MAX),
        rise: BLAST_RING_RISE,
        count: BLAST_POSTS_MAX,
        spread: POST_SPREAD,
        blind: 2,
    };
    /// BOT-142: the attackers' guard places round the planted bomb (see [`GUARD_MIN`]); seeing ones only.
    pub const GUARD: Ring = Ring {
        min: GUARD_MIN,
        max: GUARD_MAX,
        ideal: 0.5 * (GUARD_MIN + GUARD_MAX),
        rise: GUARD_RISE,
        count: GUARDS_MAX,
        spread: GUARD_SPREAD,
        blind: 0,
    };
}

/// BOT-136: the posts of `ring` round `center` among the grid's `nodes` (`sees`: node to center), in the order of
/// their angle round it.
pub fn post_ring(
    center: [f32; 3],
    nodes: &[[f32; 3]],
    ring: Ring,
    sees: impl FnMut([f32; 3], [f32; 3]) -> bool,
) -> Vec<[f32; 3]> {
    post_ring_scored(center, nodes, ring, sees, |_| 0.0)
}

/// BOT-173 (amends BOT-136 and BOT-142): [`post_ring`] with the tested nodes that see the centre taken in the order
/// of their cover score (`cover`, [`crate::cover::score`], higher first; equal scores keep the nearest-the-ideal
/// order), so a post by a wall with sight of the approaches comes before one in the open. Before, the nearest the
/// ideal distance first.
// DC-DEFAULT: BOT-173 (proposed)
pub fn post_ring_scored(
    center: [f32; 3],
    nodes: &[[f32; 3]],
    ring: Ring,
    mut sees: impl FnMut([f32; 3], [f32; 3]) -> bool,
    mut cover: impl FnMut([f32; 3]) -> f32,
) -> Vec<[f32; 3]> {
    let site = center;
    let mut cand: Vec<([f32; 3], f32)> = nodes
        .iter()
        .filter(|n| {
            let d = flat(**n, site);
            (ring.min..=ring.max).contains(&d) && (n[1] - site[1]).abs() <= ring.rise
        })
        .map(|n| (*n, (flat(*n, site) - ring.ideal).abs()))
        .collect();
    cand.sort_by(|a, b| {
        a.1.total_cmp(&b.1)
            .then(a.0[0].total_cmp(&b.0[0]))
            .then(a.0[2].total_cmp(&b.0[2]))
    });
    // spread the tests: a node next to one already tested adds little
    let mut tested: Vec<([f32; 3], bool)> = Vec::new();
    for (n, _) in cand {
        if tested.len() >= POST_SIGHT_TESTS {
            break;
        }
        if tested.iter().any(|(t, _)| flat(*t, n) < 0.5 * ring.spread) {
            continue;
        }
        tested.push((n, sees(n, site)));
    }
    let scores: Vec<f32> = tested
        .iter()
        .map(|(n, s)| if *s { cover(*n) } else { 0.0 })
        .collect();
    let mut order: Vec<usize> = (0..tested.len()).collect();
    order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
    let tested: Vec<([f32; 3], bool)> = order.into_iter().map(|i| tested[i]).collect();
    let mut picked: Vec<[f32; 3]> = Vec::new();
    for want_sight in [true, false] {
        for (n, s) in &tested {
            let room = if want_sight { ring.count } else { ring.blind };
            if *s != want_sight || picked.len() >= room {
                continue;
            }
            if picked.iter().all(|p| flat(*p, *n) >= ring.spread) {
                picked.push(*n);
            }
        }
    }
    let angle = |p: &[f32; 3]| (p[0] - site[0]).atan2(p[2] - site[2]);
    picked.sort_by(|a, b| angle(a).total_cmp(&angle(b)));
    picked
}

/// BOT-136: a node `near..=far` units (flat) from `from`, at most 64 higher or lower and not where the bot stands:
/// the grid node nearest to a point `step` units away in a direction picked by `h`, the other five sixths of the
/// turn tried in order when that gives none (a grid known only along the ways walked has few nodes off them).
fn step_near(
    h: u64,
    from: [f32; 3],
    step: f32,
    near: f32,
    far: f32,
    me: [f32; 3],
    snap: &mut dyn FnMut([f32; 3]) -> Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    let a0 = (h % 3600) as f32 * std::f32::consts::TAU / 3600.0;
    (0..6).find_map(|i| {
        let a = a0 + i as f32 * std::f32::consts::TAU / 6.0;
        let target = [from[0] + step * a.sin(), from[1], from[2] + step * a.cos()];
        snap(target).filter(|n| {
            (near..=far).contains(&flat(*n, from))
                && (n[1] - from[1]).abs() <= 64.0
                && flat(*n, me) > 0.75 * near
        })
    })
}

/// The bot's post in an area (BOT-136).
#[derive(Debug, Clone, Copy)]
struct Post {
    key: u32,
    at: [f32; 3],
    /// Posts taken so far in this area (the dwell and the crouch go by it).
    k: u32,
    since: u32,
    arrived: Option<u32>,
    peek: Option<([f32; 3], u32)>,
    next_peek: u32,
}

/// A waiting attacker's shift (BOT-136).
#[derive(Debug, Clone, Copy)]
struct Shift {
    anchor: [f32; 3],
    k: u32,
    next: u32,
    step: Option<([f32; 3], u32)>,
}

/// What the post walk gives this tick.
struct PostStep {
    goal: Objective,
    crouch: bool,
    watch: Option<[f32; 3]>,
    peek: bool,
}

/// The attackers' jobs for a team of `n` (job order, before the round's turn): one lead, one escort up to four
/// players and two from five, the rest flankers; with three or more flankers and two sites the last one feints.
pub fn attack_jobs(n: usize, sites: usize) -> Vec<Job> {
    if n == 0 {
        return Vec::new();
    }
    let escorts = match n {
        1 => 0,
        2..=4 => 1,
        _ => 2,
    };
    let flankers = n - 1 - escorts;
    let feint = sites >= 2 && flankers >= 3;
    let mut jobs = vec![Job::Lead];
    jobs.extend((0..escorts).map(|k| Job::Escort(k as u8)));
    jobs.extend((0..flankers - usize::from(feint)).map(|k| Job::Flank(k as u8)));
    if feint {
        jobs.push(Job::Feint);
    }
    jobs
}

/// The defenders' jobs for a team of `n`: holders spread over the sites in turn; with an odd team (and more than
/// one site) the last one rotates.
pub fn defend_jobs(n: usize, sites: usize) -> Vec<Job> {
    let sites = sites.max(1);
    let rotate = sites >= 2 && n % 2 == 1;
    let holders = n - usize::from(rotate);
    let mut jobs: Vec<Job> = (0..holders).map(|j| Job::Hold((j % sites) as u8)).collect();
    if rotate {
        jobs.push(Job::Rotate);
    }
    jobs
}

/// The job of the team member at `index` (player id order) in round `round`: the job list turned by the round, so
/// a different bot leads each round.
pub fn job_of(index: usize, n: usize, round: u32, attacking: bool, sites: usize) -> Option<Job> {
    let jobs = if attacking {
        attack_jobs(n, sites)
    } else {
        defend_jobs(n, sites)
    };
    (index < n).then(|| jobs[(index + round as usize) % n])
}

/// How long after its round start a job stays at the spawn: the lead and the feint go first, the flankers in turn,
/// the escorts once the lead is ahead, the holders one by one, the rotator last.
// DC-DEFAULT: BOT-122 (proposed) lead / feint 0, flank 400 + 700 k, escort 1200 + 600 k, hold 500 per slot,
// rotate 2500 ms
pub fn start_delay_ms(job: Job, hold_slot: u32) -> u32 {
    match job {
        Job::Lead | Job::Feint => 0,
        Job::Flank(k) => 400 + 700 * u32::from(k),
        Job::Escort(k) => 1200 + 600 * u32::from(k),
        Job::Hold(_) => 500 * hold_slot,
        Job::Rotate => 2500,
    }
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// BOT-129: the spacing distance: across the floor plus the height difference (a teammate a floor below or above is
/// neither ahead on the way nor near).
// DC-DEFAULT: BOT-129 (proposed)
fn spaced(a: [f32; 3], b: [f32; 3]) -> f32 {
    flat(a, b) + (a[1] - b[1]).abs()
}

fn same(a: [f32; 3], b: [f32; 3]) -> bool {
    (a[0] - b[0]).abs() < 1.0 && (a[2] - b[2]).abs() < 1.0
}

/// The `k`th flank waypoint from `origin` to `site`: halfway along the line, to the side by 40 % of the line's
/// length (at least 384, at most 1536 units), the even ones to the left, the odd ones to the right, every second
/// pair half as far again. `None` when the site is nearer than [`FLANK_MIN_LEG`].
// DC-DEFAULT: BOT-122 (proposed)
pub fn flank_via(origin: [f32; 3], site: [f32; 3], k: u8) -> Option<[f32; 3]> {
    let (dx, dz) = (site[0] - origin[0], site[2] - origin[2]);
    let len = (dx * dx + dz * dz).sqrt();
    if len < FLANK_MIN_LEG {
        return None;
    }
    let side = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
    let off = (0.4 * len).clamp(384.0, 1536.0) * (1.0 + 0.5 * f32::from(k / 2)) * side;
    let (px, pz) = (-dz / len, dx / len);
    Some([
        origin[0] + dx * 0.5 + px * off,
        (origin[1] + site[1]) * 0.5,
        origin[2] + dz * 0.5 + pz * off,
    ])
}

/// A member of the bot's team as the bot sees it (the bot itself included).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mate {
    pub pid: u8,
    /// The streamed position (`None`: not streamed).
    pub pos: Option<[f32; 3]>,
    pub alive: bool,
}

/// What the bot knows about its team and the round.
#[derive(Debug, Clone, Default)]
pub struct Team {
    pub own_pid: u8,
    pub own_pos: [f32; 3],
    /// The team in player id order, the bot itself included.
    pub mates: Vec<Mate>,
    pub round: u32,
    pub attacking: bool,
    /// The plant points the bot can use (object id order, BOT-103).
    pub sites: Vec<[f32; 3]>,
    pub planted: bool,
    /// The planted bomb's defuse point.
    pub bomb_pos: Option<[f32; 3]>,
    /// BOT-136: the posts round each site (index as `sites`; empty or missing: none known, the BOT-103 hold).
    pub posts: Vec<Vec<[f32; 3]>>,
    /// BOT-136: when the bot last had an enemy in sight (`None`: not since it joined).
    pub enemy_seen: Option<u32>,
    /// BOT-136: after the plant, the places out of the blast ([`Ring::BLAST`] round the bomb; empty: none known),
    /// the fuse left (ms, `None`: not known) and whether the own defuse hold ends before the blast.
    pub blast_ring: Vec<[f32; 3]>,
    pub fuse_left: Option<u32>,
    pub hold_in_time: bool,
    /// BOT-142: after the plant, the attackers' guard places ([`Ring::GUARD`] round the bomb; empty: none known)
    /// and the place of a defender seen at the bomb (see [`DEFUSER_NEAR`]).
    pub guard_ring: Vec<[f32; 3]>,
    pub defuser: Option<[f32; 3]>,
}

impl Team {
    /// The index of the round's site (BOT-103) and of the other one.
    fn site_index(&self) -> usize {
        self.round as usize % self.sites.len().max(1)
    }

    fn other_index(&self) -> usize {
        (self.round as usize + 1) % self.sites.len().max(1)
    }

    fn job(&self, index: usize) -> Option<Job> {
        job_of(
            index,
            self.mates.len(),
            self.round,
            self.attacking,
            self.sites.len(),
        )
    }

    /// The point a member with `job` heads for, shared with every teammate with the same point (spacing is kept
    /// between those): the planted bomb, else the site of the job. `None` for the rotator (its pick is its own).
    fn shared_point(&self, job: Job) -> Option<[f32; 3]> {
        if self.planted {
            return self.bomb_pos;
        }
        let site = |i: usize| self.sites.get(i).copied();
        match job {
            Job::Lead | Job::Escort(_) | Job::Flank(_) => site(self.site_index()),
            Job::Feint => site(self.other_index()),
            Job::Hold(s) => site(usize::from(s)),
            Job::Rotate => None,
        }
    }

    /// BOT-124: the player ids of the [`DEFUSERS`] living members nearest `bomb` (the bot itself counts as living;
    /// a member without a streamed place does not count).
    // DC-DEFAULT: BOT-124 (proposed)
    fn defusers(&self, bomb: [f32; 3]) -> Vec<u8> {
        self.nearest_living(bomb, DEFUSERS)
    }

    /// BOT-142: the defuser the attackers contest: one is seen and more than [`CONTEST_MIN_MS`] of the fuse is left.
    fn contest(&self) -> Option<[f32; 3]> {
        self.defuser
            .filter(|_| self.fuse_left.is_none_or(|f| f > CONTEST_MIN_MS))
    }

    /// The player ids of the `n` living members nearest `at` (flat; ties the lower player id; the bot itself counts
    /// as living; a member without a streamed place does not count).
    fn nearest_living(&self, at: [f32; 3], n: usize) -> Vec<u8> {
        let bomb = at;
        let mut near: Vec<(f32, u8)> = self
            .mates
            .iter()
            .filter_map(|m| {
                let p = if m.pid == self.own_pid {
                    Some(self.own_pos)
                } else {
                    m.pos.filter(|_| m.alive)
                };
                p.map(|p| (flat(p, bomb), m.pid))
            })
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        near.into_iter().take(n).map(|n| n.1).collect()
    }

    /// BOT-136: whether a defender runs from the blast: less fuse is left than a defuse and the margin take, and
    /// no own hold ends in time.
    fn flee(&self) -> bool {
        self.planted
            && !self.hold_in_time
            && self
                .fuse_left
                .is_some_and(|f| f < DEFUSE_MS + FLEE_MARGIN_MS)
    }

    /// BOT-136: whether the member at `index` is the one of site `s`'s holders who looks at the other site in a
    /// quiet round: the last living holder of `s` in job order, with another living holder of `s` staying.
    // DC-DEFAULT: BOT-136 (proposed)
    fn checker(&self, index: usize, s: usize) -> bool {
        let holders: Vec<usize> = (0..self.mates.len())
            .filter(|&i| self.job(i) == Some(Job::Hold(s as u8)))
            .filter(|&i| self.mates[i].alive || self.mates[i].pid == self.own_pid)
            .collect();
        holders.len() >= 2 && holders.last() == Some(&index)
    }

    /// The order slot of a holder among the holders of its site (for the start delay).
    fn hold_slot(&self, index: usize, job: Job) -> u32 {
        (0..index).filter(|&i| self.job(i) == Some(job)).count() as u32
    }
}

/// The bot's order this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Order {
    pub job: Job,
    /// Attackers before the plant: the plant point this bot plants at.
    pub plant_at: Option<[f32; 3]>,
    /// Where the job walks to and how close it stands there.
    pub goal: Option<Objective>,
    /// The bot stands where it is (its start delay, an escort near the lead, the spacing to a teammate ahead).
    pub wait: bool,
    /// Why it waits (logs): "start", "escort", "spacing".
    pub why: &'static str,
    /// BOT-129: a "spacing" wait behind a teammate still on its way (not standing at the objective): the session's
    /// BOT-103 stall clock runs through it; every other wait stops that clock.
    pub stall_counts: bool,
    /// Defenders after the plant: the bot is one of the [`DEFUSERS`] (it walks to the defuse point and holds there);
    /// `false` for a cover and before the plant.
    pub defuse: bool,
    /// BOT-136: a waiting bot's shift: it walks to this place (its wait goes on).
    pub shift: Option<[f32; 3]>,
    /// BOT-136: the bot crouches at its post.
    pub crouch: bool,
    /// BOT-136: standing at its post the bot looks toward this place (the site, the bomb) while it has no target.
    pub watch: Option<[f32; 3]>,
    /// BOT-136: the goal is a post of the bot's area (logs: "post", "peek", "check"; "" otherwise).
    pub post: &'static str,
}

/// The bot's job state over a round.
#[derive(Debug, Clone, Default)]
pub struct Squad {
    round: Option<u32>,
    last: Option<u32>,
    started: Option<u32>,
    origin: Option<[f32; 3]>,
    job: Option<Job>,
    /// The flank waypoint: place, best distance and when it last got [`VIA_GAIN`] closer; `done` once passed.
    via: Option<([f32; 3], f32, u32)>,
    via_done: bool,
    escort_walk: bool,
    spacing_wait: bool,
    wait_since: Option<u32>,
    released_until: Option<u32>,
    post: Option<Post>,
    shift: Option<Shift>,
    /// BOT-136: the quiet-round look at the other site: when it began; when the last one ended.
    check: Option<u32>,
    check_done: Option<u32>,
    /// BOT-136: the posts not reached in [`POST_REACH_MS`] this round (left out from then on).
    failed: Vec<[f32; 3]>,
}

impl Squad {
    /// The bot's job this round.
    pub fn job(&self) -> Option<Job> {
        self.job
    }

    /// The flank waypoint the bot walks by (logs).
    pub fn via(&self) -> Option<[f32; 3]> {
        self.via.filter(|_| !self.via_done).map(|v| v.0)
    }

    /// The order for this tick, called while the bot lives in a running round; `snap` moves a waypoint onto the
    /// bot's way grid (the known node nearest to it), `None` leaves it where it is.
    pub fn order(
        &mut self,
        now: u32,
        t: &Team,
        snap: &mut dyn FnMut([f32; 3]) -> Option<[f32; 3]>,
    ) -> Option<Order> {
        let index = t.mates.iter().position(|m| m.pid == t.own_pid)?;
        if t.sites.is_empty() {
            return None;
        }
        let job = t.job(index)?;
        let fresh = self.round != Some(t.round)
            || self.last.is_none_or(|l| now.wrapping_sub(l) > GAP_MS)
            || self.job != Some(job);
        if fresh {
            *self = Squad {
                round: Some(t.round),
                started: Some(now),
                origin: Some(t.own_pos),
                job: Some(job),
                ..Squad::default()
            };
        }
        self.last = Some(now);
        let me = t.own_pos;
        let elapsed = now.wrapping_sub(self.started.unwrap_or(now));
        let site = t.sites[t.site_index()];
        let plant_at = (t.attacking && !t.planted).then(|| match job {
            Job::Feint => t.sites[t.other_index()],
            _ => site,
        });
        let mut order = Order {
            job,
            plant_at,
            goal: None,
            wait: false,
            why: "",
            stall_counts: false,
            defuse: false,
            shift: None,
            crouch: false,
            watch: None,
            post: "",
        };
        // the start delay holds the job at the spawn (not after a plant: then everybody runs)
        if !t.planted && elapsed < start_delay_ms(job, t.hold_slot(index, job)) {
            order.wait = true;
            order.why = "start";
            let point = plant_at
                .or(t.shared_point(job))
                .unwrap_or_else(|| self.rotate_site(t, elapsed));
            order.goal = Some(Objective {
                goal: point,
                stand_within: SITE_HOLD,
            });
            return Some(order);
        }
        let at = |goal: [f32; 3], stand_within: f32| Some(Objective { goal, stand_within });
        let mut posted: Option<(PostStep, &'static str)> = None;
        let mut push = false;
        order.goal = match (t.attacking, t.planted) {
            // BOT-142: the guards hold places 250..500 units out that see the bomb, leave for the blast ring late in
            // the fuse and contest a defuse (the nearest pushes); else BOT-136's posts round the bomb's site
            (true, true) => {
                let bomb = t.bomb_pos.unwrap_or(site);
                let contest = t.contest();
                if let Some(d) =
                    contest.filter(|_| t.nearest_living(bomb, 1).first() == Some(&t.own_pid))
                {
                    push = true;
                    self.post = None;
                    at(d, PUSH_STAND)
                } else {
                    let leave = contest.is_none() && t.fuse_left.is_some_and(|f| f <= LEAVE_MS);
                    if leave {
                        posted = self.ring_post(now, t, bomb, RING_KEY, snap);
                    } else {
                        posted = self.guard_post(now, t, bomb, contest, snap);
                    }
                    if posted.is_none() {
                        posted = self.bomb_post(now, t, bomb, snap);
                    }
                    if posted.is_none() && !leave {
                        posted = self.ring_post(now, t, bomb, RING_KEY, snap);
                    }
                    posted.as_ref().map(|p| p.0.goal).or(at(bomb, BOMB_GUARD))
                }
            }
            // BOT-124: the nearest defenders defuse, the rest cover them at the bomb
            (false, true) => t.bomb_pos.and_then(|b| {
                // BOT-136: too late to defuse: every defender runs from the blast
                if t.flee() {
                    posted = self.ring_post(now, t, b, FLEE_KEY, snap);
                    if let Some(p) = &mut posted {
                        p.1 = "flee";
                        return Some(p.0.goal);
                    }
                }
                order.defuse = t.defusers(b).contains(&t.own_pid);
                if order.defuse {
                    self.post = None;
                    return at(b, 0.0);
                }
                posted = self.bomb_post(now, t, b, snap);
                posted.as_ref().map(|p| p.0.goal).or(at(b, BOMB_GUARD))
            }),
            (true, false) => match job {
                Job::Escort(_) => match self.lead(t) {
                    Some(lead) if lead != t.own_pid => {
                        let lead_pos = t.mates.iter().find(|m| m.pid == lead).and_then(|m| m.pos);
                        match lead_pos {
                            Some(lp) => {
                                let d = flat(me, lp);
                                if d > ESCORT_FAR {
                                    self.escort_walk = true;
                                } else if d < ESCORT_NEAR {
                                    self.escort_walk = false;
                                }
                                if !self.escort_walk {
                                    order.wait = true;
                                    order.why = "escort";
                                }
                                at(lp, 0.0)
                            }
                            None => at(site, SITE_HOLD),
                        }
                    }
                    _ => at(site, SITE_HOLD),
                },
                Job::Flank(k) => match self.flank_step(now, me, site, k, snap) {
                    Some(v) => at(v, 0.0),
                    None => at(site, SITE_HOLD),
                },
                Job::Feint => at(t.sites[t.other_index()], SITE_HOLD),
                _ => at(site, SITE_HOLD),
            },
            // BOT-136: a holder holds an area: posts round its site in turn, a look at the other site when quiet
            (false, false) => {
                let s = match job {
                    Job::Hold(s) => usize::from(s) % t.sites.len(),
                    _ => self.rotate_index(t, elapsed),
                };
                posted = self.hold_post(now, t, index, job, s, snap);
                posted
                    .as_ref()
                    .map(|p| p.0.goal)
                    .or(at(t.sites[s], SITE_HOLD))
            }
        };
        if let Some((p, kind)) = &posted {
            order.crouch = p.crouch;
            order.watch = p.watch;
            order.post = kind;
        } else {
            self.post = None;
            if push {
                order.post = "push";
            }
        }
        // a defuser never waits for the teammate ahead (BOT-124): the spacing let every defender stand behind another;
        // BOT-129: nor does the lead on its way to plant (it is the one the others space behind); BOT-136: nor does a
        // bot walking between the posts of its area (the posts are apart by themselves)
        let lead_run = job == Job::Lead && t.attacking && !t.planted;
        if !order.wait && !order.defuse && !lead_run && !push {
            if let Some(shared) = order.goal.map(|g| match job {
                // the escort and the flanker keep the spacing on the way to their site, not to their waypoint
                Job::Escort(_) | Job::Flank(_) if !t.planted => site,
                _ => g.goal,
            }) {
                let in_area = posted.is_some() && spaced(me, shared) <= POST_AREA;
                if in_area {
                    self.spacing_wait = false;
                    self.wait_since = None;
                } else if let Some(settled) = self.spacing(now, t, index, shared) {
                    order.wait = true;
                    order.why = "spacing";
                    order.stall_counts = !settled;
                }
            }
        }
        // BOT-136: a waiting bot (not at its start) shifts a few steps now and then
        if order.wait && order.why != "start" {
            order.shift = self.shift_step(now, t, snap);
        } else {
            self.shift = None;
        }
        Some(order)
    }

    /// BOT-136: the post of a holder of site `s` (`None`: no posts known there, the BOT-103 hold), or its look at
    /// the other site's approach in a quiet round.
    // DC-DEFAULT: BOT-136 (proposed)
    fn hold_post(
        &mut self,
        now: u32,
        t: &Team,
        index: usize,
        job: Job,
        s: usize,
        snap: &mut dyn FnMut([f32; 3]) -> Option<[f32; 3]>,
    ) -> Option<(PostStep, &'static str)> {
        let n = t.sites.len();
        if n >= 2 && matches!(job, Job::Hold(_)) {
            let other = (s + 1) % n;
            let started = self.started.unwrap_or(now);
            let quiet_from = match t.enemy_seen {
                Some(e) if (e.wrapping_sub(started) as i32) > 0 => e,
                _ => started,
            };
            let quiet = now.wrapping_sub(quiet_from);
            let other_posts = t.posts.get(other).filter(|p| !p.is_empty());
            let key = CHECK_KEY + other as u32;
            if let Some(c) = self.check {
                let seen = t.enemy_seen.is_some_and(|e| (e.wrapping_sub(c) as i32) > 0);
                let looked = self.post.is_some_and(|p| {
                    p.key == key
                        && p.arrived
                            .is_some_and(|a| now.wrapping_sub(a) >= CHECK_DWELL_MS)
                });
                if seen || looked || now.wrapping_sub(c) >= CHECK_MAX_MS || other_posts.is_none() {
                    self.check = None;
                    self.check_done = Some(now);
                }
            } else if quiet >= QUIET_MS
                && other_posts.is_some()
                && self
                    .check_done
                    .is_none_or(|d| now.wrapping_sub(d) >= QUIET_MS)
                && t.checker(index, s)
            {
                self.check = Some(now);
            }
            if let (Some(_), Some(posts)) = (self.check, other_posts) {
                let own_site = t.sites[s];
                let step =
                    self.post_step(now, t, key, posts, 0, Some(own_site), t.sites[other], snap);
                match step {
                    Some(step) => return Some((step, "check")),
                    None => {
                        self.check = None;
                        self.check_done = Some(now);
                    }
                }
            }
        }
        let posts = t.posts.get(s).filter(|p| !p.is_empty())?;
        let slot = match job {
            Job::Rotate => 1,
            _ => t.hold_slot(index, job) as usize,
        };
        let step = self.post_step(now, t, s as u32, posts, slot, None, t.sites[s], snap)?;
        let kind = if step.peek { "peek" } else { "post" };
        Some((step, kind))
    }

    /// BOT-136: a place on the blast ring round the planted `bomb` (`None`: none known): the guards take the places
    /// in turn (the first one the free place nearest the bot), a fleeing defender the one nearest it.
    fn ring_post(
        &mut self,
        now: u32,
        t: &Team,
        bomb: [f32; 3],
        key: u32,
        snap: &mut dyn FnMut([f32; 3]) -> Option<[f32; 3]>,
    ) -> Option<(PostStep, &'static str)> {
        if t.blast_ring.is_empty() {
            return None;
        }
        let ring = t.blast_ring.clone();
        let step = self.post_step(now, t, key, &ring, 0, Some(t.own_pos), bomb, snap)?;
        Some((step, "ring"))
    }

    /// BOT-142: a guard place round the planted `bomb` (`None`: none known), the first one the free place nearest
    /// the bot; it faces a seen `defuser` (else the bomb) while it stands there.
    // DC-DEFAULT: BOT-142 (proposed)
    fn guard_post(
        &mut self,
        now: u32,
        t: &Team,
        bomb: [f32; 3],
        defuser: Option<[f32; 3]>,
        snap: &mut dyn FnMut([f32; 3]) -> Option<[f32; 3]>,
    ) -> Option<(PostStep, &'static str)> {
        if t.guard_ring.is_empty() {
            return None;
        }
        let ring = t.guard_ring.clone();
        let mut step = self.post_step(now, t, GUARD_KEY, &ring, 0, Some(t.own_pos), bomb, snap)?;
        if let (Some(d), Some(_)) = (defuser, step.watch) {
            step.watch = Some(d);
        }
        Some((step, "guard"))
    }

    /// BOT-136: a post round the planted `bomb` (the posts of the site it lies at that are within [`POST_MAX`] of
    /// it, the one nearest the bomb first); `None` when none is known.
    fn bomb_post(
        &mut self,
        now: u32,
        t: &Team,
        bomb: [f32; 3],
        snap: &mut dyn FnMut([f32; 3]) -> Option<[f32; 3]>,
    ) -> Option<(PostStep, &'static str)> {
        let (si, _) = t
            .sites
            .iter()
            .enumerate()
            .map(|(i, p)| (i, flat(*p, bomb)))
            .filter(|(_, d)| *d <= POST_AREA)
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        let posts: Vec<[f32; 3]> = t
            .posts
            .get(si)?
            .iter()
            .copied()
            .filter(|p| flat(*p, bomb) <= POST_MAX)
            .collect();
        if posts.is_empty() {
            return None;
        }
        let step = self.post_step(
            now,
            t,
            BOMB_KEY + si as u32,
            &posts,
            0,
            Some(bomb),
            bomb,
            snap,
        )?;
        let kind = if step.peek { "peek" } else { "post" };
        Some((step, kind))
    }

    /// BOT-136: the walk between the `posts` of area `key`: the first post by the round and the bot's `slot` (or
    /// the one nearest `prefer`), a teammate's post passed over; [`post_dwell_ms`] there, then the next one round
    /// the area (each bot its own way round); peeks and crouches while there.
    #[allow(clippy::too_many_arguments)]
    fn post_step(
        &mut self,
        now: u32,
        t: &Team,
        key: u32,
        posts: &[[f32; 3]],
        slot: usize,
        prefer: Option<[f32; 3]>,
        watch: [f32; 3],
        snap: &mut dyn FnMut([f32; 3]) -> Option<[f32; 3]>,
    ) -> Option<PostStep> {
        // a post the bot did not reach is left out for the round; with none left the caller's fallback holds
        let kept: Vec<[f32; 3]> = posts
            .iter()
            .copied()
            .filter(|q| !self.failed.iter().any(|f| flat(*f, *q) < 1.0))
            .collect();
        if kept.is_empty() {
            self.post = None;
            return None;
        }
        let posts = &kept[..];
        let me = t.own_pos;
        let (pid, round) = (t.own_pid, t.round);
        let taken = |p: [f32; 3]| {
            t.mates
                .iter()
                .any(|m| m.pid != pid && m.alive && m.pos.is_some_and(|q| flat(q, p) <= POST_TAKEN))
        };
        let len = posts.len();
        if self.post.is_none_or(|p| p.key != key) {
            let order: Vec<usize> = match prefer {
                Some(q) => {
                    let mut idx: Vec<usize> = (0..len).collect();
                    idx.sort_by(|a, b| flat(posts[*a], q).total_cmp(&flat(posts[*b], q)));
                    idx
                }
                None => {
                    let base = (round as usize + slot * (len / 2).max(1)) % len;
                    (0..len).map(|o| (base + o) % len).collect()
                }
            };
            let first = order
                .iter()
                .copied()
                .find(|&i| !taken(posts[i]))
                .unwrap_or(order[0]);
            self.post = Some(Post {
                key,
                at: posts[first],
                k: 0,
                since: now,
                arrived: None,
                peek: None,
                next_peek: now,
            });
        }
        let Some(mut p) = self.post else {
            unreachable!("set above")
        };
        let gap = |h: u64| PEEK_GAP_MIN_MS + (h % u64::from(PEEK_GAP_SPAN_MS + 1)) as u32;
        if p.arrived.is_none() && flat(me, p.at) <= POST_ARRIVE && (me[1] - p.at[1]).abs() <= 64.0 {
            p.arrived = Some(now);
            p.next_peek = now.wrapping_add(gap(seed(pid, round, key ^ 0x55, p.k)));
        }
        let ring = key == RING_KEY || key == FLEE_KEY;
        let late = ring && (key == FLEE_KEY || t.fuse_left.is_some_and(|f| f <= RING_STILL_MS));
        // BOT-142: a guard keeps its place while a defuse is contested
        let guard = key == GUARD_KEY;
        let swap = late
            && p.arrived.is_none()
            && now.wrapping_sub(p.since) >= LEAVE_SWITCH_MS
            && flat(me, watch) < BLAST_CLEAR;
        let move_on = swap
            || (!late
                && !(guard && t.defuser.is_some())
                && match p.arrived {
                    Some(a) => now.wrapping_sub(a) >= post_dwell_ms(pid, round, key, p.k),
                    None => now.wrapping_sub(p.since) >= POST_REACH_MS,
                });
        if move_on {
            if p.arrived.is_none() && key != FLEE_KEY {
                self.failed.push(p.at);
            }
            let clockwise = seed(pid, round, key, 0) & 1 == 0;
            let cur = (0..len)
                .min_by(|a, b| flat(posts[*a], p.at).total_cmp(&flat(posts[*b], p.at)))
                .unwrap_or(0);
            let next = (1..len)
                .map(|o| {
                    if clockwise {
                        (cur + o) % len
                    } else {
                        (cur + len - o) % len
                    }
                })
                .map(|i| posts[i])
                .find(|q| flat(*q, p.at) > POST_ARRIVE && !taken(*q));
            p.k += 1;
            p.since = now;
            p.arrived = None;
            p.peek = None;
            if let Some(q) = next {
                p.at = q;
            }
        }
        let mut goal = p.at;
        let mut peeking = false;
        // on the blast ring a peek stays out of the blast too; a fleeing defender and a guard (BOT-142) do not peek
        if p.arrived.is_some() && key != FLEE_KEY && !guard {
            if let Some((q, t0)) = p.peek {
                if flat(me, q) <= POST_STAND || now.wrapping_sub(t0) >= PEEK_MS {
                    p.peek = None;
                    p.next_peek = now.wrapping_add(gap(seed(pid, round, key ^ t0, p.k)));
                } else {
                    goal = q;
                    peeking = true;
                }
            } else if (now.wrapping_sub(p.next_peek) as i32) >= 0 {
                let h = seed(pid, round, key ^ p.next_peek, p.k);
                let near = step_near(h, p.at, PEEK_STEP, PEEK_NEAR, PEEK_FAR, me, snap)
                    .filter(|n| !ring || flat(*n, watch) >= BLAST_CLEAR);
                match near {
                    Some(n) => {
                        p.peek = Some((n, now));
                        goal = n;
                        peeking = true;
                    }
                    None => p.next_peek = now.wrapping_add(gap(h >> 7)),
                }
            }
        }
        let crouch = !peeking
            && flat(me, p.at) <= POST_ARRIVE
            && p.arrived.is_some_and(|a| {
                let h = seed(pid, round, key ^ 0xc0, p.k);
                let held = now.wrapping_sub(a);
                let len = CROUCH_MIN_MS + ((h >> 8) % u64::from(CROUCH_SPAN_MS + 1)) as u32;
                !h.is_multiple_of(3) && held >= CROUCH_AFTER_MS && held < CROUCH_AFTER_MS + len
            });
        let watch = (p.arrived.is_some() && !peeking).then_some(watch);
        self.post = Some(p);
        Some(PostStep {
            goal: Objective {
                goal,
                stand_within: POST_STAND,
            },
            crouch,
            watch,
            peek: peeking,
        })
    }

    /// BOT-136: a waiting bot's shift: every 5..10 s a step to a node 48..192 units from where the wait began.
    fn shift_step(
        &mut self,
        now: u32,
        t: &Team,
        snap: &mut dyn FnMut([f32; 3]) -> Option<[f32; 3]>,
    ) -> Option<[f32; 3]> {
        let me = t.own_pos;
        let (pid, round) = (t.own_pid, t.round);
        let gap = |h: u64| SHIFT_GAP_MIN_MS + (h % u64::from(SHIFT_GAP_SPAN_MS + 1)) as u32;
        let s = self.shift.get_or_insert(Shift {
            anchor: me,
            k: 0,
            next: now.wrapping_add(gap(seed(pid, round, 300, 0))),
            step: None,
        });
        if let Some((q, t0)) = s.step {
            if flat(me, q) <= SHIFT_NEAR * 0.75 || now.wrapping_sub(t0) >= SHIFT_MS {
                s.step = None;
                s.k += 1;
                s.next = now.wrapping_add(gap(seed(pid, round, 300, s.k)));
            } else {
                return Some(q);
            }
        }
        if (now.wrapping_sub(s.next) as i32) < 0 {
            return None;
        }
        let h = seed(pid, round, 301, s.k);
        let anchor = s.anchor;
        let near = step_near(h, anchor, SHIFT_STEP, SHIFT_NEAR, SHIFT_FAR, me, snap);
        let s = self.shift.as_mut()?;
        match near {
            Some(n) => {
                s.step = Some((n, now));
                Some(n)
            }
            None => {
                s.k += 1;
                s.next = now.wrapping_add(gap(h >> 7));
                None
            }
        }
    }

    /// The acting lead: the first living member in job order among the lead and the escorts.
    fn lead(&self, t: &Team) -> Option<u8> {
        let mut cands: Vec<(Job, u8)> = t
            .mates
            .iter()
            .enumerate()
            .filter(|(_, m)| m.alive)
            .filter_map(|(i, m)| match t.job(i)? {
                j @ (Job::Lead | Job::Escort(_)) => Some((j, m.pid)),
                _ => None,
            })
            .collect();
        cands.sort_by_key(|(j, _)| match j {
            Job::Lead => 0,
            Job::Escort(k) => 1 + u32::from(*k),
            _ => u32::MAX,
        });
        cands.first().map(|c| c.1)
    }

    /// The flank waypoint while it is not passed (`None`: walk to the site).
    fn flank_step(
        &mut self,
        now: u32,
        me: [f32; 3],
        site: [f32; 3],
        k: u8,
        snap: &mut dyn FnMut([f32; 3]) -> Option<[f32; 3]>,
    ) -> Option<[f32; 3]> {
        if self.via_done {
            return None;
        }
        if self.via.is_none() {
            let Some(v) = self.origin.and_then(|o| flank_via(o, site, k)) else {
                self.via_done = true;
                return None;
            };
            let v = snap(v).unwrap_or(v);
            self.via = Some((v, flat(me, v), now));
        }
        let (v, best, at) = self.via?;
        let d = flat(me, v);
        if d <= VIA_REACH || flat(me, site) < flat(v, site) * 0.9 {
            self.via_done = true;
            return None;
        }
        if d < best - VIA_GAIN {
            self.via = Some((v, d, now));
        } else if now.wrapping_sub(at) >= VIA_STALL_MS {
            self.via_done = true;
            return None;
        }
        Some(v)
    }

    /// The rotator's site: the one with fewest living holders, the sites in turn every [`ROTATE_MS`] while even.
    fn rotate_site(&self, t: &Team, elapsed: u32) -> [f32; 3] {
        t.sites[self.rotate_index(t, elapsed)]
    }

    /// The index of the rotator's site (see [`Squad::rotate_site`]).
    fn rotate_index(&self, t: &Team, elapsed: u32) -> usize {
        let n = t.sites.len();
        let alive_at = |s: usize| {
            t.mates
                .iter()
                .enumerate()
                .filter(|(i, m)| m.alive && t.job(*i) == Some(Job::Hold(s as u8)))
                .count()
        };
        let turn = (elapsed / ROTATE_MS) as usize;
        (0..n)
            .map(|k| (k + turn) % n)
            .min_by_key(|&s| alive_at(s))
            .unwrap_or(0)
    }

    /// Whether the bot waits for a teammate ahead of it on the way to `shared` (closer to it, nearer than
    /// [`SPACING`] to the bot): `Some(settled)`, `settled` when the one ahead stands at the objective. A wait behind
    /// one that stands at the objective lasts; behind one still on its way at most [`SPACING_WAIT_MAX_MS`].
    /// BOT-129: distances count the height difference ([`spaced`]); within [`SPACING_TIE`] of each other the lower
    /// player id is ahead, so of two teammates at most one waits for the other.
    fn spacing(&mut self, now: u32, t: &Team, index: usize, shared: [f32; 3]) -> Option<bool> {
        let me = t.own_pos;
        let own_d = spaced(me, shared);
        if own_d <= NO_WAIT_NEAR {
            self.spacing_wait = false;
            self.wait_since = None;
            return None;
        }
        let reach = if self.spacing_wait {
            SPACING_RESUME
        } else {
            SPACING
        };
        let settled = if t.planted && t.attacking {
            BOMB_GUARD
        } else {
            SITE_HOLD
        } + RESUME_MARGIN;
        let mut ahead: Option<bool> = None;
        for (i, m) in t.mates.iter().enumerate() {
            if i == index || !m.alive {
                continue;
            }
            let (Some(p), Some(mj)) = (m.pos, t.job(i)) else {
                continue;
            };
            let theirs = if mj == Job::Rotate && !t.planted {
                None
            } else {
                t.shared_point(mj)
            };
            if !theirs.is_some_and(|s| same(s, shared)) {
                continue;
            }
            let d = spaced(p, shared);
            let before =
                d + SPACING_TIE < own_d || ((d - own_d).abs() <= SPACING_TIE && m.pid < t.own_pid);
            if before && spaced(me, p) < reach {
                let at_objective = d <= settled;
                ahead = Some(ahead.unwrap_or(false) || at_objective);
            }
        }
        let Some(settled_ahead) = ahead else {
            self.spacing_wait = false;
            self.wait_since = None;
            return None;
        };
        if !settled_ahead {
            if self
                .released_until
                .is_some_and(|u| (u.wrapping_sub(now) as i32) > 0)
            {
                // released: the bot passes the one ahead
                self.spacing_wait = false;
                return None;
            }
            let since = *self.wait_since.get_or_insert(now);
            if now.wrapping_sub(since) >= SPACING_WAIT_MAX_MS {
                self.released_until = Some(now.wrapping_add(SPACING_RELEASE_MS));
                self.wait_since = None;
                self.spacing_wait = false;
                return None;
            }
        } else {
            self.wait_since = None;
        }
        self.spacing_wait = true;
        Some(settled_ahead)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: [f32; 3]) -> Option<[f32; 3]> {
        None
    }

    /// Orders every 100 ms from `from` to `to` (a session asks every tick); the last one.
    fn step(sq: &mut Squad, t: &Team, from: u32, to: u32) -> Order {
        let mut o = None;
        let mut ms = from;
        while ms <= to {
            o = sq.order(ms, t, &mut none);
            ms += 100;
        }
        o.unwrap()
    }

    fn team(n: usize, own: usize, attacking: bool) -> Team {
        Team {
            own_pid: own as u8 + 1,
            own_pos: [0.0; 3],
            mates: (0..n)
                .map(|i| Mate {
                    pid: i as u8 + 1,
                    pos: Some([0.0, 0.0, -100.0 * i as f32]),
                    alive: true,
                })
                .collect(),
            round: 0,
            attacking,
            sites: vec![[3000.0, 0.0, 0.0], [-3000.0, 0.0, 0.0]],
            planted: false,
            bomb_pos: None,
            posts: Vec::new(),
            enemy_seen: None,
            blast_ring: Vec::new(),
            fuse_left: None,
            hold_in_time: false,
            guard_ring: Vec::new(),
            defuser: None,
        }
    }

    /// BOT-122: seven attackers are lead, two escorts, three flankers and a feint; seven defenders three and three
    /// over two sites and a rotator; every member of a team gets a different job, the same on every bot, turned by
    /// the round.
    #[test]
    fn jobs_split_the_team_and_turn_with_the_round() {
        use Job::*;
        assert_eq!(
            attack_jobs(7, 2),
            vec![
                Lead,
                Escort(0),
                Escort(1),
                Flank(0),
                Flank(1),
                Flank(2),
                Feint
            ]
        );
        assert_eq!(attack_jobs(4, 2), vec![Lead, Escort(0), Flank(0), Flank(1)]);
        assert_eq!(
            attack_jobs(6, 1),
            vec![Lead, Escort(0), Escort(1), Flank(0), Flank(1), Flank(2)]
        );
        assert_eq!(attack_jobs(1, 2), vec![Lead]);
        assert_eq!(
            defend_jobs(7, 2),
            vec![Hold(0), Hold(1), Hold(0), Hold(1), Hold(0), Hold(1), Rotate]
        );
        assert_eq!(defend_jobs(4, 2), vec![Hold(0), Hold(1), Hold(0), Hold(1)]);
        assert_eq!(defend_jobs(3, 1), vec![Hold(0), Hold(0), Hold(0)]);
        for round in 0..3 {
            let jobs: Vec<_> = (0..7)
                .map(|i| job_of(i, 7, round, true, 2).unwrap())
                .collect();
            let mut sorted = jobs.clone();
            sorted.dedup();
            assert_eq!(sorted.len(), 7, "seven different jobs");
            assert_eq!(
                jobs[(7 - round as usize) % 7],
                Lead,
                "the lead turns with the round"
            );
        }
        assert_eq!(job_of(7, 7, 0, true, 2), None);
    }

    /// BOT-122: the flank waypoints lie to both sides of the straight line, farther for the second pair; none for a
    /// near site.
    #[test]
    fn flank_waypoints_are_off_the_line_on_both_sides() {
        let (o, s) = ([0.0, 0.0, 0.0], [2000.0, 0.0, 0.0]);
        let a = flank_via(o, s, 0).unwrap();
        let b = flank_via(o, s, 1).unwrap();
        let c = flank_via(o, s, 2).unwrap();
        assert_eq!((a[0], b[0]), (1000.0, 1000.0));
        assert!(a[2] > 700.0 && b[2] < -700.0, "{a:?} {b:?}");
        assert!(c[2] > a[2] + 300.0);
        assert_eq!(flank_via(o, [500.0, 0.0, 0.0], 0), None);
    }

    /// BOT-122: the start delays leave the spawn one job after another; the lead goes at once.
    #[test]
    fn jobs_leave_the_spawn_in_turn() {
        let mut sq = Squad::default();
        // the escort of round 0 is member 1
        let t = team(7, 1, true);
        let o = sq.order(0, &t, &mut none).unwrap();
        assert_eq!((o.job, o.wait, o.why), (Job::Escort(0), true, "start"));
        assert!(step(&mut sq, &t, 100, 1100).wait);
        let mut lead = Squad::default();
        let o = lead.order(0, &team(7, 0, true), &mut none).unwrap();
        assert_eq!((o.job, o.wait), (Job::Lead, false));
        assert_eq!(o.goal.map(|g| g.goal), Some([3000.0, 0.0, 0.0]));
        assert_eq!(o.plant_at, Some([3000.0, 0.0, 0.0]));
    }

    /// BOT-122: an escort walks to the lead while it is far, stands once near; the next escort takes over a dead
    /// lead's job.
    #[test]
    fn escorts_trail_the_lead_in_a_band() {
        let mut t = team(7, 1, true);
        t.mates[0].pos = Some([1000.0, 0.0, 0.0]);
        let mut sq = Squad::default();
        let o = step(&mut sq, &t, 0, 1500);
        assert_eq!(
            (o.wait, o.goal.map(|g| g.goal)),
            (false, Some([1000.0, 0.0, 0.0]))
        );
        t.own_pos = [700.0, 0.0, 0.0];
        let o = sq.order(1600, &t, &mut none).unwrap();
        assert_eq!((o.wait, o.why), (true, "escort"));
        t.own_pos = [600.0, 0.0, 0.0];
        assert!(
            sq.order(1700, &t, &mut none).unwrap().wait,
            "inside the band it stays"
        );
        t.mates[0].alive = false;
        let o = sq.order(1800, &t, &mut none).unwrap();
        assert_eq!(
            o.goal.map(|g| g.goal),
            Some([3000.0, 0.0, 0.0]),
            "it leads now"
        );
    }

    /// BOT-122: a flanker walks by its waypoint (snapped onto the grid) and to the site once it passed it or made no
    /// way to it; the feint goes for the other site and plants there.
    #[test]
    fn flankers_walk_by_their_waypoint_and_the_feint_takes_the_other_site() {
        let mut t = team(7, 3, true);
        let mut sq = Squad::default();
        let mut snap = |p: [f32; 3]| Some([p[0], p[1], p[2] + 10.0]);
        let o = sq.order(0, &t, &mut snap).unwrap();
        assert_eq!(o.job, Job::Flank(0));
        let o = sq.order(500, &t, &mut snap).unwrap();
        let via = o.goal.unwrap().goal;
        assert!(via[2] > 1000.0 && via[0] == 1500.0, "{via:?}");
        assert_eq!(sq.via(), Some(via));
        t.own_pos = [via[0] - 100.0, 0.0, via[2]];
        let o = sq.order(600, &t, &mut snap).unwrap();
        assert_eq!(o.goal.unwrap().goal, [3000.0, 0.0, 0.0], "passed");
        // a waypoint it gets no closer to is given up
        let mut t2 = team(7, 4, true);
        let mut sq2 = Squad::default();
        assert!(step(&mut sq2, &t2, 0, 1200).goal.unwrap().goal[2] < -1000.0);
        t2.own_pos = [1.0, 0.0, 0.0];
        // its start delay ends at 1100 ms, when it takes its waypoint
        assert!(
            step(&mut sq2, &t2, 1300, 1100 + VIA_STALL_MS - 100)
                .goal
                .unwrap()
                .goal[2]
                < -1000.0
        );
        assert_eq!(
            step(&mut sq2, &t2, 1100 + VIA_STALL_MS, 1100 + VIA_STALL_MS)
                .goal
                .unwrap()
                .goal,
            [3000.0, 0.0, 0.0]
        );
        let mut f = Squad::default();
        let o = f.order(0, &team(7, 6, true), &mut none).unwrap();
        assert_eq!(o.job, Job::Feint);
        assert_eq!(o.plant_at, Some([-3000.0, 0.0, 0.0]));
    }

    /// BOT-122: a bot behind a teammate on the way to the same site waits while it is within the spacing, at most a
    /// while behind one still walking, for good behind one at the site; a bot at its point never waits.
    #[test]
    fn spacing_to_the_teammate_ahead() {
        // two defenders holding site 0 (members 0 and 2), the bot is member 2
        let mut t = team(7, 2, false);
        t.mates[0].pos = Some([1000.0, 0.0, 0.0]);
        t.own_pos = [800.0, 0.0, 0.0];
        let mut sq = Squad::default();
        let o = step(&mut sq, &t, 0, 600);
        assert_eq!((o.job, o.wait, o.why), (Job::Hold(0), true, "spacing"));
        // its start delay (the second holder of the site) ends at 500 ms, when the wait starts
        assert!(step(&mut sq, &t, 700, 500 + SPACING_WAIT_MAX_MS - 100).wait);
        assert!(
            !step(
                &mut sq,
                &t,
                500 + SPACING_WAIT_MAX_MS,
                500 + SPACING_WAIT_MAX_MS
            )
            .wait,
            "released behind one still walking"
        );
        // the one ahead stands at the site: the wait lasts
        t.mates[0].pos = Some([2900.0, 0.0, 0.0]);
        t.own_pos = [2700.0, 0.0, 0.0];
        let mut sq = Squad::default();
        assert!(step(&mut sq, &t, 0, 600).wait);
        assert!(
            step(&mut sq, &t, 700, 20_000).wait,
            "behind one at the site the wait lasts"
        );
        // hysteresis: it walks again only past the resume distance
        t.own_pos = [2900.0 - 300.0, 0.0, 0.0];
        assert!(sq.order(20_100, &t, &mut none).unwrap().wait);
        t.own_pos = [2900.0 - 330.0, 0.0, 0.0];
        assert!(!sq.order(20_200, &t, &mut none).unwrap().wait);
        // a teammate of the other site does not count
        let mut t = team(7, 2, false);
        t.mates[1].pos = Some([1000.0, 0.0, 0.0]);
        t.own_pos = [800.0, 0.0, 0.0];
        let mut sq = Squad::default();
        sq.order(0, &t, &mut none);
        assert!(!sq.order(600, &t, &mut none).unwrap().wait);
        // planted: every defender goes for the bomb, the nearest one never waits
        let mut t = team(7, 2, false);
        t.planted = true;
        t.bomb_pos = Some([3000.0, 0.0, 0.0]);
        t.own_pos = [2950.0, 0.0, 0.0];
        t.mates[0].pos = Some([2900.0, 0.0, 10.0]);
        let mut sq = Squad::default();
        let o = sq.order(0, &t, &mut none).unwrap();
        assert_eq!(o.goal.map(|g| g.goal), Some([3000.0, 0.0, 0.0]));
        assert!(!o.wait, "at the bomb it defuses");
    }

    /// BOT-129: two teammates level with each other on the way to the same point (each sees the other a little
    /// ahead, as streamed places lag) never both wait: the lower player id goes first; a teammate a floor below is
    /// not near; the lead never waits; a wait behind one still walking lets the stall clock run.
    #[test]
    fn spacing_ties_go_by_player_id_and_the_lead_never_waits() {
        // defenders holding site 0 (members 0 and 2): each sees the other 30 units nearer the site
        let level = |own: usize, other: usize| {
            let mut t = team(7, own, false);
            t.own_pos = [2000.0, 0.0, 0.0];
            t.mates[other].pos = Some([2030.0, 0.0, 0.0]);
            t
        };
        let waits = |t: &Team| step(&mut Squad::default(), t, 0, 1500).wait;
        assert!(!waits(&level(0, 2)), "the lower id goes on");
        assert!(waits(&level(2, 0)), "the higher id waits");
        let o = step(&mut Squad::default(), &level(2, 0), 0, 1500);
        assert!(
            o.stall_counts,
            "behind one still walking the stall clock runs"
        );
        // clearly ahead still counts, whatever the ids
        let mut t = level(0, 2);
        t.mates[2].pos = Some([2200.0, 0.0, 0.0]);
        assert!(waits(&t));
        // a teammate right above on another floor is not near
        t.mates[2].pos = Some([2100.0, 300.0, 0.0]);
        assert!(!waits(&t));
        // the lead (member 0 in round 0) walks on with an escort ahead of it
        let mut t = team(7, 0, true);
        t.own_pos = [2000.0, 0.0, 0.0];
        t.mates[1].pos = Some([2150.0, 0.0, 0.0]);
        let o = step(&mut Squad::default(), &t, 0, 1500);
        assert_eq!((o.job, o.wait), (Job::Lead, false));
        // behind one standing at the site the wait lasts and the clock stops
        let mut t = team(7, 2, false);
        t.own_pos = [2700.0, 0.0, 0.0];
        t.mates[0].pos = Some([2900.0, 0.0, 0.0]);
        let o = step(&mut Squad::default(), &t, 0, 1500);
        assert!(o.wait && !o.stall_counts);
    }

    /// BOT-124: after the plant every defender goes for the bomb whatever its job: the two living ones nearest to
    /// it defuse (all the way to the defuse point, never waiting for the one ahead), the others cover it (standing
    /// within the bomb guard radius); a fallen defender's body takes no defuser place.
    #[test]
    fn a_planted_bomb_sends_the_nearest_defenders_to_defuse_under_every_job() {
        let bomb = [3000.0, 0.0, 0.0];
        let planted = |own: usize, round: u32| {
            let mut t = team(7, own, false);
            t.round = round;
            t.planted = true;
            t.bomb_pos = Some(bomb);
            // a line toward the bomb, 200 units apart: member 0 stands 100 from it
            for (i, m) in t.mates.iter_mut().enumerate() {
                m.pos = Some([2900.0 - 200.0 * i as f32, 0.0, 0.0]);
            }
            t.own_pos = t.mates[own].pos.unwrap();
            t
        };
        let mut defuser_jobs = Vec::new();
        for round in 0..7 {
            for own in 0..7 {
                let t = planted(own, round);
                let mut sq = Squad::default();
                let o = step(&mut sq, &t, 0, 2000);
                let g = o.goal.expect("a goal after the plant");
                assert_eq!(g.goal, bomb, "round {round} member {own} {:?}", o.job);
                assert_eq!(o.defuse, own < DEFUSERS, "round {round} member {own}");
                if o.defuse {
                    // member 1 is 200 behind member 0, who stands at the bomb: the spacing held it there for good
                    assert!(!o.wait, "a defuser never waits ({:?})", o.job);
                    assert_eq!(g.stand_within, 0.0);
                    defuser_jobs.push(o.job);
                } else {
                    assert_eq!(g.stand_within, BOMB_GUARD);
                }
            }
        }
        for job in [Job::Hold(0), Job::Hold(1), Job::Rotate] {
            assert!(defuser_jobs.contains(&job), "{job:?} defuses too");
        }
        // member 0 fell at the bomb: members 1 and 2 defuse, member 3 covers
        for (own, defuse) in [(1, true), (2, true), (3, false)] {
            let mut t = planted(own, 0);
            t.mates[0].alive = false;
            let o = step(&mut Squad::default(), &t, 0, 2000);
            assert_eq!(o.defuse, defuse, "member {own}");
            assert!(!defuse || !o.wait, "member {own} waits behind a body");
        }
    }

    /// BOT-122: the rotator goes to the site with fewer living holders.
    #[test]
    fn the_rotator_backs_the_thinner_site() {
        let mut t = team(7, 6, false);
        let mut sq = Squad::default();
        assert_eq!(sq.order(0, &t, &mut none).unwrap().job, Job::Rotate);
        step(&mut sq, &t, 100, 2900);
        // holders of site 0 are members 0, 2, 4; two of them die
        t.mates[0].alive = false;
        t.mates[2].alive = false;
        let o = sq.order(3000, &t, &mut none).unwrap();
        assert_eq!(o.goal.unwrap().goal, [3000.0, 0.0, 0.0]);
        t.mates[0].alive = true;
        t.mates[2].alive = true;
        t.mates[1].alive = false;
        let o = sq.order(3100, &t, &mut none).unwrap();
        assert_eq!(o.goal.unwrap().goal, [-3000.0, 0.0, 0.0]);
    }

    fn ring(c: [f32; 3], r: f32, n: usize) -> Vec<[f32; 3]> {
        (0..n)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / n as f32;
                [c[0] + r * a.sin(), c[1], c[2] + r * a.cos()]
            })
            .collect()
    }

    fn same_place(snap: [f32; 3]) -> Option<[f32; 3]> {
        Some(snap)
    }

    /// Orders every 100 ms from `from` to `to`, the bot standing at each goal at once (a walk takes no time); the
    /// orders.
    fn walk(sq: &mut Squad, t: &mut Team, from: u32, to: u32) -> Vec<(u32, Order)> {
        let mut out = Vec::new();
        let mut ms = from;
        while ms <= to {
            let o = sq.order(ms, t, &mut same_place).unwrap();
            if let Some(g) = o.shift.or(o.goal.filter(|_| !o.wait).map(|g| g.goal)) {
                t.own_pos = g;
            }
            out.push((ms, o));
            ms += 100;
        }
        out
    }

    /// BOT-136: the posts lie 128..480 units from the site, at least 192 apart, at most four, those that see the
    /// site first, one that does not only while fewer than two do; in angle order round the site.
    #[test]
    fn posts_ring_the_site_apart_and_seeing_first() {
        let site = [1000.0, 0.0, 1000.0];
        // a lattice of nodes every 64 units round the site, one high above it
        let mut nodes: Vec<[f32; 3]> = (-10..=10)
            .flat_map(|i| {
                (-10..=10).map(move |j| [1000.0 + 64.0 * i as f32, 0.0, 1000.0 + 64.0 * j as f32])
            })
            .collect();
        nodes.push([1300.0, 400.0, 1000.0]);
        let posts = post_set(site, &nodes, |n, _| n[0] > site[0]);
        assert!(posts.len() >= 3, "{posts:?}");
        for (i, p) in posts.iter().enumerate() {
            let d = flat(*p, site);
            assert!((POST_MIN..=POST_MAX).contains(&d), "{p:?} {d}");
            assert!(p[0] > site[0], "the posts that see the site: {p:?}");
            assert!(p[1].abs() < 1.0);
            for q in &posts[i + 1..] {
                assert!(flat(*p, *q) >= POST_SPREAD);
            }
        }
        let ang: Vec<f32> = posts
            .iter()
            .map(|p| (p[0] - site[0]).atan2(p[2] - site[2]))
            .collect();
        assert!(ang.windows(2).all(|w| w[0] <= w[1]), "in angle order");
        // only one node sees the site: it and one that does not
        let one = post_set(site, &nodes, |n, _| {
            n[0] > 1300.0 && n[2] > 1000.0 && n[2] < 1100.0
        });
        assert_eq!(one.len(), 2, "{one:?}");
        // no node sees it: two posts all the same; no node near: none
        assert_eq!(post_set(site, &nodes, |_, _| false).len(), 2);
        assert!(post_set([9000.0, 0.0, 0.0], &nodes, |_, _| true).is_empty());
    }

    /// BOT-136: the dwell at a post is 10..25 s and differs between bots, rounds and posts.
    #[test]
    fn the_dwell_at_a_post_is_10_to_25_s_per_bot() {
        let mut seen = std::collections::BTreeSet::new();
        for pid in 1..9 {
            for round in 0..6 {
                for k in 0..4 {
                    let d = post_dwell_ms(pid, round, 0, k);
                    assert!(
                        (POST_DWELL_MIN_MS..=POST_DWELL_MIN_MS + POST_DWELL_SPAN_MS).contains(&d)
                    );
                    seen.insert(d / 1000);
                }
            }
        }
        assert!(seen.len() >= 12, "spread over the range: {seen:?}");
    }

    /// BOT-136: a holder walks between the posts of its site, every 10..25 s to the next one, never to a post a
    /// teammate holds; it peeks now and then, crouches at some posts and watches the site; its teammate of the same
    /// site starts at another post.
    #[test]
    fn a_holder_moves_between_its_posts_and_peeks() {
        let site = [3000.0, 0.0, 0.0];
        let posts = ring(site, 320.0, 4);
        let mut t = team(4, 0, false);
        t.posts = vec![posts.clone(), ring([-3000.0, 0.0, 0.0], 320.0, 4)];
        t.own_pos = [2000.0, 0.0, 0.0];
        // member 2 holds the same site; far away for now
        t.mates[2].pos = Some([0.0, 0.0, 0.0]);
        let mut sq = Squad::default();
        let orders = walk(&mut sq, &mut t, 0, 120_000);
        let at_post = |g: [f32; 3]| posts.iter().position(|p| flat(*p, g) < 1.0);
        let mut changes = Vec::new();
        let mut last = None;
        let (mut peeks, mut crouch, mut watch) = (0, 0, 0);
        for (ms, o) in &orders {
            assert_eq!(o.job, Job::Hold(0));
            let g = o.goal.unwrap();
            assert!(
                flat(g.goal, site) <= POST_MAX + PEEK_FAR,
                "near the site {g:?}"
            );
            if let Some(i) = at_post(g.goal) {
                if last != Some(i) {
                    changes.push(*ms);
                    last = Some(i);
                }
            }
            peeks += usize::from(o.post == "peek");
            crouch += usize::from(o.crouch);
            watch += usize::from(o.watch == Some(site));
            assert!(
                !o.wait || o.why == "start",
                "no spacing wait between the posts"
            );
        }
        assert!(changes.len() >= 5, "{changes:?}");
        for w in changes.windows(2).skip(1) {
            let gap = w[1] - w[0];
            assert!(
                (POST_DWELL_MIN_MS..=POST_DWELL_MIN_MS + POST_DWELL_SPAN_MS + 200).contains(&gap),
                "{changes:?}"
            );
        }
        assert!(
            peeks > 0 && crouch > 0 && watch > 0,
            "{peeks} {crouch} {watch}"
        );
        // the teammate of the same site starts elsewhere; a post a teammate stands at is passed over
        let first = |own: usize, mate_at: Option<[f32; 3]>| {
            let mut t = team(4, own, false);
            t.posts = vec![posts.clone(), Vec::new()];
            t.own_pos = [2000.0, 0.0, 0.0];
            t.mates[2 - own].pos = mate_at.or(Some([0.0, 0.0, 0.0]));
            step(&mut Squad::default(), &t, 0, 2000).goal.unwrap().goal
        };
        assert!(flat(first(0, None), first(2, None)) > POST_SPREAD);
        let mine = first(0, None);
        assert!(
            flat(first(0, Some(mine)), mine) > POST_SPREAD,
            "the teammate's post"
        );
    }

    /// BOT-136: a post the bot does not reach in 15 s (here: one on a floor below, the bot never gets there) is
    /// left out for the round; with none left the BOT-103 hold of the site is back.
    #[test]
    fn unreached_posts_are_left_out_then_the_site_hold_is_back() {
        let site = [3000.0, 0.0, 0.0];
        let mut t = team(4, 0, false);
        t.posts = vec![ring(site, 320.0, 4), Vec::new()];
        t.mates[2].pos = Some([0.0, 0.0, 0.0]);
        // right above the site on a balcony: a flat distance is no arrival
        t.own_pos = [3000.0, 250.0, 320.0];
        let mut sq = Squad::default();
        let early = step(&mut sq, &t, 0, 1000).goal.unwrap();
        assert_eq!(early.stand_within, POST_STAND);
        let late = step(&mut sq, &t, 1100, 4 * POST_REACH_MS + 1000)
            .goal
            .unwrap();
        assert_eq!((late.goal, late.stand_within), (site, SITE_HOLD));
    }

    /// BOT-136: a quiet round (no enemy seen for 45 s) sends the last holder of a site that keeps another holder to
    /// the other site's post nearest its own site, for a look, and back; never the only holder left.
    #[test]
    fn a_quiet_round_sends_one_holder_to_look_at_the_other_site() {
        let sites = [[3000.0, 0.0, 0.0], [-3000.0, 0.0, 0.0]];
        let mk = |own: usize| {
            let mut t = team(4, own, false);
            t.posts = vec![ring(sites[0], 320.0, 4), ring(sites[1], 320.0, 4)];
            t.own_pos = [2700.0, 0.0, 0.0];
            t.mates[2 - own].pos = Some([3000.0, 0.0, 900.0]);
            t
        };
        // members 0 and 2 hold site 0: member 2 is the last of them
        let mut t = mk(2);
        let mut sq = Squad::default();
        let orders = walk(&mut sq, &mut t, 0, 90_000);
        let away: Vec<u32> = orders
            .iter()
            .filter(|(_, o)| o.post == "check")
            .map(|(ms, _)| *ms)
            .collect();
        assert!(!away.is_empty());
        assert!(away[0] >= QUIET_MS, "{}", away[0]);
        let g = orders
            .iter()
            .find(|(_, o)| o.post == "check")
            .unwrap()
            .1
            .goal
            .unwrap()
            .goal;
        assert!(
            flat(g, sites[1]) <= POST_MAX && g[0] > sites[1][0],
            "the near side of the other site {g:?}"
        );
        let back = orders
            .iter()
            .find(|(ms, o)| *ms > away[0] && o.post != "check")
            .unwrap();
        assert!(back.0 - away[0] <= CHECK_MAX_MS);
        assert!(
            flat(back.1.goal.unwrap().goal, sites[0]) <= POST_MAX + PEEK_FAR,
            "back at its site"
        );
        // member 0 stays; an enemy seen keeps the round loud; alone at the site the holder stays
        let stays = |mut t: Team, seen: bool| {
            let mut sq = Squad::default();
            let mut ms = 0;
            let mut out = true;
            while ms <= 90_000 {
                if seen {
                    t.enemy_seen = Some(ms);
                }
                let o = sq.order(ms, &t, &mut same_place).unwrap();
                t.own_pos = o.goal.unwrap().goal;
                out &= o.post != "check";
                ms += 100;
            }
            out
        };
        assert!(stays(mk(0), false));
        assert!(stays(mk(2), true));
        let mut alone = mk(2);
        alone.mates[0].alive = false;
        assert!(stays(alone, false));
    }

    /// BOT-136: a waiting escort shifts every 5..10 s to a node 48..192 units from where its wait began and comes
    /// back to its wait; the wait goes on.
    #[test]
    fn a_waiting_attacker_shifts_now_and_then() {
        let mut t = team(7, 1, true);
        t.mates[0].pos = Some([600.0, 0.0, 0.0]);
        t.own_pos = [300.0, 0.0, 0.0];
        let mut sq = Squad::default();
        let orders = walk(&mut sq, &mut t, 0, 40_000);
        let shifts: Vec<&(u32, Order)> = orders.iter().filter(|(_, o)| o.shift.is_some()).collect();
        assert!(!shifts.is_empty());
        let mut starts = Vec::new();
        let mut prev = false;
        for (ms, o) in &orders {
            let s = o.shift.is_some();
            if s && !prev {
                starts.push(*ms);
            }
            prev = s;
            if let Some(p) = o.shift {
                assert!(o.wait && o.why == "escort");
                let d = flat(p, [300.0, 0.0, 0.0]);
                assert!((SHIFT_NEAR..=SHIFT_FAR).contains(&d), "{d}");
            }
        }
        assert!(starts.len() >= 3, "{starts:?}");
        for w in starts.windows(2) {
            assert!(w[1] - w[0] >= SHIFT_GAP_MIN_MS, "{starts:?}");
            assert!(
                w[1] - w[0] <= SHIFT_GAP_MIN_MS + SHIFT_GAP_SPAN_MS + SHIFT_MS + 200,
                "{starts:?}"
            );
        }
        // the start wait does not shift
        let mut sq = Squad::default();
        let o = sq.order(0, &team(7, 1, true), &mut same_place).unwrap();
        assert_eq!((o.why, o.shift), ("start", None));
    }

    /// BOT-136: after the plant the guarding attackers stand on the blast ring (out of the blast, a sight line to the
    /// bomb) and keep still there late in the fuse; with less fuse left than a defuse and its margin every defender
    /// runs to the ring, the defusers too, but for one whose own hold ends in time.
    #[test]
    fn the_guards_stand_out_of_the_blast_and_late_defenders_run() {
        let bomb = [3000.0, 0.0, 0.0];
        let nodes: Vec<[f32; 3]> = (0..36)
            .flat_map(|i| {
                let a = std::f32::consts::TAU * i as f32 / 36.0;
                [300.0f32, 700.0, 900.0, 1400.0]
                    .map(|r| [bomb[0] + r * a.sin(), 0.0, bomb[2] + r * a.cos()])
            })
            .collect();
        let ring = post_ring(bomb, &nodes, Ring::BLAST, |_, _| true);
        assert!(ring.len() >= 3 && ring.len() <= BLAST_POSTS_MAX, "{ring:?}");
        assert!(ring
            .iter()
            .all(|p| (BLAST_CLEAR..=BLAST_RING_MAX).contains(&flat(*p, bomb))));
        const { assert!(BLAST_CLEAR > BLAST_RADIUS + 100.0) };
        let mk = |own: usize, attacking: bool, fuse: u32| {
            let mut t = team(4, own, attacking);
            t.planted = true;
            t.bomb_pos = Some(bomb);
            t.blast_ring = ring.clone();
            t.fuse_left = Some(fuse);
            for (i, m) in t.mates.iter_mut().enumerate() {
                m.pos = Some([2900.0 - 100.0 * i as f32, 0.0, 0.0]);
            }
            t.own_pos = t.mates[own].pos.unwrap();
            t
        };
        // a guard walks to the ring, stands there; late in the fuse it does not move on
        let mut t = mk(1, true, 30_000);
        let mut sq = Squad::default();
        let o = sq.order(0, &t, &mut same_place).unwrap();
        let g = o.goal.unwrap().goal;
        assert!(flat(g, bomb) >= BLAST_CLEAR, "{g:?}");
        assert_eq!(o.post, "ring");
        t.own_pos = g;
        t.fuse_left = Some(RING_STILL_MS);
        for ms in (100..40_000).step_by(100) {
            let o = sq.order(ms, &t, &mut same_place).unwrap();
            let q = o.goal.unwrap().goal;
            // it keeps its place (a peek stays near it and out of the blast)
            assert!(
                flat(q, g) <= PEEK_FAR && flat(q, bomb) >= BLAST_CLEAR,
                "still at {ms}"
            );
        }
        // early in the fuse the nearest defenders defuse
        let o = step(&mut Squad::default(), &mk(0, false, 20_000), 0, 500);
        assert!(o.defuse);
        // late: everybody runs out of the blast
        for own in 0..4 {
            let o = step(&mut Squad::default(), &mk(own, false, DEFUSE_MS), 0, 500);
            assert!(!o.defuse && o.post == "flee", "member {own}");
            assert!(flat(o.goal.unwrap().goal, bomb) >= BLAST_CLEAR);
        }
        // a defuser whose hold ends in time stays on it
        let mut t = mk(0, false, DEFUSE_MS);
        t.hold_in_time = true;
        let o = step(&mut Squad::default(), &t, 0, 500);
        assert!(o.defuse);
        assert_eq!(o.goal.unwrap().goal, bomb);
    }

    /// BOT-142: the guard places lie 250..500 units from the bomb, see it (no blind place), two or three, 256 apart.
    #[test]
    fn the_guard_places_see_the_bomb_from_close() {
        let bomb = [3000.0, 0.0, 0.0];
        let nodes: Vec<[f32; 3]> = (0..36)
            .flat_map(|i| {
                let a = std::f32::consts::TAU * i as f32 / 36.0;
                [150.0f32, 300.0, 420.0, 700.0]
                    .map(|r| [bomb[0] + r * a.sin(), 0.0, bomb[2] + r * a.cos()])
            })
            .collect();
        let g = post_ring(bomb, &nodes, Ring::GUARD, |_, _| true);
        assert!((2..=GUARDS_MAX).contains(&g.len()), "{g:?}");
        assert!(g
            .iter()
            .all(|p| (GUARD_MIN..=GUARD_MAX).contains(&flat(*p, bomb))));
        for (i, p) in g.iter().enumerate() {
            assert!(g[i + 1..].iter().all(|q| flat(*p, *q) >= GUARD_SPREAD));
        }
        // only places with a sight line (here: the half with x > bomb's)
        let g = post_ring(bomb, &nodes, Ring::GUARD, |n, _| n[0] > bomb[0] + 1.0);
        assert!(!g.is_empty() && g.iter().all(|p| p[0] > bomb[0]), "{g:?}");
        assert!(post_ring(bomb, &nodes, Ring::GUARD, |_, _| false).is_empty());
        const { assert!(GUARD_MAX <= BLAST_RADIUS && LEAVE_MS > DEFUSE_MS) };
    }

    /// BOT-142: after the plant the attackers hold the guard places, leave for the blast ring with 10 s of the fuse
    /// left; a defender seen at the bomb is pushed by the nearest attacker while the others keep their places facing
    /// him, also late in the fuse, but not in its last 4 s.
    #[test]
    fn the_guards_hold_close_contest_a_defuse_and_leave_late() {
        let bomb = [3000.0, 0.0, 0.0];
        let guards = ring(bomb, 380.0, 3);
        let blast = ring(bomb, 800.0, 6);
        let mk = |own: usize, fuse: u32, defuser: Option<[f32; 3]>| {
            let mut t = team(4, own, true);
            t.planted = true;
            t.bomb_pos = Some(bomb);
            t.guard_ring = guards.clone();
            t.blast_ring = blast.clone();
            t.fuse_left = Some(fuse);
            t.defuser = defuser;
            for (i, m) in t.mates.iter_mut().enumerate() {
                m.pos = Some([3000.0 - 300.0 - 200.0 * i as f32, 0.0, 0.0]);
            }
            t.own_pos = t.mates[own].pos.unwrap();
            t
        };
        let is_guard = |q: [f32; 3]| guards.iter().any(|p| flat(*p, q) < 1.0);
        // early in the fuse every attacker walks to a guard place
        for own in 0..4 {
            let o = step(&mut Squad::default(), &mk(own, 30_000, None), 0, 300);
            assert_eq!(o.post, "guard", "member {own}");
            assert!(is_guard(o.goal.unwrap().goal));
        }
        // a guard at its place stays there, faces the bomb, never peeks
        let mut t = mk(1, 30_000, None);
        let mut sq = Squad::default();
        let g = sq.order(0, &t, &mut same_place).unwrap().goal.unwrap().goal;
        t.own_pos = g;
        for ms in (100..9000).step_by(100) {
            let o = sq.order(ms, &t, &mut same_place).unwrap();
            assert_eq!(o.goal.unwrap().goal, g, "at {ms}");
            assert_eq!(o.watch, Some(bomb));
        }
        // with 10 s left it leaves for the blast ring; a place it does not reach in 4 s is given up for another
        t.fuse_left = Some(LEAVE_MS);
        let o = sq.order(9100, &t, &mut same_place).unwrap();
        assert_eq!(o.post, "ring");
        let out = o.goal.unwrap().goal;
        assert!(flat(out, bomb) >= BLAST_CLEAR);
        let o = step(&mut sq, &t, 9200, 9100 + LEAVE_SWITCH_MS - 100);
        assert_eq!(o.goal.unwrap().goal, out);
        let w = 9100 + LEAVE_SWITCH_MS;
        let o = step(&mut sq, &t, w, w + 200);
        let other = o.goal.unwrap().goal;
        assert!(flat(other, out) > 1.0 && flat(other, bomb) >= BLAST_CLEAR);
        let mut sq = Squad::default();
        // a defuse seen late in the fuse: member 0 (nearest the bomb) pushes, the others hold their places
        let d = [3010.0, 0.0, 20.0];
        let o = step(&mut Squad::default(), &mk(0, 9000, Some(d)), 0, 300);
        assert_eq!(o.post, "push");
        assert_eq!(o.goal.unwrap().goal, d);
        assert_eq!(o.goal.unwrap().stand_within, PUSH_STAND);
        assert!(!o.wait);
        t.fuse_left = Some(9000);
        t.defuser = Some(d);
        let o = sq.order(9200, &t, &mut same_place).unwrap();
        assert_eq!(o.post, "guard");
        let q = o.goal.unwrap().goal;
        t.own_pos = q;
        let o = step(&mut sq, &t, 9300, 30_000);
        assert_eq!(
            o.goal.unwrap().goal,
            q,
            "keeps its place through the defuse"
        );
        assert_eq!(o.watch, Some(d));
        // the pusher falls: the next nearest pushes
        let mut t2 = mk(1, 9000, Some(d));
        t2.mates[0].alive = false;
        assert_eq!(step(&mut Squad::default(), &t2, 0, 300).post, "push");
        // in the last 4 s nobody contests: everybody leaves
        for own in 0..4 {
            let o = step(
                &mut Squad::default(),
                &mk(own, CONTEST_MIN_MS, Some(d)),
                0,
                300,
            );
            assert_eq!(o.post, "ring", "member {own}");
        }
    }

    /// BOT-136: at the plant report a cover leaves for a post of the bomb's site at once (no stand near the bomb),
    /// the defusers run to the bomb, and a guarding attacker takes a post too.
    #[test]
    fn at_the_plant_the_covers_take_posts_and_the_defusers_run() {
        let bomb = [3000.0, 0.0, 40.0];
        let posts = ring([3000.0, 0.0, 0.0], 320.0, 4);
        let mk = |own: usize, attacking: bool| {
            let mut t = team(4, own, attacking);
            t.posts = vec![posts.clone(), Vec::new()];
            for (i, m) in t.mates.iter_mut().enumerate() {
                m.pos = Some([2900.0 - 300.0 * i as f32, 0.0, 0.0]);
            }
            t.own_pos = t.mates[own].pos.unwrap();
            t
        };
        // before the plant member 3 holds site 1 (no posts there): the BOT-103 hold
        let mut t = mk(3, false);
        let mut sq = Squad::default();
        let o = step(&mut sq, &t, 0, 3000);
        assert_eq!(o.goal.unwrap().stand_within, SITE_HOLD);
        t.planted = true;
        t.bomb_pos = Some(bomb);
        let o = sq.order(3100, &t, &mut same_place).unwrap();
        let g = o.goal.unwrap();
        assert!(!o.defuse && !o.wait);
        assert!(
            posts.iter().any(|p| flat(*p, g.goal) < 1.0),
            "a post: {g:?}"
        );
        assert_eq!(g.stand_within, POST_STAND);
        let d = mk(0, false);
        let o = step(
            &mut Squad::default(),
            &Team {
                planted: true,
                bomb_pos: Some(bomb),
                ..d
            },
            0,
            200,
        );
        assert!(o.defuse);
        assert_eq!(o.goal.unwrap().goal, bomb);
        let a = mk(2, true);
        let o = step(
            &mut Squad::default(),
            &Team {
                planted: true,
                bomb_pos: Some(bomb),
                ..a
            },
            0,
            200,
        );
        assert!(posts.iter().any(|p| flat(*p, o.goal.unwrap().goal) < 1.0));
    }
}
