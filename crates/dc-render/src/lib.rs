//! `dc-render`: the first wgpu renderer slice of the DeCombat client and an offline world viewer.
//!
//! Written from `rust/specs/RENDER_2009.md` (frame sequence, world buckets, lightmap pass and multiply pass,
//! texture slots, camera), `FMT_DAT85.md` (world render data, through `dc-world`) and `FMT_DTX.md` (textures,
//! through `dc-tex`). The ROM model applies: worlds and textures are read from the user's client folder at run
//! time, nothing of it is stored here, and every image this crate writes is a local, never-committed file.
//!
//! Drawn: the opaque world sections (kinds 1, 2, 4, 8, 9 of RENDER_2009.md §5.2) in bucket order 0..22 with the
//! lightmap pass (bucket 14) before the multiply passes (buckets 15 and 20) and the main camera of §4.1 with the
//! [0.1, 1.0] viewport depth split of §4.3 (slice 1); the sky pass of §5.3 with the sky camera of §4.4; models
//! (§7, `FMT_LTB.md` through `dc-model`): rigid and skinned pieces, the three animation trackers blended through
//! the body's blend sets, per-vertex lighting from the light grid and the strongest lights (§6.3); translucent
//! and additive world models sorted back to front (§7.1); eye-attached models (first-person weapon and hands)
//! with the player-view camera of §4.2 and the [0.0, 0.1] depth range of §4.3.
//! Models draw with their render styles (§7.3, the draft FMT_RENDERSTYLE_2009: material, texture stages and
//! raster state per pass, the engine's default style for empty slots), every LOD chosen by distance, and
//! attached objects at sockets.
//! Not drawn yet: morph (vertex-animated) playback, projected shadows (§7.5),
//! dynamic lights on the world (§5.4), sprites and particles (§8), screen glow and post effects (§9), the HUD (§12).
//! Since client-render2: linear world fog from the world settings (§3.3) and the gamma ramp (§2.5) as a final pass.
//!
//! # Public surface
//!
//! What a client (dc-client, the `worldview` bin, the tests) builds a frame from. Everything below is re-exported
//! at the crate root; the modules themselves are private.
//!
//! **Device and frame** (`gpu`):
//! - [`GpuContext::headless`] / [`GpuContext::for_surface`]: adapter, device and queue (no optional features but
//!   the adapter-specific texture format features when the adapter has them, for the MSAA counts beyond 4, and the
//!   profiler's timestamps when asked; downlevel limits raised to the adapter's resolution limits, so WebGL2-class
//!   devices stay possible).
//! - [`WorldRenderer::new`]: every world, sky and translucent pipeline for one colour format, samplers from
//!   [`RenderSettings`], the frame uniforms, and the lazy model pipeline cache ([`ModelPipelines`],
//!   [`WorldRenderer::models`]).
//! - [`WorldRenderer::upload`]: a [`SceneMesh`] to a static [`GpuScene`] (one vertex and one index buffer per map,
//!   one texture per world texture and lightmap, one bind group per material, the world and sky [`Fog`]); dropped
//!   on map change.
//! - [`WorldRenderer::encode_frame_with_view`] (and [`WorldRenderer::encode_frame`] without eye-attached
//!   models): records ONE "world" render pass into the caller's encoder: frame uniform (with the scene's fog),
//!   optional sky pass (sky fog), world batches in bucket order, models, optional translucent world models, then
//!   the eye-attached models in the player-view depth range. [`FrameOptions`] picks sky / translucent / sorting:
//!   the client (dc-client's window and offscreen paths) draws [`FrameOptions::default`] (sky, translucent world
//!   models sorted back to front); [`FrameOptions::opaque_only`] is the first slice's frame, kept for
//!   differential tests. [`FrameTarget`] names the colour and depth views and their size.
//! - [`WorldRenderer::render_frame_offscreen_with_view`] and friends: own targets, submit, blocking read-back to a
//!   [`FrameImage`] (tests and PNG proofs; not a frame-rate path).
//! - [`WorldRenderer::render_frame_to_target`] into an [`OffscreenTarget`] (mapkit-stage2, MAP-27): the same frame
//!   recorded into a colour + depth pair the caller keeps and resizes, submitted without a wait, the colour left on
//!   the GPU as a texture with `TEXTURE_BINDING` ([`OffscreenTarget::view`], [`OffscreenTarget::format`]) for the
//!   dc-mapkit editor's `egui::PaintCallback` viewport (docs/MAPKIT_PLAN.md D7); [`OffscreenTarget::read_back`]
//!   fetches its pixels through the same unpacking as the read-back path.
//!
//! **Fog and gamma** (`fog`, `gamma`): [`world_fog`] reads the world's and the sky pass's linear [`Fog`] from its
//! world-settings object when the scene is built ([`SceneOptions::fog`], on by default); the world shaders apply
//! it per pixel to world sections and models.
//!
//! **Object vocabulary** (mapkit-stage2, MAP-26): every class, property and value name the readers of this crate
//! compare against is a `dc_world::names::Key` matched through `Key::matches`, so a community map written in
//! DeCombat's own vocabulary (docs/MAPKIT_VOCAB.md: `dc.world_settings` with `fog_enable` / `fog_near` / `fog_far` /
//! `fog_color` / `sky_fog_near` / `sky_fog_far`, `dc.global_light` and `dc.point_light` with `color` / `bright` /
//! `to_ambient` / `radius` / `cone` / `lights_objects`, `dc.world_model` with `visible` / `blend` / `alpha`) reads
//! exactly like a ROM world: [`world_fog`], [`global_light`], [`static_light`] / [`world_lights`] and
//! [`model_visibility`]. The ROM-only inputs (inner colour, object bright scale, the fast-lights flag, attenuation
//! coefficients / exponents / kind, hidden / start-hidden) keep their hash-only match; the read order of each reader
//! is unchanged (`static_light`: colour then inner colour; `global_light`: inner colour then colour). A community
//! map's light grid is one cell of its baked ambient colour, which [`AmbientGrid::sample`] returns everywhere (MAP-28). The finished frame (world, models, HUD) goes through the 2009 gamma ramp ([`ramp_for_gamma`],
//! value from [`profile_gamma`], default [`PROFILE_GAMMA_DEFAULT`]): on screen the caller draws into
//! [`GammaStage::view`] and records [`GammaStage::encode`] as the last pass into the screen view (with gamma 1
//! [`GammaStage::active`] is false and the frame goes straight to the screen); offscreen read-backs apply
//! [`apply_ramp`] on the CPU.
//!
//! **Scene build** (`scene`, `bucket`, `vertex`, `lightmap`): [`SceneMesh::build`] flattens DAT85 render blocks
//! into [`GpuVertex`] / u32 indices with one [`Batch`] per drawn section, sorted by bucket ([`select_bucket`],
//! [`PassKind`]); kind-2 sections carry their decoded lightmap ([`decode_lightmap`], [`apply_light_groups`]);
//! world models are split by [`model_visibility`] into merged, sky and [`BlendedModel`] draws. [`SceneStats`] counts
//! what the build kept and dropped, including `sky_portals`: the sky portals collected from the level's top-level
//! render blocks (the loops the sky pass is scissored to; without one the sky pass is skipped), the number a map
//! builder checks after a build (docs/MAPKIT_PLAN.md 5.3 step 4, open point 2).
//!
//! **Models** (`model`, `model_gpu`, `anim`, `character`, `style`): [`ModelFiles`] (LTB through dc-model) ->
//! [`WorldRenderer::upload_model_look`] -> [`ModelInstance`] (its own buffers, textures, bone palette and
//! per-pass bind groups); per frame the caller poses it ([`ModelInstance::set_pose`] with [`AnimSet`] /
//! [`Tracker`], [`ModelInstance::follow`] for a face on a body, [`ModelInstance::attach`] at sockets) and hands
//! `&[&ModelInstance]` to the frame encoder. Render styles ([`RenderStyle`], [`StyleLibrary`]) decide each
//! pass's stages and raster state; [`CharacterTable`] / [`ViewModels`] / [`HandModels`] are attribute tables.
//!
//! **Lighting** (`lights`): [`world_lights`] / [`AmbientGrid`] read once per map; [`LightSet::build`] picks the
//! [`MAX_MODEL_LIGHTS`] strongest lights for one model and folds the rest into its ambient term.
//!
//! **Textures and sources** (`texture`, `source`): [`WorldSource`] / [`TextureIndex`] look entries up in the
//! user's ROM archives at run time; [`decode_dtx`] / [`RomTextures`] decode DTX to RGBA8 mip chains.
//!
//! **Camera and sky** (`camera`, `sky`): [`Camera`] (left-handed view, D3D-style projection, near
//! [`NEAR_Z`]), [`PlayerViewSettings`] for eye-attached models, [`SkySettings`] / [`sky_camera`] /
//! [`portal_screen_rect`].
//!
//! **Viewer** (`viewer`, `window` with the `window` feature): [`RomViewer`] prepares and renders a ROM map
//! offscreen for the `worldview` bin and the ROM tests; [`default_out_dir`] is where their PNGs go.
//!
//! Invariants a caller and any extension keep: world draws stay in bucket order 0..22, a section's lightmap draw
//! (bucket [`LIGHTMAP_BUCKET`]) precedes its multiply draw on the same triangles; the world and normal models
//! use [`WORLD_DEPTH_RANGE`], eye-attached models [`PLAYER_VIEW_DEPTH_RANGE`] in the same pass without a depth
//! clear; no sRGB conversion (RENDER_2009 §2); the gamma ramp is the very last pass, after the HUD.
//!
//! **MSAA** (`msaa`, CLI-1593): [`RenderSettings::msaa`] (1 off; 2, 4, 8) is the sample count every pipeline of a
//! [`WorldRenderer`] is built with ([`WorldRenderer::msaa`], [`multisample_state`]); [`supported_msaa`] lists what
//! the adapter offers for a colour format and the depth format. With it on, the frame loop keeps [`MsaaTargets`]
//! (multisampled colour and depth, made again on resize, `None` when off) and hands their views as the
//! [`FrameTarget`] of every 3D pass; the frame's LAST 3D pass gets [`FrameTarget::resolve`] = the single-sample
//! scene target, resolves the samples there once and discards the multisampled attachments
//! ([`FrameTarget::colour_attachment`] / [`FrameTarget::depth_attachment`] build the attachments for every pass,
//! [`FrameTarget::encode_resolve_only`] is the empty resolving pass of a pass that had nothing to draw). The HUD,
//! the debug panel and the gamma copy stay single-sampled. Changing the count means a new renderer (and a new scene
//! upload and model re-upload: their pipelines come from it); with the count 1 every pass is byte-identical to before.
//! The HUD and the debug overlay are not drawn here: dc-client's `overlay` module records its own passes after
//! [`WorldRenderer::encode_frame_with_view`] (and any effect passes) into the same encoder and target (batched
//! quads over a texture atlas), before the gamma copy.
//!
//! **Frame counters, culling, cached uniforms** (render-core): [`WorldRenderer::encode_frame_with_view`] returns a
//! [`FrameReport`] with what it drew and the command counters (draw calls, bind-group sets, pipeline sets, world
//! draws recorded / batches culled, model uniform bytes written). [`WorldRenderer::upload`] keeps world bounds per
//! batch; per frame the world batches outside the view frustum ([`frustum_planes`], [`box_outside`], 1 unit margin)
//! are not recorded, touching batches with the same recipe and material become one draw, and bind groups are set
//! only when they change ([`WorldRenderer::set_world_culling`] switches the culling, default on). A
//! [`ModelInstance`]'s instance and bone uniforms are written only when their bytes changed, so an instance appears
//! at most once per frame (one uniform buffer each). [`LightSet::build_with`] takes the static lights from an
//! iterator (a caller's light index). [`unpack_rows`] is the shared read-back unpacking (padded rows, BGRA swap).
//!
//! Further invariants (render-core): culling, run merging and skipped re-binds never reorder the drawn batches and
//! never change a pixel (`tests/culling.rs`: every ROM map at nine views byte-identical with culling on and off);
//! nothing here submits during [`WorldRenderer::encode_frame_with_view`] (queue writes: the frame uniforms; the
//! changed model uniforms are one staged upload copied into their buffers in the frame's encoder before the world
//! pass, dx12-perf); new passes or draw kinds count their commands in [`FrameReport`].

mod anim;
mod bucket;
mod camera;
pub mod camo;
mod character;
mod determinism;
mod fog;
mod gamma;
mod gpu;
mod lightmap;
mod lights;
mod model;
mod model_gpu;
mod msaa;
mod scene;
mod sky;
mod source;
mod style;
mod texture;
mod vertex;
mod viewer;
#[cfg(feature = "window")]
mod window;
mod xray;

pub use anim::{
    channels_at, clip_length_ms, decompose_rigid, local_at, mat_of_rows, pos_channel_at,
    quat_from_key, rot_channel_at, AnimSet, Local, QuatOrder, Skeleton, Tracker, KEY_ORDER,
};
pub use bucket::{
    select_bucket, PassKind, RenderSettings, SectionClass, TextureTraits, BUCKET_COUNT,
    LIGHTMAP_BUCKET,
};
pub use camera::{
    box_outside, fov_pair_2009, fov_x_for, fov_y_deg_for_reference_x, fov_y_for, frustum_planes,
    Camera, PlayerViewSettings, DEFAULT_FAR_Z, FAR_Z_MAX, FAR_Z_MIN, FOV_Y_DEFAULT_DEG,
    FULL_WINDOW, NEAR_Z, PLAYER_VIEW_DEFAULT, PLAYER_VIEW_DEPTH_RANGE, WORLD_DEPTH_RANGE,
};
pub use character::{
    fill_decimal, grip_socket, name_key, pv_none_texture, resolve_view_skins, BodyRow,
    CharacterTable, HandModel, HandModels, NameKey, ViewChoices, ViewModel, ViewModels,
    CLIP_LOWER_STAND, CLIP_LOWER_WALK, CLIP_UPPER_STAND, CLIP_UPPER_WALK, MARK_REPEAT,
    PLAYER_CLIPS, WEAPON_SOCKET_INDEX, WEAPON_SOCKET_KEY,
};
pub use determinism::{
    hex, journal_enable, journal_snapshot, mutation, pipeline_settings, render_pipeline, sampler,
    shader_module, texel_hash, Journal, Mutation, TEXEL_CONTRACT,
};
pub use fog::{world_far_z, world_fog, Fog, WORLD_FAR_Z_MIN};
pub use gamma::{
    apply_ramp, gamma_of_profile_text, profile_gamma, profile_key, ramp_for_gamma, GammaStage,
    PROFILE_GAMMA_DEFAULT,
};
pub mod macwin;
pub mod perf;
pub mod warm;
pub use gpu::{
    frame_latency_for, probe_adapters, unpack_rows, AdapterChoice, AdapterProbe, FrameImage,
    FrameOptions, FrameReport, FrameTarget, FrameTimings, GpuContext, GpuScene, OffscreenTarget,
    RenderError, UploadStats, WorldRenderer, CLEAR_COLOUR, OFFSCREEN_FORMAT,
};
pub use lightmap::{decode_lightmap, lightmap_rgba, pack_lightmap, TexelOrder, MAX_LIGHTMAP_BYTES};
pub use lights::{
    dynamic_coefs, global_light, global_light_count, light_class, static_light, world_lights,
    AmbientGrid, Falloff, GlobalLight, GroupGrid, LightSet, LightShape, ModelLight,
    ModelLightSettings, MAX_MODEL_LIGHTS,
};
pub use macwin::{system_cursor_hidden, window_animations_off};
pub use model::{
    child_path, read_rom_entry, rez_entry_of, select_lod, span_weights, ModelFiles, ModelLoadError,
    ModelMesh, ModelMeshStats, ModelVertex, PieceDraw, MAX_BONES,
};
pub use model_gpu::ModelUploader;
pub use model_gpu::{
    ModelInstance, ModelLook, ModelPipelines, PrewarmCount, StyleChoice, StyleStats,
};
pub use msaa::{multisample_state, supported_msaa, MsaaTargets, MSAA_COUNTS};
pub use scene::{
    apply_light_groups, model_visibility, Batch, BlendedModel, LightmapImage, ModelVisibility,
    SceneMesh, SceneModel, SceneOptions, SceneStats,
};
pub use sky::{
    portal_screen_rect, sky_camera, sky_eye, SkyBlock, SkySettings, SkyView, SKY_FAR_Z,
    SKY_MIN_EXTENT, SKY_NEAR, SKY_SCALE,
};
pub use source::{TextureIndex, WorldSource};
pub use style::{
    address, alpha_op, blend_factors, colour_op, compare, cull, depth_mode, describe, filter,
    stage_arg, stage_texture, style_for_slot, style_key, uv_source, Address, BlendFactor, Compare,
    Cull, Filter, Pass, RenderStyle, Stage, StageArg, StageOp, StageTexture, StyleLibrary,
    UvSource, MAX_STYLE_SLOTS,
};
pub use texture::{
    decode_dtx, is_sprite_name, sprite_first_frame, traits_of, RomTextures, TextureFx, TextureImage,
};
pub use vertex::{GpuVertex, RenderVertex};
pub use viewer::{
    default_out_dir, path_hash, screen_rect_of, AnimSpec, CameraSpec, ModelReport, ModelRequest,
    PoseSpec, PreparedView, RomViewer, ViewError, ViewModelReport, ViewModelRequest, ViewReport,
    ViewRequest, WeaponReport, WeaponRequest, CLIP_VIEW_IDLE,
};
#[cfg(feature = "window")]
pub use window::{run_window, WindowOptions};
pub use xray::{
    project_px, push_model_quads, skeleton_world, QuadVertex, XrayMode, XrayModel, BOX_BORDER_PX,
    BOX_FILL_ALPHA_DIV, CHAMS_FACING, JOINT_PX, LINE_PX,
};
