//! Stress scenarios of the render performance baseline (docs/RENDER_PERF_GATES.md G1; perf-g1, our own tooling).
//!
//! A [`StressSource`] wraps a ROM map ([`RomWorld`]) and hands the frame loop a made-up, frame-indexed
//! [`FrameState`]: remote characters, weapon-effect events and HUD inputs as pure functions of the frame number, so a
//! headless run with a fixed `--frame-ms` and `--headless-frames N` is reproducible (inline model loads). The same
//! source runs in the window (`dc-client --window ... --stress <name>`), where the clock is the wall clock.
//!
//! | scenario | what is stressed |
//! |---|---|
//! | `bsp` | world visibility: the view with the most world triangles drawn ([`search_views`] over every spawn, 16 yaws, 3 pitches), pinned, nothing else |
//! | `characters` | [`StressConfig::remotes`] (16) skinned characters, all in view, walking in place, each with another weapon |
//! | `fx` | the characters plus every character firing (tracers, muzzle flashes, impacts) every [`FX_PERIOD`] frames, a blast every [`BLAST_PERIOD`] frames, a placed effect group every [`WORLD_FX_PERIOD`] frames |
//! | `hud` | the characters plus every HUD element: roster, scoreboard (the run sets it), kill feed refilled every [`KILL_PERIOD`] frames, kill-run banners, damage arcs, hit flash, low health, ammunition, aim mark, name tags and the moving radar |
//! | `cold` | everything at once from frame 0 (first-use model, texture, sound and pipeline loads; the run keeps warm-up 0) |
//! | `match` | moderate rates of all of it while the player walks ([`crate::WalkTest`]) |
//!
//! Nothing of the ROM is named here: models, weapons, ammunition and effect groups are picked by index from the
//! user's ROM tables at run time.
// ORIGINAL: perf-g1 measurement scenarios (no 2009 behaviour; the made-up events only drive the existing paths)

use crate::error::ClientError;
use crate::fx::{FxEvent, ShotRecord, WorldFx, NO_OBJECT};
use crate::hud::state::{AmmoView, HudEvent};
use crate::input::{InputSource, WalkTest};
use crate::look::ViewAngles;
use crate::world::{
    FrameState, GpuTarget, LoadedWorld, ModelRef, RemoteBody, RemoteLook, RemoteTracker, RomWorld,
    SpawnPoint, WorldSource,
};
use dc_botclient::shell::{KillRecord, RosterRecord};
use dc_render::{name_key, ModelFiles, CLIP_LOWER_WALK, CLIP_UPPER_WALK};
use dc_sim::{drop_to_floor, Commands, Cylinder, MovementParams, V3};
use std::path::{Path, PathBuf};

/// Frames between two shots of one character (`fx`; `match` uses four times this).
pub const FX_PERIOD: u64 = 3;
/// Frames between two blasts (`fx`).
pub const BLAST_PERIOD: u64 = 20;
/// Frames between two placed effect groups (`fx`).
pub const WORLD_FX_PERIOD: u64 = 10;
/// Frames between two kills in the feed (`hud`).
pub const KILL_PERIOD: u64 = 8;
/// Remote characters of the scenarios with characters.
pub const DEFAULT_REMOTES: usize = 16;

/// The stress scenarios.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    Bsp,
    Characters,
    Fx,
    Hud,
    Cold,
    Match,
}

impl Scenario {
    pub const ALL: [Scenario; 6] = [
        Scenario::Bsp,
        Scenario::Characters,
        Scenario::Fx,
        Scenario::Hud,
        Scenario::Cold,
        Scenario::Match,
    ];

    pub fn parse(s: &str) -> Option<Scenario> {
        Scenario::ALL
            .into_iter()
            .find(|k| k.name() == s.to_ascii_lowercase())
    }

    pub fn name(self) -> &'static str {
        match self {
            Scenario::Bsp => "bsp",
            Scenario::Characters => "characters",
            Scenario::Fx => "fx",
            Scenario::Hud => "hud",
            Scenario::Cold => "cold",
            Scenario::Match => "match",
        }
    }

    fn characters(self) -> bool {
        self != Scenario::Bsp
    }

    fn effects(self) -> Option<u64> {
        match self {
            Scenario::Fx | Scenario::Cold => Some(FX_PERIOD),
            Scenario::Match => Some(FX_PERIOD * 4),
            _ => None,
        }
    }

    fn hud(self) -> bool {
        matches!(self, Scenario::Hud | Scenario::Cold | Scenario::Match)
    }

    /// The scoreboard is held open.
    pub fn board(self) -> bool {
        matches!(self, Scenario::Hud | Scenario::Cold)
    }
}

/// What a stress run is made of.
#[derive(Debug, Clone, PartialEq)]
pub struct StressConfig {
    pub scenario: Scenario,
    pub remotes: usize,
    /// Spawn index and view angles (radians) of the camera; `None`: spawn 0, its facing, level.
    pub view: Option<(usize, f32, f32)>,
    /// Frame interval for the scenario's own clock (ms).
    pub frame_ms: f64,
}

impl StressConfig {
    pub fn new(scenario: Scenario) -> StressConfig {
        StressConfig {
            scenario,
            remotes: DEFAULT_REMOTES,
            view: None,
            frame_ms: 1000.0 / 60.0,
        }
    }

    /// The spawn the run starts from.
    pub fn spawn(&self) -> usize {
        self.view.map_or(0, |v| v.0)
    }
}

/// A weapon picked from the ROM table: id, third-person model, ammunition.
#[derive(Debug, Clone)]
struct Gun {
    id: u16,
    model: ModelRef,
    ammo: u16,
}

#[derive(Debug, Clone)]
struct Prepared {
    view: ViewAngles,
    /// The camera's feet.
    feet: V3,
    bodies: Vec<V3>,
    half: V3,
    guns: Vec<Gun>,
    ammos: Vec<u16>,
    fx_names: Vec<String>,
    clips: Option<(usize, usize)>,
}

/// A ROM map with the frame-indexed made-up state of a [`Scenario`].
pub struct StressSource {
    inner: RomWorld,
    rom: PathBuf,
    cfg: StressConfig,
    frame: u64,
    prepared: Option<Prepared>,
    /// What the preparation found (models, weapons, effect groups), for the run's note.
    pub note: String,
}

impl StressSource {
    pub fn new(rom: &Path, map: &str, cfg: StressConfig) -> StressSource {
        StressSource {
            inner: RomWorld::new(rom, map),
            rom: rom.to_path_buf(),
            cfg,
            frame: 0,
            prepared: None,
            note: String::new(),
        }
    }

    fn prepare(&mut self, w: &LoadedWorld) -> Result<Prepared, ClientError> {
        let spawn = w.spawn(self.cfg.spawn())?;
        let params = MovementParams::default();
        let me = crate::LocalPlayer::spawn(params, &w.collision, &spawn, w.stand_dims, 60);
        let view = match self.cfg.view {
            Some((_, yaw, pitch)) => ViewAngles { yaw, pitch },
            None => ViewAngles {
                yaw: spawn.yaw,
                pitch: 0.0,
            },
        };
        let feet = me.state.pos;
        let shape = Cylinder::from_dims(w.stand_dims);
        // four rows of four (more: further rows), 180..600 units ahead, spread over the view's width
        let fwd = V3::new(view.yaw.sin(), 0.0, view.yaw.cos());
        let right = V3::new(view.yaw.cos(), 0.0, -view.yaw.sin());
        let n = if self.cfg.scenario.characters() {
            self.cfg.remotes
        } else {
            0
        };
        let bodies: Vec<V3> = (0..n)
            .map(|i| {
                let row = (i / 4) as f32;
                let col = (i % 4) as f32 - 1.5;
                let dist = 180.0 + row * 110.0;
                let at = feet + fwd * dist + right * (col * dist * 0.28);
                drop_to_floor(&w.collision, &shape, at, params.step(), 1000.0)
            })
            .collect();
        let mut guns = Vec::new();
        let mut ammos = Vec::new();
        if let Ok(table) = crate::WeaponTable::open(&self.rom) {
            for (id, r) in &table.rows {
                if let Some(a) = r.ammo {
                    if !ammos.contains(&a) {
                        ammos.push(a);
                    }
                }
                if let (Some(m), Some(a)) = (&r.hh_model, r.ammo) {
                    guns.push(Gun {
                        id: *id,
                        model: ModelRef {
                            model: m.clone(),
                            textures: r.hh_skins.clone(),
                        },
                        ammo: a,
                    });
                }
            }
        }
        // spread the picks over the whole table (by index, nothing named)
        let step = (guns.len() / n.max(1)).max(1);
        let guns: Vec<Gun> = guns.into_iter().step_by(step).take(n.max(1)).collect();
        let fx_names = if self.cfg.scenario.effects().is_some() {
            crate::fx::EffectLibrary::open(&self.rom).names()
        } else {
            Vec::new()
        };
        let clips = crate::characters::ModelScene::open(&self.rom)
            .ok()
            .and_then(|m| m.default_look().into_iter().next())
            .and_then(|body| {
                let vfs = dc_rez::RezVfs::new(&self.rom);
                let anim = ModelFiles::load(&vfs, &body.model).ok()?.anim_set();
                let clip = |k| anim.find_clip(|c| name_key(c) == k);
                Some((clip(CLIP_UPPER_WALK)?, clip(CLIP_LOWER_WALK)?))
            });
        self.note = format!(
            "stress {}: {} characters, {} weapons, {} ammunition kinds, {} effect groups, walk clips {}",
            self.cfg.scenario.name(),
            bodies.len(),
            guns.len(),
            ammos.len(),
            fx_names.len(),
            if clips.is_some() { "found" } else { "missing" }
        );
        Ok(Prepared {
            view,
            feet,
            bodies,
            half: w.stand_dims,
            guns,
            ammos,
            fx_names,
            clips,
        })
    }

    /// The made-up state of frame `f` (a pure function of `f` and the prepared scene).
    fn state_at(&self, p: &Prepared, f: u64) -> FrameState {
        let mut fs = FrameState::default();
        let s = self.cfg.scenario;
        let now = (f as f64 * self.cfg.frame_ms) as u32;
        let fwd = V3::new(p.view.yaw.sin(), 0.0, p.view.yaw.cos());
        let right = V3::new(p.view.yaw.cos(), 0.0, -p.view.yaw.sin());
        for (i, base) in p.bodies.iter().enumerate() {
            // a slow sway across the view: the radar and the name tags move, the walk clips play
            let sway = ((now as f32 / 1000.0) + i as f32 * 0.7).sin() * 30.0;
            let pos = *base + right * sway;
            let gun = (!p.guns.is_empty()).then(|| p.guns[i % p.guns.len()].clone());
            let trackers = match p.clips {
                Some((upper, lower)) => vec![
                    RemoteTracker {
                        clip: 0,
                        time_ms: 0.0,
                        blend_set: None,
                    },
                    RemoteTracker {
                        clip: upper,
                        time_ms: (now + i as u32 * 97) as f32,
                        blend_set: Some(0),
                    },
                    RemoteTracker {
                        clip: lower,
                        time_ms: (now + i as u32 * 97) as f32,
                        blend_set: Some(1),
                    },
                ],
                None => Vec::new(),
            };
            fs.remotes.push(RemoteBody {
                obj: 100 + i as u16,
                pid: (i + 1).min(255) as u8,
                name: format!("Stress{:02}", i + 1),
                team: 1 + (i % 2) as u8,
                pos: pos.to_array(),
                half: p.half.to_array(),
                yaw: Some(p.view.yaw + std::f32::consts::PI),
                look: Some(RemoteLook {
                    styles: Vec::new(),
                    models: Vec::new(),
                    weapon: gun.map(|g| (g.model, 2)),
                    trackers,
                    visible: true,
                    aim: None,
                }),
            });
        }
        if let Some(period) = s.effects() {
            for (i, r) in fs.remotes.iter().enumerate() {
                if !(f + i as u64).is_multiple_of(period) || p.guns.is_empty() {
                    continue;
                }
                let g = &p.guns[i % p.guns.len()];
                let from = V3::from_array(r.pos) + V3::new(0.0, 20.0, 0.0);
                // the impact lands on the floor between the shooter and the camera
                let to = p.feet + fwd * 60.0 + right * ((i as f32 - 7.5) * 12.0);
                fs.fx_events.push(FxEvent::Record(ShotRecord {
                    shooter: r.obj,
                    first: NO_OBJECT,
                    weapon: g.id,
                    ammo: g.ammo,
                    from: from.to_array(),
                    to: [to.x, p.feet.y - p.half.y, to.z],
                    normal: [0.0, 1.0, 0.0],
                    surface: ((f / 7 + i as u64) % 8) as u8,
                    clear: 0,
                    own: false,
                    mods: [0xff; 3],
                }));
            }
            if f.is_multiple_of(BLAST_PERIOD) && !p.ammos.is_empty() {
                // every ammunition kind in turn: the thrown kinds blast
                let a = p.ammos[((f / BLAST_PERIOD) as usize) % p.ammos.len()];
                let at = p.feet + fwd * 250.0;
                fs.fx_events.push(FxEvent::Record(ShotRecord {
                    shooter: 100,
                    first: NO_OBJECT,
                    weapon: p.guns.first().map_or(0, |g| g.id),
                    ammo: a,
                    from: at.to_array(),
                    to: [at.x, p.feet.y - p.half.y, at.z],
                    normal: [0.0, 1.0, 0.0],
                    surface: 0,
                    clear: 0,
                    own: false,
                    mods: [0xff; 3],
                }));
            }
            if f.is_multiple_of(WORLD_FX_PERIOD) && !p.fx_names.is_empty() {
                let k = ((f / WORLD_FX_PERIOD) as usize * 7) % p.fx_names.len();
                let at = p.feet
                    + fwd * 200.0
                    + right * (((f / WORLD_FX_PERIOD) % 5) as f32 * 40.0 - 80.0);
                fs.fx_events.push(FxEvent::World(WorldFx {
                    name: p.fx_names[k].clone(),
                    pos: at.to_array(),
                    rot: [0.0, 0.0, 0.0, 1.0],
                    obj: None,
                    value: 0,
                }));
            }
        }
        if s.hud() {
            hud_inputs(&mut fs, p, f, now);
        }
        fs
    }
}

fn hud_inputs(fs: &mut FrameState, p: &Prepared, f: u64, now: u32) {
    let n = p.bodies.len().max(1) as u64;
    if f == 0 {
        let rec = |pid: u8, name: String, team: u8| RosterRecord {
            name,
            pid,
            team,
            complete: true,
            ..RosterRecord::default()
        };
        fs.hud_events.push((
            now,
            HudEvent::Own {
                pid: Some(0),
                mode_class: Some(1),
            },
        ));
        fs.hud_events
            .push((now, HudEvent::Roster(rec(0, "Stress00".into(), 1))));
        for i in 0..p.bodies.len() {
            fs.hud_events.push((
                now,
                HudEvent::Roster(rec(
                    (i + 1).min(255) as u8,
                    format!("Stress{:02}", i + 1),
                    1 + (i % 2) as u8,
                )),
            ));
        }
        fs.hud_events.push((now, HudEvent::Life(1)));
        for (kind, value) in [(0x0f, 100.0), (2, 18.0), (3, 41.0)] {
            fs.hud_events.push((now, HudEvent::Status { kind, value }));
        }
        fs.hud_events.push((now, HudEvent::RoundBegin(600)));
    }
    let weapon = p.guns.first().map_or(0, |g| g.id);
    if f.is_multiple_of(KILL_PERIOD) {
        let k = f / KILL_PERIOD;
        fs.hud_events.push((
            now,
            HudEvent::Kill(KillRecord {
                victim: (1 + k % n) as u8,
                killer: (1 + (k + 3) % n) as u8,
                kill_kind: 1,
                weapon,
                hit_zone: [1u8, 3, 5][(k % 3) as usize],
                feed_value: 2,
                complete: true,
                ..KillRecord::default()
            }),
        ));
    }
    if f % 90 == 45 {
        fs.hud_events.push((
            now,
            HudEvent::KillRun {
                kills: (2 + (f / 90) % 4) as u16,
                head: f % 180 == 45,
                combo: 0,
            },
        ));
    }
    if f.is_multiple_of(15) {
        let a = f as f32 * 0.4;
        fs.hud_events
            .push((now, HudEvent::Damage([a.sin(), 0.0, a.cos()])));
    }
    if f.is_multiple_of(30) {
        fs.hud_events.push((
            now,
            HudEvent::Hit {
                damage: 7,
                zone: (f % 4) as u8,
            },
        ));
    }
    fs.ammo = Some(AmmoView {
        weapon,
        clip: (30 - (f / 6) % 30) as u32,
        reserve: 90,
        clip_size: Some(30),
        single_shot: false,
        order_class: 1,
        infinite: false,
    });
    fs.aim_mark = Some(crate::spread::AimMarkView {
        distance: 6.0 + (f % 20) as f32,
        since_shot_s: None,
        colour: [0, 255, 0, 255],
        rest: 5.0,
    });
}

impl WorldSource for StressSource {
    fn describe(&self) -> String {
        format!(
            "{} with stress scenario {}",
            self.inner.describe(),
            self.cfg.scenario.name()
        )
    }

    fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError> {
        let w = self.inner.load(gpu)?;
        self.prepared = Some(self.prepare(&w)?);
        println!("{}", self.note);
        Ok(w)
    }

    fn reload_scene(&mut self, gpu: GpuTarget<'_>) -> Result<LoadedWorld, ClientError> {
        self.inner.reload_scene(gpu)
    }

    fn frame_state(&mut self) -> FrameState {
        let f = self.frame;
        self.frame += 1;
        match &self.prepared {
            Some(p) => self.state_at(p, f),
            None => FrameState::default(),
        }
    }

    fn rom(&self) -> Option<PathBuf> {
        Some(self.rom.clone())
    }

    fn scripted_input(&mut self, start: ViewAngles) -> Option<Box<dyn InputSource>> {
        let view = self.prepared.as_ref().map_or(start, |p| p.view);
        Some(match self.cfg.scenario {
            Scenario::Match => Box::new(WalkTest::new(view)),
            _ => Box::new(Hold { view }),
        })
    }
}

/// Holds the view still and gives no movement (the pinned cameras of the stress runs).
#[derive(Debug, Clone, Copy)]
pub struct Hold {
    pub view: ViewAngles,
}

impl InputSource for Hold {
    fn frame(&mut self, view: &mut ViewAngles) {
        *view = self.view;
    }

    fn tick(&mut self, _tick: u64, _hz: u32, view: &mut ViewAngles) -> Commands {
        *view = self.view;
        Commands::default()
    }

    fn label(&self) -> &'static str {
        "stress-hold"
    }
}

/// One candidate view of the visibility search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewScore {
    pub spawn: usize,
    pub yaw: f32,
    pub pitch: f32,
    pub world_tris: u64,
    pub draws: u64,
    pub culled: u64,
}

/// The visibility search of a map.
#[derive(Debug, Clone)]
pub struct ViewSearch {
    pub map: String,
    pub candidates: usize,
    /// The five views with the most world triangles, the worst first.
    pub top: Vec<ViewScore>,
    pub ms: f64,
}

/// Yaw steps of the visibility search.
pub const SEARCH_YAWS: usize = 16;
/// Pitches of the visibility search (radians).
pub const SEARCH_PITCHES: [f32; 3] = [-0.35, 0.0, 0.35];

/// Records the world frame of every spawn x [`SEARCH_YAWS`] x [`SEARCH_PITCHES`] view of `map` (frustum culling,
/// sky, translucent objects: the client's frame options) and ranks them by world triangles drawn, then draws.
/// Deterministic: same ROM, same size, same list.
pub fn search_views(
    rom: &Path,
    map: &str,
    size: (u32, u32),
    adapter: dc_render::AdapterChoice,
) -> Result<ViewSearch, ClientError> {
    let t = std::time::Instant::now();
    let ctx =
        dc_render::GpuContext::headless(adapter).map_err(|e| ClientError::Gpu(e.to_string()))?;
    let renderer = dc_render::WorldRenderer::new(
        &ctx,
        dc_render::OFFSCREEN_FORMAT,
        dc_render::RenderSettings::default(),
    );
    let mut src = RomWorld::new(rom, map);
    let w = src.load(Some(GpuTarget {
        ctx: &ctx,
        renderer: &renderer,
    }))?;
    let scene = w
        .scene
        .as_ref()
        .ok_or_else(|| ClientError::Gpu("no scene".into()))?;
    // culling depends on the aspect only: a small target of the same aspect
    let (sw, sh) = (320u32, (320 * size.1 / size.0.max(1)).max(1));
    let target = crate::offscreen::Offscreen::new(&ctx, dc_render::OFFSCREEN_FORMAT, sw, sh);
    let spawns: Vec<SpawnPoint> = w.spawns.clone();
    let mut scores = Vec::new();
    let params = MovementParams::default();
    for (si, sp) in spawns.iter().enumerate() {
        let me = crate::LocalPlayer::spawn(params, &w.collision, sp, w.stand_dims, 60);
        for k in 0..SEARCH_YAWS {
            for &pitch in &SEARCH_PITCHES {
                let yaw = crate::look::wrap_angle(
                    sp.yaw + k as f32 * std::f32::consts::TAU / SEARCH_YAWS as f32,
                );
                let view = ViewAngles { yaw, pitch };
                let eye = me.eye_target(&view);
                let camera = crate::ViewSettings::default().camera(eye, &view, sw, sh);
                let mut enc = ctx
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("view search"),
                    });
                let r = renderer.encode_frame_with_view(
                    &ctx,
                    scene,
                    &camera,
                    target.target(),
                    &dc_render::FrameOptions::default(),
                    &[],
                    &[],
                    &dc_render::PLAYER_VIEW_DEFAULT,
                    &mut enc,
                );
                ctx.queue.submit([enc.finish()]);
                scores.push(ViewScore {
                    spawn: si,
                    yaw,
                    pitch,
                    world_tris: r.world_tris,
                    draws: r.draw_calls,
                    culled: r.world_culled,
                });
            }
        }
        let _ = ctx.device.poll(wgpu::PollType::Wait);
    }
    let candidates = scores.len();
    // stable sort: ties keep the search order (spawn, yaw, pitch)
    scores.sort_by(|a, b| b.world_tris.cmp(&a.world_tris).then(b.draws.cmp(&a.draws)));
    scores.truncate(5);
    Ok(ViewSearch {
        map: map.to_string(),
        candidates,
        top: scores,
        ms: t.elapsed().as_secs_f64() * 1000.0,
    })
}

impl ViewSearch {
    /// One JSON line (the stress runner reads it).
    pub fn json(&self) -> String {
        let top: Vec<serde_json::Value> = self
            .top
            .iter()
            .map(|v| {
                serde_json::json!({"spawn": v.spawn, "yaw": v.yaw, "pitch": v.pitch, "world_tris": v.world_tris,
                                   "draws": v.draws, "culled": v.culled})
            })
            .collect();
        serde_json::json!({"map": self.map, "candidates": self.candidates, "ms": self.ms, "top": top}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenarios_parse_by_name() {
        for s in Scenario::ALL {
            assert_eq!(Scenario::parse(s.name()), Some(s));
        }
        assert_eq!(Scenario::parse("BSP"), Some(Scenario::Bsp));
        assert_eq!(Scenario::parse("nope"), None);
        assert!(Scenario::Hud.board() && !Scenario::Fx.board());
    }

    fn prepared(n: usize) -> Prepared {
        Prepared {
            view: ViewAngles {
                yaw: 0.3,
                pitch: 0.0,
            },
            feet: V3::new(0.0, 0.0, 0.0),
            bodies: (0..n)
                .map(|i| V3::new(i as f32 * 10.0, 0.0, 200.0))
                .collect(),
            half: V3::new(16.0, 50.0, 16.0),
            guns: vec![Gun {
                id: 3,
                model: ModelRef {
                    model: "m".into(),
                    textures: Vec::new(),
                },
                ammo: 4,
            }],
            ammos: vec![4, 5],
            fx_names: vec!["a".into(), "b".into()],
            clips: Some((1, 2)),
        }
    }

    fn source(s: Scenario) -> StressSource {
        StressSource::new(Path::new("."), "none", StressConfig::new(s))
    }

    #[test]
    fn frame_state_is_a_function_of_the_frame() {
        let src = source(Scenario::Cold);
        let p = prepared(16);
        for f in [0u64, 1, 20, 45, 90, 333] {
            assert_eq!(src.state_at(&p, f), src.state_at(&p, f), "frame {f}");
        }
        let a = src.state_at(&p, 0);
        assert_eq!(a.remotes.len(), 16);
        assert!(a.hud_events.len() > 16, "roster and status at frame 0");
        assert!(a.fx_events.iter().any(|e| matches!(e, FxEvent::World(_))));
        let b = src.state_at(&p, 1);
        assert_ne!(a.remotes[0].pos, b.remotes[0].pos, "the characters sway");
    }

    #[test]
    fn scenarios_enable_only_their_load() {
        let p = prepared(4);
        let bsp = source(Scenario::Bsp).state_at(&p, 0);
        assert!(bsp.fx_events.is_empty() && bsp.hud_events.is_empty());
        let ch = source(Scenario::Characters).state_at(&p, 0);
        assert_eq!(ch.remotes.len(), 4);
        assert!(ch.fx_events.is_empty() && ch.hud_events.is_empty());
        let fx = (0..FX_PERIOD)
            .map(|f| source(Scenario::Fx).state_at(&p, f).fx_events.len())
            .sum::<usize>();
        assert!(fx >= 4, "every character fires once per period: {fx}");
        let hud = source(Scenario::Hud).state_at(&p, KILL_PERIOD);
        assert!(hud
            .hud_events
            .iter()
            .any(|e| matches!(e.1, HudEvent::Kill(_))));
        assert!(hud.ammo.is_some() && hud.aim_mark.is_some());
    }

    #[test]
    fn hold_keeps_the_view() {
        let mut h = Hold {
            view: ViewAngles {
                yaw: 1.0,
                pitch: -0.2,
            },
        };
        let mut v = ViewAngles::default();
        h.frame(&mut v);
        assert_eq!(v, h.view);
        let c = h.tick(5, 60, &mut v);
        assert_eq!(c, Commands::default());
    }
}
