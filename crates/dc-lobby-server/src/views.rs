//! Wire views of DB rows (character -> lobby user / room user / profile / character info).

use crate::db::{self, Character};
use crate::state::Core;
use crate::wire::{CharSheet, LobbyUser, Profile, SeatRow};

fn clamp_u32(v: i64) -> u32 {
    v.clamp(0, u32::MAX as i64) as u32
}

/// 0x303 +0x56 face value: the client sends 6000 + n and expects the same raw value back.
// SPEC: PROTOCOL_2009.md §2.2 "Face value" (EVIDENCE: cshell+0x3f2c0 subtracts 6000)
pub fn face_value(face_index: i64) -> i32 {
    if face_index < 0 {
        -1
    } else {
        6000 + face_index as i32
    }
}

/// Face index stored in the DB from the 0x202 / item-use face value (6004 -> 4).
pub fn face_index(face_value: i32) -> i64 {
    if face_value >= 6000 {
        (face_value - 6000) as i64
    } else {
        face_value.max(0) as i64
    }
}

impl Core {
    pub fn character(&self, account: u32) -> Option<Character> {
        db::character(&self.db, account).ok().flatten()
    }

    /// The rank number of an experience value by the 2009 client's own rule (the one rule behind every rank the
    /// lobby sends: room slot words, profile, and the game server's badge tokens).
    // EVIDENCE: cshell+0x267f10 (rank of exp), cshell+0x2683d0 (lobby / room rank icon = icon of this rank)
    // DC-DEFAULT: LG-6 (proposed) the rank words of the room slot, the profile and the progress popup use the
    // client's rule (past the last table row the client shows rank 0), not a second count of the bounds
    pub fn rank_of(&self, exp: i64) -> u32 {
        crate::game::client_rank(&self.rom.ranks.need, exp)
    }

    /// The clan's name colour (a clan colour item, kind 9, set it; 0 = none).
    // EVIDENCE: cshell+0x3f2c0 (0x303 +0xe4 clan colour), PROTOCOL_2009.md room user +0xb0
    // DC-DEFAULT: LG-198 (proposed) the stored clan colour goes out in 0x303 and the room / profile views
    pub fn clan_colour(&self, clan_uid: i64) -> u32 {
        if clan_uid == 0 {
            return 0;
        }
        self.db
            .query_row(
                "SELECT colour FROM clan_colours WHERE clan_uid = ?1",
                [clan_uid],
                |r| r.get::<_, i64>(0),
            )
            .map(clamp_u32)
            .unwrap_or(0)
    }

    /// True while the account has a room badge item (Elite Moderator, kind 10) in use.
    // EVIDENCE: cshell+0x3f2c0 (0x303 +0xe8 != 0 -> shell +0x424, the flag cshell+0x15dde0 tests for Kick User),
    // cshell+0x2452f0 (a kind 10 use sets the same flag)
    // DC-DEFAULT: LG-197 (proposed) 0x303 +0xe8 = 1 while an unexpired kind-10 item is in use
    pub fn room_badge(&self, account: u32) -> bool {
        let now = crate::clock::now_ticks();
        db::items(&self.db, account)
            .map(|v| {
                v.iter().any(|i| {
                    i.used != 0
                        && (i.end_time == 0 || i.end_time > now)
                        && crate::itemuse::subtype(self, i.item_no)
                            == Some(crate::itemuse::SUB_ROOM_BADGE)
                })
            })
            .unwrap_or(false)
    }

    pub fn lobby_user(&self, account: u32) -> LobbyUser {
        let c = self.character(account).unwrap_or_default();
        LobbyUser {
            account,
            // DC-DEFAULT: CD-22 in-channel id = account number
            chan_id: account,
            name: c.nick,
            exp: clamp_u32(c.exp),
            clan_id: clamp_u32(c.clan_uid),
            emblem: clamp_u32(c.emblem),
            background: clamp_u32(c.background),
            name_colour: clamp_u32(c.nick_color),
            nation: clamp_u32(c.nation),
        }
    }

    pub fn seat_row(&self, account: u32, team: u32, state: u32, sub: u32) -> SeatRow {
        let c = self.character(account).unwrap_or_default();
        SeatRow {
            char_no: account,
            chan_id: account,
            rank: self.rank_of(c.exp),
            class: 0,
            name: c.nick,
            clan_id: clamp_u32(c.clan_uid),
            team,
            state,
            sub_state: sub,
            exp: clamp_u32(c.exp),
            clan_title: c.clan_title,
            option_word: clamp_u32(c.options),
            emblem: clamp_u32(c.emblem),
            background: clamp_u32(c.background),
            name_colour: clamp_u32(c.nick_color),
            clan_colour: self.clan_colour(c.clan_uid),
            nation: clamp_u32(c.nation),
        }
    }

    pub fn profile(&self, c: &Character) -> Profile {
        Profile {
            account: c.user_no,
            chan_id: c.user_no,
            name: c.nick.clone(),
            exp: clamp_u32(c.exp),
            headshots: clamp_u32(c.headshots),
            kills: clamp_u32(c.kills),
            deaths: clamp_u32(c.deaths),
            wins: clamp_u32(c.wins),
            losses: clamp_u32(c.losses),
            rank: self.rank_of(c.exp),
            grade: clamp_u32(c.grade),
            clan_id: clamp_u32(c.clan_uid),
            clan_title: c.clan_title.clone(),
            emblem: clamp_u32(c.emblem),
            background: clamp_u32(c.background),
            name_colour: clamp_u32(c.nick_color),
            clan_colour: self.clan_colour(c.clan_uid),
            nation: clamp_u32(c.nation),
        }
    }

    pub fn char_sheet(&self, c: &Character) -> CharSheet {
        let unread: i64 = self
            .db
            .query_row(
                "SELECT COUNT(*) FROM inbox WHERE user_no = ?1 AND opened = 0",
                [c.user_no as i64],
                |r| r.get(0),
            )
            .unwrap_or(0);
        CharSheet {
            char_no: c.user_no,
            name: c.nick.clone(),
            face_value: face_value(c.face),
            gp: clamp_u32(c.gp),
            exp: clamp_u32(c.exp),
            headshots: clamp_u32(c.headshots),
            kills: clamp_u32(c.kills),
            deaths: clamp_u32(c.deaths),
            wins: clamp_u32(c.wins),
            losses: clamp_u32(c.losses),
            combined_rank: 0,
            clan_id: clamp_u32(c.clan_uid),
            clan_title: c.clan_title.clone(),
            clan_role: clamp_u32(c.clan_role),
            option_word: clamp_u32(c.options),
            emblem: clamp_u32(c.emblem),
            background: clamp_u32(c.background),
            inbox_unread: (unread > 0) as u32,
            name_colour: clamp_u32(c.nick_color),
            clan_colour: self.clan_colour(c.clan_uid),
            badge: self.room_badge(c.user_no) as u32,
            muted_until: c.mute_until.max(0) as u64,
            grade: clamp_u32(c.grade),
            nation: clamp_u32(c.nation),
            pending_notice: (c.notice_pending != 0) as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_round_trip() {
        assert_eq!(face_index(6004), 4);
        assert_eq!(face_index(6002), 2);
        assert_eq!(face_value(2), 6002);
        assert_eq!(face_value(-1), -1);
    }
}
