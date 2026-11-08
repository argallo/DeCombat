//! The build pipeline over the three templates (MAPKIT_PLAN.md 5.3; DC-DEFAULT: MAP-43 .. MAP-48): every
//! template builds, parses back with a tiling layout, passes the world-level checks, stands its spawns on the
//! floor through dc-sim, sees the sky, carries its zones and brushes, builds the same bytes twice, and yields
//! the expected batches through dc-render's CPU scene build. No ROM, no GPU.

use dc_mapkit::build::{build, Assets, Built};
use dc_mapkit::diag::{Code, Severity};
use dc_mapkit::project::{BrushRole, Project};
use dc_mapkit::template::{make, Made, Template, ZONE_EXTENT};
use dc_mapkit::validate::{check_world, FLOOR_LIFT, STAND_HALF_HEIGHT, STAND_RADIUS};
use dc_mapkit::worldgen::{BSP_FLAG_BRUSH, LEVEL_MODEL_NAME, SURFACE_SKY};
use dc_render::{SceneMesh, SceneOptions, TextureTraits};
use dc_sim::from_world::{collision_world, WorldFilter};
use dc_sim::{Collider, Cylinder, V3};
use dc_world::synth::{SynthModel, SynthObject, SynthValue, SynthWorld};
use dc_world::{FieldValue, ObjectRole, World};
use std::path::{Path, PathBuf};

/// Scaffold a template into a scratch folder (the own PNGs must exist for the project checks).
fn scaffold(t: Template, tag: &str) -> (PathBuf, Made) {
    let dir = std::env::temp_dir().join(format!(
        "dc-mapkit-build-{}-{tag}-{}",
        t.name(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let made = make(t, "Build Test", "tester", 11);
    for f in &made.files {
        let path = Project::resolve(&dir, &f.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &f.bytes).unwrap();
    }
    made.project.save_all(&dir, &made.heights).unwrap();
    (dir, made)
}

fn build_made(dir: &Path, made: &Made) -> Built {
    build(
        &made.project,
        &made.heights,
        &Assets::new(dir),
        &made.project.build,
    )
    .unwrap_or_else(|e| panic!("build failed: {:?}", e.items))
}

fn stand() -> Cylinder {
    Cylinder::new(STAND_RADIUS, STAND_HALF_HEIGHT)
}

#[test]
fn every_template_builds_parses_and_passes_the_world_checks() {
    for t in Template::ALL {
        let (dir, made) = scaffold(t, "all");
        let b = build_made(&dir, &made);
        let errors: Vec<_> = b
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{}: {errors:?}", t.name());
        assert!(
            !b.diagnostics
                .iter()
                .any(|d| matches!(d.code, Code::SpawnFloor | Code::SpawnClear)),
            "{}: {:?}",
            t.name(),
            b.diagnostics.items
        );
        let w = World::parse(&b.dat).unwrap();
        w.check_layout().unwrap();
        assert!(check_world(&b.dat).count(Severity::Error) == 0);

        // stats
        let s = &b.stats;
        assert_eq!(s.bytes, b.dat.len());
        assert!(s.polys > 8000, "{}: {s:?}", t.name());
        assert!(s.nodes >= s.polys, "{}: {s:?}", t.name());
        assert!(s.depth > 0 && s.depth < 200, "{}: {s:?}", t.name());
        assert_eq!(s.brushes, 2);
        assert_eq!(s.sky_faces, 6);
        assert_eq!(s.textures, 4);
        // corners equal in every field share a vertex (MAP-104)
        assert!(s.sections >= 3 && s.vertices >= 3 && s.vertices < s.triangles * 3);
        let ws = w.stats();
        assert_eq!(ws.bsp_nodes as usize, s.nodes);
        assert_eq!(ws.render.triangles as usize, s.triangles);
        assert_eq!(ws.render.vertices as usize, s.vertices);
        assert_eq!(ws.render.sections as usize, s.sections);
        assert_eq!(ws.light_grid, [1, 1, 1]);
        assert_eq!(w.info_text, b"decombat mapkit v1");

        // the level is the one unnamed model, flag bit 1 clear; every brush links by name with origin = pos
        let unnamed: Vec<_> = w
            .models
            .iter()
            .filter(|m| w.object_named(m.name).is_none())
            .collect();
        assert_eq!(unnamed.len(), 1);
        assert_eq!(unnamed[0].name, LEVEL_MODEL_NAME.as_bytes());
        assert_eq!(unnamed[0].flags & BSP_FLAG_BRUSH, 0);
        // every sky face carries the client's sky surface id as texflags (C4; MAP-43), whatever the
        // material's surface_id (0 in every template)
        let sky_polys = unnamed[0]
            .polygons
            .iter()
            .filter(|p| {
                unnamed[0]
                    .surfaces
                    .get(p.surface as usize)
                    .is_some_and(|s| s.flags & 0x10 != 0)
            })
            .count();
        assert_eq!(sky_polys, 6, "{}", t.name());
        for m in &w.models {
            for p in &m.polygons {
                let s = m.surfaces[p.surface as usize];
                if s.flags & 0x10 != 0 {
                    assert_eq!(s.texflags, SURFACE_SKY, "{}: {:?}", t.name(), s);
                }
            }
        }
        // every brush but a blocker (MAP-124: an invisible wall, no model and no object) names its model
        for solid in made
            .project
            .solids
            .iter()
            .filter(|s| s.brush.is_some() && s.brush != Some(BrushRole::Blocker))
        {
            let o = w.object_named(solid.id.as_bytes()).expect("brush object");
            let m = w.brush_of(o).expect("brush model");
            assert_eq!(m.flags & BSP_FLAG_BRUSH, BSP_FLAG_BRUSH);
            assert_eq!(m.origin, solid.pos);
            assert_eq!(o.position(), Some(solid.pos));
            let role = match solid.brush.unwrap() {
                BrushRole::KillVolume => ObjectRole::KillVolume,
                BrushRole::Sky => ObjectRole::SkyWorldModel,
                BrushRole::Ladder => ObjectRole::Ladder,
                BrushRole::WorldModel => ObjectRole::PlainWorldModel,
                BrushRole::Breakable => ObjectRole::Breakable,
                BrushRole::Blocker => unreachable!("filtered above"),
            };
            assert_eq!(o.role, Some(role), "{}", solid.id);
        }
        assert_eq!(w.objects_with_role(ObjectRole::WorldSettings).count(), 1);
        assert_eq!(w.objects_of_class(b"dc.global_light").count(), 1);

        // spawns stand on the floor and see the sky (dc-sim over the built world)
        let cw = collision_world(&w, &WorldFilter::default());
        let shape = stand();
        let mut spawns = 0;
        for o in w.objects_with_role(ObjectRole::PlayerSpawn) {
            spawns += 1;
            let p = o.position().unwrap();
            let at = V3::new(p[0], p[1], p[2]);
            // a ray from the centre down meets the floor about half a body below (the ground snap put it there)
            let reach = STAND_HALF_HEIGHT + 9.0;
            let hit = cw
                .ray(at, V3::new(at.x, at.y - reach, at.z))
                .unwrap_or_else(|| panic!("{}: {} has no floor", t.name(), o.at));
            let drop = hit.t * reach;
            assert!(
                (30.0..=reach).contains(&drop),
                "{}: floor {drop} below",
                t.name()
            );
            // the server's body (C15: lifted by the server's SPAWN_LIFT) sweeps down onto it within 8 units
            let hit = cw
                .sweep(
                    &shape,
                    at + V3::new(0.0, FLOOR_LIFT, 0.0),
                    V3::new(at.x, at.y - 8.0, at.z),
                )
                .expect("standing box meets the floor");
            assert!(hit.t > 0.0);
            // straight up, the sky face (solid, 0x10) stops the ray
            let up = cw
                .ray(at, V3::new(at.x, at.y + 8192.0, at.z))
                .expect("a sky face above");
            assert_ne!(
                up.surface_flags & 0x10,
                0,
                "{}: flags {:x}",
                t.name(),
                up.surface_flags
            );
            assert_ne!(up.surface_flags & 0x1, 0);
        }
        assert_eq!(spawns, 32);

        // zones with HALF extents (flags 0x4), bomb sites numbered 0 and 1, flags per team
        let mut numbers = Vec::new();
        for o in w.objects_with_role(ObjectRole::BombSite) {
            let e = o.prop_named(&dc_world::names::PROP_EXTENT).expect("extent");
            assert_eq!(e.flags, 0x4);
            assert_eq!(e.value, FieldValue::Vec3(ZONE_EXTENT));
            match o.prop_named(&dc_world::names::PROP_NUMBER).unwrap().value {
                FieldValue::Int(n) => numbers.push(n),
                v => panic!("{v:?}"),
            }
        }
        numbers.sort_unstable();
        assert_eq!(numbers, [0, 1]);
        assert_eq!(w.objects_with_role(ObjectRole::FlagZone).count(), 2);
        assert_eq!(w.objects_with_role(ObjectRole::CaptureFlag).count(), 2);
        for o in w.objects_with_role(ObjectRole::FlagZone) {
            let e = o.vec3(dc_world::PropKey::BoxExtent).expect("zone extent");
            assert_eq!(e, ZONE_EXTENT);
        }

        // determinism: the same project builds the same bytes
        let again = build_made(&dir, &made);
        assert_eq!(
            again.dat,
            b.dat,
            "{}: bytes differ between builds",
            t.name()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn the_render_data_batches_by_texture_and_carries_the_sky() {
    let (dir, made) = scaffold(Template::Arena, "render");
    let b = build_made(&dir, &made);
    let w = World::parse(&b.dat).unwrap();
    let mut asked: Vec<Vec<u8>> = Vec::new();
    let mesh = SceneMesh::build(&w, SceneOptions::default(), &mut |name: &[u8]| {
        asked.push(name.to_vec());
        TextureTraits {
            present: true,
            mode: 0,
            hi: false,
        }
    });
    let key = &made.project.key;
    let names: Vec<String> = mesh
        .textures
        .iter()
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .collect();
    for want in ["ground", "wall", "block", "sky"] {
        let full = format!("@mod:{key}/assets/{want}.dtx");
        assert!(names.contains(&full), "{full} missing in {names:?}");
    }
    assert_eq!(mesh.textures.len(), 4);
    assert!(!asked.is_empty());
    assert!(asked.iter().all(|n| n.starts_with(b"@mod:")));
    // the level block draws three main batches (ground, wall, block), no sky face among them
    assert_eq!(mesh.batches.len(), 3, "{:?}", mesh.batches);
    assert_eq!(mesh.stats.sections_hidden, 0);
    assert_eq!(mesh.stats.bad_ranges, 0);
    assert_eq!(
        mesh.stats.world_models, 0,
        "kill volume and level only: nothing drawn as a world model"
    );
    // sky: six portals from the lid's faces, one sky model with one section, drawn through the sky camera
    assert_eq!(mesh.stats.sky_portals, 6);
    assert_eq!(mesh.sky_portals.len(), 6);
    assert!(mesh.sky_portals.iter().all(|p| p.len() == 4));
    assert_eq!(mesh.stats.sky_models, 1);
    assert_eq!(mesh.sky_batches.len(), 1);
    assert_eq!(mesh.sky_models.len(), 1);
    let sky_tex = mesh.sky_batches[0].tex0.unwrap() as usize;
    assert_eq!(names[sky_tex], format!("@mod:{key}/assets/sky.dtx"));
    assert_eq!(mesh.sky_bounds.unwrap().1[0], 512.0);
    assert_eq!(
        mesh.stats.triangles_drawn as usize, b.stats.triangles,
        "every written triangle is drawn (main + sky)"
    );
    // the fog and the sun come from the generated settings objects (read through dc-world: dc-render's fog and
    // global-light readers learn the dc.* names in the render slice of this stage)
    let ws = w
        .objects_with_role(ObjectRole::WorldSettings)
        .next()
        .expect("world settings");
    assert_eq!(prop(ws, b"fog_enable"), Some(FieldValue::Bool(true)));
    assert_eq!(
        prop(ws, b"fog_near"),
        Some(FieldValue::Real(made.project.settings.fog_near))
    );
    assert_eq!(
        prop(ws, b"far_z"),
        Some(FieldValue::Real(made.project.settings.far_z))
    );
    // one-sided unless the project asks (MAP-118): no property, and the scene keeps culling back faces
    assert_eq!(prop(ws, b"two_sided"), None);
    assert!(!mesh.two_sided);
    let sun = w.objects_of_class(b"dc.global_light").next().expect("sun");
    let sun_dir = dc_mapkit::worldgen::rotated_z(sun.rotation().unwrap());
    let want = glam_dir(made.project.settings.sun_dir);
    for k in 0..3 {
        assert!(
            (sun_dir[k] - want[k]).abs() < 1e-4,
            "{sun_dir:?} vs {want:?}"
        );
    }
    assert_eq!(
        prop(sun, b"color"),
        Some(FieldValue::Rgb([255.0, 245.0, 225.0]))
    );
    // the light grid is the ambient colour everywhere
    let grid = dc_render::AmbientGrid::of(&w.light_grid);
    let a = made.project.settings.ambient;
    let c = grid.sample([10.0f32, 20.0, -300.0].into());
    assert_eq!([c.x, c.y, c.z], [a[0] as f32, a[1] as f32, a[2] as f32]);
    // vertex colours: alpha 0xFF, red channel in range
    let blk = &w.render.as_ref().unwrap().world.blocks[0];
    let v0 = dc_render::RenderVertex::read(blk.vertices, 0).unwrap();
    assert_eq!(v0.colour >> 24, 0xFF);
    assert!((v0.colour >> 16) & 0xFF > 0);
    assert_eq!(v0.uv1, [0.0, 0.0]);
    assert!(
        (v0.normal[0].powi(2) + v0.normal[1].powi(2) + v0.normal[2].powi(2) - 1.0).abs() < 1e-4
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// MAP-118: `settings.set {two_sided: true}` is kept in `map.json` (and left out while off), written into the
/// generated `dc.world_settings`, and read back by the renderer's scene build as a two-sided world.
#[test]
fn a_two_sided_project_builds_a_two_sided_world() {
    use dc_mapkit::ops::{apply, Op};
    let (dir, made) = scaffold(Template::Flat, "two-sided");
    let off = build_made(&dir, &made);
    let text = std::fs::read_to_string(dir.join("map.json")).unwrap();
    assert!(!text.contains("two_sided"), "off is left out of map.json");
    let (mut p, mut h) = (made.project.clone(), made.heights.clone());
    let op: Op = serde_json::from_str(r#"{"op": "settings.set", "two_sided": true}"#).unwrap();
    apply(&mut p, &mut h, &[op]).unwrap();
    assert!(p.settings.two_sided);
    p.save_all(&dir, &h).unwrap();
    let text = std::fs::read_to_string(dir.join("map.json")).unwrap();
    assert!(text.contains("\"two_sided\": true"));
    let (back, _) = Project::open(&dir).unwrap();
    assert!(back.settings.two_sided);
    let on = build(&p, &h, &Assets::new(&dir), &p.build).unwrap();
    let w = World::parse(&on.dat).unwrap();
    let ws = w
        .objects_with_role(ObjectRole::WorldSettings)
        .next()
        .expect("world settings");
    assert_eq!(prop(ws, b"two_sided"), Some(FieldValue::Bool(true)));
    assert!(dc_render::world_two_sided(&w));
    let traits = &mut |_: &[u8]| TextureTraits::default();
    assert!(SceneMesh::build(&w, SceneOptions::default(), traits).two_sided);
    let w_off = World::parse(&off.dat).unwrap();
    assert!(!dc_render::world_two_sided(&w_off));
    assert!(!SceneMesh::build(&w_off, SceneOptions::default(), traits).two_sided);
    // the setting adds that one property and changes nothing else of the objects
    assert_eq!(w.objects.len(), w_off.objects.len());
    for (a, b) in w.objects.iter().zip(&w_off.objects) {
        let props = |o: &dc_world::Object<'_>| -> Vec<(Vec<u8>, String)> {
            o.props
                .iter()
                .filter(|p| p.name != b"two_sided")
                .map(|p| (p.name.to_vec(), format!("{:?}", p.value)))
                .collect()
        };
        assert_eq!((a.class_name, props(a)), (b.class_name, props(b)));
    }
    let ws_off = w_off
        .objects_with_role(ObjectRole::WorldSettings)
        .next()
        .unwrap();
    assert_eq!(ws.props.len(), ws_off.props.len() + 1);
    let _ = std::fs::remove_dir_all(&dir);
}

fn prop<'a>(o: &dc_world::Object<'a>, n: &[u8]) -> Option<FieldValue<'a>> {
    o.prop(n).map(|p| p.value)
}

fn glam_dir(d: [f32; 3]) -> [f32; 3] {
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    [d[0] / l, d[1] / l, d[2] / l]
}

/// A hand-made world with one fault per model / object, for the world-level codes.
fn synth(models: Vec<SynthModel>, objects: Vec<SynthObject>, render_models: Vec<&str>) -> Vec<u8> {
    use dc_world::synth::{SynthBlock, SynthRenderWorld};
    SynthWorld {
        info: b"test".to_vec(),
        extent_min: [-100.0; 3],
        extent_max: [100.0; 3],
        tree_shape: vec![false],
        models,
        objects,
        render: SynthRenderWorld {
            blocks: vec![SynthBlock::default()],
            models: render_models
                .iter()
                .map(|n| (n.as_bytes().to_vec(), SynthRenderWorld::default()))
                .collect(),
        },
        ..SynthWorld::default()
    }
    .build()
}

/// A closed unit box model (six quads, outward, one node per polygon in a chain of NEGATIVE children: every
/// other face of a convex solid lies behind a face's plane, so this is the naive BSP the walk expects and a
/// sweep from any side finds the face it meets).
fn box_model(name: &str, flags: u32, surf_flags: u32, lo: f32, hi: f32) -> SynthModel {
    let p = |x: f32, y: f32, z: f32| [x, y, z];
    let points = vec![
        p(lo, lo, lo),
        p(hi, lo, lo),
        p(hi, hi, lo),
        p(lo, hi, lo),
        p(lo, lo, hi),
        p(hi, lo, hi),
        p(hi, hi, hi),
        p(lo, hi, hi),
    ];
    // faces wound so cross(c1 - c0, c2 - c1) points outward
    let faces: [([f32; 3], f32, [u32; 4]); 6] = [
        ([0.0, 0.0, -1.0], -lo, [0, 3, 2, 1]),
        ([0.0, 0.0, 1.0], hi, [4, 5, 6, 7]),
        ([-1.0, 0.0, 0.0], -lo, [0, 4, 7, 3]),
        ([1.0, 0.0, 0.0], hi, [1, 2, 6, 5]),
        ([0.0, -1.0, 0.0], -lo, [0, 1, 5, 4]),
        ([0.0, 1.0, 0.0], hi, [3, 7, 6, 2]),
    ];
    let n = faces.len() as i32;
    SynthModel {
        flags,
        name: name.as_bytes().to_vec(),
        textures: vec![b"t".to_vec()],
        planes: faces.iter().map(|f| (f.0, f.1)).collect(),
        surfaces: vec![(surf_flags, 0, 0)],
        polys: faces
            .iter()
            .enumerate()
            .map(|(i, f)| (0, i as u32, f.2.to_vec()))
            .collect(),
        nodes: (0..n)
            .map(|i| (i as u32, 0, if i + 1 < n { i + 1 } else { -2 }, -1))
            .collect(),
        points,
        root: 0,
        box_min: [lo; 3],
        box_max: [hi; 3],
        origin: [0.0; 3],
    }
}

/// The model moved up by `dy` (points, planes with a y component, the box).
fn shift_y(mut m: SynthModel, dy: f32) -> SynthModel {
    for p in &mut m.points {
        p[1] += dy;
    }
    for (n, d) in &mut m.planes {
        *d += n[1] * dy;
    }
    m.box_min[1] += dy;
    m.box_max[1] += dy;
    m
}

fn obj(class: &str, name: &str, pos: [f32; 3], props: &[(&str, SynthValue)]) -> SynthObject {
    SynthObject::dc(class, name, pos, [0.0; 3], props)
}

fn codes(dat: &[u8]) -> Vec<Code> {
    check_world(dat).iter().map(|d| d.code).collect()
}

#[test]
fn the_world_level_checks_name_their_codes() {
    // a sound world: one level box (sky face on top would need a portal; none here), one kill volume
    let level = box_model(LEVEL_MODEL_NAME, 0, 0x1, -50.0, 50.0);
    let kv = box_model("kv_1", BSP_FLAG_BRUSH, 0x1, -5.0, 5.0);
    let sound = synth(
        vec![level.clone(), kv.clone()],
        vec![obj("dc.kill_volume", "kv_1", [0.0; 3], &[])],
        vec![],
    );
    assert!(codes(&sound).is_empty(), "{:?}", check_world(&sound).items);

    // two unnamed models, a named one without bit 1, an object named `level`
    let two = synth(
        vec![
            level.clone(),
            box_model("other", 0, 0x1, -5.0, 5.0),
            box_model("kv_1", 0, 1, -5.0, 5.0),
        ],
        vec![
            obj("dc.kill_volume", "kv_1", [0.0; 3], &[]),
            obj("dc.item_spawn", LEVEL_MODEL_NAME, [0.0; 3], &[]),
        ],
        vec![],
    );
    let c = codes(&two);
    assert!(
        c.iter().filter(|c| **c == Code::WorldLevel).count() >= 3,
        "{c:?}"
    );

    // a solid polygon no node references; a brush with a flag-less polygon
    let mut unref = level.clone();
    unref.nodes.truncate(3);
    unref.nodes[2].2 = -2;
    let mut helper = kv.clone();
    helper.surfaces.push((0, 0, 0));
    helper.polys.push((1, 0, vec![]));
    let bad = synth(
        vec![unref, helper],
        vec![obj("dc.kill_volume", "kv_1", [0.0; 3], &[])],
        vec![],
    );
    let c = codes(&bad);
    assert!(c.contains(&Code::BspUnreferenced), "{c:?}");
    assert!(c.contains(&Code::BrushHelper), "{c:?}");

    // an open kill volume, a non-convex ladder (a face turned inward), a bomb site numbered 5, a spawn in
    // the air, a spawn inside the level box's wall, a model outside the extents
    let mut open = kv.clone();
    open.polys.truncate(5);
    open.nodes.truncate(5);
    open.nodes[4].2 = -2;
    let mut ladder = box_model("ld_1", BSP_FLAG_BRUSH, 0x1, 10.0, 20.0);
    ladder.planes[5] = ([0.0, -1.0, 0.0], -20.0);
    let mut far = box_model("wm_1", BSP_FLAG_BRUSH, 0x1, 150.0, 160.0);
    far.box_max = [160.0; 3];
    far.box_min = [150.0; 3];
    // a solid world-model box at y 85..95 through the standing body of the spawn on the level's top (y 50 + 40)
    let blocker = shift_y(box_model("wm_2", BSP_FLAG_BRUSH, 0x1, -5.0, 5.0), 90.0);
    let faulty = synth(
        vec![level.clone(), open, ladder, far, blocker],
        vec![
            obj("dc.kill_volume", "kv_1", [0.0; 3], &[]),
            obj("dc.ladder", "ld_1", [15.0; 3], &[]),
            obj("dc.world_model", "wm_1", [155.0; 3], &[]),
            obj("dc.world_model", "wm_2", [0.0, 90.0, 0.0], &[]),
            obj(
                "dc.bomb_site",
                "bs_1",
                [0.0; 3],
                &[
                    ("number", SynthValue::Int(5.0)),
                    ("extent", SynthValue::Vec3([1.0; 3])),
                ],
            ),
            obj(
                "dc.player_spawn",
                "ps_1",
                [0.0, 2000.0, 0.0],
                &[("team", SynthValue::Text(b"1".to_vec()))],
            ),
            obj(
                "dc.player_spawn",
                "ps_2",
                [0.0, 90.0, 0.0],
                &[("team", SynthValue::Text(b"1".to_vec()))],
            ),
            // C15: a spawn standing on the world-model box's top (y 95 + half height 53): the ground snap
            // counts the brush as a floor, the server's level-only drop does not
            obj(
                "dc.player_spawn",
                "ps_3",
                [0.0, 95.0 + STAND_HALF_HEIGHT, 0.0],
                &[("team", SynthValue::Text(b"2".to_vec()))],
            ),
        ],
        vec![],
    );
    let all = check_world(&faulty);
    let c: Vec<Code> = all.iter().map(|d| d.code).collect();
    for want in [
        Code::KillvolClosed,
        Code::LadderConvex,
        Code::BombNumber,
        Code::SpawnFloor,
        Code::SpawnClear,
        Code::SpawnBrushFloor,
        Code::ExtentModel,
    ] {
        assert!(c.contains(&want), "{want} missing in {c:?}");
    }
    let of = |code: Code| -> Vec<String> {
        all.iter()
            .filter(|d| d.code == code)
            .map(|d| d.path.clone())
            .collect()
    };
    assert_eq!(of(Code::SpawnFloor), ["objects.ps_1"]);
    assert_eq!(of(Code::SpawnClear), ["objects.ps_2"]);
    assert_eq!(of(Code::SpawnBrushFloor), ["objects.ps_3"]);

    // sky faces without portals and without a sky model; the sky faces carry texflags 0 (C4: `sky.surface`)
    let mut sky = level.clone();
    sky.surfaces[0].0 = 0x11;
    let no_sky = synth(vec![sky.clone()], vec![], vec![]);
    let c = codes(&no_sky);
    assert!(
        c.contains(&Code::SkyPortals) && c.contains(&Code::SkyModel),
        "{:?}",
        check_world(&no_sky).items
    );
    assert!(c.contains(&Code::SkySurface), "{c:?}");
    sky.surfaces[0].2 = SURFACE_SKY;
    let c = codes(&synth(vec![sky.clone()], vec![], vec![]));
    assert!(!c.contains(&Code::SkySurface), "{c:?}");
    let with_model = synth(
        vec![sky, box_model("sky_1", BSP_FLAG_BRUSH, 0x1, -5.0, 5.0)],
        vec![obj("dc.sky_model", "sky_1", [0.0; 3], &[])],
        vec!["sky_1"],
    );
    let c = codes(&with_model);
    assert!(!c.contains(&Code::SkyModel), "{c:?}");
    assert!(c.contains(&Code::SkyPortals), "{c:?}");

    // garbage does not parse
    assert_eq!(codes(b"not a world"), [Code::WorldParse]);
}

#[test]
fn the_size_estimate_is_within_a_quarter_of_a_real_export() {
    // C17: `info` counts the world at BYTES_PER_POLY, the radar DTX and a screenshot allowance
    let (dir, made) = scaffold(Template::Starter, "estimate");
    let b = build_made(&dir, &made);
    let info = dc_mapkit::validate::info(&dir, &made.project, &made.heights, None);
    let e = dc_mapkit::export::export(
        &made.project,
        &b,
        &dir.join("build"),
        dc_mapkit::export::ExportOptions {
            zip: false,
            no_rom_paths: true,
            project_dir: Some(dir.clone()),
            force: false,
        },
    )
    .unwrap_or_else(|d| panic!("{:?}", d.items));
    let (est, real) = (info.size_estimate_bytes as f64, e.bytes as f64);
    assert!(
        (est - real).abs() <= real * 0.25,
        "estimate {est} vs exported {real} ({} polygons, {} world bytes)",
        b.stats.polys,
        b.stats.bytes
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn crossing_rotated_walls_answer_a_budget_diagnostic_not_a_hang() {
    // C13: 20 long walls through one point, 9 degrees apart, as level solids
    use dc_mapkit::ops::{apply, Op, SolidSpec};
    let (dir, made) = scaffold(Template::Flat, "budget");
    let (mut p, mut h) = Project::open(&dir).unwrap();
    let ops: Vec<Op> = (0..20)
        .map(|k| Op::SolidAdd {
            kind: SolidSpec::Box,
            pos: [0.0, 300.0, 0.0],
            rot_y_deg: 9.0 * k as f32 + 1.0,
            size: [3000.0, 600.0, 8.0],
            material: "block".into(),
            faces: Default::default(),
            group: Some("spokes".into()),
            brush: None,
            ground: false,
            rises: None,
        })
        .collect();
    apply(&mut p, &mut h, &ops).unwrap_or_else(|d| panic!("{:?}", d.items));
    let t0 = std::time::Instant::now();
    let e = build(&p, &h, &Assets::new(&dir), &p.build).unwrap_err();
    assert!(t0.elapsed().as_secs_f64() < 60.0);
    let hit = e
        .iter()
        .find(|d| d.code == Code::BspBudget)
        .unwrap_or_else(|| panic!("{:?}", e.items));
    assert_eq!(hit.severity, Severity::Error);
    assert!(hit.message.contains("solid_"), "{}", hit.message);
    assert!(hit.fix.as_deref().is_some_and(|f| f.contains("split")));
    let _ = made;
    let _ = std::fs::remove_dir_all(&dir);
}
