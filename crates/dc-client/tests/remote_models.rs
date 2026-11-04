//! ROM-gated GPU test: a remote character drawn with its model (the default character's body and face, the third-
//! person model of a ROM weapon at socket 2, the standing pose of the streamed trackers) covers the place where its
//! debug box was drawn. Three offscreen frames from the same camera on the first ROM map: the world alone, the world
//! with the box, the world with the model; the model's pixels must lie inside the box's screen rectangle and there
//! must be enough of them. Skipped without `CA_ROM` or without a GPU adapter; nothing of the ROM is asserted by
//! value, the PNGs go to the local render output folder (never committed).
// SPEC: UDP_SESSION_2009_B.md §3.5 (character object, weapon at socket 2), §4.3 (trackers); RENDER_2009.md §7

use dc_client::characters::ModelScene;
use dc_client::debug_draw::{BoxInstance, DebugBoxes};
use dc_client::offscreen::Offscreen;
use dc_client::world::GpuTarget;
use dc_client::{
    LocalPlayer, ModelRef, RemoteBody, RemoteLook, RemoteTracker, RomWorld, ShootTest, ViewAngles,
    ViewSettings, WeaponTable, WorldSource,
};
use dc_render::{
    name_key, AdapterChoice, FrameImage, GpuContext, ModelFiles, RenderSettings, WorldRenderer,
    CLIP_LOWER_STAND, CLIP_UPPER_STAND, OFFSCREEN_FORMAT,
};
use dc_sim::{drop_to_floor, Cylinder, MovementParams, V3};

fn diff_mask(a: &FrameImage, b: &FrameImage) -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    for y in 0..a.height {
        for x in 0..a.width {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            let d: i32 = (0..3).map(|k| (p[k] as i32 - q[k] as i32).abs()).sum();
            if d > 24 {
                v.push((x, y));
            }
        }
    }
    v
}

fn rect(m: &[(u32, u32)]) -> Option<(u32, u32, u32, u32)> {
    let x0 = m.iter().map(|p| p.0).min()?;
    let x1 = m.iter().map(|p| p.0).max()?;
    let y0 = m.iter().map(|p| p.1).min()?;
    let y1 = m.iter().map(|p| p.1).max()?;
    Some((x0, y0, x1, y1))
}

#[test]
fn rom_remote_character_model_is_drawn_where_its_box_was() {
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
    let map = maps.first().cloned().expect("a ROM map");
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
    // the remote player stands 170 units ahead of the spawn's facing, turned toward the camera
    let fwd = V3::new(spawn.yaw.sin(), 0.0, spawn.yaw.cos());
    let centre = drop_to_floor(
        &world.collision,
        &Cylinder::from_dims(world.stand_dims),
        me.state.pos + fwd * 170.0,
        params.step(),
        1000.0,
    );
    let view = ShootTest::look_at(eye.to_array(), centre.to_array());
    let (w, h) = (320u32, 240u32);
    let camera = ViewSettings::default().camera(eye, &view, w, h);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let mut boxes = DebugBoxes::new(&ctx, OFFSCREEN_FORMAT);

    // the look the server would stream: no model block (the default character), the ROM weapon 145's third-person
    // model at socket 2, the standing trackers found by their name keys
    let mut models = ModelScene::open(&rom).expect("ROM models");
    let body = models.default_look();
    assert!(!body.is_empty(), "the default character resolves");
    let vfs = dc_rez::RezVfs::new(&rom);
    let anim = ModelFiles::load(&vfs, &body[0].model)
        .expect("body model")
        .anim_set();
    let clip = |k| anim.find_clip(|n| name_key(n) == k).expect("player clip");
    let trackers = vec![
        RemoteTracker {
            clip: 0,
            time_ms: 0.0,
            blend_set: None,
        },
        RemoteTracker {
            clip: clip(CLIP_UPPER_STAND),
            time_ms: 0.0,
            blend_set: Some(0),
        },
        RemoteTracker {
            clip: clip(CLIP_LOWER_STAND),
            time_ms: 0.0,
            blend_set: Some(1),
        },
    ];
    let table = WeaponTable::open(&rom).expect("weapon table");
    let gun = table.get(145).and_then(|r| {
        Some(ModelRef {
            model: r.hh_model.clone()?,
            textures: r.hh_skins.clone(),
        })
    });
    let remote = RemoteBody {
        obj: 7,
        pid: 1,
        name: "Remote".into(),
        team: 1,
        pos: centre.to_array(),
        half: world.stand_dims.to_array(),
        yaw: Some(spawn.yaw + std::f32::consts::PI),
        look: Some(RemoteLook {
            styles: Vec::new(),
            models: Vec::new(),
            weapon: gun.map(|g| (g, 2)),
            trackers,
            visible: true,
            aim: None,
        }),
    };
    let lights = world.model_lights.as_ref();
    models.update_remotes(&ctx, &renderer, std::slice::from_ref(&remote), lights);
    let inst = models.instances();
    println!(
        "{}: model stats {:?}, {} instances, failed {:?}",
        world.name,
        models.stats,
        inst.len(),
        models.failed
    );
    assert_eq!(models.stats.characters, 1, "the character has a model");
    assert_eq!(models.stats.defaulted, 1);
    assert!(inst.len() >= 3, "body, face and weapon: {}", inst.len());
    if let Some((lo, hi)) = inst[0].bounds_in_world() {
        println!(
            "body bounds y {:.1}..{:.1} around the object centre y {:.1} (half height {:.1})",
            lo.y, hi.y, centre.y, world.stand_dims.y
        );
    }
    let bx = BoxInstance {
        centre: centre.to_array(),
        half: world.stand_dims.to_array(),
        colour: [0.85, 0.25, 0.2, 1.0],
    };
    let plain = target
        .render_with(&ctx, &renderer, scene, &camera, None, &[], None)
        .unwrap();
    let with_box = target
        .render_with(
            &ctx,
            &renderer,
            scene,
            &camera,
            Some((&mut boxes, &[bx])),
            &[],
            None,
        )
        .unwrap();
    let with_model = target
        .render_with(&ctx, &renderer, scene, &camera, None, &inst, None)
        .unwrap();
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    for (n, img) in [
        ("remote_plain", &plain),
        ("remote_box", &with_box),
        ("remote_model", &with_model),
    ] {
        let _ = img.save_png(&out.join(format!("{n}.png")));
    }
    println!("PNGs in {} (ROM-derived, never commit)", out.display());
    let box_px = diff_mask(&plain, &with_box);
    let model_px = diff_mask(&plain, &with_model);
    let (x0, y0, x1, y1) = rect(&box_px).expect("the box is on screen");
    println!(
        "box pixels {} rect ({x0}, {y0})..({x1}, {y1}); model pixels {} rect {:?}",
        box_px.len(),
        model_px.len(),
        rect(&model_px)
    );
    assert!(model_px.len() > 300, "model pixels {}", model_px.len());
    // inside the box's screen rectangle, grown by 15 % for the weapon and the arms
    let (gx, gy) = (
        ((x1 - x0) as f32 * 0.15) as u32,
        ((y1 - y0) as f32 * 0.15) as u32,
    );
    let inside = model_px
        .iter()
        .filter(|p| p.0 + gx >= x0 && p.0 <= x1 + gx && p.1 + gy >= y0 && p.1 <= y1 + gy)
        .count();
    let share = inside as f32 / model_px.len() as f32;
    assert!(
        share > 0.9,
        "{inside} of {} model pixels inside the box",
        model_px.len()
    );
    let cx = model_px.iter().map(|p| p.0 as f32).sum::<f32>() / model_px.len() as f32;
    let cy = model_px.iter().map(|p| p.1 as f32).sum::<f32>() / model_px.len() as f32;
    assert!(
        (x0 as f32..=x1 as f32).contains(&cx) && (y0 as f32..=y1 as f32).contains(&cy),
        "model centroid ({cx:.0}, {cy:.0}) inside the box rect"
    );
    // the model fills a good part of the box (a character, not a speck)
    assert!(
        model_px.len() as f32 > 0.25 * box_px.len() as f32,
        "model {} vs box {} pixels",
        model_px.len(),
        box_px.len()
    );
    // remote-aim (playtest report): through the frame loop's path (update_remotes_animated, twice as two
    // frames) a new stream yaw turns the drawn body, and an aim note bends its upper body
    // SPEC: UDP_SESSION_2009_B.md §4.3 (yaw8); DISASM: cshell+0x26f60 (aim notes)
    let anims = dc_client::anims::AnimFrame::default();
    let drawn = |r: &RemoteBody, models: &mut ModelScene| {
        models.update_remotes_animated(&ctx, &renderer, std::slice::from_ref(r), &anims, lights);
        models.update_remotes_animated(&ctx, &renderer, std::slice::from_ref(r), &anims, lights);
        target
            .render_with(
                &ctx,
                &renderer,
                scene,
                &camera,
                None,
                &models.instances(),
                None,
            )
            .unwrap()
    };
    let facing = drawn(&remote, &mut models);
    let mut turned = remote.clone();
    turned.yaw = remote.yaw.map(|y| y + 1.3);
    let turned_img = drawn(&turned, &mut models);
    let mut aimed = remote.clone();
    if let Some(l) = aimed.look.as_mut() {
        l.aim = Some([-0.8, 0.0]);
    }
    let aimed_img = drawn(&aimed, &mut models);
    let back = drawn(&remote, &mut models);
    let turn_px = diff_mask(&facing, &turned_img).len();
    let aim_px = diff_mask(&facing, &aimed_img).len();
    let back_px = diff_mask(&facing, &back).len();
    println!("turn changed {turn_px} pixels, aim {aim_px}, back to the first pose {back_px}");
    assert!(
        turn_px > 100,
        "the body turns with the stream yaw: {turn_px} pixels"
    );
    assert!(
        aim_px > 50,
        "the upper body follows the aim: {aim_px} pixels"
    );
    assert!(
        back_px < turn_px / 4,
        "the same input draws the same pose again"
    );
    // the remote leaves: its models go
    models.update_remotes(&ctx, &renderer, &[], lights);
    assert!(models.instances().is_empty());
}
