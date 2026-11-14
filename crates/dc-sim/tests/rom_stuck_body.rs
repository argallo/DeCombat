//! ROM-gated (skipped when CA_ROM is unset): a body wedged inside world geometry on one ROM world (bot programme
//! report, 2026-11-10: stuck 134.7 s after a jump into a corner). The point is wedged (overlaps, blocked along every
//! axis); SIM-28 moves it to the nearest spot it fits without going through a wall. The point just before it overlaps
//! the world too but can still move, and must be left alone.
// SPEC: MOVEMENT_2009.md §9.5 (slide loop), §9.8 / §9.9 (face tests: front sides only)
// DC-DEFAULT: SIM-28 (proposed) the wedge test and its escape
#![cfg(feature = "world")]

use dc_sim::from_world::{collision_world, WorldFilter};
use dc_sim::{Collider, Commands, Cylinder, MovementParams, PlayerState, TickInput, V3};
use dc_world::{ParseOptions, World};

/// The stand box of the user's player models (same lookup as rom_spawns.rs).
const STAND_KEY: ([u8; 8], usize) = ([0x7f, 0x00, 0x7b, 0xf2, 0xff, 0x2b, 0xcc, 0x78], 3);

fn stand_dims(rom: &std::path::Path) -> V3 {
    let game = rom.join("Game");
    let mut names: Vec<String> = std::fs::read_dir(&game)
        .expect("Game dir")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.to_ascii_lowercase().ends_with(".rez"))
        .collect();
    names.sort();
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
            for c in &m.clips {
                if c.name.0.len() == STAND_KEY.1
                    && dc_rez::dc_rom::sha256(&c.name.0)[..8] == STAND_KEY.0
                {
                    return V3::from_array(c.extent);
                }
            }
        }
    }
    panic!("no player model with the stand clip in the ROM");
}

/// The world file whose archive entry name contains `needle` (upper case).
fn world_file(rom: &std::path::Path, needle: &str) -> Vec<u8> {
    let arch =
        dc_rez::RezArchive::open(&rom.join("Game").join("WORLDS.rez")).expect("open world archive");
    for (name, off, size) in arch.entries() {
        let up = name.to_ascii_uppercase();
        if up.ends_with(".DAT") && up.contains(needle) {
            return arch.read_raw(*off, *size).expect("read world");
        }
    }
    panic!("no world file matching");
}

#[test]
fn rom_a_wedged_body_gets_out_and_a_merely_embedded_one_is_left_alone() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let dims = stand_dims(&rom);
    let data = world_file(&rom, "GRAYHAMMER");
    let w = World::parse_with(&data, ParseOptions { render: false }).expect("world parses");
    let cw = collision_world(&w, &WorldFilter::default());
    let shape = Cylinder::from_dims(dims);
    let before = V3::new(260.7, -123.9, -66.8);
    let stuck = V3::new(261.733_7, -118.022_095, -84.191_5);

    // the point before the jump overlaps the world but is not wedged (it can move up): never touched
    assert!(cw.overlaps(&shape, before));
    assert!(!dc_sim::wedged(&cw, &shape, before));
    // the stuck point is wedged, and the escape is a fitting spot reached without crossing a front face
    assert!(dc_sim::wedged(&cw, &shape, stuck));
    let to = dc_sim::escape_wedge(&cw, &shape, stuck, dc_sim::player::UNWEDGE_MAX)
        .expect("an escape spot");
    let dist = (to - stuck).len();
    println!("escape {:?} at {dist:.1} units", to - stuck);
    assert!(!cw.overlaps(&shape, to));
    assert!(cw.ray(stuck, to).is_none(), "not through a wall");
    assert!(dist <= dc_sim::player::UNWEDGE_MAX as f32 + 0.01);

    // the whole step: a body pushing forward at the stuck point is freed and then moves on
    let p = MovementParams::default();
    let mut s = PlayerState::new(stuck, dims, &p);
    let run = TickInput {
        commands: Commands {
            forward: true,
            run: true,
            ..Commands::default()
        },
        ..TickInput::default()
    };
    // the search runs when the step clock crosses a quarter second: freed within the first 0.25 s
    let mut freed_at = None;
    for i in 0..20 {
        if s.step(&p, &cw, &run, 1.0 / 60.0).unwedged {
            freed_at = Some(i);
            break;
        }
        assert_eq!(s.pos, stuck, "held in place until the search runs");
    }
    println!("freed at step {freed_at:?}");
    assert!(freed_at.is_some(), "freed within a third of a second");
    assert_eq!(s.pos, to);
    let after = s.pos;
    for _ in 0..120 {
        s.step(&p, &cw, &run, 1.0 / 60.0);
        assert!(s.pos.is_finite());
    }
    println!(
        "moved {:.1} units in the 2 s after the escape",
        (s.pos - after).len()
    );
    assert!((s.pos - after).len() > 1.0, "the body moves again");

    // a body at the merely embedded point, pushing forward, is never moved by SIM-28
    let mut e = PlayerState::new(before, dims, &p);
    for _ in 0..120 {
        assert!(!e.step(&p, &cw, &run, 1.0 / 60.0).unwedged);
    }
}
