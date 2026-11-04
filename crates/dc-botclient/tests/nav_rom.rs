//! ROM checks of the bots' navigation grid (BOT-93, BOT-112): run with `CA_ROM` (the user's client folder); without
//! it they pass without checking. Every number comes from the world file at run time.

use dc_botclient::nav::NavGrid;
use dc_botclient::romworld::{RomWorlds, WORLD_ARCHIVE};
use dc_world::{ObjectRole, ParseOptions, World, WORLD_VERSION};

/// The player spawn points of an announced world (positions only), read from the ROM.
fn spawn_points(rom: &std::path::Path, listed: &str) -> Vec<[f32; 3]> {
    let arch = dc_rez::RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE)).unwrap();
    let raw = arch.entry_raw(listed).unwrap().unwrap();
    let data = if raw.starts_with(&WORLD_VERSION.to_le_bytes()) {
        raw
    } else {
        let key = dc_rez::dc_rom::crypt_key(rom).unwrap();
        dc_rez::decrypt_with_key(&raw, &key).unwrap()
    };
    let w = World::parse_with(&data, ParseOptions { render: false }).unwrap();
    w.objects_with_role(ObjectRole::PlayerSpawn)
        .filter_map(|o| o.position())
        .collect()
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// BOT-112: on Gray Hammer the grid grown from one spawn point reaches every other one. Before, the grid grown from
/// the north spawn area never reached the south half (stairs steeper than the lattice step, a stairway under a
/// bridge, passages the lattice columns missed), so the bots of the two teams never met and never fired.
#[test]
fn rom_grayhammer_the_grid_reaches_every_spawn_point() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let listed = r.resolve(b"WORLDS/GRAYHAMMER.DAT").expect("the world");
    let w = r.load(&listed).expect("loads");
    let spawns = spawn_points(r.rom(), &listed);
    assert!(spawns.len() > 10, "{} spawn points", spawns.len());
    // the bot's body: the server's teleport box (BOT-112 grows the grid for it)
    let mut nav = NavGrid::new(w.collision.clone(), [16.0, 50.0, 16.0]);
    // the first spawn point the body stands at (a spawn point lies at the floor or a little above it)
    let start = spawns
        .iter()
        .find(|s| nav.seed(**s))
        .copied()
        .expect("a spawn point the body stands at");
    while nav.growing() && nav.probes < 3_000_000 {
        nav.expand(2_000, None);
    }
    let mut missed = Vec::new();
    for (i, s) in spawns.iter().enumerate().skip(1) {
        let t = *s;
        let reached = nav
            .path(start, t)
            .is_some_and(|p| flat(p[p.len() - 1], t) <= 72.0);
        if !reached {
            missed.push((i, s.map(f32::round)));
        }
    }
    assert!(
        missed.is_empty(),
        "{} of {} spawn points out of the grid's reach ({} nodes): {missed:?}",
        missed.len(),
        spawns.len() - 1,
        nav.len()
    );
}
