//! ROM- and GPU-gated (skipped without `CA_ROM` or a GPU adapter): fx-fixes3 bug 2 ("the train is missing
//! textures", JUNK_FLEA). An effect model key names up to fifteen skins (texture slot `i` of its model's pieces) and
//! four render styles (style slot `i`); the client read the first five skins and no style, so the pieces of the
//! world's scripted train whose material slot is 5 or more drew without a texture (the checker) and every piece took
//! the style the tables give the model file (none: the engine default). Asserted on the world's start-on placed
//! effects in a headless frame: every effect model drawn has no texture slot without an image, and no piece falls back
//! to the default style when its key names styles. Nothing of the ROM is printed by name.
// EVIDENCE: clientfx+0x17290 (model key parser: Skin0..Skin14 at +0xdc + 0x80 x i, RenderStyle0..3 at +0x85c + 0x80 x i)

use dc_client::characters::ModelScene;
use dc_client::fx::FxRunner;
use dc_client::world::GpuTarget;
use dc_client::{RomWorld, WorldSource};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use glam::{Quat, Vec3};

const MAP: &str = "JUNK_FLEA";

#[test]
fn rom_effect_model_pieces_all_have_their_skin() {
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
    for e in info.effects.iter().filter(|e| e.start_on) {
        fx.start_group(
            &String::from_utf8_lossy(&e.fx),
            Vec3::from(e.pos),
            Quat::IDENTITY,
            false,
            0.0,
        );
    }
    let mut models = ModelScene::open(&rom).expect("ROM models");
    models.set_async_loads(false);
    let mut checked = 0;
    let mut animated = 0;
    // one cycle of every group at 500 ms: each effect model alive then, uploaded as the frame uploads it
    for step in 0..(80 * 2) {
        let ms = step * 500;
        let list = fx.models_at(ms);
        models.update_fx_models(&ctx, &renderer, &list, lights);
        let skins = list
            .iter()
            .map(|m| m.model.textures.len())
            .max()
            .unwrap_or(0);
        let styles = list.iter().map(|m| m.styles.len()).max().unwrap_or(0);
        for inst in models.instances() {
            checked += 1;
            animated += usize::from(inst.anim.clip_count() > 0);
            assert_eq!(
                inst.missing_textures, 0,
                "at {ms} ms an effect model has {} texture slots without an image (keys name up to {skins} skins)",
                inst.missing_textures
            );
            if styles > 0 {
                assert_eq!(
                    inst.style_stats.default_pieces, 0,
                    "at {ms} ms a piece fell back to the default style (keys name up to {styles} styles)"
                );
            }
        }
    }
    assert!(checked > 0 && animated > 0, "the world draws the train");
    println!("{checked} effect-model frames checked, {animated} animated");
}
