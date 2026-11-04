//! gpu-recover (CLI-460 amended): a GPU device loss inside the match is followed by a rebuild on a NEW device, and
//! the client draws the same frame again from the same world state, without the leave-the-match path. The loss is
//! the loss path without a driver reset (`gpulost::simulate_loss`, what `DC_FAULT=gpu_lost` runs in the window):
//! the frame thread unwinds out of the frame, the lost device's objects are retired, a new device gets the world's
//! render scene loaded again from the ROM (with the server's state applied again, `LoadedWorld::adopt_scene`) and
//! the models upload again from the CPU caches (`ModelScene::gpu_reset`). Skipped without `CA_ROM` or a GPU adapter;
//! nothing of the ROM is asserted by value and no image is written.
// ORIGINAL: our own test of the device rebuild (gpu-recover); no 2009 behaviour

use dc_client::characters::ModelScene;
use dc_client::gpulost;
use dc_client::offscreen::Offscreen;
use dc_client::world::GpuTarget;
use dc_client::{
    LocalPlayer, ModelRef, RemoteBody, RemoteLook, RemoteTracker, RomWorld, ShootTest, ViewAngles,
    ViewSettings, ViewWeapon, WeaponTable, WorldSource,
};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use dc_sim::{drop_to_floor, Cylinder, MovementParams, V3};

#[test]
fn a_lost_device_is_rebuilt_and_draws_the_same_frame_from_the_same_world() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return;
        }
    };
    gpulost::watch(&ctx.device);
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let map = RomWorld::map_names(&rom).unwrap().first().cloned().unwrap();
    let mut src = RomWorld::new(&rom, &map);
    let mut world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("ROM world");
    let spawn = world.spawn(0).expect("spawn 0");
    let params = MovementParams::default();
    let me = LocalPlayer::spawn(params, &world.collision, &spawn, world.stand_dims, 60);
    let view0 = ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    };
    let eye = me.eye_target(&view0);
    let fwd = V3::new(spawn.yaw.sin(), 0.0, spawn.yaw.cos());
    let floor = |w: &dc_client::LoadedWorld, p: V3| {
        drop_to_floor(
            &w.collision,
            &Cylinder::from_dims(w.stand_dims),
            p,
            params.step(),
            1000.0,
        )
    };
    let centre = floor(&world, me.state.pos + fwd * 170.0);
    let view = ShootTest::look_at(eye.to_array(), centre.to_array());
    let (w, h) = (320u32, 240u32);
    let camera = ViewSettings::default().camera(eye, &view, w, h);
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
    let one = [RemoteBody {
        obj: 7,
        pid: 1,
        name: "Remote7".into(),
        team: 1,
        pos: centre.to_array(),
        half: world.stand_dims.to_array(),
        yaw: Some(spawn.yaw + std::f32::consts::PI),
        look: Some(RemoteLook {
            styles: Vec::new(),
            models: Vec::new(),
            weapon: gun.clone().map(|g| (g, 2)),
            trackers: vec![RemoteTracker {
                clip: 0,
                time_ms: 0.0,
                blend_set: None,
            }],
            visible: true,
            aim: None,
        }),
    }];
    let eye_v = glam::Vec3::new(eye.x, eye.y, eye.z);
    let lights = world.model_lights.clone();

    // before the loss: the world, a remote character and the own weapon, loaded on loader threads
    let mut models = ModelScene::open(&rom).expect("ROM models");
    models.set_async_loads(true);
    let update = |m: &mut ModelScene, ctx: &GpuContext, r: &WorldRenderer, lights| {
        m.update_remotes(ctx, r, &one, lights);
        m.place_view_weapon(ctx, r, held.as_ref(), eye_v, lights);
        m.wait_for_loads();
        m.update_remotes(ctx, r, &one, lights);
        m.place_view_weapon(ctx, r, held.as_ref(), eye_v, lights);
    };
    update(&mut models, &ctx, &renderer, lights.as_ref());
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let before = target
        .render_with_view(
            &ctx,
            &renderer,
            world.scene.as_ref().unwrap(),
            &camera,
            None,
            &models.instances(),
            &models.view_instances(),
            None,
        )
        .expect("frame before the loss");
    let collision_models = world.collision.models.len();
    let floor_before = floor(&world, me.state.pos + fwd * 170.0);

    // the loss, met inside a frame: the frame unwinds to the guard, the code after it never runs
    let mut after_loss = false;
    let lost = gpulost::guard(|| {
        gpulost::simulate_loss();
        after_loss = true;
    });
    assert_eq!(lost, Err(gpulost::GpuReset));
    assert!(!after_loss);
    assert!(gpulost::is_lost());

    // the rebuild, as window::App::rebuild_gpu does it: a new device watched from its first call, the scene of the
    // same world loaded again, the uploads dropped; the lost device's objects are retired, never dropped
    let t = std::time::Instant::now();
    let ctx2 = GpuContext::headless(AdapterChoice::Auto).expect("a new device");
    gpulost::watch_keep_lost(&ctx2.device);
    let renderer2 = WorldRenderer::new(&ctx2, OFFSCREEN_FORMAT, RenderSettings::default());
    let fresh = src
        .reload_scene(GpuTarget {
            ctx: &ctx2,
            renderer: &renderer2,
        })
        .expect("the world's scene on the new device");
    world.adopt_scene(fresh);
    models.gpu_reset();
    gpulost::retire(target);
    gpulost::retire(renderer);
    gpulost::retire(ctx);
    gpulost::recovered();
    println!("rebuild {} ms", t.elapsed().as_millis());
    assert!(!gpulost::is_lost());
    assert_eq!(gpulost::recoveries(), 1);
    assert_eq!(gpulost::leaves(), 0, "no leave-the-match path ran");
    assert!(gpulost::retired() >= 4);

    // the same world state (the collision set was never touched) and the same frame on the new device
    assert_eq!(world.collision.models.len(), collision_models);
    assert_eq!(floor(&world, me.state.pos + fwd * 170.0), floor_before);
    // right after the reset the models are drawn with nothing until their uploads are back
    assert!(models.instances().is_empty());
    update(&mut models, &ctx2, &renderer2, lights.as_ref());
    let target2 = Offscreen::new(&ctx2, OFFSCREEN_FORMAT, w, h);
    let again = target2
        .render_with_view(
            &ctx2,
            &renderer2,
            world.scene.as_ref().unwrap(),
            &camera,
            None,
            &models.instances(),
            &models.view_instances(),
            None,
        )
        .expect("frame after the rebuild");
    assert_eq!(models.stats.characters, 1);
    assert!(
        before.rgba == again.rgba,
        "the frame after the rebuild differs from the frame before the loss"
    );
}
