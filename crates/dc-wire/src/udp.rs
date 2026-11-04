//! UDP transport of the 2009 game connection: bit stream, out-of-band datagrams, link datagrams (check byte,
//! control / reliable / unreliable blocks) and the compact field encodings that messages use.
//!
//! Written only from `rust/specs/UDP_TRANSPORT_2009.md` and `rust/specs/GLOSSARY_UDP.md`; every name below is
//! the glossary's (or ours where the glossary has none). What a delivered message means is the session-layer
//! spec (`UDP_SESSION_2009.md`), not this module. The delivery state machine (resend, flow control, liveness)
//! is described by the spec; this module provides its constants and pure helper formulas, not the machine.
//!
//! Items the spec marks UNPROVEN are `TODO_*` constants (or functions named `*_presumed`) with a doc comment
//! that says which validation step (V1..V20 of spec §18) settles them.

use std::fmt;

// ------------------------------------------------------------------------------------------------- constants

/// First 32 bits of every out-of-band datagram (wire bytes `10 7F 33 01`).
// SPEC: UDP_TRANSPORT_2009.md §4, §5 (EVIDENCE engine+0xb5d70, engine+0xb6850)
pub const OOB_MAGIC: u32 = 0x0133_7F10;
/// Width of the out-of-band opcode that follows [`OOB_MAGIC`].
// SPEC: UDP_TRANSPORT_2009.md §5
pub const OOB_OPCODE_BITS: u32 = 3;

/// Out-of-band opcode: info probe ask (`app_id:128`, `probe_tag:u8`).
// SPEC: UDP_TRANSPORT_2009.md §5 (engine+0xb4260)
pub const OOB_INFO_ASK: u8 = 0;
/// Out-of-band opcode: info probe reply.
// SPEC: UDP_TRANSPORT_2009.md §5 (engine+0xb5120 case 0)
pub const OOB_INFO_REPLY: u8 = 1;
/// Out-of-band opcode: join ask (`app_id:128`, `rate_bps:u32`).
// SPEC: UDP_TRANSPORT_2009.md §5 (engine+0xb6850)
pub const OOB_JOIN_ASK: u8 = 2;
/// Out-of-band opcode: join reply (`accepted:u1`, if 0 `same_app:u1`).
// SPEC: UDP_TRANSPORT_2009.md §5 (engine+0xb5120 case 2)
pub const OOB_JOIN_REPLY: u8 = 3;
/// Out-of-band opcode: echo ask (`echo_id:u24`).
// SPEC: UDP_TRANSPORT_2009.md §5 (engine+0xb71e0)
pub const OOB_ECHO_ASK: u8 = 4;
/// Out-of-band opcode: echo reply (`echo_id:u24`).
// SPEC: UDP_TRANSPORT_2009.md §5 (engine+0xb5120 case 4 / 5)
pub const OOB_ECHO_REPLY: u8 = 5;

/// Width of `ctl_op` at the start of a control block.
// SPEC: UDP_TRANSPORT_2009.md §7
pub const CTL_OP_BITS: u32 = 4;
/// Control op: ack (`ack_seq:u13`, `has_sack:u1`, `sack_mask:u8`), also the periodic keep-alive.
// SPEC: UDP_TRANSPORT_2009.md §7, §10.2 (engine+0xabc10, engine+0xb2500)
pub const CTL_ACK: u8 = 0;
/// Control op: disconnect without a code.
// SPEC: UDP_TRANSPORT_2009.md §7 (engine+0xb3920)
pub const CTL_BYE: u8 = 1;
/// Control op: disconnect with `bye_code:u32`.
// SPEC: UDP_TRANSPORT_2009.md §7 (engine+0xb3a10)
pub const CTL_BYE_CODE: u8 = 3;

/// Width of `rel_seq` and `ack_seq`.
// SPEC: UDP_TRANSPORT_2009.md §8 (engine+0xb2e20)
pub const SEQ_BITS: u32 = 13;
/// Mask of the 13-bit sequence space (`rel_seq` wraps 8191 -> 0).
// SPEC: UDP_TRANSPORT_2009.md §8, §8.1
pub const SEQ_MASK: u16 = 0x1FFF;
/// `rel_seq` of the first reliable block of a link.
// SPEC: UDP_TRANSPORT_2009.md §8 (counters start at 0 and are pre-incremented, engine+0xb48b0)
pub const FIRST_REL_SEQ: u16 = 1;
/// Number of out-of-order slots on the receiver; accepted window is 1..8 ahead of `expected`.
// SPEC: UDP_TRANSPORT_2009.md §8.1 (engine+0xaf450, engine+0xb2690)
pub const OUT_OF_ORDER_SLOTS: usize = 8;
/// Width of `sack_mask`.
// SPEC: UDP_TRANSPORT_2009.md §7, §10.2
pub const SACK_BITS: u32 = 8;
/// Only messages longer than this many bits are split across reliable blocks.
// SPEC: UDP_TRANSPORT_2009.md §8.2 (engine+0xb2e20)
pub const SPLIT_THRESHOLD_BITS: usize = 255;
/// Datagram body budget in bits (reliable block also `<= W / 2`).
// SPEC: UDP_TRANSPORT_2009.md §11.2, §11.4 (engine+0xb2e20, engine+0xaf560)
pub const BODY_BUDGET_BITS: usize = 8800;
/// Receive buffer of the peer; a larger datagram fails `recvfrom` and is dropped.
// SPEC: UDP_TRANSPORT_2009.md §4 (engine+0xafaa0)
pub const RECV_BUFFER_BYTES: usize = 8192;
/// IP + UDP header cost added to every datagram by the flow control (28 bytes).
// SPEC: UDP_TRANSPORT_2009.md §11.6 (engine+0xb1440)
pub const DATAGRAM_HEADER_COST_BITS: u32 = 224;
/// Room flow control must leave before the pump builds another datagram.
// SPEC: UDP_TRANSPORT_2009.md §11.1 (engine+0xb3b20)
pub const PUMP_ROOM_BITS: u32 = 256;
/// Cost the pump tests before writing an ack.
// SPEC: UDP_TRANSPORT_2009.md §11.1, §11.3 (blocked(58), engine+0xb3b20)
pub const ACK_TEST_COST_BITS: u32 = 58;
/// Extra bits a resend tests on top of the record's own bits.
// SPEC: UDP_TRANSPORT_2009.md §11.3 (engine+0xb3680)
pub const RESEND_TEST_EXTRA_BITS: u32 = 296;
/// A body under this many bits is not sent.
// SPEC: UDP_TRANSPORT_2009.md §11.1 step 3
pub const MIN_SEND_BODY_BITS: usize = 4;

/// Client join ask is repeated whenever more than 299 ms passed since the last send.
// SPEC: UDP_TRANSPORT_2009.md §5.2 (engine+0xb6850)
pub const JOIN_RETRY_MS: u32 = 300;
/// Client gives up joining when more than 9999 ms passed since the first ask.
// SPEC: UDP_TRANSPORT_2009.md §5.2 (engine+0xb6850)
pub const JOIN_TIMEOUT_MS: u32 = 10_000;
/// Host side: an existing link older than this (traffic stamp) is dropped on a re-join and not answered.
// SPEC: UDP_TRANSPORT_2009.md §5.3 step 1 (engine+0xb5120)
pub const STALE_LINK_MS: u32 = 9_999;
/// Lower bound of the heartbeat interval: `hb_due = now - last_ack_sent >= max(200, L_ms)`.
// SPEC: UDP_TRANSPORT_2009.md §11.1 (engine+0xad520)
pub const HEARTBEAT_MIN_MS: u32 = 200;
/// Initial mean round trip of a link.
// SPEC: UDP_TRANSPORT_2009.md §12.1 (engine+0xb48b0)
pub const INITIAL_RTT_MS: f32 = 200.0;
/// Number of round-trip samples averaged.
// SPEC: UDP_TRANSPORT_2009.md §12.1 (engine+0xabb40)
pub const RTT_SAMPLES: usize = 16;
/// Lower clamp of the resend timeout.
// SPEC: UDP_TRANSPORT_2009.md §11.3 (engine+0xb3680)
pub const RTO_MIN_MS: u32 = 17;
/// Upper clamp of the resend timeout.
// SPEC: UDP_TRANSPORT_2009.md §11.3
pub const RTO_MAX_MS: u32 = 1000;
/// Resend timeout floor in trickle mode.
// SPEC: UDP_TRANSPORT_2009.md §11.3
pub const TRICKLE_RTO_MIN_MS: u32 = 500;
/// A record resent more than this many times switches the link to trickle mode.
// SPEC: UDP_TRANSPORT_2009.md §11.3
pub const TRICKLE_RESEND_LIMIT: u32 = 10;
/// New reliable blocks pause while more than this many sent records are outstanding.
// SPEC: UDP_TRANSPORT_2009.md §11.3, §11.5
pub const PAUSE_SENT_RECORDS: usize = 8;
/// New reliable blocks pause while a record was resent more than this many times.
// SPEC: UDP_TRANSPORT_2009.md §11.3
pub const PAUSE_RESEND_LIMIT: u32 = 2;
/// Resend counters of the records left after a cumulative ack are capped at this value.
// SPEC: UDP_TRANSPORT_2009.md §10.2
pub const RESEND_CAP_AFTER_ACK: u32 = 3;
/// Flow window period: `W = rate_bps * 200 / 1000` bits.
// SPEC: UDP_TRANSPORT_2009.md §12.2 (engine+0xad5e0)
pub const FLOW_WINDOW_MS: u32 = 200;
/// Flow control only drains when at least this many ms passed since the last tick.
// SPEC: UDP_TRANSPORT_2009.md §12.3 (engine+0xad650)
pub const FLOW_TICK_MIN_MS: u32 = 17;
/// Link rate before it is set from the join ask.
// SPEC: UDP_TRANSPORT_2009.md §12.2 (engine+0xb48b0)
pub const INITIAL_LINK_RATE_BPS: u32 = 0x7FFF_FFFF;
/// A link with nothing received for more than this many ms is dropped (`now - last_recv >= 90001`).
// SPEC: UDP_TRANSPORT_2009.md §13 (engine+0xb5c30)
pub const IDLE_DROP_MS: u32 = 90_000;
/// Warning status threshold of the (undecompiled) link stall / latency method.
// SPEC: UDP_TRANSPORT_2009.md §13 (engine+0xb5c30)
pub const HIGH_LATENCY_WARN_MS: f32 = 30_000.0;
/// Bye datagrams are sent this many times.
// SPEC: UDP_TRANSPORT_2009.md §9.1 (engine+0xb3920, engine+0xb3a10)
pub const BYE_REPEAT: u32 = 4;
/// Gap between bye copies.
// SPEC: UDP_TRANSPORT_2009.md §9.1
pub const BYE_GAP_MS: u32 = 100;
/// Default host listen port and info-probe port.
// SPEC: UDP_TRANSPORT_2009.md §5.4, §5.6 (engine+0xb6690, engine+0xb3ef0)
pub const DEFAULT_GAME_PORT: u16 = 27888;
/// Untimed `probe_tag`.
// SPEC: UDP_TRANSPORT_2009.md §5.4
pub const PROBE_TAG_UNTIMED: u8 = 0xFF;
/// Engine protocol version carried by the host's first session message.
// SPEC: UDP_TRANSPORT_2009.md §14 (engine+0xa2a70, engine+0x8e9f0)
pub const ENGINE_PROTOCOL_VERSION: u32 = 7;

/// Join result: no join reply within 10 s.
// SPEC: GLOSSARY_UDP.md T.7 (engine+0xb6850)
pub const JOIN_TIMED_OUT: u32 = 0x4F;
/// Join result: could not bind a UDP port.
// SPEC: GLOSSARY_UDP.md T.7 (engine+0xb6850, engine+0xb6690)
pub const JOIN_NO_SOCKET: u32 = 0x50;
/// Join result: `accepted = 0, same_app = 1`.
// SPEC: GLOSSARY_UDP.md T.7 (engine+0xb6850)
pub const JOIN_REFUSED: u32 = 0x51;
/// Join result: `accepted = 0, same_app = 0` (application id mismatch).
// SPEC: GLOSSARY_UDP.md T.7 (engine+0xb6850)
pub const JOIN_WRONG_APP: u32 = 0x52;
/// Result: engine protocol version is not 7.
// SPEC: GLOSSARY_UDP.md T.7 (engine+0xa2a70)
pub const VERSION_MISMATCH: u32 = 0x4E;
/// Drop event code: idle timeout, stale re-join, or plain bye seen by a non-host.
// SPEC: GLOSSARY_UDP.md T.7 (engine+0xb5c30, engine+0xb5120, engine+0xb5d70)
pub const DROP_STALE: u32 = 3;
/// Drop event code: plain bye seen by the host.
// SPEC: GLOSSARY_UDP.md T.7 (engine+0xb5d70)
pub const DROP_PLAIN: u32 = 0;

/// Combat Arms application id (value per PROTOCOL §4.4, PROVEN) laid out as a standard Windows GUID
/// record (Data1..Data3 little-endian, Data4 in order).
///
/// TODO (UNPROVEN, spec §5.1): the value is PROVEN (PROTOCOL §4.4) but the in-memory byte layout the client
/// copies into the 128-bit field is not; confirm with validation V3 (bits 35..162 of the first join ask).
// SPEC: UDP_TRANSPORT_2009.md §5.1
pub const TODO_GAME_APP_ID_GUID_LAYOUT: [u8; 16] = [
    0xCB, 0x33, 0x58, 0x62, 0xC8, 0x3B, 0xA1, 0x44, 0x91, 0xD8, 0xEC, 0xB5, 0xCE, 0x89, 0x6B, 0x72,
];

/// Initial value of the client's join `rate_bps` (PE-DATA, VA 0x6cb298).
///
/// TODO (UNPROVEN, spec §5.3 / §19 item 3): whether a config or console value overrides it at run time;
/// V3 reads the real value from a capture.
// SPEC: UDP_TRANSPORT_2009.md §5.3
pub const TODO_CLIENT_JOIN_RATE_BPS: u32 = 35_840;

/// Initial value of the host rate carried by the first session message (PE-DATA, VA 0x6cb29c).
///
/// TODO (UNPROVEN, spec §14): may be overridden at run time.
// SPEC: UDP_TRANSPORT_2009.md §14
pub const TODO_HOST_RATE_BPS: u32 = 5_120_000;

/// Whether `L_ms` (the truncated latency used by the timers) is the effective latency `L_eff` (true) or the
/// plain mean round trip (false).
///
/// TODO (UNPROVEN, spec §12.1): decompiler lost the x87 argument; `L_eff` is the likelier input. V7 measures it.
// SPEC: UDP_TRANSPORT_2009.md §12.1
pub const TODO_LATENCY_USES_EFFECTIVE: bool = true;

/// `precise` flag the `pos48` path passes to the position codec.
///
/// TODO (UNPROVEN, spec §15.1): not visible in the C. With the extra byte 0 it only changes the cell offset
/// (`(0.5, 0.5, 0.5)` when precise, `(4, 2, 4)` otherwise). V17 fits it.
// SPEC: UDP_TRANSPORT_2009.md §15.1
pub const TODO_POS48_PRECISE: bool = false;

/// Scale of the 18-bit ratios `q1`, `q2` of `vec72` (`q = trunc(|o / major| * scale)`).
///
/// TODO (UNPROVEN, spec §15.2): the truncation inputs are not visible; the spec presumes `|o / major|` scaled to
/// 18 bits. `2^18 - 1` keeps a ratio of 1.0 inside 18 bits. V18 fits the scale.
// SPEC: UDP_TRANSPORT_2009.md §15.2
pub const TODO_VEC72_RATIO_SCALE: f32 = 262_143.0;

/// Radians per `yaw8` step on read: 2π/254.
// SPEC: UDP_TRANSPORT_2009.md §15.3 (engine+0x5b990)
pub const YAW8_RADIANS_PER_STEP: f32 = 0.024_736_95;

/// Scale applied to the yaw angle before truncation to s8 on write.
///
/// TODO (UNPROVEN, spec §15.3): not visible in the C; it must be 254/(2π) for a round trip. V18 confirms.
// SPEC: UDP_TRANSPORT_2009.md §15.3
pub const TODO_YAW8_WRITE_SCALE: f32 = 254.0 / (2.0 * std::f32::consts::PI);

// ------------------------------------------------------------------------------------------------- CRC / check8

const fn crc_table() -> [u32; 256] {
    // Public IEEE 802.3 CRC-32, reflected polynomial 0xEDB88320.
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32 (IEEE, reflected 0xEDB88320, init `!0`, final complement): the zlib form.
// SPEC: UDP_TRANSPORT_2009.md §6.1 (engine+0x80010, engine+0x80f70)
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// `check8` of a link datagram body: the XOR of the four bytes of `crc32(body)`.
// SPEC: UDP_TRANSPORT_2009.md §6.1 (fold engine+0xb1440 / engine+0xb4a70)
pub fn check8(body: &[u8]) -> u8 {
    let c = crc32(body);
    (c ^ (c >> 8) ^ (c >> 16) ^ (c >> 24)) as u8
}

// ------------------------------------------------------------------------------------------------- bit stream

#[inline]
fn mask32(n: u32) -> u32 {
    if n >= 32 {
        u32::MAX
    } else {
        (1u32 << n) - 1
    }
}

/// Owned bit string and writer. Bit k of the stream is bit `k % 8` (from the least significant bit) of byte
/// `k / 8`; every multi-bit value is stored least significant bit first; nothing is byte aligned.
/// Unused high bits of the last byte are always 0, so two equal bit strings compare equal.
// SPEC: UDP_TRANSPORT_2009.md §2.1, §2.2 (engine+0x80a30, engine+0xb0c90)
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct BitWriter {
    bytes: Vec<u8>,
    bits: usize,
}

impl fmt::Debug for BitWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BitWriter({} bits: ", self.bits)?;
        for b in &self.bytes {
            write!(f, "{b:02x}")?;
        }
        write!(f, ")")
    }
}

impl BitWriter {
    /// Empty bit string.
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty bit string with room for `bytes` bytes.
    pub fn with_capacity(bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(bytes),
            bits: 0,
        }
    }

    /// Bit string of the first `bits` bits of `data` (bits past the end of `data` are 0).
    pub fn from_bits(data: &[u8], bits: usize) -> Self {
        let mut w = Self::with_capacity(bits.div_ceil(8));
        let mut r = BitReader::with_bit_len(data, bits.min(data.len() * 8));
        w.append_reader_bits(&mut r, bits);
        w
    }

    /// Bit string of all bits of `data`.
    pub fn from_bytes(data: &[u8]) -> Self {
        Self {
            bytes: data.to_vec(),
            bits: data.len() * 8,
        }
    }

    /// Length in bits.
    pub fn bit_len(&self) -> usize {
        self.bits
    }

    /// Length in whole bytes (`ceil(bits / 8)`).
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    /// True when no bit was written.
    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// The datagram bytes (`ceil(bits / 8)`, high pad bits 0).
    // SPEC: UDP_TRANSPORT_2009.md §2.2
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Copy of [`BitWriter::as_bytes`].
    pub fn to_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    /// Consumes the writer and returns its bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// A reader over this bit string (exact bit length).
    pub fn reader(&self) -> BitReader<'_> {
        BitReader::with_bit_len(&self.bytes, self.bits)
    }

    /// Writes the low `n <= 32` bits of `value`, least significant bit first.
    // SPEC: UDP_TRANSPORT_2009.md §2.1 (engine+0x80a30)
    pub fn write_bits(&mut self, value: u32, n: usize) -> &mut Self {
        assert!(n <= 32, "write_bits: n = {n} > 32");
        let mut v = value & mask32(n as u32);
        let mut left = n;
        while left > 0 {
            let used = self.bits % 8;
            if used == 0 {
                self.bytes.push(0);
            }
            let room = 8 - used;
            let take = room.min(left);
            let last = self.bytes.len() - 1;
            self.bytes[last] |= ((v & mask32(take as u32)) as u8) << used;
            v = if take >= 32 { 0 } else { v >> take };
            left -= take;
            self.bits += take;
        }
        self
    }

    /// Writes `n <= 64` bits: the low 32 bits first, then the high `n - 32` bits.
    // SPEC: UDP_TRANSPORT_2009.md §2.5 (engine+0x80ac0)
    pub fn write_bits64(&mut self, value: u64, n: usize) -> &mut Self {
        assert!(n <= 64, "write_bits64: n = {n} > 64");
        if n <= 32 {
            return self.write_bits(value as u32, n);
        }
        self.write_bits(value as u32, 32);
        self.write_bits((value >> 32) as u32, n - 32)
    }

    /// Writes one bit.
    pub fn write_bit(&mut self, v: bool) -> &mut Self {
        self.write_bits(v as u32, 1)
    }

    /// Writes an 8-bit field.
    pub fn write_u8(&mut self, v: u8) -> &mut Self {
        self.write_bits(v as u32, 8)
    }

    /// Writes an 8-bit two's-complement field.
    // SPEC: UDP_TRANSPORT_2009.md §2.4
    pub fn write_s8(&mut self, v: i8) -> &mut Self {
        self.write_bits(v as u8 as u32, 8)
    }

    /// Writes a 16-bit field.
    pub fn write_u16(&mut self, v: u16) -> &mut Self {
        self.write_bits(v as u32, 16)
    }

    /// Writes a 16-bit two's-complement field.
    pub fn write_s16(&mut self, v: i16) -> &mut Self {
        self.write_bits(v as u16 as u32, 16)
    }

    /// Writes a 32-bit field.
    pub fn write_u32(&mut self, v: u32) -> &mut Self {
        self.write_bits(v, 32)
    }

    /// Writes a 64-bit field.
    pub fn write_u64(&mut self, v: u64) -> &mut Self {
        self.write_bits64(v, 64)
    }

    /// Writes an `f32` as its raw IEEE-754 bit pattern.
    // SPEC: UDP_TRANSPORT_2009.md §2.6 (engine+0x5b3d0 / engine+0x5b650)
    pub fn write_f32(&mut self, v: f32) -> &mut Self {
        self.write_bits(v.to_bits(), 32)
    }

    /// Writes a raw block: the memory bytes in order, each LSB first, at the current bit position.
    // SPEC: UDP_TRANSPORT_2009.md §2.7 (engine+0x80b00)
    pub fn write_raw(&mut self, data: &[u8]) -> &mut Self {
        if self.bits.is_multiple_of(8) {
            self.bytes.extend_from_slice(data);
            self.bits += data.len() * 8;
            return self;
        }
        for &b in data {
            self.write_bits(b as u32, 8);
        }
        self
    }

    /// Appends another bit string with its exact bit length (not rounded).
    // SPEC: UDP_TRANSPORT_2009.md §2.8 (engine+0x80bf0)
    pub fn append(&mut self, other: &BitWriter) -> &mut Self {
        let mut r = other.reader();
        self.append_reader_bits(&mut r, other.bits);
        self
    }

    fn append_reader_bits(&mut self, r: &mut BitReader<'_>, mut n: usize) {
        while n >= 32 {
            let v = r.read_bits(32);
            self.write_bits(v, 32);
            n -= 32;
        }
        if n > 0 {
            let v = r.read_bits(n);
            self.write_bits(v, n);
        }
    }

    /// `zstr8`: 8-bit code units, then one 8-bit 0. A unit 0 inside `units` ends the string early on read, so
    /// it is not written: the string is cut at the first 0.
    // SPEC: UDP_TRANSPORT_2009.md §3 (write engine+0x80e30)
    pub fn write_zstr8(&mut self, units: &[u8]) -> &mut Self {
        for &u in units.iter().take_while(|&&u| u != 0) {
            self.write_u8(u);
        }
        self.write_u8(0)
    }

    /// `zstr16`: 16-bit code units, then one 16-bit 0 (cut at the first 0 unit).
    ///
    /// The writer is not decompiled (spec §3: UNPROVEN, presumed symmetric to the PROVEN reader).
    // SPEC: UDP_TRANSPORT_2009.md §3 (read engine+0x80490)
    pub fn write_zstr16_presumed(&mut self, units: &[u16]) -> &mut Self {
        for &u in units.iter().take_while(|&&u| u != 0) {
            self.write_u16(u);
        }
        self.write_u16(0)
    }

    /// `lstr16`: u16 byte count, then the bytes. Longer input is cut to 0xFFFF bytes.
    ///
    /// The writer is not decompiled (spec §3: UNPROVEN, presumed symmetric to the reader at engine+0x5b900).
    // SPEC: UDP_TRANSPORT_2009.md §3
    pub fn write_lstr16_presumed(&mut self, bytes: &[u8]) -> &mut Self {
        let n = bytes.len().min(0xFFFF);
        self.write_u16(n as u16);
        self.write_raw(&bytes[..n])
    }

    /// `lstr16h` (string handle): u16 byte count, 0xFFFF = no string, 0 = empty, else the bytes.
    /// Present strings are cut to 0xFFFE bytes so the count never reads as "none".
    ///
    /// The writer is not decompiled (spec §3: UNPROVEN, presumed symmetric to engine+0x5b560).
    // SPEC: UDP_TRANSPORT_2009.md §3
    pub fn write_lstr16h_presumed(&mut self, s: Option<&[u8]>) -> &mut Self {
        match s {
            None => self.write_u16(0xFFFF),
            Some(b) => {
                let n = b.len().min(0xFFFE);
                self.write_u16(n as u16);
                self.write_raw(&b[..n])
            }
        }
    }

    /// `nest16`: u16 bit count, then the nested message bits (cut to 0xFFFF bits).
    ///
    /// The writer is not decompiled (spec §3: UNPROVEN, presumed symmetric to engine+0x5b9d0).
    // SPEC: UDP_TRANSPORT_2009.md §3
    pub fn write_nest16_presumed(&mut self, nested: &BitWriter) -> &mut Self {
        let n = nested.bits.min(0xFFFF);
        self.write_u16(n as u16);
        let mut r = nested.reader();
        self.append_reader_bits(&mut r, n);
        self
    }

    /// `objref16`: u16 object id, 0xFFFF = no object.
    // SPEC: UDP_TRANSPORT_2009.md §3 (write engine+0x5b530)
    pub fn write_objref16(&mut self, obj: Option<u16>) -> &mut Self {
        self.write_u16(obj.unwrap_or(OBJREF_NONE))
    }

    /// `vlen` length code of a message of `len` bits.
    // SPEC: UDP_TRANSPORT_2009.md §10.1 (engine+0xab9e0)
    pub fn write_vlen(&mut self, len: u32) -> &mut Self {
        self.write_bits(len & 0x7F, 7);
        if len < 128 {
            return self.write_bit(false);
        }
        self.write_bit(true);
        self.write_bits((len >> 7) & 7, 3);
        if len < 1024 {
            return self.write_bit(false);
        }
        self.write_bit(true);
        let mut v = len >> 10;
        while v != 0 {
            self.write_bits(v & 1, 1);
            self.write_bit((v >> 1) != 0);
            v >>= 1;
        }
        self
    }

    /// One block entry: `[vlen L][L bits]`.
    // SPEC: UDP_TRANSPORT_2009.md §8, §9
    pub fn write_entry(&mut self, msg: &BitWriter) -> &mut Self {
        self.write_vlen(msg.bits as u32);
        self.append(msg)
    }
}

/// `objref16` value meaning "no object".
// SPEC: UDP_TRANSPORT_2009.md §3
pub const OBJREF_NONE: u16 = 0xFFFF;

/// Number of header bits `write_vlen(len)` produces: 8 if `len < 128`, else `12 + 2 * bitlen(len >> 10)`.
// SPEC: UDP_TRANSPORT_2009.md §10.1 (budget formula engine+0xaf560, engine+0xb2e20)
pub fn vlen_bits(len: u32) -> u32 {
    if len < 128 {
        8
    } else {
        12 + 2 * (32 - (len >> 10).leading_zeros())
    }
}

/// Bit reader over a datagram or bit string. Reads are clamped to the bits that remain: missing bits read
/// as 0 and the position never passes the end.
// SPEC: UDP_TRANSPORT_2009.md §2.3 (engine+0x80220)
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    len: usize,
    pos: usize,
}

impl<'a> BitReader<'a> {
    /// Reader over all `8 * data.len()` bits (the receive side's bit length).
    // SPEC: UDP_TRANSPORT_2009.md §2.2 (engine+0xafaa0, recv_len * 8)
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            len: data.len() * 8,
            pos: 0,
        }
    }

    /// Reader over the first `bits` bits of `data` (clamped to `8 * data.len()`).
    pub fn with_bit_len(data: &'a [u8], bits: usize) -> Self {
        Self {
            data,
            len: bits.min(data.len() * 8),
            pos: 0,
        }
    }

    /// Current bit position.
    pub fn bit_pos(&self) -> usize {
        self.pos
    }

    /// Total bit length.
    pub fn bit_len(&self) -> usize {
        self.len
    }

    /// Bits left.
    pub fn remaining_bits(&self) -> usize {
        self.len - self.pos
    }

    /// True when every bit was read.
    pub fn at_end(&self) -> bool {
        self.pos >= self.len
    }

    /// Moves to an absolute bit position (clamped to the end).
    pub fn seek_bits(&mut self, pos: usize) -> &mut Self {
        self.pos = pos.min(self.len);
        self
    }

    /// Skips `n` bits (clamped to the end).
    pub fn skip_bits(&mut self, n: usize) -> &mut Self {
        self.pos = self.pos.saturating_add(n).min(self.len);
        self
    }

    /// Reads `n <= 32` bits without moving.
    pub fn peek_bits(&self, n: usize) -> u32 {
        self.clone().read_bits(n)
    }

    /// Reads the next `n <= 32` bits zero-extended; bits past the end read as 0.
    // SPEC: UDP_TRANSPORT_2009.md §2.3 (engine+0x80220)
    pub fn read_bits(&mut self, n: usize) -> u32 {
        assert!(n <= 32, "read_bits: n = {n} > 32");
        let avail = n.min(self.remaining_bits());
        let mut v = 0u32;
        let mut got = 0usize;
        while got < avail {
            let byte = self.data[self.pos / 8];
            let off = self.pos % 8;
            let take = (8 - off).min(avail - got);
            let part = ((byte >> off) as u32) & mask32(take as u32);
            v |= part << got;
            got += take;
            self.pos += take;
        }
        v
    }

    /// Reads `n <= 64` bits: the low 32 bits first, then the high `n - 32` bits.
    // SPEC: UDP_TRANSPORT_2009.md §2.5 (engine+0x802c0)
    pub fn read_bits64(&mut self, n: usize) -> u64 {
        assert!(n <= 64, "read_bits64: n = {n} > 64");
        if n <= 32 {
            return self.read_bits(n) as u64;
        }
        let lo = self.read_bits(32) as u64;
        let hi = self.read_bits(n - 32) as u64;
        lo | (hi << 32)
    }

    /// Reads one bit.
    pub fn read_bit(&mut self) -> bool {
        self.read_bits(1) != 0
    }

    /// Reads an 8-bit field.
    pub fn read_u8(&mut self) -> u8 {
        self.read_bits(8) as u8
    }

    /// Reads an 8-bit field and sign-extends it (C cast).
    // SPEC: UDP_TRANSPORT_2009.md §2.4 (engine+0x5b990)
    pub fn read_s8(&mut self) -> i8 {
        self.read_bits(8) as u8 as i8
    }

    /// Reads a 16-bit field.
    pub fn read_u16(&mut self) -> u16 {
        self.read_bits(16) as u16
    }

    /// Reads a 16-bit field and sign-extends it (C cast).
    // SPEC: UDP_TRANSPORT_2009.md §2.4 (engine+0xa4160)
    pub fn read_s16(&mut self) -> i16 {
        self.read_bits(16) as u16 as i16
    }

    /// Reads a 32-bit field.
    pub fn read_u32(&mut self) -> u32 {
        self.read_bits(32)
    }

    /// Reads a 64-bit field.
    pub fn read_u64(&mut self) -> u64 {
        self.read_bits64(64)
    }

    /// Reads an `f32` from its raw bit pattern.
    // SPEC: UDP_TRANSPORT_2009.md §2.6
    pub fn read_f32(&mut self) -> f32 {
        f32::from_bits(self.read_bits(32))
    }

    /// Reads a raw block of `n` bytes (missing bytes read as 0).
    // SPEC: UDP_TRANSPORT_2009.md §2.7 (engine+0x80300)
    pub fn read_raw(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.read_u8()).collect()
    }

    /// Reads the next `n` bits as an owned bit string (clamped to what remains).
    pub fn read_bit_string(&mut self, n: usize) -> BitWriter {
        let n = n.min(self.remaining_bits());
        let mut w = BitWriter::with_capacity(n.div_ceil(8));
        w.append_reader_bits(self, n);
        w
    }

    /// `zstr8`: reads 8-bit units up to and including the terminating 0 (or the end of data).
    // SPEC: UDP_TRANSPORT_2009.md §3 (read engine+0x80450)
    pub fn read_zstr8(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while !self.at_end() {
            let u = self.read_u8();
            if u == 0 {
                break;
            }
            out.push(u);
        }
        out
    }

    /// `zstr16`: reads 16-bit units up to and including the terminating 0 (or the end of data).
    // SPEC: UDP_TRANSPORT_2009.md §3 (read engine+0x80490)
    pub fn read_zstr16(&mut self) -> Vec<u16> {
        let mut out = Vec::new();
        while !self.at_end() {
            let u = self.read_u16();
            if u == 0 {
                break;
            }
            out.push(u);
        }
        out
    }

    /// `lstr16`: u16 byte count, then that many bytes (the full count is consumed).
    ///
    /// How the 2009 reader skips the tail of an over-long string is UNPROVEN (spec §3, §19 item 5); this
    /// reader always consumes the full count.
    // SPEC: UDP_TRANSPORT_2009.md §3 (engine+0x5b900, engine+0x5bf40)
    pub fn read_lstr16(&mut self) -> Vec<u8> {
        let n = self.read_u16() as usize;
        self.read_raw(n.min(self.remaining_bits() / 8))
    }

    /// `lstr16h`: `None` for count 0xFFFF, else the bytes.
    // SPEC: UDP_TRANSPORT_2009.md §3 (engine+0x5b560, engine+0x5bc10)
    pub fn read_lstr16h(&mut self) -> Option<Vec<u8>> {
        let n = self.read_u16();
        if n == 0xFFFF {
            return None;
        }
        Some(self.read_raw((n as usize).min(self.remaining_bits() / 8)))
    }

    /// `nest16`: u16 bit count, then that many bits as a nested bit string.
    // SPEC: UDP_TRANSPORT_2009.md §3 (engine+0x5b9d0, engine+0x5bb80)
    pub fn read_nest16(&mut self) -> BitWriter {
        let n = self.read_u16() as usize;
        self.read_bit_string(n)
    }

    /// `objref16`: `None` for 0xFFFF.
    // SPEC: UDP_TRANSPORT_2009.md §3 (read engine+0x5cde0)
    pub fn read_objref16(&mut self) -> Option<u16> {
        match self.read_u16() {
            OBJREF_NONE => None,
            v => Some(v),
        }
    }

    /// `vlen` length code.
    // SPEC: UDP_TRANSPORT_2009.md §10.1 (engine+0xaba90)
    pub fn read_vlen(&mut self) -> u32 {
        let mut len = self.read_bits(7);
        if !self.read_bit() {
            return len;
        }
        len |= self.read_bits(3) << 7;
        if !self.read_bit() {
            return len;
        }
        let mut shift = 10u32;
        loop {
            let b = self.read_bits(1);
            if shift < 32 {
                len |= b << shift;
            }
            shift += 1;
            if !self.read_bit() || self.at_end() {
                break;
            }
        }
        len
    }
}

// ------------------------------------------------------------------------------------------------- link datagrams

/// Errors of the datagram decoders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UdpError {
    /// Zero-length datagram.
    Empty,
    /// `check8` does not match the body.
    // SPEC: UDP_TRANSPORT_2009.md §6.1 (a mismatch drops the datagram)
    BadCheck { got: u8, want: u8 },
    /// A reliable entry claims more bits than remain (the block is abandoned).
    // SPEC: UDP_TRANSPORT_2009.md §8
    RelEntryOverrun { claimed: u32, remaining: usize },
    /// An unreliable entry claims more bits than remain (the block is abandoned).
    // SPEC: UDP_TRANSPORT_2009.md §9
    UnrelEntryOverrun { claimed: u32, remaining: usize },
    /// The first 32 bits are not [`OOB_MAGIC`].
    NotOob,
}

impl fmt::Display for UdpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UdpError::Empty => write!(f, "empty datagram"),
            UdpError::BadCheck { got, want } => {
                write!(f, "check8 mismatch: got {got:#04x}, want {want:#04x}")
            }
            UdpError::RelEntryOverrun { claimed, remaining } => {
                write!(
                    f,
                    "reliable entry of {claimed} bits with {remaining} bits left"
                )
            }
            UdpError::UnrelEntryOverrun { claimed, remaining } => {
                write!(
                    f,
                    "unreliable entry of {claimed} bits with {remaining} bits left"
                )
            }
            UdpError::NotOob => write!(f, "not an out-of-band datagram"),
        }
    }
}

impl std::error::Error for UdpError {}

/// Control block.
// SPEC: UDP_TRANSPORT_2009.md §7
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CtlBlock {
    /// `CTL_ACK`: cumulative `ack_seq` and the optional selective mask (bit d = block `ack_seq + 2 + d`).
    Ack { ack_seq: u16, sack_mask: Option<u8> },
    /// `CTL_BYE`.
    Bye,
    /// `CTL_BYE_CODE` with its code.
    ByeCode(u32),
    /// Any other op (2, 4..15): no body is read; the 2009 receiver then parses the rest from the wrong
    /// position, so a sender must never produce it.
    Unknown(u8),
}

impl CtlBlock {
    fn write(&self, w: &mut BitWriter) {
        match *self {
            CtlBlock::Ack { ack_seq, sack_mask } => {
                w.write_bits(CTL_ACK as u32, CTL_OP_BITS as usize);
                w.write_bits((ack_seq & SEQ_MASK) as u32, SEQ_BITS as usize);
                match sack_mask {
                    Some(m) => {
                        w.write_bit(true);
                        w.write_u8(m);
                    }
                    None => {
                        w.write_bit(false);
                    }
                }
            }
            CtlBlock::Bye => {
                w.write_bits(CTL_BYE as u32, CTL_OP_BITS as usize);
            }
            CtlBlock::ByeCode(code) => {
                w.write_bits(CTL_BYE_CODE as u32, CTL_OP_BITS as usize);
                w.write_u32(code);
            }
            CtlBlock::Unknown(op) => {
                w.write_bits(op as u32 & 0xF, CTL_OP_BITS as usize);
            }
        }
    }

    fn read(r: &mut BitReader<'_>) -> Self {
        match r.read_bits(CTL_OP_BITS as usize) as u8 {
            CTL_ACK => {
                let ack_seq = r.read_bits(SEQ_BITS as usize) as u16;
                let sack_mask = if r.read_bit() {
                    Some(r.read_u8())
                } else {
                    None
                };
                CtlBlock::Ack { ack_seq, sack_mask }
            }
            CTL_BYE => CtlBlock::Bye,
            CTL_BYE_CODE => CtlBlock::ByeCode(r.read_u32()),
            op => CtlBlock::Unknown(op),
        }
    }
}

/// Reliable block: `[rel_seq:u13][tail_cont:u1] entry { [more:u1] entry } [more = 0]`.
/// An empty `entries` list is written as one zero-length entry (the keep-alive form); decoding keeps every
/// entry, including zero-length ones (they deliver nothing).
// SPEC: UDP_TRANSPORT_2009.md §8 (writer engine+0xb2e20, resend engine+0xb3490, reader engine+0xb2690)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelBlock {
    pub rel_seq: u16,
    /// The last entry continues as the first entry of the next reliable block.
    pub tail_cont: bool,
    pub entries: Vec<BitWriter>,
}

impl RelBlock {
    fn write(&self, w: &mut BitWriter) {
        w.write_bits((self.rel_seq & SEQ_MASK) as u32, SEQ_BITS as usize);
        w.write_bit(self.tail_cont);
        if self.entries.is_empty() {
            w.write_vlen(0);
        } else {
            for (i, e) in self.entries.iter().enumerate() {
                if i > 0 {
                    w.write_bit(true);
                }
                w.write_entry(e);
            }
        }
        w.write_bit(false);
    }

    fn read(r: &mut BitReader<'_>) -> Result<Self, UdpError> {
        let rel_seq = r.read_bits(SEQ_BITS as usize) as u16;
        let tail_cont = r.read_bit();
        let mut entries = Vec::new();
        loop {
            let len = r.read_vlen();
            if len as usize > r.remaining_bits() {
                return Err(UdpError::RelEntryOverrun {
                    claimed: len,
                    remaining: r.remaining_bits(),
                });
            }
            entries.push(r.read_bit_string(len as usize));
            if !r.read_bit() {
                break;
            }
        }
        Ok(RelBlock {
            rel_seq,
            tail_cont,
            entries,
        })
    }
}

/// Body of a link datagram (everything after `check8`).
///
/// Encoding: `has_ctl` and `has_rel` are always written (0 when absent); `has_unrel` and the unreliable
/// entries only when `unrel` is non-empty (no terminator: the block is last and pad bits decode as
/// `vlen = 0`). Zero-length unreliable entries cannot be represented (`vlen = 0` ends the block).
// SPEC: UDP_TRANSPORT_2009.md §6, §6.2, §9 (engine+0xb4a70, engine+0xb3b20, engine+0xaf560)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinkBody {
    pub ctl: Option<CtlBlock>,
    pub rel: Option<RelBlock>,
    pub unrel: Vec<BitWriter>,
}

impl LinkBody {
    /// Encodes the body bits.
    pub fn encode_bits(&self) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_bit(self.ctl.is_some());
        if let Some(c) = &self.ctl {
            c.write(&mut w);
        }
        w.write_bit(self.rel.is_some());
        if let Some(rb) = &self.rel {
            rb.write(&mut w);
        }
        let unrel: Vec<&BitWriter> = self.unrel.iter().filter(|e| !e.is_empty()).collect();
        if !unrel.is_empty() {
            w.write_bit(true);
            for e in unrel {
                w.write_entry(e);
            }
        }
        w
    }

    /// Decodes body bits. Absent trailing presence bits read as 0.
    // SPEC: UDP_TRANSPORT_2009.md §6.2, §2.3
    pub fn decode(r: &mut BitReader<'_>) -> Result<Self, UdpError> {
        let mut body = LinkBody::default();
        if r.read_bit() {
            body.ctl = Some(CtlBlock::read(r));
        }
        if r.read_bit() {
            body.rel = Some(RelBlock::read(r)?);
        }
        if r.read_bit() {
            while !r.at_end() {
                let len = r.read_vlen();
                if len == 0 {
                    break;
                }
                if len as usize > r.remaining_bits() {
                    return Err(UdpError::UnrelEntryOverrun {
                        claimed: len,
                        remaining: r.remaining_bits(),
                    });
                }
                body.unrel.push(r.read_bit_string(len as usize));
            }
        }
        Ok(body)
    }
}

/// Builds a link datagram: `check8` followed by the body bytes.
// SPEC: UDP_TRANSPORT_2009.md §6.1, §11.6 (engine+0xb1440)
pub fn encode_link_datagram(body: &LinkBody) -> Vec<u8> {
    seal_link_body(&body.encode_bits())
}

/// Prefixes already-encoded body bits with their `check8`.
// SPEC: UDP_TRANSPORT_2009.md §6.1
pub fn seal_link_body(body: &BitWriter) -> Vec<u8> {
    let b = body.as_bytes();
    let mut out = Vec::with_capacity(b.len() + 1);
    out.push(check8(b));
    out.extend_from_slice(b);
    out
}

/// Checks `check8` and decodes a link datagram.
// SPEC: UDP_TRANSPORT_2009.md §6, §6.1 (on the wire `d[0] == fold(crc32(d[1..]))`)
pub fn decode_link_datagram(datagram: &[u8]) -> Result<LinkBody, UdpError> {
    let (&got, body) = datagram.split_first().ok_or(UdpError::Empty)?;
    let want = check8(body);
    if got != want {
        return Err(UdpError::BadCheck { got, want });
    }
    LinkBody::decode(&mut BitReader::new(body))
}

/// Bye datagram body: `[1][CTL_BYE:4]` or `[1][CTL_BYE_CODE:4][bye_code:u32]`, no other presence bits.
/// Send it [`BYE_REPEAT`] times, [`BYE_GAP_MS`] apart.
// SPEC: UDP_TRANSPORT_2009.md §9.1 (engine+0xb3920, engine+0xb3a10)
pub fn encode_bye_datagram(code: Option<u32>) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bit(true);
    match code {
        None => CtlBlock::Bye.write(&mut w),
        Some(c) => CtlBlock::ByeCode(c).write(&mut w),
    }
    seal_link_body(&w)
}

// ------------------------------------------------------------------------------------------------- OOB datagrams

/// Body of an `OOB_INFO_REPLY` when `ok = 1`.
// SPEC: UDP_TRANSPORT_2009.md §5, §5.4 (engine+0xb5120 case 0)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoReply {
    pub session_name: Vec<u8>,
    pub probe_tag: u8,
    /// Meaning UNPROVEN (spec §5.4, §19 item 4).
    pub host_flag: bool,
    pub link_count: u8,
    /// Meaning UNPROVEN (spec §5.4, §19 item 4).
    pub host_byte: u8,
}

/// Out-of-band datagram: `[OOB_MAGIC:u32][opcode:u3][payload]`, no check byte.
// SPEC: UDP_TRANSPORT_2009.md §5
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OobDatagram {
    InfoAsk {
        app_id: [u8; 16],
        probe_tag: u8,
    },
    /// `None` = `ok = 0` (application id mismatch).
    InfoReply(Option<InfoReply>),
    JoinAsk {
        app_id: [u8; 16],
        rate_bps: u32,
    },
    /// `accepted = 1`, or `accepted = 0` with `same_app` (1 = refused, 0 = id mismatch).
    JoinReply {
        accepted: bool,
        same_app: bool,
    },
    /// `echo_id` is 24 bits.
    EchoAsk {
        echo_id: u32,
    },
    EchoReply {
        echo_id: u32,
    },
    /// Opcodes 6 and 7: the 2009 server logs and ignores them; the rest of the datagram is kept as bits.
    Other {
        opcode: u8,
        rest: BitWriter,
    },
}

/// True when the first 32 bits are [`OOB_MAGIC`].
// SPEC: UDP_TRANSPORT_2009.md §4 step 1
pub fn is_oob_datagram(datagram: &[u8]) -> bool {
    datagram.len() >= 4 && BitReader::new(datagram).read_u32() == OOB_MAGIC
}

impl OobDatagram {
    /// Encodes the datagram bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.write_u32(OOB_MAGIC);
        let op = |w: &mut BitWriter, o: u8| {
            w.write_bits(o as u32, OOB_OPCODE_BITS as usize);
        };
        match self {
            OobDatagram::InfoAsk { app_id, probe_tag } => {
                op(&mut w, OOB_INFO_ASK);
                w.write_raw(app_id).write_u8(*probe_tag);
            }
            OobDatagram::InfoReply(r) => {
                op(&mut w, OOB_INFO_REPLY);
                w.write_bit(r.is_some());
                if let Some(r) = r {
                    w.write_zstr8(&r.session_name)
                        .write_u8(r.probe_tag)
                        .write_bit(r.host_flag)
                        .write_u8(r.link_count)
                        .write_u8(r.host_byte);
                }
            }
            OobDatagram::JoinAsk { app_id, rate_bps } => {
                op(&mut w, OOB_JOIN_ASK);
                w.write_raw(app_id).write_u32(*rate_bps);
            }
            OobDatagram::JoinReply { accepted, same_app } => {
                op(&mut w, OOB_JOIN_REPLY);
                w.write_bit(*accepted);
                if !*accepted {
                    w.write_bit(*same_app);
                }
            }
            OobDatagram::EchoAsk { echo_id } => {
                op(&mut w, OOB_ECHO_ASK);
                w.write_bits(*echo_id & 0xFF_FFFF, 24);
            }
            OobDatagram::EchoReply { echo_id } => {
                op(&mut w, OOB_ECHO_REPLY);
                w.write_bits(*echo_id & 0xFF_FFFF, 24);
            }
            OobDatagram::Other { opcode, rest } => {
                op(&mut w, *opcode & 7);
                w.append(rest);
            }
        }
        w.into_bytes()
    }

    /// Decodes an out-of-band datagram. Missing trailing fields read as 0 (spec §2.3).
    pub fn decode(datagram: &[u8]) -> Result<Self, UdpError> {
        if !is_oob_datagram(datagram) {
            return Err(UdpError::NotOob);
        }
        let mut r = BitReader::new(datagram);
        r.skip_bits(32);
        let opcode = r.read_bits(OOB_OPCODE_BITS as usize) as u8;
        let app_id = |r: &mut BitReader<'_>| {
            let mut a = [0u8; 16];
            a.copy_from_slice(&r.read_raw(16));
            a
        };
        Ok(match opcode {
            OOB_INFO_ASK => {
                let app_id = app_id(&mut r);
                OobDatagram::InfoAsk {
                    app_id,
                    probe_tag: r.read_u8(),
                }
            }
            OOB_INFO_REPLY => {
                if r.read_bit() {
                    OobDatagram::InfoReply(Some(InfoReply {
                        session_name: r.read_zstr8(),
                        probe_tag: r.read_u8(),
                        host_flag: r.read_bit(),
                        link_count: r.read_u8(),
                        host_byte: r.read_u8(),
                    }))
                } else {
                    OobDatagram::InfoReply(None)
                }
            }
            OOB_JOIN_ASK => {
                let app_id = app_id(&mut r);
                OobDatagram::JoinAsk {
                    app_id,
                    rate_bps: r.read_u32(),
                }
            }
            OOB_JOIN_REPLY => {
                let accepted = r.read_bit();
                let same_app = if accepted { false } else { r.read_bit() };
                OobDatagram::JoinReply { accepted, same_app }
            }
            OOB_ECHO_ASK => OobDatagram::EchoAsk {
                echo_id: r.read_bits(24),
            },
            OOB_ECHO_REPLY => OobDatagram::EchoReply {
                echo_id: r.read_bits(24),
            },
            _ => {
                let n = r.remaining_bits();
                OobDatagram::Other {
                    opcode,
                    rest: r.read_bit_string(n),
                }
            }
        })
    }
}

// ------------------------------------------------------------------------------------------------- formulas

/// `round8(x) = (x + 7) & !7`.
pub fn round8(bits: u32) -> u32 {
    (bits + 7) & !7
}

/// Flow-control cost of sending a body of `body_bits`: `round8(body_bits + 8) + 224`.
// SPEC: UDP_TRANSPORT_2009.md §11.6 (engine+0xb1440)
pub fn datagram_cost_bits(body_bits: u32) -> u32 {
    round8(body_bits + 8) + DATAGRAM_HEADER_COST_BITS
}

/// Flow window `W = rate_bps * 200 / 1000` bits.
// SPEC: UDP_TRANSPORT_2009.md §12.2 (engine+0xad5e0)
pub fn flow_window_bits(rate_bps: u32) -> u32 {
    ((rate_bps as u64 * FLOW_WINDOW_MS as u64) / 1000) as u32
}

/// `blocked(cost) = 2W < used + debt + cost`.
// SPEC: UDP_TRANSPORT_2009.md §12.3 (engine+0xad7c0)
pub fn flow_blocked(window_bits: u32, used: u32, debt: u32, cost: u32) -> bool {
    2 * (window_bits as u64) < used as u64 + debt as u64 + cost as u64
}

/// Resend timeout: `clamp(L_ms, 17, 1000)`, at least 500 in trickle mode.
// SPEC: UDP_TRANSPORT_2009.md §11.3 (engine+0xb3680)
pub fn rto_ms(l_ms: u32, trickle: bool) -> u32 {
    let r = l_ms.clamp(RTO_MIN_MS, RTO_MAX_MS);
    if trickle {
        r.max(TRICKLE_RTO_MIN_MS)
    } else {
        r
    }
}

/// Heartbeat due: `now - last_ack_sent >= max(200, L_ms)` (unsigned 32-bit time arithmetic).
// SPEC: UDP_TRANSPORT_2009.md §11.1 (engine+0xad520)
pub fn heartbeat_due(now: u32, last_ack_sent: u32, l_ms: u32) -> bool {
    now.wrapping_sub(last_ack_sent) >= l_ms.max(HEARTBEAT_MIN_MS)
}

/// Unreliable message lifetime: `2 * max(L_ms, 200)` ms.
// SPEC: UDP_TRANSPORT_2009.md §11.4 (engine+0xaf560)
pub fn unrel_ttl_ms(l_ms: u32) -> u32 {
    2 * l_ms.max(HEARTBEAT_MIN_MS)
}

/// Next expected reliable sequence: `(last_in_order + 1) & 0x1FFF`.
// SPEC: UDP_TRANSPORT_2009.md §8.1
pub fn next_seq(seq: u16) -> u16 {
    seq.wrapping_add(1) & SEQ_MASK
}

/// How a receiver treats a reliable block.
// SPEC: UDP_TRANSPORT_2009.md §8.1 (engine+0xb2690, engine+0xaf450)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelArrival {
    /// `rel_seq == expected`: deliver.
    InOrder,
    /// 1..8 ahead: store in an out-of-order slot.
    Ahead(u16),
    /// Old, duplicate or more than 8 ahead: parse and discard.
    Discard,
}

/// Classifies `rel_seq` against `expected = next_seq(last_in_order)`.
// SPEC: UDP_TRANSPORT_2009.md §8.1 (`(rel_seq - expected) & 0x1FFF < 9`)
pub fn classify_rel_seq(rel_seq: u16, last_in_order: u16) -> RelArrival {
    let expected = next_seq(last_in_order);
    let d = rel_seq.wrapping_sub(expected) & SEQ_MASK;
    if d == 0 {
        RelArrival::InOrder
    } else if d < 9 {
        RelArrival::Ahead(d)
    } else {
        RelArrival::Discard
    }
}

/// `sack_mask` bit of a stored out-of-order block: `d = (stored - last_in_order - 2) & 0x1FFF`, if `d < 8`.
// SPEC: UDP_TRANSPORT_2009.md §10.2 (engine+0xabc10)
pub fn sack_bit(stored_seq: u16, last_in_order: u16) -> Option<u8> {
    let d = stored_seq.wrapping_sub(last_in_order).wrapping_sub(2) & SEQ_MASK;
    (d < 8).then_some(d as u8)
}

/// Sequence a `sack_mask` bit `i` acknowledges: `ack_seq + 2 + i`, masked to 13 bits (the spec advises
/// masking; the 2009 receiver does not, which only matters near the wrap).
// SPEC: UDP_TRANSPORT_2009.md §10.2 (quirk)
pub fn sack_seq(ack_seq: u16, bit: u8) -> u16 {
    ack_seq.wrapping_add(2).wrapping_add(bit as u16) & SEQ_MASK
}

// ------------------------------------------------------------------------------------------------- encodings

/// `pos48`: compact position `[px:u16][py:u16][pz:u16]`, 19/18/19-bit fixed point over a box.
// SPEC: UDP_TRANSPORT_2009.md §15.1 (write engine+0x5c040, read engine+0x5c320)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pos48 {
    pub px: u16,
    pub py: u16,
    pub pz: u16,
}

impl Pos48 {
    /// Writes the three u16 fields.
    pub fn write(&self, w: &mut BitWriter) {
        w.write_u16(self.px).write_u16(self.py).write_u16(self.pz);
    }

    /// Reads the three u16 fields.
    pub fn read(r: &mut BitReader<'_>) -> Self {
        Pos48 {
            px: r.read_u16(),
            py: r.read_u16(),
            pz: r.read_u16(),
        }
    }

    /// Decodes into world coordinates over the box `min..max` with the extra-precision byte `extra`
    /// (the `pos48` reader passes 0).
    // SPEC: UDP_TRANSPORT_2009.md §15.1 decode (EVIDENCE engine+0x21910)
    pub fn decode(&self, min: [f32; 3], max: [f32; 3], precise: bool, extra: u8) -> [f32; 3] {
        let (mut ix, mut iy, mut iz) = (8 * self.px as u32, 4 * self.py as u32, 8 * self.pz as u32);
        let (ox, oy, oz) = if precise {
            ix += (extra & 7) as u32;
            iy += ((extra >> 3) & 3) as u32;
            iz += ((extra >> 5) & 7) as u32;
            (0.5f32, 0.5f32, 0.5f32)
        } else {
            (4.0, 2.0, 4.0)
        };
        let s19 = 1.0f32 / (1u32 << 19) as f32;
        let s18 = 1.0f32 / (1u32 << 18) as f32;
        [
            min[0] + (max[0] - min[0]) * (ix as f32 + ox) * s19,
            min[1] + (max[1] - min[1]) * (iy as f32 + oy) * s18,
            min[2] + (max[2] - min[2]) * (iz as f32 + oz) * s19,
        ]
    }

    /// Encodes world coordinates; returns the fields and the extra-precision byte.
    ///
    /// UNPROVEN (spec §15.1): the truncation inputs are not visible; this uses the inverse the decoder implies,
    /// `u = trunc((v - min) / (max - min) * 2^19)` (2^18 for y), clamped to the field range. V17 validates it.
    // SPEC: UDP_TRANSPORT_2009.md §15.1 encode (EVIDENCE engine+0x21820)
    pub fn encode_presumed(pos: [f32; 3], min: [f32; 3], max: [f32; 3]) -> (Pos48, u8) {
        let q = |v: f32, lo: f32, hi: f32, bits: u32| -> u32 {
            let span = hi - lo;
            let top = (1u32 << bits) - 1;
            if span.is_nan() || span <= 0.0 {
                return 0;
            }
            let t = ((v - lo) / span * (1u32 << bits) as f32).trunc();
            if t.is_nan() || t <= 0.0 {
                0
            } else if t >= top as f32 {
                top
            } else {
                t as u32
            }
        };
        let ux = q(pos[0], min[0], max[0], 19);
        let uy = q(pos[1], min[1], max[1], 18);
        let uz = q(pos[2], min[2], max[2], 19);
        let p = Pos48 {
            px: (ux >> 3) as u16,
            py: (uy >> 2) as u16,
            pz: (uz >> 3) as u16,
        };
        let extra = ((ux & 7) | (uy & 3) << 3 | (uz & 7) << 5) as u8;
        (p, extra)
    }
}

/// `vec72`: compact vector `[major:f32][r1:u16][r2:u16][vflags:u8]` (velocities).
// SPEC: UDP_TRANSPORT_2009.md §15.2 (write engine+0x5b360, read engine+0x5b5d0, encode engine+0x21540)
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec72 {
    pub major: f32,
    pub r1: u16,
    pub r2: u16,
    /// Bits 0-1 high bits of r2, 2 sign r2, 3-4 high bits of r1, 5 sign r1, 6-7 major axis.
    pub vflags: u8,
}

impl Vec72 {
    /// Writes the four fields.
    pub fn write(&self, w: &mut BitWriter) {
        w.write_f32(self.major)
            .write_u16(self.r1)
            .write_u16(self.r2)
            .write_u8(self.vflags);
    }

    /// Reads the four fields.
    pub fn read(r: &mut BitReader<'_>) -> Self {
        Vec72 {
            major: r.read_f32(),
            r1: r.read_u16(),
            r2: r.read_u16(),
            vflags: r.read_u8(),
        }
    }

    /// Major axis (0 = x, 1 = y, 2 = z; 3 is never produced).
    pub fn major_axis(&self) -> u8 {
        self.vflags >> 6
    }

    /// 18-bit ratio `q1` (`r1` plus bits 3-4 of `vflags`).
    pub fn q1(&self) -> u32 {
        self.r1 as u32 | (((self.vflags >> 3) & 3) as u32) << 16
    }

    /// 18-bit ratio `q2` (`r2` plus bits 0-1 of `vflags`).
    pub fn q2(&self) -> u32 {
        self.r2 as u32 | ((self.vflags & 3) as u32) << 16
    }

    /// Encodes a vector. Axis choice, sign bits and field packing are PROVEN (spec §15.2); the ratio scale
    /// is [`TODO_VEC72_RATIO_SCALE`] (UNPROVEN).
    // SPEC: UDP_TRANSPORT_2009.md §15.2 encoder (EVIDENCE engine+0x21540)
    pub fn encode(v: [f32; 3]) -> Self {
        let [x, y, z] = v;
        // ties prefer x over y, then the larger of x / y over z
        let xy = if x.abs() >= y.abs() { 0u8 } else { 1u8 };
        let axis = if v[xy as usize].abs() >= z.abs() {
            xy
        } else {
            2
        };
        let (major, o1, o2) = match axis {
            0 => (x, y, z),
            1 => (y, x, z),
            _ => (z, x, y),
        };
        let ratio = |o: f32| -> u32 {
            if major == 0.0 || !major.is_finite() {
                return 0;
            }
            let t = ((o / major).abs() * TODO_VEC72_RATIO_SCALE).trunc();
            if t.is_nan() || t <= 0.0 {
                0
            } else {
                (t as u32).min((1 << 18) - 1)
            }
        };
        let (q1, q2) = (ratio(o1), ratio(o2));
        let pos = |f: f32| f >= 0.0;
        let s1 = pos(o1) != pos(major);
        let s2 = pos(o2) != pos(major);
        let vflags = ((q2 >> 16) & 3) as u8
            | (s2 as u8) << 2
            | (((q1 >> 16) & 3) as u8) << 3
            | (s1 as u8) << 5
            | axis << 6;
        Vec72 {
            major,
            r1: q1 as u16,
            r2: q2 as u16,
            vflags,
        }
    }

    /// Decodes a vector. The 2009 decoder is not decompiled (spec §15.2: UNPROVEN, presumed inverse of the
    /// encoder with [`TODO_VEC72_RATIO_SCALE`]).
    // SPEC: UDP_TRANSPORT_2009.md §15.2 decoder (VA 0x4816a0, not decompiled)
    pub fn decode_presumed(&self) -> [f32; 3] {
        let sign_of_major = if self.major >= 0.0 { 1.0f32 } else { -1.0 };
        let comp = |q: u32, flip: bool| -> f32 {
            let s = if flip { -sign_of_major } else { sign_of_major };
            s * self.major.abs() * (q as f32 / TODO_VEC72_RATIO_SCALE)
        };
        let o1 = comp(self.q1(), self.vflags & 0x20 != 0);
        let o2 = comp(self.q2(), self.vflags & 0x04 != 0);
        match self.major_axis() {
            0 => [self.major, o1, o2],
            1 => [o1, self.major, o2],
            _ => [o1, o2, self.major],
        }
    }
}

/// `yaw8`: reads an s8 and returns the yaw in radians (`s8 * 2π/254`).
// SPEC: UDP_TRANSPORT_2009.md §15.3 (read engine+0x5b990)
pub fn yaw8_to_radians(v: i8) -> f32 {
    v as f32 * YAW8_RADIANS_PER_STEP
}

/// Yaw of a quaternion `(x, y, z, w)`: `atan2(2 (x z + w y), 1 - 2 (x² + y²))`.
// SPEC: UDP_TRANSPORT_2009.md §15.3 write (engine+0x5b8a0)
pub fn quat_yaw(q: [f32; 4]) -> f32 {
    let [x, y, z, w] = q;
    (2.0 * (x * z + w * y)).atan2(1.0 - 2.0 * (x * x + y * y))
}

/// `yaw8` write: yaw of the quaternion scaled by [`TODO_YAW8_WRITE_SCALE`] (UNPROVEN) and truncated to s8.
// SPEC: UDP_TRANSPORT_2009.md §15.3 write (engine+0x5b8a0)
pub fn yaw8_from_quat_presumed(q: [f32; 4]) -> i8 {
    yaw8_from_radians_presumed(quat_yaw(q))
}

/// `yaw8` from an angle in radians, scaled by [`TODO_YAW8_WRITE_SCALE`] (UNPROVEN) and truncated toward 0.
// SPEC: UDP_TRANSPORT_2009.md §15.3
pub fn yaw8_from_radians_presumed(yaw: f32) -> i8 {
    (yaw * TODO_YAW8_WRITE_SCALE).trunc().clamp(-128.0, 127.0) as i8
}

/// `quat128`: `[x:f32][y:f32][z:f32][w:f32]`.
// SPEC: UDP_TRANSPORT_2009.md §15.4 (engine+0x5b3d0 / engine+0x5b650, order engine+0x5f6d0)
pub fn write_quat128(w: &mut BitWriter, q: [f32; 4]) {
    for c in q {
        w.write_f32(c);
    }
}

/// Reads a `quat128`.
// SPEC: UDP_TRANSPORT_2009.md §15.4
pub fn read_quat128(r: &mut BitReader<'_>) -> [f32; 4] {
    [r.read_f32(), r.read_f32(), r.read_f32(), r.read_f32()]
}

/// Rotation matrix (row-major 3x3) of a quaternion `(x, y, z, w)`, normalised by `2 / |q|²`.
// EVIDENCE: engine+0x5f6d0 (quaternion-to-matrix helper; standard formula)
pub fn quat_to_matrix(q: [f32; 4]) -> [[f32; 3]; 3] {
    let [x, y, z, w] = q;
    let n = x * x + y * y + z * z + w * w;
    let s = if n > 0.0 { 2.0 / n } else { 0.0 };
    let (xs, ys, zs) = (x * s, y * s, z * s);
    [
        [1.0 - (y * ys + z * zs), x * ys - w * zs, x * zs + w * ys],
        [x * ys + w * zs, 1.0 - (x * xs + z * zs), y * zs - w * xs],
        [x * zs - w * ys, y * zs + w * xs, 1.0 - (x * xs + y * ys)],
    ]
}

/// `rot24` / `rot48`: compact rotation, `[c0:s8][c1:s8][c2:s8]` and, only when `c0 >= 0`, `[c3][c4][c5]`.
/// `c0 < 0` marks the 3-byte short form.
// SPEC: UDP_TRANSPORT_2009.md §15.5 (encode engine+0x21b90, read engine+0x5bdd0)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rot48 {
    pub c: [i8; 6],
}

impl Rot48 {
    /// True for the 3-byte form (`rot24`).
    pub fn is_short(&self) -> bool {
        self.c[0] < 0
    }

    /// Writes 3 or 6 bytes.
    pub fn write(&self, w: &mut BitWriter) {
        let n = if self.is_short() { 3 } else { 6 };
        for &b in &self.c[..n] {
            w.write_s8(b);
        }
    }

    /// Reads 3 bytes, then 3 more when the first is not negative (short-form bytes 3..5 read as 0).
    // SPEC: UDP_TRANSPORT_2009.md §15.5 (engine+0x5bdd0)
    pub fn read(r: &mut BitReader<'_>) -> Self {
        let mut c = [0i8; 6];
        for b in c.iter_mut().take(3) {
            *b = r.read_s8();
        }
        if c[0] >= 0 {
            for b in c.iter_mut().skip(3) {
                *b = r.read_s8();
            }
        }
        Rot48 { c }
    }

    /// Encodes a quaternion. With `F` = rotated +Z axis and `U` = rotated +Y axis (columns 2 and 1 of the
    /// matrix): `c0 = trunc((F.y + 1) * 63.9)`, `c1 = trunc((F.x + 1) * 63.9)`, `c2 = trunc(F.z * 127.9)`,
    /// `c3..c5 = trunc(U * 127.9)`; the short form is chosen when the up vector rebuilt from the quantised
    /// forward vector matches `U` within 0.999 (or its negation, then `c1` is negated).
    // SPEC: UDP_TRANSPORT_2009.md §15.5 (EVIDENCE engine+0x21b90)
    pub fn encode(q: [f32; 4]) -> Self {
        let m = quat_to_matrix(q);
        let f = [m[0][2], m[1][2], m[2][2]];
        let u = [m[0][1], m[1][1], m[2][1]];
        let tr = |v: f32| v as i32 as i8;
        let mut c = [
            tr((f[1] + 1.0) * 63.9),
            tr((f[0] + 1.0) * 63.9),
            tr(f[2] * 127.9),
            tr(u[0] * 127.9),
            tr(u[1] * 127.9),
            tr(u[2] * 127.9),
        ];
        let fq = forward_from_codes(c[1], c[0], c[2]);
        if let Some(up) = up_from_forward(fq) {
            let d = up[0] * u[0] + up[1] * u[1] + up[2] * u[2];
            let short = if d > 0.999 {
                true
            } else if d < -0.999 {
                c[1] = negate_marker(c[1]);
                true
            } else {
                false
            };
            if short {
                c[0] = negate_marker(c[0]);
                // only c0..c2 go on the wire in the short form
                c[3] = 0;
                c[4] = 0;
                c[5] = 0;
            }
        }
        Rot48 { c }
    }

    /// Decodes into (forward, up) unit-ish vectors. The 2009 decoder is not decompiled (spec §15.5:
    /// UNPROVEN, presumed to rebuild `U` from `F'` in the short form and to flip it when `c1 < 0`).
    /// The short-form markers cannot tell 0 from 1 (both become -1); this reads them back as 1.
    // SPEC: UDP_TRANSPORT_2009.md §15.5 decoder (VA 0x481dc0, not decompiled)
    pub fn decode_presumed(&self) -> ([f32; 3], [f32; 3]) {
        let c = self.c;
        if !self.is_short() {
            let f = forward_from_codes(c[1], c[0], c[2]);
            let s = 1.0 / 127.0f32;
            return (f, [c[3] as f32 * s, c[4] as f32 * s, c[5] as f32 * s]);
        }
        let c0 = -(c[0] as i32);
        let flip = c[1] < 0;
        let c1 = if flip { -(c[1] as i32) } else { c[1] as i32 };
        let f = forward_from_codes(c1 as i8, c0 as i8, c[2]);
        let mut up = up_from_forward(f).unwrap_or([0.0, 1.0, 0.0]);
        if flip {
            up = [-up[0], -up[1], -up[2]];
        }
        (f, up)
    }
}

fn negate_marker(v: i8) -> i8 {
    if v == 0 {
        -1
    } else {
        v.wrapping_neg()
    }
}

/// `F' = ((c1/127 - 0.5) * 2, (c0/127 - 0.5) * 2, c2/127)`.
// SPEC: UDP_TRANSPORT_2009.md §15.5
fn forward_from_codes(c1: i8, c0: i8, c2: i8) -> [f32; 3] {
    let k = 1.0 / 127.0f32;
    [
        (c1 as f32 * k - 0.5) * 2.0,
        (c0 as f32 * k - 0.5) * 2.0,
        c2 as f32 * k,
    ]
}

/// `u = 50·(0,1,0)` made orthogonal to `f`, normalised; `None` when `|u|² <= 0.1`.
// SPEC: UDP_TRANSPORT_2009.md §15.5 (EVIDENCE engine+0x21b90)
fn up_from_forward(f: [f32; 3]) -> Option<[f32; 3]> {
    // same operation order as the encoder C: t = -(fy (fy + 50) + fz² + fx²) / (fy² + fz² + fx²),
    // u = f t + f + (0, 50, 0)
    let [fx, fy, fz] = f;
    let side = fz * fz + fx * fx;
    let den = fy * fy + side;
    if den.is_nan() || den <= 0.0 {
        return None;
    }
    let t = 0.0 - (fy * (fy + 50.0) + side) / den;
    let ux = fx * t + fx;
    let uz = fz * t + fz;
    let uy = t * fy + fy + 50.0;
    let uu = uz * uz + uy * uy + ux * ux;
    if 0.1 < uu {
        let k = 1.0 / uu.sqrt();
        Some([k * ux, k * uy, k * uz])
    } else {
        None
    }
}
