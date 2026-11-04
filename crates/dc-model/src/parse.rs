//! The reader, in the section order of the engine's model loader.

use crate::cursor::Cursor;
use crate::error::ModelError;
use crate::model::*;
use crate::vertex::{stride, VertexStream};

/// Smallest encodings, used to bound counts before allocating.
const MIN_PIECE: usize = 2 + 4 + 4 + 4;
const MIN_LOD: usize = 4 + 16 + 4 + 1 + 4 + 4 + 1;
const MIN_NODE: usize = 2 + 2 + 1 + 64 + 4;
const MIN_BLEND_SET: usize = 2 + 4;
const MIN_CLIP: usize = 12 + 2 + 4 + 4 + 4;
const MIN_MARKER: usize = 4 + 2;
const MIN_SOCKET: usize = 4 + 2 + 16 + 12 + 12;
const MIN_OVERRIDE: usize = 2 + 12 + 12;

/// What the first bytes of an `.ltb` entry say it is, without parsing the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Model,
    RenderStyle,
    Other(u8),
}

impl FileKind {
    pub fn of(bytes: &[u8]) -> Option<FileKind> {
        Some(match *bytes.first()? {
            KIND_MODEL => FileKind::Model,
            KIND_RENDER_STYLE => FileKind::RenderStyle,
            k => FileKind::Other(k),
        })
    }
}

impl Model {
    /// Parses a complete model file. Bytes after the last section are kept in [`Model::trailing`].
    // SPEC: FMT_LTB.md §4; EVIDENCE: engine+0x6f380
    pub fn parse(bytes: &[u8]) -> Result<Model, ModelError> {
        Model::parse_with(bytes, true)
    }

    /// [`Model::parse`] without the clips' key frames: every track is checked and stepped over, and each
    /// [`Clip::tracks`] is left empty. For readers that need only the clip list (names, markers, extents), the child
    /// models, sockets and clip overrides: a large animation model parses several times faster.
    pub fn parse_without_tracks(bytes: &[u8]) -> Result<Model, ModelError> {
        Model::parse_with(bytes, false)
    }

    fn parse_with(bytes: &[u8], keep_tracks: bool) -> Result<Model, ModelError> {
        let mut c = Cursor::new(bytes);
        let header = read_header(&mut c)?;
        // SPEC: FMT_LTB.md §3 (15 dwords, engine+0x6fd30)
        let hints = SizeHints(c.u32s::<15>()?);
        let command = c.string()?;
        let radius = c.f32()?;

        // SPEC: FMT_LTB.md §4 step 5 (box records, read and discarded)
        let box_count = c.count("box", BOX_RECORD_LEN)?;
        let mut todo_boxes = Vec::with_capacity(box_count);
        for _ in 0..box_count {
            let mut b = [0u8; BOX_RECORD_LEN];
            b.copy_from_slice(c.take(BOX_RECORD_LEN)?);
            todo_boxes.push(b);
        }

        // SPEC: FMT_LTB.md §4 step 6
        let piece_count = c.u32()?;
        if piece_count > MAX_PIECES {
            return Err(ModelError::TooManyPieces { count: piece_count });
        }
        let at = c.pos();
        let piece_count = c.room("piece", at, piece_count as u64, MIN_PIECE)?;
        let mut pieces = Vec::with_capacity(piece_count);
        for _ in 0..piece_count {
            pieces.push(read_piece(&mut c, &header)?);
        }

        let nodes = read_nodes(&mut c)?;

        // SPEC: FMT_LTB.md §8
        let set_count = c.count("blend set", MIN_BLEND_SET)?;
        let mut blend_sets = Vec::with_capacity(set_count);
        for set in 0..set_count {
            let name = c.string()?;
            let n = c.u32()?;
            if n as u64 != nodes.len() as u64 {
                return Err(ModelError::BlendSetSize {
                    set,
                    expected: nodes.len(),
                    got: n,
                });
            }
            let mut weights = Vec::with_capacity(nodes.len());
            for _ in 0..n {
                weights.push(c.f32()?);
            }
            blend_sets.push(BlendSet { name, weights });
        }

        // SPEC: FMT_LTB.md §4 step 11, §9
        let slots = c.u32()?;
        if !(1..=MAX_CHILD_SLOTS).contains(&slots) {
            return Err(ModelError::ChildCount { count: slots });
        }
        let mut child_models = Vec::with_capacity(slots as usize - 1);
        for _ in 1..slots {
            child_models.push(c.string()?);
        }

        // SPEC: FMT_LTB.md §10
        let clip_count = c.count("clip", MIN_CLIP)?;
        let mut clips = Vec::with_capacity(clip_count);
        for i in 0..clip_count {
            clips.push(read_clip(&mut c, i, nodes.len(), keep_tracks)?);
        }

        // SPEC: FMT_LTB.md §11.1; EVIDENCE: engine+0x6f160
        let socket_count = c.count("attach point", MIN_SOCKET)?;
        let mut sockets = Vec::with_capacity(socket_count);
        for i in 0..socket_count {
            let node = c.u32()?;
            if node as u64 >= nodes.len() as u64 {
                return Err(ModelError::SocketNode {
                    socket: i,
                    node,
                    node_count: nodes.len(),
                });
            }
            let name = c.string()?;
            let rot = c.f32s::<4>()?;
            let pos = c.f32s::<3>()?;
            let scale = c.f32s::<3>()?;
            sockets.push(Socket {
                node,
                name,
                rot,
                pos,
                scale,
            });
        }

        // SPEC: FMT_LTB.md §11.2; EVIDENCE: engine+0x6e0d0
        let mut clip_overrides = Vec::with_capacity(slots as usize);
        for _ in 0..slots {
            let n = c.count("clip override", MIN_OVERRIDE)?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                let clip_name = c.string()?;
                let extent = c.f32s::<3>()?;
                let offset = c.f32s::<3>()?;
                list.push(ClipOverride {
                    clip_name,
                    extent,
                    offset,
                });
            }
            clip_overrides.push(list);
        }

        let trailing = c.rest().to_vec();
        Ok(Model {
            header,
            hints,
            command,
            radius,
            todo_boxes,
            pieces,
            nodes,
            blend_sets,
            child_models,
            clips,
            sockets,
            clip_overrides,
            trailing,
        })
    }
}

// SPEC: FMT_LTB.md §2
fn read_header(c: &mut Cursor) -> Result<Header, ModelError> {
    let file_kind = c.u8()?;
    if file_kind != KIND_MODEL {
        return Err(ModelError::NotAModel { file_kind });
    }
    let todo_pad = c.u8()?;
    let container_rev = c.u16()?;
    if container_rev != CONTAINER_REV {
        return Err(ModelError::BadContainerRev { rev: container_rev });
    }
    let mut todo_reserved = [0u8; 16];
    todo_reserved.copy_from_slice(c.take(16)?);
    let model_rev = c.u32()?;
    if model_rev < MIN_MODEL_REV {
        return Err(ModelError::UnsupportedRevision { rev: model_rev });
    }
    Ok(Header {
        file_kind,
        todo_pad,
        container_rev,
        todo_reserved,
        model_rev,
    })
}

// SPEC: FMT_LTB.md §5; EVIDENCE: engine+0x6e720
fn read_piece(c: &mut Cursor, header: &Header) -> Result<Piece, ModelError> {
    let name = c.string()?;
    let lod_count = c.count("lod", 4 + MIN_LOD)?;
    let mut lod_dist = Vec::with_capacity(lod_count);
    for _ in 0..lod_count {
        lod_dist.push(c.f32()?);
    }
    let todo_lod_lo = c.u32()?;
    let todo_lod_hi = c.u32()?;
    let mut lods = Vec::with_capacity(lod_count);
    for _ in 0..lod_count {
        lods.push(read_lod(c, header)?);
    }
    Ok(Piece {
        name,
        lod_dist,
        todo_lod_lo,
        todo_lod_hi,
        lods,
    })
}

fn read_lod(c: &mut Cursor, _header: &Header) -> Result<Lod, ModelError> {
    let slot_count = c.u32()?;
    let slots = c.u32s::<4>()?;
    let style_index = c.u32()?;
    let priority = c.u8()?;
    let kind_at = c.pos();
    let kind = c.u32()?;
    // The render-object readers re-check the header block (file_kind 1, container_rev 9); `read_header`
    // already enforced both. SPEC: FMT_LTB.md §2
    let declared_body_len = c.u32()?;
    let mesh = match kind {
        MESH_RIGID => Mesh::Rigid(read_rigid(c)?),
        MESH_SKINNED => Mesh::Skinned(read_skinned(c)?),
        MESH_MORPH => Mesh::Morph(read_morph(c)?),
        _ => {
            return Err(ModelError::UnknownMeshKind {
                offset: kind_at,
                kind,
            })
        }
    };
    let n = c.u8()? as usize;
    let used_nodes = c.take(n)?.to_vec();
    Ok(Lod {
        slot_count,
        slots,
        style_index,
        priority,
        declared_body_len,
        mesh,
        used_nodes,
    })
}

/// Reads the four vertex streams of a body.
// SPEC: FMT_LTB.md §6.1 (flags 0 = absent; vertex_count * stride bytes per present stream)
fn read_streams(
    c: &mut Cursor,
    flags: [u32; 4],
    blend_kind: u32,
    vertex_count: u32,
) -> Result<[VertexStream; 4], ModelError> {
    let mut out: [VertexStream; 4] = Default::default();
    for (s, f) in out.iter_mut().zip(flags) {
        s.flags = f;
        if f != 0 {
            s.data = c.block(vertex_count, stride(blend_kind, f))?.to_vec();
        }
    }
    Ok(out)
}

/// `tri_count * 3` u16 indices.
// SPEC: FMT_LTB.md §6.1 (index data)
fn read_indices(c: &mut Cursor, tri_count: u32) -> Result<Vec<u16>, ModelError> {
    let raw = c.block(tri_count, 6)?;
    Ok(raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u16::from_le_bytes(*p))
        .collect())
}

// SPEC: FMT_LTB.md §6.2; EVIDENCE: engine+0x161210
fn read_rigid(c: &mut Cursor) -> Result<RigidMesh, ModelError> {
    let vertex_count = c.u32()?;
    let tri_count = c.u32()?;
    let todo_influences = c.u32()?;
    let todo_unused = c.u32()?;
    let flags = c.u32s::<4>()?;
    let node = c.u32()?;
    let streams = read_streams(c, flags, 0, vertex_count)?;
    let indices = read_indices(c, tri_count)?;
    Ok(RigidMesh {
        vertex_count,
        tri_count,
        todo_influences,
        todo_unused,
        node,
        streams,
        indices,
    })
}

// SPEC: FMT_LTB.md §6.3; EVIDENCE: engine+0x15f890, engine+0x15f160, engine+0x15f2d0
fn read_skinned(c: &mut Cursor) -> Result<SkinnedMesh, ModelError> {
    let vertex_count = c.u32()?;
    let tri_count = c.u32()?;
    let infl_at = c.pos();
    let max_influences = c.u32()?;
    let pal_at = c.pos();
    let palette_influences = c.u32()?;
    let has_palette_map = c.u8()?;
    let flags = c.u32s::<4>()?;
    let sel_at = c.pos();
    let selector = c.u8()?;
    match selector {
        0 => {
            if !(1..=4).contains(&max_influences) {
                return Err(ModelError::BadInfluences {
                    offset: infl_at,
                    value: max_influences,
                });
            }
            let streams = read_streams(c, flags, max_influences - 1, vertex_count)?;
            let indices = read_indices(c, tri_count)?;
            let n = c.count("skin span", 12)?;
            let mut spans = Vec::with_capacity(n);
            for _ in 0..n {
                let first_vertex = c.u16()?;
                let vertex_count = c.u16()?;
                let mut nodes = [0u8; 4];
                nodes.copy_from_slice(c.take(4)?);
                let index_end = c.u32()?;
                spans.push(SkinSpan {
                    first_vertex,
                    vertex_count,
                    nodes,
                    index_end,
                });
            }
            Ok(SkinnedMesh {
                vertex_count,
                tri_count,
                max_influences,
                palette_influences,
                has_palette_map,
                streams,
                indices,
                tail: SkinTail::Spans(spans),
            })
        }
        1 => {
            let todo_cc = c.u32()?;
            let todo_d0 = c.u32()?;
            if !(2..=4).contains(&palette_influences) {
                return Err(ModelError::BadInfluences {
                    offset: pal_at,
                    value: palette_influences,
                });
            }
            let palette_map = if has_palette_map != 0 {
                let n = c.count("palette map", 4)?;
                let mut m = Vec::with_capacity(n);
                for _ in 0..n {
                    m.push(c.u32()?);
                }
                Some(m)
            } else {
                None
            };
            let streams = read_streams(c, flags, palette_influences + 2, vertex_count)?;
            let indices = read_indices(c, tri_count)?;
            Ok(SkinnedMesh {
                vertex_count,
                tri_count,
                max_influences,
                palette_influences,
                has_palette_map,
                streams,
                indices,
                tail: SkinTail::Palette {
                    todo_cc,
                    todo_d0,
                    palette_map,
                },
            })
        }
        value => Err(ModelError::BadTailSelector {
            offset: sel_at,
            value,
        }),
    }
}

// SPEC: FMT_LTB.md §6.4; EVIDENCE: engine+0x15e4d0
fn read_morph(c: &mut Cursor) -> Result<MorphMesh, ModelError> {
    let vertex_count = c.u32()?;
    let base_vertex_count = c.u32()?;
    let tri_count = c.u32()?;
    let todo_influences = c.u32()?;
    let todo_a8 = c.u32()?;
    let flags = c.u32s::<4>()?;
    let todo_a4 = c.u32()?;
    let node = c.u32()?;
    let streams = read_streams(c, flags, 0, vertex_count)?;
    let indices = read_indices(c, tri_count)?;
    let n = c.count("morph vertex map", 4)?;
    let mut vertex_map = Vec::with_capacity(n);
    for _ in 0..n {
        vertex_map.push(c.u32()?);
    }
    Ok(MorphMesh {
        vertex_count,
        base_vertex_count,
        tri_count,
        todo_influences,
        todo_a8,
        todo_a4,
        node,
        streams,
        indices,
        vertex_map,
    })
}

/// Reads the node tree depth-first without recursion (hostile files cannot exhaust the stack).
// SPEC: FMT_LTB.md §7; EVIDENCE: engine+0x6ed10
fn read_nodes(c: &mut Cursor) -> Result<Vec<Node>, ModelError> {
    let mut nodes: Vec<Node> = Vec::new();
    // (node position, children still to read)
    let mut stack: Vec<(usize, u64)> = Vec::new();
    let mut parent: Option<usize> = None;
    loop {
        let name = c.string()?;
        let index = c.u16()?;
        let flags = c.u8()?;
        let model_xform = c.f32s::<16>()?;
        let at = c.pos();
        let child_count = c.u32()? as u64;
        // every child needs at least MIN_NODE more bytes
        c.room("node child", at, child_count, MIN_NODE)?;
        let me = nodes.len();
        if let Some(p) = parent {
            nodes[p].children.push(me);
        }
        nodes.push(Node {
            name,
            index,
            flags,
            model_xform,
            parent,
            children: Vec::new(),
        });
        stack.push((me, child_count));
        // find the next node that still expects a child
        loop {
            match stack.last_mut() {
                None => return Ok(nodes),
                Some((p, left)) if *left > 0 => {
                    *left -= 1;
                    parent = Some(*p);
                    break;
                }
                Some(_) => {
                    stack.pop();
                }
            }
        }
    }
}

// SPEC: FMT_LTB.md §10.1-10.3; EVIDENCE: engine+0x6eb40, engine+0x6dd60
fn read_clip(
    c: &mut Cursor,
    idx: usize,
    node_count: usize,
    keep_tracks: bool,
) -> Result<Clip, ModelError> {
    let extent = c.f32s::<3>()?;
    let name = c.string()?;
    let codec = c.u32()?;
    if codec > CODEC_F32_POS_I16_ROT {
        return Err(ModelError::BadCodec { clip: idx, codec });
    }
    let blend_ms = c.u32()?;
    let marker_count = c.count("marker", MIN_MARKER)?;
    let mut markers = Vec::with_capacity(marker_count);
    for _ in 0..marker_count {
        let time_ms = c.u32()?;
        let command = c.string()?;
        markers.push(Marker { time_ms, command });
    }
    let at = c.pos();
    // every track takes at least one byte (codec 0) or eight (codecs 1..3)
    c.room("track", at, node_count as u64, 1)?;
    let mut tracks = Vec::with_capacity(if keep_tracks { node_count } else { 0 });
    for _ in 0..node_count {
        if keep_tracks {
            tracks.push(read_track(c, codec, marker_count)?);
        } else {
            skip_track(c, codec, marker_count)?;
        }
    }
    Ok(Clip {
        extent,
        name,
        codec,
        blend_ms,
        markers,
        tracks,
    })
}

fn read_pos_f32(c: &mut Cursor, n: usize) -> Result<Vec<[f32; 3]>, ModelError> {
    let at = c.pos();
    c.room("position key", at, n as u64, 12)?;
    (0..n).map(|_| c.f32s::<3>()).collect()
}

fn read_rot_f32(c: &mut Cursor, n: usize) -> Result<Vec<[f32; 4]>, ModelError> {
    let at = c.pos();
    c.room("rotation key", at, n as u64, 16)?;
    (0..n).map(|_| c.f32s::<4>()).collect()
}

/// Steps over one track with the checks of [`read_track`] (same layout, no key frames kept).
fn skip_track(c: &mut Cursor, codec: u32, marker_count: usize) -> Result<(), ModelError> {
    let keys =
        |c: &mut Cursor, what: &'static str, n: usize, size: usize| -> Result<(), ModelError> {
            let at = c.pos();
            c.room(what, at, n as u64, size)?;
            c.take(n * size).map(|_| ())
        };
    if codec == CODEC_RAW {
        let flag = c.u8()?;
        if flag != 0 {
            let at = c.pos();
            c.room("morph frame", at, marker_count as u64, 4)?;
            for _ in 0..marker_count {
                let n = c.count("morph vertex", 12)?;
                keys(c, "position key", n, 12)?;
            }
            return Ok(());
        }
        keys(c, "position key", marker_count, 12)?;
        return keys(c, "rotation key", marker_count, 16);
    }
    let (pos_i16, rot_i16) = codec_i16(codec);
    let size = if pos_i16 { 6 } else { 12 };
    let n = c.count("position key", size)?;
    keys(c, "position key", n, size)?;
    let size = if rot_i16 { 8 } else { 16 };
    let n = c.count("rotation key", size)?;
    keys(c, "rotation key", n, size)
}

fn read_track(c: &mut Cursor, codec: u32, marker_count: usize) -> Result<Track, ModelError> {
    if codec == CODEC_RAW {
        let flag = c.u8()?;
        if flag != 0 {
            let at = c.pos();
            c.room("morph frame", at, marker_count as u64, 4)?;
            let mut frames = Vec::with_capacity(marker_count);
            for _ in 0..marker_count {
                let n = c.count("morph vertex", 12)?;
                frames.push(read_pos_f32(c, n)?);
            }
            return Ok(Track::Morph { flag, frames });
        }
        let pos = read_pos_f32(c, marker_count)?;
        let rot = read_rot_f32(c, marker_count)?;
        return Ok(Track::Keys {
            pos: PosKeys::F32(pos),
            rot: RotKeys::F32(rot),
        });
    }
    let (pos_i16, rot_i16) = codec_i16(codec);
    let n = c.count("position key", if pos_i16 { 6 } else { 12 })?;
    let pos = if pos_i16 {
        PosKeys::I16((0..n).map(|_| c.i16s::<3>()).collect::<Result<_, _>>()?)
    } else {
        PosKeys::F32(read_pos_f32(c, n)?)
    };
    let n = c.count("rotation key", if rot_i16 { 8 } else { 16 })?;
    let rot = if rot_i16 {
        RotKeys::I16((0..n).map(|_| c.i16s::<4>()).collect::<Result<_, _>>()?)
    } else {
        RotKeys::F32(read_rot_f32(c, n)?)
    };
    Ok(Track::Keys { pos, rot })
}
