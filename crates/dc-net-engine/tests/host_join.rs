//! Host <-> joiner integration over the simulated channel: join, info probe, echo, refusal paths, stale re-join,
//! messages both ways, disconnect with a bye code.

use dc_net_engine::sim::{Faults, SimChannel};
use dc_net_engine::{
    Bye, CloseCause, Host, HostConfig, HostEvent, JoinStatus, Joiner, Link, LinkEvent,
    APP_ID_BIG_ENDIAN_CANDIDATE,
};
use dc_wire::udp::{
    BitWriter, OobDatagram, TODO_CLIENT_JOIN_RATE_BPS, TODO_GAME_APP_ID_GUID_LAYOUT,
};

const CLIENT: u32 = 7;

fn ask(host: &mut Host<u32>, now: u32, from: u32, d: OobDatagram) -> Vec<HostEvent<u32>> {
    host.handle_datagram(now, from, &d.encode());
    host.poll(now)
}

fn replies(ev: &[HostEvent<u32>]) -> Vec<OobDatagram> {
    ev.iter()
        .filter_map(|e| match e {
            HostEvent::Transmit { bytes, .. } => OobDatagram::decode(bytes).ok(),
            _ => None,
        })
        .collect()
}

#[test]
fn join_then_messages_both_ways_then_coded_bye() {
    let mut host: Host<u32> = Host::new(HostConfig::default());
    let mut joiner = Joiner::new(TODO_GAME_APP_ID_GUID_LAYOUT, TODO_CLIENT_JOIN_RATE_BPS);
    let mut up = SimChannel::new(1, Faults::clean(3));
    let mut down = SimChannel::new(2, Faults::clean(3));
    let mut client: Option<Link> = None;
    let mut opened = None;
    let mut host_got = Vec::new();
    let mut client_got = Vec::new();
    let mut client_closed = None;
    let mut host_closed = None;
    for now in 0..3000u32 {
        if let Some(d) = joiner.poll(now) {
            up.send(now, d);
        }
        for d in up.deliver(now) {
            host.handle_datagram(now, CLIENT, &d);
        }
        for e in host.poll(now) {
            match e {
                HostEvent::Transmit { to, bytes } => {
                    assert_eq!(to, CLIENT);
                    down.send(now, bytes)
                }
                HostEvent::LinkOpened {
                    peer,
                    rate_bps,
                    app_id_index,
                } => opened = Some((peer, rate_bps, app_id_index)),
                HostEvent::Message { bits, reliable, .. } => host_got.push((reliable, bits)),
                HostEvent::LinkClosed { code, cause, .. } => host_closed = Some((code, cause)),
            }
        }
        for d in down.deliver(now) {
            match client.as_mut() {
                Some(l) => l.handle_datagram(now, &d),
                None => {
                    if joiner.handle_datagram(&d) == JoinStatus::Accepted {
                        let mut l = joiner.clone().into_link(now).unwrap();
                        let mut hello = BitWriter::new();
                        hello.write_u8(5).write_u16(0);
                        l.send_reliable(hello);
                        l.send_unreliable(now, BitWriter::from_bytes(&[11, 0x64, 0, 0]));
                        client = Some(l);
                    }
                }
            }
        }
        if now == 500 {
            assert!(host.send_reliable(&CLIENT, BitWriter::from_bytes(&[4, 7, 0, 0, 0])));
        }
        if now == 1000 {
            host.disconnect(now, &CLIENT, Bye::Code(6));
        }
        if let Some(l) = client.as_mut() {
            for e in l.poll(now) {
                match e {
                    LinkEvent::Transmit(d) => up.send(now, d),
                    LinkEvent::Message { bits, .. } => client_got.push(bits),
                    LinkEvent::Closed { code, cause } => client_closed = Some((code, cause)),
                }
            }
        }
    }
    assert_eq!(opened, Some((CLIENT, dc_wire::udp::TODO_HOST_RATE_BPS, 0)));
    assert_eq!(host_got.len(), 2, "{host_got:?}");
    assert!(host_got.iter().any(|m| m.0 && m.1.as_bytes() == [5, 0, 0]));
    assert!(host_got
        .iter()
        .any(|m| !m.0 && m.1.as_bytes() == [11, 0x64, 0, 0]));
    assert_eq!(client_got, vec![BitWriter::from_bytes(&[4, 7, 0, 0, 0])]);
    assert_eq!(host_closed, Some((6, CloseCause::Local)));
    assert_eq!(client_closed, Some((6, CloseCause::PeerBye)));
    assert_eq!(host.open_links(), 0);
    assert!(host.link(&CLIENT).is_none(), "finished link forgotten");
}

#[test]
fn info_probe_echo_and_wrong_app() {
    let mut host: Host<u32> = Host::new(HostConfig {
        session_name: b"room 1".to_vec(),
        host_byte: 16,
        ..HostConfig::default()
    });
    // one live link so link_count = 1
    let _ = ask(
        &mut host,
        0,
        1,
        OobDatagram::JoinAsk {
            app_id: APP_ID_BIG_ENDIAN_CANDIDATE,
            rate_bps: 1,
        },
    );
    let r = replies(&ask(
        &mut host,
        1,
        2,
        OobDatagram::InfoAsk {
            app_id: TODO_GAME_APP_ID_GUID_LAYOUT,
            probe_tag: 9,
        },
    ));
    match &r[..] {
        [OobDatagram::InfoReply(Some(i))] => {
            assert_eq!(i.session_name, b"room 1");
            assert_eq!((i.probe_tag, i.link_count, i.host_byte), (9, 1, 16));
        }
        other => panic!("{other:?}"),
    }
    let r = replies(&ask(
        &mut host,
        2,
        2,
        OobDatagram::InfoAsk {
            app_id: [0; 16],
            probe_tag: 1,
        },
    ));
    assert_eq!(r, vec![OobDatagram::InfoReply(None)]);
    let r = replies(&ask(
        &mut host,
        3,
        2,
        OobDatagram::EchoAsk { echo_id: 0xABCDEF },
    ));
    assert_eq!(r, vec![OobDatagram::EchoReply { echo_id: 0xABCDEF }]);
    let r = replies(&ask(
        &mut host,
        4,
        3,
        OobDatagram::JoinAsk {
            app_id: [9; 16],
            rate_bps: 1,
        },
    ));
    assert_eq!(
        r,
        vec![OobDatagram::JoinReply {
            accepted: false,
            same_app: false
        }]
    );
    assert_eq!(host.open_links(), 1);
}

#[test]
fn repeated_join_is_answered_stale_rejoin_is_dropped_and_limit_refuses() {
    let mut host: Host<u32> = Host::new(HostConfig {
        max_links: Some(1),
        ..HostConfig::default()
    });
    let join = OobDatagram::JoinAsk {
        app_id: TODO_GAME_APP_ID_GUID_LAYOUT,
        rate_bps: 99_000_000,
    };
    let ev = ask(&mut host, 0, 1, join.clone());
    assert!(ev.iter().any(|e| matches!(
        e,
        HostEvent::LinkOpened {
            rate_bps: 99_000_000,
            ..
        }
    )));
    // repeat within 9999 ms: answered again, no new link
    let ev = ask(&mut host, 300, 1, join.clone());
    assert!(!ev.iter().any(|e| matches!(e, HostEvent::LinkOpened { .. })));
    assert!(replies(&ev).contains(&OobDatagram::JoinReply {
        accepted: true,
        same_app: false
    }));
    // a second peer is over the DeCombat limit
    let ev = ask(&mut host, 400, 2, join.clone());
    assert_eq!(
        replies(&ev),
        vec![OobDatagram::JoinReply {
            accepted: false,
            same_app: true
        }]
    );
    // more than 9999 ms without traffic on peer 1's link: dropped with code 3, not answered
    let ev = ask(&mut host, 10_001, 1, join.clone());
    assert!(ev.iter().any(|e| matches!(
        e,
        HostEvent::LinkClosed {
            peer: 1,
            code: 3,
            cause: CloseCause::StaleRejoin
        }
    )));
    assert!(replies(&ev).is_empty());
    // the client's next retry creates a fresh link
    let ev = ask(&mut host, 10_301, 1, join);
    assert!(ev
        .iter()
        .any(|e| matches!(e, HostEvent::LinkOpened { peer: 1, .. })));
}

#[test]
fn link_datagram_from_unknown_peer_is_dropped() {
    let mut host: Host<u32> = Host::new(HostConfig::default());
    host.handle_datagram(0, 5, &[0x12, 0x34]);
    assert!(host.poll(0).is_empty());
    assert_eq!(host.stats().unknown_peer, 1);
}
