//! Link silence after the session layer's `C2S_BYE` (PROVEN-CAPTURE tdm1): after `Link::quiesce` nothing leaves
//! the link except the explicit `Link::send_plain_bye` burst; the host side keeps streaming, closes with bye code 5
//! (4 copies) and the quiet link reports it.
// SPEC: UDP_TRANSPORT_2009.md §9.1; CAPTURE_tdm1_TRANSPORT_VALIDATION.md C-T12

use dc_net_engine::sim::{Faults, SimChannel};
use dc_net_engine::{
    Bye, CloseCause, Host, HostConfig, HostEvent, JoinStatus, Joiner, Link, LinkConfig, LinkEvent,
    LinkState,
};
use dc_wire::udp::{
    decode_link_datagram, BitWriter, CtlBlock, TODO_CLIENT_JOIN_RATE_BPS,
    TODO_GAME_APP_ID_GUID_LAYOUT,
};

const CLIENT: u32 = 7;

fn host_cfg() -> LinkConfig {
    LinkConfig {
        hosting: true,
        ..LinkConfig::default()
    }
}

fn transmits(ev: &[LinkEvent]) -> Vec<Vec<u8>> {
    ev.iter()
        .filter_map(|e| match e {
            LinkEvent::Transmit(d) => Some(d.clone()),
            _ => None,
        })
        .collect()
}

fn is_plain_bye(d: &[u8]) -> bool {
    matches!(
        decode_link_datagram(d).map(|b| b.ctl),
        Ok(Some(CtlBlock::Bye))
    )
}

/// Two raw links; `a` quiesces at t=1000 while `b` keeps sending reliable and unreliable data every 20 ms.
#[test]
fn nothing_leaves_a_quiet_link_but_the_explicit_bye_burst() {
    let mut a = Link::new(0, LinkConfig::default());
    let mut b = Link::new(0, host_cfg());
    let mut ab = SimChannel::new(1, Faults::clean(5));
    let mut ba = SimChannel::new(2, Faults::clean(5));
    let mut a_out_after_quiet: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut a_delivered_after_quiet = 0;
    for now in 0..20_000u32 {
        if now == 1000 {
            a.quiesce();
            assert!(a.is_quiet());
        }
        if now == 12_000 {
            a.send_plain_bye(now);
        }
        if now % 20 == 0 {
            b.send_reliable(BitWriter::from_bytes(&[9, (now / 20) as u8, 0, 0]));
            b.send_unreliable(now, BitWriter::from_bytes(&[3, 3, 3]));
        }
        if now < 1000 && now % 50 == 0 {
            a.send_reliable(BitWriter::from_bytes(&[2, 2]));
        }
        if (1000..12_000).contains(&now) {
            // queuing on a quiet link is accepted but never sent
            a.send_reliable(BitWriter::from_bytes(&[2, 2]));
        }
        for d in ba.deliver(now) {
            a.handle_datagram(now, &d);
        }
        for d in ab.deliver(now) {
            b.handle_datagram(now, &d);
        }
        for e in a.poll(now) {
            match e {
                LinkEvent::Transmit(d) => {
                    if now >= 1000 {
                        a_out_after_quiet.push((now, d.clone()));
                    }
                    ab.send(now, d);
                }
                LinkEvent::Message { .. } if now >= 1000 => a_delivered_after_quiet += 1,
                _ => {}
            }
        }
        for e in b.poll(now) {
            if let LinkEvent::Transmit(d) = e {
                ba.send(now, d);
            }
        }
    }
    assert!(
        a_delivered_after_quiet > 100,
        "quiet link still parses and delivers"
    );
    for (_, d) in &a_out_after_quiet {
        assert!(is_plain_bye(d));
    }
    let burst: Vec<u32> = a_out_after_quiet.iter().map(|(t, _)| *t).collect();
    assert_eq!(burst, vec![12_000, 12_100, 12_200, 12_300]);
    assert!(a.is_finished());
    // the other side received the plain bye (hosting side reports code 0)
    assert_eq!(b.state(), LinkState::Closed);
}

/// A block arriving in the same tick as the quiesce is not acknowledged.
#[test]
fn block_arriving_after_quiesce_is_not_acked() {
    let mut host = Link::new(0, host_cfg());
    let mut c = Link::new(0, LinkConfig::default());
    host.send_reliable(BitWriter::from_bytes(&[9, 1, 2, 3]));
    let out = transmits(&host.poll(0));
    assert!(!out.is_empty());
    c.quiesce();
    for d in &out {
        c.handle_datagram(1, d);
    }
    let ev = c.poll(1);
    assert!(ev
        .iter()
        .any(|e| matches!(e, LinkEvent::Message { reliable: true, .. })));
    assert!(transmits(&ev).is_empty());
    let mut t = 1;
    while t < 5000 {
        t += 10;
        assert!(transmits(&c.poll(t)).is_empty(), "t={t}");
    }
    assert_eq!(c.stats().acks_out, 0);
}

#[test]
fn quiet_idle_drop_is_silent() {
    let mut c = Link::new(0, LinkConfig::default());
    c.quiesce();
    let mut closed = None;
    for t in (0..95_000).step_by(10) {
        for e in c.poll(t) {
            match e {
                LinkEvent::Transmit(_) => panic!("datagram from a quiet link at {t}"),
                LinkEvent::Closed { code, cause } => closed = Some((code, cause)),
                _ => {}
            }
        }
    }
    assert_eq!(closed, Some((3, CloseCause::IdleTimeout)));
}

/// Plain bye after the peer's bye already closed the link (the capture: 8.7 s later), once only.
#[test]
fn plain_bye_after_peer_bye_sends_four_copies_once() {
    let mut host = Link::new(0, host_cfg());
    let mut c = Link::new(0, LinkConfig::default());
    c.quiesce();
    host.close(0, Bye::Code(5));
    let mut closed = None;
    let mut sent = Vec::new();
    for t in 0..10_000u32 {
        for d in transmits(&host.poll(t)) {
            c.handle_datagram(t, &d);
        }
        for ev in c.poll(t) {
            match ev {
                LinkEvent::Closed { code, cause } => {
                    assert!(closed.is_none(), "closed once");
                    closed = Some((code, cause));
                }
                LinkEvent::Transmit(d) => {
                    assert!(t >= 8700, "quiet link transmitted at {t}");
                    assert!(is_plain_bye(&d));
                    sent.push(t);
                }
                _ => {}
            }
        }
        if t == 8700 {
            assert_eq!(c.state(), LinkState::Closed);
            c.send_plain_bye(t);
            c.send_plain_bye(t);
            for d in transmits(&c.poll(t)) {
                assert!(is_plain_bye(&d));
                sent.push(t);
            }
        }
    }
    assert_eq!(closed, Some((5, CloseCause::PeerBye)));
    assert_eq!(sent, vec![8700, 8800, 8900, 9000]);
    assert!(c.is_finished());
}

/// Host + joiner end to end: the client leaves (session `C2S_BYE` modelled as a reliable message that makes the
/// host close with code 5), goes quiet, keeps receiving the host's stream, reports the host's bye, and later sends
/// the plain burst.
#[test]
fn host_streams_to_quiet_peer_then_closes_with_code_5_x4() {
    let mut host: Host<u32> = Host::new(HostConfig::default());
    let mut joiner = Joiner::new(TODO_GAME_APP_ID_GUID_LAYOUT, TODO_CLIENT_JOIN_RATE_BPS);
    let mut up = SimChannel::new(1, Faults::clean(3));
    let mut down = SimChannel::new(2, Faults::clean(3));
    let mut client: Option<Link> = None;
    let mut client_closed = None;
    let mut host_closed = None;
    let mut code5 = Vec::new();
    let mut client_tx_after_bye: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut quiet_msgs = 0usize;
    for now in 0..15_000u32 {
        if let Some(d) = joiner.poll(now) {
            up.send(now, d);
        }
        for d in up.deliver(now) {
            host.handle_datagram(now, CLIENT, &d);
        }
        for _pass in 0..2 {
            let mut close_now = false;
            for e in host.poll(now) {
                match e {
                    HostEvent::Transmit { bytes, .. } => {
                        if let Ok(b) = decode_link_datagram(&bytes) {
                            if matches!(b.ctl, Some(CtlBlock::ByeCode(5))) {
                                code5.push(now);
                            }
                        }
                        down.send(now, bytes)
                    }
                    HostEvent::Message { bits, reliable, .. } => {
                        if reliable && bits.as_bytes().first() == Some(&6) {
                            close_now = true;
                        }
                    }
                    HostEvent::LinkClosed { code, cause, .. } => host_closed = Some((code, cause)),
                    _ => {}
                }
            }
            if close_now {
                // the session layer's C2S_BYE handler: the host answers with bye code 5
                host.disconnect(now, &CLIENT, Bye::Code(5));
            }
        }
        for d in down.deliver(now) {
            match client.as_mut() {
                Some(l) => l.handle_datagram(now, &d),
                None => {
                    if joiner.handle_datagram(&d) == JoinStatus::Accepted {
                        client = Some(joiner.clone().into_link(now).unwrap());
                    }
                }
            }
        }
        if now < 1500 && now % 10 == 0 {
            let _ = host.send_reliable(&CLIENT, BitWriter::from_bytes(&[10, 0, 0, 0]));
        }
        if let Some(l) = client.as_mut() {
            if now == 1000 {
                l.send_reliable(BitWriter::from_bytes(&[6]));
            }
            if now == 1001 {
                l.quiesce();
            }
            if now == 11_000 {
                l.send_plain_bye(now);
            }
            for e in l.poll(now) {
                match e {
                    LinkEvent::Transmit(d) => {
                        if now >= 1001 {
                            client_tx_after_bye.push((now, d.clone()));
                        }
                        up.send(now, d);
                    }
                    LinkEvent::Message { .. } if now > 1001 => quiet_msgs += 1,
                    LinkEvent::Closed { code, cause } => client_closed = Some((code, cause)),
                    _ => {}
                }
            }
        }
    }
    assert!(quiet_msgs > 0, "the quiet client kept receiving");
    assert_eq!(code5.len(), 4, "{code5:?}");
    for w in code5.windows(2) {
        assert_eq!(w[1] - w[0], 100);
    }
    assert_eq!(client_closed, Some((5, CloseCause::PeerBye)));
    assert_eq!(host_closed, Some((5, CloseCause::Local)));
    let times: Vec<u32> = client_tx_after_bye
        .iter()
        .filter(|(_, d)| is_plain_bye(d))
        .map(|(t, _)| *t)
        .collect();
    assert_eq!(times, vec![11_000, 11_100, 11_200, 11_300]);
    assert_eq!(client_tx_after_bye.len(), 4, "nothing else after quiesce");
}

/// `quiesce_when_flushed`: the queued last block (the `C2S_BYE` carrier) leaves, then nothing does.
#[test]
fn quiesce_when_flushed_sends_the_queued_block_then_nothing() {
    let mut host = Link::new(0, host_cfg());
    let mut c = Link::new(0, LinkConfig::default());
    c.send_reliable(BitWriter::from_bytes(&[6]));
    c.quiesce_when_flushed();
    let out = transmits(&c.poll(0));
    assert_eq!(out.len(), 1);
    assert!(c.is_quiet());
    host.handle_datagram(1, &out[0]);
    let got = host.poll(1);
    assert!(got
        .iter()
        .any(|e| matches!(e, LinkEvent::Message { reliable: true, .. })));
    // the host's ack and later traffic do not make the quiet link talk
    for t in (0..3000).step_by(10) {
        for d in transmits(&host.poll(t)) {
            c.handle_datagram(t, &d);
        }
        assert!(transmits(&c.poll(t)).is_empty(), "t={t}");
    }
}
