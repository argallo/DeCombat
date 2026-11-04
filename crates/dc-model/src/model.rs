//! In-memory model. Field names are ours; `todo_*` fields are values the spec has not identified.

use crate::vertex::{VertexLayout, VertexStream};
use crate::RawStr;

/// `file_kind` of a model file.
// SPEC: FMT_LTB.md §2
pub const KIND_MODEL: u8 = 1;
/// `file_kind` of the render-style files that share the extension (out of scope).
pub const KIND_RENDER_STYLE: u8 = 5;
/// `file_kind` of the third container type that shares the extension (rejected by the model loader).
pub const KIND_OTHER9: u8 = 9;
/// The only accepted `container_rev`.
// SPEC: FMT_LTB.md §2 (cmp word [hdr+2],9 in all three render-object readers)
pub const CONTAINER_REV: u16 = 9;
/// Lowest `model_rev` the engine loads. The ROM holds revisions 23 and 25.
// SPEC: FMT_LTB.md §2, §12
pub const MIN_MODEL_REV: u32 = 23;
/// At most this many pieces.
// SPEC: FMT_LTB.md §4 step 6 (piece_count < 0x41)
pub const MAX_PIECES: u32 = 64;
/// Child-model slots (including the model itself) must lie in 1..=32.
// SPEC: FMT_LTB.md §4 step 11
pub const MAX_CHILD_SLOTS: u32 = 32;
/// Size of one (discarded) box record.
// SPEC: FMT_LTB.md §4 step 5 (0x44-byte records)
pub const BOX_RECORD_LEN: usize = 0x44;
/// Position decode factor of the i16 position encoding.
// SPEC: FMT_LTB.md §10.3 (factor at VA 0x67a678 = 0.0625); EVIDENCE: engine+0x626c0
pub const POS_I16_SCALE: f32 = 0.0625;
/// Rotation decode factor of the i16 rotation encoding (1/32767).
// SPEC: FMT_LTB.md §10.3 (factor at VA 0x678478); EVIDENCE: engine+0x62760
pub const ROT_I16_SCALE: f32 = 1.0 / 32767.0;

/// The 0x14-byte header block plus `model_rev`.
// SPEC: FMT_LTB.md §2
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub file_kind: u8,
    /// Byte 1, zero in all model files.
    pub todo_pad: u8,
    pub container_rev: u16,
    /// Bytes 4..0x14, never read by the engine.
    pub todo_reserved: [u8; 16],
    pub model_rev: u32,
}

impl Default for Header {
    fn default() -> Self {
        Self {
            file_kind: KIND_MODEL,
            todo_pad: 0,
            container_rev: CONTAINER_REV,
            todo_reserved: [0; 16],
            model_rev: 25,
        }
    }
}

/// The 15 size hints. Only some indices have an observed meaning; the engine uses them to size one arena.
// SPEC: FMT_LTB.md §3
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SizeHints(pub [u32; 15]);

impl SizeHints {
    pub fn markers(&self) -> u32 {
        self.0[0]
    }
    pub fn clips(&self) -> u32 {
        self.0[1]
    }
    pub fn nodes(&self) -> u32 {
        self.0[2]
    }
    pub fn pieces(&self) -> u32 {
        self.0[3]
    }
    pub fn child_slots(&self) -> u32 {
        self.0[4]
    }
    pub fn attach_points(&self) -> u32 {
        self.0[9]
    }
    pub fn blend_sets(&self) -> u32 {
        self.0[10]
    }
}

/// A whole model file.
// SPEC: FMT_LTB.md §4 (section order)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Model {
    pub header: Header,
    pub hints: SizeHints,
    /// Free-form model command text.
    pub command: RawStr,
    pub radius: f32,
    /// Box records: read and discarded by this engine build; layout unknown (0 in every ROM file).
    // SPEC: FMT_LTB.md §4 step 5, §13
    pub todo_boxes: Vec<[u8; BOX_RECORD_LEN]>,
    pub pieces: Vec<Piece>,
    /// Skeleton nodes, flattened in file order (depth-first, root first). Clip tracks use the same order.
    pub nodes: Vec<Node>,
    pub blend_sets: Vec<BlendSet>,
    /// Paths of child models whose clips this model borrows (slots 1..; slot 0 is the model itself).
    pub child_models: Vec<RawStr>,
    pub clips: Vec<Clip>,
    pub sockets: Vec<Socket>,
    /// Per child slot (including slot 0), the clip overrides. Length = `child_models.len() + 1`.
    pub clip_overrides: Vec<Vec<ClipOverride>>,
    /// Bytes after the last section (0 in every loadable ROM file).
    pub trailing: Vec<u8>,
}

/// One piece with its LOD chain.
// SPEC: FMT_LTB.md §5
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Piece {
    pub name: RawStr,
    /// One switch distance per LOD (first 0.0, increasing in the ROM).
    pub lod_dist: Vec<f32>,
    /// Read into a local by the engine; 0 in all files.
    pub todo_lod_lo: u32,
    /// Read into a local by the engine; 0 for 1-LOD pieces, 2 for 3-LOD pieces.
    pub todo_lod_hi: u32,
    pub lods: Vec<Lod>,
}

/// One LOD: material slots, render style, and the render object.
// SPEC: FMT_LTB.md §5 (lod record)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lod {
    pub slot_count: u32,
    /// Material slot indices; unused tail entries hold their own position index in the ROM.
    pub slots: [u32; 4],
    pub style_index: u32,
    /// Draw-order bias.
    pub priority: u8,
    /// `body_len` as stored. The engine ignores it; [`Mesh::body_len`] is the length the body encodes to.
    pub declared_body_len: u32,
    pub mesh: Mesh,
    /// Nodes this LOD's mesh uses.
    pub used_nodes: Vec<u8>,
}

/// Render-object kind values.
// SPEC: FMT_LTB.md §5 (ro_kind), §1 (constructors stamp kinds 4 / 5 / 6)
pub const MESH_RIGID: u32 = 4;
pub const MESH_SKINNED: u32 = 5;
pub const MESH_MORPH: u32 = 6;

/// The three render-object bodies recovered from vtable slot 6 of the three renderer classes.
// SPEC: FMT_LTB.md §6
#[derive(Debug, Clone, PartialEq)]
pub enum Mesh {
    Rigid(RigidMesh),
    Skinned(SkinnedMesh),
    Morph(MorphMesh),
}

impl Default for Mesh {
    fn default() -> Self {
        Mesh::Rigid(RigidMesh::default())
    }
}

impl Mesh {
    pub fn kind(&self) -> u32 {
        match self {
            Mesh::Rigid(_) => MESH_RIGID,
            Mesh::Skinned(_) => MESH_SKINNED,
            Mesh::Morph(_) => MESH_MORPH,
        }
    }

    /// The blend kind the vertex streams of this body are laid out with.
    pub fn blend_kind(&self) -> Option<u32> {
        match self {
            Mesh::Rigid(_) | Mesh::Morph(_) => Some(0),
            Mesh::Skinned(s) => s.blend_kind(),
        }
    }

    pub fn streams(&self) -> &[VertexStream; 4] {
        match self {
            Mesh::Rigid(m) => &m.streams,
            Mesh::Skinned(m) => &m.streams,
            Mesh::Morph(m) => &m.streams,
        }
    }

    pub fn vertex_count(&self) -> u32 {
        match self {
            Mesh::Rigid(m) => m.vertex_count,
            Mesh::Skinned(m) => m.vertex_count,
            Mesh::Morph(m) => m.vertex_count,
        }
    }

    pub fn indices(&self) -> &[u16] {
        match self {
            Mesh::Rigid(m) => &m.indices,
            Mesh::Skinned(m) => &m.indices,
            Mesh::Morph(m) => &m.indices,
        }
    }

    /// Field layout of stream `i`, if present and of a known shape.
    pub fn layout(&self, i: usize) -> Option<VertexLayout> {
        let s = self.streams().get(i)?;
        if !s.is_present() {
            return None;
        }
        VertexLayout::new(self.blend_kind()?, s.flags)
    }

    /// Bytes this body encodes to after its `body_len` field (the value `body_len` must hold).
    // SPEC: FMT_LTB.md §6 (body_len counts the bytes after itself up to the kind's last field)
    pub fn body_len(&self) -> u64 {
        let streams: u64 = self.streams().iter().map(|s| s.data.len() as u64).sum();
        let idx = self.indices().len() as u64 * 2;
        streams
            + idx
            + match self {
                Mesh::Rigid(_) => 4 * 4 + 16 + 4,
                Mesh::Skinned(m) => {
                    4 * 4
                        + 1
                        + 16
                        + 1
                        + match &m.tail {
                            SkinTail::Spans(s) => 4 + 12 * s.len() as u64,
                            SkinTail::Palette { palette_map, .. } => {
                                8 + palette_map.as_ref().map_or(0, |p| 4 + 4 * p.len() as u64)
                            }
                        }
                }
                Mesh::Morph(m) => 6 * 4 + 16 + 4 + 4 + 4 * m.vertex_map.len() as u64,
            }
    }
}

/// Rigid body: the whole mesh follows one node.
// SPEC: FMT_LTB.md §6.2
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RigidMesh {
    pub vertex_count: u32,
    pub tri_count: u32,
    /// Unused by the reader; 1 in all files.
    pub todo_influences: u32,
    /// Unused by the reader; 1 or 2 in the ROM.
    pub todo_unused: u32,
    pub node: u32,
    pub streams: [VertexStream; 4],
    pub indices: Vec<u16>,
}

/// Skinned body (per-vertex blend weights).
// SPEC: FMT_LTB.md §6.3
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SkinnedMesh {
    pub vertex_count: u32,
    pub tri_count: u32,
    /// 1..4.
    pub max_influences: u32,
    /// Used only by the palette tail (2..4 there).
    pub palette_influences: u32,
    /// Raw flag byte; on the palette tail a non-zero value adds a palette map.
    pub has_palette_map: u8,
    pub streams: [VertexStream; 4],
    pub indices: Vec<u16>,
    pub tail: SkinTail,
}

impl SkinnedMesh {
    /// The blend kind the tail reader derives, or `None` where the engine fails.
    // SPEC: FMT_LTB.md §6.3 (per-vertex: max_influences - 1; palette: 4 / 5 / 6 for 2 / 3 / 4)
    pub fn blend_kind(&self) -> Option<u32> {
        match self.tail {
            SkinTail::Spans(_) => (1..=4)
                .contains(&self.max_influences)
                .then(|| self.max_influences - 1),
            SkinTail::Palette { .. } => (2..=4)
                .contains(&self.palette_influences)
                .then(|| self.palette_influences + 2),
        }
    }

    /// True when the spans tile `0..vertex_count` in order and their index ends rise strictly to
    /// `3 * tri_count` (the contiguity the spec observed in every skinned body). `None` for the palette tail.
    // SPEC: FMT_LTB.md §6.3 (span table, OBSERVED over all 2,997 skinned bodies)
    pub fn spans_contiguous(&self) -> Option<bool> {
        let SkinTail::Spans(spans) = &self.tail else {
            return None;
        };
        let mut next_vertex = 0u64;
        let mut last_end: Option<u32> = None;
        for s in spans {
            if s.first_vertex as u64 != next_vertex {
                return Some(false);
            }
            next_vertex += s.vertex_count as u64;
            if let Some(prev) = last_end {
                if s.index_end <= prev {
                    return Some(false);
                }
            }
            last_end = Some(s.index_end);
        }
        Some(
            next_vertex == self.vertex_count as u64
                && last_end.map_or(self.tri_count == 0, |e| {
                    e as u64 == 3 * self.tri_count as u64
                }),
        )
    }
}

/// The two tails of a skinned body, selected by the `palette_path` byte (0 / 1).
// SPEC: FMT_LTB.md §6.3
#[derive(Debug, Clone, PartialEq)]
pub enum SkinTail {
    /// `palette_path` = 0: streams, indices, then the span table.
    Spans(Vec<SkinSpan>),
    /// `palette_path` = 1: two unidentified dwords, an optional palette map, then streams and indices.
    /// No ROM file uses this path.
    Palette {
        todo_cc: u32,
        todo_d0: u32,
        palette_map: Option<Vec<u32>>,
    },
}

impl Default for SkinTail {
    fn default() -> Self {
        SkinTail::Spans(Vec::new())
    }
}

/// One 12-byte span of a skinned body.
// SPEC: FMT_LTB.md §6.3 (span table)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SkinSpan {
    pub first_vertex: u16,
    pub vertex_count: u16,
    /// Nodes driving the span; 0xff = unused.
    pub nodes: [u8; 4],
    /// Running end of the span's indices.
    pub index_end: u32,
}

/// Morph (vertex-animated) body. The animated positions live in the clip tracks.
// SPEC: FMT_LTB.md §6.4
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MorphMesh {
    pub vertex_count: u32,
    pub base_vertex_count: u32,
    pub tri_count: u32,
    /// Stored at object +0xac; 0 in the ROM.
    pub todo_influences: u32,
    /// Stored at object +0xa8; 0 in the ROM.
    pub todo_a8: u32,
    /// Stored at object +0xa4; 1..5 in the ROM.
    pub todo_a4: u32,
    pub node: u32,
    pub streams: [VertexStream; 4],
    pub indices: Vec<u16>,
    /// By the spec's reading a vertex -> base-vertex map.
    pub vertex_map: Vec<u32>,
}

/// One skeleton node.
// SPEC: FMT_LTB.md §7
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Node {
    pub name: RawStr,
    pub index: u16,
    pub flags: u8,
    /// Model-space bind transform, 4 rows of 4, row-major.
    pub model_xform: [f32; 16],
    /// Position of the parent in [`Model::nodes`] (`None` for the root).
    pub parent: Option<usize>,
    /// Positions of the children in [`Model::nodes`], in file order.
    pub children: Vec<usize>,
}

impl Node {
    /// True when the last matrix row is exactly (0,0,0,1).
    // SPEC: FMT_LTB.md §7 (last row (0,0,0,1) in all 48,730 nodes, OBSERVED)
    pub fn last_row_affine(&self) -> bool {
        self.model_xform[12..16] == [0.0, 0.0, 0.0, 1.0]
    }
}

/// 4x4 row-major identity.
pub const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// Row-major `a * b`.
pub fn mat_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut r = [0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            r[i * 4 + j] = (0..4).map(|k| a[i * 4 + k] * b[k * 4 + j]).sum();
        }
    }
    r
}

/// Rigid inverse: transposed rotation, negated rotated translation (translation in column 3).
// SPEC: FMT_LTB.md §7 (inverse built by transposing the rotation and negating the rotated translation)
pub fn rigid_inverse(m: &[f32; 16]) -> [f32; 16] {
    let mut r = IDENTITY;
    for i in 0..3 {
        for j in 0..3 {
            r[i * 4 + j] = m[j * 4 + i];
        }
    }
    for i in 0..3 {
        r[i * 4 + 3] = -(0..3).map(|k| r[i * 4 + k] * m[k * 4 + 3]).sum::<f32>();
    }
    r
}

/// One per-node weight mask used when blending clips.
// SPEC: FMT_LTB.md §8
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlendSet {
    pub name: RawStr,
    /// One weight per node, in node order.
    pub weights: Vec<f32>,
}

/// Track codecs.
// SPEC: FMT_LTB.md §10.3
pub const CODEC_RAW: u32 = 0;
pub const CODEC_F32: u32 = 1;
pub const CODEC_I16: u32 = 2;
pub const CODEC_F32_POS_I16_ROT: u32 = 3;

/// One animation clip.
// SPEC: FMT_LTB.md §10.1
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Clip {
    pub extent: [f32; 3],
    pub name: RawStr,
    pub codec: u32,
    /// Transition time, by the spec's reading.
    pub blend_ms: u32,
    pub markers: Vec<Marker>,
    /// One track per node, in [`Model::nodes`] order.
    pub tracks: Vec<Track>,
}

/// Clip key time with its command text.
// SPEC: FMT_LTB.md §10.2
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Marker {
    pub time_ms: u32,
    pub command: RawStr,
}

/// One node's track.
// SPEC: FMT_LTB.md §10.3
#[derive(Debug, Clone, PartialEq)]
pub enum Track {
    /// Codec 0 with a non-zero `morph` byte: per marker, a set of vertex positions for a morph mesh.
    Morph {
        flag: u8,
        frames: Vec<Vec<[f32; 3]>>,
    },
    /// Transform channels. For codec 0 both hold exactly `marker_count` f32 keys. For codecs 1..3 an empty
    /// channel means "none" and one key means constant.
    Keys { pos: PosKeys, rot: RotKeys },
}

/// Position keys in either encoding.
#[derive(Debug, Clone, PartialEq)]
pub enum PosKeys {
    F32(Vec<[f32; 3]>),
    I16(Vec<[i16; 3]>),
}

impl PosKeys {
    pub fn len(&self) -> usize {
        match self {
            PosKeys::F32(v) => v.len(),
            PosKeys::I16(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn decode(&self) -> Vec<[f32; 3]> {
        match self {
            PosKeys::F32(v) => v.clone(),
            PosKeys::I16(v) => v
                .iter()
                .map(|k| k.map(|c| c as f32 * POS_I16_SCALE))
                .collect(),
        }
    }
}

/// Rotation (quaternion) keys in either encoding.
#[derive(Debug, Clone, PartialEq)]
pub enum RotKeys {
    F32(Vec<[f32; 4]>),
    I16(Vec<[i16; 4]>),
}

impl RotKeys {
    pub fn len(&self) -> usize {
        match self {
            RotKeys::F32(v) => v.len(),
            RotKeys::I16(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn decode(&self) -> Vec<[f32; 4]> {
        match self {
            RotKeys::F32(v) => v.clone(),
            RotKeys::I16(v) => v
                .iter()
                .map(|k| k.map(|c| c as f32 * ROT_I16_SCALE))
                .collect(),
        }
    }
}

/// Whether a codec stores positions / rotations as i16.
pub(crate) fn codec_i16(codec: u32) -> (bool, bool) {
    match codec {
        CODEC_I16 => (true, true),
        CODEC_F32_POS_I16_ROT => (false, true),
        _ => (false, false),
    }
}

/// Attach point ("socket" in the engine's own API).
// SPEC: FMT_LTB.md §11.1
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Socket {
    pub node: u32,
    pub name: RawStr,
    pub rot: [f32; 4],
    pub pos: [f32; 3],
    pub scale: [f32; 3],
}

/// Per-child clip override, looked up by clip name.
// SPEC: FMT_LTB.md §11.2
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClipOverride {
    pub clip_name: RawStr,
    pub extent: [f32; 3],
    pub offset: [f32; 3],
}

impl Model {
    /// Local transforms: `inverse(parent model_xform) * model_xform`, root against identity.
    // SPEC: FMT_LTB.md §7 (local = parent_inverse * model_xform)
    pub fn local_xforms(&self) -> Vec<[f32; 16]> {
        self.nodes
            .iter()
            .map(|n| {
                let parent = n
                    .parent
                    .and_then(|p| self.nodes.get(p))
                    .map_or(IDENTITY, |p| p.model_xform);
                mat_mul(&rigid_inverse(&parent), &n.model_xform)
            })
            .collect()
    }

    /// Iterates every LOD of every piece.
    pub fn lods(&self) -> impl Iterator<Item = &Lod> {
        self.pieces.iter().flat_map(|p| p.lods.iter())
    }

    /// Recomputes every LOD's `declared_body_len` from its body (for models built in memory).
    pub fn fix_body_lens(&mut self) {
        for p in &mut self.pieces {
            for l in &mut p.lods {
                l.declared_body_len = u32::try_from(l.mesh.body_len()).unwrap_or(u32::MAX);
            }
        }
    }
}
