//! Thrown objects (DSV-39, a DeCombat rule): the `UP_FIRE` that releases a throw starts a flight on a ballistic
//! arc; the server creates the object on every client, moves it with gravity, bounces or stops it on the world
//! geometry of the trace world, and when the fuse runs out removes it, sends the blast effect and applies area
//! damage (full in the inner quarter of the radius, then a linear falloff) through the damage path of DSV-30.
//!
//! The client supplies only the release (`UP_THROW_HOLD` 0x4d repeats, then one `UP_FIRE` 0x82 with the throwable's
//! ids, origin and direction); trajectory, fuse and blast are server policy (GAME_MESSAGES_2009_C2S.md §4.6). The
//! wire forms are the ones the real 2009 client accepted from the reference stack (UDP_SESSION_2009_B.md §3.5,
//! GAME_MESSAGES_2009_S2C.md §8.2 rows 0x13 and the blast row); the numbers are ours or the user's ROM rows.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.6; GAME_MESSAGES_2009_S2C.md §8.2; UDP_SESSION_2009_B.md §3.5;
// CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.4 (tdm2 D9); CAPTURE_tdm4_KILL_VALIDATION.md §2.6, §3.1, §3.6
// DC-DEFAULT: DSV-39 (proposed)

use super::combat::DamageSource;
use super::{
    finite3, len3, sub3, Game, GameOut, Life, ProjectileOut, RoundPhase, WeaponLook,
    THROW_FIRE_WINDOW_MS, TODO_DAMAGE_ZONE, TODO_KILL_ZONE_BODY,
};
use crate::gs;
use crate::rom_tables::{AreaDamage, ProgDamage};
use crate::up::UpFire;
use dc_net_engine::time::{later, since};
use dc_net_engine::Millis;

/// Longest integration step of a flight.
// DC-DEFAULT: DSV-39 (proposed) 16 ms (about the 60 Hz step of the player physics, MOVEMENT_2009.md §8)
pub const FLIGHT_STEP_MS: u32 = 16;
/// Normal of the blast effect record (the reference stack's value for a blast was not reported; a floor normal).
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.3 (`normal` (0, 1, 0) on floors); UNPROVEN for the blast record
// DC-DEFAULT: DSV-39 (proposed)
pub const TODO_BLAST_NORMAL: [f32; 3] = [0.0, 1.0, 0.0];
/// The blast's line-of-sight ray starts this far above the blast point (the point lies on a surface).
// DC-DEFAULT: DSV-39 (proposed)
pub const BLAST_RAY_LIFT: f32 = 1.0;

/// Fraction of the blast radius with the full blast damage (see [`blast_factor`]).
// OBSERVED: CAPTURE_tdm4_KILL_VALIDATION.md §3.1, CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.4, CAPTURE_tdm6_VALIDATION.md
// (three 2009 blasts of the same ammunition row, damage-calibrate.md §2: victims 51.9 / 153.8 / 211.3 units from the
// blast point took the full `AreaDamage`, 0.887 and 0.705 of it; the distances use the update's position candidate,
// UNPROVEN as the position); with the ROM radius read at run time, an inner quarter at full damage and a linear ramp
// to the radius reproduces all three within 0.6 damage (the plain linear falloff missed by up to 13.7)
// DC-DEFAULT: DSV-39 (amendment, proposed; PLAUSIBLE)
pub const BLAST_INNER_FRACTION: f32 = 0.25;

/// Whether a wall of the trace world between a blast and a player stops the blast's damage (see
/// [`crate::game::ProjectileTuning::blast_los`]). Kept on (the rule since DSV-39) until the user decides: in the
/// user's rust6 round it zeroed 3 of the 8 blasts with the bot inside the radius (walls 9-80 units from the grenade;
/// the falloff would have given 10.5 / 36.2 / 43.0), and no 2009 capture shows a blast behind a wall.
// OBSERVED: capture 20261025_125432_rust6 (our dserver log `throw 1 / 3 / 13: client 1 behind a
// wall`, blast records seq 8153 / 10622 / 55226); ae-triage-reddot2-blast §2
// DC-DEFAULT: DSV-92 (proposed; project decision pending V-BLAST-LOS)
pub const BLAST_LOS: bool = true;

/// The surface byte of a launcher's shot record ([`Game::launch_effect`]): the sky value, for which the client draws no
/// impact.
// DISASM: cshell+0x2c8dbb (surface 0x6e skips the impact effect, mark, surface effect and sound unless the impact row
// asks for sky impacts); EVIDENCE: cshell+0x23a910 (a miss's record carries 0x6e)
pub const LAUNCH_SURFACE: u8 = 0x6e;

/// One burn step: a burning player takes `per_second x 0.25` every 250 ms.
// DC-DEFAULT: DSV-455 (proposed) 250 ms (four damage messages a second, each a visible amount)
pub const BURN_STEP_MS: u32 = 250;

/// A burning player (DSV-455): who set him burning with which weapon, the damage per second and its type, the end of
/// the burn and the next step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Burn {
    pub victim: u8,
    pub owner: u8,
    pub weapon: u16,
    pub per_second: f32,
    pub damage_type: Option<u8>,
    pub until: Millis,
    pub next: Millis,
}

/// The share of a blast's full damage at `distance` from its centre: 1 within `inner` x `radius`, then falling
/// linearly to 0 at `radius`; 0 at or beyond the radius and for a radius that is not positive.
// DC-DEFAULT: DSV-39 (amendment, proposed)
pub fn blast_factor(distance: f32, radius: f32, inner: f32) -> f32 {
    if !(radius.is_finite() && radius > 0.0 && distance.is_finite()) || distance >= radius {
        return 0.0;
    }
    let r0 = radius * inner.clamp(0.0, 1.0);
    if distance <= r0 {
        return 1.0;
    }
    ((radius - distance) / (radius - r0)).clamp(0.0, 1.0)
}

/// One thrown object in flight.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Flight {
    pub key: u32,
    pub owner: u8,
    pub owner_obj: u16,
    pub owner_team: u8,
    pub weapon: u16,
    pub ammo: u16,
    /// The release's origin (the `from` of the blast record).
    pub origin: [f32; 3],
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    /// Acceleration along +Y.
    pub gravity: f32,
    pub detonate_at: Millis,
    /// Time the flight was simulated up to.
    pub last: Millis,
    pub resting: bool,
    /// A fired projectile (a rocket): it blows up where it first meets the world or a player (DSV-181).
    pub impact: bool,
    /// An impact flight met something: it blows up in this tick.
    pub burst: bool,
}

/// The fraction along `a` -> `b` where the segment enters the box centred at `c` with half extents `h`, if it does.
fn segment_box(a: [f32; 3], b: [f32; 3], c: [f32; 3], h: [f32; 3]) -> Option<f32> {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for i in 0..3 {
        let d = b[i] - a[i];
        let (lo, hi) = (c[i] - h[i], c[i] + h[i]);
        if d.abs() < 1e-9 {
            if a[i] < lo || a[i] > hi {
                return None;
            }
            continue;
        }
        let (mut e, mut x) = ((lo - a[i]) / d, (hi - a[i]) / d);
        if e > x {
            std::mem::swap(&mut e, &mut x);
        }
        t0 = t0.max(e);
        t1 = t1.min(x);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

fn add_scaled(a: [f32; 3], d: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] + d[0] * k, a[1] + d[1] * k, a[2] + d[2] * k]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = len3(a);
    (l.is_finite() && l > 1e-6).then(|| [a[0] / l, a[1] / l, a[2] / l])
}

impl Game {
    /// Whether `UP_FIRE` of `pid` is a throw's release (within the window since the last hold); consumes the hold.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (the release is the 0x82 sent 0.2 s after the last 0x4d)
    // DC-DEFAULT: DSV-29 (proposed) window
    pub(super) fn take_throw(&mut self, pid: u8, now: Millis) -> bool {
        let Some(p) = self.players.get_mut(&pid) else {
            return false;
        };
        let throw = p
            .last_throw
            .is_some_and(|t| since(now, t) <= THROW_FIRE_WINDOW_MS);
        if throw {
            p.last_throw = None;
        }
        throw
    }

    /// The throw sound for everyone, the thrower included, unreliable (instead of a shot effect).
    // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.4; CAPTURE_tdm4_KILL_VALIDATION.md §3.6 (type 0x19 `k` 0x12;
    // REFERENCE, ACCEPTED); GAME_MESSAGES_2009_S2C.md §8.2
    // DC-DEFAULT: CV2-3 (proposed)
    pub(super) fn throw_sound(&mut self, pid: u8, f: &UpFire) {
        let team = self.players.get(&pid).map_or(1, |p| p.team);
        let (a, b, c) = super::throw_sound_bytes(team);
        let m = gs::throw_sound(a, u32::from(pid), f.origin, b, c);
        for to in self.introduced() {
            self.out.push(GameOut::Unreliable(to, m.clone()));
        }
    }

    /// The model and skin of a thrown object: the ammunition's projectile row (`Model`, `Skin`), else the weapon's
    /// third-person model with its first skin; no render style (the reference stack's create had none).
    // OBSERVED: ROM table key FX.TXT [ProjectileFX<k>] Model, Skin; WEAPONS.TXT [Weapon<n>] HHModel, HHSkin0
    // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.4 (one texture, no style; REFERENCE, ACCEPTED)
    fn projectile_look(&self, weapon: u16, ammo: u16) -> Option<WeaponLook> {
        let row = self
            .tables
            .weapons
            .projectile_fx(weapon, ammo)
            .and_then(|n| self.tables.projectiles.get(n));
        if let Some(model) = row.and_then(|r| r.model.clone()) {
            return Some(WeaponLook {
                weapon,
                model,
                textures: row.and_then(|r| r.skin.clone()).into_iter().collect(),
                styles: Vec::new(),
            });
        }
        let mut hh = self.weapon_look(weapon)?;
        hh.textures.truncate(1);
        hh.styles.clear();
        Some(hh)
    }

    /// The release of a throw: launch speed, gravity and fuse from the ammunition's projectile row (`Velocity`,
    /// `Gravity` / `GravityOverride`, `Lifetime`), else the DSV-39 defaults; the flight starts at the release's
    /// origin along its direction and the object is created on every client in the world.
    // OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] ProjectileFXName; FX.TXT [ProjectileFX<k>] Velocity, Gravity,
    // GravityOverride, Lifetime, ModelScale
    // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.4 (the create 31 ms after the 0x82, `pos48` with a velocity;
    // removal and blast 2.5 s later; REFERENCE, ACCEPTED)
    // DC-DEFAULT: DSV-39 (proposed)
    pub(super) fn throw_release(&mut self, pid: u8, f: &UpFire, now: Millis) -> bool {
        self.launch(pid, f, now, false)
    }

    /// A launcher's shot (ammunition `Type` 1, [`crate::rom_tables::WeaponTable::fires_projectile`]): the client
    /// sends it without a hit, so the server flies the rocket from the shot's origin along its direction with the
    /// projectile row's speed, gravity and lifetime, and blows it up where it first meets the world or a player
    /// (or at the end of its lifetime) with the ammunition row's area damage, as a grenade's blast.
    // OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] Type, ProjectileFXName (LAW: Missile); FX.TXT [ProjectileFX<k>]
    // Class, Velocity, Gravity, Lifetime (Missile: CProjectile, 1300, 0, 10; values read at run time)
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-3.4 (the client traces only the instant kind; a launcher's 0x82
    // carries no hit)
    // DC-DEFAULT: DSV-181 (proposed) the server's rocket flight and impact blast
    pub(super) fn rocket_release(&mut self, pid: u8, f: &UpFire, now: Millis) -> bool {
        self.launch(pid, f, now, true)
    }

    fn launch(&mut self, pid: u8, f: &UpFire, now: Millis, impact: bool) -> bool {
        let Some(p) = self.players.get(&pid) else {
            return false;
        };
        let (owner_obj, owner_team, yaw, at) = (p.obj, p.team, p.yaw, p.pos);
        // DC-DEFAULT: DC-C2S-11 (proposed) the origin tolerance of a shot applies to a throw
        let Some(dir) = unit(f.dir)
            .filter(|_| finite3(f.origin) && len3(sub3(f.origin, at)) <= self.cfg.origin_tolerance)
        else {
            self.log(format!(
                "client {pid}: throw refused (bad direction or origin away from the player)"
            ));
            return false;
        };
        let pt = self.cfg.projectile;
        let row = self
            .tables
            .weapons
            .projectile_fx(f.weapon_id, f.ammo_id)
            .and_then(|n| self.tables.projectiles.get(n))
            .cloned();
        let speed = row
            .as_ref()
            .and_then(|r| r.velocity)
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(pt.speed);
        let gravity = match &row {
            Some(r) if r.gravity == Some(false) => 0.0,
            Some(r) => r
                .gravity_override
                .filter(|g| g.is_finite() && *g < 0.0)
                .unwrap_or(pt.gravity),
            None => pt.gravity,
        };
        let fuse_ms = row
            .as_ref()
            .and_then(|r| r.lifetime_s)
            .filter(|s| s.is_finite() && *s > 0.0 && *s < 600.0)
            .map_or(pt.fuse_ms, |s| (s * 1000.0).round() as u32);
        let scale = row.as_ref().and_then(|r| r.scale).unwrap_or([1.0; 3]);
        let vel = [dir[0] * speed, dir[1] * speed, dir[2] * speed];
        let key = self.next_flight;
        self.next_flight = self.next_flight.wrapping_add(1).max(1);
        self.flights.push(Flight {
            key,
            owner: pid,
            owner_obj,
            owner_team,
            weapon: f.weapon_id,
            ammo: f.ammo_id,
            origin: f.origin,
            pos: f.origin,
            vel,
            gravity,
            detonate_at: now.wrapping_add(fuse_ms),
            last: now,
            resting: false,
            impact,
            burst: false,
        });
        let look = self.projectile_look(f.weapon_id, f.ammo_id);
        if look.is_none() {
            self.log(format!(
                "client {pid}: throw of weapon {} has no model in the ROM tables: no object, blast only",
                f.weapon_id
            ));
        }
        self.out.push(GameOut::Projectile(Box::new(ProjectileOut {
            key,
            weapon: f.weapon_id,
            ammo: f.ammo_id,
            look,
            scale,
            pos: f.origin,
            vel,
            // a launched round faces along its flight (the client starts the round's effect on the object, in the
            // object's frame: the flame's jet points the way the object faces); a throw keeps the thrower's yaw
            // EVIDENCE: cshell+0x2cab60 (the projectile row's effect group is created on the object)
            // DC-DEFAULT: DSV-181 (amended, proposed) the launched object's rotation is the shot direction's
            rot: if impact {
                crate::synth_client::look_quat(dir)
            } else {
                crate::stream::yaw_quat(yaw)
            },
        })));
        self.log(format!(
            "client {pid}: {} {key} (weapon {} ammo {}), speed {speed:.0}, gravity {gravity:.0}, fuse {fuse_ms} ms",
            if impact { "rocket" } else { "throw" },
            f.weapon_id,
            f.ammo_id
        ));
        true
    }

    /// After a throw's release: `GS_PLAYER_STATUS` kind 1 to the thrower with the throwable's remaining count (every
    /// round it still carries, clip and stock, after the throw's round was booked), as the reference stack answered the
    /// release. A 2-pack: 1.0 after the first throw, 0.0 after the second; a single grenade: 0.0.
    // SPEC: CAPTURE_tdm4_KILL_VALIDATION.md §2.6 (the 0x82 was answered with 0x66 kind 1 `[weapon 7][ammo kind 9]
    // [ammo 0.0][slot 3][1]` to the thrower, 2.47 s before the explosion; REFERENCE); GAME_MESSAGES_2009_S2C.md §4.3
    // (kind 1 layout), §4.3.1; GAME_MESSAGES_2009_S2C_addendum_blast_questions.md Q9
    // OBSERVED: capture 20261025_095358_rust4 (item 143, a 2-pack: after the first release the
    // client reloaded from its stock, 0x83 [143] in the release block and 0x76 [3][1][1]; our 0x66 kind 1 with the
    // clip mirror 0.0 left it with an empty hand: no wind-up sound, no second release); reference tdm4 / tdm6 (weapon 7:
    // grant 1.0, after the throw 0.0 = grant minus thrown; REFERENCE)
    // DC-DEFAULT: DC-C2S-4 (proposed) the clip mirror; DSV-39 (amended, proposed) the count is clip + stock and the
    // throw costs one clip round
    pub(super) fn throw_count(&mut self, pid: u8, wi: usize) {
        let Some(w) = self.players.get(&pid).and_then(|p| p.weapons.get(wi)) else {
            return;
        };
        let m = gs::player_ammo(
            w.weapon,
            w.ammo,
            w.carried() as f32,
            w.slot,
            gs::AMMO_EXTRA_THROW,
        );
        self.send(pid, m);
    }

    /// Stream entries of every thrown object from its create to its removal (`-projectile-stream`, on by
    /// default): `(key, position, velocity)` in every tick; a resting object keeps its entry with a zero velocity
    /// (the stream writes it without the velocity field) until the blast removes it.
    // SPEC: UDP_SESSION_2009_B.md §3.5.4 (Q6 answered: the reference stack streamed the object in every stream message,
    // `uf` 4, `pos48` with the velocity while it moves and without it at rest, until the removal); §4.3 (entry
    // form, as a player's without rotation and animation)
    // OBSERVED: capture 20261025_095358_rust4 (no entries: the user saw no throw, only the
    // blast; the 2009 client does not move a type-0x13 object by itself, V-PROJ-1); reference tdm4 / tdm6 captures (64-74
    // entries per throw, 58 of tdm4's at rest)
    // DC-DEFAULT: DSV-41 (amended, proposed) on by default, every tick
    pub fn flight_stream(&self) -> Vec<(u32, [f32; 3], [f32; 3])> {
        if !self.cfg.projectile_stream {
            return Vec::new();
        }
        self.flights
            .iter()
            .map(|fl| {
                let vel = if fl.resting { [0.0; 3] } else { fl.vel };
                (fl.key, fl.pos, vel)
            })
            .collect()
    }

    /// First solid of the trace world on the segment `from` -> `to`: the free fraction and the surface normal.
    // SPEC: MOVEMENT_2009.md §9.6 (the sweep of dc-sim; a zero-size shape is a ray)
    // DC-DEFAULT: DSV-39 (proposed) thrown objects use the level geometry of DSV-34
    pub(super) fn projectile_ray(&self, from: [f32; 3], to: [f32; 3]) -> Option<(f32, [f32; 3])> {
        let w = self.trace.as_ref()?;
        let h = w.ray(dc_sim::V3::from_array(from), dc_sim::V3::from_array(to))?;
        Some((h.t.clamp(0.0, 1.0), h.normal.to_array()))
    }

    /// Moves a flight up to `until` in steps of at most [`FLIGHT_STEP_MS`]: gravity, then the segment against the
    /// trace world (a hit reflects the velocity about the surface normal and keeps `bounce` of it; below
    /// `stop_speed` the object rests) and the world box (it rests on the box's floor and does not leave the box).
    /// Returns the bounces off the trace world that hit with at least `stop_speed` into the surface: the point and
    /// the unit direction of travel into it (their impact sounds, DSV-64).
    // DC-DEFAULT: DSV-39 (proposed); DSV-64 (proposed) the bounces that sound
    fn fly(&self, fl: &mut Flight, until: Millis) -> Vec<([f32; 3], [f32; 3])> {
        let pt = self.cfg.projectile;
        let mut bounces = Vec::new();
        while !fl.resting && later(until, fl.last) {
            let step = since(until, fl.last).min(FLIGHT_STEP_MS);
            let dt = step as f32 / 1000.0;
            fl.last = fl.last.wrapping_add(step);
            let mut v = fl.vel;
            v[1] += fl.gravity * dt;
            let next = add_scaled(fl.pos, v, dt);
            if fl.impact {
                // the first player box or solid on this step's segment stops the rocket there
                let world = self.projectile_ray(fl.pos, next).map(|(t, _)| t);
                let body = self
                    .players
                    .values()
                    .filter(|p| p.pid != fl.owner && p.life == Life::Alive && p.introduced)
                    .filter_map(|p| segment_box(fl.pos, next, p.pos, self.hit_dims_of(p.pid)))
                    .fold(None, |m: Option<f32>, t| Some(m.map_or(t, |m| m.min(t))));
                let first = match (world, body) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                fl.vel = v;
                match first {
                    Some(t) => {
                        fl.pos = add_scaled(fl.pos, sub3(next, fl.pos), t);
                        fl.burst = true;
                    }
                    None => fl.pos = next,
                }
                let (lo, hi) = (self.pbox.min, self.pbox.max);
                if (0..3).any(|i| fl.pos[i] < lo[i] || fl.pos[i] > hi[i]) {
                    // out of the world box: it blows up at the edge
                    for i in 0..3 {
                        fl.pos[i] = fl.pos[i].clamp(lo[i], hi[i]);
                    }
                    fl.burst = true;
                }
                if fl.burst {
                    fl.resting = true;
                }
                continue;
            }
            match self.projectile_ray(fl.pos, next) {
                Some((t, n)) => {
                    fl.pos = add_scaled(fl.pos, sub3(next, fl.pos), t);
                    let vn = dot3(v, n);
                    let speed = len3(v);
                    if -vn >= pt.stop_speed && speed > 0.0 {
                        bounces.push((fl.pos, [v[0] / speed, v[1] / speed, v[2] / speed]));
                    }
                    v = add_scaled(v, n, -2.0 * vn);
                    v = [v[0] * pt.bounce, v[1] * pt.bounce, v[2] * pt.bounce];
                }
                None => fl.pos = next,
            }
            let (lo, hi) = (self.pbox.min, self.pbox.max);
            for (i, (p, vi)) in fl.pos.iter_mut().zip(v.iter_mut()).enumerate() {
                if *p < lo[i] || *p > hi[i] {
                    *p = p.clamp(lo[i], hi[i]);
                    *vi = if i == 1 { -*vi * pt.bounce } else { 0.0 };
                }
            }
            fl.vel = v;
            if len3(v) < pt.stop_speed {
                // a slow object rests only on something: the box floor or a solid just below it
                let below = [fl.pos[0], fl.pos[1] - 1.0, fl.pos[2]];
                if fl.pos[1] <= self.pbox.min[1] || self.projectile_ray(fl.pos, below).is_some() {
                    fl.vel = [0.0; 3];
                    fl.resting = true;
                }
            }
        }
        if fl.resting {
            fl.last = until;
        }
        bounces
    }

    /// Moves every flight; a flight whose fuse ran out blows up. Flights end without a blast when the round is
    /// not running.
    // DC-DEFAULT: DSV-39 (proposed)
    pub(super) fn tick_projectiles(&mut self, now: Millis) {
        if self.flights.is_empty() {
            return;
        }
        let mut flights = std::mem::take(&mut self.flights);
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            for fl in flights {
                self.out.push(GameOut::ProjectileGone { key: fl.key });
            }
            return;
        }
        let mut keep = Vec::with_capacity(flights.len());
        for mut fl in flights.drain(..) {
            let fused = !later(fl.detonate_at, now);
            let until = if fused { fl.detonate_at } else { now };
            let bounces = self.fly(&mut fl, until);
            let due = fused || fl.burst;
            for (at, dir) in bounces {
                // the surface word of the polygon the object hit (V-SURF-1), its grenade impact sound
                let s = self
                    .surfaces
                    .as_ref()
                    .and_then(|p| p.at_shot_end(at, dir))
                    .unwrap_or_else(|| self.world_surface_below(at));
                self.bounce_sound(at, s);
            }
            if due {
                self.detonate(&fl, now);
            } else {
                keep.push(fl);
            }
        }
        // a blast may have ended the round; flights added meanwhile are kept
        keep.append(&mut self.flights);
        self.flights = keep;
    }

    /// The blast: removal of the object, the blast effect (type 0x06 with no hit object, the thrower's character,
    /// the throw's ids, from the release origin to the blast point) to everyone, and area damage to every living
    /// player within the radius whose centre the trace world does not hide: `damage x` [`blast_factor`] (full damage
    /// in the inner quarter of the radius, then linear to 0 at the radius)
    /// (ammunition row `AreaDamage` / `AreaDamageRadius`, else the DSV-39 defaults), damage type `AreaDamageType`,
    /// the thrower included, team mates only with the room's friendly fire.
    // SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (the blast row: type 0x06 with `first` 0xFFFF, the throw's weapon and
    // ammunition, `to` = the blast point; plus 0x74 / 0x66 to the players in range; REFERENCE, ACCEPTED);
    // CAPTURE_tdm4_KILL_VALIDATION.md §3.1 items 3, 4 (0x74 `zone` = AreaDamageType, `damage` = AreaDamage at the
    // centre; REFERENCE); CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.4 (removal record, then the blast in the same tick)
    // OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] AreaDamage, AreaDamageType, AreaDamageRadius
    // DC-DEFAULT: DSV-39 (proposed) line of sight, self damage, falloff amended (inner quarter full); DSV-30 (proposed)
    // damage path
    fn detonate(&mut self, fl: &Flight, now: Millis) {
        self.out.push(GameOut::ProjectileGone { key: fl.key });
        // DC-DEFAULT: DSV-455 (proposed) a flame bursts without a blast: it sets the players near it burning
        if let Some(prog) = self.tables.weapons.prog(fl.weapon, fl.ammo) {
            self.flame_burst(fl, prog, now);
            return;
        }
        let pt = self.cfg.projectile;
        let area = self
            .tables
            .weapons
            .area(fl.weapon, fl.ammo)
            .unwrap_or(AreaDamage {
                damage: pt.area_damage,
                damage_type: None,
                radius: pt.area_radius,
            });
        let m = gs::shot_effect(&gs::ShotEffect {
            first: None,
            shooter: fl.owner_obj,
            weapon: fl.weapon,
            ammo: fl.ammo,
            from: fl.origin,
            to: fl.pos,
            normal: TODO_BLAST_NORMAL,
            // the surface below the blast point (V-SURF-1)
            // OBSERVED: capture 20261025_065016_tdm6 c6 seq 9727 / 20355 (`a` 10 on JUNK_FLEA),
            // 20261025_002236_tdm4 c6 seq 8065 and 20261025_001409_tdm2 c6 seq 8517 (`a` 71 on SNOWVALLEY): each the
            // surface word of the polygon below the blast point; REFERENCE
            // DC-DEFAULT: DSV-62 (proposed)
            surface: self.world_surface_below(fl.pos),
            mask: gs::SHOT_MASK_MISS,
            // the thrower's client id (a thrower who left still has his id: no client of the match has it now)
            // DC-DEFAULT: DSV-85 (proposed)
            client: fl.owner,
            mods: gs::NO_SHOT_MODS,
        });
        for to in self.introduced() {
            self.out.push(GameOut::Unreliable(to, m.clone()));
        }
        let src = DamageSource {
            weapon: fl.weapon,
            dtype: area.damage_type.unwrap_or(TODO_DAMAGE_ZONE),
        };
        let killer = self.players.contains_key(&fl.owner).then_some(fl.owner);
        let lifted = [fl.pos[0], fl.pos[1] + BLAST_RAY_LIFT, fl.pos[2]];
        // DC-DEFAULT: DSV-390 (amended; its spy clause replaced by DSV-450) the thrower's team at the blast, not at
        // the throw (left the match: the team of the throw)
        let owner_team = self.players.get(&fl.owner).map_or(fl.owner_team, |p| p.team);
        let victims: Vec<(u8, [f32; 3])> = self
            .players
            .values()
            .filter(|v| v.life == Life::Alive && v.introduced)
            .filter(|v| v.pid == fl.owner || self.team_may_hurt(owner_team, v.team))
            .map(|v| (v.pid, v.pos))
            .collect();
        let mut hit = 0;
        for (vpid, vpos) in victims {
            let d = len3(sub3(vpos, fl.pos));
            if !(d.is_finite() && d < area.radius) {
                continue;
            }
            // a player inside its spawn window takes no blast damage either (no 2009 capture has a blast inside a
            // spawn window; the 2009 shield is the shooter's hit test, which a server-side blast never runs)
            // DC-DEFAULT: DSV-166 (proposed) the spawn shield (DSV-156) covers area damage
            if self.spawn_shielded(vpid) {
                self.log(format!("throw {}: client {vpid} spawn-shielded", fl.key));
                continue;
            }
            // DC-DEFAULT: DSV-92 (proposed) the line-of-sight rule is switchable (`-combat blast_los=on|off`)
            if pt.blast_los && self.projectile_ray(lifted, vpos).is_some() {
                self.log(format!("throw {}: client {vpid} behind a wall", fl.key));
                continue;
            }
            // DC-DEFAULT: DSV-165 (gameplay choice, user-tunable) the room's blast damage scale; DSV-456 the agent's
            let dmg = area.damage
                * blast_factor(d, area.radius, pt.inner_fraction)
                * pt.blast_damage_scale
                * self.agent_damage_scale(fl.owner);
            if !(dmg.is_finite() && dmg > 0.0) {
                continue;
            }
            // the round may have ended on an earlier victim of this blast
            if !matches!(self.phase, RoundPhase::Running { .. }) {
                break;
            }
            if self.players.get(&vpid).map(|v| v.life) != Some(Life::Alive) {
                continue;
            }
            self.log(format!(
                "throw {}: client {vpid} takes {dmg:.1} at {d:.0} units (x {})",
                fl.key, pt.blast_damage_scale
            ));
            let dir = unit(sub3(vpos, fl.pos)).unwrap_or([0.0, 1.0, 0.0]);
            self.apply_damage(vpid, killer, dmg, dir, src, TODO_KILL_ZONE_BODY, now);
            hit += 1;
        }
        self.log(format!(
            "throw {} blew up at ({:.0}, {:.0}, {:.0}): {hit} player(s) hit within {:.0}",
            fl.key, fl.pos[0], fl.pos[1], fl.pos[2], area.radius
        ));
        // DC-DEFAULT: DSV-119 (proposed) a blast also damages the breakables in its radius
        self.blast_breakables(
            fl.pos,
            area.radius,
            area.damage,
            pt.inner_fraction,
            killer,
            fl.weapon,
            now,
        );
    }

    /// The shot record of a launcher's `UP_FIRE` (a round the server flies, [`Game::rocket_release`]): no hit object, a
    /// one-unit path from the muzzle point along the shot, the sky surface, the miss mask. The other clients play the
    /// shooter's fire sound from it; the short path draws no tracer and the sky surface no impact, so no explosion
    /// or blood is drawn where the shooter aimed before the round gets there (its burst is the blast record).
    // EVIDENCE: cshell+0x23c041..0x23c064 with the table at cshell+0x23c298 (the 2009 client traces no pellet of a
    // `Type` 1, 2 or 5 round: the launcher's 0x82 carries no hit); DISASM: cshell+0x2c8dbb (surface 0x6e skips the
    // impact effect, mark, surface effect and sound), cshell+0x2c6160 (no tracer for a path of 100 units or less)
    // DC-DEFAULT: DSV-181 (amended, proposed) the launcher's shot record (before: the record of the client's traced
    // hit, which drew an impact or blood at the aim point with no damage)
    pub(super) fn launch_effect(&mut self, pid: u8, f: &UpFire) {
        let Some(shooter) = self.players.get(&pid).map(|p| p.obj) else {
            return;
        };
        let Some(dir) = unit(f.dir) else {
            return;
        };
        let from = if finite3(f.weapon_pos) {
            f.weapon_pos
        } else {
            f.origin
        };
        let mods = self.shot_mods_of(pid, f.weapon_id);
        let m = gs::shot_effect(&gs::ShotEffect {
            first: None,
            shooter,
            weapon: f.weapon_id,
            ammo: f.ammo_id,
            from,
            to: add_scaled(from, dir, 1.0),
            normal: [-dir[0], -dir[1], -dir[2]],
            surface: LAUNCH_SURFACE,
            mask: gs::SHOT_MASK_MISS,
            client: pid,
            mods,
        });
        for to in self.introduced() {
            self.out.push(GameOut::Unreliable(to, m.clone()));
        }
    }

    /// A flame's burst: every living, introduced player other than the shooter within the row's `ProgDamageRadius`
    /// of the burst point (not hidden by the level geometry, as a blast; team mates only with the room's friendly
    /// fire; not inside his spawn window) starts burning, or burns on: [`Burn`] until the burst plus
    /// `ProgDamageDuration`. No blast record, no area damage.
    // OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] ProgDamage, ProgDamageType, ProgDamageDuration, ProgDamageRadius
    // DC-DEFAULT: DSV-455 (proposed)
    fn flame_burst(&mut self, fl: &Flight, prog: ProgDamage, now: Millis) {
        let owner_team = self.players.get(&fl.owner).map_or(fl.owner_team, |p| p.team);
        let lifted = [fl.pos[0], fl.pos[1] + BLAST_RAY_LIFT, fl.pos[2]];
        let blast_los = self.cfg.projectile.blast_los;
        let near: Vec<(u8, [f32; 3])> = self
            .players
            .values()
            .filter(|v| v.life == Life::Alive && v.introduced && v.pid != fl.owner)
            .filter(|v| self.team_may_hurt(owner_team, v.team))
            .filter(|v| {
                let d = len3(sub3(v.pos, fl.pos));
                d.is_finite() && d <= prog.radius
            })
            .map(|v| (v.pid, v.pos))
            .collect();
        let until = now.wrapping_add((prog.duration_s * 1000.0).round().max(1.0) as u32);
        for (vpid, vpos) in near {
            if self.spawn_shielded(vpid) {
                continue;
            }
            if blast_los && self.projectile_ray(lifted, vpos).is_some() {
                self.log(format!("flame {}: client {vpid} behind a wall", fl.key));
                continue;
            }
            match self.burns.iter_mut().find(|b| b.victim == vpid) {
                Some(b) => {
                    b.until = until;
                    b.owner = fl.owner;
                    b.weapon = fl.weapon;
                    b.per_second = prog.per_second;
                    b.damage_type = prog.damage_type;
                }
                None => self.burns.push(Burn {
                    victim: vpid,
                    owner: fl.owner,
                    weapon: fl.weapon,
                    per_second: prog.per_second,
                    damage_type: prog.damage_type,
                    until,
                    next: now.wrapping_add(BURN_STEP_MS),
                }),
            }
            self.log(format!(
                "flame {}: client {vpid} burns ({:.0} per second until {until})",
                fl.key, prog.per_second
            ));
        }
    }

    /// The burns: each step ([`BURN_STEP_MS`]) up to the burn's end deals `per_second x` the step's seconds through
    /// the damage path of DSV-30 (the burner credited); a burn ends after its end, at its victim's death or when the
    /// round is not running.
    // DC-DEFAULT: DSV-455 (proposed)
    pub(super) fn tick_burns(&mut self, now: Millis) {
        if self.burns.is_empty() {
            return;
        }
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            self.burns.clear();
            return;
        }
        let mut i = 0;
        while i < self.burns.len() {
            let b = self.burns[i];
            let alive = self.players.get(&b.victim).map(|v| v.life) == Some(Life::Alive);
            if !alive || later(b.next, b.until) {
                self.burns.remove(i);
                continue;
            }
            if later(b.next, now) {
                i += 1;
                continue;
            }
            self.burns[i].next = b.next.wrapping_add(BURN_STEP_MS);
            let dmg = b.per_second * BURN_STEP_MS as f32 / 1000.0 * self.agent_damage_scale(b.owner);
            let killer = self.players.contains_key(&b.owner).then_some(b.owner);
            let src = DamageSource {
                weapon: b.weapon,
                dtype: b.damage_type.unwrap_or(TODO_DAMAGE_ZONE),
            };
            self.apply_damage(
                b.victim,
                killer,
                dmg,
                [0.0, 1.0, 0.0],
                src,
                TODO_KILL_ZONE_BODY,
                now,
            );
            if !matches!(self.phase, RoundPhase::Running { .. }) {
                self.burns.clear();
                return;
            }
        }
    }

    /// The damage multiplier of a round `owner` fired: `spy.agent_damage_scale` while he is the Spy Hunt agent,
    /// else 1 (DSV-456).
    fn agent_damage_scale(&self, owner: u8) -> f32 {
        if self.spy.agent == Some(owner) {
            self.cfg.agent_damage_scale
        } else {
            1.0
        }
    }

    /// Players burning now (tests and tools).
    pub fn burning(&self) -> Vec<u8> {
        self.burns.iter().map(|b| b.victim).collect()
    }

    /// Thrown objects in flight (tests and tools).
    pub fn flights_in_air(&self) -> usize {
        self.flights.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // made-up blast: 80 at the centre, radius 1000
    #[test]
    fn blast_factor_shape() {
        let f = |d: f32| blast_factor(d, 1000.0, BLAST_INNER_FRACTION);
        assert_eq!(f(0.0), 1.0);
        assert_eq!(f(250.0), 1.0);
        assert!((f(625.0) - 0.5).abs() < 1e-6);
        assert_eq!(f(1000.0), 0.0);
        assert_eq!(f(1500.0), 0.0);
        // the plain linear falloff with inner 0
        assert!((blast_factor(400.0, 1000.0, 0.0) - 0.6).abs() < 1e-6);
        assert_eq!(blast_factor(10.0, 0.0, 0.25), 0.0);
        assert_eq!(blast_factor(f32::NAN, 1000.0, 0.25), 0.0);
    }

    /// The three captured 2009 blasts against the ROM row of the thrown ammunition (`AreaDamage`,
    /// `AreaDamageRadius` read at run time; skipped without the ROM): one test per sample.
    // OBSERVED: damage-calibrate.md §2 (tdm4 51.9 -> full, tdm2 153.8 -> 53.2, tdm6 211.3 -> 42.32 health; weapon 7
    // ammunition 9 in all three; the distances use an UNPROVEN position field)
    fn rom_blast(distance: f32) -> Option<f32> {
        let rom = dc_rez::dc_rom::test_rom()?;
        let (t, _) = crate::rom_tables::RomTables::load(&rom);
        let a = t.weapons.area(7, 9)?;
        Some(a.damage * blast_factor(distance, a.radius, BLAST_INNER_FRACTION))
    }

    /// `observed` = the health the 2009 victim lost (capture values, not ROM values).
    fn check(distance: f32, observed: f32) {
        let Some(dmg) = rom_blast(distance) else {
            println!("skipped: CA_ROM unset or no blast row");
            return;
        };
        assert!(
            (dmg - observed).abs() <= 0.6,
            "distance {distance}: ours {dmg:.2}, 2009 {observed:.2}"
        );
    }

    #[test]
    fn rom_blast_tdm4_sample_full_damage() {
        check(51.9, 60.0);
    }

    #[test]
    fn rom_blast_tdm2_sample() {
        check(153.8, 53.2);
    }

    #[test]
    fn rom_blast_tdm6_sample() {
        check(211.3, 42.32);
    }
}
