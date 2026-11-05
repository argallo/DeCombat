//! The game side of the destructible world objects (`crate::destructibles`): the breakables the session created are
//! bound to their object ids; a hit claim on one (`UP_FIRE` `hit_obj`) or a blast near one takes hit points; at 0 the
//! breakable dies: the session switches its model off on every client and creates the effects its death command
//! switches on, and each explosion object it names blows up (the blast record with the ammunition whose impact effect
//! it names, area damage to the players and to the other breakables in its radius, so barrels set each other off).
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (`hit_obj`, hit point = origin + dist x dir); GAME_MESSAGES_2009_S2C.md §8.2
// (the blast row: type 0x06 with `first` 0xFFFF, plus 0x74 / 0x66 to the players in range; REFERENCE, ACCEPTED)
// DC-DEFAULT: DSV-115..DSV-120 (proposed, staged world-objects)

use super::combat::DamageSource;
use super::{len3, sub3, Game, GameOut, Life, RoundPhase, TODO_DAMAGE_ZONE, TODO_KILL_ZONE_BODY};
use crate::destructibles::{DeathAction, DestructibleWorld};
use crate::gs;
use crate::up::UpFire;
use dc_net_engine::time::later;
use dc_net_engine::Millis;

/// A reported hit point may lie this far from the breakable's model centre (the models are crates and barrels up to
/// about 70 units across).
// DC-DEFAULT: DSV-120 (proposed) 128 units
pub const BREAK_HIT_RANGE: f32 = 128.0;
/// The explosion's line-of-sight ray starts this far above its point.
// DC-DEFAULT: DSV-119 (proposed), as DSV-39's blast
pub const EXPLOSION_RAY_LIFT: f32 = 1.0;
/// How far the server traces a shot's pellet for breakables (the weapon rows' default range is 3000,
/// cshell+0x17f7b0; the server's weapon table carries no range), and a melee swing.
// DC-DEFAULT: DSV-123 (proposed) 4096 units (the miss distance of DSV-29) and 100 units
pub const BREAK_RAY_RANGE: f32 = super::TODO_MISS_TRACE_UNITS;
pub const MELEE_BREAK_RANGE: f32 = 100.0;
/// Ammunition damage types of the knives and hands (the client plays their melee / hands impact sounds).
// EVIDENCE: cshell+0x27f320 (0xd / 0x20 melee, 0x24 hands)
pub const MELEE_DAMAGE_TYPES: [u8; 3] = [0x0d, 0x20, 0x24];
/// Up normal of the explosion's blast record (the reference stack's blasts carried (0, 1, 0)).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (blast row: `normal` exactly (0, 1, 0); REFERENCE)
pub const EXPLOSION_NORMAL: [f32; 3] = [0.0, 1.0, 0.0];

/// A breakable death the session shows on every client: the brush object `obj` switches its model off and every
/// `(key, effect name)` becomes a placed-effect object until its [`GameOut::WorldFxGone`].
#[derive(Debug, Clone, PartialEq)]
pub struct BreakOut {
    pub obj: u16,
    pub effects: Vec<(u32, Vec<u8>)>,
}

/// One bound breakable.
#[derive(Debug, Clone, PartialEq)]
pub struct Breakable {
    pub obj: u16,
    pub name: Vec<u8>,
    /// The model centre (the brush create's position).
    pub pos: [f32; 3],
    pub hit_points: f32,
    pub left: f32,
    pub dead: bool,
    death: Vec<DeathAction>,
}

/// The destructible state of a match.
#[derive(Debug, Clone, Default)]
pub(crate) struct DestructState {
    pub world: DestructibleWorld,
    pub items: Vec<Breakable>,
    /// Effect objects on the clients and when they go.
    fx: Vec<(u32, Millis)>,
    next_key: u32,
}

impl DestructState {
    pub fn new(world: DestructibleWorld) -> DestructState {
        DestructState {
            world,
            next_key: 1,
            ..DestructState::default()
        }
    }
}

impl Game {
    /// Binds the breakables the session created (object id, brush name, model centre) to the world's destructible
    /// data; brushes without hit points stay plain.
    // DC-DEFAULT: DSV-115 (proposed)
    pub fn bind_breakables(&mut self, created: &[(u16, Vec<u8>, [f32; 3])]) {
        let mut items = Vec::new();
        for (obj, name, pos) in created {
            let Some(b) = self.destruct.world.breakable(name) else {
                continue;
            };
            items.push(Breakable {
                obj: *obj,
                name: b.name.clone(),
                pos: *pos,
                hit_points: b.hit_points,
                left: b.hit_points,
                dead: false,
                death: b.death.clone(),
            });
        }
        let n = items.len();
        self.destruct.items = items;
        if n > 0 {
            self.log(format!(
                "destructibles: {n} breakable(s), {} explosion object(s)",
                self.destruct.world.explosions.len()
            ));
        }
    }

    /// The bound breakables (tests and tools).
    pub fn breakables(&self) -> &[Breakable] {
        &self.destruct.items
    }

    /// A hit claim of `pid` on the object `obj`: true when `obj` is a bound breakable (the claim is then answered here:
    /// its hit points drop by the weapon's ROM damage when the reported point is at the model and no wall of the
    /// trace world was in the way), false for any other object.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (`hit_obj`, `hit_dist`)
    // DC-DEFAULT: DSV-115 (proposed) the weapon's InstDamage, no zone factor; DSV-120 (proposed) the point check
    pub(super) fn hit_breakable(
        &mut self,
        pid: u8,
        obj: u16,
        f: &UpFire,
        dist: f32,
        wall: bool,
        now: Millis,
    ) -> bool {
        let Some(i) = self.destruct.items.iter().position(|b| b.obj == obj) else {
            return false;
        };
        if !self.cfg.destruct.enabled || self.destruct.items[i].dead || wall {
            return true;
        }
        let dl = len3(f.dir);
        if !(dl.is_finite() && dl > 1e-6 && dist.is_finite() && dist >= 0.0) {
            return true;
        }
        let hit = [0, 1, 2].map(|k| f.origin[k] + dist * f.dir[k] / dl);
        if len3(sub3(hit, self.destruct.items[i].pos)) > BREAK_HIT_RANGE {
            return true;
        }
        let shooter = self.players.get(&pid).map(|p| p.pos);
        if shooter.is_some_and(|s| len3(sub3(f.origin, s)) > self.cfg.origin_tolerance) {
            return true;
        }
        let dmg = self.hit_damage(pid, f);
        self.damage_breakable(i, dmg, Some(pid), f.weapon_id, now);
        true
    }

    /// The first living bound breakable a pellet's ray meets before the level's walls, within `range`: its index
    /// and the distance along the ray.
    // DC-DEFAULT: DSV-122 (proposed)
    pub(super) fn breakable_on_ray(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        range: f32,
    ) -> Option<(usize, f32)> {
        if !(range.is_finite() && range > 0.0) || !origin.iter().chain(&dir).all(|v| v.is_finite())
        {
            return None;
        }
        let limit = self.wall_along(origin, dir, range).unwrap_or(range);
        let from = dc_sim::V3::from_array(origin);
        let to = dc_sim::V3::from_array([0, 1, 2].map(|k| origin[k] + dir[k] * limit));
        self.destruct
            .items
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.dead)
            .filter_map(|(i, b)| {
                let shape = self.destruct.world.shape(&b.name)?;
                let h = shape.ray(from, to)?;
                Some((i, h.t.clamp(0.0, 1.0) * limit))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The server's own trace of a pellet that names no hit object: the 2009 client reports a hit on a world model
    /// (a crate, a drum) without an object (its trace returns the model's polygon, and only polygon-less hits on
    /// characters fill `hit_obj`), so the pellet's ray (`origin`, `dir`) is traced here against the living bound
    /// breakables; the first one before the level's walls takes the weapon's damage. A melee swing (a weapon whose
    /// ammunition's damage type is one of [`MELEE_DAMAGE_TYPES`]) reaches [`MELEE_BREAK_RANGE`], a shot
    /// [`BREAK_RAY_RANGE`]. Returns whether a breakable took the pellet.
    // EVIDENCE: cshell+0x238eb0 (the hit object only without a polygon and with user flag 0x01000000); DISASM:
    // engine+0x40650 (world models, kinds 2 / 9, are traced through their BSP with a polygon result)
    // OBSERVED: capture 20261025_000014_tdm1 (the real client's shots into the crates: 623-bit,
    // no object)
    // DC-DEFAULT: DSV-122 (proposed) the server traces; DSV-123 (proposed) the ranges
    pub(super) fn ray_breakables(&mut self, pid: u8, f: &UpFire, now: Millis) -> bool {
        if !self.cfg.destruct.enabled || self.destruct.items.iter().all(|b| b.dead) {
            return false;
        }
        let dl = len3(f.dir);
        if !(dl.is_finite() && dl > 1e-6) {
            return false;
        }
        let dir = [0, 1, 2].map(|k| f.dir[k] / dl);
        let shooter = self.players.get(&pid).map(|p| p.pos);
        if shooter.is_some_and(|s| len3(sub3(f.origin, s)) > self.cfg.origin_tolerance) {
            return false;
        }
        let melee = self
            .tables
            .weapons
            .damage_type(f.weapon_id, f.ammo_id)
            .is_some_and(|t| MELEE_DAMAGE_TYPES.contains(&t));
        let range = if melee {
            MELEE_BREAK_RANGE
        } else {
            BREAK_RAY_RANGE
        };
        let Some((i, _)) = self.breakable_on_ray(f.origin, dir, range) else {
            return false;
        };
        let dmg = self.hit_damage(pid, f);
        self.damage_breakable(i, dmg, Some(pid), f.weapon_id, now);
        true
    }

    /// A blast at `at` (a thrown object or an explosion object): every living breakable whose model centre lies
    /// within `radius` takes `damage` x the blast falloff x `destruct.blast_factor` (no line-of-sight test: the
    /// barrels stand in the open and their centres lie inside their own models).
    // DC-DEFAULT: DSV-119 (proposed)
    #[allow(clippy::too_many_arguments)]
    pub(super) fn blast_breakables(
        &mut self,
        at: [f32; 3],
        radius: f32,
        damage: f32,
        inner: f32,
        by: Option<u8>,
        weapon: u16,
        now: Millis,
    ) {
        if !self.cfg.destruct.enabled {
            return;
        }
        let k = self.cfg.destruct.blast_factor;
        let hits: Vec<(usize, f32)> = self
            .destruct
            .items
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.dead)
            .map(|(i, b)| {
                let d = len3(sub3(b.pos, at));
                (
                    i,
                    damage * k * super::projectile::blast_factor(d, radius, inner),
                )
            })
            .filter(|(_, d)| d.is_finite() && *d > 0.0)
            .collect();
        for (i, d) in hits {
            self.damage_breakable(i, d, by, weapon, now);
        }
    }

    /// Destroys the living bound breakable of this name (harness entry of the tests); false when there is none.
    pub fn break_named(&mut self, name: &[u8], now: Millis) -> bool {
        let Some(i) = self
            .destruct
            .items
            .iter()
            .position(|b| !b.dead && b.name.eq_ignore_ascii_case(name))
        else {
            return false;
        };
        let left = self.destruct.items[i].left;
        self.damage_breakable(i, left + 1.0, None, 0, now);
        true
    }

    fn damage_breakable(&mut self, i: usize, dmg: f32, by: Option<u8>, weapon: u16, now: Millis) {
        let b = &mut self.destruct.items[i];
        if b.dead {
            return;
        }
        b.left -= dmg;
        if b.left > 0.0 {
            return;
        }
        b.left = 0.0;
        b.dead = true;
        let (obj, death) = (b.obj, b.death.clone());
        let mut effects = Vec::new();
        let mut blasts = Vec::new();
        for a in death {
            match a {
                DeathAction::Effect(name) => {
                    let key = self.destruct.next_key;
                    self.destruct.next_key = self.destruct.next_key.wrapping_add(1).max(1);
                    let until = now.wrapping_add(self.cfg.destruct.effect_ms);
                    self.destruct.fx.push((key, until));
                    effects.push((key, name));
                }
                DeathAction::Explode(e) => blasts.push(e),
            }
        }
        let name = String::from_utf8_lossy(&self.destruct.items[i].name).into_owned();
        self.log(format!(
            "breakable {obj} ({name}) destroyed by {}: {} effect(s), {} explosion(s)",
            by.map_or("the world".to_string(), |p| format!("client {p}")),
            effects.len(),
            blasts.len()
        ));
        self.out
            .push(GameOut::WorldBreak(Box::new(BreakOut { obj, effects })));
        // DC-DEFAULT: DSV-575 (proposed) a mission world model's death command runs with the world's statements
        let script = self
            .destruct
            .world
            .breakable(&self.destruct.items[i].name)
            .map(|b| b.script.clone())
            .unwrap_or_default();
        if !script.is_empty() && self.is_coop() {
            self.out.push(GameOut::WorldScript(script));
        }
        for e in blasts {
            self.explode(e, by, weapon, now);
        }
    }

    /// An explosion object blows up: the blast record to every client (the ids of the ammunition whose impact effect
    /// the object names, when the ROM has one), area damage to the living players within its radius (full in the
    /// inner part, then linear; the line-of-sight rule of `-combat blast_los`; the breaker is the killer) and to the
    /// other breakables.
    // SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (blast row); OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] ImpactFXName,
    // AreaDamageType; the explosion objects of the ROM worlds (radius, damage at the centre, impact effect name)
    // DC-DEFAULT: DSV-119 (proposed) the thrower's rules of DSV-39 / DSV-92 for the victims
    fn explode(&mut self, e: usize, by: Option<u8>, weapon: u16, now: Millis) {
        let Some(x) = self.destruct.world.explosions.get(e).cloned() else {
            return;
        };
        let ammo = x.impact_fx.as_ref().and_then(|fx| {
            self.tables.weapons.ammo.iter().find_map(|(id, a)| {
                a.impact_fx
                    .as_ref()
                    .filter(|n| n.as_bytes().eq_ignore_ascii_case(fx))
                    .map(|_| *id)
            })
        });
        let fx_weapon = ammo.and_then(|a| {
            self.tables
                .weapons
                .weapons
                .iter()
                .find_map(|(w, r)| (r.ammo == Some(a)).then_some(*w))
        });
        let by_obj = by
            .and_then(|p| self.players.get(&p))
            .map(|p| (p.obj, p.team));
        if let (Some(a), Some(w)) = (ammo, fx_weapon) {
            let m = gs::shot_effect(&gs::ShotEffect {
                first: None,
                shooter: by_obj.map_or(0xFFFF, |o| o.0),
                weapon: w,
                ammo: a,
                from: x.pos,
                to: x.pos,
                normal: EXPLOSION_NORMAL,
                surface: self.world_surface_below(x.pos),
                mask: gs::SHOT_MASK_MISS,
                client: by.unwrap_or(gs::SHOT_CLIENT_NONE),
                mods: gs::NO_SHOT_MODS,
            });
            for to in self.introduced() {
                self.out.push(GameOut::Unreliable(to, m.clone()));
            }
        }
        let dtype = ammo
            .and_then(|a| self.tables.weapons.ammo.get(&a))
            .and_then(|r| r.area_damage_type)
            .unwrap_or(TODO_DAMAGE_ZONE);
        let src = DamageSource { weapon, dtype };
        let pt = self.cfg.projectile;
        let lifted = [x.pos[0], x.pos[1] + EXPLOSION_RAY_LIFT, x.pos[2]];
        let team = by_obj.map(|o| o.1);
        let victims: Vec<(u8, [f32; 3])> = self
            .players
            .values()
            .filter(|v| v.life == Life::Alive && v.introduced)
            .filter(|v| Some(v.pid) == by || team.is_none_or(|t| self.team_may_hurt(t, v.team)))
            .map(|v| (v.pid, v.pos))
            .collect();
        let killer = by.filter(|p| self.players.contains_key(p));
        let mut hurt = 0;
        for (vpid, vpos) in victims {
            let d = len3(sub3(vpos, x.pos));
            if !(d.is_finite() && d < x.radius) {
                continue;
            }
            if pt.blast_los {
                let blocked = self.trace.as_ref().is_some_and(|w| {
                    w.ray(dc_sim::V3::from_array(lifted), dc_sim::V3::from_array(vpos))
                        .is_some()
                });
                if blocked {
                    continue;
                }
            }
            let dmg = x.damage * super::projectile::blast_factor(d, x.radius, pt.inner_fraction);
            if !(dmg.is_finite() && dmg > 0.0) {
                continue;
            }
            if !matches!(self.phase, RoundPhase::Running { .. }) {
                break;
            }
            if self.players.get(&vpid).map(|v| v.life) != Some(Life::Alive) {
                continue;
            }
            let l = len3(sub3(vpos, x.pos));
            let dir = if l > 1e-6 {
                [0, 1, 2].map(|k| (vpos[k] - x.pos[k]) / l)
            } else {
                [0.0, 1.0, 0.0]
            };
            self.apply_damage(vpid, killer, dmg, dir, src, TODO_KILL_ZONE_BODY, now);
            hurt += 1;
        }
        let name = String::from_utf8_lossy(&x.name).into_owned();
        self.log(format!(
            "explosion {name} at ({:.0}, {:.0}, {:.0}): {hurt} player(s) within {:.0}",
            x.pos[0], x.pos[1], x.pos[2], x.radius
        ));
        self.blast_breakables(
            x.pos,
            x.radius,
            x.damage,
            pt.inner_fraction,
            by,
            weapon,
            now,
        );
    }

    /// The death effects whose time is up leave the clients.
    // DC-DEFAULT: DSV-118 (proposed)
    pub(super) fn tick_destruct(&mut self, now: Millis) {
        let mut gone = Vec::new();
        self.destruct.fx.retain(|&(key, until)| {
            if later(until, now) {
                true
            } else {
                gone.push(key);
                false
            }
        });
        for key in gone {
            self.out.push(GameOut::WorldFxGone { key });
        }
    }
}
