//! TCP framing of the 2009 lobby / auth protocol: the outer frame header (`FrameHeader8`), the inner message
//! header (`InnerHeader6`) and a packed little-endian field reader / writer.
//!
//! SPEC: PROTOCOL_2009.md §1.2 (frame := i32 frame_id | i32 body_len | body; body := u16 msg_id | u32 msg_len |
//! fields; fields packed with no padding; strings are fixed `wstr[n]` UTF-16LE arrays).
//! HARNESS: rust_refdump.py (wire_vectors.jsonl, 1000 reference frames replayed byte for byte in
//! `tests/ref_vectors.rs`).

use std::io::{self, Read};

/// Largest body a frame may carry: the client's writer refuses `body_len > 0x8000`.
// SPEC: PROTOCOL_2009.md §1.2 (max body 0x8000, PROVEN cs+0x18fb40)
pub const MAX_FRAME_PAYLOAD_SIZE: usize = 0x8000;

/// Size in bytes of the outer frame header (`i32 frame_id`, `i32 body_len`).
// SPEC: PROTOCOL_2009.md §1.2
pub const OUTER_HEADER_SIZE: usize = 8;

/// Size in bytes of the inner message header (`u16 msg_id`, `u32 msg_len`); the first field is at +6.
// SPEC: PROTOCOL_2009.md §1.2
pub const INNER_HEADER_SIZE: usize = 6;

/// Outer 8-byte frame header: `i32 frame_id`, `i32 body_len`. The client dispatches on `frame_id`.
// SPEC: PROTOCOL_2009.md §1.2 (FrameHeader8)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader8 {
    pub id: i32,
    pub size: i32,
}

impl FrameHeader8 {
    #[inline]
    pub fn from_bytes(b: [u8; 8]) -> Self {
        let id = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let size = i32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        Self { id, size }
    }

    #[inline]
    pub fn to_bytes(self) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[0..4].copy_from_slice(&self.id.to_le_bytes());
        out[4..8].copy_from_slice(&self.size.to_le_bytes());
        out
    }
}

/// Inner 6-byte message header: `u16 msg_id`, `u32 msg_len` (`msg_len == body_len`).
// SPEC: PROTOCOL_2009.md §1.2 (InnerHeader6)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InnerHeader6 {
    pub id: u16,
    pub size: u32,
}

impl InnerHeader6 {
    #[inline]
    pub fn from_slice(payload: &[u8]) -> Option<Self> {
        if payload.len() < INNER_HEADER_SIZE {
            return None;
        }
        let id = u16::from_le_bytes([payload[0], payload[1]]);
        let size = u32::from_le_bytes([payload[2], payload[3], payload[4], payload[5]]);
        Some(Self { id, size })
    }

    #[inline]
    pub fn to_bytes(self) -> [u8; 6] {
        let mut out = [0u8; 6];
        out[0..2].copy_from_slice(&self.id.to_le_bytes());
        out[2..6].copy_from_slice(&self.size.to_le_bytes());
        out
    }
}

/// Earlier name of [`FrameHeader8`], kept so existing callers still compile.
pub type PacketHeader8 = FrameHeader8;

/// Earlier name of [`InnerHeader6`], kept so existing callers still compile.
pub type OldProtocolHeader = InnerHeader6;

/// Resolves the id a server should dispatch a client frame on. In four cases the inner `msg_id` differs
/// from the frame id (SPEC: PROTOCOL_2009.md §1.2, inner-id overrides):
/// - `frame_id == 0x324` with inner `msg_id` 0x368 or 0x36a -> the inner id (function item equip / unequip).
/// - `frame_id == 0x356` with inner `msg_id` 0x35a or 0x35c -> the inner id (list-3 / list-4 version request).
/// - All other frames -> `frame_id as u16`.
#[inline]
pub fn effective_packet_id(frame_id: i32, payload: &[u8]) -> u16 {
    let pay_id = if payload.len() >= 2 {
        u16::from_le_bytes([payload[0], payload[1]])
    } else {
        frame_id as u16
    };
    let overridden = (frame_id == 0x324 && (pay_id == 0x368 || pay_id == 0x36a))
        || (frame_id == 0x356 && (pay_id == 0x35a || pay_id == 0x35c));
    if overridden {
        pay_id
    } else {
        frame_id as u16
    }
}

/// Reads one frame from `reader`.
// SPEC: PROTOCOL_2009.md §1.2 (frame check: complete when len >= 8 and len >= body_len + 8)
///
/// - Returns `Ok(None)` on clean EOF at the very first byte of a frame header.
/// - Returns `Err(UnexpectedEof)` if EOF occurs mid-header or mid-payload.
/// - Returns `Err(InvalidData)` if `sz < 0 || sz > 0x8000`.
pub fn read_tcp_frame<R: Read>(reader: &mut R) -> io::Result<Option<(i32, Vec<u8>)>> {
    let mut hdr = [0u8; OUTER_HEADER_SIZE];
    match reader.read(&mut hdr[0..1])? {
        0 => return Ok(None),
        1 => {}
        _ => unreachable!(),
    }
    reader.read_exact(&mut hdr[1..OUTER_HEADER_SIZE])?;

    let parsed = FrameHeader8::from_bytes(hdr);
    if parsed.size < 0 || (parsed.size as usize) > MAX_FRAME_PAYLOAD_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid TCP frame size {} for frame id 0x{:x}",
                parsed.size, parsed.id
            ),
        ));
    }

    let mut payload = vec![0u8; parsed.size as usize];
    if !payload.is_empty() {
        reader.read_exact(&mut payload)?;
    }
    Ok(Some((parsed.id, payload)))
}

/// The `(frame_id, payload)` frames [`parse_tcp_frames`] slices from a buffer, in order.
pub type TcpFrames = Vec<(i32, Vec<u8>)>;

/// Slices as many complete `(frame_id, payload)` frames as available from a byte buffer.
///
/// Returns `Ok((frames, consumed))`, where `consumed` is the byte count of the complete frames, or
/// `Err(InvalidData)` if an invalid frame size
/// (`< 0` or `> 0x8000`) is encountered.
pub fn parse_tcp_frames(buf: &[u8]) -> io::Result<(TcpFrames, usize)> {
    let mut frames = Vec::new();
    let mut offset = 0usize;

    while buf.len().saturating_sub(offset) >= OUTER_HEADER_SIZE {
        let mut hdr_bytes = [0u8; OUTER_HEADER_SIZE];
        hdr_bytes.copy_from_slice(&buf[offset..offset + OUTER_HEADER_SIZE]);
        let hdr = FrameHeader8::from_bytes(hdr_bytes);
        if hdr.size < 0 || (hdr.size as usize) > MAX_FRAME_PAYLOAD_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid TCP frame size {} for frame id 0x{:x}",
                    hdr.size, hdr.id
                ),
            ));
        }
        let sz = hdr.size as usize;
        if buf.len() - offset < OUTER_HEADER_SIZE + sz {
            break;
        }
        let payload = buf[offset + OUTER_HEADER_SIZE..offset + OUTER_HEADER_SIZE + sz].to_vec();
        frames.push((hdr.id, payload));
        offset += OUTER_HEADER_SIZE + sz;
    }

    Ok((frames, offset))
}

/// Non-panicking cursor over a frame body.
///
/// Starts at offset `6`, the first field after the inner header (SPEC: PROTOCOL_2009.md §1.2, reader: the
/// first field is +6).
/// All read methods return `0` / empty / zero-padded data on out-of-bounds reads and
/// still advance the cursor by the requested width.
#[derive(Debug, Clone)]
pub struct PktReader<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> PktReader<'a> {
    /// Creates a reader over a frame body, starting at offset `6`.
    #[inline]
    pub fn new(payload: &'a [u8]) -> Self {
        Self {
            b: payload,
            o: INNER_HEADER_SIZE,
        }
    }

    /// Creates a reader starting at offset `0` (for headerless sub-structures).
    #[inline]
    pub fn from_raw(buf: &'a [u8]) -> Self {
        Self { b: buf, o: 0 }
    }

    /// Returns the underlying payload slice.
    #[inline]
    pub fn as_slice(&self) -> &'a [u8] {
        self.b
    }

    /// Returns the current cursor offset relative to the start of `payload`.
    #[inline]
    pub fn pos(&self) -> usize {
        self.o
    }

    /// Returns the total length of the underlying payload slice.
    #[inline]
    pub fn len(&self) -> usize {
        self.b.len()
    }

    /// Returns `true` if the underlying payload slice is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.b.is_empty()
    }

    /// Returns the number of unread bytes remaining from the current cursor position.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.b.len().saturating_sub(self.o)
    }

    /// Reads the inner `msg_id` at `payload[0..2]` (or `0` if too short).
    #[inline]
    pub fn inner_id(&self) -> u16 {
        if self.b.len() >= 2 {
            u16::from_le_bytes([self.b[0], self.b[1]])
        } else {
            0
        }
    }

    /// Reads the inner `msg_len` at `payload[2..6]` (or `0` if too short).
    #[inline]
    pub fn inner_size(&self) -> u32 {
        if self.b.len() >= 6 {
            u32::from_le_bytes([self.b[2], self.b[3], self.b[4], self.b[5]])
        } else {
            0
        }
    }

    /// Seeks the cursor to an absolute byte offset `pos` relative to the start of `payload`.
    #[inline]
    pub fn at(&mut self, pos: usize) -> &mut Self {
        self.o = pos;
        self
    }

    /// Advances the cursor by `n` bytes.
    #[inline]
    pub fn skip(&mut self, n: usize) -> &mut Self {
        self.o = self.o.saturating_add(n);
        self
    }

    #[inline]
    fn take_array<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.o.saturating_add(N);
        if end > self.b.len() {
            self.o = end;
            return None;
        }
        let mut arr = [0u8; N];
        arr.copy_from_slice(&self.b[self.o..end]);
        self.o = end;
        Some(arr)
    }

    #[inline]
    pub fn u8(&mut self) -> u8 {
        self.take_array::<1>().map(|a| a[0]).unwrap_or(0)
    }

    #[inline]
    pub fn i8(&mut self) -> i8 {
        self.take_array::<1>().map(|a| a[0] as i8).unwrap_or(0)
    }

    #[inline]
    pub fn u16(&mut self) -> u16 {
        self.take_array::<2>().map(u16::from_le_bytes).unwrap_or(0)
    }

    #[inline]
    pub fn i16(&mut self) -> i16 {
        self.take_array::<2>().map(i16::from_le_bytes).unwrap_or(0)
    }

    #[inline]
    pub fn u32(&mut self) -> u32 {
        self.take_array::<4>().map(u32::from_le_bytes).unwrap_or(0)
    }

    #[inline]
    pub fn i32(&mut self) -> i32 {
        self.take_array::<4>().map(i32::from_le_bytes).unwrap_or(0)
    }

    #[inline]
    pub fn u64(&mut self) -> u64 {
        self.take_array::<8>().map(u64::from_le_bytes).unwrap_or(0)
    }

    #[inline]
    pub fn i64(&mut self) -> i64 {
        self.take_array::<8>().map(i64::from_le_bytes).unwrap_or(0)
    }

    #[inline]
    pub fn f32(&mut self) -> f32 {
        self.take_array::<4>()
            .map(f32::from_le_bytes)
            .unwrap_or(0.0)
    }

    /// Returns a slice of up to `n` bytes starting at the current cursor and advances by `n`.
    /// Never panics on out-of-bounds: returns the available tail (or `&[]`) if truncated.
    #[inline]
    pub fn bytes(&mut self, n: usize) -> &'a [u8] {
        let start = self.o;
        let end = self.o.saturating_add(n);
        self.o = end;
        if start >= self.b.len() {
            &[]
        } else {
            let clamped_end = end.min(self.b.len());
            &self.b[start..clamped_end]
        }
    }

    /// Returns a `Vec<u8>` of length `n`, right-padded with zeros if the payload is truncated.
    #[inline]
    pub fn bytes_padded(&mut self, n: usize) -> Vec<u8> {
        let slice = self.bytes(n);
        let mut out = Vec::with_capacity(n);
        out.extend_from_slice(slice);
        if out.len() < n {
            out.resize(n, 0);
        }
        out
    }

    /// Reads a fixed `wstr[chars]` field (`chars * 2` bytes of UTF-16LE), stopping at the first NUL unit.
    // SPEC: PROTOCOL_2009.md conventions (`wstr[n]` is a fixed array of n UTF-16LE code units, NUL-padded)
    pub fn wstr(&mut self, chars: usize) -> String {
        let byte_len = chars.saturating_mul(2);
        let raw = self.bytes_padded(byte_len);
        let mut units = Vec::with_capacity(chars);
        for chunk in raw.as_chunks::<2>().0 {
            let u = u16::from_le_bytes([chunk[0], chunk[1]]);
            if u == 0 {
                break;
            }
            units.push(u);
        }
        String::from_utf16_lossy(&units)
    }
}

/// Builder for a frame body (inner header + packed fields) and the complete frame.
///
/// SPEC: PROTOCOL_2009.md §1.2. HARNESS: rust_refdump.py (`Wr`: u8/u16/u32/i32/i64/f32 bits/wstr/at, frame);
/// the replay test in `tests/ref_vectors.rs` checks byte equality against 1000 reference frames.
#[derive(Debug, Clone)]
pub struct PktWriter {
    id: u16,
    has_header: bool,
    b: Vec<u8>,
    o: usize,
}

impl PktWriter {
    /// Creates a writer whose body starts with the inner header (`u16 msg_id = id`, `u32 msg_len` patched later).
    /// Cursor starts at offset `6`.
    #[inline]
    pub fn new(id: u16) -> Self {
        let mut b = Vec::with_capacity(64);
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        Self {
            id,
            has_header: true,
            b,
            o: INNER_HEADER_SIZE,
        }
    }

    /// Creates a headerless writer for building sub-records (fixed-size rows embedded in a body).
    #[inline]
    pub fn empty() -> Self {
        Self {
            id: 0,
            has_header: false,
            b: Vec::with_capacity(64),
            o: 0,
        }
    }

    /// Alias for [`PktWriter::empty`].
    #[inline]
    pub fn body() -> Self {
        Self::empty()
    }

    /// Returns the packet ID passed to [`PktWriter::new`].
    #[inline]
    pub fn id(&self) -> u16 {
        self.id
    }

    /// Returns the current cursor position.
    #[inline]
    pub fn pos(&self) -> usize {
        self.o
    }

    /// Returns the current total byte length of the underlying buffer.
    #[inline]
    pub fn len(&self) -> usize {
        self.b.len()
    }

    /// Returns `true` if the underlying buffer has 0 bytes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.b.is_empty()
    }

    /// Moves the write cursor to `pos`. If `pos > self.b.len()`, zero-extends the buffer to `pos`.
    #[inline]
    pub fn at(&mut self, pos: usize) -> &mut Self {
        if pos > self.b.len() {
            self.b.resize(pos, 0);
        }
        self.o = pos;
        self
    }

    #[inline]
    fn put(&mut self, raw: &[u8]) -> &mut Self {
        let end = self.o + raw.len();
        if end > self.b.len() {
            self.b.resize(end, 0);
        }
        self.b[self.o..end].copy_from_slice(raw);
        self.o = end;
        self
    }

    #[inline]
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.put(&[v])
    }

    #[inline]
    pub fn i8(&mut self, v: i8) -> &mut Self {
        self.put(&[v as u8])
    }

    #[inline]
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.put(&v.to_le_bytes())
    }

    #[inline]
    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.put(&v.to_le_bytes())
    }

    #[inline]
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.put(&v.to_le_bytes())
    }

    #[inline]
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.put(&v.to_le_bytes())
    }

    #[inline]
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.put(&v.to_le_bytes())
    }

    #[inline]
    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.put(&v.to_le_bytes())
    }

    #[inline]
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.put(&v.to_le_bytes())
    }

    /// Writes a 4-byte field from its raw IEEE-754 bit pattern (bit exact, also for NaN payloads).
    #[inline]
    pub fn f32_bits(&mut self, bits: u32) -> &mut Self {
        self.put(&bits.to_le_bytes())
    }

    /// Writes raw bytes at the current cursor position.
    #[inline]
    pub fn bytes(&mut self, data: &[u8]) -> &mut Self {
        self.put(data)
    }

    /// Alias for [`PktWriter::bytes`].
    #[inline]
    pub fn raw(&mut self, data: &[u8]) -> &mut Self {
        self.put(data)
    }

    /// Writes `n` zero bytes at the current cursor position.
    #[inline]
    pub fn pad(&mut self, n: usize) -> &mut Self {
        let end = self.o + n;
        if end > self.b.len() {
            self.b.resize(end, 0);
        } else {
            self.b[self.o..end].fill(0);
        }
        self.o = end;
        self
    }

    /// Writes a fixed `wstr[chars]` field: at most `chars - 1` UTF-16 code units, NUL-padded to `chars * 2` bytes
    /// so the field is always terminated.
    // SPEC: PROTOCOL_2009.md conventions (`wstr[n]`). HARNESS: rust_refdump.py (`Wr.wstr`, truncation to n-1 units)
    pub fn wstr(&mut self, s: &str, chars: usize) -> &mut Self {
        if chars == 0 {
            return self;
        }
        let byte_len = chars * 2;
        let max_units = chars - 1;
        let mut buf = vec![0u8; byte_len];
        for (i, u) in s.encode_utf16().take(max_units).enumerate() {
            let le = u.to_le_bytes();
            buf[i * 2] = le[0];
            buf[i * 2 + 1] = le[1];
        }
        self.put(&buf)
    }

    /// Returns a slice of the raw buffer without patching `b[2..6]` (useful for headerless
    /// sub-records created with [`PktWriter::empty`]).
    #[inline]
    pub fn raw_bytes(&self) -> &[u8] {
        &self.b
    }

    /// Returns the body with `msg_len` (`b[2..6]`) patched to the body length.
    #[inline]
    pub fn to_payload(&self) -> Vec<u8> {
        let mut out = self.b.clone();
        if self.has_header && out.len() >= INNER_HEADER_SIZE {
            let sz = (out.len() as u32).to_le_bytes();
            out[2..6].copy_from_slice(&sz);
        }
        out
    }

    /// Alias for [`PktWriter::to_payload`].
    #[inline]
    pub fn payload(&self) -> Vec<u8> {
        self.to_payload()
    }

    /// Consumes the writer and returns the body with `msg_len` patched.
    #[inline]
    pub fn into_payload(mut self) -> Vec<u8> {
        if self.has_header && self.b.len() >= INNER_HEADER_SIZE {
            let sz = (self.b.len() as u32).to_le_bytes();
            self.b[2..6].copy_from_slice(&sz);
        }
        self.b
    }

    /// Returns a complete frame: `i32 frame_id`, `i32 body_len`, then the patched body.
    #[inline]
    pub fn to_frame(&self) -> Vec<u8> {
        let p = self.to_payload();
        let hdr = FrameHeader8 {
            id: self.id as i32,
            size: p.len() as i32,
        }
        .to_bytes();
        let mut out = Vec::with_capacity(OUTER_HEADER_SIZE + p.len());
        out.extend_from_slice(&hdr);
        out.extend_from_slice(&p);
        out
    }

    /// Alias for [`PktWriter::to_frame`].
    #[inline]
    pub fn frame(&self) -> Vec<u8> {
        self.to_frame()
    }

    /// Consumes the writer and returns the complete 8-byte-prefixed TCP wire frame.
    #[inline]
    pub fn into_frame(self) -> Vec<u8> {
        let id = self.id as i32;
        let p = self.into_payload();
        let hdr = FrameHeader8 {
            id,
            size: p.len() as i32,
        }
        .to_bytes();
        let mut out = Vec::with_capacity(OUTER_HEADER_SIZE + p.len());
        out.extend_from_slice(&hdr);
        out.extend_from_slice(&p);
        out
    }
}
