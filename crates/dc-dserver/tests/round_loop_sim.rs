//! The round loop of the round modes (GMD-20..GMD-24), deterministic: the server and two synthetic clients over the
//! simulated channel of `dc-net-engine` with an injected clock, on the synthetic world with made-up spawn points, team
//! names and one made-up crate (no ROM content). An elimination match (class 12) ends a round when one team has no
//! living player, starts the next round with everyone alive and the crate restored, and ends the match when a side
//! reaches the goal in round wins.
// SPEC: GAME_MODES_2009.md §4.1 (round modes row), §4.2 (no respawn request in 2, 3, 11, 12, 13; cshell+0x61e40),
// §5.8; GAME_MESSAGES_2009_S2C.md §2.1 (0x44, 0x46, 0x47, 0x48, 0x3f layouts)
// DC-DEFAULT: GMD-5, GMD-9, GMD-13 (proposed); GMD-20..GMD-24 (staged)

use dc_dserver::game::{GameTuning, RoundPhase};
use dc_dserver::rom_tables::{RomTables, TeamNames};
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{synthetic_world_bytes, world_from_bytes, SpawnPoint, WorldInfo};
use dc_net_engine::sim::{Faults, SimChannel};

const ANNOUNCE: &str = "Worlds\\Synth.dat";
const RED: [f32; 3] = [-300.0, 0.0, -100.0];
const BLUE: [f32; 3] = [300.0, 0.0, 100.0];

/// The synthetic world with a red and a blue spawn point and one made-up breakable crate (60 hit points) whose death
/// switches on a made-up effect.
fn world_with_crate() -> WorldInfo {
    use dc_dserver::destructibles::{BreakableInfo, DeathAction};
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
    let at = [0.0, 20.0, 200.0];
    w.brushes.push(dc_dserver::world::BrushInfo {
        name: b"Box1".to_vec(),
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
        name: b"Box1Fx".to_vec(),
        pos: at,
        rot: [0.0; 3],
        fx: b"TestBurst".to_vec(),
        looped: false,
        start_on: false,
    });
    w.destructibles.breakables.push(BreakableInfo {
        name: b"box1".to_vec(),
        hit_points: 60.0,
        death: vec![DeathAction::Effect(b"box1fx".to_vec())],
        skipped: 0,
    });
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
    pipe: Vec<String>,
    logs: Vec<String>,
    now: u32,
}

impl Net {
    /// Reference start order (the default), the room line given; Dave (team 1) and Big Erin (team 2) alive.
    fn two_alive(cfg: ServerTuning, room: &str, mode_class: u8) -> Net {
        Net::two_alive_in(cfg, world_with_crate(), room, mode_class)
    }

    fn two_alive_in(cfg: ServerTuning, world: WorldInfo, room: &str, mode_class: u8) -> Net {
        let mut server = Server::with_tables(cfg, world, tables(), 0);
        assert!(server.handle_line(0, room));
        assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
        assert!(server.handle_line(0, "user 8 4 55 1 0 Big_Erin"));
        let mut net = Net {
            server,
            peers: Vec::new(),
            pipe: Vec::new(),
            logs: Vec::new(),
            now: 0,
        };
        for (addr, name) in [(1u32, "Dave"), (2, "Big Erin")] {
            let mut c = SynthConfig::named(name);
            c.box_bounds = Some((net.server.world().extent_min, net.server.world().extent_max));
            c.walk_step = 0.0;
            c.mode_class_hint = Some(mode_class);
            net.peers.push(Peer {
                addr,
                client: SynthClient::new(c),
                up: SimChannel::new(addr as u64 * 2 + 1, Faults::clean(2)),
                down: SimChannel::new(addr as u64 * 2 + 2, Faults::clean(2)),
            });
        }
        assert!(net.run_until(20_000, |n| n.c(0).life_state() == 1
            && n.c(1).life_state() == 1));
        net.run_for(500);
        net
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
                Output::Pipe(l) => self.pipe.push(l),
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

    fn pid(&self, i: usize) -> u8 {
        self.c(i).seen.client_id.expect("client id").0 as u8
    }

    /// One lethal pellet (default damage 100) from client `from` at client `to`; returns the kill time.
    fn kill(&mut self, from: usize, to: usize) -> u32 {
        let target = self.c(to).seen.view_object.expect("view object");
        let before = self.c(from).seen.kills.len();
        assert!(
            self.cm(from).fire_at(target, [0.0; 3], true),
            "target not in the stream"
        );
        let limit = self.now + 1_000;
        assert!(
            self.run_until(limit, |n| n.c(from).seen.kills.len() > before),
            "no kill"
        );
        self.c(from).seen.kills.last().unwrap().at
    }

    /// Shell ids client `i` received from `t` on, the unreliable notes (0xa6) and the ping tables (0xa2) left out.
    fn shells_since(&self, i: usize, t: u32) -> Vec<u8> {
        self.c(i)
            .seen
            .shell_ids
            .iter()
            .filter(|x| x.0 >= t && x.1 != 0xa6 && x.1 != 0xa2)
            .map(|x| x.1)
            .collect()
    }
}

fn fast(goal_room: &str) -> (ServerTuning, String) {
    let cfg = ServerTuning {
        game: GameTuning {
            default_damage: 100.0,
            end_pause_ms: (1_000, 800),
            timeout_hold_ms: 1_000,
            between_round_ms: 2_000,
            // the round tests start at round 1: no S&D warm-up (DSV-512)
            warmup_secs: Some(0),
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    (cfg, goal_room.to_string())
}

/// GMD-20..GMD-24 on an elimination match with goal 2: the kill that leaves team 2 without a living player ends round
/// 1 for team 1 (0x46 [1][1][0 0], 0xb5 [1][0]); the dead player stays dead (the class-12 client never asks); no
/// summary and no result screen between rounds: after the first pause 0x47, the roster refresh and 0x48 with the
/// between-round seconds; after the between-round pause every player respawns (the frozen survivor released first),
/// gets 0x44 and 0xb5, and the broken crate is back on every client with full hit points and its effect gone. The
/// second elimination decides the match (2 round wins = the goal): the match-end set with the summaries and the
/// result screen for team 1, one `@result` with the round wins.
#[test]
fn elimination_rounds_restart_restore_and_end_at_the_goal() {
    let (cfg, room) = fast("room key=ELIM type=12 goal=2 ff=0");
    let mut net = Net::two_alive(cfg, &room, 12);
    let (p0, p1) = (net.pid(0), net.pid(1));
    let g = net.server.game();
    assert_eq!(g.round_no(), 1);
    assert!(g.round_loop());
    assert_eq!(g.players()[&p0].team, 1);
    assert_eq!(g.players()[&p1].team, 2);
    // GMD-23: 0x7d carries the goal as the round limit
    for i in 0..2 {
        assert_eq!(net.c(i).seen.settings.map(|s| (s.1, s.2)), Some((2, 2)));
    }

    // the crate breaks (one pellet of 100 against its 60 hit points)
    let crate0 = net.server.game().breakables()[0].clone();
    net.cm(0).fire_at_point(crate0.obj, crate0.pos, true);
    net.run_for(200);
    assert!(net.server.game().breakables()[0].dead);
    for i in 0..2 {
        assert_eq!(net.c(i).seen.objects[&crate0.obj].flags, Some(0x0100));
        assert_eq!(
            net.c(i)
                .seen
                .objects
                .values()
                .filter(|o| o.blob_type == Some(0x64))
                .count(),
            1,
            "client {i}: the crate's death effect"
        );
    }

    // round 1: Dave eliminates team 2
    let freezes0 = net.c(0).seen.freezes.len();
    let t1 = net.kill(0, 1);
    net.run_for(100);
    for i in 0..2 {
        // DSV-400: 0x46 carries the round wins after the round
        assert_eq!(net.c(i).seen.round_result, Some((1, 1, 0)), "client {i}");
        assert_eq!(
            net.c(i).seen.team_kills.last().map(|t| (t.1, t.2)),
            Some((1, 0))
        );
    }
    assert!(matches!(
        net.server.game().phase(),
        RoundPhase::Over {
            win: 1,
            last: false,
            ..
        }
    ));
    assert_eq!(net.server.game().round_wins(), [1, 0]);
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("team(s) 2 without a living player: win 1")));
    // between the rounds: Erin stays dead, nobody gets a summary or the result screen
    let limit = t1 + 2_900;
    let early = net.run_until(limit, |n| n.c(1).life_state() == 1);
    assert!(!early, "no respawn before the next round");
    for i in 0..2 {
        let o = net.shells_since(i, t1);
        assert!(
            !o.contains(&0x65) && !o.contains(&0x3f) && !o.contains(&0x44),
            "client {i}: {o:x?}"
        );
        let p = o.iter().position(|&x| x == 0x46).unwrap();
        // GMD-21: 0x46, 0xb5 at once (DSV-77 freeze for the living Dave), after the first pause 0x47, the refresh, 0x48
        let freeze: &[u8] = if i == 0 { &[0xa0] } else { &[] };
        let want: Vec<u8> = [&[0x46, 0xb5][..], freeze, &[0x47, 0x6b, 0x6b, 0x48]].concat();
        assert_eq!(&o[p..], &want[..], "client {i}: {o:x?}");
        assert_eq!(
            net.c(i).seen.round_close.map(|r| r.1),
            Some(2),
            "0x48 = the between-round seconds"
        );
        let at47 = net
            .c(i)
            .seen
            .shell_ids
            .iter()
            .find(|x| x.0 >= t1 && x.1 == 0x47)
            .unwrap()
            .0;
        assert!((1_000..=1_060).contains(&(at47 - t1)), "{}", at47 - t1);
    }
    assert!(
        net.pipe.iter().all(|l| !l.starts_with("@result")),
        "no lobby report between rounds"
    );

    // round 2 starts 1000 + 2000 ms after the result: everyone alive again, the crate restored
    assert!(net.run_until(t1 + 3_300, |n| n.c(1).life_state() == 1));
    let back = net.c(1).seen.life_states.last().unwrap().0;
    assert!((3_000..=3_060).contains(&(back - t1)), "{}", back - t1);
    net.run_for(200);
    let g = net.server.game();
    assert_eq!(g.round_no(), 2);
    assert!(matches!(g.phase(), RoundPhase::Running { .. }));
    assert!(!g.breakables()[0].dead);
    assert_eq!(g.breakables()[0].left, 60.0);
    // the session switches the crate's brush on again on the clients (integrator hook GMD-24 in session.rs, see
    // `the_crate_is_back_on_every_client_at_the_next_round`); without the hook the game keeps the id for it
    let handed = g.restored_breakables() == [crate0.obj];
    for i in 0..2 {
        let flags = net.c(i).seen.objects[&crate0.obj].flags;
        assert!(handed || flags == Some(0x3101), "client {i}: {flags:x?}");
    }
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert_eq!(net.c(i).life_state(), 1, "client {i}");
        assert_eq!(s.health, Some(100.0), "client {i}");
        assert!(
            !s.objects.values().any(|o| o.blob_type == Some(0x64)),
            "client {i}: the death effect is gone"
        );
        let o = net.shells_since(i, t1 + 2_990);
        let b44 = o.iter().position(|&x| x == 0x44).expect("0x44");
        assert_eq!(o.get(b44 + 1), Some(&0xb5), "client {i}: {o:x?}");
        assert!(
            o[b44..].contains(&0x45),
            "DSV-243: 0x45 again each round: {o:x?}"
        );
        assert_eq!(s.team_kills.last().map(|t| (t.1, t.2)), Some((1, 0)));
    }
    // DSV-77: the survivor was frozen at the result and released by his respawn
    assert_eq!(&net.c(0).seen.freezes[freezes0..], &[true, false]);
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("1 breakable(s) restored")));

    // round 2: Dave eliminates team 2 again: 2 round wins = the goal, the match ends
    let t2 = net.kill(0, 1);
    net.run_for(2_000);
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert_eq!(
            s.round_result,
            Some((1, 2, 0)),
            "client {i}: wins after this round (DSV-400)"
        );
        let o = net.shells_since(i, t2);
        let p = o.iter().position(|&x| x == 0x46).unwrap();
        let freeze: &[u8] = if i == 0 { &[0xa0] } else { &[] };
        let want: Vec<u8> = [
            &[0x46, 0xb5][..],
            freeze,
            &[0x47, 0x6b, 0x6b, 0x65, 0x65, 0x48, 0x3f],
        ]
        .concat();
        assert_eq!(&o[p..], &want[..], "client {i}: {o:x?}");
        assert_eq!(s.result_value, Some((false, 1)), "the match winner");
        assert_eq!(s.team_kills.last().map(|t| (t.1, t.2)), Some((2, 0)));
    }
    assert!(matches!(
        net.server.game().phase(),
        RoundPhase::Over {
            last: true,
            stage: 2,
            ..
        }
    ));
    let results: Vec<&String> = net
        .pipe
        .iter()
        .filter(|l| l.starts_with("@result"))
        .collect();
    assert_eq!(
        results,
        vec!["@result mode=ELIM win=1 alpha=2 bravo=0 limit=2 rounds=2:0"]
    );
    // no third round
    net.run_for(4_000);
    assert_eq!(net.server.game().round_no(), 2);
    assert_ne!(net.c(1).life_state(), 1);
}

/// GMD-22: in a round mode the goal counts rounds, not kills (the old form ended the match at `goal` team kills);
/// the clock end decides by living players in 12 (equal = draw, the next round follows) and for the defenders in
/// S&D (class 2); a respawn request inside a round is not answered.
#[test]
fn round_modes_decide_by_rounds_living_players_and_defenders() {
    // class 12, goal 1, 3 s rounds: both alive at the clock end -> draw, next round; the round cap (2 x goal) ends
    // the match as a draw after round 2
    let (mut cfg, _) = fast("");
    cfg.game.round_secs = Some(3);
    let mut net = Net::two_alive(cfg, "room key=ELIM type=12 goal=1 ff=0", 12);
    let t0 = net.c(0).seen.life_states.last().unwrap().0;
    assert!(net.run_until(t0 + 4_000, |n| n.c(0).seen.round_result.is_some()));
    assert_eq!(
        net.c(0).seen.round_result,
        Some((0, 0, 0)),
        "draw by living players"
    );
    assert!(matches!(
        net.server.game().phase(),
        RoundPhase::Over { last: false, .. }
    ));
    assert!(net.run_until(net.now + 3_500, |n| n.server.game().round_no() == 2));
    assert!(net.run_until(net.now + 4_000, |n| matches!(
        n.server.game().phase(),
        RoundPhase::Over { last: true, .. }
    )));
    assert_eq!(net.server.game().round_wins(), [0, 0]);
    net.run_for(2_000);
    assert_eq!(
        net.c(0).seen.result_value,
        Some((false, 0)),
        "a drawn match"
    );

    // class 2 (S&D), goal 3: a kill does not end the match on the kill goal; the clock gives the round to team 2
    let (mut cfg, _) = fast("");
    cfg.game.round_secs = Some(3);
    let mut net = Net::two_alive(cfg, "room key=SND type=2 goal=3 ff=0", 2);
    // Erin (team 2) shoots Dave: team 1 is out, team 2 wins round 1
    let t1 = net.kill(1, 0);
    net.run_for(100);
    assert_eq!(net.c(0).seen.round_result, Some((2, 0, 1)));
    // a respawn request of the dead player inside the between-round time is ignored
    net.cm(0).request_respawn();
    net.run_for(500);
    assert_ne!(net.c(0).life_state(), 1);
    assert!(net.run_until(t1 + 3_300, |n| n.c(0).life_state() == 1));
    // round 2: nobody dies, the clock gives it to the defenders
    let t2 = net.now;
    assert!(net.run_until(t2 + 3_500, |n| n
        .c(0)
        .seen
        .round_result
        .is_some_and(|r| r.2 == 2)));
    assert_eq!(net.c(0).seen.round_result, Some((2, 0, 2)));
    assert_eq!(net.server.game().round_wins(), [0, 2]);
    assert!(matches!(
        net.server.game().phase(),
        RoundPhase::Over { last: false, .. }
    ));
}

/// GMD-22: in a round mode the team that empties forfeits the match (DSV-31 window), whatever the round wins.
#[test]
fn an_emptied_team_forfeits_a_round_mode_match() {
    let (mut cfg, _) = fast("");
    cfg.game.team_empty_ms = 1_000;
    let mut net = Net::two_alive(cfg, "room key=ELIM type=12 goal=5 ff=0", 12);
    let t1 = net.kill(1, 0);
    // round 1 to team 2; round 2 starts, then Erin (team 2) leaves: her team is out at once (the leave empties it:
    // round 2 to team 1), and in round 3 the empty team forfeits the match to team 1
    assert!(net.run_until(t1 + 3_300, |n| n.server.game().round_no() == 2));
    net.run_for(200);
    net.cm(1).quit();
    assert!(net.run_until(net.now + 2_000, |n| n
        .c(0)
        .seen
        .round_result
        .is_some_and(|r| r.0 == 1)));
    assert_eq!(net.server.game().round_wins(), [1, 1]);
    assert!(net.run_until(net.now + 6_000, |n| matches!(
        n.server.game().phase(),
        RoundPhase::Over { last: true, .. }
    )));
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("team 2 has had no players")));
    let results: Vec<&String> = net
        .pipe
        .iter()
        .filter(|l| l.starts_with("@result"))
        .collect();
    assert_eq!(results.len(), 1);
    assert!(results[0].contains(" win=1 "), "{results:?}");
}

/// GMD-24 on the clients: at the next round start the broken crate's brush has its created flag words again on every
/// client. Needs the integrator hook GMD-24 (session.rs takes `Game::take_restored_breakables` and sends the flag
/// records); enable it with the hook.
#[test]
fn the_crate_is_back_on_every_client_at_the_next_round() {
    let (cfg, room) = fast("room key=ELIM type=12 goal=2 ff=0");
    let mut net = Net::two_alive(cfg, &room, 12);
    let crate0 = net.server.game().breakables()[0].clone();
    net.cm(0).fire_at_point(crate0.obj, crate0.pos, true);
    net.run_for(200);
    for i in 0..2 {
        assert_eq!(net.c(i).seen.objects[&crate0.obj].flags, Some(0x0100));
    }
    let t1 = net.kill(0, 1);
    assert!(net.run_until(t1 + 3_500, |n| n.server.game().round_no() == 2));
    net.run_for(200);
    assert!(net.server.game().restored_breakables().is_empty(), "taken");
    for i in 0..2 {
        assert_eq!(
            net.c(i).seen.objects[&crate0.obj].flags,
            Some(0x3101),
            "client {i}: crate back"
        );
    }
}

/// GMD-21 with DSV-112: the world camera shot of the end trigger runs at every round result and the start trigger
/// switches it off at every round start (the latches of the shot follow the game's round number). Needs the
/// integrator hook GMD-21 (camshot.rs + session.rs); enable it with the hook.
#[test]
fn the_world_camera_shot_runs_at_every_round() {
    use dc_dserver::camshot::{
        CameraInfo, CameraPoint, WorldCameras, OBJ_CAMERA_TYPE, UFLAG_CAMERA_LIVE,
    };
    let mut w = world_with_crate();
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
            pos: [40.0, 120.0, -60.0],
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
    let (cfg, _) = fast("");
    let room = "room key=ELIM type=12 goal=2 ff=0 trig=|RoundStart|RoundEnd|";
    let mut net = Net::two_alive_in(cfg, w, room, 12);
    let live = |n: &Net| {
        n.c(0)
            .seen
            .objects
            .values()
            .find(|o| o.blob_type == Some(OBJ_CAMERA_TYPE))
            .and_then(|o| o.uflags)
            == Some(UFLAG_CAMERA_LIVE)
    };
    assert!(!live(&net));
    let t1 = net.kill(0, 1);
    assert!(
        net.run_until(t1 + 500, |n| live(n)),
        "live at the round 1 result"
    );
    assert!(net.run_until(t1 + 3_500, |n| n.server.game().round_no() == 2));
    assert!(
        net.run_until(net.now + 500, |n| !live(n)),
        "off at the round 2 start"
    );
    net.run_for(300);
    let t2 = net.kill(0, 1);
    assert!(
        net.run_until(t2 + 500, |n| live(n)),
        "live again at the round 2 result"
    );
}
