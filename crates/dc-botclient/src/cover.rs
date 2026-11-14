//! BOT-173: the shared cover-spot scorer, sans-IO. Level rays from a place tell how much wall stands close round it
//! and how much open ground it faces (the Quarantine stand, BOT-160, ranks its spots from the same rays); a guard's
//! spot round the place it guards (a flag base, a bomb site, the planted bomb, the uplink) is scored from them, from
//! its sight lines to the guarded place and to the approaches the enemy walks in by, and from its height. A DeCombat
//! bot behaviour, no 2009 rule.
// DC-DEFAULT: BOT-173 (proposed)

/// BOT-160 / BOT-173: the number of level rays round a place (one every 22.5 degrees).
pub const RAYS: usize = 16;

/// BOT-160 / BOT-173: the share (0..1) of each of the [`RAYS`] level rays of `len` units from `p` that runs free
/// (`free(from, to)`, the caller adds the eye height), ray `k` at `k / RAYS` of a turn (x by the sine, z by the
/// cosine of the angle).
pub fn ray_shares(p: [f32; 3], len: f32, mut free: impl FnMut([f32; 3], [f32; 3]) -> f32) -> [f32; RAYS] {
    let mut out = [0.0f32; RAYS];
    for (k, o) in out.iter_mut().enumerate() {
        let a = ray_angle(k);
        let to = [p[0] + a.sin() * len, p[1], p[2] + a.cos() * len];
        *o = free(p, to).clamp(0.0, 1.0);
    }
    out
}

/// The angle of ray `k` (the yaw convention of the aim: x by the sine, z by the cosine).
pub fn ray_angle(k: usize) -> f32 {
    k as f32 * std::f32::consts::TAU / RAYS as f32
}

/// `scored` (place, score, tie order) best first, each kept only when it lies at least `apart` (flat) from every
/// place kept before it; at most `count`.
pub fn best_apart<T: Copy>(mut scored: Vec<(f32, usize, T)>, apart: f32, count: usize, at: impl Fn(&T) -> [f32; 3]) -> Vec<T> {
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out: Vec<T> = Vec::new();
    for (_, _, s) in scored {
        if out.len() >= count {
            break;
        }
        if out.iter().all(|q| flat(at(q), at(&s)) >= apart) {
            out.push(s);
        }
    }
    out
}

/// BOT-173: a guard spot's rays: [`COVER_RAY_LEN`] long; a ray stopped within [`COVER_WALL`] is cover (a wall, a
/// crate, a pillar), a ray free for [`COVER_OPEN`] of its length is open ground. The score: cover rays (at most half
/// of them count, x [`W_COVER`]), the cover share of the rays at the spot's back (the five turned away from where
/// the enemy comes from, x [`W_BACK`]), minus the open share (x [`W_OPEN`]), plus the share of the approaches it sees
/// (x [`W_APPROACH`]), its height over the guarded place (up to [`HIGH_FULL`], x [`W_HIGH`]), minus its distance
/// from the band's ideal (x [`W_IDEAL`] per ideal); a spot without a sight line to the guarded place loses
/// [`BLIND_COST`] (it ranks under every spot that sees it).
// DC-DEFAULT: BOT-173 (proposed) rays of 600 units, cover within 200, open from 0.9; weights cover 3, back 1, open
// 2, approaches 2, height 1 (full at 128 units), ideal 0.5, blind 10
pub const COVER_RAY_LEN: f32 = 600.0;
pub const COVER_WALL: f32 = 200.0;
pub const COVER_OPEN: f32 = 0.9;
pub const W_COVER: f32 = 3.0;
pub const W_BACK: f32 = 1.0;
pub const W_OPEN: f32 = 2.0;
pub const W_APPROACH: f32 = 2.0;
pub const W_HIGH: f32 = 1.0;
pub const HIGH_FULL: f32 = 128.0;
pub const W_IDEAL: f32 = 0.5;
pub const BLIND_COST: f32 = 10.0;

/// BOT-173: where the guard spots of a place lie: `min`..`max` units (flat) from it, at most `rise` higher or
/// lower, `ideal` the best distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub min: f32,
    pub max: f32,
    pub ideal: f32,
    pub rise: f32,
}

impl Band {
    pub fn holds(&self, center: [f32; 3], p: [f32; 3]) -> bool {
        (self.min..=self.max).contains(&flat(p, center)) && (p[1] - center[1]).abs() <= self.rise
    }
}

/// BOT-173: a scored guard spot: the node, its score, the share of its rays that are open, where it looks
/// (the guarded place and the approaches it sees) and a peek place (a node a half step out toward an approach).
#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    pub at: [f32; 3],
    pub score: f32,
    pub open: f32,
    pub looks: Vec<[f32; 3]>,
    pub peek: Option<[f32; 3]>,
    /// BOT-179: whether the spot sees the guarded place, and whether its peek place does.
    pub sees: bool,
    pub peek_sees: bool,
}

/// BOT-173: the cover score of spot `p` from its rays `shares` (see [`COVER_RAY_LEN`]), whether it sees the
/// guarded place `center`, which of the `approaches` it sees (`seen`), and the band.
pub fn score(p: [f32; 3], center: [f32; 3], shares: &[f32; RAYS], sees_center: bool, approaches: &[[f32; 3]], seen: &[bool], band: Band) -> f32 {
    let wall = |s: f32| s * COVER_RAY_LEN < COVER_WALL;
    let walls = shares.iter().filter(|s| wall(**s)).count();
    let open = shares.iter().filter(|s| **s >= COVER_OPEN).count() as f32 / RAYS as f32;
    // the enemy comes from the approaches (the guarded place when none is known): the back is the other way
    let mut v = [0.0f32; 2];
    for a in approaches.iter().chain(std::iter::once(&center)) {
        let d = flat(*a, p).max(1.0);
        v[0] += (a[0] - p[0]) / d;
        v[1] += (a[2] - p[2]) / d;
    }
    let back = if v[0].hypot(v[1]) > 1e-3 {
        let front = v[0].atan2(v[1]);
        let behind: Vec<usize> = (0..RAYS).filter(|k| angle_off(ray_angle(*k), front + std::f32::consts::PI) <= 1.2).collect();
        behind.iter().filter(|k| wall(shares[**k])).count() as f32 / behind.len().max(1) as f32
    } else {
        0.0
    };
    let seen_share = if approaches.is_empty() {
        0.0
    } else {
        seen.iter().filter(|s| **s).count() as f32 / approaches.len() as f32
    };
    let high = ((p[1] - center[1]) / HIGH_FULL).clamp(0.0, 1.0);
    let ideal = (flat(p, center) - band.ideal).abs() / band.ideal.max(1.0);
    W_COVER * (walls.min(RAYS / 2) as f32 / (RAYS / 2) as f32) + W_BACK * back - W_OPEN * open
        + W_APPROACH * seen_share
        + W_HIGH * high
        - W_IDEAL * ideal
        - if sees_center { 0.0 } else { BLIND_COST }
}

/// BOT-173: at most this many grid nodes of a band are scored (picked spread out, nearest the ideal first), the
/// guard spots kept at least [`SPOT_APART`] apart, at most [`SPOTS_MAX`] per guarded place.
// DC-DEFAULT: BOT-173 (proposed) 40 nodes, 64 units between two tested, spots 192 apart (BOT-144), 8 per place
pub const SCORE_TESTS: usize = 40;
pub const TEST_SPREAD: f32 = 64.0;
pub const SPOT_APART: f32 = 192.0;
pub const SPOTS_MAX: usize = 8;
/// BOT-173: an approach is the place the enemy's way (the grid path from its side) first comes within this of the
/// guarded place; with no path the point this far toward the enemy's side.
// DC-DEFAULT: BOT-173 (proposed) 900 units
pub const APPROACH_R: f32 = 900.0;
/// BOT-173: a peek place lies [`PEEK_NEAR`]..[`PEEK_FAR`] units from its spot.
// DC-DEFAULT: BOT-173 (proposed) 72..200 units
pub const PEEK_NEAR: f32 = 72.0;
pub const PEEK_FAR: f32 = 200.0;
/// BOT-179: while fewer than [`SEEING_WANT`] band nodes see the guarded place, the nodes from [`NEAR_MIN`] to the
/// band's inner edge are scored too; every guard holds at least [`SHARE_MIN`] spots that see it (a blind spot is
/// never a guard's only one); a spot within [`TAKEN`] of a teammate guard is taken; a guard stays at most
/// [`STAY_CAP`] times its dwell at a spot; with no free place to go it stays and tries again after
/// [`STAY_RETRY_MS`] (event `why: stay`, the spots ranked again).
// DC-DEFAULT: BOT-179 (proposed) 4 seeing nodes, 64 units, 2 spots, 96 units, 2 x the dwell, 3 s
pub const SEEING_WANT: usize = 4;
pub const NEAR_MIN: f32 = 64.0;
pub const SHARE_MIN: usize = 2;
pub const TAKEN: f32 = 96.0;
pub const STAY_CAP: u32 = 2;
pub const STAY_RETRY_MS: u32 = 3000;

/// BOT-173: the nodes of the band round `center` to score: `nodes` in the band, nearest the ideal first, each at
/// least [`TEST_SPREAD`] from those taken, at most [`SCORE_TESTS`].
pub fn candidates(center: [f32; 3], nodes: &[[f32; 3]], band: Band) -> Vec<[f32; 3]> {
    let mut c: Vec<[f32; 3]> = nodes.iter().copied().filter(|n| band.holds(center, *n)).collect();
    c.sort_by(|a, b| {
        (flat(*a, center) - band.ideal)
            .abs()
            .total_cmp(&(flat(*b, center) - band.ideal).abs())
            .then(a[0].total_cmp(&b[0]))
            .then(a[2].total_cmp(&b[2]))
    });
    let mut out: Vec<[f32; 3]> = Vec::new();
    for n in c {
        if out.len() >= SCORE_TESTS {
            break;
        }
        if out.iter().all(|t| flat(*t, n) >= TEST_SPREAD) {
            out.push(n);
        }
    }
    out
}

/// BOT-173: the approach from a way `path` of the enemy (from its side toward `center`): the point where it first
/// comes within [`APPROACH_R`] (flat), on the segment that crosses; the way's end when it never comes that near but
/// ends within twice that; else none.
pub fn approach_on(path: &[[f32; 3]], center: [f32; 3]) -> Option<[f32; 3]> {
    let first = *path.first()?;
    if flat(first, center) <= APPROACH_R {
        return Some(first);
    }
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        if flat(b, center) <= APPROACH_R {
            // the crossing by bisection on the segment (the flat distance is convex along it)
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            for _ in 0..12 {
                let m = 0.5 * (lo + hi);
                if flat(lerp(a, b, m), center) <= APPROACH_R {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            return Some(lerp(a, b, hi));
        }
    }
    let last = *path.last()?;
    (flat(last, center) <= 2.0 * APPROACH_R).then_some(last)
}

/// BOT-173: the approach toward `from` when the grid knows no way: the point [`APPROACH_R`] from `center` toward it
/// (at the centre's height).
pub fn approach_toward(from: [f32; 3], center: [f32; 3]) -> [f32; 3] {
    let d = flat(from, center).max(1.0);
    let k = (APPROACH_R / d).min(1.0);
    [center[0] + (from[0] - center[0]) * k, center[1], center[2] + (from[2] - center[2]) * k]
}

/// BOT-173: the peek place of a spot `p`: of `nodes`, one [`PEEK_NEAR`]..[`PEEK_FAR`] from it on about its level
/// whose direction lies nearest the first thing the spot looks at (`look`).
pub fn peek_place(p: [f32; 3], look: [f32; 3], nodes: &[[f32; 3]]) -> Option<[f32; 3]> {
    let want = yaw(p, look);
    nodes
        .iter()
        .copied()
        .filter(|n| (PEEK_NEAR..=PEEK_FAR).contains(&flat(*n, p)) && (n[1] - p[1]).abs() <= 48.0)
        .min_by(|a, b| angle_off(yaw(p, *a), want).total_cmp(&angle_off(yaw(p, *b), want)))
        .filter(|n| angle_off(yaw(p, *n), want) <= std::f32::consts::FRAC_PI_2)
}

/// BOT-173: the guard spots round `center`: the band's [`candidates`] among `nodes`, each scored ([`score`]) from
/// its rays (`free`, as [`ray_shares`]) and sight lines (`sees(from, to)`, node to place, the caller adds the eye
/// heights), best first and [`SPOT_APART`] apart, at most [`SPOTS_MAX`]; each with its peek place among `nodes`.
/// BOT-179: while fewer than [`SEEING_WANT`] of the band's nodes see the place, the nodes nearer it
/// ([`NEAR_MIN`] up to the band's inner edge, nearest that edge first) are scored too.
pub fn rank_guard_spots(
    center: [f32; 3],
    nodes: &[[f32; 3]],
    band: Band,
    approaches: &[[f32; 3]],
    mut free: impl FnMut([f32; 3], [f32; 3]) -> f32,
    mut sees: impl FnMut([f32; 3], [f32; 3]) -> bool,
) -> Vec<Spot> {
    let mut eval = |p: [f32; 3]| {
        let shares = ray_shares(p, COVER_RAY_LEN, &mut free);
        let sees_center = sees(p, center);
        let seen: Vec<bool> = approaches.iter().map(|a| sees(p, *a)).collect();
        let s = score(p, center, &shares, sees_center, approaches, &seen, band);
        let mut looks = vec![center];
        looks.extend(approaches.iter().zip(&seen).filter(|(_, s)| **s).map(|(a, _)| *a));
        Spot {
            at: p,
            score: s,
            open: open_share(&shares),
            looks,
            peek: None,
            sees: sees_center,
            peek_sees: false,
        }
    };
    let mut spots: Vec<Spot> = candidates(center, nodes, band).into_iter().map(&mut eval).collect();
    if spots.iter().filter(|s| s.sees).count() < SEEING_WANT && band.min > NEAR_MIN {
        let near = Band {
            min: NEAR_MIN,
            max: band.min,
            ideal: band.min,
            rise: band.rise,
        };
        let extra: Vec<[f32; 3]> = candidates(center, nodes, near)
            .into_iter()
            .filter(|n| spots.iter().all(|s| flat(s.at, *n) >= TEST_SPREAD))
            .collect();
        spots.extend(extra.into_iter().map(&mut eval));
    }
    let order: Vec<(f32, usize, usize)> = spots.iter().enumerate().map(|(i, s)| (s.score, i, i)).collect();
    let picked = best_apart(order, SPOT_APART, SPOTS_MAX, |i| spots[*i].at);
    picked
        .into_iter()
        .map(|i| {
            let mut s = spots[i].clone();
            s.peek = peek_place(s.at, *s.looks.get(1).unwrap_or(&center), nodes);
            s.peek_sees = s.peek.is_some_and(|q| sees(q, center));
            s
        })
        .collect()
}

/// BOT-173: the rays ([`COVER_RAY_LEN`]) of a body place `p` through a world's collision at eye height (half the
/// bot's standing height over the place), as the session casts them.
pub fn world_shares(coll: &dc_sim::CollisionWorld, p: [f32; 3]) -> [f32; RAYS] {
    use dc_sim::{Collider, Cylinder, V3};
    let eye = 0.5 * crate::motion::BOT_STAND_DIMS[1];
    let shape = Cylinder::from_dims(V3::from_array(crate::hunt::RAY_DIMS));
    let up = |q: [f32; 3]| V3::new(q[0], q[1] + eye, q[2]);
    ray_shares(p, COVER_RAY_LEN, |a, b| coll.sweep(&shape, up(a), up(b)).map_or(1.0, |h| h.t))
}

/// BOT-173: the open share at a place from its rays (the evidence's "standing in the open" measure).
pub fn open_share(shares: &[f32; RAYS]) -> f32 {
    shares.iter().filter(|s| **s >= COVER_OPEN).count() as f32 / RAYS as f32
}

/// BOT-173: each guard holds up to [`SPOTS_PER_GUARD`] of the ranked spots of its place (guard `g` of `n` the
/// spots `g`, `g + n`, `g + 2n`, ... so the guards of a team share them out; the first guard with fewer than two of
/// its own also gets the others, a later guard never: where a flag room has only two spots, two guards that both
/// held both swapped them all the time (Death Room: 31 % of the time within 150 units)), and moves between them: it stays at a spot it reached
/// for [`dwell_ms`] (a random [`DWELL_MIN_MS`]..+[`DWELL_SPAN_MS`], up to [`SKILL_SHORTEN`] shorter for a skilled
/// bot), passes over a spot not reached in [`REACH_MS`], leaves a spot an earlier guard of its team stands within
/// [`SPOT_APART`] of, and after a fight away from its spot ([`CONTACT_MS`] since it saw an enemy, at most once per
/// [`CONTACT_MS`]) goes back to the spot nearest to it that no teammate guard stands near. The next spot is a random other one of its own no teammate guard stands near. At its spot
/// every [`PEEK_GAP_MIN_MS`]..+[`PEEK_GAP_SPAN_MS`] it steps out to the spot's peek place for [`PEEK_MS`] and back,
/// and it turns its view between the guarded place and the approaches it sees every
/// [`LOOK_MIN_MS`]..+[`LOOK_SPAN_MS`], with a slow sway of [`SWAY`] radians round each. Before, a flag guard stood
/// exactly 10 s (the ring's turn) on one node and relocated (CTF review round 2, defect 4).
// DC-DEFAULT: BOT-173 (proposed) 4 spots, 6..20 s, 30 % shorter at skill 100, 15 s to reach, stand within 48 and
// reached within 112 units, 4 s after contact, peeks every 4..8 s for 1.8 s, a look every 1.5..4 s, sway 0.35 rad
pub const SPOTS_PER_GUARD: usize = 4;
pub const DWELL_MIN_MS: u32 = 6000;
pub const DWELL_SPAN_MS: u32 = 14_000;
pub const SKILL_SHORTEN: f32 = 0.3;
pub const REACH_MS: u32 = 15_000;
/// BOT-173: the time to reach a spot runs only while the guard is within this of it.
pub const REACH_NEAR: f32 = 600.0;
pub const STAND: f32 = 48.0;
pub const ARRIVE: f32 = 112.0;
pub const CONTACT_MS: u32 = 4000;
/// BOT-173: after contact another spot is taken only when it is this much nearer than the held one.
pub const CONTACT_NEARER: f32 = 150.0;
pub const PEEK_GAP_MIN_MS: u32 = 4000;
pub const PEEK_GAP_SPAN_MS: u32 = 4000;
pub const PEEK_MS: u32 = 1800;
pub const LOOK_MIN_MS: u32 = 1500;
pub const LOOK_SPAN_MS: u32 = 2500;
pub const SWAY: f32 = 0.35;

/// BOT-173: guard `g` of `n`'s share of the `ranked` spots (best first). See [`SPOTS_PER_GUARD`].
/// BOT-179: only the spots that see the guarded place are shared out (guard `g` the seeing spots `g`, `g + n`,
/// ...); a guard with fewer than [`SHARE_MIN`] of its own also gets the others' last ones (any guard, not only the
/// first: the runtime keeps two guards off one spot by [`TAKEN`]); then the peek places of its spots that see the
/// place ([`peek_spot`]); a blind spot only fills up to [`SHARE_MIN`] beside a seeing one when nothing else does;
/// with no seeing spot at all the share is empty (the ring's place then).
pub fn share(ranked: &[Spot], g: usize, n: usize) -> Vec<Spot> {
    let n = n.max(1);
    let seeing: Vec<&Spot> = ranked.iter().filter(|s| s.sees).collect();
    let mut own: Vec<Spot> = seeing.iter().skip(g % n).step_by(n).take(SPOTS_PER_GUARD).map(|s| (*s).clone()).collect();
    let fill = |own: &mut Vec<Spot>, from: &mut dyn Iterator<Item = &Spot>| {
        for s in from {
            if own.len() >= SHARE_MIN {
                break;
            }
            if !own.iter().any(|o| o.at == s.at) {
                own.push(s.clone());
            }
        }
    };
    fill(&mut own, &mut seeing.iter().rev().copied());
    if own.is_empty() {
        return own;
    }
    let peeks: Vec<Spot> = own.iter().filter_map(peek_spot).collect();
    fill(&mut own, &mut peeks.iter());
    fill(&mut own, &mut ranked.iter().filter(|s| !s.sees));
    own
}

/// BOT-179: the peek place of spot `s` as a spot of its own when it sees the guarded place (its looks, no peek).
pub fn peek_spot(s: &Spot) -> Option<Spot> {
    let at = s.peek.filter(|pk| s.peek_sees && flat(*pk, s.at) >= 1.0)?;
    Some(Spot {
        at,
        score: if s.sees { s.score } else { s.score + BLIND_COST },
        open: s.open,
        looks: s.looks.clone(),
        peek: None,
        sees: true,
        peek_sees: false,
    })
}

/// BOT-173: how long a guard stays at a spot it reached (`seed` a mixed random number, `level` the bot's skill
/// 0..100). See [`DWELL_MIN_MS`].
pub fn dwell_ms(seed: u64, level: Option<u8>) -> u32 {
    let base = DWELL_MIN_MS + (mix(seed) % u64::from(DWELL_SPAN_MS + 1)) as u32;
    let k = 1.0 - SKILL_SHORTEN * f32::from(level.unwrap_or(0).min(100)) / 100.0;
    (base as f32 * k) as u32
}

/// A well-mixed 64-bit number from `x` (SplitMix64).
pub fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// BOT-173: why a guard took a spot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    First,
    Dwell,
    Reach,
    Crowd,
    Contact,
    /// BOT-179: it stood [`STAY_CAP`] times its dwell (contact or not) and moves on.
    Cap,
    /// BOT-179: no free place to go: it stays where it is (the spot it holds is returned) and asks for new spots.
    Stay,
}

impl Why {
    pub fn name(self) -> &'static str {
        match self {
            Why::First => "first",
            Why::Dwell => "dwell",
            Why::Reach => "reach",
            Why::Crowd => "crowd",
            Why::Contact => "contact",
            Why::Cap => "cap",
            Why::Stay => "stay",
        }
    }
}

/// BOT-173: a guard's spots and where it is among them. See [`SPOTS_PER_GUARD`].
#[derive(Debug, Clone, Default)]
pub struct Hold {
    /// The guarded place the spots belong to and when they were ranked.
    pub center: [f32; 3],
    pub ranked: bool,
    pub ranked_at: u32,
    pub spots: Vec<Spot>,
    /// BOT-179: every seeing spot of the place (the team's), taken when none of its own is free.
    pub pool: Vec<Spot>,
    pub cur: Option<Spot>,
    /// BOT-179: it found no free place to go and wants its spots ranked again.
    pub rerank: bool,
    since: u32,
    arrived: Option<u32>,
    dwell: u32,
    moves: u64,
    peek_until: u32,
    next_peek: u32,
    look: Option<[f32; 3]>,
    next_look: u32,
    retry_at: u32,
}

impl Hold {
    /// New spots for the place `center` (the spot it holds stays until it moves on; none when it has no spot) and
    /// the place's `pool` of seeing spots.
    pub fn set_spots(&mut self, center: [f32; 3], spots: Vec<Spot>, pool: Vec<Spot>, now: u32) {
        if flat(center, self.center) > 64.0 || spots.is_empty() {
            self.cur = None;
        }
        self.center = center;
        self.spots = spots;
        self.pool = pool;
        self.ranked = true;
        self.ranked_at = now;
        self.rerank = false;
    }

    /// The dwell at the spot it holds (0 before it got there).
    pub fn dwell(&self) -> u32 {
        if self.arrived.is_some() { self.dwell } else { 0 }
    }

    /// One tick at `now` with the body at `pos`: `crowd` the places of the team's earlier guards (a spot by one of
    /// them is left), `avoid` those of all its other guards (no new spot by one of them), `contact` whether it saw an
    /// enemy in the last [`CONTACT_MS`], `seed` the bot's random base, `level` its skill. The spot it takes and why,
    /// when it takes one.
    pub fn tick(
        &mut self,
        now: u32,
        pos: Option<[f32; 3]>,
        crowd: &[[f32; 3]],
        avoid: &[[f32; 3]],
        contact: bool,
        (seed, level): (u64, Option<u8>),
    ) -> Option<(Spot, Why, u32)> {
        let free = |s: &&Spot| avoid.iter().all(|a| flat(*a, s.at) >= TAKEN);
        let Some(cur) = self.cur.clone() else {
            let first = self
                .spots
                .iter()
                .find(free)
                .or_else(|| self.pool.iter().find(free))
                .or(self.spots.first())?
                .clone();
            return Some(self.take(first, Why::First, now, seed, level));
        };
        if let Some(p) = pos {
            // the time to reach a spot runs only near it (a guard back from its spawn walks far)
            if self.arrived.is_none() && flat(p, cur.at) > REACH_NEAR {
                self.since = now;
            }
            if self.arrived.is_none() && flat(p, cur.at) <= ARRIVE {
                self.arrived = Some(now);
                self.next_peek = now + PEEK_GAP_MIN_MS + (mix(seed ^ u64::from(now) ^ 0x7e) % u64::from(PEEK_GAP_SPAN_MS)) as u32;
            }
            if contact && flat(p, cur.at) > ARRIVE && now.wrapping_sub(self.since) >= CONTACT_MS {
                let near = self
                    .spots
                    .iter()
                    .filter(free)
                    .min_by(|a, b| flat(a.at, p).total_cmp(&flat(b.at, p)))
                    .filter(|s| flat(s.at, p) + CONTACT_NEARER < flat(cur.at, p))
                    .cloned();
                if let Some(n) = near {
                    return Some(self.take(n, Why::Contact, now, seed, level));
                }
            }
        }
        // BOT-179: after a stay it tries again only after STAY_RETRY_MS (it still peeks meanwhile)
        let waiting = now < self.retry_at;
        if !waiting && self.arrived.is_some_and(|t| now.wrapping_sub(t) >= STAY_CAP * self.dwell.max(1)) {
            return self.next(&cur, Why::Cap, now, avoid, seed, level);
        }
        if !waiting && crowd.iter().any(|m| flat(*m, cur.at) < TAKEN) {
            return self.next(&cur, Why::Crowd, now, avoid, seed, level);
        }
        match self.arrived {
            _ if waiting => {
                self.peek_tick(now, contact, &cur, seed);
                None
            }
            Some(t) if !contact && now.wrapping_sub(t) >= self.dwell => self.next(&cur, Why::Dwell, now, avoid, seed, level),
            None if !contact && now.wrapping_sub(self.since) >= REACH_MS => self.next(&cur, Why::Reach, now, avoid, seed, level),
            _ => {
                self.peek_tick(now, contact, &cur, seed);
                None
            }
        }
    }

    fn peek_tick(&mut self, now: u32, contact: bool, cur: &Spot, seed: u64) {
        if self.arrived.is_some() && !contact && now >= self.next_peek && cur.peek.is_some() {
            self.peek_until = now + PEEK_MS;
            self.next_peek = now + PEEK_MS + PEEK_GAP_MIN_MS + (mix(seed ^ u64::from(now)) % u64::from(PEEK_GAP_SPAN_MS)) as u32;
        }
    }

    /// The place the guard walks to now: its spot, or the spot's peek place while it peeks.
    pub fn goal(&self, now: u32) -> Option<[f32; 3]> {
        let cur = self.cur.as_ref()?;
        Some(match cur.peek {
            Some(p) if now < self.peek_until => p,
            _ => cur.at,
        })
    }

    /// Whether it peeks at `now`.
    pub fn peeking(&self, now: u32) -> bool {
        now < self.peek_until && self.cur.as_ref().is_some_and(|c| c.peek.is_some())
    }

    /// The place the guard looks at now (one of its spot's looks, changed now and then) and the sway round it
    /// (radians, added to the yaw).
    pub fn look(&mut self, now: u32, seed: u64) -> Option<([f32; 3], f32)> {
        let cur = self.cur.as_ref()?;
        if self.look.is_none() || now >= self.next_look {
            let r = mix(seed ^ u64::from(now) ^ 0x10c);
            self.look = cur.looks.get((r % cur.looks.len().max(1) as u64) as usize).copied();
            self.next_look = now + LOOK_MIN_MS + ((r >> 20) % u64::from(LOOK_SPAN_MS)) as u32;
        }
        let sway = SWAY * (now as f32 / 1000.0 * 0.9 + (seed % 7) as f32).sin();
        self.look.map(|l| (l, sway))
    }

    /// BOT-179: where it goes from `cur`: a random free other spot of its own, else the best free seeing spot of the
    /// pool, else its spot's peek place when that sees the guarded place and is free; at the stay cap ([`Why::Cap`])
    /// also a place a teammate guard stands near (the one farthest from the guards). With none it stays: the spot it
    /// holds comes back as [`Why::Stay`], it asks for new spots and tries again after [`STAY_RETRY_MS`] (at the cap
    /// its stay starts over). Before, it stayed silently and dwelt again, without end (CTF review round 3: 150 s on
    /// one blind spot, two guards on one spot for 44 s).
    fn next(&mut self, cur: &Spot, why: Why, now: u32, avoid: &[[f32; 3]], seed: u64, level: Option<u8>) -> Option<(Spot, Why, u32)> {
        let free = |p: [f32; 3]| avoid.iter().all(|a| flat(*a, p) >= TAKEN);
        let other = |s: &&Spot| flat(s.at, cur.at) >= TAKEN;
        let own: Vec<&Spot> = self.spots.iter().filter(other).filter(|s| free(s.at)).collect();
        let pick = if !own.is_empty() {
            Some(own[(mix(seed ^ self.moves ^ u64::from(now)) % own.len() as u64) as usize].clone())
        } else if let Some(p) = self.pool.iter().filter(other).find(|s| free(s.at)) {
            Some(p.clone())
        } else if let Some(pk) = peek_spot(cur).filter(|p| free(p.at)) {
            Some(pk)
        } else if why == Why::Cap {
            let room = |p: [f32; 3]| avoid.iter().map(|a| flat(*a, p)).fold(f32::INFINITY, f32::min);
            let mut any: Vec<Spot> = self.spots.iter().chain(&self.pool).filter(other).cloned().collect();
            if let Some(pk) = cur.peek.filter(|pk| flat(*pk, cur.at) >= 1.0) {
                any.push(Spot {
                    at: pk,
                    peek: None,
                    peek_sees: false,
                    sees: cur.peek_sees,
                    ..cur.clone()
                });
            }
            any.into_iter().max_by(|a, b| room(a.at).total_cmp(&room(b.at)))
        } else {
            None
        };
        if let Some(p) = pick {
            return Some(self.take(p, why, now, seed, level));
        }
        self.retry_at = now + STAY_RETRY_MS;
        self.rerank = true;
        if why == Why::Cap {
            self.arrived = Some(now);
        }
        Some((cur.clone(), Why::Stay, self.dwell))
    }

    fn take(&mut self, s: Spot, why: Why, now: u32, seed: u64, level: Option<u8>) -> (Spot, Why, u32) {
        self.moves += 1;
        self.cur = Some(s.clone());
        self.since = now;
        self.arrived = None;
        self.peek_until = 0;
        self.look = None;
        self.retry_at = 0;
        // the dwell is drawn when the spot is reached; logged with the draw it will get
        let d = dwell_ms(seed ^ self.moves.wrapping_mul(0x51) ^ u64::from(now), level);
        self.dwell = d;
        (s, why, d)
    }
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn yaw(a: [f32; 3], b: [f32; 3]) -> f32 {
    (b[0] - a[0]).atan2(b[2] - a[2])
}

fn angle_off(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(std::f32::consts::TAU);
    d.min(std::f32::consts::TAU - d)
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAND: Band = Band {
        min: 150.0,
        max: 450.0,
        ideal: 300.0,
        rise: 160.0,
    };

    /// A wall along x = 400 (east of the flag room) and a wall along z = -400: a ray stops where it crosses one.
    fn walls(from: [f32; 3], to: [f32; 3]) -> f32 {
        let mut t = 1.0f32;
        for (axis, at) in [(0usize, 400.0f32), (2, -400.0)] {
            let (a, b) = (from[axis], to[axis]);
            if (a - at) * (b - at) < 0.0 {
                t = t.min((at - a) / (b - a));
            }
        }
        t
    }

    /// BOT-173: a node with a wall close beside it outranks one in the open, both seeing the flag; a node that does
    /// not see the flag ranks last whatever its cover; spots stay 192 apart.
    #[test]
    fn a_node_by_a_wall_beats_an_open_one_and_a_blind_one_loses() {
        let flag = [0.0, 0.0, 0.0];
        let open = [0.0, 0.0, 300.0];
        let walled = [300.0, 0.0, -300.0]; // in the corner of the two walls
        let blind = [-300.0, 0.0, -300.0];
        let nodes = [open, walled, blind];
        let r = rank_guard_spots(flag, &nodes, BAND, &[], walls, |a, _| a != blind);
        let at: Vec<[f32; 3]> = r.iter().map(|s| s.at).collect();
        assert_eq!(at, vec![walled, open, blind], "{r:?}");
        assert!(r[0].score > r[1].score && r[2].score < r[1].score - 5.0);
        assert!(r[0].open < r[1].open, "the corner faces less open ground");
        // a node out of the band is no spot; two nodes closer than SPOT_APART give one
        assert!(rank_guard_spots(flag, &[[0.0, 0.0, 600.0]], BAND, &[], walls, |_, _| true).is_empty());
        let two = rank_guard_spots(flag, &[walled, [300.0, 0.0, -150.0]], BAND, &[], walls, |_, _| true);
        assert_eq!(two.len(), 1);
    }

    /// BOT-173: seeing the approach counts, the back to a wall away from the approach counts, height counts.
    #[test]
    fn approaches_and_height_raise_the_score() {
        let flag = [0.0, 0.0, 0.0];
        let shares = [1.0f32; RAYS];
        let p = [300.0, 0.0, 0.0];
        let app = [[900.0, 0.0, 0.0]];
        let base = score(p, flag, &shares, true, &app, &[false], BAND);
        let sees = score(p, flag, &shares, true, &app, &[true], BAND);
        assert!((sees - base - W_APPROACH).abs() < 1e-4);
        let high = score([300.0, 200.0, 0.0], flag, &shares, true, &app, &[false], BAND);
        assert!((high - base - W_HIGH).abs() < 1e-4);
        // walls at the back: the guarded place and the approach lie toward -z, the walls toward +z
        let mut back = shares;
        for (k, s) in back.iter_mut().enumerate() {
            if angle_off(ray_angle(k), 0.0) <= 1.0 {
                *s = 0.1; // walls to +z
            }
        }
        let east_only = [[300.0, 0.0, -900.0]];
        let a = score(p, [300.0, 0.0, -300.0], &back, true, &east_only, &[false], BAND);
        let b = score(p, [300.0, 0.0, -300.0], &shares, true, &east_only, &[false], BAND);
        assert!(a > b + W_BACK * 0.5, "{a} {b}");
    }

    /// BOT-173: the approach is where the enemy's way first comes within APPROACH_R of the place.
    #[test]
    fn the_approach_is_where_the_way_comes_near() {
        let c = [0.0, 0.0, 0.0];
        let path = [[3000.0, 0.0, 0.0], [2000.0, 0.0, 0.0], [500.0, 0.0, 0.0], c];
        let a = approach_on(&path, c).unwrap();
        assert!((flat(a, c) - APPROACH_R).abs() < 2.0 && a[0] > 0.0, "{a:?}");
        assert_eq!(approach_on(&[[100.0, 0.0, 0.0]], c), Some([100.0, 0.0, 0.0]));
        assert_eq!(approach_on(&[[5000.0, 0.0, 0.0]], c), None);
        let t = approach_toward([0.0, 0.0, -5000.0], c);
        assert!((t[2] + APPROACH_R).abs() < 0.1);
        assert_eq!(peek_place([0.0; 3], [0.0, 0.0, 1000.0], &[[0.0, 0.0, 120.0], [0.0, 0.0, -120.0]]), Some([0.0, 0.0, 120.0]));
        assert_eq!(peek_place([0.0; 3], [0.0, 0.0, 1000.0], &[[0.0, 0.0, -120.0]]), None, "not away from the look");
    }

    /// BOT-173: the dwell is random in 6..20 s (no fixed period), shorter for a skilled bot.
    #[test]
    fn the_dwell_is_random_and_skill_shortens_it() {
        let d: Vec<u32> = (0..200u64).map(|i| dwell_ms(i, None)).collect();
        assert!(d.iter().all(|x| (DWELL_MIN_MS..=DWELL_MIN_MS + DWELL_SPAN_MS).contains(x)));
        let (lo, hi) = (d.iter().min().unwrap(), d.iter().max().unwrap());
        assert!(*lo < 8000 && *hi > 18_000, "{lo} {hi}");
        let mut u = d.clone();
        u.sort();
        u.dedup();
        assert!(u.len() > 150, "spread out");
        for i in 0..50u64 {
            assert!(dwell_ms(i, Some(100)) < dwell_ms(i, Some(0)));
            assert!(dwell_ms(i, Some(100)) as f32 >= DWELL_MIN_MS as f32 * (1.0 - SKILL_SHORTEN) - 1.0);
        }
    }

    /// BOT-173: the S&D posts and the uplink guard take the better covered of equally placed nodes; a zero score
    /// keeps the BOT-136 order.
    #[test]
    fn site_posts_and_uplink_guards_prefer_cover() {
        use crate::squad::{post_ring, post_ring_scored, Ring};
        let site = [0.0, 0.0, 0.0];
        let nodes = [[320.0, 0.0, 0.0], [300.0, 0.0, 120.0], [-320.0, 0.0, 0.0], [0.0, 0.0, 320.0]];
        let plain = post_ring(site, &nodes, Ring::SITE, |_, _| true);
        assert_eq!(post_ring_scored(site, &nodes, Ring::SITE, |_, _| true, |_| 0.0), plain);
        // the two east nodes lie within the spread of each other: the covered one is the post
        let covered = post_ring_scored(site, &nodes, Ring::SITE, |_, _| true, |n| if n == nodes[1] { 5.0 } else { 0.0 });
        assert!(covered.contains(&nodes[1]) && !covered.contains(&nodes[0]), "{covered:?}");
        assert!(plain.contains(&nodes[0]) && !plain.contains(&nodes[1]), "{plain:?}");
        // the uplink guard: two nodes about the angle, the covered one wins; far off the angle nothing changes
        let u = [0.0, 0.0, 0.0];
        let g = [[600.0, 0.0, 0.0], [590.0, 0.0, 100.0], [-600.0, 0.0, 0.0]];
        assert_eq!(crate::spyrun::guard_place(&g, u, 0.0, 0), Some(g[0]));
        assert_eq!(crate::spyrun::guard_place_scored(&g, u, 0.0, 0, |n| if n == g[1] { 2.0 } else { 0.0 }), Some(g[1]));
        assert_eq!(crate::spyrun::guard_place_scored(&g, u, 0.0, 0, |n| if n == g[2] { 9.0 } else { 0.0 }), Some(g[0]));
    }

    fn spot(x: f32, z: f32) -> Spot {
        Spot {
            at: [x, 0.0, z],
            score: 0.0,
            open: 0.0,
            looks: vec![[0.0; 3]],
            peek: Some([x + 100.0, 0.0, z]),
            sees: true,
            peek_sees: true,
        }
    }

    /// BOT-173: two guards share the ranked spots out (none in common), each gets 2..4; a lone guard 4.
    #[test]
    fn guards_share_the_spots_out() {
        let ranked: Vec<Spot> = (0..8).map(|i| spot(i as f32 * 300.0, 0.0)).collect();
        let a = share(&ranked, 0, 2);
        let b = share(&ranked, 1, 2);
        assert_eq!((a.len(), b.len()), (4, 4));
        assert!(a.iter().all(|s| !b.iter().any(|t| t.at == s.at)));
        assert_eq!(share(&ranked, 0, 1).len(), SPOTS_PER_GUARD);
        // BOT-179: two spots, two guards: both get both (its own first; the runtime keeps them apart), one spot:
        // the later guard gets it too, and its peek place
        let few = share(&ranked[..2], 0, 2);
        assert_eq!(few.iter().map(|s| s.at).collect::<Vec<_>>(), vec![ranked[0].at, ranked[1].at]);
        let second = share(&ranked[..2], 1, 2);
        assert_eq!(second.iter().map(|s| s.at).collect::<Vec<_>>(), vec![ranked[1].at, ranked[0].at]);
        let lone = share(&ranked[..1], 1, 2);
        assert_eq!(lone.iter().map(|s| s.at).collect::<Vec<_>>(), vec![ranked[0].at, ranked[0].peek.unwrap()]);
        assert!(share(&[], 0, 2).is_empty());
    }

    /// BOT-173: a guard takes a spot, stands its dwell, peeks out and back, moves to another spot, leaves one an
    /// earlier guard stands by, passes over one it cannot reach, and after contact goes to the spot nearest it.
    #[test]
    fn a_guard_rotates_between_its_spots() {
        let spots = vec![spot(0.0, 300.0), spot(300.0, 0.0), spot(-300.0, 0.0)];
        let mut h = Hold::default();
        h.set_spots([0.0; 3], spots.clone(), Vec::new(), 0);
        let (first, why, dwell) = h.tick(0, Some([0.0, 0.0, 900.0]), &[], &[], false, (7, None)).unwrap();
        assert_eq!((first.at, why), (spots[0].at, Why::First));
        assert!((DWELL_MIN_MS..=DWELL_MIN_MS + DWELL_SPAN_MS).contains(&dwell));
        assert_eq!(h.goal(0), Some(spots[0].at));
        // arrives at 1 s
        assert!(h.tick(1000, Some(spots[0].at), &[], &[], false, (7, None)).is_none());
        assert_eq!(h.dwell(), dwell);
        // it peeks within the gap and comes back
        let mut peeked = false;
        let mut t = 1000;
        while t < 1000 + dwell - 100 {
            h.tick(t, Some(spots[0].at), &[], &[], false, (7, None));
            if h.peeking(t) {
                peeked = true;
                assert_eq!(h.goal(t), spots[0].peek);
            }
            t += 100;
        }
        assert!(peeked || dwell < PEEK_GAP_MIN_MS + PEEK_GAP_SPAN_MS);
        // the dwell over: another spot
        let (next, why, _) = h.tick(1000 + dwell, Some(spots[0].at), &[], &[], false, (7, None)).unwrap();
        assert_eq!(why, Why::Dwell);
        assert_ne!(next.at, spots[0].at);
        // an earlier guard stands by it: it leaves, never for the spot by a mate
        let mate = next.at;
        let (n2, why, _) = h.tick(1000 + dwell + 10, None, &[mate], &[mate], false, (7, None)).unwrap();
        assert_eq!(why, Why::Crowd);
        assert!(n2.at != mate && flat(n2.at, mate) >= SPOT_APART);
        // not reached in REACH_MS while near it: passed over
        let t0 = 1000 + dwell + 10;
        let near = [n2.at[0] + 200.0, 0.0, n2.at[2]];
        h.tick(t0 + 1, Some(near), &[], &[], false, (7, None));
        let r = h.tick(t0 + 2 + REACH_MS, Some(near), &[], &[], false, (7, None));
        assert_eq!(r.map(|x| x.1), Some(Why::Reach));
        // contact far from its spot, by another: that one
        let cur = h.cur.clone().unwrap().at;
        let other = spots.iter().find(|s| s.at != cur).unwrap().at;
        let by = [other[0] + 10.0, 0.0, other[2]];
        let (n3, why, _) = h.tick(t0 + 2 + REACH_MS + CONTACT_MS, Some(by), &[], &[], true, (7, None)).unwrap();
        assert_eq!((n3.at, why), (other, Why::Contact));
        // its looks: the guarded place
        assert_eq!(h.look(t0, 7).map(|l| l.0), Some([0.0; 3]));
    }

    /// BOT-173: after contact a guard does not take the spot a teammate guard stands by (before, contact and crowd
    /// swapped two spots every tick: 1189 moves in 6 min on Death Room), nor moves again within CONTACT_MS.
    #[test]
    fn contact_never_takes_a_mates_spot_and_waits_between_moves() {
        let spots = vec![spot(0.0, 300.0), spot(300.0, 0.0), spot(-300.0, 0.0)];
        let mut h = Hold::default();
        h.set_spots([0.0; 3], spots.clone(), Vec::new(), 0);
        h.tick(0, None, &[], &[], false, (3, None)).unwrap();
        let mate = spots[1].at;
        let by = [mate[0] - 20.0, 0.0, mate[2]];
        let r = h.tick(CONTACT_MS, Some(by), &[mate], &[mate], true, (3, None));
        assert!(r.as_ref().is_none_or(|m| m.0.at != mate), "{r:?}");
        let mut moves = 0;
        for t in (CONTACT_MS..CONTACT_MS + 20_000).step_by(100) {
            let p = if (t / 100) % 2 == 0 { spots[2].at } else { spots[0].at };
            let far = [p[0] + 150.0, 0.0, p[2] + 150.0];
            if h.tick(t, Some(far), &[], &[], true, (3, None)).is_some() {
                moves += 1;
            }
        }
        assert!(moves <= 20_000 / CONTACT_MS as usize, "{moves}");
    }

    fn blind(x: f32, z: f32) -> Spot {
        Spot {
            sees: false,
            score: -6.0,
            ..spot(x, z)
        }
    }

    /// BOT-179: every guard gets two spots that see the flag (Power Surge: the later guard's only spot was blind);
    /// with enough seeing spots no blind one is shared out; a blind one only beside a seeing one; none seeing: none.
    #[test]
    fn every_guard_gets_two_seeing_spots_and_a_blind_one_is_never_alone() {
        let ranked = vec![spot(0.0, 300.0), spot(300.0, 0.0), blind(-300.0, 0.0)];
        let g0 = share(&ranked, 0, 2);
        let g1 = share(&ranked, 1, 2);
        assert!(g0.len() == 2 && g0.iter().all(|s| s.sees), "{g0:?}");
        assert!(g1.len() == 2 && g1.iter().all(|s| s.sees), "{g1:?}");
        assert_eq!(g1[0].at, ranked[1].at, "its own spot first");
        let mut many: Vec<Spot> = (0..6).map(|i| spot(i as f32 * 300.0, 0.0)).collect();
        many.push(blind(0.0, 900.0));
        many.push(blind(300.0, 900.0));
        for g in 0..3 {
            let s = share(&many, g, 3);
            assert!(s.len() >= SHARE_MIN && s.iter().all(|x| x.sees), "{g} {s:?}");
        }
        // one seeing spot: its seeing peek place before a blind spot; a blind one only when the peek is blind
        let one = vec![spot(300.0, 0.0), blind(0.0, 300.0)];
        let s = share(&one, 1, 2);
        assert_eq!(s.iter().map(|x| (x.at, x.sees)).collect::<Vec<_>>(), vec![(one[0].at, true), (one[0].peek.unwrap(), true)]);
        let blind_peek = vec![Spot { peek_sees: false, ..spot(300.0, 0.0) }, blind(0.0, 300.0)];
        let s = share(&blind_peek, 1, 2);
        assert_eq!(s.len(), 2);
        assert!(s[0].sees && !s[1].sees);
        assert!(share(&[blind(0.0, 300.0), blind(300.0, 0.0)], 0, 1).is_empty());
    }

    /// BOT-179: the Gray Hammer stack: a later guard crowded off its spot with its other spot taken goes to a free
    /// pool spot, else to its peek place that sees the flag; with nothing free it says so (stay), asks for new spots
    /// and tries again after STAY_RETRY_MS, never silently.
    #[test]
    fn a_crowded_guard_never_stays_silently() {
        let a = spot(0.0, 300.0);
        let b = spot(300.0, 0.0);
        let c = spot(-300.0, 0.0);
        let mate = [a.at[0] - 22.0, 0.0, a.at[2]];
        let fresh = |own: Vec<Spot>, pool: Vec<Spot>| {
            let mut h = Hold::default();
            h.set_spots([0.0; 3], own, pool, 0);
            assert_eq!(h.tick(0, Some(a.at), &[], &[], false, (5, None)).unwrap().0.at, a.at);
            h
        };
        let mut h = fresh(vec![a.clone(), b.clone()], vec![a.clone(), b.clone(), c.clone()]);
        let (n, why, _) = h.tick(100, Some(a.at), &[mate], &[mate, b.at], false, (5, None)).unwrap();
        assert_eq!((n.at, why), (c.at, Why::Crowd));
        let mut h = fresh(vec![a.clone(), b.clone()], vec![a.clone(), b.clone(), c.clone()]);
        let (n, why, _) = h.tick(100, Some(a.at), &[mate], &[mate, b.at, c.at], false, (5, None)).unwrap();
        assert_eq!((n.at, why), (a.peek.unwrap(), Why::Crowd));
        assert!(n.sees);
        let blind_peek = Spot {
            peek_sees: false,
            ..a.clone()
        };
        let mut h = fresh(vec![blind_peek.clone(), b.clone()], Vec::new());
        let r = h.tick(100, Some(a.at), &[mate], &[mate, b.at], false, (5, None)).unwrap();
        assert_eq!((r.0.at, r.1), (a.at, Why::Stay));
        assert!(h.rerank);
        assert!(h.tick(200, Some(a.at), &[mate], &[mate, b.at], false, (5, None)).is_none());
        let r = h.tick(100 + STAY_RETRY_MS, Some(a.at), &[mate], &[mate, b.at], false, (5, None));
        assert_eq!(r.map(|x| x.1), Some(Why::Stay));
        h.set_spots([0.0; 3], vec![blind_peek, b.clone(), c.clone()], Vec::new(), 4000);
        assert!(!h.rerank);
        let r = h.tick(4000 + 2 * STAY_RETRY_MS, Some(a.at), &[mate], &[mate, b.at], false, (5, None)).unwrap();
        assert_eq!((r.0.at, r.1), (c.at, Why::Crowd));
    }

    /// BOT-179: a stay ends at STAY_CAP times the dwell even in a long fight; at the cap it moves on even to a
    /// place a later guard stands near; a lone spot with nowhere to go starts its stay over (event stay).
    #[test]
    fn a_stay_is_capped_at_twice_the_dwell() {
        let a = spot(0.0, 300.0);
        let b = spot(300.0, 0.0);
        let mut h = Hold::default();
        h.set_spots([0.0; 3], vec![a.clone(), b.clone()], Vec::new(), 0);
        let (_, _, dwell) = h.tick(0, None, &[], &[], false, (9, None)).unwrap();
        assert!(h.tick(10, Some(a.at), &[], &[], true, (9, None)).is_none());
        let mut t = 10;
        let moved = loop {
            t += 100;
            assert!(t < 10 + 3 * dwell, "never moved");
            if let Some(m) = h.tick(t, Some(a.at), &[], &[], true, (9, None)) {
                break m;
            }
        };
        assert_eq!((moved.0.at, moved.1), (b.at, Why::Cap));
        assert!((10 + STAY_CAP * dwell..10 + STAY_CAP * dwell + 200).contains(&t), "{t} {dwell}");
        let a2 = Spot { peek: None, ..a.clone() };
        let mut h = Hold::default();
        h.set_spots([0.0; 3], vec![a2.clone(), b.clone()], Vec::new(), 0);
        let (_, _, dwell) = h.tick(0, None, &[], &[], false, (9, None)).unwrap();
        h.tick(10, Some(a.at), &[], &[], true, (9, None));
        let later = [b.at[0] + 30.0, 0.0, b.at[2]];
        let r = h.tick(10 + STAY_CAP * dwell, Some(a.at), &[], &[later], true, (9, None)).unwrap();
        assert_eq!((r.0.at, r.1), (b.at, Why::Cap));
        let mut h = Hold::default();
        h.set_spots([0.0; 3], vec![a2], Vec::new(), 0);
        let (_, _, dwell) = h.tick(0, None, &[], &[], false, (9, None)).unwrap();
        h.tick(10, Some(a.at), &[], &[], true, (9, None));
        let r = h.tick(10 + STAY_CAP * dwell, Some(a.at), &[], &[], true, (9, None)).unwrap();
        assert_eq!(r.1, Why::Stay);
        assert!(h.tick(10 + STAY_CAP * dwell + STAY_RETRY_MS + 100, Some(a.at), &[], &[], true, (9, None)).is_none());
    }

    /// BOT-179: when the band's nodes do not see the flag, the nodes nearer it are scored too and a seeing one ranks
    /// first; with enough seeing band nodes they are not tried.
    #[test]
    fn nodes_nearer_the_flag_are_tried_when_the_band_is_blind() {
        let flag = [0.0; 3];
        let near = [0.0, 0.0, 100.0];
        let nodes = [[300.0, 0.0, 0.0], [-300.0, 0.0, 0.0], near];
        let r = rank_guard_spots(flag, &nodes, BAND, &[], |_, _| 1.0, |a, _| a == near);
        assert_eq!(r.first().map(|s| (s.at, s.sees)), Some((near, true)), "{r:?}");
        assert!(r.len() == 3 && r[1..].iter().all(|s| !s.sees));
        let mut ring: Vec<[f32; 3]> = (0..6).map(|k| [300.0 * (k as f32).sin(), 0.0, 300.0 * (k as f32).cos()]).collect();
        ring.push(near);
        let r = rank_guard_spots(flag, &ring, BAND, &[], |_, _| 1.0, |_, _| true);
        assert!(!r.is_empty() && r.iter().all(|s| s.at != near), "{r:?}");
    }
}
