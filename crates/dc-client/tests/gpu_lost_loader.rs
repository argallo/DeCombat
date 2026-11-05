//! gpu-lost-loader: the model loader threads across an in-match GPU device recovery. The user's session of
//! 2026-11-04 (Quarantine, Junk Flea): a driver reset while character models were loading; the loader threads went
//! on uploading onto the lost device (`Device with 'dc-render device' label is invalid` in `create_buffer`, three
//! panic messages) while the window rebuilt its device. Here: loads are requested on device A, the loss is marked
//! (`gpulost::simulate_loss`, what `DC_FAULT=gpu_lost` runs) and A is destroyed (its uploads now fail as the lost
//! device's did), the scene forgets A's uploads (`ModelScene::gpu_reset`, what the recovery calls), and on device B
//! (a new instance, as the recovery makes) the same character is loaded again and draws what an inline load on B
//! draws. Skipped without `CA_ROM` or a GPU adapter; nothing of the ROM is asserted by value and no image is written.
// ORIGINAL: our own test of the device recovery's model loads (gpu-lost-loader); no 2009 behaviour

use dc_client::characters::ModelScene;
use dc_client::offscreen::Offscreen;
use dc_client::world::GpuTarget;
use dc_client::{
    gpulost, loader, LocalPlayer, ModelRef, RemoteBody, RemoteLook, RemoteTracker, RomWorld,
    ShootTest, ViewAngles, ViewSettings, WeaponTable, WorldSource,
};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use dc_sim::{drop_to_floor, Cylinder, MovementParams, V3};

fn gpu() -> Option<GpuContext> {
    match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => Some(c),
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            None
        }
    }
}

#[test]
fn rom_models_loading_at_a_device_loss_load_again_on_the_new_device() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Some(a) = gpu() else {
        return;
    };
    gpulost::watch(&a.device);
    let map = RomWorld::map_names(&rom).unwrap().first().cloned().unwrap();
    let table = WeaponTable::open(&rom).expect("weapon table");
    let row = table.get(145).expect("weapon 145").clone();
    let gun = row.hh_model.clone().map(|m| ModelRef {
        model: m,
        textures: row.hh_skins.clone(),
    });

    // loads requested on the first device, still loading when it is lost
    let renderer_a = WorldRenderer::new(&a, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = RomWorld::new(&rom, &map);
    let world = src
        .load(Some(GpuTarget {
            ctx: &a,
            renderer: &renderer_a,
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
    let centre = drop_to_floor(
        &world.collision,
        &Cylinder::from_dims(world.stand_dims),
        me.state.pos + fwd * 170.0,
        params.step(),
        1000.0,
    );
    let remote = RemoteBody {
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
    };
    let one = [remote];
    let mut scene = ModelScene::open(&rom).expect("ROM models");
    scene.set_async_loads(true);
    scene.update_remotes(&a, &renderer_a, &one, world.model_lights.as_ref());
    let requested = scene.pending_loads();
    println!(
        "loads in flight at the loss: {requested} on {} threads",
        scene.loader_threads()
    );

    // the loss: marked, and the device made invalid as a lost one is
    assert!(gpulost::guard(gpulost::simulate_loss).is_err());
    assert!(gpulost::is_lost());
    a.device.destroy();
    // the recovery: the scene forgets the old device's uploads and its loader (the queued jobs are cancelled)
    scene.gpu_reset();
    assert_eq!(scene.loader_threads(), 0);
    assert_eq!(scene.pending_loads(), 0);
    let Some(b) = gpu() else {
        return;
    };
    gpulost::watch_keep_lost(&b.device);
    let renderer_b = WorldRenderer::new(&b, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src_b = RomWorld::new(&rom, &map);
    let world_b = src_b
        .load(Some(GpuTarget {
            ctx: &b,
            renderer: &renderer_b,
        }))
        .expect("ROM world on the new device");
    gpulost::device_replaced();
    gpulost::recovered();
    assert!(!gpulost::is_lost());

    // the next updates load the character again, on the new device
    let lights = world_b.model_lights.as_ref();
    scene.update_remotes(&b, &renderer_b, &one, lights);
    assert!(
        scene.loader_threads() >= 1,
        "a new loader on the new device"
    );
    scene.wait_for_loads();
    scene.update_remotes(&b, &renderer_b, &one, lights);
    scene.wait_for_loads();
    scene.update_remotes(&b, &renderer_b, &one, lights);
    assert_eq!(scene.pending_loads(), 0);
    assert!(scene.loading().is_empty(), "nothing is still loading");
    assert_eq!(scene.stats.characters, 1, "the character is drawn again");

    // and it draws what an inline load on the new device draws
    let mut inline = ModelScene::open(&rom).expect("ROM models");
    inline.update_remotes(&b, &renderer_b, &one, lights);
    let (w, h) = (320u32, 240u32);
    let view = ShootTest::look_at(eye.to_array(), centre.to_array());
    let camera = ViewSettings::default().camera(eye, &view, w, h);
    let target = Offscreen::new(&b, OFFSCREEN_FORMAT, w, h);
    let gpu_scene = world_b.scene.as_ref().expect("scene");
    let frame = |m: &ModelScene| {
        target
            .render_with(
                &b,
                &renderer_b,
                gpu_scene,
                &camera,
                None,
                &m.instances(),
                None,
            )
            .expect("frame")
            .rgba
    };
    assert!(
        frame(&scene) == frame(&inline),
        "the reloaded character draws as an inline load on the new device"
    );
    println!(
        "loader jobs skipped {}, panicked {}",
        loader::skipped_jobs(),
        loader::panicked_jobs()
    );
    // the lost device's objects are never dropped (crate::gpulost)
    std::mem::forget((a, renderer_a, world, src, scene));
}
