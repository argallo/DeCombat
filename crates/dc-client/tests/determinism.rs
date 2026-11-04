//! The client's frame loop adds nothing to the simulation and does not depend on the frame rate: the
//! scripted sequence run through `run_headless` at several frame intervals ends in the same state digest as
//! dc-sim's own `Simulation` (and `replay`) fed the same tick inputs directly.
// SPEC: MOVEMENT_2009.md §15 (shared crate, same inputs give the same bits); DC-DEFAULT: SIM-1 (proposed)

use dc_client::testkit::synth_source;
use dc_client::{run_headless, HeadlessOptions, ScriptedInput, ViewAngles, WorldSource};
use dc_sim::{drop_to_floor, Cylinder, MovementParams, PlayerState, Simulation, TickInput};

/// dc-sim alone: same placement, same per-tick inputs.
fn reference(src: &mut dyn WorldSource, hz: u32) -> u64 {
    let world = src.load(None).expect("load");
    let spawn = world.spawn(0).expect("spawn");
    let p = MovementParams::default();
    let at = drop_to_floor(
        &world.collision,
        &Cylinder::from_dims(world.stand_dims),
        spawn.pos,
        p.step(),
        1000.0,
    );
    let start = PlayerState::new(at, world.stand_dims, &p);
    let script = ScriptedInput::new(ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    });
    let inputs: Vec<TickInput<'static>> = (0..ScriptedInput::ticks(hz))
        .map(|t| {
            let (commands, v) = script.at(t as f64 / hz as f64);
            TickInput {
                commands,
                yaw: v.yaw,
                pitch: v.pitch,
                ..TickInput::default()
            }
        })
        .collect();
    let mut sim = Simulation::new(p, &world.collision, start.clone(), hz);
    for i in &inputs {
        sim.tick(i);
    }
    let mut replayed = start;
    dc_sim::replay(
        &p,
        &world.collision,
        &mut replayed,
        &inputs,
        1.0 / hz as f32,
    );
    assert_eq!(
        sim.state.digest(),
        replayed.digest(),
        "Simulation vs replay"
    );
    sim.state.digest()
}

fn client(src: &mut dyn WorldSource, hz: u32, frame_us: u64) -> (u64, u64) {
    let ticks = ScriptedInput::ticks(hz);
    let wall_us = ticks * 1_000_000 / hz as u64;
    let opts = HeadlessOptions {
        frames: wall_us.div_ceil(frame_us) as u32,
        frame_us,
        render: false,
        tick_hz: hz,
        ..HeadlessOptions::default()
    };
    let r = run_headless(src, None, &opts).expect("headless");
    assert_eq!(r.nan_ticks, 0);
    (r.ticks, r.final_digest)
}

#[test]
fn client_loop_equals_dc_sim_at_every_frame_rate() {
    let mut src = synth_source();
    for hz in [60u32, 30] {
        let want = reference(&mut src, hz);
        for frame_us in [16_667u64, 10_000, 33_334, 7_000, 3_000] {
            let (ticks, digest) = client(&mut src, hz, frame_us);
            assert_eq!(
                ticks,
                ScriptedInput::ticks(hz),
                "hz {hz} frame {frame_us} us"
            );
            assert_eq!(
                digest, want,
                "hz {hz}, frame {frame_us} us: client loop digest differs from dc-sim"
            );
        }
        println!("hz {hz}: digest {want:#018x} at every frame interval");
    }
}
