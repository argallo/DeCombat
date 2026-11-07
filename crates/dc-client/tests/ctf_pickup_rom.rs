//! ROM-gated (skipped without CA_ROM): the flag pickup on the room of the 2026-11-06 report (map row 1, mode row 4: a
//! capture-the-flag world). The headless client (no GPU) joins our dedicated server in this process as client 0, sends
//! its first update in the reliable state form as a human's client does, walks onto the enemy flag and the server
//! hands it the flag (DSV-192). Two offline checks on the same world: every flag is reached from most sides by a
//! walking body within the touch distances, and each team's spawn points lie nearer its own flag than the enemy's.
//! Nothing of the ROM is asserted by value and no ROM name is printed; output is distances and counts.
// DC-DEFAULT: DSV-192 (proposed)

use dc_net_engine::socket_reader::SocketReader;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, RemoteBody,
    RomWorld, ServerHold, ViewAngles, WorldSource,
};
use dc_dserver::game::ctf::{TOUCH_HEIGHT, TOUCH_RADIUS};
use dc_dserver::session::{Output, Server};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{SpawnPoint, WorldInfo};
use dc_sim::{Commands, V3};

/// Map and mode rows of the reported room.
const MAP_ROW: usize = 1;
const MODE_ROW: usize = 4;

/// The room's line, the server's view of its world and the client's.
struct Room {
    line: String,
    info: WorldInfo,
    local: dc_client::LoadedWorld,
    tables: dc_dserver::rom_tables::RomTables,
}

fn room(rom: &std::path::Path) -> Option<Room> {
    let vfs = dc_rez::RezVfs::new(rom);
    let gr = dc_bute::GameRoomData::from_vfs(&vfs).expect("room table");
    let world = dc_bute::world_for_room(rom, &gr, MAP_ROW, MODE_ROW, false, None);
    let line = dc_bute::room_line_with_vfs(
        &vfs,
        &gr,
        &dc_bute::RoomConfig {
            map: MAP_ROW,
            mode: MODE_ROW,
            goal: 3,
            ..dc_bute::RoomConfig::default()
        },
        &dc_bute::RoomOptions::default(),
    );
    let map = world.trim_start_matches("Worlds\\").to_string();
    let local = RomWorld::new(rom, &map).load(None).ok()?;
    let info =
        dc_dserver::world::load_from_rom(rom, &world, dc_dserver::world::NameForm::Listed, None)
            .ok()?;
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(rom);
    Some(Room {
        line,
        info,
        local,
        tables,
    })
}

/// The flags with a model and a team: (team, position).
fn flags(r: &Room) -> Vec<(u8, V3)> {
    r.info
        .flags
        .iter()
        .filter(|f| f.model.is_some())
        .filter_map(|f| {
            let t = r.tables.teams.team_of(f.team_text.as_deref())?;
            Some((t, V3::from_array(f.pos)))
        })
        .collect()
}

fn across(a: V3, b: V3) -> f32 {
    ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

/// Walks straight at `target` (horizontally) while alive, until the server says the flag is taken.
struct Walker {
    target: [f32; 3],
    live: bool,
    taken: Arc<AtomicBool>,
}

impl InputSource for Walker {
    fn frame(&mut self, _view: &mut ViewAngles) {}
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands {
            forward: self.live && !self.taken.load(Ordering::SeqCst),
            ..Commands::default()
        }
    }
    fn aim(
        &mut self,
        eye: [f32; 3],
        _remotes: &[RemoteBody],
        _own_team: Option<u8>,
        view: &mut ViewAngles,
    ) {
        let (yaw, _) =
            dc_botclient::shooter::aim_angles(eye, [self.target[0], eye[1], self.target[2]]);
        view.yaw = yaw;
        view.pitch = 0.0;
    }
    fn server(&mut self, v: &ServerHold) {
        self.live = v.life_state == Some(1);
    }
    fn label(&self) -> &'static str {
        "flag walker"
    }
}

/// A floor point `dist` from `flag` near its height with a clear line to it (the client's collision set, at knee,
/// centre and head height).
fn spot_for(cw: &dc_sim::CollisionWorld, flag: V3) -> Option<V3> {
    for dist in [260.0f32, 200.0, 320.0, 400.0] {
        for k in 0..24 {
            let a = k as f32 * std::f32::consts::TAU / 24.0;
            let (x, z) = (flag.x + dist * a.sin(), flag.z + dist * a.cos());
            let from = V3::new(x, flag.y + 60.0, z);
            let to = V3::new(x, flag.y - 200.0, z);
            let Some(h) = cw.ray(from, to) else {
                continue;
            };
            if h.normal.y < 0.8 {
                continue;
            }
            let ground = from + (to - from) * h.t;
            if (ground.y - flag.y).abs() > 60.0 {
                continue;
            }
            let clear = [20.0f32, 50.0, 90.0].iter().all(|&up| {
                let a = ground + V3::new(0.0, up, 0.0);
                cw.ray(a, V3::new(flag.x, a.y, flag.z)).is_none()
            });
            if clear {
                return Some(ground);
            }
        }
    }
    None
}

#[test]
fn rom_the_client_takes_the_enemy_flag_by_walking_onto_it() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Some(rm) = room(&rom) else {
        println!("skipped: the room's world is not in this ROM");
        return;
    };
    // the enemy flag of team 1 (the walker's team)
    let enemy = flags(&rm)
        .into_iter()
        .find(|f| f.0 == 2)
        .map(|f| f.1)
        .expect("a team-2 flag in the world");
    let Room {
        line,
        mut info,
        local,
        tables,
    } = rm;
    let spot = spot_for(&local.collision, enemy).expect("a clear walk to the flag");
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
            // the stand box's centre above the floor point (the server's spawn placement lifts by a step only)
            pos: (spot + V3::new(0.0, local.stand_dims.y + 1.0, 0.0)).to_array(),
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
        ]
        .map(String::from),
    )
    .expect("server arguments");
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = sock.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let taken = Arc::new(AtomicBool::new(false));
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    // the walker's server-side place nearest the untaken flag: (across, up)
    let nearest = Arc::new(Mutex::new((f32::INFINITY, 0.0f32)));
    let server_thread = {
        let (stop, taken, lines, nearest) =
            (stop.clone(), taken.clone(), lines.clone(), nearest.clone());
        let extent = (info.extent_min, info.extent_max);
        std::thread::spawn(move || {
            let start = Instant::now();
            let now = || start.elapsed().as_millis() as u32;
            let mut server: Server<SocketAddr> =
                Server::with_tables(args.server.clone(), info, tables, now());
            server.set_endpoint("127.0.0.1", port);
            assert!(server.handle_line(0, &format!("room {line}")));
            assert!(server.handle_line(0, "user 1 1 1 0 0 Walker"));
            assert!(server.handle_line(0, "user 2 2 2 1 0 Idle"));
            let idle_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
            let mut cfg = SynthConfig::named("Idle");
            cfg.box_bounds = Some(extent);
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
                // the walker joins first: client 0, as the human of the report
                let idle_out = if server.game().players().is_empty() {
                    Vec::new()
                } else {
                    idle.poll(t)
                };
                for d in idle_out {
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
                let g = server.game();
                let flag = g.ctf().flags.iter().find(|f| f.team == 2).cloned();
                if let (Some(f), Some(p)) = (flag, g.players().values().find(|p| p.team == 1)) {
                    if f.carrier == Some(p.pid) {
                        taken.store(true, Ordering::SeqCst);
                    } else if f.carrier.is_none() {
                        let d = across(V3::from_array(p.pos), V3::from_array(f.pos));
                        let mut n = nearest.lock().unwrap();
                        if d < n.0 {
                            *n = (d, p.pos[1] - f.pos[1]);
                        }
                    }
                }
            }
        })
    };
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "Walker",
    );
    o.rom = Some(rom.clone());
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 12,
        frame_us: 16_667,
        render: false,
        realtime: true,
        trace: true,
        ..HeadlessOptions::default()
    };
    let mut input = Walker {
        target: enemy.to_array(),
        live: false,
        taken: taken.clone(),
    };
    let run = run_headless(&mut net, Some(&mut input as &mut dyn InputSource), &opts);
    let st = handle.shutdown(Duration::from_secs(5));
    stop.store(true, Ordering::SeqCst);
    server_thread.join().expect("server thread");
    let lines = lines.lock().unwrap().clone();
    let r = run.expect("headless run");
    let n = *nearest.lock().unwrap();
    let own = r
        .trace
        .iter()
        .map(|t| (across(t.pos, enemy), t.pos.y - enemy.y))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    println!(
        "start {:.0} across; server's nearest untaken {:.1} across {:+.1} up; client's nearest {own:?}",
        across(spot, enemy),
        n.0,
        n.1
    );
    let first = lines
        .iter()
        .find(|l| l.contains("client 0: first player update"));
    println!("{first:?}");
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert!(
        first.is_some_and(|l| l.contains("(reliable, flags 0x0010, position None)")),
        "the walker is client 0 and opens with the state form, as the human of the report"
    );
    if !taken.load(Ordering::SeqCst) {
        for l in lines.iter().rev().take(30).rev() {
            println!("    {l}");
        }
    }
    assert!(
        taken.load(Ordering::SeqCst),
        "the server handed the walker the flag"
    );
    assert!(lines
        .iter()
        .any(|l| l.contains("ctf: team 2 flag taken by client 0")));
}

/// A body walking straight at a flag from 16 sides 220 units out (the client's collision set, every linked model
/// solid) comes within the touch distances from most of them: no solid round a flag keeps a player off it.
#[test]
fn rom_both_flags_are_reached_from_most_sides() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Some(r) = room(&rom) else {
        println!("skipped: the room's world is not in this ROM");
        return;
    };
    let fl = flags(&r);
    assert_eq!(fl.len(), 2, "two flags with a team");
    let params = dc_sim::MovementParams::default();
    for (team, flag) in fl {
        let mut reached = 0;
        let mut misses = Vec::new();
        for k in 0..16 {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            let (x, z) = (flag.x + 220.0 * a.sin(), flag.z + 220.0 * a.cos());
            let from = V3::new(x, flag.y + 100.0, z);
            let Some(h) = r.local.collision.ray(from, from - V3::new(0.0, 300.0, 0.0)) else {
                misses.push(f32::NAN);
                continue;
            };
            let ground = from.y - 300.0 * h.t;
            let start = V3::new(x, ground + r.local.stand_dims.y + 1.0, z);
            let mut st = dc_sim::PlayerState::new(start, r.local.stand_dims, &params);
            let mut best = (f32::INFINITY, 0.0f32);
            for _ in 0..(60 * 8) {
                let d = flag - st.pos;
                let input = dc_sim::TickInput {
                    commands: Commands {
                        forward: true,
                        ..Commands::default()
                    },
                    yaw: d.x.atan2(d.z),
                    pitch: 0.0,
                    ctrl_state: 1,
                    ..dc_sim::TickInput::default()
                };
                st.step(&params, &r.local.collision, &input, 1.0 / 60.0);
                let flat = across(st.pos, flag);
                if flat < best.0 {
                    best = (flat, st.pos.y - flag.y);
                }
            }
            if best.0 <= TOUCH_RADIUS && best.1.abs() <= TOUCH_HEIGHT {
                reached += 1;
            } else {
                misses.push(best.0);
            }
        }
        println!(
            "team {team} flag: reached from {reached} of 16 sides; nearest of the others {misses:?}"
        );
        assert!(reached >= 12, "team {team}: {reached} of 16");
    }
}

/// Each team's spawn points lie nearer its own flag than the enemy's: a player who runs from his spawn to the far
/// flag runs to the flag the server hands him.
#[test]
fn rom_team_spawns_are_nearer_their_own_flag() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Some(r) = room(&rom) else {
        println!("skipped: the room's world is not in this ROM");
        return;
    };
    let fl = flags(&r);
    let of = |t: u8| fl.iter().find(|f| f.0 == t).map(|f| f.1).expect("flag");
    let mut counted = [0usize; 2];
    for s in &r.info.spawns {
        let Some(team) = r.tables.teams.team_of(s.team_filter.as_deref()) else {
            continue;
        };
        let p = V3::from_array(s.pos);
        let (own, enemy) = (of(team), of(3 - team));
        assert!(
            (p - own).len() < (p - enemy).len(),
            "a team {team} spawn point nearer the enemy flag"
        );
        counted[usize::from(team - 1)] += 1;
    }
    println!("team spawn points: {counted:?}");
    assert!(counted.iter().all(|&n| n > 0));
}
