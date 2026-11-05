//! Headless run: the client's frame loop with a fixed frame interval and (by default) the scripted input,
//! rendering frames offscreen to PNG files and checking the simulation on every tick. Against a game server
//! (a [`crate::net::NetWorld`] source) the frames are paced to the wall clock ([`HeadlessOptions::realtime`]).

use crate::characters::ModelScene;
use crate::debug_draw::{BoxInstance, DebugBoxes};
use crate::error::ClientError;
use crate::frame::{apply_view_set, overlay_lines, remote_boxes, ViewSettings};
use crate::input::{InputSource, ScriptedInput, ServerHold};
use crate::look::ViewAngles;
use crate::offscreen::Offscreen;
use crate::overlay::{font_from_bytes, load_rom_font, OverlayFont, OverlayRenderer};
use crate::player::LocalPlayer;
use crate::update::LocalUpdate;
use crate::world::{ArmsFrame, GpuTarget, SpawnPoint, WorldSource};
use dc_render::{
    AdapterChoice, FrameImage, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT,
};
use dc_sim::{MovementParams, V3};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::time::Instant;

/// World-camera frames kept in [`HeadlessReport::cam_shot_marks`].
pub const CAM_SHOT_MARKS: usize = 4096;

/// Options of a headless run.
#[derive(Debug, Clone)]
pub struct HeadlessOptions {
    /// CLI-1619: the server's mods for the match, as a window run gets them (`None`: no mods).
    pub mods: Option<crate::mods::HostRecipe>,
    /// LG-274 / CLI-1627: the room's Hacks setting when the lobby offered one.
    pub room_hacks: Option<u32>,
    /// Frames to run.
    pub frames: u32,
    /// MAP-95: draw from this place (eye, yaw, pitch; radians, positive pitch looks down) instead of the player's
    /// eye. The match itself still runs from the spawn.
    pub camera: Option<([f32; 3], f32, f32)>,
    /// Wall time per frame (microseconds). Keep it at or below 8 ticks (dc-sim's FixedStep drops a longer
    /// backlog).
    pub frame_us: u64,
    pub width: u32,
    pub height: u32,
    /// Where PNGs go; `None` renders without writing.
    pub out: Option<PathBuf>,
    /// File name prefix of the PNGs (`<prefix>NNNN.png`).
    pub prefix: String,
    /// Render and save every n-th frame (and the last one).
    pub save_every: u32,
    /// Render at all (false: simulation only, no GPU).
    pub render: bool,
    pub adapter: AdapterChoice,
    pub tick_hz: u32,
    pub spawn: usize,
    pub view: ViewSettings,
    pub params: MovementParams,
    /// ROM folder for the overlay font (`None`: no overlay text unless `font_bytes` is given).
    pub font_rom: Option<PathBuf>,
    /// A font file to use when `font_rom` is `None` (tests, user-supplied fonts).
    pub font_bytes: Option<Vec<u8>>,
    /// Pace the frames to the wall clock (one frame per `frame_us`): needed against a game server, whose clock is
    /// the wall clock.
    pub realtime: bool,
    /// End the run this many frames after the source said its game session is over (`None`: run all `frames`).
    pub stop_after_over: Option<u32>,
    /// Draw remote characters and the own weapon with their models when the source names a ROM (else boxes).
    pub models: bool,
    /// Draw the team-coloured boxes of remote players also where a model is drawn (`--debug-boxes`).
    pub debug_boxes: bool,
    /// The in-match HUD (off by default; the debug panel stays unless `debug_panel` is false).
    pub hud: crate::hud::HudChoice,
    /// Show the scoreboard (the window shows it while its key is held).
    pub show_board: bool,
    /// Draw the debug text panel.
    pub debug_panel: bool,
    /// The sounds (director on by default; no output device: the mixer is a null backend).
    pub sound: crate::sound::SoundOptions,
    /// Keep the body position after every simulation tick ([`HeadlessReport::trace`]).
    pub trace: bool,
    /// Render and save one PNG at each match-loop state the first time it is reached (first damage, death after
    /// [`DEATH_SHOT_MS`], respawn, result screen after [`RESULT_SHOT_MS`]): `<prefix><state>.png` in `out`
    /// ([`HeadlessReport::state_images`]).
    pub state_shots: bool,
    /// The exact lane (docs/RENDER_DETERMINISM_CONTRACT.md): record the texel hash of every rendered frame in
    /// [`HeadlessReport::frame_hashes`] and load models inline (never on loader threads, whatever `DC_ASYNC_LOADS`
    /// says), so that what a frame shows depends on the frame index only. Off: no hashing, no cost.
    pub hash_frames: bool,
}

/// One rendered frame's exact-lane hash ([`HeadlessOptions::hash_frames`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHash {
    pub frame: u32,
    pub width: u32,
    pub height: u32,
    /// [`dc_render::texel_hash`] of the read-back RGBA8 texels (padding stripped, alpha 255, gamma ramp applied).
    pub sha256: [u8; 32],
}

/// A frame of a wall-clock-paced run ([`HeadlessOptions::realtime`]) that started at least this long after the
/// previous one is logged as a stall ([`HeadlessReport::stalls`]): the machine was busy (other processes, a shader
/// compile, a slow read-back), the frames in between were not run on time and the run catches up afterwards.
pub const STALL_MS: u32 = 250;
/// Stalls kept in [`HeadlessReport::stalls`].
pub const STALLS_KEPT: usize = 256;
/// PNG files waiting for the writer thread before a save blocks the frame loop.
pub const PNG_QUEUE: usize = 8;

/// A late frame of a wall-clock-paced run (see [`STALL_MS`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStall {
    /// The frame that started late.
    pub frame: u32,
    /// Wall time since the previous frame started (ms).
    pub wall_ms: u32,
    /// The source's session clock at the previous frame and at this one (`None`: the source has no clock).
    pub clock_before: Option<u32>,
    pub clock_after: Option<u32>,
}

/// The session clock at the end of one frame of a run against a clocked source: the simulation ticks run so far and
/// the source's clock (ms). Tests map their tick-driven input onto the server's clock with these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockMark {
    pub frame: u32,
    pub ticks: u64,
    pub clock_ms: u32,
}

/// The PNG files of a run, encoded and written on a worker thread so that a slow encoder (unoptimised test builds,
/// a busy disk) never holds a wall-clock-paced frame. Every file exists when [`run_headless`] returns.
struct PngWriter {
    tx: Option<SyncSender<(FrameImage, PathBuf)>>,
    worker: Option<std::thread::JoinHandle<Vec<String>>>,
}

impl PngWriter {
    fn start() -> PngWriter {
        let (tx, rx) = sync_channel::<(FrameImage, PathBuf)>(PNG_QUEUE);
        let worker = std::thread::Builder::new()
            .name("dc-headless-png".into())
            .spawn(move || {
                let mut errors = Vec::new();
                for (img, path) in rx {
                    if let Err(e) = img.save_png(&path) {
                        errors.push(format!("{}: {e}", path.display()));
                    }
                }
                errors
            });
        match worker {
            Ok(w) => PngWriter {
                tx: Some(tx),
                worker: Some(w),
            },
            // no thread: the frame loop writes the files itself
            Err(_) => PngWriter {
                tx: None,
                worker: None,
            },
        }
    }

    fn save(&mut self, img: &FrameImage, path: &Path) -> Result<(), ClientError> {
        if let Some(tx) = &self.tx {
            if tx.send((img.clone(), path.to_path_buf())).is_ok() {
                return Ok(());
            }
            self.tx = None;
        }
        img.save_png(path)
            .map_err(|e| ClientError::Io(e.to_string()))
    }

    /// Waits for the queued files; the first write error, if any.
    fn finish(mut self) -> Result<(), ClientError> {
        self.tx = None;
        let Some(w) = self.worker.take() else {
            return Ok(());
        };
        let errors = w
            .join()
            .map_err(|_| ClientError::Io("the PNG writer thread panicked".into()))?;
        match errors.into_iter().next() {
            Some(e) => Err(ClientError::Io(e)),
            None => Ok(()),
        }
    }
}

/// The death picture is taken this long after the 0x67 state 2 (the death camera has turned by then).
pub const DEATH_SHOT_MS: u32 = 1500;
/// The result picture is taken this long after the result screen opened.
pub const RESULT_SHOT_MS: u32 = 300;

/// The HUD and camera state of the match loop at one frame (the headless report keeps one each time it changes).
#[derive(Debug, Clone, PartialEq)]
pub struct HudMark {
    pub frame: u32,
    /// The HUD clock (ms).
    pub at_ms: u32,
    pub life_state: u8,
    pub health: Option<f32>,
    pub armour: Option<f32>,
    /// Some damage-indicator sector is lit.
    pub damage_lit: bool,
    /// The aim cross's hit flash is running.
    pub flash: bool,
    /// The own death's killer as the HUD keeps it (`Some(None)`: died to nobody).
    pub killer: Option<Option<String>>,
    /// The respawn countdown in whole seconds, while it runs.
    pub respawn_secs: Option<u32>,
    pub result_screen: bool,
    pub death_cam: Option<crate::deathcam::DeathCamMode>,
    /// The death panel is drawn ([`crate::hud::death::panel_shown`]).
    pub death_panel: bool,
    /// The last 0x74 taken while alive (damage, `dmg_kind`), as the death panel keeps it.
    pub wound: Option<(u32, u8)>,
    /// The result board of 0x46 is drawn (3000 ms after the result, [`crate::hud::HudState::result_board_shown`]).
    pub result_board: bool,
}

impl HudMark {
    fn same_state(&self, o: &HudMark) -> bool {
        (
            self.life_state,
            self.health,
            self.armour,
            self.damage_lit,
            self.flash,
            &self.killer,
            self.respawn_secs,
            self.result_screen,
            self.death_cam,
            self.death_panel,
            self.wound,
            self.result_board,
        ) == (
            o.life_state,
            o.health,
            o.armour,
            o.damage_lit,
            o.flash,
            &o.killer,
            o.respawn_secs,
            o.result_screen,
            o.death_cam,
            o.death_panel,
            o.wound,
            o.result_board,
        )
    }

    fn of(
        frame: u32,
        s: &crate::hud::HudState,
        cam: Option<crate::deathcam::DeathCamMode>,
    ) -> HudMark {
        HudMark {
            frame,
            at_ms: s.now_ms,
            life_state: s.life_state,
            health: s.health,
            armour: s.armour,
            damage_lit: s.damage.iter().any(|v| *v > 0.0),
            flash: s.hit_flash() > 0.0,
            killer: s.death.as_ref().map(|d| d.killer.clone()),
            respawn_secs: s.respawn_left().map(|(ms, _)| ms.div_ceil(1000)),
            result_screen: s.result_screen.is_some(),
            death_cam: cam,
            death_panel: crate::hud::death::panel_shown(s),
            wound: s.wound.map(|w| (w.damage, w.kind)),
            result_board: s.result_board_shown(),
        }
    }
}

/// One simulation tick of a traced run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TracePoint {
    /// Absolute tick index of the run.
    pub tick: u64,
    /// Body centre after the tick.
    pub pos: V3,
    pub events: dc_sim::StepEvents,
    /// The server's sprint permission as the tick saw it.
    pub sprint_allowed: bool,
}

impl Default for HeadlessOptions {
    fn default() -> Self {
        HeadlessOptions {
            mods: None,
            room_hacks: None,
            frames: 60,
            frame_us: 16_667,
            width: 800,
            height: 600,
            out: None,
            prefix: "frame_".into(),
            save_every: 1,
            render: true,
            adapter: AdapterChoice::Auto,
            tick_hz: dc_sim::DEFAULT_TICK_HZ,
            spawn: 0,
            camera: None,
            view: ViewSettings::default(),
            params: MovementParams::default(),
            font_rom: None,
            font_bytes: None,
            realtime: false,
            stop_after_over: None,
            models: true,
            debug_boxes: false,
            hud: crate::hud::HudChoice::Off,
            show_board: false,
            debug_panel: true,
            trace: false,
            state_shots: false,
            sound: crate::sound::SoundOptions {
                enabled: true,
                log: None,
            },
            hash_frames: false,
        }
    }
}

/// What a headless run did.
#[derive(Debug, Clone, Default)]
pub struct HeadlessReport {
    pub world: String,
    /// The exact-lane hashes of the rendered frames in frame order ([`HeadlessOptions::hash_frames`]; empty
    /// otherwise, and empty for a profile run that skips the read-back).
    pub frame_hashes: Vec<FrameHash>,
    pub adapter: Option<String>,
    /// Why nothing was rendered, when rendering was asked for but no adapter worked.
    pub render_skipped: Option<String>,
    pub font: Option<String>,
    /// Where the HUD layout came from (`None`: no HUD).
    pub hud: Option<String>,
    /// HUD inputs applied and HUD images composed.
    pub hud_events: u64,
    pub hud_composed: u64,
    /// The last HUD image (full target, straight alpha), for tests.
    pub last_hud: Option<crate::overlay::text::RgbaImage>,
    pub frames: u32,
    pub ticks: u64,
    pub spawn: Option<SpawnPoint>,
    pub start: V3,
    pub end: V3,
    pub final_digest: u64,
    /// Ticks whose state had a non-finite component.
    pub nan_ticks: u32,
    /// Frames whose eye or view angles were non-finite.
    pub nan_frames: u32,
    /// Ticks whose body centre crossed a solid surface (a ray from the old to the new centre hit something).
    pub tunnel_ticks: u32,
    /// The body went more than 100 units below the lowest collision geometry.
    pub fell_out: bool,
    pub min_y: f32,
    pub max_y: f32,
    /// Horizontal path length.
    pub path_xz: f32,
    pub jumps: u64,
    pub landings: u64,
    /// Standing on something (or touching a steep contact) at the end.
    pub ended_supported: bool,
    pub images: Vec<PathBuf>,
    /// Mean time per rendered frame (encode + submit + read-back), milliseconds.
    pub render_ms_mean: f64,
    pub last_lines: Vec<String>,
    /// The last rendered frame.
    pub last_frame: Option<FrameImage>,
    /// Server corrections applied (0xa0 teleports).
    pub corrections: u32,
    /// Frames whose source reported at least one remote player.
    pub remote_frames: u32,
    /// Remote boxes drawn over all rendered frames.
    pub boxes_drawn: u64,
    /// Remote players with a non-finite position (skipped).
    pub nan_remotes: u32,
    /// The server's player state at the end (`None`: no server).
    pub life_state: Option<u8>,
    /// First frame at which the source said the game session was over.
    pub session_over_frame: Option<u32>,
    /// Frames with the fire button held.
    pub fire_frames: u32,
    /// Model instances drawn over all rendered frames (remote bodies, faces, weapons, the own weapon).
    pub models_drawn: u64,
    /// Remote characters drawn with a model over all rendered frames.
    pub characters_drawn: u64,
    /// Rendered frames that drew the own weapon's first-person model.
    pub view_weapon_frames: u32,
    /// The weapon effects' counters at the end and the effect quads drawn over the run ([`crate::fx`]).
    pub fx: crate::fx::FxStats,
    pub fx_quads: u64,
    /// Frames with at least one thrown object in the world, and the most at once.
    pub thrown_frames: u32,
    pub thrown_max: usize,
    /// Why no models are drawn (no ROM, open failure), when they are not.
    pub models_note: Option<String>,
    /// Every tick's body position (with [`HeadlessOptions::trace`]).
    pub trace: Vec<TracePoint>,
    /// The step height the mover used at the end (the server's variable, or the code default).
    pub step_height: f32,
    /// Every sound start of the run, the counters and the table note (`None`: sounds off).
    pub sound_log: Vec<crate::sound::SoundLogEntry>,
    pub sound_stats: crate::sound::SoundStats,
    pub sound_note: Option<String>,
    /// The spawn notes the sounds saw: (game time, object, on) (w2-audiocues).
    pub sound_spawn_log: Vec<(u32, u16, bool)>,
    /// The sound director's bounded resources (log writer, decode cache; perf-hitch).
    pub sound_bounds: Option<String>,
    /// The HUD and camera state each time it changed (with a HUD).
    pub hud_marks: Vec<HudMark>,
    /// Frames drawn from the death camera.
    pub death_cam_frames: u32,
    /// Frames drawn from a live world camera (CLI-240).
    pub cam_shot_frames: u32,
    /// The first frames drawn from a world camera: (frame, eye, yaw, pitch, the body held).
    pub cam_shot_marks: Vec<(u32, [f32; 3], f32, f32, bool)>,
    /// CLI-610: the letterbox bar height in target pixels whenever it changed: (frame, HUD clock ms, bar px).
    pub letterbox_marks: Vec<(u32, u32, u32)>,
    /// The pictures of [`HeadlessOptions::state_shots`]: (state, file).
    pub state_images: Vec<(String, PathBuf)>,
    /// Dropped weapons drawn with a model over all rendered frames ([`crate::drops`]).
    pub drops_drawn: u64,
    /// client-destructfx: effect models drawn (summed over frames).
    pub fx_models_drawn: u64,
    /// Placed effects of the server handed to the effect player (CLI-232).
    pub world_effects: u64,
    /// The linked world models drawn at the end (the server's set; all the file's own with a local source).
    pub world_models_drawn: Vec<Vec<u8>>,
    /// Late frames of a wall-clock-paced run (at most [`STALLS_KEPT`]).
    pub stalls: Vec<FrameStall>,
    /// The longest wall time between two frame starts of a wall-clock-paced run (ms).
    pub longest_frame_ms: u32,
    /// One mark per frame against a source with a session clock ([`ClockMark`]).
    pub clock_marks: Vec<ClockMark>,
    /// The weapon the frame source held up for the first-person view, each time it changed: (frame, session clock,
    /// weapon; `None` = no first-person weapon).
    pub view_weapon_marks: Vec<(u32, Option<u32>, Option<u16>)>,
    /// The first-person model the view pass drew, each time it changed on a rendered frame: (frame, model file;
    /// `None` = nothing at the eye).
    pub view_model_marks: Vec<(u32, Option<String>)>,
    /// The scope of the frame, each time it changed: (frame, session clock, zoom level / state / horizontal field).
    pub zoom_marks: Vec<(u32, Option<u32>, crate::world::ZoomFrame)>,
}

impl HeadlessReport {
    /// The session clock (ms) of the first frame whose simulation had run `ticks` ticks (`None`: never, or no
    /// clock).
    pub fn clock_at_tick(&self, ticks: u64) -> Option<u32> {
        self.clock_marks
            .iter()
            .find(|m| m.ticks >= ticks)
            .map(|m| m.clock_ms)
    }

    /// The logged stalls that overlap the session-clock window `from..to` (ms).
    pub fn stalls_within(&self, from: u32, to: u32) -> Vec<FrameStall> {
        self.stalls
            .iter()
            .filter(|s| match (s.clock_before, s.clock_after) {
                (Some(a), Some(b)) => a < to && b > from,
                _ => false,
            })
            .copied()
            .collect()
    }

    /// One line on the late frames of a wall-clock-paced run.
    pub fn stall_note(&self) -> String {
        format!(
            "stalls {} (>= {STALL_MS} ms; longest frame {} ms){}",
            self.stalls.len(),
            self.longest_frame_ms,
            self.stalls
                .iter()
                .take(8)
                .map(|s| format!(
                    " [frame {} {} ms clock {:?}..{:?}]",
                    s.frame, s.wall_ms, s.clock_before, s.clock_after
                ))
                .collect::<String>()
        )
    }

    pub fn summary(&self) -> String {
        format!(
            "{}\n  adapter {} | font {}\n  frames {} ticks {} spawn {:?}\n  start {:?} end {:?} y [{:.1}, {:.1}] path_xz {:.1} jumps {} landings {} supported {}\n  \
             nan ticks {} nan frames {} tunnel ticks {} fell out {} | images {} render {:.2} ms/frame | digest {:#018x}\n  \
             server: corrections {} remote frames {} boxes drawn {} nan remotes {} state {:?} session over at frame {:?}
               models: instances drawn {} characters drawn {} own weapon frames {} fire frames {} {}",
            self.world,
            self.adapter.as_deref().unwrap_or(self.render_skipped.as_deref().unwrap_or("none")),
            self.font.as_deref().unwrap_or("none"),
            self.frames,
            self.ticks,
            self.spawn.map(|s| (s.pos.to_array(), s.yaw)),
            self.start.to_array(),
            self.end.to_array(),
            self.min_y,
            self.max_y,
            self.path_xz,
            self.jumps,
            self.landings,
            self.ended_supported,
            self.nan_ticks,
            self.nan_frames,
            self.tunnel_ticks,
            self.fell_out,
            self.images.len(),
            self.render_ms_mean,
            self.final_digest,
            self.corrections,
            self.remote_frames,
            self.boxes_drawn,
            self.nan_remotes,
            self.life_state,
            self.session_over_frame,
            self.models_drawn,
            self.characters_drawn,
            self.view_weapon_frames,
            self.fire_frames,
            self.models_note.as_deref().unwrap_or("")
        )
    }
}

struct Gpu {
    ctx: GpuContext,
    renderer: WorldRenderer,
    overlay: OverlayRenderer,
    hud_layer: OverlayRenderer,
    boxes: DebugBoxes,
    target: Offscreen,
    models: Option<ModelScene>,
}

/// The boxes drawn for remote players: every one with `debug`, else only the shown ones without a model this frame.
// DC-DEFAULT: CLI-39 (proposed) remote players with a model are drawn without their box unless `--debug-boxes`
pub fn box_list(
    remotes: &[crate::world::RemoteBody],
    drawn: &[u16],
    debug: bool,
) -> Vec<BoxInstance> {
    let keep: Vec<crate::world::RemoteBody> = remotes
        .iter()
        // DC-DEFAULT: CLI-351 (proposed) a hidden object (the dead character, CLI-42 / CLI-350) gets no box either
        .filter(|r| debug || (!drawn.contains(&r.obj) && r.look.as_ref().is_none_or(|l| l.visible)))
        .cloned()
        .collect();
    remote_boxes(&keep).0
}

/// The shot origin and direction of a frame: the body position the 0x64 reports plus its eye offset, the view
/// angles, the control word.
// OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.6 (`origin` = the latest 0x64 trailer position + `eye_offset`)
pub fn shot_aim(state: &dc_sim::PlayerState, view: &ViewAngles) -> crate::arms::Aim {
    let e = crate::update::eye_offset_of(state);
    let p = state.pos.to_array();
    crate::arms::Aim {
        origin: [p[0] + e[0], p[1] + e[1], p[2] + e[2]],
        yaw: view.yaw,
        pitch: view.pitch,
        control: state.controls,
    }
}

/// Adds the weapon's recoil pitch change to the view, within the look's pitch limit.
// SPEC: MOVEMENT_2009.md §13.2 step 6 (|pitch| <= 1.4707963); EVIDENCE: cshell+0x1f5150 / +0x1f3070 (the recoil
// writes the view pitch, controller +0x48)
pub fn apply_recoil(view: &mut ViewAngles, pitch: f32) {
    if pitch != 0.0 && pitch.is_finite() {
        let lim = crate::look::LookParams::default().pitch_limit;
        view.pitch = (view.pitch + pitch).clamp(-lim, lim);
    }
}

/// Adds the weapon's recoil yaw change to the view (kept wrapped, as the mouse look keeps it).
// EVIDENCE: cshell+0x1f2f60 / +0x1f3070 (the yaw kick and its drain write the view yaw, controller +0x4c)
pub fn apply_recoil_yaw(view: &mut ViewAngles, yaw: f32) {
    if yaw != 0.0 && yaw.is_finite() {
        view.yaw = crate::look::wrap_angle(view.yaw + yaw);
    }
}

fn finite_state(s: &dc_sim::PlayerState) -> bool {
    s.pos.is_finite() && s.vel.is_finite() && s.acc.is_finite()
}

/// Runs the client loop headless on `source`. With `input = None` the scripted sequence
/// ([`ScriptedInput`]) starts from the spawn's facing.
pub fn run_headless(
    source: &mut dyn WorldSource,
    input: Option<&mut dyn InputSource>,
    opts: &HeadlessOptions,
) -> Result<HeadlessReport, ClientError> {
    let mut report = HeadlessReport {
        min_y: f32::INFINITY,
        max_y: f32::NEG_INFINITY,
        ..HeadlessReport::default()
    };
    // the frame profiler (`--profile`): every frame rendered, no PNG, the HUD only when due (crate::prof); its load
    // phase starts before the device exists (pipelines, world upload, model tables)
    let prof_on = crate::prof::on();
    let mut prof_rec = crate::prof::Recorder::new();
    if prof_on {
        prof_rec.load_begin();
    }
    let mut gpu = None;
    if opts.render {
        match GpuContext::headless(opts.adapter) {
            Ok(ctx) => {
                let renderer =
                    WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
                let overlay = OverlayRenderer::new(&ctx, OFFSCREEN_FORMAT);
                let hud_layer = OverlayRenderer::new(&ctx, OFFSCREEN_FORMAT);
                let boxes = DebugBoxes::new(&ctx, OFFSCREEN_FORMAT);
                let mut target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, opts.width, opts.height);
                target.set_gamma(crate::display::client_gamma(source.rom().as_deref()));
                report.adapter = Some(ctx.describe());
                gpu = Some(Gpu {
                    ctx,
                    renderer,
                    overlay,
                    hud_layer,
                    boxes,
                    target,
                    models: None,
                });
            }
            Err(e) => report.render_skipped = Some(e.to_string()),
        }
    }
    // MAP-31 (the window.rs pattern): the match's mods before anything reads a file (the world, models, sounds,
    // HUD textures); kept until the run ends, when their drop installs nothing again
    let mods = crate::modassets::MatchMods::open(opts.mods.as_ref());
    let mut world = source.load(gpu.as_ref().map(|g| GpuTarget {
        ctx: &g.ctx,
        renderer: &g.renderer,
    }))?;
    report.world = world.summary();
    if let Some(g) = gpu.as_mut() {
        match (opts.models, source.rom()) {
            (false, _) => report.models_note = Some("models off".into()),
            (true, None) => report.models_note = Some("no ROM: remote players as boxes".into()),
            (true, Some(rom)) => match ModelScene::open(&rom) {
                Ok(mut m) => {
                    // inline loads keep the headless frames as before; DC_ASYNC_LOADS=1: loader threads (never in
                    // the exact lane)
                    m.set_async_loads(
                        !opts.hash_frames && crate::characters::async_loads_from_env(false),
                    );
                    g.models = Some(m)
                }
                Err(e) => report.models_note = Some(format!("models not loaded: {e}")),
            },
        }
    }
    let loaded = match (&gpu, &opts.font_rom, &opts.font_bytes) {
        (None, _, _) => None,
        (Some(_), Some(rom), _) => Some(load_rom_font(rom)),
        (Some(_), None, Some(bytes)) => Some(font_from_bytes(bytes.clone(), 16.0)),
        (Some(_), None, None) => None,
    };
    let font: Option<OverlayFont> = match loaded {
        Some(Ok(f)) => {
            report.font = Some(f.source.clone());
            Some(f)
        }
        Some(Err(e)) => {
            report.font = Some(format!("not loaded: {e}"));
            None
        }
        None => None,
    };
    let hud_font = match (&opts.font_bytes, &opts.font_rom) {
        (Some(b), _) => Some((b.clone(), 0)),
        (None, Some(rom)) => crate::overlay::load_rom_font_bytes(rom),
        (None, None) => None,
    };
    let mut hud = if gpu.is_some() {
        crate::hud::HudDriver::open(&opts.hud, opts.width, opts.height, hud_font)
    } else {
        None
    }
    .map(|(mut d, src)| {
        report.hud = Some(src);
        d.set_reality(source.reality());
        d.set_draw_voice(source.draw_voice());
        // MAP-31: the set's HUD overrides and hooks, as the window wires them
        if let Some(m) = mods.as_ref() {
            m.wire(&mut d);
        }
        if !opts
            .room_hacks
            .is_none_or(|v| v & dc_wire::roomhacks::WALLHACKS != 0)
        {
            d.xray = None;
        }
        d
    });
    let mut sound = opts.sound.enabled.then(|| {
        let rom = source.rom();
        let mut d = crate::sound::SoundDirector::open(rom.as_deref());
        // inline decodes unless DC_ASYNC_LOADS=1 (as the model loads; the samples are the same either way)
        d.set_async_decode(crate::characters::async_loads_from_env(false));
        if let Some(p) = opts.sound.log.as_deref() {
            if let Err(e) = d.log_to(p) {
                d.note = format!("{}; log not written: {e}", d.note);
            }
        }
        d
    });
    // the weapon effects (ROM tables and effect file; nothing without a GPU target)
    let mut fx = gpu
        .as_ref()
        .map(|_| crate::fx::FxStage::open(source.rom().as_deref()));
    let spawn = world.spawn(opts.spawn)?;
    report.spawn = Some(spawn);
    let cw = &world.collision;
    let mut player = LocalPlayer::spawn(opts.params, cw, &spawn, world.stand_dims, opts.tick_hz);
    player.volumes = world.containers.clone();
    player.state.crouch_dims = world.crouch_dims;
    // sim-sprint-jump: the held weapon's and the body's speed rows from the ROM (MOVEMENT_2009.md §7)
    player.load_speed_rows(source.rom().as_deref());
    report.start = player.state.pos;
    let mut view = ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    };
    // the own view yaw offset sent as 0x64 aux[1] (CLI-636)
    let mut twist = crate::aim::OwnTwist::default();
    let mut script = ScriptedInput::new(view);
    // a source with its own input (the stress scenarios, perf-g1) drives the player when the caller gives none
    let mut source_input = if input.is_none() {
        source.scripted_input(view)
    } else {
        None
    };
    let input: &mut dyn InputSource = match (input, source_input.as_deref_mut()) {
        (Some(i), _) => i,
        (None, Some(i)) => i,
        (None, None) => &mut script,
    };
    let mut death_cam = crate::deathcam::DeathCam::default();
    let mut cam_shot = crate::camshot::CamShot::default();
    let mut shots_done: Vec<&'static str> = Vec::new();
    let mut was_dead = false;
    // the mover's surface type, held while airborne
    let mut surface_hold = 0u8;
    let floor_limit = world.floor_limit;
    let save_every = opts.save_every.max(1);
    // the CPU HUD image of the last frame, deferred past the profiler in profile mode
    let mut cpu_hud_after_prof = false;
    let mut render_ms = 0.0;
    let mut rendered = 0u32;
    let (mut nan, mut tunnel, mut fell, mut path) = (0u32, 0u32, false, 0.0f32);
    let (mut min_y, mut max_y) = (report.min_y, report.max_y);
    let started = Instant::now();
    // the frame profiler (`--profile`): every frame rendered, no PNG, the HUD only when due (crate::prof)
    use crate::prof::Stage as PS;
    prof_rec.note = format!("{} {}x{}", world.name, opts.width, opts.height);
    prof_rec.size = (opts.width, opts.height);
    if prof_on {
        prof_rec.adapter = report.adapter.clone().unwrap_or_default();
        prof_rec.load_end(gpu.as_ref().map(|g| &g.ctx.device));
    }
    let mut png = opts.out.as_ref().map(|_| PngWriter::start());
    // `--events` / `--steps` (gate T2): the match's events and the steps' shots
    let mut watch = crate::events::GameWatch::default();
    crate::events::screen("game");
    let mut prev_start: Option<Instant> = None;
    let mut prev_clock: Option<u32> = None;
    // gate T1: the headless loop feeds the heartbeat; the watchdog watches it only when asked (DC_WATCHDOG=1)
    let hb_started = Instant::now();
    if crate::watchdog::enabled_headless() {
        crate::watchdog::arm(
            opts.sound
                .log
                .as_ref()
                .and_then(|p| p.parent().map(|d| d.to_path_buf())),
        );
    }
    crate::watchdog::phase(crate::watchdog::Phase::Headless);
    for f in 0..opts.frames {
        let frame_began = Instant::now();
        crate::watchdog::frame();
        crate::watchdog::selftest(hb_started.elapsed().as_millis() as u64);
        if prof_on {
            prof_rec.begin(gpu.as_ref().map(|g| &g.ctx.device));
        }
        let mut laps = crate::prof::Laps::start();
        input.frame(&mut view);
        laps.lap(PS::Input);
        for c in source.poll_corrections() {
            player.apply_correction(&world.collision, &c);
            report.corrections += 1;
            crate::prof::note(crate::prof::Note::Corrections, 1);
        }
        let mut fs = source.frame_state();
        if opts.realtime {
            if let Some(p) = prev_start {
                let wall_ms = frame_began
                    .duration_since(p)
                    .as_millis()
                    .min(u32::MAX as u128) as u32;
                report.longest_frame_ms = report.longest_frame_ms.max(wall_ms);
                if wall_ms >= STALL_MS && report.stalls.len() < STALLS_KEPT {
                    report.stalls.push(FrameStall {
                        frame: f,
                        wall_ms,
                        clock_before: prev_clock,
                        clock_after: fs.clock_ms,
                    });
                }
            }
            prev_start = Some(frame_began);
            prev_clock = fs.clock_ms;
        }
        // DC-DEFAULT: CLI-230 (proposed) the server's world models; CLI-232 the effect player's call site
        if let Some(o) = &fs.world_objects {
            let fresh = world.apply_objects(o);
            // CLI-1369: a body standing on a model that left the collision set falls
            player.detach_from_models(world.world_models.left());
            report.world_effects +=
                crate::objects::play_world_effects(&fresh, &mut fs.fx_events) as u64;
        }
        // client-hudgaps: the server's sky table (view box, sky objects)
        if let Some(sky) = &fs.sky {
            world.apply_sky(sky);
        }
        let cw = &world.collision;
        for (kind, a) in &fs.view_sets {
            apply_view_set(&mut view, *kind, *a);
            twist.reset(view.yaw);
        }
        player.follow(&fs);
        if let Some(ls) = fs.life_state {
            report.life_state = Some(ls);
        }
        input.server(&ServerHold {
            next_tick: player.ticks(),
            life_state: fs.life_state,
            corrections: report.corrections,
            on_ground: player.state.on_ground,
        });
        if fs.session_over && report.session_over_frame.is_none() {
            report.session_over_frame = Some(f);
        }
        if !fs.remotes.is_empty() {
            report.remote_frames += 1;
        }
        let prof_remotes = fs.remotes.len();
        if prof_on {
            use crate::prof::{note, Note};
            note(Note::HudEvents, fs.hud_events.len());
            note(Note::FxEvents, fs.fx_events.len());
            note(Note::SoundCues, fs.sound_cues.len());
        }
        let ticks_before = player.ticks();
        laps.lap(PS::Net);
        if let Some(h) = hud.as_mut() {
            let local_ms =
                (opts.frame_us.saturating_mul(f as u64) / 1000).min(u32::MAX as u64) as u32;
            h.update(
                &fs,
                local_ms,
                view.yaw,
                player.state.crouch_held,
                opts.show_board,
            );
            let px = crate::hud::letterbox::bar_px(
                opts.height,
                h.state.letterbox.progress(h.state.now_ms),
                h.hud.letterbox_percent,
            );
            if report.letterbox_marks.last().is_none_or(|m| m.2 != px) {
                report.letterbox_marks.push((f, h.state.now_ms, px));
            }
            // the stamina bar follows the sprint gauge (CLI-589)
            h.set_stamina(player.stamina_level());
            // the kill banners' voice lines play with this frame's sounds
            fs.sound_cues.extend(h.take_sound_cues());
        }
        laps.lap(PS::HudUpdate);
        let (_, bad): (Vec<BoxInstance>, u32) = remote_boxes(&fs.remotes);
        report.nan_remotes += bad;
        // a scripted shooter turns toward a remote player before the frame's ticks
        let eye_now = player.eye_target(&view);
        input.aim(eye_now.to_array(), &fs.remotes, fs.own_team, &mut view);
        let mut moves: Vec<dc_sim::StepEvents> = Vec::new();
        let trace = &mut report.trace;
        let sprint_allowed = player.sprint_allowed;
        player.advance(cw, opts.frame_us, input, &mut view, &mut |r| {
            if opts.trace {
                trace.push(TracePoint {
                    tick: r.tick,
                    pos: r.after,
                    events: r.events,
                    sprint_allowed,
                });
            }
            if r.events.notice_codes().next().is_some() {
                moves.push(r.events);
            }
            if !(r.after.is_finite() && r.before.is_finite()) {
                nan += 1;
                return;
            }
            if cw.ray(r.before, r.after).is_some() {
                tunnel += 1;
            }
            if r.after.y < floor_limit - 100.0 {
                fell = true;
            }
            min_y = min_y.min(r.after.y);
            max_y = max_y.max(r.after.y);
            path += (r.after - r.before).len_xz();
        });
        if !finite_state(&player.state) {
            nan += 1;
        }
        if let Some(clock_ms) = fs.clock_ms {
            report.clock_marks.push(ClockMark {
                frame: f,
                ticks: player.ticks(),
                clock_ms,
            });
        }
        crate::prof::note(
            crate::prof::Note::Ticks,
            player.ticks().saturating_sub(ticks_before) as usize,
        );
        let held_view = fs.view_weapon.as_ref().map(|v| v.weapon);
        if report.view_weapon_marks.last().map(|m| m.2) != Some(held_view) {
            report.view_weapon_marks.push((f, fs.clock_ms, held_view));
        }
        laps.lap(PS::Sim);
        for m in &moves {
            source.moved(m);
        }
        laps.lap(PS::Net);
        let weapon_cmd = input.weapon();
        let kick = source.arms(&ArmsFrame {
            commands: weapon_cmd,
            aim: shot_aim(&player.state, &view),
            body: player.state.pos.to_array(),
            remotes: &fs.remotes,
            world: cw,
            motion: crate::spread::Motion::of(&player.state, &player.params, view.yaw, view.pitch),
            loaded: Some(&world),
            anims: Some(&fs.anims),
            // the zoom changes run against the frame's normal horizontal field (cshell+0x1f9100)
            normal_fov_x: dc_render::fov_pair_2009(opts.view.fov_y_deg, opts.width, opts.height).0,
        });
        // the scope: the mouse turns 1 / (3 z + 1) as fast, a zoomed body walks instead of sprinting
        // SPEC: MOVEMENT_2009.md §13.2 step 2, §7 step 2
        input.set_zoom_level(kick.zoom.level);
        player.sprint_blocked = kick.zoom.active;
        if report.zoom_marks.last().map(|m| m.2) != Some(kick.zoom) {
            report.zoom_marks.push((f, fs.clock_ms, kick.zoom));
        }
        apply_recoil(&mut view, kick.view_pitch);
        apply_recoil_yaw(&mut view, kick.view_yaw);
        if weapon_cmd.fire {
            report.fire_frames += 1;
        }
        laps.lap(PS::Arms);
        // the own twist: the body stays put while the standing view turns (CLI-636)
        twist.step(view.yaw, player.state.controls);
        let mut update = LocalUpdate::from_state(&player.state, &view, player.state.clock as f32)
            .with_twist(view.yaw, &twist);
        if let Some(t) = world.surface_under(&player.state) {
            surface_hold = t;
        }
        update.surface = surface_hold;
        source.report(&update);
        laps.lap(PS::Net);
        let dt = opts.frame_us as f32 / 1e6;
        // the step-up smoothing runs per tick inside the player (CLI-650)
        let eye = player.eye(&view);
        // dead: the death camera replaces the own view (CLI-85)
        let (eye, cam_view) = match death_cam.frame(
            fs.death.as_ref(),
            &fs.remotes,
            eye,
            player.render_pos(),
            &view,
            Some(cw),
            dt,
        ) {
            Some((e, v)) => {
                report.death_cam_frames += 1;
                (e, v)
            }
            None => (eye, view),
        };
        // the death panel shows the followed player (CLI-128)
        if let Some(h) = hud.as_mut() {
            h.set_observed(crate::deathcam::observed(death_cam.mode(), &fs.remotes));
        }
        // the own camera draws the hit jar's offsets and the recoil's kick-back (crate::camfx)
        let own_view = fs.death.is_none();
        let (eye, cam_view) = if own_view {
            crate::camfx::own_camera(eye, &cam_view, kick.camera, kick.eye_back)
        } else {
            (eye, cam_view)
        };
        // a live world camera replaces the own and the death camera (CLI-240)
        let (eye, cam_view, shot_fov) = match cam_shot.frame(fs.cam_shot.as_ref()) {
            Some((e, v, fov)) => {
                report.cam_shot_frames += 1;
                if report.cam_shot_marks.len() < CAM_SHOT_MARKS {
                    report
                        .cam_shot_marks
                        .push((f, e.to_array(), v.yaw, v.pitch, player.cam_lock));
                }
                (e, v, Some(fov))
            }
            None => (eye, cam_view, None),
        };
        let (eye, cam_view) = match opts.camera {
            Some((at, yaw, pitch)) => (V3::from_array(at), ViewAngles { yaw, pitch }),
            None => (eye, cam_view),
        };
        // the rest of the frame (sound listener, camera, panel) draws from the camera's view; the loop's own view
        // angles (outside this block) stay the player's
        let view = cam_view;
        if let Some(h) = hud.as_ref() {
            let m = HudMark::of(f, &h.state, death_cam.mode());
            if report.hud_marks.last().is_none_or(|l| !l.same_state(&m)) {
                report.hud_marks.push(m);
            }
        }
        // the match-loop pictures, each once
        let mut shot: Option<&'static str> = None;
        if opts.state_shots {
            let hs = hud.as_ref().map(|h| &h.state);
            let dead = fs.death.is_some();
            let want = if hs.is_some_and(|s| {
                s.result_screen
                    .is_some_and(|r| s.now_ms.saturating_sub(r.at) >= RESULT_SHOT_MS)
            }) {
                Some("result")
            } else if hs.is_some_and(|s| s.result_board_shown() && s.result_screen.is_none()) {
                Some("result_board")
            } else if fs.death.is_some_and(|d| d.since_ms >= DEATH_SHOT_MS) {
                Some("death")
            } else if !dead && was_dead && fs.life_state == Some(1) {
                Some("respawn")
            } else if !dead && hs.is_some_and(|s| s.damage.iter().any(|v| *v > 0.0)) {
                Some("damage")
            } else {
                None
            };
            was_dead |= dead;
            // the death panel's picture once it is drawn (after the killer camera)
            let want = want.filter(|w| !shots_done.contains(w)).or_else(|| {
                hs.filter(|s| crate::hud::death::panel_shown(s))
                    .map(|_| "death_panel")
            });
            if let Some(w) = want.filter(|w| !shots_done.contains(w)) {
                shots_done.push(w);
                shot = Some(w);
            }
        }
        let fx_ms = fs
            .clock_ms
            .unwrap_or((opts.frame_us.saturating_mul(f as u64) / 1000).min(u32::MAX as u64) as u32);
        if !fs.thrown.is_empty() {
            report.thrown_frames += 1;
            report.thrown_max = report.thrown_max.max(fs.thrown.len());
        }
        laps.lap(PS::Sim);
        let fx_cues = fx
            .as_mut()
            .map(|x| x.frame(fx_ms, &fs, &world))
            .unwrap_or_default();
        // w2-fx: the effects' camera shake and model lights on the frame's final eye (crate::fx)
        let eye = fx
            .as_ref()
            .map_or(eye, |x| x.camera_frame(fx_ms, eye, &world));
        let cues: Vec<crate::sound::SoundCue> =
            fs.sound_cues.iter().cloned().chain(fx_cues).collect();
        laps.lap(PS::Fx);
        if let Some(d) = sound.as_mut() {
            let local_ms =
                (opts.frame_us.saturating_mul(f as u64) / 1000).min(u32::MAX as u64) as u32;
            let p = player.state.pos;
            let own = crate::sound::OwnAudio {
                look: fs.own_look.as_ref(),
                obj: fs.own_object,
                feet: [p.x, p.y - player.state.dims.y, p.z],
                surface: update.surface,
                control: player.state.controls,
                on_ground: player.state.on_ground,
                airborne: player.state.jumping,
                sprinting: player.state.stamina.mode == dc_sim::GaugeMode::Sprint,
                alive: fs.life_state.is_none_or(|s| s == 1),
                jumped: moves.iter().any(|m| m.notice_codes().any(|c| c == 1)),
                speed: player.state.vel.len(),
                life_state: fs.life_state,
            };
            d.frame(&crate::sound::AudioFrame {
                now_ms: fs.clock_ms.unwrap_or(local_ms),
                eye: eye.to_array(),
                yaw: view.yaw,
                pitch: view.pitch,
                cues: &cues,
                remotes: &fs.remotes,
                own: Some(own),
                world: Some(&world),
            });
            if crate::events::on() {
                crate::events::sound_poll(&d.mixer());
            }
        }
        watch.frame(&fs);
        if !(eye.is_finite() && view.yaw.is_finite() && view.pitch.is_finite()) {
            report.nan_frames += 1;
        }
        laps.lap(PS::Sound);
        let stop_now = match (opts.stop_after_over, report.session_over_frame) {
            (Some(n), Some(o)) => f >= o.saturating_add(n),
            _ => false,
        } || crate::steps::end_game();
        let step_shot = crate::steps::take_shot();
        let last = f + 1 == opts.frames || stop_now;
        let mut lines = overlay_lines(
            &world.name,
            &player,
            &view,
            1e6 / opts.frame_us.max(1) as f64,
            input.label(),
        );
        lines.extend(fs.hud.iter().cloned());
        if let (Some(g), Some(scene)) = (gpu.as_mut(), world.scene.as_ref()) {
            if f % save_every == 0 || last || shot.is_some() || step_shot.is_some() || prof_on {
                if let (Some(font), true) = (&font, opts.debug_panel) {
                    g.overlay.update_panel(&g.ctx, &font.atlas, &lines);
                }
                laps.lap(PS::Panel);
                let mut camera =
                    crate::camshot::view_settings(opts.view.with_world_far(world.far_z), shot_fov)
                        .camera(eye, &view, opts.width, opts.height);
                if own_view && shot_fov.is_none() {
                    crate::camfx::roll_camera(&mut camera, kick.camera.roll);
                    // the scope's field of view (cshell+0x1f9100)
                    kick.zoom.apply(&mut camera);
                }
                let t = Instant::now();
                let mut drawn: Vec<u16> = Vec::new();
                let mut fx_socket = None;
                if let Some(m) = g.models.as_mut() {
                    let lights = world.model_lights.as_ref();
                    let bodies = crate::fx::with_thrown(&fs.remotes, &fs.thrown);
                    // client-hudgaps: the team-mate mark
                    m.own_team = fs.own_team;
                    m.update_remotes_animated(&g.ctx, &g.renderer, &bodies, &fs.anims, lights);
                    m.place_remote_attachments(&g.ctx, &g.renderer, &fs.remote_attach);
                    m.place_carried(&g.ctx, &g.renderer, &fs.remote_carried, &fs.remote_gear);
                    laps.lap(PS::Models);
                    let drop_ms = fs.clock_ms.unwrap_or(
                        (opts.frame_us.saturating_mul(f as u64) / 1000).min(u32::MAX as u64) as u32,
                    );
                    let cw: &dyn dc_sim::Collider = &world.collision;
                    m.update_drops(&g.ctx, &g.renderer, &fs.drops, drop_ms, Some(cw), lights);
                    report.drops_drawn += u64::from(m.drops_drawn());
                    // client-destructfx: the effect models (a breaking crate's pieces, a launched drum)
                    if let Some(x) = fx.as_mut() {
                        let list = x.runner.models_at(fx_ms);
                        m.update_fx_models(&g.ctx, &g.renderer, &list, lights);
                        report.fx_models_drawn += u64::from(m.fx_models.drawn);
                    }
                    laps.lap(PS::Drops);
                    m.set_own_look(fs.own_look.as_ref());
                    m.place_view_weapon(
                        &g.ctx,
                        &g.renderer,
                        fs.view_weapon.as_ref(),
                        glam::Vec3::new(eye.x, eye.y, eye.z),
                        lights,
                    );
                    m.place_view_attachments(&g.ctx, &g.renderer, &fs.view_attach);
                    m.set_view_hidden(fs.view_hidden);
                    laps.lap(PS::ViewWeapon);
                    drawn = m.drawn();
                    watch.corpses(&fs, &drawn);
                    crate::heartbeat::corpses(&fs, &drawn);
                    report.models_drawn += m.count() as u64;
                    report.characters_drawn += m.stats.characters as u64;
                    report.view_weapon_frames += m.stats.view_weapon as u32;
                    let at_eye = m
                        .view_model_path()
                        .filter(|_| m.stats.view_weapon)
                        .map(str::to_string);
                    if report.view_model_marks.last().map(|v| &v.1) != Some(&at_eye) {
                        report.view_model_marks.push((f, at_eye));
                    }
                    fx_socket = fx.as_mut().and_then(|x| x.view_socket(m));
                }
                let mut shown: Vec<BoxInstance> = box_list(&fs.remotes, &drawn, opts.debug_boxes);
                // MAP-34: the capture flags without a model, as bars in the team colour
                shown.extend(crate::world::flag_marker_boxes(&fs.flag_markers));
                laps.lap(PS::Models);
                // client-tags2: the HUD after the characters are posed, so the names stand on this frame's name
                // sockets (its time stays out of the render time, as before the move)
                // DC-DEFAULT: CLI-371 (proposed)
                let hud_t = Instant::now();
                let name_points = g
                    .models
                    .as_ref()
                    .map(|m| m.name_points())
                    .unwrap_or_default();
                if let Some(h) = hud.as_mut() {
                    h.set_cases(&fs.hud_cases, &camera);
                    h.set_flags(&fs.hud_flags, &camera);
                    h.set_marks_at(&fs.remotes, &camera, Some(&world.collision), &name_points);
                    h.set_radar(&fs.remotes, &camera);
                    // CLI-1620: the render hook's answer and this frame's x-ray players (as the window)
                    h.set_entity_visibility(
                        &fs.remotes,
                        &camera,
                        Some(&world.collision),
                        fs.own_object.map(|o| (o, fs.own_team.unwrap_or(0))),
                    );
                    // the display list as batched quads, as the window draws it; profiling builds it only when it
                    // is due, as the window does
                    h.draw_into(&g.ctx, &mut g.hud_layer, hud_every_frame());
                    // the report's HUD image (tests read the pixels): the CPU compositor, on the last frame only.
                    // In profile mode it runs after the profiler has finished (the profiled frames stay the window's
                    // frames, perf-g1), so a profiled run reports the same images as a plain one (contract 2.1)
                    if last && !prof_on {
                        report.last_hud = Some(h.hud.render(&h.state));
                        laps.reset();
                    } else if last {
                        cpu_hud_after_prof = true;
                    }
                }
                let hud_spent = hud_t.elapsed();
                laps.lap(PS::HudCompose);
                let models = g.models.as_ref().map(|m| m.instances()).unwrap_or_default();
                let view_models = g
                    .models
                    .as_ref()
                    .map(|m| m.view_instances())
                    .unwrap_or_default();
                // CLI-1624: the chams / skeleton players, each with its posed instances (as the window)
                let xray: Vec<dc_render::XrayModel<'_>> = match (&hud, &g.models) {
                    (Some(h), Some(m)) => h
                        .xray_draws
                        .iter()
                        .filter_map(|d| {
                            let instances = m.character_instances(d.entity);
                            (!instances.is_empty()).then_some(dc_render::XrayModel {
                                instances,
                                mode: d.mode,
                                primary: d.primary,
                                secondary: d.secondary,
                            })
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let img = g.target.render_with_xray(
                    &g.ctx,
                    &g.renderer,
                    scene,
                    &camera,
                    Some((&mut g.boxes, &shown)),
                    &models,
                    &view_models,
                    &xray,
                    fx.as_mut().map(|x| (x, fx_ms, fx_socket)),
                    hud.as_ref().map(|_| &g.hud_layer),
                    font.as_ref()
                        .filter(|_| opts.debug_panel)
                        .map(|_| &g.overlay),
                )?;
                if opts.hash_frames && !img.rgba.is_empty() {
                    report.frame_hashes.push(FrameHash {
                        frame: f,
                        width: img.width,
                        height: img.height,
                        sha256: img.texel_hash(),
                    });
                }
                report.boxes_drawn += g.boxes.last_drawn as u64;
                report.fx_quads += fx.as_ref().map_or(0, |x| x.drawn() as u64);
                render_ms += t.elapsed().saturating_sub(hud_spent).as_secs_f64() * 1000.0;
                rendered += 1;
                laps.reset();
                let save = !prof_on || crate::prof::config().png;
                if let (Some(dir), true) = (&opts.out, save) {
                    let path = match shot {
                        Some(w) => dir.join(format!("{}{w}.png", opts.prefix)),
                        None => dir.join(format!("{}{f:04}.png", opts.prefix)),
                    };
                    match png.as_mut() {
                        Some(w) => w.save(&img, &path)?,
                        None => img
                            .save_png(&path)
                            .map_err(|e| ClientError::Io(e.to_string()))?,
                    }
                    match shot {
                        Some(w) => report.state_images.push((w.to_string(), path)),
                        None => report.images.push(path),
                    }
                }
                if let Some(n) = &step_shot {
                    crate::steps::save_shot(n, &img);
                }
                report.last_frame = Some(img);
                laps.lap(PS::Png);
            }
        }
        if prof_on {
            prof_rec.end_with(prof_remotes, gpu.as_ref().map(|g| &g.ctx.device));
        }
        if last {
            report.last_lines = lines;
        }
        report.frames = f + 1;
        if stop_now {
            break;
        }
        if opts.realtime {
            let due = std::time::Duration::from_micros(opts.frame_us.saturating_mul(f as u64 + 1));
            if let Some(wait) = due.checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
        }
    }
    crate::events::screen("match_end");
    if prof_on {
        prof_rec.finish_with(gpu.as_ref().map(|g| &g.ctx.device));
    }
    if cpu_hud_after_prof {
        if let Some(h) = hud.as_mut() {
            report.last_hud = Some(h.hud.render(&h.state));
        }
    }
    if let Some(w) = png {
        w.finish()?;
    }
    if let Some(h) = &hud {
        report.hud_events = h.events_applied;
        report.hud_composed = h.composed;
    }
    if let Some(x) = &fx {
        report.fx = x.runner.stats;
    }
    crate::watchdog::phase(crate::watchdog::Phase::Idle);
    report.ticks = player.ticks();
    report.step_height = player.params.step_height;
    report.world_models_drawn = world
        .world_models
        .drawn()
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect();
    report.end = player.state.pos;
    report.final_digest = player.state.digest();
    report.nan_ticks = nan;
    report.tunnel_ticks = tunnel;
    report.fell_out = fell;
    report.min_y = min_y;
    report.max_y = max_y;
    report.path_xz = path;
    report.jumps = player.jumps;
    report.landings = player.landings;
    report.ended_supported = player.state.on_ground || player.state.contact.is_some();
    report.render_ms_mean = if rendered > 0 {
        render_ms / rendered as f64
    } else {
        0.0
    };
    if let Some(d) = sound {
        report.sound_note = Some(d.summary());
        report.sound_bounds = Some(d.bounds_line());
        report.sound_stats = d.stats;
        report.sound_spawn_log = d.spawn_log;
        report.sound_log = d.log;
    }
    // the match's mods live until here (their drop installs nothing again)
    drop(mods);
    Ok(report)
}

/// Default folder for client images when the caller names none (never inside the source tree).
pub fn default_out_dir() -> PathBuf {
    dc_render::default_out_dir().join("dc-client")
}

/// Makes `dir` (and parents).
pub fn ensure_dir(dir: &Path) -> Result<(), ClientError> {
    std::fs::create_dir_all(dir).map_err(|e| ClientError::Io(format!("{}: {e}", dir.display())))
}

/// The headless loop composes the HUD on every frame (`true`: profiling off, or `DC_PROF_HUDFORCE=1`), or, in profile
/// mode without `DC_PROF_HUDFORCE`, only when its items are due on the session clock, as the window does. The two are
/// different render policies: the exact lane records the second as an input (`render_settings.hud_cadence`,
/// docs/RENDER_DETERMINISM_CONTRACT.md section 2.1), since its HUD frames differ.
pub fn hud_every_frame() -> bool {
    !crate::prof::on() || crate::prof::config().hud_force
}
