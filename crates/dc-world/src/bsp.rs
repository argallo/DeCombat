//! World body (section 4): info text, extents, quadtree and the BSP model list.

use crate::cursor::{le_u32, le_vec3, Cur, Vec3};
use crate::error::{Result, Section, WorldError};

/// Region quadtree over the world extents.
// SPEC: FMT_DAT85.md §4.1 (engine+0xc0220, nodes engine+0xbfb20)
#[derive(Debug, Clone, PartialEq)]
pub struct Quadtree<'a> {
    pub min: Vec3,
    pub max: Vec3,
    /// Declared node count (`tree_nodes`); equals the nodes in `shape`.
    pub node_count: u32,
    /// `tree_reserved`; 0 in the ROM.
    pub reserved: u32,
    /// Pre-order shape bit stream, least significant bit first.
    pub shape: &'a [u8],
}

impl Quadtree<'_> {
    /// Whether pre-order node `i` has four children (bit set) or is a leaf.
    // SPEC: FMT_DAT85.md §4.1 (bit 1 = 4 children, bit 0 = leaf)
    pub fn is_split(&self, i: u32) -> Option<bool> {
        if i >= self.node_count {
            return None;
        }
        let byte = *self.shape.get((i / 8) as usize)?;
        Some(byte >> (i % 8) & 1 == 1)
    }
}

pub(crate) fn read_quadtree<'a>(c: &mut Cur<'a>) -> Result<Quadtree<'a>> {
    let saved = c.section;
    c.section = Section::Quadtree;
    // SPEC: FMT_DAT85.md §4.1 items 1-4
    let min = c.vec3()?;
    let max = c.vec3()?;
    let node_count = c.u32()?;
    let reserved = c.u32()?;
    // SPEC: FMT_DAT85.md §4.1 item 5: one byte per 8 nodes, read when the bit counter reaches 8. Each node
    // consumes one bit whatever its depth, so walking pre-order is a count of pending nodes.
    let start = c.pos();
    let mut pending: u64 = 1;
    let mut parsed: u64 = 0;
    let mut byte = 0u8;
    let mut bit = 8u32;
    while pending > 0 {
        if bit == 8 {
            byte = c.u8()?;
            bit = 0;
        }
        let split = byte >> bit & 1 == 1;
        bit += 1;
        pending -= 1;
        parsed += 1;
        if parsed > node_count as u64 {
            return Err(WorldError::QuadtreeShape {
                declared: node_count,
                parsed,
            });
        }
        if split {
            pending += 4;
        }
    }
    if parsed != node_count as u64 {
        return Err(WorldError::QuadtreeShape {
            declared: node_count,
            parsed,
        });
    }
    let shape = c.since(start);
    c.section = saved;
    Ok(Quadtree {
        min,
        max,
        node_count,
        reserved,
        shape,
    })
}

/// Reference from a BSP node to a child node or one of the two engine sentinels.
// SPEC: FMT_DAT85.md §5.2 (node_front/node_back, root_node: -1 / -2 sentinels)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRef {
    Node(u32),
    /// File value -1 (engine sentinel at VA 0x6ca7c8).
    // EVIDENCE: engine+0x26a7c8
    TermA,
    /// File value -2 (engine sentinel at VA 0x6ca7cc).
    // EVIDENCE: engine+0x26a7cc
    TermB,
}

impl NodeRef {
    fn decode(v: i32, n_nodes: u32, c: &Cur<'_>, what: &'static str) -> Result<Self> {
        match v {
            -1 => Ok(NodeRef::TermA),
            -2 => Ok(NodeRef::TermB),
            v if v >= 0 && (v as u32) < n_nodes => Ok(NodeRef::Node(v as u32)),
            v => Err(WorldError::IndexOutOfRange {
                section: Section::Bsp,
                what,
                at: c.pos(),
                index: v as i64,
                limit: n_nodes as u64,
            }),
        }
    }
}

/// Plane: unit normal and distance.
// SPEC: FMT_DAT85.md §5.2 (plane, 16 bytes)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub dist: f32,
}

/// Surface record.
// SPEC: FMT_DAT85.md §5.2 (surface, 8 bytes; read engine+0x3345d)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surface {
    pub flags: u32,
    /// Index into the model's texture names (OBSERVED `< n_textures`).
    pub texture: u16,
    /// The surface id (the footstep / impact surface row) of the polygons under this surface; a sky face
    /// (flag 0x10) carries [`SURFACE_SKY`].
    pub texflags: u16,
}

/// The surface id the client treats as "sky": a shot record or an own trace ending on a polygon with this id
/// plays no impact, lays no mark and makes no sound. The ROM's sky polygons carry it as their `texflags`, the
/// game server answers it for a shot stopped by a sky face (MAP-24) and dc-mapkit writes it on every sky face it
/// builds, so the three agree through this one constant (dc-client `fx.rs SURFACE_SKY` is the same number).
// OBSERVED: every polygon with surface flag 0x10 of the 21 ROM worlds (126 polygons) carries texflags 110, no
// other value (scan 2026-11-01, mapkit review C4)
// EVIDENCE: cshell+0x23a910 (a miss produces the impact record with surface 0x6e); DISASM cshell+0x2c8dbb
// (surface 0x6e skips the impact effect, mark, surface effect and sound unless the impact row asks for sky impacts)
// DC-DEFAULT: MAP-24
pub const SURFACE_SKY: u16 = 0x6e;

/// Polygon: surface, plane and a run of point indices in [`BspModel::poly_points`].
// SPEC: FMT_DAT85.md §5.2 (polygon, 8 + 4k bytes; read engine+0x334d0)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Polygon {
    pub surface: u32,
    pub plane: u32,
    pub first_point: u32,
    pub corner_count: u8,
}

/// BSP node.
// SPEC: FMT_DAT85.md §5.2 (node, 14 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BspNode {
    pub poly: u32,
    pub reserved: u16,
    pub front: NodeRef,
    pub back: NodeRef,
}

/// Leaf record (none in the ROM).
// SPEC: FMT_DAT85.md §5.2 (leaf)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leaf<'a> {
    /// `n == 0xFFFF`: a single index.
    Index(u16),
    /// `n` runs of `{ id, bytes }`.
    Runs(Vec<(u16, &'a [u8])>),
}

/// One world model: the collision/visibility BSP body.
// SPEC: FMT_DAT85.md §5, §5.2 (engine+0x33060)
#[derive(Debug, Clone, PartialEq)]
pub struct BspModel<'a> {
    /// `model_reserved`, read before the body.
    // SPEC: FMT_DAT85.md §5 item 1
    pub reserved: u32,
    pub flags: u32,
    /// Link key to the object with the same name.
    // SPEC: FMT_DAT85.md §5.2 item 2, §6.4
    pub name: &'a [u8],
    /// `n_poly_verts` as declared (OBSERVED: sum of polygon vertex counts).
    pub declared_poly_verts: u32,
    pub reserved_a: u32,
    pub reserved_b: u32,
    pub box_min: Vec3,
    pub box_max: Vec3,
    pub origin: Vec3,
    /// Texture names, read from the user's file at run time (never stored in the tree).
    pub texture_names: Vec<&'a [u8]>,
    pub leaves: Vec<Leaf<'a>>,
    pub planes: Vec<Plane>,
    pub surfaces: Vec<Surface>,
    pub polygons: Vec<Polygon>,
    pub poly_points: Vec<u32>,
    pub nodes: Vec<BspNode>,
    pub points: Vec<Vec3>,
    pub root: NodeRef,
    /// File offset of the body (after `model_reserved`).
    pub body_at: usize,
}

impl<'a> BspModel<'a> {
    /// Bit 1 of `bsp_flags`: the engine loads the body twice (two roles).
    // SPEC: FMT_DAT85.md §5.1 (test engine+0xcf60c)
    pub fn is_dual(&self) -> bool {
        self.flags & 2 != 0
    }

    /// Point indices of polygon `p`.
    pub fn polygon_points(&self, p: &Polygon) -> &[u32] {
        let s = p.first_point as usize;
        self.poly_points
            .get(s..s + p.corner_count as usize)
            .unwrap_or(&[])
    }

    /// Texture name of a surface, if its index is in range.
    pub fn surface_texture(&self, s: &Surface) -> Option<&'a [u8]> {
        self.texture_names.get(s.texture as usize).copied()
    }
}

fn oob(c: &Cur<'_>, what: &'static str, index: u64, limit: u32) -> WorldError {
    WorldError::IndexOutOfRange {
        section: Section::Bsp,
        what,
        at: c.pos(),
        index: index as i64,
        limit: limit as u64,
    }
}

pub(crate) fn read_bsp_body<'a>(c: &mut Cur<'a>, model: u32) -> Result<BspModel<'a>> {
    let saved = c.section;
    c.section = Section::Bsp;
    // SPEC: FMT_DAT85.md §5 item 1 (engine+0xcf5aa)
    let reserved = c.u32()?;
    let body_at = c.pos();
    // SPEC: FMT_DAT85.md §5.2 items 1-17 (engine+0x3308a .. 0x33182)
    let flags = c.u32()?;
    let name = c.lstr()?;
    let n_points = c.u32()?;
    let n_planes = c.u32()?;
    let n_surfaces = c.u32()?;
    let n_portals = c.u32()?;
    let n_polys = c.u32()?;
    let n_leaves = c.u32()?;
    let declared_poly_verts = c.u32()?;
    let reserved_a = c.u32()?;
    let reserved_b = c.u32()?;
    let n_nodes = c.u32()?;
    let box_min = c.vec3()?;
    let box_max = c.vec3()?;
    let origin = c.vec3()?;
    // SPEC: FMT_DAT85.md §5.2 item 6: portals must be 0 (engine+0x33155)
    if n_portals != 0 {
        return Err(WorldError::PortalsPresent {
            model,
            count: n_portals,
        });
    }
    let texname_bytes = c.u32()?;
    let n_textures = c.u32()?;
    // SPEC: FMT_DAT85.md §5.2 item 18: n_textures NUL-terminated names filling exactly texname_bytes
    let block_at = c.pos();
    let block = c.take(texname_bytes as usize)?;
    let mut texture_names = Vec::with_capacity((n_textures as usize).min(block.len()));
    let mut rest = block;
    for _ in 0..n_textures {
        let nul = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(WorldError::TextureNames {
                model,
                at: block_at,
            })?;
        texture_names.push(&rest[..nul]);
        rest = &rest[nul + 1..];
    }
    if !rest.is_empty() {
        return Err(WorldError::TextureNames {
            model,
            at: block_at,
        });
    }
    // SPEC: FMT_DAT85.md §5.2 item 19
    let counts = c.take_n(n_polys, 1)?;
    // SPEC: FMT_DAT85.md §5.2 item 20 (loop engine+0x333b5..0x33431)
    let mut leaves = Vec::with_capacity(c.cap(n_leaves, 2));
    for _ in 0..n_leaves {
        let n = c.u16()?;
        if n == 0xFFFF {
            leaves.push(Leaf::Index(c.u16()?));
        } else {
            let mut runs = Vec::with_capacity(c.cap(n as u32, 4));
            for _ in 0..n {
                let id = c.u16()?;
                let bytes = c.lstr()?;
                runs.push((id, bytes));
            }
            leaves.push(Leaf::Runs(runs));
        }
    }
    // SPEC: FMT_DAT85.md §5.2 item 21 (single read engine+0x33443)
    let raw = c.take_n(n_planes, 16)?;
    let planes: Vec<Plane> = raw
        .chunks_exact(16)
        .map(|r| Plane {
            normal: le_vec3(r, 0),
            dist: crate::cursor::le_f32(r, 12),
        })
        .collect();
    // SPEC: FMT_DAT85.md §5.2 item 22
    let raw = c.take_n(n_surfaces, 8)?;
    let surfaces: Vec<Surface> = raw
        .chunks_exact(8)
        .map(|r| Surface {
            flags: le_u32(r, 0),
            texture: u16::from_le_bytes([r[4], r[5]]),
            texflags: u16::from_le_bytes([r[6], r[7]]),
        })
        .collect();
    // SPEC: FMT_DAT85.md §5.2 item 23 with the bounds rules of engine+0x334d7 / 0x334e4 / 0x33527
    let mut polygons = Vec::with_capacity(c.cap(n_polys, 8));
    let mut poly_points: Vec<u32> = Vec::new();
    for &k in counts {
        let rec = c.take(8 + 4 * k as usize)?;
        let surface = le_u32(rec, 0);
        if surface >= n_surfaces {
            return Err(oob(c, "poly_surface", surface as u64, n_surfaces));
        }
        let plane = le_u32(rec, 4);
        if plane >= n_planes {
            return Err(oob(c, "poly_plane", plane as u64, n_planes));
        }
        let first_point = poly_points.len() as u32;
        for j in 0..k as usize {
            let p = le_u32(rec, 8 + 4 * j);
            if p >= n_points {
                return Err(oob(c, "poly_point", p as u64, n_points));
            }
            poly_points.push(p);
        }
        polygons.push(Polygon {
            surface,
            plane,
            first_point,
            corner_count: k,
        });
    }
    // SPEC: FMT_DAT85.md §5.2 item 24 (loop engine+0x33571..0x33615)
    let mut nodes = Vec::with_capacity(c.cap(n_nodes, 14));
    for _ in 0..n_nodes {
        let poly = c.u32()?;
        if poly >= n_polys {
            return Err(oob(c, "node_poly", poly as u64, n_polys));
        }
        let reserved = c.u16()?;
        let f = c.i32()?;
        let b = c.i32()?;
        let front = NodeRef::decode(f, n_nodes, c, "node_front")?;
        let back = NodeRef::decode(b, n_nodes, c, "node_back")?;
        nodes.push(BspNode {
            poly,
            reserved,
            front,
            back,
        });
    }
    // SPEC: FMT_DAT85.md §5.2 item 25 (single read engine+0x33646)
    let raw = c.take_n(n_points, 12)?;
    let points: Vec<Vec3> = raw.chunks_exact(12).map(|r| le_vec3(r, 0)).collect();
    // SPEC: FMT_DAT85.md §5.2 items 26-27
    let root_raw = c.i32()?;
    let root = NodeRef::decode(root_raw, n_nodes, c, "root_node")?;
    let trailing = c.u32()?;
    if trailing != 0 {
        return Err(WorldError::TrailingCount {
            model,
            value: trailing,
        });
    }
    c.section = saved;
    Ok(BspModel {
        reserved,
        flags,
        name,
        declared_poly_verts,
        reserved_a,
        reserved_b,
        box_min,
        box_max,
        origin,
        texture_names,
        leaves,
        planes,
        surfaces,
        polygons,
        poly_points,
        nodes,
        points,
        root,
        body_at,
    })
}
