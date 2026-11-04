//! ROM-gated (skipped when CA_ROM is unset): a player is placed at every spawn point of every world of the
//! user's client, stands for 5 s and then runs forward along the spawn's facing for 5 s. Nothing may become
//! non-finite and the body centre may never cross a solid surface (no falling through the world).
//! The standing box is read at run time from the user's own player models (the clip whose name hashes to
//! the stand key of MOVEMENT_2009.md §9.2); no ROM value is stored here. Maps are reported by index and
//! byte size only.
// SPEC: MOVEMENT_2009.md §9.2 (stand_dims from the stand animation's dims, key 7f007bf2ff2bcc78/3), §10.1, §10.2
// SPEC: FMT_DAT85.md §6.2 (player spawn role), §6.1 (position, orientation)
#![cfg(feature = "world")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use dc_sim::from_world::{collision_world, WorldFilter};
use dc_sim::{Collider, CollisionWorld, Commands, MovementParams, PlayerState, TickInput, V3};
use dc_world::{ObjectRole, ParseOptions, World};

/// sha256(stand animation name)[:8], length 3 (MOVEMENT_2009.md §9.2).
const STAND_KEY: ([u8; 8], usize) = ([0x7f, 0x00, 0x7b, 0xf2, 0xff, 0x2b, 0xcc, 0x78], 3);

/// The stand box of the user's player models (all models carrying the stand clip must agree).
fn stand_dims(rom: &std::path::Path) -> V3 {
    let game = rom.join("Game");
    let mut found: Vec<[f32; 3]> = Vec::new();
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
                    found.push(c.extent);
                }
            }
        }
    }
    assert!(
        !found.is_empty(),
        "no player model with the stand clip in the ROM"
    );
    assert!(
        found.windows(2).all(|w| w[0] == w[1]),
        "stand boxes disagree: {} models",
        found.len()
    );
    V3::from_array(found[0])
}

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

#[derive(Default, Debug, Clone, Copy)]
struct SpawnResult {
    nan: bool,
    tunnel: bool,
    floor_below: bool,
    landed: bool,
    below_floor: bool,
    walk_on_ground: bool,
    walk_dist: f32,
    embedded: bool,
    placed_embedded: bool,
    became_embedded: bool,
    run_overlap: bool,
    digest: u64,
}

fn simulate(p: &MovementParams, w: &CollisionWorld, dims: V3, pos: V3, yaw: f32) -> SpawnResult {
    let dt = 1.0 / 60.0;
    let mut r = SpawnResult::default();
    // the floor under the spawn, if any
    let floor = w
        .ray(pos, pos - V3::new(0.0, 5000.0, 0.0))
        .map(|h| pos.y - 5000.0 * h.t);
    r.floor_below = floor.is_some();
    // spawn placement: lift by one step and drop to the floor (PROPOSED DC default, see drop_to_floor)
    let placed =
        dc_sim::drop_to_floor(w, &dc_sim::Cylinder::from_dims(dims), pos, p.step(), 1000.0);
    r.placed_embedded = w.overlaps(&dc_sim::Cylinder::from_dims(dims), placed);
    let mut s = PlayerState::new(placed, dims, p);
    let mut last = s.pos;
    let check = |s: &PlayerState, last: &mut V3, r: &mut SpawnResult| {
        if !(s.pos.is_finite() && s.vel.is_finite() && s.acc.is_finite()) {
            r.nan = true;
        }
        if w.ray(*last, s.pos).is_some() {
            r.tunnel = true;
        }
        *last = s.pos;
    };
    let idle = TickInput::default();
    for _ in 0..300 {
        s.step(p, w, &idle, dt);
        check(&s, &mut last, &mut r);
    }
    r.landed = s.on_ground || s.contact.is_some();
    r.embedded = w.overlaps(&s.shape(), s.pos);
    // standing still never creates an overlap
    r.became_embedded = r.embedded && !r.placed_embedded;
    if let Some(fy) = floor {
        // the centre must not end below the floor that was under the spawn by more than the body's
        // half height (sliding down a steep surface below the spawn can lower it, never through it)
        r.below_floor = s.pos.y < fy - 1.0 && !r.landed;
    }
    let start = s.pos;
    let run = TickInput {
        commands: Commands {
            forward: true,
            run: true,
            ..Commands::default()
        },
        yaw,
        ..TickInput::default()
    };
    for _ in 0..300 {
        s.step(p, w, &run, dt);
        check(&s, &mut last, &mut r);
    }
    r.walk_on_ground = s.on_ground;
    // An overlap after running that was not there at placement is possible under the 2009 rules (faces are
    // only tested while the move goes into their front side, and a move parallel to a face is judged at its
    // start, MOVEMENT_2009.md §9.8 / §9.9 steps 2-4): back faces, lips between floor models, step-ups that
    // clip a steep face's corner. Reported, not asserted; falling through is the `tunnel` check.
    r.run_overlap = w.overlaps(&s.shape(), s.pos) && !r.placed_embedded;
    r.walk_dist = (s.pos - start).len_xz();
    r.digest = s.digest();
    r
}

#[test]
fn rom_every_spawn_stands_and_runs() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let t0 = Instant::now();
    let dims = stand_dims(&rom);
    let files = world_files(&rom);
    assert_eq!(files.len(), 21, "world files");
    let p = MovementParams::default();
    let mut total = SpawnTotals::default();
    // combined digest of every final state, in map and spawn order: equal on i686 and x86_64
    let mut all_digest: u64 = 0xcbf2_9ce4_8422_2325;
    for (mi, data) in files.iter().enumerate() {
        let w = World::parse_with(data, ParseOptions { render: false }).expect("world parses");
        let cw = collision_world(&w, &WorldFilter::default());
        let spawns: Vec<(V3, f32)> = w
            .objects_with_role(ObjectRole::PlayerSpawn)
            .filter_map(|o| {
                Some((
                    V3::from_array(o.position()?),
                    o.rotation().map_or(0.0, |r| r[1]),
                ))
            })
            .collect();
        let next = AtomicUsize::new(0);
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(32);
        let results: Vec<SpawnResult> = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..threads)
                .map(|_| {
                    sc.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(&(pos, yaw)) = spawns.get(i) else {
                                break;
                            };
                            out.push((i, simulate(&p, &cw, dims, pos, yaw)));
                        }
                        out
                    })
                })
                .collect();
            let mut all: Vec<(usize, SpawnResult)> =
                hs.into_iter().flat_map(|h| h.join().unwrap()).collect();
            all.sort_by_key(|x| x.0);
            all.into_iter().map(|x| x.1).collect()
        });
        let m = SpawnTotals::of(&results);
        for r in &results {
            all_digest = (all_digest ^ r.digest).wrapping_mul(0x0000_0100_0000_01b3);
        }
        println!(
            "map {mi:2} bytes {:9} models {:3} blockers {:3} spawns {:3}: nan {} tunnel {} floor_below {} landed {} \
             below_floor {} overlap_at_placement {} overlap_after_stand {} stand_new_overlap {} run_new_overlap {} run_on_ground {} mean_run_dist {:.0}",
            data.len(),
            cw.models.len(),
            cw.blocker_count(),
            m.spawns,
            m.nan,
            m.tunnel,
            m.floor_below,
            m.landed,
            m.below_floor,
            m.placed_embedded,
            m.embedded,
            m.became_embedded,
            m.run_overlap,
            m.walk_on_ground,
            m.walk_dist / m.spawns.max(1) as f32
        );
        total.add(&m);
    }
    println!(
        "stand box from the ROM (half extents) = {dims:?}; total {total:?}; combined state digest {all_digest:#018x}; {:.1} s",
        t0.elapsed().as_secs_f32()
    );
    assert_eq!(
        total.spawns, 1560,
        "player spawns over all maps (FMT_DAT85.md §11.2)"
    );
    assert_eq!(total.nan, 0, "non-finite state");
    assert_eq!(total.tunnel, 0, "the body centre crossed a solid surface");
    assert_eq!(
        total.below_floor, 0,
        "ended below the floor under the spawn without standing anywhere"
    );
    assert_eq!(
        total.landed, total.floor_below,
        "every spawn with a floor below lands"
    );
    assert_eq!(
        total.became_embedded, 0,
        "standing created an overlap that was not there at placement"
    );
}

#[derive(Default, Debug, Clone, Copy)]
struct SpawnTotals {
    spawns: usize,
    nan: usize,
    tunnel: usize,
    floor_below: usize,
    landed: usize,
    below_floor: usize,
    walk_on_ground: usize,
    walk_dist: f32,
    embedded: usize,
    placed_embedded: usize,
    became_embedded: usize,
    run_overlap: usize,
}

impl SpawnTotals {
    fn of(r: &[SpawnResult]) -> SpawnTotals {
        let mut t = SpawnTotals {
            spawns: r.len(),
            ..SpawnTotals::default()
        };
        for x in r {
            t.nan += x.nan as usize;
            t.tunnel += x.tunnel as usize;
            t.floor_below += x.floor_below as usize;
            t.landed += x.landed as usize;
            t.below_floor += x.below_floor as usize;
            t.walk_on_ground += x.walk_on_ground as usize;
            t.walk_dist += x.walk_dist;
            t.embedded += x.embedded as usize;
            t.placed_embedded += x.placed_embedded as usize;
            t.became_embedded += x.became_embedded as usize;
            t.run_overlap += x.run_overlap as usize;
        }
        t
    }

    fn add(&mut self, o: &SpawnTotals) {
        self.spawns += o.spawns;
        self.nan += o.nan;
        self.tunnel += o.tunnel;
        self.floor_below += o.floor_below;
        self.landed += o.landed;
        self.below_floor += o.below_floor;
        self.walk_on_ground += o.walk_on_ground;
        self.walk_dist += o.walk_dist;
        self.embedded += o.embedded;
        self.placed_embedded += o.placed_embedded;
        self.became_embedded += o.became_embedded;
        self.run_overlap += o.run_overlap;
    }
}

#[allow(dead_code)]
fn _collider_is_object_safe(w: &dyn Collider) -> bool {
    w.overlaps(&dc_sim::Cylinder::POINT, V3::ZERO)
}
