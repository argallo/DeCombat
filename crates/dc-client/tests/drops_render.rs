//! ROM-gated GPU test: a dropped weapon drawn with its model. A drop of a ROM weapon (the server's form: the row's
//! third-person model and skins, clip 1 at time 0, created 16 units above the floor in front of the first spawn of
//! the first ROM map) is moved by the client's drop motion (the hop and the tumbling fall of the ROM's motion row)
//! and drawn: once in flight and once at rest, where it must lie flat above the floor. The offscreen frames go to
//! the local render output folder (ROM-derived, never committed); nothing of the ROM is asserted by value. Skipped
//! without `CA_ROM` or without a GPU adapter.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (type 0x17 kind 6); UDP_SESSION_2009_B.md §3.4 (model block), §4.4
// DISASM: cshell+0x282670 (the pickup object's fall and rest)

use dc_client::characters::ModelScene;
use dc_client::drops::{DropMotionRow, WeaponDrop};
use dc_client::offscreen::Offscreen;
use dc_client::world::GpuTarget;
use dc_client::{
    LocalPlayer, ModelRef, RemoteTracker, RomWorld, ShootTest, ViewAngles, ViewSettings,
    WeaponTable, WorldSource,
};
use dc_render::{
    AdapterChoice, FrameImage, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT,
};
use dc_sim::{drop_to_floor, Collider, Cylinder, MovementParams, V3};

fn diff_count(a: &FrameImage, b: &FrameImage) -> usize {
    let mut n = 0;
    for y in 0..a.height {
        for x in 0..a.width {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            let d: i32 = (0..3).map(|k| (p[k] as i32 - q[k] as i32).abs()).sum();
            n += (d > 24) as usize;
        }
    }
    n
}

#[test]
fn rom_a_dropped_weapon_is_drawn_falling_and_lying_flat() {
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
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let maps = RomWorld::map_names(&rom).expect("ROM world archive");
    let map = std::env::var("DC_DROPS_MAP")
        .ok()
        .filter(|m| maps.contains(m))
        .or_else(|| maps.first().cloned())
        .expect("a ROM map");
    let mut src = RomWorld::new(&rom, &map);
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("ROM world");
    let scene = world.scene.as_ref().expect("scene");
    let spawn = world.spawn(0).expect("spawn 0");
    let params = MovementParams::default();
    let me = LocalPlayer::spawn(params, &world.collision, &spawn, world.stand_dims, 60);
    let eye = me.eye_target(&ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    });
    // the victim stood 110 units ahead; the server puts the drop 16 above the floor below him (DSV-93)
    let fwd = V3::new(spawn.yaw.sin(), 0.0, spawn.yaw.cos());
    let centre = drop_to_floor(
        &world.collision,
        &Cylinder::from_dims(world.stand_dims),
        me.state.pos + fwd * 110.0,
        params.step(),
        1000.0,
    );
    let floor_y = centre.y - world.stand_dims.y;
    let at = [centre.x, floor_y + 16.0, centre.z];

    let table = WeaponTable::open(&rom).expect("weapon table");
    let weapon = 145u16;
    let row = table.get(weapon).expect("weapon row");
    let model = ModelRef {
        model: row.hh_model.clone().expect("third-person model"),
        textures: row.hh_skins.iter().take(1).cloned().collect(),
    };
    let drop = WeaponDrop {
        obj: 300,
        kind: dc_client::drops::PICKUP_KIND_WEAPON,
        weapon,
        rounds: 150,
        spin: false,
        fall: true,
        pos: at,
        rot: [0.0, (spawn.yaw * 0.5).sin(), 0.0, (spawn.yaw * 0.5).cos()],
        model: Some(model),
        trackers: vec![RemoteTracker {
            clip: 1,
            time_ms: 0.0,
            blend_set: None,
        }],
        seen_ms: 0,
    };
    let row_motion = DropMotionRow::from_vfs(&dc_rez::RezVfs::new(&rom));
    println!(
        "{}: drop at {at:?}, floor {floor_y:.1}, motion row {row_motion:?}",
        world.name
    );

    let mut models = ModelScene::open(&rom).expect("ROM models");
    let lights = world.model_lights.as_ref();
    let cw: &dyn Collider = &world.collision;
    let (w, h) = (480u32, 320u32);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    let render_from = |models: &ModelScene, eye: V3, look_at: [f32; 3], name: &str| {
        let view = ShootTest::look_at(eye.to_array(), look_at);
        let camera = ViewSettings::default().camera(eye, &view, w, h);
        let plain = target
            .render_with(&ctx, &renderer, scene, &camera, None, &[], None)
            .unwrap();
        let inst = models.instances();
        let with = target
            .render_with(&ctx, &renderer, scene, &camera, None, &inst, None)
            .unwrap();
        let _ = with.save_png(&out.join(format!("{name}.png")));
        diff_count(&plain, &with)
    };
    let render = |models: &ModelScene, look_at: [f32; 3], name: &str| {
        render_from(models, eye, look_at, name)
    };

    // in flight: a few steps after the create (the hop)
    models.update_drops(
        &ctx,
        &renderer,
        std::slice::from_ref(&drop),
        120,
        Some(cw),
        lights,
    );
    assert_eq!(models.drops_drawn(), 1, "the drop has a model");
    let fly_at = models.instances()[0].position.to_array();
    println!("in flight at {fly_at:?}");
    let flight_px = render(&models, fly_at, "drops_flight");
    assert!(
        flight_px > 50,
        "the weapon in flight is visible: {flight_px} pixels"
    );
    // at rest
    models.update_drops(
        &ctx,
        &renderer,
        std::slice::from_ref(&drop),
        8000,
        Some(cw),
        lights,
    );
    let inst = models.instances();
    let rest = inst.first().expect("the drop instance");
    println!(
        "rest position {:?} rotation {:?}; flight pixels {flight_px}",
        rest.position, rest.rotation
    );
    assert!(
        rest.position.y >= floor_y - 0.01 && rest.position.y <= at[1],
        "at rest between the floor and its start: {:?} (floor {floor_y})",
        rest.position
    );
    assert!(
        rest.rotation.angle_between(glam::Quat::IDENTITY) < 1e-5,
        "flat"
    );
    let rest_at = rest.position.to_array();
    let rest_px = render(&models, rest_at, "drops_rest");
    // a close look from above and behind the victim's place
    let close_eye = V3::from_array(rest_at) - fwd * 40.0 + V3::new(0.0, 30.0, 0.0);
    let close_px = render_from(&models, close_eye, rest_at, "drops_rest_close");
    println!(
        "rest pixels {rest_px}, close {close_px}; PNGs in {} (ROM-derived, never commit)",
        out.display()
    );
    assert!(
        rest_px > 150,
        "the weapon at rest is visible: {rest_px} pixels"
    );
    // the drop goes with its object
    models.update_drops(&ctx, &renderer, &[], 9000, Some(cw), lights);
    assert!(models.instances().is_empty());
}
