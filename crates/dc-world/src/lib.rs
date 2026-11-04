//! `dc-world`: reader for the version-85 world files (`.dat`) of the 2009 client.
//!
//! Written only from `rust/specs/FMT_DAT85.md`. The user's file is borrowed (`World<'a>`); names, texture
//! paths and payloads are slices into it and are never copied into this tree. Every count is checked against
//! the bytes that remain before anything is allocated, so hostile input yields a [`WorldError`], not a panic.
//!
//! Object classes and properties are identified by hash (ROM worlds) or by DeCombat's own map vocabulary
//! (community maps built by dc-mapkit): [`names`] holds every key the readers compare against, [`vocab`] the
//! machine-readable class list (docs/MAPKIT_VOCAB.md), and [`synth`] writes a world in either naming.

mod bsp;
mod cursor;
mod error;
pub mod names;
mod objects;
mod render;
mod roles;
mod side;
pub mod synth;
pub mod vocab;

pub use bsp::{BspModel, BspNode, Leaf, NodeRef, Plane, Polygon, Quadtree, Surface, SURFACE_SKY};
pub use cursor::Vec3;
pub use error::{Result, Section, WorldError};
pub use names::Key;
pub use objects::{FieldValue, Object, PropKind, Property};
pub use render::{
    LightGroupGrid, LightPatch, PlanePoly, RenderBlock, RenderData, RenderLightGroup,
    RenderSection, RenderSubWorld, RenderTotals, RenderWorld, MAX_RENDER_DEPTH,
    RENDER_VERTEX_BYTES,
};
pub use roles::{hash16, hash16_hex, Geometry, ObjectRole, PropKey};
pub use side::{BlindBlob, BlockerPoly, LightGrid, PolySoup};

use cursor::Cur;

/// The only version the 2009 engine accepts.
// SPEC: FMT_DAT85.md §3 (check engine+0xce905)
pub const WORLD_VERSION: u32 = 85;

/// Header size; the world body starts here.
// SPEC: FMT_DAT85.md §3, §4 (body at 0x3c)
pub const HEADER_BYTES: usize = 0x3c;

/// File header: version and absolute section offsets.
// SPEC: FMT_DAT85.md §3 (engine+0xce8f0, 15 reads 0xce902..0xce9d1)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    pub objects_at: u32,
    pub blind_at: u32,
    pub lightgrid_at: u32,
    pub blockers_at: u32,
    pub pblockers_at: u32,
    pub render_at: u32,
    /// Read and dropped by the engine; zero in the ROM.
    pub reserved: [u32; 8],
}

impl Header {
    pub fn parse(data: &[u8]) -> Result<Header> {
        let mut c = Cur::at(data, 0, Section::Header);
        let version = c.u32()?;
        if version != WORLD_VERSION {
            return Err(WorldError::BadVersion(version));
        }
        let mut w = [0u32; 6];
        for v in &mut w {
            *v = c.u32()?;
        }
        let mut reserved = [0u32; 8];
        for v in &mut reserved {
            *v = c.u32()?;
        }
        let h = Header {
            version,
            objects_at: w[0],
            blind_at: w[1],
            lightgrid_at: w[2],
            blockers_at: w[3],
            pblockers_at: w[4],
            render_at: w[5],
            reserved,
        };
        for (s, off) in h.offsets() {
            if (off as usize) < HEADER_BYTES || off as usize > data.len() {
                return Err(WorldError::BadOffset {
                    section: s,
                    offset: off,
                    file_len: data.len(),
                });
            }
        }
        Ok(h)
    }

    /// The six section offsets in header order.
    pub fn offsets(&self) -> [(Section, u32); 6] {
        [
            (Section::Objects, self.objects_at),
            (Section::BlindData, self.blind_at),
            (Section::LightGrid, self.lightgrid_at),
            (Section::Blockers, self.blockers_at),
            (Section::ParticleBlockers, self.pblockers_at),
            (Section::Render, self.render_at),
        ]
    }
}

/// Byte range `[start, end)` one section occupied.
// SPEC: FMT_DAT85.md §3 (section offsets), §11 (boundary checks)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub section: Section,
    pub start: usize,
    pub end: usize,
}

/// What to read. The server needs everything but the render data (FMT_DAT85.md §12).
// SPEC: FMT_DAT85.md §12 (render data is client-only)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    pub render: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions { render: true }
    }
}

/// A parsed world file.
// SPEC: FMT_DAT85.md §3-§10 (one field per section)
#[derive(Debug, Clone, PartialEq)]
pub struct World<'a> {
    pub header: Header,
    /// World settings text (grammar not specified, FMT_DAT85.md §13 item 5).
    // SPEC: FMT_DAT85.md §4 items 1-2 (engine+0xcf45d, 0xcf499)
    pub info_text: &'a [u8],
    // SPEC: FMT_DAT85.md §4 items 3-5 (engine+0xcf4b1, 0xcf4be, 0xcf4cb)
    pub extent_min: Vec3,
    pub extent_max: Vec3,
    pub world_offset: Vec3,
    pub quadtree: Quadtree<'a>,
    pub models: Vec<BspModel<'a>>,
    pub objects: Vec<Object<'a>>,
    pub blind: Vec<BlindBlob<'a>>,
    pub light_grid: LightGrid<'a>,
    pub blockers: PolySoup,
    pub particle_blockers: PolySoup,
    /// `None` when skipped by [`ParseOptions`].
    pub render: Option<RenderData<'a>>,
    /// Where each section started and ended, in file order of the header.
    pub spans: Vec<Span>,
    pub file_len: usize,
}

fn start(data: &[u8], off: u32, s: Section) -> Cur<'_> {
    Cur::at(data, off as usize, s)
}

impl<'a> World<'a> {
    /// Parses every section, render data included.
    pub fn parse(data: &'a [u8]) -> Result<World<'a>> {
        Self::parse_with(data, ParseOptions::default())
    }

    /// Parses with options. Each section is read from its header offset, as the engine seeks to each one.
    pub fn parse_with(data: &'a [u8], opts: ParseOptions) -> Result<World<'a>> {
        let header = Header::parse(data)?;
        let mut spans = Vec::with_capacity(8);

        // SPEC: FMT_DAT85.md §4 (engine+0xcf3f0)
        let mut c = Cur::at(data, HEADER_BYTES, Section::WorldBody);
        let info_len = c.u32()?;
        let info_text = c.take(info_len as usize)?;
        let extent_min = c.vec3()?;
        let extent_max = c.vec3()?;
        let world_offset = c.vec3()?;
        let quadtree = bsp::read_quadtree(&mut c)?;
        // SPEC: FMT_DAT85.md §4 items 7-8 (engine+0xcf531, loop 0xcf561..0xcf6aa)
        let model_count = c.u32()?;
        let mut models = Vec::with_capacity(c.cap(model_count, 120));
        for i in 0..model_count {
            models.push(bsp::read_bsp_body(&mut c, i)?);
        }
        spans.push(Span {
            section: Section::WorldBody,
            start: HEADER_BYTES,
            end: c.pos(),
        });

        // SPEC: FMT_DAT85.md §6 (engine+0x93fd0, seek at 0x940c5)
        let mut c = start(data, header.objects_at, Section::Objects);
        let objects = objects::read_objects(&mut c)?;
        spans.push(Span {
            section: Section::Objects,
            start: header.objects_at as usize,
            end: c.pos(),
        });

        // SPEC: FMT_DAT85.md §7 (server seeks to blind_at at engine+0xa1545)
        let mut c = start(data, header.blind_at, Section::BlindData);
        let blind = side::read_blind(&mut c)?;
        spans.push(Span {
            section: Section::BlindData,
            start: header.blind_at as usize,
            end: c.pos(),
        });

        let mut c = start(data, header.lightgrid_at, Section::LightGrid);
        let light_grid = side::read_light_grid(&mut c)?;
        spans.push(Span {
            section: Section::LightGrid,
            start: header.lightgrid_at as usize,
            end: c.pos(),
        });

        let mut c = start(data, header.blockers_at, Section::Blockers);
        let blockers = side::read_soup(&mut c, Section::Blockers)?;
        spans.push(Span {
            section: Section::Blockers,
            start: header.blockers_at as usize,
            end: c.pos(),
        });

        let mut c = start(data, header.pblockers_at, Section::ParticleBlockers);
        let particle_blockers = side::read_soup(&mut c, Section::ParticleBlockers)?;
        spans.push(Span {
            section: Section::ParticleBlockers,
            start: header.pblockers_at as usize,
            end: c.pos(),
        });

        // SPEC: FMT_DAT85.md §10 (client seeks to render_at at engine+0xcb9dc)
        let render = if opts.render {
            let mut c = start(data, header.render_at, Section::Render);
            let r = render::read_render(&mut c)?;
            spans.push(Span {
                section: Section::Render,
                start: header.render_at as usize,
                end: c.pos(),
            });
            Some(r)
        } else {
            None
        };

        Ok(World {
            header,
            info_text,
            extent_min,
            extent_max,
            world_offset,
            quadtree,
            models,
            objects,
            blind,
            light_grid,
            blockers,
            particle_blockers,
            render,
            spans,
            file_len: data.len(),
        })
    }

    /// Checks the layout the ROM shows: each parsed section ends exactly where the next header offset
    /// begins, and the last parsed section ends at end of file (only when render data was read).
    // SPEC: FMT_DAT85.md §3 (OBSERVED tiling), §11
    pub fn check_layout(&self) -> Result<()> {
        for w in self.spans.windows(2) {
            if w[0].end != w[1].start {
                return Err(WorldError::Layout {
                    section: w[0].section,
                    end: w[0].end,
                    expected: w[1].start,
                });
            }
        }
        if let Some(last) = self.spans.last() {
            if self.render.is_some() && last.end != self.file_len {
                return Err(WorldError::Layout {
                    section: last.section,
                    end: last.end,
                    expected: self.file_len,
                });
            }
        }
        Ok(())
    }

    /// Objects with the given role (class identified by hash).
    pub fn objects_with_role(&self, role: ObjectRole) -> impl Iterator<Item = &Object<'a>> + '_ {
        self.objects.iter().filter(move |o| o.role == Some(role))
    }

    /// Objects whose class name equals `class` (bytes as read from the user's file).
    pub fn objects_of_class<'s>(
        &'s self,
        class: &'s [u8],
    ) -> impl Iterator<Item = &'s Object<'a>> + 's {
        self.objects.iter().filter(move |o| o.class_name == class)
    }

    /// The object whose name (property 0) equals `name`.
    pub fn object_named(&self, name: &[u8]) -> Option<&Object<'a>> {
        self.objects.iter().find(|o| o.name() == Some(name))
    }

    /// The BSP model whose name equals `name`.
    pub fn model_named(&self, name: &[u8]) -> Option<&BspModel<'a>> {
        self.models.iter().find(|m| m.name == name)
    }

    /// The BSP model linked to a brush-type object by name.
    // SPEC: FMT_DAT85.md §6.4
    pub fn brush_of(&self, obj: &Object<'a>) -> Option<&BspModel<'a>> {
        self.model_named(obj.name()?)
    }

    /// Distinct class names, sorted bytewise.
    pub fn class_names(&self) -> Vec<&'a [u8]> {
        let mut v: Vec<&'a [u8]> = self.objects.iter().map(|o| o.class_name).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Per-map counts as in FMT_DAT85.md §11.1.
    pub fn stats(&self) -> WorldStats {
        let mut s = WorldStats {
            bytes: self.file_len as u64,
            quadtree_nodes: self.quadtree.node_count as u64,
            models: self.models.len() as u64,
            objects: self.objects.len() as u64,
            classes: self.class_names().len() as u64,
            blind: self.blind.len() as u64,
            blockers: self.blockers.polys.len() as u64,
            particle_blockers: self.particle_blockers.polys.len() as u64,
            light_grid: self.light_grid.dims,
            properties: self.objects.iter().map(|o| o.props.len() as u64).sum(),
            ..WorldStats::default()
        };
        for m in &self.models {
            s.dual += m.is_dual() as u64;
            s.bsp_nodes += m.nodes.len() as u64;
            s.bsp_polys += m.polygons.len() as u64;
            s.points += m.points.len() as u64;
            s.planes += m.planes.len() as u64;
            s.surfaces += m.surfaces.len() as u64;
            s.textures += m.texture_names.len() as u64;
        }
        if let Some(r) = &self.render {
            s.render_top = r.world.level_totals();
            s.render = r.world.totals();
            s.lg_grids = r.grids.len() as u64;
        }
        s
    }
}

/// Counts for one map (see [`World::stats`]).
// SPEC: FMT_DAT85.md §11.1 (columns)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorldStats {
    pub bytes: u64,
    pub quadtree_nodes: u64,
    pub models: u64,
    pub dual: u64,
    pub bsp_nodes: u64,
    pub bsp_polys: u64,
    pub points: u64,
    pub planes: u64,
    pub surfaces: u64,
    pub textures: u64,
    pub objects: u64,
    pub properties: u64,
    pub classes: u64,
    pub blind: u64,
    pub blockers: u64,
    pub particle_blockers: u64,
    pub light_grid: [u32; 3],
    /// Render totals of the top-level render world only.
    pub render_top: RenderTotals,
    /// Render totals including every nested render model.
    pub render: RenderTotals,
    pub lg_grids: u64,
}
