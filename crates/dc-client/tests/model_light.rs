//! ROM-gated GPU test of how dc-client draws characters and the first-person weapon (Wave AG client-render): on the
//! user's map of the first Rust-client round (JUNK_FLEA) and on SNOWVALLEY, a remote character 170 units ahead of a
//! spawn is drawn with its render styles and lit at its object position with the world's sun, and the first-person
//! M4A1 is drawn with the own character's hand identifier and hand skin. Asserted: the lit character is not a dark
//! flat silhouette (mean luminance and luminance spread over its pixels), it is brighter than the former plain-style
//! draw lit at its feet, every body piece uses its highest LOD at close range, the first-person model is the one of
//! the body row's hand identifier and its hands placeholder resolved to a ROM texture. PNGs (before / after) go to
//! the local render output folder (never committed); nothing of the ROM is asserted by value. Skipped without
//! `CA_ROM` or a GPU adapter.
// SPEC: RENDER_2009.md §6.3 / §6.3.1 (model light set, sun), §7.3 (render styles); EVIDENCE: engine+0x1b21b0 (light
// set at the object position), engine+0x1b1ff0 (LOD by camera distance), cshell+0x23b6a0 (first-person model create
// data), cshell+0x1e3180 (hand skin)

use dc_client::arms::{WeaponClips, WeaponTable};
use dc_client::characters::ModelScene;
use dc_client::offscreen::Offscreen;
use dc_client::world::GpuTarget;
use dc_client::{
    LocalPlayer, ModelRef, RemoteBody, RemoteLook, RemoteTracker, RomWorld, ShootTest, ViewAngles,
    ViewSettings, ViewWeapon, WorldSource,
};
use dc_render::{
    name_key, AdapterChoice, CharacterTable, FrameImage, GpuContext, LightSet, ModelFiles,
    ModelLightSettings, RenderSettings, ViewModels, WorldRenderer, CLIP_LOWER_STAND,
    CLIP_UPPER_STAND, OFFSCREEN_FORMAT,
};
use dc_sim::{drop_to_floor, Cylinder, MovementParams, V3};
use glam::Vec3;

const W: u32 = 480;
const H: u32 = 360;

fn mask(a: &FrameImage, b: &FrameImage) -> Vec<(u32, u32)> {
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

/// Mean and standard deviation of the Rec. 601 luma (0..255) of `img` over `px`.
fn luma_stats(img: &FrameImage, px: &[(u32, u32)]) -> (f32, f32) {
    if px.is_empty() {
        return (0.0, 0.0);
    }
    let l: Vec<f32> = px
        .iter()
        .map(|&(x, y)| {
            let p = img.pixel(x, y);
            0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
        })
        .collect();
    let n = l.len() as f32;
    let mean = l.iter().sum::<f32>() / n;
    let var = l.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n;
    (mean, var.sqrt())
}

struct Shot {
    map: String,
    plain_mean: f32,
    lit: (f32, f32),
    view: (f32, f32),
}

fn one_map(rom: &std::path::Path, ctx: &GpuContext, map: &str, spawn_no: usize) -> Option<Shot> {
    let renderer = WorldRenderer::new(ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = RomWorld::new(rom, map);
    let world = match src.load(Some(GpuTarget {
        ctx,
        renderer: &renderer,
    })) {
        Ok(w) => w,
        Err(e) => {
            println!("{map}: not loaded ({e}), skipped");
            return None;
        }
    };
    let scene = world.scene.as_ref().expect("scene");
    let spawn = world.spawn(spawn_no).expect("spawn");
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
    let view = ShootTest::look_at(eye.to_array(), centre.to_array());
    let camera = ViewSettings::default().camera(eye, &view, W, H);
    let target = Offscreen::new(ctx, OFFSCREEN_FORMAT, W, H);
    let lights = world.model_lights.as_ref();

    let mut models = ModelScene::open(rom).expect("ROM models");
    let body = models.default_look();
    let vfs = dc_rez::RezVfs::new(rom);
    let body_files = ModelFiles::load(&vfs, &body[0].model).expect("body model");
    let anim = body_files.anim_set();
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
    let table = WeaponTable::open(rom).expect("weapon table");
    let row = table.get(145).expect("weapon 145").clone();
    let gun = row.hh_model.clone().map(|m| ModelRef {
        model: m,
        textures: row.hh_skins.clone(),
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
    models.update_remotes(ctx, &renderer, std::slice::from_ref(&remote), lights);
    let inst = models.instances();
    assert!(inst.len() >= 2, "body and face");
    let body_inst = inst[0];
    println!(
        "{map}: light set at the object position: ambient {:?}, {} lights, sun {:?} visibility {:?}; default-style pieces {}",
        body_inst.lights.ambient,
        body_inst.lights.lights.len(),
        lights.and_then(|l| l.sun).map(|s| s.colour),
        lights.map(|l| l.sun_visibility(Vec3::from(centre.to_array()))),
        models.stats.default_style_pieces
    );

    // LOD at close range: every piece at its highest LOD (index 0)
    let lods = body_inst.select_lods(camera.pos);
    let dist = camera.pos.distance(body_inst.position);
    println!("{map}: eye distance {dist:.0}, LODs {lods:?}");
    assert!(dist < 400.0, "close range");
    assert!(
        !lods.is_empty() && lods.iter().all(|&(_, l)| l == 0),
        "LOD 0 at close range: {lods:?}"
    );
    assert!(
        body_inst.lod_triangles.len() > 1,
        "the default body carries LODs"
    );
    assert!(
        body_inst.triangles_at(camera.pos)
            >= body_inst.triangles_at(camera.pos + Vec3::splat(5000.0)),
        "far away draws fewer triangles"
    );

    let world_only = target
        .render_with(ctx, &renderer, scene, &camera, None, &[], None)
        .unwrap();
    let after = target
        .render_with(ctx, &renderer, scene, &camera, None, &inst[..1], None)
        .unwrap();

    // the former draw: plain style (no render styles), lit at the model origin (the box bottom) without the sun and
    // without the light groups
    let images: Vec<Option<dc_render::TextureImage>> = {
        let index = dc_render::TextureIndex::open(rom).unwrap();
        let mut tex = dc_render::RomTextures::new(&index);
        body[0]
            .textures
            .iter()
            .map(|n| tex.image(n.as_bytes()))
            .collect()
    };
    let mut old = renderer.upload_model(ctx, &body_files, &images);
    old.position = body_inst.position;
    old.rotation = body_inst.rotation;
    old.set_pose(&body_inst.trackers);
    if let Some(l) = lights {
        let mut bare = l.grid.clone();
        bare.groups.clear();
        old.lights = LightSet::build(
            old.position,
            old.radius,
            Some(&bare),
            &l.statics,
            &[],
            &ModelLightSettings::default(),
        );
    }
    let before = target
        .render_with(ctx, &renderer, scene, &camera, None, &[&old], None)
        .unwrap();

    // the first-person weapon of the own (default) character
    let pv = row.pv_model.clone().expect("first-person model");
    let clips = WeaponClips::load(&vfs, &pv).expect("clips");
    let held = ViewWeapon {
        weapon: 145,
        model: Some(ModelRef {
            model: pv.clone(),
            textures: row.pv_skins.clone(),
        }),
        clip: clips.idle.as_ref().map(|c| (c.index, 0.0)),
        pos: [0.0; 3],
    };
    models.set_own_look(None);
    let eye_v = Vec3::new(eye.x, eye.y, eye.z);
    models.place_view_weapon(ctx, &renderer, Some(&held), eye_v, lights);
    assert!(models.stats.view_weapon);
    let chosen = models.view_model_path().expect("view model").to_string();
    let chars = CharacterTable::from_vfs(&vfs).unwrap();
    let ident = chars.bodies[&0].pv_ident.clone();
    let expect = ViewModels::from_vfs(&vfs)
        .unwrap()
        .pick(145, ident.as_deref())
        .expect("a first-person entry");
    assert_eq!(
        chosen, expect.model,
        "the body row's hand identifier picks the model"
    );
    let tex = models.view_textures().unwrap().to_vec();
    let index = dc_render::TextureIndex::open(rom).unwrap();
    let mut rt = dc_render::RomTextures::new(&index);
    assert!(
        !tex[0].is_empty() && rt.image(tex[0].as_bytes()).is_some(),
        "the hands entry resolves to a ROM texture"
    );
    println!(
        "{map}: first-person model {} the session's CLI-34 choice, {} skins, hands resolved {}",
        if chosen.eq_ignore_ascii_case(&pv) {
            "same as"
        } else {
            "differs from"
        },
        tex.len(),
        rt.image(tex[0].as_bytes()).is_some()
    );
    let vinst = models.view_instances();
    let inst = models.instances();
    let with_view = target
        .render_with_view(ctx, &renderer, scene, &camera, None, &inst, &vinst, None)
        .unwrap();

    // the weapon's pixels: the full frame against the frame with the whole character and no first-person model
    let all = target
        .render_with(ctx, &renderer, scene, &camera, None, &inst, None)
        .unwrap();
    let out = dc_render::default_out_dir()
        .join("dc-client")
        .join("client-render");
    let _ = std::fs::create_dir_all(&out);
    for (n, img) in [
        ("world", &world_only),
        ("remote_before", &before),
        ("remote_after", &after),
        ("remote_after_full", &all),
        ("view_after", &with_view),
    ] {
        let _ = img.save_png(&out.join(format!("{}_{n}.png", map.to_ascii_lowercase())));
    }
    let px_after = mask(&world_only, &after);
    let px_before = mask(&world_only, &before);
    let plain_mean = luma_stats(&before, &px_before).0;
    let lit = luma_stats(&after, &px_after);
    let vpx = mask(&all, &with_view);
    let vlit = luma_stats(&with_view, &vpx);
    println!(
        "{map}: remote pixels before {} after {}; luma before {:.1}, after mean {:.1} sd {:.1}; view weapon {} px, mean {:.1} sd {:.1}",
        px_before.len(),
        px_after.len(),
        plain_mean,
        lit.0,
        lit.1,
        vpx.len(),
        vlit.0,
        vlit.1
    );
    assert!(px_after.len() > 500, "the character is on screen");
    assert!(vpx.len() > 2000, "the weapon is on screen");
    Some(Shot {
        map: map.to_string(),
        plain_mean,
        lit,
        view: vlit,
    })
}

#[test]
fn rom_characters_and_view_weapon_are_lit_and_use_the_right_model() {
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
    let mut shots = Vec::new();
    for (map, spawn) in [("JUNK_FLEA", 0usize), ("SNOWVALLEY", 0)] {
        shots.extend(one_map(&rom, &ctx, map, spawn));
    }
    assert!(!shots.is_empty(), "at least one ROM map rendered");
    for s in &shots {
        // not a near-black silhouette: a visible mean and real shading / texture detail over the model
        assert!(
            s.lit.0 > 25.0,
            "{}: lit character mean luma {:.1}",
            s.map,
            s.lit.0
        );
        assert!(
            s.lit.1 > 12.0,
            "{}: lit character luma spread {:.1}",
            s.map,
            s.lit.1
        );
        assert!(
            s.lit.0 > s.plain_mean * 2.0,
            "{}: brighter than the former draw ({:.1} vs {:.1})",
            s.map,
            s.lit.0,
            s.plain_mean
        );
        assert!(
            s.view.0 > 25.0 && s.view.1 > 12.0,
            "{}: view weapon {:?}",
            s.map,
            s.view
        );
    }
    println!(
        "PNGs in {} (ROM-derived, never commit)",
        dc_render::default_out_dir()
            .join("dc-client")
            .join("client-render")
            .display()
    );
}
