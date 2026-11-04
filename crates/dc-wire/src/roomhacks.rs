//! A modded room's Hacks setting (LG-274, project decision). Our own design, no 2009 counterpart.
//!
//! **What.** A lobby whose loaded mod set declares `world.full_visibility` (MOD-22) and / or `combat.no_recoil`
//! (MOD-23) offers its rooms a Hacks setting: wallhacks (culling off), no-recoil (no recoil kick, no spread), or both.
//! The set decides what is offered; the room's host picks one of the offered combinations.
//!
//! **Carrier.** The 2009 room record (0x3035 field types, 0x38c requests) has no free field for it, and an unknown
//! 0x3035 type would reach the 2009 waiting room's per-type state. So the setting travels in our own ids on the lobby
//! connection, which the 2009 dispatcher drops (SPEC: PROTOCOL_2009.md §1.2, LIVE for 0x3f01):
//!
//! ```text
//! 0x3f03 dc_room_hacks_push     (S->C; to a member after the 0x389 room info, and to every member on a change)
//!   +6  u32 offered   bit 0 = wallhacks offered, bit 1 = no-recoil offered
//!   +10 u32 value     the room's setting: bit 0 = wallhacks on, bit 1 = no-recoil on
//! 0x3f04 dc_room_hacks_request  (C->S; the host, while the room waits)
//!   +6  u32 value     the wanted setting (answered by a 0x3f03 with the room's value, changed or not)
//! ```
//!
//! A lobby that offers nothing sends no 0x3f03 (a vanilla room has no Hacks row) and ignores 0x3f04.
// DC-DEFAULT: LG-274 (proposed, project decision) the room hacks rule and its carrier

use crate::tcp::PktWriter;

/// Our push id (not a 2009 id).
pub const ROOM_HACKS_PUSH: u16 = crate::msg_ids::DC_ROOM_HACKS_PUSH;
/// Our request id (not a 2009 id).
pub const ROOM_HACKS_REQUEST: u16 = crate::msg_ids::DC_ROOM_HACKS_REQUEST;

/// Bit of `offered` / `value`: wallhacks (the match's culling off, the x-ray render hook on).
pub const WALLHACKS: u32 = 1;
/// Bit of `offered` / `value`: no-recoil (no recoil kick, no spread; the game server's cone check off).
pub const NO_RECOIL: u32 = 2;

/// The settings a room may take under `offered`, in the dropdown's order: Wallhacks, Wallhacks + no-recoil,
/// No-recoil only (each only when every bit it needs is offered). Empty when nothing is offered.
pub fn choices(offered: u32) -> Vec<u32> {
    [WALLHACKS, WALLHACKS | NO_RECOIL, NO_RECOIL]
        .into_iter()
        .filter(|v| v & !offered == 0 && offered != 0)
        .collect()
}

/// The setting a new room starts with: wallhacks when offered, else the first choice; 0 when nothing is offered.
pub fn default_value(offered: u32) -> u32 {
    choices(offered).first().copied().unwrap_or(0)
}

/// Whether `value` is one of the [`choices`] of `offered`.
pub fn allowed(offered: u32, value: u32) -> bool {
    choices(offered).contains(&value)
}

/// The 0x3f03 frame.
pub fn push_frame(offered: u32, value: u32) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_HACKS_PUSH);
    w.u32(offered);
    w.u32(value);
    w.into_frame()
}

/// The 0x3f04 frame.
pub fn request_frame(value: u32) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_HACKS_REQUEST);
    w.u32(value);
    w.into_frame()
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Reads a 0x3f03 payload (inner header included): `(offered, value)`.
pub fn parse_push(d: &[u8]) -> Option<(u32, u32)> {
    Some((u32_at(d, 6)?, u32_at(d, 10)?))
}

/// Reads a 0x3f04 payload (inner header included): the wanted value.
pub fn parse_request(d: &[u8]) -> Option<u32> {
    u32_at(d, 6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_follow_what_the_set_offers() {
        assert!(choices(0).is_empty());
        assert_eq!(default_value(0), 0);
        assert_eq!(choices(WALLHACKS), vec![WALLHACKS]);
        assert_eq!(choices(NO_RECOIL), vec![NO_RECOIL]);
        assert_eq!(default_value(NO_RECOIL), NO_RECOIL);
        assert_eq!(
            choices(WALLHACKS | NO_RECOIL),
            vec![WALLHACKS, WALLHACKS | NO_RECOIL, NO_RECOIL]
        );
        assert_eq!(default_value(WALLHACKS | NO_RECOIL), WALLHACKS);
        assert!(!allowed(WALLHACKS, NO_RECOIL));
        assert!(!allowed(WALLHACKS | NO_RECOIL, 0));
        assert!(!allowed(WALLHACKS | NO_RECOIL, 4));
    }

    #[test]
    fn frames_round_trip() {
        let f = push_frame(3, 2);
        assert_eq!(u16::from_le_bytes([f[8], f[9]]), ROOM_HACKS_PUSH);
        assert_eq!(parse_push(&f[8..]), Some((3, 2)));
        let f = request_frame(3);
        assert_eq!(u16::from_le_bytes([f[8], f[9]]), ROOM_HACKS_REQUEST);
        assert_eq!(parse_request(&f[8..]), Some(3));
        assert_eq!(parse_push(&[0; 8]), None);
    }
}
