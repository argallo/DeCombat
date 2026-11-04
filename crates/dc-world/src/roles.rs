//! Gameplay roles of object classes, identified by `sha256(name)[:16]` and length so that no ROM string is
//! stored in the tree, or by DeCombat's own `dc.*` class name (docs/MAPKIT_VOCAB.md). Role names are ours.
// SPEC: FMT_DAT85.md §6.2 (class-hash roles), §12 (identify by hash)
// DC-DEFAULT: MAP-1 (the `dc.*` class names), MAP-2 (alias matching through `names::Key`)

use crate::names::{self, Key};

/// First 8 bytes (16 hex digits) of SHA-256 of `name`.
pub fn hash16(name: &[u8]) -> [u8; 8] {
    let d = dc_rom::sha256(name);
    let mut out = [0u8; 8];
    out.copy_from_slice(&d[..8]);
    out
}

/// Lower-case hex of [`hash16`].
pub fn hash16_hex(name: &[u8]) -> String {
    hash16(name).iter().map(|b| format!("{b:02x}")).collect()
}

/// How an object of a role occupies space.
// SPEC: FMT_DAT85.md §6.2 (geometry column)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Geometry {
    /// Position + orientation only.
    Point,
    /// Position plus the box-extent property ([`PropKey::BoxExtent`]).
    Box,
    /// The BSP model with the same name as the object (FMT_DAT85.md §6.4).
    Brush,
}

/// Gameplay role of an object class.
// SPEC: FMT_DAT85.md §6.2 (one variant per row of the class-hash table)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectRole {
    PlayerSpawn,
    ItemSpawn,
    KillVolume,
    TriggerVolume,
    ModeController,
    CaptureFlag,
    FlagZone,
    BombSite,
    WorldSettings,
    StartupCommand,
    Ladder,
    Breakable,
    PlainWorldModel,
    SkyWorldModel,
    HazardVolume,
    CameraPoint,
}

/// The class key of every role (`names::CLASS_*`): the ROM class hash and length (FMT_DAT85.md §6.2) and the
/// `dc.*` name of docs/MAPKIT_PLAN.md §4 (empty for the roles the vocabulary does not offer: ROM-only).
// SPEC: FMT_DAT85.md §6.2 (role, class hash, length, geometry)
// DC-DEFAULT: MAP-1
const ROLE_TABLE: [(ObjectRole, Key, Geometry); 16] = [
    (
        ObjectRole::PlayerSpawn,
        names::CLASS_PLAYER_SPAWN,
        Geometry::Point,
    ),
    (
        ObjectRole::ItemSpawn,
        names::CLASS_ITEM_SPAWN,
        Geometry::Point,
    ),
    (
        ObjectRole::KillVolume,
        names::CLASS_KILL_VOLUME,
        Geometry::Brush,
    ),
    (
        ObjectRole::TriggerVolume,
        names::CLASS_TRIGGER_VOLUME,
        Geometry::Box,
    ),
    (
        ObjectRole::ModeController,
        names::CLASS_MODE_CONTROLLER,
        Geometry::Point,
    ),
    (
        ObjectRole::CaptureFlag,
        names::CLASS_CAPTURE_FLAG,
        Geometry::Point,
    ),
    (ObjectRole::FlagZone, names::CLASS_FLAG_ZONE, Geometry::Box),
    (ObjectRole::BombSite, names::CLASS_BOMB_SITE, Geometry::Box),
    (
        ObjectRole::WorldSettings,
        names::CLASS_WORLD_SETTINGS,
        Geometry::Point,
    ),
    (
        ObjectRole::StartupCommand,
        names::CLASS_STARTUP_COMMAND,
        Geometry::Point,
    ),
    (ObjectRole::Ladder, names::CLASS_LADDER, Geometry::Brush),
    (
        ObjectRole::Breakable,
        names::CLASS_BREAKABLE,
        Geometry::Brush,
    ),
    (
        ObjectRole::PlainWorldModel,
        names::CLASS_WORLD_MODEL,
        Geometry::Brush,
    ),
    (
        ObjectRole::SkyWorldModel,
        names::CLASS_SKY_MODEL,
        Geometry::Brush,
    ),
    (
        ObjectRole::HazardVolume,
        names::CLASS_HAZARD,
        Geometry::Brush,
    ),
    (
        ObjectRole::CameraPoint,
        names::CLASS_CAMERA_POINT,
        Geometry::Point,
    ),
];

impl ObjectRole {
    pub const ALL: [ObjectRole; 16] = [
        ObjectRole::PlayerSpawn,
        ObjectRole::ItemSpawn,
        ObjectRole::KillVolume,
        ObjectRole::TriggerVolume,
        ObjectRole::ModeController,
        ObjectRole::CaptureFlag,
        ObjectRole::FlagZone,
        ObjectRole::BombSite,
        ObjectRole::WorldSettings,
        ObjectRole::StartupCommand,
        ObjectRole::Ladder,
        ObjectRole::Breakable,
        ObjectRole::PlainWorldModel,
        ObjectRole::SkyWorldModel,
        ObjectRole::HazardVolume,
        ObjectRole::CameraPoint,
    ];

    fn row(self) -> &'static (ObjectRole, Key, Geometry) {
        // ROLE_TABLE has one row per variant, in ALL order.
        &ROLE_TABLE[self as usize]
    }

    /// Role of a class name read from the user's file: by hash and length (a ROM class) or by its `dc.*` name,
    /// ASCII case ignored (a DeCombat map).
    // DC-DEFAULT: MAP-2
    pub fn of_class(class_name: &[u8]) -> Option<ObjectRole> {
        if let Some(r) = ROLE_TABLE.iter().find(|r| r.1.matches_ours(class_name)) {
            return Some(r.0);
        }
        if !ROLE_TABLE.iter().any(|r| r.1.len == class_name.len()) {
            return None;
        }
        let h = hash16(class_name);
        ROLE_TABLE
            .iter()
            .find(|r| r.1.is_hash(h, class_name.len()))
            .map(|r| r.0)
    }

    /// `(sha256(class)[:8 bytes], class name length)`.
    pub fn class_hash(self) -> ([u8; 8], usize) {
        self.row().1.hash_len()
    }

    /// The class key: ROM hash and length plus the `dc.*` name (empty for a ROM-only role).
    pub fn class_key(self) -> &'static Key {
        &self.row().1
    }

    /// The `dc.*` class name of the role, if the vocabulary offers it.
    pub fn dc_name(self) -> Option<&'static str> {
        let k = self.class_key();
        k.has_ours().then_some(k.ours)
    }

    pub fn geometry(self) -> Geometry {
        self.row().2
    }
}

/// Property names with a known meaning, identified by hash and length, or by their DeCombat name.
// SPEC: FMT_DAT85.md §6.1 and §6.2 (property hashes)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropKey {
    /// Property 0, text.
    Name,
    /// Property 1, vec3.
    Position,
    /// Property 2, euler.
    Rotation,
    /// Game-mode filter (text) on spawn points and zones.
    ModeFilter,
    /// Team filter (text) on player spawn points.
    TeamFilter,
    /// Damage amount (real) on kill volumes.
    Damage,
    /// Damage kind (text) on kill volumes.
    DamageKind,
    /// Box extent (vec3, flags 0x4) of box-shaped classes; half vs full extent is not resolved.
    BoxExtent,
    /// Team (text) on flag capture zones.
    Team,
}

impl PropKey {
    /// The key: ROM hash and length plus the DeCombat name (`names::PROP_*`).
    pub fn key(self) -> &'static Key {
        match self {
            PropKey::Name => &names::PROP_NAME,
            PropKey::Position => &names::PROP_POS,
            PropKey::Rotation => &names::PROP_ROT,
            PropKey::ModeFilter => &names::PROP_MODES,
            PropKey::TeamFilter => &names::PROP_TEAM_FILTER,
            PropKey::Damage => &names::PROP_DAMAGE,
            PropKey::DamageKind => &names::PROP_DAMAGE_KIND,
            PropKey::BoxExtent => &names::PROP_EXTENT,
            PropKey::Team => &names::PROP_TEAM,
        }
    }

    /// `(sha256(name)[:8 bytes], name length)` of the ROM property name.
    pub fn hash(self) -> ([u8; 8], usize) {
        self.key().hash_len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_order_matches_all() {
        for (i, r) in ObjectRole::ALL.iter().enumerate() {
            assert_eq!(ROLE_TABLE[i].0, *r);
            assert_eq!(*r as usize, i);
        }
    }

    #[test]
    fn unknown_class_has_no_role() {
        assert_eq!(ObjectRole::of_class(b"NotAClass"), None);
        assert_eq!(ObjectRole::of_class(b"dc.not_a_class"), None);
        assert_eq!(ObjectRole::of_class(b""), None);
        assert_eq!(hash16_hex(b"").len(), 16);
    }

    #[test]
    fn dc_names_resolve_without_case() {
        for r in ObjectRole::ALL {
            let Some(n) = r.dc_name() else {
                assert!(!r.class_key().has_ours());
                continue;
            };
            assert_eq!(ObjectRole::of_class(n.as_bytes()), Some(r));
            assert_eq!(
                ObjectRole::of_class(n.to_ascii_uppercase().as_bytes()),
                Some(r)
            );
        }
        assert_eq!(ObjectRole::TriggerVolume.dc_name(), None);
        assert_eq!(ObjectRole::ModeController.dc_name(), None);
        assert_eq!(ObjectRole::StartupCommand.dc_name(), None);
        assert_eq!(ObjectRole::CameraPoint.dc_name(), None);
        assert_eq!(
            ObjectRole::ALL
                .iter()
                .filter(|r| r.dc_name().is_some())
                .count(),
            12
        );
    }

    #[test]
    fn dc_names_are_distinct() {
        let mut names: Vec<&str> = ObjectRole::ALL.iter().filter_map(|r| r.dc_name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 12);
    }
}
