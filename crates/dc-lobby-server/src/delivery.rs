//! How a product's items reach an inventory: new rows or extensions by the ROM consume kind, supply case draws,
//! and the inventory capacity. Used by the shop (0x328), the inbox (0x32e) and the item windows.

use crate::clock::{self, TICKS_PER_DAY};
use crate::db::{self, DbResult, Item};
use crate::state::Core;
use rusqlite::params;

/// Consume kind of one product item (`Consumetype<k>` of the ROM shop list).
// DC-DEFAULT: CD-72 (proposed) delivery by consume kind: 0 period days from delivery (0 = permanent), 1 period
// that starts at the first use (CD-53), 2 supply case (CD-01 / CD-02), 3 counted uses, 4 GP grant (BonusGP, no
// item); any other kind is delivered like 0
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consume {
    Period,
    PeriodOnUse,
    Case,
    Counted,
    GpGrant,
    Other(i32),
}

impl Consume {
    pub fn from_code(c: i32) -> Self {
        match c {
            0 => Consume::Period,
            1 => Consume::PeriodOnUse,
            2 => Consume::Case,
            3 => Consume::Counted,
            4 => Consume::GpGrant,
            x => Consume::Other(x),
        }
    }

    /// `use_type` column written for a new row (the inventory record +0x20 use kind).
    fn use_type(self) -> i64 {
        match self {
            Consume::PeriodOnUse => 1,
            Consume::Counted => 3,
            _ => 0,
        }
    }
}

/// Item type code of a catalogue number, by the client's number ranges; weapons (< 1000) take their `ItemType`
/// from the ROM item table.
// EVIDENCE: cshell+0x25c010 (number -> type: 1000s option kinds, 2000 -> 0xe, 3000 -> 0xf, 4000 -> 0xb,
// 5000 -> 0xc, 6000 -> 0xd, 7000 -> 0x10 backpack, 8000 -> 0x14 case, 9000 -> 0x15 function, 11000 -> 0x17,
// 12000 -> 0x18, 13000 -> 0x19)
pub fn type_code(st: &Core, item_no: i64) -> Option<i32> {
    match item_no {
        0..=999 => st
            .rom
            .weapon_kinds
            .get(&crate::inventory::base_weapon(st, item_no))
            .copied(),
        1000..=1999 => {
            let k = st
                .rom
                .items
                .as_ref()
                .map(|t| t.option_kind(item_no as i32))
                .unwrap_or(-1);
            Some(if k >= 0 { k } else { 0x11 })
        }
        2000..=2999 => Some(0xe),
        3000..=3999 => Some(0xf),
        4000..=4999 => Some(0xb),
        5000..=5999 => Some(0xc),
        6000..=6999 => Some(0xd),
        7000..=7999 => Some(0x10),
        8000..=8999 => Some(0x14),
        9000..=9999 => Some(0x15),
        10000..=10999 => Some(0x1e),
        11000..=11999 => Some(0x17),
        12000..=12999 => Some(0x18),
        13000..=13999 => Some(0x19),
        _ => None,
    }
}

/// Equip slot of an item by its type (None: the client has no slot for it).
// EVIDENCE: cshell+0x259f00 (type -> slot; dc_bute::equip_slot_by_class_code)
pub fn own_slot(st: &Core, item_no: i64) -> Option<i64> {
    type_code(st, item_no)
        .and_then(dc_bute::equip_slot_by_class_code)
        .map(i64::from)
}

pub const CLASS_CASE: i32 = 0x14;
pub const CLASS_GP_GRANT: i32 = 0x19;

/// One delivered item (a new row or an extended one).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Delivered {
    pub uid: i64,
    pub item_no: i64,
    pub end: i64,
    pub extended: bool,
    /// Award duration in days for case draws (0 = permanent).
    pub days: i64,
}

pub fn insert_item(
    st: &Core,
    account: u32,
    item_no: i64,
    start: i64,
    end: i64,
    use_type: i64,
    remain: i64,
) -> DbResult<i64> {
    st.db.execute(
        "INSERT INTO items (user_no, item_no, start_time, end_time, equip_slot, default_type, use_type, remain) \
         VALUES (?1, ?2, ?3, ?4, -1, 0, ?5, ?6)",
        params![account as i64, item_no, start, end, use_type, remain],
    )?;
    Ok(st.db.last_insert_rowid())
}

/// End time after adding `days` to an item that ends at `end` (0 = permanent stays permanent; a past end counts
/// from now).
// DC-DEFAULT: CD-71 (proposed) extensions count from max(now, current end); permanent stays permanent
pub fn extended_end(end: i64, now: i64, days: i64) -> i64 {
    if end == 0 {
        return 0;
    }
    if days <= 0 || days == PERMANENT_DAYS {
        return 0;
    }
    end.max(now)
        .saturating_add(days.saturating_mul(TICKS_PER_DAY))
}

/// The catalogue period that means "permanent" besides 0: the 2009 client shows a product of 0 or 999 days as
/// permanent and refuses to sell a 999-day non-function product the user already owns (box 0x18b5).
// EVIDENCE: cshell+0x149f80 / cshell+0x157c00 (permanent mark for period 0 or 999), cshell+0x68df0 (inbox period
// tag: 0 or 999 = the permanent mark), cshell+0x1442f0 (period 999 and type != 0x15 already owned -> box 0x18b5)
// DC-DEFAULT: LG-196 (proposed) a 999-day product item is delivered (or activated) permanent, end 0
pub const PERMANENT_DAYS: i64 = 999;

/// End time of a new item that lasts `days` from `now` (0 or 999 days = permanent).
pub fn new_end(now: i64, days: i64) -> i64 {
    if days <= 0 || days == PERMANENT_DAYS {
        0
    } else {
        now.saturating_add(days.saturating_mul(TICKS_PER_DAY))
    }
}

/// An owned instance that a delivery may extend: same number, not a default item, not an attachment target.
pub fn extendable(st: &Core, account: u32, uid: i64, item_no: i64) -> DbResult<Option<Item>> {
    Ok(db::item(&st.db, uid)?
        .filter(|i| i.user_no == account && i.item_no == item_no && i.default_type == 0))
}

/// Delivers one product item: extends `extend_uid` when it names an owned timed instance (CD-71), else a new
/// row.
/// Returns None for kinds that leave no row (GP grants; cases are drawn by [`open_case`]).
pub fn deliver(
    st: &Core,
    account: u32,
    item_no: i64,
    consume: Consume,
    period: i64,
    extend_uid: Option<i64>,
) -> DbResult<Option<Delivered>> {
    let now = clock::now_ticks();
    if matches!(consume, Consume::GpGrant | Consume::Case) {
        return Ok(None);
    }
    if let Some(uid) = extend_uid.filter(|&u| u > 0 && st.rules.shop.extend) {
        if let Some(it) = extendable(st, account, uid, item_no)? {
            match consume {
                Consume::Counted => {
                    st.db.execute(
                        "UPDATE items SET remain = remain + ?2 WHERE uid = ?1",
                        params![uid, period.max(0)],
                    )?;
                    return Ok(Some(Delivered {
                        uid,
                        item_no,
                        end: it.end_time,
                        extended: true,
                        days: 0,
                    }));
                }
                Consume::PeriodOnUse if it.used == 0 => {
                    st.db.execute(
                        "UPDATE items SET remain = remain + ?2 WHERE uid = ?1",
                        params![uid, period.max(0)],
                    )?;
                    return Ok(Some(Delivered {
                        uid,
                        item_no,
                        end: it.end_time,
                        extended: true,
                        days: period,
                    }));
                }
                // a permanent instance stays permanent: the reply names the owned instance with end 0 (the client
                // shows "usage time extended") and no copy is made
                // DC-DEFAULT: CD-71 (amended, LG-45) the 2009 reply of an extension carries the owned
                // unique id; a permanent item has nothing to add to, so it is reported unchanged, never duplicated
                _ if it.end_time == 0 => {
                    return Ok(Some(Delivered {
                        uid,
                        item_no,
                        end: 0,
                        extended: true,
                        days: period,
                    }));
                }
                _ => {
                    let end = extended_end(it.end_time, now, period);
                    st.db.execute(
                        "UPDATE items SET end_time = ?2 WHERE uid = ?1",
                        params![uid, end],
                    )?;
                    return Ok(Some(Delivered {
                        uid,
                        item_no,
                        end,
                        extended: true,
                        days: period,
                    }));
                }
            }
        }
    }
    let (start, end, remain) = match consume {
        Consume::PeriodOnUse => (0, 0, period.max(0)),
        Consume::Counted => (now, 0, period.max(0)),
        _ => (now, new_end(now, period), 0),
    };
    let uid = insert_item(st, account, item_no, start, end, consume.use_type(), remain)?;
    Ok(Some(Delivered {
        uid,
        item_no,
        end,
        extended: false,
        days: period,
    }))
}

/// Weighted draw of the award duration.
// DC-DEFAULT: CD-01 supply case award duration (economy.case_odds; 0 days = permanent)
pub fn draw_days(odds: &[(u32, u32)], roll: u32) -> i64 {
    let total: u64 = odds.iter().map(|&(_, w)| w as u64).sum();
    if total == 0 {
        return 1;
    }
    let mut r = roll as u64 % total;
    for &(days, w) in odds {
        if r < w as u64 {
            return days as i64;
        }
        r -= w as u64;
    }
    odds.last().map(|&(d, _)| d as i64).unwrap_or(1)
}

/// Opens one supply case: draws the award item uniformly from the case's ROM pool and its duration per CD-01,
/// then grants it or extends an owned instance from its current remaining expiry (CD-02).
// DC-DEFAULT: CD-76 (proposed) award item = uniform pick from ITEMS.TXT Supply<n> Available_ItemID<k>
// DC-DEFAULT: CD-02 every opening grants or extends the awarded item from its current remaining expiry
pub fn open_case(st: &mut Core, account: u32, case_item_no: i64) -> DbResult<Option<Delivered>> {
    let odds = st.rules.shop.case_odds.clone();
    open_case_with(st, account, case_item_no, &odds)
}

/// [`open_case`] with an explicit duration table (HiSec cases draw from `economy.hisec_case_odds`, HS-8).
pub fn open_case_with(
    st: &mut Core,
    account: u32,
    case_item_no: i64,
    odds: &[(u32, u32)],
) -> DbResult<Option<Delivered>> {
    let pool = st
        .rom
        .items
        .as_ref()
        .map(|t| t.supply_pool(case_item_no as i32))
        .unwrap_or_default();
    if pool.is_empty() {
        return Ok(None);
    }
    let pick = st.keys.u32();
    let roll = st.keys.u32();
    let item_no = pool[pick as usize % pool.len()] as i64;
    let days = draw_days(odds, roll);
    let now = clock::now_ticks();
    let owned: Vec<Item> = db::items(&st.db, account)?
        .into_iter()
        .filter(|i| i.item_no == item_no && i.default_type == 0)
        .collect();
    // a permanent instance stays permanent; otherwise extend the timed instance that lasts longest
    if let Some(p) = owned.iter().find(|i| i.end_time == 0) {
        return Ok(Some(Delivered {
            uid: p.uid,
            item_no,
            end: 0,
            extended: true,
            days,
        }));
    }
    if let Some(t) = owned.iter().max_by_key(|i| i.end_time) {
        let end = if days == 0 {
            0
        } else {
            extended_end(t.end_time, now, days)
        };
        st.db.execute(
            "UPDATE items SET end_time = ?2 WHERE uid = ?1",
            params![t.uid, end],
        )?;
        return Ok(Some(Delivered {
            uid: t.uid,
            item_no,
            end,
            extended: true,
            days,
        }));
    }
    let end = new_end(now, days);
    let uid = insert_item(st, account, item_no, now, end, 0, 0)?;
    Ok(Some(Delivered {
        uid,
        item_no,
        end,
        extended: false,
        days,
    }))
}

/// Non-default items an account holds (the capacity counts these).
pub fn item_count(st: &Core, account: u32) -> DbResult<i64> {
    st.db.query_row(
        "SELECT COUNT(*) FROM items WHERE user_no = ?1 AND default_type = 0",
        [account as i64],
        |r| r.get(0),
    )
}

/// True when `n` more items fit the inventory.
// DC-DEFAULT: CD-69 (proposed) at most inventory.max_items non-default items (refusal reason 0x3c, res 0x503)
pub fn room_for(st: &Core, account: u32, n: i64) -> DbResult<bool> {
    Ok(item_count(st, account)? + n <= st.rules.shop.max_items as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_arithmetic() {
        let now = 1_000 * TICKS_PER_DAY;
        assert_eq!(new_end(now, 0), 0, "0 days = permanent");
        // LG-196: 999 days is the catalogue's other permanent period
        assert_eq!(new_end(now, PERMANENT_DAYS), 0);
        assert_eq!(extended_end(now + TICKS_PER_DAY, now, PERMANENT_DAYS), 0);
        assert_eq!(new_end(now, 7), now + 7 * TICKS_PER_DAY);
        assert_eq!(extended_end(0, now, 7), 0, "permanent stays permanent");
        assert_eq!(
            extended_end(now - 5, now, 1),
            now + TICKS_PER_DAY,
            "an expired end counts from now"
        );
        assert_eq!(
            extended_end(now + 3 * TICKS_PER_DAY, now, 2),
            now + 5 * TICKS_PER_DAY
        );
    }

    #[test]
    fn weighted_draw_covers_every_bucket_in_order() {
        let odds = [(1, 60), (3, 25), (7, 10), (30, 4), (0, 1)];
        assert_eq!(draw_days(&odds, 0), 1);
        assert_eq!(draw_days(&odds, 59), 1);
        assert_eq!(draw_days(&odds, 60), 3);
        assert_eq!(draw_days(&odds, 85), 7);
        assert_eq!(draw_days(&odds, 95), 30);
        assert_eq!(draw_days(&odds, 99), 0);
        assert_eq!(draw_days(&odds, 100), 1, "wraps by the weight total");
        let mut hist = std::collections::BTreeMap::new();
        for r in 0..10_000u32 {
            *hist.entry(draw_days(&odds, r)).or_insert(0) += 1;
        }
        assert_eq!(hist[&1], 6000);
        assert_eq!(hist[&0], 100);
    }

    #[test]
    fn type_codes_follow_the_client_ranges() {
        let st = crate::state::testing::state();
        assert_eq!(type_code(&st, 7001), Some(0x10));
        assert_eq!(type_code(&st, 8003), Some(CLASS_CASE));
        assert_eq!(type_code(&st, 9042), Some(0x15));
        assert_eq!(type_code(&st, 13000), Some(CLASS_GP_GRANT));
        assert_eq!(type_code(&st, 4001), Some(0xb));
        assert_eq!(own_slot(&st, 7001), Some(9));
        assert_eq!(own_slot(&st, 4001), Some(4));
        assert_eq!(own_slot(&st, 9042), None);
    }
}
