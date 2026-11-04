//! ROM-gated GPU test (no server): a kill draws no box where the victim stood. In the death group the victim's
//! character object turns to its hidden form and the corpse object appears (UDP_SESSION_2009_B.md §3.5.5); since
//! CLI-350 the scene drops the hidden character's models, so the character was in neither the drawn list nor the
//! loading list, and the frame's box list (`headless::box_list`, shared by the window and the headless runner) gave
//! it the remote box in its team colour: the "blue rectangular prism in his spot" of the user's round on a579a3f
//! (team 2 = blue). The frame is rendered with inline and with threaded model loads (the window's mode); with
//! `DC_PNG_OUT` set the frames are written there. Skipped without `CA_ROM` or without a GPU adapter; nothing of the
//! ROM is asserted by value.
// SPEC: UDP_SESSION_2009_B.md §3.5.5 (death group: the character with flags 0x0802, then the corpse create)
// DC-DEFAULT: CLI-42 (proposed), CLI-350 (proposed), CLI-351 (proposed)

use dc_client::anims::AnimFrame;
use dc_client::characters::{async_loads_from_env, ModelScene};
use dc_client::debug_draw::DebugBoxes;
use dc_client::headless::box_list;
use dc_client::offscreen::Offscreen;
use dc_client::testkit::synth_source;
use dc_client::world::GpuTarget;
use dc_client::{RemoteBody, RemoteLook, RemoteTracker, ViewAngles, ViewSettings, WorldSource};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use dc_sim::V3;

const VICTIM: u16 = 75;
const CORPSE: u16 = 80;
/// The victim's team: 2, drawn blue by the remote boxes.
const TEAM: u8 = 2;

fn body(obj: u16, team: u8, look: &[dc_client::ModelRef], visible: bool) -> RemoteBody {
    RemoteBody {
        obj,
        pid: if obj == CORPSE { 0xFF } else { 1 },
        name: String::new(),
        team,
        pos: [0.0, 60.0, 300.0],
        half: [16.0, 40.0, 16.0],
        yaw: Some(0.0),
        look: Some(RemoteLook {
            styles: Vec::new(),
            models: look.to_vec(),
            weapon: None,
            trackers: vec![RemoteTracker {
                clip: 0,
                time_ms: 0.0,
                blend_set: None,
            }],
            visible,
            aim: None,
        }),
    }
}

/// Pixels close to the team-2 box colour (clearly more blue than red and green).
fn blue_pixels(img: &dc_render::FrameImage, w: u32, h: u32) -> u32 {
    let mut n = 0;
    for y in 0..h {
        for x in 0..w {
            let p = img.pixel(x, y);
            if p[2] as i32 > p[0] as i32 + 80 && p[2] as i32 > p[1] as i32 + 40 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn rom_a_kill_leaves_no_box_in_the_victims_spot() {
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
    let mut src = synth_source();
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("synthetic world");
    let gscene = world.scene.as_ref().expect("scene");
    let (w, h) = (320u32, 240u32);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let mut boxes = DebugBoxes::new(&ctx, OFFSCREEN_FORMAT);
    let camera =
        ViewSettings::default().camera(V3::new(0.0, 100.0, 0.0), &ViewAngles::default(), w, h);
    let out = std::env::var_os("DC_PNG_OUT").map(std::path::PathBuf::from);
    if let Some(d) = &out {
        std::fs::create_dir_all(d).expect("PNG dir");
    }
    // DC_ASYNC_LOADS=0 / 1 runs that mode only; unset runs both
    let modes: Vec<bool> = match std::env::var("DC_ASYNC_LOADS").ok().as_deref() {
        Some("0") | Some("1") => vec![async_loads_from_env(false)],
        _ => vec![false, true],
    };
    for threaded in modes {
        let mut scene = ModelScene::open(&rom).expect("ROM models");
        scene.set_async_loads(threaded);
        scene.own_team = Some(1);
        let look = scene.default_look();
        assert!(!look.is_empty(), "the default character");
        let frame = AnimFrame::default();
        let alive = [body(VICTIM, TEAM, &look, true)];
        let mut built = false;
        for _ in 0..2000 {
            scene.update_remotes_animated(&ctx, &renderer, &alive, &frame, None);
            if !scene.instances().is_empty() {
                built = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(built, "threaded {threaded}: the victim was built");
        assert!(
            box_list(&alive, &scene.drawn(), false).is_empty(),
            "threaded {threaded}: the living victim is drawn with its model, no box"
        );

        // the death group: the character hidden, its corpse created (the victim's body); the corpses reach the
        // scene through the animation frame, the roster characters through the remote list (as in the window)
        let dead_roster = [body(VICTIM, TEAM, &look, false)];
        let mut corpse_frame = AnimFrame::default();
        corpse_frame.corpses.push(body(CORPSE, 0, &look[..1], true));
        for i in 0..3 {
            scene.update_remotes_animated(&ctx, &renderer, &dead_roster, &corpse_frame, None);
            let drawn = scene.drawn();
            let shown = box_list(&dead_roster, &drawn, false);
            let models = scene.instances();
            let img = target
                .render_with(
                    &ctx,
                    &renderer,
                    gscene,
                    &camera,
                    Some((&mut boxes, &shown)),
                    &models,
                    None,
                )
                .expect("frame");
            let blue = blue_pixels(&img, w, h);
            if let Some(d) = &out {
                let p = d.join(format!(
                    "corpse_frame{i}_{}.png",
                    if threaded { "async" } else { "inline" }
                ));
                img.save_png(&p).expect("PNG");
                println!("wrote {}", p.display());
            }
            println!(
                "threaded {threaded} frame {i}: drawn {drawn:?}, boxes {}, blue px {blue}, instances {}",
                shown.len(),
                models.len()
            );
            assert!(
                shown.is_empty(),
                "threaded {threaded}: no box for the hidden victim in the corpse's frame {i}: {} box(es), {blue} blue px",
                shown.len()
            );
            assert_eq!(boxes.last_drawn, 0);
            assert!(
                drawn.contains(&CORPSE),
                "threaded {threaded}: the corpse is drawn (its body comes from the upload cache): {drawn:?}"
            );
            assert!(
                !models.is_empty(),
                "threaded {threaded}: the corpse model is drawn"
            );
        }
        // the debug boxes still show every remote, hidden or not
        assert_eq!(
            box_list(&dead_roster, &scene.drawn(), true).len(),
            1,
            "threaded {threaded}: debug boxes keep the hidden character"
        );
    }
}
