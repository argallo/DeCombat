//! Shot-trace evidence tool (DSV-34 / DSV-42 tuning). Reads one world from the user's own ROM and
//!
//! - `segments <csv>`: traces each row (`origin_x..z`, `dir_x..z`, `hit_dist` columns by header name, as the capture
//!   analysis writes them) through the dserver's trace world and prints the first wall and its margin to the hit
//!   point;
//! - `sweep [n]`: shoots from the eye height at each player spawn point to sample points on a standing box at the
//!   other spawn points, and compares the shooter-side line of sight of the bot client (the level model, plain world
//!   models, breakables and the blocker polygons, `dc_sim::WorldFilter::default`) with the dserver's trace (the
//!   level model only): how many shots a shooter sees as clear would the dserver refuse at each slack.
//!
//! Usage: `cargo run -p dc-dserver --example shot_trace -- <rom> <Worlds\name> segments <csv>` or `... sweep 4000`.
//! Prints numbers only (no ROM names or content).
// DC-DEFAULT: DSV-34 (proposed) the trace and its slack; DSV-42 (proposed) the tuning evidence

use dc_dserver::world::{find_entry, trace_world, WORLD_ARCHIVE};
use dc_sim::V3;
use dc_world::{ObjectRole, ParseOptions, World};
use std::path::Path;

fn world_bytes(rom: &Path, arg: &str) -> Result<Vec<u8>, String> {
    let arch = dc_rez::RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE))
        .map_err(|e| format!("world archive: {e}"))?;
    let listed = find_entry(arch.entries(), arg)
        .ok_or_else(|| format!("world {arg:?} not found"))?
        .to_string();
    let raw = arch
        .entry_raw(&listed)
        .map_err(|e| format!("world read: {e}"))?
        .ok_or("world entry vanished")?;
    if raw.starts_with(&85u32.to_le_bytes()) {
        return Ok(raw);
    }
    let key = dc_rez::dc_rom::crypt_key(rom).map_err(|e| format!("world key: {e}"))?;
    dc_rez::decrypt_with_key(&raw, &key).map_err(|e| format!("world decrypt: {e}"))
}

fn norm(d: [f32; 3]) -> [f32; 3] {
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    [d[0] / l, d[1] / l, d[2] / l]
}

/// Distance along `dir` (unit) from `from` to the first solid of `w` within `len`.
fn first_solid(w: &dc_sim::CollisionWorld, from: [f32; 3], dir: [f32; 3], len: f32) -> Option<f32> {
    let to = [
        from[0] + dir[0] * len,
        from[1] + dir[1] * len,
        from[2] + dir[2] * len,
    ];
    w.ray(V3::from_array(from), V3::from_array(to))
        .map(|h| h.t.clamp(0.0, 1.0) * len)
}

fn segments(trace: &dc_sim::CollisionWorld, csv: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(csv).map_err(|e| format!("{}: {e}", csv.display()))?;
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().ok_or("empty csv")?.split(',').collect();
    let col = |n: &str| head.iter().position(|h| h.trim() == n);
    let names = [
        "origin_x", "origin_y", "origin_z", "dir_x", "dir_y", "dir_z", "hit_dist",
    ];
    let idx: Vec<usize> = names
        .iter()
        .map(|n| col(n).ok_or_else(|| format!("column {n} missing")))
        .collect::<Result<_, _>>()?;
    println!("row,hit_dist,first_wall,margin");
    for (i, l) in lines.enumerate() {
        let f: Vec<&str> = l.split(',').collect();
        let v: Vec<Option<f32>> = idx
            .iter()
            .map(|&k| f.get(k).and_then(|x| x.trim().parse().ok()))
            .collect();
        let (Some(ox), Some(oy), Some(oz), Some(dx), Some(dy), Some(dz)) =
            (v[0], v[1], v[2], v[3], v[4], v[5])
        else {
            continue;
        };
        let Some(dist) = v[6] else {
            continue;
        };
        let dir = norm([dx, dy, dz]);
        match first_solid(trace, [ox, oy, oz], dir, dist + 200.0) {
            Some(w) => println!("{i},{dist:.1},{w:.1},{:.1}", dist - w),
            None => println!("{i},{dist:.1},none,"),
        }
    }
    Ok(())
}

fn sweep(world: &World<'_>, trace: &dc_sim::CollisionWorld, n: usize) {
    let sight =
        dc_sim::from_world::collision_world(world, &dc_sim::from_world::WorldFilter::default());
    let spawns: Vec<[f32; 3]> = world
        .objects_with_role(ObjectRole::PlayerSpawn)
        .filter_map(|o| o.position())
        // the standing centre about 35 below the spawn point (OBSERVED in the bot rounds: spawned at y 86, standing
        // at y 51 with the eye at 84)
        .map(|p| [p[0], p[1] - 35.0, p[2]])
        .collect();
    let eye_up = 33.0f32;
    let ext = [15.0f32, 50.0, 15.0];
    let slacks = [0.0f32, 4.0, 8.0, 16.0, 32.0];
    let (mut shots, mut clear) = (0usize, 0usize);
    let mut refused = [0usize; 5];
    let mut near_edge = 0usize;
    let (mut blocked, mut blocked_refused) = (0usize, 0usize);
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 40) as f32 / (1u64 << 24) as f32
    };
    'outer: for (a, sa) in spawns.iter().enumerate() {
        for (b, sb) in spawns.iter().enumerate() {
            if a == b {
                continue;
            }
            let eye = [sa[0], sa[1] + eye_up, sa[2]];
            for _ in 0..4 {
                if shots >= n {
                    break 'outer;
                }
                // a point on the near half of the target box (as a box-entry hit point would be)
                let p = [
                    sb[0] + (next() * 2.0 - 1.0) * ext[0],
                    sb[1] + (next() * 2.0 - 1.0) * ext[1],
                    sb[2] + (next() * 2.0 - 1.0) * ext[2],
                ];
                let d = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
                let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if !(dist > 1.0 && dist < 3500.0) {
                    continue;
                }
                let dir = norm(d);
                shots += 1;
                if first_solid(&sight, eye, dir, dist).is_some() {
                    blocked += 1;
                    if first_solid(trace, eye, dir, dist).is_some_and(|w| w + 16.0 < dist) {
                        blocked_refused += 1;
                    }
                    continue;
                }
                clear += 1;
                // a grazing shot: blocked when the end point moves 2 units sideways
                let side = norm([-dir[2], 0.0, dir[0]]);
                let graze = [-2.0f32, 2.0].iter().any(|s| {
                    let q = [p[0] + side[0] * s, p[1], p[2] + side[2] * s];
                    let dq = [q[0] - eye[0], q[1] - eye[1], q[2] - eye[2]];
                    let lq = (dq[0] * dq[0] + dq[1] * dq[1] + dq[2] * dq[2]).sqrt();
                    first_solid(&sight, eye, norm(dq), lq).is_some()
                });
                if graze {
                    near_edge += 1;
                }
                let wall = first_solid(trace, eye, dir, dist);
                for (k, s) in slacks.iter().enumerate() {
                    if wall.is_some_and(|w| w + s < dist) {
                        refused[k] += 1;
                    }
                }
            }
        }
    }
    println!(
        "spawns {} shots {shots} clear_for_the_shooter {clear} grazing {near_edge}",
        spawns.len()
    );
    for (k, s) in slacks.iter().enumerate() {
        println!(
            "slack {s}: dserver refuses {} of the {clear} clear shots",
            refused[k]
        );
    }
    println!("blocked for the shooter {blocked}: dserver refuses {blocked_refused} at slack 16 (the rest: blocked only by a non-level solid)");
}

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 3 {
        return Err("usage: shot_trace <rom> <Worlds\\name> segments <csv> | sweep [n]".into());
    }
    let rom = Path::new(&a[0]);
    if a[1] == "*" {
        // every world of the archive, by index (the world names are ROM content and are not printed)
        let arch = dc_rez::RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE))
            .map_err(|e| format!("world archive: {e}"))?;
        let names: Vec<String> = arch
            .entries()
            .iter()
            .map(|e| e.0.clone())
            .filter(|n| {
                n.to_ascii_uppercase()
                    .ends_with(dc_dserver::world::WORLD_EXT)
            })
            .collect();
        for (i, n) in names.iter().enumerate() {
            let base = n.rsplit('\\').next().unwrap_or(n);
            let arg = format!(
                "Worlds\\{}",
                &base[..base.len() - dc_dserver::world::WORLD_EXT.len()]
            );
            println!("== world {i}");
            let Ok(data) = world_bytes(rom, &arg) else {
                continue;
            };
            let Ok(world) = World::parse_with(&data, ParseOptions { render: false }) else {
                continue;
            };
            println!("extent {:?} .. {:?}", world.extent_min, world.extent_max);
            if let Some(trace) = trace_world(&world) {
                segments(&trace, Path::new(a.get(3).ok_or("segments needs a csv")?))?;
            }
        }
        return Ok(());
    }
    let data = world_bytes(rom, &a[1])?;
    let world = World::parse_with(&data, ParseOptions { render: false })
        .map_err(|e| format!("world parse: {e}"))?;
    let trace = trace_world(&world).ok_or("no level model")?;
    match a[2].as_str() {
        "segments" => segments(&trace, Path::new(a.get(3).ok_or("segments needs a csv")?)),
        "sweep" => {
            let n = a.get(3).and_then(|x| x.parse().ok()).unwrap_or(4000);
            sweep(&world, &trace, n);
            Ok(())
        }
        other => Err(format!("unknown mode {other:?}")),
    }
}
