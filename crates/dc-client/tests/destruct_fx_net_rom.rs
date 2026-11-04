//! ROM- and GPU-gated loopback pictures of client-destructfx (skipped without CA_ROM or a GPU adapter): on JUNK_FLEA
//! the headless client stands where it sees a crate (then, in the second test, an explosive drum), aims at it and
//! holds the fire button; its own pellets stop at the model (no hit object on the wire), our dedicated server (in this
//! process, `-destruct` on) traces them against its breakables, breaks the model and sends the flag change, the
//! placed-effect object (and the drum's blast record); the client plays the breaking group: anchors, the linked model
//! pieces, the sound key. The client's own frames go to `reports/client-destructfx/`
//! (ROM-derived, never committed): before the first shot and a sequence after the break. Names are never printed.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1.1; GAME_MESSAGES_2009_S2C.md §8.2, §8.4
// DISASM: cshell+0x26e070 (the placed effect takes its object's position and rotation)
// EVIDENCE: clientfx+0x1cae0 / +0x1cd50 (anchors), clientfx+0x17290 (model keys), clientfx+0x20ec0 (sound keys)
// DC-DEFAULT: CLI-250, DSV-122, CLI-360..CLI-362 (proposed)

use dc_net_engine::socket_reader::SocketReader;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dc_client::arms::WeaponCommands;
use dc_client::testkit::synth_font;
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, RemoteBody,
    RomWorld, ServerHold, ViewAngles, WorldSource,
};
use dc_dserver::destructibles::DeathAction;
use dc_dserver::session::{Output, Server};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::SpawnPoint;
use dc_render::{AdapterChoice, GpuContext};
use dc_sim::{Commands, V3};

const MAP: &str = "JUNK_FLEA";
const W: u32 = 800;
const H: u32 = 600;
/// Height of the eye above the floor point the spot search uses (the body's eye is about there).
const EYE_UP: f32 = 50.0;

fn out_dir() -> std::path::PathBuf {
    let o = dc_render::default_out_dir().join("client-destructfx");
    let _ = std::fs::create_dir_all(&o);
    o
}

/// A floor point with a clear line from the eye above it to `target` (the first polygon on that line lies within
/// `reach` of the target) and room around the body.
fn spot_for(cw: &dc_sim::CollisionWorld, target: V3, reach: f32) -> Option<V3> {
    for dist in [220.0f32, 280.0, 180.0, 340.0, 400.0] {
        for k in 0..24 {
            let a = k as f32 * std::f32::consts::TAU / 24.0;
            let (x, z) = (target.x + dist * a.sin(), target.z + dist * a.cos());
            let Some(floor) = cw.ray(
                V3::new(x, target.y + 80.0, z),
                V3::new(x, target.y - 300.0, z),
            ) else {
                continue;
            };
            let from = V3::new(x, target.y + 80.0, z);
            let ground = from + (V3::new(x, target.y - 300.0, z) - from) * floor.t;
            if floor.normal.y < 0.8 {
                continue;
            }
            let base = ground + V3::new(0.0, 2.0, 0.0);
            let roomy = cw.ray(base, base + V3::new(0.0, 100.0, 0.0)).is_none()
                && [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)]
                    .iter()
                    .all(|&(dx, dz)| {
                        let c = base + V3::new(0.0, 30.0, 0.0);
                        cw.ray(c, c + V3::new(dx * 40.0, 0.0, dz * 40.0)).is_none()
                    });
            if !roomy {
                continue;
            }
            // clear from a band of eye heights (the body's eye height is not modelled here)
            let clear = [EYE_UP - 25.0, EYE_UP, EYE_UP + 25.0].iter().all(|&up| {
                let eye = ground + V3::new(0.0, up, 0.0);
                match cw.ray(eye, target) {
                    None => true,
                    Some(h) => (1.0 - h.t) * (target - eye).len() < reach,
                }
            });
            if clear {
                return Some(ground);
            }
        }
    }
    None
}

/// Aims at the breakable and holds the fire button until the server broke it; counts frames and notes the frame of the
/// first shot and the frame the break was seen.
struct Shooter {
    target: [f32; 3],
    dead: Arc<AtomicBool>,
    frame: u32,
    live: bool,
    fired_at: Arc<AtomicU32>,
    broke_at: Arc<AtomicU32>,
}

impl InputSource for Shooter {
    fn frame(&mut self, _view: &mut ViewAngles) {
        self.frame += 1;
        if self.dead.load(Ordering::SeqCst) && self.broke_at.load(Ordering::SeqCst) == 0 {
            self.broke_at.store(self.frame, Ordering::SeqCst);
        }
    }
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands::default()
    }
    fn aim(
        &mut self,
        eye: [f32; 3],
        _remotes: &[RemoteBody],
        _own_team: Option<u8>,
        view: &mut ViewAngles,
    ) {
        let (yaw, pitch) = dc_botclient::shooter::aim_angles(eye, self.target);
        view.yaw = yaw;
        view.pitch = pitch;
    }
    fn weapon(&mut self) -> WeaponCommands {
        // a second of looking first (the "before" picture), then fire until the model is gone
        let fire = self.live && self.frame > 90 && !self.dead.load(Ordering::SeqCst);
        if fire && self.fired_at.load(Ordering::SeqCst) == 0 {
            self.fired_at.store(self.frame, Ordering::SeqCst);
        }
        WeaponCommands {
            fire,
            ..WeaponCommands::default()
        }
    }
    fn server(&mut self, v: &ServerHold) {
        self.live = v.life_state == Some(1);
    }
    fn label(&self) -> &'static str {
        "breakable shooter"
    }
}

fn shoot_and_watch(explosive: bool, tag: &str) {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    if let Err(e) = GpuContext::headless(AdapterChoice::Auto) {
        println!("skipped: no GPU adapter ({e})");
        return;
    }
    let Ok(local) = RomWorld::new(&rom, MAP).load(None) else {
        println!("skipped: {MAP} not in this ROM");
        return;
    };
    let mut info = dc_dserver::world::load_from_rom(
        &rom,
        &format!("Worlds\\{MAP}"),
        dc_dserver::world::NameForm::Listed,
        None,
    )
    .expect("server world");
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    // the breakables of the kind (a drum's death names an explosion object, a crate's does not) that name a placed
    // effect; the first one the server creates and the client can see from a floor point
    let targets: Vec<Vec<u8>> = info
        .destructibles
        .breakables
        .iter()
        .filter(|b| {
            b.death.iter().any(|d| matches!(d, DeathAction::Effect(_)))
                && b.death.iter().any(|d| matches!(d, DeathAction::Explode(_))) == explosive
        })
        .map(|b| b.name.clone())
        .collect();
    assert!(
        !targets.is_empty(),
        "breakables of the kind in the world file"
    );
    assert!(
        targets
            .iter()
            .all(|n| info.destructibles.shape(n).is_some()),
        "every one has its model shape for the server's trace"
    );
    // aim at the middle of the model (its bounds), which the server created
    let found = targets.iter().find_map(|n| {
        info.brushes
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case(n))?;
        let (lo, hi) = info.destructibles.shape(n)?.models.first()?.bounds();
        let at = (lo + hi) * 0.5;
        spot_for(&local.collision, at, 0.5 * (hi - lo).len()).map(|s| (n.clone(), at.to_array(), s))
    });
    let Some((target, target_at, spot)) = found else {
        panic!("no floor point sees one");
    };
    // the client's team spawns at the spot, the idle player's at the world spawn farthest from it
    let names = tables.teams.names.clone();
    let far = info
        .spawns
        .iter()
        .max_by(|a, b| {
            let d = |p: &SpawnPoint| (V3::from_array(p.pos) - spot).len();
            d(a).total_cmp(&d(b))
        })
        .map(|s| s.pos)
        .expect("a world spawn");
    info.spawns = vec![
        SpawnPoint {
            pos: spot.to_array(),
            rot: [0.0; 3],
            team_filter: names[0].clone(),
        },
        SpawnPoint {
            pos: far,
            rot: [0.0; 3],
            team_filter: names[1].clone(),
        },
    ];
    let args = dc_dserver::cli::parse(
        [
            "-worldfile",
            "unused.dat",
            "-tick",
            "20",
            "-roundsecs",
            "120",
            "-loadout",
            "145",
            "-destruct",
            "enabled=on,effect_ms=4000",
            "-combat",
            "cone_check=log",
        ]
        .map(String::from),
    )
    .expect("server arguments");
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();

    let port = sock.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let dead = Arc::new(AtomicBool::new(false));
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let server_thread = {
        let (stop, dead, lines) = (stop.clone(), dead.clone(), lines.clone());
        let extent = (info.extent_min, info.extent_max);
        let target = target.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let now = || start.elapsed().as_millis() as u32;
            let mut server: Server<SocketAddr> =
                Server::with_tables(args.server.clone(), info, tables, now());
            server.set_endpoint("127.0.0.1", port);
            assert!(server.handle_line(0, "user 1 1 1 0 0 Shooter"));
            assert!(server.handle_line(0, "user 2 2 2 1 0 Idle"));
            let idle_sock = UdpSocket::bind("127.0.0.1:0").unwrap();

            let mut cfg = SynthConfig::named("Idle");
            cfg.box_bounds = Some(extent);
            cfg.mode_class_hint = Some(1);
            cfg.load_ms = 300;
            let mut idle = SynthClient::new(cfg);
            let server_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
            let rd = SocketReader::start(&sock, 4096).unwrap();
            let idle_rd = SocketReader::start(&idle_sock, 4096).unwrap();
            while !stop.load(Ordering::SeqCst) {
                let mut got: Vec<(SocketAddr, Vec<u8>)> = Vec::new();
                while let Some(Ok((d, from))) = rd.next_within(Duration::from_millis(4)) {
                    got.push((from, d));
                }
                let mut idle_got: Vec<Vec<u8>> = Vec::new();
                while let Some(Ok((d, _))) = idle_rd.next_within(Duration::from_millis(1)) {
                    idle_got.push(d);
                }
                let t = now();
                for (from, d) in got {
                    server.handle_datagram(t, from, &d);
                }
                for d in idle_got {
                    idle.handle_datagram(t, &d);
                }
                for d in idle.poll(t) {
                    let _ = idle_sock.send_to(&d, server_addr);
                }
                for o in server.poll(t) {
                    match o {
                        Output::Transmit { to, bytes } => {
                            let _ = sock.send_to(&bytes, to);
                        }
                        Output::Pipe(_) => {}
                        Output::Log(l) => lines.lock().unwrap().push(l),
                    }
                }
                if server
                    .game()
                    .breakables()
                    .iter()
                    .any(|b| b.dead && b.name.eq_ignore_ascii_case(&target))
                {
                    dead.store(true, Ordering::SeqCst);
                }
            }
        })
    };
    let out = out_dir();
    let frames_dir = out.join(format!("frames_{tag}"));
    let _ = std::fs::remove_dir_all(&frames_dir);
    std::fs::create_dir_all(&frames_dir).unwrap();
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "Shooter",
    );
    o.rom = Some(rom.clone());
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 14,
        frame_us: 16_667,
        width: W,
        height: H,
        out: Some(frames_dir.clone()),
        prefix: "f".into(),
        save_every: 3,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    let fired_at = Arc::new(AtomicU32::new(0));
    let broke_at = Arc::new(AtomicU32::new(0));
    let mut input = Shooter {
        target: target_at,
        dead: dead.clone(),
        frame: 0,
        live: false,
        fired_at: fired_at.clone(),
        broke_at: broke_at.clone(),
    };
    let run = run_headless(&mut net, Some(&mut input as &mut dyn InputSource), &opts);
    let st = handle.shutdown(Duration::from_secs(5));
    stop.store(true, Ordering::SeqCst);
    server_thread.join().expect("server thread");
    let lines = lines.lock().unwrap().clone();
    let r = run.unwrap_or_else(|e| {
        for l in &lines {
            println!("    {l}");
        }
        panic!("headless run: {e:?}");
    });
    println!("{}", r.summary());
    println!("arms {:?}; fx {:?}", st.arms, r.fx);
    // the log names the models: only counts go to the test output
    println!(
        "server: {} breakable(s) destroyed, {} explosion(s)",
        lines.iter().filter(|l| l.contains("destroyed by")).count(),
        lines
            .iter()
            .filter(|l| l.starts_with("dc-dserver: explosion"))
            .count()
    );
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    if !dead.load(Ordering::SeqCst) {
        // the server's last lines, for the failure only
        for l in lines.iter().rev().take(25).rev() {
            println!("    {l}");
        }
    }
    assert!(dead.load(Ordering::SeqCst), "the server broke it");
    assert_eq!(st.arms.hits, 0, "no 0x82 named a hit object: {:?}", st.arms);
    assert!(
        r.fx.model_impacts >= 1,
        "own impacts on the model: {:?}",
        r.fx
    );
    assert!(r.world_effects >= 1, "the death effect: {:?}", r.fx);
    if explosive {
        assert!(r.fx.blasts >= 1, "the drum's blast record: {:?}", r.fx);
        // the 2009 rise (CLI-600): 121..175 units
        assert!(
            (100..=190).contains(&r.fx.anchor_rise),
            "the drum flew up: {:?}",
            r.fx
        );
    } else {
        assert!(r.fx.anchors >= 5, "the crate's five anchors: {:?}", r.fx);
        assert!(r.fx.fx_models_max >= 5, "its five pieces: {:?}", r.fx);
        assert!(r.fx.sound_keys >= 1, "its breaking sound: {:?}", r.fx);
    }
    assert!(
        r.fx_models_drawn > 0,
        "effect models drawn: {}",
        r.fx_models_drawn
    );
    // keep the picture before the first shot and three after the break; the frame dumps go
    let pick = |want: u32| -> Option<std::path::PathBuf> {
        let mut best: Option<(u32, std::path::PathBuf)> = None;
        for e in std::fs::read_dir(&frames_dir).ok()?.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let Some(f) = n
                .strip_prefix('f')
                .and_then(|s| s.strip_suffix(".png"))
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let d = f.abs_diff(want);
            if best.as_ref().is_none_or(|b| d < b.0) {
                best = Some((d, e.path()));
            }
        }
        best.map(|b| b.1)
    };
    let fired = fired_at.load(Ordering::SeqCst);
    let broke = broke_at.load(Ordering::SeqCst);
    println!("first shot at frame {fired}, break seen at frame {broke}");
    let mut kept = 0;
    let mut shots: Vec<(String, u32)> =
        vec![(format!("net_{tag}_before"), fired.saturating_sub(3))];
    // frame offsets after the break was seen (60 frames a second of the run; under load the session clock and the
    // frame count drift apart, so the names carry frames, not milliseconds)
    for n in [0u32, 6, 15, 30, 54, 84, 114] {
        shots.push((format!("net_{tag}_after_{n:03}f"), broke + n));
    }
    for (name, f) in &shots {
        if let Some(p) = pick(*f) {
            std::fs::copy(&p, out.join(format!("{name}.png"))).unwrap();
            kept += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&frames_dir);
    assert_eq!(kept, shots.len(), "the pictures");
    println!("PNGs in {} (ROM-derived, never commit)", out.display());
}

#[test]
fn rom_the_client_shoots_a_crate_and_watches_it_break_apart() {
    shoot_and_watch(false, "crate");
}

#[test]
fn rom_the_client_shoots_a_drum_and_watches_it_thrown_up() {
    shoot_and_watch(true, "drum");
}
