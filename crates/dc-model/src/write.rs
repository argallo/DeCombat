//! The writer: the exact inverse of the reader. Stored `declared_body_len` values and opaque fields are
//! written back unchanged, so a parsed file re-encodes to its original bytes.

use crate::cursor::Out;
use crate::error::ModelError;
use crate::model::*;
use crate::vertex::{stride, VertexStream};

fn enc(what: &'static str) -> ModelError {
    ModelError::Encode { what }
}

impl Model {
    /// Encodes the model in the section order of FMT_LTB.md §4.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ModelError> {
        let mut o = Out::default();
        let h = &self.header;
        o.u8(h.file_kind);
        o.u8(h.todo_pad);
        o.u16(h.container_rev);
        o.bytes(&h.todo_reserved);
        o.u32(h.model_rev);
        for v in self.hints.0 {
            o.u32(v);
        }
        o.string(&self.command)?;
        o.f32(self.radius);
        o.len_u32(self.todo_boxes.len(), "box count")?;
        for b in &self.todo_boxes {
            o.bytes(b);
        }
        if self.pieces.len() > MAX_PIECES as usize {
            return Err(enc("more than 64 pieces"));
        }
        o.len_u32(self.pieces.len(), "piece count")?;
        for p in &self.pieces {
            write_piece(&mut o, p)?;
        }
        write_nodes(&mut o, &self.nodes)?;
        o.len_u32(self.blend_sets.len(), "blend set count")?;
        for s in &self.blend_sets {
            o.string(&s.name)?;
            if s.weights.len() != self.nodes.len() {
                return Err(enc("blend set weight count differs from node count"));
            }
            o.len_u32(s.weights.len(), "blend weights")?;
            o.f32s(&s.weights);
        }
        let slots = self.child_models.len() + 1;
        if slots > MAX_CHILD_SLOTS as usize {
            return Err(enc("more than 32 child slots"));
        }
        o.len_u32(slots, "child slots")?;
        for p in &self.child_models {
            o.string(p)?;
        }
        o.len_u32(self.clips.len(), "clip count")?;
        for clip in &self.clips {
            write_clip(&mut o, clip, self.nodes.len())?;
        }
        o.len_u32(self.sockets.len(), "attach count")?;
        for s in &self.sockets {
            if s.node as usize >= self.nodes.len() {
                return Err(enc("attach point names a missing node"));
            }
            o.u32(s.node);
            o.string(&s.name)?;
            o.f32s(&s.rot);
            o.f32s(&s.pos);
            o.f32s(&s.scale);
        }
        if self.clip_overrides.len() != slots {
            return Err(enc("clip override lists must number child slots"));
        }
        for list in &self.clip_overrides {
            o.len_u32(list.len(), "override count")?;
            for ov in list {
                o.string(&ov.clip_name)?;
                o.f32s(&ov.extent);
                o.f32s(&ov.offset);
            }
        }
        o.bytes(&self.trailing);
        Ok(o.buf)
    }
}

fn write_piece(o: &mut Out, p: &Piece) -> Result<(), ModelError> {
    o.string(&p.name)?;
    if p.lod_dist.len() != p.lods.len() {
        return Err(enc("lod distance count differs from lod count"));
    }
    o.len_u32(p.lods.len(), "lod count")?;
    o.f32s(&p.lod_dist);
    o.u32(p.todo_lod_lo);
    o.u32(p.todo_lod_hi);
    for l in &p.lods {
        o.u32(l.slot_count);
        for s in l.slots {
            o.u32(s);
        }
        o.u32(l.style_index);
        o.u8(l.priority);
        o.u32(l.mesh.kind());
        o.u32(l.declared_body_len);
        write_mesh(o, &l.mesh)?;
        let n = u8::try_from(l.used_nodes.len()).map_err(|_| enc("more than 255 used nodes"))?;
        o.u8(n);
        o.bytes(&l.used_nodes);
    }
    Ok(())
}

fn write_streams(
    o: &mut Out,
    streams: &[VertexStream; 4],
    blend_kind: u32,
    vertex_count: u32,
) -> Result<(), ModelError> {
    for s in streams {
        if s.flags == 0 {
            if !s.data.is_empty() {
                return Err(enc("absent stream carries data"));
            }
            continue;
        }
        let want = (vertex_count as u64) * stride(blend_kind, s.flags) as u64;
        if s.data.len() as u64 != want {
            return Err(enc("stream size differs from vertex_count * stride"));
        }
        o.bytes(&s.data);
    }
    Ok(())
}

fn flags(streams: &[VertexStream; 4], o: &mut Out) {
    for s in streams {
        o.u32(s.flags);
    }
}

fn write_indices(o: &mut Out, idx: &[u16], tri_count: u32) -> Result<(), ModelError> {
    if idx.len() as u64 != 3 * tri_count as u64 {
        return Err(enc("index count differs from 3 * tri_count"));
    }
    for v in idx {
        o.u16(*v);
    }
    Ok(())
}

fn write_mesh(o: &mut Out, mesh: &Mesh) -> Result<(), ModelError> {
    match mesh {
        Mesh::Rigid(m) => {
            o.u32(m.vertex_count);
            o.u32(m.tri_count);
            o.u32(m.todo_influences);
            o.u32(m.todo_unused);
            flags(&m.streams, o);
            o.u32(m.node);
            write_streams(o, &m.streams, 0, m.vertex_count)?;
            write_indices(o, &m.indices, m.tri_count)?;
        }
        Mesh::Skinned(m) => {
            o.u32(m.vertex_count);
            o.u32(m.tri_count);
            o.u32(m.max_influences);
            o.u32(m.palette_influences);
            o.u8(m.has_palette_map);
            flags(&m.streams, o);
            let blend = m
                .blend_kind()
                .ok_or(enc("skinned influence count out of range"))?;
            match &m.tail {
                SkinTail::Spans(spans) => {
                    o.u8(0);
                    write_streams(o, &m.streams, blend, m.vertex_count)?;
                    write_indices(o, &m.indices, m.tri_count)?;
                    o.len_u32(spans.len(), "span count")?;
                    for s in spans {
                        o.u16(s.first_vertex);
                        o.u16(s.vertex_count);
                        o.bytes(&s.nodes);
                        o.u32(s.index_end);
                    }
                }
                SkinTail::Palette {
                    todo_cc,
                    todo_d0,
                    palette_map,
                } => {
                    o.u8(1);
                    o.u32(*todo_cc);
                    o.u32(*todo_d0);
                    match (m.has_palette_map != 0, palette_map) {
                        (true, Some(map)) => {
                            o.len_u32(map.len(), "palette map")?;
                            for v in map {
                                o.u32(*v);
                            }
                        }
                        (false, None) => {}
                        _ => return Err(enc("palette map presence differs from has_palette_map")),
                    }
                    write_streams(o, &m.streams, blend, m.vertex_count)?;
                    write_indices(o, &m.indices, m.tri_count)?;
                }
            }
        }
        Mesh::Morph(m) => {
            o.u32(m.vertex_count);
            o.u32(m.base_vertex_count);
            o.u32(m.tri_count);
            o.u32(m.todo_influences);
            o.u32(m.todo_a8);
            flags(&m.streams, o);
            o.u32(m.todo_a4);
            o.u32(m.node);
            write_streams(o, &m.streams, 0, m.vertex_count)?;
            write_indices(o, &m.indices, m.tri_count)?;
            o.len_u32(m.vertex_map.len(), "morph vertex map")?;
            for v in &m.vertex_map {
                o.u32(*v);
            }
        }
    }
    Ok(())
}

/// Writes the flat node list as a tree; it must be in depth-first file order with consistent links.
fn write_nodes(o: &mut Out, nodes: &[Node]) -> Result<(), ModelError> {
    if nodes.is_empty() {
        return Err(enc("a model needs a root node"));
    }
    // Check that `children` lists reproduce the flat order exactly.
    let mut next = 1usize;
    let mut stack = vec![(0usize, 0usize)];
    o.string(&nodes[0].name)?;
    write_node_fields(o, &nodes[0])?;
    while let Some((n, k)) = stack.pop() {
        let node = &nodes[n];
        if k < node.children.len() {
            stack.push((n, k + 1));
            let ch = node.children[k];
            if ch != next || ch >= nodes.len() || nodes[ch].parent != Some(n) {
                return Err(enc("node links are not in depth-first file order"));
            }
            next += 1;
            o.string(&nodes[ch].name)?;
            write_node_fields(o, &nodes[ch])?;
            stack.push((ch, 0));
        }
    }
    if next != nodes.len() || nodes[0].parent.is_some() {
        return Err(enc("node list is not one tree"));
    }
    Ok(())
}

fn write_node_fields(o: &mut Out, n: &Node) -> Result<(), ModelError> {
    o.u16(n.index);
    o.u8(n.flags);
    o.f32s(&n.model_xform);
    o.len_u32(n.children.len(), "node child count")
}

fn write_clip(o: &mut Out, clip: &Clip, node_count: usize) -> Result<(), ModelError> {
    o.f32s(&clip.extent);
    o.string(&clip.name)?;
    if clip.codec > CODEC_F32_POS_I16_ROT {
        return Err(enc("unknown track codec"));
    }
    o.u32(clip.codec);
    o.u32(clip.blend_ms);
    o.len_u32(clip.markers.len(), "marker count")?;
    for m in &clip.markers {
        o.u32(m.time_ms);
        o.string(&m.command)?;
    }
    if clip.tracks.len() != node_count {
        return Err(enc("track count differs from node count"));
    }
    let mc = clip.markers.len();
    let (pos_i16, rot_i16) = codec_i16(clip.codec);
    for t in &clip.tracks {
        match (clip.codec, t) {
            (CODEC_RAW, Track::Morph { flag, frames }) => {
                if *flag == 0 || frames.len() != mc {
                    return Err(enc(
                        "morph track needs a non-zero flag and one frame per marker",
                    ));
                }
                o.u8(*flag);
                for f in frames {
                    o.len_u32(f.len(), "morph vertices")?;
                    for v in f {
                        o.f32s(v);
                    }
                }
            }
            (
                CODEC_RAW,
                Track::Keys {
                    pos: PosKeys::F32(p),
                    rot: RotKeys::F32(r),
                },
            ) => {
                if p.len() != mc || r.len() != mc {
                    return Err(enc("codec 0 channels need one key per marker"));
                }
                o.u8(0);
                for v in p {
                    o.f32s(v);
                }
                for v in r {
                    o.f32s(v);
                }
            }
            (_, Track::Keys { pos, rot }) if clip.codec != CODEC_RAW => {
                o.len_u32(pos.len(), "position keys")?;
                match (pos, pos_i16) {
                    (PosKeys::F32(v), false) => v.iter().for_each(|k| o.f32s(k)),
                    (PosKeys::I16(v), true) => v.iter().for_each(|k| o.i16s(k)),
                    _ => return Err(enc("position encoding differs from the codec")),
                }
                o.len_u32(rot.len(), "rotation keys")?;
                match (rot, rot_i16) {
                    (RotKeys::F32(v), false) => v.iter().for_each(|k| o.f32s(k)),
                    (RotKeys::I16(v), true) => v.iter().for_each(|k| o.i16s(k)),
                    _ => return Err(enc("rotation encoding differs from the codec")),
                }
            }
            _ => return Err(enc("track shape differs from the codec")),
        }
    }
    Ok(())
}
