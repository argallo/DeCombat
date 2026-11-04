//! w1-doors (SIM-18, CLI-420): a door is a solid world model that moves with its server object. Closed (its object at
//! the model origin) it blocks the doorway; slid or turned by the server it blocks where it now is and the doorway is
//! free; a door the server switched non-solid keeps its moved place for when it is solid again; a door the server did
//! not create is neither drawn nor solid (CLI-230).
//!
//! Synthetic (no ROM): a floor and one door panel linked to an object of a made-up class that the test's filter
//! names as a mover. ROM-gated: every sliding door of the user's worlds is in the default collision set and moves
//! with its object (bounds moved by the open offset), and every mover-class model is drawable.
// SPEC: MOVEMENT_2009.md §9.6 stage 2; RENDER_2009.md §5.1 step 6; UDP_SESSION_2009_B.md §4.2 items 6, 7
// DC-DEFAULT: SIM-18, CLI-420 (proposed, w1-doors)

use dc_client::objects::{pose_of, server_objects, ServerObjects, WorldModels};
use dc_sim::from_world::{collision_world, WorldFilter, FILE_FIRST_CHILD_IS_NEGATIVE};
use dc_sim::{build_bsp, ConvexPoly, SURF_SOLID, V3};
use dc_world::synth::{SynthModel, SynthObject, SynthWorld};

/// The door panel: 4 thick (x), 100 high, 60 wide (z), standing in a doorway at x = 0, z in -30..30.
const LO: [f32; 3] = [-2.0, 0.0, -30.0];
const HI: [f32; 3] = [2.0, 100.0, 30.0];
const ORIGIN: [f32; 3] = [0.0, 50.0, 0.0];
const CLASS: &[u8] = b"SynthMover";

fn box_faces(a: [f32; 3], b: [f32; 3]) -> Vec<ConvexPoly> {
    let c = |x: f32, y: f32, z: f32| V3::new(x, y, z);
    let quads = [
        [
            c(a[0], b[1], a[2]),
            c(a[0], b[1], b[2]),
            c(b[0], b[1], b[2]),
            c(b[0], b[1], a[2]),
        ],
        [
            c(a[0], a[1], a[2]),
            c(b[0], a[1], a[2]),
            c(b[0], a[1], b[2]),
            c(a[0], a[1], b[2]),
        ],
        [
            c(a[0], a[1], a[2]),
            c(a[0], a[1], b[2]),
            c(a[0], b[1], b[2]),
            c(a[0], b[1], a[2]),
        ],
        [
            c(b[0], a[1], a[2]),
            c(b[0], b[1], a[2]),
            c(b[0], b[1], b[2]),
            c(b[0], a[1], b[2]),
        ],
        [
            c(a[0], a[1], a[2]),
            c(a[0], b[1], a[2]),
            c(b[0], b[1], a[2]),
            c(b[0], a[1], a[2]),
        ],
        [
            c(a[0], a[1], b[2]),
            c(b[0], a[1], b[2]),
            c(b[0], b[1], b[2]),
            c(a[0], b[1], b[2]),
        ],
    ];
    quads
        .iter()
        .map(|q| ConvexPoly::from_corners(q.to_vec(), SURF_SOLID))
        .collect()
}

fn model(
    name: &[u8],
    polys: &[ConvexPoly],
    lo: [f32; 3],
    hi: [f32; 3],
    origin: [f32; 3],
) -> SynthModel {
    let (nodes, root) = build_bsp(polys);
    let mut m = SynthModel {
        flags: 2,
        name: name.to_vec(),
        textures: vec![b"none".to_vec()],
        surfaces: vec![(SURF_SOLID, 0, 0)],
        box_min: lo,
        box_max: hi,
        origin,
        ..SynthModel::default()
    };
    for (i, poly) in polys.iter().enumerate() {
        m.planes.push((poly.normal.to_array(), poly.dist));
        let first = m.points.len() as u32;
        m.points.extend(poly.verts.iter().map(|v| v.to_array()));
        m.polys.push((
            0,
            i as u32,
            (first..first + poly.verts.len() as u32).collect(),
        ));
    }
    let enc = |c: Option<u32>| c.map_or(-1, |i| i as i32);
    for (face, neg, pos) in nodes {
        let (a, b) = if FILE_FIRST_CHILD_IS_NEGATIVE {
            (neg, pos)
        } else {
            (pos, neg)
        };
        m.nodes.push((face, 0, enc(a), enc(b)));
    }
    m.root = enc(root);
    m
}

fn world_bytes() -> Vec<u8> {
    let floor = ConvexPoly::from_corners(
        vec![
            V3::new(-1000.0, -1.0, -1000.0),
            V3::new(-1000.0, -1.0, 1000.0),
            V3::new(1000.0, -1.0, 1000.0),
            V3::new(1000.0, -1.0, -1000.0),
        ],
        SURF_SOLID,
    );
    let mut level = model(
        b"level",
        &[floor],
        [-1000.0, -2.0, -1000.0],
        [1000.0, 0.0, 1000.0],
        [0.0; 3],
    );
    level.flags = 0;
    let names: [&[u8]; 3] = [b"Nm", b"Pos", b"Rot"];
    SynthWorld {
        extent_min: [-1000.0, -100.0, -1000.0],
        extent_max: [1000.0, 400.0, 1000.0],
        tree_shape: vec![false],
        models: vec![level, model(b"DoorA", &box_faces(LO, HI), LO, HI, ORIGIN)],
        objects: vec![SynthObject::point(CLASS, names, b"DoorA", ORIGIN, [0.0; 3])],
        grid_cell: [64.0; 3],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![0, 0, 0],
        ..SynthWorld::default()
    }
    .build()
}

fn filter() -> WorldFilter {
    WorldFilter {
        movers: vec![dc_world::Key {
            hash: dc_world::hash16(CLASS),
            len: CLASS.len(),
            ours: "",
        }],
        ..WorldFilter::default()
    }
}

fn door(flags: u16, pos: [f32; 3], rot: Option<[f32; 4]>) -> ServerObjects {
    let mut s = ServerObjects::default();
    s.brushes.insert(b"doora".to_vec(), flags);
    s.poses.insert(b"doora".to_vec(), (pos, rot));
    s
}

/// A ray across the doorway line x = 0 at height 40 and depth z.
fn blocked(cw: &dc_sim::CollisionWorld, z: f32) -> bool {
    cw.ray(V3::new(-50.0, 40.0, z), V3::new(50.0, 40.0, z))
        .is_some()
}

#[test]
fn a_door_is_solid_where_the_server_puts_it() {
    let bytes = world_bytes();
    let w = dc_world::World::parse(&bytes).expect("synthetic world");
    let f = filter();
    // a class without a mover entry stays out of the collision set (the former rule)
    let plain = collision_world(&w, &WorldFilter::default());
    assert!(!blocked(&plain, 0.0));
    let mut cw = collision_world(&w, &f);
    let mut m = WorldModels::of(&w, &f, None);
    assert!(blocked(&cw, 0.0), "the door of the file is solid offline");
    // created closed (0x3101 at the model origin)
    m.apply(&door(0x3101, ORIGIN, None), &mut cw, None);
    assert_eq!(m.state(b"DoorA"), Some((true, true)));
    assert_eq!(m.pose(b"DoorA"), None);
    assert!(blocked(&cw, 0.0) && !blocked(&cw, -60.0));
    // slid open by 60 along -z
    let open = [0.0, 50.0, -60.0];
    m.apply(&door(0x3101, open, None), &mut cw, None);
    assert!(m.pose(b"DoorA").is_some());
    assert!(!blocked(&cw, 0.0), "the doorway is free");
    assert!(blocked(&cw, -60.0), "the panel is solid where it slid");
    // half way
    m.apply(&door(0x3101, [0.0, 50.0, -30.0], None), &mut cw, None);
    assert!(blocked(&cw, -50.0) && !blocked(&cw, 10.0));
    // switched non-solid while open, then solid again: still at the open place
    m.apply(&door(0x1101, open, None), &mut cw, None);
    assert!(!blocked(&cw, -60.0) && !blocked(&cw, 0.0));
    m.apply(&door(0x3101, open, None), &mut cw, None);
    assert!(blocked(&cw, -60.0) && !blocked(&cw, 0.0));
    // closed again: the file model exactly
    m.apply(&door(0x3101, ORIGIN, None), &mut cw, None);
    assert_eq!(m.pose(b"DoorA"), None);
    assert!(blocked(&cw, 0.0) && !blocked(&cw, -60.0));
    // turned 90 degrees about y around a hinge at the panel edge z = 30
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let rot = [0.0, h, 0.0, h];
    let hinge = [0.0, 50.0, 30.0];
    let swing = pose_of(hinge, hinge, Some(rot))
        .expect("a turn")
        .point(V3::from_array(ORIGIN))
        .to_array();
    // the panel centre went from 30 before the hinge (along -z) to 30 beside it (along x)
    assert!((swing[2] - 30.0).abs() < 1e-3 && (swing[0].abs() - 30.0).abs() < 1e-3);
    m.apply(&door(0x3101, swing, Some(rot)), &mut cw, None);
    assert!(!blocked(&cw, 0.0), "the doorway is free");
    let across = |cw: &dc_sim::CollisionWorld, x: f32| {
        cw.ray(V3::new(x, 40.0, 80.0), V3::new(x, 40.0, -20.0))
            .is_some()
    };
    assert!(
        across(&cw, swing[0]),
        "the swung panel blocks at x {}",
        swing[0]
    );
    // not created by the server: neither drawn nor solid
    m.apply(&ServerObjects::default(), &mut cw, None);
    assert_eq!(m.state(b"DoorA"), Some((false, false)));
    assert!(!blocked(&cw, 0.0) && !across(&cw, swing[0]));
}

#[test]
fn server_objects_report_brush_positions_and_rotations() {
    let mut t = dc_botclient::objects::ObjectTable::default();
    let mut b = dc_botclient::objects::ClientObject::default();
    b.kind = dc_botclient::objects::kind::BRUSH;
    b.name = Some(b"DoorA".to_vec());
    b.flags = 0x3101;
    b.pos = Some([1.0, 2.0, 3.0]);
    b.rot = Some([0.0, 0.0, 0.0, 1.0]);
    t.objects.insert(80, b);
    let s = server_objects(&t);
    assert_eq!(
        s.poses.get(b"doora".as_slice()),
        Some(&([1.0, 2.0, 3.0], Some([0.0, 0.0, 0.0, 1.0])))
    );
    // a pose at the origin with no turn is the file position; a tiny drift too
    assert_eq!(pose_of(ORIGIN, ORIGIN, None), None);
    assert_eq!(
        pose_of(ORIGIN, [0.0, 50.0, 0.005], Some([0.0, 0.0, 0.0, 1.0])),
        None
    );
    assert!(pose_of(ORIGIN, [0.0, 50.0, 1.0], None).is_some());
}

#[test]
fn rom_sliding_doors_are_solid_movers() {
    let rom = dc_rom::rom_or_skip!();
    let vfs = dc_rez::RezVfs::new(&rom);
    let entries = vfs.list_entries("WORLDS.rez").expect("world archive");
    let (mut worlds, mut doors) = (0, 0);
    for (name, _, _) in entries
        .iter()
        .filter(|e| e.0.to_ascii_uppercase().ends_with(".DAT"))
    {
        let data = vfs.entry("WORLDS.rez", name).ok().flatten().expect("entry");
        let Ok(w) = dc_world::World::parse(&data) else {
            continue;
        };
        let infos = dc_dserver::doors::door_infos(&w);
        if infos.is_empty() {
            continue;
        }
        worlds += 1;
        let f = WorldFilter::default();
        let mut cw = collision_world(&w, &f);
        let mut m = WorldModels::of(&w, &f, None);
        // every linked model of a mover class is drawable (the scene builds it as a world model)
        for o in w.objects.iter().filter(|o| f.is_mover(o)) {
            assert_ne!(
                dc_render::model_visibility(Some(o)),
                dc_render::ModelVisibility::Hidden,
                "{name}"
            );
        }
        // every door created closed (CLI-230: a door without its brush is not solid)
        let mut s = ServerObjects::default();
        for d in &infos {
            let key = d.name.to_ascii_lowercase();
            s.brushes.insert(key.clone(), 0x3101);
            s.poses.insert(key, (d.closed, None));
        }
        m.apply(&s, &mut cw, None);
        for d in &infos {
            doors += 1;
            let key = d.name.to_ascii_lowercase();
            let c = (0..cw.models.len())
                .find(|&c| m.name_at_collision(c) == Some(key.as_slice()))
                .unwrap_or_else(|| panic!("{name}: a door outside the collision set"));
            let (lo, hi) = cw.models[c].bounds();
            let off = d.dir.map(|k| k * d.dist);
            let open = [0, 1, 2].map(|k| d.closed[k] + off[k]);
            s.poses.insert(key.clone(), (open, None));
            m.apply(&s, &mut cw, None);
            let (lo2, hi2) = cw.models[c].bounds();
            for k in 0..3 {
                assert!(
                    (lo2.to_array()[k] - lo.to_array()[k] - off[k]).abs() < 0.01,
                    "{name} {k}: {lo:?} -> {lo2:?} by {off:?}"
                );
                assert!(
                    (hi2.to_array()[k] - hi.to_array()[k] - off[k]).abs() < 0.01,
                    "{name}"
                );
            }
            s.poses.insert(key.clone(), (d.closed, None));
            m.apply(&s, &mut cw, None);
            assert_eq!(
                cw.models[c].bounds(),
                (lo, hi),
                "{name}: closed = the file model"
            );
        }
    }
    println!("{doors} sliding doors on {worlds} worlds move with their objects");
    assert!(doors > 0);
}

/// The server's rotating doors (w1-doors `turn_infos` / `TurnDoor`), fully open, sent as change records through the
/// client's object table: each door's collision model lies where the server turned it (bounds of the turned file
/// points), and closed again it is the file model.
#[test]
fn rom_rotating_doors_turn_on_the_client_as_on_the_server() {
    use dc_dserver::doors::{quat_turn, turn_infos, TurnDoor, TurnKind};
    let rom = dc_rom::rom_or_skip!();
    let vfs = dc_rez::RezVfs::new(&rom);
    let entries = vfs.list_entries("WORLDS.rez").expect("world archive");
    let (mut worlds, mut doors, mut switches) = (0, 0, 0);
    for (name, _, _) in entries
        .iter()
        .filter(|e| e.0.to_ascii_uppercase().ends_with(".DAT"))
    {
        let data = vfs.entry("WORLDS.rez", name).ok().flatten().expect("entry");
        let Ok(w) = dc_world::World::parse(&data) else {
            continue;
        };
        let infos = turn_infos(&w);
        if infos.is_empty() {
            continue;
        }
        worlds += 1;
        let f = WorldFilter::default();
        let mut cw = collision_world(&w, &f);
        let mut m = WorldModels::of(&w, &f, None);
        let mut table = dc_botclient::objects::ObjectTable::default();
        for (k, d) in infos.iter().enumerate() {
            let mut o = dc_botclient::objects::ClientObject::default();
            o.kind = dc_botclient::objects::kind::BRUSH;
            o.name = Some(d.name.clone());
            o.flags = 0x3101;
            o.pos = Some(d.origin);
            table.objects.insert(100 + k as u16, o);
        }
        m.apply(&server_objects(&table), &mut cw, None);
        let closed: Vec<_> = infos
            .iter()
            .map(|d| {
                let key = d.name.to_ascii_lowercase();
                let c = (0..cw.models.len())
                    .find(|&c| m.name_at_collision(c) == Some(key.as_slice()))
                    .unwrap_or_else(|| panic!("{name}: a turning model outside the collision set"));
                (c, cw.models[c].bounds())
            })
            .collect();
        // open every door fully on the server and send its record
        let mut body = dc_wire::udp::BitWriter::new();
        let mut turned = Vec::new();
        for (k, d) in infos.iter().enumerate() {
            let mut t = TurnDoor::new(100 + k as u16, d.clone());
            if d.kind == TurnKind::Switch {
                switches += 1;
                turned.push(None);
                continue;
            }
            doors += 1;
            t.frac = 1.0;
            let rec = t.record();
            body.write_u32(rec.bit_len() as u32).append(&rec);
            turned.push(Some(t));
        }
        table.apply_group(&body);
        assert!(table.errors.is_empty(), "{name}: {:?}", table.errors);
        m.apply(&server_objects(&table), &mut cw, None);
        for (k, d) in infos.iter().enumerate() {
            let (c, file) = closed[k];
            let Some(t) = &turned[k] else {
                assert_eq!(cw.models[c].bounds(), file, "{name}: a switch stays");
                continue;
            };
            let model = w.model_named(&d.name).expect("model");
            let (rot, pos) = (t.rot(), t.pos());
            let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            for p in model.points.iter() {
                let q = quat_turn(rot, [0, 1, 2].map(|a| p[a] - d.origin[a]));
                for a in 0..3 {
                    lo[a] = lo[a].min(pos[a] + q[a]);
                    hi[a] = hi[a].max(pos[a] + q[a]);
                }
            }
            let (lo2, hi2) = cw.models[c].bounds();
            for a in 0..3 {
                assert!(
                    (lo2.to_array()[a] - lo[a]).abs() < 0.05
                        && (hi2.to_array()[a] - hi[a]).abs() < 0.05,
                    "{name} {a}: {lo:?} {hi:?} vs {lo2:?} {hi2:?}"
                );
            }
        }
        // closed again
        let mut body = dc_wire::udp::BitWriter::new();
        for t in turned.iter().flatten() {
            let mut t = t.clone();
            t.frac = 0.0;
            let rec = t.record();
            body.write_u32(rec.bit_len() as u32).append(&rec);
        }
        table.apply_group(&body);
        m.apply(&server_objects(&table), &mut cw, None);
        for (c, file) in &closed {
            assert_eq!(
                cw.models[*c].bounds(),
                *file,
                "{name}: closed = the file model"
            );
        }
    }
    println!(
        "{doors} rotating doors and {switches} switches on {worlds} worlds turn on the client"
    );
    assert!(doors > 0);
}

fn out_dir() -> std::path::PathBuf {
    let o = dc_render::default_out_dir().join("w1-doors");
    let _ = std::fs::create_dir_all(&o);
    o
}

/// The first ROM world with sliding doors, loaded with its render scene: a camera in front of one door sees the door
/// closed, then open after the server objects move it (the drawn panel moves), then closed again byte for byte.
/// PNGs `door_<map>_{closed,open}.png` in the report folder.
#[test]
fn rom_an_opened_door_is_drawn_open() {
    use dc_client::offscreen::Offscreen;
    use dc_client::world::GpuTarget;
    use dc_client::{RomWorld, ShootTest, ViewSettings, WorldSource};
    use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
    let rom = dc_rom::rom_or_skip!();
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return;
        }
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let mut pick = None;
    for map in RomWorld::map_names(&rom).expect("maps") {
        let Ok(Some(data)) = vfs.entry("WORLDS.rez", &format!("WORLDS\\{map}.DAT")) else {
            continue;
        };
        let Ok(w) = dc_world::World::parse(&data) else {
            continue;
        };
        let infos = dc_dserver::doors::door_infos(&w);
        if !infos.is_empty() {
            pick = Some((map, infos));
            break;
        }
    }
    let (map, infos) = pick.expect("a ROM world with sliding doors");
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = RomWorld::new(&rom, &map);
    let mut world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("world");
    let names = world.scene.as_ref().unwrap().model_names().to_vec();
    for d in &infos {
        assert!(
            names.iter().any(|n| n.eq_ignore_ascii_case(&d.name)),
            "{map}: a door in the render scene"
        );
    }
    let set = |open: Option<usize>| {
        let mut s = ServerObjects::default();
        for (k, d) in infos.iter().enumerate() {
            let key = d.name.to_ascii_lowercase();
            let pos = if open == Some(k) {
                [0, 1, 2].map(|a| d.closed[a] + d.dir[a] * d.dist)
            } else {
                d.closed
            };
            s.brushes.insert(key.clone(), 0x3101);
            s.poses.insert(key, (pos, None));
        }
        s
    };
    // the door seen face on: from 160 units along its thinnest box axis
    let d = &infos[0];
    let ext = [0, 1, 2].map(|a| d.hi[a] - d.lo[a]);
    let thin = (0..3).min_by(|a, b| ext[*a].total_cmp(&ext[*b])).unwrap();
    let mut eye = d.closed;
    eye[thin] += 160.0;
    let (w, h) = (640, 480);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let shoot = |world: &dc_client::world::LoadedWorld| {
        let view = ShootTest::look_at(eye, d.closed);
        let camera = ViewSettings::default().camera(V3::new(eye[0], eye[1], eye[2]), &view, w, h);
        target
            .render(
                &ctx,
                &renderer,
                world.scene.as_ref().unwrap(),
                &camera,
                None,
            )
            .unwrap()
    };
    world.apply_objects(&set(None));
    let closed = shoot(&world);
    world.apply_objects(&set(Some(0)));
    let open = shoot(&world);
    world.apply_objects(&set(None));
    let again = shoot(&world);
    let _ = closed.save_png(&out_dir().join(format!("door_{map}_closed.png")));
    let _ = open.save_png(&out_dir().join(format!("door_{map}_open.png")));
    let mut changed = 0usize;
    for y in 0..h {
        for x in 0..w {
            let (p, q) = (closed.pixel(x, y), open.pixel(x, y));
            if (0..3)
                .map(|k| (p[k] as i32 - q[k] as i32).abs())
                .sum::<i32>()
                > 24
            {
                changed += 1;
            }
        }
    }
    println!(
        "{map}: door {} slid {} units; {changed} of {} pixels changed; PNGs in {}",
        String::from_utf8_lossy(&d.name),
        d.dist,
        w * h,
        out_dir().display()
    );
    assert!(
        changed > (w * h / 50) as usize,
        "the opened door is drawn open"
    );
    assert!(again.rgba == closed.rgba, "closed again: the file frame");
}
