//! ROM-gated (skipped without CA_ROM): the co-op AI find their way through the two co-op worlds of the ROM (FT-S11).
//! Humans stand at their start points (the AI do not hit them, nobody shoots the AI) for a few minutes of simulated
//! time; every 250 ms each living AI is looked at. A stuck episode is 8 s or more within 48 units of one place while the
//! AI wants to move (its nearest human more than 96 units away and not in sight within the hold range). Printed per
//! world: the episodes, the time from an AI's spawn to its contact (the nearest human in sight within the hold range),
//! the navigation counters and the planning time per AI step. `NAV_BEFORE=1` also runs the same with `coop.nav=off`
//! (FT-S5's steering) for the comparison. No ROM value is asserted or written; worlds are named by hash.
// OBSERVED: ROM SANDHOG_CP1 and the wave map (their spawners, placed AI and start points, read at run time)
// DC-DEFAULT: FT-S11 (proposed)

use dc_bute::GameRoomData;
use dc_dserver::game::coop::AiState;
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{load_from_rom, NameForm, WorldInfo};
use dc_net_engine::sim::{Faults, SimChannel};
use dc_rez::RezVfs;
use dc_sim::{Collider, Cylinder, V3};

struct Peer {
    addr: u32,
    client: SynthClient,
    up: SimChannel,
    down: SimChannel,
}

fn step(server: &mut Server<u32>, peers: &mut [Peer], now: u32) {
    for p in peers.iter_mut() {
        for d in p.client.poll(now) {
            p.up.send(now, d);
        }
    }
    for p in peers.iter_mut() {
        for d in p.up.deliver(now) {
            server.handle_datagram(now, p.addr, &d);
        }
    }
    for o in server.poll(now) {
        match o {
            Output::Transmit { to, bytes } => {
                if let Some(p) = peers.iter_mut().find(|p| p.addr == to) {
                    p.down.send(now, bytes);
                }
            }
            Output::Log(l) if std::env::var("NAV_LOG").is_ok() => println!("{now}: {l}"),
            _ => {}
        }
    }
    for p in peers.iter_mut() {
        for d in p.down.deliver(now) {
            p.client.handle_datagram(now, &d);
        }
    }
}

/// `(mode key, Worlds\<name>, mission world)` of every map row with a GMS for the class-5 mode row.
fn coop_worlds(vfs: &RezVfs) -> Vec<(String, String)> {
    let Ok(gr) = GameRoomData::from_vfs(vfs) else {
        return Vec::new();
    };
    let modes = gr.modes(false);
    let Some((mi, key)) = modes.iter().enumerate().find_map(|(i, m)| {
        let t = m.get("GameType")?.trim().trim_matches('"').to_string();
        let k = m.get("Key")?.trim().trim_matches('"').to_string();
        (t == "5").then_some((i, k))
    }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, m) in gr.maps().iter().enumerate() {
        if !m.contains_key(&format!("GMS_{key}")) {
            continue;
        }
        let (_md, doc) = dc_bute::gms_file_with_vfs(vfs, &gr, i, mi, false);
        let Some(world) = doc.get_str("MapInfo", "Map") else {
            continue;
        };
        let w = world.trim().trim_matches('"').replace('/', "\\");
        let base = w.rsplit('\\').next().unwrap_or(&w).to_string();
        let base = base
            .strip_suffix(".dat")
            .or_else(|| base.strip_suffix(".DAT"))
            .unwrap_or(&base)
            .to_string();
        let arg = format!("Worlds\\{base}");
        if !out.iter().any(|o: &(String, String)| o.1 == arg) {
            out.push((key.clone(), arg));
        }
    }
    out
}

fn short(s: &str) -> String {
    dc_rez::dc_rom::sha256_hex(s.to_ascii_uppercase().as_bytes())[..12].to_string()
}

/// The room line's `trig=` field as the lobby builds it from the world's GMS.
// SPEC: DSERVER_PIPE.md §3.2 (room line); OBSERVED: ROM table key GMS [Mission0] StartTrigger / Controller,
// [Object<n>] StartTrigger / EndTrigger
fn trig_field(rom: &std::path::Path, key: &str, world: &str, goal: u32) -> String {
    let vfs = RezVfs::new(rom);
    let Some(doc) = dc_dserver::coop_rom::find_gms(&vfs, key, world) else {
        return String::new();
    };
    let get = |s: &str, k: &str| {
        doc.get_str(s, k)
            .map(|v| v.trim().trim_matches('"').to_string())
            .unwrap_or_default()
    };
    let o = format!("Object{goal}");
    format!(
        " trig={}|{}|{}|{}",
        get("Mission0", "StartTrigger"),
        get(&o, "StartTrigger"),
        get(&o, "EndTrigger"),
        get("Mission0", "Controller")
    )
}

#[derive(Debug, Default)]
struct Metrics {
    /// (duration ms, place) of each stuck episode.
    episodes: Vec<(u32, [f32; 3])>,
    /// Spawn-to-contact times (ms) and the AI lives that never made contact.
    contacts: Vec<u32>,
    never: u32,
    lives: u32,
    overlaps: u32,
    island_samples: u32,
    /// The most samples in a row one AI stood inside the level geometry (a face passed from its back counts).
    inside_run: u32,
    stages: u32,
    nav: Option<dc_dserver::game::coop::NavStats>,
    wall_ms: u128,
}

#[derive(Clone, Copy)]
struct Track {
    alive: bool,
    inside: u32,
    born: u32,
    contact: bool,
    anchor: [f32; 3],
    anchor_at: u32,
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Runs one world for `secs` with `humans` standing at their start points.
fn run(
    rom: &std::path::Path,
    key: &str,
    w: &WorldInfo,
    nav: bool,
    humans: usize,
    secs: u32,
) -> Metrics {
    let (tables, _) = RomTables::load(rom);
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    cfg.game.fall.enabled = false;
    cfg.game.coop.hit_chance = [0.0; 3];
    cfg.game.coop.nav = nav;
    let hold = cfg.game.coop.hold_range;
    let trig = if w.coop.mission {
        trig_field(rom, key, &w.announce, 0)
    } else {
        String::new()
    };
    let mut server = Server::with_tables(cfg, w.clone(), tables, 0);
    assert!(server.handle_line(
        0,
        &format!("room key={key} type=5 goal=0 ff=0 time=1260{trig}")
    ));
    let names = ["Dave", "Erin", "Kim", "Lou"];
    for (i, n) in names.iter().take(humans).enumerate() {
        assert!(server.handle_line(0, &format!("user {} {} 99 0 120 {n}", 7 + i, 3 + i)));
    }
    let mut peers: Vec<Peer> = names
        .iter()
        .take(humans)
        .enumerate()
        .map(|(i, n)| {
            let mut c = SynthConfig::named(n);
            c.box_bounds = Some((w.extent_min, w.extent_max));
            c.walk_step = 0.0;
            c.mode_class_hint = Some(5);
            Peer {
                addr: 1 + i as u32,
                client: SynthClient::new(c),
                up: SimChannel::new(3 + 2 * i as u64, Faults::clean(2)),
                down: SimChannel::new(4 + 2 * i as u64, Faults::clean(2)),
            }
        })
        .collect();
    let trace = w.trace.clone().expect("level model");
    // a mission world: the humans go from box to box of the trigger network (the boxes a player walks into, nearest
    // to the start first), every `stage_ms`, standing on the floor below the box's centre, so its statements spawn
    // the AI area by area
    let mission = server.game().coop().is_some_and(|c| c.mission);
    let boxes: Vec<[f32; 3]> = {
        let start = w.spawns.first().map_or([0.0; 3], |s| s.pos);
        let mut b: Vec<[f32; 3]> = w
            .triggers
            .iter()
            .filter(|t| t.touch && t.dims.iter().any(|d| *d > 16.5))
            .map(|t| t.pos)
            .collect();
        let d = |p: &[f32; 3]| (p[0] - start[0]).hypot(p[2] - start[2]);
        b.sort_by(|x, y| d(x).total_cmp(&d(y)));
        b.into_iter()
            .filter_map(|p| {
                let h = trace.ray(
                    V3::from_array(p),
                    V3::from_array([p[0], p[1] - 2000.0, p[2]]),
                )?;
                Some([p[0], p[1] - 2000.0 * h.t + 54.0, p[2]])
            })
            .collect()
    };
    let stage_ms: u32 = std::env::var("NAV_STAGE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30_000);
    let mut stage = 0usize;
    let t0 = std::time::Instant::now();
    let mut now = 0u32;
    let mut m = Metrics::default();
    let mut tracks: Vec<Track> = Vec::new();
    let end = secs * 1000;
    while now < end {
        if mission && now >= 20_000 + stage as u32 * stage_ms && stage < boxes.len() {
            let at = boxes[stage];
            for (i, p) in peers.iter_mut().enumerate() {
                let spot = [at[0] + 40.0 * i as f32, at[1], at[2]];
                p.client.set_height(spot[1]);
                p.client.walk_to(Some(spot), 100_000.0);
            }
            m.stages += 1;
            stage += 1;
        }
        step(&mut server, &mut peers, now);
        now += 10;
        if !now.is_multiple_of(250) {
            continue;
        }
        let g = server.game();
        let humans: Vec<[f32; 3]> = g
            .players()
            .values()
            .filter(|p| p.life == dc_dserver::game::Life::Alive)
            .map(|p| p.pos)
            .collect();
        let Some(c) = g.coop() else {
            continue;
        };
        let every: u32 = std::env::var("NAV_TRACE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if every > 0 && now.is_multiple_of(every) {
            println!(
                "t {now}: humans {:?}",
                humans.iter().map(|h| h.map(f32::round)).collect::<Vec<_>>()
            );
            for a in c.ais.iter().filter(|a| a.state == AiState::Alive) {
                println!(
                    "   ai {} at {:?} motion {:?}",
                    a.slot,
                    a.pos.map(f32::round),
                    a.motion
                );
            }
        }
        for a in &c.ais {
            if tracks.len() <= a.slot {
                tracks.resize(
                    a.slot + 1,
                    Track {
                        alive: false,
                        inside: 0,
                        born: 0,
                        contact: false,
                        anchor: [0.0; 3],
                        anchor_at: 0,
                    },
                );
            }
            let t = &mut tracks[a.slot];
            let alive = a.state == AiState::Alive;
            if alive && !t.alive {
                *t = Track {
                    alive: true,
                    inside: 0,
                    born: now,
                    contact: false,
                    anchor: a.pos,
                    anchor_at: now,
                };
                m.lives += 1;
            }
            if !alive {
                if t.alive {
                    if now - t.anchor_at >= 8000 {
                        m.episodes.push((now - t.anchor_at, t.anchor));
                    }
                    if !t.contact {
                        m.never += 1;
                    }
                }
                t.alive = false;
                continue;
            }
            let dims = a.dims([24.0, 53.0, 24.0]);
            // an AI with no way to the human on this world (or no room to stand where the file keeps it) holds its
            // place (FT-S11): not stuck
            let island = g.coop_nav_island(a.slot);
            if island {
                m.island_samples += 1;
            }
            if !island
                && trace.overlaps(
                    &Cylinder::from_dims(V3::new(dims[0] - 1.0, dims[1] - 1.0, dims[2] - 1.0)),
                    V3::from_array(a.pos),
                )
            {
                m.overlaps += 1;
                t.inside += 1;
                m.inside_run = m.inside_run.max(t.inside);
            } else {
                t.inside = 0;
            }
            let Some(h) = humans
                .iter()
                .copied()
                .min_by(|x, y| dist(*x, a.pos).total_cmp(&dist(*y, a.pos)))
            else {
                continue;
            };
            let d = dist(h, a.pos);
            let eye = [a.pos[0], a.pos[1] + dims[1] * 0.8, a.pos[2]];
            let heye = [h[0], h[1] + 42.0, h[2]];
            let los = trace
                .ray(V3::from_array(eye), V3::from_array(heye))
                .is_none();
            if los && d <= hold + 1.0 && !t.contact {
                t.contact = true;
                m.contacts.push(now - t.born);
            }
            let wants = d > 96.0 && !(los && d <= hold + 1.0) && !island;
            if !wants || dist(a.pos, t.anchor) > 48.0 {
                if now - t.anchor_at >= 8000 {
                    m.episodes.push((now - t.anchor_at, t.anchor));
                }
                t.anchor = a.pos;
                t.anchor_at = now;
            }
        }
    }
    for t in tracks.iter().filter(|t| t.alive) {
        if now - t.anchor_at >= 8000 {
            m.episodes.push((now - t.anchor_at, t.anchor));
        }
        if !t.contact {
            m.never += 1;
        }
    }
    m.nav = server.game().coop_nav_stats();
    m.wall_ms = t0.elapsed().as_millis();
    m
}

fn report(label: &str, m: &Metrics) {
    let mut c = m.contacts.clone();
    c.sort();
    let med = c.get(c.len() / 2).copied();
    println!(
        "  {label}: {} boxes walked, {} AI lives, {} stuck episodes (8 s+), {} lives without contact, contact median {:?} ms max {:?} ms, {} samples inside geometry (at most {} in a row), {} samples holding on an island, {} ms wall",
        m.stages,
        m.lives,
        m.episodes.len(),
        m.never,
        med,
        c.last(),
        m.overlaps,
        m.inside_run,
        m.island_samples,
        m.wall_ms
    );
    for (ms, at) in &m.episodes {
        println!("    stuck {ms} ms at {:?}", at.map(f32::round));
    }
    if let Some(n) = &m.nav {
        println!(
            "    nav: {} plans ({} whole, {} none, {} on an island), {} jumps, {} stuck re-plans, {} blocked bans, {} transits, {} freed, grid {} nodes / {} probes; {} steps, {} us in all, {:.0} us per step, worst step {} us (one plan {} us, one growth {} us, one stride {} us)",
            n.plans,
            n.whole,
            n.failed,
            n.islands,
            n.jumps,
            n.stuck_replans,
            n.blocked_bans,
            n.transits,
            n.freed,
            n.nodes,
            n.probes,
            n.steps,
            n.busy_us,
            n.busy_us as f64 / n.steps.max(1) as f64,
            n.step_max_us,
            n.plan_max_us,
            n.grow_max_us,
            n.walk_max_us
        );
    }
}

/// FT-S11: over a few minutes on each co-op world no AI stands stuck for 8 s (it wants to move and stays within 48
/// units), and no AI stands inside the level geometry.
#[test]
fn rom_coop_ai_find_their_way_and_never_stand_stuck() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let vfs = RezVfs::new(&rom);
    let worlds = coop_worlds(&vfs);
    assert!(!worlds.is_empty(), "the ROM room table has co-op maps");
    let secs: u32 = std::env::var("NAV_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180);
    let humans: usize = std::env::var("NAV_HUMANS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let mut failures = Vec::new();
    for (key, arg) in worlds {
        let w = load_from_rom(&rom, &arg, NameForm::Listed, None)
            .unwrap_or_else(|e| panic!("co-op world {}: {e}", short(&arg)));
        println!(
            "co-op world {} ({}): {humans} humans, {secs} s",
            short(&arg),
            if w.coop.mission {
                "statements spawn AI"
            } else {
                "waves"
            }
        );
        if std::env::var("NAV_BEFORE").is_ok() {
            let b = run(&rom, &key, &w, false, humans, secs);
            report("before (coop.nav=off)", &b);
        }
        let a = run(&rom, &key, &w, true, humans, secs);
        report("after", &a);
        // a face passed from its back may hold an AI inside the geometry for a stride or two, never for a second
        if !a.episodes.is_empty() || a.inside_run >= 4 {
            failures.push(format!(
                "{}: {} stuck episodes, {} samples in a row inside geometry",
                short(&arg),
                a.episodes.len(),
                a.inside_run
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
