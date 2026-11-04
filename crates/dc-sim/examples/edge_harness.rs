//! Edge-catch harness (headless, reads the user's ROM at run time; nothing from it is stored).
//!
//! Modes:
//! - `sweep [--csv FILE]...`: synthetic forward runs at 16 angles from every player spawn of the map and from
//!   the ground positions of the given position streams (capanalyze `updates.csv`), counting "stuck" ticks:
//!   forward held, standing, the body moved less than 0.5 units in the tick, and nothing is ahead above step
//!   height (an obstacle the 2009 mover steps over or slides along).
//! - `replay FILE`: for every pair of consecutive ground updates of a position stream, start dc-sim at the first
//!   one (position, velocity, controls, body yaw) and run the interval; report the end-position error against the
//!   second one and the intervals where dc-sim was blocked while the client moved.
//! - `trace X Y Z YAW [TICKS]`: run forward from a point and print every tick.
//!
//! Usage: `CA_ROM=<client folder> cargo run --release -p dc-sim --example edge_harness -- [--map NAME] <mode> ...`
//! Output: counts and positions only (positions are our own measurements of the map, not ROM bytes).
// SPEC: MOVEMENT_2009.md §9.4-§9.9 (move, slide, stair, sweep), §10.1 (ground probe), §16.2 V-M6 / V-M10
// OBSERVED: capture 20261025_065016_tdm6, 20261025_134212_ae-scope (the 2009 client on
// JUNK_FLEA), 20261025_205716_rustclient (the user's dc-client round) through capanalyze.py

use std::path::{Path, PathBuf};

use dc_sim::from_world::{collision_world, WorldFilter};
use dc_sim::{
    bits, ground_probe, Collider, CollisionWorld, Commands, Cylinder, MovementParams, PlayerState,
    TickInput, V3,
};
use dc_world::{ObjectRole, ParseOptions, World};

/// sha256(stand animation name)[:8], length 3 (MOVEMENT_2009.md §9.2).
const STAND_KEY: ([u8; 8], usize) = ([0x7f, 0x00, 0x7b, 0xf2, 0xff, 0x2b, 0xcc, 0x78], 3);

fn stand_dims(rom: &Path) -> V3 {
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
    panic!("no stand clip in the ROM")
}

fn world_bytes(rom: &Path, map: &str) -> Vec<u8> {
    let arch =
        dc_rez::RezArchive::open(&rom.join("Game").join("WORLDS.rez")).expect("open world archive");
    let want = map.to_ascii_uppercase();
    for (name, off, size) in arch.entries() {
        let up = name.to_ascii_uppercase();
        if up.ends_with(".DAT") && up.contains(&want) {
            return arch.read_raw(*off, *size).expect("read world");
        }
    }
    panic!("no world named like {map}")
}

#[derive(Debug, Clone, Copy)]
struct Update {
    t: f32,
    pos: V3,
    vel: V3,
    yaw: f32,
    control: u32,
    on_ground: bool,
    surface: i32,
}

fn read_updates(path: &Path) -> Vec<Update> {
    let text = std::fs::read_to_string(path).expect("read csv");
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().expect("header").split(',').collect();
    let col = |n: &str| head.iter().position(|h| *h == n).expect(n);
    let (ct, cx, cy, cz) = (col("time_s"), col("pos_x"), col("pos_y"), col("pos_z"));
    let (vx, vy, vz) = (col("vel_x"), col("vel_y"), col("vel_z"));
    let (cyaw, cc, cg, cs) = (
        col("body_yaw_rad"),
        col("control_hex"),
        col("on_ground"),
        col("surface"),
    );
    let mut out = Vec::new();
    for l in lines {
        let f: Vec<&str> = l.split(',').collect();
        let num = |i: usize| f[i].parse::<f32>().unwrap_or(0.0);
        out.push(Update {
            t: num(ct),
            pos: V3::new(num(cx), num(cy), num(cz)),
            vel: V3::new(num(vx), num(vy), num(vz)),
            yaw: num(cyaw),
            control: u32::from_str_radix(f[cc].trim_start_matches("0x"), 16).unwrap_or(0),
            on_ground: f[cg] == "1",
            surface: f[cs].parse().unwrap_or(-1),
        });
    }
    out
}

fn commands_of(c: u32) -> Commands {
    Commands {
        forward: c & bits::FWD != 0,
        backward: c & bits::BACK != 0,
        left: c & bits::LEFT != 0,
        right: c & bits::RIGHT != 0,
        jump: false,
        crouch: c & bits::CROUCH != 0,
        run: c & bits::RUN != 0,
        sprint: c & bits::SPRINT != 0,
        ..Commands::default()
    }
}

const DT: f32 = 1.0 / 60.0;

/// The part of the body above step height, as its own cylinder: centre and shape.
fn upper(p: &MovementParams, shape: &Cylinder, pos: V3) -> (V3, Cylinder) {
    let h = shape.half_height;
    let bottom = pos.y - h + p.step() + 1.0;
    let top = pos.y + h;
    let half = (top - bottom) * 0.5;
    (
        V3::new(pos.x, bottom + half, pos.z),
        Cylinder::new(shape.radius, half),
    )
}

/// Whether nothing is ahead of the body's upper part within `dist` along `dir`.
fn free_above_step(
    w: &CollisionWorld,
    p: &MovementParams,
    s: &PlayerState,
    dir: V3,
    dist: f32,
) -> bool {
    let (c, cyl) = upper(p, &s.shape(), s.pos);
    if w.overlaps(&cyl, c) {
        return false;
    }
    w.sweep(&cyl, c, c + dir * dist).is_none()
}

#[derive(Default, Debug, Clone, Copy)]
struct SweepStats {
    depth: [usize; 4],
    runs: usize,
    overlaps: usize,
    tunnels: usize,
    legit_ticks: usize,
    ticks: usize,
    stuck_ticks: usize,
    stuck_runs: usize,
    episodes: usize,
}

/// One forward run; returns the stuck ticks and the first stuck positions.
#[allow(clippy::too_many_arguments)]
fn run_once(
    w: &CollisionWorld,
    p: &MovementParams,
    dims: V3,
    start: V3,
    yaw: f32,
    ticks: usize,
    stats: &mut SweepStats,
    where_: &mut Vec<(V3, f32)>,
    faces: &[Vec<f32>],
) {
    let shape = Cylinder::from_dims(dims);
    let placed = dc_sim::drop_to_floor(w, &shape, start, p.step(), 200.0);
    if w.overlaps(&shape, placed) {
        return;
    }
    let mut s = PlayerState::new(placed, dims, p);
    let idle = TickInput::default();
    for _ in 0..20 {
        s.step(p, w, &idle, DT);
    }
    if !s.on_ground {
        return;
    }
    stats.runs += 1;
    let input = TickInput {
        commands: Commands {
            forward: true,
            run: true,
            ..Commands::default()
        },
        yaw,
        ..TickInput::default()
    };
    let dir = V3::new(yaw.sin(), 0.0, yaw.cos());
    let mut streak = 0usize;
    let mut any = false;
    for i in 0..ticks {
        let before = s.pos;
        let snapshot = s.clone();
        s.step(p, w, &input, DT);
        stats.ticks += 1;
        if std::env::var("EDGE_OVERLAP").is_ok()
            && !w.overlaps(&snapshot.shape(), before)
            && w.overlaps(&s.shape(), s.pos)
        {
            stats.overlaps += 1;
            // depth: the smallest lift that clears the overlap (cap 8 units)
            let mut lift = 8.0f32;
            for k in 1..=80 {
                let dy = k as f32 * 0.1;
                if !w.overlaps(&s.shape(), s.pos + V3::new(0.0, dy, 0.0)) {
                    lift = dy;
                    break;
                }
            }
            let bin = if lift <= 0.1 {
                0
            } else if lift <= 0.5 {
                1
            } else if lift <= 2.0 {
                2
            } else {
                3
            };
            stats.depth[bin] += 1;
            if stats.overlaps <= 6 || (bin == 3 && stats.depth[3] <= 6) {
                println!(
                    "  overlap lift {lift:.1} from {:?} vel {:?} to {:?} yaw {yaw:.4}",
                    before, snapshot.vel, s.pos
                );
            }
        }
        if let Some(h) = w.ray(before, s.pos) {
            stats.tunnels += 1;
            if std::env::var("EDGE_TUNNEL").is_ok() {
                println!(
                    "  tunnel from {:?} vel {:?} to {:?} yaw {yaw:.4} through model {} poly {} n {:?} contact {:?}",
                    before, snapshot.vel, s.pos, h.model, h.poly, h.normal, snapshot.contact
                );
            }
        }
        if i < 15 || !s.on_ground {
            streak = 0;
            continue;
        }
        let moved = (s.pos - before).len_xz();
        if moved < 0.5 && free_above_step(w, p, &s, dir, 4.0) {
            // a steep slope ahead (2009: no walking up it) or a body embedded in geometry is not an edge catch
            let shape = s.shape();
            let steep = w.sweep(&shape, s.pos, s.pos + dir * 4.0).is_some_and(|h| {
                let ny = face_ny(faces, &h);
                ny > 0.001 && ny < p.standable_cos
            });
            if steep || w.overlaps(&shape, s.pos) {
                stats.legit_ticks += 1;
                streak = 0;
                continue;
            }
            stats.stuck_ticks += 1;
            streak += 1;
            if streak == 3 {
                stats.episodes += 1;
                if where_.len() < 4000 {
                    where_.push((s.pos, yaw));
                }
            }
            any = true;
        } else {
            streak = 0;
        }
    }
    stats.stuck_runs += any as usize;
}

fn starts_from_csv(path: &Path, grid: f32, out: &mut Vec<V3>) {
    for u in read_updates(path) {
        if !u.on_ground || u.surface == 200 {
            continue;
        }
        let key = |v: V3| {
            (
                (v.x / grid).floor(),
                (v.y / grid).floor(),
                (v.z / grid).floor(),
            )
        };
        if out.iter().any(|&o| key(o) == key(u.pos)) {
            continue;
        }
        out.push(u.pos);
    }
}

/// Plane normal y of every polygon of every collision model (same order as `collision_world`).
fn face_normals_y(world: &World<'_>) -> Vec<Vec<f32>> {
    dc_sim::from_world::selected_models(world, &WorldFilter::default())
        .into_iter()
        .map(|i| {
            let m = &world.models[i];
            m.polygons
                .iter()
                .map(|p| m.planes[p.plane as usize].normal[1])
                .collect()
        })
        .collect()
}

fn face_ny(faces: &[Vec<f32>], h: &dc_sim::Hit) -> f32 {
    faces
        .get(h.model as usize)
        .and_then(|m| m.get(h.poly as usize))
        .copied()
        .unwrap_or(h.normal.y)
}

fn sweep(w: &CollisionWorld, world: &World<'_>, p: &MovementParams, dims: V3, csvs: &[PathBuf]) {
    let faces = face_normals_y(world);
    let mut starts: Vec<V3> = world
        .objects_with_role(ObjectRole::PlayerSpawn)
        .filter_map(|o| Some(V3::from_array(o.position()?)))
        .collect();
    let spawns = starts.len();
    for c in csvs {
        starts_from_csv(c, 48.0, &mut starts);
    }
    let angles = 16;
    let jobs: Vec<(V3, f32)> = starts
        .iter()
        .flat_map(|&s| {
            (0..angles).map(move |k| (s, k as f32 * core::f32::consts::TAU / angles as f32))
        })
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let (stats, mut spots) = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                sc.spawn(|| {
                    let mut st = SweepStats::default();
                    let mut sp = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&(s, yaw)) = jobs.get(i) else {
                            break;
                        };
                        run_once(w, p, dims, s, yaw, 90, &mut st, &mut sp, &faces);
                    }
                    (st, sp)
                })
            })
            .collect();
        let mut tot = SweepStats::default();
        let mut spots = Vec::new();
        for h in hs {
            let (st, sp) = h.join().unwrap();
            tot.runs += st.runs;
            for k in 0..4 {
                tot.depth[k] += st.depth[k];
            }
            tot.overlaps += st.overlaps;
            tot.tunnels += st.tunnels;
            tot.legit_ticks += st.legit_ticks;
            tot.ticks += st.ticks;
            tot.stuck_ticks += st.stuck_ticks;
            tot.stuck_runs += st.stuck_runs;
            tot.episodes += st.episodes;
            spots.extend(sp);
        }
        (tot, spots)
    });
    spots.sort_by(|a, b| {
        (a.0.x, a.0.z, a.1)
            .partial_cmp(&(b.0.x, b.0.z, b.1))
            .unwrap()
    });
    println!(
        "sweep: {} starts ({} spawns), {} angles, {} runs placed, {} ticks; stuck ticks {} ({:.3} %), runs with a catch {}, episodes (>= 3 ticks) {}; blocked by a steep slope or embedded (not counted) {}; tunnels {}; new overlaps {} (cleared by a lift of <= 0.1 / 0.5 / 2 / more: {:?})",
        starts.len(),
        spawns,
        angles,
        stats.runs,
        stats.ticks,
        stats.stuck_ticks,
        100.0 * stats.stuck_ticks as f64 / stats.ticks.max(1) as f64,
        stats.stuck_runs,
        stats.episodes,
        stats.legit_ticks,
        stats.tunnels,
        stats.overlaps,
        stats.depth
    );
    if std::env::var("EDGE_SPOTS").is_ok() {
        for (p, y) in spots.iter() {
            println!("  spot {:.1} {:.1} {:.1} yaw {:.3}", p.x, p.y, p.z, y);
        }
    }
}

/// Alive intervals from the capanalyze `events.csv` next to the stream (server state 1 = alive); every time
/// counts as alive when the file is missing.
fn alive_at(csv: &Path) -> impl Fn(f32) -> bool {
    let mut changes: Vec<(f32, bool)> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(csv.with_file_name("events.csv")) {
        for l in text.lines().skip(1) {
            let f: Vec<&str> = l.splitn(5, ',').collect();
            if f.len() == 5 && f[2] == "s2c_state" {
                let alive = f[4].contains("\"state\"\": 1}");
                if let Ok(t) = f[0].parse::<f32>() {
                    changes.push((t, alive));
                }
            }
        }
    }
    move |t: f32| {
        if changes.is_empty() {
            return true;
        }
        let mut alive = false;
        for &(ct, a) in &changes {
            if ct <= t {
                alive = a;
            }
        }
        alive
    }
}

fn replay(w: &CollisionWorld, p: &MovementParams, dims: V3, csv: &Path) {
    let ups = read_updates(csv);
    let alive = alive_at(csv);
    let mut errs: Vec<f32> = Vec::new();
    let mut blocked = 0usize;
    let mut blocked_spots = Vec::new();
    let mut pairs = 0usize;
    let mut client_moving = 0usize;
    let mut dump: Vec<String> = Vec::new();
    for ab in ups.windows(2) {
        let (a, b) = (ab[0], ab[1]);
        let dt = b.t - a.t;
        if !(0.03..0.12).contains(&dt) || !a.on_ground || !b.on_ground {
            continue;
        }
        if a.control & (bits::JUMP | bits::CROUCH) != 0 || a.surface == 200 || b.surface == 200 {
            continue;
        }
        if (b.pos - a.pos).len() > 60.0 || !alive(a.t) || !alive(b.t) {
            continue; // teleport / respawn, dead or waiting
        }
        let mut s = PlayerState::new(a.pos, dims, p);
        s.vel = V3::new(a.vel.x, 0.0, a.vel.z);
        s.controls = a.control;
        s.prev_controls = a.control;
        s.ramp_fwd = p.max_accel;
        s.ramp_side = p.max_accel;
        ground_probe(
            w,
            &s.shape(),
            s.pos,
            &mut s.contact,
            p.step(),
            p.standable_cos,
        );
        s.on_ground = s.contact.is_some();
        if !s.on_ground {
            continue;
        }
        pairs += 1;
        let n = ((dt / DT).round() as usize).max(1);
        for i in 0..n {
            let f = (i as f32 + 0.5) / n as f32;
            let input = TickInput {
                commands: commands_of(a.control),
                yaw: a.yaw + wrap(b.yaw - a.yaw) * f,
                ..TickInput::default()
            };
            s.step(p, w, &input, dt / n as f32);
        }
        let e = (s.pos - b.pos).len();
        errs.push(e);
        // something solid within 10 units along the wish direction (lifted 1 unit off the floor)
        let wish = {
            let (f, r) = (
                (a.control & bits::FWD != 0) as i32 - (a.control & bits::BACK != 0) as i32,
                (a.control & bits::RIGHT != 0) as i32 - (a.control & bits::LEFT != 0) as i32,
            );
            let fw = V3::new(a.yaw.sin(), 0.0, a.yaw.cos());
            let rt = V3::new(a.yaw.cos(), 0.0, -a.yaw.sin());
            (fw * f as f32 + rt * r as f32).normalized()
        };
        let lifted = a.pos + V3::new(0.0, 1.0, 0.0);
        let near = !wish.is_zero()
            && w.sweep(&Cylinder::from_dims(dims), lifted, lifted + wish * 10.0)
                .is_some();
        dump.push(format!(
            "{:.3},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.3},{:.1},{:.2},{:.2},{:.2},{},{}",
            a.t,
            a.pos.x,
            a.pos.y,
            a.pos.z,
            s.pos.x,
            s.pos.y,
            s.pos.z,
            e,
            a.vel.len_xz(),
            b.pos.x,
            b.pos.y,
            b.pos.z,
            a.control,
            near as u8
        ));
        let cm = (b.pos - a.pos).len_xz();
        if cm > 5.0 && a.control & (bits::FWD | bits::BACK | bits::LEFT | bits::RIGHT) != 0 {
            client_moving += 1;
            let sm = (s.pos - a.pos).len_xz();
            if sm < 0.3 * cm {
                blocked += 1;
                blocked_spots.push((a.t, a.pos, b.pos, s.pos));
            }
        }
    }
    if let Ok(path) = std::env::var("EDGE_DUMP") {
        let name = csv
            .parent()
            .and_then(|p| p.file_name())
            .map_or("x".into(), |n| n.to_string_lossy().to_string());
        std::fs::write(
            format!("{path}_{name}.csv"),
            dump.join(
                "
",
            ),
        )
        .expect("dump");
    }
    errs.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let q = |f: f32| {
        errs.get(((errs.len() as f32 - 1.0) * f) as usize)
            .copied()
            .unwrap_or(0.0)
    };
    let mean = errs.iter().sum::<f32>() / errs.len().max(1) as f32;
    let over = |l: f32| errs.iter().filter(|&&e| e > l).count();
    println!(
        "replay {}: {} ground pairs; error mean {:.3} median {:.3} p90 {:.3} p99 {:.3} max {:.3}; > 2 units {}, > 5 units {}; client moving {} of which dc-sim blocked {}",
        csv.display(),
        pairs,
        mean,
        q(0.5),
        q(0.9),
        q(0.99),
        q(1.0),
        over(2.0),
        over(5.0),
        client_moving,
        blocked
    );
    if std::env::var("EDGE_SPOTS").is_ok() {
        for (t, a, b, s) in blocked_spots {
            println!(
                "  blocked t {t:.2} from ({:.1},{:.1},{:.1}) client ({:.1},{:.1},{:.1}) sim ({:.1},{:.1},{:.1})",
                a.x, a.y, a.z, b.x, b.y, b.z, s.x, s.y, s.z
            );
        }
    }
}

fn wrap(a: f32) -> f32 {
    let t = core::f32::consts::TAU;
    let mut x = a % t;
    if x > core::f32::consts::PI {
        x -= t;
    }
    if x < -core::f32::consts::PI {
        x += t;
    }
    x
}

fn trace(w: &CollisionWorld, p: &MovementParams, dims: V3, at: V3, yaw: f32, ticks: usize) {
    let mut s = PlayerState::new(at, dims, p);
    if let Ok(v) = std::env::var("EDGE_VEL") {
        let f: Vec<f32> = v.split(',').map(|x| x.parse().unwrap()).collect();
        s.vel = V3::new(f[0], f[1], f[2]);
        s.ramp_fwd = p.max_accel;
        s.ramp_side = p.max_accel;
        s.controls = bits::FWD | bits::RUN | bits::ANY_MOVE;
        s.prev_controls = s.controls;
    }
    ground_probe(
        w,
        &s.shape(),
        s.pos,
        &mut s.contact,
        p.step(),
        p.standable_cos,
    );
    let input = TickInput {
        commands: Commands {
            forward: true,
            run: true,
            ..Commands::default()
        },
        yaw,
        ..TickInput::default()
    };
    let dir = V3::new(yaw.sin(), 0.0, yaw.cos());
    for i in 0..ticks {
        let before = s.pos;
        s.step(p, w, &input, DT);
        let shape = s.shape();
        let probe = w.sweep(&shape, s.pos, s.pos + dir * 4.0);
        println!(
            "tick {i:3} pos ({:.3},{:.3},{:.3}) moved {:.3} vel ({:.1},{:.1},{:.1}) ground {} contact {:?} ahead {:?} free_above {}",
            s.pos.x,
            s.pos.y,
            s.pos.z,
            (s.pos - before).len_xz(),
            s.vel.x,
            s.vel.y,
            s.vel.z,
            s.on_ground,
            s.contact.map(|c| (c.normal, c.model, c.poly)),
            probe.map(|h| (h.t, h.normal, h.model, h.poly)),
            free_above_step(w, p, &s, dir, 4.0)
        );
    }
}

/// The steps of the stair move at one position, printed (same calls as dc_sim::stair_move).
fn stair_debug(w: &CollisionWorld, p: &MovementParams, dims: V3, p0: V3, yaw: f32, len: f32) {
    let shape = Cylinder::from_dims(dims);
    let d = V3::new(yaw.sin(), 0.0, yaw.cos()) * len;
    let step = p.step();
    println!("start {p0:?} d {d:?} overlaps {}", w.overlaps(&shape, p0));
    let down = V3::new(p0.x, p0.y - 0.1 * shape.half_height, p0.z);
    println!("slope probe {:?}", w.sweep(&shape, p0, down));
    let h1 = w.sweep(&shape, p0, p0 + d);
    println!("first sweep {h1:?}");
    let r1 = dc_sim::slide_move(w, &shape, p0, p0 + d);
    println!("r1 {r1:?}");
    let up = p0 + V3::new(0.0, step, 0.0);
    println!("up probe {:?}", w.sweep(&shape, p0, up));
    println!("raised sweep {:?}", w.sweep(&shape, up, up + d));
    let r2 = dc_sim::slide_move(w, &shape, up, up + d);
    println!("r2 {r2:?}");
    let low = r2.pos - V3::new(0.0, step, 0.0);
    let hd = w.sweep(&shape, r2.pos, low);
    println!("down probe {hd:?}");
    if let Some(hh) = hd {
        let land = r2.pos + (low - r2.pos) * hh.t;
        println!(
            "landing {land:?} progress {} vs r1 {}",
            (land - p0).dot(d),
            (r1.pos - p0).dot(d)
        );
    }
}

/// Every polygon of every model with a corner within `rad` of `at` (collision model index when selected).
fn geom(world: &World<'_>, at: V3, rad: f32) {
    println!("blockers in the world: {}", world.blockers.polys.len());
    let sel = dc_sim::from_world::selected_models(world, &WorldFilter::default());
    for (wi, m) in world.models.iter().enumerate() {
        let (lo, hi) = (V3::from_array(m.box_min), V3::from_array(m.box_max));
        let inside = at.x > lo.x - rad
            && at.x < hi.x + rad
            && at.y > lo.y - rad
            && at.y < hi.y + rad
            && at.z > lo.z - rad
            && at.z < hi.z + rad;
        if inside {
            let o = world.object_named(m.name);
            println!(
                "model {wi} {:?} selected {} class {:?} role {:?} box {:?}..{:?} polys {}",
                String::from_utf8_lossy(m.name),
                sel.contains(&wi),
                o.map(|o| String::from_utf8_lossy(o.class_name).to_string()),
                o.and_then(|o| o.role),
                m.box_min,
                m.box_max,
                m.polygons.len()
            );
        }
    }
    for o in world.objects.iter() {
        let Some(pos) = o.position() else { continue };
        let pv = V3::from_array(pos);
        if (pv - at).len() < rad * 3.0 {
            println!(
                "object {:?} name {:?} role {:?} at {:?} dist {:.1}",
                String::from_utf8_lossy(o.class_name),
                o.name().map(String::from_utf8_lossy),
                o.role,
                pos,
                (pv - at).len()
            );
        }
    }
    for (i, b) in world.blockers.polys.iter().enumerate() {
        let mut lo = V3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut hi = -lo;
        for v in &b.verts {
            let v = V3::from_array(*v);
            lo = V3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
            hi = V3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
        }
        let inb = at.x > lo.x - rad
            && at.x < hi.x + rad
            && at.y > lo.y - rad
            && at.y < hi.y + rad
            && at.z > lo.z - rad
            && at.z < hi.z + rad;
        if inb {
            println!(
                "blocker {i} n {:?} d {} verts {:?}",
                b.normal, b.dist, b.verts
            );
        }
    }
    let sel = dc_sim::from_world::selected_models(world, &WorldFilter::default());
    for (wi, m) in world.models.iter().enumerate() {
        let ci = sel.iter().position(|&x| x == wi);
        for (pi, poly) in m.polygons.iter().enumerate() {
            let pts: Vec<V3> = m
                .polygon_points(poly)
                .iter()
                .map(|&i| V3::from_array(m.points[i as usize]))
                .collect();
            let want = std::env::var("EDGE_POLY").ok().map(|v| {
                let f: Vec<usize> = v.split(',').map(|x| x.parse().unwrap()).collect();
                (f[0], f[1])
            });
            if let Some((cm, cp)) = want {
                if ci != Some(cm) || pi != cp {
                    continue;
                }
            } else if !{
                let mut lo = V3::new(f32::MAX, f32::MAX, f32::MAX);
                let mut hi = -lo;
                for v in &pts {
                    lo = V3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
                    hi = V3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
                }
                at.x > lo.x - rad
                    && at.x < hi.x + rad
                    && at.y > lo.y - rad
                    && at.y < hi.y + rad
                    && at.z > lo.z - rad
                    && at.z < hi.z + rad
            } {
                continue;
            }
            let pl = m.planes[poly.plane as usize];
            let fl = m.surfaces[poly.surface as usize].flags;
            println!(
                "model {wi} coll {ci:?} name {:?} poly {pi} n ({:?},{:?},{:?}) d {:?} solid {} pts {:?}",
                String::from_utf8_lossy(m.name),
                pl.normal[0],
                pl.normal[1],
                pl.normal[2],
                pl.dist,
                fl & 1,
                pts.iter().map(|v| (v.x, v.y, v.z)).collect::<Vec<_>>()
            );
        }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let rom = PathBuf::from(std::env::var("CA_ROM").expect("set CA_ROM to the client folder"));
    let mut map = "JUNK_FLEA".to_string();
    if let Some(i) = args.iter().position(|a| a == "--map") {
        map = args[i + 1].clone();
        args.drain(i..i + 2);
    }
    let dims = stand_dims(&rom);
    let bytes = world_bytes(&rom, &map);
    let world = World::parse_with(&bytes, ParseOptions { render: false }).expect("world parses");
    let w = collision_world(&world, &WorldFilter::default());
    let p = MovementParams::default();
    let mode = args.first().cloned().unwrap_or_else(|| "sweep".into());
    match mode.as_str() {
        "sweep" => {
            let mut csvs = Vec::new();
            let mut i = 1;
            while i < args.len() {
                if args[i] == "--csv" {
                    csvs.push(PathBuf::from(&args[i + 1]));
                    i += 2;
                } else {
                    i += 1;
                }
            }
            sweep(&w, &world, &p, dims, &csvs);
        }
        "replay" => {
            for a in &args[1..] {
                replay(&w, &p, dims, Path::new(a));
            }
        }
        "trace" => {
            let f = |i: usize| args[i].parse::<f32>().expect("number");
            let ticks = args.get(5).map_or(60, |t| t.parse().unwrap());
            trace(&w, &p, dims, V3::new(f(1), f(2), f(3)), f(4), ticks);
        }
        "stair" => {
            let f = |i: usize| args[i].parse::<f32>().expect("number");
            stair_debug(&w, &p, dims, V3::new(f(1), f(2), f(3)), f(4), f(5));
        }
        "probe" => {
            let f = |i: usize| args[i].parse::<f32>().expect("number");
            let shape = Cylinder::from_dims(dims);
            let from = V3::new(f(1), f(2), f(3));
            let to = V3::new(f(4), f(5), f(6));
            println!(
                "overlaps {} hit {:?}",
                w.overlaps(&shape, from),
                w.sweep(&shape, from, to)
            );
        }
        "geom" => {
            let f = |i: usize| args[i].parse::<f32>().expect("number");
            geom(&world, V3::new(f(1), f(2), f(3)), f(4));
        }
        m => panic!("unknown mode {m}"),
    }
}
