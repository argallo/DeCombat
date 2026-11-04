//! NAT rebinding (NET-23, hardened by NET-24..26): a client's source port changes mid-session. The 2009 transport keys a
//! link by the peer's address, so without help both sides talk past each other: the host drops what comes from the
//! new port as an unknown peer and its own datagrams go to the old port, which the NAT no longer maps. With
//! `Host::migrate` the link follows the client to the new port, but only for datagrams that carry the link's move
//! secret (the host hands it to the client in a reliable message; the client then sends it as a trailer of every
//! datagram). Every reliable message still arrives once and in order both ways. Other clients behind the same NAT
//! (same IP) keep their links, also when they rebind at the same moment; a stranger behind the same IP who guesses
//! sequence numbers, or sends a bye, takes nothing. Our own harness.
// ORIGINAL: our own harness (netlab)

use dc_net_engine::body::{move_trailer, parse_body};
use dc_net_engine::link::{BYE_AFTER_MOVE_MS, MAX_DATAGRAM_BYTES, MOVE_LIMIT};
use dc_net_engine::sim::{Faults, SimChannel};
use dc_net_engine::{Host, HostConfig, HostEvent, JoinStatus, Joiner, Link, LinkConfig, LinkEvent};
use dc_wire::linkmove::Secret;
use dc_wire::udp::{
    encode_link_datagram, BitWriter, CtlBlock, LinkBody, RelBlock, TODO_CLIENT_JOIN_RATE_BPS,
    TODO_GAME_APP_ID_GUID_LAYOUT,
};

/// (ip, port)
type Addr = (u32, u16);

fn same_ip(a: &Addr, b: &Addr) -> bool {
    a.0 == b.0
}

/// The test's stand-in for the session layer's secret message (dserver: shell id 0xf3).
const SECRET_MSG: u8 = 0xf3;

fn numbered(n: u32) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(9).write_u16(0);
    for b in n.to_le_bytes() {
        w.write_u8(b);
    }
    w
}

fn number(bits: &BitWriter) -> Option<u32> {
    let b = bits.as_bytes();
    (b.len() >= 7 && b[0] == 9).then(|| u32::from_le_bytes([b[3], b[4], b[5], b[6]]))
}

fn secret_of(i: usize) -> Secret {
    let mut s = [0u8; 8];
    for (k, b) in s.iter_mut().enumerate() {
        *b = (i as u8 + 1).wrapping_mul(0x9d).wrapping_add(k as u8 * 31);
    }
    s
}

struct Client {
    addr: Addr,
    joiner: Joiner,
    link: Option<Link>,
    up: SimChannel,
    down: SimChannel,
    got: Vec<u32>,
    sent: u32,
    closed: bool,
    /// A client without the feature (a 2009 client): ignores the secret message.
    old: bool,
    /// The client's uplink is cut (its own lag spike) in this window.
    mute: Option<(u32, u32)>,
}

impl Client {
    fn new(addr: Addr, seed: u64) -> Client {
        Client {
            addr,
            joiner: Joiner::new(TODO_GAME_APP_ID_GUID_LAYOUT, TODO_CLIENT_JOIN_RATE_BPS),
            link: None,
            up: SimChannel::new(seed, Faults::clean(5)),
            down: SimChannel::new(seed + 1, Faults::clean(5)),
            got: Vec::new(),
            sent: 0,
            closed: false,
            old: false,
            mute: None,
        }
    }
}

#[derive(Default)]
struct Plan {
    migrate: bool,
    /// The host hands out secrets.
    secrets: bool,
    /// (time, client, new port)
    rebinds: Vec<(u32, usize, u16)>,
    old_clients: Vec<usize>,
    /// (client, from, to): uplink cut
    mutes: Vec<(usize, u32, u32)>,
    /// A stranger behind ip 1 (port 999) sends these datagrams at these times.
    attack: Vec<(u32, Vec<u8>)>,
    ms: u32,
}

struct Outcome {
    clients: Vec<Client>,
    host_got: Vec<Vec<u32>>,
    host_sent: Vec<u32>,
    migrations: u64,
    unknown: u64,
    host_closed: usize,
    host: Host<Addr>,
}

/// Three clients: 0 and 1 behind the same NAT (ip 1), 2 elsewhere.
fn run(plan: &Plan) -> Outcome {
    let mut host: Host<Addr> = Host::new(HostConfig::default());
    let mut clients = vec![
        Client::new((1, 100), 10),
        Client::new((1, 200), 20),
        Client::new((2, 100), 30),
    ];
    for &i in &plan.old_clients {
        clients[i].old = true;
    }
    for &(i, a, b) in &plan.mutes {
        clients[i].mute = Some((a, b));
    }
    let mut host_got: Vec<Vec<u32>> = vec![Vec::new(); 3];
    let mut host_sent = vec![0u32; 3];
    let mut host_closed = 0;
    let mut migrations = 0;
    let who = |clients: &[Client], a: &Addr| clients.iter().position(|c| c.addr == *a);
    let ms = if plan.ms == 0 { 10_000 } else { plan.ms };
    for now in 0..ms {
        for &(t, i, port) in &plan.rebinds {
            if t == now {
                clients[i].addr = (clients[i].addr.0, port);
            }
        }
        // client side
        for c in clients.iter_mut() {
            if c.link.is_none() {
                if let Some(d) = c.joiner.poll(now) {
                    c.up.send(now, d);
                }
            }
            for d in c.down.deliver(now) {
                match c.link.as_mut() {
                    Some(l) => l.handle_datagram(now, &d),
                    None => {
                        if c.joiner.handle_datagram(&d) == JoinStatus::Accepted {
                            c.link = c.joiner.clone().into_link(now);
                        }
                    }
                }
            }
            if let Some(l) = c.link.as_mut() {
                if now % 50 == 0 && c.sent < 150 {
                    c.sent += 1;
                    l.send_reliable(numbered(c.sent));
                }
                if now % 33 == 0 {
                    l.send_unreliable(now, BitWriter::from_bytes(&[11, 0x64, 0, 0]));
                }
                for e in l.poll(now) {
                    match e {
                        LinkEvent::Transmit(d) => {
                            let muted = c.mute.is_some_and(|(a, b)| now >= a && now < b);
                            if !muted {
                                c.up.send(now, d);
                            }
                        }
                        LinkEvent::Message { bits, .. } => {
                            let b = bits.as_bytes();
                            if b.first() == Some(&SECRET_MSG) && b.len() == 9 {
                                if !c.old {
                                    let s: Secret = b[1..9].try_into().unwrap();
                                    l.set_move_trailer(s);
                                }
                            } else {
                                c.got.extend(number(&bits));
                            }
                        }
                        LinkEvent::Closed { .. } => c.closed = true,
                    }
                }
            }
        }
        // the network: each client's datagrams reach the host from its current address
        for c in clients.iter_mut() {
            let from = c.addr;
            for d in c.up.deliver(now) {
                if plan.migrate && host.migrate(now, &from, &d, same_ip, 250).is_some() {
                    migrations += 1;
                }
                host.handle_datagram(now, from, &d);
            }
        }
        for (t, d) in &plan.attack {
            if *t == now {
                let from = (1, 999);
                if plan.migrate && host.migrate(now, &from, d, same_ip, 250).is_some() {
                    migrations += 1;
                }
                host.handle_datagram(now, from, d);
            }
        }
        // host side
        if now % 100 == 50 {
            for (i, c) in clients.iter().enumerate() {
                if host_sent[i] < 80 && host.link(&c.addr).is_some() {
                    host_sent[i] += 1;
                    let a = c.addr;
                    host.send_reliable(&a, numbered(host_sent[i]));
                }
            }
        }
        for e in host.poll(now) {
            match e {
                HostEvent::Transmit { to, bytes } => {
                    // the NAT maps only the client's current port: datagrams to an old one are lost
                    if let Some(i) = who(&clients, &to) {
                        clients[i].down.send(now, bytes);
                    }
                }
                HostEvent::Message { peer, bits, .. } => {
                    if let (Some(i), Some(n)) = (who(&clients, &peer), number(&bits)) {
                        host_got[i].push(n);
                    }
                }
                HostEvent::LinkClosed { .. } => host_closed += 1,
                HostEvent::LinkOpened { peer, .. } => {
                    if plan.secrets {
                        let i = who(&clients, &peer).expect("a client");
                        let s = secret_of(i);
                        host.link_mut(&peer).unwrap().set_move_secret(s);
                        let mut w = BitWriter::new();
                        w.write_u8(SECRET_MSG);
                        for b in s {
                            w.write_u8(b);
                        }
                        host.send_reliable(&peer, w);
                    }
                }
            }
        }
    }
    let unknown = host.stats().unknown_peer;
    Outcome {
        clients,
        host_got,
        host_sent,
        migrations,
        unknown,
        host_closed,
        host,
    }
}

fn assert_all_delivered(o: &Outcome) {
    assert_eq!(o.host_closed, 0);
    for (i, c) in o.clients.iter().enumerate() {
        assert!(!c.closed, "client {i} closed");
        let want: Vec<u32> = (1..=c.sent).collect();
        assert_eq!(
            o.host_got[i], want,
            "client {i} -> host: every message once, in order"
        );
        let want: Vec<u32> = (1..=o.host_sent[i]).collect();
        assert_eq!(c.got, want, "host -> client {i}");
        assert_eq!(c.sent, 150);
        assert_eq!(o.host_sent[i], 80);
    }
}

fn plan(migrate: bool, secrets: bool) -> Plan {
    Plan {
        migrate,
        secrets,
        rebinds: vec![(3000, 0, 101)],
        ..Plan::default()
    }
}

#[test]
fn a_link_follows_its_client_to_a_new_port() {
    let o = run(&plan(true, true));
    assert_eq!(o.migrations, 1, "exactly one move");
    assert_all_delivered(&o);
}

#[test]
fn two_clients_behind_one_ip_rebinding_at_once_each_keep_their_own_link() {
    let mut p = plan(true, true);
    p.rebinds = vec![
        (3000, 0, 101),
        (3000, 1, 201),
        (6000, 0, 102),
        (6000, 1, 100),
    ];
    let o = run(&p);
    assert_eq!(o.migrations, 4);
    assert_eq!(o.host.stats().migrations_ambiguous, 0);
    assert_all_delivered(&o);
}

#[test]
fn without_the_move_the_rebound_client_is_cut_off() {
    let o = run(&plan(false, true));
    assert_eq!(o.migrations, 0);
    assert!(
        o.unknown > 50,
        "the new port is an unknown peer: {}",
        o.unknown
    );
    assert!(
        o.host_got[0].len() < 70,
        "client 0's later messages never arrive: {}",
        o.host_got[0].len()
    );
    // the other two are untouched
    assert_eq!(o.host_got[1].len(), 150);
    assert_eq!(o.host_got[2].len(), 150);
}

#[test]
fn a_client_without_the_secret_works_but_never_moves() {
    // a 2009 client ignores the secret message: its datagrams carry no trailer, so its link never moves
    let mut p = plan(true, true);
    p.old_clients = vec![0];
    p.rebinds = vec![];
    let o = run(&p);
    assert_eq!(o.migrations, 0);
    assert_all_delivered(&o);
    let mut p = plan(true, true);
    p.old_clients = vec![0];
    let o = run(&p);
    assert_eq!(o.migrations, 0, "no secret, no move");
    assert!(o.host_got[0].len() < 70);
    assert_eq!(o.host_got[1].len(), 150);
}

/// Every combination of sequence numbers a stranger could guess for the victim's link (all 8192 cumulative acks
/// with every reliable block number, in steps of the claim window), as datagrams with no trailer, with a wrong one,
/// and each also with a transport bye in front.
fn brute_force(start: u32) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut t = start;
    let mut wrong = [0u8; 8];
    for seq in (0..8192u16).step_by(32) {
        for (k, ctl) in [
            Some(CtlBlock::Ack {
                ack_seq: seq,
                sack_mask: None,
            }),
            Some(CtlBlock::Bye),
            Some(CtlBlock::ByeCode(5)),
            None,
        ]
        .into_iter()
        .enumerate()
        {
            let body = LinkBody {
                ctl,
                rel: Some(RelBlock {
                    rel_seq: seq.wrapping_add(1) & 0x1fff,
                    tail_cont: false,
                    entries: vec![BitWriter::from_bytes(&[11, 0x64, 0xde, 0xad])],
                }),
                unrel: vec![BitWriter::from_bytes(&[11, 0x64, 0xbe, 0xef])],
            };
            out.push((t, encode_link_datagram(&body)));
            // and with a trailer of a guessed secret
            wrong[0] = seq as u8;
            wrong[1] = (seq >> 8) as u8;
            wrong[2] = k as u8;
            let mut w = body.encode_bits();
            w.write_vlen(0);
            let mut bytes = w.as_bytes().to_vec();
            bytes.extend_from_slice(&dc_wire::linkmove::trailer(&wrong));
            let mut d = vec![dc_wire::udp::check8(&bytes)];
            d.extend_from_slice(&bytes);
            assert!(move_trailer(&d).is_some());
            out.push((t, d));
        }
        t += 1;
    }
    out
}

#[test]
fn a_stranger_behind_the_same_ip_cannot_take_or_close_a_silent_link() {
    // the victim (client 0) lags for 2 s (its uplink is cut); meanwhile a stranger behind the same public IP sends
    // every sequence guess, plain and with byes, without the secret and with wrong ones
    let mut p = plan(true, true);
    p.rebinds = vec![];
    p.mutes = vec![(0, 3000, 5000)];
    p.attack = brute_force(3500);
    assert!(p.attack.len() > 2000);
    let o = run(&p);
    assert_eq!(o.migrations, 0, "nothing moved");
    let st = o.host.stats();
    assert!(
        st.migrations_refused as usize >= p.attack.len() - 300,
        "{st:?}"
    );
    assert!(o.host.link(&(1, 999)).is_none(), "the stranger has no link");
    // the victim and the other client behind the IP lose nothing
    assert_all_delivered(&o);
}

#[test]
fn junk_to_a_silent_link_does_not_keep_it_alive() {
    // NET-26: corrupt datagrams no longer refresh the last receive time, so the silence test (move, DSV-490) still
    // sees a link that hears only junk as silent
    let mut host: Host<Addr> = Host::new(HostConfig::default());
    let mut j = Joiner::new(TODO_GAME_APP_ID_GUID_LAYOUT, TODO_CLIENT_JOIN_RATE_BPS);
    let peer = (1, 100);
    host.handle_datagram(0, peer, &j.poll(0).unwrap());
    let ev = host.poll(0);
    assert!(ev.iter().any(|e| matches!(e, HostEvent::LinkOpened { .. })));
    host.link_mut(&peer).unwrap().set_move_secret(secret_of(0));
    for t in (100..2000).step_by(50) {
        let mut d = encode_link_datagram(&LinkBody {
            ctl: Some(CtlBlock::Ack {
                ack_seq: 0,
                sack_mask: None,
            }),
            ..LinkBody::default()
        });
        d[0] ^= 0xff;
        host.handle_datagram(t, peer, &d);
    }
    assert_eq!(host.link(&peer).unwrap().last_recv(), 0);
    // the client's real datagram from its new port moves the link: junk did not keep the old address "alive"
    let mut client = Link::new(0, LinkConfig::default());
    client.set_move_trailer(secret_of(0));
    client.send_unreliable(2000, BitWriter::from_bytes(&[11, 0x64, 1, 2]));
    let d = client
        .poll(2000)
        .into_iter()
        .find_map(|e| match e {
            LinkEvent::Transmit(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert_eq!(host.migrate(2000, &(1, 101), &d, same_ip, 250), Some(peer));
}

/// A host with one open link at `peer` (secret 0) and a client link that sends with that secret.
fn pair(peer: Addr) -> (Host<Addr>, Link) {
    let mut host: Host<Addr> = Host::new(HostConfig::default());
    let mut j = Joiner::new(TODO_GAME_APP_ID_GUID_LAYOUT, TODO_CLIENT_JOIN_RATE_BPS);
    host.handle_datagram(0, peer, &j.poll(0).unwrap());
    let _ = host.poll(0);
    host.link_mut(&peer).unwrap().set_move_secret(secret_of(0));
    let mut client = Link::new(0, LinkConfig::default());
    client.set_move_trailer(secret_of(0));
    (host, client)
}

fn transmits(l: &mut Link, now: u32) -> Vec<Vec<u8>> {
    l.poll(now)
        .into_iter()
        .filter_map(|e| match e {
            LinkEvent::Transmit(d) => Some(d),
            _ => None,
        })
        .collect()
}

#[test]
fn an_unreliable_only_datagram_with_the_secret_moves_the_link() {
    let (mut host, mut client) = pair((1, 100));
    // the keep-alive (ack + empty reliable block) goes out first; 10 ms later only the unreliable message is due
    let _ = transmits(&mut client, 990);
    client.send_unreliable(1000, BitWriter::from_bytes(&[11, 0x64, 7, 7]));
    let d = transmits(&mut client, 1000);
    let rx = parse_body(&d[0][1..]);
    assert!(
        rx.ctl.is_none() && rx.rel.is_none() && rx.unrel.len() == 1,
        "{rx:?}"
    );
    assert_eq!(
        host.migrate(1000, &(1, 555), &d[0], same_ip, 250),
        Some((1, 100))
    );
    host.handle_datagram(1000, (1, 555), &d[0]);
    let got: Vec<_> = host
        .poll(1000)
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                HostEvent::Message {
                    peer: (1, 555),
                    reliable: false,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        got.len(),
        1,
        "the moved link delivers the datagram's message"
    );
}

#[test]
fn a_bye_in_the_moving_datagram_does_not_close_the_link() {
    // even the real client's own bye right at a move is held off for a moment (the move never closes a link)
    let (mut host, mut client) = pair((1, 100));
    client.close(1000, dc_net_engine::Bye::Code(5));
    let byes = transmits(&mut client, 1000);
    assert!(!byes.is_empty());
    assert!(
        move_trailer(&byes[0]).is_some(),
        "the bye carries the trailer too"
    );
    assert_eq!(
        host.migrate(1000, &(1, 101), &byes[0], same_ip, 250),
        Some((1, 100))
    );
    host.handle_datagram(1000, (1, 101), &byes[0]);
    let ev = host.poll(1000);
    assert!(
        !ev.iter().any(|e| matches!(e, HostEvent::LinkClosed { .. })),
        "{ev:?}"
    );
    assert!(host.link(&(1, 101)).unwrap().is_open());
    assert_eq!(host.link(&(1, 101)).unwrap().stats().byes_after_move, 1);
    // a bye that comes later closes it as usual
    host.handle_datagram(1000 + BYE_AFTER_MOVE_MS, (1, 101), &byes[0]);
    let ev = host.poll(1000 + BYE_AFTER_MOVE_MS);
    assert!(
        ev.iter().any(|e| matches!(e, HostEvent::LinkClosed { .. })),
        "{ev:?}"
    );
}

#[test]
fn a_link_moves_at_most_three_times_a_minute() {
    let (mut host, mut client) = pair((1, 100));
    let mut at = (1, 100);
    let mut moved = 0;
    for k in 0..5u16 {
        let now = 1000 + k as u32 * 1000;
        client.send_unreliable(now, BitWriter::from_bytes(&[11, 0x64, k as u8]));
        let d = transmits(&mut client, now);
        let to = (1, 300 + k);
        if host.migrate(now, &to, &d[0], same_ip, 250).is_some() {
            moved += 1;
            at = to;
        }
    }
    assert_eq!(moved, MOVE_LIMIT);
    assert_eq!(host.stats().migrations_limited, 2);
    assert!(host.link(&at).is_some());
    // a minute after the first move, one more is allowed
    let now = 61_500;
    client.send_unreliable(now, BitWriter::from_bytes(&[11, 0x64, 9]));
    let d = transmits(&mut client, now);
    assert!(host.migrate(now, &(1, 400), &d[0], same_ip, 250).is_some());
}

#[test]
fn queued_reliable_data_goes_to_the_new_address_at_once() {
    let (mut host, mut client) = pair((1, 100));
    // the host sends a block that is lost at the old address
    host.send_reliable(&(1, 100), BitWriter::from_bytes(&[9, 1, 2, 3]));
    let lost = host.poll(10);
    assert!(lost.iter().any(|e| matches!(e, HostEvent::Transmit { .. })));
    // the client rebinds; its next datagram moves the link, and the block goes out again to the new port in the
    // same update instead of after the resend timeout
    client.send_unreliable(400, BitWriter::from_bytes(&[11, 0x64, 1]));
    let d = transmits(&mut client, 400);
    assert!(host.migrate(400, &(1, 101), &d[0], same_ip, 250).is_some());
    host.handle_datagram(400, (1, 101), &d[0]);
    let ev = host.poll(400);
    let resent = ev.iter().any(|e| match e {
        HostEvent::Transmit { to, bytes } => {
            *to == (1, 101)
                && parse_body(&bytes[1..])
                    .rel
                    .is_some_and(|r| !r.entries.is_empty())
        }
        _ => false,
    });
    assert!(resent, "{ev:?}");
}

#[test]
fn datagrams_with_the_trailer_stay_within_the_cap() {
    let mut a = Link::new(0, LinkConfig::default());
    a.set_move_trailer(secret_of(3));
    for n in 0..20u8 {
        let bytes: Vec<u8> = (0..(40 + n as usize * 37)).map(|i| i as u8 ^ n).collect();
        a.send_reliable(BitWriter::from_bytes(&bytes));
        a.send_unreliable(0, BitWriter::from_bytes(&bytes[..bytes.len() / 3]));
    }
    let mut b = Link::new(0, LinkConfig::default());
    let mut n_out = 0;
    for now in (0..3000).step_by(10) {
        for d in transmits(&mut a, now) {
            assert!(d.len() <= MAX_DATAGRAM_BYTES, "{} bytes", d.len());
            assert_eq!(move_trailer(&d), Some(secret_of(3)));
            // a plain 2009 receiver reads the same body and ignores the trailer
            assert!(dc_wire::udp::decode_link_datagram(&d).is_ok());
            b.handle_datagram(now, &d);
            n_out += 1;
        }
        for d in transmits(&mut b, now) {
            a.handle_datagram(now, &d);
        }
    }
    assert!(n_out > 20);
    assert_eq!(b.stats().delivered_rel, 20);
    assert_eq!(b.stats().bad_check, 0);
}
