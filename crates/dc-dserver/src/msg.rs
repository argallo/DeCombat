//! Session messages of the game connection, server side: writers for the server -> client messages this slice
//! sends and a reader for every client -> server message. A message is one transport entry: `[msg_id:u8][body]`,
//! the body ends where the entry ends. Names are the glossary's (`GLOSSARY_UDP.md` S.A / S.B).

use dc_wire::udp::{BitReader, BitWriter};

// ------------------------------------------------------------------------------------------------- ids

/// `S2C_VERSION`: `[net_version:u32][host_rate_bps:u32]`.
// SPEC: UDP_SESSION_2009_A.md §4.2 (client engine+0xa2a70, host engine+0x8e9f0)
pub const S2C_VERSION: u8 = 4;
/// `S2C_LEAVE_WORLD`: no body.
// SPEC: UDP_SESSION_2009_A.md §4.10 (host engine+0x8ceb0)
pub const S2C_LEAVE_WORLD: u8 = 5;
/// `S2C_LOAD_WORLD`: `[srv_clock:f32][world_file:u16]`.
// SPEC: UDP_SESSION_2009_A.md §4.6 (host engine+0x8ddd0)
pub const S2C_LOAD_WORLD: u8 = 6;
/// `S2C_VIEW_OBJECT`: `[object_id:u16]`.
// SPEC: UDP_SESSION_2009_A.md §4.9 (host engine+0x8db60)
pub const S2C_VIEW_OBJECT: u8 = 7;
/// `S2C_OBJ_GROUP`: length-prefixed object / special records.
// SPEC: UDP_SESSION_2009_B.md §3.1 (host engine+0x8fe60)
pub const S2C_OBJ_GROUP: u8 = 8;
/// `S2C_OBJ_STREAM`: compact entries + time entry, unreliable.
// SPEC: UDP_SESSION_2009_B.md §4.3, §9.1 (client engine+0xa4160)
pub const S2C_OBJ_STREAM: u8 = 10;
/// `S2C_CLIENT_ID`: `[client_id:u16][local_host:u8]`.
// SPEC: UDP_SESSION_2009_A.md §4.3 (client engine+0xa2910)
pub const S2C_CLIENT_ID: u8 = 12;
/// `S2C_SHELL_MSG`: `[shell_id:u8][shell body]`.
// SPEC: UDP_SESSION_2009_B.md §6.1 (client engine+0xa5060)
pub const S2C_SHELL_MSG: u8 = 13;
/// `S2C_SHELL_OBJ_MSG` (id 17): a shell object message without an object (the effect dispatcher reads its
/// `[type:u8]` and the type's inline record).
// SPEC: UDP_SESSION_2009_B.md §6.3 (engine+0xa5140); GAME_MESSAGES_2009_S2C.md §8.1, §8.2
pub const S2C_SHELL_OBJ_MSG: u8 = 17;
/// `S2C_EVAR`: `[name:zstr8][value:zstr8]`.
// SPEC: UDP_SESSION_2009_A.md §4.5 (client engine+0xa50c0)
pub const S2C_EVAR: u8 = 15;
/// `S2C_SKY_TABLE`: `[sky_block:384 bits][count:u16 <= 30][count x u16]`.
// SPEC: UDP_SESSION_2009_A.md §4.8 (host engine+0x8d5a0)
pub const S2C_SKY_TABLE: u8 = 16;
/// `S2C_DIR_LIGHT`: `[dir: 3 x f32][color: 3 x f32][extra: f32]` (reliable).
// SPEC: UDP_SESSION_2009_B.md §8.4 (writer engine+0x8d6c0, reader engine+0xa31f0)
pub const S2C_DIR_LIGHT: u8 = 22;
/// `S2C_PRELOAD`: `[sub:u8][u16 ...]`.
// SPEC: UDP_SESSION_2009_A.md §4.7 (client engine+0xa45b0)
pub const S2C_PRELOAD: u8 = 19;
/// `S2C_FILE_LIST`: `{[file_id:u16][file_info:u32][name:zstr8]}` to the end of the message.
// SPEC: UDP_SESSION_2009_A.md §4.4 (client engine+0x3e990, host engine+0x3f3f0)
pub const S2C_FILE_LIST: u8 = 0x32;

/// `C2S_HELLO`: `[blob_len:u16][blob_len bytes]`.
// SPEC: UDP_SESSION_2009_A.md §5.2 (client engine+0x18d00)
pub const C2S_HELLO: u8 = 5;
/// `C2S_BYE`: no body; the host drops the link with bye code 5.
// SPEC: UDP_SESSION_2009_A.md §5.6 (host engine+0x90ae0)
pub const C2S_BYE: u8 = 6;
/// `C2S_RATE`: `[rate_8k:u16]` then `u16` ids to the end.
// SPEC: UDP_SESSION_2009_A.md §5.5, UDP_SESSION_2009_B.md §10.2 (host engine+0x90a80)
pub const C2S_RATE: u8 = 7;
/// `C2S_SOUND_ACK`: `[id:u16]*`.
// SPEC: UDP_SESSION_2009_B.md §2.5, §10.2 (host engine+0x90a10)
pub const C2S_SOUND_ACK: u8 = 8;
/// `C2S_LOAD_STATE`: `[state:u8]`, 0 = world loaded, 1 = preload done.
// SPEC: UDP_SESSION_2009_A.md §5.3 (host engine+0x90b20 DISASM)
pub const C2S_LOAD_STATE: u8 = 9;
/// `C2S_CONSOLE`: `[command:zstr8]`.
// SPEC: UDP_SESSION_2009_B.md §9.4 (host engine+0x92270)
pub const C2S_CONSOLE: u8 = 10;
/// `C2S_SHELL_MSG`: `[shell_id:u8][shell body]`.
// SPEC: UDP_SESSION_2009_B.md §11.1 (client engine+0x13f40)
pub const C2S_SHELL_MSG: u8 = 11;
/// `C2S_FILE_ACK`: `{entry:u16}` to the end, `entry = file_id | 0x8000` when accepted.
// SPEC: UDP_SESSION_2009_A.md §5.4 (client engine+0x3e990, host engine+0x3f740)
pub const C2S_FILE_ACK: u8 = 0x36;
/// Second file-list answer id the 2009 host accepts (content not described by the spec).
// SPEC: UDP_SESSION_2009_A.md §3.2 (engine+0x3f740 compares with 0x36 and 0x38)
pub const C2S_FILE_ANSWER_2: u8 = 0x38;

/// Engine protocol version the client requires.
// SPEC: UDP_SESSION_2009_A.md §4.2 (net_version must be 7, else result 0x4E)
pub const NET_VERSION: u32 = dc_wire::udp::ENGINE_PROTOCOL_VERSION;
/// Bit 15 of a `C2S_FILE_ACK` entry: the file was accepted.
// SPEC: UDP_SESSION_2009_A.md §5.4
pub const FILE_ACK_ACCEPTED: u16 = 0x8000;
/// `S2C_PRELOAD` sub 0: preload start.
// SPEC: UDP_SESSION_2009_A.md §4.7
pub const PRELOAD_START: u8 = 0;
/// `S2C_PRELOAD` sub 1: preload end, answer `C2S_LOAD_STATE(1)` requested.
// SPEC: UDP_SESSION_2009_A.md §4.7 (engine+0xa45b0 case 1)
pub const PRELOAD_END: u8 = 1;
/// `S2C_PRELOAD` sub 5: a list of sound file ids to load (meaning UNPROVEN, plausibly sounds).
// SPEC: UDP_SESSION_2009_A.md §4.7 (engine+0xa45b0 case 5); DC-DEFAULT: DSV-65 (proposed)
pub const PRELOAD_SOUNDS: u8 = 5;
/// Largest sky-table object count the client accepts (more -> result 1).
// SPEC: UDP_SESSION_2009_A.md §4.8
pub const SKY_TABLE_MAX: usize = 30;
/// Shell message id of the switching-worlds state.
// SPEC: UDP_SESSION_2009_A.md §4.11 (cs+0x1a86b0 case 0x88 -> cs+0x1a58f0)
pub const SHELL_SWITCH_STATE: u8 = 0x88;
/// Shell message id of the client's player update (client -> server).
// SPEC: UDP_SESSION_2009_B.md §10.3 (cshell+0x5f7f0, cshell+0x1fa060)
pub const SHELL_PLAYER_UPDATE: u8 = 0x64;
/// Size of the hello blob the 2009 client sends.
// SPEC: UDP_SESSION_2009_A.md §2.3, §5.2 (0x102 bytes)
pub const HELLO_BLOB_BYTES: usize = 0x102;

// ------------------------------------------------------------------------------------------------- writers

fn with_id(id: u8) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(id);
    w
}

/// `S2C_VERSION`: `04`, `07 00 00 00`, rate.
// SPEC: UDP_SESSION_2009_A.md §4.2, §9.2 S2
pub fn s2c_version(host_rate_bps: u32) -> BitWriter {
    let mut w = with_id(S2C_VERSION);
    w.write_u32(NET_VERSION).write_u32(host_rate_bps);
    w
}

/// `S2C_CLIENT_ID` with `local_host = 0` (a remote server).
// SPEC: UDP_SESSION_2009_A.md §4.3, §9.2 S3
pub fn s2c_client_id(client_id: u16) -> BitWriter {
    let mut w = with_id(S2C_CLIENT_ID);
    w.write_u16(client_id).write_u8(0);
    w
}

/// One entry of the session file table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub id: u16,
    /// Ignored by the client; DeCombat sends the file size (CD-59, proposed).
    pub info: u32,
    /// Path relative to the client's resource root, 8-bit.
    pub name: Vec<u8>,
}

/// `S2C_FILE_LIST` with the given entries (no count field).
// SPEC: UDP_SESSION_2009_A.md §4.4, §9.2 S4
pub fn s2c_file_list(entries: &[FileEntry]) -> BitWriter {
    let mut w = with_id(S2C_FILE_LIST);
    for e in entries {
        w.write_u16(e.id).write_u32(e.info).write_zstr8(&e.name);
    }
    w
}

/// `S2C_EVAR` (one engine variable).
// SPEC: UDP_SESSION_2009_A.md §4.5
pub fn s2c_evar(name: &[u8], value: &[u8]) -> BitWriter {
    let mut w = with_id(S2C_EVAR);
    w.write_zstr8(name).write_zstr8(value);
    w
}

/// `S2C_LOAD_WORLD`: server clock (seconds, f32) and the world's file id.
// SPEC: UDP_SESSION_2009_A.md §4.6, §9.2 S6
pub fn s2c_load_world(srv_clock: f32, world_file: u16) -> BitWriter {
    let mut w = with_id(S2C_LOAD_WORLD);
    w.write_f32(srv_clock).write_u16(world_file);
    w
}

/// `S2C_PRELOAD` with a sub op and an optional id list.
// SPEC: UDP_SESSION_2009_A.md §4.7, §9.2 S7
pub fn s2c_preload(sub: u8, ids: &[u16]) -> BitWriter {
    let mut w = with_id(S2C_PRELOAD);
    w.write_u8(sub);
    for &id in ids {
        w.write_u16(id);
    }
    w
}

/// `S2C_SKY_TABLE`: 48-byte block and at most 30 object ids.
// SPEC: UDP_SESSION_2009_A.md §4.8, §9.2 S8 (block layout UNPROVEN)
pub fn s2c_sky_table(block: &[u8; 48], ids: &[u16]) -> BitWriter {
    let ids = &ids[..ids.len().min(SKY_TABLE_MAX)];
    let mut w = with_id(S2C_SKY_TABLE);
    w.write_raw(block).write_u16(ids.len() as u16);
    for &id in ids {
        w.write_u16(id);
    }
    w
}

/// `S2C_DIR_LIGHT`: the world's global light (direction, colour in 0..255 units, convert-to-ambient).
// SPEC: UDP_SESSION_2009_B.md §8.4; RENDER_2009_addendum_styles.md §S2.1
pub fn s2c_dir_light(dir: [f32; 3], colour: [f32; 3], extra: f32) -> BitWriter {
    let mut w = with_id(S2C_DIR_LIGHT);
    for v in dir.into_iter().chain(colour) {
        w.write_f32(v);
    }
    w.write_f32(extra);
    w
}

/// `S2C_VIEW_OBJECT`: the client's own object.
// SPEC: UDP_SESSION_2009_A.md §4.9, §9.2 S10
pub fn s2c_view_object(object_id: u16) -> BitWriter {
    let mut w = with_id(S2C_VIEW_OBJECT);
    w.write_u16(object_id);
    w
}

/// `S2C_SHELL_OBJ_MSG` `[17]` then the effect record.
// SPEC: UDP_SESSION_2009_B.md §6.3
pub fn s2c_shell_obj_msg(body: &BitWriter) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(S2C_SHELL_OBJ_MSG).append(body);
    w
}

/// `S2C_SHELL_MSG`: `[13][shell_id][body]`.
// SPEC: UDP_SESSION_2009_B.md §6.1
pub fn s2c_shell_msg(shell_id: u8, body: &BitWriter) -> BitWriter {
    let mut w = with_id(S2C_SHELL_MSG);
    w.write_u8(shell_id).append(body);
    w
}

/// Shell message 0x88 with the switching-worlds state byte.
// SPEC: UDP_SESSION_2009_A.md §4.11, §9.2 S11 (the in-world test needs state 2 or 3)
pub fn s2c_switch_state(state: u8) -> BitWriter {
    let mut body = BitWriter::new();
    body.write_u8(state);
    s2c_shell_msg(SHELL_SWITCH_STATE, &body)
}

/// `S2C_LEAVE_WORLD`.
// SPEC: UDP_SESSION_2009_A.md §4.10
pub fn s2c_leave_world() -> BitWriter {
    with_id(S2C_LEAVE_WORLD)
}

// ------------------------------------------------------------------------------------------------- reader

/// A client -> server message.
#[derive(Debug, Clone, PartialEq)]
pub enum FromClient {
    /// `C2S_HELLO` with the blob bytes (`blob_len` bytes, fewer if the message was short).
    Hello(Vec<u8>),
    Bye,
    /// `C2S_RATE`: rate in 8000 bit/s units and the trailing tracked-sound ids.
    Rate {
        rate_8k: u16,
        ids: Vec<u16>,
    },
    SoundAck(Vec<u16>),
    LoadState(u8),
    Console(Vec<u8>),
    /// `C2S_SHELL_MSG`: shell id and the bits after it.
    Shell {
        shell_id: u8,
        body: BitWriter,
    },
    /// `C2S_FILE_ACK` entries (bit 15 = accepted).
    FileAck(Vec<u16>),
    /// An id the 2009 host ignores (dropped silently).
    Other(u8),
}

fn u16_list(r: &mut BitReader<'_>) -> Vec<u16> {
    let mut v = Vec::new();
    while r.remaining_bits() >= 16 {
        v.push(r.read_u16());
    }
    v
}

/// Reads one client message.
// SPEC: UDP_SESSION_2009_A.md §5.1, UDP_SESSION_2009_B.md §2.5 (server table: ids 5..11; 0x36 / 0x38 via the
// file announcer; everything else dropped)
pub fn read_from_client(msg: &BitWriter) -> FromClient {
    let mut r = msg.reader();
    let id = r.read_u8();
    match id {
        C2S_HELLO => {
            let len = r.read_u16() as usize;
            let n = len.min(r.remaining_bits() / 8);
            FromClient::Hello(r.read_raw(n))
        }
        C2S_BYE => FromClient::Bye,
        C2S_RATE => {
            let rate_8k = r.read_u16();
            FromClient::Rate {
                rate_8k,
                ids: u16_list(&mut r),
            }
        }
        C2S_SOUND_ACK => FromClient::SoundAck(u16_list(&mut r)),
        C2S_LOAD_STATE => FromClient::LoadState(r.read_u8()),
        C2S_CONSOLE => FromClient::Console(r.read_zstr8()),
        C2S_SHELL_MSG => {
            let shell_id = r.read_u8();
            let n = r.remaining_bits();
            FromClient::Shell {
                shell_id,
                body: r.read_bit_string(n),
            }
        }
        C2S_FILE_ACK => FromClient::FileAck(u16_list(&mut r)),
        other => FromClient::Other(other),
    }
}

/// The parts of the hello blob the server uses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hello {
    /// 8-bit copy of the name at blob offset 0x00 (at most 63 bytes).
    pub name8: Vec<u8>,
    /// Second 8-bit string at offset 0x40 (meaning UNPROVEN; logged only).
    pub extra8: Vec<u8>,
    /// UTF-16 name at offset 0x82, NUL-terminated.
    pub name16: Vec<u16>,
    /// The seat ticket after the 2009 blob, when the client sent one.
    // DC-DEFAULT: LG-327 (proposed, project decision)
    pub ticket: Option<dc_wire::seatticket::Ticket>,
}

impl Hello {
    /// The UTF-16 name as text (lossy).
    pub fn name(&self) -> String {
        String::from_utf16_lossy(&self.name16)
    }
}

fn cstr8(b: &[u8]) -> Vec<u8> {
    b.iter().take_while(|&&c| c != 0).copied().collect()
}

/// Parses the hello blob: 8-bit name (64 bytes at 0x00), second 8-bit string (64 bytes at 0x40), two zero bytes,
/// UTF-16 name at 0x82. A short blob yields empty fields.
// SPEC: UDP_SESSION_2009_A.md §2.3 (cs+0x2fac0), §5.2
pub fn parse_hello(blob: &[u8]) -> Hello {
    let part = |from: usize, len: usize| -> &[u8] {
        let a = from.min(blob.len());
        let b = (from + len).min(blob.len());
        &blob[a..b]
    };
    // the seat ticket after the 2009 blob is not part of the name
    let wide = part(
        0x82,
        blob.len()
            .min(dc_wire::seatticket::BLOB_OFFSET)
            .saturating_sub(0x82),
    );
    let name16 = wide
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&u| u != 0)
        .collect();
    Hello {
        name8: cstr8(part(0x00, 0x40)),
        extra8: cstr8(part(0x40, 0x40)),
        name16,
        ticket: dc_wire::seatticket::from_blob(blob),
    }
}

/// Builds a hello blob the way the 2009 shell fills it (for tests and the synthetic client).
// SPEC: UDP_SESSION_2009_A.md §2.3
pub fn build_hello_blob(name: &str, extra: &[u8]) -> Vec<u8> {
    let mut blob = vec![0u8; HELLO_BLOB_BYTES];
    let n8: Vec<u8> = name
        .chars()
        .map(|c| if (c as u32) < 0x100 { c as u8 } else { b'?' })
        .take(63)
        .collect();
    blob[..n8.len()].copy_from_slice(&n8);
    let e = &extra[..extra.len().min(63)];
    blob[0x40..0x40 + e.len()].copy_from_slice(e);
    for (i, u) in name.encode_utf16().take(63).enumerate() {
        blob[0x82 + 2 * i..0x84 + 2 * i].copy_from_slice(&u.to_le_bytes());
    }
    blob
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC: UDP_SESSION_2009_A.md §9.2 (byte strings of S2, S3, S6, S7, S10, S11)
    #[test]
    fn writers_match_the_slice_byte_strings() {
        assert_eq!(
            s2c_version(5_120_000).as_bytes(),
            [0x04, 0x07, 0, 0, 0, 0x00, 0x20, 0x4E, 0x00]
        );
        assert_eq!(s2c_client_id(1).as_bytes(), [0x0C, 0x01, 0x00, 0x00]);
        assert_eq!(
            s2c_load_world(0.0, 1).as_bytes(),
            [0x06, 0, 0, 0, 0, 0x01, 0x00]
        );
        assert_eq!(s2c_preload(PRELOAD_START, &[]).as_bytes(), [0x13, 0x00]);
        assert_eq!(s2c_preload(PRELOAD_END, &[]).as_bytes(), [0x13, 0x01]);
        assert_eq!(s2c_view_object(0x1234).as_bytes(), [0x07, 0x34, 0x12]);
        assert_eq!(s2c_switch_state(2).as_bytes(), [0x0D, 0x88, 0x02]);
        let mut sky = vec![0x10];
        sky.extend_from_slice(&[0u8; 48]);
        sky.extend_from_slice(&[0, 0]);
        assert_eq!(s2c_sky_table(&[0; 48], &[]).as_bytes(), &sky[..]);
        assert_eq!(s2c_leave_world().as_bytes(), [0x05]);
        assert_eq!(s2c_evar(b"a", b"1").as_bytes(), [0x0F, b'a', 0, b'1', 0]);
    }

    #[test]
    fn file_list_layout() {
        let w = s2c_file_list(&[
            FileEntry {
                id: 1,
                info: 0x0102_0304,
                name: b"x\\y.z".to_vec(),
            },
            FileEntry {
                id: 2,
                info: 0,
                name: b"m".to_vec(),
            },
        ]);
        assert_eq!(
            w.as_bytes(),
            [
                0x32, 0x01, 0x00, 0x04, 0x03, 0x02, 0x01, b'x', b'\\', b'y', b'.', b'z', 0x00,
                0x02, 0x00, 0, 0, 0, 0, b'm', 0x00
            ]
        );
    }

    #[test]
    fn client_messages() {
        let hello_blob = build_hello_blob("Dave", b"tag");
        let mut m = BitWriter::new();
        m.write_u8(C2S_HELLO).write_u16(HELLO_BLOB_BYTES as u16);
        m.write_raw(&hello_blob);
        assert_eq!(m.byte_len(), 3 + 258);
        let FromClient::Hello(blob) = read_from_client(&m) else {
            panic!()
        };
        let h = parse_hello(&blob);
        assert_eq!(h.name(), "Dave");
        assert_eq!(h.name8, b"Dave");
        assert_eq!(h.extra8, b"tag");

        let r = read_from_client(&BitWriter::from_bytes(&[0x07, 0x04, 0x00]));
        assert_eq!(
            r,
            FromClient::Rate {
                rate_8k: 4,
                ids: vec![]
            }
        );
        assert_eq!(
            read_from_client(&BitWriter::from_bytes(&[0x09, 0x01])),
            FromClient::LoadState(1)
        );
        assert_eq!(
            read_from_client(&BitWriter::from_bytes(&[0x36, 0x01, 0x80, 0x02, 0x00])),
            FromClient::FileAck(vec![0x8001, 0x0002])
        );
        assert_eq!(
            read_from_client(&BitWriter::from_bytes(&[0x06])),
            FromClient::Bye
        );
        assert_eq!(
            read_from_client(&BitWriter::from_bytes(&[0x0A, b'h', b'i', 0])),
            FromClient::Console(b"hi".to_vec())
        );
        assert_eq!(
            read_from_client(&BitWriter::from_bytes(&[0x99])),
            FromClient::Other(0x99)
        );
        match read_from_client(&BitWriter::from_bytes(&[0x0B, 0x64, 0x10, 0x00, 0x03])) {
            FromClient::Shell { shell_id, body } => {
                assert_eq!(shell_id, 0x64);
                assert_eq!(body.as_bytes(), [0x10, 0x00, 0x03]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn short_hello_is_harmless() {
        let h = parse_hello(b"ab");
        assert_eq!(h.name8, b"ab");
        assert!(h.name16.is_empty());
        let mut m = BitWriter::new();
        m.write_u8(C2S_HELLO).write_u16(500).write_u8(1);
        assert_eq!(read_from_client(&m), FromClient::Hello(vec![1]));
    }
}
