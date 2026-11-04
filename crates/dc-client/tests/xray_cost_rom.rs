//! ROM- and GPU-gated cost measurement of a declared x-ray server's frame (CLI-1635), offscreen at 2560 x 1440: the
//! remotes' pose, the bridge's line of sight and hook, and the world + x-ray encode, for every style and a few player
//! counts. Runs only with `DC_XRAY_COST=1` (it prints a table and asserts nothing about time); offscreen numbers are
//! not the live game (no present, no network, the default look without gear or weapon). No ROM name is printed.
//!
//! Provenance: ORIGINAL (a DeCombat measurement; no 2009 counterpart).

use std::path::Path;
use std::time::Instant;

use dc_client::anims::AnimFrame;
use dc_client::characters::ModelScene;
use dc_client::hud::state::BoardRow;
use dc_client::hud::{Hud, HudDriver};
use dc_client::modassets::MatchMods;
use dc_client::mods::{HostRecipe, RecipeMod};
use dc_client::settings::{XrayMode, XrayStyle};
use dc_client::world::{GpuTarget, RemoteLook, RemoteTracker};
use dc_client::{RemoteBody, RomWorld, WorldSource};
use dc_mod_lua::{ModOrigin, ModPolicy};
use dc_render::{
    AdapterChoice, Camera, FrameOptions, FrameTarget, GpuContext, RenderSettings, WorldRenderer,
    OFFSCREEN_FORMAT, PLAYER_VIEW_DEFAULT,
};

const W: u32 = 2560;
const H: u32 = 1440;
const WARM: usize = 100;
const FRAMES: usize = 600;

fn p50(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

#[test]
fn rom_xray_frame_cost_table() {
    if std::env::var("DC_XRAY_COST").is_err() {
        println!("skipped: DC_XRAY_COST unset");
        return;
    }
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let ctx = GpuContext::headless(AdapterChoice::Auto).expect("GPU");
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let maps = RomWorld::map_names(&rom).expect("maps");
    let wanted = std::env::var("DC_XRAY_MAP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let map = maps
        .iter()
        .find(|m| !wanted.is_empty() && m.to_ascii_lowercase().contains(&wanted))
        .unwrap_or(&maps[0])
        .clone();
    let mut src = RomWorld::new(&rom, &map);
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("world");
    let scene = world.scene.as_ref().expect("scene");
    let mut models = ModelScene::open(&rom).expect("models");
    let spawns: Vec<[f32; 3]> = world
        .spawns
        .iter()
        .map(|s| [s.pos.x, s.pos.y, s.pos.z])
        .collect();
    assert!(spawns.len() > 1, "the map has spawns");
    let eye = spawns[0];
    let extent = wgpu::Extent3d {
        width: W,
        height: H,
        depth_or_array_layers: 1,
    };
    let tex = |label, format| {
        ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    };
    let colour = tex("cost colour", OFFSCREEN_FORMAT).create_view(&Default::default());
    let depth = tex("cost depth", wgpu::TextureFormat::Depth32Float).create_view(&Default::default());

    let recipe = HostRecipe {
        policy: ModPolicy::Modded {
            client_visual_allowed: false,
        },
        mods: vec![RecipeMod {
            dir: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/hacking_mod"),
            origin: ModOrigin::Server,
            id: "hacking_mod".into(),
            version: "1.1.1".into(),
            kind: "shared".into(),
        }],
        settings: Default::default(),
    };
    let mods = MatchMods::open(Some(&recipe)).expect("mod");

    println!(
        "map index {} spawns {}",
        maps.iter().position(|m| *m == map).unwrap_or(0),
        spawns.len()
    );
    println!("n  style     | pose   hud    encode submit gpu    | cpu    total  draws");
    for n in [0usize, 7, 15] {
        for style in [
            None,
            Some(XrayMode::Box),
            Some(XrayMode::Chams),
            Some(XrayMode::Skeleton),
        ] {
            let mut d = HudDriver::new(Hud::fallback(1024, 768, None));
            if let Some(mode) = style {
                mods.wire(&mut d);
                d.xray_style = XrayStyle {
                    mode,
                    nametags: true,
                    health: true,
                    ..XrayStyle::default()
                };
            }
            d.state.own_pid = Some(200);
            d.state.mode_class = Some(1);
            let row = |pid, team| BoardRow {
                pid,
                name: format!("p{pid}"),
                team,
                kills: 0,
                deaths: 0,
                dead: false,
                ping: None,
            };
            d.state.roster.insert(200, row(200, 1));
            let mut bodies: Vec<RemoteBody> = (0..n)
                .map(|i| {
                    let pid = 10 + i as u8;
                    let team = 1 + (i % 2) as u8;
                    d.state.roster.insert(pid, row(pid, team));
                    let tracker = |blend_set| RemoteTracker {
                        clip: 0,
                        time_ms: 0.0,
                        blend_set,
                    };
                    RemoteBody {
                        obj: 100 + i as u16,
                        pid,
                        name: format!("p{pid}"),
                        team,
                        pos: spawns[1 + i % (spawns.len() - 1)],
                        half: [16.0, 50.0, 16.0],
                        yaw: Some(i as f32),
                        look: Some(RemoteLook {
                            models: Vec::new(),
                            styles: Vec::new(),
                            weapon: None,
                            trackers: vec![tracker(None), tracker(Some(0)), tracker(Some(1))],
                            visible: true,
                            aim: Some([0.1, 0.0]),
                        }),
                    }
                })
                .collect();
            let anims = AnimFrame::default();
            let lights = world.model_lights.as_ref();
            // the eye looks toward the first player
            let aim = bodies
                .first()
                .map_or(0.0, |b| (b.pos[0] - eye[0]).atan2(b.pos[2] - eye[2]));
            let camera = Camera::looking(
                glam::Vec3::from(eye),
                aim,
                0.0,
                1.5,
                H as f32 / W as f32 * 1.5,
            );
            let (mut pose, mut hud, mut enc_t, mut sub, mut gpu, mut cpu, mut tot) =
                (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
            let mut draws = 0u64;
            for f in 0..WARM + FRAMES {
                d.state.now_ms = (f as f32 * 1.4) as u32;
                let t0 = Instant::now();
                models.view_cull = style.map(|_| dc_client::characters::ViewCull::of(&camera));
                for b in &mut bodies {
                    for t in &mut b.look.as_mut().unwrap().trackers {
                        t.time_ms += 1.4;
                    }
                }
                models.update_remotes_animated(&ctx, &renderer, &bodies, &anims, lights);
                let t1 = Instant::now();
                let pts = models.name_points();
                d.set_marks_at(&bodies, &camera, Some(&world.collision), &pts);
                d.set_entity_visibility(&bodies, &camera, Some(&world.collision), Some((5, 1)));
                let t2 = Instant::now();
                let mut enc = ctx.device.create_command_encoder(&Default::default());
                let list = models.instances();
                let xray: Vec<dc_render::XrayModel<'_>> = d
                    .xray_draws
                    .iter()
                    .filter_map(|x| {
                        let instances = models.character_instances(x.entity);
                        (!instances.is_empty()).then_some(dc_render::XrayModel {
                            instances,
                            mode: x.mode,
                            primary: x.primary,
                            secondary: x.secondary,
                        })
                    })
                    .collect();
                let report = renderer.encode_frame_with_view(
                    &ctx,
                    scene,
                    &camera,
                    FrameTarget::new(&colour, &depth, (W, H)),
                    &FrameOptions::default(),
                    &list,
                    &[],
                    &PLAYER_VIEW_DEFAULT,
                    &mut enc,
                );
                let mut calls = report.draw_calls;
                if !xray.is_empty() {
                    calls += renderer.encode_xray(
                        &ctx,
                        &camera,
                        FrameTarget::new(&colour, &depth, (W, H)),
                        &xray,
                        &mut enc,
                    );
                }
                let cmd = enc.finish();
                let t3 = Instant::now();
                ctx.queue.submit([cmd]);
                let t4 = Instant::now();
                let _ = ctx.device.poll(wgpu::PollType::Wait);
                let t5 = Instant::now();
                if f >= WARM {
                    let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1e3;
                    pose.push(ms(t0, t1));
                    hud.push(ms(t1, t2));
                    enc_t.push(ms(t2, t3));
                    sub.push(ms(t3, t4));
                    gpu.push(ms(t4, t5));
                    cpu.push(ms(t0, t4));
                    tot.push(ms(t0, t5));
                    draws = calls;
                }
            }
            println!(
                "{n:2} {:9} | {:.3}  {:.3}  {:.3}  {:.3}  {:.3}  | {:.3}  {:.3}  {draws}",
                style.map_or("off", |m| m.name()),
                p50(&mut pose),
                p50(&mut hud),
                p50(&mut enc_t),
                p50(&mut sub),
                p50(&mut gpu),
                p50(&mut cpu),
                p50(&mut tot),
            );
        }
    }
}

/// CLI-1635: a character outside the view is placed but not posed or drawn, and the first frame back poses it again.
#[test]
fn rom_a_character_outside_the_view_is_not_posed_or_drawn_and_comes_back_posed() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Ok(ctx) = GpuContext::headless(AdapterChoice::Auto) else {
        println!("skipped: no GPU adapter");
        return;
    };
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut models = ModelScene::open(&rom).expect("models");
    let camera = Camera::looking(glam::Vec3::ZERO, 0.0, 0.0, 1.5, 1.2);
    models.view_cull = Some(dc_client::characters::ViewCull::of(&camera));
    let body = |z: f32, time_ms: f32| RemoteBody {
        obj: 100,
        pid: 10,
        name: "p".into(),
        team: 1,
        pos: [0.0, 0.0, z],
        half: [16.0, 50.0, 16.0],
        yaw: Some(0.0),
        look: Some(RemoteLook {
            models: Vec::new(),
            styles: Vec::new(),
            weapon: None,
            trackers: vec![RemoteTracker {
                clip: 0,
                time_ms,
                blend_set: None,
            }],
            visible: true,
            aim: None,
        }),
    };
    let frame = |m: &mut ModelScene, b: RemoteBody| {
        m.update_remotes_animated(&ctx, &renderer, &[b], &AnimFrame::default(), None);
    };
    frame(&mut models, body(500.0, 0.0));
    assert!(!models.character_instances(100).is_empty(), "in view: drawn");
    let drawn = models.instances().len();
    assert!(drawn > 0);
    // behind the camera: still in the scene (the same model), but nothing to draw
    frame(&mut models, body(-2000.0, 300.0));
    assert_eq!(models.drawn(), vec![100], "kept, not rebuilt");
    assert!(models.character_instances(100).is_empty());
    assert_eq!(models.instances().len(), 0);
    assert!(models.name_points().is_empty(), "no stale name point");
    // back in view: posed for this frame's tracker, as a character that was never culled
    frame(&mut models, body(500.0, 300.0));
    assert_eq!(models.instances().len(), drawn);
    let back = models.character_instances(100)[0].bones.clone();
    models.view_cull = None;
    frame(&mut models, body(500.0, 300.0));
    assert_eq!(models.character_instances(100)[0].bones, back);
}

