//! Determinism: the same inputs give bit-identical states, run to run, target to target (i686 / x86_64) and
//! with or without the `std` feature (hardware vs software square root). The golden digest below was
//! produced by this test; any change to the arithmetic changes it on purpose and must be re-recorded.
// SPEC: MOVEMENT_2009.md §15 items 1-2 (same crate on server and client, no FMA, one exp implementation)

mod common;
use common::*;

use dc_sim::{
    CollModel, CollisionWorld, Commands, Container, Correction, MoveMode, MovementParams,
    PlayerState, TickInput, V3,
};

/// Floor, a 20-unit step, a 30-degree ramp, a wall and a low ceiling (all made up).
fn course() -> CollisionWorld {
    let ramp_rise = 300.0f32 * 0.577_350_3; // tan 30
    let polys = vec![
        top(-2000.0, 2000.0, -2000.0, 2000.0, 0.0),
        // step: riser at z = 300, top from z = 300 to 600 at y = 20
        wall_neg_z(-2000.0, 2000.0, 0.0, 20.0, 300.0),
        top(-2000.0, 2000.0, 300.0, 600.0, 20.0),
        // ramp from (z = 600, y = 20) up to (z = 900, y = 20 + rise)
        quad([
            V3::new(-2000.0, 20.0, 600.0),
            V3::new(-2000.0, 20.0 + ramp_rise, 900.0),
            V3::new(2000.0, 20.0 + ramp_rise, 900.0),
            V3::new(2000.0, 20.0, 600.0),
        ]),
        // far wall
        wall_neg_z(-2000.0, 2000.0, 0.0, 1000.0, 1500.0),
        // a low ceiling over x < -500
        ceiling(-2000.0, -500.0, -2000.0, 300.0, 100.0),
    ];
    CollisionWorld::from_parts(vec![CollModel::from_polys(&polys)], &[])
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

/// 30 s at 60 Hz of pseudo-random commands, turning, a ladder stretch, a fly stretch and two corrections.
fn run() -> (PlayerState, Vec<u64>, (u32, f32, bool)) {
    let p = MovementParams::default();
    let w = course();
    let dt = 1.0 / 60.0;
    let mut s = PlayerState::new(V3::new(0.0, 80.0, 0.0), DIMS, &p);
    let mut rng = Rng(0x1234_5678);
    let ladder = [Container::ladder()];
    let mut cmd = Commands::default();
    let mut yaw = 0.0f32;
    let mut digests = Vec::new();
    let (mut jumps, mut max_y, mut laddered) = (0u32, f32::MIN, false);
    for tick in 0..1800u32 {
        if tick % 15 == 0 {
            let r = rng.next();
            cmd = Commands {
                forward: r & 1 != 0,
                backward: r & 2 != 0 && r & 1 == 0,
                left: r & 4 != 0,
                right: r & 8 != 0 && r & 4 == 0,
                jump: r & 16 != 0,
                crouch: r & 32 != 0 && r & 16 == 0,
                run: r & 64 != 0,
                sprint: r & 128 != 0,
                ..Commands::default()
            };
        }
        yaw += ((rng.next() % 200) as f32 - 100.0) * 0.0005;
        let containers: &[Container] = if (600..700).contains(&tick) {
            &ladder
        } else {
            &[]
        };
        let mode = if (900..960).contains(&tick) {
            MoveMode::Fly
        } else {
            MoveMode::Walk
        };
        let input = TickInput {
            commands: cmd,
            yaw,
            pitch: -0.3,
            mode,
            containers,
            ..TickInput::default()
        };
        jumps += s.step(&p, &w, &input, dt).jumped as u32;
        max_y = max_y.max(s.pos.y);
        laddered |= s.on_ladder;
        if tick == 1000 || tick == 1500 {
            s.apply_correction(
                &p,
                &w,
                &Correction {
                    seq: (tick / 500) as u8,
                    pos: V3::new(100.0, 60.0, 100.0),
                    dims: DIMS,
                    velocity: None,
                },
            );
        }
        assert!(s.pos.is_finite() && s.vel.is_finite(), "tick {tick}: {s:?}");
        digests.push(s.digest());
    }
    (s, digests, (jumps, max_y, laddered))
}

/// Digest of the final state of [`run`] (recorded on i686 with the default features; must be equal on
/// x86_64 and with `--no-default-features`). Re-recorded 2026-10-25 (branch sim-edges) for the 2009 edge sweep
/// (engine+0x76120), stair landing and ground probe by polygon plane (engine+0x78c50, engine+0x79210); the same
/// value on i686, x86_64 and without default features. Re-recorded 2026-10-26 (branch sim-sprint-jump): a held
/// jump key requests one jump only (the jump-key-held flag of MOVEMENT_2009.md §5.3, DISASM cshell+0x1ffad3), and
/// the flag is part of the digest. Re-recorded 2026-10-27 (branch w2-sim): (1) the airborne-after-jump flag
/// (mover+0x80, now `jump_flag`) ends one step after the take-off (MOVEMENT_2009.md §12.3 M12, cshell+0x1ffec0), so
/// air control, container drag and the fall probe act during the rest of each flight: the final position moves by
/// about 8.8 units (x -267.41 -> -270.83, z -28.07 -> -36.18; y unchanged); (2) the digest also hashes the new
/// state (sprint gauge value and mode, the kept take-off contact, `jump_flag`); this run has no gauge rates, so the
/// gauge adds no motion. Re-recorded 2026-10-27 (branch sim-fixes5): the grounded step moves at the target speed
/// (SIM-22; the 60 Hz cap-then-accelerate overshoot removed).
const GOLDEN: u64 = 0x6688_c4e1_6649_27dd;

#[test]
fn same_inputs_same_bits() {
    let (a, da, (jumps, max_y, laddered)) = run();
    let (b, db, _) = run();
    // the script exercises jumping, stepping onto the 20-unit block or the ramp, and the ladder
    assert!(
        jumps >= 3 && max_y > 60.0 && laddered,
        "coverage: jumps {jumps}, max y {max_y}, ladder {laddered}"
    );
    assert_eq!(da, db, "per-tick digests differ between two runs");
    assert_eq!(a, b);
    assert_eq!(a.pos.bits(), b.pos.bits());
    println!("final digest {:#018x} pos {:?}", a.digest(), a.pos);
    if GOLDEN != 0 {
        assert_eq!(
            a.digest(),
            GOLDEN,
            "digest differs from the recorded golden value"
        );
    }
}

#[test]
fn digest_sees_every_change() {
    let p = MovementParams::default();
    let s = PlayerState::new(V3::new(0.0, 80.0, 0.0), DIMS, &p);
    let mut t = s.clone();
    t.vel.x = f32::from_bits(t.vel.x.to_bits() ^ 1);
    assert_ne!(s.digest(), t.digest());
    let mut t = s.clone();
    t.hop = 0.999_999_94;
    assert_ne!(s.digest(), t.digest());
}
