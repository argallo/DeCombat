//! ROM-gated (skipped without CA_ROM): the user's own crouched approach to a WARHEAD window (playtest report, session
//! 20261027_164327, "still can't fit through the window crouched"), replayed through the client's own body set-up
//! (the world's stand and crouch boxes as `window.rs` / `headless.rs` give them to the player). The recorded 0x64
//! updates show the body standing on the sill (centre y 2833.1) at x -155.4, then FWD + CROUCH for 2.2 s with the
//! x frozen and the centre never dropping: the live client never took the crouch box.
// OBSERVED: CAPTURE 20261027_164327_rustclient dc-client.capture.jsonl (own 0x64 updates t 62.095..64.251: control
// word, yaw, trailer position)
// DC-DEFAULT: SIM-25 (proposed) crouch box; SIM-26 (proposed) the client finds it on the first model with both clips

use dc_client::look::ViewAngles;
use dc_client::world::SpawnPoint;
use dc_client::{LocalPlayer, RomWorld, WorldSource};
use dc_sim::{Commands, MovementParams, V3};

/// The recorded samples (about 15 per second): control word and view yaw.
const PATH: [(u32, f32); 33] = [
    (0x821, -1.671),
    (0x821, -1.741),
    (0x821, -1.752),
    (0x821, -1.750),
    (0x821, -1.743),
    (0x821, -1.743),
    (0x821, -1.750),
    (0x821, -1.798),
    (0x821, -1.829),
    (0x821, -1.818),
    (0x821, -1.813),
    (0x821, -1.813),
    (0x821, -1.813),
    (0x821, -1.813),
    (0x821, -1.829),
    (0x821, -1.822),
    (0x821, -1.802),
    (0x821, -1.748),
    (0x821, -1.644),
    (0x921, -1.553),
    (0x921, -1.544),
    (0x921, -1.544),
    (0x921, -1.549),
    (0x921, -1.576),
    (0x821, -1.653),
    (0x821, -1.712),
    (0x821, -1.761),
    (0x821, -1.795),
    (0x821, -1.809),
    (0x821, -1.807),
    (0x821, -1.791),
    (0x821, -1.718),
    (0x821, -1.653),
];

#[test]
fn rom_warhead_the_users_crouched_approach_goes_through_the_window() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Ok(world) = RomWorld::new(&rom, "WARHEAD").load(None) else {
        println!("skipped: WARHEAD not in this ROM");
        return;
    };
    // the body as the live client sets it up (window.rs), standing on the sill where the recording starts
    let spawn = SpawnPoint {
        pos: V3::new(-155.4, 2833.1, 340.9),
        yaw: -1.67,
    };
    let mut p = LocalPlayer::spawn(
        MovementParams::default(),
        &world.collision,
        &spawn,
        world.stand_dims,
        60,
    );
    p.volumes = world.containers.clone();
    p.state.crouch_dims = world.crouch_dims;
    assert!(
        (p.state.pos.y - 2833.1).abs() < 1.0,
        "on the sill: {:?}",
        p.state.pos
    );
    let start = p.state.pos;
    for (i, &(word, yaw)) in PATH.iter().enumerate() {
        let c = Commands {
            forward: word & 0x1 != 0,
            crouch: word & 0x20 != 0,
            left: word & 0x80 != 0,
            right: word & 0x100 != 0,
            ..Commands::default()
        };
        let view = ViewAngles { yaw, pitch: 0.0 };
        // 15 samples a second at 60 ticks a second (the last one held for 3 s)
        let n = if i + 1 == PATH.len() { 180 } else { 4 };
        for _ in 0..n {
            p.tick(&world.collision, c, &view);
        }
    }
    let s = &p.state;
    assert!(
        s.pos.x < start.x - 100.0,
        "stopped at {:?} (from {start:?}, box {:?})",
        s.pos,
        s.dims
    );
    let crouch = world.crouch_dims.expect("a crouch box");
    assert!(crouch.y < world.stand_dims.y, "crouch box {crouch:?}");
    assert_eq!(s.dims, crouch, "crouched body at {:?}", s.pos);
}
