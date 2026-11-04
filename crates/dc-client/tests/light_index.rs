//! ROM-gated, GPU-free equality test of the model light-set acceleration (render-core): on every ROM map, the light set
//! `WorldLights::light_set` returns (static-light grid index, sun-visibility memo, no clone of the static list) equals
//! the reference path `WorldLights::light_set_unindexed` (every static light offered in list order, a fresh sun ray)
//! bit for bit, at random positions and radii around the world plus repeated positions (memo hits). Also prints the
//! time per call of both paths and of the sun ray alone. Nothing of the ROM is asserted by value. Skipped without
//! `CA_ROM`. The first test needs no ROM: hand-made lights (far outliers, overflowing and non-finite radii, spots, a
//! directional light, a sun) and queries on and off the grid, compared the same way.

use dc_client::world::WorldLights;
use dc_render::{AmbientGrid, Falloff, LightShape, ModelLight};
use dc_sim::from_world::{collision_world, WorldFilter};
use dc_world::{ParseOptions, World};
use glam::Vec3;
use std::time::Instant;

/// Small deterministic generator (xorshift) for the sample positions.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// `a` and `b` are the same light set bit for bit (Debug text: NaN fields compare equal, unlike `==`).
fn same(a: &dc_render::LightSet, b: &dc_render::LightSet) -> bool {
    format!("{a:?}") == format!("{b:?}")
        && a.ambient.to_array().map(f32::to_bits) == b.ambient.to_array().map(f32::to_bits)
}

#[test]
fn indexed_light_sets_equal_the_reference_without_rom() {
    let light = |shape, pos: Vec3, radius: f32, c: f32, falloff| ModelLight {
        shape,
        pos,
        radius,
        colour: Vec3::new(c, c * 0.5, 255.0 - c),
        falloff,
        to_ambient: 0.25,
    };
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut statics = Vec::new();
    // a cluster of point and spot lights inside a 4000-unit box
    for i in 0..300 {
        let p = Vec3::new(rng.next(), rng.next(), rng.next()) * 4000.0 - Vec3::splat(2000.0);
        let r = 30.0 + rng.next() * 600.0;
        let shape = if i % 3 == 0 {
            LightShape::Spot {
                dir: Vec3::new(0.0, -1.0, 0.0),
                cos_half: 0.7,
            }
        } else {
            LightShape::Point
        };
        let falloff = [
            Falloff::Linear,
            Falloff::Quartic,
            Falloff::D3d([1.0, 0.0, 1e-4]),
        ][i % 3];
        statics.push(light(shape, p, r, 20.0 + (i % 200) as f32, falloff));
    }
    // edge cases: far outliers, overflowing / huge / non-finite / negative radii, a non-finite position, a
    // directional light among the statics
    let edge = [
        (Vec3::new(5.0e5, 0.0, 0.0), 100.0),
        (Vec3::splat(-3.0e38), 10.0),
        (Vec3::ZERO, f32::MAX),
        (Vec3::new(10.0, 0.0, 0.0), 1.0e30),
        (Vec3::new(-50.0, 0.0, 0.0), f32::INFINITY),
        (Vec3::new(50.0, 0.0, 0.0), f32::NAN),
        (Vec3::new(0.0, 80.0, 0.0), -120.0),
        (Vec3::splat(f32::NAN), 100.0),
    ];
    for (k, (p, r)) in edge.into_iter().enumerate() {
        statics.insert(
            k * 37,
            light(LightShape::Point, p, r, 90.0, Falloff::Linear),
        );
    }
    statics.insert(
        150,
        light(
            LightShape::Directional {
                dir: Vec3::new(0.0, -1.0, 0.0),
            },
            Vec3::ZERO,
            f32::MAX,
            40.0,
            Falloff::D3d([1.0, 0.0, 0.0]),
        ),
    );
    let grid = AmbientGrid {
        origin: Vec3::splat(-2000.0),
        cell: Vec3::splat(2000.0),
        dims: [2, 2, 2],
        rgb: (0..24).map(|i| (i * 10) as u8).collect(),
        groups: Vec::new(),
    };
    let sun = ModelLight::sun(Vec3::new(0.3, -1.0, 0.2), Vec3::new(120.0, 110.0, 90.0));
    let cases = [
        WorldLights::new(grid.clone(), statics.clone(), Some(sun)),
        WorldLights::new(grid.clone(), statics[..20].to_vec(), None),
        WorldLights::new(grid, Vec::new(), Some(sun)),
    ];
    let mut pts: Vec<(Vec3, f32)> = Vec::new();
    for _ in 0..2000 {
        let p = Vec3::new(rng.next(), rng.next(), rng.next()) * 5000.0 - Vec3::splat(2500.0);
        pts.push((p, rng.next() * 200.0));
    }
    for l in statics
        .iter()
        .filter(|l| l.pos.is_finite() && l.radius.is_finite())
    {
        let r = l.radius.abs();
        pts.push((l.pos, 0.0));
        pts.push((l.pos + Vec3::new(r + 40.0, 0.0, 0.0), 40.0));
        pts.push((l.pos + Vec3::new(0.0, -r - 39.999, 0.0), 40.0));
    }
    pts.extend([
        (Vec3::new(1.0e7, 0.0, 0.0), 50.0),
        (Vec3::new(-1.0e7, 3.0e6, 0.0), 1.0e6),
        (Vec3::new(5.0e5, 0.0, 0.0), 1.0),
        (Vec3::ZERO, 1.0e30),
        (Vec3::ZERO, -5.0),
        (Vec3::ZERO, f32::NAN),
        (Vec3::new(f32::INFINITY, 0.0, 0.0), 10.0),
    ]);
    // repeated positions: sun memo hits and ring wrap-around
    let again: Vec<(Vec3, f32)> = pts.iter().take(40).copied().collect();
    pts.extend(again.iter().rev());
    pts.extend(again);
    for (n, wl) in cases.iter().enumerate() {
        for (i, &(p, r)) in pts.iter().enumerate() {
            let (a, b) = (wl.light_set_unindexed(p, r), wl.light_set(p, r));
            assert!(
                same(&a, &b),
                "case {n}: light set {i} at {p:?} r {r} differs:
{a:?}
{b:?}"
            );
        }
    }
}

#[test]
fn indexed_light_sets_equal_the_reference_on_every_map() {
    let rom = dc_rom::rom_or_skip!();
    let worlds = dc_render::WorldSource::open(&rom).expect("ROM worlds");
    let maps = worlds.map_names();
    assert!(!maps.is_empty());
    let mut total = 0usize;
    for map in maps {
        let Ok(Some(data)) = worlds.read(&map) else {
            println!("{map}: not read, skipped");
            continue;
        };
        let world = match World::parse_with(&data, ParseOptions { render: true }) {
            Ok(w) => w,
            Err(e) => {
                println!("{map}: not parsed ({e}), skipped");
                continue;
            }
        };
        let cw = collision_world(&world, &WorldFilter::default());
        let wl = WorldLights::of(&world, Some(&cw));
        let lo = Vec3::from(world.extent_min);
        let hi = Vec3::from(world.extent_max);
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ map.len() as u64);
        let mut pts: Vec<(Vec3, f32)> = Vec::new();
        for _ in 0..1500 {
            let p = lo + (hi - lo) * Vec3::new(rng.next(), rng.next(), rng.next());
            pts.push((p, 8.0 + rng.next() * 120.0));
        }
        // light positions themselves and points just outside their reach (boundary cases)
        for l in wl.statics.iter().take(200) {
            pts.push((l.pos, 40.0));
            pts.push((l.pos + Vec3::new(l.radius + 40.0, 0.0, 0.0), 40.0));
            pts.push((l.pos + Vec3::new(0.0, -l.radius - 39.999, 0.0), 40.0));
        }
        // repeated positions (sun memo hits)
        let again: Vec<(Vec3, f32)> = pts.iter().take(64).copied().collect();
        pts.extend(again);
        let t = Instant::now();
        let reference: Vec<_> = pts
            .iter()
            .map(|&(p, r)| wl.light_set_unindexed(p, r))
            .collect();
        let ref_us = t.elapsed().as_secs_f64() * 1e6 / pts.len() as f64;
        let t = Instant::now();
        let fast: Vec<_> = pts.iter().map(|&(p, r)| wl.light_set(p, r)).collect();
        let fast_us = t.elapsed().as_secs_f64() * 1e6 / pts.len() as f64;
        let t = Instant::now();
        for &(p, _) in &pts {
            std::hint::black_box(wl.sun_visibility(p));
        }
        let ray_us = t.elapsed().as_secs_f64() * 1e6 / pts.len() as f64;
        for (i, (a, b)) in reference.iter().zip(&fast).enumerate() {
            assert!(same(a, b), "{map}: light set {i} at {:?} differs", pts[i]);
        }
        total += pts.len();
        println!(
            "{map}: {} statics, sun {}, {} sets equal; per call reference {ref_us:.2} us, indexed {fast_us:.2} us, \
             sun ray {ray_us:.2} us",
            wl.statics.len(),
            wl.sun.is_some(),
            pts.len()
        );
    }
    assert!(total > 0);
}
