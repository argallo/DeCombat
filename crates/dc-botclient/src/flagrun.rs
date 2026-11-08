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

/// The server's touch of a flag (DSV-192): the body's centre within this flat distance and height of the flag point.
/// A bot's flag walk is done only inside it (a stop at the end of a way short of it takes nothing).
// DC-DEFAULT: BOT-138 (proposed) the touch of DSV-192, 64 units across, 96 up / down
pub const TOUCH_ACROSS: f32 = 64.0;
pub const TOUCH_HEIGHT: f32 = 96.0;
/// A loose (dropped) flag this near is run to at once by any bot of either team, whatever its job and even with an
/// enemy in sight; farther (up to [`LOOSE_ALL_RANGE`]), the team's nearest player runs to it, and once it has lain
/// [`LOOSE_ALL_MS`] everyone within that range does.
// DC-DEFAULT: BOT-138 (proposed) 600 units, 6 s, 2500 units (the server sends it home after 30 s, DSV-192)
pub const LOOSE_GRAB: f32 = 600.0;
pub const LOOSE_ALL_MS: u32 = 6_000;
pub const LOOSE_ALL_RANGE: f32 = 2_500.0;
/// BOT-144: an attacker this near the enemy flag runs on for it with an enemy in sight (it fights on the way).
// DC-DEFAULT: BOT-144 (proposed) 600 units (as the grab of a loose flag)
pub const FLAG_DASH: f32 = 600.0;
/// A flag walk whose distance to the flag gets no [`STALL_GAIN`] units shorter for [`STALL_MS`] ends: the flag is
/// out of the bot's reach from here (event `flag_unreachable`) and it is not walked to for [`UNREACH_MS`] unless it
/// moves [`UNREACH_MOVED`] units.
// DC-DEFAULT: BOT-138 (proposed) 32 units, 10 s, 30 s, 128 units
pub const STALL_GAIN: f32 = 32.0;
pub const STALL_MS: u32 = 10_000;
pub const UNREACH_MS: u32 = 30_000;
pub const UNREACH_MOVED: f32 = 128.0;
/// A carrier this near its base (the server's capture distance without a zone, DSV-192) waits there for its own
/// flag; its walk is not watched.
// DC-DEFAULT: BOT-138 (proposed) 128 units
pub const CARRIER_HOME: f32 = 128.0;

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

/// Whether a body centre at `pos` touches the flag point `flag` (the server's take / return rule).
pub fn touching(pos: [f32; 3], flag: [f32; 3]) -> bool {
    flat(pos, flag) <= TOUCH_ACROSS && (pos[1] - flag[1]).abs() <= TOUCH_HEIGHT
}

/// BOT-138: whether the bot at `own` runs to the loose flag at `flag` that has lain `loose_ms`: it is within
/// [`LOOSE_GRAB`], or within [`LOOSE_ALL_RANGE`] when no other living teammate (`mates`, carriers left out) is
/// nearer or the flag has lain [`LOOSE_ALL_MS`] (teammates out of the stream count as none: the range keeps a whole
/// team from crossing the map for it).
pub fn loose_rush(own: [f32; 3], flag: [f32; 3], mates: &[[f32; 3]], loose_ms: u32) -> bool {
    let d = flat(own, flag);
    d <= LOOSE_GRAB
        || (d <= LOOSE_ALL_RANGE
            && (loose_ms >= LOOSE_ALL_MS || mates.iter().all(|m| flat(*m, flag) >= d)))
}

/// BOT-138: the watch over one flag walk (the flag team walked to, its place, the best distance and when it was
/// last bettered by [`STALL_GAIN`]).
#[derive(Debug, Default, Clone, Copy)]
pub struct FlagWalk {
    walk: Option<(u32, [f32; 3], f32, u32)>,
}

impl FlagWalk {
    /// One tick of a walk of the body at `pos` to the flag of `team` at `flag` (`None`: no flag walk this tick, the
    /// watch ends); `left` is the length of the planned way when it is whole (a detour away from the flag is
    /// progress along it), else the flat distance counts. Returns true once the walk made no way for [`STALL_MS`]
    /// outside the touch (the watch starts again).
    pub fn tick(
        &mut self,
        now: u32,
        pos: [f32; 3],
        walk: Option<(u32, [f32; 3])>,
        left: Option<f32>,
    ) -> bool {
        let Some((team, flag)) = walk else {
            self.walk = None;
            return false;
        };
        let d = left.unwrap_or_else(|| flat(pos, flag));
        match self.walk {
            Some((t, at, best, since))
                if t == team && flat(at, flag) <= UNREACH_MOVED && !touching(pos, flag) =>
            {
                if d + STALL_GAIN <= best {
                    self.walk = Some((t, at, d, now));
                } else if now.wrapping_sub(since) >= STALL_MS {
                    self.walk = None;
                    return true;
                }
            }
            _ => self.walk = Some((team, flag, d, now)),
        }
        false
    }
}

/// BOT-138: whether the flag of `team` now at `flag` is out of reach by a mark `(team, place, at)` made at `now`.
pub fn unreachable(
    mark: Option<(u32, [f32; 3], u32)>,
    team: u32,
    flag: [f32; 3],
    now: u32,
) -> bool {
    mark.is_some_and(|(t, at, when)| {
        t == team && now.wrapping_sub(when) < UNREACH_MS && flat(at, flag) <= UNREACH_MOVED
    })
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
        assert_eq!(carrying_after(c, Some(3), 2, u32::MAX, 4), None, "dropped");
        // BOT-105: the capture names the bot as the carrier, and still ends the carry
        assert_eq!(carrying_after(c, Some(3), 2, 3, 1), None, "captured");
        assert_eq!(
            carrying_after(c, Some(3), 1, 5, 2),
            c,
            "another flag's change"
        );
        assert_eq!(
            carrying_after(c, Some(3), 1, 5, 1),
            c,
            "another flag's capture"
        );
    }

    /// BOT-138: a loose flag is run to by a bot within the grab distance, by the team's nearest player, and by
    /// everyone near once it has lain a while.
    #[test]
    fn a_loose_flag_is_run_to_by_the_nearest_and_by_everyone_near() {
        let flag = [0.0, 0.0, 0.0];
        assert!(
            loose_rush([500.0, 0.0, 0.0], flag, &[[100.0, 0.0, 0.0]], 0),
            "within the grab"
        );
        assert!(
            loose_rush([1500.0, 0.0, 0.0], flag, &[[2000.0, 0.0, 0.0]], 0),
            "the nearest"
        );
        assert!(loose_rush([1500.0, 0.0, 0.0], flag, &[], 0), "alone");
        assert!(
            !loose_rush([1500.0, 0.0, 0.0], flag, &[[900.0, 0.0, 0.0]], 0),
            "a nearer mate"
        );
        assert!(
            loose_rush([1500.0, 0.0, 0.0], flag, &[[900.0, 0.0, 0.0]], LOOSE_ALL_MS),
            "lain a while"
        );
        assert!(
            !loose_rush([3000.0, 0.0, 0.0], flag, &[[900.0, 0.0, 0.0]], LOOSE_ALL_MS),
            "too far"
        );
        assert!(
            !loose_rush([3000.0, 0.0, 0.0], flag, &[], 0),
            "alone, too far"
        );
    }

    /// BOT-138: the touch is the server's (64 across, 96 up / down); a flag walk that gets no nearer for 10 s
    /// outside it is given up, the flag is out of reach for 30 s unless it moves.
    #[test]
    fn a_flag_walk_that_makes_no_way_outside_the_touch_is_given_up() {
        let flag = [0.0, 100.0, 0.0];
        assert!(touching([60.0, 150.0, 0.0], flag));
        assert!(
            !touching([70.0, 150.0, 0.0], flag),
            "the stop 72 units off takes nothing"
        );
        assert!(!touching([0.0, 200.0, 0.0], flag), "a floor above");
        let mut w = FlagWalk::default();
        // walking nearer: never given up
        for k in 0..30u32 {
            assert!(!w.tick(
                k * 1000,
                [3000.0 - k as f32 * 90.0, 100.0, 0.0],
                Some((2, flag)),
                None
            ));
        }
        // a detour away from the flag along a whole way that gets shorter: never given up
        let mut w = FlagWalk::default();
        for k in 0..30u32 {
            let away = [200.0 + k as f32 * 50.0, 100.0, 0.0];
            assert!(!w.tick(
                k * 1000,
                away,
                Some((2, flag)),
                Some(4000.0 - k as f32 * 100.0)
            ));
        }
        // standing 120 units off (a plinth's foot): given up after 10 s
        let mut w = FlagWalk::default();
        let stand = [120.0, 100.0, 0.0];
        assert!(!w.tick(0, stand, Some((2, flag)), None));
        assert!(!w.tick(9_000, stand, Some((2, flag)), None));
        assert!(w.tick(10_000, stand, Some((2, flag)), None));
        // inside the touch: no give-up (the server takes it)
        let mut w = FlagWalk::default();
        assert!(!w.tick(0, [10.0, 100.0, 0.0], Some((2, flag)), None));
        assert!(!w.tick(20_000, [10.0, 100.0, 0.0], Some((2, flag)), None));
        // a fight in between ends the watch
        let mut w = FlagWalk::default();
        assert!(!w.tick(0, stand, Some((2, flag)), None));
        assert!(!w.tick(6_000, stand, None, None));
        assert!(!w.tick(12_000, stand, Some((2, flag)), None));
        let mark = Some((2, flag, 10_000));
        assert!(unreachable(mark, 2, flag, 20_000));
        assert!(!unreachable(mark, 1, flag, 20_000), "the other flag");
        assert!(
            !unreachable(mark, 2, [500.0, 100.0, 0.0], 20_000),
            "the flag moved"
        );
        assert!(!unreachable(mark, 2, flag, 40_000), "30 s later");
    }
}
