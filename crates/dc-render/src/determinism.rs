//! The render determinism contract's tooling (gate G0 of `docs/RENDER_PERF_GATES.md`, the contract in
//! `docs/RENDER_DETERMINISM_CONTRACT.md`):
//!
//! - [`texel_hash`]: the exact-lane frame hash. SHA-256 over a header that states the texel layout
//!   ([`TEXEL_CONTRACT`], format, width, height) and the tightly packed RGBA8 rows, top row first. Only intended
//!   texels: read-backs strip the row padding ([`crate::unpack_rows`]) and force alpha to 255 before this runs.
//! - The GPU-object journal: every shader module, render pipeline and sampler made through [`shader_module`],
//!   [`render_pipeline`] and [`sampler`] notes the digest of its WGSL source or of its pixel-relevant descriptor
//!   settings while [`journal_enable`] is on, for the exact-lane manifest. Off it costs one relaxed atomic load per
//!   creation (creations are load-time events).
//! - [`mutation`]: the test-only perturbation knob `DC_DETERMINISM_MUTATE` (`arith`, `order` or `all`), read once
//!   per process. The exact-lane tests run a child process with it and require the hashes to CHANGE (a deliberate
//!   arithmetic or ordering change must be detected). Unset (every real run): no effect, one atomic load per frame.
// Provenance: ORIGINAL (our own engineering contract; no 2009 behaviour claims)

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Name and version of the texel layout every exact-lane hash covers.
pub const TEXEL_CONTRACT: &str = "dc-texels/1";

/// The exact-lane hash of an RGBA8 image (`rgba.len() == width * height * 4`, rows tightly packed, top row
/// first): SHA-256 of `"dc-texels/1 rgba8 <width>x<height>\n"` followed by the texels. `tools/pixhash.py` computes
/// the same digest from a PNG.
pub fn texel_hash(width: u32, height: u32, rgba: &[u8]) -> [u8; 32] {
    debug_assert_eq!(rgba.len(), width as usize * height as usize * 4);
    let mut h = dc_rom::Sha256::new();
    h.update(format!("{TEXEL_CONTRACT} rgba8 {width}x{height}\n").as_bytes());
    h.update(rgba);
    h.finalize()
}

/// Lower-case hex of a digest.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The deliberate perturbations of `DC_DETERMINISM_MUTATE` (all false in every real run).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mutation {
    /// One arithmetic term: the world clip transform's x row is scaled by `1 + 1/256`.
    pub arith: bool,
    /// One ordering: the opaque world batches, the model instances and the sorted translucent world objects are
    /// recorded in reverse order.
    pub order: bool,
}

impl Mutation {
    /// Parses the knob's value (`arith`, `order`, `all`; anything else: none).
    pub fn parse(v: &str) -> Mutation {
        match v.trim().to_ascii_lowercase().as_str() {
            "arith" => Mutation {
                arith: true,
                order: false,
            },
            "order" => Mutation {
                arith: false,
                order: true,
            },
            "all" | "1" => Mutation {
                arith: true,
                order: true,
            },
            _ => Mutation::default(),
        }
    }
}

/// The process's mutation knob (`DC_DETERMINISM_MUTATE`, read once).
#[inline]
pub fn mutation() -> Mutation {
    static M: OnceLock<Mutation> = OnceLock::new();
    *M.get_or_init(|| {
        std::env::var("DC_DETERMINISM_MUTATE")
            .map(|v| Mutation::parse(&v))
            .unwrap_or_default()
    })
}

static JOURNAL_ON: AtomicBool = AtomicBool::new(false);
static JOURNAL: Mutex<Journal> = Mutex::new(Journal::new());

/// Digests of the GPU objects made while the journal was on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Journal {
    /// (label, SHA-256 of the WGSL source).
    pub shaders: BTreeSet<(String, [u8; 32])>,
    /// (label, SHA-256 of the pixel-relevant descriptor settings).
    pub pipelines: BTreeSet<(String, [u8; 32])>,
    /// (label, SHA-256 of the sampler descriptor's settings).
    pub samplers: BTreeSet<(String, [u8; 32])>,
}

impl Journal {
    const fn new() -> Journal {
        Journal {
            shaders: BTreeSet::new(),
            pipelines: BTreeSet::new(),
            samplers: BTreeSet::new(),
        }
    }
}

/// Starts (`true`, clearing earlier notes) or stops the GPU-object journal (process-global; one exact-lane run at a
/// time, see `dc_rom::journal::lock`).
pub fn journal_enable(on: bool) {
    if on {
        *JOURNAL.lock().unwrap_or_else(|p| p.into_inner()) = Journal::new();
    }
    JOURNAL_ON.store(on, Ordering::Relaxed);
}

/// What the journal noted since [`journal_enable`].
pub fn journal_snapshot() -> Journal {
    JOURNAL.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

#[inline]
fn journal_on() -> bool {
    JOURNAL_ON.load(Ordering::Relaxed)
}

fn label_of(label: Option<&str>) -> String {
    label.unwrap_or("(unlabelled)").to_string()
}

/// `device.create_shader_module`, noting the WGSL source's digest while the journal is on.
pub fn shader_module(
    device: &wgpu::Device,
    desc: wgpu::ShaderModuleDescriptor<'_>,
) -> wgpu::ShaderModule {
    if journal_on() {
        let digest = match &desc.source {
            wgpu::ShaderSource::Wgsl(src) => dc_rom::sha256(src.as_bytes()),
            #[allow(unreachable_patterns)]
            _ => [0u8; 32],
        };
        JOURNAL
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .shaders
            .insert((label_of(desc.label), digest));
    }
    // gpu-prewarm: counted, and logged when made in play or on a loader thread
    crate::warm::note("shader", desc.label, None);
    device.create_shader_module(desc)
}

/// The pixel-relevant settings of a render pipeline descriptor as canonical text: entry points, vertex layouts,
/// colour targets (format, blend, write mask), primitive state, depth-stencil state and multisampling. The shader
/// source is covered by the shader digests; layouts by the bind-group contract of the crate version.
pub fn pipeline_settings(desc: &wgpu::RenderPipelineDescriptor<'_>) -> String {
    let mut s = format!(
        "vs={:?} buffers={:?}\n",
        desc.vertex.entry_point, desc.vertex.buffers
    );
    if let Some(f) = &desc.fragment {
        s.push_str(&format!("fs={:?} targets={:?}\n", f.entry_point, f.targets));
    }
    s.push_str(&format!(
        "primitive={:?}\ndepth={:?}\nmultisample={:?}\nmultiview={:?}\n",
        desc.primitive, desc.depth_stencil, desc.multisample, desc.multiview
    ));
    s
}

/// `device.create_render_pipeline`, noting [`pipeline_settings`]'s digest while the journal is on.
pub fn render_pipeline(
    device: &wgpu::Device,
    desc: &wgpu::RenderPipelineDescriptor<'_>,
) -> wgpu::RenderPipeline {
    render_pipeline_for(device, desc, None)
}

/// [`render_pipeline`] with what the pipeline is for beside its label (`detail`, the model pass's raster state), for
/// the gpu-prewarm log of a pipeline made in play ([`crate::warm`]).
pub(crate) fn render_pipeline_for(
    device: &wgpu::Device,
    desc: &wgpu::RenderPipelineDescriptor<'_>,
    detail: Option<&str>,
) -> wgpu::RenderPipeline {
    crate::warm::note("pipeline", desc.label, detail);
    if journal_on() {
        let digest = dc_rom::sha256(pipeline_settings(desc).as_bytes());
        JOURNAL
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pipelines
            .insert((label_of(desc.label), digest));
    }
    crate::perf::render_pipeline(device, desc)
}

/// `device.create_sampler`, noting the descriptor's settings digest while the journal is on.
pub fn sampler(device: &wgpu::Device, desc: &wgpu::SamplerDescriptor<'_>) -> wgpu::Sampler {
    if journal_on() {
        let mut d = desc.clone();
        d.label = None;
        let digest = dc_rom::sha256(format!("{d:?}").as_bytes());
        JOURNAL
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .samplers
            .insert((label_of(desc.label), digest));
    }
    crate::perf::sampler(device, desc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texel_hash_covers_layout_and_texels() {
        let a = vec![1u8; 2 * 3 * 4];
        let h = texel_hash(2, 3, &a);
        assert_eq!(h, texel_hash(2, 3, &a), "a pure function");
        assert_ne!(h, texel_hash(3, 2, &a), "dimensions are in the hash");
        let mut b = a.clone();
        b[5] ^= 1;
        assert_ne!(h, texel_hash(2, 3, &b), "one bit of one texel");
        // the header form is part of the contract (tools/pixhash.py computes the same bytes)
        let mut s = dc_rom::Sha256::new();
        s.update(b"dc-texels/1 rgba8 2x3\n");
        s.update(&a);
        assert_eq!(h, s.finalize());
    }

    #[test]
    fn mutation_values() {
        assert_eq!(Mutation::parse(""), Mutation::default());
        assert!(Mutation::parse("arith").arith && !Mutation::parse("arith").order);
        assert!(Mutation::parse("ORDER").order);
        assert_eq!(
            Mutation::parse("all"),
            Mutation {
                arith: true,
                order: true
            }
        );
    }
}
