//! Session-level loopback of the rank / clan path (integrator hooks of w1-dserverrank): the lobby's `user` lines with
//! `rank=` and clan tokens go into the server, two synthetic clients join through the simulated channel, and every
//! `GS_PLAYER_INFO` 0x6b record each client stored carries the announced rank (`h2`) and, for the clan member, the
//! group block; the 0x6b of the player without a clan has no group block. Both clients parse each 0x6b exactly.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 fields 15-17; DSERVER_PIPE.md §3.1
// DC-DEFAULT: DSV-141 (proposed)
// HARNESS: session_sim.rs (same simulated-channel harness)

use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig};
use dc_dserver::world::{synthetic_world_bytes, world_from_bytes};
use dc_net_engine::sim::{Faults, SimChannel};

struct Peer {
    addr: u32,
    client: SynthClient,
    up: SimChannel,
    down: SimChannel,
}

fn run(
    server: &mut Server<u32>,
    peers: &mut [Peer],
    limit: u32,
    done: impl Fn(&[Peer]) -> bool,
) -> bool {
    let mut now = 0u32;
    while now < limit {
        for p in peers.iter_mut() {
            for d in p.client.poll(now) {
                p.up.send(now, d);
            }
        }
        for p in peers.iter_mut() {
            for d in p.up.deliver(now) {
                server.handle_datagram(now, p.addr, &d);
            }
        }
        for o in server.poll(now) {
            if let Output::Transmit { to, bytes } = o {
                if let Some(p) = peers.iter_mut().find(|p| p.addr == to) {
                    p.down.send(now, bytes);
                }
            }
        }
        for p in peers.iter_mut() {
            for d in p.down.deliver(now) {
                p.client.handle_datagram(now, &d);
            }
        }
        if done(peers) {
            return true;
        }
        now += 5;
    }
    false
}

#[test]
fn roster_records_carry_the_lobby_rank_and_clan() {
    let world = world_from_bytes(&synthetic_world_bytes(), "Worlds\\Synth.dat", "Synth").unwrap();
    let mut server = Server::new(ServerTuning::default(), world, 0);
    assert!(server.handle_line(0, "room key=TDM type=0 goal=2 ff=0"));
    // Dave: exp 3570, rank 7 by the lobby's table, clan 77; Erin: exp 0, rank 0, no clan
    assert!(server.handle_line(
        0,
        "user 7 3 99 0 3570 Dave rank=7 clan=77 clanlabel=Night%20Owls mark=5/9"
    ));
    assert!(server.handle_line(0, "user 8 4 55 1 0 Big_Erin rank=0"));
    let mut peers: Vec<Peer> = [(1u32, "Dave", 3u32), (2, "Big Erin", 7)]
        .into_iter()
        .map(|(addr, name, delay)| Peer {
            addr,
            client: SynthClient::new(SynthConfig::named(name)),
            up: SimChannel::new(addr as u64 * 2 + 1, Faults::clean(delay)),
            down: SimChannel::new(addr as u64 * 2 + 2, Faults::clean(delay)),
        })
        .collect();
    assert!(run(&mut server, &mut peers, 30_000, |p| {
        p.iter()
            .all(|x| x.client.state() == ClientState::InWorld && x.client.seen.roster.len() == 2)
    }));
    for p in &peers {
        let s = &p.client.seen;
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        let by_account = |a: u32| s.roster.values().find(|r| r.account == a).unwrap();
        let dave = by_account(7);
        assert_eq!(dave.rank, 7);
        assert_eq!(
            dave.clan,
            Some(("Night Owls".to_string(), 77, [5, 9, 0])),
            "client {}",
            p.addr
        );
        let erin = by_account(8);
        assert_eq!((erin.rank, erin.clan.clone()), (0, None));
    }
}
