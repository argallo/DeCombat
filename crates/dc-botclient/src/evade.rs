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

/// BOT-116: whether an evasive move keeps the body out of danger, as `(sides, back)`: the body runs on a copy for
/// `secs` to either side of `yaw` and backwards (the movement code), and a side whose end is in danger
/// ([`crate::nav::in_danger`]: a kill volume, the void, a high drop) makes the sides unsafe.
// DC-DEFAULT: BOT-116 (proposed)
pub fn evade_safety(
    body: &crate::motion::Body,
    world: &dc_sim::CollisionWorld,
    kill: &[dc_sim::ConvexVolume],
    yaw: f32,
    secs: f32,
) -> (bool, bool) {
    let ok = |y: f32| {
        let end = body.probe_walk(y, secs);
        !crate::nav::in_danger(world, kill, body.dims(), end)
    };
    let half = std::f32::consts::FRAC_PI_2;
    (ok(yaw + half) && ok(yaw - half), ok(yaw + 2.0 * half))
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
        assert_eq!(
            evade_safety(&b, &w, &[], yaw, 0.4),
            (true, false),
            "along the walkway; not back into the pit"
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
        assert_eq!(evade_safety(&b, &w, &[], 0.0, 0.4), (false, true));
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
        assert_eq!(evade_safety(&b, &floor, &[], 0.0, 0.4), (true, true));
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
}
