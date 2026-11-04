//! Other players' health in a wallhacks match (DSV-480, USER REQUEST 2026-11-01). Our own design, no 2009 counterpart.
//!
//! **What.** The 2009 server tells a client only its own health (`GS_PLAYER_STATUS` 0x66 kinds 2 / 0x0f) and the
//! 2009 client shows no other player's health. A game server whose culling is off (`-visibility off`: a room with
//! wallhacks, DSV-479 / MOD-22) also sends every player's health to every client, for the x-ray health bars
//! (CLI-1631). A culled match (every vanilla match, a "No-recoil only" room) sends nothing of it.
//!
//! **Carrier.** A shell message (`S2C_SHELL_MSG`, engine id 13) with our own shell id **0xf1**, an id no stage of the
//! 2009 shell dispatch handles: an id none of them lists is dropped silently by the 2009 client (SPEC:
//! GAME_MESSAGES_2009_S2C.md §7.2, row 0x9d: not handled by any stage, dropped, no reaction).
//!
//! ```text
//! [13][0xf1][count:u8] count x { [pid:u8][health:u16][max:u16] }   (bit stream, little-endian fields)
//! ```
//!
//! `pid` is the roster id (the engine client id), `health` the current health rounded up (0: dead), `max` the
//! player's maximum (>= 1). One message carries the players whose value changed since the last one (reliable), and a
//! client that joins gets every player once.
// DC-DEFAULT: DSV-480 (proposed, USER REQUEST 2026-11-01) remote health in wallhacks matches and its carrier

use crate::udp::{BitReader, BitWriter};

/// Our shell id (not a 2009 id).
pub const SHELL_ID: u8 = 0xf1;

/// Most entries in one message.
pub const MAX_ENTRIES: usize = 255;

/// Bits of one entry.
const ENTRY_BITS: usize = 8 + 16 + 16;

/// One player's health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthEntry {
    pub pid: u8,
    pub health: u16,
    pub max: u16,
}

/// The message body after the shell id (the first [`MAX_ENTRIES`] of `entries`).
pub fn write_body(w: &mut BitWriter, entries: &[HealthEntry]) {
    let n = entries.len().min(MAX_ENTRIES);
    w.write_u8(n as u8);
    for e in &entries[..n] {
        w.write_u8(e.pid).write_u16(e.health).write_u16(e.max);
    }
}

/// Reads a body written by [`write_body`]; `None` when it is cut short.
pub fn read_body(r: &mut BitReader<'_>) -> Option<Vec<HealthEntry>> {
    if r.remaining_bits() < 8 {
        return None;
    }
    let n = r.read_u8() as usize;
    if r.remaining_bits() < n * ENTRY_BITS {
        return None;
    }
    Some(
        (0..n)
            .map(|_| HealthEntry {
                pid: r.read_u8(),
                health: r.read_u16(),
                max: r.read_u16(),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_round_trips_and_a_cut_body_is_refused() {
        let es = [
            HealthEntry { pid: 3, health: 57, max: 100 },
            HealthEntry { pid: 9, health: 0, max: 1500 },
        ];
        let mut w = BitWriter::new();
        write_body(&mut w, &es);
        assert_eq!(w.bit_len(), 8 + 2 * ENTRY_BITS);
        assert_eq!(w.as_bytes(), &[2, 3, 57, 0, 100, 0, 9, 0, 0, 0xdc, 0x05]);
        let mut r = w.reader();
        assert_eq!(read_body(&mut r).unwrap(), es.to_vec());
        let bytes = w.as_bytes()[..6].to_vec();
        assert_eq!(read_body(&mut BitReader::with_bit_len(&bytes, 48)), None);
    }
}
