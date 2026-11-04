//! ROM-gated (skipped when CA_ROM is unset): visibility culling on the user's own worlds. Nothing from the ROM is
//! asserted by value or printed by name; output is counts, ratios, timings and world-hash prefixes only.
//!
//! - the occluder BVH (level model only) agrees with `dc_sim::CollisionWorld::ray` on spawn-to-spawn segments;
//! - the per-tick cost of culling in an 8v8 session of synthetic clients at 30 Hz on the ROM world with the most
//!   occluder triangles (`DC_VIS_SECS` simulated seconds, default 20; release numbers are the meaningful ones).

use dc_dserver::game::GameTuning;
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::visibility::{Occluders, VisTuning};
use dc_dserver::world::{
    load_from_rom, world_from_bytes, NameForm, WorldInfo, WORLD_ARCHIVE, WORLD_EXT,
};
use dc_net_engine::sim::{Faults, SimChannel};

fn env_num(k: &str, d: usize) -> usize {
    std::env::var(k)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(d)
}

/// Every world entry of the archive with its decrypted bytes.
fn world_bytes(rom: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let Ok(arch) = dc_rez::RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE)) else {
        return Vec::new();
    };
    let names: Vec<String> = arch
        .entries()
        .iter()
        .map(|e| e.0.clone())
        .filter(|n| n.to_ascii_uppercase().ends_with(WORLD_EXT))
        .collect();
    let key = dc_rez::dc_rom::crypt_key(rom).ok();
    names
        .into_iter()
        .filter_map(|n| {
            let raw = arch.entry_raw(&n).ok()??;
            let v85 = dc_world::WORLD_VERSION.to_le_bytes();
            let data = if raw.starts_with(&v85) {
                raw
            } else {
                dc_rez::decrypt_with_key(&raw, key.as_ref()?).ok()?
            };
            data.starts_with(&v85).then_some((n, data))
        })
        .collect()
}

fn eye(p: [f32; 3], up: f32) -> [f32; 3] {
    [p[0], p[1] + up, p[2]]
}

/// BVH (level model only, so that both sides use the same polygons) against the collision ray of dc-sim on
/// segments from spawn eyes to five points of other spawns. Every world; `DC_VIS_SEGMENTS` per world (default 300).
#[test]
fn rom_worlds_occluder_bvh_agrees_with_the_collision_ray() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let worlds = world_bytes(&rom);
    if worlds.is_empty() {
        println!("skipped: world archive unavailable");
        return;
    }
    let per_world = env_num("DC_VIS_SEGMENTS", 300);
    let (mut total, mut agree, mut bvh_only, mut sim_only) = (0usize, 0usize, 0usize, 0usize);
    let (mut t_bvh, mut t_sim) = (0u128, 0u128);
    for (name, data) in &worlds {
        let w = dc_world::World::parse_with(data, dc_world::ParseOptions { render: false })
            .expect("world parses");
        let level = Occluders::from_world(&w, &[]);
        let info = world_from_bytes(data, name, "w").unwrap();
        let sim = info.trace.as_ref().expect("a level model");
        let tag = &info.sha256[..12];
        let spawns: Vec<[f32; 3]> = info.spawns.iter().map(|s| s.pos).collect();
        if spawns.len() < 2 {
            continue;
        }
        let mut n = 0usize;
        let (mut a, mut b_only, mut s_only) = (0usize, 0usize, 0usize);
        'outer: for (i, s) in spawns.iter().enumerate() {
            for (j, t) in spawns.iter().enumerate() {
                if i == j || (i * 31 + j * 17) % 7 != 0 {
                    continue;
                }
                let e = eye(*s, 70.0);
                for target in [
                    eye(*t, 30.0),
                    eye(*t, 75.0),
                    eye(*t, -20.0),
                    [t[0] + 20.0, t[1] + 30.0, t[2]],
                    [t[0], t[1] + 30.0, t[2] + 20.0],
                ] {
                    let t0 = std::time::Instant::now();
                    let bvh = level.blocked(e, target);
                    t_bvh += t0.elapsed().as_nanos();
                    let t0 = std::time::Instant::now();
                    let ray = sim
                        .ray(dc_sim::V3::from_array(e), dc_sim::V3::from_array(target))
                        .is_some();
                    t_sim += t0.elapsed().as_nanos();
                    n += 1;
                    match (bvh, ray) {
                        (x, y) if x == y => a += 1,
                        (true, false) => b_only += 1,
                        _ => s_only += 1,
                    }
                    if n >= per_world {
                        break 'outer;
                    }
                }
            }
        }
        println!(
            "world {tag}: {} level triangles, {n} segments, agree {:.2} %, bvh-only blocked {b_only}, ray-only blocked {s_only}",
            level.triangles(),
            100.0 * a as f64 / n.max(1) as f64
        );
        total += n;
        agree += a;
        bvh_only += b_only;
        sim_only += s_only;
    }
    println!(
        "all: {total} segments, agree {:.3} %, bvh-only {bvh_only}, ray-only {sim_only}; mean bvh {:.2} us, mean ray {:.2} us",
        100.0 * agree as f64 / total.max(1) as f64,
        t_bvh as f64 / total.max(1) as f64 / 1000.0,
        t_sim as f64 / total.max(1) as f64 / 1000.0
    );
    assert!(total > 0);
    // the design's census found 99.86 .. 100 % per world; allow a little slack for the sample
    assert!(
        agree * 1000 >= total * 995,
        "agreement {agree} / {total} below 99.5 %"
    );
}

/// The 8v8 cost: 16 synthetic clients (8 per team) walking on the ROM world with the most occluder triangles,
/// 30 Hz ticks, culling on. Reports the per-tick cost of the culling update.
#[test]
fn rom_largest_world_8v8_culling_cost_at_30hz() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let worlds = world_bytes(&rom);
    if worlds.is_empty() {
        println!("skipped: world archive unavailable");
        return;
    }
    // the world with the most occluder triangles (VIS-3 set)
    let (listed, tris) = worlds
        .iter()
        .map(|(n, d)| {
            let w = world_from_bytes(d, n, "w").unwrap();
            (n.clone(), w.occluders.as_ref().map_or(0, |o| o.triangles()))
        })
        .max_by_key(|x| x.1)
        .unwrap();
    let base = listed.rsplit('\\').next().unwrap_or(&listed);
    let arg = format!("Worlds\\{}", &base[..base.len() - WORLD_EXT.len()]);
    let w: WorldInfo = load_from_rom(&rom, &arg, NameForm::Listed, None).unwrap();
    let tag = w.sha256[..12].to_string();
    let wbox = (w.extent_min, w.extent_max);
    let (tables, _) = RomTables::load(&rom);
    let secs = env_num("DC_VIS_SECS", 20) as u32;
    let cfg = ServerTuning {
        tick_hz: 30,
        vis: VisTuning::default(),
        game: GameTuning {
            trace: false,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut server: Server<u32> = Server::with_tables(cfg, w, tables, 0);
    server.handle_line(0, "room key=TDM type=1 goal=500 ff=0");
    let names: Vec<String> = (0..16).map(|i| format!("Bot{i:02}")).collect();
    for (i, n) in names.iter().enumerate() {
        let line = format!("user {} {} 50 {} 0 {n}", 100 + i, 200 + i, i % 2);
        assert!(server.handle_line(0, &line));
    }
    let mut clients: Vec<SynthClient> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut c = SynthConfig::named(n);
            c.box_bounds = Some(wbox);
            // walking along x at 15 to 75 units/s, both directions
            let sign = if i % 3 == 0 { -1.0 } else { 1.0 };
            c.walk_step = sign * (1.0 + (i % 5) as f32);
            SynthClient::new(c)
        })
        .collect();
    let mut chans: Vec<(SimChannel, SimChannel)> = (0..16u64)
        .map(|i| {
            (
                SimChannel::new(100 + i, Faults::clean(2)),
                SimChannel::new(200 + i, Faults::clean(2)),
            )
        })
        .collect();
    let mut now = 0u32;
    let mut step = |server: &mut Server<u32>, clients: &mut Vec<SynthClient>, now: u32| {
        for (i, c) in clients.iter_mut().enumerate() {
            for d in c.poll(now) {
                chans[i].0.send(now, d);
            }
        }
        for (i, ch) in chans.iter_mut().enumerate() {
            for d in ch.0.deliver(now) {
                server.handle_datagram(now, i as u32 + 1, &d);
            }
        }
        let mut logs = Vec::new();
        for o in server.poll(now) {
            match o {
                Output::Transmit { to, bytes } => chans[to as usize - 1].1.send(now, bytes),
                Output::Log(l) if l.contains("visibility:") => logs.push(l),
                _ => {}
            }
        }
        for (i, ch) in chans.iter_mut().enumerate() {
            for d in ch.1.deliver(now) {
                clients[i].handle_datagram(now, &d);
            }
        }
        logs
    };
    while now < 60_000 && !clients.iter().all(|c| c.life_state() == 1) {
        step(&mut server, &mut clients, now);
        now += 5;
    }
    assert!(
        clients.iter().all(|c| c.life_state() == 1),
        "world {tag}: not all 16 alive"
    );
    let s0 = server.visibility().stats;
    let end = now + secs * 1000;
    while now < end {
        for l in step(&mut server, &mut clients, now) {
            println!("{l}");
        }
        now += 5;
    }
    let s = server.visibility().stats;
    let ticks = s.updates - s0.updates;
    let nanos = s.nanos - s0.nanos;
    let segs = s.segments - s0.segments;
    let mean_us = nanos as f64 / ticks.max(1) as f64 / 1000.0;
    let per_seg = if segs > 0 {
        nanos as f64 / segs as f64 / 1000.0
    } else {
        0.0
    };
    // how many enemy pairs are parked at the end
    let pids: Vec<u8> = clients
        .iter()
        .map(|c| c.seen.client_id.unwrap().0 as u8)
        .collect();
    let objs: Vec<u16> = clients
        .iter()
        .map(|c| c.seen.view_object.unwrap())
        .collect();
    let mut parked = 0;
    for (i, &v) in pids.iter().enumerate() {
        for (j, &o) in objs.iter().enumerate() {
            if i % 2 != j % 2 && server.visibility().parked(v, o) {
                parked += 1;
            }
        }
    }
    println!(
        "world {tag}: {tris} occluder triangles; 8v8 at 30 Hz for {secs} s: {ticks} ticks, culling mean {mean_us:.1} us/tick \
         (max {:.1} us), {:.1} segments/tick (max {}), {per_seg:.2} us/segment, {:.2} % of a core; {} tests, {} fail-open \
         verdicts, {} parks, {} refreshes, {} shows; {parked} of 128 enemy pairs parked at the end",
        s.max_nanos as f64 / 1000.0,
        segs as f64 / ticks.max(1) as f64,
        s.max_segments,
        mean_us * 30.0 / 1e4,
        s.tests - s0.tests,
        s.fail_open - s0.fail_open,
        s.parks,
        s.refreshes,
        s.shows
    );
    assert!(ticks as u32 >= secs * 29, "{ticks} ticks");
    assert!(s.parks > 0, "something was culled");
    for c in &clients {
        assert!(c.seen.errors.is_empty(), "{:?}", c.seen.errors);
    }
}
