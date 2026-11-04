//! The exact lane of the render determinism contract (gate G0 of `docs/RENDER_PERF_GATES.md`; the contract is
//! `docs/RENDER_DETERMINISM_CONTRACT.md`).
//!
//! Scripted scenes driven by explicit ticks (50 Hz, one tick per 20 ms frame, a fixed frame count; scripted input,
//! camera, remote players, drops, HUD events and effect events from a scripted frame source, never the network or a
//! wall clock; models loaded inline), each frame hashed over its intended texels only
//! ([`dc_render::texel_hash`]: RGBA8, padding stripped, alpha 255, gamma ramp applied):
//!
//! - `synth_frame_loop` (no ROM): the synthetic world through the whole headless frame loop (scripted walk, three
//!   remote players as boxes, the fallback HUD with names and radar, the debug panel in the made-up test font);
//! - `synth_order` (no ROM): dc-render alone, coplanar quads of one bucket in two colours (the later draw wins under
//!   LESSEQUAL, so the frame shows the recording order) seen from two cameras;
//! - `hud_fallback` (no ROM): HUD-only compose of a scripted event sequence (roster, health, kill feed, damage, ammo)
//!   with our fallback layout: the CPU compositor's image and the GPU frames;
//! - `rom_world_views` (CA_ROM): every ROM map from its first spawn at two fixed view directions;
//! - `rom_characters` (CA_ROM): posed remote characters (models, weapons, animation trackers at tick times), two
//!   dropped weapons, the own view weapon, the ROM HUD with name tags and the radar;
//! - `rom_fx` (CA_ROM): own shots with the first-person muzzle flash, tracers, impacts and marks, then a fixed world
//!   camera on a breaking crate, a breaking drum and its grenade-path blast;
//! - `rom_hud` (CA_ROM): the HUD-only compose of `hud_fallback` with the 2009 layout and font of the ROM.
//!
//! Tests: three runs in this process hash identically; separate processes (this test binary re-run as a child,
//! `DC_EXACT_CHILD_OUT`), one of them under CPU load (24 busy threads) and one in profile mode with three frames in
//! flight, hash identically and record the same inputs digest; `DC_DETERMINISM_MUTATE=arith` / `order` in a child
//! changes the hashes; the hashes equal the golden file `tests/golden/exact_lane.json` of this machine's lane (only
//! `tools/exact_golden.py --update` writes it, after printing the diff). ROM scenes are skipped without `CA_ROM`;
//! everything is skipped without a GPU adapter. Nothing of the ROM is asserted by value or written to the tree;
//! `DC_EXACT_PNG=<dir>` saves the hashed frames as local PNGs for a human look.
// Provenance: ORIGINAL (our own engineering contract; no 2009 behaviour claims)

use dc_botclient::shell::{KillRecord, RosterRecord};
use dc_client::fx::{FxEvent, ShotRecord, WorldFx, NO_OBJECT};
use dc_client::hud::state::{AmmoView, HudEvent};
use dc_client::hud::HudChoice;
use dc_client::world::{FrameState, GpuTarget, LoadedWorld, SpawnPoint};
use dc_client::{
    ClientError, HeadlessOptions, InputSource, LocalPlayer, ModelRef, RemoteBody, RemoteLook,
    RemoteTracker, RomWorld, ViewAngles, ViewWeapon, WorldSource,
};
use dc_render::{
    AdapterChoice, Batch, Camera, FrameImage, GpuContext, GpuVertex, PassKind, RenderSettings,
    SceneMesh, TextureImage, WorldRenderer, OFFSCREEN_FORMAT,
};
use dc_sim::{drop_to_floor, Commands, Cylinder, MovementParams, V3};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const W: u32 = 320;
const H: u32 = 240;
/// One simulation tick per frame: frame `f` is tick `f`, at `f * FRAME_MS` ms.
const TICK_HZ: u32 = 50;
const FRAME_US: u64 = 20_000;
const FRAME_MS: u32 = 20;
/// The map of the character and effect scenes (skipped when the ROM has no such map).
const MAP: &str = "JUNK_FLEA";
/// A weapon row of the ROM's weapon table (the scenes' remote, dropped and own weapon).
const WEAPON: u16 = 145;

const GOLDEN: &str = "tests/golden/exact_lane.json";

// ------------------------------------------------------------------------------------------------------------------
// scene plumbing
// ------------------------------------------------------------------------------------------------------------------

/// Where a scripted scene places things: computed once from the loaded world (first spawn, its eye, floor points
/// ahead of it, world hit points of rays from the eye).
#[derive(Debug, Clone)]
struct Place {
    spawn: SpawnPoint,
    eye: V3,
    stand: V3,
    /// Body centres on the floor at [`OFFSETS`] (ahead, right) of the spawn.
    ground: Vec<V3>,
    /// World hits of rays from the eye: (point, face normal, surface type).
    walls: Vec<(V3, V3, u16)>,
    /// An eye with a clear line to the source's focus point ([`Scripted::focus`]).
    focus_eye: Option<V3>,
}

const OFFSETS: [(f32, f32); 6] = [
    (140.0, 0.0),
    (165.0, -55.0),
    (175.0, 55.0),
    (320.0, 0.0),
    (85.0, 25.0),
    (95.0, -30.0),
];

fn place_of(world: &LoadedWorld, focus: Option<V3>) -> Place {
    let spawn = world.spawn(0).expect("spawn 0");
    let params = MovementParams::default();
    let me = LocalPlayer::spawn(params, &world.collision, &spawn, world.stand_dims, TICK_HZ);
    let view = ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    };
    let eye = me.eye_target(&view);
    let fwd = V3::new(spawn.yaw.sin(), 0.0, spawn.yaw.cos());
    let side = V3::new(fwd.z, 0.0, -fwd.x);
    let ground = OFFSETS
        .iter()
        .map(|&(a, s)| {
            drop_to_floor(
                &world.collision,
                &Cylinder::from_dims(world.stand_dims),
                me.state.pos + fwd * a + side * s,
                params.step(),
                1000.0,
            )
        })
        .collect();
    let mut walls = Vec::new();
    for k in -2i32..=2 {
        let yaw = spawn.yaw + k as f32 * 0.2;
        let dir = V3::new(yaw.sin() * 0.98, -0.2, yaw.cos() * 0.98);
        if let Some(h) = world.collision.ray(eye, eye + dir * 4000.0) {
            let p = eye + dir * (4000.0 * h.t);
            walls.push((p, h.face_normal, world.surface_type(h.model, h.poly)));
        }
    }
    Place {
        spawn,
        eye,
        stand: world.stand_dims,
        ground,
        walls,
        focus_eye: focus.map(|c| eye_for(&world.collision, c, 60.0)),
    }
}

type Script = Box<dyn FnMut(&Place, u32) -> FrameState>;

/// A world source whose frame states come from a script of the frame index (frame `f` = tick `f`), with the
/// session clock `f * FRAME_MS`: no network, no wall clock.
struct Scripted {
    inner: Box<dyn WorldSource>,
    frame: u32,
    place: Option<Place>,
    /// A world point the script wants a world camera on.
    focus: Option<V3>,
    script: Script,
}

impl WorldSource for Scripted {
    fn describe(&self) -> String {
        format!("scripted {}", self.inner.describe())
    }
    fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError> {
        let w = self.inner.load(gpu)?;
        self.place = Some(place_of(&w, self.focus));
        Ok(w)
    }
    fn frame_state(&mut self) -> FrameState {
        let place = self.place.as_ref().expect("loaded");
        let mut fs = (self.script)(place, self.frame);
        fs.clock_ms = Some(self.frame * FRAME_MS);
        self.frame += 1;
        fs
    }
    fn rom(&self) -> Option<PathBuf> {
        self.inner.rom()
    }
}

/// Scripted look: the view pans by a fixed amount per tick from the spawn's facing (no movement).
struct Pan {
    yaw0: Option<f32>,
    rate: f32,
}

impl InputSource for Pan {
    fn frame(&mut self, _view: &mut ViewAngles) {}
    fn tick(&mut self, tick: u64, _hz: u32, view: &mut ViewAngles) -> Commands {
        let y0 = *self.yaw0.get_or_insert(view.yaw);
        view.yaw = y0 + self.rate * tick as f32;
        view.pitch = 0.05;
        Commands::default()
    }
    fn label(&self) -> &'static str {
        "pan"
    }
}

/// The result of one scene: its frame hashes and its manifest.
#[derive(Debug, Clone)]
struct SceneRun {
    frames: Vec<Value>,
    manifest: Value,
}

fn png_dir() -> Option<PathBuf> {
    std::env::var_os("DC_EXACT_PNG").map(PathBuf::from)
}

fn base_opts(name: &str) -> HeadlessOptions {
    HeadlessOptions {
        frames: 40,
        frame_us: FRAME_US,
        width: W,
        height: H,
        out: png_dir(),
        prefix: format!("exact_{name}_"),
        save_every: 1,
        tick_hz: TICK_HZ,
        adapter: adapter_choice(),
        hash_frames: true,
        sound: dc_client::sound::SoundOptions {
            enabled: false,
            log: None,
        },
        ..HeadlessOptions::default()
    }
}

fn settings_of(opts: &HeadlessOptions, rom: Option<&Path>, extra: Value) -> Value {
    let cam = opts
        .view
        .camera(V3::new(0.0, 0.0, 0.0), &ViewAngles::default(), W, H);
    let mut s = dc_client::exact::render_settings(
        opts.width,
        opts.height,
        &cam,
        dc_client::display::client_gamma(rom),
        &RenderSettings::default(),
        opts.frame_us,
        opts.tick_hz,
    );
    s["frames"] = json!(opts.frames);
    s["hud"] = json!(match &opts.hud {
        HudChoice::Off => "off",
        HudChoice::Fallback => "fallback layout",
        HudChoice::Rom(_) => "ROM layout",
    });
    // profile mode without DC_PROF_HUDFORCE composes the HUD only when it is due: another render policy, so another
    // input (absent: every frame, the exact lane's default)
    if !matches!(opts.hud, HudChoice::Off) && !dc_client::headless::hud_every_frame() {
        s["hud_cadence"] = json!("when due (profile mode without DC_PROF_HUDFORCE)");
    }
    s["scene"] = extra;
    s
}

/// Runs one scripted headless scene and returns its hashes (`None`: no adapter).
fn headless_scene(
    name: &str,
    src: &mut Scripted,
    input: Option<&mut dyn InputSource>,
    opts: &HeadlessOptions,
    extra: Value,
) -> Option<SceneRun> {
    let rom = src.rom();
    let rec = dc_client::exact::record();
    let r = dc_client::run_headless(src, input, opts).expect("headless run");
    let info = adapter_info(&r.adapter)?;
    let mut frames: Vec<Value> = r
        .frame_hashes
        .iter()
        .map(dc_client::exact::frame_json)
        .collect();
    if let Some(h) = &r.last_hud {
        frames.push(json!({
            "frame": "cpu_hud",
            "width": h.width,
            "height": h.height,
            "sha256": dc_render::hex(&dc_render::texel_hash(h.width, h.height, &h.rgba)),
        }));
    }
    assert!(!frames.is_empty(), "{name}: nothing hashed");
    let manifest = rec.manifest(
        Some(&info),
        rom.as_deref(),
        settings_of(opts, rom.as_deref(), extra),
    );
    Some(SceneRun { frames, manifest })
}

/// The adapter of a headless run (`None`: it rendered nothing). The report only names it, so the info comes from a
/// context made the same way ([`adapter_choice`]).
fn adapter_info(described: &Option<String>) -> Option<wgpu::AdapterInfo> {
    let d = described.as_ref()?;
    let ctx = GpuContext::headless(adapter_choice()).ok()?;
    assert_eq!(&ctx.describe(), d, "the same adapter as the run");
    Some(ctx.info.clone())
}

/// The adapter of the exact-lane runs: `DC_EXACT_ADAPTER` = `dx12`, `vulkan`, `fallback` (WARP) or `auto` (the
/// default: dc-render's `AdapterChoice::Auto` order). Another adapter is another lane.
fn adapter_choice() -> AdapterChoice {
    match std::env::var("DC_EXACT_ADAPTER").as_deref() {
        Ok("dx12") => AdapterChoice::Backends(wgpu::Backends::DX12),
        Ok("vulkan") => AdapterChoice::Backends(wgpu::Backends::VULKAN),
        Ok("fallback") => AdapterChoice::Fallback,
        _ => AdapterChoice::Auto,
    }
}

fn context() -> Option<GpuContext> {
    GpuContext::headless(adapter_choice()).ok()
}

fn hash_json(img: &FrameImage, frame: &str) -> Value {
    json!({
        "frame": frame,
        "width": img.width,
        "height": img.height,
        "sha256": dc_render::hex(&img.texel_hash()),
    })
}

fn save_png(img: &FrameImage, name: &str) {
    if let Some(d) = png_dir() {
        let _ = img.save_png(&d.join(format!("exact_{name}.png")));
    }
}

fn rec(pid: u8, name: &str, team: u8) -> RosterRecord {
    RosterRecord {
        name: name.into(),
        pid,
        team,
        complete: true,
        ..RosterRecord::default()
    }
}

/// The HUD inputs of the scripted match: roster and own state on frame 0, then health, ammo, kill feed, damage and a
/// hit at fixed frames.
fn hud_events(f: u32, t: u32, names: [&str; 4]) -> Vec<(u32, HudEvent)> {
    let mut e = Vec::new();
    match f {
        0 => {
            e.push((
                t,
                HudEvent::Own {
                    pid: Some(0),
                    mode_class: Some(1),
                },
            ));
            for (i, n) in names.iter().enumerate() {
                e.push((t, HudEvent::Roster(rec(i as u8, n, 1 + (i as u8 & 1)))));
            }
            e.push((t, HudEvent::Life(1)));
            for kind in [0x0f, 2, 3] {
                e.push((t, HudEvent::Status { kind, value: 100.0 }));
            }
            e.push((t, HudEvent::RoundBegin(180)));
        }
        6 => e.push((
            t,
            HudEvent::Status {
                kind: 2,
                value: 64.0,
            },
        )),
        10 => e.push((
            t,
            HudEvent::Kill(KillRecord {
                victim: 2,
                killer: 1,
                weapon: WEAPON,
                hit_zone: 1,
                complete: true,
                ..KillRecord::default()
            }),
        )),
        14 => {
            e.push((t, HudEvent::Damage([1.0, 0.0, 0.2])));
            e.push((
                t,
                HudEvent::Hit {
                    damage: 18,
                    zone: 2,
                },
            ));
            e.push((
                t,
                HudEvent::Status {
                    kind: 2,
                    value: 46.0,
                },
            ));
        }
        22 => e.push((
            t,
            HudEvent::Kill(KillRecord {
                victim: 3,
                killer: 0,
                weapon: WEAPON,
                hit_zone: 0,
                complete: true,
                ..KillRecord::default()
            }),
        )),
        _ => {}
    }
    e
}

fn ammo() -> AmmoView {
    AmmoView {
        weapon: WEAPON,
        clip: 24,
        reserve: 90,
        clip_size: Some(30),
        single_shot: false,
        order_class: 1,
        infinite: false,
    }
}

// ------------------------------------------------------------------------------------------------------------------
// ROM-free scenes
// ------------------------------------------------------------------------------------------------------------------

/// Names in the two glyphs of the made-up test font (`testkit::synth_font`: 'A' and 'B').
const SYNTH_NAMES: [&str; 4] = ["ABBA", "BABA", "AAB", "BBA"];

fn box_body(p: &Place, k: usize, obj: u16, pid: u8, team: u8, name: &str, f: u32) -> RemoteBody {
    let c = p.ground[k];
    RemoteBody {
        obj,
        pid,
        name: name.into(),
        team,
        pos: [c.x + (f as f32 * 0.7).sin() * 6.0, c.y, c.z],
        half: p.stand.to_array(),
        yaw: Some(p.spawn.yaw + std::f32::consts::PI + f as f32 * 0.02),
        look: None,
    }
}

fn synth_frame_loop() -> Option<SceneRun> {
    let mut src = Scripted {
        inner: Box::new(dc_client::testkit::synth_source()),
        frame: 0,
        place: None,
        focus: None,
        script: Box::new(|p, f| {
            let t = f * FRAME_MS;
            FrameState {
                life_state: Some(1),
                remotes: vec![
                    box_body(p, 0, 10, 1, 1, SYNTH_NAMES[1], f),
                    box_body(p, 1, 11, 2, 2, SYNTH_NAMES[2], f),
                    box_body(p, 2, 12, 3, 1, SYNTH_NAMES[3], f),
                ],
                hud_events: hud_events(f, t, SYNTH_NAMES),
                ammo: Some(ammo()),
                ..FrameState::default()
            }
        }),
    };
    let opts = HeadlessOptions {
        font_bytes: Some(dc_client::testkit::synth_font()),
        hud: HudChoice::Fallback,
        debug_panel: true,
        debug_boxes: true,
        ..base_opts("synth_frame_loop")
    };
    // the default scripted walk (forward, turn, jump) drives the body and the camera
    headless_scene(
        "synth_frame_loop",
        &mut src,
        None,
        &opts,
        json!({ "world": "testkit synth", "input": "ScriptedInput", "remotes": 3 }),
    )
}

/// A quad facing the camera at the origin (clockwise on screen), centred at `c`, half size `s`, in bucket 1.
fn quad(mesh: &mut SceneMesh, c: [f32; 3], s: f32, colour: [u8; 4], tex: u32) {
    let base = mesh.vertices.len() as u32;
    for (dx, dy) in [(-s, -s), (-s, s), (s, s), (s, -s)] {
        mesh.vertices.push(GpuVertex {
            pos: [c[0] + dx, c[1] + dy, c[2]],
            colour,
            uv0: [0.5, 0.5],
            uv1: [0.5, 0.5],
        });
    }
    let first = mesh.indices.len() as u32;
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    mesh.batches.push(Batch {
        bucket: 1,
        pass: PassKind::VertexLit,
        tex0: Some(tex),
        tex1: None,
        lightmap: None,
        first_index: first,
        index_count: 6,
    });
}

fn synth_order() -> Option<SceneRun> {
    let rec = dc_client::exact::record();
    let ctx = context()?;
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut mesh = SceneMesh {
        textures: vec![b"a".to_vec(), b"b".to_vec()],
        ..SceneMesh::default()
    };
    // coplanar pairs in one bucket: under LESSEQUAL the later draw wins, so the frame shows the recording order
    quad(&mut mesh, [-30.0, 0.0, 100.0], 25.0, [220, 40, 40, 255], 0);
    quad(&mut mesh, [-20.0, 5.0, 100.0], 25.0, [40, 220, 40, 255], 1);
    quad(&mut mesh, [30.0, -10.0, 140.0], 30.0, [40, 40, 220, 255], 0);
    quad(&mut mesh, [40.0, -5.0, 140.0], 30.0, [220, 220, 40, 255], 1);
    // a diagonal edge across the screen (sub-pixel coverage)
    quad(&mut mesh, [0.0, 40.0, 180.0], 20.0, [200, 200, 200, 255], 0);
    let scene = r.upload(&ctx, &mesh, &mut |n| {
        Some(TextureImage::rgba(
            2,
            2,
            if n == b"a" {
                [255u8, 255, 255, 255]
            } else {
                [180, 180, 255, 255]
            }
            .repeat(4),
        ))
    });
    let mut frames = Vec::new();
    let fx = 90f32.to_radians();
    for (k, (yaw, pitch)) in [(0.0f32, 0.0f32), (0.13, -0.07)].into_iter().enumerate() {
        let cam = Camera::looking(
            glam::Vec3::ZERO,
            yaw,
            pitch,
            fx,
            dc_render::fov_y_for(fx, W, H),
        );
        let (img, _) = r
            .render_frame_offscreen(
                &ctx,
                &scene,
                &cam,
                &dc_render::FrameOptions::default(),
                &[],
                W,
                H,
            )
            .expect("frame");
        save_png(&img, &format!("synth_order_{k}"));
        frames.push(hash_json(&img, &format!("view{k}")));
    }
    let cam = Camera::looking(
        glam::Vec3::ZERO,
        0.0,
        0.0,
        fx,
        dc_render::fov_y_for(fx, W, H),
    );
    let mut settings =
        dc_client::exact::render_settings(W, H, &cam, 1.0, &RenderSettings::default(), 0, 0);
    settings["scene"] = json!({ "world": "synthetic coplanar quads", "views": 2 });
    let manifest = rec.manifest(Some(&ctx.info), None, settings);
    Some(SceneRun { frames, manifest })
}

fn hud_scene(name: &str, rom: Option<PathBuf>) -> Option<SceneRun> {
    let names = if rom.is_some() {
        ["casey", "alpha", "bravo", "charlie"]
    } else {
        SYNTH_NAMES
    };
    let mut src = Scripted {
        inner: Box::new(dc_client::testkit::synth_source()),
        frame: 0,
        place: None,
        focus: None,
        script: Box::new(move |_, f| FrameState {
            life_state: Some(1),
            hud_events: hud_events(f, f * FRAME_MS, names),
            ammo: Some(ammo()),
            ..FrameState::default()
        }),
    };
    let opts = HeadlessOptions {
        frames: 30,
        font_bytes: rom.is_none().then(dc_client::testkit::synth_font),
        font_rom: rom.clone(),
        hud: match &rom {
            Some(r) => HudChoice::Rom(r.clone()),
            None => HudChoice::Fallback,
        },
        debug_panel: false,
        ..base_opts(name)
    };
    let mut pan = Pan {
        yaw0: None,
        rate: 0.0,
    };
    headless_scene(
        name,
        &mut src,
        Some(&mut pan),
        &opts,
        json!({ "world": "testkit synth", "input": "still", "hud_only": true }),
    )
}

// ------------------------------------------------------------------------------------------------------------------
// ROM scenes
// ------------------------------------------------------------------------------------------------------------------

fn rom_world_views(rom: &Path) -> Option<SceneRun> {
    let rec = dc_client::exact::record();
    let ctx = context()?;
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut target = dc_client::offscreen::Offscreen::new(&ctx, OFFSCREEN_FORMAT, W, H);
    let gamma = dc_client::display::client_gamma(Some(rom));
    target.set_gamma(gamma);
    let view = dc_client::ViewSettings::default();
    let maps = RomWorld::map_names(rom).expect("ROM maps");
    let mut frames = Vec::new();
    for (m, map) in maps.iter().enumerate() {
        let mut src = RomWorld::new(rom, map);
        let Ok(world) = src.load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        })) else {
            frames.push(json!({ "frame": format!("map{m}"), "skipped": "load failed" }));
            continue;
        };
        let p = place_of(&world, None);
        for (k, turn) in [0.0f32, std::f32::consts::PI].into_iter().enumerate() {
            let v = ViewAngles {
                yaw: p.spawn.yaw + turn,
                pitch: 0.1,
            };
            let cam = view.camera(p.eye, &v, W, H);
            let img = target
                .render(
                    &ctx,
                    &renderer,
                    world.scene.as_ref().expect("scene"),
                    &cam,
                    None,
                )
                .expect("frame");
            save_png(&img, &format!("world_map{m}_{k}"));
            frames.push(hash_json(&img, &format!("map{m}_view{k}")));
        }
    }
    let cam = view.camera(V3::new(0.0, 0.0, 0.0), &ViewAngles::default(), W, H);
    let mut settings =
        dc_client::exact::render_settings(W, H, &cam, gamma, &RenderSettings::default(), 0, 0);
    settings["scene"] = json!({ "maps": maps.len(), "views_per_map": 2 });
    let manifest = rec.manifest(Some(&ctx.info), Some(rom), settings);
    Some(SceneRun { frames, manifest })
}

/// The ROM map of the character and effect scenes, when the ROM has it.
fn scene_map(rom: &Path) -> Option<String> {
    RomWorld::map_names(rom)
        .ok()?
        .into_iter()
        .find(|m| m.eq_ignore_ascii_case(MAP))
}

fn rom_characters(rom: &Path) -> Option<SceneRun> {
    let map = scene_map(rom)?;
    let table = dc_client::WeaponTable::open(rom).expect("weapon table");
    let row = table.get(WEAPON).expect("weapon row").clone();
    let gun = row.hh_model.clone().map(|m| ModelRef {
        model: m,
        textures: row.hh_skins.clone(),
    });
    let held = row.pv_model.clone().map(|m| ViewWeapon {
        inspect: None,
        weapon: WEAPON,
        model: Some(ModelRef {
            model: m,
            textures: row.pv_skins.clone(),
        }),
        clip: None,
        pos: row.pv_pos.unwrap_or([0.0; 3]),
    });
    let names = ["casey", "alpha", "bravo", "charlie"];
    let mut src = Scripted {
        inner: Box::new(RomWorld::new(rom, &map)),
        frame: 0,
        place: None,
        focus: None,
        script: Box::new(move |p, f| {
            let t = f * FRAME_MS;
            let body = |k: usize, obj: u16, pid: u8, team: u8, clip: usize| RemoteBody {
                obj,
                pid,
                name: names[pid as usize].into(),
                team,
                pos: p.ground[k].to_array(),
                half: p.stand.to_array(),
                yaw: Some(p.spawn.yaw + std::f32::consts::PI + 0.4 * k as f32),
                look: Some(RemoteLook {
                    styles: Vec::new(),
                    models: Vec::new(),
                    weapon: gun.clone().map(|g| (g, 2)),
                    trackers: vec![RemoteTracker {
                        clip,
                        // the animation time is the tick time (explicit ticks, no wall clock)
                        time_ms: t as f32,
                        blend_set: None,
                    }],
                    visible: true,
                    aim: None,
                }),
            };
            let drop = |k: usize, obj: u16, spin: bool| dc_client::drops::WeaponDrop {
                obj,
                kind: dc_client::drops::PICKUP_KIND_WEAPON,
                weapon: WEAPON,
                rounds: 60,
                spin,
                fall: true,
                pos: [
                    p.ground[k].x,
                    p.ground[k].y - p.stand.y + 16.0,
                    p.ground[k].z,
                ],
                rot: [
                    0.0,
                    (p.spawn.yaw * 0.5).sin(),
                    0.0,
                    (p.spawn.yaw * 0.5).cos(),
                ],
                model: gun.clone(),
                trackers: vec![RemoteTracker {
                    clip: 1,
                    time_ms: 0.0,
                    blend_set: None,
                }],
                seen_ms: 0,
            };
            FrameState {
                life_state: Some(1),
                remotes: vec![
                    body(0, 10, 1, 1, 0),
                    body(1, 11, 2, 2, 1),
                    body(2, 12, 3, 1, 2),
                ],
                drops: vec![drop(4, 300, false), drop(5, 301, true)],
                view_weapon: held.clone(),
                hud_events: hud_events(f, t, names),
                ammo: Some(ammo()),
                own_team: Some(1),
                ..FrameState::default()
            }
        }),
    };
    let opts = HeadlessOptions {
        frames: 30,
        font_rom: Some(rom.to_path_buf()),
        hud: HudChoice::Rom(rom.to_path_buf()),
        debug_panel: false,
        ..base_opts("rom_characters")
    };
    let mut pan = Pan {
        yaw0: None,
        rate: 0.004,
    };
    headless_scene(
        "rom_characters",
        &mut src,
        Some(&mut pan),
        &opts,
        json!({ "map": "scene map", "remotes": 3, "drops": 2, "view_weapon": true }),
    )
}

/// An eye with a clear line to `target`: candidates on rings around it, the first whose ray reaches the target's box.
fn eye_for(cw: &dc_sim::CollisionWorld, target: V3, reach: f32) -> V3 {
    for dist in [240.0f32, 300.0, 200.0, 360.0, 420.0] {
        for k in 0..16 {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            for up in [60.0f32, 110.0, 30.0] {
                let eye = V3::new(
                    target.x + dist * a.sin(),
                    target.y + up,
                    target.z + dist * a.cos(),
                );
                let clear = match cw.ray(eye, target) {
                    None => true,
                    Some(h) => (1.0 - h.t) * (target - eye).len() < reach,
                };
                if clear {
                    return eye;
                }
            }
        }
    }
    V3::new(target.x, target.y + 200.0, target.z - 300.0)
}

/// The room's `trig` setting for this world in a room of the mode with this key, as our lobby writes it.
fn trig_for(rom: &Path, world: &str, key: &str) -> Option<String> {
    let vfs = dc_rez::RezVfs::new(rom);
    let gr = dc_bute::GameRoomData::from_vfs(&vfs).ok()?;
    let unq = |s: &str| s.trim().trim_matches('"').to_string();
    let mode = gr.modes(false).iter().position(|m| {
        m.get("Key")
            .is_some_and(|k| unq(k).eq_ignore_ascii_case(key))
    })?;
    let base = |p: &str| {
        let n = p.replace('/', "\\").to_ascii_uppercase();
        let b = n.rsplit('\\').next().unwrap_or(&n).to_string();
        b.strip_suffix(".DAT").map(str::to_string).unwrap_or(b)
    };
    let want = base(world);
    for (i, m) in gr.maps().iter().enumerate() {
        if !m.contains_key(&format!("GMS_{key}")) {
            continue;
        }
        let (_md, doc) = dc_bute::gms_file_with_vfs(&vfs, &gr, i, mode, false);
        if doc
            .get_str("MapInfo", "Map")
            .is_some_and(|w| base(&unq(w)) == want)
        {
            let cfg = dc_bute::RoomConfig {
                map: i,
                mode,
                ..dc_bute::RoomConfig::default()
            };
            let line =
                dc_bute::room_line_with_vfs(&vfs, &gr, &cfg, &dc_bute::RoomOptions::default());
            return match dc_dserver::pipe::parse_control(&line) {
                dc_dserver::pipe::Control::Room(r) => r.get("trig").map(str::to_string),
                _ => None,
            };
        }
    }
    None
}

/// What the effect scene's world camera phase needs from the server's view of the map.
struct Breaks {
    set: dc_client::objects::ServerObjects,
    crate_name: Vec<u8>,
    crate_fx: WorldFx,
    crate_centre: V3,
    drum_name: Option<Vec<u8>>,
    drum_fx: Option<WorldFx>,
    blast: Option<ShotRecord>,
}

fn breaks(rom: &Path, map: &str) -> Option<Breaks> {
    use dc_dserver::destructibles::DeathAction;
    use dc_dserver::worldobj::{brush_flags, plan_scene, WorldObject, WorldObjects, ZoneClass};
    let info = dc_dserver::world::load_from_rom(
        rom,
        &format!("Worlds\\{map}"),
        dc_dserver::world::NameForm::Listed,
        None,
    )
    .ok()?;
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(rom);
    let trig = trig_for(rom, &info.announce, "TSV");
    let mut next = 1000u16..;
    let mut set = dc_client::objects::ServerObjects::default();
    for o in plan_scene(
        &info,
        &tables,
        WorldObjects::World,
        ZoneClass::Room,
        12,
        trig.as_deref(),
        || next.next().unwrap(),
    ) {
        if let WorldObject::Brush { brush, .. } = o {
            set.brushes.insert(
                brush.name.to_ascii_lowercase(),
                brush_flags(&brush).0 & !0x0024,
            );
        }
    }
    let pick = |explosive: bool| {
        info.destructibles.breakables.iter().find(|b| {
            let fx = b.death.iter().any(|d| matches!(d, DeathAction::Effect(_)));
            let ex = b.death.iter().any(|d| matches!(d, DeathAction::Explode(_)));
            fx && ex == explosive
                && info.destructibles.shape(&b.name).is_some()
                && set.brushes.contains_key(&b.name.to_ascii_lowercase())
        })
    };
    let fx_of = |b: &dc_dserver::destructibles::BreakableInfo| {
        b.death.iter().find_map(|d| match d {
            DeathAction::Effect(n) => info
                .effects
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(n.as_slice()))
                .map(|e| WorldFx {
                    name: String::from_utf8_lossy(&e.fx).into_owned(),
                    pos: e.pos,
                    rot: dc_dserver::worldobj::euler_quat(e.rot),
                    obj: None,
                    value: 0,
                }),
            _ => None,
        })
    };
    let c = pick(false)?;
    let crate_fx = fx_of(c)?;
    let (lo, hi) = info
        .destructibles
        .shape(&c.name)
        .and_then(|s| s.models.first().map(|m| m.bounds()))?;
    let d = pick(true);
    let blast = d.and_then(|d| {
        let x = d.death.iter().find_map(|a| match a {
            DeathAction::Explode(i) => info.destructibles.explosions.get(*i).cloned(),
            _ => None,
        })?;
        let name = x.impact_fx.clone()?;
        let ammo = tables.weapons.ammo.iter().find_map(|(id, a)| {
            a.impact_fx
                .as_ref()
                .filter(|n| n.as_bytes() == name.as_slice())
                .map(|_| *id)
        })?;
        let weapon = tables
            .weapons
            .weapons
            .iter()
            .find_map(|(w, row)| (row.ammo == Some(ammo)).then_some(*w))?;
        Some(ShotRecord {
            shooter: 0xFFFF,
            first: NO_OBJECT,
            weapon,
            ammo,
            from: x.pos,
            to: x.pos,
            normal: [0.0, 1.0, 0.0],
            surface: 0,
            clear: dc_dserver::gs::SHOT_MASK_MISS,
            own: false,
            mods: [0xff; 3],
        })
    });
    Some(Breaks {
        crate_name: c.name.to_ascii_lowercase(),
        crate_fx,
        crate_centre: (lo + hi) * 0.5,
        drum_name: d.map(|d| d.name.to_ascii_lowercase()),
        drum_fx: d.and_then(fx_of),
        blast,
        set,
    })
}

fn rom_fx(rom: &Path) -> Option<SceneRun> {
    let map = scene_map(rom)?;
    let b = breaks(rom, &map)?;
    let table = dc_client::WeaponTable::open(rom).expect("weapon table");
    let row = table.get(WEAPON).expect("weapon row").clone();
    let ammo_id = row.ammo.unwrap_or(0);
    let held = row.pv_model.clone().map(|m| ViewWeapon {
        inspect: None,
        weapon: WEAPON,
        model: Some(ModelRef {
            model: m,
            textures: row.pv_skins.clone(),
        }),
        clip: None,
        pos: row.pv_pos.unwrap_or([0.0; 3]),
    });
    const BREAK_AT: u32 = 24;
    let mut whole = b.set.clone();
    whole.brushes.insert(b.crate_name.clone(), 0x3101);
    if let Some(d) = &b.drum_name {
        whole.brushes.insert(d.clone(), 0x3101);
    }
    let mut broken = whole.clone();
    broken.brushes.insert(b.crate_name.clone(), 0x0100);
    if let Some(d) = &b.drum_name {
        broken.brushes.insert(d.clone(), 0x0100);
    }
    let crate_centre = b.crate_centre;
    let mut cam: Option<dc_client::camshot::CamShotView> = None;
    let mut src = Scripted {
        inner: Box::new(RomWorld::new(rom, &map)),
        frame: 0,
        place: None,
        focus: Some(b.crate_centre),
        script: Box::new(move |p, f| {
            let mut fs = FrameState {
                life_state: Some(1),
                view_weapon: held.clone(),
                world_objects: Some(if f < BREAK_AT {
                    whole.clone()
                } else {
                    broken.clone()
                }),
                ..FrameState::default()
            };
            // phase 1: own shots at the walls ahead every 4th tick (muzzle flash, tracer, impact, mark), and a
            // remote shooter's records (tracer and impact)
            if f < BREAK_AT && f % 4 == 1 {
                for (k, (at, normal, surface)) in p.walls.iter().enumerate() {
                    let dir = (*at - p.eye).normalized();
                    if k == 0 {
                        fs.fx_events.push(FxEvent::Shot(dc_client::arms::ShotFx {
                            weapon: WEAPON,
                            ammo: ammo_id,
                            origin: p.eye.to_array(),
                            muzzle: p.eye.to_array(),
                            dir: dir.to_array(),
                            range: 8000.0,
                            hit: dc_client::arms::RayHit::Miss,
                            first: true,
                            time_ms: f * FRAME_MS,
                            impact: None,
                            flash_mod: None,
                        }));
                    } else {
                        let from = p.ground[k % p.ground.len()] + V3::new(0.0, 30.0, 0.0);
                        fs.fx_events.push(FxEvent::Record(ShotRecord {
                            shooter: 10,
                            first: NO_OBJECT,
                            weapon: WEAPON,
                            ammo: ammo_id,
                            from: from.to_array(),
                            to: at.to_array(),
                            normal: normal.to_array(),
                            surface: (*surface).min(255) as u8,
                            clear: 0,
                            own: false,
                            mods: [0xff; 3],
                        }));
                    }
                }
            }
            // phase 2: a world camera on the crate; the crate and the drum break, the drum's blast record
            if f >= BREAK_AT - 2 {
                let c = *cam.get_or_insert_with(|| {
                    let eye = p.focus_eye.expect("the crate camera");
                    let look = dc_client::ShootTest::look_at(
                        eye.to_array(),
                        (crate_centre + V3::new(0.0, 40.0, 0.0)).to_array(),
                    );
                    let q = glam::Quat::from_rotation_y(look.yaw)
                        * glam::Quat::from_rotation_x(look.pitch);
                    dc_client::camshot::CamShotView {
                        obj: 900,
                        pos: eye.to_array(),
                        rot: Some(q.to_array()),
                        allow_move: false,
                        listener: false,
                        fov: [0.0, 0.0],
                    }
                });
                fs.cam_shot = Some(c);
            }
            if f == BREAK_AT {
                fs.fx_events.push(FxEvent::World(b.crate_fx.clone()));
                if let Some(d) = &b.drum_fx {
                    fs.fx_events.push(FxEvent::World(d.clone()));
                }
                if let Some(r) = b.blast {
                    fs.fx_events.push(FxEvent::Record(r));
                }
            }
            fs
        }),
    };
    let opts = HeadlessOptions {
        frames: 64,
        debug_panel: false,
        ..base_opts("rom_fx")
    };
    let mut pan = Pan {
        yaw0: None,
        rate: 0.0,
    };
    headless_scene(
        "rom_fx",
        &mut src,
        Some(&mut pan),
        &opts,
        json!({ "map": "scene map", "break_at": BREAK_AT, "phases": "own shots, world camera" }),
    )
}

// ------------------------------------------------------------------------------------------------------------------
// running and comparing
// ------------------------------------------------------------------------------------------------------------------

/// Every scene this machine can run, by name (ROM scenes only with `CA_ROM`). `None`: no GPU adapter.
fn run_all() -> Option<BTreeMap<String, SceneRun>> {
    let mut out = BTreeMap::new();
    out.insert("synth_order".to_string(), synth_order()?);
    out.insert("synth_frame_loop".to_string(), synth_frame_loop()?);
    out.insert("hud_fallback".to_string(), hud_scene("hud_fallback", None)?);
    if let Some(rom) = dc_rom::test_rom() {
        out.insert("rom_world_views".to_string(), rom_world_views(&rom)?);
        if let Some(s) = rom_characters(&rom) {
            out.insert("rom_characters".to_string(), s);
        }
        if let Some(s) = rom_fx(&rom) {
            out.insert("rom_fx".to_string(), s);
        }
        out.insert("rom_hud".to_string(), hud_scene("rom_hud", Some(rom))?);
    }
    Some(out)
}

fn to_json(runs: &BTreeMap<String, SceneRun>) -> Value {
    let mut scenes = serde_json::Map::new();
    for (k, r) in runs {
        scenes.insert(
            k.clone(),
            json!({ "frames": r.frames, "manifest": r.manifest }),
        );
    }
    Value::Object(scenes)
}

/// Scene -> frame hashes (the comparable part of a run).
fn hashes(v: &Value) -> BTreeMap<String, Vec<Value>> {
    v.as_object()
        .map(|o| {
            o.iter()
                .map(|(k, s)| {
                    (
                        k.clone(),
                        s["frames"].as_array().cloned().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The frames of `a` and `b` that differ, per scene (empty: identical).
fn differing(a: &Value, b: &Value) -> Vec<String> {
    let (ha, hb) = (hashes(a), hashes(b));
    let mut out = Vec::new();
    for (scene, fa) in &ha {
        let Some(fb) = hb.get(scene) else {
            out.push(format!("{scene}: missing"));
            continue;
        };
        if fa.len() != fb.len() {
            out.push(format!("{scene}: {} vs {} frames", fa.len(), fb.len()));
            continue;
        }
        for (x, y) in fa.iter().zip(fb) {
            if x != y {
                out.push(format!("{scene}: frame {}", x["frame"]));
            }
        }
    }
    out
}

/// Runs this test binary's child entry with `envs` and returns its scenes.
fn child(tag: &str, envs: &[(&str, String)]) -> Value {
    let dir = std::env::temp_dir().join(format!("dc_exact_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join(format!("{tag}.json"));
    let _ = std::fs::remove_file(&out);
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args([
        "exact_child_entry",
        "--exact",
        "--nocapture",
        "--test-threads=1",
    ])
    .env("DC_EXACT_CHILD_OUT", &out)
    .env_remove("DC_DETERMINISM_MUTATE")
    .env_remove("DC_ASYNC_LOADS")
    .env_remove("DC_PROF");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let t = std::time::Instant::now();
    let o = cmd.output().expect("child test process");
    assert!(
        o.status.success(),
        "child {tag} failed: {}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    println!("child {tag}: {:.1} s", t.elapsed().as_secs_f64());
    let v: Value = serde_json::from_slice(&std::fs::read(&out).expect("child output")).unwrap();
    v
}

/// Busy threads on every core but a few, while `f` runs.
fn under_load<T>(threads: usize, f: impl FnOnce() -> T) -> T {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let workers: Vec<_> = (0..threads)
        .map(|i| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut x = i as u64 + 1;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    for _ in 0..10_000 {
                        x = x
                            .wrapping_mul(6364136223846793005)
                            .wrapping_add(1442695040888963407);
                    }
                    std::hint::black_box(x);
                }
            })
        })
        .collect();
    let r = f();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for w in workers {
        let _ = w.join();
    }
    r
}

/// The entry of a child process (does nothing unless `DC_EXACT_CHILD_OUT` names its output file).
#[test]
fn exact_child_entry() {
    let Some(out) = std::env::var_os("DC_EXACT_CHILD_OUT") else {
        return;
    };
    let runs = run_all().expect("a GPU adapter in the child");
    std::fs::write(out, serde_json::to_vec_pretty(&to_json(&runs)).unwrap()).unwrap();
}

#[test]
fn exact_lane_repeats_in_one_process() {
    if std::env::var_os("DC_EXACT_CHILD_OUT").is_some() {
        return;
    }
    let Some(first) = run_all() else {
        println!("skipped: no GPU adapter");
        return;
    };
    let first = to_json(&first);
    for run in 1..3 {
        let again = to_json(&run_all().expect("adapter"));
        let d = differing(&first, &again);
        assert!(d.is_empty(), "in-process run {run} differs: {d:?}");
    }
    let n: usize = hashes(&first).values().map(Vec::len).sum();
    println!(
        "3 in-process runs identical: {} scenes, {n} hashes",
        hashes(&first).len()
    );
}

#[test]
fn exact_lane_across_processes_load_inflight_and_mutations() {
    if std::env::var_os("DC_EXACT_CHILD_OUT").is_some() {
        return;
    }
    if context().is_none() {
        println!("skipped: no GPU adapter");
        return;
    }
    let base = child("a", &[]);
    let b = under_load(24, || child("b_load24", &[]));
    let prof_out = std::env::temp_dir().join(format!("dc_exact_prof_{}.json", std::process::id()));
    let c = child(
        "c_inflight3",
        &[
            ("DC_PROF", prof_out.display().to_string()),
            ("DC_PROF_INFLIGHT", "3".into()),
            ("DC_PROF_READBACK", "1".into()),
            ("DC_PROF_HUDFORCE", "1".into()),
            ("DC_PROF_WARMUP", "0".into()),
        ],
    );
    for (tag, other) in [("b_load24", &b), ("c_inflight3", &c)] {
        let d = differing(&base, other);
        assert!(d.is_empty(), "process {tag} differs from process a: {d:?}");
        for (scene, s) in base.as_object().unwrap() {
            assert_eq!(
                s["manifest"]["inputs_digest"], other[scene]["manifest"]["inputs_digest"],
                "{scene}: the same inputs in {tag}"
            );
        }
    }
    println!(
        "3 separate processes identical (one under 24 busy threads, one with 3 frames in flight): {} scenes",
        hashes(&base).len()
    );
    // profile mode without DC_PROF_HUDFORCE: the HUD is composed only when due, a different render policy recorded
    // as an input (render_settings.hud_cadence). Scenes without a HUD keep their inputs and hashes; scenes with a HUD
    // change their inputs_digest, and a frame hash may differ only there (contract section 2.1).
    let d_run = child(
        "d_prof_hud_due",
        &[
            ("DC_PROF", prof_out.display().to_string()),
            ("DC_PROF_INFLIGHT", "3".into()),
            ("DC_PROF_READBACK", "1".into()),
            ("DC_PROF_WARMUP", "0".into()),
        ],
    );
    let d = differing(&base, &d_run);
    let mut hud_scenes = 0;
    for (scene, s) in base.as_object().unwrap() {
        let o = &d_run[scene];
        let has_hud = s["manifest"]["render_settings"]["hud"]
            .as_str()
            .is_some_and(|h| h != "off");
        assert_eq!(
            o["manifest"]["render_settings"]["hud_cadence"].is_string(),
            has_hud,
            "{scene}: hud_cadence recorded exactly when a headless scene draws a HUD"
        );
        if has_hud {
            hud_scenes += 1;
            assert_ne!(
                s["manifest"]["inputs_digest"], o["manifest"]["inputs_digest"],
                "{scene}: the HUD cadence is an input"
            );
        } else {
            assert_eq!(
                s["manifest"]["inputs_digest"], o["manifest"]["inputs_digest"],
                "{scene}: the same inputs without a HUD"
            );
            assert!(
                !d.iter().any(|x| x.starts_with(&format!("{scene}:"))),
                "{scene}: no HUD, no hash may change in profile mode: {d:?}"
            );
        }
    }
    assert!(
        d.iter().any(|x| x.starts_with("hud_fallback:")),
        "the due-only HUD cadence changes hud_fallback frames (a deterministic, documented difference)"
    );
    println!(
        "profile mode without DC_PROF_HUDFORCE: {} frames differ, all in the {hud_scenes} HUD scenes, whose inputs record the cadence: {:?}",
        d.len(),
        d.iter().take(8).collect::<Vec<_>>()
    );
    // a deliberate arithmetic change and a deliberate ordering change are detected
    for knob in ["arith", "order"] {
        let m = child(
            &format!("m_{knob}"),
            &[("DC_DETERMINISM_MUTATE", knob.to_string())],
        );
        let d = differing(&base, &m);
        println!(
            "mutation {knob}: {} frames differ: {:?}",
            d.len(),
            d.iter().take(6).collect::<Vec<_>>()
        );
        assert!(
            d.iter().any(|x| x.starts_with("synth_order")),
            "{knob}: the synthetic order scene must change"
        );
        if knob == "arith" {
            assert!(
                d.iter().any(|x| x.starts_with("synth_frame_loop")),
                "arith: the frame loop scene must change"
            );
        }
        if dc_rom::test_rom().is_some() {
            assert!(
                d.iter().any(|x| x.starts_with("rom_world_views")),
                "{knob}: the ROM world views must change"
            );
        }
    }
    // the golden hashes of this lane
    golden_check(&base);
}

/// Compares a run with the golden file's section for its lane (no section: reported, failing only with
/// `DC_EXACT_REQUIRE_GOLDEN=1`). A scene whose ROM inputs differ from the golden's (another ROM) is not compared.
fn golden_check(run: &Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    let require = std::env::var("DC_EXACT_REQUIRE_GOLDEN").is_ok_and(|v| v == "1");
    let golden: Value = match std::fs::read(&path) {
        Ok(b) => serde_json::from_slice(&b).expect("golden JSON"),
        Err(e) => {
            assert!(!require, "no golden file {}: {e}", path.display());
            println!("no golden file ({e})");
            return;
        }
    };
    let Some(lane) = run
        .as_object()
        .and_then(|o| o.values().next())
        .and_then(|s| s["manifest"]["lane_key"].as_str())
    else {
        return;
    };
    let Some(g) = golden["lanes"].get(lane) else {
        assert!(!require, "no golden section for lane {lane}");
        println!("no golden section for lane {lane}: not compared (tools/exact_golden.py --update adds one)");
        return;
    };
    let mut compared = 0;
    for (scene, s) in run.as_object().unwrap() {
        let Some(gs) = g["scenes"].get(scene) else {
            println!("golden: scene {scene} not in the golden section");
            continue;
        };
        if gs["rom_digest"] != s["manifest"]["inputs"]["rom"]["digest"] {
            println!("golden: {scene} read other ROM data than the golden run: not compared");
            continue;
        }
        let want = gs["frames"].as_array().cloned().unwrap_or_default();
        let got = s["frames"].as_array().cloned().unwrap_or_default();
        let code_same = gs["code_digest"] == s["manifest"]["inputs"]["code_digest"];
        let bad: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(a, b)| a != b)
            .map(|(a, _)| a["frame"].to_string())
            .collect();
        assert!(
            want.len() == got.len() && bad.is_empty(),
            "golden mismatch in {scene} ({} vs {} frames; differing frames {bad:?}); shaders / pipelines / settings \
             {}: an intended change regenerates the goldens with tools/exact_golden.py --update (reviewed)",
            want.len(),
            got.len(),
            if code_same { "unchanged (a pixel regression or nondeterminism)" } else { "CHANGED" }
        );
        compared += 1;
    }
    println!("golden: {compared} scenes equal the golden hashes of lane {lane}");
}
