//! Composing a world from a community map and pieces of the player's own ROM worlds (DC-DEFAULT: MAP-116).
//!
//! A map built by dc-mapkit may name parts of the ROM's worlds instead of carrying them: each `dc.rom_piece`
//! object names a world of the ROM's world archive, the offset it is moved by, a region box and cut boxes (in the
//! composed world's coordinates). Every program that loads a map (the game server, the client, the bot client and
//! dc-mapkit) runs [`compose`] on the bytes it read, with its own ROM, before it parses them. The answer is one
//! ordinary version-85 world; the map file itself holds nothing of the ROM but paths and digests (CLEANROOM §1,
//! §4: the pieces are read from the player's ROM at load time, the recipe rule the camo packs follow too).
//!
//! What a piece brings, moved by its offset and kept to its region outside its cuts:
//! - collision: the ROM level model's polygons clipped, on the ROM's own tree, joined to the map's tree by a kd
//!   partition over the regions ([`tree`]); the ROM's sky faces are left out (the map's sky shell is the sky);
//! - render: the top-level blocks clipped ([`render`]); lightmaps and light groups kept;
//! - brushes: kill volumes and hazards clipped as convex volumes, every other brush model kept whole when its box
//!   centre lies in the region outside the cuts, renamed `<piece>~<name>` with its object and render model;
//! - objects: player spawns as team-less `dc.player_spawn` (`spawns = "ffa"`, near duplicates merged) or none,
//!   item spawns as `dc.item_spawn` (`items`), lights, sounds, sprites, effects and explosions as they are
//!   (renamed, moved, text references to renamed objects follow); mode controllers, start-up commands, triggers,
//!   flags, flag zones, bomb sites, cameras, world settings and global lights stay out (the map's own come first);
//! - the sky model of the one piece with `sky` (the map's own sky model is dropped then);
//! - blockers clipped the same way; particle blockers left out (nothing reads them);
//! - the light grid: one grid over the composed extents ([`grid`]).
//!
//! Pieces never overlap (an error otherwise). The composed bytes are the same on every machine with the same ROM
//! world (a piece may pin its world's SHA-256): fixed iteration order, no hash maps, plain `f32` arithmetic.

pub mod clip;
pub mod fill;
#[doc(hidden)]
pub mod fixture;
pub mod grid;
pub mod render;
pub mod tree;

use crate::names;
use crate::synth::{SynthModel, SynthObject, SynthProp, SynthRenderWorld, SynthValue, SynthWorld};
use crate::{BspModel, NodeRef, Object, ObjectRole, PropKind, Property, World, WorldError};
use clip::{Aabb, HalfSpace, V};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The class of a piece record (DeCombat vocabulary; written by dc-mapkit, consumed here).
pub const PIECE_CLASS: &str = "dc.rom_piece";
/// Property names of a piece record (the three leading ones are its id, its offset and an unused rotation).
pub const PROP_WORLD: &str = "world";
pub const PROP_SHA256: &str = "sha256";
pub const PROP_REGION_MIN: &str = "region_min";
pub const PROP_REGION_MAX: &str = "region_max";
pub const PROP_CUTS: &str = "cuts";
pub const PROP_SPAWNS: &str = "spawns";
pub const PROP_ITEMS: &str = "items";
pub const PROP_SKY: &str = "sky";
pub const PROP_BLOCKERS: &str = "blockers";
pub const PROP_FILL: &str = "fill";
pub const PROP_FILL_Y: &str = "fill_y";
pub const PROP_FILL_TEXTURE: &str = "fill_texture";
pub const PROP_FILL_TILE: &str = "fill_tile";
pub const PROP_FILL_COLOR: &str = "fill_color";
pub const PROP_FILL_SURFACE: &str = "fill_surface";
pub const PROP_MIN_LIGHT: &str = "min_light";
pub const PROP_DROP_SPAWNS: &str = "drop_spawns";
/// A dropped spawn point takes the world's player spawns this near across, and this near up or down (MAP-125).
pub const DROP_SPAWN_NEAR: f32 = 48.0;
pub const DROP_SPAWN_NEAR_Y: f32 = 96.0;
/// An open piece's kill volumes and hazards are kept to its play area grown by this much across (MAP-120).
pub const OPEN_KILL_MARGIN: f32 = 96.0;
/// An open, filled piece keeps its kill volumes and hazards only this far under its fill floor and lower (MAP-120).
pub const KILL_UNDER: f32 = 100.0;
/// The grid the fill floor is laid on (MAP-120).
pub const FILL_CELL: f32 = 128.0;
/// What a piece record may ask for. A pack is downloaded data, so nothing here sizes an allocation by a record's
/// numbers alone: past these bounds the map is refused at load (a [`ComposeError`]).
// DC-DEFAULT: MAP-116
pub const MAX_PIECES: usize = 64;
/// Cuts per piece (each one clips every polygon the region keeps).
pub const MAX_CUTS: usize = 256;
pub const MAX_DROP_SPAWNS: usize = 1024;
/// A region's extent on each axis (the extents limit of a world; the fill grid and the clipping are sized by it).
pub const MAX_REGION_EXTENT: f32 = 65_536.0;
/// Any coordinate of a region, a cut or an offset.
pub const MAX_COORD: f32 = 1_048_576.0;
/// The pieces' worlds together, and the composed world: 1 GiB.
pub const MAX_COMPOSED_BYTES: usize = 1 << 30;
/// Joins a piece's name and an original object name (`piece_3~name`).
pub const RENAME_SEP: char = '~';
/// Longest object / model name the engine reads (FMT_DAT85.md §5.2: `lstr` max 0x40 keeps 63 bytes).
pub const MAX_NAME: usize = 63;
/// Player spawns closer than this to one already kept are merged (the ROM has one set per mode).
pub const SPAWN_MERGE: f32 = 48.0;
/// Margin added around the composed geometry for the world extents (as dc-mapkit's `EXTENT_MARGIN`).
pub const EXTENT_MARGIN: f32 = 128.0;

/// What a piece does with the ROM's player spawns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spawns {
    /// Every spawn as a team-less `dc.player_spawn` serving every mode.
    Ffa,
    /// None.
    None,
}

impl Spawns {
    pub fn parse(s: &str) -> Option<Spawns> {
        match s.to_ascii_lowercase().as_str() {
            "ffa" | "" => Some(Spawns::Ffa),
            "none" => Some(Spawns::None),
            _ => None,
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Spawns::Ffa => "ffa",
            Spawns::None => "none",
        }
    }
}

/// One piece record.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// The piece's id (`piece_3`): the object's name and the prefix of everything it brings.
    pub name: String,
    /// The ROM world, as the archive spells it (`WORLDS\NAME.DAT`, `dc-mapkit rom worlds`).
    pub world: String,
    /// Lower-case hex SHA-256 of the world entry; empty = not checked.
    pub sha256: String,
    /// Added to every ROM coordinate.
    pub offset: V,
    /// Composed-world coordinates.
    pub region: Aabb,
    pub cuts: Vec<Aabb>,
    pub spawns: Spawns,
    pub items: bool,
    pub sky: bool,
    /// Keep the world's blockers (its invisible walls). `false` opens the piece: the blockers go, and its kill
    /// volumes and hazards are kept to its play area, so a player may walk out of it wherever its geometry lets
    /// them (MAP-120).
    pub blockers: bool,
    /// A floor laid in the region's columns where the world has no floor (in a closed piece only outside its play
    /// area), so nobody falls out of the world (MAP-120).
    pub fill: Option<Fill>,
    /// The least light of the piece's lightmaps and vertex colours, 0..255 per channel (0 = as the world has it):
    /// the outer faces of a world, never seen from its play area, got its ambient alone (MAP-120).
    pub min_light: u8,
    /// Points in the map's coordinates whose nearby player spawns of the world are not kept (MAP-125): a spawn the
    /// original map left inside a kill volume or in a place a player cannot leave.
    pub drop_spawns: Vec<V>,
}

/// The fill floor of a piece (MAP-120): height and look, from the map's own ground.
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    /// Height of the floor (map coordinates).
    pub y: f32,
    /// Texture name as the map's world writes it (a ROM path or an `@mod:` path).
    pub texture: String,
    /// World units per texture repeat.
    pub tile: f32,
    /// Vertex colour, 0..255 per channel (128 draws the texture as it is).
    pub color: [f32; 3],
    /// Footstep and impact surface id.
    pub surface: u16,
}

fn fmt_box(b: &Aabb) -> String {
    format!(
        "{} {} {} {} {} {}",
        b.min[0], b.min[1], b.min[2], b.max[0], b.max[1], b.max[2]
    )
}

/// `x0 y0 z0 x1 y1 z1; ...` (corners in any order) to boxes.
pub fn parse_boxes(s: &str) -> Result<Vec<Aabb>, String> {
    let mut out = Vec::new();
    for part in s.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let v: Result<Vec<f32>, _> = part.split_whitespace().map(str::parse::<f32>).collect();
        let v = v.map_err(|_| format!("`{part}` is not six numbers"))?;
        if v.len() != 6 || v.iter().any(|x| !x.is_finite()) {
            return Err(format!("`{part}` is not six finite numbers"));
        }
        let b = Aabb::new([v[0], v[1], v[2]], [v[3], v[4], v[5]]);
        if !b.is_valid() {
            return Err(format!("`{part}` is an empty box"));
        }
        out.push(b);
    }
    Ok(out)
}

/// Boxes to the text [`parse_boxes`] reads (shortest round-trip numbers).
/// `"x y z; x y z; ..."` (the record's drop_spawns, MAP-125); an empty text is none.
pub fn parse_points(s: &str) -> Result<Vec<V>, String> {
    let mut out = Vec::new();
    for part in s.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let v: Result<Vec<f32>, _> = part.split_whitespace().map(str::parse::<f32>).collect();
        match v {
            Ok(v) if v.len() == 3 && v.iter().all(|x| x.is_finite()) => {
                out.push([v[0], v[1], v[2]])
            }
            _ => return Err(format!("`{part}` is not three finite numbers")),
        }
    }
    Ok(out)
}

pub fn format_boxes(boxes: &[Aabb]) -> String {
    boxes.iter().map(fmt_box).collect::<Vec<_>>().join("; ")
}

impl Piece {
    /// The record as an object of the map's world.
    pub fn to_object(&self) -> SynthObject {
        let props = [
            (PROP_WORLD, SynthValue::Text(self.world.as_bytes().to_vec())),
            (
                PROP_SHA256,
                SynthValue::Text(self.sha256.as_bytes().to_vec()),
            ),
            (PROP_REGION_MIN, SynthValue::Vec3(self.region.min)),
            (PROP_REGION_MAX, SynthValue::Vec3(self.region.max)),
            (
                PROP_CUTS,
                SynthValue::Text(format_boxes(&self.cuts).into_bytes()),
            ),
            (
                PROP_SPAWNS,
                SynthValue::Text(self.spawns.text().as_bytes().to_vec()),
            ),
            (PROP_ITEMS, SynthValue::Bool(self.items)),
            (PROP_SKY, SynthValue::Bool(self.sky)),
            (PROP_BLOCKERS, SynthValue::Bool(self.blockers)),
            (PROP_FILL, SynthValue::Bool(self.fill.is_some())),
            (PROP_MIN_LIGHT, SynthValue::Int(self.min_light as f32)),
        ];
        let mut o = SynthObject::dc(PIECE_CLASS, &self.name, self.offset, [0.0; 3], &props);
        if !self.drop_spawns.is_empty() {
            // written only when there are some, so a map without them builds the bytes it built before
            let text = self
                .drop_spawns
                .iter()
                .map(|p| format!("{} {} {}", p[0], p[1], p[2]))
                .collect::<Vec<_>>()
                .join("; ");
            o.props.push(SynthProp::dc(
                PROP_DROP_SPAWNS,
                SynthValue::Text(text.into_bytes()),
            ));
        }
        if let Some(f) = &self.fill {
            let more = [
                (PROP_FILL_Y, SynthValue::Real(f.y)),
                (
                    PROP_FILL_TEXTURE,
                    SynthValue::Text(f.texture.as_bytes().to_vec()),
                ),
                (PROP_FILL_TILE, SynthValue::Real(f.tile)),
                (PROP_FILL_COLOR, SynthValue::Rgb(f.color)),
                (PROP_FILL_SURFACE, SynthValue::Int(f.surface as f32)),
            ];
            o.props
                .extend(more.into_iter().map(|(n, v)| SynthProp::dc(n, v)));
        }
        o
    }

    /// The record of a piece object; errors name what is wrong.
    pub fn of_object(o: &Object<'_>) -> Result<Piece, String> {
        let name = o
            .name()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        let get = |n: &str| {
            o.props
                .iter()
                .skip(3)
                .find(|p| p.name.eq_ignore_ascii_case(n.as_bytes()))
        };
        let text = |n: &str| -> String {
            match get(n).map(|p| p.value) {
                Some(crate::FieldValue::Text(t)) => String::from_utf8_lossy(t).into_owned(),
                _ => String::new(),
            }
        };
        let vec3 = |n: &str| -> Option<V> {
            match get(n).map(|p| p.value) {
                Some(crate::FieldValue::Vec3(v)) => Some(v),
                _ => None,
            }
        };
        let flag = |n: &str| -> bool {
            matches!(get(n).map(|p| p.value), Some(crate::FieldValue::Bool(true)))
        };
        let world = text(PROP_WORLD);
        if world.is_empty() {
            return Err(format!("piece `{name}` names no world"));
        }
        let offset = o.position().unwrap_or([0.0; 3]);
        if offset.iter().any(|x| !x.is_finite()) {
            return Err(format!("piece `{name}` has an offset that is not finite"));
        }
        let (Some(a), Some(b)) = (vec3(PROP_REGION_MIN), vec3(PROP_REGION_MAX)) else {
            return Err(format!("piece `{name}` has no region"));
        };
        let region = Aabb::new(a, b);
        if !region.is_valid() {
            return Err(format!("piece `{name}` has an empty region"));
        }
        let far = |b: &Aabb| {
            b.min
                .iter()
                .chain(b.max.iter())
                .any(|v| v.abs() > MAX_COORD)
        };
        if far(&region) || offset.iter().any(|v| v.abs() > MAX_COORD) {
            return Err(format!(
                "piece `{name}` lies beyond {MAX_COORD} units from the origin"
            ));
        }
        if (0..3).any(|i| region.max[i] - region.min[i] > MAX_REGION_EXTENT) {
            return Err(format!(
                "piece `{name}` has a region over {MAX_REGION_EXTENT} units across"
            ));
        }
        let cuts =
            parse_boxes(&text(PROP_CUTS)).map_err(|e| format!("piece `{name}` cuts: {e}"))?;
        if cuts.len() > MAX_CUTS {
            return Err(format!(
                "piece `{name}` has {} cuts (at most {MAX_CUTS})",
                cuts.len()
            ));
        }
        if cuts.iter().any(far) {
            return Err(format!(
                "piece `{name}` has a cut beyond {MAX_COORD} units from the origin"
            ));
        }
        let drop_spawns = parse_points(&text(PROP_DROP_SPAWNS))
            .map_err(|e| format!("piece `{name}` drop_spawns: {e}"))?;
        if drop_spawns.len() > MAX_DROP_SPAWNS {
            return Err(format!(
                "piece `{name}` leaves out {} spawns (at most {MAX_DROP_SPAWNS})",
                drop_spawns.len()
            ));
        }
        let sha256 = text(PROP_SHA256).to_ascii_lowercase();
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!(
                "piece `{name}` pins no SHA-256 digest of its world (the map was built without the ROM)"
            ));
        }
        let spawns_text = text(PROP_SPAWNS);
        let spawns = Spawns::parse(&spawns_text)
            .ok_or_else(|| format!("piece `{name}`: spawns `{spawns_text}` is not ffa or none"))?;
        let real = |n: &str| -> Option<f32> {
            match get(n).map(|p| p.value) {
                Some(crate::FieldValue::Real(v)) if v.is_finite() => Some(v),
                Some(crate::FieldValue::Int(v)) => Some(v as f32),
                _ => None,
            }
        };
        let fill = if flag(PROP_FILL) {
            let rgb = match get(PROP_FILL_COLOR).map(|p| p.value) {
                Some(crate::FieldValue::Rgb(c)) => c,
                _ => [128.0; 3],
            };
            let texture = text(PROP_FILL_TEXTURE);
            if texture.is_empty() {
                return Err(format!("piece `{name}` fills but names no fill texture"));
            }
            Some(Fill {
                y: real(PROP_FILL_Y).unwrap_or(0.0),
                texture,
                tile: real(PROP_FILL_TILE).filter(|t| *t > 0.0).unwrap_or(256.0),
                color: rgb,
                surface: real(PROP_FILL_SURFACE).unwrap_or(0.0).clamp(0.0, 65535.0) as u16,
            })
        } else {
            None
        };
        // a record written before MAP-120 has no `blockers`: it keeps them
        let blockers = match get(PROP_BLOCKERS).map(|p| p.value) {
            Some(crate::FieldValue::Bool(b)) => b,
            _ => true,
        };
        Ok(Piece {
            name,
            world,
            sha256,
            offset,
            region,
            cuts,
            spawns,
            items: flag(PROP_ITEMS),
            sky: flag(PROP_SKY),
            blockers,
            fill,
            min_light: real(PROP_MIN_LIGHT).unwrap_or(0.0).clamp(0.0, 255.0) as u8,
            drop_spawns,
        })
    }
}

/// Why a world could not be composed.
#[derive(Debug, Clone, PartialEq)]
pub enum ComposeError {
    /// The map's own file does not parse.
    Map(WorldError),
    /// A piece record is malformed, or two pieces overlap.
    Record(String),
    /// A piece's ROM world could not be read, does not match its digest or does not parse.
    Rom {
        piece: String,
        world: String,
        message: String,
    },
}

impl fmt::Display for ComposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComposeError::Map(e) => write!(f, "the map's world file: {e}"),
            ComposeError::Record(m) => write!(f, "{m}"),
            ComposeError::Rom {
                piece,
                world,
                message,
            } => write!(f, "piece `{piece}` ({world}): {message}"),
        }
    }
}

impl std::error::Error for ComposeError {}

/// Reads a world entry of the user's ROM by its path (`WORLDS\\NAME.DAT`): [`rom_reader`], or a test's own.
pub type RomRead<'a> = dyn FnMut(&str) -> Result<Vec<u8>, String> + 'a;
/// The size in texels of a texture of the user's ROM by the name a section carries (`None`: unknown): [`rom_texture_size`].
pub type TexSize<'a> = dyn FnMut(&str) -> Option<(u32, u32)> + 'a;

/// Counts of a composed world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ComposeStats {
    pub pieces: usize,
    pub level_polys: usize,
    pub level_nodes: usize,
    pub models: usize,
    pub objects: usize,
    pub blocks: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub player_spawns: usize,
    pub item_spawns: usize,
    pub grid_dims: [u32; 3],
}

/// A composed world.
#[derive(Debug, Clone, PartialEq)]
pub struct Composed {
    pub bytes: Vec<u8>,
    pub stats: ComposeStats,
}

/// The piece records of a parsed map, in object order.
pub fn pieces_of(w: &World<'_>) -> Result<Vec<Piece>, ComposeError> {
    w.objects
        .iter()
        .filter(|o| o.class_name.eq_ignore_ascii_case(PIECE_CLASS.as_bytes()))
        .map(|o| Piece::of_object(o).map_err(ComposeError::Record))
        .collect()
}

/// Whether the map's bytes hold any piece record (a cheap look at the object list only).
pub fn has_pieces(bytes: &[u8]) -> bool {
    match World::parse_with(bytes, crate::ParseOptions { render: false }) {
        Ok(w) => w
            .objects
            .iter()
            .any(|o| o.class_name.eq_ignore_ascii_case(PIECE_CLASS.as_bytes())),
        Err(_) => false,
    }
}

/// A reader of the user's ROM worlds: `WORLDS\NAME.DAT` (the first path part names the archive, `WORLDS.rez`).
pub fn rom_reader(rom: &std::path::Path) -> impl FnMut(&str) -> Result<Vec<u8>, String> {
    let vfs = dc_rez::RezVfs::new(rom);
    move |path: &str| {
        let norm = path.replace('/', "\\");
        let stem = norm
            .split('\\')
            .next()
            .filter(|s| !s.is_empty() && norm.contains('\\'))
            .ok_or_else(|| format!("`{path}` is not <archive>\\<entry>"))?;
        let archive = format!("{stem}.rez");
        match vfs.entry(&archive, &norm) {
            Ok(Some(b)) => Ok(b),
            Ok(None) => Err(format!("`{path}` is not in the ROM's {archive}")),
            Err(e) => Err(format!("reading `{path}`: {e}")),
        }
    }
}

/// The size of a texture of the user's ROM from its header (FMT_DTX.md: width and height as u16 at 4 and 6), read
/// without the rest of the file; the first part of the name names the archive, as for [`rom_reader`].
// SPEC: FMT_DTX.md §2 (header: width 0x04, height 0x06)
pub fn rom_texture_size(rom: &std::path::Path) -> impl FnMut(&str) -> Option<(u32, u32)> {
    let vfs = dc_rez::RezVfs::new(rom);
    move |path: &str| {
        let norm = path.replace('/', "\\");
        let stem = norm
            .split('\\')
            .next()
            .filter(|s| !s.is_empty() && norm.contains('\\'))?;
        let head = vfs.entry_head(&format!("{stem}.rez"), &norm, 8).ok()??;
        (head.len() >= 8).then(|| {
            (
                u16::from_le_bytes([head[4], head[5]]) as u32,
                u16::from_le_bytes([head[6], head[7]]) as u32,
            )
        })
    }
}

/// Composes when the bytes hold piece records, reading the pieces from the ROM at `rom`; the bytes unchanged
/// otherwise. The one call every loader makes.
pub fn compose_with_rom(
    bytes: Vec<u8>,
    rom: Option<&std::path::Path>,
) -> Result<Vec<u8>, ComposeError> {
    if !has_pieces(&bytes) {
        return Ok(bytes);
    }
    let Some(rom) = rom else {
        return Err(ComposeError::Record(
            "the map is built from pieces of the ROM's worlds, and no ROM is at hand".into(),
        ));
    };
    let mut read = rom_reader(rom);
    let mut size = rom_texture_size(rom);
    match compose_with(&bytes, &mut read, &mut size)? {
        Some(c) => Ok(c.bytes),
        None => Ok(bytes),
    }
}

/// What dc-mapkit and a mapper want to know of a ROM world before placing it.
#[derive(Debug, Clone, PartialEq)]
pub struct PieceInfo {
    pub extent: Aabb,
    /// The box around the player spawns, item spawns and blockers (where the map is played).
    pub play: Option<Aabb>,
    /// The median height of the player spawns.
    pub floor: Option<f32>,
    pub player_spawns: usize,
    pub item_spawns: usize,
}

/// [`PieceInfo`] of a parsed ROM world.
pub fn piece_info(w: &World<'_>) -> PieceInfo {
    let spawns: Vec<V> = w
        .objects_with_role(ObjectRole::PlayerSpawn)
        .filter_map(Object::position)
        .collect();
    let items: Vec<V> = w
        .objects_with_role(ObjectRole::ItemSpawn)
        .filter_map(Object::position)
        .collect();
    let mut pts: Vec<V> = spawns.iter().chain(items.iter()).copied().collect();
    for b in &w.blockers.polys {
        pts.extend(b.verts.iter().copied());
    }
    let mut ys: Vec<f32> = spawns.iter().map(|p| p[1]).collect();
    ys.sort_by(f32::total_cmp);
    PieceInfo {
        extent: Aabb::new(w.extent_min, w.extent_max),
        play: Aabb::of_points(pts.iter()),
        floor: ys.get(ys.len() / 2).copied(),
        player_spawns: spawns.len(),
        item_spawns: items.len(),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// copies

fn child(r: NodeRef) -> i32 {
    match r {
        NodeRef::Node(i) => i as i32,
        NodeRef::TermA => -1,
        NodeRef::TermB => -2,
    }
}

/// A BSP model moved by `t` under a new name.
fn moved_model(m: &BspModel<'_>, t: V, name: &[u8]) -> SynthModel {
    SynthModel {
        flags: m.flags,
        name: name.to_vec(),
        textures: m.texture_names.iter().map(|x| x.to_vec()).collect(),
        planes: m
            .planes
            .iter()
            .map(|p| (p.normal, p.dist + clip::dot(p.normal, t)))
            .collect(),
        surfaces: m
            .surfaces
            .iter()
            .map(|s| (s.flags, s.texture, s.texflags))
            .collect(),
        polys: m
            .polygons
            .iter()
            .map(|p| (p.surface, p.plane, m.polygon_points(p).to_vec()))
            .collect(),
        nodes: m
            .nodes
            .iter()
            .map(|n| (n.poly, n.reserved, child(n.front), child(n.back)))
            .collect(),
        points: m.points.iter().map(|&p| clip::add(p, t)).collect(),
        root: child(m.root),
        box_min: clip::add(m.box_min, t),
        box_max: clip::add(m.box_max, t),
        origin: clip::add(m.origin, t),
    }
}

fn value_of(p: &Property<'_>) -> SynthValue {
    match p.kind {
        PropKind::Text => SynthValue::Text(p.raw.to_vec()),
        k => SynthValue::Raw(k.code(), p.raw.to_vec()),
    }
}

/// An object as it is, every property byte for byte.
fn copy_object(o: &Object<'_>) -> SynthObject {
    SynthObject {
        class: o.class_name.to_vec(),
        props: o
            .props
            .iter()
            .map(|p| SynthProp {
                name: p.name.to_vec(),
                flags: p.flags,
                value: value_of(p),
            })
            .collect(),
    }
}

/// `<piece>~<name>`, cut to the engine's 63 bytes (a long name keeps its start and a short digest).
fn renamed(piece: &str, name: &[u8]) -> Vec<u8> {
    let mut out = piece.as_bytes().to_vec();
    out.push(RENAME_SEP as u8);
    out.extend_from_slice(name);
    if out.len() > MAX_NAME {
        let digest = dc_rom::sha256_hex(&out);
        out.truncate(MAX_NAME - 9);
        out.push(RENAME_SEP as u8);
        out.extend_from_slice(&digest.as_bytes()[..8]);
    }
    out
}

/// Replaces every word of a text that names a renamed object (ASCII case ignored), keeping the separators.
fn rename_words(text: &[u8], renames: &BTreeMap<Vec<u8>, Vec<u8>>) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() + 16);
    let mut changed = false;
    let mut word = Vec::new();
    let flush = |word: &mut Vec<u8>, out: &mut Vec<u8>, changed: &mut bool| {
        if word.is_empty() {
            return;
        }
        match renames.get(&word.to_ascii_lowercase()) {
            Some(n) => {
                out.extend_from_slice(n);
                *changed = true;
            }
            None => out.extend_from_slice(word),
        }
        word.clear();
    };
    for &b in text {
        if b.is_ascii_whitespace() || b == b';' || b == b',' || b == b'(' || b == b')' {
            flush(&mut word, &mut out, &mut changed);
            out.push(b);
        } else {
            word.push(b);
        }
    }
    flush(&mut word, &mut out, &mut changed);
    changed.then_some(out)
}

/// The kept-object point classes that are neither a role nor a brush: lights, sounds, sprites, effects,
/// explosions (identified by key: the ROM hash or the DeCombat name).
fn kept_point_class(class: &[u8]) -> bool {
    [
        names::CLASS_POINT_LIGHT,
        names::CLASS_DIRECTIONAL_LIGHT,
        names::CLASS_OBJECT_LIGHT,
        names::CLASS_SOUND,
        names::CLASS_SPRITE,
        names::CLASS_SPECIAL_FX,
        names::CLASS_EXPLOSION,
    ]
    .iter()
    .any(|k| k.matches(class))
}

/// The half-spaces of a convex brush moved by `t`, oriented to hold the mean of its points, each with its
/// polygon's surface; near-equal planes once.
fn brush_halfspaces(m: &BspModel<'_>, t: V) -> Vec<(HalfSpace, (u32, u16, u16))> {
    let n_pts = m.points.len().max(1) as f32;
    let mut mean = [0.0f32; 3];
    for p in &m.points {
        mean = clip::add(mean, *p);
    }
    let mean = clip::add(clip::scale(mean, 1.0 / n_pts), t);
    let mut out: Vec<(HalfSpace, (u32, u16, u16))> = Vec::new();
    for p in &m.polygons {
        if p.corner_count < 3 {
            continue;
        }
        let (Some(pl), Some(s)) = (
            m.planes.get(p.plane as usize),
            m.surfaces.get(p.surface as usize),
        ) else {
            continue;
        };
        let mut n = pl.normal;
        let mut d = pl.dist + clip::dot(pl.normal, t);
        if clip::dot(n, mean) - d > 0.0 {
            n = clip::scale(n, -1.0);
            d = -d;
        }
        let dup = out
            .iter()
            .any(|(h, _)| clip::length(clip::sub(h.n, n)) < 1.0e-4 && (h.d - d).abs() < 0.01);
        if !dup {
            out.push((HalfSpace { n, d }, (s.flags, s.texture, s.texflags)));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// the composer

/// Composes the map's world with its pieces; `Ok(None)` when it names none. `read` gets a piece's `world` path
/// and answers the entry's bytes from the user's ROM.
pub fn compose(map: &[u8], read: &mut RomRead<'_>) -> Result<Option<Composed>, ComposeError> {
    compose_with(map, read, &mut |_| None)
}

/// [`compose`] with the ROM's texture sizes, so a piece's placeholder faces take the texture of the wall they are
/// back to back with ([`render::retexture`], MAP-120).
pub fn compose_with(
    map: &[u8],
    read: &mut RomRead<'_>,
    size: &mut TexSize<'_>,
) -> Result<Option<Composed>, ComposeError> {
    let bw = World::parse(map).map_err(ComposeError::Map)?;
    let pieces = pieces_of(&bw)?;
    if pieces.is_empty() {
        return Ok(None);
    }
    if pieces.len() > MAX_PIECES {
        return Err(ComposeError::Record(format!(
            "the map has {} pieces (at most {MAX_PIECES})",
            pieces.len()
        )));
    }
    for (i, a) in pieces.iter().enumerate() {
        for b in &pieces[i + 1..] {
            if a.region.overlaps(&b.region) {
                return Err(ComposeError::Record(format!(
                    "pieces `{}` and `{}` overlap",
                    a.name, b.name
                )));
            }
            if a.name.eq_ignore_ascii_case(&b.name) {
                return Err(ComposeError::Record(format!(
                    "two pieces are named `{}`",
                    a.name
                )));
            }
        }
    }
    let sky_piece = pieces.iter().position(|p| p.sky);

    // the map's own level model: the one model no object names
    let named: BTreeSet<&[u8]> = bw.objects.iter().filter_map(|o| o.name()).collect();
    let levels: Vec<usize> = (0..bw.models.len())
        .filter(|&i| !named.contains(bw.models[i].name))
        .collect();
    let [level_i] = levels[..] else {
        return Err(ComposeError::Record(format!(
            "the map has {} level models (want 1)",
            levels.len()
        )));
    };
    let own_sky: BTreeSet<Vec<u8>> = if sky_piece.is_some() {
        bw.objects_with_role(ObjectRole::SkyWorldModel)
            .filter_map(|o| o.name().map(<[u8]>::to_vec))
            .collect()
    } else {
        BTreeSet::new()
    };

    let lm = &bw.models[level_i];
    let mut level = tree::Tree::new(lm.flags, lm.name, lm.origin);
    let own_root = level.add_plain(lm);
    let own_polys = level.own_polys();

    let mut brushes: Vec<SynthModel> = Vec::new();
    for (i, m) in bw.models.iter().enumerate() {
        if i == level_i || own_sky.contains(m.name) {
            continue;
        }
        brushes.push(moved_model(m, [0.0; 3], m.name));
    }
    let mut objects: Vec<SynthObject> = bw
        .objects
        .iter()
        .filter(|o| !o.class_name.eq_ignore_ascii_case(PIECE_CLASS.as_bytes()))
        .filter(|o| !o.name().is_some_and(|n| own_sky.contains(n)))
        .map(copy_object)
        .collect();
    let empty_render = crate::RenderData::default();
    let br = bw.render.as_ref().unwrap_or(&empty_render);
    let mut render = SynthRenderWorld {
        blocks: br
            .world
            .blocks
            .iter()
            .map(|b| render::moved_block(b, [0.0; 3], false, 0))
            .collect(),
        models: br
            .world
            .models
            .iter()
            .filter(|m| !own_sky.contains(m.name))
            .map(|m| {
                (
                    m.name.to_vec(),
                    render::moved_world(&m.world, [0.0; 3], false, 0),
                )
            })
            .collect(),
    };
    let grids: Vec<_> = br
        .grids
        .iter()
        .map(|g| (g.name.to_vec(), g.colour, g.offset, g.dims, g.data.to_vec()))
        .collect();
    let soup = |s: &crate::PolySoup| -> Vec<([f32; 3], f32, Vec<[f32; 3]>)> {
        s.polys
            .iter()
            .map(|p| (p.normal, p.dist, p.verts.clone()))
            .collect()
    };
    let mut blockers = soup(&bw.blockers);
    let particle_blockers = soup(&bw.particle_blockers);
    let mut bounds = Aabb::new(bw.extent_min, bw.extent_max);
    let all = Aabb {
        min: [-1.0e9; 3],
        max: [1.0e9; 3],
    };
    let own_grid = grid::Window::of(&bw.light_grid, &all);
    let mut sources: Vec<(Aabb, Vec<Aabb>, V, grid::Window)> = Vec::new();
    let mut roots: Vec<(Aabb, i32)> = Vec::new();
    let mut stats = ComposeStats {
        pieces: pieces.len(),
        ..ComposeStats::default()
    };

    // DC-DEFAULT: MAP-120 a texture at most PLACEHOLDER_TEXELS a side, once per name
    let mut tiny: BTreeMap<Vec<u8>, bool> = BTreeMap::new();
    let mut placeholder = |name: &[u8]| -> bool {
        if let Some(&v) = tiny.get(name) {
            return v;
        }
        let v = size(&String::from_utf8_lossy(name)).is_some_and(|(w, h)| {
            w <= render::PLACEHOLDER_TEXELS && h <= render::PLACEHOLDER_TEXELS
        });
        tiny.insert(name.to_vec(), v);
        v
    };
    let mut rom_bytes = 0usize;
    for (k, piece) in pieces.iter().enumerate() {
        let rom_err = |message: String| ComposeError::Rom {
            piece: piece.name.clone(),
            world: piece.world.clone(),
            message,
        };
        let bytes = read(&piece.world).map_err(rom_err)?;
        rom_bytes += bytes.len();
        if rom_bytes > MAX_COMPOSED_BYTES {
            return Err(ComposeError::Record(format!(
                "the pieces' worlds hold more than {MAX_COMPOSED_BYTES} bytes together"
            )));
        }
        let got = dc_rom::sha256_hex(&bytes);
        if got != piece.sha256 {
            return Err(rom_err(format!(
                "the ROM's world has SHA-256 {got}, the map was built against {}",
                piece.sha256
            )));
        }
        let rw = World::parse(&bytes).map_err(|e| rom_err(e.to_string()))?;
        let t = piece.offset;
        let (region, cuts) = (&piece.region, &piece.cuts[..]);
        let in_area = |p: V| region.contains(p) && !cuts.iter().any(|c| c.contains(p));

        // the level
        let rnamed: BTreeSet<&[u8]> = rw.objects.iter().filter_map(|o| o.name()).collect();
        let rlevels: Vec<usize> = (0..rw.models.len())
            .filter(|&i| !rnamed.contains(rw.models[i].name))
            .collect();
        let Some(&rl) = rlevels.iter().max_by_key(|&&i| rw.models[i].polygons.len()) else {
            return Err(rom_err("the world has no level model".into()));
        };
        let mut root = level.add_piece(&rw.models[rl], t, region, cuts, &own_polys);
        // the world's play area in the map's coordinates (its spawns, item spawns and blockers)
        let play = piece_info(&rw)
            .play
            .map(|b| b.translated(t))
            .unwrap_or(*region);
        // DC-DEFAULT: MAP-120 the fill floor: the region's columns outside the play area where the world has no
        // floor get the map's own ground, in the collision and drawn
        let mut fill_block = None;
        if let Some(f) = &piece.fill {
            let guarded = piece.blockers.then_some(&play);
            // what stands for ground: the level's solid floors, and any upward face of the world models (a sea
            // drawn as a world model is not paved over); volumes and the sky box do not count
            let mut cover: Vec<(&BspModel<'_>, bool)> = vec![(&rw.models[rl], true)];
            for o in &rw.objects {
                if matches!(
                    o.role,
                    Some(ObjectRole::KillVolume)
                        | Some(ObjectRole::HazardVolume)
                        | Some(ObjectRole::Ladder)
                        | Some(ObjectRole::SkyWorldModel)
                ) {
                    continue;
                }
                if let Some(m) = o
                    .name()
                    .and_then(|n| rw.models.iter().find(|m| m.name == n && m.is_dual()))
                {
                    cover.push((m, false));
                }
            }
            let rects = fill::rects(&cover, t, region, guarded, cuts, f.y, FILL_CELL);
            // the banks under the floors the region and the cuts cut off over the fill
            let caps = fill::caps(&rw.models[rl], t, region, cuts, f.y);
            if !rects.is_empty() || !caps.is_empty() {
                let tex = level.texture(f.texture.as_bytes());
                let surf = level.surface(tree::SURF_SOLID, tex, f.surface);
                let plane = level.plane([0.0, 1.0, 0.0], f.y - fill::SINK);
                for r in &rects {
                    let c = fill::corners(r, f.y);
                    let id = level.poly(surf, plane, &c);
                    root = level.insert(root, id, &c);
                }
                for c in &caps {
                    let plane = level.plane(c.normal, clip::dot(c.normal, c.corners[0]));
                    let id = level.poly(surf, plane, &c.corners);
                    root = level.insert(root, id, &c.corners);
                }
                fill_block = Some(fill::block(&rects, &caps, f));
            }
        }
        roots.push((*region, root));
        // DC-DEFAULT: MAP-120 an open piece keeps its kill volumes and hazards to its play area; a filled one only
        // what lies more than KILL_UNDER under its fill floor (the out-of-bounds killers round a world sit where the
        // fill now carries players, and on roofs; a pit with a floor of its own under the fill keeps its bottom)
        let kill_region = if piece.blockers {
            Some(*region)
        } else {
            let top = piece.fill.as_ref().map_or(region.max[1], |f| {
                (f.y - fill::SINK - KILL_UNDER).min(region.max[1])
            });
            let grown = Aabb {
                min: [
                    play.min[0] - OPEN_KILL_MARGIN,
                    region.min[1],
                    play.min[2] - OPEN_KILL_MARGIN,
                ],
                max: [
                    play.max[0] + OPEN_KILL_MARGIN,
                    top,
                    play.max[2] + OPEN_KILL_MARGIN,
                ],
            };
            grown.intersection(region)
        };

        // what to keep of the objects
        enum Kind {
            Spawn,
            Item,
            Point,
            Brush(usize),
            Clipped(usize),
            Volume(usize),
            Sky(usize),
        }
        let model_named = |n: &[u8]| rw.models.iter().position(|m| m.name == n && m.is_dual());
        let mut keep: Vec<(usize, Kind)> = Vec::new();
        for (oi, o) in rw.objects.iter().enumerate() {
            let pos = o.position().map(|p| clip::add(p, t));
            let model = o.name().and_then(model_named);
            let centre_in = |mi: usize| {
                let m = &rw.models[mi];
                in_area(clip::add(Aabb::new(m.box_min, m.box_max).centre(), t))
            };
            let kind = match o.role {
                Some(ObjectRole::PlayerSpawn) => {
                    // DC-DEFAULT: MAP-125 a spawn near a dropped point is not kept
                    let dropped = |q: V| {
                        piece.drop_spawns.iter().any(|d| {
                            let (dx, dz) = (d[0] - q[0], d[2] - q[2]);
                            dx * dx + dz * dz <= DROP_SPAWN_NEAR * DROP_SPAWN_NEAR
                                && (d[1] - q[1]).abs() <= DROP_SPAWN_NEAR_Y
                        })
                    };
                    (piece.spawns == Spawns::Ffa && pos.is_some_and(|q| in_area(q) && !dropped(q)))
                        .then_some(Kind::Spawn)
                }
                Some(ObjectRole::ItemSpawn) => {
                    (piece.items && pos.is_some_and(in_area)).then_some(Kind::Item)
                }
                Some(ObjectRole::SkyWorldModel) => {
                    if sky_piece == Some(k) {
                        model.map(Kind::Sky)
                    } else {
                        None
                    }
                }
                Some(ObjectRole::KillVolume) | Some(ObjectRole::HazardVolume) => {
                    model.map(Kind::Volume)
                }
                Some(ObjectRole::ModeController)
                | Some(ObjectRole::StartupCommand)
                | Some(ObjectRole::TriggerVolume)
                | Some(ObjectRole::CaptureFlag)
                | Some(ObjectRole::FlagZone)
                | Some(ObjectRole::BombSite)
                | Some(ObjectRole::WorldSettings)
                | Some(ObjectRole::CameraPoint) => None,
                Some(ObjectRole::PlainWorldModel) | Some(ObjectRole::Breakable) => {
                    model.and_then(|mi| {
                        let m = &rw.models[mi];
                        let mbox = Aabb::new(m.box_min, m.box_max).translated(t);
                        region.touches(&mbox).then_some(Kind::Clipped(mi))
                    })
                }
                _ => match model {
                    Some(mi) => centre_in(mi).then_some(Kind::Brush(mi)),
                    None => (kept_point_class(o.class_name) && pos.is_some_and(in_area))
                        .then_some(Kind::Point),
                },
            };
            if let Some(kind) = kind {
                keep.push((oi, kind));
            }
        }
        let mut renames: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        for (oi, kind) in &keep {
            if matches!(kind, Kind::Spawn | Kind::Item) {
                continue;
            }
            if let Some(n) = rw.objects[*oi].name() {
                renames.insert(n.to_ascii_lowercase(), renamed(&piece.name, n));
            }
        }
        let place = |o: &Object<'_>, name: &[u8]| -> SynthObject {
            let mut s = copy_object(o);
            for (i, p) in s.props.iter_mut().enumerate() {
                match i {
                    0 => p.value = SynthValue::Text(name.to_vec()),
                    1 => {
                        if let Some(pos) = o.position() {
                            p.value = SynthValue::Vec3(clip::add(pos, t));
                        }
                    }
                    _ => {
                        if let SynthValue::Text(txt) = &p.value {
                            if let Some(n) = rename_words(txt, &renames) {
                                p.value = SynthValue::Text(n);
                            }
                        }
                    }
                }
            }
            s
        };
        let rr = rw.render.as_ref().unwrap_or(&empty_render);
        let sub_named = |n: &[u8]| rr.world.models.iter().find(|m| m.name == n);
        let mut spawn_pts: Vec<V> = Vec::new();
        let mut n_items = 0usize;
        for (oi, kind) in keep {
            let o = &rw.objects[oi];
            let oname = o.name().unwrap_or_default();
            match kind {
                Kind::Spawn => {
                    let Some(p) = o.position().map(|p| clip::add(p, t)) else {
                        continue;
                    };
                    if spawn_pts
                        .iter()
                        .any(|q| clip::length(clip::sub(*q, p)) < SPAWN_MERGE)
                    {
                        continue;
                    }
                    spawn_pts.push(p);
                    let rot = o.rotation().unwrap_or([0.0; 3]);
                    let name = format!("{}{}spawn_{}", piece.name, RENAME_SEP, spawn_pts.len());
                    objects.push(SynthObject::dc(
                        names::CLASS_PLAYER_SPAWN.ours,
                        &name,
                        p,
                        rot,
                        &[
                            (names::PROP_TEAM_FILTER.ours, SynthValue::Text(Vec::new())),
                            (names::PROP_MODES.ours, SynthValue::Text(Vec::new())),
                        ],
                    ));
                }
                Kind::Item => {
                    let Some(p) = o.position().map(|p| clip::add(p, t)) else {
                        continue;
                    };
                    n_items += 1;
                    let name = format!("{}{}item_{}", piece.name, RENAME_SEP, n_items);
                    objects.push(SynthObject::dc(
                        names::CLASS_ITEM_SPAWN.ours,
                        &name,
                        p,
                        o.rotation().unwrap_or([0.0; 3]),
                        &[(names::PROP_MODES.ours, SynthValue::Text(Vec::new()))],
                    ));
                }
                Kind::Point => {
                    let name = renamed(&piece.name, oname);
                    objects.push(place(o, &name));
                }
                Kind::Brush(mi) => {
                    let name = renamed(&piece.name, oname);
                    let m = &rw.models[mi];
                    let sm = moved_model(m, t, &name);
                    bounds = bounds.union(&Aabb::new(sm.box_min, sm.box_max));
                    brushes.push(sm);
                    objects.push(place(o, &name));
                    if let Some(sub) = sub_named(m.name) {
                        render.models.push((
                            name.clone(),
                            render::moved_world(&sub.world, t, true, piece.min_light),
                        ));
                    }
                }
                Kind::Sky(mi) => {
                    // the sky box is drawn by the sky pass alone: it may stand anywhere, so it is moved into the
                    // region (under its ceiling) when the ROM put it outside, and the extents stay the pieces'
                    let m = &rw.models[mi];
                    let mbox = Aabb::new(m.box_min, m.box_max).translated(t);
                    let mut ts = t;
                    if !region.contains_box(&mbox) {
                        let c = mbox.centre();
                        let half = (mbox.max[1] - mbox.min[1]) * 0.5;
                        let want = [
                            (region.min[0] + region.max[0]) * 0.5,
                            region.max[1] - half - 16.0,
                            (region.min[2] + region.max[2]) * 0.5,
                        ];
                        ts = clip::add(t, clip::sub(want, c));
                    }
                    let name = renamed(&piece.name, oname);
                    brushes.push(moved_model(m, ts, &name));
                    let mut so = place(o, &name);
                    if let (Some(p), Some(slot)) = (o.position(), so.props.get_mut(1)) {
                        slot.value = SynthValue::Vec3(clip::add(p, ts));
                    }
                    objects.push(so);
                    if let Some(sub) = sub_named(m.name) {
                        render
                            .models
                            .push((name.clone(), render::moved_world(&sub.world, ts, true, 0)));
                    }
                }
                Kind::Clipped(mi) => {
                    let m = &rw.models[mi];
                    let mbox = Aabb::new(m.box_min, m.box_max).translated(t);
                    let name = renamed(&piece.name, oname);
                    let whole =
                        region.contains_box(&mbox) && !cuts.iter().any(|c| c.overlaps(&mbox));
                    let sm = if whole {
                        moved_model(m, t, &name)
                    } else {
                        let mut tr = tree::Tree::new(m.flags, &name, clip::add(m.origin, t));
                        let root = tr.add_clipped(m, t, region, cuts, false);
                        if !tr.has_faces() {
                            continue;
                        }
                        tr.into_synth(root)
                    };
                    bounds = bounds.union(&Aabb::new(sm.box_min, sm.box_max));
                    brushes.push(sm);
                    objects.push(place(o, &name));
                    if let Some(sub) = sub_named(m.name) {
                        let w = if whole {
                            render::moved_world(&sub.world, t, true, piece.min_light)
                        } else {
                            SynthRenderWorld {
                                blocks: sub
                                    .world
                                    .blocks
                                    .iter()
                                    .filter_map(|b| {
                                        render::clipped_block(b, t, region, cuts, piece.min_light)
                                    })
                                    .collect(),
                                models: Vec::new(),
                            }
                        };
                        render.models.push((name.clone(), w));
                    }
                }
                Kind::Volume(mi) => {
                    let m = &rw.models[mi];
                    let mbox = Aabb::new(m.box_min, m.box_max).translated(t);
                    let Some(region) = kill_region.as_ref() else {
                        continue;
                    };
                    if !region.touches(&mbox) {
                        continue;
                    }
                    let name = renamed(&piece.name, oname);
                    if region.contains_box(&mbox) && !cuts.iter().any(|c| c.overlaps(&mbox)) {
                        brushes.push(moved_model(m, t, &name));
                        objects.push(place(o, &name));
                        continue;
                    }
                    // clipped: the convex volume inside the region, outside the cuts
                    let hs = brush_halfspaces(m, t);
                    let default_surface = hs.first().map(|h| h.1).unwrap_or((0, 0, 0));
                    let mut start: Vec<HalfSpace> = hs.iter().map(|h| h.0).collect();
                    start.extend(clip::box_halfspaces(region));
                    let mut parts = vec![start];
                    for c in cuts {
                        let mut next = Vec::new();
                        for p in &parts {
                            next.extend(clip::volume_minus_box(p, &mbox, c));
                        }
                        parts = next;
                    }
                    let textures: Vec<Vec<u8>> =
                        m.texture_names.iter().map(|x| x.to_vec()).collect();
                    let mut j = 0usize;
                    for part in parts {
                        let faces = clip::volume_faces(&part, &mbox);
                        if faces.len() < 4 {
                            continue;
                        }
                        j += 1;
                        let pname = if j == 1 {
                            name.clone()
                        } else {
                            let mut n = name.clone();
                            n.truncate(MAX_NAME - 4);
                            n.extend_from_slice(format!("#{j}").as_bytes());
                            n
                        };
                        let fs: Vec<tree::BrushFace> = faces
                            .into_iter()
                            .map(|(hi, corners)| {
                                let s = hs.get(hi).map(|h| h.1).unwrap_or(default_surface);
                                let h = part[hi];
                                (s.0, s.1, s.2, h.n, h.d, corners)
                            })
                            .collect();
                        let pos = clip::add(o.position().unwrap_or(m.origin), t);
                        brushes.push(tree::convex_brush(&pname, m.flags, pos, &textures, &fs));
                        objects.push(place(o, &pname));
                    }
                }
            }
        }
        stats.player_spawns += spawn_pts.len();
        stats.item_spawns += n_items;

        // render: the top-level blocks, clipped
        for b in &rr.world.blocks {
            if let Some(mut sb) = render::clipped_block(b, t, region, cuts, piece.min_light) {
                render::retexture(&mut sb, &mut placeholder);
                if let Some(bb) = render::block_bounds(&sb) {
                    bounds = bounds.union(&bb);
                }
                render.blocks.push(sb);
            }
        }
        if let Some(b) = fill_block {
            if let Some(bb) = render::block_bounds(&b) {
                bounds = bounds.union(&bb);
            }
            render.blocks.push(b);
        }
        // blockers, clipped (an open piece has none)
        for p in rw.blockers.polys.iter().filter(|_| piece.blockers) {
            let corners: Vec<V> = p.verts.iter().map(|&v| clip::add(v, t)).collect();
            for part in clip::keep(&corners, region, cuts) {
                blockers.push((p.normal, p.dist + clip::dot(p.normal, t), part));
            }
        }
        sources.push((
            *region,
            cuts.to_vec(),
            t,
            grid::Window::of(&rw.light_grid, &region.translated(clip::scale(t, -1.0))),
        ));
    }

    let root = level.partition(&roots, own_root);
    if let Some(b) = level.bounds() {
        bounds = bounds.union(&b);
    }
    let m = [EXTENT_MARGIN; 3];
    let extent = Aabb {
        min: clip::sub(bounds.min, m),
        max: clip::add(bounds.max, m),
    };
    let srcs: Vec<grid::Source<'_>> = sources
        .iter()
        .map(|(region, cuts, offset, window)| grid::Source {
            region: *region,
            cuts,
            offset: *offset,
            window: window.clone(),
        })
        .collect();
    let (grid_origin, grid_cell, grid_dims, grid_rgb) = grid::compose(&extent, &own_grid, &srcs);
    let level_model = level.into_synth(root);
    stats.level_polys = level_model.polys.len();
    stats.level_nodes = level_model.nodes.len();
    let mut models = vec![level_model];
    models.extend(brushes);
    stats.models = models.len();
    stats.objects = objects.len();
    stats.blocks = render.blocks.len();
    stats.vertices = render.blocks.iter().map(|b| b.vertices.len()).sum();
    stats.triangles = render.blocks.iter().map(|b| b.triangles.len()).sum();
    stats.grid_dims = grid_dims;
    let w = SynthWorld {
        info: bw.info_text.to_vec(),
        extent_min: extent.min,
        extent_max: extent.max,
        world_offset: [0.0; 3],
        tree_shape: vec![false],
        models,
        objects,
        blind: bw.blind.iter().map(|b| (b.id, b.data.to_vec())).collect(),
        grid_origin,
        grid_cell,
        grid_dims,
        grid_rgb,
        blockers,
        particle_blockers,
        render,
        grids,
    };
    let bytes = w.build();
    if bytes.len() > MAX_COMPOSED_BYTES {
        return Err(ComposeError::Record(format!(
            "the composed world is {} bytes (at most {MAX_COMPOSED_BYTES})",
            bytes.len()
        )));
    }
    Ok(Some(Composed { bytes, stats }))
}
