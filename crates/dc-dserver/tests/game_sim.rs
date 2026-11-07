//! Slice 2, deterministic: the server and synthetic clients over the simulated channel of `dc-net-engine` with an
//! injected clock. S1-S12 to player state 1, the spawn order, two teams, the movement relay through the object
//! stream, hits, kills and the respawn timeline, the round end by goal and by timer, chat, the ping table and the
//! tick rates. The world is the synthetic one with made-up spawn points and team names (no ROM content).

use dc_dserver::game::hits::{HitCounters, HitVerdict};
use dc_dserver::game::{GameTuning, StartOrder, SPAWN_NOTE_MS};
use dc_dserver::rom_tables::{RomTables, TeamNames};
use dc_dserver::session::{Output, Server, ServerTuning, Step};
use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig, View};
use dc_dserver::world::{synthetic_world_bytes, world_from_bytes, SpawnPoint, WorldInfo};
use dc_net_engine::sim::{Faults, SimChannel};
use dc_wire::udp::BitWriter;

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
    /// One-way delay of every peer channel added from now on.
    delay_ms: u32,
    /// The look bytes of the clients added from now on (`None`: the synthetic client's default).
    look: Option<[i8; 15]>,
}

impl Net {
    /// The slice-2 tests check the spec order (`-startorder spec`); slice 3 tests use [`Net::with`].
    fn new(mut cfg: ServerTuning, room: &str) -> Net {
        cfg.game.start_order = StartOrder::Spec;
        Net::with(cfg, tables(), room)
    }

    fn with(cfg: ServerTuning, tables: RomTables, room: &str) -> Net {
        Net::with_world(cfg, world(), tables, room)
    }

    fn with_world(cfg: ServerTuning, world: WorldInfo, tables: RomTables, room: &str) -> Net {
        let mut server = Server::with_tables(cfg, world, tables, 0);
        assert!(server.handle_line(0, room));
        assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
        assert!(server.handle_line(0, "user 8 4 55 1 0 Big_Erin"));
        Net {
            server,
            peers: Vec::new(),
            pipe: Vec::new(),
            logs: Vec::new(),
            now: 0,
            delay_ms: 2,
            look: None,
        }
    }

    fn add(&mut self, addr: u32, name: &str, walk: f32) {
        self.add_with(addr, name, walk, None);
    }

    /// [`Net::add`] with a fixed control word in every player update.
    fn add_with(&mut self, addr: u32, name: &str, walk: f32, control: Option<u32>) {
        let mut cfg = SynthConfig::named(name);
        cfg.control = control;
        cfg.box_bounds = Some((
            self.server.world().extent_min,
            self.server.world().extent_max,
        ));
        cfg.walk_step = walk;
        // the rooms of these tests are TDM (`type=1`); the server sends no 0x9e in the spec order
        cfg.mode_class_hint = Some(1);
        cfg.respawn_wait_ms = self.server.config().game.respawn_wait_ms;
        if let Some(l) = self.look {
            cfg.look = l;
        }
        self.peers.push(Peer {
            addr,
            client: SynthClient::new(cfg),
            up: SimChannel::new(addr as u64 * 2 + 1, Faults::clean(self.delay_ms)),
            down: SimChannel::new(addr as u64 * 2 + 2, Faults::clean(self.delay_ms)),
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
                Output::Log(l) => {
                    assert!(!l.starts_with('@'));
                    self.logs.push(l)
                }
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

    fn pipe_lines(&self) -> Vec<String> {
        self.pipe.iter().map(|p| p.1.clone()).collect()
    }

    fn obj(&self, i: usize) -> u16 {
        self.c(i).seen.view_object.expect("view object")
    }

    fn pid(&self, i: usize) -> u8 {
        self.c(i).seen.client_id.expect("client id").0 as u8
    }

    /// Both clients alive (state 1).
    fn two_alive(cfg: ServerTuning, room: &str, walk: [f32; 2]) -> Net {
        let mut net = Net::new(cfg, room);
        net.add(1, "Dave", walk[0]);
        net.add(2, "Big Erin", walk[1]);
        assert!(
            net.run_until(20_000, |n| n.c(0).life_state() == 1
                && n.c(1).life_state() == 1),
            "states {} {}",
            n_state(&net, 0),
            n_state(&net, 1)
        );
        // let the stream carry both positions
        net.run_for(500);
        net
    }

    /// Fires at client `to` from client `from` until `n` pellets were sent (one per step).
    fn shoot(&mut self, from: usize, to: usize, n: u32, offset: [f32; 3]) {
        let target = self.obj(to);
        for _ in 0..n {
            assert!(
                self.cm(from).fire_at(target, offset, true),
                "target not in the stream"
            );
            self.run_for(20);
        }
    }
}

/// Synthetic character tables in the ROM key shape (made-up names) and a rig with the four player clips.
fn character_tables() -> RomTables {
    use dc_dserver::chars::{BodyRow, CharTables, FaceRow};
    let mut t = tables();
    let doc = dc_bute::ButeDoc::parse(concat!(
        "[Weapon3]\nName = \"G3\"\nHHModel = \"HH\\\\g3.ltb\"\nHHSkin0 = \"HH\\\\g3.dtx\"\n",
        "HHRenderStyle0 = \"RS\\\\a.ltb\"\n",
        "[Weapon4]\nName = \"G4\"\nHHModel = \"HH\\\\g4.ltb\"\nHHSkin0 = \"HH\\\\g4.dtx\"\n",
        "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"G4\"\nP1 = \"G3\"\n",
    ));
    t.weapons = dc_dserver::rom_tables::WeaponTable::from_doc(&doc);
    let mut chars = CharTables::default();
    chars.bodies.insert(
        0,
        BodyRow {
            model: "BODY\\b.ltb".into(),
            styles: vec!["RS\\a.ltb".into(), "RS\\b.ltb".into(), "RS\\c.ltb".into()],
            skins: [(0, (Some("BODY\\u.dtx".into()), Some("BODY\\l.dtx".into())))].into(),
            ..BodyRow::default()
        },
    );
    chars.faces.insert(
        0,
        FaceRow {
            model: "FACE\\f.ltb".into(),
            skins: vec!["FACE\\f.dtx".into()],
            styles: vec!["RS\\a.ltb".into(), "RS\\a.ltb".into()],
        },
    );
    chars.team_marks.insert(0, "BODY\\m.dtx".into());
    t.chars = chars;
    t
}

fn rig() -> dc_dserver::chars::Rig {
    use dc_dserver::chars::{
        ClipFact, Rig, CLIP_DEATH_BODY_LEFT, CLIP_DEATH_BODY_RIGHT, CLIP_DEATH_HEAD_LEFT,
        CLIP_DEATH_HEAD_RIGHT, CLIP_LOWER_STAND, CLIP_LOWER_WALK, CLIP_UPPER_STAND,
        CLIP_UPPER_WALK,
    };
    let c = |key, len_ms| ClipFact {
        key,
        len_ms,
        extent: [15.0, 50.0, 15.0],
        footsteps: 0,
        upper: None,
    };
    Rig {
        children: vec!["BODY\\kid.ltb".into()],
        clips: vec![
            c(([1; 8], 4), 7200),
            c(CLIP_LOWER_STAND, 1000),
            c(CLIP_LOWER_WALK, 800),
            c(CLIP_UPPER_STAND, 1500),
            c(CLIP_UPPER_WALK, 900),
            c(CLIP_DEATH_BODY_LEFT, 1333),
            c(CLIP_DEATH_BODY_RIGHT, 1433),
            c(CLIP_DEATH_HEAD_LEFT, 1766),
            c(CLIP_DEATH_HEAD_RIGHT, 1233),
            // DSV-28 refinement: run forward / forward-left, crouched stand, crouch forward (indices 9..12)
            c(dc_dserver::chars::CLIP_LOWER_RUN[0], 600),
            c(dc_dserver::chars::CLIP_LOWER_RUN[4], 600),
            c(dc_dserver::chars::CLIP_LOWER_CROUCH_STAND, 1200),
            c(dc_dserver::chars::CLIP_LOWER_CROUCH[0], 1100),
        ],
        own_clips: 1,
        grip_socket: 2,
        sockets: Vec::new(),
        stand_extent: Some([15.0, 50.0, 15.0]),
        tracker_masks: [None; 2],
    }
}

/// DSV-28 refinement and DSV-23 change: the lower-body clip follows the reported control word (`RUN` + `FWD` -> the
/// run forward clip, `RUN` + `FWD` + `LEFT` -> run forward-left, `CROUCH` alone -> the crouched stand, `CROUCH` +
/// `FWD` -> crouch forward), and each client gets its own character's entries with the animation block; with
/// `stream_own` off (DSV-21) it gets none, and with `directional_anims` off every movement is the walk pair.
#[test]
fn control_word_picks_the_gait_clip_and_the_owner_gets_its_entries() {
    // (Dave's control, Erin's control, Dave's lower clip as Erin sees it, Erin's as Dave sees it)
    let cases: [(u32, u32, u16, u16, bool, bool); 4] = [
        (0x0a01, 0x0020, 9, 11, true, true),
        (0x0a81, 0x0821, 10, 12, true, true),
        (0x0a01, 0x0020, 2, 1, false, false),
        (0x0001, 0x0000, 2, 1, true, false),
    ];
    for (dave_c, erin_c, dave_clip, erin_clip, directional, own) in cases {
        let cfg = ServerTuning {
            game: GameTuning {
                default_loadout: vec![3, 4],
                directional_anims: directional,
                stream_own: own,
                ..GameTuning::default()
            },
            ..ServerTuning::default()
        };
        let mut net = Net::with(
            cfg,
            character_tables(),
            "room key=TDM type=1 goal=20 ff=0 time=0",
        );
        net.server.insert_rig("BODY\\b.ltb", rig());
        net.add_with(1, "Dave", 2.0, Some(dave_c));
        net.add_with(2, "Big Erin", 0.0, Some(erin_c));
        assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
            && n.c(1).life_state() == 1));
        net.run_for(1500);
        let label = format!("controls {dave_c:#x} / {erin_c:#x}");
        assert_eq!(
            net.c(1).seen.remote[&net.obj(0)].last_anim[2],
            dave_clip,
            "{label}: Dave's lower clip"
        );
        assert_eq!(
            net.c(0).seen.remote[&net.obj(1)].last_anim[2],
            erin_clip,
            "{label}: Erin's lower clip"
        );
        for me in 0..2 {
            let s = &net.c(me).seen;
            assert!(s.errors.is_empty(), "{label}: {:?}", s.errors);
            match s.remote.get(&net.obj(me)) {
                Some(r) => {
                    assert!(own, "{label}: own entries with stream_own off");
                    assert!(r.anim_entries > 5 && r.anim_entries == r.entries, "{label}");
                }
                None => assert!(!own, "{label}: client {me} got no own entry"),
            }
        }
        // the owner sees the same clip on its own object as the other client does
        if own {
            assert_eq!(net.c(0).seen.remote[&net.obj(0)].last_anim[2], dave_clip);
        }
    }
}

/// DSV-36 (reference order, the default): the link opens with 0x90 [1] / 0x88 [2]; the placement sends 0x72, 0x9e,
/// 0x9d [0], 0x7d; the round's first spawn gets 0x43, 0x44, 0xb5 [0][0] and 0x45, after 0x87 0x90 [0] / 0x88 [3]; the
/// joiner gets kind 2 for itself (dead) and for the others, the others its kind 0 (dead), then kind 2 (alive) after
/// its spawn set. A player announced after the round started (late join) gets 0x90 [0] / 0x88 [3] at the link and
/// not again, and 0x43 / 0x44 (remaining seconds) / 0xb5 without 0x45. Never 0x4a.
#[test]
fn reference_start_and_late_join_fidelity() {
    let mut net = Net::with(
        ServerTuning::default(),
        tables(),
        "room key=TDM type=1 goal=20 ff=0 time=120",
    );
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    let (p0, p1) = (net.pid(0), net.pid(1));
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
        let o = shell_order(net.c(i));
        assert_eq!(&o[..2], &[0x90, 0x88], "client {i}: {o:x?}");
        assert_eq!(s.flag90, vec![true, false]);
        assert_eq!(s.switch_states, vec![2, 3]);
        let p = o.iter().position(|&x| x == 0x72).unwrap();
        assert_eq!(&o[p..p + 4], &[0x72, 0x9e, 0x9d, 0x7d], "{o:x?}");
        assert_eq!(s.shell_9d, vec![0]);
        assert_eq!(s.endpoint, Some(1));
        let q = o.iter().position(|&x| x == 0x43).unwrap();
        assert_eq!(&o[q..q + 4], &[0x43, 0x44, 0xb5, 0x45], "{o:x?}");
        assert_eq!(s.team_kills.first().map(|t| (t.1, t.2)), Some((0, 0)));
        assert!(!o.contains(&0x4a), "never 0x4a");
        // DC-DEFAULT: DSV-20 (revision, proposed) the second 0x45 flag clear: the 2009 client would otherwise keep
        // its "waiting for players to connect" banner up for the whole round (Wave AA item 7)
        assert_eq!(s.match_begin.map(|m| m.1), Some(false), "client {i}");
    }
    // roster kinds: Erin (the second) got kind 2 for herself (dead) and for Dave; Dave got Erin's kind 0 (dead),
    // then kind 2 (alive)
    let erin = &net.c(1).seen.roster_log;
    assert_eq!((erin[0].1, erin[0].2, erin[0].3), (p1, 2, true));
    assert!(erin.iter().any(|r| r.1 == p0 && r.2 == 2));
    let dave: Vec<(u8, bool)> = net
        .c(0)
        .seen
        .roster_log
        .iter()
        .filter(|r| r.1 == p1)
        .map(|r| (r.2, r.3))
        .collect();
    assert_eq!(&dave[..2], &[(0, true), (2, false)], "{dave:?}");
    // a late joiner: announced after the round started
    let t = net.now;
    assert!(net.server.handle_line(t, "user 9 5 77 0 0 Carl"));
    net.add(3, "Carl", 0.0);
    assert!(net.run_until(t + 20_000, |n| n.c(2).life_state() == 1));
    net.run_for(500);
    let s = &net.c(2).seen;
    assert!(s.errors.is_empty(), "{:?}", s.errors);
    let o = shell_order(net.c(2));
    assert_eq!(&o[..2], &[0x90, 0x88], "{o:x?}");
    assert_eq!(s.flag90, vec![false], "0x90 [0] at the link, not again");
    assert_eq!(s.switch_states, vec![3]);
    assert!(
        !o.contains(&0x45),
        "no 0x45 for a joiner of a running round: {o:x?}"
    );
    let q = o.iter().position(|&x| x == 0x43).unwrap();
    assert_eq!(&o[q..q + 3], &[0x43, 0x44, 0xb5], "{o:x?}");
    let left = s.round_begin.expect("0x44");
    assert!((100..120).contains(&left), "remaining seconds {left}");
    assert!(net.c(2).seen.match_begin.is_none());
}

/// Slice 3 (reference order, synthetic character tables): every client sees the other player as a character object
/// in the reference stack's create form (kind 1, type-0x18 blob with the player's id, body + child + face models, the
/// weapon model attached at socket 2, dims), hidden until the spawn and visible after it; the stream carries yaw8
/// and the three trackers (standing pair for the stander, walking pair for the walker); the own body comes with
/// the mover update (body names, friction 5); a weapon change re-sends the hand model (CF_FILES); a second
/// reader (dc-botclient's object table) parses every object group without an error.
#[test]
fn characters_are_created_streamed_and_animated() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![3, 4],
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(
        cfg,
        character_tables(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
    );
    net.server.insert_rig("BODY\\b.ltb", rig());
    net.add(1, "Dave", 2.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(1500);
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
        // the files the records name were announced (single-entry lists after the world)
        assert!(
            s.files.len() >= 12, // world + 11 character and weapon files
            "client {i}: {} files",
            s.files.len()
        );
        // own mover body: the body model and the two skins, friction 5 in both blocks and after 0x87
        assert_eq!(s.bodies.len(), 1);
        assert_eq!(s.bodies[0].0, b"BODY\\B.LTB".to_vec());
        assert_eq!(
            s.bodies[0].1,
            vec![b"BODY\\U.DTX".to_vec(), b"BODY\\L.DTX".to_vec()]
        );
        assert_eq!(s.frictions, vec![5.0, 5.0, 5.0]);
        assert_eq!(
            s.weapon_switches,
            vec![(0x15, true)],
            "auto switch to weapon 3 by its command"
        );
    }
    let other = |me: usize| 1 - me;
    for me in 0..2 {
        let o = net.obj(other(me));
        let s = &net.c(me).seen;
        let ch = &s.objects[&o];
        assert_eq!(ch.kind, 1);
        assert_eq!(ch.blob_type, Some(0x18));
        assert_eq!(ch.blob_pid, Some(net.pid(other(me))));
        assert_eq!(ch.models.len(), 3, "body, child, face");
        assert_eq!(
            ch.dims,
            Some([15.0, 50.0, 15.0]),
            "the rig's stand box, not the configured default"
        );
        assert_eq!(ch.flags, Some(0x3803), "visible after the spawn");
        assert_eq!(ch.attached.len(), 1);
        let (wobj, socket) = ch.attached[0];
        assert_eq!(socket, 2);
        assert_eq!(s.objects[&wobj].kind, 1);
        assert_eq!(s.objects[&wobj].flags, Some(0x0003));
        let r = s.remote[&o];
        assert!(r.yaw8_entries > 5 && r.anim_entries > 5);
        assert_eq!(
            r.anim_entries, r.entries,
            "every entry carries the animation block"
        );
        // DSV-23 (change): the own character is streamed to its owner with the animation block (footsteps)
        let own = s.remote[&net.obj(me)];
        assert!(own.anim_entries > 5 && own.anim_entries == own.entries);
    }
    // Dave walks (FWD): Erin sees the walking pair; Erin stands: Dave sees the standing pair
    assert_eq!(net.c(1).seen.remote[&net.obj(0)].last_anim, [0, 4, 2]);
    assert_eq!(net.c(0).seen.remote[&net.obj(1)].last_anim, [0, 3, 1]);
    // Dave switches to his second weapon: Erin's copy of Dave's weapon object gets the other model
    let wobj = net.c(1).seen.objects[&net.obj(0)].attached[0].0;
    let before = net.c(1).seen.objects[&wobj].models.clone();
    let mut b = BitWriter::new();
    b.write_u8(1).write_u16(4).write_u16(0);
    net.cm(0).send_raw_shell(0x7f, &b);
    net.run_for(300);
    let after = net.c(1).seen.objects[&wobj].models.clone();
    assert_ne!(before, after, "hand model re-sent with CF_FILES");
    // a player without a loadout gets a weapon model once the drawn weapon is known (UP_WEAPON_CHANGE)
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![],
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut bare = Net::with(
        cfg,
        character_tables(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
    );
    bare.server.insert_rig("BODY\\b.ltb", rig());
    bare.add(1, "Dave", 0.0);
    bare.add(2, "Big Erin", 0.0);
    assert!(bare.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    bare.run_for(300);
    let dave = bare.obj(0);
    assert!(bare.c(1).seen.objects[&dave].attached.is_empty());
    let mut b = BitWriter::new();
    b.write_u8(0).write_u16(3).write_u16(0);
    bare.cm(0).send_raw_shell(0x7f, &b);
    bare.run_for(300);
    let seen = &bare.c(1).seen;
    assert!(seen.errors.is_empty(), "{:?}", seen.errors);
    let att = seen.objects[&dave].attached.clone();
    assert_eq!(att.len(), 1, "attached after the weapon change");
    assert_eq!(seen.objects[&att[0].0].kind, 1);
    assert_eq!(seen.objects[&att[0].0].flags, Some(0x0003));
    // a second, independent reader of the same object groups
    for i in 0..2 {
        let mut t = dc_botclient::objects::ObjectTable::default();
        for g in &net.c(i).seen.groups {
            t.apply_group(g);
        }
        assert!(t.errors.is_empty(), "client {i}: {:?}", t.errors);
        assert!(t.creates >= 4);
    }
}

/// Slice 3 relays (CAPTURE_tdm2 §3.5, CAPTURE_tdm1 §3.11): the sprint permission echo to the sender on a change of
/// its sprinting bit only; a weapon sound and a jump go to everyone as effect type 0x19 with the player id; each
/// bullet gives everyone a type 0x06 shot effect naming the shooter's character, but the fire after a throw
/// release does not; remote creates carry a rotation, the own create none (CD-67).
#[test]
fn relays_sprint_echo_and_effects() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![3, 4],
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(
        cfg,
        character_tables(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
    );
    net.server.insert_rig("BODY\\b.ltb", rig());
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    let echoes = |n: &Net, i: usize| n.c(i).seen.shell_ids.iter().filter(|s| s.1 == 0xca).count();
    let sprint = |on: bool, off: bool| {
        let mut b = BitWriter::new();
        b.write_bit(on).write_bit(off);
        b
    };
    net.cm(0).send_raw_shell(0xca, &sprint(true, false));
    net.cm(0).send_raw_shell(0xca, &sprint(true, false));
    net.run_for(200);
    assert_eq!(
        (echoes(&net, 0), echoes(&net, 1)),
        (1, 0),
        "one echo, to the sender"
    );
    net.cm(0).send_raw_shell(0xca, &sprint(false, true));
    net.run_for(200);
    assert_eq!(echoes(&net, 0), 2);
    // weapon sound 0x80 [sound 3][weapon 3][own id][pos] and a jump 0x76 [1][1]; an event 3 is not relayed
    let mut s = BitWriter::new();
    s.write_u8(3).write_u16(3).write_u8(net.pid(0));
    for c in [1.0f32, 2.0, 3.0] {
        s.write_f32(c);
    }
    net.cm(0).send_raw_shell(0x80, &s);
    let mut l = BitWriter::new();
    l.write_u8(1).write_u8(3);
    net.cm(0).send_raw_shell(0x76, &l);
    let mut j = BitWriter::new();
    j.write_u8(1).write_u8(1);
    net.cm(0).send_raw_shell(0x76, &j);
    net.run_for(200);
    for i in 0..2 {
        let fx: Vec<(u8, u16, u32)> = net
            .c(i)
            .seen
            .effects
            .iter()
            .filter(|e| e.1 == 0x19)
            .map(|e| {
                let mut r = e.2.reader();
                (r.read_u8(), r.read_u16(), r.read_u32())
            })
            .collect();
        let me = u32::from(net.pid(0));
        assert_eq!(fx, vec![(3, 3, me), (0x0f, 0, me)], "client {i}");
    }
    // a bullet: shot effect to both; the fire after a throw release: the throw sound instead
    let target = net.obj(1);
    assert!(net.cm(0).fire_at(target, [0.0; 3], false));
    net.run_for(200);
    net.cm(0).send_raw_shell(0x4d, &BitWriter::new());
    net.run_for(50);
    assert!(net.cm(0).fire_at(target, [0.0; 3], false));
    net.run_for(200);
    for i in 0..2 {
        let shots: Vec<u16> = net
            .c(i)
            .seen
            .effects
            .iter()
            .filter(|e| e.1 == 0x06)
            .map(|e| {
                let mut r = e.2.reader();
                r.read_u16();
                r.read_u16()
            })
            .collect();
        assert_eq!(
            shots,
            vec![net.obj(0)],
            "client {i}: one shot effect naming the shooter"
        );
        // DSV-85: `c` = the shooter's client id (here 0, the rust6 case) on every link; DSV-86: a miss keeps the
        // bullet mask
        // OBSERVED: capture 20261025_065016_tdm6 (c = the shooter's client id; REFERENCE)
        let fields: Vec<(u16, u16, u8)> = net
            .c(i)
            .seen
            .effects
            .iter()
            .filter(|e| e.1 == 0x06)
            .map(|e| {
                let mut r = e.2.reader();
                let first = r.read_u16();
                r.skip_bits(6 * 8 + 8);
                (first, r.read_u16(), r.read_u8())
            })
            .collect();
        assert_eq!(
            fields,
            vec![(0xFFFF, 0x4f10, net.pid(0))],
            "client {i}: (first, b, c)"
        );
        assert_eq!(net.pid(0), 0);
        let throws = net
            .c(i)
            .seen
            .effects
            .iter()
            .filter(|e| e.1 == 0x19 && e.2.reader().read_u8() == 0x12)
            .count();
        assert_eq!(throws, 1, "client {i}: the throw sound");
        assert_eq!(
            net.c(i)
                .seen
                .effects
                .iter()
                .filter(|e| e.1 == 0x06)
                .map(|e| e.2.bit_len())
                .next(),
            Some(416)
        );
    }
    // the record of each character was completed with 0xa6 sub 0x11 at its first spawn
    assert!(
        net.c(0)
            .seen
            .shell_ids
            .iter()
            .filter(|s| s.1 == 0xa6)
            .count()
            >= 2
    );
    // remote creates carry a rotation, the own one does not
    for i in 0..2 {
        let mut t = dc_botclient::objects::ObjectTable::default();
        for g in &net.c(i).seen.groups {
            t.apply_group(g);
        }
        assert!(t.errors.is_empty(), "client {i}: {:?}", t.errors);
        assert!(
            t.objects[&net.obj(1 - i)].rot.is_some(),
            "client {i}: remote rotation"
        );
        assert!(
            t.objects[&net.obj(i)].rot.is_none(),
            "client {i}: no own rotation"
        );
    }
}

fn n_state(n: &Net, i: usize) -> String {
    format!("{:?}/{}", n.c(i).state(), n.c(i).life_state())
}

fn shell_order(c: &SynthClient) -> Vec<u8> {
    c.seen.shell_ids.iter().map(|x| x.1).collect()
}

#[test]
fn s1_to_s12_and_the_spawn_set_reach_state_1() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![3, 4],
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::new(cfg, "room key=TDM type=1 goal=20 ff=0 time=0");
    net.add(1, "Dave", 1.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1));
    net.run_for(2000);
    let c = net.c(0);
    let s = &c.seen;
    assert!(s.errors.is_empty(), "{:?}", s.errors);
    assert_eq!(c.state(), ClientState::InWorld);
    // S11 then the game layer: identify, roster, settings, S12 state 3, then after 0x87 the match start and the
    // spawn set 0xa0 -> 0x6a -> 0xb2 / 0xb3 -> 0x66 x 4 -> 0x67 state 1, then the own record refreshed (kind 2,
    // alive; CV2-7)
    let order = shell_order(c);
    let start = order.iter().position(|&x| x == 0x88).unwrap();
    assert_eq!(
        &order[start..start + 19],
        &[
            0x88, 0x72, 0x6b, 0x7d, 0x67, 0x45, 0xb5, 0xc3, 0xa0, 0x6a, 0xb2, 0xb3, 0x66, 0x66,
            0x66, 0x66, 0x67, 0x6b, 0xa2
        ],
        "{order:x?}"
    );
    assert_eq!(
        s.life_states.iter().map(|x| x.1).collect::<Vec<_>>(),
        vec![3, 1]
    );
    let ready = s.world_ready_sent.expect("0x87 sent");
    assert!(ready >= s.life_states[0].0, "0x87 only after state 3");
    assert_eq!(s.identify_requests, 1);
    // own record: team 1 (lobby team 0), names the view object
    assert_eq!(s.own_team, Some(1));
    let me = &s.roster[&0];
    assert_eq!(
        (me.team, me.character, me.account),
        (1, s.view_object.unwrap(), 7)
    );
    assert_eq!(s.settings, Some((1, 20, 1)));
    assert_eq!(
        s.match_begin,
        Some((0, false)),
        "no time limit: timer hidden; DSV-20 revision: no waiting banner"
    );
    // spawn: team 1 point, sequence 1, angles of the point, full health
    let tp = s.teleports[0];
    assert_eq!((tp.1, tp.2), (1, RED));
    assert_eq!(s.angles, Some([0.0, 1.5, 0.0]));
    assert_eq!(
        (s.health, s.max_health, s.armor),
        (Some(100.0), Some(100.0), Some(0.0))
    );
    assert_eq!(
        s.weapons,
        vec![(0xb2, 4, 0), (0xb3, 3, 0)],
        "0xb2 for the others, 0xb3 for the drawn one"
    );
    assert!(s.perf_enabled);
    // the client moves only in state 1 and with the teleport's sequence; the server follows it
    let (_, sc) = net.server.clients().iter().next().unwrap();
    let at = |st: Step| {
        sc.reached(st)
            .unwrap_or_else(|| panic!("{st:?}: {:?}", sc.steps))
    };
    assert!(at(Step::SwitchState) <= at(Step::IdentifyRequest));
    assert!(at(Step::IdentifyRequest) < at(Step::PlayerInfo));
    assert!(at(Step::PlayerInfo) <= at(Step::Introduced));
    assert!(at(Step::Introduced) <= at(Step::WaitingState));
    assert!(at(Step::WaitingState) < at(Step::WorldReady));
    assert!(at(Step::WorldReady) <= at(Step::Spawned));
    assert!(at(Step::Spawned) < at(Step::FirstUpdate));
    assert!(sc.updates >= 25, "{}", sc.updates);
    let gp = &net.server.game().players()[&0];
    assert_eq!(gp.stale_updates, 0);
    assert_eq!(gp.corrections, 0);
    assert!(
        (gp.pos[0] - c.position()[0]).abs() <= 2.0,
        "{:?} vs {:?}",
        gp.pos,
        c.position()
    );
    assert!(gp.pos[0] > RED[0] + 20.0);
}

#[test]
fn two_teams_see_each_other_and_updates_are_relayed() {
    let mut net = Net::two_alive(
        ServerTuning::default(),
        "room key=TDM type=1 goal=20 ff=0",
        [1.0, 2.0],
    );
    net.run_for(2000);
    let (o0, o1) = (net.obj(0), net.obj(1));
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
        assert_eq!(s.roster.len(), 2, "client {i} roster");
        assert_eq!(s.own_team, Some(i as u8 + 1));
        assert!(s.objects.contains_key(&o0) && s.objects.contains_key(&o1));
    }
    assert_eq!(net.c(0).seen.roster[&1].team, 2);
    assert_eq!(net.c(1).seen.roster[&0].team, 1);
    // spawn points of the own team
    assert_eq!(net.c(0).seen.teleports[0].2, RED);
    assert_eq!(net.c(1).seen.teleports[0].2, BLUE);
    // each client sees the other's object move in the stream (pos48 precision, at most one update behind), never
    // its own object
    for (me, other) in [(0usize, 1usize), (1, 0)] {
        let r = net.c(me).seen.remote[&net.obj(other)];
        let truth = net.c(other).position();
        let step = if other == 0 { 1.0 } else { 2.0 };
        assert!(r.entries > 20, "{} entries", r.entries);
        assert!(
            (r.pos[0] - truth[0]).abs() <= 3.0 * step + 0.05,
            "client {me} sees {:?}, truth {:?}",
            r.pos,
            truth
        );
        assert!((r.pos[2] - truth[2]).abs() <= 0.05);
        assert!((r.vel[0] - step * 15.0).abs() < 0.5, "{:?}", r.vel);
        assert!(r.yaw.is_some());
        assert!(!net.c(me).seen.remote.contains_key(&net.obj(me)));
    }
    // leaving: the other client gets 0x6c and the removal record
    net.cm(1).quit();
    net.run_for(1000);
    let s = &net.c(0).seen;
    assert_eq!(s.players_left, vec![1]);
    assert_eq!(s.removed, vec![o1]);
    assert!(!s.roster.contains_key(&1));
}

#[test]
fn hits_kill_and_the_respawn_timeline() {
    let mut net = Net::two_alive(
        ServerTuning::default(),
        "room key=TDM type=1 goal=10 ff=0",
        [0.0, 0.0],
    );
    let (p0, p1) = (net.pid(0), net.pid(1));
    // DeCombat default damage 25 per hit (no ROM table): four hits
    net.shoot(0, 1, 3, [0.0; 3]);
    assert_eq!(
        net.c(1).seen.damage.iter().map(|d| d.1).collect::<Vec<_>>(),
        vec![25, 25, 25]
    );
    assert_eq!(net.c(1).seen.health, Some(25.0));
    assert!(net.c(1).seen.kills.is_empty());
    net.shoot(0, 1, 1, [0.0; 3]);
    let t_kill = net.c(1).seen.kills.first().expect("kill").at;
    for i in 0..2 {
        let k = &net.c(i).seen.kills;
        assert_eq!(k.len(), 1, "client {i}");
        // DSV-33: the reference stack's 0x6d values; torso zone 2 without a model node; streak 1 on the first kill of a life
        assert_eq!((k[0].victim, k[0].killer, k[0].zone), (p1, p0, 2));
        assert_eq!(
            (
                k[0].kill_kind,
                k[0].relation,
                k[0].streak_flag,
                k[0].feed_value
            ),
            (1, false, 1, 2)
        );
        // DC-KILL-3: team kills in 0xb5 after the kill ([0][0] at the start), never 0x4a
        let tk = &net.c(i).seen.team_kills;
        assert_eq!(tk.first().map(|t| (t.1, t.2)), Some((0, 0)));
        assert_eq!(tk.last().map(|t| (t.1, t.2)), Some((1, 0)));
        assert!(net.c(i).seen.team_scores.is_empty(), "no 0x4a");
    }
    // DC-KILL-2: 0x74 in the observed form: from_dir = direction x health loss / 100, ammo_kind 0xFFFF, mode 1,
    // dmg_kind 3, zone = the ammunition's damage type (0 without a ROM table) repeated in the unread byte
    let f = net.c(1).seen.damage_forms[0];
    let mag = (f.from_dir[0].powi(2) + f.from_dir[1].powi(2) + f.from_dir[2].powi(2)).sqrt();
    assert!((mag - 0.25).abs() < 1e-4, "{mag}");
    assert_eq!(
        (
            f.unused,
            f.mode,
            f.ammo_kind,
            f.dmg_kind,
            f.zone,
            f.part,
            f.damage
        ),
        (0, 1, 0xFFFF, 3, 0, 0, 25)
    );
    // the kill messages come before the victim's own 0x74 / 0x66 / 0x67 of the lethal hit
    let o = &net.c(1).seen.shell_ids;
    let at = |id: u8, from: usize| from + o[from..].iter().position(|x| x.1 == id).unwrap();
    let k6d = o.iter().rposition(|x| x.1 == 0x6d).unwrap();
    let last74 = o.iter().rposition(|x| x.1 == 0x74).unwrap();
    assert!(k6d < last74 && at(0xb5, k6d) < last74);
    assert!(at(0x67, last74) > last74);
    assert_eq!(net.c(1).seen.health, Some(0.0));
    // a dead player is not damaged and an early respawn request is ignored (DSV-32: wait minus the guard)
    net.shoot(0, 1, 1, [0.0; 3]);
    assert_eq!(net.c(1).seen.damage.len(), 4);
    net.cm(1).request_respawn();
    // death states 3 then 2 in one block (DSV-32 reference order); the client arms its own 7000 ms wait on the 2 and
    // asks with 0x89 once it passed; the server answers at once (teleport seq 2, state 1); no server timer in TDM
    net.run_until(t_kill + 8_000, |n| n.c(1).seen.life_states.len() >= 5);
    let ls: Vec<(u32, u8)> = net.c(1).seen.life_states.clone();
    assert_eq!(
        ls.iter().map(|x| x.1).collect::<Vec<_>>(),
        vec![3, 1, 3, 2, 1],
        "{ls:?}"
    );
    assert_eq!(ls[2].0, ls[3].0, "3 and 2 in the same block");
    let s1 = &net.c(1).seen;
    assert_eq!(s1.respawn_armed.len(), 1);
    assert_eq!(s1.respawn_armed[0].1, 7_000);
    let req = *s1.respawn_requests.last().expect("0x89 sent");
    assert_eq!(req.1, 0x89);
    assert!(
        req.0 - s1.respawn_armed[0].0 > 7_000,
        "strictly after the wait"
    );
    let d7 = ls[4].0 - ls[3].0;
    assert!((7_000..=7_060).contains(&d7), "respawn after {d7} ms");
    assert!(ls[4].0 >= req.0, "answered after the request");
    assert!(net.logs.iter().any(|l| l.contains("ignored (too early")));
    assert!(net.logs.iter().any(|l| l.contains("answered")));
    assert!(!net
        .logs
        .iter()
        .any(|l| l.contains("respawned by the server")));
    let tp = *net.c(1).seen.teleports.last().unwrap();
    assert_eq!((tp.1, tp.2), (2, BLUE));
    assert_eq!(net.c(1).seen.health, Some(100.0));
    // the clients counted the kill themselves from 0x6d
    assert_eq!(net.c(0).seen.roster[&p0].kills, 1);
    assert_eq!(net.c(0).seen.roster[&p1].deaths, 1);
    let st = net.server.game().stats();
    assert_eq!(
        (st[&7].kills, st[&7].deaths, st[&8].kills, st[&8].deaths),
        (1, 0, 0, 1)
    );
    // the respawned player moves again with the new sequence
    net.run_for(1000);
    let gp = &net.server.game().players()[&p1];
    assert_eq!(gp.move_seq, 2);
    assert_eq!(gp.corrections, 0);
}

#[test]
fn head_shots_team_hits_clip_and_multi_kill() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_damage: 60.0,
            default_clip: 3,
            respawn_wait_ms: Some(2_500),
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::two_alive(cfg, "room key=TDM type=1 goal=10 ff=0", [0.0, 0.0]);
    // 1) a shot whose reported hit point lies far from the victim's box is refused (it still uses a round: 3 -> 2)
    net.shoot(0, 1, 1, [0.0, 0.0, 400.0]);
    assert!(net.c(1).seen.damage.is_empty());
    assert!(net.logs.iter().any(|l| l.contains("refused")));
    // 2) head shot: the top of the box doubles the damage, kills at once, zone 1 in the feed (2 -> 1); 0x74 carries
    // the health damage before the clamp to the remaining health (120, not the 100 lost; the 2009 form of
    // CAPTURE_tdm6 §3 item 3: 210 on a victim with 70.25 left), DC-KILL-2 / DSV-33
    net.shoot(0, 1, 1, [0.0, 50.0, 0.0]);
    assert_eq!(net.c(1).seen.damage.last().map(|d| d.1), Some(120));
    let f = net.c(1).seen.damage_forms.last().unwrap();
    let mag = (f.from_dir[0].powi(2) + f.from_dir[1].powi(2) + f.from_dir[2].powi(2)).sqrt();
    assert!(
        (mag - 1.0).abs() < 1e-4,
        "|from_dir| = min(1, 120 / 100): {mag}"
    );
    assert_eq!(net.c(0).seen.kills[0].zone, 1);
    assert_eq!(net.server.game().stats()[&7].headshots, 1);
    // 3) respawn after 2.5 s; a second kill within 4 s of the first: level 2 (the double-kill form) in the kill
    // message, no 0xc8 (DSV-105; 1 -> 0)
    net.run_until(net.now + 4_000, |n| n.c(1).life_state() == 1);
    net.run_for(300);
    net.shoot(0, 1, 1, [0.0, 50.0, 0.0]);
    assert_eq!(net.c(0).seen.kills.len(), 2);
    assert_eq!(net.c(0).seen.kills[1].kill_kind, 2);
    assert!(net.c(0).seen.multi_kills.is_empty());
    // 4) the clip of 3 is empty: no damage until a reload (DC-C2S-4)
    net.run_until(net.now + 4_000, |n| n.c(1).life_state() == 1);
    net.run_for(300);
    let before = net.c(1).seen.damage.len();
    net.shoot(0, 1, 1, [0.0; 3]);
    assert_eq!(
        net.c(1).seen.damage.len(),
        before,
        "a round beyond the clip"
    );
    assert!(net.logs.iter().any(|l| l.contains("beyond the clip")));
    // 5) a reload refills the clip from the stock
    net.cm(0).reload();
    net.run_for(50);
    net.shoot(0, 1, 1, [0.0; 3]);
    assert_eq!(net.c(1).seen.damage.len(), before + 1);
    assert_eq!(net.c(1).seen.health, Some(40.0));
}

/// A synthetic armour row in the key shape of the ROM weapon table (made-up numbers): 60 armour points, the damage
/// type 0 (the type of a hit without a ROM ammunition row) protected with share 0 on the head and 0.2 on the torso.
fn gear_tables() -> RomTables {
    let doc = dc_bute::ButeDoc::parse(concat!(
        "[Gear0]\nArmor = 60.0\nProtectionType0 = 0\n",
        "NodeHeadProtection0 = 0.0\nNodeTorsoProtection0 = 0.2\n",
        "NodeArmProtection0 = 0.2\nNodelegProtection0 = 0.2\n",
    ));
    RomTables {
        weapons: dc_dserver::rom_tables::WeaponTable::from_doc(&doc),
        ..tables()
    }
}

/// DSV-30 as calibrated (project decision): every player wears armour row 0 (60 points at the spawn), a body
/// hit of the default 25 costs the armour 0.2 x 25 and the health 25 x 0.8, a head shot (2.0 by the box) is not
/// protected and leaves the armour as it is; 0x74 carries the health damage; `armor_gear = off` and `armor_cost = hit`
/// change it as configured.
#[test]
fn armour_row_shares_body_hits_and_spares_head_shots() {
    let mut net = Net::with(
        ServerTuning::default(),
        gear_tables(),
        "room key=TDM type=1 goal=10 ff=0",
    );
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    assert_eq!(
        net.c(1).seen.armor,
        Some(60.0),
        "armour of the row at the spawn"
    );
    // body: 20 to the health, 5 from the armour
    net.shoot(0, 1, 1, [0.0; 3]);
    assert_eq!(net.c(1).seen.damage.last().map(|d| d.1), Some(20));
    assert_eq!(
        (net.c(1).seen.health, net.c(1).seen.armor),
        (Some(80.0), Some(55.0))
    );
    // head (top of the box, factor 2): 50 to the health, the armour untouched
    net.shoot(0, 1, 1, [0.0, 50.0, 0.0]);
    assert_eq!(net.c(1).seen.damage.last().map(|d| d.1), Some(50));
    assert_eq!(
        (net.c(1).seen.health, net.c(1).seen.armor),
        (Some(30.0), Some(55.0))
    );
    assert_eq!(net.server.game().players()[&net.pid(1)].armor, 55.0);
    // body again: 20 / 5; the lethal body hit carries the unclamped 20 with 10 left
    net.shoot(0, 1, 1, [0.0; 3]);
    assert_eq!(
        (net.c(1).seen.health, net.c(1).seen.armor),
        (Some(10.0), Some(50.0))
    );
    net.shoot(0, 1, 1, [0.0; 3]);
    assert_eq!(net.c(1).seen.damage.last().map(|d| d.1), Some(20));
    assert_eq!(net.c(1).seen.health, Some(0.0));
    assert_eq!(net.c(0).seen.kills.len(), 1);

    // operator keys: no armour at all; the zone multiplier of the body (1.5 with the ROM zones off) does not raise
    // the armour cost unless `armor_cost = hit`
    for (keys, health, armour) in [
        (&[("armor_gear", "off")][..], 75.0, 0.0),
        (
            &[("zone_factors", "off"), ("body_factor", "1.5")][..],
            70.0,
            55.0,
        ),
        (
            &[
                ("zone_factors", "off"),
                ("body_factor", "1.5"),
                ("armor_cost", "hit"),
            ][..],
            70.0,
            52.5,
        ),
    ] {
        let mut cfg = ServerTuning::default();
        for (k, v) in keys {
            cfg.game.set_combat(k, v).unwrap();
        }
        let mut net = Net::with(cfg, gear_tables(), "room key=TDM type=1 goal=10 ff=0");
        net.add(1, "Dave", 0.0);
        net.add(2, "Big Erin", 0.0);
        assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
            && n.c(1).life_state() == 1));
        net.run_for(500);
        net.shoot(0, 1, 1, [0.0; 3]);
        let s = &net.c(1).seen;
        assert_eq!(
            (s.health, s.armor),
            (Some(health), Some(armour)),
            "{keys:?}"
        );
    }
}

#[test]
fn friendly_fire_off_by_default() {
    let mut net = Net::new(ServerTuning::default(), "room key=TDM type=1 goal=10 ff=0");
    // both players in the lobby's team 0
    assert!(net.server.handle_line(0, "user 8 4 55 0 0 Big_Erin"));
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    net.shoot(0, 1, 2, [0.0; 3]);
    assert!(net.c(1).seen.damage.is_empty());
    assert_eq!(net.c(1).seen.own_team, Some(1));
}

#[test]
fn round_ends_on_the_goal() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_damage: 100.0,
            respawn_wait_ms: Some(2_100),
            end_pause_ms: (1_000, 800),
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::two_alive(cfg, "room key=TDM type=1 goal=2 ff=0", [0.0, 0.0]);
    net.shoot(0, 1, 1, [0.0; 3]);
    assert!(net.c(0).seen.round_result.is_none());
    net.run_until(net.now + 3_000, |n| n.c(1).life_state() == 1);
    net.run_for(300);
    net.shoot(0, 1, 1, [0.0; 3]);
    let ended = net.c(0).seen.kills.last().expect("second kill").at;
    // DSV-42: the round-end counter line includes the lethal claim that ended the round
    assert!(
        net.logs
            .iter()
            .any(|l| l.contains("hits: 2 claims, 2 accepted (0 rewound); refused none")),
        "{:?}",
        net.logs
            .iter()
            .filter(|l| l.contains("hits:"))
            .collect::<Vec<_>>()
    );
    // the lobby computed the rewards meanwhile and sends them back (DSV-35)
    // DC-DEFAULT: DSV-142 (proposed) with the bonus pair
    assert!(net.server.handle_line(net.now, "gain 7 38 40 5 8"));
    // DSV-35: 0x46 [pause secs][win][round wins before: 0, 0] at once; after the first pause 0x47, the roster refresh
    // (0x6b kind 2), 0x65 per player and 0x48; after the second 0x3f [0][win]; never 0x41 or 0x99
    net.run_for(2_500);
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.round_result, Some((1, 0, 0)), "client {i}");
        assert_eq!(s.round_result_secs, Some(1));
        // the death note of the lethal kill (DSV-91, unreliable) may arrive among the reliable round-end messages
        let mut o = shell_order(net.c(i));
        o.retain(|&x| x != 0xa6);
        let p = o.iter().position(|&x| x == 0x46).unwrap();
        // DSV-77: the living killer is frozen with the result (0xa0), the dead victim is not
        let freeze: &[u8] = if i == 0 { &[0xa0] } else { &[] };
        let want: Vec<u8> = [
            &[0x46][..],
            freeze,
            &[0x47, 0x6b, 0x6b, 0x65, 0x65, 0x48, 0x3f],
        ]
        .concat();
        assert_eq!(&o[p..], &want[..], "client {i}: {o:x?}");
        assert!(!o.contains(&0x41) && !o.contains(&0x99));
        let at = |id: u8| s.shell_ids.iter().find(|x| x.1 == id).unwrap().0;
        assert!(
            (1_000..=1_060).contains(&(at(0x47) - ended)),
            "{}",
            at(0x47) - ended
        );
        assert!(
            (1_800..=1_860).contains(&(at(0x3f) - ended)),
            "{}",
            at(0x3f) - ended
        );
        assert_eq!(
            s.result_value,
            Some((false, 1)),
            "result value = the winning team"
        );
        assert_eq!(s.round_close.map(|r| r.1), Some(1));
        // the summary names each player's character; the lobby's gains reached Dave's row only
        let mut rows = s.summary_rows.clone();
        rows.sort();
        assert_eq!(rows.len(), 2);
        let dave = rows.iter().find(|r| r.0 == net.pid(0)).unwrap();
        assert_eq!((dave.1, dave.2, dave.3, dave.4), (net.obj(0), 120, 38, 40));
        let erin = rows.iter().find(|r| r.0 == net.pid(1)).unwrap();
        assert_eq!((erin.1, erin.3, erin.4), (net.obj(1), 0, 0));
        // DSV-142: the bonus pair reaches the 0x65 bonus fields of Dave's row only
        let mut bonus = s.summary_bonus.clone();
        bonus.sort();
        assert!(bonus.contains(&(net.pid(0), 5, 8)), "{bonus:?}");
        assert!(bonus.contains(&(net.pid(1), 0, 0)), "{bonus:?}");
        // the refresh carries the absolute counts
        assert_eq!(s.roster[&net.pid(0)].kills, 2);
        assert_eq!(s.roster[&net.pid(1)].deaths, 2);
        assert_eq!(s.roster[&net.pid(1)].kind, 2);
        assert!(s.level_exit.is_none());
    }
    let lines = net.pipe_lines();
    let n = lines.len();
    let mut players = lines[n - 3..n - 1].to_vec();
    players.sort();
    assert_eq!(
        players,
        vec![
            "@player uid=7 kills=2 deaths=0 team=1 tk=0 hs=0 name=Dave".to_string(),
            "@player uid=8 kills=0 deaths=2 team=2 tk=0 hs=0 name=Big_Erin".to_string(),
        ]
    );
    assert_eq!(
        lines[n - 1],
        "@result mode=TDM win=1 alpha=2 bravo=0 limit=2 rounds=1:0"
    );
    // no respawn after the end; the links stay open (no server bye) until the clients leave
    net.run_for(5_000);
    assert!(!net.server.is_finished());
    for i in 0..2 {
        assert_eq!(net.c(i).state(), ClientState::InWorld);
        assert_eq!(
            net.c(i).seen.life_states.last().map(|x| x.1),
            Some(if i == 1 { 2 } else { 1 })
        );
    }
    net.cm(0).quit();
    net.run_for(500);
    assert!(!net.server.is_finished(), "one client still linked");
    net.cm(1).quit();
    net.run_for(500);
    assert!(net.server.is_finished());
    let lines = net.pipe_lines();
    assert_eq!(lines.last().map(String::as_str), Some("@end"));
    assert_eq!(lines.iter().filter(|l| l.starts_with("@result")).count(), 1);
    assert_eq!(lines.iter().filter(|l| l.starts_with("@leave")).count(), 2);
}

/// DSV-31: a team that had a player and has none left for `team_empty_ms` loses the round; a rejoin inside the
/// window cancels it. The end set and the result screen follow as for a goal.
#[test]
fn round_ends_when_a_team_has_no_players_left() {
    let cfg = ServerTuning {
        game: GameTuning {
            team_empty_ms: 2_000,
            end_pause_ms: (300, 300),
            linger_ms: 3_000,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::two_alive(cfg, "room key=TDM type=1 goal=10 ff=0", [0.0, 0.0]);
    net.cm(1).quit();
    let left = net.now;
    net.run_for(1_500);
    assert!(
        net.c(0).seen.round_result.is_none(),
        "not before the window"
    );
    net.run_until(left + 3_000, |n| n.c(0).seen.round_result.is_some());
    let s = &net.c(0).seen;
    let at = s.shell_ids.iter().find(|x| x.1 == 0x46).unwrap().0;
    assert!((2_000..=2_150).contains(&(at - left)), "{}", at - left);
    assert_eq!(s.round_result, Some((1, 0, 0)), "team 1 wins");
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("team 2 has had no players")));
    assert!(net
        .pipe_lines()
        .contains(&"@result mode=TDM win=1 alpha=0 bravo=0 limit=10 rounds=1:0".to_string()));
    // the remaining client never leaves: the linger limit ends the session after the result screen
    net.run_for(1_000);
    assert!(net.c(0).seen.result_screen.is_some());
    assert!(!net.server.is_finished());
    net.run_for(3_000);
    assert!(net.server.is_finished(), "linger limit");
    assert!(matches!(net.c(0).state(), ClientState::Dropped(_)));
}

#[test]
fn round_ends_on_the_timer() {
    // one kill, then the 60 s timer (DSV-250: values below 60 are minutes): team 1 wins
    let cfg = ServerTuning {
        game: GameTuning {
            default_damage: 100.0,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::two_alive(cfg, "room key=TDM type=1 goal=50 ff=0 time=60", [0.0, 0.0]);
    let started = net.c(0).seen.world_ready_sent.unwrap();
    assert_eq!(net.c(0).seen.match_begin.map(|m| m.0), Some(60));
    // the second player joined a running round: the remaining time
    assert!(net.c(1).seen.match_begin.map(|m| m.0).unwrap() <= 60);
    net.shoot(1, 0, 1, [0.0; 3]);
    net.run_until(started + 62_000, |n| n.c(0).seen.round_result.is_some());
    let s = &net.c(0).seen;
    // DSV-35: 0x46 carries the round wins before this round (0 / 0 in a one-round match), not the kill score
    assert_eq!(s.round_result, Some((2, 0, 0)));
    let at = s.shell_ids.iter().find(|x| x.1 == 0x46).unwrap().0;
    assert!(
        (60_000..=60_050).contains(&(at - started)),
        "ended after {} ms",
        at - started
    );
    let lines = net.pipe_lines();
    assert!(
        lines.contains(&"@result mode=TDM win=2 alpha=0 bravo=1 limit=50 rounds=0:1".to_string())
    );
    // ping table every 5 s while the round ran: at the start only (the round lasted 60 s): 0, 5, .., 55 s
    assert_eq!(s.ping_tables.len(), 12);
}

// DC-DEFAULT: GMD-2 (clarification) the lobby's TDM room line now carries the class fallback `time=600` (the ROM
// mode file has no limit); the client's round clock is fed by 0x44 / 0x45 `round_secs` and by the co-op 0xd7 sub 4 (absent from every TDM capture)
// (GAME_MESSAGES_2009_S2C.md §2.1, §2.1.1 step 1; DISASM cshell+0x49a40 case 0x45, cshell+0xbe070, cshell+0xbe570)
#[test]
fn fallback_round_time_reaches_round_begin_and_match_begin() {
    // spec start order (this file's default): the round's first spawns get 0x45 with the seconds left
    let net = Net::two_alive(
        ServerTuning::default(),
        "room key=TDM type=1 goal=100 ff=0 arms=0 time=600 train=0",
        [0.0, 0.0],
    );
    assert_eq!(net.c(0).seen.match_begin, Some((600, false)));
    let (mb, banner) = net.c(1).seen.match_begin.expect("0x45");
    assert!((590..=600).contains(&mb) && !banner, "0x45 round_secs {mb}");
    assert!(net.c(0).seen.round_result.is_none());
}

#[test]
fn draw_on_timer_and_ping_table_period() {
    let mut net = Net::two_alive(
        ServerTuning::default(),
        "room key=TDM type=1 goal=50 ff=0 time=60",
        [0.0, 0.0],
    );
    let started = net.c(0).seen.world_ready_sent.unwrap();
    net.run_until(started + 61_000, |n| n.c(1).seen.round_result.is_some());
    assert_eq!(net.c(1).seen.round_result, Some((0, 0, 0)));
    let t: Vec<u32> = net
        .c(0)
        .seen
        .ping_tables
        .iter()
        .map(|p| p.0 - started)
        .collect();
    assert_eq!(t.len(), 12, "{t:?}");
    assert!(
        t.windows(2)
            .all(|w| (5_000..=5_020).contains(&(w[1] - w[0]))),
        "{t:?}"
    );
    assert_eq!(net.c(0).seen.ping_tables[2].1.len(), 2);
    assert!(net
        .pipe_lines()
        .contains(&"@result mode=TDM win=0 alpha=0 bravo=0 limit=50 rounds=0:0".to_string()));
}

#[test]
fn chat_goes_to_the_audience_including_the_sender() {
    let mut net = Net::two_alive(
        ServerTuning::default(),
        "room key=TDM type=1 goal=9",
        [0.0, 0.0],
    );
    net.cm(0).chat("hello all", 0);
    net.cm(1).chat("team only", 1);
    net.run_for(200);
    let c0: Vec<_> = net.c(0).seen.chat.clone();
    let c1: Vec<_> = net.c(1).seen.chat.clone();
    assert_eq!(
        c0,
        vec![("Dave".to_string(), "hello all".to_string(), 0, false)]
    );
    assert_eq!(
        c1,
        vec![
            ("Dave".to_string(), "hello all".to_string(), 0, false),
            ("Big Erin".to_string(), "team only".to_string(), 1, true),
        ]
    );
}

#[test]
fn unhandled_and_developer_ids_are_logged_once() {
    let mut net = Net::two_alive(
        ServerTuning::default(),
        "room key=TDM type=1 goal=9",
        [0.0, 0.0],
    );
    for _ in 0..3 {
        net.cm(0)
            .send_raw_shell(0xee, &BitWriter::from_bytes(&[1, 2]));
        net.cm(0).send_raw_shell(0xa7, &BitWriter::from_bytes(&[0]));
    }
    net.run_for(200);
    let n = |pat: &str| net.logs.iter().filter(|l| l.contains(pat)).count();
    assert_eq!(n("shell message 0xee"), 1);
    assert_eq!(n("developer message 0xa7"), 1);
}

#[test]
fn leave_match_removes_the_player_at_once() {
    let mut net = Net::two_alive(
        ServerTuning::default(),
        "room key=TDM type=1 goal=9",
        [0.0, 0.0],
    );
    net.cm(1).leave_match();
    net.run_for(300);
    assert_eq!(net.c(0).seen.players_left, vec![1]);
    assert!(matches!(net.c(1).state(), ClientState::Dropped(_)));
    assert!(net.pipe_lines().contains(&"@leave 1".to_string()));
}

#[test]
fn tick_rates_10_20_30_60() {
    for hz in [10u32, 20, 30, 60] {
        let cfg = ServerTuning {
            tick_hz: hz,
            ..ServerTuning::default()
        };
        let mut net = Net::new(cfg, "room key=TDM type=1 goal=9");
        net.add(1, "Dave", 1.0);
        assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1));
        let t0 = net.now;
        net.run_for(2_000);
        let n = net
            .c(0)
            .seen
            .server_times
            .iter()
            .filter(|t| t.0 >= t0 && t.0 < t0 + 2_000)
            .count() as u32;
        let want = 2 * hz;
        // 5 ms simulation steps: a 60 Hz tick (16 ms) lands on 15 / 20 ms steps
        assert!(
            n + 2 * hz / 10 + 2 >= want && n <= want + 2,
            "{hz} Hz: {n} time entries in 2 s"
        );
    }
}

/// Synthetic tables for a throwable (made-up ids, names and numbers in the ROM key shape): weapon 40 with ammunition
/// 41 whose projectile row names a model and skin, launch speed 100, fuse 1.8 s; blast 80 at the centre, radius 200,
/// damage type 5. The weapon is its own ammunition (`IsAmmo`), one in the hand (`ShotsPerClip` 1), a single grenade.
fn throw_tables() -> RomTables {
    throw_tables_with(1)
}

/// [`throw_tables`] with `pack` grenades granted (`MaxAmount`).
fn throw_tables_with(pack: u32) -> RomTables {
    use dc_dserver::rom_tables::{AmmoRow, ProjectileRow, ProjectileTable, WeaponRow, WeaponTable};
    let mut weapons = WeaponTable::default();
    weapons.weapons.insert(
        40,
        WeaponRow {
            clip: Some(1),
            ammo: Some(41),
            name: Some("Nade".into()),
            own_ammo: true,
            gun_type: None,
            ..WeaponRow::default()
        },
    );
    weapons.ammo.insert(
        41,
        AmmoRow {
            damage: Some(0.0),
            max_amount: Some(pack),
            area_damage: Some(80.0),
            area_damage_type: Some(5),
            area_radius: Some(200.0),
            projectile_fx: Some("TestNade".into()),
            ..AmmoRow::default()
        },
    );
    let mut projectiles = ProjectileTable::default();
    projectiles.rows.insert(
        dc_bute::fold_latin1("TestNade"),
        ProjectileRow {
            model: Some("PROJ\nade.ltb".into()),
            skin: Some("PROJ\nade.dtx".into()),
            scale: Some([1.5; 3]),
            gravity: Some(true),
            gravity_override: Some(-900.0),
            lifetime_s: Some(1.8),
            velocity: Some(100.0),
            ..ProjectileRow::default()
        },
    );
    RomTables {
        weapons,
        projectiles,
        ..tables()
    }
}

/// DSV-39 (proposed): a throw (0x4d, then the release 0x82) creates a kind-1 object with the 32-bit type-0x13 blob
/// on every client in the world; at the fuse it is removed, the blast effect (type 0x06, no hit object, the throw's
/// ids) goes to everyone and area damage falls off linearly from the centre: the thrower and a team mate next to it
/// are in range (the team mate only with friendly fire), the enemy 630 units away is not.
#[test]
fn a_throw_creates_an_object_then_blows_up_with_area_damage() {
    for ff in [true, false] {
        let room = format!("room key=TDM type=1 goal=20 ff={}", u8::from(ff));
        let mut net = Net::with(ServerTuning::default(), throw_tables(), &room);
        assert!(net.server.handle_line(0, "user 9 5 10 0 0 Cid"));
        net.add(1, "Dave", 0.0);
        net.add(2, "Big Erin", 0.0);
        net.add(3, "Cid", 0.0);
        assert!(
            net.run_until(20_000, |n| (0..3).all(|i| n.c(i).life_state() == 1)),
            "ff {ff}: all alive"
        );
        net.run_for(500);
        let (p0, p2) = (net.pid(0), net.pid(2));
        let teams = &net.server.game().players();
        assert_eq!(
            teams[&p0].team, teams[&p2].team,
            "Dave and Cid are team mates"
        );
        let thrower = teams[&p0].pos;
        assert!(net.c(1).seen.damage.is_empty() && net.c(2).seen.damage.is_empty());
        // at the own feet
        net.cm(0).throw((40, 41), [0.0, -1.0, 0.0]);
        net.run_for(300);
        assert_eq!(net.server.game().flights_in_air(), 1, "ff {ff}: in flight");
        let created = |n: &Net, i: usize| {
            let mut t = dc_botclient::objects::ObjectTable::default();
            for g in &n.c(i).seen.groups {
                t.apply_group(g);
            }
            assert!(t.errors.is_empty(), "client {i}: {:?}", t.errors);
            assert!(
                n.c(i).seen.errors.is_empty(),
                "client {i}: {:?}",
                n.c(i).seen.errors
            );
            t
        };
        let mut nade = None;
        for i in 0..3 {
            let t = created(&net, i);
            let o: Vec<(&u16, &dc_botclient::objects::ClientObject)> = t
                .objects
                .iter()
                .filter(|(_, o)| o.blob_bits == 32)
                .collect();
            assert_eq!(o.len(), 1, "ff {ff}, client {i}: one thrown object");
            assert_eq!(o[0].1.kind, 1, "a model object");
            let v = o[0].1.vel.expect("launch velocity");
            assert!(v[1] < -50.0, "falling: {v:?}");
            assert_eq!(
                *nade.get_or_insert(*o[0].0),
                *o[0].0,
                "the same id everywhere"
            );
        }
        let nade = nade.unwrap();
        // the fuse of the projectile row (1.8 s) runs out
        net.run_for(2_600);
        assert_eq!(net.server.game().flights_in_air(), 0);
        for i in 0..3 {
            let t = created(&net, i);
            assert!(!t.objects.contains_key(&nade), "client {i}: removed");
            assert!(t.removed.contains_key(&nade), "client {i}: removal record");
            let blasts: Vec<(u16, u16, u16)> = net
                .c(i)
                .seen
                .effects
                .iter()
                .filter(|e| e.1 == 0x06)
                .map(|e| {
                    let mut r = e.2.reader();
                    (r.read_u16(), r.read_u16(), r.read_u16())
                })
                .collect();
            assert_eq!(
                blasts,
                vec![(0xFFFF, net.obj(0), 40)],
                "ff {ff}, client {i}: one blast effect"
            );
        }
        // the object rests on the box floor (64 below the players): DSV-39 amended falloff, full inside the inner
        // quarter (50), then linear: 80 x (200 - 64) / 150 = 72.5
        let floor = net.server.world().extent_min[1];
        let d = thrower[1] - floor;
        let want = (80.0
            * dc_dserver::game::blast_factor(d, 200.0, dc_dserver::game::BLAST_INNER_FRACTION))
        .round() as u32;
        assert_eq!(want, 73);
        let dmg = |n: &Net, i: usize| n.c(i).seen.damage.iter().map(|x| x.1).collect::<Vec<_>>();
        assert_eq!(dmg(&net, 0), vec![want], "ff {ff}: the thrower is in range");
        assert_eq!(
            net.c(0).seen.damage_forms[0].zone,
            5,
            "the blast's damage type"
        );
        assert!(
            dmg(&net, 1).is_empty(),
            "ff {ff}: the enemy is out of range"
        );
        if ff {
            assert_eq!(dmg(&net, 2), vec![want], "the team mate with friendly fire");
        } else {
            assert!(dmg(&net, 2).is_empty(), "no friendly fire");
        }
        assert!(net.logs.iter().any(|l| l.contains("blew up")));
    }
}

/// Throws a grenade of [`throw_tables`] with the given projectile-stream switch (`None`: the default) and runs it
/// past its fuse. Returns the net, the id of the thrown object and, per client, the stream entries that named it and
/// the last velocity streamed for it (both read just before the fuse).
fn throw_run(stream: Option<bool>) -> (Net, u16, Vec<(u32, [f32; 3])>) {
    let mut cfg = ServerTuning::default();
    if let Some(s) = stream {
        cfg.game.projectile_stream = s;
    }
    // the throwable is granted (0xb2 / 0xb3 carry its count)
    cfg.game.default_loadout = vec![40];
    let mut net = Net::with(cfg, throw_tables(), "room key=TDM type=1 goal=20 ff=0");
    assert!(net.server.handle_line(0, "user 9 5 10 0 0 Cid"));
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    net.add(3, "Cid", 0.0);
    assert!(net.run_until(20_000, |n| (0..3).all(|i| n.c(i).life_state() == 1)));
    net.run_for(500);
    // a throw at the own feet: it falls to the box floor over several ticks, bounces and comes to rest (the box
    // floor keeps a horizontal velocity, so a throw along the floor would slide until the fuse)
    net.cm(0).throw((40, 41), [0.0, -1.0, 0.0]);
    net.run_for(150);
    let mut t = dc_botclient::objects::ObjectTable::default();
    for g in &net.c(1).seen.groups {
        t.apply_group(g);
    }
    let nade = *t
        .objects
        .iter()
        .find(|(_, o)| o.blob_bits == 32)
        .expect("the thrown object")
        .0;
    // the removal record drops the client's entry for the object, so the entries are read just before the fuse
    // (1.8 s after the release) and the last position seen in flight is kept
    net.run_for(1_400);
    assert_eq!(net.server.game().flights_in_air(), 1, "before the fuse");
    let entries = (0..3)
        .map(|i| {
            net.c(i)
                .seen
                .remote
                .get(&nade)
                .map_or((0, [0.0; 3]), |r| (r.entries, r.vel))
        })
        .collect();
    net.run_for(1_200);
    (net, nade, entries)
}

/// DSV-41 (amended) / UDP_SESSION_2009_B.md §3.5.4 Q6: by default every client that has the object gets one
/// position entry for it in every tick from the create to the removal, as the reference stack streamed it: with the velocity
/// while it falls, without it at rest (the resting object is not dropped from the stream). The object lands on the
/// box floor about 0.4 s after the release, the entries are read 1.55 s after it: at 10 Hz about 15 ticks, most of
/// them at rest. `-projectile-stream off` sends none.
// SPEC: UDP_SESSION_2009_B.md §3.5.4
// OBSERVED: capture 20261025_095358_rust4 (no entries: the throw was not shown)
#[test]
fn a_thrown_object_is_streamed_from_create_to_removal() {
    let (_, _, off) = throw_run(Some(false));
    assert!(
        off.iter().all(|e| e.0 == 0),
        "off: no entries for the object: {off:?}"
    );
    let (net, nade, on) = throw_run(None);
    for (i, (n, vel)) in on.iter().enumerate() {
        assert!(
            *n >= 12,
            "client {i}: {n} entries (one per tick, rest included)"
        );
        assert_eq!(*vel, [0.0; 3], "client {i}: the rest form has no velocity");
        assert!(
            net.c(i).seen.errors.is_empty(),
            "{:?}",
            net.c(i).seen.errors
        );
        assert!(
            net.c(i).seen.removed.contains(&nade),
            "client {i}: removed at the fuse"
        );
    }
}

/// Q9: the release is answered with `0x66` kind 1 to the thrower only: the throwable's ids, the rounds left in the
/// clip mirror (the clip of 1 is spent: 0.0, the reference stack's value for its grenade), the slot, and `extra` 1.
#[test]
fn the_release_of_a_throw_answers_the_thrower_with_the_grenade_count() {
    let (net, _, _) = throw_run(None);
    let notes = |i: usize| net.c(i).seen.ammo_notes.clone();
    let n0 = notes(0);
    assert_eq!(n0.len(), 1, "one note for the thrower: {n0:?}");
    let (_, w, a, v, _slot, extra) = n0[0];
    assert_eq!((w, a, v, extra), (40, 41, 0.0, 1));
    // the grant of a single grenade is 1.0 (the reference stack's give for its weapon 7)
    let given: Vec<f32> = net
        .c(0)
        .seen
        .weapon_amounts
        .iter()
        .filter(|x| x.0 == 40)
        .map(|x| x.1)
        .collect();
    assert!(
        !given.is_empty() && given.iter().all(|v| *v == 1.0),
        "{given:?}"
    );
    assert!(notes(1).is_empty() && notes(2).is_empty(), "nobody else");
    assert!(
        net.c(0).seen.errors.is_empty(),
        "{:?}",
        net.c(0).seen.errors
    );
}

/// DSV-39 (amended): a 2-pack of a weapon that is its own ammunition (`IsAmmo`, `ShotsPerClip` 1, `MaxAmount` 2) is
/// granted as 2.0 (one in the hand, one in the stock), the first release is answered with 1.0 (the client then
/// reloads its hand from the stock, `0x83` in the release block as in rust4), the second with 0.0, and a third
/// release is refused (no object, no note): two throws, not three.
// OBSERVED: capture 20261025_095358_rust4 (item 143: give 2.0, release + 0x83, our 0.0 left the
// hand empty); reference tdm4 / tdm6 (single grenade: give 1.0, after the throw 0.0)
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (0x66 kind 1 sets the slot's count)
#[test]
fn a_two_pack_counts_down_one_per_throw_and_refuses_a_third() {
    let mut cfg = ServerTuning::default();
    cfg.game.default_loadout = vec![40];
    let mut net = Net::with(
        cfg,
        throw_tables_with(2),
        "room key=TDM type=1 goal=20 ff=0",
    );
    assert!(net.server.handle_line(0, "user 9 5 10 0 0 Cid"));
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    net.add(3, "Cid", 0.0);
    assert!(net.run_until(20_000, |n| (0..3).all(|i| n.c(i).life_state() == 1)));
    net.run_for(500);
    let given: Vec<f32> = net
        .c(0)
        .seen
        .weapon_amounts
        .iter()
        .filter(|x| x.0 == 40)
        .map(|x| x.1)
        .collect();
    assert!(
        !given.is_empty() && given.iter().all(|v| *v == 2.0),
        "the grant counts both grenades: {given:?}"
    );
    // all three releases before the first fuse (1.8 s): the blasts do not interfere
    net.cm(0).throw((40, 41), [1.0, 0.0, 0.0]);
    net.cm(0).reload_ammo(41);
    net.run_for(150);
    net.cm(0).throw((40, 41), [0.0, 0.0, 1.0]);
    net.run_for(150);
    net.cm(0).throw((40, 41), [-1.0, 0.0, 0.0]);
    net.run_for(150);
    let counts: Vec<f32> = net.c(0).seen.ammo_notes.iter().map(|n| n.3).collect();
    assert_eq!(counts, vec![1.0, 0.0], "one note per accepted throw");
    assert_eq!(
        net.server.game().flights_in_air(),
        2,
        "the third throw is refused"
    );
    assert!(
        net.logs.iter().any(|l| l.contains("beyond the clip")),
        "the refusal is logged"
    );
    assert!(
        net.c(0).seen.errors.is_empty(),
        "{:?}",
        net.c(0).seen.errors
    );
}

/// The user's 2026-11-04 report: "sometimes grenades right after throwing instantly explode". A grenade held through a
/// sprint is let go with no 0x4d for the whole sprint (CLI-573), so its release comes more than the 1000 ms hold window
/// (DSV-29) after the last 0x4d. With the throwable's ammunition `Type` 2 (a thrown round, as the ROM grenade rows)
/// the release is still the throw (DSV-29 amendment): a flight, the blast only at the fuse. A row without a `Type`
/// keeps the window: the late release is a shot, whose effect record every client draws as the grenade's explosion
/// at once (the bug the user saw).
#[test]
fn a_grenade_released_long_after_its_last_hold_still_flies() {
    for kind in [Some(dc_dserver::rom_tables::AMMO_KIND_THROWN), None] {
        let mut tables = throw_tables();
        tables.weapons.ammo.get_mut(&41).unwrap().kind = kind;
        let mut cfg = ServerTuning::default();
        cfg.game.default_loadout = vec![40];
        let mut net = Net::with(cfg, tables, "room key=TDM type=1 goal=20 ff=0");
        assert!(net.server.handle_line(0, "user 9 5 10 0 0 Cid"));
        net.add(1, "Dave", 0.0);
        net.add(2, "Big Erin", 0.0);
        net.add(3, "Cid", 0.0);
        assert!(net.run_until(20_000, |n| (0..3).all(|i| n.c(i).life_state() == 1)));
        net.run_for(500);
        // the pin pulled, then a 1.5 s sprint without 0x4d, then the key goes up
        net.cm(0).throw_hold();
        net.run_for(1_500);
        let before = net.c(1).seen.effects.len();
        net.cm(0).throw_release((40, 41), [1.0, 0.0, 0.0]);
        net.run_for(200);
        let thrower = net.obj(0);
        let records = |n: &Net, from: usize| -> Vec<u16> {
            n.c(1).seen.effects[from..]
                .iter()
                .filter(|e| e.1 == 0x06)
                .filter_map(|e| {
                    let mut r = e.2.reader();
                    let _first = r.read_u16();
                    let shooter = r.read_u16();
                    let weapon = r.read_u16();
                    (shooter == thrower && weapon == 40).then_some(weapon)
                })
                .collect()
        };
        if kind.is_some() {
            assert_eq!(net.server.game().flights_in_air(), 1, "a flight");
            assert!(records(&net, before).is_empty(), "no record at the release");
            net.run_for(2_400);
            assert_eq!(net.server.game().flights_in_air(), 0, "blown at the fuse");
            assert_eq!(records(&net, before).len(), 1, "one blast record");
        } else {
            assert_eq!(net.server.game().flights_in_air(), 0, "no flight");
            assert_eq!(
                records(&net, before).len(),
                1,
                "a shot record of the grenade at once"
            );
        }
    }
}

// ------------------------------------------------------------------------------------------------- co-op (class 5)

const COOP_ROOM: &str = "room key=COOP type=5 goal=1 ff=0 time=0";

/// A co-op room on the synthetic world with made-up spawners and one AI template (no ROM): the AIs fall back to
/// the default player character of the synthetic character tables and carry the template's right-hand weapon.
fn coop_net(cfg: ServerTuning, room: &str) -> Net {
    use dc_dserver::world::{AiTemplate, CoopSpawner, CoopWorld};
    let mut w = world();
    let spawner = |name: &[u8], pos| CoopSpawner {
        name: name.to_vec(),
        pos,
        rot: [0.0; 3],
        target: Some(b"tmpl".to_vec()),
    };
    w.coop = CoopWorld {
        spawners: vec![
            spawner(b"s0", [300.0, 0.0, 300.0]),
            spawner(b"s1", [300.0, 0.0, -300.0]),
            spawner(b"s2", [350.0, 0.0, 0.0]),
        ],
        templates: vec![AiTemplate {
            name: b"tmpl".to_vec(),
            pos: [0.0; 3],
            rot: [0.0; 3],
            body_row: None,
            right_hand: Some("G3".into()),
            max_health: Some(60.0),
            template: true,
            death_cmd: Vec::new(),
            ..Default::default()
        }],
        nodes: vec![[0.0, 0.0, 0.0]],
        mission: false,
        rounds: None,
        ..CoopWorld::default()
    };
    let mut server = Server::with_tables(cfg, w, character_tables(), 0);
    server.insert_rig("BODY\\b.ltb", rig());
    assert!(server.handle_line(0, room));
    assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
    assert!(server.handle_line(0, "user 8 4 55 1 0 Big_Erin"));
    Net {
        server,
        peers: Vec::new(),
        pipe: Vec::new(),
        logs: Vec::new(),
        now: 0,
        delay_ms: 2,
        look: None,
    }
}

fn coop_add(net: &mut Net, addr: u32, name: &str) {
    // the clients learn class 5 from 0x9e (the TDM hint of `add` only matters before it)
    net.add(addr, name, 0.0);
}

/// The NPC record of a create blob as the client reads it (FIRETEAM_2009.md §1.2): `roster_bound`, `has_tail`, the
/// `pid` byte, then the tail T1-T5.
fn npc_record(b: &BitWriter) -> (bool, bool, u8, u8, u8, u8, f32, Vec<u8>) {
    let mut r = b.reader();
    assert_eq!(r.read_u8(), 0x18);
    let (rb, tail) = (r.read_bit(), r.read_bit());
    r.skip_bits(5 * 8);
    let pid = r.read_u8();
    r.skip_bits(64 + 6 * 32 + 96 + 96 + 32 + 32 + 1 + 8 + 1);
    let (rank, team, kind) = (r.read_u8(), r.read_u8(), r.read_u8());
    let health = f32::from_bits(r.read_u32());
    let caption = r.read_zstr8();
    assert!(r.at_end(), "nothing after the caption");
    (rb, tail, pid, rank, team, kind, health, caption)
}

fn coop_cfg(coop: dc_dserver::game::coop::CoopTuning) -> ServerTuning {
    ServerTuning {
        game: GameTuning {
            default_loadout: vec![3, 4],
            respawn_wait_ms: Some(1_000),
            coop,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    }
}

/// Co-op slice 1 (FT-C1, FT-K1, FT-W1, FT-R1, FT-L1 proposed): the class-5 start set (0x9e class 5, 0x7d goal = the
/// difficulty, the own 0x6b `m1` = the lives before anything else, 0xd7 sub 2 after the spawn, 0xc9 wave list and
/// banner); each AI is a kind-1 character with the type-0x18 record `roster_bound` 0 / `has_tail` 1 / `pid` 0xff and
/// the tail (rank 0, team 2, kind 1, health 1.005, caption), body and weapon at socket 2, three trackers in the stream;
/// a hit by the server's ray test and by `hit_obj` lowers the bar (0xa6 sub 0x24, the truncation shows the percent),
/// points by 0xd7 sub 3 and the kill by sub 1; a dead AI is hidden at the park point and its objects are reused (no
/// removal record, no third object for a pool of two); the cleared wave ends the mission with the complete banner.
#[test]
fn coop_ai_created_hit_killed_hidden_and_reused() {
    use dc_dserver::game::coop::{AiState, CoopTuning};
    let cfg = coop_cfg(CoopTuning {
        max_alive: 2,
        waves: 1,
        lives: Some(3),
        first_wave_ms: 500,
        spawn_interval_ms: 100,
        corpse_ms: 400,
        hit_chance: [0.0; 3],
        ..CoopTuning::default()
    });
    let mut net = coop_net(cfg, COOP_ROOM);
    coop_add(&mut net, 1, "Dave");
    coop_add(&mut net, 2, "Big Erin");
    assert!(
        net.run_until(20_000, |n| n.c(0).life_state() == 1
            && n.c(1).life_state() == 1),
        "states {} {}",
        n_state(&net, 0),
        n_state(&net, 1)
    );
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
        assert_eq!(s.endpoint, Some(5), "0x9e mode class 5");
        assert_eq!(s.settings.map(|x| x.1), Some(1), "0x7d goal = difficulty 1");
        assert_eq!(s.own_team, Some(1), "every human on team 1");
        // the own 0x6b `m1` first, then 0xd7 sub 2 after the spawn
        assert_eq!(s.lives_log.first().map(|x| (x.1, x.2)), Some((0x6b, 3)));
        assert!(s.lives_log.iter().any(|x| x.1 == 0xd7 && x.2 == 3));
    }
    // the first wave
    assert!(net.run_until(net.now + 10_000, |n| n
        .server
        .game()
        .ai_chars()
        .iter()
        .any(|a| a.obj.is_some())));
    // past the first AI's fire interval
    net.run_for(1_000);
    let first = net.server.game().ai_chars()[0].clone();
    let obj = first.obj.unwrap();
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
        assert_eq!(
            s.missions.first().map(|m| (m.2, m.3)),
            Some((1, 1)),
            "0xc9 sub 1 wave 1, goal 1"
        );
        assert_eq!(
            s.banners.first().map(|b| b.as_slice()),
            Some(&b"Wave 1 / 1"[..])
        );
        let o = &s.objects[&obj];
        assert_eq!(
            (o.kind, o.blob_type, o.blob_pid),
            (1, Some(0x18), Some(0xff))
        );
        let (rb, tail, pid, rank, team, kind, health, caption) =
            npc_record(o.blob.as_ref().unwrap());
        assert_eq!(
            (rb, tail, pid, rank, team, kind),
            (false, true, 0xff, 0, 2, 1)
        );
        assert_eq!((health as f64 * 100.0) as u32, 100, "full bar");
        assert_eq!(caption, b"AI".to_vec());
        assert_eq!(o.flags, Some(0x3803), "visible");
        assert_eq!(o.dims, Some([15.0, 50.0, 15.0]));
        assert_eq!(o.attached.len(), 1);
        assert_eq!(o.attached[0].1, 2, "weapon at socket 2");
        assert!(s
            .remote
            .get(&obj)
            .is_some_and(|r| r.anim_entries > 0 && r.yaw8_entries > 0));
        // AI shots are shot effects with the AI's character as the shooter
        let shot_by_ai = s.effects.iter().any(|e| {
            let mut r = e.2.reader();
            e.1 == 0x06 && {
                r.read_u16();
                r.read_u16() == obj
            }
        });
        assert!(shot_by_ai, "client {i}: an AI shot effect");
    }
    // two shots that name no object: the server's ray test finds the AI on the line
    let before = net.server.game().ai_chars()[0].health;
    for _ in 0..2 {
        assert!(net.cm(0).fire_at(obj, [0.0; 3], false));
        net.run_for(30);
    }
    let after = net.server.game().ai_chars()[0].health;
    assert!(after < before, "ray hits: {before} -> {after}");
    let p0 = net.pid(0) as u32;
    assert!(
        net.c(1).seen.coop_notes.iter().any(|n| n.1 == p0),
        "0xd7 sub 3 points"
    );
    assert!(net
        .c(1)
        .seen
        .npc_health
        .iter()
        .any(|h| h.1 == obj && h.2 < 1.0));
    // kill every AI of the wave with reported hits; watch the pool: hidden, then shown again
    let mut hidden_seen = std::collections::BTreeSet::new();
    let mut reshown = std::collections::BTreeSet::new();
    let mut corpse_seen = false;
    let end = net.now + 60_000;
    while net.now < end && net.c(0).seen.coop_result.is_none() {
        let alive: Vec<u16> = net
            .server
            .game()
            .ai_chars()
            .iter()
            .filter(|a| a.state == AiState::Alive)
            .filter_map(|a| a.obj)
            .collect();
        for o in alive {
            net.cm(0).fire_at(o, [0.0; 3], true);
        }
        net.run_for(40);
        for (o, so) in &net.c(1).seen.objects {
            corpse_seen |= so.blob_type == Some(0x26);
            if so.blob_pid == Some(0xff) {
                if so.flags == Some(0x2802) {
                    hidden_seen.insert(*o);
                } else if so.flags == Some(0x3803) && hidden_seen.contains(o) {
                    reshown.insert(*o);
                }
            }
        }
    }
    let s = &net.c(1).seen;
    assert!(s.errors.is_empty(), "{:?}", s.errors);
    let (stage, values, names) = s.coop_result.clone().expect("mission over");
    assert_eq!(stage, 2, "complete banner");
    assert_eq!(values[1], 1, "waves cleared");
    assert_eq!(
        values[2], 6,
        "AIs killed (wave 1 of the synthetic plan: 4 + 2)"
    );
    assert_eq!(names.len(), 2);
    let ai_objs: Vec<u16> = s
        .objects
        .iter()
        .filter(|(_, o)| o.blob_pid == Some(0xff))
        .map(|(o, _)| *o)
        .collect();
    assert_eq!(
        ai_objs.len(),
        2,
        "a pool of two character objects for six AIs"
    );
    assert!(
        ai_objs.iter().all(|o| !s.removed.contains(o)),
        "never removed"
    );
    assert!(!hidden_seen.is_empty(), "a dead AI was hidden");
    // DSV-585: a corpse lies where an AI fell
    assert!(corpse_seen, "an AI corpse object");
    assert!(
        !reshown.is_empty(),
        "and its object shown again for the next AI"
    );
    let kills: Vec<_> = s
        .coop_points
        .iter()
        .filter(|p| p.1 == p0)
        .cloned()
        .collect();
    assert_eq!(kills.len(), 6, "0xd7 sub 1 per kill");
    assert!(
        kills.windows(2).all(|w| w[1].4 > w[0].4),
        "the points total grows"
    );
    // each kill emptied the bar
    assert!(s.npc_health.iter().filter(|h| h.2 == 0.0).count() >= 6);
    // the summary's co-op block: points, kills, points, deaths
    net.run_for(4_000);
    let row = net
        .c(0)
        .seen
        .coop_summaries
        .iter()
        .find(|r| r.0 == p0 as u8)
        .copied();
    let (_, c) = row.expect("co-op summary block");
    assert_eq!((c[1], c[3]), (6, 0));
    assert_eq!(c[0], c[2]);
    assert!(c[0] >= kills.last().unwrap().4);
    assert!(
        net.c(0).seen.errors.is_empty(),
        "{:?}",
        net.c(0).seen.errors
    );
}

/// FT-L1 / FIRETEAM_2009.md §2.3: with 2 lives, the first death sends 0xd7 sub 2 [1][1] before 0x67 state 2 (the
/// client holds 1 when it processes the death and arms its wait; one 0x89; the server respawns it and sends [1][1]
/// again); the second death sends [0][0] first: no wait is armed, no request, no respawn; with every human out the
/// mission fails (banner stage 3) and the result counts four deaths.
#[test]
fn coop_lives_arrive_before_the_death_and_end_the_respawns() {
    use dc_dserver::game::coop::CoopTuning;
    let cfg = coop_cfg(CoopTuning {
        max_alive: 1,
        waves: 1,
        lives: Some(2),
        first_wave_ms: 300,
        hit_chance: [1.0; 3],
        fire_ms: 100,
        shot_range: 4000.0,
        default_damage: 60.0,
        ..CoopTuning::default()
    });
    let mut net = coop_net(cfg, COOP_ROOM);
    coop_add(&mut net, 1, "Dave");
    coop_add(&mut net, 2, "Big Erin");
    assert!(net.run_until(60_000, |n| n.c(0).seen.coop_result.is_some()));
    net.run_for(500);
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
        assert_eq!(s.lives_at_death, vec![Some(1), Some(0)], "client {i}");
        assert_eq!(
            s.respawn_armed.len(),
            1,
            "client {i}: armed after the first death only"
        );
        assert_eq!(s.respawn_requests.len(), 1, "client {i}");
        let ones = s.life_states.iter().filter(|x| x.1 == 1).count();
        assert_eq!(ones, 2, "client {i}: the first spawn and one respawn");
        assert_ne!(
            s.life_states.last().map(|x| x.1),
            Some(1),
            "client {i}: stays dead"
        );
        let lives: Vec<(u8, u8)> = s.lives_log.iter().map(|x| (x.1, x.2)).collect();
        assert_eq!(lives[0], (0x6b, 2), "client {i}: {lives:?}");
        let subs: Vec<u8> = lives.iter().filter(|l| l.0 == 0xd7).map(|l| l.1).collect();
        assert_eq!(
            subs,
            vec![2, 1, 1, 0],
            "client {i}: spawn, death, respawn, death"
        );
        assert!(lives.contains(&(0xd7, 1)) && lives.contains(&(0xd7, 0)));
        assert!(
            !lives
                .iter()
                .skip_while(|l| *l != &(0xd7, 0))
                .any(|l| l.1 > 0),
            "no life back"
        );
        let (stage, values, _) = s.coop_result.clone().unwrap();
        assert_eq!(stage, 3, "failed banner");
        assert_eq!(values[3], 4, "four human deaths");
    }
    assert!(net
        .logs
        .iter()
        .all(|l| !l.contains("respawned by the server")));
}

// ------------------------------------------------------------------------------------------------- hit acceptance

/// One moving-target scenario: what fraction of correct hit claims the server accepts.
#[derive(Debug, Clone, Copy)]
struct HitCase {
    rewind_ms: u32,
    tick_hz: u32,
    /// One-way channel delay of both peers.
    delay_ms: u32,
    /// Units per update (15 updates per second): 19 = 285 u/s, the 2009 run speed (MOVEMENT_2009.md §2 `RUN_FWD`).
    step: f32,
    view: View,
}

/// Fires 200 correct claims (aimed at the target as the shooter shows it, hit points spread over the near half of
/// its box) from a standing shooter at a target that walks back and forth along x, and returns the verdicts.
fn hit_case(c: HitCase) -> HitCounters {
    let cfg = ServerTuning {
        tick_hz: c.tick_hz,
        game: GameTuning {
            start_health: 1.0e9,
            max_health: 1.0e9,
            default_damage: 1.0,
            default_clip: 0,
            rewind_ms: c.rewind_ms,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::new(cfg, "room key=TDM type=1 goal=10 ff=0");
    net.delay_ms = c.delay_ms;
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", c.step);
    // back and forth between x = 300 and about 500: `pos48` cannot carry a position outside the world box (x 512 in
    // the synthetic world)
    net.cm(1).walk_period((200.0 / c.step) as u32);
    assert!(
        net.run_until(20_000, |n| n.c(0).life_state() == 1
            && n.c(1).life_state() == 1),
        "states {} {}",
        n_state(&net, 0),
        n_state(&net, 1)
    );
    net.run_for(1_000);
    let target = net.obj(1);
    for i in 0..200u32 {
        // the near face (the shooter stands at -x) and spread over the body: x -22..-2, y -45..+45, z -20..+20
        let offset = [
            -2.0 - (i * 7 % 21) as f32,
            -45.0 + (i * 37 % 91) as f32,
            -20.0 + (i * 13 % 41) as f32,
        ];
        let now = net.now;
        assert!(net.cm(0).fire_at_view(target, offset, now, c.view));
        net.run_for(50);
    }
    net.run_for(500);
    if std::env::var_os("DC_HIT_SIM_LOG").is_some() {
        for l in net
            .logs
            .iter()
            .filter(|l| l.contains("refused") || l.contains("rejected"))
        {
            println!("  {l}");
        }
    }
    net.server.game().hit_counters().clone()
}

#[test]
fn moving_target_claims_are_accepted_with_the_rewind() {
    let base = HitCase {
        rewind_ms: 200,
        tick_hz: 30,
        delay_ms: 2,
        step: 19.0,
        view: View::Raw,
    };
    let cases: Vec<(&str, HitCase)> = vec![
        (
            "loopback, raw view, no rewind",
            HitCase {
                rewind_ms: 0,
                ..base
            },
        ),
        ("loopback, raw view, rewind 200", base),
        (
            "60 ms each way, 100 ms interpolation, no rewind",
            HitCase {
                rewind_ms: 0,
                delay_ms: 60,
                view: View::Interpolate(100),
                ..base
            },
        ),
        (
            "60 ms each way, 100 ms interpolation, rewind 200",
            HitCase {
                delay_ms: 60,
                view: View::Interpolate(100),
                ..base
            },
        ),
        (
            "60 ms each way, 100 ms interpolation, tick 10, rewind 200",
            HitCase {
                delay_ms: 60,
                tick_hz: 10,
                view: View::Interpolate(100),
                ..base
            },
        ),
        (
            "60 ms each way, extrapolated view, no rewind",
            HitCase {
                rewind_ms: 0,
                delay_ms: 60,
                view: View::Extrapolate,
                ..base
            },
        ),
        (
            "60 ms each way, extrapolated view, rewind 200",
            HitCase {
                delay_ms: 60,
                view: View::Extrapolate,
                ..base
            },
        ),
        (
            "sprint 29 u/update, 60 ms each way, 100 ms interpolation, rewind 200",
            HitCase {
                delay_ms: 60,
                step: 29.0,
                view: View::Interpolate(100),
                ..base
            },
        ),
        (
            "sprint 29 u/update, 60 ms each way, 100 ms interpolation, no rewind",
            HitCase {
                rewind_ms: 0,
                delay_ms: 60,
                step: 29.0,
                view: View::Interpolate(100),
                ..base
            },
        ),
        (
            "120 ms each way, 100 ms interpolation, no rewind",
            HitCase {
                rewind_ms: 0,
                delay_ms: 120,
                view: View::Interpolate(100),
                ..base
            },
        ),
        (
            "120 ms each way, 100 ms interpolation, rewind 200",
            HitCase {
                delay_ms: 120,
                view: View::Interpolate(100),
                ..base
            },
        ),
        (
            "sprint 29 u/update, 120 ms each way, 100 ms interpolation, rewind 200",
            HitCase {
                delay_ms: 120,
                step: 29.0,
                view: View::Interpolate(100),
                ..base
            },
        ),
    ];
    let mut rates = Vec::new();
    for (name, c) in &cases {
        let h = hit_case(*c);
        let rate = h.accepted() as f64 / h.claims().max(1) as f64;
        println!("{name}: {} ({:.1} %)", h.summary(), rate * 100.0);
        assert_eq!(h.claims(), 200, "{name}: every claim gets a verdict");
        rates.push(rate);
    }
    // a standing-near-loopback shooter is accepted either way
    assert!(rates[0] >= 0.99 && rates[1] >= 0.99, "{rates:?}");
    // realistic latency and interpolation: the latest position alone refuses many claims; the rewind accepts >= 90 %
    assert!(rates[2] < 0.9, "{rates:?}");
    for i in [3, 4, 6, 7, 10] {
        assert!(rates[i] >= 0.9, "case {}: {rates:?}", cases[i].0);
    }
    // the rewind never accepts less than the latest position alone
    assert!(rates[3] >= rates[2] && rates[7] >= rates[8] && rates[10] >= rates[9]);
}

#[test]
fn rewind_follows_the_victims_path_only() {
    // a claim at a point the target never was (beyond its turning point) is refused even with the rewind
    let cfg = ServerTuning {
        tick_hz: 30,
        game: GameTuning {
            start_health: 1.0e9,
            max_health: 1.0e9,
            default_damage: 1.0,
            rewind_ms: 200,
            verbose_hits: true,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::new(cfg, "room key=TDM type=1 goal=10 ff=0");
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 19.0);
    net.cm(1).walk_period(10);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(1_000);
    let target = net.obj(1);
    let x = net.c(1).position()[0];
    // 150 units beyond the far end of its walk (x 300..490) or before the near end: never on its path
    let ahead = [
        if x > 395.0 {
            490.0 + 150.0 - x
        } else {
            300.0 - 150.0 - x
        },
        0.0,
        0.0,
    ];
    assert!(net.cm(0).fire_at(target, ahead, true));
    net.run_for(200);
    let h = net.server.game().hit_counters().clone();
    assert_eq!(h.get(HitVerdict::OutsideBox), 1, "{}", h.summary());
    assert!(net.logs.iter().any(|l| l.contains("refused (outside_box)")));
    assert!(net.c(1).seen.damage.is_empty());
}

/// DSV-33 correction and DSV-44 (synthetic character tables, reference order): the match's first kill alone carries
/// `streak_flag` 1 (a later kill that is its killer's first since the spawn carries 0); at a death every client sees
/// the victim's character hidden (flags 0x0802) with an empty attachment list, its weapon model removed and a
/// corpse object (kind 1, type-0x26 blob, the body model, the death clip on the main tracker) at the victim's
/// position; the corpse goes after `corpse_ms`; at the respawn the weapon model is created again and attached and
/// the character is visible (0x3803); a client that joins while a player lies dead creates it hidden and without
/// a weapon model; a second reader (dc-botclient's object table) parses every group.
#[test]
fn first_kill_flag_and_the_death_presentation() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![3, 4],
            default_damage: 200.0,
            respawn_wait_ms: Some(6_000),
            corpse_ms: 4_000,
            // shots right after a spawn: the shield (DSV-156) is not under test here
            spawn_shield: false,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(
        cfg,
        character_tables(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
    );
    net.server.insert_rig("BODY\\b.ltb", rig());
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(1500);
    let (dave, erin) = (net.obj(0), net.obj(1));
    let erin_weapon = net.c(0).seen.objects[&erin].attached[0].0;
    let before: Vec<u16> = net.c(0).seen.objects.keys().copied().collect();
    let erin_files = net.c(0).seen.objects[&erin].models.clone();
    // 1) Dave kills Erin with a body shot
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(100);
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
        assert_eq!(s.kills.len(), 1);
        assert_eq!(s.kills[0].streak_flag, 1, "the match's first kill");
        let ch = &s.objects[&erin];
        assert_eq!(ch.flags, Some(0x0802), "client {i}: hidden at the death");
        assert!(
            ch.attached.is_empty(),
            "client {i}: attachment list emptied"
        );
        assert!(
            !s.objects.contains_key(&erin_weapon) && s.removed.contains(&erin_weapon),
            "client {i}: weapon model removed"
        );
        let corpses: Vec<(&u16, &dc_dserver::synth_client::SeenObject)> = s
            .objects
            .iter()
            .filter(|(o, x)| !before.contains(o) && x.blob_type == Some(0x26))
            .collect();
        assert_eq!(corpses.len(), 1, "client {i}: one corpse");
        let c = corpses[0].1;
        assert_eq!(c.kind, 1);
        assert_eq!(c.flags, Some(0x1803));
        // CLI-585: the corpse wears the victim's body and face entries (the head is the face model)
        assert_eq!(
            c.models, erin_files,
            "client {i}: the corpse keeps its head"
        );
        assert_eq!(c.blob.as_ref().map(|b| b.bit_len()), Some(219));
        assert!(
            c.anim == vec![5] || c.anim == vec![6],
            "client {i}: a body death clip {:?}",
            c.anim
        );
        let p = c.pos.expect("corpse position");
        assert!(
            (p[0] - BLUE[0]).abs() < 2.0 && (p[2] - BLUE[2]).abs() < 2.0,
            "client {i}: corpse at the victim {p:?}"
        );
    }
    let corpse = *net
        .c(0)
        .seen
        .objects
        .iter()
        .find(|(_, x)| x.blob_type == Some(0x26))
        .unwrap()
        .0;
    // DSV-44 amendment: the dead character streams the corpse's clip on its main tracker, not looping, its time
    // counting from the death and held at the clip's end (reference tdm4 seq 3135 .. 3510)
    let clip = net.c(0).seen.objects[&corpse].anim[0];
    let len = if clip == 5 { 1333 } else { 1433 };
    let main = |net: &Net| {
        net.c(0).seen.remote[&erin]
            .last_main
            .expect("an animation block")
    };
    let (a, s0, s1, t1) = main(&net);
    assert_eq!(
        (a, s0, s1),
        (clip, true, false),
        "the corpse's clip, not looping"
    );
    assert!(t1 <= 200, "time from the death on: {t1}");
    net.run_for(500);
    let (_, _, s1, t2) = main(&net);
    assert!(
        !s1 && t2 > t1 + 300 && t2 < 800,
        "the clip runs: {t1} -> {t2}"
    );
    net.run_for(1_500);
    let held = u32::div_ceil(len, 8) * 8;
    assert_eq!(
        main(&net),
        (clip, true, false, held),
        "held at the clip's end"
    );
    // the dead player's character takes no damage and the dead state holds
    net.shoot(0, 1, 1, [0.0; 3]);
    assert_eq!(net.c(1).seen.kills.len(), 1);
    // 2) a client joining now creates Erin hidden and without her weapon model
    let t = net.now;
    assert!(net.server.handle_line(t, "user 9 5 77 0 0 Carl"));
    net.add(3, "Carl", 0.0);
    assert!(net.run_until(t + 2_000, |n| n.c(2).seen.objects.contains_key(&erin)));
    {
        let s = &net.c(2).seen;
        assert_eq!(s.objects[&erin].flags, Some(0x2802));
        assert!(s.objects[&erin].attached.is_empty());
        assert!(!s.objects.contains_key(&erin_weapon));
    }
    // 3) Erin respawns (6 s wait): the weapon model is created again and attached, the character visible
    assert!(net.run_until(net.now + 5_000, |n| n.c(1).life_state() == 1));
    net.run_for(300);
    for i in 0..3 {
        let s = &net.c(i).seen;
        assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
        let ch = &s.objects[&erin];
        assert_eq!(ch.flags, Some(0x3803), "client {i}: visible again");
        if i != 1 {
            assert_eq!(ch.attached, vec![(erin_weapon, 2)], "client {i}");
            assert_eq!(s.objects[&erin_weapon].flags, Some(0x0003), "client {i}");
        }
        // DSV-44 amendment: the respawn change in the reference stack's form (wire `cf` 0x2ba8: files, trackers, flags, the
        // position by CF_POS_SNAP alone, attachments; tdm6 seq 10964), the trackers from their start
        if i != 2 {
            assert_eq!(
                ch.last_cf | 0x80,
                0x2ba8,
                "client {i}: the respawn mask {:#06x}",
                ch.last_cf
            );
            assert_eq!(
                ch.anim,
                vec![0, 3, 1],
                "client {i}: main clip and the standing pair"
            );
            assert_eq!(
                ch.models, erin_files,
                "client {i}: the create's model files again"
            );
        }
        if let Some(r) = s.remote.get(&erin) {
            let (a, _, s1, _) = r.last_main.expect("an animation block");
            assert_eq!(
                (a, s1),
                (0, true),
                "client {i}: the looping main clip again"
            );
        }
    }
    // 4) the corpse goes after corpse_ms (4 s after the death)
    net.run_for(2_000);
    for i in 0..2 {
        assert!(
            net.c(i).seen.removed.contains(&corpse),
            "client {i}: corpse removed"
        );
    }
    // 5) Erin kills Dave: her first kill since her spawn is not the match's first kill
    net.shoot(1, 0, 1, [0.0; 3]);
    net.run_for(100);
    let k = &net.c(0).seen.kills;
    assert_eq!(k.len(), 2);
    assert_eq!((k[1].victim, k[1].streak_flag), (net.pid(0), 0));
    assert_eq!(net.c(1).seen.objects[&dave].flags, Some(0x0802));
    // DSV-91: one death note per death to the dead character, on every link in the world at the time (Carl joined
    // after the first death)
    for (i, want) in [
        (0, vec![erin, dave]),
        (1, vec![erin, dave]),
        (2, vec![dave]),
    ] {
        let got: Vec<u16> = net.c(i).seen.death_notes.iter().map(|n| n.1).collect();
        assert_eq!(got, want, "client {i}: death notes");
    }
    // a second, independent reader of the same object groups
    for i in 0..3 {
        let mut t = dc_botclient::objects::ObjectTable::default();
        for g in &net.c(i).seen.groups {
            t.apply_group(g);
        }
        assert!(t.errors.is_empty(), "client {i}: {:?}", t.errors);
    }
}

// ------------------------------------------------------------------------------------------------- HiSec case (HS-1..5)

/// [`tables`] with a synthetic HiSec case row (made-up file names in the ROM key shape).
fn hisec_tables() -> RomTables {
    use dc_dserver::rom_tables::{HisecRow, HisecRows};
    let mut t = tables();
    t.hisec = HisecRows {
        rows: [(
            0,
            HisecRow {
                model: Some("PROPS\\SynthCase\\Case.ltb".into()),
                skins: vec!["PROPS\\SynthCase\\Case.dtx".into()],
                styles: vec!["RS\\synth.ltb".into()],
                scale: Some([1.0; 3]),
            },
        )]
        .into_iter()
        .collect(),
    };
    t
}

/// Two players alive in a TDM room with the HiSec rules `h`.
fn hisec_net(h: dc_dserver::game::hisec::HisecTuning, tables: RomTables) -> Net {
    hisec_net_in(h, tables, "room key=TDM type=1 goal=10 ff=0")
}

fn hisec_net_in(h: dc_dserver::game::hisec::HisecTuning, tables: RomTables, room: &str) -> Net {
    let mut cfg = ServerTuning::default();
    cfg.game.start_order = StartOrder::Spec;
    cfg.game.hisec = h;
    let mut net = Net::with(cfg, tables, room);
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    net
}

/// The client's case objects: (object, case key) from the type-0x17 kind-5 blobs.
fn case_objects(c: &SynthClient) -> Vec<(u16, u32)> {
    c.seen
        .objects
        .iter()
        .filter_map(|(o, s)| {
            let b = s.blob.as_ref()?;
            let mut r = b.reader();
            if r.read_u8() != 0x17 {
                return None;
            }
            let flags = (r.read_bit(), r.read_bit(), r.read_bit());
            let name = r.read_zstr8();
            let (kind, key) = (r.read_u32(), r.read_u32());
            (flags == (true, false, false)
                && name.is_empty()
                && kind == 5
                && r.remaining_bits() == 0)
                .then_some((*o, key))
        })
        .collect()
}

fn hisec_seen(c: &SynthClient) -> Vec<(u8, u32, u32)> {
    c.seen.hisec.iter().map(|x| (x.1, x.2, x.3)).collect()
}

/// HS-1..HS-4 with the test flag: a kill drops case 1 at the victim (0xd6 sub 0, sub 2 for the killer, after the
/// create), the countdown runs 3, 2, 1 and the release sends sub 2 with nobody and 0; no second case while one is in
/// the world; the case is removed at the end of its lifetime and the next kill drops case 2.
#[test]
fn hisec_case_drops_counts_down_is_released_and_expires() {
    let h = dc_dserver::game::hisec::HisecTuning {
        force: true,
        reserve_ms: 3_000,
        lifetime_ms: 12_000,
        // 0: nobody can take it (the respawned victim stands on the case)
        pickup_radius: 0.0,
        ..Default::default()
    };
    let mut net = hisec_net(h, hisec_tables());
    let (p0, p1) = (net.pid(0), net.pid(1));
    net.shoot(0, 1, 4, [0.0; 3]);
    let t_drop = net.now;
    net.run_for(200);
    for i in 0..2 {
        let c = net.c(i);
        assert!(c.seen.errors.is_empty(), "client {i}: {:?}", c.seen.errors);
        let cases = case_objects(c);
        assert_eq!(cases.len(), 1, "client {i}: one case object");
        assert_eq!(cases[0].1, 1, "case key 1");
        assert_eq!(
            hisec_seen(c),
            vec![(0, 0, 0), (2, 1, u32::from(p0))],
            "client {i}"
        );
        // the countdown went to the created object
        assert_eq!(
            c.seen.case_notes.first().map(|n| (n.1, n.2)),
            Some((cases[0].0, 3))
        );
    }
    assert_eq!(net.server.game().hisec_cases().len(), 1);
    assert_eq!(net.server.game().hisec_cases()[0].2, Some(p0));
    net.run_until(t_drop + 3_500, |_| false);
    for i in 0..2 {
        let c = net.c(i);
        let left: Vec<u32> = c.seen.case_notes.iter().map(|n| n.2).collect();
        assert_eq!(left, vec![3, 2, 1, 0], "client {i}");
        assert_eq!(
            hisec_seen(c).last(),
            Some(&(2, 1, 0xFFFF_FFFF)),
            "client {i}: released"
        );
    }
    assert_eq!(net.server.game().hisec_cases()[0].2, None);
    // a second kill while case 1 lies in the world drops nothing (max_active 1)
    assert!(net.run_until(net.now + 9_000, |n| n.c(1).life_state() == 1));
    net.run_for(300);
    net.shoot(0, 1, 4, [0.0; 3]);
    net.run_for(200);
    assert_eq!(net.c(0).seen.hisec.iter().filter(|x| x.1 == 0).count(), 1);
    // lifetime: removed from both clients 12 s after the drop
    net.run_until(t_drop + 12_200, |_| false);
    assert!(net.server.game().hisec_cases().is_empty());
    for i in 0..2 {
        assert!(case_objects(net.c(i)).is_empty(), "client {i}: removed");
    }
    assert!(net.logs.iter().any(|l| l.contains("case 1 expired")));
    assert!(net.run_until(net.now + 9_000, |n| n.c(1).life_state() == 1));
    net.run_for(300);
    net.shoot(1, 0, 4, [0.0; 3]);
    net.run_for(200);
    assert_eq!(
        hisec_seen(net.c(0)).last(),
        Some(&(2, 2, u32::from(p1))),
        "case 2 for the new killer"
    );
    assert_eq!(case_objects(net.c(0)).first().map(|c| c.1), Some(2));
    assert!(net.c(0).seen.errors.is_empty() && net.c(1).seen.errors.is_empty());
}

/// HS-5: the reserving killer in range picks the case up: 0xd6 sub 1 to everyone, the object removed, the
/// HS-15: the drop draws its case row by weight among the rows the ROM table has; the pickup line names that row,
/// so the lobby delivers the black case for row 1. A weight on a row the table lacks never drops it.
#[test]
fn hisec_case_rows_are_drawn_by_weight_and_reported() {
    use dc_dserver::rom_tables::HisecRow;
    let mut t = hisec_tables();
    t.hisec.rows.insert(
        1,
        HisecRow {
            model: Some("PROPS\\SynthCase\\BlackCase.ltb".into()),
            skins: vec!["PROPS\\SynthCase\\BlackCase.dtx".into()],
            styles: vec![],
            scale: Some([1.0; 3]),
        },
    );
    for (rows, want) in [
        ([(1, 100), (0, 0), (0, 0), (0, 0)], 1u32),
        ([(0, 100), (1, 0), (0, 0), (0, 0)], 0),
        // row 7 is not in the table: the weight falls to the rows that are
        ([(7, 100), (1, 1), (0, 0), (0, 0)], 1),
    ] {
        let h = dc_dserver::game::hisec::HisecTuning {
            force: true,
            pickup_radius: 5_000.0,
            case_rows: rows,
            ..Default::default()
        };
        let mut net = hisec_net(h, t.clone());
        net.shoot(0, 1, 4, [0.0; 3]);
        net.run_for(300);
        assert!(
            net.pipe_lines()
                .contains(&format!("@hisec uid=7 case={want} key=1 name=Dave")),
            "{rows:?}: {:?}",
            net.pipe_lines()
        );
    }
}

/// `@hisec` line to the lobby.
#[test]
fn hisec_case_pickup_by_the_killer_reaches_the_lobby() {
    let h = dc_dserver::game::hisec::HisecTuning {
        force: true,
        pickup_radius: 5_000.0,
        ..Default::default()
    };
    let mut net = hisec_net(h, hisec_tables());
    let p0 = net.pid(0);
    net.shoot(0, 1, 4, [0.0; 3]);
    net.run_for(300);
    for i in 0..2 {
        let c = net.c(i);
        assert!(c.seen.errors.is_empty(), "client {i}: {:?}", c.seen.errors);
        assert_eq!(
            hisec_seen(c),
            vec![(0, 0, 0), (2, 1, u32::from(p0)), (1, 1, u32::from(p0))],
            "client {i}"
        );
        assert!(
            case_objects(c).is_empty(),
            "client {i}: the case object was removed"
        );
    }
    assert!(net.server.game().hisec_cases().is_empty());
    assert!(net
        .pipe_lines()
        .contains(&"@hisec uid=7 case=0 key=1 name=Dave".to_string()));
}

/// CRATE-2 / CRATE-3: in a match whose weapon table has a variant, `camo_share` 100 makes every drop a Camo Crate: its
/// key carries the camo bit, it shows the `camo_row` model and its pickup is an `@crate` line; without a variant (or
/// with share 0) the same drop is a HiSec case.
#[test]
fn camo_crate_drops_only_with_variants_and_reports_its_own_line() {
    use dc_dserver::game::hisec::{HisecTuning, CAMO_KEY_BIT};
    use dc_dserver::rom_tables::HisecRow;
    let mut t = hisec_tables();
    t.hisec.rows.insert(
        1,
        HisecRow {
            model: Some("PROPS\\SynthCase\\BlackCase.ltb".into()),
            skins: vec![],
            styles: vec![],
            scale: Some([1.0; 3]),
        },
    );
    let mut camo_t = t.clone();
    camo_t.weapons.variants.insert(502, 25);
    let h = |share| HisecTuning {
        force: true,
        pickup_radius: 5_000.0,
        camo_share: share,
        ..Default::default()
    };
    let key = 1 | CAMO_KEY_BIT;
    let mut net = hisec_net(h(100), camo_t.clone());
    net.shoot(0, 1, 4, [0.0; 3]);
    net.run_for(300);
    assert!(
        net.pipe_lines()
            .contains(&format!("@crate uid=7 kind=camo key={key} name=Dave")),
        "{:?}",
        net.pipe_lines()
    );
    assert!(!net.pipe_lines().iter().any(|l| l.starts_with("@hisec")));
    let p0 = net.pid(0);
    assert_eq!(
        hisec_seen(net.c(1)),
        vec![(0, 0, 0), (2, key, u32::from(p0)), (1, key, u32::from(p0))]
    );
    for (share, tables) in [(100, t.clone()), (0, camo_t.clone())] {
        let mut net = hisec_net(h(share), tables);
        net.shoot(0, 1, 4, [0.0; 3]);
        net.run_for(300);
        assert!(
            net.pipe_lines()
                .iter()
                .any(|l| l.starts_with("@hisec uid=7 case=0 key=1 ")),
            "share {share}: {:?}",
            net.pipe_lines()
        );
    }
}

/// HS-1: the drop chance is 1 / the room goal in TDM; no drop without the case model, below `min_players`, or when
/// switched off.
#[test]
fn hisec_drop_rules() {
    let net = hisec_net(Default::default(), hisec_tables());
    assert!((net.server.game().hisec_drop_chance() - 0.1).abs() < 1e-12);
    // no case table row: no drop even when forced
    let h = dc_dserver::game::hisec::HisecTuning {
        force: true,
        ..Default::default()
    };
    let mut net = hisec_net(h, tables());
    net.shoot(0, 1, 4, [0.0; 3]);
    net.run_for(200);
    assert!(net.c(0).seen.hisec.is_empty());
    assert!(net.logs.iter().any(|l| l.contains("no case model")));
    for h in [
        dc_dserver::game::hisec::HisecTuning {
            force: true,
            min_players: 3,
            ..Default::default()
        },
        dc_dserver::game::hisec::HisecTuning {
            force: true,
            enabled: false,
            ..Default::default()
        },
    ] {
        let mut net = hisec_net(h, hisec_tables());
        net.shoot(0, 1, 4, [0.0; 3]);
        net.run_for(200);
        assert!(net.c(0).seen.hisec.is_empty() && net.c(1).seen.hisec.is_empty());
    }
    // the kill that reaches the goal drops nothing; outside TDM the chance is 1 / `expected_kills`
    let h = dc_dserver::game::hisec::HisecTuning {
        force: true,
        ..Default::default()
    };
    let mut net = hisec_net_in(h, hisec_tables(), "room key=TDM type=1 goal=1 ff=0");
    net.shoot(0, 1, 4, [0.0; 3]);
    net.run_for(200);
    assert!(net.c(0).seen.hisec.is_empty());
    let net = hisec_net_in(
        Default::default(),
        hisec_tables(),
        "room key=X type=0 goal=10 ff=0",
    );
    assert!((net.server.game().hisec_drop_chance() - 0.05).abs() < 1e-12);
}

/// HS-3 amendment: the reservation ends at once when the reserving player dies (sub 2 with nobody and the countdown 0
/// right after the kill), as the reference stack did.
#[test]
fn hisec_reservation_ends_when_the_owner_dies() {
    let h = dc_dserver::game::hisec::HisecTuning {
        force: true,
        reserve_ms: 20_000,
        pickup_radius: 0.0,
        ..Default::default()
    };
    let mut net = hisec_net(h, hisec_tables());
    let p0 = net.pid(0);
    net.shoot(0, 1, 4, [0.0; 3]);
    assert!(net.run_until(net.now + 9_000, |n| n.c(1).life_state() == 1));
    net.run_for(300);
    assert_eq!(net.server.game().hisec_cases()[0].2, Some(p0));
    net.shoot(1, 0, 4, [0.0; 3]);
    net.run_for(100);
    assert_eq!(net.server.game().hisec_cases()[0].2, None);
    for i in 0..2 {
        let c = net.c(i);
        assert_eq!(
            hisec_seen(c).last(),
            Some(&(2, 1, 0xFFFF_FFFF)),
            "client {i}: released"
        );
        assert_eq!(c.seen.case_notes.last().map(|n| n.2), Some(0));
        // only one case: the owner's death dropped none (case 1 still lies there)
        assert_eq!(c.seen.hisec.iter().filter(|x| x.1 == 0).count(), 1);
    }
}
/// DSV-80: the spawn set's weapon grants carry the rows' default attachments (the scope mod in part 1 for a scoped
/// weapon, -1 x 4 for a weapon without defaults); `-combat weapon_parts=none` sends -1 x 4 everywhere.
// DISASM: cshell+0x60d90 (0xb2 / 0xb3 empty the sockets and attach the parts != -1), cshell+0x1f4180 (zoom needs the
// socket-1 mod); DC-DEFAULT: DSV-80 (proposed)
#[test]
fn weapon_grants_carry_the_default_attachments() {
    // made-up rows in the ROM key shape: weapon 27 names the scope of [Mod27]
    let doc = dc_bute::ButeDoc::parse(concat!(
        "[Mod26]\nName = \"Dot A\"\n",
        "[Mod27]\nName = \"Scope A\"\n",
        "[Weapon3]\nName = \"G3\"\n",
        "[Weapon27]\nName = \"Long\"\nDefaultScopeAttach = \"scope a\"\n",
    ));
    for (parts_on, want27) in [(true, [27i8, -1, -1, -1]), (false, [-1; 4])] {
        let mut t = tables();
        t.weapons = dc_dserver::rom_tables::WeaponTable::from_doc(&doc);
        let mut game = GameTuning {
            default_loadout: vec![3, 27],
            start_order: StartOrder::Spec,
            ..GameTuning::default()
        };
        if !parts_on {
            game.set_combat("weapon_parts", "none").unwrap();
        }
        let cfg = ServerTuning {
            game,
            ..ServerTuning::default()
        };
        let mut net = Net::with(cfg, t, "room key=TDM type=1 goal=20 ff=0 time=0");
        net.add(1, "Dave", 1.0);
        assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1));
        let c = net.c(0);
        assert!(c.seen.errors.is_empty(), "{:?}", c.seen.errors);
        assert_eq!(
            c.seen.weapons,
            vec![(0xb2, 27, 0), (0xb3, 3, 0)],
            "0xb2 for the scoped weapon, 0xb3 for the drawn one"
        );
        assert_eq!(c.seen.weapon_parts, vec![(27, want27), (3, [-1; 4])]);
    }
}

/// DSV-99 / DSV-100: the lobby's `parts=` token puts the player's attachments into the grants socket by socket over the
/// row's defaults: the sight on the drawn weapon (part 1, a zooming mod), a muzzle next to the default scope of the
/// backpack weapon; a mod the weapon row does not list is dropped; `-combat player_parts=off` keeps the defaults only.
// DISASM: cshell+0x60d90 (0xb2 / 0xb3 parts), cshell+0x235ec0 (socket = the mod row's Kinds), cshell+0x1f4180 (zoom
// reads the socket-1 mod)
// DC-DEFAULT: DSV-99 (proposed), DSV-100 (proposed); LDO-2 (proposed, the lobby's token)
#[test]
fn weapon_grants_carry_the_players_attachments() {
    // made-up rows in the ROM key shape (indices, names and numbers are ours)
    let doc = dc_bute::ButeDoc::parse(concat!(
        "[Mod26]
Name = \"Muzzle A\"
Kinds = 0
",
        "[Mod27]
Name = \"Scope A\"
Kinds = 1
ZoomLevel = 1
",
        "[Mod35]
Name = \"Dot B\"
Kinds = 1
ZoomLevel = 1
",
        "[Weapon3]
Name = \"G3\"
ModName0 = \"Dot B\"
",
        "[Weapon27]
Name = \"Long\"
DefaultScopeAttach = \"scope a\"
ModName0 = \"Muzzle A\"
",
    ));
    let user = "user 7 3 99 0 120 Dave loadout=0:3,10:27 parts=0:1026/1035/-1,10:1026/-1/-1 face=0";
    for (player_parts, want27, want3) in [
        (true, [27i8, 26, -1, -1], [35i8, -1, -1, -1]),
        (false, [27, -1, -1, -1], [-1; 4]),
    ] {
        let mut t = tables();
        t.weapons = dc_dserver::rom_tables::WeaponTable::from_doc(&doc);
        let mut game = GameTuning {
            start_order: StartOrder::Spec,
            ..GameTuning::default()
        };
        if !player_parts {
            game.set_combat("player_parts", "off").unwrap();
        }
        let cfg = ServerTuning {
            game,
            ..ServerTuning::default()
        };
        let mut net = Net::with(cfg, t, "room key=TDM type=1 goal=20 ff=0 time=0");
        assert!(net.server.handle_line(0, user));
        net.add(1, "Dave", 1.0);
        assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1));
        let c = net.c(0);
        assert!(c.seen.errors.is_empty(), "{:?}", c.seen.errors);
        assert_eq!(
            c.seen.weapons,
            vec![(0xb2, 27, 0), (0xb3, 3, 0)],
            "0xb2 for the backpack weapon, 0xb3 for the drawn one"
        );
        assert_eq!(c.seen.weapon_parts, vec![(27, want27), (3, want3)]);
        let noted = net
            .logs
            .iter()
            .any(|l| l.contains("weapon 3 attachments") && l.contains("(zoom true)"));
        assert_eq!(noted, player_parts, "{:?}", net.logs);
    }
}

/// DSV-167: the shot record's mod bytes `e`, `f`, `g` carry the shooter weapon's `[Mod<n>]` numbers by socket
/// (muzzle, sight, magazine): the slot-0 sniper with the S3 suppressor (item 1033), its default scope and the loader
/// (item 1053) sends 33, 27, 53; a weapon without mods sends `ff ff ff`; `d` stays 0.
// DISASM: cshell+0x2c6a7e..0x2c6a96 (the three bytes kept as mod numbers), cshell+0x235ec0 (socket = Kinds);
// EVIDENCE: cshell+0x2c6580 (the first is the muzzle mod: the silenced fire sound)
// DC-DEFAULT: DSV-167 (proposed)
#[test]
fn shot_records_carry_the_shooters_socket_mods() {
    // made-up rows in the ROM key shape (indices and names are ours)
    let doc = dc_bute::ButeDoc::parse(concat!(
        "[Mod27]
Name = \"Scope A\"
Kinds = 1
ZoomLevel = 1
",
        "[Mod33]
Name = \"Quiet A\"
Kinds = 0
",
        "[Mod53]
Name = \"Loader A\"
Kinds = 2
",
        "[Weapon3]
Name = \"G3\"
",
        "[Weapon27]
Name = \"Long\"
DefaultScopeAttach = \"scope a\"
ModName0 = \"Quiet A\"
",
        "ModName1 = \"Loader A\"
",
    ));
    let mut t = tables();
    t.weapons = dc_dserver::rom_tables::WeaponTable::from_doc(&doc);
    let cfg = ServerTuning {
        game: GameTuning {
            start_order: StartOrder::Spec,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(cfg, t, "room key=TDM type=1 goal=20 ff=0 time=0");
    assert!(net.server.handle_line(
        0,
        "user 7 3 99 0 120 Dave loadout=0:27 parts=0:1033/-1/1053 face=0"
    ));
    assert!(net
        .server
        .handle_line(0, "user 8 4 55 1 0 Big_Erin loadout=0:3 face=0"));
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(300);
    let (dave, erin) = (net.obj(0), net.obj(1));
    assert!(net.cm(0).fire_at(erin, [0.0, 500.0, 0.0], false));
    net.run_for(200);
    assert!(net.cm(1).fire_at(dave, [0.0, 500.0, 0.0], false));
    net.run_for(200);
    for i in 0..2 {
        let tails: Vec<(u16, u16, [u8; 4])> = net
            .c(i)
            .seen
            .effects
            .iter()
            .filter(|e| e.1 == dc_dserver::gs::FX_SHOT)
            .map(|e| {
                let mut r = e.2.reader();
                r.read_u16();
                let (shooter, weapon) = (r.read_u16(), r.read_u16());
                r.skip_bits(16 + 8 + 16 + 8 + 3 * 96);
                (
                    shooter,
                    weapon,
                    [r.read_u8(), r.read_u8(), r.read_u8(), r.read_u8()],
                )
            })
            .collect();
        assert!(!tails.is_empty(), "client {i}: shot records");
        for (shooter, weapon, tail) in &tails {
            let want = if *shooter == dave {
                assert_eq!(*weapon, 27, "client {i}");
                [0, 33, 27, 53]
            } else {
                assert_eq!((*shooter, *weapon), (erin, 3), "client {i}");
                [0, 0xff, 0xff, 0xff]
            };
            assert_eq!(*tail, want, "client {i}: shooter {shooter} d, e, f, g");
        }
        assert!(tails.iter().any(|t| t.0 == dave) && tails.iter().any(|t| t.0 == erin));
    }
}

/// DSV-440 / CLI-1440 over the loopback: Dave's rifle carries a suppressor (`Instdamage` -0.1) and a Fast Loader
/// (`ShotsPerClipPlus0` -2): the server grants the clip of 10 - 2 and its body hit takes 50 x 0.9 from Erin;
/// with `combat.mod_damage=off` the same hit takes the row's 50.
// EVIDENCE: cshell+0x233430 (signed clip add), cshell+0x176f10 (`Instdamage` -> mod +0x29c)
// DC-DEFAULT: DSV-440 (proposed), CLI-1440 (proposed), DSV-182 (proposed)
#[test]
fn a_modded_rifle_hits_for_less_and_holds_the_smaller_clip() {
    // made-up rows in the ROM key shape (indices and names are ours)
    let doc = dc_bute::ButeDoc::parse(concat!(
        "[Mod33]\nName = \"Quiet A\"\nKinds = 0\nInstdamage = -0.1\n",
        "[Mod53]\nName = \"Loader A\"\nKinds = 2\nShotsPerClipPlus0 = -2//10 rounds (20%)\n",
        "[Ammo0]\nName = \"R\"\nInstDamage = 50\nMaxAmount = 30\n",
        "[Weapon3]\nName = \"G3\"\nAmmoName0 = \"R\"\n",
        "[Weapon27]\nName = \"Long\"\n",
        "ShotsPerClip = 10\nAmmoName0 = \"R\"\n",
        "ModName0 = \"Quiet A\"\nModModel0 = 0\nModName1 = \"Loader A\"\nModModel1 = 0\n",
        "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"Long\"\nP1 = \"G3\"\n",
    ));
    for (mod_damage, want_health) in [(true, 55.0), (false, 50.0)] {
        let mut t = tables();
        t.weapons = dc_dserver::rom_tables::WeaponTable::from_doc(&doc);
        let mut game = GameTuning {
            start_order: StartOrder::Spec,
            ..GameTuning::default()
        };
        if !mod_damage {
            game.set_combat("mod_damage", "off").unwrap();
        }
        let cfg = ServerTuning {
            game,
            ..ServerTuning::default()
        };
        let mut net = Net::with(cfg, t, "room key=TDM type=1 goal=20 ff=0 time=0");
        assert!(net.server.handle_line(
            0,
            "user 7 3 99 0 120 Dave loadout=0:27 parts=0:1033/-1/1053 face=0"
        ));
        assert!(net
            .server
            .handle_line(0, "user 8 4 55 1 0 Big_Erin loadout=0:3 face=0"));
        net.add(1, "Dave", 0.0);
        net.add(2, "Big Erin", 0.0);
        assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
            && n.c(1).life_state() == 1));
        net.run_for(300);
        let w = net.server.game().players()[&net.pid(0)].weapons[0];
        assert_eq!(
            (w.weapon, w.clip_size, w.clip),
            (27, 8, 8),
            "the Fast Loader's clip"
        );
        assert_eq!(net.c(0).seen.weapon_parts.first().map(|p| p.0), Some(27));
        net.shoot(0, 1, 1, [0.0; 3]);
        assert_eq!(
            net.c(1).seen.health,
            Some(want_health),
            "mod_damage {mod_damage}"
        );
    }
}

/// MOD-21 / DSV-478: a `-content` document reaches the damage dealt. The rows of the modded-rifle test without
/// parts: the ROM-shaped ammunition row hits for 50; a content file patching `[Ammo0] InstDamage` to 30 makes the
/// same body hit take 30, and the patched clip of the weapon row is what the server grants.
// DC-DEFAULT: DSV-478 (proposed), MOD-21 (proposed)
#[test]
fn a_content_file_changes_the_damage_dealt() {
    // made-up rows in the ROM key shape (indices and names are ours)
    let doc = dc_bute::ButeDoc::parse(concat!(
        "[Ammo0]
Name = \"R\"
InstDamage = 50
MaxAmount = 30
",
        "[Weapon3]
Name = \"G3\"
AmmoName0 = \"R\"
",
        "[Weapon27]
Name = \"Long\"
ShotsPerClip = 10
AmmoName0 = \"R\"
",
        "[WeaponOrder]
Class0 = \"P\"
P0 = \"Long\"
P1 = \"G3\"
",
    ));
    let dir = std::env::temp_dir().join(format!("dc_dserver_content_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("content.json");
    std::fs::write(
        &file,
        concat!(
            r#"{"version":1,"weapons":[{"weapon":27,"op":"patch","fields":{"ShotsPerClip":"6"}}],"#,
            r#""ammo":[{"ammo":0,"op":"patch","fields":{"InstDamage":"30"}}]}"#
        ),
    )
    .unwrap();
    let args = dc_dserver::cli::parse(
        [
            "-worldfile",
            "synth.dat",
            "-content",
            &file.display().to_string(),
        ]
        .map(String::from),
    )
    .unwrap();
    let content = dc_dserver::runner::load_content(&args)
        .unwrap()
        .expect("-content");
    assert_eq!(
        dc_dserver::runner::content_line(&content),
        format!(
            "content: 1 weapon patches, 1 ammo patches, hash {}",
            content.hash()
        )
    );
    for (patched, want_health, want_clip) in [(false, 50.0, 10), (true, 70.0, 6)] {
        let mut t = tables();
        t.weapons = if patched {
            dc_dserver::rom_tables::WeaponTable::from_doc_with_content(&doc, &content).unwrap()
        } else {
            dc_dserver::rom_tables::WeaponTable::from_doc(&doc)
        };
        let cfg = ServerTuning {
            game: GameTuning {
                start_order: StartOrder::Spec,
                ..GameTuning::default()
            },
            ..ServerTuning::default()
        };
        let mut net = Net::with(cfg, t, "room key=TDM type=1 goal=20 ff=0 time=0");
        assert!(net
            .server
            .handle_line(0, "user 7 3 99 0 120 Dave loadout=0:27 face=0"));
        assert!(net
            .server
            .handle_line(0, "user 8 4 55 1 0 Big_Erin loadout=0:3 face=0"));
        net.add(1, "Dave", 0.0);
        net.add(2, "Big Erin", 0.0);
        assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
            && n.c(1).life_state() == 1));
        net.run_for(300);
        let w = net.server.game().players()[&net.pid(0)].weapons[0];
        assert_eq!(
            (w.weapon, w.clip_size),
            (27, want_clip),
            "patched {patched}"
        );
        net.shoot(0, 1, 1, [0.0; 3]);
        assert_eq!(net.c(1).seen.health, Some(want_health), "patched {patched}");
    }
    // a malformed file and a patch of a row the table lacks are refused
    std::fs::write(&file, "{\"version\":1,\"weapons\":[{\"weapon\":27}]}").unwrap();
    assert!(dc_dserver::runner::load_content(&args).is_err());
    let missing = dc_bute::modpatch::WeaponContent::from_json(
        r#"{"version":1,"weapons":[{"weapon":28,"op":"patch","fields":{"ShotsPerClip":"6"}}]}"#,
    )
    .unwrap();
    assert!(dc_dserver::rom_tables::WeaponTable::from_doc_with_content(&doc, &missing).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

/// The user's 2026-10-27 sniper loadout (lobby `--start-loadout`, LG-5) as the lobby writes it to the server: with the
/// ROM weapon rows every chosen suppressor / sight / magazine lands in its socket, the sniper keeps its default scope
/// and zooms, the pistol zooms through the chosen dot sight.
// DC-DEFAULT: DSV-99 (proposed), DSV-100 (proposed), LG-5 (proposed)
#[test]
fn rom_user_sniper_loadout_grants_its_attachments() {
    let Some(rom) = dc_rez::dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let (rt, notes) = RomTables::load(&rom);
    assert!(!rt.weapons.is_empty(), "{notes:?}");
    let mut t = tables();
    t.weapons = rt.weapons;
    let user = "user 7 3 99 0 120 Dave loadout=0:27,1:164,2:58,3:7,10:25                 parts=0:1033/-1/1053,1:1034/1040/1055,10:1031/-1/1051 face=0";
    let cfg = ServerTuning {
        game: GameTuning {
            start_order: StartOrder::Spec,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(cfg, t, "room key=TDM type=1 goal=20 ff=0 time=0");
    assert!(net.server.handle_line(0, user));
    net.add(1, "Dave", 1.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1));
    let c = net.c(0);
    assert!(c.seen.errors.is_empty(), "{:?}", c.seen.errors);
    let parts = |w: u16| {
        c.seen
            .weapon_parts
            .iter()
            .find(|(x, _)| *x == w)
            .map(|(_, p)| *p)
    };
    // the part bytes are a list the client files by each mod row's Kinds (DISASM cshell+0x235ec0), not by position
    let has = |w: u16, want: &[i8]| {
        let p = parts(w).unwrap_or_else(|| panic!("weapon {w} granted: {:?}", c.seen.weapon_parts));
        assert!(
            want.iter().all(|m| p.contains(m)),
            "weapon {w}: {p:?} lacks one of {want:?}"
        );
    };
    has(27, &[27, 33, 53]); // the default Sniper Scope + S3 suppressor + fast loader
    has(164, &[34, 40, 55]);
    has(25, &[31, 51]);
    for w in [27u16, 164] {
        assert!(
            net.logs.iter().any(
                |l| l.contains(&format!("weapon {w} attachments")) && l.contains("(zoom true)")
            ),
            "{w}: {:?}",
            net.logs
        );
    }
}

// ------------------------------------------------------------------------------------------------- roster look (DSV-90)

/// DSV-90: the 0x6b look of every player on every link has no overridable-model row (byte 14 = -1) and an all-zero
/// look arrives as the default look; the other bytes pass unchanged.
// OBSERVED: capture 20261025_065016_tdm6 seq 219 (the reference stack's default look for the bot);
// 20261025_125432_rust6 conn 5 seq 810 (our former forwarding of the bot's zeros)
// DC-DEFAULT: DSV-90 (proposed)
#[test]
fn roster_look_has_no_override_row_and_a_default_for_zeros() {
    let mut net = Net::with(
        ServerTuning::default(),
        tables(),
        "room key=TDM type=1 goal=20 ff=0",
    );
    net.add(1, "Dave", 0.0);
    net.look = Some([0; 15]);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    let (dave, erin) = (net.pid(0), net.pid(1));
    let mut dave_look = [0i8; 15];
    for (i, b) in dave_look.iter_mut().enumerate() {
        *b = i as i8;
    }
    dave_look[dc_dserver::gs::LOOK_OVERRIDE_BYTE] = -1;
    for i in 0..2 {
        let r = &net.c(i).seen.roster;
        assert_eq!(r[&dave].look, dave_look, "client {i}: Dave's look");
        assert_eq!(
            r[&erin].look,
            dc_dserver::gs::DEFAULT_LOOK_BYTES,
            "client {i}: the default look for zeros"
        );
    }
}
/// DSV-63: every spawn of a character brings the start note (0xa6 type 0x18 sub 0x1f) to every introduced client
/// and the end note (sub 0x25) `spawn_note_ms` later; a respawn brings a new pair to both clients. Both first spawns
/// happen in one tick: the client introduced second gets the first spawner's start note at its introduction, so
/// every end note has its start note on the same link (as in the reference stack's tdm6; the headless round of
/// captures/20261025_132219_ad-polish had c5 with the end note alone).
#[test]
fn spawn_notes_bracket_every_spawn() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![3, 4],
            default_damage: 200.0,
            respawn_wait_ms: Some(6_000),
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(
        cfg,
        character_tables(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
    );
    net.server.insert_rig("BODY\\b.ltb", rig());
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(4_000);
    let (dave, erin) = (net.obj(0), net.obj(1));
    let pair = |n: &Net, i: usize, o: u16| -> Vec<(u32, u8)> {
        n.c(i)
            .seen
            .spawn_notes
            .iter()
            .filter(|x| x.1 == o)
            .map(|x| (x.0, x.2))
            .collect()
    };
    for i in 0..2 {
        assert!(
            net.c(i).seen.errors.is_empty(),
            "{:?}",
            net.c(i).seen.errors
        );
        for o in [dave, erin] {
            let p = pair(&net, i, o);
            assert_eq!(
                p.iter().map(|x| x.1).collect::<Vec<_>>(),
                vec![0x1f, 0x25],
                "client {i}, object {o}: {p:?}"
            );
            let d = p[1].0 - p[0].0;
            assert!(
                (SPAWN_NOTE_MS..=SPAWN_NOTE_MS + 60).contains(&d),
                "client {i}, object {o}: end note after {d} ms"
            );
        }
    }
    // Dave kills Erin; Erin's respawn brings a new pair to both clients
    net.shoot(0, 1, 1, [0.0; 3]);
    let before = [pair(&net, 0, erin).len(), pair(&net, 1, erin).len()];
    assert!(net.run_until(net.now + 12_000, |n| n.c(1).life_state() == 1
        && n.c(1).seen.life_states.len() >= 4));
    net.run_for(4_000);
    for i in 0..2 {
        let p = pair(&net, i, erin);
        let new: Vec<u8> = p[before[i]..].iter().map(|x| x.1).collect();
        assert_eq!(new, vec![0x1f, 0x25], "client {i}: {p:?}");
        assert!(
            net.c(i).seen.errors.is_empty(),
            "{:?}",
            net.c(i).seen.errors
        );
    }
}

/// HS-2 / HS-3 amendments: a client that enters the world while a reserved case lies there gets the case object in
/// its world-load group, the countdown note to it, and at its introduction the reservation (0xd6 sub 2 with the
/// owner), with no client error; it then sees the release like everyone else.
#[test]
fn hisec_case_reaches_a_client_that_joins_later() {
    let h = dc_dserver::game::hisec::HisecTuning {
        force: true,
        reserve_ms: 5_000,
        lifetime_ms: 0,
        pickup_radius: 0.0,
        min_players: 2,
        ..Default::default()
    };
    let mut net = hisec_net(h, hisec_tables());
    let p0 = net.pid(0);
    net.shoot(0, 1, 4, [0.0; 3]);
    net.run_for(300);
    assert_eq!(case_objects(net.c(0)).len(), 1);
    assert!(net.server.handle_line(0, "user 9 5 10 0 0 Carl"));
    net.add(3, "Carl", 0.0);
    assert!(net.run_until(net.now + 20_000, |n| n.c(2).life_state() == 1));
    net.run_for(300);
    let c = net.c(2);
    assert!(c.seen.errors.is_empty(), "{:?}", c.seen.errors);
    let cases = case_objects(c);
    assert_eq!(cases.len(), 1, "the joiner has the case object");
    assert_eq!(cases[0].1, 1);
    assert_eq!(
        cases[0].0,
        case_objects(net.c(0))[0].0,
        "the same object id"
    );
    assert!(
        hisec_seen(c).contains(&(2, 1, u32::from(p0))),
        "the reservation: {:?}",
        hisec_seen(c)
    );
    assert!(
        c.seen
            .case_notes
            .first()
            .is_some_and(|n| n.1 == cases[0].0 && n.2 >= 1),
        "{:?}",
        c.seen.case_notes
    );
    // the release reaches the joiner too
    net.run_for(5_500);
    let c = net.c(2);
    assert_eq!(hisec_seen(c).last(), Some(&(2, 1, 0xFFFF_FFFF)));
    assert_eq!(c.seen.case_notes.last().map(|n| n.2), Some(0));
    assert!(c.seen.errors.is_empty(), "{:?}", c.seen.errors);
}

/// The event sound rows of a synthetic table (made-up file names).
fn sound_tables() -> RomTables {
    let mut t = throw_tables();
    t.event_sounds = dc_dserver::sounds::EventSoundTables {
        pain: vec!["SNDX\\PAIN1.WAV".into(), "SNDX\\PAIN2.WAV".into()],
        headshot: Some("SNDX\\HEAD.WAV".into()),
        grenade_impact: (0..=255u32)
            .map(|i| (i, "SNDX\\BOUNCE.WAV".to_string()))
            .collect(),
        surface_ids: Vec::new(),
        ..Default::default()
    };
    t
}

/// DSV-64: a damaging hit starts a pain sound (the reference stack's record form) at the victim on both clients; a second
/// hit while it plays starts none; the track is removed after its life and the next hit starts a new one.
#[test]
fn pain_sounds_start_once_per_play_and_are_removed() {
    let mut net = Net::with(
        ServerTuning::default(),
        sound_tables(),
        "room key=TDM type=1 goal=10 ff=0",
    );
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    let starts = |n: &Net, i: usize| n.c(i).seen.sound_starts.clone();
    let before = [starts(&net, 0).len(), starts(&net, 1).len()];
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(100);
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(100);
    let mut track = 0;
    for (i, &b) in before.iter().enumerate() {
        let c = net.c(i);
        assert!(c.seen.errors.is_empty(), "client {i}: {:?}", c.seen.errors);
        let new = &starts(&net, i)[b..];
        assert_eq!(
            new.len(),
            1,
            "client {i}: one pain sound for two hits: {new:?}"
        );
        let (t, f) = new[0];
        track = t;
        let name = c
            .seen
            .files
            .get(&f)
            .map(|x| x.1.clone())
            .unwrap_or_default();
        assert!(name.starts_with(b"SNDX\\PAIN"), "client {i}: file {name:?}");
        let s = c.seen.sounds[&t];
        assert_eq!(
            (s.sflags, s.prio, s.outer, s.ratio, s.tmask),
            (0x132, 2, 500, 17, 3)
        );
        assert!(!c.seen.objects.contains_key(&t), "a track is not an object");
    }
    // life = the unknown play time (no ROM) + the tail; then removed on both clients
    net.run_for(dc_dserver::sounds::UNKNOWN_PLAY_MS + dc_dserver::sounds::TRACK_TAIL_MS + 200);
    for i in 0..2 {
        assert!(
            net.c(i).seen.sounds_removed.contains(&track),
            "client {i}: track {track} removed"
        );
    }
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(100);
    for (i, &b) in before.iter().enumerate() {
        assert_eq!(starts(&net, i).len(), b + 2, "client {i}: a new pain sound");
    }
}

/// The synthetic world with a floor quad (made-up surface word 42) 40 units below the spawn height over the whole
/// box.
fn floor_world() -> WorldInfo {
    use dc_world::synth::{SynthModel, SynthWorld};
    let y = -40.0;
    let bytes = SynthWorld {
        info: b"floor".to_vec(),
        extent_min: [-512.0, -64.0, -512.0],
        extent_max: [512.0, 256.0, 512.0],
        tree_shape: vec![false],
        models: vec![SynthModel {
            name: b"main".to_vec(),
            textures: vec![b"t".to_vec()],
            planes: vec![([0.0, 1.0, 0.0], y)],
            surfaces: vec![(1, 0, 42)],
            polys: vec![(0, 0, vec![0, 1, 2, 3])],
            nodes: vec![(0, 0, -1, -2)],
            points: vec![
                [-500.0, y, -500.0],
                [-500.0, y, 500.0],
                [500.0, y, 500.0],
                [500.0, y, -500.0],
            ],
            root: 0,
            box_min: [-500.0, y, -500.0],
            box_max: [500.0, y, 500.0],
            ..SynthModel::default()
        }],
        ..SynthWorld::default()
    }
    .build();
    let mut w = world_from_bytes(&bytes, ANNOUNCE, "Floor").unwrap();
    w.spawns = world().spawns;
    w
}

/// DSV-64 / DSV-62: a throw at the feet falls onto the floor polygon, bounces and plays the grenade impact sound of
/// that polygon's surface (prio 1, outer 1000) on every client.
#[test]
fn a_bouncing_throw_plays_the_surface_impact_sound() {
    let mut t = sound_tables();
    t.event_sounds.grenade_impact = [(42u32, "SNDX\\BOUNCE42.WAV".to_string())]
        .into_iter()
        .collect();
    let mut cfg = ServerTuning::default();
    cfg.game.default_loadout = vec![40];
    let mut server = Server::with_tables(cfg, floor_world(), t, 0);
    assert!(server.handle_line(0, "room key=TDM type=1 goal=20 ff=0"));
    assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
    assert!(server.handle_line(0, "user 8 4 55 1 0 Big_Erin"));
    let mut net = Net {
        server,
        peers: Vec::new(),
        pipe: Vec::new(),
        logs: Vec::new(),
        now: 0,
        delay_ms: 2,
        look: None,
    };
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| (0..2).all(|i| n.c(i).life_state() == 1)));
    net.run_for(500);
    net.cm(0).throw((40, 41), [0.0, -1.0, 0.0]);
    net.run_for(2_500);
    for i in 0..2 {
        let c = net.c(i);
        assert!(c.seen.errors.is_empty(), "client {i}: {:?}", c.seen.errors);
        let bounces: Vec<u16> = c
            .seen
            .sound_starts
            .iter()
            .filter(|(_, f)| {
                c.seen
                    .files
                    .get(f)
                    .is_some_and(|x| x.1.as_slice() == b"SNDX\\BOUNCE42.WAV")
            })
            .map(|x| x.0)
            .collect();
        assert!(
            !bounces.is_empty(),
            "client {i}: no bounce sound: {:?}",
            c.seen.sound_starts
        );
        let s = c.seen.sounds[&bounces[0]];
        assert_eq!((s.sflags, s.prio, s.outer, s.ratio), (0x132, 1, 1000, 17));
    }
}

/// Every end note a client gets (sub 0x25) had its start note (sub 0x1f) for the same object on the same link.
fn spawn_notes_are_paired(c: &SynthClient, who: &str) {
    let mut last: std::collections::BTreeMap<u16, u8> = std::collections::BTreeMap::new();
    for &(at, obj, sub) in &c.seen.spawn_notes {
        if sub == 0x25 {
            assert_eq!(
                last.get(&obj),
                Some(&0x1f),
                "{who}: end note for object {obj} at {at} without its start note"
            );
        }
        last.insert(obj, sub);
    }
}

/// DSV-63 amendment: a client introduced while spawn-note windows are open gets their start notes at its
/// introduction and the end notes with everyone else (the headless round of captures/20261025_132219_ad-polish had the
/// joiner c9 with an end note alone); no client ever gets an end note without its start note.
#[test]
fn spawn_notes_catch_up_for_a_client_introduced_in_the_window() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![3, 4],
            spawn_note_ms: 15_000,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(
        cfg,
        character_tables(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
    );
    net.server.insert_rig("BODY\\b.ltb", rig());
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(2_000);
    assert!(net.server.handle_line(net.now, "user 9 5 10 0 0 Carl"));
    net.add(3, "Carl", 0.0);
    assert!(net.run_until(net.now + 20_000, |n| n.c(2).life_state() == 1));
    let joined = net.now;
    net.run_for(16_000);
    let notes = |n: &Net, i: usize, o: u16| -> Vec<(u32, u8)> {
        n.c(i)
            .seen
            .spawn_notes
            .iter()
            .filter(|x| x.1 == o)
            .map(|x| (x.0, x.2))
            .collect()
    };
    let (dave, erin, carl) = (net.obj(0), net.obj(1), net.obj(2));
    for i in 0..3 {
        let c = net.c(i);
        assert!(c.seen.errors.is_empty(), "client {i}: {:?}", c.seen.errors);
        spawn_notes_are_paired(c, &format!("client {i}"));
        for o in [dave, erin, carl] {
            let p = notes(&net, i, o);
            assert_eq!(
                p.iter().map(|x| x.1).collect::<Vec<_>>(),
                vec![0x1f, 0x25],
                "client {i}, object {o}: {p:?}"
            );
        }
    }
    // Carl's start notes for the open windows came at his introduction (before his own spawn), his end notes in the
    // tick of everyone else's
    for o in [dave, erin] {
        let mine = notes(&net, 2, o);
        let theirs = notes(&net, 0, o);
        assert!(
            mine[0].0 < joined && mine[0].0 > theirs[0].0,
            "object {o}: Carl's start note at {} (join {joined}, Dave's {})",
            mine[0].0,
            theirs[0].0
        );
        assert_eq!(mine[1].0, theirs[1].0, "object {o}: the end note tick");
    }
}

/// DSV-64 amendment: the removal record of an event sound goes to the clients that were sent its start; a client
/// that entered the world after the start never had the track and gets no removal for it.
#[test]
fn event_sound_removal_skips_a_client_that_entered_after_the_start() {
    let mut net = Net::with(
        ServerTuning::default(),
        sound_tables(),
        "room key=TDM type=1 goal=10 ff=0",
    );
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    let before = net.c(0).seen.sound_starts.len();
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(100);
    let new = &net.c(0).seen.sound_starts[before..];
    assert_eq!(new.len(), 1, "one pain sound: {new:?}");
    let track = new[0].0;
    assert!(net.server.handle_line(net.now, "user 9 5 10 0 0 Carl"));
    net.add(3, "Carl", 0.0);
    let shot_at = net.now;
    assert!(net.run_until(net.now + 20_000, |n| n.c(2).life_state() == 1));
    let life = dc_dserver::sounds::UNKNOWN_PLAY_MS + dc_dserver::sounds::TRACK_TAIL_MS;
    assert!(
        net.now - shot_at < life,
        "Carl entered {} ms after the start, the track lives {life} ms",
        net.now - shot_at
    );
    net.run_for(life + 200);
    for i in 0..2 {
        assert!(
            net.c(i).seen.sounds_removed.contains(&track),
            "client {i}: track {track} removed"
        );
    }
    let carl = net.c(2);
    assert!(carl.seen.errors.is_empty(), "{:?}", carl.seen.errors);
    assert!(
        !carl.seen.sounds.contains_key(&track),
        "Carl never had the track"
    );
    assert!(
        !carl.seen.removed_unknown.contains(&track) && !carl.seen.sounds_removed.contains(&track),
        "Carl got a removal for track {track}: {:?}",
        carl.seen.removed_unknown
    );
}

/// DSV-35 amendment (project decision): when the clock runs out, the summary set (0x47, the roster refresh,
/// 0x65 per player, 0x48: the scoreboard) goes out in the tick of the 0x46 result; the result screen (0x3f) keeps its
/// pause; a goal end keeps the reference stack's first pause; `-timeout-hold` sets the time-limit pause.
#[test]
fn time_limit_end_shows_the_scoreboard_at_once() {
    let end_set: [u8; 8] = [0x46, 0x47, 0x6b, 0x6b, 0x65, 0x65, 0x48, 0x3f];
    let check =
        |net: &Net, first: std::ops::RangeInclusive<u32>, second: std::ops::RangeInclusive<u32>| {
            for i in 0..2 {
                let s = &net.c(i).seen;
                assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
                // the spawn notes (0xa6, DSV-63) run on their own 3.5 s timer and may land inside the end set
                // the DSV-77 freeze (0xa0) has its own test
                let o: Vec<u8> = shell_order(net.c(i))
                    .into_iter()
                    .filter(|&x| x != 0xa6 && x != 0xa0)
                    .collect();
                let p = o.iter().position(|&x| x == 0x46).expect("0x46");
                assert_eq!(&o[p..], &end_set, "client {i}: {o:x?}");
                let at = |id: u8| s.shell_ids.iter().find(|x| x.1 == id).unwrap().0;
                let (d1, d2) = (at(0x48) - at(0x46), at(0x3f) - at(0x46));
                assert!(first.contains(&d1), "client {i}: 0x48 {d1} ms after 0x46");
                assert!(second.contains(&d2), "client {i}: 0x3f {d2} ms after 0x46");
                // the wire values stay the reference stack's
                assert_eq!(s.round_result_secs, Some(3), "client {i}");
                assert_eq!(s.round_close.map(|r| r.1), Some(3), "client {i}");
            }
        };
    // the clock runs out: the default hold is the reference stack's 3 s (the client draws the scoreboard panel itself 3 s
    // after the 0x46), the result screen 3 s after the scoreboard set
    let mut net = Net::two_alive(
        ServerTuning::default(),
        "room key=TDM type=1 goal=50 ff=0 time=60",
        [0.0, 0.0],
    );
    let started = net.c(0).seen.world_ready_sent.unwrap();
    assert!(net.run_until(started + 61_000, |n| n.c(1).seen.round_result.is_some()));
    net.run_for(6_500);
    assert_eq!(net.c(0).seen.round_result, Some((0, 0, 0)));
    check(&net, 3_000..=3_060, 6_000..=6_060);
    // -timeout-hold 0: the scoreboard set in the tick of the result
    let cfg = ServerTuning {
        game: GameTuning {
            timeout_hold_ms: 0,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::two_alive(cfg, "room key=TDM type=1 goal=50 ff=0 time=60", [0.0, 0.0]);
    let started = net.c(0).seen.world_ready_sent.unwrap();
    assert!(net.run_until(started + 61_000, |n| n.c(1).seen.round_result.is_some()));
    net.run_for(3_500);
    check(&net, 0..=0, 3_000..=3_060);
    // a goal end keeps the reference stack's 3 s before the scoreboard set
    let cfg = ServerTuning {
        game: GameTuning {
            default_damage: 100.0,
            respawn_wait_ms: Some(2_100),
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::two_alive(cfg, "room key=TDM type=1 goal=2 ff=0", [0.0, 0.0]);
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_until(net.now + 3_000, |n| n.c(1).life_state() == 1);
    net.run_for(300);
    net.shoot(0, 1, 1, [0.0; 3]);
    assert!(net.run_until(net.now + 2_000, |n| n.c(1).seen.round_result.is_some()));
    net.run_for(6_500);
    check(&net, 3_000..=3_060, 6_000..=6_060);
    // -timeout-hold 1000: the time-limit pause is the operator's
    let cfg = ServerTuning {
        game: GameTuning {
            timeout_hold_ms: 1_000,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::two_alive(cfg, "room key=TDM type=1 goal=50 ff=0 time=60", [0.0, 0.0]);
    let started = net.c(0).seen.world_ready_sent.unwrap();
    assert!(net.run_until(started + 61_000, |n| n.c(1).seen.round_result.is_some()));
    net.run_for(4_500);
    check(&net, 1_000..=1_060, 4_000..=4_060);
}
/// The time of the first `GS_MOVER_UPDATE` (0xa0) client `i` got at or after its `GS_ROUND_RESULT`, and the ms from
/// the result to it.
fn freeze_after_result(net: &Net, i: usize) -> Option<(u32, u32)> {
    let ids = &net.c(i).seen.shell_ids;
    let p = ids.iter().position(|x| x.1 == 0x46)?;
    let t46 = ids[p].0;
    ids[p..]
        .iter()
        .find(|x| x.1 == 0xa0)
        .map(|x| (x.0, x.0 - t46))
}
/// DSV-77 (reinstated, project decision, rust7: "it still lets me move around when it says bravo team
/// wins"): with `GS_ROUND_RESULT` every living, grounded player gets `GS_MOVER_UPDATE` `[0x0080][freeze 1]` in the
/// same tick, before the scoreboard set; a dead player gets none; an airborne one is frozen at his first grounded
/// trailer, at the latest `freeze_wait_ms` after the result; `-roundfreeze off` sends none.
// DC-DEFAULT: DSV-77 (reinstated, proposed)
#[test]
fn round_result_freezes_the_living_players() {
    let room = "room key=TDM type=1 goal=50 ff=0 time=60";
    // the clock runs out with both players on the ground
    let mut net = Net::two_alive(ServerTuning::default(), room, [0.0, 0.0]);
    let started = net.c(0).seen.world_ready_sent.unwrap();
    assert!(net.run_until(started + 61_000, |n| n.c(1).seen.round_result.is_some()));
    net.run_for(3_500);
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
        assert_eq!(s.freezes, vec![true], "client {i}");
        assert_eq!(
            freeze_after_result(&net, i).map(|x| x.1),
            Some(0),
            "client {i}"
        );
        // the freeze goes with the result; the scoreboard set follows after the 3 s hold (DSV-35 amendment 2)
        let o: Vec<u8> = shell_order(net.c(i))
            .into_iter()
            .filter(|&x| x != 0xa6)
            .collect();
        let p = o.iter().position(|&x| x == 0x46).unwrap();
        assert_eq!(&o[p..p + 3], &[0x46, 0xa0, 0x47], "client {i}: {o:x?}");
    }
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("frozen at the round result (DSV-77)")));

    // a goal end: the killer is frozen, the dead victim is not
    let cfg = ServerTuning {
        game: GameTuning {
            default_damage: 100.0,
            respawn_wait_ms: Some(2_100),
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::two_alive(cfg, "room key=TDM type=1 goal=1 ff=0", [0.0, 0.0]);
    net.shoot(0, 1, 1, [0.0; 3]);
    assert!(net.run_until(net.now + 2_000, |n| n.c(1).seen.round_result.is_some()));
    net.run_for(1_500);
    assert_eq!(net.c(0).seen.freezes, vec![true]);
    assert_eq!(freeze_after_result(&net, 0).map(|x| x.1), Some(0));
    assert!(
        net.c(1).seen.freezes.is_empty(),
        "the dead player stays unfrozen"
    );
    assert_eq!(freeze_after_result(&net, 1), None);

    // airborne at the result: client 0 lands 300 ms later and is frozen at that trailer; client 1 stays in the air
    // and is frozen when the 1000 ms wait runs out; nobody is frozen twice
    let mut net = Net::two_alive(ServerTuning::default(), room, [0.0, 0.0]);
    let started = net.c(0).seen.world_ready_sent.unwrap();
    net.run_until(started + 11_000, |_| false);
    net.cm(0).set_on_ground(false);
    net.cm(1).set_on_ground(false);
    assert!(net.run_until(started + 61_000, |n| n.c(0).seen.round_result.is_some()));
    assert!(net.c(0).seen.freezes.is_empty() && net.c(1).seen.freezes.is_empty());
    net.run_for(300);
    assert!(net.c(0).seen.freezes.is_empty(), "no freeze in the air");
    net.cm(0).set_on_ground(true);
    net.run_for(1_200);
    for i in 0..2 {
        assert_eq!(net.c(i).seen.freezes, vec![true], "client {i}");
    }
    let landed = freeze_after_result(&net, 0).unwrap().1;
    // the next update after the landing (15 Hz) plus the tick
    assert!(
        (300..=420).contains(&landed),
        "landing freeze {landed} ms after 0x46"
    );
    let waited = freeze_after_result(&net, 1).unwrap().1;
    assert!(
        (1_000..=1_060).contains(&waited),
        "wait freeze {waited} ms after 0x46"
    );
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("frozen at the landing after the round result")));
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("frozen after the landing wait")));

    // -roundfreeze off: no freeze at all
    let cfg = ServerTuning {
        game: GameTuning {
            round_end_freeze: false,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::two_alive(cfg, room, [0.0, 0.0]);
    let started = net.c(0).seen.world_ready_sent.unwrap();
    assert!(net.run_until(started + 61_000, |n| n.c(1).seen.round_result.is_some()));
    net.run_for(1_500);
    for i in 0..2 {
        assert!(net.c(i).seen.freezes.is_empty(), "client {i}");
    }
}
/// DSV-473 (on by default, settles DSV-98): with `respawn_match_begin` each respawn's spawn set is followed by a 0x45
/// without the intro (the seconds left, the waiting flag clear) and the 2009 client's round-start voice (every 0x45
/// taken in player state 1) plays again, after the 0x67 [1] of that spawn set; `-respawnbegin off` keeps the reference stack
/// order (`GS_MATCH_BEGIN` once per player and round, its first spawn), so the voice plays once.
// DISASM: cshell+0x49f00..0x49f7b (voice gate), cshell+0x4a0ff (the intro banner latch)
// REFERENCE: one 0x45 per link and round (tdm6 c6 seq 734 only, five respawns after it)
// OBSERVED: playtest report (the 2009 client says "round start" at each respawn)
// DC-DEFAULT: DSV-473
#[test]
fn round_start_voice_once_and_at_each_respawn_when_asked() {
    assert!(GameTuning::default().respawn_match_begin, "on by default");
    for on in [false, true] {
        let cfg = ServerTuning {
            game: GameTuning {
                default_damage: 200.0,
                respawn_wait_ms: Some(2_500),
                respawn_match_begin: on,
                ..GameTuning::default()
            },
            ..ServerTuning::default()
        };
        let mut net = Net::with(cfg, tables(), "room key=TDM type=1 goal=10 ff=0 time=600");
        net.add(1, "Dave", 0.0);
        net.add(2, "Big Erin", 0.0);
        assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
            && n.c(1).life_state() == 1));
        net.run_for(500);
        let intro = net.c(1).seen.intro.clone();
        assert!(intro.is_some(), "first spawn: 0x45 with the intro");
        assert_eq!(net.c(1).seen.round_start_cues.len(), 1);
        net.shoot(0, 1, 1, [0.0; 3]);
        assert_eq!(net.c(0).seen.kills.len(), 1);
        assert!(net.run_until(net.now + 6_000, |n| n.c(1).life_state() == 1));
        net.run_for(300);
        let s = &net.c(1).seen;
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        let respawned = s.life_states.last().expect("state 1").0;
        if on {
            assert_eq!(s.match_begins.len(), 2, "0x45 after the respawn");
            assert_eq!(s.round_start_cues.len(), 2);
            let (t, waiting, _) = s.round_start_cues[1];
            assert!(t >= respawned && !waiting);
            let (secs, banner) = s.match_begin.expect("0x45");
            assert!((590..600).contains(&secs) && !banner, "{secs} {banner}");
            assert_eq!(s.intro, intro, "no intro block in the respawn 0x45");
        } else {
            assert_eq!(s.match_begins.len(), 1, "the reference stack's single 0x45");
            assert_eq!(s.round_start_cues.len(), 1);
        }
        // the killer did not respawn: one 0x45 either way
        assert_eq!(net.c(0).seen.match_begins.len(), 1);
    }
}
// ------------------------------------------------------------------------------------------ weapon drops (DSV-93..95)

/// Two players alive with the synthetic character tables (weapons 3 and 4 with third-person models, loadout 3, 4)
/// and the drop rules `d`; `users` replace `user` lines (loadouts of their own).
fn drop_net(d: dc_dserver::game::drops::DropTuning, room: &str, users: &[&str]) -> Net {
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![3, 4],
            default_damage: 200.0,
            respawn_wait_ms: Some(6_000),
            corpse_ms: 4_000,
            drops: d,
            // shots right after a spawn: the shield (DSV-156) is not under test here
            spawn_shield: false,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(cfg, character_tables(), room);
    for line in users {
        assert!(net.server.handle_line(0, line));
    }
    net.server.insert_rig("BODY\\b.ltb", rig());
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(1500);
    net
}

/// The client's dropped weapons: (object, weapon, rounds, position) from the type-0x17 kind-6 blobs.
fn drop_objects(c: &SynthClient) -> Vec<(u16, u16, u16, [f32; 3])> {
    c.seen
        .objects
        .iter()
        .filter_map(|(o, s)| {
            let b = s.blob.as_ref()?;
            let mut r = b.reader();
            if r.read_u8() != 0x17 {
                return None;
            }
            let flags = (r.read_bit(), r.read_bit(), r.read_bit());
            let name = r.read_zstr8();
            let kind = r.read_u32();
            if kind != 6 {
                return None;
            }
            let (weapon, rounds) = (r.read_u16(), r.read_u16());
            (flags == (false, false, true) && name.is_empty() && r.remaining_bits() == 0)
                .then_some((*o, weapon, rounds, s.pos.unwrap_or(s.create_pos)))
        })
        .collect()
}

/// DSV-93 / DSV-94: Erin dies holding weapon 3: every client gets one kind-6 pickup of weapon 3 with her 120 rounds
/// (clip 30 + stock 90), drawn with the weapon's third-person model at her body (16 units above the bottom of her
/// box); a second reader parses the groups; Dave, who carries weapon 3 too, walks over it after the arm time and
/// gets the rounds by a 0x66 kind-1 count (stock 90 + 120, CLI-581), and the object is removed from both clients.
#[test]
fn weapon_drop_falls_at_the_body_and_gives_its_rounds() {
    let mut net = drop_net(
        Default::default(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
        &[],
    );
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(100);
    let mut drop_obj = 0;
    for i in 0..2 {
        let c = net.c(i);
        assert!(c.seen.errors.is_empty(), "client {i}: {:?}", c.seen.errors);
        let d = drop_objects(c);
        assert_eq!(d.len(), 1, "client {i}: one dropped weapon");
        let (obj, weapon, rounds, pos) = d[0];
        assert_eq!((weapon, rounds), (3, 120), "client {i}");
        assert!(
            (pos[0] - BLUE[0]).abs() < 2.0
                && (pos[2] - BLUE[2]).abs() < 2.0
                && (pos[1] - (BLUE[1] - 50.0 + 16.0)).abs() < 2.0,
            "client {i}: at the body {pos:?}"
        );
        let s = &c.seen.objects[&obj];
        assert_eq!((s.kind, s.flags), (1, Some(0x1001)));
        assert_eq!(s.anim, vec![1]);
        let model = &c.seen.files[&s.models[0]].1;
        let hh = character_tables()
            .weapons
            .hand_model(3)
            .unwrap()
            .hh_model
            .clone();
        assert_eq!(
            model,
            &dc_dserver::chars::announce_spelling(&hh.unwrap()).into_bytes(),
            "client {i}: the weapon's third-person model"
        );
        drop_obj = obj;
        let mut t = dc_botclient::objects::ObjectTable::default();
        for g in &c.seen.groups {
            t.apply_group(g);
        }
        assert!(t.errors.is_empty(), "client {i}: {:?}", t.errors);
    }
    assert_eq!(net.server.game().weapon_drops().len(), 1);
    // Dave walks to the body; the drop goes when he is there
    let gives_before = net.c(0).seen.weapon_amounts.len();
    let notes_before = net.c(0).seen.ammo_notes.len();
    net.cm(0).walk_to(Some(BLUE), 20.0);
    assert!(net.run_until(net.now + 6_000, |n| n
        .server
        .game()
        .weapon_drops()
        .is_empty()));
    net.run_for(200);
    let p = net.c(0).position();
    assert!(
        (p[0] - BLUE[0]).abs() < 50.0 && (p[2] - BLUE[2]).abs() < 50.0,
        "taken near the body {p:?}"
    );
    // CLI-581: the rounds come as a 0x66 kind-1 count (no pickup notice, so no weapon change and no select one-shot
    // the other clients would see); no 0xb2
    assert!(
        net.c(0).seen.weapon_amounts[gives_before..].is_empty(),
        "no 0xb2 for a weapon he holds"
    );
    let notes: Vec<(u16, f32, u8)> = net.c(0).seen.ammo_notes[notes_before..]
        .iter()
        .map(|n| (n.1, n.3, n.4))
        .collect();
    assert_eq!(notes, [(3, 210.0, 0)], "the rounds into his weapon 3");
    for i in 0..2 {
        let c = net.c(i);
        assert!(drop_objects(c).is_empty(), "client {i}: removed");
        assert!(c.seen.removed.contains(&drop_obj), "client {i}");
    }
    assert!(net.logs.iter().any(|l| l.contains("taken by client")));
}

/// DSV-94: a player without the weapon and with its slot empty gets the weapon into that slot (0xb2 slot 1, the
/// stock outside the full clip, `class_pos` 1 behind his weapon 3 of the same class); a player whose slot holds
/// another weapon takes nothing and the drop stays.
#[test]
fn weapon_drop_fills_an_empty_slot_and_never_replaces_a_weapon() {
    // Erin carries only weapon 4 (slot 1), Dave only weapon 3 (slot 0)
    let mut net = drop_net(
        Default::default(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
        &[
            "user 7 3 99 0 120 Dave loadout=0:3",
            "user 8 4 55 1 0 Big_Erin loadout=1:4",
        ],
    );
    assert_eq!(net.c(0).seen.weapons.iter().filter(|w| w.1 == 4).count(), 0);
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(100);
    assert_eq!(
        drop_objects(net.c(0)).first().map(|d| (d.1, d.2)),
        Some((4, 120))
    );
    net.cm(0).walk_to(Some(BLUE), 20.0);
    assert!(net.run_until(net.now + 5_000, |n| n
        .server
        .game()
        .weapon_drops()
        .is_empty()));
    net.run_for(200);
    assert_eq!(net.c(0).seen.weapon_amounts.last(), Some(&(4, 90.0)));
    let held = net.server.game().players()[&net.pid(0)].weapons.clone();
    assert!(
        held.iter()
            .any(|w| w.weapon == 4 && w.slot == 1 && w.clip == 30 && w.stock == 90),
        "{held:?}"
    );
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("taken by client 0: Weapon { slot: 1 }")));
    // Dave holds weapon 4 in slot 0; Erin drops weapon 3 (slot 0): nothing is replaced, the drop stays
    let mut net = drop_net(
        Default::default(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
        &["user 7 3 99 0 120 Dave loadout=0:4"],
    );
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(100);
    assert_eq!(drop_objects(net.c(0)).first().map(|d| d.1), Some(3));
    net.cm(0).walk_to(Some(BLUE), 20.0);
    net.run_for(4_000);
    assert_eq!(net.server.game().weapon_drops().len(), 1, "not taken");
    assert_eq!(drop_objects(net.c(0)).len(), 1);
}

/// DSV-301 over the loopback: Dave holds weapon 4 in slot 0 and weapon 5 in the backpack (weapon slot 4) and has
/// the backpack gun out (0x7f); his use on Erin's dropped weapon 3 replaces the backpack gun (0xb2 slot 4), his slot-0
/// weapon 4 stays and the backpack weapon 5 is the one dropped.
#[test]
fn the_use_key_swaps_the_drawn_backpack_gun() {
    let use_body = |obj: u16| {
        let mut b = BitWriter::new();
        b.write_u16(obj).write_u8(0);
        b
    };
    let mut net = drop_net(
        Default::default(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
        &["user 7 3 99 0 120 Dave loadout=0:4,10:5"],
    );
    let held = net.server.game().players()[&net.pid(0)].weapons.clone();
    assert!(
        held.iter().any(|w| w.weapon == 5 && w.slot == 4),
        "backpack gun: {held:?}"
    );
    let mut ch = BitWriter::new();
    ch.write_u8(4).write_u16(5).write_u16(0);
    net.cm(0).send_raw_shell(0x7f, &ch);
    net.run_for(200);
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(100);
    let (drop_obj, weapon, _, _) = drop_objects(net.c(0))[0];
    assert_eq!(weapon, 3);
    net.cm(0).walk_to(Some(BLUE), 20.0);
    net.run_for(4_000);
    net.cm(0).send_raw_shell(0x59, &use_body(drop_obj));
    net.run_for(300);
    let held = net.server.game().players()[&net.pid(0)].weapons.clone();
    assert!(
        held.iter().any(|w| w.weapon == 3 && w.slot == 4),
        "the drop took the backpack slot: {held:?}"
    );
    assert!(
        held.iter().any(|w| w.weapon == 4 && w.slot == 0),
        "slot 0 kept: {held:?}"
    );
    assert!(!held.iter().any(|w| w.weapon == 5), "{held:?}");
    // the test tables give weapon 5 no third-person model, so it is not laid down; the server tries to
    assert!(
        net.logs
            .iter()
            .any(|l| l.contains("weapon 5 has no third-person model")),
        "the swapped backpack gun is the one dropped"
    );
}

/// DSV-170: the use key on a dropped weapon of another kind swaps it in. Dave holds weapon 4 in slot 0; Erin drops
/// weapon 3 (slot 0, 120 rounds). Walking over it takes nothing; a use from afar is refused; standing on it, his
/// `UP_USE` `[drop object][0]` gets him weapon 3 by 0xb2 (full clip 30, stock 90) in slot 0, the taken object is
/// removed and his weapon 4 falls where he stands as a new kind-6 drop with its rounds.
#[test]
fn the_use_key_swaps_a_dropped_weapon_for_the_held_one() {
    let use_body = |obj: u16| {
        let mut b = BitWriter::new();
        b.write_u16(obj).write_u8(0);
        b
    };
    let mut net = drop_net(
        Default::default(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
        &["user 7 3 99 0 120 Dave loadout=0:4"],
    );
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(100);
    let (drop_obj, weapon, rounds, _) = drop_objects(net.c(0))[0];
    assert_eq!((weapon, rounds), (3, 120));
    let held_4 = net.server.game().players()[&net.pid(0)]
        .weapons
        .iter()
        .find(|w| w.weapon == 4)
        .map(|w| w.carried())
        .expect("Dave carries weapon 4");
    // from the spawn point, far from the body: refused
    net.run_for(1_200);
    net.cm(0).send_raw_shell(0x59, &use_body(drop_obj));
    net.run_for(200);
    assert_eq!(
        net.server.game().weapon_drops().len(),
        1,
        "refused from afar"
    );
    net.cm(0).walk_to(Some(BLUE), 20.0);
    net.run_for(4_000);
    assert_eq!(
        net.server.game().weapon_drops().len(),
        1,
        "walking over it takes nothing"
    );
    let gives_before = net.c(0).seen.weapon_amounts.len();
    net.cm(0).send_raw_shell(0x59, &use_body(drop_obj));
    net.run_for(300);
    assert_eq!(
        net.c(0).seen.weapon_amounts[gives_before..],
        [(3, 90.0)],
        "0xb2 of weapon 3"
    );
    let held = net.server.game().players()[&net.pid(0)].weapons.clone();
    assert!(
        held.iter()
            .any(|w| w.weapon == 3 && w.slot == 0 && w.clip == 30 && w.stock == 90),
        "{held:?}"
    );
    assert!(!held.iter().any(|w| w.weapon == 4), "{held:?}");
    let drops = net.server.game().weapon_drops();
    assert_eq!(drops.len(), 1, "{drops:?}");
    assert_eq!(
        (drops[0].1, drops[0].2),
        (4, held_4),
        "his weapon 4 lies there"
    );
    let p = net.c(0).position();
    assert!(
        (drops[0].3[0] - p[0]).abs() < 2.0 && (drops[0].3[2] - p[2]).abs() < 2.0,
        "at his feet {:?} vs {p:?}",
        drops[0].3
    );
    for i in 0..2 {
        let c = net.c(i);
        assert!(
            c.seen.removed.contains(&drop_obj),
            "client {i}: taken drop removed"
        );
        let d = drop_objects(c);
        assert_eq!(d.len(), 1, "client {i}: {d:?}");
        assert_eq!(d[0].1, 4, "client {i}: the swapped weapon's drop");
    }
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("taken by client 0: Swap { index: 0 }")));
}

/// DSV-95: a drop nobody takes is removed after its lifetime; a client that enters the world while a drop lies there
/// gets its create; the kill that ends the round drops nothing and the round end removes every drop; switched off,
/// nothing drops.
#[test]
fn weapon_drop_lifetime_late_joiner_and_round_end() {
    let d = dc_dserver::game::drops::DropTuning {
        lifetime_ms: 3_000,
        ..Default::default()
    };
    let mut net = drop_net(d, "room key=TDM type=1 goal=20 ff=0 time=0", &[]);
    net.shoot(0, 1, 1, [0.0; 3]);
    let t_drop = net.now;
    net.run_for(100);
    // a third client joins while the drop lies in the world
    let t = net.now;
    assert!(net.server.handle_line(t, "user 9 5 77 0 0 Carl"));
    net.add(3, "Carl", 0.0);
    assert!(net.run_until(t + 2_500, |n| !drop_objects(n.c(2)).is_empty()));
    assert_eq!(drop_objects(net.c(2))[0].1, 3);
    net.run_until(t_drop + 3_300, |_| false);
    assert!(net.server.game().weapon_drops().is_empty());
    for i in 0..3 {
        assert!(drop_objects(net.c(i)).is_empty(), "client {i}: expired");
    }
    assert!(net.logs.iter().any(|l| l.contains("drop 1 expired")));
    // goal 2: the first kill drops, the goal kill does not, and the end removes the first drop
    let d = dc_dserver::game::drops::DropTuning {
        lifetime_ms: 0,
        ..Default::default()
    };
    let mut net = drop_net(d, "room key=TDM type=1 goal=2 ff=0 time=0", &[]);
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(100);
    assert_eq!(drop_objects(net.c(0)).len(), 1);
    assert!(net.run_until(net.now + 9_000, |n| n.c(1).life_state() == 1));
    net.run_for(300);
    net.shoot(0, 1, 1, [0.0; 3]);
    assert!(net.run_until(net.now + 2_000, |n| n.c(0).seen.round_result.is_some()));
    net.run_for(200);
    assert!(net.server.game().weapon_drops().is_empty());
    assert!(
        drop_objects(net.c(0)).is_empty(),
        "removed at the round end"
    );
    let drops_made = net
        .logs
        .iter()
        .filter(|l| l.contains("drops: drop ") && l.contains(" rounds from client"))
        .count();
    assert_eq!(drops_made, 1, "no drop on the goal kill");
    // switched off: nothing drops
    let d = dc_dserver::game::drops::DropTuning {
        enabled: false,
        ..Default::default()
    };
    let mut net = drop_net(d, "room key=TDM type=1 goal=20 ff=0 time=0", &[]);
    net.shoot(0, 1, 1, [0.0; 3]);
    net.run_for(200);
    assert!(drop_objects(net.c(0)).is_empty());
}

/// The type-0x06 shot effect records a client received: (`first`, `b`, `c`, `to`).
fn shot_records(c: &SynthClient) -> Vec<(u16, u16, u8, [f32; 3])> {
    c.seen
        .effects
        .iter()
        .filter(|e| e.1 == dc_dserver::gs::FX_SHOT)
        .map(|e| {
            let mut r = e.2.reader();
            let first = r.read_u16();
            r.skip_bits(6 * 8 + 8);
            let (b, cl) = (r.read_u16(), r.read_u8());
            r.skip_bits(96);
            (first, b, cl, [r.read_f32(), r.read_f32(), r.read_f32()])
        })
        .collect()
}

/// The records of a client's own shots after which the 2009 client plays its hit sound (`[Guns] DamageFXSound1/2`):
/// `c` = its own id and bit 0x4000 of `b` clear, so the flag survives `(row | 0x4000) & !b`.
// DISASM: cshell+0x2c8b36..0x2c8b8e (extra flags := (row | 0x4000) & !b; own shot plays the feedback only while 0x4000
// survives), cshell+0x2c8b4a..0x2c8be6 -> cshell+0x201c00 (the sound)
fn hit_sounds(c: &SynthClient, own: u8) -> usize {
    shot_records(c)
        .iter()
        .filter(|r| r.2 == own && r.1 & 0x4000 == 0)
        .count()
}

/// DSV-101: a claim the server refuses by its victim gets the miss form of the shot effect on every link (no record
/// names the character, `b` 0x4f10, no continuation), so the shooter's client plays no hit sound: a team mate with
/// friendly fire off, a corpse. An accepted hit keeps the hit form and its continuation to the wall behind; a claim
/// by a dead shooter gets no record at all. `-combat refused_hits=hit` restores the hit form for refused claims.
// OBSERVED: capture 20261025_013028_botduel4 (40 claims on a dead character: `first` 0 or none,
// b 0x4f10 / 0x4f00, no continuation; one claim by a dead shooter: no record; tests/refused_cap.rs; REFERENCE);
// 20261025_171754_rustclient (ours before DSV-101: friendly and victim_dead claims got 0x0f11 + continuation)
#[test]
fn refused_claims_get_the_miss_form_and_no_hit_sound() {
    use dc_dserver::gs::{SHOT_MASK_EXIT, SHOT_MASK_HIT, SHOT_MASK_MISS};
    use dc_sim::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID, V3};
    const WALL_X: f32 = 1500.0;
    for refused_miss in [true, false] {
        let mut w = world();
        w.spawns.push(SpawnPoint {
            pos: [-300.0, 0.0, 500.0],
            rot: [0.0, 1.5, 0.0],
            team_filter: Some(b"Red".to_vec()),
        });
        // one wall behind the enemy's spawn: the continuation of a hit on +x ends there
        let wall = ConvexPoly::from_corners(
            vec![
                V3::new(WALL_X, -500.0, 2000.0),
                V3::new(WALL_X, 1000.0, 2000.0),
                V3::new(WALL_X, 1000.0, -2000.0),
                V3::new(WALL_X, -500.0, -2000.0),
            ],
            SURF_SOLID,
        );
        w.trace = Some(std::sync::Arc::new(CollisionWorld::from_parts(
            vec![CollModel::from_polys(&[wall])],
            &[],
        )));
        let mut cfg = ServerTuning {
            game: GameTuning {
                default_damage: 60.0,
                respawn_wait_ms: Some(60_000),
                ..GameTuning::default()
            },
            ..ServerTuning::default()
        };
        if !refused_miss {
            cfg.game.set_combat("refused_hits", "hit").unwrap();
        }
        let mut net = Net::with_world(cfg, w, tables(), "room key=TDM type=1 goal=20 ff=0 time=0");
        assert!(net.server.handle_line(0, "user 9 5 10 0 0 Cid"));
        net.add(1, "Dave", 0.0);
        net.add(2, "Big Erin", 0.0);
        net.add(3, "Cid", 0.0);
        assert!(net.run_until(20_000, |n| (0..3).all(|i| n.c(i).life_state() == 1)));
        net.run_for(500);
        let (dave, erin, cid) = (net.pid(0), net.pid(1), net.pid(2));
        let players = net.server.game().players();
        assert_eq!(
            players[&dave].team, players[&cid].team,
            "Dave and Cid are team mates"
        );
        assert_ne!(players[&dave].team, players[&erin].team);
        let (dpos, cpos) = (players[&dave].pos, players[&cid].pos);
        assert!(
            ((dpos[0] - cpos[0]).powi(2) + (dpos[2] - cpos[2]).powi(2)).sqrt() > 100.0,
            "the team mates stand apart: {dpos:?} {cpos:?}"
        );
        let (o_erin, o_cid) = (net.obj(1), net.obj(2));
        let mark = |n: &Net| {
            (0..3)
                .map(|i| shot_records(n.c(i)).len())
                .collect::<Vec<_>>()
        };
        let since_mark = |n: &Net, m: &[usize], i: usize| shot_records(n.c(i))[m[i]..].to_vec();
        // 1) the friendly claim (ff 0): refused, drawn as a miss everywhere, no hit sound for Dave
        let m = mark(&net);
        net.shoot(0, 2, 1, [0.0; 3]);
        net.run_for(200);
        let h = net.server.game().hit_counters().clone();
        assert_eq!(h.get(HitVerdict::Friendly), 1);
        assert!(
            net.c(2).seen.damage.is_empty(),
            "no damage to the team mate"
        );
        for i in 0..3 {
            let r = since_mark(&net, &m, i);
            if refused_miss {
                assert_eq!(r.len(), 1, "client {i}: one record, no continuation: {r:?}");
                assert_ne!(r[0].0, o_cid, "client {i}: the team mate is not named");
                assert_eq!((r[0].1, r[0].2), (SHOT_MASK_MISS, dave), "client {i}");
            } else {
                assert_eq!(
                    r[0].0, o_cid,
                    "client {i}: refused_hits=hit names the team mate"
                );
                assert_eq!(r[0].1, SHOT_MASK_HIT);
            }
        }
        assert_eq!(
            hit_sounds(net.c(0), dave),
            usize::from(!refused_miss),
            "friendly"
        );
        // 2) an accepted hit on the enemy: the hit form and the continuation to the wall behind, one hit sound
        let m = mark(&net);
        net.shoot(0, 1, 1, [0.0; 3]);
        net.run_for(200);
        assert_eq!(net.server.game().hit_counters().accepted(), 1);
        assert_eq!(net.c(1).seen.damage.len(), 1);
        for i in 0..3 {
            let r = since_mark(&net, &m, i);
            assert_eq!(r.len(), 2, "client {i}: hit and continuation: {r:?}");
            assert_eq!(
                (r[0].0, r[0].1, r[0].2),
                (o_erin, SHOT_MASK_HIT, dave),
                "client {i}"
            );
            assert_eq!(
                (r[1].0, r[1].1, r[1].2),
                (0, SHOT_MASK_EXIT, dave),
                "client {i}"
            );
            assert!(
                (r[1].3[0] - WALL_X).abs() < 1.0,
                "client {i}: ends at the wall {:?}",
                r[1].3
            );
        }
        assert_eq!(
            hit_sounds(net.c(0), dave),
            1 + usize::from(!refused_miss),
            "accepted"
        );
        // 3) the second hit kills; then a claim on the corpse: refused, drawn as a miss past the body to the wall
        net.shoot(0, 1, 1, [0.0; 3]);
        net.run_for(300);
        assert_eq!(net.c(0).seen.kills.len(), 1, "Erin is dead");
        let sounds = hit_sounds(net.c(0), dave);
        let m = mark(&net);
        net.shoot(0, 1, 1, [0.0; 3]);
        net.run_for(200);
        assert_eq!(
            net.server.game().hit_counters().get(HitVerdict::VictimDead),
            1
        );
        for i in 0..3 {
            let r = since_mark(&net, &m, i);
            if refused_miss {
                assert_eq!(r.len(), 1, "client {i}: no continuation: {r:?}");
                assert_eq!(
                    (r[0].0, r[0].1, r[0].2),
                    (0, SHOT_MASK_MISS, dave),
                    "client {i}: the world"
                );
                assert!(
                    (r[0].3[0] - WALL_X).abs() < 1.0,
                    "client {i}: past the body {:?}",
                    r[0].3
                );
            } else {
                assert_eq!((r[0].0, r[0].1), (o_erin, SHOT_MASK_HIT), "client {i}");
            }
        }
        assert_eq!(
            hit_sounds(net.c(0), dave),
            sounds + usize::from(!refused_miss),
            "corpse"
        );
        // 4) the dead player's own claim: no record at all
        let m = mark(&net);
        let target = net.obj(0);
        assert!(net.cm(1).fire_at(target, [0.0; 3], true));
        net.run_for(200);
        assert_eq!(
            net.server
                .game()
                .hit_counters()
                .get(HitVerdict::ShooterDead),
            1
        );
        for i in 0..3 {
            assert!(
                since_mark(&net, &m, i).is_empty(),
                "client {i}: nothing for a dead shooter"
            );
        }
    }
}

// ------------------------------------------------------------------------------------------- DSV-102 cone check

/// A synthetic weapon row with the perturb keys of the ROM shape (made-up values).
const CONE_ROW: dc_dserver::cone::ConeBounds = dc_dserver::cone::ConeBounds {
    min: 4,
    max: 12,
    zoomed_min: Some(1),
    zoomed_max: Some(2),
    triple_min: None,
    triple_max: None,
};

fn cone_net(mode: &str, seed: u32) -> Net {
    let doc = dc_bute::ButeDoc::parse(concat!(
        "[Weapon0]\n",
        "MinPerturb = 4\n",
        "MaxPerturb = 12\n",
        "ZoomedMinPerturb = 1\n",
        "ZoomedMaxPerturb = 2\n",
    ));
    let mut t = tables();
    t.weapons = dc_dserver::rom_tables::WeaponTable::from_doc(&doc);
    assert_eq!(t.weapons.cone_bounds(0), Some(&CONE_ROW));
    let mut cfg = ServerTuning::default();
    cfg.game.start_order = StartOrder::Spec;
    cfg.game.set_combat("cone_check", mode).unwrap();
    cfg.game.set_combat("shot_seed", &seed.to_string()).unwrap();
    let mut net = Net::with(cfg, t, "room key=TDM type=1 goal=10 ff=0");
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    net
}

fn cone_shot(frac: u8, zoom: bool, forged: bool) -> dc_dserver::synth_client::ConeShot {
    let h = match CONE_ROW.half_width(zoom, false, 1.0, f32::from(frac) / 255.0) {
        dc_dserver::cone::HalfWidth::H(h) => h,
        x => panic!("{x:?}"),
    };
    dc_dserver::synth_client::ConeShot {
        frac,
        scale: 1.0,
        zoom,
        h,
        forged,
    }
}

/// DSV-102: pellets built like the 2009 client's (the table drawn from the counter the 0x7d set, across the table's
/// wrap) pass; a forged "no spread" direction is `cone_off`: logged and still damaging in `log` mode, refused in
/// `refuse` mode; a client that never drew from the table is `cone_na` and never refused.
#[test]
fn cone_check_passes_the_2009_cone_and_logs_or_refuses_a_forged_direction() {
    let table = dc_dserver::cone::shared_generated();
    // log mode, the counter starts 10 below the table size: the fifth pellet wraps
    let mut net = cone_net("log", 9_990);
    assert_eq!(net.c(0).seen.shot_seed, Some(9_990));
    let target = net.obj(1);
    for i in 0..6 {
        let shot = cone_shot(if i % 2 == 0 { 128 } else { 255 }, i == 5, false);
        assert!(net.cm(0).fire_cone_at(target, [0.0; 3], table, shot, i < 2));
        net.run_for(20);
    }
    assert_eq!(net.c(0).seen.shot_counter, 2, "wrapped at the table size");
    let c = net.server.game().cone_counters().clone();
    assert_eq!(
        (c.ok, c.off, c.na, c.stat_skew, c.unbounded),
        (6, 0, 0, 0, 0),
        "{c:?}"
    );
    assert_eq!(net.c(1).seen.damage.len(), 2);
    // a forged direction: logged, damage still applied
    assert!(net
        .cm(0)
        .fire_cone_at(target, [0.0; 3], table, cone_shot(128, false, true), true));
    net.run_for(40);
    let off: Vec<&String> = net.logs.iter().filter(|l| l.contains("cone_off")).collect();
    assert_eq!(off.len(), 1, "{off:?}");
    assert!(
        off[0].contains("client")
            && off[0].contains("weapon 0 stat 4")
            && off[0].contains("row bounds 8..8")
    );
    assert!(!off[0].contains("refused"));
    assert_eq!(net.c(1).seen.damage.len(), 3);
    assert_eq!(
        net.server.game().cone_summary(),
        "cone: 7 pellets, 6 ok, 1 off (worst 7.0e-3 rad, 0 refused), 0 na, 0 unchecked; 0 without row bounds; stat \
         skew 0; mode log"
    );
    net.server.finish(net.now);
    net.step();
    assert!(net.logs.iter().any(|l| l.ends_with("session cone: 7 pellets, 6 ok, 1 off (worst 7.0e-3 rad, 0 refused), \
                                                  0 na, 0 unchecked; 0 without row bounds; stat skew 0; mode log")));

    // refuse mode: the real pellet damages, the forged one does not and its claim gets the verdict cone_off
    let mut net = cone_net("refuse", 0);
    let target = net.obj(1);
    assert!(net
        .cm(0)
        .fire_cone_at(target, [0.0; 3], table, cone_shot(40, false, false), true));
    net.run_for(40);
    assert_eq!(net.c(1).seen.damage.len(), 1);
    assert!(net
        .cm(0)
        .fire_cone_at(target, [0.0; 3], table, cone_shot(40, false, true), true));
    net.run_for(40);
    assert_eq!(net.c(1).seen.damage.len(), 1, "refused");
    assert_eq!(net.server.game().hit_counters().get(HitVerdict::ConeOff), 1);
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("cone_off") && l.ends_with("), refused")));
    // a client that never drew from the table (the old synthetic pellet): cone_na, noted once, never refused
    net.shoot(1, 0, 2, [0.0; 3]);
    assert_eq!(net.c(0).seen.damage.len(), 2);
    let c = net.server.game().cone_counters().clone();
    assert_eq!((c.ok, c.off, c.na, c.refused), (1, 1, 2, 1), "{c:?}");
    assert_eq!(
        net.logs.iter().filter(|l| l.contains("(cone_na:")).count(),
        1
    );
    // off: nothing checked
    let mut net = cone_net("off", 0);
    let target = net.obj(1);
    assert!(net
        .cm(0)
        .fire_cone_at(target, [0.0; 3], table, cone_shot(40, false, true), true));
    net.run_for(40);
    assert_eq!(net.c(1).seen.damage.len(), 1);
    assert_eq!(net.server.game().cone_counters().pellets(), 0);
    assert_eq!(net.server.game().cone_summary(), "cone: check off");
}
/// CLI-170 / DSV-110: the 2009 client has no end-of-match camera cut and no input lock (the 0x46 handler
/// cshell+0x4a608..0x4aa42 moves no camera for a living player; the scoreboard panel is the client's own 3000 ms
/// timer cshell+0xc02e0; the only cut is the result screen 0x3f, interface state 5), so between the result and the
/// result screen a living player gets nothing that turns or moves his view or changes his player state: no 0x67, no
/// 0x6a view set, no 0xa3 camera kick, no 0x99 level exit; the one 0xa0 is the DSV-77 freeze. The order stays
/// 0x46 (+ freeze), +3 s 0x47 .. 0x48, +3 s 0x3f for both a time-limit and a goal end.
// DISASM: cshell+0x4a608..0x4aa42, cshell+0xc02e0; OBSERVED: capture 20261025_204341_realclient
// (goal end: the user's client sent 82 updates in the 6 s from 0x46 to 0x3f, 70 of them with a new view)
#[test]
fn round_end_sends_no_camera_or_state_change_to_the_living() {
    let check = |net: &Net, i: usize| {
        let ids = &net.c(i).seen.shell_ids;
        let p = ids.iter().position(|x| x.1 == 0x46).expect("0x46");
        let q = ids.iter().position(|x| x.1 == 0x3f).expect("0x3f");
        let t46 = ids[p].0;
        let between: Vec<u8> = ids[p..q]
            .iter()
            .map(|x| x.1)
            .filter(|&x| x != 0xa6)
            .collect();
        for bad in [0x67u8, 0x6a, 0xa3, 0x99] {
            assert!(
                !between.contains(&bad),
                "client {i}: 0x{bad:x} in {between:x?}"
            );
        }
        assert_eq!(
            between.iter().filter(|&&x| x == 0xa0).count(),
            1,
            "client {i}: only the freeze: {between:x?}"
        );
        let t = |id: u8| ids[p..=q].iter().find(|x| x.1 == id).unwrap().0 - t46;
        assert!(
            (2_900..=3_100).contains(&t(0x48)),
            "client {i}: 0x48 at +{}",
            t(0x48)
        );
        assert!(
            (5_900..=6_200).contains(&t(0x3f)),
            "client {i}: 0x3f at +{}",
            t(0x3f)
        );
    };
    // time-limit end, both alive
    let mut net = Net::two_alive(
        ServerTuning::default(),
        "room key=TDM type=1 goal=50 ff=0 time=60",
        [0.0, 0.0],
    );
    let started = net.c(0).seen.world_ready_sent.unwrap();
    assert!(net.run_until(started + 61_000, |n| n.c(1).seen.round_result.is_some()));
    net.run_for(6_500);
    for i in 0..2 {
        check(&net, i);
    }
    // goal end: the killer is alive (the victim is dead and gets no freeze)
    let cfg = ServerTuning {
        game: GameTuning {
            default_damage: 100.0,
            respawn_wait_ms: Some(2_100),
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::two_alive(cfg, "room key=TDM type=1 goal=1 ff=0", [0.0, 0.0]);
    net.shoot(0, 1, 1, [0.0; 3]);
    assert!(net.run_until(net.now + 2_000, |n| n.c(0).seen.round_result.is_some()));
    net.run_for(6_500);
    check(&net, 0);
}
/// Four players, friendly fire on: Dave (uid 7) and Carl (uid 9) in one team, Big Erin (uid 8) and Fay (uid 10) in
/// the other; one shot of 200 kills; the respawn wait 2.5 s.
fn four_alive_ff() -> Net {
    let cfg = ServerTuning {
        game: GameTuning {
            default_damage: 200.0,
            respawn_wait_ms: Some(2_500),
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::new(cfg, "room key=TDM type=1 goal=20 ff=1 time=0");
    assert!(net.server.handle_line(0, "user 9 5 10 0 0 Carl"));
    assert!(net.server.handle_line(0, "user 10 6 10 1 0 Fay"));
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    net.add(3, "Carl", 0.0);
    net.add(4, "Fay", 0.0);
    assert!(net.run_until(20_000, |n| (0..4).all(|i| n.c(i).life_state() == 1)));
    net.run_for(500);
    net
}
/// DSV-106 (team kill form): a team kill goes out with `kill_kind` 0, which the 2009 client announces nowhere (no
/// kill-count banner, no head-shot banner on a head hit, no voice line, no note), never takes the match's first-kill
/// flag, sends no 0xc8 and counts neither a kill, a head shot nor the victim's death; the killer's multi-kill chain
/// is neither broken nor extended by it.
#[test]
fn team_kill_and_head_shot_team_kill_announce_nothing() {
    let mut net = four_alive_ff();
    let (dave, erin, carl, fay) = (net.pid(0), net.pid(1), net.pid(2), net.pid(3));
    assert_eq!(net.c(0).seen.own_team, net.c(2).seen.own_team);
    assert_ne!(net.c(0).seen.own_team, net.c(1).seen.own_team);
    // 1) Dave kills his mate Carl with a body shot
    net.shoot(0, 2, 1, [0.0; 3]);
    net.run_for(100);
    for i in 0..4 {
        let k = &net.c(i).seen.kills;
        assert_eq!(k.len(), 1, "client {i}");
        assert_eq!((k[0].victim, k[0].killer, k[0].zone), (carl, dave, 2));
        assert_eq!((k[0].kill_kind, k[0].streak_flag), (0, 0), "client {i}");
        // the 2009 counting rule: same team, nothing counted
        assert_eq!(net.c(i).seen.roster[&dave].kills, 0, "client {i}");
        assert_eq!(net.c(i).seen.roster[&carl].deaths, 0, "client {i}");
        assert!(net.c(i).seen.multi_kills.is_empty(), "no 0xc8");
        assert_eq!(
            net.c(i).seen.team_kills.last().map(|t| (t.1, t.2)),
            Some((0, 0))
        );
    }
    // 2) Carl back; Dave kills him again with a head shot: still level 0, the head mark stays in the feed
    assert!(net.run_until(net.now + 6_000, |n| n.c(2).life_state() == 1));
    net.run_for(300);
    net.shoot(0, 2, 1, [0.0, 50.0, 0.0]);
    net.run_for(100);
    let k = *net.c(0).seen.kills.last().unwrap();
    assert_eq!(
        (k.victim, k.zone, k.kill_kind, k.streak_flag),
        (carl, 1, 0, 0)
    );
    let st = net.server.game().stats();
    assert_eq!(
        (st[&7].kills, st[&7].team_kills, st[&7].headshots),
        (0, 2, 0)
    );
    assert_eq!(st[&9].deaths, 0, "a team kill is no death of the victim");
    // 3) the match's first foe kill takes the first-kill flag; a team kill in between keeps the chain: Dave kills
    // Erin (level 1), Carl again (level 0), then Fay within 4 s of Erin (level 2)
    assert!(net.run_until(net.now + 6_000, |n| n.c(2).life_state() == 1));
    net.run_for(300);
    net.shoot(0, 1, 1, [0.0; 3]);
    net.shoot(0, 2, 1, [0.0; 3]);
    net.shoot(0, 3, 1, [0.0; 3]);
    net.run_for(100);
    let ks: Vec<_> = net.c(1).seen.kills[2..]
        .iter()
        .map(|k| (k.victim, k.kill_kind, k.streak_flag))
        .collect();
    assert_eq!(ks, vec![(erin, 1, 1), (carl, 0, 0), (fay, 2, 0)]);
    assert_eq!(net.c(3).seen.roster[&dave].kills, 2);
    assert!(net.c(0).seen.multi_kills.is_empty(), "no 0xc8");
    let st = net.server.game().stats();
    assert_eq!((st[&7].kills, st[&7].team_kills), (2, 3));
    assert_eq!((st[&8].deaths, st[&10].deaths, st[&9].deaths), (1, 1, 0));
}

/// DC-C2S-13 as amended (DSV-105): each foe kill at most 4 s after the killer's previous one raises the level in
/// 0x6d `kill_kind` (2 = the double-kill form, 3 = the next form, both with the client's own voice line); a longer
/// pause starts again at 1; 0xc8 (the "N COMBO" banner) is never sent.
#[test]
fn double_and_triple_kill_ride_the_kill_message() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_damage: 200.0,
            respawn_wait_ms: Some(2_500),
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::two_alive(cfg, "room key=TDM type=1 goal=20 ff=0 time=0", [0.0, 0.0]);
    let kill_once = |net: &mut Net, head: bool| {
        assert!(net.run_until(net.now + 6_000, |n| n.c(1).life_state() == 1));
        net.run_for(300);
        let off = if head { [0.0, 50.0, 0.0] } else { [0.0; 3] };
        net.shoot(0, 1, 1, off);
        net.run_for(50);
    };
    kill_once(&mut net, false);
    kill_once(&mut net, false);
    kill_once(&mut net, true);
    // a pause longer than the window
    assert!(net.run_until(net.now + 6_000, |n| n.c(1).life_state() == 1));
    net.run_for(4_200);
    kill_once(&mut net, false);
    kill_once(&mut net, false);
    for i in 0..2 {
        let s = &net.c(i).seen;
        let gaps: Vec<u32> = s.kills.windows(2).map(|w| w[1].at - w[0].at).collect();
        assert!(gaps[..2].iter().all(|&g| g <= 4_000), "{gaps:?}");
        assert!(gaps[2] > 4_000, "{gaps:?}");
        let lv: Vec<(u8, u8)> = s.kills.iter().map(|k| (k.kill_kind, k.zone)).collect();
        assert_eq!(
            lv,
            vec![(1, 2), (2, 2), (3, 1), (1, 2), (2, 2)],
            "client {i}"
        );
        assert!(s.multi_kills.is_empty(), "no 0xc8 (client {i})");
    }
    assert_eq!(net.server.game().stats()[&7].kills, 5);
}

/// The level is capped at 7 (the client's last form; its per-level counter would run into the record's later
/// fields above 7) and `multi_kill_min` raises the first multi-kill level.
#[test]
fn kill_level_cap_and_minimum() {
    let cfg = |min: u32| ServerTuning {
        game: GameTuning {
            default_damage: 200.0,
            respawn_wait_ms: Some(2_500),
            multi_kill_window_ms: 60_000,
            multi_kill_min: min,
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    for (min, n, want) in [
        (2, 9, vec![1, 2, 3, 4, 5, 6, 7, 7, 7]),
        (3, 3, vec![1, 1, 3]),
    ] {
        let mut net = Net::two_alive(
            cfg(min),
            "room key=TDM type=1 goal=50 ff=0 time=0",
            [0.0, 0.0],
        );
        for _ in 0..n {
            assert!(net.run_until(net.now + 6_000, |n| n.c(1).life_state() == 1));
            net.run_for(300);
            net.shoot(0, 1, 1, [0.0; 3]);
            net.run_for(50);
        }
        let lv: Vec<u8> = net.c(0).seen.kills.iter().map(|k| k.kill_kind).collect();
        assert_eq!(lv, want, "min {min}");
    }
}

/// A made-up explosive barrel next to the blue spawn point: a breakable brush with 60 hit points whose death
/// switches on a made-up effect and a made-up explosion object (radius 200, 50 damage at the centre).
fn world_with_barrel() -> WorldInfo {
    use dc_dserver::destructibles::{BreakableInfo, DeathAction, ExplosionInfo};
    let mut w = world();
    let at = [BLUE[0] + 40.0, 20.0, BLUE[2]];
    w.brushes.push(dc_dserver::world::BrushInfo {
        name: b"Barrel1".to_vec(),
        role: Some(dc_world::ObjectRole::Breakable),
        pos: at,
        rot: [0.0; 3],
        visible: true,
        solid: true,
        rayhit: true,
        blend: 0,
        fog_disable: false,
        class: dc_dserver::world::BrushClass::Breakable,
        sky: None,
    });
    w.effects.push(dc_dserver::world::EffectInfo {
        name: b"Barrel1Fx".to_vec(),
        pos: at,
        rot: [0.0; 3],
        fx: b"TestBurst".to_vec(),
        looped: false,
        start_on: false,
    });
    w.destructibles.explosions.push(ExplosionInfo {
        name: b"Barrel1Exp".to_vec(),
        pos: [at[0], at[1] + 20.0, at[2]],
        damage: 50.0,
        radius: 200.0,
        impact_fx: None,
    });
    w.destructibles.breakables.push(BreakableInfo {
        name: b"barrel1".to_vec(),
        hit_points: 60.0,
        death: vec![
            DeathAction::Effect(b"barrel1fx".to_vec()),
            DeathAction::Explode(0),
        ],
        skipped: 0,
        script: Vec::new(),
    });
    w
}

/// DSV-115..DSV-118 (world-objects): the barrel is created as a solid visible brush and bound; three hits of the
/// default damage (25) break it: every client sees its flag words lose visible / solid / ray-hit (0x3101 -> 0x0100)
/// and a placed-effect object (type 0x64) appear; the explosion damages the player standing next to it (0x74), not
/// the shooter far away; a later hit claim on the dead barrel is swallowed; the effect object is removed after
/// `destruct.effect_ms`. `-destruct enabled=off`: the barrel never breaks.
#[test]
fn a_shot_barrel_explodes_damages_a_nearby_player_and_its_model_goes() {
    let mut cfg = ServerTuning::default();
    cfg.game.destruct.effect_ms = 1500;
    let mut net = Net::with_world(
        cfg,
        world_with_barrel(),
        tables(),
        "room key=TDM type=1 goal=10 ff=0",
    );
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    let near_blue = |p: [f32; 3]| (p[0] - BLUE[0]).abs() < 50.0 && (p[2] - BLUE[2]).abs() < 50.0;
    let victim = (0..2)
        .find(|&i| near_blue(net.c(i).position()))
        .expect("a player at the blue spawn");
    let shooter = 1 - victim;
    let barrel = net.server.game().breakables()[0].clone();
    assert_eq!(net.server.game().breakables().len(), 1);
    for i in 0..2 {
        let o = &net.c(i).seen.objects[&barrel.obj];
        assert_eq!((o.kind, o.flags), (2, Some(0x3101)), "client {i}");
    }
    let objects_before = net.c(shooter).seen.objects.len();
    for _ in 0..2 {
        net.cm(shooter).fire_at_point(barrel.obj, barrel.pos, true);
        net.run_for(40);
    }
    assert_eq!(net.server.game().breakables()[0].left, 10.0);
    assert!(net.c(victim).seen.damage.is_empty());
    net.cm(shooter).fire_at_point(barrel.obj, barrel.pos, true);
    net.run_for(200);
    assert!(net.server.game().breakables()[0].dead);
    for i in 0..2 {
        let o = &net.c(i).seen.objects[&barrel.obj];
        assert_eq!(o.flags, Some(0x0100), "client {i}: model switched off");
        let fx: Vec<_> = net
            .c(i)
            .seen
            .objects
            .values()
            .filter(|o| o.blob_type == Some(0x64))
            .collect();
        assert_eq!(fx.len(), 1, "client {i}: one death effect");
    }
    assert_eq!(net.c(shooter).seen.objects.len(), objects_before + 1);
    // the explosion: 50 at the centre, the victim stands about 45 units away (inside the full-damage quarter)
    let d = &net.c(victim).seen.damage;
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].1 >= 40 && d[0].1 <= 50, "{d:?}");
    assert!(net.c(shooter).seen.damage.is_empty());
    // a claim on the dead barrel is swallowed, no second explosion
    net.cm(shooter).fire_at_point(barrel.obj, barrel.pos, true);
    net.run_for(100);
    assert_eq!(net.c(victim).seen.damage.len(), 1);
    // the effect object leaves after its lifetime, the brush stays (switched off)
    net.run_for(1600);
    for i in 0..2 {
        assert!(
            !net.c(i)
                .seen
                .objects
                .values()
                .any(|o| o.blob_type == Some(0x64)),
            "client {i}"
        );
        assert!(net.c(i).seen.objects.contains_key(&barrel.obj));
    }
    assert!(
        net.logs.iter().any(|l| l.contains("destroyed by client")),
        "{:?}",
        net.logs.len()
    );

    let mut cfg = ServerTuning::default();
    cfg.game.destruct.enabled = false;
    let mut net = Net::with_world(
        cfg,
        world_with_barrel(),
        tables(),
        "room key=TDM type=1 goal=10 ff=0",
    );
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    let barrel = net.server.game().breakables()[0].clone();
    for _ in 0..5 {
        net.cm(0).fire_at_point(barrel.obj, barrel.pos, true);
        net.run_for(40);
    }
    assert!(!net.server.game().breakables()[0].dead);
    assert_eq!(net.c(1).seen.objects[&barrel.obj].flags, Some(0x3101));
}

/// DSV-119: a thrown grenade's blast breaks the barrel in its radius, and the barrel's explosion follows.
#[test]
fn a_grenade_blast_sets_off_a_barrel() {
    // the synthetic world has no floor: the grenade falls below the spawn height, about 90 units from the barrel
    let mut w = world_with_barrel();
    w.destructibles.breakables[0].hit_points = 40.0;
    let mut net = Net::with_world(
        ServerTuning::default(),
        w,
        throw_tables(),
        "room key=TDM type=1 goal=10 ff=0",
    );
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    let barrel = net.server.game().breakables()[0].clone();
    // at the own feet: the blast's falloff share of 80 beats the barrel's 40 hit points
    let thrower = (0..2)
        .find(|&i| (net.c(i).position()[0] - BLUE[0]).abs() < 50.0)
        .expect("a player at the blue spawn");
    net.cm(thrower).throw((40, 41), [0.0, -1.0, 0.0]);
    let limit = net.now + 8000;
    let broke = net.run_until(limit, |n| n.server.game().breakables()[0].dead);
    assert!(
        broke,
        "{:?}",
        net.logs
            .iter()
            .filter(|l| l.contains("throw") || l.contains("break"))
            .collect::<Vec<_>>()
    );
    net.run_for(200);
    assert_eq!(
        net.c(1 - thrower).seen.objects[&barrel.obj].flags,
        Some(0x0100)
    );
    assert!(net.logs.iter().any(|l| l.contains("explosion")));
}
/// DSV-112: the world camera shot at the round result. The server creates the world's camera object on every client
/// (kind 0, type-0x0d blob, live bit clear); with the round result (0x46) it runs the camera statements of the
/// room's end trigger: the camera goes live (user flag 0x20) at the camera point the trigger names, which the 2009
/// client draws from (cshell+0x1f83d0) with the players' movement off (the blob's movement byte 0, cshell+0x1f8502).
/// The end set keeps its times: 0x48 at +3 s, 0x3f at +6 s. `-camshot off` creates no camera.
// DISASM: cshell+0x1f8439..0x1f8451 (live test), cshell+0x217b4a..0x217be7 (blob); OBSERVED: ROM world scripts
// (every PvP end trigger: camera on, moved to the first camera point; hashes only, no text here)
#[test]
fn round_result_runs_the_end_triggers_world_camera_shot() {
    use dc_dserver::camshot::{
        CameraInfo, CameraPoint, WorldCameras, OBJ_CAMERA_TYPE, UFLAG_CAMERA_LIVE,
    };
    let point = [40.0, 120.0, -60.0];
    let mut w = world();
    w.scripts.cameras = WorldCameras {
        cameras: vec![CameraInfo {
            name: b"Cam0".to_vec(),
            pos: [0.0, 50.0, 0.0],
            rot: [0.0, 0.0, 0.0],
            allow_move: false,
            listener: true,
            fov: [90.0, 75.0],
            end_shot: false,
        }],
        points: vec![CameraPoint {
            name: b"Overview".to_vec(),
            pos: point,
            rot: [0.3, 1.2, 0.0],
        }],
    };
    w.scripts.triggers.push((
        b"RoundEnd".to_vec(),
        vec![
            b"msg cam0 on;".to_vec(),
            b"msg Cam0 (moveto overview);".to_vec(),
        ],
    ));
    w.scripts
        .triggers
        .push((b"RoundStart".to_vec(), vec![b"msg Cam0 off;".to_vec()]));
    let room = "room key=TDM type=1 goal=50 ff=0 time=60 trig=|RoundStart|RoundEnd|";
    let make = |cfg: ServerTuning| {
        let mut net = Net::with_world(cfg, w.clone(), tables(), room);
        net.add(1, "Dave", 0.0);
        net.add(2, "Big Erin", 0.0);
        assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
            && n.c(1).life_state() == 1));
        net.run_for(500);
        net
    };
    let camera = |net: &Net, i: usize| {
        net.c(i)
            .seen
            .objects
            .values()
            .find(|o| o.blob_type == Some(OBJ_CAMERA_TYPE))
            .cloned()
    };
    let mut net = make(ServerTuning::default());
    for i in 0..2 {
        let c = camera(&net, i).expect("camera object");
        assert_eq!(c.kind, 0);
        assert_eq!(c.uflags, Some(0), "client {i}: off before the end");
        let b = c.blob.as_ref().unwrap();
        let mut r = b.reader();
        r.read_u8();
        assert_eq!(r.read_u8(), 0, "movement byte");
        assert_eq!(r.read_u8(), 1, "listener byte");
    }
    let started = net.c(0).seen.world_ready_sent.unwrap();
    assert!(net.run_until(started + 61_000, |n| n.c(1).seen.round_result.is_some()));
    let at = net.now;
    assert!(
        net.run_until(at + 500, |n| camera(n, 0).and_then(|c| c.uflags)
            == Some(UFLAG_CAMERA_LIVE)
            && camera(n, 1).and_then(|c| c.uflags) == Some(UFLAG_CAMERA_LIVE))
    );
    assert!(
        net.now - at <= 200,
        "live {} ms after the result",
        net.now - at
    );
    for i in 0..2 {
        let p = camera(&net, i).unwrap().pos.expect("position");
        for k in 0..3 {
            assert!((p[k] - point[k]).abs() < 2.0, "client {i}: {p:?}");
        }
    }
    assert!(net.server.world_cameras()[0].live);
    net.run_for(6_500);
    for i in 0..2 {
        let ids = &net.c(i).seen.shell_ids;
        let p = ids.iter().position(|x| x.1 == 0x46).unwrap();
        let t = |id: u8| ids[p..].iter().find(|x| x.1 == id).unwrap().0 - ids[p].0;
        assert!(
            (2_900..=3_100).contains(&t(0x48)),
            "client {i}: 0x48 at +{}",
            t(0x48)
        );
        assert!(
            (5_900..=6_200).contains(&t(0x3f)),
            "client {i}: 0x3f at +{}",
            t(0x3f)
        );
        // still live at the result screen (the result screen leaves the world)
        assert_eq!(camera(&net, i).unwrap().uflags, Some(UFLAG_CAMERA_LIVE));
    }
    // off: no camera object
    let net = make(ServerTuning {
        cam_shot: false,
        ..ServerTuning::default()
    });
    assert!(camera(&net, 0).is_none());
    assert!(net.server.world_cameras().is_empty());
}

// ------------------------------------------------------------------------------- DSV-125 .. DSV-127 upper body

/// Weapons of the upper-body tests (made-up names): `AniType` 0 (a rifle family), 10 (pistol), 14 (knife), 11
/// (throwable).
const ANIM_RIFLE: u16 = 3;
const ANIM_PISTOL: u16 = 4;
const ANIM_KNIFE: u16 = 5;
const ANIM_THROW: u16 = 6;

fn anim_tables() -> RomTables {
    let mut t = character_tables();
    let doc = dc_bute::ButeDoc::parse(concat!(
        "[Weapon3]\nName = \"R1\"\nAniType = 0\nHHModel = \"HH\\\\r1.ltb\"\n",
        "[Weapon4]\nName = \"P1\"\nAniType = 10\nHHModel = \"HH\\\\p1.ltb\"\n",
        "[Weapon5]\nName = \"K1\"\nAniType = 14\nHHModel = \"HH\\\\k1.ltb\"\n",
        "[Weapon6]\nName = \"T1\"\nAniType = 11\nHHModel = \"HH\\\\t1.ltb\"\n",
        "[WeaponOrder]\nClass0 = \"A\"\nA0 = \"R1\"\nA1 = \"P1\"\nA2 = \"K1\"\nA3 = \"T1\"\n",
    ));
    t.weapons = dc_dserver::rom_tables::WeaponTable::from_doc(&doc);
    t
}

/// [`rig`] plus classified upper-body clips of four families (indices from 13 on) and the hit clip (35). The
/// play-once clips are longer than in the ROM so that a stream tick always catches them.
fn anim_rig() -> dc_dserver::chars::Rig {
    use dc_dserver::anims::{Heading as H, Slot, Stance as S, Variant as V, CLIP_HIT, FAMILIES};
    let fam = |ani: u8| FAMILIES.iter().find(|f| f.0 == ani).unwrap().1;
    let mut r = rig();
    let mut add = |ani: u8, slot: Slot, len_ms: u32| {
        r.clips.push(dc_dserver::chars::ClipFact {
            key: ([0xee; 8], 9),
            len_ms,
            upper: Some((fam(ani), slot)),
            ..Default::default()
        })
    };
    // rifle 13..20
    add(0, Slot::Aim(S::Run, H::Still), 4000);
    add(0, Slot::Aim(S::Run, H::Forward), 600);
    add(0, Slot::Aim(S::Walk, H::Forward), 1000);
    add(0, Slot::Fire(S::Run, H::Still), 400);
    add(0, Slot::Select(V::Still), 733);
    add(0, Slot::Select(V::Moving), 733);
    add(0, Slot::Reload(V::Still), 1866);
    add(0, Slot::Reload(V::Moving), 1866);
    // pistol 21..25
    add(10, Slot::Aim(S::Run, H::Still), 4000);
    add(10, Slot::Aim(S::Run, H::Forward), 600);
    add(10, Slot::Fire(S::Run, H::Forward), 400);
    add(10, Slot::Fire(S::Run, H::Still), 400);
    add(10, Slot::Select(V::Moving), 550);
    // knife 26..29
    add(14, Slot::Aim(S::Run, H::Still), 4000);
    add(14, Slot::Aim(S::Run, H::Forward), 600);
    add(14, Slot::Fire(S::Run, H::Still), 500);
    add(14, Slot::Fire(S::Run, H::Forward), 500);
    // throwable 30..34
    add(11, Slot::Aim(S::Run, H::Still), 4000);
    add(11, Slot::Aim(S::Run, H::Forward), 600);
    add(11, Slot::ThrowHold(S::Run, H::Still), 267);
    add(11, Slot::Throw(S::Run, H::Still), 900);
    add(11, Slot::Select(V::Still), 533);
    r.clips.push(dc_dserver::chars::ClipFact {
        key: CLIP_HIT,
        len_ms: 467,
        ..Default::default()
    });
    r
}

/// Dave (client 0, `control`) and Erin (client 1) alive on the upper-body rig; Erin watches Dave.
fn anim_net(control: u32, tune: impl FnOnce(&mut GameTuning)) -> Net {
    let mut game = GameTuning {
        default_loadout: vec![ANIM_RIFLE, ANIM_PISTOL],
        // shots right after a spawn: the shield (DSV-156) is not under test here
        spawn_shield: false,
        ..GameTuning::default()
    };
    tune(&mut game);
    let cfg = ServerTuning {
        game,
        ..ServerTuning::default()
    };
    let mut net = Net::with(
        cfg,
        anim_tables(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
    );
    net.server.insert_rig("BODY\\b.ltb", anim_rig());
    net.add_with(1, "Dave", 0.0, Some(control));
    net.add_with(2, "Big Erin", 0.0, Some(0));
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(1000);
    net
}

impl Net {
    /// Dave's tracker 1 as Erin saw it last: (clip, looping, time).
    fn dave_upper(&self) -> (u16, bool, u32) {
        let u = self.c(1).seen.remote[&self.obj(0)]
            .last_upper
            .expect("an upper tracker");
        (u.0, u.2, u.3)
    }

    /// `UP_WEAPON_CHANGE` `[slot:u8][weapon_id:u16][aux:u16]` from Dave.
    fn dave_draws(&mut self, weapon: u16) {
        let mut b = BitWriter::new();
        b.write_u8(0);
        b.write_u16(weapon);
        b.write_u16(weapon);
        self.cm(0).send_raw_shell(0x7f, &b);
    }
}

/// DSV-125: tracker 1 follows the drawn weapon's family, the stance and the direction: a rifle and a pistol run
/// forward on their run-aim clips, a knife stands on its still aim clip, a throwable runs on its own; walking takes
/// the walk column, crouching without a crouch clip falls back to the run column; each change plays the family's
/// select clip once first (still / moving variant).
#[test]
fn upper_body_follows_the_weapon_family_stance_and_direction() {
    // (control, weapon, select clip, aim clip)
    let cases: [(u32, u16, u16, u16); 6] = [
        (0x0a01, ANIM_RIFLE, 18, 14),
        (0x0a01, ANIM_PISTOL, 25, 22),
        (0x0000, ANIM_KNIFE, 0, 26),
        (0x0a01, ANIM_THROW, 34, 31),
        (0x0801, ANIM_RIFLE, 18, 15),
        (0x0821, ANIM_RIFLE, 18, 14),
    ];
    for (control, weapon, select, aim) in cases {
        let mut net = anim_net(control, |_| {});
        let label = format!("control {control:#x} weapon {weapon}");
        net.dave_draws(weapon);
        net.run_for(150);
        if select != 0 {
            let (clip, looping, _) = net.dave_upper();
            assert_eq!((clip, looping), (select, false), "{label}: select once");
        }
        net.run_for(1200);
        let (clip, looping, _) = net.dave_upper();
        assert_eq!((clip, looping), (aim, true), "{label}: aim loop");
        assert!(
            net.c(1).seen.errors.is_empty(),
            "{:?}",
            net.c(1).seen.errors
        );
    }
    // `-directionalanims off`: the plain walk pair as before the table
    let mut net = anim_net(0x0a01, |g| g.directional_anims = false);
    net.dave_draws(ANIM_PISTOL);
    net.run_for(1200);
    assert_eq!(net.dave_upper().0, 4, "the walk pair's upper clip");
}

/// DSV-125 / DSV-126: fire, reload and the throw are play-once clips (`state1` 0, time since the action, held at
/// the clip's end) chosen at the start, then the aim loop again: the rifle's still fire clip for a shot, its still
/// reload; the pistol fires its moving clip when running; the knife fires its still clip in every direction; the
/// throwable plays the hold phase on `UP_THROW_HOLD` and the release phase on the throw.
#[test]
fn fire_reload_and_throw_play_once() {
    // rifle, standing: fire then reload
    let mut net = anim_net(0x0000, |_| {});
    net.dave_draws(ANIM_RIFLE);
    net.run_for(1000);
    let erin = net.obj(1);
    assert!(net.cm(0).fire_at(erin, [0.0, 500.0, 0.0], false));
    net.run_for(120);
    let (clip, looping, t) = net.dave_upper();
    assert_eq!((clip, looping), (16, false), "the fire one-shot");
    assert!(t < 400, "time since the shot {t}");
    net.run_for(600);
    let (clip, looping, _) = net.dave_upper();
    assert_eq!((clip, looping), (13, true), "back on the aim loop");
    net.cm(0).reload();
    net.run_for(1000);
    let (clip, looping, t) = net.dave_upper();
    assert_eq!((clip, looping), (19, false), "the still reload");
    assert!((800..=1866).contains(&t), "reload time {t}");
    net.run_for(1500);
    assert_eq!(net.dave_upper().0, 13, "reload over");
    // pistol, running: the moving fire clip
    let mut net = anim_net(0x0a01, |_| {});
    net.dave_draws(ANIM_PISTOL);
    net.run_for(1000);
    let erin = net.obj(1);
    assert!(net.cm(0).fire_at(erin, [0.0, 500.0, 0.0], false));
    net.run_for(120);
    assert_eq!(net.dave_upper().0, 23, "pistol moving fire");
    // knife, running: one fire clip for every direction
    net.dave_draws(ANIM_KNIFE);
    net.run_for(1000);
    assert_eq!(net.dave_upper().0, 27, "knife run aim");
    assert!(net.cm(0).fire_at(erin, [0.0, 500.0, 0.0], false));
    net.run_for(120);
    let (clip, looping, _) = net.dave_upper();
    assert_eq!((clip, looping), (28, false), "knife still fire");
    // throwable, standing: hold phase, then the release phase
    let mut net = anim_net(0x0000, |_| {});
    net.dave_draws(ANIM_THROW);
    net.run_for(1000);
    net.cm(0).send_raw_shell(0x4d, &BitWriter::new());
    net.run_for(150);
    assert_eq!(net.dave_upper().0, 32, "throw hold");
    net.cm(0).send_raw_shell(0x4d, &BitWriter::new());
    net.run_for(400);
    let (clip, looping, t) = net.dave_upper();
    assert_eq!((clip, looping), (32, false), "still holding");
    // the wire's time field has 8 ms steps
    assert_eq!(t, 272, "the hold phase is held at its end (267 ms)");
    net.cm(0).throw((ANIM_THROW, 0), [0.0, 0.0, 1.0]);
    net.run_for(150);
    let (clip, looping, _) = net.dave_upper();
    assert_eq!((clip, looping), (33, false), "the throw");
}

/// DSV-127 (project decision: on): by default a hit that takes health plays the ROM body's hit clip once on
/// the victim's tracker 1 (state bit 1 clear, then back to the aim loop); `-combat hit_clip=off` gives the reference stack's
/// stream (no hit clip); a hit does not cut a select or reload short.
#[test]
fn hit_clip_is_on_by_default_and_a_switch() {
    // (tuning, the hit clip expected)
    for (set, on) in [(None, true), (Some("off"), false), (Some("on"), true)] {
        let mut net = anim_net(0x0000, |g| {
            if let Some(v) = set {
                g.set_combat("hit_clip", v).unwrap()
            }
        });
        net.dave_draws(ANIM_RIFLE);
        net.run_for(1000);
        let mut seen_hit = None;
        let dave = net.obj(0);
        assert!(net.cm(1).fire_at(dave, [0.0; 3], true));
        for _ in 0..10 {
            net.run_for(30);
            let u = net.dave_upper();
            if u.0 == 35 {
                seen_hit = Some(u);
            }
        }
        assert_eq!(seen_hit.is_some(), on, "hit_clip {set:?}");
        if let Some((_, looping, t)) = seen_hit {
            assert!(!looping, "the hit clip plays once");
            assert!(t <= 467 + 8, "held at most at its end: {t}");
            // after the clip the still aim loop returns
            net.run_for(600);
            assert_eq!(net.dave_upper().0, 13, "back on the aim loop");
        }
    }
    let mut g = GameTuning::default();
    assert!(g.hit_clip, "on by default");
    g.set_combat("hit_clip", "off").unwrap();
    assert!(!g.hit_clip);
    assert!(g.set_combat("hit_clip", "maybe").is_err());

    // a hit during the select one-shot leaves the select playing
    let mut net = anim_net(0x0000, |_| {});
    net.dave_draws(ANIM_RIFLE);
    net.run_for(60);
    let dave = net.obj(0);
    assert!(net.cm(1).fire_at(dave, [0.0; 3], true));
    for _ in 0..8 {
        net.run_for(30);
        assert_ne!(net.dave_upper().0, 35, "no hit clip over the select");
    }
}

/// A box of half-size `h` around `c` as one collision model (six faces, outward normals).
fn box_shape(c: [f32; 3], h: f32) -> dc_sim::CollisionWorld {
    use dc_sim::{ConvexPoly, V3};
    let v = |x: f32, y: f32, z: f32| V3::new(c[0] + x * h, c[1] + y * h, c[2] + z * h);
    let faces = [
        [
            v(1., -1., -1.),
            v(1., 1., -1.),
            v(1., 1., 1.),
            v(1., -1., 1.),
        ],
        [
            v(-1., -1., 1.),
            v(-1., 1., 1.),
            v(-1., 1., -1.),
            v(-1., -1., -1.),
        ],
        [
            v(-1., 1., -1.),
            v(-1., 1., 1.),
            v(1., 1., 1.),
            v(1., 1., -1.),
        ],
        [
            v(-1., -1., 1.),
            v(-1., -1., -1.),
            v(1., -1., -1.),
            v(1., -1., 1.),
        ],
        [
            v(-1., -1., 1.),
            v(1., -1., 1.),
            v(1., 1., 1.),
            v(-1., 1., 1.),
        ],
        [
            v(1., -1., -1.),
            v(-1., -1., -1.),
            v(-1., 1., -1.),
            v(1., 1., -1.),
        ],
    ];
    let polys: Vec<ConvexPoly> = faces
        .iter()
        .map(|f| ConvexPoly::from_corners(f.to_vec(), dc_sim::SURF_SOLID))
        .collect();
    dc_sim::CollisionWorld::from_parts(vec![dc_sim::CollModel::from_polys(&polys)], &[])
}

/// DSV-122 (client-brushhits): the 2009 client sends a pellet into a crate or a drum without a hit object (623 bits;
/// cshell+0x238eb0 names objects only for polygon-less hits on characters), so the server traces the pellet's ray
/// against the living breakables itself: three such pellets through the barrel break it, a pellet beside it does
/// nothing.
#[test]
fn pellets_without_a_hit_object_break_the_barrel_their_ray_meets() {
    let mut w = world_with_barrel();
    let at = w.brushes[0].pos;
    w.destructibles.shapes.push((
        b"barrel1".to_vec(),
        std::sync::Arc::new(box_shape(at, 15.0)),
    ));
    let mut net = Net::with_world(
        ServerTuning::default(),
        w,
        tables(),
        "room key=TDM type=1 goal=10 ff=0",
    );
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    let shooter = (0..2)
        .find(|&i| (net.c(i).position()[0] - RED[0]).abs() < 50.0)
        .expect("a player at the red spawn");
    let barrel = net.server.game().breakables()[0].clone();
    // beside the barrel: nothing
    net.cm(shooter).fire_at_point(
        0,
        [barrel.pos[0], barrel.pos[1] + 200.0, barrel.pos[2]],
        false,
    );
    net.run_for(60);
    assert_eq!(net.server.game().breakables()[0].left, 60.0);
    // through it, reported as a plain miss
    net.cm(shooter).fire_at_point(0, barrel.pos, false);
    net.run_for(60);
    assert_eq!(net.server.game().breakables()[0].left, 35.0, "one pellet");
    for _ in 0..2 {
        net.cm(shooter).fire_at_point(0, barrel.pos, false);
        net.run_for(60);
    }
    net.run_for(200);
    assert!(net.server.game().breakables()[0].dead);
    for i in 0..2 {
        assert_eq!(net.c(i).seen.objects[&barrel.obj].flags, Some(0x0100));
    }
    assert!(net.logs.iter().any(|l| l.contains("destroyed by client")));
}

/// DSV-153: a player's view pitch (`aux[0]` of its updates) reaches every client as type 0x18 sub 0x80 notes on
/// its character, only when the words change, and the latest note follows the pitch.
// OBSERVED: capture 20261025_001409_tdm2 (the reference server's sub 0x80 notes); DISASM:
// cshell+0x26f60
#[test]
fn view_pitch_is_relayed_as_aim_notes() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![3, 4],
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(
        cfg,
        character_tables(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
    );
    net.server.insert_rig("BODY\\b.ltb", rig());
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    net.run_for(500);
    let obj0 = net.obj(0);
    let notes = |n: &Net, i: usize| -> Vec<f32> {
        n.c(i)
            .seen
            .aim_notes
            .iter()
            .filter(|x| x.1 == obj0)
            .map(|x| f32::from_bits(x.2[0]))
            .collect()
    };
    let before = notes(&net, 1).len();
    net.cm(0).set_pitch(0.4);
    net.run_for(1000);
    let after = notes(&net, 1);
    assert!(after.len() > before, "client 1 got aim notes for client 0");
    assert_eq!(after.last(), Some(&0.4));
    // unchanged words: no further notes
    let n = after.len();
    net.run_for(1000);
    assert_eq!(notes(&net, 1).len(), n, "notes only on a change");
    net.cm(0).set_pitch(-0.3);
    net.run_for(500);
    assert_eq!(notes(&net, 1).last(), Some(&-0.3));
    // the owner gets them too, as the reference server sent them
    assert_eq!(notes(&net, 0).last(), Some(&-0.3));
    for i in 0..2 {
        assert!(
            net.c(i).seen.errors.is_empty(),
            "{:?}",
            net.c(i).seen.errors
        );
    }
}

/// DSV-156: a character inside its spawn window (spawn note 0x1f .. 0x25) takes no hit; after the end note it does.
// EVIDENCE: cshell+0x238eb0 (the 2009 shooter claims no hit on a character with its spawn flag set)
#[test]
fn spawn_shield_refuses_hits_until_the_end_note() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![3, 4],
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(
        cfg,
        character_tables(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
    );
    net.server.insert_rig("BODY\\b.ltb", rig());
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    let erin = net.obj(1);
    let opened = net
        .c(0)
        .seen
        .spawn_notes
        .iter()
        .any(|n| n.1 == erin && n.2 == 0x1f);
    assert!(opened, "the start note went out");
    net.run_for(300);
    net.shoot(0, 1, 2, [0.0; 3]);
    assert!(
        net.c(1).seen.damage.is_empty(),
        "no damage inside the window"
    );
    assert!(net.run_until(net.now + 5_000, |n| n
        .c(0)
        .seen
        .spawn_notes
        .iter()
        .any(|x| x.1 == erin && x.2 == 0x25)));
    net.run_for(100);
    net.shoot(0, 1, 1, [0.0; 3]);
    assert_eq!(net.c(1).seen.damage.len(), 1, "hit after the end note");
}

/// DSV-477: a player's first shot inside its own spawn window sends that window's end note at once and drops its
/// shield; a player who did not fire stays shielded until his own end note.
// DC-DEFAULT: DSV-477 (proposed; project decision)
#[test]
fn first_shot_ends_the_spawn_shield() {
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![3, 4],
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut net = Net::with(
        cfg,
        character_tables(),
        "room key=TDM type=1 goal=20 ff=0 time=0",
    );
    net.server.insert_rig("BODY\\b.ltb", rig());
    net.add(1, "Dave", 0.0);
    net.add(2, "Big Erin", 0.0);
    assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
        && n.c(1).life_state() == 1));
    let (dave, erin) = (net.obj(0), net.obj(1));
    let ended = |n: &Net, viewer: usize, o: u16| {
        n.c(viewer)
            .seen
            .spawn_notes
            .iter()
            .filter(|x| x.1 == o && x.2 == 0x25)
            .map(|x| x.0)
            .next()
    };
    net.run_for(300);
    assert!(ended(&net, 0, erin).is_none());
    let t_shot = net.now;
    net.shoot(1, 0, 1, [0.0; 3]);
    net.run_for(100);
    let t_end = ended(&net, 0, erin).expect("Erin's end note after her first shot");
    assert!(
        u64::from(t_end) < u64::from(t_shot) + 500 && SPAWN_NOTE_MS > 700,
        "end note at {t_end}, shot at {t_shot}"
    );
    assert!(ended(&net, 0, dave).is_none(), "Dave has not fired");
    assert!(
        net.c(0).seen.damage.is_empty(),
        "Dave, who did not fire, is still shielded"
    );
    net.shoot(0, 1, 1, [0.0; 3]);
    assert_eq!(net.c(1).seen.damage.len(), 1, "Dave hits Erin back");
}

/// Knife tables in the ROM key shape (made-up ids; the values of the user's knife row: `Range` 105, `InstDamage` 45,
/// `InstDamageType` 13).
fn knife_tables() -> RomTables {
    let doc = dc_bute::ButeDoc::parse(concat!(
        "[Weapon50]\nName = \"Blade\"\nRange = 105\nAmmoName0 = \"Blade\"\n",
        "[Ammo51]\nName = \"Blade\"\nInstDamage = 45\nInstDamageType = 13\nMaxAmount = 1000\n",
    ));
    RomTables {
        weapons: dc_dserver::rom_tables::WeaponTable::from_doc(&doc),
        ..tables()
    }
}

/// DSV-161: a melee swing reaches the knife row's `Range` (105) from the eye. Two enemies 80 units apart: each swing
/// does the row's 45 and the third kills (0x6d with the knife's weapon id); the same swings with the players 600
/// units apart are refused (`out_of_range`) and do nothing. Playtest report: "killing an enemy with the knife".
#[test]
fn knife_swings_kill_in_range_and_do_nothing_beyond_it() {
    for (gap, in_range) in [(80.0f32, true), (600.0, false)] {
        let mut w = world();
        w.spawns[0].pos = [-gap / 2.0, 0.0, 0.0];
        w.spawns[1].pos = [gap / 2.0, 0.0, 0.0];
        let mut cfg = ServerTuning::default();
        cfg.game.default_loadout = vec![50];
        cfg.game.spawn_shield = false;
        let mut net = Net::with_world(cfg, w, knife_tables(), "room key=TDM type=1 goal=10 ff=0");
        net.add(1, "Dave", 0.0);
        net.add(2, "Big Erin", 0.0);
        assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
            && n.c(1).life_state() == 1));
        net.run_for(500);
        net.shoot(0, 1, 3, [0.0; 3]);
        let dmg: Vec<u32> = net.c(1).seen.damage.iter().map(|d| d.1).collect();
        let refused = net.server.game().hit_counters().get(HitVerdict::OutOfRange);
        if in_range {
            assert_eq!(dmg, vec![45, 45, 45], "gap {gap}");
            let k = net.c(0).seen.kills.first().expect("a knife kill");
            assert_eq!((k.victim, k.killer, k.weapon), (net.pid(1), net.pid(0), 50));
            assert_eq!(refused, 0);
        } else {
            assert!(dmg.is_empty(), "gap {gap}: {dmg:?}");
            assert!(net.c(0).seen.kills.is_empty());
            assert_eq!(refused, 3, "gap {gap}");
            assert!(net
                .logs
                .iter()
                .any(|l| l.contains("melee hit") && l.contains("row range 105")));
        }
    }
}
