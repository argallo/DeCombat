//! ROM-gated: render styles, the world's global light, light-group ambient, LOD switching and the held weapon
//! with the user's ROM. Skipped when CA_ROM is unset (and the GPU parts when no graphics adapter exists).
//! Counts below are OBSERVED over the EU-test ROM (dc-render example `style_census`); map names come from the
//! user's archive at run time; images are ROM-derived local files (folder from `DC_RENDER_OUT`, else the system
//! temp folder), never part of the tree.

use dc_model::StyleFile;
use dc_render::*;
use dc_world::World;
use glam::Vec3;
use std::collections::BTreeMap;

// OBSERVED: style_census (RS.rez: 62 entries, 61 render-style files of one variant, 1 zero-length entry;
// passes 1 / 2 / 3 in 54 / 6 / 1 files; stage-0 colour op MODULATE2X in 25 files; 13 files without the
// options block)
const STYLE_REZ: &str = "RS.rez";
const STYLE_ENTRIES: usize = 62;
const STYLE_FILES: usize = 61;
const STYLE_PASSES: [(usize, u64); 3] = [(1, 54), (2, 6), (3, 1)];
const STYLE_MODULATE2X_FILES: u64 = 25;
const STYLE_NO_OPTIONS: u64 = 13;
// OBSERVED: style_census (297 weapon rows with a third-person model, style slot 0 MODULATE2X in 284)
const HAND_ROWS: usize = 297;
const HAND_MODULATE2X: u64 = 284;
// OBSERVED: world_lights / class census (the global light class on 19 of the 21 worlds, one each)
const WORLDS: usize = 21;
const WORLDS_WITH_GLOBAL_LIGHT: usize = 19;

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

/// Pixels that differ from `bare` (the model's coverage) and their mean luminance in `img`.
fn coverage(img: &FrameImage, bare: &FrameImage) -> (usize, f64) {
    let (mut n, mut sum) = (0usize, 0f64);
    for (x, y) in img
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(bare.rgba.as_chunks::<4>().0)
    {
        if (0..3).any(|k| (x[k] as i32 - y[k] as i32).abs() > 2) {
            n += 1;
            sum += 0.299 * x[0] as f64 + 0.587 * x[1] as f64 + 0.114 * x[2] as f64;
        }
    }
    (n, if n > 0 { sum / n as f64 } else { 0.0 })
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
fn rom_style_census_counts() {
    let rom = dc_rom::rom_or_skip!();
    let vfs = dc_rez::RezVfs::new(&rom);
    let entries = vfs.list_entries(STYLE_REZ).expect("style archive");
    assert_eq!(entries.len(), STYLE_ENTRIES);
    let mut parsed = BTreeMap::new();
    let (mut passes, mut m2x, mut no_options) = (BTreeMap::new(), 0u64, 0u64);
    for (name, _, _) in &entries {
        let data = vfs.entry(STYLE_REZ, name).unwrap().unwrap_or_default();
        let Ok(f) = StyleFile::parse(&data) else {
            assert!(data.is_empty(), "a non-empty style entry does not parse");
            continue;
        };
        assert_eq!(f.variants.len(), 1);
        assert_eq!(f.trailing, 0);
        let v = &f.variants[0];
        assert_eq!(v.len_word, StyleFile::variant_len(v));
        // a parsed file encodes back to its own bytes
        assert_eq!(f.to_bytes().unwrap(), data);
        *passes.entry(v.passes.len()).or_insert(0u64) += 1;
        no_options += v.options.is_none() as u64;
        let s = RenderStyle::from_file(&f, name).expect("translates");
        // OBSERVED: no ROM style selects the effect path (options byte 0)
        assert!(!s.effect);
        m2x += (s.passes[0].stages[0].colour_op == StageOp::Modulate2x) as u64;
        parsed.insert(style_key(name), s);
    }
    println!(
        "styles {} of {}, passes {passes:?}, stage-0 MODULATE2X {m2x}, without options {no_options}",
        parsed.len(),
        entries.len()
    );
    assert_eq!(parsed.len(), STYLE_FILES);
    assert_eq!(passes, STYLE_PASSES.into_iter().collect());
    assert_eq!(m2x, STYLE_MODULATE2X_FILES);
    assert_eq!(no_options, STYLE_NO_OPTIONS);

    // the default character: body row 0 and face row 0 name styles that load; the body's skin pieces use
    // MODULATE2X (the brightness question of render-models, settled by the data)
    let table = CharacterTable::from_vfs(&vfs).expect("character table");
    let body = table.bodies.get(&0).expect("body row 0");
    let mut lib = StyleLibrary::new();
    let list = lib.list(&vfs, &body.styles);
    println!("body row 0 styles: {:?}", describe(&list));
    assert!(!list.is_empty() && list.iter().all(|s| s.is_some()));
    let files = ModelFiles::load(&vfs, &body.model).expect("body model");
    let mesh = ModelMesh::build(&files.body);
    let mut ops: BTreeMap<String, u64> = BTreeMap::new();
    for d in mesh.draws.iter().filter(|d| d.lod == 0) {
        let s = style_for_slot(&list, d.style);
        *ops.entry(format!("{:?}", s.passes[0].stages[0].colour_op))
            .or_default() += 1;
    }
    println!("body row 0 pieces by stage-0 op: {ops:?}");
    assert!(ops.get("Modulate2x").copied().unwrap_or(0) >= 1);
    let face = table.face_styles.get(&0).expect("face row 0");
    let flist = lib.list(&vfs, face);
    assert!(flist.iter().all(|s| s.is_some()));
    let hands = HandModels::from_vfs(&vfs).expect("weapon table");
    assert_eq!(hands.rows.len(), HAND_ROWS);
    let mut h2x = 0u64;
    for r in hands.rows.values() {
        if let Some(s) = r.styles.first().and_then(|p| lib.get(&vfs, p)) {
            h2x += (s.passes[0].stages[0].colour_op == StageOp::Modulate2x) as u64;
        }
    }
    assert_eq!(h2x, HAND_MODULATE2X);
}

#[test]
fn rom_global_light_and_light_group_grids() {
    let rom = dc_rom::rom_or_skip!();
    let src = WorldSource::open(&rom).expect("worlds");
    let maps = src.map_names();
    assert_eq!(maps.len(), WORLDS);
    let (mut with_light, mut grids, mut lit_cells) = (0usize, 0usize, 0usize);
    for map in &maps {
        let data = src.read(map).unwrap().unwrap();
        let w = World::parse(&data).unwrap();
        assert!(
            global_light_count(&w) <= 1,
            "{map}: more than one global light"
        );
        if let Some(g) = global_light(&w) {
            with_light += 1;
            // the sun shines down and is not black
            assert!(g.dir.y < -0.3, "{map}: sun direction {}", g.dir);
            assert!(
                g.colour.max_element() > 20.0,
                "{map}: sun colour {}",
                g.colour
            );
            assert!((0.0..=1.0).contains(&g.to_ambient));
            println!(
                "{map}: sun dir ({:.3}, {:.3}, {:.3}) colour ({:.0}, {:.0}, {:.0}) to-ambient {:.2}",
                g.dir.x, g.dir.y, g.dir.z, g.colour.x, g.colour.y, g.colour.z, g.to_ambient
            );
        }
        let r = w.render.as_ref().expect("render data");
        if r.grids.is_empty() {
            continue;
        }
        let amb = AmbientGrid::with_groups(&w.light_grid, &r.grids, 1.0);
        for (gi, lg) in r.grids.iter().enumerate() {
            grids += 1;
            // sample the middle of the group's box
            let mid = Vec3::new(
                lg.offset[0] as f32 + lg.dims[0] as f32 * 0.5,
                lg.offset[1] as f32 + lg.dims[1] as f32 * 0.5,
                lg.offset[2] as f32 + lg.dims[2] as f32 * 0.5,
            );
            let p = Vec3::from(w.light_grid.origin) + mid * Vec3::from(w.light_grid.cell);
            let add = amb.group_part(p);
            let nonzero = lg.data.iter().filter(|&&b| b > 0).count();
            lit_cells += nonzero;
            println!(
                "{map}: group grid {gi}: dims {:?}, {nonzero} lit cells, colour {:?}, adds ({:.1}, {:.1}, {:.1}) at its centre",
                lg.dims, lg.colour, add.x, add.y, add.z
            );
            assert!(add.min_element() >= 0.0);
        }
    }
    println!(
        "worlds with a global light {with_light}, light-group grids {grids}, lit cells {lit_cells}"
    );
    assert_eq!(with_light, WORLDS_WITH_GLOBAL_LIGHT);
    assert!(grids > 0 && lit_cells > 0, "no light-group grid adds light");
}

fn character_request(map: &str, table: &CharacterTable) -> ViewRequest {
    let body = table.bodies.get(&0).expect("body row 0");
    let mut req = ViewRequest::new(map);
    req.width = 800;
    req.height = 600;
    let mut m = ModelRequest::new(&body.model);
    m.textures = table.body_textures(0, 0, 0);
    m.attachments.extend(
        table
            .faces
            .get(&0)
            .map(|(f, s)| (f.clone(), s.iter().take(1).cloned().collect::<Vec<_>>())),
    );
    m.anim = Some(AnimSpec {
        pose: PoseSpec::Player { walk: false },
        phase: 0.0,
    });
    req.model = Some(m);
    req
}

#[test]
fn rom_default_character_with_and_without_styles() {
    let rom = dc_rom::rom_or_skip!();
    let Some(v) = viewer(&rom) else {
        return;
    };
    let table = CharacterTable::from_vfs(v.vfs()).expect("character table");
    let out = default_out_dir();
    for map in v.map_names().into_iter().take(3) {
        let mut shots = Vec::new();
        let mut bare = None;
        // (plain, world sun): the old state, styles alone, styles with the world's global light
        for (plain, sun, label) in [
            (true, false, "plain"),
            (false, false, "styles"),
            (false, true, "styles_sun"),
        ] {
            let mut req = character_request(&map, &table);
            if let Some(m) = req.model.as_mut() {
                m.plain = plain;
                m.world_sun = sun;
            }
            let p = v.prepare(&req).unwrap_or_else(|e| panic!("{map}: {e}"));
            let rep = p.report.model.clone().expect("model report");
            if bare.is_none() {
                bare = Some(
                    v.renderer
                        .render_frame_offscreen(
                            &v.ctx,
                            &p.scene,
                            &p.camera,
                            &p.frame,
                            &[],
                            req.width,
                            req.height,
                        )
                        .expect("frame")
                        .0,
                );
            }
            let (img, _) = v.render_prepared(&p, req.width, req.height).expect("frame");
            let file = out.join(format!("{}_{label}.png", map.to_ascii_lowercase()));
            img.save_png(&file).expect("png");
            let (n, lum) = coverage(&img, bare.as_ref().unwrap());
            println!(
                "{map} {label}: model pixels {n}, mean luminance {lum:.1}, ambient {:?}, styles {:?}, stage-0 ops {:?}, sun {:?} -> {}",
                rep.ambient.map(|a| a.round()),
                rep.styles,
                rep.style_stats.stage0_ops,
                rep.sun.map(|s| s.1.map(|c| c.round())),
                file.display()
            );
            if !plain {
                assert_eq!(
                    rep.style_stats.default_pieces, 0,
                    "{map}: a style slot is empty"
                );
                assert!(rep.style_stats.stage0_ops.contains_key("Modulate2x"));
            }
            shots.push((label, img, n, lum));
        }
        let changed = diff_pixels(&shots[0].1, &shots[1].1);
        println!(
            "{map}: styles vs plain: {changed} pixels differ, mean luminance {:.1} -> {:.1} ({:.2}x); with the world sun {:.1}",
            shots[0].3,
            shots[1].3,
            shots[1].3 / shots[0].3.max(1e-9),
            shots[2].3
        );
        assert!(shots.iter().all(|s| s.2 > 500), "{map}: model not drawn");
        assert!(changed > 500, "{map}: styles changed only {changed} pixels");
        assert!(
            shots[1].3 > shots[0].3 * 1.2,
            "{map}: styled model not brighter than plain"
        );
        assert!(
            shots[2].3 >= shots[1].3 - 0.5,
            "{map}: the sun darkened the model"
        );
    }
}

#[test]
fn rom_world_sun_on_an_outdoor_map() {
    let rom = dc_rom::rom_or_skip!();
    let Some(v) = viewer(&rom) else {
        return;
    };
    let table = CharacterTable::from_vfs(v.vfs()).expect("character table");
    // the outdoor map: the world whose global light is brightest (largest scaled colour channel)
    let src = WorldSource::open(&rom).expect("worlds");
    let mut best: Option<(String, f32)> = None;
    for map in src.map_names() {
        let data = src.read(&map).unwrap().unwrap();
        let w = World::parse(&data).unwrap();
        if let Some(g) = global_light(&w) {
            let c = g.colour.max_element() * (1.0 - g.to_ambient);
            if best.as_ref().is_none_or(|b| c > b.1) {
                best = Some((map.clone(), c));
            }
        }
    }
    let (map, _) = best.expect("a world with a global light");
    let mut lum = Vec::new();
    let mut bare = None;
    for sun in [false, true] {
        let mut req = character_request(&map, &table);
        if let Some(m) = req.model.as_mut() {
            m.world_sun = sun;
        }
        let p = v.prepare(&req).expect("prepare");
        let rep = p.report.model.clone().unwrap();
        if bare.is_none() {
            bare = Some(
                v.renderer
                    .render_frame_offscreen(
                        &v.ctx,
                        &p.scene,
                        &p.camera,
                        &p.frame,
                        &[],
                        req.width,
                        req.height,
                    )
                    .unwrap()
                    .0,
            );
        }
        let (img, _) = v.render_prepared(&p, req.width, req.height).unwrap();
        img.save_png(&default_out_dir().join(format!(
            "{}_sun_{}.png",
            map.to_ascii_lowercase(),
            if sun { "world" } else { "off" }
        )))
        .unwrap();
        let (n, l) = coverage(&img, bare.as_ref().unwrap());
        println!(
            "{map} sun {sun}: report sun {:?} from world {}, lights {}, model pixels {n}, luminance {l:.1}",
            rep.sun, rep.sun_from_world, rep.lights
        );
        if sun {
            let (d, c, _) = rep.sun.expect("the world's sun is the default");
            assert!(rep.sun_from_world);
            assert!(d[1] < -0.3 && c.iter().any(|&x| x > 20.0));
        } else {
            assert!(rep.sun.is_none());
        }
        lum.push(l);
    }
    assert!(
        lum[1] > lum[0],
        "{map}: the world sun does not light the model"
    );
}

#[test]
fn rom_lod_selection_at_three_distances() {
    let rom = dc_rom::rom_or_skip!();
    let Some(v) = viewer(&rom) else {
        return;
    };
    let table = CharacterTable::from_vfs(v.vfs()).expect("character table");
    let map = v.map_names().into_iter().next().unwrap();
    let mut tris = Vec::new();
    for (d, want) in [(100.0f32, 0usize), (500.0, 1), (1000.0, 2)] {
        let mut req = character_request(&map, &table);
        if let Some(m) = req.model.as_mut() {
            m.lod_distance = Some(d);
        }
        let p = v.prepare(&req).expect("prepare");
        let rep = p.report.model.clone().unwrap();
        println!(
            "distance {d}: LODs {:?}, triangles {} (LOD totals {:?})",
            rep.lods, rep.triangles_drawn, p.models[0].lod_triangles
        );
        // SPEC: FMT_LTB.md §5 (the ROM's 3-LOD pieces switch at 400 and 800, style_census)
        assert!(!rep.lods.is_empty());
        assert!(rep.lods.iter().all(|&(_, l)| l == want), "distance {d}");
        let (img, fr) = v.render_prepared(&p, req.width, req.height).unwrap();
        img.save_png(
            &default_out_dir().join(format!("{}_lod{want}.png", map.to_ascii_lowercase())),
        )
        .unwrap();
        assert_eq!(fr.models, p.models.len() as u64);
        tris.push(rep.triangles_drawn);
    }
    assert!(
        tris[0] > tris[1] && tris[1] > tris[2] && tris[2] > 0,
        "{tris:?}"
    );
}

#[test]
fn rom_weapon_attached_at_the_hand_socket() {
    let rom = dc_rom::rom_or_skip!();
    let Some(v) = viewer(&rom) else {
        return;
    };
    let table = CharacterTable::from_vfs(v.vfs()).expect("character table");
    let hands = HandModels::from_vfs(v.vfs()).expect("weapon table");
    let (_, hand) = hands.rows.iter().next().expect("a weapon row");
    let map = v.map_names().into_iter().next().unwrap();
    let mut req = character_request(&map, &table);
    let no_weapon = v.prepare(&req).expect("prepare");
    if let Some(m) = req.model.as_mut() {
        m.weapon = Some(WeaponRequest {
            path: hand.model.clone(),
            textures: hand.skins.iter().take(1).cloned().collect(),
            styles: hand.styles.clone(),
            socket: None,
        });
    }
    let p = v.prepare(&req).expect("prepare");
    let w = p
        .report
        .model
        .clone()
        .unwrap()
        .weapon
        .expect("weapon report");
    println!(
        "weapon {}: socket {} node {} at {:?}, node bounds {:?}, weapon bounds {:?}, styles {:?}",
        w.path_hash, w.socket, w.node, w.socket_at, w.node_bounds, w.weapon_bounds, w.styles
    );
    // SPEC: UDP_SESSION_2009_B.md §3.5.1 (socket 2 in every ROM body that has the grip socket)
    assert_eq!(w.socket, WEAPON_SOCKET_INDEX);
    assert_eq!(w.missing_textures, 0);
    assert_eq!(p.models.len(), 3, "body, face, weapon");
    // the socket lies within the bounds of the hand node's vertices (2 units of slack for the skin surface)
    let (lo, hi) = w.node_bounds.expect("the socket's node owns vertices");
    let s = w.socket_at;
    for k in 0..3 {
        assert!(
            s[k] >= lo[k] - 2.0 && s[k] <= hi[k] + 2.0,
            "socket {s:?} outside the hand bounds {lo:?}..{hi:?}"
        );
    }
    // and the weapon model overlaps the hand
    let (wl, wh) = w.weapon_bounds.expect("weapon bounds");
    assert!((0..3).all(|k| wl[k] <= hi[k] + 2.0 && wh[k] >= lo[k] - 2.0));
    let (a, _) = v
        .render_prepared(&no_weapon, req.width, req.height)
        .unwrap();
    let (b, fr) = v.render_prepared(&p, req.width, req.height).unwrap();
    b.save_png(&default_out_dir().join(format!("{}_weapon.png", map.to_ascii_lowercase())))
        .unwrap();
    let changed = diff_pixels(&a, &b);
    println!("weapon pixels: {changed}, models drawn {}", fr.models);
    assert_eq!(fr.models, 3);
    assert!(changed > 20, "the weapon is not visible ({changed} pixels)");
}
