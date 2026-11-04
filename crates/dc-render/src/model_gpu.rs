//! GPU side of the model passes: the model pipelines (skinning, per-vertex lighting with the render style's
//! material and the render style's texture stages, `model.wgsl`; one pipeline per pass raster state) and
//! [`ModelInstance`], one uploaded model with every LOD, its pose, world transform and light set.

use crate::anim::{AnimSet, Tracker};
use crate::camera::Camera;
use crate::fog::Fog;
use crate::gpu::{FrameReport, GpuContext};
use crate::lights::{Falloff, LightSet, LightShape, MAX_MODEL_LIGHTS};
use crate::model::{
    select_lod, ModelFiles, ModelMesh, ModelMeshStats, ModelVertex, PieceDraw, MAX_BONES,
};
use crate::style::{
    Address, BlendFactor, Compare, Cull, Filter, Pass, RenderStyle, StageOp, StageTexture, UvSource,
};
use crate::texture::TextureImage;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex};

const MODEL_WGSL: &str = include_str!("shaders/model.wgsl");

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct LightUniform {
    pos_radius: [f32; 4],
    colour: [f32; 4],
    dir_cos: [f32; 4],
    kind: [f32; 4],
    coefs: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct InstanceUniform {
    clip_from_model: [[f32; 4]; 4],
    world_from_model: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    ambient: [f32; 4],
    eye: [f32; 4],
    lights: [LightUniform; MAX_MODEL_LIGHTS],
    fog: [f32; 4],
    fog_colour: [f32; 4],
}

fn light_uniform(l: &crate::lights::ModelLight) -> LightUniform {
    let (shape, dir, cos) = match l.shape {
        LightShape::Point => (0.0, Vec3::Z, -1.0),
        LightShape::Spot { dir, cos_half } => (1.0, dir.normalize_or_zero(), cos_half),
        LightShape::Directional { dir } => (2.0, dir.normalize_or_zero(), -1.0),
    };
    let (fk, c) = match l.falloff {
        Falloff::D3d(c) => (0.0, c),
        Falloff::Linear => (1.0, [0.0; 3]),
        Falloff::Quartic => (2.0, [0.0; 3]),
    };
    // SPEC: RENDER_2009.md §6.3 (diffuse = light colour / 255)
    let col = (l.colour / 255.0).max(Vec3::ZERO);
    LightUniform {
        pos_radius: [l.pos.x, l.pos.y, l.pos.z, l.radius],
        colour: [col.x, col.y, col.z, 1.0],
        dir_cos: [dir.x, dir.y, dir.z, cos],
        kind: [shape, fk, 0.0, 0.0],
        coefs: [c[0], c[1], c[2], 0.0],
    }
}

/// Raster state of one model pipeline (one per distinct render-style pass state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PipeKey {
    blend: Option<(BlendFactor, BlendFactor)>,
    depth_test: bool,
    depth_write: bool,
    depth_compare: Compare,
    cull: Cull,
}

impl PipeKey {
    fn of(p: &Pass) -> PipeKey {
        PipeKey {
            blend: p.blend,
            depth_test: p.depth_test,
            depth_write: p.depth_write && p.depth_test,
            depth_compare: p.depth_compare,
            cull: p.cull,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SampKey {
    address: [Address; 2],
    filter: Filter,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct StageUniform {
    ops: [u32; 4],
    ops2: [u32; 4],
    xform: [f32; 16],
    extra: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct PassUniform {
    stages: [StageUniform; 4],
    factor: [f32; 4],
    alpha: [f32; 4],
    mat_ambient: [f32; 4],
    mat_diffuse: [f32; 4],
    mat_emissive: [f32; 4],
    mat_specular: [f32; 4],
    fx_a: [f32; 4],
    fx_b: [f32; 4],
}

/// The seconds the camo effects run on (DC_DEFAULTS.md VAR-4), wrapped so they stay exact in a float.
fn fx_clock() -> f32 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f32() % 3600.0
}

fn blend_factor(f: BlendFactor) -> wgpu::BlendFactor {
    // public Direct3D 9 D3DBLEND meanings mapped to the wgpu factors
    match f {
        BlendFactor::Zero => wgpu::BlendFactor::Zero,
        BlendFactor::One => wgpu::BlendFactor::One,
        BlendFactor::SrcColour => wgpu::BlendFactor::Src,
        BlendFactor::InvSrcColour => wgpu::BlendFactor::OneMinusSrc,
        BlendFactor::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
        BlendFactor::InvSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
        BlendFactor::DestColour => wgpu::BlendFactor::Dst,
        BlendFactor::InvDestColour => wgpu::BlendFactor::OneMinusDst,
    }
}

fn compare_fn(c: Compare) -> wgpu::CompareFunction {
    match c {
        Compare::Always => wgpu::CompareFunction::Always,
        Compare::Less => wgpu::CompareFunction::Less,
        Compare::LessEqual => wgpu::CompareFunction::LessEqual,
        Compare::Greater => wgpu::CompareFunction::Greater,
        Compare::GreaterEqual => wgpu::CompareFunction::GreaterEqual,
        Compare::Equal => wgpu::CompareFunction::Equal,
        Compare::NotEqual => wgpu::CompareFunction::NotEqual,
    }
}

fn address_mode(a: Address) -> wgpu::AddressMode {
    match a {
        Address::Wrap => wgpu::AddressMode::Repeat,
        Address::Clamp => wgpu::AddressMode::ClampToEdge,
        // DC-DEFAULT: REN-4 (proposed), see DC_DEFAULTS.md (wgpu has no mirror-once mode; mirror is the nearest)
        Address::Mirror | Address::MirrorOnce => wgpu::AddressMode::MirrorRepeat,
    }
}

/// ARGB word to RGBA in [0, 1].
fn argb(v: u32) -> [f32; 4] {
    let c = |s: u32| ((v >> s) & 0xff) as f32 / 255.0;
    [c(16), c(8), c(0), c(24)]
}

/// Pipelines (one per pass state, created on first use), samplers (one per address / filter pair) and the
/// layouts of the model pass for one colour format.
pub struct ModelPipelines {
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    colour_format: wgpu::TextureFormat,
    zbuf_format: wgpu::TextureFormat,
    pipelines: Mutex<HashMap<PipeKey, wgpu::RenderPipeline>>,
    samplers: Mutex<HashMap<SampKey, wgpu::Sampler>>,
    instance_layout: wgpu::BindGroupLayout,
    pass_layout: wgpu::BindGroupLayout,
    mip_bias: f32,
    anisotropy: u16,
    /// CLI-1593: samples per pixel of the frame target the pipelines draw into (1: MSAA off).
    msaa: u32,
    /// Bound to stages without a texture: opaque black.
    // DC-DEFAULT: REN-1 (proposed), see DC_DEFAULTS.md (empty stage reads opaque black; UNPROVEN for D3D9)
    black: wgpu::TextureView,
    fx_sampler: wgpu::Sampler,
    _black_tex: wgpu::Texture,
}

impl fmt::Debug for ModelPipelines {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelPipelines").finish()
    }
}

fn uniform_entry(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// What one model entry is drawn with: its texture list (`textures[slot]` for material slot `slot`; `None`
/// or a slot past the list: the missing-texture checker) and its render styles.
#[derive(Debug, Clone, Copy)]
pub struct ModelLook<'a> {
    pub textures: &'a [Option<TextureImage>],
    pub styles: StyleChoice<'a>,
}

/// Which render styles a model upload uses.
#[derive(Debug, Clone, Copy)]
pub enum StyleChoice<'a> {
    /// Every piece with [`RenderStyle::plain`] (the model state before styles were decoded).
    Plain,
    /// The object's style list (`list[i]` for style slot `i`; a missing entry or a slot past the list: the
    /// engine's default style, [`RenderStyle::engine_default`]).
    List(&'a [Option<Arc<RenderStyle>>]),
}

type GpuTexture = (wgpu::Texture, wgpu::TextureView, TextureImageInfo);

impl ModelPipelines {
    pub fn new(
        ctx: &GpuContext,
        colour_format: wgpu::TextureFormat,
        zbuf_format: wgpu::TextureFormat,
        mip_bias: f32,
        anisotropy: u16,
        msaa: u32,
    ) -> ModelPipelines {
        let device = &ctx.device;
        let module = crate::determinism::shader_module(
            device,
            wgpu::ShaderModuleDescriptor {
                label: Some("model.wgsl"),
                source: wgpu::ShaderSource::Wgsl(MODEL_WGSL.into()),
            },
        );
        let instance_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model instance"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                uniform_entry(1, wgpu::ShaderStages::VERTEX),
            ],
        });
        let mut entries = vec![uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)];
        for k in 0..4u32 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 1 + k,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        for k in 0..4u32 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 5 + k,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        // VAR-4: a camo effect's tile and mask, and their sampler
        for k in 0..2u32 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 9 + k,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 11,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
        let fx_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("camo effect"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model pass"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model"),
            bind_group_layouts: &[&instance_layout, &pass_layout],
            push_constant_ranges: &[],
        });
        let black_tex = crate::gpu::upload_image(
            ctx,
            &TextureImage::rgba(1, 1, vec![0, 0, 0, 255]),
            "model empty stage",
        );
        let black = black_tex.create_view(&Default::default());
        ModelPipelines {
            module,
            layout,
            colour_format,
            zbuf_format,
            pipelines: Mutex::new(HashMap::new()),
            samplers: Mutex::new(HashMap::new()),
            instance_layout,
            pass_layout,
            mip_bias,
            anisotropy,
            msaa: msaa.max(1),
            black,
            fx_sampler,
            _black_tex: black_tex,
        }
    }

    /// The bind-group layout of an instance's uniforms (group 0: instance block, bone palette), for passes that draw
    /// the model's geometry with a pipeline of their own (the x-ray chams, CLI-1624).
    pub(crate) fn instance_layout(&self) -> &wgpu::BindGroupLayout {
        &self.instance_layout
    }

    /// The depth format the model pipelines are built for.
    pub(crate) fn zbuf_format(&self) -> wgpu::TextureFormat {
        self.zbuf_format
    }

    fn pipeline(&self, ctx: &GpuContext, key: PipeKey) -> wgpu::RenderPipeline {
        let mut map = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = map.get(&key) {
            return p.clone();
        }
        let blend = key.blend.map(|(s, d)| {
            let c = wgpu::BlendComponent {
                src_factor: blend_factor(s),
                dst_factor: blend_factor(d),
                operation: wgpu::BlendOperation::Add,
            };
            wgpu::BlendState { color: c, alpha: c }
        });
        let p = crate::determinism::render_pipeline(
            &ctx.device,
            &wgpu::RenderPipelineDescriptor {
                label: Some("model"),
                layout: Some(&self.layout),
                vertex: wgpu::VertexState {
                    module: &self.module,
                    entry_point: Some("vs_model"),
                    compilation_options: Default::default(),
                    buffers: &[ModelVertex::layout()],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.module,
                    entry_point: Some("fs_model"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.colour_format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                // SPEC: FMT_RENDERSTYLE_2009 §4 (pass +0x1e8: CULLMODE NONE / CCW / CW); Direct3D's front faces are
                // clockwise on screen and the scene camera keeps Direct3D's handedness (RENDER_2009.md §4.1)
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: match key.cull {
                        Cull::None => None,
                        Cull::Ccw => Some(wgpu::Face::Back),
                        Cull::Cw => Some(wgpu::Face::Front),
                    },
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                // SPEC: FMT_RENDERSTYLE_2009 §4 (pass +0x1e4 ZENABLE / ZWRITEENABLE, +0x1f8 ZFUNC); RENDER_2009.md
                // §4.3 (normal objects in [0.1, 1.0])
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: self.zbuf_format,
                    depth_write_enabled: key.depth_write,
                    depth_compare: if key.depth_test {
                        compare_fn(key.depth_compare)
                    } else {
                        wgpu::CompareFunction::Always
                    },
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                // CLI-1593: the sample count of the frame's 3D target (1: the default state, MSAA off)
                multisample: crate::msaa::multisample_state(self.msaa),
                multiview: None,
                cache: None,
            },
        );
        map.insert(key, p.clone());
        p
    }

    fn sampler(&self, ctx: &GpuContext, key: SampKey) -> wgpu::Sampler {
        let mut map = self.samplers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = map.get(&key) {
            return s.clone();
        }
        let f = if key.filter.linear {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        };
        let (mip, max_lod) = match key.filter.mip {
            None => (wgpu::FilterMode::Nearest, 0.0),
            Some(false) => (wgpu::FilterMode::Nearest, 32.0),
            Some(true) => (wgpu::FilterMode::Linear, 32.0),
        };
        let aniso = if key.filter.anisotropic && key.filter.linear && key.filter.mip == Some(true) {
            self.anisotropy.clamp(1, 16)
        } else {
            1
        };
        let s = crate::determinism::sampler(
            &ctx.device,
            &wgpu::SamplerDescriptor {
                label: Some("model stage"),
                address_mode_u: address_mode(key.address[0]),
                address_mode_v: address_mode(key.address[1]),
                address_mode_w: wgpu::AddressMode::Repeat,
                mag_filter: f,
                min_filter: f,
                mipmap_filter: mip,
                lod_max_clamp: max_lod,
                anisotropy_clamp: aniso,
                ..Default::default()
            },
        );
        map.insert(key, s.clone());
        s
    }

    /// Uploads a model with its texture list and the plain style (see [`ModelPipelines::upload_look`]).
    pub fn upload(
        &self,
        ctx: &GpuContext,
        files: &ModelFiles,
        textures: &[Option<TextureImage>],
    ) -> ModelInstance {
        self.upload_look(
            ctx,
            files,
            ModelLook {
                textures,
                styles: StyleChoice::Plain,
            },
        )
    }

    /// The bind group and uniform of one style pass of one piece LOD.
    #[allow(clippy::too_many_arguments)]
    fn pass_group(
        &self,
        ctx: &GpuContext,
        d: &PieceDraw,
        style: &RenderStyle,
        p: &Pass,
        kept: &mut [Option<u32>; 4],
        textures: &[Option<TextureImage>],
        gpu_textures: &mut BTreeMap<u32, GpuTexture>,
        missing: &mut BTreeSet<u32>,
        stats: &mut StyleStats,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let n = p.active_stages();
        let mut su = [StageUniform::zeroed(); 4];
        let mut views: [Option<u32>; 4] = [None; 4];
        let mut samplers = Vec::with_capacity(4);
        let mut first_cutoff: Option<u8> = None;
        for (k, st) in p.stages.iter().enumerate() {
            // EVIDENCE: engine+0x1b9390 (the piece's texture k = the object's texture at the piece's slot k for
            // k < slot count, else null), engine+0x1a8240 (stage texture source)
            let tex = match st.texture {
                StageTexture::None => None,
                StageTexture::Keep => kept[k],
                StageTexture::Piece(t) if t < d.slot_count.max(1) => Some(d.slots[t as usize]),
                StageTexture::Piece(_) => None,
            };
            kept[k] = tex;
            let mut bias = self.mip_bias;
            if let Some(slot) = tex {
                let e = gpu_textures.entry(slot).or_insert_with(|| {
                    let img = match textures.get(slot as usize).and_then(|t| t.clone()) {
                        Some(i) => i,
                        None => {
                            // a slot past the object's texture list, or an image that did not load: the engine
                            // binds no texture to the stage and the sampler reads (0, 0, 0, 1), as the world
                            // sections do (RS\EdgeAmp01's stage 1 reads slot 1 of the spy body, which has none:
                            // a magenta checker there was the super spy's purple body, 2026-10-31)
                            // DC-DEFAULT: CLI-1592 (proposed) a model stage whose texture slot has no image samples
                            // the unbound black, not the debug checker
                            missing.insert(slot);
                            TextureImage::unbound()
                        }
                    };
                    let t = crate::gpu::upload_image(ctx, &img, "model texture");
                    let v = t.create_view(&Default::default());
                    let info = TextureImageInfo {
                        alpha_cutoff: img.alpha_cutoff,
                        mip_bias: img.mip_bias,
                    };
                    (t, v, info)
                });
                // SPEC: RENDER_2009.md §10.3 (LOD bias = mip_bias + the texture's own bias)
                bias += e.2.mip_bias;
                if k == 0 {
                    first_cutoff = e.2.alpha_cutoff;
                }
            }
            views[k] = tex;
            let (flags, xform) = match st.transform {
                Some((m, count, projected)) => {
                    (1 | ((projected as u32) << 1) | (count.min(15) << 4), m)
                }
                None => (0, Mat4::IDENTITY.to_cols_array()),
            };
            if k < n {
                if matches!(st.uv, UvSource::Set(s) if s > 0) {
                    stats.missing_uv_sets += 1;
                }
                if matches!(
                    st.colour_op,
                    StageOp::BumpEnvMap | StageOp::BumpEnvMapLuminance
                ) {
                    stats.bump_stages += 1;
                }
                if !matches!(st.uv, UvSource::Set(_)) {
                    stats.generated_uv_stages += 1;
                }
            }
            su[k] = StageUniform {
                ops: [
                    st.colour_op.code(),
                    st.colour_args[0].code(),
                    st.colour_args[1].code(),
                    st.alpha_op.code(),
                ],
                ops2: [
                    st.alpha_args[0].code(),
                    st.alpha_args[1].code(),
                    // DC-DEFAULT: REN-2 (proposed), see DC_DEFAULTS.md (the ROM vertices carry one uv set; sets
                    // 1..3 read set 0)
                    match st.uv {
                        UvSource::Set(_) => 0,
                        other => other.code() & 7,
                    },
                    flags,
                ],
                xform,
                extra: [bias, 0.0, 0.0, 0.0],
            };
            samplers.push(self.sampler(
                ctx,
                SampKey {
                    address: st.address,
                    filter: st.filter,
                },
            ));
        }
        // EVIDENCE: engine+0x1a88f0 (ALPHATESTENABLE / ALPHAFUNC from pass +0x1fc), engine+0x1a8240 (ALPHAREF from
        // pass +0x1f0, set after the textures are bound)
        let (alpha_code, alpha_reference) = if p.alpha_from_texture {
            match first_cutoff {
                Some(c) => (Compare::GreaterEqual.code(), c as f32),
                None => (0, 0.0),
            }
        } else {
            match p.alpha_test {
                Some(c) => (c.code(), p.alpha_reference.min(255) as f32),
                None => (0, 0.0),
            }
        };
        // VAR-4: the first pass of a piece carries the effect of its first texture, once
        let fx = views
            .iter()
            .flatten()
            .next()
            .filter(|_| std::ptr::eq(p, &style.passes[0]))
            .and_then(|slot| textures.get(*slot as usize)?.as_ref()?.fx.as_ref());
        let fx_tile_view;
        let fx_mask_view;
        let (fx_tile, fx_mask) = match fx {
            Some(f) => {
                let view = |img: &TextureImage, label: &str| {
                    crate::gpu::upload_image(ctx, img, label).create_view(&Default::default())
                };
                fx_tile_view = view(&f.tile, "camo effect tile");
                fx_mask_view = view(&f.mask, "camo effect mask");
                (&fx_tile_view, &fx_mask_view)
            }
            None => (&self.black, &self.black),
        };
        let m = &style.material;
        // SPEC: FMT_RENDERSTYLE_2009 §3 (a black specular colour forces power 0; SPECULARENABLE = power > 0)
        let power = if m.specular[..3].iter().all(|&c| c == 0.0) {
            0.0
        } else {
            m.power.max(0.0)
        };
        let pu = PassUniform {
            stages: su,
            factor: argb(p.texture_factor),
            alpha: [
                alpha_reference,
                alpha_code as f32,
                n as f32,
                p.lighting as u32 as f32,
            ],
            mat_ambient: m.ambient,
            mat_diffuse: m.diffuse,
            mat_emissive: m.emissive,
            mat_specular: [m.specular[0], m.specular[1], m.specular[2], power],
            fx_a: fx.map_or([0.0; 4], |f| [1.0, f.tiles, f.speed[0], f.speed[1]]),
            fx_b: fx.map_or([0.0; 4], |f| [f.gain[0], f.gain[1], f.seconds, 0.0]),
        };
        let device = &ctx.device;
        let ub = crate::perf::buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("model pass"),
                contents: bytemuck::bytes_of(&pu),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        let view_of = |k: usize| match views[k] {
            Some(slot) => &gpu_textures[&slot].1,
            None => &self.black,
        };
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: ub.as_entire_binding(),
        }];
        for k in 0..4 {
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + k as u32,
                resource: wgpu::BindingResource::TextureView(view_of(k)),
            });
        }
        for (k, s) in samplers.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 5 + k as u32,
                resource: wgpu::BindingResource::Sampler(s),
            });
        }
        for (binding, view) in [(9, fx_tile), (10, fx_mask)] {
            entries.push(wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 11,
            resource: wgpu::BindingResource::Sampler(&self.fx_sampler),
        });
        let g = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model pass"),
            layout: &self.pass_layout,
            entries: &entries,
        });
        (ub, g)
    }

    /// Uploads a model: every LOD of every piece, each drawn with the render style of its style slot (one
    /// draw per style pass) and the textures its stages name.
    // SPEC: FMT_RENDERSTYLE_2009 §4 (stage +0x00 texture source: 0 none, 1..4 the piece's textures 0..3);
    // EVIDENCE: engine+0x1b9390 (style by slot, default when missing), engine+0x1b94e0 (per pass: the pass setup
    // engine+0x1a88f0, then the texture binding engine+0x1a8240, then the draw)
    pub fn upload_look(
        &self,
        ctx: &GpuContext,
        files: &ModelFiles,
        look: ModelLook<'_>,
    ) -> ModelInstance {
        let _pack = crate::perf::span(crate::perf::Counter::PackNs);
        let device = &ctx.device;
        let mesh = ModelMesh::build(&files.body);
        let anim = files.anim_set();
        let placeholder = [ModelVertex::zeroed()];
        let vertex_buffer = crate::perf::buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("model vertices"),
                contents: bytemuck::cast_slice(if mesh.vertices.is_empty() {
                    &placeholder[..]
                } else {
                    &mesh.vertices
                }),
                usage: wgpu::BufferUsages::VERTEX,
            },
        );
        let index_buffer = crate::perf::buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("model indices"),
                contents: bytemuck::cast_slice(if mesh.indices.is_empty() {
                    &[0u32, 0, 0][..]
                } else {
                    &mesh.indices
                }),
                usage: wgpu::BufferUsages::INDEX,
            },
        );
        let instance_buffer = crate::perf::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("model instance"),
                size: std::mem::size_of::<InstanceUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        let identity = vec![Mat4::IDENTITY.to_cols_array(); MAX_BONES];
        let bones_buffer = crate::perf::buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("model bones"),
                contents: bytemuck::cast_slice(&identity),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let instance_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model instance"),
            layout: &self.instance_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: bones_buffer.as_entire_binding(),
                },
            ],
        });
        let mut gpu_textures: BTreeMap<u32, GpuTexture> = BTreeMap::new();
        let mut missing: BTreeSet<u32> = BTreeSet::new();
        let mut stats = StyleStats::default();
        let plain = Arc::new(RenderStyle::plain());
        let default = Arc::new(RenderStyle::engine_default());
        let mut draws = Vec::with_capacity(mesh.draws.len());
        let mut pipelines: Vec<wgpu::RenderPipeline> = Vec::new();
        let mut pipe_keys: Vec<PipeKey> = Vec::new();
        let mut groups: Vec<wgpu::BindGroup> = Vec::new();
        let mut buffers: Vec<wgpu::Buffer> = Vec::new();
        for d in &mesh.draws {
            let style = match look.styles {
                StyleChoice::Plain => plain.clone(),
                StyleChoice::List(list) => match list.get(d.style as usize).and_then(|s| s.clone())
                {
                    Some(s) => s,
                    None => {
                        stats.default_pieces += (d.lod == 0) as u64;
                        default.clone()
                    }
                },
            };
            let mut passes = Vec::with_capacity(style.passes.len());
            // the binding a stage keeps when a later pass names no texture for it (engine values above 4)
            let mut kept: [Option<u32>; 4] = [None; 4];
            for p in &style.passes {
                if d.lod == 0 {
                    stats.wireframe_passes += p.wireframe as u64;
                    stats.bump_passes += p.bump as u64;
                }
                let key = PipeKey::of(p);
                let pi = match pipe_keys.iter().position(|k| *k == key) {
                    Some(i) => i,
                    None => {
                        pipe_keys.push(key);
                        pipelines.push(self.pipeline(ctx, key));
                        pipelines.len() - 1
                    }
                };
                let (ub, g) = self.pass_group(
                    ctx,
                    d,
                    &style,
                    p,
                    &mut kept,
                    look.textures,
                    &mut gpu_textures,
                    &mut missing,
                    &mut stats,
                );
                buffers.push(ub);
                groups.push(g);
                passes.push((pi, groups.len() - 1));
            }
            if d.lod == 0 {
                stats.pieces += 1;
                stats.effect_pieces += style.effect as u64;
                stats.passes += passes.len() as u64;
                *stats
                    .stage0_ops
                    .entry(format!("{:?}", style.passes[0].stages[0].colour_op))
                    .or_default() += 1;
            }
            draws.push(ModelDraw {
                draw: *d,
                blends: style.blends(),
                passes,
            });
        }
        // DC-DEFAULT: REN-6 (proposed), see DC_DEFAULTS.md (pieces in file order, stable-sorted by priority,
        // blending pieces after the others; the engine sorts its piece list at engine+0x1b92a0, comparator not
        // traced)
        draws.sort_by_key(|d| (d.draw.priority, d.blends));
        stats.missing_textures = missing.len() as u64;
        // CPU copies of the LOD 0 vertices for the posed bounds
        let mut lod0 = vec![false; mesh.vertices.len()];
        for d in mesh.draws.iter().filter(|d| d.lod == 0) {
            let r = d.first_index as usize..(d.first_index + d.index_count) as usize;
            for &i in &mesh.indices[r] {
                if let Some(f) = lod0.get_mut(i as usize) {
                    *f = true;
                }
            }
        }
        let cpu_vertices: Vec<_> = mesh
            .vertices
            .iter()
            .zip(&lod0)
            .filter(|(_, l)| **l)
            .map(|(v, _)| (v.pos, v.bones, v.weights))
            .collect();
        // perf-g1: the CPU copy of the LOD0 vertices lives as long as the template (no eviction): a pool gauge
        crate::perf::gauge(
            crate::perf::Gauge::ModelCpuBytes,
            std::mem::size_of_val(&cpu_vertices[..]) as i64,
        );
        let mut inst = ModelInstance {
            vertex_buffer,
            index_buffer,
            draws,
            lod_dists: mesh.lod_dists.clone(),
            pipelines,
            groups,
            _buffers: buffers,
            _textures: gpu_textures.into_values().map(|t| t.0).collect(),
            has_fx: look.textures.iter().flatten().any(|t| t.fx.is_some()),
            instance_buffer,
            bones_buffer,
            instance_group,
            radius: files.body.radius.max(1.0),
            bounds: mesh.bounds,
            cpu_vertices: Arc::new(cpu_vertices),
            mesh_stats: mesh.stats,
            lod_triangles: mesh.lod_triangles.clone(),
            missing_textures: missing.len() as u64,
            style_stats: stats,
            lod_offset: 0,
            lod_distance: None,
            bones: vec![Mat4::IDENTITY; anim.skeleton.len().max(1)],
            globals: anim.skeleton.bind_global.clone(),
            anim,
            trackers: Vec::new(),
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            lights: LightSet {
                ambient: Vec3::splat(255.0),
                lights: Vec::new(),
            },
            uploaded: Mutex::new(UniformCache::default()),
            follow_map: None,
            piece_names: Arc::new(
                files
                    .body
                    .pieces
                    .iter()
                    .map(|p| p.name.as_bytes().to_vec())
                    .collect(),
            ),
            hidden_pieces: Vec::new(),
        };
        inst.set_pose(&[]);
        inst
    }
}

#[derive(Debug, Clone, Copy)]
struct TextureImageInfo {
    alpha_cutoff: Option<u8>,
    mip_bias: f32,
}

/// Counters of one upload (for reports).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleStats {
    /// LOD 0 pieces and their passes.
    pub pieces: u64,
    pub passes: u64,
    /// LOD 0 pieces drawn with the engine default because their style slot is empty.
    pub default_pieces: u64,
    /// LOD 0 pieces whose style selects the effect path (drawn with the passes).
    pub effect_pieces: u64,
    /// Stage-0 colour op per LOD 0 piece.
    pub stage0_ops: BTreeMap<String, u64>,
    /// LOD 0 passes with the wireframe fill value (drawn solid) and with the bump block on.
    pub wireframe_passes: u64,
    pub bump_passes: u64,
    /// Active stages with bump ops (passed through), reading uv sets 1..3 (read as set 0), with generated
    /// coordinates.
    pub bump_stages: u64,
    pub missing_uv_sets: u64,
    pub generated_uv_stages: u64,
    /// Texture-list slots that had no image (bound as the unbound black, CLI-1592).
    pub missing_textures: u64,
}

/// One piece LOD with its passes: (pipeline index, bind group index) per style pass.
#[derive(Debug, Clone)]
struct ModelDraw {
    draw: PieceDraw,
    blends: bool,
    passes: Vec<(usize, usize)>,
}

// One uploaded model: buffers, textures, the pose and where it stands.
pub struct ModelInstance {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    draws: Vec<ModelDraw>,
    /// Switch distances per piece.
    lod_dists: Vec<Vec<f32>>,
    pipelines: Vec<wgpu::RenderPipeline>,
    groups: Vec<wgpu::BindGroup>,
    _buffers: Vec<wgpu::Buffer>,
    _textures: Vec<wgpu::Texture>,
    /// A texture of this model carries a camo effect (VAR-4).
    has_fx: bool,
    instance_buffer: wgpu::Buffer,
    bones_buffer: wgpu::Buffer,
    instance_group: wgpu::BindGroup,
    /// Bounding radius from the model file (FMT_LTB.md §4 step 4), for the light selection.
    pub radius: f32,
    /// Bind-pose bounds of the drawn vertices (model space).
    pub bounds: Option<([f32; 3], [f32; 3])>,
    /// Bind-pose positions with their bones and weights (for [`ModelInstance::posed_bounds`]).
    /// Shared by the instances of one upload ([`ModelPipelines::instantiate`]).
    cpu_vertices: CpuVertices,
    pub mesh_stats: ModelMeshStats,
    /// Triangles per LOD level over all pieces.
    pub lod_triangles: Vec<u64>,
    pub missing_textures: u64,
    /// What the render styles did at upload.
    pub style_stats: StyleStats,
    /// `model_lod_offset` (default 0) added to every piece's LOD.
    pub lod_offset: i32,
    /// Distance used for the LOD choice instead of the eye distance (`Some(0.0)`: always LOD 0).
    pub lod_distance: Option<f32>,
    /// Skeleton and the combined clip list.
    pub anim: AnimSet,
    /// The trackers of the current pose.
    pub trackers: Vec<Tracker>,
    /// Bone matrices of the current pose.
    pub bones: Vec<Mat4>,
    /// Model-space node transforms of the current pose.
    pub globals: Vec<Mat4>,
    pub position: Vec3,
    pub rotation: Quat,
    /// Per-axis scale of the model (1 for every model but a mod model with an `AttachScale` row).
    // EVIDENCE: cshell+0x237b68..0x237b91 (an attached mod model is scaled by its row's three floats, engine
    // interface slot +0x224)
    pub scale: Vec3,
    pub lights: LightSet,
    /// What the instance and bone buffers hold (render-core: unchanged uniforms are not written again).
    uploaded: Mutex<UniformCache>,
    /// The node map of the last leader [`ModelInstance::follow`] took (its node names and our map onto it).
    follow_map: Option<FollowMap>,
    /// The body model's piece names in file order (shared by the instances of one upload).
    piece_names: Arc<Vec<Vec<u8>>>,
    /// Pieces this instance does not draw ([`ModelInstance::hide_pieces`]); empty draws every piece.
    hidden_pieces: Vec<usize>,
}

/// Bind-pose LOD 0 positions with their bones and weights, shared by the instances of one upload.
type CpuVertices = Arc<Vec<([f32; 3], [u8; 4], [f32; 4])>>;

/// A leader's node names and our node map onto them (see [`ModelInstance::follow`]).
type FollowMap = (Vec<Vec<u8>>, Vec<Option<usize>>);

/// The bytes last written to an instance's uniform buffers. `bones` holds the palette entries written so far (the
/// rest of the 256 stay the identity the buffer was created with); `scratch` is the next palette, kept to reuse its
/// allocation.
#[derive(Default)]
struct UniformCache {
    instance: Option<InstanceUniform>,
    bones: Vec<[f32; 16]>,
    scratch: Vec<[f32; 16]>,
}

impl fmt::Debug for ModelInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelInstance")
            .field("draws", &self.draws.len())
            .field("mesh", &self.mesh_stats)
            .field("position", &self.position)
            .finish()
    }
}

impl ModelInstance {
    /// Sets the pose from a tracker list (empty: the bind pose).
    pub fn set_pose(&mut self, trackers: &[Tracker]) {
        self.trackers.clear();
        self.trackers.extend_from_slice(trackers);
        let pose = self.anim.pose(trackers);
        self.anim.skeleton.globals_into(&pose, &mut self.globals);
        self.update_bones();
    }

    /// `bones = globals x inverse bind`, in the reused vector.
    fn update_bones(&mut self) {
        self.bones.clear();
        self.bones.extend(
            self.globals
                .iter()
                .zip(&self.anim.skeleton.inverse_bind)
                .map(|(g, ib)| *g * *ib),
        );
    }

    /// Takes the pose, transform and lights of `leader` for a model drawn in the same object (a child model of
    /// the body; DeCombat skins the character's face with [`ModelInstance::follow_skinned`], CLI-1485): each
    /// node follows the leader's node of the same name (else the node at the
    /// same position when both trees have the same size); unmatched nodes keep the bind pose.
    ///
    /// DeCombat reading: the 2009 character object carries the body and the face as two entries of one model
    /// block (UDP_REFERENCE_FINDINGS_botclient.md reference run, REFERENCE) and the face models share the body's node
    /// names (OBSERVED: the default face row's model has the 53 node names of the default body, 52 with the
    /// same bind transform; `character_census --faces`).
    pub fn follow(&mut self, leader: &ModelInstance) {
        self.follow_in(leader, false);
    }

    /// CLI-1346: as [`ModelInstance::follow`], but a matched node is skinned with the leader's own skin matrix (its
    /// pose times the leader's inverse bind): the piece's vertices are read in the leader's model space, whatever
    /// bind its own file gives the node. A head-gear / hair piece whose file binds the head node lower than the body
    /// does (the female figure's hair row: 2.3 units) then sits on the body's head instead of that far above it.
    // OBSERVED: inbox-mei/ (the reference client's shop figure of the female character wears its hair
    // row on the head, the fringe over the forehead; with the piece's own bind the Rust figure's hair stood above
    // the scalp)
    // DC-DEFAULT: CLI-1346 hair / head-gear pieces skin in the body's bind space
    pub fn follow_skinned(&mut self, leader: &ModelInstance) {
        self.follow_in(leader, true);
    }

    /// [`ModelInstance::follow`] (`leader_space` false) or [`ModelInstance::follow_skinned`] (true); the node
    /// transforms are [`crate::Skeleton::follow_globals`].
    pub fn follow_in(&mut self, leader: &ModelInstance, leader_space: bool) {
        // the name map depends only on the two node lists: kept while the leader's names are the same
        let names = &leader.anim.skeleton.names;
        if self.follow_map.as_ref().is_none_or(|(n, _)| n != names) {
            let map = self.anim.skeleton.map_onto(&leader.anim.skeleton);
            self.follow_map = Some((names.clone(), map));
        }
        let map = self.follow_map.as_ref().map(|f| &f.1[..]).unwrap_or(&[]);
        self.anim.skeleton.follow_globals(
            &leader.anim.skeleton,
            map,
            &leader.globals,
            leader_space,
            &mut self.globals,
        );
        self.update_bones();
        self.trackers.clear();
        self.trackers.extend_from_slice(&leader.trackers);
        self.position = leader.position;
        self.rotation = leader.rotation;
        self.lights.clone_from(&leader.lights);
    }

    /// World transform: rotation then translation.
    pub fn world(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }

    /// Hides the pieces whose name equals one of `names` (ASCII case ignored) and draws every other piece again;
    /// returns how many pieces are hidden. An empty list draws every piece.
    // DISASM: cshell+0x282490 (a pickup hides one piece of its model by name; the engine's piece visibility call)
    pub fn hide_pieces(&mut self, names: &[&str]) -> usize {
        self.hidden_pieces.clear();
        for (i, n) in self.piece_names.iter().enumerate() {
            if names.iter().any(|w| w.as_bytes().eq_ignore_ascii_case(n)) {
                self.hidden_pieces.push(i);
            }
        }
        self.hidden_pieces.len()
    }

    /// The pieces [`ModelInstance::hide_pieces`] hid (indices in file order).
    pub fn hidden_pieces(&self) -> &[usize] {
        &self.hidden_pieces
    }

    /// The body model's piece names in file order.
    pub fn piece_names(&self) -> &[Vec<u8>] {
        &self.piece_names
    }

    /// Number of piece draws at LOD 0 (one per drawn piece).
    pub fn draw_count(&self) -> usize {
        self.draws.iter().filter(|d| d.draw.lod == 0).count()
    }

    /// Number of passes the LOD 0 pieces draw.
    pub fn pass_count(&self) -> usize {
        self.draws
            .iter()
            .filter(|d| d.draw.lod == 0)
            .map(|d| d.passes.len())
            .sum()
    }

    /// The LOD each piece is drawn with when the eye is at `eye`, as (piece, LOD) for every piece that has a
    /// built LOD.
    // EVIDENCE: engine+0x1b1ff0 (distance from the camera position to the object position)
    pub fn select_lods(&self, eye: Vec3) -> Vec<(usize, usize)> {
        let dist = self
            .lod_distance
            .unwrap_or_else(|| eye.distance(self.position));
        let mut out = Vec::new();
        for (pi, d) in self.lod_dists.iter().enumerate() {
            if self.draws.iter().any(|x| x.draw.piece == pi) {
                out.push((pi, select_lod(d, dist, self.lod_offset)));
            }
        }
        out
    }

    /// Triangles drawn with the eye at `eye`.
    pub fn triangles_at(&self, eye: Vec3) -> u64 {
        let lods = self.select_lods(eye);
        self.draws
            .iter()
            .filter(|d| lods.contains(&(d.draw.piece, d.draw.lod as usize)))
            .map(|d| (d.draw.index_count / 3) as u64)
            .sum()
    }

    /// The model-space transform of attach point `socket` of `model` (the body model file this instance was
    /// uploaded from) in the current pose: node transform times the socket's scale, rotation and position.
    // SPEC: FMT_LTB.md §11.1 (socket = node, rotation, position, scale); DeCombat reading of the composition
    // (node pose x socket offset): the engine's attachment update was not traced (PLAUSIBLE)
    pub fn socket_transform(&self, model: &dc_model::Model, socket: usize) -> Option<Mat4> {
        let s = model.sockets.get(socket)?;
        let node = *self.globals.get(s.node as usize)?;
        let q = Quat::from_xyzw(s.rot[0], s.rot[1], s.rot[2], s.rot[3]);
        let q = if q.length_squared() > 1e-12 {
            q.normalize()
        } else {
            Quat::IDENTITY
        };
        let local = Mat4::from_scale_rotation_translation(
            Vec3::from(s.scale).max(Vec3::splat(1e-6)),
            q,
            Vec3::from(s.pos),
        );
        Some(node * local)
    }

    /// World transform of an attach point (see [`ModelInstance::socket_transform`]).
    pub fn socket_world(&self, model: &dc_model::Model, socket: usize) -> Option<Mat4> {
        Some(self.world() * self.socket_transform(model, socket)?)
    }

    /// Places `child` (an attached object such as the held weapon) at attach point `socket` of this model
    /// with the attachment's offset and rotation (zero / identity for the weapon of a player), and gives it this
    /// model's light set. `false` when the socket or its node does not exist.
    // SPEC: UDP_SESSION_2009_B.md §3.5 (weapon child at socket 2, offset 0, rotation identity), §4.2 item 8
    // (attachment = child, socket, offset, rotation)
    pub fn attach(
        &self,
        model: &dc_model::Model,
        socket: usize,
        offset: Vec3,
        rotation: Quat,
        child: &mut ModelInstance,
    ) -> bool {
        let Some(m) = self.socket_world(model, socket) else {
            return false;
        };
        let m = m * Mat4::from_rotation_translation(rotation, offset);
        let (_, r, t) = m.to_scale_rotation_translation();
        child.position = t;
        child.rotation = r.normalize();
        child.lights = self.lights.clone();
        true
    }

    /// Writes the instance and bone uniforms for `camera` (call before recording the frame).
    /// The world pass's linear fog applies (RENDER_2009.md §3.3: the fog state is device state, so the model passes
    /// of the world pass are fogged like the world). Returns the bytes written.
    pub(crate) fn write_fogged(&self, ctx: &GpuContext, camera: &Camera, fog: Option<&Fog>) -> u64 {
        let world = self.world();
        self.write_matrices(
            ctx,
            camera,
            model_view_projection(camera) * world,
            world,
            fog,
        )
    }

    /// Writes the uniforms of an eye-attached instance: its own transform (`position` / `rotation`) is in eye
    /// space and is drawn with `projection` and VIEW = identity; lighting runs in the world at the eye, so the
    /// light set (world positions) and the world camera's view still apply to the world transform
    /// `camera.world_from_eye() * own`.
    // SPEC: RENDER_2009.md §4.2 (VIEW = identity, eye-space placement); §7.2 (WORLD = model transform per model,
    // LIGHTING on for the model pass); DC-DEFAULT: REN-13 (proposed), see DC_DEFAULTS.md (lights evaluated in world
    // space with the eye-space object carried into the world by the camera)
    pub(crate) fn write_view(&self, ctx: &GpuContext, camera: &Camera, projection: Mat4) -> u64 {
        let own = self.world();
        self.write_matrices(
            ctx,
            camera,
            projection * own,
            camera.world_from_eye() * own,
            None,
        )
    }

    fn write_matrices(
        &self,
        ctx: &GpuContext,
        camera: &Camera,
        clip: Mat4,
        world: Mat4,
        fog: Option<&Fog>,
    ) -> u64 {
        let (fog, fog_colour) = Fog::uniform(fog);
        let mut lights = [LightUniform::zeroed(); MAX_MODEL_LIGHTS];
        let n = self.lights.lights.len().min(MAX_MODEL_LIGHTS);
        for (i, l) in self.lights.lights.iter().take(n).enumerate() {
            lights[i] = light_uniform(l);
        }
        // SPEC: RENDER_2009.md §6.3 (AMBIENT render state = the light-grid colour packed as 0xff RGB)
        let a = (self.lights.ambient / 255.0).clamp(Vec3::ZERO, Vec3::ONE);
        let u = InstanceUniform {
            clip_from_model: clip.to_cols_array_2d(),
            world_from_model: world.to_cols_array_2d(),
            view: camera.view().to_cols_array_2d(),
            ambient: [a.x, a.y, a.z, n as f32],
            // VAR-4: only a model with an effect reads the clock; the others keep their uniforms unchanged between frames
            eye: [
                camera.pos.x,
                camera.pos.y,
                camera.pos.z,
                if self.has_fx { fx_clock() } else { 1.0 },
            ],
            lights,
            fog,
            fog_colour,
        };
        // Only what changed is written: the buffers keep their contents between frames, so the shader reads the
        // same values as with a full write every frame. The bone palette is written up to the skeleton's length
        // (or the length written before, padded with the identity), the rest stays the identity of its creation.
        let mut written = 0u64;
        let mut cache = self.uploaded.lock().unwrap_or_else(|e| e.into_inner());
        let c = &mut *cache;
        if c.instance
            .is_none_or(|old| bytemuck::bytes_of(&old) != bytemuck::bytes_of(&u))
        {
            crate::perf::write_buffer(&ctx.queue, &self.instance_buffer, 0, bytemuck::bytes_of(&u));
            c.instance = Some(u);
            written += std::mem::size_of_val(&u) as u64;
        }
        let n = self.bones.len().min(MAX_BONES);
        let m = n.max(c.bones.len());
        c.scratch.clear();
        c.scratch
            .extend(self.bones[..n].iter().map(|b| b.to_cols_array()));
        c.scratch.resize(m, Mat4::IDENTITY.to_cols_array());
        let same: bool =
            bytemuck::cast_slice::<_, u8>(&c.scratch) == bytemuck::cast_slice::<_, u8>(&c.bones);
        if !same && m > 0 {
            crate::perf::add(crate::perf::Counter::BonesUploaded, m as u64);
            crate::perf::write_buffer(
                &ctx.queue,
                &self.bones_buffer,
                0,
                bytemuck::cast_slice(&c.scratch),
            );
            written += std::mem::size_of_val(&c.scratch[..]) as u64;
            std::mem::swap(&mut c.bones, &mut c.scratch);
        }
        written
    }

    /// Records the draws of this model for the eye at `eye` (each piece at its LOD, each style pass with its
    /// own pipeline; viewport set by the caller).
    pub(crate) fn record<'p>(
        &'p self,
        pass: &mut wgpu::RenderPass<'p>,
        eye: Vec3,
        report: &mut FrameReport,
    ) {
        if self.draws.is_empty() {
            return;
        }
        // the LOD of each piece as `select_lods` picks it, computed per draw without a list
        let dist = self
            .lod_distance
            .unwrap_or_else(|| eye.distance(self.position));
        pass.set_bind_group(0, &self.instance_group, &[]);
        report.bind_sets += 1;
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        let mut current: Option<usize> = None;
        for d in &self.draws {
            let Some(ld) = self.lod_dists.get(d.draw.piece) else {
                continue;
            };
            if !self.hidden_pieces.is_empty() && self.hidden_pieces.contains(&d.draw.piece) {
                continue;
            }
            if select_lod(ld, dist, self.lod_offset) != d.draw.lod as usize {
                continue;
            }
            for &(pi, g) in &d.passes {
                if current != Some(pi) {
                    pass.set_pipeline(&self.pipelines[pi]);
                    current = Some(pi);
                    report.pipeline_sets += 1;
                }
                pass.set_bind_group(1, &self.groups[g], &[]);
                pass.draw_indexed(
                    d.draw.first_index..d.draw.first_index + d.draw.index_count,
                    0,
                    0..1,
                );
                report.bind_sets += 1;
                report.draw_calls += 1;
            }
        }
    }

    /// Records this model's geometry once per drawn piece (each at its LOD, hidden pieces left out) with the
    /// caller's pipeline: group 0 = this instance's uniforms, the caller binds the rest (the x-ray chams, CLI-1624).
    /// Returns the draw calls recorded.
    pub(crate) fn record_flat<'p>(&'p self, pass: &mut wgpu::RenderPass<'p>, eye: Vec3) -> u64 {
        if self.draws.is_empty() {
            return 0;
        }
        let dist = self
            .lod_distance
            .unwrap_or_else(|| eye.distance(self.position));
        pass.set_bind_group(0, &self.instance_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        let mut calls = 0;
        for d in &self.draws {
            let Some(ld) = self.lod_dists.get(d.draw.piece) else {
                continue;
            };
            if self.hidden_pieces.contains(&d.draw.piece)
                || select_lod(ld, dist, self.lod_offset) != d.draw.lod as usize
            {
                continue;
            }
            pass.draw_indexed(
                d.draw.first_index..d.draw.first_index + d.draw.index_count,
                0,
                0..1,
            );
            calls += 1;
        }
        calls
    }

    /// Model-space bounds of the current pose: the vertices skinned on the CPU with the same bone matrices and
    /// weights the shader uses.
    pub fn posed_bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut out: Option<(Vec3, Vec3)> = None;
        for (p, b, w) in self.cpu_vertices.iter() {
            let mut m = Mat4::ZERO;
            for k in 0..4 {
                if w[k] != 0.0 {
                    m += *self.bones.get(b[k] as usize).unwrap_or(&Mat4::IDENTITY) * w[k];
                }
            }
            let q = m.transform_point3(Vec3::from(*p));
            if !q.is_finite() {
                continue;
            }
            out = Some(match out {
                None => (q, q),
                Some((a, c)) => (a.min(q), c.max(q)),
            });
        }
        out
    }

    /// Model-space bounds of the current pose over the LOD 0 vertices whose largest weight is on `node`.
    pub fn node_bounds(&self, node: usize) -> Option<(Vec3, Vec3)> {
        let mut out: Option<(Vec3, Vec3)> = None;
        for (p, b, w) in self.cpu_vertices.iter() {
            let k = (0..4).max_by(|&i, &j| w[i].total_cmp(&w[j])).unwrap_or(0);
            if b[k] as usize != node {
                continue;
            }
            let mut m = Mat4::ZERO;
            for k in 0..4 {
                if w[k] != 0.0 {
                    m += *self.bones.get(b[k] as usize).unwrap_or(&Mat4::IDENTITY) * w[k];
                }
            }
            let q = m.transform_point3(Vec3::from(*p));
            if !q.is_finite() {
                continue;
            }
            out = Some(match out {
                None => (q, q),
                Some((a, c)) => (a.min(q), c.max(q)),
            });
        }
        out
    }

    /// World-space bounds of the current pose at the current transform (for placing a camera).
    pub fn bounds_in_world(&self) -> Option<(Vec3, Vec3)> {
        let (lo, hi) = self.posed_bounds()?;
        let w = self.world();
        let mut out: Option<(Vec3, Vec3)> = None;
        for k in 0..8 {
            let c = Vec3::new(
                if k & 1 == 0 { lo.x } else { hi.x },
                if k & 2 == 0 { lo.y } else { hi.y },
                if k & 4 == 0 { lo.z } else { hi.z },
            );
            let p = w.transform_point3(c);
            out = Some(match out {
                None => (p, p),
                Some((a, b)) => (a.min(p), b.max(p)),
            });
        }
        out
    }
}

/// Main camera matrices for the model pass: the same camera as the world (RENDER_2009.md §7.2: WORLD = model
/// transform per model, VIEW / PROJECTION of the scene).
pub(crate) fn model_view_projection(camera: &Camera) -> Mat4 {
    camera.view_projection()
}

impl ModelPipelines {
    /// The per-instance uniform buffers of a model (instance block, bone palette starting as the identity) and
    /// their bind group.
    fn instance_uniforms(&self, ctx: &GpuContext) -> (wgpu::Buffer, wgpu::Buffer, wgpu::BindGroup) {
        let device = &ctx.device;
        let instance_buffer = crate::perf::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("model instance"),
                size: std::mem::size_of::<InstanceUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        let identity = vec![Mat4::IDENTITY.to_cols_array(); MAX_BONES];
        let bones_buffer = crate::perf::buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("model bones"),
                contents: bytemuck::cast_slice(&identity),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let instance_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model instance"),
            layout: &self.instance_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: bones_buffer.as_entire_binding(),
                },
            ],
        });
        (instance_buffer, bones_buffer, instance_group)
    }

    /// A new instance of an uploaded model: it shares `template`'s vertex and index buffers,
    /// textures, pipelines and pass bind groups (all written once at upload and only read by the draws) and gets
    /// uniform buffers of its own. It starts in the state a fresh [`ModelPipelines::upload_look`] of the same model
    /// returns: bind pose, at the origin, unrotated, the neutral light set, no LOD override. So drawing it gives the
    /// pixels of a fresh upload (dc-render test `instances_render_like_fresh_uploads`) while a second character
    /// with the same look costs no ROM read, decode or texture upload.
    pub fn instantiate(&self, ctx: &GpuContext, template: &ModelInstance) -> ModelInstance {
        let (instance_buffer, bones_buffer, instance_group) = self.instance_uniforms(ctx);
        let anim = template.anim.clone();
        let mut inst = ModelInstance {
            vertex_buffer: template.vertex_buffer.clone(),
            index_buffer: template.index_buffer.clone(),
            draws: template.draws.clone(),
            lod_dists: template.lod_dists.clone(),
            pipelines: template.pipelines.clone(),
            groups: template.groups.clone(),
            _buffers: template._buffers.clone(),
            _textures: template._textures.clone(),
            has_fx: template.has_fx,
            instance_buffer,
            bones_buffer,
            instance_group,
            radius: template.radius,
            bounds: template.bounds,
            cpu_vertices: template.cpu_vertices.clone(),
            mesh_stats: template.mesh_stats,
            lod_triangles: template.lod_triangles.clone(),
            missing_textures: template.missing_textures,
            style_stats: template.style_stats.clone(),
            lod_offset: 0,
            lod_distance: None,
            bones: vec![Mat4::IDENTITY; anim.skeleton.len().max(1)],
            globals: anim.skeleton.bind_global.clone(),
            anim,
            trackers: Vec::new(),
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            lights: LightSet {
                ambient: Vec3::splat(255.0),
                lights: Vec::new(),
            },
            uploaded: Mutex::new(UniformCache::default()),
            follow_map: None,
            piece_names: template.piece_names.clone(),
            hidden_pieces: Vec::new(),
        };
        inst.set_pose(&[]);
        inst
    }
}

/// Model uploads away from the frame thread: a handle on the device and queue and on a
/// renderer's model pipelines, usable on any thread (get one from [`crate::WorldRenderer::model_uploader`]).
///
/// wgpu resource creation and queue writes are thread-safe; a model uploaded here may be drawn by the renderer's
/// thread once it has been handed over (its buffer and texture writes are queued before any submit that can use it).
/// Pipelines a new render style needs are created (and cached for every later upload) on the calling thread.
#[derive(Clone)]
pub struct ModelUploader {
    ctx: GpuContext,
    models: Arc<ModelPipelines>,
}

impl fmt::Debug for ModelUploader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelUploader").finish_non_exhaustive()
    }
}

impl ModelUploader {
    pub(crate) fn new(ctx: GpuContext, models: Arc<ModelPipelines>) -> ModelUploader {
        ModelUploader { ctx, models }
    }

    /// [`ModelPipelines::upload_look`] with the uploader's device.
    pub fn upload_look(&self, files: &ModelFiles, look: ModelLook<'_>) -> ModelInstance {
        self.models.upload_look(&self.ctx, files, look)
    }

    /// [`ModelPipelines::instantiate`] with the uploader's device.
    pub fn instantiate(&self, template: &ModelInstance) -> ModelInstance {
        self.models.instantiate(&self.ctx, template)
    }

    /// The device context the uploads go to.
    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }
}
