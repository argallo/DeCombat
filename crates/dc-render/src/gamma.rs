//! The 2009 gamma ramp: the whole finished frame (world, models, HUD) goes through a 256-entry ramp per channel,
//! `ramp[i] = trunc(65535 * (i / 255) ^ (1 / gamma))`, which the engine hands to the device (or the desktop).
//! wgpu has no gamma ramp, so DeCombat draws the frame into an intermediate target and copies it to the screen
//! through the same ramp ([`GammaStage`]); offscreen pictures apply it on the CPU ([`apply_ramp`]).
//!
//! The gamma value comes from the user's profile: the game reads the display section's gamma entry (default 1.22)
//! at start and sets the three engine gamma values to it.

use crate::gpu::GpuContext;
use std::path::Path;

/// The game's gamma when the profile has no gamma entry.
// EVIDENCE: cshell+0x1ea350 (display section of the profile: the gamma entry read with the float default 0x3f9c28f6
// = 1.22 into settings +0x140), cshell+0x1e9640 (the three engine gamma values set to settings +0x140)
pub const PROFILE_GAMMA_DEFAULT: f32 = 1.22;

/// The engine's ramp for one channel.
// SPEC: RENDER_2009.md §2.5; EVIDENCE: engine+0xf6f10 (per entry i of 256: (float)i * (1/255), pow with 1 / gamma,
// times 65535, truncated to 16 bits)
pub fn ramp_for_gamma(gamma: f32) -> [u16; 256] {
    let mut r = [0u16; 256];
    let g = if gamma.is_finite() && gamma > 1e-3 {
        gamma
    } else {
        1.0
    };
    let inv = 1.0 / g as f64;
    for (i, v) in r.iter_mut().enumerate() {
        let x = i as f64 * (1.0 / 255.0);
        *v = (x.powf(inv) * 65535.0).clamp(0.0, 65535.0) as u16;
    }
    r
}

/// The 8-bit value an entry of the ramp puts on screen (the high byte of the 16-bit entry, as an 8-bit output
/// path takes it).
fn ramp8(ramp: &[u16; 256]) -> [u8; 256] {
    ramp.map(|v| (v >> 8) as u8)
}

/// Applies `ramp` to RGBA8 pixels in place (alpha untouched).
pub fn apply_ramp(rgba: &mut [u8], ramp: &[u16; 256]) {
    let t = ramp8(ramp);
    if t.iter().enumerate().all(|(i, &v)| v as usize == i) {
        return;
    }
    for p in rgba.as_chunks_mut::<4>().0 {
        p[0] = t[p[0] as usize];
        p[1] = t[p[1] as usize];
        p[2] = t[p[2] as usize];
    }
}

/// Where the 2009 client keeps the player's profile (relative to the client folder).
// OBSERVED: CA_ROM `Profiles/` holds the player's profile (written by the 2009 client's options page, read at start)
// and three detail presets with the same display section
const PROFILE_FILE: &str = "Profiles/player.txt";
/// The profile key string in the user's CShell.dll: the constructor of the profile object stores its address.
// EVIDENCE: cshell+0x259930 (profile object constructor: key pointer = VA of rva 0x3ee6e4); only the SHA-256 of the
// string is kept here
const PROFILE_KEY_RVA: u32 = 0x3ee6e4;
const PROFILE_KEY_SHA256: &str = "050ec4c5d1f667e1c803635636f11e1f3f832ad40cd24dd6ff7915703bf92057";
/// Display section and gamma entry of the profile (`sha256(name)[:16]`, length).
// EVIDENCE: cshell+0x1ea350 (section and entry names read there), cshell+0x1e91d0 (written back at save)
const SECTION_DISPLAY: (&str, usize) = ("34e108c0896d0158", 7);
const ENTRY_SCREEN_GAMMA: (&str, usize) = ("54d299cd188556bb", 11);

fn is(name: &str, key: (&str, usize)) -> bool {
    name.len() == key.1 && dc_world::hash16_hex(name.as_bytes()) == key.0
}

/// The gamma entry of a decrypted profile text: the display section's gamma value (a trailing `f` allowed);
/// `None` when the section or entry is missing or not a positive number.
pub fn gamma_of_profile_text(text: &str) -> Option<f32> {
    entry_of_profile_text(text, SECTION_DISPLAY, ENTRY_SCREEN_GAMMA)
}

fn entry_of_profile_text(text: &str, section: (&str, usize), entry: (&str, usize)) -> Option<f32> {
    let mut in_display = false;
    for line in text.lines() {
        let l = line.trim();
        if let Some(sec) = l.strip_prefix('[') {
            in_display = sec.strip_suffix(']').is_some_and(|s| is(s.trim(), section));
            continue;
        }
        if !in_display {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else {
            continue;
        };
        if is(k.trim(), entry) {
            let v = v.trim().trim_matches('"');
            let v = v.strip_suffix(['f', 'F']).unwrap_or(v);
            return v.parse::<f32>().ok().filter(|g| g.is_finite() && *g > 0.0);
        }
    }
    None
}

/// The gamma of the user's profile in the client folder `rom` (decrypted with the client's own profile key, read
/// from its CShell.dll by RVA and accepted by its pinned SHA-256); `None` when anything is missing.
// SPEC: same framing as the archive attributes (Blowfish ECB over byte-swapped words, plain tail), dc-rez;
// OBSERVED: the four CA_ROM profiles decrypt this way to plain text with a display section
pub fn profile_gamma(rom: &Path) -> Option<f32> {
    let key = profile_key(rom)?;
    let raw = std::fs::read(rom.join(PROFILE_FILE)).ok()?;
    dc_rom::journal::note(
        || format!("file:{}", rom.join(PROFILE_FILE).display()),
        &raw,
    );
    let plain = dc_rez::decrypt_with_key(&raw, &key).ok()?;
    gamma_of_profile_text(&String::from_utf8_lossy(&plain))
}

/// The client's own profile key (read from the user's CShell.dll by RVA, accepted by its pinned SHA-256); `None`
/// when the binary is missing or does not match. The options screen of dc-client reads and writes the profile with
/// it (w3-options).
// EVIDENCE: cshell+0x259930 (see PROFILE_KEY_RVA)
pub fn profile_key(rom: &Path) -> Option<Vec<u8>> {
    let pe = dc_rom::PeFile::from_file(&rom.join("Game").join("CShell.dll")).ok()?;
    dc_rom::crypt_key_from_pe(&pe, PROFILE_KEY_RVA, PROFILE_KEY_SHA256).ok()
}

/// Final copy of a frame through the gamma ramp: the frame is drawn into [`GammaStage::view`] (an intermediate
/// target of the screen's size and format), then [`GammaStage::encode`] writes it through the ramp into the
/// screen's view. With gamma 1 nothing is needed ([`GammaStage::active`] is false).
pub struct GammaStage {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    lut: wgpu::Buffer,
    format: wgpu::TextureFormat,
    target: Option<(
        wgpu::Texture,
        wgpu::TextureView,
        (u32, u32),
        wgpu::BindGroup,
    )>,
    gamma: f32,
}

const SHADER: &str = r#"
// Gamma copy of the DeCombat renderer: one full-screen triangle, each pixel looked up in the ramp table.
struct Ramp { v: array<vec4<f32>, 256> };
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> ramp: Ramp;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(src, vec2<i32>(p.xy), 0);
    let k = vec3<u32>(round(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0));
    return vec4<f32>(ramp.v[k.x].x, ramp.v[k.y].y, ramp.v[k.z].z, c.a);
}
"#;

impl GammaStage {
    /// The stage for screens of `format` with `gamma`.
    pub fn new(ctx: &GpuContext, format: wgpu::TextureFormat, gamma: f32) -> GammaStage {
        let device = &ctx.device;
        let module = crate::determinism::shader_module(
            device,
            wgpu::ShaderModuleDescriptor {
                label: Some("gamma copy"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            },
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gamma copy"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gamma copy"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = crate::determinism::render_pipeline(
            device,
            &wgpu::RenderPipelineDescriptor {
                label: Some("gamma copy"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            },
        );
        let lut = crate::perf::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("gamma ramp"),
                size: 256 * 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        let mut s = GammaStage {
            pipeline,
            layout,
            lut,
            format,
            target: None,
            gamma: 1.0,
        };
        s.set_gamma(ctx, gamma);
        s
    }

    /// Changes the gamma (writes the ramp table).
    pub fn set_gamma(&mut self, ctx: &GpuContext, gamma: f32) {
        self.gamma = if gamma.is_finite() && gamma > 1e-3 {
            gamma
        } else {
            1.0
        };
        let t = ramp8(&ramp_for_gamma(self.gamma));
        let mut data = Vec::with_capacity(256 * 4);
        for v in t {
            let f = v as f32 / 255.0;
            data.extend_from_slice(&[f, f, f, 1.0]);
        }
        crate::perf::write_buffer(&ctx.queue, &self.lut, 0, bytemuck::cast_slice(&data));
    }

    pub fn gamma(&self) -> f32 {
        self.gamma
    }

    /// The ramp changes the picture (gamma not 1).
    pub fn active(&self) -> bool {
        (self.gamma - 1.0).abs() > 1e-4
    }

    /// Makes the intermediate target `size` (re-created when the size changed). Call before [`GammaStage::view`].
    pub fn prepare(&mut self, ctx: &GpuContext, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        if self.target.as_ref().is_some_and(|t| t.2 == size) {
            return;
        }
        let tex = crate::perf::texture(
            &ctx.device,
            &wgpu::TextureDescriptor {
                label: Some("gamma source"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        );
        let view = tex.create_view(&Default::default());
        let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gamma copy"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.lut.as_entire_binding(),
                },
            ],
        });
        self.target = Some((tex, view, size, group));
    }

    /// The view to draw the frame into (after [`GammaStage::prepare`]).
    pub fn view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.1)
    }

    /// Writes the intermediate target through the ramp into `screen` (same size and format).
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, screen: &wgpu::TextureView) {
        let Some((_, _, _, group)) = &self.target else {
            return;
        };
        let stamp = crate::perf::pass_stamp("gamma");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gamma copy"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: screen,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: stamp.as_ref().map(crate::perf::PassStamp::writes),
            occlusion_query_set: None,
        });
        crate::perf::add(crate::perf::Counter::ClientDraws, 1);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_matches_the_engine_formula() {
        let r = ramp_for_gamma(1.0);
        assert_eq!(r[0], 0);
        assert_eq!(r[255], 65535);
        // identity at gamma 1 up to the truncation of i / 255 * 65535
        for (i, v) in r.iter().enumerate() {
            assert!((*v as i64 - i as i64 * 257).abs() <= 1, "{i}: {v}");
        }
        let g = ramp_for_gamma(1.22);
        assert_eq!(g[0], 0);
        assert_eq!(g[255], 65535);
        let x: f64 = 64.0 / 255.0;
        assert_eq!(g[64], (x.powf(1.0 / 1.22_f32 as f64) * 65535.0) as u16);
        assert!(g.windows(2).all(|w| w[0] <= w[1]));
        // brightens the darks more than the lights
        assert!(g[40] >> 8 > 48 && g[200] >> 8 < 215);
        // a broken gamma falls back to the identity
        assert_eq!(ramp_for_gamma(f32::NAN), ramp_for_gamma(1.0));
    }

    #[test]
    fn apply_leaves_alpha_and_identity_alone() {
        let mut px = vec![10, 128, 250, 7];
        apply_ramp(&mut px, &ramp_for_gamma(1.0));
        assert_eq!(px, vec![10, 128, 250, 7]);
        apply_ramp(&mut px, &ramp_for_gamma(1.5));
        assert!(px[0] > 10 && px[1] > 128 && px[2] >= 250);
        assert_eq!(px[3], 7);
    }

    #[test]
    fn profile_text_gamma() {
        // synthetic section and entry names (the real ones are matched by hash only)
        let (sec, key) = ("Sec", "Ent");
        let hs = dc_world::hash16_hex(sec.as_bytes());
        let hk = dc_world::hash16_hex(key.as_bytes());
        let find = |t: &str| entry_of_profile_text(t, (&hs, 3), (&hk, 3));
        let t = format!("[Other]\n{key} = 3.0\n[{sec}]\nWidth = 1\n{key} = 1.25f\n");
        assert_eq!(find(&t), Some(1.25));
        let t = format!("[{sec}]\n{key}\t\t= \"0.9\"\n");
        assert_eq!(find(&t), Some(0.9));
        assert_eq!(find(&format!("[{sec}]\n{key} = x\n")), None);
        assert_eq!(find(&format!("[{sec}]\n{key} = -1\n")), None);
        assert_eq!(find("[Other]\nA = 1\n"), None);
        assert_eq!(gamma_of_profile_text("[Other]\nA = 1\n"), None);
    }
}
