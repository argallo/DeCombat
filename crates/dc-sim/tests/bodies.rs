//! Solid character bodies (w1-bodies): the own mover against the boxes of other characters, through the full
//! `PlayerState::step` (velocity law, integrator, slide loop, ground probe) on a made-up floor. All numbers are
//! made up for the tests.
// SPEC: MOVEMENT_2009.md §9.6 stage 2 (objects: the box test engine+0x75860, the 0.1 back-off)
// DC-DEFAULT: SIM-13 (proposed) the own mover collides with the remote characters' boxes

mod common;
use common::*;

use dc_sim::collide::{Body, BodySet, WithBodies, BODY_MODEL};
use dc_sim::{Collider, Commands, MovementParams, PlayerState, TickInput, V3};

const DT: f32 = 1.0 / 60.0;

fn standing_body(x: f32, z: f32) -> Body {
    Body {
        id: 42,
        centre: V3::new(x, DIMS.y, z),
        half: DIMS,
    }
}

fn set(list: &[Body]) -> BodySet {
    let mut s = BodySet::default();
    for b in list {
        assert!(s.push(*b));
    }
    s
}

fn run_yaw(yaw: f32) -> TickInput<'static> {
    TickInput {
        commands: fwd(true),
        yaw,
        ..TickInput::default()
    }
}

/// The grown box face the mover's centre stops at, along -z of a body at `z`.
fn face_z(z: f32) -> f32 {
    z - DIMS.z - DIMS.x
}

#[test]
fn running_into_a_standing_body_stops_at_its_box() {
    let p = MovementParams::default();
    let floor = flat_world();
    let bodies = set(&[standing_body(0.0, 200.0)]);
    let w = WithBodies {
        world: &floor,
        bodies: &bodies,
    };
    let mut s = settled(&p, &floor, DT);
    let mut max_z = f32::MIN;
    for _ in 0..120 {
        s.step(&p, &w, &run_yaw(0.0), DT);
        max_z = max_z.max(s.pos.z);
    }
    assert!(max_z < face_z(200.0), "never inside the box: {max_z}");
    assert!(
        s.pos.z > face_z(200.0) - 1.0,
        "stopped at the box, not before it: {:?}",
        s.pos
    );
    assert!(s.pos.x.abs() < 1e-3, "no sideways drift: {:?}", s.pos);
    assert!(s.on_ground, "still standing on the floor");
    // without the body the same run goes through
    let mut free = settled(&p, &floor, DT);
    for _ in 0..120 {
        free.step(&p, &floor, &run_yaw(0.0), DT);
    }
    assert!(free.pos.z > 300.0, "{:?}", free.pos);
}

#[test]
fn running_into_a_body_at_an_angle_slides_along_it_and_past_it() {
    let p = MovementParams::default();
    let floor = flat_world();
    // the run (20 degrees to +x) meets the body's -z face 8 units right of its centre line
    let (bx, bz) = (35.0f32, 150.0f32);
    let bodies = set(&[standing_body(bx, bz)]);
    let w = WithBodies {
        world: &floor,
        bodies: &bodies,
    };
    let mut s = settled(&p, &floor, DT);
    let yaw = 20.0f32.to_radians();
    let grown = DIMS.x + DIMS.x;
    let mut blocked_ticks = 0u32;
    let mut slid_x = Vec::new();
    for _ in 0..180 {
        let before = s.pos;
        s.step(&p, &w, &run_yaw(yaw), DT);
        let within = (s.pos.x - bx).abs() < DIMS.x;
        assert!(
            !(within && s.pos.z > face_z(bz) && s.pos.z < bz + grown),
            "inside the body's box: {:?}",
            s.pos
        );
        if (s.pos.x - bx).abs() < grown && (s.pos.z - face_z(bz)).abs() < 0.5 {
            blocked_ticks += 1;
            slid_x.push(s.pos.x - before.x);
        }
    }
    assert!(blocked_ticks > 3, "touched the face: {blocked_ticks}");
    assert!(
        slid_x.iter().all(|&dx| dx > 0.0),
        "slides towards +x along the face: {slid_x:?}"
    );
    assert!(s.pos.z > bz + grown, "got past the body: {:?}", s.pos);
    assert!(s.pos.x > bx + grown, "on the +x side of it: {:?}", s.pos);
}

#[test]
fn a_body_can_be_stood_on() {
    let p = MovementParams::default();
    let floor = flat_world();
    let bodies = set(&[standing_body(0.0, 0.0)]);
    let w = WithBodies {
        world: &floor,
        bodies: &bodies,
    };
    let mut s = PlayerState::new(V3::new(5.0, 300.0, 5.0), DIMS, &p);
    for _ in 0..180 {
        s.step(&p, &w, &idle(), DT);
    }
    let top = 2.0 * DIMS.y + DIMS.y;
    assert!(
        (s.pos.y - top).abs() < 1.0,
        "rests on the head: {:?}",
        s.pos
    );
    assert!(s.on_ground);
    assert_eq!(s.contact.map(|c| c.model), Some(BODY_MODEL));
}

#[test]
fn the_body_stage_is_deterministic_and_the_set_is_rebuilt_without_drift() {
    let p = MovementParams::default();
    let floor = flat_world();
    let run = |rebuild_every_tick: bool| {
        let mut bodies = set(&[standing_body(30.0, 180.0), standing_body(-60.0, 260.0)]);
        let mut s = settled(&p, &floor, DT);
        for i in 0..240u32 {
            if rebuild_every_tick {
                // the client rebuilds the set every frame from the drawn remotes; the second body walks
                bodies.clear();
                bodies.push(standing_body(30.0, 180.0));
                bodies.push(standing_body(-60.0 + i as f32 * 0.5, 260.0));
            }
            let w = WithBodies {
                world: &floor,
                bodies: &bodies,
            };
            let yaw = (i as f32 * 0.01).sin() * 0.6;
            s.step(&p, &w, &run_yaw(yaw), DT);
        }
        s.digest()
    };
    assert_eq!(run(false), run(false));
    assert_eq!(run(true), run(true));
    // an empty set is the world alone, bit for bit
    let empty = BodySet::default();
    let mut a = settled(&p, &floor, DT);
    let mut b = settled(&p, &floor, DT);
    for _ in 0..120 {
        a.step(&p, &floor, &run_yaw(0.3), DT);
        let w = WithBodies {
            world: &floor,
            bodies: &empty,
        };
        b.step(&p, &w, &run_yaw(0.3), DT);
    }
    assert_eq!(a.digest(), b.digest());
}

#[test]
fn moving_away_from_an_overlapping_body_is_free() {
    // two characters placed into each other (a spawn on the same point) can walk apart
    let p = MovementParams::default();
    let floor = flat_world();
    let mut s = settled(&p, &floor, DT);
    let bodies = set(&[standing_body(0.0, 10.0)]);
    let w = WithBodies {
        world: &floor,
        bodies: &bodies,
    };
    assert!(w.overlaps(&dc_sim::Cylinder::from_dims(DIMS), s.pos));
    let back = TickInput {
        commands: Commands {
            backward: true,
            run: true,
            ..Commands::default()
        },
        ..TickInput::default()
    };
    for _ in 0..60 {
        s.step(&p, &w, &back, DT);
    }
    assert!(s.pos.z < -100.0, "walked out: {:?}", s.pos);
}

// ---- fix-hang: every start position against bodies terminates (no unbounded loop in the body / slide stage) ----

/// Runs `f` on a thread and fails when it does not finish within `secs` (a hang becomes a test failure, not a stuck
/// test run).
fn within(secs: u64, what: &str, f: impl FnOnce() + Send + 'static) {
    let (tx, rx) = std::sync::mpsc::channel();
    let h = std::thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    match rx.recv_timeout(std::time::Duration::from_secs(secs)) {
        Ok(()) => h.join().expect("no panic"),
        Err(_) => panic!("{what}: did not finish within {secs} s (unbounded loop)"),
    }
}

/// 600 ticks of every command mix (run, sprint, jump, crouch, strafe, turning) from `start` among `bodies`.
fn soak_from(start: V3, bodies: Vec<Body>) {
    let p = MovementParams::default();
    let floor = flat_world();
    let set = set(&bodies);
    let w = WithBodies {
        world: &floor,
        bodies: &set,
    };
    let mut s = settled(&p, &floor, DT);
    s.pos = start;
    for i in 0..600u32 {
        let mut c = fwd(true);
        c.sprint = i % 90 < 45;
        c.jump = i % 70 == 10;
        c.crouch = i % 150 > 120;
        c.left = i % 40 < 10;
        c.right = (20..30).contains(&(i % 40));
        c.backward = i % 200 > 170;
        let input = TickInput {
            commands: c,
            yaw: i as f32 * 0.37,
            ..TickInput::default()
        };
        s.step(&p, &w, &input, DT);
        assert!(
            s.pos.x.is_finite() && s.pos.y.is_finite() && s.pos.z.is_finite(),
            "tick {i}: {:?}",
            s.pos
        );
    }
}

#[test]
fn a_start_inside_a_body_terminates_every_tick() {
    let y = DIMS.y + 0.5;
    let cases: Vec<(&str, V3, Vec<Body>)> = vec![
        // respawned onto a body: the own centre at the body's centre (fully enclosed)
        (
            "enclosed",
            V3::new(0.0, y, 0.0),
            vec![standing_body(0.0, 0.0)],
        ),
        // partly inside, off-centre (the killer standing on the victim's spot)
        (
            "overlapping",
            V3::new(10.0, y, 7.0),
            vec![standing_body(0.0, 0.0)],
        ),
        // exactly on the grown box's face and corner
        (
            "touching face",
            V3::new(0.0, y, -(2.0 * DIMS.z)),
            vec![standing_body(0.0, 0.0)],
        ),
        (
            "touching corner",
            V3::new(2.0 * DIMS.x, y, 2.0 * DIMS.z),
            vec![standing_body(0.0, 0.0)],
        ),
        // two bodies on the same spot and one beside, all overlapping the start
        (
            "stacked",
            V3::new(3.0, y, 3.0),
            vec![
                standing_body(0.0, 0.0),
                standing_body(0.0, 0.0),
                standing_body(20.0, 0.0),
            ],
        ),
        // a ring of bodies around the start (no way out)
        (
            "ring",
            V3::new(0.0, y, 0.0),
            (0..8)
                .map(|k| {
                    let a = k as f32 * std::f32::consts::FRAC_PI_4;
                    standing_body(30.0 * a.cos(), 30.0 * a.sin())
                })
                .collect(),
        ),
        // a lying (flat) body under the feet
        (
            "lying",
            V3::new(0.0, y, 0.0),
            vec![Body {
                id: 9,
                centre: V3::new(0.0, 8.0, 0.0),
                half: V3::new(40.0, 8.0, 16.0),
            }],
        ),
    ];
    for (name, start, bodies) in cases {
        within(20, name, move || soak_from(start, bodies));
    }
}
