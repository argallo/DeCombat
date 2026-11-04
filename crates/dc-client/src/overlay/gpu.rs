//! The overlay pass, alpha-blended over the finished world frame (loads the colour target, no depth). Public
//! wgpu API only. Two ways to fill it:
//!
//! - [`OverlayRenderer::update`]: one CPU-composed RGBA image drawn as a single textured quad (uploaded whole on
//!   every call; kept for callers and tests that compose on the CPU).
//! - [`OverlayRenderer::update_items`] (the HUD display list) and [`OverlayRenderer::update_panel`] (debug text):
//!   instanced quads over a texture atlas ([`super::batch`]); per update only the quad list (a few KB) is
//!   uploaded, and only when it changed; textures and glyph sheets are uploaded once.
//!
//! The last update decides what [`OverlayRenderer::encode`] draws.

use super::batch::{Atlas, Quad, QuadList, QUAD_BYTES};
use super::text::{GlyphAtlas, RgbaImage};
use dc_render::GpuContext;

/// The batched quads' shader: per pixel the same values as the CPU compositor (see `super::batch`).
const QUAD_WGSL: &str = include_str!("quads.wgsl");

const OVERLAY_WGSL: &str = r#"
struct Rect {
    ndc: vec4<f32>,  // x0, y0, x1, y1 in clip space
    uv: vec4<f32>,   // u0, v0, u1, v1
};
@group(0) @binding(0) var<uniform> rect: Rect;
@group(0) @binding(1) var panel: texture_2d<f32>;
@group(0) @binding(2) var panel_sampler: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_panel(@builtin(vertex_index) i: u32) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0));
    let c = corners[i];
    var o: VOut;
    o.pos = vec4<f32>(mix(rect.ndc.x, rect.ndc.z, c.x), mix(rect.ndc.y, rect.ndc.w, c.y), 0.0, 1.0);
    o.uv = vec2<f32>(mix(rect.uv.x, rect.uv.z, c.x), mix(rect.uv.y, rect.uv.w, c.y));
    return o;
}

@fragment
fn fs_panel(v: VOut) -> @location(0) vec4<f32> {
    return textureSample(panel, panel_sampler, v.uv);
}
"#;

/// GPU side of the overlay for one colour format.
pub struct OverlayRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    texture: wgpu::Texture,
    group: wgpu::BindGroup,
    tex_size: (u32, u32),
    /// Size of the panel currently in the texture (0 = nothing to draw).
    panel: (u32, u32),
    /// The rectangle last written to `uniform` (rewritten only when it changes).
    last_rect: std::cell::Cell<Option<[f32; 8]>>,
    quads: QuadPass,
    /// What `encode` draws: the image (`false`) or the quad list (`true`).
    use_quads: bool,
}

/// The batched path: pipeline, view uniform, instance buffer, atlas.
struct QuadPass {
    pipeline: wgpu::RenderPipeline,
    view_uniform: wgpu::Buffer,
    view_group: wgpu::BindGroup,
    last_view: std::cell::Cell<Option<[f32; 8]>>,
    /// Bound for runs without a page (fills only).
    empty_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: u64,
    atlas: Atlas,
    /// The list being built and the list in `instances`.
    list: QuadList,
    drawn: QuadList,
    runs: Vec<(u16, u32, u32)>,
    /// Bytes the last build uploaded: (texels, quad instances).
    last_upload: (u64, u64),
}

fn panel_texture(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
    dc_render::perf::texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("overlay panel"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
    )
}

impl OverlayRenderer {
    pub fn new(ctx: &GpuContext, format: wgpu::TextureFormat) -> OverlayRenderer {
        let device = &ctx.device;
        let module = dc_render::shader_module(
            device,
            wgpu::ShaderModuleDescriptor {
                label: Some("overlay.wgsl"),
                source: wgpu::ShaderSource::Wgsl(OVERLAY_WGSL.into()),
            },
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = dc_render::render_pipeline(
            device,
            &wgpu::RenderPipelineDescriptor {
                label: Some("overlay"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_panel"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_panel"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            },
        );
        // pixel-exact panel: nearest filtering
        let sampler = dc_render::sampler(
            device,
            &wgpu::SamplerDescriptor {
                label: Some("overlay"),
                ..Default::default()
            },
        );
        let uniform = dc_render::perf::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("overlay rect"),
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        let texture = panel_texture(device, 1, 1);
        let group = Self::bind(device, &layout, &uniform, &texture, &sampler);
        let quads = QuadPass::new(ctx, format);
        OverlayRenderer {
            pipeline,
            layout,
            sampler,
            uniform,
            texture,
            group,
            tex_size: (1, 1),
            panel: (0, 0),
            last_rect: std::cell::Cell::new(None),
            quads,
            use_quads: false,
        }
    }

    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform: &wgpu::Buffer,
        texture: &wgpu::Texture,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        let view = texture.create_view(&Default::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// Uploads this frame's panel (the texture grows to the next power of two when needed).
    pub fn update(&mut self, ctx: &GpuContext, img: &RgbaImage) {
        self.use_quads = false;
        if img.width == 0 || img.height == 0 {
            self.panel = (0, 0);
            return;
        }
        let max = ctx.device.limits().max_texture_dimension_2d;
        let (w, h) = (img.width.min(max), img.height.min(max));
        if w > self.tex_size.0 || h > self.tex_size.1 {
            let tw = w.next_power_of_two().max(self.tex_size.0).min(max);
            let th = h.next_power_of_two().max(self.tex_size.1).min(max);
            self.texture = panel_texture(&ctx.device, tw, th);
            dc_render::perf::regrow();
            self.tex_size = (tw, th);
            self.group = Self::bind(
                &ctx.device,
                &self.layout,
                &self.uniform,
                &self.texture,
                &self.sampler,
            );
        }
        dc_render::perf::write_texture(
            &ctx.queue,
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img.rgba[..(4 * img.width * h) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * img.width),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        crate::prof::OVERLAY_UPLOAD_BYTES.fetch_add(
            (4 * img.width * h) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        self.panel = (w, h);
    }

    /// The HUD display list `items` on a `size` target as batched quads (textures from `assets`, uploaded into
    /// the atlas the first time and again only when they change). Returns the number of quads.
    pub fn update_items(
        &mut self,
        ctx: &GpuContext,
        items: &[crate::hud::DrawItem],
        assets: &mut crate::hud::HudAssets,
        size: (u32, u32),
    ) -> usize {
        self.use_quads = true;
        self.quads.build(ctx, |atlas, list| {
            super::hud_quads::hud_quads(ctx, atlas, list, items, assets, size.0, size.1)
        })
    }

    /// The debug panel of `lines` in `font` as batched quads (the glyph sheet is uploaded once per font).
    /// Returns the number of quads.
    pub fn update_panel(&mut self, ctx: &GpuContext, font: &GlyphAtlas, lines: &[String]) -> usize {
        self.use_quads = true;
        self.quads.build(ctx, |atlas, list| {
            super::hud_quads::panel_quads(ctx, atlas, list, font, lines)
        })
    }

    /// Quads drawn by the batched path (after the last update).
    pub fn quad_count(&self) -> usize {
        self.quads.drawn.quads.len()
    }

    /// Bytes the last batched update uploaded: (atlas texels, quad instances). Both are 0 when nothing changed.
    pub fn last_upload(&self) -> (u64, u64) {
        self.quads.last_upload
    }

    /// Atlas pages and clears so far (logs, tests).
    pub fn atlas_stats(&self) -> (usize, u64) {
        (self.quads.atlas.page_count(), self.quads.atlas.resets)
    }

    /// Records the overlay pass: the panel at pixel `(x, y)` (top-left) of a `size` target.
    pub fn encode(
        &self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        at: (u32, u32),
    ) {
        if self.use_quads {
            self.quads.encode(ctx, encoder, target, size, at);
            return;
        }
        if self.panel.0 == 0 || size.0 == 0 || size.1 == 0 {
            return;
        }
        let (tw, th) = (size.0 as f32, size.1 as f32);
        let x0 = at.0 as f32 / tw * 2.0 - 1.0;
        let x1 = (at.0 + self.panel.0) as f32 / tw * 2.0 - 1.0;
        let y0 = 1.0 - at.1 as f32 / th * 2.0;
        let y1 = 1.0 - (at.1 + self.panel.1) as f32 / th * 2.0;
        let u1 = self.panel.0 as f32 / self.tex_size.0 as f32;
        let v1 = self.panel.1 as f32 / self.tex_size.1 as f32;
        let rect: [f32; 8] = [x0, y0, x1, y1, 0.0, 0.0, u1, v1];
        if self.last_rect.get() != Some(rect) {
            dc_render::perf::write_buffer(
                &ctx.queue,
                &self.uniform,
                0,
                bytemuck::cast_slice(&rect),
            );
            self.last_rect.set(Some(rect));
        }
        let stamp = dc_render::perf::pass_stamp("panel");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: stamp.as_ref().map(dc_render::perf::PassStamp::writes),
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.draw(0..6, 0..1);
        dc_render::perf::add(dc_render::perf::Counter::ClientDraws, 1);
    }
}

impl QuadPass {
    fn new(ctx: &GpuContext, format: wgpu::TextureFormat) -> QuadPass {
        let device = &ctx.device;
        let module = dc_render::shader_module(
            device,
            wgpu::ShaderModuleDescriptor {
                label: Some("overlay quads.wgsl"),
                source: wgpu::ShaderSource::Wgsl(QUAD_WGSL.into()),
            },
        );
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay quads view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let page_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay atlas page"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay quads"),
            bind_group_layouts: &[&view_layout, &page_layout],
            push_constant_ranges: &[],
        });
        const ATTRS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Uint32x4,
            6 => Float32x4
        ];
        let pipeline = dc_render::render_pipeline(
            device,
            &wgpu::RenderPipelineDescriptor {
                label: Some("overlay quads"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_quad"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: QUAD_BYTES,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &ATTRS,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_quad"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            },
        );
        let view_uniform = dc_render::perf::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("overlay quads view"),
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        let view_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay quads view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_uniform.as_entire_binding(),
            }],
        });
        let empty = panel_texture(device, 1, 1);
        let empty_view = empty.create_view(&Default::default());
        let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay atlas empty"),
            layout: &page_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&empty_view),
            }],
        });
        let capacity = 256;
        let instances = Self::instance_buffer(device, capacity);
        QuadPass {
            pipeline,
            view_uniform,
            view_group,
            last_view: std::cell::Cell::new(None),
            empty_group,
            instances,
            capacity,
            atlas: Atlas::new(ctx, page_layout),
            list: QuadList::default(),
            drawn: QuadList::default(),
            runs: Vec::new(),
            last_upload: (0, 0),
        }
    }

    fn instance_buffer(device: &wgpu::Device, quads: u64) -> wgpu::Buffer {
        dc_render::perf::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("overlay quads"),
                size: quads * QUAD_BYTES,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        )
    }

    /// Builds the list with `fill` (again on a cleared atlas when it ran out of room) and uploads it when it
    /// differs from the list drawn now.
    fn build(
        &mut self,
        ctx: &GpuContext,
        mut fill: impl FnMut(&mut Atlas, &mut QuadList) -> Result<(), super::batch::AtlasFull>,
    ) -> usize {
        use std::sync::atomic::Ordering::Relaxed;
        crate::prof::OVERLAY_BUILDS.fetch_add(1, Relaxed);
        if fill(&mut self.atlas, &mut self.list).is_err() {
            self.atlas.clear();
            if fill(&mut self.atlas, &mut self.list).is_err() {
                // more than the atlas holds at once: what fitted is drawn
                eprintln!("overlay: atlas full, part of the overlay is not drawn");
            }
        }
        let uploaded = self.atlas.take_uploaded();
        let n = self.list.quads.len();
        crate::prof::OVERLAY_QUADS.fetch_add(n as u64, Relaxed);
        let mut bytes = uploaded;
        self.last_upload = (uploaded, 0);
        if self.list != self.drawn {
            if n as u64 > self.capacity {
                self.capacity = (n as u64).next_power_of_two();
                self.instances = Self::instance_buffer(&ctx.device, self.capacity);
                dc_render::perf::regrow();
            }
            if n > 0 {
                let data: &[u8] = bytemuck::cast_slice::<Quad, u8>(&self.list.quads);
                dc_render::perf::write_buffer(&ctx.queue, &self.instances, 0, data);
                bytes += n as u64 * QUAD_BYTES;
                self.last_upload.1 = n as u64 * QUAD_BYTES;
            }
            std::mem::swap(&mut self.list, &mut self.drawn);
            self.runs = self.drawn.runs();
        }
        crate::prof::OVERLAY_UPLOAD_BYTES.fetch_add(bytes, Relaxed);
        n
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        at: (u32, u32),
    ) {
        if self.drawn.quads.is_empty() || size.0 == 0 || size.1 == 0 {
            return;
        }
        let view: [f32; 8] = [
            self.drawn.canvas.0 as f32,
            self.drawn.canvas.1 as f32,
            at.0 as f32,
            at.1 as f32,
            size.0 as f32,
            size.1 as f32,
            0.0,
            0.0,
        ];
        if self.last_view.get() != Some(view) {
            dc_render::perf::write_buffer(
                &ctx.queue,
                &self.view_uniform,
                0,
                bytemuck::cast_slice(&view),
            );
            self.last_view.set(Some(view));
        }
        let stamp = dc_render::perf::pass_stamp("hud");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay quads"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: stamp.as_ref().map(dc_render::perf::PassStamp::writes),
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.view_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        for &(page, first, count) in &self.runs {
            let g = self.atlas.page_group(page).unwrap_or(&self.empty_group);
            pass.set_bind_group(1, g, &[]);
            pass.draw(0..6, first..first + count);
            dc_render::perf::add(dc_render::perf::Counter::ClientDraws, 1);
        }
    }
}
