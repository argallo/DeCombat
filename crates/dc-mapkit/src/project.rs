//! The project format (`map.json`, format 1) and the heightmap file (`terrain.r16`).
//!
//! MAPKIT_PLAN.md 5.1 / D3: the project is the source of truth; `.dat` is a build output. Every map is a
//! `BTreeMap` and every list keeps its written order, so a saved project is byte-stable (DC-DEFAULT: MAP-10).
//!
//! Units: world units everywhere (a standing player is 100 units tall, the templates are 4096 x 4096). A solid's
//! `pos` is its CENTRE and `size` its full extents; an object's `pos` is its point. Rotations are degrees.
//!
//! Paths (`terrain.file`, `terrain.paint`, own textures, imports) are forward-slash and relative to the project
//! folder: [`check_rel_path`] refuses a backslash, a drive letter, a leading slash and any `..` segment.
//!
//! Ids are `<prefix>_<n>` (`player_spawn_3`, `solid_12`, `import_1`, `prefab_2`) assigned from `next_id`, which
//! only grows, so an agent's reference to a removed id fails loudly instead of pointing at something else. The one
//! exception is a `clear` that empties the project: it starts over, so a generator that runs again gets the same ids.
//!
//! Heights (`terrain.r16`): little-endian u16, row-major over the VERTEX grid `(cells_x + 1) x (cells_z + 1)`
//! (row = z, column = x); world y = `origin[1] + raw / 65535 * height_scale`. A missing file = flat at raw 0.

use crate::diag::{Code, Diagnostic, Diagnostics};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The format version this crate reads and writes.
pub const FORMAT: u32 = 1;
/// File names inside a project folder.
pub const MAP_FILE: &str = "map.json";
pub const TERRAIN_FILE: &str = "terrain.r16";
pub const ASSETS_DIR: &str = "assets";
pub const BUILD_DIR: &str = "build";
pub const HISTORY_DIR: &str = ".mapkit/history";
/// The units line every project carries so a reader without the docs knows the scale.
pub const UNITS_DOC: &str = "world units: a standing player is 100 tall, rotations in degrees, solids are centre + full size";
/// Half the standing body's height: `ground: true` puts a player spawn this far above the floor (DC-DEFAULT:
/// MAP-11). It is the half height of the game server's fallback player box ([24, 53, 24], `validate::SERVER_BODY`;
/// the ROM stand box is [15, 50, 15]): with the centre here the body's bottom rests on the floor, so the
/// server's own drop (DSV-48: lift 18, sweep down) starts free even on a slope and never keeps a spawn embedded.
/// A lower value (the old 40) left the 106-tall fallback body 13 units in the floor, more than the lift clears
/// on a 15-degree slope (review C15: the server would then start the player stuck).
pub const STAND_OFFSET: f32 = 53.0;
/// Most corners one polygon may have: the world file stores a polygon's corner count in one byte
/// (`dc_world::synth` writes `poly_vert_counts` as `u8`; FMT_DAT85.md §5.2), so a cylinder's `sides` and a
/// prism's `points.len()` stop at 255 (DC-DEFAULT: MAP-13, `solid.shape`).
pub const MAX_CORNERS: usize = 255;

pub type Vec3f = [f32; 3];
pub type Rgb8 = [u8; 3];

/// The whole project. Field docs live in [`crate::schema`] (a test checks every key is documented).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Project {
    pub format: u32,
    pub name: String,
    pub key: String,
    pub seed: u64,
    pub units: String,
    pub next_id: u32,
    pub terrain: Option<Terrain>,
    pub materials: Vec<Material>,
    pub solids: Vec<Solid>,
    pub prefabs: Vec<PrefabInstance>,
    pub imports: Vec<Import>,
    pub objects: Vec<MapObject>,
    pub settings: WorldSettings,
    pub mod_meta: ModMeta,
    pub build: BuildSettings,
    /// Models the `asset` verb converted, by name (MAP-104).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, AssetRecord>,
}

/// A converted model: one mesh and material per part, placed together by `asset.place`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AssetRecord {
    pub parts: Vec<AssetPart>,
    /// Width, height and depth in metres, base on the origin.
    pub size: Vec3f,
    pub tris: usize,
    /// The convex outline of the model seen from above (metres, counter-clockwise), for its hidden core (MAP-105).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub footprint: Vec<[f32; 2]>,
    /// Boxes following the model's solid parts, `[min x, y, z, max x, y, z]` in metres: when there are any they are its
    /// hidden core instead of the footprint, so a shot goes through what can be seen through (MAP-109).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hull_boxes: Vec<[f32; 6]>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AssetPart {
    pub file: String,
    pub material: String,
}

/// A credit for somebody else's work in the map (MOD-28).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Credit {
    pub title: String,
    pub author: String,
    pub license: String,
    pub source: String,
    pub changes: String,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            format: FORMAT,
            name: "Untitled".into(),
            key: "untitled".into(),
            seed: 1,
            units: UNITS_DOC.into(),
            next_id: 1,
            terrain: None,
            materials: Vec::new(),
            solids: Vec::new(),
            prefabs: Vec::new(),
            imports: Vec::new(),
            objects: Vec::new(),
            settings: WorldSettings::default(),
            mod_meta: ModMeta::default(),
            build: BuildSettings::default(),
            assets: BTreeMap::new(),
        }
    }
}

/// Heightmap terrain settings; the heights themselves live in `file`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Terrain {
    pub cells_x: u32,
    pub cells_z: u32,
    pub cell_size: f32,
    pub origin: Vec3f,
    pub height_scale: f32,
    pub file: String,
    pub layers: Vec<TerrainLayer>,
    pub paint: Option<String>,
}

impl Default for Terrain {
    fn default() -> Self {
        Terrain {
            cells_x: 64,
            cells_z: 64,
            cell_size: 64.0,
            origin: [-2048.0, -256.0, -2048.0],
            height_scale: 1024.0,
            file: TERRAIN_FILE.into(),
            layers: Vec::new(),
            paint: None,
        }
    }
}

impl Terrain {
    pub fn cols(&self) -> usize {
        self.cells_x as usize + 1
    }
    pub fn rows(&self) -> usize {
        self.cells_z as usize + 1
    }
    pub fn vertex_count(&self) -> usize {
        self.cols() * self.rows()
    }
    /// World x extent of the grid (min, max).
    pub fn span_x(&self) -> (f32, f32) {
        (
            self.origin[0],
            self.origin[0] + self.cells_x as f32 * self.cell_size,
        )
    }
    pub fn span_z(&self) -> (f32, f32) {
        (
            self.origin[2],
            self.origin[2] + self.cells_z as f32 * self.cell_size,
        )
    }
    /// Raw u16 -> world y.
    pub fn raw_to_y(&self, raw: u16) -> f32 {
        self.origin[1] + raw as f32 / 65535.0 * self.height_scale
    }
    /// World y -> raw u16 (clamped to the range).
    pub fn y_to_raw(&self, y: f32) -> u16 {
        let t = (y - self.origin[1]) / self.height_scale * 65535.0;
        t.round().clamp(0.0, 65535.0) as u16
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerrainLayer {
    pub material: String,
}

impl Default for TerrainLayer {
    fn default() -> Self {
        TerrainLayer {
            material: "ground".into(),
        }
    }
}

/// Where a texture comes from: the user's ROM at run time (never committed) or an own PNG under `assets/`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureRef {
    /// Archive-relative ROM path, resolved at build time from the user's own ROM.
    Rom(String),
    /// Project-relative PNG path under `assets/`, converted to DTX at build time.
    Own(String),
}

impl Default for TextureRef {
    fn default() -> Self {
        TextureRef::Own("assets/texture.png".into())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MaterialFlags {
    pub solid: bool,
    pub no_step: bool,
    pub sky: bool,
    /// Collision only: a solid of this material stops players and bullets and is never drawn, the way to give a
    /// see-through model a simple hull (DC-DEFAULT: MAP-104).
    #[serde(skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// With `hidden`: casts no shadow and bounces no light. A thin layer of collision over paint, so that bullet marks
    /// land in front of the paint and not behind it (DC-DEFAULT: MAP-111).
    #[serde(skip_serializing_if = "is_false")]
    pub skin: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Default for MaterialFlags {
    fn default() -> Self {
        MaterialFlags {
            solid: true,
            no_step: false,
            sky: false,
            hidden: false,
            skin: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Material {
    pub id: String,
    pub texture: TextureRef,
    pub tile: f32,
    pub surface_id: u8,
    pub flags: MaterialFlags,
    /// Fit this part of the texture `[u0, v0, u1, v1]` (0..1, v down) once across each face of a solid, instead
    /// of tiling it: how an atlas texture or a whole container side goes on one face (MAP-97).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<[f32; 4]>,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            id: "material".into(),
            texture: TextureRef::default(),
            tile: 128.0,
            surface_id: 0,
            flags: MaterialFlags::default(),
            fit: None,
        }
    }
}

/// Convex solid shapes. A cylinder stores its unit circle (computed once at authoring time with f64 sin / cos),
/// so the build path never calls a transcendental (MAPKIT_PLAN.md section 7 determinism).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolidKind {
    /// Axis box of `size`, rotated about y.
    #[default]
    Box,
    /// A box cut from its front-bottom edge (-z) to its back-top edge (+z): the slope faces -z and up.
    Wedge,
    /// `sides` quads around y; `circle` = the `sides` unit points (cos, sin), CCW seen from above.
    Cylinder { sides: u32, circle: Vec<[f32; 2]> },
    /// A convex CCW outline in local x / z (world units), extruded `size[1]` along y.
    Prism { points: Vec<[f32; 2]> },
}

/// What a solid is at build time: level geometry (`None`) or a brush model with an object of the given class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrushRole {
    WorldModel,
    KillVolume,
    Ladder,
    Breakable,
    Sky,
}

impl BrushRole {
    /// The `dc.*` class of the object that names the brush model.
    pub fn class(self) -> &'static str {
        use crate::vocab_names as v;
        match self {
            BrushRole::WorldModel => v::CLASS_WORLD_MODEL,
            BrushRole::KillVolume => v::CLASS_KILL_VOLUME,
            BrushRole::Ladder => v::CLASS_LADDER,
            BrushRole::Breakable => v::CLASS_BREAKABLE,
            BrushRole::Sky => v::CLASS_SKY_MODEL,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Solid {
    pub id: String,
    pub kind: SolidKind,
    pub pos: Vec3f,
    pub rot_y_deg: f32,
    pub size: Vec3f,
    pub material: String,
    pub faces: BTreeMap<String, String>,
    pub group: Option<String>,
    pub brush: Option<BrushRole>,
}

impl Default for Solid {
    fn default() -> Self {
        Solid {
            id: "solid_0".into(),
            kind: SolidKind::Box,
            pos: [0.0; 3],
            rot_y_deg: 0.0,
            size: [64.0; 3],
            material: "ground".into(),
            faces: BTreeMap::new(),
            group: None,
            brush: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrefabInstance {
    pub id: String,
    pub prefab: String,
    pub pos: Vec3f,
    pub rot_y_deg: f32,
    pub scale: f32,
}

impl Default for PrefabInstance {
    fn default() -> Self {
        PrefabInstance {
            id: "prefab_0".into(),
            prefab: "crate".into(),
            pos: [0.0; 3],
            rot_y_deg: 0.0,
            scale: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Import {
    pub id: String,
    pub file: String,
    pub pos: Vec3f,
    pub rot_y_deg: f32,
    pub scale: f32,
    pub material: String,
    pub collision: bool,
}

impl Default for Import {
    fn default() -> Self {
        Import {
            id: "import_0".into(),
            file: "assets/mesh.obj".into(),
            pos: [0.0; 3],
            rot_y_deg: 0.0,
            scale: 1.0,
            material: "ground".into(),
            collision: true,
        }
    }
}

/// A property value. JSON: a string, a number, a bool, a 3-array, or `{"rgb": [r, g, b]}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropValue {
    Bool(bool),
    Num(f64),
    Text(String),
    Vec3(Vec3f),
    Rgb { rgb: Rgb8 },
}

impl PropValue {
    pub fn kind(&self) -> &'static str {
        match self {
            PropValue::Bool(_) => "bool",
            PropValue::Num(_) => "num",
            PropValue::Text(_) => "text",
            PropValue::Vec3(_) => "vec3",
            PropValue::Rgb { .. } => "rgb",
        }
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            PropValue::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_num(&self) -> Option<f64> {
        match self {
            PropValue::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_vec3(&self) -> Option<Vec3f> {
        match self {
            PropValue::Vec3(v) => Some(*v),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MapObject {
    pub id: String,
    pub class: String,
    pub name: String,
    pub pos: Vec3f,
    pub rot: Vec3f,
    pub props: BTreeMap<String, PropValue>,
}

impl Default for MapObject {
    fn default() -> Self {
        MapObject {
            id: "player_spawn_0".into(),
            class: crate::vocab_names::CLASS_PLAYER_SPAWN.into(),
            name: String::new(),
            pos: [0.0; 3],
            rot: [0.0; 3],
            props: BTreeMap::new(),
        }
    }
}

impl MapObject {
    pub fn text(&self, key: &str) -> Option<&str> {
        self.props.get(key).and_then(PropValue::as_text)
    }
    pub fn num(&self, key: &str) -> Option<f64> {
        self.props.get(key).and_then(PropValue::as_num)
    }
}

/// Fog, far plane, sun, ambient and sky: the build stage emits `dc.world_settings`, `dc.global_light` and the
/// `dc.sky_model` from these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorldSettings {
    pub fog_enable: bool,
    pub fog_near: f32,
    pub fog_far: f32,
    pub fog_color: Rgb8,
    pub far_z: f32,
    pub sun_dir: Vec3f,
    pub sun_color: Rgb8,
    pub sun_bright: f32,
    pub ambient: Rgb8,
    pub sky_size: Vec3f,
    pub sky_material: String,
}

impl Default for WorldSettings {
    fn default() -> Self {
        WorldSettings {
            fog_enable: true,
            fog_near: 1024.0,
            fog_far: 6000.0,
            fog_color: [140, 160, 190],
            far_z: 8192.0,
            sun_dir: [-0.4, -0.8, 0.3],
            sun_color: [255, 245, 225],
            sun_bright: 1.0,
            ambient: [90, 95, 110],
            sky_size: [1024.0, 1024.0, 1024.0],
            sky_material: "sky".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Players {
    pub min: u32,
    pub max: u32,
}

impl Default for Players {
    fn default() -> Self {
        Players { min: 2, max: 16 }
    }
}

/// What `export` writes into the mod manifest and the `mod.content.maps:register` record (MAPKIT_PLAN.md 5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModMeta {
    pub id: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub tags: Vec<String>,
    pub homepage: Option<String>,
    pub modes: Vec<String>,
    pub goals: BTreeMap<String, u32>,
    pub round_seconds: Option<u32>,
    pub players: Option<Players>,
    /// Somebody else's work in the map, shipped with it (MOD-28).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credits: Vec<Credit>,
}

impl Default for ModMeta {
    fn default() -> Self {
        ModMeta {
            id: "untitled_map".into(),
            version: "0.1.0".into(),
            author: "unknown".into(),
            description: "A community map built with dc-mapkit.".into(),
            tags: vec!["map".into()],
            homepage: None,
            modes: vec!["TDM".into(), "FFA".into()],
            goals: BTreeMap::new(),
            round_seconds: None,
            players: None,
            credits: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bake {
    pub ambient: f32,
    pub sun: f32,
    pub hemisphere: f32,
    pub ao: f32,
    /// Shadows on floors and walls from solids, hidden cores included: 0 (off, the default) .. 1 (DC-DEFAULT: MAP-104).
    #[serde(skip_serializing_if = "is_zero")]
    pub shadow: f32,
    /// World units a lightmap texel covers: above 0 turns on lightmaps and the lamps (DC-DEFAULT: MAP-108).
    #[serde(skip_serializing_if = "is_zero")]
    pub lightmap: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

impl Default for Bake {
    fn default() -> Self {
        Bake {
            ambient: 0.35,
            sun: 0.65,
            hemisphere: 0.15,
            ao: 0.5,
            shadow: 0.0,
            lightmap: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuildSettings {
    pub cell_polys: u32,
    pub bake: Bake,
}

impl Default for BuildSettings {
    fn default() -> Self {
        BuildSettings {
            cell_polys: 16,
            bake: Bake::default(),
        }
    }
}

/// The corner cap of a solid's polygons ([`MAX_CORNERS`]): a cylinder's `sides` and a prism's `points` count
/// are the corner count of its top and bottom faces. `Err` names the overrun (the `solid.shape` message).
pub fn check_solid_corners(s: &Solid) -> Result<(), String> {
    let corners = match &s.kind {
        SolidKind::Box | SolidKind::Wedge => return Ok(()),
        SolidKind::Cylinder { sides, circle } => (*sides as usize).max(circle.len()),
        SolidKind::Prism { points } => points.len(),
    };
    if corners > MAX_CORNERS {
        return Err(format!(
            "{corners} corners; a polygon holds at most {MAX_CORNERS} (the world file counts them in one byte)"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// paths, keys, ids

/// The project-relative path rule (DC-DEFAULT: MAP-10): forward slashes only, no drive letter, no leading
/// slash, no `..` segment, no empty segment, no control character.
pub fn check_rel_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("empty path".into());
    }
    if path.contains('\\') {
        return Err(format!("`{path}` has a backslash (use forward slashes)"));
    }
    let b = path.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return Err(format!("`{path}` starts with a drive letter"));
    }
    if path.starts_with('/') {
        return Err(format!(
            "`{path}` is absolute (paths are relative to the project)"
        ));
    }
    if path.chars().any(|c| c.is_control()) {
        return Err(format!("`{path}` has a control character"));
    }
    for seg in path.split('/') {
        if seg == ".." {
            return Err(format!("`{path}` has a `..` segment"));
        }
        if seg.is_empty() {
            return Err(format!("`{path}` has an empty segment"));
        }
    }
    Ok(())
}

/// The map key rule: 1..32 characters of `a-z 0-9 _` (MAPKIT_PLAN.md 5.4).
pub fn check_key(key: &str) -> bool {
    (1..=32).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Make a key from a free name: lower-case, runs of other characters become one `_`, 1..32 characters.
pub fn key_from_name(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
        if out.len() >= 32 {
            break;
        }
    }
    if out.is_empty() {
        "map".into()
    } else {
        out
    }
}

/// The id rule: `<prefix>_<n>` with `a-z 0-9 _` only and a decimal suffix.
pub fn check_id(id: &str) -> bool {
    id_suffix(id).is_some()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The numeric suffix of an id (`solid_12` -> 12).
pub fn id_suffix(id: &str) -> Option<u32> {
    let (_, n) = id.rsplit_once('_')?;
    if n.is_empty() || n.len() > 9 {
        return None;
    }
    n.parse().ok()
}

impl Project {
    /// Hand out the next id for `prefix` (`<prefix>_<n>`); `next_id` only grows.
    pub fn alloc_id(&mut self, prefix: &str) -> String {
        let n = self.next_id;
        self.next_id += 1;
        format!("{prefix}_{n}")
    }

    /// Every id of a placed thing (objects, solids, prefabs, imports), in file order.
    pub fn all_ids(&self) -> Vec<&str> {
        let mut v: Vec<&str> = Vec::new();
        v.extend(self.objects.iter().map(|o| o.id.as_str()));
        v.extend(self.solids.iter().map(|s| s.id.as_str()));
        v.extend(self.prefabs.iter().map(|p| p.id.as_str()));
        v.extend(self.imports.iter().map(|i| i.id.as_str()));
        v
    }

    pub fn has_id(&self, id: &str) -> bool {
        self.all_ids().contains(&id)
    }

    pub fn object(&self, id: &str) -> Option<&MapObject> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn object_mut(&mut self, id: &str) -> Option<&mut MapObject> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    pub fn solid(&self, id: &str) -> Option<&Solid> {
        self.solids.iter().find(|s| s.id == id)
    }

    pub fn solid_mut(&mut self, id: &str) -> Option<&mut Solid> {
        self.solids.iter_mut().find(|s| s.id == id)
    }

    pub fn material(&self, id: &str) -> Option<&Material> {
        self.materials.iter().find(|m| m.id == id)
    }

    pub fn material_index(&self, id: &str) -> Option<usize> {
        self.materials.iter().position(|m| m.id == id)
    }

    /// Objects of one class.
    pub fn of_class<'a>(&'a self, class: &'a str) -> impl Iterator<Item = &'a MapObject> + 'a {
        self.objects.iter().filter(move |o| o.class == class)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// heights

/// The heightmap in memory: raw u16 over the vertex grid, row-major (row = z, column = x).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heights {
    pub cols: usize,
    pub rows: usize,
    pub data: Vec<u16>,
}

impl Heights {
    pub fn flat(cols: usize, rows: usize, raw: u16) -> Self {
        Heights {
            cols,
            rows,
            data: vec![raw; cols * rows],
        }
    }

    pub fn for_terrain(t: &Terrain, raw: u16) -> Self {
        Self::flat(t.cols(), t.rows(), raw)
    }

    /// The empty heightmap of a project without terrain.
    pub fn none() -> Self {
        Heights {
            cols: 0,
            rows: 0,
            data: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn get(&self, x: usize, z: usize) -> u16 {
        self.data[z * self.cols + x]
    }

    pub fn set(&mut self, x: usize, z: usize, v: u16) {
        self.data[z * self.cols + x] = v;
    }

    /// Parse the `terrain.r16` bytes (little-endian u16, row-major).
    pub fn from_r16(bytes: &[u8], cols: usize, rows: usize) -> Result<Self, String> {
        if bytes.len() != cols * rows * 2 {
            return Err(format!(
                "terrain holds {} bytes, expected {} ({cols} x {rows} x 2)",
                bytes.len(),
                cols * rows * 2
            ));
        }
        let data = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        Ok(Heights { cols, rows, data })
    }

    pub fn to_r16(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() * 2);
        for h in &self.data {
            out.extend_from_slice(&h.to_le_bytes());
        }
        out
    }

    /// Raw min / max over the grid (`None` when empty).
    pub fn raw_range(&self) -> Option<(u16, u16)> {
        let min = *self.data.iter().min()?;
        let max = *self.data.iter().max()?;
        Some((min, max))
    }

    /// Bilinear world height at world (x, z); `None` outside the grid or without terrain.
    pub fn sample(&self, t: &Terrain, x: f32, z: f32) -> Option<f32> {
        if self.is_empty() || self.cols != t.cols() || self.rows != t.rows() {
            return None;
        }
        let fx = (x - t.origin[0]) / t.cell_size;
        let fz = (z - t.origin[2]) / t.cell_size;
        if fx < 0.0 || fz < 0.0 || fx > t.cells_x as f32 || fz > t.cells_z as f32 {
            return None;
        }
        let x0 = (fx.floor() as usize).min(t.cells_x as usize - 1);
        let z0 = (fz.floor() as usize).min(t.cells_z as usize - 1);
        let tx = fx - x0 as f32;
        let tz = fz - z0 as f32;
        let h = |xx: usize, zz: usize| t.raw_to_y(self.get(xx, zz));
        let top = h(x0, z0) * (1.0 - tx) + h(x0 + 1, z0) * tx;
        let bottom = h(x0, z0 + 1) * (1.0 - tx) + h(x0 + 1, z0 + 1) * tx;
        Some(top * (1.0 - tz) + bottom * tz)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// load / save

fn io_diag(path: &Path, what: &str, e: impl std::fmt::Display) -> Diagnostics {
    Diagnostics::one(Diagnostic::new(
        Code::ProjectIo,
        path.display().to_string(),
        format!("{what}: {e}"),
    ))
}

/// Write `bytes` to `path` through a temp file in the same folder and a rename (never a half-written file).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    // Windows refuses to rename over an existing file; remove it first (the temp file still holds the content).
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&tmp, path)
}

impl Project {
    /// Parse map.json text.
    pub fn from_json(text: &str) -> Result<Self, Diagnostics> {
        let p: Project = serde_json::from_str(text).map_err(|e| {
            Diagnostics::one(Diagnostic::new(Code::ProjectParse, MAP_FILE, e.to_string()))
        })?;
        if p.format != FORMAT {
            return Err(Diagnostics::one(Diagnostic::new(
                Code::ProjectFormat,
                "format",
                format!(
                    "format {} is not supported (this tool reads {FORMAT})",
                    p.format
                ),
            )));
        }
        Ok(p)
    }

    /// Pretty JSON with a trailing newline (the canonical on-disk form; keys in struct order, maps sorted).
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("project serialises");
        s.push('\n');
        s
    }

    /// Read `<dir>/map.json`.
    pub fn load(dir: &Path) -> Result<Self, Diagnostics> {
        let path = dir.join(MAP_FILE);
        let text = fs::read_to_string(&path).map_err(|e| io_diag(&path, "read", e))?;
        Self::from_json(&text)
    }

    /// Write `<dir>/map.json` by temp file + rename.
    pub fn save(&self, dir: &Path) -> Result<(), Diagnostics> {
        let path = dir.join(MAP_FILE);
        write_atomic(&path, self.to_json().as_bytes()).map_err(|e| io_diag(&path, "write", e))
    }

    /// Read the heightmap named by `terrain` (flat at raw 0 when the file is absent; empty without terrain).
    pub fn load_heights(&self, dir: &Path) -> Result<Heights, Diagnostics> {
        let Some(t) = &self.terrain else {
            return Ok(Heights::none());
        };
        check_rel_path(&t.file)
            .map_err(|e| Diagnostics::one(Diagnostic::new(Code::PathInvalid, "terrain.file", e)))?;
        let path = dir.join(&t.file);
        if !path.exists() {
            return Ok(Heights::for_terrain(t, 0));
        }
        let bytes = fs::read(&path).map_err(|e| io_diag(&path, "read", e))?;
        Heights::from_r16(&bytes, t.cols(), t.rows())
            .map_err(|e| Diagnostics::one(Diagnostic::new(Code::TerrainSize, t.file.clone(), e)))
    }

    /// Write the heightmap next to map.json (no-op without terrain).
    pub fn save_heights(&self, dir: &Path, heights: &Heights) -> Result<(), Diagnostics> {
        let Some(t) = &self.terrain else {
            return Ok(());
        };
        let path = dir.join(&t.file);
        write_atomic(&path, &heights.to_r16()).map_err(|e| io_diag(&path, "write", e))
    }

    /// Load the project and its heights together.
    pub fn open(dir: &Path) -> Result<(Self, Heights), Diagnostics> {
        let p = Self::load(dir)?;
        let h = p.load_heights(dir)?;
        Ok((p, h))
    }

    /// Save both files.
    pub fn save_all(&self, dir: &Path, heights: &Heights) -> Result<(), Diagnostics> {
        self.save(dir)?;
        self.save_heights(dir, heights)
    }

    /// Resolve a project-relative path (after [`check_rel_path`]).
    pub fn resolve(dir: &Path, rel: &str) -> PathBuf {
        let mut p = dir.to_path_buf();
        for seg in rel.split('/') {
            p.push(seg);
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Project {
        let mut p = Project {
            name: "Round Trip".into(),
            key: "round_trip".into(),
            seed: 7,
            terrain: Some(Terrain::default()),
            ..Default::default()
        };
        p.materials.push(Material {
            id: "ground".into(),
            texture: TextureRef::Own("assets/ground.png".into()),
            ..Default::default()
        });
        p.materials.push(Material {
            id: "rock".into(),
            texture: TextureRef::Rom("some/rom/path.dtx".into()),
            surface_id: 3,
            flags: MaterialFlags {
                no_step: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let id = p.alloc_id("solid");
        p.solids.push(Solid {
            id,
            kind: SolidKind::Cylinder {
                sides: 3,
                circle: vec![[1.0, 0.0], [-0.5, 0.866], [-0.5, -0.866]],
            },
            faces: BTreeMap::from([("top".to_string(), "rock".to_string())]),
            group: Some("cover".into()),
            brush: Some(BrushRole::KillVolume),
            ..Default::default()
        });
        let id = p.alloc_id("player_spawn");
        let mut props = BTreeMap::new();
        props.insert("team".to_string(), PropValue::Text("1".into()));
        props.insert("number".to_string(), PropValue::Num(1.0));
        props.insert("extent".to_string(), PropValue::Vec3([128.0, 48.0, 128.0]));
        props.insert("color".to_string(), PropValue::Rgb { rgb: [255, 0, 10] });
        props.insert("visible".to_string(), PropValue::Bool(false));
        p.objects.push(MapObject {
            id,
            props,
            ..Default::default()
        });
        p.prefabs.push(PrefabInstance::default());
        p.imports.push(Import::default());
        p.mod_meta.goals.insert("TDM".into(), 100);
        p.mod_meta.players = Some(Players::default());
        p
    }

    #[test]
    fn map_json_round_trips_and_is_stable() {
        let p = sample();
        let text = p.to_json();
        let back = Project::from_json(&text).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.to_json(), text);
        // the untagged prop values keep their kinds
        let o = &back.objects[0];
        assert_eq!(o.props["team"], PropValue::Text("1".into()));
        assert_eq!(o.props["number"], PropValue::Num(1.0));
        assert_eq!(o.props["extent"], PropValue::Vec3([128.0, 48.0, 128.0]));
        assert_eq!(o.props["color"], PropValue::Rgb { rgb: [255, 0, 10] });
        assert_eq!(o.props["visible"], PropValue::Bool(false));
        assert!(text.contains("\"own\": \"assets/ground.png\""));
        assert!(text.contains("\"rom\": \"some/rom/path.dtx\""));
    }

    #[test]
    fn unknown_keys_and_other_formats_are_refused() {
        let e = Project::from_json("{\"format\": 1, \"bogus\": 1}").unwrap_err();
        assert_eq!(e.items[0].code, Code::ProjectParse);
        let e = Project::from_json("{\"format\": 2}").unwrap_err();
        assert_eq!(e.items[0].code, Code::ProjectFormat);
    }

    #[test]
    fn path_rule() {
        assert!(check_rel_path("assets/ground.png").is_ok());
        assert!(check_rel_path("terrain.r16").is_ok());
        assert!(check_rel_path("assets\\ground.png").is_err());
        assert!(check_rel_path("C:/x.png").is_err());
        assert!(check_rel_path("/etc/x.png").is_err());
        assert!(check_rel_path("assets/../x.png").is_err());
        assert!(check_rel_path("assets//x.png").is_err());
        assert!(check_rel_path("").is_err());
    }

    #[test]
    fn keys_and_ids() {
        assert!(check_key("crossfire_valley"));
        assert!(!check_key("Crossfire"));
        assert!(!check_key(""));
        assert_eq!(key_from_name("Crossfire Valley!"), "crossfire_valley");
        assert_eq!(key_from_name("***"), "map");
        assert!(check_id("player_spawn_12"));
        assert!(!check_id("player_spawn"));
        assert!(!check_id("Solid_1"));
        assert_eq!(id_suffix("solid_12"), Some(12));
        let mut p = Project::default();
        assert_eq!(p.alloc_id("solid"), "solid_1");
        assert_eq!(p.alloc_id("import"), "import_2");
        assert_eq!(p.next_id, 3);
    }

    #[test]
    fn heights_round_trip_and_sample() {
        let t = Terrain {
            cells_x: 2,
            cells_z: 2,
            cell_size: 10.0,
            origin: [0.0, 0.0, 0.0],
            height_scale: 65535.0,
            ..Default::default()
        };
        let mut h = Heights::for_terrain(&t, 0);
        for z in 0..3 {
            for x in 0..3 {
                h.set(x, z, (x * 100 + z * 1000) as u16);
            }
        }
        let bytes = h.to_r16();
        assert_eq!(bytes.len(), 18);
        assert_eq!(&bytes[2..4], &100u16.to_le_bytes());
        let back = Heights::from_r16(&bytes, 3, 3).unwrap();
        assert_eq!(back, h);
        assert!(Heights::from_r16(&bytes[1..], 3, 3).is_err());
        assert_eq!(h.sample(&t, 0.0, 0.0), Some(0.0));
        assert_eq!(h.sample(&t, 10.0, 0.0), Some(100.0));
        assert_eq!(h.sample(&t, 5.0, 5.0), Some(550.0));
        assert_eq!(h.sample(&t, 20.0, 20.0), Some(2200.0));
        assert_eq!(h.sample(&t, 21.0, 0.0), None);
        assert_eq!(t.y_to_raw(t.raw_to_y(1234)), 1234);
    }

    #[test]
    fn save_and_open_use_a_folder() {
        let dir = std::env::temp_dir().join(format!("dc-mapkit-project-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let p = sample();
        let h = Heights::for_terrain(p.terrain.as_ref().unwrap(), 5);
        p.save_all(&dir, &h).unwrap();
        let (p2, h2) = Project::open(&dir).unwrap();
        assert_eq!(p2, p);
        assert_eq!(h2, h);
        assert!(!dir.read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")));
        fs::remove_file(dir.join(TERRAIN_FILE)).unwrap();
        let h3 = p2.load_heights(&dir).unwrap();
        assert_eq!(h3, Heights::for_terrain(p.terrain.as_ref().unwrap(), 0));
        let _ = fs::remove_dir_all(&dir);
    }
}
