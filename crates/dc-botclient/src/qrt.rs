//! Quarantine for a human bot (mode class 11, team 1): the humans keep to one stand per round (BOT-160), hold and
//! fire there, and fall back only from an infected close by (their claws reach about 120 units; a human shoots from
//! afar). The infected (team 2) rush the humans with the plain hunt or a chase. A DeCombat bot behaviour, not a 2009
//! rule.
// DC-DEFAULT: BOT-128 (proposed)

/// The class of the mode and the two teams (dc-dserver `infect`: every player joins as a human, team 1; an infected
/// player is team 2).
pub const QRT_CLASS: u8 = 11;
pub const HUMANS: u8 = 1;
pub const INFECTED: u8 = 2;

/// BOT-128: within this flat distance of an infected a human with no other human near falls back (BOT-160).
// DC-DEFAULT: BOT-128 (proposed)
pub const NO_HUNT: f32 = 650.0;
/// The places a human falls back to: grid nodes between these flat distances from it, at most this much higher or
/// lower.
pub const RETREAT_MIN: f32 = 160.0;
pub const RETREAT_MAX: f32 = 900.0;
pub const RETREAT_HEIGHT: f32 = 160.0;
/// How long a picked place stands before the bot picks again, and the most places whose sight line is tested.
pub const RETREAT_MS: u32 = 1000;
pub const RETREAT_RAYS: usize = 48;
/// With no grid place the bot walks this far straight away from the infected.
pub const RETREAT_STRAIGHT: f32 = 500.0;
/// With no stand (BOT-160) a human walks back to the other humans' middle until within this flat distance.
pub const REGROUP_NEAR: f32 = 250.0;

/// BOT-143: the Quarantine shell (0xd3) and its sub for the own override row (`[3][row:i8]`, dc-dserver
/// `infect::override_message`; -1 = a human again).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.5 row 0xd3
pub const QRT_SHELL: u8 = 0xd3;
pub const QRT_SUB_OVERRIDE: u8 = 3;
/// BOT-160: sub 0 (a player's team, with the infector when infected; [`NO_INFECTOR`]: the outbreak's host) and sub
/// 1 (a round's countdown).
pub const QRT_SUB_TEAM: u8 = 0;
pub const QRT_SUB_COUNTDOWN: u8 = 1;
pub const NO_INFECTOR: u32 = u32::MAX;
/// BOT-143: a damaging hit on an infected body slows it this long by this factor, as the client's mover does
/// (CLI-1760); a fall (damage kind 0) does not.
// DC-DEFAULT: BOT-143 (proposed) mirrors CLI-1760: 400 ms, x 0.5
pub const SHOT_SLOW_MS: u32 = 400;
pub const SHOT_SLOW_FACTOR: f32 = 0.5;
pub const FALL_DAMAGE_KIND: u8 = 0;
/// BOT-143: the damage type a hazard volume hurts with (dc-dserver `killvol::DAMAGE_TYPE_POISON`, the 0x74 `zone`).
pub const HAZARD_DAMAGE_TYPE: u8 = 9;
/// BOT-143: after a hazard hit the bot walks to a safe grid place for this long.
// DC-DEFAULT: BOT-143 (proposed) 4 s, a safe place within 900 units
pub const HAZARD_LEAVE_MS: u32 = 4000;
pub const HAZARD_LEAVE_RANGE: f32 = 900.0;
/// BOT-143: an infected bot holds the sprint key while it walks to a human within this flat distance; with a spent
/// gauge it lets go until the gauge is back to this fraction.
// DC-DEFAULT: BOT-143 (proposed) 400 units, 0.5
pub const SPRINT_RANGE: f32 = 400.0;
pub const SPRINT_RESUME: f32 = 0.5;
/// BOT-143: the regroup place is the safe grid node nearest the humans' middle within this flat distance (else the
/// living human nearest the middle).
// DC-DEFAULT: BOT-143 (proposed) 400 units
pub const REGROUP_SNAP: f32 = 400.0;

/// BOT-143: whether the infected bot holds the sprint key: it chases (`chasing`), and its gauge (`gauge` = value and
/// full, `None`: no gauge rule) is not resting; a spent or empty gauge rests (`rest`) until it is back to
/// [`SPRINT_RESUME`] of full.
pub fn sprint_key(
    chasing: bool,
    gauge: Option<(f32, i32)>,
    mode: dc_sim::GaugeMode,
    rest: &mut bool,
) -> bool {
    if let Some((v, full)) = gauge {
        if mode == dc_sim::GaugeMode::Spent || v < 1.0 {
            *rest = true;
        } else if *rest && v >= SPRINT_RESUME * full as f32 {
            *rest = false;
        }
    }
    chasing && !*rest
}

/// BOT-143: the regroup place for the humans' middle `mid`: the safe grid place `snap` found for it, else the human
/// of `mates` nearest to it (a place a player stands), else none.
pub fn regroup_place(
    mid: Option<[f32; 3]>,
    mates: &[[f32; 3]],
    snap: impl Fn([f32; 3]) -> Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    let m = mid?;
    snap(m).or_else(|| nearest(m, mates).map(|n| n.0))
}

/// BOT-150: how far ahead (seconds) a Quarantine walk is run on a copy for the danger test.
// DC-DEFAULT: BOT-150 (proposed) 0.5 s, 5 samples, every 250 ms
pub const WALK_GUARD_SECS: f32 = 0.5;

/// BOT-150: the infected the humans fire at first: the one nearest the humans' middle `mid` (the bot's own place
/// among them), so the group's fire meets on one body.
// DC-DEFAULT: BOT-150 (proposed)
pub fn focus(mid: [f32; 3], infected: &[(u8, [f32; 3])]) -> Option<u8> {
    infected
        .iter()
        .min_by(|a, b| {
            flat(a.1, mid)
                .total_cmp(&flat(b.1, mid))
                .then(a.0.cmp(&b.0))
        })
        .map(|i| i.0)
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

/// BOT-160: the stand: of the world's spawn places (alike for every bot of the room) a corner with a long view is
/// best. [`SPOT_RAYS`] level rays of [`SPOT_RAY_LEN`] from eye height (`free`: the share of a ray that runs free,
/// 0..1): a ray that meets a wall within [`SPOT_WALL`] is a wall at its back (one point each: few ways in), a ray
/// free for [`SPOT_VIEW`] is a view to shoot down (half a point each, at most three; a place needs one). It must be
/// safe (`safe`). The best places, at least [`SPOT_APART`] apart, best first.
// DC-DEFAULT: BOT-160 (proposed) 16 rays of 800 units, a wall within 250, a view of 0.75, 600 units apart
pub const SPOT_RAYS: usize = 16;
pub const SPOT_RAY_LEN: f32 = 800.0;
pub const SPOT_WALL: f32 = 250.0;
pub const SPOT_VIEW: f32 = 0.75;
pub const SPOT_APART: f32 = 600.0;
/// BOT-160: the rounds take turns over this many of the best places.
pub const SPOT_TURNS: usize = 3;

pub fn rank_spots(
    points: &[[f32; 3]],
    free: impl Fn([f32; 3], [f32; 3]) -> f32,
    safe: impl Fn([f32; 3]) -> bool,
) -> Vec<[f32; 3]> {
    let mut scored: Vec<(f32, usize, [f32; 3])> = points
        .iter()
        .enumerate()
        .filter(|(_, p)| safe(**p))
        .filter_map(|(i, p)| {
            let (mut walls, mut views) = (0.0f32, 0.0f32);
            for k in 0..SPOT_RAYS {
                let a = k as f32 * std::f32::consts::TAU / SPOT_RAYS as f32;
                let to = [
                    p[0] + a.sin() * SPOT_RAY_LEN,
                    p[1],
                    p[2] + a.cos() * SPOT_RAY_LEN,
                ];
                let f = free(*p, to).clamp(0.0, 1.0);
                if f * SPOT_RAY_LEN < SPOT_WALL {
                    walls += 1.0;
                }
                if f >= SPOT_VIEW {
                    views += 1.0;
                }
            }
            (views > 0.0).then_some((walls + 0.5 * views.min(3.0), i, *p))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out: Vec<[f32; 3]> = Vec::new();
    for (_, _, p) in scored {
        if out.iter().all(|q| flat(*q, p) >= SPOT_APART) {
            out.push(p);
        }
    }
    out
}

/// BOT-160: [`rank_spots`] over a world: rays through its collision from eye height, no place in danger (`kill`:
/// the kill and hazard volumes).
pub fn world_spots(
    coll: &dc_sim::CollisionWorld,
    kill: &[dc_sim::ConvexVolume],
    spawns: &[[f32; 3]],
) -> Vec<[f32; 3]> {
    use dc_sim::{Collider, Cylinder, V3};
    let dims = crate::motion::BOT_STAND_DIMS;
    let eye = 0.5 * dims[1];
    let shape = Cylinder::from_dims(V3::from_array(crate::hunt::RAY_DIMS));
    let body = Cylinder::from_dims(V3::from_array(dims));
    // a spawn place stands above the floor: the place a body comes to rest below it
    let landed: Vec<[f32; 3]> = spawns
        .iter()
        .map(|p| {
            let a = V3::from_array(*p);
            let b = V3::new(p[0], p[1] - 400.0, p[2]);
            coll.sweep(&body, a, b).map_or(*p, |h| {
                let l = a + (b - a) * h.t;
                [l.x, l.y, l.z]
            })
        })
        .collect();
    let free = |from: [f32; 3], to: [f32; 3]| {
        let a = V3::new(from[0], from[1] + eye, from[2]);
        let b = V3::new(to[0], to[1] + eye, to[2]);
        coll.sweep(&shape, a, b).map_or(1.0, |h| h.t)
    };
    rank_spots(&landed, free, |p| !crate::nav::in_danger(coll, kill, dims, p))
}

/// BOT-161: the head node a bot names for a head hit: of the default body row (`[Model0]`, the bots' body, BOT-149)
/// its skeleton row's first node whose `Location` is the head (1), read from the user's ROM at run time.
// OBSERVED: ROM table keys MODELBUTES.TXT [Model<n>] Skeleton, [Skeleton<s>] Node<k>Location (read at run time, no
// value copied)
// DC-DEFAULT: BOT-161 (proposed) body row 0
pub fn head_node_of_rom(rom: &std::path::Path) -> Option<u8> {
    let vfs = dc_rez::RezVfs::new(rom);
    let doc = dc_bute::ButeDoc::try_from_vfs(
        &vfs,
        crate::shooter::ATTRIBUTES_REZ,
        "ATTRIBUTES\\MODELBUTES.TXT",
    )
    .ok()??;
    head_node(&doc)
}

/// BOT-161: [`head_node_of_rom`] on a parsed table.
pub fn head_node(doc: &dc_bute::ButeDoc) -> Option<u8> {
    let num = |v: Option<&String>| -> Option<f32> {
        let t = v?.trim().trim_matches('"').trim();
        t.strip_suffix(['f', 'F']).unwrap_or(t).parse().ok()
    };
    let skel = num(doc.get("Model0")?.get("Skeleton"))? as i64;
    let sec = doc.get(&format!("Skeleton{skel}"))?;
    (0..=255u8).find(|k| num(sec.get(&format!("Node{k}Location"))) == Some(1.0))
}

/// BOT-160: the stand of round `round` (1 = the first) among the ranked places.
pub fn spot_for_round(ranked: &[[f32; 3]], round: u8) -> Option<[f32; 3]> {
    let n = ranked.len().min(SPOT_TURNS);
    (n > 0).then(|| ranked[usize::from(round.max(1) - 1) % n])
}

/// BOT-160: before the outbreak a human waits at a place [`PRE_POST_MIN`]..[`PRE_POST_MAX`] from the stand (spread:
/// the host is one of them); after it, within [`POST_RADIUS`] of it. Reached at [`POST_REACHED`].
// DC-DEFAULT: BOT-160 (proposed) 300..700 units before, 300 units after, 48 units
pub const PRE_POST_MIN: f32 = 300.0;
pub const PRE_POST_MAX: f32 = 700.0;
pub const POST_RADIUS: f32 = 300.0;
/// BOT-160: whether the humans gather within [`POST_RADIUS`] of the stand after the outbreak (else they keep their
/// spread places round it).
pub const GATHER: bool = false;
pub const POST_REACHED: f32 = 48.0;

/// BOT-160: before the outbreak a place this near another human counts against it (the host is one of them: it
/// should find nobody within a sprint), and a human whose place has another human this near picks again.
// DC-DEFAULT: BOT-160 (proposed) 450 units, 250 units, at most every 3 s
pub const PRE_SPREAD: f32 = 450.0;
pub const PRE_CROWD: f32 = 250.0;
pub const PRE_REPICK_MS: u32 = 3000;

/// BOT-160: the own place near the stand `spot` among grid places `nodes` (`min`..`max` flat from it, on about its
/// level): the one whose bearing from the stand is nearest the bot's own bearing (`slot` of 8, from its player id),
/// so the humans spread round it, and (before the outbreak) away from the other humans `mates` (a unit nearer than
/// [`PRE_SPREAD`] costs as much as 1/150 radian off the bearing). `None` without such a place.
pub fn post(
    spot: [f32; 3],
    nodes: &[[f32; 3]],
    min: f32,
    max: f32,
    slot: u8,
    mates: &[[f32; 3]],
) -> Option<[f32; 3]> {
    posts(spot, nodes, min, max, slot, mates).first().copied()
}

/// BOT-160: every place [`post`] could pick, best first.
pub fn posts(
    spot: [f32; 3],
    nodes: &[[f32; 3]],
    min: f32,
    max: f32,
    slot: u8,
    mates: &[[f32; 3]],
) -> Vec<[f32; 3]> {
    let want = f32::from(slot % 8) * std::f32::consts::TAU / 8.0;
    let mid = 0.5 * (min + max);
    let mut v: Vec<(f32, [f32; 3])> = nodes
        .iter()
        .copied()
        .filter(|n| {
            let d = flat(*n, spot);
            (min..=max).contains(&d) && (n[1] - spot[1]).abs() <= RETREAT_HEIGHT
        })
        .map(|n| {
            let a = (n[0] - spot[0]).atan2(n[2] - spot[2]).rem_euclid(std::f32::consts::TAU);
            let mut off = (a - want).abs();
            off = off.min(std::f32::consts::TAU - off);
            let crowd = nearest(n, mates).map_or(0.0, |m| (PRE_SPREAD - m.1).max(0.0));
            (off + crowd / 150.0 + (flat(n, spot) - mid).abs() / 1000.0, n)
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|x| x.1).collect()
}

/// BOT-161: a place is taken only when the bot's grid has a way to it no longer than [`POST_WALK`] (about 15 s of a
/// rifle human's walk) ending within [`POST_REACHED`] of it; at most [`POST_TRIES`] places are tested, else the
/// grid's reachable place nearest the stand. A place not reached within [`POST_GIVE_UP_MS`] is given up for the
/// round and another picked.
// DC-DEFAULT: BOT-161 (proposed) 3300 units, 6 places, 15 s
pub const POST_WALK: f32 = 3300.0;
pub const POST_TRIES: usize = 6;
pub const POST_GIVE_UP_MS: u32 = 15_000;

/// BOT-161: whether a way `path` from `at` (as `NavGrid::path` gives it) reaches `goal` within [`POST_WALK`].
pub fn way_reaches(at: [f32; 3], path: &[[f32; 3]], goal: [f32; 3]) -> bool {
    let Some(end) = path.last() else {
        return false;
    };
    let mut len = 0.0;
    let mut prev = at;
    for p in path {
        len += flat(prev, *p);
        prev = *p;
    }
    flat(*end, goal) <= POST_REACHED && (end[1] - goal[1]).abs() <= RETREAT_HEIGHT && len <= POST_WALK
}

/// BOT-160: an infected inside this flat distance makes a human fall back; a human with no other human within
/// [`ALONE`] falls back from one inside [`NO_HUNT`]; inside [`HOLD_FIRE`] (a body it knows of, or a place one was
/// seen at within [`SEEN_THREAT_MS`]) the human stands and fires; else it takes its place at the stand.
// DC-DEFAULT: BOT-160 (proposed) 350 / 450 / 500 units, 3 s
pub const FALL_BACK: f32 = 350.0;
pub const ALONE: f32 = 450.0;
pub const HOLD_FIRE: f32 = 500.0;
pub const SEEN_THREAT_MS: u32 = 3000;
/// BOT-160: a place an infected was seen at is kept this long as a place to fall back away from.
pub const SEEN_KEEP_MS: u32 = 10_000;

/// BOT-160: what a Quarantine human does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Plan {
    /// Walk to (or stand at) its own place at the stand.
    Post,
    /// Stand and fire.
    Hold,
    /// Fall back (BOT-128's keep-away place, pulled toward the stand).
    FallBack,
}

/// BOT-160: the human's plan from the outbreak state, the nearest infected's distance and whether another human is
/// near. Never a hunt.
pub fn human_plan(outbreak: bool, threat: Option<f32>, alone: bool) -> Plan {
    match threat.filter(|_| outbreak) {
        Some(d) if d <= FALL_BACK || (alone && d <= NO_HUNT) => Plan::FallBack,
        Some(d) if d <= HOLD_FIRE => Plan::Hold,
        _ => Plan::Post,
    }
}

/// BOT-160: an infected's chase gives up a human it gains less than [`CHASE_GAIN`] on in [`CHASE_STALL_MS`] and
/// leaves him alone for [`CHASE_DROP_MS`].
// DC-DEFAULT: BOT-160 (proposed) 64 units / 10 s, 30 s
pub const CHASE_GAIN: f32 = 64.0;
pub const CHASE_STALL_MS: u32 = 10_000;
pub const CHASE_DROP_MS: u32 = 30_000;

/// BOT-160: the Quarantine state of a round as the bot knows it from the server's messages (not from the bodies it
/// sees): the round, who is infected (0xd3 sub 0), the stand, places infected were seen at, the chase.
#[derive(Debug, Default, Clone)]
pub struct Round {
    pub round: u8,
    pub infected: std::collections::BTreeSet<u8>,
    pub spot: Option<[f32; 3]>,
    pub seen: Vec<([f32; 3], u32)>,
    /// The chased human: pid, since when, the least distance so far.
    pub chase: Option<(u8, u32, f32)>,
    pub dropped: Vec<(u8, u32)>,
    /// BOT-161: places given up this round (not reached in time).
    pub bad: Vec<[f32; 3]>,
}

impl Round {
    /// 0xd3 sub 1, the countdown of round `round`: a new round starts with humans only.
    pub fn on_countdown(&mut self, round: u8) {
        if round != self.round {
            self.spot = None;
        }
        self.round = round;
        self.infected.clear();
        self.seen.clear();
        self.chase = None;
        self.dropped.clear();
        self.bad.clear();
    }

    /// 0xd3 sub 0: `pid` is now of `team`.
    pub fn on_team(&mut self, pid: u8, team: u8) {
        if team == INFECTED {
            self.infected.insert(pid);
        } else {
            self.infected.remove(&pid);
        }
    }

    /// Whether the outbreak happened: an infection was announced or a roster row is infected (`roster_infected`).
    pub fn outbreak(&self, roster_infected: bool) -> bool {
        roster_infected || !self.infected.is_empty()
    }

    /// Notes the places infected are seen at now; drops the ones older than [`SEEN_KEEP_MS`].
    pub fn note_seen(&mut self, now: u32, places: &[[f32; 3]]) {
        self.seen
            .retain(|s| now.wrapping_sub(s.1) < SEEN_KEEP_MS && now.wrapping_sub(s.1) < u32::MAX / 2);
        for p in places {
            if let Some(s) = self.seen.iter_mut().find(|s| flat(s.0, *p) < 64.0) {
                *s = (*p, now);
            } else if self.seen.len() < 32 {
                self.seen.push((*p, now));
            }
        }
    }

    /// The places infected were seen at within `ms`.
    pub fn seen_within(&self, now: u32, ms: u32) -> Vec<[f32; 3]> {
        self.seen
            .iter()
            .filter(|s| now.wrapping_sub(s.1) < ms)
            .map(|s| s.0)
            .collect()
    }

    /// The human an infected chases (of `humans`, pid and place): the one it chases while it gains on him, else
    /// the nearest one not given up. Returns the pick and a human given up just now.
    pub fn chase_pick(
        &mut self,
        now: u32,
        at: [f32; 3],
        humans: &[(u8, [f32; 3])],
    ) -> (Option<(u8, [f32; 3])>, Option<u8>) {
        self.dropped.retain(|d| now.wrapping_sub(d.1) < CHASE_DROP_MS);
        let mut gave_up = None;
        if let Some((pid, since, best)) = self.chase {
            if let Some(h) = humans.iter().find(|h| h.0 == pid) {
                let d = flat(h.1, at);
                if d < best - CHASE_GAIN {
                    self.chase = Some((pid, now, d));
                    return (Some(*h), None);
                }
                if now.wrapping_sub(since) < CHASE_STALL_MS {
                    return (Some(*h), None);
                }
                self.dropped.push((pid, now));
                gave_up = Some(pid);
            }
            self.chase = None;
        }
        let pick = humans
            .iter()
            .filter(|h| !self.dropped.iter().any(|d| d.0 == h.0))
            .min_by(|a, b| flat(a.1, at).total_cmp(&flat(b.1, at)).then(a.0.cmp(&b.0)))
            .copied();
        self.chase = pick.map(|h| (h.0, now, flat(h.1, at)));
        (pick, gave_up)
    }
}

/// The nearest of `points` to `at` (flat) and its distance.
pub fn nearest(at: [f32; 3], points: &[[f32; 3]]) -> Option<([f32; 3], f32)> {
    points
        .iter()
        .map(|p| (*p, flat(*p, at)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// The middle of `points`.
pub fn centroid(points: &[[f32; 3]]) -> Option<[f32; 3]> {
    if points.is_empty() {
        return None;
    }
    let n = points.len() as f32;
    let s = points
        .iter()
        .fold([0.0f32; 3], |s, p| [s[0] + p[0], s[1] + p[1], s[2] + p[2]]);
    Some([s[0] / n, s[1] / n, s[2] / n])
}

/// BOT-128: where a human at `at` falls back to from the nearest infected `threat` (the others are `infected`):
/// of the grid places `nodes`, one [`RETREAT_MIN`]..[`RETREAT_MAX`] away on about its level that leaves the threat
/// farther behind than the bot is now and does not lie past it; the best is the farthest from every infected (up
/// to 1200 units), with a sight line to the threat (`sees`, place to threat) worth 300 units and the humans' middle
/// `mates` pulling it in (0.3 per unit). Without such a place: straight away from the threat.
pub fn retreat(
    at: [f32; 3],
    threat: [f32; 3],
    infected: &[[f32; 3]],
    mates: Option<[f32; 3]>,
    nodes: &[[f32; 3]],
    sees: impl Fn([f32; 3], [f32; 3]) -> bool,
) -> [f32; 3] {
    let now_d = flat(at, threat);
    let away = [at[0] - threat[0], at[2] - threat[2]];
    let mut cand: Vec<[f32; 3]> = nodes
        .iter()
        .copied()
        .filter(|n| {
            let d = flat(*n, at);
            (RETREAT_MIN..=RETREAT_MAX).contains(&d)
                && (n[1] - at[1]).abs() <= RETREAT_HEIGHT
                && flat(*n, threat) >= now_d + RETREAT_MIN * 0.5
                && (n[0] - at[0]) * away[0] + (n[2] - at[2]) * away[1] >= 0.0
        })
        .collect();
    if cand.len() > RETREAT_RAYS {
        let step = cand.len() as f32 / RETREAT_RAYS as f32;
        cand = (0..RETREAT_RAYS)
            .map(|i| cand[(i as f32 * step) as usize])
            .collect();
    }
    let score = |n: [f32; 3]| {
        let room = infected
            .iter()
            .map(|p| flat(n, *p))
            .fold(f32::INFINITY, f32::min)
            .min(1200.0);
        let sight = if sees(n, threat) { 300.0 } else { 0.0 };
        let pull = mates.map_or(0.0, |m| 0.3 * flat(n, m));
        room + sight - pull
    };
    let best = cand
        .into_iter()
        .map(|n| (score(n), n))
        .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1[0].total_cmp(&a.1[0])));
    if let Some((_, n)) = best {
        return n;
    }
    let len = away[0].hypot(away[1]).max(1.0);
    [
        at[0] + away[0] / len * RETREAT_STRAIGHT,
        at[1],
        at[2] + away[1] / len * RETREAT_STRAIGHT,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BOT-128: the fall-back place lies away from the infected (never past it), prefers a sight line to it and the
    /// other humans; with no place on the grid the bot walks straight away.
    #[test]
    fn the_fall_back_place_is_away_from_the_infected_with_a_sight_line() {
        let at = [0.0, 0.0, 0.0];
        let threat = [300.0, 0.0, 0.0];
        let nodes = [
            [600.0, 0.0, 0.0],    // past the infected
            [-500.0, 0.0, 0.0],   // away, no sight line
            [-450.0, 0.0, 200.0], // away, in sight
            [-500.0, 400.0, 0.0], // another floor
            [-50.0, 0.0, 0.0],    // too near
        ];
        let sees = |n: [f32; 3], _t: [f32; 3]| n[2] > 100.0;
        assert_eq!(
            retreat(at, threat, &[threat], None, &nodes, sees),
            [-450.0, 0.0, 200.0]
        );
        // the humans' middle pulls toward them
        let nodes = [[-500.0, 0.0, 600.0], [-500.0, 0.0, -600.0]];
        let mates = Some([-300.0, 0.0, -700.0]);
        assert_eq!(
            retreat(at, threat, &[threat], mates, &nodes, |_, _| true),
            [-500.0, 0.0, -600.0]
        );
        // nothing on the grid: straight away
        let g = retreat(at, threat, &[threat], None, &[], |_, _| true);
        assert!(
            (g[0] + RETREAT_STRAIGHT).abs() < 1e-3 && g[2].abs() < 1e-3,
            "{g:?}"
        );
    }

    /// BOT-143: the sprint key holds while chasing until the gauge is spent, then rests until half full.
    #[test]
    fn the_sprint_rests_a_spent_gauge_until_half_full() {
        use dc_sim::GaugeMode::*;
        let mut rest = false;
        assert!(sprint_key(true, Some((80.0, 100)), Sprint, &mut rest));
        assert!(!sprint_key(false, Some((80.0, 100)), Rest, &mut rest));
        assert!(!sprint_key(true, Some((0.0, 100)), Spent, &mut rest) && rest);
        assert!(!sprint_key(true, Some((30.0, 100)), Rest, &mut rest));
        assert!(sprint_key(true, Some((50.0, 100)), Rest, &mut rest) && !rest);
        assert!(sprint_key(true, None, Sprint, &mut rest), "no gauge rule");
    }

    /// BOT-143: the regroup place is the snapped safe place, else the human nearest the middle; an infected with no
    /// hunt goal chases the nearest human.
    #[test]
    fn the_regroup_place_is_a_safe_place_and_the_infected_chase_the_nearest_human() {
        let mates = [[0.0, 0.0, 0.0], [1000.0, 0.0, 0.0], [600.0, 0.0, 0.0]];
        let mid = centroid(&mates);
        assert_eq!(
            regroup_place(mid, &mates, |_| Some([500.0, 10.0, 0.0])),
            Some([500.0, 10.0, 0.0])
        );
        assert_eq!(
            regroup_place(mid, &mates, |_| None),
            Some([600.0, 0.0, 0.0])
        );
        assert_eq!(regroup_place(None, &mates, |_| None), None);
        // (the chase of the nearest human: `a_chase_without_headway_is_given_up_for_the_next_human`)
    }

    /// ROM-gated (skipped without CA_ROM): with the ROM speed rows every infected override row that moves faster
    /// (MovePenalty below -0.01) gives a lower penalty than a human with a rifle, and a hit's slow makes it slower.
    #[test]
    fn rom_the_infected_override_rows_are_faster_than_a_human() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let rows = dc_sim::SpeedTable::from_vfs(&dc_rez::RezVfs::new(&rom)).expect("speed rows");
        let human = crate::motion::loadout_mods(&rows, Some(145), (None, None, None), None, false);
        let n = rows.bodies.len() - rows.override_base;
        let mut faster = 0;
        for slot in 0..n as u32 {
            let o = rows.bodies[rows.override_base + slot as usize];
            if o.penalty >= -0.01 {
                continue;
            }
            let m = crate::motion::loadout_mods(&rows, None, (None, None, None), Some(slot), false);
            eprintln!(
                "override {slot}: penalty {} sprint add {} mul {:?} stamina {:?} (human {} / {:?})",
                m.penalty, m.sprint_add, m.sprint_mul, m.stamina, human.penalty, human.stamina
            );
            assert!(
                m.penalty < human.penalty - 0.1,
                "{slot}: {m:?} vs {human:?}"
            );
            eprintln!("claws 191: {:?}", rows.weapons.get(&191));
            assert!(
                crate::motion::loadout_mods(&rows, None, (None, None, None), Some(slot), true)
                    .penalty
                    > m.penalty
            );
            faster += 1;
        }
        assert!(faster >= 2, "the infected and host rows");
    }

    /// BOT-150: the humans fire first at the infected nearest their middle (ties: the lower pid).
    #[test]
    fn the_humans_focus_the_infected_nearest_their_middle() {
        let mid = [0.0, 0.0, 0.0];
        let inf = [
            (5, [300.0, 0.0, 0.0]),
            (2, [-200.0, 0.0, 0.0]),
            (7, [0.0, 0.0, 200.0]),
        ];
        assert_eq!(focus(mid, &inf), Some(2));
        assert_eq!(focus(mid, &inf[..1]), Some(5));
        assert_eq!(focus(mid, &[]), None);
    }

    /// BOT-160: the stand is a walled-in spawn place with a view; places closer than 600 units to a better one
    /// drop out; the rounds take turns over the best three.
    #[test]
    fn the_stand_is_a_walled_place_with_a_view_and_rounds_take_turns() {
        // a wall 100 units off on every side but +x (free); an open place; a closed box; a place near the first
        let walled = [0.0, 0.0, 0.0];
        let open = [3000.0, 0.0, 0.0];
        let boxed = [0.0, 0.0, 3000.0];
        let near = [300.0, 0.0, 0.0];
        let free = |from: [f32; 3], to: [f32; 3]| {
            if from == open {
                1.0
            } else if from == boxed {
                0.1
            } else if to[0] - from[0] > 700.0 {
                1.0
            } else {
                0.125
            }
        };
        let r = rank_spots(&[open, near, walled, boxed], free, |_| true);
        assert_eq!(r, vec![near, open], "boxed has no view; walled is too near");
        let r = rank_spots(&[open, walled], free, |p| p != walled);
        assert_eq!(r, vec![open], "an unsafe place is none");
        let ranked = [[1.0; 3], [2.0; 3], [3.0; 3], [4.0; 3]];
        assert_eq!(spot_for_round(&ranked, 1), Some([1.0; 3]));
        assert_eq!(spot_for_round(&ranked, 3), Some([3.0; 3]));
        assert_eq!(spot_for_round(&ranked, 4), Some([1.0; 3]));
        assert_eq!(spot_for_round(&ranked[..1], 2), Some([1.0; 3]));
        assert_eq!(spot_for_round(&[], 1), None);
    }

    /// ROM-gated (skipped without CA_ROM): every Quarantine world has stands, safe and with a view.
    #[test]
    fn rom_every_quarantine_world_has_a_stand() {
        let Some(r) = crate::romworld::RomWorlds::from_env() else {
            return;
        };
        for name in ["JUNK_FLEA", "TWOTOWERS", "WARHEAD", "WAREHOUSE_QRT", "OVERDOSE"] {
            let listed = r
                .resolve(format!("WORLDS/{name}.DAT").as_bytes())
                .expect("the world");
            let w = r.load(&listed).expect("loads");
            let mut kill: Vec<_> = w.kill_volumes.iter().map(|k| k.1.clone()).collect();
            kill.extend(w.hazard_volumes.iter().cloned());
            let ranked = world_spots(&w.collision, &kill, &w.spawn_points);
            eprintln!(
                "{name}: {} spawns, stands {:?}",
                w.spawn_points.len(),
                &ranked[..ranked.len().min(SPOT_TURNS)]
            );
            assert!(!ranked.is_empty(), "{name}");
        }
    }

    /// BOT-161: a place counts as reachable only when the way ends at it and is short enough.
    #[test]
    fn a_place_is_reachable_by_a_short_way_that_ends_at_it() {
        let at = [0.0; 3];
        let goal = [1000.0, 0.0, 0.0];
        assert!(way_reaches(at, &[[500.0, 0.0, 0.0], [1000.0, 0.0, 20.0]], goal));
        assert!(!way_reaches(at, &[[500.0, 0.0, 0.0], [800.0, 0.0, 0.0]], goal), "ends short");
        assert!(!way_reaches(at, &[[1000.0, 300.0, 0.0]], goal), "another floor");
        let long = [[0.0, 0.0, 2000.0], [1000.0, 0.0, 2000.0], [1000.0, 0.0, 0.0]];
        assert!(!way_reaches(at, &long, goal), "5000 units of way");
        assert!(!way_reaches(at, &[], goal));
    }

    /// BOT-161: the head node is the default body's skeleton's first head-location node; ROM-gated: it exists.
    #[test]
    fn the_head_node_is_the_first_head_location_of_the_default_skeleton() {
        let doc = dc_bute::ButeDoc::parse(
            "[Model0]\nSkeleton=2\n[Skeleton2]\nNode0Location=2\nNode1Location=3\nNode2Location=1\nNode3Location=1\n",
        );
        assert_eq!(head_node(&doc), Some(2));
        assert_eq!(head_node(&dc_bute::ButeDoc::parse("[Model0]\n")), None);
        if let Some(rom) = std::env::var_os("CA_ROM") {
            let n = head_node_of_rom(std::path::Path::new(&rom));
            eprintln!("head node {n:?}");
            assert!(n.is_some());
        }
    }

    /// BOT-160: each human's place lies at its own bearing round the stand, within the ring.
    #[test]
    fn the_humans_spread_round_the_stand() {
        let spot = [0.0; 3];
        let nodes = [
            [0.0, 0.0, 350.0],   // bearing 0
            [350.0, 0.0, 0.0],   // bearing 90
            [0.0, 0.0, -350.0],  // bearing 180
            [0.0, 0.0, 100.0],   // too near
            [-350.0, 500.0, 0.0], // another floor
        ];
        assert_eq!(post(spot, &nodes, 250.0, 450.0, 0, &[]), Some([0.0, 0.0, 350.0]));
        assert_eq!(post(spot, &nodes, 250.0, 450.0, 2, &[]), Some([350.0, 0.0, 0.0]));
        assert_eq!(post(spot, &nodes, 250.0, 450.0, 4, &[]), Some([0.0, 0.0, -350.0]));
        assert_eq!(post(spot, &nodes, 0.0, 160.0, 4, &[]), Some([0.0, 0.0, 100.0]));
        assert_eq!(post(spot, &nodes[..3], 0.0, 160.0, 4, &[]), None);
        // another human at the own bearing's place: the next place round the stand, away from him
        let mates = [[0.0, 0.0, 330.0]];
        assert_eq!(post(spot, &nodes, 250.0, 450.0, 0, &mates), Some([350.0, 0.0, 0.0]));
    }

    /// BOT-160: before the outbreak a human takes its place whatever it sees; after it, it falls back from a near
    /// infected (or one in reach while alone), stands and fires at a nearer one, else takes its place. Never a hunt.
    #[test]
    fn a_human_holds_and_fires_and_falls_back_only_when_close_or_alone() {
        assert_eq!(human_plan(false, Some(100.0), true), Plan::Post);
        assert_eq!(human_plan(true, Some(150.0), false), Plan::FallBack);
        assert_eq!(human_plan(true, Some(300.0), false), Plan::FallBack);
        assert_eq!(human_plan(true, Some(450.0), false), Plan::Hold);
        assert_eq!(human_plan(true, Some(300.0), true), Plan::FallBack);
        assert_eq!(human_plan(true, Some(700.0), true), Plan::Post);
        assert_eq!(human_plan(true, None, false), Plan::Post);
    }

    /// BOT-160: the outbreak comes from the server's messages, not from bodies in view; a new round's countdown
    /// clears it; places infected were seen at are kept for a while.
    #[test]
    fn the_outbreak_is_known_without_an_infected_in_view() {
        let mut r = Round::default();
        r.on_countdown(1);
        assert!(!r.outbreak(false));
        r.on_team(5, INFECTED);
        assert!(r.outbreak(false), "announced, none in view");
        r.on_team(5, HUMANS);
        assert!(!r.outbreak(false) && r.outbreak(true));
        r.on_team(3, INFECTED);
        r.spot = Some([1.0; 3]);
        r.on_countdown(1);
        assert!(!r.outbreak(false) && r.spot.is_some(), "the countdown sent again keeps the stand");
        r.on_countdown(2);
        assert!(r.spot.is_none());
        r.note_seen(1000, &[[0.0; 3]]);
        r.note_seen(2000, &[[10.0, 0.0, 0.0], [500.0, 0.0, 0.0]]);
        assert_eq!(r.seen.len(), 2, "a place seen again is one place");
        assert_eq!(r.seen_within(4500, SEEN_THREAT_MS).len(), 2);
        assert_eq!(r.seen_within(5500, SEEN_THREAT_MS).len(), 0);
        r.note_seen(13_000, &[]);
        assert!(r.seen.is_empty());
    }

    /// BOT-160: an infected gives up a human it does not gain on for 10 s, takes the next one and leaves the first
    /// alone for 30 s.
    #[test]
    fn a_chase_without_headway_is_given_up_for_the_next_human() {
        let mut r = Round::default();
        let at = [0.0; 3];
        let humans = [(2, [200.0, 0.0, 0.0]), (6, [900.0, 0.0, 0.0])];
        assert_eq!(r.chase_pick(0, at, &humans), (Some(humans[0]), None));
        assert_eq!(r.chase_pick(9_000, at, &humans).0, Some(humans[0]));
        assert_eq!(r.chase_pick(10_500, at, &humans), (Some(humans[1]), Some(2)));
        assert_eq!(r.chase_pick(20_000, at, &humans).0, Some(humans[1]));
        // headway keeps the chase going
        let nearer = [(6, [700.0, 0.0, 0.0])];
        assert_eq!(r.chase_pick(25_000, at, &nearer).0, Some(nearer[0]));
        assert_eq!(r.chase_pick(34_000, at, &nearer).0, Some(nearer[0]));
        // the first is back after 30 s
        let both = [(2, [200.0, 0.0, 0.0]), (6, [700.0, 0.0, 0.0])];
        assert_eq!(r.chase_pick(41_000, at, &both), (Some(both[0]), Some(6)));
        assert_eq!(r.chase_pick(41_100, at, &[]), (None, None));
    }
}
