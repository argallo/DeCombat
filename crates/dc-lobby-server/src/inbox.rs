//! The inbox: credit purchases, gifts and coupon products wait here until the owner opens them (0x32c..0x32f);
//! supply cases are drawn on opening; pending notices (0x303 +0x104 -> 0x366 / 0x367) and the stale push 0x3014.
// SPEC: PROTOCOL_2009.md §4.3

use crate::delivery::{self, Consume, Delivered};
use crate::shop::{self, account_of, reason};
use crate::shopmsg::*;
use crate::state::{Core, HResult, Sid};
use crate::{clock, msg};
use dc_wire::{PktReader, PktWriter};
use rusqlite::{params, OptionalExtension};
use std::sync::Arc;

/// One inbox row as the client lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InboxRow {
    pub id: u32,
    pub product: u32,
    pub gift_kind: u32,
    pub opened: u32,
    pub sender: String,
    pub received: u64,
    pub message: String,
    pub end: u64,
}

/// Adds the inbox rows of one product: one row, or one single-case row per case of a pack.
// DC-DEFAULT: CD-02 a pack of N cases becomes N inbox rows, each its own unique id
pub fn add_product_rows(
    st: &Core,
    account: u32,
    product: i32,
    gift_kind: i64,
    sender: &str,
    message: &str,
) -> rusqlite::Result<Vec<i64>> {
    let rom = Arc::clone(&st.rom);
    let n = rom
        .shop
        .catalog
        .as_ref()
        .and_then(|c| c.product(product).map(|p| shop::product_items(c, p)))
        .filter(|items| shop::is_case(items) && st.rules.shop.case_split_on_purchase)
        .map(|items| items[0].period.max(1))
        .unwrap_or(1);
    let mut ids = Vec::new();
    for _ in 0..n {
        ids.push(add_row(st, account, product, gift_kind, sender, message)?);
    }
    Ok(ids)
}

/// Adds exactly one unopened inbox row for `product` and returns its entry id.
pub fn add_row(
    st: &Core,
    account: u32,
    product: i32,
    gift_kind: i64,
    sender: &str,
    message: &str,
) -> rusqlite::Result<i64> {
    st.db.execute(
        "INSERT INTO inbox (user_no, product_id, created, gift_kind, sender, message, opened) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
        params![account as i64, product, clock::now_ticks(), gift_kind, sender, message],
    )?;
    Ok(st.db.last_insert_rowid())
}

/// Tells an online account that its inbox changed (the client marks it stale).
// SPEC: PROTOCOL_2009.md §4.3 0x3014 (EVIDENCE: cshell+0x33c20)
pub fn push_stale(st: &mut Core, account: u32) {
    let f = PktWriter::new(INBOX_STALE_PUSH).into_frame();
    for sid in st.sids_of(account) {
        st.send(sid, f.clone());
    }
}

/// Queues a notice the account reads with 0x366 at its next character-info refresh (0x303 +0x104).
pub fn queue_notice(st: &Core, account: u32, text: &str) -> rusqlite::Result<()> {
    queue_notice_on(&st.db, account, text, clock::unix_now())
}

/// [`queue_notice`] on a bare connection (the host's `account` commands).
pub fn queue_notice_on(
    c: &rusqlite::Connection,
    account: u32,
    text: &str,
    now: i64,
) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO notices (user_no, text, created) VALUES (?1, ?2, ?3)",
        params![account as i64, text, now],
    )?;
    c.execute(
        "UPDATE characters SET notice_pending = 1 WHERE user_no = ?1",
        params![account as i64],
    )?;
    Ok(())
}

/// The end date an inbox row shows: the receipt plus the period of the product's item when it has a real one (1..998
/// days; 0 and 999 mean permanent), else 0.
// EVIDENCE: PROTOCOL_2009.md 0x32d (+0xe0 FILETIME end); cshell+0x68df0 (the client draws the row's day mark from the
// product's period and the TEMPORARY mark / EXPIRES line from a real end)
// DC-DEFAULT: LG-60 the end of a period product's row is its receipt plus the period (the period itself starts at the
// opening, CD-53); the earlier server sent 0 for every row, so a 7-day item showed no date
fn end_of(st: &Core, product: u32, received: u64) -> u64 {
    let days = st
        .rom
        .shop
        .catalog
        .as_ref()
        .and_then(|c| c.product(product as i32).map(|p| shop::product_items(c, p)))
        .and_then(|items| items.into_iter().next())
        .filter(|i| matches!(i.consume, Consume::Period | Consume::PeriodOnUse))
        .map_or(0, |i| i.period);
    if (1..=998).contains(&days) {
        received.saturating_add((days * clock::TICKS_PER_DAY) as u64)
    } else {
        0
    }
}

fn rows(st: &Core, account: u32, limit: u32) -> rusqlite::Result<Vec<InboxRow>> {
    let mut q = st.db.prepare(
        "SELECT uproduct, product_id, gift_kind, opened, sender, created, message FROM inbox \
         WHERE user_no = ?1 AND opened = 0 ORDER BY uproduct DESC LIMIT ?2",
    )?;
    let mut list = q
        .query_map(params![account as i64, limit as i64], |r| {
            Ok(InboxRow {
                id: r.get::<_, i64>(0)? as u32,
                product: r.get::<_, i64>(1)? as u32,
                gift_kind: r.get::<_, i64>(2)?.clamp(0, u32::MAX as i64) as u32,
                opened: r.get::<_, i64>(3)? as u32,
                sender: r.get(4)?,
                received: r.get::<_, i64>(5)?.max(0) as u64,
                message: r.get(6)?,
                end: 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<InboxRow>>>()?;
    // LG-314: off a featured entry a storage row of a pack item stays hidden (and stays stored)
    if !crate::inventory::pack_items_allowed(st, account) {
        list.retain(|r| !shop::holds_pack_item(st, r.product as i32));
    }
    for r in &mut list {
        r.end = end_of(st, r.product, r.received);
    }
    Ok(list)
}

/// 0x32d: +6 ok; +0xe total; +0x12 count; +0x16 rows of 0xe8 (+0 id, +4 product, +8 gift kind, +0xc opened,
/// +0x10 wstr[17] sender, +0x32 FILETIME received, +0x3a wstr[83] message, +0xe0 FILETIME end).
// SPEC: PROTOCOL_2009.md §4.3 0x32d (LIVE; parser cshell+0x267a40 copies 0x3a words per row)
pub fn page_reply(total: u32, rows: &[InboxRow]) -> Vec<u8> {
    let mut w = PktWriter::new(INBOX_PAGE_REPLY);
    w.u32(1).at(0xe).u32(total).u32(rows.len() as u32);
    for r in rows {
        let base = w.len();
        w.u32(r.id)
            .u32(r.product)
            .u32(r.gift_kind)
            .u32(r.opened)
            .wstr(&r.sender, 17)
            .u64(r.received);
        w.wstr(&r.message, 83).u64(r.end);
        debug_assert_eq!(w.len() - base, 0xe8);
    }
    w.into_frame()
}

/// 0x32c: every unopened row, newest first, in one reply; `total` equals the rows sent, so the client never asks
/// for a further page (it fetches only while it holds fewer rows than the total: cshell+0x267070).
// DC-DEFAULT: CD-74 (proposed) one reply with up to inbox.max_rows rows; the page index is not used
pub fn on_page(st: &mut Core, sid: Sid, _p: &[u8]) -> HResult {
    let Some(account) = account_of(st, sid) else {
        st.send(sid, page_reply(0, &[]));
        return Ok(());
    };
    let list = rows(st, account, st.rules.shop.inbox_max_rows)?;
    st.send(sid, page_reply(list.len() as u32, &list));
    Ok(())
}

/// 0x32f for a product: +6 ok; +0xa reason; +0xe item count (must equal the product's item count, cshell+0x14e010);
/// +0x22 u32[5] unique ids; +0x36 FILETIME[5] ends.
// SPEC: PROTOCOL_2009.md §4.3 0x32f (EVIDENCE: cshell+0x33ca0 -> cshell+0x14e010)
pub fn open_reply(ok: bool, reason: u32, items: &[(u32, u64)]) -> Vec<u8> {
    let n = items.len().min(5);
    let mut w = PktWriter::new(INBOX_OPEN_REPLY);
    w.u32(ok as u32).u32(reason).u32(n as u32);
    w.at(0x22);
    for k in 0..5 {
        w.u32(items.get(k).filter(|_| k < n).map(|i| i.0).unwrap_or(0));
    }
    for k in 0..5 {
        w.u64(items.get(k).filter(|_| k < n).map(|i| i.1).unwrap_or(0));
    }
    w.into_frame()
}

/// 0x32f for a supply case (the client takes this branch when the row's product is a case, type 0x14):
/// +6 ok; +0xe 1; +0x12 award item number; +0x16 GP total after a GP award (0 = item award); +0x1a award days
/// (0 = permanent); +0x1e unique id of a counted case item to decrement (0 = none; a HiSec opening names the key
/// it used, which also selects the HiSec pop-up); +0x22 award unique id; +0x36 award FILETIME end.
// EVIDENCE: cshell+0x14e010 (case branch: +0x12 -> cshell+0x25c010, +0x16 != 0 -> GP pop-up cshell+0x287110,
// else cshell+0x287150 with +0x22 / +0x1a and cshell+0x22c790 adds +0x22 with end +0x36; +0x1e count decrement)
// SPEC: HISEC_2009.md §4.3 (+0x1e = the unique id of the HiSec key used)
pub fn case_reply(award: &Delivered, decrement_uid: u32) -> Vec<u8> {
    let mut w = PktWriter::new(INBOX_OPEN_REPLY);
    w.u32(1)
        .u32(0)
        .u32(1)
        .u32(award.item_no as u32)
        .u32(0)
        .u32(award.days.max(0) as u32)
        .u32(decrement_uid);
    w.u32(award.uid as u32)
        .at(0x36)
        .u64(award.end.max(0) as u64);
    w.at(0x5e);
    w.into_frame()
}

fn open_fail(st: &mut Core, sid: Sid, reason: u32) {
    st.send(sid, open_reply(false, reason, &[]));
}

/// 0x32e: opens one row; the row is consumed.
// DC-DEFAULT: CD-02 open = consume that row; CD-75 (proposed) case rows draw per CD-01 / CD-76, other rows
// deliver per CD-72 with the period starting at the opening (res 0x17b6: the usage period starts when the item is
// activated from the inbox), the product's bonus GP is added (the client adds it too, cshell+0x14e010)
pub fn on_open(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let mut r = PktReader::new(p);
    let entry = r.at(6).u32() as i64;
    let product = r.u32() as i32;
    let Some(account) = account_of(st, sid) else {
        open_fail(st, sid, reason::NOT_FOR_SALE);
        return Ok(());
    };
    let row: Option<i64> = st
        .db
        .query_row(
            "SELECT product_id FROM inbox WHERE uproduct = ?1 AND user_no = ?2 AND opened = 0",
            params![entry, account as i64],
            |r| r.get(0),
        )
        .optional()?;
    let Some(row_product) = row.filter(|&p| p as i32 == product) else {
        open_fail(st, sid, reason::NOT_FOR_SALE);
        return Ok(());
    };
    let rom = Arc::clone(&st.rom);
    let Some(cat) = rom.shop.catalog.as_ref() else {
        open_fail(st, sid, reason::SHOP_UNAVAILABLE);
        return Ok(());
    };
    let Some(prod) = cat.product(row_product as i32) else {
        open_fail(st, sid, reason::NOT_FOR_SALE);
        return Ok(());
    };
    let items = shop::product_items(cat, prod);
    // DC-DEFAULT: HS-7 (proposed) a HiSec case opens only with a matching key (hisec.rs)
    if let Some(value) = items
        .first()
        .and_then(|i| crate::hisec::case_value(st, i.item_no))
    {
        let case_item = items[0].item_no;
        return crate::hisec::open(st, sid, account, entry, case_item, value);
    }
    if shop::is_case(&items) {
        if !delivery::room_for(st, account, 1)? {
            open_fail(st, sid, reason::INVENTORY_FULL);
            return Ok(());
        }
        match delivery::open_case(st, account, items[0].item_no)? {
            Some(award) => {
                st.db
                    .execute("DELETE FROM inbox WHERE uproduct = ?1", [entry])?;
                st.send(sid, case_reply(&award, 0));
                crate::notices::case_award(st, account, &award)?;
                st.note(&format!(
                    "inbox: account {} opened case {} -> item {} ({} days)",
                    account, items[0].item_no, award.item_no, award.days
                ));
            }
            None => open_fail(st, sid, reason::SHOP_UNAVAILABLE),
        }
        return Ok(());
    }
    let new_rows = items
        .iter()
        .filter(|i| i.consume != Consume::GpGrant)
        .count() as i64;
    if !delivery::room_for(st, account, new_rows)? {
        open_fail(st, sid, reason::INVENTORY_FULL);
        return Ok(());
    }
    let mut out = Vec::new();
    for it in items.iter().take(5) {
        match delivery::deliver(st, account, it.item_no, it.consume, it.period, None)? {
            Some(d) => out.push((d.uid as u32, d.end.max(0) as u64)),
            None => out.push((0, 0)),
        }
    }
    if prod.bonus > 0 {
        st.db.execute(
            "UPDATE characters SET gp = gp + ?2 WHERE user_no = ?1",
            params![account as i64, prod.bonus as i64],
        )?;
    }
    st.db
        .execute("DELETE FROM inbox WHERE uproduct = ?1", [entry])?;
    st.send(sid, open_reply(true, 0, &out));
    st.note(&format!(
        "inbox: account {} opened product {} -> {:?}",
        account, row_product, out
    ));
    Ok(())
}

/// 0x367 (0x418): +6 1; +0x12 wstr[513] text; +0x414 u32 more.
// SPEC: PROTOCOL_2009.md §3.3 0x367 (LIVE)
pub fn notice_reply(text: &str, more: bool) -> Vec<u8> {
    let mut w = PktWriter::new(msg::NOTICE_REPLY);
    w.u32(1).at(0x12).wstr(text, 513).u32(more as u32);
    w.at(0x418);
    w.into_frame()
}

/// 0x366: the oldest pending notice; the flag stays set while more are queued.
// DC-DEFAULT: CD-73 (proposed) each queued notice is read once
pub fn on_notice(st: &mut Core, sid: Sid) -> HResult {
    let Some(account) = account_of(st, sid) else {
        st.send(sid, notice_reply("", false));
        return Ok(());
    };
    let next: Option<(i64, String)> = st
        .db
        .query_row(
            "SELECT id, text FROM notices WHERE user_no = ?1 ORDER BY id LIMIT 1",
            [account as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let text = match &next {
        Some((id, t)) => {
            st.db.execute("DELETE FROM notices WHERE id = ?1", [id])?;
            t.clone()
        }
        None => String::new(),
    };
    let left: i64 = st.db.query_row(
        "SELECT COUNT(*) FROM notices WHERE user_no = ?1",
        [account as i64],
        |r| r.get(0),
    )?;
    st.db.execute(
        "UPDATE characters SET notice_pending = ?2 WHERE user_no = ?1",
        params![account as i64, (left > 0) as i64],
    )?;
    st.send(sid, notice_reply(&text, left > 0));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::u32_at;

    fn split(f: &[u8]) -> Vec<u8> {
        let (frames, _) = dc_wire::parse_tcp_frames(f).unwrap();
        frames.into_iter().next().unwrap().1
    }

    fn ws(p: &[u8], o: usize, n: usize) -> String {
        PktReader::from_raw(p).at(o).wstr(n)
    }

    fn u64_at(p: &[u8], o: usize) -> u64 {
        u64::from_le_bytes(p[o..o + 8].try_into().unwrap())
    }

    #[test]
    fn page_layout_matches_the_spec() {
        let r = InboxRow {
            id: 7,
            product: 1475,
            gift_kind: 1,
            opened: 0,
            sender: "Alice".into(),
            received: 0x11,
            message: "hi bob".into(),
            end: 0x22,
        };
        let p = split(&page_reply(1, &[r.clone(), InboxRow { id: 8, ..r }]));
        assert_eq!(p.len(), 0x16 + 2 * 0xe8);
        assert_eq!(
            (u32_at(&p, 6), u32_at(&p, 0xe), u32_at(&p, 0x12)),
            (1, 1, 2)
        );
        let b = 0x16;
        assert_eq!(
            (
                u32_at(&p, b),
                u32_at(&p, b + 4),
                u32_at(&p, b + 8),
                u32_at(&p, b + 0xc)
            ),
            (7, 1475, 1, 0)
        );
        assert_eq!(ws(&p, b + 0x10, 17), "Alice");
        assert_eq!(
            &p[b + 0x10..b + 0x16],
            "Ali"
                .encode_utf16()
                .flat_map(|c| c.to_le_bytes())
                .collect::<Vec<_>>()
                .as_slice()
        );
        assert_eq!(u64_at(&p, b + 0x32), 0x11);
        assert_eq!(ws(&p, b + 0x3a, 83), "hi bob");
        assert_eq!(u64_at(&p, b + 0xe0), 0x22);
        assert_eq!(u32_at(&p, b + 0xe8), 8, "second row");
    }

    #[test]
    fn open_layouts_match_the_spec() {
        let p = split(&open_reply(true, 0, &[(11, 0x99), (12, 0)]));
        assert_eq!(p.len(), 0x5e);
        assert_eq!(
            (
                u32_at(&p, 6),
                u32_at(&p, 0xe),
                u32_at(&p, 0x22),
                u32_at(&p, 0x26)
            ),
            (1, 2, 11, 12)
        );
        assert_eq!((u64_at(&p, 0x36), u64_at(&p, 0x3e)), (0x99, 0));
        let p = split(&open_reply(false, reason::INVENTORY_FULL, &[]));
        assert_eq!(
            (u32_at(&p, 6), u32_at(&p, 0xa), u32_at(&p, 0xe)),
            (0, 0x3c, 0)
        );
        let a = Delivered {
            uid: 77,
            item_no: 54,
            end: 0x1234,
            extended: false,
            days: 3,
        };
        let p = split(&case_reply(&a, 0));
        assert_eq!(p.len(), 0x5e);
        assert_eq!(
            (
                u32_at(&p, 6),
                u32_at(&p, 0x12),
                u32_at(&p, 0x16),
                u32_at(&p, 0x1a)
            ),
            (1, 54, 0, 3)
        );
        assert_eq!(
            (u32_at(&p, 0x1e), u32_at(&p, 0x22), u64_at(&p, 0x36)),
            (0, 77, 0x1234)
        );
        let p = split(&case_reply(&a, 4321));
        assert_eq!(
            (p.len(), u32_at(&p, 0x1e), u32_at(&p, 0x22)),
            (0x5e, 4321, 77)
        );
    }

    #[test]
    fn notice_layout() {
        let p = split(&notice_reply("Dave sent you a gift", true));
        assert_eq!(p.len(), 0x418);
        assert_eq!(ws(&p, 0x12, 513), "Dave sent you a gift");
        assert_eq!(u32_at(&p, 0x414), 1);
    }
}
