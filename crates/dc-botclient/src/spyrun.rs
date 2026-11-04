//! Spy Hunt for a hunting bot (mode class 9): where the intel cases lie (type-0x17 kind-3 objects of the object
//! stream), where the uplink is (the type-0x37 zone of kind 8 and class 9), how many cases the bot holds (server 0xcb
//! sub 0 with its own client id) of how many (sub 1), and the walk goal: the uplink while it holds every case, else
//! the nearest case. A DeCombat bot behaviour, not a 2009 rule.
// SPEC: GAME_MODES_2009.md §4.3 (zone record order, kind 8), §4.4 (kind 3), §5.6 (0xcb sub 0 `[value][team][pid]`,
// sub 1 `[total][dropped][spies]`); GAME_MESSAGES_2009_S2C.md §8.2 (type-0x17 blob head)
// DC-DEFAULT: BOT-75 (proposed)

use crate::objects::ObjectTable;
use dc_wire::udp::BitWriter;

/// `GS_BOUNTY_MODE`.
pub const GS_BOUNTY_MODE: u8 = 0xcb;
/// Shell type of a pickup blob, the kind of an intel case; the zone type and the use-target kind.
pub const PICKUP_TYPE: u8 = 0x17;
pub const PICKUP_KIND_INTEL: u32 = 3;
pub const ZONE_TYPE: u8 = 0x37;
pub const ZONE_KIND_USE_TARGET: u32 = 8;
pub const SPY_CLASS: u8 = 9;

/// Height differences count this many times in the choice of the nearest case.
// DC-DEFAULT: BOT-75 (proposed)
pub const HEIGHT_WEIGHT: f32 = 6.0;

/// What the bot knows of the mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpyRun {
    /// Cases the bot holds (its own sub 0 `value`, 0 once it is a hunter again).
    pub held: u32,
    /// Every case of the room (sub 1 `total`).
    pub total: u32,
}

/// Whether a blob is an intel case.
pub fn is_intel(blob: &BitWriter) -> bool {
    let mut r = blob.reader();
    if r.remaining_bits() < 8 + 3 + 8 + 32 || r.read_u8() != PICKUP_TYPE {
        return false;
    }
    r.read_bits(3);
    r.read_zstr8();
    r.remaining_bits() >= 32 && r.read_u32() == PICKUP_KIND_INTEL
}

/// Whether a blob is the uplink zone (kind 8, class 9).
pub fn is_uplink(blob: &BitWriter) -> bool {
    let mut r = blob.reader();
    if r.remaining_bits() < 8 + 64 + 1 + 72 + 32 + 96 || r.read_u8() != ZONE_TYPE {
        return false;
    }
    r.skip_bits(64 + 1 + 72 + 8 + 8);
    let class = r.read_u8();
    r.skip_bits(8 + 32);
    let kind = r.read_u32();
    kind == ZONE_KIND_USE_TARGET && class == SPY_CLASS
}

fn places(table: &ObjectTable, f: fn(&BitWriter) -> bool) -> Vec<[f32; 3]> {
    table
        .objects
        .values()
        .filter(|o| o.blob.as_ref().is_some_and(f))
        .map(|o| o.pos.unwrap_or(o.create_pos))
        .collect()
}

/// The 0xcb body (after the shell id): `Some((value, pid))` for sub 0, `Some((total, u32::MAX))` for sub 1.
pub fn read_bounty(body: &BitWriter) -> Option<(u8, u32, u32)> {
    let mut r = body.reader();
    if r.remaining_bits() < 8 {
        return None;
    }
    match r.read_u8() {
        0 if r.remaining_bits() >= 32 + 8 + 32 => {
            let v = r.read_u32();
            r.read_u8();
            Some((0, v, r.read_u32()))
        }
        1 if r.remaining_bits() >= 32 => Some((1, r.read_u32(), u32::MAX)),
        5 if r.remaining_bits() >= 8 => Some((5, u32::from(r.read_u8()), u32::MAX)),
        _ => None,
    }
}

impl SpyRun {
    /// One 0xcb body for the bot of client id `own`.
    pub fn on_bounty(&mut self, body: &BitWriter, own: Option<u32>) {
        match read_bounty(body) {
            Some((0, v, pid)) if Some(pid) == own => self.held = v,
            Some((1, total, _)) => self.total = total,
            // stage 2 ends: the agent's cases go back to the map
            Some((5, 0, _)) => self.held = 0,
            _ => {}
        }
    }

    /// The walk goal from `at`: the uplink with every case, else the nearest case.
    pub fn goal(&self, table: &ObjectTable, at: [f32; 3]) -> Option<[f32; 3]> {
        if self.total > 0 && self.held >= self.total {
            return places(table, is_uplink).into_iter().next();
        }
        // a case on another level costs more than one across the floor (the way-finder rarely finds the stairs)
        let d = |p: &[f32; 3]| {
            (p[0] - at[0]).powi(2)
                + (p[2] - at[2]).powi(2)
                + (HEIGHT_WEIGHT * (p[1] - at[1])).powi(2)
        };
        places(table, is_intel)
            .into_iter()
            .min_by(|a, b| d(a).total_cmp(&d(b)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::ClientObject;

    fn intel() -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(PICKUP_TYPE)
            .write_bit(false)
            .write_bit(false)
            .write_bit(true)
            .write_zstr8(b"")
            .write_u32(PICKUP_KIND_INTEL)
            .write_bit(false);
        w
    }

    fn uplink() -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(ZONE_TYPE)
            .write_u32(0)
            .write_u32(0)
            .write_bit(false);
        dc_wire::udp::Vec72::encode([10.0; 3]).write(&mut w);
        w.write_u8(0xff)
            .write_u8(2)
            .write_u8(SPY_CLASS)
            .write_u8(0)
            .write_u32(0)
            .write_u32(ZONE_KIND_USE_TARGET)
            .write_u32(15.0f32.to_bits())
            .write_u8(0)
            .write_u32(0)
            .write_zstr8(b"u")
            .write_zstr8(b"");
        w
    }

    /// BOT-75: the bot walks to the nearest intel case, and to the uplink once it holds every case.
    #[test]
    fn a_spy_bot_walks_to_the_nearest_intel_then_to_the_uplink() {
        let mut t = ObjectTable::default();
        for (id, b, x) in [
            (10u16, intel(), 900.0f32),
            (11, intel(), -200.0),
            (12, uplink(), 5000.0),
        ] {
            let mut o = ClientObject::default();
            o.blob = Some(b);
            o.pos = Some([x, 0.0, 0.0]);
            t.objects.insert(id, o);
        }
        let mut s = SpyRun::default();
        assert_eq!(s.goal(&t, [0.0; 3]), Some([-200.0, 0.0, 0.0]));
        let mut w = BitWriter::new();
        w.write_u8(1).write_u32(2).write_u32(0).write_u32(0);
        s.on_bounty(&w, Some(4));
        let mut w = BitWriter::new();
        w.write_u8(0).write_u32(2).write_u8(2).write_u32(4);
        s.on_bounty(&w, Some(4));
        assert_eq!(s.held, 2);
        assert_eq!(s.goal(&t, [0.0; 3]), Some([5000.0, 0.0, 0.0]));
        // another player's message changes nothing; stage 1 again drops the count
        let mut w = BitWriter::new();
        w.write_u8(0).write_u32(0).write_u8(1).write_u32(5);
        s.on_bounty(&w, Some(4));
        assert_eq!(s.held, 2);
        let mut w = BitWriter::new();
        w.write_u8(5).write_u8(0);
        s.on_bounty(&w, Some(4));
        assert_eq!(s.held, 0);
    }
}
