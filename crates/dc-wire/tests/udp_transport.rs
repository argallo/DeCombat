//! Tests of the UDP transport codecs against UDP_TRANSPORT_2009.md: fixed examples derived from the spec text
//! plus proptest round-trips of the bit stream, the link framing, the out-of-band datagrams and every encoding.

use dc_wire::udp::*;
use proptest::prelude::*;

// ------------------------------------------------------------------------------------------- fixed examples

// SPEC: UDP_TRANSPORT_2009.md §2.1 (bit k = bit k % 8 of byte k / 8, LSB first)
#[test]
fn bit_order_is_lsb_first() {
    let mut w = BitWriter::new();
    w.write_bit(true)
        .write_u8(0xA5)
        .write_bit(false)
        .write_u16(0x1234);
    // bit 0 = 1, bits 1..8 = 0xA5 -> byte 0 = (0xA5 << 1 | 1) & 0xFF = 0x4B, byte 1 bit 0 = 0xA5 >> 7 = 1
    assert_eq!(w.as_bytes()[0], 0x4B);
    assert_eq!(w.as_bytes()[1] & 1, 1);
    assert_eq!(w.bit_len(), 26);
    assert_eq!(w.byte_len(), 4);
    let mut r = w.reader();
    assert!(r.read_bit());
    assert_eq!(r.read_u8(), 0xA5);
    assert!(!r.read_bit());
    assert_eq!(r.read_u16(), 0x1234);
    assert!(r.at_end());
}

// SPEC: UDP_TRANSPORT_2009.md §2.3 (reads clamp; missing bits read as 0; position never passes the end)
#[test]
fn reads_past_the_end_are_zero() {
    let data = [0xFFu8];
    let mut r = BitReader::new(&data);
    assert_eq!(r.read_bits(4), 0xF);
    assert_eq!(r.read_bits(8), 0xF);
    assert_eq!(r.bit_pos(), 8);
    assert_eq!(r.read_u32(), 0);
    assert_eq!(r.bit_pos(), 8);
}

// SPEC: UDP_TRANSPORT_2009.md §6.1 (zlib CRC-32; public check value of "123456789" is 0xCBF43926)
#[test]
fn check8_is_the_xor_fold_of_crc32() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(check8(b"123456789"), 0xCB ^ 0xF4 ^ 0x39 ^ 0x26);
    assert_eq!(crc32(b""), 0);
}

// SPEC: UDP_TRANSPORT_2009.md §4, §5 (wire bytes 10 7F 33 01; sizes 25 / 5 / 22 / 8 bytes)
#[test]
fn oob_layouts_and_sizes() {
    let app_id = TODO_GAME_APP_ID_GUID_LAYOUT;
    let join = OobDatagram::JoinAsk {
        app_id,
        rate_bps: TODO_CLIENT_JOIN_RATE_BPS,
    }
    .encode();
    assert_eq!(&join[..4], &[0x10, 0x7F, 0x33, 0x01]);
    assert_eq!(join.len(), 25);
    let mut r = BitReader::new(&join);
    r.skip_bits(32);
    assert_eq!(r.read_bits(3), OOB_JOIN_ASK as u32); // V1: opcode bits 32-34 = 2
    assert_eq!(r.read_raw(16), app_id.to_vec()); // app id starts at stream bit 35
    assert_eq!(r.read_u32(), TODO_CLIENT_JOIN_RATE_BPS);

    let ok = OobDatagram::JoinReply {
        accepted: true,
        same_app: false,
    }
    .encode();
    assert_eq!(ok.len(), 5);
    assert!(BitReader::new(&ok).skip_bits(35).read_bit()); // V2: bit 35 = 1
    let refused = OobDatagram::JoinReply {
        accepted: false,
        same_app: true,
    }
    .encode();
    assert_eq!(refused.len(), 5);

    let info = OobDatagram::InfoAsk {
        app_id,
        probe_tag: PROBE_TAG_UNTIMED,
    }
    .encode();
    assert_eq!(info.len(), 22);
    let echo = OobDatagram::EchoAsk { echo_id: 0xABCDEF }.encode();
    assert_eq!(echo.len(), 8);
    assert!(is_oob_datagram(&echo));
    assert!(!is_oob_datagram(&echo[..3]));
    assert_eq!(OobDatagram::decode(&[0u8; 8]), Err(UdpError::NotOob));
}

// SPEC: UDP_TRANSPORT_2009.md §9.1 (bye body [1][CTL_BYE:4] or [1][CTL_BYE_CODE:4][bye_code:u32])
#[test]
fn bye_datagrams() {
    let plain = encode_bye_datagram(None);
    assert_eq!(plain.len(), 2);
    assert_eq!(plain[1], 0x03); // bit 0 = has_ctl, bits 1-4 = op 1
    assert_eq!(plain[0], check8(&plain[1..]));
    assert_eq!(
        decode_link_datagram(&plain).unwrap().ctl,
        Some(CtlBlock::Bye)
    );

    let coded = encode_bye_datagram(Some(DROP_STALE));
    assert_eq!(coded.len(), 1 + 5); // 37 body bits
    assert_eq!(coded[1], 0x07 | (3 << 5));
    let body = decode_link_datagram(&coded).unwrap();
    assert_eq!(body.ctl, Some(CtlBlock::ByeCode(3)));
    assert!(body.rel.is_none() && body.unrel.is_empty());
}

// SPEC: UDP_TRANSPORT_2009.md §8 (keep-alive reliable block [rel_seq][0][vlen 0][0]) and §11.1
#[test]
fn heartbeat_datagram() {
    let body = LinkBody {
        ctl: Some(CtlBlock::Ack {
            ack_seq: 0,
            sack_mask: None,
        }),
        rel: Some(RelBlock {
            rel_seq: FIRST_REL_SEQ,
            tail_cont: false,
            entries: vec![],
        }),
        unrel: vec![],
    };
    let bits = body.encode_bits();
    // has_ctl 1 + op 4 + ack 13 + has_sack 1 + has_rel 1 + seq 13 + tail 1 + vlen 8 + more 1
    assert_eq!(bits.bit_len(), 1 + 4 + 13 + 1 + 1 + 13 + 1 + 8 + 1);
    let d = encode_link_datagram(&body);
    let back = decode_link_datagram(&d).unwrap();
    assert_eq!(back.rel.as_ref().unwrap().entries, vec![BitWriter::new()]);

    let mut bad = d.clone();
    bad[0] ^= 1;
    assert!(matches!(
        decode_link_datagram(&bad),
        Err(UdpError::BadCheck { .. })
    ));
    assert_eq!(decode_link_datagram(&[]), Err(UdpError::Empty));
}

// SPEC: UDP_TRANSPORT_2009.md §8, §9 (an entry longer than the remaining bits abandons the block)
#[test]
fn overlong_entries_are_errors() {
    let mut w = BitWriter::new();
    w.write_bit(false)
        .write_bit(true)
        .write_bits(5, 13)
        .write_bit(false)
        .write_vlen(200);
    w.write_bits(0, 10);
    let d = seal_link_body(&w);
    assert!(matches!(
        decode_link_datagram(&d),
        Err(UdpError::RelEntryOverrun { claimed: 200, .. })
    ));

    let mut w = BitWriter::new();
    w.write_bit(false)
        .write_bit(false)
        .write_bit(true)
        .write_vlen(100);
    let d = seal_link_body(&w);
    assert!(matches!(
        decode_link_datagram(&d),
        Err(UdpError::UnrelEntryOverrun { claimed: 100, .. })
    ));
}

// SPEC: UDP_TRANSPORT_2009.md §10.1 (vlen layouts)
#[test]
fn vlen_examples() {
    for (len, bits) in [
        (0u32, 8u32),
        (127, 8),
        (128, 12),
        (1023, 12),
        (1024, 14),
        (2047, 14),
        (2048, 16),
    ] {
        let mut w = BitWriter::new();
        w.write_vlen(len);
        assert_eq!(w.bit_len() as u32, bits, "len {len}");
        assert_eq!(vlen_bits(len), bits);
        assert_eq!(w.reader().read_vlen(), len);
    }
    // 1024: [0:7][1][0:3][1] then v = 1: [1][0]
    let mut w = BitWriter::new();
    w.write_vlen(1024);
    let mut r = w.reader();
    assert_eq!(r.read_bits(7), 0);
    assert!(r.read_bit());
    assert_eq!(r.read_bits(3), 0);
    assert!(r.read_bit());
    assert!(r.read_bit());
    assert!(!r.read_bit());
}

// SPEC: UDP_TRANSPORT_2009.md §8.1, §10.2
#[test]
fn sequence_helpers() {
    assert_eq!(next_seq(8191), 0);
    assert_eq!(classify_rel_seq(1, 0), RelArrival::InOrder);
    assert_eq!(classify_rel_seq(0, 8191), RelArrival::InOrder);
    assert_eq!(classify_rel_seq(3, 8190), RelArrival::Ahead(4));
    assert_eq!(classify_rel_seq(9, 0), RelArrival::Ahead(8));
    assert_eq!(classify_rel_seq(10, 0), RelArrival::Discard);
    assert_eq!(classify_rel_seq(0, 0), RelArrival::Discard);
    assert_eq!(sack_bit(2, 0), Some(0));
    assert_eq!(sack_bit(9, 0), Some(7));
    assert_eq!(sack_bit(10, 0), None);
    assert_eq!(sack_bit(1, 8191), Some(0));
    assert_eq!(sack_seq(8190, 1), 1);
}

// SPEC: UDP_TRANSPORT_2009.md §11, §12
#[test]
fn timing_and_flow_formulas() {
    assert_eq!(rto_ms(5, false), 17);
    assert_eq!(rto_ms(5000, false), 1000);
    assert_eq!(rto_ms(300, true), 500);
    assert!(heartbeat_due(1200, 1000, 150));
    assert!(!heartbeat_due(1199, 1000, 150));
    assert!(heartbeat_due(5, u32::MAX - 300, 250)); // timeGetTime wrap
    assert_eq!(unrel_ttl_ms(50), 400);
    assert_eq!(datagram_cost_bits(0), 8 + 224);
    assert_eq!(datagram_cost_bits(1), 16 + 224);
    let w = flow_window_bits(TODO_CLIENT_JOIN_RATE_BPS);
    assert_eq!(w, 7168);
    assert!(!flow_blocked(w, 2 * w - 10, 0, 10));
    assert!(flow_blocked(w, 2 * w - 10, 0, 11));
    assert_eq!(flow_window_bits(INITIAL_LINK_RATE_BPS), 429_496_729);
}

// SPEC: UDP_TRANSPORT_2009.md §15.5 (identity rotation: F = +Z, U = +Y -> short form)
#[test]
fn rot_identity_uses_short_form() {
    let r = Rot48::encode([0.0, 0.0, 0.0, 1.0]);
    assert!(r.is_short());
    assert_eq!(&r.c[..3], &[-63, 63, 127]);
    let mut w = BitWriter::new();
    r.write(&mut w);
    assert_eq!(w.bit_len(), 24);
    let (f, u) = Rot48::decode_presumed(&r);
    assert!(
        (f[2] - 1.0).abs() < 0.02 && f[0].abs() < 0.02 && f[1].abs() < 0.02,
        "{f:?}"
    );
    assert!((u[1] - 1.0).abs() < 0.01, "{u:?}");
}

// SPEC: UDP_TRANSPORT_2009.md §15.2 (axis, sign bits, packing)
#[test]
fn vec72_example() {
    let v = Vec72::encode([3.0, -1.5, 0.75]);
    assert_eq!(v.major, 3.0);
    assert_eq!(v.major_axis(), 0);
    assert_eq!(v.q1(), 131_071);
    assert_eq!(v.q2(), 65_535);
    assert_eq!(v.vflags, 0x08 | 0x20);
    let d = v.decode_presumed();
    assert!(
        (d[1] + 1.5).abs() < 1e-4 && (d[2] - 0.75).abs() < 1e-4,
        "{d:?}"
    );
    // ties prefer x, then x/y over z
    assert_eq!(Vec72::encode([1.0, -1.0, 1.0]).major_axis(), 0);
    assert_eq!(Vec72::encode([0.0, 0.0, -2.0]).major_axis(), 2);
    assert_eq!(Vec72::encode([0.0, 0.0, 0.0]).vflags, 0);
}

// SPEC: UDP_TRANSPORT_2009.md §15.3
#[test]
fn yaw8_scale() {
    assert!((yaw8_to_radians(127) - std::f32::consts::PI).abs() < 1e-4);
    assert_eq!(yaw8_from_quat_presumed([0.0, 0.0, 0.0, 1.0]), 0);
}

// SPEC: UDP_TRANSPORT_2009.md §15.1 (cell offsets (4, 2, 4) / (0.5, 0.5, 0.5))
#[test]
fn pos48_decode_offsets() {
    let p = Pos48 {
        px: 0,
        py: 0,
        pz: 0,
    };
    let min = [0.0; 3];
    let max = [524_288.0, 262_144.0, 524_288.0];
    assert_eq!(p.decode(min, max, false, 0), [4.0, 2.0, 4.0]);
    assert_eq!(p.decode(min, max, true, 0), [0.5, 0.5, 0.5]);
    assert_eq!(p.decode(min, max, true, 0xFF), [7.5, 3.5, 7.5]);
}

// ------------------------------------------------------------------------------------------- strategies

fn bit_string() -> impl Strategy<Value = BitWriter> {
    (0usize..700).prop_flat_map(|bits| {
        prop::collection::vec(any::<u8>(), bits.div_ceil(8))
            .prop_map(move |b| BitWriter::from_bits(&b, bits))
    })
}

fn nonempty_bit_string() -> impl Strategy<Value = BitWriter> {
    bit_string().prop_filter("non-empty", |b| !b.is_empty())
}

fn ctl() -> impl Strategy<Value = CtlBlock> {
    prop_oneof![
        (0u16..8192, prop::option::of(any::<u8>())).prop_map(|(s, m)| CtlBlock::Ack {
            ack_seq: s,
            sack_mask: m
        }),
        Just(CtlBlock::Bye),
        any::<u32>().prop_map(CtlBlock::ByeCode),
        prop_oneof![Just(2u8), 4u8..16].prop_map(CtlBlock::Unknown),
    ]
}

fn rel() -> impl Strategy<Value = RelBlock> {
    (
        0u16..8192,
        any::<bool>(),
        prop::collection::vec(bit_string(), 1..5),
    )
        .prop_map(|(s, t, e)| RelBlock {
            rel_seq: s,
            tail_cont: t,
            entries: e,
        })
}

fn link_body() -> impl Strategy<Value = LinkBody> {
    (
        prop::option::of(ctl()),
        prop::option::of(rel()),
        prop::collection::vec(nonempty_bit_string(), 0..4),
    )
        .prop_map(|(ctl, rel, unrel)| LinkBody { ctl, rel, unrel })
}

#[derive(Debug, Clone)]
enum Field {
    Bits(u32, usize),
    Bits64(u64, usize),
    Zstr8(Vec<u8>),
    Zstr16(Vec<u16>),
    Lstr16(Vec<u8>),
    Lstr16h(Option<Vec<u8>>),
    Nest16(BitWriter),
    ObjRef(Option<u16>),
    Raw(Vec<u8>),
    Vlen(u32),
}

fn field() -> impl Strategy<Value = Field> {
    prop_oneof![
        (any::<u32>(), 0usize..=32).prop_map(|(v, n)| Field::Bits(v, n)),
        (any::<u64>(), 0usize..=64).prop_map(|(v, n)| Field::Bits64(v, n)),
        prop::collection::vec(1u8..=255, 0..20).prop_map(Field::Zstr8),
        prop::collection::vec(1u16..=u16::MAX, 0..20).prop_map(Field::Zstr16),
        prop::collection::vec(any::<u8>(), 0..40).prop_map(Field::Lstr16),
        prop::option::of(prop::collection::vec(any::<u8>(), 0..40)).prop_map(Field::Lstr16h),
        bit_string().prop_map(Field::Nest16),
        prop::option::of(0u16..0xFFFF).prop_map(Field::ObjRef),
        prop::collection::vec(any::<u8>(), 0..24).prop_map(Field::Raw),
        any::<u32>().prop_map(Field::Vlen),
    ]
}

fn mask(v: u64, n: usize) -> u64 {
    if n >= 64 {
        v
    } else {
        v & ((1u64 << n) - 1)
    }
}

fn unit_quat() -> impl Strategy<Value = [f32; 4]> {
    (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0)
        .prop_filter("non-degenerate", |(a, b, c, d)| {
            a * a + b * b + c * c + d * d > 0.05
        })
        .prop_map(|(a, b, c, d)| {
            let n = (a * a + b * b + c * c + d * d).sqrt();
            [a / n, b / n, c / n, d / n]
        })
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

// ------------------------------------------------------------------------------------------- properties

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    // SPEC: UDP_TRANSPORT_2009.md §2.1-§2.5, §2.7
    #[test]
    fn bit_stream_roundtrip(fields in prop::collection::vec(field(), 0..24), lead in 0usize..8) {
        let mut w = BitWriter::new();
        w.write_bits(0, lead);
        for f in &fields {
            match f {
                Field::Bits(v, n) => { w.write_bits(*v, *n); }
                Field::Bits64(v, n) => { w.write_bits64(*v, *n); }
                Field::Zstr8(s) => { w.write_zstr8(s); }
                Field::Zstr16(s) => { w.write_zstr16_presumed(s); }
                Field::Lstr16(s) => { w.write_lstr16_presumed(s); }
                Field::Lstr16h(s) => { w.write_lstr16h_presumed(s.as_deref()); }
                Field::Nest16(b) => { w.write_nest16_presumed(b); }
                Field::ObjRef(o) => { w.write_objref16(*o); }
                Field::Raw(b) => { w.write_raw(b); }
                Field::Vlen(l) => { w.write_vlen(*l); }
            }
        }
        prop_assert_eq!(w.byte_len(), w.bit_len().div_ceil(8));
        if !w.bit_len().is_multiple_of(8) {
            let last = *w.as_bytes().last().unwrap();
            prop_assert_eq!(last >> (w.bit_len() % 8), 0, "pad bits must be 0");
        }
        let mut r = w.reader();
        prop_assert_eq!(r.read_bits(lead), 0);
        for f in &fields {
            match f {
                Field::Bits(v, n) => prop_assert_eq!(r.read_bits(*n) as u64, mask(*v as u64, *n)),
                Field::Bits64(v, n) => prop_assert_eq!(r.read_bits64(*n), mask(*v, *n)),
                Field::Zstr8(s) => prop_assert_eq!(&r.read_zstr8(), s),
                Field::Zstr16(s) => prop_assert_eq!(&r.read_zstr16(), s),
                Field::Lstr16(s) => prop_assert_eq!(&r.read_lstr16(), s),
                Field::Lstr16h(s) => prop_assert_eq!(&r.read_lstr16h(), s),
                Field::Nest16(b) => prop_assert_eq!(&r.read_nest16(), b),
                Field::ObjRef(o) => prop_assert_eq!(r.read_objref16(), *o),
                Field::Raw(b) => prop_assert_eq!(&r.read_raw(b.len()), b),
                Field::Vlen(l) => prop_assert_eq!(r.read_vlen(), *l),
            }
        }
        prop_assert!(r.at_end());
        // the same bits read from the padded datagram bytes
        let bytes = w.to_bytes();
        let again = BitWriter::from_bits(&bytes, w.bit_len());
        prop_assert_eq!(&again, &w);
    }

    // SPEC: UDP_TRANSPORT_2009.md §2.8 (append keeps the exact bit length)
    #[test]
    fn append_is_concatenation(a in bit_string(), b in bit_string()) {
        let mut w = a.clone();
        w.append(&b);
        prop_assert_eq!(w.bit_len(), a.bit_len() + b.bit_len());
        let mut r = w.reader();
        prop_assert_eq!(&r.read_bit_string(a.bit_len()), &a);
        prop_assert_eq!(&r.read_bit_string(b.bit_len()), &b);
    }

    // SPEC: UDP_TRANSPORT_2009.md §10.1
    #[test]
    fn vlen_roundtrip_and_cost(len in any::<u32>()) {
        let mut w = BitWriter::new();
        w.write_vlen(len);
        prop_assert_eq!(w.bit_len() as u32, vlen_bits(len));
        prop_assert_eq!(w.reader().read_vlen(), len);
    }

    // SPEC: UDP_TRANSPORT_2009.md §6, §7, §8, §9
    #[test]
    fn link_datagram_roundtrip(body in link_body()) {
        let d = encode_link_datagram(&body);
        prop_assert_eq!(d[0], check8(&d[1..]));
        let back = decode_link_datagram(&d).unwrap();
        prop_assert_eq!(&back, &body);
        prop_assert_eq!(encode_link_datagram(&back), d);
    }

    // SPEC: UDP_TRANSPORT_2009.md §6.1 (a datagram whose check byte does not match is dropped)
    #[test]
    fn wrong_check_byte_is_rejected(body in link_body(), flip in 1u8..=255) {
        let mut d = encode_link_datagram(&body);
        d[0] ^= flip;
        let bad = matches!(decode_link_datagram(&d), Err(UdpError::BadCheck { .. }));
        prop_assert!(bad);
    }

    // SPEC: UDP_TRANSPORT_2009.md §5
    #[test]
    fn oob_roundtrip(
        app_id in any::<[u8; 16]>(),
        tag in any::<u8>(),
        rate in any::<u32>(),
        echo in 0u32..(1 << 24),
        name in prop::collection::vec(1u8..=255, 0..32),
        flag in any::<bool>(),
        count in any::<u8>(),
        hb in any::<u8>(),
        same_app in any::<bool>(),
        other in (6u8..8, bit_string()),
    ) {
        let cases = vec![
            OobDatagram::InfoAsk { app_id, probe_tag: tag },
            OobDatagram::InfoReply(None),
            OobDatagram::InfoReply(Some(InfoReply {
                session_name: name, probe_tag: tag, host_flag: flag, link_count: count, host_byte: hb,
            })),
            OobDatagram::JoinAsk { app_id, rate_bps: rate },
            OobDatagram::JoinReply { accepted: true, same_app: false },
            OobDatagram::JoinReply { accepted: false, same_app },
            OobDatagram::EchoAsk { echo_id: echo },
            OobDatagram::EchoReply { echo_id: echo },
        ];
        for c in cases {
            let d = c.encode();
            prop_assert!(is_oob_datagram(&d));
            prop_assert_eq!(OobDatagram::decode(&d).unwrap(), c);
        }
        // opcodes 6 / 7 keep their bits (plus pad): re-encoding the decoded datagram is stable
        let o = OobDatagram::Other { opcode: other.0, rest: other.1 };
        let d = o.encode();
        let back = OobDatagram::decode(&d).unwrap();
        prop_assert_eq!(back.encode(), d);
    }

    // SPEC: UDP_TRANSPORT_2009.md §15.1
    #[test]
    fn pos48_roundtrip(px in any::<u16>(), py in any::<u16>(), pz in any::<u16>(), lead in 0usize..8,
                       fx in 0.0f32..1.0, fy in 0.0f32..1.0, fz in 0.0f32..1.0) {
        let p = Pos48 { px, py, pz };
        let mut w = BitWriter::new();
        w.write_bits(0, lead);
        p.write(&mut w);
        prop_assert_eq!(w.bit_len(), lead + 48);
        let mut r = w.reader();
        r.skip_bits(lead);
        prop_assert_eq!(Pos48::read(&mut r), p);

        // presumed encoder against the PROVEN decoder: error within one fine cell (precise) or one coarse cell
        let min = [-4096.0f32, -1024.0, -4096.0];
        let max = [4096.0f32, 1024.0, 4096.0];
        let pos = [min[0] + fx * 8192.0, min[1] + fy * 2048.0, min[2] + fz * 8192.0];
        let (q, extra) = Pos48::encode_presumed(pos, min, max);
        let fine = q.decode(min, max, true, extra);
        let coarse = q.decode(min, max, false, 0);
        let cell = [8192.0 / 524_288.0, 2048.0 / 262_144.0, 8192.0 / 524_288.0];
        for i in 0..3 {
            prop_assert!((fine[i] - pos[i]).abs() <= cell[i] * 1.01, "fine axis {} {:?} {:?}", i, fine, pos);
            prop_assert!((coarse[i] - pos[i]).abs() <= cell[i] * 8.01, "coarse axis {} {:?} {:?}", i, coarse, pos);
        }
    }

    // SPEC: UDP_TRANSPORT_2009.md §15.2
    #[test]
    fn vec72_roundtrip(x in -2000.0f32..2000.0, y in -2000.0f32..2000.0, z in -2000.0f32..2000.0, lead in 0usize..8) {
        let v = Vec72::encode([x, y, z]);
        let mut w = BitWriter::new();
        w.write_bits(0, lead);
        v.write(&mut w);
        prop_assert_eq!(w.bit_len(), lead + 72);
        let mut r = w.reader();
        r.skip_bits(lead);
        let back = Vec72::read(&mut r);
        prop_assert_eq!(back.major.to_bits(), v.major.to_bits());
        prop_assert_eq!((back.r1, back.r2, back.vflags), (v.r1, v.r2, v.vflags));
        prop_assert!(v.major_axis() < 3);
        let d = v.decode_presumed();
        let mag = x.abs().max(y.abs()).max(z.abs());
        for (a, b) in d.iter().zip([x, y, z]) {
            prop_assert!((a - b).abs() <= mag * 1e-5 + 1e-6, "{:?} vs {:?}", d, [x, y, z]);
        }
    }

    // SPEC: UDP_TRANSPORT_2009.md §15.3 (read scale PROVEN; write scale and truncation lose at most one step)
    #[test]
    fn yaw8_roundtrip(v in -127i8..=127, lead in 0usize..8) {
        let mut w = BitWriter::new();
        w.write_bits(0, lead).write_s8(v);
        let mut r = w.reader();
        r.skip_bits(lead);
        prop_assert_eq!(r.read_s8(), v);
        let back = yaw8_from_radians_presumed(yaw8_to_radians(v));
        prop_assert!((back as i32 - v as i32).abs() <= 1, "{} -> {}", v, back);
        let half = yaw8_to_radians(v) * 0.5;
        let q = [0.0, half.sin(), 0.0, half.cos()];
        let tau = 2.0 * std::f32::consts::PI;
        let diff = (quat_yaw(q) - yaw8_to_radians(v)).rem_euclid(tau);
        prop_assert!(diff.min(tau - diff) < 1e-4);
    }

    // SPEC: UDP_TRANSPORT_2009.md §15.4
    #[test]
    fn quat128_roundtrip(q in any::<[u32; 4]>(), lead in 0usize..8) {
        let qf = q.map(f32::from_bits);
        let mut w = BitWriter::new();
        w.write_bits(0, lead);
        write_quat128(&mut w, qf);
        prop_assert_eq!(w.bit_len(), lead + 128);
        let mut r = w.reader();
        r.skip_bits(lead);
        prop_assert_eq!(read_quat128(&mut r).map(f32::to_bits), q);
    }

    // SPEC: UDP_TRANSPORT_2009.md §15.5
    #[test]
    fn rot48_roundtrip(q in unit_quat(), lead in 0usize..8) {
        let rot = Rot48::encode(q);
        let mut w = BitWriter::new();
        w.write_bits(0, lead);
        rot.write(&mut w);
        prop_assert_eq!(w.bit_len(), lead + if rot.is_short() { 24 } else { 48 });
        let mut r = w.reader();
        r.skip_bits(lead);
        prop_assert_eq!(Rot48::read(&mut r), rot);

        let m = quat_to_matrix(q);
        let f = [m[0][2], m[1][2], m[2][2]];
        let u = [m[0][1], m[1][1], m[2][1]];
        let (df, du) = rot.decode_presumed();
        prop_assert!((df[0] - f[0]).abs() < 0.04 && (df[1] - f[1]).abs() < 0.04 && (df[2] - f[2]).abs() < 0.04,
            "forward {:?} vs {:?}", df, f);
        let nu = dot(du, du).sqrt();
        prop_assert!(dot(du, u) / nu > 0.97, "up {:?} vs {:?} (short {})", du, u, rot.is_short());
    }
}
