//! BOT-110 / BOT-111 on a synthetic world: the bot's collision set follows the server's named brushes the way the
//! client's does (a linked world model is solid only while the server created it with the solid flag; a door only
//! while it stands closed), and the kill volumes the server created keep the bot's grid out. Playtest reports: bots
//! floated over WarHead's sewer and walked on Two Towers' water, both on world models the start scripts leave
//! non-solid (the client falls through them into the fall volume below).

use std::sync::Arc;

use dc_botclient::motion::{Body, MotionKind, BOT_STAND_DIMS};
use dc_botclient::nav::NavGrid;
use dc_botclient::objects::{kind, ClientObject, ObjectTable};
use dc_botclient::romworld::{world_from_bytes, BRUSH_FLAG_SOLID, KILL_VOLUME_CODE};
use dc_sim::from_world::FILE_FIRST_CHILD_IS_NEGATIVE;
use dc_sim::{build_bsp, ConvexPoly, SURF_SOLID, V3};
use dc_world::synth::{SynthModel, SynthObject, SynthWorld};

fn box_faces(a: [f32; 3], b: [f32; 3]) -> Vec<ConvexPoly> {
    let c = V3::new;
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

fn model(name: &[u8], lo: [f32; 3], hi: [f32; 3], dual: bool) -> SynthModel {
    let polys = box_faces(lo, hi);
    let (nodes, root) = build_bsp(&polys);
    let mut m = SynthModel {
        flags: if dual { 2 } else { 0 },
        name: name.to_vec(),
        textures: vec![b"none".to_vec()],
        surfaces: vec![(SURF_SOLID, 0, 0)],
        box_min: lo,
        box_max: hi,
        origin: lo,
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

/// A floor at y 0, a deck (a linked world model, top at y 100) over x 200..400 and a pit volume over x -400..-200.
const DECK_LO: [f32; 3] = [200.0, 80.0, -100.0];
const DECK_HI: [f32; 3] = [400.0, 100.0, 100.0];
const PIT_LO: [f32; 3] = [-400.0, -10.0, -1000.0];
const PIT_HI: [f32; 3] = [-200.0, 60.0, 1000.0];

fn world_bytes() -> Vec<u8> {
    SynthWorld {
        extent_min: [-1000.0, -100.0, -1000.0],
        extent_max: [1000.0, 400.0, 1000.0],
        tree_shape: vec![false],
        models: vec![
            model(
                b"level",
                [-1000.0, -64.0, -1000.0],
                [1000.0, 0.0, 1000.0],
                false,
            ),
            model(b"Deck", DECK_LO, DECK_HI, true),
            model(b"Pit", PIT_LO, PIT_HI, true),
        ],
        objects: vec![
            SynthObject::dc("dc.world_model", "Deck", DECK_LO, [0.0; 3], &[]),
            SynthObject::dc("dc.kill_volume", "Pit", PIT_LO, [0.0; 3], &[]),
        ],
        grid_cell: [64.0; 3],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![0, 0, 0],
        ..SynthWorld::default()
    }
    .build()
}

fn brush(name: &[u8], flags: u16) -> ClientObject {
    let mut o = ClientObject::default();
    o.kind = kind::BRUSH;
    o.name = Some(name.to_vec());
    o.flags = flags;
    o
}

fn table(objs: Vec<ClientObject>) -> ObjectTable {
    let mut t = ObjectTable::default();
    for (i, o) in objs.into_iter().enumerate() {
        t.objects.insert(100 + i as u16, o);
    }
    t
}

/// Height of the first solid thing below `(x, 300, z)`.
fn ground(c: &dc_sim::CollisionWorld, x: f32, z: f32) -> Option<f32> {
    let (a, b) = (V3::new(x, 300.0, z), V3::new(x, -50.0, z));
    c.ray(a, b).map(|h| a.y + (b.y - a.y) * h.t)
}

#[test]
fn the_collision_set_follows_the_servers_named_brushes() {
    let mut w = world_from_bytes("synth", &world_bytes()).expect("world");
    assert_eq!(w.linked.len(), 1, "the deck is linked, the pit is a volume");
    // the file's own set until the server's objects come: the deck is there
    assert!((ground(&w.collision, 300.0, 0.0).unwrap() - 100.0).abs() < 0.5);
    // the server created it visible but not solid (a start script's `solid 0`): the bot falls through as players do
    let (c, gone) = w
        .follow_server(&table(vec![brush(b"DECK", 0x0001)]))
        .expect("changed");
    assert_eq!(gone, vec![w.linked[0].index as u32]);
    assert!(ground(&c, 300.0, 0.0).unwrap().abs() < 0.5, "on the floor");
    assert!(
        w.follow_server(&table(vec![brush(b"Deck", 0x0001)]))
            .is_none(),
        "unchanged"
    );
    // created solid: back in the set
    let (c, gone) = w
        .follow_server(&table(vec![brush(b"deck", 0x0001 | BRUSH_FLAG_SOLID)]))
        .expect("changed");
    assert!(gone.is_empty());
    assert!((ground(&c, 300.0, 0.0).unwrap() - 100.0).abs() < 0.5);
    // not created at all: not solid (the client collides with what the server creates, nothing else)
    let (c, _) = w.follow_server(&table(Vec::new())).expect("changed");
    assert!(ground(&c, 300.0, 0.0).unwrap().abs() < 0.5);
}

#[test]
fn a_bot_on_a_model_the_server_made_non_solid_falls_to_the_floor() {
    let mut w = world_from_bytes("synth", &world_bytes()).expect("world");
    let half = BOT_STAND_DIMS[1];
    let mut b = Body::new(
        [300.0, 100.0 + half, 0.0],
        BOT_STAND_DIMS,
        0.0,
        MotionKind::Stand,
        60,
        Some(w.collision.clone()),
    );
    b.advance(500_000);
    assert!((b.pos()[1] - (100.0 + half)).abs() < 1.0, "{:?}", b.pos());
    let (c, gone) = w
        .follow_server(&table(vec![brush(b"Deck", 0x0001)]))
        .expect("changed");
    b.set_world(c, &gone);
    for _ in 0..120 {
        b.advance(16_667);
    }
    assert!(
        (b.pos()[1] - half).abs() < 1.0,
        "fell to the floor: {:?}",
        b.pos()
    );
}

#[test]
fn the_grid_keeps_out_of_the_kill_volumes_the_server_created() {
    let w = world_from_bytes("synth", &world_bytes()).expect("world");
    assert_eq!(w.kill_volumes.len(), 1);
    let mut pit = ClientObject::default();
    pit.kind = kind::ZONE;
    pit.name = Some(b"PIT".to_vec());
    pit.code = Some(KILL_VOLUME_CODE);
    assert!(
        w.active_kill_volumes(&table(Vec::new())).is_empty(),
        "not created: not active"
    );
    let kill = w.active_kill_volumes(&table(vec![pit]));
    assert_eq!(kill.len(), 1);
    let mut nav = NavGrid::new(Arc::clone(&w.full), BOT_STAND_DIMS).with_kill_volumes(kill);
    nav.seed([0.0, BOT_STAND_DIMS[1], 0.0]);
    let mut probes = 0;
    while nav.growing() && probes < 200 {
        nav.expand(dc_botclient::nav::BUDGET, Some([-900.0, 40.0, 0.0]));
        probes += 1;
    }
    let p = nav
        .path(
            [0.0, BOT_STAND_DIMS[1], 0.0],
            [-900.0, BOT_STAND_DIMS[1], 0.0],
        )
        .unwrap_or_default();
    assert!(!p.is_empty());
    assert!(
        p.iter().all(|q| q[0] > PIT_HI[0] + 15.0),
        "the way stops before the pit: {p:?}"
    );
}
