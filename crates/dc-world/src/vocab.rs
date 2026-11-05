//! The DeCombat map vocabulary as data: every `dc.*` class a community map may place, its role, geometry and
//! properties (docs/MAPKIT_PLAN.md §4 with the 2026-11-01 review's fixes). dc-mapkit's validator, its schema
//! output and docs/MAPKIT_VOCAB.md are generated from this table; the readers match the names through
//! [`crate::names`].
// DC-DEFAULT: MAP-1 (the vocabulary), MAP-3 (brush classes link by model name; rot ignored)

use crate::names::{self, Key};
use crate::roles::{Geometry, ObjectRole};
use crate::PropKind;

/// One property of a class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropSpec {
    /// The property name as written into the file (a `Key::ours` of [`crate::names`]).
    pub name: &'static str,
    /// The property type code the file carries.
    pub kind: PropKind,
    /// Whether a valid map must write it (the three leading properties are always written).
    pub required: bool,
    /// What the readers assume when it is absent, as text; `None` = no default (required, or absent = unset).
    pub default: Option<&'static str>,
    /// One line for the human table.
    pub doc: &'static str,
}

/// One class of the vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassSpec {
    /// The `dc.*` class name.
    pub name: &'static str,
    /// The gameplay role the name resolves to; `None` for the classes the readers identify by their own key
    /// (lights, sound, sprite).
    pub role: Option<ObjectRole>,
    /// The key the class resolves through (`names::CLASS_*`; a role's class key).
    pub key: &'static Key,
    pub geometry: Geometry,
    /// Who consumes the class, for the human table.
    pub consumer: &'static str,
    /// The properties beyond the three leading ones ([`LEADING`]).
    pub props: &'static [PropSpec],
}

const fn p(
    name: &'static str,
    kind: PropKind,
    required: bool,
    default: Option<&'static str>,
    doc: &'static str,
) -> PropSpec {
    PropSpec {
        name,
        kind,
        required,
        default,
        doc,
    }
}

/// The three fixed leading properties of every object (by index 0 / 1 / 2 in the file; the names are aliases
/// for readers that look them up by name).
// SPEC: FMT_DAT85.md §6.1
pub const LEADING: [PropSpec; 3] = [
    p(
        "name",
        PropKind::Text,
        true,
        None,
        "object name; a brush class links to the BSP model of the same name (MAP-3)",
    ),
    p(
        "pos",
        PropKind::Vec3,
        true,
        None,
        "position (flags 0x00100000); a brush's model origin equals it",
    ),
    p(
        "rot",
        PropKind::Euler,
        true,
        None,
        "orientation, three radians; ignored on brush classes (MAP-3)",
    ),
];

const MODES: PropSpec = p(
    "modes",
    PropKind::Text,
    false,
    Some("\"\" (every mode)"),
    "comma list of DeCombat mode names, e.g. `TDM,FFA,ELIM`",
);
const EXTENT: PropSpec = p(
    "extent",
    PropKind::Vec3,
    true,
    None,
    "HALF extents of the box (flags 0x4)",
);
const TEAM_12: PropSpec = p("team", PropKind::Text, true, None, "`\"1\"` or `\"2\"`");

const PLAYER_SPAWN: [PropSpec; 2] = [
    p(
        "team",
        PropKind::Text,
        false,
        Some("\"\" (any team)"),
        "`\"1\"`, `\"2\"` or `\"\"`",
    ),
    MODES,
];
const ITEM_SPAWN: [PropSpec; 1] = [MODES];
const BOMB_SITE: [PropSpec; 4] = [
    EXTENT,
    p("number", PropKind::Int, true, None, "0 = A, 1 = B"),
    MODES,
    p(
        "defuse_team",
        PropKind::Text,
        false,
        Some("any team"),
        "the defending team, `\"1\"` or `\"2\"`",
    ),
];
const FLAG_ZONE: [PropSpec; 3] = [EXTENT, TEAM_12, MODES];
const CAPTURE_FLAG: [PropSpec; 2] = [
    TEAM_12,
    p(
        "model",
        PropKind::Text,
        false,
        Some("\"\" (the server's default flag look)"),
        "ROM LTB path of the ground model",
    ),
];
const KILL_VOLUME: [PropSpec; 3] = [
    p(
        "pending_sec",
        PropKind::Real,
        false,
        Some("0 (at once)"),
        "delay before the kill, seconds",
    ),
    p(
        "damage",
        PropKind::Real,
        false,
        None,
        "reserved: the server kills outright",
    ),
    p("damage_kind", PropKind::Text, false, None, "reserved"),
];
// DC-DEFAULT: DSV-607 (proposed) the hazard volume's damage per second and kind
const HAZARD: [PropSpec; 2] = [
    p(
        "damage",
        PropKind::Real,
        true,
        None,
        "damage per second to a player inside (> 0)",
    ),
    p(
        "damage_kind",
        PropKind::Text,
        false,
        Some("unspecified"),
        "`poison`, else the unspecified type",
    ),
];
const WORLD_MODEL: [PropSpec; 5] = [
    p("visible", PropKind::Bool, false, Some("true"), "drawn"),
    p(
        "solid",
        PropKind::Bool,
        false,
        Some("true"),
        "blocks movement",
    ),
    p("rayhit", PropKind::Bool, false, Some("true"), "stops shots"),
    p(
        "blend",
        PropKind::Text,
        false,
        Some("opaque"),
        "`opaque`, `translucent` or `additive`",
    ),
    p("alpha", PropKind::Real, false, Some("1"), "0..1"),
];
const BREAKABLE: [PropSpec; 1] = [p(
    "hit_points",
    PropKind::Real,
    true,
    None,
    "> 0, else the model never breaks",
)];
const SKY_MODEL: [PropSpec; 3] = [
    p(
        "sky_index",
        PropKind::Int,
        false,
        Some("0"),
        "sky table index",
    ),
    p(
        "sky_dims",
        PropKind::Vec3,
        false,
        Some("0 0 0"),
        "sky box dimensions",
    ),
    p(
        "sky_inner",
        PropKind::Vec3,
        false,
        Some("0 0 0"),
        "inner percentages; the readers serve it as the ROM's three reals",
    ),
];
const WORLD_SETTINGS: [PropSpec; 10] = [
    p("far_z", PropKind::Real, true, None, "far plane distance"),
    p("fog_enable", PropKind::Bool, false, Some("false"), "fog on"),
    p("fog_near", PropKind::Real, false, Some("0"), "fog start"),
    p("fog_far", PropKind::Real, false, Some("0"), "fog end"),
    p(
        "fog_color",
        PropKind::Rgb,
        false,
        Some("0 0 0"),
        "rgb 0..255",
    ),
    p(
        "sky_fog_enable",
        PropKind::Bool,
        false,
        Some("false"),
        "sky-pass fog on",
    ),
    p(
        "sky_fog_near",
        PropKind::Real,
        false,
        Some("0"),
        "sky-pass fog start",
    ),
    p(
        "sky_fog_far",
        PropKind::Real,
        false,
        Some("0"),
        "sky-pass fog end",
    ),
    p("wind", PropKind::Vec3, false, Some("0 0 0"), "wind vector"),
    p(
        "all_sky_portals",
        PropKind::Bool,
        false,
        Some("false"),
        "every sky portal open",
    ),
];
const GLOBAL_LIGHT: [PropSpec; 3] = [
    p(
        "color",
        PropKind::Rgb,
        false,
        Some("255 255 255"),
        "rgb 0..255",
    ),
    p("bright", PropKind::Real, false, Some("1"), "bright scale"),
    p(
        "to_ambient",
        PropKind::Real,
        false,
        Some("0"),
        "fraction converted to ambient",
    ),
];
const POINT_LIGHT: [PropSpec; 6] = [
    p(
        "color",
        PropKind::Rgb,
        false,
        Some("255 255 255"),
        "rgb 0..255",
    ),
    p(
        "radius",
        PropKind::Real,
        true,
        None,
        "> 0 (the reader drops a light without it)",
    ),
    p("bright", PropKind::Real, false, Some("1"), "bright scale"),
    p(
        "cone",
        PropKind::Real,
        false,
        Some("absent (point)"),
        "cone angle in degrees; 0 or absent = point light",
    ),
    p(
        "lights_objects",
        PropKind::Bool,
        true,
        Some("true (the tool writes it)"),
        "the reader returns no light unless this is true",
    ),
    p(
        "to_ambient",
        PropKind::Real,
        false,
        Some("0"),
        "fraction converted to ambient",
    ),
];
const SOUND: [PropSpec; 6] = [
    p(
        "file",
        PropKind::Text,
        true,
        None,
        "ROM WAV path or `@mod:<id>/assets/...`",
    ),
    p("volume", PropKind::Int, false, Some("100"), "percent"),
    p(
        "inner_radius",
        PropKind::Real,
        false,
        Some("= outer_radius"),
        "full volume inside",
    ),
    p(
        "outer_radius",
        PropKind::Real,
        false,
        Some("0"),
        "silent beyond",
    ),
    p("loop", PropKind::Bool, false, Some("false"), "looped"),
    p(
        "ambient",
        PropKind::Bool,
        false,
        Some("false"),
        "no direction",
    ),
];
const SPRITE: [PropSpec; 5] = [
    p(
        "file",
        PropKind::Text,
        true,
        None,
        "ROM SPR/DTX path or `@mod:<id>/assets/...`",
    ),
    p("scale_x", PropKind::Real, false, Some("1"), "width scale"),
    p("scale_y", PropKind::Real, false, Some("1"), "height scale"),
    p(
        "color",
        PropKind::Rgb,
        false,
        Some("255 255 255"),
        "rgb 0..255",
    ),
    p("alpha", PropKind::Real, false, Some("1"), "0..1"),
];

const fn role_class(
    role: ObjectRole,
    key: &'static Key,
    geometry: Geometry,
    consumer: &'static str,
    props: &'static [PropSpec],
) -> ClassSpec {
    ClassSpec {
        name: key.ours,
        role: Some(role),
        key,
        geometry,
        consumer,
        props,
    }
}

const fn own_class(
    key: &'static Key,
    consumer: &'static str,
    props: &'static [PropSpec],
) -> ClassSpec {
    ClassSpec {
        name: key.ours,
        role: None,
        key,
        geometry: Geometry::Point,
        consumer,
        props,
    }
}

/// Every class of the vocabulary, in the order of docs/MAPKIT_PLAN.md §4.
pub const CLASSES: [ClassSpec; 17] = [
    role_class(
        ObjectRole::PlayerSpawn,
        &names::CLASS_PLAYER_SPAWN,
        Geometry::Point,
        "dserver spawns (`ObjectRole::PlayerSpawn`)",
        &PLAYER_SPAWN,
    ),
    role_class(
        ObjectRole::ItemSpawn,
        &names::CLASS_ITEM_SPAWN,
        Geometry::Point,
        "dserver item spawns (`ObjectRole::ItemSpawn`)",
        &ITEM_SPAWN,
    ),
    role_class(
        ObjectRole::BombSite,
        &names::CLASS_BOMB_SITE,
        Geometry::Box,
        "dserver zones (`ObjectRole::BombSite`)",
        &BOMB_SITE,
    ),
    role_class(
        ObjectRole::FlagZone,
        &names::CLASS_FLAG_ZONE,
        Geometry::Box,
        "dserver zones (`ObjectRole::FlagZone`)",
        &FLAG_ZONE,
    ),
    role_class(
        ObjectRole::CaptureFlag,
        &names::CLASS_CAPTURE_FLAG,
        Geometry::Point,
        "dserver flags (`ObjectRole::CaptureFlag`)",
        &CAPTURE_FLAG,
    ),
    role_class(
        ObjectRole::KillVolume,
        &names::CLASS_KILL_VOLUME,
        Geometry::Brush,
        "dserver kill shapes (`ObjectRole::KillVolume`, brush)",
        &KILL_VOLUME,
    ),
    role_class(
        ObjectRole::HazardVolume,
        &names::CLASS_HAZARD,
        Geometry::Brush,
        "dserver hazards (`ObjectRole::HazardVolume`, brush; hurts a player inside every 500 ms)",
        &HAZARD,
    ),
    role_class(
        ObjectRole::Ladder,
        &names::CLASS_LADDER,
        Geometry::Brush,
        "dc-sim ladders (`ObjectRole::Ladder`, convex brush)",
        &[],
    ),
    role_class(
        ObjectRole::PlainWorldModel,
        &names::CLASS_WORLD_MODEL,
        Geometry::Brush,
        "dserver brushes, dc-render scene (`ObjectRole::PlainWorldModel`, brush)",
        &WORLD_MODEL,
    ),
    role_class(
        ObjectRole::Breakable,
        &names::CLASS_BREAKABLE,
        Geometry::Brush,
        "dserver destructibles (`ObjectRole::Breakable`, brush)",
        &BREAKABLE,
    ),
    role_class(
        ObjectRole::SkyWorldModel,
        &names::CLASS_SKY_MODEL,
        Geometry::Brush,
        "dserver sky brush, dc-render sky pass (`ObjectRole::SkyWorldModel`, brush)",
        &SKY_MODEL,
    ),
    role_class(
        ObjectRole::WorldSettings,
        &names::CLASS_WORLD_SETTINGS,
        Geometry::Point,
        "dserver evars, dc-render fog (`ObjectRole::WorldSettings`)",
        &WORLD_SETTINGS,
    ),
    own_class(
        &names::CLASS_GLOBAL_LIGHT,
        "dc-render `global_light`, dserver `worldlight`; direction = `rot`",
        &GLOBAL_LIGHT,
    ),
    own_class(
        &names::CLASS_POINT_LIGHT,
        "dc-render `world_lights` (characters and held weapons only; the world's light is the baked vertex colour)",
        &POINT_LIGHT,
    ),
    own_class(
        &names::CLASS_END_CAMERA,
        "dserver camera shot at the round result; place = `pos`, `rot` x looks down, `rot` y turns (MAP-101)",
        &[],
    ),
    own_class(
        &names::CLASS_SOUND,
        "dserver world sounds",
        &SOUND,
    ),
    own_class(
        &names::CLASS_SPRITE,
        "dserver scale sprites",
        &SPRITE,
    ),
];

impl ClassSpec {
    /// The key the class resolves through (`names::CLASS_*`).
    pub fn key(&self) -> &'static Key {
        self.key
    }

    /// Whether `class_name` (bytes from a file) is this class ([`Key::matches`]).
    pub fn matches(&self, class_name: &[u8]) -> bool {
        self.key().matches(class_name)
    }

    /// The property of the class named `name` (ASCII case ignored), leading properties included.
    pub fn prop(&self, name: &str) -> Option<&'static PropSpec> {
        LEADING
            .iter()
            .chain(self.props.iter())
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// The class spec named `name` (ASCII case ignored).
pub fn class_named(name: &str) -> Option<&'static ClassSpec> {
    CLASSES.iter().find(|c| c.name.eq_ignore_ascii_case(name))
}

/// The class spec of an object's class name as read from a file (hash or `dc.*` name).
pub fn class_of(class_name: &[u8]) -> Option<&'static ClassSpec> {
    CLASSES.iter().find(|c| c.matches(class_name))
}

fn kind_text(k: PropKind) -> &'static str {
    match k {
        PropKind::Text => "text",
        PropKind::Vec3 => "vec3",
        PropKind::Rgb => "rgb",
        PropKind::Real => "real",
        PropKind::Bits => "bits",
        PropKind::Bool => "bool",
        PropKind::Int => "int",
        PropKind::Euler => "euler",
        PropKind::Other(_) => "other",
    }
}

fn geometry_text(g: Geometry) -> &'static str {
    match g {
        Geometry::Point => "point",
        Geometry::Box => "box",
        Geometry::Brush => "brush",
    }
}

/// The human tables of docs/MAPKIT_VOCAB.md, generated from [`LEADING`] and [`CLASSES`] (the doc's test
/// asserts the file contains this text verbatim; regenerate with `cargo run -p dc-world --example vocab_md`).
pub fn markdown() -> String {
    use std::fmt::Write;
    let mut s = String::new();
    s.push_str("### Leading properties (every class, by index)\n\n");
    s.push_str("| index | name | type | meaning |\n|---|---|---|---|\n");
    for (i, p) in LEADING.iter().enumerate() {
        let _ = writeln!(
            s,
            "| {i} | `{}` | {} | {} |",
            p.name,
            kind_text(p.kind),
            p.doc
        );
    }
    s.push_str("\n### Classes\n\n");
    for c in CLASSES.iter() {
        let _ = writeln!(
            s,
            "#### `{}`\n\nGeometry: {}. Role: {}. Consumer: {}.\n",
            c.name,
            geometry_text(c.geometry),
            c.role
                .map_or("none (own class key)".to_string(), |r| format!("`{r:?}`")),
            c.consumer
        );
        if c.props.is_empty() {
            s.push_str("No properties beyond the leading three.\n\n");
            continue;
        }
        s.push_str("| property | type | required | default | meaning |\n|---|---|---|---|---|\n");
        for p in c.props {
            let _ = writeln!(
                s,
                "| `{}` | {} | {} | {} | {} |",
                p.name,
                kind_text(p.kind),
                if p.required { "yes" } else { "no" },
                p.default.unwrap_or("-"),
                p.doc
            );
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_class_resolves_to_its_role_and_key() {
        for c in CLASSES.iter() {
            assert_eq!(
                ObjectRole::of_class(c.name.as_bytes()),
                c.role,
                "{}",
                c.name
            );
            assert!(c.matches(c.name.as_bytes()), "{}", c.name);
            assert!(
                c.matches(c.name.to_ascii_uppercase().as_bytes()),
                "{}",
                c.name
            );
            if let Some(r) = c.role {
                assert_eq!(r.dc_name(), Some(c.name));
                assert_eq!(r.geometry(), c.geometry, "{}", c.name);
                assert_eq!(r.class_key(), c.key(), "{}", c.name);
            }
            assert_eq!(c.key().ours, c.name);
            assert_eq!(class_of(c.name.as_bytes()).map(|x| x.name), Some(c.name));
            assert_eq!(
                class_named(&c.name.to_ascii_uppercase()).map(|x| x.name),
                Some(c.name)
            );
        }
        // every role the vocabulary offers has exactly one class
        for r in ObjectRole::ALL {
            let n = CLASSES.iter().filter(|c| c.role == Some(r)).count();
            assert_eq!(n, usize::from(r.dc_name().is_some()), "{r:?}");
        }
    }

    #[test]
    fn every_prop_name_is_a_key() {
        for p in LEADING.iter() {
            assert!(names::keys_named(p.name).next().is_some(), "{}", p.name);
        }
        for c in CLASSES.iter() {
            let mut seen = std::collections::HashSet::new();
            for p in c.props {
                assert!(
                    names::keys_named(p.name).next().is_some(),
                    "{}.{}",
                    c.name,
                    p.name
                );
                assert!(seen.insert(p.name), "{}.{} twice", c.name, p.name);
                assert!(c.prop(p.name).is_some());
                assert!(
                    !LEADING.iter().any(|l| l.name == p.name),
                    "{}.{}",
                    c.name,
                    p.name
                );
                assert!(
                    p.required || p.default.is_some() || p.doc.contains("reserved"),
                    "{}.{}",
                    c.name,
                    p.name
                );
            }
            assert!(c.prop("NAME").is_some());
        }
    }

    #[test]
    fn review_fixes_are_in() {
        assert!(class_named("dc.ladder").unwrap().props.is_empty());
        let bomb = class_named("dc.bomb_site").unwrap();
        assert!(!bomb.prop("defuse_team").unwrap().required);
        let light = class_named("dc.point_light").unwrap();
        assert!(light.prop("radius").unwrap().required);
        assert!(light.prop("lights_objects").unwrap().required);
        let kill = class_named("dc.kill_volume").unwrap();
        assert!(kill.prop("damage").unwrap().doc.contains("reserved"));
        assert!(LEADING[2].doc.contains("ignored on brush"));
        let hazard = class_named("dc.hazard").unwrap();
        assert!(hazard.prop("damage").unwrap().required);
        assert!(hazard.prop("pending_sec").is_none());
    }

    #[test]
    fn markdown_mentions_everything() {
        let md = markdown();
        for c in CLASSES.iter() {
            assert!(md.contains(&format!("`{}`", c.name)));
            for p in c.props {
                assert!(md.contains(&format!("`{}`", p.name)));
            }
        }
    }
}
