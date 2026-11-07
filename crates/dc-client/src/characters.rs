//! Remote characters and the own weapon drawn with dc-render's models: each remote player's character object as
//! the server describes it (the body and face models of its model block, the weapon object attached to it at
//! socket 2), placed at its stream position and yaw and posed by the stream's three animation trackers; and the
//! first-person model of the held weapon in front of the eye.
//!
//! Models and textures are read from the user's ROM by the resource paths the server announced (session file
//! table), the way the 2009 client resolves a file-list name in its mounted archives. A character whose object
//! names no model is drawn with the default character (body row 0, skin 0, face row 0, team mark 0), the choice the
//! reference made for players whose look selected nothing. Nothing of the ROM is stored here.
// SPEC: UDP_SESSION_2009_B.md §3.4 (model block), §3.5 (character object: body + child model, 5 textures, face; weapon
// object at socket 2; `CF_FILES` on it = the drawn weapon), §4.3 (three trackers per stream entry: main, tracker 1 with
// weight set 0, tracker 2 with weight set 1), §4.4; UDP_SESSION_2009_A.md §4.4 (names resolved in every mounted
// resource tree); RENDER_2009.md §6.3 (model lighting), §7 (models), §7.3 (each piece with its render style); FMT_LTB.md
// §11.1 (sockets: node, rotation, position)

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use dc_render::{
    pv_none_texture, quat_from_key, resolve_view_skins, CharacterTable, GpuContext, HandModels,
    LightSet, ModelInstance, TextureIndex, Tracker, ViewModels, WorldRenderer, KEY_ORDER,
};
use dc_rez::RezVfs;
use glam::{Mat4, Quat, Vec3};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::loader::{loader_threads, Done, Loaded, Loader, Shared, UploadKey};

use crate::world::{ModelRef, RemoteBody, RemoteLook, ViewWeapon, WorldLights};
// client-fps: the 2009 per-object sun cache
use crate::world::SunObject;

/// Where the first-person weapon model's origin sits in eye space: the weapon row's `Pos` plus the offset of the
/// playing clip (the per-child clip overrides of the model files, [`dc_render::AnimSet::clip_offset`]); the clip's
/// own node tracks (the rig root included) pose the model from there. The model is drawn by dc-render's
/// player-view pass (its own projection, VIEW identity, depth range [0, 0.1]).
// DISASM: engine+0x1b4870 (the player-view pass: projection = public D3DX left-handed perspective with the engine's
// player-view fov console value, default 50 degrees vertical, aspect = the camera context's width / height, near 0.1,
// far 7; VIEW = identity); engine+0xfb8c0 (the context's width / height = right - left, bottom - top of the
// viewport). The CShell camera reset writes a player-view aspect console value (cshell+0x1f3e9d) that the engine never
// reads (no reference in Engine.exe besides its registration); no ROM config sets the fov value.
// DISASM: cshell+0x232c10 / cshell+0x2f6e0 (every frame the game sets the model position to the row's `Pos`, record
// +0x14, plus the bob and recoil terms; nothing else positions or rotates the object)
// EVIDENCE: engine+0x6e0d0 (the clip override's offset lands in the clip info at +0x10); SPEC: FMT_LTB.md §11.2
// OBSERVED: the ROM first-person clip sets carry offsets (-0.4, -3.6, -1.2) (rifle 145), (-0.4, -3.6, -1.0) (pistol
// 164), (-0.7, -8.0, -1.0) (grenade 7) and none for the knife (0), whose clips instead hold the rig root 4.21 below its
// bind; the rifle's measured 2009 placement (CLI-330: `Pos` + (-0.40, -3.61, -1.19) from
// captures/20261025_185323_sbs3-real/driver/rest_1.png, 6 px rms) is its clip offset to 0.01. With offset and rig
// root as authored, the rig roots of all four land at eye-space height -3.5 .. -3.85.
// DC-DEFAULT: CLI-575 (proposed) the engine's reader of the clip info offset is not traced: the offset is taken as a
// model translation in eye space (the CLI-330 measurement agrees), the main tracker's clip chooses it
pub fn view_model_origin(pos: [f32; 3], clip_offset: Option<Vec3>) -> Vec3 {
    Vec3::from(pos) + clip_offset.unwrap_or(Vec3::ZERO)
}

/// The default character for an object without a usable model block: body row, skin variant, face row, team mark.
// DC-DEFAULT: CLI-38 (proposed) body row 0, skin 0, face row 0, mark 0 (the reference stack's choice for an all-zero look,
// UDP_SESSION_2009_B.md §3.5.1, REFERENCE; dc-dserver's DSV-24)
pub const DEFAULT_BODY_ROW: u32 = 0;
pub const DEFAULT_SKIN: u32 = 0;
pub const DEFAULT_FACE_ROW: u32 = 0;
pub const DEFAULT_MARK: u32 = 0;

/// The texture of a first-person skin entry that names no resolvable file (since CLI-105 the hands entry resolves
/// to the character's hand skin; this is left for a skin the ROM does not hold).
// DC-DEFAULT: CLI-41 (proposed) a plain made-up mid-tone (not a ROM colour) instead of the missing-texture checker
pub const VIEW_SKIN_FALLBACK: [u8; 4] = [150, 125, 105, 255];
/// Which entry of the own body row's hand skin list the first-person hands use.
// DC-DEFAULT: CLI-107 (proposed) entry 0. EVIDENCE: cshell+0x23b6a0 / +0x1e3180 (the entry index is a byte of the local
// player's record, +0x154, passed through a redirect table of the character data, +0x15e8 stride 0x88 +0x7c, -1 =
// keep); neither the record field nor the table is read by this client (PLAUSIBLE: 0 for the default character)
pub const VIEW_HANDS_INDEX: u32 = 0;

/// Counters for reports and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModelStats {
    /// Remote characters drawn with a model this frame.
    pub characters: u32,
    /// Model instances (bodies, faces, weapons) drawn this frame.
    pub instances: u32,
    /// Characters drawn with the default look this frame.
    pub defaulted: u32,
    /// Model files or uploads that failed so far.
    pub failures: u32,
    /// The own weapon's first-person model was drawn this frame.
    pub view_weapon: bool,
    /// Pieces of the uploads so far drawn with the engine default style (empty or missing style slot).
    pub default_style_pieces: u64,
}

/// The own character as the first-person model needs it: body row and skin variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnBody {
    pub row: u32,
    pub variant: u32,
}

impl Default for OwnBody {
    fn default() -> Self {
        OwnBody {
            row: DEFAULT_BODY_ROW,
            variant: DEFAULT_SKIN,
        }
    }
}

/// What the first-person model was built from (rebuilt when any part changes).
#[derive(Debug, Clone, PartialEq)]
struct ViewKey {
    weapon: u16,
    model: ModelRef,
    styles: Vec<String>,
}

/// The body's upload key: the render styles the server's model block names win over the table row's (the infected
/// look of class 11 is the normal body with the override row's styles, DSV-214).
// SPEC: UDP_SESSION_2009_B.md §3.4 (the model block's render-style ids are the object's styles)
// DC-DEFAULT: CLI-1150 (proposed) the block's body styles replace the table's; a block whose style names do not all
// resolve keeps the table's
fn body_key(mut key: UploadKey, block: &[String]) -> UploadKey {
    if !block.is_empty() {
        key.styles = block.to_vec();
    }
    key
}

/// CLI-1485: the face model is skinned in the body's bind space (as hair and head gear, CLI-1346): each node it
/// shares with the body by name takes the body's skin matrix, so the face's vertices are read in the body's model
/// space whatever bind its own file gives the node. The creation table's sixth model (body row 7 with face row 5)
/// has a face file that binds its nodes 2.27 units below the body file's, and with its own bind the face stood that
/// far above the neck.
// OBSERVED: ROM census of face row 5 / 6 against body row 7 (9 nodes shared by name, 1 with the same bind, the
// others 2.27 units lower); playtest report ("the last model's head floats above the body")
// DC-DEFAULT: CLI-1485 (proposed), see DC_DEFAULTS.md
pub const FACE_IN_BODY_SPACE: bool = true;

/// One remote character on the GPU.
/// CLI-1635: the input of a character's rebuild check.
type SeenLook = (Vec<ModelRef>, Option<(ModelRef, u32)>, Vec<String>, u8, Option<u8>);

struct Live {
    key: (Vec<ModelRef>, Option<(ModelRef, u32)>, Vec<String>),
    body: ModelInstance,
    face: Option<ModelInstance>,
    weapon: Option<ModelInstance>,
    /// The weapon model's files (its sockets) and the mod models on it ([`crate::attach`], CLI-618).
    weapon_files: Option<Arc<dc_render::ModelFiles>>,
    attach: Vec<(crate::attach::WeaponAttach, Option<ModelInstance>, bool)>,
    /// The weapon's socket on the body: node and the socket's local transform.
    socket: Option<(usize, Mat4)>,
    /// CLI-1635: outside the view this frame ([`ModelScene::view_cull`]): placed but not posed, not drawn.
    culled: bool,
    /// CLI-1635: what the last rebuild check saw (the stream's models, weapon and styles, the team and the own team); the
    /// same input needs no new key.
    seen: Option<SeenLook>,
    /// client-tags2: the caption socket of the name over the head on the body: node and the socket's position in
    /// that node's space (none: the body has no such socket).
    name_at: Option<(usize, Vec3)>,
    /// shop-model: the body file (its sockets) and the extra pieces on the body ([`ModelScene::place_body_pieces`])
    /// with whether each is placed this frame.
    body_files: Arc<dc_render::ModelFiles>,
    pieces: Vec<(BodyPiece, Option<ModelInstance>, bool)>,
}

/// shop-model: an extra piece of a character (head gear, goggles, backpack): its model with textures and render
/// styles, on the body's skeleton (`socket: None`: posed with the body by node name, as the face) or at a body socket.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyPiece {
    pub model: ModelRef,
    pub styles: Vec<String>,
    pub socket: Option<String>,
}

/// A model upload as the frame thread sees it.
enum Slot {
    /// Requested from the loader threads, not handed back yet.
    Pending,
    Ready(Loaded),
    /// The model file does not load.
    Failed,
}

/// A model the frame asked for: there, or still loading.
enum Fetch<T> {
    Ready(T),
    Pending,
}

/// The models of the frame: remote characters and the own weapon.
///
/// Models load either inline (the default: the call that first needs a model reads, decodes and uploads it, as
/// before) or, after [`ModelScene::set_async_loads`], on loader threads ([`crate::loader`]): until a model is ready
/// its object is drawn with nothing (a character whose look changed keeps its previous look; the first-person model
/// and a drop are left out), and [`ModelScene::wait_for_loads`] gives tests and PNG recipes the frames of the inline
/// mode. Every upload is kept per (file, textures, styles) key and each object draws its own instance of it, so a
/// second character with the same look costs no ROM read, decode or texture upload.
// DC-DEFAULT: CLI-260, CLI-262 (proposed), see DC_DEFAULTS.md
pub struct ModelScene {
    table: Option<CharacterTable>,
    /// Third-person weapon rows (their render styles by model file).
    hand_table: Option<HandModels>,
    /// First-person weapon rows with their per-hand-identifier entries.
    view_table: Option<ViewModels>,
    /// The texture the arm placeholders of the first-person skin lists become (read from the user's CShell.dll).
    none_texture: Option<String>,
    /// The ROM caches shared with the loader threads (model files, decoded textures, render styles).
    shared: Arc<Shared>,
    /// Every upload asked for, by key.
    templates: HashMap<UploadKey, Slot>,
    /// Model files requested for a clip map (async mode).
    files_pending: HashSet<String>,
    /// Load on loader threads ([`ModelScene::set_async_loads`]).
    async_loads: bool,
    loader: Option<Loader>,
    /// The loader threads could not be started (loads stay inline).
    loader_failed: bool,
    /// Objects whose model is still loading this frame (drawn with nothing).
    loading: Vec<u16>,
    /// Clip index maps between two model files (by clip name), for the first-person clip of another model.
    clip_maps: HashMap<(String, String), Vec<Option<usize>>>,
    live: BTreeMap<u16, Live>,
    view: Option<(ViewKey, ModelInstance)>,
    /// The files of the first-person model (its sockets).
    view_files: Option<Arc<dc_render::ModelFiles>>,
    /// The mod models on the first-person model ([`crate::attach`]) and whether each is placed this frame.
    view_attach: Vec<(crate::attach::WeaponAttach, Option<ModelInstance>, bool)>,
    /// The scope hides the first-person models ([`ModelScene::set_view_hidden`]).
    view_hidden: bool,
    own: OwnBody,
    pub stats: ModelStats,
    /// Objects whose models could not be loaded (drawn as boxes).
    pub failed: Vec<u16>,
    /// The dropped weapons ([`crate::drops`]).
    drops: crate::drops::DropModels,
    /// The clip-switch blends of the drawn objects ([`crate::anims::ClipBlends`]).
    anim_blends: crate::anims::ClipBlends,
    /// The local player's team (client-hudgaps): a character of this team carries the team-mate mark.
    pub own_team: Option<u8>,
    /// CLI-1635: with a declared x-ray server every player of the map is in the frame, so a character outside this view
    /// (`None`: never) is only placed: no pose, no gear, no uniforms, no draws.
    pub view_cull: Option<ViewCull>,
    /// client-destructfx: the effect models of this frame ([`crate::fxmodels`]).
    pub fx_models: crate::fxmodels::FxModelSet,
    /// CRATE-8: one model shown on its own (the case reel's won gun), with the model it is.
    showcase: Option<(ModelRef, ModelInstance)>,
    /// client-tags2: the name of the body socket the name over the head stands on, read from the user's CShell.dll
    /// ([`crate::hud::marks::NAME_SOCKET`]); `None`: the names keep their box points.
    name_socket: Option<String>,
    /// CLI-1231: the look tables the gear pieces come from, and each character's gear as last built (keyed by the
    /// body and face files and the gear ids, so an unchanged look costs no rebuild).
    figure: Option<Arc<crate::menu::figure::FigureTables>>,
    gear: BTreeMap<u16, (GearKey, Vec<BodyPiece>)>,
}

/// What a character's gear pieces are built from: body file, face file, gear ids.
type GearKey = (String, Option<String>, crate::menu::figure::GearIds);

fn key_of(path: &str) -> String {
    path.trim().replace('/', "\\").to_ascii_uppercase()
}

impl ModelScene {
    /// Opens the user's ROM (archive VFS, texture index, character table).
    pub fn open(rom: &Path) -> Result<ModelScene, String> {
        let index = Arc::new(TextureIndex::open(rom).map_err(|e| e.to_string())?);
        let vfs = Arc::new(RezVfs::new(rom));
        let table = CharacterTable::from_vfs(&vfs).ok();
        let hand_table = HandModels::from_vfs(&vfs).ok();
        let view_table = ViewModels::from_vfs(&vfs).ok();
        let drop_row = crate::drops::DropMotionRow::from_vfs(&vfs);
        let drop_pieces = crate::drops::ProjectilePieces::from_vfs(&vfs);
        Ok(ModelScene {
            shared: Arc::new(Shared::new(vfs, index)),
            table,
            hand_table,
            view_table,
            none_texture: pv_none_texture(rom),
            templates: HashMap::new(),
            files_pending: HashSet::new(),
            async_loads: false,
            loader: None,
            loader_failed: false,
            loading: Vec::new(),
            clip_maps: HashMap::new(),
            live: BTreeMap::new(),
            view: None,
            view_files: None,
            view_attach: Vec::new(),
            view_hidden: false,
            own: OwnBody::default(),
            stats: ModelStats::default(),
            failed: Vec::new(),
            drops: crate::drops::DropModels::new(drop_row).with_pieces(drop_pieces),
            anim_blends: crate::anims::ClipBlends::default(),
            own_team: None,
            view_cull: None,
            fx_models: crate::fxmodels::FxModelSet::default(),
            showcase: None,
            name_socket: crate::hud::marks::name_socket_from_rom(rom),
            figure: Some(crate::menu::figure::FigureTables::cached(rom)),
            gear: BTreeMap::new(),
        })
    }

    /// The render-style names of a model file from the ROM tables: a body or face row, else a third-person weapon
    /// row naming the file (empty: every piece gets the engine default, as for an object without styles).
    // SPEC: UDP_SESSION_2009_B.md §3.5 (the server's model block names the row's RenderStyle<k> / HHRenderStyle<k>);
    // DC-DEFAULT: CLI-109 (proposed) the client takes the names from the tables by model file until the session
    // hands it the block's style ids
    fn style_names(&self, path: &str) -> Vec<String> {
        if let Some(v) = self.table.as_ref().and_then(|t| t.styles_of_model(path)) {
            return v;
        }
        let want = key_of(path);
        self.hand_table
            .as_ref()
            .and_then(|h| h.rows.values().find(|r| key_of(&r.model) == want))
            .map(|r| r.styles.clone())
            .unwrap_or_default()
    }

    /// The upload key of a model with the render styles of its table row (no texture fallback).
    fn key_for(&self, m: &ModelRef) -> UploadKey {
        UploadKey {
            model: m.clone(),
            styles: self.style_names(&m.model),
            fallback: None,
        }
    }

    /// Books what the loader threads handed back.
    fn take(&mut self, done: Vec<Done>) {
        for d in done {
            match d {
                Done::Upload(k, r) => {
                    crate::prof::MODEL_LOADS.fetch_add(1, Ordering::Relaxed);
                    dc_render::perf::add(dc_render::perf::Counter::ModelLoads, 1);
                    dc_render::perf::stream_done(stream_id(&k));
                    self.templates
                        .insert(k, r.map_or(Slot::Failed, Slot::Ready));
                }
                Done::Files(p) => {
                    self.files_pending.remove(&key_of(&p));
                }
            }
        }
        self.stats.failures = self.shared.failures.load(Ordering::Relaxed);
    }

    /// gpu-prewarm: makes every pipeline the ROM's render styles need with `renderer` on this (the frame) thread, and
    /// the renderer's other on-demand passes ([`WorldRenderer::prewarm`]).
    pub fn prewarm(&self, ctx: &GpuContext, renderer: &WorldRenderer) -> dc_render::PrewarmCount {
        let styles = self.shared.rom_styles();
        renderer.prewarm(ctx, styles.iter().map(|s| &**s))
    }

    /// Takes the finished loads (never blocks).
    fn drain(&mut self) {
        if let Some(l) = self.loader.as_mut() {
            let done = l.poll();
            self.take(done);
        }
    }

    /// The upload of `key`: from the cache, else requested from the loader threads (async mode: `Pending` until it
    /// is handed back), else read, decoded and uploaded here ([`Shared::build`]).
    fn fetch(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        key: UploadKey,
    ) -> Fetch<Option<Loaded>> {
        match self.templates.get(&key) {
            Some(Slot::Ready(l)) => return Fetch::Ready(Some(l.clone())),
            Some(Slot::Failed) => return Fetch::Ready(None),
            Some(Slot::Pending) => return Fetch::Pending,
            None => {}
        }
        // perf-g1: request-to-resident latency of the model streaming (crate::prof)
        let sid = stream_id(&key);
        dc_render::perf::stream_request(sid);
        if self.async_loads && self.loader.is_none() && !self.loader_failed {
            self.loader = Loader::start(
                self.shared.clone(),
                renderer.model_uploader(ctx),
                loader_threads(),
            );
            self.loader_failed = self.loader.is_none();
        }
        if self.async_loads {
            if let Some(l) = self.loader.as_mut() {
                // gpu-prewarm: the pipelines of the key's styles are made here, on the frame thread, when the request
                // is made (a style the match did not pre-warm: logged as made in play); the loader only uploads
                renderer
                    .models()
                    .ensure_styles(ctx, &self.shared.style_list(&key.styles));
                if l.upload(key.clone()) {
                    self.templates.insert(key, Slot::Pending);
                    return Fetch::Pending;
                }
            }
        }
        let r = self
            .shared
            .build(&key, &|f, look| renderer.upload_model_look(ctx, f, look));
        crate::prof::MODEL_LOADS.fetch_add(1, Ordering::Relaxed);
        dc_render::perf::add(dc_render::perf::Counter::ModelLoads, 1);
        dc_render::perf::stream_done(sid);
        self.stats.failures = self.shared.failures.load(Ordering::Relaxed);
        let slot = match &r {
            Some(l) => Slot::Ready(l.clone()),
            None => Slot::Failed,
        };
        self.templates.insert(key, slot);
        Fetch::Ready(r)
    }

    /// An object's own instance of a finished upload.
    fn instance(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        l: &Loaded,
    ) -> ModelInstance {
        let inst = renderer.instantiate_model(ctx, &l.template);
        self.stats.default_style_pieces += inst.style_stats.default_pieces;
        inst
    }

    /// Takes the own character's look (the own object's model block): its body row and skin variant choose the
    /// first-person model and the hand skin. `None` or an unknown body: the default character (CLI-105).
    // DC-DEFAULT: CLI-105 (proposed) the body row = the table row whose model file is the body entry's, the variant =
    // the row's skin variant whose upper skin is the entry's first texture; otherwise row 0, variant 0
    pub fn set_own_look(&mut self, look: Option<&RemoteLook>) {
        let own = look
            .and_then(|l| l.models.first())
            .and_then(|body| {
                let t = self.table.as_ref()?;
                let (row, _) = t.row_of_model(&body.model)?;
                let variant = body
                    .textures
                    .first()
                    .and_then(|u| t.skin_variant_of(row, u))
                    .unwrap_or(DEFAULT_SKIN);
                Some(OwnBody { row, variant })
            })
            .unwrap_or_default();
        self.own = own;
    }

    /// The own character's body row and skin variant.
    pub fn own_body(&self) -> OwnBody {
        self.own
    }

    /// The first-person model of weapon `weapon` for the own character: model, resolved skins (hands placeholder =
    /// the body row's hand skin, arm placeholders = the no-texture file) and render styles; `held` (the session's
    /// choice) when the table has no entry.
    // EVIDENCE: cshell+0x23b6a0 (identifier entry of the character row, hands skin, arm names), cshell+0x1e3180 (hand
    // skin `HandsSkin<index>` of the row with the skin variant byte), cshell+0x1e0f00 (the row's identifier)
    pub fn view_look(
        &self,
        weapon: u16,
        held: Option<&ModelRef>,
    ) -> Option<(ModelRef, Vec<String>)> {
        let row = self
            .table
            .as_ref()
            .and_then(|t| t.bodies.get(&self.own.row));
        let ident = row.and_then(|r| r.pv_ident.as_deref());
        let hands = self.table.as_ref().and_then(|t| {
            t.hands_skin(
                self.own.row,
                VIEW_HANDS_INDEX,
                self.own.variant.min(255) as u8,
            )
        });
        let (model, skins, styles) = match self
            .view_table
            .as_ref()
            .and_then(|v| crate::camo::view_model(v, weapon as u32, ident))
        {
            Some(vm) => (vm.model, vm.skins, vm.styles),
            None => {
                let h = held?;
                (h.model.clone(), h.textures.clone(), Vec::new())
            }
        };
        let textures = resolve_view_skins(&skins, hands.as_deref(), self.none_texture.as_deref());
        Some((ModelRef { model, textures }, styles))
    }

    /// The clip of model `to` that has the name of clip `clip` of model `from` (the session's first-person clip
    /// index is one of the model it chose).
    fn map_clip(&mut self, from: &str, to: &str, clip: usize) -> Option<usize> {
        if key_of(from) == key_of(to) {
            return Some(clip);
        }
        let k = (key_of(from), key_of(to));
        if !self.clip_maps.contains_key(&k) {
            let map = match (self.shared.files(from), self.shared.files(to)) {
                (Some(a), Some(b)) => {
                    let (a, b) = (a.anim_set(), b.anim_set());
                    (0..a.clip_count())
                        .map(|i| {
                            let name = a.clip(i)?.name.as_bytes().to_ascii_lowercase();
                            b.find_clip(|n| n.to_ascii_lowercase() == name)
                        })
                        .collect()
                }
                _ => Vec::new(),
            };
            self.clip_maps.insert(k.clone(), map);
        }
        self.clip_maps.get(&k)?.get(clip).copied().flatten()
    }

    /// The models of the default character: body and face with their texture lists.
    pub fn default_look(&self) -> Vec<ModelRef> {
        let Some(t) = &self.table else {
            return Vec::new();
        };
        let mut v = Vec::new();
        if let (Some(row), Some(tex)) = (
            t.bodies.get(&DEFAULT_BODY_ROW),
            t.body_textures(DEFAULT_BODY_ROW, DEFAULT_SKIN, DEFAULT_MARK),
        ) {
            v.push(ModelRef {
                model: row.model.clone(),
                textures: tex,
            });
        }
        if let Some((model, skins)) = t.faces.get(&DEFAULT_FACE_ROW) {
            v.push(ModelRef {
                model: model.clone(),
                textures: skins.iter().take(1).cloned().collect(),
            });
        }
        v
    }

    /// Builds or updates the models of this frame's remote players (drops the ones that left) and poses them.
    pub fn update_remotes(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        remotes: &[RemoteBody],
        lights: Option<&WorldLights>,
    ) {
        let list: Vec<&RemoteBody> = remotes.iter().collect();
        self.update_bodies(ctx, renderer, &list, lights);
    }

    fn update_bodies(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        remotes: &[&RemoteBody],
        lights: Option<&WorldLights>,
    ) {
        self.stats.characters = 0;
        self.stats.instances = 0;
        self.stats.defaulted = 0;
        self.drain();
        self.loading.clear();
        let present: Vec<u16> = remotes.iter().map(|r| r.obj).collect();
        self.live.retain(|k, _| present.contains(k));
        self.failed.retain(|k| present.contains(k));
        for &r in remotes {
            let Some(look) = &r.look else {
                continue;
            };
            // A hidden object (the dead character from its death to its respawn, CLI-42) is not drawn: its built
            // models go, or the last pose of the living character would stay drawn, upright and frozen, until the
            // respawn (user retest 2026-10-26 item 7). The rebuild at the respawn comes from the upload cache.
            // SPEC: UDP_SESSION_2009_B.md §3.5.5 (in the death group the character turns to flags 0x0802 and the
            // corpse object is created); DC-DEFAULT: CLI-42 (proposed), CLI-350 (proposed)
            if !look.visible {
                self.live.remove(&r.obj);
                continue;
            }
            if !r.pos.iter().all(|c| c.is_finite()) {
                continue;
            }
            let own_team = self.own_team;
            let defaulted = look.models.is_empty();
            // CLI-1635: the same stream input as the last frame: the same key, no copies of its names
            let seen_same = self.live.get(&r.obj).is_some_and(|l| {
                l.seen.as_ref().is_some_and(|s| {
                    s.0 == look.models
                        && s.1 == look.weapon
                        && s.2 == look.styles
                        && s.3 == r.team
                        && s.4 == own_team
                })
            });
            if seen_same {
                let culled = self.view_cull.is_some_and(|v| v.outside(r.pos));
                if let Some(l) = self.live.get_mut(&r.obj) {
                    l.culled = culled;
                    if culled {
                        place(l, r);
                    } else {
                        pose(l, r, lights);
                    }
                    self.stats.characters += 1;
                    self.stats.defaulted += defaulted as u32;
                }
                continue;
            }
            let models = if defaulted {
                self.default_look()
            } else {
                look.models.clone()
            };
            if models.is_empty() {
                continue;
            }
            let models = self.team_marked(models, r.team);
            let styles = if defaulted {
                Vec::new()
            } else {
                look.styles.clone()
            };
            let key = (models.clone(), look.weapon.clone(), styles);
            if self.live.get(&r.obj).is_none_or(|l| l.key != key) && !self.failed.contains(&r.obj) {
                match self.build(ctx, renderer, &models, &key.2, look.weapon.as_ref()) {
                    Fetch::Ready(Some(mut l)) => {
                        l.key = key.clone();
                        self.live.insert(r.obj, l);
                    }
                    Fetch::Ready(None) => {
                        self.live.remove(&r.obj);
                        self.failed.push(r.obj);
                    }
                    // still loading: the previous look stays (none yet: drawn with nothing)
                    Fetch::Pending => {
                        if !self.live.contains_key(&r.obj) {
                            self.loading.push(r.obj);
                        }
                    }
                }
            }
            let Some(l) = self.live.get_mut(&r.obj) else {
                continue;
            };
            if l.key == key {
                l.seen = Some((
                    look.models.clone(),
                    look.weapon.clone(),
                    look.styles.clone(),
                    r.team,
                    own_team,
                ));
            }
            l.culled = false;
            pose(l, r, lights);
            self.stats.characters += 1;
            self.stats.defaulted += defaulted as u32;
        }
    }

    /// The models of one character: body, face and weapon. Every part is requested before any is waited for (in the
    /// async mode they load side by side); `Pending` until all of them are there; `None` when the body does not load
    /// (a face or weapon that does not load is left out).
    fn build(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        models: &[ModelRef],
        body_styles: &[String],
        weapon: Option<&(ModelRef, u32)>,
    ) -> Fetch<Option<Live>> {
        let Some(first) = models.first() else {
            return Fetch::Ready(None);
        };
        let key = body_key(self.key_for(first), body_styles);
        let body = self.fetch(ctx, renderer, key);
        if matches!(body, Fetch::Ready(None)) {
            return Fetch::Ready(None);
        }
        let face = models
            .get(1)
            .map(|m| self.fetch(ctx, renderer, self.key_for(m)));
        let gun = weapon.map(|(m, _)| self.fetch(ctx, renderer, self.key_for(m)));
        fn ready(f: &Fetch<Option<Loaded>>) -> Option<Option<Loaded>> {
            match f {
                Fetch::Ready(l) => Some(l.clone()),
                Fetch::Pending => None,
            }
        }
        let (Some(Some(body)), Some(face), Some(gun)) = (
            ready(&body),
            face.as_ref().map_or(Some(None), ready),
            gun.as_ref().map_or(Some(None), ready),
        ) else {
            return Fetch::Pending;
        };
        let files = body.files.clone();
        let body = self.instance(ctx, renderer, &body);
        let face = face.map(|l| self.instance(ctx, renderer, &l));
        let mut socket = None;
        let weapon_files = gun.as_ref().map(|g| g.files.clone());
        let weapon = match weapon.zip(gun) {
            Some(((_, s), gun)) => {
                // SPEC: FMT_LTB.md §11.1 (socket: node, rotation quaternion, position)
                if let Some(sk) = files.body.sockets.get(*s as usize) {
                    let rot = quat_from_key(sk.rot, KEY_ORDER);
                    socket = Some((
                        sk.node as usize,
                        Mat4::from_rotation_translation(rot, Vec3::from(sk.pos)),
                    ));
                }
                let mut w = self.instance(ctx, renderer, &gun);
                // the weapon object's main tracker plays its first clip (UDP_SESSION_2009_B.md §3.5.1)
                w.set_pose(&[Tracker::main(0, 0.0)]);
                Some(w)
            }
            None => None,
        };
        let name_at = self
            .name_socket
            .as_deref()
            .and_then(|n| name_socket_of(&files.body, n));
        Fetch::Ready(Some(Live {
            key: (Vec::new(), None, Vec::new()),
            culled: false,
            seen: None,
            body_files: files.clone(),
            pieces: Vec::new(),
            body,
            face,
            weapon,
            weapon_files,
            attach: Vec::new(),
            socket,
            name_at,
        }))
    }

    /// The first-person weapon model at the eye: the held weapon's first-person model with the clip its weapon
    /// state plays, placed in eye space for the player-view pass (none when the weapon has no first-person model or
    /// it does not load). `eye` is the world eye position the model is lit at.
    pub fn place_view_weapon(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        held: Option<&ViewWeapon>,
        eye: Vec3,
        lights: Option<&WorldLights>,
    ) {
        self.stats.view_weapon = false;
        let Some(vw) = held else {
            self.view = None;
            return;
        };
        let Some((model, styles)) = self.view_look(vw.weapon, vw.model.as_ref()) else {
            self.view = None;
            return;
        };
        let key = ViewKey {
            weapon: vw.weapon,
            model: model.clone(),
            styles: styles.clone(),
        };
        self.drain();
        if self.view.as_ref().is_none_or(|v| v.0 != key) {
            let up = UploadKey {
                model: model.clone(),
                styles: styles.clone(),
                fallback: Some(VIEW_SKIN_FALLBACK),
            };
            match self.fetch(ctx, renderer, up) {
                Fetch::Ready(r) => {
                    self.view_files = r.as_ref().map(|l| l.files.clone());
                    self.view = r.map(|l| (key, self.instance(ctx, renderer, &l)));
                }
                // still loading: nothing is drawn at the eye this frame
                Fetch::Pending => return,
            }
        }
        if let (Some(_), Some(m)) = (vw.clip, vw.model.as_ref()) {
            if !self.clip_map_ready(&m.model, &model.model) {
                return;
            }
        }
        // the session's clip index belongs to the model it chose; take the clip of the same name in this one
        let clip = match (vw.clip, vw.model.as_ref()) {
            (Some((c, t)), Some(m)) => self.map_clip(&m.model, &model.model, c).map(|c| (c, t)),
            (c, None) => c,
            (None, _) => None,
        };
        let Some((_, inst)) = self.view.as_mut() else {
            return;
        };
        // SPEC: RENDER_2009.md §4.2 (positioned in eye space by the game)
        inst.rotation = vw.inspect.map_or(Quat::IDENTITY, |p| p.rotation);
        let tr: Vec<Tracker> = clip
            .map(|(clip, t)| vec![Tracker::main(clip, t)])
            .unwrap_or_default();
        inst.set_pose(&tr);
        // EVIDENCE: cshell+0x232c10 (the row's `Pos` is the model position in eye space; DISASM); the playing clip's
        // offset (CLI-575, see `view_model_origin`); the rig root keeps its animated translation (CLI-191 retired)
        let origin = view_model_origin(vw.pos, clip.map(|c| inst.anim.clip_offset(c.0)));
        // DC-DEFAULT: CLI-1845 the inspect turns the whole model about its pivot
        inst.position = vw.inspect.map_or(origin, |p| p.place(origin));
        // DC-DEFAULT: REN-13 (proposed), see DC_DEFAULTS.md (an eye-attached model is lit at the eye)
        inst.lights = light_set_for(SunObject::ViewWeapon, eye, inst.radius, lights);
        self.stats.view_weapon = true;
    }

    /// The model file of the first-person model built last (for reports and tests).
    pub fn view_model_path(&self) -> Option<&str> {
        self.view.as_ref().map(|v| v.0.model.model.as_str())
    }

    /// The texture list of the first-person model built last (for reports and tests).
    pub fn view_textures(&self) -> Option<&[String]> {
        self.view.as_ref().map(|v| v.0.model.textures.as_slice())
    }

    /// The first-person model built last, for tests that move it ([`ModelScene::place_view_weapon`] places it again
    /// every frame).
    pub fn view_instance_mut(&mut self) -> Option<&mut ModelInstance> {
        self.view.as_mut().map(|v| &mut v.1)
    }

    /// Hides the first-person weapon and its mod models from the player-view pass (the scope is up); they are still
    /// placed, so the muzzle socket stays.
    // DISASM: cshell+0x22d350 / cshell+0x22d320 (the weapon manager's hide / show of the first-person weapons)
    pub fn set_view_hidden(&mut self, hidden: bool) {
        self.view_hidden = hidden;
    }

    /// The placed first-person model (drawn or hidden), for the muzzle socket.
    // DC-DEFAULT: CLI-645 the flash keeps the hidden model's socket
    pub fn view_placed(&self) -> Option<&ModelInstance> {
        match &self.view {
            Some((_, w)) if self.stats.view_weapon => Some(w),
            _ => None,
        }
    }

    /// Every world instance to draw this frame (remote bodies, faces, weapons).
    pub fn instances(&self) -> Vec<&ModelInstance> {
        let mut v = Vec::new();
        for l in self.live.values().filter(|l| !l.culled) {
            v.push(&l.body);
            if let Some(f) = &l.face {
                v.push(f);
            }
            if let Some(w) = &l.weapon {
                v.push(w);
                v.extend(l.attach.iter().filter(|a| a.2).filter_map(|a| a.1.as_ref()));
            }
            v.extend(l.pieces.iter().filter(|p| p.2).filter_map(|p| p.1.as_ref()));
        }
        v.extend(self.drops.instances());
        v.extend(self.fx_models.instances());
        v.extend(self.showcase.as_ref().map(|s| &s.1));
        v
    }

    /// CRATE-8: loads `m` as the showcase model (once; the last one is replaced) and returns its bind-pose bounds in
    /// model space (`None`: it failed to load, or is still loading on the loader threads).
    pub fn showcase_bounds(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        m: &ModelRef,
    ) -> Option<([f32; 3], [f32; 3])> {
        if self.showcase.as_ref().is_none_or(|s| &s.0 != m) {
            self.drain();
            let Fetch::Ready(Some(l)) = self.fetch(ctx, renderer, self.key_for(m)) else {
                return None;
            };
            let mut inst = self.instance(ctx, renderer, &l);
            inst.set_pose(&[]);
            self.showcase = Some((m.clone(), inst));
        }
        self.showcase.as_ref().and_then(|s| s.1.bounds)
    }

    /// CRATE-8: turns the showcase model by `rotation` about the middle of its bounds and puts that middle at `centre`,
    /// lit by `lights`.
    pub fn pose_showcase(&mut self, rotation: glam::Quat, centre: Vec3, lights: LightSet) {
        let Some((_, inst)) = self.showcase.as_mut() else {
            return;
        };
        let mid = inst
            .bounds
            .map_or(Vec3::ZERO, |(lo, hi)| (Vec3::from(lo) + Vec3::from(hi)) * 0.5);
        inst.rotation = rotation;
        inst.position = centre - rotation * mid;
        inst.lights = lights;
    }

    /// CRATE-8: no showcase model.
    pub fn clear_showcase(&mut self) {
        self.showcase = None;
    }

    /// CLI-1624: the posed instances of the character of object `obj` this frame (body first, then face, gear pieces,
    /// weapon and its mod models) for the x-ray pass; empty when it has no model (loading, failed, not drawn, outside
    /// the view).
    pub fn character_instances(&self, obj: u16) -> Vec<&ModelInstance> {
        let Some(l) = self.live.get(&obj).filter(|l| !l.culled) else {
            return Vec::new();
        };
        let mut v = vec![&l.body];
        v.extend(l.face.iter());
        v.extend(l.pieces.iter().filter(|p| p.2).filter_map(|p| p.1.as_ref()));
        if let Some(w) = &l.weapon {
            v.push(w);
            v.extend(l.attach.iter().filter(|a| a.2).filter_map(|a| a.1.as_ref()));
        }
        v
    }

    /// The eye-attached instances of this frame (the own weapon's first-person model, then its mod models), for the
    /// player-view pass.
    pub fn view_instances(&self) -> Vec<&ModelInstance> {
        match &self.view {
            Some(_) if self.view_hidden => Vec::new(),
            Some((_, w)) if self.stats.view_weapon => std::iter::once(w)
                .chain(
                    self.view_attach
                        .iter()
                        .filter(|a| a.2)
                        .filter_map(|a| a.1.as_ref()),
                )
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Builds and places the mod models of the first-person weapon (call after [`ModelScene::place_view_weapon`]):
    /// each at its socket of the first-person model, moved and turned by its offsets in the socket's frame. A mod
    /// whose socket the model lacks, or whose model does not load, is not drawn.
    // EVIDENCE: cshell+0x237983..0x2379ee (socket by name on the first-person model; a missing socket attaches
    // nothing), cshell+0x234380 (per frame: socket world transform, offset along the socket's right / up / forward
    // axes, angles plus the angle offset); see `crate::attach`
    pub fn place_view_attachments(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        list: &[crate::attach::WeaponAttach],
    ) {
        let same = self.view_attach.len() == list.len()
            && self.view_attach.iter().zip(list).all(|(a, b)| a.0 == *b);
        if !same {
            self.view_attach = list.iter().map(|a| (a.clone(), None, false)).collect();
        }
        for i in 0..self.view_attach.len() {
            self.view_attach[i].2 = false;
            if self.view_attach[i].1.is_none() {
                let a = &self.view_attach[i].0;
                let up = UploadKey {
                    model: a.model.clone(),
                    styles: a.styles.clone(),
                    fallback: Some(VIEW_SKIN_FALLBACK),
                };
                if let Fetch::Ready(Some(l)) = self.fetch(ctx, renderer, up) {
                    let inst = self.instance(ctx, renderer, &l);
                    self.view_attach[i].1 = Some(inst);
                }
            }
        }
        let (Some((_, view)), Some(files), true) = (
            self.view.as_ref(),
            self.view_files.as_ref(),
            self.stats.view_weapon,
        ) else {
            return;
        };
        for (a, inst, placed) in self.view_attach.iter_mut() {
            let Some(child) = inst.as_mut() else { continue };
            let Some(socket) = files
                .body
                .sockets
                .iter()
                .position(|s| s.name.as_bytes().eq_ignore_ascii_case(a.socket.as_bytes()))
            else {
                continue;
            };
            *placed = view.attach(
                &files.body,
                socket,
                Vec3::from(a.pos_off),
                Quat::IDENTITY,
                child,
            );
            child.rotation = crate::attach::turned(child.rotation, a.rot_off);
            child.scale = Vec3::from(a.scale);
        }
    }

    /// shop-model: builds and places the extra pieces of character `obj` (call after its update of the frame): a
    /// piece without a socket is posed with the body by node name (as the face), a socket piece sits at that body
    /// socket. A piece whose model does not load, or whose socket the body lacks, is not drawn.
    pub fn place_body_pieces(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        obj: u16,
        want: &[BodyPiece],
    ) {
        let Some(l) = self.live.get_mut(&obj) else {
            return;
        };
        let mut pieces = std::mem::take(&mut l.pieces);
        let same = pieces.len() == want.len() && pieces.iter().zip(want).all(|(a, b)| a.0 == *b);
        if !same {
            pieces = want.iter().map(|p| (p.clone(), None, false)).collect();
        }
        for p in pieces.iter_mut() {
            p.2 = false;
            if p.1.is_none() {
                let up = UploadKey {
                    model: p.0.model.clone(),
                    styles: p.0.styles.clone(),
                    fallback: None,
                };
                if let Fetch::Ready(Some(loaded)) = self.fetch(ctx, renderer, up) {
                    p.1 = Some(self.instance(ctx, renderer, &loaded));
                }
            }
        }
        let l = self.live.get_mut(&obj).expect("checked above");
        for (piece, inst, placed) in pieces.iter_mut() {
            let Some(child) = inst.as_mut() else { continue };
            match &piece.socket {
                None => {
                    // CLI-1346: head gear and hair skin in the body's bind space (`follow_skinned`)
                    child.follow_skinned(&l.body);
                    *placed = true;
                }
                Some(name) => {
                    let Some(i) = l
                        .body_files
                        .body
                        .sockets
                        .iter()
                        .position(|s| s.name.as_bytes().eq_ignore_ascii_case(name.as_bytes()))
                    else {
                        continue;
                    };
                    // CLI-1171 (playtest report: "his mask is above his head looking backwards"): a socket piece is a
                    // model object of its own and plays its first clip, as the held weapon does; drawn in its bind
                    // pose, the masks whose file turns the root node stood above the head facing backwards
                    // SPEC: UDP_SESSION_2009_B.md §3.5.1 (a model object's main tracker plays its first clip)
                    // DC-DEFAULT: CLI-1171 clip 0 at time 0 for socket pieces (OBSERVED: the reference client's shop figure
                    // wears goggle row 8 on the face, shop-r13/pairs/)
                    if child.trackers.is_empty() {
                        child.set_pose(&[Tracker::main(0, 0.0)]);
                    }
                    *placed =
                        l.body
                            .attach(&l.body_files.body, i, Vec3::ZERO, Quat::IDENTITY, child);
                }
            }
        }
        l.pieces = pieces;
    }

    /// CLI-1231: the gear pieces of character `obj` wearing `ids` (head gear, goggles, backpack), from its body and
    /// face files; rebuilt only when the files or the ids change.
    fn gear_pieces(&mut self, obj: u16, ids: Option<&crate::menu::figure::GearIds>) -> Vec<BodyPiece> {
        let (Some(ids), Some(t), Some(l)) = (ids, self.figure.as_ref(), self.live.get(&obj)) else {
            self.gear.remove(&obj);
            return Vec::new();
        };
        let Some(body) = l.key.0.first() else {
            return Vec::new();
        };
        let key: GearKey = (body.model.clone(), l.key.0.get(1).map(|f| f.model.clone()), *ids);
        if let Some((k, v)) = self.gear.get(&obj) {
            if *k == key {
                return v.clone();
            }
        }
        let pieces = t.gear_for_body(&key.0, key.1.as_deref(), ids);
        self.gear.insert(obj, (key, pieces.clone()));
        pieces
    }

    /// CLI-1072: the pickups each drawn character carries (a capture flag at its body socket, by socket index), and
    /// CLI-1231: the gear its look record names (head gear, goggles, backpack), through the shop figure's piece code;
    /// a character without either keeps no extra piece.
    pub fn place_carried(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        lists: &BTreeMap<u16, Vec<(ModelRef, u32)>>,
        gear: &BTreeMap<u16, crate::menu::figure::GearIds>,
    ) {
        self.gear.retain(|o, _| self.live.contains_key(o));
        let objs: Vec<u16> = self
            .live
            .iter()
            .filter(|(_, l)| !l.culled)
            .map(|(o, _)| *o)
            .collect();
        for obj in objs {
            let mut want = self.gear_pieces(obj, gear.get(&obj));
            let carried: Vec<BodyPiece> = match (lists.get(&obj), self.live.get(&obj)) {
                (Some(list), Some(l)) => list
                    .iter()
                    .filter_map(|(m, socket)| {
                        let s = l.body_files.body.sockets.get(*socket as usize)?;
                        Some(BodyPiece {
                            model: m.clone(),
                            styles: Vec::new(),
                            socket: Some(String::from_utf8_lossy(s.name.as_bytes()).into_owned()),
                        })
                    })
                    .collect(),
                _ => Vec::new(),
            };
            want.extend(carried);
            if want.is_empty() && self.live.get(&obj).is_some_and(|l| l.pieces.is_empty()) {
                continue;
            }
            self.place_body_pieces(ctx, renderer, obj, &want);
        }
    }

    /// shop-model: lights every model of character `obj` (body, face, weapon and its mods, pieces) with `lights`
    /// (call after the frame's placement: a placement copies the body's world light set).
    pub fn set_object_lights(&mut self, obj: u16, lights: &LightSet) {
        let Some(l) = self.live.get_mut(&obj) else {
            return;
        };
        l.body.lights = lights.clone();
        for m in l
            .face
            .iter_mut()
            .chain(l.weapon.iter_mut())
            .chain(l.attach.iter_mut().filter_map(|a| a.1.as_mut()))
            .chain(l.pieces.iter_mut().filter_map(|p| p.1.as_mut()))
        {
            m.lights = lights.clone();
        }
    }

    /// shop-model: the extra pieces of character `obj` drawn this frame (model file, socket).
    pub fn body_pieces_drawn(&self, obj: u16) -> Vec<(String, Option<String>)> {
        self.live.get(&obj).map_or(Vec::new(), |l| {
            l.pieces
                .iter()
                .filter(|p| p.2 && p.1.is_some())
                .map(|p| (p.0.model.model.clone(), p.0.socket.clone()))
                .collect()
        })
    }

    /// Builds and places the third-person mod models on the remote characters' weapons (call after the remotes'
    /// update of the frame): each at its socket of the weapon model, by character object (CLI-618).
    pub fn place_remote_attachments(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        lists: &BTreeMap<u16, Vec<crate::attach::WeaponAttach>>,
    ) {
        let objs: Vec<u16> = self
            .live
            .iter()
            .filter(|(_, l)| !l.culled)
            .map(|(o, _)| *o)
            .collect();
        for obj in objs {
            let want = lists.get(&obj).map(Vec::as_slice).unwrap_or(&[]);
            let mut attach = std::mem::take(&mut self.live.get_mut(&obj).expect("listed").attach);
            let same =
                attach.len() == want.len() && attach.iter().zip(want).all(|(a, b)| a.0 == *b);
            if !same {
                attach = want.iter().map(|a| (a.clone(), None, false)).collect();
            }
            for a in attach.iter_mut() {
                a.2 = false;
                if a.1.is_none() {
                    let up = self.key_for(&a.0.model);
                    let up = UploadKey {
                        styles: if a.0.styles.is_empty() {
                            up.styles
                        } else {
                            a.0.styles.clone()
                        },
                        ..up
                    };
                    if let Fetch::Ready(Some(l)) = self.fetch(ctx, renderer, up) {
                        a.1 = Some(self.instance(ctx, renderer, &l));
                    }
                }
            }
            let l = self.live.get_mut(&obj).expect("listed");
            if let (Some(w), Some(files)) = (l.weapon.as_ref(), l.weapon_files.as_ref()) {
                for (a, inst, placed) in attach.iter_mut() {
                    let Some(child) = inst.as_mut() else { continue };
                    let Some(socket) =
                        files.body.sockets.iter().position(|s| {
                            s.name.as_bytes().eq_ignore_ascii_case(a.socket.as_bytes())
                        })
                    else {
                        continue;
                    };
                    *placed = w.attach(
                        &files.body,
                        socket,
                        Vec3::from(a.pos_off),
                        Quat::IDENTITY,
                        child,
                    );
                    child.rotation = crate::attach::turned(child.rotation, a.rot_off);
                    child.scale = Vec3::from(a.scale);
                }
            }
            l.attach = attach;
        }
    }

    /// A remote character's posed weapon model and its file, and its mod models placed this frame (for checks).
    pub fn remote_weapon_parts(
        &self,
        obj: u16,
    ) -> Option<(
        &ModelInstance,
        &dc_render::ModelFiles,
        Vec<(&crate::attach::WeaponAttach, &ModelInstance)>,
    )> {
        let l = self.live.get(&obj)?;
        let parts = l
            .attach
            .iter()
            .filter(|a| a.2)
            .filter_map(|a| Some((&a.0, a.1.as_ref()?)))
            .collect();
        Some((l.weapon.as_ref()?, l.weapon_files.as_deref()?, parts))
    }

    /// The mod models drawn on the remote characters' weapons this frame: object, model file, socket.
    pub fn remote_attachments_drawn(&self) -> Vec<(u16, String, String)> {
        self.live
            .iter()
            .flat_map(|(o, l)| {
                l.attach
                    .iter()
                    .filter(|a| a.2 && a.1.is_some())
                    .map(|a| (*o, a.0.model.model.clone(), a.0.socket.clone()))
            })
            .collect()
    }

    /// The mod models drawn on the first-person weapon this frame: model file, socket name and eye-space position.
    pub fn view_attachments_drawn(&self) -> Vec<(String, String, Vec3)> {
        if !self.stats.view_weapon || self.view_hidden {
            return Vec::new();
        }
        self.view_attach
            .iter()
            .filter(|a| a.2)
            .filter_map(|a| {
                let i = a.1.as_ref()?;
                Some((a.0.model.model.clone(), a.0.socket.clone(), i.position))
            })
            .collect()
    }

    /// Objects drawn with a model this frame, and the ones whose model is still loading (drawn with nothing); their
    /// boxes are left out unless the debug boxes are on.
    pub fn drawn(&self) -> Vec<u16> {
        let mut v: Vec<u16> = self.live.keys().copied().collect();
        v.extend(self.loading.iter().copied());
        v
    }

    /// Moves, builds and poses the dropped weapons of this frame ([`crate::drops`]): the motion advanced to `now_ms`
    /// against `world`, each drop with the model its object names.
    pub fn update_drops(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        drops: &[crate::drops::WeaponDrop],
        now_ms: u32,
        world: Option<&dyn dc_sim::Collider>,
        lights: Option<&WorldLights>,
    ) {
        self.drain();
        let placed = self.drops.world.advance(drops, now_ms, world);
        self.drops.keep(&placed);
        for p in &placed {
            let Some(m) = p.model.as_ref() else {
                continue;
            };
            if self.drops.needs(p.obj, m) {
                match self.fetch(ctx, renderer, self.key_for(m)) {
                    Fetch::Ready(Some(l)) => {
                        let inst = self.instance(ctx, renderer, &l);
                        self.drops.insert(p.obj, m.clone(), inst);
                    }
                    Fetch::Ready(None) => self.drops.fail(p.obj, m),
                    // still loading: not drawn yet
                    Fetch::Pending => {}
                }
            }
            let key = SunObject::Drop(p.obj);
            self.drops
                .pose(p, &|pos, r| light_set_for(key, pos, r, lights));
        }
    }

    /// client-destructfx: builds and places the effect models of this frame ([`crate::fx::FxRunner::models_at`]): each
    /// through the upload cache (a model still loading is not drawn yet), lit at its place.
    pub fn update_fx_models(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        list: &[crate::fxkeys::FxModel],
        lights: Option<&WorldLights>,
    ) {
        self.drain();
        self.fx_models.keep(list);
        for m in list {
            if self.fx_models.needs(m) {
                // the key's own render styles when it names any (clientfx+0x17290 slots 0..3), else the tables'
                let mut key = self.key_for(&m.model);
                if !m.styles.is_empty() {
                    key.styles = m.styles.clone();
                }
                match self.fetch(ctx, renderer, key) {
                    Fetch::Ready(Some(l)) => {
                        let inst = self.instance(ctx, renderer, &l);
                        self.fx_models.insert(m, inst);
                    }
                    Fetch::Ready(None) => self.fx_models.fail(m.id),
                    Fetch::Pending => {}
                }
            }
            // effect models live about 2 s and move every frame: the exact-position memo, not the per-object sun cache
            self.fx_models
                .place(m, &|pos, r| light_set_memo(pos, r, lights));
        }
    }

    /// Dropped weapons drawn with a model in the last [`ModelScene::update_drops`].
    pub fn drops_drawn(&self) -> u32 {
        self.drops.drawn
    }

    /// The pickup set of [`ModelScene::update_drops`] (drawn instances, hidden pieces).
    pub fn drop_models(&self) -> &crate::drops::DropModels {
        &self.drops
    }

    /// Counts the instances of the last update.
    pub fn count(&mut self) -> usize {
        let n = self.instances().len() + self.view_instances().len();
        self.stats.instances = n as u32;
        n
    }
}

/// The light set at a position through the exact-position memo (`WorldLights::light_set`), for short-lived objects.
fn light_set_memo(pos: Vec3, radius: f32, lights: Option<&WorldLights>) -> LightSet {
    match lights {
        Some(l) => l.light_set(pos, radius),
        None => LightSet {
            ambient: Vec3::splat(255.0),
            lights: Vec::new(),
        },
    }
}

/// The light set of one model object with the 2009 per-object sun cache (client-fps, `WorldLights::light_set_for`).
fn light_set_for(
    object: SunObject,
    pos: Vec3,
    radius: f32,
    lights: Option<&WorldLights>,
) -> LightSet {
    match lights {
        Some(l) => l.light_set_for(object, pos, radius),
        None => LightSet {
            ambient: Vec3::splat(255.0),
            lights: Vec::new(),
        },
    }
}

/// Where a character model's origin sits: at the bottom of the object's box (the stream position is the box centre).
// DC-DEFAULT: CLI-40 (proposed). OBSERVED: the ROM default body's standing pose spans 0..84 units above its origin
// (2026-10-25, tests/remote_models.rs prints the bounds); the 2009 client reports its eye 33 above the position with
// the half height 50 (CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (g)): 83 above the box bottom, the model's head height.
// How the 2009 engine places a model against its object's box is not read (PLAUSIBLE).
pub fn model_origin(centre: [f32; 3], half: [f32; 3]) -> Vec3 {
    Vec3::new(centre[0], centre[1] - half[1], centre[2])
}

/// CLI-1635: the view a character must touch to be posed and drawn: a sphere of [`CULL_RADIUS`] around its object
/// position against the four side planes and the near plane of the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewCull {
    planes: [glam::Vec4; 5],
}

/// CLI-1635: far more than a character reaches from its box centre with weapon, gear and a corpse's fall, so nothing
/// that could show is left out.
// DC-DEFAULT: CLI-1635 (proposed): the model's standing height is about 84 units (CLI-40)
pub const CULL_RADIUS: f32 = 250.0;

impl ViewCull {
    pub fn of(camera: &dc_render::Camera) -> ViewCull {
        let m = camera.view_projection().transpose();
        let (x, y, z, w) = (m.x_axis, m.y_axis, m.z_axis, m.w_axis);
        let unit = |p: glam::Vec4| p / p.truncate().length().max(1e-6);
        ViewCull {
            planes: [unit(w + x), unit(w - x), unit(w + y), unit(w - y), unit(z)],
        }
    }

    /// Whether the sphere around `pos` lies wholly outside the view.
    pub fn outside(&self, pos: [f32; 3]) -> bool {
        let p = glam::Vec4::new(pos[0], pos[1], pos[2], 1.0);
        self.planes.iter().any(|n| n.dot(p) < -CULL_RADIUS)
    }
}

/// Places and poses one character: the object position (the box centre the stream carries; the model origin at the
/// box bottom, CLI-40) and yaw, the trackers of the stream, the face following the body, the weapon at its socket.
// SPEC: UDP_SESSION_2009_B.md §4.3 (position, yaw8, trackers), §4.2 item 8 (attachment: child, socket, offset,
// rotation; ours: offset 0, rotation 1 as the server sends them)
/// CLI-1635: a character outside the view: the body and its weapon follow the object with the last pose (a remote's
/// muzzle effect still starts at his place), nothing else runs; the first frame back in the view poses it again.
fn place(l: &mut Live, r: &RemoteBody) {
    l.body.rotation = Quat::from_rotation_y(r.yaw.unwrap_or(0.0));
    l.body.position = model_origin(r.pos, r.half);
    if let (Some(w), Some((node, local))) = (l.weapon.as_mut(), l.socket) {
        let g = l.body.globals.get(node).copied().unwrap_or(Mat4::IDENTITY);
        let m = l.body.world() * g * local;
        let (_, rot, pos) = m.to_scale_rotation_translation();
        w.rotation = rot.normalize();
        w.position = pos;
    }
}

fn pose(l: &mut Live, r: &RemoteBody, lights: Option<&WorldLights>) {
    let yaw = r.yaw.unwrap_or(0.0);
    l.body.rotation = Quat::from_rotation_y(yaw);
    l.body.position = model_origin(r.pos, r.half);
    let trackers: Vec<Tracker> = r
        .look
        .as_ref()
        .map(|lk| {
            lk.trackers
                .iter()
                .map(|t| Tracker {
                    clip: t.clip,
                    time_ms: t.time_ms,
                    blend_set: t.blend_set,
                })
                .collect()
        })
        .unwrap_or_default();
    l.body.set_pose(&trackers);
    aim_body(l, r);
    // the light set at the object position (the stream position = the box centre), not at the model origin
    // EVIDENCE: engine+0x1b21b0 (the builder samples the grid and gathers the lights at object +0xc8..+0xd0, the
    // object position; the model radius +0x1e8 -> +0x48 sizes the static-light query box)
    // DC-DEFAULT: CLI-108 (proposed) the object position = the stream's box centre (CLI-40 puts the model origin at
    // the box bottom, which sampled the light at the feet)
    l.body.lights = light_set_for(
        SunObject::Character(r.obj),
        Vec3::from(r.pos),
        l.body.radius,
        lights,
    );
    if let Some(f) = l.face.as_mut() {
        f.follow_in(&l.body, FACE_IN_BODY_SPACE);
    }
    if let (Some(w), Some((node, local))) = (l.weapon.as_mut(), l.socket) {
        let g = l.body.globals.get(node).copied().unwrap_or(Mat4::IDENTITY);
        let m = l.body.world() * g * local;
        let (_, rot, pos) = m.to_scale_rotation_translation();
        w.rotation = rot.normalize();
        w.position = pos;
        w.lights = l.body.lights.clone();
    }
}

/// Turns the posed body's torso, neck, head and shoulders by the character's aim ([`crate::aim`]) and rebuilds the
/// bone matrices; a body without aim notes keeps its pose.
// EVIDENCE: cshell+0x26600 (the character object's frame update runs the aim easing cshell+0x256f0 and hands the
// drawn angles to the node controllers, cshell+0x24340)
fn aim_body(l: &mut Live, r: &RemoteBody) {
    let Some(aim) = r.look.as_ref().and_then(|lk| lk.aim) else {
        return;
    };
    crate::aim::apply(&l.body.anim.skeleton, &mut l.body.globals, aim);
    l.body.bones = l
        .body
        .globals
        .iter()
        .zip(&l.body.anim.skeleton.inverse_bind)
        .map(|(g, ib)| *g * *ib)
        .collect();
}

impl ModelScene {
    /// [`ModelScene::update_remotes`] for the roster characters and the corpses of `anims`, posed by the 2009
    /// tracker rules: a clip change blends from the previous clip over the new clip's blend time
    /// ([`crate::anims::ClipBlends`]), and a tracker the server marked as playing once (the death clip, one-shot
    /// upper-body clips) stops on its clip's last frame instead of wrapping. Clip lengths and blend times come from
    /// the model built for the object (a body seen for the first time is built and posed as streamed first).
    // SPEC: UDP_SESSION_2009_B.md §3.5.5 (corpse object), §4.3, §4.4 (state bit 1); DC-DEFAULT: CLI-180, CLI-182
    // (proposed)
    pub fn update_remotes_animated(
        &mut self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        remotes: &[RemoteBody],
        anims: &crate::anims::AnimFrame,
        lights: Option<&WorldLights>,
    ) {
        let all: Vec<&RemoteBody> = remotes.iter().chain(&anims.corpses).collect();
        self.update_bodies(ctx, renderer, &all, lights);
        let drawn: Vec<u16> = self.live.keys().copied().collect();
        self.anim_blends.retain(&drawn);
        for &r in &all {
            let (Some(look), Some(l)) = (r.look.as_ref(), self.live.get_mut(&r.obj)) else {
                continue;
            };
            if !look.visible {
                continue;
            }
            let set = &l.body.anim;
            let mut layers = self.anim_blends.step(
                r.obj,
                &look.trackers,
                |c| set.clip(c).map(|c| c.blend_ms),
                |c| set.clip_length_ms(c),
            );
            let once = anims.once_of(r.obj);
            let mut shown: Vec<_> = layers.iter().map(|x| x.tracker).collect();
            crate::anims::hold_once(&mut shown, once, |c| set.clip_length_ms(c));
            for (x, t) in layers.iter_mut().zip(shown) {
                x.tracker = t;
            }
            let blending = layers.iter().any(|x| x.from.is_some());
            let moved = layers
                .iter()
                .zip(&look.trackers)
                .any(|(x, t)| x.tracker.time_ms != t.time_ms);
            if (blending || moved) && !l.culled {
                pose_layers(l, r, &layers, lights);
            }
        }
    }
}

// ---- client-hudgaps: the team-mate mark on the body
impl ModelScene {
    /// The body's texture list as the 2009 client rebuilds it for the drawn character: the first team-mark slot
    /// (the texture after the skins, the one the body's mark piece samples) becomes team mark 1 for a character of
    /// the local player's team and team mark 0 for any other; the other slots stay as given.
    // EVIDENCE: cshell+0x206730 (character rebuild: mark index 1 when the passed id equals the character's team,
    // slot +0x7c, else 0), cshell+0x1c2f10 (texture list = the row's skins, then the team mark of that index, then the
    // two emblem slots), cshell+0x2a5770 (team mark k of the mark table); OBSERVED: CA_ROM body model 2026-10-26 (the
    // mark piece samples texture slot 2 with the row's alpha-tested style; mark 0 is fully transparent, mark 1 the
    // band), arena 20261026_032409 pairs/mate_close.png (the reference stack draws the band on the team mate only)
    // DC-DEFAULT: CLI-283 (proposed) the passed id is the local player's team (the caller is not traced); the slot
    // replaced is the first texture equal to a team mark of the table (the server names mark 0 there, DSV-24)
    fn team_marked(&self, mut models: Vec<ModelRef>, team: u8) -> Vec<ModelRef> {
        let Some(t) = self.table.as_ref() else {
            return models;
        };
        let (Some(m0), Some(m1)) = (t.marks.get(&0), t.marks.get(&1)) else {
            return models;
        };
        let mate = self
            .own_team
            .is_some_and(|o| o == team && team != 0 && team != 0xff);
        let want = if mate { m1 } else { m0 };
        if let Some(body) = models.first_mut() {
            if t.row_of_model(&body.model).is_some() {
                let is_mark = |s: &String| t.marks.values().any(|m| m.eq_ignore_ascii_case(s));
                if let Some(slot) = body.textures.iter_mut().find(|s| is_mark(s)) {
                    *slot = want.clone();
                }
            }
        }
        models
    }
}

/// [`pose`] with per-tracker blend layers: the pose of the shown trackers, mixed per node toward the pose with a
/// blending tracker's previous clip by `1 - fraction`; then the face and the weapon follow the body as in [`pose`].
// DC-DEFAULT: CLI-182 (proposed) whole-pose mix (EVIDENCE engine+0x2e10 for the fraction)
fn pose_layers(
    l: &mut Live,
    r: &RemoteBody,
    layers: &[crate::anims::Layer],
    lights: Option<&WorldLights>,
) {
    let tr = |t: &crate::world::RemoteTracker| Tracker {
        clip: t.clip,
        time_ms: t.time_ms,
        blend_set: t.blend_set,
    };
    let current: Vec<Tracker> = layers.iter().map(|x| tr(&x.tracker)).collect();
    // the same pose the shots' node spheres use (CLI-300)
    let locals = crate::hitnodes::layered_pose(&l.body.anim, layers);
    // place and light as the plain pose does, then replace the skeleton pose
    pose(l, r, lights);
    l.body.trackers = current;
    l.body.globals = l.body.anim.skeleton.globals(&locals);
    if let Some(aim) = r.look.as_ref().and_then(|lk| lk.aim) {
        crate::aim::apply(&l.body.anim.skeleton, &mut l.body.globals, aim);
    }
    l.body.bones = l
        .body
        .globals
        .iter()
        .zip(&l.body.anim.skeleton.inverse_bind)
        .map(|(g, ib)| *g * *ib)
        .collect();
    if let Some(f) = l.face.as_mut() {
        f.follow_in(&l.body, FACE_IN_BODY_SPACE);
    }
    if let (Some(w), Some((node, local))) = (l.weapon.as_mut(), l.socket) {
        let g = l.body.globals.get(node).copied().unwrap_or(Mat4::IDENTITY);
        let m = l.body.world() * g * local;
        let (_, rot, pos) = m.to_scale_rotation_translation();
        w.rotation = rot.normalize();
        w.position = pos;
    }
}

// ---- client-tags2: the point the name over a character's head stands on

/// The caption socket of a body model by name (compared without case): its node and its position in the node's space.
// EVIDENCE: engine+0xd33b0 (socket by name on the object's model), engine+0x6d350 (the socket's position is rotated by
// its node's transform and added to the node's position: only the node and the position decide the point)
// DC-DEFAULT: CLI-370 (proposed) the name compares without case and only the body model's sockets are searched (the
// engine searches the attached models first; the face and weapon models carry no such socket in the user's ROM)
pub fn name_socket_of(body: &dc_model::Model, name: &str) -> Option<(usize, Vec3)> {
    body.sockets
        .iter()
        .find(|s| s.name.as_bytes().eq_ignore_ascii_case(name.as_bytes()))
        .filter(|s| (s.node as usize) < body.nodes.len())
        .map(|s| (s.node as usize, Vec3::from(s.pos)))
}

impl ModelScene {
    /// The world points of the name sockets of the characters drawn with a model, by object, in the pose of the
    /// last [`ModelScene::update_remotes`] / [`ModelScene::update_remotes_animated`] (call it after the frame's
    /// update: the point then follows the drawn head, run lean and crouch included). Characters without the socket
    /// (or before the CShell name could be read) are left out; the HUD keeps their box points.
    // EVIDENCE: cshell+0x89310 (the caption point is the socket's world-space position each frame), engine+0x6d070 /
    // +0x6c4e0 (from the object's posed node transforms)
    pub fn name_points(&self) -> BTreeMap<u16, [f32; 3]> {
        self.live
            .iter()
            .filter(|(_, l)| !l.culled)
            .filter_map(|(obj, l)| {
                let (node, pos) = l.name_at?;
                let g = *l.body.globals.get(node)?;
                let p = l.body.world().transform_point3(g.transform_point3(pos));
                p.is_finite().then_some((*obj, p.to_array()))
            })
            .collect()
    }

    /// Whether the caption socket's name was read from the user's CShell.dll.
    pub fn has_name_socket(&self) -> bool {
        self.name_socket.is_some()
    }
}

/// `DC_ASYNC_LOADS=1` / `0` overrides a caller's default for [`ModelScene::set_async_loads`].
pub fn async_loads_from_env(default: bool) -> bool {
    match std::env::var("DC_ASYNC_LOADS")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("1") => true,
        Some("0") => false,
        _ => default,
    }
}

impl ModelScene {
    /// gpu-recover: forgets every upload of a lost GPU device (templates, the drawn characters, the first-person
    /// model, the drops' and the effect models' instances) and stops the loader threads, which upload onto that
    /// device. The ROM caches (model files, decoded textures, render styles) stay, so the next updates upload again
    /// from memory onto the new device; until then those objects are drawn with nothing (as a model still loading).
    /// The old GPU objects are put away without a drop ([`crate::gpulost::retire`]).
    // DC-DEFAULT: CLI-460 (amended by gpu-recover)
    pub fn gpu_reset(&mut self) {
        crate::gpulost::retire(std::mem::take(&mut self.templates));
        crate::gpulost::retire(std::mem::take(&mut self.live));
        crate::gpulost::retire(self.view.take());
        self.drops.gpu_reset();
        self.fx_models.gpu_reset();
        crate::gpulost::retire(self.showcase.take());
        // dropping the loader closes its job queue: its threads end after their current job
        self.loader = None;
        self.loader_failed = false;
        self.files_pending.clear();
        self.loading.clear();
        self.failed.clear();
    }

    /// Loads models on loader threads (`true`) or inline in the call that first needs them (`false`, the default).
    /// The threads start with the first load.
    // DC-DEFAULT: CLI-262 (proposed), see DC_DEFAULTS.md
    pub fn set_async_loads(&mut self, on: bool) {
        self.async_loads = on;
    }

    /// Models load on loader threads.
    pub fn async_loads(&self) -> bool {
        self.async_loads
    }

    /// Loader threads running (0 before the first load and in the inline mode).
    pub fn loader_threads(&self) -> usize {
        self.loader.as_ref().map_or(0, |l| l.threads)
    }

    /// Loads requested and not handed back yet.
    pub fn pending_loads(&self) -> usize {
        self.loader.as_ref().map_or(0, |l| l.outstanding())
    }

    /// Objects whose model was still loading in the last update (drawn with nothing).
    pub fn loading(&self) -> &[u16] {
        &self.loading
    }

    /// Waits for every requested load. The next update then draws what the inline mode draws (tests and PNG recipes
    /// in the async mode: update, wait, update again).
    pub fn wait_for_loads(&mut self) {
        if let Some(l) = self.loader.as_mut() {
            let done = l.wait();
            self.take(done);
        }
    }

    /// The files of a clip map's two models are read (always true in the inline mode, which reads them in the map);
    /// in the async mode a missing one is requested from the loader threads and `false` returned.
    fn clip_map_ready(&mut self, from: &str, to: &str) -> bool {
        if !self.async_loads || self.loader.is_none() || key_of(from) == key_of(to) {
            return true;
        }
        if self.clip_maps.contains_key(&(key_of(from), key_of(to))) {
            return true;
        }
        let mut ready = true;
        for p in [from, to] {
            if self.shared.files_cached(p).is_some() {
                continue;
            }
            ready = false;
            let k = key_of(p);
            if !self.files_pending.contains(&k) {
                if let Some(l) = self.loader.as_mut() {
                    if l.read_files(p.to_string()) {
                        self.files_pending.insert(k);
                    }
                }
            }
        }
        ready
    }
}

/// The streaming telemetry's id of an upload key (a fixed-key hash: the same key gives the same id in every run).
fn stream_id(key: &UploadKey) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blocks_body_styles_win_over_the_tables() {
        // CLI-1150: the class-11 infected body is drawn with the styles the server's block names
        let table = UploadKey {
            model: ModelRef {
                model: "body.ltb".into(),
                textures: Vec::new(),
            },
            styles: vec!["normal.ltb".into()],
            fallback: None,
        };
        let k = body_key(table.clone(), &["infected.ltb".to_string()]);
        assert_eq!(k.styles, vec!["infected.ltb".to_string()]);
        assert_eq!(k.model, table.model);
        assert_eq!(
            body_key(table.clone(), &[]),
            table,
            "no block styles: the table's"
        );
    }
}
