//! Our reason code on a refused 0x393 `room_user_flag_reply` (LG-342, project decision). No 2009 counterpart.
//!
//! The lobby refuses a Ready / Start / late join whose equipped weapons fail the room's weapon limit (LG-66) with
//! `0x393 {ok 0, reason REASON_WEAPON_LIMIT, kind 0}`. Our client then shows the 2009 weapon-limit box (CLI-1132). The
//! 2009 client shows a box only for kind 3 (reasons 10 / 11 / 0x8e) and ignores this one, as it ignored the bare
//! refusal before.
// EVIDENCE: cshell+0x15f0d0 (0x393 refused: a box only when +0xc is 3, by reason 10 / 0xb / 0x8e; else nothing)
// DC-DEFAULT: LG-342 (proposed, project decision) the reason value and its box

/// +0xa reason of a refusal by the room's weapon limit (not a 2009 reason).
pub const REASON_WEAPON_LIMIT: u16 = 0xdc42;
/// +0xc kind sent with it (not the 2009 join kind 3, so a 2009 client shows nothing).
pub const KIND_WEAPON_LIMIT: i16 = 0;
