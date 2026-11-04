//! The 3D viewport: dc-render's `WorldRenderer` draws the built world into an `OffscreenTarget` on the GPU
//! eframe shares (MAPKIT_PLAN.md D7 as amended: the GUI builds the headless `GpuContext` first and hands its
//! handles to eframe), and an egui paint callback samples that target into the viewport rectangle. Textures
//! come from the project's own PNGs, from the user's ROM when one is set, and a flat colour for the rest.
// DC-DEFAULT: MAP-62

use super::camera::FlyCamera;
use dc_render::{
    Camera, FrameOptions, GpuContext, GpuScene, OffscreenTarget, PlayerViewSettings,
    RenderSettings, SceneMesh, SceneOptions, SceneStats, TextureImage, TextureIndex, TextureTraits,
    WorldRenderer, OFFSCREEN_FORMAT,
};
use egui_wgpu::wgpu;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// The blit of the offscreen colour texture into the egui pass.
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

/// What the paint callback needs, stored in egui-wgpu's callback resources.
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

/// The textures of a built world: own PNGs from the project folder (`@mod:<id>/assets/x.dtx` -> `assets/x.png`),
/// ROM paths through the ROM's texture index when one is open, a flat colour for the rest.
pub struct TextureSource {
    pub project_dir: PathBuf,
    pub rom: Option<Arc<TextureIndex>>,
    cache: HashMap<Vec<u8>, (Option<TextureImage>, TextureTraits)>,
    pub own: usize,
    pub rom_hits: usize,
    pub flat: usize,
}

impl TextureSource {
    pub fn new(project_dir: PathBuf, rom: Option<Arc<TextureIndex>>) -> TextureSource {
        TextureSource {
            project_dir,
            rom,
            cache: HashMap::new(),
            own: 0,
            rom_hits: 0,
            flat: 0,
        }
    }

    /// The project-relative PNG of an own texture name, if it is one.
    pub fn own_png(name: &str) -> Option<String> {
        let rest = name.strip_prefix("@mod:")?;
        let (_, rel) = rest.split_once('/')?;
        let png = rel.strip_suffix(".dtx")?;
        Some(format!("{png}.png"))
    }

    fn flat_colour(name: &[u8]) -> TextureImage {
        let mut h: u32 = 2166136261;
        for b in name {
            h = (h ^ *b as u32).wrapping_mul(16777619);
        }
        let c = [
            120 + (h & 0x3f) as u8,
            120 + ((h >> 8) & 0x3f) as u8,
            120 + ((h >> 16) & 0x3f) as u8,
            255,
        ];
        TextureImage::rgba(2, 2, c.repeat(4))
    }

    fn resolve(&mut self, name: &[u8]) -> (Option<TextureImage>, TextureTraits) {
        let plain = TextureTraits {
            present: true,
            mode: 0,
            hi: false,
        };
        let text = String::from_utf8_lossy(name).into_owned();
        if let Some(png) = Self::own_png(&text) {
            let path = crate::project::Project::resolve(&self.project_dir, &png);
            if let Ok(bytes) = std::fs::read(&path) {
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let mut t = TextureImage::rgba(w, h, rgba.into_raw());
                    t.generate_mips();
                    self.own += 1;
                    return (Some(t), plain);
                }
            }
        } else if let Some(rom) = &self.rom {
            if let Ok(Some(bytes)) = rom.read(name) {
                if let Ok(dtx) = dc_tex::DtxFile::parse(&bytes) {
                    let traits = dc_render::traits_of(&dtx);
                    if let Some(img) = dc_render::decode_dtx(&dtx) {
                        self.rom_hits += 1;
                        return (Some(img), traits);
                    }
                }
            }
        }
        self.flat += 1;
        (Some(Self::flat_colour(name)), plain)
    }

    fn entry(&mut self, name: &[u8]) -> &(Option<TextureImage>, TextureTraits) {
        if !self.cache.contains_key(name) {
            let v = self.resolve(name);
            self.cache.insert(name.to_vec(), v);
        }
        &self.cache[name]
    }

    pub fn traits(&mut self, name: &[u8]) -> TextureTraits {
        self.entry(name).1
    }

    pub fn image(&mut self, name: &[u8]) -> Option<TextureImage> {
        self.entry(name).0.clone()
    }
}

/// The viewport's GPU side.
pub struct Viewport {
    pub ctx: GpuContext,
    renderer: WorldRenderer,
    target: OffscreenTarget,
    scene: Option<GpuScene>,
    bound_generation: Option<u64>,
    pub stats: Option<SceneStats>,
}

impl Viewport {
    /// Makes the renderer and the first target on the shared GPU and installs the blit into egui-wgpu.
    pub fn new(ctx: GpuContext, render_state: &egui_wgpu::RenderState) -> Viewport {
        let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
        let target = OffscreenTarget::for_renderer(&ctx, &renderer, (64, 64));
        let device = &render_state.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mapkit viewport blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mapkit viewport blit"),
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
            label: Some("mapkit viewport blit"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mapkit viewport blit"),
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
            label: Some("mapkit viewport blit"),
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
            ctx,
            renderer,
            target,
            scene: None,
            bound_generation: None,
            stats: None,
        }
    }

    /// Parse a built world, build its scene mesh and upload it.
    pub fn set_world(
        &mut self,
        dat: &[u8],
        textures: &mut TextureSource,
    ) -> Result<SceneStats, String> {
        let world = dc_world::World::parse_with(dat, dc_world::ParseOptions { render: true })
            .map_err(|e| e.to_string())?;
        let mesh = SceneMesh::build(&world, SceneOptions::default(), &mut |n| textures.traits(n));
        let stats = mesh.stats;
        let scene = self
            .renderer
            .upload(&self.ctx, &mesh, &mut |n| textures.image(n));
        self.scene = Some(scene);
        self.stats = Some(stats);
        Ok(stats)
    }

    pub fn clear_world(&mut self) {
        self.scene = None;
        self.stats = None;
    }

    pub fn has_world(&self) -> bool {
        self.scene.is_some()
    }

    /// Render the frame for `rect` (points) and queue the blit into it.
    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        cam: &FlyCamera,
        render_state: &egui_wgpu::RenderState,
    ) {
        let ppp = ui.ctx().pixels_per_point();
        let size = (
            ((rect.width() * ppp).round() as u32).max(1),
            ((rect.height() * ppp).round() as u32).max(1),
        );
        let resized = self.target.resize(&self.ctx, size);
        if resized || self.bound_generation != Some(self.target.generation()) {
            let mut r = render_state.renderer.write();
            if let Some(res) = r.callback_resources.get_mut::<BlitResources>() {
                let bg = render_state
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("mapkit viewport blit"),
                        layout: &res.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(self.target.view()),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&res.sampler),
                            },
                        ],
                    });
                res.bind_group = Some(bg);
            }
            self.bound_generation = Some(self.target.generation());
        }
        let camera = render_camera(cam, size.0, size.1);
        match &self.scene {
            Some(scene) => {
                self.renderer.render_frame_to_target(
                    &self.ctx,
                    scene,
                    &camera,
                    &FrameOptions::default(),
                    &[],
                    &[],
                    &PlayerViewSettings::default(),
                    &[],
                    &self.target,
                );
            }
            None => {
                let mut enc =
                    self.ctx
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("mapkit viewport clear"),
                        });
                {
                    let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("mapkit viewport clear"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: self.target.view(),
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.16,
                                    g: 0.17,
                                    b: 0.2,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                }
                self.ctx.queue.submit([enc.finish()]);
            }
        }
        ui.painter()
            .add(egui_wgpu::Callback::new_paint_callback(rect, BlitCallback));
    }

    /// A screenshot of the last frame.
    pub fn read_back(&self) -> Result<dc_render::FrameImage, String> {
        self.target.read_back(&self.ctx).map_err(|e| e.to_string())
    }
}

/// dc-render's camera for the fly camera on a `w x h` pixel target (same conventions, see [`FlyCamera`]).
pub fn render_camera(cam: &FlyCamera, w: u32, h: u32) -> Camera {
    let fov_y = cam.fov_y_deg.to_radians();
    let fov_x = dc_render::fov_x_for(fov_y, w, h);
    let mut c = Camera::looking(glam::Vec3::from(cam.pos), cam.yaw, cam.pitch, fov_x, fov_y);
    c.far_z = cam.far;
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_texture_names_map_to_the_project_png() {
        assert_eq!(
            TextureSource::own_png("@mod:my_map/assets/ground.dtx").as_deref(),
            Some("assets/ground.png")
        );
        assert_eq!(
            TextureSource::own_png("@mod:x/assets/sub/wall.dtx").as_deref(),
            Some("assets/sub/wall.png")
        );
        assert_eq!(TextureSource::own_png("some/rom/path.dtx"), None);
        assert_eq!(TextureSource::own_png("@mod:x/assets/not_a_dtx.png"), None);
        let a = TextureSource::flat_colour(b"a");
        let b = TextureSource::flat_colour(b"b");
        assert_ne!(a.levels[0], b.levels[0]);
    }

    #[test]
    fn the_render_camera_agrees_with_the_fly_camera_projection() {
        let cam = FlyCamera::default();
        let (w, h) = (800u32, 500u32);
        let c = render_camera(&cam, w, h);
        let p = [300.0f32, 100.0, 400.0];
        let clip = c.view_projection() * glam::Vec4::new(p[0], p[1], p[2], 1.0);
        let ndc = [clip.x / clip.w, clip.y / clip.w];
        let px = [
            (ndc[0] + 1.0) * 0.5 * w as f32,
            (1.0 - ndc[1]) * 0.5 * h as f32,
        ];
        let (mine, _) = cam.project(p, w as f32, h as f32).unwrap();
        assert!(
            (mine[0] - px[0]).abs() < 0.05 && (mine[1] - px[1]).abs() < 0.05,
            "{mine:?} vs {px:?}"
        );
    }
}
