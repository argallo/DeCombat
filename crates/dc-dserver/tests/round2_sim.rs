//! Deterministic session tests of the round-2 rules (DSV-46..52, proposed): the server and synthetic clients over the
//! simulated channel with an injected clock, on the synthetic world with made-up spawn points, names and volumes (no
//! ROM content).

use dc_dserver::game::GameTuning;
use dc_dserver::rom_tables::{RomTables, TeamNames};
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig};
use dc_dserver::world::{synthetic_world_bytes, world_from_bytes, SpawnPoint, WorldInfo};
use dc_net_engine::sim::{Faults, SimChannel};

const ANNOUNCE: &str = "Worlds\\Synth.dat";
const RED: [f32; 3] = [-300.0, 0.0, -100.0];
const BLUE: [f32; 3] = [300.0, 0.0, 100.0];

fn world() -> WorldInfo {
    let mut w = world_from_bytes(&synthetic_world_bytes(), ANNOUNCE, "Synth").unwrap();
    w.spawns = vec![
        SpawnPoint {
            pos: RED,
            rot: [0.0, 1.5, 0.0],
            team_filter: Some(b"Red".to_vec()),
        },
        SpawnPoint {
            pos: BLUE,
            rot: [0.0, -1.5, 0.0],
            team_filter: Some(b"Blue".to_vec()),
        },
    ];
    w.spawn_modes = vec![None, None];
    w
}

fn tables() -> RomTables {
    RomTables {
        teams: TeamNames {
            names: [Some(b"red".to_vec()), Some(b"BLUE".to_vec())],
        },
        ..RomTables::default()
    }
}

struct Peer {
    addr: u32,
    client: SynthClient,
    up: SimChannel,
    down: SimChannel,
}

struct Net {
    server: Server<u32>,
    peers: Vec<Peer>,
    pipe: Vec<(u32, String)>,
    logs: Vec<String>,
    now: u32,
}

impl Net {
    fn with(cfg: ServerTuning, world: WorldInfo, room: &str, users: &[&str]) -> Net {
        let mut server = Server::with_tables(cfg, world, tables(), 0);
        assert!(server.handle_line(0, room));
        for u in users {
            assert!(server.handle_line(0, u));
        }
        Net {
            server,
            peers: Vec::new(),
            pipe: Vec::new(),
            logs: Vec::new(),
            now: 0,
        }
    }

    fn add(&mut self, addr: u32, name: &str) {
        let mut cfg = SynthConfig::named(name);
        cfg.box_bounds = Some((
            self.server.world().extent_min,
            self.server.world().extent_max,
        ));
        cfg.walk_step = 0.0;
        cfg.mode_class_hint = Some(1);
        self.peers.push(Peer {
            addr,
            client: SynthClient::new(cfg),
            up: SimChannel::new(addr as u64 * 2 + 1, Faults::clean(2)),
            down: SimChannel::new(addr as u64 * 2 + 2, Faults::clean(2)),
        });
    }

    fn step(&mut self) {
        let now = self.now;
        for p in &mut self.peers {
            for d in p.client.poll(now) {
                p.up.send(now, d);
            }
        }
        for i in 0..self.peers.len() {
            let addr = self.peers[i].addr;
            for d in self.peers[i].up.deliver(now) {
                self.server.handle_datagram(now, addr, &d);
            }
        }
        for o in self.server.poll(now) {
            match o {
                Output::Transmit { to, bytes } => {
                    if let Some(p) = self.peers.iter_mut().find(|p| p.addr == to) {
                        p.down.send(now, bytes);
                    }
                }
                Output::Pipe(l) => self.pipe.push((now, l)),
                Output::Log(l) => self.logs.push(l),
            }
        }
        for p in &mut self.peers {
            for d in p.down.deliver(now) {
                p.client.handle_datagram(now, &d);
            }
        }
        self.now += 5;
    }

    fn run_until(&mut self, limit: u32, mut done: impl FnMut(&Net) -> bool) -> bool {
        while self.now < limit {
            self.step();
            if done(self) {
                return true;
            }
        }
        false
    }

    fn run_for(&mut self, ms: u32) {
        let end = self.now + ms;
        self.run_until(end, |_| false);
    }

    fn c(&self, i: usize) -> &SynthClient {
        &self.peers[i].client
    }

    fn cm(&mut self, i: usize) -> &mut SynthClient {
        &mut self.peers[i].client
    }

    fn alive(&mut self) {
        let n = self.peers.len();
        assert!(self.run_until(20_000, |net| (0..n).all(|i| net.c(i).life_state() == 1)));
        self.run_for(300);
    }
}

const ROOM: &str = "room key=TDM type=1 goal=9";
const DAVE: &str = "user 7 3 99 0 120 Dave";
const ERIN: &str = "user 8 4 55 1 0 Big_Erin";

/// DSV-51 (P10): a leave (0xbd) followed by the client's `C2S_BYE` closes the link with code 5, never with the plain
/// bye first; a leave without a bye is closed with code 5 after 1000 ms, not before. The leaver leaves the world at
/// once in both cases (the other player sees it go).
#[test]
fn exit_waits_for_the_bye_and_closes_with_code_5() {
    let mut net = Net::with(ServerTuning::default(), world(), ROOM, &[DAVE, ERIN]);
    net.add(1, "Dave");
    net.add(2, "Big Erin");
    net.alive();
    net.cm(1).leave_match();
    net.run_for(300);
    assert_eq!(net.c(1).state(), ClientState::Dropped(5));
    assert!(!net.c(0).seen.players_left.is_empty() || !net.c(0).seen.removed.is_empty());

    let mut net = Net::with(ServerTuning::default(), world(), ROOM, &[DAVE, ERIN]);
    net.add(1, "Dave");
    net.add(2, "Big Erin");
    net.alive();
    let t0 = net.now;
    net.cm(1).leave_match_only();
    net.run_for(300);
    // out of the world at once: the other client got the removal of its objects
    assert!(!net.c(0).seen.removed.is_empty(), "removal at the leave");
    assert!(net.pipe.iter().any(|p| p.1 == "@leave 1"));
    net.run_until(t0 + 950, |_| false);
    assert!(
        !matches!(net.c(1).state(), ClientState::Dropped(_)),
        "no bye before the wait"
    );
    assert!(net.run_until(t0 + 1400, |n| matches!(
        n.c(1).state(),
        ClientState::Dropped(_)
    )));
    assert_eq!(net.c(1).state(), ClientState::Dropped(5));
    assert!(net.logs.iter().any(|l| l.contains("no bye within 1000 ms")));
}

/// DSV-49 (P9): after every spawn set the melee and throwable slots (2 and 3) get 0x66 kind 1 with the granted
/// weapon, ammunition kind and rounds and `extra` 1; the gun slots get none.
#[test]
fn slot_status_follows_each_spawn_set() {
    let dave = "user 7 3 99 0 120 Dave loadout=0:5,1:6,2:7,3:8";
    let mut net = Net::with(ServerTuning::default(), world(), ROOM, &[dave]);
    net.add(1, "Dave");
    net.alive();
    let notes = &net.c(0).seen.ammo_notes;
    let slots: Vec<(u16, u8, u16)> = notes.iter().map(|n| (n.1, n.4, n.5)).collect();
    assert_eq!(slots, vec![(7, 2, 1), (8, 3, 1)], "{notes:?}");
    // the rounds equal the grant's (0xb2) rounds of the same slot
    assert!(net.c(0).seen.weapons.iter().any(|w| w.1 == 7));
    // the amount is what the client was told (clip + stock): never 0 for a granted weapon (aa-grenades verify: a lone
    // grenade with clip 1 / stock 0 must read 1.0 here, as the reference stack sent; tdm4 seq 881, tdm6 seq 763)
    for n in notes {
        assert!(n.3 > 0.0, "slot {} amount {} in {notes:?}", n.4, n.3);
    }
}

/// DSV-50 (P7): the round's first spawn gets 0x45 with the intro from the room line (caption, mission text, map and
/// radar pictures); the synthetic client reads the whole block (no parse error).
#[test]
fn match_intro_from_the_room_line() {
    let room = "room key=TDM type=1 goal=9 obj=Cap.dtx cmt=Text.dtx|Text.dtx| \
                large=MapA.dtx|MapB.dtx|0.1,0,-0.1|0.4,0,0.3 radar=Radar.dtx|0.2,0,0.2|0.5,0,0.4|0.0|0.34|1024|1024";
    let mut net = Net::with(ServerTuning::default(), world(), room, &[DAVE]);
    net.add(1, "Dave");
    net.alive();
    let s = &net.c(0).seen;
    assert!(s.errors.is_empty(), "{:?}", s.errors);
    let i = s.intro.clone().expect("intro block");
    assert_eq!(i[1], b"Cap.dtx");
    assert_eq!(i[2], b"MapA.dtx");
    assert_eq!(i[3], b"MapB.dtx");
    assert_eq!(i[4], b"Radar.dtx");
    // `-nointro`: no block
    let cfg = ServerTuning {
        game: GameTuning {
            show_intro: false,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(cfg, world(), room, &[DAVE]);
    net.add(1, "Dave");
    net.alive();
    assert!(net.c(0).seen.intro.is_none());
    assert!(net.c(0).seen.match_begin.is_some());
}

/// DSV-47 (P3 / P4): the operator's `-evar` variables go out before `S2C_LOAD_WORLD`; a world without a
/// world-settings object sends no world variable; `-evars off` sends none of the world's.
#[test]
fn world_variables_before_the_world_load() {
    let mut w = world();
    w.world_vars = vec![dc_dserver::evars::WorldVar {
        label: "test",
        key: (dc_world::hash16(b"Alpha"), 5),
        name: Some(b"Alpha".to_vec()),
        value: b"1.500000".to_vec(),
    }];
    let cfg = ServerTuning {
        extra_evars: vec![("Beta".into(), "7".into())],
        ..ServerTuning::default()
    };
    let mut net = Net::with(cfg, w.clone(), ROOM, &[DAVE]);
    net.add(1, "Dave");
    net.alive();
    let ev: Vec<(Vec<u8>, Vec<u8>)> = net
        .c(0)
        .seen
        .evars
        .iter()
        .map(|e| (e.1.clone(), e.2.clone()))
        .collect();
    assert_eq!(
        ev,
        vec![
            (b"Alpha".to_vec(), b"1.500000".to_vec()),
            (b"Beta".to_vec(), b"7".to_vec())
        ]
    );
    let cfg = ServerTuning {
        evars: dc_dserver::evars::EvarMode::Off,
        ..ServerTuning::default()
    };
    let mut net = Net::with(cfg, w, ROOM, &[DAVE]);
    net.add(1, "Dave");
    net.alive();
    assert!(net.c(0).seen.evars.is_empty());
    let mut net = Net::with(ServerTuning::default(), world(), ROOM, &[DAVE]);
    net.add(1, "Dave");
    net.alive();
    assert!(
        net.c(0).seen.evars.is_empty(),
        "the synthetic world has no world settings"
    );
}

/// A kill volume of made-up name around a spawn point, and its world-file volume record.
fn world_with_kill_volume(around: [f32; 3], pending_ms: u32) -> WorldInfo {
    let mut w = world();
    let lo = [around[0] - 40.0, -60.0, around[2] - 40.0];
    let hi = [around[0] + 40.0, 200.0, around[2] + 40.0];
    w.kill_shapes
        .push(dc_dserver::killvol::box_shape(b"Pit", lo, hi, pending_ms));
    w.volumes.push(dc_dserver::world::VolumeInfo {
        name: b"Pit".to_vec(),
        kind: dc_dserver::world::VolumeKind::KillVolume,
        pos: around,
        fog_enable: false,
        fog_far: 0.0,
        fog_near: 0.0,
        fog_rgb: [0.0; 3],
        tint: [255.0; 3],
        light_add: [0.0; 3],
        movement_sounds: true,
        current: [0.0; 3],
        viscosity: 0.0,
        friction: 1.0,
        pending: pending_ms > 0,
    });
    w
}

/// DSV-52 (P2): a living player whose position is inside a created kill volume dies with no killer (0x6d killer
/// 0xff to everybody, a death counted, no kill), after the volume's pending time; `-killvolumes off` keeps it alive.
#[test]
fn kill_volume_kills_without_a_killer() {
    let mut net = Net::with(
        ServerTuning::default(),
        world_with_kill_volume(BLUE, 0),
        ROOM,
        &[DAVE, ERIN],
    );
    net.add(1, "Dave");
    net.add(2, "Big Erin");
    // Erin (team 2) spawns in the volume and dies at once
    assert!(net.run_until(20_000, |n| !n.c(0).seen.kills.is_empty()));
    let k = &net.c(0).seen.kills[0];
    assert_eq!(k.killer, 0xff, "no killer");
    assert_eq!(k.victim as u32, net.c(1).seen.client_id.unwrap().0 as u32);
    assert_eq!(net.server.active_kill_volumes(), &[0]);
    assert!(net.logs.iter().any(|l| l.contains("died in a kill volume")));

    // pending time: no death before it
    let mut net = Net::with(
        ServerTuning::default(),
        world_with_kill_volume(BLUE, 2000),
        ROOM,
        &[DAVE, ERIN],
    );
    net.add(1, "Dave");
    net.add(2, "Big Erin");
    assert!(net.run_until(20_000, |n| n
        .logs
        .iter()
        .any(|l| l.contains("entered kill volume"))));
    let t_in = net.now;
    assert!(net.run_until(t_in + 4000, |n| !n.c(0).seen.kills.is_empty()));
    assert!(net.now >= t_in + 1990, "died after {} ms", net.now - t_in);

    let cfg = ServerTuning {
        kill_volumes: false,
        ..ServerTuning::default()
    };
    let mut net = Net::with(cfg, world_with_kill_volume(BLUE, 0), ROOM, &[DAVE, ERIN]);
    net.add(1, "Dave");
    net.add(2, "Big Erin");
    net.alive();
    net.run_for(2000);
    assert!(net.c(0).seen.kills.is_empty());
}

/// A sliding door of made-up name next to the red spawn point: its model box, move direction and distance.
fn world_with_door(near: [f32; 3]) -> WorldInfo {
    let mut w = world();
    let closed = [near[0] + 60.0, 50.0, near[2]];
    w.brushes.push(dc_dserver::world::BrushInfo {
        name: b"Door1".to_vec(),
        role: None,
        pos: closed,
        rot: [0.0; 3],
        visible: true,
        solid: true,
        rayhit: true,
        blend: 0,
        fog_disable: false,
        class: dc_dserver::world::BrushClass::Door,
        sky: None,
    });
    w.doors.push(dc_dserver::doors::DoorInfo {
        link: Vec::new(),
        locked: false,
        name: b"Door1".to_vec(),
        closed,
        lo: [closed[0] - 1.0, 0.0, closed[2] - 30.0],
        hi: [closed[0] + 1.0, 100.0, closed[2] + 30.0],
        dir: [0.0, 0.0, -1.0],
        dist: 59.0,
        open_ms: 1000,
        close_ms: 1000,
        open_by: dc_dserver::doors::OpenBy::Near,
        cmds: Default::default(),
        sounds: Default::default(),
        usable: true,
    });
    w
}

/// DSV-46 (doors slice 1): the door is created closed (kind 2 with the type-0x39 blob, full precision) and opens
/// when a living player is near: the client sees change records move it by its distance along its direction within
/// about its opening time; with nobody near (the player left) it closes 3 s later. `-doors off`: no door object.
#[test]
fn door_opens_near_a_player_and_closes_after_it_left() {
    // DSV-302: the door's opening / closing sounds
    let mut w = world_with_door(RED);
    w.doors[0].sounds = dc_dserver::doors::DoorSounds {
        opening: "SND/door_open.wav".into(),
        closing: "SND/door_close.wav".into(),
        offset: [0.0; 3],
        radius: 1000.0,
        locked: String::new(),
    };
    let mut net = Net::with(ServerTuning::default(), w, ROOM, &[DAVE]);
    net.add(1, "Dave");
    assert!(net.run_until(20_000, |n| n.c(0).seen.view_object.is_some()));
    let door = net.server.doors()[0].obj;
    let o = net.c(0).seen.objects[&door].clone();
    assert_eq!(o.kind, 2);
    assert_eq!(o.blob_type, Some(0x39));
    assert_eq!(o.flags, Some(0x3101));
    // DSV-340: the blob is the activate entry: row 0, enabled, state off (closed)
    let mut br = o.blob.as_ref().expect("blob").reader();
    assert_eq!(
        (br.read_u8(), br.read_u8(), br.read_bit(), br.read_u8()),
        (0x39, 0, false, 1)
    );
    let closed = o.pos.expect("create position");
    net.alive();
    // Dave stands at the red point, 60 units from the door: it opens
    assert!(net.run_until(net.now + 3000, |n| n.c(0).seen.objects[&door]
        .pos
        .is_some_and(|p| (p[2] - (closed[2] - 59.0)).abs() < 0.01)));
    let records = net.c(0).seen.objects[&door].records;
    assert!(records >= 5, "{records} door records");
    assert_eq!(
        net.c(0).seen.sound_starts.len(),
        1,
        "one opening sound: {:?}",
        net.c(0).seen.sound_starts
    );
    assert!(
        net.c(0).seen.errors.is_empty(),
        "{:?}",
        net.c(0).seen.errors
    );
    // the player leaves: the door closes 3 s after the last near step
    net.cm(0).leave_match_only();
    let t_left = net.now;
    net.run_for(100);
    assert_eq!(net.server.doors()[0].offset, 59.0);
    net.run_until(t_left + 2800, |_| false);
    assert_eq!(net.server.doors()[0].offset, 59.0, "still open");
    net.run_until(t_left + 5000, |_| false);
    assert_eq!(net.server.doors()[0].offset, 0.0, "closed again");
    // DSV-340: the state note at the start of the opening (on) went to the client; the closing note (off) went to
    // the clients in the world then (Dave left), the server's mark follows
    let notes: Vec<(u16, u8, u8)> = net
        .c(0)
        .seen
        .door_notes
        .iter()
        .map(|n| (n.1, n.2, n.3))
        .collect();
    assert_eq!(notes.first(), Some(&(door, 2, 0)), "{notes:?}");
    assert_eq!(net.server.door_marks().get(&door), Some(&(false, 1)));

    let cfg = ServerTuning {
        doors: false,
        ..ServerTuning::default()
    };
    let mut net = Net::with(cfg, world_with_door(RED), ROOM, &[DAVE]);
    net.add(1, "Dave");
    net.alive();
    assert!(net.server.doors().is_empty());
    assert!(!net
        .c(0)
        .seen
        .objects
        .values()
        .any(|o| o.blob_type == Some(0x39)));
}

/// DSV-270: a door the world does not mark player-activated (a garage shutter switched on by a lever) stays shut with
/// a living player next to it; as a use-key door it stays shut until the player presses the use key (`UP_USE` with
/// no object), then opens; a second press closes it.
#[test]
fn a_message_door_ignores_a_near_player_and_a_use_door_opens_on_the_use_key() {
    use dc_dserver::doors::OpenBy;
    let mut w = world_with_door(RED);
    w.doors[0].open_by = OpenBy::Message;
    w.doors[0].usable = false;
    let mut net = Net::with(ServerTuning::default(), w, ROOM, &[DAVE]);
    net.add(1, "Dave");
    assert!(net.run_until(20_000, |n| n.c(0).seen.view_object.is_some()));
    // DSV-340: a door players may not use is created with its activate entry disabled
    let door = net.server.doors()[0].obj;
    let mut br = net.c(0).seen.objects[&door].blob.as_ref().expect("blob").reader();
    br.read_u16();
    assert!(br.read_bit(), "disabled");
    net.alive();
    net.run_for(3000);
    assert_eq!(
        net.server.doors()[0].offset,
        0.0,
        "walking up does not open it"
    );
    let use_key = |net: &mut Net| {
        let mut b = dc_wire::udp::BitWriter::new();
        b.write_u16(0xffff);
        b.write_u8(0);
        net.cm(0).send_raw_shell(0x59, &b);
    };
    use_key(&mut net);
    net.run_for(2000);
    assert_eq!(
        net.server.doors()[0].offset,
        0.0,
        "no use key for a message door"
    );

    let mut w = world_with_door(RED);
    w.doors[0].open_by = OpenBy::Use;
    let mut net = Net::with(ServerTuning::default(), w, ROOM, &[DAVE]);
    net.add(1, "Dave");
    assert!(net.run_until(20_000, |n| n.c(0).seen.view_object.is_some()));
    net.alive();
    net.run_for(3000);
    assert_eq!(net.server.doors()[0].offset, 0.0);
    use_key(&mut net);
    net.run_for(2000);
    assert_eq!(net.server.doors()[0].offset, 59.0, "opened by the use key");
    use_key(&mut net);
    net.run_for(2000);
    assert_eq!(
        net.server.doors()[0].offset,
        0.0,
        "closed by the next press"
    );
    // DSV-340: each press turned the door's activate state (on, then off) by an object note
    let door = net.server.doors()[0].obj;
    let notes: Vec<(u16, u8, u8)> = net
        .c(0)
        .seen
        .door_notes
        .iter()
        .map(|n| (n.1, n.2, n.3))
        .collect();
    assert_eq!(notes, vec![(door, 2, 0), (door, 2, 1)]);
    assert!(net.c(0).seen.errors.is_empty(), "{:?}", net.c(0).seen.errors);
}

/// w1-doors (DOOR-4, DOOR-5): a rotating door next to the red spawn point turns open near a living player (its object
/// goes around the hinge with position and rotation records), a rotating switch is created and never moves, and a
/// client that joins while the door is open gets its turned pose after the create.
#[test]
fn rotating_door_turns_open_near_a_player_and_a_joiner_sees_it_open() {
    use dc_dserver::doors::{TurnInfo, TurnKind};
    use dc_dserver::world::{BrushClass, BrushInfo};
    let mut w = world();
    let hinge = [RED[0] + 60.0, 50.0, RED[2] - 30.0];
    let origin = [hinge[0], 50.0, hinge[2] + 30.0];
    let sw = [BLUE[0], 50.0, BLUE[2]];
    for (name, class, pos) in [
        (b"Turn1".to_vec(), BrushClass::Door, origin),
        (b"Switch1".to_vec(), BrushClass::Switch, sw),
    ] {
        w.brushes.push(BrushInfo {
            name,
            role: None,
            pos,
            rot: [0.0; 3],
            visible: true,
            solid: true,
            rayhit: true,
            blend: 0,
            fog_disable: false,
            class,
            sky: None,
        });
    }
    let turn = TurnInfo {
        link: Vec::new(),
        locked: false,
        name: b"Turn1".to_vec(),
        kind: TurnKind::Door,
        origin,
        lo: [hinge[0] - 2.0, 0.0, hinge[2]],
        hi: [hinge[0] + 2.0, 100.0, hinge[2] + 60.0],
        pivot: hinge,
        axis: [0.0, 1.0, 0.0],
        angle: std::f32::consts::FRAC_PI_2,
        open_ms: 1000,
        close_ms: 1000,
        away: false,
        start_open: false,
        open_by: dc_dserver::doors::OpenBy::Near,
        cmds: Default::default(),
        sounds: Default::default(),
    };
    w.turns.push(turn.clone());
    w.turns.push(TurnInfo {
        name: b"Switch1".to_vec(),
        kind: TurnKind::Switch,
        origin: sw,
        lo: [sw[0] - 2.0, 40.0, sw[2] - 2.0],
        hi: [sw[0] + 2.0, 60.0, sw[2] + 2.0],
        pivot: sw,
        ..turn
    });
    let mut net = Net::with(ServerTuning::default(), w, ROOM, &[DAVE, ERIN]);
    net.add(1, "Dave");
    assert!(net.run_until(20_000, |n| n.c(0).seen.view_object.is_some()));
    assert_eq!(net.server.turns().len(), 2);
    let (door, switch) = (net.server.turns()[0].obj, net.server.turns()[1].obj);
    assert_eq!(net.c(0).seen.objects[&door].kind, 2);
    assert_eq!(net.c(0).seen.objects[&switch].kind, 2);
    net.alive();
    // 90 degrees about +y: the panel middle goes from 30 along +z of the hinge to 30 along +x
    let open =
        |p: [f32; 3]| (p[0] - (hinge[0] + 30.0)).abs() < 0.05 && (p[2] - hinge[2]).abs() < 0.05;
    assert!(net.run_until(net.now + 3000, |n| n.c(0).seen.objects[&door]
        .pos
        .is_some_and(open)));
    assert_eq!(
        net.c(0).seen.objects[&door].last_cf & 0x0006,
        0x0006,
        "position and rotation"
    );
    assert!(
        net.c(0).seen.errors.is_empty(),
        "{:?}",
        net.c(0).seen.errors
    );
    assert_eq!(
        net.c(0).seen.objects[&switch].records,
        1,
        "the switch only has its create"
    );
    // a client joining now: the create at the model origin, then the turned pose
    net.add(2, "Big Erin");
    assert!(net.run_until(net.now + 20_000, |n| n
        .c(1)
        .seen
        .objects
        .get(&door)
        .is_some_and(|o| o.pos.is_some_and(open))));
    assert!(
        net.c(1).seen.errors.is_empty(),
        "{:?}",
        net.c(1).seen.errors
    );
}

/// MAP-20 / DSV-52 on a community map: the `dc.kill_volume` brush read through the vocabulary (its `pending_sec`)
/// kills a player who spawns inside it, with no killer, after its pending time. The synthetic `dc.*` map's team-2
/// deathmatch points are moved into its pit for the test.
#[test]
fn dc_kill_volume_kills_on_a_community_map() {
    let mut w = world_from_bytes(
        &dc_dserver::world::synthetic_dc_world_bytes(),
        "@mod:test/assets/maps/synth_dc.dat",
        "dc map",
    )
    .unwrap();
    assert_eq!(w.kill_shapes[0].pending_ms, 1500);
    for s in w
        .spawns
        .iter_mut()
        .filter(|s| s.team_filter.as_deref() == Some(b"2"))
    {
        s.pos = [500.0, 40.0, 500.0];
    }
    let mut net = Net::with(ServerTuning::default(), w, ROOM, &[DAVE, ERIN]);
    net.add(1, "Dave");
    net.add(2, "Big Erin");
    assert!(net.run_until(20_000, |n| n
        .logs
        .iter()
        .any(|l| l.contains("entered kill volume"))));
    let t_in = net.now;
    assert!(net.run_until(t_in + 4000, |n| !n.c(0).seen.kills.is_empty()));
    let k = &net.c(0).seen.kills[0];
    assert_eq!(k.killer, 0xff, "no killer");
    assert!(net.now >= t_in + 1490, "died after {} ms", net.now - t_in);
    assert!(net.logs.iter().any(|l| l.contains("died in a kill volume")));
}

/// A rotating door of the world file near the red point, turned only by messages (DSV-500).
fn turn_door(name: &[u8], at: [f32; 3]) -> (dc_dserver::world::BrushInfo, dc_dserver::doors::TurnInfo) {
    let brush = dc_dserver::world::BrushInfo {
        name: name.to_vec(),
        role: None,
        pos: at,
        rot: [0.0; 3],
        visible: true,
        solid: true,
        rayhit: true,
        blend: 0,
        fog_disable: false,
        class: dc_dserver::world::BrushClass::Door,
        sky: None,
    };
    let h = std::f32::consts::FRAC_PI_2;
    let info = dc_dserver::doors::TurnInfo {
        name: name.to_vec(),
        kind: dc_dserver::doors::TurnKind::Door,
        origin: at,
        lo: [at[0] - 2.0, 0.0, at[2]],
        hi: [at[0] + 2.0, 100.0, at[2] + 60.0],
        pivot: [at[0], 50.0, at[2]],
        axis: [0.0, 1.0, 0.0],
        angle: h,
        open_ms: 500,
        close_ms: 500,
        away: false,
        start_open: false,
        open_by: dc_dserver::doors::OpenBy::Message,
        cmds: Default::default(),
        sounds: Default::default(),
        link: Vec::new(),
        locked: false,
    };
    (brush, info)
}

/// DSV-500 / DSV-501 (playtest report: WarHead's spawn fence had a gap that killed whoever walked through; the gap is
/// a rotating door the players do not open, in front of a fall volume, opened only by the quarantine mode's start
/// trigger): a message door is created closed, stays closed with a player right next to it and offers no use prompt;
/// the room's start trigger `on` creates it open; a door the start scripts make invisible and not solid is not
/// created.
#[test]
fn a_message_door_stands_closed_unless_the_start_scripts_open_it() {
    let mut w = world();
    let (gb, gi) = turn_door(b"Gate", [RED[0] + 40.0, 50.0, RED[2]]);
    let (hb, hi) = turn_door(b"Hatch", [RED[0] - 40.0, 50.0, RED[2]]);
    w.brushes.extend([gb, hb]);
    w.turns.extend([gi, hi]);
    w.scripts.triggers.push((
        b"QuarantineStart".to_vec(),
        vec![b"msg Gate on;".to_vec()],
    ));
    w.scripts.triggers.push((
        b"PlainStart".to_vec(),
        vec![b"msg Hatch (visible 0);\r\nmsg Hatch (solid 0);".to_vec()],
    ));
    let gate = |net: &Net| {
        net.server
            .turns()
            .iter()
            .find(|t| t.info.name == b"Gate")
            .cloned()
    };
    // the plain room: the gate is a closed wall, the hatch is gone
    let room = "room key=TDM type=1 goal=9 trig=|PlainStart||";
    let mut net = Net::with(ServerTuning::default(), w.clone(), room, &[DAVE]);
    net.add(1, "Dave");
    assert!(net.run_until(20_000, |n| n.c(0).seen.view_object.is_some()));
    net.alive();
    net.run_for(3000);
    let g = gate(&net).expect("the gate is created");
    assert_eq!(g.frac, 0.0, "nobody opens it by walking up");
    assert_eq!(g.activate().map(|a| a.0), Some(true), "no use prompt");
    assert!(net.c(0).seen.objects.contains_key(&g.obj));
    assert!(
        !net.server.turns().iter().any(|t| t.info.name == b"Hatch"),
        "the scripts removed the hatch"
    );
    // the quarantine room: its start trigger opens the gate from the start
    let room = "room key=TDM type=1 goal=9 trig=|QuarantineStart||";
    let mut net = Net::with(ServerTuning::default(), w, room, &[DAVE]);
    net.add(1, "Dave");
    assert!(net.run_until(20_000, |n| n.c(0).seen.view_object.is_some()));
    let g = gate(&net).expect("the gate is created");
    assert_eq!(g.frac, 1.0, "open from the start");
    assert!(net.server.turns().iter().any(|t| t.info.name == b"Hatch"));
}
