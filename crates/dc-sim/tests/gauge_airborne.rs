//! The sprint gauge (M13), the blocked sprint (M16 input) and the airborne-after-jump flag (m24), each at several
//! fixed step rates: the rules depend on elapsed time, not on the number of steps.
// EVIDENCE: cshell+0x95850 (gauge law), cshell+0x1ff980 (sampling: SPRINT and the gauge mode), cshell+0x1ffec0
// (ground test: a standing contact ends the airborne-after-jump flag and sends the land notice)
// SPEC: MOVEMENT_2009.md §5.5, §7 step 2 (a blocked sprint walks), §10.2, §12.3 (M12: the land notice follows the
// take-off one frame later)

mod common;
use common::*;

use dc_sim::{
    bits, Commands, GaugeMode, MovementParams, SpeedMods, StaminaRates, StepEvents, TickInput,
};

// made-up rates, chosen to differ from every ROM figure row
const RATES: StaminaRates = StaminaRates {
    max: 120,
    drain: 40,
    regen: 15,
};

fn mods() -> SpeedMods {
    SpeedMods {
        stamina: Some(RATES),
        ..SpeedMods::default()
    }
}

fn sprint_keys(sprint: bool) -> TickInput<'static> {
    TickInput {
        commands: Commands {
            forward: true,
            run: true,
            sprint,
            ..Commands::default()
        },
        mods: mods(),
        ..TickInput::default()
    }
}

#[test]
fn a_full_gauge_sprints_max_over_drain_seconds_at_every_rate() {
    let p = MovementParams::default();
    let w = flat_world();
    for hz in [30u32, 60, 144, 240] {
        let dt = 1.0 / hz as f32;
        let mut s = settled(&p, &w, dt);
        let input = sprint_keys(true);
        let mut last_sprint = None;
        for t in 0..5 * hz {
            s.step(&p, &w, &input, dt);
            if s.controls & bits::SPRINT != 0 {
                last_sprint = Some(t);
            }
        }
        // a sprint step needs the value truncated above 0: the last one starts at (max - 1) / drain
        let dur = (last_sprint.expect("it sprinted") + 1) as f32 * dt;
        let want = (RATES.max - 1) as f32 / RATES.drain as f32;
        assert!(
            (dur - want).abs() <= dt + 1e-3,
            "{hz} Hz: sprinted {dur} s, want {want} s"
        );
        // empty with the keys still held: no sprint, the gauge held at 0, the run speed
        assert_eq!(s.controls & bits::SPRINT, 0, "{hz} Hz");
        assert_eq!(s.stamina.mode, GaugeMode::Spent, "{hz} Hz");
        assert_eq!(s.stamina.value, Some(0.0), "{hz} Hz");
        assert!(
            (s.speed_cap(&p, &input) - p.run_forward).abs() < 1e-3,
            "{hz} Hz: an empty gauge runs"
        );
    }
}

#[test]
fn letting_go_refills_by_time_and_a_held_key_keeps_it_empty() {
    let p = MovementParams::default();
    let w = flat_world();
    for hz in [30u32, 60, 240] {
        let dt = 1.0 / hz as f32;
        let mut s = settled(&p, &w, dt);
        // run the gauge down, then hold the keys 2 s more: still empty
        for _ in 0..6 * hz {
            s.step(&p, &w, &sprint_keys(true), dt);
        }
        assert_eq!(s.stamina.value, Some(0.0));
        // let go of the sprint key (forward and run held) for 1 s: refills at `regen` per second
        for _ in 0..hz {
            s.step(&p, &w, &sprint_keys(false), dt);
        }
        let v = s.stamina.value.unwrap();
        assert!(
            (v - RATES.regen as f32).abs() < 1e-2,
            "{hz} Hz: refilled to {v}"
        );
        // a new sprint lasts (regen - 1) / drain s
        let mut n = 0u32;
        for _ in 0..2 * hz {
            s.step(&p, &w, &sprint_keys(true), dt);
            n += (s.controls & bits::SPRINT != 0) as u32;
        }
        let dur = n as f32 * dt;
        let want2 = (RATES.regen - 1) as f32 / RATES.drain as f32;
        assert!(
            (dur - want2).abs() <= dt + 1e-3,
            "{hz} Hz: second sprint {dur} s"
        );
        // standing still refills to max and not beyond
        for _ in 0..10 * hz {
            s.step(&p, &w, &input(Commands::default()).with_mods(mods()), dt);
        }
        assert_eq!(s.stamina.value, Some(RATES.max as f32), "{hz} Hz");
    }
}

#[test]
fn without_gauge_rates_a_sprint_never_runs_out() {
    let p = MovementParams::default();
    let w = flat_world();
    let mut s = settled(&p, &w, 1.0 / 60.0);
    let input = TickInput {
        mods: SpeedMods::default(),
        ..sprint_keys(true)
    };
    for _ in 0..60 * 20 {
        s.step(&p, &w, &input, 1.0 / 60.0);
    }
    assert_ne!(s.controls & bits::SPRINT, 0);
}

#[test]
fn a_blocked_sprint_walks_but_keeps_the_sprint_bit() {
    // zoom level >= 1 (controller+0xf0): the sprint test is true but the speed is the walk speed
    let p = MovementParams::default();
    let w = flat_world();
    let mut s = settled(&p, &w, 1.0 / 60.0);
    let input = TickInput {
        sprint_blocked: true,
        ..sprint_keys(true)
    };
    for _ in 0..60 {
        s.step(&p, &w, &input, 1.0 / 60.0);
    }
    assert_ne!(s.controls & bits::SPRINT, 0);
    assert!((s.speed_cap(&p, &input) - p.walk_speed).abs() < 1e-3);
    let free = sprint_keys(true);
    assert!(s.speed_cap(&p, &free) > p.run_forward);
}

// EVIDENCE: cshell+0x1ff980 (the mover veto, method +8, goes to gauge mode 0 with the pair (0, 1), not mode 2)
#[test]
fn a_vetoed_sprint_refills_instead_of_emptying() {
    let p = MovementParams::default();
    let w = flat_world();
    for hz in [30u32, 60, 240] {
        let dt = 1.0 / hz as f32;
        let mut s = settled(&p, &w, dt);
        // 1 s of sprint: the gauge is part drained
        for _ in 0..hz {
            s.step(&p, &w, &sprint_keys(true), dt);
        }
        let drained = s.stamina.value.unwrap();
        assert!(drained < RATES.max as f32 - 1.0, "{hz} Hz: {drained}");
        // the same keys under an outside veto: no sprint, the gauge refills (it is not emptied)
        let vetoed = TickInput {
            stamina: false,
            ..sprint_keys(true)
        };
        for _ in 0..hz / 2 {
            s.step(&p, &w, &vetoed, dt);
        }
        assert_eq!(s.controls & bits::SPRINT, 0, "{hz} Hz");
        assert_eq!(s.stamina.mode, GaugeMode::Rest, "{hz} Hz");
        let v = s.stamina.value.unwrap();
        let want = drained + 0.5 * RATES.regen as f32;
        assert!((v - want).abs() < 0.1, "{hz} Hz: {v} vs {want}");
    }
}

// EVIDENCE: cshell+0x1ff980 (the crouch eye mask runs ahead of the FWD branch, which sets SPRINT and mode 1 again)
#[test]
fn the_crouch_eye_return_sprints_and_drains() {
    let p = MovementParams::default();
    let w = flat_world();
    for hz in [30u32, 60, 144, 240] {
        let dt = 1.0 / hz as f32;
        let mut s = settled(&p, &w, dt);
        let crouch = TickInput {
            commands: Commands {
                crouch: true,
                ..Commands::default()
            },
            mods: mods(),
            ..TickInput::default()
        };
        for _ in 0..hz {
            s.step(&p, &w, &crouch, dt);
        }
        assert!(s.eye_dip < 0.0, "{hz} Hz: the eye is down");
        // stand up with the sprint keys: the eye is still returning, yet the step sprints and drains
        s.step(&p, &w, &sprint_keys(true), dt);
        assert_ne!(
            s.controls & bits::SPRINT,
            0,
            "{hz} Hz: SPRINT during the eye return"
        );
        assert_eq!(s.stamina.mode, GaugeMode::Sprint, "{hz} Hz");
        assert!(
            s.stamina.value.unwrap() < RATES.max as f32,
            "{hz} Hz: drains"
        );
    }
}

/// Jumps once on flat ground at `hz`; returns the events of every step from the take-off on and whether the
/// airborne-after-jump flag was still set on each of them.
fn one_jump(hz: u32) -> Vec<(StepEvents, bool, bool)> {
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / hz as f32;
    let mut s = settled(&p, &w, dt);
    let mut out = Vec::new();
    for t in 0..2 * hz {
        let ev = s.step(
            &p,
            &w,
            &input(Commands {
                jump: t < hz / 10,
                ..Commands::default()
            }),
            dt,
        );
        if ev.jumped || !out.is_empty() {
            out.push((ev, s.jump_flag, s.on_ground));
            assert!(
                s.jumping || s.on_ground,
                "the whole-flight state lasts until the landing"
            );
        }
    }
    out
}

#[test]
fn the_airborne_flag_ends_one_step_after_the_take_off_at_every_rate() {
    for hz in [30u32, 60, 144, 240] {
        let steps = one_jump(hz);
        assert!(!steps.is_empty(), "{hz} Hz: it jumped");
        let (take_off, flag0, _) = steps[0];
        assert!(take_off.jumped && flag0, "{hz} Hz: take-off");
        // the next step: the land notice (3), the flag is clear, the body is still in the air
        let (next, flag1, ground1) = steps[1];
        assert!(next.landed_from_jump, "{hz} Hz: land notice one step later");
        assert!(!flag1 && !ground1, "{hz} Hz: flag cleared, still airborne");
        // the rest of the flight carries no flag, and the real landing on flat ground sends nothing
        assert!(steps[1..].iter().all(|(_, f, _)| !f), "{hz} Hz");
        let notices: Vec<u8> = steps
            .iter()
            .flat_map(|(e, _, _)| e.notice_codes())
            .collect();
        assert_eq!(notices, vec![1, 3], "{hz} Hz: notices {notices:?}");
        assert!(steps.last().unwrap().2, "{hz} Hz: on the ground again");
    }
}

/// `TickInput` with other modifiers (test helper).
trait WithMods {
    fn with_mods(self, m: SpeedMods) -> Self;
}

impl WithMods for TickInput<'static> {
    fn with_mods(self, m: SpeedMods) -> Self {
        TickInput { mods: m, ..self }
    }
}
