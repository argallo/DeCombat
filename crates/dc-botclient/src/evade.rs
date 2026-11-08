//! Evasive movement while the bot engages a target it can see (BOT-92), sans-IO: strafing that reverses at
//! random, a step back when the enemy is close, jumps now and then and right after being hit, a crouch at range.
//! A DeCombat bot behaviour (test and filler traffic), none of it a 2009 rule; every number scales with the
//! skill like the aim does (`crate::skill`).
// DC-DEFAULT: BOT-92 (proposed)

use crate::skill::AimRng;

type Millis = u32;

fn since(now: Millis, then: Millis) -> u32 {
    now.wrapping_sub(then)
}

fn lerp(at0: f32, at100: f32, level: u8) -> f32 {
    at0 + (at100 - at0) * level.min(100) as f32 / 100.0
}

/// The evasive move of one tick, relative to `yaw` (the bot looks at its target along it).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Move {
    /// The look yaw the strafe is relative to (radians).
    pub yaw: f32,
    /// -1 left, 0 none, 1 right.
    pub strafe: i8,
    /// A step backwards (the enemy is close).
    pub back: bool,
    /// Jump this tick.
    pub jump: bool,
    /// Crouch (the enemy is far and nothing hit the bot lately).
    pub crouch: bool,
    /// What began this tick, for the event log (`"jump"`, `"hit_reverse"`, `"reverse"`), else `None`.
    pub note: Option<&'static str>,
}

/// Reversal period of the strafe, ms (uniform in the range).
// DC-DEFAULT: BOT-92 (proposed) 500..1400 ms at 0, 350..900 ms at 100
pub const REVERSE_MIN_MS: (f32, f32) = (500.0, 350.0);
pub const REVERSE_MAX_MS: (f32, f32) = (1400.0, 900.0);
/// A hit within this long starts a reversal and a jump at once, ms.
// DC-DEFAULT: BOT-92 (proposed) 250 ms
pub const HIT_REACT_MS: u32 = 250;
/// Jumps per second while engaging (a Poisson rate), never two within JUMP_GAP_MS.
// DC-DEFAULT: BOT-92 (proposed) 0.12 /s at 0 .. 0.45 /s at 100; gap 900 ms
pub const JUMP_RATE: (f32, f32) = (0.12, 0.45);
pub const JUMP_GAP_MS: u32 = 900;
/// Closer than this the bot steps back on BACK_CHANCE of its reversals, for BACK_MS.
// DC-DEFAULT: BOT-92 (proposed) 220 units, 30 %, 400 ms
pub const BACK_DIST: f32 = 220.0;
pub const BACK_CHANCE: f32 = 0.30;
pub const BACK_MS: u32 = 400;
/// Farther than this, with no hit for CROUCH_CALM_MS, the bot crouches while it fires.
// DC-DEFAULT: BOT-92 (proposed) 900 units, 2500 ms
pub const CROUCH_DIST: f32 = 900.0;
pub const CROUCH_CALM_MS: u32 = 2500;

/// The evasion state of one bot.
#[derive(Debug, Clone)]
pub struct Evader {
    rng: AimRng,
    strafe: i8,
    next_reverse: Option<Millis>,
    back_until: Option<Millis>,
    last_jump: Option<Millis>,
    hit_at: Option<Millis>,
    reacted_hit: Option<Millis>,
    last_tick: Option<Millis>,
    pub reversals: u32,
    pub jumps: u32,
}

impl Evader {
    pub fn new(seed: u64) -> Self {
        Evader {
            rng: AimRng::new(seed ^ 0x00e7_ade0),
            strafe: 1,
            next_reverse: None,
            back_until: None,
            last_jump: None,
            hit_at: None,
            reacted_hit: None,
            last_tick: None,
            reversals: 0,
            jumps: 0,
        }
    }

    /// The bot took damage now.
    pub fn on_damage(&mut self, now: Millis) {
        self.hit_at = Some(now);
    }

    /// Not engaging (no target in sight): the state rests; the next engagement starts fresh.
    pub fn idle(&mut self) {
        self.next_reverse = None;
        self.back_until = None;
        self.last_tick = None;
    }

    fn period(&mut self, level: u8) -> u32 {
        let lo = lerp(REVERSE_MIN_MS.0, REVERSE_MIN_MS.1, level);
        let hi = lerp(REVERSE_MAX_MS.0, REVERSE_MAX_MS.1, level);
        (lo + (hi - lo) * self.rng.unit()) as u32
    }

    /// The move of this tick while engaging a target `dist` units away, looking along `yaw`.
    pub fn tick(&mut self, now: Millis, yaw: f32, dist: f32, level: u8) -> Move {
        let dt = self.last_tick.map_or(0, |t| since(now, t)).min(250);
        self.last_tick = Some(now);
        let mut note = None;
        let mut jump = false;
        // a fresh hit: reverse at once and jump (once per hit)
        let fresh_hit = self
            .hit_at
            .is_some_and(|h| since(now, h) <= HIT_REACT_MS && self.reacted_hit != Some(h));
        if fresh_hit {
            self.reacted_hit = self.hit_at;
            self.strafe = -self.strafe;
            self.reversals += 1;
            self.next_reverse = Some(now.wrapping_add(self.period(level)));
            if self.last_jump.is_none_or(|j| since(now, j) >= JUMP_GAP_MS) {
                jump = true;
                self.last_jump = Some(now);
                self.jumps += 1;
            }
            note = Some("hit_reverse");
        } else {
            match self.next_reverse {
                Some(t) if since(now, t) < 0x8000_0000 => {
                    // due: the other way; close in, sometimes a step back instead
                    self.strafe = -self.strafe;
                    self.reversals += 1;
                    self.next_reverse = Some(now.wrapping_add(self.period(level)));
                    if dist < BACK_DIST && self.rng.unit() < BACK_CHANCE {
                        self.back_until = Some(now.wrapping_add(BACK_MS));
                    }
                    note = Some("reverse");
                }
                Some(_) => {}
                None => {
                    self.strafe = if self.rng.unit() < 0.5 { -1 } else { 1 };
                    self.next_reverse = Some(now.wrapping_add(self.period(level)));
                }
            }
            // a jump now and then, in a fight (the enemy within a sprint), never two close together
            let rate = lerp(JUMP_RATE.0, JUMP_RATE.1, level);
            let p = rate * dt as f32 / 1000.0;
            if dist < 1200.0
                && self.last_jump.is_none_or(|j| since(now, j) >= JUMP_GAP_MS)
                && self.rng.unit() < p
            {
                jump = true;
                self.last_jump = Some(now);
                self.jumps += 1;
                note = Some("jump");
            }
        }
        let back = self
            .back_until
            .is_some_and(|u| since(u, now) < 0x8000_0000 && u != now);
        if self.back_until.is_some_and(|u| since(now, u) < 0x8000_0000) {
            self.back_until = None;
        }
        let calm = self.hit_at.is_none_or(|h| since(now, h) >= CROUCH_CALM_MS);
        Move {
            yaw,
            strafe: self.strafe,
            back,
            jump,
            crouch: dist > CROUCH_DIST && calm,
            note,
        }
    }
}

/// BOT-134: how far ahead an evasive move is run on a copy (seconds), and how often along it the danger test looks;
/// a jump is run for its whole flight and the landing (at 330 up and gravity 1100 a jump lasts 0.6 s).
// DC-DEFAULT: BOT-134 (proposed) 0.8 s, 8 samples; jump 1.0 s, 10 samples
pub const PROBE_SECS: f32 = 0.8;
pub const PROBE_SAMPLES: usize = 8;
pub const JUMP_PROBE_SECS: f32 = 1.0;
pub const JUMP_PROBE_SAMPLES: usize = 10;
/// BOT-134: a body whose box, moved this many body widths, touches a kill volume stands at its edge and steps away.
// DC-DEFAULT: BOT-134 (proposed) one body width (twice the half width)
pub const EDGE_WIDTHS: f32 = 1.0;

/// BOT-116 / BOT-134: which evasive moves keep the body out of danger (each run on a copy, every place along it
/// tested): the strafe to the left, to the right, the step back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Safety {
    pub left: bool,
    pub right: bool,
    pub back: bool,
}

impl Default for Safety {
    fn default() -> Self {
        Safety {
            left: true,
            right: true,
            back: true,
        }
    }
}

/// BOT-134: whether every place of `path` is out of danger ([`crate::nav::in_danger`]: in or above a kill volume,
/// above the void, above a drop of more than 300).
pub fn path_safe(
    world: &dc_sim::CollisionWorld,
    kill: &[dc_sim::ConvexVolume],
    dims: [f32; 3],
    path: &[[f32; 3]],
) -> bool {
    path.iter()
        .all(|p| !crate::nav::in_danger(world, kill, dims, *p))
}

fn evade_move(yaw: f32, strafe: i8, back: bool, jump: bool) -> Move {
    Move {
        yaw,
        strafe,
        back,
        jump,
        crouch: false,
        note: None,
    }
}

/// BOT-116 / BOT-134: which evasive moves keep the body out of danger: the strafe to each side of `yaw` and the step
/// back, each run on a copy for `secs` with the movement code (the real strafe and back commands, the body's
/// momentum) and every place along it tested. Before (BOT-116) the end of a 0.4 s run straight to each side was
/// tested, which a strafe of up to 1.4 s at run speed, a reversal against the momentum and a way that crossed the
/// danger and ended on a floor all got past.
// DC-DEFAULT: BOT-116 (proposed), BOT-134 (proposed)
pub fn evade_safety(
    body: &crate::motion::Body,
    world: &dc_sim::CollisionWorld,
    kill: &[dc_sim::ConvexVolume],
    yaw: f32,
    secs: f32,
) -> Safety {
    let ok = |strafe: i8, back: bool| {
        let path = body.probe_path(
            None,
            Some(evade_move(yaw, strafe, back, false)),
            secs,
            PROBE_SAMPLES,
        );
        if !path_safe(world, kill, body.dims(), &path) {
            return false;
        }
        // BOT-134: nor a move that brings the body within a body width of the danger (the place one body width on
        // beyond the end of the move, along it): a strafe back to the edge of a ledge or a kill volume after a step
        // away from it made the bot dance there
        let (from, end) = (body.pos(), path[path.len() - 1]);
        let (dx, dz) = (end[0] - from[0], end[2] - from[2]);
        let len = dx.hypot(dz);
        if len < 1.0 {
            return true;
        }
        let w = 2.0 * body.dims()[0] * EDGE_WIDTHS / len;
        let beyond = [end[0] + dx * w, end[1], end[2] + dz * w];
        !crate::nav::in_danger(world, kill, body.dims(), beyond)
    };
    Safety {
        left: ok(-1, false),
        right: ok(1, false),
        back: ok(0, true),
    }
}

/// BOT-134: whether a jump now keeps the body out of danger: the body walking along `steer` (or doing the evasive
/// move `evade`) jumps on a copy and its whole flight and landing are tested. The evasive jumps, the stuck jumps and
/// the watchdog's jump all ask.
// DC-DEFAULT: BOT-134 (proposed)
pub fn jump_safe(
    body: &crate::motion::Body,
    world: &dc_sim::CollisionWorld,
    kill: &[dc_sim::ConvexVolume],
    steer: Option<f32>,
    evade: Option<Move>,
) -> bool {
    let mut m = evade.unwrap_or_else(|| evade_move(body.angles[1], 0, false, false));
    m.jump = true;
    let path = body.probe_path(steer, Some(m), JUMP_PROBE_SECS, JUMP_PROBE_SAMPLES);
    path_safe(world, kill, body.dims(), &path)
}

/// BOT-159: the way off a small island of the grid (a pit, a deck, a trench) is looked for at this many headings, each
/// walked and jumped on a copy for this long (sampled this many times); its end must lie this far (flat) from every
/// node of the island, or this far in height.
// DC-DEFAULT: BOT-159 (proposed) 16 headings, 2.0 s, 20 samples, 48 / 60 units
pub const ESCAPE_HEADINGS: usize = 16;
pub const ESCAPE_SECS: f32 = 2.0;
pub const ESCAPE_SAMPLES: usize = 20;
pub const ESCAPE_CLEAR: f32 = 48.0;
pub const ESCAPE_CLEAR_Y: f32 = 60.0;
/// BOT-159: a run that jumps where it is blocked jumps on the ground after this long with a flat speed under this
/// (units/s): a bar or a ledge face it walked into.
// DC-DEFAULT: BOT-159 (proposed) 0.15 s, 40 units/s
pub const ESCAPE_BLOCK_SECS: f32 = 0.15;
pub const ESCAPE_BLOCK_SPEED: f32 = 40.0;

/// BOT-159: the jump of a way off an island: none, at once, or where the walk is blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeJump {
    No,
    Start,
    Blocked,
}

/// BOT-159: a way off an island: walk along `yaw` (with the jump `jump`) for [`ESCAPE_SECS`]; the body falls
/// `fall` units on it and ends at `end`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Escape {
    pub yaw: f32,
    pub jump: EscapeJump,
    pub fall: f32,
    pub end: [f32; 3],
}

/// BOT-159: the way off the island `part` (the nodes of the grid the body is confined to, `NavGrid::confined_part`)
/// that a body standing at its place takes: of [`ESCAPE_HEADINGS`] headings, walked, jumped at once and jumped where
/// the walk is blocked on a copy for
/// [`ESCAPE_SECS`] with the movement code, a run whose places stay out of the kill volumes, whose end stands on the
/// floor (the last two places level) out of danger (`nav::in_danger`), away from every node of the island
/// ([`ESCAPE_CLEAR`] flat or [`ESCAPE_CLEAR_Y`] in height), and that leaves the island by a fall of more than a
/// step down ([`crate::nav::STEP_DOWN`]) or over something the body stood on higher than a step (a bar it jumped
/// over); the fall (its highest place to its end) at most `max_drop`. The least fall wins (then the first heading,
/// walks before jumps). The session walks it the same way (`motion::Body::escape_jump_due`). The server's fall damage is the price (BOT-159: a 400-unit fall costs 50 health); before,
/// the greedy way-finder refused every drop deeper than [`crate::nav::DEADLY_DROP`] (BOT-145) and the bot stayed in
/// the pit for the match.
// DC-DEFAULT: BOT-159 (proposed)
pub fn escape_move(
    body: &crate::motion::Body,
    world: &dc_sim::CollisionWorld,
    kill: &[dc_sim::ConvexVolume],
    part: &[[f32; 3]],
    max_drop: f32,
) -> Option<Escape> {
    use dc_sim::Collider;
    let start = body.pos();
    let dims = body.dims();
    let shape = dc_sim::Cylinder::from_dims(dc_sim::V3::new(dims[0], dims[1], dims[2]));
    let v = |p: [f32; 3]| dc_sim::V3::new(p[0], p[1], p[2]);
    let stands = |p: [f32; 3]| {
        world
            .sweep(&shape, v(p), v([p[0], p[1] - 4.0, p[2]]))
            .is_some_and(|h| h.t < 1.0)
    };
    let mut best: Option<Escape> = None;
    for i in 0..ESCAPE_HEADINGS {
        let yaw = i as f32 * std::f32::consts::TAU / ESCAPE_HEADINGS as f32;
        for jump in [EscapeJump::No, EscapeJump::Start, EscapeJump::Blocked] {
            let path = body.probe_escape(yaw, jump, ESCAPE_SECS, ESCAPE_SAMPLES);
            let n = path.len();
            if n < 2 {
                continue;
            }
            let end = path[n - 1];
            let top = path.iter().fold(start[1], |t, p| t.max(p[1]));
            let fall = top - end[1];
            let landed = (path[n - 2][1] - end[1]).abs() <= 1.0;
            let out = part.iter().all(|q| {
                (q[0] - end[0]).hypot(q[2] - end[2]) > ESCAPE_CLEAR
                    || (q[1] - end[1]).abs() > ESCAPE_CLEAR_Y
            });
            let left = start[1] - end[1] > crate::nav::STEP_DOWN
                || path
                    .iter()
                    .any(|p| p[1] > start[1] + crate::nav::STEP_UP && stands(*p));
            if !landed
                || !out
                || !left
                || fall > max_drop
                || best.is_some_and(|b| b.fall <= fall)
                || path.iter().any(|p| kill.iter().any(|k| k.touches(&shape, v(*p))))
                || crate::nav::in_danger(world, kill, dims, end)
            {
                continue;
            }
            best = Some(Escape {
                yaw,
                jump,
                fall,
                end,
            });
        }
    }
    best
}

/// BOT-134: the evasive move of this tick made safe by `s`: a strafe toward an unsafe side turns to the other side
/// when that one is safe, else stops; a step back into danger is not taken. With neither side safe the jump is not
/// taken either (BOT-116). Returns what was changed (`"flip"`, `"stop"`, `"no_back"`) for the log.
// DC-DEFAULT: BOT-116 (proposed), BOT-134 (proposed)
pub fn gate(mv: &mut Move, s: Safety) -> Option<&'static str> {
    let mut what = None;
    let side_ok = |d: i8| if d < 0 { s.left } else { s.right };
    if mv.strafe != 0 && !side_ok(mv.strafe) {
        if side_ok(-mv.strafe) {
            mv.strafe = -mv.strafe;
            what = Some("flip");
        } else {
            mv.strafe = 0;
            what = Some("stop");
        }
    }
    if !s.left && !s.right {
        mv.jump = false;
    }
    if mv.back && !s.back {
        mv.back = false;
        what = what.or(Some("no_back"));
    }
    what
}

/// BOT-145: how far beside the body the danger round it is looked for, how long the step inward runs on a copy (and is
/// walked), and at how many places along it the danger test looks.
// DC-DEFAULT: BOT-145 (proposed) 64 units, 0.4 s, 4 places
pub const INSIDE_REACH: f32 = 64.0;
pub const INSIDE_SECS: f32 = 0.4;
pub const INSIDE_SAMPLES: usize = 4;

/// BOT-145: where a bot at a deck's edge that may strafe to neither side (BOT-134 `stop`) steps instead: the danger
/// round the body (in or above a kill volume, the void, a drop of more than 300, [`INSIDE_REACH`] beside it in 8
/// directions) gives the way away from it, and of the 8 walking directions, nearest to that way first, the first
/// whose walk of [`INSIDE_SECS`] (run on a copy) and the place one body width beyond its end stay out of danger is
/// taken. `None` when no danger lies beside the body (nothing to step away from) or no direction is safe. Before, a
/// bot on Power Surge's upper deck stood 20 s on the spot while it fought (every strafe refused).
// DC-DEFAULT: BOT-145 (proposed)
pub fn inside_heading(
    body: &crate::motion::Body,
    world: &dc_sim::CollisionWorld,
    kill: &[dc_sim::ConvexVolume],
) -> Option<f32> {
    let (pos, dims) = (body.pos(), body.dims());
    let dir = |k: usize| k as f32 * std::f32::consts::TAU / 8.0;
    let (mut sx, mut sz) = (0.0f32, 0.0f32);
    for k in 0..8 {
        let a = dir(k);
        // yaw convention of the walk (`hunt::yaw_to`): x = sin, z = cos
        let p = [
            pos[0] + a.sin() * INSIDE_REACH,
            pos[1],
            pos[2] + a.cos() * INSIDE_REACH,
        ];
        if crate::nav::in_danger(world, kill, dims, p) {
            sx += a.sin();
            sz += a.cos();
        }
    }
    if sx.hypot(sz) < 1e-3 {
        return None;
    }
    let away = (-sx).atan2(-sz);
    let mut heads: Vec<f32> = (0..8).map(dir).collect();
    let off = |h: f32| {
        let d = (h - away).rem_euclid(std::f32::consts::TAU);
        d.min(std::f32::consts::TAU - d)
    };
    heads.sort_by(|a, b| off(*a).total_cmp(&off(*b)));
    heads.into_iter().find(|h| {
        let path = body.probe_path(Some(*h), None, INSIDE_SECS, INSIDE_SAMPLES);
        if !path_safe(world, kill, dims, &path) {
            return false;
        }
        let end = path[path.len() - 1];
        let w = 2.0 * dims[0] * EDGE_WIDTHS;
        let beyond = [end[0] + h.sin() * w, end[1], end[2] + h.cos() * w];
        // (a walk that a wall stops is no step either)
        (end[0] - pos[0]).hypot(end[2] - pos[2]) >= dims[0]
            && !crate::nav::in_danger(world, kill, dims, beyond)
    })
}

/// BOT-134: the walking direction (yaw) away from a kill volume whose edge the body at `pos` (half extents `dims`)
/// stands within [`EDGE_WIDTHS`] body widths of: the box moved that far in each of 8 directions is tested against the
/// volumes, and the bot steps away from the side (or sides) that touch. `None` when no side touches, or all do.
// DC-DEFAULT: BOT-134 (proposed)
pub fn edge_away(kill: &[dc_sim::ConvexVolume], dims: [f32; 3], pos: [f32; 3]) -> Option<f32> {
    if kill.is_empty() {
        return None;
    }
    let shape = dc_sim::Cylinder::from_dims(dc_sim::V3::from_array(dims));
    let reach = 2.0 * dims[0] * EDGE_WIDTHS;
    let (mut sx, mut sz, mut n) = (0.0f32, 0.0f32, 0);
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::TAU / 8.0;
        // yaw convention of the walk (`hunt::yaw_to`): x = sin, z = cos
        let (dx, dz) = (a.sin(), a.cos());
        let at = dc_sim::V3::new(pos[0] + dx * reach, pos[1], pos[2] + dz * reach);
        if kill.iter().any(|v| v.touches(&shape, at)) {
            sx += dx;
            sz += dz;
            n += 1;
        }
    }
    if n == 0 || n == 8 || sx.hypot(sz) < 1e-3 {
        return None;
    }
    Some((-sx).atan2(-sz))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        e: &mut Evader,
        from: Millis,
        to: Millis,
        step: Millis,
        dist: f32,
        level: u8,
    ) -> Vec<(Millis, Move)> {
        let mut out = Vec::new();
        let mut t = from;
        while t <= to {
            out.push((t, e.tick(t, 0.0, dist, level)));
            t += step;
        }
        out
    }

    #[test]
    fn the_strafe_reverses_within_the_period_bounds() {
        let mut e = Evader::new(3);
        let m = run(&mut e, 0, 20_000, 20, 500.0, 50);
        let mut last = m[0].1.strafe;
        let mut at = 0;
        let mut gaps = Vec::new();
        for (t, mv) in &m[1..] {
            assert!(mv.strafe == -1 || mv.strafe == 1);
            if mv.strafe != last {
                gaps.push(t - at);
                at = *t;
                last = mv.strafe;
            }
        }
        assert!(gaps.len() >= 10, "{gaps:?}");
        let (lo, hi) = (
            lerp(REVERSE_MIN_MS.0, REVERSE_MIN_MS.1, 50) as u32,
            lerp(REVERSE_MAX_MS.0, REVERSE_MAX_MS.1, 50) as u32,
        );
        for g in &gaps[1..] {
            assert!(*g >= lo && *g <= hi + 20, "gap {g} outside {lo}..{hi}");
        }
    }

    #[test]
    fn a_hit_reverses_at_once_and_jumps_once() {
        let mut e = Evader::new(5);
        let before = e.tick(0, 0.0, 400.0, 50).strafe;
        e.tick(20, 0.0, 400.0, 50);
        e.on_damage(40);
        let m = e.tick(40, 0.0, 400.0, 50);
        assert_eq!(m.strafe, -before);
        assert!(m.jump && m.note == Some("hit_reverse"));
        // the same hit does not react twice; a hit older than the window neither
        let m2 = e.tick(60, 0.0, 400.0, 50);
        assert_eq!(m2.strafe, -before);
        assert!(!m2.jump);
        e.on_damage(60);
        let m3 = e.tick(60 + HIT_REACT_MS + 1, 0.0, 400.0, 50);
        assert_eq!(m3.note.filter(|n| *n == "hit_reverse"), None);
    }

    /// BOT-116: on a walkway along a drop the sides are no place to strafe to; its back is (the walkway goes on
    /// behind); in the middle of a wide floor everything is safe.
    #[test]
    fn a_strafe_toward_a_drop_is_unsafe() {
        use crate::hunt::tests::solid_box;
        use crate::motion::{Body, MotionKind};
        use std::sync::Arc;
        // a floor x -2000..2000 at y 0, z -2000..2000, and a walkway x -40..40 running along z at y 0 over a pit
        // 2000 deep on both sides (x beyond 40)
        let w = Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-40.0, -64.0, -2000.0], [40.0, 0.0, 2000.0]),
                solid_box([-4000.0, -2100.0, -4000.0], [4000.0, -2000.0, 4000.0]),
            ],
            &[],
        ));
        // facing +x (the enemy across the pit): the sides are +z / -z along the walkway, the back is -x into the pit
        let yaw = std::f32::consts::FRAC_PI_2;
        let b = Body::new(
            [0.0, 50.0, 0.0],
            [15.0, 50.0, 15.0],
            yaw,
            MotionKind::Hunt,
            30,
            Some(w.clone()),
        );
        let s = evade_safety(&b, &w, &[], yaw, PROBE_SECS);
        assert!(
            s.left && s.right && !s.back,
            "along the walkway; not back into the pit: {s:?}"
        );
        // facing +z (along the walkway): both sides are the pit
        let b = Body::new(
            [0.0, 50.0, 0.0],
            [15.0, 50.0, 15.0],
            0.0,
            MotionKind::Hunt,
            30,
            Some(w.clone()),
        );
        let s = evade_safety(&b, &w, &[], 0.0, PROBE_SECS);
        assert!(!s.left && !s.right && s.back, "{s:?}");
        let floor = Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![solid_box([-2000.0, -64.0, -2000.0], [2000.0, 0.0, 2000.0])],
            &[],
        ));
        let b = Body::new(
            [0.0, 50.0, 0.0],
            [15.0, 50.0, 15.0],
            0.0,
            MotionKind::Hunt,
            30,
            Some(floor.clone()),
        );
        assert_eq!(
            evade_safety(&b, &floor, &[], 0.0, PROBE_SECS),
            Safety::default()
        );
    }

    /// A floor that ends at x 40 over a pit 2000 deep (the edge runs along z), a body at `x` on it facing `yaw`.
    fn at_an_edge(
        x: f32,
        yaw: f32,
    ) -> (crate::motion::Body, std::sync::Arc<dc_sim::CollisionWorld>) {
        use crate::hunt::tests::solid_box;
        use crate::motion::{Body, MotionKind};
        let w = std::sync::Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-4000.0, -64.0, -4000.0], [40.0, 0.0, 4000.0]),
                solid_box([-4000.0, -2100.0, -4000.0], [4000.0, -2000.0, 4000.0]),
            ],
            &[],
        ));
        let b = Body::new(
            [x, 50.0, 0.0],
            [15.0, 50.0, 15.0],
            yaw,
            MotionKind::Hunt,
            30,
            Some(w.clone()),
        );
        (b, w)
    }

    /// BOT-134: with the pit on one side only, a strafe toward the pit turns to the other side; a strafe away from it
    /// is kept; with both sides unsafe the strafe and the jump stop; a step back into danger is not taken.
    #[test]
    fn a_strafe_toward_the_pit_turns_to_the_safe_side() {
        // facing +z along the edge: +x (the pit) is one side, -x the other
        let (b, w) = at_an_edge(0.0, 0.0);
        let s = evade_safety(&b, &w, &[], 0.0, PROBE_SECS);
        assert!(s.left != s.right && s.back, "{s:?}");
        let pit = if s.left { 1 } else { -1 };
        let mut mv = evade_move(0.0, pit, false, true);
        assert_eq!(gate(&mut mv, s), Some("flip"));
        assert_eq!(mv.strafe, -pit);
        assert!(
            mv.jump,
            "the safe side keeps the jump (asked for separately)"
        );
        let mut mv = evade_move(0.0, -pit, false, false);
        assert_eq!(gate(&mut mv, s), None);
        assert_eq!(mv.strafe, -pit);
        let none = Safety {
            left: false,
            right: false,
            back: false,
        };
        let mut mv = evade_move(0.0, 1, true, true);
        assert_eq!(gate(&mut mv, none), Some("stop"));
        assert!(mv.strafe == 0 && !mv.jump && !mv.back);
        // far from the edge everything is safe
        let (b, w) = at_an_edge(-1000.0, 0.0);
        assert_eq!(
            evade_safety(&b, &w, &[], 0.0, PROBE_SECS),
            Safety::default()
        );
    }

    /// BOT-134: a strafe that ends within a body width of a kill volume is unsafe although the body never touches it
    /// (a wall stops the body at x 85, the volume starts at x 110); the other side stays safe.
    #[test]
    fn a_strafe_that_ends_beside_a_kill_volume_is_unsafe() {
        use crate::hunt::tests::solid_box;
        use crate::motion::{Body, MotionKind};
        let w = std::sync::Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-4000.0, -64.0, -4000.0], [4000.0, 0.0, 4000.0]),
                solid_box([100.0, 0.0, -4000.0], [140.0, 300.0, 4000.0]),
            ],
            &[],
        ));
        let kill = vec![dc_sim::ConvexVolume::from_box(
            dc_sim::V3::new(110.0, 0.0, -4000.0),
            dc_sim::V3::new(400.0, 300.0, 4000.0),
        )];
        let b = Body::new(
            [0.0, 50.0, 0.0],
            [15.0, 50.0, 15.0],
            0.0,
            MotionKind::Hunt,
            30,
            Some(w.clone()),
        );
        let s = evade_safety(&b, &w, &kill, 0.0, PROBE_SECS);
        assert!(s.left != s.right && s.back, "{s:?}");
        assert_eq!(
            evade_safety(&b, &w, &[], 0.0, PROBE_SECS),
            Safety::default(),
            "without the volume both sides are safe"
        );
    }

    /// BOT-134: a jump while walking toward the pit is unsafe, one walking away from it is not.
    #[test]
    fn a_jump_toward_the_pit_is_unsafe() {
        let half = std::f32::consts::FRAC_PI_2;
        let (b, w) = at_an_edge(-20.0, half);
        assert!(!jump_safe(&b, &w, &[], Some(half), None));
        assert!(jump_safe(&b, &w, &[], Some(-half), None));
        assert!(jump_safe(&b, &w, &[], None, None), "in place");
    }

    /// BOT-134: a body within a body width of a kill volume's edge steps away from it; farther away it does not.
    #[test]
    fn a_body_at_a_kill_volume_edge_steps_away() {
        let water = vec![dc_sim::ConvexVolume::from_box(
            dc_sim::V3::new(40.0, -100.0, -500.0),
            dc_sim::V3::new(400.0, 100.0, 500.0),
        )];
        let dims = [15.0, 50.0, 15.0];
        let y = edge_away(&water, dims, [0.0, 50.0, 0.0]).expect("at the edge");
        assert!(y.sin() < -0.9, "away from +x: {y}");
        assert_eq!(edge_away(&water, dims, [-100.0, 50.0, 0.0]), None);
        assert_eq!(edge_away(&[], dims, [0.0, 50.0, 0.0]), None);
    }

    #[test]
    fn jumps_keep_their_gap_and_the_crouch_needs_range_and_calm() {
        let mut e = Evader::new(11);
        let m = run(&mut e, 0, 60_000, 20, 500.0, 100);
        let jumps: Vec<Millis> = m
            .iter()
            .filter(|(_, mv)| mv.jump)
            .map(|(t, _)| *t)
            .collect();
        assert!(
            jumps.len() >= 10,
            "{} jumps in a minute at skill 100",
            jumps.len()
        );
        for w in jumps.windows(2) {
            assert!(w[1] - w[0] >= JUMP_GAP_MS, "{w:?}");
        }
        assert!(m.iter().all(|(_, mv)| !mv.crouch), "no crouch at 500 units");
        let mut e = Evader::new(11);
        assert!(e.tick(0, 0.0, 1500.0, 50).crouch, "far and calm: crouch");
        e.on_damage(10);
        assert!(!e.tick(20, 0.0, 1500.0, 50).crouch, "just hit: up");
        assert!(e.tick(20 + CROUCH_CALM_MS, 0.0, 1500.0, 50).crouch);
        // far away nothing jumps for a while (no fight)
        let mut e = Evader::new(11);
        assert!(run(&mut e, 0, 10_000, 20, 3000.0, 100)
            .iter()
            .all(|(_, mv)| !mv.jump));
    }

    /// BOT-145: a body at the edge of a deck over a drop (both strafes along the edge would run over it) steps inward,
    /// away from the edge; in the middle of the deck there is nothing to step away from.
    #[test]
    fn a_body_at_a_deck_edge_steps_inward() {
        use dc_nav::testworld::solid_box;
        let w = std::sync::Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-3000.0, -1064.0, -3000.0], [3000.0, -1000.0, 3000.0]),
                // a deck 1000 above the floor below, its edge at z = 0 (the deck lies at z < 0)
                solid_box([-1000.0, -64.0, -1000.0], [1000.0, 0.0, 0.0]),
            ],
            &[],
        ));
        let mk = |pos: [f32; 3]| {
            crate::motion::Body::new(
                pos,
                [16.0, 40.0, 16.0],
                0.0,
                crate::motion::MotionKind::Hunt,
                60,
                Some(w.clone()),
            )
        };
        let edge = mk([0.0, 41.0, -20.0]);
        let h = inside_heading(&edge, &w, &[]).expect("a step inward");
        // inward is -z: yaw pi (x = sin, z = cos)
        assert!(h.cos() < -0.5, "heading {h}");
        let mid = mk([0.0, 41.0, -500.0]);
        assert_eq!(inside_heading(&mid, &w, &[]), None);
    }

    /// BOT-159: a deck 120 square whose floor ends over a floor `drop` below on every side, a body in its middle and
    /// the deck's grid nodes (every 48 units).
    fn on_a_deck(drop: f32) -> (crate::motion::Body, std::sync::Arc<dc_sim::CollisionWorld>, Vec<[f32; 3]>) {
        use crate::hunt::tests::solid_box;
        use crate::motion::{Body, MotionKind};
        let w = std::sync::Arc::new(dc_sim::CollisionWorld::from_parts(
            vec![
                solid_box([-60.0, -64.0, -60.0], [60.0, 0.0, 60.0]),
                solid_box([-4000.0, -drop - 64.0, -4000.0], [4000.0, -drop, 4000.0]),
            ],
            &[],
        ));
        let b = Body::new([0.0, 50.0, 0.0], [15.0, 50.0, 15.0], 0.0, MotionKind::Hunt, 30, Some(w.clone()));
        let part = (-1..=1)
            .flat_map(|i| (-1..=1).map(move |j| [i as f32 * 48.0, 50.0, j as f32 * 48.0]))
            .collect();
        (b, w, part)
    }

    /// BOT-159: off a deck over a floor 330 below the way off is a walk off its edge (a fall the greedy way-finder
    /// refuses, BOT-145); with the fall limit below the drop, or a kill volume on the floor, there is none.
    #[test]
    fn the_way_off_a_deck_is_a_fall_within_the_limit_and_out_of_kill_volumes() {
        let (b, w, part) = on_a_deck(330.0);
        let e = escape_move(&b, &w, &[], &part, 400.0).expect("a way off");
        assert!((300.0..=400.0).contains(&e.fall), "{e:?}");
        assert!(e.end[1] < -250.0, "{e:?}");
        assert_eq!(escape_move(&b, &w, &[], &part, 250.0), None);
        let kill = dc_sim::ConvexVolume::from_box(
            dc_sim::V3::new(-4000.0, -330.0, -4000.0),
            dc_sim::V3::new(4000.0, -200.0, 4000.0),
        );
        assert_eq!(escape_move(&b, &w, &[kill], &part, 400.0), None);
    }
}
