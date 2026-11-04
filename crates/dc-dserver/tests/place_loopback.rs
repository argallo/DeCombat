//! GATE-PLACE loopback tests: the server and two synthetic clients (simulated channel, injected time) with the
//! test-only placement feature on, off, and asked with a wrong key.

use dc_dserver::session::{Output, Server, ServerTuning, StartOrder};
use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig};
use dc_dserver::world::{synthetic_world_bytes, world_from_bytes, WorldInfo};
use dc_net_engine::sim::{Faults, SimChannel};

const ANNOUNCE: &str = "Worlds\\Synth.dat";

fn world() -> WorldInfo {
    world_from_bytes(&synthetic_world_bytes(), ANNOUNCE, "Synth").unwrap()
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

    fn client(&self, i: usize) -> &SynthClient {
        &self.peers[i].client
    }
}

fn dave() -> SynthConfig {
    SynthConfig::named("Dave")
}

const KEY: &str = "sesame";
const SPOT: [f32; 3] = [100.0, 10.0, -50.0];

fn room(place: Option<&str>, entry: Option<&str>) -> Net {
    let mut cfg = ServerTuning::default();
    if let Some(k) = place {
        cfg.game.place.key = Some(k.into());
    }
    if let Some(e) = entry {
        cfg.game
            .place
            .entries
            .push(dc_dserver::game::place::parse_entry(e).unwrap());
    }
    let mut net = Net::reference(cfg);
    net.add(1, dave(), Faults::clean(2));
    net.add(2, SynthConfig::named("Big Erin"), Faults::clean(2));
    assert!(
        net.run_until(20_000, |n| n.client(0).state() == ClientState::InWorld
            && n.client(1).state() == ClientState::InWorld)
    );
    net.run_until(net.now + 1_500, |_| false);
    net
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 60.0)
}

/// A scripted entry fires once, at its time after the spawn, and the bot's next update is at the spot.
#[test]
fn scripted_placement_moves_the_named_bot_once() {
    let mut net = room(Some(KEY), Some("dave,100,10,-50,90,6000"));
    let t0 = net.now;
    let before = net.client(0).seen.teleports.len();
    assert!(
        !net.logs.iter().any(|l| l.contains("place: dave")),
        "{:?}",
        net.logs
    );
    assert!(
        net.run_until(t0 + 20_000, |n| n.client(0).seen.teleports.len() > before
            && near(n.client(0).seen.teleports.last().unwrap().2, SPOT))
    );
    let (at, seq, _, _) = *net.client(0).seen.teleports.last().unwrap();
    let spawned = net.client(0).seen.teleports[0].0;
    assert!(
        at >= t0 && at >= spawned + 6_000,
        "placement came before the script time"
    );
    // the next update carries the new sequence and the walk starts from the spot
    net.run_until(net.now + 600, |_| false);
    assert!(
        near(net.client(0).position(), SPOT),
        "{:?} seq {seq}",
        net.client(0).position()
    );
    assert!(!near(net.client(1).position(), SPOT));
    // the other bot sees him at the spot in the stream
    let placed = net
        .logs
        .iter()
        .filter(|l| l.contains("place: dave"))
        .count();
    assert_eq!(placed, 1, "{:?}", net.logs);
    net.run_until(net.now + 2_000, |_| false);
    assert_eq!(
        net.logs
            .iter()
            .filter(|l| l.contains("place: dave"))
            .count(),
        1
    );
}

/// A client's console request is refused with a wrong key and with the feature off; the right key moves only the sender.
#[test]
fn console_request_needs_the_key() {
    let mut net = room(Some(KEY), None);
    let n0 = net.client(1).seen.teleports.len();
    net.peers[1].client.send_console("place wrong 100 10 -50 0");
    net.run_until(net.now + 500, |_| false);
    assert_eq!(net.client(1).seen.teleports.len(), n0);
    assert!(
        net.logs.iter().any(|l| l.contains("refused (wrong key)")),
        "{:?}",
        net.logs
    );
    net.peers[1]
        .client
        .send_console(&format!("place {KEY} 100 10 -50 0"));
    assert!(net.run_until(net.now + 2_000, |n| near(
        n.client(1).seen.teleports.last().unwrap().2,
        SPOT
    )));
    assert!(!near(net.client(0).seen.teleports.last().unwrap().2, SPOT));

    let mut off = room(None, None);
    let n1 = off.client(1).seen.teleports.len();
    off.peers[1]
        .client
        .send_console(&format!("place {KEY} 100 10 -50 0"));
    off.run_until(off.now + 500, |_| false);
    assert_eq!(off.client(1).seen.teleports.len(), n1);
    assert!(
        off.logs.iter().any(|l| l.contains("refused (feature off)")),
        "{:?}",
        off.logs
    );
}

#[test]
fn launch_arguments_are_keyed_and_bounded() {
    use dc_dserver::cli::parse;
    let p = |s: &str| parse(s.split_whitespace().map(String::from));
    assert!(p("-world w -place dave,1,2,3,0").is_err());
    let a = p("-world w -place keyed=k -place dave,1,2,3,90,500").unwrap();
    assert_eq!(a.server.game.place.entries.len(), 1);
    assert_eq!(a.server.game.place.key.as_deref(), Some("k"));
    assert!(p("-world w").unwrap().server.game.place.key.is_none());
    assert!(p("-world w -place keyed=k -place dave,1,2").is_err());
    let many: String = (0..17).map(|i| format!(" -place p{i},0,0,0,0")).collect();
    assert!(p(&format!("-world w -place keyed=k{many}")).is_err());
}
