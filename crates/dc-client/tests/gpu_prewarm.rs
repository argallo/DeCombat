//! gpu-prewarm: after the match's pre-warm (`dc_client::prewarm::match_gpu`, what the match window runs at match
//! load and after a device or renderer rebuild) no pipeline or shader module is made during frames of play that load
//! and draw characters (one with the server-announced infected style), the own first-person weapon, the x-ray chams
//! and weapon effects, and none is ever made on a model loader thread. A render style the pre-warm did not cover
//! (here: a match pre-warmed with the built-in styles only) is made on the frame thread when the model is requested,
//! logged as made in play, and the model draws what an inline load draws. Skipped without `CA_ROM` or a GPU adapter;
//! nothing of the ROM is asserted by value and no image is written.
// ORIGINAL: our own test of the pipeline pre-warm (gpu-prewarm); no 2009 behaviour

use std::sync::Mutex;
use std::time::Instant;

use dc_client::arms::{RayHit, ShotFx};
use dc_client::characters::ModelScene;
use dc_client::fx::{FxEvent, FxStage};
use dc_client::offscreen::Offscreen;
use dc_client::world::{FrameState, GpuTarget};
use dc_client::{
    prewarm, LocalPlayer, ModelRef, RemoteBody, RemoteLook, RemoteTracker, RomWorld, ShootTest,
    ViewAngles, ViewSettings, ViewWeapon, WeaponTable, WorldSource,
};
use dc_render::{
    warm, AdapterChoice, GpuContext, RenderSettings, WorldRenderer, XrayMode, XrayModel,
    OFFSCREEN_FORMAT,
};
use dc_sim::{drop_to_floor, Cylinder, MovementParams, V3};

/// The pre-warm counters are process-wide: the tests here run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn gpu() -> Option<GpuContext> {
    match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => Some(c),
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            None
        }
    }
}

/// Counters of the pre-warm bookkeeping at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Counts {
    creates: u64,
    play: u64,
    forbidden: u64,
    unready: u64,
}

fn counts() -> Counts {
    Counts {
        creates: warm::creates(),
        play: warm::play_creates(),
        forbidden: warm::forbidden_creates(),
        unready: warm::unready_draws(),
    }
}

fn remote(
    obj: u16,
    at: V3,
    yaw: f32,
    half: [f32; 3],
    gun: &Option<ModelRef>,
    models: &[ModelRef],
    styles: &[&str],
) -> RemoteBody {
    RemoteBody {
        obj,
        pid: obj as u8,
        name: format!("Remote{obj}"),
        team: 1,
        pos: at.to_array(),
        half,
        yaw: Some(yaw),
        look: Some(RemoteLook {
            styles: styles.iter().map(|s| s.to_string()).collect(),
            // the server's model block (styles apply to a look with models; an empty one is the default character)
            models: models.to_vec(),
            weapon: gun.clone().map(|g| (g, 2)),
            trackers: vec![RemoteTracker {
                clip: 0,
                time_ms: 0.0,
                blend_set: None,
            }],
            visible: true,
            aim: None,
        }),
    }
}

#[test]
fn rom_no_pipeline_is_made_in_play_after_the_match_prewarm() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Some(ctx) = gpu() else {
        return;
    };
    prewarm::load_begins();
    let t_load = Instant::now();
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let map = RomWorld::map_names(&rom).unwrap().first().cloned().unwrap();
    let mut src = RomWorld::new(&rom, &map);
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("ROM world");
    let scene = world.scene.as_ref().expect("scene");
    let mut models = ModelScene::open(&rom).expect("ROM models");
    models.set_async_loads(true);
    let mut stage = FxStage::open(Some(&rom));
    stage.set_msaa(renderer.msaa());
    let load_ms = t_load.elapsed().as_secs_f64() * 1000.0;

    let p = prewarm::match_gpu(&ctx, &renderer, Some(&models), &mut stage, OFFSCREEN_FORMAT);
    println!(
        "prewarm on {map}: {} creates in {:.1} ms ({} model pipelines, {} samplers, {} styles; effects made {}); \
         world + renderer + models load before it {load_ms:.0} ms",
        p.creates, p.ms, p.models.pipelines, p.models.samplers, p.models.styles, p.fx
    );
    assert!(warm::in_play());
    assert!(p.models.styles > 2, "the ROM's render styles were walked");
    assert!(p.models.pipelines > 2, "model pipelines made up front");
    assert!(p.fx, "the effect pass made up front");
    let model_pipes = renderer.models().pipeline_count();

    let spawn = world.spawn(0).expect("spawn 0");
    let params = MovementParams::default();
    let me = LocalPlayer::spawn(params, &world.collision, &spawn, world.stand_dims, 60);
    let view0 = ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    };
    let eye = me.eye_target(&view0);
    let fwd = V3::new(spawn.yaw.sin(), 0.0, spawn.yaw.cos());
    let side = V3::new(fwd.z, 0.0, -fwd.x);
    let floor = |p: V3| {
        drop_to_floor(
            &world.collision,
            &Cylinder::from_dims(world.stand_dims),
            p,
            params.step(),
            1000.0,
        )
    };
    let a = floor(me.state.pos + fwd * 170.0);
    let b = floor(me.state.pos + fwd * 170.0 + side * 50.0);
    let c = floor(me.state.pos + fwd * 170.0 - side * 50.0);
    let view = ShootTest::look_at(eye.to_array(), a.to_array());
    let (w, h) = (320u32, 240u32);
    let camera = ViewSettings::default().camera(eye, &view, w, h);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let table = WeaponTable::open(&rom).expect("weapon table");
    let row = table.get(145).expect("weapon 145").clone();
    let gun = row.hh_model.clone().map(|m| ModelRef {
        model: m,
        textures: row.hh_skins.clone(),
    });
    let held = row.pv_model.clone().map(|m| ViewWeapon {
        inspect: None,
        weapon: 145,
        model: Some(ModelRef {
            model: m,
            textures: row.pv_skins.clone(),
        }),
        clip: None,
        pos: [0.0; 3],
    });
    let half = world.stand_dims.to_array();
    let back = spawn.yaw + std::f32::consts::PI;
    // a plain character and two with the server-announced infected styles (Quarantine)
    let look = models.default_look();
    assert!(!look.is_empty());
    let bodies = [
        remote(7, a, back, half, &gun, &[], &[]),
        remote(8, b, back, half, &gun, &look, &["rs/infected.ltb"]),
        remote(9, c, back, half, &gun, &look, &["rs/infected_b.ltb"]),
    ];
    let eye_v = glam::Vec3::new(eye.x, eye.y, eye.z);
    let lights = world.model_lights.as_ref();
    let origin = [eye.x, eye.y, eye.z];
    let dir = dc_client::arms::forward(view.yaw, view.pitch);

    let before = counts();
    let (mut fx_quads, mut xray_frames, mut model_frames) = (0usize, 0usize, 0usize);
    for f in 0..90u32 {
        let now_ms = 1000 + f * 16;
        models.update_remotes(&ctx, &renderer, &bodies, lights);
        models.place_view_weapon(&ctx, &renderer, held.as_ref(), eye_v, lights);
        if f == 5 {
            // the loads finish while frames are drawn (they hand back on the loader threads' own time)
            models.wait_for_loads();
        }
        // a shot every 10 frames: muzzle flash, tracer, impact
        let events = if f % 10 == 0 {
            vec![FxEvent::Shot(ShotFx {
                weapon: 145,
                ammo: row.ammo.unwrap_or(145),
                origin,
                muzzle: origin,
                dir,
                range: 3000.0,
                hit: RayHit::Miss,
                first: f == 0,
                time_ms: now_ms,
                impact: None,
                flash_mod: None,
            })]
        } else {
            Vec::new()
        };
        let fs = FrameState {
            fx_events: events,
            ..FrameState::default()
        };
        stage.frame(now_ms, &fs, &world);
        let socket = stage.view_socket(&models);
        let inst = models.instances();
        let view_inst = models.view_instances();
        let chams = models.character_instances(8);
        let xray: Vec<XrayModel<'_>> = if chams.is_empty() {
            Vec::new()
        } else {
            vec![XrayModel {
                instances: chams,
                mode: XrayMode::Chams,
                primary: [255, 0, 0, 160],
                secondary: [255, 255, 0, 255],
            }]
        };
        target
            .render_with_xray(
                &ctx,
                &renderer,
                scene,
                &camera,
                None,
                &inst,
                &view_inst,
                &xray,
                Some((&mut stage, now_ms, socket)),
                None,
                None,
            )
            .expect("frame");
        fx_quads += stage.drawn();
        xray_frames += usize::from(!xray.is_empty());
        model_frames += usize::from(!inst.is_empty());
    }
    let after = counts();
    let (made, _) = warm::made();
    println!(
        "90 frames: {model_frames} with models, {xray_frames} with x-ray chams, {fx_quads} effect quads; \
         characters {}, counters {before:?} -> {after:?}, made in play {made:?}",
        models.stats.characters
    );
    assert_eq!(models.stats.characters, 3, "the three characters drawn");
    assert!(models.stats.view_weapon, "the own weapon drawn");
    assert!(model_frames > 0 && xray_frames > 0 && fx_quads > 0);
    assert_eq!(
        after, before,
        "no pipeline or shader module made in play, none on a loader thread, no pass skipped"
    );
    assert_eq!(renderer.models().pipeline_count(), model_pipes);
    prewarm::load_begins();
}

#[test]
fn rom_a_style_announced_mid_match_is_made_on_the_frame_thread() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Some(ctx) = gpu() else {
        return;
    };
    prewarm::load_begins();
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let map = RomWorld::map_names(&rom).unwrap().first().cloned().unwrap();
    let mut src = RomWorld::new(&rom, &map);
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("ROM world");
    let scene = world.scene.as_ref().expect("scene");
    let mut stage = FxStage::open(Some(&rom));
    stage.set_msaa(renderer.msaa());
    // the built-in styles only: every ROM style is "new" to this match
    prewarm::match_gpu(&ctx, &renderer, None, &mut stage, OFFSCREEN_FORMAT);
    let made_before = warm::made().0.len();
    let before = counts();

    let spawn = world.spawn(0).expect("spawn 0");
    let params = MovementParams::default();
    let me = LocalPlayer::spawn(params, &world.collision, &spawn, world.stand_dims, 60);
    let view0 = ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    };
    let eye = me.eye_target(&view0);
    let fwd = V3::new(spawn.yaw.sin(), 0.0, spawn.yaw.cos());
    let at = drop_to_floor(
        &world.collision,
        &Cylinder::from_dims(world.stand_dims),
        me.state.pos + fwd * 170.0,
        params.step(),
        1000.0,
    );
    let view = ShootTest::look_at(eye.to_array(), at.to_array());
    let (w, h) = (320u32, 240u32);
    let camera = ViewSettings::default().camera(eye, &view, w, h);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let lights = world.model_lights.as_ref();
    // a ROM style with a blending pass (a raster state the built-in styles do not have; the infected styles share
    // the built-in state)
    let vfs = dc_rez::RezVfs::new(&rom);
    let mut lib = dc_render::StyleLibrary::new();
    let announced = vfs
        .list_entries("RS.rez")
        .expect("RS.rez")
        .into_iter()
        .map(|(n, _, _)| n)
        .find(|n| {
            lib.get(&vfs, n)
                .is_some_and(|s| s.passes.iter().any(|p| p.blend.is_some()))
        })
        .expect("a blending style");
    println!("announced style: {announced}");
    // the server announces the style: the request alone makes its pipelines, here
    let mut lazy = ModelScene::open(&rom).expect("ROM models");
    lazy.set_async_loads(true);
    let infected = [remote(
        7,
        at,
        spawn.yaw + std::f32::consts::PI,
        world.stand_dims.to_array(),
        &None,
        &lazy.default_look(),
        &[announced.as_str()],
    )];
    lazy.update_remotes(&ctx, &renderer, &infected, lights);
    let requested = counts();
    lazy.wait_for_loads();
    lazy.update_remotes(&ctx, &renderer, &infected, lights);
    assert_eq!(lazy.stats.characters, 1);
    let frame = |m: &ModelScene| {
        target
            .render_with_view(
                &ctx,
                &renderer,
                scene,
                &camera,
                None,
                &m.instances(),
                &m.view_instances(),
                None,
            )
            .expect("frame")
    };
    let got = frame(&lazy);
    let after = counts();
    let (made, _) = warm::made();
    let new: Vec<_> = made[made_before..].to_vec();
    let me_thread = std::thread::current().name().unwrap_or("?").to_string();
    println!(
        "made in play on the request: {new:?}; counters {before:?} -> {requested:?} -> {after:?}"
    );
    assert!(
        requested.play > before.play,
        "the announced style's pipelines were made at the request"
    );
    assert_eq!(
        after.play, requested.play,
        "nothing made later (loader, frame)"
    );
    assert_eq!(after.forbidden, before.forbidden, "none on a loader thread");
    assert_eq!(after.unready, before.unready, "no pass skipped");
    assert!(!new.is_empty());
    for m in &new {
        assert_eq!(m.kind, "pipeline");
        assert!(m.what.starts_with("model "), "a model pipeline: {}", m.what);
        assert_eq!(m.thread, me_thread, "made on the frame thread: {m:?}");
        assert!(!m.forbidden);
    }

    // no behaviour change: the same character loaded inline draws the same pixels
    let mut inline = ModelScene::open(&rom).expect("ROM models");
    inline.update_remotes(&ctx, &renderer, &infected, lights);
    assert_eq!(inline.stats.characters, 1);
    assert!(
        frame(&inline).rgba == got.rgba,
        "a style made on the request draws as an inline load does"
    );
    prewarm::load_begins();
}
