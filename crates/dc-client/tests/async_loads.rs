//! ROM-gated GPU test of the model loader threads: with `set_async_loads(true)` a remote character
//! and the own first-person weapon are drawn with nothing while their models load, then, after `wait_for_loads`,
//! exactly as the inline mode draws them (byte-identical offscreen frames); a second character with the same look is
//! drawn in the same update it appears, without a new request. Skipped without `CA_ROM` or a GPU adapter; nothing of
//! the ROM is asserted by value and no image is written.
// DC-DEFAULT: CLI-260, CLI-262 (proposed), see DC_DEFAULTS.md

use std::time::Instant;

use dc_client::characters::ModelScene;
use dc_client::offscreen::Offscreen;
use dc_client::world::GpuTarget;
use dc_client::{
    LocalPlayer, ModelRef, RemoteBody, RemoteLook, RemoteTracker, RomWorld, ShootTest, ViewAngles,
    ViewSettings, ViewWeapon, WeaponTable, WorldSource,
};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use dc_sim::{drop_to_floor, Cylinder, MovementParams, V3};

#[test]
fn rom_async_loads_draw_nothing_then_what_inline_loads_draw() {
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
    let map = RomWorld::map_names(&rom).unwrap().first().cloned().unwrap();
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
    let centre = floor(me.state.pos + fwd * 170.0);
    let centre2 = floor(me.state.pos + fwd * 170.0 + side * 40.0);
    let view = ShootTest::look_at(eye.to_array(), centre.to_array());
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
        weapon: 145,
        model: Some(ModelRef {
            model: m,
            textures: row.pv_skins.clone(),
        }),
        clip: None,
        pos: [0.0; 3],
    });
    let remote = |obj: u16, pid: u8, at: V3| RemoteBody {
        obj,
        pid,
        name: format!("Remote{obj}"),
        team: 1,
        pos: at.to_array(),
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
    let one = [remote(7, 1, centre)];
    let two = [remote(7, 1, centre), remote(9, 2, centre2)];
    let eye_v = glam::Vec3::new(eye.x, eye.y, eye.z);
    let lights = world.model_lights.as_ref();
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

    // the inline mode (the default): drawn in the first update
    let mut inline = ModelScene::open(&rom).expect("ROM models");
    assert!(!inline.async_loads());
    inline.update_remotes(&ctx, &renderer, &one, lights);
    inline.place_view_weapon(&ctx, &renderer, held.as_ref(), eye_v, lights);
    assert_eq!(inline.stats.characters, 1);
    assert!(inline.loading().is_empty());
    let want = frame(&inline);
    let plain = target
        .render_with(&ctx, &renderer, scene, &camera, None, &[], None)
        .unwrap();
    assert!(want.rgba != plain.rgba, "the character is on screen");

    // the async mode: nothing drawn while loading, the object still counts as drawn (no box)
    let mut lazy = ModelScene::open(&rom).expect("ROM models");
    lazy.set_async_loads(true);
    let t = Instant::now();
    lazy.update_remotes(&ctx, &renderer, &one, lights);
    lazy.place_view_weapon(&ctx, &renderer, held.as_ref(), eye_v, lights);
    let request_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "async: first update {request_ms:.2} ms on the frame thread, {} loads pending on {} loader threads",
        lazy.pending_loads(),
        lazy.loader_threads()
    );
    assert!(lazy.loader_threads() >= 1);
    if lazy.pending_loads() > 0 {
        assert_eq!(lazy.stats.characters, 0, "nothing drawn while loading");
        assert_eq!(lazy.loading(), &[7]);
        assert_eq!(lazy.drawn(), vec![7], "a loading character gets no box");
        assert!(lazy.instances().is_empty());
    }
    lazy.wait_for_loads();
    assert_eq!(lazy.pending_loads(), 0);
    lazy.update_remotes(&ctx, &renderer, &one, lights);
    lazy.place_view_weapon(&ctx, &renderer, held.as_ref(), eye_v, lights);
    lazy.wait_for_loads();
    lazy.place_view_weapon(&ctx, &renderer, held.as_ref(), eye_v, lights);
    assert_eq!(lazy.stats.characters, 1);
    assert!(lazy.loading().is_empty());
    assert_eq!(lazy.stats.view_weapon, inline.stats.view_weapon);
    assert_eq!(lazy.stats.failures, inline.stats.failures);
    assert_eq!(
        lazy.stats.default_style_pieces,
        inline.stats.default_style_pieces
    );
    let got = frame(&lazy);
    assert!(
        got.rgba == want.rgba,
        "async-loaded models draw what inline-loaded ones draw"
    );

    // a second character with the same look: drawn in the update it appears, no new request
    let t = Instant::now();
    lazy.update_remotes(&ctx, &renderer, &two, lights);
    let second_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("async: second character with the same look {second_ms:.2} ms on the frame thread");
    assert_eq!(
        lazy.pending_loads(),
        0,
        "the same look is not requested again"
    );
    assert_eq!(lazy.stats.characters, 2);
    inline.update_remotes(&ctx, &renderer, &two, lights);
    lazy.place_view_weapon(&ctx, &renderer, held.as_ref(), eye_v, lights);
    inline.place_view_weapon(&ctx, &renderer, held.as_ref(), eye_v, lights);
    assert!(
        frame(&lazy).rgba == frame(&inline).rgba,
        "two characters of one look draw alike in both modes"
    );
    // the second leaves, then the first: their instances go
    lazy.update_remotes(&ctx, &renderer, &one, lights);
    assert_eq!(lazy.instances().len(), inline_count(&one));
    lazy.update_remotes(&ctx, &renderer, &[], lights);
    assert!(lazy.instances().is_empty());
}

/// Body, face and weapon of each character.
fn inline_count(r: &[RemoteBody]) -> usize {
    r.len() * 3
}
