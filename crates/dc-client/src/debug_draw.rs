//! Debug primitives: solid boxes drawn into the finished world frame (colour and depth loaded, depth tested and
//! written), with the world pass's camera and depth range. The client draws remote players as one box each until
//! it has models (RENDER_2009.md §7 is not implemented yet); the pass lives here, not in dc-render, because it is
//! a client debugging aid.
// SPEC: RENDER_2009.md §4.1 (camera matrices, through dc-render's Camera), §4.3 (world depth range [0.1, 1.0])

use dc_render::{Camera, FrameTarget, GpuContext, WORLD_DEPTH_RANGE};

const BOXES_WGSL: &str = r#"
struct Frame {
    clip_from_world: mat4x4<f32>,
    light: vec4<f32>,
};
@group(0) @binding(0) var<uniform> frame: Frame;

struct VIn {
    @location(0) corner: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) centre: vec3<f32>,
    @location(3) half: vec3<f32>,
    @location(4) colour: vec4<f32>,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) colour: vec4<f32>,
};

@vertex
fn vs_box(v: VIn) -> VOut {
    var o: VOut;
    let world = v.centre + v.corner * v.half;
    o.pos = frame.clip_from_world * vec4<f32>(world, 1.0);
    let lit = 0.55 + 0.45 * max(dot(v.normal, frame.light.xyz), 0.0);
    o.colour = vec4<f32>(v.colour.rgb * lit, v.colour.a);
    return o;
}

@fragment
fn fs_box(v: VOut) -> @location(0) vec4<f32> {
    return v.colour;
}
"#;

/// One box: centre, half extents and an RGBA colour (0..1).
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct BoxInstance {
    pub centre: [f32; 3],
    pub half: [f32; 3],
    pub colour: [f32; 4],
}

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct CubeVertex {
    corner: [f32; 3],
    normal: [f32; 3],
}

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct FrameUniform {
    clip_from_world: [[f32; 4]; 4],
    light: [f32; 4],
}

/// The unit cube [-1, 1]^3: six faces of two triangles, each with its face normal.
fn cube() -> Vec<CubeVertex> {
    let mut v = Vec::with_capacity(36);
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut n = [0.0; 3];
            n[axis] = sign;
            let (u, w) = ((axis + 1) % 3, (axis + 2) % 3);
            let corner = |a: f32, b: f32| {
                let mut c = [0.0; 3];
                c[axis] = sign;
                c[u] = a;
                c[w] = b;
                CubeVertex {
                    corner: c,
                    normal: n,
                }
            };
            for (a, b) in [
                (-1.0, -1.0),
                (1.0, -1.0),
                (1.0, 1.0),
                (-1.0, -1.0),
                (1.0, 1.0),
                (-1.0, 1.0),
            ] {
                v.push(corner(a, b));
            }
        }
    }
    v
}

/// The box pass for one colour format (depth format Depth32Float, as dc-render's world pass).
pub struct DebugBoxes {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
    cube: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: usize,
    /// CLI-1593: samples per pixel of the frame target the pipeline draws into (1: MSAA off).
    msaa: u32,
    /// Boxes drawn by the last [`DebugBoxes::encode`].
    pub last_drawn: usize,
}

const BOX_DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

fn instance_buffer(device: &wgpu::Device, n: usize) -> wgpu::Buffer {
    dc_render::perf::buffer(
        device,
        &wgpu::BufferDescriptor {
            label: Some("debug box instances"),
            size: (n.max(1) * std::mem::size_of::<BoxInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        },
    )
}

impl DebugBoxes {
    /// The pass for a single-sample target (MSAA off).
    pub fn new(ctx: &GpuContext, format: wgpu::TextureFormat) -> DebugBoxes {
        Self::with_msaa(ctx, format, 1)
    }

    /// CLI-1593: the pass for the frame's 3D target of `format` with `msaa` samples per pixel (the world renderer's
    /// [`dc_render::WorldRenderer::msaa`]).
    pub fn with_msaa(ctx: &GpuContext, format: wgpu::TextureFormat, msaa: u32) -> DebugBoxes {
        let msaa = msaa.max(1);
        let device = &ctx.device;
        let module = dc_render::shader_module(
            device,
            wgpu::ShaderModuleDescriptor {
                label: Some("debug_boxes.wgsl"),
                source: wgpu::ShaderSource::Wgsl(BOXES_WGSL.into()),
            },
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug boxes"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform = dc_render::perf::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("debug boxes frame"),
                size: std::mem::size_of::<FrameUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("debug boxes"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug boxes"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let cube_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<CubeVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
        };
        let inst_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BoxInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![2 => Float32x3, 3 => Float32x3, 4 => Float32x4],
        };
        let pipeline = dc_render::render_pipeline(
            device,
            &wgpu::RenderPipelineDescriptor {
                label: Some("debug boxes"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_box"),
                    compilation_options: Default::default(),
                    buffers: &[cube_layout, inst_layout],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_box"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                // both windings drawn: the cube is closed, so the depth test hides the far faces
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: BOX_DEPTH,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                // CLI-1593: the sample count of the frame's 3D target (1: the default state, MSAA off)
                multisample: dc_render::multisample_state(msaa),
                multiview: None,
                cache: None,
            },
        );
        let cube = dc_render::perf::buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("debug unit cube"),
                contents: bytemuck::cast_slice(&cube()),
                usage: wgpu::BufferUsages::VERTEX,
            },
        );
        DebugBoxes {
            pipeline,
            uniform,
            group,
            cube,
            instances: instance_buffer(device, 16),
            capacity: 16,
            msaa,
            last_drawn: 0,
        }
    }

    /// Samples per pixel the pipeline was built for (1: MSAA off).
    pub fn msaa(&self) -> u32 {
        self.msaa
    }

    /// Records the box pass over `colour` / `depth` (both loaded): the world pass must have run first. CLI-1593: with
    /// `target.resolve` set this is the frame's last 3D pass and resolves the multisampled frame (the resolve alone
    /// when no box survives the filter).
    pub fn encode(
        &mut self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: FrameTarget<'_>,
        camera: &Camera,
        boxes: &[BoxInstance],
    ) {
        let size = target.size;
        self.last_drawn = 0;
        let boxes: Vec<BoxInstance> = boxes
            .iter()
            .copied()
            .filter(|b| {
                b.centre.iter().chain(b.half.iter()).all(|v| v.is_finite())
                    && b.half.iter().all(|h| *h > 0.0)
            })
            .collect();
        if boxes.is_empty() || size.0 == 0 || size.1 == 0 {
            if size.0 != 0 && size.1 != 0 {
                target.encode_resolve_only(encoder);
            }
            return;
        }
        if boxes.len() > self.capacity {
            self.capacity = boxes.len().next_power_of_two();
            self.instances = instance_buffer(&ctx.device, self.capacity);
            dc_render::perf::regrow();
        }
        let l = glam::Vec3::new(0.35, 0.85, -0.4).normalize();
        let u = FrameUniform {
            clip_from_world: camera.view_projection().to_cols_array_2d(),
            light: [l.x, l.y, l.z, 0.0],
        };
        dc_render::perf::write_buffer(&ctx.queue, &self.uniform, 0, bytemuck::bytes_of(&u));
        dc_render::perf::write_buffer(&ctx.queue, &self.instances, 0, bytemuck::cast_slice(&boxes));
        let stamp = dc_render::perf::pass_stamp("boxes");
        // CLI-1593: colour and depth loaded; stored, or resolved and discarded when this pass ends the 3D frame
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("debug boxes"),
            color_attachments: &[target.colour_attachment(wgpu::LoadOp::Load)],
            depth_stencil_attachment: target.depth_attachment(wgpu::LoadOp::Load),
            timestamp_writes: stamp.as_ref().map(dc_render::perf::PassStamp::writes),
            occlusion_query_set: None,
        });
        pass.set_viewport(
            0.0,
            0.0,
            size.0 as f32,
            size.1 as f32,
            WORLD_DEPTH_RANGE.0,
            WORLD_DEPTH_RANGE.1,
        );
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_vertex_buffer(0, self.cube.slice(..));
        pass.set_vertex_buffer(1, self.instances.slice(..));
        pass.draw(0..36, 0..boxes.len() as u32);
        dc_render::perf::add(dc_render::perf::Counter::ClientDraws, 1);
        self.last_drawn = boxes.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_faces_are_closed_and_outward() {
        let c = cube();
        assert_eq!(c.len(), 36);
        for v in &c {
            // every corner lies on the face its normal names
            let k = v.normal.iter().position(|n| *n != 0.0).unwrap();
            assert_eq!(v.corner[k], v.normal[k]);
            assert!(v.corner.iter().all(|x| x.abs() == 1.0));
        }
    }
}
