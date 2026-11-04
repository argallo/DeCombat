//! CLI-1593: the client's offscreen frame with MSAA 4 on the made-up world of `dc_client::testkit` (no ROM): the
//! world pass, the debug box pass and the effect pass draw into the multisampled target and exactly one of them
//! resolves into the colour target, whichever draws last: the world pass alone; the box pass when it has boxes; the
//! box pass's empty resolve when every box is filtered out; the world pass when the effect stage has no quads. Every
//! frame reads back without a validation error, the fully covered pixels equal the single-sample frame's, and the
//! same world through the single-sample path with an idle effect stage draws the same bytes as without it. Skipped
//! when no adapter exists.
// SPEC: RENDER_2009.md §4.1 (camera), §4.3 (world depth range)

use dc_client::debug_draw::{BoxInstance, DebugBoxes};
use dc_client::fx::FxStage;
use dc_client::offscreen::Offscreen;
use dc_client::testkit::synth_source;
use dc_client::world::GpuTarget;
use dc_client::{ViewAngles, ViewSettings, WorldSource};
use dc_render::{
    supported_msaa, AdapterChoice, FrameImage, GpuContext, RenderSettings, WorldRenderer,
    OFFSCREEN_FORMAT,
};
use dc_sim::V3;

const W: u32 = 160;
const H: u32 = 120;

fn differing(a: &FrameImage, b: &FrameImage) -> usize {
    a.rgba
        .as_chunks::<4>().0.iter()
        .zip(b.rgba.as_chunks::<4>().0.iter())
        .filter(|(x, y)| x != y)
        .count()
}

#[test]
fn the_offscreen_frame_resolves_once_whichever_pass_draws_last() {
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no adapter ({e})");
            return;
        }
    };
    if !supported_msaa(&ctx.adapter, OFFSCREEN_FORMAT).contains(&4) {
        println!("skipped: the adapter has no 4x MSAA for {OFFSCREEN_FORMAT:?}");
        return;
    }
    let camera =
        ViewSettings::default().camera(V3::new(0.0, 100.0, 0.0), &ViewAngles::default(), W, H);
    let near = BoxInstance {
        centre: [0.0, 100.0, 300.0],
        half: [30.0, 30.0, 30.0],
        colour: [1.0, 0.1, 0.1, 1.0],
    };
    let degenerate = BoxInstance {
        half: [0.0, 0.0, 0.0],
        ..near
    };

    // ---- the single-sample reference: world alone, world with the box, world with an idle effect stage ----
    let renderer1 = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src1 = synth_source();
    let world1 = src1
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer1,
        }))
        .expect("synthetic world");
    let scene1 = world1.scene.as_ref().expect("scene");
    let target1 = Offscreen::new(&ctx, OFFSCREEN_FORMAT, W, H);
    assert_eq!(target1.msaa(), 1);
    let mut boxes1 = DebugBoxes::new(&ctx, OFFSCREEN_FORMAT);
    let mut fx1 = FxStage::open(None);
    let plain1 = target1
        .render_with(&ctx, &renderer1, scene1, &camera, None, &[], None)
        .unwrap();
    let boxed1 = target1
        .render_with(
            &ctx,
            &renderer1,
            scene1,
            &camera,
            Some((&mut boxes1, &[near])),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(boxes1.last_drawn, 1);
    assert_ne!(plain1.rgba, boxed1.rgba, "the box shows");
    let idle_fx1 = target1
        .render_with_fx(
            &ctx,
            &renderer1,
            scene1,
            &camera,
            Some((&mut boxes1, &[])),
            &[],
            &[],
            Some((&mut fx1, 0, None)),
            None,
            None,
        )
        .unwrap();
    assert_eq!(fx1.drawn(), 0);
    assert_eq!(plain1.rgba, idle_fx1.rgba, "an idle effect stage leaves the single-sample frame alone");

    // ---- 4 samples: renderer, scene, targets and passes of that count ----
    let renderer4 = WorldRenderer::new(
        &ctx,
        OFFSCREEN_FORMAT,
        RenderSettings {
            msaa: 4,
            ..RenderSettings::default()
        },
    );
    assert_eq!(renderer4.msaa(), 4);
    let mut src4 = synth_source();
    let world4 = src4
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer4,
        }))
        .expect("synthetic world");
    let scene4 = world4.scene.as_ref().expect("scene");
    let mut target4 = Offscreen::new(&ctx, OFFSCREEN_FORMAT, W, H);
    target4.set_msaa(&ctx, 4);
    assert_eq!(target4.msaa(), 4);
    let mut boxes4 = DebugBoxes::with_msaa(&ctx, OFFSCREEN_FORMAT, 4);
    assert_eq!(boxes4.msaa(), 4);
    let mut fx4 = FxStage::open(None);
    fx4.set_msaa(4);

    // the world pass resolves
    let plain4 = target4
        .render_with(&ctx, &renderer4, scene4, &camera, None, &[], None)
        .unwrap();
    assert_eq!((plain4.width, plain4.height), (W, H));
    assert_eq!(plain4.pixel(W / 2, H / 2), plain1.pixel(W / 2, H / 2), "flat interior unchanged");
    let d = differing(&plain1, &plain4);
    println!("world alone: {d} of {} pixels differ between 1x and 4x", W * H);
    assert!(d < (W * H / 4) as usize, "{d} pixels differ: more than edges");

    // the box pass resolves (it has a box)
    let boxed4 = target4
        .render_with(
            &ctx,
            &renderer4,
            scene4,
            &camera,
            Some((&mut boxes4, &[near])),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(boxes4.last_drawn, 1);
    assert_eq!(boxed4.pixel(W / 2, H / 2), boxed1.pixel(W / 2, H / 2), "the box's face");
    assert_ne!(plain4.rgba, boxed4.rgba);

    // the box pass is named the resolving pass but every box is filtered out: it resolves alone
    let filtered4 = target4
        .render_with(
            &ctx,
            &renderer4,
            scene4,
            &camera,
            Some((&mut boxes4, &[degenerate])),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(boxes4.last_drawn, 0);
    assert_eq!(plain4.rgba, filtered4.rgba, "the empty resolve gives the world frame");

    // the effect stage has no quads and the box list is empty: the world pass resolves, nothing else records
    let idle_fx4 = target4
        .render_with_fx(
            &ctx,
            &renderer4,
            scene4,
            &camera,
            Some((&mut boxes4, &[])),
            &[],
            &[],
            Some((&mut fx4, 0, None)),
            None,
            None,
        )
        .unwrap();
    assert_eq!(fx4.drawn(), 0);
    assert_eq!(boxes4.last_drawn, 0);
    assert_eq!(plain4.rgba, idle_fx4.rgba);

    // back to single sampling: the targets go and the frame is the reference again (renderer of count 1)
    target4.set_msaa(&ctx, 1);
    assert_eq!(target4.msaa(), 1);
    let back = target4
        .render_with(&ctx, &renderer1, scene1, &camera, None, &[], None)
        .unwrap();
    assert_eq!(back.rgba, plain1.rgba);
}
