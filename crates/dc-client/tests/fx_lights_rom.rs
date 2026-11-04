//! ROM-gated checks of w2-fx (skipped without CA_ROM): the effect file's dynamic-light keys (type 4) and camera-shake
//! keys (type 6) are all read as lights and shakes, the weapons' muzzle groups light the models while they run, and
//! the explosion groups shake a camera next to them. Only counts are asserted (the spec's own counts); no ROM name or
//! value is printed or written.
// SPEC: FMT_SPR_LTC_FX.md §5.8 (types used: 4 dynamic light 93 keys, 6 camera shake 52 keys), §6.3
// EVIDENCE: clientfx+0xd970 (light update), clientfx+0x3f40 (shake update)

use dc_client::fx::{
    EffectLibrary, FxEvent, FxRunner, FxStage, FxTables, KeyKind, TextureStore, WorldFx,
};
use dc_client::world::{FrameState, GpuTarget};
use dc_client::{RomWorld, WorldSource};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use glam::{Quat, Vec3};

fn rom() -> Option<std::path::PathBuf> {
    let r = std::env::var_os("CA_ROM").map(std::path::PathBuf::from)?;
    r.is_dir().then_some(r)
}

#[test]
fn rom_light_and_shake_keys_are_all_read() {
    let Some(rom) = rom() else {
        eprintln!("skipped: CA_ROM not set");
        return;
    };
    let lib = EffectLibrary::open(&rom);
    assert!(!lib.is_empty());
    let (mut lights, mut shakes, mut lit_groups, mut shaking_groups) = (0, 0, 0, 0);
    for n in lib.names() {
        let g = lib.get(&n).expect("listed group");
        let l = g
            .keys
            .iter()
            .filter(|k| matches!(k.kind, KeyKind::Light(_)))
            .count();
        let s = g
            .keys
            .iter()
            .filter(|k| matches!(k.kind, KeyKind::Shake(_)))
            .count();
        lights += l;
        shakes += s;
        lit_groups += usize::from(l > 0);
        shaking_groups += usize::from(s > 0);
        for k in &g.keys {
            if let KeyKind::Shake(sk) = &k.kind {
                // clientfx+0x3c10: the outer square is never below the inner one (inner square + 1 in f32 may round
                // back to the inner square for a large radius)
                assert!(sk.outer2 >= sk.inner2);
            }
        }
    }
    eprintln!("light keys {lights} in {lit_groups} groups, shake keys {shakes} in {shaking_groups} groups");
    assert_eq!(lights, 93, "FMT_SPR_LTC_FX.md §5.8: 93 type-4 keys");
    assert_eq!(shakes, 52, "FMT_SPR_LTC_FX.md §5.8: 52 type-6 keys");
}

#[test]
fn rom_muzzle_groups_light_and_explosions_shake() {
    let Some(rom) = rom() else {
        eprintln!("skipped: CA_ROM not set");
        return;
    };
    let tables = FxTables::open(&rom);
    let library = EffectLibrary::open(&rom);
    // every group that holds a light key gives at least one light while it runs (radius and colour from its keys)
    let names: Vec<String> = library.names();
    let (mut with_light, mut lit) = (0, 0);
    for n in &names {
        let g = library.get(n).expect("group");
        let keys: Vec<_> = g
            .keys
            .iter()
            .filter(|k| matches!(k.kind, KeyKind::Light(_)))
            .collect();
        if keys.is_empty() {
            continue;
        }
        with_light += 1;
        let mut r = FxRunner::new(
            FxTables::default(),
            library.clone(),
            TextureStore::default(),
        );
        assert!(r.start_group(n, Vec3::ZERO, Quat::IDENTITY, false, 0.0));
        let any = keys.iter().any(|k| {
            let mid = (k.start + k.end) * 0.5;
            !r.lights_at((mid * 1000.0) as u32, Vec3::ZERO).is_empty()
        });
        lit += usize::from(any);
    }
    eprintln!("groups with light keys {with_light}, lit mid-key {lit}");
    assert!(with_light > 0);
    assert!(lit * 10 >= with_light * 9, "{lit} of {with_light}");
    // the weapons' first-person muzzle groups: at least one lights the view (a muzzle flash)
    let mut muzzle_lit = 0;
    for (pv, _) in tables.muzzle.values() {
        let Some(name) = pv else { continue };
        let Some(g) = library.get(name) else { continue };
        let ok = g.keys.iter().any(|k| matches!(k.kind, KeyKind::Light(_)));
        muzzle_lit += usize::from(ok);
    }
    eprintln!("weapon rows whose first-person muzzle group carries a light key: {muzzle_lit}");
    assert!(muzzle_lit > 0);
    // a group with a shake key moves a camera standing at its place while the key runs
    let mut shook = 0;
    for n in &names {
        let g = library.get(n).expect("group");
        let Some(k) = g.keys.iter().find(|k| matches!(k.kind, KeyKind::Shake(_))) else {
            continue;
        };
        let mut runner = FxRunner::new(
            FxTables::default(),
            library.clone(),
            TextureStore::default(),
        );
        runner.start_group(n, Vec3::ZERO, Quat::IDENTITY, false, 0.0);
        let mid = ((k.start + k.end) * 0.5 * 1000.0) as u32;
        let off = runner.shake_at(mid, Vec3::from(k.offset));
        shook += usize::from(off != Vec3::ZERO);
    }
    eprintln!("groups whose shake moved a camera at the key: {shook}");
    assert!(shook > 0);
}

/// The frame stage on a loaded ROM world: the effect quads take the world's fog, a running light group reaches the
/// world's model lighting through `camera_frame`, and a looping placed effect of a light group keeps lighting after its
/// first cycle while its object exists.
#[test]
fn rom_stage_hands_fog_and_effect_lights_to_the_frame() {
    let Some(rom) = rom() else {
        eprintln!("skipped: CA_ROM not set");
        return;
    };
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipped: no GPU adapter ({e})");
            return;
        }
    };
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = RomWorld::new(&rom, "JUNK_FLEA");
    let Ok(world) = src.load(Some(GpuTarget {
        ctx: &ctx,
        renderer: &renderer,
    })) else {
        eprintln!("skipped: map not in this ROM");
        return;
    };
    let mut stage = FxStage::open(Some(&rom));
    let _ = stage.frame(0, &FrameState::default(), &world);
    let scene_fog = world.scene.as_ref().and_then(|s| s.fog);
    assert!(
        scene_fog.is_some(),
        "the parity audit: every ROM world is fogged"
    );
    assert_eq!(stage.fog(), scene_fog);
    // a group with a light key, started as a looping placed effect (value bit 0) of object 300
    let lib = &stage.runner.library;
    let (name, start, end, len) = lib
        .names()
        .into_iter()
        .find_map(|n| {
            let g = lib.get(&n)?;
            let k = g
                .keys
                .iter()
                .find(|k| matches!(k.kind, KeyKind::Light(_)))?;
            (g.length_s > 0.2 && k.end > k.start).then(|| (n.clone(), k.start, k.end, g.length_s))
        })
        .expect("a group with a light key");
    let p = world
        .spawns
        .first()
        .map(|s| s.pos)
        .unwrap_or(dc_sim::V3::new(0.0, 0.0, 0.0));
    let at = [p.x, p.y, p.z];
    let fs = FrameState {
        fx_events: vec![FxEvent::World(WorldFx {
            name,
            pos: at,
            rot: [0.0, 0.0, 0.0, 1.0],
            obj: Some(300),
            value: dc_client::fx::WORLD_FX_LOOP,
        })],
        ..FrameState::default()
    };
    let _ = stage.frame(0, &fs, &world);
    let eye = dc_sim::V3::new(at[0], at[1] + 50.0, at[2]);
    let lights = world.model_lights.as_ref().expect("model lights");
    let mid = |cycle: f32| (((start + end) * 0.5 + cycle * len) * 1000.0) as u32;
    let _ = stage.camera_frame(mid(0.0), eye, &world);
    assert!(!lights.fx_lights().is_empty(), "first cycle lit");
    // step the clock through two more group lengths at 25 Hz: the third cycle lights again
    let mut ms = 0;
    while ms < mid(2.0) {
        ms += 40;
        let _ = stage.frame(ms, &FrameState::default(), &world);
    }
    let _ = stage.camera_frame(mid(2.0), eye, &world);
    assert!(!lights.fx_lights().is_empty(), "third cycle lit");
    assert!(stage.runner.stats.world_cycles >= 2);
}
