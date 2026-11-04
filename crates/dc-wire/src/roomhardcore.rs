//! A room's Hardcore setting (LG-306, project decision). Our own design, no 2009 counterpart.
//!
//! A room on a server entry with `features = ["hardcore"]` may turn Hardcore on: effective health 20 % of normal, in
//! every mode, on top of any weapon limit. The 2009 weapon limit is one exclusive choice and its list is the ROM's, so
//! Hardcore is its own setting, not a row of that list.
//!
//! ```text
//! 0x3f08 dc_room_hardcore_push    (S->C; to a member after the 0x389 room info, to every member on a change)
//!   +6  u32 value   0 = off, 1 = on
//! 0x3f09 dc_room_hardcore_request (C->S; the host, while the room waits; answered by a 0x3f08)
//!   +6  u32 value
//! ```
//!
//! A lobby entry without the feature sends no 0x3f08 and changes nothing on 0x3f09. The 2009 dispatcher drops both.
// DC-DEFAULT: LG-306 (proposed, project decision) the Hardcore setting and its carrier

use crate::tcp::PktWriter;

/// Our push id (not a 2009 id).
pub const PUSH: u16 = crate::msg_ids::DC_ROOM_HARDCORE_PUSH;
/// Our request id (not a 2009 id).
pub const REQUEST: u16 = crate::msg_ids::DC_ROOM_HARDCORE_REQUEST;
/// The game server's value: the share of normal health a player effectively has (DSV-482).
pub const HEALTH_SCALE: &str = "0.2";

/// The 0x3f08 frame.
pub fn push_frame(on: bool) -> Vec<u8> {
    let mut w = PktWriter::new(PUSH);
    w.u32(on as u32);
    w.into_frame()
}

/// The 0x3f09 frame.
pub fn request_frame(on: bool) -> Vec<u8> {
    let mut w = PktWriter::new(REQUEST);
    w.u32(on as u32);
    w.into_frame()
}

/// Reads a 0x3f08 or 0x3f09 payload (inner header included).
pub fn parse(d: &[u8]) -> Option<bool> {
    d.get(6..10)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let f = push_frame(true);
        assert_eq!(u16::from_le_bytes([f[8], f[9]]), PUSH);
        assert_eq!(parse(&f[8..]), Some(true));
        let f = request_frame(false);
        assert_eq!(u16::from_le_bytes([f[8], f[9]]), REQUEST);
        assert_eq!(parse(&f[8..]), Some(false));
        assert_eq!(parse(&[0; 8]), None);
    }
}
