//! client-brushhits (CLI-250, DSV-122): the client shoots a crate and a drum itself and the server breaks them. A
//! dedicated server runs in this process on a loopback socket (our `dc-dserver` state machine with `-destruct` on) on
//! a made-up world file that holds a floor, a crate model and a drum model (each linked to an object of a made-up
//! class, made breakable on the server); the real client session (`NetWorld`, headless frame loop) aims at the crate,
//! holds the fire button until the server switched the crate off, then does the same with the drum. An idle second
//! player only keeps the round running (it stands far to the side). No synthetic shooter.
//!
//! Checked, the 2009 rule: the client's pellets stop at the models (its trace meets the world models the server
//! created ray-hit / solid) and every 0x82 goes out without a hit object (a world-model hit names none,
//! cshell+0x238eb0); the server traces those pellets against its breakables and breaks both; the client draws its
//! own impacts on the models with the models' surface and plays the two death effects (and with a ROM the drum's
//! blast). Without a hit object the old path (`hit_obj` on the brush) is never used.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1, §4.1.1 (world hits carry has_hit 0); GAME_MESSAGES_2009_S2C.md §8.2, §8.4
// EVIDENCE: cshell+0x23a910, cshell+0x238eb0; DISASM: engine+0x40920, engine+0x40650
// DC-DEFAULT: CLI-250 (proposed), DSV-122 (proposed)

use dc_net_engine::socket_reader::SocketReader;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dc_client::arms::WeaponCommands;
use dc_client::testkit::synth_font;
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, RemoteBody,
    ServerHold, ViewAngles, WorldFile,
};
use dc_dserver::destructibles::{BreakableInfo, DeathAction, ExplosionInfo};
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{BrushClass, BrushInfo, EffectInfo, SpawnPoint, WorldInfo};
use dc_sim::from_world::FILE_FIRST_CHILD_IS_NEGATIVE;
use dc_sim::{build_bsp, Commands, ConvexPoly, SURF_SOLID, V3};
use dc_world::synth::{SynthModel, SynthObject, SynthWorld};

const CRATE_AT: [f32; 3] = [60.0, 20.0, 260.0];
const DRUM_AT: [f32; 3] = [-60.0, 20.0, 260.0];
const HALF: f32 = 20.0;
/// Made-up surface ids of the floor and of the two models' polygons (the impact surface the client must report).
const FLOOR_SURFACE: u16 = 3;
const CRATE_SURFACE: u16 = 41;
const DRUM_SURFACE: u16 = 42;

fn quad(c: [V3; 4]) -> ConvexPoly {
    ConvexPoly::from_corners(c.to_vec(), SURF_SOLID)
}

/// The six outward faces of a box of half-size `h` around `c`.
fn box_faces(c: [f32; 3], h: f32) -> Vec<ConvexPoly> {
    let v = |x: f32, y: f32, z: f32| V3::new(c[0] + x * h, c[1] + y * h, c[2] + z * h);
    vec![
        quad([
            v(1., -1., -1.),
            v(1., 1., -1.),
            v(1., 1., 1.),
            v(1., -1., 1.),
        ]),
        quad([
            v(-1., -1., 1.),
            v(-1., 1., 1.),
            v(-1., 1., -1.),
            v(-1., -1., -1.),
        ]),
        quad([
            v(-1., 1., -1.),
            v(-1., 1., 1.),
            v(1., 1., 1.),
            v(1., 1., -1.),
        ]),
        quad([
            v(-1., -1., 1.),
            v(-1., -1., -1.),
            v(1., -1., -1.),
            v(1., -1., 1.),
        ]),
        quad([
            v(-1., -1., 1.),
            v(1., -1., 1.),
            v(1., 1., 1.),
            v(-1., 1., 1.),
        ]),
        quad([
            v(1., -1., -1.),
            v(-1., -1., -1.),
            v(-1., 1., -1.),
            v(1., 1., -1.),
        ]),
    ]
}

fn model(
    name: &[u8],
    polys: &[ConvexPoly],
    lo: [f32; 3],
    hi: [f32; 3],
    surface: u16,
) -> SynthModel {
    let (nodes, root) = build_bsp(polys);
    let mut m = SynthModel {
        flags: 2,
        name: name.to_vec(),
        textures: vec![b"none".to_vec()],
        surfaces: vec![(SURF_SOLID, 0, surface)],
        box_min: lo,
        box_max: hi,
        origin: [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k])),
        ..SynthModel::default()
    };
    for (i, poly) in polys.iter().enumerate() {
        m.planes.push((poly.normal.to_array(), poly.dist));
        let first = m.points.len() as u32;
        m.points.extend(poly.verts.iter().map(|v| v.to_array()));
        m.polys.push((
            0,
            i as u32,
            (first..first + poly.verts.len() as u32).collect(),
        ));
    }
    let enc = |c: Option<u32>| c.map_or(-1, |i| i as i32);
    for (face, neg, pos) in nodes {
        let (a, b) = if FILE_FIRST_CHILD_IS_NEGATIVE {
            (neg, pos)
        } else {
            (pos, neg)
        };
        m.nodes.push((face, 0, enc(a), enc(b)));
    }
    m.root = enc(root);
    m
}

fn prop_model(name: &[u8], c: [f32; 3], surface: u16) -> SynthModel {
    model(
        name,
        &box_faces(c, HALF),
        [c[0] - HALF, c[1] - HALF, c[2] - HALF],
        [c[0] + HALF, c[1] + HALF, c[2] + HALF],
        surface,
    )
}

/// A floor (the level model) and the two box models, each named by an object of a made-up class.
fn world_bytes() -> Vec<u8> {
    let floor = quad([
        V3::new(-2000.0, 0.0, -2000.0),
        V3::new(-2000.0, 0.0, 2000.0),
        V3::new(2000.0, 0.0, 2000.0),
        V3::new(2000.0, 0.0, -2000.0),
    ]);
    let mut level = model(
        b"level",
        &[floor],
        [-2000.0, -1.0, -2000.0],
        [2000.0, 1.0, 2000.0],
        FLOOR_SURFACE,
    );
    level.flags = 0;
    let names: [&[u8]; 3] = [b"Nm", b"Pos", b"Rot"];
    SynthWorld {
        extent_min: [-2000.0, -100.0, -2000.0],
        extent_max: [2000.0, 600.0, 2000.0],
        tree_shape: vec![false],
        models: vec![
            level,
            prop_model(b"Crate1", CRATE_AT, CRATE_SURFACE),
            prop_model(b"Drum1", DRUM_AT, DRUM_SURFACE),
        ],
        objects: vec![
            SynthObject::point(b"SynthProp", names, b"Crate1", CRATE_AT, [0.0; 3]),
            SynthObject::point(b"SynthProp", names, b"Drum1", DRUM_AT, [0.0; 3]),
        ],
        grid_cell: [64.0; 3],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![0, 0, 0],
        ..SynthWorld::default()
    }
    .build()
}

fn breakable_brush(name: &[u8], at: [f32; 3]) -> BrushInfo {
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

/// The server's world: the file's floor and models, the two models made breakable (hit points, death effects, the
/// drum's explosion), their shapes for the server's pellet trace, two spawn points.
fn server_world(bytes: &[u8], tables: &RomTables, fx: (&str, &str)) -> WorldInfo {
    let mut info = dc_dserver::world::world_from_bytes(bytes, "Worlds\\Brush.dat", "Brush")
        .expect("server world");
    info.spawns = vec![
        SpawnPoint {
            pos: [0.0, 0.0, 0.0],
            rot: [0.0; 3],
            team_filter: None,
        },
        SpawnPoint {
            pos: [1500.0, 0.0, 0.0],
            rot: [0.0; 3],
            team_filter: None,
        },
    ];
    for (name, at) in [
        (b"Crate1".as_slice(), CRATE_AT),
        (b"Drum1".as_slice(), DRUM_AT),
    ] {
        info.brushes.retain(|b| !b.name.eq_ignore_ascii_case(name));
        info.brushes.push(breakable_brush(name, at));
    }
    info.effects.push(effect(b"Crate1Fx", fx.0, CRATE_AT));
    info.effects.push(effect(b"Drum1Fx", fx.1, DRUM_AT));
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
        pos: [DRUM_AT[0], DRUM_AT[1] + 20.0, DRUM_AT[2]],
        damage: 10.0,
        radius: 120.0,
        impact_fx: blast_fx,
    });
    // three pellets of the loadout's rifle each, with or without a ROM
    let hp = 2.5
        * tables
            .weapons
            .damage(145, 145)
            .unwrap_or(dc_dserver::game::GameTuning::default().default_damage);
    info.destructibles.breakables.push(BreakableInfo {
        name: b"crate1".to_vec(),
        hit_points: hp,
        death: vec![DeathAction::Effect(b"crate1fx".to_vec())],
        skipped: 0,
    });
    info.destructibles.breakables.push(BreakableInfo {
        name: b"drum1".to_vec(),
        hit_points: hp,
        death: vec![
            DeathAction::Effect(b"drum1fx".to_vec()),
            DeathAction::Explode(0),
        ],
        skipped: 0,
    });
    let w = dc_world::World::parse(bytes).expect("world file");
    info.destructibles.add_shapes(&w);
    assert_eq!(info.destructibles.shapes.len(), 2, "both models' shapes");
    info
}

/// Effect group names that exist in the user's ROM (two impact rows' groups), made-up names without one.
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

/// Target centres and whether the server broke them (written by the server thread).
type Targets = Arc<Mutex<Vec<([f32; 3], bool)>>>;

/// Aims at the first target the server has not broken yet and holds the fire button while one is left.
struct BrushShooter {
    targets: Targets,
    alive: Arc<AtomicBool>,
    live: bool,
    aimed: bool,
}

impl InputSource for BrushShooter {
    fn frame(&mut self, _view: &mut ViewAngles) {}
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
        let t = self.targets.lock().unwrap();
        self.aimed = false;
        if let Some((at, _)) = t.iter().find(|(_, dead)| !dead) {
            let (yaw, pitch) = dc_botclient::shooter::aim_angles(eye, *at);
            view.yaw = yaw;
            view.pitch = pitch;
            self.aimed = true;
        }
    }
    fn weapon(&mut self) -> WeaponCommands {
        WeaponCommands {
            fire: self.live && self.aimed,
            ..WeaponCommands::default()
        }
    }
    fn server(&mut self, v: &ServerHold) {
        self.live = v.life_state == Some(1);
        if self.live {
            self.alive.store(true, Ordering::SeqCst);
        }
    }
    fn label(&self) -> &'static str {
        "brush shooter"
    }
}

/// What the server thread tells the test.
#[derive(Debug, Default)]
struct ServerNotes {
    crate_dead: bool,
    drum_dead: bool,
    lines: Vec<String>,
}

#[test]
fn the_client_shoots_a_crate_and_a_drum_and_the_server_breaks_them() {
    let rom = dc_rom::test_rom();
    let bytes = world_bytes();
    let tables = match rom.as_deref() {
        Some(r) => RomTables::load(r).0,
        None => RomTables::default(),
    };
    let (crate_fx, drum_fx) = group_names(rom.as_deref());
    let info = server_world(&bytes, &tables, (&crate_fx, &drum_fx));
    let with_blast = info.destructibles.explosions[0].impact_fx.is_some();
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
            "enabled=on,effect_ms=2500",
            "-combat",
            "cone_check=log",
        ]
        .map(String::from),
    )
    .expect("server arguments");
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();

    let port = sock.local_addr().unwrap().port();
    let alive = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let targets = Arc::new(Mutex::new(vec![(CRATE_AT, false), (DRUM_AT, false)]));
    let server_thread = {
        let (stop, targets) = (stop.clone(), targets.clone());
        let extent = (info.extent_min, info.extent_max);
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
            let mut log = ServerNotes::default();
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
                        Output::Log(l) => log.lines.push(l),
                    }
                }
                let items = server.game().breakables().to_vec();
                if items.len() == 2 {
                    log.crate_dead = items[0].dead;
                    log.drum_dead = items[1].dead;
                    let mut t = targets.lock().unwrap();
                    t[0].1 = items[0].dead;
                    t[1].1 = items[1].dead;
                }
            }
            log
        })
    };
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "Shooter",
    );
    o.world_file = Some(WorldFile {
        bytes,
        stand_dims: dc_client::testkit::SYNTH_DIMS,
        textures: Vec::new(),
    });
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 16,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 30,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    let mut input = BrushShooter {
        targets,
        alive,
        live: false,
        aimed: false,
    };
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
    println!(
        "arms {:?}; fx {:?}; world effects {}",
        st.arms, r.fx, r.world_effects
    );
    for l in log
        .lines
        .iter()
        .filter(|l| l.contains("destroy") || l.contains("explosion") || l.contains("hits:"))
    {
        println!("    {l}");
    }
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert!(
        log.crate_dead && log.drum_dead,
        "both broke from the client's own pellets: {:?}",
        log.lines
            .iter()
            .filter(|l| l.contains("client 0"))
            .take(20)
            .collect::<Vec<_>>()
    );
    assert!(st.arms.fire_msgs >= 6, "{:?}", st.arms);
    assert_eq!(st.arms.hits, 0, "no 0x82 named a hit object: {:?}", st.arms);
    assert!(
        log.lines
            .iter()
            .filter(|l| l.contains("destroyed by client"))
            .count()
            >= 2
    );
    // the effects run only with a graphics adapter (a Linux CI runner has none): the server half above still holds
    if r.adapter.is_none() {
        println!("no GPU adapter: the effect checks are skipped");
        return;
    }
    // the client's own impacts lay on the models (their surface ids), and both death effects were handed over once
    assert!(r.fx.model_impacts >= 5, "{:?}", r.fx);
    assert_eq!(r.world_effects, 2, "{:?}", r.fx);
    assert_eq!(r.fx.world, 2, "{:?}", r.fx);
    if rom.is_some() {
        assert!(r.fx.groups_started >= 2, "the groups played: {:?}", r.fx);
        if with_blast {
            assert!(r.fx.blasts >= 1, "the drum's blast record: {:?}", r.fx);
        }
    }
}

/// CLI-250 without a server: a linked model of a class without a movement role stops shot rays (with its own surface)
/// only while the server's named brush has the ray-hit or solid flag and has not moved; the body never collides with
/// it; the level keeps its own surface.
#[test]
fn shot_rays_meet_the_models_the_server_made_ray_hit() {
    use dc_client::objects::ServerObjects;
    use dc_client::world::SynthSource;
    use dc_client::WorldSource;
    let mut src = SynthSource {
        name: "BRUSH".into(),
        bytes: world_bytes(),
        spawns: Vec::new(),
        stand_dims: dc_client::testkit::SYNTH_DIMS,
        textures: Vec::new(),
    };
    let mut world = src.load(None).expect("world");
    let eye = [CRATE_AT[0], 50.0, 0.0];
    let to_crate = {
        let d = [0, 1, 2].map(|k| CRATE_AT[k] - eye[k]);
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        d.map(|x| x / l)
    };
    let set = |flags: u16, moved: bool| {
        let mut s = ServerObjects::default();
        s.brushes.insert(b"crate1".to_vec(), flags);
        s.brushes.insert(b"drum1".to_vec(), 0x3101);
        if moved {
            s.moved.insert(b"crate1".to_vec());
        }
        s
    };
    // no server object yet: the ray goes on to the floor behind
    let h = world.shot_hit(eye, to_crate, 3000.0).expect("the floor");
    assert_eq!((h.surface, h.model.as_deref()), (FLOOR_SURFACE, None));
    for (flags, moved, hit) in [
        (0x3101u16, false, true),
        (0x1101, false, true),
        (0x2101, false, true),
        (0x0101, false, false),
        (0x0100, false, false),
        (0x3101, true, false),
    ] {
        world.apply_objects(&set(flags, moved));
        let h = world.shot_hit(eye, to_crate, 3000.0).expect("something");
        if hit {
            assert_eq!(h.model.as_deref(), Some(b"crate1".as_slice()), "{flags:#x}");
            assert_eq!(h.surface, CRATE_SURFACE, "{flags:#x}");
            assert!(
                (h.dist - (CRATE_AT[2] - HALF) / to_crate[2]).abs() < 1.0,
                "{}",
                h.dist
            );
        } else {
            assert_eq!(h.model, None, "{flags:#x} moved {moved}");
            assert_eq!(h.surface, FLOOR_SURFACE);
        }
        // the body never collides with the made-up class
        let body = world.collision.ray(
            V3::from_array(eye),
            V3::new(CRATE_AT[0], CRATE_AT[1], CRATE_AT[2] + 100.0),
        );
        assert!(body.is_none_or(|b| b.model == 0), "{body:?}");
    }
}

/// fx-fixes3 bug 3 (CLI-603), ROM-gated: bullet holes on a crate go with the crate. Records of world hits on the
/// crate's face leave marks while the server's brush is on; when the destroy record switches it off (flags 0x0100),
/// the shot ray through the old volume goes on to the floor behind it, and the crate's marks are dropped instead of
/// staying in the air; a mark on the floor stays.
#[test]
fn rom_marks_on_a_broken_crate_go_with_it_and_rays_pass_its_old_volume() {
    use dc_client::fx::{FxEvent, FxRunner, ShotRecord};
    use dc_client::objects::ServerObjects;
    use dc_client::world::SynthSource;
    use dc_client::WorldSource;
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut src = SynthSource {
        name: "BRUSH".into(),
        bytes: world_bytes(),
        spawns: Vec::new(),
        stand_dims: dc_client::testkit::SYNTH_DIMS,
        textures: Vec::new(),
    };
    let mut world = src.load(None).expect("world");
    let set = |flags: u16| {
        let mut s = ServerObjects::default();
        s.brushes.insert(b"crate1".to_vec(), flags);
        s.brushes.insert(b"drum1".to_vec(), 0x3101);
        s
    };
    world.apply_objects(&set(0x3101));
    let eye = [CRATE_AT[0], 50.0, 0.0];
    let dir = {
        let d = [0, 1, 2].map(|k| CRATE_AT[k] - eye[k]);
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        d.map(|x| x / l)
    };
    let on_crate = world.shot_hit(eye, dir, 3000.0).expect("the crate");
    assert_eq!(on_crate.model.as_deref(), Some(b"crate1".as_slice()));
    let floor_at = [300.0, 0.0, 300.0];
    let rec = |to: [f32; 3], surface: u8| ShotRecord {
        shooter: 7,
        first: 0,
        weapon: 145,
        ammo: 145,
        from: eye,
        to,
        normal: [-dir[0], -dir[1], -dir[2]],
        surface,
        clear: 0x1010,
        own: false,
        mods: [0xff; 3],
    };
    // a ROM surface whose impacts leave a mark (the synthetic world's own ids are not ROM rows)
    let mut probe = FxRunner::open(Some(&rom));
    let surface = (1u8..=120)
        .find(|s| {
            let before = probe.live_counts().2;
            let at = [f32::from(*s) * 40.0 - 2400.0, 0.0, -1500.0];
            probe.feed(0, &[FxEvent::Record(rec(at, *s))], None);
            probe.live_counts().2 > before
        })
        .expect("a surface with bullet holes");
    let mut fx = FxRunner::open(Some(&rom));
    let mut events: Vec<FxEvent> = (0..4)
        .map(|i| {
            let mut p = on_crate.point;
            p[0] += i as f32 * 3.0;
            FxEvent::Record(rec(p, surface))
        })
        .collect();
    events.push(FxEvent::Record(rec(floor_at, surface)));
    fx.feed(100, &events, Some(&world));
    let marks_before = fx.live_counts().2;
    assert!(
        marks_before >= 5,
        "marks on the crate and the floor: {marks_before}"
    );
    // the destroy record: the crate's brush loses visible / solid / ray-hit
    world.apply_objects(&set(0x0100));
    let through = world.shot_hit(eye, dir, 3000.0).expect("the floor behind");
    assert_eq!(through.model, None, "the ray passes the old volume");
    assert!(
        through.dist > on_crate.dist + 2.0 * HALF - 1.0,
        "{} vs {}",
        through.dist,
        on_crate.dist
    );
    fx.feed(200, &[], Some(&world));
    assert_eq!(fx.live_counts().2, 1, "only the floor mark stays");
    assert_eq!(fx.stats.marks_dropped, 4);
    // a record of a world hit that arrives after the crate left (a shot fired before the destroy) leaves no mark in
    // the air
    fx.feed(
        300,
        &[FxEvent::Record(rec(on_crate.point, surface))],
        Some(&world),
    );
    assert_eq!(fx.live_counts().2, 1);
}
