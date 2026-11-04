//! A featured server entry's curated set (LG-313) and the pack item rule (LG-314), on a state with fixture packs.

use crate::db::{self, NewItem};
use crate::logs::{Listener, Logger};
use crate::modset::{build_entry_sets, EntrySet};
use crate::msg::*;
use crate::net::route;
use crate::state::testing::*;
use crate::state::{Core, Outgoing, Sid};
use dc_wire::entrymods::{self, EntryHashes};
use dc_wire::PktWriter;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc_entry_mods_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Packs a fixture mod into `<dir>/<sha256>.zip`; the reference for a config entry.
fn pack(dir: &Path, id: &str, kind: &str, caps: &[&str], lua: &str) -> dc_config::schema::ModRef {
    let src = dir.join("src").join(id);
    std::fs::create_dir_all(src.join("screenshots")).unwrap();
    std::fs::write(
        src.join("screenshots/main.png"),
        b"\x89PNG\r\n\x1a\nfixture",
    )
    .unwrap();
    let caps: Vec<String> = caps.iter().map(|c| format!("\"{c}\"")).collect();
    std::fs::write(
        src.join("manifest.json"),
        format!(
            r#"{{"id": "{id}", "name": "{id}", "version": "1.0.0", "entry": "main.lua", "kind": "{kind}",
              "description": "A test fixture pack for the unit tests.", "author": "tests",
              "screenshots": ["screenshots/main.png"], "capabilities": [{}]}}"#,
            caps.join(",")
        ),
    )
    .unwrap();
    std::fs::write(src.join("main.lua"), lua).unwrap();
    let (bytes, e) = dc_modkit::pack_mod_bytes(&src).unwrap();
    std::fs::write(dir.join(format!("{}.zip", e.sha256)), &bytes).unwrap();
    dc_config::schema::ModRef {
        id: id.into(),
        version: "1.0.0".into(),
        sha256: e.sha256,
    }
}

const WEAPON_LUA: &str =
    "return function(mod) mod.content.weapons:patch(145, {AmmoDamage = 41}) end";
const NOTHING_LUA: &str = "return function(mod) end";

fn entry(name: &str, mods: Vec<dc_config::schema::ModRef>) -> dc_config::LobbyServerCfg {
    dc_config::LobbyServerCfg {
        name: name.into(),
        capacity: 100,
        host: String::new(),
        channels: 3,
        features: Vec::new(),
        mods,
        lobby: String::new(),
        fingerprint: String::new(),
        policy: String::new(),
        region: String::new(),
    }
}

fn entries(mods: Vec<dc_config::schema::ModRef>) -> Vec<dc_config::LobbyServerCfg> {
    vec![
        entry("US Central", vec![]),
        entry("US West", vec![]),
        entry("Vanilla++ (US West)", mods),
    ]
}

/// A state with US Central, US West and a featured entry 3 holding `mods`.
fn featured(dir: &Path, mods: Vec<dc_config::schema::ModRef>) -> Core {
    let mut st = state();
    st.opts.servers = entries(mods);
    st.entry_mods = build_entry_sets(
        &st.opts.servers,
        Some(dir),
        dir,
        1 << 24,
        12,
        &Arc::new(Logger::none()),
    );
    st
}

struct Client {
    sid: Sid,
    rx: Receiver<Outgoing>,
}

impl Client {
    fn send(&self, st: &mut Core, id: u16, body: &[u8]) -> Vec<(u16, Vec<u8>)> {
        let mut w = PktWriter::new(id);
        w.bytes(body);
        let p = w.into_payload();
        st.scope(|st| route(st, self.sid, id as i32, &p)).unwrap();
        drain(&self.rx)
    }

    fn claim(&self, st: &mut Core, c: &EntryHashes) {
        let p = entrymods::claim_message(c).into_payload();
        st.scope(|st| route(st, self.sid, entrymods::CLAIM as i32, &p))
            .unwrap();
    }
}

fn wstr(s: &str, n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n * 2];
    for (i, u) in s.encode_utf16().take(n - 1).enumerate() {
        v[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
    }
    v
}

/// An auth login and character: the client and everything the server sent.
fn login(st: &mut Core, sid: Sid, name: &str) -> (Client, Vec<(u16, Vec<u8>)>) {
    let c = Client {
        sid,
        rx: connect(st, sid, Listener::Auth),
    };
    let mut body = wstr(&format!("LOCAL:{name}"), 4096);
    body.extend(wstr("pw", 17));
    body.extend(wstr("00-11", 20));
    body.extend(0i32.to_le_bytes());
    let mut got = c.send(st, LOGIN_REQUEST, &body);
    let r = &got.iter().find(|(id, _)| *id == LOGIN_REPLY).unwrap().1;
    if u32_at(r, 6) == 0 {
        let mut b = 6002i32.to_le_bytes().to_vec();
        b.extend(wstr(name, 17));
        got.extend(c.send(st, NEW_CHAR, &b));
    }
    (c, got)
}

fn pick(c: &Client, st: &mut Core, entry: u32) -> (bool, Option<String>, Vec<u8>) {
    let got = c.send(st, SERVER_PICK, &entry.to_le_bytes());
    let r = got
        .iter()
        .find(|(id, _)| *id == SERVER_PICK_REPLY)
        .unwrap()
        .1
        .clone();
    let text = got
        .iter()
        .find(|(id, _)| *id == entrymods::REFUSAL)
        .and_then(|(_, d)| entrymods::parse_refusal(d));
    (u32_at(&r, 6) == 1, text, r)
}

fn right_claim(st: &Core, entry: u32) -> EntryHashes {
    st.entry_mods[&entry].hashes()
}

/// A lobby session on `entry`, its account.
fn on_entry(st: &mut Core, sid: Sid, name: &str, entry: u32) -> (Client, u32) {
    let (c, _) = login(st, sid, name);
    let account = st.sessions[&sid].account.unwrap();
    st.sessions.remove(&sid);
    let l = Client {
        sid: sid + 1,
        rx: connect(st, sid + 1, Listener::Lobby),
    };
    let key = crate::auth::issue_ticket(st, account, entry);
    let mut b = key.to_vec();
    b.extend(account.to_le_bytes());
    let got = l.send(st, LOBBY_LOGIN, &b);
    assert_eq!(
        u32_at(
            &got.iter()
                .find(|(id, _)| *id == LOBBY_LOGIN_REPLY)
                .unwrap()
                .1,
            6
        ),
        1
    );
    drop(c);
    (l, account)
}

#[test]
fn a_pick_of_a_featured_entry_needs_its_exact_hashes_and_a_refusal_says_so() {
    let dir = temp("pick");
    let pack_a = pack(&dir, "pack_a", "shared", &["content.weapons"], WEAPON_LUA);
    let mut st = featured(&dir, vec![pack_a]);
    assert_eq!(st.entry_mods.keys().copied().collect::<Vec<_>>(), [3]);
    let want = right_claim(&st, 3);
    assert_ne!(
        want.content, "0",
        "the pack's weapon patch is client content"
    );
    assert_ne!(want.set, dc_wire::loginmods::EMPTY_SET_HASH);
    let (c, got) = login(&mut st, 1, "alice");
    assert_eq!(
        got.iter()
            .find(|(id, _)| *id == entrymods::PUSH)
            .map(|(_, d)| entrymods::parse_push(d)),
        Some(vec![want.clone()]),
        "the login tells the entries' hashes"
    );
    // nothing claimed (an older client, the 2009 client)
    let (ok, text, _) = pick(&c, &mut st, 3);
    assert!(!ok);
    assert!(text.unwrap().contains("Update DeCombat"));
    // a wrong set, a wrong content, a claim of another entry
    for bad in [
        EntryHashes {
            set: "1f".repeat(32),
            ..want.clone()
        },
        EntryHashes {
            content: "0".into(),
            ..want.clone()
        },
        EntryHashes {
            entry: 2,
            ..want.clone()
        },
    ] {
        c.claim(&mut st, &bad);
        let (ok, text, _) = pick(&c, &mut st, 3);
        assert!(!ok, "{bad:?}");
        assert!(text.is_some());
    }
    // the right one: a ticket; the claim is used up
    c.claim(&mut st, &want);
    assert!(pick(&c, &mut st, 3).0);
    assert!(!pick(&c, &mut st, 3).0, "a claim counts for one pick");
    // a plain entry never looks
    assert!(pick(&c, &mut st, 1).0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_move_to_a_featured_entry_is_checked_too_and_its_channels_need_the_pick() {
    let dir = temp("move");
    let pack_a = pack(&dir, "pack_a", "shared", &["content.weapons"], WEAPON_LUA);
    let mut st = featured(&dir, vec![pack_a]);
    let (l, account) = on_entry(&mut st, 1, "bob", 1);
    let want = right_claim(&st, 3);
    let got = l.send(&mut st, SERVER_MOVE, &3u32.to_le_bytes());
    assert!(got.iter().any(|(id, _)| *id == entrymods::REFUSAL));
    assert_eq!(
        u32_at(
            &got.iter()
                .find(|(id, _)| *id == SERVER_MOVE_REPLY)
                .unwrap()
                .1,
            6
        ),
        0
    );
    l.claim(&mut st, &want);
    let got = l.send(&mut st, SERVER_MOVE, &3u32.to_le_bytes());
    assert_eq!(
        u32_at(
            &got.iter()
                .find(|(id, _)| *id == SERVER_MOVE_REPLY)
                .unwrap()
                .1,
            6
        ),
        1
    );
    // a session that picked nothing (a --lenient login) on a lobby whose FIRST entry is the featured one
    let mut set = st.entry_mods.remove(&3).unwrap();
    set.entry = 1;
    st.entry_mods.insert(1, set);
    let mut b = account.to_le_bytes().to_vec();
    b.extend(1u32.to_le_bytes());
    b.extend(wstr("Channel 1", 32));
    st.sessions.get_mut(&l.sid).unwrap().server = 0;
    let got = l.send(&mut st, CHANNEL_ENTER, &b);
    assert_eq!(
        u32_at(
            &got.iter()
                .find(|(id, _)| *id == CHANNEL_ENTER_REPLY)
                .unwrap()
                .1,
            6
        ),
        0
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn plain_entries_keep_their_login_and_pick_byte_identical() {
    let dir = temp("bytes");
    let pack_a = pack(&dir, "pack_a", "shared", &["content.weapons"], WEAPON_LUA);
    // today's lobby: no set anywhere, the 3 entries as they are
    let mut plain = state();
    plain.opts.servers = entries(vec![]);
    let mut with = featured(&dir, vec![pack_a]);
    let (cp, got_plain) = login(&mut plain, 1, "carl");
    let (cw, got_with) = login(&mut with, 1, "carl");
    assert!(
        got_plain.iter().all(|(id, _)| *id != entrymods::PUSH),
        "no set anywhere: no new frame"
    );
    // the same frames, in the same order, but for our push; the server list too
    let others: Vec<_> = got_with
        .iter()
        .filter(|(id, _)| *id != entrymods::PUSH)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        others,
        got_plain.iter().map(|(id, _)| *id).collect::<Vec<_>>()
    );
    let list = |g: &[(u16, Vec<u8>)]| {
        g.iter()
            .find(|(id, _)| *id == SERVER_LIST_PUSH)
            .unwrap()
            .1
            .clone()
    };
    assert_eq!(list(&got_with), list(&got_plain));
    let at = got_with
        .iter()
        .position(|(id, _)| *id == entrymods::PUSH)
        .unwrap();
    assert_eq!(
        got_with[at + 1].0,
        SERVER_LIST_PUSH,
        "our push comes right before 0x2000"
    );
    // US Central and US West: a pick without a claim, the reply's form unchanged (the key is random)
    for e in [1, 2] {
        let (a, ta, ra) = pick(&cp, &mut plain, e);
        let (b, tb, rb) = pick(&cw, &mut with, e);
        assert!(a && b && ta.is_none() && tb.is_none());
        assert_eq!((ra.len(), &ra[..0xe]), (rb.len(), &rb[..0xe]));
    }
    // the lobby's own policy stays vanilla: the login hash check is what it was
    assert_eq!(with.login_mods, crate::state::ExpectedMods::default());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn every_forbidden_capability_is_refused_and_logged_and_the_good_pack_stays() {
    let dir = temp("limit");
    let log_file = dir.join("lobby.log");
    let good = pack(
        &dir,
        "good",
        "shared",
        &["content.weapons", "content.text", "content.hud", "assets"],
        NOTHING_LUA,
    );
    let mut mods = vec![good.clone()];
    let mut bad = Vec::new();
    for (id, kind, cap) in [
        ("rewards", "shared", "hooks.lobby"),
        ("prices", "shared", "content.shop"),
        ("xray", "shared", "world.full_visibility"),
        ("chams", "shared", "hooks.render"),
        ("damage", "shared", "hooks.combat"),
        ("hudhook", "shared", "hooks.hud"),
        ("recoil", "shared", "combat.no_recoil"),
        ("store", "shared", "save"),
        ("listen", "shared", "events"),
        ("items", "shared", "content.items"),
        ("rules_only", "server_rules", "content.weapons"),
        ("cosmetic", "client_visual", "content.text"),
    ] {
        mods.push(pack(&dir, id, kind, &[cap], NOTHING_LUA));
        bad.push(id);
    }
    // a new gun outside the reserved numbers
    mods.push(pack(
        &dir,
        "low_row",
        "shared",
        &["content.weapons"],
        r#"return function(mod) mod.content.weapons:register({id = "Weapon500", Name = "G"}) end"#,
    ));
    bad.push("low_row");
    // not there at all
    mods.push(dc_config::schema::ModRef {
        id: "absent".into(),
        version: "1.0.0".into(),
        sha256: "7a".repeat(32),
    });
    bad.push("absent");
    let log = Arc::new(Logger::open(Some(&log_file), None).0);
    let servers = entries(mods);
    let sets = build_entry_sets(&servers, Some(&dir), &dir, 1 << 24, 12, &log);
    let set = &sets[&3].set;
    assert_eq!(
        set.mods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["good"],
        "only the good pack is in the set"
    );
    assert_eq!(sets[&3].client.loaded, ["good"]);
    assert!(sets[&3].doc.contains("\"curated\": true"));
    drop(sets);
    let text = std::fs::read_to_string(&log_file).unwrap();
    for id in bad {
        assert!(
            text.contains(&format!("'{id}' left out")),
            "{id} is logged:\n{text}"
        );
    }
    // nothing left: the entry stays plain, the server runs
    let rewards = pack(&dir, "rewards", "shared", &["hooks.lobby"], NOTHING_LUA);
    let sets = build_entry_sets(&entries(vec![rewards]), Some(&dir), &dir, 1 << 24, 12, &log);
    assert!(sets.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_new_row_in_the_reserved_numbers_loads_and_its_content_reaches_that_entry_alone() {
    let dir = temp("content");
    let gun = pack(
        &dir,
        "new_gun",
        "shared",
        &["content.weapons"],
        r#"return function(mod) mod.content.weapons:register({id = "Weapon900", Name = "New Gun"}) end"#,
    );
    let st = featured(&dir, vec![gun]);
    let set =
        crate::servers::entry_set_of_channel(&st, 7).expect("channel 7 is the featured entry's");
    assert_eq!(set.entry, 3);
    let doc = std::fs::read_to_string(set.client.content_file.as_ref().unwrap()).unwrap();
    assert!(doc.contains(r#""weapon":900"#), "{doc}");
    for ch in [1, 3, 4, 6, 100] {
        assert!(
            crate::servers::entry_set_of_channel(&st, ch).is_none(),
            "channel {ch}"
        );
    }
    assert!(
        st.mods.is_none(),
        "the lobby's own mod host stays out of it"
    );
    assert_eq!(crate::game::content_arg(st.mods.as_ref()), None);
    // the request to a remote game host carries it only when asked
    let c = &set.client.content;
    let (hash, text) = (c.hash(), c.to_json());
    let j = crate::game::start_json("W", "ca", 60, "x.log", &[], Some((&hash, &text)), None);
    let v = dc_mod_lua::JsonValue::parse(&j).unwrap();
    assert_eq!(
        v.get("content").and_then(|p| p.as_str()),
        Some(text.as_str())
    );
    assert_eq!(
        v.get("content_hash").and_then(|p| p.as_str()),
        Some(hash.as_str())
    );
    assert!(!crate::game::start_json("W", "ca", 60, "x.log", &[], None, None).contains("content"));
    std::fs::remove_dir_all(&dir).ok();
}

/// A session that went through `on_entry`; the pack rule needs only an entry with a set.
fn add(st: &Core, account: u32, item_no: i64, slot: i64, default_type: i64) -> i64 {
    db::add_item(
        &st.db,
        &NewItem {
            user_no: account,
            item_no,
            start_time: 1,
            end_time: 0,
            slot,
            default_type,
        },
    )
    .unwrap()
}

#[test]
fn a_pack_item_is_hidden_and_falls_back_off_a_featured_entry_and_returns_on_it() {
    let dir = temp("items");
    let pack_a = pack(&dir, "pack_a", "shared", &["content.weapons"], WEAPON_LUA);
    let mut st = featured(&dir, vec![pack_a]);
    let (l, account) = on_entry(&mut st, 1, "dora", 1);
    let default_primary = db::items(&st.db, account)
        .unwrap()
        .into_iter()
        .find(|i| i.slot == 0)
        .unwrap();
    assert_ne!(default_primary.default_type, 0);
    // the granted test item, equipped in the primary slot: the default leaves the slot in the stored row
    db::set_slot(&st.db, default_primary.uid, -1).unwrap();
    let uid = add(&st, account, 900, 0, 0);
    let option = add(&st, account, 1035, -1, 0);
    st.db
        .execute(
            "UPDATE items SET parent_uid = ?1 WHERE uid = ?2",
            [uid, option],
        )
        .unwrap();
    let in_slots = |st: &mut Core, l: &Client| {
        let got = l.send(st, EQUIPMENT, &[]);
        u32_at(
            &got.iter().find(|(id, _)| *id == EQUIPMENT_REPLY).unwrap().1,
            0x12,
        ) as i64
    };
    let listed = |st: &mut Core, l: &Client, no: u32| {
        let got = l.send(st, INVENTORY, &[]);
        let r = got
            .iter()
            .find(|(id, _)| *id == INVENTORY_REPLY)
            .unwrap()
            .1
            .clone();
        r.windows(4).any(|w| w == no.to_le_bytes())
    };
    let tokens = |st: &Core, a: u32| crate::game::loadout_tokens(st, a, &st.character(a).unwrap());
    // a plain entry: the view shows the default, the list has no pack item, the match gets none
    assert_eq!(in_slots(&mut st, &l), default_primary.item_no);
    assert!(!listed(&mut st, &l, 900));
    let t = tokens(&st, account);
    assert!(!t.contains(":900"), "{t}");
    assert!(t.contains(&format!("0:{}", default_primary.item_no)), "{t}");
    // nor can it be equipped, discarded or given an attachment there
    assert!(!crate::loadout::equip(&st, account, uid as u32, 900, 0).unwrap());
    assert!(!crate::loadout::discard(&st, account, uid as u32, 900).unwrap());
    assert!(db::item(&st.db, uid).unwrap().is_some());
    // repair at login leaves the row: it is still stored in slot 0
    crate::inventory::repair_defaults(&st, account).unwrap();
    assert_eq!(
        db::item(&st.db, uid).unwrap().unwrap().slot,
        0,
        "the stored loadout is not touched"
    );
    // the featured entry: it is back, in its slot, in the list and in the match
    st.sessions.get_mut(&l.sid).unwrap().server = 3;
    assert_eq!(in_slots(&mut st, &l), 900);
    assert!(listed(&mut st, &l, 900));
    let t = tokens(&st, account);
    assert!(t.contains("0:900"), "{t}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_product_holding_a_pack_item_is_off_the_shop_and_off_a_plain_entrys_storage() {
    let mut st = state();
    let shop = [
        "[ItemInfo0]",
        "Itemindex=0",
        "Itemid=900",
        "Itemtype=0",
        "[ItemInfo1]",
        "Itemindex=1",
        "Itemid=110",
        "Itemtype=0",
        "[Product0]",
        "ProductID=7",
        "SaleType=0",
        "Shopview=1",
        "Price=10",
        "SalePrice=10",
        "Itemnum=1",
        "Itemindex0=0",
        "Consumetype0=0",
        "Period0=0",
        "[Product1]",
        "ProductID=8",
        "SaleType=0",
        "Shopview=1",
        "Price=10",
        "SalePrice=10",
        "Itemnum=1",
        "Itemindex0=1",
        "Consumetype0=0",
        "Period0=0",
    ];
    let doc = dc_bute::ButeDoc::parse(&shop.join(
        "
",
    ));
    Arc::get_mut(&mut st.rom).unwrap().shop.catalog = Some(dc_bute::ShopCatalog::from_doc(&doc));
    assert!(crate::shop::holds_pack_item(&st, 7) && !crate::shop::holds_pack_item(&st, 8));
    assert_eq!(crate::shop::view_list(&st), [8]);
    assert!(!crate::shop::is_shown(&st, 7) && crate::shop::is_shown(&st, 8));
    st.rules.shop.list_override[0] = Some(vec![7, 8]);
    assert_eq!(
        crate::shop::view_list(&st),
        [8],
        "not by an operator's list either: no sale yet"
    );
}

#[test]
fn the_set_document_reads_back_as_a_set_with_the_pushed_hash() {
    let dir = temp("doc");
    let pack_a = pack(&dir, "pack_a", "shared", &["content.weapons"], WEAPON_LUA);
    let st = featured(&dir, vec![pack_a]);
    let e: &EntrySet = &st.entry_mods[&3];
    let v = dc_mod_lua::JsonValue::parse(&e.doc).unwrap();
    assert_eq!(
        v.get("mod_set_hash").and_then(|p| p.as_str()),
        Some(e.expected.set.as_str())
    );
    assert_eq!(
        e.set.fragment().get("policy").and_then(|p| p.as_str()),
        Some("modded")
    );
    let mut h = crate::hosting::Hosting::none();
    h.entry_sets.insert(3, e.doc.clone());
    assert_eq!(
        crate::hosting::route(&h, "/entry_mods/3.json")
            .unwrap()
            .status,
        200
    );
    assert_eq!(
        crate::hosting::route(&h, "/entry_mods/1.json")
            .unwrap()
            .status,
        404
    );
    assert_eq!(
        crate::hosting::route(&h, "/entry_mods/x.json")
            .unwrap()
            .status,
        404
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The weapon content in the start request of a remote game host, and an agent too old for it.
#[test]
fn a_featured_entrys_content_goes_to_its_agent_and_an_old_agent_refuses_the_start() {
    let dir = temp("agent");
    let pack_a = pack(&dir, "pack_a", "shared", &["content.weapons"], WEAPON_LUA);
    let mut st = featured(&dir, vec![pack_a]);
    st.opts.servers[2].host = "us-west".into();
    st.opts.dserver = true;
    st.opts.dserver_world = "Worlds/Test".into();
    let (l, account) = on_entry(&mut st, 1, "erin", 3);
    let mut b = account.to_le_bytes().to_vec();
    b.extend(7u32.to_le_bytes());
    b.extend(wstr("Channel 7", 32));
    let got = l.send(&mut st, CHANNEL_ENTER, &b);
    assert_eq!(
        u32_at(
            &got.iter()
                .find(|(id, _)| *id == CHANNEL_ENTER_REPLY)
                .unwrap()
                .1,
            6
        ),
        1
    );
    let mut b = 8i32.to_le_bytes().to_vec();
    b.extend(wstr("Room", 33));
    b.extend(wstr("", 9));
    b.extend(0i32.to_le_bytes());
    let got = l.send(&mut st, ROOM_CREATE, &b);
    assert_eq!(
        u32_at(
            &got.iter()
                .find(|(id, _)| *id == ROOM_CREATE_REPLY)
                .unwrap()
                .1,
            6
        ),
        1
    );
    let start = |st: &mut Core, takes_content: bool| {
        let (tx, rx) = std::sync::mpsc::channel();
        st.hosts.online.insert(
            "us-west".into(),
            crate::hosts::Agent {
                conn: 1,
                tx,
                address: "203.0.113.9".into(),
                peer: "test".into(),
                ports: None,
                version: "t".into(),
                takes_content,
                takes_packs: false,
                packs: Default::default(),
                since: std::time::Instant::now(),
                takes_update: false,
                offered: None,
                sending: false,
                sends: 0,
            },
        );
        let mut flag = 0u32.to_le_bytes().to_vec();
        flag.extend(2u32.to_le_bytes());
        let got = l.send(st, ROOM_USER_FLAG, &flag);
        let ok = u32_at(
            &got.iter()
                .find(|(id, _)| *id == ROOM_USER_FLAG_REPLY)
                .unwrap()
                .1,
            6,
        ) == 1;
        let sent: Vec<String> = rx
            .try_iter()
            .filter_map(|o| match o {
                crate::hosts::HostOut::Line(t) => Some(t),
                _ => None,
            })
            .collect();
        (ok, sent)
    };
    let (ok, sent) = start(&mut st, false);
    assert!(!ok && sent.is_empty(), "{sent:?}");
    let (ok, sent) = start(&mut st, true);
    assert!(ok, "{sent:?}");
    let line = sent
        .iter()
        .find(|t| t.starts_with("start "))
        .expect("a start request");
    assert!(
        line.contains("content_hash") && line.contains("AmmoDamage"),
        "{line}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ------------------------------------------------------------------ community maps on the entry (MAP-80 .. MAP-84)

/// A map pack of ours as `<dir>/<sha256>.zip`: shared, content.maps + assets, one record on TDM / BOMB, TDM goal 80.
fn map_pack(dir: &Path, id: &str) -> dc_config::schema::ModRef {
    let src = crate::maps_tests::write_map_mod(&dir.join("src"), id, "shared", r#""TDM", "BOMB""#);
    let (bytes, e) = dc_modkit::pack_mod_bytes(&src).unwrap();
    std::fs::write(dir.join(format!("{}.zip", e.sha256)), &bytes).unwrap();
    dc_config::schema::ModRef {
        id: id.into(),
        version: "1.0.0".into(),
        sha256: e.sha256,
    }
}

/// [`featured`] over the synthetic room table: one ROM map, so the community rows start at 1 and have mode bits.
fn featured_with_table(dir: &Path, mods: Vec<dc_config::schema::ModRef>) -> Core {
    let mut st = state();
    Arc::get_mut(&mut st.rom).unwrap().game_room =
        dc_bute::GameRoomData::from_doc(dc_bute::ButeDoc::parse(crate::maps_tests::TABLE));
    st.opts.servers = entries(mods);
    let base = st.maps().first_community_index() as u32;
    st.entry_mods = build_entry_sets(
        &st.opts.servers,
        Some(dir),
        dir,
        1 << 24,
        base,
        &Arc::new(Logger::none()),
    );
    st
}

/// Enters channel `ch` and opens a room (mode 0: TDM of the table): its number.
fn room_on(l: &Client, st: &mut Core, account: u32, ch: u32) -> u32 {
    let mut b = account.to_le_bytes().to_vec();
    b.extend(ch.to_le_bytes());
    b.extend(wstr(&format!("Channel {ch}"), 32));
    let got = l.send(st, CHANNEL_ENTER, &b);
    assert_eq!(
        u32_at(
            &got.iter()
                .find(|(id, _)| *id == CHANNEL_ENTER_REPLY)
                .unwrap()
                .1,
            6
        ),
        1
    );
    let mut b = 8i32.to_le_bytes().to_vec();
    b.extend(wstr("Room", 33));
    b.extend(wstr("", 9));
    b.extend(0i32.to_le_bytes());
    let got = l.send(st, ROOM_CREATE, &b);
    let r = &got
        .iter()
        .find(|(id, _)| *id == ROOM_CREATE_REPLY)
        .unwrap()
        .1;
    assert_eq!(u32_at(r, 6), 1);
    u32_at(r, 0xe)
}

fn set_map(l: &Client, st: &mut Core, map: u32) -> bool {
    let mut b = 0u32.to_le_bytes().to_vec();
    b.extend(field::MAP.to_le_bytes());
    b.extend(map.to_le_bytes());
    let got = l.send(st, ROOM_SETTING_CHANGE, &b);
    u32_at(
        &got.iter()
            .find(|(id, _)| *id == ROOM_SETTING_CHANGE_REPLY)
            .unwrap()
            .1,
        6,
    ) == 1
}

fn settings(l: &Client, st: &mut Core) -> Vec<u8> {
    let got = l.send(st, ROOM_SETTINGS_FETCH, &[0u8; 8]);
    got.into_iter()
        .find(|(id, _)| *id == ROOM_SETTINGS_REPLY)
        .unwrap()
        .1
}

/// MAP-80: a map pack passes the curated limit. MAP-81: the set's document carries the base and the records.
#[test]
fn the_entry_document_lists_the_sets_maps_after_the_common_base() {
    let dir = temp("doc_maps");
    let m = map_pack(&dir, "valley");
    let st = featured_with_table(&dir, vec![m]);
    let e = &st.entry_mods[&3];
    assert_eq!(e.client.loaded, ["valley"]);
    let v = dc_mod_lua::JsonValue::parse(&e.doc).unwrap();
    assert_eq!(
        v.get("community_map_base").and_then(|p| p.as_i64()),
        Some(1)
    );
    let maps = v.get("maps").and_then(|p| p.as_array()).unwrap();
    assert_eq!(maps.len(), 1);
    assert_eq!(maps[0].get("id").and_then(|p| p.as_str()), Some("valley"));
    assert_eq!(
        maps[0].get("mod_id").and_then(|p| p.as_str()),
        Some("valley")
    );
    assert_eq!(
        maps[0].get("world").and_then(|p| p.as_str()),
        Some("@mod:valley/assets/maps/valley.dat")
    );
    assert_eq!(
        maps[0].get("round_seconds").and_then(|p| p.as_i64()),
        Some(300)
    );
    assert_eq!(
        maps[0]
            .get("modes")
            .and_then(|p| p.as_array())
            .map(|a| a.len()),
        Some(2)
    );
    // a set without maps says so
    let dir2 = temp("doc_nomaps");
    let pack_a = pack(&dir2, "pack_a", "shared", &["content.weapons"], WEAPON_LUA);
    let st2 = featured_with_table(&dir2, vec![pack_a]);
    let v = dc_mod_lua::JsonValue::parse(&st2.entry_mods[&3].doc).unwrap();
    assert_eq!(
        v.get("maps").and_then(|p| p.as_array()).map(|a| a.len()),
        Some(0)
    );
    assert_eq!(
        v.get("community_map_base").and_then(|p| p.as_i64()),
        Some(1)
    );
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&dir2).ok();
}

/// MAP-81: a curated set keeps its good records; one whose world file is missing is left out with a log line.
#[test]
fn a_curated_record_without_its_world_is_left_out_and_the_rest_stay() {
    let dir = temp("left_out");
    let src = crate::maps_tests::write_map_mod(&dir, "valley", "shared", r#""TDM""#);
    std::fs::write(
        src.join("main.lua"),
        r#"return function(mod)
    mod.content.maps:register({ id = "good", name = "good", world = mod.assets:path("assets/maps/valley.dat"), modes = { "TDM" } })
    mod.content.maps:register({ id = "gone", name = "gone", world = mod.assets:path("assets/maps/gone.dat"), modes = { "TDM" } })
end"#,
    )
    .unwrap();
    let logfile = dir.join("lobby.log");
    let log = Arc::new(Logger::open(Some(&logfile), None).0);
    let c = crate::modhost::ModClient::start_curated(
        vec![crate::modset::Resolved {
            id: "valley".into(),
            dir: src,
        }],
        log,
    )
    .unwrap();
    assert_eq!(c.loaded, ["valley"]);
    let keys: Vec<&str> = c.maps.iter().map(|m| m.key()).collect();
    assert_eq!(keys, ["good"]);
    let text = std::fs::read_to_string(&logfile).unwrap();
    assert!(
        text.contains("mods: curated map left out: map \"gone\""),
        "{text}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// MAP-82: the table belongs to the entry: its session sees the map in 0x311 under a version from its set hash, a
/// plain entry's session sees neither, and a room picks the index on the entry only.
#[test]
fn a_session_on_the_entry_sees_its_maps_and_a_plain_entrys_session_does_not() {
    let dir = temp("table");
    let m = map_pack(&dir, "valley");
    let mut st = featured_with_table(&dir, vec![m]);
    assert_eq!(st.maps_for(Some(3)).community_indices(), [(1, "valley")]);
    assert!(st.maps_for(Some(1)).community.is_empty(), "a plain entry");
    assert!(
        st.maps_for(None).community.is_empty(),
        "the lobby-wide table of a vanilla lobby"
    );
    assert!(st.maps_of_channel(7).is_community(1) && st.maps_of_channel(9).is_community(1));
    assert!(!st.maps_of_channel(1).is_community(1) && !st.maps_of_channel(4).is_community(1));
    assert!(!st.maps_of_channel(100).is_community(1), "the clan lobby");
    let hash = st.entry_mods[&3].expected.set.clone();
    let version = u32::from_str_radix(&hash[..8], 16).unwrap();

    let (on, account) = on_entry(&mut st, 1, "vera", 3);
    assert_eq!(
        crate::servers::map_entry_of_session(&st, on.sid),
        Some(3),
        "by its pick, before any channel"
    );
    let d = settings(&on, &mut st);
    assert_eq!(d.len(), 0x10a);
    assert_eq!(u32_at(&d, 0x12), version);
    assert_eq!(st.room_settings_version_for(Some(3)), version);
    assert_eq!(
        crate::maps_tests::pairs(&d),
        [(0, 0b111), (1, 0b011)],
        "the ROM row, then TDM | BOMB"
    );

    let (plain, plain_account) = on_entry(&mut st, 3, "paul", 1);
    assert_eq!(
        crate::servers::map_entry_of_session(&st, plain.sid),
        Some(1)
    );
    let d = settings(&plain, &mut st);
    assert_eq!(u32_at(&d, 0x12), crate::lobby::ROOM_SETTINGS_VERSION);
    assert_eq!(crate::maps_tests::pairs(&d), [(0, 0b111)]);

    // the room's channel decides: the index is the entry's, with the record's goal
    let no = room_on(&on, &mut st, account, 7);
    assert!(set_map(&on, &mut st, 1));
    let key = crate::room::RoomKey { channel: 7, no };
    assert_eq!((st.rooms[&key].map, st.rooms[&key].goal), (1, 80));
    assert_eq!(
        crate::servers::map_entry_of_session(&st, on.sid),
        Some(3),
        "by its channel"
    );
    room_on(&plain, &mut st, plain_account, 1);
    assert!(!set_map(&plain, &mut st, 1), "past the plain entry's rows");
    std::fs::remove_dir_all(&dir).ok();
}

/// A room of the featured entry, its host `us-west`, on the map: what a Start sends the agent.
struct RemoteRoom {
    st: Core,
    l: Client,
    sha: String,
    dir: PathBuf,
}

fn remote_room(tag: &str) -> RemoteRoom {
    let dir = temp(tag);
    let m = map_pack(&dir, "valley");
    let sha = m.sha256.clone();
    let mut st = featured_with_table(&dir, vec![m]);
    st.opts.servers[2].host = "us-west".into();
    st.opts.dserver = true;
    st.opts.dserver_world = "Worlds/Test".into();
    let (l, account) = on_entry(&mut st, 1, "rita", 3);
    room_on(&l, &mut st, account, 7);
    assert!(set_map(&l, &mut st, 1));
    RemoteRoom { st, l, sha, dir }
}

impl RemoteRoom {
    /// A Start with an agent of these abilities: the reply's verdict and the lines it got.
    fn start(&mut self, takes_packs: bool, holds: bool) -> (bool, Vec<String>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut packs = std::collections::BTreeSet::new();
        if holds {
            packs.insert(self.sha.clone());
        }
        self.st.hosts.online.insert(
            "us-west".into(),
            crate::hosts::Agent {
                conn: 1,
                tx,
                address: "203.0.113.9".into(),
                peer: "test".into(),
                ports: None,
                version: "t".into(),
                takes_content: true,
                takes_packs,
                packs,
                since: std::time::Instant::now(),
                takes_update: false,
                offered: None,
                sending: false,
                sends: 0,
            },
        );
        let mut flag = 0u32.to_le_bytes().to_vec();
        flag.extend(2u32.to_le_bytes());
        let got = self.l.send(&mut self.st, ROOM_USER_FLAG, &flag);
        let ok = u32_at(
            &got.iter()
                .find(|(id, _)| *id == ROOM_USER_FLAG_REPLY)
                .unwrap()
                .1,
            6,
        ) == 1;
        let sent = rx
            .try_iter()
            .filter_map(|o| match o {
                crate::hosts::HostOut::Line(t) => Some(t),
                _ => None,
            })
            .collect();
        (ok, sent)
    }
}

/// MAP-83 / MAP-84: the first Start sends the pack whole, then a `start` naming it; what the lobby sent is remembered.
#[test]
fn a_remote_start_on_the_map_sends_the_pack_and_then_the_request_that_names_it() {
    let mut r = remote_room("remote_pack");
    let (ok, sent) = r.start(true, false);
    assert!(ok, "{sent:?}");
    let zip = std::fs::read(r.dir.join(format!("{}.zip", r.sha))).unwrap();
    let want: Vec<String> = zip
        .chunks(crate::game::PACK_CHUNK)
        .map(|c| format!("pack {} {}", r.sha, crate::identity::b64std(c)))
        .collect();
    let at = sent
        .iter()
        .position(|t| t.starts_with("start "))
        .expect("a start request");
    assert_eq!(sent[..at], want[..], "the whole pack, before the start");
    let json = sent[at].splitn(3, ' ').nth(2).unwrap();
    let v = dc_mod_lua::JsonValue::parse(json).unwrap();
    let pack = v.get("world_pack").unwrap();
    assert_eq!(
        pack.get("sha256").and_then(|p| p.as_str()),
        Some(r.sha.as_str())
    );
    assert_eq!(
        pack.get("size").and_then(|p| p.as_i64()),
        Some(zip.len() as i64)
    );
    assert_eq!(
        pack.get("url").and_then(|p| p.as_str()),
        Some(format!("/mods/{}.zip", r.sha).as_str())
    );
    assert_eq!(
        v.get("world_file").and_then(|p| p.as_str()),
        Some("assets/maps/valley.dat")
    );
    let announce = "@mod:valley/assets/maps/valley.dat";
    assert_eq!(v.get("announce").and_then(|p| p.as_str()), Some(announce));
    assert_eq!(
        v.get("world").and_then(|p| p.as_str()),
        Some(announce),
        "never this machine's path"
    );
    assert!(r.st.hosts.online["us-west"].packs.contains(&r.sha));
    assert!(
        sent[at + 1..]
            .iter()
            .any(|t| t.contains(" room ") && t.contains("time=300")),
        "the room line follows, with the record's time: {:?}",
        &sent[at + 1..]
    );
    std::fs::remove_dir_all(&r.dir).ok();
}

/// MAP-84: a pack the agent holds is not sent again.
#[test]
fn a_remote_start_does_not_send_a_pack_the_agent_holds() {
    let mut r = remote_room("remote_held");
    let (ok, sent) = r.start(true, true);
    assert!(ok, "{sent:?}");
    assert!(!sent.iter().any(|t| t.starts_with("pack ")), "{sent:?}");
    assert!(
        sent.iter()
            .any(|t| t.starts_with("start ") && t.contains("world_pack")),
        "{sent:?}"
    );
    std::fs::remove_dir_all(&r.dir).ok();
}

/// MAP-83: an agent whose hello lacks `packs=1` is refused with nothing sent; the room stays waiting.
#[test]
fn a_host_without_packs_is_refused_a_community_room() {
    let mut r = remote_room("remote_old");
    let logfile = r.dir.join("lobby.log");
    r.st.log = Arc::new(Logger::open(Some(&logfile), None).0);
    let (ok, sent) = r.start(false, false);
    assert!(!ok && sent.is_empty(), "{sent:?}");
    let text = std::fs::read_to_string(&logfile).unwrap();
    assert!(
        text.contains("game host us-west is too old for this entry's community maps (no packs=1"),
        "{text}"
    );
    let room = r.st.rooms.values().next().unwrap();
    assert_eq!(room.state, crate::room::RoomPhase::Waiting);
    assert!(room.game.is_none());
    std::fs::remove_dir_all(&r.dir).ok();
}

/// A ROM map on the same entry is what it was: no pack, no pack fields, any agent.
#[test]
fn a_rom_map_on_the_entry_asks_nothing_of_the_agent() {
    let mut r = remote_room("remote_rom");
    assert!(set_map(&r.l, &mut r.st, 0));
    let (ok, sent) = r.start(false, false);
    assert!(ok, "{sent:?}");
    assert!(!sent.iter().any(|t| t.starts_with("pack ")));
    let start = sent.iter().find(|t| t.starts_with("start ")).unwrap();
    assert!(
        !start.contains("world_pack") && !start.contains("announce"),
        "{start}"
    );
    std::fs::remove_dir_all(&r.dir).ok();
}

/// MAP-84: the lines stay under the agent's 16 KiB limit, each piece decodes alone, and a changed zip is refused.
#[test]
fn pack_lines_fit_the_line_limit_and_a_changed_zip_is_refused() {
    let dir = temp("lines");
    let zip = dir.join("x.zip");
    let bytes: Vec<u8> = (0..30_001u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&zip, &bytes).unwrap();
    let sha = dc_rom::sha256_hex(&bytes);
    let w = crate::game::RemoteWorld {
        sha256: sha.clone(),
        size: bytes.len() as u64,
        zip: zip.clone(),
        world_file: "assets/maps/x.dat".into(),
        announce: "@mod:x/assets/maps/x.dat".into(),
    };
    let lines = crate::game::pack_lines(&w).unwrap();
    assert_eq!(lines.len(), 3);
    assert!(crate::game::PACK_CHUNK.is_multiple_of(3));
    for l in &lines {
        assert!(l.len() <= crate::hosts::MAX_LINE, "{}", l.len());
        assert!(l.starts_with(&format!("pack {sha} ")));
    }
    assert!(!lines[0].ends_with('='), "only the last piece pads");
    std::fs::write(&zip, b"other").unwrap();
    assert!(crate::game::pack_lines(&w)
        .unwrap_err()
        .contains("no longer hashes"));
    std::fs::remove_dir_all(&dir).ok();
}
