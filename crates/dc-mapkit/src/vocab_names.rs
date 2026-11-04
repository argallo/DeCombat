//! DeCombat's own map vocabulary: the `dc.*` class names and short property names a community map writes
//! (MAPKIT_PLAN.md section 4, D1). The readers (dc-dserver, dc-sim, dc-render, dc-client) learn them as aliases
//! next to the ROM hashes; no ROM name appears here or anywhere in a project.
//!
//! TODO(mapkit-stage2): switch to `dc_world::vocab` once the P0 dc-world branch lands; keep the strings equal.

/// Player spawn point (`team`, `modes`).
pub const CLASS_PLAYER_SPAWN: &str = "dc.player_spawn";
/// Item / weapon spawn (`modes`).
pub const CLASS_ITEM_SPAWN: &str = "dc.item_spawn";
/// Bomb site box (`extent` HALF extents, `number` 0 = A / 1 = B, `modes`, optional `defuse_team`).
pub const CLASS_BOMB_SITE: &str = "dc.bomb_site";
/// Flag capture zone (`extent`, `team`, `modes`).
pub const CLASS_FLAG_ZONE: &str = "dc.flag_zone";
/// Capture flag (`team`, `model`; empty model = the server's default look).
pub const CLASS_CAPTURE_FLAG: &str = "dc.capture_flag";
/// Kill volume brush (`pending_sec`).
pub const CLASS_KILL_VOLUME: &str = "dc.kill_volume";
/// Hazard volume brush (no server behaviour today).
pub const CLASS_HAZARD: &str = "dc.hazard";
/// Ladder brush (convex; climbable by its container kind alone).
pub const CLASS_LADDER: &str = "dc.ladder";
/// Plain world-model brush (`visible`, `solid`, `rayhit`, `blend`, `alpha`).
pub const CLASS_WORLD_MODEL: &str = "dc.world_model";
/// Breakable brush (`hit_points`).
pub const CLASS_BREAKABLE: &str = "dc.breakable";
/// Sky box brush (`sky_index`, `sky_dims`, `sky_inner`).
pub const CLASS_SKY_MODEL: &str = "dc.sky_model";
/// World settings (fog, far plane, wind).
pub const CLASS_WORLD_SETTINGS: &str = "dc.world_settings";
/// The sun: `color`, `bright`, `to_ambient`; direction = `rot`.
pub const CLASS_GLOBAL_LIGHT: &str = "dc.global_light";
/// Point light for characters and held weapons (`color`, `radius`, `bright`, `cone`, `lights_objects`).
pub const CLASS_POINT_LIGHT: &str = "dc.point_light";
/// World sound (`file`, `volume`, `inner_radius`, `outer_radius`, `loop`, `ambient`).
pub const CLASS_SOUND: &str = "dc.sound";
pub const CLASS_END_CAMERA: &str = "dc.end_camera";
/// Scale sprite (`file`, `scale_x`, `scale_y`, `color`, `alpha`).
pub const CLASS_SPRITE: &str = "dc.sprite";

/// Every point class an `add` op accepts (brush classes come from solids with a `brush` role, not from `add`).
pub const POINT_CLASSES: &[&str] = &[
    CLASS_PLAYER_SPAWN,
    CLASS_ITEM_SPAWN,
    CLASS_BOMB_SITE,
    CLASS_FLAG_ZONE,
    CLASS_CAPTURE_FLAG,
    CLASS_WORLD_SETTINGS,
    CLASS_GLOBAL_LIGHT,
    CLASS_POINT_LIGHT,
    CLASS_SOUND,
    CLASS_END_CAMERA,
    CLASS_SPRITE,
];

/// The brush classes a solid with a `brush` role becomes at build time.
pub const BRUSH_CLASSES: &[&str] = &[
    CLASS_KILL_VOLUME,
    CLASS_HAZARD,
    CLASS_LADDER,
    CLASS_WORLD_MODEL,
    CLASS_BREAKABLE,
    CLASS_SKY_MODEL,
];

// Property names (beyond `name`, `pos`, `rot`, which stay by index: FMT_DAT85.md §6.1).
pub const PROP_TEAM: &str = "team";
pub const PROP_MODES: &str = "modes";
pub const PROP_EXTENT: &str = "extent";
pub const PROP_NUMBER: &str = "number";
pub const PROP_DEFUSE_TEAM: &str = "defuse_team";
pub const PROP_MODEL: &str = "model";
pub const PROP_PENDING_SEC: &str = "pending_sec";
pub const PROP_COLOR: &str = "color";
pub const PROP_BRIGHT: &str = "bright";
pub const PROP_RADIUS: &str = "radius";

/// The DeCombat mode names a `modes` filter or `mod_meta.modes` may carry (dc-config `modes.playable`; every
/// name also serves its `_CLAN` twin, MAPKIT_PLAN.md section 4).
pub const MODE_NAMES: &[&str] = &[
    "TDM",
    "TDM_CLAN",
    "COOP",
    "FFA",
    "ELIM",
    "ELIM_CLAN",
    "FLAG",
    "FLAG_CLAN",
    "BOMB",
    "BOMB_CLAN",
    "INFECT",
    "BOUNTY",
];

/// Team texts a spawn, zone or flag carries (`""` = any team).
pub const TEAM_1: &str = "1";
pub const TEAM_2: &str = "2";

/// The id prefix of an object of this class: `dc.player_spawn` -> `player_spawn` (ids are `<prefix>_<n>`).
pub fn id_prefix(class: &str) -> &str {
    class.strip_prefix("dc.").unwrap_or(class)
}

/// True for a class `add` accepts.
pub fn is_point_class(class: &str) -> bool {
    POINT_CLASSES.contains(&class)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_class_is_in_our_namespace_and_prefixes_strip_it() {
        for c in POINT_CLASSES.iter().chain(BRUSH_CLASSES) {
            assert!(c.starts_with("dc."), "{c}");
            assert!(!id_prefix(c).contains('.'));
        }
        assert_eq!(id_prefix(CLASS_PLAYER_SPAWN), "player_spawn");
        assert!(is_point_class(CLASS_BOMB_SITE));
        assert!(!is_point_class(CLASS_KILL_VOLUME));
    }
}
