//! Team mates spread out in the hunting modes (BOT-148), sans-IO: a team's bots hunt like four people, not one blob.
//! Every bot works out the same split from the places it sees (its team mates' streamed places and player ids), so
//! no talk between the bots is needed: (1) the enemies a bot may go for are shared out in player id order, each
//! member taking the nearest one not yet taken; (2) a hunt or roam goal a team mate stands near is moved to a grid
//! node beside it; (3) a bot fighting from where a team mate stands steps aside; (4) a walking bot right behind a
//! team mate waits a moment. A DeCombat bot behaviour, no 2009
//! rule. The bomb, flag, Spy Hunt and Quarantine modes keep their own jobs; one-man-army has no team mates.
// DC-DEFAULT: BOT-148 (proposed)

use dc_net_engine::time::{later, since};
use dc_net_engine::Millis;

/// A hunt or roam goal with a team mate within this (flat) of it is moved aside.
pub const ASIDE: f32 = 160.0;
/// How far to the side of the goal (across the bot's line to it) the moved goal lies, before snapping to the grid.
pub const ASIDE_STEP: f32 = 240.0;
/// The moved goal is the grid node nearest that side point within this (flat).
pub const ASIDE_SNAP: f32 = 160.0;
/// A team mate counts as being at a place only within this height of it (another floor does not crowd it).
pub const ASIDE_RISE: f32 = 128.0;
/// A bot fighting with a team mate within this (flat, and [`STACK_RISE`] in height) steps aside.
pub const STACK_NEAR: f32 = 96.0;
pub const STACK_RISE: f32 = 96.0;
/// The side step lasts until the nearest team mate is this far (flat), or [`STEP_MAX_MS`] passed.
pub const STEP_CLEAR: f32 = 160.0;
pub const STEP_MAX_MS: u32 = 2500;
/// After a side step the bot does not start another for this long (the plain evasive strafe runs).
pub const STEP_REST_MS: u32 = 2000;
/// The shared-out enemy is taken only while it is at most this much farther than the nearest one.
pub const SPREAD_EXTRA: f32 = 400.0;

/// Whether the team rules apply in this mode class: team deathmatch (1) and the elimination rounds (12 / 13).
pub fn spreads(mode_class: Option<u8>) -> bool {
    matches!(mode_class, Some(1 | 12 | 13))
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Rule 1: the candidate (an enemy's place, or a place to patrol to) the bot `own_pid` at `own` goes for. The members
/// (the bot and its `mates`, player id and place) in player id order each take the candidate nearest to them that no
/// earlier member took (when all are taken, the share starts over); the bot's own share is its pick unless it lies
/// more than [`SPREAD_EXTRA`] farther than its nearest candidate (then the nearest). Without mates: the nearest (the
/// first of equals). `None` without candidates.
pub fn pick(
    own_pid: u8,
    own: [f32; 3],
    mates: &[(u8, [f32; 3])],
    cands: &[[f32; 3]],
) -> Option<usize> {
    let nearest_free = |at: [f32; 3], taken: &[bool]| {
        (0..cands.len()).filter(|i| !taken[*i]).min_by(|a, b| {
            dist(at, cands[*a])
                .total_cmp(&dist(at, cands[*b]))
                .then(a.cmp(b))
        })
    };
    let none = vec![false; cands.len()];
    let near = nearest_free(own, &none)?;
    let mut members: Vec<(u8, [f32; 3])> =
        mates.iter().copied().filter(|m| m.0 != own_pid).collect();
    members.push((own_pid, own));
    members.sort_by_key(|m| m.0);
    let mut taken = none;
    for (pid, at) in members {
        if taken.iter().all(|t| *t) {
            taken.fill(false);
        }
        let k = nearest_free(at, &taken)?;
        if pid == own_pid {
            return Some(
                if dist(own, cands[k]) <= dist(own, cands[near]) + SPREAD_EXTRA {
                    k
                } else {
                    near
                },
            );
        }
        taken[k] = true;
    }
    Some(near)
}

/// Whether a team mate stands at `goal` (within [`ASIDE`] flat and [`ASIDE_RISE`] in height).
pub fn crowded(goal: [f32; 3], mates: &[(u8, [f32; 3])]) -> bool {
    mates
        .iter()
        .any(|m| flat(m.1, goal) < ASIDE && (m.1[1] - goal[1]).abs() <= ASIDE_RISE)
}

/// Rule 2: where a bot at `own` walks instead of `goal` when a team mate stands at the goal: of the two points
/// [`ASIDE_STEP`] to either side of the goal (across the bot's line to it), snapped to the grid by `snap`, the one
/// farthest from the nearest team mate, if it is at least [`ASIDE`] from every team mate and [`ASIDE`] / 2 from the
/// goal. `None` when the goal is not crowded, lies within [`ASIDE`] of the bot (the side step takes over there) or no
/// side point qualifies (the goal stays).
pub fn push_aside(
    own: [f32; 3],
    goal: [f32; 3],
    mates: &[(u8, [f32; 3])],
    snap: impl Fn([f32; 3]) -> Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    if !crowded(goal, mates) || flat(own, goal) <= ASIDE {
        return None;
    }
    let (dx, dz) = (goal[0] - own[0], goal[2] - own[2]);
    let l = dx.hypot(dz);
    let (sx, sz) = (-dz / l * ASIDE_STEP, dx / l * ASIDE_STEP);
    let clear = |p: [f32; 3]| mates.iter().map(|m| flat(m.1, p)).fold(f32::MAX, f32::min);
    [
        [goal[0] + sx, goal[1], goal[2] + sz],
        [goal[0] - sx, goal[1], goal[2] - sz],
    ]
    .into_iter()
    .filter_map(snap)
    .filter(|p| clear(*p) >= ASIDE && flat(*p, goal) >= ASIDE / 2.0)
    .max_by(|a, b| clear(*a).total_cmp(&clear(*b)))
}

/// Rule 3: the side (-1 left, 1 right of the look `yaw`) a fighting bot at `own` steps to, away from the nearest team
/// mate within [`STACK_NEAR`] (flat, [`STACK_RISE`] in height); a mate straight ahead or behind: the higher player id
/// steps right, the lower left. `None` with no mate that near.
pub fn step_side(own_pid: u8, own: [f32; 3], yaw: f32, mates: &[(u8, [f32; 3])]) -> Option<i8> {
    let m = mates
        .iter()
        .filter(|m| m.0 != own_pid && (m.1[1] - own[1]).abs() <= STACK_RISE)
        .map(|m| (flat(m.1, own), m))
        .filter(|x| x.0 < STACK_NEAR)
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1 .0.cmp(&b.1 .0)))?
        .1;
    // the look's right in the walk's yaw convention (forward x = sin, z = cos): x = cos, z = -sin
    let side = (m.1[0] - own[0]) * yaw.cos() - (m.1[2] - own[2]) * yaw.sin();
    Some(if side > 1.0 {
        -1
    } else if side < -1.0 || own_pid > m.0 {
        1
    } else {
        -1
    })
}

/// Rule 3's clock: a side step runs until the nearest team mate is [`STEP_CLEAR`] away or [`STEP_MAX_MS`] passed,
/// then rests [`STEP_REST_MS`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Step {
    /// The side and when the step began.
    pub on: Option<(i8, Millis)>,
    /// No new step before this time.
    pub rest_until: Option<Millis>,
}

impl Step {
    /// The side to strafe to this tick (and whether the step began now), for a fighting bot at `own` looking along
    /// `yaw`.
    pub fn tick(
        &mut self,
        now: Millis,
        own_pid: u8,
        own: [f32; 3],
        yaw: f32,
        mates: &[(u8, [f32; 3])],
    ) -> Option<(i8, bool)> {
        if let Some((side, at)) = self.on {
            let near = mates
                .iter()
                .filter(|m| m.0 != own_pid && (m.1[1] - own[1]).abs() <= STACK_RISE)
                .any(|m| flat(m.1, own) < STEP_CLEAR);
            if near && since(now, at) < STEP_MAX_MS {
                return Some((side, false));
            }
            self.on = None;
            self.rest_until = Some(now.wrapping_add(STEP_REST_MS));
            return None;
        }
        if self.rest_until.is_some_and(|r| later(r, now)) {
            return None;
        }
        self.rest_until = None;
        let side = step_side(own_pid, own, yaw, mates)?;
        self.on = Some((side, now));
        Some((side, true))
    }

    /// The fight ended (no target, a walk goal): the step stops and its rest runs.
    pub fn idle(&mut self, now: Millis) {
        if self.on.take().is_some() {
            self.rest_until = Some(now.wrapping_add(STEP_REST_MS));
        }
    }
}

/// A side step that has not cleared the team mate after this long (a wall, or both sides unsafe) becomes a short walk
/// over the grid to [`space_place`].
pub const SPACE_AFTER_MS: u32 = 600;
/// That walk's place lies this far from the bot (before snapping to the grid within [`SPACE_SNAP`]); it ends there
/// (within [`SPACE_REACHED`]) or after [`SPACE_MS`].
pub const SPACE_STEP: f32 = 200.0;
pub const SPACE_SNAP: f32 = 64.0;
pub const SPACE_REACHED: f32 = 48.0;
pub const SPACE_MS: u32 = 2000;

/// Rule 3's fallback: of the eight points [`SPACE_STEP`] around `own`, snapped to the grid by `snap`, the one farthest
/// from the nearest team mate, if that is at least [`STEP_CLEAR`] and the snapped place is within
/// [`SPACE_STEP`] + [`SPACE_SNAP`] of the bot; ties: the first in turn from the look's right (`yaw`), clockwise.
pub fn space_place(
    own: [f32; 3],
    yaw: f32,
    mates: &[(u8, [f32; 3])],
    snap: impl Fn([f32; 3]) -> Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    let clear = |p: [f32; 3]| mates.iter().map(|m| flat(m.1, p)).fold(f32::MAX, f32::min);
    let mut best: Option<(f32, [f32; 3])> = None;
    for k in 0..8 {
        let a = yaw + std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::FRAC_PI_4;
        let c = [
            own[0] + a.sin() * SPACE_STEP,
            own[1],
            own[2] + a.cos() * SPACE_STEP,
        ];
        let Some(p) = snap(c) else { continue };
        let cl = clear(p);
        if cl >= STEP_CLEAR
            && flat(p, own) <= SPACE_STEP + SPACE_SNAP
            && best.is_none_or(|b| cl > b.0)
        {
            best = Some((cl, p));
        }
    }
    best.map(|b| b.1)
}

/// A walking bot with a team mate this near (flat, [`STACK_RISE`] in height) right ahead on its line to its goal
/// waits.
pub const TRAIL_NEAR: f32 = 72.0;
/// The wait ends once no team mate ahead is within this, or after [`TRAIL_WAIT_MS`] (shorter than the session's
/// watch gap, so the way is kept); then the bot does not wait again for [`TRAIL_OFF_MS`].
pub const TRAIL_RESUME: f32 = 128.0;
/// A team mate ahead within this (it stopped on the bot's way) turns the wait into a short walk to a clear place.
pub const TRAIL_CLOSE: f32 = 40.0;
pub const TRAIL_WAIT_MS: u32 = 1200;
pub const TRAIL_OFF_MS: u32 = 1000;
/// "Ahead": within 60 degrees of the line to the goal.
const AHEAD_COS: f32 = 0.5;

/// Whether a team mate stands within `within` of `own`, ahead of it on its line to `goal`.
pub fn mate_ahead(own: [f32; 3], goal: [f32; 3], mates: &[(u8, [f32; 3])], within: f32) -> bool {
    let (gx, gz) = (goal[0] - own[0], goal[2] - own[2]);
    let gl = gx.hypot(gz);
    if gl < 1.0 {
        return false;
    }
    mates.iter().any(|m| {
        let (mx, mz) = (m.1[0] - own[0], m.1[2] - own[2]);
        let ml = mx.hypot(mz);
        ml < within
            && (m.1[1] - own[1]).abs() <= STACK_RISE
            && (ml < 1.0 || (mx * gx + mz * gz) / (ml * gl) >= AHEAD_COS)
    })
}

/// Rule 4's clock: a walking bot right behind a team mate waits a moment so the two do not walk as one body.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Trail {
    /// When the wait began.
    pub since: Option<Millis>,
    /// No new wait before this time.
    pub off_until: Option<Millis>,
}

impl Trail {
    /// Whether the bot at `own` walking to `goal` waits this tick.
    pub fn tick(
        &mut self,
        now: Millis,
        own: [f32; 3],
        goal: [f32; 3],
        mates: &[(u8, [f32; 3])],
    ) -> bool {
        if let Some(at) = self.since {
            if since(now, at) < TRAIL_WAIT_MS && mate_ahead(own, goal, mates, TRAIL_RESUME) {
                return true;
            }
            self.since = None;
            self.off_until = Some(now.wrapping_add(TRAIL_OFF_MS));
            return false;
        }
        if self.off_until.is_some_and(|r| later(r, now)) {
            return false;
        }
        self.off_until = None;
        if mate_ahead(own, goal, mates, TRAIL_NEAR) {
            self.since = Some(now);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enemies_are_shared_out_in_player_id_order() {
        let e = [[1100.0, 0.0, 0.0], [1300.0, 0.0, 100.0]];
        let at = [0.0, 0.0, 0.0];
        // four mates on one spot: 1 and 3 take the nearer enemy, 2 and 4 the other
        let mates = [(1, at), (2, at), (3, at), (4, at)];
        assert_eq!(pick(1, at, &mates, &e), Some(0));
        assert_eq!(pick(2, at, &mates, &e), Some(1));
        assert_eq!(pick(3, at, &mates, &e), Some(0));
        assert_eq!(pick(4, at, &mates, &e), Some(1));
        // without mates: the nearest
        assert_eq!(pick(2, at, &[], &e), Some(0));
        // a share much farther than the nearest is not taken
        let far = [[1100.0, 0.0, 0.0], [1600.0, 0.0, 0.0]];
        assert_eq!(pick(2, at, &mates, &far), Some(0));
        // a mate nearer an enemy takes it; the bot takes the other one, though nearer the first
        let mates = [(1, [1000.0, 0.0, 0.0])];
        assert_eq!(pick(2, at, &mates, &e), Some(1));
        assert_eq!(pick(2, at, &mates, &[]), None);
    }

    #[test]
    fn a_goal_a_mate_stands_at_is_moved_aside() {
        let own = [0.0, 0.0, 0.0];
        let goal = [0.0, 0.0, 1000.0];
        let snap = |p: [f32; 3]| Some(p);
        // nobody at the goal: no change
        assert_eq!(push_aside(own, goal, &[(2, [0.0, 0.0, 400.0])], snap), None);
        // a mate at the goal, a bit to the right (+x): the goal moves to the left (-x), across the line
        let mates = [(2, [60.0, 0.0, 1000.0])];
        let g = push_aside(own, goal, &mates, snap).expect("moved");
        assert!(
            (g[0] + ASIDE_STEP).abs() < 1.0 && (g[2] - 1000.0).abs() < 1.0,
            "{g:?}"
        );
        // a mate on another floor does not crowd the goal
        assert_eq!(
            push_aside(own, goal, &[(2, [0.0, 300.0, 1000.0])], snap),
            None
        );
        // the bot itself is at the goal: the side step's job
        assert_eq!(push_aside([0.0, 0.0, 900.0], goal, &mates, snap), None);
        // no grid node on either side: the goal stays
        assert_eq!(push_aside(own, goal, &mates, |_| None), None);
    }

    #[test]
    fn a_blocked_side_step_walks_to_a_clear_grid_place() {
        let own = [0.0, 0.0, 0.0];
        let mates = [(2, [90.0, 0.0, 0.0])];
        // open floor: the place farthest from the mate (at -x, away from it)
        let p = space_place(own, 0.0, &mates, Some).expect("a place");
        assert!(p[0] < -150.0, "{p:?}");
        // only the side toward the mate has floor: not clear enough, no place
        let only_east = |c: [f32; 3]| (c[0] > 150.0).then_some(c);
        assert_eq!(space_place(own, 0.0, &mates, only_east), None);
        // no grid: none
        assert_eq!(space_place(own, 0.0, &mates, |_| None), None);
    }

    #[test]
    fn a_walker_waits_a_moment_behind_the_mate_ahead() {
        let own = [0.0, 0.0, 0.0];
        let goal = [0.0, 0.0, 1000.0];
        // ahead on the line, beside it, behind, on another floor
        assert!(mate_ahead(own, goal, &[(2, [10.0, 0.0, 50.0])], TRAIL_NEAR));
        assert!(!mate_ahead(
            own,
            goal,
            &[(2, [60.0, 0.0, 10.0])],
            TRAIL_NEAR
        ));
        assert!(!mate_ahead(
            own,
            goal,
            &[(2, [0.0, 0.0, -50.0])],
            TRAIL_NEAR
        ));
        assert!(!mate_ahead(
            own,
            goal,
            &[(2, [0.0, 150.0, 50.0])],
            TRAIL_NEAR
        ));
        let mut t = Trail::default();
        let mates = [(2, [0.0, 0.0, 50.0])];
        assert!(t.tick(1000, own, goal, &mates));
        // until the mate is TRAIL_RESUME ahead
        assert!(t.tick(1500, own, goal, &[(2, [0.0, 0.0, 100.0])]));
        assert!(!t.tick(1600, own, goal, &[(2, [0.0, 0.0, 140.0])]));
        // then no wait for TRAIL_OFF_MS
        assert!(!t.tick(2000, own, goal, &mates));
        assert!(t.tick(1600 + TRAIL_OFF_MS, own, goal, &mates));
        // and at most TRAIL_WAIT_MS
        assert!(!t.tick(1600 + TRAIL_OFF_MS + TRAIL_WAIT_MS, own, goal, &mates));
    }

    #[test]
    fn a_fighter_steps_away_from_the_mate_beside_it() {
        // looking along +z (yaw 0): the right is +x
        let own = [0.0, 0.0, 0.0];
        assert_eq!(step_side(1, own, 0.0, &[(2, [40.0, 0.0, 0.0])]), Some(-1));
        assert_eq!(step_side(1, own, 0.0, &[(2, [-40.0, 0.0, 0.0])]), Some(1));
        // straight ahead: the higher id right, the lower left
        assert_eq!(step_side(3, own, 0.0, &[(2, [0.0, 0.0, 40.0])]), Some(1));
        assert_eq!(step_side(2, own, 0.0, &[(3, [0.0, 0.0, 40.0])]), Some(-1));
        // too far, or another floor: no step
        assert_eq!(step_side(1, own, 0.0, &[(2, [120.0, 0.0, 0.0])]), None);
        assert_eq!(step_side(1, own, 0.0, &[(2, [10.0, 150.0, 0.0])]), None);
        // the clock: on until the mate is STEP_CLEAR away, then a rest
        let mut s = Step::default();
        let mates = [(2, [40.0, 0.0, 0.0])];
        assert_eq!(s.tick(1000, 1, own, 0.0, &mates), Some((-1, true)));
        assert_eq!(
            s.tick(1500, 1, [-100.0, 0.0, 0.0], 0.0, &mates),
            Some((-1, false))
        );
        assert_eq!(s.tick(2000, 1, [-130.0, 0.0, 0.0], 0.0, &mates), None);
        assert_eq!(s.tick(2500, 1, own, 0.0, &mates), None);
        assert_eq!(s.tick(4100, 1, own, 0.0, &mates), Some((-1, true)));
        // and at most STEP_MAX_MS
        assert_eq!(s.tick(4100 + STEP_MAX_MS, 1, own, 0.0, &mates), None);
    }
}
