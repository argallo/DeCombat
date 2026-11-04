//! ROM-gated (skipped when CA_ROM is unset): the pruned point ray (`CollisionWorld::ray_pruned`, client-fps) returns
//! the same hit as `CollisionWorld::ray` on every world of the user's client, for long rays as long as the world's
//! extents diagonal (the client's sun test) and for short ones, from points inside each world's bounds in random
//! directions. Also prints the time of both walks. Maps are reported by index only; no ROM value is stored here.
#![cfg(feature = "world")]

use std::time::Instant;

use dc_sim::from_world::{collision_world, WorldFilter};
use dc_sim::{CollisionWorld, V3};
use dc_world::{ParseOptions, World};

/// The world files (`.dat`, stored plain inside the world archive).
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

/// Rays per world (a quarter of them short, the rest a world diagonal long).
const RAYS_PER_MAP: usize = 2000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// The world's geometry only (no blockers), as the client's sun test uses it.
fn sky_world(w: &World<'_>) -> CollisionWorld {
    let full = collision_world(w, &WorldFilter::default());
    CollisionWorld::from_parts(full.models.clone(), &[])
}

#[test]
fn rom_pruned_rays_equal_the_sweep_on_every_world() {
    let Ok(rom) = dc_rez::dc_rom::resolve_rom() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let files = world_files(&rom);
    assert!(!files.is_empty(), "no worlds in the ROM");
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (mut rays, mut hits, mut sky) = (0u64, 0u64, 0u64);
    let (mut t_full, mut t_pruned) = (0f64, 0f64);
    for (i, data) in files.iter().enumerate() {
        let w = World::parse_with(data, ParseOptions { render: false }).expect("world parses");
        let cw = sky_world(&w);
        let lo = V3::from_array(w.extent_min);
        let hi = V3::from_array(w.extent_max);
        let diag = (hi - lo).len();
        let reach = if diag.is_finite() && diag > 1.0 {
            diag
        } else {
            1.0e5
        };
        let (mut map_full, mut map_pruned) = (0f64, 0f64);
        for k in 0..RAYS_PER_MAP {
            let from = V3::new(
                lo.x + (hi.x - lo.x) * rng.next(),
                lo.y + (hi.y - lo.y) * rng.next(),
                lo.z + (hi.z - lo.z) * rng.next(),
            );
            let dir = V3::new(
                rng.next() * 2.0 - 1.0,
                rng.next() * 2.0 - 1.0,
                rng.next() * 2.0 - 1.0,
            );
            let l = dir.len();
            if l < 1e-3 {
                continue;
            }
            let len = if k % 4 == 3 {
                50.0 + 2000.0 * rng.next()
            } else {
                reach
            };
            let to = from + dir * (len / l);
            let t0 = Instant::now();
            let a = cw.ray(from, to);
            let t1 = Instant::now();
            let b = cw.ray_pruned(from, to);
            let t2 = Instant::now();
            map_full += (t1 - t0).as_secs_f64();
            map_pruned += (t2 - t1).as_secs_f64();
            assert_eq!(a, b, "map {i} ray {from:?} -> {to:?}");
            rays += 1;
            if let Some(h) = a {
                hits += 1;
                if h.surface_flags & 0x10 != 0 {
                    sky += 1;
                }
            }
        }
        eprintln!(
            "map {i}: sweep {:.1} us / ray, pruned {:.1} us / ray",
            map_full * 1e6 / RAYS_PER_MAP as f64,
            map_pruned * 1e6 / RAYS_PER_MAP as f64
        );
        t_full += map_full;
        t_pruned += map_pruned;
    }
    eprintln!(
        "{rays} rays on {} worlds, {hits} hits ({sky} on sky polygons): sweep {:.3} s, pruned {:.3} s",
        files.len(),
        t_full,
        t_pruned
    );
    assert!(hits > rays / 4, "too few hits: {hits} of {rays}");
}
