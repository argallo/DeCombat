//! The movement laws against the worked numbers of MOVEMENT_2009.md (synthetic flat world, no ROM).
// SPEC: MOVEMENT_2009.md §6, §7, §8, §16.2 (V-M1 .. V-M5 expected shapes)

mod common;
use common::*;

use dc_sim::{bits, Commands, Container, Correction, MovementParams, PlayerState, TickInput, V3};

fn jump_apex_with(p: &MovementParams, hz: f32) -> f32 {
    let w = flat_world();
    let dt = 1.0 / hz;
    let mut s = settled(p, &w, dt);
    let y0 = s.pos.y;
    let jump = input(Commands {
        jump: true,
        ..Commands::default()
    });
    let ev = s.step(p, &w, &jump, dt);
    assert!(ev.jumped, "take-off on the first frame");
    let mut apex = s.pos.y;
    let mut landed = false;
    for _ in 0..((3.0 / dt) as usize) {
        let ev = s.step(p, &w, &idle(), dt);
        apex = apex.max(s.pos.y);
        landed |= ev.landed_from_jump;
    }
    assert!(landed, "landing reported");
    assert!(s.on_ground && !s.jumping);
    assert!(
        (s.pos.y - y0).abs() < 0.2,
        "back on the floor: {} vs {y0}",
        s.pos.y
    );
    apex - y0
}

fn jump_apex(hz: f32) -> f32 {
    jump_apex_with(&MovementParams::default(), hz)
}

#[test]
fn jump_apex_matches_the_spec_at_60_100_and_10_hz() {
    // SPEC: MOVEMENT_2009.md §16.2 V-M3 (apex about 50.9 at 60 frames/s, 50.3 at 100, 57.8 at 10), §8.3 dt^2/4
    let a60 = jump_apex(60.0);
    let a10 = jump_apex(10.0);
    assert!((a60 - 50.875).abs() < 0.01, "60 Hz apex {a60}");
    assert!((a10 - 57.75).abs() < 0.01, "10 Hz apex {a10}");
    // 100 Hz: the parabola of the dt^2/4 law gives 50.325 (the report's 50.33) only without the 0.1-unit
    // displacement drop of §8.1 step 5. With it (the 2009 default) the last upward step near the apex
    // (11 * 0.01 - 0.0275 = 0.0825 units) is dropped, so the apex is 50.2425.
    // SPEC: MOVEMENT_2009.md §8.1 step 5 (|out|^2 > 0.01, else the body does not move)
    let a100 = jump_apex(100.0);
    assert!((a100 - 50.2425).abs() < 0.01, "100 Hz apex {a100}");
    let no_drop = MovementParams {
        min_move_sq: 0.0,
        ..MovementParams::default()
    };
    let a100_parabola = jump_apex_with(&no_drop, 100.0);
    assert!(
        (a100_parabola - 50.325).abs() < 0.01,
        "100 Hz parabola {a100_parabola}"
    );
    assert!((jump_apex_with(&no_drop, 60.0) - 50.875).abs() < 0.01);
}

fn plateau(cmd: Commands, hz: f32) -> f32 {
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / hz;
    let mut s = settled(&p, &w, dt);
    let mv = input(cmd);
    for _ in 0..((2.0 / dt) as usize) {
        s.step(&p, &w, &mv, dt);
    }
    let z0 = s.pos.z;
    let n = (1.0 / dt) as usize;
    for _ in 0..n {
        s.step(&p, &w, &mv, dt);
    }
    assert!(s.on_ground);
    (s.pos.z - z0) / n as f32
}

#[test]
fn stance_speeds_plateau_at_walk_run_crouch() {
    // SPEC: MOVEMENT_2009.md §7 (walk 70, run 285, crouch 50), §6.3 (cap before acceleration), §8.4 (k = 5)
    // DC-DEFAULT: SIM-22 (the grounded step moves at the cap, the 2009 law's limit at the real client's 1-2 ms frames)
    let walk = plateau(fwd(false), 60.0) as f64;
    let run = plateau(fwd(true), 60.0) as f64;
    let crouch = plateau(
        Commands {
            forward: true,
            crouch: true,
            ..Commands::default()
        },
        60.0,
    ) as f64;
    assert!(
        (walk - (70.0) * (1.0 / 60.0f64)).abs() < 1e-3,
        "walk {walk}"
    );
    assert!((run - (285.0) * (1.0 / 60.0f64)).abs() < 1e-3, "run {run}");
    assert!(
        (crouch - (50.0) * (1.0 / 60.0f64)).abs() < 1e-3,
        "crouch {crouch}"
    );
}

#[test]
fn sprint_uses_the_figure_multiplier() {
    // SPEC: MOVEMENT_2009.md §7 step 2 ((add + 1) * run_speed * fig_sprint_mul, default 1.4)
    let sprint = plateau(
        Commands {
            forward: true,
            run: true,
            sprint: true,
            ..Commands::default()
        },
        60.0,
    ) as f64;
    assert!(
        (sprint - (285.0 * 1.4) * (1.0 / 60.0f64)).abs() < 1e-3,
        "sprint {sprint}"
    );
}

#[test]
fn penalty_scales_the_target_speed() {
    // SPEC: MOVEMENT_2009.md §7 step 5 ((1 - p) * s)
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    let mut mv = input(fwd(true));
    mv.mods.penalty = 0.2;
    for _ in 0..180 {
        s.step(&p, &w, &mv, dt);
    }
    let z0 = s.pos.z;
    for _ in 0..60 {
        s.step(&p, &w, &mv, dt);
    }
    let per = ((s.pos.z - z0) / 60.0) as f64;
    assert!((per - (228.0) * (1.0 / 60.0f64)).abs() < 1e-3, "{per}");
}

#[test]
fn ramp_starts_at_500_and_adds_6000_per_second() {
    // SPEC: MOVEMENT_2009.md §6.5 (first frame exactly START_ACCEL, then + ACCEL_RATE * frame_dt up to 3000)
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    let mv = input(fwd(true));
    let mut mags = Vec::new();
    for _ in 0..40 {
        s.step(&p, &w, &mv, dt);
        mags.push(s.acc.len());
    }
    assert!((mags[0] - 500.0).abs() < 1e-3, "{mags:?}");
    assert!((mags[1] - 600.0).abs() < 1e-2, "{mags:?}");
    assert!((mags[10] - 1500.0).abs() < 0.1, "{mags:?}");
    // 500 -> 3000 in 2500 / 6000 = 0.4167 s = 25 frames
    assert!((mags[26] - 3000.0).abs() < 1e-3, "{mags:?}");
    assert!(mags.iter().all(|&m| m <= 3000.0 + 1e-3));
}

#[test]
fn releasing_the_keys_stops_quickly() {
    // SPEC: MOVEMENT_2009.md §6.8 (slide to stop, run_speed / STOP_TIME), §8.4 (exp(-5 t) friction)
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    for _ in 0..120 {
        s.step(&p, &w, &input(fwd(true)), dt);
    }
    let mut frames = 0;
    while s.vel != V3::ZERO && frames < 60 {
        s.step(&p, &w, &idle(), dt);
        frames += 1;
    }
    // without the stop rule exp(-5t) alone would need ~0.8 s to drop below 5 units/s
    assert!(frames <= 8, "stopped after {frames} frames");
}

#[test]
fn airborne_acceleration_is_the_ramp_value_times_hop() {
    // SPEC: MOVEMENT_2009.md §6.4 (airborne: a0 *= AIR_CONTROL), §6.5 step 5 (AR := normalize(AR) * (sa + ft K)),
    // §6.7 (acc *= HOP_ACCEL_MUL * hop while hop < 1). As written, the ramp re-normalises the axis to the ramp
    // value, so AIR_CONTROL only matters on frames where the ramp is skipped (jump request or crouch).
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    s.step(
        &p,
        &w,
        &input(Commands {
            jump: true,
            ..Commands::default()
        }),
        dt,
    );
    assert!(s.jumping);
    assert_eq!(s.hop, 0.3);
    let strafe = input(Commands {
        right: true,
        ..Commands::default()
    });
    // first strafe frame: ANY_MOVE was already on (the JUMP bit), so no boost; sa = 500 + 100
    s.step(&p, &w, &strafe, dt);
    assert!((s.acc.xz().len() - 600.0 * 0.3).abs() < 1e-2, "{:?}", s.acc);
    let hop = s.hop;
    s.step(&p, &w, &strafe, dt);
    assert!(
        (s.acc.xz().len() - 700.0 * hop).abs() < 1e-2,
        "{:?} hop {hop}",
        s.acc
    );
}

#[test]
fn chained_jump_uses_the_hop_penalty() {
    // SPEC: MOVEMENT_2009.md §6.7 (hop := HOP_MIN on a jump, += frame_dt * HOP_RECOVER, jump_v uses hop)
    let p = MovementParams::default();
    // a ceiling 10 units above the head keeps the jump short, so the penalty has not recovered on landing
    // (a full jump lasts about 0.6 s, longer than the 0.58 s recovery)
    let head = DIMS.y * 2.0 + 0.1;
    let w = dc_sim::CollisionWorld::from_parts(
        vec![dc_sim::CollModel::from_polys(&[
            top(-1000.0, 1000.0, -1000.0, 1000.0, 0.0),
            ceiling(-1000.0, 1000.0, -1000.0, 1000.0, head + 10.0),
        ])],
        &[],
    );
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    let hold = input(Commands {
        jump: true,
        ..Commands::default()
    });
    let ev = s.step(&p, &w, &hold, dt);
    assert!(ev.jumped);
    assert_eq!(s.hop, 0.3);
    // let go once the body stands again and press at once: a new request (a held key would request nothing,
    // MOVEMENT_2009.md §5.3, DISASM cshell+0x1ffad3..0x1ffb75)
    let mut second = None;
    for _ in 0..120 {
        let hop_before = s.hop;
        let c = if s.on_ground && !s.jumping && s.jump_held {
            idle()
        } else {
            hold
        };
        let ev = s.step(&p, &w, &c, dt);
        if ev.jumped {
            second = Some((hop_before, s.vel.y));
            break;
        }
    }
    let (hop_before, vy) = second.expect("second take-off");
    assert!(hop_before < 1.0, "hop not yet recovered: {hop_before}");
    // take-off frame: v.y = 330 hop, then one leaving-branch integration step of gravity
    let want = 330.0 * hop_before - 1100.0 * dt;
    assert!((vy - want).abs() < 1e-3, "{vy} vs {want}");
}

#[test]
fn ladder_climbs_with_the_view_and_caps_at_200() {
    // SPEC: MOVEMENT_2009.md §11.4 (speed 200, gravity off, acceleration along the full view direction)
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    let ladder = [Container::ladder()];
    let climb = TickInput {
        commands: fwd(false),
        pitch: -1.2, // looking up
        containers: &ladder,
        ..TickInput::default()
    };
    let y0 = s.pos.y;
    for _ in 0..120 {
        s.step(&p, &w, &climb, dt);
        assert!(s.vel.y <= 200.0 + 50.0, "{:?}", s.vel);
    }
    assert!(s.on_ladder && !s.gravity_on);
    assert!(s.pos.y > y0 + 100.0, "climbed: {} from {y0}", s.pos.y);
    let hover = s.pos.y;
    // let go of the keys on the ladder: no gravity, the body stays put (friction only)
    for _ in 0..60 {
        s.step(
            &p,
            &w,
            &TickInput {
                containers: &ladder,
                ..TickInput::default()
            },
            dt,
        );
    }
    assert!(s.pos.y >= hover - 1.0, "no gravity on a ladder");
}

#[test]
fn controller_state_3_freezes_motion_and_blocks_jump() {
    // SPEC: MOVEMENT_2009.md §12.5 (state 3 zeroes acceleration and velocity every frame)
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    let z0 = s.pos.z;
    let st3 = TickInput {
        commands: Commands {
            forward: true,
            jump: true,
            ..Commands::default()
        },
        ctrl_state: 3,
        ..TickInput::default()
    };
    for _ in 0..30 {
        let ev = s.step(&p, &w, &st3, dt);
        assert!(!ev.jumped);
    }
    assert_eq!(s.pos.z, z0);
    assert_eq!(s.vel, V3::ZERO);
}

#[test]
fn crouch_eye_dip_blocks_jumping_until_it_returns() {
    // SPEC: MOVEMENT_2009.md §5.3 (JUMP removed while the crouch eye offset is non-zero), §13.4 (rates 75)
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    let crouch = input(Commands {
        crouch: true,
        ..Commands::default()
    });
    for _ in 0..30 {
        s.step(&p, &w, &crouch, dt);
    }
    assert!(s.controls & bits::CROUCH != 0);
    assert_eq!(s.eye_dip, -13.0);
    // release crouch and hold jump: the first frames cannot jump
    let jump = input(Commands {
        jump: true,
        ..Commands::default()
    });
    let ev = s.step(&p, &w, &jump, dt);
    assert!(!ev.jumped, "eye still dipped");
    let mut jumped_at = None;
    for i in 0..30 {
        // release and press again so the latch sees a 0 -> 1 edge once the eye is back up
        let c = if i % 2 == 0 { idle() } else { jump };
        if s.step(&p, &w, &c, dt).jumped {
            jumped_at = Some(i);
            break;
        }
    }
    assert!(
        s.eye_dip == 0.0 && jumped_at.is_some(),
        "{jumped_at:?} {}",
        s.eye_dip
    );
}

#[test]
fn correction_teleports_sets_box_and_seq() {
    // SPEC: MOVEMENT_2009.md §12.4 flag 0x0020 (move_seq := seq; box (0.5)^3; teleport; set_box(dims, 1) else 0)
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    s.step(&p, &w, &input(fwd(true)), dt);
    let fitted = s.apply_correction(
        &p,
        &w,
        &Correction {
            seq: 7,
            pos: V3::new(500.0, DIMS.y + 0.1, 200.0),
            dims: DIMS,
            velocity: None,
        },
    );
    assert!(fitted);
    assert_eq!(s.move_seq, 7);
    assert_eq!(s.pos, V3::new(500.0, DIMS.y + 0.1, 200.0));
    assert_eq!((s.vel, s.acc, s.dims), (V3::ZERO, V3::ZERO, DIMS));
    // a box that cannot fit (embedded in the floor) is forced and reported
    let forced = s.apply_correction(
        &p,
        &w,
        &Correction {
            seq: 8,
            pos: V3::new(0.0, 10.0, 0.0),
            dims: DIMS,
            velocity: Some(V3::new(1.0, 2.0, 3.0)),
        },
    );
    assert!(!forced);
    assert_eq!(s.dims, DIMS);
    assert_eq!(s.vel, V3::new(1.0, 2.0, 3.0));
    // the body settles again after a correction
    let mut s2 = settled(&p, &w, dt);
    s2.apply_correction(
        &p,
        &w,
        &Correction {
            seq: 1,
            pos: V3::new(0.0, 60.0, 0.0),
            dims: DIMS,
            velocity: None,
        },
    );
    for _ in 0..120 {
        s2.step(&p, &w, &idle(), dt);
    }
    assert!(
        s2.on_ground && (s2.pos.y - (DIMS.y + 0.1)).abs() < 0.05,
        "{:?}",
        s2.pos
    );
}

#[test]
fn freeze_holds_the_position_until_released() {
    // SPEC: MOVEMENT_2009.md §12.4 flags 0x0080 / 0x0100
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    s.set_freeze(true);
    let at = s.pos;
    for _ in 0..30 {
        s.step(&p, &w, &input(fwd(true)), dt);
    }
    assert!(
        (s.pos - at).len() < 6.0,
        "at most one step of drift per frame: {:?}",
        s.pos
    );
    s.set_freeze_release(true);
    for _ in 0..30 {
        s.step(&p, &w, &input(fwd(true)), dt);
    }
    assert!(s.pos.z > at.z + 20.0);
}

#[test]
fn walking_direction_follows_yaw() {
    // SPEC: MOVEMENT_2009.md §6.3 (F = rotated +Z; R = rotated +X), §5.2 (LEFT = -X, RIGHT = +X)
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    let start = s.pos;
    let mv = TickInput {
        commands: fwd(true),
        yaw: core::f32::consts::FRAC_PI_2,
        ..TickInput::default()
    };
    for _ in 0..60 {
        s.step(&p, &w, &mv, dt);
    }
    let d = s.pos - start;
    assert!(d.x > 200.0 && d.z.abs() < 1.0, "{d:?}");
    let mut s = settled(&p, &w, dt);
    let start = s.pos;
    for _ in 0..60 {
        s.step(
            &p,
            &w,
            &input(Commands {
                right: true,
                run: true,
                ..Commands::default()
            }),
            dt,
        );
    }
    let d = s.pos - start;
    assert!(
        d.x > 200.0 && d.z.abs() < 1.0,
        "right strafes along +X: {d:?}"
    );
}

#[test]
fn frozen_branch_halves_horizontal_speed() {
    // SPEC: MOVEMENT_2009.md §6 (frozen branch: v = (v.x 0.5, min(v.y, 0), v.z 0.5))
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = settled(&p, &w, dt);
    for _ in 0..60 {
        s.step(&p, &w, &input(fwd(true)), dt);
    }
    let mut frozen = input(fwd(true));
    frozen.movement_allowed = false;
    let z0 = s.pos.z;
    for _ in 0..60 {
        s.step(&p, &w, &frozen, dt);
    }
    assert!(
        s.pos.z - z0 < 10.0,
        "stops within a few frames: {}",
        s.pos.z - z0
    );
    assert!(
        s.controls & bits::FWD == 0,
        "direction bits need movement allowed"
    );
}

#[test]
fn spawn_drop_lands_and_reports() {
    // SPEC: MOVEMENT_2009.md §10.2 (landing after a fall without a jump sends e = 3; the fall notice e = 2
    // needs 150 units of air below)
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / 60.0;
    let mut s = PlayerState::new(V3::new(0.0, 400.0, 0.0), DIMS, &p);
    let mut codes = Vec::new();
    for _ in 0..180 {
        let ev = s.step(&p, &w, &idle(), dt);
        codes.extend(ev.notice_codes());
    }
    assert_eq!(codes, vec![2, 3], "fall started, then landed");
    assert!(
        s.on_ground && (s.pos.y - (DIMS.y + 0.1)).abs() < 0.05,
        "{:?}",
        s.pos
    );
}

/// A body at rest on a box whose model leaves the collision set (a destroyed breakable) falls on the next idle steps
/// once the riders are detached; without the detach the stale standing contact rests it in the air (the user's
/// "float until I move").
// DISASM: engine+0x70c70 (riders of a removed object lose their standing contact)
// DC-DEFAULT: CLI-1369 (proposed)
#[test]
fn a_body_on_a_removed_box_falls_without_input() {
    use dc_sim::{CollModel, CollisionWorld};
    let p = MovementParams::default();
    let dt = 1.0 / 60.0;
    let floor = CollModel::from_polys(&[top(-5000.0, 5000.0, -5000.0, 5000.0, 0.0)]);
    let crate_top = CollModel::from_polys(&[top(-50.0, 50.0, -50.0, 50.0, 48.0)]);
    let mut w = CollisionWorld::from_parts(vec![floor, crate_top], &[]);
    let mut s = PlayerState::new(V3::new(0.0, 48.0 + DIMS.y + 5.0, 0.0), DIMS, &p);
    for _ in 0..120 {
        s.step(&p, &w, &idle(), dt);
    }
    assert!(s.on_ground && s.vel == V3::ZERO, "on the box: {s:?}");
    assert_eq!(s.contact.map(|c| c.model), Some(1));
    let on_box = s.pos.y;
    // the box is destroyed: its model is swapped for an empty one (the client's stand-in, CLI-231)
    w.models[1] = CollModel::new(&[], &[], None);
    let mut stale = s.clone();
    for _ in 0..60 {
        stale.step(&p, &w, &idle(), dt);
    }
    assert_eq!(
        stale.pos.y, on_box,
        "without the detach the body rests in the air"
    );
    assert!(!s.detach_from_models(&[0]), "the floor did not leave");
    assert!(s.detach_from_models(&[1]));
    for _ in 0..60 {
        s.step(&p, &w, &idle(), dt);
    }
    assert!(
        (s.pos.y - DIMS.y).abs() < 0.5 && s.on_ground,
        "fell to the floor: {s:?}"
    );
    assert_eq!(s.contact.map(|c| c.model), Some(0));
}
