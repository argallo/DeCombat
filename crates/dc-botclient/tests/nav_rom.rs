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

/// How near the body gets to `to` when it runs from the standing place `from` straight at it (the movement code, at
/// 60 Hz, for the time the run takes plus half a second).
fn body_gets_to(
    world: &std::sync::Arc<dc_sim::CollisionWorld>,
    from: [f32; 3],
    to: [f32; 3],
) -> f32 {
    use dc_botclient::motion::{Body, MotionKind};
    let mut b = Body::new(
        from,
        [16.0, 50.0, 16.0],
        0.0,
        MotionKind::Hunt,
        60,
        Some(world.clone()),
    );
    let p = b.pos();
    b.steer = Some((to[0] - p[0]).atan2(to[2] - p[2]));
    let mut best = flat(p, to);
    for _ in 0..((best / 285.0 + 0.5) * 60.0) as u32 {
        b.advance(16_667);
        best = best.min(flat(b.pos(), to));
    }
    best
}

/// BOT-118: around the places where bots pushed into the world for 8-12 s at a time in a 6-minute Gray Hammer match
/// (a railing above the trench at x 1290, the sides of the stairs at x 870), the body walks every link of the grid.
/// Before, 111 of 6257 links there were no way for the body: links checked only to the probed place but stored to a
/// node up to a lattice step beyond it, and nodes on slopes the mover slides down.
#[test]
fn rom_grayhammer_the_body_walks_every_link_near_the_stuck_places() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let listed = r.resolve(b"WORLDS/GRAYHAMMER.DAT").expect("the world");
    let w = r.load(&listed).expect("loads");
    let places = [
        ([1354.0f32, 50.1, -376.0], [-2102.0f32, -2.0, -1962.0]),
        ([806.0, 50.1, -1152.0], [1447.0, 50.0, -723.0]),
        ([-1308.0, 50.1, -27.0], [-2102.0, -2.0, -1962.0]),
    ];
    let mut bad = Vec::new();
    let mut links = 0;
    for (at, goal) in places {
        let mut nav = NavGrid::new(w.collision.clone(), [16.0, 50.0, 16.0]);
        assert!(
            [
                (0.0f32, 0.0f32),
                (8.0, 0.0),
                (-8.0, 0.0),
                (0.0, 8.0),
                (0.0, -8.0),
                (24.0, 0.0)
            ]
            .iter()
            .any(|(x, z)| nav.seed([at[0] + x, at[1], at[2] + z])),
            "a node at {at:?}"
        );
        while nav.growing() && nav.probes < 400_000 {
            nav.expand(2_000, Some(goal));
        }
        for (n, to) in nav.links_near(at, 400.0) {
            for t in to {
                links += 1;
                let left = body_gets_to(&w.collision, n, t);
                if left > 40.0 {
                    bad.push((n.map(f32::round), t.map(f32::round), left.round()));
                }
            }
        }
    }
    assert!(links > 3000, "{links} links");
    assert!(
        bad.is_empty(),
        "{} of {links} links the body does not walk: {bad:?}",
        bad.len()
    );
}

/// BOT-119: on Power Surge a bot on a crate or container top has a way down on the grid, and the body walks every
/// link from its nodes. Before, 17 of 290 tops 40..200 units above the floor were islands (the drop's straight sweep met
/// the edge; drops over 96 were no link), so a bot that got onto one (a stuck jump, a fall) stayed there.
#[test]
fn rom_powersurge_every_crate_top_has_a_way_down() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let listed = r.resolve(b"WORLDS/RAPIDSTREAM.DAT").expect("the world");
    let w = r.load(&listed).expect("loads");
    let spawns = spawn_points(r.rom(), &listed);
    // tops found by a sweep from above, off the grid grown from the spawns (OBSERVED 2026-11-04, EU-test ROM)
    let tops = [
        [-589.0f32, -3112.0, -2283.0],
        [35.0, -3037.0, -843.0],
        [227.0, -3313.0, -939.0],
        [-685.0, -3354.0, 69.0],
        [515.0, -2928.0, -123.0],
    ];
    let mut bad = Vec::new();
    for top in tops {
        let mut nav = NavGrid::new(w.collision.clone(), [16.0, 50.0, 16.0]);
        let home = spawns
            .iter()
            .find(|s| nav.seed(**s))
            .copied()
            .expect("a spawn point the body stands at");
        let at = [top[0], top[1] + 51.0, top[2]];
        assert!(nav.seed(at), "a node on the top at {top:?}");
        while nav.growing() && nav.probes < 400_000 {
            nav.expand(2_000, Some(at));
        }
        let off = nav
            .path(at, home)
            .is_some_and(|p| p.iter().any(|q| q[1] < at[1] - 30.0));
        if !off {
            bad.push((top, "no way down".to_string()));
        }
        // the links from the top's nodes (the ways off it)
        for (n, to) in nav.links_near(at, 150.0) {
            if (n[1] - at[1]).abs() > 20.0 {
                continue;
            }
            for t in to {
                let left = body_gets_to(&w.collision, n, t);
                if left > 40.0 {
                    bad.push((top, format!("{:?} -> {:?} left {left:.0}", n.map(f32::round), t.map(f32::round))));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}
