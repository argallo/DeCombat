//! ROM-gated (skipped when CA_ROM is unset): the ladder volumes of every world of the user's client. Each ladder
//! brush gives one convex region (CLI-59); a body placed beside it touches it, gets the ladder container and climbs
//! when it moves forward with the view pitched up (MOVEMENT_2009.md §11.4: the acceleration follows the full view
//! direction on a ladder, gravity off, speed 200). Maps are reported by index only; the standing box is a made-up
//! test box of the 2009 size class, not a ROM value.
// SPEC: MOVEMENT_2009.md §11.1, §11.2 (kind 11), §11.4, §6.4 (ladder: Fdir = F unnormalised), §7 step 1 (200)
// SPEC: FMT_DAT85.md §6.2 (ladder role), §6.4 (object name = BSP model name)
#![cfg(feature = "world")]

use dc_sim::from_world::{brush_volume, collision_world, ladder_volumes, WorldFilter};
use dc_sim::{Collider, Commands, Container, Cylinder, MovementParams, PlayerState, TickInput, V3};
use dc_world::{BspModel, ObjectRole, ParseOptions, World};

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

/// A position near the bottom of `v` where the body fits and touches the region: the middle first, then points
/// pushed out along x and z.
fn spot(
    cw: &dc_sim::CollisionWorld,
    v: &dc_sim::ConvexVolume,
    shape: &Cylinder,
    mid: V3,
) -> Option<V3> {
    let half = (v.hi - v.lo) * 0.5;
    for lift in [2.0f32, 20.0, 40.0] {
        let y = v.lo.y + DIMS.y + lift;
        for k in 0..=8 {
            let f = k as f32 / 4.0;
            for (dx, dz) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                let at = V3::new(
                    mid.x + dx * f * (half.x + shape.radius),
                    y,
                    mid.z + dz * f * (half.z + shape.radius),
                );
                if !cw.overlaps(shape, at) && v.touches(shape, at) {
                    return Some(at);
                }
            }
        }
    }
    None
}

/// Made-up test box (half extents).
const DIMS: V3 = V3::new(15.0, 50.0, 15.0);

/// The ladder brushes of `w` in the order `ladder_volumes` takes them (an object with the ladder role names the
/// model; models without points are skipped).
fn ladder_models<'a, 'w>(w: &'a World<'w>) -> Vec<&'a BspModel<'w>> {
    w.models
        .iter()
        .filter(|m| {
            w.object_named(m.name)
                .is_some_and(|o| o.role == Some(ObjectRole::Ladder))
                && !m.points.is_empty()
        })
        .collect()
}

/// How far the farthest brush point of `m` lies outside any plane of `v` (at most 0 for a convex brush whose
/// planes all face out).
fn worst_violation(m: &BspModel<'_>, v: &dc_sim::ConvexVolume) -> f32 {
    m.points
        .iter()
        .map(|p| {
            let x = V3::from_array(*p);
            v.planes
                .iter()
                .map(|&(n, d)| n.dot(x) - d)
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .fold(f32::NEG_INFINITY, f32::max)
}

#[test]
fn rom_ladders_are_convex_touchable_and_climbable() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let p = MovementParams::default();
    let shape = Cylinder::from_dims(DIMS);
    let (mut ladders, mut convex, mut placed, mut climbed) = (0usize, 0usize, 0usize, 0usize);
    let (mut vy_max, mut worst) = (0.0f32, f32::NEG_INFINITY);
    for (mi, data) in world_files(&rom).iter().enumerate() {
        let w = World::parse_with(data, ParseOptions { render: false }).expect("world parses");
        let cw = collision_world(&w, &WorldFilter::default());
        let vols = ladder_volumes(&w);
        let models = ladder_models(&w);
        assert_eq!(
            models.len(),
            vols.len(),
            "map {mi}: one volume per ladder brush"
        );
        let (mut map_placed, mut map_climbed) = (0usize, 0usize);
        for ((v, c), m) in vols.volumes.iter().zip(&models) {
            assert_eq!(*c, Container::ladder());
            assert_eq!(
                brush_volume(m).as_ref(),
                Some(v),
                "map {mi}: volume of its brush"
            );
            ladders += 1;
            // every brush point lies inside (or on) every plane: the brush is convex
            let outside = worst_violation(m, v);
            worst = worst.max(outside);
            if outside <= 0.5 {
                convex += 1;
            }
            let mid = (v.lo + v.hi) * 0.5;
            // a free spot beside or inside the ladder region, near its bottom, that touches it; face its middle
            let Some(start) = spot(&cw, v, &shape, mid) else {
                continue;
            };
            placed += 1;
            map_placed += 1;
            let to = mid - start;
            let yaw = to.x.atan2(to.z);
            let mut s = PlayerState::new(start, DIMS, &p);
            let mut list = Vec::new();
            let mut on_ladder = false;
            for _ in 0..90 {
                vols.touched(&shape, s.pos, &mut list);
                let input = TickInput {
                    commands: Commands {
                        forward: true,
                        run: true,
                        ..Commands::default()
                    },
                    yaw,
                    pitch: -1.2,
                    containers: &list,
                    ..TickInput::default()
                };
                s.step(&p, &cw, &input, 1.0 / 60.0);
                on_ladder |= s.on_ladder;
                if s.on_ladder {
                    vy_max = vy_max.max(s.vel.y);
                }
                assert!(s.pos.is_finite(), "map {mi}: non-finite on a ladder");
            }
            assert!(
                on_ladder,
                "map {mi}: a body touching a ladder volume is on the ladder"
            );
            let best = s.pos.y - start.y;
            if best > 50.0 {
                climbed += 1;
                map_climbed += 1;
            }
        }
        println!(
            "map {mi}: {} ladder volumes, {map_placed} placed, {map_climbed} climbed",
            vols.len()
        );
    }
    println!(
        "ladders {ladders}, convex {convex} (worst point outside a plane {worst:.3}), placed {placed}, climbed >50 units in 1.5 s {climbed}, top vertical speed on a ladder {vy_max:.1}"
    );
    assert_eq!(
        convex, ladders,
        "every ladder brush is convex (every point inside every plane; worst {worst})"
    );
    assert!(
        placed * 10 >= ladders * 9,
        "a free spot beside 90 % of the ladders"
    );
    assert!(
        climbed * 10 >= placed * 9,
        "90 % of the placed bodies climb"
    );
    // the cap of 200 before the acceleration (§6.3) plus one fixed step of acceleration (3000 x 1/60 = 50): the
    // velocity after a step reads up to about 250 at 60 Hz. The 2009 client reported 201-208 (GAME_MESSAGES_2009_C2S.md
    // §2.1.1, tdm1, 69 ladder updates) at its own short frames: a per-frame difference of the cap-then-accelerate
    // order (the displacement per step averages 200 + 3000 x dt / 4 = 212.5 units/s at 60 Hz)
    assert!(
        vy_max > 195.0 && vy_max < 260.0,
        "ladder climb speed {vy_max}"
    );
}
