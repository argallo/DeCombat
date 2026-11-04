//! wgpu side: adapter selection (headless first), the world pipelines, scene upload, the frame and the
//! read-back to RGBA8.

use crate::bucket::{PassKind, RenderSettings};
use crate::camera::{
    box_outside, frustum_planes, Camera, PlayerViewSettings, PLAYER_VIEW_DEPTH_RANGE,
    WORLD_DEPTH_RANGE,
};
use crate::fog::Fog;
use crate::model::ModelFiles;
use crate::model_gpu::{ModelInstance, ModelLook, ModelPipelines};
use crate::scene::{Batch, SceneMesh};
use crate::sky::{portal_screen_rect, sky_camera, SkySettings};
use crate::texture::TextureImage;
use crate::vertex::GpuVertex;
use bytemuck::{Pod, Zeroable};
use glam::{Affine3A, Mat4, Vec3};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::Instant;

const WORLD_WGSL: &str = include_str!("shaders/world.wgsl");

/// Colour the frame is cleared to. The sky pass is not drawn yet (RENDER_2009.md §5.3), so this shows behind
/// sky portals. A viewer choice, not a 2009 value.
pub const CLEAR_COLOUR: [f64; 4] = [0.18, 0.2, 0.24, 1.0];

/// Depth buffer format.
// SPEC: RENDER_2009.md §2.2 (DeCombat: Depth32Float; reversed-Z is not used because of the [0,1] split)
pub(crate) const ZBUF_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Offscreen colour format (8 bits per channel, no sRGB conversion: the 2009 fixed-function blending works on
/// the stored values, so the frame stays in that space).
// SPEC: RENDER_2009.md §2.1 (DeCombat: the 32-bit back buffer; Bgra8Unorm for a window, Rgba8Unorm offscreen)
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Flush the upload staging memory after this many bytes (keeps the 32-bit build inside its address space).
const UPLOAD_FLUSH_BYTES: u64 = 48 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    NoAdapter(String),
    Device(String),
    Readback(String),
    Image(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::NoAdapter(s) => write!(f, "no usable graphics adapter: {s}"),
            RenderError::Device(s) => write!(f, "device request failed: {s}"),
            RenderError::Readback(s) => write!(f, "frame read-back failed: {s}"),
            RenderError::Image(s) => write!(f, "image write failed: {s}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Which adapter to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterChoice {
    /// The default backends, then DX12, then Vulkan, then the software fallback adapter.
    Auto,
    /// Only these backends (hardware or software, whatever the backend offers first).
    Backends(wgpu::Backends),
    /// Only the software fallback adapter (WARP on Windows).
    Fallback,
}

/// Result of one adapter probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterProbe {
    pub attempt: &'static str,
    /// `name (backend, device type)` or the error text.
    pub result: Result<String, String>,
}

fn describe(info: &wgpu::AdapterInfo) -> String {
    format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type)
}

fn instance_for(backends: wgpu::Backends) -> wgpu::Instance {
    wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    })
}

fn request(
    instance: &wgpu::Instance,
    fallback: bool,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter, String> {
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: fallback,
        compatible_surface: surface,
    }))
    .map_err(|e| e.to_string())
}

/// The attempts of [`AdapterChoice::Auto`], in order.
fn auto_attempts() -> [(&'static str, wgpu::Backends, bool); 4] {
    [
        ("default backends", wgpu::Backends::PRIMARY, false),
        ("dx12", wgpu::Backends::DX12, false),
        ("vulkan", wgpu::Backends::VULKAN, false),
        ("software fallback", wgpu::Backends::all(), true),
    ]
}

/// Tries every attempt of [`AdapterChoice::Auto`] and reports what each finds (no device is created).
pub fn probe_adapters() -> Vec<AdapterProbe> {
    auto_attempts()
        .iter()
        .map(|&(attempt, backends, fallback)| AdapterProbe {
            attempt,
            result: request(&instance_for(backends), fallback, None)
                .map(|a| describe(&a.get_info())),
        })
        .collect()
}

/// Whether `adapter` offers (and every [`GpuContext`] on it therefore requests) the adapter-specific texture format
/// features, which let a device use the adapter's own per-format sample counts beyond the guaranteed 1 and 4.
pub(crate) fn adapter_specific_formats(adapter: &wgpu::Adapter) -> bool {
    adapter
        .features()
        .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
}

/// Device and queue on one adapter.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
}

impl fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuContext")
            .field("adapter", &describe(&self.info))
            .finish()
    }
}

impl GpuContext {
    /// A headless device (no surface).
    pub fn headless(choice: AdapterChoice) -> Result<GpuContext, RenderError> {
        let attempts: Vec<(&'static str, wgpu::Backends, bool)> = match choice {
            AdapterChoice::Auto => auto_attempts().to_vec(),
            AdapterChoice::Backends(b) => vec![("requested backends", b, false)],
            AdapterChoice::Fallback => vec![("software fallback", wgpu::Backends::all(), true)],
        };
        let mut errors = Vec::new();
        for (name, backends, fallback) in attempts {
            let instance = instance_for(backends);
            match request(&instance, fallback, None) {
                Ok(adapter) => return GpuContext::with_adapter(instance, adapter),
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        Err(RenderError::NoAdapter(errors.join("; ")))
    }

    /// A device for an adapter that can present to `surface` (window mode).
    pub fn for_surface(
        instance: wgpu::Instance,
        surface: &wgpu::Surface<'_>,
    ) -> Result<GpuContext, RenderError> {
        let adapter = request(&instance, false, Some(surface))
            .or_else(|_| request(&instance, true, Some(surface)))
            .map_err(RenderError::NoAdapter)?;
        GpuContext::with_adapter(instance, adapter)
    }

    fn with_adapter(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
    ) -> Result<GpuContext, RenderError> {
        let info = adapter.get_info();
        // perf-g1: the profiler's per-pass GPU timestamps ask for the one optional feature they need (only when the
        // profiler wants them and the adapter has it; otherwise no optional features, as before)
        let timed = crate::perf::timestamps_wanted()
            && adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let mut required_features = if timed {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        // CLI-1593: the adapter's own per-format sample counts (2x and 8x MSAA where the device has them; without
        // this feature wgpu accepts only the WebGPU-guaranteed 1 and 4 samples whatever the adapter reports), asked
        // for only when the adapter has it. Validation scope only: it changes no pixel. `supported_msaa` applies the
        // same rule, so it never names a count the device would refuse.
        if adapter_specific_formats(&adapter) {
            required_features |= wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("dc-render device"),
            required_features,
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| RenderError::Device(e.to_string()))?;
        if timed {
            crate::perf::install_timer(&device, &queue);
        }
        Ok(GpuContext {
            instance,
            adapter,
            device,
            queue,
            info,
        })
    }

    pub fn describe(&self) -> String {
        describe(&self.info)
    }

    fn wait(&self) {
        let _ = self.device.poll(wgpu::PollType::Wait);
    }
}

/// Per-frame uniforms (layout shared with `world.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    clip_from_world: [[f32; 4]; 4],
    params: [f32; 4],
    lm_factor: [f32; 4],
    /// Linear fog `[far, 1 / (far - near), on, 0]` and its colour (RENDER_2009.md §3.3).
    fog: [f32; 4],
    fog_colour: [f32; 4],
}

/// Per-material uniforms (layout shared with `world.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct MaterialUniform {
    alpha: [f32; 4],
}

fn unpack_argb(c: u32) -> [f32; 4] {
    [
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
        (c >> 24) as f32 / 255.0,
    ]
}

/// Pipelines and fixed resources of the world passes for one colour format.
pub struct WorldRenderer {
    settings: RenderSettings,
    colour_format: wgpu::TextureFormat,
    /// Samples per pixel every pipeline here (and the client's passes into the same target) is built for
    /// ([`RenderSettings::msaa_count`]); 1 = off. Fixed for the renderer's life: a new count needs a new renderer.
    msaa: u32,
    pipelines: HashMap<PassKind, wgpu::RenderPipeline>,
    /// Sky pass: the same recipes with Z test and Z write off (RENDER_2009.md §5.3).
    sky_pipelines: HashMap<PassKind, wgpu::RenderPipeline>,
    /// Translucent pass: (recipe, additive) with blending on and Z write off (RENDER_2009.md §7.1 step 7).
    blend_pipelines: HashMap<(PassKind, bool), wgpu::RenderPipeline>,
    material_layout: wgpu::BindGroupLayout,
    object_layout: wgpu::BindGroupLayout,
    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    sky_frame_buffer: wgpu::Buffer,
    sky_frame_group: wgpu::BindGroup,
    wrap_sampler: wgpu::Sampler,
    clamp_sampler: wgpu::Sampler,
    /// Shared with [`crate::ModelUploader`]s (model uploads on loader threads).
    models: std::sync::Arc<ModelPipelines>,
    /// Leave out opaque world batches whose bounds lie outside the view frustum (default on; pixel-neutral).
    world_culling: bool,
    /// CLI-1624: the x-ray pass's pipelines, built on its first use.
    xray: std::sync::OnceLock<crate::xray::XrayPipes>,
}

/// How far (world units) a world batch's bounds must lie outside a frustum plane before it is left out: well above
/// float rounding of the plane test at world coordinates, so a batch with any pixel on screen is always drawn.
const CULL_MARGIN: f32 = 1.0;

impl fmt::Debug for WorldRenderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorldRenderer")
            .field("colour_format", &self.colour_format)
            .field("pipelines", &self.pipelines.len())
            .finish()
    }
}

fn blend_for(pass: PassKind, s: &RenderSettings) -> Option<wgpu::BlendState> {
    if !pass.multiplies() {
        return None;
    }
    // SPEC: RENDER_2009.md §5.2 bucket 15: SRC = DESTCOLOR, DST = SRCCOLOR if `saturate` else ZERO
    let colour = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Dst,
        dst_factor: if s.saturate {
            wgpu::BlendFactor::Src
        } else {
            wgpu::BlendFactor::Zero
        },
        operation: wgpu::BlendOperation::Add,
    };
    // The frame alpha is not part of the 2009 output; keep it as the lightmap pass wrote it.
    let alpha = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    Some(wgpu::BlendState {
        color: colour,
        alpha,
    })
}

/// Blend of the translucent state: SRCALPHA / INVSRCALPHA, additive SRCALPHA / ONE.
// SPEC: RENDER_2009.md §7.1 step 7 (SRC = SRCALPHA / DST = INVSRCALPHA), §5.1 step 6 and engine+0x1a9790
// (additive world models: SRC = SRCALPHA, DST = ONE)
fn translucent_blend(additive: bool) -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: if additive {
                wgpu::BlendFactor::One
            } else {
                wgpu::BlendFactor::OneMinusSrcAlpha
            },
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

fn entry_point(pass: PassKind) -> &'static str {
    match pass {
        PassKind::Flat => "fs_flat",
        PassKind::VertexLit => "fs_vertex_lit",
        PassKind::BlendVertexLit => "fs_blend_vertex_lit",
        PassKind::Lightmap => "fs_lightmap",
        PassKind::LightmappedTexture => "fs_lightmapped_texture",
        PassKind::BlendLightmapped => "fs_blend_lightmapped",
    }
}

/// Fragment entry of a recipe in the translucent pass (only the recipes a blended model can use).
fn blend_entry_point(pass: PassKind) -> Option<&'static str> {
    match pass {
        PassKind::Flat => Some("fs_flat_blend"),
        PassKind::VertexLit => Some("fs_vertex_lit_blend"),
        PassKind::BlendVertexLit => Some("fs_blend_vertex_lit_blend"),
        _ => None,
    }
}

const ALL_PASSES: [PassKind; 6] = [
    PassKind::Flat,
    PassKind::VertexLit,
    PassKind::BlendVertexLit,
    PassKind::Lightmap,
    PassKind::LightmappedTexture,
    PassKind::BlendLightmapped,
];

/// Depth state of a world pipeline: (compare, write).
struct DepthMode(wgpu::CompareFunction, bool);

#[allow(clippy::too_many_arguments)]
fn world_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    label: &str,
    entry: &str,
    colour_format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    depth: DepthMode,
    msaa: u32,
) -> wgpu::RenderPipeline {
    crate::determinism::render_pipeline(
        device,
        &wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_world"),
                compilation_options: Default::default(),
                buffers: &[GpuVertex::layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: colour_format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            // SPEC: RENDER_2009.md §5.1 step 4 (CULLMODE = CCW: front faces wind clockwise on screen) and
            // the DeCombat note there (FrontFace::Cw, Face::Back)
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Cw,
                cull_mode: Some(wgpu::Face::Back),
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            // SPEC: RENDER_2009.md §2.4 (ZENABLE / ZWRITEENABLE / ZFUNC = TRUE / TRUE / LESSEQUAL; the world
            // recipes keep it, §5.2); the sky and translucent variants change it (§5.3, §7.1)
            depth_stencil: Some(wgpu::DepthStencilState {
                format: ZBUF_FORMAT,
                depth_write_enabled: depth.1,
                depth_compare: depth.0,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            // CLI-1593: the sample count of the frame's 3D target (1: the default state, MSAA off)
            multisample: crate::msaa::multisample_state(msaa),
            multiview: None,
            cache: None,
        },
    )
}

impl WorldRenderer {
    pub fn new(
        ctx: &GpuContext,
        colour_format: wgpu::TextureFormat,
        settings: RenderSettings,
    ) -> WorldRenderer {
        let device = &ctx.device;
        let module = crate::determinism::shader_module(
            device,
            wgpu::ShaderModuleDescriptor {
                label: Some("world.wgsl"),
                source: wgpu::ShaderSource::Wgsl(WORLD_WGSL.into()),
            },
        );
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let samp_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                uniform(0, wgpu::ShaderStages::FRAGMENT),
                tex_entry(1),
                tex_entry(2),
                samp_entry(3),
                samp_entry(4),
            ],
        });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
            entries: &[uniform(0, wgpu::ShaderStages::FRAGMENT)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world"),
            bind_group_layouts: &[&frame_layout, &material_layout],
            push_constant_ranges: &[],
        });
        let blend_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world translucent"),
            bind_group_layouts: &[&frame_layout, &material_layout, &object_layout],
            push_constant_ranges: &[],
        });
        // CLI-1593: every pipeline that draws into the frame's 3D target carries the sample count; the client's
        // passes into the same target (debug boxes, effect quads) read it from [`WorldRenderer::msaa`]
        let msaa = settings.msaa_count();
        let mut pipelines = HashMap::new();
        let mut sky_pipelines = HashMap::new();
        let mut blend_pipelines = HashMap::new();
        for pass in ALL_PASSES {
            let entry = entry_point(pass);
            pipelines.insert(
                pass,
                world_pipeline(
                    device,
                    &module,
                    &layout,
                    entry,
                    entry,
                    colour_format,
                    blend_for(pass, &settings),
                    DepthMode(wgpu::CompareFunction::LessEqual, true),
                    msaa,
                ),
            );
            // SPEC: RENDER_2009.md §5.3 (sky objects drawn with Z test and Z write off)
            sky_pipelines.insert(
                pass,
                world_pipeline(
                    device,
                    &module,
                    &layout,
                    "sky",
                    entry,
                    colour_format,
                    blend_for(pass, &settings),
                    DepthMode(wgpu::CompareFunction::Always, false),
                    msaa,
                ),
            );
            if let Some(be) = blend_entry_point(pass) {
                for additive in [false, true] {
                    // SPEC: RENDER_2009.md §7.1 step 7 (ALPHABLENDENABLE on, ZWRITEENABLE off; the Z test stays)
                    blend_pipelines.insert(
                        (pass, additive),
                        world_pipeline(
                            device,
                            &module,
                            &blend_layout,
                            be,
                            be,
                            colour_format,
                            Some(translucent_blend(additive)),
                            DepthMode(wgpu::CompareFunction::LessEqual, false),
                            msaa,
                        ),
                    );
                }
            }
        }
        let frame_buffer_of = |label| {
            crate::perf::buffer(
                device,
                &wgpu::BufferDescriptor {
                    label: Some(label),
                    size: std::mem::size_of::<FrameUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
            )
        };
        let frame_buffer = frame_buffer_of("frame uniforms");
        let sky_frame_buffer = frame_buffer_of("sky frame uniforms");
        let group_of = |buf: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("frame"),
                layout: &frame_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            })
        };
        let frame_group = group_of(&frame_buffer);
        let sky_frame_group = group_of(&sky_frame_buffer);
        // SPEC: RENDER_2009.md §3.3 (per-frame filters: LINEAR with `bilinear`, mip LINEAR with `trilinear`,
        // ANISOTROPIC with `anisotropy`), §10.3 (WRAP unless the pass sets CLAMP)
        let filter = if settings.bilinear {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        };
        let mip = if settings.trilinear {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        };
        let aniso = if settings.anisotropy > 1 && settings.bilinear && settings.trilinear {
            settings.anisotropy.min(16) as u16
        } else {
            1
        };
        let wrap = || {
            crate::determinism::sampler(
                device,
                &wgpu::SamplerDescriptor {
                    label: Some("wrap"),
                    address_mode_u: wgpu::AddressMode::Repeat,
                    address_mode_v: wgpu::AddressMode::Repeat,
                    address_mode_w: wgpu::AddressMode::Repeat,
                    mag_filter: filter,
                    min_filter: filter,
                    mipmap_filter: mip,
                    anisotropy_clamp: aniso,
                    ..Default::default()
                },
            )
        };
        let wrap_sampler = wrap();
        // SPEC: RENDER_2009.md §5.2 bucket 14 (address CLAMP), §6.1 (stage 0 samples the lightmap with CLAMP)
        let clamp_sampler = crate::determinism::sampler(
            device,
            &wgpu::SamplerDescriptor {
                label: Some("clamp"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            },
        );
        let models = ModelPipelines::new(
            ctx,
            colour_format,
            ZBUF_FORMAT,
            settings.mip_bias,
            aniso,
            msaa,
        );
        WorldRenderer {
            settings,
            colour_format,
            msaa,
            pipelines,
            sky_pipelines,
            blend_pipelines,
            material_layout,
            object_layout,
            frame_buffer,
            frame_group,
            sky_frame_buffer,
            sky_frame_group,
            wrap_sampler,
            clamp_sampler,
            models: std::sync::Arc::new(models),
            world_culling: true,
            xray: std::sync::OnceLock::new(),
        }
    }

    pub fn colour_format(&self) -> wgpu::TextureFormat {
        self.colour_format
    }

    /// Samples per pixel of the frame's 3D target this renderer draws into (1: MSAA off). The frame loop keeps
    /// [`crate::MsaaTargets`] of this count and builds the passes it adds to the target (debug boxes, effect quads)
    /// with it. Fixed at creation: to change it, build a new renderer with the new `RenderSettings::msaa`, upload the
    /// scene again and re-upload the models (their pipelines were cloned from this renderer's).
    pub fn msaa(&self) -> u32 {
        self.msaa
    }

    /// The settings this renderer was built with.
    pub fn settings(&self) -> &RenderSettings {
        &self.settings
    }

    /// Switches view-frustum culling of the opaque world batches (on by default). A batch whose bounds lie entirely
    /// outside the view volume produces no fragment, so the frame is the same either way; off records every batch
    /// (for A/B tests).
    pub fn set_world_culling(&mut self, on: bool) {
        self.world_culling = on;
    }

    pub fn world_culling(&self) -> bool {
        self.world_culling
    }

    /// The model pipelines.
    pub fn models(&self) -> &ModelPipelines {
        &self.models
    }

    /// Uploads a model with its texture list (see [`ModelPipelines::upload`]).
    pub fn upload_model(
        &self,
        ctx: &GpuContext,
        files: &ModelFiles,
        textures: &[Option<TextureImage>],
    ) -> ModelInstance {
        self.models.upload(ctx, files, textures)
    }

    /// Uploads a model with its texture list and render styles (see [`ModelPipelines::upload_look`]).
    pub fn upload_model_look(
        &self,
        ctx: &GpuContext,
        files: &ModelFiles,
        look: ModelLook<'_>,
    ) -> ModelInstance {
        self.models.upload_look(ctx, files, look)
    }

    /// Uploads a scene: buffers, every texture (through `images`, which returns `None` for names it cannot
    /// resolve; those get [`TextureImage::unbound`], the 2009 unbound stage) and every lightmap.
    pub fn upload(
        &self,
        ctx: &GpuContext,
        mesh: &SceneMesh,
        images: &mut dyn FnMut(&[u8]) -> Option<TextureImage>,
    ) -> GpuScene {
        let t0 = Instant::now();
        let device = &ctx.device;
        let mut stats = UploadStats::default();
        let mut pending = 0u64;
        let flush = |ctx: &GpuContext, bytes: u64, pending: &mut u64| {
            *pending += bytes;
            if *pending >= UPLOAD_FLUSH_BYTES {
                ctx.queue.submit(std::iter::empty());
                ctx.wait();
                *pending = 0;
            }
        };
        let white = upload_image(ctx, &TextureImage::white(), "white");
        // DISASM: engine+0x1020b0 (a texture that did not load leaves its stage unbound)
        let missing_img = TextureImage::unbound();
        let mut textures: Vec<(wgpu::Texture, MaterialInfo)> =
            Vec::with_capacity(mesh.textures.len());
        let mut decode_ms = 0f64;
        for name in &mesh.textures {
            let td = Instant::now();
            let img = images(name);
            decode_ms += td.elapsed().as_secs_f64() * 1000.0;
            let (img, found) = match img {
                Some(i) => (i, true),
                None => (missing_img.clone(), false),
            };
            if found {
                stats.textures += 1;
            } else {
                stats.missing_textures += 1;
            }
            let bytes: u64 = img.levels.iter().map(|l| l.len() as u64).sum();
            stats.texture_bytes += bytes;
            let info = MaterialInfo {
                alpha_cutoff: img.alpha_cutoff,
                bias: self.settings.mip_bias + img.mip_bias,
            };
            textures.push((upload_image(ctx, &img, "world texture"), info));
            flush(ctx, bytes, &mut pending);
        }
        let mut lightmaps = Vec::with_capacity(mesh.lightmaps.len());
        for lm in &mesh.lightmaps {
            let img = TextureImage::rgba(lm.width, lm.height, lm.rgba.clone());
            stats.lightmap_bytes += lm.rgba.len() as u64;
            lightmaps.push(upload_image(ctx, &img, "lightmap"));
            flush(ctx, lm.rgba.len() as u64, &mut pending);
        }
        let placeholder = [GpuVertex::zeroed()];
        let vertex_buffer = crate::perf::buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("world vertices"),
                contents: bytemuck::cast_slice(if mesh.vertices.is_empty() {
                    &placeholder[..]
                } else {
                    &mesh.vertices
                }),
                // COPY_DST: a world model moved by its object rewrites its own vertex range (set_model_transform)
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        );
        let index_buffer = crate::perf::buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("world indices"),
                contents: bytemuck::cast_slice(if mesh.indices.is_empty() {
                    &[0u32, 0, 0][..]
                } else {
                    &mesh.indices
                }),
                usage: wgpu::BufferUsages::INDEX,
            },
        );
        let white_view = white.create_view(&Default::default());
        let views: Vec<wgpu::TextureView> = textures
            .iter()
            .map(|(t, _)| t.create_view(&Default::default()))
            .collect();
        let lm_views: Vec<wgpu::TextureView> = lightmaps
            .iter()
            .map(|t| t.create_view(&Default::default()))
            .collect();
        let mut groups: Vec<wgpu::BindGroup> = Vec::new();
        let mut group_of: HashMap<MaterialKey, usize> = HashMap::new();
        let mut resolve = |b: &Batch| -> Option<usize> {
            if b.index_count == 0 {
                return None;
            }
            let key = if b.pass == PassKind::Lightmap {
                // undecodable lightmap: nothing to draw
                (None, None, Some(b.lightmap?))
            } else {
                (b.tex0, b.tex1, None)
            };
            if let Some(&g) = group_of.get(&key) {
                return Some(g);
            }
            let (v0, s0, info): (&wgpu::TextureView, &wgpu::Sampler, MaterialInfo) = match key {
                (_, _, Some(l)) => (
                    &lm_views[l as usize],
                    &self.clamp_sampler,
                    MaterialInfo::default(),
                ),
                (Some(t), _, _) => (
                    &views[t as usize],
                    &self.wrap_sampler,
                    textures[t as usize].1,
                ),
                _ => (&white_view, &self.wrap_sampler, MaterialInfo::default()),
            };
            let (v1, bias1) = match key.1 {
                Some(t) => (&views[t as usize], textures[t as usize].1.bias),
                None => (&white_view, self.settings.mip_bias),
            };
            // SPEC: RENDER_2009.md §10.3 (ALPHAREF from the texture's command, 0 when absent; alpha test is
            // switched by the pass) — our reading: test on when the texture carries a reference
            // (open item 11: the source of the per-batch flag is UNPROVEN)
            let u = MaterialUniform {
                alpha: [
                    info.alpha_cutoff.unwrap_or(0) as f32,
                    if info.alpha_cutoff.is_some() {
                        1.0
                    } else {
                        0.0
                    },
                    info.bias,
                    bias1,
                ],
            };
            let ub = crate::perf::buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("material"),
                    contents: bytemuck::bytes_of(&u),
                    usage: wgpu::BufferUsages::UNIFORM,
                },
            );
            let g = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("material"),
                layout: &self.material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ub.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(v0),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(v1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(s0),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&self.wrap_sampler),
                    },
                ],
            });
            groups.push(g);
            group_of.insert(key, groups.len() - 1);
            Some(groups.len() - 1)
        };
        let mut draws = Vec::with_capacity(mesh.batches.len());
        let mut draw_model = Vec::with_capacity(mesh.batches.len());
        for b in &mesh.batches {
            if let Some(g) = resolve(b) {
                draws.push((*b, g));
                draw_model.push(mesh.model_of(b.first_index));
            }
        }
        let mut sky_draws = Vec::with_capacity(mesh.sky_batches.len());
        let mut sky_draw_model = Vec::with_capacity(mesh.sky_batches.len());
        for b in &mesh.sky_batches {
            if let Some(g) = resolve(b) {
                sky_draws.push((*b, g));
                sky_draw_model.push(mesh.sky_model_of(b.first_index));
            }
        }
        let mut blended = Vec::with_capacity(mesh.blended.len());
        for m in &mesh.blended {
            let mut d = Vec::with_capacity(m.batches.len());
            for b in &m.batches {
                if blend_entry_point(b.pass).is_none() {
                    continue;
                }
                if let Some(g) = resolve(b) {
                    d.push((*b, g));
                }
            }
            if d.is_empty() {
                continue;
            }
            let ub = crate::perf::buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("object"),
                    // y = 1: additive, its fog colour is black (RENDER_2009.md §5.1 step 6, engine+0x1a9790)
                    contents: bytemuck::cast_slice(&[
                        m.alpha,
                        if m.additive { 1.0 } else { 0.0 },
                        0.0,
                        0.0,
                    ]),
                    usage: wgpu::BufferUsages::UNIFORM,
                },
            );
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("object"),
                layout: &self.object_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ub.as_entire_binding(),
                }],
            });
            let model = m.batches.first().and_then(|b| mesh.model_of(b.first_index));
            blended.push(GpuBlended {
                draws: d,
                origin: Vec3::from(m.origin),
                base_origin: Vec3::from(m.origin),
                additive: m.additive,
                group,
                model,
            });
        }
        // world-space bounds of every opaque draw, from the vertices its index range uses (an index outside the
        // vertex list makes the bounds infinite: never culled)
        let draw_bounds: Vec<(Vec3, Vec3)> = draws
            .iter()
            .map(|(b, _)| {
                let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
                let r = b.first_index as usize..(b.first_index + b.index_count) as usize;
                for &i in mesh.indices.get(r).unwrap_or(&[]) {
                    match mesh.vertices.get(i as usize) {
                        Some(v) if Vec3::from(v.pos).is_finite() => {
                            lo = lo.min(Vec3::from(v.pos));
                            hi = hi.max(Vec3::from(v.pos));
                        }
                        _ => {
                            lo = Vec3::splat(f32::NEG_INFINITY);
                            hi = Vec3::splat(f32::INFINITY);
                            break;
                        }
                    }
                }
                (lo, hi)
            })
            .collect();
        let model_base = model_vertex_ranges(mesh);
        ctx.queue.submit(std::iter::empty());
        ctx.wait();
        stats.upload_ms = t0.elapsed().as_secs_f64() * 1000.0 - decode_ms;
        stats.decode_ms = decode_ms;
        stats.draws = draws.len() as u64;
        stats.sky_draws = sky_draws.len() as u64;
        stats.blended_objects = blended.len() as u64;
        stats.bind_groups = groups.len() as u64;
        GpuScene {
            vertex_buffer,
            index_buffer,
            draws,
            draw_model,
            draw_bounds_base: draw_bounds.clone(),
            draw_bounds,
            model_transform: vec![None; mesh.models.len()],
            model_posed: vec![Vec::new(); mesh.models.len()],
            model_base,
            model_pending: std::sync::Mutex::new(Vec::new()),
            sky_draws,
            blended,
            model_names: mesh.models.iter().map(|m| m.name.clone()).collect(),
            model_off: vec![false; mesh.models.len()],
            groups,
            _textures: textures
                .into_iter()
                .map(|t| t.0)
                .chain(lightmaps)
                .chain([white])
                .collect(),
            sky_bounds: mesh.sky_bounds,
            sky_portals: mesh.sky_portals.clone(),
            world_extents: mesh.world_extents,
            fog: mesh.fog,
            sky_fog: mesh.sky_fog,
            stats,
            sky_draw_model,
            sky_model_names: mesh.sky_models.iter().map(|m| m.name.clone()).collect(),
            sky_model_off: vec![false; mesh.sky_models.len()],
            sky_view_box: None,
        }
    }

    fn frame_uniform(&self, view_projection: Mat4, fog: Option<&Fog>) -> FrameUniform {
        let s = &self.settings;
        let (fog, fog_colour) = Fog::uniform(fog);
        FrameUniform {
            fog,
            fog_colour,
            clip_from_world: view_projection.to_cols_array_2d(),
            params: [
                if s.saturate { 2.0 } else { 1.0 },
                if s.full_bright { 2.0 } else { 1.0 },
                if s.saturate { 1.0 } else { 0.0 },
                0.0,
            ],
            lm_factor: unpack_argb(s.lightmap_factor()),
        }
    }

    /// Records the world frame into `encoder`: clear, then every batch in bucket order with the world viewport
    /// depth range [0.1, 1.0]. Opaque world only (no sky, no translucent world models, no models): the frame of
    /// the first renderer slice; [`WorldRenderer::encode_frame`] draws the rest.
    // SPEC: RENDER_2009.md §3.2 (clear colour + depth 1.0), §3.3 step 4 (main draw), §4.3 (depth split),
    // §5.1 (world pass order)
    pub fn encode(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &Camera,
        frame: FrameTarget<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        self.encode_frame(
            ctx,
            scene,
            camera,
            frame,
            &FrameOptions::opaque_only(),
            &[],
            encoder,
        );
    }

    /// Records a full frame: the sky pass (when enabled and a sky portal is on screen), the opaque world in
    /// bucket order, the models, then the translucent and additive world models back to front.
    // SPEC: RENDER_2009.md §3.3 step 4 (main draw: world pass incl. sky, then the object queue), §5.1 steps 3-6,
    // §5.3 (sky), §7.1 (object queue order: solid world models, solid models, translucent state, sorted
    // translucent objects)
    #[allow(clippy::too_many_arguments)]
    pub fn encode_frame(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &Camera,
        frame: FrameTarget<'_>,
        opts: &FrameOptions,
        models: &[&ModelInstance],
        encoder: &mut wgpu::CommandEncoder,
    ) -> FrameReport {
        self.encode_frame_with_view(
            ctx,
            scene,
            camera,
            frame,
            opts,
            models,
            &[],
            &PlayerViewSettings::default(),
            encoder,
        )
    }

    /// As [`WorldRenderer::encode_frame`], then the eye-attached `view_models` (first-person weapon and hands)
    /// with the player-view camera: each instance's `position` / `rotation` is in eye space (+X right, +Y up,
    /// +Z forward), drawn with `pv`'s projection for the target's aspect, VIEW = identity and the viewport depth
    /// range [`PLAYER_VIEW_DEPTH_RANGE`], without clearing depth, after everything else of the frame. Their light
    /// sets are world lights evaluated at the instance's world place (the caller builds them at the eye). An
    /// instance must not be in both lists (one uniform buffer per instance).
    // SPEC: RENDER_2009.md §4.2 (player-view camera), §4.3 (depth split: the view models win without a depth
    // clear), §7.2 (eye-attached models switch to the player-view camera and back); DC-DEFAULT: REN-12 (proposed),
    // see DC_DEFAULTS.md (drawn last, after the translucent world objects)
    #[allow(clippy::too_many_arguments)]
    pub fn encode_frame_with_view(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &Camera,
        frame: FrameTarget<'_>,
        opts: &FrameOptions,
        models: &[&ModelInstance],
        view_models: &[&ModelInstance],
        pv: &PlayerViewSettings,
        encoder: &mut wgpu::CommandEncoder,
    ) -> FrameReport {
        let size = frame.size;
        // CLI-1593: the sky pass never resolves (the world pass follows it into the same target)
        let sky_target = FrameTarget {
            resolve: None,
            ..frame
        };
        scene.upload_model_poses(&ctx.queue);
        // the G0 mutation knob (tests only; unset: no change), docs/RENDER_DETERMINISM_CONTRACT.md
        let mutate = crate::determinism::mutation();
        let mut vp = camera.view_projection();
        if mutate.arith {
            vp = Mat4::from_scale(Vec3::new(1.0 + 1.0 / 256.0, 1.0, 1.0)) * vp;
        }
        crate::perf::write_buffer(
            &ctx.queue,
            &self.frame_buffer,
            0,
            bytemuck::bytes_of(&self.frame_uniform(vp, scene.fog.as_ref())),
        );
        let mut report = FrameReport::default();
        let cc = opts.clear.unwrap_or(CLEAR_COLOUR);
        let clear = wgpu::Color {
            r: cc[0],
            g: cc[1],
            b: cc[2],
            a: cc[3],
        };
        // ---- sky (RENDER_2009.md §5.1 step 3, §5.3) ----
        // client-hudgaps: the server's sky table may replace the view box and switch sky models off
        let sky_opts = opts.sky.map(|mut s| {
            if let Some(b) = scene.sky_view_box {
                s.view_box = Some(b);
            }
            s
        });
        let any_sky = (0..scene.sky_draws.len()).any(|k| scene.sky_drawn(k));
        let sky_rect = match &sky_opts {
            Some(_) if any_sky => portal_screen_rect(vp, &scene.sky_portals, size.0, size.1),
            _ => None,
        };
        let mut colour_load = wgpu::LoadOp::Clear(clear);
        if let (Some(rect), Some(sky)) = (sky_rect, &sky_opts) {
            let view = sky_camera(camera, scene.world_extents, scene.sky_bounds, sky);
            crate::perf::write_buffer(
                &ctx.queue,
                &self.sky_frame_buffer,
                0,
                // SPEC: RENDER_2009.md §5.3 (the sky objects use the sky fog range)
                bytemuck::bytes_of(
                    &self.frame_uniform(view.view_projection, scene.sky_fog.as_ref()),
                ),
            );
            report.sky_rect = Some(rect);
            report.sky_eye = Some(view.eye.to_array());
            let stamp = crate::perf::pass_stamp("sky");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sky"),
                color_attachments: &[sky_target.colour_attachment(wgpu::LoadOp::Clear(clear))],
                depth_stencil_attachment: sky_target.depth_attachment(wgpu::LoadOp::Clear(1.0)),
                timestamp_writes: stamp.as_ref().map(crate::perf::PassStamp::writes),
                occlusion_query_set: None,
            });
            pass.set_viewport(0.0, 0.0, size.0 as f32, size.1 as f32, 0.0, 1.0);
            // SPEC: RENDER_2009.md §5.3 (the sky is drawn only into the screen bounds of the visible portals)
            pass.set_scissor_rect(rect[0], rect[1], rect[2] - rect[0], rect[3] - rect[1]);
            pass.set_bind_group(0, &self.sky_frame_group, &[]);
            pass.set_vertex_buffer(0, scene.vertex_buffer.slice(..));
            pass.set_index_buffer(scene.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            report.bind_sets += 1;
            let mut current: Option<PassKind> = None;
            for (k, (b, g)) in scene.sky_draws.iter().enumerate() {
                if !scene.sky_drawn(k) {
                    continue;
                }
                if current != Some(b.pass) {
                    pass.set_pipeline(&self.sky_pipelines[&b.pass]);
                    current = Some(b.pass);
                    report.pipeline_sets += 1;
                }
                pass.set_bind_group(1, &scene.groups[*g], &[]);
                pass.draw_indexed(b.first_index..b.first_index + b.index_count, 0, 0..1);
                report.sky_draws += 1;
                report.world_tris += u64::from(b.index_count) / 3;
                report.bind_sets += 1;
                report.draw_calls += 1;
            }
            colour_load = wgpu::LoadOp::Load;
        }
        for m in models {
            report.uniform_bytes += m.write_fogged(ctx, camera, scene.fog.as_ref());
        }
        // SPEC: RENDER_2009.md §4.2 (aspect = width / height of the camera context)
        let pv_projection = pv.projection(size.0, size.1);
        for m in view_models {
            report.uniform_bytes += m.write_view(ctx, camera, pv_projection);
        }
        // ---- world, models, translucent objects ----
        // SPEC: RENDER_2009.md §5.3 (after the sky: clear depth only so the world draws over it)
        let stamp = crate::perf::pass_stamp("world");
        // CLI-1593: with `frame.resolve` set this pass is the frame's last 3D pass and resolves the samples
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("world"),
            color_attachments: &[frame.colour_attachment(colour_load)],
            depth_stencil_attachment: frame.depth_attachment(wgpu::LoadOp::Clear(1.0)),
            timestamp_writes: stamp.as_ref().map(crate::perf::PassStamp::writes),
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
        pass.set_bind_group(0, &self.frame_group, &[]);
        pass.set_vertex_buffer(0, scene.vertex_buffer.slice(..));
        pass.set_index_buffer(scene.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        report.bind_sets += 1;
        // Bucket order, with three pixel-neutral reductions of the recorded commands (render-core):
        // - batches whose bounds lie outside the view frustum are left out: they produce no fragment;
        // - consecutive surviving batches with the same recipe and material whose index ranges touch are one draw
        //   (the same triangles in the same order under the same state);
        // - the pipeline and the material bind group are set only when they change.
        // The order of the batches that are drawn is untouched (RENDER_2009.md §5.1, buckets 0..22).
        // DC-DEFAULT: REN-23 (proposed), see the staged rows (pixel-neutral: the frame is byte-identical, tests/culling.rs)
        let planes = frustum_planes(vp);
        let mut current: Option<PassKind> = None;
        let mut bound: Option<usize> = None;
        let mut run: Option<(PassKind, usize, u32, u32)> = None;
        let mut flush = |pass: &mut wgpu::RenderPass<'_>,
                         r: (PassKind, usize, u32, u32),
                         report: &mut FrameReport| {
            let (kind, g, start, end) = r;
            if current != Some(kind) {
                pass.set_pipeline(&self.pipelines[&kind]);
                current = Some(kind);
                report.pipeline_sets += 1;
            }
            if bound != Some(g) {
                pass.set_bind_group(1, &scene.groups[g], &[]);
                bound = Some(g);
                report.bind_sets += 1;
            }
            pass.draw_indexed(start..end, 0, 0..1);
            report.draw_calls += 1;
            report.world_draws += 1;
            report.world_tris += u64::from(end - start) / 3;
        };
        let n_draws = scene.draws.len();
        for k in 0..n_draws {
            let i = if mutate.order { n_draws - 1 - k } else { k };
            let (b, g) = &scene.draws[i];
            // a world model switched off by its object (RENDER_2009.md §5.1 step 6)
            if scene.draw_model[i].is_some_and(|m| scene.model_off[m]) {
                continue;
            }
            if self.world_culling {
                if let Some(&(lo, hi)) = scene.draw_bounds.get(i) {
                    if box_outside(&planes, lo, hi, CULL_MARGIN) {
                        report.world_culled += 1;
                        continue;
                    }
                }
            }
            let end = b.first_index + b.index_count;
            match run.as_mut() {
                Some(r) if r.0 == b.pass && r.1 == *g && r.3 == b.first_index => r.3 = end,
                _ => {
                    if let Some(r) = run.take() {
                        flush(&mut pass, r, &mut report);
                    }
                    run = Some((b.pass, *g, b.first_index, end));
                }
            }
        }
        if let Some(r) = run.take() {
            flush(&mut pass, r, &mut report);
        }
        // SPEC: RENDER_2009.md §7.1 step 6 (solid models after the solid world models), §4.3 ([0.1, 1.0])
        for k in 0..models.len() {
            let m = models[if mutate.order {
                models.len() - 1 - k
            } else {
                k
            }];
            m.record(&mut pass, camera.pos, &mut report);
            report.models += 1;
        }
        if opts.translucent && !scene.blended.is_empty() {
            // SPEC: RENDER_2009.md §7.1 step 9 (key = squared distance from the eye to the object position;
            // drawn back to front: farthest first; the comparator direction is UNPROVEN, open item 9)
            let eye = camera.pos;
            let mut order: Vec<(f32, usize)> = scene
                .blended
                .iter()
                .enumerate()
                .filter(|(_, m)| !m.model.is_some_and(|k| scene.model_off[k]))
                .map(|(i, m)| (m.origin.distance_squared(eye), i))
                .collect();
            if opts.sort_translucent {
                order.sort_by(|a, b| b.0.total_cmp(&a.0));
            }
            if mutate.order {
                order.reverse();
            }
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_vertex_buffer(0, scene.vertex_buffer.slice(..));
            pass.set_index_buffer(scene.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            report.bind_sets += 1;
            for (_, i) in order {
                let m = &scene.blended[i];
                let mut current: Option<PassKind> = None;
                for (b, g) in &m.draws {
                    if current != Some(b.pass) {
                        pass.set_pipeline(&self.blend_pipelines[&(b.pass, m.additive)]);
                        current = Some(b.pass);
                        report.pipeline_sets += 1;
                    }
                    pass.set_bind_group(2, &m.group, &[]);
                    pass.set_bind_group(1, &scene.groups[*g], &[]);
                    pass.draw_indexed(b.first_index..b.first_index + b.index_count, 0, 0..1);
                    report.bind_sets += 2;
                    report.draw_calls += 1;
                    report.world_tris += u64::from(b.index_count) / 3;
                }
                report.blended_objects += 1;
            }
        }
        if !view_models.is_empty() {
            // SPEC: RENDER_2009.md §4.2 / §4.3 (same rectangle, depth range [0.0, 0.1]; restored afterwards, which a
            // pass of its own does not need)
            pass.set_viewport(
                0.0,
                0.0,
                size.0 as f32,
                size.1 as f32,
                PLAYER_VIEW_DEPTH_RANGE.0,
                PLAYER_VIEW_DEPTH_RANGE.1,
            );
            for m in view_models {
                // eye-space instances: the LOD distance is the distance from the eye, at the origin
                m.record(&mut pass, Vec3::ZERO, &mut report);
                report.view_models += 1;
            }
        }
        report
    }

    /// CLI-1624: records the x-ray pass of `draws` ([`crate::XrayModel`]: chams, skeleton, box) into `frame` after the
    /// frame's 3D pass (colour and depth loaded; every instance of `draws` must be in that frame's model list). With
    /// `frame.resolve` set this is the frame's resolving pass (the resolve alone when nothing is drawn). Returns the
    /// draw calls recorded.
    pub fn encode_xray(
        &self,
        ctx: &GpuContext,
        camera: &Camera,
        frame: FrameTarget<'_>,
        draws: &[crate::XrayModel<'_>],
        encoder: &mut wgpu::CommandEncoder,
    ) -> u64 {
        if draws.is_empty() {
            frame.encode_resolve_only(encoder);
            return 0;
        }
        let pipes = self.xray.get_or_init(|| {
            crate::xray::XrayPipes::new(ctx, &self.models, self.colour_format, self.msaa)
        });
        pipes.encode(ctx, encoder, frame, camera, draws)
    }

    /// Renders one frame offscreen and reads it back as RGBA8 (alpha forced to 255).
    pub fn render_offscreen(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &Camera,
        width: u32,
        height: u32,
    ) -> Result<(FrameImage, f64), RenderError> {
        self.render_frame_offscreen(
            ctx,
            scene,
            camera,
            &FrameOptions::opaque_only(),
            &[],
            width,
            height,
        )
        .map(|(img, r)| (img, r.gpu_ms))
    }

    /// Renders a full frame (see [`WorldRenderer::encode_frame`]) offscreen and reads it back.
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame_offscreen(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &Camera,
        opts: &FrameOptions,
        models: &[&ModelInstance],
        width: u32,
        height: u32,
    ) -> Result<(FrameImage, FrameReport), RenderError> {
        self.render_frame_offscreen_with_view(
            ctx,
            scene,
            camera,
            opts,
            models,
            &[],
            &PlayerViewSettings::default(),
            width,
            height,
        )
    }

    /// Renders a full frame with eye-attached models (see [`WorldRenderer::encode_frame_with_view`]) offscreen
    /// and reads it back.
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame_offscreen_with_view(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &Camera,
        opts: &FrameOptions,
        models: &[&ModelInstance],
        view_models: &[&ModelInstance],
        pv: &PlayerViewSettings,
        width: u32,
        height: u32,
    ) -> Result<(FrameImage, FrameReport), RenderError> {
        self.render_frame_offscreen_xray(
            ctx,
            scene,
            camera,
            opts,
            models,
            view_models,
            pv,
            &[],
            width,
            height,
        )
    }

    /// CLI-1624: as [`WorldRenderer::render_frame_offscreen_with_view`], then the x-ray pass of `xray`
    /// ([`WorldRenderer::encode_xray`]; empty: no pass) before the read-back. The shop preview and the tests draw a
    /// style on a model with it.
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame_offscreen_xray(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &Camera,
        opts: &FrameOptions,
        models: &[&ModelInstance],
        view_models: &[&ModelInstance],
        pv: &PlayerViewSettings,
        xray: &[crate::XrayModel<'_>],
        width: u32,
        height: u32,
    ) -> Result<(FrameImage, FrameReport), RenderError> {
        let device = &ctx.device;
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let target = crate::perf::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("offscreen colour"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.colour_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
        );
        let depth = crate::perf::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("offscreen depth"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: ZBUF_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
        );
        let tv = target.create_view(&Default::default());
        let dv = depth.create_view(&Default::default());
        // CLI-1593: with MSAA the passes draw into multisampled textures and the world pass (the last 3D pass of
        // this frame) resolves into the read-back target
        let ms = crate::msaa::MsaaTargets::new(ctx, self.colour_format, (width, height), self.msaa);
        let t = Instant::now();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
        let pass_target = |resolves: bool| match &ms {
            Some(ms) => ms.target(resolves.then_some(&tv)),
            None => FrameTarget {
                colour: &tv,
                depth: &dv,
                size: (width, height),
                resolve: None,
            },
        };
        let mut report = self.encode_frame_with_view(
            ctx,
            scene,
            camera,
            pass_target(xray.is_empty()),
            opts,
            models,
            view_models,
            pv,
            &mut enc,
        );
        if !xray.is_empty() {
            report.draw_calls += self.encode_xray(ctx, camera, pass_target(true), xray, &mut enc);
        }
        ctx.queue.submit([enc.finish()]);
        ctx.wait();
        report.gpu_ms = t.elapsed().as_secs_f64() * 1000.0;
        let img = read_back_texture(ctx, &target, width, height, self.colour_format)?;
        Ok((img, report))
    }

    /// The editor viewport's entry (docs/MAPKIT_PLAN.md D7, MAP-27): records the same frame as
    /// [`WorldRenderer::render_frame_offscreen_xray`] (sky, world, models, translucent objects, eye-attached models,
    /// then the x-ray pass when `xray` is not empty) into `target`, a colour + depth pair the caller keeps across
    /// frames ([`OffscreenTarget`]), and submits it. The colour stays on the GPU: the caller samples
    /// [`OffscreenTarget::view`] from its own pass (an `egui::PaintCallback`, a blit) and the queue's submission
    /// order guarantees the frame is complete by then; nothing blocks here, so [`FrameReport::gpu_ms`] stays 0.
    /// [`OffscreenTarget::read_back`] fetches the pixels when a caller wants them (a viewport screenshot).
    ///
    /// `target` must have been made for this renderer's colour format and sample count
    /// ([`OffscreenTarget::for_renderer`]); with MSAA on the frame's last 3D pass resolves into the single-sample
    /// colour texture, exactly as the read-back path does.
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame_to_target(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &Camera,
        opts: &FrameOptions,
        models: &[&ModelInstance],
        view_models: &[&ModelInstance],
        pv: &PlayerViewSettings,
        xray: &[crate::XrayModel<'_>],
        target: &OffscreenTarget,
    ) -> FrameReport {
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viewport frame"),
            });
        let mut report = self.encode_frame_with_view(
            ctx,
            scene,
            camera,
            target.frame_target(xray.is_empty()),
            opts,
            models,
            view_models,
            pv,
            &mut enc,
        );
        if !xray.is_empty() {
            report.draw_calls +=
                self.encode_xray(ctx, camera, target.frame_target(true), xray, &mut enc);
        }
        ctx.queue.submit([enc.finish()]);
        report
    }
}

/// Copies `texture` (a `width` x `height` single-sample colour texture of `format` with `COPY_SRC`) into a
/// mapped buffer and unpacks it as RGBA8 with alpha 255: the one read-back of every offscreen path (the frame
/// read-backs above and [`OffscreenTarget::read_back`]). Blocks until the GPU has finished every earlier
/// submission.
pub(crate) fn read_back_texture(
    ctx: &GpuContext,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> Result<FrameImage, RenderError> {
    let device = &ctx.device;
    let extent = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let row = 4 * width;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = crate::perf::buffer(
        device,
        &wgpu::BufferDescriptor {
            label: Some("read-back"),
            size: padded as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        },
    );
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("copy"),
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        extent,
    );
    ctx.queue.submit([enc.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    ctx.wait();
    rx.recv()
        .map_err(|e| RenderError::Readback(e.to_string()))?
        .map_err(|e| RenderError::Readback(e.to_string()))?;
    let data = slice.get_mapped_range();
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let rgba = unpack_rows(&data, width, height, padded, bgra);
    drop(data);
    buffer.unmap();
    Ok(FrameImage {
        width,
        height,
        rgba,
    })
}

/// A colour + depth target a caller keeps between frames and draws [`WorldRenderer::render_frame_to_target`]
/// into: the editor viewport of dc-mapkit (docs/MAPKIT_PLAN.md D7) samples its colour texture from an
/// `egui::PaintCallback` instead of reading the pixels back.
///
/// The colour texture is single-sample, of the renderer's colour format ([`OffscreenTarget::format`];
/// [`OFFSCREEN_FORMAT`] = `Rgba8Unorm` for a headless renderer), made with `RENDER_ATTACHMENT | TEXTURE_BINDING |
/// COPY_SRC`, so it can be sampled by any pass on the same device and still read back
/// ([`OffscreenTarget::read_back`], the same unpacking as the frame read-backs). With the renderer's MSAA count
/// above 1 the target also holds the multisampled colour and depth ([`MsaaTargets`](crate::MsaaTargets)) the 3D
/// passes draw into, and the frame's last 3D pass resolves into the colour texture (CLI-1593). Sizes are clamped
/// to at least 1 x 1; [`OffscreenTarget::resize`] makes the textures again only when the size changed (the
/// previous textures are dropped: a caller that registered the old view with its UI toolkit re-registers the new
/// one, [`OffscreenTarget::generation`] tells it when).
// DC-DEFAULT: MAP-27 (the viewport target: format, usage, MSAA resolve, resize, read-back)
pub struct OffscreenTarget {
    colour: wgpu::Texture,
    colour_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    msaa: Option<crate::msaa::MsaaTargets>,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    samples: u32,
    generation: u64,
}

impl fmt::Debug for OffscreenTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OffscreenTarget")
            .field("size", &self.size)
            .field("format", &self.format)
            .field("samples", &self.samples)
            .field("generation", &self.generation)
            .finish()
    }
}

impl OffscreenTarget {
    /// A target of `size` for frames in `format` with `samples` per pixel (1 = MSAA off).
    pub fn new(
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        samples: u32,
        size: (u32, u32),
    ) -> OffscreenTarget {
        let size = (size.0.max(1), size.1.max(1));
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let colour = crate::perf::texture(
            &ctx.device,
            &wgpu::TextureDescriptor {
                label: Some("viewport colour"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
        );
        let depth = crate::perf::texture(
            &ctx.device,
            &wgpu::TextureDescriptor {
                label: Some("viewport depth"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: ZBUF_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
        );
        OffscreenTarget {
            colour_view: colour.create_view(&Default::default()),
            colour,
            depth_view: depth.create_view(&Default::default()),
            _depth: depth,
            msaa: crate::msaa::MsaaTargets::new(ctx, format, size, samples),
            size,
            format,
            samples: samples.max(1),
            generation: 0,
        }
    }

    /// A target for `renderer`'s colour format and sample count.
    pub fn for_renderer(
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        size: (u32, u32),
    ) -> OffscreenTarget {
        OffscreenTarget::new(ctx, renderer.colour_format(), renderer.msaa(), size)
    }

    /// Makes the textures again at `size` when it differs from the current one (clamped to 1 x 1); returns
    /// whether they were remade (then [`OffscreenTarget::view`] is a new view).
    pub fn resize(&mut self, ctx: &GpuContext, size: (u32, u32)) -> bool {
        let size = (size.0.max(1), size.1.max(1));
        if size == self.size {
            return false;
        }
        let generation = self.generation + 1;
        *self = OffscreenTarget::new(ctx, self.format, self.samples, size);
        self.generation = generation;
        true
    }

    /// Counts the times the textures were remade ([`OffscreenTarget::resize`]); a caller that keeps a handle to
    /// the view compares it with the one it registered.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The colour texture's format (what a sampler or UI toolkit registers the view with).
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Samples per pixel the 3D passes draw with (1 = MSAA off).
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// The single-sample colour texture (the frame once drawn, resolved with MSAA on).
    pub fn texture(&self) -> &wgpu::Texture {
        &self.colour
    }

    /// The view of [`OffscreenTarget::texture`] a caller samples from.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.colour_view
    }

    /// The frame target of one 3D pass into this target: with MSAA on the multisampled attachments, resolving
    /// into the colour texture when `resolves` (the frame's last 3D pass); with MSAA off the colour and depth
    /// views themselves (`resolves` is then irrelevant). What [`WorldRenderer::render_frame_to_target`] hands
    /// each pass; a caller that records its own passes into the same encoder (debug boxes, effect quads) uses it
    /// the same way.
    pub fn frame_target(&self, resolves: bool) -> FrameTarget<'_> {
        match &self.msaa {
            Some(ms) => ms.target(resolves.then_some(&self.colour_view)),
            None => FrameTarget::new(&self.colour_view, &self.depth_view, self.size),
        }
    }

    /// The colour texture's pixels as RGBA8 (alpha 255; the same unpacking as the frame read-backs). Blocks until
    /// every earlier submission is done.
    pub fn read_back(&self, ctx: &GpuContext) -> Result<FrameImage, RenderError> {
        read_back_texture(ctx, &self.colour, self.size.0, self.size.1, self.format)
    }
}

/// Rows of a mapped read-back buffer (`padded` bytes apart, 4 bytes per pixel) as tight RGBA8 with alpha 255;
/// `bgra` swaps the first and third channel. Used by every offscreen read-back.
pub fn unpack_rows(data: &[u8], width: u32, height: u32, padded: u32, bgra: bool) -> Vec<u8> {
    let row = 4 * width as usize;
    let mut rgba = vec![0u8; row * height as usize];
    for (dst, src) in rgba.chunks_exact_mut(row).zip(data.chunks(padded as usize)) {
        dst.copy_from_slice(&src[..row]);
        for p in dst.as_chunks_mut::<4>().0 {
            if bgra {
                p.swap(0, 2);
            }
            p[3] = 255;
        }
    }
    rgba
}

/// What a full frame draws besides the opaque world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameOptions {
    /// Sky pass with these settings; `None` leaves the clear colour behind the sky portals.
    pub sky: Option<SkySettings>,
    /// Translucent and additive world models.
    pub translucent: bool,
    /// Sort the translucent objects back to front (`sort_translucent`, default 1); off draws in scene order.
    // SPEC: RENDER_2009.md §13 (sort_translucent default 1), §7.1 step 9
    pub sort_translucent: bool,
    /// The colour the frame is cleared to (`None`: [`CLEAR_COLOUR`]). The menu's character view clears to a
    /// transparent colour and keys it out (engineering only, no 2009 rule).
    pub clear: Option<[f64; 4]>,
}

impl Default for FrameOptions {
    fn default() -> Self {
        FrameOptions {
            sky: Some(SkySettings::default()),
            translucent: true,
            sort_translucent: true,
            clear: None,
        }
    }
}

impl FrameOptions {
    /// The first slice's frame: opaque world only.
    pub fn opaque_only() -> FrameOptions {
        FrameOptions {
            sky: None,
            translucent: false,
            sort_translucent: true,
            clear: None,
        }
    }
}

/// What a recorded frame drew.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameReport {
    /// Scissor of the sky pass `[x0, y0, x1, y1]` in pixels, when the sky was drawn.
    pub sky_rect: Option<[u32; 4]>,
    pub sky_eye: Option<[f32; 3]>,
    pub sky_draws: u64,
    pub models: u64,
    pub blended_objects: u64,
    /// Eye-attached models drawn with the player-view camera.
    pub view_models: u64,
    /// Submit-to-completion time of the frame, ms (offscreen renders only).
    pub gpu_ms: f64,
    /// Commands the frame recorded: draw calls, bind-group sets and pipeline sets over every pass of
    /// [`WorldRenderer::encode_frame_with_view`] (sky, world, models, translucent objects, view models).
    pub draw_calls: u64,
    pub bind_sets: u64,
    pub pipeline_sets: u64,
    /// Opaque world draw calls recorded, and world batches left out because their bounds lie outside the view
    /// frustum.
    pub world_draws: u64,
    pub world_culled: u64,
    /// Bytes of model uniforms (instance block + bone palette) written to the queue for this frame.
    pub uniform_bytes: u64,
    /// World triangles drawn (opaque world, sky and translucent world models; perf-g1's visibility measure).
    pub world_tris: u64,
}

/// One translucent or additive world model on the GPU.
struct GpuBlended {
    draws: Vec<(Batch, usize)>,
    origin: Vec3,
    /// `origin` at the file position (before any model transform).
    base_origin: Vec3,
    additive: bool,
    group: wgpu::BindGroup,
    /// Its scene model (index into [`GpuScene::model_names`]).
    model: Option<usize>,
}

/// Bind-group key: (first texture, second texture, lightmap).
type MaterialKey = (Option<u32>, Option<u32>, Option<u32>);

/// Where a frame goes: colour and depth views of one size.
///
/// With MSAA on (`RenderSettings::msaa` > 1) `colour` and `depth` are the multisampled textures of
/// [`crate::MsaaTargets`] (same sample count as the pipelines), and `resolve` names the single-sample view the frame's
/// LAST 3D pass resolves the samples into (the gamma stage's target or the screen); every earlier pass into the same
/// target is recorded with `resolve: None`. Exactly one pass per frame resolves. With MSAA off `resolve` is `None`
/// and the attachments are what they always were.
#[derive(Debug, Clone, Copy)]
pub struct FrameTarget<'a> {
    pub colour: &'a wgpu::TextureView,
    pub depth: &'a wgpu::TextureView,
    pub size: (u32, u32),
    /// CLI-1593: the single-sample view this pass resolves `colour` into, for the frame's last 3D pass with MSAA on.
    /// `None`: no resolve in this pass (MSAA off, or a later pass of the frame resolves).
    pub resolve: Option<&'a wgpu::TextureView>,
}

impl<'a> FrameTarget<'a> {
    /// A single-sample target (no resolve): the frame as drawn with MSAA off, or a pass before the resolving one.
    pub fn new(colour: &'a wgpu::TextureView, depth: &'a wgpu::TextureView, size: (u32, u32)) -> Self {
        FrameTarget {
            colour,
            depth,
            size,
            resolve: None,
        }
    }

    /// The colour attachment of a pass into this target: `load`, then stored; with `resolve` set the samples are
    /// resolved into it and the multisampled colour is discarded (nothing reads it after the resolve).
    pub fn colour_attachment(
        &self,
        load: wgpu::LoadOp<wgpu::Color>,
    ) -> Option<wgpu::RenderPassColorAttachment<'a>> {
        Some(wgpu::RenderPassColorAttachment {
            view: self.colour,
            resolve_target: self.resolve,
            ops: wgpu::Operations {
                load,
                store: if self.resolve.is_some() {
                    wgpu::StoreOp::Discard
                } else {
                    wgpu::StoreOp::Store
                },
            },
        })
    }

    /// The depth attachment of a pass into this target: `load`, then stored, except in the resolving pass (the
    /// frame's last 3D pass), which discards the multisampled depth.
    pub fn depth_attachment(
        &self,
        load: wgpu::LoadOp<f32>,
    ) -> Option<wgpu::RenderPassDepthStencilAttachment<'a>> {
        Some(wgpu::RenderPassDepthStencilAttachment {
            view: self.depth,
            depth_ops: Some(wgpu::Operations {
                load,
                store: if self.resolve.is_some() {
                    wgpu::StoreOp::Discard
                } else {
                    wgpu::StoreOp::Store
                },
            }),
            stencil_ops: None,
        })
    }

    /// Records a pass that draws nothing and only resolves the samples into `resolve` (the fallback of a pass that
    /// was named the frame's resolving pass but had nothing to draw, so the frame still resolves exactly once).
    /// Nothing is recorded without `resolve`.
    pub fn encode_resolve_only(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.resolve.is_none() {
            return;
        }
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("msaa resolve"),
            color_attachments: &[self.colour_attachment(wgpu::LoadOp::Load)],
            depth_stencil_attachment: self.depth_attachment(wgpu::LoadOp::Load),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct MaterialInfo {
    alpha_cutoff: Option<u8>,
    bias: f32,
}

pub(crate) fn upload_image(ctx: &GpuContext, img: &TextureImage, label: &str) -> wgpu::Texture {
    let tex = crate::perf::texture(
        &ctx.device,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: img.width.max(1),
                height: img.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: img.levels.len().max(1) as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
    );
    for (i, level) in img.levels.iter().enumerate() {
        let w = (img.width >> i).max(1);
        let h = (img.height >> i).max(1);
        if level.len() < (w * h * 4) as usize {
            continue;
        }
        crate::perf::write_texture(
            &ctx.queue,
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: i as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &level[..(w * h * 4) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }
    tex
}

/// Upload counters and times.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UploadStats {
    pub textures: u64,
    pub missing_textures: u64,
    pub texture_bytes: u64,
    pub lightmap_bytes: u64,
    pub draws: u64,
    /// Sky-pass draws and translucent objects uploaded.
    pub sky_draws: u64,
    pub blended_objects: u64,
    pub bind_groups: u64,
    /// Time spent in the caller's image callback (ROM read + DTX decode).
    pub decode_ms: f64,
    /// Time spent creating and filling GPU resources.
    pub upload_ms: f64,
}

/// A scene resident on the GPU.
pub struct GpuScene {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    draws: Vec<(Batch, usize)>,
    /// The scene model of each draw (`None`: the level geometry).
    draw_model: Vec<Option<usize>>,
    /// World-space bounds (min, max) of each entry of `draws`, and the same at the file position.
    draw_bounds: Vec<(Vec3, Vec3)>,
    draw_bounds_base: Vec<(Vec3, Vec3)>,
    /// Per world model ([`GpuScene::model_names`] order): its vertex range at the file position (first vertex, the
    /// vertices), its current transform (`None`: the file position), its transformed vertices (a reused buffer)
    /// and the models whose vertex range still has to be written to the GPU.
    model_base: Vec<(u32, Vec<GpuVertex>)>,
    model_transform: Vec<Option<Affine3A>>,
    model_posed: Vec<Vec<GpuVertex>>,
    model_pending: std::sync::Mutex<Vec<usize>>,
    sky_draws: Vec<(Batch, usize)>,
    blended: Vec<GpuBlended>,
    /// Names of the world models of the scene ([`SceneMesh::models`] order) and which of them are switched off.
    model_names: Vec<Vec<u8>>,
    model_off: Vec<bool>,
    groups: Vec<wgpu::BindGroup>,
    _textures: Vec<wgpu::Texture>,
    /// Bounds of the sky geometry (the default sky view box is its centre).
    pub sky_bounds: Option<([f32; 3], [f32; 3])>,
    pub sky_portals: Vec<Vec<[f32; 3]>>,
    /// The world's extents (the sky eye is placed by the eye's position inside them).
    pub world_extents: Option<([f32; 3], [f32; 3])>,
    pub stats: UploadStats,
    /// Linear fog of the world pass (world sections and models) and of the sky pass; `None` = no fog. Taken from
    /// the scene mesh (the world settings); a caller may replace it (for example with a server's values).
    pub fog: Option<Fog>,
    pub sky_fog: Option<Fog>,
    /// The sky model of each entry of `sky_draws`, the sky models' names and which are switched off.
    sky_draw_model: Vec<Option<usize>>,
    sky_model_names: Vec<Vec<u8>>,
    sky_model_off: Vec<bool>,
    /// The sky view box of the server's sky table; when set it replaces the frame options' view box (`None`: the
    /// frame options decide, the viewer's default).
    // EVIDENCE: engine+0xa44f0 (DISASM: the table's 48-byte block is kept in the client state), engine+0xfb8c0 (the
    // sky eye is placed in floats 6..11 of that block)
    pub sky_view_box: Option<([f32; 3], [f32; 3])>,
}

impl GpuScene {
    /// Names of the sky world models of the scene ([`SceneMesh::sky_models`] order).
    pub fn sky_model_names(&self) -> &[Vec<u8>] {
        &self.sky_model_names
    }

    /// Switches the sky model `i` of [`GpuScene::sky_model_names`] on or off for the sky pass (all are on after the
    /// upload; the 2009 client draws only the sky objects its server's sky table names).
    // EVIDENCE: engine+0x188c0 (only the table's objects become sky objects), engine+0x193ea0 (no sky pass without
    // sky objects)
    pub fn set_sky_model_shown(&mut self, i: usize, on: bool) {
        if let Some(o) = self.sky_model_off.get_mut(i) {
            *o = !on;
        }
    }

    /// Whether any sky draw is left after the sky model switches.
    fn sky_drawn(&self, k: usize) -> bool {
        !self
            .sky_draw_model
            .get(k)
            .copied()
            .flatten()
            .is_some_and(|m| self.sky_model_off.get(m).copied().unwrap_or(false))
    }
}

impl GpuScene {
    /// Names of the world models of the scene (nested render worlds built into the main view or the translucent pass).
    pub fn model_names(&self) -> &[Vec<u8>] {
        &self.model_names
    }

    /// Switches the world model `i` of [`GpuScene::model_names`] on or off (its draws are skipped while off).
    // SPEC: RENDER_2009.md §5.1 step 6 (the object queue draws a world model only with the visible bit of its object)
    pub fn set_model_shown(&mut self, i: usize, on: bool) {
        if let Some(o) = self.model_off.get_mut(i) {
            *o = !on;
        }
    }

    /// Whether the world model `i` is drawn.
    pub fn model_shown(&self, i: usize) -> bool {
        self.model_off.get(i).is_some_and(|o| !o)
    }

    /// Places the world model `i` of [`GpuScene::model_names`]: every vertex `p` of it is drawn at `t * p` (`None`:
    /// at its file position). The model's vertex range is rewritten on the GPU at the next frame; its culling bounds
    /// and (for a translucent model) its sort position follow. Nothing is done when `t` equals the current
    /// transform, and nothing is allocated after the model's first move.
    // SPEC: RENDER_2009.md §5.1 step 6 (world models are drawn by the object queue, placed by their object)
    // DC-DEFAULT: REN-62 (proposed, w1-doors) the moved model's vertices are rewritten in place
    pub fn set_model_transform(&mut self, i: usize, t: Option<Affine3A>) {
        let Some(cur) = self.model_transform.get_mut(i) else {
            return;
        };
        if *cur == t {
            return;
        }
        *cur = t;
        let (_, base) = &self.model_base[i];
        let posed = &mut self.model_posed[i];
        posed.clear();
        if let Some(a) = t {
            posed.extend(base.iter().map(|v| GpuVertex {
                pos: a.transform_point3(Vec3::from(v.pos)).to_array(),
                ..*v
            }));
        }
        for (d, m) in self.draw_model.iter().enumerate() {
            if *m == Some(i) {
                let b = self.draw_bounds_base[d];
                self.draw_bounds[d] = match t {
                    Some(a) => transformed_bounds(a, b),
                    None => b,
                };
            }
        }
        for b in &mut self.blended {
            if b.model == Some(i) {
                b.origin = t.map_or(b.base_origin, |a| a.transform_point3(b.base_origin));
            }
        }
        let mut p = self.model_pending.lock().unwrap_or_else(|e| e.into_inner());
        if !p.contains(&i) {
            p.push(i);
        }
    }

    /// The current transform of world model `i` (`None`: the file position).
    pub fn model_transform(&self, i: usize) -> Option<Affine3A> {
        self.model_transform.get(i).copied().flatten()
    }

    /// Writes the vertex ranges of the models placed since the last frame.
    fn upload_model_poses(&self, queue: &wgpu::Queue) {
        let mut p = self.model_pending.lock().unwrap_or_else(|e| e.into_inner());
        for &i in p.iter() {
            let (first, base) = &self.model_base[i];
            let data = if self.model_transform[i].is_some() {
                &self.model_posed[i]
            } else {
                base
            };
            if !data.is_empty() {
                let at = *first as u64 * std::mem::size_of::<GpuVertex>() as u64;
                queue.write_buffer(&self.vertex_buffer, at, bytemuck::cast_slice(data));
            }
        }
        p.clear();
    }
}

/// The vertex range of each world model of `mesh` (from the lowest to the highest vertex its indices use: the scene
/// builder appends a model's vertices together with its indices) and a copy of those vertices.
fn model_vertex_ranges(mesh: &SceneMesh) -> Vec<(u32, Vec<GpuVertex>)> {
    mesh.models
        .iter()
        .map(|m| {
            let idx = mesh
                .indices
                .get(m.first_index as usize..m.end_index as usize)
                .unwrap_or(&[]);
            let lo = idx.iter().copied().min();
            let hi = idx.iter().copied().max();
            match (lo, hi) {
                (Some(lo), Some(hi)) if (hi as usize) < mesh.vertices.len() => {
                    (lo, mesh.vertices[lo as usize..=hi as usize].to_vec())
                }
                _ => (0, Vec::new()),
            }
        })
        .collect()
}

/// The axis-aligned box around the eight corners of `b` moved by `a` (an infinite box stays infinite).
fn transformed_bounds(a: Affine3A, b: (Vec3, Vec3)) -> (Vec3, Vec3) {
    if !(b.0.is_finite() && b.1.is_finite()) {
        return b;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for k in 0..8 {
        let c = Vec3::new(
            if k & 1 == 0 { b.0.x } else { b.1.x },
            if k & 2 == 0 { b.0.y } else { b.1.y },
            if k & 4 == 0 { b.0.z } else { b.1.z },
        );
        let p = a.transform_point3(c);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    (lo, hi)
}

impl fmt::Debug for GpuScene {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuScene")
            .field("stats", &self.stats)
            .finish()
    }
}

/// Timings of one viewer run, milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameTimings {
    /// World parse (FMT_DAT85) of the file bytes.
    pub parse_ms: f64,
    /// CPU scene build: vertices, indices, buckets, lightmap unpacking.
    pub scene_build_ms: f64,
    /// ROM texture reads and DTX decoding.
    pub texture_decode_ms: f64,
    /// GPU resource creation and uploads.
    pub upload_ms: f64,
    /// One frame on the GPU (submit to completion), without the read-back.
    pub gpu_frame_ms: f64,
}

/// A read-back frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl FrameImage {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[o],
            self.rgba[o + 1],
            self.rgba[o + 2],
            self.rgba[o + 3],
        ]
    }

    /// The exact-lane hash of the frame's texels ([`crate::texel_hash`], docs/RENDER_DETERMINISM_CONTRACT.md).
    pub fn texel_hash(&self) -> [u8; 32] {
        crate::determinism::texel_hash(self.width, self.height, &self.rgba)
    }

    /// Writes a PNG.
    pub fn save_png(&self, path: &Path) -> Result<(), RenderError> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| RenderError::Image(e.to_string()))?;
            }
        }
        image::save_buffer_with_format(
            path,
            &self.rgba,
            self.width,
            self.height,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|e| RenderError::Image(e.to_string()))
    }
}

/// A second handle on the same device and queue (wgpu handles are reference-counted), for model uploads on loader
/// threads. The adapter's instance, adapter and info are shared too.
impl Clone for GpuContext {
    fn clone(&self) -> GpuContext {
        GpuContext {
            instance: self.instance.clone(),
            adapter: self.adapter.clone(),
            device: self.device.clone(),
            queue: self.queue.clone(),
            info: self.info.clone(),
        }
    }
}

impl WorldRenderer {
    /// A handle that uploads models with this renderer's model pipelines on any thread ([`crate::ModelUploader`]).
    pub fn model_uploader(&self, ctx: &GpuContext) -> crate::ModelUploader {
        crate::ModelUploader::new(ctx.clone(), self.models.clone())
    }

    /// A new instance of an uploaded model sharing its GPU mesh, textures and passes
    /// ([`ModelPipelines::instantiate`]).
    pub fn instantiate_model(&self, ctx: &GpuContext, template: &ModelInstance) -> ModelInstance {
        self.models.instantiate(ctx, template)
    }
}
