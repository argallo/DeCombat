//! client-hitnodes (CLI-300), ROM-gated loopback: the client's shots name the node of the 2009 node-sphere test and
//! our server resolves it through the victim's skeleton row. A dedicated server runs in this process on a loopback
//! socket (the `dc-dserver` state machine with the ROM tables and `-verbose`, a made-up floor world); a synthetic
//! second player stands at the other spawn point with the server's default character (posed by the server's stand
//! clips; it turns slowly about its axis); the real client session (`NetWorld`, headless frame loop, session clock)
//! holds the ROM rifle and fires three single shots, each when the aim it computes for the shot meets the drawn pose:
//!
//! 1. a level ray at the eye height, 6 units beside the head (beside every head sphere): the 2009 rule gives no hit,
//!    so the 0x82 carries no hit object and the server judges nothing;
//! 2. a ray at a torso node of the drawn body: the client names a torso-location node, the server takes the node's
//!    factor;
//! 3. a level ray at the eye height through the drawn head: the client names a head-location node (the 9 / 53 family
//!    of the male skeleton row), the server deals the rifle's damage x the node's factor (a one-shot kill) in the head
//!    zone, and its `@player` line counts one head shot.
//!
//! The aim uses the pose the client itself draws (its own [`HitRigs`], inline), so the test checks the path from the
//! pose to the wire and through the server, not the aiming. Skipped without `CA_ROM`. No window, no GPU needed.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1, §4.1.1 (the extra block of a hit: object, part byte, point, normal);
// DSERVER_PIPE.md §4.4 (`@player ... hs=`)
// EVIDENCE: cshell+0x234fb0, cshell+0x235c00, cshell+0x238eb0
// DC-DEFAULT: CLI-300 (proposed), DSV-30 (the node through the victim's row), DSV-42 (`-verbose`)

use dc_net_engine::socket_reader::SocketReader;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dc_client::arms::{RayHit, Target, WeaponCommands};
use dc_client::hitnodes::{FrameNodes, HitRigs, HitSkeletons, NodeSphere, NodeStats};
use dc_client::testkit::synth_font;
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetHandle, NetOptions, NetWorld,
    RemoteBody, ServerHold, ViewAngles, WorldFile,
};
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::SpawnPoint;
use dc_sim::from_world::FILE_FIRST_CHILD_IS_NEGATIVE;
use dc_sim::{build_bsp, Commands, ConvexPoly, SURF_SOLID, V3};
use dc_world::synth::{SynthModel, SynthWorld};

/// The two spawn points (the floor is at y 0; the server stands each box on it).
const SPAWNS: [[f32; 3]; 2] = [[0.0, 0.0, 0.0], [0.0, 0.0, 200.0]];
/// How far beside the head the miss passes (world units).
const BESIDE: f32 = 6.0;
/// Frames between two shots (the spread and the recoil settle).
const SHOT_GAP_FRAMES: u32 = 90;
/// The shot range of the predictions (the rifle row's range is longer than the spawn distance anyway).
const RANGE: f32 = 3000.0;
/// Frames the shooter waits once the victim is first in sight before its first shot: the client's shot test passes a
/// character inside its spawn window (CLI-572, `dc_client::net::SPAWN_SHIELD_MS` after the start note 0x1f), and the
/// frame loop runs in real time; 4 s covers the 3.5 s window.
const SHIELD_WAIT_FRAMES: u32 = 240;
/// ROM `Node<k>Location` values the test asserts on: head 1, torso 2 (the dserver's zone numbers).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3 (`hit_zone` 1 = head); DC_DEFAULTS DSV-30 (location 1 head, 2 torso)
const LOC_HEAD: u32 = 1;
const LOC_TORSO: u32 = 2;

fn floor_world() -> Vec<u8> {
    let floor = ConvexPoly::from_corners(
        vec![
            V3::new(-2000.0, 0.0, -2000.0),
            V3::new(-2000.0, 0.0, 2000.0),
            V3::new(2000.0, 0.0, 2000.0),
            V3::new(2000.0, 0.0, -2000.0),
        ],
        SURF_SOLID,
    );
    let (nodes, root) = build_bsp(std::slice::from_ref(&floor));
    let mut m = SynthModel {
        flags: 0,
        name: b"level".to_vec(),
        textures: vec![b"none".to_vec()],
        surfaces: vec![(SURF_SOLID, 0, 3)],
        box_min: [-2000.0, -1.0, -2000.0],
        box_max: [2000.0, 1.0, 2000.0],
        ..SynthModel::default()
    };
    m.planes.push((floor.normal.to_array(), floor.dist));
    m.points.extend(floor.verts.iter().map(|v| v.to_array()));
    m.polys
        .push((0, 0, (0..floor.verts.len() as u32).collect()));
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
    SynthWorld {
        extent_min: [-2000.0, -100.0, -2000.0],
        extent_max: [2000.0, 600.0, 2000.0],
        tree_shape: vec![false],
        models: vec![m],
        grid_cell: [64.0; 3],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![0, 0, 0],
        ..SynthWorld::default()
    }
    .build()
}

/// `Node<k>Location` of a skeleton row, read from the ROM for the assertions (no value in the tree).
fn locations(rom: &std::path::Path, skeleton: u32) -> Vec<u32> {
    let vfs = dc_rez::RezVfs::new(rom);
    let doc = dc_bute::ButeDoc::try_from_vfs(&vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\MODELBUTES.TXT")
        .ok()
        .flatten()
        .expect("model table");
    let sec = doc
        .get(&format!("Skeleton{skeleton}"))
        .expect("skeleton row");
    (0..255)
        .map_while(|k| {
            sec.get(&format!("Node{k}Name"))?;
            Some(
                sec.get(&format!("Node{k}Location"))
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .map_or(0, |v| v as u32),
            )
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Beside,
    Torso,
    Head,
}

/// One shot as the input planned and the client sent it.
#[derive(Debug, Clone)]
struct Fired {
    kind: Kind,
    predicted: RayHit,
    /// The client's node counters before and after the shot.
    before: NodeStats,
    after: NodeStats,
    hits_before: u32,
    hits_after: u32,
    /// The client's 0x82 count when the shot was planned (the shot has left once it grows).
    msgs_before: u32,
}

/// What the test reads back from the input.
#[derive(Debug, Default)]
struct Notes {
    fired: Vec<Fired>,
    skeleton: Option<u32>,
}

/// Fires the three planned shots, each when the aim it computes for it meets the drawn pose.
struct NodeShooter {
    rigs: HitRigs,
    table: HitSkeletons,
    rom: std::path::PathBuf,
    locs: Option<Vec<u32>>,
    handle: NetHandle,
    plan: Vec<Kind>,
    live: bool,
    wait: u32,
    /// Frames the victim has been in sight (the first shot waits [`SHIELD_WAIT_FRAMES`]).
    seen: u32,
    /// The shot being fired: its plan entry and the counters before it.
    pending: Option<Fired>,
    fire: bool,
    notes: Arc<Mutex<Notes>>,
}

fn dir_to(from: [f32; 3], to: [f32; 3]) -> [f32; 3] {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    d.map(|x| x / l)
}

impl NodeShooter {
    fn location(&self, node: u8) -> u32 {
        self.locs
            .as_ref()
            .and_then(|l| l.get(node as usize).copied())
            .unwrap_or(0)
    }

    /// An aim point for `kind` whose predicted trace is the wanted outcome (`None`: not this frame).
    fn aim_for(
        &self,
        kind: Kind,
        eye: [f32; 3],
        target: Target,
        nodes: &FrameNodes,
        spheres: &[NodeSphere],
    ) -> Option<([f32; 3], RayHit)> {
        let predict = |p: [f32; 3]| {
            let mut st = NodeStats::default();
            dc_client::hitnodes::trace(eye, dir_to(eye, p), RANGE, &[target], nodes, None, &mut st)
        };
        let of = |loc: u32| spheres.iter().filter(move |s| self.location(s.node) == loc);
        match kind {
            Kind::Head => of(LOC_HEAD).find_map(|s| {
                // level at the eye height, at the head node's place
                let p = [s.centre[0], eye[1], s.centre[2]];
                let h = predict(p);
                matches!(h, RayHit::Character { node, .. } if self.location(node) == LOC_HEAD)
                    .then_some((p, h))
            }),
            Kind::Torso => of(LOC_TORSO).find_map(|s| {
                let h = predict(s.centre);
                matches!(h, RayHit::Character { node, .. } if self.location(node) == LOC_TORSO)
                    .then_some((s.centre, h))
            }),
            Kind::Beside => {
                // level at the eye height, BESIDE units sideways (across the line of sight) from the head sphere
                // whose rim reaches farthest to that side
                let d = dir_to(eye, target.centre);
                let side = [d[2], 0.0, -d[0]];
                let heads: Vec<&NodeSphere> = of(LOC_HEAD).collect();
                [1.0f32, -1.0].into_iter().find_map(|sign| {
                    let reach =
                        |s: &&NodeSphere| sign * (s.centre[0] * side[0] + s.centre[2] * side[2]);
                    let edge = heads.iter().max_by(|a, b| reach(a).total_cmp(&reach(b)))?;
                    let p = [
                        edge.centre[0] + sign * side[0] * BESIDE,
                        eye[1],
                        edge.centre[2] + sign * side[2] * BESIDE,
                    ];
                    // beside every head sphere's centre by at least BESIDE, and a predicted miss
                    let ray = dir_to(eye, p);
                    let clear = heads.iter().all(|s| {
                        let v = [
                            s.centre[0] - eye[0],
                            s.centre[1] - eye[1],
                            s.centre[2] - eye[2],
                        ];
                        let t = v[0] * ray[0] + v[1] * ray[1] + v[2] * ray[2];
                        let q = [ray[0] * t - v[0], ray[1] * t - v[1], ray[2] * t - v[2]];
                        (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt() >= BESIDE - 0.5
                    });
                    let h = predict(p);
                    (clear && h == RayHit::Miss).then_some((p, h))
                })
            }
        }
    }
}

impl InputSource for NodeShooter {
    fn frame(&mut self, _view: &mut ViewAngles) {}
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands::default()
    }
    fn aim(
        &mut self,
        eye: [f32; 3],
        remotes: &[RemoteBody],
        _own_team: Option<u8>,
        view: &mut ViewAngles,
    ) {
        let arms = self.handle.stats().arms;
        if let Some(p) = self.pending.as_mut() {
            if arms.fire_msgs > p.msgs_before {
                // the shot left: release the trigger and wait
                let mut p = self.pending.take().expect("pending");
                p.after = arms.nodes;
                p.hits_after = arms.hits;
                self.notes.lock().unwrap().fired.push(p);
                self.fire = false;
                self.wait = SHOT_GAP_FRAMES;
                return;
            }
        }
        if self.wait > 0 {
            self.wait -= 1;
            self.fire = false;
            return;
        }
        // the shot being fired keeps its kind until it has left
        let next = self.pending.as_ref().map(|p| p.kind);
        let Some(kind) = next.or_else(|| self.plan.first().copied()) else {
            self.fire = false;
            return;
        };
        let Some(r) = remotes
            .iter()
            .find(|r| r.look.as_ref().is_some_and(|l| l.visible))
        else {
            self.fire = false;
            return;
        };
        if !self.live {
            self.fire = false;
            return;
        }
        // CLI-572 / DSV-156 (b97d37f): no shot while the victim may still be shielded
        if self.seen < SHIELD_WAIT_FRAMES {
            self.seen += 1;
            self.fire = false;
            return;
        }
        let nodes = self.rigs.frame(remotes, None);
        let Some(spheres) = nodes.of(r.obj).map(<[NodeSphere]>::to_vec) else {
            self.fire = false;
            return;
        };
        if self.locs.is_none() {
            let model = r
                .look
                .as_ref()
                .and_then(|l| l.models.first())
                .map(|m| m.model.clone());
            let s = model.and_then(|m| self.table.skeleton_number_of_model(&m));
            if let Some(s) = s {
                self.locs = Some(locations(&self.rom, s));
                self.notes.lock().unwrap().skeleton = Some(s);
            }
        }
        let target = Target {
            obj: r.obj,
            centre: r.pos,
            half: r.half,
        };
        match self.aim_for(kind, eye, target, &nodes, &spheres) {
            Some((p, predicted)) => {
                let (yaw, pitch) = dc_botclient::shooter::aim_angles(eye, p);
                view.yaw = yaw;
                view.pitch = pitch;
                if self.pending.is_none() {
                    self.plan.remove(0);
                    self.pending = Some(Fired {
                        kind,
                        predicted,
                        before: arms.nodes,
                        after: arms.nodes,
                        hits_before: arms.hits,
                        hits_after: arms.hits,
                        msgs_before: arms.fire_msgs,
                    });
                } else if let Some(p) = self.pending.as_mut() {
                    p.predicted = predicted;
                }
                self.fire = true;
            }
            None => self.fire = false,
        }
    }
    fn weapon(&mut self) -> WeaponCommands {
        WeaponCommands {
            fire: self.live && self.fire,
            ..WeaponCommands::default()
        }
    }
    fn server(&mut self, v: &ServerHold) {
        self.live = v.life_state == Some(1);
    }
    fn label(&self) -> &'static str {
        "node shooter"
    }
}

/// What the server thread hands back.
#[derive(Debug, Default)]
struct ServerNotes {
    lines: Vec<String>,
    pipe: Vec<String>,
}

#[test]
fn rom_the_client_names_head_and_torso_nodes_and_misses_beside_the_head() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let bytes = floor_world();
    let tables = RomTables::load(&rom).0;
    assert!(!tables.chars.is_empty(), "the ROM's character tables");
    let base_damage = tables
        .weapons
        .damage(145, 145)
        .expect("the rifle's damage row");
    let skeleton_factors = tables.chars.skeletons.clone();
    let mut info = dc_dserver::world::world_from_bytes(&bytes, "Worlds\\Nodes.dat", "Nodes")
        .expect("server world");
    info.spawns = SPAWNS
        .iter()
        .map(|p| SpawnPoint {
            pos: *p,
            rot: [0.0; 3],
            team_filter: None,
        })
        .collect();
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
            "-combat",
            "cone_check=log",
            "-verbose",
        ]
        .map(String::from),
    )
    .expect("server arguments");
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();

    let port = sock.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let server_thread = {
        let stop = stop.clone();
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
            let mut notes = ServerNotes::default();
            let rd = SocketReader::start(&sock, 4096).unwrap();
            let idle_rd = SocketReader::start(&idle_sock, 4096).unwrap();
            let mut killed_at: Option<u32> = None;
            let mut finished = false;
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
                // the head kill ends the match: the `@player` lines, then the client's bye
                if !finished && killed_at.is_some_and(|k| t >= k + 1500) {
                    server.finish(t);
                    finished = true;
                }
                for o in server.poll(t) {
                    match o {
                        Output::Transmit { to, bytes } => {
                            let _ = sock.send_to(&bytes, to);
                        }
                        Output::Pipe(l) => notes.pipe.push(l),
                        Output::Log(l) => {
                            if killed_at.is_none() && l.contains(" killed by Some(") {
                                killed_at = Some(t);
                            }
                            notes.lines.push(l);
                        }
                    }
                }
            }
            if !finished {
                server.finish(now());
                for o in server.poll(now()) {
                    if let Output::Pipe(l) = o {
                        notes.pipe.push(l);
                    }
                }
            }
            notes
        })
    };
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "Shooter",
    );
    // SIM-25 (9684fd2): the eye height comes from the standing box (`PlayerState::stand_dims`), no longer from the box
    // in force, so the client's standing box must be the ROM character's, the box the server gives both bodies; with
    // the made-up 40-unit half height the eye sat at 26.4 and a level ray never met the drawn head (about 28-34 up)
    let stand_dims = dc_client::world::rom_stand_dims(&rom).expect("the ROM's standing box");
    o.world_file = Some(WorldFile {
        bytes,
        stand_dims,
        textures: Vec::new(),
    });
    o.rom = Some(rom.clone());
    // the shot test's rig is loaded in the frame that first needs it (the input waits for the pose anyway)
    o.hit_rigs_async = false;
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let notes = Arc::new(Mutex::new(Notes::default()));
    let vfs = dc_rez::RezVfs::new(&rom);
    let mut input = NodeShooter {
        rigs: HitRigs::new(Some(rom.clone()), false),
        table: HitSkeletons::from_vfs(&vfs).expect("model table"),
        rom: rom.clone(),
        locs: None,
        handle: handle.clone(),
        plan: vec![Kind::Beside, Kind::Torso, Kind::Head],
        live: false,
        wait: 0,
        seen: 0,
        pending: None,
        fire: false,
        notes: notes.clone(),
    };
    let opts = HeadlessOptions {
        frames: 60 * 90,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 60,
        font_bytes: Some(synth_font()),
        realtime: true,
        stop_after_over: Some(30),
        ..HeadlessOptions::default()
    };
    let run = run_headless(&mut net, Some(&mut input as &mut dyn InputSource), &opts);
    let st = handle.shutdown(Duration::from_secs(5));
    stop.store(true, Ordering::SeqCst);
    let log = server_thread.join().expect("server thread");
    let (fired_all, skeleton) = {
        let n = notes.lock().unwrap();
        (n.fired.clone(), n.skeleton)
    };
    let dump = |why: &str| -> String {
        let mut s = format!("{why}\n  client arms {:?}\n  fired:\n", st.arms);
        for f in &fired_all {
            s.push_str(&format!("    {f:?}\n"));
        }
        for l in log.lines.iter().filter(|l| !l.contains("ping")) {
            s.push_str(&format!("    | {l}\n"));
        }
        for l in &log.pipe {
            s.push_str(&format!("    > {l}\n"));
        }
        s
    };
    let r = run.unwrap_or_else(|e| panic!("{}", dump(&format!("headless run: {e:?}"))));
    println!("{}", r.summary());
    println!("{}", dump("result"));
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    let skeleton = skeleton.expect("the victim's skeleton row");
    let locs = locations(&rom, skeleton);
    let factors = skeleton_factors
        .get(&skeleton)
        .expect("server skeleton row");
    let fired: Vec<&Fired> = fired_all.iter().collect();
    assert_eq!(
        fired.iter().map(|f| f.kind).collect::<Vec<_>>(),
        vec![Kind::Beside, Kind::Torso, Kind::Head],
        "{}",
        dump("all three shots")
    );
    // the dserver's accepted-hit lines name the client's node: "(node N, zone Z, factor F, damage D)"
    let accepted: Vec<(i32, u32, f32)> = log
        .lines
        .iter()
        .filter(|l| l.contains(": hit on client ") && l.contains(" accepted (node "))
        .filter_map(|l| {
            let rest = l.split("(node ").nth(1)?;
            let node: i32 = rest.split(',').next()?.trim().parse().ok()?;
            let zone: u32 = rest
                .split("zone ")
                .nth(1)?
                .split(',')
                .next()?
                .trim()
                .parse()
                .ok()?;
            let dmg: f32 = rest
                .split("damage ")
                .nth(1)?
                .trim_end_matches(')')
                .trim()
                .parse()
                .ok()?;
            Some((node, zone, dmg))
        })
        .collect();
    // 1. beside the head: the client's own trace found no sphere, sent no hit object, the server judged nothing
    let b = fired[0];
    assert_eq!(
        b.hits_after,
        b.hits_before,
        "{}",
        dump("the beside shot sent a hit")
    );
    assert!(
        b.after.sphere_misses > b.before.sphere_misses,
        "{}",
        dump("the beside shot entered the box without a sphere")
    );
    // 2. the torso shot and 3. the head shot: two accepted claims, in that order
    assert_eq!(accepted.len(), 2, "{}", dump("two accepted hits"));
    let (torso_node, torso_zone, torso_dmg) = accepted[0];
    let (head_node, head_zone, head_dmg) = accepted[1];
    assert_eq!(
        locs.get(torso_node as usize).copied(),
        Some(LOC_TORSO),
        "{}",
        dump("a torso-location node")
    );
    assert_eq!(torso_zone, LOC_TORSO, "{}", dump("torso zone"));
    let tf = factors[torso_node as usize].factor;
    assert!(
        (torso_dmg - base_damage * tf).abs() < 0.11,
        "{}",
        dump("torso damage")
    );
    assert_eq!(
        locs.get(head_node as usize).copied(),
        Some(LOC_HEAD),
        "{}",
        dump("a head-location node")
    );
    assert_eq!(head_zone, LOC_HEAD, "{}", dump("head zone"));
    let hf = factors[head_node as usize].factor;
    assert!(
        (head_dmg - base_damage * hf).abs() < 0.11,
        "{}",
        dump("head damage")
    );
    assert!(head_dmg >= 100.0, "{}", dump("the head shot kills at once"));
    assert!(
        log.lines
            .iter()
            .any(|l| l.contains(" killed by Some(") && l.contains(") (zone 1,")),
        "{}",
        dump("a head-zone kill")
    );
    let shooter = log
        .pipe
        .iter()
        .find(|l| l.starts_with("@player") && l.ends_with("name=Shooter"))
        .unwrap_or_else(|| panic!("{}", dump("the shooter's @player line")));
    assert!(
        shooter.contains(" hs=1 "),
        "{}",
        dump("one head shot counted")
    );
    // the client side agrees with its predictions (same pose) and counted two node hits
    assert_eq!(
        (st.arms.nodes.node_hits, st.arms.nodes.box_hits),
        (2, 0),
        "{}",
        dump("two node hits, no box-only hit")
    );
    for f in &fired[1..] {
        assert!(matches!(f.predicted, RayHit::Character { .. }), "{f:?}");
    }
    println!(
        "nodes: torso {torso_node} (factor {tf}), head {head_node} (factor {hf}); damage {torso_dmg} / {head_dmg}"
    );
}
