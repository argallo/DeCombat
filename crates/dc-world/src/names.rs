//! Names the readers of a world file compare object classes and properties against, in one place.
//!
//! Every ROM name is identified by `sha256(name)[:16]` and its length (no ROM string in the tree,
//! FMT_DAT85.md §12). A community map written by dc-mapkit carries DeCombat's own vocabulary instead
//! (`dc.*` classes and short property names, docs/MAPKIT_PLAN.md §4, docs/MAPKIT_VOCAB.md), and every reader
//! accepts those names as aliases next to the hashes through [`Key::matches`] (MAP-2).
//!
//! The constants below centralise the per-file hash tables of dc-dserver (`world.rs`, `worldlight.rs`,
//! `killvol.rs`, `destructibles.rs`, `evars.rs`), dc-render (`fog.rs`, `lights.rs`, `scene.rs`) and dc-sim
//! (`from_world.rs`), so those crates can replace their local `PropHash` / `(&str, usize)` constants with
//! `dc_world::names::X` and their local `prop()` helpers with [`Key::matches`]. The hash values are copied from
//! those files unchanged. A key whose `ours` is empty is ROM-only: no DeCombat name exists for it and only the hash
//! matches.
// SPEC: FMT_DAT85.md §6.1, §6.2, §12 (identify by hash)
// DC-DEFAULT: MAP-1 (the vocabulary), MAP-2 (alias matching rule)

use crate::roles::hash16;

const fn nib(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => panic!("bad hex digit in a name key"),
    }
}

/// 16 hex digits to 8 bytes (compile time only).
pub const fn hex8(s: &str) -> [u8; 8] {
    let b = s.as_bytes();
    assert!(b.len() == 16);
    let mut out = [0u8; 8];
    let mut i = 0;
    while i < 8 {
        out[i] = nib(b[2 * i]) << 4 | nib(b[2 * i + 1]);
        i += 1;
    }
    out
}

/// One name a reader compares against: the ROM name's hash prefix and length, and DeCombat's own name for it
/// (`ours`, empty for a ROM-only key).
// DC-DEFAULT: MAP-2 (exact ROM hash, or ASCII-case-insensitive equality with `ours`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    /// `sha256(rom name)[:8]`; all zero for a key that has no ROM name ([`Key::ours_only`]).
    pub hash: [u8; 8],
    /// Length of the ROM name in bytes; 0 for a key that has no ROM name.
    pub len: usize,
    /// DeCombat's own name (docs/MAPKIT_VOCAB.md); empty = ROM-only.
    pub ours: &'static str,
}

impl Key {
    /// A key from the 16-hex-digit hash prefix and length used by the readers today, plus our name.
    pub const fn new(hex: &str, len: usize, ours: &'static str) -> Key {
        Key {
            hash: hex8(hex),
            len,
            ours,
        }
    }

    /// A ROM-only key: no DeCombat name, only the hash matches.
    pub const fn rom_only(hex: &str, len: usize) -> Key {
        Key::new(hex, len, "")
    }

    /// A key that exists only in DeCombat's vocabulary (no ROM property corresponds one-to-one): a zero hash
    /// with length 0 matches no real property name (`sha256` of any name is never all zero).
    pub const fn ours_only(ours: &'static str) -> Key {
        Key {
            hash: [0; 8],
            len: 0,
            ours,
        }
    }

    /// Whether `name` (bytes as read from the user's file) is this key: the exact `(hash, len)` of the ROM
    /// name, or ASCII-case-insensitive equality with `ours`.
    // DC-DEFAULT: MAP-2
    pub fn matches(&self, name: &[u8]) -> bool {
        self.matches_ours(name) || self.matches_hash(name)
    }

    /// The ROM half of [`Key::matches`]: exact hash and length.
    pub fn matches_hash(&self, name: &[u8]) -> bool {
        self.len == name.len() && self.len != 0 && hash16(name) == self.hash
    }

    /// The DeCombat half of [`Key::matches`]: `ours` compared without ASCII case.
    pub fn matches_ours(&self, name: &[u8]) -> bool {
        !self.ours.is_empty() && name.eq_ignore_ascii_case(self.ours.as_bytes())
    }

    /// Whether an already computed `(hash16(name), name.len())` pair is this key's ROM name (for callers that
    /// compare precomputed `PropHash` pairs; the `ours` alias is not covered, use [`Key::matches`] on the name).
    pub fn is_hash(&self, hash: [u8; 8], len: usize) -> bool {
        self.len == len && self.len != 0 && self.hash == hash
    }

    /// `(hash, len)` in the `PropHash` shape of dc-dserver.
    pub const fn hash_len(&self) -> ([u8; 8], usize) {
        (self.hash, self.len)
    }

    /// Whether the key has a DeCombat name.
    pub const fn has_ours(&self) -> bool {
        !self.ours.is_empty()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The three fixed leading properties (by index in the file; the names are aliases for readers that look them up
// by name, FMT_DAT85.md §6.1).
// ---------------------------------------------------------------------------------------------------------------

/// Property 0, text: the object name (links a brush object to its BSP model, §6.4).
pub const PROP_NAME: Key = Key::new("dcd1d5223f73b3a9", 4, "name");
/// Property 1, vec3, flags 0x00100000: the position.
pub const PROP_POS: Key = Key::new("704e75860aa0d98d", 3, "pos");
/// Property 2, euler (three radians): the orientation.
pub const PROP_ROT: Key = Key::new("57b5e2fc1bba18bb", 8, "rot");

// ---------------------------------------------------------------------------------------------------------------
// The remaining `PropKey` rows (roles.rs): gameplay properties of the role classes.
// ---------------------------------------------------------------------------------------------------------------

/// Game-mode filter (text) on spawn points and zones: `modes` is a comma list of DeCombat mode names.
pub const PROP_MODES: Key = Key::new("e5792f5953852926", 8, "modes");
/// Team filter (text) on player spawn points and capture flags: `team` is `"1"`, `"2"` or `""` (any).
pub const PROP_TEAM_FILTER: Key = Key::new("1e1a1c078af2887a", 5, "team");
/// Team (text) on flag zones: `team` is `"1"` or `"2"`. Same DeCombat name as [`PROP_TEAM_FILTER`]; the reader
/// picks the key by role (docs/MAPKIT_PLAN.md §4 key mapping notes).
pub const PROP_TEAM: Key = Key::new("5985039f106df054", 4, "team");
/// Damage amount (real) on kill volumes; reserved in the vocabulary (the server kills outright).
pub const PROP_DAMAGE: Key = Key::new("0c874f581a4534f1", 6, "damage");
/// Damage kind (text) on kill volumes; reserved in the vocabulary.
pub const PROP_DAMAGE_KIND: Key = Key::new("29df1a8f796fdf57", 10, "damage_kind");
/// Damage-kind value: poison (the ROM hazard volumes' kind; `dc-dserver killvol::damage_type_of`).
// OBSERVED: the damage-kind texts of the ROM worlds' hazard volumes (hash only, 2026-11-05 survey)
pub const DAMAGE_KIND_POISON: Key = Key::new("500177cd8f619d5c", 6, "poison");
/// Box extent (vec3, flags 0x4) of box classes: HALF extents in the vocabulary.
pub const PROP_EXTENT: Key = Key::new("2d7b3d0c6d75ff11", 4, "extent");

// ---------------------------------------------------------------------------------------------------------------
// The sixteen role classes of FMT_DAT85.md §6.2 (`ObjectRole`, roles.rs ROLE_TABLE). The roles the vocabulary does
// not offer (trigger volume, mode controller, start-up command, camera point) are ROM-only.
// ---------------------------------------------------------------------------------------------------------------

pub const CLASS_PLAYER_SPAWN: Key = Key::new("6fe7e0eb8921d10c", 14, "dc.player_spawn");
pub const CLASS_ITEM_SPAWN: Key = Key::new("c25f64269d13d012", 16, "dc.item_spawn");
pub const CLASS_KILL_VOLUME: Key = Key::new("df93348241153dc0", 11, "dc.kill_volume");
pub const CLASS_TRIGGER_VOLUME: Key = Key::rom_only("8b9c643731c94119", 7);
pub const CLASS_MODE_CONTROLLER: Key = Key::rom_only("0878a7b251301f5d", 14);
pub const CLASS_CAPTURE_FLAG: Key = Key::new("e576ae011fa1c731", 8, "dc.capture_flag");
pub const CLASS_FLAG_ZONE: Key = Key::new("0d6a598a1816da49", 15, "dc.flag_zone");
pub const CLASS_BOMB_SITE: Key = Key::new("ab0d16e1dda32b6c", 15, "dc.bomb_site");
pub const CLASS_WORLD_SETTINGS: Key = Key::new("9819e9e19f000398", 15, "dc.world_settings");
pub const CLASS_STARTUP_COMMAND: Key = Key::rom_only("892748f74ce86ab8", 14);
pub const CLASS_LADDER: Key = Key::new("df0663641d3ef583", 6, "dc.ladder");
pub const CLASS_BREAKABLE: Key = Key::new("ebbd45154c24defa", 9, "dc.breakable");
pub const CLASS_WORLD_MODEL: Key = Key::new("33b5bf07771368ec", 10, "dc.world_model");
pub const CLASS_SKY_MODEL: Key = Key::new("689cfa21cab3e0cb", 17, "dc.sky_model");
pub const CLASS_HAZARD: Key = Key::new("4713c0097e04864f", 9, "dc.hazard");
pub const CLASS_CAMERA_POINT: Key = Key::rom_only("7b8770f70d81e7f7", 11);
/// The overview camera a community map cuts to at the round result (DC-DEFAULT: MAP-101). A ROM world does this with
/// camera objects and trigger scripts, which the vocabulary does not offer.
pub const CLASS_END_CAMERA: Key = Key::ours_only("dc.end_camera");

// ---------------------------------------------------------------------------------------------------------------
// Classes beyond the sixteen roles:
// ---------------------------------------------------------------------------------------------------------------

/// The world's global directional light (dc-render `lights.rs CLASS_GLOBAL`, dserver `worldlight.rs
/// CLASS_GLOBAL`): direction = rotated +Z of `rot`.
pub const CLASS_GLOBAL_LIGHT: Key = Key::new("7cea843ab1383e94", 14, "dc.global_light");
/// The point light the client's light scan creates a static light for (dc-render `lights.rs CLASS_POINT`).
pub const CLASS_POINT_LIGHT: Key = Key::new("dbcd5e7bb7a0f538", 5, "dc.point_light");
/// The directional light class of the scan (`lights.rs CLASS_DIRECTIONAL`); ROM-only.
pub const CLASS_DIRECTIONAL_LIGHT: Key = Key::rom_only("22b4211a53edd69c", 8);
/// The object light class of the scan (`lights.rs CLASS_OBJECT`); ROM-only.
pub const CLASS_OBJECT_LIGHT: Key = Key::rom_only("3cf839804e3c200b", 11);
/// The placed world sound (dserver `world.rs CLASS_SOUND`).
pub const CLASS_SOUND: Key = Key::new("80e9f4c6b49e600c", 7, "dc.sound");
/// The scaled sprite (dserver `world.rs CLASS_SCALE_SPRITE`).
pub const CLASS_SPRITE: Key = Key::new("ad52a1d2eb46a655", 11, "dc.sprite");
/// The placed client effect (dserver `world.rs CLASS_SPECIAL_FX`); ROM-only.
pub const CLASS_SPECIAL_FX: Key = Key::rom_only("dc800a8131725ee2", 9);
/// Model-linked classes with no DeCombat name (dserver `world.rs`; the doors and the switch are also dc-sim's
/// `MOVER_CLASSES`): ROM-only, scope cut D12.
pub const CLASS_SPINNING_MODEL: Key = Key::rom_only("23035079905e6954", 18);
pub const CLASS_ROTATING_MODEL: Key = Key::rom_only("89d1766daf24c936", 18);
pub const CLASS_SLIDING_DOOR: Key = Key::rom_only("f96e23498669c7b7", 11);
pub const CLASS_ROTATING_DOOR: Key = Key::rom_only("1782b26722e2707c", 12);
pub const CLASS_SWITCH: Key = Key::rom_only("b060f4405afec302", 14);
pub const CLASS_WATER: Key = Key::rom_only("7ca7dea906800f87", 5);
/// The explosion objects a breakable's death command switches on (dserver `destructibles.rs CLASS_EXPLOSION`, same
/// hash). ROM-only.
pub const CLASS_EXPLOSION: Key = Key::rom_only("ca6482a42babcb32", 9);
/// The world models that move with their object: sliding door, rotating door, rotating switch (dc-sim
/// `from_world.rs MOVER_CLASSES`, same hashes as the dserver constants). ROM-only.
pub const MOVER_CLASSES: [Key; 3] = [CLASS_SLIDING_DOOR, CLASS_ROTATING_DOOR, CLASS_SWITCH];

// ---------------------------------------------------------------------------------------------------------------
// Properties of the role classes beyond `PropKey` (dserver `world.rs` 303-320).
// ---------------------------------------------------------------------------------------------------------------

/// Bomb-site number (int): 0 = A, 1 = B (`world.rs PROP_ZONE_NUMBER`).
pub const PROP_NUMBER: Key = Key::new("5f4bf9e630a032fc", 10, "number");
/// Bomb site's defending team (text, optional in the vocabulary: absent = any team) (`PROP_DEFUSE_TEAM`).
pub const PROP_DEFUSE_TEAM: Key = Key::new("df717b9e3bde9ea6", 13, "defuse_team");
/// Bomb site's planted command text (`PROP_PLANTED_COMMAND`); ROM-only.
pub const PROP_PLANTED_COMMAND: Key = Key::rom_only("f7572ef0946db6e3", 14);
/// Capture flag's ground model (text, ROM LTB path; empty = the server's default flag look): the ROM file-name
/// property (`world.rs PROP_FILE_NAME`) under the vocabulary name `model`. The same ROM property is
/// [`PROP_SPRITE_FILE`] on a sprite.
pub const PROP_MODEL: Key = Key::new("971b974235a86ca6", 8, "model");
/// Kill volume's delay in seconds before the kill (real) (`world.rs` / `killvol.rs PROP_PENDING_SEC`).
pub const PROP_PENDING_SEC: Key = Key::new("ef4bd1495f4205eb", 10, "pending_sec");
/// World model visible (bool, default true) (`world.rs` / `scene.rs PROP_VISIBLE`).
pub const PROP_VISIBLE: Key = Key::new("8411f5abb4c710b1", 7, "visible");
/// World model solid (bool, default true) (`world.rs PROP_SOLID`).
pub const PROP_SOLID: Key = Key::new("b8b311b0f2739e0c", 5, "solid");
/// World model stops rays (bool, default true) (`world.rs PROP_RAYHIT`).
pub const PROP_RAYHIT: Key = Key::new("4faad5829950f829", 6, "rayhit");
/// World model blend mode (text; values [`BLEND_OPAQUE`], [`BLEND_TRANSLUCENT`], [`BLEND_ADDITIVE`])
/// (`world.rs` / `scene.rs PROP_BLEND_MODE`).
pub const PROP_BLEND: Key = Key::new("f6a07c0a7faf40de", 9, "blend");
/// Blend-mode value: opaque (`scene.rs BLEND_MODE_OPAQUE`).
pub const BLEND_OPAQUE: Key = Key::new("dc937b59892604f5", 4, "opaque");
/// Blend-mode value: translucent (`world.rs BLEND_TRANSLUCENT`).
pub const BLEND_TRANSLUCENT: Key = Key::new("fa5668c88f92faf4", 11, "translucent");
/// Blend-mode value: additive (`world.rs BLEND_ADDITIVE`, `scene.rs BLEND_MODE_ADDITIVE`).
pub const BLEND_ADDITIVE: Key = Key::new("765fc5369dc160b0", 8, "additive");
/// World model / sprite alpha (real 0..1) (`world.rs` / `scene.rs PROP_ALPHA`).
pub const PROP_ALPHA: Key = Key::new("b1a96dd646bccaa2", 5, "alpha");
/// World model hidden at start (bool) (`scene.rs PROP_HIDDEN`); ROM-only.
pub const PROP_HIDDEN: Key = Key::rom_only("7e6fefff0f0aeb6c", 6);
/// World model starts hidden (bool) (`scene.rs PROP_START_HIDDEN`); ROM-only.
pub const PROP_START_HIDDEN: Key = Key::rom_only("f5e282252cf55a76", 11);
/// Brush fog disable (bool) (`world.rs PROP_FOG_DISABLE`); ROM-only.
pub const PROP_FOG_DISABLE: Key = Key::rom_only("78fa952062604ece", 10);
/// Breakable hit points (real > 0) (`destructibles.rs PROP_HIT_POINTS`).
pub const PROP_HIT_POINTS: Key = Key::new("4028ccd1a5e1275d", 9, "hit_points");

// Sky brush (dserver `world.rs` 427-437).

/// Sky index (int) (`world.rs PROP_INDEX`).
pub const PROP_SKY_INDEX: Key = Key::new("39c3be1198a2fddf", 5, "sky_index");
/// Sky dimensions (vec3) (`world.rs PROP_SKY_DIMS`).
pub const PROP_SKY_DIMS: Key = Key::new("e62b876f06d008cf", 7, "sky_dims");
/// Sky inner percentages as ONE vec3 in the vocabulary. The ROM carries three reals instead
/// ([`SKY_INNER_PERCENT`]); an alias adopter that finds this key with a vec3 value serves component `i` as the
/// value of `SKY_INNER_PERCENT[i]` (docs/MAPKIT_PLAN.md §4 key mapping notes).
pub const PROP_SKY_INNER: Key = Key::ours_only("sky_inner");
/// The ROM's three sky inner-percent reals (`world.rs PROP_INNER_PERCENT`); ROM-only, see [`PROP_SKY_INNER`].
pub const SKY_INNER_PERCENT: [Key; 3] = [
    Key::rom_only("0ead0667b1ddb2dc", 13),
    Key::rom_only("244ff605c604fbca", 13),
    Key::rom_only("2f349b60cd95d9c8", 13),
];

// Volume container properties the server defaults for kill volumes and ladders (dserver `world.rs` 711-720);
// no DeCombat name (the vocabulary has no ladder viscosity, review 2026-11-01).

pub const PROP_VISCOSITY: Key = Key::rom_only("e5fae82fdecd6047", 9);
pub const PROP_FRICTION: Key = Key::rom_only("17e83883ff9cc3d1", 8);
pub const PROP_CURRENT: Key = Key::rom_only("e0d1b68224bf0b31", 7);
pub const PROP_TINT_COLOR: Key = Key::rom_only("1eac561883128374", 9);
pub const PROP_LIGHT_ADD: Key = Key::rom_only("ba99104aa5fd316b", 8);
pub const PROP_MOVEMENT_SOUNDS: Key = Key::rom_only("b89e096ff3df833c", 21);

// Command texts (dserver `world.rs` 455-469); the vocabulary has no scripts: ROM-only.

pub const PROP_COMMAND: Key = Key::rom_only("713166971d730f81", 7);
pub const PROP_TRIGGER_COMMANDS: [Key; 10] = [
    Key::rom_only("76f42e2f55263b82", 8),
    Key::rom_only("e0fef1a7f59b7b07", 8),
    Key::rom_only("204a7df2eff2fdd8", 8),
    Key::rom_only("097c42f63005a279", 8),
    Key::rom_only("2deb23312649cd50", 8),
    Key::rom_only("1d6ffa3457b673b5", 8),
    Key::rom_only("a7bcbc86232bc6a3", 8),
    Key::rom_only("6952944cebc5111b", 8),
    Key::rom_only("ef821d2104b4d1a5", 8),
    Key::rom_only("b38fa2ee23f2d62c", 9),
];
pub const PROP_OBJECT_START_COMMAND: Key = Key::rom_only("9cdffca770384e60", 18);

// ---------------------------------------------------------------------------------------------------------------
// World settings (dserver `evars.rs WORLD_VARS` property hashes, `world.rs` fog keys, dc-render `fog.rs`).
// The rows the vocabulary covers carry a DeCombat name; the fog colour is ONE rgb property that `evars.rs`
// splits into its three parts (`fog_r/g/b`), as the ROM property already is; the wind is one vec3 split into
// `wind_x/y/z`. A row matched through the alias has no file name to send as the evar name (`evars.rs name: None`).
// ---------------------------------------------------------------------------------------------------------------

/// Far plane (real).
pub const PROP_FAR_Z: Key = Key::new("12462a1852d9fd08", 4, "far_z");
/// Fog on (bool).
pub const PROP_FOG_ENABLE: Key = Key::new("b42e4232c66d28c4", 9, "fog_enable");
/// Fog near distance (real) (`world.rs PROP_FOG_NEAR_Z`, `fog.rs P_FOG_NEAR`).
pub const PROP_FOG_NEAR: Key = Key::new("63cb48901b40a07e", 8, "fog_near");
/// Fog far distance (real) (`world.rs PROP_FOG_FAR_Z`, `fog.rs P_FOG_FAR`).
pub const PROP_FOG_FAR: Key = Key::new("068c8f33716eac9c", 7, "fog_far");
/// Fog colour (rgb 0..255) (`world.rs PROP_FOG_COLOR`, `fog.rs P_FOG_COLOUR`, `evars.rs` parts 0..2).
pub const PROP_FOG_COLOR: Key = Key::new("5c589613c2ea1c49", 8, "fog_color");
/// Sky-pass fog on (bool).
pub const PROP_SKY_FOG_ENABLE: Key = Key::new("cd9c0c6e0d40407e", 12, "sky_fog_enable");
/// Sky-pass fog near (real) (`fog.rs P_SKY_FOG_NEAR`).
pub const PROP_SKY_FOG_NEAR: Key = Key::new("10eb8275f09917ef", 11, "sky_fog_near");
/// Sky-pass fog far (real) (`fog.rs P_SKY_FOG_FAR`).
pub const PROP_SKY_FOG_FAR: Key = Key::new("3885263f3d410645", 10, "sky_fog_far");
/// Wind (vec3) (`evars.rs` parts 0..2 of the `wind_x/y/z` rows).
pub const PROP_WIND: Key = Key::new("d151346fe7eea3c6", 4, "wind");
/// Every sky portal open (bool).
pub const PROP_ALL_SKY_PORTALS: Key = Key::new("5f483004cf36e238", 13, "all_sky_portals");
/// Both faces of the level and its opaque world models drawn (bool, absent = false) (dc-render `scene.rs
/// world_two_sided`). DeCombat's own: no ROM property corresponds, the server sends no variable for it, and on a ROM
/// object it never matches (MAP-2).
// DC-DEFAULT: MAP-118
pub const PROP_TWO_SIDED: Key = Key::ours_only("two_sided");

// ---------------------------------------------------------------------------------------------------------------
// Lights (dc-render `lights.rs` 25-54, dserver `worldlight.rs` 17-23).
// ---------------------------------------------------------------------------------------------------------------

/// Light colour (rgb 0..255) (`lights.rs` / `worldlight.rs P_COLOUR`). Both readers fall back to
/// [`PROP_LIGHT_INNER_COLOR`], so a map that writes `color` is read by either.
pub const PROP_LIGHT_COLOR: Key = Key::new("13b4d1b9cdf6ee57", 10, "color");
/// The alternative colour (`P_INNER_COLOUR`); ROM-only.
pub const PROP_LIGHT_INNER_COLOR: Key = Key::rom_only("9a072808beba8a7a", 10);
/// Bright scale (real, default 1) (`P_BRIGHT`).
pub const PROP_BRIGHT: Key = Key::new("1ae3f3a79e685b27", 11, "bright");
/// Object bright scale (real, default 1) (`P_OBJECT_BRIGHT`); ROM-only.
pub const PROP_OBJECT_BRIGHT: Key = Key::rom_only("639a93a207df88a6", 17);
/// Convert-to-ambient fraction (real, default 0) (`P_TO_AMBIENT`).
pub const PROP_TO_AMBIENT: Key = Key::new("dd6d73b963113d14", 16, "to_ambient");
/// Point light radius (real, required > 0) (`lights.rs P_RADIUS`).
pub const PROP_RADIUS: Key = Key::new("bc4d3952329d2f76", 11, "radius");
/// Cone angle in degrees (real; absent or wide = point light) (`lights.rs P_CONE_DEG`).
pub const PROP_CONE: Key = Key::new("5608c2a19433a83e", 3, "cone");
/// Lights objects (bool; the scan returns no light unless true) (`lights.rs P_LIGHTS_OBJECTS`).
pub const PROP_LIGHTS_OBJECTS: Key = Key::new("9b88161ea169513c", 12, "lights_objects");
/// Fast lights objects (bool) (`lights.rs P_FAST_LIGHTS_OBJECTS`); ROM-only.
pub const PROP_FAST_LIGHTS_OBJECTS: Key = Key::rom_only("09ba7c7657f8c9e1", 16);
/// Attenuation coefficients (vec3), exponents (vec3) and kind (text) (`lights.rs`); ROM-only.
pub const PROP_ATT_COEFS: Key = Key::rom_only("189127eb5517ce75", 8);
pub const PROP_ATT_EXPS: Key = Key::rom_only("c40ad9b59d1eaa88", 7);
pub const PROP_ATTENUATION: Key = Key::rom_only("1201a8d9c29928f4", 11);
/// Attenuation-kind values (`lights.rs V_D3D / V_QUARTIC / V_LINEAR`); ROM-only.
pub const ATT_D3D: Key = Key::rom_only("ed0cb080d1dbcd61", 3);
pub const ATT_QUARTIC: Key = Key::rom_only("4d02e4a79f39de49", 7);
pub const ATT_LINEAR: Key = Key::rom_only("e6950b45d25c2027", 6);

// ---------------------------------------------------------------------------------------------------------------
// Sounds (dserver `world.rs` 415-425).
// ---------------------------------------------------------------------------------------------------------------

/// Sound file (text: ROM WAV path or `@mod:` path) (`world.rs PROP_SOUND_FILE`).
pub const PROP_SOUND_FILE: Key = Key::new("46124e023603420f", 5, "file");
/// Volume percent (int, default 100) (`PROP_VOLUME`).
pub const PROP_VOLUME: Key = Key::new("b10fb966d72063f5", 6, "volume");
/// Inner radius (real, default = outer) (`PROP_INNER_RADIUS`).
pub const PROP_INNER_RADIUS: Key = Key::new("487cc23156cc7e95", 11, "inner_radius");
/// Outer radius (real, default 0) (`PROP_OUTER_RADIUS`).
pub const PROP_OUTER_RADIUS: Key = Key::new("4bf718bcf0e0bbd2", 11, "outer_radius");
/// Looped (bool, default false) (`PROP_LOOP`; the effect class reads the same property).
pub const PROP_LOOP: Key = Key::new("f2f6a01889951edc", 4, "loop");
/// Ambient, no direction (bool, default false) (`PROP_AMBIENT`).
pub const PROP_AMBIENT: Key = Key::new("44bb0e5fb3702293", 7, "ambient");
/// Priority (int), pitch factor (real), attach to player (bool), start switched on (bool); ROM-only.
pub const PROP_PRIORITY: Key = Key::rom_only("d60dbba079223254", 8);
pub const PROP_PITCH_SHIFT: Key = Key::rom_only("17c05affb431981f", 10);
pub const PROP_PLAY_ATTACHED: Key = Key::rom_only("ba008f2d56371c77", 12);
pub const PROP_START_ON: Key = Key::rom_only("0ef0d5b4d098b9e0", 7);

// ---------------------------------------------------------------------------------------------------------------
// Sprites (dserver `world.rs` 428-434, the file name of 303-320) and effects.
// ---------------------------------------------------------------------------------------------------------------

/// Sprite file (text: ROM SPR/DTX path or `@mod:` path): the ROM file-name property (same hash as
/// [`PROP_MODEL`]) under the vocabulary name `file`.
pub const PROP_SPRITE_FILE: Key = Key::new("971b974235a86ca6", 8, "file");
/// Sprite scale x / y (real, default 1) (`PROP_SCALE_X` / `PROP_SCALE_Y`).
pub const PROP_SCALE_X: Key = Key::new("210f3003763682c3", 6, "scale_x");
pub const PROP_SCALE_Y: Key = Key::new("801f279731ef5c30", 6, "scale_y");
/// Sprite colour (rgb 0..255, default 255) (`PROP_COLOR`; a different ROM property from the light colour).
pub const PROP_SPRITE_COLOR: Key = Key::new("6b73191a0a4b6742", 5, "color");
/// Effect name (text) of the placed client effect (`PROP_FX_NAME`); ROM-only.
pub const PROP_FX_NAME: Key = Key::rom_only("1c216057c33fb291", 6);

/// Every key of this module, for tooling and the vocabulary tests.
pub const ALL: &[&Key] = &[
    &PROP_NAME,
    &PROP_POS,
    &PROP_ROT,
    &PROP_MODES,
    &PROP_TEAM_FILTER,
    &PROP_TEAM,
    &PROP_DAMAGE,
    &PROP_DAMAGE_KIND,
    &DAMAGE_KIND_POISON,
    &PROP_EXTENT,
    &CLASS_PLAYER_SPAWN,
    &CLASS_ITEM_SPAWN,
    &CLASS_KILL_VOLUME,
    &CLASS_TRIGGER_VOLUME,
    &CLASS_MODE_CONTROLLER,
    &CLASS_CAPTURE_FLAG,
    &CLASS_FLAG_ZONE,
    &CLASS_BOMB_SITE,
    &CLASS_WORLD_SETTINGS,
    &CLASS_STARTUP_COMMAND,
    &CLASS_LADDER,
    &CLASS_BREAKABLE,
    &CLASS_WORLD_MODEL,
    &CLASS_SKY_MODEL,
    &CLASS_HAZARD,
    &CLASS_CAMERA_POINT,
    &CLASS_GLOBAL_LIGHT,
    &CLASS_POINT_LIGHT,
    &CLASS_DIRECTIONAL_LIGHT,
    &CLASS_OBJECT_LIGHT,
    &CLASS_SOUND,
    &CLASS_END_CAMERA,
    &CLASS_SPRITE,
    &CLASS_SPECIAL_FX,
    &CLASS_SPINNING_MODEL,
    &CLASS_ROTATING_MODEL,
    &CLASS_SLIDING_DOOR,
    &CLASS_ROTATING_DOOR,
    &CLASS_SWITCH,
    &CLASS_WATER,
    &PROP_NUMBER,
    &PROP_DEFUSE_TEAM,
    &PROP_PLANTED_COMMAND,
    &PROP_MODEL,
    &PROP_PENDING_SEC,
    &PROP_VISIBLE,
    &PROP_SOLID,
    &PROP_RAYHIT,
    &PROP_BLEND,
    &BLEND_OPAQUE,
    &BLEND_TRANSLUCENT,
    &BLEND_ADDITIVE,
    &PROP_ALPHA,
    &PROP_HIDDEN,
    &PROP_START_HIDDEN,
    &PROP_FOG_DISABLE,
    &PROP_HIT_POINTS,
    &PROP_SKY_INDEX,
    &PROP_SKY_DIMS,
    &PROP_SKY_INNER,
    &SKY_INNER_PERCENT[0],
    &SKY_INNER_PERCENT[1],
    &SKY_INNER_PERCENT[2],
    &PROP_VISCOSITY,
    &PROP_FRICTION,
    &PROP_CURRENT,
    &PROP_TINT_COLOR,
    &PROP_LIGHT_ADD,
    &PROP_MOVEMENT_SOUNDS,
    &PROP_COMMAND,
    &PROP_TRIGGER_COMMANDS[0],
    &PROP_TRIGGER_COMMANDS[1],
    &PROP_TRIGGER_COMMANDS[2],
    &PROP_TRIGGER_COMMANDS[3],
    &PROP_TRIGGER_COMMANDS[4],
    &PROP_TRIGGER_COMMANDS[5],
    &PROP_TRIGGER_COMMANDS[6],
    &PROP_TRIGGER_COMMANDS[7],
    &PROP_TRIGGER_COMMANDS[8],
    &PROP_TRIGGER_COMMANDS[9],
    &PROP_OBJECT_START_COMMAND,
    &PROP_FAR_Z,
    &PROP_FOG_ENABLE,
    &PROP_FOG_NEAR,
    &PROP_FOG_FAR,
    &PROP_FOG_COLOR,
    &PROP_SKY_FOG_ENABLE,
    &PROP_SKY_FOG_NEAR,
    &PROP_SKY_FOG_FAR,
    &PROP_WIND,
    &PROP_ALL_SKY_PORTALS,
    &PROP_TWO_SIDED,
    &PROP_LIGHT_COLOR,
    &PROP_LIGHT_INNER_COLOR,
    &PROP_BRIGHT,
    &PROP_OBJECT_BRIGHT,
    &PROP_TO_AMBIENT,
    &PROP_RADIUS,
    &PROP_CONE,
    &PROP_LIGHTS_OBJECTS,
    &PROP_FAST_LIGHTS_OBJECTS,
    &PROP_ATT_COEFS,
    &PROP_ATT_EXPS,
    &PROP_ATTENUATION,
    &ATT_D3D,
    &ATT_QUARTIC,
    &ATT_LINEAR,
    &PROP_SOUND_FILE,
    &PROP_VOLUME,
    &PROP_INNER_RADIUS,
    &PROP_OUTER_RADIUS,
    &PROP_LOOP,
    &PROP_AMBIENT,
    &PROP_PRIORITY,
    &PROP_PITCH_SHIFT,
    &PROP_PLAY_ATTACHED,
    &PROP_START_ON,
    &PROP_SPRITE_FILE,
    &PROP_SCALE_X,
    &PROP_SCALE_Y,
    &PROP_SPRITE_COLOR,
    &PROP_FX_NAME,
];

/// The keys whose DeCombat name equals `ours` (ASCII case ignored). Several keys may share a name when the same
/// word means different ROM properties on different classes (`team`, `color`, `file`).
pub fn keys_named(ours: &str) -> impl Iterator<Item = &'static Key> + '_ {
    ALL.iter()
        .copied()
        .filter(move |k| k.has_ours() && k.ours.eq_ignore_ascii_case(ours))
}

/// Whether `class_name` (bytes from a file) is one of DeCombat's own `dc.*` class names, ASCII case ignored: the
/// object is a vocabulary object and its properties may be named by their own names ([`crate::Object::dc`]). A ROM
/// class never is (the hash half still resolves its role and key), so the own-name half of a property key never
/// fires on a ROM object whose ROM property happens to spell a vocabulary word (`rayhit`, `team`, `color`, `radius`).
// DC-DEFAULT: MAP-2 (the own-name half of a property key applies on `dc.*` objects only)
pub fn is_dc_class(class_name: &[u8]) -> bool {
    ALL.iter()
        .any(|k| k.ours.starts_with("dc.") && k.matches_ours(class_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_class_is_an_own_class_name_without_case() {
        assert!(is_dc_class(b"dc.player_spawn"));
        assert!(is_dc_class(b"DC.World_Model"));
        assert!(is_dc_class(b"dc.sprite"));
        assert!(!is_dc_class(b"dc.not_a_class"));
        assert!(!is_dc_class(b"dc."));
        assert!(!is_dc_class(b""));
        assert!(!is_dc_class(b"player_spawn"));
        // a property's own name is not a class name
        assert!(!is_dc_class(b"rayhit"));
        // every own class name of the table is one, and only those
        for k in ALL {
            assert_eq!(
                is_dc_class(k.ours.as_bytes()),
                k.ours.starts_with("dc."),
                "{:?}",
                k.ours
            );
        }
        assert_eq!(ALL.iter().filter(|k| k.ours.starts_with("dc.")).count(), 17);
    }

    #[test]
    fn matches_by_hash_or_own_name() {
        // The ROM half: the hash of a made-up name, so the test needs no ROM string.
        let k = Key {
            hash: hash16(b"MadeUpName"),
            len: 10,
            ours: "made_up",
        };
        assert!(k.matches(b"MadeUpName"));
        assert!(!k.matches(b"MadeUpNamf"));
        assert!(!k.matches(b"MadeUpNam"));
        assert!(k.matches(b"made_up"));
        assert!(k.matches(b"MADE_UP"));
        assert!(k.matches(b"Made_Up"));
        assert!(!k.matches(b"made_up2"));
        assert!(!k.matches(b""));
        assert!(k.is_hash(hash16(b"MadeUpName"), 10));
        assert!(!k.is_hash(hash16(b"MadeUpName"), 9));
        assert_eq!(k.hash_len(), (hash16(b"MadeUpName"), 10));
    }

    #[test]
    fn rom_only_never_matches_a_name_text() {
        let k = Key::rom_only("0000000000000001", 4);
        assert!(!k.has_ours());
        assert!(!k.matches(b""));
        assert!(!k.matches(b"name"));
        assert!(!k.matches_ours(b"name"));
    }

    #[test]
    fn ours_only_matches_nothing_by_hash() {
        assert!(PROP_SKY_INNER.matches(b"sky_inner"));
        assert!(PROP_SKY_INNER.matches(b"SKY_INNER"));
        assert!(!PROP_SKY_INNER.matches(b""));
        assert!(!PROP_SKY_INNER.matches_hash(b""));
        assert!(!PROP_SKY_INNER.is_hash([0; 8], 0));
    }

    #[test]
    fn leading_keys_agree_with_prop_key() {
        use crate::PropKey;
        assert_eq!(PropKey::Name.hash(), PROP_NAME.hash_len());
        assert_eq!(PropKey::Position.hash(), PROP_POS.hash_len());
        assert_eq!(PropKey::Rotation.hash(), PROP_ROT.hash_len());
        assert_eq!(PropKey::ModeFilter.hash(), PROP_MODES.hash_len());
        assert_eq!(PropKey::TeamFilter.hash(), PROP_TEAM_FILTER.hash_len());
        assert_eq!(PropKey::Team.hash(), PROP_TEAM.hash_len());
        assert_eq!(PropKey::Damage.hash(), PROP_DAMAGE.hash_len());
        assert_eq!(PropKey::DamageKind.hash(), PROP_DAMAGE_KIND.hash_len());
        assert_eq!(PropKey::BoxExtent.hash(), PROP_EXTENT.hash_len());
    }

    #[test]
    fn own_names_are_short_lower_case_identifiers() {
        for k in ALL {
            if !k.has_ours() {
                continue;
            }
            let ok = k
                .ours
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.');
            assert!(ok, "{:?}", k.ours);
            assert!(
                k.ours.starts_with("dc.") == k.ours.contains('.'),
                "{:?}",
                k.ours
            );
        }
    }

    #[test]
    fn all_lists_every_key_once() {
        let mut seen = std::collections::HashSet::new();
        for k in ALL {
            assert!(seen.insert((k.hash, k.len, k.ours)), "{k:?} listed twice");
        }
        assert_eq!(keys_named("TEAM").count(), 2);
        assert_eq!(keys_named("file").count(), 2);
        assert_eq!(keys_named("color").count(), 2);
        assert_eq!(keys_named("no_such_key").count(), 0);
    }
}
