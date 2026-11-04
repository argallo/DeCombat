//! CPU side of the model passes: loading a model file (and the child models it borrows clips from) from the
//! user's ROM, and building one GPU vertex / index list per piece with the bone indices and weights the
//! skinning shader needs.
//!
//! Formats: `FMT_LTB.md` through `dc-model`. Vertices of every mesh kind are stored in the bind pose's model
//! space (OBSERVED: `model_census`), so rigid pieces are bound to their node with weight 1 and skinned pieces
//! with the span's node list and the stored weights.

use crate::anim::AnimSet;
use bytemuck::{Pod, Zeroable};
use dc_model::{Mesh, Model, ModelError, SkinTail};
use dc_rez::{RezError, RezVfs};
use std::fmt;

/// Most bones a model may use: one bone per node, and the palette uniform holds 256 matrices.
/// OBSERVED: the largest node count of any ROM model is 221 (`model_census`).
pub const MAX_BONES: usize = 256;

#[derive(Debug)]
pub enum ModelLoadError {
    /// The path has no archive component (`<archive>\...`).
    BadPath(String),
    /// The entry is not in the user's ROM.
    Missing(String),
    Rez(RezError),
    Parse(String, ModelError),
    /// More nodes than the bone palette holds.
    TooManyNodes(usize),
}

impl fmt::Display for ModelLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelLoadError::BadPath(p) => write!(f, "resource path without an archive: {p}"),
            ModelLoadError::Missing(p) => write!(f, "not in the ROM: {p}"),
            ModelLoadError::Rez(e) => write!(f, "archive: {e}"),
            ModelLoadError::Parse(p, e) => write!(f, "model {p}: {e:?}"),
            ModelLoadError::TooManyNodes(n) => {
                write!(f, "model has {n} nodes, the palette holds {MAX_BONES}")
            }
        }
    }
}

impl std::error::Error for ModelLoadError {}

impl From<RezError> for ModelLoadError {
    fn from(e: RezError) -> Self {
        ModelLoadError::Rez(e)
    }
}

/// The archive and the entry of a ROM resource path: the first path component names the archive (every ROM
/// archive lists its entries under a top folder of its own name), the whole path is the entry.
// OBSERVED: ROM archive layout (the entries of every `.rez` under `Game` start with `<archive name>\`; the same
// rule dc-dserver's character resolution uses)
pub fn rez_entry_of(path: &str) -> Option<(String, String)> {
    let p = path.trim().replace('/', "\\");
    let p = p.trim_start_matches('\\');
    let (top, rest) = p.split_once('\\')?;
    if top.is_empty() || rest.is_empty() {
        return None;
    }
    Some((format!("{top}.rez"), p.to_string()))
}

/// Bytes of a ROM resource by its path (model entries are stored plain; the VFS returns them as stored).
pub fn read_rom_entry(vfs: &RezVfs, path: &str) -> Result<Vec<u8>, ModelLoadError> {
    let (rez, entry) =
        rez_entry_of(path).ok_or_else(|| ModelLoadError::BadPath(path.to_string()))?;
    let _io = crate::perf::span(crate::perf::Counter::IoNs);
    let bytes = vfs
        .entry(&rez, &entry)?
        .ok_or_else(|| ModelLoadError::Missing(path.to_string()))?;
    crate::perf::add(crate::perf::Counter::IoBytes, bytes.len() as u64);
    Ok(bytes)
}

/// A model file and the child models it borrows clips from.
#[derive(Debug, Clone)]
pub struct ModelFiles {
    pub path: String,
    pub body: Model,
    /// `(path, model)` per child slot 1.. in file order.
    pub children: Vec<(String, Model)>,
}

/// Path of a child model named in a model file: the child name in the parent's folder unless it carries a
/// folder of its own.
// SPEC: FMT_LTB.md §9 (child slot = str path naming another model file); OBSERVED: the ROM body models name
// their child by file name only, the file sits in the body's folder
pub fn child_path(parent: &str, child: &str) -> String {
    let child = child.replace('/', "\\");
    if child.contains('\\') {
        return child;
    }
    match parent.replace('/', "\\").rsplit_once('\\') {
        Some((folder, _)) => format!("{folder}\\{child}"),
        None => child,
    }
}

impl ModelFiles {
    /// Reads `path` and every child model from the ROM.
    pub fn load(vfs: &RezVfs, path: &str) -> Result<ModelFiles, ModelLoadError> {
        let bytes = read_rom_entry(vfs, path)?;
        let parse = crate::perf::span(crate::perf::Counter::ParseNs);
        let body = Model::parse(&bytes).map_err(|e| ModelLoadError::Parse(path.to_string(), e))?;
        drop(parse);
        if body.nodes.len() > MAX_BONES {
            return Err(ModelLoadError::TooManyNodes(body.nodes.len()));
        }
        let mut children = Vec::with_capacity(body.child_models.len());
        for c in &body.child_models {
            let cp = child_path(path, &c.to_text());
            let cb = read_rom_entry(vfs, &cp)?;
            let parse = crate::perf::span(crate::perf::Counter::ParseNs);
            let cm = Model::parse(&cb).map_err(|e| ModelLoadError::Parse(cp.clone(), e))?;
            drop(parse);
            children.push((cp, cm));
        }
        Ok(ModelFiles {
            path: path.to_string(),
            body,
            children,
        })
    }

    /// A model built in memory (no children).
    pub fn from_model(path: &str, body: Model) -> ModelFiles {
        ModelFiles {
            path: path.to_string(),
            body,
            children: Vec::new(),
        }
    }

    /// Skeleton and the combined clip list (own clips, then each child's).
    pub fn anim_set(&self) -> AnimSet {
        let kids: Vec<Model> = self.children.iter().map(|c| c.1.clone()).collect();
        AnimSet::new(&self.body, &kids)
    }
}

/// GPU vertex of the model passes: bind-pose position and normal (model space), one uv pair, four bone indices
/// and four weights.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct ModelVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub bones: [u8; 4],
    pub weights: [f32; 4],
}

impl ModelVertex {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
        3 => Uint8x4,
        4 => Float32x4,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ModelVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// One drawable piece LOD: a range of the model's index list and the material it is drawn with.
///
/// The material slots, the render-style slot and the priority are the piece's, which the loader leaves at the
/// values of the piece's LAST LOD record (every LOD record overwrites the same piece fields); the draw code
/// reads them from the piece for whichever LOD it draws.
// SPEC: FMT_LTB.md §5 (fields slot_count..priority written into the piece once per LOD); EVIDENCE:
// engine+0x1b9390 (texture slots, style index and priority read from the piece, param 2), engine+0x1b1ff0
// (the LOD's render object taken from piece +0x24 [lod])
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PieceDraw {
    pub piece: usize,
    /// LOD index inside the piece.
    pub lod: u32,
    pub first_index: u32,
    pub index_count: u32,
    /// Material slot of the piece's first texture (index into the model's texture list).
    pub material: u32,
    /// The piece's material slots (texture 0..3 of a render-style stage) and how many are used.
    pub slots: [u32; 4],
    pub slot_count: u32,
    /// Render-style slot (index into the object's style list of this model entry).
    pub style: u32,
    /// Draw-order bias of the piece.
    pub priority: u8,
    pub kind: u32,
}

/// Counters of one mesh build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModelMeshStats {
    pub pieces: u64,
    pub rigid: u64,
    pub skinned: u64,
    /// Morph (vertex-animated) bodies: drawn in their base pose; the per-marker positions are not played.
    pub morph: u64,
    /// Pieces left out (unsupported vertex layout, bad indices, bad span table).
    pub skipped: u64,
    pub vertices: u64,
    pub triangles: u64,
}

/// The vertex and index lists of every LOD of a model's pieces.
#[derive(Debug, Clone, Default)]
pub struct ModelMesh {
    pub vertices: Vec<ModelVertex>,
    pub indices: Vec<u32>,
    /// Every built (piece, LOD), in piece order then LOD order.
    pub draws: Vec<PieceDraw>,
    /// Switch distances per piece (index = piece), as stored (FMT_LTB.md §5 `lod_dist`).
    pub lod_dists: Vec<Vec<f32>>,
    /// Counters of LOD 0 (the counts the census and the reports use).
    pub stats: ModelMeshStats,
    /// Triangles per LOD level over all pieces (index = LOD).
    pub lod_triangles: Vec<u64>,
    /// Bind-pose bounds of the drawn LOD 0 vertices.
    pub bounds: Option<([f32; 3], [f32; 3])>,
}

/// The LOD a piece is drawn with at distance `dist` from the eye: 0 when the distance is not positive, else
/// the first `i` with `dist < lod_dist[i + 1]` (the last LOD past the last switch distance), then
/// `lod_offset` (`model_lod_offset`, default 0) added and the result clamped to the piece's LODs.
// EVIDENCE: engine+0x1b1ff0 (distance = |camera position (VA 0x7e5d58) - object position (+0xc8)|, 0 for an
// object with flag 0x40 or +0x1c8 set; scan of piece +0x28 from index 1; `model_lod_offset` VA 0x6d44e4 added;
// clamp to [0, lod_count - 1]); SPEC: RENDER_2009.md §13 (`model_lod_offset` default 0)
pub fn select_lod(lod_dist: &[f32], dist: f32, lod_offset: i32) -> usize {
    let last = lod_dist.len().saturating_sub(1);
    let mut lod = 0usize;
    if dist > 0.0 && last > 0 {
        lod = last;
        for (i, d) in lod_dist.iter().enumerate().skip(1) {
            if dist < *d {
                lod = i - 1;
                break;
            }
        }
    }
    (lod as i64 + lod_offset as i64).clamp(0, last as i64) as usize
}

/// Weights of one vertex of a span with `bones` nodes: the vertex stores `max_influences - 1` weights; the span
/// uses its first `bones - 1` and the last is `1 - sum` (public Direct3D 9 vertex blending: with `n` blend
/// weights the matrix `n` gets `1 - sum`). One bone: weight 1.
// SPEC: RENDER_2009.md §7.4 (per bone set up to 4 indices, 0xff ends the list; VERTEXBLEND 1..3 WEIGHTS for 2..4
// bones, DISABLE for 1; the last weight is 1 - sum)
pub fn span_weights(stored: &[f32], bones: usize) -> [f32; 4] {
    let mut w = [0f32; 4];
    let bones = bones.clamp(1, 4);
    let mut sum = 0f32;
    for (k, slot) in w.iter_mut().enumerate().take(bones - 1) {
        let v = stored.get(k).copied().unwrap_or(0.0);
        let v = if v.is_finite() { v } else { 0.0 };
        *slot = v;
        sum += v;
    }
    w[bones - 1] = 1.0 - sum;
    w
}

impl ModelMesh {
    /// Every LOD of every piece; each LOD draws with the piece's material slots, style and priority (those of
    /// its last LOD record, see [`PieceDraw`]).
    // SPEC: FMT_LTB.md §5 (pieces, LODs, material slots, render-style slot), §6 (bodies), RENDER_2009.md §7.2
    pub fn build(m: &Model) -> ModelMesh {
        let mut out = ModelMesh::default();
        let nodes = m.nodes.len();
        for (pi, piece) in m.pieces.iter().enumerate() {
            out.lod_dists.push(piece.lod_dist.clone());
            let Some(last) = piece.lods.last() else {
                continue;
            };
            let (slots, slot_count, style, priority) =
                (last.slots, last.slot_count, last.style_index, last.priority);
            for (li, lod) in piece.lods.iter().enumerate() {
                let first_lod = li == 0;
                if first_lod {
                    out.stats.pieces += 1;
                }
                let mut st = ModelMeshStats::default();
                if let Some(draw) = Self::build_lod(&mut out, &mut st, nodes, lod, pi, li) {
                    out.draws.push(PieceDraw {
                        material: slots[0],
                        slots,
                        slot_count,
                        style,
                        priority,
                        ..draw
                    });
                    if out.lod_triangles.len() <= li {
                        out.lod_triangles.resize(li + 1, 0);
                    }
                    out.lod_triangles[li] += (draw.index_count / 3) as u64;
                }
                if first_lod {
                    out.stats.rigid += st.rigid;
                    out.stats.skinned += st.skinned;
                    out.stats.morph += st.morph;
                    out.stats.skipped += st.skipped;
                    out.stats.vertices += st.vertices;
                    out.stats.triangles += st.triangles;
                }
            }
        }
        out
    }

    /// Builds one LOD's vertices and indices into `out`; `None` when the LOD is left out.
    fn build_lod(
        out: &mut ModelMesh,
        stats: &mut ModelMeshStats,
        nodes: usize,
        lod: &dc_model::Lod,
        pi: usize,
        li: usize,
    ) -> Option<PieceDraw> {
        let mesh = &lod.mesh;
        let Some(layout) = mesh.layout(0) else {
            stats.skipped += 1;
            return None;
        };
        let stream = &mesh.streams()[0];
        let n = mesh.vertex_count() as usize;
        let base = out.vertices.len() as u32;
        // per-vertex bones and weights
        let mut bones = vec![[0u8; 4]; n];
        let mut weights = vec![[1.0f32, 0.0, 0.0, 0.0]; n];
        match mesh {
            Mesh::Rigid(r) => {
                let b = (r.node as usize).min(nodes.saturating_sub(1)) as u8;
                bones.iter_mut().for_each(|x| *x = [b, b, b, b]);
                stats.rigid += 1;
            }
            Mesh::Morph(r) => {
                let b = (r.node as usize).min(nodes.saturating_sub(1)) as u8;
                bones.iter_mut().for_each(|x| *x = [b, b, b, b]);
                stats.morph += 1;
            }
            Mesh::Skinned(s) => {
                let SkinTail::Spans(spans) = &s.tail else {
                    // the palette tail: no ROM file uses it (FMT_LTB.md §6.3)
                    stats.skipped += 1;
                    return None;
                };
                let mut ok = true;
                for sp in spans {
                    let list: Vec<u8> = sp
                        .nodes
                        .iter()
                        .copied()
                        .take_while(|&x| x != 0xff)
                        .collect();
                    if list.is_empty() || list.iter().any(|&x| x as usize >= nodes) {
                        ok = false;
                        break;
                    }
                    let first = sp.first_vertex as usize;
                    let end = (first + sp.vertex_count as usize).min(n);
                    for v in first..end {
                        let stored = stream.weights(&layout, v).unwrap_or_default();
                        weights[v] = span_weights(&stored, list.len());
                        let mut b = [list[0]; 4];
                        for (k, &x) in list.iter().enumerate() {
                            b[k] = x;
                        }
                        bones[v] = b;
                    }
                }
                if !ok {
                    stats.skipped += 1;
                    return None;
                }
                stats.skinned += 1;
            }
        }
        let mut good = true;
        let mut verts = Vec::with_capacity(n);
        for v in 0..n {
            let (Some(p), Some(nm)) = (stream.position(&layout, v), stream.normal(&layout, v))
            else {
                good = false;
                break;
            };
            let uv = stream.uv(&layout, v, 0).unwrap_or([0.0, 0.0]);
            verts.push(ModelVertex {
                pos: p,
                normal: nm,
                uv,
                bones: bones[v],
                weights: weights[v],
            });
        }
        let idx = mesh.indices();
        if !good || idx.iter().any(|&i| i as usize >= n) {
            stats.skipped += 1;
            return None;
        }
        for v in verts.iter().filter(|_| li == 0) {
            let b = out.bounds.get_or_insert((v.pos, v.pos));
            for k in 0..3 {
                b.0[k] = b.0[k].min(v.pos[k]);
                b.1[k] = b.1[k].max(v.pos[k]);
            }
        }
        out.vertices.extend(verts);
        let first_index = out.indices.len() as u32;
        out.indices.extend(idx.iter().map(|&i| base + i as u32));
        let count = idx.len() as u32;
        stats.vertices += n as u64;
        stats.triangles += (count / 3) as u64;
        Some(PieceDraw {
            piece: pi,
            lod: li as u32,
            first_index,
            index_count: count,
            material: lod.slots[0],
            slots: lod.slots,
            slot_count: lod.slot_count,
            style: lod.style_index,
            priority: lod.priority,
            kind: mesh.kind(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_follow_the_d3d_rule() {
        assert_eq!(span_weights(&[0.25, 0.5, 0.1], 1), [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(span_weights(&[0.25, 0.5, 0.1], 2), [0.25, 0.75, 0.0, 0.0]);
        let w = span_weights(&[0.25, 0.5, 0.1], 4);
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!((w[3] - 0.15).abs() < 1e-6);
        assert_eq!(span_weights(&[f32::NAN], 2), [0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn paths() {
        assert_eq!(
            rez_entry_of("Models\\A\\b.ltb"),
            Some(("Models.rez".into(), "Models\\A\\b.ltb".into()))
        );
        assert_eq!(rez_entry_of("b.ltb"), None);
        assert_eq!(
            child_path("M\\Bodies\\b.ltb", "kid.ltb"),
            "M\\Bodies\\kid.ltb"
        );
        assert_eq!(child_path("M/Bodies/b.ltb", "X\\kid.ltb"), "X\\kid.ltb");
        assert_eq!(std::mem::size_of::<ModelVertex>(), 52);
    }
}
