//! Search and Destroy for a hunting bot (mode classes 2 / 3): the plant points (type-0x37 zones of kind 5, `z_sub`
//! 1) of the object stream, whether the bot carries the bomb (server 0x56 names its character), the walk goal (the
//! nearest plant point while it carries the bomb and nothing is planted) and the hold to plant there
//! (`UP_USE_HOLD` 0xd8 phase 1, held until the server's 0x51). A DeCombat bot behaviour, not a 2009 rule.
// SPEC: GAME_MODES_2009.md §4.3 (zone record order), §4.5 (hold to use), §5.2; GAME_MESSAGES_2009_C2S.md §7.1
// (`[phase:u8][target:objref16]`); GAME_MESSAGES_2009_S2C.md §2.2 rows 0x51 / 0x56
// DC-DEFAULT: BOT-70 (proposed)

use crate::objects::ObjectTable;
use dc_wire::udp::BitWriter;

/// `GS_BOMB_EXPLODED`, `GS_BOMB_DEFUSED`, `GS_BOMB_PLANTED`, `GS_OBJECTIVE_ITEM`.
pub const GS_BOMB_EXPLODED: u8 = 0x4f;
pub const GS_BOMB_DEFUSED: u8 = 0x50;
pub const GS_BOMB_PLANTED: u8 = 0x51;
pub const GS_OBJECTIVE_ITEM: u8 = 0x56;
/// `UP_USE_HOLD`.
pub const UP_USE_HOLD: u8 = 0xd8;
/// The bot starts its hold this close to a plant point.
// DC-DEFAULT: BOT-70 (proposed) 48 units (the sites are at least 96 units round their point, DSV-200)
pub const PLANT_REACH: f32 = 48.0;
/// A hold that brought no 0x51 is let go and tried again after this time.
// DC-DEFAULT: BOT-70 (proposed) 6 s (the plant takes 4 s, GMD-6)
pub const HOLD_RETRY_MS: u32 = 6000;

/// A defuse hold that brought no 0x50 is let go and tried again after this time.
// DC-DEFAULT: BOT-104 (proposed) 9 s (the defuse takes 7 s, GMD-6)
pub const DEFUSE_RETRY_MS: u32 = 9000;

/// A site the bot walks to without getting [`STALL_GAIN`] closer in [`STALL_MS`] counts as out of its reach for the
/// round (its way grid found no way there); the bot picks another site.
// DC-DEFAULT: BOT-103 (proposed) 10 s for 64 units
pub const STALL_MS: u32 = 10_000;
pub const STALL_GAIN: f32 = 64.0;

/// The inventory slot of the bomb item (DSV-240).
// DC-DEFAULT: BOT-80 (proposed) the bot draws the item of this slot before its plant hold
pub const BOMB_SLOT: u8 = 9;

/// The `z_sub` of a kind-5 zone blob (type byte first), `None` for any other blob.
pub fn use_point_sub(blob: &BitWriter) -> Option<u8> {
    let mut r = blob.reader();
    if r.remaining_bits() < 8 + 64 + 1 + 72 + 32 + 96 + 8 || r.read_u8() != 0x37 {
        return None;
    }
    r.skip_bits(64 + 1 + 72 + 32 + 32);
    let kind = r.read_u32();
    r.skip_bits(32);
    let sub = r.read_u8();
    (kind == 5).then_some(sub)
}

/// The plant points of the world: object id and place.
pub fn plant_points(table: &ObjectTable) -> Vec<(u16, [f32; 3])> {
    table
        .objects
        .iter()
        .filter(|(_, o)| o.blob.as_ref().and_then(use_point_sub) == Some(1))
        .map(|(id, o)| (*id, o.pos.unwrap_or(o.create_pos)))
        .collect()
}

/// The defuse point of the planted bomb (a kind-5 `z_sub` 2 zone the server streams at the bomb): object id and
/// place.
// SPEC: GAME_MODES_2009.md §4.3 (kind 5: `z_sub` 2 defuse point), §5.2 step 3 (the defuse zone at the bomb)
// DC-DEFAULT: BOT-104 (proposed)
pub fn defuse_point(table: &ObjectTable) -> Option<(u16, [f32; 3])> {
    table
        .objects
        .iter()
        .find(|(_, o)| o.blob.as_ref().and_then(use_point_sub) == Some(2))
        .map(|(id, o)| (*id, o.pos.unwrap_or(o.create_pos)))
}

/// BOT-103: the plant point of the team's site this round (the points in object id order, one site per round in
/// turn), with its object id.
pub fn team_point(points: &[(u16, [f32; 3])], round: u32) -> Option<(u16, [f32; 3])> {
    (!points.is_empty()).then(|| points[round as usize % points.len()])
}

/// BOT-103: the plant points without the ones the bot found out of reach this round (all of them when every one is).
pub fn usable(points: Vec<(u16, [f32; 3])>, blocked: &[[f32; 3]]) -> Vec<(u16, [f32; 3])> {
    let near = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2) < 64.0 * 64.0;
    let open: Vec<_> = points
        .iter()
        .copied()
        .filter(|p| !blocked.iter().any(|b| near(*b, p.1)))
        .collect();
    if open.is_empty() {
        points
    } else {
        open
    }
}

/// The nearest plant point to `at` (across the floor).
pub fn nearest(points: &[(u16, [f32; 3])], at: [f32; 3]) -> Option<(u16, [f32; 3])> {
    let d = |p: &[f32; 3]| (p[0] - at[0]).powi(2) + (p[2] - at[2]).powi(2);
    points
        .iter()
        .copied()
        .min_by(|a, b| d(&a.1).total_cmp(&d(&b.1)))
}

/// The 0x56 body's object (`None` = no object).
pub fn read_objective_item(body: &BitWriter) -> Option<u16> {
    let mut r = body.reader();
    if r.remaining_bits() < 16 {
        return None;
    }
    r.read_objref16()
}

/// The 0xd8 body.
pub fn use_hold_body(phase: u8, target: u16) -> BitWriter {
    let mut b = BitWriter::new();
    b.write_u8(phase).write_objref16(Some(target));
    b
}

/// The bot's bomb state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BombRun {
    /// The object 0x56 marked (the carrier's character).
    pub carrier: Option<u16>,
    pub planted: bool,
    /// The running hold: target and start.
    pub hold: Option<(u16, u32)>,
    /// The bomb item granted in [`BOMB_SLOT`]: weapon and ammunition (kept over the round reset; 0xb4 ends it).
    pub item: Option<(u16, u16)>,
    /// The bot has drawn the item (it sent the weapon change).
    pub drawn: bool,
    /// The number of the running round (0x48), for the team's site.
    pub round: u32,
    /// BOT-103: the sites found out of reach this round.
    pub blocked: Vec<[f32; 3]>,
    /// BOT-103: the site goal walked to, the best distance to it so far and when it was reached.
    progress: Option<([f32; 3], f32, u32)>,
}

impl BombRun {
    /// A new round: nothing planted, no hold, no mark.
    pub fn reset(&mut self) {
        *self = BombRun {
            item: self.item,
            round: self.round,
            ..BombRun::default()
        };
        // (the sites out of reach are forgotten: a new round starts from the spawn again)
    }

    /// The server's grant in a slot: the bomb slot's weapon is the item.
    pub fn on_grant(&mut self, slot: u8, weapon: u16, ammo_kind: u16) {
        if slot == BOMB_SLOT {
            self.item = Some((weapon, ammo_kind));
            self.drawn = false;
        }
    }

    /// The server's removal of a slot.
    pub fn on_remove(&mut self, slot: u8) {
        if slot == BOMB_SLOT {
            self.item = None;
            self.drawn = false;
        }
    }

    /// BOT-80: the weapon change that takes the bomb item out, once, before the plant hold (`None`: no item, or it is
    /// out already).
    pub fn draw_item(&mut self) -> Option<crate::weapon::WeaponChange> {
        let (weapon_id, aux) = self.item.filter(|_| !self.drawn)?;
        self.drawn = true;
        Some(crate::weapon::WeaponChange {
            slot: BOMB_SLOT,
            weapon_id,
            aux,
        })
    }

    /// BOT-103: one walk step toward a site goal (`None`: not walking to a site). Returns the goal when it just
    /// counted as out of reach (it is added to [`BombRun::blocked`]).
    pub fn walk(&mut self, now: u32, goal: Option<[f32; 3]>, pos: [f32; 3]) -> Option<[f32; 3]> {
        let Some(g) = goal else {
            self.progress = None;
            return None;
        };
        let d = ((g[0] - pos[0]).powi(2) + (g[2] - pos[2]).powi(2)).sqrt();
        match self.progress {
            Some((pg, best, at)) if (pg[0] - g[0]).abs() < 1.0 && (pg[2] - g[2]).abs() < 1.0 => {
                if d < best - STALL_GAIN {
                    self.progress = Some((g, d, now));
                } else if now.wrapping_sub(at) >= STALL_MS {
                    self.progress = None;
                    self.blocked.push(g);
                    return Some(g);
                }
            }
            _ => self.progress = Some((g, d, now)),
        }
        None
    }

    /// How long a hold runs before the bot lets go and tries again: a plant before the bomb is down, a defuse after.
    pub fn retry_ms(&self) -> u32 {
        if self.planted {
            DEFUSE_RETRY_MS
        } else {
            HOLD_RETRY_MS
        }
    }

    /// Whether a hold the bot started is still running at `now` (the bot stands still meanwhile).
    pub fn holding(&self, now: u32) -> bool {
        self.hold
            .is_some_and(|(_, at)| now.wrapping_sub(at) < self.retry_ms())
    }

    /// Whether the bot (character `own_obj`) carries the bomb and nothing is planted.
    pub fn carrying(&self, own_obj: Option<u16>) -> bool {
        !self.planted && own_obj.is_some() && self.carrier == own_obj
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(kind: u32, sub: u8) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(0x37).write_u32(0).write_u32(0).write_bit(false);
        for _ in 0..9 {
            w.write_u8(0);
        }
        w.write_u8(0xff).write_u8(0xfe).write_u8(2).write_u8(0);
        w.write_u32(1)
            .write_u32(kind)
            .write_u32(0)
            .write_u8(sub)
            .write_u32(0);
        w.write_zstr8(b"A").write_zstr8(b"");
        w
    }

    /// BOT-70: kind-5 zones name their point kind; the nearest point wins; the carrier mark is the bot's own
    /// character only while nothing is planted.
    #[test]
    fn plant_points_carrier_and_hold_body() {
        assert_eq!(use_point_sub(&zone(5, 1)), Some(1));
        assert_eq!(use_point_sub(&zone(4, 0)), None);
        let pts = [(1, [0.0, 0.0, 0.0]), (2, [1000.0, 0.0, 0.0])];
        assert_eq!(nearest(&pts, [900.0, 50.0, 0.0]).map(|p| p.0), Some(2));
        let mut b = BombRun {
            carrier: Some(7),
            ..BombRun::default()
        };
        assert!(b.carrying(Some(7)));
        assert!(!b.carrying(Some(8)));
        b.planted = true;
        assert!(!b.carrying(Some(7)));
        assert_eq!(use_hold_body(1, 2).as_bytes(), &[1, 2, 0]);
        let mut w = BitWriter::new();
        w.write_objref16(Some(7));
        assert_eq!(read_objective_item(&w), Some(7));
    }

    /// BOT-103 / BOT-104: the team's point turns with the round; the defuse point is the `z_sub` 2 zone; a hold
    /// runs 6 s before the plant and 9 s after it.
    #[test]
    fn team_point_defuse_point_and_hold_times() {
        use crate::objects::ClientObject;
        let pts = [(4, [0.0, 0.0, 0.0]), (6, [1000.0, 0.0, 0.0])];
        assert_eq!(team_point(&pts, 1).map(|p| p.0), Some(6));
        assert_eq!(team_point(&pts, 2).map(|p| p.0), Some(4));
        assert_eq!(team_point(&[], 2), None);
        let mut t = ObjectTable::default();
        for (id, sub, x) in [(4u16, 1u8, 0.0f32), (9, 2, 500.0)] {
            let mut o = ClientObject::default();
            o.blob = Some(zone(5, sub));
            o.pos = Some([x, 0.0, 0.0]);
            t.objects.insert(id, o);
        }
        assert_eq!(plant_points(&t).len(), 1);
        assert_eq!(defuse_point(&t), Some((9, [500.0, 0.0, 0.0])));
        let mut b = BombRun {
            hold: Some((9, 1000)),
            ..BombRun::default()
        };
        assert!(b.holding(1000 + HOLD_RETRY_MS - 1));
        assert!(!b.holding(1000 + HOLD_RETRY_MS));
        b.planted = true;
        assert!(b.holding(1000 + DEFUSE_RETRY_MS - 1));
        b.round = 3;
        b.reset();
        assert_eq!((b.round, b.hold, b.planted), (3, None, false));
    }

    /// BOT-80: the bomb slot's grant is the item; the bot takes it out once; the round reset keeps it, the removal
    /// ends it.
    #[test]
    fn the_bomb_item_is_taken_out_once() {
        let mut b = BombRun::default();
        b.on_grant(0, 5, 5);
        assert_eq!(b.draw_item(), None);
        b.on_grant(BOMB_SLOT, 16, 40);
        let c = b.draw_item().expect("draw");
        assert_eq!((c.slot, c.weapon_id, c.aux), (BOMB_SLOT, 16, 40));
        assert_eq!(b.draw_item(), None);
        b.reset();
        assert_eq!(b.item, Some((16, 40)));
        b.on_remove(BOMB_SLOT);
        assert_eq!(b.draw_item(), None);
    }
}
