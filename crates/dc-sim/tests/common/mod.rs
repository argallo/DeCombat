//! Shared helpers for the integration tests. All geometry here is made up for the tests.
#![allow(dead_code)]

use dc_sim::{
    CollModel, CollisionWorld, Commands, ConvexPoly, MovementParams, PlayerState, TickInput,
    SURF_SOLID, V3,
};

/// A made-up standing box for the synthetic tests (not a ROM value).
pub const DIMS: V3 = V3::new(16.0, 40.0, 16.0);

pub fn quad(c: [V3; 4]) -> ConvexPoly {
    ConvexPoly::from_corners(c.to_vec(), SURF_SOLID)
}

/// Horizontal rectangle at height `y`, facing up.
pub fn top(x0: f32, x1: f32, z0: f32, z1: f32, y: f32) -> ConvexPoly {
    quad([
        V3::new(x0, y, z0),
        V3::new(x0, y, z1),
        V3::new(x1, y, z1),
        V3::new(x1, y, z0),
    ])
}

/// Horizontal rectangle at height `y`, facing down.
pub fn ceiling(x0: f32, x1: f32, z0: f32, z1: f32, y: f32) -> ConvexPoly {
    quad([
        V3::new(x0, y, z0),
        V3::new(x1, y, z0),
        V3::new(x1, y, z1),
        V3::new(x0, y, z1),
    ])
}

/// Vertical rectangle in the plane z = z0 facing -z.
pub fn wall_neg_z(x0: f32, x1: f32, y0: f32, y1: f32, z0: f32) -> ConvexPoly {
    quad([
        V3::new(x0, y0, z0),
        V3::new(x0, y1, z0),
        V3::new(x1, y1, z0),
        V3::new(x1, y0, z0),
    ])
}

/// A big flat floor at y = 0.
pub fn flat_world() -> CollisionWorld {
    CollisionWorld::from_parts(
        vec![CollModel::from_polys(&[top(
            -100_000.0, 100_000.0, -100_000.0, 100_000.0, 0.0,
        )])],
        &[],
    )
}

pub fn input(c: Commands) -> TickInput<'static> {
    TickInput {
        commands: c,
        ..TickInput::default()
    }
}

pub fn idle() -> TickInput<'static> {
    TickInput::default()
}

pub fn fwd(run: bool) -> Commands {
    Commands {
        forward: true,
        run,
        ..Commands::default()
    }
}

/// A body dropped onto the flat floor and settled (on the ground, at rest).
pub fn settled(p: &MovementParams, w: &CollisionWorld, dt: f32) -> PlayerState {
    let mut s = PlayerState::new(V3::new(0.0, DIMS.y + 5.0, 0.0), DIMS, p);
    for _ in 0..((2.0 / dt) as usize) {
        s.step(p, w, &idle(), dt);
    }
    assert!(s.on_ground && s.contact.is_some(), "settled: {s:?}");
    assert_eq!(s.vel, V3::ZERO);
    s
}
