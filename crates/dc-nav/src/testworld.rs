//! Small synthetic collision worlds for the tests of the grid and of its users (no ROM data).

use std::sync::Arc;

use dc_sim::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID, V3};

/// A closed box (6 outward faces) from `lo` to `hi`.
pub fn solid_box(lo: [f32; 3], hi: [f32; 3]) -> CollModel {
    let p = |x: f32, y: f32, z: f32| V3::new(x, y, z);
    let (a, b) = (lo, hi);
    let face = |c: [V3; 4]| ConvexPoly::from_corners(c.to_vec(), SURF_SOLID);
    let faces = vec![
        // -x, +x
        face([
            p(a[0], a[1], a[2]),
            p(a[0], a[1], b[2]),
            p(a[0], b[1], b[2]),
            p(a[0], b[1], a[2]),
        ]),
        face([
            p(b[0], a[1], a[2]),
            p(b[0], b[1], a[2]),
            p(b[0], b[1], b[2]),
            p(b[0], a[1], b[2]),
        ]),
        // -z, +z
        face([
            p(a[0], a[1], a[2]),
            p(a[0], b[1], a[2]),
            p(b[0], b[1], a[2]),
            p(b[0], a[1], a[2]),
        ]),
        face([
            p(a[0], a[1], b[2]),
            p(b[0], a[1], b[2]),
            p(b[0], b[1], b[2]),
            p(a[0], b[1], b[2]),
        ]),
        // -y, +y
        face([
            p(a[0], a[1], a[2]),
            p(b[0], a[1], a[2]),
            p(b[0], a[1], b[2]),
            p(a[0], a[1], b[2]),
        ]),
        face([
            p(a[0], b[1], a[2]),
            p(a[0], b[1], b[2]),
            p(b[0], b[1], b[2]),
            p(b[0], b[1], a[2]),
        ]),
    ];
    CollModel::from_polys(&faces)
}

/// A floor at y = 0 and a wall across the straight line from (0, 40, 0) to (0, 40, 1200).
pub fn walled() -> Arc<CollisionWorld> {
    let models = vec![
        solid_box([-5000.0, -64.0, -5000.0], [5000.0, 0.0, 5000.0]),
        solid_box([-400.0, 0.0, 580.0], [400.0, 300.0, 620.0]),
    ];
    Arc::new(CollisionWorld::from_parts(models, &[]))
}

/// A floor and a cup open toward -z around the start (0, 40, 0): a back wall at z 300..340 across x -500..500 and
/// side walls from z -500 to 340 at x = +-500. The goal (0, 40, 1500) lies behind the back wall, so the way out
/// starts away from the goal.
pub fn cupped() -> Arc<CollisionWorld> {
    let models = vec![
        solid_box([-5000.0, -64.0, -5000.0], [5000.0, 0.0, 5000.0]),
        solid_box([-540.0, 0.0, 300.0], [540.0, 300.0, 340.0]),
        solid_box([-540.0, 0.0, -500.0], [-500.0, 300.0, 340.0]),
        solid_box([500.0, 0.0, -500.0], [540.0, 300.0, 340.0]),
    ];
    Arc::new(CollisionWorld::from_parts(models, &[]))
}
