//! Vertex stream layouts: stream flags plus blend kind -> vertex code -> byte stride.

/// Stream flag: positions present (together with [`FLAG_NORMAL`]).
// SPEC: FMT_LTB.md §6.1 (0x01 and 0x02 both set -> position + normal)
pub const FLAG_POSITION: u32 = 0x01;
/// Stream flag: normals present (together with [`FLAG_POSITION`]).
pub const FLAG_NORMAL: u32 = 0x02;
/// Stream flags: 1..4 texture-coordinate pairs; the first match in this order wins.
// SPEC: FMT_LTB.md §6.1 (0x10 / 0x20 / 0x40 / 0x80); EVIDENCE: engine+0x15e2f0
pub const FLAG_UV1: u32 = 0x10;
pub const FLAG_UV2: u32 = 0x20;
pub const FLAG_UV3: u32 = 0x40;
pub const FLAG_UV4: u32 = 0x80;
/// Stream flag: 24 extra bytes per vertex (by the spec's reading two vec3 of a tangent frame). Their
/// position inside the vertex is not established, so the per-field accessors refuse such layouts.
// SPEC: FMT_LTB.md §6.1 (0x100 -> +24 bytes, code treated as custom)
pub const FLAG_EXTRA24: u32 = 0x100;

/// How vertex positions carry skinning data: 0 plain, 1..3 that many float blend weights, 4..6 the same
/// plus a trailing dword of 4 x u8 palette indices.
// SPEC: FMT_LTB.md §6.1 (blend kind -> position part)
pub type BlendKind = u32;

/// Flags -> vertex code (public Direct3D FVF bit values). Returns `(code, uv_pairs, extra24)`.
// EVIDENCE: engine+0x15e2f0
pub fn vertex_code(blend_kind: BlendKind, flags: u32) -> (u32, u32, bool) {
    let mut code = 0u32;
    if flags & FLAG_POSITION != 0 && flags & FLAG_NORMAL != 0 {
        code = match blend_kind {
            0 => 0x12,
            1 => 0x16,
            2 => 0x18,
            3 => 0x1a,
            4 => 0x1018,
            5 => 0x101a,
            6 => 0x101c,
            _ => 0,
        };
    }
    let uv = if flags & FLAG_UV1 != 0 {
        1
    } else if flags & FLAG_UV2 != 0 {
        2
    } else if flags & FLAG_UV3 != 0 {
        3
    } else if flags & FLAG_UV4 != 0 {
        4
    } else {
        0
    };
    code |= uv << 8;
    (code, uv, flags & FLAG_EXTRA24 != 0)
}

/// Size in bytes of the position part (xyz plus blend dwords) of a vertex code.
fn position_bytes(code: u32) -> u32 {
    let lo = code & 0xff;
    if lo & 0xc == 0xc {
        28
    } else if lo & 0xa == 0xa {
        24
    } else if lo & 0x8 != 0 {
        20
    } else if lo & 0x4 != 0 {
        16
    } else if lo & 0x2 != 0 {
        12
    } else {
        0
    }
}

/// Vertex code -> byte size (the public FVF size rules as the engine applies them).
// EVIDENCE: engine+0x15e240
pub fn code_size(code: u32) -> u32 {
    let mut n = position_bytes(code);
    if code & 0x10 != 0 {
        n += 12;
    }
    if code & 0x40 != 0 {
        n += 4;
    }
    if code & 0x80 != 0 {
        n += 4;
    }
    n + if code & 0x400 != 0 {
        32
    } else if code & 0x300 == 0x300 {
        24
    } else if code & 0x200 != 0 {
        16
    } else if code & 0x100 != 0 {
        8
    } else {
        0
    }
}

/// Byte stride of one vertex of a stream with `flags` under `blend_kind`.
// SPEC: FMT_LTB.md §6.1 (two-step stride); EVIDENCE: engine+0x15e2f0, engine+0x15e240
pub fn stride(blend_kind: BlendKind, flags: u32) -> u32 {
    let (code, _, extra) = vertex_code(blend_kind, flags);
    code_size(code) + if extra { 24 } else { 0 }
}

/// Field offsets inside one vertex, following the public FVF component order (position, blend dwords,
/// normal, texture pairs). Only built for layouts without the extra 24 bytes, whose placement is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexLayout {
    pub stride: u32,
    /// Bytes of position plus blend dwords (0 when the stream has no positions).
    pub position_bytes: u32,
    /// Float blend weights stored after xyz.
    pub weights: u32,
    /// The last position dword holds 4 x u8 palette indices (blend kinds 4..6).
    pub palette_indices: bool,
    pub with_normals: bool,
    pub uv_pairs: u32,
}

impl VertexLayout {
    pub fn new(blend_kind: BlendKind, flags: u32) -> Option<Self> {
        let (code, uv, extra) = vertex_code(blend_kind, flags);
        if extra {
            return None;
        }
        let pos = position_bytes(code);
        let palette = code & 0x1000 != 0;
        let dwords = pos.saturating_sub(12) / 4;
        let weights = if palette {
            dwords.saturating_sub(1)
        } else {
            dwords
        };
        Some(Self {
            stride: code_size(code),
            position_bytes: pos,
            weights,
            palette_indices: palette && pos > 0,
            with_normals: code & 0x10 != 0,
            uv_pairs: uv,
        })
    }

    pub fn normal_offset(&self) -> Option<u32> {
        self.with_normals.then_some(self.position_bytes)
    }

    pub fn uv_offset(&self, pair: u32) -> Option<u32> {
        (pair < self.uv_pairs)
            .then(|| self.position_bytes + if self.with_normals { 12 } else { 0 } + 8 * pair)
    }
}

fn f32_at(b: &[u8], off: usize) -> Option<f32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// One vertex stream of a render object.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VertexStream {
    pub flags: u32,
    /// `vertex_count * stride` bytes; empty when `flags` is 0.
    pub data: Vec<u8>,
}

impl VertexStream {
    pub fn is_present(&self) -> bool {
        self.flags != 0
    }

    fn vertex(&self, layout: &VertexLayout, i: usize) -> Option<&[u8]> {
        let st = layout.stride as usize;
        let start = i.checked_mul(st)?;
        self.data.get(start..start.checked_add(st)?)
    }

    pub fn position(&self, layout: &VertexLayout, i: usize) -> Option<[f32; 3]> {
        if layout.position_bytes == 0 {
            return None;
        }
        let v = self.vertex(layout, i)?;
        Some([f32_at(v, 0)?, f32_at(v, 4)?, f32_at(v, 8)?])
    }

    /// The stored float blend weights (the FVF convention leaves the last influence's weight implicit).
    pub fn weights(&self, layout: &VertexLayout, i: usize) -> Option<Vec<f32>> {
        let v = self.vertex(layout, i)?;
        (0..layout.weights as usize)
            .map(|k| f32_at(v, 12 + 4 * k))
            .collect()
    }

    pub fn palette_indices(&self, layout: &VertexLayout, i: usize) -> Option<[u8; 4]> {
        if !layout.palette_indices {
            return None;
        }
        let v = self.vertex(layout, i)?;
        let o = 12 + 4 * layout.weights as usize;
        let s = v.get(o..o + 4)?;
        Some([s[0], s[1], s[2], s[3]])
    }

    pub fn normal(&self, layout: &VertexLayout, i: usize) -> Option<[f32; 3]> {
        let o = layout.normal_offset()? as usize;
        let v = self.vertex(layout, i)?;
        Some([f32_at(v, o)?, f32_at(v, o + 4)?, f32_at(v, o + 8)?])
    }

    pub fn uv(&self, layout: &VertexLayout, i: usize, pair: u32) -> Option<[f32; 2]> {
        let o = layout.uv_offset(pair)? as usize;
        let v = self.vertex(layout, i)?;
        Some([f32_at(v, o)?, f32_at(v, o + 4)?])
    }
}
