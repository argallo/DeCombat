//! The gun's picture in the window: the studio draws into an offscreen target on the GPU it shares with egui, and a paint callback
//! samples that target into the viewport's rectangle, as `dc-mapkit`'s viewport does (MAP-62).

use crate::studio::Studio;
use dc_render::OffscreenTarget;
use egui_wgpu::wgpu;

const BLIT_WGSL: &str = r#"
@group(0) @binding(0) var colour: texture_2d<f32>;
@group(0) @binding(1) var colour_sampler: sampler;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: Out;
    o.pos = vec4<f32>(p[i], 0.0, 1.0);
    o.uv = vec2<f32>((p[i].x + 1.0) * 0.5, 1.0 - (p[i].y + 1.0) * 0.5);
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(colour, colour_sampler, i.uv).rgb, 1.0);
}
"#;

struct BlitResources {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind_group: Option<wgpu::BindGroup>,
}

struct BlitCallback;

impl egui_wgpu::CallbackTrait for BlitCallback {
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(r) = resources.get::<BlitResources>() {
            if let Some(bg) = &r.bind_group {
                pass.set_pipeline(&r.pipeline);
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..3, 0..1);
            }
        }
    }
}

/// The viewport.
pub struct Viewport {
    target: Option<OffscreenTarget>,
    bound_generation: Option<u64>,
}

impl Viewport {
    /// Installs the blit into egui-wgpu.
    pub fn new(render_state: &egui_wgpu::RenderState) -> Viewport {
        let device = &render_state.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gunkit viewport blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gunkit viewport blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gunkit viewport blit"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gunkit viewport blit"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_state.target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gunkit viewport blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(BlitResources {
                pipeline,
                layout,
                sampler,
                bind_group: None,
            });
        Viewport {
            target: None,
            bound_generation: None,
        }
    }

    /// Draws the gun at the rectangle's size and places the picture there.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        studio: &mut Studio,
        render_state: &egui_wgpu::RenderState,
    ) {
        let ppp = ui.ctx().pixels_per_point();
        let size = (
            ((rect.width() * ppp).round() as u32).max(1),
            ((rect.height() * ppp).round() as u32).max(1),
        );
        let target = self.target.get_or_insert_with(|| {
            OffscreenTarget::for_renderer(&studio.ctx, studio.renderer(), size)
        });
        target.resize(&studio.ctx, size);
        if self.bound_generation != Some(target.generation()) {
            let mut r = render_state.renderer.write();
            if let Some(res) = r.callback_resources.get_mut::<BlitResources>() {
                let bg = render_state
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("gunkit viewport blit"),
                        layout: &res.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(target.view()),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&res.sampler),
                            },
                        ],
                    });
                res.bind_group = Some(bg);
            }
            self.bound_generation = Some(target.generation());
        }
        studio.draw(target);
        ui.painter()
            .add(egui_wgpu::Callback::new_paint_callback(rect, BlitCallback));
    }
}
