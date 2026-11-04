//! Room settings the host bot changes before the start: the kill goal (`room_setting_change` 0x38c, field type
//! 8), the smallest one the client offers for the room's mode read from the user's ROM at run time.
//!
//! A settings change clears the ready state of every member (observed in tdm1, DC row RM-1), so the host sets
//! the goal right after creating the room, before guests ready up.
// SPEC: PROTOCOL_2009.md §3.7 (0x38c `+6 i32 mode, +0xa u32 field type, +0xe u32 value`, 0x16 bytes; 0x38d
// `+6 u32 ok`; 0x3035 field type 8 = goal)
// OBSERVED: capture 20261025_000014_tdm1 (the real client's 0x38c frames are 0x16 bytes, the
// last four zero; the lobby echoes the change as 0x3035 and resets the goal to 100 on a mode change)

use std::path::Path;
use std::time::Duration;

use dc_wire::tcp::PktWriter;
use serde_json::json;

use crate::lobby::Lobby;
use crate::record::Recorder;
use crate::{bot_err, BotResult};

/// `room_setting_change` and its reply.
pub const ROOM_SETTING_CHANGE: u16 = 0x38c;
pub const ROOM_SETTING_CHANGE_REPLY: u16 = 0x38d;
/// Field type of the kill goal (as in 0x3035).
pub const SETTING_GOAL: u32 = 8;
/// The mode row key with the smallest goal the create-room screen offers.
// OBSERVED: ROM table key GAMEROOM.TXT [Mode<n>] GoalMin (with GoalMax, GoalInc, DefaultGoal)
pub const KEY_GOAL_MIN: &str = "GoalMin";

/// Which goal the host sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalChoice {
    /// `GoalMin` of the room mode's ROM row.
    Min,
    Value(u32),
}

/// The mode row key with the mode class (the client's `mode_class`, GAME_MESSAGES_2009.md 1.2).
// OBSERVED: ROM table key GAMEROOM.TXT [Mode<n>] GameType
pub const KEY_GAME_TYPE: &str = "GameType";

/// `GameType` of mode row `mode` in the user's ROM (the mode class the 2009 client uses for the room).
pub fn rom_mode_class(rom: &Path, mode: usize) -> Option<u8> {
    let g = dc_bute::GameRoomData::from_rom(rom).ok()?;
    let row = g.modes(false).into_iter().nth(mode)?;
    row.get(KEY_GAME_TYPE)?.trim().parse::<u8>().ok()
}

/// `GoalMin` of mode row `mode` in the user's ROM.
pub fn rom_goal_min(rom: &Path, mode: usize) -> Option<u32> {
    let g = dc_bute::GameRoomData::from_rom(rom).ok()?;
    let row = g.modes(false).into_iter().nth(mode)?;
    row.get(KEY_GOAL_MIN)?.trim().parse::<u32>().ok()
}

/// Sends one 0x38c and waits for 0x38d.
pub fn set_setting(
    lobby: &mut Lobby,
    mode: i32,
    kind: u32,
    value: u32,
    timeout: Duration,
) -> BotResult<()> {
    let mut w = PktWriter::new(ROOM_SETTING_CHANGE);
    w.i32(mode).u32(kind).u32(value).u32(0);
    lobby.link.send(&w)?;
    let d = lobby.link.recv_until(ROOM_SETTING_CHANGE_REPLY, timeout)?;
    let ok = d
        .get(6..10)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .unwrap_or(0);
    if ok == 0 {
        return Err(bot_err!("lobby: setting {kind} = {value} refused"));
    }
    Ok(())
}

/// Sets the room goal for `choice` (logs `room_goal`); a goal the ROM cannot give is skipped with a log line.
pub fn set_goal(
    lobby: &mut Lobby,
    mode: i32,
    choice: GoalChoice,
    rom: Option<&Path>,
    timeout: Duration,
    rec: &Recorder,
) -> BotResult<Option<u32>> {
    let value = match choice {
        GoalChoice::Value(v) => Some(v),
        GoalChoice::Min => rom.and_then(|r| rom_goal_min(r, mode.max(0) as usize)),
    };
    let Some(v) = value else {
        rec.event(
            "room_goal",
            json!({"skipped": "no ROM mode row to read the smallest goal from", "mode": mode}),
        );
        return Ok(None);
    };
    set_setting(lobby, mode, SETTING_GOAL, v, timeout)?;
    rec.event(
        "room_goal",
        json!({"goal": v, "mode": mode, "from": if choice == GoalChoice::Min { "rom GoalMin" } else { "cli" }}),
    );
    Ok(Some(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_change_frame_is_0x16_bytes() {
        // OBSERVED: tdm1 `8c03 1600 0000 | 08000000 | 04000000 | 00000000 | 00000000`
        let mut w = PktWriter::new(ROOM_SETTING_CHANGE);
        w.i32(8).u32(4).u32(0).u32(0);
        let f = w.to_frame();
        assert_eq!(f.len() - 8, 0x16);
        assert_eq!(&f[8..10], &[0x8c, 0x03]);
        assert_eq!(&f[8 + 6..8 + 0x12], &[8, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn rom_goal_min_reads_the_mode_row() {
        let Some(rom) = dc_rez::dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let g = rom_goal_min(&rom, 0).expect("mode 0 GoalMin");
        assert!(g > 0);
    }
}
