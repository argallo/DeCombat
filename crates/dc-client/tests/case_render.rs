//! ROM- and GPU-gated (skipped without `CA_ROM` or a GPU adapter): world-fixes bug 2 ("the HiSec case model did not
//! appear; sometimes it had no lighting"). A HiSec case of each case-table model, at three places of the first ROM
//! map, is in the pickup draw set on the frame of its arrival (inline loads) and lit with the world light set at its
//! position (the per-object sun cache form every model object takes); with loader threads it is drawn once its load
//! lands. Nothing of the ROM is asserted by value and no ROM name is printed.
// SPEC: HISEC_2009.md §3.1 (the case is a kind-1 model object with the type-0x17 kind-5 blob); RENDER_2009.md §6.3
// (the model light set at the object position)
// EVIDENCE: engine+0x1b21b0 (the light set is gathered at the object position)

use dc_client::characters::ModelScene;
use dc_client::drops::{WeaponDrop, PICKUP_KIND_CASE};
use dc_client::world::{GpuTarget, SunObject};
use dc_client::{ModelRef, RomWorld, WorldSource};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use glam::Vec3;

#[test]
fn rom_case_is_drawn_on_arrival_and_lit_by_the_world() {
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
    let mut src = RomWorld::new(&rom, &maps[0]);
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("ROM world");
    let lights = world.model_lights.as_ref().expect("world lights");
    let rows = dc_dserver::rom_tables::HisecRows::from_doc(
        &dc_bute::ButeDoc::from_vfs(
            &dc_rez::RezVfs::new(&rom),
            "ATTRIBUTES.rez",
            dc_dserver::rom_tables::HISECS_ENTRY,
        )
        .expect("case table"),
    )
    .rows;
    let spawns: Vec<Vec3> = world
        .spawns
        .iter()
        .take(3)
        .map(|s| Vec3::new(s.pos.x, s.pos.y, s.pos.z))
        .collect();
    assert!(!spawns.is_empty(), "the map has spawns");
    for asyncl in [false, true] {
        let mut models = ModelScene::open(&rom).expect("ROM models");
        models.set_async_loads(asyncl);
        let mut obj = 70u16;
        for row in rows.values().filter(|r| r.model.is_some()) {
            for at in &spawns {
                obj += 1;
                let d = WeaponDrop {
                    obj,
                    kind: PICKUP_KIND_CASE,
                    weapon: 0,
                    rounds: 0,
                    spin: true,
                    fall: false,
                    pos: (*at - Vec3::new(0.0, 30.0, 0.0)).to_array(),
                    rot: [0.0, 0.0, 0.0, 1.0],
                    model: Some(ModelRef {
                        model: row.model.clone().unwrap(),
                        textures: row.skins.iter().take(1).cloned().collect(),
                    }),
                    trackers: Vec::new(),
                    seen_ms: 0,
                };
                let mut frames = 0;
                loop {
                    models.update_drops(
                        &ctx,
                        &renderer,
                        std::slice::from_ref(&d),
                        frames * 16,
                        None,
                        Some(lights),
                    );
                    if models.drop_models().instance(obj).is_some() || frames > 500 {
                        break;
                    }
                    frames += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                let inst = models
                    .drop_models()
                    .instance(obj)
                    .expect("the case is drawn");
                if !asyncl {
                    assert_eq!(frames, 0, "inline: drawn on the frame of its arrival");
                }
                assert_eq!(models.drops_drawn(), 1);
                let want =
                    lights.light_set_for(SunObject::Drop(obj), Vec3::from(d.pos), inst.radius);
                assert_eq!(
                    inst.lights.ambient, want.ambient,
                    "the world's ambient at its place"
                );
                assert_eq!(inst.lights.lights.len(), want.lights.len());
                assert!(
                    inst.lights.ambient.max_element() > 0.0 || !inst.lights.lights.is_empty(),
                    "lit"
                );
                println!(
                    "async {asyncl}: drawn after {frames} frame(s), ambient {:?}, {} lights",
                    inst.lights.ambient,
                    inst.lights.lights.len()
                );
                models.update_drops(&ctx, &renderer, &[], 0, None, Some(lights));
            }
        }
    }
}
