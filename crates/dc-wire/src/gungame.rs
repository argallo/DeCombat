//! Gun Game (LG-305, project decision). Our own design, no 2009 counterpart.
//!
//! **What.** A room of a server entry with `features = ["gun_game"]` may play One Man Army's match (the 2009 mode id,
//! unchanged on every 2009 message) under a gun ladder. The ladder is drawn by the lobby at each start.
//!
//! **Carrier.** Our own ids on the lobby connection, which the 2009 dispatcher drops (SPEC: PROTOCOL_2009.md §1.2,
//! LIVE for 0x3f01, 0x3f03). Nothing of it is sent on an entry without the feature:
//!
//! ```text
//! 0x3f05 dc_gun_game_offer  (S->C; after the 0x301 channel enter reply on a featured entry)
//!   +6  u32 offered         1
//! 0x3f06 dc_gun_game_request (C->S; the host, while the room waits; the room's mode must be free-for-all)
//!   +6  u32 value           1 = Gun Game, 0 = plain One Man Army
//! 0x3f07 dc_gun_game_room   (S->C; to the channel on a change and after the room list; to a member after the room
//!                            info and just before the match address)
//!   +6  u32 room            room number in the channel
//!   +10 u32 value           1 = Gun Game, 0 = not
//!   +14 u32 count           ladder length (0 before the room's start and in the channel's copies)
//!   +18 u16[count] ladder   weapon numbers, rung 0 first
//! ```
// DC-DEFAULT: LG-305 (proposed, project decision) the Gun Game carrier

use crate::tcp::PktWriter;

/// Our offer push (not a 2009 id).
pub const OFFER: u16 = crate::msg_ids::DC_GUN_GAME_OFFER;
/// Our request (not a 2009 id).
pub const REQUEST: u16 = crate::msg_ids::DC_GUN_GAME_REQUEST;
/// Our room push (not a 2009 id).
pub const ROOM: u16 = crate::msg_ids::DC_GUN_GAME_ROOM;

/// The name players see; ours, the ROM has no such string.
pub const MODE_NAME: &str = "Gun Game";
/// The most rungs a frame carries (far above any ROM's gun list).
pub const MAX_LADDER: usize = 512;

/// The 0x3f05 frame.
pub fn offer_frame() -> Vec<u8> {
    let mut w = PktWriter::new(OFFER);
    w.u32(1);
    w.into_frame()
}

/// The 0x3f06 frame.
pub fn request_frame(on: bool) -> Vec<u8> {
    let mut w = PktWriter::new(REQUEST);
    w.u32(on as u32);
    w.into_frame()
}

/// The 0x3f07 frame.
pub fn room_frame(room: u32, on: bool, ladder: &[u16]) -> Vec<u8> {
    let ladder = &ladder[..ladder.len().min(MAX_LADDER)];
    let mut w = PktWriter::new(ROOM);
    w.u32(room).u32(on as u32).u32(ladder.len() as u32);
    for g in ladder {
        w.bytes(&g.to_le_bytes());
    }
    w.into_frame()
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Reads a 0x3f06 payload (inner header included).
pub fn parse_request(d: &[u8]) -> Option<bool> {
    u32_at(d, 6).map(|v| v != 0)
}

/// Reads a 0x3f07 payload (inner header included): room, value, ladder.
pub fn parse_room(d: &[u8]) -> Option<(u32, bool, Vec<u16>)> {
    let room = u32_at(d, 6)?;
    let on = u32_at(d, 10)? != 0;
    let n = u32_at(d, 14)? as usize;
    if n > MAX_LADDER {
        return None;
    }
    let body = d.get(18..18 + n * 2)?;
    // `body` is exactly n * 2 bytes, so there is no remainder
    let ladder = body
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    Some((room, on, ladder))
}

fn mix(x: &mut u64) -> u64 {
    // SplitMix64, public algorithm (Steele, Lea, Flood 2014)
    *x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The ladder of `seed`: a Fisher-Yates shuffle of `pool` (sorted and deduplicated first, so the seed alone reproduces
/// it for one ROM), cut to `goal` rungs. Primaries and secondaries mix at random: no fixed pattern a player can learn.
// DC-DEFAULT: LG-305 (proposed) the draw
pub fn draw_ladder(pool: &[u16], seed: u64, goal: u32) -> Vec<u16> {
    let mut v = pool.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut s = seed;
    for i in (1..v.len()).rev() {
        let j = (mix(&mut s) % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
    v.truncate(goal as usize);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_reproduces_the_ladder_without_repeats_and_the_goal_caps_it() {
        let pool: Vec<u16> = (1..=40).rev().collect();
        let a = draw_ladder(&pool, 7, 20);
        assert_eq!(a, draw_ladder(&pool, 7, 20));
        assert_ne!(a, draw_ladder(&pool, 8, 20));
        assert_eq!(a.len(), 20);
        let mut b = a.clone();
        b.sort_unstable();
        b.dedup();
        assert_eq!(b.len(), 20, "no repeats");
        assert_eq!(draw_ladder(&pool, 7, 99).len(), 40, "capped at the guns");
        assert_eq!(draw_ladder(&[3, 3, 3], 1, 3), vec![3]);
    }

    #[test]
    fn frames_round_trip() {
        let f = room_frame(4, true, &[7, 300]);
        assert_eq!(u16::from_le_bytes([f[8], f[9]]), ROOM);
        assert_eq!(parse_room(&f[8..]), Some((4, true, vec![7, 300])));
        let f = request_frame(true);
        assert_eq!(parse_request(&f[8..]), Some(true));
        assert_eq!(parse_room(&[0; 8]), None);
    }
}
