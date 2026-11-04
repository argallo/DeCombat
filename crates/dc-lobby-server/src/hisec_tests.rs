//! In-process flows of the HiSec lobby path (HS-6..HS-9) over a synthetic ROM: every table below is a made-up
//! fixture with the ROM's key shapes (no ROM values; the item numbers are invented).
// SPEC: HISEC_2009.md §2, §4.1-§4.3, §6.1; DC-DEFAULT: HS-6..HS-9, HS-13 (proposed)

use crate::db;
use crate::game::{self, Game};
use crate::logs::Listener;
use crate::net::route;
use crate::notices::CASE_ANNOUNCE_PUSH;
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
        let mut w = PktWriter::new(id);
        w.bytes(body);
        let p = w.into_payload();
        st.scope(|st| route(st, self.sid, id as i32, &p)).unwrap();
        self.got.extend(drain(&self.rx));
    }

    fn pump(&mut self) {
        self.got.extend(drain(&self.rx));
    }

    fn take(&mut self, id: u16) -> Vec<u8> {
        self.pump();
        let i = self
            .got
            .iter()
            .position(|(f, _)| *f == id)
            .unwrap_or_else(|| panic!("no 0x{id:x}"));
        let d = self.got[i].1.clone();
        self.got.drain(..=i);
        d
    }

    fn count(&mut self, id: u16) -> usize {
        self.pump();
        self.got.iter().filter(|(f, _)| *f == id).count()
    }
}

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Shop list: gray case 8071 (product 31), black cases 8072 / 8073 (32, 33), keys 9077 / 9078 (counted, 34 / 35),
/// a plain supply case 8001 (36).
const SHOP: &str = "
[ItemInfo0]
Itemindex=0
Itemid=8071
Itemtype=20
[ItemInfo1]
Itemindex=1
Itemid=8072
Itemtype=20
[ItemInfo2]
Itemindex=2
Itemid=8073
Itemtype=20
[ItemInfo3]
Itemindex=3
Itemid=9077
Itemtype=21
[ItemInfo4]
Itemindex=4
Itemid=9078
Itemtype=21
[ItemInfo5]
Itemindex=5
Itemid=8001
Itemtype=20
[Product0]
ProductID=31
Shopview=0
Itemnum=1
Itemindex0=0
Consumetype0=2
Period0=1
[Product1]
ProductID=32
Shopview=0
Itemnum=1
Itemindex0=1
Consumetype0=2
Period0=1
[Product2]
ProductID=33
Shopview=0
Itemnum=1
Itemindex0=2
Consumetype0=2
Period0=1
[Product3]
ProductID=34
Shopview=1
Itemnum=1
Itemindex0=3
Consumetype0=3
Period0=2
[Product4]
ProductID=35
Shopview=1
Itemnum=1
Itemindex0=4
Consumetype0=3
Period0=2
[Product5]
ProductID=36
Shopview=1
Itemnum=1
Itemindex0=5
Consumetype0=2
Period0=1
";

/// Item table: case 71 key class 4, cases 72 / 73 key class 5, plain case 1 (value -1); keys 77 (class 4) and 78
/// (class 5) of SubType 19; a function item of another subtype with class 4 (not a key). The award pools hold the
/// invented numbers 9001..9004 (function-item range: the award's type needs no table lookup).
const ITEMS: &str = "
[Supply71]
Available_ItemID0=9001
FunctionValue0=4
[Supply72]
Available_ItemID0=9002
FunctionValue0=5
[Supply73]
Available_ItemID0=9003
FunctionValue0=5
[Supply1]
Available_ItemID0=9004
FunctionValue0=-1
[UseItem0]
ItemID=77
SubType=19
FunctionValue0=4
[UseItem1]
ItemID=78
SubType=19
FunctionValue0=5
[UseItem2]
ItemID=79
SubType=12
FunctionValue0=4
";

fn rom_state() -> Core {
    let mut st = state();
    let rom = Arc::get_mut(&mut st.rom).unwrap();
    rom.shop.catalog = Some(ShopCatalog::from_doc(&ButeDoc::parse(SHOP)));
    rom.shop.all_products = vec![31, 32, 33, 34, 35, 36];
    rom.items = Some(ItemsTable::from_doc(ButeDoc::parse(ITEMS)));
    rom.hisec_rows = crate::hisec::rows_from_doc(&ButeDoc::parse(
        r#"[Hisec0]
Type=4
SubType=0
[Hisec1]
Type=5
SubType=0
[Hisec2]
Type=5
SubType=1
[Hisec3]
Type=9
SubType=0
"#,
    ));
    st
}

fn player(st: &mut Core, sid: Sid, login: &str, nick: &str, channel: u32) -> (Client, u32) {
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
    let rx = connect(st, sid, Listener::Lobby);
    let s = st.sessions.get_mut(&sid).unwrap();
    s.account = Some(a);
    s.in_lobby = true;
    s.channel = Some(channel);
    (
        Client {
            sid,
            rx,
            got: Vec::new(),
        },
        a,
    )
}

/// A game record whose `@join` table holds `accounts` (no room record: HS-14 reads the `@join` table only).
fn game_of(st: &mut Core, id: u64, accounts: &[u32]) {
    st.games.insert(
        id,
        Game {
            id,
            room: crate::room::RoomKey {
                channel: 1,
                no: 900 + id as u32,
            },
            port: 0,
            map: 0,
            mode: 0,
            link: Default::default(),
            ready: true,
            over: false,
            players: Vec::new(),
            clients: accounts
                .iter()
                .enumerate()
                .map(|(i, &a)| (i.to_string(), a))
                .collect(),
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
}

fn feed(st: &mut Core, game: u64, line: &str) {
    st.scope(|st| game::on_line(st, game, line)).unwrap();
}

/// Unopened inbox rows of an account: (entry id, product).
fn inbox(st: &Core, a: u32) -> Vec<(i64, i64)> {
    let mut q = st
        .db
        .prepare("SELECT uproduct, product_id FROM inbox WHERE user_no = ?1 AND opened = 0 ORDER BY uproduct")
        .unwrap();
    q.query_map([a as i64], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn give_key(st: &Core, a: u32, item_no: i64, remain: i64, end: i64) -> i64 {
    st.db
        .execute(
            "INSERT INTO items (user_no, item_no, use_type, remain, end_time) VALUES (?1, ?2, 3, ?3, ?4)",
            params![a as i64, item_no, remain, end],
        )
        .unwrap();
    st.db.last_insert_rowid()
}

fn open(c: &mut Client, st: &mut Core, entry: i64, product: i64) -> Vec<u8> {
    c.send(st, INBOX_OPEN, &u32s(&[entry as u32, product as u32]));
    c.take(INBOX_OPEN_REPLY)
}

#[test]
fn rom_lookups_find_cases_keys_rows_and_products() {
    let st = rom_state();
    assert_eq!(crate::hisec::case_value(&st, 8071), Some(4));
    assert_eq!(crate::hisec::case_value(&st, 8072), Some(5));
    assert_eq!(
        crate::hisec::case_value(&st, 8001),
        None,
        "value -1: plain case"
    );
    assert_eq!(crate::hisec::case_value(&st, 9077), None);
    assert_eq!(
        crate::hisec::key_items(&st, 4),
        vec![9077],
        "SubType 12 is no key"
    );
    assert_eq!(crate::hisec::key_items(&st, 5), vec![9078]);
    assert_eq!(crate::hisec::case_item_for_row(&st, 0), Some(8071));
    assert_eq!(crate::hisec::case_item_for_row(&st, 1), Some(8072));
    assert_eq!(
        crate::hisec::case_item_for_row(&st, 2),
        Some(8073),
        "SubType picks the variant"
    );
    assert_eq!(
        crate::hisec::case_item_for_row(&st, 3),
        None,
        "no case of key class 9"
    );
    assert_eq!(crate::hisec::case_product(&st, 1), Some(32));
    assert_eq!(crate::hisec::case_product(&st, 2), Some(33));
    assert_eq!(
        crate::hisec::case_product(&st, 3),
        Some(31),
        "fallback: the first HiSec case product"
    );
    assert_eq!(crate::hisec::case_product(&st, 77), Some(31));
    // without any ROM table nothing resolves
    let bare = state();
    assert_eq!(crate::hisec::case_product(&bare, 0), None);
}

#[test]
fn pickup_line_becomes_one_inbox_row_and_a_stale_push() {
    let mut st = rom_state();
    let (mut a, na) = player(&mut st, 1, "h1", "Alice", 1);
    let (mut b, nb) = player(&mut st, 2, "h2", "Bob", 1);
    let (_c, nc) = player(&mut st, 3, "h3", "Carol", 1);
    game_of(&mut st, 5, &[na, nb]);
    feed(
        &mut st,
        5,
        &format!("@hisec uid={na} case=1 key=1 name=Alice"),
    );
    assert_eq!(
        inbox(&st, na).iter().map(|r| r.1).collect::<Vec<_>>(),
        vec![32]
    );
    assert_eq!(a.count(INBOX_STALE_PUSH), 1);
    assert_eq!(b.count(INBOX_STALE_PUSH), 0, "only the picker's sessions");
    // the same case key again: nothing (one row per case)
    feed(
        &mut st,
        5,
        &format!("@hisec uid={na} case=1 key=1 name=Alice"),
    );
    assert_eq!(inbox(&st, na).len(), 1);
    // another key of the same match, another player, row 2 variant
    feed(
        &mut st,
        5,
        &format!("@hisec uid={nb} case=2 key=2 name=Bob"),
    );
    assert_eq!(
        inbox(&st, nb).iter().map(|r| r.1).collect::<Vec<_>>(),
        vec![33]
    );
    assert_eq!(b.count(INBOX_STALE_PUSH), 1);
    // an account that is not a player of the game, and an unknown account: ignored
    feed(
        &mut st,
        5,
        &format!("@hisec uid={nc} case=0 key=3 name=Carol"),
    );
    feed(&mut st, 5, "@hisec uid=999999 case=0 key=4 name=Nobody");
    assert!(inbox(&st, nc).is_empty());
    // a member of the game's room without a `@join` row (still in the waiting room) is no player of it (HS-14)
    let room = st.games[&5].room;
    st.rooms.insert(
        room,
        crate::room::Room {
            key: room,
            title: "r".into(),
            password: String::new(),
            mode: 0,
            map: 0,
            goal: 0,
            max_players: 8,
            mid_join: 1,
            ff_on: 0,
            killcam: 0,
            weapon_limit: 0,
            hacks: 0,
            hardcore: false,
            gun_game: false,
            possession: false,
            gun_seed: 0,
            ladder: Vec::new(),
            host_badge: 0,
            host: na,
            state: crate::room::RoomPhase::Playing,
            members: vec![crate::room::Member {
                account: nc,
                sid: 3,
                team: 1,
                state: crate::room::MEMBER_WAITING,
                sub: 0,
                seq: 1,
            }],
            kicked: Default::default(),
            gen: 0,
            game: Some(5),
            address_sent: true,
            played_since: None,
        },
    );
    feed(
        &mut st,
        5,
        &format!("@hisec uid={nc} case=0 key=3 name=Carol"),
    );
    assert!(inbox(&st, nc).is_empty());
    // a bad line is an ignored event, not a fault
    feed(&mut st, 5, "@hisec uid=x key=5");
    // an unknown game id: nothing
    feed(
        &mut st,
        6,
        &format!("@hisec uid={na} case=0 key=9 name=Alice"),
    );
    assert_eq!(inbox(&st, na).len(), 1);
    // the operator names the product
    let v = |t: &str| dc_mod_lua::JsonValue::parse(t).unwrap();
    assert!(st.rules.apply("hisec.case_product", &v("31")));
    feed(
        &mut st,
        5,
        &format!("@hisec uid={na} case=1 key=6 name=Alice"),
    );
    assert_eq!(
        inbox(&st, na).iter().map(|r| r.1).collect::<Vec<_>>(),
        vec![32, 31]
    );
}

/// HS-14: a line that delivers nothing does not use up its case key; the same key delivers once a product resolves.
#[test]
fn a_line_that_delivers_nothing_leaves_its_case_key_unused() {
    let mut st = state();
    let (mut a, na) = player(&mut st, 1, "n1", "Alice", 1);
    game_of(&mut st, 5, &[na]);
    feed(
        &mut st,
        5,
        &format!("@hisec uid={na} case=0 key=1 name=Alice"),
    );
    assert!(inbox(&st, na).is_empty(), "no ROM, no hisec.case_product");
    assert!(
        st.games[&5].hisec_keys.is_empty(),
        "the key is not recorded"
    );
    assert_eq!(a.count(INBOX_STALE_PUSH), 0);
    let v = |t: &str| dc_mod_lua::JsonValue::parse(t).unwrap();
    assert!(st.rules.apply("hisec.case_product", &v("31")));
    feed(
        &mut st,
        5,
        &format!("@hisec uid={na} case=0 key=1 name=Alice"),
    );
    assert_eq!(
        inbox(&st, na).iter().map(|r| r.1).collect::<Vec<_>>(),
        vec![31]
    );
    assert!(st.games[&5].hisec_keys.contains(&1));
    assert_eq!(a.count(INBOX_STALE_PUSH), 1);
    feed(
        &mut st,
        5,
        &format!("@hisec uid={na} case=0 key=1 name=Alice"),
    );
    assert_eq!(inbox(&st, na).len(), 1, "now the key is used up");
}

#[test]
fn opening_needs_a_matching_key_consumes_one_count_and_announces() {
    let mut st = rom_state();
    let (mut a, na) = player(&mut st, 1, "o1", "Alice", 1);
    let (mut b, _nb) = player(&mut st, 2, "o2", "Bob", 2);
    game_of(&mut st, 5, &[na]);
    feed(
        &mut st,
        5,
        &format!("@hisec uid={na} case=0 key=1 name=Alice"),
    );
    feed(
        &mut st,
        5,
        &format!("@hisec uid={na} case=0 key=2 name=Alice"),
    );
    let rows = inbox(&st, na);
    assert_eq!(rows.len(), 2);
    // no key: refused with the HS-7 reason, the row stays, nothing announced
    let d = open(&mut a, &mut st, rows[0].0, rows[0].1);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (0, 0x99));
    assert_eq!(inbox(&st, na).len(), 2);
    // a key of the other class does not open it
    let other = give_key(&st, na, 9078, 3, 0);
    let d = open(&mut a, &mut st, rows[0].0, rows[0].1);
    assert_eq!(u32_at(&d, 6), 0);
    assert_eq!(db::item(&st.db, other).unwrap().unwrap().remain, 3);
    // a matching key: case layout, +0x1e = the key's uid, one count used, 0x3013 to everyone (other channel too)
    let key = give_key(&st, na, 9077, 2, 0);
    a.got.clear();
    b.got.clear();
    let d = open(&mut a, &mut st, rows[0].0, rows[0].1);
    assert_eq!(d.len(), 0x5e);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0x12)), (1, 9001));
    assert_eq!(u32_at(&d, 0x1e) as i64, key);
    let days = u32_at(&d, 0x1a);
    assert!([7, 30, 0].contains(&days), "HS-8 table: {days}");
    assert_eq!(db::item(&st.db, key).unwrap().unwrap().remain, 1);
    assert_eq!(inbox(&st, na).len(), 1);
    let ann = a.take(CASE_ANNOUNCE_PUSH);
    assert_eq!((u32_at(&ann, 6), u32_at(&ann, 0x34)), (0, 9001));
    assert_eq!(b.count(CASE_ANNOUNCE_PUSH), 1);
    // the last count: the key row goes
    let d = open(&mut a, &mut st, rows[1].0, rows[1].1);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0x1e) as i64), (1, key));
    assert!(db::item(&st.db, key).unwrap().is_none());
    assert!(inbox(&st, na).is_empty());
}

#[test]
fn the_key_that_expires_first_is_used_and_expired_or_empty_keys_are_not() {
    let mut st = rom_state();
    let (mut a, na) = player(&mut st, 1, "k1", "Alice", 1);
    let now = crate::clock::now_ticks();
    let day = crate::clock::TICKS_PER_DAY;
    let permanent = give_key(&st, na, 9077, 1, 0);
    let _expired = give_key(&st, na, 9077, 5, now - day);
    let _empty = give_key(&st, na, 9077, 0, 0);
    let soon = give_key(&st, na, 9077, 1, now + day);
    let later = give_key(&st, na, 9077, 1, now + 3 * day);
    let mut used = Vec::new();
    for _ in 0..3 {
        let e = crate::inbox::add_row(&st, na, 31, 0, "", "").unwrap();
        let d = open(&mut a, &mut st, e, 31);
        assert_eq!(u32_at(&d, 6), 1);
        used.push(u32_at(&d, 0x1e) as i64);
    }
    assert_eq!(used, vec![soon, later, permanent]);
    let e = crate::inbox::add_row(&st, na, 31, 0, "", "").unwrap();
    assert_eq!(
        u32_at(&open(&mut a, &mut st, e, 31), 6),
        0,
        "only unusable keys left"
    );
}

#[test]
fn announcement_scope_and_plain_cases_are_unchanged() {
    let mut st = rom_state();
    let (mut a, na) = player(&mut st, 1, "s1", "Alice", 1);
    let (mut b, _) = player(&mut st, 2, "s2", "Bob", 1);
    let (mut c, _) = player(&mut st, 3, "s3", "Carol", 2);
    let v = |t: &str| dc_mod_lua::JsonValue::parse(t).unwrap();
    assert!(st.rules.apply("hisec.announce", &v("\"channel\"")));
    give_key(&st, na, 9077, 5, 0);
    let e = crate::inbox::add_row(&st, na, 31, 0, "", "").unwrap();
    open(&mut a, &mut st, e, 31);
    assert_eq!(
        (
            a.count(CASE_ANNOUNCE_PUSH),
            b.count(CASE_ANNOUNCE_PUSH),
            c.count(CASE_ANNOUNCE_PUSH)
        ),
        (1, 1, 0)
    );
    assert!(st.rules.apply("hisec.announce", &v("\"off\"")));
    a.got.clear();
    let e = crate::inbox::add_row(&st, na, 31, 0, "", "").unwrap();
    assert_eq!(u32_at(&open(&mut a, &mut st, e, 31), 6), 1);
    assert_eq!(a.count(CASE_ANNOUNCE_PUSH), 0);
    // a plain supply case needs no key and names none
    let e = crate::inbox::add_row(&st, na, 36, 0, "", "").unwrap();
    let d = open(&mut a, &mut st, e, 36);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0x12), u32_at(&d, 0x1e)),
        (1, 9004, 0)
    );
    // the duration table is an operator key
    assert!(st.rules.apply("economy.hisec_case_odds", &v("[[0, 1]]")));
    let e = crate::inbox::add_row(&st, na, 31, 0, "", "").unwrap();
    assert_eq!(u32_at(&open(&mut a, &mut st, e, 31), 0x1a), 0);
}

/// With the user's ROM: every case-table row resolves to a product whose item is a HiSec case with at least one
/// key, and different key classes stay apart. Structural only (no ROM value is asserted or stored).
#[test]
fn rom_case_rows_resolve_to_hisec_case_products_with_keys() {
    let Some(rom) = dc_bute::dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut st = state();
    st.rom = Arc::new(crate::rom::RomData::load(&rom));
    assert!(
        !st.rom.hisec_rows.is_empty(),
        "the ROM has a HiSec case table"
    );
    let cat = st.rom.shop.catalog.as_ref().expect("shop list");
    for (&row, &(class, _)) in &st.rom.hisec_rows {
        let pid = crate::hisec::case_product(&st, row).expect("a product for every row");
        let p = cat.product(pid).unwrap();
        let item = crate::shop::product_items(cat, p)[0].item_no;
        assert_eq!(
            crate::hisec::case_value(&st, item),
            Some(class),
            "row {row}"
        );
        let keys = crate::hisec::key_items(&st, class);
        assert!(!keys.is_empty(), "row {row}: a key opens it");
        for k in &keys {
            assert_eq!(
                crate::itemuse::subtype(&st, *k),
                Some(crate::hisec::SUB_HISEC_KEY)
            );
        }
    }
}
