//! Weapons dropped at a death (DSV-93, DeCombat rules on the 2009 wire forms): the victim's weapon falls at the body
//! as a type-0x17 kind-6 pickup object every client draws with the weapon's third-person model; a living player who
//! comes close takes it (the rounds of a weapon he carries, or the weapon into his empty slot); a drop nobody takes
//! is removed after its lifetime, and every drop is removed at the round end.
//!
//! What the 2009 client shows and accepts is the reference stack's record (REFERENCE, accepted by the real client, rebuilt bit
//! for bit in `round2_rom`); which weapon drops, where, for how long, who may take it and what the taker gets are
//! ours: no capture shows a player taking a dropped weapon, and the client sends nothing for a pickup.
// SPEC: GAME_MODES_2009.md §4.4 (pickup kind 6 `[item:u16][b:u16]`); GAME_MESSAGES_2009_S2C.md §4.4 (0xb2), §8.2
// row 0x17; GAME_MESSAGES_2009_C2S.md §1.3 (the send-site inventory has no pickup request; 0x59 / 0xd8 target the
// mode zones of GAME_MODES_2009.md §4.3 / §4.5, not pickups)
// DISASM: cshell+0x282490 (pickup object init: kind 6 looks the weapon row up, hides a piece when the rounds are 0),
// cshell+0x282670 (its per-frame update: spin, fall, FX visibility; no message is sent), cshell+0x2828e0 (its server
// messages: sub 1 FX, sub 2 countdown)
// OBSERVED: capture 20261025_065016_tdm6 c5 / c6 (7 deaths: one kind-6 pickup at the body per
// death, the weapon the victim held; the reference stack also created the victim's other weapons and six kind-0 pickups
// parked at (-9999.9, -9999.8, -9999.9), never shown; the drop at the body was removed 20.22-20.33 s after its
// create, the parked ones with the corpse at 18.8 s; nobody took one); 20261025_002236_tdm4 c5 / c6 (one death, the
// same form, removed after 20.31 s); REFERENCE
// DC-DEFAULT: DSV-93..DSV-95 (proposed)

use super::{len3, Game, GameOut, Life, RoundPhase, WeaponLook, WeaponState};
use crate::gs;
use dc_net_engine::time::{later, since};
use dc_net_engine::Millis;

/// Weapon slots whose weapons never drop: the melee and the throwable slot.
// SPEC: PROTOCOL_2009.md §3.3 (equipment slots 0..3: primary, secondary, melee, throwable)
// DC-DEFAULT: DSV-93 (proposed)
pub const NO_DROP_SLOTS: [u8; 2] = [2, 3];
/// Height of a drop above the floor below the victim (the client lets the object fall onto the floor itself).
// DC-DEFAULT: DSV-93 (proposed) our value; the reference stack's drops lay 8-16 units above the floor (OBSERVED only)
pub const DROP_FLOOR_LIFT: f32 = 16.0;
/// How far below the victim the floor is searched.
pub const DROP_FLOOR_SEARCH: f32 = 512.0;

/// The `drops.*` rule values (`-drops key=value[,key=value...]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropTuning {
    /// A death drops the victim's weapon (`drops.enabled`).
    // DC-DEFAULT: DSV-93 (proposed) on
    pub enabled: bool,
    /// Lifetime of a drop from its create (`drops.lifetime_ms`, 0 = until the round ends).
    // DC-DEFAULT: DSV-95 (proposed) 30 s (our value; the reference stack removed its drops after about 20.3 s, OBSERVED only)
    pub lifetime_ms: u32,
    /// A drop can be taken this long after its create (the client shows it falling first).
    // DC-DEFAULT: DSV-94 (proposed) 1000 ms
    pub arm_ms: u32,
    /// A living player takes a drop when the drop point is this close to his box (`drops.pickup_radius`, 0 = only
    /// inside the box).
    // DC-DEFAULT: DSV-94 (proposed) 32 units
    pub pickup_radius: f32,
    /// Drops in the world at once; the oldest goes first (`drops.max_active`, 0 = no drops).
    // DC-DEFAULT: DSV-95 (proposed) 16
    pub max_active: u32,
    /// A use request (`UP_USE` on a drop, DSV-170) is honoured when the drop point is this close to the player's box
    /// (`drops.use_radius`).
    // DC-DEFAULT: DSV-170 (proposed) 48 units
    pub use_radius: f32,
}

impl Default for DropTuning {
    fn default() -> Self {
        DropTuning {
            enabled: true,
            lifetime_ms: 30_000,
            arm_ms: 1_000,
            pickup_radius: 32.0,
            max_active: 16,
            use_radius: 48.0,
        }
    }
}

/// The `drops.*` keys.
pub const DROP_KEYS: [&str; 6] = [
    "enabled",
    "lifetime_ms",
    "arm_ms",
    "pickup_radius",
    "max_active",
    "use_radius",
];

impl DropTuning {
    /// Sets one `drops.*` key (the prefix is optional).
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let k = key.trim();
        let k = k.strip_prefix("drops.").unwrap_or(k);
        let v = value.trim();
        let bad = || format!("drops.{k}: bad value {v:?}");
        let int = || v.parse::<u32>().map_err(|_| bad());
        match k {
            "enabled" => {
                self.enabled = match v.to_ascii_lowercase().as_str() {
                    "on" | "1" | "true" => true,
                    "off" | "0" | "false" => false,
                    _ => return Err(bad()),
                }
            }
            "lifetime_ms" => self.lifetime_ms = int()?.min(3_600_000),
            "arm_ms" => self.arm_ms = int()?.min(60_000),
            "pickup_radius" | "use_radius" => {
                let r = v
                    .parse::<f32>()
                    .ok()
                    .filter(|x| x.is_finite() && *x >= 0.0)
                    .ok_or_else(bad)?;
                if k == "pickup_radius" {
                    self.pickup_radius = r
                } else {
                    self.use_radius = r
                }
            }
            "max_active" => self.max_active = int()?.min(256),
            _ => {
                return Err(format!(
                    "unknown key drops.{k} (known: {})",
                    DROP_KEYS.join(", ")
                ))
            }
        }
        Ok(())
    }
}

/// A drop the session creates on every client in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct DropOut {
    /// The drop key (the session maps it to an object id).
    pub key: u32,
    pub look: WeaponLook,
    /// Rounds the blob shows (clip and stock, capped at 0xFFFF).
    pub rounds: u16,
    pub pos: [f32; 3],
    pub yaw: f32,
}

/// One drop in the world.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Drop {
    pub key: u32,
    /// The weapon as the victim carried it (clip and stock included).
    pub weapon: WeaponState,
    /// The floor point the drop rests on (pickup distance).
    pub rest: [f32; 3],
    pub created: Millis,
    pub expires: Option<Millis>,
}

/// What a player gets from a drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Take {
    /// Rounds into the weapon of this index of the player's weapons (the same weapon).
    Rounds { index: usize, rounds: u32 },
    /// The weapon into this empty slot.
    Weapon { slot: u8 },
    /// The drop's weapon in place of the different weapon of this index (the same slot), which drops at the player
    /// (a use request only, DSV-170).
    Swap { index: usize },
}

/// Distance from a point to an axis-aligned box (centre, half extents); 0 inside.
fn box_distance(p: [f32; 3], centre: [f32; 3], half: [f32; 3]) -> f32 {
    let d: [f32; 3] = std::array::from_fn(|i| ((p[i] - centre[i]).abs() - half[i].abs()).max(0.0));
    len3(d)
}

/// The weapon that drops from a player's weapons: the drawn one when it may drop, else the first one in slot order
/// that may (not melee, not throwable, not its own ammunition); `None` when none may.
// DC-DEFAULT: DSV-93 (proposed)
pub fn dropped_weapon(weapons: &[WeaponState], current: Option<usize>) -> Option<usize> {
    let may = |w: &WeaponState| !NO_DROP_SLOTS.contains(&w.slot) && !w.own_ammo;
    if let Some(i) = current.filter(|&i| weapons.get(i).is_some_and(may)) {
        return Some(i);
    }
    let mut order: Vec<usize> = (0..weapons.len()).filter(|&i| may(&weapons[i])).collect();
    order.sort_by_key(|&i| weapons[i].slot);
    order.first().copied()
}

/// What a player with `weapons` gets from a drop of `w`: the drop's rounds into the same weapon he carries (up to
/// `stock_cap` in its stock, the ROM's granted amount), else the weapon into its slot when that slot is empty and
/// the drop holds at least a full clip (a weapon given by 0xb2 starts with a full clip on the 2009 client); `None`
/// (the drop stays) when he would gain nothing. A held weapon is never replaced.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0xb2: an identical weapon in the slot is kept and its ammunition count set,
// cshell+0x22da30; another weapon in the slot is replaced by a new object)
// DC-DEFAULT: DSV-94 (proposed)
pub fn take_of(weapons: &[WeaponState], w: &WeaponState, stock_cap: Option<u32>) -> Option<Take> {
    let rounds = w.carried();
    if let Some(index) = weapons.iter().position(|h| h.weapon == w.weapon) {
        let have = weapons[index].stock;
        let room = stock_cap.map_or(u32::MAX, |c| c.saturating_sub(have));
        let n = rounds.min(room);
        return (n > 0).then_some(Take::Rounds { index, rounds: n });
    }
    let full = w.clip_size.max(1);
    if weapons.iter().any(|h| h.slot == w.slot) || rounds < full {
        return None;
    }
    Some(Take::Weapon { slot: w.slot })
}

/// What a player with `weapons` gets from a drop of `w` when he asks for it with the use key (DSV-170): the rounds of
/// the same weapon as [`take_of`] gives them; a primary drop (slot 0 or a backpack slot) in place of the drawn
/// weapon (`current`) when that one is a primary too (the slot-0 gun or a backpack gun held out, DSV-301); the
/// weapon into its empty slot whatever its rounds; else the weapon in place of the different weapon he holds in
/// that slot (a swap). `None` when he would gain nothing.
// DC-DEFAULT: DSV-170 (proposed) the use key takes a drop; the 2009 client has no pickup request (C2S §1.3), so the
// request form and this rule are ours
// DC-DEFAULT: DSV-301 (proposed, playtest report "it should swap the currently active primary gun not just the one in
// your first slot") a primary drop replaces the drawn primary
pub fn use_of(
    weapons: &[WeaponState],
    current: Option<usize>,
    w: &WeaponState,
    stock_cap: Option<u32>,
) -> Option<Take> {
    if weapons.iter().any(|h| h.weapon == w.weapon) {
        return take_of(weapons, w, stock_cap);
    }
    if is_primary_slot(w.slot) {
        if let Some(index) =
            current.filter(|&i| weapons.get(i).is_some_and(|h| is_primary_slot(h.slot)))
        {
            return Some(Take::Swap { index });
        }
    }
    match weapons.iter().position(|h| h.slot == w.slot) {
        None => Some(Take::Weapon { slot: w.slot }),
        Some(index) => Some(Take::Swap { index }),
    }
}

/// A primary weapon slot: slot 0 or one of the backpack weapon slots (DSV-38).
// SPEC: PROTOCOL_2009.md §3.3 (slot 0 primary); DC-DEFAULT: DSV-38 (backpack weapon slots 4..7)
pub fn is_primary_slot(slot: u8) -> bool {
    slot == 0
        || (super::BACKPACK_WEAPON_FIRST..super::BACKPACK_WEAPON_FIRST + super::BACKPACK_SLOTS)
            .contains(&slot)
}

impl Game {
    /// Drops in the world (tests and tools): `(key, weapon, rounds, rest point)`.
    pub fn weapon_drops(&self) -> Vec<(u32, u16, u32, [f32; 3])> {
        self.drops
            .iter()
            .map(|d| (d.key, d.weapon.weapon, d.weapon.carried(), d.rest))
            .collect()
    }

    /// The death of `vpid`: its weapon drops at the body (DSV-93): only while the round runs, not in co-op, with a
    /// weapon that may drop and a third-person model in the ROM weapon row.
    // DC-DEFAULT: DSV-93 (proposed)
    pub(super) fn drop_on_death(&mut self, vpid: u8, now: Millis) {
        let t = self.cfg.drops;
        if !t.enabled
            || t.max_active == 0
            || !matches!(self.phase, RoundPhase::Running { .. })
            || self.is_coop()
            // DC-DEFAULT: DSV-481 (proposed) Gun Game: no drops
            || self.is_gun_game()
            // DC-DEFAULT: DSV-483 (proposed) Possession: the guns go to the killer, no pickups
            || self.is_possession()
        {
            return;
        }
        let Some(p) = self.players.get(&vpid) else {
            return;
        };
        let Some(i) = dropped_weapon(&p.weapons, p.current) else {
            return;
        };
        let w = p.weapons[i];
        let (at, yaw) = (p.pos, p.yaw);
        let dims = self.dims_of(vpid);
        self.drop_weapon_at(w, vpid, at, yaw, dims, now);
    }

    /// `w` (of client `from`) falls as a drop at the floor below `at` (a death, DSV-93, or a swap, DSV-170).
    // DC-DEFAULT: DSV-93 (proposed)
    fn drop_weapon_at(
        &mut self,
        w: WeaponState,
        from: u8,
        at: [f32; 3],
        yaw: f32,
        dims: [f32; 3],
        now: Millis,
    ) {
        let t = self.cfg.drops;
        let Some(look) = self.weapon_look(w.weapon) else {
            self.log(format!(
                "drops: weapon {} has no third-person model, no drop",
                w.weapon
            ));
            return;
        };
        // the oldest drops go first when the world is full
        while self.drops.len() as u32 >= t.max_active {
            let key = self.drops.remove(0).key;
            self.out.push(GameOut::DropGone { key });
        }
        let rest = self.drop_rest(at, dims);
        let pos = [rest[0], rest[1] + DROP_FLOOR_LIFT, rest[2]];
        let key = self.next_drop_key;
        self.next_drop_key = self.next_drop_key.wrapping_add(1).max(1);
        let rounds = w.carried().min(u32::from(u16::MAX)) as u16;
        self.out.push(GameOut::DropCreate(Box::new(DropOut {
            key,
            look,
            rounds,
            pos,
            yaw,
        })));
        self.log(format!(
            "drops: drop {key}: weapon {} with {} rounds from client {from} at ({:.0}, {:.0}, {:.0})",
            w.weapon,
            w.carried(),
            pos[0],
            pos[1],
            pos[2]
        ));
        self.drops.push(Drop {
            key,
            weapon: w,
            rest,
            created: now,
            expires: (t.lifetime_ms > 0).then(|| now.wrapping_add(t.lifetime_ms)),
        });
    }

    /// The GMS weapon rows of co-op round `round` (DSV-583): each row's weapon lies at its item spawn point as a drop
    /// (the type-0x17 kind-6 object of DSV-93) with a full clip and the ROM's stock, for the row's lifetime (else the
    /// drop rules'). Taking it follows the drop rules (DSV-94, the use key DSV-170).
    // OBSERVED: ROM table key GMS [WeaponSpawn<n>] Round, WeaponID, LifeTime, Position (five rows of the round map:
    // rounds 11 / 13 at one item spawn point, 16 / 18 / 20 at the other, 180 s each)
    // DC-DEFAULT: DSV-583 (proposed)
    pub(super) fn coop_weapon_rows(&mut self, round: i32, now: Millis) {
        let rows: Vec<crate::coop_rom::WeaponSpawnRow> = match self.coop.as_ref() {
            Some(c) => c
                .plan
                .weapon_spawns
                .iter()
                .filter(|r| r.round == round)
                .cloned()
                .collect(),
            None => return,
        };
        if rows.is_empty() || self.cfg.drops.max_active == 0 {
            return;
        }
        for r in rows {
            let Some(at) = self
                .coop_world
                .item_points
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(r.place.as_bytes()))
                .map(|p| (p.pos, p.yaw))
            else {
                self.log(format!(
                    "co-op: weapon row of round {round}: no item spawn point {:?}",
                    r.place
                ));
                continue;
            };
            let slot = self
                .tables
                .items
                .weapon_types
                .get(&r.weapon)
                .and_then(|t| dc_bute::arms::slot_class(*t))
                .unwrap_or(0);
            let w = WeaponState {
                slot,
                ..self.new_weapon(r.weapon, None)
            };
            let key = self.next_drop_key;
            self.drop_weapon_at(w, 0, at.0, at.1, [0.0; 3], now);
            if let Some(d) = self.drops.iter_mut().find(|d| d.key == key) {
                if let Some(s) = r.life_s {
                    d.expires = Some(now.wrapping_add((s * 1000.0).min(3_600_000.0) as u32));
                }
                if let Some(c) = self.coop.as_mut() {
                    c.show.weapon_drops += 1;
                }
                self.log(format!(
                    "co-op: round {round}: weapon {} lies at {:?}",
                    r.weapon, r.place
                ));
            }
        }
    }

    /// The floor point below a victim at `at` with the box `dims` (half extents): the first solid below in the trace
    /// world, else the bottom of the box.
    // DC-DEFAULT: DSV-93 (proposed)
    fn drop_rest(&self, at: [f32; 3], dims: [f32; 3]) -> [f32; 3] {
        match self.wall_along(at, [0.0, -1.0, 0.0], DROP_FLOOR_SEARCH) {
            Some(d) => [at[0], at[1] - d, at[2]],
            None => [at[0], at[1] - dims[1].abs(), at[2]],
        }
    }

    /// Every drop is removed (round end).
    // DC-DEFAULT: DSV-95 (proposed)
    pub(super) fn drops_clear(&mut self) {
        for d in std::mem::take(&mut self.drops) {
            self.out.push(GameOut::DropGone { key: d.key });
        }
    }

    /// Per tick: lifetimes, then pickups (while the round runs).
    // DC-DEFAULT: DSV-94, DSV-95 (proposed)
    pub(super) fn tick_drops(&mut self, now: Millis) {
        if self.drops.is_empty() {
            return;
        }
        let gone: Vec<u32> = self
            .drops
            .iter()
            .filter(|d| d.expires.is_some_and(|t| !later(t, now)))
            .map(|d| d.key)
            .collect();
        for key in gone {
            self.drops.retain(|d| d.key != key);
            self.out.push(GameOut::DropGone { key });
            self.log(format!("drops: drop {key} expired"));
        }
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        let t = self.cfg.drops;
        let keys: Vec<u32> = self
            .drops
            .iter()
            .filter(|d| since(now, d.created) >= t.arm_ms)
            .map(|d| d.key)
            .collect();
        for key in keys {
            let Some(d) = self.drops.iter().find(|d| d.key == key).cloned() else {
                continue;
            };
            let cap = self.tables.weapons.max_amount(d.weapon.ammo);
            let taker = self
                .players
                .values()
                .filter(|p| p.life == Life::Alive && p.introduced)
                .map(|p| (p.pid, box_distance(d.rest, p.pos, self.dims_of(p.pid))))
                .filter(|(_, dist)| dist.is_finite() && *dist <= t.pickup_radius)
                .filter_map(|(pid, dist)| {
                    let p = self.players.get(&pid)?;
                    take_of(&p.weapons, &d.weapon, cap).map(|tk| (pid, dist, tk))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((pid, _, take)) = taker {
                self.drop_take(&d, pid, take, now);
            }
        }
    }

    /// `UP_USE` from `pid` on the drop of `key` (the use key over a dropped weapon, DSV-170): while the round runs, an
    /// armed drop within `use_radius` of the living player's box is taken by [`use_of`]: the rounds, the weapon into
    /// the empty slot, or a swap (the drop's weapon into the slot, the held one falls at the player).
    // DC-DEFAULT: DSV-170 (proposed)
    pub fn drop_use(&mut self, pid: u8, key: u32, now: Millis) {
        let t = self.cfg.drops;
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        let Some(d) = self.drops.iter().find(|d| d.key == key).cloned() else {
            return;
        };
        if since(now, d.created) < t.arm_ms {
            return;
        }
        let Some(p) = self
            .players
            .get(&pid)
            .filter(|p| p.life == Life::Alive && p.introduced)
        else {
            return;
        };
        let dist = box_distance(d.rest, p.pos, self.dims_of(pid));
        if !(dist.is_finite() && dist <= t.use_radius) {
            self.log(format!(
                "drops: use of drop {key} by client {pid} refused: {dist:.0} units away"
            ));
            return;
        }
        let cap = self.tables.weapons.max_amount(d.weapon.ammo);
        if let Some(take) = use_of(&p.weapons, p.current, &d.weapon, cap) {
            self.drop_take(&d, pid, take, now);
        }
    }

    /// `pid` takes drop `d`: the rounds or the weapon by 0xb2 (the 2009 client keeps an identical weapon and sets its
    /// count, or fills the empty slot, with its pickup notice), the object removed from every client.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0xb2 `[weapon][ammo_kind][ammo:f32][class_pos][slot][parts]`; the
    // ammunition set through status +0x38 with the pickup-notice flag 1)
    // DC-DEFAULT: DSV-94 (proposed)
    fn drop_take(&mut self, d: &Drop, pid: u8, take: Take, now: Millis) {
        let parts_on = self.cfg.weapon_parts;
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        let mut swapped: Option<WeaponState> = None;
        let index = match take {
            Take::Rounds { index, rounds } => {
                let h = &mut p.weapons[index];
                h.stock = h.stock.saturating_add(rounds);
                index
            }
            Take::Weapon { slot } => {
                let mut w = d.weapon;
                // a plain take is sent with the row's default parts (below), so its clip follows those parts, not
                // the dropper's magazine mod: the client derives the clip from the parts it is told (review of
                // mod-effects, 2026-10-30; DC-DEFAULT: CLI-1440 (proposed) amended)
                if w.clip_size > 0 && !w.own_ammo {
                    let parts = if parts_on {
                        self.tables.weapons.default_parts(w.weapon)
                    } else {
                        gs::NO_PARTS
                    };
                    let (clip_plus, _) = self.tables.weapons.magazine_bonus(w.weapon, &parts);
                    let base = self.tables.weapons.clip(w.weapon).unwrap_or(w.clip_size);
                    w.clip_size = u32::try_from((i64::from(base) + i64::from(clip_plus)).max(1))
                        .unwrap_or(u32::MAX);
                }
                let all = w.carried();
                w.clip = w.clip_size.min(all);
                w.stock = all - w.clip;
                w.slot = slot;
                p.weapons.push(w);
                p.weapons.len() - 1
            }
            Take::Swap { index } => {
                // DC-DEFAULT: DSV-170 (proposed) the drop's weapon takes the held weapon's place with all its rounds
                // (a full clip when it has one) and the parts it dropped with
                let mut w = d.weapon;
                let all = w.carried();
                w.clip = if w.clip_size == 0 {
                    all
                } else {
                    w.clip_size.min(all)
                };
                w.stock = all - w.clip;
                w.slot = p.weapons[index].slot;
                swapped = Some(std::mem::replace(&mut p.weapons[index], w));
                index
            }
        };
        let weapons = p.weapons.clone();
        let w = &weapons[index];
        let class = self.tables.weapons.class_of(w.weapon);
        let class_pos = class.map_or(0, |c| {
            weapons
                .iter()
                .filter(|x| x.slot < w.slot && self.tables.weapons.class_of(x.weapon) == Some(c))
                .count()
                .min(32) as u8
        });
        let spec = gs::WeaponSpec {
            weapon: w.weapon,
            ammo_kind: w.ammo,
            ammo: w.told_amount(),
            clip: w.clip,
            class_pos,
            slot: w.slot,
            parts: if !parts_on {
                gs::NO_PARTS
            } else if matches!(take, Take::Swap { .. }) && w.parts != gs::NO_PARTS {
                w.parts
            } else {
                self.tables.weapons.default_parts(w.weapon)
            },
        };
        // A same-weapon take only tops the count up: 0x66 kind 1 (status +0x38 with the pickup-notice flag 0) sets it
        // without the weapon change that the 0xb2 pickup notice runs (cshell+0x1f0700 -> ChangeWeapon, whose 0x7f
        // makes every client play the taker's select one-shot: the "pickup animation" over a gun he already holds).
        // DISASM: cshell+0x63106..0x63120 (status +0x38 calls cshell+0x1f0700 only when its pickup flag is set)
        // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (0x66 kind 1 passes the flag 0), §4.4 (0xb2 passes 1)
        // DC-DEFAULT: CLI-581 (proposed) amends DSV-94: rounds by 0x66 kind 1, a new weapon by 0xb2
        // DC-DEFAULT: DSV-167 (proposed) a taken weapon fires with the parts it was granted with
        if let (Take::Weapon { .. } | Take::Swap { .. }, Some(p)) =
            (take, self.players.get_mut(&pid))
        {
            p.weapons[index].parts = spec.parts;
        }
        let m = match take {
            Take::Rounds { .. } => gs::player_ammo(
                w.weapon,
                w.ammo,
                w.told_amount(),
                w.slot,
                super::arrival::TODO_SLOT_STATUS_EXTRA,
            ),
            Take::Weapon { .. } | Take::Swap { .. } => gs::weapon_give(&spec),
        };
        self.send(pid, m);
        self.drops.retain(|x| x.key != d.key);
        self.out.push(GameOut::DropGone { key: d.key });
        self.log(format!(
            "drops: drop {} (weapon {}) taken by client {pid}: {take:?}",
            d.key, d.weapon.weapon
        ));
        // the swapped weapon falls where the taker stands, with its rounds (DSV-170)
        if let Some(old) = swapped {
            if let Some(p) = self.players.get(&pid) {
                let (at, yaw) = (p.pos, p.yaw);
                let dims = self.dims_of(pid);
                self.drop_weapon_at(old, pid, at, yaw, dims, now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(weapon: u16, slot: u8, clip: u32, stock: u32) -> WeaponState {
        WeaponState {
            weapon,
            ammo: 1,
            clip_size: 30,
            clip,
            stock,
            slot,
            own_ammo: false,
            parts: gs::NO_PARTS,
            shot_open: false,
            shot_ok: false,
        }
    }

    #[test]
    fn keys_set_and_refuse() {
        let mut t = DropTuning::default();
        t.set("drops.lifetime_ms", "5000").unwrap();
        t.set("pickup_radius", "48").unwrap();
        t.set("enabled", "off").unwrap();
        assert_eq!(
            (t.lifetime_ms, t.pickup_radius, t.enabled),
            (5000, 48.0, false)
        );
        assert!(t.set("nope", "1").is_err());
        assert!(t.set("pickup_radius", "-1").is_err());
    }

    /// DSV-93: the drawn weapon drops, unless it is a melee or throwable weapon; then the first weapon in slot order
    /// that may drop.
    #[test]
    fn the_drawn_weapon_drops_else_the_first_firearm() {
        let w = vec![
            ws(5, 1, 10, 20),
            ws(3, 0, 30, 90),
            ws(9, 2, 1, 0),
            ws(7, 3, 1, 0),
        ];
        assert_eq!(dropped_weapon(&w, Some(0)), Some(0));
        assert_eq!(
            dropped_weapon(&w, Some(2)),
            Some(1),
            "knife drawn: the primary"
        );
        assert_eq!(dropped_weapon(&w, None), Some(1));
        assert_eq!(dropped_weapon(&w[2..], Some(0)), None);
    }

    /// DSV-94: rounds into the same weapon (capped by the stock cap), the weapon into an empty slot only with a full
    /// clip, nothing when the slot is held by another weapon.
    #[test]
    fn what_a_taker_gets() {
        let drop = ws(3, 0, 12, 40);
        let same = vec![ws(3, 0, 30, 70)];
        assert_eq!(
            take_of(&same, &drop, Some(90)),
            Some(Take::Rounds {
                index: 0,
                rounds: 20
            })
        );
        assert_eq!(take_of(&same, &drop, Some(70)), None, "stock full");
        assert_eq!(
            take_of(&same, &drop, None),
            Some(Take::Rounds {
                index: 0,
                rounds: 52
            })
        );
        let other = vec![ws(4, 0, 30, 90)];
        assert_eq!(take_of(&other, &drop, Some(90)), None, "slot held");
        let secondary_only = vec![ws(4, 1, 30, 90)];
        assert_eq!(
            take_of(&secondary_only, &drop, Some(90)),
            Some(Take::Weapon { slot: 0 })
        );
        assert_eq!(
            take_of(&secondary_only, &ws(3, 0, 10, 5), Some(90)),
            None,
            "less than a full clip"
        );
    }

    /// DSV-170: the use key takes the rounds of the same weapon, the weapon into an empty slot whatever its rounds,
    /// and swaps it for the different weapon of its slot.
    #[test]
    fn what_a_use_gets() {
        let drop = ws(3, 0, 10, 5);
        let use_of = |w: &[WeaponState], d: &WeaponState, c| use_of(w, None, d, c);
        assert_eq!(
            use_of(&[ws(3, 0, 30, 70)], &drop, Some(90)),
            Some(Take::Rounds {
                index: 0,
                rounds: 15
            })
        );
        assert_eq!(use_of(&[ws(3, 0, 30, 90)], &drop, Some(90)), None);
        assert_eq!(
            use_of(&[ws(4, 1, 30, 90)], &drop, Some(90)),
            Some(Take::Weapon { slot: 0 })
        );
        assert_eq!(
            use_of(&[ws(4, 1, 30, 90), ws(5, 0, 30, 90)], &drop, Some(90)),
            Some(Take::Swap { index: 1 })
        );
    }

    /// DSV-301: with a backpack gun drawn (slot 4) a primary drop swaps that one, not the gun in slot 0; with the
    /// pistol drawn the slot rule stays.
    #[test]
    fn a_use_swaps_the_drawn_primary() {
        let drop = ws(3, 0, 10, 5);
        let held = [ws(5, 0, 30, 90), ws(4, 1, 12, 36), ws(6, 4, 30, 90)];
        assert_eq!(
            use_of(&held, Some(2), &drop, Some(90)),
            Some(Take::Swap { index: 2 }),
            "the backpack rifle in the hand"
        );
        assert_eq!(
            use_of(&held, Some(0), &drop, Some(90)),
            Some(Take::Swap { index: 0 })
        );
        assert_eq!(
            use_of(&held, Some(1), &drop, Some(90)),
            Some(Take::Swap { index: 0 }),
            "a pistol in the hand: the slot rule"
        );
    }

    #[test]
    fn distance_to_the_box() {
        assert_eq!(box_distance([0.0; 3], [0.0; 3], [10.0; 3]), 0.0);
        assert!((box_distance([20.0, 0.0, 0.0], [0.0; 3], [10.0; 3]) - 10.0).abs() < 1e-6);
        assert!(
            (box_distance([0.0, -63.0, 0.0], [0.0; 3], [15.0, 50.0, 15.0]) - 13.0).abs() < 1e-4
        );
    }
}
