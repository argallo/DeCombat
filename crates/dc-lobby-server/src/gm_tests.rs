//! In-process flows of the operator tools, notices, reports and the messenger relay over fake connections.
//! They follow the steps of HARNESS: lobby_harness::test_lb5gm / test_lobby5 / test_persist_restart
//! (messenger part) with the DC_DEFAULTS rules.

use crate::clock;
use crate::db;
use crate::gm::*;
use crate::logs::Listener;
use crate::messenger::MESSENGER_PUSH;
use crate::msg::*;
use crate::net::route;
use crate::notices::*;
use crate::state::testing::*;
use crate::state::{Core, Outgoing, Sid};
use dc_wire::{PktReader, PktWriter};
use rusqlite::params;
use std::sync::mpsc::Receiver;

struct Client {
    sid: Sid,
    rx: Receiver<Outgoing>,
    got: Vec<(u16, Vec<u8>)>,
    closed: bool,
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
        while let Ok(o) = self.rx.try_recv() {
            match o {
                Outgoing::Frame(f) => self
                    .got
                    .push((u16::from_le_bytes([f[0], f[1]]), f[8..].to_vec())),
                Outgoing::Close => self.closed = true,
            }
        }
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

    fn has(&mut self, id: u16) -> bool {
        self.pump();
        self.got.iter().any(|(f, _)| *f == id)
    }

    fn clear(&mut self) {
        self.pump();
        self.got.clear();
    }

    fn chat(&mut self, st: &mut Core, text: &str) {
        self.send(st, CHAT_SEND, &wstr(text, 121));
    }

    /// Fields of the next 0x3f01 line.
    fn line(&mut self) -> Vec<String> {
        let d = self.take(MESSENGER_PUSH);
        PktReader::new(&d)
            .at(6)
            .wstr((d.len() - 6) / 2)
            .split('\t')
            .map(|s| s.to_string())
            .collect()
    }
}

fn wstr(s: &str, n: usize) -> Vec<u8> {
    let mut w = PktWriter::empty();
    w.wstr(s, n);
    w.raw_bytes().to_vec()
}

fn u16_at(p: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([p[o], p[o + 1]])
}

/// An account with a character, logged into the lobby on `sid` and waiting in channel 1.
fn player(st: &mut Core, sid: Sid, login: &str, nick: &str) -> (Client, u32) {
    let acc = db::create_account(&st.db, login, 0).unwrap();
    st.scope(|st| crate::auth::create_character(st, acc, nick, 2))
        .unwrap();
    let rx = connect(st, sid, Listener::Lobby);
    {
        let s = st.sessions.get_mut(&sid).unwrap();
        s.account = Some(acc);
        s.in_lobby = true;
        s.channel = Some(1);
    }
    (
        Client {
            sid,
            rx,
            got: Vec::new(),
            closed: false,
        },
        acc,
    )
}

fn restrict(
    st: &mut Core,
    c: &mut Client,
    nick: &str,
    action: u32,
    period: i32,
    reason: &str,
) -> (u32, u16) {
    let mut b = wstr(nick, 17);
    b.extend(action.to_le_bytes());
    b.extend(period.to_le_bytes());
    b.extend(wstr(reason, 65));
    c.send(st, OP_RESTRICT, &b);
    let d = c.take(OP_RESTRICT_REPLY);
    (u32_at(&d, 6), u16_at(&d, 0xa))
}

fn mute(st: &mut Core, c: &mut Client, nick: &str, on: u32, minutes: i32) -> (u32, u16) {
    let mut b = wstr(nick, 17);
    b.extend(on.to_le_bytes());
    b.extend(minutes.to_le_bytes());
    b.extend([0u8; 0x82]);
    c.send(st, OP_MUTE, &b);
    let d = c.take(OP_MUTE_REPLY);
    (u32_at(&d, 6), u16_at(&d, 0xa))
}

fn auth_login(st: &mut Core, sid: Sid, login: &str) -> Vec<u8> {
    let mut a = Client {
        sid,
        rx: connect(st, sid, Listener::Auth),
        got: Vec::new(),
        closed: false,
    };
    let mut b = wstr(&format!("LOCAL:{login}"), 4096);
    b.extend(wstr("pw", 17));
    b.extend(wstr("00-11", 20));
    b.extend(0i32.to_le_bytes());
    a.send(st, LOGIN_REQUEST, &b);
    let r = a.take(LOGIN_REPLY);
    st.sessions.remove(&sid);
    r
}

fn set_gm(st: &Core, account: u32, level: i64) {
    st.db
        .execute(
            "UPDATE accounts SET gm = ?2 WHERE user_no = ?1",
            params![account as i64, level],
        )
        .unwrap();
}

#[test]
fn operator_commands_follow_the_lb5gm_steps() {
    let mut st = state();
    let (mut alice, a) = player(&mut st, 1, "alice", "Alice");
    let (mut bob, b) = player(&mut st, 2, "bob", "Bob");
    // not a GM: refused with no reason (the client shows nothing)
    assert_eq!(
        restrict(&mut st, &mut bob, "Alice", ACTION_KICK, 0, ""),
        (0, 0)
    );
    set_gm(&st, a, 2);
    // lookup by name, by account; unknown name; bad selector
    let mut q = 2i32.to_le_bytes().to_vec();
    q.extend(wstr("bob", 17));
    alice.send(&mut st, OP_LOOKUP, &q);
    let d = alice.take(OP_LOOKUP_REPLY);
    assert_eq!(
        (d.len(), u32_at(&d, 6), u32_at(&d, 0xe), u32_at(&d, 0x78)),
        (0x90, 1, b, 1)
    );
    assert_eq!(PktReader::new(&d).at(0x34).wstr(17), "Bob");
    assert_eq!(PktReader::new(&d).at(0x12).wstr(17), "bob");
    assert_eq!(
        (u32_at(&d, 0x80), u32_at(&d, 0x84), u32_at(&d, 0x88)),
        (0, 0, u32::MAX),
        "server 0, channel 0, no room"
    );
    let mut q = 0i32.to_le_bytes().to_vec();
    q.extend(b.to_le_bytes());
    q.extend([0u8; 0x1e]);
    alice.send(&mut st, OP_LOOKUP, &q);
    assert_eq!(u32_at(&alice.take(OP_LOOKUP_REPLY), 0xe), b);
    let mut q = 2i32.to_le_bytes().to_vec();
    q.extend(wstr("Nobody", 17));
    alice.send(&mut st, OP_LOOKUP, &q);
    let d = alice.take(OP_LOOKUP_REPLY);
    assert_eq!(
        (u32_at(&d, 6), u16_at(&d, 0xa)),
        (0, REASON_LOOKUP_NOT_FOUND)
    );
    let mut q = 7i32.to_le_bytes().to_vec();
    q.extend([0u8; 0x22]);
    alice.send(&mut st, OP_LOOKUP, &q);
    assert_eq!(u16_at(&alice.take(OP_LOOKUP_REPLY), 0xa), REASON_BAD_QUERY);
    // mute: unknown target, then Bob for 2 minutes
    assert_eq!(
        mute(&mut st, &mut alice, "Nobody", 1, 2),
        (0, REASON_MUTE_UNKNOWN)
    );
    let before = clock::now_ticks();
    assert_eq!(mute(&mut st, &mut alice, "Bob", 1, 2), (1, 0));
    let ft = u64::from_le_bytes(bob.take(MUTE_UNTIL_PUSH)[6..14].try_into().unwrap()) as i64;
    assert!(
        (ft - before - 120 * clock::TICKS_PER_SEC).abs() < 5 * clock::TICKS_PER_SEC,
        "local FILETIME + 2 min"
    );
    bob.send(&mut st, CHAR_SHEET_FETCH, &[]);
    let d = bob.take(CHAR_SHEET_REPLY);
    assert_eq!(
        u64::from_le_bytes(d[0xec..0xf4].try_into().unwrap()) as i64,
        ft,
        "0x303 +0xec carries the mute end"
    );
    // muted chat is dropped server-side; the sender hears its mute end again
    alice.clear();
    bob.chat(&mut st, "hello");
    assert!(!alice.has(CHAT_PUSH), "nobody hears a muted player");
    assert!(bob.has(MUTE_UNTIL_PUSH) && !bob.has(CHAT_PUSH));
    bob.clear();
    assert_eq!(mute(&mut st, &mut alice, "Bob", 0, 0), (1, 0));
    assert_eq!(
        u64::from_le_bytes(bob.take(MUTE_UNTIL_PUSH)[6..14].try_into().unwrap()),
        0
    );
    bob.chat(&mut st, "hello again");
    assert!(alice.has(CHAT_PUSH));
    // observer mode: GM only
    alice.send(&mut st, OBSERVER_SET, &1u32.to_le_bytes());
    assert_eq!(u32_at(&alice.take(OBSERVER_SET_REPLY), 6), 1);
    assert!(st.ops.observers.contains(&1));
    bob.send(&mut st, OBSERVER_SET, &1u32.to_le_bytes());
    assert_eq!(u32_at(&bob.take(OBSERVER_SET_REPLY), 6), 0);
    // report (kind 2), no reply
    let mut rep = PktWriter::new(PLAYER_REPORT);
    rep.u32(2)
        .u32(1)
        .u32(a)
        .wstr("Alice", 17)
        .wstr("bad words", 601)
        .u32(3)
        .wstr("abc", 0x4b0)
        .at(0xe4c);
    let p = rep.into_payload();
    bob.clear();
    bob.send(&mut st, PLAYER_REPORT, &p[6..]);
    assert!(bob.got.is_empty(), "no reply to 0x312");
    let row: (i64, String, String, String, i64, i64) = st
        .db
        .query_row(
            "SELECT kind, target_nick, text, extra, category, sub FROM reports WHERE reporter = ?1",
            [b as i64],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .unwrap();
    // 0x312 +0xa / +0xe are the picked general kind and detail index (cshell+0x273790): 1 and `a` here
    assert_eq!(
        row,
        (
            2,
            "Alice".into(),
            "bad words".into(),
            "abc".into(),
            1,
            a as i64
        )
    );
    // ban: unknown target, then Bob for 3 days -> dropped, login refused with the day count
    assert_eq!(
        restrict(&mut st, &mut alice, "Nobody", ACTION_BAN, 3, ""),
        (0, REASON_RESTRICT_UNKNOWN)
    );
    assert_eq!(
        restrict(&mut st, &mut alice, "Bob", ACTION_BAN, 3, "test"),
        (1, 0)
    );
    bob.pump();
    assert!(bob.closed, "the server drops a banned player");
    assert!(!st.sessions.contains_key(&2));
    let d = auth_login(&mut st, 10, "bob");
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0xa), u32_at(&d, 0x2a)),
        (0, REASON_LOGIN_BANNED, 3)
    );
    assert_eq!(
        restrict(&mut st, &mut alice, "Bob", ACTION_LIFT, 0, ""),
        (1, 0)
    );
    assert_eq!(u32_at(&auth_login(&mut st, 11, "bob"), 6), 1);
    let n: i64 = st
        .db
        .query_row(
            "SELECT COUNT(*) FROM op_actions WHERE by_no = ?1",
            [a as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert!(n >= 6, "every operator action is logged ({n})");
}

#[test]
fn kick_drops_an_online_player_and_refuses_an_offline_one() {
    let mut st = state();
    let (mut alice, a) = player(&mut st, 1, "alice", "Alice");
    let (mut bob, _b) = player(&mut st, 2, "bob", "Bob");
    set_gm(&st, a, 1);
    let mut b = wstr("bob", 17);
    b.extend(ACTION_KICK.to_le_bytes());
    b.extend(0i32.to_le_bytes());
    b.extend(wstr("afk", 65));
    alice.send(&mut st, OP_RESTRICT, &b);
    // the other channel users see Bob leave the lobby list, then the operator gets the result
    assert_eq!(u32_at(&alice.take(LOBBY_USER_REMOVE_PUSH), 6), _b);
    assert_eq!(u32_at(&alice.take(OP_RESTRICT_REPLY), 6), 1);
    bob.pump();
    assert!(bob.closed);
    assert!(!st.sessions.contains_key(&2));
    assert_eq!(
        restrict(&mut st, &mut alice, "Bob", ACTION_KICK, 0, ""),
        (0, REASON_RESTRICT_UNKNOWN),
        "offline"
    );
    // a kick does not ban
    assert_eq!(u32_at(&auth_login(&mut st, 10, "bob"), 6), 1);
    // an expired ban is forgotten
    st.db
        .execute(
            "INSERT INTO op_bans (user_no, until) VALUES (?1, 5)",
            [_b as i64],
        )
        .unwrap();
    assert_eq!(u32_at(&auth_login(&mut st, 11, "bob"), 6), 1);
    assert_eq!(
        st.db
            .query_row("SELECT COUNT(*) FROM op_bans", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    // a permanent ban reports gm.permanent_ban_days
    st.rules.ops.permanent_ban_days = 9;
    assert_eq!(
        restrict(&mut st, &mut alice, "Bob", ACTION_BAN, 0, ""),
        (1, 0)
    );
    let d = auth_login(&mut st, 12, "bob");
    assert_eq!(
        (u32_at(&d, 0xa), u32_at(&d, 0x2a)),
        (REASON_LOGIN_BANNED, 9)
    );
}

#[test]
fn test_chat_commands_follow_the_lobby5_steps() {
    let mut st = state();
    let (mut alice, a) = player(&mut st, 1, "alice", "Alice");
    let (mut bob, _) = player(&mut st, 2, "bob", "Bob");
    // the test server binds loopback (the default --bind): everyone may use them (OPS-1)
    alice.chat(&mut st, "/notice Server maintenance at 5");
    for c in [&mut alice, &mut bob] {
        let d = c.take(TICKER_PUSH);
        let n = u32_at(&d, 0xe) as usize;
        assert_eq!(
            PktReader::new(&d).at(0x12).wstr(n + 1),
            "Server maintenance at 5"
        );
        assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (30, 3));
        assert!(!c.has(CHAT_PUSH), "a command is not chat");
    }
    st.db
        .execute(
            "INSERT INTO items (user_no, item_no, end_time) VALUES (?1, 12, 1234)",
            [a as i64],
        )
        .unwrap();
    alice.chat(&mut st, "/expire");
    assert!(alice.has(0x33f), "the removal push comes too");
    let d = alice.take(EXPIRY_NOTICE_PUSH);
    let n = u32_at(&d, 6) as usize;
    assert!(n >= 1 && (0..n).any(|i| u32_at(&d, 0xa + 4 * i) == 12));
    alice.chat(&mut st, "/shopver hot sale price");
    for id in [0x3056u16, 0x3058, 0x3059] {
        let d = bob.take(id);
        assert!(d.len() == 0xa && u32_at(&d, 6) >= 2, "0x{id:x}");
    }
    alice.chat(&mut st, "/grade 3");
    assert_eq!(u32_at(&alice.take(GRADE_PUSH), 6), 3);
    assert_eq!(st.character(a).unwrap().grade, 3);
    // a public server: only GM accounts; for others the line is plain chat
    st.opts.bind = "0.0.0.0".into();
    alice.clear();
    bob.clear();
    alice.chat(&mut st, "/notice hi");
    assert!(!bob.has(TICKER_PUSH) && bob.has(CHAT_PUSH));
    set_gm(&st, a, 2);
    alice.chat(&mut st, "/notice hi");
    assert!(bob.has(TICKER_PUSH));
}

#[test]
fn messenger_relay_needs_acceptance() {
    let mut st = state();
    let (mut dave, d) = player(&mut st, 1, "dave", "Dave");
    let (mut erin, e) = player(&mut st, 2, "erin", "Erin");
    dave.chat(&mut st, "!nm add\tErin");
    assert_eq!(dave.line(), ["status", "Friend request sent"]);
    assert!(
        !dave.has(CHAT_PUSH) && !erin.has(CHAT_PUSH),
        "messenger lines are not room chat"
    );
    assert_eq!(erin.line(), ["request", "Dave"]);
    let rows = |st: &Core| {
        st.db
            .query_row("SELECT COUNT(*) FROM friends", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(rows(&st), 0, "no friendship before the acceptance");
    erin.chat(&mut st, "!nm accept\tDave");
    assert_eq!(dave.line()[..3], ["friends", "Dave", "Erin"]);
    assert_eq!(erin.line()[..3], ["friends", "Erin", "Dave"]);
    assert_eq!(rows(&st), 2, "symmetric rows");
    let fr: i64 = st
        .db
        .query_row(
            "SELECT COUNT(*) FROM friends WHERE user_no = ?1 AND friend_no = ?2",
            params![d as i64, e as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fr, 1);
    dave.chat(&mut st, "!nm list");
    assert_eq!(dave.line()[..3], ["friends", "Dave", "Erin"]);
    assert_eq!(dave.line(), ["online", "Erin", "1"]);
    // whisper to both sides; a muted player cannot whisper
    dave.chat(&mut st, "!nm w\tErin\tsee you");
    assert_eq!(erin.line(), ["whisper", "Dave", "Erin", "see you"]);
    assert_eq!(dave.line(), ["whisper", "Dave", "Erin", "see you"]);
    st.db
        .execute(
            "UPDATE characters SET mute_until = ?2 WHERE user_no = ?1",
            params![d as i64, clock::now_ticks() + clock::TICKS_PER_DAY],
        )
        .unwrap();
    dave.chat(&mut st, "!nm w\tErin\tagain");
    assert!(dave.has(crate::gm::MUTE_UNTIL_PUSH));
    assert_eq!(dave.line(), ["status", "You are muted"]);
    st.db
        .execute("UPDATE characters SET mute_until = 0", [])
        .unwrap();
    // friend-menu invite: the 2009 room invite 0x3023 reaches a friend waiting in the lobby
    st.scope(|st| {
        let key = crate::room::RoomKey { channel: 1, no: 4 };
        st.sessions.get_mut(&1).unwrap().room = Some(key);
        Ok(())
    })
    .unwrap();
    dave.chat(&mut st, "!nm invite\tErin");
    assert_eq!(dave.line(), ["status", "Invite sent"]);
    assert_eq!(u32_at(&erin.take(ROOM_INVITE_PUSH), 6), 4);
    // presence: Erin's last session goes
    st.scope(|st| crate::lobby::disconnect(st, 2)).unwrap();
    assert_eq!(dave.line(), ["online", "Erin", "0"]);
    // remove
    dave.chat(&mut st, "!nm remove\tErin");
    assert_eq!(dave.line(), ["friends", "Dave"]);
    assert_eq!(rows(&st), 0);
}

#[test]
fn friend_requests_survive_and_are_resent_at_login() {
    let mut st = state();
    let (mut dave, _) = player(&mut st, 1, "dave", "Dave");
    let (mut erin, e) = player(&mut st, 2, "erin", "Erin");
    st.scope(|st| crate::lobby::disconnect(st, 2)).unwrap();
    erin.clear();
    dave.chat(&mut st, "!nm add Erin");
    assert_eq!(dave.line(), ["status", "Friend request sent"]);
    // Erin comes back: the open request is shown again
    let rx = connect(&mut st, 3, Listener::Lobby);
    st.sessions.get_mut(&3).unwrap().account = Some(e);
    st.sessions.get_mut(&3).unwrap().in_lobby = true;
    let mut erin = Client {
        sid: 3,
        rx,
        got: Vec::new(),
        closed: false,
    };
    st.scope(|st| crate::gm::on_lobby_login(st, 3, e)).unwrap();
    assert_eq!(erin.line(), ["friends", "Erin"]);
    assert_eq!(erin.line(), ["request", "Dave"]);
    // adding back accepts
    erin.chat(&mut st, "!nm add\tDave");
    assert_eq!(dave.line()[..3], ["friends", "Dave", "Erin"]);
    dave.chat(&mut st, "!nm add\tErin");
    assert_eq!(dave.line(), ["status", "Already friends"]);
    dave.chat(&mut st, "!nm bogus");
    assert_eq!(dave.line(), ["status", "Unknown messenger command"]);
    dave.chat(&mut st, "!nm add\tNobody");
    assert_eq!(dave.line(), ["status", "No such player"]);
}

#[test]
fn case_announcements_and_login_notices() {
    let mut st = state();
    let (mut dave, d) = player(&mut st, 1, "dave", "Dave");
    let (mut erin, _) = player(&mut st, 2, "erin", "Erin");
    let award = |days| crate::delivery::Delivered {
        uid: 1,
        item_no: 54,
        end: 0,
        extended: false,
        days,
    };
    st.scope(|st| case_award(st, d, &award(3))).unwrap();
    assert!(
        !erin.has(CASE_ANNOUNCE_PUSH),
        "timed awards are not announced by default"
    );
    st.scope(|st| case_award(st, d, &award(0))).unwrap();
    let p = erin.take(CASE_ANNOUNCE_PUSH);
    assert_eq!(
        (PktReader::new(&p).at(0x12).wstr(17), u32_at(&p, 0x34)),
        ("Dave".into(), 54)
    );
    assert!(dave.has(CASE_ANNOUNCE_PUSH));
    // operator login texts
    st.rules.ops.login_ticker = "Welcome".into();
    st.rules.ops.login_box = "Read the rules".into();
    st.scope(|st| crate::gm::on_lobby_login(st, 1, d)).unwrap();
    st.scope(|st| crate::gm::on_lobby_login(st, 1, d)).unwrap();
    let t = dave.take(TICKER_PUSH);
    assert_eq!(PktReader::new(&t).at(0x12).wstr(8), "Welcome");
    assert_eq!(st.character(d).unwrap().notice_pending, 1);
    let n: i64 = st
        .db
        .query_row(
            "SELECT COUNT(*) FROM notices WHERE user_no = ?1",
            [d as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "queued once while unread");
    dave.send(&mut st, NOTICE_FETCH, &[]);
    let p = dave.take(NOTICE_REPLY);
    assert!(PktReader::new(&p)
        .at(0x12)
        .wstr(513)
        .contains("Read the rules"));
}

#[test]
fn moderators_are_flagged_to_the_game_server() {
    let mut st = state();
    let (_c, a) = player(&mut st, 1, "alice", "Alice");
    assert_eq!(moderator_token(&st, a), "");
    set_gm(&st, a, 1);
    assert_eq!(moderator_token(&st, a), " mod=1");
    st.rules.ops.flag_moderators_to_game = false;
    assert_eq!(moderator_token(&st, a), "");
}

#[test]
fn report_cap_per_hour() {
    let mut st = state();
    let (mut bob, b) = player(&mut st, 1, "bob", "Bob");
    st.rules.ops.reports_per_hour = 2;
    let mut rep = PktWriter::new(PLAYER_REPORT);
    rep.u32(0)
        .u32(0)
        .u32(0)
        .wstr("X", 17)
        .wstr("t", 601)
        .at(0xe4c);
    let p = rep.into_payload();
    for _ in 0..4 {
        bob.send(&mut st, PLAYER_REPORT, &p[6..]);
    }
    let n: i64 = st
        .db
        .query_row(
            "SELECT COUNT(*) FROM reports WHERE reporter = ?1",
            [b as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2);
}

/// `!nm sync` (the reference stack's line after a match) is taken without any reply: no status line, no room chat.
// DC-DEFAULT: CLI-274 (proposed)
#[test]
fn messenger_sync_is_taken_without_a_reply() {
    let mut st = state();
    let (mut dave, _) = player(&mut st, 1, "dave", "Dave");
    let (mut erin, _) = player(&mut st, 2, "erin", "Erin");
    dave.clear();
    erin.clear();
    dave.chat(&mut st, "!nm sync");
    assert!(
        !dave.has(crate::messenger::MESSENGER_PUSH),
        "no status line"
    );
    assert!(
        !dave.has(CHAT_PUSH) && !erin.has(CHAT_PUSH),
        "not room chat"
    );
    dave.chat(&mut st, "!nm bogus");
    assert_eq!(dave.line(), ["status", "Unknown messenger command"]);
}
