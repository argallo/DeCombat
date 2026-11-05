//! What a dead co-op AI drops (DSV-591, DSV-592): its brain row's drop rows are rolled at the death, each item that
//! falls lies at the body as a pickup object every client draws, and a living player who walks over it takes it:
//! a health pack heals him fully, a standard ammunition pack refills the reserve of his primary and secondary guns,
//! a heavy ammunition case the reserve of every weapon and thrown item, a grenade adds one throw. The 2009 client
//! shows the drop-item note of its co-op HUD for the first three (0xc9 sub 6).
//!
//! The drop rows (chance and item word per row) and the pack models are the ROM's, read at run time; how the rows
//! are rolled, what each item gives and the object form are ours: no capture shows a pack. The owner's December 2009
//! Cabin Fever recording shows the three packs of the Cabin Fever brains (a small standard ammunition pack, a larger
//! heavy ammunition case, a health pack that restores health fully).
// OBSERVED: ROM table key AIBUTES.TXT [Brain<n>] DropItem<k> / DropItemType<k> (four item words: hashes below; the
// Desert Thunder brains four rows, the Cabin Fever brains two or three rows without a grenade row; chances read at
// run time), PROPS archive folder of three drop-item models with a texture each (hashes below), read at run time
// OBSERVED: owner description of a December 2009 Cabin Fever recording (Normal), 2026-11-04: three pickup kinds, a
// small light "standard ammo" pack (assault-rifle / SMG ammunition), a larger reddish "heavy ammo" case (ammunition
// across the weapon and item categories), a greenish health pack that restores health fully
// SPEC: GAME_MESSAGES_2009_S2C.md §6.2 (0xc9 sub 6 `[pickup:u8]`), §8.2 row 0x17, §4.3 (0x66 kinds 1 / health), §4.4
// (0xb2)
// DC-DEFAULT: DSV-591, DSV-592 (proposed)

use super::drops::{box_distance, is_primary_slot, DropOut, DROP_FLOOR_LIFT};
use super::{Game, GameOut, Life, RoundPhase, WeaponLook, WeaponState};
use crate::gs;
use crate::world::PropHash;
use dc_net_engine::time::{later, since};
use dc_net_engine::Millis;

/// What an AI pack gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackKind {
    /// Health to its maximum.
    Health,
    /// The reserve of the primary and secondary guns.
    Ammo,
    /// The reserve of every weapon, thrown items included.
    HeavyAmmo,
    /// One throw more of the thrown weapon (a grenade of the default kit when the player has none).
    Grenade,
}

/// How a brain row's drop rows are rolled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiRoll {
    /// Each row on its own: an AI can drop several items.
    Each,
    /// One roll over the rows in order: at most one item.
    One,
}

impl AiRoll {
    pub fn from_key(v: &str) -> Option<AiRoll> {
        match v.to_ascii_lowercase().as_str() {
            "each" => Some(AiRoll::Each),
            "one" => Some(AiRoll::One),
            _ => None,
        }
    }
}

/// What an AI's death drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiForce {
    /// The brain row's drop rows.
    Rom,
    /// Nothing.
    Nothing,
    /// This item at every death.
    Kind(PackKind),
}

impl AiForce {
    pub fn from_key(v: &str) -> Option<AiForce> {
        Some(match v.to_ascii_lowercase().as_str() {
            "rom" => AiForce::Rom,
            "none" => AiForce::Nothing,
            "health" => AiForce::Kind(PackKind::Health),
            "ammo" => AiForce::Kind(PackKind::Ammo),
            "heavy_ammo" => AiForce::Kind(PackKind::HeavyAmmo),
            "grenade" => AiForce::Kind(PackKind::Grenade),
            _ => return None,
        })
    }
}

fn key_of(w: &[u8]) -> PropHash {
    let l = w.to_ascii_lowercase();
    (dc_world::hash16(&l), l.len())
}

/// The item words of the drop rows (hash of the lower-case word and its length).
// OBSERVED: ROM AIBUTES.TXT DropItemType<k> values (hashes only)
const WORD_AMMO: PropHash = ([0x46, 0xc6, 0x48, 0x2d, 0xd3, 0x2b, 0xca, 0xe7], 8);
const WORD_HEAVY_AMMO: PropHash = ([0xbd, 0x8c, 0x70, 0x9a, 0x4b, 0xf4, 0x21, 0x86], 9);
const WORD_HEALTH: PropHash = ([0xcd, 0xfd, 0xd3, 0x6a, 0xc7, 0x4c, 0x6d, 0x33], 7);
const WORD_GRENADE: PropHash = ([0x94, 0x91, 0x6c, 0x54, 0x1b, 0x4a, 0x20, 0xde], 7);

/// The pack an item word names (`None`: a word we do not know).
pub fn pack_kind(word: &str) -> Option<PackKind> {
    let k = key_of(word.trim().as_bytes());
    [
        (WORD_HEALTH, PackKind::Health),
        (WORD_AMMO, PackKind::Ammo),
        (WORD_HEAVY_AMMO, PackKind::HeavyAmmo),
        (WORD_GRENADE, PackKind::Grenade),
    ]
    .into_iter()
    .find(|(w, _)| *w == k)
    .map(|(_, p)| p)
}

/// The items of one death: `table` holds `(item, chance in percent)` in row order, `roll` gives a value in [0, 100).
/// [`AiRoll::Each`]: every row whose roll falls below its chance; [`AiRoll::One`]: the row whose share of one roll
/// covers it (the rest of 100 is nothing).
// DC-DEFAULT: DSV-591 (proposed) the chance is a percentage of one death; the roll form is ours
pub fn roll_drops(
    table: &[(PackKind, f32)],
    mode: AiRoll,
    roll: &mut impl FnMut() -> f32,
) -> Vec<PackKind> {
    match mode {
        AiRoll::Each => table
            .iter()
            .filter(|(_, c)| roll() < *c)
            .map(|(k, _)| *k)
            .collect(),
        AiRoll::One => {
            let r = roll();
            let mut upto = 0.0;
            for (k, c) in table {
                upto += c.max(0.0);
                if r < upto {
                    return vec![*k];
                }
            }
            Vec::new()
        }
    }
}

/// The archive and the models of the three packs (hash of the lower-case archive stem, of the lower-case entry
/// path).
// OBSERVED: ROM PROPS archive, its drop-item folder (three models, each with a texture whose name ends with the
// model's name; hashes only)
const PACK_ARCHIVE: PropHash = ([0xb2, 0x64, 0x80, 0x77, 0x09, 0x57, 0x8e, 0x5c], 5);
const MODEL_HEALTH: PropHash = ([0x3b, 0xb9, 0x80, 0x0a, 0xf8, 0x18, 0xfa, 0xe2], 32);
const MODEL_AMMO: PropHash = ([0x62, 0x14, 0xdb, 0x2f, 0x44, 0xe8, 0xb5, 0xb0], 35);
const MODEL_HEAVY_AMMO: PropHash = ([0xa4, 0x8f, 0xa9, 0x54, 0x4c, 0x61, 0x80, 0xaf], 36);
/// Texture extension of the ROM's model skins.
// SPEC: FMT_DTX.md (texture file)
const TEXTURE_EXT: &str = ".dtx";

/// The models of the three packs (`None`: not in the ROM).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PackLooks {
    pub health: Option<WeaponLook>,
    pub ammo: Option<WeaponLook>,
    pub heavy: Option<WeaponLook>,
}

impl PackLooks {
    pub fn of(&self, kind: PackKind) -> Option<&WeaponLook> {
        match kind {
            PackKind::Health => self.health.as_ref(),
            PackKind::Ammo => self.ammo.as_ref(),
            PackKind::HeavyAmmo => self.heavy.as_ref(),
            PackKind::Grenade => None,
        }
    }
}

/// The look of the model whose entry path hashes to `key` among `names` (an archive listing): the model and, as its
/// skin, the texture of the same folder whose file stem ends with the model's file stem.
// DC-DEFAULT: DSV-591 (proposed) the skin rule
pub fn look_in(names: &[String], key: PropHash) -> Option<WeaponLook> {
    let model = names
        .iter()
        .find(|n| key_of(n.replace('/', "\\").as_bytes()) == key)?;
    let (dir, file) = model.rsplit_once(['\\', '/']).unwrap_or(("", model));
    let stem = file
        .rsplit_once('.')
        .map_or(file, |s| s.0)
        .to_ascii_lowercase();
    let skin = names.iter().find(|n| {
        let l = n.to_ascii_lowercase();
        let Some((d, f)) = l.rsplit_once(['\\', '/']) else {
            return false;
        };
        d.eq_ignore_ascii_case(dir)
            && f.strip_suffix(TEXTURE_EXT)
                .is_some_and(|s| s.ends_with(&stem))
    });
    Some(WeaponLook {
        weapon: 0,
        model: model.clone(),
        textures: skin.into_iter().cloned().collect(),
        styles: Vec::new(),
    })
}

/// The three pack looks from an archive listing.
pub fn pack_looks_in(names: &[String]) -> PackLooks {
    PackLooks {
        health: look_in(names, MODEL_HEALTH),
        ammo: look_in(names, MODEL_AMMO),
        heavy: look_in(names, MODEL_HEAVY_AMMO),
    }
}

/// The three pack looks of the ROM: the listing of the game folder's archive whose stem hashes to the pack
/// archive's.
// DC-DEFAULT: DSV-591 (proposed)
pub fn pack_looks_from_rom(vfs: &dc_rez::RezVfs) -> PackLooks {
    let names = || -> Option<Vec<String>> {
        let dir = std::fs::read_dir(vfs.rom_dir().join("Game")).ok()?;
        let rez = dir.flatten().find_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let (stem, ext) = n.rsplit_once('.')?;
            (ext.eq_ignore_ascii_case("rez") && key_of(stem.as_bytes()) == PACK_ARCHIVE)
                .then_some(n)
        })?;
        Some(
            vfs.list_entries(&rez)
                .ok()?
                .into_iter()
                .map(|e| e.0)
                .collect(),
        )
    };
    names().map(|n| pack_looks_in(&n)).unwrap_or_default()
}

/// Which weapons a refill fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefillSet {
    /// The guns of the primary slots (slot 0 and the backpack slots) and of the secondary slot 1.
    Guns,
    /// Every weapon but the melee slot's, thrown items included.
    All,
}

/// Equipment slots of the refill sets.
// SPEC: PROTOCOL_2009.md §3.3 (slots 0..3: primary, secondary, melee, throwable)
const SLOT_SECONDARY: u8 = 1;
const SLOT_MELEE: u8 = 2;
pub const SLOT_THROWN: u8 = 3;

/// Fills the reserve of the weapons of `set` to `cap(weapon)`: a gun's stock to the cap, a weapon that is its own
/// ammunition to the cap of every round it carries; the indices of the weapons that gained (nothing goes down).
// DC-DEFAULT: DSV-592 (proposed)
pub fn refill(
    weapons: &mut [WeaponState],
    set: RefillSet,
    cap: impl Fn(&WeaponState) -> u32,
) -> Vec<usize> {
    let mut gained = Vec::new();
    for (i, w) in weapons.iter_mut().enumerate() {
        let wanted = match set {
            RefillSet::Guns => !w.own_ammo && (is_primary_slot(w.slot) || w.slot == SLOT_SECONDARY),
            RefillSet::All => w.slot != SLOT_MELEE,
        };
        if !wanted || (w.clip_size == 0 && !w.own_ammo) {
            continue;
        }
        let c = cap(w);
        let stock = if w.own_ammo {
            c.saturating_sub(w.clip)
        } else {
            c
        };
        if stock > w.stock {
            w.stock = stock;
            gained.push(i);
        }
    }
    gained
}

/// One pack in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    pub key: u32,
    pub kind: PackKind,
    /// The grenade's weapon row (a grenade pack).
    pub weapon: Option<u16>,
    /// The floor point it rests on (pickup distance).
    pub rest: [f32; 3],
    pub created: Millis,
    pub expires: Option<Millis>,
}

/// Where the `i`-th item of one death lies around the body (the first at the body).
// DC-DEFAULT: DSV-591 (proposed) 24 units apart
const PACK_SPACING: f32 = 24.0;
fn spot_of(at: [f32; 3], i: usize) -> [f32; 3] {
    let o = [[0.0, 0.0], [1.0, 0.0], [-1.0, 0.0], [0.0, 1.0], [0.0, -1.0]][i % 5];
    [
        at[0] + o[0] * PACK_SPACING,
        at[1],
        at[2] + o[1] * PACK_SPACING,
    ]
}

impl Game {
    /// The packs in the world (tests and tools): `(key, kind, rest point)`.
    pub fn ai_packs(&self) -> Vec<(u32, PackKind, [f32; 3])> {
        self.packs.iter().map(|p| (p.key, p.kind, p.rest)).collect()
    }

    /// The death of the AI in `slot`: the items its brain row's drop rows roll (or `drops.ai_force`) fall at its body.
    // DC-DEFAULT: DSV-591 (proposed)
    pub(super) fn coop_ai_drops(&mut self, slot: usize, now: Millis) {
        let t = self.cfg.drops;
        if !t.ai_drops || t.max_active == 0 || !self.is_coop() {
            return;
        }
        let Some((rows, pos, dims)) = self.coop_ai_drop_rows(slot) else {
            return;
        };
        let kinds = match t.ai_force {
            AiForce::Nothing => Vec::new(),
            AiForce::Kind(k) => vec![k],
            AiForce::Rom => {
                let mut table = Vec::new();
                for (word, chance) in &rows {
                    match pack_kind(word) {
                        Some(k) => table.push((k, *chance)),
                        None => self.log(format!("co-op: drop row {word:?} names no pack we know")),
                    }
                }
                let mut roll = || (self.next_rand() % 10_000) as f32 / 100.0;
                roll_drops(&table, t.ai_roll, &mut roll)
            }
        };
        for (i, k) in kinds.into_iter().enumerate() {
            self.pack_create(k, spot_of(pos, i), dims, now);
        }
    }

    /// A pack of `kind` falls at the floor below `at` (a body with the box `dims`).
    // DC-DEFAULT: DSV-591 (proposed) a grenade lies as the default kit's thrown weapon (kind 6, one round); the
    // other packs as kind-0 pickups with the ROM's pack models; lifetime and the oldest-first limit of DSV-95
    pub(super) fn pack_create(
        &mut self,
        kind: PackKind,
        at: [f32; 3],
        dims: [f32; 3],
        now: Millis,
    ) {
        let t = self.cfg.drops;
        let (look, weapon, pickup_kind, rounds) = if kind == PackKind::Grenade {
            let Some(w) = self
                .loadout_weapons(None)
                .into_iter()
                .find(|w| w.0 == SLOT_THROWN)
                .map(|w| w.1)
            else {
                self.log("co-op: no thrown weapon in the default kit, no grenade drop".into());
                return;
            };
            let Some(look) = self.weapon_look(w) else {
                self.log(format!(
                    "co-op: weapon {w} has no third-person model, no grenade drop"
                ));
                return;
            };
            (look, Some(w), crate::modelobj::PICKUP_KIND_WEAPON, 1)
        } else {
            let looks = self.pack_looks();
            let Some(look) = looks.of(kind).cloned() else {
                self.log(format!(
                    "co-op: no ROM model for the {kind:?} pack, no drop"
                ));
                return;
            };
            (look, None, crate::modelobj::PICKUP_KIND_PACK, 0)
        };
        while self.packs.len() as u32 >= t.max_active {
            let key = self.packs.remove(0).key;
            self.out.push(GameOut::DropGone { key });
        }
        let rest = self.drop_rest(at, dims);
        let pos = [rest[0], rest[1] + DROP_FLOOR_LIFT, rest[2]];
        let key = self.next_drop_key;
        self.next_drop_key = self.next_drop_key.wrapping_add(1).max(1);
        self.out.push(GameOut::DropCreate(Box::new(DropOut {
            key,
            look,
            rounds,
            pos,
            yaw: 0.0,
            pickup_kind,
        })));
        self.log(format!(
            "co-op: drop {key}: {kind:?} at ({:.0}, {:.0}, {:.0})",
            pos[0], pos[1], pos[2]
        ));
        self.packs.push(Pack {
            key,
            kind,
            weapon,
            rest,
            created: now,
            expires: (t.lifetime_ms > 0).then(|| now.wrapping_add(t.lifetime_ms)),
        });
    }

    /// The pack models, read from the ROM's pack archive on first use.
    fn pack_looks(&mut self) -> PackLooks {
        if let Some(l) = &self.pack_looks {
            return l.clone();
        }
        let looks = self
            .vfs
            .as_ref()
            .map(pack_looks_from_rom)
            .unwrap_or_default();
        self.pack_looks = Some(looks.clone());
        looks
    }

    /// Sets the pack models (tests without a ROM).
    pub fn set_pack_looks(&mut self, looks: PackLooks) {
        self.pack_looks = Some(looks);
    }

    /// Every pack is removed (round end).
    pub(super) fn packs_clear(&mut self) {
        for p in std::mem::take(&mut self.packs) {
            self.out.push(GameOut::DropGone { key: p.key });
        }
    }

    /// Per tick: lifetimes, then walk-over pickups (while the round runs; the nearest living player who gains takes
    /// it).
    // DC-DEFAULT: DSV-592 (proposed) the pickup rules of a weapon drop (DSV-94): armed after `drops.arm_ms`, within
    // `drops.pickup_radius` of the player's box
    pub(super) fn tick_packs(&mut self, now: Millis) {
        if self.packs.is_empty() {
            return;
        }
        let gone: Vec<u32> = self
            .packs
            .iter()
            .filter(|p| p.expires.is_some_and(|t| !later(t, now)))
            .map(|p| p.key)
            .collect();
        for key in gone {
            self.packs.retain(|p| p.key != key);
            self.out.push(GameOut::DropGone { key });
            self.log(format!("co-op: drop {key} expired"));
        }
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        let t = self.cfg.drops;
        let armed: Vec<Pack> = self
            .packs
            .iter()
            .filter(|p| since(now, p.created) >= t.arm_ms)
            .cloned()
            .collect();
        for p in armed {
            let mut near: Vec<(u8, f32)> = self
                .players
                .values()
                .filter(|q| q.life == Life::Alive && q.introduced)
                .map(|q| (q.pid, box_distance(p.rest, q.pos, self.dims_of(q.pid))))
                .filter(|(_, d)| d.is_finite() && *d <= t.pickup_radius)
                .collect();
            near.sort_by(|a, b| a.1.total_cmp(&b.1));
            for (pid, _) in near {
                if self.pack_take(&p, pid) {
                    break;
                }
            }
        }
    }

    /// `UP_USE` on a pack (DSV-170's request): taken as by walking over it, within `drops.use_radius`.
    // DC-DEFAULT: DSV-592 (proposed)
    pub(super) fn pack_use(&mut self, pid: u8, key: u32, now: Millis) {
        let t = self.cfg.drops;
        let Some(p) = self.packs.iter().find(|p| p.key == key).cloned() else {
            return;
        };
        let near = self
            .players
            .get(&pid)
            .filter(|q| q.life == Life::Alive && q.introduced)
            .map(|q| box_distance(p.rest, q.pos, self.dims_of(pid)))
            .is_some_and(|d| d.is_finite() && d <= t.use_radius);
        if near && since(now, p.created) >= t.arm_ms {
            self.pack_take(&p, pid);
        }
    }

    /// The stock cap of a weapon: the ROM's granted amount of its ammunition.
    fn stock_cap(&self, w: &WeaponState) -> u32 {
        self.tables
            .weapons
            .max_amount(w.ammo)
            .unwrap_or(self.cfg.default_stock)
    }

    /// Fills the reserve of `pid`'s weapons of `set` to their stock caps and tells his client each new count (0x66
    /// kind 1); the number of weapons that gained.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (0x66 kind 1 sets a slot's ammunition count without the pickup notice)
    // DC-DEFAULT: DSV-592 (proposed)
    pub fn refill_reserve(&mut self, pid: u8, set: RefillSet) -> usize {
        self.refill_with(pid, set, false)
    }

    /// The round-end recovery's refill (DSV-593): the reserve of `pid`'s weapons of `set` grows until clip and reserve
    /// hold what a new kit holds (a gun: a full clip and the stock cap), the clip itself untouched; his client is told
    /// each new count as by [`Game::refill_reserve`].
    // OBSERVED: owner description of a December 2009 Cabin Fever recording (Normal), 2026-11-05: after a round end
    // 9 in the clip and 111 in reserve, after the reload 30 and 90
    // DC-DEFAULT: DSV-593 (proposed)
    pub fn refill_kit(&mut self, pid: u8, set: RefillSet) -> usize {
        self.refill_with(pid, set, true)
    }

    /// [`Game::refill_reserve`] (`kit` false: the reserve to the stock cap) or [`Game::refill_kit`] (`kit` true: the
    /// reserve to the stock cap plus the clip's empty rounds).
    fn refill_with(&mut self, pid: u8, set: RefillSet, kit: bool) -> usize {
        let Some(mut weapons) = self.players.get(&pid).map(|p| p.weapons.clone()) else {
            return 0;
        };
        let gained = refill(&mut weapons, set, |w| {
            let empty = if kit && !w.own_ammo {
                w.clip_size.saturating_sub(w.clip)
            } else {
                0
            };
            self.stock_cap(w).saturating_add(empty)
        });
        if gained.is_empty() {
            return 0;
        }
        for &i in &gained {
            let w = &weapons[i];
            self.send(
                pid,
                gs::player_ammo(
                    w.weapon,
                    w.ammo,
                    w.told_amount(),
                    w.slot,
                    super::arrival::TODO_SLOT_STATUS_EXTRA,
                ),
            );
        }
        if let Some(p) = self.players.get_mut(&pid) {
            p.weapons = weapons;
        }
        gained.len()
    }

    /// `pid` takes `pack` when it gives him something: the effect, his client told (status, ammunition counts or the
    /// weapon, then the drop-item note), the object removed. `false`: he would gain nothing, the pack stays.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (health status), §4.4 (0xb2), §6.2 (0xc9 sub 6)
    // DC-DEFAULT: DSV-592 (proposed) health to its maximum (OBSERVED: owner's 2009 recording, full heal), the
    // standard pack refills the primary and secondary guns, the heavy case every weapon and thrown item, a grenade one
    // throw up to the stock cap (the default kit's grenade into an empty thrown slot)
    fn pack_take(&mut self, pack: &Pack, pid: u8) -> bool {
        let note = match pack.kind {
            PackKind::Health => {
                let max = self.cfg.max_health;
                let Some(p) = self.players.get_mut(&pid).filter(|p| p.health < max) else {
                    return false;
                };
                p.health = max;
                self.send(pid, gs::player_status(gs::STATUS_HEALTH, max));
                Some(gs::PICKUP_NOTE_HEALTH)
            }
            PackKind::Ammo => {
                if self.refill_reserve(pid, RefillSet::Guns) == 0 {
                    return false;
                }
                Some(gs::PICKUP_NOTE_AMMO)
            }
            PackKind::HeavyAmmo => {
                if self.refill_reserve(pid, RefillSet::All) == 0 {
                    return false;
                }
                Some(gs::PICKUP_NOTE_AMMO_BOX)
            }
            PackKind::Grenade => {
                if !self.grenade_take(pid, pack.weapon) {
                    return false;
                }
                None
            }
        };
        if let Some(n) = note {
            self.send(pid, gs::coop_pickup_note(n));
        }
        self.packs.retain(|p| p.key != pack.key);
        self.out.push(GameOut::DropGone { key: pack.key });
        self.log(format!(
            "co-op: drop {} ({:?}) taken by client {pid}",
            pack.key, pack.kind
        ));
        true
    }

    /// One throw more for `pid`: his thrown weapon gains a round below its stock cap (0x66 kind 1); with an empty
    /// thrown slot the pack's grenade is given with one round (0xb2).
    // DC-DEFAULT: DSV-592 (proposed)
    fn grenade_take(&mut self, pid: u8, weapon: Option<u16>) -> bool {
        let Some(p) = self.players.get(&pid) else {
            return false;
        };
        if let Some(i) = p.weapons.iter().position(|w| w.slot == SLOT_THROWN) {
            let w = p.weapons[i];
            if w.carried() >= self.stock_cap(&w) {
                return false;
            }
            let Some(p) = self.players.get_mut(&pid) else {
                return false;
            };
            let h = &mut p.weapons[i];
            h.stock += 1;
            let h = *h;
            self.send(
                pid,
                gs::player_ammo(
                    h.weapon,
                    h.ammo,
                    h.told_amount(),
                    h.slot,
                    super::arrival::TODO_SLOT_STATUS_EXTRA,
                ),
            );
            return true;
        }
        let Some(gw) = weapon else {
            return false;
        };
        let mut w = WeaponState {
            slot: SLOT_THROWN,
            ..self.new_weapon(gw, None)
        };
        w.clip = w.clip_size.min(1);
        w.stock = 1 - w.clip;
        let parts = if self.cfg.weapon_parts {
            self.tables.weapons.default_parts(gw)
        } else {
            gs::NO_PARTS
        };
        w.parts = parts;
        let Some(p) = self.players.get_mut(&pid) else {
            return false;
        };
        p.weapons.push(w);
        let weapons = p.weapons.clone();
        let spec = self.grant_spec(&weapons, weapons.len() - 1, parts);
        self.send(pid, gs::weapon_give(&spec));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(weapon: u16, slot: u8, clip: u32, stock: u32, own: bool) -> WeaponState {
        WeaponState {
            weapon,
            ammo: weapon,
            clip_size: if own { 1 } else { 30 },
            clip,
            stock,
            slot,
            own_ammo: own,
            parts: gs::NO_PARTS,
            shot_open: false,
            shot_ok: false,
        }
    }

    /// DSV-591: the four item words by their hashes; another word is none.
    #[test]
    fn item_words_by_hash() {
        let k = |w: &str| key_of(w.as_bytes());
        for (word, kind) in [
            (WORD_HEALTH, PackKind::Health),
            (WORD_AMMO, PackKind::Ammo),
            (WORD_HEAVY_AMMO, PackKind::HeavyAmmo),
            (WORD_GRENADE, PackKind::Grenade),
        ] {
            assert_ne!(word, k("x"), "{kind:?}");
        }
        assert_eq!(pack_kind("not a pack"), None);
    }

    /// DSV-591: each row on its own (several items or none), or one roll over the rows in order.
    #[test]
    fn rolls_each_row_or_one_roll() {
        let table = [
            (PackKind::Ammo, 40.0),
            (PackKind::HeavyAmmo, 10.0),
            (PackKind::Health, 5.0),
            (PackKind::Grenade, 25.0),
        ];
        let seq = |v: Vec<f32>| {
            let mut it = v.into_iter();
            move || it.next().unwrap_or(99.9)
        };
        assert_eq!(
            roll_drops(&table, AiRoll::Each, &mut seq(vec![10.0, 50.0, 50.0, 24.0])),
            vec![PackKind::Ammo, PackKind::Grenade]
        );
        assert!(roll_drops(&table, AiRoll::Each, &mut seq(vec![99.0; 4])).is_empty());
        assert_eq!(
            roll_drops(&table, AiRoll::One, &mut seq(vec![44.0])),
            vec![PackKind::HeavyAmmo]
        );
        assert_eq!(
            roll_drops(&table, AiRoll::One, &mut seq(vec![52.0])),
            vec![PackKind::Health]
        );
        assert_eq!(
            roll_drops(&table, AiRoll::One, &mut seq(vec![56.0])),
            vec![PackKind::Grenade]
        );
        assert!(
            roll_drops(&table, AiRoll::One, &mut seq(vec![80.0])).is_empty(),
            "the rest of 100: nothing"
        );
        assert!(roll_drops(&[], AiRoll::Each, &mut seq(vec![])).is_empty());
    }

    /// DSV-591: a model by its path hash, its skin the folder's texture ending with its stem.
    #[test]
    fn model_and_skin_from_a_listing() {
        let names: Vec<String> = [
            "A\\K\\BOX.LTB",
            "A\\K\\512XBOX.DTX",
            "A\\K\\OTHER.DTX",
            "B\\512XBOX.DTX",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let l = look_in(&names, key_of(b"a\\k\\box.ltb")).unwrap();
        assert_eq!(l.model, "A\\K\\BOX.LTB");
        assert_eq!(l.textures, vec!["A\\K\\512XBOX.DTX".to_string()]);
        assert_eq!(look_in(&names, key_of(b"a\\k\\none.ltb")), None);
    }

    /// DSV-592: the standard refill fills the primary and secondary guns only, the full one every weapon and the
    /// thrown item (its carried rounds to the cap), never the melee slot; nothing goes down.
    #[test]
    fn refill_sets() {
        let base = vec![
            ws(1, 0, 10, 20, false),
            ws(2, 1, 5, 60, false),
            ws(3, 2, 0, 0, false),
            ws(4, 3, 1, 0, true),
            ws(5, 4, 30, 0, false),
        ];
        let cap = |w: &WeaponState| if w.own_ammo { 3 } else { 90 };
        let mut a = base.clone();
        assert_eq!(refill(&mut a, RefillSet::Guns, cap), vec![0, 1, 4]);
        assert_eq!(
            (a[0].stock, a[1].stock, a[3].stock, a[4].stock),
            (90, 90, 0, 90)
        );
        let mut b = base.clone();
        assert_eq!(refill(&mut b, RefillSet::All, cap), vec![0, 1, 3, 4]);
        assert_eq!((b[3].clip, b[3].stock, b[2].stock), (1, 2, 0));
        assert!(
            refill(&mut b, RefillSet::All, cap).is_empty(),
            "full: no gain"
        );
        let mut c = vec![ws(1, 0, 30, 200, false)];
        assert!(
            refill(&mut c, RefillSet::Guns, cap).is_empty(),
            "above the cap stays"
        );
        assert_eq!(c[0].stock, 200);
    }

    // ------------------------------------------------------------------------------------- a co-op AI's death

    use crate::coop_rom::AiBrainRow;
    use crate::pipe::RoomSettings;
    use crate::rom_tables::RomTables;
    use crate::world::{synthetic_world_bytes, world_from_bytes, AiTemplate, CoopWorld};

    /// Synthetic tables: a gun (3), a pistol (4) and a thrown weapon (7, its own ammunition, three at most), one
    /// body row for the AI's and the players' character.
    fn tables() -> RomTables {
        use crate::chars::{BodyRow, CharTables, FaceRow};
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Weapon3]\nName = \"G3\"\nHHModel = \"HH\\\\g3.ltb\"\nAmmoName0 = \"A3\"\nShotsPerClip = 30\n",
            "[Weapon4]\nName = \"G4\"\nHHModel = \"HH\\\\g4.ltb\"\nAmmoName0 = \"A4\"\nShotsPerClip = 12\n",
            "[Weapon7]\nName = \"N7\"\nHHModel = \"HH\\\\n7.ltb\"\nAmmoName0 = \"A7\"\nShotsPerClip = 1\nIsAmmo = 1\n",
            "[Ammo3]\nName = \"A3\"\nMaxAmount = 90\n",
            "[Ammo4]\nName = \"A4\"\nMaxAmount = 36\n",
            "[Ammo7]\nName = \"A7\"\nMaxAmount = 3\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"G3\"\nP1 = \"G4\"\nP2 = \"N7\"\n",
        ));
        let mut chars = CharTables::default();
        chars.bodies.insert(
            0,
            BodyRow {
                model: "BODY\\b.ltb".into(),
                styles: vec!["RS\\a.ltb".into(), "RS\\b.ltb".into(), "RS\\c.ltb".into()],
                skins: [(0, (Some("BODY\\u.dtx".into()), Some("BODY\\l.dtx".into())))].into(),
                ..BodyRow::default()
            },
        );
        chars.faces.insert(
            0,
            FaceRow {
                model: "FACE\\f.ltb".into(),
                skins: vec!["FACE\\f.dtx".into()],
                styles: vec!["RS\\a.ltb".into(), "RS\\a.ltb".into()],
            },
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

    fn look(model: &str) -> WeaponLook {
        WeaponLook {
            weapon: 0,
            model: model.into(),
            textures: vec![format!("{model}.dtx")],
            styles: Vec::new(),
        }
    }

    /// A running co-op room (class 5) on the synthetic world with one AI template whose brain row has `drops`, one
    /// introduced living human (client 1) far from the AI with the gun, the pistol and the thrown weapon; the
    /// default kit's thrown slot holds weapon 7; the drops are forced to `force`.
    fn game(drops: Vec<(String, f32)>, force: AiForce) -> Game {
        let mut w = world_from_bytes(&synthetic_world_bytes(), "Worlds\\S.dat", "S").unwrap();
        w.coop = CoopWorld {
            templates: vec![AiTemplate {
                name: b"tmpl".to_vec(),
                right_hand: Some("G3".into()),
                max_health: Some(60.0),
                template: true,
                brain: Some("Br".into()),
                ..Default::default()
            }],
            ..CoopWorld::default()
        };
        let mut cfg = super::super::GameTuning {
            default_loadout: vec![3, 4, 5, 7],
            ..super::super::GameTuning::default()
        };
        cfg.drops.ai_force = force;
        cfg.drops.arm_ms = 500;
        let mut g = Game::new(cfg, &w, tables());
        g.insert_rig("BODY\\b.ltb", rig());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "5".into());
        g.set_room(&room);
        g.coop_tables_mut().unwrap().brains.push(AiBrainRow {
            name: "br".into(),
            drops,
            ..AiBrainRow::default()
        });
        g.prepare_player(1, None, None, 10, [500.0, 0.0, 0.0], [0.0; 3], None, 0);
        let p = g.players.get_mut(&1).unwrap();
        p.introduced = true;
        p.life = Life::Alive;
        p.weapons = vec![
            ws(3, 0, 30, 10, false),
            ws(4, 1, 12, 0, false),
            ws(7, 3, 1, 0, true),
        ];
        g.phase = RoundPhase::Running { start: 0 };
        g.set_pack_looks(PackLooks {
            health: Some(look("P\\H.LTB")),
            ammo: Some(look("P\\A.LTB")),
            heavy: Some(look("P\\B.LTB")),
        });
        g.take_out();
        g
    }

    fn sent_to(out: &[GameOut], pid: u8) -> Vec<dc_wire::udp::BitWriter> {
        out.iter()
            .filter_map(|o| match o {
                GameOut::To(p, m) if *p == pid => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    /// Kills the AI of the template at the origin; the drop's create (key, pickup kind, model) and its rest point.
    fn kill(g: &mut Game) -> Vec<(u32, u32, String)> {
        assert!(g.coop_spawn_test_ai(b"tmpl", [0.0, 0.0, 0.0], 100));
        g.coop_kill_ai(0, 1, 200);
        g.take_out()
            .into_iter()
            .filter_map(|o| match o {
                GameOut::DropCreate(d) => Some((d.key, d.pickup_kind, d.look.model.clone())),
                _ => None,
            })
            .collect()
    }

    /// Walks client 1 onto the drop at `rest` at `now` and runs the pickups.
    fn walk_over(g: &mut Game, rest: [f32; 3], now: Millis) -> Vec<GameOut> {
        g.players.get_mut(&1).unwrap().pos = [rest[0], rest[1] + 50.0, rest[2]];
        g.tick_packs(now);
        g.take_out()
    }

    /// DSV-591 / DSV-592: a forced health pack falls at the dead AI as a kind-0 pickup with the pack model; walking over
    /// it heals fully (health status, the health note), the object goes. A player at full health leaves it lying.
    #[test]
    fn a_health_pack_heals_fully_when_walked_over() {
        let mut g = game(Vec::new(), AiForce::Kind(PackKind::Health));
        let made = kill(&mut g);
        assert_eq!(made.len(), 1, "{made:?}");
        let (key, kind, model) = made[0].clone();
        assert_eq!(
            (kind, model.as_str()),
            (crate::modelobj::PICKUP_KIND_PACK, "P\\H.LTB")
        );
        let rest = g.ai_packs()[0].2;
        assert_eq!((rest[0], rest[2]), (0.0, 0.0), "at the body");
        // full health: it stays
        let out = walk_over(&mut g, rest, 1_000);
        assert!(sent_to(&out, 1).is_empty());
        assert_eq!(g.ai_packs().len(), 1);
        g.players.get_mut(&1).unwrap().health = 33.0;
        // not armed yet just after the death is covered by arm_ms: 200 + 500
        let out = walk_over(&mut g, rest, 650);
        assert!(sent_to(&out, 1).is_empty(), "not armed yet");
        let out = walk_over(&mut g, rest, 1_000);
        let max = g.cfg.max_health;
        assert_eq!(g.players[&1].health, max);
        let sent = sent_to(&out, 1);
        assert_eq!(
            sent,
            vec![
                gs::player_status(gs::STATUS_HEALTH, max),
                gs::coop_pickup_note(gs::PICKUP_NOTE_HEALTH)
            ]
        );
        assert!(out
            .iter()
            .any(|o| matches!(o, GameOut::DropGone { key: k } if *k == key)));
        assert!(g.ai_packs().is_empty());
    }

    /// DSV-592: the standard pack refills the gun and the pistol (not the grenade), the heavy case the grenade too.
    #[test]
    fn ammo_packs_refill_the_reserve() {
        let mut g = game(Vec::new(), AiForce::Kind(PackKind::Ammo));
        let made = kill(&mut g);
        assert_eq!(made[0].2, "P\\A.LTB");
        let rest = g.ai_packs()[0].2;
        let out = walk_over(&mut g, rest, 1_000);
        let w = &g.players[&1].weapons;
        assert_eq!((w[0].stock, w[1].stock, w[2].carried()), (90, 36, 1));
        let sent = sent_to(&out, 1);
        assert_eq!(sent.len(), 3, "two counts and the note");
        assert_eq!(sent[2], gs::coop_pickup_note(gs::PICKUP_NOTE_AMMO));
        assert!(g.ai_packs().is_empty());

        let mut g = game(Vec::new(), AiForce::Kind(PackKind::HeavyAmmo));
        let made = kill(&mut g);
        assert_eq!(made[0].2, "P\\B.LTB");
        let rest = g.ai_packs()[0].2;
        let out = walk_over(&mut g, rest, 1_000);
        let w = &g.players[&1].weapons;
        assert_eq!((w[0].stock, w[1].stock, w[2].carried()), (90, 36, 3));
        assert_eq!(
            sent_to(&out, 1).last(),
            Some(&gs::coop_pickup_note(gs::PICKUP_NOTE_AMMO_BOX))
        );
        assert!(g.ai_packs().is_empty());
    }

    /// DSV-591 / DSV-592: a grenade lies as the kit's thrown weapon (kind 6) and adds one throw; without a thrown weapon
    /// the grenade is given (0xb2); at the cap it stays.
    #[test]
    fn a_grenade_adds_one_throw() {
        let mut g = game(Vec::new(), AiForce::Kind(PackKind::Grenade));
        let made = kill(&mut g);
        assert_eq!(made[0].1, crate::modelobj::PICKUP_KIND_WEAPON);
        assert!(made[0].2.ends_with("n7.ltb"), "{:?}", made[0].2);
        let rest = g.ai_packs()[0].2;
        let out = walk_over(&mut g, rest, 1_000);
        assert_eq!(g.players[&1].weapons[2].carried(), 2);
        assert_eq!(sent_to(&out, 1).len(), 1, "the new count, no note");
        assert!(g.ai_packs().is_empty());

        // no thrown weapon: the grenade is given into slot 3 with one round
        let mut g = game(Vec::new(), AiForce::Kind(PackKind::Grenade));
        g.players.get_mut(&1).unwrap().weapons.truncate(2);
        kill(&mut g);
        let rest = g.ai_packs()[0].2;
        walk_over(&mut g, rest, 1_000);
        let w = g.players[&1].weapons[2];
        assert_eq!((w.weapon, w.slot, w.carried()), (7, SLOT_THROWN, 1));
        assert!(g.ai_packs().is_empty());

        // at the cap: it stays, then expires with the drop lifetime
        let mut g = game(Vec::new(), AiForce::Kind(PackKind::Grenade));
        g.players.get_mut(&1).unwrap().weapons[2].stock = 2;
        kill(&mut g);
        let rest = g.ai_packs()[0].2;
        walk_over(&mut g, rest, 1_000);
        assert_eq!(g.ai_packs().len(), 1);
        let life = g.cfg.drops.lifetime_ms;
        g.tick_packs(200 + life);
        assert!(g.ai_packs().is_empty());
    }

    /// DSV-591: a row whose item word we do not know, a brain without rows, a forced none and `ai_drops` off drop
    /// nothing (the known words are checked against the ROM in `coop_rom::rom_ai_brains_drop_known_packs_with_rom_models`).
    #[test]
    fn the_rows_decide_and_none_drops_nothing() {
        let mut g = game(vec![("unknown".into(), 100.0)], AiForce::Rom);
        assert!(kill(&mut g).is_empty());
        let mut g = game(Vec::new(), AiForce::Rom);
        assert!(kill(&mut g).is_empty(), "no rows");
        let mut g = game(vec![("unknown".into(), 100.0)], AiForce::Nothing);
        assert!(kill(&mut g).is_empty(), "forced none");
        // ai_drops off: nothing even when forced
        let mut g = game(Vec::new(), AiForce::Kind(PackKind::Health));
        g.cfg.drops.ai_drops = false;
        assert!(kill(&mut g).is_empty());
    }

    #[test]
    fn tuning_keys() {
        let mut t = super::super::drops::DropTuning::default();
        assert_eq!(
            (t.ai_drops, t.ai_roll, t.ai_force),
            (true, AiRoll::Each, AiForce::Rom)
        );
        t.set("drops.ai_roll", "one").unwrap();
        t.set("ai_force", "heavy_ammo").unwrap();
        t.set("ai_drops", "off").unwrap();
        assert_eq!(
            (t.ai_drops, t.ai_roll, t.ai_force),
            (false, AiRoll::One, AiForce::Kind(PackKind::HeavyAmmo))
        );
        assert!(t.set("ai_force", "cake").is_err());
        assert!(t.set("ai_roll", "two").is_err());
    }
}
