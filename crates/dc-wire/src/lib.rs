//! `dc-wire`: wire codecs of the 2009 Combat Arms client.
//!
//! - [`tcp`]: lobby / auth TCP framing (`FrameHeader8` + `InnerHeader6`) and packed field reader / writer
//!   (SPEC: PROTOCOL_2009.md §1.2).
//! - [`msg_ids`]: TCP message ids with the spec's names (SPEC: PROTOCOL_2009.md Appendix A).
//! - [`udp`]: game-connection transport: bit stream, out-of-band and link datagrams, field encodings
//!   (SPEC: UDP_TRANSPORT_2009.md, names from GLOSSARY_UDP.md).
//! - [`loginmods`]: the mod-set and content hashes our clients append to the login name (LG-271).
//! - [`entrymods`]: a featured server entry's mod set at the pick (our frames 0x3f10..0x3f12, LG-313).
//! - [`moddedlist`]: the other lobbies a lobby lists before its server list (our push 0x3f02, LG-272).
//! - [`gungame`]: a Gun Game room (our pushes 0x3f05 / 0x3f07, request 0x3f06, LG-305).
//! - [`possession`]: a room's Possession Arms and its move (our 0x3f0a / 0x3f0b, shell id 0xf2, LG-307, DSV-483).
//! - [`seatticket`]: the per-match seat ticket of the game hello (our push 0x3f0c, LG-327).
//! - [`roomhardcore`]: a room's Hardcore setting (our push 0x3f08 and request 0x3f09, LG-306).
//! - [`roomhacks`]: a modded room's Hacks setting (our push 0x3f03 and request 0x3f04, LG-274).
//! - [`remotehealth`]: other players' health in a wallhacks match (our shell id 0xf1, DSV-480).
//! - [`linkmove`]: the link move secret and its datagram trailer (our shell id 0xf3, NET-24).
//! - [`filetime`]: FILETIME / SYSTEMTIME helpers in local wall-clock time (SPEC: PROTOCOL_2009.md §1.2).
//!
//! No runtime dependencies (`std` only).

pub mod entrymods;
pub mod filetime;
pub mod flagrefusal;
pub mod gungame;
pub mod linkmove;
pub mod loginmods;
pub mod moddedlist;
pub mod msg_ids;
pub mod possession;
pub mod remotehealth;
pub mod report;
pub mod roomhacks;
pub mod roomhardcore;
pub mod seatticket;
pub mod tcp;
pub mod udp;

pub use filetime::{
    filetime_to_systemtime_civil, from_filetime_local, from_filetime_utc,
    local_filetime_after_days, now_unix_secs, systemtime_bytes_local, systemtime_bytes_now,
    systemtime_local, systemtime_to_filetime_civil, to_filetime_local, to_filetime_utc, SystemTime,
    EPOCH_DIFF, FT_DAY, HNS_PER_SEC, NO_END_FILETIME,
};
pub use msg_ids::*;
pub use tcp::{
    effective_packet_id, parse_tcp_frames, read_tcp_frame, FrameHeader8, InnerHeader6,
    OldProtocolHeader, PacketHeader8, PktReader, PktWriter, INNER_HEADER_SIZE,
    MAX_FRAME_PAYLOAD_SIZE, OUTER_HEADER_SIZE,
};
pub use udp::{BitReader, BitWriter};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, ErrorKind};

    // SPEC: PROTOCOL_2009.md §2.2 (`login_reply` 0x201: +6 u32 ok, +0xa u32 reason, ...; size >= 0x2e)
    #[test]
    fn writer_pads_with_at_and_patches_both_lengths() {
        let mut w = PktWriter::new(LOGIN_REPLY);
        w.u32(1).u32(1).u8(1).at(50);

        let payload = w.to_payload();
        assert_eq!(payload.len(), 50);
        assert_eq!(&payload[0..2], &[0x01, 0x02]);
        assert_eq!(&payload[2..6], &[50, 0, 0, 0]);
        assert_eq!(&payload[6..10], &[1, 0, 0, 0]);
        assert_eq!(&payload[10..14], &[1, 0, 0, 0]);
        assert_eq!(payload[14], 1);
        assert!(payload[15..50].iter().all(|&b| b == 0));

        let frame = w.to_frame();
        assert_eq!(frame.len(), 58);
        assert_eq!(&frame[0..4], &0x201i32.to_le_bytes());
        assert_eq!(&frame[4..8], &50i32.to_le_bytes());
        assert_eq!(&frame[8..], &payload[..]);

        let mut r = PktReader::new(&payload);
        assert_eq!(r.inner_id(), 0x201);
        assert_eq!(r.inner_size(), 50);
        assert_eq!(r.u32(), 1);
        assert_eq!(r.u32(), 1);
        assert_eq!(r.u8(), 1);
    }

    // SPEC: PROTOCOL_2009.md §3.3 (`lobby_login_reply` 0x301: +6 u32 ok, +0xe SYSTEMTIME 16 bytes)
    #[test]
    fn systemtime_field_roundtrip() {
        let st = SystemTime {
            year: 2009,
            month: 7,
            weekday: 4,
            day: 16,
            hour: 14,
            minute: 30,
            second: 45,
            milliseconds: 0,
        };
        let mut w = PktWriter::new(LOBBY_LOGIN_REPLY);
        w.u32(1).u32(0).raw(&st.to_bytes());
        let payload = w.to_payload();
        assert_eq!(payload.len(), 0x1e);

        let mut r = PktReader::new(&payload);
        assert_eq!(r.u32(), 1);
        r.at(0xe);
        let mut st_raw = [0u8; 16];
        st_raw.copy_from_slice(r.bytes(16));
        assert_eq!(SystemTime::from_bytes(st_raw), st);
    }

    // SPEC: PROTOCOL_2009.md §3.7 (`room_create` 0x382: +6 i32 max players; +0xa wstr[33] title; size 0x62)
    #[test]
    fn wstr_fields_roundtrip_and_truncate() {
        let mut req = PktWriter::new(ROOM_CREATE);
        req.i32(16).wstr("Snow Valley DM", 33).wstr("pass123", 9);
        let p = req.to_payload();
        assert_eq!(p.len(), 6 + 4 + 66 + 18);

        let mut r = PktReader::new(&p);
        assert_eq!(r.i32(), 16);
        assert_eq!(r.wstr(33), "Snow Valley DM");
        assert_eq!(r.wstr(9), "pass123");

        // truncation to n - 1 units keeps the terminator
        let mut t = PktWriter::empty();
        t.wstr("abcdef", 4);
        assert_eq!(t.raw_bytes(), &[b'a', 0, b'b', 0, b'c', 0, 0, 0]);
        let mut z = PktWriter::empty();
        z.wstr("x", 0);
        assert!(z.raw_bytes().is_empty());
    }

    // SPEC: PROTOCOL_2009.md §1.2 (inner-id overrides)
    #[test]
    fn effective_packet_id_overrides() {
        let p = PktWriter::new(EQUIP).u32(0).to_payload();
        assert_eq!(effective_packet_id(0x324, &p), EQUIP);

        let p = PktWriter::new(FUNCTION_EQUIP).u32(45).to_payload();
        assert_eq!(effective_packet_id(0x324, &p), FUNCTION_EQUIP);
        let p = PktWriter::new(FUNCTION_UNEQUIP).u8(1).to_payload();
        assert_eq!(effective_packet_id(0x324, &p), FUNCTION_UNEQUIP);

        let p = PktWriter::new(LIST_FETCH_3).u32(12).to_payload();
        assert_eq!(effective_packet_id(0x356, &p), LIST_FETCH_3);
        let p = PktWriter::new(LIST_FETCH_4).u32(2).to_payload();
        assert_eq!(effective_packet_id(0x356, &p), LIST_FETCH_4);

        let p = PktWriter::new(LIST_FETCH_2).u8(1).to_payload();
        assert_eq!(effective_packet_id(0x356, &p), LIST_FETCH_2);

        // an override pair only applies to its own frame id
        let p = PktWriter::new(FUNCTION_EQUIP).to_payload();
        assert_eq!(effective_packet_id(0x356, &p), 0x356);
        assert_eq!(effective_packet_id(0x200, &[]), 0x200);
    }

    #[test]
    fn reader_out_of_bounds_never_panics() {
        let short = [0x00u8, 0x02, 0x08, 0x00, 0x00, 0x00, 0x7f, 0xff];
        let mut r = PktReader::new(&short);
        assert_eq!(r.u32(), 0);
        assert_eq!(r.u64(), 0);
        assert_eq!(r.u16(), 0);
        assert_eq!(r.u8(), 0);
        assert_eq!(r.i32(), 0);
        assert_eq!(r.f32(), 0.0);
        assert_eq!(r.bytes(10), &[] as &[u8]);
        assert_eq!(r.bytes_padded(4), vec![0, 0, 0, 0]);
        assert_eq!(r.wstr(16), "");
    }

    // SPEC: PROTOCOL_2009.md §1.2 (frame check, max body 0x8000)
    #[test]
    fn frame_reader_and_cutter() {
        let f1 = PktWriter::new(SERVER_LIST_PUSH).u32(123).to_frame();
        let f2 = PktWriter::new(CHARACTER_INFO).to_frame();
        let mut stream = Vec::new();
        stream.extend_from_slice(&f1);
        stream.extend_from_slice(&f2);

        let mut cursor = Cursor::new(&stream);
        let (id1, pay1) = read_tcp_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(id1, 0x2000);
        assert_eq!(PktReader::new(&pay1).u32(), 123);

        let (id2, pay2) = read_tcp_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(id2, 0x302);
        assert_eq!(pay2.len(), INNER_HEADER_SIZE);

        assert!(read_tcp_frame(&mut cursor).unwrap().is_none());

        let mut partial_hdr = Cursor::new(&[0x05u8, 0x20, 0x00]);
        assert_eq!(
            read_tcp_frame(&mut partial_hdr).unwrap_err().kind(),
            ErrorKind::UnexpectedEof
        );

        let bad_hdr = FrameHeader8 {
            id: 0x200,
            size: 0x8001,
        }
        .to_bytes();
        let mut bad_cur = Cursor::new(&bad_hdr);
        assert_eq!(
            read_tcp_frame(&mut bad_cur).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        let neg_hdr = FrameHeader8 {
            id: 0x200,
            size: -1,
        }
        .to_bytes();
        assert_eq!(
            parse_tcp_frames(&neg_hdr).unwrap_err().kind(),
            ErrorKind::InvalidData
        );

        // exactly 0x8000 is legal
        let mut max = FrameHeader8 {
            id: 0x200,
            size: 0x8000,
        }
        .to_bytes()
        .to_vec();
        max.resize(8 + 0x8000, 0);
        let (frames, used) = parse_tcp_frames(&max).unwrap();
        assert_eq!((frames.len(), used), (1, max.len()));

        let mut stream_with_tail = stream.clone();
        stream_with_tail.extend_from_slice(&[0x01, 0x02, 0x00]);
        let (parsed, consumed) = parse_tcp_frames(&stream_with_tail).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(consumed, stream.len());
    }

    #[test]
    fn header_aliases_are_the_same_types() {
        let a: PacketHeader8 = FrameHeader8 { id: 1, size: 2 };
        assert_eq!(FrameHeader8::from_bytes(a.to_bytes()), a);
        let b: OldProtocolHeader = InnerHeader6 {
            id: 0x3043,
            size: 0x30,
        };
        assert_eq!(InnerHeader6::from_slice(&b.to_bytes()), Some(b));
        assert_eq!(InnerHeader6::from_slice(&[1, 2, 3]), None);
    }

    #[test]
    fn filetime_and_systemtime_roundtrips() {
        assert_eq!(to_filetime_utc(0), EPOCH_DIFF);
        assert_eq!(from_filetime_utc(EPOCH_DIFF), 0);

        let sample_ts = 1_700_000_000i64;
        assert_eq!(from_filetime_local(to_filetime_local(sample_ts)), sample_ts);

        let st = SystemTime {
            year: 2026,
            month: 10,
            weekday: 5,
            day: 23,
            hour: 23,
            minute: 15,
            second: 42,
            milliseconds: 125,
        };
        assert_eq!(
            filetime_to_systemtime_civil(systemtime_to_filetime_civil(st)),
            st
        );

        let ft0 = local_filetime_after_days(0);
        let ft30 = local_filetime_after_days(30);
        assert!(ft30.abs_diff(ft0 + 30 * FT_DAY) < 2 * HNS_PER_SEC);
        assert_eq!(NO_END_FILETIME, 0);
    }

    // SPEC: PROTOCOL_2009.md Appendix A
    #[test]
    fn message_ids_follow_the_spec() {
        assert_eq!(LOGIN_REQUEST, 0x200);
        assert_eq!(LOGIN_REPLY, 0x201);
        assert_eq!(SERVER_LIST_PUSH, 0x2000);
        assert_eq!(MATCH_ADDRESS_PUSH, 0x3043);
        assert_eq!(FUNCTION_ITEM_D, 0x364);
        assert_eq!(FUNCTION_ITEM_D_REPLY, 0x365);
        assert_eq!(DC_MESSENGER_PUSH, 0x3f01);
        assert_eq!(tcp_message_name(0x382), Some("room_create"));
        assert_eq!(tcp_message_name(0x331), None);
        // ids unique and sorted table complete
        let mut ids: Vec<u16> = TCP_MESSAGE_NAMES.iter().map(|(i, _)| *i).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
        // request / reply pairing (SPEC: PROTOCOL_2009.md §1.2): every *_reply below 0x2000 is request + 1
        for (id, name) in TCP_MESSAGE_NAMES {
            if *id < 0x2000 && name.ends_with("_reply") {
                assert_eq!(id & 1, 1, "{name} {id:#x}");
            }
        }
    }
}
