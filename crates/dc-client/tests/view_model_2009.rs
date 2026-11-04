//! ROM-gated GPU tests of the first-person model (findings 2 and 3 of the 2026-10-26 user retest):
//! 1. the size: the M4A1 (row 145) idle pose drawn by the player-view pass with the CLI-330 placement covers the
//!    screen area the 2009 client's frame shows; the previous (0, -4, 0) placement drew it about 0.7 x as large.
//!    With `DC_VIEWMODEL_REF` set to the 2009 frame (`captures/20261025_185323_sbs3-real/driver/rest_1.png`, never
//!    committed) the dark-pixel silhouettes are compared too.
//! 2. the sprint clips: an arsenal holding row 145 plays the entry clip, the loop and the exit clip around a sprint
//!    on the session clock, and the drawn poses differ from the idle pose and between loop phases.
//!
//! Skipped without `CA_ROM` or a GPU adapter; nothing of the ROM is asserted by value.
// SPEC: RENDER_2009.md §4.2 / §4.3 (player-view pass); DC-DEFAULT: CLI-330 (proposed) placement, CLI-331 (proposed)
// sprint clips; EVIDENCE: cshell+0x238a20, cshell+0x23ac50 (the sprint clip rule, crate::viewanim)

use std::sync::Arc;

use dc_client::arms::{Aim, ArmsConfig, Arsenal, WeaponClips, WeaponCommands, WeaponTable};
use dc_client::characters::ModelScene;
use dc_client::offscreen::Offscreen;
use dc_client::world::GpuTarget;
use dc_client::{ModelRef, RomWorld, ViewAngles, ViewSettings, ViewWeapon, WorldSource};
use dc_render::{
    AdapterChoice, FrameImage, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT,
};
use glam::Vec3;

const M4: u16 = 145;

fn changed(a: &FrameImage, b: &FrameImage) -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    for y in 0..a.height {
        for x in 0..a.width {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            if (0..3)
                .map(|k| (p[k] as i32 - q[k] as i32).abs())
                .sum::<i32>()
                > 30
            {
                v.push((x, y));
            }
        }
    }
    v
}

fn bounds(px: &[(u32, u32)]) -> (u32, u32, u32, u32) {
    px.iter().fold((u32::MAX, u32::MAX, 0, 0), |b, p| {
        (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))
    })
}

struct Rig {
    ctx: GpuContext,
    renderer: WorldRenderer,
}

fn rig() -> Option<(std::path::PathBuf, Rig)> {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return None;
    };
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return None;
        }
    };
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    Some((rom, Rig { ctx, renderer }))
}

/// Places the held weapon until its upload is ready (loads are asynchronous).
fn place(models: &mut ModelScene, r: &Rig, held: &ViewWeapon, eye: Vec3) {
    for _ in 0..500 {
        models.place_view_weapon(&r.ctx, &r.renderer, Some(held), eye, None);
        if models.stats.view_weapon {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the first-person model did not load");
}

fn dark_iou(a: &FrameImage, reference: &image::RgbImage) -> f32 {
    // the gun and the glove are the darkest things in the lower right quarter of both frames
    let (w, h) = (
        a.width.min(reference.width()),
        a.height.min(reference.height()),
    );
    let (mut i, mut u) = (0u32, 0u32);
    for y in h / 2..h {
        for x in w * 2 / 5..w {
            // the 2009 HUD ammo counter (lower right corner) is not part of the model
            if x > w * 85 / 100 && y > h * 93 / 100 {
                continue;
            }
            let p = a.pixel(x, y);
            let q = reference.get_pixel(x, y).0;
            let dp = (p[0] as u32 + p[1] as u32 + p[2] as u32) < 135;
            let dq = (q[0] as u32 + q[1] as u32 + q[2] as u32) < 135;
            i += (dp && dq) as u32;
            u += (dp || dq) as u32;
        }
    }
    i as f32 / u.max(1) as f32
}

#[test]
fn rom_view_model_matches_the_2009_size() {
    let Some((rom, r)) = rig() else {
        return;
    };
    let names = RomWorld::map_names(&rom).unwrap();
    let map = if names.iter().any(|n| n == "JUNK_FLEA") {
        "JUNK_FLEA".to_string()
    } else {
        names[0].clone()
    };
    let mut src = RomWorld::new(&rom, &map);
    let world = src
        .load(Some(GpuTarget {
            ctx: &r.ctx,
            renderer: &r.renderer,
        }))
        .unwrap();
    let scene = world.scene.as_ref().unwrap();
    // OBSERVED: the eye of the 2009 JUNK_FLEA frame (client-fov's view_fov test)
    let eye = dc_sim::V3::new(588.0, 83.1, 932.0);
    let view = ViewAngles {
        yaw: std::f32::consts::PI,
        pitch: 0.0,
    };
    let (w, h) = (1920u32, 1080u32);
    let camera = ViewSettings::default().camera(eye, &view, w, h);
    let target = Offscreen::new(&r.ctx, OFFSCREEN_FORMAT, w, h);
    let table = WeaponTable::open(&rom).unwrap();
    let row = table.get(M4).expect("weapon 145").clone();
    let pv = row.pv_model.clone().expect("first-person model");
    let clips = WeaponClips::load(&dc_rez::RezVfs::new(&rom), &pv).expect("clips");
    let held = ViewWeapon {
        weapon: M4,
        model: Some(ModelRef {
            model: pv,
            textures: row.pv_skins.clone(),
        }),
        clip: clips.idle.as_ref().map(|c| (c.index, 0.0)),
        pos: row.pv_pos.unwrap_or([0.0; 3]),
    };
    let mut models = ModelScene::open(&rom).unwrap();
    let eye_v = Vec3::new(eye.x, eye.y, eye.z);
    place(&mut models, &r, &held, eye_v);
    let plain = target
        .render_with(&r.ctx, &r.renderer, scene, &camera, None, &[], None)
        .unwrap();
    let now = target
        .render_with_view(
            &r.ctx,
            &r.renderer,
            scene,
            &camera,
            None,
            &[],
            &models.view_instances(),
            None,
        )
        .unwrap();
    // the placement before CLI-330: (0, -4, 0) + Pos
    let old_place = Vec3::new(0.0, -4.0, 0.0) + Vec3::from(held.pos);
    let placed = models.view_instance_mut().unwrap().position;
    models.view_instance_mut().unwrap().position = old_place;
    let before = target
        .render_with_view(
            &r.ctx,
            &r.renderer,
            scene,
            &camera,
            None,
            &[],
            &models.view_instances(),
            None,
        )
        .unwrap();
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    let _ = now.save_png(&out.join("view_model_2009_now.png"));
    let _ = before.save_png(&out.join("view_model_2009_before.png"));
    let (a_now, a_before) = (changed(&plain, &now), changed(&plain, &before));
    let (b_now, b_before) = (bounds(&a_now), bounds(&a_before));
    println!(
        "placed at {placed:?}; drawn pixels now {} (box {b_now:?}), before {} (box {b_before:?}), ratio {:.2}",
        a_now.len(),
        a_before.len(),
        a_now.len() as f32 / a_before.len().max(1) as f32
    );
    // arms-fixes (CLI-575): the origin is now `Pos` + the clip's ROM offset, (-0.10, -4.20, -0.05); the measured fit
    // (-0.10, -4.21, -0.04) agrees within its 6 px rms
    assert!(
        (placed - Vec3::new(-0.10, -4.21, -0.04)).length() < 0.02,
        "row 145 lands where the 2009 frame shows it"
    );
    // OBSERVED (2009 frame): the model fills the lower right: front sight near x 0.60 W, y 0.57 H, the receiver
    // reaches the bottom edge and x 0.85 W
    assert!(
        a_now.len() as f32 > 1.4 * a_before.len() as f32,
        "larger than before"
    );
    assert!(b_now.3 >= h - 2, "reaches the bottom edge");
    assert!(
        b_now.0 < w * 55 / 100 && b_now.2 > w * 80 / 100,
        "spans the lower right"
    );
    assert!(
        b_now.1 > h / 2 && b_now.1 < h * 62 / 100,
        "top near 0.57 H: {}",
        b_now.1
    );
    if let Ok(p) = std::env::var("DC_VIEWMODEL_REF") {
        let reference = image::open(&p).expect("reference frame").to_rgb8();
        let (i_now, i_before) = (dark_iou(&now, &reference), dark_iou(&before, &reference));
        println!("silhouette IoU vs the 2009 frame: now {i_now:.3}, before {i_before:.3}");
        assert!(i_now > i_before + 0.05, "closer to the 2009 frame");
    }
    println!("PNGs in {} (ROM-derived, never commit)", out.display());
}

#[test]
fn rom_sprint_clips_play_on_the_first_person_model() {
    let Some((rom, r)) = rig() else {
        return;
    };
    let table = Arc::new(WeaponTable::open(&rom).unwrap());
    let mut a = Arsenal::new(ArmsConfig {
        table: Some(table.clone()),
        rom: Some(rom.clone()),
        ..ArmsConfig::default()
    });
    a.on_life(0, 1);
    a.on_grant(&dc_botclient::weapon::WeaponGrant {
        weapon: M4,
        ammo_kind: 0,
        ammo: 120.0,
        clip: Some(30),
        class_pos: 0,
        slot: 0,
        equip: true,
        parts: [-1; 4],
    });
    let aim = |sprint: bool| Aim {
        origin: [0.0, 33.0, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        control: if sprint {
            0x200 | dc_sim::player::bits::SPRINT
        } else {
            0x200
        },
    };
    let idle_cmd = WeaponCommands::default();
    let clips = a.clips_of(M4).cloned().expect("first-person clips");
    let (enter, run, exit) = (
        clips.sprint_enter.clone().expect("entry clip"),
        clips.sprint_loop.clone().expect("loop clip"),
        clips.sprint_exit.clone().expect("exit clip"),
    );
    let idle = clips.idle.clone().expect("idle clip");
    // the select clip runs first; step every 16 ms of the session clock until the weapon is idle
    let mut t = 0u32;
    while t < 3000 {
        a.step(t, &idle_cmd, &aim(false), &[], None);
        t += 16;
    }
    assert_eq!(a.view_clip(t).map(|v| v.1), Some(idle.index), "idle before");
    // sprint from 3000 ms to 4500 ms
    let start = t;
    let mut seen: Vec<(u32, usize, f32)> = Vec::new();
    while t < start + 2500 {
        let sprint = t < start + 1500;
        a.step(t, &idle_cmd, &aim(sprint), &[], None);
        let (_, i, ct) = a.view_clip(t).unwrap();
        seen.push((t - start, i, ct));
        t += 16;
    }
    let at = |ms: u32| *seen.iter().find(|s| s.0 >= ms).unwrap();
    assert_eq!(
        at(0).1,
        enter.index,
        "the entry clip starts with the sprint"
    );
    assert_eq!(at(enter.len_ms + 40).1, run.index, "then the loop");
    assert_eq!(at(1400).1, run.index, "the loop lasts while sprinting");
    assert_eq!(at(1510).1, exit.index, "the exit clip when the sprint ends");
    assert_eq!(
        at(1500 + exit.len_ms + 40).1,
        idle.index,
        "idle when the exit clip is done"
    );
    // draw the idle pose, the entry clip, two loop phases and the exit clip
    let pv = a.clip_model(M4).expect("model");
    let row = table.get(M4).unwrap().clone();
    let (w, h) = (640u32, 360u32);
    let names = RomWorld::map_names(&rom).unwrap();
    let mut src = RomWorld::new(&rom, &names[0]);
    let world = src
        .load(Some(GpuTarget {
            ctx: &r.ctx,
            renderer: &r.renderer,
        }))
        .unwrap();
    let scene = world.scene.as_ref().unwrap();
    let spawn = world.spawn(0).unwrap();
    let eye = dc_sim::V3::new(spawn.pos.x, spawn.pos.y + 33.0, spawn.pos.z);
    let view = ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    };
    let camera = ViewSettings::default().camera(eye, &view, w, h);
    let target = Offscreen::new(&r.ctx, OFFSCREEN_FORMAT, w, h);
    let mut models = ModelScene::open(&rom).unwrap();
    let eye_v = Vec3::new(eye.x, eye.y, eye.z);
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    let mut frames = Vec::new();
    for (name, clip, time) in [
        ("idle", idle.index, 0.0),
        ("enter", enter.index, enter.len_ms as f32 * 0.5),
        ("loop_a", run.index, 0.0),
        ("loop_b", run.index, run.len_ms as f32 * 0.5),
        ("exit", exit.index, exit.len_ms as f32 * 0.5),
    ] {
        let held = ViewWeapon {
            weapon: M4,
            model: Some(ModelRef {
                model: pv.clone(),
                textures: row.pv_skins.clone(),
            }),
            clip: Some((clip, time)),
            pos: row.pv_pos.unwrap_or([0.0; 3]),
        };
        place(&mut models, &r, &held, eye_v);
        let img = target
            .render_with_view(
                &r.ctx,
                &r.renderer,
                scene,
                &camera,
                None,
                &[],
                &models.view_instances(),
                None,
            )
            .unwrap();
        let _ = img.save_png(&out.join(format!("view_sprint_{name}.png")));
        frames.push((name, img));
    }
    let diff = |a: usize, b: usize| changed(&frames[a].1, &frames[b].1).len();
    for (i, j) in [(0, 1), (0, 2), (0, 3), (2, 3), (0, 4)] {
        let d = diff(i, j);
        println!("{} vs {}: {d} pixels differ", frames[i].0, frames[j].0);
        assert!(d > 500, "{} and {} differ", frames[i].0, frames[j].0);
    }
    println!("PNGs in {} (ROM-derived, never commit)", out.display());
}
