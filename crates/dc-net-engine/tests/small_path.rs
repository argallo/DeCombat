//! A path that loses every datagram above a size (a small tunnel MTU with fragments dropped, a path-MTU black hole)
//! must not stop a link: no link datagram is larger than the cap (NET-10), so nothing is lost for its size, the
//! reliable stream of a join burst arrives exactly once and in order, and both sides' unreliable streams keep
//! flowing while reliable data waits (NET-11). The message mix is the one measured on a headless 2 + 7 bot match
//! (fix-big-datagrams): the server's per-player burst of reliable messages of 478 to 5490 bits when 7 players join,
//! a 30 Hz object stream of up to 4000 bits (DSV-17) per client, and the client's 800-bit player update at 28 Hz plus
//! an 887-bit reliable message every 300 ms. Our own harness; nothing of the 2009 wire changes.
// ORIGINAL: our own harness (fix-big-datagrams)

use dc_net_engine::link::MAX_DATAGRAM_BYTES;
use dc_net_engine::sim::{Faults, SimChannel};
use dc_net_engine::{Link, LinkConfig, LinkEvent};
use dc_wire::udp::{BitWriter, TODO_HOST_RATE_BPS};

const TICK_MS: u32 = 5;

/// A message of `bits` bits whose first 32 bits carry `n` (the rest a pattern), so order and integrity can be checked.
fn numbered(n: u32, bits: usize) -> BitWriter {
    let bytes: Vec<u8> = (0..bits.div_ceil(8))
        .map(|i| match i {
            0..=3 => n.to_le_bytes()[i],
            _ => (n as u8).wrapping_mul(31).wrapping_add(i as u8),
        })
        .collect();
    BitWriter::from_bits(&bytes, bits)
}

fn number_of(m: &BitWriter) -> u32 {
    let b = m.as_bytes();
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

#[derive(Default)]
struct Side {
    rel: Vec<BitWriter>,
    unrel_at: Vec<u32>,
    closed: bool,
    largest_out: usize,
}

fn take(events: Vec<LinkEvent>, chan: &mut SimChannel, now: u32, side: &mut Side) {
    for e in events {
        match e {
            LinkEvent::Transmit(d) => {
                side.largest_out = side.largest_out.max(d.len());
                chan.send(now, d);
            }
            LinkEvent::Message { reliable: true, bits } => side.rel.push(bits),
            LinkEvent::Message { reliable: false, .. } => side.unrel_at.push(now),
            LinkEvent::Closed { .. } => side.closed = true,
        }
    }
}

/// The reliable burst one client gets when `players` players join at once: per player the measured mix (player
/// info, loadout, preload list, roster lines, world objects), in bits.
fn join_burst(players: u32) -> Vec<usize> {
    let per_player = [
        5490, 2248, 937, 887, 620, 478, 478, 478, 478, 232, 128, 128, 120, 112, 112, 72, 72, 24,
    ];
    let mut out = Vec::new();
    for _ in 0..players {
        out.extend_from_slice(&per_player);
    }
    // the world objects and preload lists of a fresh client (the largest measured run of 0x08 messages)
    out.extend_from_slice(&[8532; 6]);
    out
}

struct Outcome {
    /// at the server: what the client sent
    server: Side,
    /// at the client: what the server sent
    client: Side,
    server_rel_sent: Vec<BitWriter>,
    client_rel_sent: Vec<BitWriter>,
    up: SimChannel,
    down: SimChannel,
    end: u32,
}

/// Server and client link on a path that drops every datagram above `limit` bytes (both ways), 40-60 ms one way and
/// 1 % random loss. At 2 s, 7 players join: the burst for 9 players is queued on the server.
fn run(limit: usize, seed: u64) -> Outcome {
    let faults = Faults {
        loss_permille: 10,
        min_delay_ms: 40,
        max_delay_ms: 60,
        ..Faults::default()
    };
    let mut up = SimChannel::new(seed * 2 + 1, faults);
    let mut down = SimChannel::new(seed * 2 + 2, faults);
    up.drop_over_bytes = Some(limit);
    down.drop_over_bytes = Some(limit);
    let mut server_link = Link::new(
        0,
        LinkConfig {
            hosting: true,
            rate_bps: TODO_HOST_RATE_BPS,
        },
    );
    let mut client_link = Link::new(0, LinkConfig::default());
    let mut o = Outcome {
        server: Side::default(),
        client: Side::default(),
        server_rel_sent: Vec::new(),
        client_rel_sent: Vec::new(),
        up,
        down,
        end: 20_000,
    };
    let mut n = 0u32;
    let mut next = |bits: usize| {
        n += 1;
        numbered(n, bits.max(40))
    };
    // the first client's own join: a 2-player burst
    for bits in join_burst(2) {
        let m = next(bits);
        o.server_rel_sent.push(m.clone());
        server_link.send_reliable(m);
    }
    let mut now = 0u32;
    while now < o.end {
        if now == 2_000 {
            for bits in join_burst(9) {
                let m = next(bits);
                o.server_rel_sent.push(m.clone());
                server_link.send_reliable(m);
            }
        }
        if now % 35 == 0 {
            // 30 Hz object stream, up to the DSV-17 budget
            server_link.send_unreliable(now, next(4060));
        }
        if now % 35 == 15 {
            client_link.send_unreliable(now, next(800));
        }
        if now % 300 == 0 {
            let m = next(887);
            o.client_rel_sent.push(m.clone());
            client_link.send_reliable(m);
        }
        for d in o.up.deliver(now) {
            server_link.handle_datagram(now, &d);
        }
        for d in o.down.deliver(now) {
            client_link.handle_datagram(now, &d);
        }
        take(server_link.poll(now), &mut o.down, now, &mut o.server);
        take(client_link.poll(now), &mut o.up, now, &mut o.client);
        now += TICK_MS;
    }
    o
}

/// Longest gap between unreliable arrivals in `[from, to)` (ms).
fn longest_gap(at: &[u32], from: u32, to: u32) -> u32 {
    let mut last = from;
    let mut gap = 0;
    for &t in at.iter().filter(|&&t| t >= from && t < to) {
        gap = gap.max(t - last);
        last = t;
    }
    gap.max(to - last)
}

fn check(limit: usize, seed: u64) {
    let o = run(limit, seed);
    assert!(!o.server.closed && !o.client.closed, "limit {limit}: a link closed");
    assert!(
        o.server.largest_out <= MAX_DATAGRAM_BYTES && o.client.largest_out <= MAX_DATAGRAM_BYTES,
        "limit {limit}: largest datagram server {} client {}",
        o.server.largest_out,
        o.client.largest_out
    );
    assert_eq!(
        (o.up.oversize_lost, o.down.oversize_lost),
        (0, 0),
        "limit {limit}: datagrams lost for their size"
    );
    // every reliable message arrived once, in order, intact
    assert_eq!(o.client.rel, o.server_rel_sent, "limit {limit}: server -> client reliable stream");
    let sent_before_end: Vec<u32> = o.client_rel_sent.iter().map(number_of).collect();
    let got: Vec<u32> = o.server.rel.iter().map(number_of).collect();
    assert!(
        sent_before_end.starts_with(&got) && got.len() + 2 >= sent_before_end.len(),
        "limit {limit}: client -> server reliable stream {} of {}",
        got.len(),
        sent_before_end.len()
    );
    // the unreliable streams never stop for long, burst or not (1 % loss: a few 35 ms ticks at most)
    let down_gap = longest_gap(&o.client.unrel_at, 1_000, o.end - 200);
    let up_gap = longest_gap(&o.server.unrel_at, 1_000, o.end - 200);
    assert!(down_gap < 400, "limit {limit}: object stream gap {down_gap} ms");
    assert!(up_gap < 400, "limit {limit}: player update gap {up_gap} ms");
    // and the burst is through within a few seconds
    let last = o.server_rel_sent.last().map(number_of);
    assert_eq!(o.client.rel.last().map(number_of), last);
}

#[test]
fn a_path_dropping_datagrams_over_1200_bytes_keeps_the_link_working() {
    for seed in 1..=3 {
        check(1200, seed);
    }
}

#[test]
fn a_path_dropping_datagrams_over_576_bytes_keeps_the_link_working() {
    for seed in 1..=3 {
        check(576, seed);
    }
}

/// The payload cap is the IP limit minus the 28 bytes of IPv4 and UDP headers.
#[test]
fn the_cap_fits_a_576_byte_ip_datagram() {
    assert_eq!(MAX_DATAGRAM_BYTES + 20 + 8, 576);
}

/// An unreliable message longer than the cap cannot be split by the 2009 framing: it is the only datagram that may
/// exceed the cap, it goes out alone and is counted.
#[test]
fn only_an_unreliable_message_longer_than_the_cap_exceeds_it() {
    let mut a = Link::new(0, LinkConfig::default());
    a.send_reliable(numbered(1, 30_000));
    a.send_unreliable(0, numbered(2, 6_000));
    a.send_unreliable(0, numbered(3, 800));
    let mut sizes = Vec::new();
    for e in a.poll(0) {
        if let LinkEvent::Transmit(d) = e {
            sizes.push(d.len());
        }
    }
    let over: Vec<usize> = sizes.iter().copied().filter(|&s| s > MAX_DATAGRAM_BYTES).collect();
    assert_eq!(over.len(), 1, "{sizes:?}");
    assert!(over[0] >= 750 && over[0] < 800, "{sizes:?}");
    assert_eq!(a.stats().oversize_out, 1);
    assert_eq!(a.stats().max_datagram_bytes as usize, over[0]);
}

/// NET-11: a link whose reliable stream is paused (its blocks unacknowledged) still sends its unreliable messages.
#[test]
fn unreliable_messages_flow_while_reliable_blocks_wait() {
    let mut a = Link::new(0, LinkConfig::default());
    for i in 0..40 {
        a.send_reliable(numbered(i, 2_000));
    }
    let mut now = 0;
    let mut unrel_datagrams_while_paused = 0;
    while now < 5_000 {
        a.send_unreliable(now, numbered(1000 + now, 800));
        let paused = a.is_paused();
        for e in a.poll(now) {
            if let LinkEvent::Transmit(d) = e {
                let body = dc_net_engine::body::parse_body(&d[1..]);
                if paused && body.rel.is_none() && !body.unrel.is_empty() {
                    unrel_datagrams_while_paused += 1;
                }
            }
        }
        now += 35;
    }
    assert!(a.is_paused(), "nothing acknowledged: the reliable stream waits");
    assert!(unrel_datagrams_while_paused > 100, "{unrel_datagrams_while_paused}");
}

/// NET-12: while its own block waits for an ack, a link still acknowledges the peer's data at least once a second
/// (the 2009 heartbeat interval grows with the wait).
#[test]
fn the_heartbeat_ack_continues_while_waiting_for_an_ack() {
    let mut a = Link::new(0, LinkConfig::default());
    let mut b = Link::new(0, LinkConfig::default());
    // b's first block reaches a, so a has something to acknowledge; a's own block is never acknowledged
    b.send_reliable(numbered(7, 64));
    for e in b.poll(0) {
        if let LinkEvent::Transmit(d) = e {
            a.handle_datagram(10, &d);
        }
    }
    a.poll(10);
    a.send_reliable(numbered(8, 64));
    let mut acks_at = Vec::new();
    let mut now = 20;
    while now < 10_000 {
        for e in a.poll(now) {
            if let LinkEvent::Transmit(d) = e {
                if dc_net_engine::body::parse_body(&d[1..]).ctl.is_some() {
                    acks_at.push(now);
                }
            }
        }
        now += 5;
    }
    let late: Vec<u32> = acks_at.iter().copied().filter(|&t| t > 5_000).collect();
    assert!(late.len() >= 4, "acks after 5 s: {late:?}");
}
