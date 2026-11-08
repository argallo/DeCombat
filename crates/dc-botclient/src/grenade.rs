//! BOT-147: the bot's grenade. A person in a Combat Arms match throws the grenade of his loadout now and then: early
//! in a fight, at an enemy a few hundred units away or at the corner the enemy just went behind. This module holds the
//! numbers and the arithmetic of such a throw: the flight rules of the thrown round (read from the user's ROM at run
//! time, with the server's DSV-39 fallbacks), a prediction of where the round blows up (the server's own flight rule
//! of DSV-39 run against the bot's collision world), the search for the pitch that puts the blast near the aim point
//! and the safety test (the thrower and its team mates out of the blast). The decision and the throw sequence are the
//! shooter's ([`crate::shooter::Shooter`]).
// DC-DEFAULT: BOT-147 (proposed)

use std::collections::BTreeMap;

use dc_bute::ButeDoc;
use dc_rez::RezVfs;

/// The FX table entry with the projectile rows, their section prefix and the keys the flight uses.
// OBSERVED: ROM table key FX.TXT (entry of ATTRIBUTES.rez), [ProjectileFX<k>] Name, Gravity, GravityOverride, Lifetime,
// Velocity (the same keys dc-dserver reads for DSV-39; values read at run time, never copied)
pub const FX_ENTRY: &str = "ATTRIBUTES\\FX.TXT";
pub const PROJECTILE_SECTION: &str = "ProjectileFX";
pub const KEY_PFX_GRAVITY: &str = "Gravity";
pub const KEY_PFX_GRAVITY_OVERRIDE: &str = "GravityOverride";
pub const KEY_PFX_LIFETIME: &str = "Lifetime";
pub const KEY_PFX_VELOCITY: &str = "Velocity";
/// `[Ammo<n>]` keys of a thrown round: its kind, its projectile row and its blast.
// OBSERVED: ROM table keys WEAPONS.TXT [Ammo<n>] Type (2 = thrown), ProjectileFXName, AreaDamage, AreaDamageRadius
pub const KEY_AMMO_KIND: &str = "Type";
pub const AMMO_KIND_THROWN: f32 = 2.0;
pub const KEY_PROJECTILE_FX: &str = "ProjectileFXName";
pub const KEY_AREA_DAMAGE: &str = "AreaDamage";
pub const KEY_AREA_RADIUS: &str = "AreaDamageRadius";

/// The server's fallbacks of a thrown round without its ROM values (DSV-39): launch speed, gravity along +Y, fuse,
/// blast damage and radius; and its flight rule: the step, the share of the speed a bounce keeps, the rest speed.
// DC-DEFAULT: DSV-39 (proposed) 1500 u/s, -1100 u/s², 3000 ms, 100 at the centre, radius 400; 16 ms steps, bounce
// 0.35, rest below 100 u/s (the dserver defaults of `ProjectileTuning`, mirrored here: the bot predicts the flight
// the server will fly)
pub const DEFAULT_SPEED: f32 = 1500.0;
pub const DEFAULT_GRAVITY: f32 = -1100.0;
pub const DEFAULT_FUSE_MS: u32 = 3000;
pub const DEFAULT_DAMAGE: f32 = 100.0;
pub const DEFAULT_RADIUS: f32 = 400.0;
pub const FLIGHT_STEP_MS: u32 = 16;
pub const BOUNCE_KEEP: f32 = 0.35;
pub const REST_SPEED: f32 = 100.0;

/// BOT-147: when a bot throws. The aim point (the engaged enemy, or where it was last in clear sight) is between
/// [`MIN_RANGE`] and [`MAX_RANGE`] away; the throw is weighed once per sighting, within [`EARLY_MS`] of its start
/// (or within [`LOST_MS`] after the enemy went out of sight, at the place it was last seen), and taken with
/// [`CHANCE`]; after a throw the bot throws no other for [`COOLDOWN_MS`] (and one per life at most: the loadout
/// grants one). The predicted blast lies within [`MISS_SHARE`] of the blast radius of the aim point; the thrower is
/// at least [`SELF_SHARE`] radii from it and no living team mate within [`MATE_SHARE`] radii.
// DC-DEFAULT: BOT-147 (proposed) 300..900 units, 4 s early window, 3 s after the loss, chance 0.25, 90 s between two
// throws, miss <= 0.45 R, self >= 1.2 R, mates >= 1.1 R (tuned on the netlab rooms of BOT-147 to 1-3 throws per bot
// per 10 min, the coordinator's estimate of an average player)
pub const MIN_RANGE: f32 = 300.0;
pub const MAX_RANGE: f32 = 900.0;
pub const EARLY_MS: u32 = 4000;
pub const LOST_MS: u32 = 3000;
pub const CHANCE: f32 = 0.25;
pub const COOLDOWN_MS: u32 = 90_000;
pub const MISS_SHARE: f32 = 0.45;
pub const SELF_SHARE: f32 = 1.2;
pub const MATE_SHARE: f32 = 1.1;
/// A plan that failed (no pitch lands near enough, or the blast is unsafe) is tried again this much later.
pub const REPLAN_MS: u32 = 500;
/// BOT-147: the throw's error: the bot misjudges the landing point across and along the line to its mark, each
/// with a standard deviation of this multiple of the skill's per-shot aim error (BOT-23) as an angle seen from the
/// eye (12 degrees at skill 0 .. 1.2 at 100; about 70 units at 600 units for skill 50), and throws to that point.
// DC-DEFAULT: BOT-147 (proposed) 3.0
pub const ERROR_FACTOR: f32 = 3.0;

/// The throw sequence of the 2009 client without the first-person clips (CLI-29 fallbacks): the draw after the select,
/// the wind-up, the hold passes (one `UP_THROW_HOLD` 0x4d at the start of each) and the release `UP_FIRE` this long
/// into the release clip; the bot holds [`HOLD_PASSES`] passes (a quick throw).
// SPEC: CLI-29 (select 500 ms, throwable wind-up 700 ms, hold passes 67 ms, release at 133 ms of the release clip);
// GAME_MESSAGES_2009_C2S.md §4.6 (0x4d repeats while held, then the release 0x82)
// DC-DEFAULT: BOT-147 (proposed) two hold passes
pub const DRAW_MS: u32 = 500;
pub const WINDUP_MS: u32 = 700;
pub const HOLD_PASS_MS: u32 = 67;
pub const HOLD_PASSES: u32 = 2;
pub const RELEASE_AT_MS: u32 = 133;
/// A throw given up after its hold passes (the blast would be unsafe after all): the gun fires no round for this
/// long, so that no shot falls in the server's throw window after the last hold (DSV-29, 1000 ms) and flies as the
/// grenade.
// DC-DEFAULT: BOT-147 (proposed) 1100 ms
pub const CANCEL_HOLD_OFF_MS: u32 = 1100;
/// A kill by the grenade's weapon counts as the grenade's within its fuse and this much after the release.
// DC-DEFAULT: BOT-147 (proposed) 1500 ms
pub const OUTCOME_SLACK_MS: u32 = 1500;
/// While its grenade is live a bot within this much of the blast's keep-out ring ([`SELF_SHARE`] radii) that would
/// enter the ring within [`APPROACH_LOOK_S`] at its current velocity stops and waits for the blast.
// DC-DEFAULT: BOT-147 (proposed) 300 units, 1.5 s
pub const APPROACH_MARGIN: f32 = 300.0;
pub const APPROACH_LOOK_S: f32 = 1.5;

/// The flight and blast of one thrown round.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LobRow {
    pub speed: f32,
    /// Acceleration along +Y (negative: down).
    pub gravity: f32,
    pub fuse_ms: u32,
    pub damage: f32,
    pub radius: f32,
}

impl Default for LobRow {
    fn default() -> Self {
        LobRow {
            speed: DEFAULT_SPEED,
            gravity: DEFAULT_GRAVITY,
            fuse_ms: DEFAULT_FUSE_MS,
            damage: DEFAULT_DAMAGE,
            radius: DEFAULT_RADIUS,
        }
    }
}

fn num(v: Option<&String>) -> Option<f32> {
    let t = v?.trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

fn folded(s: &str) -> String {
    s.trim().trim_matches('"').trim().to_ascii_lowercase()
}

fn index_of(section: &str, prefix: &str) -> Option<u16> {
    let rest = section.strip_prefix(prefix)?;
    (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| rest.parse().ok())
        .flatten()
}

/// The thrown weapons of the weapon table (first ammunition row of `Type` 2) with their flight (the FX table's
/// projectile row named by the ammunition, when `fx` is given) and blast, each missing value the DSV-39 fallback.
pub fn lob_rows(weapons: &ButeDoc, fx: Option<&ButeDoc>) -> BTreeMap<u16, LobRow> {
    // projectile rows by folded name: (velocity, gravity on, gravity override, lifetime s)
    type Pfx = (Option<f32>, Option<f32>, Option<f32>, Option<f32>);
    let mut pfx: BTreeMap<String, Pfx> = BTreeMap::new();
    if let Some(fx) = fx {
        for (name, sec) in fx.iter() {
            if !name.starts_with(PROJECTILE_SECTION) {
                continue;
            }
            if let Some(n) = sec.get("Name") {
                pfx.entry(folded(n)).or_insert((
                    num(sec.get(KEY_PFX_VELOCITY)),
                    num(sec.get(KEY_PFX_GRAVITY)),
                    num(sec.get(KEY_PFX_GRAVITY_OVERRIDE)),
                    num(sec.get(KEY_PFX_LIFETIME)),
                ));
            }
        }
    }
    let mut ammo: BTreeMap<String, LobRow> = BTreeMap::new();
    for (name, sec) in weapons.iter() {
        if index_of(name, "Ammo").is_none() {
            continue;
        }
        let Some(nm) = sec.get("Name") else {
            continue;
        };
        if num(sec.get(KEY_AMMO_KIND)) != Some(AMMO_KIND_THROWN) {
            continue;
        }
        let mut row = LobRow::default();
        if let Some(p) = sec.get(KEY_PROJECTILE_FX).and_then(|n| pfx.get(&folded(n))) {
            if let Some(v) = p.0.filter(|v| *v > 0.0) {
                row.speed = v;
            }
            // the server's rule (DSV-39): no gravity when `Gravity` is 0, the override when it is negative
            row.gravity = match (p.1, p.2) {
                (Some(0.0), _) => 0.0,
                (_, Some(o)) if o < 0.0 => o,
                _ => DEFAULT_GRAVITY,
            };
            if let Some(s) = p.3.filter(|s| *s > 0.0 && *s < 600.0) {
                row.fuse_ms = (s * 1000.0).round() as u32;
            }
        }
        if let Some(d) = num(sec.get(KEY_AREA_DAMAGE)).filter(|d| *d > 0.0) {
            row.damage = d;
        }
        if let Some(r) = num(sec.get(KEY_AREA_RADIUS)).filter(|r| *r > 0.0) {
            row.radius = r;
        }
        ammo.entry(folded(nm)).or_insert(row);
    }
    let mut out = BTreeMap::new();
    for (name, sec) in weapons.iter() {
        if let Some(n) = index_of(name, "Weapon") {
            if let Some(r) = sec.get("AmmoName0").and_then(|a| ammo.get(&folded(a))) {
                out.insert(n, *r);
            }
        }
    }
    out
}

/// [`lob_rows`] with the FX table of the user's ROM folder (none: the fallbacks for the flight).
pub fn open_fx(vfs: &RezVfs) -> Option<ButeDoc> {
    ButeDoc::try_from_vfs(vfs, crate::shooter::ATTRIBUTES_REZ, FX_ENTRY)
        .ok()
        .flatten()
}

fn add_scaled(a: [f32; 3], d: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] + d[0] * k, a[1] + d[1] * k, a[2] + d[2] * k]
}

fn len3(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Unit forward vector of yaw / pitch (pitch negative = up), as the shooter's aim.
pub fn forward(yaw: f32, pitch: f32) -> [f32; 3] {
    crate::shooter::forward(yaw, pitch)
}

/// Where a round released at `origin` along the unit `dir` blows up: the server's flight (DSV-39) run against
/// `world` until the fuse: steps of [`FLIGHT_STEP_MS`], gravity then the segment, a hit reflects the velocity about
/// the contact normal and keeps [`BOUNCE_KEEP`] of it, the round rests below [`REST_SPEED`] with a solid just below.
pub fn blast_point(
    world: &dc_sim::CollisionWorld,
    origin: [f32; 3],
    dir: [f32; 3],
    row: &LobRow,
) -> [f32; 3] {
    let mut pos = origin;
    let mut v = [dir[0] * row.speed, dir[1] * row.speed, dir[2] * row.speed];
    let mut t = 0u32;
    let dt = FLIGHT_STEP_MS as f32 / 1000.0;
    while t < row.fuse_ms {
        t += FLIGHT_STEP_MS;
        v[1] += row.gravity * dt;
        let next = add_scaled(pos, v, dt);
        match world.ray(dc_sim::V3::from_array(pos), dc_sim::V3::from_array(next)) {
            Some(h) => {
                pos = add_scaled(pos, sub3(next, pos), h.t.clamp(0.0, 1.0));
                let n = h.normal.to_array();
                let vn = v[0] * n[0] + v[1] * n[1] + v[2] * n[2];
                v = add_scaled(v, n, -2.0 * vn);
                v = [v[0] * BOUNCE_KEEP, v[1] * BOUNCE_KEEP, v[2] * BOUNCE_KEEP];
            }
            None => pos = next,
        }
        if len3(v) < REST_SPEED {
            let below = [pos[0], pos[1] - 1.0, pos[2]];
            if world
                .ray(dc_sim::V3::from_array(pos), dc_sim::V3::from_array(below))
                .is_some()
            {
                break;
            }
        }
    }
    pos
}

/// A planned throw: the release direction (yaw, pitch), the predicted blast point and its distance from the aim
/// point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    pub yaw: f32,
    pub pitch: f32,
    pub blast: [f32; 3],
    pub miss: f32,
}

/// The pitch (yaw straight at `mark`) whose predicted blast lies nearest `mark`: a coarse scan from slightly down
/// to steeply up, then two refinements around the best.
pub fn plan(world: &dc_sim::CollisionWorld, eye: [f32; 3], mark: [f32; 3], row: &LobRow) -> Plan {
    let yaw = (mark[0] - eye[0]).atan2(mark[2] - eye[2]);
    let try_pitch = |p: f32| {
        let blast = blast_point(world, eye, forward(yaw, p), row);
        Plan {
            yaw,
            pitch: p,
            blast,
            miss: len3(sub3(blast, mark)),
        }
    };
    let mut best = try_pitch(0.2);
    let mut p = 0.1;
    while p > -1.05 {
        let c = try_pitch(p);
        if c.miss < best.miss {
            best = c;
        }
        p -= 0.1;
    }
    for step in [0.05f32, 0.025] {
        let base = best.pitch;
        for c in [try_pitch(base - step), try_pitch(base + step)] {
            if c.miss < best.miss {
                best = c;
            }
        }
    }
    best
}

/// Why a blast at `blast` is unsafe: the thrower (at `own`) or a living team mate (`mates`) too near it.
pub fn unsafe_blast(
    blast: [f32; 3],
    own: [f32; 3],
    mates: &[[f32; 3]],
    row: &LobRow,
) -> Option<&'static str> {
    if len3(sub3(blast, own)) < SELF_SHARE * row.radius {
        return Some("self");
    }
    if mates
        .iter()
        .any(|m| len3(sub3(blast, *m)) < MATE_SHARE * row.radius)
    {
        return Some("mate");
    }
    None
}

/// BOT-147: whether a walk from `pos` to `goal` (the body moving at `vel`) takes the bot into the blast of its own
/// live grenade at `blast` (radius `radius`): the goal, or the straight line to it, within [`SELF_SHARE`] radii of
/// the blast, or the body within [`APPROACH_MARGIN`] of that and its next [`APPROACH_LOOK_S`] of motion entering it
/// (a walk around a corner toward it); a bot already inside only when the goal is nearer the blast than it is
/// (walking out is fine).
pub fn walks_into(
    pos: [f32; 3],
    goal: [f32; 3],
    vel: [f32; 3],
    blast: [f32; 3],
    radius: f32,
) -> bool {
    let keep = SELF_SHARE * radius;
    let here = len3(sub3(pos, blast));
    let there = len3(sub3(goal, blast));
    if here < keep {
        return there < here;
    }
    let near = |a: [f32; 3], d: [f32; 3]| {
        let l2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        if l2 < 1e-6 {
            return len3(sub3(a, blast)) < keep;
        }
        let w = sub3(blast, a);
        let t = ((w[0] * d[0] + w[1] * d[1] + w[2] * d[2]) / l2).clamp(0.0, 1.0);
        len3(sub3(add_scaled(a, d, t), blast)) < keep
    };
    there < keep
        || near(pos, sub3(goal, pos))
        || (here < keep + APPROACH_MARGIN
            && near(
                pos,
                [
                    vel[0] * APPROACH_LOOK_S,
                    vel[1] * APPROACH_LOOK_S,
                    vel[2] * APPROACH_LOOK_S,
                ],
            ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat floor at y = 0.
    fn floor() -> dc_sim::CollisionWorld {
        dc_sim::CollisionWorld::from_parts(
            vec![dc_nav::testworld::solid_box(
                [-5000.0, -64.0, -5000.0],
                [5000.0, 0.0, 5000.0],
            )],
            &[],
        )
    }

    #[test]
    fn a_plan_lands_the_blast_near_the_mark_on_a_flat_floor() {
        let w = floor();
        let row = LobRow {
            speed: 900.0,
            ..LobRow::default()
        };
        let eye = [0.0, 66.0, 0.0];
        for d in [300.0f32, 600.0, 900.0] {
            let mark = [d * 0.6, 0.0, d * 0.8];
            let p = plan(&w, eye, mark, &row);
            assert!(
                p.miss < MISS_SHARE * row.radius,
                "{d}: miss {} at pitch {}",
                p.miss,
                p.pitch
            );
            assert!(
                p.blast[1] > -1.0 && p.blast[1] < 5.0,
                "on the floor: {:?}",
                p.blast
            );
        }
    }

    #[test]
    fn a_walk_through_the_own_blast_waits() {
        let b = [1000.0, 0.0, 0.0];
        // through it, to it, past it, away from it while inside
        let still = [0.0; 3];
        assert!(walks_into([0.0; 3], [2000.0, 0.0, 0.0], still, b, 400.0));
        assert!(walks_into([0.0; 3], [900.0, 0.0, 0.0], still, b, 400.0));
        assert!(!walks_into([0.0; 3], [0.0, 0.0, 2000.0], still, b, 400.0));
        assert!(!walks_into(
            [800.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            still,
            b,
            400.0
        ));
        assert!(walks_into(
            [800.0, 0.0, 0.0],
            [950.0, 0.0, 0.0],
            still,
            b,
            400.0
        ));
        // a goal away, but the body heading at the blast near it (a path around a corner)
        let toward = [220.0, 0.0, 0.0];
        assert!(walks_into(
            [300.0, 0.0, 0.0],
            [300.0, 0.0, -2000.0],
            toward,
            b,
            400.0
        ));
        assert!(!walks_into(
            [-500.0, 0.0, 0.0],
            [-500.0, 0.0, -2000.0],
            toward,
            b,
            400.0
        ));
    }

    #[test]
    fn the_blast_is_unsafe_near_the_thrower_or_a_mate() {
        let row = LobRow::default();
        let b = [500.0, 0.0, 0.0];
        assert_eq!(unsafe_blast(b, [0.0; 3], &[], &row), None);
        assert_eq!(unsafe_blast(b, [200.0, 0.0, 0.0], &[], &row), Some("self"));
        assert_eq!(
            unsafe_blast(b, [0.0; 3], &[[600.0, 0.0, 0.0]], &row),
            Some("mate")
        );
    }

    /// With the user's ROM: the grenade rows carry a flight and a blast (values printed, not asserted).
    #[test]
    fn rom_grenade_rows_read() {
        let Some(rom) = std::env::var_os("CA_ROM") else {
            return;
        };
        let vfs = RezVfs::new(std::path::Path::new(&rom));
        let Ok(Some(w)) = ButeDoc::try_from_vfs(
            &vfs,
            crate::shooter::ATTRIBUTES_REZ,
            crate::shooter::WEAPONS_ENTRY,
        ) else {
            return;
        };
        let fx = open_fx(&vfs);
        let rows = lob_rows(&w, fx.as_ref());
        assert!(!rows.is_empty(), "thrown weapons in the ROM table");
        for (n, r) in rows.iter().take(4) {
            eprintln!("weapon {n}: {r:?}");
        }
    }
}
