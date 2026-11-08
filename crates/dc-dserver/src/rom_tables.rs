//! Game tables the dedicated server reads from the user's ROM at run time (never copied into the tree): the weapon
//! and ammunition table the client itself loads (cshell+0x17f270, the entry [`WEAPONS_ENTRY`]) and the team names that
//! the world's spawn points use as their team filter. Without a ROM the tables are empty and the DeCombat defaults of
//! [`crate::game::GameTuning`] apply.

use std::collections::BTreeMap;
use std::path::Path;

use dc_bute::ButeDoc;
use dc_rez::RezVfs;

/// Archive holding the attribute tables.
// OBSERVED: ROM table key in ATTRIBUTES.rez (the archive dc-bute reads ITEMS.TXT / GAMEROOM.TXT from)
pub const ATTRIBUTES_REZ: &str = "ATTRIBUTES.rez";
/// The weapon table entry. The client re-reads the same file on `GS_RELOAD_WEAPON_TABLE` (str(VA 0x374e7048;
/// f6276e61a84356df; 22): the sha256 prefix matches this name in the case the client uses).
// SPEC: GAME_MESSAGES_2009_S2C.md §5.1 (0x73, cshell+0x23c8e0 -> cshell+0x17f270)
// OBSERVED: ROM table key WEAPONS.TXT (entry of ATTRIBUTES.rez)
pub const WEAPONS_ENTRY: &str = "ATTRIBUTES\\WEAPONS.TXT";
/// The team table entry.
// OBSERVED: ROM table key TEAMS.TXT (entry of ATTRIBUTES.rez)
pub const TEAMS_ENTRY: &str = "ATTRIBUTES\\TEAMS.TXT";

/// Section prefixes and keys of the weapon table: `[Weapon<n>]` rows (n = the `weapon_db` index the client sends in
/// `UP_FIRE` and the server sends in `GS_WEAPON_GIVE`), `[Ammo<n>]` rows (n = the ammunition id).
// OBSERVED: ROM table key WEAPONS.TXT sections [Weapon<n>] (0..N-1 contiguous) and [Ammo<n>] (0..M-1 contiguous)
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (the weapon number is a `weapon_db` index, row by number cshell+0x172300);
// SPEC: UNPROVEN that the row number is the section number n (PLAUSIBLE: the table is loaded in section order)
pub const WEAPON_SECTION: &str = "Weapon";
pub const AMMO_SECTION: &str = "Ammo";
/// `[Weapon<n>]` clip size.
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] ShotsPerClip
pub const KEY_CLIP: &str = "ShotsPerClip";
/// `[Weapon<n>]` flag: the weapon is its own ammunition (non-zero for the throwables, mines and kits; their
/// `MaxAmount` counts the round in the hand).
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] IsAmmo (1 on 20 rows, every one with ShotsPerClip 1; launchers
// with throwable-type ammunition rows have none)
pub const KEY_IS_AMMO: &str = "IsAmmo";
/// `[Weapon<n>]` kind of the weapon on the client: 2 for the hand-thrown and placed weapons (the grenades, the kits,
/// the mines, the sensors), 1 for the guns and launchers.
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] ClientWeaponType (2 on every hand grenade row and the mine rows, 1 on
// the launchers, the rocket rows and the snowball gun; values read at run time 2026-11-08); dc-client reads the same
// key for its throw path (`arms::CLIENT_KIND_THROWN`, the meaning PLAUSIBLE)
pub const KEY_CLIENT_KIND: &str = "ClientWeaponType";
/// The [`KEY_CLIENT_KIND`] value of a hand-thrown weapon.
pub const CLIENT_KIND_HAND: u32 = 2;
/// `[Weapon<n>]` pellets per round (the client sends one `UP_FIRE` per pellet).
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] VectorsPerRound
pub const KEY_PELLETS: &str = "VectorsPerRound";
/// `[Weapon<n>]` trace length of the client's shot (knife 105).
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] Range (read at run time); SPEC: WEAPON_FIRE_2009.md §5.2
pub const KEY_RANGE: &str = "Range";
/// `[Weapon<n>]` sensor keys: the row's `SensorOption` (-1 or missing: no sensor) and its scan distance.
// OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] SensorOption, ScanDist (read at run time)
// DISASM: cshell+0x17d04f (`ScanDist` -> row +0x48c), cshell+0x17d0c3 (`SensorOption` -> row +0x21c, default -1)
pub const KEY_SENSOR_OPTION: &str = "SensorOption";
pub const KEY_SCAN_DIST: &str = "ScanDist";
/// `[Weapon<n>]` perturb bounds of the shot cone: plain, zoomed (while the 0x82 zoom bit is set) and triple-shot
/// (while `wflag` is set), integers in thousandths of a tangent unit.
// OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] MinPerturb, MaxPerturb, ZoomedMinPerturb, ZoomedMaxPerturb,
// TripleShotMinPerturb, TripleShotMaxPerturb (read at run time, never copied)
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-2.1 (row +0x288 / +0x28c, +0x298 / +0x29c, +0x330 / +0x334)
pub const KEY_MIN_PERTURB: &str = "MinPerturb";
pub const KEY_MAX_PERTURB: &str = "MaxPerturb";
pub const KEY_ZOOMED_MIN_PERTURB: &str = "ZoomedMinPerturb";
pub const KEY_ZOOMED_MAX_PERTURB: &str = "ZoomedMaxPerturb";
pub const KEY_TRIPLE_MIN_PERTURB: &str = "TripleShotMinPerturb";
pub const KEY_TRIPLE_MAX_PERTURB: &str = "TripleShotMaxPerturb";
/// `[Weapon<n>]` first ammunition name.
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] AmmoName0
pub const KEY_AMMO_NAME0: &str = "AmmoName0";
/// `[Ammo<n>]` / `[Weapon<n>]` name.
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] Name
pub const KEY_NAME: &str = "Name";
/// `[Ammo<n>]` damage of one hit.
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] InstDamage
pub const KEY_DAMAGE: &str = "InstDamage";
/// `[Ammo<n>]` damage of one alternate-fire hit (the knife's heavy stab).
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] Alt_InstDamage (33 rows; the knife's 75 against its InstDamage 45);
// DISASM: the client reads the key into its ammunition record (literal at cshell+0x3e7714)
pub const KEY_ALT_DAMAGE: &str = "Alt_InstDamage";
/// `Guntype` of a weapon row: 0 melee, 0xd double barrel, 0xe snowballs (the rows the 2009 client fires in mode 1).
pub const KEY_GUN_TYPE: &str = "Guntype";
/// `[Ammo<n>]` damage type of a direct hit (a small number: the `zone` field of `GS_DAMAGE_TAKEN`).
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] InstDamageType (CAPTURE_tdm4_KILL_VALIDATION.md §3.1 item 3: the
// reference stack's 0x74 `zone` equals this key's value)
pub const KEY_DAMAGE_TYPE: &str = "InstDamageType";
/// `[Ammo<n>]` largest carried amount.
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] MaxAmount
pub const KEY_MAX_AMOUNT: &str = "MaxAmount";
/// `[Team<k>]` name (k = 1, 2 for the two teams); the same text is the team filter of the world's spawn points.
// OBSERVED: ROM table key TEAMS.TXT [Team<n>] Name (2026-10-24: [Team1] / [Team2] names equal the two spawn team
// filter values of the ROM worlds, compared case-insensitively; read at run time, never copied)
pub const TEAM_SECTION: &str = "Team";
/// `[Ammo<n>]` area damage of a blast (at its centre), its damage type (the `zone` of `GS_DAMAGE_TAKEN`) and radius.
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] AreaDamage, AreaDamageType, AreaDamageRadius
// (CAPTURE_tdm4_KILL_VALIDATION.md §3.1 items 3, 4: the reference stack's 0x74 for a grenade carried `zone` = AreaDamageType
// and `damage` = AreaDamage of the thrown ammunition)
pub const KEY_AREA_DAMAGE: &str = "AreaDamage";
pub const KEY_AREA_DAMAGE_TYPE: &str = "AreaDamageType";
pub const KEY_AREA_RADIUS: &str = "AreaDamageRadius";
/// `[Ammo<n>]` name of the projectile row of a thrown or launched ammunition (a `Name` of the FX table's
/// `[ProjectileFX<k>]` rows).
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] ProjectileFXName; FX.TXT [ProjectileFX<k>] Name (2026-10-25: the
// thrown grenade's ammunition names a row of FX.TXT; values read at run time, never copied)
pub const KEY_PROJECTILE_FX: &str = "ProjectileFXName";
/// `[Ammo<n>]` name of the impact effect of an ammunition (the world's explosion objects name the same effects: the
/// blast of a destroyed world object is shown with the ammunition whose impact effect it names).
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] ImpactFXName; the explosion objects of the ROM worlds name one of
// these values (2026-10-25 scratch survey, world-objects; values read at run time, never copied)
pub const KEY_IMPACT_FX: &str = "ImpactFXName";
/// The ammunition row's kind (`Type`): 0 an instant trace, 1 a fired projectile (the launchers' rows), 2 a thrown
/// one (the grenades' rows). Not the whole story: the projectile row's class decides launched or thrown where it
/// names one ([`RomTables::round_flight`], DSV-650; the grenade launcher's row is `Type` 2).
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] Type (LAW / M136_AT4 rows 1 with ProjectileFXName Missile, the M67 row
// 2; values read at run time); WEAPON_FIRE_2009_addendum_cone.md AC-3.4 (the client traces only the instant kind)
pub const KEY_AMMO_KIND: &str = "Type";
/// The `Type` of a fired projectile's ammunition row.
pub const AMMO_KIND_PROJECTILE: u8 = 1;
/// The `Type` of a thrown round's ammunition row (the grenades).
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] Type (the M67 row 2; values read at run time)
pub const AMMO_KIND_THROWN: u8 = 2;
/// `[Ammo<n>]` progressive damage of a round: the damage, its damage type, how long one touch burns (seconds), the
/// radius round the round's burst point that it reaches, and a lifetime (seconds; read, not used: DSV-455). The
/// flamethrower rows carry them with no area damage.
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] ProgDamage, ProgDamageType, ProgDamageDuration, ProgDamageRadius,
// ProgDamageLifetime (values read at run time, never copied)
// EVIDENCE: cshell+0x174d30 (the client's ammunition row reader takes the five keys into the row, after the area keys)
pub const KEY_PROG_DAMAGE: &str = "ProgDamage";
pub const KEY_PROG_DAMAGE_TYPE: &str = "ProgDamageType";
pub const KEY_PROG_DURATION: &str = "ProgDamageDuration";
pub const KEY_PROG_RADIUS: &str = "ProgDamageRadius";
pub const KEY_PROG_LIFETIME: &str = "ProgDamageLifetime";
/// The FX table entry holding the projectile rows, their section prefix and keys: model, skin, model scale, the
/// gravity switch and its override, life time in seconds and launch speed.
// OBSERVED: ROM table key FX.TXT (entry of ATTRIBUTES.rez), sections [ProjectileFX<k>] with Name, Model, Skin,
// ModelScale, Gravity, GravityOverride, Lifetime, Velocity (2026-10-25; the row named by the tdm2 grenade's
// ammunition has the model and skin of the reference stack's projectile create, CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.4)
pub const FX_ENTRY: &str = "ATTRIBUTES\\FX.TXT";
pub const PROJECTILE_SECTION: &str = "ProjectileFX";
pub const KEY_PFX_MODEL: &str = "Model";
pub const KEY_PFX_SKIN: &str = "Skin";
pub const KEY_PFX_SCALE: &str = "ModelScale";
pub const KEY_PFX_GRAVITY: &str = "Gravity";
pub const KEY_PFX_GRAVITY_OVERRIDE: &str = "GravityOverride";
pub const KEY_PFX_LIFETIME: &str = "Lifetime";
pub const KEY_PFX_VELOCITY: &str = "Velocity";
/// A projectile row's object class and the name of the class-data row it uses.
// OBSERVED: ROM table key FX.TXT [ProjectileFX<k>] Class, ClassData (the mine rows name a class-data row, the grenade
// and rocket rows none; 2026-11-05 scratch run)
// EVIDENCE: cshell+0x292f90 (the client's projectile row reader takes `Class` and resolves `ClassData` by name in
// the class-data rows, cshell+0x28fdf0)
pub const KEY_PFX_CLASS: &str = "Class";
pub const KEY_PFX_CLASS_DATA: &str = "ClassData";
/// How the server flies a round: launched (it bursts where it first meets the world or a player, or at the end of
/// its row's `Lifetime`) or thrown (it bounces and goes off at its fuse, the row's `Lifetime`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundFlight {
    Launched,
    Thrown,
}
/// Name keys of the two projectile row classes that tell the flight (folded class names; the names are ROM data,
/// matched by key, never quoted): the impact class (the user's ROM: the rocket, flame, snowball and the grenade
/// launcher's shell rows) and the fused class (every hand grenade row: frag, flash, smoke, the kit).
// OBSERVED: ROM table key FX.TXT [ProjectileFX<k>] Class, read at run time 2026-11-08 (m32-launcher scratch run, rows
// tabulated, not copied): the LAW / RPG / AT4 rocket row and the grenade launcher's shell row share the impact class
// while the launcher's ammunition row is `Type` 2 like the frag's; the flash, smoke and two other hand grenades are
// `Type` 1 with the fused class. The ammunition `Type` therefore does not tell a launched round from a thrown one; the
// projectile row's class does (both launcher rows carry it, no hand grenade does)
// EVIDENCE: cshell+0x292f90 (the client reads `Class` into its projectile row; what the class does is the server
// object's, not in the client)
// DC-DEFAULT: DSV-650 (proposed) the projectile row's class decides launched or thrown
pub const PFX_CLASS_IMPACT: crate::chars::NameKey =
    ([0x31, 0x6e, 0x49, 0xfa, 0xe0, 0x02, 0xc9, 0x84], 11);
pub const PFX_CLASS_FUSED: crate::chars::NameKey =
    ([0x92, 0x7e, 0xc5, 0xa5, 0x17, 0xa9, 0xa9, 0x6a], 8);

/// The flight a projectile row's `Class` names (`None`: another class, such as a mine's, or none).
// DC-DEFAULT: DSV-650 (proposed)
pub fn class_flight(class: &str) -> Option<RoundFlight> {
    let k = crate::chars::name_key(dc_bute::fold_latin1(class.trim()).as_bytes());
    if k == PFX_CLASS_IMPACT {
        Some(RoundFlight::Launched)
    } else if k == PFX_CLASS_FUSED {
        Some(RoundFlight::Thrown)
    } else {
        None
    }
}
/// The mine class-data rows of the FX table and their keys: the distance at which a mine goes off and the delay
/// before it is armed (seconds).
// DISASM: cshell+0x295e44 (the FX table loader reads `LandTrapClassData<n>` rows from 0 up), cshell+0x28e680 (the
// row reader: `Name`, `DetonateRadius` as an integer, `ArmDelay` as a float)
// OBSERVED: ROM table key FX.TXT [LandTrapClassData<n>] Name, ArmDelay (the user's ROM has one row and no
// DetonateRadius; values read at run time)
pub const TRAP_SECTION: &str = "LandTrapClassData";
pub const KEY_TRAP_DETONATE_RADIUS: &str = "DetonateRadius";
pub const KEY_TRAP_ARM_DELAY: &str = "ArmDelay";
/// The 2009 client's values for a mine class-data row without `DetonateRadius` / `ArmDelay`.
// DISASM: cshell+0x28e6ab (`push 0x40` as the integer default of DetonateRadius), cshell+0x28e6ba (the float default
// of ArmDelay, the double at cshell+0x3d61a8 = 1.0)
pub const TRAP_DEFAULT_DETONATE_RADIUS: f32 = 64.0;
pub const TRAP_DEFAULT_ARM_DELAY_S: f32 = 1.0;

/// `[Gear<n>]` armour rows of the weapon table: the armour points (`Armor`) and, per protection slot i, the damage
/// type the slot protects (`ProtectionType<i>`) and the share of a hit it takes on the head, torso, arms and legs
/// (`NodeHeadProtection<i>`, `NodeTorsoProtection<i>`, `NodeArmProtection<i>`, `NodelegProtection<i>`; the client's
/// reader clamps each share to 0..1).
// OBSERVED: ROM table key WEAPONS.TXT [Gear<n>] Armor, ProtectionType<i>, NodeHeadProtection<i>,
// NodeTorsoProtection<i>, NodeArmProtection<i>, NodelegProtection<i> (key names as the ROM spells them; values read
// at run time)
// EVIDENCE: cshell+0x17e750 (the client's reader of one gear row: the protection type list, the four node share
// lists each clamped to [0, 1], then `Armor` and `Health`)
pub const GEAR_SECTION: &str = "Gear";
pub const KEY_PROTECTION_TYPE: &str = "ProtectionType";
/// The share keys by hit location (1 head, 2 torso, 3 arms, 4 legs: `[Skeleton<s>] Node<k>Location`).
pub const KEY_NODE_PROTECTION: [&str; 4] = [
    "NodeHeadProtection",
    "NodeTorsoProtection",
    "NodeArmProtection",
    "NodelegProtection",
];
/// Highest protection slot index read per gear row (the rows stop at the first missing `ProtectionType<i>`).
pub const GEAR_SLOTS_MAX: u32 = 16;

/// One `[Gear<n>]` row: its armour points and the protected damage types with their share by hit location.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GearProtection {
    pub armor: Option<f32>,
    /// `(damage type, [head, torso, arms, legs] share)` per protection slot, in slot order.
    pub slots: Vec<(u8, [f32; 4])>,
}

impl GearProtection {
    /// The share of a hit of damage type `dtype` at hit location `location` (1 head, 2 torso, 3 arms, 4 legs; any
    /// other value counts as torso) that this row's armour takes: the first slot protecting that type, else 0.
    pub fn share(&self, dtype: u8, location: u8) -> f32 {
        let z = match location {
            1 => 0,
            3 => 2,
            4 => 3,
            _ => 1,
        };
        self.slots
            .iter()
            .find(|s| s.0 == dtype)
            .map_or(0.0, |s| s.1[z])
    }
}

/// Which `UP_FIRE` field selects the damage row. The fire message carries both the weapon index and the
/// ammunition id; which table row the 2009 server took the damage from is UNPROVEN, DeCombat uses the ammunition
/// row and falls back to the weapon's first ammunition.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (`weapon_id` weapon+0x98, `ammo_id` weapon+0x9c; meanings PLAUSIBLE)
// SPEC: UNPROVEN
pub const TODO_DAMAGE_FROM_AMMO_ROW: bool = true;

/// `[Weapon<n>]` third-person model of the weapon (the model other players see in the character's hand), its
/// skins and render styles.
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] HHModel, HHSkin0..3, HHRenderStyle0..1
pub const KEY_HH_MODEL: &str = "HHModel";
pub const KEY_HH_SKIN: &str = "HHSkin";
pub const KEY_HH_STYLE: &str = "HHRenderStyle";
/// The weapon order section: `Class<k>` names the weapon classes in order, `<class name><i>` the weapon names of
/// each class in order. The client concatenates them into its weapon command table; `GS_WEAPON_SWITCH` 0x7f names
/// a weapon by `0x14 +` its position in that table.
// EVIDENCE: cshell+0x180590 (weapon_db constructor: "WeaponOrder" / "Class" reads, table at weapon_db+0x15dc)
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0x7f: weapon := command table [command - 0x14])
// OBSERVED: ROM table key WEAPONS.TXT [WeaponOrder] Class<k>, <class><i>
pub const WEAPON_ORDER_SECTION: &str = "WeaponOrder";
pub const KEY_CLASS: &str = "Class";
/// First command value of the weapon command table.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0x7f `command - 0x14`)
pub const WEAPON_COMMAND_BASE: u32 = 0x14;

/// `[Mod<n>]` rows: weapon attachments (scopes, muzzles, magazines); n is the mod index the client's mod table uses
/// and the part bytes of `GS_WEAPON_GIVE` / `GS_WEAPON_EQUIP` carry.
// EVIDENCE: cshell+0x17f270 (the mod table is filled in section order, "Mod%d")
// OBSERVED: ROM table key WEAPONS.TXT [Mod<n>] Name (sections 0..N-1 contiguous; read at run time, never copied)
pub const MOD_SECTION: &str = "Mod";
/// `[Weapon<n>]` default attachments, by socket: scope (socket 1), muzzle, magazine; each names a `[Mod<n>]` row.
// EVIDENCE: cshell+0x17bd20 (row +0xd8 DefaultScopeAttach, muzzle and magazine defaults next), cshell+0x172510 (mod
// lookup by name, case-insensitive, first match)
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] DefaultScopeAttach, DefaultMuzzleAttach, DefaultMagazineAttach
pub const KEY_DEFAULT_ATTACH: [&str; 3] = [
    "DefaultScopeAttach",
    "DefaultMuzzleAttach",
    "DefaultMagazineAttach",
];
/// `[Mod<n>]` socket (`Kinds`: 0 muzzle, 1 sight, 2 magazine) and zoom level; `[Weapon<n>]` attachment list
/// (`ModName<k>`, mod names).
// EVIDENCE: cshell+0x176f10 (mod row reader: Kinds -> row +0x17c, ZoomLevel -> row +0x110), cshell+0x235ec0 (attach:
// the socket is row +0x17c, 0..2)
// OBSERVED: ROM table keys WEAPONS.TXT [Mod<n>] Kinds, ZoomLevel; [Weapon<n>] ModName<k>
pub const KEY_KINDS: &str = "Kinds";
pub const KEY_ZOOM_LEVEL: &str = "ZoomLevel";
pub const KEY_MOD_NAME: &str = "ModName";
/// `[Weapon<n>]` `ModModel<i>`: the index `k` the mod `ModName<i>` uses on this weapon (its `AttachModel<k>`, its
/// `ShotsPerClipPlus<k>` / `MaxAmountPlus<k>`).
// EVIDENCE: cshell+0x17bd20 (`ModModel<i>` -> row +0x268), cshell+0x172b20 (the `ModModel` of a mod id)
pub const KEY_MOD_MODEL: &str = "ModModel";
/// `[Mod<n>]` magazine bonuses, per `k` (0..9): rounds added to the clip and to the carried maximum.
// EVIDENCE: cshell+0x176f10 (`ShotsPerClipPlus<k>` -> mod +0x1b0 + 4 k, `MaxAmountPlus<k>` -> +0x1d8 + 4 k, k < 10)
pub const KEY_CLIP_PLUS: &str = "ShotsPerClipPlus";
pub const KEY_MAX_PLUS: &str = "MaxAmountPlus";
/// `[Mod<n>]` damage term (the suppressors' -0.1); the 2009 client reads it but nothing in the client consumes it.
// EVIDENCE: cshell+0x176f10 (`Instdamage` -> mod +0x29c, default 0); DISASM: cshell+0x2334f0, cshell+0x82c00 (the only
// code that walks the three sockets near a +0x29c read takes it from the weapon row, method +0x70, not from the mod)
// OBSERVED: ROM table key WEAPONS.TXT [Mod<n>] Instdamage (12 suppressor rows hold -0.1)
pub const KEY_MOD_DAMAGE: &str = "Instdamage";
/// Number of `k` indices a mod row carries.
pub const MOD_MODEL_SLOTS: usize = 10;
/// Mod sockets of the client's weapon (`Kinds`).
// EVIDENCE: cshell+0x235ec0 (three sockets of 0x2c bytes at weapon +0x14), cshell+0x60d90 (0xb2 / 0xb3 empty sockets
// 1, 0, 2 in this order)
pub const SOCKET_MUZZLE: u8 = 0;
pub const SOCKET_SIGHT: u8 = 1;
pub const SOCKET_MAGAZINE: u8 = 2;
/// The part byte position of each socket in `GS_WEAPON_GIVE` / `GS_WEAPON_EQUIP` (part 1 sight, part 2 muzzle,
/// part 3 magazine: the order of the defaults and of the client's socket clearing). The client files every part by
/// its own `Kinds`, so the position is a convention of ours, not a client rule.
// DISASM: cshell+0x60d90 (each part != -1 attached through slot +0x104 = cshell+0x235ec0, which takes the socket from
// the mod row)
// DC-DEFAULT: DSV-80 (proposed) part order; DSV-99 (proposed) the player's attachments in the same positions
pub const PART_OF_SOCKET: [usize; 3] = [1, 0, 2];

/// One `[Weapon<n>]` row as far as the server uses it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeaponRow {
    pub clip: Option<u32>,
    pub pellets: Option<u32>,
    /// The ammunition id named by the first ammunition name.
    pub ammo: Option<u16>,
    /// `Name` (matched by the weapon order).
    pub name: Option<String>,
    /// Third-person model, skins, render styles.
    pub hh_model: Option<String>,
    pub hh_skins: Vec<String>,
    pub hh_styles: Vec<String>,
    /// `IsAmmo` non-zero: the weapon is its own ammunition.
    pub own_ammo: bool,
    /// `ClientWeaponType` ([`CLIENT_KIND_HAND`]: thrown by hand).
    pub client_kind: Option<u32>,
    /// `Guntype` of the row (`None` when absent): mode-1 fire is honoured only for 0 / 0xd / 0xe.
    pub gun_type: Option<u32>,
    /// The row's default attachments as part bytes (scope, muzzle, magazine mod indices, finish; -1 = none);
    /// `None` when the row names no default attachment.
    // DC-DEFAULT: DSV-80 (proposed)
    pub default_parts: Option<[i8; 4]>,
    /// The mods the row lists (`ModName<k>` resolved to mod indices; unknown names dropped), in list order.
    pub mod_list: Vec<u16>,
    /// `(mod id, ModModel)` of every `ModName<i>` that names a known mod row.
    pub mod_models: Vec<(u16, i32)>,
    /// The shot cone's perturb bounds (`None` when the row has no `MinPerturb` / `MaxPerturb`).
    // DC-DEFAULT: DSV-102 (proposed)
    pub cone: Option<crate::cone::ConeBounds>,
    /// `AniType`: the weapon's animation family, the first index of the character animation table
    /// ([`crate::anims::FAMILIES`]).
    // DISASM: cshell+0x17bd73 (the weapon row reader cshell+0x17bd20 takes the key)
    // DC-DEFAULT: DSV-125 (proposed)
    pub ani_type: Option<u8>,
    /// `Range`: how far the client traces the weapon's shot (WEAPON_FIRE_2009.md §5.2; the knife row 105).
    pub range: Option<f32>,
    /// `SubControlleeWeaponID`: the weapon whose resting mines this one sets off (DSV-619; `None` for -1 or none).
    pub controllee: Option<u16>,
    /// A heartbeat-sensor row (`SensorOption` 0 or 1): its `ScanDist` (0 when missing); `None` for any other row.
    pub scan_dist: Option<f32>,
    /// The AI burst keys of the row (`None` unless all four are present): how a co-op AI holding it fires (DSV-640).
    pub ai_burst: Option<AiBurst>,
}

/// How an AI fires a weapon (DSV-640): a burst of `shots.0..=shots.1` shots, then a pause of `pause_s.0..=pause_s.1`
/// seconds before the next burst.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AiBurst {
    pub shots: (u32, u32),
    pub pause_s: (f32, f32),
}

/// The AI burst keys of a `[Weapon<n>]` row: the fewest and most shots of a burst, the shortest and longest pause
/// (seconds) after one.
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] AIMinBurstShots, AIMaxBurstShots, AIMinBurstInterval,
// AIMaxBurstInterval (six rows, the ones the co-op AI templates hold; the launcher and the bolt rifle rows give one
// shot per burst, the automatic rows more; values read at run time, 2026-11-08 scratch run)
pub const KEY_AI_BURST: [&str; 4] = [
    "AIMinBurstShots",
    "AIMaxBurstShots",
    "AIMinBurstInterval",
    "AIMaxBurstInterval",
];

/// The row's AI burst: all four keys present, the shot counts at least 1 (the larger end at least the smaller), the
/// pauses finite and not negative.
fn ai_burst(sec: &dc_bute::SectionMap) -> Option<AiBurst> {
    let [a, b, c, d] = KEY_AI_BURST.map(|k| num_f32(sec.get(k)));
    let (lo, hi, plo, phi) = (a?, b?, c?, d?);
    if ![lo, hi, plo, phi].iter().all(|v| v.is_finite()) || plo < 0.0 || phi < 0.0 {
        return None;
    }
    let lo = (lo.max(1.0) as u32).min(1_000);
    let hi = (hi.max(1.0) as u32).clamp(lo, 1_000);
    Some(AiBurst {
        shots: (lo, hi),
        pause_s: (plo.min(phi), plo.max(phi)),
    })
}

/// `[Weapon<n>]` key naming the weapon a controller sets off (the claymore controller row names the claymore).
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] SubControlleeWeaponID (read at run time)
// DISASM: cshell+0x17d155 (the weapon row reader: the key -> row +0x228, default -1)
pub const KEY_CONTROLLEE: &str = "SubControlleeWeaponID";

/// `[Weapon<n>]` animation family key.
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] AniType (read at run time)
pub const KEY_ANI_TYPE: &str = "AniType";

/// One `[Mod<n>]` row as far as the server uses it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModRow {
    /// `ShotsPerClipPlus<k>` / `MaxAmountPlus<k>` (0 when missing). Signed: a Fast Loader takes rounds away.
    // EVIDENCE: cshell+0x233430, cshell+0x233490 (plain signed adds)
    pub clip_plus: [i32; MOD_MODEL_SLOTS],
    pub max_plus: [i32; MOD_MODEL_SLOTS],
    /// `Instdamage` (0 when missing): DSV-440's damage term.
    pub damage: f32,
    /// `Kinds`: the socket the client files the mod in (`SOCKET_*`); `None` when missing or beyond 0..2.
    pub socket: Option<u8>,
    /// `ZoomLevel` (0 when missing): above 0 the client zooms with this mod in the sight socket.
    pub zoom_level: u32,
}

/// One `[Ammo<n>]` row.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AmmoRow {
    pub damage: Option<f32>,
    /// `Alt_InstDamage` (DSV-405).
    pub alt_damage: Option<f32>,
    pub damage_type: Option<u8>,
    pub max_amount: Option<u32>,
    pub area_damage: Option<f32>,
    pub area_damage_type: Option<u8>,
    pub area_radius: Option<f32>,
    /// `ProjectileFXName` (empty values dropped).
    pub projectile_fx: Option<String>,
    /// `ImpactFXName` (empty values dropped).
    pub impact_fx: Option<String>,
    /// `Type` (see [`KEY_AMMO_KIND`]).
    pub kind: Option<u8>,
    /// `ProgDamage`, `ProgDamageType`, `ProgDamageDuration`, `ProgDamageRadius`, `ProgDamageLifetime` (see
    /// [`KEY_PROG_DAMAGE`]).
    pub prog_damage: Option<f32>,
    pub prog_damage_type: Option<u8>,
    pub prog_duration: Option<f32>,
    pub prog_radius: Option<f32>,
    pub prog_lifetime: Option<f32>,
}

/// The progressive damage of an ammunition (a flame): damage per second, its damage type, the seconds one touch
/// burns and the reach round the burst point (all from the ammunition row).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgDamage {
    pub per_second: f32,
    pub damage_type: Option<u8>,
    pub duration_s: f32,
    pub radius: f32,
}

/// The blast of an ammunition: damage at the centre, damage type and radius (all three from the ammunition row).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AreaDamage {
    pub damage: f32,
    pub damage_type: Option<u8>,
    pub radius: f32,
}

/// The weapon table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeaponTable {
    pub weapons: BTreeMap<u16, WeaponRow>,
    pub ammo: BTreeMap<u16, AmmoRow>,
    /// The client's weapon command table: position -> weapon number (`WEAPON_COMMAND_BASE` + position = command).
    pub commands: Vec<u16>,
    /// A variant row's base gun (VAR-1): a variant has no entry of its own in the command table and uses its base gun's.
    pub variants: BTreeMap<u16, u16>,
    /// End position (exclusive) in `commands` of each weapon class of the order section, in class order.
    pub class_ends: Vec<usize>,
    /// The `[Gear<n>]` armour rows by n.
    pub gear: BTreeMap<u16, GearProtection>,
    /// The `[Mod<n>]` rows by n.
    pub mods: BTreeMap<u16, ModRow>,
}

fn numbered(sec: &dc_bute::SectionMap, key: &str, max: u32) -> Vec<String> {
    (0..max)
        .map_while(|k| {
            sec.get(&format!("{key}{k}"))
                .map(|v| unquote(v).to_string())
                .filter(|v| !v.is_empty())
        })
        .collect()
}

fn index_of(section: &str, prefix: &str) -> Option<u16> {
    let rest = section.strip_prefix(prefix)?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn num_f32(v: Option<&String>) -> Option<f32> {
    let t = v?.trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

fn num_u32(v: Option<&String>) -> Option<u32> {
    num_f32(v).filter(|x| *x >= 0.0).map(|x| x as u32)
}

fn num_i32(v: Option<&String>) -> Option<i32> {
    num_f32(v).map(|x| x as i32)
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"').trim()
}

/// The perturb bounds of a weapon row; each key read as the client reads an integer field of the row (the text's
/// number, truncated).
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-2.1
fn cone_bounds(sec: &dc_bute::SectionMap) -> Option<crate::cone::ConeBounds> {
    let int = |k: &str| num_f32(sec.get(k)).map(|v| v as i32);
    Some(crate::cone::ConeBounds {
        min: int(KEY_MIN_PERTURB)?,
        max: int(KEY_MAX_PERTURB)?,
        zoomed_min: int(KEY_ZOOMED_MIN_PERTURB),
        zoomed_max: int(KEY_ZOOMED_MAX_PERTURB),
        triple_min: int(KEY_TRIPLE_MIN_PERTURB),
        triple_max: int(KEY_TRIPLE_MAX_PERTURB),
    })
}

impl WeaponTable {
    /// The shot cone's bounds of a weapon row.
    pub fn cone_bounds(&self, weapon: u16) -> Option<&crate::cone::ConeBounds> {
        self.weapons.get(&weapon)?.cone.as_ref()
    }

    /// Builds the table from a parsed document.
    pub fn from_doc(doc: &ButeDoc) -> Self {
        let mut t = WeaponTable::default();
        let mut ammo_names: BTreeMap<String, u16> = BTreeMap::new();
        // mod name -> index; the lowest index wins a duplicate name (the client's lookup returns the first match)
        // EVIDENCE: cshell+0x172510 (linear search over the mod table, _stricmp on the name)
        let mut mod_names: BTreeMap<String, u16> = BTreeMap::new();
        for (name, sec) in doc.iter() {
            if let Some(n) = index_of(name, MOD_SECTION) {
                if let Some(nm) = sec.get(KEY_NAME) {
                    let key = unquote(nm).to_ascii_lowercase();
                    let e = mod_names.entry(key).or_insert(n);
                    *e = (*e).min(n);
                }
                t.mods.insert(
                    n,
                    ModRow {
                        socket: num_u32(sec.get(KEY_KINDS))
                            .and_then(|k| u8::try_from(k).ok())
                            .filter(|k| *k <= SOCKET_MAGAZINE),
                        zoom_level: num_u32(sec.get(KEY_ZOOM_LEVEL)).unwrap_or(0),
                        clip_plus: std::array::from_fn(|k| {
                            num_i32(sec.get(&format!("{KEY_CLIP_PLUS}{k}"))).unwrap_or(0)
                        }),
                        max_plus: std::array::from_fn(|k| {
                            num_i32(sec.get(&format!("{KEY_MAX_PLUS}{k}"))).unwrap_or(0)
                        }),
                        damage: num_f32(sec.get(KEY_MOD_DAMAGE)).unwrap_or(0.0),
                    },
                );
            }
        }
        for (name, sec) in doc.iter() {
            if let Some(n) = index_of(name, AMMO_SECTION) {
                t.ammo.insert(
                    n,
                    AmmoRow {
                        damage: num_f32(sec.get(KEY_DAMAGE)),
                        alt_damage: num_f32(sec.get(KEY_ALT_DAMAGE)),
                        damage_type: num_u32(sec.get(KEY_DAMAGE_TYPE))
                            .and_then(|v| u8::try_from(v).ok()),
                        max_amount: num_u32(sec.get(KEY_MAX_AMOUNT)),
                        area_damage: num_f32(sec.get(KEY_AREA_DAMAGE)),
                        area_damage_type: num_u32(sec.get(KEY_AREA_DAMAGE_TYPE))
                            .and_then(|v| u8::try_from(v).ok()),
                        area_radius: num_f32(sec.get(KEY_AREA_RADIUS)),
                        projectile_fx: sec
                            .get(KEY_PROJECTILE_FX)
                            .map(|v| unquote(v).to_string())
                            .filter(|v| !v.is_empty()),
                        impact_fx: sec
                            .get(KEY_IMPACT_FX)
                            .map(|v| unquote(v).to_string())
                            .filter(|v| !v.is_empty()),
                        kind: num_u32(sec.get(KEY_AMMO_KIND)).and_then(|v| u8::try_from(v).ok()),
                        prog_damage: num_f32(sec.get(KEY_PROG_DAMAGE)),
                        prog_damage_type: num_u32(sec.get(KEY_PROG_DAMAGE_TYPE))
                            .and_then(|v| u8::try_from(v).ok()),
                        prog_duration: num_f32(sec.get(KEY_PROG_DURATION)),
                        prog_radius: num_f32(sec.get(KEY_PROG_RADIUS)),
                        prog_lifetime: num_f32(sec.get(KEY_PROG_LIFETIME)),
                    },
                );
                if let Some(nm) = sec.get(KEY_NAME) {
                    ammo_names.insert(unquote(nm).to_ascii_lowercase(), n);
                }
            }
            if let Some(n) = index_of(name, GEAR_SECTION) {
                let slots = (0..GEAR_SLOTS_MAX)
                    .map_while(|i| {
                        let ty = num_u32(sec.get(&format!("{KEY_PROTECTION_TYPE}{i}")))
                            .and_then(|v| u8::try_from(v).ok())?;
                        let share = KEY_NODE_PROTECTION.map(|k| {
                            num_f32(sec.get(&format!("{k}{i}")))
                                .unwrap_or(0.0)
                                .clamp(0.0, 1.0)
                        });
                        Some((ty, share))
                    })
                    .collect();
                t.gear.insert(
                    n,
                    GearProtection {
                        armor: num_f32(sec.get(KEY_ARMOR)).filter(|a| *a >= 0.0),
                        slots,
                    },
                );
            }
        }
        for (name, sec) in doc.iter() {
            if let Some(n) = index_of(name, WEAPON_SECTION) {
                let ammo = sec
                    .get(KEY_AMMO_NAME0)
                    .and_then(|a| ammo_names.get(&unquote(a).to_ascii_lowercase()).copied());
                // DC-DEFAULT: DSV-80 (proposed) part k = the mod index of the k-th default attachment (-1 when the
                // key is empty, the name unknown or the index beyond a part byte); finish -1
                let mut parts = [crate::gs::NO_PART; 4];
                for (k, key) in KEY_DEFAULT_ATTACH.iter().enumerate() {
                    parts[k] = sec
                        .get(key)
                        .map(|v| unquote(v).to_ascii_lowercase())
                        .filter(|v| !v.is_empty())
                        .and_then(|v| mod_names.get(&v).copied())
                        .and_then(|m| i8::try_from(m).ok())
                        .unwrap_or(crate::gs::NO_PART);
                }
                let default_parts = parts
                    .iter()
                    .any(|p| *p != crate::gs::NO_PART)
                    .then_some(parts);
                let mod_names_of_row = numbered(sec, KEY_MOD_NAME, 64);
                let mod_models: Vec<(u16, i32)> = mod_names_of_row
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| {
                        let m = mod_names.get(&v.to_ascii_lowercase()).copied()?;
                        let k = sec
                            .get(&format!("{KEY_MOD_MODEL}{i}"))
                            .and_then(|v| unquote(v).trim().parse::<i32>().ok())
                            .unwrap_or(-1);
                        Some((m, k))
                    })
                    .collect();
                let mod_list = mod_models.iter().map(|(m, _)| *m).collect();
                let of = sec
                    .get(dc_bute::modpatch::KEY_VARIANT_OF)
                    .map(|v| v.as_str());
                if let Some(base) = dc_bute::modpatch::variant_base(of) {
                    t.variants.insert(n, base);
                }
                t.weapons.insert(
                    n,
                    WeaponRow {
                        clip: num_u32(sec.get(KEY_CLIP)),
                        pellets: num_u32(sec.get(KEY_PELLETS)),
                        ammo,
                        name: sec.get(KEY_NAME).map(|v| unquote(v).to_string()),
                        hh_model: sec
                            .get(KEY_HH_MODEL)
                            .map(|v| unquote(v).to_string())
                            .filter(|v| !v.is_empty()),
                        hh_skins: numbered(sec, KEY_HH_SKIN, 8),
                        hh_styles: numbered(sec, KEY_HH_STYLE, 4),
                        own_ammo: num_u32(sec.get(KEY_IS_AMMO)).is_some_and(|v| v != 0),
                        client_kind: num_u32(sec.get(KEY_CLIENT_KIND)),
                        gun_type: num_u32(sec.get(KEY_GUN_TYPE)),
                        default_parts,
                        mod_list,
                        mod_models,
                        cone: cone_bounds(sec),
                        ani_type: num_u32(sec.get(KEY_ANI_TYPE)).and_then(|v| u8::try_from(v).ok()),
                        range: num_f32(sec.get(KEY_RANGE)).filter(|r| r.is_finite() && *r > 0.0),
                        controllee: num_f32(sec.get(KEY_CONTROLLEE))
                            .filter(|v| (0.0..=f32::from(u16::MAX)).contains(v))
                            .map(|v| v as u16),
                        scan_dist: num_f32(sec.get(KEY_SENSOR_OPTION))
                            .filter(|o| *o == 0.0 || *o == 1.0)
                            .map(|_| {
                                num_f32(sec.get(KEY_SCAN_DIST))
                                    .filter(|d| d.is_finite() && *d > 0.0)
                                    .unwrap_or(0.0)
                            }),
                        ai_burst: ai_burst(sec),
                    },
                );
            }
        }
        // the command table: classes in order, each class's weapon names in order, matched by row name
        // EVIDENCE: cshell+0x180590 (class loop, then entry loop with a name lookup per entry)
        if let Some(order) = doc.get(WEAPON_ORDER_SECTION) {
            let by_name: BTreeMap<String, u16> = t
                .weapons
                .iter()
                // VAR-1: a variant carries its base gun's name and must not take the base gun's entry
                .filter(|(n, _)| !t.variants.contains_key(n))
                .filter_map(|(n, w)| w.name.as_ref().map(|nm| (dc_bute::fold_latin1(nm), *n)))
                .collect();
            for k in 0..64u32 {
                let Some(class) = order
                    .get(&format!("{KEY_CLASS}{k}"))
                    .map(|v| unquote(v).to_string())
                else {
                    break;
                };
                for i in 0..4096u32 {
                    let Some(name) = order.get(&format!("{class}{i}")) else {
                        break;
                    };
                    if let Some(w) = by_name.get(&dc_bute::fold_latin1(unquote(name))) {
                        t.commands.push(*w);
                    }
                }
                t.class_ends.push(t.commands.len());
            }
        }
        t
    }

    /// The gun a weapon row switches and sorts as: a variant's base gun, else the row itself (VAR-1).
    pub fn base_of(&self, weapon: u16) -> u16 {
        self.variants.get(&weapon).copied().unwrap_or(weapon)
    }

    /// `GS_WEAPON_SWITCH` command of a weapon (`None` when the weapon is not in the command table).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0x7f); EVIDENCE: cshell+0x180590
    pub fn command_of(&self, weapon: u16) -> Option<u8> {
        let weapon = self.base_of(weapon);
        let i = self.commands.iter().position(|w| *w == weapon)? as u32;
        u8::try_from(WEAPON_COMMAND_BASE + i).ok()
    }

    /// The weapon class of a weapon: the index of the order-section class whose block of the command table holds it
    /// (the client derives the class the same way, from the position in the whole table, so weapons beyond the 8-bit
    /// command range have a class too).
    // EVIDENCE: cshell+0x47da0 (position of the weapon in weapon_db +0x15dc, then the first class end above it from the
    // table at +0x15e8); cshell+0x180590 (the table is built class by class)
    pub fn class_of(&self, weapon: u16) -> Option<usize> {
        let weapon = self.base_of(weapon);
        let i = self.commands.iter().position(|w| *w == weapon)?;
        self.class_ends.iter().position(|&end| i < end)
    }

    /// Part bytes of `GS_WEAPON_GIVE` / `GS_WEAPON_EQUIP` for a weapon: its default attachments (-1 x 4 without
    /// any, or for an unknown weapon).
    // DISASM: cshell+0x60d90 (0xb2 / 0xb3 empty the three sockets, then attach only the parts != -1), cshell+0x1f4180
    // (zoom needs a mod with ZoomLevel > 0 in socket 1)
    // DC-DEFAULT: DSV-80 (proposed)
    pub fn default_parts(&self, weapon: u16) -> [i8; 4] {
        self.weapons
            .get(&weapon)
            .and_then(|w| w.default_parts)
            .unwrap_or([crate::gs::NO_PART; 4])
    }

    /// Part bytes of a grant with the player's own attachments: `chosen` = the mod index per socket (`SOCKET_*`
    /// order: muzzle, sight, magazine), `None` = nothing chosen for that socket. A chosen mod is granted when the mod
    /// row exists, its `Kinds` is that socket, the weapon row lists it (`ModName<k>`) and the index fits a part byte;
    /// otherwise, and for every socket without a choice, the row's default part stays (DSV-80).
    // DISASM: cshell+0x60d90 (0xb2 / 0xb3: sockets emptied, each part != -1 attached), cshell+0x235ec0 (the socket is
    // the mod row's Kinds; no weapon check in the client)
    // DC-DEFAULT: DSV-99 (proposed) the player's attachments over the defaults; DSV-100 (proposed) only mods the weapon
    // row lists, in their own socket
    pub fn player_parts(&self, weapon: u16, chosen: [Option<u16>; 3]) -> [i8; 4] {
        let mut parts = self.default_parts(weapon);
        let Some(row) = self.weapons.get(&weapon) else {
            return parts;
        };
        for (socket, m) in chosen.iter().enumerate() {
            let Some(m) = *m else {
                continue;
            };
            let fits = self
                .mods
                .get(&m)
                .is_some_and(|r| r.socket == Some(socket as u8))
                && row.mod_list.contains(&m);
            if let (true, Ok(b)) = (fits, i8::try_from(m)) {
                parts[PART_OF_SOCKET[socket]] = b;
            }
        }
        parts
    }

    /// The magazine bonus of a weapon granted with these parts: (rounds added to the clip, rounds added to the
    /// carried maximum) = the magazine-socket mod's `ShotsPerClipPlus<k>` / `MaxAmountPlus<k>` with `k` the weapon
    /// row's `ModModel` for that mod; (0, 0) without such a mod or with `k` outside 0..9. Signed (a Fast Loader's
    /// entries are negative).
    // EVIDENCE: cshell+0x233430 (clip size = row ShotsPerClip + the socket-2 mod's ShotsPerClipPlus[ModModel]),
    // cshell+0x233490 (maximum = ammunition MaxAmount + MaxAmountPlus[ModModel]), cshell+0x172b20, cshell+0x172df0,
    // cshell+0x172e10
    pub fn magazine_bonus(&self, weapon: u16, parts: &[i8; 4]) -> (i32, i32) {
        let Some(row) = self.weapons.get(&weapon) else {
            return (0, 0);
        };
        let magazine = parts[..3]
            .iter()
            .filter(|p| **p >= 0)
            .map(|p| *p as u16)
            .rfind(|m| {
                self.mods
                    .get(m)
                    .is_some_and(|r| r.socket == Some(SOCKET_MAGAZINE))
            });
        let Some(m) = magazine else {
            return (0, 0);
        };
        let Some(k) = row
            .mod_models
            .iter()
            .find(|(id, _)| *id == m)
            .and_then(|(_, k)| usize::try_from(*k).ok())
            .filter(|k| *k < MOD_MODEL_SLOTS)
        else {
            return (0, 0);
        };
        let r = &self.mods[&m];
        (r.clip_plus[k], r.max_plus[k])
    }

    /// The damage factor of a weapon granted with these parts: 1 + the `Instdamage` of the mods filed in its three
    /// sockets (a later part of the same socket wins, as the client files them), at least 0. A suppressor's -0.1
    /// makes 0.9.
    // DISASM: cshell+0x235ec0 (the socket is the mod row's Kinds)
    // DC-DEFAULT: DSV-440 (proposed) the server scales a hit by the shooter's granted mods (the 2009 server rule is
    // not in the client)
    pub fn mod_damage_factor(&self, parts: &[i8; 4]) -> f32 {
        let mut sockets: [Option<f32>; 3] = [None; 3];
        for p in parts[..3].iter().filter(|p| **p >= 0) {
            if let Some(r) = self.mods.get(&(*p as u16)) {
                if let Some(s) = r.socket.filter(|s| *s <= SOCKET_MAGAZINE) {
                    sockets[usize::from(s)] = Some(r.damage);
                }
            }
        }
        (1.0 + sockets.iter().flatten().sum::<f32>()).max(0.0)
    }

    /// Whether a weapon granted with these parts zooms in the 2009 client: a mod in the sight socket whose
    /// `ZoomLevel` is above 0 (the client files each part by its `Kinds`; a later part of the same socket wins).
    // EVIDENCE: cshell+0x176f10 (ZoomLevel -> row +0x110); DISASM: cshell+0x230a60 (the mod of a socket),
    // cshell+0x1f4180 (zoom reads the socket-1 mod), cshell+0x235ec0
    pub fn zooms_with(&self, parts: &[i8; 4]) -> bool {
        let sight = parts[..3]
            .iter()
            .rev()
            .filter(|p| **p >= 0)
            .filter_map(|p| self.mods.get(&(*p as u16)))
            .find(|r| r.socket == Some(SOCKET_SIGHT));
        sight.is_some_and(|r| r.zoom_level > 0)
    }

    /// The shot record's mod bytes `e`, `f`, `g` of a weapon granted with these parts: the `[Mod<n>]` number the
    /// client files in each socket (muzzle, sight, magazine; a later part of the same socket wins, as the client's
    /// filing), [`crate::gs::SHOT_MOD_NONE`] for an empty socket.
    // DISASM: cshell+0x235ec0 (the socket is the mod row's Kinds), cshell+0x2c6a7e..0x2c6a96 (the record's three mod
    // numbers); EVIDENCE: cshell+0x2c6580 (the first is the muzzle mod)
    // DC-DEFAULT: DSV-167 (proposed)
    pub fn shot_mods(&self, parts: &[i8; 4]) -> [u8; 3] {
        let mut out = crate::gs::NO_SHOT_MODS;
        for p in parts[..3].iter().filter(|p| **p >= 0) {
            let socket = self.mods.get(&(*p as u16)).and_then(|r| r.socket);
            if let Some(s) = socket.filter(|s| *s <= SOCKET_MAGAZINE) {
                out[usize::from(s)] = *p as u8;
            }
        }
        out
    }

    /// Third-person model, first skin and first style of a weapon.
    /// The lowest weapon number whose row `Name` has the key `key` (the name as the row spells it).
    pub fn by_name_key(&self, key: crate::chars::NameKey) -> Option<u16> {
        self.weapons
            .iter()
            .find(|(_, r)| {
                r.name
                    .as_deref()
                    .is_some_and(|n| crate::chars::name_key(n.as_bytes()) == key)
            })
            .map(|(n, _)| *n)
    }

    pub fn hand_model(&self, weapon: u16) -> Option<&WeaponRow> {
        self.weapons.get(&weapon).filter(|w| w.hh_model.is_some())
    }

    /// Reads the table from the user's ROM.
    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, String> {
        match ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, WEAPONS_ENTRY) {
            Ok(Some(d)) => Ok(Self::from_doc(&d)),
            Ok(None) => Err("weapon table entry missing".into()),
            Err(e) => Err(format!("weapon table: {e}")),
        }
    }

    /// The table of a weapon document after the mod set's content (MOD-21) is applied to it: every row, damage,
    /// clip and mod value downstream sees the patched keys. A refused patch (a missing row, a taken register id)
    /// is an error and nothing is applied.
    // DC-DEFAULT: DSV-478 (proposed), MOD-21 (proposed)
    pub fn from_doc_with_content(
        doc: &ButeDoc,
        content: &dc_bute::modpatch::WeaponContent,
    ) -> Result<Self, String> {
        let mut d = doc.clone();
        content.apply_to_doc(&mut d).map_err(|e| e.to_string())?;
        Ok(Self::from_doc(&d))
    }

    /// [`WeaponTable::from_vfs`] with the mod set's content applied.
    pub fn from_vfs_with_content(
        vfs: &RezVfs,
        content: &dc_bute::modpatch::WeaponContent,
    ) -> Result<Self, String> {
        match ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, WEAPONS_ENTRY) {
            Ok(Some(mut d)) => {
                content.apply_to_doc(&mut d).map_err(|e| e.to_string())?;
                Ok(Self::from_doc(&d))
            }
            Ok(None) => Err("weapon table entry missing".into()),
            Err(e) => Err(format!("weapon table: {e}")),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.weapons.is_empty() && self.ammo.is_empty()
    }

    /// Damage of one hit by weapon `weapon_id` with ammunition `ammo_id`.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1, §12.3 DC-DEFAULT: DC-C2S-11 (proposed) ROM weapon values at run time
    pub fn damage(&self, weapon_id: u16, ammo_id: u16) -> Option<f32> {
        let by_ammo = || self.ammo.get(&ammo_id).and_then(|a| a.damage);
        let by_weapon = || {
            self.weapons
                .get(&weapon_id)
                .and_then(|w| w.ammo)
                .and_then(|a| self.ammo.get(&a))
                .and_then(|a| a.damage)
        };
        if TODO_DAMAGE_FROM_AMMO_ROW {
            by_ammo().or_else(by_weapon)
        } else {
            by_weapon().or_else(by_ammo)
        }
    }

    /// Damage of one hit of a shot: the alternate fire (the 0x82 `mode1` bit) takes the ammunition row's
    /// `Alt_InstDamage` when it has one, else the shot's [`WeaponTable::damage`].
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (`mode1` = the weapon's fire mode 1); DISASM: cshell+0x1f8b40 (mode 1 = the
    // zoom bit of a Guntype 0 / 0xd / 0xe weapon without the fire bit)
    // DC-DEFAULT: DSV-405 (proposed) the alternate fire's damage is `Alt_InstDamage` of the ammunition the damage comes
    // from (the 2009 server rule is not in the client)
    pub fn shot_damage(&self, weapon_id: u16, ammo_id: u16, alt: bool) -> Option<f32> {
        // the mode-1 bit counts only for a row the 2009 client fires in mode 1 (Guntype 0 / 0xd / 0xe): a modified
        // client cannot claim a rifle's alternate damage (review of match-r15, 2026-10-29)
        let alt = alt && self.alt_capable(weapon_id);
        if alt {
            let row = |a: u16| self.ammo.get(&a).and_then(|r| r.alt_damage);
            let by_weapon = || {
                self.weapons
                    .get(&weapon_id)
                    .and_then(|w| w.ammo)
                    .and_then(row)
            };
            let d = if TODO_DAMAGE_FROM_AMMO_ROW {
                row(ammo_id).or_else(by_weapon)
            } else {
                by_weapon().or_else(|| row(ammo_id))
            };
            if d.is_some() {
                return d;
            }
        }
        self.damage(weapon_id, ammo_id)
    }

    /// [`WeaponTable::shot_damage`] of a weapon granted with `parts`: scaled by [`WeaponTable::mod_damage_factor`].
    /// The parts are the server's own grant record, never the shot's claim, so a modified client cannot shed a
    /// suppressor's -0.1 or claim a bigger term than its fitted rows hold.
    // DC-DEFAULT: DSV-440 (proposed)
    pub fn shot_damage_with_mods(
        &self,
        weapon_id: u16,
        ammo_id: u16,
        alt: bool,
        parts: &[i8; 4],
    ) -> Option<f32> {
        self.shot_damage(weapon_id, ammo_id, alt)
            .map(|d| d * self.mod_damage_factor(parts))
    }

    /// Whether `weapon_id` is a row the 2009 client fires in mode 1 (`Guntype` 0 melee, 0xd double barrel, 0xe
    /// snowballs; DISASM cshell+0x1f8b40).
    pub fn alt_capable(&self, weapon_id: u16) -> bool {
        self.weapons
            .get(&weapon_id)
            .and_then(|w| w.gun_type)
            .is_some_and(|g| matches!(g, 0 | 0xd | 0xe))
    }

    /// Damage type of a hit by `weapon_id` with `ammo_id` (the ammunition row, else the weapon's first ammunition).
    // SPEC: CAPTURE_tdm4_KILL_VALIDATION.md §3.1 item 3, §8.1 S6
    /// The row `Range` of `weapon_id` (`None` without a positive value).
    pub fn range(&self, weapon_id: u16) -> Option<f32> {
        self.weapons.get(&weapon_id).and_then(|w| w.range)
    }

    pub fn damage_type(&self, weapon_id: u16, ammo_id: u16) -> Option<u8> {
        self.ammo
            .get(&ammo_id)
            .and_then(|a| a.damage_type)
            .or_else(|| {
                self.weapons
                    .get(&weapon_id)
                    .and_then(|w| w.ammo)
                    .and_then(|a| self.ammo.get(&a))
                    .and_then(|a| a.damage_type)
            })
    }

    /// The AI burst of a weapon row (DSV-640; `None` without the four keys).
    pub fn ai_burst(&self, weapon_id: u16) -> Option<AiBurst> {
        self.weapons.get(&weapon_id).and_then(|w| w.ai_burst)
    }

    /// Clip size of a weapon.
    pub fn clip(&self, weapon_id: u16) -> Option<u32> {
        self.weapons.get(&weapon_id).and_then(|w| w.clip)
    }

    /// Whether a weapon is its own ammunition (`IsAmmo`).
    pub fn is_own_ammo(&self, weapon_id: u16) -> bool {
        self.weapons.get(&weapon_id).is_some_and(|w| w.own_ammo)
    }

    /// The first ammunition id of a weapon.
    pub fn ammo_of(&self, weapon_id: u16) -> Option<u16> {
        self.weapons.get(&weapon_id).and_then(|w| w.ammo)
    }

    /// Largest carried amount of an ammunition.
    pub fn max_amount(&self, ammo_id: u16) -> Option<u32> {
        self.ammo.get(&ammo_id).and_then(|a| a.max_amount)
    }

    /// The ammunition row of a fire: `ammo_id`, else the weapon's first ammunition.
    fn ammo_row(&self, weapon_id: u16, ammo_id: u16) -> Option<&AmmoRow> {
        self.ammo.get(&ammo_id).or_else(|| {
            self.weapons
                .get(&weapon_id)
                .and_then(|w| w.ammo)
                .and_then(|a| self.ammo.get(&a))
        })
    }

    /// The blast of `weapon_id` / `ammo_id`: `None` unless the row has a positive area damage and radius.
    // OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] AreaDamage, AreaDamageType, AreaDamageRadius
    pub fn area(&self, weapon_id: u16, ammo_id: u16) -> Option<AreaDamage> {
        let a = self.ammo_row(weapon_id, ammo_id)?;
        let damage = a.area_damage.filter(|d| *d > 0.0)?;
        let radius = a.area_radius.filter(|r| *r > 0.0)?;
        Some(AreaDamage {
            damage,
            damage_type: a.area_damage_type,
            radius,
        })
    }

    /// The progressive damage (a flame) of `weapon_id` / `ammo_id`: `None` unless the row has a positive
    /// `ProgDamage`, `ProgDamageDuration` and `ProgDamageRadius` and no blast ([`WeaponTable::area`] is `None`).
    // OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] ProgDamage, ProgDamageType, ProgDamageDuration, ProgDamageRadius,
    // AreaDamage (the flamethrower rows: progressive damage, area damage 0)
    // DC-DEFAULT: DSV-455 (proposed) a launched round with progressive damage and no blast is a flame
    pub fn prog(&self, weapon_id: u16, ammo_id: u16) -> Option<ProgDamage> {
        if self.area(weapon_id, ammo_id).is_some() {
            return None;
        }
        let a = self.ammo_row(weapon_id, ammo_id)?;
        let pos = |v: Option<f32>| v.filter(|x| x.is_finite() && *x > 0.0);
        Some(ProgDamage {
            per_second: pos(a.prog_damage)?,
            damage_type: a.prog_damage_type,
            duration_s: pos(a.prog_duration)?,
            radius: pos(a.prog_radius)?,
        })
    }

    /// `ProgDamageLifetime` of `weapon_id` / `ammo_id` in seconds (how long its progressive damage lingers where it
    /// went off: a dead AI's cloud, DSV-600), when positive.
    // OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] ProgDamageLifetime
    pub fn prog_lifetime(&self, weapon_id: u16, ammo_id: u16) -> Option<f32> {
        self.ammo_row(weapon_id, ammo_id)?
            .prog_lifetime
            .filter(|v| v.is_finite() && *v > 0.0)
    }

    /// Whether `weapon_id` / `ammo_id` fires a projectile (a launcher's rocket): ammunition `Type` 1 with a
    /// projectile row. The client sends such a shot without a hit; the server flies it (DSV-181).
    pub fn fires_projectile(&self, weapon_id: u16, ammo_id: u16) -> bool {
        self.ammo_row(weapon_id, ammo_id)
            .is_some_and(|a| a.kind == Some(AMMO_KIND_PROJECTILE) && a.projectile_fx.is_some())
    }

    /// Whether `weapon_id` / `ammo_id` is a thrown round (ammunition `Type` 2): its `UP_FIRE` is always a throw's
    /// release, whatever the time since the last `UP_THROW_HOLD` (DSV-29 amendment).
    // DC-DEFAULT: DSV-29 (amendment, proposed)
    pub fn throws_round(&self, weapon_id: u16, ammo_id: u16) -> bool {
        self.ammo_row(weapon_id, ammo_id)
            .is_some_and(|a| a.kind == Some(AMMO_KIND_THROWN))
    }

    /// Whether `weapon_id` is thrown by hand (`ClientWeaponType` [`CLIENT_KIND_HAND`]): its release is a throw on
    /// the client's side (the callout, no shot record, the count mirror of DSV-39 Q9) whatever its round's flight.
    // DC-DEFAULT: DSV-650 (amendment, proposed) the hand decides the bookkeeping, the class only the flight
    pub fn hand_thrown(&self, weapon_id: u16) -> bool {
        self.weapons
            .get(&weapon_id)
            .is_some_and(|w| w.client_kind == Some(CLIENT_KIND_HAND))
    }

    /// The weapon whose mines `weapon_id` sets off (its `SubControlleeWeaponID`): the claymore controller's claymore.
    pub fn controllee(&self, weapon_id: u16) -> Option<u16> {
        self.weapons.get(&weapon_id)?.controllee
    }

    /// The projectile row name of `weapon_id` / `ammo_id`.
    // OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] ProjectileFXName
    pub fn projectile_fx(&self, weapon_id: u16, ammo_id: u16) -> Option<&str> {
        self.ammo_row(weapon_id, ammo_id)?.projectile_fx.as_deref()
    }
}

/// One `[ProjectileFX<k>]` row as far as the server uses it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProjectileRow {
    pub model: Option<String>,
    pub skin: Option<String>,
    pub scale: Option<[f32; 3]>,
    /// `Gravity` (false = the projectile flies straight).
    pub gravity: Option<bool>,
    pub gravity_override: Option<f32>,
    /// `Lifetime` in seconds.
    pub lifetime_s: Option<f32>,
    pub velocity: Option<f32>,
    /// `Class` (the object class the row's round is).
    pub class: Option<String>,
    /// The flight the row's `Class` names ([`class_flight`]); `None` keeps the ammunition `Type` rule.
    pub flight: Option<RoundFlight>,
    /// `ClassData` (the name of a class-data row; a mine's names a [`TRAP_SECTION`] row).
    pub class_data: Option<String>,
    /// The lowest and the highest corner of the row's `Model` mesh (model space, bind pose), read for a mine's row
    /// only (DSV-614, DSV-618): where it sits in the ground and where a shot hits it.
    pub model_box: Option<([f32; 3], [f32; 3])>,
}

/// One `[LandTrapClassData<n>]` row: what a mine of a projectile row naming it does once it lies.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrapRow {
    /// `DetonateRadius` (units), when the row has it.
    pub detonate_radius: Option<f32>,
    /// `ArmDelay` (seconds), when the row has it.
    pub arm_delay_s: Option<f32>,
}

impl TrapRow {
    /// The distance at which the mine goes off: the row's `DetonateRadius`, else the 2009 client's default.
    pub fn radius(&self) -> f32 {
        self.detonate_radius
            .filter(|r| r.is_finite() && *r > 0.0)
            .unwrap_or(TRAP_DEFAULT_DETONATE_RADIUS)
    }

    /// The arming delay in milliseconds: the row's `ArmDelay`, else the 2009 client's default.
    pub fn arm_ms(&self) -> u32 {
        let s = self
            .arm_delay_s
            .filter(|s| s.is_finite() && *s >= 0.0 && *s < 600.0)
            .unwrap_or(TRAP_DEFAULT_ARM_DELAY_S);
        (s * 1000.0).round() as u32
    }
}

/// The projectile rows of the FX table, by folded `Name`, and the mine class-data rows by folded `Name`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProjectileTable {
    pub rows: BTreeMap<String, ProjectileRow>,
    pub traps: BTreeMap<String, TrapRow>,
}

/// The lowest and the highest corner of the first LOD of every piece of a model (vertex positions are stored in
/// the bind pose's model space); `None` for a model without a vertex.
// SPEC: FMT_LTB.md §5 (piece, LOD chain), §6 (render-object bodies, vertex streams)
pub fn mesh_box(m: &dc_model::Model) -> Option<([f32; 3], [f32; 3])> {
    let mut out: Option<([f32; 3], [f32; 3])> = None;
    for lod in m.pieces.iter().filter_map(|p| p.lods.first()) {
        let mesh = &lod.mesh;
        for (i, st) in mesh.streams().iter().enumerate() {
            let Some(layout) = mesh.layout(i) else {
                continue;
            };
            for v in 0..mesh.vertex_count() as usize {
                let Some(q) = st
                    .position(&layout, v)
                    .filter(|q| q.iter().all(|x| x.is_finite()))
                else {
                    continue;
                };
                let (lo, hi) = out.get_or_insert((q, q));
                for k in 0..3 {
                    lo[k] = lo[k].min(q[k]);
                    hi[k] = hi[k].max(q[k]);
                }
            }
        }
    }
    out
}

/// `<x, y, z>` (the table's vector form).
fn num_vec3(v: Option<&String>) -> Option<[f32; 3]> {
    let t = unquote(v?);
    let t = t.strip_prefix('<')?.strip_suffix('>')?;
    let mut out = [0.0f32; 3];
    let mut it = t.split(',');
    for o in &mut out {
        *o = it
            .next()?
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|x| x.is_finite())?;
    }
    it.next().is_none().then_some(out)
}

impl ProjectileTable {
    /// Builds the rows from a parsed FX table (rows without a `Name` are skipped).
    pub fn from_doc(doc: &ButeDoc) -> Self {
        let mut t = ProjectileTable::default();
        for (name, sec) in doc.iter() {
            if index_of(name, TRAP_SECTION).is_some() {
                if let Some(n) = sec.get(KEY_NAME).map(|v| unquote(v).to_string()) {
                    t.traps.insert(
                        dc_bute::fold_latin1(&n),
                        TrapRow {
                            detonate_radius: num_f32(sec.get(KEY_TRAP_DETONATE_RADIUS)),
                            arm_delay_s: num_f32(sec.get(KEY_TRAP_ARM_DELAY)),
                        },
                    );
                }
                continue;
            }
            if index_of(name, PROJECTILE_SECTION).is_none() {
                continue;
            }
            let Some(n) = sec.get(KEY_NAME).map(|v| unquote(v).to_string()) else {
                continue;
            };
            let text = |k: &str| {
                sec.get(k)
                    .map(|v| unquote(v).to_string())
                    .filter(|v| !v.is_empty())
            };
            t.rows.insert(
                dc_bute::fold_latin1(&n),
                ProjectileRow {
                    model: text(KEY_PFX_MODEL),
                    skin: text(KEY_PFX_SKIN),
                    scale: num_vec3(sec.get(KEY_PFX_SCALE)),
                    gravity: num_f32(sec.get(KEY_PFX_GRAVITY)).map(|g| g != 0.0),
                    gravity_override: num_f32(sec.get(KEY_PFX_GRAVITY_OVERRIDE)),
                    lifetime_s: num_f32(sec.get(KEY_PFX_LIFETIME)),
                    velocity: num_f32(sec.get(KEY_PFX_VELOCITY)),
                    class: text(KEY_PFX_CLASS),
                    flight: text(KEY_PFX_CLASS).and_then(|c| class_flight(&c)),
                    class_data: text(KEY_PFX_CLASS_DATA),
                    model_box: None,
                },
            );
        }
        t
    }

    /// The mine class-data row of the projectile row named `name`: `Some` when the row's `ClassData` names a
    /// [`TRAP_SECTION`] row (the round is a mine, DSV-611).
    // DC-DEFAULT: DSV-611 (proposed) a projectile row with mine class data is a mine
    pub fn trap_of(&self, name: &str) -> Option<TrapRow> {
        let cd = self.get(name)?.class_data.as_deref()?;
        self.traps.get(&dc_bute::fold_latin1(cd)).copied()
    }

    /// Reads the rows from the user's ROM, with the mesh box of every mine row's model.
    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, String> {
        match ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, FX_ENTRY) {
            Ok(Some(d)) => {
                let mut t = Self::from_doc(&d);
                t.read_mine_models(vfs);
                Ok(t)
            }
            Ok(None) => Err("FX table entry missing".into()),
            Err(e) => Err(format!("FX table: {e}")),
        }
    }

    /// Fills [`ProjectileRow::model_box`] of the mine rows (a row whose model is missing or unreadable keeps none).
    // SPEC: FMT_LTB.md §5, §6 (pieces, LOD meshes, vertex streams) through dc-model
    // DC-DEFAULT: DSV-614 (proposed), DSV-618 (proposed)
    fn read_mine_models(&mut self, vfs: &RezVfs) {
        let names: Vec<String> = self
            .rows
            .keys()
            .filter(|n| self.trap_of(n).is_some())
            .cloned()
            .collect();
        for n in names {
            let Some(row) = self.rows.get_mut(&n) else {
                continue;
            };
            row.model_box = row
                .model
                .as_deref()
                .and_then(|m| crate::chars::read_entry(vfs, m).ok())
                .and_then(|b| dc_model::Model::parse_without_tracks(&b).ok())
                .and_then(|m| mesh_box(&m));
        }
    }

    /// The row named `name` (names compared folded, as the weapon order does).
    pub fn get(&self, name: &str) -> Option<&ProjectileRow> {
        self.rows.get(&dc_bute::fold_latin1(name))
    }
}

/// The item table entry (the client's item rows; the lobby's inventory numbers name them).
// OBSERVED: ROM table key ITEMS.TXT (entry of ATTRIBUTES.rez)
pub const ITEMS_ENTRY: &str = "ATTRIBUTES\\ITEMS.TXT";
/// Item-table keys of a gear row: its type code and id, the armour points it adds and the armour grade.
// OBSERVED: ROM table key ITEMS.TXT rows with ItemType, ItemID, Armor, ArmorGrade (head, face, camouflage and vest
// rows carry `Armor`; values read at run time)
pub const KEY_ITEM_TYPE: &str = "ItemType";
pub const KEY_ITEM_ID: &str = "ItemID";
pub const KEY_ARMOR: &str = "Armor";
pub const KEY_ARMOR_GRADE: &str = "ArmorGrade";
/// The `WeaponItem` row prefix, its name key and rank key (the ROM default weapons, CD-18).
// OBSERVED: ROM table key ITEMS.TXT [WeaponItem<n>] ItemID, ItemType, ItemName, RankPoint
pub const WEAPON_ITEM_SECTION: &str = "WeaponItem";
pub const KEY_ITEM_NAME: &str = "ItemName";
pub const KEY_RANK_POINT: &str = "RankPoint";
/// The item row's damage rating (the balanced pick among several default-issue rows orders by it).
// OBSERVED: ROM table key ITEMS.TXT [WeaponItem<n>] Damage
pub const KEY_ITEM_DAMAGE: &str = "Damage";

/// What an inventory item number names: weapons are numbered by their weapon row (below 1000), the gear rows by
/// `1000 x k + ItemID` with the item type code of the thousand.
// EVIDENCE: cshell+0x25c010 (number -> (id, type): < 1000 weapon lists; 2000s -> 0xe, 3000s -> 0xf, 4000s -> 0xb,
// 5000s -> 0xc, 6000s -> 0xd, 7000s -> 0x10 backpack); OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §2 (0x323 item numbers of
// slots 0..3 = the 0xb2 weapon numbers)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemNo {
    Weapon(u16),
    /// A weapon option (attachment) `1000 + ItemID`: the ItemID is the client's mod index (`[Mod<ItemID>]`).
    Option(u16),
    /// `(item type code, ItemID)` of a gear row.
    Gear(u8, u32),
    Other(u32),
}

// EVIDENCE: cshell+0x25c010 (1000s: options, types 0x11..0x13, the client keeps n % 1000), cshell+0x287930 /
// cshell+0x226700 (that number is filed in the weapon's option field of its kind)
// OBSERVED: capture 20261025_153705_rust7 (lobby item 1000 + 35 on the M4A1 -> the client's
// `UP_PLAYER_INFO` entry 145 carries 35 in its sight field)
pub fn item_no(n: u32) -> ItemNo {
    match n {
        0..=999 => ItemNo::Weapon(n as u16),
        1000..=1999 => ItemNo::Option((n - 1000) as u16),
        2000..=2999 => ItemNo::Gear(0x0e, n - 2000),
        3000..=3999 => ItemNo::Gear(0x0f, n - 3000),
        4000..=4999 => ItemNo::Gear(0x0b, n - 4000),
        5000..=5999 => ItemNo::Gear(0x0c, n - 5000),
        6000..=6999 => ItemNo::Gear(0x0d, n - 6000),
        _ => ItemNo::Other(n),
    }
}

/// One gear row's armour values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GearRow {
    pub armor: f32,
    pub grade: f32,
}

/// The gear rows of the item table that carry an armour value, by (type code, ItemID); and the ROM default weapons
/// (one per weapon slot 0..3).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemRows {
    pub gear: BTreeMap<(u8, u32), GearRow>,
    /// Per weapon slot the default weapon (CD-18's rule: among the unranked rows marked as default issue with the
    /// item type of the first such row, the middle one by damage rating; else the first without a rank requirement,
    /// else the first).
    pub issue_weapons: [Option<u16>; 4],
    /// Weapon number -> the `WeaponItem` row's `ItemType` (the room's weapon limit names types, DSV-180).
    pub weapon_types: BTreeMap<u16, i32>,
}

/// The suffix the item table puts on the names of its default-issue rows.
// OBSERVED: ROM table key ITEMS.TXT [WeaponItem<n>] ItemName (CD-18 approved: the lobby's starter rule uses the same
// marker)
pub const DEFAULT_ISSUE_MARK: &str = "(Default)";

/// The balanced default of one slot: among the marked rows with the item type of the first one, the middle one by
/// damage rating (the lower middle for an even count; ties by file order).
// DC-DEFAULT: CD-18 (refinement, lobby-slots, merged at master 7ec5c41): the same pick as the lobby's starter kit, so
// a player without a lobby loadout gets the weapons a new account gets (rust3: two rifles carry the default-issue mark
// and the first in file order was not the one the user knew)
fn balanced_default(rows: &[(u32, i64, u16)]) -> Option<u16> {
    let ty = rows.first()?.0;
    let mut same: Vec<(i64, usize, u16)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.0 == ty)
        .map(|(i, r)| (r.1, i, r.2))
        .collect();
    same.sort_unstable();
    same.get((same.len() - 1) / 2).map(|r| r.2)
}

impl ItemRows {
    pub fn from_doc(doc: &ButeDoc) -> Self {
        let mut t = ItemRows::default();
        // marked default rows per slot: (item type, damage rating, weapon) in file order
        let mut marked: [Vec<(u32, i64, u16)>; 4] = Default::default();
        let mut unranked: [Option<u16>; 4] = [None; 4];
        let mut first: [Option<u16>; 4] = [None; 4];
        for (name, sec) in doc.iter() {
            let ty = num_u32(sec.get(KEY_ITEM_TYPE));
            let id = num_u32(sec.get(KEY_ITEM_ID));
            if name.starts_with(WEAPON_ITEM_SECTION) {
                // DC-DEFAULT: CD-18 (approved) starter weapons by slot kind; EVIDENCE: cshell+0x259f00 (type -> slot)
                let (Some(ty), Some(id)) = (ty, id) else {
                    continue;
                };
                if let Ok(w) = u16::try_from(id) {
                    t.weapon_types.insert(w, ty as i32);
                }
                let Some(slot) = dc_bute::equip_slot_by_class_code(ty as i32)
                    .filter(|s| (0..4).contains(s))
                    .map(|s| s as usize)
                else {
                    continue;
                };
                let Ok(w) = u16::try_from(id) else {
                    continue;
                };
                first[slot].get_or_insert(w);
                let rank = sec
                    .get(KEY_RANK_POINT)
                    .and_then(|v| v.trim().parse::<i64>().ok())
                    .unwrap_or(-1);
                if rank <= 0 {
                    unranked[slot].get_or_insert(w);
                }
                if rank <= 0
                    && sec
                        .get(KEY_ITEM_NAME)
                        .is_some_and(|n| unquote(n).ends_with(DEFAULT_ISSUE_MARK))
                {
                    let dmg = sec
                        .get(KEY_ITEM_DAMAGE)
                        .and_then(|v| v.trim().parse::<i64>().ok())
                        .unwrap_or(i64::MAX);
                    marked[slot].push((ty, dmg, w));
                }
                continue;
            }
            let (Some(ty), Some(id), Some(armor)) = (ty, id, num_f32(sec.get(KEY_ARMOR))) else {
                continue;
            };
            let Ok(ty) = u8::try_from(ty) else {
                continue;
            };
            t.gear.insert(
                (ty, id),
                GearRow {
                    armor,
                    grade: num_f32(sec.get(KEY_ARMOR_GRADE)).unwrap_or(0.0),
                },
            );
        }
        for s in 0..4 {
            t.issue_weapons[s] = balanced_default(&marked[s]).or(unranked[s]).or(first[s]);
        }
        t
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, String> {
        match ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, ITEMS_ENTRY) {
            Ok(Some(d)) => Ok(Self::from_doc(&d)),
            Ok(None) => Err("item table entry missing".into()),
            Err(e) => Err(format!("item table: {e}")),
        }
    }

    /// The armour row of an inventory item number (gear items only).
    pub fn gear_of(&self, item: u32) -> Option<GearRow> {
        match item_no(item) {
            ItemNo::Gear(ty, id) => self.gear.get(&(ty, id)).copied(),
            _ => None,
        }
    }
}

/// The attachment table entry and its flag rows: `[Flag<k>]` (k = team % 2, the reference stack's pairing) `Skin0`, `Skin1`,
/// `RenderStyle0`, the
/// textures and the render style of the team's flag model.
// OBSERVED: ROM table key ATTACHMENTS.TXT [Flag<k>] Skin<n>, RenderStyle<n> (the reference stack's flag pickups named these
// files with the world object's model file: compared by sha256 in tests/rom_world.rs)
pub const ATTACHMENTS_ENTRY: &str = "ATTRIBUTES\\ATTACHMENTS.TXT";
pub const FLAG_SECTION: &str = "Flag";
pub const KEY_SKIN: &str = "Skin";
pub const KEY_RENDER_STYLE: &str = "RenderStyle";
/// `[Flag<k>]` model of the carried flag and the body socket it hangs on.
// OBSERVED: ROM table key ATTACHMENTS.TXT [Flag<k>] Model0, Socket (both rows: a back-carried flag model and the same
// socket of the body models; values read at run time)
// DC-DEFAULT: DSV-217 (proposed) the carried flag is that model at that socket
pub const KEY_ATTACH_MODEL: &str = "Model0";
pub const KEY_ATTACH_SOCKET: &str = "Socket";

/// The ground flag model the capture flag of every ROM world names (the archive path, upper case).
// OBSERVED: ROM world capture flag objects (the model property of each flag of the ROM flag worlds checked)
// DC-DEFAULT: MAP-114 (proposed) a community map's flag without a model uses it
pub const GROUND_FLAG_MODEL: crate::chars::NameKey = ([191, 100, 172, 176, 200, 27, 51, 163], 26);

/// `(textures, styles)` of the flag rows by `k`, the carried form `(model, socket name)`, and the ground model.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlagLooks {
    pub rows: BTreeMap<u32, (Vec<String>, Vec<String>)>,
    pub carried: BTreeMap<u32, (String, String)>,
    pub ground: Option<String>,
}

impl FlagLooks {
    pub fn from_doc(doc: &ButeDoc) -> Self {
        let mut t = FlagLooks::default();
        for (name, sec) in doc.iter() {
            let Some(k) = index_of(name, FLAG_SECTION) else {
                continue;
            };
            t.rows.insert(
                u32::from(k),
                (
                    numbered(sec, KEY_SKIN, 8),
                    numbered(sec, KEY_RENDER_STYLE, 4),
                ),
            );
            if let (Some(m), Some(s)) = (sec.get(KEY_ATTACH_MODEL), sec.get(KEY_ATTACH_SOCKET)) {
                let (m, s) = (unquote(m), unquote(s));
                if !m.is_empty() && !s.is_empty() {
                    t.carried
                        .insert(u32::from(k), (m.to_string(), s.to_string()));
                }
            }
        }
        t
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, String> {
        match ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, ATTACHMENTS_ENTRY) {
            Ok(Some(d)) => {
                let mut t = Self::from_doc(&d);
                t.ground = find_ground_flag(vfs);
                Ok(t)
            }
            Ok(None) => Err("attachment table entry missing".into()),
            Err(e) => Err(format!("attachment table: {e}")),
        }
    }
}

/// The path of [`GROUND_FLAG_MODEL`] in the user's archives (`None` when the ROM lacks it).
fn find_ground_flag(vfs: &RezVfs) -> Option<String> {
    let mut rez: Vec<String> = std::fs::read_dir(vfs.rom_dir().join("Game"))
        .ok()?
        .filter_map(|e| e.ok()?.file_name().to_str().map(str::to_string))
        .filter(|n| n.to_ascii_lowercase().ends_with(".rez"))
        .collect();
    rez.sort();
    rez.iter().find_map(|name| {
        vfs.list_entries(name)
            .ok()?
            .into_iter()
            .find_map(|(entry, _, _)| {
                let up = entry.to_ascii_uppercase();
                (crate::chars::name_key(up.as_bytes()) == GROUND_FLAG_MODEL).then_some(up)
            })
    })
}

/// The HiSec case table: `[Hisec<n>]` rows with the case model (`Filename`), its skins (`Skin<k>`), render styles
/// (`RenderStyle<k>`) and `Scale`. The model of the in-match case object (type 0x17 kind 5) comes from one row.
// OBSERVED: ROM table key HISECS.TXT (entry of ATTRIBUTES.rez) [Hisec<n>] Filename, Skin<k>, RenderStyle<k>, Scale
// (2026-10-25: row 0's model, skin and render style are the three files of the reference stack's case creates in
// capture 20261025_065016_tdm6 c6 seq 3015 and 20261025_002236_tdm4 c6 seq 3132; values read
// at run time, never copied)
pub const HISECS_ENTRY: &str = "ATTRIBUTES\\HISECS.TXT";
pub const HISEC_SECTION: &str = "Hisec";
pub const KEY_HISEC_MODEL: &str = "Filename";
pub const KEY_HISEC_SCALE: &str = "Scale";

/// One `[Hisec<n>]` row.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HisecRow {
    pub model: Option<String>,
    pub skins: Vec<String>,
    pub styles: Vec<String>,
    pub scale: Option<[f32; 3]>,
}

/// The `[Hisec<n>]` rows by n.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HisecRows {
    pub rows: BTreeMap<u32, HisecRow>,
}

impl HisecRows {
    pub fn from_doc(doc: &ButeDoc) -> Self {
        let mut t = HisecRows::default();
        for (name, sec) in doc.iter() {
            let Some(k) = index_of(name, HISEC_SECTION) else {
                continue;
            };
            t.rows.insert(
                u32::from(k),
                HisecRow {
                    model: sec
                        .get(KEY_HISEC_MODEL)
                        .map(|v| unquote(v).to_string())
                        .filter(|v| !v.is_empty()),
                    skins: numbered(sec, KEY_SKIN, 8),
                    styles: numbered(sec, KEY_RENDER_STYLE, 4),
                    scale: num_vec3(sec.get(KEY_HISEC_SCALE)),
                },
            );
        }
        t
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, String> {
        match ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, HISECS_ENTRY) {
            Ok(Some(d)) => Ok(Self::from_doc(&d)),
            Ok(None) => Err("HiSec case table entry missing".into()),
            Err(e) => Err(format!("HiSec case table: {e}")),
        }
    }
}

/// The two team names (team 1, team 2) as the spawn points' team filter texts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TeamNames {
    pub names: [Option<Vec<u8>>; 2],
}

impl TeamNames {
    pub fn from_doc(doc: &ButeDoc) -> Self {
        let get = |k: u32| {
            doc.get_str(&format!("{TEAM_SECTION}{k}"), KEY_NAME)
                .map(|s| unquote(s).as_bytes().to_vec())
                .filter(|s| !s.is_empty())
        };
        TeamNames {
            names: [get(1), get(2)],
        }
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, String> {
        match ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, TEAMS_ENTRY) {
            Ok(Some(d)) => Ok(Self::from_doc(&d)),
            Ok(None) => Err("team table entry missing".into()),
            Err(e) => Err(format!("team table: {e}")),
        }
    }

    /// Team (1 or 2) whose name equals a spawn point's team filter (ASCII case-insensitive), or the DeCombat team
    /// texts `"1"` / `"2"` of a community map (accepted with or without a ROM); `None` = any team.
    // DC-DEFAULT: MAP-22 (the team texts `"1"` / `"2"` next to the ROM team names)
    pub fn team_of(&self, filter: Option<&[u8]>) -> Option<u8> {
        let f = filter?;
        let f = std::str::from_utf8(f).map(unquote).unwrap_or("").trim();
        match f {
            "" => return None,
            "1" => return Some(1),
            "2" => return Some(2),
            _ => {}
        }
        self.names.iter().enumerate().find_map(|(i, n)| {
            n.as_deref()
                .filter(|n| n.eq_ignore_ascii_case(f.as_bytes()))
                .map(|_| i as u8 + 1)
        })
    }
}

/// Everything read from the ROM for the game rules.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RomTables {
    pub weapons: WeaponTable,
    pub teams: TeamNames,
    /// Gear armour rows and the default weapons of the item table (slice 4).
    pub items: ItemRows,
    /// The flag rows of the attachment table (slice 4, the capture flags of the world).
    pub flag_looks: FlagLooks,
    /// Character rows (slice 3).
    pub chars: crate::chars::CharTables,
    /// Projectile rows of the FX table (thrown objects, DSV-39).
    pub projectiles: ProjectileTable,
    /// HiSec case rows (the in-match case object, HS-2).
    pub hisec: HisecRows,
    /// The pain, head-shot and grenade impact sound rows (DSV-64).
    pub event_sounds: crate::sounds::EventSoundTables,
    /// The shot cone's random table read from the ROM (`None`: the entry is missing; the check then uses the
    /// table's generator, [`crate::cone::shared_generated`]).
    // DC-DEFAULT: DSV-102 (proposed)
    pub random_table: Option<std::sync::Arc<[f32]>>,
    /// The ROM folder, for the model files the characters need (read on first use).
    pub rom: Option<std::path::PathBuf>,
}

impl RomTables {
    /// How the server flies the round of `weapon` / `ammo` (ammunition `Type` 1 or 2): the flight its projectile row's
    /// class names, else launched for `Type` 1 with a projectile row and thrown for `Type` 2 (the rule before DSV-650).
    /// `None` for any other round (traced, the controller). A mine's row (DSV-611) names neither class and keeps the
    /// `Type` rule here; [`crate::game::Game`] tests it first.
    // DC-DEFAULT: DSV-650 (proposed; amends DSV-181, DSV-29) the class first, the `Type` after
    pub fn round_flight(&self, weapon: u16, ammo: u16) -> Option<RoundFlight> {
        let w = &self.weapons;
        if !(w.fires_projectile(weapon, ammo) || w.throws_round(weapon, ammo)) {
            return None;
        }
        let class = w
            .projectile_fx(weapon, ammo)
            .and_then(|n| self.projectiles.get(n))
            .and_then(|r| r.flight);
        Some(class.unwrap_or(if w.throws_round(weapon, ammo) {
            RoundFlight::Thrown
        } else {
            RoundFlight::Launched
        }))
    }

    /// The item table's `ItemType` of weapon `w`; a variant (VAR-1) has its base gun's type, since a variant has no
    /// item row of its own.
    // DC-DEFAULT: VAR-1 (proposed) type lookups go through the base gun; DSV-610 (proposed, OWNER REPORT 2026-11-05)
    pub fn weapon_type(&self, w: u16) -> Option<i32> {
        let base = self.weapons.variants.get(&w).copied().unwrap_or(w);
        self.items.weapon_types.get(&base).copied()
    }

    /// The shot cone's random table: the ROM entry, else its generator (DSV-102).
    // DC-DEFAULT: DSV-102 (proposed) the generator where the ROM entry is missing
    pub fn cone_table(&self) -> &[f32] {
        match &self.random_table {
            Some(t) => t,
            None => crate::cone::shared_generated(),
        }
    }

    /// Reads both tables; a missing table is reported in the returned notes and left empty.
    pub fn load(rom: &Path) -> (Self, Vec<String>) {
        match Self::load_with_content(rom, None) {
            Ok(r) => r,
            Err(_) => unreachable!("without content nothing is refused"),
        }
    }

    /// [`RomTables::load`] with the mod set's weapon content (`-content`, DSV-478) applied to the weapon table
    /// before the match tables are built. A content patch the table refuses is an error (the server does not
    /// start); a missing table is a note as in `load`.
    // DC-DEFAULT: DSV-478 (proposed)
    pub fn load_with_content(
        rom: &Path,
        content: Option<&dc_bute::modpatch::WeaponContent>,
    ) -> Result<(Self, Vec<String>), String> {
        let vfs = RezVfs::new(rom);
        let mut notes = Vec::new();
        let weapons = match content {
            Some(c) => {
                WeaponTable::from_vfs_with_content(&vfs, c).map_err(|e| format!("content: {e}"))?
            }
            None => WeaponTable::from_vfs(&vfs).unwrap_or_else(|e| {
                notes.push(e);
                WeaponTable::default()
            }),
        };
        let teams = TeamNames::from_vfs(&vfs).unwrap_or_else(|e| {
            notes.push(e);
            TeamNames::default()
        });
        let chars = crate::chars::CharTables::from_vfs(&vfs).unwrap_or_else(|e| {
            notes.push(e);
            crate::chars::CharTables::default()
        });
        let items = ItemRows::from_vfs(&vfs).unwrap_or_else(|e| {
            notes.push(e);
            ItemRows::default()
        });
        let flag_looks = FlagLooks::from_vfs(&vfs).unwrap_or_else(|e| {
            notes.push(e);
            FlagLooks::default()
        });
        let projectiles = ProjectileTable::from_vfs(&vfs).unwrap_or_else(|e| {
            notes.push(e);
            ProjectileTable::default()
        });
        let hisec = HisecRows::from_vfs(&vfs).unwrap_or_else(|e| {
            notes.push(e);
            HisecRows::default()
        });
        let event_sounds = crate::sounds::EventSoundTables::from_vfs(&vfs).unwrap_or_else(|e| {
            notes.push(e);
            crate::sounds::EventSoundTables::default()
        });
        let random_table = crate::cone::rom_table(&vfs).map(std::sync::Arc::from);
        if random_table.is_none() {
            notes
                .push("random table entry missing (the shot cone check uses its generator)".into());
        }
        Ok((
            RomTables {
                random_table,
                hisec,
                event_sounds,
                weapons,
                teams,
                items,
                flag_looks,
                chars,
                projectiles,
                rom: Some(rom.to_path_buf()),
            },
            notes,
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_variant_switches_as_its_base_gun_and_leaves_the_table_alone() {
        // VAR-1: a variant carries its base gun's name; the base gun keeps its entry
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"G\"\nClass1 = \"K\"\nK0 = \"Knife\"\n",
            "[Weapon5]\nName = \"G\"\n[Weapon6]\nName = \"Knife\"\n",
            "[Weapon500]\nName = \"G\"\nvariant_of = 5\n",
        ));
        let t = super::WeaponTable::from_doc(&doc);
        assert_eq!(t.commands, vec![5, 6]);
        assert_eq!(t.command_of(500), t.command_of(5));
        assert!(t.command_of(5).is_some());
        assert_eq!(t.class_of(500), t.class_of(5));
        assert_ne!(t.class_of(500), t.class_of(6));
    }

    use super::*;

    // a synthetic table with the key shape of the ROM table (made-up values)
    const SYNTH: &str = concat!(
        "[Ammo0]\n",
        "Name = \"Pellet A\"\n",
        "InstDamage = 12.5\n",
        "InstDamageType = 2\n",
        "MaxAmount = 90\n",
        "[Ammo1]\n",
        "Name = \"Pellet B\"\n",
        "InstDamage = 40\n",
        "[Weapon0]\n",
        "ShotsPerClip = 30\n",
        "VectorsPerRound = 1\n",
        "AmmoName0 = \"pellet b\"\n",
        "Name = \"Gun A\"\n",
        "HHModel = \"HH\\gun.ltb\"\n",
        "HHSkin0 = \"HH\\gun.dtx\"\n",
        "HHRenderStyle0 = \"S\\a.ltb\"\n",
        "[Weapon1]\n",
        "Name = \"Gun B\"\n",
        "IsAmmo = 1\n",
        "[WeaponOrder]\n",
        "Class0 = \"Long\"\n",
        "Class1 = \"Short\"\n",
        "Long0 = \"gun b\"\n",
        "Long1 = \"Missing\"\n",
        "Short0 = \"Gun A\"\n",
        "[Team1]\n",
        "Name = \"Red\"\n",
        "[Team2]\n",
        "Name = \"Blue\"\n",
    );

    #[test]
    fn the_alternate_fire_takes_alt_inst_damage() {
        // DSV-405: made-up rows; ammunition 0 has an alternate damage, ammunition 1 none
        let d = ButeDoc::parse(
            "[Ammo0]
Name = \"Blade\"
InstDamage = 45
Alt_InstDamage = 75
[Ammo1]
Name = \"Pellet\"
InstDamage = 40
[Weapon0]
AmmoName0 = \"blade\"
Guntype = 0
[Weapon1]
AmmoName0 = \"pellet\"
Guntype = 1
[Weapon2]
AmmoName0 = \"blade\"
Guntype = 1
",
        );
        let t = WeaponTable::from_doc(&d);
        assert_eq!(t.shot_damage(0, 0, false), Some(45.0));
        assert_eq!(t.shot_damage(0, 0, true), Some(75.0));
        assert_eq!(
            t.shot_damage(1, 1, true),
            Some(40.0),
            "no alternate damage: the primary's"
        );
        // a rifle row (Guntype 1) firing the blade ammunition never gets the alternate damage, mode1 or not
        assert_eq!(
            t.shot_damage(2, 0, true),
            Some(45.0),
            "mode1 on a non-melee row is ignored"
        );
        assert!(t.alt_capable(0) && !t.alt_capable(2));
    }

    /// DSV-440: a hit by a weapon granted with a suppressor (`Instdamage` -0.1, socket 0) does 0.9 of the row's
    /// damage; a magazine mod without the key changes nothing; two parts of one socket count once (the later one);
    /// the factor never goes below 0.
    #[test]
    fn granted_mods_scale_the_shot_damage() {
        let d = ButeDoc::parse(
            "[Ammo0]
Name = \"Pellet\"
InstDamage = 40
Alt_InstDamage = 60
[Weapon0]
AmmoName0 = \"pellet\"
Guntype = 0
[Mod0]
Name = \"Sup\"
Kinds = 0
Instdamage = -0.1
[Mod1]
Name = \"Mag\"
Kinds = 2
ShotsPerClipPlus0 = -6//30 rounds (20%)
[Mod2]
Name = \"Heavy\"
Kinds = 0
Instdamage = -3
",
        );
        let t = WeaponTable::from_doc(&d);
        assert_eq!(t.mods[&1].clip_plus[0], -6, "signed, the comment dropped");
        let none = [-1i8; 4];
        assert_eq!(t.shot_damage_with_mods(0, 0, false, &none), Some(40.0));
        let sup = [-1, 0, 1, -1];
        assert_eq!(t.mod_damage_factor(&sup), 0.9);
        assert!((t.shot_damage_with_mods(0, 0, false, &sup).unwrap() - 36.0).abs() < 1e-4);
        assert!(
            (t.shot_damage_with_mods(0, 0, true, &sup).unwrap() - 54.0).abs() < 1e-4,
            "the alternate too"
        );
        assert_eq!(
            t.mod_damage_factor(&[0, 2, -1, -1]),
            0.0,
            "the later part of socket 0, clamped at 0"
        );
        assert_eq!(t.mod_damage_factor(&[2, 0, -1, -1]), 0.9);
    }

    #[test]
    fn weapon_rows_and_damage_choice() {
        let d = ButeDoc::parse(SYNTH);
        let t = WeaponTable::from_doc(&d);
        assert_eq!(t.weapons.len(), 2, "WeaponOrder is not a row");
        assert_eq!(
            t.commands,
            vec![1, 0],
            "classes in order, names matched without case, unknown skipped"
        );
        assert_eq!(t.command_of(0), Some(0x15));
        assert_eq!(t.command_of(1), Some(0x14));
        assert_eq!(t.command_of(7), None);
        let hh = t.hand_model(0).unwrap();
        assert_eq!((hh.hh_skins.len(), hh.hh_styles.len()), (1, 1));
        assert!(t.hand_model(1).is_none());
        assert_eq!(t.clip(0), Some(30));
        assert_eq!(t.ammo_of(0), Some(1));
        assert_eq!(t.damage(0, 0), Some(12.5), "the ammunition row decides");
        assert_eq!(
            t.damage(0, 9),
            Some(40.0),
            "unknown ammunition: the weapon's first"
        );
        assert_eq!(t.damage(5, 9), None);
        assert_eq!(t.max_amount(0), Some(90));
        assert_eq!(t.damage_type(0, 0), Some(2));
        assert_eq!(t.damage_type(0, 1), None, "Pellet B has no type");
        assert!(!t.is_own_ammo(0), "no IsAmmo key");
        assert!(t.is_own_ammo(1), "IsAmmo = 1");
        assert!(!t.is_own_ammo(7), "unknown row");
    }

    #[test]
    fn gear_rows_share_by_damage_type_and_location() {
        // made-up rows in the key shape of the weapon table's armour rows
        let w = WeaponTable::from_doc(&ButeDoc::parse(concat!(
            "[Gear0]
Armor = 80.0
ProtectionType0 = 4
ProtectionType1 = 6
",
            "NodeHeadProtection0 = 0.0
NodeTorsoProtection0 = 0.2
NodeArmProtection0 = 0.25
",
            "NodelegProtection0 = 0.3
NodeHeadProtection1 = 1.5
NodeTorsoProtection1 = -1
",
            "[Gear1]
Armor = 0.0
ProtectionType0 = 4
NodeHeadProtection0 = 0.4
",
            "[Gear2]
Name = \"nothing protected\"
",
        )));
        assert_eq!(w.gear.len(), 3);
        let g = &w.gear[&0];
        assert_eq!(g.armor, Some(80.0));
        assert_eq!(g.slots.len(), 2);
        assert_eq!(
            [1, 2, 3, 4, 0].map(|l| g.share(4, l)),
            [0.0, 0.2, 0.25, 0.3, 0.2],
            "by location; no location = torso"
        );
        assert_eq!(g.share(6, 1), 1.0, "clamped to 1 as the client does");
        assert_eq!(g.share(6, 2), 0.0, "clamped to 0");
        assert_eq!(g.share(6, 3), 0.0, "a missing share is 0");
        assert_eq!(g.share(7, 2), 0.0, "an unprotected damage type");
        assert_eq!(w.gear[&1].share(4, 1), 0.4);
        assert!(w.gear[&2].slots.is_empty() && w.gear[&2].armor.is_none());
    }

    #[test]
    fn area_damage_and_projectile_rows() {
        // made-up rows in the key shape of the weapon and FX tables
        let w = WeaponTable::from_doc(&ButeDoc::parse(concat!(
            "[Ammo0]\nName = \"Lob\"\nAreaDamage = 50\nAreaDamageType = 4\nAreaDamageRadius = 120\n",
            "ProjectileFXName = \"Arc1\"\n",
            "[Ammo1]\nName = \"Flat\"\nAreaDamage = 0\nAreaDamageRadius = 0\nProjectileFXName = \n",
            "[Weapon0]\nName = \"Thrower\"\nAmmoName0 = \"lob\"\n",
        )));
        let a = w.area(0, 0).unwrap();
        assert_eq!((a.damage, a.damage_type, a.radius), (50.0, Some(4), 120.0));
        assert_eq!(
            w.area(0, 7).map(|a| a.radius),
            Some(120.0),
            "the weapon's first ammunition"
        );
        assert_eq!(w.area(0, 1), None, "no blast without damage and radius");
        assert_eq!(w.projectile_fx(0, 0), Some("Arc1"));
        assert_eq!(w.projectile_fx(5, 1), None, "an empty name is none");
        let p = ProjectileTable::from_doc(&ButeDoc::parse(concat!(
            "[ProjectileFX0]\nName = Arc1\nModel = M\\a.ltb\nModelScale = <2.000000, 3.0, 4>\nSkin = M\\a.dtx\n",
            "Gravity = 1\nGravityOverride = -900.0\nLifetime = 1.5\nVelocity = 800\n",
            "[ProjectileFX1]\nName = Straight\nGravity = 0\nModelScale = <1, 2>\n",
            "[ProjectileFX2]\nModel = nameless.ltb\n",
        )));
        assert_eq!(p.rows.len(), 2, "a row without a name is skipped");
        let r = p.get("ARC1").unwrap();
        assert_eq!(r.model.as_deref(), Some("M\\a.ltb"));
        assert_eq!(r.skin.as_deref(), Some("M\\a.dtx"));
        assert_eq!(r.scale, Some([2.0, 3.0, 4.0]));
        assert_eq!(
            (r.gravity, r.gravity_override, r.lifetime_s, r.velocity),
            (Some(true), Some(-900.0), Some(1.5), Some(800.0))
        );
        let s = p.get("straight").unwrap();
        assert_eq!((s.gravity, s.scale), (Some(false), None));
    }

    #[test]
    fn item_numbers_gear_rows_and_issue_weapons() {
        // EVIDENCE: cshell+0x25c010 number ranges
        assert_eq!(item_no(145), ItemNo::Weapon(145));
        assert_eq!(item_no(3005), ItemNo::Gear(0x0f, 5));
        assert_eq!(item_no(4002), ItemNo::Gear(0x0b, 2));
        assert_eq!(item_no(7001), ItemNo::Other(7001));
        assert_eq!(item_no(1035), ItemNo::Option(35));
        assert_eq!(item_no(1000), ItemNo::Option(0));
        // made-up rows in the key shape of the item table
        let doc = ButeDoc::parse(concat!(
            "[WeaponItem0]\nItemID = 10\nItemType = 1\nRankPoint = 3\nItemName = \"A\"\n",
            "[WeaponItem1]\nItemID = 11\nItemType = 0\nRankPoint = -1\nItemName = \"B\"\n",
            "[WeaponItem2]\nItemID = 12\nItemType = 0\nItemName = \"C (Default)\"\n",
            "[WeaponItem3]\nItemID = 20\nItemType = 4\nRankPoint = 5\nItemName = \"D\"\n",
            "[WeaponItem4]\nItemID = 30\nItemType = 7\n",
            "[VestRow0]\nItemID = 5\nItemType = 15\nArmor = 20\nArmorGrade = 40\n",
            "[HeadRow0]\nItemID = 2\nItemType = 11\nArmor = 0\n",
            "[NoArmor0]\nItemID = 3\nItemType = 11\n",
        ));
        let t = ItemRows::from_doc(&doc);
        assert_eq!(t.issue_weapons, [Some(12), Some(20), Some(30), None]);
        assert_eq!(
            t.gear_of(3005),
            Some(GearRow {
                armor: 20.0,
                grade: 40.0
            })
        );
        assert_eq!(t.gear_of(4002).map(|g| g.armor), Some(0.0));
        assert_eq!(t.gear_of(4003), None);
        assert_eq!(t.gear_of(145), None);
        // several marked rows: the middle one by damage rating among those of the first one's item type
        let doc2 = ButeDoc::parse(concat!(
            "[WeaponItem0]
ItemID = 13
ItemType = 0
Damage = 48
ItemName = \"E (Default)\"
",
            "[WeaponItem1]
ItemID = 14
ItemType = 0
Damage = 38
ItemName = \"F (Default)\"
",
            "[WeaponItem2]
ItemID = 15
ItemType = 1
Damage = 1
ItemName = \"G (Default)\"
",
            "[WeaponItem3]
ItemID = 21
ItemType = 4
Damage = 33
ItemName = \"H (Default)\"
",
            "[WeaponItem4]
ItemID = 22
ItemType = 4
Damage = 38
ItemName = \"I (Default)\"
",
        ));
        let t2 = ItemRows::from_doc(&doc2);
        assert_eq!(&t2.issue_weapons[..2], &[Some(14), Some(21)]);
    }

    #[test]
    fn team_filter_mapping() {
        let n = TeamNames::from_doc(&ButeDoc::parse(SYNTH));
        assert_eq!(n.team_of(Some(b"red")), Some(1));
        assert_eq!(n.team_of(Some(b"BLUE")), Some(2));
        assert_eq!(n.team_of(Some(b"")), None);
        assert_eq!(n.team_of(Some(b"green")), None);
        assert_eq!(n.team_of(None), None);
        // MAP-22: the DeCombat team texts, with or without ROM names
        assert_eq!(n.team_of(Some(b"1")), Some(1));
        assert_eq!(n.team_of(Some(b" 2 ")), Some(2));
        assert_eq!(n.team_of(Some(b"3")), None);
        let none = TeamNames::default();
        assert_eq!(none.team_of(Some(b"1")), Some(1));
        assert_eq!(none.team_of(Some(b"2")), Some(2));
        assert_eq!(none.team_of(Some(b"red")), None);
    }

    #[test]
    fn rom_tables_load() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let (t, notes) = RomTables::load(&rom);
        assert!(notes.is_empty(), "{notes:?}");
        let with_damage = t
            .weapons
            .weapons
            .keys()
            .filter(|&&w| t.weapons.damage(w, u16::MAX).is_some())
            .count();
        println!(
            "ROM weapon table: {} weapons, {} ammunition rows, {} weapons with a damage value; teams named: {}",
            t.weapons.weapons.len(),
            t.weapons.ammo.len(),
            with_damage,
            t.teams.names.iter().filter(|n| n.is_some()).count()
        );
        assert!(!t.weapons.weapons.is_empty() && !t.weapons.ammo.is_empty());
        assert!(with_damage > 0);
        let hand = t
            .weapons
            .weapons
            .keys()
            .filter(|&&w| t.weapons.hand_model(w).is_some())
            .count();
        println!(
            "ROM weapon command table: {} entries; {} weapons with a third-person model; {} character body rows",
            t.weapons.commands.len(),
            hand,
            t.chars.bodies.len()
        );
        assert!(!t.weapons.commands.is_empty() && hand > 0 && !t.chars.is_empty());
        assert!(t.teams.names.iter().all(|n| n.is_some()));
        // DSV-30 inputs: every body row names a skeleton row with hit nodes whose locations are codes 0..=5; the
        // tdm4 shoulder parts (24, 11) are arm nodes; armour rows and the four default weapons exist; the damage type
        // of every ammunition row is set
        let nodes = |row: u32| {
            t.chars
                .bodies
                .get(&row)
                .and_then(|b| b.skeleton)
                .and_then(|s| t.chars.skeletons.get(&s))
                .cloned()
                .unwrap_or_default()
        };
        let n0 = nodes(0);
        assert!(n0.len() > 24, "{} nodes", n0.len());
        assert!(n0.iter().all(|n| n.location <= 5 && n.factor > 0.0));
        assert_eq!((n0[24].location, n0[11].location), (3, 3));
        assert!(n0.iter().any(|n| n.location == 1), "a head node");
        let typed = t
            .weapons
            .ammo
            .values()
            .filter(|a| a.damage_type.is_some())
            .count();
        println!(
            "ROM item rows: {} skeleton rows, {} armour rows ({} with armour > 0), default weapons {:?}, {typed} of {} \
             ammunition rows typed",
            t.chars.skeletons.len(),
            t.items.gear.len(),
            t.items.gear.values().filter(|g| g.armor > 0.0).count(),
            t.items.issue_weapons.map(|w| w.is_some()),
            t.weapons.ammo.len()
        );
        assert!(t.items.issue_weapons.iter().all(|w| w.is_some()));
        // DSV-38: the default weapons are weapon rows (item number = weapon number) the client can switch to
        for w in t.items.issue_weapons.iter().flatten() {
            assert!(t.weapons.weapons.contains_key(w), "weapon row");
        }
        let commands = t
            .items
            .issue_weapons
            .map(|w| w.and_then(|w| t.weapons.command_of(w)).is_some());
        println!("default weapons in the command table by slot: {commands:?}");
        assert!(commands[0], "the primary can be switched to with 0x7f");
        assert!(t
            .items
            .gear
            .values()
            .any(|g| g.armor > 0.0 && g.grade > 0.0));
        assert_eq!(typed, t.weapons.ammo.len());
        // DSV-39 inputs: ammunition rows that name a projectile row resolve in the FX table, and some of them have a
        // blast and a model
        let named: Vec<&AmmoRow> = t
            .weapons
            .ammo
            .values()
            .filter(|a| a.projectile_fx.is_some())
            .collect();
        let resolved = named
            .iter()
            .filter(|a| {
                t.projectiles
                    .get(a.projectile_fx.as_deref().unwrap())
                    .is_some()
            })
            .count();
        let blasts = t
            .weapons
            .ammo
            .keys()
            .filter(|&&a| t.weapons.area(u16::MAX, a).is_some())
            .count();
        println!(
            "ROM projectile rows: {}; {} ammunition rows name one ({resolved} resolve); {blasts} with a blast",
            t.projectiles.rows.len(),
            named.len()
        );
        assert!(!t.projectiles.rows.is_empty() && resolved > 0 && blasts > 0);
        assert!(t
            .projectiles
            .rows
            .values()
            .any(|r| r.model.is_some() && r.velocity.is_some() && r.lifetime_s.is_some()));
    }

    /// DSV-611: the user's FX table has mine class-data rows, projectile rows that name one carry their class and
    /// model, and a launched (`Type` 1) ammunition row of a weapon names such a row with a blast: the mine the
    /// server lays. Values are printed, never asserted (skipped without the ROM).
    /// ROM-gated (DSV-650): the grenade launcher 171 (ammunition `Type` 2) is a launched round like the rocket 29;
    /// the frag 7 and the flash grenade 8 (ammunition `Type` 1) are thrown. Counts only are printed.
    #[test]
    fn rom_the_grenade_launcher_is_launched_and_the_hand_grenades_thrown() {
        let Some(rom) = dc_rez::dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let (t, _) = RomTables::load(&rom);
        let mut by = BTreeMap::new();
        for (&w, row) in &t.weapons.weapons {
            let Some(a) = row.ammo else { continue };
            if let Some(f) = t.round_flight(w, a) {
                *by.entry((format!("{f:?}"), t.weapons.throws_round(w, a)))
                    .or_insert(0usize) += 1;
            }
        }
        println!("rows by (flight, Type 2): {by:?}");
        // the rows whose class overrides the `Type` rule, exactly: a ROM or pack change that re-classifies another
        // row (a throwing knife, a decoy, a flare) fails here
        let (mut launched2, mut thrown1, mut hand_impact) = (Vec::new(), Vec::new(), Vec::new());
        for (&w, row) in &t.weapons.weapons {
            let Some(a) = row.ammo else { continue };
            let two = t.weapons.throws_round(w, a);
            match t.round_flight(w, a) {
                Some(RoundFlight::Launched) if two => launched2.push(w),
                Some(RoundFlight::Thrown) if !two => thrown1.push(w),
                _ => {}
            }
            if t.round_flight(w, a) == Some(RoundFlight::Launched)
                && t.weapons.hand_thrown(w)
                && t.projectiles
                    .trap_of(t.weapons.projectile_fx(w, a).unwrap_or(""))
                    .is_none()
            {
                hand_impact.push(w);
            }
        }
        println!("Type 2 launched {launched2:?}; Type 1 thrown {thrown1:?}; hand-thrown impact {hand_impact:?}");
        // the user's ROM (2026-11-08): the airstrike shell guns 32 / 265 / 294, the snow guns 100 / 101 and the grenade
        // launchers 171 / 250 / 290 (all `ClientWeaponType` 1: fired, with a gun's clip); the flash, smoke and three
        // other hand grenades 8 / 9 / 14 / 18 / 19; no hand-thrown row has the impact class
        assert_eq!(launched2, [32, 100, 101, 171, 250, 265, 290, 294]);
        assert_eq!(thrown1, [8, 9, 14, 18, 19]);
        assert!(hand_impact.is_empty(), "{hand_impact:?}");
        for (w, want) in [
            (171u16, RoundFlight::Launched),
            (29, RoundFlight::Launched),
            (7, RoundFlight::Thrown),
            (8, RoundFlight::Thrown),
        ] {
            let a = t.weapons.ammo_of(w).expect("ammunition row");
            assert_eq!(t.round_flight(w, a), Some(want), "weapon {w}");
        }
        let a = t.weapons.ammo_of(171).unwrap();
        assert!(
            t.weapons.throws_round(171, a),
            "the launcher's row is Type 2"
        );
    }

    #[test]
    fn rom_mine_rows_load_with_their_class_and_keys() {
        let Some(rom) = dc_rez::dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let (t, _) = RomTables::load(&rom);
        assert!(!t.projectiles.traps.is_empty(), "mine class-data rows");
        for (n, tr) in &t.projectiles.traps {
            println!(
                "class data {n}: DetonateRadius {:?} ArmDelay {:?} -> {:.0} units, {} ms",
                tr.detonate_radius,
                tr.arm_delay_s,
                tr.radius(),
                tr.arm_ms()
            );
        }
        let mines: Vec<(&String, &ProjectileRow)> = t
            .projectiles
            .rows
            .iter()
            .filter(|(n, _)| t.projectiles.trap_of(n).is_some())
            .collect();
        assert!(!mines.is_empty(), "projectile rows with mine class data");
        for (n, r) in &mines {
            println!(
                "mine row {n}: class {:?}, model {:?}, mesh box {:?}",
                r.class, r.model, r.model_box
            );
            assert!(r.class.is_some() && r.model.is_some(), "{n}");
            // DSV-614 / DSV-618: the model's mesh is read for the sink and the shot box
            assert!(r.model_box.is_some(), "{n}: mesh box");
        }
        let thrown: Vec<u16> = t
            .weapons
            .weapons
            .iter()
            .filter_map(|(&w, row)| {
                let a = row.ammo?;
                let fx = t.weapons.projectile_fx(w, a)?;
                (t.projectiles.trap_of(fx).is_some()
                    && t.weapons.fires_projectile(w, a)
                    && t.weapons.area(w, a).is_some())
                .then_some(w)
            })
            .collect();
        println!("mine weapons (Type 1, mine class data, a blast): {thrown:?}");
        assert!(!thrown.is_empty());
        // DSV-619: a controller row names a mine weapon as its controllee
        let controllers: Vec<(u16, u16)> = t
            .weapons
            .weapons
            .keys()
            .filter_map(|&w| t.weapons.controllee(w).map(|c| (w, c)))
            .collect();
        println!("controllers (weapon, controllee): {controllers:?}");
        assert!(controllers.iter().any(|(_, c)| thrown.contains(c)));
    }

    /// DSV-80: default attachments become mod indices by `[Mod<n>]` name (case-insensitive, lowest n first); unknown
    /// or empty names and rows without defaults give -1.
    // EVIDENCE: cshell+0x17bd20, cshell+0x172510; DC-DEFAULT: DSV-80 (proposed)
    #[test]
    fn default_attachments_become_part_bytes() {
        // made-up rows in the key shape of the ROM table
        let t = WeaponTable::from_doc(&ButeDoc::parse(concat!(
            "[Mod0]
Name = \"Muzzle A\"
",
            "[Mod1]
Name = \"Scope A\"
",
            "[Mod2]
Name = \"scope a\"
",
            "[Mod3]
Name = \"Mag A\"
",
            "[Weapon0]
Name = \"Plain\"
DefaultScopeAttach = \"\"
",
            "[Weapon1]
Name = \"Scoped\"
DefaultScopeAttach = \"SCOPE A\"
",
            "[Weapon2]
Name = \"Full\"
DefaultScopeAttach = \"Scope A\"
",
            "DefaultMuzzleAttach = \"Muzzle A\"
DefaultMagazineAttach = \"Mag A\"
",
            "[Weapon3]
Name = \"Odd\"
DefaultScopeAttach = \"Nowhere\"
",
        )));
        assert_eq!(t.default_parts(0), [-1; 4], "empty default");
        assert_eq!(t.default_parts(1), [1, -1, -1, -1], "first match, any case");
        assert_eq!(
            t.default_parts(2),
            [1, 0, 3, -1],
            "scope, muzzle, magazine; finish -1"
        );
        assert_eq!(t.default_parts(3), [-1; 4], "unknown mod name");
        assert_eq!(t.default_parts(9), [-1; 4], "unknown weapon");
        assert_eq!(t.weapons[&3].default_parts, None);
    }

    /// DSV-99 / DSV-100: the player's mods go into the part of their socket over the defaults, only when the weapon
    /// row lists them and their `Kinds` is the socket; the zoom follows the sight socket's ZoomLevel.
    // DC-DEFAULT: DSV-99 (proposed), DSV-100 (proposed)
    #[test]
    fn player_attachments_override_the_defaults_per_socket() {
        // made-up rows in the key shape of the ROM table
        let t = WeaponTable::from_doc(&ButeDoc::parse(concat!(
            "[Mod0]
Name = \"Muzzle A\"
Kinds = 0
",
            "[Mod1]
Name = \"Sight Fixed\"
Kinds = 1
ZoomLevel = 1
",
            "[Mod2]
Name = \"Sight Plain\"
Kinds = 1
ZoomLevel = 0
",
            "[Mod3]
Name = \"Mag A\"
Kinds = 2
",
            "[Mod4]
Name = \"Sight Zoom\"
Kinds = 1
ZoomLevel = 1
",
            "[Weapon0]
Name = \"Rifle\"
ModName0 = \"muzzle a\"
ModName1 = \"Sight Zoom\"
ModName2 = \"Mag A\"
ModName3 = \"Sight Plain\"
",
            "[Weapon1]
Name = \"Sniper\"
DefaultScopeAttach = \"Sight Fixed\"
ModName0 = \"Muzzle A\"
",
        )));
        assert_eq!(t.weapons[&0].mod_list, vec![0, 4, 3, 2]);
        assert_eq!(t.mods[&3].socket, Some(SOCKET_MAGAZINE));
        assert_eq!(t.player_parts(0, [None; 3]), [-1; 4], "nothing chosen");
        assert_eq!(t.player_parts(0, [None, Some(4), None]), [4, -1, -1, -1]);
        assert_eq!(
            t.player_parts(0, [Some(0), Some(4), Some(3)]),
            [4, 0, 3, -1]
        );
        assert!(t.zooms_with(&t.player_parts(0, [None, Some(4), None])));
        assert!(
            !t.zooms_with(&t.player_parts(0, [None, Some(2), None])),
            "no zoom level"
        );
        assert!(!t.zooms_with(&[-1; 4]));
        // a mod in the wrong socket, a mod the row does not list, an unknown mod: not granted
        assert_eq!(t.player_parts(0, [Some(3), Some(0), None]), [-1; 4]);
        assert_eq!(t.player_parts(0, [None, Some(1), None]), [-1; 4]);
        assert_eq!(t.player_parts(0, [None, Some(99), None]), [-1; 4]);
        // the default stays in a socket without a valid choice; a listed muzzle joins it
        assert_eq!(t.player_parts(1, [None; 3]), [1, -1, -1, -1]);
        assert_eq!(t.player_parts(1, [Some(0), Some(4), None]), [1, 0, -1, -1]);
        assert!(t.zooms_with(&t.player_parts(1, [None; 3])));
        assert_eq!(
            t.player_parts(7, [Some(0), None, None]),
            [-1; 4],
            "unknown weapon"
        );
    }
}
