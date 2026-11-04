//! The flag modes for a hunting bot (mode classes 7 / 8): where the capture flags are (type-0x17 kind-2 objects of
//! the object stream), whether the bot carries one (server 0xc6), and the walk goal: the enemy flag while it carries
//! none, its own flag's place while it carries one. A DeCombat bot behaviour, not a 2009 rule.
// SPEC: GAME_MODES_2009.md §4.4 (kind 2 `[state][team][at_base]`), §5.5 (0xc6 field roles: `carrier` = the client
// id of the carrier); GAME_MESSAGES_2009_S2C.md §8.2 (type-0x17 blob head: type byte, three bits, name, kind)
// DC-DEFAULT: BOT-66 (proposed)

use crate::objects::ObjectTable;
use dc_wire::udp::BitWriter;

/// Shell type of a pickup blob and the kind of a capture flag.
pub const PICKUP_TYPE: u8 = 0x17;
pub const PICKUP_KIND_FLAG: u32 = 2;
/// `GS_OBJECTIVE_STATE`.
pub const GS_OBJECTIVE_STATE: u8 = 0xc6;

/// The team of a capture flag's create blob (`None` for any other blob).
pub fn flag_team(blob: &BitWriter) -> Option<u32> {
    let mut r = blob.reader();
    if r.remaining_bits() < 8 + 3 + 8 + 32 * 3 || r.read_u8() != PICKUP_TYPE {
        return None;
    }
    r.read_bits(3);
    r.read_zstr8();
    if r.remaining_bits() < 32 * 3 || r.read_u32() != PICKUP_KIND_FLAG {
        return None;
    }
    r.read_u32();
    Some(r.read_u32())
}

/// The place of the flag of `team` (its latest position on the stream).
pub fn flag_pos(table: &ObjectTable, team: u32) -> Option<[f32; 3]> {
    table
        .objects
        .values()
        .find(|o| o.blob.as_ref().and_then(flag_team) == Some(team))
        .and_then(|o| o.pos)
}

/// A 0xc6 body: (flag team, carrier, event).
pub fn read_objective_state(body: &BitWriter) -> Option<(u32, u32, u32)> {
    let mut r = body.reader();
    if r.remaining_bits() < 16 + 32 * 5 {
        return None;
    }
    r.read_u16();
    r.read_u32();
    let team = r.read_u32();
    let carrier = r.read_u32();
    r.read_u32();
    Some((team, carrier, r.read_u32()))
}

/// What the bot carries after a 0xc6 `event`: nothing of that flag after a capture (1), a return (3) or a drop (4)
/// (the capture names the carrier, so the carrier test alone kept the bot "carrying" after its own capture and it
/// stood circling at its base); the flag team when the bot is the carrier, nothing when that flag's carrier changed
/// to someone else, else unchanged.
// SPEC: GAME_MODES_2009.md §5.5 (event 1 captured, 2 taken, 3 returned, 4 dropped / reset)
// DC-DEFAULT: BOT-105 (proposed)
pub fn carrying_after(
    carrying: Option<u32>,
    own_pid: Option<u32>,
    team: u32,
    carrier: u32,
    event: u32,
) -> Option<u32> {
    match event {
        1 | 3 | 4 if carrying == Some(team) => None,
        1 | 3 | 4 => carrying,
        _ if own_pid == Some(carrier) => Some(team),
        _ if carrying == Some(team) => None,
        _ => carrying,
    }
}

/// The walk goal of a flag-mode bot of `own_team`.
pub fn goal(table: &ObjectTable, own_team: u8, carrying: Option<u32>) -> Option<[f32; 3]> {
    let own = u32::from(own_team);
    if carrying.is_some() {
        flag_pos(table, own)
    } else {
        flag_pos(table, if own == 1 { 2 } else { 1 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::ClientObject;

    fn blob(team: u32) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(PICKUP_TYPE)
            .write_bit(true)
            .write_bit(false)
            .write_bit(false)
            .write_zstr8(b"")
            .write_u32(PICKUP_KIND_FLAG)
            .write_u32(team)
            .write_u32(team)
            .write_bit(true);
        w
    }

    /// BOT-66: the bot walks to the enemy flag, and home with it once the server names it the carrier.
    #[test]
    fn a_flag_bot_walks_to_the_enemy_flag_then_home() {
        let mut t = ObjectTable::default();
        for (id, team, x) in [(10u16, 1u32, -500.0f32), (11, 2, 500.0)] {
            let mut o = ClientObject::default();
            o.blob = Some(blob(team));
            o.pos = Some([x, 0.0, 0.0]);
            t.objects.insert(id, o);
        }
        assert_eq!(flag_team(&blob(2)), Some(2));
        assert_eq!(goal(&t, 1, None), Some([500.0, 0.0, 0.0]));
        let c = carrying_after(None, Some(3), 2, 3, 2);
        assert_eq!(c, Some(2));
        assert_eq!(goal(&t, 1, c), Some([-500.0, 0.0, 0.0]));
        assert_eq!(
            carrying_after(c, Some(3), 2, u32::MAX, 4),
            None,
            "dropped"
        );
        // BOT-105: the capture names the bot as the carrier, and still ends the carry
        assert_eq!(carrying_after(c, Some(3), 2, 3, 1), None, "captured");
        assert_eq!(carrying_after(c, Some(3), 1, 5, 2), c, "another flag's change");
        assert_eq!(carrying_after(c, Some(3), 1, 5, 1), c, "another flag's capture");
    }
}
