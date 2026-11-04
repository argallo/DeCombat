//! Mode rules over the loopback of the simulated channel (`dc-net-engine` sim, injected clock): the server and
//! synthetic 2009-style clients. Free-for-all with one team value per player and the kill goal (GMD-10, DSV-148), the
//! clan side switch with 0x5e / 0x5f (GMD-7, DSV-147, DSV-149), the fall damage table with the fall-hurt file
//! (DSV-76, DSV-152) and the pain sound of the character's voice family (DSV-151). The 0x5e / 0x5f / 0x6b bytes are
//! also read back with dc-botclient's readers. The world and the sound rows are synthetic (made-up names).
// SPEC: GAME_MODES_2009.md §4.6, §5.4; GAME_MESSAGES_2009_S2C.md §2.3, §3.2.1; UDP_SESSION_2009_B.md §8.3
// EVIDENCE: cshell+0xcd210 (half-time picture), cshell+0x49a40 cases 0x5e / 0x5f, cshell+0x2b820 (0x6b kind 2 team)
// DC-DEFAULT: GMD-7, GMD-10 (proposed); DSV-147..DSV-152 (proposed)

use dc_dserver::game::modes::SideSwitch;
use dc_dserver::game::{GameTuning, StartOrder};
use dc_dserver::gs;
use dc_dserver::rom_tables::{RomTables, TeamNames};
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::sounds::EventSoundTables;
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{synthetic_world_bytes, world_from_bytes, SpawnPoint, WorldInfo};
use dc_net_engine::sim::{Faults, SimChannel};

const ANNOUNCE: &str = "Worlds\\Synth.dat";
const SNOW_VALLEY: &str = "Worlds\\SnowValley.dat";
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
    pipe: Vec<String>,
    logs: Vec<String>,
    now: u32,
    mode_class: u8,
}

impl Net {
    /// A room line and `user` lines (`user <uid> <sid> <key> <team> <exp> <nick> [tokens]`).
    fn new(cfg: ServerTuning, tables: RomTables, room: &str, users: &[&str]) -> Net {
        Net::new_in(world(), cfg, tables, room, users)
    }

    fn new_in(
        world: WorldInfo,
        mut cfg: ServerTuning,
        tables: RomTables,
        room: &str,
        users: &[&str],
    ) -> Net {
        cfg.game.start_order = StartOrder::Spec;
        let mut server = Server::with_tables(cfg, world, tables, 0);
        assert!(server.handle_line(0, room));
        for u in users {
            assert!(server.handle_line(0, u), "{u}");
        }
        let mode_class = room
            .split_whitespace()
            .find_map(|t| t.strip_prefix("type="))
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        Net {
            server,
            peers: Vec::new(),
            pipe: Vec::new(),
            logs: Vec::new(),
            now: 0,
            mode_class,
        }
    }

    fn add(&mut self, addr: u32, name: &str) {
        let mut cfg = SynthConfig::named(name);
        cfg.box_bounds = Some((
            self.server.world().extent_min,
            self.server.world().extent_max,
        ));
        cfg.walk_step = 0.0;
        cfg.mode_class_hint = Some(self.mode_class);
        cfg.respawn_wait_ms = self.server.config().game.respawn_wait_ms;
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

    fn obj(&self, i: usize) -> u16 {
        self.c(i).seen.view_object.expect("view object")
    }

    fn pid(&self, i: usize) -> u8 {
        self.c(i).seen.client_id.expect("client id").0 as u8
    }

    fn all_alive(&mut self, limit: u32) {
        let n = self.peers.len();
        assert!(
            self.run_until(limit, |net| (0..n).all(|i| net.c(i).life_state() == 1)),
            "states {:?}",
            (0..n).map(|i| self.c(i).life_state()).collect::<Vec<_>>()
        );
        self.run_for(500);
    }

    /// One hit from client `from` on client `to`.
    fn shoot(&mut self, from: usize, to: usize) {
        let target = self.obj(to);
        assert!(
            self.cm(from).fire_at(target, [0.0; 3], true),
            "target not in the stream"
        );
        self.run_for(40);
    }

    fn errors(&self) {
        for i in 0..self.peers.len() {
            assert!(
                self.c(i).seen.errors.is_empty(),
                "client {i}: {:?}",
                self.c(i).seen.errors
            );
        }
    }
}

fn one_shot_kills(mut g: GameTuning) -> ServerTuning {
    g.default_damage = 100.0;
    g.end_pause_ms = (300, 300);
    ServerTuning {
        game: g,
        ..ServerTuning::default()
    }
}

/// GMD-10 / DSV-148: in FFA every player has his own team value (the lobby put Dave and Finn on one team), so a hit
/// between the two lobby team mates hurts and counts; each kill refreshes the counters of killer and victim (0x6b
/// kind 2); a player's kills reach the goal and end the match with his team value as the winner; the lobby gets no
/// team winner.
#[test]
fn ffa_gives_every_player_a_team_value_and_counts_player_kills() {
    let mut net = Net::new(
        one_shot_kills(GameTuning::default()),
        tables(),
        "room key=FFA type=6 goal=2 ff=0",
        &[
            "user 7 3 99 0 120 Dave",
            "user 8 4 55 1 0 Erin",
            "user 9 5 33 0 0 Finn",
        ],
    );
    net.add(1, "Dave");
    net.add(2, "Erin");
    net.add(3, "Finn");
    net.all_alive(20_000);
    let teams: Vec<u8> = (0..3).map(|i| net.c(i).seen.own_team.unwrap()).collect();
    assert_eq!(teams, vec![1, 2, 3], "distinct values in join order");
    // every client's roster agrees
    for i in 0..3 {
        for (j, &t) in teams.iter().enumerate() {
            assert_eq!(net.c(i).seen.roster[&net.pid(j)].team, t);
        }
    }
    let (p0, p2) = (net.pid(0), net.pid(2));
    // Dave on Finn: the lobby's team mates, no friendly refusal in FFA
    let before = net.c(2).seen.roster_log.len();
    net.shoot(0, 2);
    net.run_for(200);
    assert_eq!(net.c(2).seen.damage.len(), 1, "the hit hurts");
    let k = net.c(0).seen.kills.last().expect("kill");
    assert_eq!((k.victim, k.killer, k.kill_kind), (p2, p0, 1), "a foe kill");
    // the 0x6b kind 2 refresh of killer and victim right after the kill
    let refresh: Vec<(u8, u8)> = net.c(2).seen.roster_log[before..]
        .iter()
        .map(|r| (r.1, r.2))
        .collect();
    assert!(refresh.contains(&(p0, gs::INFO_UPDATE)), "{refresh:?}");
    assert!(refresh.contains(&(p2, gs::INFO_UPDATE)), "{refresh:?}");
    for i in 0..3 {
        assert_eq!(net.c(i).seen.roster[&p0].kills, 1, "client {i}");
        assert_eq!(net.c(i).seen.roster[&p2].deaths, 1, "client {i}");
    }
    assert_eq!(net.server.game().ffa_kills_of(p0), 1);
    assert_eq!(net.server.game().scores(), [0, 0], "no team totals in FFA");
    assert!(net.c(0).seen.round_result.is_none(), "goal 2 not reached");
    // Dave on Erin: the second kill reaches the goal; Dave's team value wins
    net.shoot(0, 1);
    net.run_for(1_000);
    for i in 0..3 {
        assert_eq!(
            net.c(i).seen.round_result.map(|r| r.0),
            Some(teams[0] as i8),
            "client {i}"
        );
        assert_eq!(
            net.c(i).seen.result_value.map(|r| r.1),
            Some(teams[0] as u32)
        );
    }
    let result = net.pipe.iter().find(|l| l.starts_with("@result")).unwrap();
    assert!(result.contains(" win=0 "), "{result}");
    // DSV-148: Dave's team value 1 is no team round win
    assert!(result.ends_with("rounds=0:0"), "{result}");
    net.errors();
}

/// DSV-148: the clock runs out in FFA: the player with the most kills wins; a lone player wins after the others left.
#[test]
fn ffa_time_limit_and_last_player_standing() {
    let g = GameTuning {
        team_empty_ms: 1_000,
        ..GameTuning::default()
    };
    let mut net = Net::new(
        one_shot_kills(g),
        tables(),
        "room key=FFA type=6 goal=10 ff=0 time=60",
        &["user 7 3 99 0 120 Dave", "user 8 4 55 0 0 Erin"],
    );
    net.add(1, "Dave");
    net.add(2, "Erin");
    net.all_alive(20_000);
    net.shoot(1, 0);
    let erin = net.c(1).seen.own_team.unwrap();
    assert!(net.run_until(net.now + 62_000, |n| n.c(1).seen.round_result.is_some()));
    assert_eq!(net.c(1).seen.round_result.map(|r| r.0), Some(erin as i8));
    // a second match: two players, one leaves, the other wins after team_empty_ms
    let g = GameTuning {
        team_empty_ms: 1_000,
        ..GameTuning::default()
    };
    let mut net = Net::new(
        one_shot_kills(g),
        tables(),
        "room key=FFA type=6 goal=10 ff=0",
        &["user 7 3 99 0 120 Dave", "user 8 4 55 0 0 Erin"],
    );
    net.add(1, "Dave");
    net.add(2, "Erin");
    net.all_alive(20_000);
    let dave = net.c(0).seen.own_team.unwrap();
    net.cm(1).quit();
    assert!(net.run_until(net.now + 4_000, |n| n.c(0).seen.round_result.is_some()));
    assert_eq!(net.c(0).seen.round_result.map(|r| r.0), Some(dave as i8));
    assert!(net.logs.iter().any(|l| l.contains("ffa: one player left")));
}

/// DSV-471 (project decision): clan Elimination Pro (class 13, goal 2 = two rounds) keeps each team on its
/// side for the whole match: no 0x5e team swap at half time, the roster teams and the own team stay, round 2 spawns
/// Dave at his first base, and the match result names the sides as the lobby gave them.
#[test]
fn clan_elimination_keeps_its_sides_at_half_time() {
    // the bomb round tests start at round 1: no warm-up (DSV-512)
    let g = GameTuning {
        between_round_ms: 500,
        warmup_secs: Some(0),
        ..GameTuning::default()
    };
    let mut net = Net::new(
        one_shot_kills(g),
        tables(),
        "room key=ELIM_CLAN type=13 goal=2 ff=0",
        &["user 7 3 99 0 120 Dave", "user 8 4 55 1 0 Erin"],
    );
    net.add(1, "Dave");
    net.add(2, "Erin");
    net.all_alive(20_000);
    assert_eq!(net.c(0).seen.own_team, Some(1));
    let first_base = *net.c(0).seen.teleports.last().unwrap();
    assert_eq!(first_base.2, RED);
    net.shoot(0, 1);
    // round 1 over (Dave's side: team 1), the between-round set, round 2
    assert!(
        net.run_until(net.now + 5_000, |n| n.server.game().round_no() == 2
            && n.c(1).life_state() == 1),
        "{:?}",
        net.logs
    );
    net.run_for(300);
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert!(s.team_swaps.is_empty(), "client {i}: no switch (DSV-471)");
        assert_eq!(s.roster[&net.pid(0)].team, 1);
        assert_eq!(s.roster[&net.pid(1)].team, 2);
        assert_eq!(s.own_team, Some(if i == 0 { 1 } else { 2 }));
    }
    // Dave spawned at his own base again for round 2
    let tp = *net.c(0).seen.teleports.last().unwrap();
    assert_eq!(tp.2, RED);
    net.run_for(500);
    net.shoot(0, 1);
    net.run_for(1_500);
    assert_eq!(
        net.c(0).seen.round_result.map(|r| r.0),
        Some(1),
        "Dave's team, unchanged"
    );
    let result = net.pipe.iter().find(|l| l.starts_with("@result")).unwrap();
    assert!(
        result.contains(" win=1 ") && result.ends_with("rounds=2:0"),
        "the lobby's sides: {result}"
    );
    assert!(net.c(0).seen.team_swaps.is_empty());
    net.errors();
}

/// GMD-7 / DSV-147: a clan TDM room (class 4, goal 4) is played as two halves, here with the 0x5f kind 1 form
/// (`-sideswitch 5f`). Nothing switches in the tick of a kill: the kill that brings a side to half the goal ends the
/// first half with a `GS_ROUND_RESULT` carrying the team kills (the 2009 client's half-time test: 0x44 `round_limit`
/// = goal, a counter >= goal / 2, cshell+0xcd210), the between-round set carries the switch before the roster
/// refresh, the second half starts with 0x44 round 2 and everyone at his side's new base; the match ends at the goal
/// with the team kills in 0x46 (a counter equal to the limit: no picture).
#[test]
fn clan_tdm_plays_two_halves_with_the_switch_between() {
    let g = GameTuning {
        side_switch: SideSwitch::ClanStatus,
        respawn_wait_ms: Some(2_100),
        between_round_ms: 500,
        ..GameTuning::default()
    };
    let mut net = Net::new(
        one_shot_kills(g),
        tables(),
        "room key=TDM_CLAN type=4 goal=4 ff=0 time=60",
        &["user 7 3 99 0 120 Dave", "user 8 4 55 1 0 Erin"],
    );
    net.add(1, "Dave");
    net.add(2, "Erin");
    net.all_alive(20_000);
    // every 0x44 of the first half: round 1, round_limit = goal
    for i in 0..2 {
        for rb in &net.c(i).seen.round_begins {
            assert_eq!((rb.2, rb.3), (1, 4), "client {i}");
        }
    }
    net.shoot(0, 1);
    assert!(net.c(0).seen.team_swaps.is_empty(), "1 of 4: not yet");
    assert!(net.c(0).seen.round_results.is_empty());
    assert!(net.run_until(net.now + 4_000, |n| n.c(1).life_state() == 1));
    net.run_for(300);
    net.shoot(0, 1);
    // the kill tick: the first half's result with the team kills, no switch yet
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert!(
            s.team_swaps.is_empty(),
            "client {i}: no switch in the kill tick"
        );
        assert_eq!(s.round_results.len(), 1, "client {i}");
        let (_, w, a, b) = s.round_results[0];
        assert_eq!(
            (w, a, b),
            (1, 2, 0),
            "client {i}: the team kills of the half"
        );
        assert_eq!(s.own_team, Some(if i == 0 { 1 } else { 2 }));
    }
    assert_eq!(net.server.game().scores(), [2, 0]);
    assert!(
        net.pipe.iter().all(|l| !l.starts_with("@result")),
        "the match goes on"
    );
    // the between-round set, then the second half
    assert!(
        net.run_until(net.now + 5_000, |n| n.server.game().round_no() == 2
            && n.c(1).life_state() == 1),
        "{:?}",
        net.logs
    );
    net.run_for(300);
    for i in 0..2 {
        let s = &net.c(i).seen;
        assert_eq!(s.team_swaps.len(), 1, "client {i}");
        let (_, id, scores, own) = s.team_swaps[0];
        assert_eq!(id, gs::GS_CLAN_STATUS);
        assert_eq!(scores, [0, 2], "client {i}: the kills moved with the sides");
        assert_eq!(own, Some(if i == 0 { 2 } else { 1 }));
        assert_eq!(s.roster[&net.pid(0)].team, 2);
        assert_eq!(s.roster[&net.pid(1)].team, 1);
        // 0x46 before the switch; the switch inside the between-round set: 0x47, 0x5f, roster, 0x48
        let o: Vec<u8> = s
            .shell_ids
            .iter()
            .map(|x| x.1)
            .filter(|&x| x != 0xa6)
            .collect();
        let r = o.iter().position(|&x| x == 0x46).unwrap();
        let at = o.iter().position(|&x| x == gs::GS_CLAN_STATUS).unwrap();
        assert!(r < at, "client {i}: {o:x?}");
        assert_eq!(o[at - 1], 0x47, "client {i}: {o:x?}");
        assert_eq!(&o[at + 1..at + 3], &[0x6b, 0x6b], "client {i}: {o:x?}");
        assert_eq!(o[at + 3], 0x48, "client {i}: {o:x?}");
        let rb = *s.round_begins.last().unwrap();
        assert_eq!((rb.2, rb.3), (2, 4), "client {i}: second half");
        assert!(
            rb.1 < 60 && rb.1 > 50,
            "client {i}: the clock goes on: {} s",
            rb.1
        );
    }
    // Dave spawned at team 2's base for the second half
    let tp = *net.c(0).seen.teleports.last().unwrap();
    assert_eq!(tp.2, BLUE);
    assert_eq!(net.server.game().scores(), [0, 2]);
    // two more kills by Dave (now team 2) reach the goal for team 2
    for _ in 0..2 {
        assert!(net.run_until(net.now + 4_000, |n| n.c(1).life_state() == 1));
        net.run_for(300);
        net.shoot(0, 1);
    }
    net.run_for(1_000);
    let (_, w, a, b) = *net.c(0).seen.round_results.last().unwrap();
    assert_eq!(
        (w, a, b),
        (2, 0, 4),
        "the match end: a counter at the limit"
    );
    assert_eq!(net.c(0).seen.round_results.len(), 2);
    assert_eq!(net.c(0).seen.team_swaps.len(), 1, "once per match");
    let result = net.pipe.iter().find(|l| l.starts_with("@result")).unwrap();
    assert!(
        result.contains(" win=1 alpha=4 bravo=0 "),
        "the lobby's sides: {result}"
    );
    net.errors();
}

/// DSV-147: the client's half-time test (cshell+0xcd210) on the 0x44 / 0x46 values of a two-halves class-4 match:
/// the picture at the half, never at the match end on the goal.
#[test]
fn clan_tdm_half_values_meet_the_client_test() {
    use dc_dserver::game::modes::half_time_picture;
    assert!(half_time_picture(4, 4, [2, 0], 1, false), "at the half");
    assert!(
        !half_time_picture(4, 4, [0, 4], 2, false),
        "the goal: match over"
    );
    // the old single-round form: round_limit 1, wins 0 / 0 drew the picture at the match end
    assert!(half_time_picture(4, 1, [0, 0], 1, false));
}

/// DSV-148: in FFA a player without a statistics row (no uid in the roster) keeps his kills in the per-kill refresh.
#[test]
fn ffa_refresh_counts_a_player_without_a_uid() {
    let mut cfg = one_shot_kills(GameTuning::default());
    cfg.accept_unlisted = true;
    let mut net = Net::new(
        cfg,
        tables(),
        "room key=FFA type=6 goal=5 ff=0",
        &["user 7 3 99 0 120 Dave"],
    );
    net.add(1, "Dave");
    net.add(2, "Erin");
    net.all_alive(20_000);
    let (p0, p1) = (net.pid(0), net.pid(1));
    net.shoot(1, 0);
    net.run_for(200);
    assert_eq!(net.server.game().ffa_kills_of(p1), 1);
    for i in 0..2 {
        assert_eq!(
            net.c(i).seen.roster[&p1].kills,
            1,
            "client {i}: Erin's kill"
        );
        assert_eq!(net.c(i).seen.roster[&p0].deaths, 1, "client {i}");
    }
    net.errors();
}

/// The 0x5e / 0x5f / 0x6b writers read back with dc-botclient's own readers.
#[test]
fn switch_messages_read_back_with_the_bot_client_readers() {
    let m = gs::team_swap([3, 300]);
    let mut r = m.reader();
    assert_eq!((r.read_u8(), r.read_u8()), (0x0d, gs::GS_TEAM_SWAP));
    assert_eq!(dc_botclient::shell::read_team_pair(&mut r), [3, 255]);
    let m = gs::clan_status_scores([1, 2], false);
    let mut r = m.reader();
    assert_eq!((r.read_u8(), r.read_u8()), (0x0d, gs::GS_CLAN_STATUS));
    assert_eq!(r.read_u8(), gs::CLAN_STATUS_SCORES);
    assert_eq!(dc_botclient::shell::read_team_pair(&mut r), [1, 2]);
    assert!(!r.read_bit(), "no_swap 0");
}

/// Synthetic event sound rows: two voice families with made-up files; body row 4 uses the second family's figure.
fn sound_tables() -> RomTables {
    let mut t = tables();
    t.event_sounds = EventSoundTables {
        pain: vec!["SNDM\\PAINM1.WAV".into()],
        pain_w: vec!["SNDW\\PAINW1.WAV".into()],
        fall: Some("SNDM\\FALLM.WAV".into()),
        fall_w: Some("SNDM\\FALLW.WAV".into()),
        body_figure: [(0u32, 0u32), (4, 1)].into_iter().collect(),
        figures_w: [1u32].into_iter().collect(),
        ..EventSoundTables::default()
    };
    t
}

fn new_sound_files(net: &Net, i: usize, from: usize) -> Vec<String> {
    let c = net.c(i);
    c.seen.sound_starts[from..]
        .iter()
        .map(|(_, f)| {
            String::from_utf8_lossy(&c.seen.files.get(f).map(|x| x.1.clone()).unwrap_or_default())
                .into_owned()
        })
        .collect()
}

/// Lifts client `i` `h` units above its ground height (airborne) and lands it there again.
fn fall(net: &mut Net, i: usize, h: f32) {
    let y0 = net.c(i).position()[1];
    net.cm(i).set_on_ground(false);
    net.cm(i).set_height(y0 + h);
    net.run_for(200);
    net.cm(i).set_height(y0);
    net.cm(i).set_on_ground(true);
    net.run_for(200);
}

/// DSV-76 / DSV-152 / DSV-151: a landing after a fall hurts by the table (0.5 x (h - 300) with the defaults) with
/// damage type 5, and plays the fall-hurt file of the faller's voice family instead of a pain variant; a gun hit plays
/// the pain variant of the victim's family. Erin's `user` line names body row 4 (second family), Dave's row 0.
#[test]
fn falls_and_hits_play_the_voice_family_of_the_character() {
    let g = GameTuning {
        move_check: false,
        ..GameTuning::default()
    };
    let cfg = ServerTuning {
        game: g,
        ..ServerTuning::default()
    };
    let mut net = Net::new(
        cfg,
        sound_tables(),
        "room key=TDM type=1 goal=10 ff=0",
        &[
            "user 7 3 99 0 120 Dave model=0",
            "user 8 4 55 1 0 Erin model=4",
        ],
    );
    net.add(1, "Dave");
    net.add(2, "Erin");
    net.all_alive(20_000);
    let s0 = net.c(0).seen.sound_starts.len();
    // Erin falls 350 units: 25 damage, zone 5, the second family's fall file
    fall(&mut net, 1, 350.0);
    let d = net
        .c(1)
        .seen
        .damage_forms
        .last()
        .copied()
        .expect("fall damage");
    assert_eq!((d.zone, d.damage, d.dmg_kind), (5, 25, 0));
    assert_eq!(
        new_sound_files(&net, 0, s0),
        vec!["SNDM\\FALLW.WAV".to_string()]
    );
    // Dave falls 400 units: 50 damage, the default family's fall file
    let s0 = net.c(1).seen.sound_starts.len();
    fall(&mut net, 0, 400.0);
    let d = net
        .c(0)
        .seen
        .damage_forms
        .last()
        .copied()
        .expect("fall damage");
    assert_eq!((d.zone, d.damage), (5, 50));
    assert_eq!(
        new_sound_files(&net, 1, s0),
        vec!["SNDM\\FALLM.WAV".to_string()]
    );
    // a fall below the lower limit does nothing
    let n = net.c(0).seen.damage_forms.len();
    fall(&mut net, 0, 290.0);
    assert_eq!(net.c(0).seen.damage_forms.len(), n);
    // gun hits after the fall sounds ended: the pain variant of each victim's family
    net.run_for(dc_dserver::sounds::UNKNOWN_PLAY_MS + 100);
    let s0 = net.c(0).seen.sound_starts.len();
    net.shoot(0, 1);
    net.run_for(100);
    assert_eq!(
        new_sound_files(&net, 0, s0),
        vec!["SNDW\\PAINW1.WAV".to_string()]
    );
    let s0 = net.c(1).seen.sound_starts.len();
    net.shoot(1, 0);
    net.run_for(100);
    assert_eq!(
        new_sound_files(&net, 1, s0),
        vec!["SNDM\\PAINM1.WAV".to_string()]
    );
    net.errors();
}

/// The kind-5 `z_sub` of a type-0x37 blob (`None` for any other blob).
fn use_point_sub(o: &dc_dserver::synth_client::SeenObject) -> Option<u8> {
    let b = o.blob.as_ref()?;
    let mut r = b.reader();
    if r.read_u8() != 0x37 {
        return None;
    }
    // z_a, z_b, z_flag, extent, icon / team / class / c, z_value
    r.skip_bits(64 + 1 + 72 + 32 + 32);
    (r.read_u32() == 5).then(|| {
        r.read_u32();
        r.read_u8()
    })
}

/// DSV-220..DSV-222 over the loopback: a bomb room offers only plant points at load; the attacker's 4 s hold inside
/// the site plants the bomb, and every client gets the defuse point at the bomb (with the user's ROM also the bomb
/// model and its looping indicator effect); the blast 40 s later removes them and places the one-shot blast effect,
/// which leaves after its time.
#[test]
fn the_planted_bomb_and_its_blast_reach_every_client() {
    use dc_dserver::game::bomb::{BLAST_FX_MS, BOMB_SECS, PLANT_MS};
    use dc_dserver::world::{ZoneInfo, ZoneKind};
    let mut w = world();
    w.zones = vec![ZoneInfo {
        kind: ZoneKind::BombSite,
        pos: RED,
        rot: [0.0; 3],
        dims: [200.0; 3],
        name: b"SiteA".to_vec(),
        team_text: Some(b"Blue".to_vec()),
        number: 1,
        planted_command: Vec::new(),
    }];
    let rom = std::env::var("CA_ROM").ok();
    let mut t = tables();
    t.rom = rom.as_ref().map(std::path::PathBuf::from);
    // the bomb round tests start at round 1: no warm-up (DSV-512)
    let g = GameTuning {
        between_round_ms: 500,
        warmup_secs: Some(0),
        ..GameTuning::default()
    };
    let mut net = Net::new_in(
        w,
        one_shot_kills(g),
        t,
        "room key=BOMB type=2 goal=3 ff=0 time=300",
        &["user 7 3 99 0 120 Dave", "user 8 4 55 1 0 Erin"],
    );
    net.add(1, "Dave");
    net.add(2, "Erin");
    net.all_alive(20_000);
    let points = |net: &Net, i: usize| -> Vec<(u16, u8, [f32; 3])> {
        net.c(i)
            .seen
            .objects
            .iter()
            .filter_map(|(id, o)| use_point_sub(o).map(|s| (*id, s, o.pos.unwrap_or(o.create_pos))))
            .collect()
    };
    let at_load = points(&net, 1);
    assert_eq!(
        at_load.iter().map(|p| p.1).collect::<Vec<_>>(),
        vec![1],
        "only the plant point at load: {at_load:?}"
    );
    let before: Vec<u16> = net.c(1).seen.objects.keys().copied().collect();
    // Dave (team 1) carries the bomb and stands in the site at his spawn
    let mut body = dc_wire::udp::BitWriter::new();
    body.write_u8(1).write_objref16(Some(at_load[0].0));
    net.cm(0).send_raw_shell(0xd8, &body);
    net.run_for(PLANT_MS + 500);
    assert!(
        net.logs.iter().any(|l| l.contains("bomb: planted")),
        "{:#?}",
        net.logs
            .iter()
            .filter(|l| l.contains("bomb"))
            .collect::<Vec<_>>()
    );
    let new_objs = |net: &Net, i: usize| -> Vec<(u16, Option<u8>, usize)> {
        net.c(i)
            .seen
            .objects
            .iter()
            .filter(|(id, _)| !before.contains(id))
            .map(|(id, o)| (*id, o.blob_type, o.models.len()))
            .collect()
    };
    for i in 0..2 {
        let p = points(&net, i);
        let defuse: Vec<_> = p.iter().filter(|p| p.1 == 2).collect();
        assert_eq!(defuse.len(), 1, "client {i}: one defuse point {p:?}");
        let d = defuse[0].2;
        let at = net.c(0).position();
        // DSV-360: below the planter (this world has no trace geometry: at his feet, below his centre)
        assert!(
            [0, 2].iter().all(|&k| (d[k] - at[k]).abs() < 2.0) && d[1] < at[1] - 20.0,
            "at the bomb: {d:?} (planter {at:?})"
        );
        let n = new_objs(&net, i);
        if rom.is_some() {
            assert!(
                n.iter().any(|o| o.1 == Some(0x17) && o.2 >= 1),
                "client {i}: the bomb model {n:?}"
            );
            assert!(
                n.iter().any(|o| o.1 == Some(0x64)),
                "client {i}: the indicator {n:?}"
            );
        }
    }
    let shown: Vec<u16> = new_objs(&net, 1).iter().map(|o| o.0).collect();
    net.run_for(BOMB_SECS * 1000);
    assert!(net.logs.iter().any(|l| l.contains("bomb: exploded")));
    for i in 0..2 {
        assert!(
            shown.iter().all(|o| net.c(i).seen.removed.contains(o)),
            "client {i}: the bomb objects are removed"
        );
        assert!(points(&net, i).iter().all(|p| p.1 != 2));
    }
    if rom.is_some() {
        let blast: Vec<u16> = new_objs(&net, 1)
            .iter()
            .filter(|o| o.1 == Some(0x64) && !shown.contains(&o.0))
            .map(|o| o.0)
            .collect();
        assert_eq!(blast.len(), 1, "the blast effect");
        net.run_for(BLAST_FX_MS + 200);
        assert!(
            net.c(1).seen.removed.contains(&blast[0]),
            "the blast effect leaves"
        );
    }
    net.errors();
}

/// CLI-1340: the bomb site's planted command switches its indicators on every client: the green indicator (a world
/// effect on from the start) is removed and the red one (off at the start) created at the plant; the next round
/// restores both (the green one again under its own object, the red one removed). A client that enters meanwhile
/// would get the red one and not the green one (the world records skip a switched-off effect).
#[test]
fn a_plant_turns_the_site_indicator_from_green_to_red_until_the_next_round() {
    use dc_dserver::game::bomb::{BOMB_SECS, PLANT_MS};
    use dc_dserver::world::{EffectInfo, ZoneInfo, ZoneKind};
    const GREEN_FX: &[u8] = b"FxGreen";
    const RED_FX: &[u8] = b"FxRed";
    let mut w = world();
    w.zones = vec![ZoneInfo {
        kind: ZoneKind::BombSite,
        pos: RED,
        rot: [0.0; 3],
        dims: [200.0; 3],
        name: b"SiteA".to_vec(),
        team_text: Some(b"Blue".to_vec()),
        number: 1,
        planted_command: b"msg SiteA lock;
msg GreenA off;
msg RedA on;
"
        .to_vec(),
    }];
    let fx = |name: &[u8], fx: &[u8], on: bool| EffectInfo {
        name: name.to_vec(),
        pos: RED,
        rot: [-std::f32::consts::FRAC_PI_2, 0.0, 0.0],
        fx: fx.to_vec(),
        looped: true,
        start_on: on,
    };
    w.effects = vec![fx(b"GreenA", GREEN_FX, true), fx(b"RedA", RED_FX, false)];
    // the bomb round tests start at round 1: no warm-up (DSV-512)
    let g = GameTuning {
        between_round_ms: 500,
        warmup_secs: Some(0),
        ..GameTuning::default()
    };
    let mut net = Net::new_in(
        w,
        one_shot_kills(g),
        tables(),
        "room key=BOMB type=2 goal=3 ff=0 time=300",
        &["user 7 3 99 0 120 Dave", "user 8 4 55 1 0 Erin"],
    );
    net.add(1, "Dave");
    net.add(2, "Erin");
    net.all_alive(20_000);
    // the effect object by the effect name in its blob
    let effect_at = |net: &Net, i: usize, fx: &[u8]| -> Option<u16> {
        net.c(i)
            .seen
            .objects
            .iter()
            .find(|(_, o)| {
                o.blob_type == Some(0x64)
                    && o.blob
                        .as_ref()
                        .is_some_and(|b| b.as_bytes().windows(fx.len()).any(|w| w == fx))
            })
            .map(|(id, _)| *id)
    };
    let green: Vec<u16> = (0..2)
        .map(|i| effect_at(&net, i, GREEN_FX).expect("the green indicator at load"))
        .collect();
    for i in 0..2 {
        assert_eq!(
            effect_at(&net, i, RED_FX),
            None,
            "client {i}: no red indicator at load"
        );
    }
    let plant_point = net
        .c(0)
        .seen
        .objects
        .iter()
        .find(|(_, o)| use_point_sub(o) == Some(1))
        .map(|(id, _)| *id)
        .expect("the plant point");
    let mut body = dc_wire::udp::BitWriter::new();
    body.write_u8(1).write_objref16(Some(plant_point));
    net.cm(0).send_raw_shell(0xd8, &body);
    net.run_for(PLANT_MS + 500);
    assert!(net.logs.iter().any(|l| l.contains("bomb: planted")));
    let mut red = Vec::new();
    for (i, g) in green.iter().enumerate().take(2) {
        assert!(
            net.c(i).seen.removed.contains(g),
            "client {i}: the green indicator goes"
        );
        assert_eq!(effect_at(&net, i, GREEN_FX), None);
        red.push(effect_at(&net, i, RED_FX).expect("the red indicator at the plant"));
    }
    // the blast ends the round; the red indicator stays through the round end, the next round restores
    net.run_for(BOMB_SECS * 1000);
    assert!(net.logs.iter().any(|l| l.contains("bomb: exploded")));
    for (i, r) in red.iter().enumerate().take(2) {
        assert_eq!(
            effect_at(&net, i, RED_FX),
            Some(*r),
            "client {i}: red during the round end"
        );
    }
    let t = net.now;
    assert!(net.run_until(t + 15_000, |n| n.server.game().round_no() == 2));
    net.run_for(500);
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("world effects restored: 1 back, 1 removed")));
    for i in 0..2 {
        assert!(
            net.c(i).seen.removed.contains(&red[i]),
            "client {i}: the red indicator goes"
        );
        assert_eq!(
            effect_at(&net, i, GREEN_FX),
            Some(green[i]),
            "client {i}: green again"
        );
    }
    net.errors();
}

/// DSV-240 over the loopback (user's ROM): in a Search and Destroy room of a world with a GMS row the attacker's
/// client gets the GMS bomb item in slot 9 (0xb2), takes it out (0x7f), plants with it (0xd8 hold) and loses it at
/// the plant (0xb4 of slot 9); the defender never gets it.
#[test]
fn the_carrier_gets_the_bomb_item_takes_it_out_and_loses_it_at_the_plant() {
    use dc_dserver::game::bomb::{BOMB_SLOT, PLANT_MS};
    use dc_dserver::world::{ZoneInfo, ZoneKind};
    let Ok(rom) = std::env::var("CA_ROM") else {
        eprintln!("skipped: CA_ROM not set");
        return;
    };
    let (mut t, _) = RomTables::load(std::path::Path::new(&rom));
    t.teams = tables().teams;
    let mut w = world_from_bytes(&synthetic_world_bytes(), SNOW_VALLEY, "SnowValley").unwrap();
    w.spawns = world().spawns;
    w.zones = vec![ZoneInfo {
        kind: ZoneKind::BombSite,
        pos: RED,
        rot: [0.0; 3],
        dims: [200.0; 3],
        name: b"SiteA".to_vec(),
        team_text: Some(b"Blue".to_vec()),
        number: 1,
        planted_command: Vec::new(),
    }];
    let mut net = Net::new_in(
        w,
        one_shot_kills(GameTuning {
            warmup_secs: Some(0),
            ..GameTuning::default()
        }),
        t,
        "room key=TMM type=2 goal=3 ff=0 time=300",
        &["user 7 3 99 0 120 Dave", "user 8 4 55 1 0 Erin"],
    );
    net.add(1, "Dave");
    net.add(2, "Erin");
    net.all_alive(20_000);
    net.run_for(500);
    let item = net
        .logs
        .iter()
        .find_map(|l| {
            l.split("the bomb item is weapon ")
                .nth(1)
                .and_then(|r| r.split_whitespace().next())
                .and_then(|n| n.parse::<u16>().ok())
        })
        .unwrap_or_else(|| {
            panic!(
                "{:#?}",
                net.logs
                    .iter()
                    .filter(|l| l.contains("bomb"))
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        net.c(0)
            .seen
            .weapons
            .iter()
            .any(|w| w.0 == 0xb2 && w.1 == item),
        "the carrier's grant"
    );
    assert!(
        !net.c(1).seen.weapons.iter().any(|w| w.1 == item),
        "no item for the defender"
    );
    // take it out (0x76 select, 0x7f change), then hold at the plant point
    let mut sel = dc_wire::udp::BitWriter::new();
    sel.write_u8(3).write_u8(2);
    net.cm(0).send_raw_shell(0x76, &sel);
    let mut ch = dc_wire::udp::BitWriter::new();
    ch.write_u8(BOMB_SLOT).write_u16(item).write_u16(0);
    net.cm(0).send_raw_shell(0x7f, &ch);
    net.run_for(200);
    let plant = net
        .c(0)
        .seen
        .objects
        .iter()
        .find_map(|(id, o)| (use_point_sub(o) == Some(1)).then_some(*id))
        .expect("plant point");
    let removes_before = net
        .c(0)
        .seen
        .shell_ids
        .iter()
        .filter(|s| s.1 == 0xb4)
        .count();
    let mut body = dc_wire::udp::BitWriter::new();
    body.write_u8(1).write_objref16(Some(plant));
    net.cm(0).send_raw_shell(0xd8, &body);
    net.run_for(PLANT_MS + 500);
    assert!(net.logs.iter().any(|l| l.contains("bomb: planted")));
    let removes = net
        .c(0)
        .seen
        .shell_ids
        .iter()
        .filter(|s| s.1 == 0xb4)
        .count();
    assert_eq!(
        removes,
        removes_before + 1,
        "0xb4 of the bomb slot at the plant"
    );
    // DSV-300: the planter is held in place for the hold and released after it
    let n = |k: &str| net.logs.iter().filter(|l| l.contains(k)).count();
    assert_eq!(
        (n("frozen for the plant"), n("released after the hold")),
        (1, 1),
        "{:#?}",
        net.logs
            .iter()
            .filter(|l| l.contains("DSV-300"))
            .collect::<Vec<_>>()
    );
    net.errors();
}
