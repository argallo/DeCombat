//! Render data (section 10, client only). Structure is parsed to the end of the file; the lightmap and
//! light-group patch payloads stay opaque (FMT_DAT85.md §13 item 4), as does the 68-byte vertex (item 2).

use crate::cursor::{le_u32, Cur, Vec3};
use crate::error::{Result, Section, WorldError};

/// Size of one render vertex on disk.
// SPEC: FMT_DAT85.md §10.2 item 6 (68 bytes, one read at engine+0x181d23)
pub const RENDER_VERTEX_BYTES: usize = 68;

/// Our safety limit for nested render worlds (the ROM nests one level; not an engine rule).
pub const MAX_RENDER_DEPTH: u32 = 8;

/// Textured section of a render block.
// SPEC: FMT_DAT85.md §10.2 (section)
#[derive(Debug, Clone, PartialEq)]
pub struct RenderSection<'a> {
    /// Two texture paths, read from the user's file at run time.
    pub textures: [&'a [u8]; 2],
    pub shader_kind: u8,
    pub triangles: u32,
    pub effect: &'a [u8],
    pub lightmap_w: u32,
    pub lightmap_h: u32,
    /// Lightmap payload, not decoded (open item).
    pub lightmap: &'a [u8],
}

/// Polygon with a plane: sky portals and occluders.
// SPEC: FMT_DAT85.md §10.4 (engine+0x17e6b0)
#[derive(Debug, Clone, PartialEq)]
pub struct PlanePoly {
    pub verts: Vec<Vec3>,
    pub normal: Vec3,
    pub dist: f32,
}

/// Patch of a light-group slot.
// SPEC: FMT_DAT85.md §10.3 (per patch: 4 x u32, u32 len, u8[len])
#[derive(Debug, Clone, PartialEq)]
pub struct LightPatch<'a> {
    pub words: [u32; 4],
    pub data: &'a [u8],
}

/// Render light group of a block.
// SPEC: FMT_DAT85.md §10.3 (engine+0x180990)
#[derive(Debug, Clone, PartialEq)]
pub struct RenderLightGroup<'a> {
    pub name: &'a [u8],
    pub colour: Vec3,
    pub data: &'a [u8],
    pub slots: Vec<Vec<LightPatch<'a>>>,
}

/// Render block.
// SPEC: FMT_DAT85.md §10.2 (engine+0x1818e0)
#[derive(Debug, Clone, PartialEq)]
pub struct RenderBlock<'a> {
    pub center: Vec3,
    pub half: Vec3,
    pub sections: Vec<RenderSection<'a>>,
    pub vertex_count: u32,
    /// `vertex_count * 68` bytes, field split unresolved.
    pub vertices: &'a [u8],
    pub triangle_count: u32,
    /// `triangle_count * 16` bytes: 3 vertex indices + 1 source index each.
    pub triangles: &'a [u8],
    pub sky_portals: Vec<PlanePoly>,
    pub occluders: Vec<(PlanePoly, u32)>,
    pub light_groups: Vec<RenderLightGroup<'a>>,
    pub child_mask: u8,
    /// Child block indices; a slot is meaningful only when its mask bit is set (engine+0x182062 reads both).
    pub children: [u32; 2],
}

impl RenderBlock<'_> {
    /// Triangle `i` as `[v0, v1, v2, source]`.
    pub fn triangle(&self, i: u32) -> Option<[u32; 4]> {
        let off = (i as usize).checked_mul(16)?;
        let r = self.triangles.get(off..off + 16)?;
        Some([le_u32(r, 0), le_u32(r, 4), le_u32(r, 8), le_u32(r, 12)])
    }
}

/// Render model: a named nested render world.
// SPEC: FMT_DAT85.md §10.1 item 4
#[derive(Debug, Clone, PartialEq)]
pub struct RenderSubWorld<'a> {
    pub name: &'a [u8],
    pub world: RenderWorld<'a>,
}

/// Render world (recursive).
// SPEC: FMT_DAT85.md §10.1 (engine+0x137610)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenderWorld<'a> {
    pub blocks: Vec<RenderBlock<'a>>,
    pub models: Vec<RenderSubWorld<'a>>,
}

/// Light-group grid after the renderer body.
// SPEC: FMT_DAT85.md §10.5 (engine+0xcbed0, 0x4f6a0)
#[derive(Debug, Clone, PartialEq)]
pub struct LightGroupGrid<'a> {
    pub name: &'a [u8],
    pub colour: Vec3,
    pub offset: [i32; 3],
    pub dims: [u32; 3],
    pub data: &'a [u8],
}

/// Whole render section.
// SPEC: FMT_DAT85.md §10 (engine+0xcb990 -> 0xccae0)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenderData<'a> {
    pub world: RenderWorld<'a>,
    pub grids: Vec<LightGroupGrid<'a>>,
}

/// Totals over a render world and every nested level.
// SPEC: FMT_DAT85.md §11.1 (render columns)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderTotals {
    pub blocks: u64,
    pub vertices: u64,
    pub triangles: u64,
    pub sections: u64,
    pub lightmap_bytes: u64,
    pub sky_portals: u64,
    pub occluders: u64,
    pub light_groups: u64,
    pub models: u64,
}

impl RenderWorld<'_> {
    pub fn totals(&self) -> RenderTotals {
        let mut t = RenderTotals::default();
        self.add_totals(&mut t);
        t
    }

    /// Totals over this level's own blocks only (nested render models excluded).
    pub fn level_totals(&self) -> RenderTotals {
        let mut t = RenderTotals::default();
        self.add_blocks(&mut t);
        t.models = self.models.len() as u64;
        t
    }

    fn add_totals(&self, t: &mut RenderTotals) {
        self.add_blocks(t);
        t.models += self.models.len() as u64;
        for m in &self.models {
            m.world.add_totals(t);
        }
    }

    fn add_blocks(&self, t: &mut RenderTotals) {
        for b in &self.blocks {
            t.blocks += 1;
            t.vertices += b.vertex_count as u64;
            t.triangles += b.triangle_count as u64;
            t.sections += b.sections.len() as u64;
            t.lightmap_bytes += b
                .sections
                .iter()
                .map(|s| s.lightmap.len() as u64)
                .sum::<u64>();
            t.sky_portals += b.sky_portals.len() as u64;
            t.occluders += b.occluders.len() as u64;
            t.light_groups += b.light_groups.len() as u64;
        }
    }
}

fn read_plane_poly(c: &mut Cur<'_>) -> Result<PlanePoly> {
    // SPEC: FMT_DAT85.md §10.4 (engine+0x17e6c5, 0x17e71e, 0x17e7c5, 0x17e7d2)
    let n = c.u8()?;
    let raw = c.take_n(n as u32, 12)?;
    let verts = raw
        .chunks_exact(12)
        .map(|r| crate::cursor::le_vec3(r, 0))
        .collect();
    let normal = c.vec3()?;
    let dist = c.f32()?;
    Ok(PlanePoly {
        verts,
        normal,
        dist,
    })
}

fn read_light_group<'a>(c: &mut Cur<'a>) -> Result<RenderLightGroup<'a>> {
    // SPEC: FMT_DAT85.md §10.3 (engine+0x1809bb .. 0x180b8f)
    let name = c.lstr()?;
    let colour = c.vec3()?;
    let len = c.u32()?;
    let data = c.take(len as usize)?;
    let n_slots = c.u32()?;
    let mut slots = Vec::with_capacity(c.cap(n_slots, 4));
    for _ in 0..n_slots {
        let n_patches = c.u32()?;
        let mut patches = Vec::with_capacity(c.cap(n_patches, 20));
        for _ in 0..n_patches {
            let words = [c.u32()?, c.u32()?, c.u32()?, c.u32()?];
            let len = c.u32()?;
            let data = c.take(len as usize)?;
            patches.push(LightPatch { words, data });
        }
        slots.push(patches);
    }
    Ok(RenderLightGroup {
        name,
        colour,
        data,
        slots,
    })
}

fn read_block<'a>(c: &mut Cur<'a>) -> Result<RenderBlock<'a>> {
    // SPEC: FMT_DAT85.md §10.2 items 1-3 (engine+0x181933, 0x181940, 0x1819ae)
    let center = c.vec3()?;
    let half = c.vec3()?;
    let n_sections = c.u32()?;
    let mut sections = Vec::with_capacity(c.cap(n_sections, 23));
    for _ in 0..n_sections {
        // SPEC: FMT_DAT85.md §10.2 section items 1-7 (engine+0x181a0a .. 0x181ca5)
        let t0 = c.lstr()?;
        let t1 = c.lstr()?;
        let shader_kind = c.u8()?;
        let triangles = c.u32()?;
        let effect = c.lstr()?;
        let lightmap_w = c.u32()?;
        let lightmap_h = c.u32()?;
        let len = c.u32()?;
        let lightmap = c.take(len as usize)?;
        sections.push(RenderSection {
            textures: [t0, t1],
            shader_kind,
            triangles,
            effect,
            lightmap_w,
            lightmap_h,
            lightmap,
        });
    }
    // SPEC: FMT_DAT85.md §10.2 items 5-8 (engine+0x181cfb, 0x181d23, 0x181d33, 0x181d8b..0x181df7)
    let vertex_count = c.u32()?;
    let vertices = c.take_n(vertex_count, RENDER_VERTEX_BYTES)?;
    let triangle_count = c.u32()?;
    let tri_at = c.pos();
    let triangles = c.take_n(triangle_count, 16)?;
    for (i, r) in triangles.chunks_exact(16).enumerate() {
        for k in 0..3 {
            let v = le_u32(r, 4 * k);
            if v >= vertex_count {
                return Err(WorldError::IndexOutOfRange {
                    section: Section::Render,
                    what: "triangle vertex",
                    at: tri_at + 16 * i + 4 * k,
                    index: v as i64,
                    limit: vertex_count as u64,
                });
            }
        }
    }
    // SPEC: FMT_DAT85.md §10.2 items 9-11 (engine+0x181e8d, 0x181f1f / 0x181f82, 0x181fd3)
    let n = c.u32()?;
    let mut sky_portals = Vec::with_capacity(c.cap(n, 17));
    for _ in 0..n {
        sky_portals.push(read_plane_poly(c)?);
    }
    let n = c.u32()?;
    let mut occluders = Vec::with_capacity(c.cap(n, 21));
    for _ in 0..n {
        let p = read_plane_poly(c)?;
        let id = c.u32()?;
        occluders.push((p, id));
    }
    let n = c.u32()?;
    let mut light_groups = Vec::with_capacity(c.cap(n, 22));
    for _ in 0..n {
        light_groups.push(read_light_group(c)?);
    }
    // SPEC: FMT_DAT85.md §10.2 items 12-13 (engine+0x182047, 0x182062: both slots always read)
    let child_mask = c.u8()?;
    let children = [c.u32()?, c.u32()?];
    Ok(RenderBlock {
        center,
        half,
        sections,
        vertex_count,
        vertices,
        triangle_count,
        triangles,
        sky_portals,
        occluders,
        light_groups,
        child_mask,
        children,
    })
}

fn read_world<'a>(c: &mut Cur<'a>, depth: u32) -> Result<RenderWorld<'a>> {
    if depth > MAX_RENDER_DEPTH {
        return Err(WorldError::NestingTooDeep { at: c.pos(), depth });
    }
    // SPEC: FMT_DAT85.md §10.1 items 1-4 (engine+0x137649, 0x1376cd, 0x137722, 0x137793, 0x1377cb)
    let n_blocks = c.u32()?;
    let mut blocks = Vec::with_capacity(c.cap(n_blocks, 50));
    for _ in 0..n_blocks {
        blocks.push(read_block(c)?);
    }
    let n_models = c.u32()?;
    let mut models = Vec::with_capacity(c.cap(n_models, 10));
    for _ in 0..n_models {
        let name = c.lstr()?;
        let world = read_world(c, depth + 1)?;
        models.push(RenderSubWorld { name, world });
    }
    Ok(RenderWorld { blocks, models })
}

pub(crate) fn read_render<'a>(c: &mut Cur<'a>) -> Result<RenderData<'a>> {
    c.section = Section::Render;
    let world = read_world(c, 0)?;
    c.section = Section::LightGroupGrids;
    // SPEC: FMT_DAT85.md §10.5 (engine+0xccb3a; per grid 0xcbee4, 0xcbf00, 0xcbf2d, 0x4f786, 0x4f794, 0x4f80c)
    let n = c.u32()?;
    let mut grids = Vec::with_capacity(c.cap(n, 38));
    for _ in 0..n {
        let name = c.lstr()?;
        let colour = c.vec3()?;
        let offset = [c.i32()?, c.i32()?, c.i32()?];
        let dims = [c.u32()?, c.u32()?, c.u32()?];
        let cells = dims
            .iter()
            .try_fold(1u64, |a, &d| a.checked_mul(d as u64))
            .and_then(|t| u32::try_from(t).ok());
        let cells = cells.ok_or(WorldError::CountOverflow {
            section: Section::LightGroupGrids,
            at: c.pos(),
            count: u64::MAX,
        })?;
        let data = c.take_n(cells, 1)?;
        grids.push(LightGroupGrid {
            name,
            colour,
            offset,
            dims,
            data,
        });
    }
    Ok(RenderData { world, grids })
}
