//! What the first real map found (DC-DEFAULT: MAP-91 .. MAP-95): flat terrain, the direction of a wedge, running a
//! generator twice, surface names, and the textured picture.

use dc_mapkit::diag::{Code, Severity};
use dc_mapkit::geom::{solid_polys, Surface};
use dc_mapkit::ops::{apply, Op, Preset, SolidSpec, WedgeRise};
use dc_mapkit::project::{Heights, Project, Solid};
use dc_mapkit::render_gpu::{draw_textured, TexturedRequest};
use dc_mapkit::render_sw::View;
use dc_mapkit::romlist::{self, RomKind, RomQuery};
use dc_mapkit::script::{self, RunOptions};
use dc_mapkit::template::{make, Made, Template};
use dc_mapkit::validate::validate;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn scaffold(t: Template, tag: &str) -> (PathBuf, Made) {
    let dir = std::env::temp_dir().join(format!(
        "dc-mapkit-fixes-{}-{tag}-{}",
        t.name(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let made = make(t, "Fix Test", "tester", 11);
    for f in &made.files {
        let path = Project::resolve(&dir, &f.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &f.bytes).unwrap();
    }
    made.project.save_all(&dir, &made.heights).unwrap();
    (dir, made)
}

fn flat_op(preset: Preset, amplitude: f32) -> Op {
    Op::TerrainGen {
        preset,
        seed: None,
        amplitude,
        base: 24.0,
        scale: 1024.0,
        octaves: 5,
        cells_x: Some(8),
        cells_z: Some(6),
        cell_size: Some(64.0),
    }
}

#[test]
fn a_flat_terrain_is_level_at_its_base_and_can_still_be_raised() {
    for op in [flat_op(Preset::Flat, 256.0), flat_op(Preset::Hills, 0.0)] {
        let made = make(Template::Flat, "Flat", "tester", 11);
        let (mut p, mut h) = (made.project, made.heights);
        apply(&mut p, &mut h, &[op]).unwrap_or_else(|d| panic!("{:?}", d.items));
        let t = p.terrain.clone().unwrap();
        assert_eq!((t.cells_x, t.cells_z), (8, 6));
        for (x, z) in [(0.0, 0.0), (100.0, -100.0), (-200.0, 150.0)] {
            let y = h.sample(&t, x, z).unwrap();
            assert!((y - 24.0).abs() < 0.05, "{y} at {x},{z}");
        }
        apply(
            &mut p,
            &mut h,
            &[Op::TerrainRaise {
                x: 0.0,
                z: 0.0,
                radius: 128.0,
                amount: 64.0,
                falloff: Default::default(),
            }],
        )
        .unwrap();
        assert!(h.sample(&t, 0.0, 0.0).unwrap() > 80.0);
    }
    let made = make(Template::Flat, "Flat", "tester", 11);
    let (mut p, mut h) = (made.project, made.heights);
    let e = apply(&mut p, &mut h, &[flat_op(Preset::Hills, -1.0)]).unwrap_err();
    assert_eq!(e.items[0].code, Code::OpInvalid);
}

fn wedge(rises: Option<WedgeRise>, kind: SolidSpec, rot_y_deg: f32) -> Op {
    Op::SolidAdd {
        kind,
        pos: [100.0, 50.0, -100.0],
        rot_y_deg,
        size: [128.0, 100.0, 200.0],
        material: "ground".into(),
        faces: BTreeMap::new(),
        group: None,
        brush: None,
        ground: false,
        rises,
    }
}

/// The mean x / z of a solid's highest corners, from its centre: the way its top leans.
fn high_side(s: &Solid) -> (f32, f32) {
    let polys = solid_polys(s, &|_| Surface::default()).unwrap();
    let corners: Vec<_> = polys
        .iter()
        .flat_map(|p| p.corners.iter().copied())
        .collect();
    let top = corners.iter().map(|c| c.y).fold(f32::MIN, f32::max);
    let high: Vec<_> = corners.iter().filter(|c| c.y > top - 0.01).collect();
    let n = high.len() as f32;
    (
        high.iter().map(|c| c.x).sum::<f32>() / n - s.pos[0],
        high.iter().map(|c| c.z).sum::<f32>() / n - s.pos[2],
    )
}

#[test]
fn rises_names_the_side_a_wedge_climbs_toward() {
    for (rise, want) in [
        (WedgeRise::North, (0.0, 100.0)),
        (WedgeRise::East, (100.0, 0.0)),
        (WedgeRise::South, (0.0, -100.0)),
        (WedgeRise::West, (-100.0, 0.0)),
    ] {
        let made = make(Template::Flat, "W", "tester", 11);
        let (mut p, mut h) = (made.project, made.heights);
        apply(&mut p, &mut h, &[wedge(Some(rise), SolidSpec::Wedge, 0.0)]).unwrap();
        let s = p.solids.last().unwrap();
        let (x, z) = high_side(s);
        assert!(
            (x - want.0).abs() < 0.01 && (z - want.1).abs() < 0.01,
            "{rise:?}: high side at {x},{z}"
        );
        // solid.set turns an existing wedge the same way
        let id = s.id.clone();
        apply(
            &mut p,
            &mut h,
            &[Op::SolidSet {
                id: id.clone(),
                pos: None,
                rot_y_deg: Some(33.0),
                size: None,
                material: None,
                faces: None,
                group: None,
                brush: None,
                rises: None,
            }],
        )
        .unwrap();
        apply(
            &mut p,
            &mut h,
            &[Op::SolidSet {
                id: id.clone(),
                pos: None,
                rot_y_deg: None,
                size: None,
                material: None,
                faces: None,
                group: None,
                brush: None,
                rises: Some(rise),
            }],
        )
        .unwrap();
        let (x, z) = high_side(p.solid(&id).unwrap());
        assert!((x - want.0).abs() < 0.01 && (z - want.1).abs() < 0.01);
    }
}

#[test]
fn rises_is_refused_on_a_box_and_next_to_a_rotation() {
    let made = make(Template::Flat, "W", "tester", 11);
    let (mut p, mut h) = (made.project, made.heights);
    for op in [
        wedge(Some(WedgeRise::East), SolidSpec::Box, 0.0),
        wedge(Some(WedgeRise::East), SolidSpec::Wedge, 45.0),
    ] {
        let e = apply(&mut p, &mut h, &[op]).unwrap_err();
        assert_eq!(e.items[0].code, Code::OpInvalid);
    }
}

const GENERATOR: &str = r#"
map.clear{ group = "mine" }
map.solid.box{ pos = { 0, 40, 0 }, size = { 64, 80, 64 }, material = "ground", group = "mine" }
map.solid.wedge{ pos = { 200, 40, 0 }, size = { 64, 80, 128 }, material = "ground", rises = "east", group = "mine" }
"#;

fn run_script(p: &mut Project, h: &mut Heights, text: &str) {
    let g = script::generate(p, h, text, &RunOptions::default())
        .unwrap_or_else(|d| panic!("{:?}", d.items));
    apply(p, h, &g.ops).unwrap_or_else(|d| panic!("{:?}", d.items));
}

fn in_group(p: &Project, g: &str) -> usize {
    p.solids
        .iter()
        .filter(|s| s.group.as_deref() == Some(g))
        .count()
}

#[test]
fn a_generator_that_clears_its_group_runs_again_without_doubling() {
    let (dir, made) = scaffold(Template::Flat, "again");
    let (mut p, mut h) = (made.project, made.heights);
    let others = p.solids.len();
    run_script(&mut p, &mut h, GENERATOR);
    run_script(&mut p, &mut h, GENERATOR);
    assert_eq!(in_group(&p, "mine"), 2);
    assert_eq!(p.solids.len(), others + 2);
    let d = validate(&dir, &p, &h);
    assert!(!d.items.iter().any(|x| x.code == Code::SolidDuplicate));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn running_a_generator_twice_without_a_clear_is_a_warning() {
    let (dir, made) = scaffold(Template::Flat, "twice");
    let (mut p, mut h) = (made.project, made.heights);
    let text = GENERATOR.replace("map.clear{ group = \"mine\" }", "");
    run_script(&mut p, &mut h, &text);
    assert!(!validate(&dir, &p, &h)
        .items
        .iter()
        .any(|x| x.code == Code::SolidDuplicate));
    run_script(&mut p, &mut h, &text);
    let d = validate(&dir, &p, &h);
    let hit = d
        .items
        .iter()
        .find(|x| x.code == Code::SolidDuplicate)
        .expect("the repeat is reported");
    assert_eq!(hit.severity, Severity::Warning);
    assert!(hit.message.starts_with("2 solids"), "{}", hit.message);
    assert!(hit.fix.as_deref().is_some_and(|f| f.contains("map.clear")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clear_takes_everything_or_only_what_it_names() {
    let made = make(Template::Arena, "C", "tester", 11);
    let (mut p, mut h) = (made.project, made.heights);
    let materials = p.materials.len();
    let spawns = dc_mapkit::vocab_names::CLASS_PLAYER_SPAWN;
    let objects = p.objects.len();
    let of_class = p.objects.iter().filter(|o| o.class == spawns).count();
    assert!(of_class > 0);
    apply(
        &mut p,
        &mut h,
        &[Op::Clear {
            group: None,
            class: Some(spawns.into()),
        }],
    )
    .unwrap();
    assert_eq!(p.objects.len(), objects - of_class);
    assert!(!p.solids.is_empty());
    let a = apply(
        &mut p,
        &mut h,
        &[Op::Clear {
            group: None,
            class: None,
        }],
    )
    .unwrap();
    assert!(!a.applied[0].ids.is_empty());
    assert!(p.objects.is_empty() && p.solids.is_empty());
    assert_eq!(p.materials.len(), materials);
    assert!(p.terrain.is_some());
    // an emptied project starts its ids over: the same generator gets the same ids
    assert_eq!(p.alloc_id("solid"), "solid_1");
}

#[test]
fn surface_rows_carry_their_rom_name_and_filter_on_it() {
    let Some((rom, _)) = romlist::discover_rom(None) else {
        eprintln!("no ROM: skipped");
        return;
    };
    let query = |filter: Option<String>| RomQuery {
        kind: RomKind::Surfaces,
        filter,
        thumbs: None,
    };
    let rows = romlist::list(&rom, &query(None)).unwrap();
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|r| r.id.is_some()));
    let named = rows.iter().find_map(|r| r.name.clone()).expect("a name");
    assert!(rows.iter().filter(|r| r.name.is_some()).count() > rows.len() / 2);
    let hits = romlist::list(&rom, &query(Some(named.clone()))).unwrap();
    assert!(hits.iter().any(|r| r.name.as_deref() == Some(&named)));
}

#[test]
fn a_textured_picture_comes_from_the_game_renderer() {
    let binaries = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    if dc_mapkit::play::find_binary(Some(&binaries), "dc-client").is_err() {
        eprintln!("no dc-client built: skipped");
        return;
    }
    let (dir, _made) = scaffold(Template::Arena, "textured");
    let request = |view: &str| TexturedRequest {
        dir: &dir,
        view: View::parse(view).unwrap(),
        size: (160, 120),
        out: None,
        binaries: Some(&binaries),
        rom: None,
    };
    let t = match draw_textured(&request("spawn:0")) {
        Ok(t) => t,
        Err(d) if d.items.iter().any(|x| x.message.contains("adapter")) => {
            eprintln!("no GPU: skipped");
            return;
        }
        Err(d) => panic!("{:?}", d.items),
    };
    assert!(t.png.starts_with(b"\x89PNG"));
    let image = dir.join("build/render/spawn_0_textured.png");
    assert_eq!(std::fs::read(&image).unwrap(), t.png);
    assert_eq!(t.outcome.result["textured"], true);
    let e = draw_textured(&request("spawn:9999")).err().unwrap();
    assert_eq!(e.items[0].code, Code::OpInvalid);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fit_puts_the_region_once_across_a_face_unmirrored_from_outside() {
    use dc_mapkit::geom::V3;
    let solid = Solid {
        pos: [0.0, 50.0, 0.0],
        size: [40.0, 100.0, 200.0],
        rot_y_deg: 0.0,
        faces: BTreeMap::new(),
        ..Default::default()
    };
    let region = [0.25, 0.5, 0.75, 1.0];
    let polys = solid_polys(&solid, &|_| Surface {
        fit: Some(region),
        ..Default::default()
    })
    .unwrap();
    for p in &polys {
        let uvs: Vec<[f32; 2]> = p.corners.iter().map(|c| p.uv(*c)).collect();
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for uv in &uvs {
            for i in 0..2 {
                lo[i] = lo[i].min(uv[i]);
                hi[i] = hi[i].max(uv[i]);
            }
        }
        for (got, want) in [lo[0], lo[1], hi[0], hi[1]].iter().zip(region) {
            assert!((got - want).abs() < 1e-4, "{uvs:?}");
        }
    }
    // the +x side, seen from +x looking toward -x with +y up: the viewer's right is -z (left-handed axes), and v
    // runs down
    let east = polys.iter().find(|p| p.plane.n.x > 0.9).unwrap();
    let at = |z: f32, y: f32| east.uv(V3::new(20.0, y, z));
    assert!(
        (at(100.0, 100.0)[0] - 0.75).abs() < 1e-4 || (at(-100.0, 100.0)[0] - 0.75).abs() < 1e-4
    );
    assert!(at(0.0, 100.0)[1] < at(0.0, 0.0)[1], "v runs down the face");
    // a yawed solid keeps the region on its own faces
    let turned = Solid {
        rot_y_deg: 33.0,
        ..solid
    };
    let polys = solid_polys(&turned, &|_| Surface {
        fit: Some(region),
        ..Default::default()
    })
    .unwrap();
    let top = polys.iter().find(|p| p.plane.n.y > 0.9).unwrap();
    let us: Vec<f32> = top.corners.iter().map(|c| top.uv(*c)[0]).collect();
    assert!(
        us.iter().any(|u| (u - 0.25).abs() < 1e-4) && us.iter().any(|u| (u - 0.75).abs() < 1e-4)
    );
}

#[test]
fn an_empty_fit_is_refused() {
    let made = make(Template::Flat, "F", "tester", 11);
    let (mut p, mut h) = (made.project, made.heights);
    let e = apply(
        &mut p,
        &mut h,
        &[Op::MaterialSet {
            id: "ground".into(),
            texture: None,
            tile: None,
            surface_id: None,
            flags: None,
            fit: Some([0.5, 0.0, 0.5, 1.0]),
        }],
    )
    .unwrap_err();
    assert_eq!(e.items[0].code, Code::OpInvalid);
}

#[test]
fn a_generator_can_set_the_world_settings() {
    let made = make(Template::Flat, "S", "tester", 11);
    let (mut p, mut h) = (made.project, made.heights);
    run_script(
        &mut p,
        &mut h,
        "map.settings{ fog_near = 1234, ambient = { 1, 2, 3 } }",
    );
    assert_eq!(p.settings.fog_near, 1234.0);
    assert_eq!(p.settings.ambient, [1, 2, 3]);
}

#[test]
fn a_tiled_texture_turns_with_its_solid() {
    let solid = |rot: f32| Solid {
        pos: [300.0, 2.0, -40.0],
        size: [120.0, 4.0, 320.0],
        rot_y_deg: rot,
        ..Default::default()
    };
    let tile = |_: &str| Surface {
        uv_scale: 128.0,
        ..Default::default()
    };
    // the floor's corners have the same texture coordinates whichever way the solid is turned, about its centre
    let corners_uv = |rot: f32| {
        let polys = solid_polys(&solid(rot), &tile).unwrap();
        let top = polys.iter().find(|p| p.plane.n.y > 0.9).unwrap().clone();
        let mut uvs: Vec<[i32; 2]> = top
            .corners
            .iter()
            .map(|c| {
                let uv = top.uv(*c);
                [
                    (uv[0] * 1000.0).round() as i32,
                    (uv[1] * 1000.0).round() as i32,
                ]
            })
            .collect();
        uvs.sort();
        uvs
    };
    assert_eq!(corners_uv(24.1), corners_uv(-11.6));
    assert_eq!(corners_uv(24.1), corners_uv(0.4));
    // a straight edge along the solid keeps one u, which a slanted texture would not
    let polys = solid_polys(&solid(24.1), &tile).unwrap();
    let top = polys.iter().find(|p| p.plane.n.y > 0.9).unwrap();
    let us: Vec<f32> = top.corners.iter().map(|c| top.uv(*c)[0]).collect();
    let distinct = us.iter().filter(|u| (**u - us[0]).abs() < 1e-3).count();
    assert_eq!(distinct, 2, "{us:?}");
    // an unturned solid keeps the world projection
    let polys = solid_polys(&solid(0.0), &tile).unwrap();
    assert!(polys.iter().all(|p| p.uv_frame.is_none()));
}

#[test]
fn a_map_with_rom_textures_gets_plain_pictures_without_the_client() {
    use dc_mapkit::project::TextureRef;
    use dc_mapkit::render_gpu::{pictures, PicturesRequest, LOADING_SIZE, PREVIEW_SIZE};
    let (dir, made) = scaffold(Template::Flat, "cards");
    let (mut p, h) = (made.project, made.heights);
    p.materials[0].texture = TextureRef::Rom("TEXTURES\\TEST.DTX".into());
    p.save_all(&dir, &h).unwrap();
    // no binaries folder and no ROM: a render would fail, so this proves nothing was rendered
    let missing = dir.join("no-such-folder");
    let o = pictures(&PicturesRequest {
        dir: &dir,
        preview: View::parse("spawn:0").unwrap(),
        loading: View::parse("spawn:0").unwrap(),
        binaries: Some(&missing),
        rom: Some(&missing),
    })
    .unwrap_or_else(|d| panic!("{:?}", d.items));
    assert_eq!(o.result["kind"], "card");
    for (suffix, size) in [("", (PREVIEW_SIZE, PREVIEW_SIZE)), ("_load", LOADING_SIZE)] {
        let file = dir.join(format!("assets/pictures/{}{suffix}.png", p.key));
        let img = image::open(file).unwrap();
        assert_eq!((img.width(), img.height()), size);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
