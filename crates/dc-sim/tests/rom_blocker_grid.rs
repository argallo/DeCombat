//! ROM-gated (skipped when CA_ROM is unset): on every world of the user's client, the blocker grid (SIM-27, an
//! index for speed) finds exactly the hits of the plain scan of every blocker face, so the index changes nothing
//! for an existing map. Sweeps of the standing body from every player spawn: eight directions, four lengths (the long
//! ones run past the grid's query limit and test the plain scan against itself), at the spawn's height and
//! lifted, and one straight down. Maps are reported by index and counts only.
// DC-DEFAULT: SIM-27 (an index for speed only: the same hits as the blocker scan of MOVEMENT_2009.md §9.6)
#![cfg(feature = "world")]

use dc_sim::from_world::{collision_world, WorldFilter};
use dc_sim::{Collider, Cylinder, V3};
use dc_world::{ObjectRole, ParseOptions, World};

/// The world files (`.dat`, stored plain inside the world archive; read raw, see the dc-world ROM test).
fn world_files(rom: &std::path::Path) -> Vec<Vec<u8>> {
    let arch =
        dc_rez::RezArchive::open(&rom.join("Game").join("WORLDS.rez")).expect("open world archive");
    let mut out = Vec::new();
    for (name, off, size) in arch.entries() {
        if !name.to_ascii_uppercase().ends_with(".DAT") {
            continue;
        }
        let raw = arch.read_raw(*off, *size).expect("read world");
        if raw.starts_with(&dc_world::WORLD_VERSION.to_le_bytes()) {
            out.push(raw);
        }
    }
    out
}

#[test]
fn rom_the_blocker_grid_finds_the_plain_scans_hits_on_every_world() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let files = world_files(&rom);
    assert!(!files.is_empty(), "world files");
    let shape = Cylinder::new(15.0, 50.0);
    let d = core::f32::consts::FRAC_1_SQRT_2;
    let dirs = [
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (d, d),
        (-d, d),
        (d, -d),
        (-d, -d),
    ];
    let (mut worlds, mut sweeps, mut hits) = (0usize, 0usize, 0usize);
    for (mi, data) in files.iter().enumerate() {
        let w = World::parse_with(data, ParseOptions { render: false }).expect("world parses");
        let grid = collision_world(&w, &WorldFilter::default());
        if grid.blocker_count() == 0 {
            continue;
        }
        worlds += 1;
        let plain = grid.clone().with_plain_blocker_scan();
        let spawns: Vec<V3> = w
            .objects_with_role(ObjectRole::PlayerSpawn)
            .filter_map(|o| o.position().map(V3::from_array))
            .collect();
        assert!(!spawns.is_empty(), "map {mi}: no player spawns");
        for s in &spawns {
            for lift in [0.0f32, 40.0, 200.0] {
                let from = *s + V3::new(0.0, lift, 0.0);
                for (dx, dz) in dirs {
                    for reach in [60.0f32, 400.0, 900.0, 3000.0] {
                        let to = from + V3::new(dx * reach, -10.0, dz * reach);
                        let (a, b) = (grid.sweep(&shape, from, to), plain.sweep(&shape, from, to));
                        assert_eq!(a, b, "map {mi}: sweep from {from:?} to {to:?}");
                        sweeps += 1;
                        hits += a.is_some() as usize;
                    }
                }
            }
            let to = *s - V3::new(0.0, 3000.0, 0.0);
            assert_eq!(
                grid.sweep(&shape, *s, to),
                plain.sweep(&shape, *s, to),
                "map {mi}: fall from {s:?}"
            );
            sweeps += 1;
        }
    }
    assert!(
        worlds > 0 && hits > 0,
        "{worlds} worlds with blockers, {hits} hits"
    );
    eprintln!("{worlds} worlds, {sweeps} sweeps, {hits} hits: the grid and the plain scan agree on every one");
}
