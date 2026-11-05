//! The co-op AI kinds' specials: a melee AI walks until a human comes within its rush distance, then runs
//! (DSV-597); what a dead AI's brain row sets off at its body: a blast (DSV-598), thrown items it drops that go off
//! when a human comes near or at their fuse (DSV-599), a cloud that hurts the humans inside it (DSV-600); and a hit on
//! an AI that wears another row's body counts by its own skeleton row (DSV-601).
//!
//! The rows are the ROM's, read at run time (`coop_rom`: the attribute row's walk speed and sight distance, the brain
//! row's death keys naming weapon rows; `rom_tables`: those weapons' blast and progressive-damage rows). How the 2009
//! server used them is not evidenced: the rules, the moments and the numbers marked DC-DEFAULT are ours, shaped by the
//! owner's notes on how the infected played (DC_DEFAULTS.md DSV-597..DSV-601).

use super::super::projectile::{Burn, Flight, BLAST_RAY_LIFT, BURN_STEP_MS, TODO_BLAST_NORMAL};
use super::super::{len3, sub3, Game, GameOut, Life};
use super::{AiSpec, AI_TEAM};
use crate::chars::HitNode;
use crate::gs;
use crate::world::AiTemplate;
use dc_net_engine::time::later;
use dc_net_engine::Millis;

/// The object id a blast record names when the dead AI's object is not bound (no object).
// DC-DEFAULT: DSV-598 (proposed)
pub const NO_OBJECT: u16 = 0xffff;
/// Launch speed and elevation of a dead AI's dropped items (spread evenly around its facing).
// DC-DEFAULT: DSV-599 (proposed) 150 u/s, 45 degrees (they land about 50 units from the body)
pub const GROWTH_TOSS: f32 = 150.0;
pub const GROWTH_TOSS_ANGLE: f32 = std::f32::consts::FRAC_PI_4;
/// How long a cloud lingers when its ammunition row has no `ProgDamageLifetime`.
// DC-DEFAULT: DSV-600 (proposed) 4000 ms
pub const CLOUD_LIFETIME_MS: u32 = 4_000;

/// What a dead AI of a template sets off, resolved once with its spec: the weapon and ammunition of its brain's
/// death explosion, and the thrown weapon it drops with the count.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeathSpecial {
    pub blast: Option<(u16, u16)>,
    pub throw: Option<((u16, u16), u32)>,
}

/// One item a dead AI dropped (DSV-599): its flight, when it arms, the damage scale of its blast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Growth {
    pub key: u32,
    pub arm_at: Millis,
    pub scale: f32,
}

/// A cloud at a dead AI's body (DSV-600): until `until`, every [`BURN_STEP_MS`] each living human within `radius`
/// takes the progressive damage of `weapon` (`per_second` for `duration_ms` after he was last inside).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cloud {
    pub pos: [f32; 3],
    pub until: Millis,
    pub next: Millis,
    pub weapon: u16,
    pub per_second: f32,
    pub damage_type: Option<u8>,
    pub duration_ms: u32,
    pub radius: f32,
}

/// The speed of an AI's step: its walk before the rush, else its speed.
// DC-DEFAULT: DSV-597 (proposed)
pub(super) fn step_speed(spec: &AiSpec, rushing: bool) -> f32 {
    match spec.walk {
        Some(w) if !rushing => w.min(spec.speed),
        _ => spec.speed,
    }
}

/// The factor of a hit on an AI wearing another row's body: its own skeleton's factor for the hit's location (the
/// highest of its nodes there) when the hit named a node (`by_node`), else the worn factor but never above its own;
/// the worn factor when its own skeleton has no node at that location.
// DC-DEFAULT: DSV-601 (proposed)
pub(super) fn own_factor(own: &[HitNode], worn: (f32, u8), by_node: bool) -> f32 {
    let best = own
        .iter()
        .filter(|n| n.location == worn.1 && n.factor.is_finite() && n.factor >= 0.0)
        .map(|n| n.factor)
        .reduce(f32::max);
    match best {
        Some(o) if by_node => o,
        Some(o) => worn.0.min(o),
        None => worn.0,
    }
}

impl Game {
    /// The walk speed and the rush distance of a melee AI of template `t` running at `run` (DSV-597): its
    /// attribute row's `WalkSpeed` (below `run`) and `SeeEnemyDistance`; `(None, None)` for a shooting AI, a row
    /// without both, or with the specials off.
    // OBSERVED: ROM table key AIBUTES.TXT [AttributeTemplate<n>] WalkSpeed, SeeEnemyDistance
    // DC-DEFAULT: DSV-597 (proposed)
    pub(super) fn ai_rush(
        &self,
        t: Option<&AiTemplate>,
        run: f32,
        melee: bool,
    ) -> (Option<f32>, Option<f32>) {
        if !self.cfg.coop.specials || !melee {
            return (None, None);
        }
        let row = t
            .and_then(|t| t.body_row.as_deref())
            .zip(self.coop.as_ref())
            .and_then(|(n, c)| c.tables.attributes_named(n));
        match row.map(|r| (r.walk_speed, r.see_enemy)) {
            Some((Some(w), Some(r))) if w < run => (Some(w), Some(r)),
            _ => (None, None),
        }
    }

    /// What a dead AI of template `t` sets off: the weapon rows its brain row's death keys name (a name the weapon
    /// table lacks sets off nothing).
    // DC-DEFAULT: DSV-598 / DSV-599 (proposed)
    pub(super) fn ai_death_special(&self, t: Option<&AiTemplate>) -> DeathSpecial {
        let brain = t
            .and_then(|t| t.brain.as_deref())
            .zip(self.coop.as_ref())
            .and_then(|(b, c)| c.tables.brain_named(b));
        let Some(b) = brain else {
            return DeathSpecial::default();
        };
        let row = |n: &str| {
            let w = self.weapon_named(n)?;
            Some((w, self.tables.weapons.ammo_of(w).unwrap_or(0)))
        };
        DeathSpecial {
            blast: b.death_blast.as_deref().and_then(row),
            throw: b
                .death_throw
                .as_ref()
                .and_then(|(n, k)| Some((row(n)?, *k))),
        }
    }

    /// The hit nodes of the skeleton row of template `t`'s own body row when the AI wears another row (`worn`, or
    /// the default character), so that its hits count by its own row (DSV-601).
    // OBSERVED: ROM table key MODELBUTES.TXT [AIModel<n>] Skeleton; [Skeleton<s>] Node<k>DamageFactor, Node<k>Location
    // (the light infected row names a skeleton row of its own whose head nodes have a factor far below the normal
    // row's; 2026-11-05 scratch run)
    // DC-DEFAULT: DSV-601 (proposed)
    pub(super) fn ai_own_nodes(
        &self,
        t: Option<&AiTemplate>,
        worn: Option<&str>,
    ) -> Option<Vec<HitNode>> {
        if !self.cfg.coop.specials {
            return None;
        }
        let want = t?.body_row.as_deref()?;
        if worn.is_some_and(|w| w.eq_ignore_ascii_case(want)) {
            return None;
        }
        let row = self.coop.as_ref()?.tables.body_named(want)?;
        let nodes = self.tables.chars.skeletons.get(&row.skeleton?)?;
        (!nodes.is_empty()).then(|| nodes.clone())
    }

    /// The damage scale of a dead AI's dropped item `key` (1 for every other flight).
    pub(in crate::game) fn coop_growth_scale(&self, key: u32) -> f32 {
        self.coop
            .as_ref()
            .and_then(|c| c.growths.iter().find(|g| g.key == key))
            .map_or(1.0, |g| g.scale)
    }

    /// The AI of `slot` just died: its blast or cloud goes off at its body and its thrown items drop, each hurting
    /// by the difficulty's `AIDamageFactor`.
    // DC-DEFAULT: DSV-598..DSV-600 (proposed)
    pub(super) fn coop_death_specials(&mut self, slot: usize, now: Millis) {
        if !self.cfg.coop.specials {
            return;
        }
        let Some(c) = self.coop.as_ref() else {
            return;
        };
        let Some(a) = c.ais.get(slot) else {
            return;
        };
        let (death, pos, yaw, obj) = (
            a.spec.death.clone(),
            a.pos,
            a.yaw,
            a.obj.unwrap_or(NO_OBJECT),
        );
        let scale = c.plan.difficulty.ai_damage_factor.unwrap_or(1.0);
        if let Some(wa) = death.blast {
            self.death_blast(slot, wa, pos, obj, scale, now);
        }
        if let Some((wa, n)) = death.throw {
            self.death_throw(slot, wa, n, pos, yaw, obj, scale, now);
        }
    }

    /// The death explosion (DSV-598): an ammunition row with a blast goes off at once as a thrown round's blast does
    /// (the blast record, area damage to the living humans within the radius, the breakables); one with progressive
    /// damage only leaves a cloud (DSV-600) and its record.
    // OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] AreaDamage, AreaDamageType, AreaDamageRadius, ProgDamage,
    // ProgDamageType, ProgDamageDuration, ProgDamageRadius, ProgDamageLifetime (the two infected death weapons: one a
    // blast, one progressive damage only)
    // DC-DEFAULT: DSV-598 (proposed), DSV-600 (proposed)
    fn death_blast(
        &mut self,
        slot: usize,
        (weapon, ammo): (u16, u16),
        pos: [f32; 3],
        obj: u16,
        scale: f32,
        now: Millis,
    ) {
        if self.tables.weapons.area(weapon, ammo).is_some() {
            let fl = Flight {
                key: 0,
                owner: gs::SHOT_CLIENT_NONE,
                owner_obj: obj,
                owner_team: AI_TEAM,
                weapon,
                ammo,
                origin: pos,
                pos,
                vel: [0.0; 3],
                gravity: 0.0,
                detonate_at: now,
                last: now,
                resting: true,
                impact: false,
                burst: false,
            };
            self.log(format!(
                "co-op: AI slot {slot} blows up (weapon {weapon} ammo {ammo})"
            ));
            self.blast(&fl, scale, now);
            if let Some(c) = self.coop.as_mut() {
                c.death_blasts += 1;
            }
        } else if let Some(prog) = self.tables.weapons.prog(weapon, ammo) {
            let m = gs::shot_effect(&gs::ShotEffect {
                first: None,
                shooter: obj,
                weapon,
                ammo,
                from: pos,
                to: pos,
                normal: TODO_BLAST_NORMAL,
                surface: self.world_surface_below(pos),
                mask: gs::SHOT_MASK_MISS,
                client: gs::SHOT_CLIENT_NONE,
                mods: gs::NO_SHOT_MODS,
            });
            for to in self.introduced() {
                self.out.push(GameOut::Unreliable(to, m.clone()));
            }
            let life_ms = self
                .tables
                .weapons
                .prog_lifetime(weapon, ammo)
                .map_or(CLOUD_LIFETIME_MS, |s| (s * 1000.0).round() as u32);
            let cloud = Cloud {
                pos,
                until: now.wrapping_add(life_ms),
                next: now,
                weapon,
                per_second: prog.per_second * scale,
                damage_type: prog.damage_type,
                duration_ms: (prog.duration_s * 1000.0).round().max(1.0) as u32,
                radius: prog.radius,
            };
            self.log(format!(
                "co-op: AI slot {slot} leaves a cloud (weapon {weapon}, radius {:.0}, {life_ms} ms)",
                prog.radius
            ));
            if let Some(c) = self.coop.as_mut() {
                c.clouds.push(cloud);
                c.death_clouds += 1;
            }
        } else {
            self.log(format!(
                "co-op: AI slot {slot}: death weapon {weapon} has no blast or progressive damage row"
            ));
        }
    }

    /// The dropped items (DSV-599): `n` flights of the thrown weapon from the body, spread evenly around its facing
    /// at [`GROWTH_TOSS`], each armed after `coop.growth_arm_ms`.
    // DC-DEFAULT: DSV-599 (proposed)
    #[allow(clippy::too_many_arguments)]
    fn death_throw(
        &mut self,
        slot: usize,
        wa: (u16, u16),
        n: u32,
        pos: [f32; 3],
        yaw: f32,
        obj: u16,
        scale: f32,
        now: Millis,
    ) {
        let arm_at = now.wrapping_add(self.cfg.coop.growth_arm_ms);
        let h = GROWTH_TOSS * GROWTH_TOSS_ANGLE.cos();
        let up = GROWTH_TOSS * GROWTH_TOSS_ANGLE.sin();
        for k in 0..n {
            let a = yaw + k as f32 * std::f32::consts::TAU / n as f32;
            let vel = [a.sin() * h, up, a.cos() * h];
            let key = self.server_throw(obj, AI_TEAM, wa, pos, vel, now);
            if let Some(c) = self.coop.as_mut() {
                c.growths.push(Growth { key, arm_at, scale });
                c.death_throws += 1;
            }
        }
        self.log(format!("co-op: AI slot {slot} drops {n} x weapon {}", wa.0));
    }

    /// The dead AIs' hazards each step: an armed dropped item with a living human within `coop.growth_trigger` goes
    /// off in this tick (else at its fuse; an item whose flight ended is forgotten); a cloud hurts every living human
    /// within its radius whose centre the trace world does not hide (not inside his spawn window): his burn is set to
    /// last the cloud's damage duration from now (DSV-455's burn steps deal it).
    // DC-DEFAULT: DSV-599 (proposed), DSV-600 (proposed)
    pub(super) fn coop_tick_hazards(&mut self, now: Millis) {
        let Some(c) = self.coop.as_mut() else {
            return;
        };
        if c.growths.is_empty() && c.clouds.is_empty() {
            return;
        }
        let growths = std::mem::take(&mut c.growths);
        let clouds = std::mem::take(&mut c.clouds);
        let humans: Vec<(u8, [f32; 3])> = self
            .players
            .values()
            .filter(|p| p.life == Life::Alive && p.introduced)
            .map(|p| (p.pid, p.pos))
            .collect();
        let trigger = self.cfg.coop.growth_trigger;
        let mut keep = Vec::with_capacity(growths.len());
        for g in growths {
            let Some(at) = self.flight_pos(g.key) else {
                continue;
            };
            let near = humans.iter().find(|(_, p)| len3(sub3(*p, at)) <= trigger);
            if let Some((pid, _)) = near.filter(|_| !later(g.arm_at, now)) {
                if self.burst_flight(g.key) {
                    self.log(format!("co-op: AI drop {} goes off at client {pid}", g.key));
                }
            }
            keep.push(g);
        }
        let blast_los = self.cfg.projectile.blast_los;
        let mut kept_clouds = Vec::with_capacity(clouds.len());
        for mut cl in clouds {
            if later(now, cl.until) {
                continue;
            }
            if later(cl.next, now) {
                kept_clouds.push(cl);
                continue;
            }
            cl.next = now.wrapping_add(BURN_STEP_MS);
            let lifted = [cl.pos[0], cl.pos[1] + BLAST_RAY_LIFT, cl.pos[2]];
            for (vpid, vpos) in &humans {
                let d = len3(sub3(*vpos, cl.pos));
                if !(d.is_finite() && d <= cl.radius) || self.spawn_shielded(*vpid) {
                    continue;
                }
                if blast_los && self.projectile_ray(lifted, *vpos).is_some() {
                    continue;
                }
                let until = now.wrapping_add(cl.duration_ms);
                match self.burns.iter_mut().find(|b| b.victim == *vpid) {
                    Some(b) => {
                        b.until = until;
                        b.owner = gs::SHOT_CLIENT_NONE;
                        b.weapon = cl.weapon;
                        b.per_second = cl.per_second;
                        b.damage_type = cl.damage_type;
                    }
                    None => self.burns.push(Burn {
                        victim: *vpid,
                        owner: gs::SHOT_CLIENT_NONE,
                        weapon: cl.weapon,
                        per_second: cl.per_second,
                        damage_type: cl.damage_type,
                        until,
                        next: now.wrapping_add(BURN_STEP_MS),
                    }),
                }
            }
            kept_clouds.push(cl);
        }
        if let Some(c) = self.coop.as_mut() {
            keep.append(&mut c.growths);
            kept_clouds.append(&mut c.clouds);
            c.growths = keep;
            c.clouds = kept_clouds;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::{GameTuning, RoundPhase};
    use super::super::AiState;
    use super::*;
    use crate::coop_rom::{AiAttributeRow, AiBodyRow, AiBrainRow};
    use crate::pipe::RoomSettings;
    use crate::rom_tables::RomTables;
    use crate::world::{synthetic_world_bytes, world_from_bytes, CoopWorld};

    /// Synthetic tables: a gun (3) the AI hold, a blast weapon (20), a thrown item (21), a progressive-damage weapon
    /// (22), one body row for the fallback character, a skeleton row 3 whose head is barely above its torso.
    fn tables() -> RomTables {
        use crate::chars::{BodyRow, CharTables};
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Weapon3]\nName = \"G3\"\nHHModel = \"HH\\\\g3.ltb\"\nAmmoName0 = \"A3\"\nShotsPerClip = 30\n",
            "[Weapon20]\nName = \"Boom\"\nAmmoName0 = \"A20\"\n",
            "[Weapon21]\nName = \"Egg\"\nHHModel = \"HH\\\\egg.ltb\"\nAmmoName0 = \"A21\"\nIsAmmo = 1\n",
            "[Weapon22]\nName = \"Fog\"\nAmmoName0 = \"A22\"\n",
            "[Ammo3]\nName = \"A3\"\nMaxAmount = 90\n",
            "[Ammo20]\nName = \"A20\"\nType = 2\nAreaDamage = 40\nAreaDamageType = 7\nAreaDamageRadius = 250\n",
            "[Ammo21]\nName = \"A21\"\nType = 2\nAreaDamage = 12\nAreaDamageRadius = 120\n",
            "[Ammo22]\nName = \"A22\"\nType = 2\nProgDamage = 8\nProgDamageType = 9\nProgDamageDuration = 6\n",
            "ProgDamageRadius = 180\nProgDamageLifetime = 4\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"G3\"\n",
        ));
        let mut chars = CharTables::default();
        chars.bodies.insert(
            0,
            BodyRow {
                model: "BODY\\b.ltb".into(),
                ..BodyRow::default()
            },
        );
        chars.skeletons.insert(
            3,
            vec![
                HitNode {
                    factor: 1.0,
                    location: 2,
                },
                HitNode {
                    factor: 1.25,
                    location: 1,
                },
            ],
        );
        RomTables {
            weapons: crate::rom_tables::WeaponTable::from_doc(&doc),
            chars,
            ..RomTables::default()
        }
    }

    fn rig() -> crate::chars::Rig {
        use crate::chars::{ClipFact, Rig};
        let c = |key, len_ms| ClipFact {
            key,
            len_ms,
            extent: [15.0, 50.0, 15.0],
            footsteps: 0,
            upper: None,
        };
        Rig {
            children: vec!["BODY\\kid.ltb".into()],
            clips: vec![
                c(([1; 8], 4), 7200),
                c(crate::chars::CLIP_LOWER_STAND, 1000),
                c(crate::chars::CLIP_LOWER_WALK, 800),
                c(crate::chars::CLIP_UPPER_STAND, 1500),
                c(crate::chars::CLIP_UPPER_WALK, 900),
            ],
            own_clips: 1,
            grip_socket: 2,
            sockets: Vec::new(),
            stand_extent: Some([15.0, 50.0, 15.0]),
            tracker_masks: [None; 2],
        }
    }

    /// A running co-op room (class 5) on the synthetic world with six AI templates (`pop` blows up, `lay` drops three
    /// items, `fog` leaves a cloud, `plain` does none of these, `biter` walks then rushes, `lite` wears another body
    /// than its own row's) and introduced living humans at `at` (clients 1..).
    fn game(at: &[[f32; 3]]) -> Game {
        let mut w = world_from_bytes(&synthetic_world_bytes(), "Worlds\\S.dat", "S").unwrap();
        let t = |name: &str, brain: &str, body: Option<&str>| AiTemplate {
            name: name.as_bytes().to_vec(),
            right_hand: Some("G3".into()),
            max_health: Some(60.0),
            template: true,
            brain: Some(brain.into()),
            body_row: body.map(str::to_string),
            ..Default::default()
        };
        w.coop = CoopWorld {
            templates: vec![
                t("pop", "Popper", None),
                t("lay", "Layer", None),
                t("fog", "Fogger", None),
                t("plain", "Plain", None),
                t("biter", "Biter", Some("BiterBody")),
                t("lite", "Plain", Some("LiteBody")),
            ],
            ..CoopWorld::default()
        };
        let mut cfg = GameTuning::default();
        cfg.coop.nav = false;
        cfg.drops.ai_drops = false;
        let mut g = Game::new(cfg, &w, tables());
        g.insert_rig("BODY\\b.ltb", rig());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "5".into());
        g.set_room(&room);
        let tb = g.coop_tables_mut().unwrap();
        let brain = |name: &str| AiBrainRow {
            name: name.into(),
            ranged_max: Some(0.0),
            melee_max: Some(90.0),
            ..AiBrainRow::default()
        };
        tb.brains.push(AiBrainRow {
            death_blast: Some("Boom".into()),
            ..brain("popper")
        });
        tb.brains.push(AiBrainRow {
            death_throw: Some(("Egg".into(), 3)),
            ..brain("layer")
        });
        tb.brains.push(AiBrainRow {
            death_blast: Some("Fog".into()),
            ..brain("fogger")
        });
        tb.brains.push(brain("plain"));
        tb.brains.push(brain("biter"));
        tb.attributes.push(AiAttributeRow {
            name: "BiterBody".into(),
            run_speed: Some(160.0),
            walk_speed: Some(80.0),
            see_enemy: Some(300.0),
            hit_points: None,
        });
        tb.bodies.insert(
            7,
            AiBodyRow {
                index: 7,
                name: "LiteBody".into(),
                model: "BODY\\lite.ltb".into(),
                skeleton: Some(3),
                ..AiBodyRow::default()
            },
        );
        for (i, pos) in at.iter().enumerate() {
            let pid = i as u8 + 1;
            g.prepare_player(
                pid,
                None,
                None,
                10 + u16::from(pid),
                *pos,
                [0.0; 3],
                None,
                0,
            );
            let p = g.players.get_mut(&pid).unwrap();
            p.introduced = true;
            p.life = Life::Alive;
            p.health = 100.0;
        }
        g.spawn_notes.clear();
        g.phase = RoundPhase::Running { start: 0 };
        g.take_out();
        g
    }

    fn health(g: &Game, pid: u8) -> f32 {
        g.players[&pid].health
    }

    /// Kills a new AI of `template` at the origin at 200 ms (client 1 the killer).
    fn kill(g: &mut Game, template: &[u8]) -> Vec<GameOut> {
        assert!(g.coop_spawn_test_ai(template, [0.0, 0.0, 0.0], 100));
        g.take_out();
        g.coop_kill_ai(0, 1, 200);
        g.take_out()
    }

    /// Runs the hazards, the flights and the burns every 50 ms from `from` to `to`.
    fn run(g: &mut Game, from: Millis, to: Millis) {
        let mut t = from;
        while t <= to {
            g.coop_tick_hazards(t);
            g.tick_projectiles(t);
            g.tick_burns(t);
            t += 50;
        }
    }

    fn floor() -> dc_sim::CollisionWorld {
        use dc_sim::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID, V3};
        let f = ConvexPoly::from_corners(
            vec![
                V3::new(-2000.0, -50.0, -2000.0),
                V3::new(-2000.0, -50.0, 2000.0),
                V3::new(2000.0, -50.0, 2000.0),
                V3::new(2000.0, -50.0, -2000.0),
            ],
            SURF_SOLID,
        );
        CollisionWorld::from_parts(vec![CollModel::from_polys(std::slice::from_ref(&f))], &[])
    }

    fn shot_records(out: &[GameOut]) -> usize {
        out.iter()
            .filter(|o| matches!(o, GameOut::Unreliable(1, m) if m.as_bytes()[1] == gs::FX_SHOT))
            .count()
    }

    /// DSV-598: a Charger-like AI killed next to a human hurts him by the blast row (full damage inside the inner
    /// quarter), sends the blast record, and leaves a human 400 units away unhurt.
    #[test]
    fn a_blast_at_the_body_hurts_the_human_next_to_it() {
        let mut g = game(&[[60.0, 0.0, 0.0], [400.0, 0.0, 0.0]]);
        let out = kill(&mut g, b"pop");
        assert_eq!(health(&g, 1), 60.0, "40 at the centre");
        assert_eq!(health(&g, 2), 100.0, "outside the 250-unit radius");
        assert_eq!(shot_records(&out), 1, "the blast record");
        let c = g.coop().unwrap();
        assert_eq!((c.death_blasts, c.death_throws, c.death_clouds), (1, 0, 0));
        assert!(c.clouds.is_empty() && g.flights_in_air() == 0);
    }

    /// DSV-598: the difficulty's `AIDamageFactor` scales the blast; the specials off: no blast.
    #[test]
    fn the_blast_takes_the_ai_damage_factor_and_the_switch() {
        let mut g = game(&[[60.0, 0.0, 0.0]]);
        g.coop.as_mut().unwrap().plan.difficulty.ai_damage_factor = Some(0.5);
        kill(&mut g, b"pop");
        assert_eq!(health(&g, 1), 80.0);
        let mut g = game(&[[60.0, 0.0, 0.0]]);
        g.cfg.coop.specials = false;
        kill(&mut g, b"pop");
        assert_eq!(health(&g, 1), 100.0);
        assert_eq!(g.coop().unwrap().death_blasts, 0);
    }

    /// DSV-599: a Striker-like AI drops three items as thrown objects; before they arm a human beside them sets
    /// none off, once armed they go off at once and hurt him.
    #[test]
    fn dropped_items_arm_then_go_off_near_a_human() {
        let mut g = game(&[[30.0, 0.0, 0.0]]);
        g.set_trace_world(Some(floor()));
        let out = kill(&mut g, b"lay");
        let made = out
            .iter()
            .filter(|o| matches!(o, GameOut::Projectile(_)))
            .count();
        assert_eq!((made, g.flights_in_air()), (3, 3), "three objects");
        assert_eq!(g.coop().unwrap().growths.len(), 3);
        run(&mut g, 250, 1_150);
        assert_eq!(g.flights_in_air(), 3, "not armed before 1 s");
        assert_eq!(health(&g, 1), 100.0);
        run(&mut g, 1_200, 1_300);
        assert_eq!(g.flights_in_air(), 0, "armed: all three went off");
        let hurt = 100.0 - health(&g, 1);
        assert!(hurt > 0.0 && hurt <= 36.0, "took {hurt}");
        run(&mut g, 1_350, 1_400);
        assert!(g.coop().unwrap().growths.is_empty(), "forgotten");
    }

    /// DSV-599: with nobody near, the items go off at their fuse; a human far away takes nothing.
    #[test]
    fn dropped_items_go_off_at_their_fuse() {
        let mut g = game(&[[900.0, 0.0, 0.0]]);
        g.set_trace_world(Some(floor()));
        kill(&mut g, b"lay");
        let fuse = g.cfg.projectile.fuse_ms;
        run(&mut g, 250, 200 + fuse - 100);
        assert_eq!(g.flights_in_air(), 3, "before the fuse");
        run(&mut g, 200 + fuse - 50, 200 + fuse + 100);
        assert_eq!(g.flights_in_air(), 0, "at the fuse");
        assert_eq!(health(&g, 1), 100.0);
    }

    /// DSV-600: a Crusher-like AI leaves a cloud (and its record): a human standing in it for 2 s takes its damage
    /// per second, one 400 units away nothing; the cloud ends after its row's lifetime.
    #[test]
    fn a_cloud_hurts_only_the_humans_inside() {
        let mut g = game(&[[50.0, 0.0, 0.0], [400.0, 0.0, 0.0]]);
        let out = kill(&mut g, b"fog");
        assert_eq!(shot_records(&out), 1, "the cloud's record");
        assert_eq!(g.coop().unwrap().clouds.len(), 1);
        run(&mut g, 250, 2_250);
        let hurt = 100.0 - health(&g, 1);
        assert!((14.0..=17.0).contains(&hurt), "2 s at 8 per second: {hurt}");
        assert_eq!(health(&g, 2), 100.0, "400 units away");
        run(&mut g, 2_300, 4_300);
        assert!(g.coop().unwrap().clouds.is_empty(), "the cloud is gone");
    }

    /// DSV-598..DSV-600: an AI whose brain names no death weapon sets nothing off.
    #[test]
    fn a_plain_death_sets_nothing_off() {
        let mut g = game(&[[30.0, 0.0, 0.0]]);
        let out = kill(&mut g, b"plain");
        run(&mut g, 250, 3_000);
        assert_eq!(health(&g, 1), 100.0);
        assert_eq!(shot_records(&out), 0);
        let c = g.coop().unwrap();
        assert_eq!((c.death_blasts, c.death_throws, c.death_clouds), (0, 0, 0));
        assert!(g.flights_in_air() == 0 && g.burning().is_empty());
    }

    /// DSV-597: a melee AI walks at its row's walk speed until a human in sight comes within its rush distance, then
    /// runs at its speed, and keeps running when the human goes away again.
    #[test]
    fn the_rush_starts_at_the_distance() {
        let mut g = game(&[[1000.0, 0.0, 0.0]]);
        assert!(g.coop_spawn_test_ai(b"biter", [0.0, 0.0, 0.0], 100));
        let spec = g.ai_chars()[0].spec.clone();
        assert_eq!(
            (spec.speed, spec.walk, spec.rush),
            (160.0, Some(80.0), Some(300.0))
        );
        let x = |g: &Game| g.ai_chars()[0].pos[0];
        let x0 = x(&g);
        g.ai_step(0, 0.1, 200);
        assert!((x(&g) - x0 - 8.0).abs() < 0.01, "walks: {}", x(&g) - x0);
        assert!(!g.ai_chars()[0].rushing);
        g.players.get_mut(&1).unwrap().pos = [x(&g) + 250.0, 0.0, 0.0];
        let x1 = x(&g);
        g.ai_step(0, 0.1, 300);
        assert!((x(&g) - x1 - 16.0).abs() < 0.01, "runs: {}", x(&g) - x1);
        g.players.get_mut(&1).unwrap().pos = [x(&g) + 1000.0, 0.0, 0.0];
        let x2 = x(&g);
        g.ai_step(0, 0.1, 400);
        assert!((x(&g) - x2 - 16.0).abs() < 0.01, "keeps running");
        // an AI whose body row has no attribute row: always its speed
        let plain = g.ai_spec(Some(b"pop")).unwrap();
        assert_eq!((plain.walk, plain.rush), (None, None));
    }

    /// DSV-601: an AI wearing another body than its own row's gets its own skeleton's nodes; a hit counts by its own
    /// factor for the location (by a node), never above it (by the box).
    #[test]
    fn a_substitute_body_counts_by_the_own_skeleton() {
        let mut g = game(&[[1000.0, 0.0, 0.0]]);
        let lite = g.ai_spec(Some(b"lite")).unwrap();
        let own = lite.own_nodes.clone().expect("its own nodes");
        assert_eq!(own.len(), 2);
        assert_eq!(g.ai_spec(Some(b"plain")).unwrap().own_nodes, None);
        assert_eq!(own_factor(&own, (6.0, 1), true), 1.25, "the own head");
        assert_eq!(
            own_factor(&own, (2.0, 1), false),
            1.25,
            "capped by the own head"
        );
        assert_eq!(
            own_factor(&own, (1.0, 1), false),
            1.0,
            "a lower box factor stays"
        );
        assert_eq!(own_factor(&own, (0.75, 4), true), 0.75, "no own node there");
        let tank = [HitNode {
            factor: 3.0,
            location: 1,
        }];
        assert_eq!(own_factor(&tank, (6.0, 1), true), 3.0);
        assert_eq!(own_factor(&tank, (2.0, 1), false), 2.0);
    }

    /// A thrown round of the blast weapon (40 at the centre, radius 250, damage type 7) by client `pid`, lying at `at`.
    fn grenade(g: &Game, pid: u8, at: [f32; 3]) -> Flight {
        let p = &g.players[&pid];
        Flight {
            key: 900,
            owner: pid,
            owner_obj: p.obj,
            owner_team: p.team,
            weapon: 20,
            ammo: 20,
            origin: at,
            pos: at,
            vel: [0.0; 3],
            gravity: 0.0,
            detonate_at: 0,
            last: 0,
            resting: true,
            impact: false,
            burst: false,
        }
    }

    /// Spawns an AI of `template` at `at` and returns its slot.
    fn spawn_at(g: &mut Game, template: &[u8], at: [f32; 3]) -> usize {
        assert!(g.coop_spawn_test_ai(template, at, 100));
        g.take_out();
        g.ai_chars().len() - 1
    }

    /// DSV-603: the health maximum is the kind's attribute row `HitPoints` (x the round's rate), else the placed
    /// object's value, else `coop.default_health`; `coop.ai_health=world` puts the placed value first.
    #[test]
    fn the_health_maximum_comes_from_the_kind_row() {
        let max = |g: &mut Game, t: &[u8]| {
            let s = spawn_at(g, t, [0.0; 3]);
            g.ai_chars()[s].max_health
        };
        let mut g = game(&[[900.0, 0.0, 0.0]]);
        g.coop_tables_mut().unwrap().attributes[0].hit_points = Some(1000.0);
        assert_eq!(max(&mut g, b"biter"), 1000.0, "the row");
        assert_eq!(max(&mut g, b"plain"), 60.0, "no row: the placed value");
        let mut g = game(&[[900.0, 0.0, 0.0]]);
        g.coop_tables_mut().unwrap().attributes[0].hit_points = Some(1000.0);
        g.coop.as_mut().unwrap().plan.health_rates = vec![1.5];
        assert_eq!(max(&mut g, b"biter"), 1500.0, "x the round's rate");
        let mut g = game(&[[900.0, 0.0, 0.0]]);
        g.coop_tables_mut().unwrap().attributes[0].hit_points = Some(1000.0);
        g.cfg.coop.set("ai_health", "world").unwrap();
        assert_eq!(max(&mut g, b"biter"), 60.0, "world: the placed value first");
        g.coop_world.templates[3].max_health = None;
        assert_eq!(max(&mut g, b"plain"), 100.0, "neither: the default");
        assert!(g.cfg.coop.set("coop.ai_health", "rows").is_ok());
        assert!(g.cfg.coop.set("ai_health", "both").is_err());
    }

    /// DSV-604: a player's grenade hurts an AI 100 units away by the falloff (40 x 150 / 187.5), a hit of the
    /// thrower's; one at its body kills it: the kill and its points are the thrower's, 0xd7 sub 1 goes out.
    #[test]
    fn a_grenade_hurts_the_ai_by_the_falloff_and_credits_the_kill() {
        let mut g = game(&[[900.0, 0.0, 0.0]]);
        let s = spawn_at(&mut g, b"plain", [0.0; 3]);
        let at = g.ai_chars()[s].pos;
        g.blast(&grenade(&g, 1, [at[0] - 100.0, at[1], at[2]]), 1.0, 200);
        g.take_out();
        let h = g.ai_chars()[s].health;
        assert!((h - 28.0).abs() < 1e-3, "60 - 32: {h}");
        assert_eq!(health(&g, 1), 100.0, "the thrower is outside the radius");
        g.blast(&grenade(&g, 1, at), 1.0, 300);
        let out = g.take_out();
        assert_eq!(g.ai_chars()[s].state, AiState::Dead { at: 300 });
        let c = g.coop().unwrap();
        let key = 0x8000_0001;
        assert_eq!((c.ai_killed, c.kills.get(&key)), (1, Some(&1)));
        assert_eq!(
            c.points[&key],
            8 + 40 + 8,
            "a body hit, a kill and its lethal hit (DSV-605)"
        );
        let sub1 = gs::coop_points(1, None, 48, 56);
        assert!(out
            .iter()
            .any(|o| matches!(o, GameOut::To(1, m) if m.as_bytes() == sub1.as_bytes())));
    }

    /// DSV-603 / DSV-604: a Crusher-like AI (a row of 1000) survives a grenade at its body.
    #[test]
    fn a_durable_kind_survives_a_grenade() {
        let mut g = game(&[[900.0, 0.0, 0.0]]);
        g.coop_tables_mut().unwrap().attributes[0].hit_points = Some(1000.0);
        let s = spawn_at(&mut g, b"biter", [0.0; 3]);
        let at = g.ai_chars()[s].pos;
        g.blast(&grenade(&g, 1, at), 1.0, 200);
        let a = &g.ai_chars()[s];
        assert_eq!((a.state, a.health), (AiState::Alive, 960.0));
    }

    /// DSV-604: an AI whose own body row's gear row protects the blast's damage type in full takes nothing, half
    /// protection halves it; an AI's death blast hurts an unprotected AI beside it, credited to no player.
    #[test]
    fn the_body_gear_protects_by_damage_type() {
        use crate::rom_tables::GearProtection;
        let gear = |share: f32| GearProtection {
            armor: Some(500.0),
            slots: vec![(7, [share; 4])],
        };
        let mut g = game(&[[900.0, 0.0, 0.0]]);
        g.coop_tables_mut()
            .unwrap()
            .bodies
            .get_mut(&7)
            .unwrap()
            .gear = Some(30);
        g.tables.weapons.gear.insert(30, gear(1.0));
        let s = spawn_at(&mut g, b"lite", [0.0; 3]);
        let at = g.ai_chars()[s].pos;
        g.blast(&grenade(&g, 1, at), 1.0, 200);
        assert_eq!(g.ai_chars()[s].health, 60.0, "immune");
        g.tables.weapons.gear.insert(30, gear(0.5));
        g.blast(&grenade(&g, 1, at), 1.0, 300);
        assert_eq!(g.ai_chars()[s].health, 40.0, "half");
        // a Charger-like death next to an unprotected AI
        let mut g = game(&[[900.0, 0.0, 0.0]]);
        let s = spawn_at(&mut g, b"plain", [60.0, 0.0, 0.0]);
        assert!(g.coop_spawn_test_ai(b"pop", [0.0, 0.0, 0.0], 100));
        g.take_out();
        g.coop_kill_ai(s + 1, 1, 200);
        assert_eq!(g.ai_chars()[s].health, 20.0, "40 at the inner part");
        let c = g.coop().unwrap();
        assert_eq!(
            c.points[&0x8000_0001],
            40 + 50 + 10,
            "only the shot kill is the player's (kill, head kill, the head hit)"
        );
    }

    const TORSO: u8 = 2;

    /// The 0xc8 banners `out` sent to client `pid`, as `(kills, head, combo)`.
    fn banners(out: &[GameOut], pid: u8) -> Vec<(u16, bool, u32)> {
        out.iter()
            .filter_map(|x| match x {
                GameOut::To(p, m) if *p == pid && m.as_bytes()[1] == gs::GS_MULTI_KILL => {
                    let mut r = m.reader();
                    r.read_u8();
                    r.read_u8();
                    Some((r.read_u16(), r.read_bit(), r.read_u32()))
                }
                _ => None,
            })
            .collect()
    }

    /// DSV-602: each AI kill by a human sends the killer alone a 0xc8 with its AI kill count, the head flag of the
    /// lethal hit and its place in the kill chain; a gap longer than the window restarts the chain, the count stays.
    #[test]
    fn quick_ai_kills_send_a_counted_banner() {
        let mut g = game(&[[1000.0, 0.0, 0.0], [1100.0, 0.0, 0.0]]);
        for i in 0..4 {
            assert!(g.coop_spawn_test_ai(b"plain", [0.0, 0.0, 0.0], 100 + i));
        }
        g.take_out();
        let window = g.cfg.multi_kill_window_ms;
        // two quick body kills: the count and the combo rise together
        g.ai_damage(0, 1, 1000.0, TORSO, 1000);
        let out = g.take_out();
        assert_eq!(banners(&out, 1), [(1, false, 1)]);
        assert_eq!(banners(&out, 2), [], "only the killer");
        g.ai_damage(1, 1, 1000.0, TORSO, 1500);
        assert_eq!(banners(&g.take_out(), 1), [(2, false, 2)]);
        // a head kill inside the window sets the flag; another player's chain is its own
        g.ai_damage(2, 1, 1000.0, gs::KILL_ZONE_HEAD, 2000);
        assert_eq!(banners(&g.take_out(), 1), [(3, true, 3)]);
        // a hit that does not kill sends nothing
        g.ai_damage(3, 2, 1.0, TORSO, 2100);
        assert_eq!(banners(&g.take_out(), 2), []);
        g.ai_damage(3, 2, 1000.0, TORSO, 2200);
        assert_eq!(banners(&g.take_out(), 2), [(1, false, 1)]);
        // a gap beyond the window: the combo restarts at 1, the kill count goes on
        assert!(g.coop_spawn_test_ai(b"plain", [0.0, 0.0, 0.0], 2300));
        g.take_out();
        g.ai_damage(4, 1, 1000.0, TORSO, 2000 + window + 1);
        assert_eq!(banners(&g.take_out(), 1), [(4, false, 1)]);
    }

    /// A table like the Cabin Fever ROM's (hit zones 10 / 15 / 8 / 5, kill 40, head kill + 50, combo + 30) at its
    /// `ScoreFactor` 0.6: body hit 9, kill 24, head kill 54, combo bonus 18.
    fn score_like_the_rom(g: &mut Game) {
        let c = g.coop.as_mut().unwrap();
        c.plan.score = crate::coop_rom::ScoreTable {
            hit: [Some(10.0), Some(15.0), Some(8.0), Some(5.0)],
            kill: Some(40.0),
            boss_death: Some(500.0),
            head_kill: Some(50.0),
            combo: Some(30.0),
        };
        c.plan.difficulty.score_factor = Some(0.6);
    }

    /// DSV-605: a kill pays the kill (or head kill) and the lethal hit's zone points; one inside the chain window
    /// also the combo bonus; a kill after the window starts a new chain without it.
    #[test]
    fn a_kill_pays_the_lethal_hit_and_a_chain_bonus() {
        let mut g = game(&[[1000.0, 0.0, 0.0], [1100.0, 0.0, 0.0]]);
        score_like_the_rom(&mut g);
        for i in 0..5 {
            assert!(g.coop_spawn_test_ai(b"plain", [0.0, 0.0, 0.0], 100 + i));
        }
        g.take_out();
        let window = g.cfg.multi_kill_window_ms;
        // a body-shot kill alone: 24 + 9
        g.ai_damage(0, 1, 1000.0, TORSO, 1000);
        assert_eq!(g.coop_points_of(1), 33);
        // a second kill inside the window: + 18
        g.ai_damage(1, 1, 1000.0, TORSO, 1500);
        assert_eq!(g.coop_points_of(1), 33 + 51);
        // after the window the chain starts again: no bonus (a head kill alone: 54 + the head hit's 6)
        g.ai_damage(2, 1, 1000.0, gs::KILL_ZONE_HEAD, 1500 + window + 1);
        assert_eq!(g.coop_points_of(1), 33 + 51 + 60);
        // a head kill inside the window of that one: 60 + 18
        g.ai_damage(3, 1, 1000.0, gs::KILL_ZONE_HEAD, 1500 + window + 2);
        assert_eq!(g.coop_points_of(1), 33 + 51 + 60 + 78);
        // another player's chain is its own; a hit that does not kill pays the zone points only
        g.ai_damage(4, 2, 1.0, TORSO, 1500 + window + 3);
        assert_eq!(g.coop_points_of(2), 9);
        g.ai_damage(4, 2, 1000.0, TORSO, 1500 + window + 4);
        assert_eq!(g.coop_points_of(2), 9 + 33);
    }

    /// The running total of 0xd7 sub 1 follows the kill's points (killer, AI object, gained, total).
    #[test]
    fn the_kill_message_carries_the_kill_points_and_the_total() {
        let mut g = game(&[[1000.0, 0.0, 0.0]]);
        score_like_the_rom(&mut g);
        for i in 0..2 {
            assert!(g.coop_spawn_test_ai(b"plain", [0.0, 0.0, 0.0], 100 + i));
        }
        g.take_out();
        let sums = |out: &[GameOut]| -> Vec<(u32, u32)> {
            out.iter()
                .filter_map(|x| match x {
                    GameOut::To(1, m) if m.as_bytes()[1] == gs::GS_COOP => {
                        let mut r = m.reader();
                        r.read_u8();
                        r.read_u8();
                        (r.read_u8() == 1).then(|| {
                            r.read_u32();
                            r.read_u16();
                            (r.read_u32(), r.read_u32())
                        })
                    }
                    _ => None,
                })
                .collect()
        };
        g.ai_damage(0, 1, 1000.0, TORSO, 1000);
        assert_eq!(sums(&g.take_out()), [(33, 33)]);
        g.ai_damage(1, 1, 1000.0, TORSO, 1200);
        assert_eq!(sums(&g.take_out()), [(51, 84)]);
    }
}
