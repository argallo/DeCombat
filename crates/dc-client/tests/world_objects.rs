//! world-objects (CLI-230..CLI-232): the client draws and collides with a linked world model only while the server has
//! a named brush of that name with the visible / solid flags (the 2009 object model), so each mode shows the set its
//! server created and a destroyed breakable (its flags switched off) is gone from the view and the collision set.
//! Headless and synthetic: a made-up level with two made-up crates (top faces as platforms); no ROM, no window.
// SPEC: FMT_DAT85.md §6, §6.4; UDP_SESSION_2009_B.md §3.3, §4.2 item 3; RENDER_2009.md §5.1 step 6
// DISASM: engine+0xa2190 (kind-2 create binds by name)

use dc_client::objects::{ServerObjects, WorldEffect, WorldModels};
use dc_client::world::{FrameState, SynthSource};
use dc_client::WorldSource;
use dc_sim::from_world::{collision_world, WorldFilter, FILE_FIRST_CHILD_IS_NEGATIVE};
use dc_sim::{build_bsp, ConvexPoly, SURF_SOLID, V3};
use dc_world::synth::{SynthModel, SynthObject, SynthWorld};

const CRATE_A: [f32; 3] = [0.0, 40.0, 200.0];
const CRATE_B: [f32; 3] = [300.0, 40.0, 200.0];
const HALF: f32 = 30.0;

/// Upward-facing rectangle at height `y`.
fn top(x0: f32, x1: f32, z0: f32, z1: f32, y: f32) -> ConvexPoly {
    ConvexPoly::from_corners(
        vec![
            V3::new(x0, y, z0),
            V3::new(x0, y, z1),
            V3::new(x1, y, z1),
            V3::new(x1, y, z0),
        ],
        SURF_SOLID,
    )
}

fn model(name: &[u8], polys: &[ConvexPoly], lo: [f32; 3], hi: [f32; 3]) -> SynthModel {
    let (nodes, root) = build_bsp(polys);
    let mut m = SynthModel {
        flags: 2,
        name: name.to_vec(),
        textures: vec![b"none".to_vec()],
        surfaces: vec![(SURF_SOLID, 0, 0)],
        box_min: lo,
        box_max: hi,
        origin: [
            0.5 * (lo[0] + hi[0]),
            0.5 * (lo[1] + hi[1]),
            0.5 * (lo[2] + hi[2]),
        ],
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

fn crate_model(name: &[u8], c: [f32; 3]) -> SynthModel {
    let poly = top(c[0] - HALF, c[0] + HALF, c[2] - HALF, c[2] + HALF, c[1]);
    model(
        name,
        &[poly],
        [c[0] - HALF, 0.0, c[2] - HALF],
        [c[0] + HALF, c[1], c[2] + HALF],
    )
}

/// A floor (the level model, linked to no object) and two crates, each named by an object of a made-up class.
fn world_bytes() -> Vec<u8> {
    let floor = top(-1000.0, 1000.0, -1000.0, 1000.0, 0.0);
    let mut level = model(
        b"level",
        &[floor],
        [-1000.0, -1.0, -1000.0],
        [1000.0, 1.0, 1000.0],
    );
    level.flags = 0;
    let names: [&[u8]; 3] = [b"Nm", b"Pos", b"Rot"];
    SynthWorld {
        extent_min: [-1000.0, -100.0, -1000.0],
        extent_max: [1000.0, 400.0, 1000.0],
        tree_shape: vec![false],
        models: vec![
            level,
            crate_model(b"CrateA", CRATE_A),
            crate_model(b"CrateB", CRATE_B),
        ],
        objects: vec![
            SynthObject::point(b"SynthCrate", names, b"CrateA", CRATE_A, [0.0; 3]),
            SynthObject::point(b"SynthCrate", names, b"CrateB", CRATE_B, [0.0; 3]),
        ],
        grid_cell: [64.0; 3],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![0, 0, 0],
        ..SynthWorld::default()
    }
    .build()
}

fn set(brushes: &[(&[u8], u16)], effects: Vec<WorldEffect>) -> ServerObjects {
    ServerObjects {
        brushes: brushes
            .iter()
            .map(|(n, f)| (n.to_ascii_lowercase(), *f))
            .collect(),
        effects,
        ..ServerObjects::default()
    }
}

/// The height a ray straight down at (x, z) first meets.
fn ground(cw: &dc_sim::CollisionWorld, x: f32, z: f32) -> f32 {
    let (from, to) = (V3::new(x, 100.0, z), V3::new(x, -50.0, z));
    let h = cw.ray(from, to).expect("something below");
    100.0 - 150.0 * h.t
}

#[test]
fn world_models_follow_the_server_set_per_mode_and_a_destroyed_crate_goes() {
    let bytes = world_bytes();
    let w = dc_world::World::parse(&bytes).expect("synthetic world");
    // the made-up class has no role: collide with unlabelled linked models
    let filter = WorldFilter {
        unlabelled: true,
        ..WorldFilter::default()
    };
    let mut cw = collision_world(&w, &filter);
    let scene_names = vec![b"CrateA".to_vec(), b"CrateB".to_vec()];
    let mut m = WorldModels::of(&w, &filter, Some(&scene_names));
    assert_eq!(m.len(), 2, "two linked models, the level is not one");
    // before any server objects: the file's own state
    assert_eq!(m.state(b"cratea"), Some((true, true)));
    assert!((ground(&cw, CRATE_A[0], CRATE_A[2]) - CRATE_A[1]).abs() < 0.5);

    // mode X: the server created crate A only
    let fx = m.apply(&set(&[(b"CrateA", 0x3101)], vec![]), &mut cw, None);
    assert!(fx.is_empty());
    assert_eq!(m.state(b"CrateA"), Some((true, true)));
    assert_eq!(m.state(b"CrateB"), Some((false, false)));
    assert!(
        ground(&cw, CRATE_B[0], CRATE_B[2]).abs() < 0.5,
        "B no longer solid"
    );
    assert!((ground(&cw, CRATE_A[0], CRATE_A[2]) - CRATE_A[1]).abs() < 0.5);
    assert_eq!(m.drawn(), vec![b"cratea".as_slice()]);

    // mode Y: the other set
    m.apply(&set(&[(b"CrateB", 0x3101)], vec![]), &mut cw, None);
    assert_eq!(m.state(b"CrateA"), Some((false, false)));
    assert_eq!(m.state(b"CrateB"), Some((true, true)));
    assert!(ground(&cw, CRATE_A[0], CRATE_A[2]).abs() < 0.5);
    assert!((ground(&cw, CRATE_B[0], CRATE_B[2]) - CRATE_B[1]).abs() < 0.5);

    // visible but not solid, solid but not visible
    m.apply(
        &set(&[(b"CrateA", 0x1101), (b"CrateB", 0x2100)], vec![]),
        &mut cw,
        None,
    );
    assert_eq!(m.state(b"CrateA"), Some((true, false)));
    assert_eq!(m.state(b"CrateB"), Some((false, true)));

    // both, then crate A is destroyed: its flags lose visible / solid and its death effect appears once
    m.apply(
        &set(&[(b"CrateA", 0x3101), (b"CrateB", 0x3101)], vec![]),
        &mut cw,
        None,
    );
    let burst = WorldEffect {
        obj: 90,
        name: b"Burst".to_vec(),
        pos: CRATE_A,
        rot: [0.0, 0.0, 0.0, 1.0],
        value: 0,
        removed: false,
        created: 0,
    };
    let fx = m.apply(
        &set(
            &[(b"CrateA", 0x0100), (b"CrateB", 0x3101)],
            vec![burst.clone()],
        ),
        &mut cw,
        None,
    );
    assert_eq!(fx, vec![burst.clone()]);
    assert_eq!(m.state(b"CrateA"), Some((false, false)));
    assert!(
        ground(&cw, CRATE_A[0], CRATE_A[2]).abs() < 0.5,
        "walk through the broken crate"
    );
    let again = m.apply(
        &set(&[(b"CrateA", 0x0100), (b"CrateB", 0x3101)], vec![burst.clone()]),
        &mut cw,
        None,
    );
    assert!(again.is_empty(), "an effect is handed over once");
    // CLI-1582: the same id created again as another effect between two frames (the server freed it and gave it to
    // the next effect in one tick): the old one leaves first, the new one appears
    let ready = WorldEffect {
        name: b"Ready".to_vec(),
        created: 1,
        ..burst.clone()
    };
    let fx = m.apply(
        &set(&[(b"CrateA", 0x0100), (b"CrateB", 0x3101)], vec![ready.clone()]),
        &mut cw,
        None,
    );
    assert_eq!(fx.len(), 2, "{fx:?}");
    assert!(fx[0].removed && fx[0].obj == 90, "the old effect leaves first");
    assert_eq!(fx[1], ready);
    // the effect leaves, the brush is removed: nothing linked is drawn
    m.apply(&set(&[], vec![]), &mut cw, None);
    assert!(m.drawn().is_empty());
}

/// A server-backed stand-in: the synthetic crate world, objects from frame 2 on (both crates), crate A destroyed
/// from frame 5 on.
struct ServerLike {
    inner: SynthSource,
    frame: u32,
}

impl WorldSource for ServerLike {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn load(
        &mut self,
        gpu: Option<dc_client::world::GpuTarget<'_>>,
    ) -> Result<dc_client::world::LoadedWorld, dc_client::ClientError> {
        self.inner.load(gpu)
    }
    fn frame_state(&mut self) -> FrameState {
        self.frame += 1;
        let objs = match self.frame {
            0..=1 => set(&[], vec![]),
            2..=4 => set(&[(b"CrateA", 0x3101), (b"CrateB", 0x3101)], vec![]),
            _ => set(
                &[(b"CrateA", 0x0100), (b"CrateB", 0x3101)],
                vec![WorldEffect {
                    obj: 90,
                    name: b"Burst".to_vec(),
                    pos: CRATE_A,
                    rot: [0.0, 0.0, 0.0, 1.0],
                    value: 0,
                    removed: false,
                    created: 0,
                }],
            ),
        };
        FrameState {
            life_state: Some(1),
            world_objects: Some(objs),
            ..FrameState::default()
        }
    }
}

#[test]
fn the_frame_loop_applies_the_server_objects() {
    let source = || SynthSource {
        name: "CRATES".into(),
        bytes: world_bytes(),
        spawns: vec![dc_client::world::SpawnPoint {
            pos: V3::new(-200.0, 60.0, -200.0),
            yaw: 0.0,
        }],
        stand_dims: dc_client::testkit::SYNTH_DIMS,
        textures: vec![],
    };
    let opts = dc_client::HeadlessOptions {
        frames: 8,
        render: false,
        ..dc_client::HeadlessOptions::default()
    };
    let mut src = ServerLike {
        inner: source(),
        frame: 0,
    };
    let r = dc_client::run_headless(&mut src, None, &opts).expect("headless run");
    assert_eq!(r.world_models_drawn, vec![b"crateb".to_vec()]);
    assert_eq!(r.world_effects, 1);
    // a local source (no server objects): the file's own models stay
    let mut local = source();
    let r = dc_client::run_headless(&mut local, None, &opts).expect("headless run");
    assert_eq!(
        r.world_models_drawn,
        vec![b"cratea".to_vec(), b"crateb".to_vec()]
    );
    assert_eq!(r.world_effects, 0);
}
