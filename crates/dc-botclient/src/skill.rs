//! Human-like aiming for the shooter (`--skill <0..100>`), sans-IO: a finite body turn rate, a fire tolerance,
//! a reaction delay after a target becomes visible, a per-shot aim error, bursts with pauses and the weapon's ROM
//! spread, so a person can fight the bot and its traffic looks like a player's. All numbers are DeCombat bot rules
//! (test traffic), scaled linearly by the skill; none of them is a 2009 rule.
// DC-DEFAULT: BOT-20..BOT-25 (proposed, staged in DC_DEFAULTS.md)
// DC-DEFAULT: CLI-31 (proposed) the spread law of dc-client, applied to the bot's own shots

use std::f32::consts::{PI, TAU};

/// The skill used when `--skill` is not given.
// DC-DEFAULT: BOT-20 (proposed) skill 50
pub const STANDARD_LEVEL: u8 = 50;

/// Linear interpolation between the value at skill 0 and the value at skill 100.
fn lerp(at0: f32, at100: f32, t: f32) -> f32 {
    at0 + (at100 - at0) * t
}

/// The aiming parameters of one skill level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Skill {
    /// 0..=100.
    pub level: u8,
    /// Body / aim turn rate, degrees per second.
    // DC-DEFAULT: BOT-21 (proposed) 90 deg/s at skill 0 .. 540 deg/s at 100 (315 at 50)
    pub turn_deg_s: f32,
    /// The aim must be within this angle of the aim point before the bot fires, degrees.
    // DC-DEFAULT: BOT-21 (proposed) 6 deg at 0 .. 1.5 deg at 100 (3.75 at 50)
    pub fire_tolerance_deg: f32,
    /// Delay after a target first becomes visible (clear line) before the first shot, ms.
    // DC-DEFAULT: BOT-22 (proposed) 1000 ms at 0 .. 200 ms at 100 (600 at 50)
    pub reaction_ms: u32,
    /// Delay after the line to the same target was lost for a short while, ms.
    // DC-DEFAULT: BOT-22 (proposed) half the reaction delay; a loss longer than REACQUIRE_WINDOW_MS counts as new
    pub reacquire_ms: u32,
    /// Standard deviation of the per-shot aim error on each of two axes across the aim, degrees.
    // DC-DEFAULT: BOT-23 (proposed) 4.0 deg at 0 .. 0.4 deg at 100 (2.2 at 50)
    pub aim_sigma_deg: f32,
    /// Shots per burst, inclusive range.
    // DC-DEFAULT: BOT-24 (proposed) 2..3 at 0 .. 6..10 at 100 (4..7 at 50)
    pub burst_min: u32,
    pub burst_max: u32,
    /// Pause between two bursts, ms.
    // DC-DEFAULT: BOT-24 (proposed) 900 ms at 0 .. 300 ms at 100 (600 at 50), +-25 % random
    pub burst_pause_ms: u32,
    /// BOT-90: the aim follows where the target was this long ago (a moving target is led badly), ms.
    // DC-DEFAULT: BOT-90 (proposed) 280 ms at 0 .. 70 ms at 100 (175 at 50)
    pub lag_ms: u32,
    /// BOT-90: standard deviation of the slow aim wander (an Ornstein-Uhlenbeck drift of the aim point that the
    /// shots of a burst share), degrees.
    // DC-DEFAULT: BOT-90 (proposed) 2.0 deg at 0 .. 0.3 deg at 100 (1.15 at 50); time constant WANDER_TAU_MS
    pub wander_sigma_deg: f32,
    /// BOT-90: the fraction of a turn onto a new target that the aim overshoots before it settles.
    // DC-DEFAULT: BOT-90 (proposed) 0.30 at 0 .. 0.05 at 100 (0.175 at 50); settles with OVERSHOOT_TAU_MS
    pub overshoot: f32,
    /// BOT-90: the overshoot never exceeds this, degrees (a half turn still settles within a moment).
    // DC-DEFAULT: BOT-90 (proposed) 10 deg at 0 .. 4 deg at 100 (7 at 50)
    pub overshoot_cap_deg: f32,
    /// BOT-90: the aim climbs this much per shot of a burst (uncompensated recoil), degrees; it falls back with
    /// RECOIL_TAU_MS.
    // DC-DEFAULT: BOT-90 (proposed) 0.9 deg at 0 .. 0.25 deg at 100 (0.575 at 50)
    pub recoil_deg: f32,
    /// BOT-90: the per-shot aim error is this many times larger while the bot moves (walks, strafes, is airborne).
    // DC-DEFAULT: BOT-90 (proposed) 1.9 at 0 .. 1.25 at 100 (1.575 at 50)
    pub moving_factor: f32,
    /// BOT-90: a target already engaged is kept unless another one is nearer than this fraction of its distance.
    // DC-DEFAULT: BOT-90 (proposed) 0.55 at every skill
    pub stick_ratio: f32,
}

/// BOT-90: time constant of the aim wander, ms.
pub const WANDER_TAU_MS: u32 = 600;
/// BOT-90: time constant of the overshoot's settling, ms.
pub const OVERSHOOT_TAU_MS: u32 = 220;
/// BOT-90: time constant of the recoil climb's fall, ms.
pub const RECOIL_TAU_MS: u32 = 260;

/// A line loss longer than this makes the next sighting a new one (full reaction delay).
// DC-DEFAULT: BOT-22 (proposed) 2000 ms
pub const REACQUIRE_WINDOW_MS: u32 = 2000;

impl Skill {
    pub fn new(level: u8) -> Self {
        let level = level.min(100);
        let t = level as f32 / 100.0;
        let reaction = lerp(1000.0, 200.0, t).round() as u32;
        Skill {
            level,
            turn_deg_s: lerp(90.0, 540.0, t),
            fire_tolerance_deg: lerp(6.0, 1.5, t),
            reaction_ms: reaction,
            reacquire_ms: reaction / 2,
            aim_sigma_deg: lerp(4.0, 0.4, t),
            burst_min: lerp(2.0, 6.0, t).round() as u32,
            burst_max: lerp(3.0, 10.0, t).round() as u32,
            burst_pause_ms: lerp(900.0, 300.0, t).round() as u32,
            lag_ms: lerp(280.0, 70.0, t).round() as u32,
            wander_sigma_deg: lerp(2.0, 0.3, t),
            overshoot: lerp(0.30, 0.05, t),
            overshoot_cap_deg: lerp(10.0, 4.0, t),
            recoil_deg: lerp(0.9, 0.25, t),
            moving_factor: lerp(1.9, 1.25, t),
            stick_ratio: 0.55,
        }
    }

    /// BOT-91: the skill of `--skill <n>` or `--skill <lo>-<hi>`: a range gives every bot its own level, picked
    /// once from `seed` (the bot's name hashed by the caller), so a set of bots started alike differ and each
    /// bot stays the same across its restarts. `None` for `off` or a bad text.
    pub fn parse_level(text: &str, seed: u64) -> Option<u8> {
        let t = text.trim();
        if t.eq_ignore_ascii_case("off") || t.is_empty() {
            return None;
        }
        if let Some((lo, hi)) = t.split_once('-') {
            let lo: u8 = lo.trim().parse::<f32>().ok()?.clamp(0.0, 100.0) as u8;
            let hi: u8 = hi.trim().parse::<f32>().ok()?.clamp(0.0, 100.0) as u8;
            let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
            let mut r = AimRng::new(seed ^ 0x5c11_1000);
            return Some(r.range(lo as u32, hi as u32) as u8);
        }
        Some(t.parse::<f32>().ok()?.clamp(0.0, 100.0) as u8)
    }

    pub fn turn_rad_s(&self) -> f32 {
        self.turn_deg_s.to_radians()
    }
}

impl Default for Skill {
    fn default() -> Self {
        Skill::new(STANDARD_LEVEL)
    }
}

/// `a - b` wrapped to (-π, π].
pub fn wrap_diff(a: f32, b: f32) -> f32 {
    let mut d = (a - b) % TAU;
    if d > PI {
        d -= TAU;
    } else if d <= -PI {
        d += TAU;
    }
    d
}

/// Turns `(yaw, pitch)` toward `want` by at most `max_step` radians (on the larger of the two axes, both axes
/// scaled together so the path is straight).
pub fn turn_toward(cur: (f32, f32), want: (f32, f32), max_step: f32) -> (f32, f32) {
    let dy = wrap_diff(want.0, cur.0);
    let dp = want.1 - cur.1;
    let m = dy.abs().max(dp.abs());
    if m <= max_step.max(0.0) {
        return want;
    }
    let k = max_step.max(0.0) / m;
    (wrap_diff(cur.0 + dy * k, 0.0), cur.1 + dp * k)
}

/// The angle between two (yaw, pitch) directions, radians.
pub fn angle_between(a: (f32, f32), b: (f32, f32)) -> f32 {
    let fa = crate::shooter::forward(a.0, a.1);
    let fb = crate::shooter::forward(b.0, b.1);
    let dot = (fa[0] * fb[0] + fa[1] * fb[1] + fa[2] * fb[2]).clamp(-1.0, 1.0);
    dot.acos()
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        v
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Two unit axes across the unit vector `f`.
fn across(f: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let up = if f[1].abs() < 0.99 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let a = norm(cross(up, f));
    (a, cross(f, a))
}

/// `f` tilted by the angles `ea`, `eb` (radians) along the two axes across it.
pub fn tilt(f: [f32; 3], ea: f32, eb: f32) -> [f32; 3] {
    let (a, b) = across(f);
    let (ta, tb) = (ea.tan(), eb.tan());
    norm([
        f[0] + ta * a[0] + tb * b[0],
        f[1] + ta * a[1] + tb * b[1],
        f[2] + ta * a[2] + tb * b[2],
    ])
}

/// Deterministic random numbers for the aim (xorshift64*; one stream per bot, separate from the seed byte).
#[derive(Debug, Clone)]
pub struct AimRng(u64);

impl AimRng {
    pub fn new(seed: u64) -> Self {
        AimRng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Standard normal (Box-Muller).
    pub fn normal(&mut self) -> f32 {
        let u1 = self.unit().max(1e-7);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }

    /// Uniform integer in `lo..=hi`.
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % (hi - lo + 1) as u64) as u32
    }

    /// A direction uniform over the disk of the cone of half-angle `half` around `f`.
    pub fn in_cone(&mut self, f: [f32; 3], half: f32) -> [f32; 3] {
        if half.is_nan() || half <= 0.0 {
            return f;
        }
        let r = half * self.unit().sqrt();
        let phi = TAU * self.unit();
        let (s, c) = phi.sin_cos();
        tilt(f, (r * c).atan2(1.0), (r * s).atan2(1.0))
    }
}

/// The weapon spread fraction of CLI-31: grows by `PerturbIncSpeed x 0.036` per shot, falls by
/// `PerturbDecSpeed x 1.5` per second, cone half-angle `(MinPerturb + f (MaxPerturb - MinPerturb)) / 1000` rad.
// DC-DEFAULT: CLI-31 (proposed); OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] MinPerturb, MaxPerturb,
// PerturbIncSpeed, PerturbDecSpeed (read at run time)
pub mod spread {
    pub const UP_PER_UNIT: f32 = 0.036;
    pub const DOWN_PER_UNIT: f32 = 1.5;
    pub const RADIANS_PER_UNIT: f32 = 0.001;
    pub const DEFAULT_MIN: f32 = 3.0;
    pub const DEFAULT_MAX: f32 = 100.0;
    pub const DEFAULT_UP: f32 = 1.3;
    pub const DEFAULT_DOWN: f32 = 0.3;
}

/// The spread parameters of one weapon (ROM row values or the CLI-31 fallbacks).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpreadParams {
    pub min: f32,
    pub max: f32,
    pub up: f32,
    pub down: f32,
}

impl Default for SpreadParams {
    fn default() -> Self {
        SpreadParams {
            min: spread::DEFAULT_MIN,
            max: spread::DEFAULT_MAX,
            up: spread::DEFAULT_UP,
            down: spread::DEFAULT_DOWN,
        }
    }
}

impl SpreadParams {
    /// The cone half-angle in radians at spread fraction `f`.
    pub fn half_angle(&self, f: f32) -> f32 {
        (self.min + f.clamp(0.0, 1.0) * (self.max - self.min)).max(0.0) * spread::RADIANS_PER_UNIT
    }

    /// The fraction after `ms` without a shot.
    pub fn decay(&self, f: f32, ms: u32) -> f32 {
        (f - self.down * spread::DOWN_PER_UNIT * ms as f32 / 1000.0).max(0.0)
    }

    /// The fraction after one shot.
    pub fn grow(&self, f: f32) -> f32 {
        (f + self.up * spread::UP_PER_UNIT).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_scales_monotonically_and_hits_the_stated_points() {
        let s = Skill::new(50);
        assert_eq!(s.reaction_ms, 600);
        assert_eq!(s.reacquire_ms, 300);
        assert!((s.turn_deg_s - 315.0).abs() < 1e-3);
        assert!((s.aim_sigma_deg - 2.2).abs() < 1e-3);
        assert_eq!((s.burst_min, s.burst_max, s.burst_pause_ms), (4, 7, 600));
        let (lo, hi) = (Skill::new(0), Skill::new(100));
        assert!(lo.turn_deg_s < s.turn_deg_s && s.turn_deg_s < hi.turn_deg_s);
        assert!(lo.reaction_ms > s.reaction_ms && s.reaction_ms > hi.reaction_ms);
        assert!(lo.aim_sigma_deg > s.aim_sigma_deg && s.aim_sigma_deg > hi.aim_sigma_deg);
        assert_eq!(Skill::new(250).level, 100);
    }

    // BOT-90 / BOT-91: the feel parameters scale with the skill; a range picks one level per seed, the same each time
    #[test]
    fn feel_parameters_scale_and_a_skill_range_picks_a_stable_level() {
        let (lo, mid, hi) = (Skill::new(0), Skill::new(50), Skill::new(100));
        assert_eq!((lo.lag_ms, mid.lag_ms, hi.lag_ms), (280, 175, 70));
        assert!(
            lo.wander_sigma_deg > mid.wander_sigma_deg
                && mid.wander_sigma_deg > hi.wander_sigma_deg
        );
        assert!(
            lo.overshoot > hi.overshoot
                && lo.recoil_deg > hi.recoil_deg
                && lo.moving_factor > hi.moving_factor
        );
        assert_eq!(Skill::parse_level("40", 1), Some(40));
        assert_eq!(Skill::parse_level("off", 1), None);
        assert_eq!(Skill::parse_level("x", 1), None);
        let a = Skill::parse_level("30-65", 0x1234).unwrap();
        assert!((30..=65).contains(&a));
        assert_eq!(
            Skill::parse_level("30-65", 0x1234),
            Some(a),
            "stable per seed"
        );
        let levels: std::collections::BTreeSet<u8> = (0..40u64)
            .filter_map(|s| Skill::parse_level("30-65", s))
            .collect();
        assert!(levels.len() > 5, "{levels:?}");
        assert_eq!(
            Skill::parse_level("65-30", 7),
            Skill::parse_level("30-65", 7)
        );
    }

    #[test]
    fn turning_is_rate_limited_and_takes_the_short_way() {
        let c = turn_toward((0.0, 0.0), (1.0, 0.0), 0.25);
        assert!((c.0 - 0.25).abs() < 1e-6 && c.1 == 0.0);
        // across the ±π seam: from 3.0 to -3.0 is 0.283 rad the short way
        let c = turn_toward((3.0, 0.0), (-3.0, 0.0), 0.1);
        assert!((wrap_diff(c.0, 3.0) - 0.1).abs() < 1e-5, "{c:?}");
        // within one step: exactly the goal
        assert_eq!(turn_toward((0.0, 0.0), (0.1, 0.05), 0.2), (0.1, 0.05));
        assert!((angle_between((0.0, 0.0), (0.5, 0.0)) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn spread_follows_the_cli31_law() {
        let p = SpreadParams::default();
        assert!((p.half_angle(0.0) - 0.003).abs() < 1e-7);
        let f = p.grow(0.0);
        assert!((f - 1.3 * 0.036).abs() < 1e-6);
        assert_eq!(p.decay(f, 1000), 0.0);
        let mut r = AimRng::new(7);
        let f0 = [0.0, 0.0, 1.0];
        for _ in 0..1000 {
            let d = r.in_cone(f0, 0.05);
            assert!(d[2].acos() <= 0.0501, "{d:?}");
        }
    }
}
