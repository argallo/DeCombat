//! ROM-gated GPU test of the weapon effects and the first-person models of the knife and the grenade: the knife and
//! the grenade are drawn in front of the eye (their clip sets carry the rig off its bind position, CLI-191), an own
//! shot of the rifle draws its first-person muzzle group at the model's muzzle socket, a shot into a wall draws the
//! surface's impact group and the bullet-hole mark, and the blast record of a grenade draws the surface's grenade
//! group and asks for the explosion sound. Skipped without `CA_ROM` or a GPU adapter; nothing of the ROM is asserted
//! by value. The PNGs go to `reports/client-weaponfx/` (ROM-derived, never committed).
// SPEC: RENDER_2009.md §4.2 / §4.3, §8.1 / §8.2; FMT_SPR_LTC_FX.md §5; GAME_MESSAGES_2009_S2C.md §8.2

use dc_client::arms::{RayHit, ShotFx, WeaponClips, WeaponTable};
use dc_client::characters::ModelScene;
use dc_client::fx::{FxEvent, FxStage, ShotRecord, NO_OBJECT};
use dc_client::offscreen::Offscreen;
use dc_client::world::{FrameState, GpuTarget};
use dc_client::{
    LocalPlayer, ModelRef, RomWorld, ViewAngles, ViewSettings, ViewWeapon, WorldSource,
};
use dc_render::{
    AdapterChoice, FrameImage, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT,
};
use dc_sim::MovementParams;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let d = dc_render::default_out_dir().join("client-weaponfx");
    let _ = std::fs::create_dir_all(&d);
    d
}

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

fn brighter(a: &FrameImage, b: &FrameImage) -> usize {
    let mut n = 0;
    for y in 0..a.height {
        for x in 0..a.width {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            let s = |c: [u8; 4]| c[0] as i32 + c[1] as i32 + c[2] as i32;
            n += usize::from(s(q) > s(p) + 60);
        }
    }
    n
}

#[test]
fn rom_weapon_effects_and_view_models() {
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
    let (w, h) = (640u32, 480u32);
    let camera = ViewSettings::default().camera(eye, &view, w, h);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let table = WeaponTable::open(&rom).unwrap();
    let vfs = dc_rez::RezVfs::new(&rom);
    let mut models = ModelScene::open(&rom).unwrap();
    let eye_v = glam::Vec3::new(eye.x, eye.y, eye.z);
    let lights = world.model_lights.as_ref();
    let out = out_dir();
    let plain = target
        .render_with(&ctx, &renderer, scene, &camera, None, &[], None)
        .unwrap();
    let held = |weapon: u16, which: fn(&WeaponClips) -> Option<&dc_client::arms::ClipFact>| {
        let row = table.get(weapon).expect("weapon row").clone();
        // the first first-person model of the row that the ROM holds (CLI-199)
        let (pv, clips) = row
            .pv_model
            .iter()
            .chain(row.pv_models.iter())
            .find_map(|p| WeaponClips::load(&vfs, p).ok().map(|c| (p.clone(), c)))
            .expect("clips");
        ViewWeapon {
            inspect: None,
            weapon,
            model: Some(ModelRef {
                model: pv,
                textures: row.pv_skins.clone(),
            }),
            clip: which(&clips).map(|c| (c.index, 0.0)),
            pos: row.pv_pos.unwrap_or([0.0; 3]),
        }
    };
    // the knife (weapon 0) and the grenade (weapon 7) in hand: drawn, in the lower half of the view
    for (weapon, name) in [
        (0u16, "knife_in_hand"),
        (83, "sickle_in_hand"),
        (7, "grenade_in_hand"),
        (145, "rifle_in_hand"),
    ] {
        models.place_view_weapon(
            &ctx,
            &renderer,
            Some(&held(weapon, |c| c.idle.as_ref())),
            eye_v,
            lights,
        );
        assert!(models.stats.view_weapon, "{name}: drawn");
        let inst = models.view_instances();
        let img = target
            .render_with_view(&ctx, &renderer, scene, &camera, None, &[], &inst, None)
            .unwrap();
        img.save_png(&out.join(format!("{name}.png"))).unwrap();
        let px = changed(&plain, &img);
        let lower = px.iter().filter(|p| p.1 >= h / 2).count();
        println!(
            "{name}: {} pixels changed, {lower} in the lower half",
            px.len()
        );
        assert!(
            px.len() > 400,
            "{name}: the model covers part of the view ({})",
            px.len()
        );
        assert!(lower * 2 > px.len(), "{name}: mostly in the lower half");
    }
    // an own rifle shot: the muzzle group at the first-person model's muzzle socket
    let mut stage = FxStage::open(Some(&rom));
    assert!(!stage.runner.library.is_empty(), "effect groups read");
    models.place_view_weapon(
        &ctx,
        &renderer,
        Some(&held(145, |c| c.fire.as_ref())),
        eye_v,
        lights,
    );
    let sock = stage.view_socket(&models);
    assert!(
        sock.is_some(),
        "the rifle's first-person model has its muzzle socket"
    );
    let dir = dc_client::arms::forward(view.yaw, view.pitch);
    let origin = [eye.x, eye.y, eye.z];
    let shot = ShotFx {
        weapon: 145,
        ammo: table.get(145).and_then(|r| r.ammo).unwrap_or(145),
        origin,
        muzzle: origin,
        dir,
        range: 3000.0,
        hit: RayHit::Miss,
        first: true,
        time_ms: 1000,
        impact: None,
        flash_mod: None,
    };
    let fs = FrameState {
        fx_events: vec![FxEvent::Shot(shot)],
        ..FrameState::default()
    };
    let cues = stage.frame(1000, &fs, &world);
    println!(
        "shot: stats {:?}, {} sound cues",
        stage.runner.stats,
        cues.len()
    );
    assert_eq!(
        stage.runner.stats.muzzle, 1,
        "the first-person muzzle group started"
    );
    let inst = models.view_instances();
    let no_fx = target
        .render_with_view(&ctx, &renderer, scene, &camera, None, &[], &inst, None)
        .unwrap();
    let img = target
        .render_with_fx(
            &ctx,
            &renderer,
            scene,
            &camera,
            None,
            &[],
            &inst,
            Some((&mut stage, 1010, sock)),
            None,
            None,
        )
        .unwrap();
    img.save_png(&out.join("muzzle_flash.png")).unwrap();
    let flash = brighter(&no_fx, &img);
    println!(
        "muzzle flash: {} view quads, {} world quads, {flash} pixels brighter",
        stage.last.view.len(),
        stage.last.world.len()
    );
    assert!(
        !stage.last.view.is_empty(),
        "eye-space quads of the muzzle group"
    );
    assert!(flash > 20, "the flash brightens the frame ({flash})");
    // 200 ms later the group is over
    let later = stage.runner.draw(1300, &camera, sock);
    assert!(
        later.view.is_empty(),
        "the muzzle group ends with its length"
    );
    // the impact on the wall the eye looks at (the local trace finds polygon, normal and surface)
    let fresh = |t: u32, ev: Vec<FxEvent>, stage: &mut FxStage| {
        let fs = FrameState {
            fx_events: ev,
            ..FrameState::default()
        };
        stage.frame(t, &fs, &world)
    };
    let before = stage.runner.stats;
    let cues = fresh(
        2000,
        vec![FxEvent::Shot(ShotFx {
            time_ms: 2000,
            impact: None,
            ..shot
        })],
        &mut stage,
    );
    let st = stage.runner.stats;
    println!(
        "impact: {:?} -> {:?}, {} sound cues",
        before,
        st,
        cues.len()
    );
    let img = target
        .render_with_fx(
            &ctx,
            &renderer,
            scene,
            &camera,
            None,
            &[],
            &[],
            Some((&mut stage, 2060, None)),
            None,
            None,
        )
        .unwrap();
    img.save_png(&out.join("impact.png")).unwrap();
    println!("impact frame: {} world quads", stage.last.world.len());
    assert!(st.impacts > before.impacts, "the wall shot made an impact");
    // the blast of a grenade 300 units ahead on the floor below the eye: the surface's grenade group and the sound
    let ahead = glam::Vec3::from(origin) + glam::Vec3::from(dir) * 300.0;
    let floor = world
        .collision
        .ray(
            dc_sim::V3::new(ahead.x, ahead.y, ahead.z),
            dc_sim::V3::new(ahead.x, ahead.y - 500.0, ahead.z),
        )
        .map(|hh| {
            (
                ahead.y - 500.0 * hh.t,
                world.surface_type(hh.model, hh.poly),
            )
        })
        .unwrap_or((ahead.y - 60.0, 0));
    let grenade_ammo = table
        .get(7)
        .and_then(|r| r.ammo)
        .expect("grenade ammunition");
    let blast = ShotRecord {
        shooter: 5,
        first: NO_OBJECT,
        weapon: 7,
        ammo: grenade_ammo,
        from: origin,
        to: [ahead.x, floor.0, ahead.z],
        normal: [0.0, 1.0, 0.0],
        surface: floor.1.min(255) as u8,
        clear: 0x59f0,
        own: false,
        mods: [0xff; 3],
    };
    let mut stage = FxStage::open(Some(&rom));
    let cues = fresh(3000, vec![FxEvent::Record(blast)], &mut stage);
    println!("blast: {:?}, cues {:?}", stage.runner.stats, cues.len());
    assert!(stage.runner.stats.blasts >= 1);
    assert!(!cues.is_empty(), "the explosion sound");
    let img = target
        .render_with_fx(
            &ctx,
            &renderer,
            scene,
            &camera,
            None,
            &[],
            &[],
            Some((&mut stage, 3150, None)),
            None,
            None,
        )
        .unwrap();
    img.save_png(&out.join("explosion.png")).unwrap();
    for t in [3050u32, 3400, 3700] {
        let fs = FrameState::default();
        let _ = stage.frame(t, &fs, &world);
        let im = target
            .render_with_fx(
                &ctx,
                &renderer,
                scene,
                &camera,
                None,
                &[],
                &[],
                Some((&mut stage, t, None)),
                None,
                None,
            )
            .unwrap();
        im.save_png(&out.join(format!("explosion_{t}.png")))
            .unwrap();
    }
    let n = brighter(&plain, &img);
    println!(
        "explosion frame: {} world quads, {n} pixels brighter",
        stage.last.world.len()
    );

    assert!(!stage.last.world.is_empty(), "the explosion's quads");
    println!("PNGs in {} (ROM-derived, never commit)", out.display());
}
