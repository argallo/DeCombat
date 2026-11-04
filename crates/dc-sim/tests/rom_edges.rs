//! ROM-gated (skipped when CA_ROM is unset): edge catches. From every player spawn of every world of the user's
//! client the body runs forward for 1.5 s in 16 directions. A tick is an edge catch when the body stands, moved less
//! than 0.5 units, nothing solid is ahead of its part above step height, the obstacle below is not a steep slope
//! (no walking up it, MOVEMENT_2009.md §9.7 step 1) and the body is not embedded: the 2009 mover steps over such an
//! obstacle or slides along it. The standing box is read from the user's own player models at run time; maps are
//! reported by index and byte size only, no ROM value is stored here.
// SPEC: MOVEMENT_2009.md §9.5-§9.9, §10.1, §16.2 V-M6 / V-M10
// EVIDENCE: engine+0x77530 / engine+0x76120 (polygon and edge sweep), engine+0x78c50 (stair move),
// engine+0x79210 (ground probe)
#![cfg(feature = "world")]

use std::sync::atomic::{AtomicUsize, Ordering};

use dc_sim::from_world::{collision_world, WorldFilter};
use dc_sim::{
    Collider, CollisionWorld, Commands, Cylinder, MovementParams, PlayerState, TickInput, V3,
};
use dc_world::{ObjectRole, ParseOptions, World};

/// sha256(stand animation name)[:8], length 3 (MOVEMENT_2009.md §9.2).
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
    panic!("no player model with the stand clip in the ROM")
}

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
struct Counts {
    runs: usize,
    ticks: usize,
    catches: usize,
    episodes: usize,
    tunnels: usize,
}

impl Counts {
    fn add(&mut self, o: &Counts) {
        self.runs += o.runs;
        self.ticks += o.ticks;
        self.catches += o.catches;
        self.episodes += o.episodes;
        self.tunnels += o.tunnels;
    }
}

/// Plane normal y of every polygon of every collision model, in the order of `collision_world`.
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

fn run(
    w: &CollisionWorld,
    faces: &[Vec<f32>],
    p: &MovementParams,
    dims: V3,
    start: V3,
    yaw: f32,
) -> Counts {
    const DT: f32 = 1.0 / 60.0;
    let mut c = Counts::default();
    let shape = Cylinder::from_dims(dims);
    let placed = dc_sim::drop_to_floor(w, &shape, start, p.step(), 200.0);
    if w.overlaps(&shape, placed) {
        return c;
    }
    let mut s = PlayerState::new(placed, dims, p);
    for _ in 0..20 {
        s.step(p, w, &TickInput::default(), DT);
    }
    if !s.on_ground {
        return c;
    }
    c.runs = 1;
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
    // the body's part above step height, as its own cylinder
    let h = shape.half_height;
    let upper_half = (2.0 * h - p.step() - 1.0) * 0.5;
    let upper = Cylinder::new(shape.radius, upper_half);
    let mut streak = 0;
    for i in 0..90 {
        let before = s.pos;
        s.step(p, w, &input, DT);
        c.ticks += 1;
        if w.ray(before, s.pos).is_some() {
            c.tunnels += 1;
        }
        if i < 15 || !s.on_ground || (s.pos - before).len_xz() >= 0.5 {
            streak = 0;
            continue;
        }
        let uc = V3::new(s.pos.x, s.pos.y + h - upper_half, s.pos.z);
        let free_above = !w.overlaps(&upper, uc) && w.sweep(&upper, uc, uc + dir * 4.0).is_none();
        let steep = w
            .sweep(&shape, s.pos, s.pos + dir * 4.0)
            .is_some_and(|hit| {
                let ny = faces
                    .get(hit.model as usize)
                    .and_then(|m| m.get(hit.poly as usize))
                    .copied()
                    .unwrap_or(hit.normal.y);
                ny > 0.001 && ny < p.standable_cos
            });
        if free_above && !steep && !w.overlaps(&shape, s.pos) {
            c.catches += 1;
            streak += 1;
            if streak == 3 {
                c.episodes += 1;
            }
        } else {
            streak = 0;
        }
    }
    c
}

#[test]
fn rom_runs_do_not_catch_on_edges() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let dims = stand_dims(&rom);
    let p = MovementParams::default();
    let mut total = Counts::default();
    for (mi, data) in world_files(&rom).iter().enumerate() {
        let world = World::parse_with(data, ParseOptions { render: false }).expect("world parses");
        let w = collision_world(&world, &WorldFilter::default());
        let faces = face_normals_y(&world);
        let jobs: Vec<(V3, f32)> = world
            .objects_with_role(ObjectRole::PlayerSpawn)
            .filter_map(|o| Some(V3::from_array(o.position()?)))
            .flat_map(|s| (0..16).map(move |k| (s, k as f32 * std::f32::consts::TAU / 16.0)))
            .collect();
        let next = AtomicUsize::new(0);
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(32);
        let m = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..threads)
                .map(|_| {
                    sc.spawn(|| {
                        let mut c = Counts::default();
                        while let Some(&(s, yaw)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                            c.add(&run(&w, &faces, &p, dims, s, yaw));
                        }
                        c
                    })
                })
                .collect();
            let mut m = Counts::default();
            for h in hs {
                m.add(&h.join().unwrap());
            }
            m
        });
        println!(
            "map {mi:2} bytes {:9}: runs {:5} ticks {:7} edge-catch ticks {:5} episodes {:4} tunnels {}",
            data.len(),
            m.runs,
            m.ticks,
            m.catches,
            m.episodes,
            m.tunnels
        );
        total.add(&m);
    }
    println!("total {total:?}");
    assert!(total.runs > 20_000, "{total:?}");
    assert_eq!(total.tunnels, 0, "the body centre crossed a solid surface");
    // OBSERVED (counts only, 2026-10-25): before the 2009 edge sweep (conservative polytope clip) the runs over
    // the 21 maps caught on edges in thousands of ticks; with the 2009 rule the remaining catches are steps
    // whose raised landing is steep and similar spots (one run in hundreds)
    assert!(
        (total.catches as f64) < 0.003 * total.ticks as f64,
        "edge catches {} of {} ticks",
        total.catches,
        total.ticks
    );
}
