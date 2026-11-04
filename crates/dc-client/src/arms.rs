//! The local player's weapons: the inventory the server grants, weapon select, fire, reload and the throwables,
//! with the client -> server messages in the 2009 client's forms.
//!
//! The inventory comes from the server's grants (`GS_WEAPON_GIVE` 0xb2 / `GS_WEAPON_EQUIP` 0xb3, removal 0xb4,
//! ammunition 0x66 kind 1), or, when no grant arrived shortly after a spawn, from the lobby's equipment slots 0x323
//! with the ROM weapon row. Weapon slot `i` is inventory slot `i`; the number keys 1-4 and the mouse wheel choose
//! one. The messages:
//! - select: `UP_WEAPON_EVENT` 0x76 `[3][2]`, then `UP_WEAPON_CHANGE` 0x7f `[slot][weapon][ammo id]` in the same
//!   moment (PROVEN-CAPTURE forms);
//! - fire: one `UP_FIRE` 0x82 per pellet (`[Weapon<n>] VectorsPerRound`), 887 bits when the ray hits a remote
//!   character's box before the world, else 623 bits; the 0x64 update carries control bit 0x400 and `pu_flags` 0x2000
//!   while the trigger is held, and the spread float `weapon_f` (0x0200) when it changed;
//! - reload: `UP_RELOAD` 0x83 `[ammo id]` on the reload key and at once when a shot empties the clip, then
//!   `0x76 [3][1][clip size]`;
//! - throwables: `UP_THROW_HOLD` 0x4d on every pass of the throw key while the trigger is held, the release as one
//!   0x82 without a hit, and the switch back to slot 0 after the last one;
//! - `UP_WEAPON_SOUND` 0x80 at the sound keys of the select and reload animations.
//!
//! Timing is the 2009 client's own: its weapon state follows the first-person model's animation clips, and the
//! messages go out when the clip's keys pass (C2S §4.1, §4.3, §4.6). The clips and their key times are read from the
//! user's ROM at run time (the first-person model of the weapon row and the child model it borrows its clips from),
//! found by the SHA-256 keys of their names; without a ROM the DeCombat defaults below stand in. No ROM value is
//! stored here.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (0x64 flags 0x0200 / 0x2000, control 0x400), §4.1 (0x82), §4.2 (0x83), §4.3
// (0x80), §4.4 (0x7f), §4.5 (0x76 category 3), §4.6 (0x4d); GAME_MESSAGES_2009_S2C.md §4.3 (0x66 kind 1), §4.4
// (0xb2 / 0xb3 / 0xb4); PROTOCOL_2009.md §3.3 (0x323 equipment slots)
// OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §2-§4 (select pair, 0x82 forms, 0x64 while firing);
// CAPTURE_tdm1_GAME_VALIDATION.md §2.6-§2.10 (shot spacing, spread float, reloads, sounds)

use std::collections::BTreeMap;
use std::path::Path;

use dc_botclient::weapon::{
    fire_body, weapon_change_body, weapon_event_body, FireExtra, FireReport, WeaponChange,
    WeaponGrant, EVENT_WEAPON, WEAPON_EVENT_SELECT,
};
use dc_bute::ButeDoc;
use dc_render::{name_key, NameKey};
use dc_rez::RezVfs;
use dc_sim::{Collider, V3};
use dc_wire::udp::{BitReader, BitWriter};

// --------------------------------------------------------------------------------------------- message ids, forms

/// Client -> server shell ids of the weapon messages.
pub mod up {
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1
    pub const FIRE: u8 = 0x82;
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.2
    pub const RELOAD: u8 = 0x83;
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.3
    pub const WEAPON_SOUND: u8 = 0x80;
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.4
    pub const WEAPON_CHANGE: u8 = 0x7f;
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.5
    pub const WEAPON_EVENT: u8 = 0x76;
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (`UP_THROW_HOLD`, no fields)
    pub const THROW_HOLD: u8 = 0x4d;
}

/// `UP_WEAPON_EVENT` [3][1][value] after a reload; `value` = the clip size in every observed reload.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.5 (event 1 + value); OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.7 (100-250 ms
// after each 0x83, value 30 for weapon 145 and 15 for weapon 164 = their clip sizes)
pub const WEAPON_EVENT_RELOADED: u8 = 1;
/// Delay of that event after the 0x83.
// DC-DEFAULT: CLI-32 (proposed) 150 ms (OBSERVED 100-250 ms, tdm1)
pub const RELOAD_EVENT_DELAY_MS: u32 = 150;

/// Control-word bit of the held fire key and the `pu_flags` bit the 2009 client set in the same updates.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (control FIRE 0x400; pu_flags 0x2000 = the weapon's slot +0x134)
// OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §4 (0x3321 / 0x600 at the shots, 0x1321 / 0x200 between them)
pub const FIRE_CONTROL_BIT: u32 = 0x400;
pub const FIRE_PU_FLAG: u16 = 0x2000;
/// After a shot the fire bits stay on this long, so a short click still reaches one 0x64 (15 per second).
// DC-DEFAULT: CLI-33 (proposed) one update interval, 67 ms (C2S §2.1 R = 15 per second)
pub const FIRE_BITS_LATCH_MS: u32 = 67;

/// `weapon_pos` relative to the shot origin: right, up, forward (world units, turned by the yaw).
// OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §3 (weapon_pos - origin = 4.69 right, 8.85 forward, 4.27 down at yaw 1.0465)
pub const WEAPON_OFFSET: [f32; 3] = [4.69, -4.27, 8.85];
/// `extra_b` of a box-only hit of [`trace`] (the HUD's aim test); a shot's `extra_b` is the face normal of its shot
/// box ([`crate::hitnodes`]), which was (-1, 0, 0) in every hit of the captures below.
// OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §3 ((-1, 0, 0) in all four)
pub const HIT_VECTOR: [f32; 3] = [-1.0, 0.0, 0.0];
/// `stat` grows by this per 0x82 while the cone draws (two table reads per pellet, [`crate::cone`]).
// OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.6, CAPTURE_tdm4_WEAPON_FORMS.md §3 (+2 per message); SPEC:
// WEAPON_FIRE_2009_addendum_cone.md AC-1.5
pub const STAT_STEP: u32 = 2;
/// `shot_scale` when the weapon row gives none: the float 1.0 (the 32 bits 0x3f800000).
// OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §3 (0x3F800000 in all five messages); EVIDENCE: cshell+0x17f7b0 (row default)
pub const DEFAULT_SHOT_SCALE: f32 = crate::cone::row_default::SCALE;

/// The `UP_WEAPON_SOUND` body: `[sound_no:u8][weapon_id:u16][own_id:u8][weapon_pos:vec96]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.3; OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.8 (144 bits with the ids)
pub fn weapon_sound_body(sound: u8, weapon: u16, own_id: u8, weapon_pos: [f32; 3]) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(sound).write_u16(weapon).write_u8(own_id);
    for c in weapon_pos {
        w.write_f32(c);
    }
    w
}

/// The `UP_RELOAD` body: `[ammo_id:u16]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.2; OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.7 (`0b 83 91 00`)
pub fn reload_body(ammo_id: u16) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u16(ammo_id);
    w
}

/// The `UP_WEAPON_EVENT` body `[3][1][value]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.5
pub fn reloaded_event_body(value: u8) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(EVENT_WEAPON)
        .write_u8(WEAPON_EVENT_RELOADED)
        .write_u8(value);
    w
}

// --------------------------------------------------------------------------------------------- the ROM weapon table

/// Archive and entry of the weapon table.
// OBSERVED: ROM table key WEAPONS.TXT (entry of ATTRIBUTES.rez)
pub const ATTRIBUTES_REZ: &str = "ATTRIBUTES.rez";
pub const WEAPONS_ENTRY: &str = "ATTRIBUTES\\WEAPONS.TXT";

/// Keys of the `[Weapon<n>]` and `[Ammo<n>]` rows read here.
// OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] VectorsPerRound, ShotsPerClip, AmmoName0, Range, MinPerturb,
// MaxPerturb, PerturbIncSpeed, PerturbDecSpeed, DynamicPerturbFactor, RapidFireCount, ClientWeaponType, HHModel,
// HHSkin<k>, PVModel<hand>, PVSkin<k>; [PVModelIdentifiers] Name<k>; [Ammo<n>] Name, MaxAmount
pub mod key {
    pub const PELLETS: &str = "VectorsPerRound";
    pub const CLIP: &str = "ShotsPerClip";
    pub const AMMO_NAME0: &str = "AmmoName0";
    /// `[Ammo<n>]` kind of the round (0 traced, 1 launched, 2 thrown).
    pub const AMMO_TYPE: &str = "Type";
    pub const RANGE: &str = "Range";
    pub const MIN_SPREAD: &str = "MinPerturb";
    pub const MAX_SPREAD: &str = "MaxPerturb";
    pub const SPREAD_UP: &str = "PerturbIncSpeed";
    pub const SPREAD_DOWN: &str = "PerturbDecSpeed";
    pub const SHOT_SCALE: &str = "DynamicPerturbFactor";
    pub const SINGLE_SHOT: &str = "RapidFireCount";
    pub const CLIENT_KIND: &str = "ClientWeaponType";
    pub const HH_MODEL: &str = "HHModel";
    pub const HH_SKIN: &str = "HHSkin";
    pub const PV_MODEL_PREFIX: &str = "PVModel";
    pub const PV_SKIN: &str = "PVSkin";
    // OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] Pos (the first-person model position, eye space)
    pub const PV_POS: &str = "Pos";
    // OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] MuzzlePos (the muzzle point from the model position, eye space)
    pub const MUZZLE_POS: &str = "MuzzlePos";
    pub const PV_IDENTS: &str = "PVModelIdentifiers";
    pub const IDENT_NAME: &str = "Name";
    pub const NAME: &str = "Name";
    pub const MAX_AMOUNT: &str = "MaxAmount";
    // OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] HideWhenEmpty, InfiniteAmmo (read at run time)
    // EVIDENCE: cshell+0x17cded..0x17ce1a (row loader: `InfiniteAmmo` -> row +0x23c, `HideWhenEmpty` -> row +0x240;
    // DISASM)
    pub const HIDE_WHEN_EMPTY: &str = "HideWhenEmpty";
    pub const INFINITE_AMMO: &str = "InfiniteAmmo";
    // OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] ZoomTime, Guntype, ReloadAfterFire (read at run time)
    // DISASM: cshell+0x17d97b (`ZoomTime` -> row +0x284), cshell+0x17e2cb (`Guntype` -> row +0x418), cshell+0x17cd80
    // (`ReloadAfterFire` -> row +0x200 as a flag)
    pub const ZOOM_TIME: &str = "ZoomTime";
    pub const GUN_TYPE: &str = "Guntype";
    pub const RELOAD_AFTER_FIRE: &str = "ReloadAfterFire";
    // OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] DotSightName (a HUD picture on the knife rows)
    // DISASM: cshell+0x17c14e (`DotSightName` -> row +0xc8)
    pub const DOT_SIGHT: &str = "DotSightName";
}

/// `Guntype` of the melee rows (the zoom key is their alternate fire, CLI-1350).
// DISASM: cshell+0x1f8b40 (row +0x418 == 0 with the zoom bit -> fire mode 1); OBSERVED: ROM `Guntype` 0 rows are the
// knives, axes, shovels and hands (2026-10-29)
pub const GUN_TYPE_MELEE: u32 = 0;
/// The slowest play rate a mod sum may give a clip.
// DC-DEFAULT: CLI-1441 (proposed)
pub const MIN_ANIM_RATE: f32 = 0.05;

/// A clip time (ms) of a clip played at `rate` (1 = its own speed).
pub fn ms_at_rate(ms: u32, rate: f32) -> u32 {
    if rate == 1.0 || ms == 0 {
        ms
    } else {
        ((ms as f32 / rate.max(MIN_ANIM_RATE)).round() as u32).max(1)
    }
}

/// A clip's marks with their times at `rate`.
fn at_rate(marks: Vec<(u32, Mark)>, rate: f32) -> Vec<(u32, Mark)> {
    marks
        .into_iter()
        .map(|(at, m)| (ms_at_rate(at, rate), m))
        .collect()
}

/// The fire state's clip: the alternate-fire clip for the alternate fire, else the fire clip.
pub fn fire_clip_of(alt: bool) -> fn(&WeaponClips) -> Option<&ClipFact> {
    if alt {
        |c| c.alt_clip.as_ref()
    } else {
        |c| c.fire.as_ref()
    }
}

/// `Guntype` of the rifles whose zoom goes out when the sprint state starts.
// EVIDENCE: cshell+0x23ac50 (entering the sprint state 0xe with row +0x418 == 0xb calls the player manager's +0x78
// with 0, the zoom-off; read as such by the parity audit, MOVEMENT_2009.md §7 for the zoom fields)
pub const GUN_TYPE_SPRINT_UNZOOM: u32 = 0xb;

/// `ClientWeaponType` value of the throwable weapons (used only when no first-person model tells it).
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] ClientWeaponType (2 for the grenade the captures threw, weapon 7;
// 1 for the guns 145, 164 and the knife 0; the meaning is our reading, PLAUSIBLE)
pub const CLIENT_KIND_THROWN: u32 = 2;

/// One weapon row as the client uses it (numbers and resource paths read at run time).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeaponRow {
    pub pellets: Option<u32>,
    pub clip: Option<u32>,
    /// The `[Ammo<n>]` row named by the first ammunition name.
    pub ammo: Option<u16>,
    pub max_amount: Option<u32>,
    /// The `Type` of that ammunition row (`None`: no row or no key, traced as an instant round).
    // OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] Type; EVIDENCE: cshell+0x174d30 (row +0x0c)
    pub ammo_type: Option<u32>,
    pub range: Option<f32>,
    pub min_spread: Option<f32>,
    pub max_spread: Option<f32>,
    pub spread_up: Option<f32>,
    pub spread_down: Option<f32>,
    /// The cone keys (bounds, zoomed and triple-shot pairs, DynamicPerturbFactor and its movement factors).
    pub cone: crate::cone::ConeRow,
    pub single_shot: Option<u32>,
    pub client_kind: Option<u32>,
    /// Third-person model and its skins.
    pub hh_model: Option<String>,
    pub hh_skins: Vec<String>,
    /// First-person model for the hand identifier the table resolved, and its skins.
    pub pv_model: Option<String>,
    pub pv_skins: Vec<String>,
    /// The row's first-person model position in eye space (right, up, forward).
    // EVIDENCE: cshell+0x17bd20 (row loader: `Pos` -> row +0x14..+0x1c), cshell+0x232c10 (every frame the first-person
    // model position := row +0x14 plus the bob and recoil terms; DISASM)
    pub pv_pos: Option<[f32; 3]>,
    /// The row's inspect, from a pack's keys (DC-DEFAULT: CLI-1845).
    pub inspect: Option<std::sync::Arc<crate::inspect::InspectAnim>>,
    /// The row's muzzle point relative to `pv_pos` (eye space).
    // EVIDENCE: cshell+0x17bd20 (row loader: `MuzzlePos` -> row +0x20..+0x28; DISASM)
    pub muzzle_pos: Option<[f32; 3]>,
    /// Every first-person model of the row in the order of the hand identifiers (duplicates once).
    pub pv_models: Vec<String>,
    /// `HideWhenEmpty` (`None`: the row has no such key): when the round use leaves its slot with no round, the
    /// weapon list draws another weapon in the same update ([`Arsenal`]'s empty switch).
    // EVIDENCE: cshell+0x237614..0x2376fb (weapon update: the fired flag + row +0x240 + slot count 0 -> result 13),
    // cshell+0x22f1c0 (result 13 -> the empty switch cshell+0x22ec00)
    pub hide_when_empty: Option<bool>,
    /// `InfiniteAmmo`: the weapon never runs out (its rounds are not counted down, it counts as loaded).
    // EVIDENCE: cshell+0x232800 (has-ammo test: row +0x23c set -> loaded), cshell+0x233d90 (round use skips the count)
    pub infinite_ammo: bool,
    /// `ZoomTime` (seconds of a full zoom change; `None`: the row has none, the binary default applies).
    pub zoom_time: Option<f32>,
    /// `Guntype` (row +0x418).
    pub gun_type: Option<u32>,
    /// `ReloadAfterFire` (row +0x200): the bolt cycles after each shot; the zoom key is refused while it fires.
    pub reload_after_fire: bool,
    /// `DotSightName` (row +0xc8): the picture the aim element draws at the screen centre (the knife's red X).
    pub dot_sight: Option<String>,
    /// `CameraSway*` (row +0x3f0..+0x3fc): the scope sway ([`crate::zoom::CameraSway`]).
    pub sway: crate::zoom::CameraSway,
}

/// The weapon rows of the ROM.
#[derive(Debug, Clone, Default)]
pub struct WeaponTable {
    pub rows: BTreeMap<u16, WeaponRow>,
    /// The weapon command table: weapon numbers in the order of the `[WeaponOrder]` section (classes in order, each
    /// class's entries in order); `GS_WEAPON_SWITCH` names entry `command - 0x14`.
    // EVIDENCE: cshell+0x180590 (class loop, then entry loop with a name lookup per entry)
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0x7f)
    pub commands: Vec<u16>,
    /// A variant row's base gun (VAR-1): a variant has no entry of its own in the command table and uses its base gun's.
    pub variants: BTreeMap<u16, u16>,
    /// Spread, recoil and aim_mark numbers of every row ([`crate::spread`]).
    pub spread: crate::spread::SpreadTable,
    /// The `[Mod<n>]` rows' fire-routine numbers ([`crate::cone::ModRow`]).
    pub mods: BTreeMap<u16, crate::cone::ModRow>,
    /// The order class of every weapon in `[WeaponOrder]`, numbered as the 2009 client numbers them: 1 for the
    /// weapons of `Class0`, 2 for `Class1`, ... (a weapon outside the order has none, class 0).
    // EVIDENCE: cshell+0x47da0 (class = 1 + the first class block whose end is at or past the weapon's position in
    // the command table, 0 when the weapon is not in it), cshell+0x180590 (blocks built class by class)
    pub classes: BTreeMap<u16, u8>,
    /// The scope and fire-sound numbers of every `[Mod<n>]` row ([`crate::zoom::ModInfo`]).
    pub mod_info: BTreeMap<u16, crate::zoom::ModInfo>,
    /// The mod models and muzzle effects of the mod and weapon rows ([`crate::attach`]).
    pub attach: crate::attach::AttachTable,
    /// The weapon document the table was built from, for [`WeaponTable::apply_content`] (empty for a table not made
    /// by [`WeaponTable::from_doc`]).
    pub source: std::sync::Arc<ButeDoc>,
}

/// Order classes a number key or the wheel draws even without a round (2009 numbering: the first two blocks of
/// `[WeaponOrder]`); a weapon of any other class is drawn only while it is loaded (a round in its slot or
/// `InfiniteAmmo`).
// EVIDENCE: cshell+0x22e704..0x22e71f (number key: class 1 or 2 skips the has-ammo test cshell+0x232800),
// cshell+0x22e790 (next / previous weapon: the same test)
pub const DRAW_EMPTY_CLASSES: [u8; 2] = [1, 2];

/// The classes the empty switch looks at, in order: the slot last drawn of class 1, else of class 2, else of class 3.
// EVIDENCE: cshell+0x22ec00 (the weapon list's slots +0x3b0 / +0x3b4 / +0x3b8 = the class-1 / 2 / 3 entries of the
// last-drawn table written by every change, cshell+0x22e930 at 0x22eab9)
pub const EMPTY_SWITCH_CLASSES: [u8; 3] = [1, 2, 3];

/// First value of `GS_WEAPON_SWITCH`'s command number.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (weapon := command table [command - 0x14])
pub const WEAPON_COMMAND_BASE: u8 = 0x14;
/// Section of the weapon order and its class key prefix.
// OBSERVED: ROM table key WEAPONS.TXT [WeaponOrder] Class<k>, <class><i>
pub(crate) const WEAPON_ORDER_SECTION: &str = "WeaponOrder";
pub(crate) const KEY_CLASS: &str = "Class";

fn index_of(section: &str, prefix: &str) -> Option<u16> {
    let rest = section.strip_prefix(prefix)?;
    (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| rest.parse().ok())
        .flatten()
}

fn num(v: Option<&String>) -> Option<f32> {
    let t = v?.trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

fn text(v: Option<&String>) -> Option<String> {
    let t = v?.trim().trim_matches('"').trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn numbered(sec: &dc_bute::SectionMap, k: &str, max: u32) -> Vec<String> {
    (0..max)
        .map_while(|i| text(sec.get(&format!("{k}{i}"))))
        .collect()
}

impl WeaponTable {
    /// The table of a parsed weapon document. The first-person model is the row's `PVModel<hand>` for the first hand
    /// identifier of `[PVModelIdentifiers]` the row carries.
    // DC-DEFAULT: CLI-34 (proposed) the first hand identifier of the table (which one the 2009 client picks per
    // character is not read)
    pub fn from_doc(doc: &ButeDoc) -> WeaponTable {
        let idents: Vec<String> = doc
            .get(key::PV_IDENTS)
            .map(|s| numbered(s, key::IDENT_NAME, 16))
            .unwrap_or_default();
        let mut ammo: BTreeMap<String, (u16, Option<u32>, Option<u32>)> = BTreeMap::new();
        for (name, sec) in doc.iter() {
            if let (Some(n), Some(nm)) = (index_of(name, "Ammo"), text(sec.get(key::NAME))) {
                let max = num(sec.get(key::MAX_AMOUNT)).map(|x| x.max(0.0) as u32);
                let ty = num(sec.get(key::AMMO_TYPE)).map(|x| x.max(0.0) as u32);
                ammo.entry(nm.to_ascii_lowercase()).or_insert((n, max, ty));
            }
        }
        let mut rows = BTreeMap::new();
        let mut names: BTreeMap<String, u16> = BTreeMap::new();
        let mut variants: BTreeMap<u16, u16> = BTreeMap::new();
        let mut mods = BTreeMap::new();
        let mut mod_info = BTreeMap::new();
        for (name, sec) in doc.iter() {
            if let Some(n) = index_of(name, "Mod") {
                mods.insert(n, crate::cone::ModRow::of_section(sec));
                mod_info.insert(n, crate::zoom::ModInfo::of_section(sec));
            }
            let Some(n) = index_of(name, "Weapon") else {
                continue;
            };
            let a = text(sec.get(key::AMMO_NAME0)).and_then(|a| ammo.get(&a.to_ascii_lowercase()));
            let pv_model = idents
                .iter()
                .find_map(|i| text(sec.get(&format!("{}{i}", key::PV_MODEL_PREFIX))));
            let mut pv_models: Vec<String> = Vec::new();
            for m in idents
                .iter()
                .filter_map(|i| text(sec.get(&format!("{}{i}", key::PV_MODEL_PREFIX))))
            {
                if !pv_models.iter().any(|x| x.eq_ignore_ascii_case(&m)) {
                    pv_models.push(m);
                }
            }
            let u = |k: &str| num(sec.get(k)).map(|x| x.max(0.0) as u32);
            // VAR-1: a variant carries its base gun's name and must not take the base gun's entry
            let of = sec
                .get(dc_bute::modpatch::KEY_VARIANT_OF)
                .map(|v| v.as_str());
            match dc_bute::modpatch::variant_base(of) {
                Some(base) => {
                    variants.insert(n, base);
                }
                None => {
                    if let Some(nm) = text(sec.get(key::NAME)) {
                        names.entry(dc_bute::fold_latin1(&nm)).or_insert(n);
                    }
                }
            }
            rows.insert(
                n,
                WeaponRow {
                    pellets: u(key::PELLETS),
                    clip: u(key::CLIP),
                    ammo: a.map(|x| x.0),
                    max_amount: a.and_then(|x| x.1),
                    ammo_type: a.and_then(|x| x.2),
                    range: num(sec.get(key::RANGE)),
                    min_spread: num(sec.get(key::MIN_SPREAD)),
                    max_spread: num(sec.get(key::MAX_SPREAD)),
                    spread_up: num(sec.get(key::SPREAD_UP)),
                    spread_down: num(sec.get(key::SPREAD_DOWN)),
                    cone: crate::cone::ConeRow::of_section(sec),
                    single_shot: u(key::SINGLE_SHOT),
                    client_kind: u(key::CLIENT_KIND),
                    hh_model: text(sec.get(key::HH_MODEL)),
                    hh_skins: numbered(sec, key::HH_SKIN, 8),
                    pv_model,
                    pv_skins: numbered(sec, key::PV_SKIN, 16),
                    pv_pos: crate::fx::row_vec3(sec.get(key::PV_POS)),
                    inspect: crate::inspect::InspectAnim::from_row(&|k| text(sec.get(k)))
                        .map(std::sync::Arc::new),
                    muzzle_pos: crate::fx::row_vec3(sec.get(key::MUZZLE_POS)),
                    pv_models,
                    hide_when_empty: u(key::HIDE_WHEN_EMPTY).map(|v| v != 0),
                    infinite_ammo: u(key::INFINITE_AMMO).is_some_and(|v| v != 0),
                    zoom_time: num(sec.get(key::ZOOM_TIME)),
                    gun_type: u(key::GUN_TYPE),
                    reload_after_fire: u(key::RELOAD_AFTER_FIRE).is_some_and(|v| v != 0),
                    dot_sight: text(sec.get(key::DOT_SIGHT)),
                    sway: crate::zoom::CameraSway::of_section(
                        sec,
                        crate::zoom::CameraSway::WEAPON_DEFAULT,
                    ),
                },
            );
        }
        let mut commands = Vec::new();
        let mut classes = BTreeMap::new();
        if let Some(order) = doc.get(WEAPON_ORDER_SECTION) {
            for k in 0..64u32 {
                let Some(class) = text(order.get(&format!("{KEY_CLASS}{k}"))) else {
                    break;
                };
                for i in 0..4096u32 {
                    let Some(entry) = text(order.get(&format!("{class}{i}"))) else {
                        break;
                    };
                    if let Some(w) = names.get(&dc_bute::fold_latin1(&entry)) {
                        commands.push(*w);
                        classes.entry(*w).or_insert(k as u8 + 1);
                    }
                }
            }
        }
        for (variant, base) in &variants {
            if let Some(class) = classes.get(base).copied() {
                classes.insert(*variant, class);
            }
        }
        WeaponTable {
            rows,
            commands,
            variants,
            spread: crate::spread::SpreadTable::from_doc(doc),
            mods,
            classes,
            mod_info,
            attach: crate::attach::AttachTable::from_doc(doc),
            source: std::sync::Arc::new(doc.clone()),
        }
    }

    /// CLI-1611: applies the server mod set's weapon content (MOD-21, the document the game server got as
    /// `-content`) to this table: the patches overwrite the keys of the source document, the table is rebuilt from
    /// it, so the predicted clip, pellets, spread, ammunition row and every other row value read the patched keys,
    /// exactly as the server's table does. A refused document (a patch of a missing row, a taken register id)
    /// leaves the table as it was.
    // DC-DEFAULT: CLI-1611 (proposed), MOD-21 (proposed)
    pub fn apply_content(
        &mut self,
        content: &dc_bute::modpatch::WeaponContent,
    ) -> Result<(), dc_bute::modpatch::PatchError> {
        let mut doc = (*self.source).clone();
        content.apply_to_doc(&mut doc)?;
        *self = Self::from_doc(&doc);
        Ok(())
    }

    /// The gun a weapon row switches as: a variant's base gun, else the row itself (VAR-1).
    pub fn base_of(&self, weapon: u16) -> u16 {
        self.variants.get(&weapon).copied().unwrap_or(weapon)
    }

    /// The weapon a `GS_WEAPON_SWITCH` command names.
    pub fn weapon_of_command(&self, command: u8) -> Option<u16> {
        let i = command.checked_sub(WEAPON_COMMAND_BASE)?;
        self.commands.get(usize::from(i)).copied()
    }

    /// Reads the table from the user's ROM folder.
    pub fn open(rom: &Path) -> Result<WeaponTable, String> {
        let vfs = RezVfs::new(rom);
        match ButeDoc::try_from_vfs(&vfs, ATTRIBUTES_REZ, WEAPONS_ENTRY) {
            Ok(Some(d)) => Ok(Self::from_doc(&d)),
            Ok(None) => Err("weapon table entry missing".into()),
            Err(e) => Err(format!("weapon table: {e}")),
        }
    }

    pub fn get(&self, weapon: u16) -> Option<&WeaponRow> {
        self.rows.get(&weapon)
    }

    /// The order class of `weapon` (2009 numbering, 0 when it is not in `[WeaponOrder]`).
    pub fn order_class(&self, weapon: u16) -> u8 {
        self.classes.get(&weapon).copied().unwrap_or(0)
    }
}

// --------------------------------------------------------------------------------------------- first-person clips

/// What an animation key of a first-person clip does (C2S §4.1 / §4.3 / §4.6 key table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// The fire key: one shot (one 0x82 per pellet) or, in a throwable's release clip, the release.
    Fire,
    /// The throw key: one `UP_THROW_HOLD` 0x4d.
    Throw,
    /// The sound key with its argument: one `UP_WEAPON_SOUND` 0x80.
    Sound(u8),
}

/// Name keys of the key commands: SHA-256 of the upper-cased command word (the client compares them
/// case-insensitively), the form C2S §4.6 cites for the throw key.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.3 (key names compared case-insensitively by cshell+0x2eb60), §4.6 (throw key
// a40505c761a7c907/9)
pub const KEY_FIRE: NameKey = ([0xe8, 0x34, 0x14, 0x00, 0x89, 0x31, 0x7a, 0xca], 8);
pub const KEY_THROW: NameKey = ([0xa4, 0x05, 0x05, 0xc7, 0x61, 0xa7, 0xc9, 0x07], 9);
pub const KEY_SOUND: NameKey = ([0x71, 0x50, 0x5d, 0xbc, 0x89, 0xa9, 0x43, 0xed], 9);
/// The alternate-fire key of an alternate-fire clip: it releases the shot as the fire key does.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.3 (the fire key and the alternate-fire key both call weapon slot +0x180, the
// fire-pending byte); OBSERVED: the ROM knife's alternate-fire clips carry one such key (2026-10-29, key names only)
pub const KEY_ALT_FIRE: NameKey = ([0xb9, 0xd3, 0xa3, 0x25, 0x05, 0xd5, 0x9c, 0xef], 12);

/// Name keys of the first-person clips (SHA-256 of the lower-cased clip name).
// OBSERVED: the first-person models of the ROM weapon rows borrow clips of these names from their child model
// (2026-10-25, weapons 0, 7, 145, 164): select (the draw), fire, reload, prefire / postfire (a throwable's wind-up and
// release), idle; matched by key, the names are not stored
pub const CLIP_SELECT: NameKey = ([0xb1, 0xa3, 0x6d, 0x25, 0xd9, 0x63, 0x3e, 0xd2], 6);
pub const CLIP_FIRE: NameKey = ([0xe5, 0x5d, 0x4d, 0xe3, 0x10, 0x36, 0x4c, 0x9d], 6);
pub const CLIP_RELOAD: NameKey = ([0x40, 0x27, 0xf5, 0x15, 0x41, 0x8b, 0x77, 0x48], 6);
pub const CLIP_PREFIRE: NameKey = ([0x52, 0xc0, 0xdd, 0x32, 0x54, 0x76, 0x04, 0x55], 7);
pub const CLIP_POSTFIRE: NameKey = ([0xd7, 0x8d, 0xee, 0x09, 0x45, 0x91, 0x74, 0x4b], 8);
pub const CLIP_IDLE: NameKey = ([0xae, 0x0e, 0x0c, 0xad, 0x52, 0x76, 0x31, 0xa6], 6);
/// Name key of the first alternate-fire clip (the weapon's +0x11c clip, looked up next to the first fire clip).
// DISASM: cshell+0x233ec0 (InitAnimations: `<fire prefix><i>` into +0x114 / +0x118 and `<alt-fire prefix><i>` into
// +0x11c / +0x120 for i = 0, 1); OBSERVED: the ROM knife rows' clip sets hold it (2026-10-29, key only)
pub const CLIP_ALT_FIRE: NameKey = ([0x59, 0x15, 0x40, 0x55, 0x1a, 0x6b, 0x73, 0xea], 10);
/// Name keys of the first-person sprint clips (entry, loop, exit; SHA-256 of the lower-cased name).
// EVIDENCE: cshell+0x233ec0 (the client weapon looks the three clips up by name next to select / reload / idle / fire;
// the names are compared case-insensitively). OBSERVED: all 119 first-person clip sets of the ROM weapon rows hold the
// three clips (2026-10-26, name keys only, the names are not stored)
pub const CLIP_SPRINT_ENTER: NameKey = ([0x0f, 0x52, 0x62, 0x81, 0x83, 0x95, 0x53, 0xde], 7);
pub const CLIP_SPRINT_LOOP: NameKey = ([0x2d, 0x1f, 0xd0, 0x08, 0x69, 0x34, 0xac, 0xa9], 4);
pub const CLIP_SPRINT_EXIT: NameKey = ([0x1c, 0x74, 0x5e, 0x8f, 0x16, 0xf2, 0x5f, 0x6a], 8);

/// Name key of the first-person dry-fire clip (the out-of-ammunition state's clip).
// EVIDENCE: cshell+0x233ec0 (the weapon's clip indices by name: +0x108 = the dry-fire clip), cshell+0x237e20 (the
// out-of-ammunition state 10 plays clip +0x108)
pub const CLIP_DRY_FIRE: NameKey = ([0xcd, 0x3c, 0xc3, 0x13, 0x7e, 0x9b, 0x3e, 0x36], 7);

/// One clip: its index in the composed clip list (own clips first, then each child's), length and keys.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClipFact {
    pub index: usize,
    pub len_ms: u32,
    pub marks: Vec<(u32, Mark)>,
}

/// The clips of a first-person model that drive the weapon state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeaponClips {
    pub select: Option<ClipFact>,
    pub fire: Option<ClipFact>,
    /// The alternate-fire clip (the right button of a melee weapon, CLI-1350).
    pub alt_clip: Option<ClipFact>,
    pub reload: Option<ClipFact>,
    pub prefire: Option<ClipFact>,
    pub postfire: Option<ClipFact>,
    pub idle: Option<ClipFact>,
    /// The sprint clips: entry (once), loop, exit (once) ([`crate::viewanim`]).
    pub sprint_enter: Option<ClipFact>,
    pub sprint_loop: Option<ClipFact>,
    pub sprint_exit: Option<ClipFact>,
    /// The dry-fire clip (the trigger on an empty weapon).
    pub dry_fire: Option<ClipFact>,
}

fn mark_of(command: &[u8]) -> Option<Mark> {
    let t = String::from_utf8_lossy(command);
    let mut words = t.split_whitespace();
    let k = name_key(words.next()?.to_ascii_uppercase().as_bytes());
    if k == KEY_FIRE || k == KEY_ALT_FIRE {
        Some(Mark::Fire)
    } else if k == KEY_THROW {
        Some(Mark::Throw)
    } else if k == KEY_SOUND {
        words.next()?.parse().ok().map(Mark::Sound)
    } else {
        None
    }
}

impl WeaponClips {
    /// The clips of a composed clip list (`(name, length, markers)` in combined order).
    pub fn from_clips<'a>(clips: impl IntoIterator<Item = &'a dc_model::Clip>) -> WeaponClips {
        let mut out = WeaponClips::default();
        for (index, c) in clips.into_iter().enumerate() {
            let k = name_key(c.name.as_bytes().to_ascii_lowercase().as_slice());
            let fact = || ClipFact {
                index,
                len_ms: c.markers.iter().map(|m| m.time_ms).max().unwrap_or(0),
                marks: c
                    .markers
                    .iter()
                    .filter_map(|m| mark_of(m.command.as_bytes()).map(|k| (m.time_ms, k)))
                    .collect(),
            };
            let slot = if k == CLIP_SELECT {
                &mut out.select
            } else if k == CLIP_FIRE {
                &mut out.fire
            } else if k == CLIP_ALT_FIRE {
                &mut out.alt_clip
            } else if k == CLIP_RELOAD {
                &mut out.reload
            } else if k == CLIP_PREFIRE {
                &mut out.prefire
            } else if k == CLIP_POSTFIRE {
                &mut out.postfire
            } else if k == CLIP_IDLE {
                &mut out.idle
            } else if k == CLIP_SPRINT_ENTER {
                &mut out.sprint_enter
            } else if k == CLIP_SPRINT_LOOP {
                &mut out.sprint_loop
            } else if k == CLIP_SPRINT_EXIT {
                &mut out.sprint_exit
            } else if k == CLIP_DRY_FIRE {
                &mut out.dry_fire
            } else {
                continue;
            };
            if slot.is_none() {
                *slot = Some(fact());
            }
        }
        out
    }

    /// Reads a first-person model and the child models it borrows clips from.
    // SPEC: FMT_LTB.md §9 (child models lend their clips), §10.2 (markers: key times with command text)
    pub fn load(vfs: &RezVfs, path: &str) -> Result<WeaponClips, String> {
        let bytes = dc_render::read_rom_entry(vfs, path).map_err(|e| e.to_string())?;
        let body = dc_model::Model::parse(&bytes).map_err(|e| format!("model: {e:?}"))?;
        let mut all: Vec<dc_model::Clip> = body.clips.clone();
        for c in &body.child_models {
            let cp = dc_render::child_path(path, &c.to_text());
            let cb = dc_render::read_rom_entry(vfs, &cp).map_err(|e| e.to_string())?;
            let cm = dc_model::Model::parse(&cb).map_err(|e| format!("child model: {e:?}"))?;
            all.extend(cm.clips);
        }
        Ok(WeaponClips::from_clips(all.iter()))
    }

    /// A throwable: its fire clip carries the throw key.
    pub fn throwable(&self) -> bool {
        self.fire
            .as_ref()
            .is_some_and(|f| f.marks.iter().any(|m| m.1 == Mark::Throw))
    }
}

/// The sound keys of a clip (`marks`, `len_ms` long) that fall in `(from, to]` ms of a hold that plays it from 0
/// (`from` `None`: from the start, key 0 included); `looped` repeats the clip every `len_ms`. The fire key is not
/// played: the plant's outcome is the server's 0x51.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.3 (a clip's sound key plays the row's sound n and sends 0x80)
// OBSERVED: the ROM bomb row's first-person fire clip (4000 ms: sound keys 1, 2, 3 between 356 and 3192 ms, the fire
// key at 3904 ms; the row's reload sounds 1..3 are the keypad's tick / send / dial waves)
// DC-DEFAULT: CLI-1530 (proposed) the hold plays the bomb's plant clip keys
pub fn hold_marks_due(
    marks: &[(u32, Mark)],
    len_ms: u32,
    from: Option<u32>,
    to: u32,
    looped: bool,
) -> Vec<u8> {
    let mut out = Vec::new();
    let keys = || {
        marks.iter().filter_map(|(at, m)| match m {
            Mark::Sound(n) => Some((*at, *n)),
            _ => None,
        })
    };
    let laps = if looped && len_ms > 0 { to / len_ms } else { 0 };
    for lap in 0..=laps {
        let base = lap.saturating_mul(len_ms);
        for (at, n) in keys() {
            let t = base.saturating_add(at);
            if t <= to && from.is_none_or(|f| t > f) {
                out.push(n);
            }
        }
        if !looped {
            break;
        }
    }
    out
}

/// Timings used when the weapon has no first-person clips (no ROM, or a row without a model).
// DC-DEFAULT: CLI-29 (proposed) 105 ms per shot (OBSERVED tdm1: weapon 145 fired every 104-110 ms in its bursts,
// the length of its fire clip), 500 ms select with the draw sound 3 at its end (task value; the observed draw sounds
// came 0.25-0.73 s after the select), 2000 ms reload with sounds 1, 2, 3, the throwable's 700 ms wind-up with sound
// 10 near its end, 67 ms hold passes and a 366 ms release clip with the release at 133 ms (the ROM grenade's clips)
pub mod fallback {
    pub const FIRE_MS: u32 = 105;
    pub const SELECT_MS: u32 = 500;
    pub const SELECT_SOUND: (u32, u8) = (500, 3);
    pub const RELOAD_MS: u32 = 2000;
    pub const RELOAD_SOUNDS: [(u32, u8); 3] = [(400, 1), (850, 2), (1700, 3)];
    pub const THROW_PRE_MS: u32 = 700;
    pub const THROW_PRE_SOUND: (u32, u8) = (513, 10);
    pub const THROW_HOLD_MS: u32 = 67;
    pub const THROW_POST_MS: u32 = 366;
    pub const THROW_RELEASE_AT: u32 = 133;
    /// The dry-fire state without a clip: weapon sound 7 (dry fire) at once, 300 ms.
    // DC-DEFAULT: CLI-499 (see DC_DEFAULTS.md)
    pub const DRY_FIRE_MS: u32 = 300;
    pub const DRY_FIRE_SOUND: (u32, u8) = (0, 7);
}

// --------------------------------------------------------------------------------------------- spread

/// Most distance a shot ray is traced when the weapon row gives no range (the shot cone itself is
/// [`crate::cone`], which replaced the CLI-31 disk).
// EVIDENCE: cshell+0x17f7b0 (Range binary default 3000); SPEC: WEAPON_FIRE_2009.md §5.2
pub const DEFAULT_RANGE: f32 = crate::cone::row_default::RANGE;

// --------------------------------------------------------------------------------------------- geometry

fn add_scaled(a: [f32; 3], d: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t]
}

/// Forward axis of yaw / pitch (positive pitch looks down): the camera's +Z.
// SPEC: MOVEMENT_2009.md §6.3; GAME_MESSAGES_2009_C2S.md §2.1.1 (aim_q = Ry(yaw) * Rx(pitch), forward = rotated +Z)
pub fn forward(yaw: f32, pitch: f32) -> [f32; 3] {
    dc_botclient::shooter::forward(yaw, pitch)
}

/// `weapon_pos`: the origin plus [`WEAPON_OFFSET`] turned by the yaw.
pub fn weapon_pos(origin: [f32; 3], yaw: f32) -> [f32; 3] {
    let (s, c) = yaw.sin_cos();
    let [r, u, f] = WEAPON_OFFSET;
    [
        origin[0] + r * c + f * s,
        origin[1] + u,
        origin[2] - r * s + f * c,
    ]
}

/// A remote character the shots are tested against (the box drawn for it this frame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub obj: u16,
    pub centre: [f32; 3],
    pub half: [f32; 3],
}

/// Distance along a unit ray at which the world stops it (`None`: nothing within `range`). Player-only blockers do
/// not stop shots.
// SPEC: FMT_DAT85.md §9.1 (blockers are player clip geometry); MOVEMENT_2009.md §9.6 (ray through the world)
pub fn world_hit(world: &dyn Collider, origin: [f32; 3], dir: [f32; 3], range: f32) -> Option<f32> {
    let mut from = V3::from_array(origin);
    let to = V3::from_array(add_scaled(origin, dir, range));
    let mut travelled = 0.0f32;
    for _ in 0..8 {
        let seg = (to - from).len();
        if seg.is_nan() || seg <= 1e-3 {
            return None;
        }
        let h = world.sweep(&dc_sim::Cylinder::POINT, from, to)?;
        let d = h.t.clamp(0.0, 1.0) * seg;
        if h.model != dc_sim::BLOCKER_MODEL {
            return Some(travelled + d);
        }
        // step past the blocker and go on
        let step = d + 1.0;
        travelled += step;
        from += V3::from_array(dir) * step;
    }
    None
}

/// What a shot ray hit first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RayHit {
    /// A remote character's box at this distance, with the node the fire message names (0xff: none) and the
    /// normal of the box face the ray entered ([`crate::hitnodes`]).
    Character {
        obj: u16,
        dist: f32,
        node: u8,
        normal: [f32; 3],
    },
    /// The world (or nothing) first: no object block.
    Miss,
}

/// The ray against the remote boxes first, then the world: a box counts when the world does not stop the ray
/// before it. The HUD's aim test uses it (CLI-117); the shots use the 2009 node-sphere test of
/// [`crate::hitnodes::trace`] (CLI-300).
// DC-DEFAULT: CLI-43 (proposed) the hit volume is the box drawn for the remote player (the real client traced its
// model: its tdm4 hit points lay 24-27 units above the box centre, C2S §4.1.1); a world hit or a miss sends the
// 623-bit form (C2S §4.1.1: world hits carry has_hit 0, PLAUSIBLE)
pub fn trace(
    origin: [f32; 3],
    dir: [f32; 3],
    range: f32,
    targets: &[Target],
    world: Option<&dyn Collider>,
) -> RayHit {
    let best = targets
        .iter()
        .filter_map(|t| {
            dc_botclient::shooter::ray_box_entry(origin, dir, t.centre, t.half)
                .filter(|d| *d <= range)
                .map(|d| (d, t.obj))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let Some((dist, obj)) = best else {
        return RayHit::Miss;
    };
    if let Some(w) = world {
        if world_hit(w, origin, dir, dist).is_some_and(|wd| wd < dist) {
            return RayHit::Miss;
        }
    }
    RayHit::Character {
        obj,
        dist,
        node: crate::hitnodes::NO_NODE,
        normal: HIT_VECTOR,
    }
}

// --------------------------------------------------------------------------------------------- 0x82 builder

/// Everything one `UP_FIRE` is built from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotInputs {
    pub weapon_id: u16,
    pub ammo_id: u16,
    pub weapon_pos: [f32; 3],
    pub origin: [f32; 3],
    /// The view rotation (`quat128` x, y, z, w; the 0x64 `aim_q` of the same frame).
    pub aim: [f32; 4],
    pub yaw: f32,
    pub dir: [f32; 3],
    pub seed: u8,
    /// The spread fraction before the shot.
    pub spread: f32,
    pub shot_scale: f32,
    /// The zoom state (the 0x82 `moving` bit) and the triple-shot mode (`wflag`).
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-0 item 4, AC-3.3 (the zoom byte of the cone = the 0x82 bit)
    pub zoom: bool,
    pub triple: bool,
    /// The alternate fire (the 0x82 `mode1` bit: the weapon's fire mode is 1).
    // DISASM: cshell+0x238b10 (`mode1` = weapon +0xd4 == 1), cshell+0x237180 (+0xd4 := the update's mode argument)
    pub alt: bool,
    pub time_ms: u32,
    pub hit: RayHit,
    pub last: bool,
    /// The shot counter after the pellet's draws.
    pub stat: u32,
}

/// One `UP_FIRE` 0x82 in the 2009 client's form: 887 bits for a character hit (extra block with the hit object, the
/// part byte, the hit point and the constant vector; `hit_dist` and `hit_obj`), 623 bits otherwise.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1, §4.1.1; OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §3
pub fn fire_report(s: &ShotInputs) -> FireReport {
    let (extra, hit) = match s.hit {
        // EVIDENCE: cshell+0x238eb0 (the extra block: the node pick, the hit point and the hit plane's normal)
        RayHit::Character {
            obj,
            dist,
            node,
            normal,
        } => (
            Some(FireExtra {
                obj,
                kind: node,
                a: add_scaled(s.origin, s.dir, dist),
                b: normal,
            }),
            Some((dist, obj)),
        ),
        RayHit::Miss => (None, None),
    };
    FireReport {
        wkind: 0,
        weapon_id: s.weapon_id,
        ammo_id: s.ammo_id,
        weapon_pos: s.weapon_pos,
        origin: s.origin,
        aim: s.aim,
        yaw: s.yaw,
        dir: s.dir,
        seed: s.seed,
        moving: s.zoom,
        wflag: s.triple,
        mode1: s.alt,
        frac: (s.spread.clamp(0.0, 1.0) * 255.0) as u8,
        shot_no: s.shot_scale.to_bits(),
        time_ms: s.time_ms,
        extra,
        flag_c: false,
        last: s.last,
        hit,
        stat: s.stat,
    }
}

/// The body of [`fire_report`].
pub fn fire_message(s: &ShotInputs) -> BitWriter {
    fire_body(&fire_report(s))
}

// --------------------------------------------------------------------------------------------- the shot counter's writers

/// Server shell ids that write the shot counter.
pub mod down {
    /// `GS_MATCH_SETTINGS`: the counter := `own_value`.
    pub const MATCH_SETTINGS: u8 = 0x7d;
    /// `GS_PLAYER_POINTS`: kind 1 adds `delta` to the counter.
    pub const PLAYER_POINTS: u8 = 0xda;
    /// The `GS_PLAYER_POINTS` kind that advances the counter.
    pub const POINTS_SHOT_SEED: u8 = dc_botclient::shell::POINTS_SHOT_SEED;
}

/// `own_value` of a `GS_MATCH_SETTINGS` 0x7d body (after the shell id):
/// `[lobby_caption:zstr8][unused:u8][own_team:i8][own_value:u32]...`; `None` when the body ends before it.
// SPEC: GAME_MESSAGES_2009_S2C.md 0x7d row (cshell+0x2c4ff..0x2c724: the third read, 32 bits, := local_player+0x1fc)
pub fn match_settings_shot_seed(r: &mut BitReader<'_>) -> Option<u32> {
    dc_botclient::shell::read_match_shot_seed(r)
}

/// `delta` of a `GS_PLAYER_POINTS` 0xda body of kind 1 (`[kind:u8][delta:u32]`); `None` for another kind.
// SPEC: GAME_MESSAGES_2009_S2C.md 0xda row (kind 1: local_player+0x1fc += delta, cshell+0x61521..0x61539)
pub fn player_points_shot_seed(r: &mut BitReader<'_>) -> Option<u32> {
    dc_botclient::shell::read_points_shot_seed(r)
}

// --------------------------------------------------------------------------------------------- the inventory

/// One weapon in an inventory slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Carried {
    pub slot: u8,
    pub weapon: u16,
    pub ammo_id: u16,
    pub clip: u32,
    pub stock: u32,
    pub clip_size: Option<u32>,
    /// "grant" (0xb2 / 0xb3) or "lobby" (0x323 + the ROM row).
    pub source: &'static str,
}

impl Carried {
    pub fn total(&self) -> u32 {
        self.clip + self.stock
    }
}

/// What the weapon is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Select,
    Fire,
    Reload,
    /// A throwable's wind-up, the held loop, and the release.
    ThrowPre,
    ThrowHold,
    ThrowRelease,
    /// The trigger on a weapon with no round left (2009 state 10, out of ammunition): the dry-fire clip.
    // EVIDENCE: cshell+0x23b340 / WEAPON_FIRE_2009.md §6 (state 10), cshell+0x237e20 (its clip)
    DryFire,
    /// A gun's postfire clip after its fire clip when no next shot follows (the pistol lowering from the recoil); a new
    /// press, the reload key or a select cuts it short.
    PostFire,
}

impl Phase {
    pub fn label(&self) -> &'static str {
        match self {
            Phase::Idle => "ready",
            Phase::Select => "select",
            Phase::Fire => "fire",
            Phase::Reload => "reload",
            Phase::ThrowPre | Phase::ThrowHold => "hold",
            Phase::ThrowRelease => "throw",
            Phase::DryFire => "dry",
            Phase::PostFire => "postfire",
        }
    }
}

/// The frame's weapon commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WeaponCommands {
    /// The fire button is held.
    pub fire: bool,
    /// The fire button went down since the last frame (it may be up again by now): the 2009 trigger press is an
    /// event (cshell+0x23b050), not a level read once per frame, so a press is never lost between two frames.
    // DC-DEFAULT: CLI-1601 (proposed)
    pub fire_press: bool,
    /// The reload key was pressed since the last frame.
    pub reload: bool,
    /// The weapon inspect key was pressed since the last frame (DC-DEFAULT: CLI-1845).
    pub inspect: bool,
    /// The use key was pressed since the last frame (the frame loop asks for the drop under the player, CLI-675).
    pub use_key: bool,
    /// The use key is held down (a hold to plant / defuse, CLI-1054).
    pub use_held: bool,
    /// A weapon slot was chosen (0-based) since the last frame.
    pub select: Option<u8>,
    /// Mouse wheel steps since the last frame (positive = next slot).
    pub wheel: i32,
    /// The zoom key is held (command 0x3d).
    pub zoom: bool,
    /// A class key was pressed since the last frame (0-based: key 1 is 0; it cycles order class `n + 1`).
    pub class_key: Option<u8>,
}

/// Where the shots start and where they go this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aim {
    /// The shot origin: the body position the 0x64 reports plus its eye offset.
    pub origin: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// The control word of the frame (the fire bit is suppressed while sprinting).
    pub control: u32,
}

/// A message for the server (all weapon messages are reliable, C2S §1.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Outgoing {
    pub shell: u8,
    pub body: BitWriter,
}

/// Counters for reports and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArmsStats {
    pub selects: u32,
    pub shots: u32,
    pub fire_msgs: u32,
    pub hits: u32,
    pub reloads: u32,
    pub holds: u32,
    pub throws: u32,
    /// Wind-ups / holds a weapon switch cancelled (the throwable kept, nothing thrown; CLI-676).
    pub throw_cancels: u32,
    pub sounds: u32,
    /// What the node-sphere test did with the pellets ([`crate::hitnodes`]).
    pub nodes: crate::hitnodes::NodeStats,
}

/// Settings of the inventory.
#[derive(Debug, Clone, Default)]
pub struct ArmsConfig {
    pub table: Option<std::sync::Arc<WeaponTable>>,
    /// The ROM folder for the first-person clips (`None`: the fallback timings).
    pub rom: Option<std::path::PathBuf>,
    /// The lobby's equipment slots 0x323 +0x12 (item numbers, -1 empty): slot `i` = weapon slot `i`.
    pub lobby_slots: Vec<i32>,
    pub seed: u64,
    /// The 2009 options-menu "Reality" setting: hits jar the camera ([`crate::camfx`]). Off by default, as the
    /// 2009 console value registers.
    // DC-DEFAULT: CLI-313 (proposed) off; `--reality` turns it on
    pub reality: bool,
    /// The room's weapon limit (the waiting room's weapon list row, 0 = all permitted, [`dc_bute::arms`]).
    pub arms_limit: u32,
    /// Weapon number -> the item table's `ItemType` (the kinds the weapon limit names); a weapon missing here is
    /// never barred.
    pub weapon_types: std::sync::Arc<BTreeMap<u16, i32>>,
    /// CLI-1627: the room's Hacks setting has no-recoil (LG-274): own shots kick no view recoil and every pellet
    /// leaves along the cone's centre (the table's draws still run, so the 0x82 `stat` counts as before).
    pub no_recoil: bool,
}

/// The weapon slots the room's weapon limit applies to: the lobby loadout's slots (0..3 and the backpack 4..7);
/// mode items (the bomb, slot 9) are the mode's.
// SPEC: DSERVER_PIPE.md §12.A (slots 0..3 weapons, 10..13 backpack weapons -> weapon slots 4..7)
pub const LIMITED_SLOTS: std::ops::Range<u8> = 0..8;
/// The first backpack weapon slot.
pub const BACKPACK_FIRST_SLOT: u8 = 4;

/// How long after a spawn the client waits for the server's grants before it uses the lobby slots.
// DC-DEFAULT: CLI-29 (proposed) 500 ms (BOT-15 for the bot)
pub const GRANT_WAIT_MS: u32 = 500;
/// Item numbers below this are weapons.
// SPEC: PROTOCOL_2009.md §3.3 (0x323 item numbers); OBSERVED: tdm4 0x323 slots 0..3 = the 0xb2 weapon numbers
pub const WEAPON_ITEM_LIMIT: i32 = 1000;
/// Weapon slots the number keys reach (1-4) when the ROM gives no weapon order.
pub const KEY_SLOTS: u8 = 4;
/// The highest order class a class key reaches (2009 numbering).
// EVIDENCE: cshell+0x22e530 (a class above 7 returns 0xffff)
pub const CLASS_KEY_MAX: u8 = 7;
/// The order class whose key passes over entries that may not be drawn (the throwables, 2009 numbering).
// EVIDENCE: cshell+0x22e530 (class 4: the loop over the class entries until one passes slot +0x84, the has-ammo test)
pub const THROW_CLASS: u8 = 4;

/// The weapon state of the local player.
#[derive(Debug)]
pub struct Arsenal {
    cfg: ArmsConfig,
    vfs: Option<RezVfs>,
    clips: BTreeMap<u16, Option<(String, WeaponClips)>>,
    pub slots: BTreeMap<u8, Carried>,
    held: Option<u8>,
    phase: Phase,
    phase_at: u32,
    /// Keys of the playing clip already handled (times up to this, ms into the clip).
    marks_done: Option<u32>,
    /// The plant / defuse hold's clip keys already played (ms into the hold, CLI-1530).
    hold_marks_done: Option<u32>,
    alive: bool,
    /// A live world camera holds the weapons ([`Arsenal::set_cinematic`]).
    cinematic: bool,
    spawned_at: Option<u32>,
    need_select: bool,
    /// A slot the server asked to draw (0xb3).
    pending_select: Option<u8>,
    trigger: bool,
    /// A Possession move runs (CLI-1642): the fire key does not count, the new gun's draw plays on.
    glide: bool,
    /// The trigger went down since the last shot (single-shot weapons fire once per press).
    pressed: bool,
    /// The trigger of this step is the alternate fire (the zoom key of a melee weapon, CLI-1350).
    alt_wanted: bool,
    /// The running fire state is the alternate fire: its clip and the 0x82 `mode1` bit.
    alt: bool,
    release_wanted: bool,
    /// A switch cancelled a throw while the fire key was down: the trigger counts again only after the key went up.
    // DC-DEFAULT: CLI-676 (proposed)
    fire_latched: bool,
    /// The fire key as pressed, sprint or not: a throwable's hold ends only when this goes up.
    // OBSERVED: CAPTURE_tdm4_KILL_VALIDATION.md finding 1 (106.531 .. 106.797: the player sprinted with the grenade
    // held, the 0x400 bit was off and the hold went on to the release at 108.000); GAME_MESSAGES_2009_C2S.md §4.6
    // (0x4d 62-78 ms apart "with a 406 ms gap while the player sprinted", then the release)
    throw_key: bool,
    /// The reload key was pressed and not handled yet (the weapon was busy).
    reload_wanted: bool,
    /// Spread and recoil state per weapon number (the 2009 client keeps them on each weapon object).
    spreads: BTreeMap<u16, crate::spread::SpreadState>,
    motion: crate::spread::Motion,
    /// View pitch change from recoil not yet taken by the frame loop (radians, positive looks down).
    view_pitch: f32,
    /// View yaw change from recoil not yet taken by the frame loop (radians, added to the yaw).
    view_yaw: f32,
    /// Clock of the last scope-sway frame (`None` while the scope is down).
    sway_ms: Option<u32>,
    /// The view's recoil (the 2009 controller keeps it, not the weapon).
    pub recoil: crate::recoil::CameraRecoil,
    /// The camera jar of hits on the own player.
    pub camfx: crate::camfx::CameraFx,
    /// A shot left in this step (the spread steps ran with it).
    shot_this_step: bool,
    last_step: Option<u32>,
    trigger_latch_until: Option<u32>,
    reload_event_at: Option<(u32, u8)>,
    own_id: u8,
    rng: u64,
    /// The cone's random table (the ROM entry, else the generator), loaded at the first shot.
    shot_table: Option<std::sync::Arc<crate::cone::ShotTable>>,
    /// The shot counter: set by 0x7d, advanced by 0xda kind 1 and by two draws per pellet; the 0x82 `stat`.
    pub shot_counter: crate::cone::ShotCounter,
    /// The scope ([`crate::zoom`]) and a test override of its cone state ([`Arsenal::set_zoom`]).
    scope: crate::zoom::Zoom,
    zoom_override: Option<bool>,
    /// The zoom key was held in the last step (the press edge cycles the level) and the control word was sprinting.
    zoom_key: bool,
    was_sprinting: bool,
    inspector: crate::inspect::Inspector,
    /// The normal horizontal field of view of the frame (radians) the zoom changes run against.
    normal_fov_x: f32,
    /// The triple-shot mode of the held weapon.
    triple: bool,
    /// Mod rows in the three mod slots of each weapon (none until a source of the attached mods is read).
    mods: BTreeMap<u16, [Option<u16>; 3]>,
    pub stats: ArmsStats,
    /// The last frame's aim (for the 0x80 positions between shots).
    last_aim: Option<Aim>,
    /// One entry per pellet fired since the last [`Arsenal::take_shot_fx`] (for the weapon effects, `crate::fx`).
    shot_fx: Vec<ShotFx>,
    /// The node spheres of this frame's characters ([`Arsenal::set_hit_nodes`]).
    hit_nodes: crate::hitnodes::FrameNodes,
    /// The slot last drawn of each order class (2009 numbering), for the empty switch.
    // EVIDENCE: cshell+0x22e930 (every change writes the drawn slot into the entry of the weapon's class,
    // 0x22eab9), cshell+0x22ec00 (the empty switch reads the class 1 / 2 / 3 entries)
    class_slots: BTreeMap<u8, u8>,
    /// The `class_pos` of each granted slot (the order of the slot in its class list).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (`class_pos` = the position of the weapon in its class list)
    class_pos: BTreeMap<u8, u8>,
    /// The first-person sprint clips of the held weapon ([`crate::viewanim`]).
    pub sprint_anim: crate::viewanim::SprintAnim,
}

impl Arsenal {
    pub fn new(cfg: ArmsConfig) -> Arsenal {
        let vfs = cfg.rom.as_deref().map(RezVfs::new);
        Arsenal {
            rng: cfg.seed | 1,
            vfs,
            clips: BTreeMap::new(),
            slots: BTreeMap::new(),
            held: None,
            phase: Phase::Idle,
            phase_at: 0,
            marks_done: None,
            hold_marks_done: None,
            alive: false,
            cinematic: false,
            spawned_at: None,
            need_select: false,
            pending_select: None,
            trigger: false,
            glide: false,
            pressed: false,
            alt_wanted: false,
            alt: false,
            release_wanted: false,
            fire_latched: false,
            throw_key: false,
            reload_wanted: false,
            spreads: BTreeMap::new(),
            motion: crate::spread::Motion::default(),
            view_pitch: 0.0,
            view_yaw: 0.0,
            sway_ms: None,
            recoil: crate::recoil::CameraRecoil::default(),
            camfx: crate::camfx::CameraFx::new(
                cfg.reality,
                cfg.rom
                    .as_deref()
                    .filter(|_| cfg.reality)
                    .map(crate::camfx::DamageJar::open)
                    .unwrap_or_default(),
            ),
            shot_this_step: false,
            last_step: None,
            trigger_latch_until: None,
            reload_event_at: None,
            own_id: 0,
            shot_table: None,
            shot_counter: crate::cone::ShotCounter::default(),
            scope: crate::zoom::Zoom::default(),
            zoom_override: None,
            zoom_key: false,
            was_sprinting: false,
            inspector: crate::inspect::Inspector::default(),
            normal_fov_x: dc_render::fov_pair_2009(dc_render::FOV_Y_DEFAULT_DEG, 4, 3).0,
            triple: false,
            mods: BTreeMap::new(),
            stats: ArmsStats::default(),
            last_aim: None,
            shot_fx: Vec::new(),
            cfg,
            hit_nodes: crate::hitnodes::FrameNodes::default(),
            class_slots: BTreeMap::new(),
            class_pos: BTreeMap::new(),
            sprint_anim: crate::viewanim::SprintAnim::default(),
        }
    }

    pub fn config(&self) -> &ArmsConfig {
        &self.cfg
    }

    /// The own engine client id for `UP_WEAPON_SOUND`.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.3 (`own_id` = engine_client +0x84, the id of S2C_CLIENT_ID)
    pub fn set_own_id(&mut self, id: u8) {
        self.own_id = id;
    }

    /// The bomb's weapon row: the weapon the server granted in the bomb slot, else the first weapon of the last
    /// class block of the weapon order (the mission class, whose one ROM row is the attackers' bomb).
    // EVIDENCE: cshell+0x22d2b0 (the bomb item is inventory entry 9 in classes 2 / 3), cshell+0x180590 (the class
    // blocks of the order, the mission block last); OBSERVED: ROM table WEAPONS.TXT [WeaponOrder] (one row in the last
    // class, the row the S&D GMS files name as the attackers' weapon)
    // DC-DEFAULT: CLI-1531 (proposed) a defender, who never carries it, takes the order's mission row
    pub fn bomb_row(&self) -> Option<u16> {
        if let Some(c) = self.slots.get(&crate::hud::bomb::BOMB_SLOT) {
            return Some(c.weapon);
        }
        let t = self.cfg.table.as_ref()?;
        let last = t.classes.values().copied().max()?;
        t.commands
            .iter()
            .copied()
            .find(|w| t.classes.get(w) == Some(&last))
    }

    /// The plant / defuse hold's sounds (CLI-1530 / CLI-1531): the sound keys of `weapon`'s first-person fire clip
    /// (the bomb's plant clip) from the start of the hold to `since_start` play as the own weapon sounds and go to the
    /// server as `UP_WEAPON_SOUND` 0x80 (the others hear them through its relay), each key once; `looped` starts the
    /// clip again at its end (the defuse runs longer than the clip). `None` ends the hold's sounds.
    pub fn hold_sounds(
        &mut self,
        weapon: u16,
        since_start: Option<u32>,
        looped: bool,
        aim: &Aim,
    ) -> Vec<Outgoing> {
        let Some(t) = since_start else {
            self.hold_marks_done = None;
            return Vec::new();
        };
        let Some(fire) = self.clips_of(weapon).and_then(|c| c.fire.clone()) else {
            return Vec::new();
        };
        let due = hold_marks_due(&fire.marks, fire.len_ms, self.hold_marks_done, t, looped);
        self.hold_marks_done = Some(t);
        let pos = self.muzzle_point(weapon, aim);
        due.into_iter()
            .map(|n| {
                self.stats.sounds += 1;
                Outgoing {
                    shell: up::WEAPON_SOUND,
                    body: weapon_sound_body(n, weapon, self.own_id, pos),
                }
            })
            .collect()
    }

    pub fn held(&self) -> Option<&Carried> {
        self.held.and_then(|s| self.slots.get(&s))
    }

    /// `GS_MATCH_SETTINGS` 0x7d `own_value`: the shot counter := `v`.
    // SPEC: GAME_MESSAGES_2009_S2C.md 0x7d row; WEAPON_FIRE_2009_addendum_cone.md AC-1.5
    pub fn on_shot_seed(&mut self, v: u32) {
        self.shot_counter.set(v);
    }

    /// `GS_PLAYER_POINTS` 0xda kind 1 `delta`: the shot counter += `delta`.
    // SPEC: GAME_MESSAGES_2009_S2C.md 0xda row; WEAPON_FIRE_2009_addendum_cone.md AC-1.5
    pub fn on_shot_seed_add(&mut self, delta: u32) {
        self.shot_counter.add(delta);
    }

    /// Forces the zoom state the cone and the 0x82 `moving` bit read (tests of the cone); the scope itself
    /// ([`Arsenal::scope`]) is driven by the zoom key.
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-0 item 4, AC-2.1, AC-3.3
    pub fn set_zoom(&mut self, on: bool) {
        self.zoom_override = Some(on);
    }

    /// The zoom state the cone, the 0x82 `moving` byte, the 0x64 `pu_flags` 0x0800, the spread steps and the sprint
    /// test read (zoom level >= 1 or a change running).
    // DISASM: cshell+0x23bca0 (fire routine zoom byte); EVIDENCE: cshell+0x232120 / cshell+0x2319b0 (spread steps)
    pub fn zoom_active(&self) -> bool {
        self.zoom_override.unwrap_or_else(|| self.scope.active())
    }

    /// The scope state ([`crate::zoom::Zoom`]).
    pub fn scope(&self) -> &crate::zoom::Zoom {
        &self.scope
    }

    /// The control word's zoom bit of this frame: the zoom key is held and the control word is not sprinting.
    // SPEC: MOVEMENT_2009.md §5.1 (0x80000: command 0x3d on and not SPRINT); OBSERVED: rust6 / rust7 updates.csv
    // (the bit only while the key is held, never together with SPRINT 0x400000)
    pub fn zoom_control_bits(&self, control: u32) -> u32 {
        if self.alive && self.zoom_key && control & dc_sim::player::bits::SPRINT == 0 {
            crate::zoom::ZOOM_CONTROL_BIT
        } else {
            0
        }
    }

    /// The normal horizontal field of view of the frame (radians); the zoom changes run against it.
    pub fn set_normal_fov_x(&mut self, fov_x: f32) {
        if fov_x.is_finite() && fov_x > 0.0 {
            self.normal_fov_x = fov_x;
        }
    }

    /// The horizontal field of view the camera uses at `now` (radians), `None` for the normal field.
    // EVIDENCE: cshell+0x1f9100 (the camera field := the zoom field; the vertical one scaled by the same ratio)
    pub fn zoom_fov_x(&self, now: u32) -> Option<f32> {
        self.scope.fov_x(now)
    }

    /// The zoom sounds asked since the last call (played locally).
    pub fn take_zoom_sounds(&mut self) -> Vec<crate::zoom::ZoomSound> {
        self.scope.take_sounds()
    }

    /// Server 0x77 `GS_OVERLAY`.
    // SPEC: GAME_MESSAGES_2009_S2C.md 0x77 row (cshell+0x1b90e0)
    pub fn on_overlay(&mut self, slot: u8, on: bool) {
        let sight = self.sight_of(self.held().map(|c| c.weapon)).cloned();
        self.scope.server_overlay(slot, on, sight.as_ref());
    }

    /// The mod row in socket `socket` of `weapon`.
    fn mod_in(&self, weapon: u16, socket: usize) -> Option<&crate::zoom::ModInfo> {
        let id = self.mods.get(&weapon)?.get(socket).copied().flatten()?;
        self.cfg.table.as_ref()?.mod_info.get(&id)
    }

    /// The sight mod (socket 1) of `weapon`.
    fn sight_of(&self, weapon: Option<u16>) -> Option<&crate::zoom::ModInfo> {
        self.mod_in(weapon?, crate::zoom::SIGHT_SOCKET)
    }

    /// The silencer level of the weapon's muzzle mod (socket 0) when it picks the silenced fire sound (weapon sound 9),
    /// else `None` (weapon sound 6).
    // EVIDENCE: cshell+0x2330e0 (the own fire sound number: the mod at weapon +0x14 = socket 0 with 0 < SilencerLevel <
    // 10 -> 9, else 8 in the alternate mode, else 6), cshell+0x2368dd (sound 9 = the row's silenced sound
    // `SilencerLevel - 1`)
    pub fn silencer_level(&self, weapon: u16) -> Option<u8> {
        self.mod_in(weapon, crate::zoom::MUZZLE_SOCKET)?
            .silencer_level()
    }

    /// Files a grant's part bytes into the weapon's three mod sockets: each part names a `[Mod<n>]` row and goes to
    /// the socket of the row's `Kinds` (a part of no known row, or of a `Kinds` / `Type` out of range, is dropped);
    /// the sockets are cleared first, the finish byte is not a mod.
    // EVIDENCE: cshell+0x60f20..0x6118e (0xb2 / 0xb3: weapon slot +0x108 clears the three attachment points, then
    // +0x104(part, 0, 1) per part != -1); DISASM: cshell+0x235ec0 (attach: mod row exists, +8 in 0..6, `Kinds` in 0..2 ->
    // weapon +0x14 + 0x2c x Kinds; the grant's force flag replaces)
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (`[part1][part2][part3][finish]`, -1 none); DSERVER_PIPE_addendum_parts.md
    // §2 (the client files by `Kinds`, the position carries no meaning)
    pub fn file_parts(&mut self, weapon: u16, parts: [i8; 4]) {
        let mut sockets: [Option<u16>; 3] = [None; 3];
        if let Some(t) = self.cfg.table.as_ref() {
            for p in parts.iter().take(3).filter(|p| **p >= 0) {
                let id = *p as u16;
                if let Some(s) = t.mod_info.get(&id).and_then(|m| m.attach_socket()) {
                    sockets[s] = Some(id);
                }
            }
        }
        self.mods.insert(weapon, sockets);
    }

    /// The rounds the magazine mod filed for `weapon` adds to its clip: the mod row's `ShotsPerClipPlus<k>` with `k`
    /// the weapon row's `ModModel` for that mod; 0 without a magazine mod or with `k` outside 0..9.
    // EVIDENCE: cshell+0x233430 (row ShotsPerClip + the socket-2 mod's ShotsPerClipPlus[ModModel]), cshell+0x172b20,
    // cshell+0x172df0
    pub fn clip_bonus(&self, weapon: u16) -> i32 {
        let (Some(t), Some(m)) = (self.cfg.table.as_ref(), self.mods_of(weapon)[2]) else {
            return 0;
        };
        let k = t.attach.mod_model_of(weapon, m);
        usize::try_from(k)
            .ok()
            .and_then(|k| t.mod_info.get(&m).and_then(|i| i.clip_plus.get(k).copied()))
            .unwrap_or(0)
    }

    /// The clip size of a row clip `size` with the magazine mod of `weapon`: the signed sum, at least 1 (a Fast
    /// Loader takes rounds away).
    // EVIDENCE: cshell+0x233430 (row ShotsPerClip + ShotsPerClipPlus[ModModel], no bound)
    // DC-DEFAULT: CLI-1440 (proposed) a sum below 1 counts as 1 (no ROM pair reaches it)
    pub fn modded_clip(&self, weapon: u16, size: u32) -> u32 {
        let sum = i64::from(size) + i64::from(self.clip_bonus(weapon));
        u32::try_from(sum.max(1)).unwrap_or(u32::MAX)
    }

    /// The play rate of the fire clips (`fire` true) or of the reload clip of `weapon`: 1 + the three fitted mods'
    /// `Fireanirate` / `Reloadanirate`.
    // EVIDENCE: cshell+0x238110 (fire: 1 + sum of mod +0x2a0), cshell+0x23a210 (reload: 1 + sum of mod +0x2b8)
    // DC-DEFAULT: CLI-1441 (proposed) a rate below 0.05 plays at 0.05 (the 2009 animation would never end)
    pub fn anim_rate(&self, weapon: u16, fire: bool) -> f32 {
        let sum: f32 = (0..3)
            .filter_map(|s| self.mod_in(weapon, s))
            .map(|m| {
                if fire {
                    m.fire_clip_rate
                } else {
                    m.reload_clip_rate
                }
            })
            .sum();
        (1.0 + sum).max(MIN_ANIM_RATE)
    }

    /// The recoil terms of the mods fitted to `weapon`.
    pub fn kick_mods(&self, weapon: u16) -> crate::recoil::KickMods {
        crate::recoil::KickMods::of_sockets(
            [0, 1, 2].map(|s| self.mod_in(weapon, s).map(|m| &m.recoil)),
        )
    }

    /// The mod numbers in the three sockets of `weapon` (muzzle, sight, magazine).
    pub fn mods_of(&self, weapon: u16) -> [Option<u16>; 3] {
        self.mods.get(&weapon).copied().unwrap_or([None; 3])
    }

    /// The triple-shot mode: the triple-shot cone bounds and the 0x82 `wflag` bit.
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-2.1, AC-3.3 (the triple byte = the current weapon's mode)
    pub fn set_burst_mode(&mut self, on: bool) {
        self.triple = on;
    }

    /// The `[Mod<n>]` numbers in the three mod slots of `weapon` (the grants file them through
    /// [`Arsenal::file_parts`]).
    pub fn set_mods(&mut self, weapon: u16, slots: [Option<u16>; 3]) {
        self.mods.insert(weapon, slots);
    }

    /// The cone's random table (loaded once: the ROM entry when the ROM has one, else the generator).
    pub fn shot_table(&mut self) -> std::sync::Arc<crate::cone::ShotTable> {
        if self.shot_table.is_none() {
            let t = crate::cone::ShotTable::open(self.cfg.rom.as_deref());
            self.shot_table = Some(std::sync::Arc::new(t));
        }
        self.shot_table.clone().expect("loaded above")
    }

    fn mod_slots(&self, weapon: u16) -> crate::cone::ModSlots {
        let ids = self.mods.get(&weapon).copied().unwrap_or([None; 3]);
        let table = self.cfg.table.as_ref();
        ids.map(|i| i.and_then(|i| table.and_then(|t| t.mods.get(&i)).copied()))
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The spread float (0x64 `weapon_f`): the held weapon's [`crate::spread::SpreadState::value`].
    pub fn spread(&self) -> f32 {
        self.held()
            .and_then(|c| self.spreads.get(&c.weapon))
            .map_or(0.0, |s| s.value())
    }

    /// The player's movement for the spread steps of the next [`Arsenal::step`].
    pub fn set_motion(&mut self, m: crate::spread::Motion) {
        self.motion = m;
    }

    /// The recoil's view pitch change since the last call (radians, positive looks down); the frame loop adds it to
    /// the view.
    pub fn take_view_pitch(&mut self) -> f32 {
        std::mem::take(&mut self.view_pitch)
    }

    fn spread_row(&self, weapon: u16) -> crate::spread::SpreadRow {
        self.cfg
            .table
            .as_ref()
            .and_then(|t| t.spread.get(weapon))
            .cloned()
            .unwrap_or_default()
    }

    fn spread_rates(&self) -> crate::spread::PerturbRates {
        self.cfg
            .table
            .as_ref()
            .map(|t| t.spread.rates)
            .unwrap_or_default()
    }

    /// The aim_mark of the held weapon at `now` (`None`: dead, no weapon, or a row without one).
    pub fn aim_mark(&self, now: u32) -> Option<crate::spread::AimMarkView> {
        if !self.alive || self.cinematic {
            return None;
        }
        let c = self.held()?;
        let row = self.spread_row(c.weapon);
        let st = self.spreads.get(&c.weapon).cloned().unwrap_or_default();
        st.aim_mark(&row, &self.motion, now as f64)
    }

    /// The mod models to draw on the held weapon's first-person model ([`crate::attach`]).
    pub fn view_attachments(&self) -> Vec<crate::attach::WeaponAttach> {
        let (Some(c), Some(t)) = (self.held(), self.cfg.table.as_ref()) else {
            return Vec::new();
        };
        t.attach.view_attachments(c.weapon, self.mods_of(c.weapon))
    }

    /// The held weapon's row hides every aim cross (`DisableCrosshair` 1: the snipers): neither the aim mark nor the
    /// static cross is drawn (CLI-616).
    // DISASM: cshell+0x85520..0x855ad (the aim element: row +0x3dc set -> only the row's DotSightName picture, no
    // marks; cshell+0x17df07 reads DisableCrosshair into +0x3dc)
    pub fn aim_cross_hidden(&self) -> bool {
        self.alive
            && self
                .held()
                .is_some_and(|c| !self.spread_row(c.weapon).aim_mark)
    }

    /// The held weapon row's aim picture (`DotSightName`, the knife's red X), drawn by the aim element at the
    /// screen centre whether or not the row disables the aim mark (CLI-630).
    // DISASM: cshell+0x85520..0x855ad (row +0x3dc set: the +0xc8 picture only), cshell+0x85682..0x856ed (otherwise
    // after the marks), both only when +0xc8 is a non-empty text
    pub fn aim_picture(&self) -> Option<String> {
        if !self.alive {
            return None;
        }
        let c = self.held()?;
        self.row(c.weapon)?.dot_sight.clone()
    }

    fn row(&self, weapon: u16) -> Option<&WeaponRow> {
        self.cfg.table.as_ref().and_then(|t| t.get(weapon))
    }

    /// The first-person clips of a weapon (read once from the ROM).
    pub fn clips_of(&mut self, weapon: u16) -> Option<&WeaponClips> {
        self.clip_source(weapon).map(|c| &c.1)
    }

    /// The first-person model the clips were read from (see [`Arsenal::clips_of`]).
    pub fn clip_model(&mut self, weapon: u16) -> Option<String> {
        self.clip_source(weapon).map(|c| c.0.clone())
    }

    /// The clips of a weapon and the model they came from: the row's first-person model for the first hand
    /// identifier, else the next identifier's model that the ROM holds.
    // DC-DEFAULT: CLI-199 (proposed) a row whose first identifier's model is missing from the ROM takes its clips (and
    // its drawn first-person model) from the next identifier's model that loads. OBSERVED: the ROM melee row 83 names a
    // first-identifier model that no archive holds (its clips fell back to the 105 ms gun timing: the user's
    // "super fast knife sounds", capture 20261025_205716_rustclient); the identifiers' models of one row borrow the same
    // child clip model
    fn clip_source(&mut self, weapon: u16) -> Option<&(String, WeaponClips)> {
        if !self.clips.contains_key(&weapon) {
            let paths: Vec<String> = self
                .row(weapon)
                .map(|r| {
                    let mut v: Vec<String> = r.pv_model.iter().cloned().collect();
                    v.extend(
                        r.pv_models
                            .iter()
                            .filter(|m| !v.iter().any(|x| x.eq_ignore_ascii_case(m)))
                            .cloned()
                            .collect::<Vec<_>>(),
                    );
                    v
                })
                .unwrap_or_default();
            let c = self.vfs.as_ref().and_then(|v| {
                paths
                    .iter()
                    .find_map(|p| WeaponClips::load(v, p).ok().map(|c| (p.clone(), c)))
            });
            self.clips.insert(weapon, c);
        }
        self.clips.get(&weapon).and_then(|c| c.as_ref())
    }

    fn clip_len(
        &mut self,
        weapon: u16,
        which: fn(&WeaponClips) -> Option<&ClipFact>,
        dflt: u32,
    ) -> u32 {
        self.clips_of(weapon)
            .and_then(which)
            .map_or(dflt, |c| c.len_ms.max(1))
    }

    fn clip_marks(
        &mut self,
        weapon: u16,
        which: fn(&WeaponClips) -> Option<&ClipFact>,
        dflt: &[(u32, Mark)],
    ) -> Vec<(u32, Mark)> {
        match self.clips_of(weapon).and_then(which) {
            Some(c) => c.marks.clone(),
            None => dflt.to_vec(),
        }
    }

    fn throwable(&mut self, weapon: u16) -> bool {
        let kind = self.row(weapon).and_then(|r| r.client_kind);
        match self.clips_of(weapon) {
            Some(c) if c.fire.is_some() => c.throwable(),
            _ => kind == Some(CLIENT_KIND_THROWN),
        }
    }

    /// Whether the held weapon takes the alternate fire: a melee row (`Guntype` 0) whose first-person clips hold an
    /// alternate-fire clip.
    // DISASM: cshell+0x1f8b40 (the weapon update gets fire with mode 1 when the control word has the zoom bit 0x80000
    // without the fire bit 0x400 and the row's `Guntype` (+0x418) is 0, 0xd or 0xe; the fire bit gives mode 0)
    // DC-DEFAULT: CLI-1350 (proposed) only `Guntype` 0 (melee) rows take it; 0xd (double barrel) and 0xe (snowballs)
    // keep the zoom key (their alternate rounds use is not read)
    fn alt_capable(&mut self) -> bool {
        let Some(w) = self.held().map(|c| c.weapon) else {
            return false;
        };
        if self.row(w).and_then(|r| r.gun_type) != Some(GUN_TYPE_MELEE) {
            return false;
        }
        self.clips_of(w).is_some_and(|c| c.alt_clip.is_some())
    }

    pub fn single_shot(&self, weapon: u16) -> bool {
        // DC-DEFAULT: CLI-30 (proposed) `RapidFireCount` >= 1 = one shot per press (1 for the pistol 164, 0 for the
        // automatic rifle 145 that fired bursts in tdm1; the reading is PLAUSIBLE)
        self.row(weapon)
            .and_then(|r| r.single_shot)
            .is_some_and(|n| n >= 1)
    }

    /// Server 0x67: a spawn (1 after anything else) starts the select step; any other state stops the weapon.
    pub fn on_life(&mut self, now: u32, state: u8) {
        let alive = state == 1;
        if alive != self.alive {
            self.reset_zoom();
        }
        if alive && !self.alive {
            self.spawned_at = Some(now);
            self.need_select = true;
            self.phase = Phase::Idle;
            self.spreads.clear();
            self.recoil.reset();
            self.camfx.clear();
            self.release_wanted = false;
        }
        if !alive {
            self.camfx.clear();
            self.phase = Phase::Idle;
            self.trigger = false;
            self.trigger_latch_until = None;
        }
        self.alive = alive;
    }

    /// Server 0xb2 / 0xb3. The total `ammo` loads one clip (the weapon row's size) and keeps the rest as stock; 0xb3
    /// names the drawn weapon and its clip.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0xb3: status +0x08 := clip, match_ui switches to it)
    pub fn on_grant(&mut self, g: &WeaponGrant) {
        let row = self.row(g.weapon).cloned().unwrap_or_default();
        let total = if g.ammo.is_finite() && g.ammo > 0.0 {
            g.ammo as u32
        } else {
            0
        };
        // the grant's parts first: a magazine mod makes the clip bigger
        self.file_parts(g.weapon, g.parts);
        let row_clip = row.clip.map(|c| self.modded_clip(g.weapon, c));
        let (clip, stock) = match (g.clip, row_clip) {
            (Some(c), _) => (c, total),
            (None, Some(size)) => (size.min(total), total - size.min(total)),
            (None, None) => (total, 0),
        };
        let replaced = self
            .slots
            .get(&g.slot)
            .is_some_and(|c| c.weapon != g.weapon);
        self.class_pos.insert(g.slot, g.class_pos);
        self.slots.insert(
            g.slot,
            Carried {
                slot: g.slot,
                weapon: g.weapon,
                ammo_id: g.ammo_kind,
                clip,
                stock,
                clip_size: row_clip.or(g.clip),
                source: "grant",
            },
        );
        if g.equip {
            // the server draws this one
            self.want(g.slot);
        } else if replaced && self.held == Some(g.slot) {
            self.need_select = true;
        }
    }

    /// Server 0x7f `GS_WEAPON_SWITCH`: the command names a weapon; with `auto` the client draws it (the lowest slot
    /// holding it) through its own select, the same 0x76 `[3][2]` + 0x7f pair a key press sends. A negative `ammo`
    /// keeps the slot's count. An unknown command or a weapon not carried does nothing.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0x7f: `auto` != 0 switches to the weapon, `ammo` >= 0 is its count)
    // OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §2 (the select pair after the server's draw, lowest granted slot)
    pub fn switch_command(&mut self, command: u8, auto: bool, ammo: f32) {
        let Some(table) = self.cfg.table.as_ref() else {
            return;
        };
        let Some(named) = table.weapon_of_command(command) else {
            return;
        };
        // VAR-1: a variant answers to its base gun's command
        let Some((slot, weapon)) = self
            .slots
            .iter()
            .find(|(_, c)| table.base_of(c.weapon) == named)
            .map(|(s, c)| (*s, c.weapon))
        else {
            return;
        };
        if !auto {
            return;
        }
        if ammo >= 0.0 {
            self.on_ammo(slot, weapon, ammo);
        }
        self.want(slot);
    }

    fn want(&mut self, slot: u8) {
        self.need_select = false;
        self.pending_select = Some(slot);
    }

    /// Server 0xb4.
    pub fn on_remove(&mut self, slot: u8) {
        self.slots.remove(&slot);
        self.class_pos.remove(&slot);
        if self.held == Some(slot) {
            self.held = None;
            self.phase = Phase::Idle;
            self.need_select = true;
        }
    }

    /// Server 0x66 kind 1: the slot's ammunition count.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (kind 1: the ammo count of `slot` := `ammo`)
    pub fn on_ammo(&mut self, slot: u8, weapon: u16, ammo: f32) {
        if let Some(c) = self.slots.get_mut(&slot) {
            if weapon != 0xFFFF && c.weapon != weapon {
                return;
            }
            let total = if ammo.is_finite() && ammo > 0.0 {
                ammo as u32
            } else {
                0
            };
            c.clip = c.clip.min(total);
            c.stock = total - c.clip;
        }
    }

    /// The lobby's slots as an inventory (when no grant came).
    fn lobby_inventory(&mut self) {
        let slots = self.cfg.lobby_slots.clone();
        for (i, item) in slots.iter().enumerate().take(9) {
            if !(0..WEAPON_ITEM_LIMIT).contains(item) {
                continue;
            }
            let w = *item as u16;
            if self.barred(i as u8, w) {
                continue;
            }
            let Some(r) = self.row(w).cloned() else {
                continue;
            };
            let size = r.clip.unwrap_or(1).max(1);
            let total = r.max_amount.unwrap_or(size);
            self.slots.insert(
                i as u8,
                Carried {
                    slot: i as u8,
                    weapon: w,
                    ammo_id: r.ammo.unwrap_or(w),
                    clip: size.min(total),
                    stock: total.saturating_sub(size.min(total)),
                    clip_size: r.clip,
                    source: "lobby",
                },
            );
        }
    }

    /// Whether the room's weapon limit keeps `weapon` in loadout slot `slot` out of the client's own lobby-loadout
    /// fallback (the weapons assumed when no server grant arrives). A server grant is always carried and drawable:
    /// the server decides what a mode or the limit hands out.
    // DC-DEFAULT: CLI-1605 (proposed, USER REPORT 2026-10-31 "pistols are available to use in a sniper-only match")
    // the 2009 client checks the limit only before Ready / Start (cshell+0x160f70 -> cshell+0x226dd0, its only caller)
    // and carries what the server grants; ours filters only its own fallback with the server's grant rule (DSV-476,
    // `dc_bute::arms::match_grants`)
    pub fn barred(&self, slot: u8, weapon: u16) -> bool {
        if self.cfg.arms_limit == 0 || !LIMITED_SLOTS.contains(&slot) {
            return false;
        }
        let Some(&t) = self.cfg.weapon_types.get(&weapon) else {
            return false;
        };
        let backpack = slot >= BACKPACK_FIRST_SLOT;
        !dc_bute::arms::match_grants(self.cfg.arms_limit, t, backpack)
    }

    /// The order class of a weapon (2009 numbering; 0 without a table or outside the order).
    pub fn order_class(&self, weapon: u16) -> u8 {
        self.cfg.table.as_ref().map_or(0, |t| t.order_class(weapon))
    }

    /// The row's `InfiniteAmmo` (false without a table).
    pub fn infinite_ammo(&self, weapon: u16) -> bool {
        self.row(weapon).is_some_and(|r| r.infinite_ammo)
    }

    /// The weapon is loaded: `InfiniteAmmo`, or a round in its slot (clip or stock).
    // EVIDENCE: cshell+0x232800 (has-ammo test: row +0x23c, else the slot's count > 0)
    fn loaded(&self, c: &Carried) -> bool {
        let infinite = self.row(c.weapon).is_some_and(|r| r.infinite_ammo);
        infinite || c.total() > 0
    }

    /// A number key or the wheel may draw this slot: a weapon of class 1 or 2 always, any other only while loaded.
    /// Without an order class (no ROM table, a weapon outside the order) the test is kept for throwables only.
    // EVIDENCE: cshell+0x22e704..0x22e71f, cshell+0x22e790 (class 1 / 2, else the has-ammo test cshell+0x232800)
    // DC-DEFAULT: CLI-400 (proposed) no order class: only a throwable needs a round to be drawn
    pub fn may_draw(&mut self, slot: u8) -> bool {
        let Some(c) = self.slots.get(&slot).copied() else {
            return false;
        };
        let class = self.order_class(c.weapon);
        if DRAW_EMPTY_CLASSES.contains(&class) || self.loaded(&c) {
            return true;
        }
        class == 0 && !self.throwable(c.weapon)
    }

    /// The weapon leaves the hand when its last round is used: the row's `HideWhenEmpty`; a row without the key
    /// (synthetic tables) counts a throwable as one.
    // EVIDENCE: cshell+0x237614..0x2376fb (row +0x240 = `HideWhenEmpty`, read by cshell+0x17ce07)
    // DC-DEFAULT: CLI-401 (proposed) no `HideWhenEmpty` key: a throwable hides when empty
    fn hides_when_empty(&mut self, weapon: u16) -> bool {
        match self.row(weapon).and_then(|r| r.hide_when_empty) {
            Some(h) => h,
            None => self.throwable(weapon),
        }
    }

    /// The empty switch after a round use left a `HideWhenEmpty` weapon with no round in its slot: in the same
    /// update the client draws the slot it last drew of class 1, else of class 2, else of class 3 (the select pair
    /// `76 03 02` + `7f` goes out in the block of the round's 0x82), without waiting for the server's count. Returns
    /// whether it switched.
    // EVIDENCE: cshell+0x233d90 (round use: clip - 1, the fired flag, slot count := count - 1, all local),
    // cshell+0x237614..0x2376fb (the same weapon update returns 13 when the slot's count is 0), cshell+0x22f1c0 ->
    // cshell+0x22ec00 (13 -> draw the class-1 / 2 / 3 entry of the last-drawn table)
    // OBSERVED: capture 20261025_125432_rust6/analysis (the 2009 client, 2-pack 143: six last
    // throws, each 0x82 with `7f` in the same moment, 0-16 ms before the server's 0.0; five to slot 0, one to slot 4
    // = the primary drawn last, never the melee slot drawn just before the throwable)
    // DC-DEFAULT: CLI-402 (proposed) no class entry (no ROM order): the lowest other slot that may be drawn;
    // CLI-404 (proposed) the 2009 first step (another ammunition kind of the same weapon) is not modelled
    fn empty_switch(&mut self, now: u32, out: &mut Vec<Outgoing>) -> bool {
        let held = self.held;
        let remembered = EMPTY_SWITCH_CLASSES
            .iter()
            .filter_map(|k| self.class_slots.get(k).copied())
            .find(|s| Some(*s) != held && self.slots.contains_key(s));
        let target = match remembered {
            Some(s) => Some(s),
            None => {
                let keys: Vec<u8> = self.slots.keys().copied().collect();
                keys.into_iter()
                    .find(|s| Some(*s) != held && self.may_draw(*s))
            }
        };
        match target {
            Some(s) => {
                self.select_now(now, s, out);
                true
            }
            None => false,
        }
    }

    /// The next slot from `from` in the wheel direction that may be drawn ([`Arsenal::may_draw`]).
    fn next_drawable(&mut self, from: Option<u8>, step: i32) -> Option<u8> {
        let mut at = from;
        for _ in 0..self.slots.len() {
            let s = self.next_slot(at, step)?;
            if Some(s) == from {
                return None;
            }
            if self.may_draw(s) {
                return Some(s);
            }
            at = Some(s);
        }
        None
    }

    fn next_slot(&self, from: Option<u8>, step: i32) -> Option<u8> {
        let keys: Vec<u8> = self.slots.keys().copied().collect();
        if keys.is_empty() {
            return None;
        }
        let pos = from.and_then(|s| keys.iter().position(|k| *k == s));
        let n = keys.len() as i32;
        let i = match pos {
            Some(p) => (p as i32 + step).rem_euclid(n),
            None => 0,
        };
        Some(keys[i as usize])
    }

    fn rand_u64(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    /// `rand()` in 2..=255 like the client's seed byte.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (`seed` = cshell+0x19eb40(2, 255))
    fn seed_byte(&mut self) -> u8 {
        2 + (self.rand_u64() % 254) as u8
    }

    /// True while the 0x64 should carry control 0x400 and `pu_flags` 0x2000.
    pub fn trigger_flags(&self, now: u32) -> bool {
        self.alive
            && self.held.is_some()
            && (self.trigger || self.trigger_latch_until.is_some_and(|t| now < t))
    }

    fn start(&mut self, now: u32, p: Phase) {
        self.phase = p;
        self.phase_at = now;
        self.marks_done = None;
    }

    /// Keys of the playing clip that came due since the last step: `(time in clip, mark)`.
    fn due_marks(&mut self, now: u32, marks: &[(u32, Mark)]) -> Vec<Mark> {
        let t = now.saturating_sub(self.phase_at);
        let from = self.marks_done;
        let v: Vec<Mark> = marks
            .iter()
            .filter(|(at, _)| *at <= t && from.is_none_or(|d| *at > d))
            .map(|m| m.1)
            .collect();
        self.marks_done = Some(t);
        v
    }

    fn sound(&mut self, out: &mut Vec<Outgoing>, n: u8, aim: &Aim) {
        let Some(w) = self.held().map(|c| c.weapon) else {
            return;
        };
        self.stats.sounds += 1;
        out.push(Outgoing {
            shell: up::WEAPON_SOUND,
            body: weapon_sound_body(n, w, self.own_id, self.muzzle_point(w, aim)),
        });
    }

    fn select_now(&mut self, now: u32, slot: u8, out: &mut Vec<Outgoing>) {
        let Some(c) = self.slots.get(&slot).copied() else {
            return;
        };
        self.reset_zoom();
        self.held = Some(slot);
        self.stats.selects += 1;
        self.reload_event_at = None;
        self.release_wanted = false;
        // DC-DEFAULT: CLI-403 (proposed) the table is kept over lives (the spawn's draw rewrites the class-1 entry)
        let class = self.order_class(c.weapon);
        self.class_slots.insert(class, slot);
        // OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §2 (`76 03 02` then `7f [slot][weapon][aux = ammo id]`)
        out.push(Outgoing {
            shell: up::WEAPON_EVENT,
            body: weapon_event_body(WEAPON_EVENT_SELECT),
        });
        out.push(Outgoing {
            shell: up::WEAPON_CHANGE,
            body: weapon_change_body(&WeaponChange {
                slot,
                weapon_id: c.weapon,
                aux: c.ammo_id,
            }),
        });
        self.start(now, Phase::Select);
    }

    fn begin_reload(&mut self, now: u32, out: &mut Vec<Outgoing>) -> bool {
        let Some(c) = self.held().copied() else {
            return false;
        };
        let size = c.clip_size.unwrap_or(c.clip);
        // SPEC: GAME_MESSAGES_2009_C2S.md §4.2 (not sent when the clip is already full)
        if c.stock == 0 || c.clip >= size {
            return false;
        }
        // zoomed: the reload waits for the zoom-out (asked again each step while a zoom change runs)
        // DISASM: cshell+0x237180 -> cshell+0x1f6520 (see `reload_while_zoomed`)
        if self.scope.active() {
            if self.scope.changing() {
                self.reload_wanted = true;
            } else {
                self.reload_while_zoomed(now);
            }
            return true;
        }
        self.stats.reloads += 1;
        out.push(Outgoing {
            shell: up::RELOAD,
            body: reload_body(c.ammo_id),
        });
        self.reload_event_at = Some((now + RELOAD_EVENT_DELAY_MS, size.min(u8::MAX as u32) as u8));
        self.start(now, Phase::Reload);
        true
    }

    fn finish_reload(&mut self) {
        if let Some(s) = self.held {
            if let Some(c) = self.slots.get_mut(&s) {
                let size = c.clip_size.unwrap_or(c.clip);
                let take = size.saturating_sub(c.clip).min(c.stock);
                c.clip += take;
                c.stock -= take;
            }
        }
    }

    /// One shot of the held weapon: one 0x82 per pellet.
    fn shoot(
        &mut self,
        now: u32,
        aim: &Aim,
        targets: &[Target],
        world: Option<&dyn Collider>,
        out: &mut Vec<Outgoing>,
    ) {
        let Some(c) = self.held().copied() else {
            return;
        };
        // the view's drain up to the shot belongs to the shot's aim (the 2009 camera update drained before its fire
        // check); with it the shot's angles depend on the session clock only, not on the frame time
        // EVIDENCE: cshell+0x1f8b40 (drain cshell+0x1f3070, then the fire check, then the kick)
        let aim = &self.drained_aim(c.weapon, now, aim);
        let row = self.row(c.weapon).cloned().unwrap_or_default();
        let mods = self.mod_slots(c.weapon);
        // DISASM: cshell+0x23bca0 (pellets, range and scale of the fire routine); SPEC: addendum AC-3.2, AC-3.4
        let pellets = crate::cone::pellets(row.pellets, &mods);
        let range = crate::cone::range(row.range, &mods);
        // the steps of the ticks that passed, the spread before the shot, the fire step and the recoil kick
        // EVIDENCE: cshell+0x23bd15 (the fire routine reads slot 0x44 before the pellet loop), cshell+0x237180 (the
        // 2009 spread steps ran after the fire routine in the same weapon update; ours run the elapsed ticks first
        // so the byte does not depend on the frame rate, spread.rs CLI-61)
        let srow = self.spread_row(c.weapon);
        let rates = self.spread_rates();
        let motion = self.motion;
        let shot = self.spreads.entry(c.weapon).or_default().shot(
            &srow,
            &rates,
            now as f64,
            &motion,
            crate::spread::clock::TICK_MS,
        );
        self.shot_this_step = true;
        let kmods = self.kick_mods(c.weapon);
        // DC-DEFAULT: CLI-1627 (proposed, project decision) a no-recoil room: no view kick
        let kick = if self.cfg.no_recoil {
            crate::recoil::ViewKick::default()
        } else {
            self.recoil.kick_with(
                &srow.recoil,
                &kmods,
                now as f64,
                crate::spread::clock::TICK_MS,
                self.triple,
            )
        };
        self.view_pitch += kick.pitch;
        self.view_yaw += kick.yaw;
        let scale = crate::cone::shot_scale(&row.cone, &mods, crate::cone::move_state(&motion));
        let q = dc_botclient::shooter::aim_quat(aim.yaw, aim.pitch);
        // the pellets and the muzzle follow the camera: the view plus the jar's offsets (the 0x82 `aim` stays the
        // view's rotation)
        // DISASM: cshell+0x1fafba..0x1fb266 (camera rotation = euler(view + offsets)); EVIDENCE: cshell+0x234d50 (the
        // shot rotation is the camera object's), cshell+0x2348d0 (muzzle point by the camera rotation)
        let cam = self.camfx.angles(now as f64);
        let cam_aim = Aim {
            yaw: aim.yaw + cam.yaw,
            pitch: aim.pitch + cam.pitch,
            ..*aim
        };
        let q_cam = crate::camfx::camera_quat(cam_aim.yaw, cam_aim.pitch, cam.roll);
        let wpos = self.muzzle_point(c.weapon, &cam_aim);
        let before = shot.value;
        let cone = crate::cone::ConeInputs {
            aim: q_cam,
            spread: before,
            scale,
            zoom: self.zoom_active(),
            triple: self.triple,
        };
        let table = self.shot_table();
        // a launcher's round (and any other untraced `Type`) is sent without a trace: no hit claim, the server flies
        // it
        // EVIDENCE: cshell+0x23c041..0x23c064, table cshell+0x23c298; SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-3.4
        // DC-DEFAULT: CLI-1590 (proposed; before, every round was traced like a bullet)
        let traced = row.ammo_type.is_none_or(crate::fx::traced_ammo);
        for i in 0..pellets {
            // every pellet starts from the forward axis (cone, then trace, then send; `stat` after the draws)
            let dir = crate::cone::own_pellet(
                &row.cone,
                &cone,
                &mut self.shot_counter,
                &table,
                self.cfg.no_recoil,
            );
            // DC-DEFAULT: CLI-300 (proposed) the 2009 node-sphere test ([`crate::hitnodes::trace`])
            let hit = if traced {
                crate::hitnodes::trace(
                    aim.origin,
                    dir,
                    range,
                    targets,
                    &self.hit_nodes,
                    world,
                    &mut self.stats.nodes,
                )
            } else {
                RayHit::Miss
            };
            if matches!(hit, RayHit::Character { .. }) {
                self.stats.hits += 1;
            }
            self.shot_fx.push(ShotFx {
                weapon: c.weapon,
                ammo: c.ammo_id,
                origin: aim.origin,
                muzzle: wpos,
                dir,
                range,
                hit,
                first: i == 0,
                time_ms: now,
                impact: None,
                flash_mod: self.mods_of(c.weapon)[crate::zoom::MUZZLE_SOCKET],
            });
            self.stats.fire_msgs += 1;
            let seed = self.seed_byte();
            out.push(Outgoing {
                shell: up::FIRE,
                body: fire_message(&ShotInputs {
                    weapon_id: c.weapon,
                    ammo_id: c.ammo_id,
                    weapon_pos: wpos,
                    origin: aim.origin,
                    aim: q,
                    yaw: aim.yaw,
                    dir,
                    seed,
                    spread: before,
                    shot_scale: scale,
                    zoom: self.zoom_active(),
                    triple: self.triple,
                    alt: self.alt,
                    time_ms: now,
                    hit,
                    last: i + 1 == pellets,
                    stat: self.shot_counter.value,
                }),
            });
        }
        self.stats.shots += 1;
        self.trigger_latch_until = Some(now + FIRE_BITS_LATCH_MS);
        if let Some(s) = self.held {
            if let Some(cc) = self.slots.get_mut(&s) {
                if cc.clip_size.is_some() {
                    cc.clip = cc.clip.saturating_sub(1);
                }
            }
        }
    }

    /// The fire key of the playing fire clip: one shot while the clip holds a round; the shot that empties the clip
    /// starts the reload at once (its 0x83 after the 0x82s, in the same block).
    // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.7 (the automatic reloads came in the block of the 30th 0x82, after it)
    fn fire_key(
        &mut self,
        now: u32,
        aim: &Aim,
        targets: &[Target],
        world: Option<&dyn Collider>,
        out: &mut Vec<Outgoing>,
    ) {
        let has = self
            .held()
            .is_some_and(|c| c.clip > 0 || c.clip_size.is_none());
        if !has || self.phase != Phase::Fire {
            return;
        }
        self.shoot(now, aim, targets, world, out);
        // a zoomed shot of a bolt-action weapon arms its zoom-out
        // DISASM: cshell+0x1f8b40 ("fire now" while +0xf0 > 0 or +0xf4, row +0x200 -> +0x1b3 := 1, +0x1b4 := now)
        if self.scope.active()
            && self
                .row(self.held().map_or(0, |c| c.weapon))
                .is_some_and(|r| r.reload_after_fire)
        {
            self.scope.arm_bolt_unzoom(now, true);
        }
        let empty = self
            .held()
            .is_some_and(|c| c.clip_size.is_some() && c.clip == 0);
        if empty && !self.begin_reload(now, out) {
            self.after_round_use(now, out);
        }
    }

    /// After a round use (a shot or a throw's release): the empty switch for a `HideWhenEmpty` weapon left with no
    /// round, else the reload of an empty clip from the stock. Returns whether the weapon state changed (a switch
    /// or a reload started).
    // EVIDENCE: cshell+0x237614..0x2376fb (the empty test runs after the round use), cshell+0x233d90
    fn after_round_use(&mut self, now: u32, out: &mut Vec<Outgoing>) -> bool {
        let Some(c) = self.held().copied() else {
            return false;
        };
        if !self.loaded(&c) && self.hides_when_empty(c.weapon) {
            return self.empty_switch(now, out);
        }
        if c.clip_size.is_some() && c.clip == 0 && c.stock > 0 && self.phase != Phase::Reload {
            self.release_wanted = false;
            return self.begin_reload(now, out);
        }
        false
    }

    /// The release of a throwable: one 0x82 without an object block, along the view.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (the release is one UP_FIRE: origin = the eye point, dir = the throw
    // direction, no extra block, no hit, last 1)
    fn throw_release(&mut self, now: u32, aim: &Aim, out: &mut Vec<Outgoing>) {
        let Some(c) = self.held().copied() else {
            return;
        };
        let row = self.row(c.weapon).cloned().unwrap_or_default();
        let mods = self.mod_slots(c.weapon);
        self.stats.fire_msgs += 1;
        self.stats.throws += 1;
        let seed = self.seed_byte();
        // the release goes through the same fire routine: one cone draw pair, `stat` after it
        // DC-DEFAULT: CLI-124 (proposed) the release uses the cone like a shot (the throw path is not disassembled)
        // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.6 (+2 per 0x82, grenades included); SPEC: addendum AC-3.4
        let q = dc_botclient::shooter::aim_quat(aim.yaw, aim.pitch);
        let scale =
            crate::cone::shot_scale(&row.cone, &mods, crate::cone::move_state(&self.motion));
        let spread = self.spreads.get(&c.weapon).map_or(0.0, |s| s.value());
        let cone = crate::cone::ConeInputs {
            aim: q,
            spread,
            scale,
            zoom: self.zoom_active(),
            triple: self.triple,
        };
        let table = self.shot_table();
        let dir = crate::cone::pellet(&row.cone, &cone, &mut self.shot_counter, &table);
        out.push(Outgoing {
            shell: up::FIRE,
            body: fire_message(&ShotInputs {
                weapon_id: c.weapon,
                ammo_id: c.ammo_id,
                weapon_pos: self.muzzle_point(c.weapon, aim),
                origin: aim.origin,
                aim: q,
                yaw: aim.yaw,
                dir,
                seed,
                spread,
                shot_scale: scale,
                zoom: self.zoom_active(),
                triple: self.triple,
                alt: false,
                time_ms: now,
                hit: RayHit::Miss,
                last: true,
                stat: self.shot_counter.value,
            }),
        });
        self.trigger_latch_until = Some(now + FIRE_BITS_LATCH_MS);
        // the round use: one round off the slot's count at once (the server's count comes later); the empty clip
        // is refilled by a reload, never silently
        // EVIDENCE: cshell+0x233d90 (clip - 1 and slot count := count - 1 in the fire routine)
        if let Some(s) = self.held {
            if let Some(cc) = self.slots.get_mut(&s) {
                if cc.clip > 0 {
                    cc.clip -= 1;
                } else {
                    cc.stock = cc.stock.saturating_sub(1);
                }
            }
        }
    }

    /// Runs the weapon at `now` (ms of the session clock, the 0x82 `time_ms`) with this frame's commands; returns
    /// the messages to send, in order. The held weapon's spread steps and recoil recovery run after the weapon
    /// (in a shot frame the fire step already ran with the shot).
    pub fn step(
        &mut self,
        now: u32,
        cmd: &WeaponCommands,
        aim: &Aim,
        targets: &[Target],
        world: Option<&dyn Collider>,
    ) -> Vec<Outgoing> {
        self.shot_this_step = false;
        self.motion.zoomed = self.zoom_active();
        self.sway_frame(now);
        // CLI-982: a live world camera holds the weapons
        let held = WeaponCommands::default();
        let cmd = if self.cinematic { &held } else { cmd };
        self.inspect_frame(now, cmd, aim);
        let out = self.step_weapon(now, cmd, aim, targets, world);
        self.motion.zoomed = self.zoom_active();
        self.spread_frame(now);
        self.sprint_frame(now, aim);
        out
    }

    /// The inspect of the frame, before the weapon runs: anything the player does with the weapon ends it, and that
    /// action then runs as usual (DC-DEFAULT: CLI-1845).
    fn inspect_frame(&mut self, now: u32, cmd: &WeaponCommands, aim: &Aim) {
        let held = self.held().map(|c| c.weapon).filter(|_| self.alive);
        let anim = held.and_then(|w| self.inspect_anim(w));
        let busy = cmd.fire
            || cmd.fire_press
            || cmd.reload
            || cmd.select.is_some()
            || cmd.wheel != 0
            || cmd.class_key.is_some()
            || cmd.zoom
            || aim.control & dc_sim::player::bits::SPRINT != 0
            || self.phase != Phase::Idle
            || self.scope.active()
            || self.cinematic;
        self.inspector.step(
            now,
            crate::inspect::InspectFrame {
                press: cmd.inspect,
                held,
                busy,
            },
            anim.as_deref(),
        );
    }

    /// What `weapon`'s row says of its inspect; a variant has its base gun's.
    fn inspect_anim(&self, weapon: u16) -> Option<std::sync::Arc<crate::inspect::InspectAnim>> {
        self.row(weapon)
            .and_then(|r| r.inspect.clone())
            .or_else(|| {
                self.row(crate::camo::base_weapon(weapon))
                    .and_then(|r| r.inspect.clone())
            })
    }

    /// The first-person model's inspect pose now (`None`: at rest).
    pub fn inspect_pose(&self, now: u32) -> Option<crate::inspect::ViewPose> {
        let anim = self.held().and_then(|c| self.inspect_anim(c.weapon));
        self.inspector.pose(now, anim.as_deref())
    }

    /// The inspect's sounds that came due: (weapon sound number, weapon). They are not sent.
    pub fn take_inspect_sounds(&mut self) -> Vec<(u8, u16)> {
        self.inspector.take_sounds()
    }

    /// The scope sway of the frame, before the weapon runs: while the scope is up the view (and so the shot) turns
    /// by the held weapon's and its mods' sway ([`crate::zoom::CameraSway`]).
    // EVIDENCE: cshell+0x1f4f60 (controller frame: +0xf0 > 0 or +0xf4 != 0 -> vtable +0xe4 = cshell+0x1f2b70)
    fn sway_frame(&mut self, now: u32) {
        let held = self.held().map(|c| c.weapon).filter(|_| self.alive);
        let Some(w) = held.filter(|_| self.scope.active()) else {
            self.sway_ms = None;
            return;
        };
        let Some(t0) = self.sway_ms.replace(now) else {
            return;
        };
        let (dyaw, dpitch) = self
            .sway_of(w)
            .delta(f64::from(t0) / 1000.0, f64::from(now) / 1000.0);
        self.view_yaw += dyaw;
        self.view_pitch += dpitch;
    }

    /// The scope sway of `weapon`: its row's plus the rows of the mods in its three sockets.
    // EVIDENCE: cshell+0x1f2b70 (slots 0..2 through the weapon's +0x11c, then the row)
    pub fn sway_of(&self, weapon: u16) -> crate::zoom::CameraSway {
        let row = self.row(weapon).map(|r| r.sway).unwrap_or_default();
        (0..3)
            .filter_map(|s| self.mod_in(weapon, s))
            .fold(row, |acc, m| acc.plus(m.sway))
    }

    /// The sprint clips of the first-person model after the weapon step: they play while the player sprints and the
    /// weapon is idle (a select, reload, fire or throw clip finishes first).
    // EVIDENCE: cshell+0x23ac50 (the weapon update asks the movement object's sprint flag every frame; reload / select
    // and a playing fire clip finish first); SPEC: MOVEMENT_2009.md §5.1 (SPRINT 0x400000 in the control word)
    fn sprint_frame(&mut self, now: u32, aim: &Aim) {
        let held = self.held().map(|c| c.weapon).filter(|_| self.alive);
        let Some(w) = held else {
            self.sprint_anim.reset();
            return;
        };
        let sprinting = aim.control & dc_sim::player::bits::SPRINT != 0;
        let idle = self.phase == Phase::Idle;
        let lens = self.sprint_lens(w);
        self.sprint_anim.step(now, sprinting, idle, &lens);
    }

    /// The lengths of a weapon's sprint clips (none without first-person clips).
    pub fn sprint_lens(&mut self, weapon: u16) -> crate::viewanim::SprintLens {
        match self.clips_of(weapon) {
            Some(c) => crate::viewanim::SprintLens {
                enter: c.sprint_enter.as_ref().map(|f| f.len_ms),
                run: c.sprint_loop.as_ref().map(|f| f.len_ms),
                exit: c.sprint_exit.as_ref().map(|f| f.len_ms),
            },
            None => crate::viewanim::SprintLens::default(),
        }
    }

    /// The spread steps of a frame without a shot, and the recoil recovery.
    // EVIDENCE: cshell+0x237180 (the step loop runs in every weapon update), cshell+0x1f3070 (recovery per frame)
    fn spread_frame(&mut self, now: u32) {
        let Some(w) = self.held().map(|c| c.weapon).filter(|_| self.alive) else {
            return;
        };
        let row = self.spread_row(w);
        let rates = self.spread_rates();
        let motion = self.motion;
        let shot = self.shot_this_step;
        let st = self.spreads.entry(w).or_default();
        if !shot {
            st.frame(
                &row,
                &rates,
                now as f64,
                &motion,
                crate::spread::clock::TICK_MS,
            );
        }
        let back = self.recoil.drain(
            &row.recoil,
            now as f64,
            crate::spread::clock::TICK_MS,
            self.triple,
        );
        self.view_pitch += back.pitch;
        self.view_yaw += back.yaw;
    }

    /// CLI-1642: a Possession move starts or ends.
    pub fn set_glide(&mut self, on: bool) {
        self.glide = on;
    }

    fn step_weapon(
        &mut self,
        now: u32,
        cmd: &WeaponCommands,
        aim: &Aim,
        targets: &[Target],
        world: Option<&dyn Collider>,
    ) -> Vec<Outgoing> {
        let mut out = Vec::new();
        self.last_step = Some(now);
        self.last_aim = Some(*aim);
        if !self.alive {
            self.trigger = false;
            self.throw_key = false;
            return out;
        }
        // the fire key never counts while sprinting
        // OBSERVED: CAPTURE_tdm4_KILL_VALIDATION.md K4 (0x400 never together with SPRINT 0x400000)
        let sprinting = aim.control & dc_sim::player::bits::SPRINT != 0;
        // CLI-661: nor while the sprint-exit clip plays (the weapon is still in the sprint state)
        let sprint_out = match self.held().map(|c| c.weapon) {
            Some(w) if !sprinting && self.sprint_anim.active() => {
                let lens = self.sprint_lens(w);
                self.sprint_anim.holds_weapon(now, &lens)
            }
            _ => false,
        };
        if !cmd.fire || cmd.fire_press {
            self.fire_latched = false;
        }
        // CLI-1601: a press since the last frame counts as a press of this frame even when the button is up again
        // (released before the frame) or was let go and pressed again within it; the trigger is then down for
        // this frame, so one shot fires now and none at the release
        // EVIDENCE: cshell+0x23b050 (the press: idle -> firing), cshell+0x23b340 (the release: firing -> idle, no
        // shot), WEAPON_FIRE_2009.md §6
        let gate = !sprinting && !sprint_out && !self.fire_latched && !self.glide;
        let press = cmd.fire_press && gate;
        let fire = (cmd.fire || cmd.fire_press) && gate;
        // CLI-1350: without the fire key, the held zoom key of a melee row with an alternate-fire clip is the
        // alternate fire (the fire key wins)
        let alt = !fire && cmd.zoom && !sprinting && !sprint_out && self.alt_capable();
        let want = fire || alt;
        if (want && !self.trigger) || press {
            self.pressed = true;
        }
        self.trigger = want;
        self.alt_wanted = alt;
        self.throw_key = cmd.fire && !self.glide;
        // the end of a zoom change; a reload asked while zoomed runs at the end of the zoom-out
        if self.scope.frame(now) {
            self.reload_wanted = true;
        }
        if cmd.reload && !self.reload_while_zoomed(now) {
            self.reload_wanted = true;
        }
        self.bolt_unzoom(now);
        self.zoom_step(now, cmd.zoom, sprinting);
        // inventory and the select after a spawn: the slot the server drew (0xb3), else the lowest granted slot
        // OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §2 (at the spawn the client selected the lowest granted slot)
        if self.need_select || self.pending_select.is_some() {
            let waited = self
                .spawned_at
                .is_none_or(|s| now.saturating_sub(s) >= GRANT_WAIT_MS);
            if self.slots.is_empty() && waited {
                self.lobby_inventory();
            }
            let pick = self
                .pending_select
                .take()
                .filter(|s| self.slots.contains_key(s))
                .or_else(|| {
                    self.need_select
                        .then(|| self.slots.keys().next().copied())
                        .flatten()
                });
            match pick {
                Some(s) => {
                    self.need_select = false;
                    self.select_now(now, s, &mut out);
                }
                None if waited => self.need_select = false,
                None => {}
            }
        }
        // a chosen slot (number key or wheel) other than the held one; once the release clip runs the throw is
        // committed, but a wind-up or a hold (pin pulled, key down) is cancelled by the switch: the throwable is
        // kept (nothing is thrown, its count unchanged) and the chosen weapon is drawn
        // EVIDENCE: cshell+0x22e530 (class key) and cshell+0x22e020 (CanChangeToWeapon) test no weapon state;
        // cshell+0x22e930 (ChangeWeapon) calls the old weapon's slot +0x48 with 0 before it draws the new one, so the
        // firing throwable never sees its release (the throw is the firing state's release, WEAPON_FIRE_2009.md §6.1)
        // DC-DEFAULT: CLI-676 (proposed) the old weapon's slot +0x48 read as its deactivation (PLAUSIBLE); the fire
        // key held through the switch counts again only after it went up
        let busy_throw = self.phase == Phase::ThrowRelease;
        let chosen = match (cmd.select, cmd.class_key, cmd.wheel) {
            (Some(s), _, _) => Some(s),
            (None, Some(k), _) => self.class_pick(k),
            (None, None, w) if w != 0 => self.next_drawable(self.held, w.signum()),
            _ => None,
        };
        if let Some(s) = chosen {
            if !busy_throw && self.held != Some(s) && self.may_draw(s) {
                if matches!(self.phase, Phase::ThrowPre | Phase::ThrowHold) {
                    self.stats.throw_cancels += 1;
                    self.pressed = false;
                    self.trigger = false;
                    self.fire_latched = cmd.fire;
                }
                self.select_now(now, s, &mut out);
            }
        }
        let Some(c) = self.held().copied() else {
            return out;
        };
        let w = c.weapon;
        // the delayed reload notice
        if let Some((at, v)) = self.reload_event_at {
            if now >= at {
                self.reload_event_at = None;
                out.push(Outgoing {
                    shell: up::WEAPON_EVENT,
                    body: reloaded_event_body(v),
                });
            }
        }
        let t = now.saturating_sub(self.phase_at);
        match self.phase {
            Phase::Select => {
                let dflt = [(
                    fallback::SELECT_SOUND.0,
                    Mark::Sound(fallback::SELECT_SOUND.1),
                )];
                let marks = self.clip_marks(w, |c| c.select.as_ref(), &dflt);
                for m in self.due_marks(now, &marks) {
                    if let Mark::Sound(n) = m {
                        self.sound(&mut out, n, aim);
                    }
                }
                if t >= self.clip_len(w, |c| c.select.as_ref(), fallback::SELECT_MS) {
                    self.start(now, Phase::Idle);
                }
            }
            Phase::Reload => {
                let dflt: Vec<(u32, Mark)> = fallback::RELOAD_SOUNDS
                    .iter()
                    .map(|(at, n)| (*at, Mark::Sound(*n)))
                    .collect();
                // the reload clip plays at the magazine mod's rate: its sound keys and its end move with it
                // EVIDENCE: cshell+0x23a210
                let rate = self.anim_rate(w, false);
                let marks = at_rate(self.clip_marks(w, |c| c.reload.as_ref(), &dflt), rate);
                for m in self.due_marks(now, &marks) {
                    if let Mark::Sound(n) = m {
                        self.sound(&mut out, n, aim);
                    }
                }
                if t >= ms_at_rate(
                    self.clip_len(w, |c| c.reload.as_ref(), fallback::RELOAD_MS),
                    rate,
                ) {
                    self.finish_reload();
                    self.start(now, Phase::Idle);
                }
            }
            Phase::Fire => {
                // the fire clip loops while an automatic weapon's trigger is held: each pass starts where the last
                // ended (the cadence is the clip length, not the frame rate)
                let which = fire_clip_of(self.alt);
                // EVIDENCE: cshell+0x238110 (every clip of the firing state plays at 1 + the mods' Fireanirate)
                let rate = self.anim_rate(w, true);
                let marks = at_rate(self.clip_marks(w, which, &[(0, Mark::Fire)]), rate);
                let len = ms_at_rate(self.clip_len(w, which, fallback::FIRE_MS), rate);
                let single = self.single_shot(w);
                for _ in 0..64 {
                    for m in self.due_marks(now, &marks) {
                        match m {
                            Mark::Fire => self.fire_key(now, aim, targets, world, &mut out),
                            // the fire clip's sound keys (the L96's bolt at 720 ms) play while it runs
                            // SPEC: GAME_MESSAGES_2009_C2S.md §4.3 (a sound key of the playing clip plays the row's
                            // sound n locally and sends 0x80)
                            Mark::Sound(n) if self.phase == Phase::Fire => {
                                self.sound(&mut out, n, aim)
                            }
                            _ => {}
                        }
                    }
                    if self.phase != Phase::Fire || now.saturating_sub(self.phase_at) < len {
                        break;
                    }
                    let end = self.phase_at + len;
                    let round = self
                        .held()
                        .is_some_and(|c| c.clip > 0 || c.clip_size.is_none());
                    if self.trigger
                        && self.alt_wanted == self.alt
                        && !single
                        && round
                        && !self.reload_wanted
                    {
                        self.phase_at = end;
                        self.marks_done = None;
                    } else {
                        // the fire clip ran to its end with no next shot: the postfire clip follows when the weapon
                        // has one
                        // DISASM: cshell+0x238110 (the firing state's clip step: a finished fire clip is played again
                        // while the fire is wanted, else the postfire clip (+0x128) when there is one, else the state
                        // ends; the postfire clip ending ends the state)
                        // DC-DEFAULT: CLI-592 a postfire clip of zero length (rifle 145) counts as none
                        let post = ms_at_rate(self.clip_len(w, |c| c.postfire.as_ref(), 0), rate);
                        if post > 1 && !self.throwable(w) {
                            self.start(end, Phase::PostFire);
                        } else {
                            self.start(end, Phase::Idle);
                        }
                        break;
                    }
                }
            }
            Phase::PostFire => {
                // SPEC: GAME_MESSAGES_2009_C2S.md §4.3 (sound keys of the playing clip)
                let rate = self.anim_rate(w, true);
                let marks = at_rate(self.clip_marks(w, |c| c.postfire.as_ref(), &[]), rate);
                for m in self.due_marks(now, &marks) {
                    if let Mark::Sound(n) = m {
                        self.sound(&mut out, n, aim);
                    }
                }
                if t >= ms_at_rate(self.clip_len(w, |c| c.postfire.as_ref(), 0), rate) {
                    self.start(now, Phase::Idle);
                }
            }
            Phase::ThrowPre => {
                let dflt = [(
                    fallback::THROW_PRE_SOUND.0,
                    Mark::Sound(fallback::THROW_PRE_SOUND.1),
                )];
                let marks = self.clip_marks(w, |c| c.prefire.as_ref(), &dflt);
                for m in self.due_marks(now, &marks) {
                    if let Mark::Sound(n) = m {
                        self.sound(&mut out, n, aim);
                    }
                }
                // only the key going up releases: a sprint (which suppresses the fire bit) does not
                if !self.throw_key {
                    self.release_wanted = true;
                }
                let len = self.clip_len(w, |c| c.prefire.as_ref(), fallback::THROW_PRE_MS);
                if t >= len {
                    self.start(self.phase_at + len, Phase::ThrowHold);
                }
            }
            Phase::ThrowHold => {
                // the hold clip loops while the key is down, each pass from where the last ended; while sprinting
                // the hold goes on without 0x4d (the 2009 gap) and the passes restart when the sprint ends
                // DC-DEFAULT: CLI-573 (proposed) the hold pauses (no 0x4d, clip restarted) while sprinting
                let marks = self.clip_marks(w, |c| c.fire.as_ref(), &[(0, Mark::Throw)]);
                let len = self.clip_len(w, |c| c.fire.as_ref(), fallback::THROW_HOLD_MS);
                if sprinting && self.throw_key && !self.release_wanted {
                    self.phase_at = now;
                    self.marks_done = Some(0);
                }
                for _ in 0..64 {
                    for m in self.due_marks(now, &marks) {
                        if m == Mark::Throw {
                            self.stats.holds += 1;
                            out.push(Outgoing {
                                shell: up::THROW_HOLD,
                                body: BitWriter::new(),
                            });
                        }
                    }
                    if now.saturating_sub(self.phase_at) < len {
                        break;
                    }
                    let end = self.phase_at + len;
                    if self.throw_key && !self.release_wanted {
                        self.phase_at = end;
                        self.marks_done = None;
                    } else {
                        self.start(end, Phase::ThrowRelease);
                        break;
                    }
                }
            }
            Phase::ThrowRelease => {
                let dflt = [(fallback::THROW_RELEASE_AT, Mark::Fire)];
                let marks = self.clip_marks(w, |c| c.postfire.as_ref(), &dflt);
                for m in self.due_marks(now, &marks) {
                    if m == Mark::Fire {
                        self.throw_release(now, aim, &mut out);
                        // after the last one the empty switch in the same block; with rounds left the empty clip
                        // reloads at once (0x83 in the block of the release) and the throwable stays in the hand
                        // OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §1, C2S §4.6 (`76 03 02` + `7f [0][145][145]`);
                        // capture 20261025_125432_rust6/analysis (2-pack 143: the first
                        // release with 0x83 [143] in the same moment and the reload event 219-250 ms later, no `7f`)
                        if self.after_round_use(now, &mut out) {
                            return out;
                        }
                    }
                }
                if t >= self.clip_len(w, |c| c.postfire.as_ref(), fallback::THROW_POST_MS) {
                    self.release_wanted = false;
                    self.start(now, Phase::Idle);
                }
            }
            Phase::DryFire => {
                self.dry_fire_marks(now, w, aim, &mut out);
                if t >= self.clip_len(w, |c| c.dry_fire.as_ref(), fallback::DRY_FIRE_MS) {
                    self.start(now, Phase::Idle);
                }
            }
            Phase::Idle => {}
        }
        if self.phase == Phase::PostFire
            && (self.reload_wanted || (self.trigger && (self.pressed || !self.single_shot(w))))
        {
            // a new press (or the reload key) during the postfire clip: the weapon is ready again at once
            // DISASM: cshell+0x238110 (with the postfire clip playing and the fire wanted, the prefire or fire clip is
            // started over it)
            self.start(now, Phase::Idle);
        }
        if self.phase == Phase::Fire && self.reload_wanted {
            // the reload key cuts the fire clip short
            self.start(now, Phase::Idle);
        }
        if self.phase == Phase::Idle {
            let c = self.held().copied();
            if let Some(c) = c {
                let size = c.clip_size.unwrap_or(c.clip);
                if std::mem::take(&mut self.reload_wanted) && c.clip < size && c.stock > 0 {
                    self.begin_reload(now, &mut out);
                } else if self.trigger {
                    let single = self.single_shot(c.weapon);
                    if self.throwable(c.weapon) {
                        if self.pressed && c.total() > 0 {
                            self.pressed = false;
                            self.start(now, Phase::ThrowPre);
                        }
                    } else if c.clip > 0 || c.clip_size.is_none() {
                        if !single || self.pressed {
                            self.pressed = false;
                            self.alt = self.alt_wanted;
                            self.start(now, Phase::Fire);
                            let marks = self.clip_marks(
                                c.weapon,
                                fire_clip_of(self.alt),
                                &[(0, Mark::Fire)],
                            );
                            for m in self.due_marks(now, &marks) {
                                match m {
                                    Mark::Fire => self.fire_key(now, aim, targets, world, &mut out),
                                    Mark::Sound(n) if self.phase == Phase::Fire => {
                                        self.sound(&mut out, n, aim)
                                    }
                                    _ => {}
                                }
                            }
                        }
                    } else if self.pressed {
                        // an empty clip on the trigger: reload; with no round left at all the out-of-ammunition
                        // state plays the dry-fire clip ([`Arsenal::dry_fire_marks`])
                        // EVIDENCE: cshell+0x23b340 / WEAPON_FIRE_2009.md §6.2 (press from idle with no ammunition ->
                        // state 10), cshell+0x237e20 (state 10 plays the dry-fire clip +0x108)
                        // DC-DEFAULT: CLI-499 one dry-fire clip per press
                        self.pressed = false;
                        if !self.begin_reload(now, &mut out) && !self.loaded(&c) {
                            self.start(now, Phase::DryFire);
                            self.dry_fire_marks(now, c.weapon, aim, &mut out);
                        }
                    }
                }
            }
        }
        out
    }

    /// The keys of the dry-fire clip that came due: its fire key plays weapon sound 7 (`DryFireSnd`), a sound key its
    /// own sound; both go out as 0x80 and play locally like the other key sounds. Without a clip the fire key is at
    /// 0 ms.
    // OBSERVED: 2026-10-27, the ROM rifle 145 first-person dry-fire clip holds one fire key at 0 ms and no sound key (test
    // rom_sights_zoom_and_the_rifle_has_a_dry_fire_clip, counts only)
    // EVIDENCE: cshell+0x237e20 (state 10 plays the dry-fire clip), cshell+0x17bd20 (`DryFireSnd` = weapon sound 7)
    // DC-DEFAULT: CLI-499 the fire key of the dry-fire clip is the dry-fire sound
    fn dry_fire_marks(&mut self, now: u32, weapon: u16, aim: &Aim, out: &mut Vec<Outgoing>) {
        let dflt = [(fallback::DRY_FIRE_SOUND.0, Mark::Fire)];
        let marks = self.clip_marks(weapon, |c| c.dry_fire.as_ref(), &dflt);
        for m in self.due_marks(now, &marks) {
            match m {
                Mark::Fire => self.sound(out, fallback::DRY_FIRE_SOUND.1, aim),
                Mark::Sound(n) => self.sound(out, n, aim),
                Mark::Throw => {}
            }
        }
    }

    /// The zoom key of a step: its press cycles the zoom level of the held weapon's sight, unless the weapon reloads,
    /// a bolt-action weapon (`ReloadAfterFire`) is in its fire clip, or the player sprints; entering the sprint state
    /// with a `Guntype` 0xb weapon zooms out.
    // EVIDENCE: cshell+0x1f95d0 (command 0x3d -> cshell+0x1f4180(-1)), cshell+0x1f4180 (refused in the reload state 9,
    // and in the fire state 3 with row +0x200 set when cshell+0x5c450 returns 0, PLAUSIBLE: that test is not read),
    // cshell+0x23ac50 (sprint state entered with row +0x418 == 0xb -> player manager +0x78(0))
    // DC-DEFAULT: CLI-496 a press while sprinting is ignored
    fn zoom_step(&mut self, now: u32, key: bool, sprinting: bool) {
        let pressed = key && !self.zoom_key;
        self.zoom_key = key;
        let sprint_started = sprinting && !self.was_sprinting;
        self.was_sprinting = sprinting;
        // Most frames carry neither a press nor a sprint start: read the row and the sight only then.
        if !pressed && !sprint_started {
            return;
        }
        let Some(c) = self.held().copied() else {
            return;
        };
        let row = self.row(c.weapon).cloned().unwrap_or_default();
        let zoom_time = row.zoom_time.unwrap_or(crate::zoom::ZOOM_TIME_DEFAULT);
        let sight = self.sight_of(Some(c.weapon)).cloned();
        if sprint_started && row.gun_type == Some(GUN_TYPE_SPRINT_UNZOOM) && self.scope.level() > 0
        {
            self.scope
                .step_level(now, Some(0), sight.as_ref(), self.normal_fov_x, zoom_time);
        }
        if !pressed || sprinting {
            return;
        }
        // the firing state (its postfire clip included) refuses a bolt-action weapon's zoom-in, not its zoom-out
        // DISASM: cshell+0x1f4180 (state 3, cshell+0x5c450 = not zoomed, row +0x200 -> refused), cshell+0x238110 (the
        // postfire clip plays in the firing state)
        let refused = match self.phase {
            Phase::Reload => true,
            Phase::Fire | Phase::PostFire => row.reload_after_fire && !self.scope.active(),
            _ => false,
        };
        if !refused {
            self.scope
                .step_level(now, None, sight.as_ref(), self.normal_fov_x, zoom_time);
        }
    }

    /// A reload asked while the scope is up (the key, or the empty clip): during a change it does nothing; at a zoom
    /// level the reload waits for a zoom-out and runs when it ends. A plain weapon zooms out at once; a bolt-action
    /// (`ReloadAfterFire`) weapon arms its zoom-out timer ([`crate::zoom::BOLT_UNZOOM_MS`]). Returns whether the
    /// request was taken here.
    // DISASM: cshell+0x1f6520 (+0xf4 set: nothing; zoomed: row +0x200 clear -> zoom-out cshell+0x1f4180(0), set ->
    // +0x1b3 := 1 / +0x1b4 := now when +0x1b3 was clear; both +0x1ac := 1, the reload at the end of the change,
    // cshell+0x1f9100); callers: the reload command (cshell+0x1f95d0) and the empty-clip test of the weapon update
    // (cshell+0x237180: clip < 1, stock > 0, not reloading)
    fn reload_while_zoomed(&mut self, now: u32) -> bool {
        if !self.scope.active() {
            return false;
        }
        if self.scope.changing() {
            return true;
        }
        let Some(c) = self.held().copied() else {
            return false;
        };
        let size = c.clip_size.unwrap_or(c.clip);
        if c.stock == 0 || c.clip >= size {
            return true;
        }
        let row = self.row(c.weapon).cloned().unwrap_or_default();
        self.scope.defer_reload();
        if row.reload_after_fire {
            self.scope.arm_bolt_unzoom(now, false);
            return true;
        }
        let zoom_time = row.zoom_time.unwrap_or(crate::zoom::ZOOM_TIME_DEFAULT);
        let sight = self.sight_of(Some(c.weapon)).cloned();
        self.scope
            .step_level(now, Some(0), sight.as_ref(), self.normal_fov_x, zoom_time);
        true
    }

    /// The bolt-action zoom-out: once its timer is due with the scope up, the held `ReloadAfterFire` weapon issues
    /// the zoom command (the next level, from the top level back to 0).
    // DISASM: cshell+0x1f8b40 (+0x1b3, `300 < now - +0x1b4`, zoomed, row +0x200 -> command 0x3d), cshell+0x1f4180
    // (the zoom step; refused in the reload state)
    fn bolt_unzoom(&mut self, now: u32) {
        if !self.scope.bolt_unzoom_due(now) || self.phase == Phase::Reload {
            return;
        }
        let Some(c) = self.held().copied() else {
            return;
        };
        let row = self.row(c.weapon).cloned().unwrap_or_default();
        if !row.reload_after_fire {
            return;
        }
        let zoom_time = row.zoom_time.unwrap_or(crate::zoom::ZOOM_TIME_DEFAULT);
        let sight = self.sight_of(Some(c.weapon)).cloned();
        self.scope
            .step_level(now, None, sight.as_ref(), self.normal_fov_x, zoom_time);
    }

    /// A live world camera (the round-end shot): while it is live the weapons take no command (no fire, zoom, reload,
    /// switch) and the aim mark is not drawn; going live leaves the zoom at once. The weapons come back when the
    /// camera goes off.
    // EVIDENCE: cshell+0x1f50b0 (a live camera: the camera manager's +0x8c := 1 once, with the zoom reset
    // cshell+0x1f3590(1) and cshell+0x22d260, which turns every carried weapon off through its slot +0x140(0)),
    // cshell+0x1f2ca0 (no live camera: the own view back)
    // DC-DEFAULT: CLI-982 (proposed) "off" = no command reaches the weapon and no aim mark is drawn (what slot +0x140
    // does in the weapon object is not read)
    pub fn set_cinematic(&mut self, on: bool) {
        if on && !self.cinematic {
            self.reset_zoom();
        }
        self.cinematic = on;
    }

    /// Whether a live world camera holds the weapons ([`Arsenal::set_cinematic`]).
    pub fn cinematic(&self) -> bool {
        self.cinematic
    }

    /// The instant zoom reset with the held weapon's sight (its zoom-out sound when the scope was up).
    fn reset_zoom(&mut self) {
        let sight = self.sight_of(self.held().map(|c| c.weapon)).cloned();
        self.scope.reset(sight.as_ref());
    }

    /// A class key: key `k` (0-based) cycles the weapons of order class `k + 1` in their class order. With the held
    /// weapon in that class the next entry is drawn (wrapping); from another class the entry drawn last in it, else
    /// its first. The throw class skips entries that may not be drawn; any other class draws its entry only when it
    /// may be drawn ([`Arsenal::may_draw`]). Without a weapon order (no ROM table) key `k` chooses slot `k`.
    // EVIDENCE: cshell+0x22e530 (class tables of 32 slots per class, 0xffff ends; the held weapon's class == key: the next
    // entry, wrapping to the first; class 4 loops past entries that are not loaded; the draw test of classes 1 / 2 /
    // the rest; keys above 7 refused), cshell+0x22e370 (empty `HideWhenEmpty` entries skipped), cshell+0x47da0 (class)
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (`class_pos` = the place in the class list)
    // DC-DEFAULT: CLI-498 the entry from another class, the order without `class_pos`, the no-table fallback
    fn class_pick(&mut self, k: u8) -> Option<u8> {
        let class = k.checked_add(1)?;
        if class > CLASS_KEY_MAX {
            return None;
        }
        if self.cfg.table.as_ref().is_none_or(|t| t.classes.is_empty()) {
            return (k < KEY_SLOTS).then_some(k);
        }
        let mut list: Vec<u8> = self
            .slots
            .values()
            .filter(|c| self.order_class(c.weapon) == class)
            .map(|c| c.slot)
            .collect();
        list.sort_by_key(|s| (self.class_pos.get(s).copied().unwrap_or(u8::MAX), *s));
        if list.is_empty() {
            return None;
        }
        let held_at = self.held.and_then(|h| list.iter().position(|s| *s == h));
        let start = match held_at {
            Some(i) => (i + 1) % list.len(),
            None => self
                .class_slots
                .get(&class)
                .and_then(|s| list.iter().position(|x| x == s))
                .unwrap_or(0),
        };
        if class == THROW_CLASS {
            for i in 0..list.len() {
                let s = list[(start + i) % list.len()];
                if self.may_draw(s) {
                    return Some(s);
                }
            }
            return None;
        }
        let s = list[start];
        self.may_draw(s).then_some(s)
    }

    /// One HUD line: weapon, slot, clip and stock, what it is doing, the spread.
    pub fn hud_line(&self) -> Option<String> {
        let c = self.held()?;
        Some(format!(
            "weapon {} [slot {}] clip {} / {}  {}  spread {:.2}  shots {} hits {}",
            c.weapon,
            c.slot + 1,
            c.clip,
            c.stock,
            self.phase.label(),
            self.spread(),
            self.stats.shots,
            self.stats.hits
        ))
    }

    /// The play rate of the clip of `phase`: the magazine mod's for the reload, the muzzle mod's for the firing state.
    fn phase_rate(&self, weapon: u16, phase: Phase) -> f32 {
        match phase {
            Phase::Reload => self.anim_rate(weapon, false),
            Phase::Fire | Phase::PostFire => self.anim_rate(weapon, true),
            _ => 1.0,
        }
    }

    /// What the first-person view plays: the clip (index in the composed clip list) and the time into it.
    pub fn view_clip(&mut self, now: u32) -> Option<(u16, usize, f32)> {
        let w = self.held()?.weapon;
        let t = now.saturating_sub(self.phase_at) as f32;
        let phase = self.phase;
        let c = self.clips_of(w)?.clone();
        let pick = match phase {
            Phase::Select => c.select.as_ref(),
            Phase::Fire => fire_clip_of(self.alt)(&c),
            Phase::ThrowHold => c.fire.as_ref(),
            Phase::Reload => c.reload.as_ref(),
            Phase::ThrowPre => c.prefire.as_ref(),
            Phase::ThrowRelease => c.postfire.as_ref(),
            Phase::DryFire => c.dry_fire.as_ref().or(c.idle.as_ref()),
            Phase::PostFire => c.postfire.as_ref(),
            Phase::Idle => c.idle.as_ref(),
        };
        let (index, time) = match pick {
            // the clip plays at the phase's rate, so its end is the phase's end (a rate below 1 stretches the phase:
            // the unscaled time ran past the clip and the view wrapped it)
            // EVIDENCE: cshell+0x23a210 (reload clip), cshell+0x238110 (every clip of the firing state)
            // DC-DEFAULT: CLI-1441 (amendment, proposed) the first-person view plays the clip at the same rate as the
            // phase, so the clip ends when the phase does
            Some(f) => (f.index, t * self.phase_rate(w, phase)),
            None => (c.idle.as_ref().or(c.select.as_ref())?.index, 0.0),
        };
        // the sprint clips replace the idle clip while they play ([`crate::viewanim`])
        if phase == Phase::Idle {
            let lens = self.sprint_lens(w);
            if let Some((part, st)) = self.sprint_anim.playing(now, &lens) {
                use crate::viewanim::SprintPart;
                let f = match part {
                    SprintPart::Enter => c.sprint_enter.as_ref(),
                    SprintPart::Loop => c.sprint_loop.as_ref(),
                    SprintPart::Exit => c.sprint_exit.as_ref(),
                };
                if let Some(f) = f {
                    return Some((w, f.index, st));
                }
            }
        }
        Some((w, index, time))
    }
}

/// One pellet of an own shot as the weapon effects need it: the muzzle point (the 0x82 `weapon_pos`), the shot
/// origin and direction, the trace range and what the trace hit; `first` on the shot's first pellet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotFx {
    pub weapon: u16,
    pub ammo: u16,
    pub origin: [f32; 3],
    pub muzzle: [f32; 3],
    pub dir: [f32; 3],
    pub range: f32,
    pub hit: RayHit,
    pub first: bool,
    pub time_ms: u32,
    /// The world impact of a pellet that hit no character, resolved by the frame source when it was fired (CLI-250;
    /// `None`: the effect player traces it itself).
    pub impact: Option<crate::world::ShotImpactAt>,
    /// The `[Mod<n>]` in the weapon's muzzle socket (its muzzle effect replaces the row's, CLI-619).
    pub flash_mod: Option<u16>,
}

impl Arsenal {
    /// The pellets fired since the last call (the weapon effects drain them once per frame).
    pub fn take_shot_fx(&mut self) -> Vec<ShotFx> {
        std::mem::take(&mut self.shot_fx)
    }

    /// The recoil's view yaw change since the last call (radians, added to the yaw); the frame loop adds it to the
    /// view.
    pub fn take_view_yaw(&mut self) -> f32 {
        std::mem::take(&mut self.view_yaw)
    }

    /// `aim` with the recoil drained up to `now` (the drain is also kept for the frame loop).
    // DC-DEFAULT: CLI-311 (proposed) the shot's angles include the drain up to the shot's own session time (the 2009
    // shot took the frame's drain, so its angles depended on the frame time)
    fn drained_aim(&mut self, weapon: u16, now: u32, aim: &Aim) -> Aim {
        let row = self.spread_row(weapon);
        let d = self.recoil.drain(
            &row.recoil,
            now as f64,
            crate::spread::clock::TICK_MS,
            self.triple,
        );
        self.view_pitch += d.pitch;
        self.view_yaw += d.yaw;
        let lim = crate::look::LookParams::default().pitch_limit;
        Aim {
            yaw: aim.yaw + self.view_yaw,
            pitch: (aim.pitch + self.view_pitch).clamp(-lim, lim),
            ..*aim
        }
    }

    /// A `GS_DAMAGE_TAKEN` at `now` on the own player: the camera jar ([`crate::camfx`]), facing the last frame's
    /// view yaw.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 row 0x74 (ignored in states 2, 3, 5, 6: the arsenal is not alive then)
    // DC-DEFAULT: CLI-316 (proposed) the body frame of the sector is the view yaw with pitch 0 (the 2009 body yaw,
    // controller +0x6c, follows the view; its per-frame writer is not traced)
    pub fn own_hit(&mut self, now: u32, d: &dc_botclient::weapon::DamageTaken) -> Option<u8> {
        if !self.alive {
            return None;
        }
        let yaw = self.last_aim.map_or(0.0, |a| a.yaw) + self.view_yaw;
        self.camfx
            .jar_hit(now as f64, d.from_dir, d.damage_type, d.damage, yaw)
    }

    /// The camera offsets of the jar at `now` (zero without a running jar).
    pub fn camera_angles(&self, now: u32) -> crate::camfx::CamAngles {
        if !self.alive {
            return crate::camfx::CamAngles::default();
        }
        self.camfx.angles(now as f64)
    }

    /// How far the recoil's kick-back moves the eye back along the view (units).
    pub fn eye_back(&self) -> f32 {
        if self.alive {
            self.recoil.eye_back()
        } else {
            0.0
        }
    }

    /// The node spheres of this frame's characters for the shots of the next [`Arsenal::step`] (a character that is
    /// not in `nodes` is hit by its box alone, node 0xff).
    // DC-DEFAULT: CLI-300 (proposed)
    pub fn set_hit_nodes(&mut self, nodes: crate::hitnodes::FrameNodes) {
        self.hit_nodes = nodes;
    }

    /// The world muzzle point of `weapon` (the 0x82 `weapon_pos`, the 0x80 position): the shot origin plus the view
    /// rotation applied to the row's `Pos` + `MuzzlePos` (eye space: right, up, forward); a row without them keeps the
    /// observed offset [`WEAPON_OFFSET`] turned by the yaw.
    // EVIDENCE: cshell+0x2348d0 (every frame the eye-space muzzle point := Pos + MuzzlePos + bob + kick, and the world
    // muzzle point weapon +0xa8 := camera rotation x that + camera position; the 0x82 sends weapon +0xa8)
    // OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §3 (weapon 145: 4.69 right, 4.27 down, 8.85 forward = its row's Pos +
    // MuzzlePos within 0.12 units); GAME_MESSAGES_2009_C2S.md §4.1.1 (the knife's `weapon_pos` = `origin` exactly: both
    // of its row vectors are zero)
    pub fn muzzle_point(&self, weapon: u16, aim: &Aim) -> [f32; 3] {
        let row = self.row(weapon);
        match row.and_then(|r| r.pv_pos.zip(r.muzzle_pos)) {
            Some((p, m)) => {
                let q = dc_botclient::shooter::aim_quat(aim.yaw, aim.pitch);
                let q = glam::Quat::from_xyzw(q[0], q[1], q[2], q[3]);
                let v = q * glam::Vec3::new(p[0] + m[0], p[1] + m[1], p[2] + m[2]);
                [
                    aim.origin[0] + v.x,
                    aim.origin[1] + v.y,
                    aim.origin[2] + v.z,
                ]
            }
            None => weapon_pos(aim.origin, aim.yaw),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_botclient::shell::shell_msg;
    use dc_botclient::weapon::read_fire;

    /// Bytes of the delivered message `[11][shell][body]`.
    fn wire(o: &Outgoing) -> Vec<u8> {
        shell_msg(o.shell, &o.body).as_bytes().to_vec()
    }

    fn bits(o: &Outgoing) -> usize {
        shell_msg(o.shell, &o.body).bit_len()
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn f(h: &str) -> f32 {
        f32::from_le_bytes(unhex(h).try_into().unwrap())
    }

    fn grant(slot: u8, weapon: u16, ammo_kind: u16, ammo: f32) -> WeaponGrant {
        WeaponGrant {
            weapon,
            ammo_kind,
            ammo,
            clip: None,
            class_pos: 0,
            slot,
            equip: false,
            parts: [-1; 4],
        }
    }

    /// A made-up table (numbers are ours): weapon 3 = a 4-pellet single-shot weapon, clip 3; weapon 5 = an
    /// automatic weapon, clip 3; weapon 9 = a throwable (client kind 2), one per clip.
    fn table() -> std::sync::Arc<WeaponTable> {
        std::sync::Arc::new(WeaponTable::from_doc(&ButeDoc::parse(&table_text())))
    }

    /// CLI-1611: the server set's content patches the client's own table (clip and ammunition row of weapon 3,
    /// a registered weapon row); a refused document leaves the table as it was.
    #[test]
    fn apply_content_patches_the_predicted_clip_and_ammo() {
        let mut t = WeaponTable::from_doc(&ButeDoc::parse(&table_text()));
        assert_eq!((t.rows[&3].clip, t.rows[&3].max_amount), (Some(3), Some(9)));
        let c = dc_bute::modpatch::WeaponContent::from_json(concat!(
            r#"{"version":1,"weapons":[{"weapon":3,"op":"patch","fields":{"ShotsPerClip":"7"}},"#,
            r#"{"weapon":40,"op":"register","fields":{"AmmoName0":"aut","ShotsPerClip":"12"}}],"#,
            r#""ammo":[{"ammo":0,"op":"patch","fields":{"MaxAmount":"20"}}]}"#
        ))
        .unwrap();
        t.apply_content(&c).unwrap();
        assert_eq!(
            (t.rows[&3].clip, t.rows[&3].max_amount),
            (Some(7), Some(20))
        );
        assert_eq!(t.rows[&3].pellets, Some(4), "other keys stay");
        assert_eq!((t.rows[&40].clip, t.rows[&40].ammo), (Some(12), Some(1)));
        let bad = dc_bute::modpatch::WeaponContent::from_json(
            r#"{"version":1,"weapons":[{"weapon":77,"op":"patch","fields":{"ShotsPerClip":"1"}}]}"#,
        )
        .unwrap();
        assert!(t.apply_content(&bad).is_err());
        assert_eq!(
            t.rows[&3].clip,
            Some(7),
            "a refused document changes nothing"
        );
    }

    fn table_text() -> String {
        [
            "[Ammo0]",
            "Name = \"pel\"",
            "MaxAmount = 9",
            "[Ammo1]",
            "Name = \"aut\"",
            "MaxAmount = 9",
            "FirePerturb = 0.5",
            "DecDelay = 400",
            "[Ammo2]",
            "Name = \"thr\"",
            "MaxAmount = 2",
            "[Weapon3]",
            "AmmoName0 = \"pel\"",
            "VectorsPerRound = 4",
            "ShotsPerClip = 3",
            "RapidFireCount = 1",
            "MinPerturb = 10",
            "MaxPerturb = 50",
            "[Weapon5]",
            "AmmoName0 = \"aut\"",
            "VectorsPerRound = 1",
            "ShotsPerClip = 3",
            "RapidFireCount = 0",
            "DynamicPerturbFactor = 1.5",
            "MinPerturb = 2",
            "MaxPerturb = 80",
            "PerturbIncSpeed = 1.0",
            "PerturbDecSpeed = 0.5",
            "[Weapon9]",
            "AmmoName0 = \"thr\"",
            "ShotsPerClip = 1",
            "ClientWeaponType = 2",
        ]
        .join("\n")
    }

    fn arsenal() -> Arsenal {
        Arsenal::new(ArmsConfig {
            table: Some(table()),
            ..ArmsConfig::default()
        })
    }

    fn aim() -> Aim {
        Aim {
            origin: [0.0, 33.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            control: 0x200,
        }
    }

    fn fire_cmd(fire: bool) -> WeaponCommands {
        WeaponCommands {
            fire,
            ..WeaponCommands::default()
        }
    }

    fn pick(slot: u8) -> WeaponCommands {
        WeaponCommands {
            select: Some(slot),
            ..WeaponCommands::default()
        }
    }

    /// Runs the arsenal from `from` to `to` (ms) in 16 ms frames; returns (time, message) pairs.
    fn run(
        a: &mut Arsenal,
        from: u32,
        to: u32,
        cmd: impl Fn(u32) -> WeaponCommands,
        targets: &[Target],
    ) -> Vec<(u32, Outgoing)> {
        let mut out = Vec::new();
        let mut t = from;
        while t < to {
            for m in a.step(t, &cmd(t), &aim(), targets, None) {
                out.push((t, m));
            }
            t += 16;
        }
        out
    }

    fn shells(v: &[(u32, Outgoing)]) -> Vec<u8> {
        v.iter().map(|m| m.1.shell).collect()
    }

    #[test]
    fn select_pair_is_the_2009_clients() {
        // OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §2: the four grants of the spawn set, then `0b760302` and
        // `0b7f0091009100` (slot 0, weapon 145, aux = its ammunition id 145); slot 3 = weapon 7 with ammunition 9
        let mut a = Arsenal::new(ArmsConfig::default());
        for g in [
            grant(0, 145, 145, 120.0),
            grant(1, 164, 164, 45.0),
            grant(2, 0, 0, 1000.0),
            grant(3, 7, 9, 1.0),
        ] {
            a.on_grant(&g);
        }
        assert!(
            a.step(0, &fire_cmd(false), &aim(), &[], None).is_empty(),
            "dead: nothing"
        );
        a.on_life(10, 1);
        let out = a.step(10, &fire_cmd(false), &aim(), &[], None);
        assert_eq!(out.len(), 2);
        assert_eq!(hex(&wire(&out[0])), "0b760302");
        assert_eq!(bits(&out[0]), 32);
        assert_eq!(hex(&wire(&out[1])), "0b7f0091009100");
        assert_eq!(bits(&out[1]), 56);
        assert_eq!(a.held().map(|c| c.weapon), Some(145));
        // the number key 4 = slot 3
        let out = a.step(30, &pick(3), &aim(), &[], None);
        assert_eq!(hex(&wire(&out[1])), "0b7f0307000900");
        // the wheel steps from slot 3 to the next: slot 0 (wraps)
        let out = a.step(
            40,
            &WeaponCommands {
                wheel: 1,
                ..WeaponCommands::default()
            },
            &aim(),
            &[],
            None,
        );
        assert_eq!(hex(&wire(&out[1])), "0b7f0091009100");
        // without the ROM the draw sound 3 follows 500 ms after the select (CLI-29)
        let v = run(&mut a, 50, 700, |_| fire_cmd(false), &[]);
        let snd: Vec<&(u32, Outgoing)> =
            v.iter().filter(|m| m.1.shell == up::WEAPON_SOUND).collect();
        assert_eq!(snd.len(), 1);
        assert!((530..=560).contains(&snd[0].0), "{}", snd[0].0);
        // `[sound 3][weapon 145][own id 0][weapon_pos]`: 144 bits with the ids
        assert_eq!(bits(&snd[0].1), 144);
        assert_eq!(&wire(&snd[0].1)[..5], &[0x0b, 0x80, 0x03, 0x91, 0x00]);
    }

    #[test]
    fn fire_messages_match_the_proven_forms() {
        // OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §3: rifle shot 1 head and the grenade release head (inputs: the
        // floats of those heads, the stated ids; the remaining fields as the table there gives them)
        let hit = ShotInputs {
            weapon_id: 145,
            ammo_id: 145,
            weapon_pos: [f("f744b6c4"), f("632d2f44"), f("23118a44")],
            origin: [f("3885b7c4"), 704.979, 1104.178],
            aim: [0.0059, 0.4997, -0.0034, 0.8662],
            yaw: 1.0465,
            dir: [0.866, -0.013, 0.4997],
            seed: 119,
            spread: 0.0,
            shot_scale: DEFAULT_SHOT_SCALE,
            zoom: false,
            triple: false,
            alt: false,
            time_ms: 75401,
            hit: RayHit::Character {
                obj: 408,
                dist: 1329.886,
                node: 24,
                normal: [-1.0, 0.0, 0.0],
            },
            last: true,
            stat: 2,
        };
        let b = shell_msg(up::FIRE, &fire_message(&hit));
        assert_eq!(b.bit_len(), 887);
        assert_eq!(
            hex(&b.as_bytes()[..23]),
            &"0b820091009100f744b6c4632d2f4423118a443885b7c4b0"[..46]
        );
        // field positions (bit offsets from the message start, C2S §4.1.1)
        let mut r = b.reader();
        r.seek_bits(523);
        assert_eq!(r.read_u32(), 0x3f80_0000, "shot_scale 1.0");
        r.seek_bits(555);
        assert_eq!(r.read_u32(), 75401);
        r.seek_bits(587);
        assert!(r.read_bit(), "has_extra");
        assert_eq!(r.read_u16(), 408, "extra_obj @588");
        assert_eq!(r.read_u8(), 24, "extra_kind @604");
        let hp = [r.read_f32(), r.read_f32(), r.read_f32()];
        for (k, got) in hp.iter().enumerate() {
            let want = hit.origin[k] + 1329.886 * hit.dir[k];
            assert!(
                (got - want).abs() < 1e-2,
                "extra_a = origin + hit_dist * dir"
            );
        }
        assert_eq!([r.read_f32(), r.read_f32(), r.read_f32()], HIT_VECTOR);
        r.seek_bits(805);
        assert!(r.read_bit() && r.read_bit(), "last @805, has_hit @806");
        assert_eq!(r.read_f32(), 1329.886, "hit_dist @807");
        assert_eq!(r.read_u16(), 408, "hit_obj @839");
        assert_eq!(r.read_u32(), 2, "stat @855");
        assert!(r.at_end());
        // the grenade release: no extra, no hit, 623 bits
        let rel = ShotInputs {
            weapon_id: 7,
            ammo_id: 9,
            weapon_pos: [f("75011ac5"), f("dc211544"), f("e2fe0345")],
            origin: [f("62da19c5"), 600.0, 2000.0],
            aim: [-0.0002, 0.84, -0.5426, -0.0003],
            yaw: 9.4255,
            dir: [-0.001, -0.911, -0.412],
            seed: 22,
            spread: 0.0,
            shot_scale: DEFAULT_SHOT_SCALE,
            zoom: false,
            triple: false,
            alt: false,
            time_ms: 107484,
            hit: RayHit::Miss,
            last: true,
            stat: 10,
        };
        let b = shell_msg(up::FIRE, &fire_message(&rel));
        assert_eq!(b.bit_len(), 623);
        assert_eq!(
            hex(&b.as_bytes()[..23]),
            &"0b82000700090075011ac5dc211544e2fe034562da19c5b0"[..46]
        );
        let mut r = b.reader();
        r.seek_bits(16);
        let back = read_fire(&mut r).unwrap();
        assert!(back.extra.is_none() && back.hit.is_none() && back.last && back.stat == 10);
        assert!(r.at_end());
        // the reload and the reloaded event: `0b 83 91 00`, `0b 76 03 01 1e`
        assert_eq!(
            hex(shell_msg(up::RELOAD, &reload_body(145)).as_bytes()),
            "0b839100"
        );
        assert_eq!(
            hex(shell_msg(up::WEAPON_EVENT, &reloaded_event_body(30)).as_bytes()),
            "0b7603011e"
        );
        // OBSERVED: C2S §4.3, tdm1 head `0b 80 03 91 00 00 cb db 75 c4 65 c3 55 44 74 7f cd c4`
        let s = shell_msg(
            up::WEAPON_SOUND,
            &weapon_sound_body(3, 145, 0, [f("cbdb75c4"), f("65c35544"), f("747fcdc4")]),
        );
        assert_eq!(hex(s.as_bytes()), "0b8003910000cbdb75c465c35544747fcdc4");
        assert_eq!(s.bit_len(), 144);
    }

    #[test]
    fn automatic_cadence_reload_and_pellets() {
        let mut a = arsenal();
        a.on_grant(&grant(0, 5, 1, 9.0));
        a.on_grant(&grant(1, 3, 0, 9.0));
        a.on_life(0, 1);
        // the select (500 ms without the ROM), then the trigger held
        let v = run(&mut a, 0, 520, |_| fire_cmd(false), &[]);
        assert_eq!(shells(&v)[..2], [up::WEAPON_EVENT, up::WEAPON_CHANGE]);
        let target = [Target {
            obj: 77,
            centre: [0.0, 40.0, 500.0],
            half: [16.0, 40.0, 16.0],
        }];
        let v = run(&mut a, 528, 1200, |_| fire_cmd(true), &target);
        let shots: Vec<u32> = v
            .iter()
            .filter(|m| m.1.shell == up::FIRE)
            .map(|m| m.0)
            .collect();
        // one shot per 105 ms fire clip (CLI-29 without the ROM): 3 in the clip, then the reload
        assert_eq!(shots.len(), 3, "{shots:?}");
        for w in shots.windows(2) {
            assert!((105..=125).contains(&(w[1] - w[0])), "{shots:?}");
        }
        // the shot that empties the clip is followed by the 0x83 in the same moment
        let i = v
            .iter()
            .position(|m| m.1.shell == up::RELOAD)
            .expect("reload");
        assert_eq!(v[i - 1].1.shell, up::FIRE);
        assert_eq!(v[i - 1].0, v[i].0);
        assert_eq!(hex(&wire(&v[i].1)), "0b830100");
        // every shot hit the box: the 887-bit form, stat 2, 4, 6, frac 0 on the first shot then rising
        let reports: Vec<FireReport> = v
            .iter()
            .filter(|m| m.1.shell == up::FIRE)
            .map(|m| {
                assert_eq!(bits(&m.1), 887);
                read_fire(&mut m.1.body.reader()).unwrap()
            })
            .collect();
        assert_eq!(
            reports.iter().map(|r| r.stat).collect::<Vec<_>>(),
            vec![2, 4, 6]
        );
        // the spread before each shot: 0, then 0.0125 x 3.0 (no [Perturb] section) x PerturbIncSpeed 1.0 per shot,
        // no fall inside DecDelay 400 ms (crate::spread)
        assert_eq!(
            reports.iter().map(|r| r.frac).collect::<Vec<_>>(),
            vec![0, 9, 19]
        );
        assert_eq!(
            reports[0].shot_no,
            1.5f32.to_bits(),
            "DynamicPerturbFactor of the row"
        );
        assert_eq!(reports[0].hit.map(|h| h.1), Some(77));
        let d = reports[0].hit.unwrap().0;
        // no skeleton here: the shot box alone, doubled about its centre (CLI-300): z = 500 - 2 x 16
        assert!(
            (d - 468.0).abs() < 2.0,
            "enters the shot box at z = 468: {d}"
        );
        assert!(a.trigger_flags(1200));
        // the reload: the event 150 ms later with the clip size, sounds 1, 2, 3, the clip full after 2000 ms
        let reload_at = v[i].0;
        let mut v = v;
        v.extend(run(&mut a, 1200, 3300, |_| fire_cmd(false), &[]));
        let ev: Vec<&(u32, Outgoing)> =
            v.iter().filter(|m| m.1.shell == up::WEAPON_EVENT).collect();
        assert_eq!(ev.len(), 1);
        assert_eq!(hex(&wire(&ev[0].1)), "0b76030103");
        assert!(
            (150..=170).contains(&(ev[0].0 - reload_at)),
            "{} {reload_at}",
            ev[0].0
        );
        let sounds: Vec<u8> = v
            .iter()
            .filter(|m| m.1.shell == up::WEAPON_SOUND)
            .map(|m| wire(&m.1)[2])
            .collect();
        assert_eq!(sounds, vec![1, 2, 3]);
        assert_eq!(a.held().map(|c| (c.clip, c.stock)), Some((3, 3)));
        assert!(!a.trigger_flags(3300));
        // the spread fell back to 0 while no shot went out
        assert_eq!(a.spread(), 0.0);
    }

    /// The weapon-table mod rows in the fire cycle: a Fast Loader style magazine (negative `ShotsPerClipPlus`, a
    /// ROM-style trailing comment, `Reloadanirate` 0.25) shrinks the clip and plays the reload clip at 1.25; a muzzle
    /// mod's `Fireanirate` 0.5 plays the fire clip at 1.5 (cshell+0x233430, cshell+0x23a210, cshell+0x238110).
    #[test]
    fn mod_rows_shrink_the_clip_and_speed_the_fire_and_reload_clips() {
        // made-up rows: weapon 5 (clip 3, the 105 ms / 2000 ms stand-in clips) lists mods 0 (magazine) and 1 (muzzle)
        let text = table_text()
            + "\n[Mod0]\nName = \"fast\"\nKinds = 2\nType = 5\nShotsPerClipPlus0 = -1//3 rounds (30%)\n\
               Reloadanirate = 0.25\n[Mod1]\nName = \"comp\"\nKinds = 0\nType = 0\nFireanirate = 0.5\n";
        let text = text.replace(
            "[Weapon5]\n",
            "[Weapon5]\nModName0 = \"fast\"\nModModel0 = 0\nModName1 = \"comp\"\nModModel1 = 0\n",
        );
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(std::sync::Arc::new(WeaponTable::from_doc(&ButeDoc::parse(
                &text,
            )))),
            ..ArmsConfig::default()
        });
        a.on_grant(&WeaponGrant {
            parts: [1, -1, 0, -1],
            ..grant(0, 5, 1, 9.0)
        });
        a.on_life(0, 1);
        assert_eq!(a.clip_bonus(5), -1);
        let c = &a.slots[&0];
        assert_eq!(
            (c.clip_size, c.clip, c.stock),
            (Some(2), 2, 7),
            "3 - 1 rounds"
        );
        assert_eq!((a.anim_rate(5, true), a.anim_rate(5, false)), (1.5, 1.25));
        run(&mut a, 0, 520, |_| fire_cmd(false), &[]);
        let v = run(&mut a, 528, 1200, |_| fire_cmd(true), &[]);
        let shots: Vec<u32> = v
            .iter()
            .filter(|m| m.1.shell == up::FIRE)
            .map(|m| m.0)
            .collect();
        assert_eq!(shots.len(), 2, "the smaller clip: {shots:?}");
        // 105 ms / 1.5 = 70 ms between shots
        assert!((70..=86).contains(&(shots[1] - shots[0])), "{shots:?}");
        let reload_at = v
            .iter()
            .find(|m| m.1.shell == up::RELOAD)
            .expect("reload")
            .0;
        // 2000 ms / 1.25 = 1600 ms of reload
        run(&mut a, 1200, reload_at + 1560, |_| fire_cmd(false), &[]);
        assert_eq!(a.held().map(|c| c.clip), Some(0), "still reloading");
        run(
            &mut a,
            reload_at + 1560,
            reload_at + 1660,
            |_| fire_cmd(false),
            &[],
        );
        assert_eq!(a.held().map(|c| (c.clip, c.stock)), Some((2, 5)));
        // no mod: the row's clip, the clips at their own speed
        a.set_mods(5, [None; 3]);
        assert_eq!((a.anim_rate(5, true), a.anim_rate(5, false)), (1.0, 1.0));
        assert_eq!(a.modded_clip(5, 3), 3);
    }

    /// An extended magazine (`Reloadanirate` -0.5, rate 0.5) stretches the reload phase to twice the clip; the view
    /// plays the clip at the same rate, so its time reaches the clip's length exactly when the phase ends (no wrap).
    #[test]
    fn reload_view_clip_time_follows_the_mod_rate() {
        let text = table_text()
            + "
[Mod0]
Name = \"ext\"
Kinds = 2
Type = 5
Reloadanirate = -0.5
";
        let text = text.replace(
            "[Weapon5]
",
            "[Weapon5]
ModName0 = \"ext\"
ModModel0 = 0
",
        );
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(std::sync::Arc::new(WeaponTable::from_doc(&ButeDoc::parse(
                &text,
            )))),
            ..ArmsConfig::default()
        });
        a.on_grant(&WeaponGrant {
            parts: [1, -1, 0, -1],
            ..grant(0, 5, 1, 9.0)
        });
        a.on_life(0, 1);
        assert_eq!(a.anim_rate(5, false), 0.5);
        run(&mut a, 0, 520, |_| fire_cmd(false), &[]);
        a.clips.insert(
            5,
            Some((
                "test".into(),
                WeaponClips {
                    reload: Some(ClipFact {
                        index: 7,
                        len_ms: 1000,
                        marks: vec![],
                    }),
                    ..WeaponClips::default()
                },
            )),
        );
        let v = run(&mut a, 528, 1200, |_| fire_cmd(true), &[]);
        let reload_at = v
            .iter()
            .find(|m| m.1.shell == up::RELOAD)
            .expect("reload")
            .0;
        assert_eq!(a.phase(), Phase::Reload);
        let start = a.phase_at;
        // the phase lasts 2000 ms; the clip time is half the elapsed time and never passes the clip's length
        for (dt, want) in [
            (0u32, 0.0f32),
            (500, 250.0),
            (1000, 500.0),
            (1500, 750.0),
            (1990, 995.0),
        ] {
            run(
                &mut a,
                1200.max(reload_at),
                start + dt,
                |_| fire_cmd(false),
                &[],
            );
            let (_, idx, t) = a.view_clip(start + dt).expect("clip");
            assert_eq!(idx, 7);
            assert!((t - want).abs() < 1.0, "dt {dt}: {t} vs {want}");
            assert!(t <= 1000.0);
        }
    }

    #[test]
    fn single_shot_weapon_fires_once_per_press_with_one_message_per_pellet() {
        let mut a = arsenal();
        a.on_grant(&grant(0, 3, 0, 9.0));
        a.on_life(0, 1);
        run(&mut a, 0, 520, |_| fire_cmd(false), &[]);
        // held for 600 ms: one shot of 4 pellets
        let v = run(&mut a, 528, 1128, |_| fire_cmd(true), &[]);
        let fire: Vec<FireReport> = v
            .iter()
            .filter(|m| m.1.shell == up::FIRE)
            .map(|m| read_fire(&mut m.1.body.reader()).unwrap())
            .collect();
        assert_eq!(fire.len(), 4);
        assert_eq!(fire.iter().filter(|r| r.last).count(), 1);
        assert!(fire[3].last);
        // a miss: 623 bits, no object block; the four directions differ (spread 10..50 mrad)
        assert!(fire.iter().all(|r| r.hit.is_none() && r.extra.is_none()));
        assert!(fire.windows(2).all(|w| w[0].dir != w[1].dir));
        for r in &fire {
            let dz = r.dir[2];
            assert!(
                dz > (0.012f32).cos() - 1e-4,
                "within the 10 mrad cone at spread 0: {:?}",
                r.dir
            );
        }
        // release, press again: the second shot
        run(&mut a, 1128, 1200, |_| fire_cmd(false), &[]);
        let v = run(&mut a, 1200, 1300, |_| fire_cmd(true), &[]);
        assert_eq!(v.iter().filter(|m| m.1.shell == up::FIRE).count(), 4);
        // sprinting: the fire key does not count
        let mut sprint = aim();
        sprint.control |= dc_sim::player::bits::SPRINT;
        run(&mut a, 1300, 1400, |_| fire_cmd(false), &[]);
        let out = a.step(1400, &fire_cmd(true), &sprint, &[], None);
        assert!(out.iter().all(|m| m.shell != up::FIRE));
        assert!(!a.trigger_flags(1500));
    }

    #[test]
    fn throwable_hold_release_and_switch_back() {
        let mut a = arsenal();
        a.on_grant(&grant(0, 5, 1, 9.0));
        a.on_grant(&grant(3, 9, 2, 1.0));
        a.on_life(0, 1);
        run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
        a.step(600, &pick(3), &aim(), &[], None);
        run(&mut a, 616, 1200, |_| fire_cmd(false), &[]);
        // held from 1200 to 2400: the wind-up (sound 10), then a 0x4d every 67 ms; the release 133 ms into the
        // release clip, then the switch back to slot 0 in the same moment
        let v = run(&mut a, 1200, 3200, |t| fire_cmd(t < 2400), &[]);
        let s = shells(&v);
        let snd = v
            .iter()
            .find(|m| m.1.shell == up::WEAPON_SOUND)
            .expect("sound 10");
        assert_eq!(wire(&snd.1)[2], 10);
        assert!((1700..=1760).contains(&snd.0), "{}", snd.0);
        let holds: Vec<u32> = v
            .iter()
            .filter(|m| m.1.shell == up::THROW_HOLD)
            .map(|m| m.0)
            .collect();
        assert!(holds.len() >= 8, "{holds:?}");
        let first = v.iter().find(|m| m.1.shell == up::THROW_HOLD).unwrap();
        assert_eq!(hex(&wire(&first.1)), "0b4d");
        // one pass per 67 ms on average, each sampled at a 16 ms frame (the real client: 62-78 ms at its frames)
        let mean = (holds[holds.len() - 1] - holds[0]) / (holds.len() as u32 - 1);
        assert!((60..=75).contains(&mean), "{holds:?}");
        for w in holds.windows(2) {
            assert!((40..=90).contains(&(w[1] - w[0])), "{holds:?}");
        }
        let rel = v
            .iter()
            .position(|m| m.1.shell == up::FIRE)
            .expect("release");
        let r = read_fire(&mut v[rel].1.body.reader()).unwrap();
        assert_eq!((r.weapon_id, r.ammo_id, r.last), (9, 2, true));
        assert!(r.hit.is_none() && r.extra.is_none());
        assert_eq!(bits(&v[rel].1), 623);
        let last_hold = *holds.last().unwrap();
        let gap = v[rel].0 - last_hold;
        assert!(
            (140..=260).contains(&gap),
            "release {gap} ms after the last hold"
        );
        assert_eq!(&s[rel + 1..rel + 3], &[up::WEAPON_EVENT, up::WEAPON_CHANGE]);
        assert_eq!(v[rel + 2].0, v[rel].0);
        assert_eq!(a.held().map(|c| c.slot), Some(0));
    }

    /// The user's 2026-10-27 report: running / jumping with the pin pulled threw the grenade before the key went up.
    /// The fire key stays down 3 s while the control word sprints, jumps and runs; the release comes only after the
    /// key goes up (tdm4: the hold went on through a sprint, 0x4d paused, the release after the key).
    #[test]
    fn a_held_grenade_is_thrown_only_when_the_key_goes_up_while_running_and_jumping() {
        use dc_sim::player::bits;
        let mut a = arsenal();
        a.on_grant(&grant(0, 5, 1, 9.0));
        a.on_grant(&grant(3, 9, 2, 1.0));
        a.on_life(0, 1);
        run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
        a.step(600, &pick(3), &aim(), &[], None);
        run(&mut a, 616, 1200, |_| fire_cmd(false), &[]);
        // the key goes down standing (the wind-up starts), then 3 s of run / sprint / jump bursts with the key held
        let control = |t: u32| -> u32 {
            let base = 0x200 | bits::FWD | bits::RUN;
            match (t / 250) % 4 {
                0 => 0x200,
                1 => base | bits::SPRINT,
                2 => base | bits::SPRINT | bits::JUMP,
                _ => base | bits::JUMP,
            }
        };
        let mut v = Vec::new();
        let mut t = 1200;
        while t < 5000 {
            let mut am = aim();
            if t >= 1300 {
                am.control = control(t);
            }
            let held = t < 4300;
            for m in a.step(t, &fire_cmd(held), &am, &[], None) {
                v.push((t, m));
            }
            t += 16;
        }
        let rel: Vec<u32> = v
            .iter()
            .filter(|m| m.1.shell == up::FIRE)
            .map(|m| m.0)
            .collect();
        assert_eq!(rel.len(), 1, "one release: {rel:?}");
        assert!(
            rel[0] >= 4300,
            "thrown at {} ms with the key still down",
            rel[0]
        );
        // no 0x4d while sprinting, some while not
        let holds: Vec<u32> = v
            .iter()
            .filter(|m| m.1.shell == up::THROW_HOLD)
            .map(|m| m.0)
            .collect();
        assert!(holds.len() >= 4, "{holds:?}");
        assert!(
            holds.iter().all(|&h| control(h) & bits::SPRINT == 0),
            "{holds:?}"
        );
        assert_eq!(a.stats.throws, 1);
    }

    /// CLI-676: the pin pulled (wind-up, then the hold with the key down), a slot key cancels the throw: no release
    /// 0x82, the grenade count unchanged, the chosen weapon drawn (select pair), and the key still held does not fire
    /// the new weapon until it went up.
    #[test]
    fn a_switch_while_the_pin_is_pulled_keeps_the_grenade_and_draws_the_weapon() {
        let mut a = arsenal();
        a.on_grant(&grant(0, 5, 1, 9.0));
        a.on_grant(&grant(3, 9, 2, 1.0));
        a.on_life(0, 1);
        run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
        a.step(600, &pick(3), &aim(), &[], None);
        run(&mut a, 616, 1200, |_| fire_cmd(false), &[]);
        let before = a.slots[&3];
        let mut v = Vec::new();
        let mut t = 1200;
        while t < 2400 {
            v.extend(a.step(t, &fire_cmd(true), &aim(), &[], None));
            t += 16;
        }
        assert!(
            v.iter().any(|m| m.shell == up::THROW_HOLD),
            "the hold ran (pin pulled)"
        );
        assert_eq!(a.phase, Phase::ThrowHold);
        // the slot key with the fire key still down, then the key held a while, then up
        let mut after = Vec::new();
        let sw = WeaponCommands {
            fire: true,
            select: Some(0),
            ..WeaponCommands::default()
        };
        after.extend(a.step(t, &sw, &aim(), &[], None));
        while t < 4000 {
            t += 16;
            after.extend(a.step(t, &fire_cmd(t < 3500), &aim(), &[], None));
        }
        assert!(
            !after.iter().any(|m| m.shell == up::FIRE),
            "nothing thrown, nothing fired with the held key"
        );
        assert_eq!(a.stats.throws, 0);
        assert_eq!(a.stats.throw_cancels, 1);
        assert_eq!(a.held, Some(0), "the weapon of slot 0 is drawn");
        assert!(after.iter().any(|m| m.shell == up::WEAPON_CHANGE));
        assert_eq!(a.slots[&3].total(), before.total(), "grenade kept");
        // a new press fires the drawn weapon
        let shots = a.stats.shots;
        run(&mut a, 4016, 4400, |_| fire_cmd(true), &[]);
        assert!(a.stats.shots > shots);
    }

    /// A made-up table with an order (numbers and names are ours): class 1 = weapons 5 and 6, class 2 = weapon 3,
    /// class 3 = the melee weapon 8, class 4 = the throwable 9 (`HideWhenEmpty`, two per pack).
    /// CLI-1530: the plant hold plays its clip's sound keys once each, in the windows the frames cover, and never
    /// the fire key; the defuse (looped) plays them again on every lap.
    #[test]
    fn the_hold_plays_the_plant_clip_sound_keys() {
        let marks = [
            (356, Mark::Sound(1)),
            (1644, Mark::Sound(2)),
            (2534, Mark::Sound(3)),
            (2603, Mark::Sound(3)),
            (3904, Mark::Fire),
        ];
        let mut got = Vec::new();
        let mut done = None;
        for t in (0..=6000).step_by(16) {
            got.extend(hold_marks_due(&marks, 4000, done, t, false));
            done = Some(t);
        }
        assert_eq!(
            got,
            [1, 2, 3, 3],
            "each key once, no fire key, no second lap"
        );
        assert!(hold_marks_due(&marks, 4000, Some(400), 1600, false).is_empty());
        assert_eq!(hold_marks_due(&marks, 4000, None, 400, false), [1]);
        // looped (a defuse): the second lap's keys at 4356 .. 6603
        assert_eq!(
            hold_marks_due(&marks, 4000, Some(3999), 6700, true),
            [1, 2, 3, 3]
        );
        assert!(
            hold_marks_due(&marks, 0, None, 9000, true).len() == 4,
            "no clip length: one lap"
        );
    }

    /// CLI-1531: the bomb row is the one granted in the bomb slot, else the first weapon of the order's last class.
    #[test]
    fn the_bomb_row_comes_from_the_grant_or_the_last_order_class() {
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(ordered_table()),
            ..ArmsConfig::default()
        });
        assert_eq!(a.bomb_row(), Some(9), "the last class of the order");
        a.on_grant(&grant(crate::hud::bomb::BOMB_SLOT, 6, 1, 1.0));
        assert_eq!(a.bomb_row(), Some(6), "the granted bomb");
        assert!(Arsenal::new(ArmsConfig::default()).bomb_row().is_none());
    }

    fn ordered_table() -> std::sync::Arc<WeaponTable> {
        let text = ordered_text().join("\n");
        std::sync::Arc::new(WeaponTable::from_doc(&ButeDoc::parse(&text)))
    }

    fn ordered_text() -> Vec<String> {
        [
            "[WeaponOrder]",
            "Class0 = \"P\"",
            "P0 = \"w5\"",
            "P1 = \"w6\"",
            "Class1 = \"S\"",
            "S0 = \"w3\"",
            "Class2 = \"M\"",
            "M0 = \"w8\"",
            "Class3 = \"T\"",
            "T0 = \"w9\"",
            "[Ammo1]",
            "Name = \"aut\"",
            "MaxAmount = 9",
            "[Ammo2]",
            "Name = \"thr\"",
            "MaxAmount = 2",
            "[Weapon3]",
            "Name = \"w3\"",
            "AmmoName0 = \"aut\"",
            "ShotsPerClip = 3",
            "[Weapon5]",
            "Name = \"w5\"",
            "AmmoName0 = \"aut\"",
            "ShotsPerClip = 3",
            "HideWhenEmpty = 0",
            "[Weapon6]",
            "Name = \"w6\"",
            "AmmoName0 = \"aut\"",
            "ShotsPerClip = 3",
            "[Weapon8]",
            "Name = \"w8\"",
            "InfiniteAmmo = 1",
            "[Weapon9]",
            "Name = \"w9\"",
            "AmmoName0 = \"thr\"",
            "ShotsPerClip = 1",
            "ClientWeaponType = 2",
            "HideWhenEmpty = 1",
        ]
        .map(String::from)
        .to_vec()
    }

    fn throw_once(a: &mut Arsenal, from: u32) -> Vec<(u32, Outgoing)> {
        run(a, from, from + 2400, |t| fire_cmd(t < from + 900), &[])
    }

    #[test]
    fn a_variant_switches_as_its_base_gun_and_leaves_the_table_alone() {
        // VAR-1: a variant carries its base gun's name; the base gun keeps its entry
        let t = WeaponTable::from_doc(&ButeDoc::parse(concat!(
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"G\"\nClass1 = \"K\"\nK0 = \"Knife\"\n",
            "[Weapon5]\nName = \"G\"\n[Weapon6]\nName = \"Knife\"\n",
            "[Weapon500]\nName = \"G\"\nvariant_of = 5\n",
        )));
        assert_eq!(t.commands, vec![5, 6]);
        assert_eq!(t.order_class(500), t.order_class(5));
        assert_eq!(t.order_class(500), 1);
        assert_eq!(t.base_of(500), 5);
        assert_eq!(t.base_of(5), 5);
    }

    #[test]
    fn order_classes_and_the_empty_keys_come_from_the_table() {
        let t = ordered_table();
        assert_eq!(
            [5, 6, 3, 8, 9, 4].map(|w| t.order_class(w)),
            [1, 1, 2, 3, 4, 0]
        );
        assert_eq!(t.get(9).unwrap().hide_when_empty, Some(true));
        assert_eq!(t.get(5).unwrap().hide_when_empty, Some(false));
        assert_eq!(t.get(6).unwrap().hide_when_empty, None);
        assert!(t.get(8).unwrap().infinite_ammo && !t.get(9).unwrap().infinite_ammo);
    }

    #[test]
    fn the_last_grenade_switches_to_the_primary_drawn_last_and_stays_holstered() {
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(ordered_table()),
            ..ArmsConfig::default()
        });
        a.on_grant(&grant(0, 5, 1, 9.0));
        a.on_grant(&grant(1, 3, 1, 9.0));
        a.on_grant(&grant(2, 8, 0, 1000.0));
        a.on_grant(&grant(3, 9, 2, 1.0));
        a.on_grant(&grant(4, 6, 1, 9.0));
        a.on_life(0, 1);
        run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
        assert_eq!(a.held().map(|c| c.slot), Some(0));
        // the backpack primary, then the melee weapon, then the grenade (as rust6 at 271.2 / 273.6 s, melee added)
        for (t, s) in [(600, 4), (1400, 2), (2200, 3)] {
            a.step(t, &pick(s), &aim(), &[], None);
            run(&mut a, t + 16, t + 800, |_| fire_cmd(false), &[]);
            assert_eq!(a.held().map(|c| c.slot), Some(s));
        }
        let v = throw_once(&mut a, 3000);
        let rel = v
            .iter()
            .position(|m| m.1.shell == up::FIRE)
            .expect("the release");
        let s = shells(&v);
        // the empty switch in the same moment: to the class-1 slot drawn last (4), not slot 0 and not the melee
        // weapon drawn just before the throwable
        assert_eq!(&s[rel + 1..rel + 3], &[up::WEAPON_EVENT, up::WEAPON_CHANGE]);
        assert_eq!(v[rel + 2].0, v[rel].0);
        assert_eq!(hex(&wire(&v[rel + 2].1)), "0b7f0406000100");
        assert_eq!(a.held().map(|c| (c.slot, c.weapon)), Some((4, 6)));
        assert!(!s.contains(&up::RELOAD), "no reload without a round");
        // the empty throwable cannot be drawn again: its key and the wheel pass it by
        assert!(a.step(5600, &pick(3), &aim(), &[], None).is_empty());
        assert_eq!(a.held().map(|c| c.slot), Some(4));
        assert!(!a.may_draw(3));
        a.step(5700, &pick(2), &aim(), &[], None);
        run(&mut a, 5716, 6500, |_| fire_cmd(false), &[]);
        let wheel = WeaponCommands {
            wheel: 1,
            ..WeaponCommands::default()
        };
        let out = a.step(6500, &wheel, &aim(), &[], None);
        assert_eq!(hex(&wire(&out[1])), "0b7f0406000100", "slot 3 skipped");
        // the melee weapon (InfiniteAmmo) and the empty class-1 / 2 guns may still be drawn
        a.on_ammo(1, 3, 0.0);
        assert!(a.may_draw(1) && a.may_draw(2));
        // the next life's grant loads it again
        a.on_life(7000, 3);
        a.on_grant(&grant(3, 9, 2, 1.0));
        a.on_life(8000, 1);
        assert!(a.may_draw(3));
    }

    #[test]
    fn a_two_pack_reloads_after_the_first_throw_and_keeps_the_grenade() {
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(ordered_table()),
            ..ArmsConfig::default()
        });
        a.on_grant(&grant(0, 5, 1, 9.0));
        a.on_grant(&grant(3, 9, 2, 2.0));
        a.on_life(0, 1);
        run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
        a.step(600, &pick(3), &aim(), &[], None);
        run(&mut a, 616, 1400, |_| fire_cmd(false), &[]);
        assert_eq!(a.held().map(|c| (c.clip, c.stock)), Some((1, 1)));
        let v = throw_once(&mut a, 1400);
        let s = shells(&v);
        let rel = s.iter().position(|x| *x == up::FIRE).expect("release 1");
        // 0x83 with the ammunition id in the block of the release (rust6: the 2009 client), no switch
        assert_eq!(s[rel + 1], up::RELOAD);
        assert_eq!(v[rel + 1].0, v[rel].0);
        assert_eq!(hex(&wire(&v[rel + 1].1)), "0b830200");
        assert!(!s.contains(&up::WEAPON_CHANGE), "{s:?}");
        let ev = v
            .iter()
            .find(|m| m.1.shell == up::WEAPON_EVENT)
            .expect("the reload event");
        assert_eq!(hex(&wire(&ev.1)), "0b76030101");
        // the reload clip (the 2000 ms stand-in without a ROM) refills the clip from the stock
        run(&mut a, 3800, 5000, |_| fire_cmd(false), &[]);
        assert_eq!(a.held().map(|c| (c.slot, c.clip, c.stock)), Some((3, 1, 0)));
        // the server's count after the first throw (1.0) leaves the reloaded grenade in the hand
        a.on_ammo(3, 9, 1.0);
        assert_eq!(a.held().map(|c| (c.clip, c.stock)), Some((1, 0)));
        // the second (last) one: the empty switch to slot 0
        let v = throw_once(&mut a, 5000);
        let s = shells(&v);
        let rel = s.iter().position(|x| *x == up::FIRE).expect("release 2");
        assert_eq!(&s[rel + 1..rel + 3], &[up::WEAPON_EVENT, up::WEAPON_CHANGE]);
        assert_eq!(hex(&wire(&v[rel + 2].1)), "0b7f0005000100");
        assert_eq!(a.held().map(|c| c.slot), Some(0));
        assert_eq!(a.stats.throws, 2);
    }

    #[test]
    fn server_ammo_and_reload_key() {
        let mut a = arsenal();
        a.on_grant(&grant(0, 5, 1, 9.0));
        a.on_life(0, 1);
        run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
        let reload = WeaponCommands {
            reload: true,
            ..WeaponCommands::default()
        };
        // a full clip: R does nothing
        assert!(a.step(600, &reload, &aim(), &[], None).is_empty());
        run(&mut a, 616, 640, |_| fire_cmd(true), &[]);
        run(&mut a, 640, 700, |_| fire_cmd(false), &[]);
        assert_eq!(a.held().map(|c| c.clip), Some(2));
        let out = a.step(720, &reload, &aim(), &[], None);
        assert_eq!(
            out.iter().map(|m| m.shell).collect::<Vec<_>>(),
            vec![up::RELOAD]
        );
        // 0x66 kind 1: the slot's count
        a.on_ammo(0, 5, 2.0);
        assert_eq!(a.held().map(|c| (c.clip, c.stock)), Some((2, 0)));
        // 0xb4 removes the held weapon; a new 0xb3 draws its slot
        a.on_remove(0);
        assert!(a.held().is_none());
        a.on_grant(&WeaponGrant {
            clip: Some(3),
            equip: true,
            ..grant(1, 3, 0, 6.0)
        });
        let out = a.step(3000, &fire_cmd(false), &aim(), &[], None);
        assert_eq!(hex(&wire(&out[1])), "0b7f0103000000");
        assert_eq!(a.held().map(|c| (c.clip, c.stock)), Some((3, 6)));
    }

    #[test]
    fn server_weapon_switch_draws_the_named_slot() {
        let text = [
            "[WeaponOrder]",
            "Class0 = \"Alpha\"",
            "Alpha0 = \"w3\"",
            "Alpha1 = \"w5\"",
            "[Weapon3]",
            "Name = \"w3\"",
            "ShotsPerClip = 3",
            "[Weapon5]",
            "Name = \"w5\"",
            "ShotsPerClip = 3",
        ]
        .join("\n");
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(std::sync::Arc::new(WeaponTable::from_doc(&ButeDoc::parse(
                &text,
            )))),
            ..ArmsConfig::default()
        });
        assert_eq!(a.cfg.table.as_ref().unwrap().commands, vec![3, 5]);
        a.on_grant(&grant(0, 3, 0, 6.0));
        a.on_grant(&grant(2, 5, 0, 6.0));
        a.on_life(0, 1);
        // 0x15 = the second entry (weapon 5): select its slot with the key press' pair
        a.switch_command(0x15, true, -1.0);
        let out = a.step(100, &fire_cmd(false), &aim(), &[], None);
        assert_eq!(hex(&wire(&out[0])), "0b760302");
        assert_eq!(hex(&wire(&out[1])), "0b7f0205000000");
        assert_eq!(a.held().map(|c| c.slot), Some(2));
        // auto 0, an unknown command and a weapon not carried change nothing
        a.switch_command(0x14, false, -1.0);
        a.switch_command(0x7e, true, -1.0);
        let out = a.step(200, &fire_cmd(false), &aim(), &[], None);
        assert!(out.is_empty());
        assert_eq!(a.held().map(|c| c.slot), Some(2));
    }

    /// CLI-1627: in a no-recoil room the shotgun's pellets all leave along the aim and the view takes no kick;
    /// without it the same burst spreads and kicks.
    #[test]
    fn no_recoil_fires_along_the_aim_without_a_kick() {
        let fire = |no_recoil: bool| {
            let mut a = Arsenal::new(ArmsConfig {
                table: Some(table()),
                no_recoil,
                ..ArmsConfig::default()
            });
            a.on_grant(&grant(0, 3, 0, 9.0));
            a.on_life(0, 1);
            run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
            let mut kick = 0.0f32;
            let mut t = 600;
            while t < 2000 {
                a.step(t, &fire_cmd(t < 1500), &aim(), &[], None);
                kick = kick.max(a.view_pitch.abs()).max(a.view_yaw.abs());
                t += 16;
            }
            let dirs: Vec<[f32; 3]> = a.shot_fx.iter().map(|s| s.dir).collect();
            (dirs, kick, a.shot_counter.value)
        };
        let fwd = dc_botclient::shooter::forward(0.0, 0.0);
        let near = |d: &[f32; 3]| (0..3).all(|i| (d[i] - fwd[i]).abs() < 1e-5);
        let (dirs, kick, counter) = fire(true);
        assert!(dirs.len() >= 4, "{dirs:?}");
        assert!(dirs.iter().all(near), "{dirs:?}");
        assert_eq!(kick, 0.0);
        let (spread, kicked, counted) = fire(false);
        assert!(!spread.iter().all(near));
        assert!(kicked > 0.0);
        assert_eq!(counter, counted, "the table's draws run either way");
    }

    /// CLI-1605: in a Sniper Only room (weapon limit 6) every server grant is carried and drawable (the server
    /// decides); only the lobby fallback skips the pistol. Weapon types (ours): 3 sniper rifle (2), 5 pistol (4),
    /// 9 grenade (6).
    #[test]
    fn sniper_only_refuses_the_pistol_slot() {
        let cfg = |lobby: Vec<i32>| ArmsConfig {
            table: Some(table()),
            lobby_slots: lobby,
            arms_limit: dc_bute::arms::SNIPER_ONLY,
            weapon_types: std::sync::Arc::new([(3, 2), (5, 4), (9, 6)].into()),
            ..ArmsConfig::default()
        };
        let mut a = Arsenal::new(cfg(Vec::new()));
        a.on_grant(&grant(0, 3, 0, 6.0));
        a.on_grant(&grant(1, 5, 1, 6.0));
        a.on_life(0, 1);
        assert_eq!(a.slots.keys().copied().collect::<Vec<_>>(), vec![0, 1]);
        assert!(a.may_draw(1), "a granted pistol is drawable");
        assert!(a.barred(1, 5) && a.barred(3, 9) && !a.barred(0, 3));
        assert!(
            !a.barred(crate::hud::bomb::BOMB_SLOT, 9),
            "mode items are the mode's"
        );
        // the lobby fallback without grants skips the pistol
        let mut a = Arsenal::new(cfg(vec![3, 5]));
        a.on_life(0, 1);
        a.step(GRANT_WAIT_MS, &fire_cmd(false), &aim(), &[], None);
        assert_eq!(a.slots.keys().copied().collect::<Vec<_>>(), vec![0]);
        // without a limit the same grants are all carried
        let mut a = Arsenal::new(ArmsConfig {
            arms_limit: 0,
            ..cfg(Vec::new())
        });
        a.on_grant(&grant(1, 5, 1, 6.0));
        assert!(a.may_draw(1));
    }

    /// CLI-1605: a mode weapon granted into slot 0..3 outside the limit is carried and can be drawn.
    #[test]
    fn a_limited_room_still_carries_a_mode_grant() {
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(table()),
            arms_limit: dc_bute::arms::SNIPER_ONLY,
            weapon_types: std::sync::Arc::new([(3, 2), (9, 6)].into()),
            ..ArmsConfig::default()
        });
        a.on_grant(&WeaponGrant {
            equip: true,
            ..grant(2, 9, 2, 2.0)
        });
        a.on_life(0, 1);
        assert!(a.slots.contains_key(&2));
        assert!(a.may_draw(2));
        a.step(100, &fire_cmd(false), &aim(), &[], None);
        assert_eq!(a.held().map(|c| c.weapon), Some(9));
    }

    #[test]
    fn lobby_slots_without_grants() {
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(table()),
            lobby_slots: vec![5, -1, 3],
            ..ArmsConfig::default()
        });
        a.on_life(0, 1);
        assert!(a.step(100, &fire_cmd(false), &aim(), &[], None).is_empty());
        let out = a.step(GRANT_WAIT_MS, &fire_cmd(false), &aim(), &[], None);
        assert_eq!(hex(&wire(&out[1])), "0b7f0005000100");
        assert_eq!(
            a.held().map(|c| (c.source, c.clip, c.stock)),
            Some(("lobby", 3, 6))
        );
        assert_eq!(a.slots.keys().copied().collect::<Vec<_>>(), vec![0, 2]);
    }

    #[test]
    fn rays_hit_boxes_unless_the_world_is_in_front() {
        let bytes = crate::testkit::synth_world_bytes();
        let w = dc_world::World::parse(&bytes).unwrap();
        let cw =
            dc_sim::from_world::collision_world(&w, &dc_sim::from_world::WorldFilter::default());
        // the course's wall stands across z = 1500
        let near = Target {
            obj: 1,
            centre: [0.0, 60.0, 1300.0],
            half: [16.0, 40.0, 16.0],
        };
        let far = Target {
            obj: 2,
            centre: [0.0, 60.0, 1800.0],
            half: [16.0, 40.0, 16.0],
        };
        let o = [0.0, 60.0, 1000.0];
        let d = [0.0, 0.0, 1.0];
        assert_eq!(
            trace(o, d, 5000.0, &[far, near], Some(&cw)),
            RayHit::Character {
                obj: 1,
                dist: 284.0,
                node: crate::hitnodes::NO_NODE,
                normal: HIT_VECTOR,
            }
        );
        assert_eq!(trace(o, d, 5000.0, &[far], Some(&cw)), RayHit::Miss);
        assert_eq!(
            trace(o, d, 5000.0, &[far], None),
            RayHit::Character {
                obj: 2,
                dist: 784.0,
                node: crate::hitnodes::NO_NODE,
                normal: HIT_VECTOR,
            }
        );
        assert_eq!(
            trace(o, d, 200.0, &[near], None),
            RayHit::Miss,
            "out of range"
        );
        let wd = world_hit(&cw, o, d, 5000.0).unwrap();
        assert!((wd - 500.0).abs() < 1.0, "{wd}");
    }

    /// ROM-gated: the weapons the captures threw (7, the 2-pack 143) leave the hand when empty and are outside the
    /// two classes drawn without a round; the primary 145 is class 1 (the class the 2009 client's empty switch
    /// drew in rust6). Counts only are printed.
    #[test]
    fn rom_throwables_hide_when_empty_and_need_a_round() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let t = WeaponTable::open(&rom).expect("weapon table");
        let hiding: Vec<u16> = t
            .rows
            .iter()
            .filter(|(_, r)| r.hide_when_empty == Some(true))
            .map(|(w, _)| *w)
            .collect();
        let mut by_class: BTreeMap<u8, usize> = BTreeMap::new();
        for w in &hiding {
            *by_class.entry(t.order_class(*w)).or_default() += 1;
        }
        println!(
            "rows {}, HideWhenEmpty set on {} (by order class {:?}), InfiniteAmmo on {}",
            t.rows.len(),
            hiding.len(),
            by_class,
            t.rows.values().filter(|r| r.infinite_ammo).count()
        );
        for w in [7u16, 143] {
            assert!(hiding.contains(&w), "weapon {w} hides when empty");
            assert!(
                !DRAW_EMPTY_CLASSES.contains(&t.order_class(w)),
                "weapon {w}"
            );
        }
        assert!(!hiding.contains(&145));
        assert_eq!(t.order_class(145), 1);
    }

    /// ROM-gated: the first-person clips of the weapons the captures fired give the timings the real client showed
    /// (asserted against the capture numbers, not the ROM's).
    #[test]
    fn rom_a_pistol_shot_plays_its_fire_clip_then_its_postfire_clip() {
        // arms-fixes2 (user 2026-10-27: "the pistol animation resets too fast"): the fire clip of the pistol 164
        // plays to its end, then its postfire clip (the 2009 firing state, cshell+0x238110); a press inside the fire
        // clip waits for its end, a press inside the postfire clip fires at once
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let t = WeaponTable::open(&rom).expect("weapon table");
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(std::sync::Arc::new(t)),
            rom: Some(rom),
            ..ArmsConfig::default()
        });
        a.on_grant(&grant(1, 164, 164, 45.0));
        a.on_life(0, 1);
        a.step(0, &pick(1), &aim(), &[], None);
        run(&mut a, 16, 3000, |_| fire_cmd(false), &[]);
        assert_eq!(a.held().map(|c| c.weapon), Some(164));
        assert_eq!(a.phase(), Phase::Idle);
        let c = a.clips_of(164).cloned().expect("pistol clips");
        let fire = c.fire.clone().expect("fire clip");
        let post = c.postfire.clone().expect("postfire clip");
        println!(
            "pistol fire {} ms, postfire {} ms",
            fire.len_ms, post.len_ms
        );
        assert!(post.len_ms > fire.len_ms);
        // one click at 3000: the fire clip for its whole length, then the postfire clip, then idle
        let t0 = 3000u32;
        let mut shots = Vec::new();
        let mut clips = Vec::new();
        let mut now = t0;
        while now < t0 + fire.len_ms + post.len_ms + 200 {
            let v = a.step(now, &fire_cmd(now < t0 + 30), &aim(), &[], None);
            shots.extend(v.iter().filter(|m| m.shell == up::FIRE).map(|_| now));
            clips.push((now, a.phase(), a.view_clip(now).map(|v| v.1)));
            now += 10;
        }
        assert_eq!(shots, vec![t0]);
        for (at, ph, clip) in &clips {
            let dt = at - t0;
            if dt < fire.len_ms {
                assert_eq!((*ph, *clip), (Phase::Fire, Some(fire.index)), "at +{dt} ms");
            } else if dt >= fire.len_ms + 10 && dt < fire.len_ms + post.len_ms {
                assert_eq!(
                    (*ph, *clip),
                    (Phase::PostFire, Some(post.index)),
                    "at +{dt} ms"
                );
            } else if dt >= fire.len_ms + post.len_ms + 10 {
                assert_eq!(*ph, Phase::Idle, "at +{dt} ms");
            }
        }
        // rapid clicks: one inside the fire clip (+40) does not restart it; one inside the postfire clip fires at once
        let t1 = now + 500;
        run(&mut a, now, t1, |_| fire_cmd(false), &[]);
        let click = |first: u32, at: u32| {
            move |x: u32| fire_cmd((first..first + 20).contains(&x) || (at..at + 20).contains(&x))
        };
        let early = t1 + 40;
        let v = run(&mut a, t1, t1 + 400, click(t1, early), &[]);
        let fired: Vec<u32> = v
            .iter()
            .filter(|m| m.1.shell == up::FIRE)
            .map(|m| m.0)
            .collect();
        assert_eq!(fired[0], t1);
        assert!(
            fired[1..].iter().all(|f| *f >= t1 + fire.len_ms),
            "a press inside the fire clip does not cut it short: {fired:?}"
        );
        let t2 = t1 + 2000;
        run(&mut a, t1 + 400, t2, |_| fire_cmd(false), &[]);
        let late = t2 + fire.len_ms + 100;
        let v = run(&mut a, t2, t2 + 600, click(t2, late), &[]);
        let fired: Vec<u32> = v
            .iter()
            .filter(|m| m.1.shell == up::FIRE)
            .map(|m| m.0)
            .collect();
        assert_eq!(fired.len(), 2, "{fired:?}");
        assert!(
            fired[1] >= late && fired[1] < late + 20,
            "the press inside the postfire clip fires at once: {fired:?}"
        );
    }

    #[test]
    fn rom_clips_match_the_observed_timing() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let t = WeaponTable::open(&rom).expect("weapon table");
        assert!(t.rows.len() > 100, "{} rows", t.rows.len());
        let vfs = RezVfs::new(&rom);
        let clips = |w: u16| {
            let p = t
                .get(w)
                .and_then(|r| r.pv_model.clone())
                .expect("first-person model");
            WeaponClips::load(&vfs, &p).expect("clips")
        };
        // OBSERVED: tdm1 weapon 145 bursts 104-110 ms apart; draw sound 3 0.67-0.73 s after its select
        let m4 = clips(145);
        let fire = m4.fire.as_ref().expect("fire clip");
        assert!((100..=112).contains(&fire.len_ms), "{}", fire.len_ms);
        assert!(fire.marks.contains(&(0, Mark::Fire)));
        let sel = m4.select.as_ref().unwrap();
        assert!(sel
            .marks
            .iter()
            .any(|(t, m)| *m == Mark::Sound(3) && (640..=740).contains(t)));
        // OBSERVED: three reload sounds 1, 2, 3 within about 1.8 s
        let rl = m4.reload.as_ref().unwrap();
        let s: Vec<u8> = rl
            .marks
            .iter()
            .filter_map(|m| match m.1 {
                Mark::Sound(n) => Some(n),
                _ => None,
            })
            .collect();
        assert_eq!(s, vec![1, 2, 3]);
        assert!(!m4.throwable());
        // CLI-1350: the knife (row 0, a melee row) has an alternate-fire clip longer than its fire clip, with one
        // alternate-fire key; the rifle has none
        let knife = clips(0);
        assert_eq!(t.get(0).and_then(|r| r.gun_type), Some(GUN_TYPE_MELEE));
        let alt = knife.alt_clip.as_ref().expect("knife alternate-fire clip");
        let fire = knife.fire.as_ref().expect("knife fire clip");
        assert!(alt.len_ms > fire.len_ms, "{} / {}", alt.len_ms, fire.len_ms);
        assert_eq!(alt.marks.iter().filter(|m| m.1 == Mark::Fire).count(), 1);
        assert!(m4.alt_clip.is_none());
        // OBSERVED: weapon 164's draw sound 0.25 s after the select
        let pistol = clips(164);
        assert!(pistol
            .select
            .unwrap()
            .marks
            .iter()
            .any(|(t, m)| *m == Mark::Sound(3) && (200..=300).contains(t)));
        // OBSERVED: the grenade: sound 10 about 0.19 s before the first 0x4d, 0x4d every 62-78 ms, the release
        // 0.2 s after the last 0x4d
        let g = clips(7);
        assert!(g.throwable());
        let pre = g.prefire.clone().unwrap();
        let (st, _) = pre
            .marks
            .iter()
            .find(|m| m.1 == Mark::Sound(10))
            .copied()
            .unwrap();
        assert!(
            (150..=250).contains(&(pre.len_ms - st)),
            "{} {}",
            pre.len_ms,
            st
        );
        let hold = g.fire.as_ref().unwrap().len_ms;
        assert!((60..=80).contains(&hold));
        let post = g.postfire.clone().unwrap();
        let (rt, _) = post
            .marks
            .iter()
            .find(|m| m.1 == Mark::Fire)
            .copied()
            .unwrap();
        assert!((150..=260).contains(&(hold + rt)), "{}", hold + rt);
        // OBSERVED: tdm1 shot_scale 1.0 for weapons 145 and 7, 1.2 for 164 (the rows' DynamicPerturbFactor)
        assert_eq!(t.get(145).unwrap().cone.scale, 1.0);
        assert_eq!(t.get(7).unwrap().cone.scale, 1.0);
        assert!((t.get(164).unwrap().cone.scale - 1.2).abs() < 1e-6);
        // the captures' ammunition ids: 145 -> 145, 164 -> 164, 7 -> 9, 0 -> 0
        for (w, ammo) in [(145u16, 145u16), (164, 164), (7, 9), (0, 0)] {
            assert_eq!(t.get(w).and_then(|r| r.ammo), Some(ammo), "weapon {w}");
        }
        println!(
            "ROM weapon table: {} rows; clip timings agree with tdm1 / tdm4",
            t.rows.len()
        );
    }

    #[test]
    fn muzzle_point_from_the_row_vectors_and_the_melee_form() {
        // made-up rows: weapon 1 with a model position and a muzzle point, weapon 2 with both zero (a melee row)
        let doc = ButeDoc::parse(
            "[Weapon1]
Pos = <0.5, -0.5, 1.0>
MuzzlePos = <4.0, -3.0, 8.0>
             [Weapon2]
Pos = <0.000000,0.000000,0.000000>
MuzzlePos = <0.000000,0.000000,0.000000>
",
        );
        let a = Arsenal::new(ArmsConfig {
            table: Some(std::sync::Arc::new(WeaponTable::from_doc(&doc))),
            ..ArmsConfig::default()
        });
        let at = |yaw: f32, pitch: f32| Aim {
            origin: [10.0, 20.0, 30.0],
            yaw,
            pitch,
            control: 0,
        };
        let p = a.muzzle_point(1, &at(0.0, 0.0));
        assert!(
            (p[0] - 14.5).abs() < 1e-4 && (p[1] - 16.5).abs() < 1e-4 && (p[2] - 39.0).abs() < 1e-4
        );
        // a quarter turn: forward is +X, right is -Z
        let q = a.muzzle_point(1, &at(std::f32::consts::FRAC_PI_2, 0.0));
        assert!(
            (q[0] - 19.0).abs() < 1e-3 && (q[2] - 25.5).abs() < 1e-3,
            "{q:?}"
        );
        assert_eq!(a.muzzle_point(2, &at(0.3, 0.2)), [10.0, 20.0, 30.0]);
        // no row vectors: the observed offset
        assert_eq!(
            a.muzzle_point(3, &at(0.0, 0.0)),
            weapon_pos([10.0, 20.0, 30.0], 0.0)
        );
    }

    // ------------------------------------------------------------------------------------------- w2-scope

    /// The ordered table plus a sight (mod 35: socket 1, two zoom levels), a silencer (mod 7: socket 0, level 2), a
    /// toggle sight (mod 12, Type 2) and a mod of no socket (mod 40); weapon 5 zooms in 0.25 s, weapon 6 is a
    /// `Guntype` 0xb rifle, weapon 3 a bolt-action one (numbers ours).
    fn scope_table() -> std::sync::Arc<WeaponTable> {
        let mut text = ordered_text();
        text.extend(
            [
                "[Mod35]",
                "Kinds = 1",
                "ZoomLevel = 2",
                "ZoomFov0 = 40",
                "ZoomFov1 = 20",
                "ZoomSpriteName = \"scope_pic\"",
                "ZoomInSound = \"zin.wav\"",
                "ZoomOutSound = \"zout.wav\"",
                "[Mod7]",
                "Kinds = 0",
                "SilencerLevel = 2",
                "[Mod12]",
                "Kinds = 1",
                "Type = 2",
                "ZoomLevel = 1",
                "ZoomFov0 = 40",
                "[Mod40]",
                "ZoomLevel = 1",
            ]
            .map(String::from),
        );
        let mut doc = text.join("\n");
        doc = doc.replace(
            "[Weapon5]\nName = \"w5\"",
            "[Weapon5]\nName = \"w5\"\nZoomTime = 0.25\nVectorsPerRound = 1",
        );
        doc = doc.replace(
            "[Weapon6]\nName = \"w6\"",
            &format!("[Weapon6]\nName = \"w6\"\n{} = 11", key::GUN_TYPE),
        );
        doc = doc.replace(
            "[Weapon3]\nName = \"w3\"",
            "[Weapon3]\nName = \"w3\"\nReloadAfterFire = 1",
        );
        std::sync::Arc::new(WeaponTable::from_doc(&ButeDoc::parse(&doc)))
    }

    fn scoped(parts: [i8; 4]) -> Arsenal {
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(scope_table()),
            ..ArmsConfig::default()
        });
        a.set_normal_fov_x(90f32.to_radians());
        a.on_grant(&WeaponGrant {
            parts,
            ..grant(0, 5, 1, 9.0)
        });
        a.on_grant(&grant(1, 3, 1, 9.0));
        a.on_grant(&grant(2, 8, 0, 1000.0));
        a.on_grant(&grant(3, 9, 2, 1.0));
        a.on_grant(&WeaponGrant {
            parts,
            class_pos: 1,
            ..grant(4, 6, 1, 9.0)
        });
        a.on_life(0, 1);
        run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
        a
    }

    fn zoom_cmd(zoom: bool) -> WeaponCommands {
        WeaponCommands {
            zoom,
            ..WeaponCommands::default()
        }
    }

    fn sprint_aim() -> Aim {
        Aim {
            control: 0x200 | dc_sim::player::bits::SPRINT,
            ..aim()
        }
    }

    /// CLI-981: an extended magazine (a socket-2 mod) adds its `ShotsPerClipPlus<k>` to the weapon's clip, `k`
    /// being the weapon row's `ModModel` for that mod; without it the row's clip stays.
    #[test]
    fn an_extended_magazine_makes_the_clip_bigger() {
        // made-up rows: weapon 5 (clip 3) lists the magazine mod 20 with ModModel 1
        let mut text = ordered_text();
        text.extend(
            [
                "[Mod20]",
                "Name = \"mag\"",
                "Kinds = 2",
                "ShotsPerClipPlus0 = 5",
                "ShotsPerClipPlus1 = 10",
            ]
            .map(String::from),
        );
        let doc = text.join("\n").replace(
            "[Weapon5]\nName = \"w5\"",
            "[Weapon5]\nName = \"w5\"\nModName0 = \"mag\"\nModModel0 = 1",
        );
        let table = std::sync::Arc::new(WeaponTable::from_doc(&ButeDoc::parse(&doc)));
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(table),
            ..ArmsConfig::default()
        });
        a.on_grant(&WeaponGrant {
            parts: [-1, -1, 20, -1],
            ..grant(0, 5, 1, 40.0)
        });
        let c = &a.slots[&0];
        assert_eq!((c.clip_size, c.clip, c.stock), (Some(13), 13, 27));
        assert_eq!(a.clip_bonus(5), 10);
        a.on_grant(&grant(1, 5, 1, 40.0));
        let c = &a.slots[&1];
        assert_eq!(
            (c.clip_size, c.clip),
            (Some(3), 3),
            "no magazine: the row's clip"
        );
    }

    /// CLI-982: while the round-end world camera is live the weapon takes no fire or zoom command and the aim mark is
    /// gone; when the camera goes off the weapon fires again.
    #[test]
    fn a_live_world_camera_holds_fire_zoom_and_the_aim_mark() {
        let mut a = scoped([35, -1, -1, -1]);
        assert!(a.aim_mark(600).is_some(), "aim mark while playing");
        // zoomed in before the end
        a.step(700, &zoom_cmd(true), &aim(), &[], None);
        a.step(720, &zoom_cmd(false), &aim(), &[], None);
        assert!(a.zoom_active());
        a.set_cinematic(true);
        assert!(!a.zoom_active(), "going live leaves the zoom");
        assert!(a.aim_mark(800).is_none());
        let before = a.stats.shots;
        let out = run(&mut a, 800, 2000, |_| fire_cmd(true), &[]);
        assert_eq!(a.stats.shots, before, "no shot under the camera");
        assert!(out.iter().all(|m| m.1.shell != 0x82), "no 0x82 sent");
        a.step(2000, &zoom_cmd(true), &aim(), &[], None);
        assert!(!a.zoom_active(), "no zoom under the camera");
        a.set_cinematic(false);
        run(&mut a, 2100, 3000, |_| fire_cmd(true), &[]);
        assert!(a.stats.shots > before, "fires again after the camera");
        assert!(a.aim_mark(3000).is_some());
    }

    #[test]
    fn grant_parts_are_filed_by_the_mod_rows_kinds() {
        // the sight first in the grant (DSV-80 order) still lands in socket 1; an unknown mod and a mod without a
        // socket are dropped; the finish byte is no mod
        let a = scoped([35, 7, 40, 99]);
        assert_eq!(a.mods_of(5), [Some(7), Some(35), None]);
        assert_eq!(a.silencer_level(5), Some(2));
        let none = scoped([-1; 4]);
        assert_eq!(none.mods_of(5), [None; 3]);
        assert_eq!(none.silencer_level(5), None);
        // a later grant of the weapon clears the sockets first
        let mut b = scoped([35, -1, -1, -1]);
        b.on_grant(&grant(0, 5, 1, 9.0));
        assert_eq!(b.mods_of(5), [None; 3]);
    }

    #[test]
    fn the_zoom_key_cycles_the_sight_levels_with_the_control_bit_fov_and_cone() {
        let mut a = scoped([35, -1, -1, -1]);
        assert!(!a.zoom_active());
        assert_eq!(a.zoom_control_bits(0x200), 0);
        // press: level 1, the change toward 40 degrees runs (0.25 s for the whole 90 -> 40)
        a.step(1000, &zoom_cmd(true), &aim(), &[], None);
        assert_eq!(a.scope().level(), 1);
        assert!(a.zoom_active() && a.scope().changing());
        assert_eq!(a.zoom_control_bits(0x200), crate::zoom::ZOOM_CONTROL_BIT);
        let sounds = a.take_zoom_sounds();
        assert_eq!(sounds.len(), 1);
        assert!(sounds[0].zoom_in && sounds[0].file == "zin.wav");
        // held: no second step; half way after 125 ms
        a.step(1125, &zoom_cmd(true), &aim(), &[], None);
        assert_eq!(a.scope().level(), 1);
        let half = a.zoom_fov_x(1125).unwrap().to_degrees();
        assert!((half - 65.0).abs() < 0.01, "{half}");
        a.step(1300, &zoom_cmd(false), &aim(), &[], None);
        assert_eq!(
            a.zoom_control_bits(0x200),
            0,
            "the bit only while the key is held"
        );
        assert!(!a.scope().changing());
        assert!((a.zoom_fov_x(1300).unwrap().to_degrees() - 40.0).abs() < 1e-3);
        assert!(a.scope().overlays()[crate::hud::scope::OVERLAY_SCOPE].is_some());
        // a shot while zoomed: the 0x82 zoom byte (`moving`) is set
        let v = run(&mut a, 1316, 1700, |t| fire_cmd(t < 1400), &[]);
        let shot = v
            .iter()
            .find(|m| m.1.shell == up::FIRE)
            .map(|m| read_fire(&mut m.1.body.reader()).unwrap())
            .expect("a shot");
        assert!(shot.moving, "zoomed shot carries the zoom byte");
        // level 2, then past ZoomLevel back to 0 (the change out still counts as zoomed)
        a.step(2000, &zoom_cmd(true), &aim(), &[], None);
        a.step(2016, &zoom_cmd(false), &aim(), &[], None);
        assert_eq!(a.scope().level(), 2);
        a.step(3000, &zoom_cmd(true), &aim(), &[], None);
        a.step(3016, &zoom_cmd(false), &aim(), &[], None);
        assert_eq!(a.scope().level(), 0);
        assert!(a.zoom_active());
        run(&mut a, 3032, 3600, |_| fire_cmd(false), &[]);
        assert!(!a.zoom_active() && a.zoom_fov_x(3600).is_none());
        assert!(a.scope().overlays().iter().all(Option::is_none));
        let v = run(&mut a, 3600, 4000, |t| fire_cmd(t < 3700), &[]);
        let shot = v
            .iter()
            .find(|m| m.1.shell == up::FIRE)
            .map(|m| read_fire(&mut m.1.body.reader()).unwrap())
            .expect("a shot");
        assert!(!shot.moving);
    }

    #[test]
    fn without_a_sight_the_key_sends_the_bit_but_never_zooms() {
        // OBSERVED: rust6 updates.csv (0x80000 in 13 updates, pu_flags 0x0800 never: the rifle had no sight)
        let mut a = scoped([-1; 4]);
        a.step(1000, &zoom_cmd(true), &aim(), &[], None);
        assert_eq!(a.zoom_control_bits(0x200), crate::zoom::ZOOM_CONTROL_BIT);
        assert!(!a.zoom_active());
        assert_eq!(a.scope().level(), 0);
        assert!(a.take_zoom_sounds().is_empty());
        // a toggle sight flips its flag and does not zoom
        let mut t = scoped([12, -1, -1, -1]);
        t.step(1000, &zoom_cmd(true), &aim(), &[], None);
        assert!(t.scope().toggled() && !t.zoom_active());
    }

    #[test]
    fn no_shot_until_the_sprint_exit_clip_ends() {
        // CLI-661: the fire key counts again only when the sprint-exit clip is done (cshell+0x23ac50)
        let clip = |len_ms| {
            Some(ClipFact {
                index: 0,
                len_ms,
                marks: Vec::new(),
            })
        };
        let fires = |a: &mut Arsenal, from: u32, to: u32, sprint: bool, fire: bool| {
            let mut n = Vec::new();
            let mut t = from;
            while t < to {
                let aim = if sprint { sprint_aim() } else { aim() };
                for m in a.step(t, &fire_cmd(fire), &aim, &[], None) {
                    if m.shell == up::FIRE {
                        n.push(t);
                    }
                }
                t += 16;
            }
            n
        };
        let mut a = arsenal();
        a.on_grant(&grant(0, 3, 0, 9.0));
        a.on_life(0, 1);
        run(&mut a, 0, 520, |_| fire_cmd(false), &[]);
        a.clips.insert(
            3,
            Some((
                "test".into(),
                WeaponClips {
                    sprint_enter: clip(200),
                    sprint_loop: clip(600),
                    sprint_exit: clip(300),
                    ..WeaponClips::default()
                },
            )),
        );
        // sprinting with the fire key held: no shot
        assert!(fires(&mut a, 528, 1200, true, true).is_empty());
        assert!(a.sprint_anim.active());
        // the sprint ends at 1200, the exit clip runs 1200 .. 1500: the held key does not fire
        assert!(fires(&mut a, 1200, 1488, false, true).is_empty());
        // after the exit clip the held key fires (the press counts from the first free frame)
        let v = fires(&mut a, 1504, 1600, false, true);
        assert_eq!(v.first(), Some(&1504), "{v:?}");
        // a weapon without an exit clip fires on the first frame after the sprint
        let mut b = arsenal();
        b.on_grant(&grant(0, 3, 0, 9.0));
        b.on_life(0, 1);
        run(&mut b, 0, 520, |_| fire_cmd(false), &[]);
        b.clips.insert(
            3,
            Some((
                "test".into(),
                WeaponClips {
                    sprint_enter: clip(200),
                    sprint_loop: clip(600),
                    ..WeaponClips::default()
                },
            )),
        );
        fires(&mut b, 528, 1200, true, false);
        assert_eq!(fires(&mut b, 1200, 1300, false, true).first(), Some(&1200));
    }

    #[test]
    fn sprinting_blocks_the_key_and_a_guntype_11_rifle_zooms_out() {
        let mut a = scoped([35, -1, -1, -1]);
        // a press while sprinting: no bit, no zoom
        a.step(1000, &zoom_cmd(true), &sprint_aim(), &[], None);
        assert_eq!(a.zoom_control_bits(sprint_aim().control), 0);
        assert!(!a.zoom_active());
        a.step(1016, &zoom_cmd(false), &aim(), &[], None);
        // weapon 5 (no Guntype) keeps its zoom when the sprint starts
        a.step(1100, &zoom_cmd(true), &aim(), &[], None);
        a.step(1500, &zoom_cmd(false), &aim(), &[], None);
        assert_eq!(a.scope().level(), 1);
        a.step(1600, &zoom_cmd(false), &sprint_aim(), &[], None);
        assert_eq!(a.scope().level(), 1);
        // the Guntype 11 rifle (slot 4) zooms out when the sprint starts
        let mut b = scoped([35, -1, -1, -1]);
        b.step(700, &pick(4), &aim(), &[], None);
        run(&mut b, 716, 1500, |_| fire_cmd(false), &[]);
        assert_eq!(b.held().map(|c| c.weapon), Some(6));
        b.step(1500, &zoom_cmd(true), &aim(), &[], None);
        b.step(2200, &zoom_cmd(false), &aim(), &[], None);
        assert_eq!(b.scope().level(), 1);
        b.step(2300, &zoom_cmd(false), &sprint_aim(), &[], None);
        assert_eq!(b.scope().level(), 0);
        assert!(b.scope().changing(), "a change out, not the instant reset");
    }

    #[test]
    fn reload_while_zoomed_zooms_out_first_and_a_weapon_change_resets_the_zoom() {
        let mut a = scoped([35, -1, -1, -1]);
        a.step(1000, &zoom_cmd(true), &aim(), &[], None);
        a.step(1400, &zoom_cmd(false), &aim(), &[], None);
        // one shot, so the clip is not full
        run(&mut a, 1416, 1800, |t| fire_cmd(t < 1450), &[]);
        assert!(a.scope().level() == 1);
        let r = WeaponCommands {
            reload: true,
            ..WeaponCommands::default()
        };
        let out = a.step(2000, &r, &aim(), &[], None);
        assert!(
            !shells(&out.iter().map(|o| (0, o.clone())).collect::<Vec<_>>()).contains(&up::RELOAD)
        );
        assert_eq!(a.scope().level(), 0);
        assert!(a.scope().changing());
        // the change out of 40 -> 90 at (90 - 20) / 0.25 = 280 degrees per second lasts about 179 ms: the 0x83 then
        let v = run(&mut a, 2016, 2600, |_| fire_cmd(false), &[]);
        let at = v
            .iter()
            .find(|m| m.1.shell == up::RELOAD)
            .map(|m| m.0)
            .expect("the reload after the zoom-out");
        assert!((2176..2240).contains(&at), "{at}");
        // a weapon change: the instant reset, with the zoom-out sound
        run(&mut a, 2600, 5000, |_| fire_cmd(false), &[]);
        assert_eq!(a.phase(), Phase::Idle);
        a.step(5000, &zoom_cmd(true), &aim(), &[], None);
        assert!(a.zoom_active());
        a.step(5016, &zoom_cmd(false), &aim(), &[], None);
        a.take_zoom_sounds();
        a.step(5100, &pick(1), &aim(), &[], None);
        assert!(!a.zoom_active() && a.zoom_fov_x(5100).is_none());
        let s = a.take_zoom_sounds();
        assert_eq!(s.len(), 1);
        assert!(!s[0].zoom_in);
    }

    #[test]
    fn the_shot_that_empties_a_zoomed_clip_zooms_out_before_the_reload() {
        // CLI-646: the empty-clip reload goes through the zoomed rule too (cshell+0x237180 -> cshell+0x1f6520)
        let mut a = scoped([35, -1, -1, -1]);
        a.step(1000, &zoom_cmd(true), &aim(), &[], None);
        a.step(1400, &zoom_cmd(false), &aim(), &[], None);
        assert_eq!(a.scope().level(), 1);
        let v = run(&mut a, 1416, 3000, |t| fire_cmd(t < 2900), &[]);
        let last_shot = v
            .iter()
            .filter(|m| m.1.shell == up::FIRE)
            .map(|m| m.0)
            .last()
            .expect("shots");
        let reload = v
            .iter()
            .find(|m| m.1.shell == up::RELOAD)
            .map(|m| m.0)
            .expect("the reload after the zoom-out");
        assert!(reload > last_shot, "no 0x83 in the block of the last shot");
        assert_eq!(a.scope().level(), 0);
        assert!(!a.zoom_active());
        assert!(
            a.scope().overlays().iter().all(Option::is_none),
            "no scope picture"
        );
        assert!(!a.scope().weapons_hidden());
    }

    #[test]
    fn rom_the_scoped_sniper_view_sways_and_the_unscoped_one_does_not() {
        // CLI-670: while the scope is up the view turns by the row's camera sway (cshell+0x1f2b70); the L96 (27,
        // CameraSwayXSpeed 4.5, YSpeed 1.1, XFreq 10, YFreq 10) with its scope (mod 27, no sway of its own)
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let t = WeaponTable::open(&rom).expect("weapon table");
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(std::sync::Arc::new(t)),
            rom: Some(rom.clone()),
            ..ArmsConfig::default()
        });
        a.set_normal_fov_x(90f32.to_radians());
        a.on_grant(&WeaponGrant {
            // mod 53 is a Fast Loader: the clip is the row's less its rounds (no grant clip: the client's own sum)
            parts: [27, 33, 53, -1],
            clip: None,
            equip: true,
            ..grant(0, 27, 27, 20.0)
        });
        a.on_life(0, 1);
        let sway = a.sway_of(27);
        assert_eq!(
            (sway.x_speed, sway.y_speed, sway.x_freq, sway.y_freq),
            (4.5, 1.1, 10.0, 10.0)
        );
        let (ax, ay) = sway.amplitude();
        assert!((ax - 0.0045).abs() < 1e-6 && (ay - 0.0011).abs() < 1e-6);
        // the swing of the summed view change over one period (2 pi / (10 x 0.1) s) and a bit
        let swing = |a: &mut Arsenal, from: u32| {
            let (mut y, mut p) = (0f32, 0f32);
            let (mut ylo, mut yhi, mut plo, mut phi) = (0f32, 0f32, 0f32, 0f32);
            let mut t = from;
            while t < from + 7000 {
                a.step(t, &fire_cmd(false), &aim(), &[], None);
                y += a.take_view_yaw();
                p += a.take_view_pitch();
                (ylo, yhi, plo, phi) = (ylo.min(y), yhi.max(y), plo.min(p), phi.max(p));
                t += 16;
            }
            (yhi - ylo, phi - plo)
        };
        run(&mut a, 0, 1500, |_| fire_cmd(false), &[]);
        let (y0, p0) = swing(&mut a, 1500);
        assert_eq!((y0, p0), (0.0, 0.0), "no sway without the scope");
        a.step(8500, &zoom_cmd(true), &aim(), &[], None);
        run(&mut a, 8516, 9500, |_| fire_cmd(false), &[]);
        assert_eq!(a.scope().level(), 1);
        a.take_view_yaw();
        a.take_view_pitch();
        let (y1, p1) = swing(&mut a, 9500);
        assert!((y1 - 2.0 * ax).abs() < 2e-4, "yaw swing {y1}");
        assert!((p1 - 2.0 * ay).abs() < 1e-4, "pitch swing {p1}");
    }

    #[test]
    fn rom_a_zoomed_sniper_shot_zooms_out_after_its_bolt_delay() {
        // CLI-646: a bolt-action (ReloadAfterFire) weapon leaves the scope 300 ms after a zoomed shot
        // (cshell+0x1f8b40); the L96 (27) with its scope (mod 27), playtest report
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let t = WeaponTable::open(&rom).expect("weapon table");
        assert!(t.get(27).expect("L96 row").reload_after_fire);
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(std::sync::Arc::new(t)),
            rom: Some(rom.clone()),
            ..ArmsConfig::default()
        });
        a.set_normal_fov_x(90f32.to_radians());
        a.on_grant(&WeaponGrant {
            // mod 53 is a Fast Loader: the clip is the row's less its rounds (no grant clip: the client's own sum)
            parts: [27, 33, 53, -1],
            clip: None,
            equip: true,
            ..grant(0, 27, 27, 20.0)
        });
        a.on_life(0, 1);
        run(&mut a, 0, 1500, |_| fire_cmd(false), &[]);
        a.step(1500, &zoom_cmd(true), &aim(), &[], None);
        run(&mut a, 1516, 2500, |_| fire_cmd(false), &[]);
        assert_eq!(a.scope().level(), 1);
        let v = run(&mut a, 2500, 2700, |t| fire_cmd(t < 2540), &[]);
        let shot = v
            .iter()
            .find(|m| m.1.shell == up::FIRE)
            .map(|m| m.0)
            .expect("a zoomed shot");
        assert!(a.zoom_active(), "still zoomed right after the shot");
        run(
            &mut a,
            2700,
            shot + crate::zoom::BOLT_UNZOOM_MS + 32,
            |_| fire_cmd(false),
            &[],
        );
        assert_eq!(
            a.scope().level(),
            0,
            "the zoom-out started after the bolt delay"
        );
        run(&mut a, shot + 400, shot + 2000, |_| fire_cmd(false), &[]);
        assert!(!a.zoom_active());
        assert!(
            a.scope().overlays().iter().all(Option::is_none),
            "no scope picture"
        );
        // a reload key while zoomed: the same delayed zoom-out, then the reload
        a.step(shot + 2000, &zoom_cmd(true), &aim(), &[], None);
        run(&mut a, shot + 2016, shot + 3000, |_| fire_cmd(false), &[]);
        assert_eq!(a.scope().level(), 1);
        let r = WeaponCommands {
            reload: true,
            ..WeaponCommands::default()
        };
        a.step(shot + 3000, &r, &aim(), &[], None);
        let v = run(&mut a, shot + 3016, shot + 4500, |_| fire_cmd(false), &[]);
        assert_eq!(a.scope().level(), 0);
        assert!(
            a.scope().overlays().iter().all(Option::is_none),
            "no scope picture"
        );
        assert!(
            v.iter().any(|m| m.1.shell == up::RELOAD),
            "the reload after the zoom-out"
        );
    }

    /// One frame of the window: the device events since the last frame, then the weapon step with the commands the
    /// keyboard and mouse state gives. Returns the shots fired in the step.
    fn device_frame(
        a: &mut Arsenal,
        k: &mut crate::input::KeyboardMouse,
        now: u32,
        events: &[crate::input::DeviceInput],
    ) -> u32 {
        use crate::input::InputSource;
        for e in events {
            k.device(*e);
        }
        let before = a.stats.shots;
        a.step(now, &k.weapon(), &aim(), &[], None);
        a.stats.shots - before
    }

    #[test]
    fn a_press_let_go_before_the_next_frame_still_fires_once() {
        // CLI-1601 (playtest report: "when clicking down was not shooting but when releasing it the gun did
        // shoot"): the fire button was a level read once per frame, so a press whose release came before the next
        // frame never reached the weapon; the 2009 press is an event (cshell+0x23b050)
        use crate::input::DeviceInput::Fire;
        let mut a = arsenal();
        a.on_grant(&grant(0, 3, 0, 9.0));
        a.on_life(0, 1);
        run(&mut a, 0, 520, |_| fire_cmd(false), &[]);
        let mut k = crate::input::KeyboardMouse::new(crate::look::LookParams::DEFAULT);
        assert_eq!(
            device_frame(&mut a, &mut k, 528, &[Fire(true), Fire(false)]),
            1
        );
        let mut t = 544;
        while t < 1200 {
            assert_eq!(
                device_frame(&mut a, &mut k, t, &[]),
                0,
                "no shot after the release ({t})"
            );
            t += 16;
        }
        // a press held over frames fires once; let go and pressed again within one frame, it fires again
        assert_eq!(device_frame(&mut a, &mut k, 1200, &[Fire(true)]), 1);
        let mut t = 1216;
        while t < 1800 {
            assert_eq!(
                device_frame(&mut a, &mut k, t, &[]),
                0,
                "held: one shot ({t})"
            );
            t += 16;
        }
        assert_eq!(
            device_frame(&mut a, &mut k, 1800, &[Fire(false), Fire(true)]),
            1
        );
        assert_eq!(device_frame(&mut a, &mut k, 1816, &[Fire(false)]), 0);
    }

    #[test]
    fn rom_a_zoomed_sniper_fires_on_the_press_and_never_on_the_release() {
        // CLI-1601: the L96 (27) with its scope (mod 27): zoomed in, a press fires at the press (also when it is let
        // go before the next frame), nothing fires at the release
        use crate::input::DeviceInput::{Action as Act, Fire};
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let t = WeaponTable::open(&rom).expect("weapon table");
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(std::sync::Arc::new(t)),
            rom: Some(rom),
            ..ArmsConfig::default()
        });
        a.set_normal_fov_x(90f32.to_radians());
        a.on_grant(&WeaponGrant {
            parts: [27, 33, -1, -1],
            clip: Some(5),
            equip: true,
            ..grant(0, 27, 27, 20.0)
        });
        a.on_life(0, 1);
        let mut k = crate::input::KeyboardMouse::new(crate::look::LookParams::DEFAULT);
        let zoom_in = |a: &mut Arsenal, k: &mut crate::input::KeyboardMouse, from: u32| {
            device_frame(a, k, from, &[Act(crate::input::Action::Zoom, true)]);
            device_frame(a, k, from + 16, &[Act(crate::input::Action::Zoom, false)]);
            let mut t = from + 32;
            while t < from + 1000 {
                assert_eq!(device_frame(a, k, t, &[]), 0);
                t += 16;
            }
            assert_eq!(a.scope().level(), 1, "zoomed in");
            assert!(!a.scope().changing(), "the zoom change ended");
        };
        let mut t = 0;
        while t < 1500 {
            device_frame(&mut a, &mut k, t, &[]);
            t += 16;
        }
        // a press held over several frames: the shot is at the press frame, none at the release
        zoom_in(&mut a, &mut k, 1500);
        assert_eq!(
            device_frame(&mut a, &mut k, 2500, &[Fire(true)]),
            1,
            "the press fires"
        );
        let mut shots = 0;
        let mut t = 2516;
        while t < 2600 {
            shots += device_frame(&mut a, &mut k, t, &[]);
            t += 16;
        }
        shots += device_frame(&mut a, &mut k, 2600, &[Fire(false)]);
        let mut t = 2616;
        while t < 5000 {
            shots += device_frame(&mut a, &mut k, t, &[]);
            t += 16;
        }
        assert_eq!(shots, 0, "nothing fires while held or at the release");
        // the bolt left the scope; in again, and a click shorter than a frame: one shot at that frame
        zoom_in(&mut a, &mut k, 5000);
        assert_eq!(
            device_frame(&mut a, &mut k, 6000, &[Fire(true), Fire(false)]),
            1
        );
        let mut t = 6016;
        while t < 8000 {
            assert_eq!(
                device_frame(&mut a, &mut k, t, &[]),
                0,
                "no shot later ({t})"
            );
            t += 16;
        }
    }

    #[test]
    fn rom_a_sniper_shot_cues_its_bolt_sound_at_the_fire_clips_key() {
        // CLI-647 (playtest report: "sniper not making the bolt-action audio between shots"): the L96's (27) fire clip
        // carries the bolt as sound key 2 at 720 ms; it plays (0x80 + local) like the select / reload keys
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let t = WeaponTable::open(&rom).expect("weapon table");
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(std::sync::Arc::new(t)),
            rom: Some(rom),
            ..ArmsConfig::default()
        });
        a.on_grant(&WeaponGrant {
            clip: Some(5),
            equip: true,
            ..grant(0, 27, 27, 20.0)
        });
        a.on_life(0, 1);
        run(&mut a, 0, 2000, |_| fire_cmd(false), &[]);
        let fire = a
            .clips_of(27)
            .and_then(|c| c.fire.clone())
            .expect("fire clip");
        let key = fire
            .marks
            .iter()
            .find_map(|(at, m)| match m {
                Mark::Sound(n) => Some((*at, *n)),
                _ => None,
            })
            .expect("the bolt sound key");
        let v = run(&mut a, 2000, 4000, |t| fire_cmd(t < 2030), &[]);
        let shot = v
            .iter()
            .find(|m| m.1.shell == up::FIRE)
            .map(|m| m.0)
            .expect("a shot");
        let sounds: Vec<(u32, u8)> = v
            .iter()
            .filter(|m| m.1.shell == up::WEAPON_SOUND)
            .map(|m| (m.0 - shot, wire(&m.1)[2]))
            .collect();
        println!("bolt key {key:?}, sounds after the shot {sounds:?}");
        assert_eq!(sounds.len(), 1);
        assert_eq!(sounds[0].1, key.1);
        assert!((key.0..key.0 + 16).contains(&sounds[0].0), "{sounds:?}");
    }

    #[test]
    fn class_keys_cycle_the_weapon_order_class_and_reach_the_backpack() {
        let key = |k: u8| WeaponCommands {
            class_key: Some(k),
            ..WeaponCommands::default()
        };
        let mut a = scoped([-1; 4]);
        assert_eq!(a.held().map(|c| c.slot), Some(0));
        // key 1 with the primary held: the next primary, the backpack slot 4 (class_pos 1), then back to slot 0
        let out = a.step(700, &key(0), &aim(), &[], None);
        assert_eq!(hex(&wire(&out[1])), "0b7f0406000100");
        run(&mut a, 716, 1500, |_| fire_cmd(false), &[]);
        a.step(1500, &key(0), &aim(), &[], None);
        assert_eq!(a.held().map(|c| c.slot), Some(0));
        run(&mut a, 1516, 2300, |_| fire_cmd(false), &[]);
        // key 1 again to slot 4; key 3 to the melee slot; key 1 from another class: the primary drawn last (4)
        a.step(2300, &key(0), &aim(), &[], None);
        run(&mut a, 2316, 3100, |_| fire_cmd(false), &[]);
        a.step(3100, &key(2), &aim(), &[], None);
        assert_eq!(a.held().map(|c| c.slot), Some(2));
        run(&mut a, 3116, 3900, |_| fire_cmd(false), &[]);
        a.step(3900, &key(0), &aim(), &[], None);
        assert_eq!(a.held().map(|c| c.slot), Some(4));
        run(&mut a, 3916, 4700, |_| fire_cmd(false), &[]);
        // key 2: the secondary; key 5: no class 5 weapon; key 9: refused (above class 7)
        a.step(4700, &key(1), &aim(), &[], None);
        assert_eq!(a.held().map(|c| c.slot), Some(1));
        run(&mut a, 4716, 5500, |_| fire_cmd(false), &[]);
        assert!(a.step(5500, &key(4), &aim(), &[], None).is_empty());
        assert!(a.step(5516, &key(8), &aim(), &[], None).is_empty());
        // key 4 (throw class): an empty throwable is passed over, so nothing is drawn
        a.on_ammo(3, 9, 0.0);
        assert!(a.step(5532, &key(3), &aim(), &[], None).is_empty());
        a.on_ammo(3, 9, 1.0);
        a.step(5548, &key(3), &aim(), &[], None);
        assert_eq!(a.held().map(|c| c.slot), Some(3));
    }

    #[test]
    fn without_a_weapon_order_a_class_key_names_its_slot() {
        let mut a = arsenal();
        a.on_grant(&grant(0, 5, 1, 9.0));
        a.on_grant(&grant(1, 3, 0, 9.0));
        a.on_life(0, 1);
        run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
        let cmd = WeaponCommands {
            class_key: Some(1),
            ..WeaponCommands::default()
        };
        a.step(600, &cmd, &aim(), &[], None);
        assert_eq!(a.held().map(|c| c.slot), Some(1));
    }

    #[test]
    fn the_zoom_key_of_a_melee_row_plays_the_alternate_fire_clip_and_sends_mode1() {
        // CLI-1350: made-up rows: weapon 7 a melee row (Guntype 0), weapon 8 a gun (Guntype 4)
        let doc = ButeDoc::parse(
            "[Ammo0]
Name = \"knf\"
MaxAmount = 1000
[Weapon7]
AmmoName0 = \"knf\"
Guntype = 0
VectorsPerRound = 1
ShotsPerClip = 1000
RapidFireCount = 0
[Weapon8]
AmmoName0 = \"knf\"
Guntype = 4
VectorsPerRound = 1
ShotsPerClip = 1000
RapidFireCount = 0
",
        );
        let table = std::sync::Arc::new(WeaponTable::from_doc(&doc));
        let clips = WeaponClips {
            fire: Some(ClipFact {
                index: 2,
                len_ms: 650,
                marks: vec![(155, Mark::Fire)],
            }),
            alt_clip: Some(ClipFact {
                index: 5,
                len_ms: 1100,
                marks: vec![(367, Mark::Fire)],
            }),
            ..WeaponClips::default()
        };
        let zoom = |z: bool, f: bool| WeaponCommands {
            zoom: z,
            fire: f,
            ..WeaponCommands::default()
        };
        let shots = |v: &[(u32, Outgoing)]| -> Vec<(u32, bool)> {
            v.iter()
                .filter(|m| m.1.shell == up::FIRE)
                .map(|m| (m.0, read_fire(&mut m.1.body.reader()).unwrap().mode1))
                .collect()
        };
        for (weapon, melee) in [(7u16, true), (8, false)] {
            let mut a = Arsenal::new(ArmsConfig {
                table: Some(table.clone()),
                ..ArmsConfig::default()
            });
            a.on_grant(&grant(2, weapon, 0, 1000.0));
            a.on_life(0, 1);
            run(&mut a, 0, 520, |_| zoom(false, false), &[]);
            a.clips.insert(weapon, Some(("test".into(), clips.clone())));
            // the zoom key alone: the alternate-fire clip, its key at 367 ms, the 0x82 with mode1
            let v = run(&mut a, 528, 900, |_| zoom(true, false), &[]);
            if !melee {
                assert!(shots(&v).is_empty(), "a gun's zoom key fires nothing");
                continue;
            }
            assert_eq!(
                a.view_clip(900).map(|c| c.1),
                Some(5),
                "the alternate-fire clip plays"
            );
            assert_eq!(shots(&v), vec![(896, true)]);
            // the clip runs to its end (1628) before the next one
            let v = run(&mut a, 912, 1600, |_| zoom(false, false), &[]);
            assert!(shots(&v).is_empty());
            run(&mut a, 1600, 1700, |_| zoom(false, false), &[]);
            assert_ne!(a.view_clip(1700).map(|c| c.1), Some(5));
            // the fire key wins over the zoom key: the fire clip and mode1 clear
            let v = run(&mut a, 1712, 1900, |_| zoom(true, true), &[]);
            assert_eq!(shots(&v), vec![(1872, false)]);
            assert_eq!(a.view_clip(1900).map(|c| c.1), Some(2));
        }
    }

    #[test]
    fn the_trigger_on_an_empty_weapon_dry_fires_with_sound_7() {
        let mut a = scoped([-1; 4]);
        // slot 0 empty: no clip, no stock
        a.on_ammo(0, 5, 0.0);
        let v = run(&mut a, 700, 1200, |t| fire_cmd(t < 720), &[]);
        let s = shells(&v);
        assert!(!s.contains(&up::FIRE) && !s.contains(&up::RELOAD));
        let snd: Vec<String> = v
            .iter()
            .filter(|m| m.1.shell == up::WEAPON_SOUND)
            .map(|m| hex(&wire(&m.1))[..6].to_string())
            .collect();
        assert_eq!(
            snd,
            vec!["0b8007".to_string()],
            "one dry-fire sound per press"
        );
        assert_eq!(a.phase(), Phase::Idle, "back to idle after the clip");
        // held down: still one; a second press: a second one
        let v = run(&mut a, 1200, 2000, |t| fire_cmd(t < 1900), &[]);
        assert_eq!(
            v.iter().filter(|m| m.1.shell == up::WEAPON_SOUND).count(),
            1
        );
        // with rounds in the stock the trigger reloads instead
        let mut b = scoped([-1; 4]);
        b.on_ammo(0, 5, 0.0);
        b.on_ammo(0, 5, 3.0);
        if let Some(c) = b.slots.get_mut(&0) {
            c.clip = 0;
            c.stock = 3;
        }
        let v = run(&mut b, 700, 900, |t| fire_cmd(t < 720), &[]);
        assert!(shells(&v).contains(&up::RELOAD));
    }

    /// ROM-gated: the sight mods of the user's ROM zoom (socket 1, a level and a field per level), the weapon rows
    /// carry the zoom time, and the first-person rifle model has a dry-fire clip. Counts only are printed.
    /// The user's round-4 loadout: the L96 (row `DisableCrosshair` 1) unzoomed draws no aim cross at all (neither the
    /// mark nor the static cross), the G36E draws its aim mark (CLI-616).
    #[test]
    fn rom_the_sniper_has_no_aim_cross_unzoomed_and_the_rifle_has_one() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let t = std::sync::Arc::new(WeaponTable::open(&rom).expect("weapon table"));
        let crosses = |weapon: u16| -> usize {
            let mut a = Arsenal::new(ArmsConfig {
                table: Some(t.clone()),
                ..ArmsConfig::default()
            });
            a.on_grant(&grant(0, weapon, weapon, 20.0));
            a.on_life(0, 1);
            run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
            let state = crate::hud::HudState {
                life_state: 1,
                aim_mark: a.aim_mark(600),
                aim_cross_hidden: a.aim_cross_hidden(),
                scope: crate::hud::scope::ScopeView::of(a.scope()),
                ..Default::default()
            };
            let layout = crate::hud::HudLayout::fallback();
            crate::hud::items::build(&layout, &state, &|_, _| 0.0, &|_| None)
                .iter()
                .filter(|i| {
                    matches!(
                        i,
                        crate::hud::DrawItem::AimMark(_) | crate::hud::DrawItem::AimCross { .. }
                    )
                })
                .count()
        };
        assert_eq!(crosses(27), 0, "L96 unzoomed: no aim cross");
        assert_eq!(crosses(25), 1, "G36E: its aim mark");
    }

    /// Playtest report: knife has the wrong aim cross, it should be the red X icon. The knife row (weapon 0) draws its
    /// `DotSightName` picture centred on the layout's DotSightPos and no aim mark; a rifle draws its aim mark and no
    /// picture (CLI-630).
    #[test]
    fn rom_the_knife_draws_its_red_x_picture_and_the_rifle_its_aim_mark() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let keys = dc_ui_data::hud::HudKeys::from_rom(&rom).expect("hud keys");
        let names = dc_ui_data::names::UiNames::from_rom(&rom).expect("names");
        let table = dc_ui_data::layout::LayoutTable::load(&dc_rez::RezVfs::new(&rom), names)
            .expect("layout table");
        let layout = crate::hud::HudLayout::from_rom(
            &table,
            &keys,
            0,
            None,
            &std::collections::BTreeMap::new(),
        );
        let (at, size) = layout.dot_sight.expect("ROM DotSightPos / DotSightSize");
        let t = std::sync::Arc::new(WeaponTable::open(&rom).expect("weapon table"));
        let items = |weapon: u16| {
            let mut a = Arsenal::new(ArmsConfig {
                table: Some(t.clone()),
                ..ArmsConfig::default()
            });
            a.on_grant(&grant(0, weapon, weapon, 20.0));
            a.on_life(0, 1);
            run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
            let state = crate::hud::HudState {
                life_state: 1,
                aim_mark: a.aim_mark(600),
                aim_cross_hidden: a.aim_cross_hidden(),
                aim_picture: a.aim_picture(),
                scope: crate::hud::scope::ScopeView::of(a.scope()),
                ..Default::default()
            };
            crate::hud::items::build(&layout, &state, &|_, _| 0.0, &|_| None)
        };
        let pictures = |v: &[crate::hud::DrawItem]| -> Vec<(String, [f32; 4])> {
            v.iter()
                .filter_map(|i| match i {
                    crate::hud::DrawItem::Image { tex, dst, .. } if tex.contains("Sight") => {
                        Some((tex.clone(), *dst))
                    }
                    _ => None,
                })
                .collect()
        };
        let marks = |v: &[crate::hud::DrawItem]| {
            v.iter()
                .filter(|i| matches!(i, crate::hud::DrawItem::AimMark(_)))
                .count()
        };
        let knife = items(0);
        let p = pictures(&knife);
        assert_eq!(p.len(), 1, "the knife draws one aim picture: {p:?}");
        assert!(
            p[0].0.to_ascii_lowercase().ends_with("knife_sight_01.dtx"),
            "{p:?}"
        );
        let c = [p[0].1[0] + p[0].1[2] / 2.0, p[0].1[1] + p[0].1[3] / 2.0];
        assert!(
            (c[0] - at[0] as f32).abs() <= 1.0 && (c[1] - at[1] as f32).abs() <= 1.0,
            "{p:?}"
        );
        assert_eq!(marks(&knife), 0, "no aim mark for the knife");
        // the 2009 quad shows the whole texture: the picture's texels are the layout size
        use crate::hud::compose::TextureSource;
        let tex = crate::hud::compose::RomTextureSource::open(&rom)
            .expect("textures")
            .load(&p[0].0)
            .expect("knife picture");
        assert_eq!((tex.width as i32, tex.height as i32), (size[0], size[1]));
        let rifle = items(25);
        assert!(pictures(&rifle).is_empty(), "G36E: no picture");
        assert_eq!(marks(&rifle), 1, "G36E: its aim mark");
    }

    /// The user's round-4 sniper (weapon 27, scope mod 27, silencer 33, magazine 53): zoomed, the HUD's list starts with
    /// the scope picture filling the screen; not zoomed there is none (CLI-615).
    #[test]
    fn rom_a_zoomed_sniper_draws_the_scope_picture_under_the_hud() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let t = WeaponTable::open(&rom).expect("weapon table");
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(std::sync::Arc::new(t)),
            rom: Some(rom.clone()),
            ..ArmsConfig::default()
        });
        a.set_normal_fov_x(90f32.to_radians());
        a.on_grant(&WeaponGrant {
            parts: [27, 33, 53, -1],
            ..grant(0, 27, 27, 20.0)
        });
        a.on_life(0, 1);
        run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
        let mut hud = crate::hud::Hud::from_rom(&rom, 1280, 720, None).expect("ROM HUD");
        let mut state = crate::hud::HudState {
            life_state: 1,
            ..Default::default()
        };
        let scope_items = |hud: &mut crate::hud::Hud,
                           state: &crate::hud::HudState|
         -> Vec<crate::hud::DrawItem> {
            hud.items(state)
                .into_iter()
                .filter(|i| matches!(i, crate::hud::DrawItem::Image { tex, .. } if tex.contains("Scope")))
                .collect()
        };
        state.scope = crate::hud::scope::ScopeView::of(a.scope());
        assert!(
            scope_items(&mut hud, &state).is_empty(),
            "not zoomed: no scope picture"
        );
        a.step(1000, &zoom_cmd(true), &aim(), &[], None);
        a.step(1016, &zoom_cmd(false), &aim(), &[], None);
        a.step(2000, &zoom_cmd(false), &aim(), &[], None);
        assert_eq!(a.scope().level(), 1);
        state.scope = crate::hud::scope::ScopeView::of(a.scope());
        let items = hud.items(&state);
        let Some(crate::hud::DrawItem::Image { tex, src, dst, .. }) = items.first() else {
            panic!(
                "the first HUD item is the scope picture: {:?}",
                items.first()
            );
        };
        println!("scope picture {tex} {src:?} at {dst:?}");
        assert!(tex.ends_with("SR_Scope_Style_A.spr"));
        // 1024 texels x 0.001 / tan(25 deg) = 2.196 half screen heights; centred
        assert_eq!(*src, [1024.0, 1024.0]);
        let h = 768.0 * 1.024 / 25f32.to_radians().tan();
        assert!((dst[3] - h).abs() < 0.5, "height {} vs {h}", dst[3]);
        assert!((dst[1] + dst[3] * 0.5 - 384.0).abs() < 0.01);
        assert!((dst[0] + dst[2] * 0.5 - 512.0).abs() < 0.01);
        // square texels on the 16:9 target
        assert!((dst[2] * 1280.0 / 1024.0 - dst[3] * 720.0 / 768.0).abs() < 0.5);
        assert_eq!(scope_items(&mut hud, &state).len(), 1);
    }

    #[test]
    fn rom_sights_zoom_and_the_rifle_has_a_dry_fire_clip() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let t = WeaponTable::open(&rom).expect("weapon table");
        let sights: Vec<&crate::zoom::ModInfo> = t
            .mod_info
            .values()
            .filter(|m| m.attach_socket() == Some(crate::zoom::SIGHT_SOCKET) && m.zoom_level > 0)
            .collect();
        let silencers = t
            .mod_info
            .values()
            .filter(|m| m.silencer_level().is_some())
            .count();
        println!(
            "mods {}, zooming sights {}, silencers {}, rows with ZoomTime {}, Guntype 11 rows {}",
            t.mod_info.len(),
            sights.len(),
            silencers,
            t.rows.values().filter(|r| r.zoom_time.is_some()).count(),
            t.rows
                .values()
                .filter(|r| r.gun_type == Some(GUN_TYPE_SPRINT_UNZOOM))
                .count()
        );
        assert!(!sights.is_empty());
        for m in &sights {
            assert!(
                m.fov_of(1).is_some(),
                "every zooming sight has a field for level 1"
            );
        }
        // the mod the rust7 user attached (35) is such a sight
        let m35 = t.mod_info.get(&35).expect("mod 35");
        assert_eq!(m35.attach_socket(), Some(crate::zoom::SIGHT_SOCKET));
        assert!(m35.zoom_level >= 1);
        // the rust6 sniper rifle: the scope DSV-80 grants it is a zooming sight too
        let mut a = Arsenal::new(ArmsConfig {
            table: Some(std::sync::Arc::new(t)),
            rom: Some(rom),
            ..ArmsConfig::default()
        });
        a.on_grant(&WeaponGrant {
            parts: [27, -1, -1, -1],
            ..grant(4, 27, 27, 20.0)
        });
        assert_eq!(a.mods_of(27)[crate::zoom::SIGHT_SOCKET], Some(27));
        let c = a.clips_of(145).cloned().expect("rifle clips");
        assert!(
            c.dry_fire.is_some(),
            "the rifle's first-person model has the dry-fire clip"
        );
        let marks = &c.dry_fire.as_ref().unwrap().marks;
        println!(
            "rifle dry-fire clip: {} ms, marks {:?}",
            c.dry_fire.as_ref().unwrap().len_ms,
            marks
        );
    }

    /// CLI-1590: a launcher's round (ammunition `Type` 1) is sent without a trace: with a box in line the 0x82 has
    /// the no-hit form (623 bits) and the pellet effect no hit, while a traced round (`Type` 0) hits the same box
    /// (887 bits).
    #[test]
    fn a_launcher_round_is_sent_without_a_hit_claim() {
        let extra = [
            "[Ammo6]",
            "Name = \"rkt\"",
            "Type = 1",
            "MaxAmount = 9",
            "[Weapon6]",
            "AmmoName0 = \"rkt\"",
            "VectorsPerRound = 1",
            "ShotsPerClip = 3",
            "RapidFireCount = 1",
            "[Ammo7]",
            "Name = \"bul\"",
            "Type = 0",
            "MaxAmount = 9",
            "[Weapon7]",
            "AmmoName0 = \"bul\"",
            "VectorsPerRound = 1",
            "ShotsPerClip = 3",
            "RapidFireCount = 1",
        ];
        let text = table_text() + "\n" + &extra.join("\n");
        let t = std::sync::Arc::new(WeaponTable::from_doc(&ButeDoc::parse(&text)));
        assert_eq!(t.rows[&6].ammo_type, Some(1));
        let target = [Target {
            obj: 77,
            centre: [0.0, 40.0, 500.0],
            half: [16.0, 40.0, 16.0],
        }];
        for (weapon, ammo, want_bits, want_hits) in [(6u16, 6u16, 623usize, 0u32), (7, 7, 887, 1)] {
            let mut a = Arsenal::new(ArmsConfig {
                table: Some(t.clone()),
                ..ArmsConfig::default()
            });
            a.on_grant(&grant(0, weapon, ammo, 9.0));
            a.on_life(0, 1);
            run(&mut a, 0, 600, |_| fire_cmd(false), &[]);
            let v = run(&mut a, 600, 640, |_| fire_cmd(true), &target);
            let fires: Vec<&Outgoing> = v
                .iter()
                .filter(|m| m.1.shell == up::FIRE)
                .map(|m| &m.1)
                .collect();
            assert_eq!(fires.len(), 1, "weapon {weapon}");
            assert_eq!(bits(fires[0]), want_bits, "weapon {weapon}");
            assert_eq!(a.stats.hits, want_hits, "weapon {weapon}");
            let fx = a.take_shot_fx();
            assert_eq!(
                matches!(fx[0].hit, RayHit::Character { .. }),
                want_hits == 1,
                "weapon {weapon}"
            );
        }
    }
}
