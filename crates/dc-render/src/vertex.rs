//! The 68-byte world render vertex and the GPU vertex we build from it.

use bytemuck::{Pod, Zeroable};
use dc_world::RENDER_VERTEX_BYTES;

/// One file vertex, split as OBSERVED in RENDER_2009.md §5.2: position (3 f32 at 0), uv0 (2 f32 at 12), uv1
/// (2 f32 at 20, non-zero only around lightmapped geometry), packed colour (u32 ARGB at 28), normal (3 f32 at
/// 32), two more unit vectors (at 44 and 56, tangent frame of the bump buckets).
// SPEC: RENDER_2009.md §5.2 (68-byte file vertex layout, OBSERVED), FMT_DAT85.md §10.2 item 6
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderVertex {
    pub pos: [f32; 3],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    /// ARGB, alpha in the top byte (D3D colour word).
    pub colour: u32,
    pub normal: [f32; 3],
    pub tangent: [f32; 3],
    pub binormal: [f32; 3],
}

fn f(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn v3(b: &[u8], o: usize) -> [f32; 3] {
    [f(b, o), f(b, o + 4), f(b, o + 8)]
}

impl RenderVertex {
    /// Vertex `index` of a block's vertex bytes, `None` when out of range.
    pub fn read(vertices: &[u8], index: usize) -> Option<RenderVertex> {
        let off = index.checked_mul(RENDER_VERTEX_BYTES)?;
        let b = vertices.get(off..off.checked_add(RENDER_VERTEX_BYTES)?)?;
        Some(RenderVertex {
            pos: v3(b, 0),
            uv0: [f(b, 12), f(b, 16)],
            uv1: [f(b, 20), f(b, 24)],
            colour: u32::from_le_bytes([b[28], b[29], b[30], b[31]]),
            normal: v3(b, 32),
            tangent: v3(b, 44),
            binormal: v3(b, 56),
        })
    }

    /// Serialises to the 68-byte file layout (synthetic worlds).
    pub fn to_bytes(&self) -> [u8; RENDER_VERTEX_BYTES] {
        let mut o = [0u8; RENDER_VERTEX_BYTES];
        let mut put = |at: usize, v: f32| o[at..at + 4].copy_from_slice(&v.to_le_bytes());
        for k in 0..3 {
            put(4 * k, self.pos[k]);
            put(32 + 4 * k, self.normal[k]);
            put(44 + 4 * k, self.tangent[k]);
            put(56 + 4 * k, self.binormal[k]);
        }
        put(12, self.uv0[0]);
        put(16, self.uv0[1]);
        put(20, self.uv1[0]);
        put(24, self.uv1[1]);
        o[28..32].copy_from_slice(&self.colour.to_le_bytes());
        o
    }

    /// Colour as RGBA bytes (the D3D ARGB word reordered for a `Unorm8x4` attribute).
    pub fn rgba(&self) -> [u8; 4] {
        let c = self.colour;
        [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]
    }
}

/// GPU vertex: one layout for every world bucket of this slice (position, colour, uv0, uv1). The 2009 engine
/// converts into one of 16 per-bucket layouts (RENDER_2009.md §5.2 vertex formats); a superset is equivalent
/// for the buckets drawn here.
// SPEC: RENDER_2009.md §5.2 (vertex formats: position, colour, uv0, uv1)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GpuVertex {
    pub pos: [f32; 3],
    pub colour: [u8; 4],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
}

impl GpuVertex {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Unorm8x4,
        2 => Float32x2,
        3 => Float32x2,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

impl From<&RenderVertex> for GpuVertex {
    fn from(v: &RenderVertex) -> Self {
        GpuVertex {
            pos: v.pos,
            colour: v.rgba(),
            uv0: v.uv0,
            uv1: v.uv1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_vertex_round_trip() {
        let v = RenderVertex {
            pos: [1.0, -2.0, 3.5],
            uv0: [0.25, 0.75],
            uv1: [0.5, 0.125],
            colour: 0x80ff4020,
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0],
            binormal: [0.0, 0.0, 1.0],
        };
        let b = v.to_bytes();
        assert_eq!(RenderVertex::read(&b, 0), Some(v));
        assert_eq!(RenderVertex::read(&b, 1), None);
        assert_eq!(v.rgba(), [0xff, 0x40, 0x20, 0x80]);
        assert_eq!(std::mem::size_of::<GpuVertex>(), 32);
    }
}
