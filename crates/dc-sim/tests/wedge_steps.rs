//! SIM-28 through the whole step, on built worlds (no ROM): a wedged body with no escape is held exactly where it is
//! however long it pushes, a frozen wedged body is never moved, and a wedged body with an exit leaves through it.
// DC-DEFAULT: SIM-28 (proposed) the wedge test and its escape

use dc_sim::{
    CollModel, CollisionWorld, Commands, ConvexPoly, MovementParams, PlayerState, TickInput,
    SURF_SOLID, V3,
};

const DIMS: V3 = V3::new(15.0, 50.0, 15.0);
const DT: f32 = 1.0 / 60.0;

/// A quad whose front faces `toward`.
fn facing(c: [V3; 4], toward: V3) -> ConvexPoly {
    let q = ConvexPoly::from_corners(c.to_vec(), SURF_SOLID);
    if q.normal.dot(toward) > 0.0 {
        q
    } else {
        ConvexPoly::from_corners(vec![c[3], c[2], c[1], c[0]], SURF_SOLID)
    }
}

/// A closed room `lo..hi`, every face's front toward the inside; `open_px` leaves out the +x wall.
fn room(lo: V3, hi: V3, open_px: bool) -> Vec<ConvexPoly> {
    let (a, b) = (lo, hi);
    let mut f = vec![
        facing(
            [
                V3::new(a.x, a.y, a.z),
                V3::new(a.x, b.y, a.z),
                V3::new(a.x, b.y, b.z),
                V3::new(a.x, a.y, b.z),
            ],
            V3::new(1.0, 0.0, 0.0),
        ),
        facing(
            [
                V3::new(a.x, a.y, a.z),
                V3::new(b.x, a.y, a.z),
                V3::new(b.x, a.y, b.z),
                V3::new(a.x, a.y, b.z),
            ],
            V3::new(0.0, 1.0, 0.0),
        ),
        facing(
            [
                V3::new(a.x, b.y, a.z),
                V3::new(b.x, b.y, a.z),
                V3::new(b.x, b.y, b.z),
                V3::new(a.x, b.y, b.z),
            ],
            V3::new(0.0, -1.0, 0.0),
        ),
        facing(
            [
                V3::new(a.x, a.y, a.z),
                V3::new(b.x, a.y, a.z),
                V3::new(b.x, b.y, a.z),
                V3::new(a.x, b.y, a.z),
            ],
            V3::new(0.0, 0.0, 1.0),
        ),
        facing(
            [
                V3::new(a.x, a.y, b.z),
                V3::new(b.x, a.y, b.z),
                V3::new(b.x, b.y, b.z),
                V3::new(a.x, b.y, b.z),
            ],
            V3::new(0.0, 0.0, -1.0),
        ),
    ];
    if !open_px {
        f.push(facing(
            [
                V3::new(b.x, a.y, a.z),
                V3::new(b.x, b.y, a.z),
                V3::new(b.x, b.y, b.z),
                V3::new(b.x, a.y, b.z),
            ],
            V3::new(-1.0, 0.0, 0.0),
        ));
    }
    f
}

fn world(faces: &[ConvexPoly]) -> CollisionWorld {
    CollisionWorld::from_parts(vec![CollModel::from_polys(faces)], &[])
}

fn push_forward() -> TickInput<'static> {
    TickInput {
        commands: Commands {
            forward: true,
            run: true,
            ..Commands::default()
        },
        ..TickInput::default()
    }
}

#[test]
fn a_wedged_body_with_no_way_out_is_held_exactly_where_it_is() {
    let p = MovementParams::default();
    let w = world(&room(
        V3::new(-10.0, -10.0, -10.0),
        V3::new(10.0, 10.0, 10.0),
        false,
    ));
    let mut s = PlayerState::new(V3::ZERO, DIMS, &p);
    let input = push_forward();
    for _ in 0..600 {
        let ev = s.step(&p, &w, &input, DT);
        assert!(!ev.unwedged);
        assert_eq!(s.pos, V3::ZERO, "never moved through the room's walls");
    }
}

/// The closed room with its +x wall split by a slot 8 units wide (z -4..4) and a floor far below outside: the body
/// (radius 15) is wedged, the 1-unit probe passes the slot, so there is an escape.
fn slotted_room() -> Vec<ConvexPoly> {
    let (a, b) = (V3::new(-10.0, -10.0, -10.0), V3::new(10.0, 10.0, 10.0));
    let mut f = room(a, b, true);
    let wall = |z0: f32, z1: f32| {
        facing(
            [
                V3::new(b.x, a.y, z0),
                V3::new(b.x, b.y, z0),
                V3::new(b.x, b.y, z1),
                V3::new(b.x, a.y, z1),
            ],
            V3::new(-1.0, 0.0, 0.0),
        )
    };
    f.push(wall(a.z, -4.0));
    f.push(wall(4.0, b.z));
    f.push(ConvexPoly::from_corners(
        vec![
            V3::new(-500.0, -300.0, -500.0),
            V3::new(-500.0, -300.0, 500.0),
            V3::new(500.0, -300.0, 500.0),
            V3::new(500.0, -300.0, -500.0),
        ],
        SURF_SOLID,
    ));
    f
}

#[test]
fn a_frozen_wedged_body_is_never_moved() {
    let p = MovementParams::default();
    let w = world(&slotted_room());
    let shape = dc_sim::Cylinder::from_dims(DIMS);
    assert!(
        dc_sim::wedged(&w, &shape, V3::ZERO),
        "the slotted room wedges the body"
    );
    let mut s = PlayerState::new(V3::ZERO, DIMS, &p);
    s.set_freeze(true);
    let input = push_forward();
    for _ in 0..120 {
        assert!(!s.step(&p, &w, &input, DT).unwedged);
    }
    assert_eq!(s.pos, V3::ZERO);
    // released, the same body is freed within a quarter second
    s.set_freeze(false);
    let freed = (0..20).any(|_| s.step(&p, &w, &input, DT).unwedged);
    assert!(freed, "freed once the freeze is off");
    assert!(s.pos.x > 10.0, "out past the slotted wall: {:?}", s.pos);
}
