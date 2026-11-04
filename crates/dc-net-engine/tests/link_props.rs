//! Property tests: two links over a simulated lossy / duplicating / reordering channel deliver every reliable
//! message exactly once and in send order, in both directions, and never emit a malformed or over-budget
//! datagram. Our own harness; the channel is seeded so every failure replays.

use dc_net_engine::sim::{Faults, SimChannel, SimRng};
use dc_net_engine::{Link, LinkConfig, LinkEvent};
use dc_wire::udp::{decode_link_datagram, BitWriter, BODY_BUDGET_BITS};
use proptest::prelude::*;

fn make_msg(len_bits: usize, seed: u64) -> BitWriter {
    let mut rng = SimRng::new(seed);
    let bytes: Vec<u8> = (0..len_bits.div_ceil(8))
        .map(|_| rng.next_u64() as u8)
        .collect();
    BitWriter::from_bits(&bytes, len_bits)
}

#[derive(Default)]
struct Got {
    rel: Vec<BitWriter>,
    unrel: Vec<BitWriter>,
    closed: bool,
    malformed: usize,
    datagrams: usize,
}

#[derive(Debug)]
struct Plan {
    /// (send time, message) for reliable messages a -> b and b -> a, and unreliable a -> b.
    a_rel: Vec<(u32, BitWriter)>,
    b_rel: Vec<(u32, BitWriter)>,
    a_unrel: Vec<(u32, BitWriter)>,
}

struct Outcome {
    at_b: Got,
    at_a: Got,
    a: Link,
    b: Link,
    end: u32,
}

fn drain(events: Vec<LinkEvent>, chan: &mut SimChannel, now: u32, mine: &mut Got) {
    for e in events {
        match e {
            LinkEvent::Transmit(d) => {
                mine.datagrams += 1;
                if decode_link_datagram(&d).is_err() {
                    mine.malformed += 1;
                }
                chan.send(now, d);
            }
            LinkEvent::Message { reliable, bits } => {
                if reliable {
                    mine.rel.push(bits)
                } else {
                    mine.unrel.push(bits)
                }
            }
            LinkEvent::Closed { .. } => mine.closed = true,
        }
    }
}

fn simulate(
    seed: u64,
    faults: Faults,
    rate: u32,
    plan: Plan,
    lossy_until: u32,
    max_step: u32,
) -> Outcome {
    let mut rng = SimRng::new(seed ^ 0x5eed);
    let cfg_a = LinkConfig {
        hosting: true,
        rate_bps: rate,
    };
    let cfg_b = LinkConfig {
        hosting: false,
        rate_bps: rate,
    };
    let mut a = Link::new(0, cfg_a);
    let mut b = Link::new(0, cfg_b);
    let mut ab = SimChannel::new(seed, faults);
    let mut ba = SimChannel::new(seed.wrapping_add(1), faults);
    let (mut at_a, mut at_b) = (Got::default(), Got::default());
    let (mut ia, mut ib, mut iu) = (0, 0, 0);
    let want_b = plan.a_rel.iter().filter(|m| !m.1.is_empty()).count();
    let want_a = plan.b_rel.iter().filter(|m| !m.1.is_empty()).count();
    let mut now = 0u32;
    let limit = 400_000u32;
    while now < limit {
        if now >= lossy_until {
            ab.faults = Faults::clean(5);
            ba.faults = Faults::clean(5);
        }
        while ia < plan.a_rel.len() && plan.a_rel[ia].0 <= now {
            a.send_reliable(plan.a_rel[ia].1.clone());
            ia += 1;
        }
        while ib < plan.b_rel.len() && plan.b_rel[ib].0 <= now {
            b.send_reliable(plan.b_rel[ib].1.clone());
            ib += 1;
        }
        while iu < plan.a_unrel.len() && plan.a_unrel[iu].0 <= now {
            a.send_unreliable(now, plan.a_unrel[iu].1.clone());
            iu += 1;
        }
        for d in ab.deliver(now) {
            b.handle_datagram(now, &d);
        }
        for d in ba.deliver(now) {
            a.handle_datagram(now, &d);
        }
        drain(a.poll(now), &mut ab, now, &mut at_a);
        drain(b.poll(now), &mut ba, now, &mut at_b);
        let all_sent = ia == plan.a_rel.len() && ib == plan.b_rel.len() && iu == plan.a_unrel.len();
        if all_sent
            && now > lossy_until
            && at_b.rel.len() >= want_b
            && at_a.rel.len() >= want_a
            && a.is_idle()
            && b.is_idle()
        {
            break;
        }
        now += rng.range(1, max_step);
    }
    Outcome {
        at_b,
        at_a,
        a,
        b,
        end: now,
    }
}

fn len_strategy() -> impl Strategy<Value = usize> {
    prop_oneof![
        4 => 0usize..256,
        3 => 256usize..3000,
        1 => 3000usize..24_000,
    ]
}

fn rel_strategy() -> impl Strategy<Value = Vec<(u32, usize, u64)>> {
    prop::collection::vec((0u32..4000, len_strategy(), any::<u64>()), 0..30)
}

fn plan_strategy() -> impl Strategy<Value = Plan> {
    let unrel = prop::collection::vec((0u32..4000, 1usize..2000, any::<u64>()), 0..20);
    (rel_strategy(), rel_strategy(), unrel).prop_map(|(a, b, u)| {
        let mk = |v: Vec<(u32, usize, u64)>| {
            let mut v: Vec<(u32, BitWriter)> =
                v.into_iter().map(|(t, n, s)| (t, make_msg(n, s))).collect();
            v.sort_by_key(|m| m.0);
            v
        };
        Plan {
            a_rel: mk(a),
            b_rel: mk(b),
            a_unrel: mk(u),
        }
    })
}

fn faults_strategy() -> impl Strategy<Value = Faults> {
    (0u32..350, 0u32..150, 0u32..60, 0u32..250).prop_map(|(loss, dup, dmin, dspan)| Faults {
        loss_permille: loss,
        dup_permille: dup,
        corrupt_permille: 0,
        min_delay_ms: dmin,
        max_delay_ms: dmin + dspan,
    })
}

fn rate_strategy() -> impl Strategy<Value = u32> {
    prop_oneof![
        Just(35_840u32),
        Just(256_000u32),
        Just(5_120_000u32),
        Just(dc_wire::udp::INITIAL_LINK_RATE_BPS),
    ]
}

proptest! {
    // 64 cases by default; DC_PROP_CASES raises it for soak runs.
    #![proptest_config(ProptestConfig {
        cases: std::env::var("DC_PROP_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(64),
        .. ProptestConfig::default()
    })]

    #[test]
    fn reliable_delivery_is_exactly_once_and_in_order(
        seed in any::<u64>(),
        faults in faults_strategy(),
        rate in rate_strategy(),
        plan in plan_strategy(),
    ) {
        let want_b: Vec<BitWriter> = plan.a_rel.iter().map(|m| m.1.clone()).filter(|m| !m.is_empty()).collect();
        let want_a: Vec<BitWriter> = plan.b_rel.iter().map(|m| m.1.clone()).filter(|m| !m.is_empty()).collect();
        let sent_unrel: Vec<BitWriter> = plan.a_unrel.iter().map(|m| m.1.clone()).collect();
        let o = simulate(seed, faults, rate, plan, 15_000, 9);
        if std::env::var_os("DC_PROP_TRACE").is_some() {
            eprintln!("case: end {} ms, a->b {} rel / {} unrel, b->a {} rel, datagrams {} + {}, resends {} + {}",
                o.end, o.at_b.rel.len(), o.at_b.unrel.len(), o.at_a.rel.len(), o.at_a.datagrams, o.at_b.datagrams,
                o.a.stats().resends, o.b.stats().resends);
        }
        prop_assert!(!o.at_a.closed && !o.at_b.closed, "no link may close (ended at {} ms)", o.end);
        // liveness: after the channel heals every block is acknowledged and both links go idle
        prop_assert!(o.end < 60_000, "links still busy at {} ms: a {:?} b {:?}", o.end, o.a.stats(), o.b.stats());
        prop_assert_eq!(o.at_b.rel.len(), want_b.len(), "a -> b count (ended at {} ms)", o.end);
        prop_assert!(o.at_b.rel == want_b, "a -> b order / content");
        prop_assert_eq!(o.at_a.rel.len(), want_a.len(), "b -> a count");
        prop_assert!(o.at_a.rel == want_a, "b -> a order / content");
        for u in &o.at_b.unrel {
            prop_assert!(sent_unrel.contains(u), "unreliable message corrupted");
        }
        prop_assert!(o.at_b.unrel.len() <= 2 * sent_unrel.len());
        prop_assert_eq!(o.at_a.malformed + o.at_b.malformed, 0);
        prop_assert!(o.a.stats().max_body_bits as usize <= BODY_BUDGET_BITS);
        prop_assert!(o.b.stats().max_body_bits as usize <= BODY_BUDGET_BITS);
        prop_assert!(o.at_a.datagrams > 0 && o.at_b.datagrams > 0);
    }
}

#[test]
fn sequence_space_wraps_past_8191_with_loss() {
    // 11000 small messages, one per 3 ms, polled every 1 ms: more than 8192 reliable blocks, so rel_seq wraps
    // 8191 -> 0 (a few messages share a block while the sender is paused).
    let a_rel: Vec<(u32, BitWriter)> = (0..11_000u32)
        .map(|i| (i * 3, make_msg(24 + (i as usize % 40), i as u64)))
        .collect();
    let want: Vec<BitWriter> = a_rel.iter().map(|m| m.1.clone()).collect();
    let faults = Faults {
        loss_permille: 30,
        dup_permille: 20,
        corrupt_permille: 0,
        min_delay_ms: 1,
        max_delay_ms: 12,
    };
    let o = simulate(
        7,
        faults,
        5_120_000,
        Plan {
            a_rel,
            b_rel: vec![],
            a_unrel: vec![],
        },
        34_000,
        1,
    );
    assert!(!o.at_b.closed);
    assert_eq!(o.at_b.rel.len(), want.len());
    assert!(o.at_b.rel == want);
    assert!(
        o.a.stats().rel_blocks_out > 8192,
        "{}",
        o.a.stats().rel_blocks_out
    );
    assert!(o.b.stats().rel_stored > 0, "out-of-order slots were used");
}

#[test]
fn corrupted_datagrams_are_dropped_by_the_check_byte() {
    // Every corrupted datagram either fails check8 or (1 in 256) passes it; count the drops only.
    let faults = Faults {
        loss_permille: 0,
        dup_permille: 0,
        corrupt_permille: 1000,
        min_delay_ms: 1,
        max_delay_ms: 1,
    };
    let mut a = Link::new(0, LinkConfig::default());
    let mut b = Link::new(0, LinkConfig::default());
    let mut ab = SimChannel::new(3, faults);
    for i in 0..50u32 {
        a.send_reliable(make_msg(100, i as u64));
        for e in a.poll(i * 10) {
            if let LinkEvent::Transmit(d) = e {
                ab.send(i * 10, d);
            }
        }
        for d in ab.deliver(i * 10 + 1) {
            b.handle_datagram(i * 10 + 1, &d);
        }
    }
    // a pauses after 9 unacknowledged blocks (b never sees a valid one), so about 9 datagrams arrive
    let s = b.stats();
    assert!(s.datagrams_in >= 9, "{s:?}");
    assert_eq!(s.datagrams_in, ab.delivered);
    assert!(s.bad_check + 1 >= s.datagrams_in, "{s:?}");
    assert_eq!(
        s.delivered_rel + s.bad_check,
        s.datagrams_in.min(s.delivered_rel + s.bad_check)
    );
}

#[test]
fn unreliable_messages_expire_after_twice_max_latency_200() {
    // An unreliable message that flow control never lets out (7000 bits on an 8000 bps link: its estimate is
    // above 2W) is dropped once it waited longer than 2 * max(L_ms, 200) ms. The peer answers, so L_ms is small.
    let mut a = Link::new(
        0,
        LinkConfig {
            hosting: false,
            rate_bps: 8_000,
        },
    );
    let mut b = Link::new(0, LinkConfig::default());
    let (mut ab, mut ba) = (
        SimChannel::new(1, Faults::clean(1)),
        SimChannel::new(2, Faults::clean(1)),
    );
    let mut b_unrel = 0;
    let mut expired_at = None;
    for now in 0..1000u32 {
        if now == 10 {
            assert!(a.send_unreliable(now, make_msg(7000, 3)));
        }
        for d in ab.deliver(now) {
            b.handle_datagram(now, &d);
        }
        for d in ba.deliver(now) {
            a.handle_datagram(now, &d);
        }
        for e in a.poll(now) {
            if let LinkEvent::Transmit(d) = e {
                ab.send(now, d);
            }
        }
        for e in b.poll(now) {
            match e {
                LinkEvent::Transmit(d) => ba.send(now, d),
                LinkEvent::Message {
                    reliable: false, ..
                } => b_unrel += 1,
                _ => {}
            }
        }
        if expired_at.is_none() && a.stats().unrel_expired == 1 {
            expired_at = Some(now);
        }
    }
    assert_eq!(b_unrel, 0);
    let t = expired_at.expect("expired");
    assert!(
        t > 10 + 400 && t <= 10 + 401 + 1,
        "expired at {t}, L_ms {}",
        a.l_ms()
    );
}
