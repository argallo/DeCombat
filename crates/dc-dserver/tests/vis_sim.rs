//! Visibility culling (VIS-1..12, proposed), deterministic: the server and synthetic clients over the simulated
//! channel with an injected clock, in a synthetic world with one thick wall in the plane x = 0 (z below 64; made-up
//! geometry, `visibility::synthetic_walled_world_bytes`). Synthetic clients walk along x, so they cross the wall.
//!
//! - an enemy behind the wall: its viewer gets the snap-away record (park point) and no stream entry;
//! - an enemy that steps into view: the snap back to its true position, then stream entries;
//! - the exposure window: an enemy that walks into cover is parked one window after its last clear test;
//! - teammates are streamed through the wall; culling off streams everyone.

use dc_dserver::game::GameTuning;
use dc_dserver::rom_tables::{RomTables, TeamNames};
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::visibility::{synthetic_walled_world_bytes, VisTuning};
use dc_dserver::world::{world_from_bytes, SpawnPoint, WorldInfo};
use dc_net_engine::sim::{Faults, SimChannel};

const RED: [f32; 3] = [-500.0, 0.0, -500.0];
const BLUE: [f32; 3] = [500.0, 0.0, -500.0];
const TICK_HZ: u32 = 30;

fn world(red: [f32; 3], blue: [f32; 3]) -> WorldInfo {
    let mut w =
        world_from_bytes(&synthetic_walled_world_bytes(), "Worlds\\Walled.dat", "W").unwrap();
    w.spawns = vec![
        SpawnPoint {
            pos: red,
            rot: [0.0; 3],
            team_filter: Some(b"Red".to_vec()),
        },
        SpawnPoint {
            pos: blue,
            rot: [0.0; 3],
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

/// Culling on with the always-visible radius off, so that only the line of sight decides.
fn los_only() -> VisTuning {
    VisTuning {
        always_radius: 0.0,
        ..VisTuning::default()
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
    now: u32,
}

impl Net {
    fn new(vis: VisTuning, red: [f32; 3], blue: [f32; 3], users: &[&str]) -> Net {
        let cfg = ServerTuning {
            tick_hz: TICK_HZ,
            vis,
            game: GameTuning {
                start_order: dc_dserver::game::StartOrder::Spec,
                ..GameTuning::default()
            },
            ..ServerTuning::default()
        };
        let mut server = Server::with_tables(cfg, world(red, blue), tables(), 0);
        assert!(server.handle_line(0, "room key=TDM type=1 goal=20 ff=0"));
        for u in users {
            assert!(server.handle_line(0, u));
        }
        Net {
            server,
            peers: Vec::new(),
            now: 0,
        }
    }

    fn add(&mut self, addr: u32, name: &str, walk: f32) {
        let mut cfg = SynthConfig::named(name);
        cfg.box_bounds = Some((
            self.server.world().extent_min,
            self.server.world().extent_max,
        ));
        cfg.walk_step = walk;
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
            if let Output::Transmit { to, bytes } = o {
                if let Some(p) = self.peers.iter_mut().find(|p| p.addr == to) {
                    p.down.send(now, bytes);
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

    fn obj(&self, i: usize) -> u16 {
        self.c(i).seen.view_object.expect("view object")
    }

    fn pid(&self, i: usize) -> u8 {
        self.c(i).seen.client_id.expect("client id").0 as u8
    }

    fn all_alive(&mut self) {
        let n = self.peers.len();
        assert!(
            self.run_until(20_000, |net| (0..n).all(|i| net.c(i).life_state() == 1)),
            "not all alive"
        );
    }

    /// Where client `me` has client `other`'s object (from the object records).
    fn record_pos(&self, me: usize, other: usize) -> Option<[f32; 3]> {
        self.c(me).seen.objects.get(&self.obj(other))?.pos
    }

    fn at_park(&self, me: usize, other: usize) -> bool {
        let park = self.server.visibility().park_point();
        self.record_pos(me, other)
            .is_some_and(|p| (0..3).all(|k| (p[k] - park[k]).abs() < 0.01))
    }

    fn entries(&self, me: usize, other: usize) -> u32 {
        self.c(me)
            .seen
            .remote
            .get(&self.obj(other))
            .map_or(0, |r| r.entries)
    }
}

const USERS: [&str; 2] = ["user 7 3 99 0 120 Dave", "user 8 4 55 1 0 Erin"];

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[test]
fn an_enemy_behind_the_wall_is_parked_and_never_streamed() {
    let mut net = Net::new(VisTuning::default(), RED, BLUE, &USERS);
    net.add(1, "Dave", 0.0);
    net.add(2, "Erin", 0.0);
    net.all_alive();
    net.run_for(2000);
    let park = net.server.visibility().park_point();
    // the park point is on the bottom face of the world box
    assert_eq!(park[1], net.server.world().extent_min[1]);
    for (me, other) in [(0, 1), (1, 0)] {
        let s = &net.c(me).seen;
        assert!(s.errors.is_empty(), "client {me}: {:?}", s.errors);
        assert!(
            net.at_park(me, other),
            "client {me} has the enemy at {:?}, park {park:?}",
            net.record_pos(me, other)
        );
        assert_eq!(
            net.entries(me, other),
            0,
            "no stream entry of a parked enemy"
        );
        // the roster and the scores are untouched
        assert_eq!(s.roster.len(), 2);
        assert!(net.server.visibility().parked(net.pid(me), net.obj(other)));
    }
    assert!(net.server.visibility().stats.parks >= 2);
    // culling off: the same arrangement streams both ways
    let mut off = Net::new(VisTuning::off(), RED, BLUE, &USERS);
    off.add(1, "Dave", 0.0);
    off.add(2, "Erin", 0.0);
    off.all_alive();
    off.run_for(2000);
    for (me, other) in [(0, 1), (1, 0)] {
        assert!(!off.at_park(me, other));
        assert!(off.entries(me, other) > 30, "{}", off.entries(me, other));
    }
}

#[test]
fn stepping_into_view_snaps_back_then_streams() {
    // Erin walks -x from behind the wall (x = 500) through it (x = 0) toward Dave's side
    let mut net = Net::new(los_only(), RED, BLUE, &USERS);
    net.add(1, "Dave", 0.0);
    net.add(2, "Erin", -10.0);
    net.all_alive();
    assert!(net.at_park(0, 1), "hidden at first");
    let mut shown_at = None;
    while net.now < 15_000 {
        net.step();
        if net.entries(0, 1) > 0 {
            // the snap back arrived before (or with) the first stream entry: the record position is the true one
            let truth = net.c(1).position();
            let rec = net.record_pos(0, 1).expect("record");
            assert!(
                !net.at_park(0, 1) && dist(rec, truth) < 60.0,
                "record {rec:?}, truth {truth:?}"
            );
            shown_at = Some((net.now, truth));
            break;
        }
    }
    let (t, at) = shown_at.expect("Erin came into view");
    // the lookahead shows her before she is through the wall plane (x > -8): never after the line of sight opened
    assert!(at[0] > -8.0, "shown at {at:?} (t {t})");
    net.run_for(1000);
    let r = net.c(0).seen.remote[&net.obj(1)];
    let truth = net.c(1).position();
    assert!(r.entries > 20, "{}", r.entries);
    assert!(dist(r.pos, truth) < 40.0, "{:?} vs {truth:?}", r.pos);
    assert!(net.c(0).seen.errors.is_empty());
}

#[test]
fn an_enemy_walking_into_cover_is_parked_one_window_after_the_last_sight() {
    // both on Dave's side (x < 0) at first; Erin walks +x through the wall into cover
    let mut net = Net::new(los_only(), RED, [-200.0, 0.0, -500.0], &USERS);
    net.add(1, "Dave", 0.0);
    net.add(2, "Erin", 10.0);
    net.all_alive();
    net.run_for(500);
    assert!(net.entries(0, 1) > 0, "visible at first");
    assert!(!net.at_park(0, 1));
    let (dave, erin) = (net.pid(0), net.pid(1));
    let mut last_clear = None;
    let mut parked = None;
    while net.now < 20_000 {
        net.step();
        if let Some(t) = net.server.visibility().last_clear(dave, erin) {
            last_clear = Some(t);
        }
        if net.at_park(0, 1) {
            parked = Some(net.now);
            break;
        }
    }
    let (lc, p) = (last_clear.expect("seen"), parked.expect("parked"));
    let w = VisTuning::default().exposure_ms;
    let tick = 1000 / TICK_HZ;
    assert!(
        p >= lc + w && p <= lc + w + 2 * tick + 20,
        "last clear test {lc}, parked {p} (window {w})"
    );
    // no stream entry after the park
    let before = net.entries(0, 1);
    net.run_for(1500);
    assert_eq!(net.entries(0, 1), before);
    assert!(
        net.at_park(0, 1),
        "the refresh parks it again, nothing moves it"
    );
}

#[test]
fn teammates_are_streamed_through_the_wall() {
    // Dave and Carl (both red) at the red spawn, Erin (blue) behind the wall; Carl walks +x through the wall
    let users = [
        "user 7 3 99 0 120 Dave",
        "user 9 5 77 0 0 Carl",
        "user 8 4 55 1 0 Erin",
    ];
    let mut net = Net::new(los_only(), RED, BLUE, &users);
    net.add(1, "Dave", 0.0);
    net.add(3, "Carl", 10.0);
    net.add(2, "Erin", 0.0);
    net.all_alive();
    assert!(net.at_park(0, 2), "Erin is hidden from Dave");
    net.run_until(30_000, |n| n.c(1).position()[0] > 100.0);
    assert!(net.c(1).position()[0] > 100.0, "Carl crossed the wall");
    let before = net.entries(0, 1);
    net.run_for(1000);
    // Dave still gets Carl's entries (teammate) through the wall
    assert!(net.entries(0, 1) > before + 20);
    assert!(!net.at_park(0, 1));
    let r = net.c(0).seen.remote[&net.obj(1)];
    assert!(dist(r.pos, net.c(1).position()) < 40.0);
    // Erin now sees Carl (same side of the wall), Dave stays parked for her
    assert!(!net.at_park(2, 1) && net.entries(2, 1) > 0);
    assert!(net.at_park(2, 0));
    for i in 0..3 {
        assert!(
            net.c(i).seen.errors.is_empty(),
            "{:?}",
            net.c(i).seen.errors
        );
    }
}

/// DSV-480: only a server whose culling is off (`-visibility off`, a room with wallhacks) sends other players'
/// health; a culled (vanilla) server sends none.
#[test]
fn only_an_unculled_server_sends_remote_health() {
    let make = |vis: VisTuning| {
        let cfg = ServerTuning {
            tick_hz: TICK_HZ,
            vis,
            ..ServerTuning::default()
        };
        Server::<u32>::with_tables(cfg, world(RED, BLUE), tables(), 0)
    };
    assert!(!make(VisTuning::default()).game().sends_remote_health());
    let off = VisTuning {
        enabled: false,
        ..VisTuning::default()
    };
    assert!(make(off).game().sends_remote_health());
}
