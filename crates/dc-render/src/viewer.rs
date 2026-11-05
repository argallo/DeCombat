//! Offline world viewer: one map of the user's ROM, one camera, one frame; optionally one model standing at a
//! spawn point (with its render styles, the world's global light, the light groups' ambient, and an attached
//! weapon), the sky pass and the translucent world models.

use crate::anim::Tracker;
use crate::bucket::RenderSettings;
use crate::camera::{fov_y_for, Camera, PlayerViewSettings};
use crate::character::{
    grip_socket, name_key, CharacterTable, NameKey, CLIP_LOWER_STAND, CLIP_LOWER_WALK,
    CLIP_UPPER_STAND, CLIP_UPPER_WALK,
};
use crate::gpu::{
    AdapterChoice, FrameImage, FrameOptions, FrameReport, FrameTimings, GpuContext, GpuScene,
    RenderError, UploadStats, WorldRenderer, OFFSCREEN_FORMAT,
};
use crate::lights::{
    global_light, world_lights, AmbientGrid, LightSet, ModelLight, ModelLightSettings,
};
use crate::model::{ModelFiles, ModelMeshStats};
use crate::model_gpu::{ModelInstance, ModelLook, StyleChoice, StyleStats};
use crate::scene::{SceneMesh, SceneOptions, SceneStats};
use crate::source::{TextureIndex, WorldSource};
use crate::style::{describe, RenderStyle, StyleLibrary};
use crate::texture::RomTextures;
use dc_rez::RezVfs;
use dc_world::{ObjectRole, World};
use glam::{Quat, Vec3, Vec4};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug)]
pub enum ViewError {
    Rom(String),
    NoSuchMap(String),
    World(String),
    NoCamera(String),
    Model(String),
    Gpu(RenderError),
}

impl fmt::Display for ViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViewError::Rom(s) => write!(f, "ROM: {s}"),
            ViewError::NoSuchMap(s) => write!(f, "no world named {s} in the ROM"),
            ViewError::World(s) => write!(f, "world: {s}"),
            ViewError::NoCamera(s) => write!(f, "camera: {s}"),
            ViewError::Model(s) => write!(f, "model: {s}"),
            ViewError::Gpu(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ViewError {}

impl From<RenderError> for ViewError {
    fn from(e: RenderError) -> Self {
        ViewError::Gpu(e)
    }
}

/// Where the camera goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CameraSpec {
    /// The n-th player spawn point of the world (object order), at its stored position and orientation.
    Spawn(usize),
    /// Explicit eye and yaw / pitch in degrees.
    Explicit {
        eye: [f32; 3],
        yaw_deg: f32,
        pitch_deg: f32,
    },
}

/// Which clips a viewed model plays.
#[derive(Debug, Clone, PartialEq)]
pub enum PoseSpec {
    /// Combined clip indices: one (main tracker) or three (main, tracker 1 with weight set 0, tracker 2 with
    /// weight set 1).
    Clips(Vec<usize>),
    /// The player pose the game server streams: main tracker clip 0, tracker 1 the upper-body clip, tracker 2
    /// the lower-body clip, standing or walking forward, found by their name keys.
    // SPEC: SESSION_B_S2C_addendum_dserver3.md S3-5 (main 0; upper / lower stand and walk clip keys)
    Player { walk: bool },
}

/// The pose of a viewed model (SESSION_B_S2C_addendum_dserver3.md S3-5 tracker layout).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimSpec {
    pub pose: PoseSpec,
    /// Point in each clip, as a fraction of its length (0 = first key, 0.5 = halfway).
    pub phase: f32,
}

impl AnimSpec {
    /// The combined clip indices of the pose for a model, `None` when a player clip is missing.
    pub fn clip_indices(&self, m: &ModelInstance) -> Option<Vec<usize>> {
        match &self.pose {
            PoseSpec::Clips(c) => Some(c.clone()),
            PoseSpec::Player { walk } => {
                let (up, low) = if *walk {
                    (CLIP_UPPER_WALK, CLIP_LOWER_WALK)
                } else {
                    (CLIP_UPPER_STAND, CLIP_LOWER_STAND)
                };
                let find = |k| m.anim.find_clip(|n| name_key(n) == k);
                Some(vec![0, find(up)?, find(low)?])
            }
        }
    }

    /// The trackers of this pose for a model (times from each clip's length).
    pub fn trackers(&self, m: &ModelInstance) -> Vec<Tracker> {
        self.clip_indices(m)
            .unwrap_or_default()
            .iter()
            .enumerate()
            .take(3)
            .map(|(k, &clip)| {
                let len = m.anim.clip_length_ms(clip).unwrap_or(0) as f32;
                Tracker {
                    clip,
                    time_ms: len * self.phase.clamp(0.0, 1.0),
                    blend_set: if k == 0 { None } else { Some(k - 1) },
                }
            })
            .collect()
    }
}

/// A model to place in the view.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    /// Resource path of the model file in the ROM (`<archive>\...`).
    pub path: String,
    /// Texture list in material-slot order; `None`: the character table's list when the path is a body row's
    /// model file (skins of variant 0, team mark 0), else none (the missing-texture checker).
    pub textures: Option<Vec<String>>,
    pub anim: Option<AnimSpec>,
    /// Spawn point the model stands at (its position and yaw).
    pub spawn: usize,
    /// A world position of the model's origin and its yaw (radians) instead of the spawn point (viewer choice).
    pub place: Option<([f32; 3], f32)>,
    /// Stand the model's bind-pose feet on the floor below the spawn point (viewer choice).
    pub drop_to_floor: bool,
    /// When the camera is not given explicitly: distance of the camera in front of the model, looking at it.
    pub camera_distance: f32,
    /// Further model files drawn in the same object and posed with this one (a character's face model):
    /// `(path, texture list)`, see [`ModelInstance::follow`].
    pub attachments: Vec<(String, Vec<String>)>,
    /// The global directional light `(direction it travels, colour 0..255)` the server sends
    /// (`S2C_DIR_LIGHT`); when not given and [`ModelRequest::world_sun`] is on, the world's global light object.
    pub sun: Option<([f32; 3], [f32; 3])>,
    /// Use the world's global light object as the sun when `sun` is not given (default on).
    pub world_sun: bool,
    /// Render-style list of the main model (ROM paths, style-slot order); `None`: the character table's list
    /// when the path is a body or face row, else none (every piece with the engine default style). The face
    /// models of `attachments` take their lists from the table.
    pub styles: Option<Vec<String>>,
    /// Draw every model with the plain state instead of render styles (the comparison "without styles").
    pub plain: bool,
    /// The held weapon, attached at a socket of the main model.
    pub weapon: Option<WeaponRequest>,
    /// LOD distance for every model (`None`: the distance from the eye to the model).
    pub lod_distance: Option<f32>,
}

/// A weapon (or other object) attached at a socket of the viewed model.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponRequest {
    /// Resource path of the model file.
    pub path: String,
    pub textures: Vec<String>,
    pub styles: Vec<String>,
    /// Socket index on the main model; `None`: the weapon socket ([`crate::grip_socket`]).
    pub socket: Option<usize>,
}

impl ModelRequest {
    pub fn new(path: &str) -> ModelRequest {
        ModelRequest {
            path: path.to_string(),
            textures: None,
            anim: None,
            spawn: 0,
            place: None,
            drop_to_floor: true,
            camera_distance: 150.0,
            attachments: Vec::new(),
            sun: None,
            world_sun: true,
            styles: None,
            plain: false,
            weapon: None,
            lod_distance: None,
        }
    }
}

/// The idle clip of a first-person model (SHA-256 key of the lower-cased clip name).
// OBSERVED: the first-person models of the ROM weapon rows borrow a clip of this name key from their child model
// (2026-10-25, weapons 0, 7, 145, 164; the same key dc-client's weapon state plays when idle); the name is not stored
pub const CLIP_VIEW_IDLE: NameKey = ([0xae, 0x0e, 0x0c, 0xad, 0x52, 0x76, 0x31, 0xa6], 6);

/// A first-person (eye-attached) model drawn with the player-view camera (RENDER_2009.md §4.2).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewModelRequest {
    /// Resource path of the model file.
    pub path: String,
    pub textures: Vec<String>,
    /// Render-style list in style-slot order (empty slots: the engine default style).
    pub styles: Vec<String>,
    /// Where the model's origin sits in eye space (+X right, +Y up, +Z forward), world units.
    pub offset: [f32; 3],
    /// Combined clip index played on the main tracker; `None`: the idle clip ([`CLIP_VIEW_IDLE`]), else clip 0.
    pub clip: Option<usize>,
    /// Point in the clip as a fraction of its length.
    pub phase: f32,
}

impl ViewModelRequest {
    pub fn new(path: &str) -> ViewModelRequest {
        ViewModelRequest {
            path: path.to_string(),
            textures: Vec::new(),
            styles: Vec::new(),
            offset: [0.0; 3],
            clip: None,
            phase: 0.0,
        }
    }
}

/// What the first-person model of a view did.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewModelReport {
    pub path_hash: String,
    pub clip: Option<usize>,
    /// Eye-space bounds of the posed model at its offset.
    pub eye_bounds: Option<([f32; 3], [f32; 3])>,
    /// Screen rectangle `[x0, y0, x1, y1]` (pixels, y down) of those bounds through the player-view projection,
    /// clamped to the target (corners nearer than the near plane moved onto it); `None` when off screen.
    pub screen_rect: Option<[u32; 4]>,
    pub pieces: u64,
    pub missing_textures: u64,
    pub styles: Vec<String>,
    pub ambient: [f32; 3],
    pub lights: usize,
}

/// One viewer request.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewRequest {
    pub map: String,
    pub camera: CameraSpec,
    /// Added to the camera height (world units). 0 keeps the spawn point's own height (OBSERVED: the first
    /// spawn of each ROM map sits 46..322 units, typically ~110, above the render geometry below it).
    pub eye_lift: f32,
    /// Overrides the spawn orientation's yaw / pitch (degrees) while keeping the spawn position.
    pub look_override: Option<(f32, f32)>,
    /// Horizontal field of view in degrees; the vertical one follows from the target aspect (viewer choice:
    /// the game sets both, RENDER_2009.md §3.3).
    pub fov_x_deg: f32,
    pub width: u32,
    pub height: u32,
    pub scene: SceneOptions,
    pub generate_mips: bool,
    /// Sky and translucent passes of the frame.
    pub frame: FrameOptions,
    pub model: Option<ModelRequest>,
    /// A first-person model at the eye (player-view pass).
    pub view_model: Option<ViewModelRequest>,
    /// Player-view camera settings.
    pub player_view: PlayerViewSettings,
    /// Take the far plane from the world's settings ([`crate::world_far_z`]) when it has one, as the game does;
    /// `false` keeps `scene.settings.far_z` (worldview `--far`).
    // DC-DEFAULT: CLI-1851 (proposed)
    pub world_far: bool,
}

impl ViewRequest {
    pub fn new(map: &str) -> ViewRequest {
        ViewRequest {
            map: map.to_string(),
            camera: CameraSpec::Spawn(0),
            eye_lift: 0.0,
            look_override: None,
            fov_x_deg: 90.0,
            width: 1024,
            height: 768,
            scene: SceneOptions::default(),
            generate_mips: false,
            frame: FrameOptions::default(),
            model: None,
            view_model: None,
            player_view: PlayerViewSettings::default(),
            world_far: true,
        }
    }
}

/// What the model part of a view did.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelReport {
    /// First 12 hex digits of SHA-256 of the upper-cased path (no ROM name in reports).
    pub path_hash: String,
    pub mesh: ModelMeshStats,
    pub clips: usize,
    pub textures: usize,
    pub missing_textures: u64,
    pub position: [f32; 3],
    pub ambient: [f32; 3],
    pub lights: usize,
    /// Per light used: (distance to the model centre, radius, largest channel of its contribution there).
    pub light_detail: Vec<(f32, f32, f32)>,
    pub static_lights_in_world: usize,
    pub trackers: Vec<Tracker>,
    pub load_ms: f64,
    pub upload_ms: f64,
    pub pose_ms: f64,
    /// Per style slot of the main model: stage-0 op, passes, lighting (or "missing").
    pub styles: Vec<String>,
    pub style_stats: StyleStats,
    pub plain: bool,
    /// The sun used: (direction, colour, convert-to-ambient), and whether it came from the world.
    pub sun: Option<([f32; 3], [f32; 3], f32)>,
    pub sun_from_world: bool,
    /// What the light-group grids added to the ambient at the model centre.
    pub group_ambient: [f32; 3],
    /// (piece, LOD) of the main model at the view's eye, and the triangles that draws.
    pub lods: Vec<(usize, usize)>,
    pub triangles_drawn: u64,
    pub weapon: Option<WeaponReport>,
}

/// Where the attached weapon went.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponReport {
    pub path_hash: String,
    pub socket: usize,
    /// Node of the socket on the main model.
    pub node: u32,
    /// World position of the socket (= the weapon's origin).
    pub socket_at: [f32; 3],
    /// World bounds of the main model's vertices whose largest weight is on the socket's node.
    pub node_bounds: Option<([f32; 3], [f32; 3])>,
    /// World bounds of the weapon model in its pose.
    pub weapon_bounds: Option<([f32; 3], [f32; 3])>,
    pub pieces: u64,
    pub missing_textures: u64,
    pub styles: Vec<String>,
}

/// What a view produced besides the image.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewReport {
    pub map: String,
    pub adapter: String,
    pub eye: [f32; 3],
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub timings: FrameTimings,
    pub scene: SceneStats,
    pub upload: UploadStats,
    pub textures_missing: u64,
    pub textures_unreadable: u64,
    pub file_bytes: usize,
    pub frame: FrameReport,
    pub model: Option<ModelReport>,
    pub view_model: Option<ViewModelReport>,
}

/// A prepared map: uploaded scene plus its camera (window mode keeps re-drawing it).
pub struct PreparedView {
    pub scene: GpuScene,
    pub camera: Camera,
    pub report: ViewReport,
    pub frame: FrameOptions,
    pub models: Vec<ModelInstance>,
    /// Eye-attached models (drawn with the player-view camera).
    pub view_models: Vec<ModelInstance>,
    pub player_view: PlayerViewSettings,
}

impl PreparedView {
    pub fn model_refs(&self) -> Vec<&ModelInstance> {
        self.models.iter().collect()
    }

    pub fn view_model_refs(&self) -> Vec<&ModelInstance> {
        self.view_models.iter().collect()
    }
}

/// The user's ROM plus a device.
pub struct RomViewer {
    worlds: WorldSource,
    textures: TextureIndex,
    vfs: RezVfs,
    pub ctx: GpuContext,
    pub renderer: WorldRenderer,
}

fn yaw_pitch_of(c: &Camera) -> (f32, f32) {
    let f = c.forward();
    let yaw = f.x.atan2(f.z).to_degrees();
    let pitch = (-f.y).clamp(-1.0, 1.0).asin().to_degrees();
    (yaw, pitch)
}

/// First 12 hex digits of SHA-256 of an upper-cased resource path.
pub fn path_hash(path: &str) -> String {
    let h = dc_rom::sha256(
        path.trim()
            .replace('/', "\\")
            .to_ascii_uppercase()
            .as_bytes(),
    );
    h[..6].iter().map(|b| format!("{b:02x}")).collect()
}

impl RomViewer {
    pub fn open(
        rom: &Path,
        adapter: AdapterChoice,
        settings: RenderSettings,
    ) -> Result<RomViewer, ViewError> {
        let ctx = GpuContext::headless(adapter)?;
        RomViewer::with_context(rom, ctx, OFFSCREEN_FORMAT, settings)
    }

    pub fn with_context(
        rom: &Path,
        ctx: GpuContext,
        format: wgpu::TextureFormat,
        settings: RenderSettings,
    ) -> Result<RomViewer, ViewError> {
        let worlds = WorldSource::open(rom).map_err(|e| ViewError::Rom(e.to_string()))?;
        let textures = TextureIndex::open(rom).map_err(|e| ViewError::Rom(e.to_string()))?;
        let renderer = WorldRenderer::new(&ctx, format, settings);
        Ok(RomViewer {
            worlds,
            textures,
            vfs: RezVfs::new(rom),
            ctx,
            renderer,
        })
    }

    /// Map names read from the user's world archive.
    pub fn map_names(&self) -> Vec<String> {
        self.worlds.map_names()
    }

    /// The archive VFS of the user's ROM.
    pub fn vfs(&self) -> &RezVfs {
        &self.vfs
    }

    /// The texture index over every archive of the user's ROM.
    pub fn texture_index(&self) -> &TextureIndex {
        &self.textures
    }

    /// Loads and uploads a model file with a texture list (names resolved in the ROM; unresolved names and a
    /// short list give the missing-texture checker), drawn with the plain state (no render styles).
    pub fn load_model(
        &self,
        path: &str,
        textures: &[String],
    ) -> Result<(ModelInstance, f64, f64), ViewError> {
        self.load_model_styled(path, textures, None)
            .map(|(i, _, a, b)| (i, a, b))
    }

    /// Loads and uploads a model file with a texture list and a render-style list (`None`: the plain state;
    /// `Some(list)`: the styles at those ROM paths, missing or unloadable ones replaced by the engine default).
    /// Also returns the parsed files (the socket table of a body).
    pub fn load_model_styled(
        &self,
        path: &str,
        textures: &[String],
        styles: Option<&[Option<Arc<RenderStyle>>]>,
    ) -> Result<(ModelInstance, ModelFiles, f64, f64), ViewError> {
        let t = Instant::now();
        let files =
            ModelFiles::load(&self.vfs, path).map_err(|e| ViewError::Model(e.to_string()))?;
        let mut rom_tex = RomTextures::new(&self.textures);
        let images: Vec<_> = textures
            .iter()
            .map(|n| rom_tex.image(n.as_bytes()))
            .collect();
        let load_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let look = ModelLook {
            textures: &images,
            styles: match styles {
                Some(l) => StyleChoice::List(l),
                None => StyleChoice::Plain,
            },
        };
        let inst = self.renderer.upload_model_look(&self.ctx, &files, look);
        self.ctx.queue.submit(std::iter::empty());
        let _ = self.ctx.device.poll(wgpu::PollType::Wait);
        Ok((inst, files, load_ms, t.elapsed().as_secs_f64() * 1000.0))
    }

    /// Loads, builds and uploads a map and places the camera (and the model, when one is requested).
    pub fn prepare(&self, req: &ViewRequest) -> Result<PreparedView, ViewError> {
        let data = self
            .worlds
            .read(&req.map)
            .map_err(|e| ViewError::Rom(e.to_string()))?
            .ok_or_else(|| ViewError::NoSuchMap(req.map.clone()))?;
        let file_bytes = data.len();
        let t = Instant::now();
        let world = World::parse(&data).map_err(|e| ViewError::World(e.to_string()))?;
        let parse_ms = t.elapsed().as_secs_f64() * 1000.0;
        // DC-DEFAULT: CLI-1851 (proposed) the world's far plane, else the settings' one
        let far_z = req
            .world_far
            .then(|| crate::fog::world_far_z(&world))
            .flatten()
            .unwrap_or(req.scene.settings.far_z);

        let fov_x = req.fov_x_deg.clamp(1.0, 170.0).to_radians();
        let fov_y = fov_y_for(fov_x, req.width, req.height);
        let spawn_of = |n: usize| -> Result<(Vec3, [f32; 3]), ViewError> {
            let o = world
                .objects_with_role(ObjectRole::PlayerSpawn)
                .nth(n)
                .ok_or_else(|| ViewError::NoCamera(format!("map has no player spawn #{n}")))?;
            let pos = o
                .position()
                .ok_or_else(|| ViewError::NoCamera("spawn without position".into()))?;
            Ok((Vec3::from(pos), o.rotation().unwrap_or([0.0; 3])))
        };
        let mut camera = match req.camera {
            CameraSpec::Explicit {
                eye,
                yaw_deg,
                pitch_deg,
            } => Camera::looking(
                Vec3::from(eye),
                yaw_deg.to_radians(),
                pitch_deg.to_radians(),
                fov_x,
                fov_y,
            ),
            CameraSpec::Spawn(n) => {
                let (pos, rot) = spawn_of(n)?;
                Camera::from_euler(pos, rot, fov_x, fov_y)
            }
        };
        if let Some((yaw, pitch)) = req.look_override {
            camera = Camera::looking(
                camera.pos,
                yaw.to_radians(),
                pitch.to_radians(),
                fov_x,
                fov_y,
            );
        }
        camera.pos.y += req.eye_lift;
        camera.far_z = far_z;

        let mut rom_tex = RomTextures::new(&self.textures);
        let t = Instant::now();
        let mesh = SceneMesh::build(&world, req.scene, &mut |name| rom_tex.traits(name));
        let build_ms = t.elapsed().as_secs_f64() * 1000.0;

        // the model: load, place at the spawn, light from the world, pose
        let mut models = Vec::new();
        let mut model_report = None;
        if let Some(mr) = &req.model {
            let table = CharacterTable::from_vfs(&self.vfs).ok();
            let textures = match &mr.textures {
                Some(t) => t.clone(),
                None => table
                    .as_ref()
                    .and_then(|t| {
                        let (row, _) = t.row_of_model(&mr.path)?;
                        t.body_textures(row, 0, 0)
                    })
                    .unwrap_or_default(),
            };
            // SPEC: UDP_SESSION_2009_B.md §3.5.1 (a character's style list = the row's RenderStyle0..; the face
            // model's = the face row's); EVIDENCE: engine+0x1b9390 (missing slots: the engine default style)
            let mut library = StyleLibrary::new();
            let style_names_of = |path: &str| -> Vec<String> {
                table
                    .as_ref()
                    .and_then(|t| t.styles_of_model(path))
                    .unwrap_or_default()
            };
            let main_names = mr
                .styles
                .clone()
                .unwrap_or_else(|| style_names_of(&mr.path));
            let main_styles = library.list(&self.vfs, &main_names);
            let styles_arg =
                |l: &[Option<Arc<RenderStyle>>]| -> Option<Vec<Option<Arc<RenderStyle>>>> {
                    (!mr.plain).then(|| l.to_vec())
                };
            let main_list = styles_arg(&main_styles);
            let (mut inst, body_files, load_ms, upload_ms) =
                self.load_model_styled(&mr.path, &textures, main_list.as_deref())?;
            inst.lod_distance = mr.lod_distance;
            let (spawn_pos, spawn_rot) = match mr.place {
                Some((p, yaw)) => (Vec3::from(p), [0.0, yaw, 0.0]),
                None => spawn_of(mr.spawn)?,
            };
            // yaw only: a character stands upright (the spawn euler is (0, yaw, 0) in the ROM, RENDER_2009.md
            // §4.1 step 2)
            inst.rotation = Quat::from_rotation_y(spawn_rot[1]);
            inst.position = spawn_pos;
            let t = Instant::now();
            if let Some(a) = &mr.anim {
                let tr = a.trackers(&inst);
                inst.set_pose(&tr);
            }
            let pose_ms = t.elapsed().as_secs_f64() * 1000.0;
            if mr.drop_to_floor {
                if let (Some(floor), Some((lo, _))) = (
                    mesh.floor_below(spawn_pos.to_array(), 2000.0),
                    inst.posed_bounds(),
                ) {
                    inst.position.y = floor - lo.y;
                }
            }
            let lighting = WorldLighting::of(&world, req.scene.light_groups, mr.sun, mr.world_sun);
            let (grid, statics, static_count) =
                (&lighting.grid, &lighting.statics, lighting.static_count);
            let (sun, sun_from_world) = (lighting.sun, lighting.sun_from_world);
            let centre = inst
                .bounds_in_world()
                .map(|(a, b)| (a + b) * 0.5)
                .unwrap_or(inst.position);
            let group_ambient = grid.group_part(centre).to_array();
            inst.lights = LightSet::build(
                centre,
                inst.radius,
                Some(grid),
                statics,
                &[],
                &ModelLightSettings::default(),
            );
            if matches!(req.camera, CameraSpec::Spawn(_)) && req.look_override.is_none() {
                // look at the model from in front of it (the spawn faces into the level)
                let fwd = inst.rotation * Vec3::Z;
                let eye = centre + fwd * mr.camera_distance + Vec3::Y * (req.eye_lift + 10.0);
                let d = centre - eye;
                let yaw = d.x.atan2(d.z);
                let pitch = (-d.y).atan2((d.x * d.x + d.z * d.z).sqrt());
                camera = Camera::looking(eye, yaw, pitch, fov_x, fov_y);
                camera.far_z = far_z;
            }
            // the held weapon: at the body socket, posed with its main tracker at clip 0, time 0
            // SPEC: UDP_SESSION_2009_B.md §3.5.1 (weapon child: one tracker anim 0, time 0; attached at socket 2,
            // offset 0, rotation identity)
            let mut weapon = None;
            let mut weapon_report = None;
            if let Some(w) = &mr.weapon {
                let wstyles = library.list(&self.vfs, &w.styles);
                let wlist = styles_arg(&wstyles);
                let (mut wi, _, _, _) =
                    self.load_model_styled(&w.path, &w.textures, wlist.as_deref())?;
                wi.lod_distance = mr.lod_distance;
                if wi.anim.clip_count() > 0 {
                    wi.set_pose(&[Tracker::main(0, 0.0)]);
                }
                let socket = w.socket.or_else(|| grip_socket(&body_files.body));
                let placed = socket.is_some_and(|s| {
                    inst.attach(&body_files.body, s, Vec3::ZERO, Quat::IDENTITY, &mut wi)
                });
                if !placed {
                    return Err(ViewError::Model(format!(
                        "the model has no socket {socket:?} for the weapon"
                    )));
                }
                let s = socket.unwrap_or(0);
                let node = body_files.body.sockets[s].node;
                let world_m = inst.world();
                let to_world = |b: Option<(Vec3, Vec3)>| {
                    b.map(|(lo, hi)| {
                        let (a, c) = corners_bounds(world_m, lo, hi);
                        (a.to_array(), c.to_array())
                    })
                };
                weapon_report = Some(WeaponReport {
                    path_hash: path_hash(&w.path),
                    socket: s,
                    node,
                    socket_at: wi.position.to_array(),
                    node_bounds: to_world(inst.node_bounds(node as usize)),
                    weapon_bounds: wi
                        .bounds_in_world()
                        .map(|(a, b)| (a.to_array(), b.to_array())),
                    pieces: wi.mesh_stats.pieces,
                    missing_textures: wi.missing_textures,
                    styles: describe(&wstyles),
                });
                weapon = Some(wi);
            }
            model_report = Some(ModelReport {
                path_hash: path_hash(&mr.path),
                mesh: inst.mesh_stats,
                clips: inst.anim.clip_count(),
                textures: textures.len(),
                missing_textures: inst.missing_textures,
                position: inst.position.to_array(),
                ambient: inst.lights.ambient.to_array(),
                lights: inst.lights.lights.len(),
                light_detail: inst
                    .lights
                    .lights
                    .iter()
                    .map(|l| {
                        (
                            l.pos.distance(centre),
                            l.radius,
                            l.contribution(centre).max_element(),
                        )
                    })
                    .collect(),
                static_lights_in_world: static_count,
                trackers: inst.trackers.clone(),
                load_ms,
                upload_ms,
                pose_ms,
                styles: describe(&main_styles),
                style_stats: inst.style_stats.clone(),
                plain: mr.plain,
                sun,
                sun_from_world,
                group_ambient,
                lods: inst.select_lods(camera.pos),
                triangles_drawn: inst.triangles_at(camera.pos),
                weapon: weapon_report,
            });
            for (path, tex) in &mr.attachments {
                let names = style_names_of(path);
                let list = library.list(&self.vfs, &names);
                let (mut a, _, _, _) =
                    self.load_model_styled(path, tex, styles_arg(&list).as_deref())?;
                a.lod_distance = mr.lod_distance;
                a.follow(&inst);
                models.push(a);
            }
            models.insert(0, inst);
            models.extend(weapon);
        }
        // the first-person model: in eye space at its offset, posed, lit at the eye like the world's models
        let mut view_models = Vec::new();
        let mut view_report = None;
        if let Some(vr) = &req.view_model {
            let mut library = StyleLibrary::new();
            let styles = library.list(&self.vfs, &vr.styles);
            let (mut vi, _, _, _) =
                self.load_model_styled(&vr.path, &vr.textures, Some(&styles))?;
            let clip = vr.clip.or_else(|| {
                vi.anim
                    .find_clip(|n| name_key(&n.to_ascii_lowercase()) == CLIP_VIEW_IDLE)
                    .or((vi.anim.clip_count() > 0).then_some(0))
            });
            if let Some(c) = clip {
                let len = vi.anim.clip_length_ms(c).unwrap_or(0) as f32;
                vi.set_pose(&[Tracker::main(c, len * vr.phase.clamp(0.0, 1.0))]);
            }
            vi.position = Vec3::from(vr.offset);
            vi.rotation = Quat::IDENTITY;
            // SPEC: RENDER_2009.md §6.3 (light grid + world lights at the model), evaluated at the eye
            // DC-DEFAULT: REN-13 (proposed), see DC_DEFAULTS.md (an eye-attached model is lit at the eye position)
            let lighting = match &req.model {
                Some(mr) => WorldLighting::of(&world, req.scene.light_groups, mr.sun, mr.world_sun),
                None => WorldLighting::of(&world, req.scene.light_groups, None, true),
            };
            vi.lights = LightSet::build(
                camera.pos,
                vi.radius,
                Some(&lighting.grid),
                &lighting.statics,
                &[],
                &ModelLightSettings::default(),
            );
            let eye_bounds = vi.bounds_in_world();
            view_report = Some(ViewModelReport {
                path_hash: path_hash(&vr.path),
                clip,
                eye_bounds: eye_bounds.map(|(a, b)| (a.to_array(), b.to_array())),
                screen_rect: eye_bounds.and_then(|(lo, hi)| {
                    screen_rect_of(
                        req.player_view.projection(req.width, req.height),
                        req.player_view.near,
                        lo,
                        hi,
                        req.width,
                        req.height,
                    )
                }),
                pieces: vi.mesh_stats.pieces,
                missing_textures: vi.missing_textures,
                styles: describe(&styles),
                ambient: vi.lights.ambient.to_array(),
                lights: vi.lights.lights.len(),
            });
            view_models.push(vi);
        }
        drop(world);
        drop(data);
        let gen = req.generate_mips;
        let scene = self.renderer.upload(&self.ctx, &mesh, &mut |name| {
            let mut img = rom_tex.image(name);
            rom_tex.release(name);
            if gen {
                if let Some(i) = img.as_mut() {
                    i.generate_mips();
                }
            }
            img
        });
        let (yaw_deg, pitch_deg) = yaw_pitch_of(&camera);
        let report = ViewReport {
            map: req.map.to_ascii_uppercase(),
            adapter: self.ctx.describe(),
            eye: camera.pos.to_array(),
            yaw_deg,
            pitch_deg,
            timings: FrameTimings {
                parse_ms,
                scene_build_ms: build_ms,
                texture_decode_ms: scene.stats.decode_ms,
                upload_ms: scene.stats.upload_ms,
                gpu_frame_ms: 0.0,
            },
            scene: mesh.stats,
            upload: scene.stats,
            textures_missing: rom_tex.missing,
            textures_unreadable: rom_tex.unreadable,
            file_bytes,
            frame: FrameReport::default(),
            model: model_report,
            view_model: view_report,
        };
        Ok(PreparedView {
            scene,
            camera,
            report,
            frame: req.frame,
            models,
            view_models,
            player_view: req.player_view,
        })
    }

    /// Renders a prepared view offscreen.
    pub fn render_prepared(
        &self,
        p: &PreparedView,
        width: u32,
        height: u32,
    ) -> Result<(FrameImage, FrameReport), ViewError> {
        Ok(self.renderer.render_frame_offscreen_with_view(
            &self.ctx,
            &p.scene,
            &p.camera,
            &p.frame,
            &p.model_refs(),
            &p.view_model_refs(),
            &p.player_view,
            width,
            height,
        )?)
    }

    /// Prepares the map and renders one frame offscreen.
    pub fn render(&self, req: &ViewRequest) -> Result<(FrameImage, ViewReport), ViewError> {
        let p = self.prepare(req)?;
        let (img, fr) = self.render_prepared(&p, req.width, req.height)?;
        let mut report = p.report;
        report.timings.gpu_frame_ms = fr.gpu_ms;
        report.frame = fr;
        Ok((img, report))
    }
}

impl ViewReport {
    /// One-paragraph text summary (map name from the user's ROM; printed, never stored in the tree).
    pub fn summary(&self) -> String {
        let t = &self.timings;
        let s = &self.scene;
        let u = &self.upload;
        let mut out = format!(
            "{map}: adapter {ad}\n  eye ({:.1}, {:.1}, {:.1}) yaw {:.1} pitch {:.1}\n  \
             parse {:.1} ms, scene build {:.1} ms, texture read+decode {:.1} ms, upload {:.1} ms, gpu frame {:.2} ms\n  \
             blocks {} sections {} drawn {} hidden {} approximated {} triangles {} world models {} (hidden {})\n  \
             lightmaps {} (bad {}, light-group patches {}), textures {} missing {} unreadable {}, texture MB {:.1}, lightmap MB {:.1}, draws {}, bind groups {}\n  \
             sky: models {} sections {} portals {} draws {} rect {:?} eye {:?}; translucent: models {} (additive {}) sections {} (kind-2 dropped {}) drawn {}",
            self.eye[0],
            self.eye[1],
            self.eye[2],
            self.yaw_deg,
            self.pitch_deg,
            t.parse_ms,
            t.scene_build_ms,
            t.texture_decode_ms,
            t.upload_ms,
            t.gpu_frame_ms,
            s.blocks,
            s.sections,
            s.sections_drawn,
            s.sections_hidden,
            s.sections_approximated,
            s.triangles_drawn,
            s.world_models,
            s.hidden_models_skipped,
            s.lightmaps,
            s.lightmaps_bad,
            s.light_group_patches,
            u.textures,
            u.missing_textures,
            self.textures_unreadable,
            u.texture_bytes as f64 / 1048576.0,
            u.lightmap_bytes as f64 / 1048576.0,
            u.draws,
            u.bind_groups,
            s.sky_models,
            s.sky_sections,
            s.sky_portals,
            self.frame.sky_draws,
            self.frame.sky_rect,
            self.frame.sky_eye.map(|e| e.map(|v| v.round())),
            s.blended_models,
            s.blended_additive,
            s.blended_sections,
            s.blended_lightmap_sections_dropped,
            self.frame.blended_objects,
            map = self.map,
            ad = self.adapter,
        );
        if let Some(m) = &self.model {
            let tr: Vec<String> = m
                .trackers
                .iter()
                .map(|t| format!("{}@{:.0}ms/ws{:?}", t.clip, t.time_ms, t.blend_set))
                .collect();
            out.push_str(&format!(
                "\n  model {}: pieces {} (rigid {} skinned {} morph {} skipped {}), vertices {} triangles {}, clips {}, \
                 textures {} (missing {}), at ({:.1}, {:.1}, {:.1}), ambient ({:.0}, {:.0}, {:.0}), lights {} of {} static {:?}, \
                 trackers [{}]; load {:.1} ms, upload {:.1} ms, pose {:.2} ms",
                m.path_hash,
                m.mesh.pieces,
                m.mesh.rigid,
                m.mesh.skinned,
                m.mesh.morph,
                m.mesh.skipped,
                m.mesh.vertices,
                m.mesh.triangles,
                m.clips,
                m.textures,
                m.missing_textures,
                m.position[0],
                m.position[1],
                m.position[2],
                m.ambient[0],
                m.ambient[1],
                m.ambient[2],
                m.lights,
                m.static_lights_in_world,
                m.light_detail
                    .iter()
                    .map(|(d, r, c)| {
                        if *r > 1e30 {
                            format!("directional c {c:.0}")
                        } else {
                            format!("d {d:.0} r {r:.0} c {c:.0}")
                        }
                    })
                    .collect::<Vec<_>>(),
                tr.join(", "),
                m.load_ms,
                m.upload_ms,
                m.pose_ms
            ));
            let st = &m.style_stats;
            out.push_str(&format!(
                "\n  styles{}: slots {:?}; pieces {} passes {} (engine default {}, effect path {}), stage-0 ops {:?}, bump passes {}, generated-uv stages {}, wireframe passes {}\n  \
                 sun: {} ; light-group ambient ({:.1}, {:.1}, {:.1}); LODs {:?}, triangles drawn {}",
                if m.plain { " (plain: not applied)" } else { "" },
                m.styles,
                st.pieces,
                st.passes,
                st.default_pieces,
                st.effect_pieces,
                st.stage0_ops,
                st.bump_passes,
                st.generated_uv_stages,
                st.wireframe_passes,
                match m.sun {
                    Some((d, c, f)) => format!(
                        "dir ({:.3}, {:.3}, {:.3}) colour ({:.0}, {:.0}, {:.0}) to-ambient {:.2}{}",
                        d[0],
                        d[1],
                        d[2],
                        c[0],
                        c[1],
                        c[2],
                        f,
                        if m.sun_from_world { " (world global light)" } else { " (given)" }
                    ),
                    None => "none".to_string(),
                },
                m.group_ambient[0],
                m.group_ambient[1],
                m.group_ambient[2],
                m.lods,
                m.triangles_drawn,
            ));
            if let Some(w) = &m.weapon {
                out.push_str(&format!(
                    "\n  weapon {}: socket {} (node {}) at ({:.1}, {:.1}, {:.1}), node bounds {:?}, weapon bounds {:?}, pieces {}, missing textures {}, styles {:?}",
                    w.path_hash,
                    w.socket,
                    w.node,
                    w.socket_at[0],
                    w.socket_at[1],
                    w.socket_at[2],
                    w.node_bounds.map(|(a, b)| (a.map(|v| v.round()), b.map(|v| v.round()))),
                    w.weapon_bounds.map(|(a, b)| (a.map(|v| v.round()), b.map(|v| v.round()))),
                    w.pieces,
                    w.missing_textures,
                    w.styles,
                ));
            }
        }
        if let Some(v) = &self.view_model {
            let r2 = |a: [f32; 3]| a.map(|x| (x * 100.0).round() / 100.0);
            out.push_str(&format!(
                "\n  view model {}: clip {:?}, eye-space bounds {:?}, screen rect {:?}, pieces {}, missing textures {}, ambient ({:.0}, {:.0}, {:.0}), lights {}, styles {:?}; drawn {}",
                v.path_hash,
                v.clip,
                v.eye_bounds.map(|(a, b)| (r2(a), r2(b))),
                v.screen_rect,
                v.pieces,
                v.missing_textures,
                v.ambient[0],
                v.ambient[1],
                v.ambient[2],
                v.lights,
                v.styles,
                self.frame.view_models,
            ));
        }
        out
    }
}

/// The world's lighting for the viewer's models: the light grid (with the light groups), the static lights and
/// the sun.
struct WorldLighting {
    grid: AmbientGrid,
    statics: Vec<ModelLight>,
    static_count: usize,
    sun: Option<([f32; 3], [f32; 3], f32)>,
    sun_from_world: bool,
}

impl WorldLighting {
    // SPEC: RENDER_2009.md §6.3 (ambient from the light grid at the model position plus the world's lights), §6.2
    // (the light groups' grids add to that grid); no dynamic lights in the viewer
    fn of(
        world: &World,
        light_groups: Option<f32>,
        given_sun: Option<([f32; 3], [f32; 3])>,
        world_sun: bool,
    ) -> WorldLighting {
        let grid = match (&world.render, light_groups) {
            (Some(r), Some(k)) => AmbientGrid::with_groups(&world.light_grid, &r.grids, k),
            _ => AmbientGrid::of(&world.light_grid),
        };
        let mut statics: Vec<ModelLight> = world_lights(world);
        let static_count = statics.len();
        // SPEC: UDP_SESSION_2009_B.md §8.4 (S2C_DIR_LIGHT); the world's global light object as its source
        // (lights.rs `global_light`, PLAUSIBLE)
        let (sun, sun_from_world) = match given_sun {
            Some((dir, colour)) => (Some((dir, colour, 0.0)), false),
            None if world_sun => match global_light(world) {
                Some(g) => (
                    Some((g.dir.to_array(), g.colour.to_array(), g.to_ambient)),
                    true,
                ),
                None => (None, false),
            },
            None => (None, false),
        };
        if let Some((dir, colour, f)) = sun {
            statics.push(ModelLight::sun_with(Vec3::from(dir), Vec3::from(colour), f));
        }
        WorldLighting {
            grid,
            statics,
            static_count,
            sun,
            sun_from_world,
        }
    }
}

/// Screen rectangle `[x0, y0, x1, y1]` (pixels, y down) of an eye-space box through `projection`, clamped to the
/// target; corners nearer than `near` are moved onto the near plane; `None` when the box is off screen.
pub fn screen_rect_of(
    projection: glam::Mat4,
    near: f32,
    lo: Vec3,
    hi: Vec3,
    w: u32,
    h: u32,
) -> Option<[u32; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for k in 0..8 {
        let c = Vec3::new(
            if k & 1 == 0 { lo.x } else { hi.x },
            if k & 2 == 0 { lo.y } else { hi.y },
            if k & 4 == 0 { lo.z } else { hi.z },
        );
        let p = projection * Vec4::new(c.x, c.y, c.z.max(near), 1.0);
        if p.w <= 1e-6 {
            return None;
        }
        let (sx, sy) = (
            (p.x / p.w * 0.5 + 0.5) * w as f32,
            (0.5 - p.y / p.w * 0.5) * h as f32,
        );
        x0 = x0.min(sx);
        x1 = x1.max(sx);
        y0 = y0.min(sy);
        y1 = y1.max(sy);
    }
    let clamp = |v: f32, m: u32| v.clamp(0.0, m as f32) as u32;
    let r = [clamp(x0, w), clamp(y0, h), clamp(x1, w), clamp(y1, h)];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

/// World bounds of the eight corners of a model-space box.
fn corners_bounds(m: glam::Mat4, lo: Vec3, hi: Vec3) -> (Vec3, Vec3) {
    let mut out: Option<(Vec3, Vec3)> = None;
    for k in 0..8 {
        let c = Vec3::new(
            if k & 1 == 0 { lo.x } else { hi.x },
            if k & 2 == 0 { lo.y } else { hi.y },
            if k & 4 == 0 { lo.z } else { hi.z },
        );
        let p = m.transform_point3(c);
        out = Some(match out {
            None => (p, p),
            Some((a, b)) => (a.min(p), b.max(p)),
        });
    }
    out.unwrap_or((lo, hi))
}

/// Default output folder for ROM-derived images when the caller names none: `DC_RENDER_OUT`, else a folder
/// under the system temp directory. Never inside the source tree.
pub fn default_out_dir() -> PathBuf {
    match std::env::var_os("DC_RENDER_OUT") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::temp_dir().join("dc-render"),
    }
}
