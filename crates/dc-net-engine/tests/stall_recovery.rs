//! A client link that stops for several seconds (no poll, no receive: a frame or process stall on a slow machine)
//! comes back by itself: once it runs again its acks, reliable data and player updates reach the host within a
//! fraction of a second, the host's backlog arrives exactly once and in order, and neither side closes as long as the
//! stall stays under the transport's idle limit. Our own harness (fix-first-match-drop); nothing of the 2009 wire
//! changes.
// ORIGINAL: our own harness (fix-first-match-drop); no 2009 behaviour is asserted beyond the transport spec

use dc_net_engine::sim::{Faults, SimChannel};
use dc_net_engine::{Link, LinkConfig, LinkEvent};
use dc_wire::udp::{BitWriter, IDLE_DROP_MS};

/// Datagrams a socket's receive buffer holds while the owner does not read (the rest is lost).
const SOCKET_BUFFER_DATAGRAMS: usize = 64;

#[derive(Default)]
struct Side {
    rel: Vec<u32>,
    unrel_at: Vec<u32>,
    closed: bool,
    first_in_after: Option<u32>,
}

fn take(events: Vec<LinkEvent>, chan: &mut SimChannel, now: u32, side: &mut Side) {
    for e in events {
        match e {
            LinkEvent::Transmit(d) => chan.send(now, d),
            LinkEvent::Message {
                reliable: true,
                bits,
            } => {
                let b = bits.as_bytes();
                side.rel.push(u32::from_le_bytes([b[1], b[2], b[3], b[4]]));
            }
            LinkEvent::Message {
                reliable: false, ..
            } => side.unrel_at.push(now),
            LinkEvent::Closed { .. } => side.closed = true,
        }
    }
}

fn numbered(tag: u8, n: u32) -> BitWriter {
    let mut v = vec![tag];
    v.extend_from_slice(&n.to_le_bytes());
    BitWriter::from_bytes(&v)
}

struct Run {
    /// at the host: what the client sent
    host: Side,
    /// at the client: what the host sent
    client: Side,
    host_rel_sent: u32,
    client_rel_sent: u32,
    stall_end: u32,
    client_link: Link,
    host_link: Link,
}

/// Host streams reliable data every 50 ms and an unreliable update every 33 ms; the client sends an unreliable player
/// update every 33 ms and a reliable message every 500 ms. The client does nothing in `[stall_at, stall_at + stall)`;
/// what arrives for it meanwhile waits in a 64-datagram socket buffer.
fn run(stall: u32, after: u32) -> Run {
    let stall_at = 5_000;
    let stall_end = stall_at + stall;
    let end = stall_end + after;
    let mut host_link = Link::new(
        0,
        LinkConfig {
            hosting: true,
            ..LinkConfig::default()
        },
    );
    let mut client_link = Link::new(0, LinkConfig::default());
    let mut to_client = SimChannel::new(11, Faults::clean(20));
    let mut to_host = SimChannel::new(12, Faults::clean(20));
    let mut host = Side::default();
    let mut client = Side::default();
    let mut buffered: Vec<Vec<u8>> = Vec::new();
    let (mut host_rel_sent, mut client_rel_sent) = (0u32, 0u32);
    for now in 0..end {
        let stalled = (stall_at..stall_end).contains(&now);
        // host
        if now % 50 == 0 {
            host_link.send_reliable(numbered(1, host_rel_sent));
            host_rel_sent += 1;
        }
        if now % 33 == 0 {
            host_link.send_unreliable(now, BitWriter::from_bytes(&[2, 0, 0, 0, 0]));
        }
        for d in to_host.deliver(now) {
            if now >= stall_end && host.first_in_after.is_none() {
                host.first_in_after = Some(now);
            }
            host_link.handle_datagram(now, &d);
        }
        let ev = host_link.poll(now);
        take(ev, &mut to_client, now, &mut host);
        // client
        let arrived = to_client.deliver(now);
        if stalled {
            for d in arrived {
                if buffered.len() < SOCKET_BUFFER_DATAGRAMS {
                    buffered.push(d);
                }
            }
            continue;
        }
        for d in buffered.drain(..).chain(arrived) {
            client_link.handle_datagram(now, &d);
        }
        if now % 33 == 0 {
            client_link.send_unreliable(now, BitWriter::from_bytes(&[3, 0, 0, 0, 0]));
        }
        if now % 500 == 0 {
            client_link.send_reliable(numbered(4, client_rel_sent));
            client_rel_sent += 1;
        }
        let ev = client_link.poll(now);
        take(ev, &mut to_host, now, &mut client);
    }
    Run {
        host,
        client,
        host_rel_sent,
        client_rel_sent,
        stall_end,
        client_link,
        host_link,
    }
}

fn check(stall: u32) {
    let after = 15_000;
    let r = run(stall, after);
    let end = r.stall_end + after;
    assert!(
        !r.host.closed && !r.client.closed,
        "stall {stall}: a side closed"
    );
    let first = r
        .host
        .first_in_after
        .expect("the host hears the client again");
    assert!(
        first - r.stall_end <= 200,
        "stall {stall}: first datagram {} ms after the stall",
        first - r.stall_end
    );
    // the host's backlog arrives exactly once and in order (all but what was queued in the last second)
    let want: Vec<u32> = (0..r.client.rel.len() as u32).collect();
    assert_eq!(r.client.rel, want, "stall {stall}: host -> client order");
    assert!(
        r.client.rel.len() as u32 + 20 >= r.host_rel_sent,
        "stall {stall}: {} of {} host messages arrived",
        r.client.rel.len(),
        r.host_rel_sent
    );
    let want: Vec<u32> = (0..r.host.rel.len() as u32).collect();
    assert_eq!(r.host.rel, want, "stall {stall}: client -> host order");
    assert!(
        r.host.rel.len() as u32 + 2 >= r.client_rel_sent,
        "stall {stall}"
    );
    // the player updates flow at the send rate again: at least 20/s in the last 2 s
    let late = r
        .host
        .unrel_at
        .iter()
        .filter(|&&t| t + 2_000 >= end)
        .count();
    assert!(late >= 40, "stall {stall}: {late} updates in the last 2 s");
    // and the client's updates came back within a second of the stall
    let soon = r
        .host
        .unrel_at
        .iter()
        .filter(|&&t| t >= r.stall_end && t < r.stall_end + 1_000)
        .count();
    assert!(
        soon >= 15,
        "stall {stall}: {soon} updates in the first second after the stall"
    );
    assert!(
        !r.client_link.is_stalled() && !r.client_link.is_paused(),
        "stall {stall}: client still paused"
    );
    assert!(
        !r.host_link.is_stalled(),
        "stall {stall}: host still in trickle mode"
    );
}

#[test]
fn short_stall_recovers() {
    check(3_000);
}

#[test]
fn long_frame_stall_recovers() {
    check(8_000);
}

#[test]
fn twenty_second_stall_recovers() {
    check(20_000);
}

#[test]
fn stall_just_under_the_idle_limit_recovers() {
    check(IDLE_DROP_MS - 5_000);
}
