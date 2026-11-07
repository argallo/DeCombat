//! In-process flows of the shop, inbox, coupon, megaphone, item-window and loadout paths over a synthetic ROM
//! (no ROM data: every table below is a made-up fixture with the ROM's key shapes).

use crate::db;
use crate::logs::Listener;
use crate::net::route;
use crate::shopmsg::*;
use crate::state::testing::*;
use crate::state::{Core, Outgoing, Sid};
use dc_bute::{ButeDoc, ItemsTable, ShopCatalog};
use dc_wire::PktWriter;
use rusqlite::params;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

struct Client {
    sid: Sid,
    rx: Receiver<Outgoing>,
    got: Vec<(u16, Vec<u8>)>,
}

impl Client {
    fn send(&mut self, st: &mut Core, id: u16, body: &[u8]) {
        self.send_framed(st, id as i32, id, body);
    }

    /// Sends with an explicit frame id (0x368 / 0x36a travel in frame 0x324).
    fn send_framed(&mut self, st: &mut Core, frame: i32, id: u16, body: &[u8]) {
        let mut w = PktWriter::new(id);
        w.bytes(body);
        let p = w.into_payload();
        st.scope(|st| route(st, self.sid, frame, &p)).unwrap();
        self.got.extend(drain(&self.rx));
    }

    fn take(&mut self, id: u16) -> Vec<u8> {
        self.got.extend(drain(&self.rx));
        let i = self
            .got
            .iter()
            .position(|(f, _)| *f == id)
            .unwrap_or_else(|| {
                panic!(
                    "no 0x{:x}; have {:?}",
                    id,
                    self.got
                        .iter()
                        .map(|(f, _)| format!("0x{:x}", f))
                        .collect::<Vec<_>>()
                )
            });
        let d = self.got[i].1.clone();
        self.got.drain(..=i);
        d
    }

    fn has(&mut self, id: u16) -> bool {
        self.got.extend(drain(&self.rx));
        self.got.iter().any(|(f, _)| *f == id)
    }
}

fn wstr(s: &str, n: usize) -> Vec<u8> {
    let mut w = PktWriter::empty();
    w.wstr(s, n);
    w.raw_bytes().to_vec()
}

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn u64_at(p: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(p[o..o + 8].try_into().unwrap())
}

/// Synthetic shop list: item infos and products in the ROM's key shapes.
const SHOP: &str = "
[ItemInfo0]
Itemindex=0
Itemid=110
Itemtype=0
NewItem=1
[ItemInfo1]
Itemindex=1
Itemid=9104
Itemtype=21
[ItemInfo2]
Itemindex=2
Itemid=8000
Itemtype=20
[ItemInfo3]
Itemindex=3
Itemid=9122
Itemtype=21
[ItemInfo4]
Itemindex=4
Itemid=13000
Itemtype=25
[ItemInfo5]
Itemindex=5
Itemid=7002
Itemtype=16
[ItemInfo6]
Itemindex=6
Itemid=9101
Itemtype=21
[Product0]
ProductID=1
SaleType=0
Shopview=1
Price=100
SalePrice=100
Itemnum=1
Itemindex0=0
Consumetype0=0
Period0=7
[Product1]
ProductID=2
SaleType=1
Shopview=1
Price=50
SalePrice=50
Itemnum=1
Itemindex0=1
Consumetype0=1
Period0=0
[Product2]
ProductID=3
SaleType=1
Shopview=1
Price=70
SalePrice=70
Itemnum=1
Itemindex0=2
Consumetype0=2
Period0=3
[Product3]
ProductID=4
SaleType=0
Shopview=1
Price=40
SalePrice=40
Itemnum=1
Itemindex0=2
Consumetype0=2
Period0=1
[Product4]
ProductID=5
SaleType=1
Shopview=1
Price=30
SalePrice=30
Itemnum=1
Itemindex0=3
Consumetype0=3
Period0=10
[Product5]
ProductID=6
SaleType=0
Shopview=0
Price=1
SalePrice=1
BonusGP=500
Itemnum=1
Itemindex0=4
Consumetype0=4
Period0=0
[Product6]
ProductID=7
SaleType=0
Shopview=1
Price=10
SalePrice=10
Itemnum=1
Itemindex0=5
Consumetype0=0
Period0=0
[Product7]
ProductID=8
SaleType=1
Shopview=1
Price=20
SalePrice=20
Itemnum=1
Itemindex0=6
Consumetype0=1
Period0=0
[Product8]
ProductID=9
SaleType=0
Shopview=1
Price=10
SalePrice=10
Itemnum=1
Itemindex0=5
Consumetype0=0
Period0=7
";

/// Synthetic item table: function items by kind, two backpacks, one case pool, one option.
const ITEMS: &str = "
[UseItem0]
ItemID=104
SubType=4
[UseItem1]
ItemID=105
SubType=5
[UseItem2]
ItemID=108
SubType=8
[UseItem3]
ItemID=112
SubType=12
ChangeItemID=111
[UseItem4]
ItemID=122
SubType=22
[UseItem5]
ItemID=113
SubType=13
ExpPercent=0.25
IsCanUnEquip=1
[UseItem9]
ItemID=114
SubType=14
GpPercent=0.5
IsCanUnEquip=1
[UseItem10]
ItemID=102
SubType=2
[UseItem6]
ItemID=101
SubType=1
[UseItem7]
ItemID=110
SubType=10
[UseItem8]
ItemID=109
SubType=9
[UseItem11]
ItemID=117
SubType=17
[UseItem20]
ItemID=115
SubType=11
IsCanUnEquip=1
[UseItem21]
ItemID=116
SubType=11
IsCanUnEquip=1
[UseItem22]
ItemID=106
SubType=6
[UseItem23]
ItemID=107
SubType=7
[UseItem24]
ItemID=126
SubType=16
FreePassItemID0=110
FreePassItemID1=111
[BackpackItem0]
ItemID=1
MultyInvenSlot=1
[BackpackItem1]
ItemID=2
MultyInvenSlot=2
[Supply0]
Available_ItemID0=110
[Option0]
ItemID=1
ItemType=17
";

fn rom_state() -> Core {
    let mut st = state();
    let rom = Arc::get_mut(&mut st.rom).unwrap();
    rom.shop.catalog = Some(ShopCatalog::from_doc(&ButeDoc::parse(SHOP)));
    rom.shop.name_colours = 16;
    rom.items = Some(ItemsTable::from_doc(ButeDoc::parse(ITEMS)));
    for (no, kind) in [
        (101, 1),
        (102, 4),
        (103, 7),
        (104, 6),
        (110, 0),
        (111, 0),
        (112, 2),
    ] {
        rom.weapon_kinds.insert(no, kind);
    }
    st
}

fn player(st: &mut Core, sid: Sid, login: &str, nick: &str) -> (Client, u32) {
    let a = db::create_account(&st.db, login, 0).unwrap();
    db::create_character(
        &st.db,
        &db::NewCharacter {
            user_no: a,
            nick,
            face: 2,
            exp: 0,
            gp: 10000,
            nx: 5000,
            now: 0,
        },
        true,
    )
    .unwrap();
    crate::inventory::grant_starter(st, a).unwrap();
    let rx = connect(st, sid, Listener::Lobby);
    let s = st.sessions.get_mut(&sid).unwrap();
    s.account = Some(a);
    s.in_lobby = true;
    (
        Client {
            sid,
            rx,
            got: Vec::new(),
        },
        a,
    )
}

fn give(st: &Core, a: u32, item_no: i64, use_type: i64, remain: i64) -> i64 {
    st.db
        .execute(
            "INSERT INTO items (user_no, item_no, use_type, remain) VALUES (?1, ?2, ?3, ?4)",
            params![a as i64, item_no, use_type, remain],
        )
        .unwrap();
    st.db.last_insert_rowid()
}

fn col(st: &Core, a: u32, c: &str) -> i64 {
    st.db
        .query_row(
            &format!("SELECT {c} FROM characters WHERE user_no = ?1"),
            [a as i64],
            |r| r.get(0),
        )
        .unwrap()
}

fn buy(c: &mut Client, st: &mut Core, pid: u32, option: u32, extend: u32) -> Vec<u8> {
    c.send(st, SHOP_BUY, &u32s(&[pid, option, extend, 0, 0, 0, 0]));
    c.take(SHOP_BUY_REPLY)
}

fn slots(c: &mut Client, st: &mut Core) -> Vec<i32> {
    c.send(st, crate::msg::EQUIPMENT, &[]);
    let d = c.take(crate::msg::EQUIPMENT_REPLY);
    (0..15).map(|i| u32_at(&d, 0x12 + 4 * i) as i32).collect()
}

/// CD-71 / LG-45: Extend Use (0x328 with an owned unique id) on a dated item moves its end by the period (a past end
/// counts from now) and replies with the same unique id; on a permanent item it makes no copy and replies with the
/// same id and end 0. The reply (+0xe a uid the client already holds) shows "usage time extended".
#[test]
fn extending_permanent_and_dated_items() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let count = |st: &Core| -> i64 {
        st.db
            .query_row(
                "SELECT COUNT(*) FROM items WHERE user_no = ?1",
                [a as i64],
                |r| r.get(0),
            )
            .unwrap()
    };
    let d = buy(&mut c, &mut st, 1, 0, 0);
    let uid = u32_at(&d, 0xe);
    let day = crate::clock::TICKS_PER_DAY as u64;
    // dated, in the future
    let end = u64_at(&d, 0x12);
    let rows = count(&st);
    let d = buy(&mut c, &mut st, 1, 0, uid);
    assert_eq!((u32_at(&d, 0xe), u64_at(&d, 0x12)), (uid, end + 7 * day));
    assert_eq!(count(&st), rows);
    // dated, already past: counts from now
    st.db
        .execute("UPDATE items SET end_time = 1 WHERE uid = ?1", [uid as i64])
        .unwrap();
    let d = buy(&mut c, &mut st, 1, 0, uid);
    let e = u64_at(&d, 0x12);
    assert_eq!(u32_at(&d, 0xe), uid);
    assert!(e > 6 * day && e < end + 8 * day, "{e}");
    assert_eq!(count(&st), rows);
    // permanent: the same unique id, end 0, no new row
    st.db
        .execute("UPDATE items SET end_time = 0 WHERE uid = ?1", [uid as i64])
        .unwrap();
    let d = buy(&mut c, &mut st, 1, 0, uid);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0xe), u64_at(&d, 0x12)),
        (1, uid, 0)
    );
    assert_eq!(count(&st), rows);
    assert_eq!(db::item(&st.db, uid as i64).unwrap().unwrap().end_time, 0);
}

#[test]
fn gp_purchase_extension_capacity_and_refusals() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let d = buy(&mut c, &mut st, 1, 0, 0);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0x1a), u32_at(&d, 0x1e)),
        (1, 9900, u32::MAX)
    );
    let uid = u32_at(&d, 0xe);
    let end = u64_at(&d, 0x12);
    assert_eq!(db::item(&st.db, uid as i64).unwrap().unwrap().item_no, 110);
    // CD-71: the owned instance is extended by the period
    let d = buy(&mut c, &mut st, 1, 0, uid);
    assert_eq!(u32_at(&d, 0xe), uid);
    assert_eq!(
        u64_at(&d, 0x12) - end,
        7 * crate::clock::TICKS_PER_DAY as u64
    );
    // a permanent instance is reported unchanged, not copied (CD-71, see extending_permanent_and_dated_items)
    st.db
        .execute("UPDATE items SET end_time = 0 WHERE uid = ?1", [uid as i64])
        .unwrap();
    let d = buy(&mut c, &mut st, 1, 0, uid);
    assert!(u32_at(&d, 0xe) == uid && u64_at(&d, 0x12) == 0);
    assert_eq!(db::item(&st.db, uid as i64).unwrap().unwrap().end_time, 0);
    // not shown -> 0x60; unknown -> 0x60; not enough GP -> 0x3b
    assert_eq!(u32_at(&buy(&mut c, &mut st, 6, 0, 0), 0xa) & 0xffff, 0x60);
    assert_eq!(u32_at(&buy(&mut c, &mut st, 99, 0, 0), 0xa) & 0xffff, 0x60);
    st.db
        .execute(
            "UPDATE characters SET gp = 5 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    let d = buy(&mut c, &mut st, 1, 0, 0);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0xa) & 0xffff, u32_at(&d, 0x1a)),
        (0, 0x3b, 5)
    );
    // CD-69: inventory full -> 0x3c
    st.db
        .execute(
            "UPDATE characters SET gp = 100000 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    st.rules.shop.max_items = 1;
    let d = buy(&mut c, &mut st, 1, 0, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa) & 0xffff), (0, 0x3c));
    // CD-70: an operator price override is what the purchase costs
    st.rules.shop.max_items = 100;
    st.rules.shop.price_override = vec![(1, 7)];
    let d = buy(&mut c, &mut st, 1, 0, 0);
    assert_eq!(u32_at(&d, 0x1a), 100000 - 7);
}

#[test]
fn credit_purchase_goes_to_the_inbox_and_case_packs_split() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    // option 1 = prepaid (nx), option 0 = credit (nx_cr, empty here -> 0x3b)
    let d = buy(&mut c, &mut st, 2, 0, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa) & 0xffff), (0, 0x3b));
    let d = buy(&mut c, &mut st, 2, 1, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xe)), (1, 0));
    assert_eq!(col(&st, a, "nx"), 4950);
    // CD-02: a pack of three cases becomes three rows
    buy(&mut c, &mut st, 3, 1, 0);
    c.send(&mut st, INBOX_PAGE, &u32s(&[0]));
    let d = c.take(INBOX_PAGE_REPLY);
    assert_eq!((u32_at(&d, 0xe), u32_at(&d, 0x12)), (4, 4));
    let products: Vec<u32> = (0..4).map(|i| u32_at(&d, 0x16 + i * 0xe8 + 4)).collect();
    assert_eq!(products, vec![3, 3, 3, 2], "newest first");
    // opening a case row draws from the pool and consumes exactly that row
    let case_row = u32_at(&d, 0x16);
    c.send(&mut st, INBOX_OPEN, &u32s(&[case_row, 3]));
    let o = c.take(INBOX_OPEN_REPLY);
    assert_eq!(
        (u32_at(&o, 6), u32_at(&o, 0x12), u32_at(&o, 0x16)),
        (1, 110, 0)
    );
    let award = u32_at(&o, 0x22);
    assert_eq!(
        db::item(&st.db, award as i64).unwrap().unwrap().item_no,
        110
    );
    // the second case extends the same item (CD-02)
    let next = u32_at(&d, 0x16 + 0xe8);
    c.send(&mut st, INBOX_OPEN, &u32s(&[next, 3]));
    let o2 = c.take(INBOX_OPEN_REPLY);
    assert_eq!(u32_at(&o2, 0x22), award, "extends the owned instance");
    let left: i64 = st
        .db
        .query_row(
            "SELECT COUNT(*) FROM inbox WHERE user_no = ?1",
            [a as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 2);
    // the function item opens with the general layout (count 1) and starts unused
    let fn_row = u32_at(&d, 0x16 + 3 * 0xe8);
    c.send(&mut st, INBOX_OPEN, &u32s(&[fn_row, 2]));
    let o = c.take(INBOX_OPEN_REPLY);
    assert_eq!((u32_at(&o, 6), u32_at(&o, 0xe)), (1, 1));
    let it = db::item(&st.db, u32_at(&o, 0x22) as i64).unwrap().unwrap();
    assert_eq!((it.item_no, it.use_type, it.used), (9104, 1, 0));
    // a wrong product id or a foreign row is refused
    c.send(
        &mut st,
        INBOX_OPEN,
        &u32s(&[u32_at(&d, 0x16 + 2 * 0xe8), 2]),
    );
    assert_eq!(u32_at(&c.take(INBOX_OPEN_REPLY), 6), 0);
}

/// LG-125 (playtest report: "I bought a supply crate with NX but the crate did not show up in my inbox. My inbox currently
/// has 2 pages full of hisec cases"): a supply case bought with NX lands in the inbox as the newest row, ahead of the
/// older case rows, and the buyer is told the inbox changed (0x3014), so a client holding the old rows fetches again.
#[test]
fn an_nx_supply_case_lands_first_in_a_full_inbox_and_marks_it_stale() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    // two pages (4 rows each) of older rows
    for _ in 0..8 {
        crate::inbox::add_row(&st, a, 5, 0, "", "").unwrap();
    }
    c.send(&mut st, INBOX_PAGE, &u32s(&[1]));
    c.take(INBOX_PAGE_REPLY);
    let gp = col(&st, a, "gp");
    let nx = col(&st, a, "nx");
    // product 4 is a single GP case in the fixture; product 3 the NX case pack (split into rows)
    st.rules.shop.case_split_on_purchase = false;
    let d = buy(&mut c, &mut st, 3, 1, 0);
    assert_eq!(u32_at(&d, 6), 1);
    assert!(c.has(INBOX_STALE_PUSH), "the buyer's inbox is marked stale");
    assert!(col(&st, a, "nx") < nx, "NX paid");
    assert_eq!(col(&st, a, "gp"), gp);
    // no item was drawn into the inventory
    let cases: i64 = st
        .db
        .query_row(
            "SELECT COUNT(*) FROM items WHERE user_no = ?1 AND item_no >= 8000 AND item_no < 9000",
            [a as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cases, 0);
    c.send(&mut st, INBOX_PAGE, &u32s(&[1]));
    let p = c.take(INBOX_PAGE_REPLY);
    assert_eq!((u32_at(&p, 0xe), u32_at(&p, 0x12)), (9, 9));
    assert_eq!(
        u32_at(&p, 0x16 + 4),
        3,
        "the new case row is the first row of page 1"
    );
}

/// CLI-962: an inbox row of a product with a period carries an end date (the receipt plus the period), a permanent
/// product's row carries none; the client shows it as the row's EXPIRES line and TEMPORARY mark.
#[test]
fn inbox_rows_of_a_dated_product_carry_an_end_date() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let dated = crate::inbox::add_row(&st, a, 9, 0, "", "").unwrap();
    let plain = crate::inbox::add_row(&st, a, 7, 0, "", "").unwrap();
    c.send(&mut st, INBOX_PAGE, &u32s(&[0]));
    let d = c.take(INBOX_PAGE_REPLY);
    let row = |id: i64| {
        (0..u32_at(&d, 0x12) as usize)
            .map(|i| 0x16 + i * 0xe8)
            .find(|o| u32_at(&d, *o) as i64 == id)
            .unwrap()
    };
    let end = |o: usize| u64::from_le_bytes(d[o + 0xe0..o + 0xe8].try_into().unwrap());
    let recv = |o: usize| u64::from_le_bytes(d[o + 0x32..o + 0x3a].try_into().unwrap());
    let o = row(dated);
    assert_eq!(
        end(o),
        recv(o) + 7 * crate::clock::TICKS_PER_DAY as u64,
        "the 7-day item"
    );
    assert_eq!(end(row(plain)), 0, "a permanent product has no end");
}

#[test]
fn gp_case_opens_at_purchase_or_goes_to_the_inbox() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let d = buy(&mut c, &mut st, 4, 0, 0);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0x1e)),
        (1, 110),
        "CD-68 award item in +0x1e"
    );
    assert_eq!(
        db::item(&st.db, u32_at(&d, 0xe) as i64)
            .unwrap()
            .unwrap()
            .item_no,
        110
    );
    st.rules.shop.gp_case_open_on_buy = false;
    let d = buy(&mut c, &mut st, 4, 0, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0x1e)), (1, u32::MAX));
    let n: i64 = st
        .db
        .query_row(
            "SELECT COUNT(*) FROM inbox WHERE user_no = ?1",
            [a as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn gifts_and_offline_notices() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let (mut b, bn) = player(&mut st, 2, "b", "Bob");
    let carol = db::create_account(&st.db, "c", 0).unwrap();
    db::create_character(
        &st.db,
        &db::NewCharacter {
            user_no: carol,
            nick: "Carol",
            face: 0,
            exp: 0,
            gp: 0,
            nx: 0,
            now: 0,
        },
        true,
    )
    .unwrap();
    c.send(&mut st, RECIPIENT_CHECK, &wstr("bob", 17));
    let d = c.take(RECIPIENT_CHECK_REPLY);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (0, 0x20));
    let gift = |to: &str, pid: u32| {
        let mut v = u32s(&[pid]);
        v.extend(wstr(to, 17));
        v.extend(wstr("hi", 83));
        v
    };
    // online receiver: 0x3014 and the row
    c.send(&mut st, GIFT_SEND, &gift("Bob", 2));
    assert_eq!(u32_at(&c.take(GIFT_SEND_REPLY), 6), 1);
    assert!(b.has(INBOX_STALE_PUSH));
    b.send(&mut st, INBOX_PAGE, &u32s(&[0]));
    let d = b.take(INBOX_PAGE_REPLY);
    assert_eq!((u32_at(&d, 0x12), u32_at(&d, 0x16 + 8)), (1, 1));
    // offline receiver: a pending notice, read once
    c.send(&mut st, GIFT_SEND, &gift("Carol", 2));
    assert_eq!(u32_at(&c.take(GIFT_SEND_REPLY), 6), 1);
    assert_eq!(col(&st, carol, "notice_pending"), 1);
    assert_eq!(col(&st, a, "nx"), 4900);
    // refusals: GP product 0x62, self 0x56, unknown 0x56, clan item to a non-leader 0x68, no money 0x67
    for (to, pid, want) in [
        ("Bob", 1, 0x62),
        ("Alice", 2, 0x56),
        ("Nobody", 2, 0x56),
        ("Bob", 8, 0x68),
    ] {
        c.send(&mut st, GIFT_SEND, &gift(to, pid));
        let d = c.take(GIFT_SEND_REPLY);
        assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (0, want), "{to} {pid}");
    }
    st.db
        .execute(
            "UPDATE characters SET nx = 1 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    c.send(&mut st, GIFT_SEND, &gift("Bob", 2));
    assert_eq!(u32_at(&c.take(GIFT_SEND_REPLY), 0xa), 0x67);
    // Carol logs in later and reads her notice
    let rx = connect(&mut st, 3, Listener::Lobby);
    st.sessions.get_mut(&3).unwrap().account = Some(carol);
    st.sessions.get_mut(&3).unwrap().in_lobby = true;
    let mut cc = Client {
        sid: 3,
        rx,
        got: Vec::new(),
    };
    cc.send(&mut st, crate::msg::NOTICE_FETCH, &[]);
    let d = cc.take(crate::msg::NOTICE_REPLY);
    assert!(dc_wire::PktReader::from_raw(&d)
        .at(0x12)
        .wstr(513)
        .contains("Alice sent you a gift"));
    assert_eq!(col(&st, carol, "notice_pending"), 0);
    let _ = bn;
}

#[test]
fn coupons_once_per_account_and_capped() {
    let mut st = rom_state();
    st.rules.shop.coupons = vec![crate::shopcfg::CouponSpec {
        code: "abc-123".into(),
        product: 5,
        uses: 1,
    }];
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let (mut b, _) = player(&mut st, 2, "b", "Bob");
    c.send(&mut st, COUPON_REDEEM, &wstr("ABC123", 65));
    let d = c.take(COUPON_REDEEM_REPLY);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xe)), (1, 5));
    assert!(c.has(INBOX_STALE_PUSH));
    let n: i64 = st
        .db
        .query_row(
            "SELECT COUNT(*) FROM inbox WHERE user_no = ?1 AND product_id = 5",
            [a as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
    c.send(&mut st, COUPON_REDEEM, &wstr("abc123", 65));
    assert_eq!(u32_at(&c.take(COUPON_REDEEM_REPLY), 0xa), 0x72);
    b.send(&mut st, COUPON_REDEEM, &wstr("abc123", 65));
    assert_eq!(
        u32_at(&b.take(COUPON_REDEEM_REPLY), 0xa),
        0x72,
        "total cap of 1 reached"
    );
    b.send(&mut st, COUPON_REDEEM, &wstr("nope", 65));
    assert_eq!(u32_at(&b.take(COUPON_REDEEM_REPLY), 0xa), 0x71);
}

#[test]
fn megaphone_queue_is_fifo_with_growing_serials() {
    let mut st = rom_state();
    st.rules.shop.megaphone_secs = 1;
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let (mut b, bn) = player(&mut st, 2, "b", "Bob");
    let m1 = give(&st, a, 9122, 3, 1);
    let m2 = give(&st, bn, 9122, 3, 2);
    let body = |uid: i64, text: &str| {
        let mut v = u32s(&[uid as u32, 1]);
        v.extend(wstr(text, 101));
        v.extend(vec![0u8; 402]);
        v
    };
    c.send(&mut st, MEGAPHONE_SEND, &body(m1, "first"));
    let d = c.take(MEGAPHONE_SEND_REPLY);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0x12)), (1, 0));
    assert!(
        db::item(&st.db, m1).unwrap().is_none(),
        "last use removes the row"
    );
    let first = b.take(BROADCAST_PUSH);
    b.send(&mut st, MEGAPHONE_SEND, &body(m2, "second"));
    assert_eq!(u32_at(&b.take(MEGAPHONE_SEND_REPLY), 0x12), 1);
    assert!(!b.has(BROADCAST_PUSH), "waits while the first is on air");
    b.send(&mut st, MEGAPHONE_QUEUE, &[]);
    assert_eq!(
        u32_at(&b.take(MEGAPHONE_QUEUE_REPLY), 0xa),
        0x8f,
        "one pending message per account"
    );
    c.send(&mut st, MEGAPHONE_QUEUE, &[]);
    let q = c.take(MEGAPHONE_QUEUE_REPLY);
    assert_eq!((u32_at(&q, 6), u32_at(&q, 0x1e)), (1, 1));
    assert!(u32_at(&q, 0x16) > u32_at(&q, 0x12));
    // time passes: the second goes on air with a larger serial
    st.db
        .execute("UPDATE megaphone SET aired = aired - 5 WHERE aired > 0", [])
        .unwrap();
    st.scope(crate::megaphone::tick).unwrap();
    let second = c.take(BROADCAST_PUSH);
    assert!(u32_at(&second, 0x12) > u32_at(&first, 0x12));
    // too long or a non-megaphone item is refused
    let other = give(&st, a, 9104, 1, 0);
    c.send(&mut st, MEGAPHONE_SEND, &body(other, "x"));
    assert_eq!(u32_at(&c.take(MEGAPHONE_SEND_REPLY), 6), 0);
}

/// use-items (owner report: the country code change and the megaphone did nothing): the country window's 0x330 sets
/// the nation everyone's views carry and consumes the item; the megaphone's first 0x330 keeps it, its queue question
/// and send put the message on air for the other players.
#[test]
fn country_change_and_megaphone_from_the_item_windows() {
    let mut st = rom_state();
    st.rules.shop.megaphone_secs = 1;
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let (mut b, _) = player(&mut st, 2, "b", "Bob");
    // country: an out-of-range nation is refused and the item stays; nation 16 is set and the item consumed
    let u = give(&st, a, 9117, 1, 0);
    let d = use_window(&mut c, &mut st, u, "", 0, 0, 0, 300);
    assert_eq!(u32_at(&d, 6), 0);
    assert!(db::item(&st.db, u).unwrap().is_some());
    let d = use_window(&mut c, &mut st, u, "", 0, 0, 0, 16);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xe)), (1, u as u32));
    assert!(
        db::item(&st.db, u).unwrap().is_none(),
        "the client deletes it too"
    );
    assert_eq!(col(&st, a, "nation"), 16);
    // what the other players see: the lobby user list, the room seat and the profile all carry the flag
    let me = st.character(a).unwrap();
    assert_eq!(st.lobby_user(a).nation, 16);
    assert_eq!(st.seat_row(a, 0, 0, 0).nation, 16);
    assert_eq!(st.profile(&me).nation, 16);
    // and the own 0x303 at +0x100
    c.send(&mut st, dc_wire::CHARACTER_INFO, &[]);
    assert_eq!(u32_at(&c.take(dc_wire::CHARACTER_INFO_REPLY), 0x100), 16);
    // megaphone: the first use answers ok with the count and keeps the item
    let m = give(&st, a, 9122, 3, 2);
    let d = use_window(&mut c, &mut st, m, "", 0, 0, 0, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0x1e)), (1, 2));
    assert!(db::item(&st.db, m).unwrap().is_some());
    c.send(&mut st, MEGAPHONE_QUEUE, &[]);
    assert_eq!(u32_at(&c.take(MEGAPHONE_QUEUE_REPLY), 6), 1);
    let mut body = u32s(&[m as u32, 1]);
    body.extend(wstr("hello lobby", 101));
    body.extend(vec![0u8; 402]);
    c.send(&mut st, MEGAPHONE_SEND, &body);
    let d = c.take(MEGAPHONE_SEND_REPLY);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0xe), u32_at(&d, 0x12)),
        (1, m as u32, 1)
    );
    let on_air = b.take(BROADCAST_PUSH);
    assert_eq!((u32_at(&on_air, 6), u32_at(&on_air, 0x16)), (1, a));
    assert_eq!(
        dc_wire::PktReader::from_raw(&on_air).at(0x48).wstr(100),
        "hello lobby"
    );
    assert_eq!(
        dc_wire::PktReader::from_raw(&on_air).at(0x26).wstr(17),
        "Alice"
    );
    assert!(
        c.has(BROADCAST_PUSH),
        "the sender hears it too (CLI-1392 box)"
    );
}

#[allow(clippy::too_many_arguments)]
fn use_window(
    c: &mut Client,
    st: &mut Core,
    uid: i64,
    name: &str,
    face: u32,
    colour: u32,
    change: u32,
    nation: u32,
) -> Vec<u8> {
    let mut v = u32s(&[uid as u32]);
    v.extend(wstr(name, 17));
    v.extend(u32s(&[0, 0, face, colour, 0, change, nation]));
    c.send(st, crate::msg::ITEM_USE, &v);
    c.take(crate::msg::ITEM_USE_REPLY)
}

/// use-items-emblem: the clan mark window's use with the pair at +0x2c / +0x30.
fn use_mark(c: &mut Client, st: &mut Core, uid: i64, emblem: u32, background: u32) -> Vec<u8> {
    let mut v = u32s(&[uid as u32]);
    v.extend(wstr("", 17));
    v.extend(u32s(&[emblem, background, 0, 0, 0, 0, 0]));
    c.send(st, crate::msg::ITEM_USE, &v);
    c.take(crate::msg::ITEM_USE_REPLY)
}

fn mark_check(c: &mut Client, st: &mut Core, emblem: u32, background: u32) -> (u32, u32) {
    c.send(
        st,
        crate::clan::CLAN_MARK_CHECK,
        &u32s(&[emblem, background]),
    );
    let d = c.take(crate::clan::CLAN_MARK_CHECK_REPLY);
    (u32_at(&d, 6), u32_at(&d, 0xa))
}

/// use-items-emblem (CLI-1884): the clan mark window end to end: the check refuses a pair another clan wears and a
/// member who is not the leader, the use stores the pair on the clan (the item stays, in use), the other members are
/// told and every player's profile of a member shows the new mark.
#[test]
fn the_clan_mark_window_sets_the_mark_other_players_see() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let (mut b, bn) = player(&mut st, 2, "b", "Bob");
    let (mut z, zn) = player(&mut st, 3, "z", "Zed");
    let clan = |st: &Core, name: &str, master: u32, mark: (i64, i64)| -> i64 {
        st.db
            .execute(
                "INSERT INTO clans (name, master, emblem, background) VALUES (?1, ?2, ?3, ?4)",
                params![name, master as i64, mark.0, mark.1],
            )
            .unwrap();
        st.db.last_insert_rowid()
    };
    let join = |st: &Core, who: u32, k: i64, role: i64, name: &str| {
        st.db
            .execute(
                "UPDATE characters SET clan_uid = ?2, clan_type = ?3, clan_name = ?4 WHERE user_no = ?1",
                params![who as i64, k, role, name],
            )
            .unwrap();
    };
    let wolves = clan(&st, "Wolves", a, (0, 0));
    let bears = clan(&st, "Bears", zn, (5, 6));
    join(&st, a, wolves, 5, "Wolves");
    join(&st, bn, wolves, 1, "Wolves");
    join(&st, zn, bears, 5, "Bears");
    // the check: another clan's pair, a plain member, a free pair
    assert_eq!(mark_check(&mut c, &mut st, 5, 6), (0, 0x4c));
    assert_eq!(mark_check(&mut b, &mut st, 7, 8), (0, 0x4b));
    assert_eq!(mark_check(&mut c, &mut st, 7, 8), (1, 0));
    // the use: kept in use, the clan's mark stored, the member told
    let u = give(&st, a, 9102, 1, 999);
    let d = use_mark(&mut c, &mut st, u, 7, 8);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xe)), (1, u as u32));
    let it = db::item(&st.db, u).unwrap().unwrap();
    assert_eq!(it.used, 1);
    let pair: (i64, i64) = st
        .db
        .query_row(
            "SELECT emblem, background FROM clans WHERE clan_uid = ?1",
            [wolves],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(pair, (7, 8));
    b.take(CLAN_INFO_STALE_PUSH);
    // another player's view of the leader and of a member: the new mark (0x379 +0x9e / +0xa2)
    for who in [a, bn] {
        z.send(&mut st, crate::msg::PROFILE_FETCH, &u32s(&[who]));
        let d = z.take(crate::msg::PROFILE_REPLY);
        assert_eq!(
            (u32_at(&d, 0x9e), u32_at(&d, 0xa2)),
            (7, 8),
            "account {who}"
        );
    }
    // the pair is now the Wolves': the Bears' leader may not take it
    let u2 = give(&st, zn, 9102, 1, 999);
    assert_eq!(mark_check(&mut z, &mut st, 7, 8), (0, 0x4c));
    let d = use_mark(&mut z, &mut st, u2, 7, 8);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (0, 0x4c));
}

/// use-items-emblem (CLI-1885): the base character and gender items end to end: the face value is stored, the own
/// character record and the game server's player line carry it, the item is gone; a value outside the face rows is
/// refused and the item stays.
#[test]
fn the_face_windows_set_the_face_the_match_shows() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let u = give(&st, a, 9106, 1, 0);
    let d = use_window(&mut c, &mut st, u, "", 7000, 0, 0, 0);
    assert_eq!(u32_at(&d, 6), 0, "not a face value");
    assert!(db::item(&st.db, u).unwrap().is_some());
    let d = use_window(&mut c, &mut st, u, "", 6014, 0, 0, 0);
    assert_eq!(u32_at(&d, 6), 1);
    assert!(db::item(&st.db, u).unwrap().is_none());
    assert_eq!(col(&st, a, "face"), 14);
    c.send(&mut st, crate::msg::CHAR_SHEET_FETCH, &[]);
    assert_eq!(u32_at(&c.take(crate::msg::CHAR_SHEET_REPLY), 0x56), 6014);
    let me = st.character(a).unwrap();
    assert!(crate::game::loadout_tokens(&st, a, &me).contains(" face=14"));
    // the gender item likewise
    let u = give(&st, a, 9107, 1, 0);
    assert_eq!(
        u32_at(&use_window(&mut c, &mut st, u, "", 6004, 0, 0, 0), 6),
        1
    );
    assert_eq!(col(&st, a, "face"), 4);
}

/// LG-410 (OWNER REPORT 2026-11-05): with the creation table's genders the base character item takes only another
/// face of the stored gender and the gender item only a face of the other one; a gender change, then a second gender
/// item bought (not used) keeps the changed face in the record; its use goes back to the first gender.
#[test]
fn the_face_items_check_their_choice_sets_against_the_stored_face() {
    let mut st = rom_state();
    Arc::get_mut(&mut st.rom).unwrap().face_men = [(1, true), (2, true), (4, false), (5, false)]
        .into_iter()
        .collect();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let ok = |c: &mut Client, st: &mut Core, item: i64, face: u32| -> bool {
        let u = give(st, a, item, 1, 0);
        u32_at(&use_window(c, st, u, "", face, 0, 0, 0), 6) == 1
    };
    // the man of face 2: kind 6 refuses the own face and a woman, kind 7 refuses a man and an unknown face
    assert!(!ok(&mut c, &mut st, 9106, 6002));
    assert!(!ok(&mut c, &mut st, 9106, 6004));
    assert!(!ok(&mut c, &mut st, 9107, 6001));
    assert!(!ok(&mut c, &mut st, 9107, 6009));
    assert_eq!(col(&st, a, "face"), 2);
    // the gender change to face 4; a second gender item bought keeps it
    assert!(ok(&mut c, &mut st, 9107, 6004));
    assert_eq!(col(&st, a, "face"), 4);
    let second = give(&st, a, 9107, 1, 0);
    c.send(&mut st, crate::msg::CHAR_SHEET_FETCH, &[]);
    assert_eq!(u32_at(&c.take(crate::msg::CHAR_SHEET_REPLY), 0x56), 6004);
    // its use: only a man now
    assert_eq!(
        u32_at(&use_window(&mut c, &mut st, second, "", 6005, 0, 0, 0), 6),
        0
    );
    assert_eq!(
        u32_at(&use_window(&mut c, &mut st, second, "", 6001, 0, 0, 0), 6),
        1
    );
    assert_eq!(col(&st, a, "face"), 1);
    // kind 6 on the man: another man only
    assert!(!ok(&mut c, &mut st, 9106, 6005));
    assert!(ok(&mut c, &mut st, 9106, 6002));
    assert_eq!(col(&st, a, "face"), 2);
}

/// use-items-sweep (CD-100): a colour item's use with the item it replaces at +0x3c.
fn use_colour(c: &mut Client, st: &mut Core, uid: i64, colour: u32, used: i64) -> Vec<u8> {
    let mut v = u32s(&[uid as u32]);
    v.extend(wstr("", 17));
    v.extend(u32s(&[0, 0, 0, colour, used as u32, 0, 0]));
    c.send(st, crate::msg::ITEM_USE, &v);
    c.take(crate::msg::ITEM_USE_REPLY)
}

#[test]
fn a_reticle_replaces_the_one_in_use_and_the_same_item_extends() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    // the first reticle: in use with its colour, in a function slot
    let r1 = give(&st, a, 9115, 1, 30);
    let d = use_colour(&mut c, &mut st, r1, 3, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0x12)), (1, 0));
    let it = db::item(&st.db, r1).unwrap().unwrap();
    assert_eq!((it.used, it.func_value), (1, 3));
    let end1 = it.end_time;
    assert!(end1 > 0);
    // another reticle replacing it: the old one goes and is reported in +0x12
    let r2 = give(&st, a, 9116, 1, 30);
    let d = use_colour(&mut c, &mut st, r2, 5, r1);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0xe), u32_at(&d, 0x12)),
        (1, r2 as u32, r1 as u32)
    );
    assert!(db::item(&st.db, r1).unwrap().is_none());
    let end2 = db::item(&st.db, r2).unwrap().unwrap().end_time;
    // the same item number again: its period runs on from the old end
    let r3 = give(&st, a, 9116, 1, 30);
    let d = use_colour(&mut c, &mut st, r3, 5, r2);
    assert_eq!(u32_at(&d, 0x12), r2 as u32);
    let it = db::item(&st.db, r3).unwrap().unwrap();
    assert!(
        it.end_time >= end2 + 29 * crate::clock::TICKS_PER_DAY,
        "extended"
    );
    assert_eq!(u64_at(&d, 0x16), it.end_time as u64);
    // a used uid that is not the account's item of the kind is ignored
    let n = give(&st, a, 9108, 1, 30);
    let d = use_colour(&mut c, &mut st, n, 2, r3);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0x12)), (1, 0));
    assert!(db::item(&st.db, r3).unwrap().is_some());
}

#[test]
fn item_windows_by_kind() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let (_b, _) = player(&mut st, 2, "b", "Bob");
    // name change: taken -> 0x53 and the item stays; free -> consumed
    let u = give(&st, a, 9104, 1, 0);
    let d = use_window(&mut c, &mut st, u, "bob", 0, 0, 0, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (0, 0x53));
    assert!(db::item(&st.db, u).unwrap().is_some());
    // LG-172: a player may not take a GM name
    let d = use_window(&mut c, &mut st, u, "GMAlice", 0, 0, 0, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (0, 0x53));
    let d = use_window(&mut c, &mut st, u, "Alicia", 0, 0, 0, 0);
    assert_eq!(u32_at(&d, 6), 1);
    assert!(db::item(&st.db, u).unwrap().is_none());
    assert_eq!(st.character(a).unwrap().nick, "Alicia");
    // record reset clears exactly kills, deaths, headshots
    st.db.execute("UPDATE characters SET kills = 5, deaths = 6, headshots = 7, wins = 8 WHERE user_no = ?1", [a as i64]).unwrap();
    let u = give(&st, a, 9105, 1, 0);
    assert_eq!(
        u32_at(&use_window(&mut c, &mut st, u, "", 0, 0, 0, 0), 6),
        1
    );
    assert_eq!(
        (
            col(&st, a, "kills"),
            col(&st, a, "deaths"),
            col(&st, a, "headshots"),
            col(&st, a, "wins")
        ),
        (0, 0, 0, 8)
    );
    // name colour: period starts at the use, colour stored, item kept with its function word
    let u = give(&st, a, 9108, 1, 30);
    let d = use_window(&mut c, &mut st, u, "", 0, 17, 0, 0);
    assert_eq!(u32_at(&d, 6), 0, "colour index above ColorMax");
    let d = use_window(&mut c, &mut st, u, "", 0, 10, 0, 0);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0xe), u32_at(&d, 0x12)),
        (1, u as u32, 0)
    );
    assert!(u64_at(&d, 0x16) > 0);
    let it = db::item(&st.db, u).unwrap().unwrap();
    assert_eq!((it.used, it.remain, it.func_value), (1, 0, 10));
    assert_eq!(col(&st, a, "nick_color"), 10);
    // CD-81: the colour goes when the colour item goes
    db::delete_item(&st.db, u).unwrap();
    slots(&mut c, &mut st);
    assert_eq!(col(&st, a, "nick_color"), 0);
    // weapon change: the default primary is replaced, its uid reported in +0x12
    let old = st
        .db
        .query_row(
            "SELECT uid FROM items WHERE user_no = ?1 AND default_type = 1",
            [a as i64],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    let u = give(&st, a, 9112, 1, 0);
    let d = use_window(&mut c, &mut st, u, "", 0, 0, 999, 0);
    assert_eq!(
        u32_at(&d, 6),
        0,
        "change item must match the ROM ChangeItemID"
    );
    let d = use_window(&mut c, &mut st, u, "", 0, 0, 111, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0x12)), (1, old as u32));
    let it = db::item(&st.db, u).unwrap().unwrap();
    assert_eq!(
        (it.item_no, it.default_type, it.end_time, it.slot),
        (111, 1, 0, 0)
    );
    assert!(db::item(&st.db, old).unwrap().is_none());
    assert_eq!(slots(&mut c, &mut st)[0], 111);
    // clan items need a clan leader
    let u = give(&st, a, 9101, 1, 0);
    assert_eq!(
        u32_at(&use_window(&mut c, &mut st, u, "Wolves", 0, 0, 0, 0), 0xa),
        0x4a
    );
    st.db
        .execute(
            "INSERT INTO clans (name, master) VALUES ('Old', ?1)",
            [a as i64],
        )
        .unwrap();
    let clan = st.db.last_insert_rowid();
    st.db.execute("UPDATE characters SET clan_uid = ?2, clan_type = 1, clan_name = 'Old' WHERE user_no = ?1", params![a as i64, clan]).unwrap();
    assert_eq!(
        u32_at(&use_window(&mut c, &mut st, u, "Wolves", 0, 0, 0, 0), 0xa),
        0x4b
    );
    st.db
        .execute(
            "UPDATE characters SET clan_type = 5 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    assert_eq!(
        u32_at(&use_window(&mut c, &mut st, u, "Wolves", 0, 0, 0, 0), 6),
        1
    );
    let name: String = st
        .db
        .query_row("SELECT name FROM clans WHERE clan_uid = ?1", [clan], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(name, "Wolves");
    // CD-53 room badge: counted period starts now
    let u = give(&st, a, 9110, 1, 7);
    let d = use_window(&mut c, &mut st, u, "", 0, 0, 0, 0);
    assert_eq!(u32_at(&d, 6), 1);
    let it = db::item(&st.db, u).unwrap().unwrap();
    assert_eq!((it.used, it.remain, it.end_time > 0), (1, 0, true));
    // LG-197: the room badge in use sets 0x303 +0xe8
    let me = st.character(a).unwrap();
    assert_eq!(st.char_sheet(&me).badge, 1);
    // LG-198: the clan colour goes out in the views while a clan colour item is in use
    let u9 = give(&st, a, 9109, 1, 30);
    let d = use_window(&mut c, &mut st, u9, "", 0, 7, 0, 0);
    assert_eq!(u32_at(&d, 6), 1);
    let me = st.character(a).unwrap();
    assert_eq!(st.char_sheet(&me).clan_colour, 7);
    assert_eq!(st.profile(&me).clan_colour, 7);
    db::delete_item(&st.db, u9).unwrap();
    slots(&mut c, &mut st);
    assert_eq!(
        st.clan_colour(clan),
        0,
        "the clan colour goes with its item"
    );
    // LG-196: a 999-day period (the catalogue's clan emblem) starts permanent
    let u = give(&st, a, 9110, 1, 999);
    assert_eq!(
        u32_at(&use_window(&mut c, &mut st, u, "", 0, 0, 0, 0), 6),
        1
    );
    let it = db::item(&st.db, u).unwrap().unwrap();
    assert_eq!((it.used, it.remain, it.end_time), (1, 0, 0));
    // a foreign or unknown uid is refused
    assert_eq!(
        u32_at(&use_window(&mut c, &mut st, 99_999, "", 0, 0, 0, 0), 6),
        0
    );
}

#[test]
fn loadout_backpack_slots_defaults_and_discard() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let eq = |c: &mut Client, st: &mut Core, uid: i64, no: u32, slot: u32| {
        c.send(st, EQUIP, &u32s(&[uid as u32, no, slot]));
        u32_at(&c.take(EQUIP_REPLY), 6)
    };
    let rel = |c: &mut Client, st: &mut Core, uid: i64, no: u32| {
        c.send(st, UNEQUIP, &u32s(&[uid as u32, no, 0]));
        u32_at(&c.take(UNEQUIP_REPLY), 6)
    };
    let w1 = give(&st, a, 110, 0, 0);
    let w2 = give(&st, a, 112, 0, 0);
    let bp = give(&st, a, 7002, 0, 0);
    // the starter backpack (one slot): slot 10 ok, slot 11 not; a primary in slot 0 is its own slot only
    assert_eq!(eq(&mut c, &mut st, w1, 110, 10), 1);
    assert_eq!(eq(&mut c, &mut st, w2, 112, 11), 0);
    assert_eq!(
        eq(&mut c, &mut st, w2, 112, 1),
        0,
        "a primary does not fit the secondary slot"
    );
    assert_eq!(eq(&mut c, &mut st, w2, 999, 0), 0, "item number must match");
    // the two-slot backpack: equipping slot 9 empties 10..13
    assert_eq!(eq(&mut c, &mut st, bp, 7002, 9), 1);
    let s = slots(&mut c, &mut st);
    assert_eq!((s[9], s[10]), (7002, -1));
    assert_eq!(eq(&mut c, &mut st, w1, 110, 10), 1);
    assert_eq!(eq(&mut c, &mut st, w2, 112, 11), 1);
    assert_eq!(eq(&mut c, &mut st, w2, 112, 12), 0);
    // moving a weapon out of slot 0 brings the default back (CD-52); defaults are never released
    assert_eq!(eq(&mut c, &mut st, w1, 110, 0), 1);
    assert_eq!(slots(&mut c, &mut st)[0], 110);
    assert_eq!(eq(&mut c, &mut st, w1, 110, 13), 0, "beyond capacity");
    assert_eq!(rel(&mut c, &mut st, w1, 110), 1);
    assert_eq!(slots(&mut c, &mut st)[0], 101);
    let d = st
        .db
        .query_row(
            "SELECT uid FROM items WHERE user_no = ?1 AND default_type = 1",
            [a as i64],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(rel(&mut c, &mut st, d, 101), 0);
    // unequipping the backpack: its slots empty and slot 9 stays empty (CD-52 user change); a stale slot is repaired
    assert_eq!(rel(&mut c, &mut st, bp, 7002), 1);
    let s = slots(&mut c, &mut st);
    assert_eq!((s[9], s[11]), (-1, -1));
    st.rules.default_repair_backpack = true;
    assert_eq!(slots(&mut c, &mut st)[9], 7001, "\"all\" refills it");
    st.rules.default_repair_backpack = false;
    db::set_slot(&st.db, w2, 11).unwrap();
    assert_eq!(slots(&mut c, &mut st)[11], -1);
    // discard: equipped and default items refused, others removed with their attachments detached
    assert_eq!(eq(&mut c, &mut st, w1, 110, 0), 1);
    c.send(&mut st, ITEM_DISCARD, &u32s(&[w1 as u32, 110]));
    assert_eq!(u32_at(&c.take(ITEM_DISCARD_REPLY), 6), 0);
    c.send(&mut st, ITEM_DISCARD, &u32s(&[d as u32, 101]));
    assert_eq!(u32_at(&c.take(ITEM_DISCARD_REPLY), 6), 0);
    let opt = give(&st, a, 1001, 0, 0);
    c.send(&mut st, ATTACH, &u32s(&[opt as u32, 1001, w2 as u32]));
    assert_eq!(u32_at(&c.take(ATTACH_REPLY), 6), 1);
    c.send(&mut st, ITEM_DISCARD, &u32s(&[w2 as u32, 112]));
    assert_eq!(u32_at(&c.take(ITEM_DISCARD_REPLY), 6), 1);
    assert_eq!(db::item(&st.db, opt).unwrap().unwrap().parent_uid, 0);
    // tuning: kind 0x11 option attached, then cleared with -1
    c.send(
        &mut st,
        TUNING_SYNC,
        &u32s(&[w1 as u32, opt as u32, u32::MAX, u32::MAX]),
    );
    assert_eq!(db::item(&st.db, opt).unwrap().unwrap().parent_uid, w1);
    c.send(
        &mut st,
        TUNING_SYNC,
        &u32s(&[w1 as u32, u32::MAX, u32::MAX, u32::MAX]),
    );
    assert_eq!(db::item(&st.db, opt).unwrap().unwrap().parent_uid, 0);
}

/// CLI-1606 (playtest report: "snipers that already have a supressor shouldn't be able to equip one"): a
/// muzzle option goes only into an open muzzle socket (weapon-table state 1); a weapon with a built-in suppressor
/// (state 2) refuses it on 0x336 and on the tuning sync 0x33a, whatever the client sends.
// EVIDENCE: cshell+0x152d90 (socket state 1 only)
#[test]
fn a_built_in_suppressor_refuses_a_muzzle_option() {
    let mut st = rom_state();
    Arc::get_mut(&mut st.rom)
        .unwrap()
        .socket_states
        .extend([(110, [1, 1, 1]), (112, [2, 2, 1])]);
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let rifle = give(&st, a, 110, 0, 0);
    let sniper = give(&st, a, 112, 0, 0);
    let opt = give(&st, a, 1001, 0, 0);
    c.send(&mut st, ATTACH, &u32s(&[opt as u32, 1001, sniper as u32]));
    assert_eq!(u32_at(&c.take(ATTACH_REPLY), 6), 0, "the sniper refuses it");
    assert_eq!(db::item(&st.db, opt).unwrap().unwrap().parent_uid, 0);
    c.send(
        &mut st,
        TUNING_SYNC,
        &u32s(&[sniper as u32, opt as u32, u32::MAX, u32::MAX]),
    );
    assert_eq!(
        db::item(&st.db, opt).unwrap().unwrap().parent_uid,
        0,
        "nor through 0x33a"
    );
    c.send(&mut st, ATTACH, &u32s(&[opt as u32, 1001, rifle as u32]));
    assert_eq!(
        u32_at(&c.take(ATTACH_REPLY), 6),
        1,
        "an open socket takes it"
    );
    assert_eq!(db::item(&st.db, opt).unwrap().unwrap().parent_uid, rifle);
}

/// CD-77 for the worn gear: each kind goes to its own slot by item number range (cshell+0x259f00: head 4, goggle 6,
/// uniform 7, vest 8, character 14), never to another kind's slot or a backpack slot; equipping replaces the slot's
/// occupant and unequipping empties it.
#[test]
fn gear_items_equip_into_their_own_slot_only() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let eq = |c: &mut Client, st: &mut Core, uid: i64, no: u32, slot: u32| {
        c.send(st, EQUIP, &u32s(&[uid as u32, no, slot]));
        u32_at(&c.take(EQUIP_REPLY), 6)
    };
    for (no, own, other) in [
        (4005u32, 4u32, 6u32),
        (5003, 6, 4),
        (2001, 7, 8),
        (3002, 8, 7),
        (10004, 14, 8),
    ] {
        let uid = give(&st, a, no as i64, 0, 0);
        assert_eq!(
            eq(&mut c, &mut st, uid, no, other),
            0,
            "{no} refused in slot {other}"
        );
        assert_eq!(
            eq(&mut c, &mut st, uid, no, 10),
            0,
            "{no} refused in a backpack slot"
        );
        assert_eq!(eq(&mut c, &mut st, uid, no, own), 1, "{no} into slot {own}");
        assert_eq!(slots(&mut c, &mut st)[own as usize], no as i32);
    }
    // a second head item replaces the first; unequipping empties the slot
    let h2 = give(&st, a, 4008, 0, 0);
    assert_eq!(eq(&mut c, &mut st, h2, 4008, 4), 1);
    assert_eq!(slots(&mut c, &mut st)[4], 4008);
    c.send(&mut st, UNEQUIP, &u32s(&[h2 as u32, 4008, 0]));
    assert_eq!(u32_at(&c.take(UNEQUIP_REPLY), 6), 1);
    assert_eq!(slots(&mut c, &mut st)[4], -1);
}

#[test]
fn function_slots_and_lobby_versions() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let p = give(&st, a, 9113, 3, 100);
    // not in use yet: 0x368 refused
    let mut body = u32s(&[9113, p as u32]);
    c.send_framed(&mut st, EQUIP as i32, FUNCTION_EQUIP, &body);
    assert_eq!(u32_at(&c.take(FUNCTION_EQUIP_REPLY), 6), 0);
    let d = use_window(&mut c, &mut st, p, "", 0, 0, 0, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0x1e)), (1, 100));
    c.send(&mut st, crate::msg::EQUIPMENT, &[]);
    let e = c.take(crate::msg::EQUIPMENT_REPLY);
    // LG-199: SubType 13 sits in function slot 1 (cshell+0x2254e0)
    assert_eq!(u32_at(&e, 0x8a + 4), 9113, "assigned on use, by SubType");
    // empty function slots read -1 (0 would be a real item number after the client's n % 9000)
    assert!(
        [0usize, 2, 3, 4, 5, 6]
            .iter()
            .all(|&i| u32_at(&e, 0x8a + 4 * i) == u32::MAX),
        "empty slots are -1"
    );
    c.send_framed(&mut st, EQUIP as i32, FUNCTION_UNEQUIP, &body);
    let r = c.take(FUNCTION_UNEQUIP_REPLY);
    assert_eq!((u32_at(&r, 6), u32_at(&r, 0xe)), (1, p as u32));
    c.send_framed(&mut st, EQUIP as i32, FUNCTION_EQUIP, &body);
    assert_eq!(u32_at(&c.take(FUNCTION_EQUIP_REPLY), 6), 1);
    body = u32s(&[101, 1]);
    c.send_framed(&mut st, EQUIP as i32, FUNCTION_EQUIP, &body);
    assert_eq!(
        u32_at(&c.take(FUNCTION_EQUIP_REPLY), 6),
        0,
        "a weapon is no function item"
    );
    // 0x301 carries the shop versions (>= 1); a sale changes the hot list and its version
    let v1 = crate::shop::versions(&st).unwrap();
    assert!(v1.view >= 1 && v1.lists.iter().all(|&v| v >= 1) && v1.prices >= 1);
    buy(&mut c, &mut st, 1, 0, 0);
    let v2 = crate::shop::versions(&st).unwrap();
    assert_eq!(v2.lists[0], v1.lists[0] + 1, "hot list grew");
    c.send(&mut st, LIST_FETCH_0, &u32s(&[0]));
    let l = c.take(LIST_FETCH_0 + 1);
    // LG-50: the hot list is the sold product plus the credit padding of the Black Market's Hot tab
    let n = u32_at(&l, 0x12) as usize;
    assert_eq!(u32_at(&l, 0xe), v2.lists[0]);
    assert_eq!(n, crate::shop::flag_lists(&st).unwrap()[0].len());
    assert!(n > 1, "the sold product and the credit padding");
    // the ROM NewItem marker puts product 1 in list 1 (new)
    c.send(&mut st, LIST_FETCH_1, &u32s(&[0]));
    let l = c.take(LIST_FETCH_1 + 1);
    assert_eq!((u32_at(&l, 0x12), l[0x16]), (1, 1));
}

#[test]
fn seasonal_products_sell_for_gp_only_when_on_and_in_season() {
    // DC-DEFAULT: SHOP-2 (proposed)
    use dc_bute::seasonal::FIRST_PRODUCT;
    let mut st = rom_state();
    Arc::get_mut(&mut st.rom)
        .unwrap()
        .shop
        .catalog
        .as_mut()
        .unwrap()
        .add_seasonal();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let listed = |c: &mut Client, st: &mut Core| -> Vec<u16> {
        c.send(st, SHOP_VIEW_FETCH, &[]);
        let d = c.take(SHOP_VIEW_REPLY);
        let n = u32_at(&d, 0x12) as usize;
        (0..n)
            .map(|i| u16::from_le_bytes([d[0x16 + 2 * i], d[0x17 + 2 * i]]))
            .collect()
    };
    // the first row of the table, a Halloween item at 700 GP for a day
    let halloween = FIRST_PRODUCT as u16;
    let christmas_on = |date: (u32, u32)| !dc_bute::seasonal::on_sale(date.0, date.1).is_empty();
    assert!(christmas_on((12, 15)));
    // off by default: nothing changes
    assert!(!listed(&mut c, &mut st).contains(&halloween));
    assert_eq!(
        u32_at(&buy(&mut c, &mut st, halloween as u32, 0, 0), 0xa) & 0xffff,
        0x60
    );
    // on, in season: listed and bought for GP
    st.rules.shop.seasonal = true;
    st.rules.shop.seasonal_today = Some((11, 3));
    assert!(listed(&mut c, &mut st).contains(&halloween));
    let gp = col(&st, a, "gp");
    let items_before = crate::db::items(&st.db, a).unwrap().len();
    buy(&mut c, &mut st, halloween as u32, 0, 0);
    assert_eq!(col(&st, a, "gp"), gp - 700);
    let items = crate::db::items(&st.db, a).unwrap();
    assert_eq!(items.len(), items_before + 1);
    let bought = items.iter().find(|i| i.item_no == 237).unwrap();
    assert!(bought.end_time > 0, "it lasts a day, it is not permanent");
    // after its season: off the list and refused, and the buyer keeps what they bought
    st.rules.shop.seasonal_today = Some((12, 15));
    assert!(!listed(&mut c, &mut st).contains(&halloween));
    assert_eq!(
        u32_at(&buy(&mut c, &mut st, halloween as u32, 0, 0), 0xa) & 0xffff,
        0x60
    );
    assert!(crate::db::items(&st.db, a)
        .unwrap()
        .iter()
        .any(|i| i.item_no == 237));
    // an operator's own list stays exactly as written
    st.rules.shop.seasonal_today = Some((11, 3));
    st.rules.shop.list_override[0] = Some(vec![1]);
    assert_eq!(listed(&mut c, &mut st), vec![1]);
}

#[test]
fn hidden_products_are_listed_and_buyable_only_when_the_option_is_on() {
    use dc_mod_lua::JsonValue;
    let mut st = rom_state();
    let cat = st.rom.shop.catalog.clone().unwrap();
    Arc::get_mut(&mut st.rom).unwrap().shop.all_products =
        (0..0x10000).filter(|&p| cat.product(p).is_some()).collect();
    let (mut c, _a) = player(&mut st, 1, "a", "Alice");
    let listed = |c: &mut Client, st: &mut Core| -> Vec<u16> {
        c.send(st, SHOP_VIEW_FETCH, &[]);
        let d = c.take(SHOP_VIEW_REPLY);
        let n = u32_at(&d, 0x12) as usize;
        (0..n)
            .map(|i| u16::from_le_bytes([d[0x16 + 2 * i], d[0x17 + 2 * i]]))
            .collect()
    };
    // default off: product 6 (Shopview 0) is neither listed nor purchasable
    assert!(!st.rules.shop.show_hidden_products);
    let before = listed(&mut c, &mut st);
    assert!(!before.contains(&6), "{before:?}");
    assert_eq!(u32_at(&buy(&mut c, &mut st, 6, 0, 0), 0xa) & 0xffff, 0x60);
    // on: the whole catalogue is listed and the hidden product can be bought
    assert!(st
        .rules
        .apply("shop.show_hidden_products", &JsonValue::Bool(true)));
    let after = listed(&mut c, &mut st);
    assert!(
        after.contains(&6) && after.len() > before.len(),
        "{after:?}"
    );
    assert!(after.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(u32_at(&buy(&mut c, &mut st, 6, 0, 0), 6), 1);
    // an id that is not in the catalogue stays refused
    assert_eq!(u32_at(&buy(&mut c, &mut st, 99, 0, 0), 0xa) & 0xffff, 0x60);
    assert!(!st
        .rules
        .apply("shop.show_hidden_products", &JsonValue::Number(1.0)));
}

#[test]
fn config_forwards_shop_keys_and_reads_the_cd52_forms() {
    use dc_mod_lua::JsonValue;
    let mut r = crate::config::Rules::default();
    assert!(
        r.default_repair && !r.default_repair_backpack,
        "CD-52: weapons only"
    );
    assert!(r.apply("inventory.default_repair", &JsonValue::String("all".into())));
    assert!(r.default_repair && r.default_repair_backpack);
    assert!(r.apply("inventory.default_repair", &JsonValue::String("off".into())));
    assert!(!r.default_repair && !r.default_repair_backpack);
    assert!(r.apply("inventory.default_repair", &JsonValue::Bool(true)));
    assert!(!r.apply(
        "inventory.default_repair",
        &JsonValue::String("bogus".into())
    ));
    assert!(r.apply("megaphone.secs", &JsonValue::Number(5.0)));
    assert_eq!(r.shop.megaphone_secs, 5);
    assert!(!r.apply("shop.bogus", &JsonValue::Number(1.0)));
}

/// LG-50 (playtest report: "black market is empty still"): a server without sales still lists credit products in the
/// Hot list, which the Black Market's Hot tab shows.
#[test]
fn rom_hot_list_of_a_fresh_server_holds_credit_products() {
    let st = rom_state();
    let cat = st.rom.shop.catalog.clone().unwrap();
    let [hot, _, _] = crate::shop::flag_lists(&st).unwrap();
    let credit = hot
        .iter()
        .filter(|p| cat.product(**p).is_some_and(|p| p.sale_type == 1))
        .count();
    // the fixture sells four credit products of four items: all of them (the real catalogue has far more than the cap)
    assert_eq!(credit, 4.min(crate::shop::HOT_CREDIT_MIN), "{hot:?}");
}

// LG-199: the slot follows the SubType; LG-200 / LG-201: passes pay and count down; LG-202: the clan mark goes with
// its item; LG-203: the badge rule
#[test]
fn passes_pay_count_down_and_the_small_function_rules() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    // an unused pass pays nothing
    let gp_pass = give(&st, a, 9114, 3, 2);
    let exp_pass = give(&st, a, 9113, 3, 1);
    assert_eq!(crate::rewards::pass_fractions(&st, a), (0.0, 0.0));
    // use the GP pass first: SubType 14 sits in slot 2, SubType 13 in slot 1
    assert_eq!(
        u32_at(&use_window(&mut c, &mut st, gp_pass, "", 0, 0, 0, 0), 6),
        1
    );
    assert_eq!(
        u32_at(&use_window(&mut c, &mut st, exp_pass, "", 0, 0, 0, 0), 6),
        1
    );
    let fs = crate::loadout::function_slots(&st.db, a).unwrap();
    assert_eq!(fs, [-1, 9113, 9114, -1, -1, -1, -1]);
    assert_eq!(crate::rewards::pass_fractions(&st, a), (0.25, 0.5));
    assert_eq!(crate::rewards::pass_extra((0.25, 0.5), (23, 40)), (6, 20));
    // a pass taken out of its slot pays nothing
    st.db
        .execute("DELETE FROM function_slots WHERE uid = ?1", [exp_pass])
        .unwrap();
    assert_eq!(crate::rewards::pass_fractions(&st, a), (0.0, 0.5));
    st.db
        .execute(
            "INSERT INTO function_slots (uid, user_no, slot) VALUES (?1, ?2, 1)",
            params![exp_pass, a as i64],
        )
        .unwrap();
    // each match lowers every active counted pass; at 0 the item goes and the owner hears 0x33f
    c.got.clear();
    st.scope(|st| Ok(crate::rewards::count_down_passes(st, a)?))
        .unwrap();
    assert_eq!(db::item(&st.db, gp_pass).unwrap().unwrap().remain, 1);
    assert!(db::item(&st.db, exp_pass).unwrap().is_none());
    c.got.extend(drain(&c.rx));
    let p = c.take(crate::msg::EXPIRY_PUSH);
    assert_eq!((u32_at(&p, 0xe), u32_at(&p, 0x12)), (1, exp_pass as u32));
    st.scope(|st| Ok(crate::rewards::count_down_passes(st, a)?))
        .unwrap();
    assert!(db::item(&st.db, gp_pass).unwrap().is_none());
    assert_eq!(crate::rewards::pass_fractions(&st, a), (0.0, 0.0));

    // LG-202: a clan mark item that expires clears the clan's emblem
    st.db
        .execute(
            "INSERT INTO clans (name, master, emblem, background) VALUES ('Old', ?1, 5, 6)",
            [a as i64],
        )
        .unwrap();
    let clan = st.db.last_insert_rowid();
    st.db
        .execute(
            "UPDATE characters SET clan_uid = ?2 WHERE user_no = ?1",
            params![a as i64, clan],
        )
        .unwrap();
    let mark = give(&st, a, 9102, 0, 0);
    st.db
        .execute(
            "UPDATE items SET used = 1, end_time = 1 WHERE uid = ?1",
            [mark],
        )
        .unwrap();
    st.scope(|st| crate::inventory::expire(st, Some(a), false))
        .unwrap();
    let pair: (i64, i64) = st
        .db
        .query_row(
            "SELECT emblem, background FROM clans WHERE clan_uid = ?1",
            [clan],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(pair, (0, 0));

    // LG-203: the badge rule
    assert!(!crate::room::badge_user(&st, a));
    let badge = give(&st, a, 9110, 1, 7);
    use_window(&mut c, &mut st, badge, "", 0, 0, 0, 0);
    assert!(crate::room::badge_user(&st, a));
}

/// LG-401: a GM account buys a ware below its rank requirement; the same purchase is refused once the flag is off.
#[test]
fn a_gm_account_buys_below_the_rank_requirement() {
    let mut st = rom_state();
    let shop = SHOP.replace(
        "Itemid=110
Itemtype=0
",
        "Itemid=110
Itemtype=0
GP_Unlock=500
",
    );
    Arc::get_mut(&mut st.rom).unwrap().shop.catalog =
        Some(ShopCatalog::from_doc(&ButeDoc::parse(&shop)));
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let gm = |st: &Core, on: i64| {
        st.db
            .execute(
                "UPDATE accounts SET gm = ?1 WHERE user_no = ?2",
                [on, a as i64],
            )
            .unwrap();
    };
    gm(&st, 0);
    assert_eq!(u32_at(&buy(&mut c, &mut st, 1, 0, 0), 0xa) & 0xffff, 0x60);
    gm(&st, 1);
    let d = buy(&mut c, &mut st, 1, 0, 0);
    assert_eq!(u32_at(&d, 6), 1, "the GM's purchase goes through");
    assert_eq!(col(&st, a, "gp"), 9900);
}

/// LG-302: a purchase below the cell's rank requirement (the item row's GP / NX unlock experience) is refused with
/// reason 0x60 and costs nothing; at the requirement it succeeds; an extension of an owned instance and a prepaid
/// credit purchase (it waits in the inbox, 0x1824) are not refused.
#[test]
fn a_purchase_below_the_rank_requirement_is_refused() {
    let mut st = rom_state();
    let shop = SHOP
        .replace(
            "Itemid=110\nItemtype=0\n",
            "Itemid=110\nItemtype=0\nGP_Unlock=500\n",
        )
        .replace(
            "Itemid=9104\nItemtype=21\n",
            "Itemid=9104\nItemtype=21\nNX_Unlock=500\n",
        );
    Arc::get_mut(&mut st.rom).unwrap().shop.catalog =
        Some(ShopCatalog::from_doc(&ButeDoc::parse(&shop)));
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let items = |st: &Core| -> i64 {
        st.db
            .query_row(
                "SELECT COUNT(*) FROM items WHERE user_no = ?1 AND item_no = 110",
                [a as i64],
                |r| r.get(0),
            )
            .unwrap()
    };
    let d = buy(&mut c, &mut st, 1, 0, 0);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0xa) & 0xffff, u32_at(&d, 0x1a)),
        (0, 0x60, 10000)
    );
    assert_eq!((col(&st, a, "gp"), items(&st)), (10000, 0));
    st.db
        .execute(
            "UPDATE characters SET exp = 499 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    assert_eq!(u32_at(&buy(&mut c, &mut st, 1, 0, 0), 0xa) & 0xffff, 0x60);
    // at the requirement
    st.db
        .execute(
            "UPDATE characters SET exp = 500 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    let d = buy(&mut c, &mut st, 1, 0, 0);
    assert_eq!(u32_at(&d, 6), 1);
    assert_eq!((col(&st, a, "gp"), items(&st)), (9900, 1));
    // an owned instance is extended whatever the rank; a made-up unique id is not an extension
    let uid = u32_at(&d, 0xe);
    st.db
        .execute(
            "UPDATE characters SET exp = 0 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    let d = buy(&mut c, &mut st, 1, 0, uid);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xe)), (1, uid));
    assert_eq!(
        u32_at(&buy(&mut c, &mut st, 1, 0, 999_999), 0xa) & 0xffff,
        0x60
    );
    // a credit product: the credit balance (option 0) is refused, the prepaid balance goes to the inbox
    st.db
        .execute(
            "UPDATE characters SET nx_cr = 1000 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    let d = buy(&mut c, &mut st, 2, 0, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa) & 0xffff), (0, 0x60));
    assert_eq!(col(&st, a, "nx_cr"), 1000);
    let d = buy(&mut c, &mut st, 2, 1, 0);
    assert_eq!(u32_at(&d, 6), 1);
    assert_eq!(col(&st, a, "nx"), 5000 - 50);
}

/// The (credit, prepaid) of the next 0x345 the client got.
fn next_balance(c: &mut Client) -> (u32, u32) {
    let b = c.take(crate::msg::BALANCE_REPLY);
    (u32_at(&b, 0xe), u32_at(&b, 0x12))
}

/// LG-405 (owner report 2026-11-04: the NX in game was not the admin dashboard's): the balance a purchase checks and
/// the 0x345 after its answer are the DB row's at that moment, also when the row changed behind the session as the
/// admin dashboard and the account CLI change it; opening the shop reports the row too.
#[test]
fn a_balance_changed_behind_the_session_is_what_the_next_buy_checks_and_reports() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let (mut b, _) = player(&mut st, 2, "b", "Bob");
    let set_nx = |st: &Core, nx: i64| {
        st.db
            .execute(
                "UPDATE characters SET nx = ?2 WHERE user_no = ?1",
                params![a as i64, nx],
            )
            .unwrap();
    };
    // the shop opens (0x30a {1}): its answer, then the row's balance
    c.send(&mut st, crate::msg::PRIVATE_STATE, &u32s(&[1]));
    c.take(crate::msg::PRIVATE_STATE_REPLY);
    assert_eq!(next_balance(&mut c), (0, 5000));
    // the row drops behind the session: product 2 (50 prepaid) is refused on it and the answer reports it
    set_nx(&st, 30);
    let d = buy(&mut c, &mut st, 2, 1, 0);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa) & 0xffff), (0, 0x3b));
    assert_eq!(next_balance(&mut c), (0, 30));
    assert_eq!(col(&st, a, "nx"), 30);
    // the row rises behind the session: the purchase is checked and charged on it, the answer reports what is left
    set_nx(&st, 120);
    let d = buy(&mut c, &mut st, 2, 1, 0);
    assert_eq!(u32_at(&d, 6), 1);
    assert_eq!(next_balance(&mut c), (0, 70));
    assert_eq!(col(&st, a, "nx"), 70);
    // the credit balance the same way (option 0)
    st.db
        .execute(
            "UPDATE characters SET nx_cr = 60 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    let d = buy(&mut c, &mut st, 2, 0, 0);
    assert_eq!(u32_at(&d, 6), 1);
    assert_eq!(next_balance(&mut c), (10, 70));
    // a gift is paid from the row and its answer reports it
    set_nx(&st, 55);
    let mut g = u32s(&[2]);
    g.extend(wstr("Bob", 17));
    g.extend(wstr("hi", 83));
    c.send(&mut st, GIFT_SEND, &g);
    assert_eq!(u32_at(&c.take(GIFT_SEND_REPLY), 6), 1);
    assert_eq!(next_balance(&mut c), (10, 5));
    // the receiver's balance did not move and was not reported to it
    assert!(!b.has(crate::msg::BALANCE_REPLY));
}

/// One played match of `players` (account, kills) on team 1 through the game server's result lines (LG-200 pays the
/// passes, LG-201 counts them).
fn play_match(st: &mut Core, id: u64, players: &[(u32, u32)]) {
    let key = crate::room::RoomKey { channel: 1, no: 0 };
    st.games.insert(
        id,
        crate::game::Game {
            id,
            room: key,
            port: 0,
            map: 0,
            mode: 0,
            link: Default::default(),
            ready: true,
            over: false,
            players: Vec::new(),
            clients: Default::default(),
            left: Default::default(),
            result_done: false,
            entry_keys: Default::default(),
            seat_tickets: Default::default(),
            seat_told: Default::default(),
            reap_scheduled: false,
            tmp_workdir: None,
            hisec_keys: Default::default(),
            net: Default::default(),
        },
    );
    st.scope(|st| {
        for (cl, (u, kills)) in players.iter().enumerate() {
            crate::game::on_line(st, id, &format!("@join {cl} {u} 1 {cl} N{u}"))?;
            crate::game::on_line(
                st,
                id,
                &format!("@player uid={u} kills={kills} deaths=1 team=1 tk=0 hs=0 name=N{u}"),
            )?;
        }
        crate::game::on_line(
            st,
            id,
            "@result mode=TDM win=1 alpha=5 bravo=2 limit=5 rounds=1:0",
        )
    })
    .unwrap();
}

/// OWNER REPORT 2026-11-04 ("the bonus EXP passes are not supposed to be permanent items"): a bought pass is not
/// permanent. A 7-day pass (consume 1) starts at its use and ends 7 days later; it pays its ExpPercent in a match
/// result before the end and nothing after it, and the expiry sweep removes it with 0x33f. A counted pass (consume 3,
/// the 2009 ROM's kind for the Bonus EXP / GP passes) keeps no end, pays in each match while it has uses, loses one
/// per match and goes at 0 with 0x33f. The control player has no pass.
// DC-DEFAULT: CD-53 (period starts at the use), CD-72 (consume kinds), LG-200 / LG-201 (passes pay and count)
#[test]
fn bought_passes_end_pay_only_while_active_and_are_removed() {
    let mut st = rom_state();
    let extra = "
[Product20]
ProductID=20
SaleType=0
Shopview=1
Price=10
SalePrice=10
Itemnum=1
Itemindex0=7
Consumetype0=1
Period0=7
[Product21]
ProductID=21
SaleType=0
Shopview=1
Price=10
SalePrice=10
Itemnum=1
Itemindex0=7
Consumetype0=3
Period0=2
[ItemInfo7]
Itemindex=7
Itemid=9113
Itemtype=21
";
    Arc::get_mut(&mut st.rom).unwrap().shop.catalog = Some(ShopCatalog::from_doc(&ButeDoc::parse(
        &format!("{SHOP}{extra}"),
    )));
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let (_b, b) = player(&mut st, 2, "b", "Bob");
    let exp = |st: &Core, u: u32| st.character(u).unwrap().exp;
    // the gain of a match: (with the pass, the control)
    let mut game = 500;
    let mut gains = |st: &mut Core| {
        let before = (exp(st, a), exp(st, b));
        game += 1;
        play_match(st, game, &[(a, 3), (b, 3)]);
        (exp(st, a) - before.0, exp(st, b) - before.1)
    };
    let base = gains(&mut st);
    assert_eq!(base.0, base.1, "no pass: the same gain");
    assert!(base.1 > 0, "the match pays");
    let bonus = (base.1 as f64 * 0.25).round() as i64;
    assert!(bonus > 0);

    // the 7-day pass: no end and not active until used
    let d = buy(&mut c, &mut st, 20, 0, 0);
    assert_eq!(u32_at(&d, 6), 1, "bought");
    let uid = u32_at(&d, 0xe) as i64;
    let it = db::item(&st.db, uid).unwrap().unwrap();
    assert_eq!((it.use_type, it.remain, it.end_time, it.used), (1, 7, 0, 0));
    assert_eq!(gains(&mut st), base, "an unused pass pays nothing");
    let r = use_window(&mut c, &mut st, uid, "", 0, 0, 0, 0);
    assert_eq!(u32_at(&r, 6), 1, "used");
    let now = crate::clock::now_ticks();
    let end = u64_at(&r, 0x16) as i64;
    let week = 7 * crate::clock::TICKS_PER_DAY;
    assert!(
        (end - (now + week)).abs() < 60 * crate::clock::TICKS_PER_SEC,
        "the end is 7 days out: {end} vs {now}"
    );
    assert_eq!(db::item(&st.db, uid).unwrap().unwrap().end_time, end);
    assert_eq!(
        gains(&mut st),
        (base.0 + bonus, base.1),
        "the pass pays before its end"
    );
    assert!(
        db::item(&st.db, uid).unwrap().is_some(),
        "a period pass is not counted"
    );
    // its end passes: no bonus, and the sweep removes it and tells the owner
    st.db
        .execute(
            "UPDATE items SET end_time = ?2 WHERE uid = ?1",
            params![uid, now - 1],
        )
        .unwrap();
    assert_eq!(gains(&mut st), base, "an ended pass pays nothing");
    c.got.clear();
    st.scope(|st| crate::inventory::expire(st, Some(a), true))
        .unwrap();
    assert!(
        db::item(&st.db, uid).unwrap().is_none(),
        "the sweep removed it"
    );
    let p = c.take(crate::msg::EXPIRY_PUSH);
    assert_eq!((u32_at(&p, 0xe), u32_at(&p, 0x12)), (1, uid as u32));

    // the counted pass: two matches, then gone
    let d = buy(&mut c, &mut st, 21, 0, 0);
    let uid = u32_at(&d, 0xe) as i64;
    let it = db::item(&st.db, uid).unwrap().unwrap();
    assert_eq!((it.use_type, it.remain, it.end_time), (3, 2, 0));
    let r = use_window(&mut c, &mut st, uid, "", 0, 0, 0, 0);
    assert_eq!((u32_at(&r, 6), u32_at(&r, 0x1e)), (1, 2), "used, two uses");
    assert_eq!(gains(&mut st), (base.0 + bonus, base.1));
    assert_eq!(db::item(&st.db, uid).unwrap().unwrap().remain, 1);
    c.got.clear();
    assert_eq!(gains(&mut st), (base.0 + bonus, base.1));
    assert!(db::item(&st.db, uid).unwrap().is_none(), "used up");
    let p = c.take(crate::msg::EXPIRY_PUSH);
    assert_eq!((u32_at(&p, 0xe), u32_at(&p, 0x12)), (1, uid as u32));
    assert_eq!(gains(&mut st), base, "a used-up pass pays nothing");
}

/// LG-406: the game server's `user` line carries the colour words (the ROM colour table's entries) of the player's
/// name colour and of the clan's colour, which it puts into the 0x6b record every client's scoreboard reads.
#[test]
fn the_game_server_gets_the_name_and_clan_colour_words() {
    let mut st = rom_state();
    Arc::get_mut(&mut st.rom).unwrap().shop.name_colour_words =
        vec![0xff11_2233, 0xff44_5566, 0xff77_8899];
    let (_c, a) = player(&mut st, 1, "a", "Alice");
    let c = st.character(a).unwrap();
    assert!(!crate::game::badge_tokens(&st, a, &c).contains("colour="));
    st.db
        .execute(
            "INSERT INTO clans (name, master) VALUES ('Owls', ?1)",
            [a as i64],
        )
        .unwrap();
    let clan = st.db.last_insert_rowid();
    st.db.execute("UPDATE characters SET clan_uid = ?2, clan_type = 1, clan_name = 'Owls', nick_color = 2 WHERE user_no = ?1", params![a as i64, clan]).unwrap();
    st.db
        .execute(
            "INSERT INTO clan_colours (clan_uid, colour) VALUES (?1, 3)",
            [clan],
        )
        .unwrap();
    let c = st.character(a).unwrap();
    let t = crate::game::badge_tokens(&st, a, &c);
    assert!(
        t.contains(&format!(" namecolour={}", 0xff44_5566u32)),
        "{t}"
    );
    assert!(
        t.contains(&format!(" clancolour={}", 0xff77_8899u32)),
        "{t}"
    );
    // a number past the table is the default colour: no token
    st.db
        .execute(
            "UPDATE characters SET nick_color = 9 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    let c = st.character(a).unwrap();
    assert!(!crate::game::badge_tokens(&st, a, &c).contains("namecolour="));
}

/// 0x35e / 0x360 {weapon, pass item number, slot} and their replies.
fn free_pass(c: &mut Client, st: &mut Core, put: bool, item: u32, pass: u32, slot: u32) -> u32 {
    use crate::freepass::*;
    let (id, reply) = if put {
        (FREE_PASS_EQUIP, FREE_PASS_EQUIP_REPLY)
    } else {
        (FREE_PASS_UNEQUIP, FREE_PASS_UNEQUIP_REPLY)
    };
    c.send(st, id, &u32s(&[item, pass, slot]));
    let d = c.take(reply);
    assert_eq!(d.len(), 0xa, "+6 ok");
    u32_at(&d, 6)
}

/// The 0x323 view: the item and the free-pass flag (+0x4e) of slot `slot`.
fn slot_view(c: &mut Client, st: &mut Core, slot: usize) -> (i32, u32) {
    c.send(st, crate::msg::EQUIPMENT, &[]);
    let d = c.take(crate::msg::EQUIPMENT_REPLY);
    (
        u32_at(&d, 0x12 + 4 * slot) as i32,
        u32_at(&d, 0x4e + 4 * slot),
    )
}

/// LG-407: a Weapon Free Pass (kind 16) in use lets 0x35e put one of its ROM weapons (FreePassItemID<n>) into the
/// weapon's own slot without owning it: 0x323 shows it with the slot's flag set, the game server gets it in
/// `loadout=`; refused before the use, for a weapon the pass does not list and for a slot the weapon does not take;
/// 0x360 takes it out again (the owned weapon under it is back), an owned weapon equipped into the slot replaces it.
#[test]
fn a_weapon_free_pass_puts_its_weapons_into_their_slot_while_in_use() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let own = slot_view(&mut c, &mut st, 0);
    assert_eq!(own.1, 0, "an owned primary, no flag");
    let pass = give(&st, a, 9126, 1, 1);
    assert_eq!(
        free_pass(&mut c, &mut st, true, 110, 9126, 0),
        0,
        "not in use yet"
    );
    let d = use_window(&mut c, &mut st, pass, "", 0, 0, 0, 0);
    assert_eq!(u32_at(&d, 6), 1, "the pass is used");
    assert_eq!(
        free_pass(&mut c, &mut st, true, 112, 9126, 0),
        0,
        "not on the pass"
    );
    assert_eq!(
        free_pass(&mut c, &mut st, true, 110, 9126, 1),
        0,
        "not its slot"
    );
    assert_eq!(
        free_pass(&mut c, &mut st, true, 110, 9122, 0),
        0,
        "not a pass item"
    );
    assert_eq!(free_pass(&mut c, &mut st, true, 110, 9126, 0), 1);
    assert_eq!(slot_view(&mut c, &mut st, 0), (110, 1));
    let me = st.character(a).unwrap();
    assert!(crate::game::loadout_tokens(&st, a, &me).contains(" loadout=0:110"));
    // the other weapon of the pass replaces it in the slot
    assert_eq!(free_pass(&mut c, &mut st, true, 111, 9126, 0), 1);
    assert_eq!(slot_view(&mut c, &mut st, 0), (111, 1));
    // 0x360 takes it out: the owned primary is back
    assert_eq!(
        free_pass(&mut c, &mut st, false, 110, 9126, 0),
        0,
        "not in the slot"
    );
    assert_eq!(free_pass(&mut c, &mut st, false, 111, 9126, 0), 1);
    assert_eq!(slot_view(&mut c, &mut st, 0), (own.0, 0));
    // an owned primary equipped into the slot replaces the free one
    assert_eq!(free_pass(&mut c, &mut st, true, 110, 9126, 0), 1);
    let gun = give(&st, a, 111, 0, 0);
    c.send(&mut st, EQUIP, &u32s(&[gun as u32, 111, 0]));
    assert_eq!(u32_at(&c.take(EQUIP_REPLY), 6), 1);
    assert_eq!(slot_view(&mut c, &mut st, 0), (111, 0));
    let rows: i64 = st
        .db
        .query_row("SELECT COUNT(*) FROM free_pass_slots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

/// LG-407: the free weapon lasts as long as its pass: once the pass has expired (or is gone) the slot shows the owned
/// weapon again, unflagged, the game server no longer gets the free one, and the row is dropped.
#[test]
fn a_free_weapon_leaves_its_slot_when_the_pass_ends() {
    let mut st = rom_state();
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    let own = slot_view(&mut c, &mut st, 0);
    let pass = give(&st, a, 9126, 1, 1);
    let d = use_window(&mut c, &mut st, pass, "", 0, 0, 0, 0);
    assert_eq!(u32_at(&d, 6), 1);
    assert_ne!(
        db::item(&st.db, pass).unwrap().unwrap().end_time,
        0,
        "the day starts at the use (CD-53)"
    );
    assert_eq!(free_pass(&mut c, &mut st, true, 110, 9126, 0), 1);
    assert_eq!(slot_view(&mut c, &mut st, 0), (110, 1));
    st.db
        .execute("UPDATE items SET end_time = 1 WHERE uid = ?1", [pass])
        .unwrap();
    let me = st.character(a).unwrap();
    assert!(!crate::game::loadout_tokens(&st, a, &me).contains("0:110"));
    assert_eq!(slot_view(&mut c, &mut st, 0), (own.0, 0));
    assert!(
        db::item(&st.db, pass).unwrap().is_none(),
        "the expiry scan took the pass"
    );
    let rows: i64 = st
        .db
        .query_row("SELECT COUNT(*) FROM free_pass_slots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0, "pruned with the repair");
}

/// LG-428: a refused purchase writes one log line naming the account, the product and the reason with its meaning.
#[test]
fn a_refused_purchase_is_logged_with_account_product_and_reason() {
    let mut st = rom_state();
    let dir = std::env::temp_dir().join(format!("dc_shoplog_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let logfile = dir.join("lobby.log");
    st.log = Arc::new(crate::logs::Logger::open(Some(&logfile), None).0);
    let (mut c, a) = player(&mut st, 1, "a", "Alice");
    // a product the catalogue does not list: 0x60; too little GP for a listed one: 0x3b
    assert_eq!(u32_at(&buy(&mut c, &mut st, 99, 0, 0), 0xa) & 0xffff, 0x60);
    st.db
        .execute(
            "UPDATE characters SET gp = 5 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    assert_eq!(u32_at(&buy(&mut c, &mut st, 1, 0, 0), 0xa) & 0xffff, 0x3b);
    let text = std::fs::read_to_string(&logfile).unwrap();
    for want in [
        format!("shop: purchase refused for account {a} (Alice): product 99, reason 0x60 (not for sale"),
        format!("shop: purchase refused for account {a} (Alice): product 1, reason 0x3b (not enough money)"),
    ] {
        assert!(text.contains(&want), "{want}\n{text}");
    }
    // a sale that goes through logs no refusal
    st.db
        .execute(
            "UPDATE characters SET gp = 100000 WHERE user_no = ?1",
            [a as i64],
        )
        .unwrap();
    let before = std::fs::read_to_string(&logfile)
        .unwrap()
        .matches("purchase refused")
        .count();
    assert_eq!(u32_at(&buy(&mut c, &mut st, 1, 0, 0), 6), 1);
    let after = std::fs::read_to_string(&logfile)
        .unwrap()
        .matches("purchase refused")
        .count();
    assert_eq!(before, after);
    let _ = std::fs::remove_dir_all(&dir);
}
