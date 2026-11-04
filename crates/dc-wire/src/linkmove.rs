//! The link move secret (NET-23 hardening, NET-24, project decision). Our own design, no 2009 counterpart.
//!
//! **Why.** NET-23 lets the game server move a link to a new source port of the same IP after a NAT rebinding. The
//! 2009 datagram proves nothing about its sender (the check byte is a CRC fold, the sequence numbers are 13 bits), so
//! anyone behind the same public address (a carrier NAT, a campus, a shared Wi-Fi) could guess their way onto a
//! seated player's link. The server therefore gives each link a random 64-bit secret, and moves a link only for a
//! datagram that carries it.
//!
//! **Server -> client.** A shell message (`S2C_SHELL_MSG`, engine id 13) with our own shell id **0xf3**, sent once per
//! link when the player enters the world (reliable). The 2009 client drops a shell id that no stage of its dispatch
//! handles (SPEC: GAME_MESSAGES_2009_S2C.md §7.2, row 0x9d), so it simply never migrates.
//!
//! ```text
//! [13][0xf3][secret: 8 bytes]                                       (reliable)
//! ```
//!
//! **Client -> server.** From then on every link datagram of our client ends with a trailer behind its body. The body's
//! framing is closed explicitly first (`has_unrel = 0`, or a zero-length unreliable entry when it has an unreliable
//! block; a bye also gets `has_rel = 0`), so any receiver that follows the 2009 framing stops before the trailer, and
//! the check byte covers the trailer too:
//!
//! ```text
//! [check8][body bits, explicitly closed, zero-padded to a byte][TRAILER_MARK][secret: 8 bytes]
//! ```
// DC-DEFAULT: NET-24 (proposed, project decision) the link move secret and its carriers

use crate::udp::{BitReader, BitWriter};

/// Our shell id (not a 2009 id).
pub const SHELL_ID: u8 = 0xf3;
/// Secret size: 64 random bits.
pub const LEN: usize = 8;
/// First byte of the datagram trailer.
pub const TRAILER_MARK: u8 = 0xd3;
/// Trailer size in bytes (mark + secret).
pub const TRAILER_LEN: usize = 1 + LEN;

/// One secret.
pub type Secret = [u8; LEN];

/// The shell message body after the shell id.
pub fn write_body(w: &mut BitWriter, s: &Secret) {
    for b in s {
        w.write_u8(*b);
    }
}

/// Reads [`write_body`]; `None` when it is cut short.
pub fn read_body(r: &mut BitReader<'_>) -> Option<Secret> {
    if r.remaining_bits() < LEN * 8 {
        return None;
    }
    let mut s = [0u8; LEN];
    for b in s.iter_mut() {
        *b = r.read_u8();
    }
    Some(s)
}

/// The trailer bytes for `s`.
pub fn trailer(s: &Secret) -> [u8; TRAILER_LEN] {
    let mut t = [0u8; TRAILER_LEN];
    t[0] = TRAILER_MARK;
    t[1..].copy_from_slice(s);
    t
}

/// The secret of a trailer (`bytes` exactly [`TRAILER_LEN`] long, starting with the mark).
pub fn from_trailer(bytes: &[u8]) -> Option<Secret> {
    if bytes.len() != TRAILER_LEN || bytes[0] != TRAILER_MARK {
        return None;
    }
    bytes[1..].try_into().ok()
}

/// Compares two secrets in time independent of where they differ.
pub fn same_secret(a: &Secret, b: &Secret) -> bool {
    let mut d = 0u8;
    for i in 0..LEN {
        d |= a[i] ^ b[i];
    }
    std::hint::black_box(d) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_and_trailer_round_trip() {
        let s = [1, 2, 3, 4, 5, 6, 7, 0xff];
        let mut w = BitWriter::new();
        write_body(&mut w, &s);
        assert_eq!(read_body(&mut w.reader()), Some(s));
        assert_eq!(
            read_body(&mut BitWriter::from_bytes(&[1, 2]).reader()),
            None
        );
        let t = trailer(&s);
        assert_eq!(from_trailer(&t), Some(s));
        assert_eq!(from_trailer(&t[1..]), None);
        let mut bad = t;
        bad[0] ^= 1;
        assert_eq!(from_trailer(&bad), None);
    }

    #[test]
    fn secrets_compare_whole() {
        let a = [9u8; LEN];
        let mut b = a;
        assert!(same_secret(&a, &b));
        b[LEN - 1] ^= 0x80;
        assert!(!same_secret(&a, &b));
        b = a;
        b[0] = 0;
        assert!(!same_secret(&a, &b));
    }
}
