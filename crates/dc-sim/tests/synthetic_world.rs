//! Movement against a made-up world written with dc-world's synth module and read back with
//! `dc_world::World::parse`, so the whole path BSP file -> collision world -> movement is exercised.
//! Lanes along +Z (all geometry invented for the test):
//! A: floor, 20-unit step at z = 300 (climbable), wall at z = 1500
//! B: floor, 40-unit step at z = 300 (blocked)
//! C: floor, 30-degree ramp from z = 300 to 800, plateau
//! D: floor, 60-degree ramp from z = 300 (not climbable)
//! E: floor, blocker polygon (player clip) at z = 600
// SPEC: MOVEMENT_2009.md §9.5, §9.7, §10.1, §16.2 V-M6 (steps up to about 30.5 climb); FMT_DAT85.md §5.2, §9.1
#![cfg(feature = "world")]

mod common;
use common::*;

use dc_sim::from_world::{collision_world, WorldFilter, FILE_FIRST_CHILD_IS_NEGATIVE};
use dc_sim::{
    build_bsp, CollisionWorld, ConvexPoly, MovementParams, PlayerState, TickInput, SURF_SOLID, V3,
};
use dc_world::synth::{SynthModel, SynthWorld};
use dc_world::World;

fn ramp(x0: f32, x1: f32, z0: f32, z1: f32, rise: f32) -> ConvexPoly {
    quad([
        V3::new(x0, 0.0, z0),
        V3::new(x0, rise, z1),
        V3::new(x1, rise, z1),
        V3::new(x1, 0.0, z0),
    ])
}

/// The level geometry as convex polygons.
fn level() -> Vec<ConvexPoly> {
    let tan30 = 0.577_350_3f32;
    let tan60 = 1.732_050_8f32;
    let mut p = vec![top(-3000.0, 3000.0, -3000.0, 3000.0, 0.0)];
    // A: 20-unit step, x in [-1000, -600]
    p.push(wall_neg_z(-1000.0, -600.0, 0.0, 20.0, 300.0));
    p.push(top(-1000.0, -600.0, 300.0, 2000.0, 20.0));
    // B: 40-unit step, x in [-500, -100]
    p.push(wall_neg_z(-500.0, -100.0, 0.0, 40.0, 300.0));
    p.push(top(-500.0, -100.0, 300.0, 2000.0, 40.0));
    // C: 30-degree ramp, x in [0, 400]
    let rise_c = 500.0 * tan30;
    p.push(ramp(0.0, 400.0, 300.0, 800.0, rise_c));
    p.push(top(0.0, 400.0, 800.0, 2000.0, rise_c));
    // D: 60-degree ramp, x in [500, 900]
    let rise_d = 100.0 * tan60;
    p.push(ramp(500.0, 900.0, 300.0, 400.0, rise_d));
    p.push(top(500.0, 900.0, 400.0, 2000.0, rise_d));
    // wall across every lane
    p.push(wall_neg_z(-3000.0, 3000.0, 0.0, 2000.0, 1500.0));
    p
}

fn to_synth(polys: &[ConvexPoly]) -> SynthModel {
    let (nodes, root) = build_bsp(polys);
    let mut m = SynthModel {
        flags: 0,
        name: b"synthetic level".to_vec(),
        textures: vec![b"none".to_vec()],
        surfaces: vec![(SURF_SOLID, 0, 0)],
        box_min: [-3000.0, -10.0, -3000.0],
        box_max: [3000.0, 2000.0, 3000.0],
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
        let (first, second) = if FILE_FIRST_CHILD_IS_NEGATIVE {
            (neg, pos)
        } else {
            (pos, neg)
        };
        m.nodes.push((face, 0, enc(first), enc(second)));
    }
    m.root = enc(root);
    m
}

fn world_bytes() -> Vec<u8> {
    let blocker = wall_neg_z(1000.0, 1400.0, 0.0, 300.0, 600.0);
    SynthWorld {
        extent_min: [-3000.0, -100.0, -3000.0],
        extent_max: [3000.0, 2000.0, 3000.0],
        models: vec![to_synth(&level())],
        blockers: vec![(
            blocker.normal.to_array(),
            blocker.dist,
            blocker.verts.iter().map(|v| v.to_array()).collect(),
        )],
        tree_shape: vec![false],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![0, 0, 0],
        ..SynthWorld::default()
    }
    .build()
}

fn collision() -> CollisionWorld {
    let bytes = world_bytes();
    let w = World::parse(&bytes).expect("synthetic world parses");
    w.check_layout().expect("layout");
    assert_eq!(w.models.len(), 1);
    let cw = collision_world(&w, &WorldFilter::default());
    assert_eq!(cw.models.len(), 1, "the unlinked level model is selected");
    assert_eq!(cw.blocker_count(), 1);
    cw
}

/// Walks forward (+Z) for `secs` from a dropped start in lane `x`; returns the final state.
fn walk(w: &CollisionWorld, x: f32, secs: f32) -> PlayerState {
    let p = MovementParams::default();
    let dt = 1.0 / 60.0;
    let mut s = PlayerState::new(V3::new(x, 60.0, 0.0), DIMS, &p);
    for _ in 0..60 {
        s.step(&p, w, &idle(), dt);
    }
    assert!(s.on_ground, "settled in lane {x}: {s:?}");
    assert!(
        (s.pos.y - (DIMS.y + 0.1)).abs() < 0.05,
        "rests 0.1 above the floor: {:?}",
        s.pos
    );
    let mv = TickInput {
        commands: fwd(true),
        ..TickInput::default()
    };
    let mut last = s.pos;
    for _ in 0..((secs / dt) as usize) {
        s.step(&p, w, &mv, dt);
        assert!(s.pos.is_finite() && s.vel.is_finite());
        // the centre never crosses a solid front face (no tunnelling)
        assert!(
            w.ray(last, s.pos).is_none(),
            "centre crossed a surface: {last:?} -> {:?}",
            s.pos
        );
        last = s.pos;
    }
    s
}

#[test]
fn flat_floor_rest_height() {
    let w = collision();
    let s = walk(&w, 2000.0, 0.0);
    assert!(s.contact.is_some_and(|c| c.normal == V3::UP));
}

#[test]
fn twenty_unit_step_is_climbed_then_the_wall_stops() {
    let w = collision();
    let s = walk(&w, -800.0, 6.0);
    assert!(
        (s.pos.y - (20.0 + DIMS.y + 0.1)).abs() < 0.5,
        "on the step: {:?}",
        s.pos
    );
    assert!(
        (s.pos.z - (1500.0 - 16.0 - 0.1)).abs() < 0.5,
        "against the wall: {:?}",
        s.pos
    );
}

#[test]
fn forty_unit_step_blocks() {
    let w = collision();
    let s = walk(&w, -300.0, 4.0);
    assert!(s.pos.y < 41.0, "still on the floor: {:?}", s.pos);
    assert!(
        (s.pos.z - (300.0 - 16.0 - 0.1)).abs() < 0.5,
        "against the riser: {:?}",
        s.pos
    );
}

#[test]
fn thirty_degree_ramp_is_walkable() {
    let w = collision();
    let s = walk(&w, 200.0, 5.0);
    let plateau = 500.0 * 0.577_350_3 + DIMS.y;
    assert!(
        s.pos.y > plateau - 1.0 && s.pos.z > 800.0,
        "reached the plateau: {:?}",
        s.pos
    );
}

#[test]
fn sixty_degree_ramp_is_not_climbable() {
    let w = collision();
    let s = walk(&w, 700.0, 5.0);
    assert!(s.pos.z < 400.0, "did not pass the steep ramp: {:?}", s.pos);
    assert!(s.pos.y < 100.0 + DIMS.y, "stayed low: {:?}", s.pos);
}

#[test]
fn blocker_polygon_stops_the_body() {
    let w = collision();
    let s = walk(&w, 1200.0, 4.0);
    assert!(
        (s.pos.z - (600.0 - 16.0)).abs() < 0.5,
        "against the player clip: {:?}",
        s.pos
    );
}
