//! ROM-gated: the default character body model of the user's ROM (body row 0 of the ROM model table with face
//! row 0, the choice the game server makes for a player without a character choice) standing and walking, frames
//! 0 and half, on three maps; and the sky and translucent passes on every map. Skipped when CA_ROM is unset or no
//! graphics adapter exists. Map names come from the user's archive at run time; the images are ROM-derived
//! local files (folder from `DC_RENDER_OUT`, else the system temp folder), never part of the tree.

use dc_render::*;
use std::time::Instant;

/// FMT_DAT85.md §11: the ROM carries 21 worlds.
const ROM_WORLDS: usize = 21;
/// OBSERVED (dc-render example `world_lights`): 24 sky world models over the 21 worlds; 339 translucent or
/// additive world models whose objects are visible (194 translucent + 145 additive).
const ROM_SKY_MODELS: u64 = 24;
const ROM_BLENDED_MODELS: u64 = 339;

fn viewer(rom: &std::path::Path) -> Option<RomViewer> {
    match RomViewer::open(rom, AdapterChoice::Auto, RenderSettings::default()) {
        Ok(v) => Some(v),
        Err(ViewError::Gpu(e)) => {
            println!("skipped: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

fn diff_pixels(a: &FrameImage, b: &FrameImage) -> usize {
    a.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
        .filter(|(x, y)| (0..3).any(|k| (x[k] as i32 - y[k] as i32).abs() > 2))
        .count()
}

#[test]
fn rom_default_character_stands_and_walks_on_three_maps() {
    let rom = dc_rom::rom_or_skip!();
    let Some(v) = viewer(&rom) else {
        return;
    };
    let table = CharacterTable::from_vfs(v.vfs()).expect("character table");
    // SPEC: UDP_REFERENCE_FINDINGS_botclient.md (body row 0, skin 0, face row 0 for a player without a character
    // choice; DSV-24 proposed)
    let body = table.bodies.get(&0).expect("body row 0");
    let textures = table.body_textures(0, 0, 0).expect("body textures");
    let face = table
        .faces
        .get(&0)
        .map(|(m, s)| (m.clone(), s.iter().take(1).cloned().collect::<Vec<_>>()));
    let out = default_out_dir();
    let maps: Vec<String> = v.map_names().into_iter().take(3).collect();
    assert_eq!(maps.len(), 3);
    for map in &maps {
        let mut req = ViewRequest::new(map);
        req.width = 800;
        req.height = 600;
        let mut m = ModelRequest::new(&body.model);
        m.textures = Some(textures.clone());
        m.attachments.extend(face.clone());
        m.anim = Some(AnimSpec {
            pose: PoseSpec::Player { walk: false },
            phase: 0.0,
        });
        req.model = Some(m);
        let t = Instant::now();
        let mut p = v.prepare(&req).unwrap_or_else(|e| panic!("{map}: {e}"));
        let prepare_ms = t.elapsed().as_secs_f64() * 1000.0;
        let rep = p.report.model.clone().expect("model report");
        println!("{}", p.report.summary());
        assert!(
            rep.mesh.pieces > 0 && rep.mesh.skipped == 0,
            "{map}: pieces"
        );
        assert_eq!(rep.missing_textures, 0, "{map}: a body texture is missing");
        assert_eq!(rep.trackers.len(), 3, "{map}: the player clips resolve");
        assert_eq!(p.models.len(), 2, "{map}: body and face");
        // the world alone from the same camera, for the coverage check
        let (bare, _) = v
            .renderer
            .render_frame_offscreen(
                &v.ctx,
                &p.scene,
                &p.camera,
                &p.frame,
                &[],
                req.width,
                req.height,
            )
            .expect("frame");
        let mut frames = Vec::new();
        for (walk, label) in [(false, "stand"), (true, "walk")] {
            for phase in [0.0f32, 0.5] {
                let spec = AnimSpec {
                    pose: PoseSpec::Player { walk },
                    phase,
                };
                let t = Instant::now();
                let tr = spec.trackers(&p.models[0]);
                p.models[0].set_pose(&tr);
                let (body_only, rest) = p.models.split_at_mut(1);
                rest[0].follow(&body_only[0]);
                let pose_ms = t.elapsed().as_secs_f64() * 1000.0;
                let (img, fr) = v.render_prepared(&p, req.width, req.height).expect("frame");
                let file = out.join(format!(
                    "{}_{label}_{:02}.png",
                    map.to_ascii_lowercase(),
                    (phase * 10.0) as u32
                ));
                img.save_png(&file).expect("png");
                let covered = diff_pixels(&img, &bare);
                println!(
                    "  {label} phase {phase}: trackers {:?}, pose {pose_ms:.2} ms, gpu frame {:.2} ms, model pixels {covered} -> {}",
                    tr.iter().map(|t| (t.clip, t.time_ms as u32, t.blend_set)).collect::<Vec<_>>(),
                    fr.gpu_ms,
                    file.display()
                );
                assert_eq!(fr.models, 2);
                assert!(
                    covered > 500,
                    "{map} {label} {phase}: the model covers only {covered} pixels"
                );
                frames.push((label, phase, img));
            }
        }
        // walking moves the limbs: frame 0 and the half frame differ
        let walk0 = &frames[2].2;
        let walk5 = &frames[3].2;
        let moved = diff_pixels(walk0, walk5);
        println!("  walk frame 0 vs half: {moved} pixels differ; prepare {prepare_ms:.0} ms");
        assert!(moved > 100, "{map}: walking does not move the model");
    }
}

#[test]
fn rom_sky_and_translucent_passes_on_every_map() {
    let rom = dc_rom::rom_or_skip!();
    let Some(v) = viewer(&rom) else {
        return;
    };
    let out = default_out_dir();
    let maps = v.map_names();
    assert_eq!(maps.len(), ROM_WORLDS);
    let (mut sky_models, mut blended, mut sky_drawn, mut sky_changed) = (0u64, 0u64, 0u64, 0u64);
    let mut total_ms = 0.0;
    for map in &maps {
        let mut req = ViewRequest::new(map);
        req.width = 800;
        req.height = 600;
        let p = v.prepare(&req).unwrap_or_else(|e| panic!("{map}: {e}"));
        let (img, fr) = v.render_prepared(&p, req.width, req.height).expect("frame");
        let (plain, _) = v
            .renderer
            .render_frame_offscreen(
                &v.ctx,
                &p.scene,
                &p.camera,
                &FrameOptions::opaque_only(),
                &[],
                req.width,
                req.height,
            )
            .expect("frame");
        let changed = diff_pixels(&img, &plain);
        let s = &p.report.scene;
        sky_models += s.sky_models;
        blended += s.blended_models;
        if fr.sky_rect.is_some() {
            sky_drawn += 1;
        }
        if changed > 0 {
            sky_changed += 1;
        }
        let t = p.report.timings;
        total_ms += t.parse_ms + t.scene_build_ms + t.texture_decode_ms + t.upload_ms + fr.gpu_ms;
        let file = out.join(format!("{}_sky_translucent.png", map.to_ascii_lowercase()));
        img.save_png(&file).expect("png");
        println!(
            "{map}: sky models {} sections {} portals {} rect {:?} draws {}; blended {} (additive {}) drawn {}; \
             {changed} pixels differ from the opaque-only frame; gpu {:.2} ms -> {}",
            s.sky_models,
            s.sky_sections,
            s.sky_portals,
            fr.sky_rect,
            fr.sky_draws,
            s.blended_models,
            s.blended_additive,
            fr.blended_objects,
            fr.gpu_ms,
            file.display()
        );
        assert_eq!(p.report.scene.lightmaps_bad, 0);
    }
    println!(
        "all {} maps: sky models {sky_models}, blended models {blended}, sky drawn on {sky_drawn} maps from the first \
         spawn, frame changed by sky / translucent on {sky_changed} maps, {total_ms:.0} ms total",
        maps.len()
    );
    assert_eq!(sky_models, ROM_SKY_MODELS);
    assert_eq!(blended, ROM_BLENDED_MODELS);
}
