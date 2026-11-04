//! X-ray styles (DC_DEFAULTS CLI-1624): how a declared x-ray server's render mod (CLI-1620) shows a player through
//! the world. Three styles, each with two colours:
//!
//! - [`XrayMode::Box`]: the screen rectangle of the posed model, filled with the primary colour (its alpha divided
//!   by [`BOX_FILL_ALPHA_DIV`]) and bordered ([`BOX_BORDER_PX`]) with the secondary colour. The match draws its boxes
//!   as a HUD element from the server's body box (`dc-client` `hud::xray`); this renderer path is for a menu scene
//!   (the shop preview), which has a model but no HUD.
//! - [`XrayMode::Chams`]: the model's triangles drawn flat, without texture or lighting, in two tones by facing:
//!   `|N . V| >= `[`CHAMS_FACING`] (surfaces turned to the camera) in the primary colour, the grazing rim in the
//!   secondary colour.
//! - [`XrayMode::Skeleton`]: a [`LINE_PX`] line in the primary colour from every node of the first instance's posed
//!   skeleton to its parent (the root node, which has no parent, and the lines to it are left out), and a
//!   [`JOINT_PX`] square in the secondary colour on every drawn node (joints and appendage ends).
//!
//! All three are drawn by [`crate::WorldRenderer::encode_xray`] in a pass of their own after the frame's 3D pass:
//! colour blended by alpha, depth tested LESS at the near end of the world depth range and not written, so they show
//! through every wall and model but never over the first-person models (depth range [0, 0.1]).
//!
//! Provenance: ORIGINAL (DeCombat's own mod rendering; the 2009 client has no such styles).
// DC-DEFAULT: CLI-1624 (proposed) the x-ray styles box / chams / skeleton and their two colours

use crate::camera::Camera;
use crate::gpu::{FrameTarget, GpuContext};
use crate::model::ModelVertex;
use crate::model_gpu::{ModelInstance, ModelPipelines};
use bytemuck::{Pod, Zeroable};
use glam::{Vec3, Vec4};

const XRAY_WGSL: &str = include_str!("shaders/xray.wgsl");

/// A chams surface whose `|N . V|` is at or above this is drawn in the primary colour, below in the secondary.
pub const CHAMS_FACING: f32 = 0.35;
/// Width of a skeleton line (pixels).
pub const LINE_PX: f32 = 2.0;
/// Side of a skeleton joint square (pixels).
pub const JOINT_PX: f32 = 5.0;
/// Width of a box border (pixels).
pub const BOX_BORDER_PX: f32 = 2.0;
/// A box's fill alpha is the primary colour's alpha divided by this (the box stays see-through).
pub const BOX_FILL_ALPHA_DIV: u32 = 3;

/// The x-ray style of a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum XrayMode {
    /// The screen box (fill primary, border secondary).
    #[default]
    Box,
    /// The flat two-tone model through walls.
    Chams,
    /// The node lines through walls.
    Skeleton,
}

impl XrayMode {
    /// Every mode in the `/wallhack` cycle order.
    pub const ALL: [XrayMode; 3] = [XrayMode::Box, XrayMode::Chams, XrayMode::Skeleton];

    /// The lower-case name (profile value, mod answer, HUD notice).
    pub fn name(self) -> &'static str {
        match self {
            XrayMode::Box => "box",
            XrayMode::Chams => "chams",
            XrayMode::Skeleton => "skeleton",
        }
    }

    /// The mode of a name (ASCII case ignored, surrounding blanks trimmed).
    pub fn parse(s: &str) -> Option<XrayMode> {
        let s = s.trim();
        XrayMode::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(s))
    }

    /// The next mode of the cycle (box, chams, skeleton, box).
    pub fn next(self) -> XrayMode {
        match self {
            XrayMode::Box => XrayMode::Chams,
            XrayMode::Chams => XrayMode::Skeleton,
            XrayMode::Skeleton => XrayMode::Box,
        }
    }
}

/// One player to draw in an x-ray style: its posed instances (`instances[0]` is the body: the skeleton and the box
/// are taken from it; chams draw every instance), the mode and the two colours (RGBA 0..255).
///
/// Every instance must also be in the frame's model list of the same encoder (its matrices and bone palette are the
/// ones that frame wrote).
#[derive(Debug, Clone)]
pub struct XrayModel<'a> {
    pub instances: Vec<&'a ModelInstance>,
    pub mode: XrayMode,
    pub primary: [u8; 4],
    pub secondary: [u8; 4],
}

/// The pipelines of the x-ray pass (built on first use).
pub(crate) struct XrayPipes {
    chams: wgpu::RenderPipeline,
    quads: wgpu::RenderPipeline,
    tones_layout: wgpu::BindGroupLayout,
    /// CLI-1635: the buffers and vertex list kept across frames.
    reuse: std::sync::Mutex<Reuse>,
}

/// CLI-1635: what the x-ray pass keeps across frames instead of creating it per frame.
#[derive(Default)]
struct Reuse {
    tones: Option<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    quads: Option<(wgpu::Buffer, u64)>,
    verts: Vec<QuadVertex>,
    bytes: Vec<u8>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Tones {
    primary: [f32; 4],
    secondary: [f32; 4],
    params: [f32; 4],
}

/// Stride of the per-draw tone blocks (the dynamic-offset alignment every adapter accepts).
const TONES_STRIDE: u64 = 256;

/// One corner of a screen quad: normalised device coordinates and an RGBA colour (0..1).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct QuadVertex {
    pub ndc: [f32; 2],
    pub colour: [f32; 4],
}

fn rgba(c: [u8; 4]) -> [f32; 4] {
    c.map(|v| v as f32 / 255.0)
}

impl XrayPipes {
    pub(crate) fn new(
        ctx: &GpuContext,
        models: &ModelPipelines,
        colour_format: wgpu::TextureFormat,
        msaa: u32,
    ) -> XrayPipes {
        let device = &ctx.device;
        let module = crate::determinism::shader_module(
            device,
            wgpu::ShaderModuleDescriptor {
                label: Some("xray.wgsl"),
                source: wgpu::ShaderSource::Wgsl(XRAY_WGSL.into()),
            },
        );
        let tones_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xray tones"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Tones>() as u64),
                },
                count: None,
            }],
        });
        let chams_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xray chams"),
            bind_group_layouts: &[models.instance_layout(), &tones_layout],
            push_constant_ranges: &[],
        });
        let quad_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xray quads"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let target = [Some(wgpu::ColorTargetState {
            format: colour_format,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let depth = Some(wgpu::DepthStencilState {
            format: models.zbuf_format(),
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });
        let pipeline = |label: &str,
                        layout: &wgpu::PipelineLayout,
                        vs: &str,
                        fs: &str,
                        buffers: &[wgpu::VertexBufferLayout<'_>]| {
            crate::determinism::render_pipeline(
                device,
                &wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some(vs),
                        compilation_options: Default::default(),
                        buffers,
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(fs),
                        compilation_options: Default::default(),
                        targets: &target,
                    }),
                    // both windings: the flat tones do not depend on which side faces the camera
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: depth.clone(),
                    multisample: crate::msaa::multisample_state(msaa),
                    multiview: None,
                    cache: None,
                },
            )
        };
        let chams = pipeline(
            "xray chams",
            &chams_layout,
            "vs_chams",
            "fs_chams",
            &[ModelVertex::layout()],
        );
        let quads = pipeline(
            "xray quads",
            &quad_layout,
            "vs_quad",
            "fs_quad",
            &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<QuadVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
            }],
        );
        XrayPipes {
            chams,
            quads,
            tones_layout,
            reuse: Default::default(),
        }
    }

    /// Records the x-ray pass of `draws` into `target` (colour and depth loaded). Returns the draw calls recorded.
    pub(crate) fn encode(
        &self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: FrameTarget<'_>,
        camera: &Camera,
        draws: &[XrayModel<'_>],
    ) -> u64 {
        let chams: Vec<&XrayModel<'_>> = draws
            .iter()
            .filter(|d| d.mode == XrayMode::Chams && !d.instances.is_empty())
            .collect();
        let mut reuse = self.reuse.lock().unwrap_or_else(|e| e.into_inner());
        let reuse = &mut *reuse;
        reuse.verts.clear();
        let view_projection = camera.view_projection();
        for d in draws {
            push_quads_vp(&mut reuse.verts, view_projection, target.size, d);
        }
        if chams.is_empty() && reuse.verts.is_empty() {
            target.encode_resolve_only(encoder);
            return 0;
        }
        let device = &ctx.device;
        let tones_group = (!chams.is_empty()).then(|| {
            reuse.bytes.clear();
            reuse.bytes.resize(chams.len() * TONES_STRIDE as usize, 0);
            for (i, d) in chams.iter().enumerate() {
                let t = Tones {
                    primary: rgba(d.primary),
                    secondary: rgba(d.secondary),
                    params: [CHAMS_FACING, 0.0, 0.0, 0.0],
                };
                let at = i * TONES_STRIDE as usize;
                reuse.bytes[at..at + std::mem::size_of::<Tones>()]
                    .copy_from_slice(bytemuck::bytes_of(&t));
            }
            let need = reuse.bytes.len() as u64;
            if reuse.tones.as_ref().is_none_or(|t| t.2 < need) {
                let size = need.next_power_of_two().max(32 * TONES_STRIDE);
                let buf = crate::perf::buffer(
                    device,
                    &wgpu::BufferDescriptor {
                        label: Some("xray tones"),
                        size,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    },
                );
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("xray tones"),
                    layout: &self.tones_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buf,
                            offset: 0,
                            size: wgpu::BufferSize::new(std::mem::size_of::<Tones>() as u64),
                        }),
                    }],
                });
                reuse.tones = Some((buf, group, size));
            }
            let t = reuse.tones.as_ref().expect("made above");
            crate::perf::write_buffer(&ctx.queue, &t.0, 0, &reuse.bytes);
            t.1.clone()
        });
        let quad_count = reuse.verts.len() as u32;
        let quad_buf = (quad_count > 0).then(|| {
            let bytes: &[u8] = bytemuck::cast_slice(&reuse.verts);
            let need = bytes.len() as u64;
            if reuse.quads.as_ref().is_none_or(|q| q.1 < need) {
                let size = need.next_power_of_two().max(64 * 1024);
                let buf = crate::perf::buffer(
                    device,
                    &wgpu::BufferDescriptor {
                        label: Some("xray quads"),
                        size,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    },
                );
                reuse.quads = Some((buf, size));
            }
            let q = reuse.quads.as_ref().expect("made above");
            crate::perf::write_buffer(&ctx.queue, &q.0, 0, bytes);
            q.0.clone()
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("xray"),
            color_attachments: &[target.colour_attachment(wgpu::LoadOp::Load)],
            depth_stencil_attachment: target.depth_attachment(wgpu::LoadOp::Load),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_viewport(
            0.0,
            0.0,
            target.size.0 as f32,
            target.size.1 as f32,
            0.0,
            1.0,
        );
        let mut calls = 0u64;
        if let Some(g) = &tones_group {
            pass.set_pipeline(&self.chams);
            for (i, d) in chams.iter().enumerate() {
                pass.set_bind_group(1, g, &[(i as u64 * TONES_STRIDE) as u32]);
                for inst in &d.instances {
                    calls += inst.record_flat(&mut pass, camera.pos);
                }
            }
        }
        if let Some(b) = &quad_buf {
            pass.set_pipeline(&self.quads);
            pass.set_vertex_buffer(
                0,
                b.slice(..u64::from(quad_count) * std::mem::size_of::<QuadVertex>() as u64),
            );
            pass.draw(0..quad_count, 0..1);
            calls += 1;
        }
        calls
    }
}

/// A world point on the target in pixels (y down), `None` behind the camera.
pub fn project_px(camera: &Camera, size: (u32, u32), p: Vec3) -> Option<[f32; 2]> {
    project_vp(camera.view_projection(), size, p)
}

fn project_vp(view_projection: glam::Mat4, size: (u32, u32), p: Vec3) -> Option<[f32; 2]> {
    let c = view_projection * Vec4::new(p.x, p.y, p.z, 1.0);
    if c.w <= 1e-4 {
        return None;
    }
    Some([
        (c.x / c.w + 1.0) * 0.5 * size.0 as f32,
        (1.0 - c.y / c.w) * 0.5 * size.1 as f32,
    ])
}

/// The posed skeleton of `inst` in world space: the bone lines (parent, node) and the joint points; the root
/// (no parent) and the lines to it are left out.
pub fn skeleton_world(inst: &ModelInstance) -> (Vec<(Vec3, Vec3)>, Vec<Vec3>) {
    let world = inst.world();
    let parents = &inst.anim.skeleton.parents;
    let at = |i: usize| -> Option<Vec3> {
        let g = inst.globals.get(i)?;
        Some(world.transform_point3(g.w_axis.truncate()))
    };
    let mut bones = Vec::new();
    let mut joints = Vec::new();
    for (i, parent) in parents.iter().enumerate() {
        let Some(parent) = *parent else { continue };
        let Some(p) = at(i).filter(|p| p.is_finite()) else {
            continue;
        };
        joints.push(p);
        if parents.get(parent).is_some_and(|pp| pp.is_some()) {
            if let Some(q) = at(parent).filter(|q| q.is_finite()) {
                bones.push((q, p));
            }
        }
    }
    (bones, joints)
}

fn px_to_ndc(size: (u32, u32), p: [f32; 2]) -> [f32; 2] {
    [
        p[0] / size.0.max(1) as f32 * 2.0 - 1.0,
        1.0 - p[1] / size.1.max(1) as f32 * 2.0,
    ]
}

/// Two triangles over the pixel corners `c` (in order around the quad).
fn push_quad(out: &mut Vec<QuadVertex>, size: (u32, u32), c: [[f32; 2]; 4], colour: [f32; 4]) {
    for k in [0, 1, 2, 0, 2, 3] {
        out.push(QuadVertex {
            ndc: px_to_ndc(size, c[k]),
            colour,
        });
    }
}

/// A screen rectangle `[x, y, w, h]` in pixels.
fn push_rect(out: &mut Vec<QuadVertex>, size: (u32, u32), r: [f32; 4], colour: [f32; 4]) {
    let [x, y, w, h] = r;
    push_quad(
        out,
        size,
        [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
        colour,
    );
}

/// A line of `width` pixels from `a` to `b` (pixels).
fn push_line(
    out: &mut Vec<QuadVertex>,
    size: (u32, u32),
    a: [f32; 2],
    b: [f32; 2],
    width: f32,
    colour: [f32; 4],
) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len.is_nan() || len <= 1e-3 {
        return;
    }
    let (nx, ny) = (-dy / len * width * 0.5, dx / len * width * 0.5);
    push_quad(
        out,
        size,
        [
            [a[0] + nx, a[1] + ny],
            [b[0] + nx, b[1] + ny],
            [b[0] - nx, b[1] - ny],
            [a[0] - nx, a[1] - ny],
        ],
        colour,
    );
}

/// The screen quads of one model's skeleton or box (chams draw no quads).
pub fn push_model_quads(
    out: &mut Vec<QuadVertex>,
    camera: &Camera,
    size: (u32, u32),
    d: &XrayModel<'_>,
) {
    push_quads_vp(out, camera.view_projection(), size, d);
}

fn push_quads_vp(
    out: &mut Vec<QuadVertex>,
    view_projection: glam::Mat4,
    size: (u32, u32),
    d: &XrayModel<'_>,
) {
    let Some(body) = d.instances.first() else {
        return;
    };
    let (primary, secondary) = (rgba(d.primary), rgba(d.secondary));
    match d.mode {
        XrayMode::Chams => {}
        XrayMode::Skeleton => {
            // the lines first, the joints over them; no list per model (CLI-1635)
            let world = body.world();
            let parents = &body.anim.skeleton.parents;
            let at = |i: usize| -> Option<[f32; 2]> {
                let g = body.globals.get(i)?;
                let p = world.transform_point3(g.w_axis.truncate());
                if !p.is_finite() {
                    return None;
                }
                project_vp(view_projection, size, p)
            };
            for (i, parent) in parents.iter().enumerate() {
                let Some(parent) = *parent else { continue };
                if parents.get(parent).is_some_and(|pp| pp.is_some()) {
                    if let (Some(a), Some(b)) = (at(parent), at(i)) {
                        push_line(out, size, a, b, LINE_PX, primary);
                    }
                }
            }
            let h = JOINT_PX * 0.5;
            for (i, parent) in parents.iter().enumerate() {
                if parent.is_none() {
                    continue;
                }
                if let Some(p) = at(i) {
                    push_rect(
                        out,
                        size,
                        [p[0] - h, p[1] - h, JOINT_PX, JOINT_PX],
                        secondary,
                    );
                }
            }
        }
        XrayMode::Box => {
            let Some((lo, hi)) = body.bounds_in_world() else {
                return;
            };
            let (mut a, mut b) = ([f32::MAX; 2], [f32::MIN; 2]);
            for k in 0..8 {
                let c = Vec3::new(
                    if k & 1 == 0 { lo.x } else { hi.x },
                    if k & 2 == 0 { lo.y } else { hi.y },
                    if k & 4 == 0 { lo.z } else { hi.z },
                );
                let Some(p) = project_vp(view_projection, size, c) else {
                    return;
                };
                for i in 0..2 {
                    a[i] = a[i].min(p[i]);
                    b[i] = b[i].max(p[i]);
                }
            }
            let (w, h) = (b[0] - a[0], b[1] - a[1]);
            if !(w > 0.0 && h > 0.0) {
                return;
            }
            let mut fill = primary;
            fill[3] = (d.primary[3] as u32 / BOX_FILL_ALPHA_DIV) as f32 / 255.0;
            push_rect(out, size, [a[0], a[1], w, h], fill);
            let t = BOX_BORDER_PX;
            for r in [
                [a[0], a[1], w, t],
                [a[0], b[1] - t, w, t],
                [a[0], a[1], t, h],
                [b[0] - t, a[1], t, h],
            ] {
                push_rect(out, size, r, secondary);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_cycle_and_parse() {
        assert_eq!(XrayMode::default(), XrayMode::Box);
        assert_eq!(XrayMode::Box.next(), XrayMode::Chams);
        assert_eq!(XrayMode::Chams.next(), XrayMode::Skeleton);
        assert_eq!(XrayMode::Skeleton.next(), XrayMode::Box);
        for m in XrayMode::ALL {
            assert_eq!(XrayMode::parse(m.name()), Some(m));
        }
        assert_eq!(XrayMode::parse(" CHAMS "), Some(XrayMode::Chams));
        assert_eq!(XrayMode::parse("glow"), None);
    }

    #[test]
    fn a_line_is_two_triangles_of_its_width() {
        let mut out = Vec::new();
        push_line(
            &mut out,
            (100, 100),
            [10.0, 50.0],
            [90.0, 50.0],
            2.0,
            [1.0; 4],
        );
        assert_eq!(out.len(), 6);
        let (lo, hi) = out
            .iter()
            .map(|v| v.ndc[1])
            .fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(y), b.max(y)));
        assert!(((hi - lo) - 0.04).abs() < 1e-5, "2 px of 100: {lo} {hi}");
        out.clear();
        push_line(
            &mut out,
            (100, 100),
            [10.0, 50.0],
            [10.0, 50.0],
            2.0,
            [1.0; 4],
        );
        assert!(out.is_empty(), "a zero-length line draws nothing");
    }
}
