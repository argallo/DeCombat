//! Deterministic end-to-end tests of the session state machine: the server and synthetic clients talk through
//! the simulated channel of `dc-net-engine` with injected time (no sockets, no processes).

use dc_dserver::session::{Output, Phase, Server, ServerTuning, StartOrder, Step};
use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig};
use dc_dserver::world::{synthetic_world_bytes, world_from_bytes, WorldInfo};
use dc_net_engine::sim::{Faults, SimChannel};
use dc_net_engine::{Link, LinkConfig, LinkEvent};
use dc_wire::udp::{OobDatagram, TODO_GAME_APP_ID_GUID_LAYOUT};

const ANNOUNCE: &str = "Worlds\\Synth.dat";

fn world() -> WorldInfo {
    world_from_bytes(&synthetic_world_bytes(), ANNOUNCE, "Synth").unwrap()
}

/// The synthetic world with one sky model (made-up name, sky slot 0, sky box 64 x 0.5 around (1000, 0, 0)).
fn world_with_sky() -> WorldInfo {
    let mut w = world();
    w.brushes.push(dc_dserver::world::BrushInfo {
        name: b"SkyBox".to_vec(),
        role: None,
        pos: [1000.0, 0.0, 0.0],
        rot: [0.0; 3],
        visible: true,
        solid: false,
        rayhit: false,
        blend: 0,
        fog_disable: false,
        class: dc_dserver::world::BrushClass::Sky,
        sky: Some(dc_dserver::world::SkyInfo {
            index: 0,
            dims: [64.0; 3],
            inner: [0.5; 3],
        }),
    });
    w
}

struct Peer {
    addr: u32,
    client: SynthClient,
    up: SimChannel,
    down: SimChannel,
    /// The uplink is cut (a lag spike): datagrams are only kept in `last_up`.
    muted: bool,
    last_up: Vec<u8>,
}

struct Net {
    server: Server<u32>,
    peers: Vec<Peer>,
    pipe: Vec<(u32, String)>,
    logs: Vec<String>,
    now: u32,
}

impl Net {
    /// The slice-1 tests check the spec start-up order (`-startorder spec`); [`Net::reference`] keeps the default.
    fn new(mut cfg: ServerTuning) -> Net {
        cfg.game.start_order = StartOrder::Spec;
        Net::reference(cfg)
    }

    fn reference(cfg: ServerTuning) -> Net {
        Net::with_world(cfg, world())
    }

    fn with_world(cfg: ServerTuning, world: WorldInfo) -> Net {
        Net::with_room(cfg, world, "room key=TDM type=0 goal=2 ff=0")
    }

    fn with_room(cfg: ServerTuning, world: WorldInfo, room: &str) -> Net {
        let mut server = Server::new(cfg, world, 0);
        assert!(server.handle_line(0, room));
        assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
        assert!(server.handle_line(0, "user 8 4 55 1 0 Big_Erin"));
        Net {
            server,
            peers: Vec::new(),
            pipe: Vec::new(),
            logs: Vec::new(),
            now: 0,
        }
    }

    fn add(&mut self, addr: u32, cfg: SynthConfig, faults: Faults) {
        self.peers.push(Peer {
            addr,
            client: SynthClient::new(cfg),
            up: SimChannel::new(addr as u64 * 2 + 1, faults),
            down: SimChannel::new(addr as u64 * 2 + 2, faults),
            muted: false,
            last_up: Vec::new(),
        });
    }

    fn step(&mut self) {
        let now = self.now;
        for p in &mut self.peers {
            for d in p.client.poll(now) {
                p.last_up = d.clone();
                if !p.muted {
                    p.up.send(now, d);
                }
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

    fn client(&self, i: usize) -> &SynthClient {
        &self.peers[i].client
    }

    fn pipe_lines(&self) -> Vec<String> {
        // MON-1 link summaries have their own tests; these checks are about the session events
        self.pipe
            .iter()
            .map(|p| p.1.clone())
            .filter(|l| !l.starts_with("@linksum"))
            .collect()
    }
}

fn dave() -> SynthConfig {
    SynthConfig::named("Dave")
}

#[test]
fn slice_steps_s1_to_s13_on_a_clean_channel() {
    let mut net = Net::new(ServerTuning::default());
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    let t_in = net.now;
    net.run_until(t_in + 2000, |_| false);

    let c = net.client(0);
    let seen = &c.seen;
    assert_eq!(seen.version, Some((7, 5_120_000)));
    assert_eq!(
        seen.link_rate,
        Some(5_120_000),
        "client lowered its link rate to the host rate"
    );
    assert_eq!(seen.client_id, Some((0, 0)));
    assert_eq!(
        seen.files.get(&1).map(|f| f.1.as_slice()),
        Some(ANNOUNCE.as_bytes())
    );
    assert_eq!(
        seen.files.get(&1).map(|f| f.0),
        Some(net.server.world().size)
    );
    assert_eq!(seen.load_world.map(|l| l.1), Some(1));
    assert_eq!(seen.preload_subs, vec![0, 1]);
    assert_eq!(
        seen.load_states_sent
            .iter()
            .map(|x| x.1)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    let obj = seen.view_object.expect("view object");
    assert_eq!(obj, 1);
    let o = &seen.objects[&obj];
    assert_eq!(o.kind, 0);
    assert_eq!(o.flags, Some(0x0100), "full precision flag");
    assert_eq!(
        o.pos,
        Some(net.server.world().centre()),
        "no spawn role in the synthetic world: box centre"
    );
    assert_eq!(seen.switch_state, Some(2));
    assert!(seen.errors.is_empty(), "{:?}", seen.errors);

    // reliable order the client processed: start-up messages first, then world, preload, view, object, shell
    // (the view object before the own create: DSV-23 amendment, the reference stack's order, tdm6 c6 seq 714 < 743)
    let ids: Vec<u8> = seen.messages.iter().map(|m| m.1).collect();
    let first = |id: u8| ids.iter().position(|&x| x == id).unwrap_or(usize::MAX);
    assert_eq!(&ids[..3], &[4, 12, 0x32], "{ids:?}");
    assert!(first(0x32) < first(6) && first(6) < first(19));
    assert!(first(19) < first(7) && first(7) < first(8) && first(8) < first(13));
    assert!(first(13) < first(10) || first(10) == usize::MAX);

    // time stream: monotonic, about 10 per second while in world
    let times: Vec<f32> = seen.server_times.iter().map(|t| t.1).collect();
    assert!(times.windows(2).all(|w| w[1] > w[0]));
    let recent = seen
        .server_times
        .iter()
        .filter(|t| t.0 >= t_in && t.0 < t_in + 2000)
        .count();
    assert!((17..=23).contains(&recent), "{recent} time entries in 2 s");
    assert_eq!(seen.stale_times, 0);

    // server side: slice list order and gates
    let (_, sc) = net.server.clients().iter().next().expect("client");
    assert_eq!(sc.phase, Phase::InWorld);
    let at = |s: Step| {
        sc.reached(s)
            .unwrap_or_else(|| panic!("{s:?} not reached: {:?}", sc.steps))
    };
    assert!(at(Step::Linked) <= at(Step::Version));
    assert!(at(Step::LoadWorld) >= at(Step::Hello) && at(Step::LoadWorld) >= at(Step::FilesAcked));
    assert!(
        at(Step::WorldLoaded) - at(Step::LoadWorld) >= 1500,
        "the client's world load takes 1.5 s"
    );
    assert!(at(Step::PreloadDone) > at(Step::WorldLoaded));
    assert!(at(Step::ObjectCreated) >= at(Step::PreloadDone));
    assert!(at(Step::ViewObject) >= at(Step::ObjectCreated));
    assert!(at(Step::FirstTick) >= at(Step::SwitchState));
    // the client's in-world test passes on view object + shell 0x88, which can precede the first tick
    assert!(at(Step::FirstUpdate) > at(Step::SwitchState));
    assert!(
        sc.reached(Step::SkyTable).is_none(),
        "sky table is off by default"
    );
    assert_eq!(sc.rate_bps, Some(32_000), "C2S_RATE 4 x 8000");
    // player updates at about 15 per second, position parsed
    assert!(sc.updates >= 25, "{}", sc.updates);
    let pos = sc
        .last_update
        .as_ref()
        .and_then(|u| u.position())
        .expect("position");
    assert!(pos[0] > 20.0, "{pos:?}");
    assert_eq!(net.pipe_lines(), vec!["@join 0 7 99 0 Dave".to_string()]);

    // quit: results, leave, end; the client sees the plain bye (code 3 on a non-host)
    let now = net.now;
    net.server.finish(now);
    net.run_until(now + 1000, |_| false);
    let lines = net.pipe_lines();
    assert_eq!(
        &lines[1..],
        &[
            "@player uid=7 kills=0 deaths=0 team=1 tk=0 hs=0 name=Dave".to_string(),
            "@result mode=TDM win=0 alpha=0 bravo=0 limit=2 rounds=0:0".to_string(),
            "@leave 0".to_string(),
            "@end".to_string(),
        ]
    );
    assert_eq!(net.client(0).state(), ClientState::Dropped(3));
    assert!(!net.server.has_links());
}

#[test]
fn slice_completes_over_a_lossy_reordering_channel() {
    let faults = Faults {
        loss_permille: 200,
        dup_permille: 50,
        corrupt_permille: 0,
        min_delay_ms: 5,
        max_delay_ms: 80,
    };
    let mut net = Net::new(ServerTuning::default());
    net.add(1, dave(), faults);
    assert!(
        net.run_until(60_000, |n| n.client(0).state() == ClientState::InWorld),
        "state {:?}",
        net.client(0).state()
    );
    let t = net.now;
    net.run_until(t + 3000, |_| false);
    let seen = &net.client(0).seen;
    assert!(seen.errors.is_empty(), "{:?}", seen.errors);
    assert_eq!(seen.preload_subs, vec![0, 1]);
    assert!(seen.server_times.windows(2).all(|w| w[1].1 > w[0].1));
    let (_, sc) = net.server.clients().iter().next().unwrap();
    assert!(sc.updates > 10);
    assert_eq!(net.client(0).state(), ClientState::InWorld);
}

#[test]
fn two_clients_get_their_own_ids_and_objects() {
    let mut net = Net::new(ServerTuning::default());
    net.add(1, dave(), Faults::clean(3));
    net.add(2, SynthConfig::named("Big Erin"), Faults::clean(7));
    assert!(net.run_until(20_000, |n| {
        n.client(0).state() == ClientState::InWorld && n.client(1).state() == ClientState::InWorld
    }));
    let ids: Vec<(u16, u16)> = (0..2)
        .map(|i| {
            let s = &net.client(i).seen;
            (s.client_id.unwrap().0, s.view_object.unwrap())
        })
        .collect();
    assert_eq!(ids, vec![(0, 1), (1, 2)]);
    let mut lines = net.pipe_lines();
    lines.sort();
    // the hello name "Big Erin" matches the roster nick "Big_Erin" (the lobby replaced the space)
    assert_eq!(
        lines,
        vec!["@join 0 7 99 0 Dave", "@join 1 8 55 1 Big_Erin"]
    );
}

#[test]
fn unknown_name_is_refused_with_bye_code_6() {
    let mut net = Net::new(ServerTuning::default());
    net.add(1, SynthConfig::named("Mallory"), Faults::clean(2));
    assert!(net.run_until(10_000, |n| matches!(
        n.client(0).state(),
        ClientState::Dropped(_)
    )));
    assert_eq!(net.client(0).state(), ClientState::Dropped(6));
    assert!(net.pipe_lines().is_empty());
    net.run_until(net.now + 600, |_| false);
    assert!(net.server.clients().is_empty());
}

#[test]
fn open_server_accepts_unlisted_names_without_pipe_events() {
    let cfg = ServerTuning {
        accept_unlisted: true,
        ..ServerTuning::default()
    };
    let mut net = Net::new(cfg);
    net.add(1, SynthConfig::named("Guest"), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    assert!(net.pipe_lines().is_empty());
}

#[test]
fn client_bye_is_answered_with_bye_code_5_and_leave() {
    let mut net = Net::new(ServerTuning::default());
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    net.peers[0].client.quit();
    assert!(net.run_until(net.now + 3000, |n| matches!(
        n.client(0).state(),
        ClientState::Dropped(_)
    )));
    assert_eq!(net.client(0).state(), ClientState::Dropped(5));
    net.run_until(net.now + 600, |_| false);
    assert_eq!(net.pipe_lines(), vec!["@join 0 7 99 0 Dave", "@leave 0"]);
    assert!(net.server.clients().is_empty());
}

#[test]
fn unresolvable_world_name_drops_the_client_with_0x29() {
    let mut net = Net::new(ServerTuning::default());
    let mut cfg = dave();
    cfg.resources = Some(vec!["Worlds\\Other.dat".into()]);
    net.add(1, cfg, Faults::clean(2));
    assert!(net.run_until(10_000, |n| matches!(
        n.client(0).state(),
        ClientState::Dropped(_)
    )));
    assert_eq!(net.client(0).state(), ClientState::Dropped(0x29));
    net.run_until(net.now + 600, |_| false);
    assert_eq!(net.pipe_lines(), vec!["@join 0 7 99 0 Dave", "@leave 0"]);
}

#[test]
fn sky_table_and_compact_position_options() {
    let cfg = ServerTuning {
        send_sky_table: true,
        player_pos_form: dc_dserver::objects::PosForm::Compact,
        ..ServerTuning::default()
    };
    let mut net = Net::new(cfg.clone());
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    // DSV-45: no sky model in the world, no sky table
    assert_eq!(net.client(0).seen.sky_count, None);
    // with a sky model: the whole table of 30 slots, slot 0 = the sky model's object (a kind-2 record with the sky
    // flag words), the other slots free
    let mut cfg = cfg;
    cfg.game.start_order = StartOrder::Spec;
    let mut net = Net::with_world(cfg, world_with_sky());
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    let s = &net.client(0).seen;
    assert_eq!(s.sky_count, Some(30));
    let sky = s.sky_ids[0];
    assert!(s.sky_ids[1..].iter().all(|&i| i == 0xFFFF));
    assert_eq!(s.objects[&sky].kind, 2);
    assert_eq!(s.objects[&sky].flags, Some(0x0001));
    let b = s.sky_block.expect("block");
    assert_eq!(&b[..3], &[936.0, -64.0, -64.0]);
    assert_eq!(&b[6..9], &[968.0, -32.0, -32.0]);
    assert_eq!(
        s.objects[&s.view_object.expect("view object")].flags,
        None,
        "compact form sends no flag block"
    );
    assert!(s.errors.is_empty(), "{:?}", s.errors);
}

/// DSV-60: the server constants go out before `S2C_LOAD_WORLD` in the reference stack's order (world north before the step
/// height), world north `0.000000` by default; `-evars off` drops world north but keeps the step height; `-evar`
/// replaces a constant of the same name and is sent last.
// SPEC: UDP_SESSION_2009_A.md §4.5, §8.2; OBSERVED: capture 20261025_002236_tdm4 (REFERENCE order)
#[test]
fn server_constants_precede_the_world_load_in_the_reference_order() {
    let named = || {
        let mut w = world();
        // made-up names stand in for the client's (the real ones come from the user's CShell.dll)
        w.client_vars.step_height = Some(b"Stepper".to_vec());
        w.client_vars.world_north = Some(b"Northing".to_vec());
        w
    };
    let run = |cfg: ServerTuning| {
        let mut net = Net::with_world(cfg, named());
        net.add(1, dave(), Faults::clean(2));
        assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
        let s = &net.client(0).seen;
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert!(s.load_world.is_some());
        s.evars
            .iter()
            .map(|e| (e.1.clone(), e.2.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        run(ServerTuning::default()),
        vec![
            (b"Northing".to_vec(), b"0.000000".to_vec()),
            (b"Stepper".to_vec(), b"30.000000".to_vec())
        ]
    );
    let off = ServerTuning {
        evars: dc_dserver::evars::EvarMode::Off,
        ..ServerTuning::default()
    };
    assert_eq!(run(off), vec![(b"Stepper".to_vec(), b"30.000000".to_vec())]);
    let mine = ServerTuning {
        world_north: Some(1.25),
        extra_evars: vec![("stepper".into(), "12".into())],
        ..ServerTuning::default()
    };
    assert_eq!(
        run(mine),
        vec![
            (b"Northing".to_vec(), b"1.250000".to_vec()),
            (b"stepper".to_vec(), b"12".to_vec())
        ]
    );
}

/// DSV-53: the step-height variable goes out once per client before `S2C_LOAD_WORLD`, value text `30.000000` by
/// default, the configured value with `-stepheight`, nothing with `-stepheight off` or when the ROM gave no name.
// SPEC: UDP_SESSION_2009_A.md §4.5, §8.2; OBSERVED: capture 20261025_002236_tdm4 (REFERENCE form)
#[test]
fn step_height_variable_precedes_the_world_load() {
    let named = || {
        let mut w = world();
        // a made-up name stands in for the client's (the real one comes from the user's CShell.dll)
        w.client_vars.step_height = Some(b"Stepper".to_vec());
        w
    };
    let run = |cfg: ServerTuning, w: WorldInfo| {
        let mut net = Net::with_world(cfg, w);
        net.add(1, dave(), Faults::clean(2));
        assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
        net
    };
    let net = run(ServerTuning::default(), named());
    let s = &net.client(0).seen;
    let got: Vec<_> = s.evars.iter().map(|e| (e.1.clone(), e.2.clone())).collect();
    assert_eq!(got, vec![(b"Stepper".to_vec(), b"30.000000".to_vec())]);
    assert!(s.load_world.is_some());
    let sent = net
        .logs
        .iter()
        .position(|l| l.contains("server variables sent"));
    let load = net.logs.iter().position(|l| l.contains(": load world"));
    assert!(sent.is_some() && sent < load, "{:?}", net.logs);
    assert!(s.errors.is_empty(), "{:?}", s.errors);

    let cfg = ServerTuning {
        step_height: Some(18.5),
        ..ServerTuning::default()
    };
    let net = run(cfg, named());
    assert_eq!(net.client(0).seen.evars.len(), 1);
    assert_eq!(net.client(0).seen.evars[0].2, b"18.500000");

    let cfg = ServerTuning {
        step_height: None,
        ..ServerTuning::default()
    };
    assert!(run(cfg, named()).client(0).seen.evars.is_empty());
    // a world loaded from bytes has no ROM names: nothing is sent
    assert!(run(ServerTuning::default(), world())
        .client(0)
        .seen
        .evars
        .is_empty());
}

#[test]
fn hello_deadline_drops_a_client_that_never_says_hello() {
    // DSV-657: a raw link that joins and keeps the link alive but never sends C2S_HELLO is gone after 10 s (the
    // 120 s start-up deadline is for clients past their hello)
    let mut server: Server<u32> = Server::new(ServerTuning::default(), world(), 0);
    let join = OobDatagram::JoinAsk {
        app_id: TODO_GAME_APP_ID_GUID_LAYOUT,
        rate_bps: 35_840,
    }
    .encode();
    server.handle_datagram(0, 9, &join);
    let mut link = Link::new(0, LinkConfig::default());
    let mut closed = None;
    let mut now = 0u32;
    while now < 130_000 && closed.is_none() {
        let mut to_server = Vec::new();
        for e in link.poll(now) {
            match e {
                LinkEvent::Transmit(d) => to_server.push(d),
                LinkEvent::Closed { code, .. } => closed = Some((now, code)),
                LinkEvent::Message { .. } => {}
            }
        }
        for d in to_server {
            server.handle_datagram(now, 9, &d);
        }
        for o in server.poll(now) {
            if let Output::Transmit { bytes, .. } = o {
                if !dc_wire::udp::is_oob_datagram(&bytes) {
                    link.handle_datagram(now, &bytes);
                }
            }
        }
        now += 10;
    }
    let (t, code) = closed.expect("dropped");
    assert_eq!(code, 3);
    assert!((10_000..=10_200).contains(&t), "dropped at {t}");
}

/// A join datagram that never says anything else (a spoofed source).
fn raw_join(server: &mut Server<u32>, now: u32, from: u32) {
    let join = OobDatagram::JoinAsk {
        app_id: TODO_GAME_APP_ID_GUID_LAYOUT,
        rate_bps: 35_840,
    }
    .encode();
    server.handle_datagram(now, from, &join);
    let _ = server.poll(now);
}

#[test]
fn one_host_holds_at_most_sixteen_links_without_a_hello() {
    // DSV-653: same host = same hundred
    let mut server: Server<u32> = Server::new(ServerTuning::default(), world(), 0);
    server.set_address_moves(|a, b| a / 100 == b / 100);
    for (i, from) in (101..=118).enumerate() {
        raw_join(&mut server, i as u32, from);
    }
    assert_eq!(
        server.clients().len(),
        16,
        "the 17th and 18th join of host 1xx closed"
    );
    raw_join(&mut server, 30, 201);
    assert_eq!(server.clients().len(), 17, "another host is not affected");
}

#[test]
fn a_whole_room_joins_at_once_from_one_host() {
    // DSV-653: a bot swarm behind one address, 20 seats, every bot joins in the same millisecond
    let mut net = Net::new(ServerTuning {
        max_players: Some(20),
        accept_unlisted: true,
        ..ServerTuning::default()
    });
    net.server.set_address_moves(|a, b| a / 100 == b / 100);
    for i in 0..20u32 {
        net.add(
            101 + i,
            SynthConfig::named(&format!("Bot{i}")),
            Faults::clean(2),
        );
    }
    let n = net.peers.len();
    assert!(net.run_until(30_000, |net| (0..n)
        .all(|i| net.client(i).state() == ClientState::InWorld)));
}

#[test]
fn a_full_server_gives_a_spoofed_links_slot_to_a_real_player() {
    // DSV-654 / NET-29: one seat, held by a join that never says hello; a real client after the 2 s grace gets in
    let mut net = Net::new(ServerTuning {
        max_players: Some(1),
        ..ServerTuning::default()
    });
    raw_join(&mut net.server, 0, 99);
    assert_eq!(net.server.clients().len(), 1);
    net.now = 2500;
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(25_000, |n| n.client(0).state() == ClientState::InWorld));
    assert!(
        !net.server.clients().contains_key(&99),
        "the spoofed link was evicted"
    );
    // the real player's link is proven: a further spoofed join is refused, not given the seat
    let now = net.now + 5000;
    raw_join(&mut net.server, now, 98);
    assert!(net.server.clients().contains_key(&1) && !net.server.clients().contains_key(&98));
}

#[test]
fn a_link_without_a_hello_is_no_known_peer() {
    // DSV-655
    let mut net = secret_net();
    raw_join(&mut net.server, 0, 99);
    assert!(!net.server.knows_peer(&99));
    net.add(1, named("Dave", Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    assert!(net.server.knows_peer(&1));
    assert!(!net.server.knows_peer(&99));
}

#[test]
fn idle_limit_counts_from_the_last_client() {
    let cfg = ServerTuning {
        idle_ms: Some(5_000),
        ..ServerTuning::default()
    };
    let mut net = Net::new(cfg);
    assert!(!net.server.idle_expired(4_999));
    assert!(net.server.idle_expired(5_000));
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    assert!(!net.server.idle_expired(net.now));
    net.peers[0].client.quit();
    assert!(net.run_until(net.now + 3000, |n| n.server.clients().is_empty()));
    let gone = net.now;
    assert!(!net.server.idle_expired(gone + 4_000));
    assert!(net.server.idle_expired(gone + 5_000));
}

/// DSV-21: the stream time restarts with the first client, so a long-running server keeps f32 resolution.
#[test]
fn the_stream_time_restarts_with_the_first_client_of_an_empty_server() {
    let mut net = Net::new(ServerTuning::default());
    net.now = 3 * 86_400_000;
    net.add(1, dave(), Faults::clean(2));
    let t0 = net.now;
    assert!(net.run_until(t0 + 30_000, |n| n.client(0).state() == ClientState::InWorld));
    let first = net.client(0).seen.server_times.first().map(|t| t.1);
    assert!(first.is_some_and(|t| t < 30.0), "{first:?}");
}

/// CD-64: remote console commands count only for accounts the lobby marked `mod=1`; dc-dserver has no console
/// commands, so the moderator's command is logged as received and not run, the other one is ignored.
#[test]
fn console_commands_only_for_moderators() {
    let mut net = Net::new(ServerTuning::default());
    assert!(net.server.handle_line(0, "user 8 4 55 1 0 Big_Erin mod=1"));
    net.add(1, dave(), Faults::clean(2));
    net.add(2, SynthConfig::named("Big Erin"), Faults::clean(2));
    assert!(
        net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld
            && n.client(1).state() == ClientState::InWorld)
    );
    net.peers[0].client.send_console("sv_cheats 1");
    net.peers[1].client.send_console("sv_cheats 1");
    net.run_until(net.now + 500, |_| false);
    let mod_lines = net
        .logs
        .iter()
        .filter(|l| l.contains("from moderator"))
        .count();
    let ignored = net
        .logs
        .iter()
        .filter(|l| l.contains("ignored (not a moderator)"))
        .count();
    assert_eq!((mod_lines, ignored), (1, 1), "{:?}", net.logs);
}

/// The default (reference) start order, O2 / O6 / O9: shell 0x90 [1] and 0x88 [2] ahead of `S2C_VERSION` in the
/// first reliable block; the identify request, endpoint and settings before the view object; the in-world test
/// passes on the early 0x88 and the 0x67 state 1 of the spawn set; after 0x87 shell 0x90 [0] and 0x88 [3].
#[test]
fn reference_start_order_on_a_clean_channel() {
    let mut net = Net::reference(ServerTuning::default());
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    net.run_until(net.now + 1000, |_| false);
    let seen = &net.client(0).seen;
    assert!(seen.errors.is_empty(), "{:?}", seen.errors);
    let ids: Vec<u8> = seen.messages.iter().map(|m| m.1).collect();
    assert_eq!(&ids[..5], &[13, 13, 4, 12, 0x32], "{ids:?}");
    let shells: Vec<u8> = seen.shell_ids.iter().map(|s| s.1).collect();
    assert_eq!(&shells[..2], &[0x90, 0x88]);
    let first = |id: u8| ids.iter().position(|&x| x == id).unwrap_or(usize::MAX);
    // DC-DEFAULT: DSV-23 (amendment, proposed) the view object before the group with the own create
    // OBSERVED: capture 20261025_065016_tdm6 c6 seq 714 < 743 (REFERENCE)
    assert!(first(7) < first(8), "view object before the create");
    let pos = |id: u8| shells.iter().position(|&x| x == id).unwrap_or(usize::MAX);
    assert!(pos(0x72) < pos(0x9e) && pos(0x9e) < pos(0x7d) && pos(0x7d) < pos(0x6b));
    assert!(pos(0x6b) < pos(0x67) && pos(0x67) < pos(0x45));
    assert_eq!(
        seen.life_states.iter().map(|s| s.1).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(seen.flag90, vec![true, false]);
    assert_eq!(seen.switch_state, Some(3));
    assert_eq!(seen.endpoint, Some(0), "mode class 0 of the room line");
    // a second teleport after UP_WORLD_READY with the next sequence
    assert_eq!(
        seen.teleports.iter().map(|t| t.1).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(seen.updates_sent > 5);
    let (_, sc) = net.server.clients().iter().next().expect("client");
    assert!(sc.reached(Step::FirstUpdate).is_some());
}

// DC-DEFAULT: GMD-2 (clarification) the room line the lobby writes for a ROM TDM room (no mode-file limit) carries
// the class fallback `time=600`; in the default (reference) start order every player's 0x44 and the first spawn's 0x45
// carry the seconds left, so the 2009 client shows and runs its round clock (S2C §2.1, §2.1.1 step 1; DISASM
// cshell+0x49a40 case 0x45 hides the clock only for 0). REFERENCE contrast: round_secs 0 in every reference 0x44 / 0x45
// (tdm1 c5 t 139.906, tdm6 c5 t 167.343 / c6 t 173.437)
#[test]
fn fallback_round_time_in_the_reference_start_order() {
    let mut net = Net::with_room(
        ServerTuning::default(),
        world(),
        "room key=TDM type=1 goal=100 ff=0 arms=0 time=600 train=0",
    );
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    let joined = net.now;
    net.run_until(joined + 3_000, |_| false);
    net.add(2, SynthConfig::named("Big_Erin"), Faults::clean(3));
    assert!(net.run_until(joined + 25_000, |n| n.client(1).state()
        == ClientState::InWorld));
    net.run_until(net.now + 500, |_| false);
    let a = &net.client(0).seen;
    assert!(a.errors.is_empty(), "{:?}", a.errors);
    assert_eq!(a.round_begin, Some(600));
    assert_eq!(a.match_begin, Some((600, false)));
    let b = &net.client(1).seen;
    assert!(b.errors.is_empty(), "{:?}", b.errors);
    let left = b.round_begin.expect("0x44 to the later player");
    assert!((570..600).contains(&left), "time left {left}");
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("round started (mode class 1, goal 100, 600 s)")));
}

#[test]
fn reconnect_of_a_connected_player_replaces_the_older_link() {
    let mut net = Net::new(ServerTuning::default());
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    net.add(2, dave(), Faults::clean(2));
    assert!(net.run_until(net.now + 20_000, |n| n.client(1).state()
        == ClientState::InWorld));
    net.run_until(net.now + 600, |_| false);
    assert_eq!(net.client(0).state(), ClientState::Dropped(3));
    assert_eq!(
        net.pipe_lines(),
        vec!["@join 0 7 99 0 Dave", "@leave 0", "@join 1 7 99 0 Dave"]
    );
}

/// DSV-40 (proposed): a world with a global light object gets `S2C_DIR_LIGHT` (id 22) once per client after its world
/// load, with the other world-bound tables (after the preload messages, before the view object); `-globallight off`
/// and a world without the object send nothing. The synthetic world cannot hold the class (it is named by hash), so
/// the world's light is set on the loaded world; the object parse itself is the ROM-gated test of `rom_world.rs`.
#[test]
fn the_global_light_follows_the_world_load_once() {
    use dc_dserver::worldlight::{GlobalLight, LightMode};
    let light = GlobalLight {
        dir: [0.0, -0.6, 0.8],
        colour: [200.0, 190.0, 180.0],
        extra: 0.25,
    };
    let run = |mode: LightMode, lit: bool| {
        let mut w = world_with_sky();
        w.global_light = lit.then_some(light);
        let cfg = ServerTuning {
            global_light: mode,
            send_sky_table: true,
            ..ServerTuning::default()
        };
        let mut net = Net::with_world(cfg, w);
        net.add(1, dave(), Faults::clean(2));
        assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
        let end = net.now + 300;
        net.run_until(end, |_| false);
        net
    };
    let net = run(LightMode::World, true);
    let s = &net.client(0).seen;
    let ids: Vec<u8> = s.messages.iter().map(|m| m.1).collect();
    assert_eq!(ids.iter().filter(|&&i| i == 22).count(), 1, "once");
    let (_, dir, col, extra) = s.dir_light.expect("received");
    assert_eq!((dir, col, extra), (light.dir, light.colour, light.extra));
    let at = |id: u8| ids.iter().position(|&i| i == id).unwrap();
    assert!(at(16) < at(22), "after the sky table");
    assert!(at(22) < at(7), "before the view object");
    assert!(s.errors.is_empty(), "{:?}", s.errors);
    for (mode, lit) in [(LightMode::Off, true), (LightMode::World, false)] {
        let net = run(mode, lit);
        assert!(
            net.client(0).seen.dir_light.is_none(),
            "{mode:?} lit {lit}: no message"
        );
        assert!(!net.client(0).seen.messages.iter().any(|m| m.1 == 22));
    }
}

/// DSV-56: the switched-on world sounds start with the world load on every client, as tracked sounds (kind 1,
/// `tmask` 3, `dmask` 7) whose file is in the session file table and whose track id is not an object of the
/// client; a switched-off sound is not started; `world_sounds` off sends none.
#[test]
fn world_sounds_start_with_the_world_load_on_every_client() {
    let sound = |name: &[u8], on: bool| dc_dserver::world::SoundInfo {
        name: name.to_vec(),
        pos: [10.0, 20.0, 30.0],
        file: "SND\\Amb\\Wind.wav".into(),
        priority: 0,
        inner: 800.0,
        outer: 1280.0,
        volume: 50,
        pitch: 1.0,
        ambient: true,
        looped: true,
        start_on: on,
        attached: false,
    };
    for enabled in [true, false] {
        let mut w = world();
        w.sounds = vec![sound(b"S1", true), sound(b"S2", false)];
        let cfg = ServerTuning {
            world_sounds: enabled,
            ..ServerTuning::default()
        };
        let mut net = Net::with_world(cfg, w);
        net.add(1, dave(), Faults::clean(3));
        net.add(2, SynthConfig::named("Big Erin"), Faults::clean(7));
        assert!(net.run_until(20_000, |n| {
            n.client(0).state() == ClientState::InWorld
                && n.client(1).state() == ClientState::InWorld
        }));
        let end = net.now + 300;
        net.run_until(end, |_| false);
        for i in 0..2 {
            let s = &net.client(i).seen;
            assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
            if !enabled {
                assert!(s.sounds.is_empty());
                // DSV-65: no world sound, no load-sound-file list
                assert!(s.preload_sounds.is_empty());
                continue;
            }
            assert_eq!(s.sounds.len(), 1, "client {i}: one started sound");
            let (track, snd) = s.sounds.iter().next().unwrap();
            assert!(!s.objects.contains_key(track), "a sound id is no object");
            assert_eq!(
                (snd.tmask, snd.dmask, snd.sflags, snd.prio, snd.outer, snd.ratio),
                (3, 7, 0x175, 0, 1280, 159)
            );
            assert_eq!(snd.volume, Some(50));
            let (_, name) = &s.files[&snd.sfile];
            assert_eq!(name, &b"SND\\AMB\\WIND.WAV".to_vec());
            // DSV-65: the world sound's file as the preload sub 5 list, after the end mark (the reference stack's order)
            assert_eq!(s.preload_sounds, vec![vec![snd.sfile]]);
            assert_eq!(s.preload_subs, vec![0, 1, 5]);
        }
        assert_eq!(
            net.logs.iter().any(|l| l.contains(", 1 world sounds")),
            enabled,
            "{:?}",
            net.logs
                .iter()
                .filter(|l| l.contains("world objects"))
                .collect::<Vec<_>>()
        );
    }
}

/// DSV-97: at the world's first `UP_WORLD_READY` every other link gets shell 0x90 [0] and 0x88 [3], also a link that
/// is still loading; the later loader then holds both before its spawn set and its `GS_MATCH_BEGIN`, and is not sent
/// them again after its own 0x87. The first loader gets the pair after its 0x87 as before, so it takes its 0x45 with
/// the flag still set (the reference stack's first loaders did too: tdm1 c5 0x45 t 139.906, 0x90 [0] t 140.0). Both take
/// the 0x45 in player state 1, where the 2009 client plays its round-start voice.
// OBSERVED: capture 20261025_065016_tdm6 c6 (0x90 [0] / 0x88 [3] at t 167.406 after c5's 0x87 at
// 167.359; c6's own 0x67 [1] and 0x45 at 173.437), 20261025_153705_rust7 c5 (ours before: the pair at t 114.953 after
// the 0x45 at 114.859)
// DISASM: cshell+0x49f00..0x49f7b (the voice gate)
// DC-DEFAULT: DSV-97 (proposed)
#[test]
fn a_late_loader_holds_the_world_start_pair_before_its_round_start() {
    let mut net = Net::reference(ServerTuning::default());
    net.add(1, dave(), Faults::clean(2));
    let mut late = SynthConfig::named("Big_Erin");
    late.load_ms = 4_000;
    net.add(2, late, Faults::clean(3));
    assert!(
        net.run_until(30_000, |n| n.client(0).state() == ClientState::InWorld
            && n.client(1).state() == ClientState::InWorld)
    );
    net.run_until(net.now + 1_000, |_| false);
    let first = &net.client(0).seen;
    let second = &net.client(1).seen;
    for s in [first, second] {
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.flag90, vec![true, false], "one pair per link");
        assert_eq!(s.switch_states, vec![2, 3]);
        assert_eq!(s.match_begins.len(), 1);
        assert_eq!(s.round_start_cues.len(), 1, "the voice once, in state 1");
        assert!(
            !s.round_start_cues[0].1,
            "the round-start key, not the waiting one"
        );
    }
    let ready = first.world_ready_sent.expect("first 0x87");
    let at = |s: &dc_dserver::synth_client::Observed, id: u8, nth: usize| {
        s.shell_ids
            .iter()
            .filter(|x| x.1 == id)
            .nth(nth)
            .map(|x| x.0)
            .unwrap_or(u32::MAX)
    };
    // the first loader: its 0x45 before the pair (after its own 0x87)
    assert!(
        first.round_start_cues[0].2,
        "first loader: 0x45 before 0x90 [0]"
    );
    assert!(at(first, 0x90, 1) > ready);
    // the later loader: the pair (sent at the first 0x87, taken when its load ends) before its own spawn set
    let pair = at(second, 0x90, 1);
    assert!(pair > ready, "pair at {pair}, first 0x87 at {ready}");
    let spawn = second.life_states.first().map(|x| x.0).expect("state 1");
    let order: Vec<u8> = second
        .shell_ids
        .iter()
        .map(|x| x.1)
        .filter(|id| matches!(id, 0x90 | 0x88 | 0x67 | 0x45))
        .collect();
    assert_eq!(
        order,
        vec![0x90, 0x88, 0x90, 0x88, 0x67, 0x45],
        "{order:x?}"
    );
    assert!(pair <= spawn && at(second, 0x88, 1) <= spawn);
    assert!(
        !second.round_start_cues[0].2,
        "later loader: 0x90 [0] before its 0x45"
    );
    assert!(second.world_ready_sent.expect("second 0x87") > spawn);
}

// ---- LG-327: the seat ticket ----

const SEAT: [u8; 16] = [0x5a; 16];
const WRONG_SEAT: [u8; 16] = [0x5b; 16];

/// A server whose roster holds Tess with a ticket.
fn ticket_net() -> Net {
    let mut net = Net::new(ServerTuning::default());
    let line = format!(
        "user 9 5 77 0 0 Tess ticket={}",
        dc_wire::seatticket::to_hex(&SEAT)
    );
    assert!(net.server.handle_line(0, &line));
    net
}

fn tess(ticket: Option<[u8; 16]>) -> SynthConfig {
    let mut c = SynthConfig::named("Tess");
    c.seat_ticket = ticket;
    c
}

fn refused(c: &SynthClient) -> bool {
    !matches!(
        c.state(),
        ClientState::Joining
            | ClientState::Linked
            | ClientState::LoadingWorld
            | ClientState::WorldLoaded
            | ClientState::InWorld
    )
}

fn joins_of(net: &Net, uid: u32) -> usize {
    let want = format!(" {uid} ");
    net.pipe_lines()
        .iter()
        .filter(|l| l.starts_with("@join ") && l.contains(&want))
        .count()
}

#[test]
fn the_right_ticket_is_seated_and_no_line_shows_it() {
    let mut net = ticket_net();
    net.add(1, tess(Some(SEAT)), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    assert_eq!(joins_of(&net, 9), 1);
    let hex = dc_wire::seatticket::to_hex(&SEAT);
    assert!(
        !net.logs
            .iter()
            .chain(net.pipe_lines().iter())
            .any(|l| l.contains(&hex)),
        "the ticket is never logged"
    );
}

#[test]
fn a_wrong_or_missing_ticket_is_refused_and_only_a_missing_one_is_told() {
    let mut net = ticket_net();
    net.add(1, tess(Some(WRONG_SEAT)), Faults::clean(2));
    assert!(
        net.run_until(20_000, |n| refused(n.client(0))),
        "{:?}",
        net.logs
    );
    assert_eq!(
        net.client(0).state(),
        ClientState::Dropped(6),
        "the bye code of an unlisted name"
    );
    assert_eq!(joins_of(&net, 9), 0);
    assert!(!net
        .pipe_lines()
        .iter()
        .any(|l| l.starts_with("@seat_refused")));

    let mut net = ticket_net();
    net.add(1, tess(None), Faults::clean(2));
    assert!(
        net.run_until(20_000, |n| refused(n.client(0))),
        "{:?}",
        net.logs
    );
    assert_eq!(joins_of(&net, 9), 0);
    assert!(net
        .pipe_lines()
        .iter()
        .any(|l| l == "@seat_refused 9 noticket"));
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("seat of uid 9 refused (no ticket")));
}

/// A stranger with the name, on another address, does nothing to the player on the seat; the player's own new link
/// with the ticket (a reconnect, also from a new address) takes over.
#[test]
fn a_stranger_leaves_the_seated_player_alone_and_a_reconnect_with_the_ticket_takes_over() {
    let mut net = ticket_net();
    net.add(1, tess(Some(SEAT)), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    let at = net.now;
    net.add(2, tess(Some(WRONG_SEAT)), Faults::clean(2));
    net.add(3, tess(None), Faults::clean(2));
    assert!(net.run_until(at + 20_000, |n| refused(n.client(1))
        && refused(n.client(2))));
    net.run_until(net.now + 2000, |_| false);
    assert_eq!(
        net.client(0).state(),
        ClientState::InWorld,
        "{:?}",
        net.logs
    );
    assert_eq!(joins_of(&net, 9), 1);
    assert!(
        !net.pipe_lines()
            .iter()
            .any(|l| l.starts_with("@seat_refused")),
        "a seat in use: nobody is told of an old client"
    );
    let at = net.now;
    net.add(4, tess(Some(SEAT)), Faults::clean(2));
    assert!(
        net.run_until(at + 20_000, |n| n.client(3).state() == ClientState::InWorld),
        "{:?}",
        net.logs
    );
    assert_eq!(joins_of(&net, 9), 2);
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("reconnected: closing the older link")));
}

/// A `ticket=` that cannot be read seats nobody; a roster line without the token keeps the name-only check of a hand
/// written roster.
#[test]
fn an_unreadable_ticket_seats_nobody() {
    let mut net = Net::new(ServerTuning::default());
    assert!(net
        .server
        .handle_line(0, "user 9 5 77 0 0 Tess ticket=nonsense"));
    net.add(1, tess(Some(SEAT)), Faults::clean(2));
    assert!(net.run_until(20_000, |n| refused(n.client(0))));
    assert_eq!(joins_of(&net, 9), 0);
}

// ---- LOC-2: the join secret ----

const SECRET: [u8; 16] = [0x6c; 16];

/// The usual roster (Dave and Big_Erin, no tickets; Tess with her own ticket) with a join secret.
fn secret_net() -> Net {
    let cfg = ServerTuning {
        join_secret: Some(dc_dserver::session::JoinSecret(SECRET)),
        ..ServerTuning::default()
    };
    let mut net = Net::new(cfg);
    let line = format!(
        "user 9 5 77 0 0 Tess ticket={}",
        dc_wire::seatticket::to_hex(&SEAT)
    );
    assert!(net.server.handle_line(0, &line));
    net
}

fn named(name: &str, ticket: Option<[u8; 16]>) -> SynthConfig {
    let mut c = SynthConfig::named(name);
    c.seat_ticket = ticket;
    c
}

/// An unlisted name with the secret gets the next user id and the team with fewer players, and `@join`; neither the
/// log nor a pipe line shows the secret.
#[test]
fn the_join_secret_seats_an_unlisted_name_with_a_new_roster_entry() {
    let mut net = secret_net();
    net.add(1, named("Guest", Some(SECRET)), Faults::clean(2));
    assert!(
        net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld),
        "{:?}",
        net.logs
    );
    // LOC-6: teams are balanced over live links only: nobody is in yet, so team 0
    assert_eq!(net.pipe_lines(), vec!["@join 0 10 0 0 Guest"]);
    let hex = dc_wire::seatticket::to_hex(&SECRET);
    assert!(
        !net.logs
            .iter()
            .chain(net.pipe_lines().iter())
            .any(|l| l.contains(&hex)),
        "the secret is never logged"
    );
    // a second newcomer gets the next number
    net.add(2, named("Other", Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(net.now + 20_000, |n| n.client(1).state()
        == ClientState::InWorld));
    assert!(net
        .pipe_lines()
        .contains(&"@join 1 11 0 1 Other".to_string()));
}

/// A wrong secret or none is refused with the bye code of an unlisted name, and nothing reaches the pipe; the secret
/// opens no seat that has a ticket of its own.
#[test]
fn a_wrong_or_missing_join_secret_is_refused() {
    for t in [Some(WRONG_SEAT), None] {
        let mut net = secret_net();
        net.add(1, named("Guest", t), Faults::clean(2));
        assert!(net.run_until(20_000, |n| refused(n.client(0))));
        assert_eq!(net.client(0).state(), ClientState::Dropped(6));
        assert!(net.pipe_lines().is_empty(), "{:?}", net.pipe_lines());
    }
    let mut net = secret_net();
    net.add(1, tess(Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(20_000, |n| refused(n.client(0))));
    assert_eq!(joins_of(&net, 9), 0, "Tess's seat needs Tess's ticket");
    // a roster name without a ticket of its own needs the secret in such a room
    let mut net = secret_net();
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(20_000, |n| refused(n.client(0))));
    assert!(net.pipe_lines().is_empty());
    let mut net = secret_net();
    net.add(1, named("Dave", Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    assert_eq!(net.pipe_lines(), vec!["@join 0 7 99 0 Dave"]);
}

/// With a join secret, everyone who knows it shares it, so the secret cannot take a seat from a live link: a second
/// hello with a seated name is refused and the player keeps the seat (without the flag the second hello replaces the
/// older link, `reconnect_of_a_connected_player_replaces_the_older_link`). Once that link is gone, the name may come
/// back with the secret.
#[test]
fn the_join_secret_takes_no_seat_from_a_live_link() {
    let mut net = secret_net();
    net.add(1, named("Guest", Some(SECRET)), Faults::clean(2));
    net.add(2, named("Dave", Some(SECRET)), Faults::clean(2));
    assert!(
        net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld
            && n.client(1).state() == ClientState::InWorld)
    );
    let at = net.now;
    net.add(3, named("Guest", Some(SECRET)), Faults::clean(2));
    net.add(4, named("Dave", Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(at + 20_000, |n| refused(n.client(2))
        && refused(n.client(3))));
    net.run_until(net.now + 2000, |_| false);
    assert_eq!(
        net.client(0).state(),
        ClientState::InWorld,
        "{:?}",
        net.logs
    );
    assert_eq!(net.client(1).state(), ClientState::InWorld);
    assert_eq!(net.client(2).state(), ClientState::Dropped(6));
    assert_eq!(joins_of(&net, 10), 1);
    assert_eq!(joins_of(&net, 7), 1);
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("refused (in use by a live link,")));
    assert!(!net
        .logs
        .iter()
        .any(|l| l.contains("reconnected: closing the older link")));
    // the player leaves; the name comes back with the secret and keeps its number
    net.peers[0].client.quit();
    let at = net.now;
    assert!(net.run_until(at + 30_000, |n| n
        .pipe_lines()
        .iter()
        .any(|l| l == "@leave 0")));
    net.add(5, named("Guest", Some(SECRET)), Faults::clean(2));
    let i = net.peers.len() - 1;
    assert!(net.run_until(net.now + 20_000, |n| n.client(i).state()
        == ClientState::InWorld));
    assert_eq!(joins_of(&net, 10), 2);
}

/// LOC-5: a seat taken with the secret whose link went silent (a crashed client, a NAT that moved it to a new port)
/// is free for the secret again after `SECRET_REJOIN_SILENCE_MS`, long before the 20 s silent drop; before that the
/// same hello is refused. The older link is closed as a reconnect and the seat keeps its number.
#[test]
fn a_silent_secret_seat_is_free_for_the_secret_after_three_seconds() {
    use dc_dserver::session::SECRET_REJOIN_SILENCE_MS;
    let mut net = secret_net();
    // the rejoin comes from the same IP address (test addresses: the IP in the high 16 bits)
    net.server.set_address_moves(same_nat);
    net.add(1, named("Guest", Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    net.peers[0].muted = true;
    let gone = net.now;
    // too early: the link still holds the seat
    net.add(2, named("Guest", Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(gone + 20_000, |n| refused(n.client(1))));
    assert!(
        net.now < gone + SECRET_REJOIN_SILENCE_MS + 1000,
        "refused at once, not after the drop"
    );
    // after the silence the secret takes the seat back
    net.run_until(gone + SECRET_REJOIN_SILENCE_MS + 100, |_| false);
    let at = net.now;
    net.add(3, named("Guest", Some(SECRET)), Faults::clean(2));
    assert!(
        net.run_until(at + 10_000, |n| n.client(2).state() == ClientState::InWorld),
        "{:?}",
        net.logs
    );
    assert!(
        net.now < gone + dc_dserver::session::SILENT_DROP_MS,
        "back in before the silent drop"
    );
    assert_eq!(joins_of(&net, 10), 2, "the same seat");
    assert!(net
        .logs
        .iter()
        .any(|l| l.contains("reconnected: closing the older link")));
}

/// LOC-5 (amended): a hello with the secret from ANOTHER IP address does not take a silent seat after 3 s: the seat
/// stays with its link until the silent drop (20 s) closes it; only then may the other address take the name.
#[test]
fn another_address_waits_for_the_silent_drop_before_taking_a_secret_seat() {
    use dc_dserver::session::{SECRET_REJOIN_SILENCE_MS, SILENT_DROP_MS};
    let mut net = secret_net();
    net.server.set_address_moves(same_nat);
    net.add(0x0001_0001, named("Guest", Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    net.peers[0].muted = true;
    let gone = net.now;
    net.run_until(gone + SECRET_REJOIN_SILENCE_MS + 500, |_| false);
    // another IP after 3.5 s of silence: refused, the seat is still the silent player's
    net.add(0x0002_0001, named("Guest", Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(net.now + 10_000, |n| refused(n.client(1))));
    assert!(net.now < gone + SILENT_DROP_MS, "refused before the drop");
    assert_eq!(joins_of(&net, 10), 1);
    // after the silent drop the name is free for any holder of the secret
    net.run_until(gone + SILENT_DROP_MS + 1000, |_| false);
    net.add(0x0002_0002, named("Guest", Some(SECRET)), Faults::clean(2));
    assert!(
        net.run_until(net.now + 20_000, |n| n.client(2).state()
            == ClientState::InWorld),
        "{:?}",
        net.logs
    );
    assert_eq!(joins_of(&net, 10), 2);
}

/// LOC-6 (amended): a name with a control character is matched in its stored form, so its rejoin finds its own entry:
/// a second live hello with that name is refused (not seated as a duplicate), and after the link is gone the name comes
/// back on the same user id.
#[test]
fn a_name_with_a_control_character_rejoins_its_own_entry() {
    let mut net = secret_net();
    net.add(1, named("Ev\u{7}e", Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    let base = net.server.roster().len();
    net.add(2, named("Ev\u{7}e", Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(net.now + 10_000, |n| refused(n.client(1))));
    assert_eq!(net.server.roster().len(), base, "no duplicate entry");
    assert_eq!(net.client(0).state(), ClientState::InWorld);
    net.peers[0].client.quit();
    assert!(net.run_until(net.now + 5_000, |n| matches!(
        n.client(0).state(),
        ClientState::Dropped(_)
    )));
    net.run_until(net.now + 600, |_| false);
    net.add(3, named("Ev\u{7}e", Some(SECRET)), Faults::clean(2));
    assert!(net.run_until(net.now + 20_000, |n| n.client(2).state()
        == ClientState::InWorld));
    assert_eq!(net.server.roster().len(), base);
    assert_eq!(joins_of(&net, 10), 2, "the same user id");
}

/// The join secret's refusals share LG-327's log budget: the first 20 are logged one by one, then every hundredth.
#[test]
fn the_join_secret_refusals_are_logged_within_the_budget() {
    let mut net = secret_net();
    for a in 0..30 {
        net.add(10 + a, dave(), Faults::clean(2));
    }
    assert!(net.run_until(30_000, |n| (0..30).all(|i| refused(n.client(i)))));
    let logged = net
        .logs
        .iter()
        .filter(|l| l.contains("seat of uid 7 refused"))
        .count();
    assert_eq!(logged, 20, "{:?}", net.logs);
}

/// LOC-6: the join secret makes at most 2 x max players roster entries; past that a new name takes the entry of a
/// player whose link is gone (the lowest such user id), so a stream of new names cannot grow the roster. Control
/// characters leave the name.
#[test]
fn secret_entries_are_capped_and_reused_once_their_links_are_gone() {
    let cfg = ServerTuning {
        join_secret: Some(dc_dserver::session::JoinSecret(SECRET)),
        max_players: Some(2),
        ..ServerTuning::default()
    };
    let mut net = Net::new(cfg);
    let base = net.server.roster().len();
    // four names, one after the other, each leaves
    for (i, name) in ["Ann", "Bob", "Cy", "Di"].iter().enumerate() {
        net.add(1 + i as u32, named(name, Some(SECRET)), Faults::clean(2));
        assert!(net.run_until(net.now + 20_000, |n| n.client(i).state()
            == ClientState::InWorld));
        net.peers[i].client.quit();
        assert!(net.run_until(net.now + 5_000, |n| matches!(
            n.client(i).state(),
            ClientState::Dropped(_)
        )));
        net.run_until(net.now + 600, |_| false);
    }
    assert_eq!(net.server.roster().len(), base + 4);
    // the fifth name reuses Ann's entry (uid 9), the roster does not grow
    net.add(10, named("E\u{7}ve", Some(SECRET)), Faults::clean(2));
    assert!(
        net.run_until(net.now + 20_000, |n| n.client(4).state()
            == ClientState::InWorld),
        "{:?}",
        net.logs
    );
    assert_eq!(net.server.roster().len(), base + 4);
    assert!(
        net.pipe_lines()
            .iter()
            .any(|l| l.starts_with("@join ") && l.ends_with(" 9 0 0 Eve")),
        "{:?}",
        net.pipe_lines()
    );
}

/// `-joinsecret` takes 32 hex digits and nothing else; the parsed arguments never print it.
#[test]
fn the_join_secret_flag() {
    let hex = dc_wire::seatticket::to_hex(&SECRET);
    let p = |s: &str| dc_dserver::cli::parse(s.split_whitespace().map(String::from));
    let a = p(&format!("-world w -joinsecret {hex}")).unwrap();
    assert_eq!(
        a.server.join_secret,
        Some(dc_dserver::session::JoinSecret(SECRET))
    );
    assert!(!format!("{a:?}").contains(&hex));
    let up = p(&format!("-world w -joinsecret {}", hex.to_uppercase())).unwrap();
    assert_eq!(up.server.join_secret, a.server.join_secret);
    assert!(p("-world w -joinsecret abc").is_err());
    assert!(
        p(&format!("-world w -joinsecret {}", "0".repeat(32))).is_err(),
        "LOC-7: all zeros"
    );
    assert!(p("-world w -joinsecret").is_err());
    assert!(p(&format!("-world w -joinsecret {}", "zz".repeat(16))).is_err());
    assert_eq!(p("-world w").unwrap().server.join_secret, None);
    // LOC-11: the environment gives it when the flag is absent; the flag wins; a bad value fails without quoting it
    let pe = |s: &str, v: &str| {
        dc_dserver::cli::parse_with_env(
            s.split_whitespace().map(String::from),
            &[(dc_dserver::cli::JOIN_SECRET_ENV.to_string(), v.to_string())],
        )
    };
    assert_eq!(
        pe("-world w", &hex).unwrap().server.join_secret,
        Some(dc_dserver::session::JoinSecret(SECRET))
    );
    let other = dc_wire::seatticket::to_hex(&[0x11; 16]);
    assert_eq!(
        pe(&format!("-world w -joinsecret {other}"), &hex)
            .unwrap()
            .server
            .join_secret,
        Some(dc_dserver::session::JoinSecret([0x11; 16])),
        "the flag wins"
    );
    assert_eq!(pe("-world w", "  ").unwrap().server.join_secret, None);
    let e = pe("-world w", "nothex").unwrap_err();
    assert!(e.contains("DC_JOIN_SECRET") && !e.contains("nothex"), "{e}");
    assert!(pe("-world w", &"0".repeat(32)).is_err());
}

/// A client in the world that goes silent (its process died: no bye, no datagram) is dropped after
/// `silent_drop_ms` through the normal leave path; a shorter silence (a stall the client recovers from) is not.
// DC-DEFAULT: DSV-490 (proposed, project decision) 20 s, bye code 3
#[test]
fn a_silent_client_is_dropped_after_twenty_seconds_through_the_leave_path() {
    let mut net = Net::new(ServerTuning::default());
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    net.run_until(net.now + 2_000, |_| false);
    // a 15 s stall: the client comes back and stays
    let peer = net.peers.pop().expect("peer");
    net.run_until(net.now + 15_000, |_| false);
    net.peers.push(peer);
    net.run_until(net.now + 5_000, |_| false);
    assert_eq!(net.client(0).state(), ClientState::InWorld);
    assert_eq!(net.server.clients().len(), 1);
    assert!(!net.pipe_lines().iter().any(|l| l.starts_with("@leave")));
    // now it is gone for good
    net.peers.pop();
    let silent_from = net.now;
    assert!(net.run_until(silent_from + 25_000, |n| n
        .pipe_lines()
        .iter()
        .any(|l| l == "@leave 0")));
    let waited = net.now - silent_from;
    assert!(
        (19_900..=20_500).contains(&waited),
        "dropped {waited} ms after the last datagram"
    );
    net.run_until(net.now + 600, |_| false);
    assert!(net.server.clients().is_empty());
    assert!(
        net.logs
            .iter()
            .any(|l| l.contains("client 0: nothing received for")),
        "{:?}",
        net.logs
    );
}

#[test]
fn the_silent_drop_can_be_switched_off() {
    let cfg = ServerTuning {
        silent_drop_ms: None,
        ..ServerTuning::default()
    };
    let mut net = Net::new(cfg);
    net.add(1, dave(), Faults::clean(2));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    net.peers.pop();
    net.run_until(net.now + 60_000, |_| false);
    assert!(!net.pipe_lines().iter().any(|l| l.starts_with("@leave")));
    assert_eq!(net.server.clients().len(), 1);
}

/// A path that loses every datagram above `limit` bytes (both ways, 30 ms): 2 players join, 7 more join 4 s later;
/// all 9 get into the world and stay, and no datagram is lost for its size (NET-10, fix-big-datagrams).
fn nine_players_over_a_path_dropping_datagrams_over(limit: usize) {
    let cfg = ServerTuning {
        accept_unlisted: true,
        ..ServerTuning::default()
    };
    let mut net = Net::new(cfg);
    let add = |net: &mut Net, i: u32| {
        net.add(
            i,
            SynthConfig::named(&format!("Guest{i}")),
            Faults::clean(30),
        );
        let p = net.peers.last_mut().expect("peer");
        p.up.drop_over_bytes = Some(limit);
        p.down.drop_over_bytes = Some(limit);
    };
    add(&mut net, 1);
    add(&mut net, 2);
    net.run_until(4_000, |_| false);
    for i in 3..=9 {
        add(&mut net, i);
    }
    let all_in = |n: &Net| (0..9).all(|i| n.client(i).state() == ClientState::InWorld);
    assert!(
        net.run_until(30_000, all_in),
        "limit {limit}: not all in the world"
    );
    net.run_until(net.now + 10_000, |_| false);
    assert!(all_in(&net), "limit {limit}: a client left the world");
    assert_eq!(net.server.clients().len(), 9, "limit {limit}");
    let lost: u64 = net
        .peers
        .iter()
        .map(|p| p.up.oversize_lost + p.down.oversize_lost)
        .sum();
    assert_eq!(lost, 0, "limit {limit}: datagrams lost for their size");
}

#[test]
fn nine_players_join_over_a_path_dropping_datagrams_over_1200_bytes() {
    nine_players_over_a_path_dropping_datagrams_over(1200);
}

#[test]
fn nine_players_join_over_a_path_dropping_datagrams_over_576_bytes() {
    nine_players_over_a_path_dropping_datagrams_over(576);
}

// ---- NET-23 / NET-24: a link follows its client after a NAT rebinding, only with its move secret ----

/// Test addresses: the high 16 bits are the public IP, the low 16 the port.
fn same_nat(a: &u32, b: &u32) -> bool {
    a >> 16 == b >> 16
}

fn resealed(body: &[u8], trailer: Option<[u8; 8]>) -> Vec<u8> {
    let mut b = body.to_vec();
    if let Some(t) = trailer {
        b.extend_from_slice(&dc_wire::linkmove::trailer(&t));
    }
    let mut d = vec![dc_wire::udp::check8(&b)];
    d.extend_from_slice(&b);
    d
}

#[test]
fn a_rebinding_player_keeps_the_seat_and_a_stranger_behind_the_same_nat_cannot_take_it() {
    let dave_at = 0x0001_0001;
    let mut net = Net::reference(ServerTuning::default());
    net.server.set_address_moves(same_nat);
    net.add(dave_at, dave(), Faults::clean(5));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    // the server hands out the secret when the player enters the world; the client sends it from then on
    assert!(net.run_until(25_000, |n| n
        .client(0)
        .link()
        .is_some_and(|l| l.move_trailer().is_some())));
    let sends_it = |n: &Net| dc_net_engine::body::move_trailer(&n.peers[0].last_up).is_some();
    assert!(net.run_until(26_000, sends_it));

    // Dave lags (uplink cut 800 ms); a stranger behind the same NAT replays Dave's own datagrams without the
    // trailer, with a wrong secret, and sends byes with sequence numbers right in the window
    net.peers[0].muted = true;
    let t0 = net.now;
    assert!(!net.run_until(t0 + 400, |_| false));
    let real = net.peers[0].last_up.clone();
    let body = &real[1..real.len() - dc_wire::linkmove::TRAILER_LEN];
    let stranger = 0x0001_0777;
    let mut sent = 0;
    for k in 0..64u8 {
        let bye = dc_wire::udp::encode_link_datagram(&dc_wire::udp::LinkBody {
            ctl: Some(if k % 2 == 0 {
                dc_wire::udp::CtlBlock::Bye
            } else {
                dc_wire::udp::CtlBlock::ByeCode(5)
            }),
            rel: Some(dc_wire::udp::RelBlock {
                rel_seq: k as u16,
                tail_cont: false,
                entries: vec![],
            }),
            unrel: vec![],
        });
        for d in [resealed(body, None), resealed(body, Some([k; 8])), bye] {
            net.server.handle_datagram(net.now, stranger, &d);
            sent += 1;
        }
    }
    assert!(sent > 100);
    assert!(!net.run_until(t0 + 800, |_| false));
    net.peers[0].muted = false;
    assert!(!net.run_until(t0 + 2_000, |_| false));
    let moved = |n: &Net| n.logs.iter().any(|l| l.contains("address moved"));
    assert!(!moved(&net), "{:?}", net.logs);
    let refused = format!("link move to {stranger} refused");
    assert!(
        net.logs.iter().any(|l| l.contains(&refused)),
        "{:?}",
        net.logs
    );
    assert_eq!(net.client(0).state(), ClientState::InWorld);

    // the NAT gives Dave a new port: his link follows him, and the server talks to the new port
    net.peers[0].addr = 0x0001_0002;
    let before = net.client(0).link().unwrap().stats().datagrams_in;
    assert!(!net.run_until(net.now + 3_000, |_| false));
    assert!(
        net.logs
            .iter()
            .any(|l| l.contains("address moved from 65537 to 65538")),
        "{:?}",
        net.logs
    );
    assert_eq!(net.client(0).state(), ClientState::InWorld);
    assert!(net.client(0).link().unwrap().stats().datagrams_in > before + 20);
    assert!(net.client(0).link().unwrap().is_open());
}

#[test]
fn a_server_that_moves_no_links_sends_no_secret() {
    let mut net = Net::reference(ServerTuning::default());
    net.add(1, dave(), Faults::clean(5));
    assert!(net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld));
    assert!(!net.run_until(net.now + 2_000, |_| false));
    assert!(net.client(0).link().unwrap().move_trailer().is_none());
    assert!(dc_net_engine::body::move_trailer(&net.peers[0].last_up).is_none());
}
