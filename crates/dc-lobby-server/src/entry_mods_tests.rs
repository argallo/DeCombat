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
        client_visual_allowed: false,
        client_fov: None,
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

/// LG-403 fixture: [`featured`] with the test kit's weapons typed (101 primary, 102 secondary, 103 melee, 104
/// throwable), pack gun 900 a primary, and the starter backpack 7001 holding two weapon slots.
fn featured_with_backpack(dir: &Path, mods: Vec<dc_config::schema::ModRef>) -> Core {
    let mut st = featured(dir, mods);
    let rom = Arc::get_mut(&mut st.rom).unwrap();
    rom.items = Some(dc_bute::ItemsTable::from_doc(dc_bute::ButeDoc::parse(
        "[BackpackItem0]\nItemID=1\nMultyInvenSlot=2\n",
    )));
    for (no, kind) in [(101, 1), (102, 4), (103, 7), (104, 6), (900, 1)] {
        rom.weapon_kinds.insert(no, kind);
    }
    st
}

/// The 0x323 view of a session: item number per equipment slot (-1 empty).
fn seen_slots(st: &mut Core, l: &Client) -> Vec<i32> {
    let got = l.send(st, EQUIPMENT, &[]);
    let r = &got.iter().find(|(id, _)| *id == EQUIPMENT_REPLY).unwrap().1;
    (0..15).map(|s| u32_at(r, 0x12 + 4 * s) as i32).collect()
}

/// 0x324 (equip) or 0x326 (unequip) as the client sends it: the reply's ok word.
fn slot_request(st: &mut Core, l: &Client, id: u16, uid: i64, no: u32, slot: i32) -> bool {
    let mut b = (uid as u32).to_le_bytes().to_vec();
    b.extend(no.to_le_bytes());
    b.extend(slot.to_le_bytes());
    let reply = if id == crate::shopmsg::EQUIP {
        crate::shopmsg::EQUIP_REPLY
    } else {
        crate::shopmsg::UNEQUIP_REPLY
    };
    let got = l.send(st, id, &b);
    u32_at(&got.iter().find(|(i, _)| *i == reply).unwrap().1, 6) == 1
}

/// User report 2026-11-04: on Vanilla++ the default primary went into a backpack slot and a pack gun into the primary
/// slot; on the vanilla entry the primary was empty and the default stuck in the backpack (Equip on it sends 0x326,
/// refused for a default). LG-403: the default fills the primary there, and leaves the backpack in that view.
#[test]
fn a_default_primary_in_the_backpack_fills_the_primary_off_vanilla_plus_and_can_leave_the_backpack()
{
    let dir = temp("default_pack");
    let pack_a = pack(&dir, "pack_a", "shared", &["content.weapons"], WEAPON_LUA);
    let mut st = featured_with_backpack(&dir, vec![pack_a]);
    let (l, account) = on_entry(&mut st, 1, "erin", 3);
    let default = db::items(&st.db, account)
        .unwrap()
        .into_iter()
        .find(|i| i.slot == 0)
        .unwrap();
    assert_eq!((default.item_no, default.default_type), (101, 1));
    // on Vanilla++: the default primary goes into backpack slot 10, then the pack gun into the primary slot
    let gun = add(&st, account, 900, -1, 0);
    assert!(
        !slot_request(&mut st, &l, crate::shopmsg::EQUIP, default.uid, 101, 10),
        "the default holding the primary slot alone does not go into the backpack"
    );
    assert!(slot_request(
        &mut st,
        &l,
        crate::shopmsg::EQUIP,
        gun,
        900,
        0
    ));
    assert_eq!(db::item(&st.db, default.uid).unwrap().unwrap().slot, -1);
    assert!(slot_request(
        &mut st,
        &l,
        crate::shopmsg::EQUIP,
        default.uid,
        101,
        10
    ));
    let s = seen_slots(&mut st, &l);
    assert_eq!((s[0], s[10]), (900, 101), "Vanilla++ shows what is stored");
    // the vanilla entry: the pack gun is hidden, the default fills the primary and the backpack slot is free
    st.sessions.get_mut(&l.sid).unwrap().server = 1;
    let s = seen_slots(&mut st, &l);
    assert_eq!((s[0], s[10]), (101, -1), "{s:?}");
    let t = crate::game::loadout_tokens(&st, account, &st.character(account).unwrap());
    assert!(t.contains("0:101") && !t.contains("10:"), "{t}");
    // there it is the primary: Equip on it (0x326 from slot 0) is refused as for any default in its own slot
    assert!(!slot_request(
        &mut st,
        &l,
        crate::shopmsg::UNEQUIP,
        default.uid,
        101,
        0
    ));
    assert!(
        !slot_request(&mut st, &l, crate::shopmsg::EQUIP, default.uid, 101, 11),
        "nor does it leave the primary it fills for the backpack"
    );
    // the stored loadout is untouched: Vanilla++ shows the pack gun and the default in the backpack again
    st.sessions.get_mut(&l.sid).unwrap().server = 3;
    let s = seen_slots(&mut st, &l);
    assert_eq!((s[0], s[10]), (900, 101));
    // a default in a backpack slot can leave it (Equip on it, or the row's X, sends 0x326): to storage while the
    // primary holds another gun, and from there it equips into the primary slot
    assert!(slot_request(
        &mut st,
        &l,
        crate::shopmsg::UNEQUIP,
        default.uid,
        101,
        10
    ));
    let s = seen_slots(&mut st, &l);
    assert_eq!((s[0], s[10]), (900, -1));
    assert!(slot_request(
        &mut st,
        &l,
        crate::shopmsg::EQUIP,
        default.uid,
        101,
        0
    ));
    let s = seen_slots(&mut st, &l);
    assert_eq!((s[0], s[10]), (101, -1));
    std::fs::remove_dir_all(&dir).ok();
}

/// LG-403 the reverse way and the stored repair: a default moved into the backpack on the vanilla entry (pack gun hidden
/// in the primary slot), and a primary slot stored empty beside a default in the backpack (a loadout made before the
/// rule), which the next equipment read repairs on any entry.
#[test]
fn the_primary_is_never_empty_on_either_entry_and_an_old_empty_primary_is_repaired() {
    let dir = temp("default_pack_back");
    let pack_a = pack(&dir, "pack_a", "shared", &["content.weapons"], WEAPON_LUA);
    let mut st = featured_with_backpack(&dir, vec![pack_a]);
    let (l, account) = on_entry(&mut st, 1, "fern", 1);
    let default = db::items(&st.db, account)
        .unwrap()
        .into_iter()
        .find(|i| i.slot == 0)
        .unwrap();
    // stored on Vanilla++ earlier: the pack gun is the primary, the default in storage
    db::set_slot(&st.db, default.uid, -1).unwrap();
    add(&st, account, 900, 0, 0);
    // a stored loadout of the report: pack gun in the primary, default in backpack slot 10, seen from vanilla
    db::set_slot(&st.db, default.uid, 10).unwrap();
    let s = seen_slots(&mut st, &l);
    assert_eq!((s[0], s[10]), (101, -1), "{s:?}");
    // vanilla: the default fills the primary; it cannot be moved into the backpack from there
    db::set_slot(&st.db, default.uid, -1).unwrap();
    assert_eq!(seen_slots(&mut st, &l)[0], 101);
    assert!(!slot_request(
        &mut st,
        &l,
        crate::shopmsg::EQUIP,
        default.uid,
        101,
        10
    ));
    db::set_slot(&st.db, default.uid, 10).unwrap();
    // and on Vanilla++ it is what is stored, with the default free to leave the backpack
    st.sessions.get_mut(&l.sid).unwrap().server = 3;
    let s = seen_slots(&mut st, &l);
    assert_eq!((s[0], s[10]), (900, 101));
    // a primary slot stored empty with the default in the backpack (no pack gun at all): the read puts it back
    st.db
        .execute("DELETE FROM items WHERE item_no = 900", [])
        .unwrap();
    db::set_slot(&st.db, default.uid, 11).unwrap();
    for entry in [3, 1] {
        st.sessions.get_mut(&l.sid).unwrap().server = entry;
        let s = seen_slots(&mut st, &l);
        assert_eq!((s[0], s[10], s[11]), (101, -1, -1), "entry {entry}: {s:?}");
        assert_eq!(db::item(&st.db, default.uid).unwrap().unwrap().slot, 0);
        db::set_slot(&st.db, default.uid, 11).unwrap();
    }
    // another primary in the primary slot: the default may stay in the backpack, and leaves it to storage
    let other = add(&st, account, 101, 0, 0);
    let s = seen_slots(&mut st, &l);
    assert_eq!((s[0], s[11]), (101, 101));
    assert_eq!(db::item(&st.db, other).unwrap().unwrap().slot, 0);
    assert!(slot_request(
        &mut st,
        &l,
        crate::shopmsg::UNEQUIP,
        default.uid,
        101,
        11
    ));
    assert_eq!(db::item(&st.db, default.uid).unwrap().unwrap().slot, -1);
    // with the primary emptied by moving that gun into the backpack, the default returns to the primary slot
    assert!(slot_request(
        &mut st,
        &l,
        crate::shopmsg::EQUIP,
        other,
        101,
        10
    ));
    let s = seen_slots(&mut st, &l);
    assert_eq!((s[0], s[10]), (101, 101));
    assert_eq!(db::item(&st.db, default.uid).unwrap().unwrap().slot, 0);
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

/// LG-426: an entry with `client_visual_allowed` says so in its document's fragment; the set hash and the 0x3f10 row do
/// not change, and an entry without the key keeps saying false.
#[test]
fn an_entry_that_allows_local_mods_says_so_in_its_document_and_keeps_its_hash() {
    let dir = temp("cv");
    let pack_a = pack(&dir, "pack_a", "shared", &["content.weapons"], WEAPON_LUA);
    let flag = |doc: &str| {
        dc_mod_lua::JsonValue::parse(doc)
            .unwrap()
            .get("mod_set")
            .and_then(|m| m.get("client_visual_allowed"))
            .and_then(|b| b.as_bool())
    };
    let plain = featured(&dir, vec![pack_a.clone()]);
    let off = &plain.entry_mods[&3];
    assert_eq!(flag(&off.doc), Some(false));
    let mut servers = entries(vec![pack_a]);
    servers[2].client_visual_allowed = true;
    let sets = build_entry_sets(
        &servers,
        Some(&dir),
        &dir,
        1 << 24,
        12,
        &Arc::new(Logger::none()),
    );
    let on = &sets[&3];
    assert_eq!(flag(&on.doc), Some(true));
    assert_eq!(
        on.set.fragment().get("policy").and_then(|p| p.as_str()),
        Some("modded"),
        "an older client reads it as before and refuses local mods under its curated policy"
    );
    assert_eq!(on.hashes(), off.hashes(), "no part of the claim");
    std::fs::remove_dir_all(&dir).ok();
}

/// LG-429: a curated entry publishes its field-of-view range in its document: the default without a key, its own with
/// one (also a locked `[90, 90]`); the set hash and the 0x3f10 row do not change.
#[test]
fn a_curated_entry_publishes_its_field_of_view_range_in_its_document() {
    let dir = temp("fov");
    let pack_a = pack(&dir, "pack_a", "shared", &["content.weapons"], WEAPON_LUA);
    let range = |doc: &str| {
        dc_mod_lua::JsonValue::parse(doc)
            .unwrap()
            .get("mod_set")
            .and_then(|m| m.get("client_fov"))
            .map(|v| v.to_json_string())
    };
    let plain = featured(&dir, vec![pack_a.clone()]);
    assert_eq!(
        range(&plain.entry_mods[&3].doc).as_deref(),
        Some("[70,120]")
    );
    let mut servers = entries(vec![pack_a]);
    servers[2].client_fov = Some([80, 110]);
    let sets = build_entry_sets(
        &servers,
        Some(&dir),
        &dir,
        1 << 24,
        12,
        &Arc::new(Logger::none()),
    );
    assert_eq!(range(&sets[&3].doc).as_deref(), Some("[80,110]"));
    assert_eq!(
        sets[&3].hashes(),
        plain.entry_mods[&3].hashes(),
        "no part of the claim"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A room of the featured entry 3 on game host `us-west`, in a state whose pack runs `lua`: what a Start sends.
fn weapon_room(tag: &str, lua: &str) -> (Core, Client, PathBuf) {
    let dir = temp(tag);
    let pack_a = pack(&dir, "pack_a", "shared", &["content.weapons"], lua);
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
    (st, l, dir)
}

/// What an agent's hello offers, and what it holds already.
#[derive(Clone, Copy)]
struct Abilities {
    content: bool,
    content_lines: bool,
    holds_content: bool,
}

/// A Start with an agent of these abilities: the reply's verdict and the lines it got.
fn weapon_start(st: &mut Core, l: &Client, h: Abilities) -> (bool, Vec<String>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut packs = std::collections::BTreeSet::new();
    if h.holds_content {
        let hash = crate::servers::entry_set_of_channel(st, 7)
            .map(|e| e.client.content.hash())
            .unwrap();
        packs.insert(hash);
    }
    st.hosts.online.insert(
        "us-west".into(),
        crate::hosts::Agent {
            conn: 1,
            tx,
            address: "203.0.113.9".into(),
            peer: "test".into(),
            ports: None,
            version: "t".into(),
            takes_content: h.content,
            takes_content_lines: h.content_lines,
            takes_packs: false,
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
}

const OLD: Abilities = Abilities {
    content: true,
    content_lines: false,
    holds_content: false,
};
const NEW: Abilities = Abilities {
    content: true,
    content_lines: true,
    holds_content: false,
};

/// The weapon content in the start request of a remote game host, and an agent too old for it.
#[test]
fn a_featured_entrys_content_goes_to_its_agent_and_an_old_agent_refuses_the_start() {
    let (mut st, l, dir) = weapon_room("agent", WEAPON_LUA);
    let none = Abilities {
        content: false,
        ..OLD
    };
    let (ok, sent) = weapon_start(&mut st, &l, none);
    assert!(!ok && sent.is_empty(), "{sent:?}");
    // an agent that takes content but not content lines keeps the inline request when it fits
    let (ok, sent) = weapon_start(&mut st, &l, OLD);
    assert!(ok, "{sent:?}");
    let line = sent
        .iter()
        .find(|t| t.starts_with("start "))
        .expect("a start request");
    assert!(
        line.contains("content_hash") && line.contains("AmmoDamage"),
        "{line}"
    );
    assert!(!sent.iter().any(|t| t.starts_with("content ")), "{sent:?}");
    std::fs::remove_dir_all(&dir).ok();
}

/// A pack that patches enough weapons for its content document to pass one `start` line (LG-431 is for this).
const BIG_LUA: &str = "return function(mod) for id = 100, 400 do mod.content.weapons:patch(id, {AmmoDamage = 41}) end end";

/// LG-431: content over 16 KiB goes in `content` lines, in order, before a `start` that names only its hash; an agent
/// without `contentlines=1` gets the error and nothing is sent.
#[test]
fn a_big_content_goes_in_lines_and_the_start_carries_the_hash_only() {
    let (mut st, l, dir) = weapon_room("bigcontent", BIG_LUA);
    let logfile = dir.join("lobby.log");
    st.log = Arc::new(Logger::open(Some(&logfile), None).0);
    let (hash, doc) = {
        let c = &crate::servers::entry_set_of_channel(&st, 7)
            .unwrap()
            .client
            .content;
        (c.hash(), c.to_json())
    };
    assert!(
        doc.len() > 16 * 1024,
        "the fixture must be big: {}",
        doc.len()
    );
    // an agent without the lines: the error, and nothing sent
    let (ok, sent) = weapon_start(&mut st, &l, OLD);
    assert!(!ok && sent.is_empty(), "{sent:?}");
    let text = std::fs::read_to_string(&logfile).unwrap();
    assert!(
        text.contains("too large for a start request") && text.contains("contentlines=1"),
        "{text}"
    );
    assert_eq!(
        st.rooms.values().next().unwrap().state,
        crate::room::RoomPhase::Waiting
    );
    // an agent with them: the whole document in order, then the start with the hash alone
    let (ok, sent) = weapon_start(&mut st, &l, NEW);
    assert!(ok, "{sent:?}");
    let at = sent
        .iter()
        .position(|t| t.starts_with("start "))
        .expect("a start request");
    assert!(at >= 2, "the document needs more than one line: {at}");
    assert_eq!(sent[..at], crate::game::content_lines(&hash, &doc)[..]);
    for t in &sent[..at] {
        assert!(t.len() <= crate::hosts::MAX_LINE, "{}", t.len());
    }
    let json = sent[at].splitn(3, ' ').nth(2).unwrap();
    assert!(json.len() < 1024, "{}", json.len());
    let v = dc_mod_lua::JsonValue::parse(json).unwrap();
    assert_eq!(
        v.get("content_hash").and_then(|p| p.as_str()),
        Some(hash.as_str())
    );
    assert!(v.get("content").is_none(), "the hash alone");
    assert!(st.hosts.online["us-west"].packs.contains(&hash));
    std::fs::remove_dir_all(&dir).ok();
}

/// LG-431: a document the agent said `have` for is not sent again, and the start still names its hash.
#[test]
fn content_the_agent_holds_is_not_sent_again() {
    let (mut st, l, dir) = weapon_room("heldcontent", BIG_LUA);
    let held = Abilities {
        holds_content: true,
        ..NEW
    };
    let (ok, sent) = weapon_start(&mut st, &l, held);
    assert!(ok, "{sent:?}");
    assert!(!sent.iter().any(|t| t.starts_with("content ")), "{sent:?}");
    let start = sent.iter().find(|t| t.starts_with("start ")).unwrap();
    assert!(
        start.contains("content_hash") && !start.contains("\"content\""),
        "{start}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// LG-431: the content lines split a document at the pack chunk size, each decoding alone, the last one short.
#[test]
fn content_lines_split_at_the_chunk_size() {
    let hash = "ab".repeat(32);
    let doc = "x".repeat(crate::game::PACK_CHUNK * 2 + 5);
    let lines = crate::game::content_lines(&hash, &doc);
    assert_eq!(lines.len(), 3);
    let first = crate::identity::b64std(&doc.as_bytes()[..crate::game::PACK_CHUNK]);
    assert_eq!(lines[0], format!("content {hash} {first}"));
    assert_eq!(lines[1], lines[0], "equal bytes, equal line");
    assert_eq!(
        lines[2],
        format!("content {hash} {}", crate::identity::b64std(b"xxxxx"))
    );
    assert!(lines.iter().all(|l| l.len() <= crate::hosts::MAX_LINE));
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
                takes_content_lines: false,
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

/// CRATE-3 / CRATE-4 / CRATE-6 / CRATE-10: an `@crate` line of a game gives the player one inbox row of the crate; opening
/// it on the featured entry with two camos (a Common of row 25 and a Legendary of row 27) gives either a camo or the plain
/// row 25 / 27, each for the days of the HiSec table, a repeat extending the row it has (the odds pick which, see
/// `dc_bute::camo_crate::draw`). On a plain entry the row stays and the reply is ok 0.
#[test]
fn a_camo_crate_drops_into_the_inbox_and_opens_into_the_entrys_camos() {
    use crate::shopmsg::{INBOX_OPEN, INBOX_OPEN_REPLY, INBOX_STALE_PUSH};
    let dir = temp("crate");
    let camos = pack(
        &dir,
        "camos",
        "shared",
        &["content.weapons"],
        r#"return function(mod)
            mod.content.weapons:register({id = "Weapon502", variant_of = 25, variant_name = "Synth A"})
            mod.content.weapons:register({id = "Weapon503", variant_of = 27, variant_name = "Synth B", variant_rarity = "legendary"})
        end"#,
    );
    let mut st = featured(&dir, vec![camos]);
    let mut cat = dc_bute::ShopCatalog::from_doc(&dc_bute::ButeDoc::parse(""));
    cat.add_camo_crate();
    Arc::get_mut(&mut st.rom).unwrap().shop.catalog = Some(cat);
    let (l, account) = on_entry(&mut st, 1, "erin", 1);
    st.games.insert(
        5,
        crate::game::Game {
            id: 5,
            room: crate::room::RoomKey {
                channel: 7,
                no: 905,
            },
            port: 0,
            map: 0,
            mode: 0,
            link: Default::default(),
            ready: true,
            over: false,
            players: Vec::new(),
            clients: [("0".to_string(), account)].into_iter().collect(),
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
    let rows = |st: &Core| -> Vec<(i64, i64)> {
        let mut q = st
            .db
            .prepare("SELECT uproduct, product_id FROM inbox WHERE user_no = ?1 ORDER BY uproduct")
            .unwrap();
        q.query_map([account as i64], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let feed = |st: &mut Core, line: &str| {
        st.scope(|st| crate::game::on_line(st, 5, line)).unwrap();
    };
    for key in [0x4000_0001u32, 0x4000_0002, 0x4000_0003] {
        feed(
            &mut st,
            &format!("@crate uid={account} kind=camo key={key} name=erin"),
        );
        // the same key again delivers nothing
        feed(
            &mut st,
            &format!("@crate uid={account} kind=camo key={key} name=erin"),
        );
    }
    let got = rows(&st);
    assert_eq!(
        got.iter().map(|r| r.1).collect::<Vec<_>>(),
        vec![i64::from(dc_bute::camo_crate::PRODUCT); 3]
    );
    assert!(drain(&l.rx).iter().any(|(id, _)| *id == INBOX_STALE_PUSH));
    let open = |st: &mut Core, entry: i64| -> Vec<u8> {
        let mut b = (entry as u32).to_le_bytes().to_vec();
        b.extend((dc_bute::camo_crate::PRODUCT as u32).to_le_bytes());
        l.send(st, INBOX_OPEN, &b)
            .into_iter()
            .find(|(id, _)| *id == INBOX_OPEN_REPLY)
            .unwrap()
            .1
    };
    let won_items = |st: &Core, rows: std::ops::RangeInclusive<i64>| -> Vec<(i64, i64)> {
        let mut v: Vec<(i64, i64)> = db::items(&st.db, account)
            .unwrap()
            .into_iter()
            .filter(|i| rows.contains(&i.item_no))
            .map(|i| (i.item_no, i.end_time))
            .collect();
        v.sort_unstable();
        v
    };
    // a plain entry: refused, the row stays
    let r = open(&mut st, got[0].0);
    assert_eq!(u32_at(&r, 6), 0);
    assert_eq!(rows(&st).len(), 3);
    // the featured entry: each opening a camo or a plain gun for the HiSec days (7, 30 or permanent)
    st.sessions.get_mut(&l.sid).unwrap().server = 3;
    let mut camos = Vec::new();
    for (k, row) in got.iter().enumerate() {
        let r = open(&mut st, row.0);
        assert_eq!(u32_at(&r, 6), 1, "opening {k}");
        assert_eq!(u32_at(&r, 0x1e), 0, "no key");
        let (item, days) = (u32_at(&r, 0x12), u32_at(&r, 0x1a));
        assert!([0, 7, 30].contains(&days), "{days} days");
        match item {
            502 | 503 => camos.push(i64::from(item)),
            25 | 27 => {}
            n => panic!("item {n} is not in the crate"),
        }
    }
    camos.sort_unstable();
    camos.dedup();
    let held: Vec<i64> = won_items(&st, 502..=503).iter().map(|i| i.0).collect();
    assert_eq!(held, camos, "one row per camo, a repeat extends it");
    assert!(
        won_items(&st, 25..=27).len() <= 2,
        "a plain gun again is extended"
    );
    {
        use dc_bute::camo_crate::{Content, Tier};
        use dc_bute::modpatch::Rarity;
        let c = |item, tier| Content { item, tier };
        assert_eq!(
            crate::camo_crate::pool(&st, 3),
            vec![
                c(25, Tier::Regular),
                c(27, Tier::Regular),
                c(502, Tier::Camo(Rarity::Common)),
                c(503, Tier::Camo(Rarity::Legendary)),
            ]
        );
    }
    assert!(rows(&st).is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

/// A pack that registers camo variants: 500 of the SMG 101, 501 of the pistol 102 (cosmetic keys only, VAR-2).
const VARIANT_LUA: &str = "return function(mod) \
     mod.content.weapons:register({id = \"Weapon500\", variant_of = 101, variant_name = \"Camo SMG\"}) \
     mod.content.weapons:register({id = \"Weapon501\", variant_of = 102, variant_name = \"Camo Pistol\"}) end";

/// DSV-610 / LG-419 (OWNER REPORT 2026-11-05 "no backpack option ... still lets you bring your backpack item, same
/// with pistols only"): a camo variant has no item row of its own, so the weapon rules once let it through; now it
/// counts as its base gun. Under Pistols Only the camo SMG in the primary slot stays home and the camo pistol in the
/// backpack comes along; under No Backpack the camo pistol stays home; Sniper Only keeps the knife alone.
#[test]
fn a_camo_variant_counts_as_its_base_gun_under_a_weapon_rule() {
    let dir = temp("variant_rule");
    let pack_v = pack(&dir, "pack_v", "shared", &["content.weapons"], VARIANT_LUA);
    let mut st = featured_with_backpack(&dir, vec![pack_v]);
    assert_eq!(crate::inventory::base_weapon(&st, 500), 101);
    assert_eq!(crate::game::weapon_type(&st, 500), Some(1));
    assert_eq!(crate::game::weapon_type(&st, 501), Some(4));
    assert_eq!(crate::game::weapon_type(&st, 555), None, "no such variant");
    let (_l, account) = on_entry(&mut st, 1, "vic", 1);
    for i in db::items(&st.db, account).unwrap() {
        db::set_slot(&st.db, i.uid, -1).unwrap();
    }
    for (item, slot) in [(500, 0), (103, 2), (501, 10)] {
        db::add_item(
            &st.db,
            &NewItem {
                user_no: account,
                item_no: item,
                start_time: 0,
                end_time: 0,
                slot,
                default_type: 0,
            },
        )
        .unwrap();
    }
    let items = db::items(&st.db, account).unwrap();
    let slots = |limit: u32| -> Vec<(i64, i64)> {
        let items = crate::game::match_items(&st, limit, &items);
        let mut v: Vec<(i64, i64)> = crate::game::rule_items(&st, limit, items)
            .iter()
            .filter(|i| i.slot >= 0)
            .map(|i| (i.slot, i.item_no))
            .collect();
        v.sort();
        v
    };
    assert_eq!(slots(0), [(0, 500), (2, 103), (10, 501)]);
    assert_eq!(slots(dc_bute::arms::PISTOLS_ONLY), [(2, 103), (10, 501)]);
    assert_eq!(slots(dc_bute::arms::NO_BACKPACK), [(0, 500), (2, 103)]);
    assert_eq!(slots(dc_bute::arms::SNIPER_ONLY), [(2, 103)]);
    assert_eq!(
        slots(dc_bute::arms::NO_SNIPERS),
        [(0, 500), (2, 103), (10, 501)]
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// OPS-13: the monitor names a match's map from the table of the room's channel, as the lobby started it: on a curated
/// entry an added map has its own name, on a plain entry the same index is not that map (it was the lobby-wide table
/// before, which has no added map on a vanilla lobby, so the page showed the ROM's fallback world).
#[test]
fn the_monitor_names_an_added_map_from_the_entrys_table() {
    let dir = temp("monitor_name");
    let m = map_pack(&dir, "valley");
    let mut st = featured_with_table(&dir, vec![m]);
    Arc::get_mut(&mut st.rom).unwrap().game_room =
        dc_bute::GameRoomData::from_doc(dc_bute::ButeDoc::parse(
            &crate::maps_tests::TABLE.replacen("[Map0]", "[Map0]\nKey=SnowValley", 1),
        ));
    let room = |channel| crate::room::RoomKey { channel, no: 1 };
    let (world, name) = crate::opsmon::room_map(&st, room(7), 1, 0);
    assert_eq!(name, "fixture valley");
    assert!(world.ends_with("valley.dat"), "{world}");
    // the ROM map, and the same index on the plain entry (no such map there): the ROM's own key / world
    let (_rom_world, rom_name) = crate::opsmon::room_map(&st, room(1), 0, 0);
    assert_eq!(rom_name, "SnowValley");
    assert_ne!(rom_name, "fixture valley");
    let (_, plain) = crate::opsmon::room_map(&st, room(1), 1, 0);
    assert_ne!(plain, "fixture valley");
    // the live view and the stored row use the same name
    assert_eq!(st.maps_of_channel(7).name_of(1, &world), "fixture valley");
    std::fs::remove_dir_all(&dir).ok();
}

/// OPS-14: `GET /ops/guns` lists the camo variants of the loaded sets with their own names, in their base gun's slot.
#[test]
fn the_gun_list_names_the_camo_variants_of_a_curated_set() {
    let dir = temp("guns_list");
    let pack_v = pack(&dir, "pack_v", "shared", &["content.weapons"], VARIANT_LUA);
    let st = featured_with_backpack(&dir, vec![pack_v]);
    let v = crate::opsmon::guns_json(&st);
    let rows: Vec<(i64, String, i64)> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            (
                g.get("id").and_then(|x| x.as_i64()).unwrap(),
                g.get("name").and_then(|x| x.as_str()).unwrap().to_string(),
                g.get("slot").and_then(|x| x.as_i64()).unwrap(),
            )
        })
        .collect();
    assert!(rows.contains(&(500, "Camo SMG".to_string(), 0)), "{rows:?}");
    assert!(
        rows.contains(&(501, "Camo Pistol".to_string(), 1)),
        "{rows:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
