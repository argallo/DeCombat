//! Speed per held weapon and the jump-key rule, on the session clock (fixed-step `Simulation`, no ROM: the rows
//! below are made up; the ROM-gated half is `rom_speed.rs`).
//!
//! - Speed: every stance speed is `(1 - p) x s` with `p` summed from the held weapon row, the body row, the figure
//!   and the gear rows, and the sprint speed `s = (dash terms + 1) x run x figure dash`; the weapon changes run AND
//!   sprint by its own penalty (MOVEMENT_2009.md §7; `dc_sim::loadout`).
//! - Jump: a held key requests one jump; a new one needs the key seen up while the body stands (MOVEMENT_2009.md
//!   §5.3, the jump-key-held flag).
// SPEC: MOVEMENT_2009.md §5.3, §7
// EVIDENCE: cshell+0x5c270, cshell+0x1fd390; DISASM: cshell+0x1ffad3..0x1ffb75

mod common;
use common::*;

use dc_sim::{
    BodySpeed, Commands, FigureSpeed, Loadout, MovementParams, PlayerState, Simulation, SpeedTable,
    TickInput, V3,
};

const KNIFE: u16 = 1;
const RIFLE: u16 = 2;
const HEAVY: u16 = 3;

/// Made-up rows (not ROM values): a body with penalty 0.1 and dash 0.4, a figure with dash 1.0.
fn table() -> SpeedTable {
    let mut t = SpeedTable::default();
    t.weapons.insert(KNIFE, 0.0);
    t.weapons.insert(RIFLE, 0.1);
    t.weapons.insert(HEAVY, 0.3);
    t.bodies = vec![BodySpeed {
        penalty: 0.1,
        dash: 0.4,
        figure: Some(0),
        ..Default::default()
    }];
    t.override_base = 1;
    t.figures = vec![FigureSpeed {
        speed_bonus: 0.0,
        jump_bonus: 0.0,
        weight: 0.0,
        dash_mul: 1.0,
        ..Default::default()
    }];
    t
}

fn holding(weapon: u16) -> Loadout {
    Loadout {
        weapon: Some(weapon),
        body: Some(0),
        ..Loadout::default()
    }
}

/// Runs `secs` of forward motion (`sprint` or run) with `weapon` held on the session clock at `hz`; returns the
/// speed cap of the last tick and the mean horizontal speed over the last second (displacement / time).
fn plateau(weapon: u16, sprint: bool, hz: u32, secs: f32) -> (f32, f32) {
    let p = MovementParams::default();
    let w = flat_world();
    let s0 = settled(&p, &w, 1.0 / hz as f32);
    let mut sim = Simulation::new(p, &w, s0, hz);
    let mods = table().mods(&holding(weapon));
    let input = TickInput {
        commands: Commands {
            forward: true,
            run: true,
            sprint,
            ..Commands::default()
        },
        mods,
        ..TickInput::default()
    };
    let ticks = (secs * hz as f32) as u64;
    let mut mark = None;
    for t in 0..ticks {
        sim.tick(&input);
        if t == ticks - hz as u64 {
            mark = Some(sim.state.pos);
        }
    }
    let m = mark.expect("ran long enough");
    let d = sim.state.pos - m;
    let cap = sim.state.speed_cap(&p, &input);
    (cap, (d.x * d.x + d.z * d.z).sqrt())
}

#[test]
fn sprint_and_run_caps_follow_the_held_weapon() {
    let p = MovementParams::default();
    for (weapon, pen) in [(KNIFE, 0.1f32), (RIFLE, 0.2), (HEAVY, 0.4)] {
        let (run_cap, _) = plateau(weapon, false, 60, 2.0);
        let (sprint_cap, _) = plateau(weapon, true, 60, 2.0);
        let want_run = (1.0 - pen) * p.run_forward;
        let want_sprint = (1.0 - pen) * (1.4 * p.run_forward);
        assert!(
            (run_cap - want_run).abs() < 1e-3,
            "weapon {weapon}: run {run_cap} vs {want_run}"
        );
        assert!(
            (sprint_cap - want_sprint).abs() < 1e-3,
            "weapon {weapon}: sprint {sprint_cap} vs {want_sprint}"
        );
    }
}

#[test]
fn knife_sprints_faster_than_the_rifle_at_every_rate() {
    // the moved speed (not only the cap) keeps the weapon's ratio at the client's fixed steps (at 60 Hz the
    // cap-then-accelerate order of §6.3 adds one step of acceleration to every stance, which dilutes the ratio a
    // little: within 1.5 %)
    for hz in [60u32, 144, 240] {
        let (_, knife) = plateau(KNIFE, true, hz, 3.0);
        let (_, rifle) = plateau(RIFLE, true, hz, 3.0);
        let (_, heavy) = plateau(HEAVY, true, hz, 3.0);
        let want = (1.0 - 0.2) / (1.0 - 0.1);
        assert!(
            knife > rifle && rifle > heavy,
            "{hz} Hz: {knife} {rifle} {heavy}"
        );
        assert!(
            (rifle / knife - want).abs() < 0.015,
            "{hz} Hz: rifle / knife {} vs {want}",
            rifle / knife
        );
        // sprint is the 1.4 dash factor above run with the same weapon
        let (_, knife_run) = plateau(KNIFE, false, hz, 3.0);
        eprintln!("{hz} Hz: knife {knife} rifle {rifle} heavy {heavy} knife run {knife_run}");
        assert!(
            (knife / knife_run - 1.4).abs() < 0.05,
            "{hz} Hz: sprint / run {}",
            knife / knife_run
        );
    }
}

#[test]
fn a_refused_sprint_runs_with_the_weapon_penalty() {
    // without the server's permission the sprint test is false: the run speed, still scaled by (1 - p)
    let p = MovementParams::default();
    let w = flat_world();
    let mut s = settled(&p, &w, 1.0 / 60.0);
    let input = TickInput {
        commands: Commands {
            forward: true,
            run: true,
            sprint: true,
            ..Commands::default()
        },
        mods: table().mods(&holding(RIFLE)),
        sprint_allowed: false,
        ..TickInput::default()
    };
    for _ in 0..60 {
        s.step(&p, &w, &input, 1.0 / 60.0);
    }
    assert!((s.speed_cap(&p, &input) - 0.8 * p.run_forward).abs() < 1e-3);
}

/// The jump key as a function of the session time (seconds): down inside any of `downs`.
fn key(downs: &[(f32, f32)], t: f32) -> bool {
    downs.iter().any(|&(a, b)| t >= a && t < b)
}

/// Runs `secs` with the jump key pattern `downs` at `hz`; returns the take-off times (s) and the landing times.
fn jumps(downs: &[(f32, f32)], hz: u32, secs: f32) -> (Vec<f32>, Vec<f32>) {
    let p = MovementParams::default();
    let w = flat_world();
    let dt = 1.0 / hz as f32;
    let s0 = settled(&p, &w, dt);
    let mut sim = Simulation::new(p, &w, s0, hz);
    let (mut up, mut down) = (Vec::new(), Vec::new());
    let mut was_ground = true;
    for t in 0..(secs * hz as f32) as u64 {
        let now = t as f32 / hz as f32;
        let ev = sim.tick(&input(Commands {
            jump: key(downs, now),
            ..Commands::default()
        }));
        if ev.jumped {
            up.push(now);
        }
        let ground = sim.state.on_ground && !sim.state.jumping;
        if ground && !was_ground {
            down.push(now);
        }
        was_ground = ground;
    }
    (up, down)
}

#[test]
fn a_held_jump_key_jumps_once_at_every_rate() {
    for hz in [30u32, 60, 144, 240] {
        let (up, down) = jumps(&[(0.1, 4.0)], hz, 4.0);
        assert_eq!(up.len(), 1, "{hz} Hz: take-offs {up:?}");
        assert!(!down.is_empty(), "{hz} Hz: it landed");
    }
}

#[test]
fn letting_go_in_the_air_does_not_count() {
    // pressed at 0.1 s, let go and pressed again in the air (the flight lasts about 0.6 s), held past the landing:
    // no second jump; let go on the ground at 2.0 s and press at 2.1 s: the second jump
    let pattern = [(0.1, 0.3), (0.4, 2.0), (2.1, 2.2)];
    for hz in [60u32, 144] {
        let (up, _) = jumps(&pattern, hz, 3.0);
        assert_eq!(up.len(), 2, "{hz} Hz: {up:?}");
        assert!(
            up[0] < 0.2 && (2.1..2.15).contains(&up[1]),
            "{hz} Hz: {up:?}"
        );
    }
}

#[test]
fn a_press_after_the_landing_jumps_at_once() {
    // no timer: one tick with the key up on the ground and the next press jumps on its own tick
    let hz = 60u32;
    let (up, down) = jumps(&[(0.1, 0.5)], hz, 1.5);
    assert_eq!(up.len(), 1);
    let land = *down.first().expect("landed");
    let dt = 1.0 / hz as f32;
    let press = land + 2.0 * dt;
    let (up2, _) = jumps(&[(0.1, 0.5), (press, press + 0.2)], hz, 2.0);
    assert_eq!(up2.len(), 2, "{up2:?}");
    assert!(
        (up2[1] - press).abs() < 0.5 * dt,
        "the second jump on the press tick: {} vs {press}",
        up2[1]
    );
}

#[test]
fn a_buffered_press_is_refused_until_the_key_is_seen_up_on_the_ground() {
    // the key goes down just before the landing and stays down for 0.3 s after it: the 2009 client takes no jump
    // from it (the press was not a new one on the ground); the next press after letting go jumps
    let hz = 60u32;
    let (_, down) = jumps(&[(0.1, 0.2)], hz, 1.5);
    let land = *down.first().expect("landed");
    let pattern = [
        (0.1, 0.2),
        (land - 0.05, land + 0.3),
        (land + 0.4, land + 0.5),
    ];
    let (up, _) = jumps(&pattern, hz, 2.0);
    assert_eq!(up.len(), 2, "{up:?}");
    assert!(
        up[1] >= land + 0.4 - 0.5 / hz as f32,
        "{up:?} landing {land}"
    );
}

#[test]
fn the_key_flag_is_state_and_digest() {
    let p = MovementParams::default();
    let a = PlayerState::new(V3::new(0.0, 50.0, 0.0), DIMS, &p);
    let mut b = a.clone();
    b.jump_held = true;
    assert_ne!(a.digest(), b.digest());
}
