//! ROM-gated GPU test: the first-person model of the held weapon (the weapon row's first-person model with its skins,
//! the idle / fire / reload clips of its weapon state) is drawn in front of the eye, in the lower part of the view,
//! and its pose follows the clip. Skipped without `CA_ROM` or a GPU adapter; nothing of the ROM is asserted by value.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1, §4.3 (the weapon state follows the first-person model's clips); RENDER_2009.md
// §4.2 / §4.3 (player-view pass), §7; DC-DEFAULT: CLI-28 (revised, proposed) placement

use dc_client::arms::{WeaponClips, WeaponTable};
use dc_client::characters::ModelScene;
use dc_client::offscreen::Offscreen;
use dc_client::world::GpuTarget;
use dc_client::{
    LocalPlayer, ModelRef, RomWorld, ViewAngles, ViewSettings, ViewWeapon, WorldSource,
};
use dc_render::{
    AdapterChoice, FrameImage, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT,
};
use dc_sim::MovementParams;

fn changed(a: &FrameImage, b: &FrameImage) -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    for y in 0..a.height {
        for x in 0..a.width {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            if (0..3)
                .map(|k| (p[k] as i32 - q[k] as i32).abs())
                .sum::<i32>()
                > 24
            {
                v.push((x, y));
            }
        }
    }
    v
}

#[test]
fn rom_first_person_weapon_is_drawn_in_front_of_the_eye() {
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
        .unwrap();
    let scene = world.scene.as_ref().unwrap();
    let spawn = world.spawn(0).unwrap();
    let me = LocalPlayer::spawn(
        MovementParams::default(),
        &world.collision,
        &spawn,
        world.stand_dims,
        60,
    );
    let view = ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    };
    let eye = me.eye_target(&view);
    let (w, h) = (320u32, 240u32);
    let camera = ViewSettings::default().camera(eye, &view, w, h);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let table = WeaponTable::open(&rom).unwrap();
    let row = table.get(145).expect("weapon 145").clone();
    let pv = row.pv_model.clone().expect("first-person model");
    let clips = WeaponClips::load(&dc_rez::RezVfs::new(&rom), &pv).expect("clips");
    let mut models = ModelScene::open(&rom).unwrap();
    let held = |clip: Option<(usize, f32)>| ViewWeapon {
        inspect: None,
        weapon: 145,
        model: Some(ModelRef {
            model: pv.clone(),
            textures: row.pv_skins.clone(),
        }),
        clip,
        pos: [0.0; 3],
    };
    let eye_v = glam::Vec3::new(eye.x, eye.y, eye.z);
    let lights = world.model_lights.as_ref();
    let plain = target
        .render_with(&ctx, &renderer, scene, &camera, None, &[], None)
        .unwrap();
    let mut shots = Vec::new();
    for (name, clip) in [
        ("idle", clips.idle.as_ref().map(|c| (c.index, 0.0))),
        (
            "reload",
            clips
                .reload
                .as_ref()
                .map(|c| (c.index, c.len_ms as f32 * 0.4)),
        ),
    ] {
        models.place_view_weapon(&ctx, &renderer, Some(&held(clip)), eye_v, lights);
        assert!(models.stats.view_weapon, "the first-person model is drawn");
        assert!(models.instances().is_empty(), "not in the world pass");
        let inst = models.view_instances();
        assert_eq!(inst.len(), 1);
        let img = target
            .render_with_view(&ctx, &renderer, scene, &camera, None, &[], &inst, None)
            .unwrap();
        let out = dc_render::default_out_dir().join("dc-client");
        let _ = std::fs::create_dir_all(&out);
        let _ = img.save_png(&out.join(format!("view_weapon_{name}.png")));
        let px = changed(&plain, &img);
        let lower = px.iter().filter(|p| p.1 >= h / 2).count();
        println!(
            "{name}: {} pixels changed, {lower} in the lower half; eye-space bounds {:?}",
            px.len(),
            inst[0].posed_bounds()
        );
        shots.push(px);
    }
    let (idle, reload) = (&shots[0], &shots[1]);
    assert!(
        idle.len() > 500,
        "the weapon covers part of the view: {}",
        idle.len()
    );
    assert!(
        idle.iter().filter(|p| p.1 >= h / 2).count() * 2 > idle.len(),
        "mostly in the lower half"
    );
    assert_ne!(idle, reload, "the reload clip moves the model");
    // no held weapon: nothing drawn
    models.place_view_weapon(&ctx, &renderer, None, eye_v, lights);
    assert!(!models.stats.view_weapon && models.view_instances().is_empty());
    println!(
        "PNGs in {} (ROM-derived, never commit)",
        dc_render::default_out_dir().join("dc-client").display()
    );
}
