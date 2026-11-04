//! The per-match seat ticket (LG-327, project decision). Our own design, no 2009 counterpart.
//!
//! The 2009 hello names a player and proves nothing, so anyone who knows a game port and a name could take that
//! player's seat. The lobby therefore gives each player of a match a random ticket, and the game server seats a roster
//! name only for the hello that carries its ticket. Nothing compares addresses: players behind one carrier NAT share
//! theirs, and a player's address may change.
//!
//! ```text
//! 0x3f0c dc_seat_ticket_push (S->C, lobby connection, right before each 0x3043 of a match with a lobby-run server)
//!   +6  u8[16] ticket
//! user line of the game server (DSERVER_PIPE.md §3.1): trailing token ticket=<32 lower-case hex>
//! C2S_HELLO blob: after the 0x102 bytes of the 2009 blob, ascii "DCST", then u8[16] ticket
//! ```
//!
//! A server without the feature reads the 0x102 bytes it knows, whose UTF-16 name ends inside them, and ignores the rest.
//! The 2009 dispatcher drops 0x3f0c (SPEC: PROTOCOL_2009.md §1.2). The ticket is the same for every 0x3f0c of one
//! player in one match, so leaving and rejoining keeps working, and it is over with that match's server.
// DC-DEFAULT: LG-327 (proposed, project decision) the seat ticket and its carriers

use crate::tcp::PktWriter;

/// Our push id (not a 2009 id).
pub const PUSH: u16 = crate::msg_ids::DC_SEAT_TICKET_PUSH;
/// Ticket size: 128 random bits.
pub const LEN: usize = 16;
/// The `user` line's token.
pub const TOKEN: &str = "ticket";
/// Where the extension starts in the hello blob: right after the 2009 blob.
// SPEC: UDP_SESSION_2009_A.md §2.3 (0x102 bytes)
pub const BLOB_OFFSET: usize = 0x102;
/// Marks the extension in the hello blob.
pub const BLOB_MARK: &[u8; 4] = b"DCST";

/// One ticket.
pub type Ticket = [u8; LEN];

/// The 0x3f0c frame.
pub fn push_frame(t: &Ticket) -> Vec<u8> {
    let mut w = PktWriter::new(PUSH);
    w.raw(t);
    w.into_frame()
}

/// Reads a 0x3f0c payload (inner header included).
pub fn parse_push(d: &[u8]) -> Option<Ticket> {
    d.get(6..6 + LEN)?.try_into().ok()
}

/// Appends the extension to a 0x102-byte hello blob (a shorter blob is padded first: the place is fixed).
pub fn append_to_blob(blob: &mut Vec<u8>, t: &Ticket) {
    blob.resize(BLOB_OFFSET, 0);
    blob.extend_from_slice(BLOB_MARK);
    blob.extend_from_slice(t);
}

/// The ticket of a hello blob; `None` for a blob without the extension.
pub fn from_blob(blob: &[u8]) -> Option<Ticket> {
    let rest = blob.get(BLOB_OFFSET..)?;
    if !rest.starts_with(BLOB_MARK) {
        return None;
    }
    rest.get(BLOB_MARK.len()..BLOB_MARK.len() + LEN)?
        .try_into()
        .ok()
}

/// The `user` line's form of a ticket.
pub fn to_hex(t: &Ticket) -> String {
    t.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads [`to_hex`]; `None` for anything else.
pub fn from_hex(s: &str) -> Option<Ticket> {
    let b = s.as_bytes();
    if b.len() != 2 * LEN {
        return None;
    }
    let mut t = [0u8; LEN];
    for (i, o) in t.iter_mut().enumerate() {
        let d = |c: u8| (c as char).to_digit(16);
        *o = (d(b[2 * i])? * 16 + d(b[2 * i + 1])?) as u8;
    }
    Some(t)
}

/// Compares two tickets in a time that does not depend on where they differ.
#[inline(never)]
pub fn same(a: &Ticket, b: &Ticket) -> bool {
    let mut d = 0u8;
    for i in 0..LEN {
        d |= std::hint::black_box(a[i] ^ b[i]);
    }
    std::hint::black_box(d) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carriers_round_trip() {
        let t: Ticket = core::array::from_fn(|i| 0xa0 + i as u8);
        let f = push_frame(&t);
        assert_eq!(u16::from_le_bytes([f[8], f[9]]), PUSH);
        assert_eq!(parse_push(&f[8..]), Some(t));
        assert_eq!(parse_push(&[0; 8]), None);
        assert_eq!(from_hex(&to_hex(&t)), Some(t));
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex(&"zz".repeat(LEN)), None);
        let mut blob = vec![1u8; BLOB_OFFSET];
        assert_eq!(from_blob(&blob), None);
        append_to_blob(&mut blob, &t);
        assert_eq!(blob.len(), BLOB_OFFSET + 4 + LEN);
        assert_eq!(from_blob(&blob), Some(t));
        assert_eq!(from_blob(&blob[..blob.len() - 1]), None, "cut short");
        assert!(same(&t, &t));
        let mut o = t;
        o[LEN - 1] ^= 1;
        assert!(!same(&t, &o));
    }
}
