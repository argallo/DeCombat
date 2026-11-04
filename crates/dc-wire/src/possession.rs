//! The room's Possession Arms and the possession move (LG-307, DSV-483, project decision). Our own design, no
//! 2009 counterpart.
//!
//! **Room.** A room on a server entry with `features = ["possession"]` may pick "Possession" in its Arms list. The
//! ROM's limit stays 0 ("all weapons") on every 2009 message, the choice travels on the lobby connection:
//!
//! ```text
//! 0x3f0a dc_room_possession_push    (S->C; to a member after the 0x389 room info, to every member on a change)
//!   +6  u32 value   0 = off, 1 = on
//! 0x3f0b dc_room_possession_request (C->S; the host, while the room waits; answered by a 0x3f0a)
//!   +6  u32 value
//! ```
//!
//! **Match.** A shell message (`S2C_SHELL_MSG`, engine id 13) with our own shell id **0xf2**, an id no stage of the
//! 2009 shell dispatch handles (such an id is dropped, SPEC: GAME_MESSAGES_2009_S2C.md §7.2, row 0x9d):
//!
//! ```text
//! [13][0xf2][to_x:f32][to_y:f32][to_z:f32][ms:u16]
//! ```
//!
//! The killer's client glides its body to `to` over `ms`, without steering or firing, until the server's closing
//! `GS_MOVER_UPDATE` teleport at `to`. A client that does not know the message just gets that teleport.
// DC-DEFAULT: LG-307 (proposed, project decision) the room setting; DSV-483 (proposed) the move message

use crate::tcp::PktWriter;
use crate::udp::{BitReader, BitWriter};

/// Our push id (not a 2009 id).
pub const PUSH: u16 = crate::msg_ids::DC_ROOM_POSSESSION_PUSH;
/// Our request id (not a 2009 id).
pub const REQUEST: u16 = crate::msg_ids::DC_ROOM_POSSESSION_REQUEST;
/// Our shell id (not a 2009 id).
pub const SHELL_ID: u8 = 0xf2;
/// The name of the Arms row.
pub const ARMS_NAME: &str = "Possession";

/// The mode classes without Possession (USER 2026-11-01): 5 Fireteam, 9 Spy Hunt, 11 Quarantine.
// SPEC: GAME_MODES_2009.md §2 (the row's `GameType`)
pub const REFUSED_CLASSES: [u32; 3] = [5, 9, 11];

/// Whether a room of mode class `class` may pick Possession.
pub fn class_allows(class: u32) -> bool {
    !REFUSED_CLASSES.contains(&class)
}

/// The 0x3f0a frame.
pub fn push_frame(on: bool) -> Vec<u8> {
    let mut w = PktWriter::new(PUSH);
    w.u32(on as u32);
    w.into_frame()
}

/// Reads a 0x3f0a or 0x3f0b payload (inner header included).
pub fn parse(d: &[u8]) -> Option<bool> {
    d.get(6..10)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) != 0)
}

/// The move message's body after the shell id.
pub fn write_move(w: &mut BitWriter, to: [f32; 3], ms: u16) {
    w.write_f32(to[0])
        .write_f32(to[1])
        .write_f32(to[2])
        .write_u16(ms);
}

/// Reads a body written by [`write_move`]; `None` when it is cut short or not finite.
pub fn read_move(r: &mut BitReader<'_>) -> Option<([f32; 3], u16)> {
    if r.remaining_bits() < 3 * 32 + 16 {
        return None;
    }
    let to = [r.read_f32(), r.read_f32(), r.read_f32()];
    let ms = r.read_u16();
    to.iter().all(|c| c.is_finite()).then_some((to, ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let f = push_frame(true);
        assert_eq!(u16::from_le_bytes([f[8], f[9]]), PUSH);
        assert_eq!(parse(&f[8..]), Some(true));
        assert_eq!(parse(&[0; 8]), None);
        let mut b = BitWriter::new();
        write_move(&mut b, [1.0, -2.5, 3.0], 480);
        assert_eq!(read_move(&mut b.reader()), Some(([1.0, -2.5, 3.0], 480)));
        let cut = BitWriter::new();
        assert_eq!(read_move(&mut cut.reader()), None);
        assert!(class_allows(1) && class_allows(6));
        assert!(!class_allows(5) && !class_allows(9) && !class_allows(11));
    }
}
