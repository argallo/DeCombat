//! ROM-gated GPU test of dc-client's lighting against the 2009 side-by-side viewpoints (client-render2): the spawn view
//! of JUNK_FLEA of the sbs3 / sbs4 pairs (with a remote character in the doorway as in sbs4 walkstop_01) and the spawn
//! view of SNOWVALLEY of sbs1, each drawn twice through the client's own path (RomWorld, ModelScene, Offscreen): BEFORE =
//! the former state (lightmap texels read B, G, R; no fog; no gamma ramp) and AFTER = the 2009 rules (texels R, G, B;
//! the world's linear fog; the gamma ramp of the user's profile). The camera uses the 2009 frames' field of view (Hor+
//! from 90 degrees at 4:3, measured on the pairs) so the PNGs line up with the 2009 screenshots; dc-client's own field
//! of view is not changed by this test. Asserted: the after frame differs from the before frame, the gamma lifts the
//! dark world pixels, the far SNOWVALLEY pixels move toward the world's fog colour, the remote and the first-person
//! weapon are on screen. PNGs go to the local render output folder (never committed); nothing of the ROM is asserted
//! by value. Skipped without `CA_ROM` or a GPU adapter.
// SPEC: RENDER_2009.md §2.5 (gamma ramp), §3.3 (linear fog), §6.1 (lightmaps); DC-DEFAULT: REN-20, REN-21, REN-22,
// CLI-210 (proposed, staged rows of client-render2)

use dc_client::arms::{WeaponClips, WeaponTable};
use dc_client::characters::ModelScene;
use dc_client::offscreen::Offscreen;
use dc_client::world::GpuTarget;
use dc_client::{
    ModelRef, RemoteBody, RemoteLook, RemoteTracker, RomWorld, ViewAngles, ViewSettings,
    ViewWeapon, WorldSource,
};
use dc_render::{
    name_key, AdapterChoice, FrameImage, GpuContext, ModelFiles, RenderSettings, SceneOptions,
    TexelOrder, WorldRenderer, CLIP_LOWER_STAND, CLIP_UPPER_STAND, OFFSCREEN_FORMAT,
};
use dc_sim::{drop_to_floor, Cylinder, MovementParams, V3};
use glam::Vec3;

const W: u32 = 1920;
const H: u32 = 1080;
// Horizontal field of view of the 2009 frames at 16:9: the vertical one of 90 degrees at 4:3, widened to the aspect.
// OBSERVED: sbs4-real rest_1 against dc-render frames at 90 / 106.26 degrees (door edges line up at 106.26)

struct View {
    name: &'static str,
    map: &'static str,
    eye: [f32; 3],
    yaw_deg: f32,
    /// A remote character's feet position (dropped to the floor) and yaw, degrees.
    remote: Option<([f32; 3], f32)>,
}

fn luma(p: [u8; 4]) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

fn mean_luma(img: &FrameImage, pick: &dyn Fn(u32, u32, [u8; 4]) -> bool) -> (f32, usize) {
    let (mut s, mut n) = (0.0, 0usize);
    for y in 0..img.height {
        for x in 0..img.width {
            let p = img.pixel(x, y);
            if pick(x, y, p) {
                s += luma(p);
                n += 1;
            }
        }
    }
    (if n > 0 { s / n as f32 } else { 0.0 }, n)
}

fn render(
    rom: &std::path::Path,
    ctx: &GpuContext,
    v: &View,
    after: bool,
) -> Option<(FrameImage, FrameImage, usize)> {
    let renderer = WorldRenderer::new(ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = RomWorld::new(rom, v.map);
    if !after {
        // the former state: B, G, R texels, no fog
        src.scene = SceneOptions {
            lightmap_order: TexelOrder::Bgr,
            fog: false,
            ..SceneOptions::default()
        };
    }
    let world = match src.load(Some(GpuTarget {
        ctx,
        renderer: &renderer,
    })) {
        Ok(w) => w,
        Err(e) => {
            println!("{}: not loaded ({e}), skipped", v.map);
            return None;
        }
    };
    let scene = world.scene.as_ref().expect("scene");
    let eye = V3::new(v.eye[0], v.eye[1], v.eye[2]);
    let view = ViewAngles {
        yaw: v.yaw_deg.to_radians(),
        pitch: 0.0,
    };
    // the 2009 field (client-fov rule: vertical 75 degrees, horizontal from the aspect)
    let settings = ViewSettings::default();
    let camera = settings.camera(eye, &view, W, H);
    let mut target = Offscreen::new(ctx, OFFSCREEN_FORMAT, W, H);
    if after {
        target.set_gamma(dc_client::display::client_gamma(Some(rom)));
    }
    let lights = world.model_lights.as_ref();
    let mut models = ModelScene::open(rom).expect("ROM models");
    let vfs = dc_rez::RezVfs::new(rom);
    let table = WeaponTable::open(rom).expect("weapon table");
    let row = table.get(145).expect("weapon 145").clone();
    let mut remotes = Vec::new();
    if let Some((feet, yaw)) = v.remote {
        let body = models.default_look();
        let body_files = ModelFiles::load(&vfs, &body[0].model).expect("body model");
        let anim = body_files.anim_set();
        let clip = |k| anim.find_clip(|n| name_key(n) == k).expect("player clip");
        let centre = drop_to_floor(
            &world.collision,
            &Cylinder::from_dims(world.stand_dims),
            V3::new(feet[0], feet[1] + world.stand_dims.y + 2.0, feet[2]),
            MovementParams::default().step(),
            1000.0,
        );
        let gun = row.hh_model.clone().map(|m| ModelRef {
            model: m,
            textures: row.hh_skins.clone(),
        });
        remotes.push(RemoteBody {
            obj: 7,
            pid: 1,
            name: "Remote".into(),
            team: 1,
            pos: centre.to_array(),
            half: world.stand_dims.to_array(),
            yaw: Some(yaw.to_radians()),
            look: Some(RemoteLook {
                styles: Vec::new(),
                models: Vec::new(),
                weapon: gun.map(|g| (g, 2)),
                trackers: vec![
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
                ],
                visible: true,
                aim: None,
            }),
        });
    }
    models.update_remotes(ctx, &renderer, &remotes, lights);
    let pv = row.pv_model.clone().expect("first-person model");
    let clips = WeaponClips::load(&vfs, &pv).expect("clips");
    let held = ViewWeapon {
        inspect: None,
        weapon: 145,
        model: Some(ModelRef {
            model: pv.clone(),
            textures: row.pv_skins.clone(),
        }),
        clip: clips.idle.as_ref().map(|c| (c.index, 0.0)),
        pos: [0.0; 3],
    };
    models.set_own_look(None);
    models.place_view_weapon(ctx, &renderer, Some(&held), Vec3::from(v.eye), lights);
    let inst = models.instances();
    let vinst = models.view_instances();
    let full = target
        .render_with_view(ctx, &renderer, scene, &camera, None, &inst, &vinst, None)
        .ok()?;
    let world_only = target
        .render_with_view(ctx, &renderer, scene, &camera, None, &[], &vinst, None)
        .ok()?;
    // the remote character's pixels are counted without the first-person model, which may cover it (client-viewmodel:
    // the 2009 placement draws the gun larger, CLI-330)
    let remotes_bare = target
        .render_with_view(ctx, &renderer, scene, &camera, None, &inst, &[], None)
        .ok()?;
    let world_bare = target
        .render_with_view(ctx, &renderer, scene, &camera, None, &[], &[], None)
        .ok()?;
    let remote_px = (0..H)
        .flat_map(|y| (0..W).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let (a, b) = (remotes_bare.pixel(x, y), world_bare.pixel(x, y));
            (0..3)
                .map(|k| (a[k] as i32 - b[k] as i32).abs())
                .sum::<i32>()
                > 24
        })
        .count();
    Some((full, world_only, remote_px))
}

#[test]
fn rom_world_and_model_lighting_follow_the_2009_rules() {
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
    let views = [
        View {
            name: "junk_flea_spawn",
            map: "JUNK_FLEA",
            eye: [588.0, 83.1, 932.0],
            yaw_deg: 180.0,
            remote: Some(([497.0, 0.0, 633.0], 60.0)),
        },
        View {
            name: "snowvalley_spawn",
            map: "SNOWVALLEY",
            eye: [-188.969, 389.83, -714.013],
            yaw_deg: -53.1,
            remote: None,
        },
    ];
    let out = dc_render::default_out_dir()
        .join("dc-client")
        .join("client-render2");
    let _ = std::fs::create_dir_all(&out);
    println!(
        "gamma of the profile: {:?} (used: {})",
        dc_render::profile_gamma(&rom),
        dc_client::display::client_gamma(Some(&rom))
    );
    let mut done = 0;
    for v in &views {
        let (Some(before), Some(after)) =
            (render(&rom, &ctx, v, false), render(&rom, &ctx, v, true))
        else {
            continue;
        };
        done += 1;
        let _ = before
            .0
            .save_png(&out.join(format!("{}_before.png", v.name)));
        let _ = after.0.save_png(&out.join(format!("{}_after.png", v.name)));
        // dark world pixels (luma below 60 before) get lighter with the gamma ramp
        let dark: Vec<(u32, u32)> = (0..H)
            .step_by(4)
            .flat_map(|y| (0..W).step_by(4).map(move |x| (x, y)))
            .filter(|&(x, y)| luma(before.1.pixel(x, y)) < 60.0 && luma(before.1.pixel(x, y)) > 5.0)
            .collect();
        let lb = dark
            .iter()
            .map(|&(x, y)| luma(before.1.pixel(x, y)))
            .sum::<f32>();
        let la = dark
            .iter()
            .map(|&(x, y)| luma(after.1.pixel(x, y)))
            .sum::<f32>();
        let n = dark.len().max(1) as f32;
        let (mb, _) = mean_luma(&before.0, &|_, _, _| true);
        let (ma, _) = mean_luma(&after.0, &|_, _, _| true);
        println!(
            "{}: frame luma before {mb:.1} after {ma:.1}; dark world pixels {} luma {:.1} -> {:.1}; remote pixels \
             before {} after {}",
            v.name,
            dark.len(),
            lb / n,
            la / n,
            before.2,
            after.2
        );
        assert_ne!(
            before.0.rgba, after.0.rgba,
            "{}: the rules change the frame",
            v.name
        );
        assert!(
            dark.len() > 100 && la > lb,
            "{}: the gamma ramp lifts the darks",
            v.name
        );
        if v.remote.is_some() {
            assert!(
                after.2 > 300,
                "{}: the remote character is on screen",
                v.name
            );
        }
    }
    assert!(done > 0, "at least one ROM map rendered");
    println!("PNGs in {} (ROM-derived, never commit)", out.display());
}
