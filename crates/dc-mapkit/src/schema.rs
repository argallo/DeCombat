//! The project / ops reference `dc-mapkit schema` prints: hand-maintained field tables (name, type, doc) for
//! every struct of [`crate::project`] and every [`crate::ops::Op`] variant. A test serialises a default of every
//! struct and an example of every op and asserts the key sets are exactly the documented ones, so the tables
//! cannot drift from serde (MAPKIT_PLAN.md 5.1 "kept in step with the types by a test").

use crate::diag::{Code, EXIT_DIAGNOSTICS, EXIT_OK, EXIT_USAGE};
use crate::ops::Op;
use crate::vocab_names as v;
use serde_json::{json, Value};

/// (field, type, doc)
pub type Fields = &'static [(&'static str, &'static str, &'static str)];

/// (struct name, doc, fields)
pub const STRUCTS: &[(&str, &str, Fields)] = &[
    (
        "Project",
        "map.json: the whole map. Lists keep their order; every placed thing has a stable id.",
        &[
            ("format", "int", "format version, always 1"),
            ("name", "text", "the map's label in the room list"),
            ("key", "text", "the map key: 1..32 of a-z 0-9 _; names the .dat and the mod id"),
            ("seed", "int", "the seed terrain.gen uses when an op gives none"),
            ("units", "text", "a note on the units (world units; a standing player is 100 tall)"),
            ("next_id", "int", "the next id number; ids are <prefix>_<n>; only grows"),
            ("terrain", "Terrain|null", "the heightmap terrain, or null for none"),
            ("materials", "[Material]", "surfaces: texture, tile size, footstep surface id, flags"),
            ("solids", "[Solid]", "convex solids: level geometry or brushes"),
            ("prefabs", "[PrefabInstance]", "placed prefabs (named sets of solids, expanded at build)"),
            ("imports", "[Import]", "placed OBJ meshes"),
            ("objects", "[MapObject]", "point objects in the dc.* vocabulary (spawns, sites, zones, lights)"),
            ("settings", "WorldSettings", "fog, far plane, sun, ambient, sky box"),
            ("mod_meta", "ModMeta", "what export writes into the mod manifest and the map record"),
            ("build", "BuildSettings", "BSP cell size and the vertex-light bake"),
        ],
    ),
    (
        "Terrain",
        "A grid of (cells_x + 1) x (cells_z + 1) u16 heights in `file`; world y = origin[1] + raw / 65535 * height_scale.",
        &[
            ("cells_x", "int", "cells along x (1..1024)"),
            ("cells_z", "int", "cells along z (1..1024)"),
            ("cell_size", "num", "world units per cell"),
            ("origin", "vec3", "world position of vertex (0, 0); [1] is the height of raw 0"),
            ("height_scale", "num", "world height spanned by the raw range 0..65535"),
            ("file", "path", "the heights file, little-endian u16 row-major (row = z), default terrain.r16"),
            ("layers", "[TerrainLayer]", "paint layers; layer 0 is the default surface"),
            ("paint", "path|null", "optional one byte per cell naming the layer (reserved for the GUI)"),
        ],
    ),
    ("TerrainLayer", "One paint layer.", &[("material", "text", "material id")]),
    (
        "Material",
        "A surface. Referenced by id from solids, faces, layers and imports.",
        &[
            ("id", "text", "letters, digits and _"),
            ("texture", "{\"own\": path} | {\"rom\": path}", "own PNG under assets/ (converted to DTX at build) or a ROM archive path (never committed)"),
            ("tile", "num", "world units per texture repeat"),
            ("surface_id", "int", "footstep surface id 0..255 (a ROM surface-table row; 0 = default)"),
            ("flags", "MaterialFlags", "solid / no_step / sky / skin (with hidden: no shadow, for bullet marks over paint) / hidden (collision only, never drawn)"),
            ("fit", "[u0, v0, u1, v1]", "optional: fit this part of the texture once across each face of a solid (an atlas, a whole container side), instead of tiling; terrain and imports ignore it"),
        ],
    ),
    (
        "MaterialFlags",
        "Surface flags written into the polygon.",
        &[
            ("solid", "bool", "blocks movement and shots (0x1)"),
            ("no_step", "bool", "no footstep sounds (0x400000)"),
            ("sky", "bool", "a sky face: solid, drawn as sky through a portal (0x10)"),
        ],
    ),
    (
        "Solid",
        "A convex solid: pos is its centre, size its full extents, rot_y_deg its yaw. Without `brush` it is level geometry; with one it becomes a brush model plus an object of that class named by the id.",
        &[
            ("id", "text", "solid_<n>"),
            ("kind", "\"box\" | \"wedge\" | {\"cylinder\": {sides, circle}} | {\"prism\": {points}}", "the shape; a cylinder stores its unit circle, a prism a convex x/z outline in local units"),
            ("pos", "vec3", "centre"),
            ("rot_y_deg", "num", "yaw in degrees"),
            ("size", "vec3", "full extents (a prism uses only [1], its height)"),
            ("material", "text", "material id of every face without an override"),
            ("faces", "{face: material id}", "per-face override: top, bottom, north (+z), south (-z), east (+x), west (-x), back, slope, side_<i> or side"),
            ("group", "text|null", "a free grouping name (prefab source, GUI selection)"),
            ("brush", "\"world_model\" | \"kill_volume\" | \"ladder\" | \"breakable\" | \"sky\" | null", "the brush role, or null for level geometry"),
        ],
    ),
    (
        "PrefabInstance",
        "A placed prefab.",
        &[
            ("id", "text", "prefab_<n>"),
            ("prefab", "text", "prefab name (the library comes with the next stage)"),
            ("pos", "vec3", "position"),
            ("rot_y_deg", "num", "yaw in degrees"),
            ("scale", "num", "uniform scale"),
        ],
    ),
    (
        "Import",
        "A placed OBJ mesh (v / vt / vn / f; no materials).",
        &[
            ("id", "text", "import_<n>"),
            ("file", "path", "the OBJ under the project, usually assets/"),
            ("pos", "vec3", "position"),
            ("rot_y_deg", "num", "yaw in degrees"),
            ("scale", "num", "uniform scale"),
            ("material", "text", "material id of every triangle"),
            ("collision", "bool", "true = solid level geometry, false = drawn only"),
        ],
    ),
    (
        "MapObject",
        "A point object. Values: text, number, bool, [x, y, z] or {\"rgb\": [r, g, b]}.",
        &[
            ("id", "text", "<class without dc.>_<n>"),
            ("class", "text", "a dc.* class (see classes)"),
            ("name", "text", "object name (defaults to the id)"),
            ("pos", "vec3", "position"),
            ("rot", "vec3", "euler degrees (pitch, yaw, roll); a spawn's yaw is where the player looks"),
            ("props", "{name: value}", "the class's properties (team, modes, extent, number, ...)"),
        ],
    ),
    (
        "WorldSettings",
        "Fog, far plane, sun, ambient and the sky box; the build emits dc.world_settings, dc.global_light and dc.sky_model from these.",
        &[
            ("fog_enable", "bool", "distance fog on"),
            ("fog_near", "num", "fog start"),
            ("fog_far", "num", "fog end (full)"),
            ("fog_color", "rgb", "fog colour"),
            ("far_z", "num", "far clip plane (at least the map diagonal)"),
            ("sun_dir", "vec3", "direction the sunlight travels (normalised at build)"),
            ("sun_color", "rgb", "sun colour"),
            ("sun_bright", "num", "sun intensity (1 = full)"),
            ("ambient", "rgb", "ambient light"),
            ("sky_size", "vec3", "sky box dimensions"),
            ("sky_material", "text", "material of the sky box's inside faces"),
        ],
    ),
    (
        "ModMeta",
        "Mod manifest and map record fields (MODDING_GUIDE.md section 3; MAPKIT_PLAN.md 5.4).",
        &[
            ("id", "text", "mod id (letters, digits, _ -)"),
            ("version", "text", "major.minor.patch"),
            ("author", "text", "1..100 characters"),
            ("description", "text", "20..500 characters"),
            ("tags", "[text]", "up to 8 browser tags of a-z 0-9 -"),
            ("homepage", "text|null", "http(s) page"),
            ("modes", "[text]", "DeCombat mode names the map plays (TDM, FFA, ELIM, BOMB, FLAG, ...)"),
            ("goals", "{mode: int}", "optional default goal per mode"),
            ("round_seconds", "int|null", "optional round clock"),
            ("players", "{min, max}|null", "optional player range"),
        ],
    ),
    ("Players", "Player range.", &[("min", "int", "minimum"), ("max", "int", "maximum (the lobby caps rooms at 16)")]),
    (
        "BuildSettings",
        "Build parameters.",
        &[("cell_polys", "int", "polygons per BSP cell (kd split threshold)"), ("bake", "Bake", "vertex-light bake")],
    ),
    (
        "Bake",
        "Vertex colour bake weights (0..1).",
        &[
            ("ambient", "num", "ambient term"),
            ("sun", "num", "sun-by-normal term"),
            ("hemisphere", "num", "sky-up / ground-down term"),
            ("ao", "num", "terrain ambient occlusion from height differences"),
        ],
    ),
];

/// (op name, doc, fields)
pub const OPS: &[(&str, &str, Fields)] = &[
    (
        "add",
        "Add a point object; returns its new id.",
        &[
            ("class", "text", "a dc.* point class"),
            ("name", "text|null", "object name (default: the id)"),
            ("pos", "vec3", "position"),
            ("rot", "vec3|null", "euler degrees (default [0, 0, 0])"),
            ("props", "{name: value}", "properties (default none)"),
            ("ground", "bool", "snap y to the floor plus the standing offset (default false)"),
        ],
    ),
    ("remove", "Remove any placed thing by id.", &[("id", "text", "the id")]),
    (
        "clear",
        "Remove placed things in bulk so a generator can run again. With no field: every object, solid, prefab and import (materials, terrain, settings and metadata stay, ids start over). With a field: only that.",
        &[
            ("group", "text|null", "only the solids of this group"),
            ("class", "text|null", "only the objects of this class"),
        ],
    ),
    (
        "move",
        "Move any placed thing: give `pos`, `delta` or `ground: true`.",
        &[
            ("id", "text", "the id"),
            ("pos", "vec3|null", "new position"),
            ("delta", "vec3|null", "offset to add"),
            ("ground", "bool", "snap y to the floor afterwards (default false)"),
        ],
    ),
    (
        "set",
        "Set an object property (`name`, `pos`, `rot` or any class property).",
        &[("id", "text", "object id"), ("prop", "text", "property name"), ("value", "value", "text, number, bool, vec3 or {\"rgb\": [...]}")],
    ),
    ("rename", "Rename an object.", &[("id", "text", "object id"), ("name", "text", "new name")]),
    (
        "solid.add",
        "Add a convex solid; returns its new id.",
        &[
            ("kind", "\"box\" | \"wedge\" | {\"cylinder\": {\"sides\": n}} | {\"prism\": {\"points\": [[x, z], ...]}}", "the shape (a cylinder's circle is computed here)"),
            ("pos", "vec3", "centre"),
            ("rot_y_deg", "num", "yaw (default 0)"),
            ("size", "vec3", "full extents"),
            ("material", "text", "material id (must exist)"),
            ("faces", "{face: material}", "per-face overrides (default none)"),
            ("group", "text|null", "grouping name"),
            ("brush", "role|null", "world_model, kill_volume, ladder, breakable, sky, or null"),
            ("ground", "bool", "rest the bottom on the floor (default false)"),
            ("rises", "\"north\" | \"east\" | \"south\" | \"west\" | null", "a wedge only: which way it climbs (north is +z, east is +x), instead of rot_y_deg; size stays [width across, height, run along the slope]"),
        ],
    ),
    (
        "solid.set",
        "Change fields of a solid (every field optional).",
        &[
            ("id", "text", "solid id"),
            ("pos", "vec3|null", "centre"),
            ("rot_y_deg", "num|null", "yaw"),
            ("size", "vec3|null", "full extents"),
            ("material", "text|null", "material id"),
            ("faces", "{face: material}|null", "replaces the overrides"),
            ("group", "text|null", "grouping name"),
            ("brush", "role|null", "brush role"),
            ("rises", "\"north\" | \"east\" | \"south\" | \"west\" | null", "a wedge only: which way it climbs, instead of rot_y_deg"),
        ],
    ),
    (
        "terrain.gen",
        "Generate the heightmap from seeded fBm noise; creates the terrain when the project has none (or when cells are given).",
        &[
            ("preset", "\"hills\" | \"valley\" | \"ridge\" | \"flat\"", "hills, hills with a lowered centre band along z, a raised one, or one level floor at base"),
            ("seed", "int|null", "noise seed (default: the project seed)"),
            ("amplitude", "num", "height range in world units (default 256); 0 gives a level floor at base"),
            ("base", "num", "mean height (default 0)"),
            ("scale", "num", "feature size in world units (default 1024)"),
            ("octaves", "int", "noise layers 1..12 (default 5)"),
            ("cells_x", "int|null", "new terrain: cells along x (default 64)"),
            ("cells_z", "int|null", "new terrain: cells along z (default 64)"),
            ("cell_size", "num|null", "new terrain: cell size (default 64)"),
        ],
    ),
    (
        "terrain.raise",
        "Raise the heights inside a circle.",
        &[("x", "num", "centre x"), ("z", "num", "centre z"), ("radius", "num", "radius"), ("amount", "num", "world units at the centre"), ("falloff", "\"smooth\" | \"linear\" | \"flat\"", "profile (default smooth)")],
    ),
    (
        "terrain.lower",
        "Lower the heights inside a circle.",
        &[("x", "num", "centre x"), ("z", "num", "centre z"), ("radius", "num", "radius"), ("amount", "num", "world units at the centre"), ("falloff", "\"smooth\" | \"linear\" | \"flat\"", "profile (default smooth)")],
    ),
    (
        "terrain.smooth",
        "Blur the heights (3 x 3 box), inside a circle or everywhere.",
        &[("x", "num|null", "centre x (omit all three for the whole terrain)"), ("z", "num|null", "centre z"), ("radius", "num|null", "radius"), ("passes", "int", "1..64 (default 1)")],
    ),
    (
        "terrain.flatten",
        "Pull the heights inside a circle towards one height.",
        &[("x", "num", "centre x"), ("z", "num", "centre z"), ("radius", "num", "radius"), ("height", "num", "target world y"), ("falloff", "\"smooth\" | \"linear\" | \"flat\"", "profile (default smooth)")],
    ),
    (
        "material.set",
        "Create or change a material (every field but id optional).",
        &[
            ("id", "text", "material id"),
            ("texture", "{\"own\": path} | {\"rom\": path} | null", "texture source"),
            ("tile", "num|null", "world units per repeat"),
            ("surface_id", "int|null", "footstep surface id"),
            ("flags", "MaterialFlags|null", "solid / no_step / sky / skin (with hidden: no shadow, for bullet marks over paint) / hidden (collision only, never drawn)"),
            ("fit", "[u0, v0, u1, v1]|null", "fit this part of the texture (0..1, v down) once across each face of a solid, instead of tiling it"),
        ],
    ),
    (
        "import.add",
        "Place an OBJ mesh; returns its new id.",
        &[
            ("file", "path", "OBJ under the project"),
            ("pos", "vec3", "position"),
            ("rot_y_deg", "num", "yaw (default 0)"),
            ("scale", "num", "uniform scale (default 1)"),
            ("material", "text", "material id (must exist)"),
            ("collision", "bool", "solid (default true)"),
            ("ground", "bool", "snap y to the floor (default false)"),
        ],
    ),
    (
        "asset.place",
        "Place a model the `asset` verb converted; returns an import id per part. It stops players and bullets through a hidden core unless `solid` is false.",
        &[
            ("asset", "text", "the name it was converted under"),
            ("pos", "vec3", "where its base centre goes"),
            ("rot_y_deg", "num", "yaw (default 0)"),
            ("scale", "num", "units per metre (default 1)"),
            ("ground", "bool", "snap y to the floor (default false)"),
            ("solid", "bool", "a hidden core around it stops players and bullets (default true)"),
            ("surface", "int", "the core's surface id: impact marks and footsteps (default 0)"),
        ],
    ),
    (
        "prefab.add",
        "Place a prefab; returns its new id.",
        &[
            ("prefab", "text", "prefab name"),
            ("pos", "vec3", "position"),
            ("rot_y_deg", "num", "yaw (default 0)"),
            ("scale", "num", "uniform scale (default 1)"),
            ("ground", "bool", "snap y to the floor (default false)"),
        ],
    ),
    (
        "settings.set",
        "Change the world settings (fog, far plane, sun, ambient, sky); every field optional, only the given ones change.",
        &[
            ("bake_shadow", "num|null", "shadows on floors and walls from solids and hidden cores, 0 (off) .. 1"),
            ("bake_lightmap", "num|null", "world units a lightmap texel covers, 2 .. 64: lightmaps, lamps (dc.point_light) and bounced light; 0 is off"),
            ("fog_enable", "bool|null", "fog on"),
            ("fog_near", "num|null", "fog start"),
            ("fog_far", "num|null", "fog end"),
            ("fog_color", "rgb|null", "fog colour 0..255"),
            ("far_z", "num|null", "far plane, > 0"),
            ("sun_dir", "vec3|null", "direction the sun shines along (not the zero vector)"),
            ("sun_color", "rgb|null", "sun colour 0..255"),
            ("sun_bright", "num|null", "sun brightness scale"),
            ("ambient", "rgb|null", "ambient colour 0..255 (the baked base light)"),
            ("sky_size", "vec3|null", "sky box size, > 0 on every axis"),
            ("sky_material", "text|null", "material id of the sky box faces (must exist)"),
        ],
    ),
    (
        "meta.set",
        "Change the map's label and the mod metadata export writes; every field optional.",
        &[
            ("name", "text|null", "the map's label (not empty)"),
            ("version", "text|null", "mod version"),
            ("author", "text|null", "author"),
            ("description", "text|null", "description"),
            ("tags", "[text]|null", "tags"),
            ("homepage", "text|null", "homepage url; \"\" clears it"),
            ("modes", "[text]|null", "the DeCombat modes the map serves (upper-cased)"),
            ("goals", "{mode: int}|null", "starting goal per mode"),
            ("round_seconds", "int|null", "round clock; 0 clears it"),
            ("players", "Players|null", "{min, max}, 1 <= min <= max <= 16"),
        ],
    ),
];

/// (class, properties doc)
pub const CLASSES: &[(&str, &str)] = &[
    (v::CLASS_PLAYER_SPAWN, "team (\"1\", \"2\", \"\" any), modes (comma list of mode names; empty = every mode)"),
    (v::CLASS_ITEM_SPAWN, "modes"),
    (v::CLASS_BOMB_SITE, "extent (vec3 HALF extents), number (0 = A, 1 = B), modes, defuse_team (optional \"1\" / \"2\")"),
    (v::CLASS_FLAG_ZONE, "extent, team, modes"),
    (v::CLASS_CAPTURE_FLAG, "team, model (text; empty = default look)"),
    (v::CLASS_WORLD_SETTINGS, "far_z, fog_enable, fog_near, fog_far, fog_color, sky_fog_enable, sky_fog_near, sky_fog_far, wind, all_sky_portals (the build fills these from settings)"),
    (v::CLASS_GLOBAL_LIGHT, "color, bright, to_ambient; direction = rot (the build fills these from settings)"),
    (v::CLASS_POINT_LIGHT, "color, radius (> 0), bright, cone, lights_objects (characters only)"),
    ("dc.end_camera", "the overview camera at the round result: pos is the place, rot x looks down, rot y turns; without one, build adds an overview"),
    (v::CLASS_SOUND, "file, volume, inner_radius, outer_radius, loop, ambient"),
    (v::CLASS_SPRITE, "file, scale_x, scale_y, color, alpha"),
    (v::CLASS_KILL_VOLUME, "brush (solid with brush: kill_volume): pending_sec"),
    (v::CLASS_HAZARD, "brush: as kill volume (no server behaviour today)"),
    (v::CLASS_LADDER, "brush (solid with brush: ladder): none"),
    (v::CLASS_WORLD_MODEL, "brush (solid with brush: world_model): visible, solid, rayhit, blend, alpha"),
    (v::CLASS_BREAKABLE, "brush (solid with brush: breakable): hit_points"),
    (v::CLASS_SKY_MODEL, "brush (solid with brush: sky): sky_index, sky_dims, sky_inner"),
];

fn fields_json(fields: Fields) -> Value {
    Value::Array(
        fields
            .iter()
            .map(|(name, ty, doc)| json!({"name": name, "type": ty, "doc": doc}))
            .collect(),
    )
}

/// The schema document. `ops_only` leaves out the project structs.
pub fn schema(ops_only: bool) -> Value {
    let ops: Vec<Value> = OPS
        .iter()
        .map(|(name, doc, fields)| json!({"op": name, "doc": doc, "fields": fields_json(fields)}))
        .collect();
    let mut out = serde_json::Map::new();
    out.insert("format".into(), json!(crate::project::FORMAT));
    out.insert("tool_version".into(), json!(crate::TOOL_VERSION));
    out.insert(
        "ops".into(),
        json!({
            "doc": "apply takes a JSON array of ops, or {\"ops\": [...]}; each op is {\"op\": \"<name>\", ...fields}; the batch is all-or-nothing and the result is {applied: [{op, ids}], diagnostics, changed}",
            "list": ops,
        }),
    );
    if !ops_only {
        let structs: Vec<Value> = STRUCTS
            .iter()
            .map(|(name, doc, fields)| json!({"struct": name, "doc": doc, "fields": fields_json(fields)}))
            .collect();
        out.insert(
            "project".into(),
            json!({
                "doc": "map.json (format 1); paths are forward-slash and relative to the project; terrain.r16 holds the heights",
                "units": crate::project::UNITS_DOC,
                "structs": structs,
            }),
        );
        out.insert(
            "prop_value".into(),
            json!("an object property value is JSON text, a number, a bool, a 3-number array (vec3) or {\"rgb\": [r, g, b]}"),
        );
        out.insert(
            "classes".into(),
            Value::Array(
                CLASSES
                    .iter()
                    .map(|(c, d)| json!({"class": c, "props": d}))
                    .collect(),
            ),
        );
        out.insert("modes".into(), json!(v::MODE_NAMES));
        out.insert(
            "files".into(),
            json!({
                "map.json": "the project",
                "terrain.r16": "little-endian u16 heights, row-major (row = z), (cells_x + 1) * (cells_z + 1) entries",
                "assets/": "own textures (PNG), OBJ meshes, sounds",
                "build/": "outputs (gitignored)",
                ".mapkit/history/<n>/": "snapshots of map.json + terrain.r16 (keep 50)",
            }),
        );
    }
    out.insert(
        "codes".into(),
        Value::Array(
            Code::ALL
                .iter()
                .map(|c| json!({"code": c.as_str(), "severity": c.default_severity(), "doc": c.doc()}))
                .collect(),
        ),
    );
    out.insert(
        "exit_codes".into(),
        json!({"ok": EXIT_OK, "diagnostics": EXIT_DIAGNOSTICS, "usage": EXIT_USAGE}),
    );
    Value::Object(out)
}

/// Names of every struct the tables document (the test walks it against the defaults).
pub fn documented_structs() -> Vec<&'static str> {
    STRUCTS.iter().map(|(n, _, _)| *n).collect()
}

/// The documented op names, in table order.
pub fn documented_ops() -> Vec<&'static str> {
    OPS.iter().map(|(n, _, _)| *n).collect()
}

/// The documented fields of an op.
pub fn op_fields(name: &str) -> Option<Fields> {
    OPS.iter().find(|(n, _, _)| *n == name).map(|(_, _, f)| *f)
}

/// Keys of a serialised op minus the tag (helper for the coverage test and the MCP tool generator).
pub fn op_keys(op: &Op) -> Vec<String> {
    let v = serde_json::to_value(op).expect("op serialises");
    v.as_object()
        .map(|m| m.keys().filter(|k| *k != "op").cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::*;
    use std::collections::BTreeSet;

    fn keys<T: serde::Serialize>(t: &T) -> BTreeSet<String> {
        serde_json::to_value(t)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    fn documented(name: &str) -> BTreeSet<String> {
        STRUCTS
            .iter()
            .find(|(n, _, _)| *n == name)
            .unwrap_or_else(|| panic!("{name} is not documented"))
            .2
            .iter()
            .map(|(f, _, _)| f.to_string())
            .collect()
    }

    #[test]
    fn every_project_struct_key_is_documented_and_vice_versa() {
        let checks: Vec<(&str, BTreeSet<String>)> = vec![
            ("Project", keys(&Project::default())),
            ("Terrain", keys(&Terrain::default())),
            ("TerrainLayer", keys(&TerrainLayer::default())),
            (
                "Material",
                // `fit` is left out of the file when it is not set
                keys(&Material {
                    fit: Some([0.0, 0.0, 1.0, 1.0]),
                    ..Default::default()
                }),
            ),
            ("MaterialFlags", keys(&MaterialFlags::default())),
            ("Solid", keys(&Solid::default())),
            ("PrefabInstance", keys(&PrefabInstance::default())),
            ("Import", keys(&Import::default())),
            ("MapObject", keys(&MapObject::default())),
            ("WorldSettings", keys(&WorldSettings::default())),
            ("ModMeta", keys(&ModMeta::default())),
            ("Players", keys(&Players::default())),
            ("BuildSettings", keys(&BuildSettings::default())),
            ("Bake", keys(&Bake::default())),
        ];
        let mut seen = BTreeSet::new();
        for (name, actual) in checks {
            assert_eq!(actual, documented(name), "{name}");
            seen.insert(name);
        }
        for n in documented_structs() {
            assert!(seen.contains(n), "{n} documented but not checked");
        }
    }

    #[test]
    fn every_op_variant_and_field_is_documented() {
        let examples = crate::ops::example_ops();
        let mut seen = BTreeSet::new();
        for op in &examples {
            let fields =
                op_fields(op.name()).unwrap_or_else(|| panic!("{} is not documented", op.name()));
            let documented: BTreeSet<String> =
                fields.iter().map(|(f, _, _)| f.to_string()).collect();
            let actual: BTreeSet<String> = op_keys(op).into_iter().collect();
            assert_eq!(actual, documented, "{}", op.name());
            seen.insert(op.name());
        }
        assert_eq!(documented_ops(), Op::NAMES);
        for n in documented_ops() {
            assert!(seen.contains(n), "{n} has no example");
        }
        for (c, _) in CLASSES {
            assert!(
                v::POINT_CLASSES.contains(c) || v::BRUSH_CLASSES.contains(c),
                "{c}"
            );
        }
        for c in v::POINT_CLASSES.iter().chain(v::BRUSH_CLASSES) {
            assert!(CLASSES.iter().any(|(k, _)| k == c), "{c} is not documented");
        }
    }

    #[test]
    fn the_document_has_the_sections_and_the_ops_only_form_is_smaller() {
        let full = schema(false);
        for k in [
            "format",
            "ops",
            "project",
            "prop_value",
            "classes",
            "modes",
            "files",
            "codes",
            "exit_codes",
        ] {
            assert!(full.get(k).is_some(), "{k}");
        }
        assert_eq!(full["codes"].as_array().unwrap().len(), Code::ALL.len());
        let ops = schema(true);
        assert!(ops.get("project").is_none());
        assert_eq!(ops["ops"]["list"].as_array().unwrap().len(), OPS.len());
    }
}
