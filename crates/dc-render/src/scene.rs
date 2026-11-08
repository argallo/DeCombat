//! CPU side of one frame's world: vertex and index buffers, per-section draw batches sorted into the 23
//! buckets, the textures they need and the decoded lightmaps.

use crate::bucket::{select_bucket, PassKind, RenderSettings, SectionClass, TextureTraits};
use crate::lightmap::{decode_lightmap, lightmap_rgba, TexelOrder};
use crate::vertex::{GpuVertex, RenderVertex};
use dc_world::{ObjectRole, RenderBlock, RenderWorld, World};
use std::collections::HashMap;

/// What to put into the frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneOptions {
    pub settings: RenderSettings,
    /// Also draw the nested render worlds of world models (doors, breakables, ...) at their stored position.
    /// The engine draws them later through the object queue with the object transform (RENDER_2009.md §5.1
    /// step 6); this slice has no objects and uses the identity transform.
    pub world_models: bool,
    pub lightmap_order: TexelOrder,
    /// Intensity of every render light group added onto the lightmaps (RENDER_2009.md §6.2); `None` leaves the
    /// base lightmaps alone. The game switches groups at run time (not specified); the viewer default is all
    /// groups on at intensity 1.
    pub light_groups: Option<f32>,
    /// Build the sky world models into [`SceneMesh::sky_batches`] for the sky pass (RENDER_2009.md §5.3).
    pub sky: bool,
    /// Build the translucent and additive world models into [`SceneMesh::blended`] for the object queue's
    /// translucent pass (RENDER_2009.md §7.1).
    pub translucent: bool,
    /// Take the world's linear fog (and the sky pass's) from its world-settings object ([`crate::world_fog`]);
    /// off draws without fog (differential screenshots).
    pub fog: bool,
}

impl Default for SceneOptions {
    fn default() -> Self {
        SceneOptions {
            settings: RenderSettings::default(),
            world_models: true,
            lightmap_order: TexelOrder::default(),
            light_groups: Some(1.0),
            sky: true,
            translucent: true,
            fog: true,
        }
    }
}

/// One translucent or additive world model: its batches (bucket order) and what the object queue sorts and
/// blends it by.
#[derive(Debug, Clone, PartialEq)]
pub struct BlendedModel {
    pub batches: Vec<Batch>,
    /// The object position: the sort key is its squared distance from the eye (RENDER_2009.md §7.1 step 9,
    /// object +0xc8). Falls back to the centre of the model's vertices when the object has none.
    pub origin: [f32; 3],
    /// SRC = SRCALPHA, DST = ONE instead of INVSRCALPHA (RENDER_2009.md §5.1 step 6, engine+0x1a9790).
    pub additive: bool,
    /// The object's alpha property (scales the fragment alpha).
    pub alpha: f32,
}

/// Adds the render light groups of `block` that have patches for section `section` onto that section's
/// lightmap (RGBA8, `w * h`). Returns the number of patches applied.
///
/// Per group: channel colour `c = trunc(255 * colour * intensity)` (the group colour is stored as three floats
/// in [0, 1], OBSERVED); groups whose colour is black are skipped. Per patch `(x, y, w, h, bytes)`: walk the
/// rectangle row by row; a byte `0xff` followed by a count repeats the previous value that many texels, any other
/// byte is the next value; each texel channel gets `c * value / 255` added with saturation at 255. The division
/// by 255 is our reading (RENDER_2009.md §16 item 4: the scaling is UNPROVEN).
// SPEC: RENDER_2009.md §6.2 (engine+0x17b6d0), FMT_DAT85.md §10.3 (group, slots, patches). OBSERVED over the two
// ROM worlds with light groups: one slot per section of the block, patches only in kind-2 slots, every patch
// rectangle inside its lightmap, and the escape rule yields exactly w * h values in all 4,364 patches.
pub fn apply_light_groups(
    rgba: &mut [u8],
    w: u32,
    h: u32,
    block: &RenderBlock<'_>,
    section: usize,
    intensity: f32,
) -> u64 {
    let mut applied = 0;
    for g in &block.light_groups {
        let Some(slot) = g.slots.get(section) else {
            continue;
        };
        let c = g
            .colour
            .map(|v| (255.0 * v * intensity).clamp(0.0, 255.0) as u32);
        if c == [0, 0, 0] {
            continue;
        }
        for p in slot {
            let [px, py, pw, ph] = p.words;
            if px.checked_add(pw).is_none_or(|e| e > w) || py.checked_add(ph).is_none_or(|e| e > h)
            {
                continue;
            }
            let total = (pw as usize) * (ph as usize);
            let mut i = 0usize;
            let mut k = 0usize;
            let mut prev = 0u32;
            let put = |idx: usize, v: u32, rgba: &mut [u8]| {
                let tx = px as usize + idx % pw as usize;
                let ty = py as usize + idx / pw as usize;
                let o = (ty * w as usize + tx) * 4;
                for ch in 0..3 {
                    let add = c[ch] * v / 255;
                    rgba[o + ch] = (rgba[o + ch] as u32 + add).min(255) as u8;
                }
            };
            while k < total && i < p.data.len() {
                let b = p.data[i];
                if b == 0xff && i + 1 < p.data.len() {
                    let run = p.data[i + 1] as usize;
                    for _ in 0..run.min(total - k) {
                        put(k, prev, rgba);
                        k += 1;
                    }
                    i += 2;
                } else {
                    prev = b as u32;
                    put(k, prev, rgba);
                    k += 1;
                    i += 1;
                }
            }
            applied += 1;
        }
    }
    applied
}

/// One decoded lightmap.
#[derive(Debug, Clone, PartialEq)]
pub struct LightmapImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// One indexed draw: a section's triangles with its bucket and bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batch {
    pub bucket: u8,
    pub pass: PassKind,
    /// Index into [`SceneMesh::textures`].
    pub tex0: Option<u32>,
    pub tex1: Option<u32>,
    /// Index into [`SceneMesh::lightmaps`] (bucket 14 only).
    pub lightmap: Option<u32>,
    pub first_index: u32,
    pub index_count: u32,
}

/// Counters for the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SceneStats {
    pub blocks: u64,
    pub sections: u64,
    pub sections_drawn: u64,
    pub sections_hidden: u64,
    pub triangles_drawn: u64,
    /// Sections whose bucket recipe is stood in for by the base recipe of its group.
    pub sections_approximated: u64,
    pub lightmaps: u64,
    pub lightmaps_bad: u64,
    /// Light-group patches added onto lightmaps.
    pub light_group_patches: u64,
    /// Sections whose triangle range runs past the block's triangle list.
    pub bad_ranges: u64,
    pub world_models: u64,
    /// Sky world models not built (sky pass off in [`SceneOptions`]).
    pub sky_models_skipped: u64,
    /// World models of objects that are not visible at start (left out).
    pub hidden_models_skipped: u64,
    /// Translucent or additive world models not built (translucent pass off in [`SceneOptions`]).
    pub blended_models_skipped: u64,
    /// Sky world models built for the sky pass, and their sections.
    pub sky_models: u64,
    pub sky_sections: u64,
    /// Translucent / additive world models built for the translucent pass, their sections, and the kind-2
    /// (lightmap) sections left out of them.
    pub blended_models: u64,
    pub blended_additive: u64,
    pub blended_sections: u64,
    pub blended_lightmap_sections_dropped: u64,
    pub sky_portals: u64,
    pub per_bucket: [u64; crate::bucket::BUCKET_COUNT],
}

/// The world geometry of one frame.
#[derive(Debug, Clone, Default)]
pub struct SceneMesh {
    pub vertices: Vec<GpuVertex>,
    pub indices: Vec<u32>,
    /// Sorted by bucket (stable: block order inside a bucket), the flush order of RENDER_2009.md §5.1 step 5.
    pub batches: Vec<Batch>,
    /// Texture names as stored in the world (resolved by the caller at run time).
    pub textures: Vec<Vec<u8>>,
    pub lightmaps: Vec<LightmapImage>,
    pub stats: SceneStats,
    /// World-space bounds of the drawn vertices (main view: level and opaque world models).
    pub bounds: Option<([f32; 3], [f32; 3])>,
    /// Sections of the sky world models, bucket order (RENDER_2009.md §5.3).
    pub sky_batches: Vec<Batch>,
    /// Bounds of the sky geometry.
    pub sky_bounds: Option<([f32; 3], [f32; 3])>,
    /// Sky portals of the level's render blocks (vertex loops, world space): the sky is drawn only into their
    /// screen bounds (RENDER_2009.md §5.1 step 3, §5.3).
    pub sky_portals: Vec<Vec<[f32; 3]>>,
    /// Translucent and additive world models (RENDER_2009.md §7.1 step 8).
    pub blended: Vec<BlendedModel>,
    /// The world's extents (FMT_DAT85.md §4 items 3-4), the box the sky eye is placed by (RENDER_2009 addendum).
    pub world_extents: Option<([f32; 3], [f32; 3])>,
    /// Linear fog of the world pass and of the sky pass (RENDER_2009.md §3.3), from the world settings.
    pub fog: Option<crate::fog::Fog>,
    pub sky_fog: Option<crate::fog::Fog>,
    /// The world models built into the main view and the translucent pass, by name, with the index range of their
    /// triangles (in build order, ranges ascending and disjoint): the object set switches them on and off.
    pub models: Vec<SceneModel>,
    /// The sky world models built into [`SceneMesh::sky_batches`], by name, with the index range of their triangles
    /// (build order): the server's sky table picks which of them the sky pass draws ([`crate::GpuScene`]).
    // EVIDENCE: engine+0x188c0 (the scene's sky-object list is the sky table's ids resolved to objects)
    pub sky_models: Vec<SceneModel>,
    /// Draw both faces of [`SceneMesh::batches`] (the level and the opaque world models, but for filler and cut-out
    /// cards, [`crate::GpuScene::two_sided`]; not the sky, not the translucent world models): the world's `two_sided`
    /// setting ([`world_two_sided`]). False for every ROM world.
    // DC-DEFAULT: MAP-118
    pub two_sided: bool,
}

/// One world model of the scene: the name of its nested render world and the index range `[first_index,
/// end_index)` its sections were built into.
// SPEC: RENDER_2009.md §5.1 step 6 (world models are drawn by the object queue: only those with an object, with the
// visible bit of that object); FMT_DAT85.md §10 (nested render worlds keyed by name)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneModel {
    pub name: Vec<u8>,
    pub first_index: u32,
    pub end_index: u32,
}

impl SceneMesh {
    /// The scene model whose index range holds `first_index` (a batch of it), if any.
    pub fn model_of(&self, first_index: u32) -> Option<usize> {
        let i = self.models.partition_point(|m| m.end_index <= first_index);
        self.models
            .get(i)
            .filter(|m| m.first_index <= first_index && first_index < m.end_index)
            .map(|_| i)
    }

    /// The sky model ([`SceneMesh::sky_models`] index) whose triangles start at `first_index`.
    pub fn sky_model_of(&self, first_index: u32) -> Option<usize> {
        self.sky_models
            .iter()
            .position(|m| m.first_index <= first_index && first_index < m.end_index)
    }
}

/// Where the sections of a render world go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Main,
    Sky,
    Blended,
}

// Object properties of world models that this slice reads, as `dc_world::names` keys (the ROM name by
// `sha256(name)[:16]` and length so no ROM string is stored, FMT_DAT85.md §12, or the `dc.world_model` names
// `visible`, `blend`, `alpha` and the values `opaque` / `translucent` / `additive`; the hidden and start-hidden
// flags are ROM-only): `PROP_VISIBLE`, `PROP_HIDDEN`, `PROP_START_HIDDEN`, `PROP_BLEND`, `BLEND_OPAQUE`,
// `BLEND_ADDITIVE`, `PROP_ALPHA`. Values of the blend-mode text are identified the same way.
// OBSERVED: property and value names of the world-model objects of the 21 ROM worlds (hash, length)
// SPEC: RENDER_2009.md §5.1 step 6 (blend-mode values: opaque dc937b59892604f5/4, translucent fa5668c88f92faf4/11,
// additive 765fc5369dc160b0/8; alpha property b1a96dd646bccaa2/5, values 0, 0.3, 0.5, 0.7, 0.75, 1 in the ROM)
// DC-DEFAULT: MAP-26 (the renderer reads the vocabulary names through `Key::matches`)
use dc_world::names::{
    BLEND_ADDITIVE, BLEND_OPAQUE, PROP_ALPHA, PROP_BLEND, PROP_HIDDEN, PROP_START_HIDDEN,
    PROP_TWO_SIDED, PROP_VISIBLE,
};

fn named(name: &[u8], key: &dc_world::Key) -> bool {
    key.matches(name)
}

/// Whether the world asks for both faces of its geometry: the `two_sided` property of its world-settings object (the
/// first object of that role). A map built from pieces of ROM worlds sets it, because a ROM world faces its surfaces
/// only towards its own play area and the composed map shows them from outside too. The key is DeCombat's own and is
/// read through [`dc_world::Object::prop_named`], so only a `dc.world_settings` object carries it (MAP-2): a ROM world
/// is always one-sided. `true` (or a non-zero number, as the fog switch reads, [`crate::world_fog`]) turns it on.
// DC-DEFAULT: MAP-118
pub fn world_two_sided(world: &World<'_>) -> bool {
    world
        .objects
        .iter()
        .find(|o| o.role == Some(ObjectRole::WorldSettings))
        .and_then(|o| o.prop_named(&PROP_TWO_SIDED))
        .is_some_and(|p| match p.value {
            dc_world::FieldValue::Bool(b) => b,
            dc_world::FieldValue::Real(v) => v != 0.0 && !v.is_nan(),
            dc_world::FieldValue::Int(v) | dc_world::FieldValue::Bits(v) => v != 0,
            _ => false,
        })
}

/// How the main pass treats a nested render model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModelVisibility {
    /// Opaque and visible: drawn with the world sections (identity transform; RENDER_2009.md §5.1 step 6 draws
    /// it later with the object transform, which is the identity until the object moves).
    Drawn,
    /// Sky geometry: belongs to the sky camera (RENDER_2009.md §5.3), not the main view.
    Sky,
    /// The object starts invisible or hidden (RENDER_2009.md §5.1 step 1: visible bit 0x1, hidden bit 0x20).
    Hidden,
    /// Translucent (alpha blend) or additive: drawn by the object queue's translucent state, sorted back to
    /// front (RENDER_2009.md §5.1 step 6, §7.1 steps 7-9). `alpha` is the object's alpha property (1 when
    /// absent).
    Translucent { additive: bool, alpha: f32 },
}

/// Visibility of the world model owned by `obj` (the object whose name equals the render model's name,
/// FMT_DAT85.md §6.4). The game module turns object properties into engine flags (CShell, not specified);
/// our reading of the properties: OBSERVED over the 21 ROM worlds, the visibility flag is false on all 1,008
/// kill volumes, 47 ladders, 20 hazard volumes and 108 other world models, and 347 world models carry a
/// non-opaque blend mode (202 translucent, 145 additive).
///
/// A `dc.world_model` (or any other brush class of the vocabulary) reads the same way: `visible` false hides it,
/// `blend` `translucent` / `additive` sends it to the translucent pass with its `alpha`.
// DC-DEFAULT: MAP-26
pub fn model_visibility(obj: Option<&dc_world::Object<'_>>) -> ModelVisibility {
    let Some(o) = obj else {
        return ModelVisibility::Drawn;
    };
    if o.role == Some(ObjectRole::SkyWorldModel) {
        return ModelVisibility::Sky;
    }
    let mut blend: Option<bool> = None;
    let mut alpha = 1.0f32;
    for p in &o.props {
        match p.value {
            dc_world::FieldValue::Bool(false) if named(p.name, &PROP_VISIBLE) => {
                return ModelVisibility::Hidden;
            }
            dc_world::FieldValue::Bool(true)
                if named(p.name, &PROP_HIDDEN) || named(p.name, &PROP_START_HIDDEN) =>
            {
                return ModelVisibility::Hidden;
            }
            dc_world::FieldValue::Text(t)
                if named(p.name, &PROP_BLEND) && !named(t, &BLEND_OPAQUE) =>
            {
                // any non-opaque value other than the additive one blends with alpha (the ROM has only the
                // translucent value besides these two; the vocabulary's `translucent` lands here too)
                blend = Some(named(t, &BLEND_ADDITIVE));
            }
            dc_world::FieldValue::Real(a) if named(p.name, &PROP_ALPHA) && a.is_finite() => {
                alpha = a.clamp(0.0, 1.0);
            }
            _ => {}
        }
    }
    match blend {
        Some(additive) => ModelVisibility::Translucent { additive, alpha },
        None => ModelVisibility::Drawn,
    }
}

struct Builder<'f> {
    mesh: SceneMesh,
    tex_ids: HashMap<Vec<u8>, u32>,
    traits: &'f mut dyn FnMut(&[u8]) -> TextureTraits,
    opts: SceneOptions,
    target: Target,
    /// Bounds of the sky or blended model being built.
    aux_bounds: Option<([f32; 3], [f32; 3])>,
}

fn grow(b: &mut Option<([f32; 3], [f32; 3])>, p: [f32; 3]) {
    let b = b.get_or_insert((p, p));
    for (k, &v) in p.iter().enumerate() {
        b.0[k] = b.0[k].min(v);
        b.1[k] = b.1[k].max(v);
    }
}

impl Builder<'_> {
    fn texture(&mut self, name: &[u8]) -> Option<u32> {
        if name.is_empty() {
            return None;
        }
        if let Some(&i) = self.tex_ids.get(name) {
            return Some(i);
        }
        let i = self.mesh.textures.len() as u32;
        self.mesh.textures.push(name.to_vec());
        self.tex_ids.insert(name.to_vec(), i);
        Some(i)
    }

    fn grow_bounds(&mut self, p: [f32; 3]) {
        match self.target {
            Target::Main => grow(&mut self.mesh.bounds, p),
            Target::Sky | Target::Blended => grow(&mut self.aux_bounds, p),
        }
    }

    fn block(&mut self, b: &RenderBlock<'_>) {
        self.mesh.stats.blocks += 1;
        let base = self.mesh.vertices.len() as u32;
        for i in 0..b.vertex_count as usize {
            // The parser bounds every block's vertex bytes; a short block simply stops here.
            let Some(v) = RenderVertex::read(b.vertices, i) else {
                break;
            };
            self.mesh.vertices.push(GpuVertex::from(&v));
        }
        // SPEC: FMT_DAT85.md §10.2 section item 3 (section_triangles, running triangle base kept per section)
        let mut first_tri = 0u32;
        for (si, s) in b.sections.iter().enumerate() {
            self.mesh.stats.sections += 1;
            let tri_base = first_tri;
            first_tri = first_tri.saturating_add(s.triangles);
            let class = SectionClass::of_kind(s.shader_kind);
            // OBSERVED: every kind-2 section names one first texture that resolves in no archive; the lightmap
            // pass binds only the section lightmap (RENDER_2009.md §5.2 bucket 14), so its names are not loaded.
            let uses_textures = class != SectionClass::Lightmap && class != SectionClass::Hidden;
            let tex0 = if uses_textures {
                self.texture(s.textures[0])
            } else {
                None
            };
            let tex1 = if uses_textures {
                self.texture(s.textures[1])
            } else {
                None
            };
            let traits = if uses_textures && !s.textures[0].is_empty() {
                (self.traits)(s.textures[0])
            } else {
                TextureTraits::default()
            };
            let Some(bucket) = select_bucket(class, traits, &self.opts.settings) else {
                self.mesh.stats.sections_hidden += 1;
                continue;
            };
            let Some(pass) = PassKind::of_bucket(bucket) else {
                self.mesh.stats.sections_hidden += 1;
                continue;
            };
            // DeCombat: a blended world model cannot run the lightmap + multiply pair (the multiply would read
            // what is behind the model); its kind-4 / kind-9 sections take the vertex-lit recipe of their family
            // and its kind-2 sections are left out (OBSERVED: 1 + 1 such sections in the ROM, all other sections
            // of blended world models are kind 1).
            let pass = if self.target == Target::Blended {
                match pass {
                    PassKind::Lightmap => {
                        self.mesh.stats.blended_lightmap_sections_dropped += 1;
                        continue;
                    }
                    PassKind::LightmappedTexture => PassKind::VertexLit,
                    PassKind::BlendLightmapped => PassKind::BlendVertexLit,
                    p => p,
                }
            } else {
                pass
            };
            match self.target {
                Target::Sky => self.mesh.stats.sky_sections += 1,
                Target::Blended => self.mesh.stats.blended_sections += 1,
                Target::Main => {}
            }
            if tri_base.saturating_add(s.triangles) > b.triangle_count {
                self.mesh.stats.bad_ranges += 1;
                continue;
            }
            let lightmap = if pass == PassKind::Lightmap {
                // SPEC: RENDER_2009.md §6.1 (kind-2 payload, RLE, w*h*3 bytes)
                match decode_lightmap(s.lightmap, s.lightmap_w, s.lightmap_h) {
                    Some(t) => {
                        let i = self.mesh.lightmaps.len() as u32;
                        let mut rgba = lightmap_rgba(&t, self.opts.lightmap_order);
                        if let Some(k) = self.opts.light_groups {
                            self.mesh.stats.light_group_patches +=
                                apply_light_groups(&mut rgba, s.lightmap_w, s.lightmap_h, b, si, k);
                        }
                        self.mesh.lightmaps.push(LightmapImage {
                            width: s.lightmap_w,
                            height: s.lightmap_h,
                            rgba,
                        });
                        self.mesh.stats.lightmaps += 1;
                        Some(i)
                    }
                    None => {
                        self.mesh.stats.lightmaps_bad += 1;
                        None
                    }
                }
            } else {
                None
            };
            let first_index = self.mesh.indices.len() as u32;
            for t in tri_base..tri_base + s.triangles {
                let Some(tri) = b.triangle(t) else { break };
                for &vi in &tri[..3] {
                    let gi = base + vi;
                    if let Some(v) = self.mesh.vertices.get(gi as usize) {
                        let p = v.pos;
                        self.grow_bounds(p);
                    }
                    self.mesh.indices.push(gi);
                }
            }
            let index_count = self.mesh.indices.len() as u32 - first_index;
            if !PassKind::exact_for(bucket) {
                self.mesh.stats.sections_approximated += 1;
            }
            self.mesh.stats.sections_drawn += 1;
            self.mesh.stats.triangles_drawn += (index_count / 3) as u64;
            self.mesh.stats.per_bucket[bucket as usize] += 1;
            self.mesh.batches.push(Batch {
                bucket,
                pass,
                tex0,
                tex1,
                lightmap,
                first_index,
                index_count,
            });
        }
    }

    fn render_world(&mut self, w: &RenderWorld<'_>) {
        for b in &w.blocks {
            self.block(b);
        }
    }
}

impl SceneMesh {
    /// Height of the highest main-view triangle straight below `p` within `max_drop` world units (a viewer
    /// helper to stand a model on the floor; not a 2009 behaviour).
    pub fn floor_below(&self, p: [f32; 3], max_drop: f32) -> Option<f32> {
        let (px, pz) = (p[0], p[2]);
        let mut best: Option<f32> = None;
        for b in &self.batches {
            let range = b.first_index as usize..(b.first_index + b.index_count) as usize;
            let Some(idx) = self.indices.get(range) else {
                continue;
            };
            for t in idx.as_chunks::<3>().0 {
                let v = |i: u32| self.vertices.get(i as usize).map(|v| v.pos);
                let (Some(a), Some(c), Some(d)) = (v(t[0]), v(t[1]), v(t[2])) else {
                    continue;
                };
                // barycentric test in the xz plane
                let den = (c[2] - d[2]) * (a[0] - d[0]) + (d[0] - c[0]) * (a[2] - d[2]);
                if den.abs() < 1e-9 {
                    continue;
                }
                let l1 = ((c[2] - d[2]) * (px - d[0]) + (d[0] - c[0]) * (pz - d[2])) / den;
                let l2 = ((d[2] - a[2]) * (px - d[0]) + (a[0] - d[0]) * (pz - d[2])) / den;
                let l3 = 1.0 - l1 - l2;
                if l1 < 0.0 || l2 < 0.0 || l3 < 0.0 {
                    continue;
                }
                let y = l1 * a[1] + l2 * c[1] + l3 * d[1];
                if y <= p[1] && p[1] - y <= max_drop && best.is_none_or(|b| y > b) {
                    best = Some(y);
                }
            }
        }
        best
    }

    /// Builds the world part of a frame. `traits` reports the first-texture traits of a section texture name
    /// (DTX command mode and flag 0x1) for the bucket choice; the caller resolves names against its ROM.
    pub fn build(
        world: &World<'_>,
        opts: SceneOptions,
        traits: &mut dyn FnMut(&[u8]) -> TextureTraits,
    ) -> SceneMesh {
        let mut b = Builder {
            mesh: SceneMesh::default(),
            tex_ids: HashMap::new(),
            traits,
            opts,
            target: Target::Main,
            aux_bounds: None,
        };
        if let Some(r) = &world.render {
            b.render_world(&r.world);
            // SPEC: FMT_DAT85.md §10.2 item 9 (sky portals of a render block, plane-polys of §10.4)
            for blk in &r.world.blocks {
                for p in &blk.sky_portals {
                    b.mesh.sky_portals.push(p.verts.clone());
                }
            }
            b.mesh.stats.sky_portals = b.mesh.sky_portals.len() as u64;
            for m in &r.world.models {
                let obj = world.objects.iter().find(|o| o.name() == Some(m.name));
                match model_visibility(obj) {
                    ModelVisibility::Drawn if opts.world_models => {
                        b.mesh.stats.world_models += 1;
                        let first_index = b.mesh.indices.len() as u32;
                        b.render_world(&m.world);
                        b.mesh.models.push(SceneModel {
                            name: m.name.to_vec(),
                            first_index,
                            end_index: b.mesh.indices.len() as u32,
                        });
                    }
                    ModelVisibility::Drawn => {}
                    ModelVisibility::Sky if opts.sky => {
                        // SPEC: RENDER_2009.md §5.3 (sky world geometry through the section renderer)
                        b.target = Target::Sky;
                        let start = b.mesh.batches.len();
                        let first_index = b.mesh.indices.len() as u32;
                        b.render_world(&m.world);
                        b.mesh.sky_models.push(SceneModel {
                            name: m.name.to_vec(),
                            first_index,
                            end_index: b.mesh.indices.len() as u32,
                        });
                        let built: Vec<Batch> = b.mesh.batches.drain(start..).collect();
                        b.mesh.sky_batches.extend(built);
                        b.mesh.stats.sky_models += 1;
                        b.target = Target::Main;
                    }
                    ModelVisibility::Sky => b.mesh.stats.sky_models_skipped += 1,
                    ModelVisibility::Hidden => b.mesh.stats.hidden_models_skipped += 1,
                    ModelVisibility::Translucent { additive, alpha }
                        if opts.translucent && opts.world_models =>
                    {
                        b.target = Target::Blended;
                        b.aux_bounds = None;
                        let start = b.mesh.batches.len();
                        let first_index = b.mesh.indices.len() as u32;
                        b.render_world(&m.world);
                        b.mesh.models.push(SceneModel {
                            name: m.name.to_vec(),
                            first_index,
                            end_index: b.mesh.indices.len() as u32,
                        });
                        let mut built: Vec<Batch> = b.mesh.batches.drain(start..).collect();
                        b.target = Target::Main;
                        if built.is_empty() {
                            continue;
                        }
                        built.sort_by_key(|x| x.bucket);
                        let centre = b
                            .aux_bounds
                            .map(|(lo, hi)| [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k])));
                        let origin = obj
                            .and_then(|o| o.position())
                            .or(centre)
                            .unwrap_or([0.0; 3]);
                        b.mesh.stats.blended_models += 1;
                        if additive {
                            b.mesh.stats.blended_additive += 1;
                        }
                        b.mesh.blended.push(BlendedModel {
                            batches: built,
                            origin,
                            additive,
                            alpha,
                        });
                    }
                    ModelVisibility::Translucent { .. } => b.mesh.stats.blended_models_skipped += 1,
                }
            }
            b.mesh.sky_bounds = None;
            b.target = Target::Sky;
            let sky = b.mesh.sky_batches.clone();
            for bt in &sky {
                for k in bt.first_index..bt.first_index + bt.index_count {
                    if let Some(v) = b
                        .mesh
                        .indices
                        .get(k as usize)
                        .and_then(|&i| b.mesh.vertices.get(i as usize))
                    {
                        grow(&mut b.mesh.sky_bounds, v.pos);
                    }
                }
            }
            b.target = Target::Main;
            b.mesh.sky_batches.sort_by_key(|x| x.bucket);
        }
        b.mesh.world_extents = Some((world.extent_min, world.extent_max));
        if b.opts.fog {
            (b.mesh.fog, b.mesh.sky_fog) = crate::fog::world_fog(world);
        }
        b.mesh.two_sided = world_two_sided(world);
        // SPEC: RENDER_2009.md §5.1 step 5 (buckets flushed in index order 0..22)
        b.mesh.batches.sort_by_key(|x| x.bucket);
        b.mesh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_world::{LightPatch, RenderLightGroup};

    fn block_with_group(colour: [f32; 3], words: [u32; 4], data: &[u8]) -> RenderBlock<'_> {
        RenderBlock {
            center: [0.0; 3],
            half: [1.0; 3],
            sections: Vec::new(),
            vertex_count: 0,
            vertices: &[],
            triangle_count: 0,
            triangles: &[],
            sky_portals: Vec::new(),
            occluders: Vec::new(),
            light_groups: vec![RenderLightGroup {
                name: b"g",
                colour,
                data: &[],
                slots: vec![Vec::new(), vec![LightPatch { words, data }]],
            }],
            child_mask: 0,
            children: [0, 0],
        }
    }

    #[test]
    fn light_group_patch_with_run_escape() {
        // 4x2 lightmap, patch at (1, 0) of 3x2: value 100, a run of 2 more 100s, then 0, 50, 200
        let data = [100u8, 0xff, 2, 0, 50, 200];
        let b = block_with_group([1.0, 0.5, 0.0], [1, 0, 3, 2], &data);
        let mut rgba = vec![10u8; 4 * 2 * 4];
        assert_eq!(apply_light_groups(&mut rgba, 4, 2, &b, 1, 1.0), 1);
        let px = |x: usize, y: usize| &rgba[(y * 4 + x) * 4..(y * 4 + x) * 4 + 3];
        // colour (255, 127, 0): add c * v / 255
        assert_eq!(px(0, 0), &[10, 10, 10]); // outside the patch
        assert_eq!(px(1, 0), &[110, 59, 10]);
        assert_eq!(px(3, 0), &[110, 59, 10]); // run repeats 100
        assert_eq!(px(1, 1), &[10, 10, 10]); // value 0
        assert_eq!(px(2, 1), &[60, 34, 10]);
        assert_eq!(px(3, 1), &[210, 109, 10]);
        // other sections and black groups do nothing; out-of-range rectangles are ignored
        assert_eq!(apply_light_groups(&mut rgba, 4, 2, &b, 0, 1.0), 0);
        let black = block_with_group([0.0; 3], [0, 0, 1, 1], &data);
        assert_eq!(apply_light_groups(&mut rgba, 4, 2, &black, 1, 1.0), 0);
        let outside = block_with_group([1.0; 3], [3, 0, 2, 1], &data);
        assert_eq!(apply_light_groups(&mut rgba, 4, 2, &outside, 1, 1.0), 0);
    }

    #[test]
    fn saturates_at_255() {
        let data = [255u8 - 1; 1];
        let b = block_with_group([1.0, 1.0, 1.0], [0, 0, 1, 1], &data);
        let mut rgba = vec![200u8; 4];
        apply_light_groups(&mut rgba, 1, 1, &b, 1, 2.0);
        assert_eq!(&rgba[..3], &[255, 255, 255]);
    }
}
