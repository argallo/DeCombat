//! From geometry to the pieces of a version-85 world (MAPKIT_PLAN.md 5.3 steps 3-5): BSP models over a built
//! tree, render data (one block per render world, kind-1 sections grouped by texture, sky portals), the objects
//! in the `dc.*` vocabulary and the generated settings objects. `build.rs` assembles them into a
//! `dc_world::synth::SynthWorld`.
//!
//! Determinism (DC-DEFAULT: MAP-48): every table here is deduplicated through a `BTreeMap` over the bit patterns
//! of the values, sections are ordered by texture name, and the only transcendental functions (the sun's euler
//! angles) are own fixed-term series ([`atan2_det`], [`asin_det`]), so the same project gives the same bytes on
//! every platform.

use crate::geom::{Brush, Plane, Poly, V3};
use crate::project::{BrushRole, MapObject, Material, PropValue, TextureRef, WorldSettings};
use crate::vocab_names as v;
use dc_world::synth::{
    SynthBlock, SynthModel, SynthObject, SynthRenderWorld, SynthSection, SynthValue,
};
use dc_world::PropKind;
use std::collections::{BTreeMap, BTreeSet};

/// Name of the level model (the one model no object names, MAP-3). A map object may not carry this name.
pub const LEVEL_MODEL_NAME: &str = "level";
/// The world's info text (our own line; the engine's grammar for it is unspecified and nothing reads it).
pub const INFO_TEXT: &str = "decombat mapkit v1";
/// World extents = the geometry box grown by this margin on every side (DC-DEFAULT: MAP-43).
pub const EXTENT_MARGIN: f32 = 128.0;
/// `sky_inner` of the generated `dc.sky_model` (DC-DEFAULT: MAP-46).
pub const SKY_INNER: f32 = 0.9;
/// `hit_points` of a `dc.breakable` brush (the project has no field for it yet; DC-DEFAULT: MAP-46).
pub const BREAKABLE_HIT_POINTS: f32 = 100.0;
/// `bsp_flags` bit 1: a model an object names (FMT_DAT85.md §5.1, MAP-3).
pub const BSP_FLAG_BRUSH: u32 = 2;
/// Names of the generated settings objects (DC-DEFAULT: MAP-46).
pub const WORLD_SETTINGS_NAME: &str = "world_settings";
pub const GLOBAL_LIGHT_NAME: &str = "global_light";
/// Own DTX extension of an exported texture (MAP-45).
pub const DTX_EXT: &str = ".dtx";
/// The `texflags` (footstep surface id) every sky face is written with, whatever the sky material's
/// `surface_id` says: the client's impact guard recognises a sky hit by this id alone (DC-DEFAULT: MAP-43).
// EVIDENCE: the ROM's sky polygons carry texflags 110, the client's impact guard compares against 0x6e
// (dc-client fx.rs `SURFACE_SKY`; a ROM-gated scan of the 21 worlds: every 0x10 polygon has texflags 110)
// TODO(mapkit-fixes): switch to the shared `dc_world::SURFACE_SKY` once the engine fixer's constant lands.
pub const SURFACE_SKY: u16 = 0x6e;

// Property names of the brush and settings objects (dc_world::vocab; ours, lower-case).
const PROP_VISIBLE: &str = "visible";
const PROP_SOLID: &str = "solid";
const PROP_RAYHIT: &str = "rayhit";
const PROP_HIT_POINTS: &str = "hit_points";
const PROP_SKY_INDEX: &str = "sky_index";
const PROP_SKY_DIMS: &str = "sky_dims";
const PROP_SKY_INNER: &str = "sky_inner";
const PROP_FAR_Z: &str = "far_z";
const PROP_FOG_ENABLE: &str = "fog_enable";
const PROP_FOG_NEAR: &str = "fog_near";
const PROP_FOG_FAR: &str = "fog_far";
const PROP_FOG_COLOR: &str = "fog_color";
const PROP_SKY_FOG_ENABLE: &str = "sky_fog_enable";
const PROP_SKY_FOG_NEAR: &str = "sky_fog_near";
const PROP_SKY_FOG_FAR: &str = "sky_fog_far";
const PROP_TWO_SIDED: &str = "two_sided";
const PROP_TO_AMBIENT: &str = "to_ambient";

// ---------------------------------------------------------------------------------------------------------------
// texture names

/// The project-relative DTX path of an own PNG path (DC-DEFAULT: MAP-45): the extension of the LAST path
/// segment replaced by `.dtx`; a segment without an extension, or a dotfile (`.hidden`), keeps its whole name.
/// `assets/ground.png` -> `assets/ground.dtx`, `assets/v1.0/ground` -> `assets/v1.0/ground.dtx`,
/// `assets/.hidden` -> `assets/.hidden.dtx`. The ONE rule both the world's texture names ([`texture_name`])
/// and the file `export` writes (`export::dtx_path_of`) follow, so the client finds the file the world names.
pub fn dtx_rel_path(path: &str) -> String {
    let (dir, file) = match path.rfind('/') {
        Some(slash) => (&path[..=slash], &path[slash + 1..]),
        None => ("", path),
    };
    let stem = match file.rfind('.') {
        Some(dot) if dot > 0 => &file[..dot],
        _ => file,
    };
    format!("{dir}{stem}{DTX_EXT}")
}

/// The texture name written into the world for a material (DC-DEFAULT: MAP-45): a ROM path verbatim; an own
/// PNG as `@mod:<mod id>/<project path with .dtx>` (`assets/ground.png` -> `@mod:my_map/assets/ground.dtx`,
/// [`dtx_rel_path`]), the path `export` writes the converted DTX to and the client's mod asset layer resolves.
pub fn texture_name(m: &Material, mod_id: &str) -> Vec<u8> {
    match &m.texture {
        TextureRef::Rom(path) => path.as_bytes().to_vec(),
        TextureRef::Own(path) => format!("@mod:{mod_id}/{}", dtx_rel_path(path)).into_bytes(),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// BSP model

fn bits3(v: V3) -> [u32; 3] {
    let z = |x: f32| if x == 0.0 { 0.0f32 } else { x };
    [z(v.x).to_bits(), z(v.y).to_bits(), z(v.z).to_bits()]
}

/// A BSP model from polygons and the tree built over them: points, planes and surfaces deduplicated by bit
/// pattern, helper polygons appended after the input polygons (no corners, flags 0, texflags 0), nodes with the
/// `-2` / `-1` sentinels, the box from the corners.
// SPEC: FMT_DAT85.md §5.2 (body records), §6.4 (name = the object's name for a brush)
pub fn bsp_model(
    name: &str,
    polys: &[Poly],
    tree: &crate::bsp::BspTree,
    origin: V3,
    flags: u32,
    textures: &[Vec<u8>],
) -> SynthModel {
    let names: BTreeSet<&[u8]> = polys
        .iter()
        .filter_map(|p| textures.get(p.material).map(Vec::as_slice))
        .collect();
    let mut tex_list: Vec<Vec<u8>> = names.iter().map(|n| n.to_vec()).collect();
    if tex_list.is_empty() && !tree.helpers.is_empty() {
        tex_list.push(b"none".to_vec());
    }
    let tex_of = |mat: usize| -> u16 {
        textures
            .get(mat)
            .and_then(|n| tex_list.binary_search(n).ok())
            .unwrap_or(0) as u16
    };
    let mut points: Vec<[f32; 3]> = Vec::new();
    let mut point_ix: BTreeMap<[u32; 3], u32> = BTreeMap::new();
    let mut planes: Vec<([f32; 3], f32)> = Vec::new();
    let mut plane_ix: BTreeMap<[u32; 4], u32> = BTreeMap::new();
    let mut surfaces: Vec<(u32, u16, u16)> = Vec::new();
    let mut surface_ix: BTreeMap<(u32, u16, u16), u32> = BTreeMap::new();
    let mut out_polys: Vec<(u32, u32, Vec<u32>)> = Vec::new();
    let mut lo = V3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut hi = -lo;
    let mut plane_of = |pl: &Plane| -> u32 {
        let z = |x: f32| if x == 0.0 { 0.0f32 } else { x };
        let key = [
            z(pl.n.x).to_bits(),
            z(pl.n.y).to_bits(),
            z(pl.n.z).to_bits(),
            z(pl.d).to_bits(),
        ];
        *plane_ix.entry(key).or_insert_with(|| {
            planes.push((pl.n.to_array(), pl.d));
            planes.len() as u32 - 1
        })
    };
    let mut surface_of = |flags: u32, tex: u16, texflags: u16| -> u32 {
        *surface_ix.entry((flags, tex, texflags)).or_insert_with(|| {
            surfaces.push((flags, tex, texflags));
            surfaces.len() as u32 - 1
        })
    };
    for p in polys.iter().chain(tree.helpers.iter()) {
        let (flags, tex, texflags) = if p.corners.len() < 3 {
            (0, 0, 0)
        } else {
            (p.flags, tex_of(p.material), p.surface)
        };
        let s = surface_of(flags, tex, texflags);
        let pl = plane_of(&p.plane);
        let pts: Vec<u32> = p
            .corners
            .iter()
            .map(|c| {
                lo = lo.min(*c);
                hi = hi.max(*c);
                *point_ix.entry(bits3(*c)).or_insert_with(|| {
                    points.push(c.to_array());
                    points.len() as u32 - 1
                })
            })
            .collect();
        out_polys.push((s, pl, pts));
    }
    if points.is_empty() {
        lo = origin;
        hi = origin;
    }
    let nodes = tree
        .nodes
        .iter()
        .map(|n| (n.poly as u32, 0u16, n.neg, n.pos))
        .collect();
    SynthModel {
        flags,
        name: name.as_bytes().to_vec(),
        textures: tex_list,
        planes,
        surfaces,
        polys: out_polys,
        nodes,
        points,
        root: tree.root,
        box_min: lo.to_array(),
        box_max: hi.to_array(),
        origin: origin.to_array(),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// render data

/// Counts of one render world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderCounts {
    pub vertices: usize,
    pub triangles: usize,
    pub sections: usize,
}

/// The 68-byte render vertex: position, uv0, uv1 (0), ARGB colour, normal, tangent and binormal (0).
// SPEC: FMT_DAT85.md §10.2 item 6 (68 bytes; field split OBSERVED, RENDER_2009.md §5.2)
pub fn vertex_bytes(pos: V3, uv: [f32; 2], colour: u32, normal: V3) -> [u8; 68] {
    let mut o = [0u8; 68];
    let mut put = |at: usize, v: f32| o[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(0, pos.x);
    put(4, pos.y);
    put(8, pos.z);
    put(12, uv[0]);
    put(16, uv[1]);
    put(20, 0.0);
    put(24, 0.0);
    put(32, normal.x);
    put(36, normal.y);
    put(40, normal.z);
    o[28..32].copy_from_slice(&colour.to_le_bytes());
    o
}

/// What makes two vertices the same one: position, texture coordinates, colour and normal, rounded.
type VertexKey = ([u32; 3], [i32; 2], u32, [i32; 3]);

/// One render world of one block: sections grouped by texture name (kind 1, vertex-lit), one vertex per polygon
/// corner, fan triangles in the polygon's stored order (the order the renderer draws as front faces with
/// `FrontFace::Cw`; OBSERVED: the ROM's render triangles follow the same order as its BSP polygons,
/// `cross(v1 - v0, v2 - v1)` along the vertex normal for 404,633 of 405,142 triangles of six worlds),
/// `inside` reverses that order and the normal (the sky box seen from within). `portals` become the block's
/// sky portals (plane-polys, world space).
// SPEC: FMT_DAT85.md §10.1, §10.2, §10.4
pub fn render_world(
    polys: &[Poly],
    colours: &[Vec<u32>],
    textures: &[Vec<u8>],
    inside: bool,
    portals: &[Poly],
) -> (SynthRenderWorld, RenderCounts) {
    let mut groups: BTreeMap<&[u8], Vec<usize>> = BTreeMap::new();
    for (i, p) in polys.iter().enumerate() {
        if p.corners.len() < 3 {
            continue;
        }
        let name = textures.get(p.material).map_or(&b""[..], Vec::as_slice);
        groups.entry(name).or_default().push(i);
    }
    let mut block = SynthBlock::default();
    let mut lo = V3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut hi = -lo;
    let mut counts = RenderCounts::default();
    // a corner that is the same in every field is one vertex, a smooth model costs a third of the bytes; texture
    // coordinates and normals compare after rounding, each triangle works them out with its own floats (MAP-104)
    let mut seen: BTreeMap<VertexKey, u32> = BTreeMap::new();
    for (name, idxs) in groups {
        let mut tris = 0u32;
        for i in idxs {
            let p = &polys[i];
            let k = p.corners.len();
            let mut at = Vec::with_capacity(k);
            let n = if inside { -p.plane.n } else { p.plane.n };
            let order = |j: usize| if inside { k - 1 - j } else { j };
            for j in 0..k {
                let ci = order(j);
                let c = p.corners[ci];
                lo = lo.min(c);
                hi = hi.max(c);
                let colour = colours
                    .get(i)
                    .and_then(|cs| cs.get(ci))
                    .copied()
                    .unwrap_or(0xFF80_8080);
                let uv = p
                    .corner_uvs
                    .as_ref()
                    .and_then(|u| u.get(ci))
                    .copied()
                    .unwrap_or_else(|| p.uv(c));
                let n = p
                    .corner_normals
                    .as_ref()
                    .and_then(|ns| ns.get(ci))
                    .map_or(n, |m| if inside { -*m } else { *m });
                let round = |v: f32, scale: f32| (v * scale).round() as i32;
                let key = (
                    [c.x.to_bits(), c.y.to_bits(), c.z.to_bits()],
                    [round(uv[0], 8192.0), round(uv[1], 8192.0)],
                    colour,
                    [round(n.x, 1024.0), round(n.y, 1024.0), round(n.z, 1024.0)],
                );
                at.push(*seen.entry(key).or_insert_with(|| {
                    block.vertices.push(vertex_bytes(c, uv, colour, n));
                    block.vertices.len() as u32 - 1
                }));
            }
            for j in 1..k - 1 {
                block.triangles.push([at[0], at[j], at[j + 1], i as u32]);
                tris += 1;
            }
        }
        block.sections.push(SynthSection {
            textures: [name.to_vec(), Vec::new()],
            shader_kind: 1,
            triangles: tris,
            effect: Vec::new(),
            lightmap_w: 0,
            lightmap_h: 0,
            lightmap: Vec::new(),
        });
        counts.sections += 1;
        counts.triangles += tris as usize;
    }
    counts.vertices = block.vertices.len();
    if block.vertices.is_empty() {
        lo = V3::ZERO;
        hi = V3::ZERO;
    }
    block.center = ((lo + hi) * 0.5).to_array();
    block.half = ((hi - lo) * 0.5).to_array();
    for p in portals {
        block.sky_portals.push((
            p.corners.iter().map(|c| c.to_array()).collect(),
            p.plane.n.to_array(),
            p.plane.d,
        ));
    }
    (
        SynthRenderWorld {
            blocks: vec![block],
            models: Vec::new(),
        },
        counts,
    )
}

/// The render world of a lit map: for each lightmap page a kind-2 section with its pieces' triangles at their lightmap
/// coordinates, the same triangles again per texture as kind 4 (RENDER_2009.md 5.2), and the models vertex-lit as kind 1.
pub fn render_world_lit(
    polys: &[Poly],
    baked: &crate::lightmap::Baked,
    textures: &[Vec<u8>],
    portals: &[Poly],
) -> (SynthRenderWorld, RenderCounts) {
    let mut block = SynthBlock::default();
    let mut lo = V3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut hi = -lo;
    let mut counts = RenderCounts::default();
    let mut seen: BTreeMap<VertexKey, u32> = BTreeMap::new();
    let mut add = |c: V3, uv: [f32; 2], colour: u32, n: V3, block: &mut SynthBlock| -> u32 {
        lo = lo.min(c);
        hi = hi.max(c);
        let round = |v: f32, scale: f32| (v * scale).round() as i32;
        let key = (
            [c.x.to_bits(), c.y.to_bits(), c.z.to_bits()],
            [round(uv[0], 32768.0), round(uv[1], 32768.0)],
            colour,
            [round(n.x, 1024.0), round(n.y, 1024.0), round(n.z, 1024.0)],
        );
        *seen.entry(key).or_insert_with(|| {
            block.vertices.push(vertex_bytes(c, uv, colour, n));
            block.vertices.len() as u32 - 1
        })
    };
    const WHITE: u32 = 0xFFFF_FFFF;
    // the pages
    for (page, data) in baked.pages.iter().enumerate() {
        let mut tris = 0u32;
        for piece in baked.pieces.iter().filter(|p| p.page == page) {
            let corners = &piece.poly.corners;
            let at: Vec<u32> = corners
                .iter()
                .zip(&piece.lm_uv)
                .map(|(c, uv)| add(*c, *uv, WHITE, piece.poly.plane.n, &mut block))
                .collect();
            for j in 1..at.len().saturating_sub(1) {
                block.triangles.push([at[0], at[j], at[j + 1], 0]);
                tris += 1;
            }
        }
        block.sections.push(SynthSection {
            textures: [b"lightmap".to_vec(), Vec::new()],
            shader_kind: 2,
            triangles: tris,
            effect: Vec::new(),
            lightmap_w: crate::lightmap::PAGE,
            lightmap_h: crate::lightmap::PAGE,
            lightmap: data.clone(),
        });
        counts.sections += 1;
        counts.triangles += tris as usize;
    }
    // the same pieces with their textures
    let mut groups: BTreeMap<&[u8], Vec<usize>> = BTreeMap::new();
    for (i, piece) in baked.pieces.iter().enumerate() {
        let name = textures
            .get(piece.poly.material)
            .map_or(&b""[..], Vec::as_slice);
        groups.entry(name).or_default().push(i);
    }
    for (name, idxs) in groups {
        let mut tris = 0u32;
        for i in idxs {
            let p = &baked.pieces[i].poly;
            let at: Vec<u32> = p
                .corners
                .iter()
                .map(|c| add(*c, p.uv(*c), WHITE, p.plane.n, &mut block))
                .collect();
            for j in 1..at.len().saturating_sub(1) {
                block.triangles.push([at[0], at[j], at[j + 1], 0]);
                tris += 1;
            }
        }
        block.sections.push(SynthSection {
            textures: [name.to_vec(), Vec::new()],
            shader_kind: 4,
            triangles: tris,
            effect: Vec::new(),
            lightmap_w: 0,
            lightmap_h: 0,
            lightmap: Vec::new(),
        });
        counts.sections += 1;
        counts.triangles += tris as usize;
    }
    // the models
    let mut models: BTreeMap<&[u8], Vec<usize>> = BTreeMap::new();
    for (i, p) in polys.iter().enumerate() {
        if p.corner_normals.is_some() && p.corners.len() >= 3 {
            let name = textures.get(p.material).map_or(&b""[..], Vec::as_slice);
            models.entry(name).or_default().push(i);
        }
    }
    for (name, idxs) in models {
        let mut tris = 0u32;
        for i in idxs {
            let p = &polys[i];
            let colours = &baked.vertex_colours[i];
            let at: Vec<u32> = p
                .corners
                .iter()
                .enumerate()
                .map(|(j, c)| {
                    let uv = p
                        .corner_uvs
                        .as_ref()
                        .and_then(|u| u.get(j))
                        .copied()
                        .unwrap_or_else(|| p.uv(*c));
                    let n = p
                        .corner_normals
                        .as_ref()
                        .and_then(|m| m.get(j))
                        .copied()
                        .unwrap_or(p.plane.n);
                    add(
                        *c,
                        uv,
                        colours.get(j).copied().unwrap_or(0xFF80_8080),
                        n,
                        &mut block,
                    )
                })
                .collect();
            for j in 1..at.len().saturating_sub(1) {
                block.triangles.push([at[0], at[j], at[j + 1], i as u32]);
                tris += 1;
            }
        }
        block.sections.push(SynthSection {
            textures: [name.to_vec(), Vec::new()],
            shader_kind: 1,
            triangles: tris,
            effect: Vec::new(),
            lightmap_w: 0,
            lightmap_h: 0,
            lightmap: Vec::new(),
        });
        counts.sections += 1;
        counts.triangles += tris as usize;
    }
    counts.vertices = block.vertices.len();
    if block.vertices.is_empty() {
        lo = V3::ZERO;
        hi = V3::ZERO;
    }
    block.center = ((lo + hi) * 0.5).to_array();
    block.half = ((hi - lo) * 0.5).to_array();
    for p in portals {
        block.sky_portals.push((
            p.corners.iter().map(|c| c.to_array()).collect(),
            p.plane.n.to_array(),
            p.plane.d,
        ));
    }
    (
        SynthRenderWorld {
            blocks: vec![block],
            models: Vec::new(),
        },
        counts,
    )
}

/// Whether a brush role gets a nested render world (drawn), and whether it is drawn from inside.
pub fn brush_render(role: BrushRole) -> Option<bool> {
    match role {
        BrushRole::WorldModel | BrushRole::Breakable => Some(false),
        BrushRole::Sky => Some(true),
        BrushRole::KillVolume | BrushRole::Ladder | BrushRole::Blocker => None,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// objects

/// A project property as a file value, typed by the vocabulary (`number` and `sky_index` are ints, colours
/// rgb, everything else by its JSON kind; an unknown property keeps its JSON kind, numbers as reals).
pub fn synth_value(class: &str, key: &str, value: &PropValue) -> SynthValue {
    let kind = dc_world::vocab::class_named(class)
        .and_then(|c| c.prop(key))
        .map(|p| p.kind);
    match value {
        PropValue::Bool(b) => SynthValue::Bool(*b),
        PropValue::Num(n) => match kind {
            Some(PropKind::Int) => SynthValue::Int(*n as f32),
            Some(PropKind::Bits) => SynthValue::Bits(*n as f32),
            Some(PropKind::Bool) => SynthValue::Bool(*n != 0.0),
            _ => SynthValue::Real(*n as f32),
        },
        PropValue::Text(t) => SynthValue::Text(t.as_bytes().to_vec()),
        PropValue::Vec3(a) => match kind {
            Some(PropKind::Rgb) => SynthValue::Rgb(*a),
            _ => SynthValue::Vec3(*a),
        },
        PropValue::Rgb { rgb } => SynthValue::Rgb([rgb[0] as f32, rgb[1] as f32, rgb[2] as f32]),
    }
}

/// Degrees to radians by one multiplication (no transcendental).
pub fn radians(deg: f32) -> f32 {
    deg * (std::f32::consts::PI / 180.0)
}

/// The object's name in the file: its `name`, or its id when empty.
pub fn object_name(o: &MapObject) -> &str {
    if o.name.is_empty() {
        &o.id
    } else {
        &o.name
    }
}

/// A map object in the `dc.*` vocabulary: `name` / `pos` / `rot` (euler radians) then its props in key order.
// SPEC: FMT_DAT85.md §6.1 (leading properties); DC-DEFAULT: MAP-1, MAP-46
pub fn map_object(o: &MapObject) -> SynthObject {
    let props: Vec<(&str, SynthValue)> = o
        .props
        .iter()
        .map(|(k, val)| (k.as_str(), synth_value(&o.class, k, val)))
        .collect();
    SynthObject::dc(
        &o.class,
        object_name(o),
        o.pos,
        [radians(o.rot[0]), radians(o.rot[1]), radians(o.rot[2])],
        &props,
    )
}

/// The overview a map without its own `dc.end_camera` cuts to at the round result (DC-DEFAULT: MAP-101): over the
/// south edge of the area the player spawns cover, looking north and down at its centre, under the top of the world.
pub fn default_end_camera(p: &crate::project::Project, top: V3) -> SynthObject {
    let spawns: Vec<[f32; 3]> = p
        .objects
        .iter()
        .filter(|o| o.class == crate::vocab_names::CLASS_PLAYER_SPAWN)
        .map(|o| o.pos)
        .collect();
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for s in &spawns {
        for i in 0..3 {
            lo[i] = lo[i].min(s[i]);
            hi[i] = hi[i].max(s[i]);
        }
    }
    if spawns.is_empty() {
        lo = [0.0; 3];
        hi = [0.0; 3];
    }
    let half = ((hi[0] - lo[0]).max(hi[2] - lo[2]) / 2.0).max(256.0);
    let centre = [(lo[0] + hi[0]) / 2.0, lo[1], (lo[2] + hi[2]) / 2.0];
    let back = half * 1.1;
    let y = (centre[1] + half).min(top.y - 32.0);
    let down = (y - centre[1]).max(1.0).atan2(back);
    SynthObject::dc(
        crate::vocab_names::CLASS_END_CAMERA,
        "end_camera",
        [centre[0], y, centre[2] - back],
        [down, 0.0, 0.0],
        &[],
    )
}

/// Half extents of a brush's corners (the sky box's `sky_dims`).
pub fn brush_half(b: &Brush) -> V3 {
    let mut lo = V3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut hi = -lo;
    for p in &b.polys {
        for c in &p.corners {
            lo = lo.min(*c);
            hi = hi.max(*c);
        }
    }
    if lo.x.is_finite() {
        (hi - lo) * 0.5
    } else {
        V3::ZERO
    }
}

/// The object that names a brush model: class from the role, name = the brush (solid id), pos = the model origin,
/// rot 0 (MAP-3), the role's props (DC-DEFAULT: MAP-46): world models and breakables `visible` / `solid` /
/// `rayhit` true (breakables `hit_points` 100), the sky model `sky_index` 0, `sky_dims` = half its box and
/// `sky_inner` 0.9, kill volumes and ladders nothing (the kill is immediate, the ladder needs no property).
pub fn brush_object(b: &Brush) -> SynthObject {
    let shown = [
        (PROP_VISIBLE, SynthValue::Bool(true)),
        (PROP_SOLID, SynthValue::Bool(true)),
        (PROP_RAYHIT, SynthValue::Bool(true)),
    ];
    let props: Vec<(&str, SynthValue)> = match b.role {
        BrushRole::WorldModel => shown.to_vec(),
        BrushRole::Breakable => {
            let mut p = shown.to_vec();
            p.push((PROP_HIT_POINTS, SynthValue::Real(BREAKABLE_HIT_POINTS)));
            p
        }
        // a blocker never becomes a brush model (build sends its faces to the blocker section)
        BrushRole::KillVolume | BrushRole::Ladder | BrushRole::Blocker => Vec::new(),
        BrushRole::Sky => vec![
            (PROP_SKY_INDEX, SynthValue::Int(0.0)),
            (PROP_SKY_DIMS, SynthValue::Vec3(brush_half(b).to_array())),
            (PROP_SKY_INNER, SynthValue::Vec3([SKY_INNER; 3])),
        ],
    };
    SynthObject::dc(
        b.role.class(),
        &b.name,
        b.origin.to_array(),
        [0.0; 3],
        &props,
    )
}

/// The generated `dc.world_settings` (DC-DEFAULT: MAP-46): `far_z`, the fog (sky fog = the same range) and the
/// fog colour from the project settings, and `two_sided = true` when the project asks for both faces (MAP-118; off
/// writes nothing, so a map without it builds the bytes it built before).
pub fn world_settings_object(s: &WorldSettings, at: V3) -> SynthObject {
    let rgb = [
        s.fog_color[0] as f32,
        s.fog_color[1] as f32,
        s.fog_color[2] as f32,
    ];
    let mut props = vec![
        (PROP_FAR_Z, SynthValue::Real(s.far_z)),
        (PROP_FOG_ENABLE, SynthValue::Bool(s.fog_enable)),
        (PROP_FOG_NEAR, SynthValue::Real(s.fog_near)),
        (PROP_FOG_FAR, SynthValue::Real(s.fog_far)),
        (PROP_FOG_COLOR, SynthValue::Rgb(rgb)),
        (PROP_SKY_FOG_ENABLE, SynthValue::Bool(s.fog_enable)),
        (PROP_SKY_FOG_NEAR, SynthValue::Real(s.fog_near)),
        (PROP_SKY_FOG_FAR, SynthValue::Real(s.fog_far)),
    ];
    // DC-DEFAULT: MAP-118
    if s.two_sided {
        props.push((PROP_TWO_SIDED, SynthValue::Bool(true)));
    }
    SynthObject::dc(
        v::CLASS_WORLD_SETTINGS,
        WORLD_SETTINGS_NAME,
        at.to_array(),
        [0.0; 3],
        &props,
    )
}

/// The generated `dc.global_light` (DC-DEFAULT: MAP-46): `rot` turns +z onto `sun_dir` (the readers take the
/// direction as the rotated +z, `dc-dserver worldlight.rs`, `dc-render lights.rs`), `color` = the sun colour,
/// `bright` = `sun_bright`, `to_ambient` 0.
pub fn global_light_object(s: &WorldSettings, at: V3) -> SynthObject {
    let dir = V3::from_array(s.sun_dir)
        .normalized()
        .unwrap_or(V3::new(0.0, -1.0, 0.0));
    let rgb = [
        s.sun_color[0] as f32,
        s.sun_color[1] as f32,
        s.sun_color[2] as f32,
    ];
    SynthObject::dc(
        v::CLASS_GLOBAL_LIGHT,
        GLOBAL_LIGHT_NAME,
        at.to_array(),
        sun_euler(dir),
        &[
            (v::PROP_COLOR, SynthValue::Rgb(rgb)),
            (v::PROP_BRIGHT, SynthValue::Real(s.sun_bright)),
            (PROP_TO_AMBIENT, SynthValue::Real(0.0)),
        ],
    )
}

/// Euler angles (radians, about x then y, z = 0) whose rotation, composed as the readers compose it (about y,
/// then x, then z: `dc-dserver worldlight.rs euler_quat`), turns +z onto the unit direction `d`:
/// `rotated_z = (cos a sin b, -sin a, cos a cos b)` with `a = asin(-d.y)`, `b = atan2(d.x, d.z)`.
pub fn sun_euler(d: V3) -> [f32; 3] {
    let a = asin_det((-d.y as f64).clamp(-1.0, 1.0));
    let b = atan2_det(d.x as f64, d.z as f64);
    [a as f32, b as f32, 0.0]
}

/// `atan(t)` for `0 <= t <= 1` by a fixed-term series after one reduction (deterministic, no libm).
fn atan_unit(t: f64) -> f64 {
    let sqrt3 = 3.0f64.sqrt();
    // tan(pi/12) = 2 - sqrt(3): above it, atan(t) = pi/6 + atan((t sqrt3 - 1) / (t + sqrt3)), |arg| <= tan(pi/12)
    let (base, x) = if t > 2.0 - sqrt3 {
        (std::f64::consts::FRAC_PI_6, (t * sqrt3 - 1.0) / (t + sqrt3))
    } else {
        (0.0, t)
    };
    // x - x^3/3 + x^5/5 - ... up to x^25/25 (|x| <= 0.268: the next term is below 1e-16)
    let x2 = x * x;
    let mut s = 1.0 / 25.0;
    let mut k = 23.0;
    while k >= 1.0 {
        s = 1.0 / k - s * x2;
        k -= 2.0;
    }
    base + x * s
}

/// Deterministic `atan2(y, x)` in (-pi, pi] (0 for the origin).
pub fn atan2_det(y: f64, x: f64) -> f64 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let (ax, ay) = (x.abs(), y.abs());
    let mut r = if ay > ax {
        std::f64::consts::FRAC_PI_2 - atan_unit(ax / ay)
    } else {
        atan_unit(ay / ax)
    };
    if x < 0.0 {
        r = std::f64::consts::PI - r;
    }
    if y < 0.0 {
        r = -r;
    }
    r
}

/// Deterministic `asin(y)` for `-1 <= y <= 1`.
pub fn asin_det(y: f64) -> f64 {
    atan2_det(y, (1.0 - y * y).max(0.0).sqrt())
}

/// Rotated +z of the euler triple as the readers compose it (a check for the tests, not used by the build).
pub fn rotated_z(e: [f32; 3]) -> [f32; 3] {
    let (a, b) = (e[0] as f64, e[1] as f64);
    [
        (a.cos() * b.sin()) as f32,
        (-a.sin()) as f32,
        (a.cos() * b.cos()) as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsp::{self, BspOptions};
    use crate::geom::{solid_polys, Surface};
    use crate::project::{MaterialFlags, Solid};
    use std::collections::BTreeMap;

    #[test]
    fn texture_names_follow_the_mod_rule() {
        let own = Material {
            texture: TextureRef::Own("assets/tex/ground.png".into()),
            ..Default::default()
        };
        assert_eq!(
            texture_name(&own, "my_map"),
            b"@mod:my_map/assets/tex/ground.dtx".to_vec()
        );
        let noext = Material {
            texture: TextureRef::Own("assets/ground".into()),
            ..Default::default()
        };
        assert_eq!(
            texture_name(&noext, "m"),
            b"@mod:m/assets/ground.dtx".to_vec()
        );
        let dotdir = Material {
            texture: TextureRef::Own("assets/v1.0/ground".into()),
            ..Default::default()
        };
        assert_eq!(
            texture_name(&dotdir, "m"),
            b"@mod:m/assets/v1.0/ground.dtx".to_vec()
        );
        // the two inputs of the C19 finding: the world's name and the exported file follow ONE rule
        let dotfile = Material {
            texture: TextureRef::Own("assets/.hidden".into()),
            ..Default::default()
        };
        assert_eq!(
            texture_name(&dotfile, "m"),
            b"@mod:m/assets/.hidden.dtx".to_vec()
        );
        for (png, dtx) in [
            ("assets/v1.0/ground", "assets/v1.0/ground.dtx"),
            ("assets/.hidden", "assets/.hidden.dtx"),
            ("assets/ground.png", "assets/ground.dtx"),
            ("assets/noext", "assets/noext.dtx"),
            ("assets/tex/x.PNG", "assets/tex/x.dtx"),
            ("assets/a.b/c.d.png", "assets/a.b/c.d.dtx"),
            ("assets/.config.png", "assets/.config.dtx"),
        ] {
            assert_eq!(dtx_rel_path(png), dtx, "{png}");
            assert_eq!(
                crate::export::dtx_path_of(png),
                dtx,
                "export writes where the world looks: {png}"
            );
            let m = Material {
                texture: TextureRef::Own(png.into()),
                ..Default::default()
            };
            assert_eq!(texture_name(&m, "m"), format!("@mod:m/{dtx}").into_bytes());
        }
        assert_eq!(dtx_rel_path("ground.png"), "ground.dtx");
        assert_eq!(dtx_rel_path(".hidden"), ".hidden.dtx");
        let rom = Material {
            texture: TextureRef::Rom("some/where/x.dtx".into()),
            ..Default::default()
        };
        assert_eq!(texture_name(&rom, "m"), b"some/where/x.dtx".to_vec());
    }

    #[test]
    fn a_box_model_deduplicates_points_and_planes_and_keeps_helpers_apart() {
        let s = Solid {
            size: [10.0, 20.0, 30.0],
            ..Default::default()
        };
        let polys = solid_polys(&s, &|_| Surface::default()).unwrap();
        let textures = vec![b"t".to_vec()];
        let tree = bsp::build(
            &polys,
            &BspOptions {
                cell_polys: 16,
                helpers: false,
                max_depth: 8,
            },
        );
        let m = bsp_model(
            "solid_1",
            &polys,
            &tree,
            V3::new(1.0, 2.0, 3.0),
            2,
            &textures,
        );
        assert_eq!(m.points.len(), 8);
        assert_eq!(m.planes.len(), 6);
        assert_eq!(m.surfaces.len(), 1);
        assert_eq!(m.polys.len(), 6);
        assert_eq!(m.nodes.len(), 6);
        assert_eq!(m.box_min, [-5.0, -10.0, -15.0]);
        assert_eq!(m.box_max, [5.0, 10.0, 15.0]);
        assert_eq!(m.origin, [1.0, 2.0, 3.0]);
        assert_eq!(m.flags, 2);
        // helpers: one plane-only polygon with flags 0 after the real ones
        let mut t2 = tree.clone();
        t2.helpers.push(Poly {
            corners: Vec::new(),
            plane: Plane { n: V3::UP, d: 0.0 },
            material: 0,
            flags: 0,
            surface: 0,
            uv_scale: 0.0,
            uv_frame: None,
            corner_normals: None,
            corner_uvs: None,
            group: None,
        });
        let m2 = bsp_model("level", &polys, &t2, V3::ZERO, 0, &textures);
        assert_eq!(m2.polys.len(), 7);
        assert!(m2.polys[6].2.is_empty());
        let s6 = m2.surfaces[m2.polys[6].0 as usize];
        assert_eq!(s6, (0, 0, 0));
        assert_eq!(m2.planes.len(), 7);
    }

    #[test]
    fn render_world_groups_sections_by_texture_and_reverses_inside_faces() {
        let s = Solid {
            size: [10.0; 3],
            faces: BTreeMap::from([("top".to_string(), "b".to_string())]),
            ..Default::default()
        };
        let surf = |m: &str| Surface {
            material: if m == "b" { 1 } else { 0 },
            ..Default::default()
        };
        let polys = solid_polys(&s, &surf).unwrap();
        let textures = vec![b"tex_a".to_vec(), b"tex_b".to_vec()];
        let colours: Vec<Vec<u32>> = polys
            .iter()
            .map(|p| vec![0xFF11_2233; p.corners.len()])
            .collect();
        let (w, c) = render_world(&polys, &colours, &textures, false, &polys[..1]);
        assert_eq!(c.sections, 2);
        assert_eq!(c.vertices, 24);
        assert_eq!(c.triangles, 12);
        let b = &w.blocks[0];
        assert_eq!(b.sections[0].textures[0], b"tex_a");
        assert_eq!(b.sections[0].triangles, 10);
        assert_eq!(b.sections[1].textures[0], b"tex_b");
        assert_eq!(b.sections[1].triangles, 2);
        assert_eq!(b.sky_portals.len(), 1);
        assert_eq!(b.half, [5.0; 3]);
        // the vertex layout round-trips through the 68-byte split
        let v = &b.vertices[0];
        let colour = u32::from_le_bytes([v[28], v[29], v[30], v[31]]);
        assert_eq!(colour, 0xFF11_2233);
        // inside: the triangle order is reversed and the normal flipped
        let (wi, _) = render_world(&polys, &colours, &textures, true, &[]);
        let n_out = f32::from_le_bytes(b.vertices[0][36..40].try_into().unwrap());
        let n_in = f32::from_le_bytes(wi.blocks[0].vertices[0][36..40].try_into().unwrap());
        assert_eq!(n_out, -n_in);
    }

    #[test]
    fn deterministic_trig_matches_std_and_the_sun_rotation_round_trips() {
        let mut y = -1.0f64;
        while y <= 1.0 {
            assert!((asin_det(y) - y.asin()).abs() < 1e-12, "{y}");
            y += 0.0137;
        }
        for (a, b) in [
            (0.3, 0.7),
            (-0.3, 0.7),
            (0.3, -0.7),
            (-0.3, -0.7),
            (1.0, 0.0),
            (0.0, 1.0),
            (-1.0, 0.0),
            (0.0, -1.0),
            (5.0, 0.1),
            (0.1, 5.0),
        ] {
            assert!((atan2_det(a, b) - a.atan2(b)).abs() < 1e-12, "{a} {b}");
        }
        assert_eq!(atan2_det(0.0, 0.0), 0.0);
        for d in [
            [-0.4f32, -0.8, 0.3],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0],
            [0.5, -0.5, 0.5],
        ] {
            let d = V3::from_array(d).normalized().unwrap();
            let r = rotated_z(sun_euler(d));
            for k in 0..3 {
                assert!((r[k] - d.to_array()[k]).abs() < 1e-5, "{d:?} -> {r:?}");
            }
        }
    }

    #[test]
    fn objects_carry_the_vocabulary_kinds() {
        let mut props = BTreeMap::new();
        props.insert("number".to_string(), PropValue::Num(1.0));
        props.insert("extent".to_string(), PropValue::Vec3([1.0, 2.0, 3.0]));
        props.insert("modes".to_string(), PropValue::Text("TDM".into()));
        let o = MapObject {
            id: "bomb_site_3".into(),
            class: v::CLASS_BOMB_SITE.into(),
            rot: [0.0, 90.0, 0.0],
            props,
            ..Default::default()
        };
        let so = map_object(&o);
        assert_eq!(so.class, b"dc.bomb_site");
        assert_eq!(so.props[0].value, SynthValue::Text(b"bomb_site_3".to_vec()));
        assert_eq!(so.props[1].flags, 0x0010_0000);
        assert!(
            matches!(so.props[2].value, SynthValue::Euler([_, y, _, _]) if (y - std::f32::consts::FRAC_PI_2).abs() < 1e-6)
        );
        let by_name = |n: &[u8]| so.props.iter().find(|p| p.name == n).unwrap();
        assert_eq!(by_name(b"number").value, SynthValue::Int(1.0));
        assert_eq!(by_name(b"extent").flags, 0x4);
        assert_eq!(by_name(b"modes").value, SynthValue::Text(b"TDM".to_vec()));
        assert_eq!(
            synth_value(
                v::CLASS_GLOBAL_LIGHT,
                "color",
                &PropValue::Rgb { rgb: [1, 2, 3] }
            ),
            SynthValue::Rgb([1.0, 2.0, 3.0])
        );
        assert_eq!(
            synth_value("dc.nothing", "x", &PropValue::Num(2.5)),
            SynthValue::Real(2.5)
        );
        let sky = Solid {
            size: [100.0, 50.0, 100.0],
            pos: [10.0, 0.0, 0.0],
            brush: Some(BrushRole::Sky),
            ..Default::default()
        };
        let b = Brush {
            name: "solid_9".into(),
            role: BrushRole::Sky,
            origin: V3::new(10.0, 0.0, 0.0),
            polys: solid_polys(&sky, &|_| Surface {
                flags: MaterialFlags::default().solid as u32,
                ..Default::default()
            })
            .unwrap(),
        };
        let so = brush_object(&b);
        assert_eq!(so.class, b"dc.sky_model");
        assert_eq!(so.props[0].value, SynthValue::Text(b"solid_9".to_vec()));
        assert_eq!(so.props[1].value, SynthValue::Vec3([10.0, 0.0, 0.0]));
        assert!(so
            .props
            .iter()
            .any(|p| p.name == b"sky_dims" && p.value == SynthValue::Vec3([50.0, 25.0, 50.0])));
        let ws = world_settings_object(&WorldSettings::default(), V3::ZERO);
        assert!(ws.props.iter().any(|p| p.name == b"far_z"));
        // MAP-118: both faces only when asked, and then as a bool
        assert!(!ws.props.iter().any(|p| p.name == b"two_sided"));
        let both = WorldSettings {
            two_sided: true,
            ..Default::default()
        };
        let ws = world_settings_object(&both, V3::ZERO);
        assert!(ws
            .props
            .iter()
            .any(|p| p.name == b"two_sided" && p.value == SynthValue::Bool(true)));
        let gl = global_light_object(&WorldSettings::default(), V3::ZERO);
        assert_eq!(gl.class, b"dc.global_light");
    }
}
