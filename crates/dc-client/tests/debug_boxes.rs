//! The debug box pass on the made-up world of `dc_client::testkit` (no ROM): a box in front of the camera is drawn
//! in its team colour over the world, a box behind the far wall is hidden by the world's depth, and the pass
//! leaves the frame alone without boxes. Skipped when no adapter exists.
// SPEC: RENDER_2009.md §4.1 (camera), §4.3 (world depth range)

use dc_client::debug_draw::{BoxInstance, DebugBoxes};
use dc_client::offscreen::Offscreen;
use dc_client::testkit::synth_source;
use dc_client::world::GpuTarget;
use dc_client::{net::team_colour, ViewAngles, ViewSettings, WorldSource};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use dc_sim::V3;

#[test]
fn boxes_are_drawn_in_team_colour_and_hidden_behind_walls() {
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no adapter ({e})");
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
    let scene = world.scene.as_ref().expect("scene");
    let (w, h) = (160u32, 120u32);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let mut boxes = DebugBoxes::new(&ctx, OFFSCREEN_FORMAT);
    // facing +Z from 100 units up: the far wall (z = 1500) fills the centre
    let camera =
        ViewSettings::default().camera(V3::new(0.0, 100.0, 0.0), &ViewAngles::default(), w, h);
    let centre = |img: &dc_render::FrameImage| img.pixel(w / 2, h / 2);
    let plain = target
        .render_with(
            &ctx,
            &renderer,
            scene,
            &camera,
            Some((&mut boxes, &[])),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(boxes.last_drawn, 0);
    let wall = centre(&plain);
    println!("centre without boxes {wall:?}");
    let red = team_colour(1);
    let near = BoxInstance {
        centre: [0.0, 100.0, 300.0],
        half: [30.0, 30.0, 30.0],
        colour: red,
    };
    let img = target
        .render_with(
            &ctx,
            &renderer,
            scene,
            &camera,
            Some((&mut boxes, &[near])),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(boxes.last_drawn, 1);
    let px = centre(&img);
    println!("centre with a near team-1 box {px:?}");
    assert!(px[0] as i32 > px[2] as i32 + 60, "red box in front: {px:?}");
    // a team-2 box behind the wall: the centre keeps the wall's colour
    let far = BoxInstance {
        centre: [0.0, 100.0, 1800.0],
        half: [60.0, 60.0, 60.0],
        colour: team_colour(2),
    };
    let img = target
        .render_with(
            &ctx,
            &renderer,
            scene,
            &camera,
            Some((&mut boxes, &[far])),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(boxes.last_drawn, 1);
    assert_eq!(centre(&img), wall, "hidden by the wall's depth");
    // non-finite boxes are skipped
    let bad = BoxInstance {
        centre: [f32::NAN, 0.0, 0.0],
        half: [1.0; 3],
        colour: red,
    };
    target
        .render_with(
            &ctx,
            &renderer,
            scene,
            &camera,
            Some((&mut boxes, &[bad])),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(boxes.last_drawn, 0);
}
