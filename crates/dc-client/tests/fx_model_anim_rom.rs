//! ROM- and GPU-gated (skipped without `CA_ROM` or a GPU adapter): world-fixes2 bug 1 ("sometimes there's a black
//! texture on a small part of the ground by the stairs", JUNK_FLEA). The world's placed effects that start on are
//! played on the session clock; every model key among them whose model has a clip is drawn posed by that clip at the
//! key's time (clientfx+0x17be0 / +0x177a0), not at its bind pose. The JUNK_FLEA one is a long model whose clip
//! carries it along a track far from the effect's origin: at its bind pose its roof stuck out of the ground next to the
//! stairs at the origin, lit as underground. Asserted: during the key's life the drawn model never comes within
//! `NEAR` units (horizontally) of the effect's origin, and its world bounds move with the key's time. Nothing of the
//! ROM is printed by name.
// EVIDENCE: clientfx+0x17be0 (model key create: clip, looping off, override rate), clientfx+0x177a0 (sync to key)
// DC-DEFAULT: CLI-599 (proposed)

use dc_client::characters::ModelScene;
use dc_client::fx::FxRunner;
use dc_client::world::GpuTarget;
use dc_client::{RomWorld, WorldSource};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use glam::{Quat, Vec3};

const MAP: &str = "JUNK_FLEA";
/// Horizontal distance from the effect's origin the posed model keeps (the stairs stand within 400 units of it).
const NEAR: f32 = 400.0;

#[test]
fn rom_placed_effect_models_play_their_clip() {
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
    let mut src = RomWorld::new(&rom, MAP);
    let Ok(world) = src.load(Some(GpuTarget {
        ctx: &ctx,
        renderer: &renderer,
    })) else {
        println!("skipped: {MAP} not in this ROM");
        return;
    };
    let lights = world.model_lights.as_ref();
    let info = dc_dserver::world::load_from_rom(
        &rom,
        &format!("Worlds\\{MAP}"),
        dc_dserver::world::NameForm::Listed,
        None,
    )
    .expect("server world");
    let mut fx = FxRunner::open(Some(&rom));
    let mut origins = Vec::new();
    for e in info.effects.iter().filter(|e| e.start_on) {
        if e.rot.iter().any(|r| *r != 0.0) {
            continue;
        }
        let at = Vec3::from(e.pos);
        if fx.start_group(
            &String::from_utf8_lossy(&e.fx),
            at,
            Quat::IDENTITY,
            false,
            0.0,
        ) {
            origins.push(at);
        }
    }
    assert!(!origins.is_empty(), "the world starts a placed effect");
    let mut models = ModelScene::open(&rom).expect("ROM models");
    models.set_async_loads(false);
    let mut animated = 0;
    let mut spans: Vec<(Vec3, Vec3)> = Vec::new();
    // two cycles of every group at 250 ms: the effect models alive at each time, placed as the frame places them
    for step in 0..(2 * 80 * 4) {
        let ms = step * 250;
        let list = fx.models_at(ms);
        models.update_fx_models(&ctx, &renderer, &list, lights);
        if list.is_empty() {
            continue;
        }
        for inst in models.instances() {
            if inst.anim.clip_count() == 0 {
                continue;
            }
            animated += 1;
            let (lo, hi) = inst.bounds_in_world().expect("posed bounds");
            for o in &origins {
                // horizontal distance from the effect's origin to the drawn box
                let dx = (lo.x - o.x).max(o.x - hi.x).max(0.0);
                let dz = (lo.z - o.z).max(o.z - hi.z).max(0.0);
                assert!(
                    dx.hypot(dz) > NEAR,
                    "at {ms} ms an effect model is drawn {:.0} units from its effect's origin (box {lo} .. {hi})",
                    dx.hypot(dz)
                );
            }
            spans.push((lo, hi));
        }
    }
    assert!(animated > 0, "the world's effects draw an animated model");
    let moved = spans
        .iter()
        .map(|s| s.0.distance(spans[0].0))
        .fold(0.0f32, f32::max);
    assert!(
        moved > 1000.0,
        "the model moves with the key's time ({moved})"
    );
    println!("{animated} animated effect-model frames, moved {moved:.0} units");
}
