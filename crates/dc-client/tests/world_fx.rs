//! world-fx (CLI-234): a crate and an explosive drum shot on a dedicated server play their effects on the observing
//! client. The server runs in this process on a loopback socket (our `dc-dserver` state machine with a made-up world
//! to which a breakable crate, a breakable drum, their placed effects and the drum's explosion object are added); a
//! synthetic shooter breaks them by hit claims on their kind-2 objects (a form the 2009 client never sends: its
//! world-model hits carry no object, see tests/brush_hits.rs; the server keeps accepting the claim); the real client
//! session (`NetWorld`, headless frame loop) watches. Checked: each type-0x64 effect
//! object the server created was handed to the effect player exactly once (the removal of the objects after their
//! lifetime added nothing), and with a ROM the effect groups play, the drum's blast record takes the grenade path and
//! asks for the explosion sound cue.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (blast record), §8.4 (type 0x64); GAME_MESSAGES_2009_C2S.md §4.1 (`hit_obj`)
// DC-DEFAULT: CLI-234 (proposed), DSV-115..DSV-120 (proposed, the server side)

use dc_net_engine::socket_reader::SocketReader;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ServerHold,
    ViewAngles, WorldFile,
};
use dc_dserver::destructibles::{BreakableInfo, DeathAction, ExplosionInfo};
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{BrushClass, BrushInfo, EffectInfo, SpawnPoint};
use dc_sim::Commands;

fn world_file() -> WorldFile {
    let s = synth_source();
    WorldFile {
        bytes: s.bytes,
        stand_dims: SYNTH_DIMS,
        textures: s.textures,
    }
}

/// Stands still and tells the shooter's thread when the client is alive in the world.
struct Observer {
    alive: Arc<AtomicBool>,
}

impl InputSource for Observer {
    fn frame(&mut self, _view: &mut ViewAngles) {}
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands::default()
    }
    fn server(&mut self, v: &ServerHold) {
        if v.life_state == Some(1) {
            self.alive.store(true, Ordering::SeqCst);
        }
    }
    fn label(&self) -> &'static str {
        "observer"
    }
}

/// What the server thread tells the test.
#[derive(Debug, Default)]
struct ShotScript {
    crate_dead: bool,
    drum_dead: bool,
    lines: Vec<String>,
}

fn brush(name: &[u8], at: [f32; 3]) -> BrushInfo {
    BrushInfo {
        name: name.to_vec(),
        role: Some(dc_world::ObjectRole::Breakable),
        pos: at,
        rot: [0.0; 3],
        visible: true,
        solid: true,
        rayhit: true,
        blend: 0,
        fog_disable: false,
        class: BrushClass::Breakable,
        sky: None,
    }
}

fn effect(name: &[u8], fx: &str, at: [f32; 3]) -> EffectInfo {
    EffectInfo {
        name: name.to_vec(),
        pos: at,
        rot: [0.0; 3],
        fx: fx.as_bytes().to_vec(),
        looped: false,
        start_on: false,
    }
}

/// Effect group names that exist in the user's ROM, taken at run time (two impact rows' groups); made-up names
/// without one.
fn group_names(rom: Option<&std::path::Path>) -> (String, String) {
    let made = (
        "MadeUpCrateBurst".to_string(),
        "MadeUpDrumBurst".to_string(),
    );
    let Some(rom) = rom else {
        return made;
    };
    let t = dc_client::fx::FxTables::open(rom);
    let lib = dc_client::fx::EffectLibrary::open(rom);
    let mut names: Vec<String> = t
        .impacts
        .values()
        .filter_map(|r| r.fx.clone())
        .filter(|n| lib.get(n).is_some())
        .collect();
    names.sort();
    names.dedup();
    match (names.first(), names.last()) {
        (Some(a), Some(b)) => (a.clone(), b.clone()),
        _ => made,
    }
}

#[test]
fn shot_crate_and_drum_play_their_world_effects() {
    let rom = dc_rom::test_rom();
    // the server: the made-up world file, a crate and a drum next to the spawn
    let mut info = dc_dserver::world::world_from_bytes(
        &dc_client::testkit::synth_world_bytes(),
        "Worlds\\Synth.dat",
        "Synth",
    )
    .expect("server world");
    // the made-up world file has no spawn points: two at the origin of its floor
    let base = [0.0f32, 0.0, 0.0];
    info.spawns = vec![
        SpawnPoint {
            pos: base,
            rot: [0.0; 3],
            team_filter: None,
        },
        SpawnPoint {
            pos: [base[0] + 90.0, base[1], base[2]],
            rot: [0.0; 3],
            team_filter: None,
        },
    ];
    let crate_at = [base[0] + 45.0, base[1] + 20.0, base[2] + 70.0];
    let drum_at = [base[0] + 45.0, base[1] + 20.0, base[2] - 70.0];
    let (crate_fx, drum_fx) = group_names(rom.as_deref());
    info.brushes.push(brush(b"Crate1", crate_at));
    info.brushes.push(brush(b"Drum1", drum_at));
    info.effects.push(effect(b"Crate1Fx", &crate_fx, crate_at));
    info.effects.push(effect(b"Drum1Fx", &drum_fx, drum_at));
    // the drum's explosion names the grenade ammunition's impact effect with a ROM, nothing without
    let tables = match rom.as_deref() {
        Some(r) => RomTables::load(r).0,
        None => RomTables::default(),
    };
    let blast_fx = tables
        .weapons
        .weapons
        .get(&7)
        .and_then(|w| w.ammo)
        .and_then(|a| tables.weapons.ammo.get(&a))
        .and_then(|a| a.impact_fx.clone())
        .map(String::into_bytes);
    info.destructibles.explosions.push(ExplosionInfo {
        name: b"Drum1Exp".to_vec(),
        pos: [drum_at[0], drum_at[1] + 20.0, drum_at[2]],
        damage: 10.0,
        radius: 200.0,
        impact_fx: blast_fx.clone(),
    });
    info.destructibles.breakables.push(BreakableInfo {
        name: b"crate1".to_vec(),
        hit_points: 60.0,
        death: vec![DeathAction::Effect(b"crate1fx".to_vec())],
        skipped: 0,
    });
    info.destructibles.breakables.push(BreakableInfo {
        name: b"drum1".to_vec(),
        hit_points: 60.0,
        death: vec![
            DeathAction::Effect(b"drum1fx".to_vec()),
            DeathAction::Explode(0),
        ],
        skipped: 0,
    });

    let args = dc_dserver::cli::parse(
        [
            "-worldfile",
            "unused.dat",
            "-tick",
            "20",
            "-roundsecs",
            "120",
            "-destruct",
            "effect_ms=2500",
        ]
        .map(String::from),
    )
    .expect("server arguments");
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();

    let port = sock.local_addr().unwrap().port();
    let alive = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let server_thread = {
        let (alive, stop) = (alive.clone(), stop.clone());
        let extent = (info.extent_min, info.extent_max);
        std::thread::spawn(move || {
            let start = Instant::now();
            let now = || start.elapsed().as_millis() as u32;
            let mut server: Server<SocketAddr> =
                Server::with_tables(args.server.clone(), info, tables, now());
            server.set_endpoint("127.0.0.1", port);
            assert!(server.handle_line(0, "user 1 1 1 0 0 Observer"));
            assert!(server.handle_line(0, "user 2 2 2 1 0 Shooter"));
            let shooter_sock = UdpSocket::bind("127.0.0.1:0").unwrap();

            let mut cfg = SynthConfig::named("Shooter");
            cfg.box_bounds = Some(extent);
            cfg.mode_class_hint = Some(1);
            cfg.load_ms = 300;
            let mut shooter = SynthClient::new(cfg);
            let server_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
            let mut log = ShotScript::default();
            let rd = SocketReader::start(&sock, 4096).unwrap();
            let idle_rd = SocketReader::start(&shooter_sock, 4096).unwrap();
            let mut ready_at: Option<u32> = None;
            let mut last_fire = 0u32;
            while !stop.load(Ordering::SeqCst) {
                let mut got: Vec<(SocketAddr, Vec<u8>)> = Vec::new();
                while let Some(Ok((d, from))) = rd.next_within(Duration::from_millis(4)) {
                    got.push((from, d));
                }
                let mut shooter_got: Vec<Vec<u8>> = Vec::new();
                while let Some(Ok((d, _))) = idle_rd.next_within(Duration::from_millis(1)) {
                    shooter_got.push(d);
                }
                // one clock reading per round, after the receives (the server's time never goes back)
                let t = now();
                for (from, d) in got {
                    server.handle_datagram(t, from, &d);
                }
                for d in shooter_got {
                    shooter.handle_datagram(t, &d);
                }
                for d in shooter.poll(t) {
                    let _ = shooter_sock.send_to(&d, server_addr);
                }
                for o in server.poll(t) {
                    match o {
                        Output::Transmit { to, bytes } => {
                            let _ = sock.send_to(&bytes, to);
                        }
                        Output::Pipe(_) => {}
                        Output::Log(l) => log.lines.push(l),
                    }
                }
                // the shot script: the client has been alive for a second, the shooter too
                if alive.load(Ordering::SeqCst) && shooter.life_state() == 1 {
                    let r = *ready_at.get_or_insert(t + 1000);
                    if t >= r && t >= last_fire + 60 {
                        last_fire = t;
                        let items: Vec<_> = server.game().breakables().to_vec();
                        log.crate_dead = items[0].dead;
                        log.drum_dead = items[1].dead;
                        if !items[0].dead {
                            shooter.fire_at_point(items[0].obj, items[0].pos, true);
                        } else if !items[1].dead {
                            shooter.fire_at_point(items[1].obj, items[1].pos, true);
                        }
                    }
                }
            }
            log
        })
    };
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "Observer",
    );
    o.world_file = Some(world_file());
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 14,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 15,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    let mut input = Observer { alive };
    let run = run_headless(&mut net, Some(&mut input as &mut dyn InputSource), &opts);
    let st = handle.shutdown(Duration::from_secs(5));
    stop.store(true, Ordering::SeqCst);
    let log = server_thread.join().expect("server thread");
    let r = run.unwrap_or_else(|e| {
        for l in &log.lines {
            println!("    {l}");
        }
        panic!("headless run: {e:?}");
    });
    println!("{}", r.summary());
    println!("fx {:?}, world effects {}", r.fx, r.world_effects);
    for l in log.lines.iter().filter(|l| l.contains("destroy")) {
        println!("    {l}");
    }
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert!(log.crate_dead && log.drum_dead, "both broke: {log:?}");
    // once per object: two effect objects were created and later removed
    assert_eq!(r.world_effects, 2, "{:?}", r.fx);
    assert_eq!(r.fx.world, 2, "{:?}", r.fx);
    if let Some(rom) = rom {
        let _ = rom;
        assert!(r.fx.groups_started >= 2, "the groups played: {:?}", r.fx);
        if blast_fx.is_some() {
            assert!(r.fx.blasts >= 1, "the drum's blast record: {:?}", r.fx);
            assert!(r.fx.sounds >= 1, "the explosion sound cue: {:?}", r.fx);
        }
    } else {
        // no ROM: no effect library; the objects were still handed over
        assert!(r.fx.groups_missing >= 2, "{:?}", r.fx);
    }
}
