//! ROM-gated: section textures that load from no archive. The 2009 renderer keeps a null slot for them: a missing
//! first texture sends the section down the bucket selector's no-texture path, a missing second texture of a
//! two-texture blend section leaves that stage unbound (sampled as opaque black). Nothing on screen may show the
//! magenta debug checker. Skipped when CA_ROM is unset (the frame part also when no graphics adapter exists). Map
//! and texture names come from the user's archive at run time and are not part of the tree.

use dc_render::*;
use dc_world::World;

/// Counts the batches that name a texture the ROM does not hold, per stage.
#[derive(Debug, Default)]
struct Unresolved {
    first: usize,
    second: usize,
}

#[test]
fn rom_unresolved_section_textures_follow_the_null_slot_rules() {
    let rom = dc_rom::rom_or_skip!();
    let worlds = WorldSource::open(&rom).expect("worlds");
    let index = TextureIndex::open(&rom).expect("texture index");
    let mut maps_with_second = Vec::new();
    let mut total = Unresolved::default();
    for map in worlds.map_names() {
        let data = worlds.read(&map).expect("read").expect("world");
        let world = World::parse(&data).expect("parse");
        let mut tex = RomTextures::new(&index);
        let mesh = SceneMesh::build(&world, SceneOptions::default(), &mut |n| tex.traits(n));
        let resolves =
            |i: Option<u32>| i.is_none_or(|i| index.locate(&mesh.textures[i as usize]).is_some());
        let mut here = Unresolved::default();
        let all = mesh
            .batches
            .iter()
            .chain(&mesh.sky_batches)
            .chain(mesh.blended.iter().flat_map(|m| m.batches.iter()));
        for b in all {
            if !resolves(b.tex0) {
                here.first += 1;
                // SPEC: RENDER_2009.md §5.2 (no texture: vertex-lit -> bucket 0, lightmapped texture -> not drawn)
                assert!(
                    b.bucket == 0 || b.bucket == 10 || b.bucket == 20,
                    "{map}: a section with an unresolved first texture drew in textured bucket {}",
                    b.bucket
                );
            }
            if !resolves(b.tex1) {
                here.second += 1;
                assert!(
                    matches!(b.bucket, 10 | 20),
                    "{map}: second texture outside the blend buckets"
                );
            }
        }
        if here.first + here.second > 0 {
            println!(
                "{map}: unresolved first-texture batches {}, second-texture batches {}",
                here.first, here.second
            );
        }
        if here.second > 0 {
            maps_with_second.push(map.clone());
        }
        total.first += here.first;
        total.second += here.second;
    }
    println!("total {total:?}; maps with an unresolved blend texture: {maps_with_second:?}");
    // OBSERVED (2026-10-30, EU-test ROM): the outer embankment of GRAYHAMMER blends towards a texture that is in
    // no archive; the test needs at least one such section to mean anything.
    assert!(
        total.second > 0,
        "no blend section with an unresolved second texture in this ROM"
    );
}

#[test]
fn rom_unresolved_blend_texture_draws_no_debug_checker() {
    let rom = dc_rom::rom_or_skip!();
    let viewer = match RomViewer::open(&rom, AdapterChoice::Auto, RenderSettings::default()) {
        Ok(v) => v,
        Err(ViewError::Gpu(e)) => {
            println!("skipped: {e}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let map = viewer
        .map_names()
        .into_iter()
        .find(|m| m == "GRAYHAMMER")
        .expect("GRAYHAMMER in the ROM");
    let mut req = ViewRequest::new(&map);
    req.width = 640;
    req.height = 360;
    // A spot on the rail track looking south over the fence at the outer embankment (world units, degrees).
    req.camera = CameraSpec::Explicit {
        eye: [-300.0, 250.0, -3300.0],
        yaw_deg: 180.0,
        pitch_deg: 15.0,
    };
    let (img, report) = viewer.render(&req).expect("render");
    println!("{}", report.summary());
    assert!(
        report.upload.missing_textures > 0,
        "the view's world should hold the unresolved blend texture"
    );
    let magenta = img
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] as i32 > p[1] as i32 + 60 && p[2] as i32 > p[1] as i32 + 60)
        .count();
    let frac = magenta as f64 / (img.width * img.height) as f64;
    println!("magenta-like pixels {magenta} ({:.2} %)", frac * 100.0);
    if let Ok(dir) = std::env::var("DC_RENDER_OUT") {
        img.save_png(&std::path::Path::new(&dir).join("grayhammer_unbound.png"))
            .expect("png");
    }
    assert!(
        frac < 0.001,
        "{map}: the unresolved blend texture still shows the debug checker"
    );
}
