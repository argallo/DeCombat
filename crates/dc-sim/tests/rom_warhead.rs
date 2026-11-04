//! ROM-gated (skipped when CA_ROM is unset): the low window of WARHEAD (playtest report: "can't crouch-walk through the
//! window"). The opening is lower than the standing box and higher than the crouch box; a crouched body walks
//! through it, a standing one is stopped by the lintel. Both boxes are read from the user's own player models at run
//! time (the stand clip and the crouch clip); the probe point was measured with a scratch scan of the world's
//! collision set (an opening of about 86 units over the room floor, 70 wide).
// SPEC: MOVEMENT_2009.md §5.4 (stand-up test), §9.2 (box), §9.7 (stair step)
// OBSERVED: ROM player models (crouch clips carry a lower box than the stand clip)
// DC-DEFAULT: SIM-25 (proposed) crouch box from the crouch clip
#![cfg(feature = "world")]

use dc_sim::from_world::{collision_world, WorldFilter};
use dc_sim::{Collider, Commands, MovementParams, PlayerState, TickInput, V3};
use dc_world::{ParseOptions, World};

/// sha256(stand animation name)[:8], length 3 (MOVEMENT_2009.md §9.2).
const STAND_KEY: ([u8; 8], usize) = ([0x7f, 0x00, 0x7b, 0xf2, 0xff, 0x2b, 0xcc, 0x78], 3);
/// sha256(lower-body crouch idle clip name)[:8], length 2.
const CROUCH_KEY: ([u8; 8], usize) = ([0x0a, 0x98, 0x0e, 0x22, 0x63, 0x73, 0x50, 0x01], 2);

/// The stand box and the crouch box of the first player model carrying both clips.
fn stance_dims(rom: &std::path::Path) -> (V3, V3) {
    let game = rom.join("Game");
    let mut names: Vec<String> = std::fs::read_dir(&game)
        .expect("Game dir")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.to_ascii_lowercase().ends_with(".rez"))
        .collect();
    names.sort();
    let key =
        |n: &[u8], k: ([u8; 8], usize)| n.len() == k.1 && dc_rez::dc_rom::sha256(n)[..8] == k.0;
    for n in names {
        let arch = dc_rez::RezArchive::open(&game.join(&n)).expect("open rez");
        for (_, off, size) in arch
            .entries()
            .iter()
            .filter(|(e, _, _)| e.to_ascii_lowercase().ends_with(".ltb"))
        {
            let bytes = arch.read_raw(*off, *size).expect("read model");
            let Ok(m) = dc_model::Model::parse(&bytes) else {
                continue;
            };
            let stand = m.clips.iter().find(|c| key(&c.name.0, STAND_KEY));
            let crouch = m.clips.iter().find(|c| key(&c.name.0, CROUCH_KEY));
            if let (Some(s), Some(c)) = (stand, crouch) {
                return (V3::from_array(s.extent), V3::from_array(c.extent));
            }
        }
    }
    panic!("no player model with the stand and crouch clips in the ROM")
}

fn warhead(rom: &std::path::Path) -> Vec<u8> {
    let arch =
        dc_rez::RezArchive::open(&rom.join("Game").join("WORLDS.rez")).expect("open world archive");
    let (_, off, size) = arch
        .entries()
        .iter()
        .find(|(n, _, _)| {
            let n = n.to_ascii_uppercase();
            n.ends_with("WARHEAD.DAT")
        })
        .expect("WARHEAD in the world archive");
    arch.read_raw(*off, *size).expect("read world")
}

/// Walks the body forward along `yaw` for `secs` with or without the crouch key; returns the farthest distance
/// reached along that direction from the start and the final state.
fn walk(
    cw: &dc_sim::CollisionWorld,
    (stand, crouch): (V3, V3),
    start: V3,
    yaw: f32,
    crouched: bool,
    secs: f32,
) -> (f32, PlayerState) {
    let p = MovementParams::default();
    let mut s = PlayerState::new(start, stand, &p);
    s.crouch_dims = Some(crouch);
    let dir = V3::new(yaw.sin(), 0.0, yaw.cos());
    let mut far = f32::NEG_INFINITY;
    for _ in 0..(secs * 60.0) as usize {
        let input = TickInput {
            commands: Commands {
                forward: true,
                crouch: crouched,
                ..Commands::default()
            },
            yaw,
            ctrl_state: 1,
            ..TickInput::default()
        };
        s.step(&p, cw, &input, 1.0 / 60.0);
        far = far.max((s.pos - start).dot(dir));
    }
    (far, s)
}

/// A free standing spot on the floor below `(x, y, z)`.
fn on_floor(cw: &dc_sim::CollisionWorld, stand: V3, x: f32, y: f32, z: f32) -> V3 {
    let shape = dc_sim::Cylinder::from_dims(stand);
    let top = V3::new(x, y, z);
    assert!(!cw.overlaps(&shape, top), "free above the start");
    let h = cw
        .sweep(&shape, top, top - V3::new(0.0, 200.0, 0.0))
        .expect("a floor under the start");
    V3::new(x, y - 200.0 * h.t + 0.5, z)
}

#[test]
fn rom_warhead_window_takes_a_crouched_body_not_a_standing_one() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (stand, crouch) = stance_dims(&rom);
    assert!(
        crouch.y < stand.y,
        "crouch box {crouch:?} lower than {stand:?}"
    );
    let data = warhead(&rom);
    let w = World::parse_with(&data, ParseOptions { render: false }).expect("world parses");
    let cw = collision_world(&w, &WorldFilter::default());
    // the window's opening lies about 60 units ahead of either start (x about -1600, sill a step above the room
    // floor); through it the body ends at least 100 units on
    let z = -84.0f32;
    let inside = on_floor(&cw, stand, -1660.0, 2820.0, z);
    let outside = on_floor(&cw, stand, -1540.0, 2850.0, z);
    let east = std::f32::consts::FRAC_PI_2;
    for (start, yaw) in [(inside, east), (outside, -east)] {
        let (far, s) = walk(&cw, (stand, crouch), start, yaw, false, 3.0);
        assert!(
            far < 70.0,
            "standing from {start:?}: through the window ({far} units, at {:?})",
            s.pos
        );
        assert_eq!(s.dims, stand);
        let (far, s) = walk(&cw, (stand, crouch), start, yaw, true, 5.0);
        assert!(
            far > 100.0,
            "crouched from {start:?}: stopped after {far} units at {:?} (box {:?})",
            s.pos,
            s.dims
        );
        assert_eq!(s.dims, crouch);
    }
}
