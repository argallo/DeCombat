//! Unit tests of the clan layouts, rules and flows (synthetic DB and sessions; ROM-gated parts skip without CA_ROM).

use super::*;
use crate::db;
use crate::net::route;
use crate::state::testing::*;
use crate::state::{Outgoing, Sid};
use std::sync::mpsc::Receiver;

fn frame(f: &[u8]) -> (u16, Vec<u8>) {
    let (frames, _) = dc_wire::parse_tcp_frames(f).unwrap();
    let (id, p) = frames.into_iter().next().unwrap();
    (id as u16, p)
}

fn wstr_at(p: &[u8], o: usize, n: usize) -> String {
    PktReader::new(p).at(o).wstr(n)
}

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
        self.pump();
    }

    fn pump(&mut self) {
        self.got.extend(drain(&self.rx));
    }

    fn ids(&mut self) -> Vec<u16> {
        self.pump();
        self.got.iter().map(|(i, _)| *i).collect()
    }

    fn take(&mut self, id: u16) -> Vec<u8> {
        self.pump();
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

    fn clear(&mut self) {
        self.pump();
        self.got.clear();
    }

    fn chat(&mut self, st: &mut Core, text: &str) -> String {
        let mut w = PktWriter::empty();
        w.wstr(text, 121);
        let body = w.raw_bytes().to_vec();
        self.send(st, CHAT_SEND, &body);
        let d = self.take(CHAT_PUSH);
        let n = u32_at(&d, 0x34) as usize;
        String::from_utf16_lossy(
            &d[0x38..0x38 + 2 * n]
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        )
    }
}

fn wbytes(s: &str, n: usize) -> Vec<u8> {
    let mut w = PktWriter::empty();
    w.wstr(s, n);
    w.raw_bytes().to_vec()
}

fn player_on(st: &mut Core, sid: Sid, login: &str, nick: &str, l: Listener) -> (Client, u32) {
    let a = db::create_account(&st.db, login, 0).unwrap();
    db::create_character(
        &st.db,
        &db::NewCharacter {
            user_no: a,
            nick,
            face: 2,
            exp: 0,
            gp: 10000,
            nx: 0,
            now: 0,
        },
        true,
    )
    .unwrap();
    let rx = connect(st, sid, l);
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

fn player(st: &mut Core, sid: Sid, login: &str, nick: &str) -> (Client, u32) {
    player_on(st, sid, login, nick, Listener::Lobby)
}

fn col(st: &Core, a: u32, c: &str) -> rusqlite::types::Value {
    st.db
        .query_row(
            &format!("SELECT {c} FROM characters WHERE user_no = ?1"),
            [a as i64],
            |r| r.get(0),
        )
        .unwrap()
}

fn act<T>(st: &mut Core, f: impl FnOnce(&mut Core) -> Act<T>) -> Result<T, Refusal> {
    let mut out = None;
    st.scope(|st| {
        out = Some(f(st)?);
        Ok(())
    })
    .unwrap();
    out.unwrap()
}

// --- layouts --------------------------------------------------------------------------------------------------

#[test]
fn info_reply_layout() {
    let r = ClanSheet {
        uid: 7,
        member_limit: 20,
        members: 2,
        prev_rank: 3,
        rank: 1,
        level: 2,
        mark: 5,
        wins: 4,
        losses: 1,
        draws: 2,
        exp: 2500,
        points: 9,
        name: "Testers".into(),
        leader: "Alice".into(),
    };
    let (id, p) = frame(&info_reply(Some(&r)));
    assert_eq!((id, p.len()), (0x3c3, 0x82));
    assert_eq!(u32_at(&p, 6), 1);
    let words: Vec<u32> = (0..12).map(|i| u32_at(&p, 0xe + 4 * i)).collect();
    assert_eq!(words, vec![7, 20, 2, 3, 1, 2, 5, 4, 1, 2, 2500, 9]);
    // HARNESS: lobby_harness::lobby3 reads the level at +0x22 and the name at +0x3e
    assert_eq!(u32_at(&p, 0x22), 2);
    assert_eq!(wstr_at(&p, 0x3e, 17), "Testers");
    assert_eq!(wstr_at(&p, 0x60, 17), "Alice");
    let (_, p) = frame(&info_reply(None));
    assert_eq!((p.len(), u32_at(&p, 6)), (0x82, 0));
}

#[test]
fn members_reply_layout() {
    let rows = vec![
        MemberRow {
            char_no: 1,
            role: 5,
            exp: 100,
            name: "Alice".into(),
            online: 1,
        },
        MemberRow {
            char_no: 2,
            role: 1,
            exp: 7,
            name: "Bob".into(),
            online: 0,
        },
    ];
    let (id, p) = frame(&members_reply(true, &rows));
    assert_eq!((id, p.len()), (0x3c5, 0x12 + 2 * 0x32));
    assert_eq!((u32_at(&p, 6), u32_at(&p, 0xe)), (1, 2));
    let r1 = 0x12 + 0x32;
    assert_eq!(
        (u32_at(&p, 0x12), u32_at(&p, 0x16), u32_at(&p, 0x1a)),
        (1, 5, 100)
    );
    assert_eq!(wstr_at(&p, 0x12 + 0xc, 17), "Alice");
    assert_eq!(u32_at(&p, 0x12 + 0x2e), 1);
    assert_eq!(
        (
            u32_at(&p, r1),
            u32_at(&p, r1 + 4),
            u32_at(&p, r1 + 8),
            u32_at(&p, r1 + 0x2e)
        ),
        (2, 1, 7, 0)
    );
    assert_eq!(wstr_at(&p, r1 + 0xc, 17), "Bob");
    let (_, p) = frame(&members_reply(false, &rows));
    assert_eq!((p.len(), u32_at(&p, 6), u32_at(&p, 0xe)), (0x12, 0, 0));
    // the frame limit caps the rows
    let many = vec![MemberRow::default(); MAX_MEMBER_ROWS + 5];
    let f = members_reply(true, &many);
    assert!(f.len() - 8 <= 0x8000);
    assert_eq!(u32_at(&frame(&f).1, 0xe) as usize, MAX_MEMBER_ROWS);
}

#[test]
fn notice_check_and_push_layouts() {
    let (id, p) = frame(&notice_reply(Some("notice!")));
    assert_eq!((id, p.len(), u32_at(&p, 6)), (0x3c7, 0x100, 1));
    assert_eq!(wstr_at(&p, 0xe, 121), "notice!");
    let long: String = "x".repeat(200);
    let (_, p) = frame(&notice_reply(Some(&long)));
    assert_eq!((p.len(), wstr_at(&p, 0xe, 121).len()), (0x100, 120));
    let (_, p) = frame(&notice_reply(None));
    assert_eq!(u32_at(&p, 6), 0);
    for id in [CLAN_LOGIN_REPLY, CLAN_NOTICE_SET_REPLY] {
        let (i, p) = frame(&result_reply(id, true));
        assert_eq!((i, p.len(), u32_at(&p, 6)), (id, 0xa, 1));
    }
    for id in [
        CLAN_LEADER_CHECK_REPLY,
        CLAN_NAME_CHECK_REPLY,
        CLAN_MARK_CHECK_REPLY,
    ] {
        let (i, p) = frame(&check_reply(id, false, 0x4b));
        assert_eq!(
            (i, p.len(), u32_at(&p, 6), u32_at(&p, 0xa)),
            (id, 0xe, 0, 0x4b)
        );
        let (_, p) = frame(&check_reply(id, true, 0x4b));
        assert_eq!((u32_at(&p, 6), u32_at(&p, 0xa)), (1, 0));
    }
    for id in [
        CLAN_INFO_STALE_PUSH,
        CLAN_MEMBERS_STALE_PUSH,
        CLAN_NOTICE_STALE_PUSH,
        CLAN_REMOVED_PUSH,
    ] {
        let (i, p) = frame(&push(id));
        assert_eq!((i, p.len()), (id, 6));
    }
}

#[test]
fn request_parsers() {
    // 0x3c8 is 0xf8 bytes: +6 wstr[121]
    let mut w = PktWriter::new(CLAN_NOTICE_SET);
    w.wstr("hello clan", 121);
    let p = w.into_payload();
    assert_eq!(p.len(), 0xf8);
    assert_eq!(parse_notice(&p), "hello clan");
    // 0x34a / 0x348 are 0x28 bytes: +6 wstr[17]
    let mut w = PktWriter::new(CLAN_NAME_CHECK);
    w.wstr("Wolves", 17);
    let p = w.into_payload();
    assert_eq!((p.len(), parse_name(&p).as_str()), (0x28, "Wolves"));
    // 0x34c is 0xe bytes: +6 emblem, +0xa background
    let mut w = PktWriter::new(CLAN_MARK_CHECK);
    w.u32(7).u32(8);
    let p = w.into_payload();
    assert_eq!((p.len(), parse_mark(&p)), (0xe, (7, 8)));
}

#[test]
fn clan_title_holds_two_sixteen_unit_names() {
    let t = clan_title("Wolves", "Bears");
    let u: Vec<u16> = t.encode_utf16().collect();
    assert_eq!(u.len(), 21);
    assert_eq!(String::from_utf16_lossy(&u[..6]), "Wolves");
    assert!(u[6..16].iter().all(|&c| c == 0));
    assert_eq!(String::from_utf16_lossy(&u[16..]), "Bears");
    // a long name is cut to 15 units so the 16th stays NUL
    let t = clan_title(&"A".repeat(20), &"B".repeat(20));
    let u: Vec<u16> = t.encode_utf16().collect();
    assert_eq!((u.len(), u[15], u[16]), (31, 0, b'B' as u16));
    // the room record's wstr[33] carries both (SPEC: PROTOCOL_2009.md §3.6 room_record +4)
    let row = crate::wire::RoomRow {
        title: clan_title("Wolves", "Bears"),
        ..Default::default()
    };
    let rec = crate::wire::room_record(&row);
    assert_eq!(PktReader::from_raw(&rec).at(4).wstr(16), "Wolves");
    assert_eq!(PktReader::from_raw(&rec).at(4 + 32).wstr(16), "Bears");
}

// --- rules ----------------------------------------------------------------------------------------------------

#[test]
fn levels_limits_and_config_keys() {
    let mut r = ClanRules::default();
    assert_eq!(
        (
            r.level_of(0),
            r.level_of(1_999),
            r.level_of(2_000),
            r.level_of(10_000_000)
        ),
        (1, 1, 2, 6)
    );
    assert_eq!(
        (r.member_limit(1), r.member_limit(6), r.member_limit(9)),
        (20, 120, 120)
    );
    let j = |s: &str| JsonValue::parse(s).unwrap();
    assert!(r.apply("clan.level_steps", &j("[0, 10, 20]")));
    assert_eq!((r.level_of(9), r.level_of(10), r.level_of(25)), (1, 2, 3));
    assert!(
        !r.apply("clan.level_steps", &j("[5, 10]")),
        "level 1 starts at 0"
    );
    assert!(
        !r.apply("clan.level_steps", &j("[0, 10, 5]")),
        "never decreasing"
    );
    assert!(r.apply("clan.member_limits", &j("[3]")));
    assert_eq!(r.member_limit(4), 3);
    assert!(!r.apply("clan.member_limits", &j("[0]")));
    assert!(r.apply("clan.start_min_per_side", &j("1")));
    assert_eq!(r.start_min_per_side, 1);
    assert!(r.apply("clan.mode_weight.CDM", &j("0.5")));
    assert!(r.apply("clan.mode_weight.cdm", &j("2")));
    assert_eq!(
        (r.policy.weight_for("CDM"), r.policy.mode_weights.len()),
        (2.0, 1)
    );
    assert!(
        r.apply("clan.basic_point_default", &j("3"))
            && r.apply("clan.winner_point_default", &j("4"))
    );
    assert_eq!(
        (r.policy.basic_point_default, r.policy.winner_point_default),
        (3, 4)
    );
    assert!(r.apply("clan.chat_commands", &j("false")) && !r.chat_commands);
    assert!(!r.apply("clan.bogus", &j("1")));
    // the lobby config forwards clan.* keys
    let mut rules = crate::config::Rules::default();
    assert!(rules.apply("clan.channel_members_only", &j("false")));
    assert!(!rules.clan.channel_members_only);
}

#[test]
fn names_and_marks() {
    assert!(valid_name("Wolves") && valid_name(&"x".repeat(16)));
    assert!(
        !valid_name("")
            && !valid_name(&"x".repeat(17))
            && !valid_name(" pad")
            && !valid_name("a\tb")
    );
    let mut st = state();
    let (_c, a) = player(&mut st, 1, "a", "Alice");
    act(&mut st, |st| create(st, a, "Wolves", 3, 4)).unwrap();
    assert!(name_taken(&st.db, "WOLVES", 0).unwrap());
    assert!(
        !name_taken(&st.db, "Wolves", 1).unwrap(),
        "the clan's own name"
    );
    assert!(mark_taken(&st.db, 3, 4, 0).unwrap());
    assert!(!mark_taken(&st.db, 3, 4, 1).unwrap());
    assert!(!mark_taken(&st.db, 0, 0, 0).unwrap(), "(0, 0) is no mark");
}

// --- flows ----------------------------------------------------------------------------------------------------

#[test]
fn create_by_chat_then_info_members_notice() {
    let mut st = state();
    let (mut a, an) = player(&mut st, 1, "alice", "Alice");
    let (mut b, bn) = player(&mut st, 2, "bob", "Bob");
    assert_eq!(
        a.chat(&mut st, "!clan create Testers 5 6"),
        "Clan Testers created."
    );
    // 0x303 (own clan data) and the clan tab pushes come before the chat reply
    let ids = a.ids();
    assert!(
        ids.is_empty(),
        "everything up to the chat reply was taken: {ids:?}"
    );
    let k = clan_by_name(&st.db, "testers").unwrap().unwrap();
    assert_eq!((k.master, k.emblem, k.background), (an, 5, 6));
    assert_eq!(
        col(&st, an, "clan_type"),
        rusqlite::types::Value::Integer(5)
    );
    assert_eq!(
        col(&st, an, "clan_name"),
        rusqlite::types::Value::Text("Testers".into())
    );
    // the pushes in order
    st.scope(|st| {
        create(st, bn, "Other", 1, 1)?.unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(
        b.ids(),
        vec![
            CHAR_SHEET_REPLY,
            CLAN_INFO_STALE_PUSH,
            CLAN_MEMBERS_STALE_PUSH
        ]
    );
    let sheet = b.take(CHAR_SHEET_REPLY);
    assert_eq!(
        (
            u32_at(&sheet, 0xd4),
            u32_at(&sheet, 0xd8),
            u32_at(&sheet, 0xac)
        ),
        (1, 1, 5)
    );
    b.clear();
    // 0x3c0 -> 0x3c1 ok; 0x3c2 -> 0x3c3
    a.send(&mut st, CLAN_LOGIN, &[]);
    assert_eq!(u32_at(&a.take(CLAN_LOGIN_REPLY), 6), 1);
    a.send(&mut st, CLAN_INFO, &[]);
    let d = a.take(CLAN_INFO_REPLY);
    assert_eq!(
        (d.len(), u32_at(&d, 6), u32_at(&d, 0xe)),
        (0x82, 1, k.uid as u32)
    );
    assert_eq!(
        (u32_at(&d, 0x12), u32_at(&d, 0x16), u32_at(&d, 0x22)),
        (20, 1, 1)
    );
    assert_eq!(
        (wstr_at(&d, 0x3e, 17), wstr_at(&d, 0x60, 17)),
        ("Testers".to_string(), "Alice".to_string())
    );
    // the clan with more exp ranks first; ties by age
    assert_eq!(u32_at(&d, 0x1e), 1);
    st.db
        .execute("UPDATE clan_records SET exp = 99 WHERE clan_uid = 2", [])
        .unwrap();
    a.send(&mut st, CLAN_INFO, &[]);
    assert_eq!(u32_at(&a.take(CLAN_INFO_REPLY), 0x1e), 2);
    // members
    a.send(&mut st, CLAN_MEMBERS, &[]);
    let d = a.take(CLAN_MEMBERS_REPLY);
    assert_eq!(
        (
            u32_at(&d, 6),
            u32_at(&d, 0xe),
            u32_at(&d, 0x12),
            u32_at(&d, 0x16),
            u32_at(&d, 0x12 + 0x2e)
        ),
        (1, 1, an, 5, 1)
    );
    // notice: the leader sets it, the other members get 0x3063
    let mut w = PktWriter::empty();
    w.wstr("notice!", 121);
    let body = w.raw_bytes().to_vec();
    a.send(&mut st, CLAN_NOTICE_SET, &body);
    assert_eq!(u32_at(&a.take(CLAN_NOTICE_SET_REPLY), 6), 1);
    a.send(&mut st, CLAN_NOTICE, &[]);
    assert_eq!(wstr_at(&a.take(CLAN_NOTICE_REPLY), 0xe, 121), "notice!");
    // no clan: every request answers result 0
    let (mut c, _) = player(&mut st, 3, "carl", "Carl");
    for (q, r) in [
        (CLAN_LOGIN, CLAN_LOGIN_REPLY),
        (CLAN_INFO, CLAN_INFO_REPLY),
        (CLAN_MEMBERS, CLAN_MEMBERS_REPLY),
        (CLAN_NOTICE, CLAN_NOTICE_REPLY),
    ] {
        c.send(&mut st, q, &[]);
        assert_eq!(u32_at(&c.take(r), 6), 0, "0x{q:x}");
    }
    c.send(&mut st, CLAN_NOTICE_SET, &body);
    assert_eq!(u32_at(&c.take(CLAN_NOTICE_SET_REPLY), 6), 0);
    // refusals of a second create
    assert!(a
        .chat(&mut st, "!clan create Again")
        .starts_with("Cannot create the clan: you are already in clan Testers"));
    assert!(c.chat(&mut st, "!clan create testers").contains("taken"));
    assert!(c.chat(&mut st, "!clan create Third 5 6").contains("mark"));
    assert!(c
        .chat(&mut st, &format!("!clan create {}", "x".repeat(17)))
        .contains("1 to 16"));
    // ordinary chat is not taken
    a.send(&mut st, CHAT_SEND, &wbytes("hello", 121));
    let d = a.take(CHAT_PUSH);
    assert_eq!(u32_at(&d, 6), an);
}

#[test]
fn join_leave_kick_and_handover_pushes() {
    let mut st = state();
    let (mut a, an) = player(&mut st, 1, "alice", "Alice");
    let (mut b, bn) = player(&mut st, 2, "bob", "Bob");
    let (mut c, cn) = player(&mut st, 3, "cora", "Cora");
    let k = act(&mut st, |st| create(st, an, "Testers", 0, 0)).unwrap();
    a.clear();
    // LG-420: the chat join is an application; the leader's accept makes the member
    assert_eq!(
        b.chat(&mut st, "!clan join testers"),
        "Application sent to Testers."
    );
    a.clear();
    act(&mut st, |st| crate::clanreq::accept(st, an, bn)).unwrap();
    assert_eq!(
        a.ids(),
        vec![CLAN_INFO_STALE_PUSH, CLAN_MEMBERS_STALE_PUSH],
        "the other members refresh"
    );
    a.clear();
    act(&mut st, |st| join(st, cn, k.uid)).unwrap();
    assert_eq!(
        act(&mut st, |st| join(st, cn, k.uid)),
        Err(Refusal::InClan("Testers".into()))
    );
    assert_eq!(
        act(&mut st, |st| join(st, 99, k.uid)),
        Err(Refusal::NoCharacter)
    );
    a.clear();
    b.clear();
    c.clear();
    // 0x34c: leader ok, member 0x4b, no clan 0x4a, a pair in use 0x4c
    a.send(
        &mut st,
        CLAN_MARK_CHECK,
        &[7u32.to_le_bytes(), 8u32.to_le_bytes()].concat(),
    );
    assert_eq!(u32_at(&a.take(CLAN_MARK_CHECK_REPLY), 6), 1);
    b.send(
        &mut st,
        CLAN_MARK_CHECK,
        &[7u32.to_le_bytes(), 8u32.to_le_bytes()].concat(),
    );
    assert_eq!(u32_at(&b.take(CLAN_MARK_CHECK_REPLY), 0xa), 0x4b);
    // a member leaves: 0x3064 then 0x303 to it, the others refresh
    assert_eq!(b.chat(&mut st, "!clan leave"), "You left Testers.");
    assert_eq!(col(&st, bn, "clan_uid"), rusqlite::types::Value::Integer(0));
    assert_eq!(
        col(&st, bn, "clan_type"),
        rusqlite::types::Value::Integer(0)
    );
    b.send(
        &mut st,
        CLAN_MARK_CHECK,
        &[1u32.to_le_bytes(), 1u32.to_le_bytes()].concat(),
    );
    assert_eq!(u32_at(&b.take(CLAN_MARK_CHECK_REPLY), 0xa), 0x4a);
    assert_eq!(a.ids(), vec![CLAN_INFO_STALE_PUSH, CLAN_MEMBERS_STALE_PUSH]);
    a.clear();
    c.clear();
    st.scope(|st| {
        leave(st, cn)?.unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(c.ids(), vec![CLAN_REMOVED_PUSH, CHAR_SHEET_REPLY]);
    c.clear();
    a.clear();
    // kick: only the leader; the member gets 0x3064 + 0x303
    act(&mut st, |st| join(st, bn, k.uid)).unwrap();
    act(&mut st, |st| join(st, cn, k.uid)).unwrap();
    assert_eq!(act(&mut st, |st| kick(st, bn, cn)), Err(Refusal::NotLeader));
    assert_eq!(
        act(&mut st, |st| kick(st, an, an)),
        Err(Refusal::NoSuchMember)
    );
    b.clear();
    c.clear();
    assert_eq!(
        a.chat(&mut st, "!clan kick cora"),
        "Cora was removed from the clan."
    );
    assert_eq!(c.ids(), vec![CLAN_REMOVED_PUSH, CHAR_SHEET_REPLY]);
    assert_eq!(b.ids(), vec![CLAN_INFO_STALE_PUSH, CLAN_MEMBERS_STALE_PUSH]);
    // the leader leaves with a member left: the longest-standing member leads
    b.clear();
    match act(&mut st, |st| leave(st, an)).unwrap() {
        Left::HandedOver(_, next) => assert_eq!(next, bn),
        other => panic!("{other:?}"),
    }
    assert_eq!(clan_by_uid(&st.db, k.uid).unwrap().unwrap().master, bn);
    assert_eq!(
        col(&st, bn, "clan_type"),
        rusqlite::types::Value::Integer(5)
    );
    let ids = b.ids();
    assert_eq!(
        ids[0], CHAR_SHEET_REPLY,
        "the new leader's 0x303 (role 5): {ids:?}"
    );
    // the last member leaves: the clan is gone
    assert!(matches!(
        act(&mut st, |st| leave(st, bn)).unwrap(),
        Left::Disbanded(_)
    ));
    assert!(clan_by_uid(&st.db, k.uid).unwrap().is_none());
    let n: i64 = st
        .db
        .query_row("SELECT COUNT(*) FROM clan_records", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(act(&mut st, |st| leave(st, bn)), Err(Refusal::NoClan));
}

#[test]
fn member_limit_by_level() {
    let mut st = state();
    st.rules.clan.member_limits = vec![2, 3];
    st.rules.clan.level_steps = vec![0, 100];
    let (_a, an) = player(&mut st, 1, "a", "Ann");
    let (_b, bn) = player(&mut st, 2, "b", "Ben");
    let (_c, cn) = player(&mut st, 3, "c", "Cid");
    let k = act(&mut st, |st| create(st, an, "Few", 0, 0)).unwrap();
    act(&mut st, |st| join(st, bn, k.uid)).unwrap();
    assert_eq!(act(&mut st, |st| join(st, cn, k.uid)), Err(Refusal::Full));
    st.db
        .execute("UPDATE clan_records SET exp = 100", [])
        .unwrap();
    act(&mut st, |st| join(st, cn, k.uid)).unwrap();
    assert_eq!(
        act(&mut st, |st| join(st, cn, 99)),
        Err(Refusal::InClan("Few".into()))
    );
}

#[test]
fn name_and_leader_checks() {
    let mut st = state();
    let (mut a, an) = player(&mut st, 1, "a", "Ann");
    let (mut b, bn) = player(&mut st, 2, "b", "Ben");
    let k = act(&mut st, |st| create(st, an, "Wolves", 0, 0)).unwrap();
    act(&mut st, |st| join(st, bn, k.uid)).unwrap();
    for (name, ok, why) in [("Bears", 1, 0), ("wolves", 0, 0x4e), ("", 0, 0x57)] {
        b.send(&mut st, CLAN_NAME_CHECK, &wbytes(name, 17));
        let d = b.take(CLAN_NAME_CHECK_REPLY);
        assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (ok, why), "{name}");
    }
    // the leader may keep the own clan's name (rename item with a case change)
    a.send(&mut st, CLAN_NAME_CHECK, &wbytes("WOLVES", 17));
    assert_eq!(u32_at(&a.take(CLAN_NAME_CHECK_REPLY), 6), 1);
    for (name, ok, why) in [("Ann", 1, 0), ("ben", 0, 0x68), ("Nobody", 0, 0x56)] {
        b.send(&mut st, CLAN_LEADER_CHECK, &wbytes(name, 17));
        let d = b.take(CLAN_LEADER_CHECK_REPLY);
        assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (ok, why), "{name}");
    }
}

#[test]
fn item_window_clan_rename_keeps_the_member_rows_in_step() {
    // the clan item windows (itemuse.rs, CD-80) write the same tables: a renamed clan shows in 0x3c3 and 0x303
    let mut st = state();
    let (mut a, an) = player(&mut st, 1, "a", "Ann");
    let k = act(&mut st, |st| create(st, an, "Old", 0, 0)).unwrap();
    st.db
        .execute("UPDATE clans SET name = 'New' WHERE clan_uid = ?1", [k.uid])
        .unwrap();
    st.db
        .execute(
            "UPDATE characters SET clan_name = 'New' WHERE clan_uid = ?1",
            [k.uid],
        )
        .unwrap();
    a.clear();
    a.send(&mut st, CLAN_INFO, &[]);
    assert_eq!(wstr_at(&a.take(CLAN_INFO_REPLY), 0x3e, 17), "New");
}

#[test]
fn clan_lobby_channels_need_a_clan() {
    let mut st = state();
    let (mut a, an) = player_on(&mut st, 1, "a", "Ann", Listener::Clan);
    let (mut b, _bn) = player_on(&mut st, 2, "b", "Ben", Listener::Clan);
    let (_n, _nn) = player(&mut st, 3, "n", "Nia");
    act(&mut st, |st| create(st, an, "Wolves", 0, 0)).unwrap();
    a.clear();
    let enter = |c: &mut Client, st: &mut Core, acc: u32, ch: u32| {
        let mut body = [acc.to_le_bytes(), ch.to_le_bytes()].concat();
        body.extend(wbytes("Clan Channel 1", 32));
        c.send(st, CHANNEL_ENTER, &body);
        let r = c.take(CHANNEL_ENTER_REPLY);
        u32_at(&r, 6)
    };
    assert_eq!(enter(&mut b, &mut st, 2, 1), 0, "no clan, no entry (CLN-1)");
    assert_eq!(enter(&mut a, &mut st, an, 1), 1);
    assert_eq!(st.sessions[&1].channel, Some(CLAN_CHANNEL_BASE + 1));
    assert_eq!(enter(&mut a, &mut st, an, 99), 0, "no such channel");
    a.send(&mut st, CHANNEL_LIST, &[]);
    let d = a.take(CHANNEL_LIST_REPLY);
    assert_eq!(u32_at(&d, 0xe), st.rules.channels);
    assert_eq!(u32_at(&d, 0x12 + 0x46 + 8), 1, "one user in clan channel 1");
    // the normal lobby's channel 1 is a different channel
    assert_eq!(crate::lobby::channels(&st)[0].current, 0);
    st.rules.clan.channel_members_only = false;
    assert_eq!(enter(&mut b, &mut st, 2, 1), 1);
    // the clan lobby's room settings are the clan mode table's (empty without a ROM, still 0x10a bytes)
    a.send(
        &mut st,
        ROOM_SETTINGS_FETCH,
        &[0u32.to_le_bytes(), 0u32.to_le_bytes()].concat(),
    );
    assert_eq!(a.take(ROOM_SETTINGS_REPLY).len(), 0x10a);
}

fn clan_player(
    st: &mut Core,
    sid: Sid,
    login: &str,
    nick: &str,
    clan: Option<i64>,
    new_clan: Option<&str>,
) -> (Client, u32) {
    let (mut c, a) = player_on(st, sid, login, nick, Listener::Clan);
    if let Some(n) = new_clan {
        act(st, |st| create(st, a, n, sid as i64, 0)).unwrap();
    }
    if let Some(k) = clan {
        act(st, |st| join(st, a, k)).unwrap();
    }
    let mut body = [a.to_le_bytes(), 1u32.to_le_bytes()].concat();
    body.extend(wbytes("", 32));
    c.send(st, CHANNEL_ENTER, &body);
    assert_eq!(u32_at(&c.take(CHANNEL_ENTER_REPLY), 6), 1);
    c.clear();
    (c, a)
}

fn open_room_as(st: &mut Core, c: &mut Client) -> (u32, bool) {
    let mut b = 8i32.to_le_bytes().to_vec();
    b.extend(wbytes("", 33));
    b.extend(wbytes("", 9));
    b.extend(0i32.to_le_bytes());
    c.send(st, ROOM_CREATE, &b);
    let r = c.take(ROOM_CREATE_REPLY);
    (u32_at(&r, 0xe), u32_at(&r, 6) == 1)
}

fn walk_into(st: &mut Core, c: &mut Client, no: u32) -> (u32, u32) {
    let mut b = (no as i32).to_le_bytes().to_vec();
    b.extend(wbytes("", 9));
    b.extend(wbytes("", 65));
    c.send(st, ROOM_ENTER, &b);
    let r = c.take(ROOM_ENTER_REPLY);
    (u32_at(&r, 6), PktReader::new(&r).at(0xa).u16() as u32)
}

#[test]
fn clan_rooms_sides_teams_and_start_minimum() {
    let mut st = state();
    let (mut h1, h1n) = clan_player(&mut st, 1, "h1", "Hana", None, Some("Home"));
    let home = clan_of(&st, h1n).unwrap().unwrap().uid;
    let (mut h2, _) = clan_player(&mut st, 2, "h2", "Hugo", Some(home), None);
    let (mut v1, v1n) = clan_player(&mut st, 3, "v1", "Vera", None, Some("Away"));
    let away = clan_of(&st, v1n).unwrap().uid_or();
    let (mut v2, v2n) = clan_player(&mut st, 4, "v2", "Vito", Some(away), None);
    let (mut t1, _) = clan_player(&mut st, 5, "t1", "Tess", None, Some("Third"));
    // a player without a clan cannot even enter the clan lobby (CLN-1); open it for this test
    st.rules.clan.channel_members_only = false;
    let (mut n1, _) = clan_player(&mut st, 6, "n1", "Nils", None, None);
    // a lobby watcher sees the room row with both clan names
    let (no, ok) = open_room_as(&mut st, &mut h1);
    assert!(ok);
    let key = RoomKey {
        channel: CLAN_CHANNEL_BASE + 1,
        no,
    };
    assert_eq!(sides(&st, key).unwrap(), Some(Sides { home, away: 0 }));
    // the first row (empty title from the client) is replaced by the row with the clan names
    t1.take(LOBBY_ROOM_ADD_PUSH);
    assert_eq!(u32_at(&t1.take(LOBBY_ROOM_REMOVE_PUSH), 6), no);
    let row = t1.take(LOBBY_ROOM_ADD_PUSH);
    assert_eq!(PktReader::new(&row).at(6 + 4).wstr(16), "Home");
    // no clan / a third clan refused once the away side is set; members join their clan's team
    assert_eq!(walk_into(&mut st, &mut n1, no).0, 0);
    assert_eq!(walk_into(&mut st, &mut h2, no).0, 1);
    assert_eq!(st.rooms[&key].member(2 /* h2 */).map(|m| m.team), Some(1));
    assert_eq!(walk_into(&mut st, &mut v1, no).0, 1);
    assert_eq!(st.rooms[&key].member(v1n).map(|m| m.team), Some(2));
    assert_eq!(sides(&st, key).unwrap(), Some(Sides { home, away }));
    assert_eq!(st.rooms[&key].title, clan_title("Home", "Away"));
    assert_eq!(walk_into(&mut st, &mut t1, no).0, 0, "a third clan");
    // teams are fixed by clan
    v1.send(
        &mut st,
        TEAM_CHANGE,
        &[0u32.to_le_bytes(), 0u32.to_le_bytes(), 1u32.to_le_bytes()].concat(),
    );
    assert_eq!(u32_at(&v1.take(TEAM_CHANGE_REPLY), 6), 0);
    // start needs clan.start_min_per_side (2) per side
    h1.clear();
    h1.send(
        &mut st,
        ROOM_USER_FLAG,
        &[0u32.to_le_bytes(), 2u32.to_le_bytes()].concat(),
    );
    assert_eq!(u32_at(&h1.take(ROOM_USER_FLAG_REPLY), 6), 0);
    assert_eq!(st.rooms[&key].state, crate::room::RoomPhase::Waiting);
    assert_eq!(walk_into(&mut st, &mut v2, no).0, 1);
    assert_eq!(st.rooms[&key].member(v2n).map(|m| m.team), Some(2));
    h1.send(
        &mut st,
        ROOM_USER_FLAG,
        &[0u32.to_le_bytes(), 2u32.to_le_bytes()].concat(),
    );
    assert_eq!(u32_at(&h1.take(ROOM_USER_FLAG_REPLY), 6), 1);
    assert_eq!(st.rooms[&key].state, crate::room::RoomPhase::Starting);
    // when the away clan has left, another clan may take the away side
    let _ = st.take_deferred();
    for c in [&mut v1, &mut v2] {
        c.send(&mut st, ROOM_LEAVE, &[]);
    }
    if let Some(r) = st.rooms.get_mut(&key) {
        r.state = crate::room::RoomPhase::Waiting;
    }
    assert_eq!(walk_into(&mut st, &mut t1, no).0, 1);
    let third = clan_by_name(&st.db, "Third").unwrap().unwrap().uid;
    assert_eq!(sides(&st, key).unwrap(), Some(Sides { home, away: third }));
}

trait UidOr {
    fn uid_or(self) -> i64;
}
impl UidOr for Option<ClanRow> {
    fn uid_or(self) -> i64 {
        self.map(|k| k.uid).unwrap_or(0)
    }
}

#[test]
fn match_result_bookkeeping_cd11() {
    let mut st = state();
    st.rules.clan.policy.basic_point_default = 10;
    st.rules.clan.policy.winner_point_default = 5;
    st.rules.clan.policy.mode_weights = vec![("CDM".into(), 2.0)];
    let (_a, an) = player(&mut st, 1, "a", "Ann");
    let (_b, bn) = player(&mut st, 2, "b", "Ben");
    let h = act(&mut st, |st| create(st, an, "Home", 0, 0)).unwrap().uid;
    let v = act(&mut st, |st| create(st, bn, "Away", 0, 0)).unwrap().uid;
    let key = RoomKey {
        channel: CLAN_CHANNEL_BASE + 1,
        no: 0,
    };
    st.db
        .execute(
            "INSERT INTO clan_rooms (channel, room, home, away) VALUES (?1, 0, ?2, ?3)",
            params![key.channel, h, v],
        )
        .unwrap();
    let players = vec![
        PlayerResult {
            uid: an,
            kills: 2,
            team: 1,
            ..Default::default()
        },
        PlayerResult {
            uid: bn,
            kills: 0,
            team: 2,
            ..Default::default()
        },
    ];
    let r = MatchResult {
        winner: 1,
        ..Default::default()
    };
    st.scope(|st| on_match_result(st, key, 0, 0, &r, &players))
        .unwrap();
    // no ROM: the CD-11 fallbacks (10 basic, 5 winner) times the CDM weight 2, plus the members' CD-03 exp
    let rules = st.rules.clone();
    let won = rewards::base(
        &rules,
        &RewardCtx {
            kills: 2,
            team: 1,
            winner: 1,
            exp_factor: 1.0,
            gp_factor: 1.0,
            ..Default::default()
        },
    )
    .0;
    let lost = rewards::base(
        &rules,
        &RewardCtx {
            kills: 0,
            team: 2,
            winner: 1,
            exp_factor: 1.0,
            gp_factor: 1.0,
            ..Default::default()
        },
    )
    .0;
    let sh = stats(&st.db, h).unwrap();
    let sv = stats(&st.db, v).unwrap();
    assert_eq!(
        (sh.wins, sh.losses, sh.draws, sh.exp, sh.points),
        (1, 0, 0, 30 + won, 30)
    );
    assert_eq!(
        (sv.wins, sv.losses, sv.draws, sv.exp, sv.points),
        (0, 1, 0, 20 + lost, 20)
    );
    // a draw
    let r = MatchResult {
        winner: 0,
        ..Default::default()
    };
    st.scope(|st| on_match_result(st, key, 0, 0, &r, &[]))
        .unwrap();
    assert_eq!(
        (
            stats(&st.db, h).unwrap().draws,
            stats(&st.db, v).unwrap().draws
        ),
        (1, 1)
    );
    let n: i64 = st
        .db
        .query_row("SELECT COUNT(*) FROM clan_matches", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2);
    // not a clan room: nothing
    let other = RoomKey { channel: 1, no: 0 };
    st.scope(|st| on_match_result(st, other, 0, 0, &r, &players))
        .unwrap();
    assert_eq!(stats(&st.db, h).unwrap().draws, 1);
    // side gains never go below zero and ignore a bad weight
    let p = ClanPoints {
        exp_basic: -5,
        exp_winner: 0,
        money_basic: 2,
        money_winner: 3,
        weight: f64::NAN,
    };
    assert_eq!(side_gain(&p, true, -3), SideGain { exp: 0, points: 5 });
    // start-up: previous rank = rank now; room sides dropped
    st.scope(startup).unwrap();
    assert_eq!(
        (
            stats(&st.db, h).unwrap().prev_rank,
            stats(&st.db, v).unwrap().prev_rank
        ),
        (1, 2)
    );
    let n: i64 = st
        .db
        .query_row("SELECT COUNT(*) FROM clan_rooms", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn rom_clan_tables_when_available() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut st = state();
    st.rom = std::sync::Arc::new(crate::rom::RomData::load(&rom));
    // the clan lobby's room settings come from the ROM clan mode table
    let f = clan_room_settings_reply(&st);
    let (_, p) = frame(&f);
    assert_eq!(p.len(), 0x10a);
    assert!(u32_at(&p, 0x16) > 0, "clan mode masks from the ROM");
    assert!(st.rom.mode_count(true) > 0);
    // the clan points of clan mode 0 on its default map are finite and the weight is the CD-11 default
    let map = st.rom.game_room.default_map(0, true).max(0) as u32;
    let pts = clan_points(&st, map, 0);
    assert_eq!(pts.weight, 1.0);
    assert!(pts.exp_basic >= 0 && pts.money_basic >= 0, "{pts:?}");
}

/// 0x38c from the room's host: (field, value) -> the 0x38d ok flag.
fn set_room(st: &mut Core, c: &mut Client, f: u32, v: u32) -> bool {
    c.clear();
    let b: Vec<u8> = [0u32, f, v].iter().flat_map(|x| x.to_le_bytes()).collect();
    c.send(st, ROOM_SETTING_CHANGE, &b);
    u32_at(&c.take(ROOM_SETTING_CHANGE_REPLY), 6) == 1
}

#[test]
fn locked_clan_options_follow_the_config_keys() {
    let mut r = ClanRules::default();
    assert!(!r.room_mid_join && !r.room_friendly_fire);
    let j = |s: &str| JsonValue::parse(s).unwrap();
    assert!(r.apply("clan.room_mid_join", &j("true")) && r.room_mid_join);
    assert!(r.apply("clan.room_friendly_fire", &j("true")) && r.room_friendly_fire);
    assert!(!r.apply("clan.room_friendly_fire", &j("1")), "a bool key");
    // without a ROM no clan row locks anything: a clan room takes every option as before
    let mut st = state();
    let (mut h1, _) = clan_player(&mut st, 1, "h1", "Hana", None, Some("Home"));
    let (no, ok) = open_room_as(&mut st, &mut h1);
    assert!(ok);
    let key = RoomKey {
        channel: CLAN_CHANNEL_BASE + 1,
        no,
    };
    assert!(locked_values(&st, 0).is_empty());
    assert!(set_room(&mut st, &mut h1, field::OPT_FRIENDLY_FIRE, 1));
    assert_eq!(st.rooms[&key].ff_on, 1);
    assert!(set_room(&mut st, &mut h1, field::GOAL, 50));
}

/// LG-412 / LG-413: with the ROM, a clan room holds the options its clan mode row locks (friendly fire and
/// mid-match join on every 2009 clan row) and refuses other values, a goal outside the row's range and a room size
/// outside its player range; the open options (kill cam, weapon limit) change as in any room; a mode change takes
/// the clan row's goal.
#[test]
fn rom_clan_room_refuses_what_the_clan_row_locks() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut st = state();
    st.rom = std::sync::Arc::new(crate::rom::RomData::load(&rom));
    let g = &st.rom.game_room;
    let flag = |m: usize, k: &str| g.mode_flag(m, true, k);
    assert_eq!(
        flag(0, "EnableTeamKillSetting"),
        Some(false),
        "ROM clan row 0"
    );
    assert_eq!(flag(0, "EnableJoinCombatSetting"), Some(false));
    assert_eq!(flag(0, "EnableWeaponUseSetting"), Some(true));
    assert_eq!(flag(0, "EnableViewEnemyCameraSetting"), Some(true));
    let goal0 = g.default_goal(0, true) as u32;
    let goal1 = g.default_goal(1, true) as u32;
    let (gmin, gmax) = (
        g.mode_int(0, true, "GoalMin").unwrap(),
        g.mode_int(0, true, "GoalMax").unwrap(),
    );
    let pmin = g.mode_int(0, true, "MinPlayers").unwrap() as u32;
    let pmax = g.mode_int(0, true, "MaxPlayers").unwrap() as u32;
    let (mut h1, _) = clan_player(&mut st, 1, "h1", "Hana", None, Some("Home"));
    let (no, ok) = open_room_as(&mut st, &mut h1);
    assert!(ok);
    let key = RoomKey {
        channel: CLAN_CHANNEL_BASE + 1,
        no,
    };
    // created on the held values (clan.room_* off) and the clan row's goal
    let r = &st.rooms[&key];
    assert_eq!((r.mode, r.ff_on, r.mid_join, r.goal), (0, 0, 0, goal0));
    assert_eq!(
        locked_values(&st, 0),
        vec![(field::OPT_MID_JOIN, 0), (field::OPT_FRIENDLY_FIRE, 0)]
    );
    // locked: another value refused, the held one accepted
    assert!(!set_room(&mut st, &mut h1, field::OPT_FRIENDLY_FIRE, 1));
    assert!(!set_room(&mut st, &mut h1, field::OPT_MID_JOIN, 1));
    assert_eq!((st.rooms[&key].ff_on, st.rooms[&key].mid_join), (0, 0));
    assert!(set_room(&mut st, &mut h1, field::OPT_FRIENDLY_FIRE, 0));
    // open: kill cam and the weapon limit
    assert!(set_room(&mut st, &mut h1, field::OPT_KILL_CAM, 1));
    assert!(set_room(&mut st, &mut h1, field::WEAPON_LIMIT, 3));
    assert_eq!(st.rooms[&key].weapon_limit, 3);
    // the goal and the room size stay inside the clan row's ranges
    assert!(!set_room(&mut st, &mut h1, field::GOAL, gmax as u32 + 1));
    if gmin > 1 {
        assert!(!set_room(&mut st, &mut h1, field::GOAL, gmin as u32 - 1));
    }
    assert!(set_room(&mut st, &mut h1, field::GOAL, gmin as u32));
    if pmin > 1 {
        assert!(!set_room(&mut st, &mut h1, field::MAX_PLAYERS, pmin - 1));
    }
    assert!(set_room(&mut st, &mut h1, field::MAX_PLAYERS, pmax));
    // a mode change takes the clan row's default goal
    assert!(set_room(&mut st, &mut h1, field::MODE, 1));
    assert_eq!(st.rooms[&key].goal, goal1);
    // an operator who wants friendly fire in clan matches holds it on; then "off" is the refused value
    st.rules.clan.room_friendly_fire = true;
    let (mut h2, _) = clan_player(&mut st, 2, "h2", "Hugo", None, Some("Other"));
    let (no2, ok) = open_room_as(&mut st, &mut h2);
    assert!(ok);
    let key2 = RoomKey {
        channel: CLAN_CHANNEL_BASE + 1,
        no: no2,
    };
    assert_eq!(st.rooms[&key2].ff_on, 1);
    assert!(!set_room(&mut st, &mut h2, field::OPT_FRIENDLY_FIRE, 0));
}
