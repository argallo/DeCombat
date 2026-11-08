//! In-process flows over fake connections: login, channel, room, start with countdown, late join, host
//! hand-over, kick. Timers are driven by hand (the countdown thread only calls `game::tick` / `countdown_done`).

use crate::game;
use crate::logs::Listener;
use crate::msg::*;
use crate::net::route;
use crate::room::{RoomKey, RoomPhase};
use crate::state::testing::*;
use crate::state::{Core, Deferred, Outgoing, Sid};
use dc_wire::PktWriter;
use std::sync::mpsc::Receiver;

pub(crate) struct Client {
    pub(crate) sid: Sid,
    pub(crate) rx: Receiver<Outgoing>,
    pub(crate) got: Vec<(u16, Vec<u8>)>,
}

impl Client {
    pub(crate) fn send(&mut self, st: &mut Core, id: u16, body: &[u8]) {
        let mut w = PktWriter::new(id);
        w.bytes(body);
        let p = w.into_payload();
        st.scope(|st| route(st, self.sid, id as i32, &p)).unwrap();
        self.pump();
    }

    fn pump(&mut self) {
        self.got.extend(drain(&self.rx));
    }

    /// Removes and returns the first frame with `id` (and everything before it), like the harness's recv_until.
    pub(crate) fn take(&mut self, id: u16) -> Vec<u8> {
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

    pub(crate) fn has(&mut self, id: u16) -> bool {
        self.pump();
        self.got.iter().any(|(f, _)| *f == id)
    }

    pub(crate) fn all(&mut self, id: u16) -> Vec<Vec<u8>> {
        self.pump();
        self.got
            .iter()
            .filter(|(f, _)| *f == id)
            .map(|(_, d)| d.clone())
            .collect()
    }
}

pub(crate) fn wstr(s: &str, n: usize) -> Vec<u8> {
    let mut w = PktWriter::empty();
    w.wstr(s, n);
    w.raw_bytes().to_vec()
}

pub(crate) fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Auth login + character + server pick, then a lobby connection with the key and the channel.
pub(crate) fn player(
    st: &mut Core,
    sid_auth: Sid,
    sid_lobby: Sid,
    login: &str,
    nick: &str,
) -> (Client, u32) {
    let (mut l, account, key) = lobby_login(st, sid_auth, sid_lobby, login, nick);
    // the key is single use
    let mut again = Client {
        sid: sid_lobby + 100,
        rx: connect(st, sid_lobby + 100, Listener::Lobby),
        got: Vec::new(),
    };
    let mut b2 = key;
    b2.extend(account.to_le_bytes());
    again.send(st, LOBBY_LOGIN, &b2);
    assert_eq!(
        u32_at(&again.take(LOBBY_LOGIN_REPLY), 6),
        0,
        "a used key is refused"
    );
    st.sessions.remove(&(sid_lobby + 100));
    let mut b = u32s(&[account, 1]);
    b.extend(wstr("Channel 1", 32));
    l.send(st, CHANNEL_ENTER, &b);
    let r = l.take(CHANNEL_ENTER_REPLY);
    assert_eq!((u32_at(&r, 6), u32_at(&r, 0xe)), (1, account));
    (l, account)
}

/// Auth login + character + server pick, then the lobby login: the lobby client right after its login reply
/// (whatever followed the reply still in `got`), the account and the key.
fn lobby_login(
    st: &mut Core,
    sid_auth: Sid,
    sid_lobby: Sid,
    login: &str,
    nick: &str,
) -> (Client, u32, Vec<u8>) {
    let mut a = Client {
        sid: sid_auth,
        rx: connect(st, sid_auth, Listener::Auth),
        got: Vec::new(),
    };
    let mut body = wstr(&format!("LOCAL:{login}"), 4096);
    body.extend(wstr("pw", 17));
    body.extend(wstr("00-11", 20));
    body.extend(0i32.to_le_bytes());
    a.send(st, LOGIN_REQUEST, &body);
    let r = a.take(LOGIN_REPLY);
    let account = u32_at(&r, 0x12);
    if u32_at(&r, 6) == 0 {
        let mut b = 6002i32.to_le_bytes().to_vec();
        b.extend(wstr(nick, 17));
        a.send(st, NEW_CHAR, &b);
        assert_eq!(u32_at(&a.take(NEW_CHAR_REPLY), 6), 1);
    }
    a.take(SERVER_LIST_PUSH);
    a.send(st, SERVER_PICK, &u32s(&[1]));
    let r = a.take(SERVER_PICK_REPLY);
    let key = r[0xe..0x1e].to_vec();
    st.sessions.remove(&sid_auth);
    let mut l = Client {
        sid: sid_lobby,
        rx: connect(st, sid_lobby, Listener::Lobby),
        got: Vec::new(),
    };
    let mut b = key.clone();
    b.extend(account.to_le_bytes());
    l.send(st, LOBBY_LOGIN, &b);
    assert_eq!(u32_at(&l.take(LOBBY_LOGIN_REPLY), 6), 1);
    (l, account, key)
}

pub(crate) fn open_room(st: &mut Core, c: &mut Client, pw: &str) -> u32 {
    let mut b = 8i32.to_le_bytes().to_vec();
    b.extend(wstr("Room", 33));
    b.extend(wstr(pw, 9));
    b.extend(0i32.to_le_bytes());
    c.send(st, ROOM_CREATE, &b);
    let r = c.take(ROOM_CREATE_REPLY);
    assert_eq!(u32_at(&r, 6), 1);
    u32_at(&r, 0xe)
}

pub(crate) fn enter(st: &mut Core, c: &mut Client, no: u32, pw: &str) -> (u32, u16) {
    let mut b = (no as i32).to_le_bytes().to_vec();
    b.extend(wstr(pw, 9));
    b.extend(wstr("", 65));
    c.send(st, ROOM_ENTER, &b);
    let r = c.take(ROOM_ENTER_REPLY);
    (u32_at(&r, 6), u16::from_le_bytes([r[0xa], r[0xb]]))
}

fn countdown(st: &mut Core, key: RoomKey) {
    let d = st.take_deferred();
    let gen = d
        .iter()
        .find_map(|x| match x {
            Deferred::Countdown { room, gen } if *room == key => Some(*gen),
            _ => None,
        })
        .expect("countdown queued");
    for i in 0..st.rules.countdown_ticks {
        st.scope(|st| {
            assert!(game::tick(st, key, gen, i));
            Ok(())
        })
        .unwrap();
    }
    st.scope(|st| {
        game::countdown_done(st, key, gen);
        Ok(())
    })
    .unwrap();
}

/// CD-87: an item that expired while the owner was away is told at the next lobby login as a 0x3011 box (the
/// item number), once; the login's own expiry scan removed it before the reply.
#[test]
fn items_expired_while_away_are_told_at_the_next_login() {
    let mut st = state();
    let (_zoe, acc) = player(&mut st, 1, 2, "zoe", "Zoe");
    st.db
        .execute(
            "INSERT INTO items (user_no, item_no, end_time) VALUES (?1, 12, 1234)",
            [acc as i64],
        )
        .unwrap();
    let (mut zoe2, _, _) = lobby_login(&mut st, 5, 6, "zoe", "Zoe");
    let d = zoe2.take(crate::notices::EXPIRY_NOTICE_PUSH);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0xa)),
        (1, 12),
        "one box: item 12"
    );
    let left: i64 = st
        .db
        .query_row(
            "SELECT COUNT(*) FROM items WHERE user_no = ?1 AND item_no = 12",
            [acc as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "the login's scan removed it");
    let (mut zoe3, _, _) = lobby_login(&mut st, 9, 10, "zoe", "Zoe");
    assert!(!zoe3.has(crate::notices::EXPIRY_NOTICE_PUSH), "told once");
}

#[test]
fn two_players_room_start_rejoin_handover_and_kick() {
    let mut st = state();
    st.rules.kick_needs_badge = false; // LG-203 has its own test
    let (mut a, na) = player(&mut st, 1, 2, "alice", "Alice");
    let (mut b, nb) = player(&mut st, 3, 4, "bob", "Bob");
    // Alice sees Bob arrive in the lobby (in-channel id = account number, CD-22)
    assert_eq!(u32_at(&a.take(LOBBY_USER_ADD_PUSH), 6 + 4), nb);
    // lobby chat reaches both, sender included (CD-30)
    let mut chat = wstr("hello", 121);
    b.send(&mut st, CHAT_SEND, &chat);
    assert!(a.has(CHAT_PUSH) && b.has(CHAT_PUSH));
    a.got.clear();
    b.got.clear();
    // locked room
    let no = open_room(&mut st, &mut a, "1234");
    assert_eq!(
        u32_at(&b.take(LOBBY_USER_REMOVE_PUSH), 6),
        na,
        "host leaves the lobby list first"
    );
    assert_eq!(u32_at(&b.take(LOBBY_ROOM_ADD_PUSH), 6), no);
    assert_eq!(enter(&mut st, &mut b, no, "0000"), (0, 0x2a));
    assert_eq!(enter(&mut st, &mut b, no, "1234").0, 1);
    assert_eq!(u32_at(&a.take(ROOM_USER_ADD_PUSH), 6 + 4), nb);
    b.send(&mut st, ROOM_USERS, &[]);
    assert_eq!(u32_at(&b.take(ROOM_USERS_REPLY), 0xe), 2);
    let key = RoomKey { channel: 1, no };
    assert_eq!(
        st.rooms[&key].member(nb).unwrap().team,
        2,
        "entry joins the smaller team"
    );
    // a guest cannot start a waiting room (CD-24)
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(u32_at(&b.take(ROOM_USER_FLAG_REPLY), 6), 0);
    assert!(!b.has(MATCH_ADDRESS_PUSH));
    // the host cannot mark ready (CD-08 / CD-24)
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6), 0);
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(u32_at(&b.take(ROOM_USER_FLAG_REPLY), 6), 1);
    assert_eq!(u32_at(&a.take(ROOM_USER_STATE_PUSH), 0xa), 1);
    // settings: map 5 reaches the guest
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 3, 5]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    let u = b.take(ROOM_UPDATE_PUSH);
    assert_eq!((u32_at(&u, 0xa), u32_at(&u, 0xe)), (3, 5));
    // RM-1 (proposed): the change cleared Bob's ready state; he marks ready again before the host starts
    let d = b.take(ROOM_USER_STATE_PUSH);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (nb, 0));
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(u32_at(&b.take(ROOM_USER_FLAG_REPLY), 6), 1);
    // start: 0x393 ok, state pushes, five ticks 0..4, then the address push (CD-23)
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6), 1);
    let u = b.take(ROOM_UPDATE_PUSH);
    assert_eq!((u32_at(&u, 0xa), u32_at(&u, 0xe)), (7, 1));
    countdown(&mut st, key);
    let ticks: Vec<u32> = b
        .all(START_COUNTDOWN_PUSH)
        .iter()
        .map(|d| u32_at(d, 6))
        .collect();
    assert_eq!(ticks, vec![0, 1, 2, 3, 4]);
    for c in [&mut a, &mut b] {
        c.take(START_COUNTDOWN_PUSH);
        let d = c.take(MATCH_ADDRESS_PUSH);
        assert_eq!(u32_at(&d, 0x28), 27888);
        let ip: Vec<u16> = (0..9)
            .map(|i| u16::from_le_bytes([d[6 + 2 * i], d[7 + 2 * i]]))
            .collect();
        assert_eq!(String::from_utf16_lossy(&ip), "127.0.0.1");
    }
    assert!(st.rooms[&key].members.iter().all(|m| m.state == 3));
    // rejoin: the host back in the room cannot Start (room plays) but Ready joins the running match
    a.send(&mut st, MATCH_EXIT, &u32s(&[0]));
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6), 0);
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6), 1);
    assert_eq!(u32_at(&a.take(MATCH_ADDRESS_PUSH), 0x28), 27888);
    // kind 1 (shopping / away) gets no reply
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[1, 1]));
    assert!(!a.has(ROOM_USER_FLAG_REPLY));
    // both back in the room -> the room waits again (0x3035 type 7 value 0)
    a.got.clear();
    a.send(&mut st, MATCH_EXIT, &u32s(&[0]));
    b.send(&mut st, MATCH_EXIT, &u32s(&[0]));
    let ups: Vec<(u32, u32)> = a
        .all(ROOM_UPDATE_PUSH)
        .iter()
        .map(|d| (u32_at(d, 0xa), u32_at(d, 0xe)))
        .collect();
    assert!(ups.contains(&(7, 0)), "{ups:?}");
    // host hand-over by request (CD-28)
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 2, nb]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    assert_eq!(st.rooms[&key].host, nb);
    let ups: Vec<(u32, u32)> = b
        .all(ROOM_UPDATE_PUSH)
        .iter()
        .map(|d| (u32_at(d, 0xa), u32_at(d, 0xe)))
        .collect();
    assert!(ups.contains(&(2, nb)));
    // only the host kicks; the kicked user is told (reason 1) and may not come back (CD-27)
    a.send(&mut st, ROOM_KICK, &u32s(&[nb, nb]));
    assert_eq!(u32_at(&a.take(ROOM_KICK_REPLY), 6), 0);
    b.send(&mut st, ROOM_KICK, &u32s(&[na, na]));
    assert_eq!(u32_at(&b.take(ROOM_KICK_REPLY), 6), 1);
    let k = a.take(ROOM_USER_REMOVE_PUSH);
    assert_eq!((u32_at(&k, 6), u32_at(&k, 0xa)), (na, 1));
    assert_eq!(enter(&mut st, &mut a, no, "1234"), (0, 0x8a));
    // Alice is back in the lobby list: she sees the room row and chat goes to the lobby again
    chat = wstr("back", 121);
    a.send(&mut st, CHAT_SEND, &chat);
    assert!(a.has(CHAT_PUSH));
    // the last member leaves: the room closes and the lobby is told
    b.send(&mut st, ROOM_LEAVE, &[]);
    assert_eq!(u32_at(&b.take(ROOM_LEAVE_REPLY), 6), 1);
    assert_eq!(u32_at(&a.take(LOBBY_ROOM_REMOVE_PUSH), 6), no);
    assert!(st.rooms.is_empty());
}

/// LG-177: during the start countdown the host and the players ready at the start cannot leave the room (0x387 {0},
/// nothing changes); a player who was not ready can, and after the countdown everyone can again.
#[test]
fn a_starting_member_cannot_leave_during_the_countdown() {
    let mut st = state();
    let (mut a, na) = player(&mut st, 1, 2, "lk1", "Lock1");
    let (mut b, nb) = player(&mut st, 3, 4, "lk2", "Lock2");
    let (mut c, nc) = player(&mut st, 5, 6, "lk3", "Lock3");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    assert_eq!(enter(&mut st, &mut c, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(u32_at(&b.take(ROOM_USER_FLAG_REPLY), 6), 1);
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6), 1);
    assert_eq!(st.rooms[&key].state, RoomPhase::Starting);
    for (cl, id) in [(&mut a, na), (&mut b, nb)] {
        cl.send(&mut st, ROOM_LEAVE, &[]);
        assert_eq!(u32_at(&cl.take(ROOM_LEAVE_REPLY), 6), 0, "{id} is held");
        assert_eq!(st.rooms[&key].member(id).map(|m| m.state), Some(2));
    }
    // an un-ready changes nothing either (the starting state stays)
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 0]));
    assert_eq!(st.rooms[&key].member(nb).map(|m| m.state), Some(2));
    // the player who was not ready is free to go
    c.send(&mut st, ROOM_LEAVE, &[]);
    assert_eq!(u32_at(&c.take(ROOM_LEAVE_REPLY), 6), 1);
    assert!(st.rooms[&key].member(nc).is_none());
    countdown(&mut st, key);
    assert!(st.rooms[&key].members.iter().all(|m| m.state == 3));
    // in game the leave is no longer refused
    b.send(&mut st, ROOM_LEAVE, &[]);
    assert_eq!(u32_at(&b.take(ROOM_LEAVE_REPLY), 6), 1);
}

#[test]
fn host_leaving_hands_over_to_the_longest_present_member() {
    let mut st = state();
    let (mut a, _na) = player(&mut st, 1, 2, "a1", "Aaaa");
    let (mut b, nb) = player(&mut st, 3, 4, "b1", "Bbbb");
    let (mut c, nc) = player(&mut st, 5, 6, "c1", "Cccc");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    assert_eq!(enter(&mut st, &mut c, no, "").0, 1);
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    let key = RoomKey { channel: 1, no };
    a.send(&mut st, ROOM_LEAVE, &[]);
    let r = &st.rooms[&key];
    assert_eq!(r.host, nb, "B entered before C");
    assert_eq!(
        r.member(nb).unwrap().state,
        0,
        "the new host's ready mark is cleared"
    );
    let ups: Vec<(u32, u32)> = c
        .all(ROOM_UPDATE_PUSH)
        .iter()
        .map(|d| (u32_at(d, 0xa), u32_at(d, 0xe)))
        .collect();
    assert!(ups.contains(&(2, nb)) && ups.contains(&(5, 2)), "{ups:?}");
    // the guest's disconnect while the room waits leaves a 1-player room with B as host
    st.scope(|st| crate::lobby::disconnect(st, c.sid)).unwrap();
    assert_eq!(st.rooms[&key].members.len(), 1);
    assert!(!st.sessions.contains_key(&c.sid));
    let _ = nc;
}

#[test]
fn late_join_during_the_countdown_gets_the_address_at_the_end() {
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "a2", "Host");
    let (mut b, nb) = player(&mut st, 3, 4, "b2", "Late");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6), 1);
    let d = st.take_deferred();
    let gen = match d.first() {
        Some(Deferred::Countdown { gen, .. }) => *gen,
        other => panic!("{other:?}"),
    };
    st.scope(|st| {
        game::tick(st, key, gen, 0);
        game::tick(st, key, gen, 1);
        Ok(())
    })
    .unwrap();
    // B enters during the countdown (mid-match join on by default, CD-49 proposed) and readies
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(u32_at(&b.take(ROOM_USER_FLAG_REPLY), 6), 1);
    assert!(!b.has(MATCH_ADDRESS_PUSH));
    st.scope(|st| {
        for i in 2..5 {
            game::tick(st, key, gen, i);
        }
        game::countdown_done(st, key, gen);
        Ok(())
    })
    .unwrap();
    let ticks: Vec<u32> = b
        .all(START_COUNTDOWN_PUSH)
        .iter()
        .map(|d| u32_at(d, 6))
        .collect();
    assert_eq!(ticks, vec![2, 3, 4], "the remaining ticks");
    assert!(b.has(MATCH_ADDRESS_PUSH));
    assert_eq!(st.rooms[&key].member(nb).unwrap().state, 3);
    // a stale countdown (old generation) does nothing
    st.scope(|st| {
        assert!(!game::tick(st, key, gen + 7, 0));
        Ok(())
    })
    .unwrap();
}

#[test]
fn require_ready_refuses_start_with_an_unready_guest() {
    let mut st = state();
    st.opts.require_ready = true;
    let (mut a, _) = player(&mut st, 1, 2, "a3", "Host3");
    let (mut b, _) = player(&mut st, 3, 4, "b3", "Guest3");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6), 0);
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6), 1);
}

/// The rank word of each 0xb8 record of a 0x38b reply, by account.
fn seat_ranks(d: &[u8]) -> Vec<(u32, u32, u32)> {
    (0..u32_at(d, 0xe) as usize)
        .map(|i| {
            let o = 0x12 + i * 0xb8;
            (u32_at(d, o), u32_at(d, o + 8), u32_at(d, o + 0x42))
        })
        .collect()
}

#[test]
fn room_slots_carry_each_accounts_rank_in_both_views() {
    // LG-6: the rank word of a room slot is the account's rank by the client's rule (the same one the profile
    // and the game server's badge use), whoever asks: self and other slots agree
    let mut st = state();
    let (mut a, na) = player(&mut st, 1, 2, "ra", "RankA");
    let (mut b, nb) = player(&mut st, 3, 4, "rb", "RankB");
    let need = st.rom.ranks.need.clone();
    let exp_a = need[10] + 5; // rank 11
    let exp_b = need[0]; // rank 1
    for (n, e) in [(na, exp_a), (nb, exp_b)] {
        st.db
            .execute("UPDATE characters SET exp = ?2 WHERE user_no = ?1", (n, e))
            .unwrap();
    }
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    let want = vec![(na, 11, exp_a as u32), (nb, 1, exp_b as u32)];
    for c in [&mut a, &mut b] {
        c.send(&mut st, ROOM_USERS, &[]);
        let mut got = seat_ranks(&c.take(ROOM_USERS_REPLY));
        got.sort();
        assert_eq!(got, want);
    }
    // the profile rank agrees with the slot
    b.send(&mut st, PROFILE_FETCH, &u32s(&[na]));
    assert_eq!(u32_at(&b.take(PROFILE_REPLY), 0x50), 11);
    // past the last table row the client's rule gives rank 0 on every record (a second count would say 55)
    let top = *need.last().unwrap() + 10;
    st.db
        .execute(
            "UPDATE characters SET exp = ?2 WHERE user_no = ?1",
            (na, top),
        )
        .unwrap();
    a.send(&mut st, ROOM_USERS, &[]);
    let got = seat_ranks(&a.take(ROOM_USERS_REPLY));
    let me = got.iter().find(|r| r.0 == na).unwrap();
    assert_eq!(me.1, crate::game::client_rank(&need, top));
}

#[test]
fn lobby_lists_profile_and_room_settings() {
    let mut st = state();
    let (mut a, na) = player(&mut st, 1, 2, "a4", "Prof");
    a.send(&mut st, CHANNEL_LIST, &[]);
    let d = a.take(CHANNEL_LIST_REPLY);
    assert_eq!((u32_at(&d, 0xe), d.len()), (3, 0x12 + 3 * 0x5a));
    a.send(&mut st, CHANNEL_USERS, &[]);
    assert_eq!(u32_at(&a.take(CHANNEL_USERS_REPLY), 0xe), 1);
    a.send(&mut st, ROOM_SETTINGS_FETCH, &u32s(&[0, 0]));
    let d = a.take(ROOM_SETTINGS_REPLY);
    assert_eq!(d.len(), 0x10a);
    assert!(u32_at(&d, 0x16) > 0);
    a.send(&mut st, PROFILE_FETCH, &u32s(&[na]));
    let d = a.take(PROFILE_REPLY);
    assert_eq!((d.len(), u32_at(&d, 6), u32_at(&d, 0xe)), (0xe2, 1, na));
    a.send(&mut st, CHAR_SHEET_FETCH, &[]);
    let d = a.take(CHAR_SHEET_REPLY);
    assert_eq!(
        (d.len(), u32_at(&d, 0x56), u32_at(&d, 0x5a)),
        (0x108, 6002, 10000)
    );
    a.send(&mut st, INVENTORY, &[]);
    assert_eq!(u32_at(&a.take(INVENTORY_REPLY), 0xe), 5);
    a.send(&mut st, EQUIPMENT, &[]);
    let d = a.take(EQUIPMENT_REPLY);
    assert_eq!(d.len(), 0xa6);
    assert_eq!(u32_at(&d, 0x12), 101);
    a.send(&mut st, OPTION_SET, &u32s(&[1, 1]));
    a.send(&mut st, CHAR_SHEET_FETCH, &[]);
    assert_eq!(u32_at(&a.take(CHAR_SHEET_REPLY), 0xb0), 2);
}

#[test]
fn private_state_start_and_end_are_answered() {
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "ps", "Ps");
    a.send(&mut st, PRIVATE_STATE, &u32s(&[1]));
    let r = a.take(PRIVATE_STATE_REPLY);
    assert_eq!((r.len(), u32_at(&r, 6)), (0xa, 1));
    assert_eq!(st.sessions.get(&a.sid).unwrap().private_state, 1);
    a.send(&mut st, PRIVATE_STATE, &u32s(&[9]));
    assert_eq!(u32_at(&a.take(PRIVATE_STATE_REPLY), 6), 0, "unknown state");
    a.send(&mut st, PRIVATE_STATE_END, &[]);
    let r = a.take(PRIVATE_STATE_END_REPLY);
    assert_eq!((r.len(), u32_at(&r, 6)), (0xa, 1));
    assert_eq!(st.sessions.get(&a.sid).unwrap().private_state, 0);
}

#[test]
fn a_second_lobby_login_replaces_the_first() {
    let mut st = state();
    let (mut a, na) = player(&mut st, 1, 2, "dup", "Dup");
    let no = open_room(&mut st, &mut a, "");
    // the same account logs in again through the auth flow (new key, new connection)
    let key = crate::auth::issue_ticket(&mut st, na, 1);
    let rx = connect(&mut st, 9, Listener::Lobby);
    let mut c = Client {
        sid: 9,
        rx,
        got: Vec::new(),
    };
    let mut b = key.to_vec();
    b.extend(na.to_le_bytes());
    c.send(&mut st, LOBBY_LOGIN, &b);
    assert_eq!(u32_at(&c.take(LOBBY_LOGIN_REPLY), 6), 1);
    assert!(
        !st.sessions.contains_key(&a.sid),
        "the old connection is dropped"
    );
    assert!(
        !st.rooms.contains_key(&RoomKey { channel: 1, no }),
        "its room went with it"
    );
    assert!(
        matches!(a.rx.try_recv(), Ok(Outgoing::Close))
            || a.rx.try_iter().any(|o| matches!(o, Outgoing::Close))
    );
}

#[test]
fn a_settings_change_clears_the_guests_ready_state_and_start_needs_a_new_ready() {
    // DC-DEFAULT: RM-1 (proposed)
    let mut st = state();
    st.opts.require_ready = true;
    let (mut a, _) = player(&mut st, 1, 2, "rm1a", "RmHost");
    let (mut b, nb) = player(&mut st, 3, 4, "rm1b", "RmGuest");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(u32_at(&b.take(ROOM_USER_FLAG_REPLY), 6), 1);
    a.pump();
    b.pump();
    a.got.clear();
    b.got.clear();
    // map change: the guest is cleared, both members see 0x3042 {guest, 0}
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 3, 5]));
    for c in [&mut a, &mut b] {
        let d = c.take(ROOM_USER_STATE_PUSH);
        assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (nb, 0));
    }
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    assert_eq!(st.rooms[&key].member(nb).unwrap().state, 0);
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(
        u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6),
        0,
        "start needs a new ready"
    );
    // a second change with nobody ready pushes nothing more
    a.got.clear();
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 3, 4]));
    assert!(!a.has(ROOM_USER_STATE_PUSH));
    // ready again, then start works
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6), 1);
}

#[test]
fn the_ready_clearing_can_be_switched_off() {
    let mut st = state();
    st.rules.settings_change_clears_ready = false;
    let (mut a, _) = player(&mut st, 1, 2, "rm1c", "RmHost2");
    let (mut b, nb) = player(&mut st, 3, 4, "rm1d", "RmGuest2");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    a.pump();
    a.got.clear();
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 3, 5]));
    assert!(!a.has(ROOM_USER_STATE_PUSH));
    let key = RoomKey { channel: 1, no };
    assert_eq!(st.rooms[&key].member(nb).unwrap().state, 1);
    // DSV-180: the limit reaches the game server's room line
    assert!(st
        .maps()
        .room_line(0, 0, 100, 0, 6, &Default::default())
        .contains("arms=6"));
}

#[test]
fn leaving_a_running_match_with_0x394_phase_0_puts_the_room_back_to_waiting() {
    // OBSERVED: CAPTURE_tdm4 / CAPTURE_tdm2: the real client sends 0x394 {0} together with every UP_LEAVE_MATCH
    let mut st = state();
    let (mut a, na) = player(&mut st, 1, 2, "lm1a", "LeaveA");
    let (mut b, nb) = player(&mut st, 3, 4, "lm1b", "LeaveB");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    countdown(&mut st, key);
    assert!(st.rooms[&key].members.iter().all(|m| m.state == 3));
    a.pump();
    b.pump();
    a.got.clear();
    b.got.clear();

    // Bob leaves the running match: no reply, both see his state 0, the room keeps playing for Alice
    b.send(&mut st, MATCH_EXIT, &u32s(&[0]));
    for c in [&mut a, &mut b] {
        let d = c.take(ROOM_USER_STATE_PUSH);
        assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (nb, 0));
    }
    assert!(!b.has(ROOM_USER_FLAG_REPLY));
    assert_eq!(st.rooms[&key].member(nb).unwrap().state, 0);
    assert_eq!(st.rooms[&key].member(na).unwrap().state, 3);
    assert_ne!(st.rooms[&key].state, crate::room::RoomPhase::Waiting);

    // the same message again, and the second one the client sends after a failed join (phase 1), are no-ops
    a.got.clear();
    b.send(&mut st, MATCH_EXIT, &u32s(&[0]));
    b.send(&mut st, MATCH_EXIT, &u32s(&[1]));
    assert!(!a.has(ROOM_USER_STATE_PUSH) && !a.has(ROOM_UPDATE_PUSH));

    // the last one out ends the match: the room waits again (0x3035 type 7 value 0)
    a.send(&mut st, MATCH_EXIT, &u32s(&[0]));
    let ups: Vec<(u32, u32)> = a
        .all(ROOM_UPDATE_PUSH)
        .iter()
        .map(|d| (u32_at(d, 0xa), u32_at(d, 0xe)))
        .collect();
    assert!(ups.contains(&(7, 0)), "{ups:?}");
    assert_eq!(st.rooms[&key].state, crate::room::RoomPhase::Waiting);
    assert!(st.rooms[&key].members.iter().all(|m| m.state == 0));
}

#[test]
fn leave_notice_outside_a_room_or_in_a_waiting_room_is_ignored() {
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "lm2a", "Idle");
    a.send(&mut st, MATCH_EXIT, &u32s(&[0]));
    assert!(!a.has(ROOM_USER_STATE_PUSH));
    let no = open_room(&mut st, &mut a, "");
    a.pump();
    a.got.clear();
    a.send(&mut st, MATCH_EXIT, &u32s(&[0]));
    assert!(!a.has(ROOM_USER_STATE_PUSH) && !a.has(ROOM_UPDATE_PUSH));
    let key = RoomKey { channel: 1, no };
    assert_eq!(st.rooms[&key].state, crate::room::RoomPhase::Waiting);
}

fn slots_of(c: &mut Client, st: &mut Core) -> Vec<i32> {
    c.send(st, EQUIPMENT, &[]);
    let d = c.take(EQUIPMENT_REPLY);
    (0..15).map(|i| u32_at(&d, 0x12 + 4 * i) as i32).collect()
}

#[test]
fn explicit_default_weapon_keys_set_slots_0_and_1_without_a_rom() {
    // DC-DEFAULT: CD-18 refinement (project decision: the configured numbers win over the pick
    let mut st = state();
    let v = |t: &str| dc_mod_lua::JsonValue::parse(t).unwrap();
    assert!(st.rules.apply("shop.default_primary_item", &v("4321")));
    assert!(st.rules.apply("shop.default_secondary_item", &v("4322")));
    let (mut a, _) = player(&mut st, 1, 2, "dw1", "DefW");
    let s = slots_of(&mut a, &mut st);
    assert_eq!(&s[..4], &[4321, 4322, 103, 104]);
}

#[test]
fn default_weapon_keys_stay_off_without_a_rom_until_an_operator_sets_them() {
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "dw2", "DefW2");
    assert_eq!(&slots_of(&mut a, &mut st)[..4], &[101, 102, 103, 104]);
}

#[test]
fn rom_new_character_gets_the_configured_primary_and_secondary() {
    // gated: needs CA_ROM; the default numbers are the user's choice of 2026-11-03 (was 2026-10-25: 145 / 164)
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut st = state();
    st.rom = std::sync::Arc::new(crate::rom::RomData::load(&rom));
    assert_eq!(
        (
            st.rules.shop.default_primary_item,
            st.rules.shop.default_secondary_item
        ),
        (20, 1)
    );
    // the user's choice 2026-11-03: a new character starts with the M16A3 (20) and the M1911 MEU (1)
    let (mut a, na) = player(&mut st, 1, 2, "dw3", "RomDef");
    let s = slots_of(&mut a, &mut st);
    assert_eq!((s[0], s[1]), (20, 1), "{s:?}");
    let items = crate::db::items(&st.db, na).unwrap();
    assert!(items
        .iter()
        .any(|i| i.item_no == 20 && i.slot == 0 && i.default_type == 1));
    assert!(items
        .iter()
        .any(|i| i.item_no == 1 && i.slot == 1 && i.default_type == 2));
    // an operator choice replaces it; a number of the wrong slot kind is refused and the pick stays
    let v = |t: &str| dc_mod_lua::JsonValue::parse(t).unwrap();
    assert!(st.rules.apply("shop.default_primary_item", &v("4")));
    assert!(st.rules.apply("shop.default_secondary_item", &v("145")));
    let (mut b, _) = player(&mut st, 3, 4, "dw4", "RomDef2");
    let s = slots_of(&mut b, &mut st);
    assert_eq!((s[0], s[1]), (4, 164), "{s:?}");
    // 0 = unset: the ROM balanced pick
    assert!(st.rules.apply("shop.default_primary_item", &v("0")));
    let (mut c, _) = player(&mut st, 5, 6, "dw5", "RomDef3");
    assert_eq!(slots_of(&mut c, &mut st)[0], 145);
    // an account made before a change of the keys keeps what it was given at its next login (no forced change)
    st.sessions.remove(&4);
    let (mut b2, _) = player(&mut st, 11, 12, "dw4", "RomDef2");
    assert_eq!(slots_of(&mut b2, &mut st)[..2], [4, 164]);
}

/// Two players in a started room, a fake game record, and the pipe lines of a match in which Bob leaves early.
fn leaver_match(pay_leavers: bool) -> (Core, Client, Client, u32, u32, u64) {
    let mut st = state();
    st.rules.pay_leavers = pay_leavers;
    let (mut a, na) = player(&mut st, 1, 2, "lv1", "Stayer");
    let (mut b, nb) = player(&mut st, 3, 4, "lv2", "Leaver");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    countdown(&mut st, key);
    let id = 77;
    st.games.insert(
        id,
        game::Game {
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
    st.rooms.get_mut(&key).unwrap().game = Some(id);
    let feed = |st: &mut Core, line: &str| {
        st.scope(|st| game::on_line(st, id, line)).unwrap();
    };
    feed(&mut st, &format!("@join 0 {na} 1 0 Stayer"));
    feed(&mut st, &format!("@join 1 {nb} 2 1 Leaver"));
    // Bob leaves (UP_LEAVE_MATCH) before the round ends
    feed(&mut st, "@leave 1");
    feed(
        &mut st,
        &format!("@player uid={na} kills=3 deaths=1 team=1 tk=0 hs=1 name=Stayer"),
    );
    feed(
        &mut st,
        &format!("@player uid={nb} kills=2 deaths=2 team=2 tk=0 hs=0 name=Leaver"),
    );
    a.pump();
    b.pump();
    a.got.clear();
    b.got.clear();
    (st, a, b, na, nb, id)
}

// LG-310: the first login of the day pays once, shows its box through 0x303 +0x104 -> 0x366 and the 0x345 of the
// login carries the balance; a later login of the same day changes nothing on the wire.
#[test]
fn nx_daily_bonus_flows_through_the_real_lobby_once() {
    let mut st = state();
    st.rules.nx_enabled = true;
    let (mut a, acc, _) = lobby_login(&mut st, 1, 2, "nxa", "NxA");
    a.send(&mut st, CHAR_SHEET_FETCH, &[]);
    assert_eq!(u32_at(&a.take(CHAR_SHEET_REPLY), 0x104), 1, "a box waits");
    a.send(&mut st, BALANCE, &[]);
    let b = a.take(BALANCE_REPLY);
    assert_eq!(u32_at(&b, 0x12), st.rules.nx_daily as u32, "prepaid");
    assert_eq!(u32_at(&b, 0xe), 0, "credit untouched");
    a.send(&mut st, NOTICE_FETCH, &[]);
    let n = a.take(NOTICE_REPLY);
    let text: Vec<u16> = n[0x12..0x12 + 2 * 513]
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|c| *c != 0)
        .collect();
    assert_eq!(
        String::from_utf16(&text).unwrap(),
        format!("Daily bonus: +{} NX", st.rules.nx_daily)
    );
    // reconnect the same day
    let (mut a2, _, _) = lobby_login(&mut st, 3, 4, "nxa", "NxA");
    a2.send(&mut st, CHAR_SHEET_FETCH, &[]);
    assert_eq!(u32_at(&a2.take(CHAR_SHEET_REPLY), 0x104), 0, "paid once");
    assert_eq!(
        st.character(acc).unwrap().nx,
        st.rules.nx_daily,
        "the balance did not move"
    );
    // the next day (the stored day moved back by one)
    st.db
        .execute("UPDATE nx_daily SET last_day = last_day - 1", [])
        .unwrap();
    let _ = lobby_login(&mut st, 5, 6, "nxa", "NxA");
    assert_eq!(st.character(acc).unwrap().nx, 2 * st.rules.nx_daily);
}

// LG-310: the login's frames are the same with the feature on and off; only the balance and the box flag differ.
#[test]
fn nx_daily_bonus_leaves_the_login_wire_alone() {
    let frames = |on: bool| {
        let mut st = state();
        st.rules.nx_enabled = on;
        let (mut c, _, _) = lobby_login(&mut st, 1, 2, "wire", "Wire");
        c.send(&mut st, CHAR_SHEET_FETCH, &[]);
        c.send(&mut st, BALANCE, &[]);
        c.pump();
        c.got.clone()
    };
    let (off, on) = (frames(false), frames(true));
    let ids = |v: &[(u16, Vec<u8>)]| v.iter().map(|f| f.0).collect::<Vec<_>>();
    assert_eq!(ids(&off), ids(&on));
    let sheet = |v: &[(u16, Vec<u8>)]| {
        v.iter()
            .find(|f| f.0 == CHAR_SHEET_REPLY)
            .unwrap()
            .1
            .clone()
    };
    let (a, mut b) = (sheet(&off), sheet(&on));
    assert_eq!((u32_at(&a, 0x104), u32_at(&b, 0x104)), (0, 1));
    b[0x104] = 0;
    assert_eq!(a, b, "the sheet differs in the box flag only");
}

#[test]
fn nx_daily_bonus_skips_bots_and_the_switch() {
    let mut st = state();
    st.rules.nx_enabled = true;
    let (_b, bot, _) = lobby_login(&mut st, 1, 2, "PcBot7", "PcBot7");
    assert_eq!(st.character(bot).unwrap().nx, 0, "a bot name");
    st.opts.test_bot_accounts.push("helper".into());
    let (_h, helper, _) = lobby_login(&mut st, 3, 4, "helper", "Helper");
    assert_eq!(st.character(helper).unwrap().nx, 0, "--test-bot-account");
    st.rules.nx_enabled = false;
    let (_o, off, _) = lobby_login(&mut st, 5, 6, "offuser", "OffUser");
    assert_eq!(st.character(off).unwrap().nx, 0, "switched off");
}

/// Two clans of two in a played clan room; returns the core, one winner's client, both winners and losers, the game.
fn played_clan_room() -> (Core, Client, [u32; 2], [u32; 2], u64, RoomKey) {
    let mut st = state();
    st.rules.nx_enabled = true;
    st.rules.nx_daily = 0;
    let (mut a, na) = player(&mut st, 1, 2, "cw1", "Win1");
    let (mut b, nb) = player(&mut st, 3, 4, "cw2", "Win2");
    let (mut c, nc) = player(&mut st, 5, 6, "cl1", "Lose1");
    let (mut d, nd) = player(&mut st, 7, 8, "cl2", "Lose2");
    let no = open_room(&mut st, &mut a, "");
    enter(&mut st, &mut b, no, "");
    enter(&mut st, &mut c, no, "");
    enter(&mut st, &mut d, no, "");
    let key = RoomKey { channel: 1, no };
    let clan = |n: &str, id: i64, m: [u32; 2]| {
        st.db
            .execute(
                "INSERT INTO clans (clan_uid, name, master) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, n, m[0] as i64],
            )
            .unwrap();
        for u in m {
            st.db
                .execute(
                    "UPDATE characters SET clan_uid = ?2 WHERE user_no = ?1",
                    rusqlite::params![u as i64, id],
                )
                .unwrap();
        }
    };
    clan("Home", 501, [na, nb]);
    clan("Away", 502, [nc, nd]);
    st.db
        .execute(
            "INSERT INTO clan_rooms (channel, room, home, away) VALUES (?1, ?2, 501, 502)",
            rusqlite::params![key.channel as i64, no as i64],
        )
        .unwrap();
    let id = 88;
    st.games.insert(
        id,
        game::Game {
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
    {
        let r = st.rooms.get_mut(&key).unwrap();
        r.game = Some(id);
        r.played_since = Some(std::time::Instant::now() - std::time::Duration::from_secs(600));
    }
    for (u, t, cl) in [(na, 1, 0), (nb, 1, 1), (nc, 2, 2), (nd, 2, 3)] {
        st.scope(|st| {
            game::on_line(st, id, &format!("@join {cl} {u} {t} {cl} N{u}"))?;
            game::on_line(
                st,
                id,
                &format!("@player uid={u} kills=2 deaths=1 team={t} tk=0 hs=0 name=N{u}"),
            )
        })
        .unwrap();
    }
    a.got.clear();
    (st, a, [na, nb], [nc, nd], id, key)
}

fn nx_of(st: &Core, u: [u32; 2]) -> [i64; 2] {
    u.map(|x| st.character(x).unwrap().nx)
}

fn clan_result(st: &mut Core, id: u64, win: u32) {
    st.scope(|st| {
        game::on_line(
            st,
            id,
            &format!("@result mode=TDM win={win} alpha=5 bravo=2 limit=5 rounds=1:0"),
        )
    })
    .unwrap();
}

// LG-311: the winners are paid at the end, see 0x345 at once and the box when back in the room; the losers get nothing
#[test]
fn nx_clan_win_pays_the_winning_side() {
    let (mut st, mut a, win, lose, id, _) = played_clan_room();
    clan_result(&mut st, id, 1);
    let n = st.rules.nx_clan_win;
    assert_eq!(nx_of(&st, win), [n, n]);
    assert_eq!(nx_of(&st, lose), [0, 0]);
    let b = a.take(BALANCE_REPLY);
    assert_eq!(u32_at(&b, 0x12), n as u32);
    a.send(&mut st, LOCATION, &u32s(&[1]));
    assert!(a.has(NOTICE_REPLY), "the box opens back in the room");
    a.got.clear();
    a.send(&mut st, LOCATION, &u32s(&[1]));
    assert!(!a.has(NOTICE_REPLY), "read once");
    // a second @result of the same game changes nothing
    clan_result(&mut st, id, 1);
    assert_eq!(nx_of(&st, win), [n, n]);
}

#[test]
fn nx_clan_win_guards_a_draw_a_short_match_a_forfeit_and_the_cap() {
    let (mut st, _, win, _, id, _) = played_clan_room();
    clan_result(&mut st, id, 0);
    assert_eq!(nx_of(&st, win), [0, 0], "draw");

    let (mut st, _, win, _, id, key) = played_clan_room();
    st.rooms.get_mut(&key).unwrap().played_since = Some(std::time::Instant::now());
    clan_result(&mut st, id, 1);
    assert_eq!(nx_of(&st, win), [0, 0], "too short");

    let (mut st, _, win, lose, id, _) = played_clan_room();
    st.games.get_mut(&id).unwrap().left.insert(lose[0]);
    clan_result(&mut st, id, 1);
    assert_eq!(nx_of(&st, win), [0, 0], "forfeit: one loser left");

    let (mut st, _, win, _, id, _) = played_clan_room();
    st.db
        .execute(
            "UPDATE characters SET clan_uid = 0 WHERE user_no = ?1",
            [win[0] as i64],
        )
        .unwrap();
    clan_result(&mut st, id, 1);
    assert_eq!(nx_of(&st, win), [0, 0], "one winner is no clan member");

    let (mut st, _, win, _, id, _) = played_clan_room();
    let day = crate::nxrewards::utc_day(crate::clock::unix_now());
    st.db
        .execute(
            "INSERT INTO nx_clan_wins (user_no, day, wins) VALUES (?1, ?2, ?3)",
            rusqlite::params![win[0] as i64, day, st.rules.nx_clan_win_daily_cap],
        )
        .unwrap();
    clan_result(&mut st, id, 1);
    let n = st.rules.nx_clan_win;
    assert_eq!(nx_of(&st, win), [0, n], "the capped member only misses out");
}

/// LG-185: `GET /net` lists the players still in a running game with the newest `@net` window of their client.
#[test]
fn get_net_lists_the_players_of_running_games_with_their_newest_link_window() {
    let (mut st, _a, _b, na, nb, id) = leaver_match(false);
    let feed = |st: &mut Core, line: &str| {
        st.scope(|st| game::on_line(st, id, line)).unwrap();
    };
    // a player without a report yet is listed with a null report age
    let v = dc_mod_lua::JsonValue::parse(&crate::lifecycle::net_json(&st)).unwrap();
    let p = v.as_object().unwrap()["players"].as_array().unwrap();
    assert_eq!(p.len(), 1, "the leaver is not listed");
    assert!(matches!(
        p[0].as_object().unwrap()["report_age_secs"],
        dc_mod_lua::JsonValue::Null
    ));
    feed(
        &mut st,
        &format!(
            "@net client=0 uid={na} ms=10000 rtt=50 rtt_min=40 rtt_max=90 rtt_n=12 jitter=3.5 input=30.0 gap=80 resends=1 drops=0 in=600 out=300"
        ),
    );
    feed(
        &mut st,
        &format!("@net client=0 uid={na} ms=10000 rtt=61 rtt_min=44 rtt_max=120 rtt_n=11 jitter=4.25 input=29.5 gap=95 resends=2 drops=1 in=590 out=310"),
    );
    feed(
        &mut st,
        &format!("@net client=1 uid={nb} ms=10000 rtt=9 rtt_min=9 rtt_max=9 rtt_n=1"),
    );
    let (status, body) = crate::admin::handle(
        &std::sync::Arc::new(std::sync::Mutex::new(st)),
        "GET /net HTTP/1.1

",
        Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
        None,
    );
    assert_eq!(status, 200, "{body}");
    let v = dc_mod_lua::JsonValue::parse(&body).unwrap();
    let o = v.as_object().unwrap();
    assert_eq!(o["games"].as_u32(), Some(1));
    let p = o["players"].as_array().unwrap();
    assert_eq!(p.len(), 1);
    let r = p[0].as_object().unwrap();
    assert_eq!(r["account"].as_u32(), Some(na));
    assert_eq!(r["nick"].as_str(), Some("Stayer"));
    assert_eq!(r["client"].as_str(), Some("0"));
    let rtt = r["rtt_ms"].as_object().unwrap();
    assert_eq!(
        (
            rtt["mean"].as_f64(),
            rtt["min"].as_f64(),
            rtt["max"].as_f64(),
            rtt["samples"].as_u32()
        ),
        (Some(61.0), Some(44.0), Some(120.0), Some(11)),
        "the newest window wins"
    );
    assert_eq!(r["input_jitter_ms"].as_f64(), Some(4.3));
    assert_eq!(
        (
            r["resends"].as_u32(),
            r["flow_drops"].as_u32(),
            r["datagrams_in"].as_u32(),
            r["datagrams_out"].as_u32()
        ),
        (Some(2), Some(1), Some(590), Some(310))
    );
    assert!(r["tcp_age_secs"].as_f64().is_some(), "{body}");
    assert!(r["report_age_secs"].as_f64().is_some_and(|a| a < 5.0));
}

fn result_line_for(st: &mut Core, id: u64) {
    st.scope(|st| {
        game::on_line(
            st,
            id,
            "@result mode=TDM win=1 alpha=5 bravo=2 limit=5 rounds=1:0",
        )
    })
    .unwrap();
}

#[test]
fn a_leaver_gets_no_experience_stats_or_push() {
    // DC-DEFAULT: REW-1 (project decision
    let (mut st, mut a, mut b, na, nb, id) = leaver_match(false);
    let (ea, eb) = (
        crate::db::character(&st.db, na).unwrap().unwrap(),
        crate::db::character(&st.db, nb).unwrap().unwrap(),
    );
    result_line_for(&mut st, id);
    let (a2, b2) = (
        crate::db::character(&st.db, na).unwrap().unwrap(),
        crate::db::character(&st.db, nb).unwrap().unwrap(),
    );
    assert!(a2.exp > ea.exp && a2.gp > ea.gp, "the stayer is paid");
    assert_eq!(a2.kills, ea.kills + 3);
    assert!(a.has(PROGRESS_PUSH), "the stayer gets 0x3004");
    assert_eq!((b2.exp, b2.gp), (eb.exp, eb.gp), "the leaver is not paid");
    assert_eq!((b2.kills, b2.deaths), (eb.kills, eb.deaths));
    assert_eq!((b2.wins, b2.losses), (eb.wins, eb.losses));
    assert!(!b.has(PROGRESS_PUSH), "no 0x3004 to the leaver");
}

#[test]
fn pay_leavers_switches_the_rule_off() {
    let (mut st, mut a, mut b, na, nb, id) = leaver_match(true);
    let eb = crate::db::character(&st.db, nb).unwrap().unwrap();
    let ea = crate::db::character(&st.db, na).unwrap().unwrap();
    result_line_for(&mut st, id);
    let b2 = crate::db::character(&st.db, nb).unwrap().unwrap();
    assert!(b2.exp > eb.exp && b2.gp > eb.gp);
    assert!(crate::db::character(&st.db, na).unwrap().unwrap().exp > ea.exp);
    assert!(a.has(PROGRESS_PUSH) && b.has(PROGRESS_PUSH));
}

/// LG-317 / LG-320: a bot from a player's PC earns nothing, a person alone with it is paid; the same bot on this machine
/// (the operator's) pays as before.
#[test]
fn a_bot_from_a_players_pc_earns_nothing_and_a_lone_player_is_paid() {
    let run = |peer: &str| {
        let (mut st, _a, _b, na, nb, id) = leaver_match(true);
        st.db
            .execute(
                "UPDATE accounts SET self_bot = 1 WHERE user_no = ?1",
                [nb as i64],
            )
            .unwrap();
        for s in st.sessions.values_mut() {
            if s.account == Some(nb) {
                s.peer = peer.into();
            }
        }
        let ea = crate::db::character(&st.db, na).unwrap().unwrap();
        let eb = crate::db::character(&st.db, nb).unwrap().unwrap();
        result_line_for(&mut st, id);
        let a2 = crate::db::character(&st.db, na).unwrap().unwrap();
        let b2 = crate::db::character(&st.db, nb).unwrap().unwrap();
        (
            a2.exp > ea.exp,
            b2.exp > eb.exp,
            a2.gp > ea.gp,
            b2.gp > eb.gp,
        )
    };
    assert_eq!(run("203.0.113.9:5000"), (true, false, true, false));
    assert_eq!(run("127.0.0.1:5000"), (true, true, true, true));
}

#[test]
fn a_player_who_rejoins_before_the_end_is_not_a_leaver() {
    let (mut st, _a, mut b, _na, nb, id) = leaver_match(false);
    let eb = crate::db::character(&st.db, nb).unwrap().unwrap();
    st.scope(|st| game::on_line(st, id, &format!("@join 2 {nb} 2 1 Leaver")))
        .unwrap();
    // DSV-621: the game server reports the new stay only (its counters started again at the rejoin)
    st.scope(|st| {
        game::on_line(
            st,
            id,
            &format!("@player uid={nb} kills=1 deaths=0 team=2 tk=0 hs=0 name=Leaver"),
        )
    })
    .unwrap();
    result_line_for(&mut st, id);
    let b2 = crate::db::character(&st.db, nb).unwrap().unwrap();
    assert!(b2.exp > eb.exp);
    assert_eq!(
        (b2.kills, b2.deaths),
        (eb.kills + 1, eb.deaths),
        "the second stay's line"
    );
    assert!(b.has(PROGRESS_PUSH));
}

/// The leaver match with the stayer 5 exp below rank 1 (the fallback table: 100 exp), rank 1 worth 1000 GP, a shop
/// list of products 2 and 3, and `gifts` as `rewards.rank_gifts`.
fn promotion_match(gifts: Vec<(u32, Vec<i32>)>) -> (Core, Client, u32, u64) {
    let (mut st, a, _b, na, _nb, id) = leaver_match(false);
    let rom = std::sync::Arc::get_mut(&mut st.rom).expect("the test owns the ROM tables");
    rom.ranks.bonus = vec![0, 1000];
    rom.shop.catalog = Some(dc_bute::ShopCatalog::from_doc(&dc_bute::ButeDoc::parse(
        "[ItemInfo0]\nItemindex=0\nItemid=110\nItemtype=0\n\
         [ItemInfo1]\nItemindex=1\nItemid=9104\nItemtype=21\n\
         [Product0]\nProductID=2\nSaleType=1\nShopview=1\nPrice=50\nSalePrice=50\nItemnum=1\n\
         Itemindex0=1\n\
         Consumetype0=1\nPeriod0=7\n\
         [Product1]\nProductID=3\nSaleType=0\nShopview=1\nPrice=50\nSalePrice=50\nItemnum=1\n\
         Itemindex0=0\n\
         Consumetype0=0\nPeriod0=30\n",
    )));
    st.rules.rank_gifts = gifts;
    st.db
        .execute(
            "UPDATE characters SET exp = 95 WHERE user_no = ?1",
            [na as i64],
        )
        .unwrap();
    (st, a, na, id)
}

fn inbox_rows(st: &Core, account: u32) -> Vec<i64> {
    let mut q = st
        .db
        .prepare("SELECT product_id FROM inbox WHERE user_no = ?1 ORDER BY uproduct")
        .unwrap();
    q.query_map([account as i64], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// LG-303 / CD-89 / CD-41: a match result that raises the stayer from rank 0 to rank 1 pays rank 1's GP bonus, puts
/// rank 1's gift products into the inbox (one row each), sends 0x3004 with the popup flag (box 0x185f names the
/// inbox) and then 0x3014; the rank is stored, so a second crossing of it pays nothing.
#[test]
fn a_promotion_delivers_the_rank_gifts_and_bonus_once() {
    let (mut st, mut a, na, id) = promotion_match(vec![(1, vec![2, 3, 999]), (2, vec![3])]);
    let gp = crate::db::character(&st.db, na).unwrap().unwrap().gp;
    result_line_for(&mut st, id);
    let c = crate::db::character(&st.db, na).unwrap().unwrap();
    assert_eq!(st.rank_of(c.exp), 1);
    // one row per known product of rank 1 (999 is not in the shop list); rank 2 is not reached
    assert_eq!(inbox_rows(&st, na), vec![2, 3]);
    assert_eq!(crate::db::rank_rewarded(&st.db, na).unwrap(), 1);
    let d = a.take(PROGRESS_PUSH);
    assert_eq!(
        u32_at(&d, 0xe),
        1,
        "the popup flag: box 0x185f names the inbox"
    );
    assert_eq!(
        u32_at(&d, 0xa) as i64,
        c.gp,
        "the push carries the GP with the bonus"
    );
    assert!(
        c.gp >= gp + 1000,
        "rank 1's bonus is paid: {gp} -> {}",
        c.gp
    );
    assert!(
        a.has(crate::shopmsg::INBOX_STALE_PUSH),
        "0x3014 after the gifts"
    );
    // the same rank crossed again (an exp loss and a new rank-up) pays nothing
    let again = crate::rewards::promote(&mut st, na, 0, 1).unwrap();
    assert_eq!(again, crate::rewards::Promotion::default());
    assert_eq!(inbox_rows(&st, na).len(), 2);
    assert_eq!(crate::db::character(&st.db, na).unwrap().unwrap().gp, c.gp);
    // the next rank pays its own gift once
    let next = crate::rewards::promote(&mut st, na, 1, 2).unwrap();
    assert_eq!(next.gift_rows, 1);
    assert_eq!(inbox_rows(&st, na), vec![2, 3, 3]);
    assert_eq!(
        crate::rewards::promote(&mut st, na, 1, 2)
            .unwrap()
            .gift_rows,
        0
    );
}

/// LG-303: without a gift for the rank the bonus is paid, the popup flag stays 0 and no 0x3014 is sent.
#[test]
fn a_promotion_without_a_gift_pays_the_bonus_but_claims_no_inbox_items() {
    let (mut st, mut a, na, id) = promotion_match(vec![(5, vec![2])]);
    let gp = crate::db::character(&st.db, na).unwrap().unwrap().gp;
    result_line_for(&mut st, id);
    let c = crate::db::character(&st.db, na).unwrap().unwrap();
    assert_eq!(st.rank_of(c.exp), 1);
    assert!(inbox_rows(&st, na).is_empty());
    let d = a.take(PROGRESS_PUSH);
    assert_eq!(u32_at(&d, 0xe), 0, "no gift: no 0x185f");
    assert!(c.gp >= gp + 1000);
    assert!(!a.has(crate::shopmsg::INBOX_STALE_PUSH));
    // economy.rank_bonus = false: no bonus
    let (mut st, _a, na, id) = promotion_match(Vec::new());
    st.rules.rank_bonus = false;
    let gp = crate::db::character(&st.db, na).unwrap().unwrap().gp;
    result_line_for(&mut st, id);
    let c = crate::db::character(&st.db, na).unwrap().unwrap();
    assert!(c.gp > gp && c.gp < gp + 1000, "{gp} -> {}", c.gp);
}

/// A synthetic shop list over the fallback rank table (rank 1 from 100 exp, rank 2 from 218, rank 3 from 357): item
/// 110 unlocks at 100 (rank 1) in 1-, 7- and 30-day and permanent GP variants, item 111 at 150 (between ranks 1 and
/// 2: rank 2), item 112 at 100 but only for credit (no GP unlock), and two NX "Supply Case" products (one a pack of
/// 5) plus a GP "Supply Case".
const PROMO_SHOP: &str = "
[ItemInfo0]
Itemindex=0
Itemid=110
Itemtype=0
GP_Unlock=100
[ItemInfo1]
Itemindex=1
Itemid=111
Itemtype=0
GP_Unlock=150
[ItemInfo2]
Itemindex=2
Itemid=112
Itemtype=0
NX_Unlock=100
[ItemInfo3]
Itemindex=3
Itemid=8001
Itemtype=20
[ItemInfo4]
Itemindex=4
Itemid=8002
Itemtype=20
[Product0]
ProductID=10
SaleType=0
Shopview=1
Price=10
Itemnum=1
Itemindex0=0
Consumetype0=0
Period0=30
[Product1]
ProductID=11
SaleType=0
Shopview=1
Price=10
Itemnum=1
Itemindex0=0
Consumetype0=0
Period0=7
[Product2]
ProductID=12
SaleType=0
Shopview=1
Price=10
Itemnum=1
Itemindex0=0
Consumetype0=0
Period0=0
[Product3]
ProductID=13
SaleType=0
Shopview=1
Price=10
Itemnum=1
Itemindex0=1
Consumetype0=0
Period0=0
[Product4]
ProductID=14
SaleType=1
Shopview=1
Price=10
Itemnum=1
Itemindex0=2
Consumetype0=0
Period0=7
[Product5]
ProductName=\"Supply Case AK-N\"
ProductID=20
SaleType=1
Shopview=1
Price=10
Itemnum=1
Itemindex0=3
Consumetype0=2
Period0=1
[Product6]
ProductName=\"Supply Case AK-N\"
ProductID=21
SaleType=1
Shopview=1
Price=10
Itemnum=1
Itemindex0=3
Consumetype0=2
Period0=5
[Product7]
ProductName=\"Supply Case P90-N\"
ProductID=22
SaleType=1
Shopview=1
Price=10
Itemnum=1
Itemindex0=4
Consumetype0=2
Period0=1
[Product8]
ProductName=\"Supply Case Gray\"
ProductID=23
SaleType=0
Shopview=1
Price=10
Itemnum=1
Itemindex0=4
Consumetype0=2
Period0=1
[Product9]
ProductID=15
SaleType=0
Shopview=1
Price=10
Itemnum=1
Itemindex0=0
Consumetype0=0
Period0=1
";

/// CD-89 (project decision: rank 1 unlocks item 110: one row, its 7-day variant (product 11; with
/// `rank_unlock_days` 3 the shortest, 1 day: product 15); rank 2 first reaches item 111's unlock between the ranks:
/// its only (permanent) product 13; rank 3 unlocks nothing: one NX supply case of the pool {20, 22} (not the pack 21,
/// not the GP case 23); an override rank gives the override only; each once per character.
#[test]
fn a_promotion_gives_the_unlocked_products_else_one_nx_supply_case() {
    let mut st = state();
    std::sync::Arc::get_mut(&mut st.rom).unwrap().shop.catalog = Some(
        dc_bute::ShopCatalog::from_doc(&dc_bute::ButeDoc::parse(PROMO_SHOP)),
    );
    let (_a, na) = player(&mut st, 1, 2, "pr1", "Promoted");
    assert_eq!(crate::rewards::case_pool(&st), vec![20, 22]);
    let p = crate::rewards::promote(&mut st, na, 0, 1).unwrap();
    assert_eq!(p.gift_rows, 1);
    assert_eq!(inbox_rows(&st, na), vec![11]);
    let p = crate::rewards::promote(&mut st, na, 1, 3).unwrap();
    assert_eq!(p.gift_rows, 2);
    let rows = inbox_rows(&st, na);
    assert_eq!(&rows[..2], &[11, 13]);
    assert!([20, 22].contains(&rows[2]), "{rows:?}");
    // once per character
    assert_eq!(
        crate::rewards::promote(&mut st, na, 0, 3)
            .unwrap()
            .gift_rows,
        0
    );
    assert_eq!(inbox_rows(&st, na).len(), 3);
    // another character: the shortest period without a variant of the configured days; rank 1 overridden
    let (_b, nb) = player(&mut st, 3, 4, "pr2", "Other");
    st.rules.rank_unlock_days = 3;
    assert_eq!(
        crate::rewards::rank_products(&mut st, nb, 1).unwrap(),
        vec![15]
    );
    st.rules.rank_gifts = vec![(1, vec![14])];
    let p = crate::rewards::promote(&mut st, nb, 0, 1).unwrap();
    assert_eq!((p.gift_rows, inbox_rows(&st, nb)), (1, vec![14]));
    // a pinned pool; an item owned permanently is skipped (rank 2 then unlocks nothing and gives the case)
    st.rules.rank_case_products = vec![21];
    st.db
        .execute(
            "INSERT INTO items (user_no, item_no) VALUES (?1, 111)",
            [nb as i64],
        )
        .unwrap();
    assert!(crate::rewards::unlocked_products(&st, nb, 2)
        .unwrap()
        .is_empty());
    let p = crate::rewards::promote(&mut st, nb, 1, 2).unwrap();
    // the pinned pack of 5 splits into one case per row (CD-02)
    assert_eq!(
        (p.gift_rows, inbox_rows(&st, nb)),
        (5, vec![14, 21, 21, 21, 21, 21])
    );
}

/// CD-89 on the user's ROM: for every rank the unlocked products follow the shop list (each a shown GP product of an
/// item whose `GP_Unlock` lies between the previous rank's and this rank's experience, one per item, the 7-day
/// variant when the item has one); a rank without one gives one product of the NX supply case pool (credit
/// "Supply Case ..." single cases); prints what it resolved.
#[test]
fn rom_promotions_follow_the_shop_lists_unlocks() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut st = state();
    *std::sync::Arc::get_mut(&mut st.rom).unwrap() = crate::rom::RomData::load(&rom);
    let (_a, na) = player(&mut st, 1, 2, "rp1", "RomPromoted");
    let pool = crate::rewards::case_pool(&st);
    let cat = st.rom.shop.catalog.clone().expect("shop list");
    let name = |pid: i32| cat.product(pid).map_or(String::new(), |p| p.name.clone());
    println!("NX supply case pool ({}):", pool.len());
    for p in &pool {
        println!("  case {p} {}", name(*p));
    }
    assert!(!pool.is_empty());
    for &p in &pool {
        let pr = cat.product(p).unwrap();
        assert_eq!(pr.sale_type, 1);
        assert!(pr.name.contains("Supply Case"), "{}", pr.name);
        let items = crate::shop::product_items(&cat, pr);
        assert!(crate::shop::is_case(&items));
        let awards = st
            .rom
            .items
            .as_ref()
            .unwrap()
            .supply_pool(items[0].item_no as i32);
        assert!(
            awards.iter().all(|i| (0..1000).contains(i)),
            "{p}: {awards:?}"
        );
    }
    let need = st.rom.ranks.need.clone();
    let owned = crate::db::items(&st.db, na).unwrap();
    let (mut unlocking, mut empty) = (0, 0);
    for rank in 1..need.len() as u32 {
        let (lo, hi) = (
            if rank == 1 {
                0
            } else {
                need[rank as usize - 2]
            },
            need[rank as usize - 1],
        );
        let mut want_items = std::collections::BTreeSet::new();
        for pid in 0..0x10000 {
            let Some(p) = cat.product(pid) else { continue };
            let u = crate::shop::unlock_exp(&cat, p);
            if p.sale_type == 0 && crate::shop::is_shown(&st, pid) && u > lo && u <= hi {
                let item = crate::shop::product_items(&cat, p)[0].item_no;
                let skip = st.rom.starter.iter().any(|s| s.item_no == item)
                    || owned.iter().any(|i| {
                        i.item_no == item
                            && (i.default_type != 0 || i.end_time == 0 && i.remain == 0)
                    });
                if !skip {
                    want_items.insert(item);
                }
            }
        }
        let got = crate::rewards::rank_products(&mut st, na, rank).unwrap();
        if want_items.is_empty() {
            empty += 1;
            assert_eq!(got.len(), 1, "rank {rank}: one case");
            assert!(pool.contains(&got[0]), "rank {rank}: {got:?}");
            println!(
                "rank {rank}: nothing unlocks -> case {} {}",
                got[0],
                name(got[0])
            );
            continue;
        }
        unlocking += 1;
        let items: std::collections::BTreeSet<i64> = got
            .iter()
            .map(|p| crate::shop::product_items(&cat, cat.product(*p).unwrap())[0].item_no)
            .collect();
        assert_eq!(items, want_items, "rank {rank}");
        assert_eq!(got.len(), items.len(), "one product per item");
        for &p in &got {
            let pr = cat.product(p).unwrap();
            let item = crate::shop::product_items(&cat, pr)[0].item_no;
            let has7 = (0..0x10000).any(|q| {
                cat.product(q).is_some_and(|o| {
                    o.sale_type == 0
                        && crate::shop::is_shown(&st, q)
                        && o.period == 7
                        && crate::shop::product_items(&cat, o)[0].item_no == item
                })
            });
            if has7 {
                assert_eq!(pr.period, 7, "rank {rank} product {p}");
            }
        }
        println!(
            "rank {rank}: {:?}",
            got.iter()
                .map(|p| (*p, name(*p), cat.product(*p).unwrap().period))
                .collect::<Vec<_>>()
        );
    }
    println!("{unlocking} rank(s) unlock products, {empty} give a case");
    // the deliveries themselves, once
    let p = crate::rewards::promote(&mut st, na, 0, 3).unwrap();
    assert!(p.gift_rows >= 3);
    assert_eq!(
        crate::rewards::promote(&mut st, na, 0, 3)
            .unwrap()
            .gift_rows,
        0
    );
}

#[test]
fn start_items_land_in_a_new_player_inventory_unequipped() {
    // DC-DEFAULT: CD-99
    let mut st = state();
    st.opts.start_items = "4321, 333".into();
    let (mut a, na) = player(&mut st, 1, 2, "si1", "Stocked");
    let items = crate::db::items(&st.db, na).unwrap();
    for n in [4321, 333] {
        assert!(
            items.iter().any(|i| i.item_no == n && i.slot == -1),
            "{n}: {items:?}"
        );
    }
    assert!(!slots_of(&mut a, &mut st).contains(&4321), "not equipped");
    // a second login does not add them again
    let _ = player(&mut st, 3, 4, "si1", "Stocked");
    let again = crate::db::items(&st.db, na).unwrap();
    assert_eq!(again.iter().filter(|i| i.item_no == 4321).count(), 1);
    // bots get none, and a bad list is an error
    let mut o = st.opts.clone();
    assert!(
        crate::grant::apply_start_items(&st.db, &o, "bot12", 9001, 1)
            .unwrap()
            .is_empty()
    );
    o.start_items = "nonsense".into();
    assert!(crate::grant::apply_start_items(&st.db, &o, "alice", 9002, 1).is_err());
}

#[test]
fn start_loadout_equips_the_seeded_items_on_a_new_character() {
    // DC-DEFAULT: LG-4 (proposed, weapon-sweep) --start-loadout; the 0x323 slot view shows the seeded items
    let mut st = state();
    st.opts.start_loadout = "0:4321,3:333".into();
    let (mut a, na) = player(&mut st, 1, 2, "sl1", "Seeded");
    let s = slots_of(&mut a, &mut st);
    assert_eq!(&s[..4], &[4321, 102, 103, 333], "{s:?}");
    // the displaced starter items stay in the inventory, unequipped
    let items = crate::db::items(&st.db, na).unwrap();
    assert!(items.iter().any(|i| i.item_no == 101 && i.slot == -1));
    assert!(items.iter().any(|i| i.item_no == 104 && i.slot == -1));
    // a second login keeps the seed (the default-slot repair only fills empty slots)
    let (mut b, _) = player(&mut st, 3, 4, "sl1", "Seeded");
    assert_eq!(&slots_of(&mut b, &mut st)[..4], &[4321, 102, 103, 333]);
}

#[test]
fn bot_accounts_get_the_bot_loadout_and_users_the_start_loadout() {
    // DC-DEFAULT: LG-7 (proposed, bot-loadout) --bot-start-loadout for Bot<digits> accounts
    let mut st = state();
    st.opts.start_loadout = "0:4321".into();
    st.opts.bot_start_loadout = "0:5555".into();
    let (mut a, _) = player(&mut st, 1, 2, "Bot3", "Bot3");
    assert_eq!(slots_of(&mut a, &mut st)[0], 5555);
    let (mut b, _) = player(&mut st, 3, 4, "alice", "Alice");
    assert_eq!(slots_of(&mut b, &mut st)[0], 4321);
    let (mut c, _) = player(&mut st, 5, 6, "botx", "botx");
    assert_eq!(slots_of(&mut c, &mut st)[0], 4321, "bot without digits");
    // flag empty: Bot3 gets the ordinary start loadout
    let mut st = state();
    st.opts.start_loadout = "0:4321".into();
    let (mut d, _) = player(&mut st, 1, 2, "Bot3", "Bot3");
    assert_eq!(slots_of(&mut d, &mut st)[0], 4321);
}

#[test]
fn launcher_bots_lowercase_login_gets_the_bot_loadout() {
    // DC-DEFAULT: LG-9 (proposed, dsv-fixes8) the launcher passes --name Bot1; dc-botclient logs in as `bot1`
    let mut st = state();
    st.opts.start_loadout = "0:4321".into();
    st.opts.bot_start_loadout = "0:5555".into();
    let (mut a, _) = player(&mut st, 1, 2, "bot1", "bot1");
    assert_eq!(slots_of(&mut a, &mut st)[0], 5555);
}

#[test]
fn rom_start_loadout_puts_a_bare_weapon_in_its_rom_slot_without_the_shop() {
    // gated: needs CA_ROM. A weapon row that no shop product sells is still equipped (the seed bypasses the
    // catalogue, shop.show_hidden_products stays false)
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut st = state();
    st.rom = std::sync::Arc::new(crate::rom::RomData::load(&rom));
    assert!(!st.rules.shop.show_hidden_products);
    let mut kinds: Vec<(i64, i64)> = st
        .rom
        .weapon_kinds
        .keys()
        .filter_map(|&n| st.rom.slot_of_item(n).map(|s| (n, s)))
        .filter(|&(_, s)| s == 1)
        .collect();
    kinds.sort_unstable();
    let &(pistol, _) = kinds.last().expect("a secondary weapon row");
    st.opts.start_loadout = format!("{pistol}");
    let (mut a, _) = player(&mut st, 1, 2, "sl2", "SeededRom");
    let s = slots_of(&mut a, &mut st);
    assert_eq!(s[1] as i64, pistol, "{s:?}");
    // the wrong slot is a usage error at start-up
    st.opts.start_loadout = format!("0:{pistol}");
    assert!(crate::grant::start_loadout(&st.opts, &st.rom).is_err());
}

/// LG-51 (playtest report: "character doesn't show default items under body"): an account owns the default uniform as
/// a default item equipped in slot 7, also one made before it existed (login repair). LG-90 (playtest report: "no vest
/// is default") / LG-120 (playtest report: "default is no hat"): no default vest or head, and the login repair removes
/// the ones an older account holds.
#[test]
fn rom_accounts_own_default_body_items() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut st = state();
    st.rom = std::sync::Arc::new(crate::rom::RomData::load(&rom));
    let (mut a, _) = player(&mut st, 1, 2, "bd1", "BodyDefault");
    let s = slots_of(&mut a, &mut st);
    assert!(s[7] >= 2000, "uniform default equipped: {s:?}");
    assert!(!(3000..4000).contains(&s[8]), "no default vest: {s:?}");
    assert!(!(4000..5000).contains(&s[4]), "no default head: {s:?}");
    // an old account: the uniform default is gone from its rows and it holds the old default vest and head in
    // slots 8 and 4; repair gives the uniform back and takes the vest and the head away
    let account = 2u32;
    st.db
        .execute(
            "DELETE FROM items WHERE user_no = ?1 AND default_type >= 7",
            [account as i64],
        )
        .unwrap();
    crate::db::add_item(
        &st.db,
        &crate::db::NewItem {
            user_no: account,
            item_no: 3000,
            start_time: 0,
            end_time: 0,
            slot: 8,
            default_type: crate::rom::DEFAULT_KIND_VEST,
        },
    )
    .unwrap();
    crate::db::add_item(
        &st.db,
        &crate::db::NewItem {
            user_no: account,
            item_no: 4000,
            start_time: 0,
            end_time: 0,
            slot: 4,
            default_type: crate::rom::DEFAULT_KIND_HEAD,
        },
    )
    .unwrap();
    crate::inventory::repair_defaults(&st, account).unwrap();
    let items = crate::db::items(&st.db, account).unwrap();
    let mut kinds: Vec<i64> = items
        .iter()
        .filter(|i| i.default_type >= 7)
        .map(|i| i.default_type)
        .collect();
    kinds.sort();
    assert_eq!(kinds, vec![7], "{kinds:?}");
    assert!(
        !items.iter().any(|i| i.slot == 8 || i.slot == 4),
        "the vest and head slots are empty: {items:?}"
    );
}

/// A model of the 2009 waiting room's pending marks: a 0x38c of type 3 or 4 marks types 3, 4, 6 and 8, the other
/// option types mark only themselves; every 0x3035 clears the mark of its own type, a 0x38d {0} clears them all.
// EVIDENCE: cshell+0x1614f0 (marks set per request type), cshell+0x168680 (0x3035 clears its type),
// cshell+0x162370 (0x38d +6 == 0 clears all)
fn pending_after(request_type: u32, frames: &[(u16, Vec<u8>)]) -> Vec<u32> {
    let mut marks: Vec<u32> = match request_type {
        3 | 4 => vec![3, 4, 6, 8],
        t => vec![t],
    };
    for (id, d) in frames {
        if *id == ROOM_UPDATE_PUSH {
            let t = u32_at(d, 0xa);
            marks.retain(|m| *m != t);
        } else if *id == ROOM_SETTING_CHANGE_REPLY && u32_at(d, 6) == 0 {
            marks.clear();
        }
    }
    marks
}

#[test]
fn every_room_option_request_leaves_no_pending_mark_on_the_hosts_client() {
    // OBSERVED: capture 20261025_104223_rust5-auto (0x38c type 3 answered by 0x3035 type 3
    // only; the menu agent's goal option (type 8) then timed out because the client still held marks 4 / 6 / 8)
    // vs 20261025_101848_menu-frida8 (0x3035 types 3, 4, 6, 8 after the type 3 request; the goal request followed)
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "ro1a", "OptHost");
    let (mut b, _) = player(&mut st, 3, 4, "ro1b", "OptGuest");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    // map, mode, goal, max players, weapon limit (the agent's option kinds 6, 7, 8, 9 and the weapon-limit list)
    for (t, v) in [(3, 2), (4, 1), (8, 140), (6, 12), (0xe, 1), (3, 0)] {
        a.pump();
        b.pump();
        a.got.clear();
        b.got.clear();
        a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, t, v]));
        let frames = a.got.clone();
        assert_eq!(
            pending_after(t, &frames),
            Vec::<u32>::new(),
            "type {t}: {frames:?}"
        );
        assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1, "type {t}");
    }
    let r = &st.rooms[&key];
    assert_eq!(
        (r.map, r.mode, r.goal, r.max_players, r.weapon_limit),
        (0, 1, 140, 12, 1)
    );
    // a map change echoes the linked fields with the room's current values, map first; the guest sees them too
    b.pump();
    b.got.clear();
    a.got.clear();
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 3, 4]));
    b.pump();
    let ups: Vec<(u32, u32)> = b
        .all(ROOM_UPDATE_PUSH)
        .iter()
        .map(|d| (u32_at(d, 0xa), u32_at(d, 0xe)))
        .collect();
    assert_eq!(ups, vec![(3, 4), (4, 1), (6, 12), (8, 140)]);
    // a mode change echoes map, max players and goal
    a.got.clear();
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 4, 0]));
    let ups: Vec<u32> = a
        .all(ROOM_UPDATE_PUSH)
        .iter()
        .map(|d| u32_at(d, 0xa))
        .collect();
    assert_eq!(ups, vec![4, 3, 6, 8]);
    // a refused map change (guest) answers 0x38d {0}: the client clears all marks itself
    b.pump();
    b.got.clear();
    b.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 3, 1]));
    assert_eq!(pending_after(3, &b.got.clone()), Vec::<u32>::new());
    assert!(!b.has(ROOM_UPDATE_PUSH));
}

/// CLI-1905: the 0x38c password form as the client's lock box sends it (+6 mode, +0xa type 1, +0xe 0, +0x12 the
/// length as 4 bytes, +0x16 wstr[33]; 0x58 bytes with the header).
fn password_form(pw: &str) -> Vec<u8> {
    let mut b = u32s(&[0, field::LOCKED, 0]);
    b.extend((pw.encode_utf16().count() as u32).to_le_bytes());
    b.extend(wstr(pw, 33));
    assert_eq!(b.len() + 6, 0x58);
    b
}

#[test]
fn the_hosts_lock_box_sets_and_clears_the_room_password() {
    // SPEC: PROTOCOL_2009.md §3.7 (0x38c password form); EVIDENCE: cshell+0x18a9c0
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "pw1a", "LockHost");
    let (mut b, _) = player(&mut st, 3, 4, "pw1b", "LockGuest");
    let (mut c, _) = player(&mut st, 5, 6, "pw1c", "LockThird");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    // a guest's form is refused and changes nothing
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    b.send(&mut st, ROOM_SETTING_CHANGE, &password_form("zz99"));
    assert_eq!(u32_at(&b.take(ROOM_SETTING_CHANGE_REPLY), 6), 0);
    assert!(st.rooms[&key].password.is_empty());
    // the host locks the room: ok, the room pushes locked = 1
    a.pump();
    a.got.clear();
    a.send(&mut st, ROOM_SETTING_CHANGE, &password_form("ab12"));
    a.pump();
    let locked: Vec<u32> = a
        .all(ROOM_UPDATE_PUSH)
        .iter()
        .filter(|d| u32_at(d, 0xa) == field::LOCKED)
        .map(|d| u32_at(d, 0xe))
        .collect();
    assert_eq!(locked, vec![1]);
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    assert_eq!(st.rooms[&key].password, "ab12");
    // a third player needs it
    assert_ne!(enter(&mut st, &mut c, no, "").0, 1);
    assert_ne!(enter(&mut st, &mut c, no, "ab13").0, 1);
    assert_eq!(enter(&mut st, &mut c, no, "ab12").0, 1);
    // an empty form unlocks
    a.got.clear();
    a.send(&mut st, ROOM_SETTING_CHANGE, &password_form(""));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    assert!(st.rooms[&key].password.is_empty());
}

// synthetic room table: an FFA row (class 6) and a TDM row (class 1)
const FFA_TABLE: &str = r"[Map0]
Enable=TRUE
GMS_FFA=a
GMS_TDM=b
[Mode0]
Key=FFA
GameType=6
[Mode1]
Key=TDM
GameType=1
";

/// CLI-782: a team change (0x390) in a room of a free-for-all mode class is refused (0x391 {0, 0}) and the member
/// keeps his team; in a team mode room it is granted.
#[test]
fn a_team_change_in_a_free_for_all_room_is_refused() {
    let mut st = state();
    std::sync::Arc::get_mut(&mut st.rom).unwrap().game_room =
        dc_bute::GameRoomData::from_doc(dc_bute::ButeDoc::parse(FFA_TABLE));
    let (mut a, na) = player(&mut st, 1, 2, "ffa1", "Ffa");
    open_room(&mut st, &mut a, "");
    let team_of = |st: &Core| {
        st.rooms
            .values()
            .find_map(|r| r.member(na).map(|m| m.team))
            .unwrap()
    };
    let set_mode = |st: &mut Core, mode: u32| st.rooms.values_mut().for_each(|r| r.mode = mode);
    let before = team_of(&st);
    let other: u32 = if before == 1 { 2 } else { 1 };
    set_mode(&mut st, 0);
    let mut b = vec![0u8; 8];
    b.extend(other.to_le_bytes());
    a.send(&mut st, TEAM_CHANGE, &b);
    let r = a.take(TEAM_CHANGE_REPLY);
    assert_eq!((u32_at(&r, 6), u32_at(&r, 0xa)), (0, 0), "FFA: refused");
    assert_eq!(team_of(&st), before);
    set_mode(&mut st, 1);
    a.send(&mut st, TEAM_CHANGE, &b);
    assert_eq!(u32_at(&a.take(TEAM_CHANGE_REPLY), 6), 1, "TDM: granted");
    assert_eq!(team_of(&st), other);
}

/// True when `d` holds `s` as UTF-16LE.
fn has_wide(d: &[u8], s: &str) -> bool {
    let w: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    d.windows(w.len()).any(|x| x == w.as_slice())
}

fn set_addr(st: &mut Core, sid: Sid, peer: &str, local: &str) {
    let s = st.sessions.get_mut(&sid).unwrap();
    s.peer = peer.into();
    s.local = local.into();
}

// LG-30: a loopback / LAN peer is told the address it reached us at, a remote peer the configured address,
// both in the server list and in the match address push.
#[test]
fn announced_address_follows_the_peer() {
    let mut st = state();
    st.opts.lobbyip = "203.0.113.7".into();
    st.opts.gameip = "203.0.113.7".into();
    // the game server binds every address (the public-hosting launcher passes this)
    st.opts.dserver_args = "-bindip 0.0.0.0".into();
    // auth login from a remote peer: the server-list push carries the public address
    let mut r = Client {
        sid: 50,
        rx: connect(&mut st, 50, Listener::Auth),
        got: Vec::new(),
    };
    set_addr(&mut st, 50, "198.51.100.4:6000", "192.168.1.20:10000");
    let mut body = wstr("LOCAL:remote", 4096);
    body.extend(wstr("pw", 17));
    body.extend(wstr("00-11", 20));
    body.extend(0i32.to_le_bytes());
    r.send(&mut st, LOGIN_REQUEST, &body);
    if u32_at(&r.take(LOGIN_REPLY), 6) == 0 {
        let mut b = 6002i32.to_le_bytes().to_vec();
        b.extend(wstr("Remote", 17));
        r.send(&mut st, NEW_CHAR, &b);
    }
    let push = r.take(SERVER_LIST_PUSH);
    assert!(has_wide(&push, "203.0.113.7"));
    assert!(!has_wide(&push, "192.168.1.20"));
    st.sessions.remove(&50);

    let (mut a, _) = player(&mut st, 1, 2, "hostp", "Hostp");
    let (mut b, _) = player(&mut st, 3, 4, "lanp", "Lanp");
    let (mut c, _) = player(&mut st, 5, 6, "farp", "Farp");
    set_addr(&mut st, a.sid, "127.0.0.1:50000", "127.0.0.1:10002");
    set_addr(&mut st, b.sid, "192.168.1.30:50000", "192.168.1.20:10002");
    set_addr(&mut st, c.sid, "198.51.100.4:50000", "192.168.1.20:10002");
    for (cl, want) in [
        (&mut a, "127.0.0.1"),
        (&mut b, "192.168.1.20"),
        (&mut c, "203.0.113.7"),
    ] {
        cl.send(&mut st, SERVER_LIST, &[]);
        let d = cl.take(SERVER_LIST_REPLY);
        assert!(has_wide(&d, want), "server list for {want}");
    }
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    assert_eq!(enter(&mut st, &mut c, no, "").0, 1);
    for cl in [&mut b, &mut c] {
        cl.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
        assert_eq!(u32_at(&cl.take(ROOM_USER_FLAG_REPLY), 6), 1);
    }
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(u32_at(&a.take(ROOM_USER_FLAG_REPLY), 6), 1);
    countdown(&mut st, key);
    for (cl, want) in [
        (&mut a, "127.0.0.1"),
        (&mut b, "192.168.1.20"),
        (&mut c, "203.0.113.7"),
    ] {
        let d = cl.take(MATCH_ADDRESS_PUSH);
        let ip: Vec<u16> = (0..17)
            .map(|i| u16::from_le_bytes([d[6 + 2 * i], d[7 + 2 * i]]))
            .take_while(|&u| u != 0)
            .collect();
        assert_eq!(String::from_utf16_lossy(&ip), want);
    }
}

/// One auth login attempt from `peer`: (ok, reason) of the 0x201 reply.
fn login_from(st: &mut Core, sid: Sid, peer: &str, login: &str, pw: &str) -> (u32, u32) {
    let mut c = Client {
        sid,
        rx: connect(st, sid, Listener::Auth),
        got: Vec::new(),
    };
    set_addr(st, sid, peer, "192.168.1.20:10000");
    let mut body = wstr(&format!("LOCAL:{login}"), 4096);
    body.extend(wstr(pw, 17));
    body.extend(wstr("00-11", 20));
    body.extend(0i32.to_le_bytes());
    c.send(st, LOGIN_REQUEST, &body);
    let r = c.take(LOGIN_REPLY);
    st.sessions.remove(&sid);
    (u32_at(&r, 6), u32_at(&r, 0xa))
}

// LG-32: a public server with --check-password, --no-auto-create and --add-account: the friend's account and
// password gate remote logins; loopback logins (the host's own client and bots) are exempt.
#[test]
fn public_server_gates_remote_logins_by_account_and_password() {
    let mut st = state();
    st.opts.public = true;
    st.opts.pw_check = true;
    st.opts.auto_create = false;
    st.opts.autochar = true;
    st.scope(|st| {
        crate::auth::ensure_account(st, "Friend", "hunter2")?;
        Ok(())
    })
    .unwrap();
    let remote = "198.51.100.4:6000";
    let bad = crate::wire::REASON_BAD_PASSWORD;
    assert_eq!(
        login_from(&mut st, 60, remote, "Friend", "wrong"),
        (0, bad),
        "wrong password refused"
    );
    assert_eq!(
        login_from(&mut st, 61, remote, "Friend", "hunter2").0,
        1,
        "right password logs in"
    );
    assert_eq!(
        login_from(&mut st, 62, remote, "stranger", "x"),
        (0, bad),
        "unknown account refused"
    );
    assert!(
        crate::db::account_by_login(&st.db, "stranger")
            .unwrap()
            .is_none(),
        "no account created"
    );
    // the host's own client on loopback: created on first login, no password needed
    assert_eq!(login_from(&mut st, 63, "127.0.0.1:5000", "quinn", "").0, 1);
    // an account without a password (made on loopback) cannot be claimed remotely
    assert_eq!(login_from(&mut st, 64, remote, "quinn", "guess"), (0, bad));
    // without --public the old rules hold: an unknown login is still refused when auto-create is off
    st.opts.public = false;
    assert_eq!(
        login_from(&mut st, 65, "127.0.0.1:5000", "other", "pw"),
        (0, bad)
    );
}

/// LG-187: the VPS unit's flags (public, `--check-password`, `--no-auto-create`, no `--autochar`): a bot on the VPS
/// itself (loopback) needs no account and no password (LG-32) and reaches the nickname step; a bot from elsewhere
/// logs in only with its account's password (`dc-botclient --login-password`, BOT-83).
#[test]
fn vps_flags_admit_loopback_bots_and_remote_bots_with_their_password() {
    let mut st = state();
    st.opts.public = true;
    st.opts.pw_check = true;
    st.opts.auto_create = false;
    st.opts.autochar = false;
    st.scope(|st| {
        crate::auth::ensure_account(st, "bot1", "bot pass 1")?;
        Ok(())
    })
    .unwrap();
    let remote = "198.51.100.9:6000";
    let bad = crate::wire::REASON_BAD_PASSWORD;
    // the bot's default password text is refused remotely, its account password is taken
    assert_eq!(login_from(&mut st, 160, remote, "bot1", "pw"), (0, bad));
    assert_eq!(
        login_from(&mut st, 161, remote, "bot1", "bot pass 1"),
        (0, crate::wire::REASON_NO_CHARACTER),
        "past the gate, at the nickname step"
    );
    assert_eq!(login_from(&mut st, 162, remote, "bot2", "pw"), (0, bad));
    // on the VPS itself: an unknown bot name passes the gate without a password
    assert_eq!(
        login_from(&mut st, 163, "127.0.0.1:41000", "bot2", "pw"),
        (0, crate::wire::REASON_NO_CHARACTER)
    );
    assert!(crate::db::account_by_login(&st.db, "bot2")
        .unwrap()
        .is_some());
}

/// LG-146: ten failed logins for one name (from any address) or from one address (for any name) within ten
/// minutes refuse that name / address for five minutes, even with the right password; a good login clears the name.
#[test]
fn ten_failed_logins_lock_the_name_and_the_address() {
    let mut st = state();
    st.opts.public = true;
    st.opts.pw_check = true;
    st.opts.auto_create = false;
    st.opts.autochar = true;
    st.scope(|st| {
        crate::auth::ensure_account(st, "Erika", "hunter22")?;
        crate::auth::ensure_account(st, "Kim", "kimpass1")?;
        Ok(())
    })
    .unwrap();
    let bad = crate::wire::REASON_BAD_PASSWORD;
    // nine failures from distinct addresses, then the right password still works and clears the name
    for i in 0..9 {
        let peer = format!("198.51.100.{}:6000", 10 + i);
        assert_eq!(
            login_from(&mut st, 70 + i, &peer, "Erika", "guess"),
            (0, bad)
        );
    }
    assert_eq!(
        login_from(&mut st, 79, "198.51.100.20:6000", "Erika", "hunter22").0,
        1
    );
    for i in 0..10 {
        let peer = format!("198.51.100.{}:6000", 30 + i);
        assert_eq!(
            login_from(&mut st, 100 + i, &peer, "erika", "guess"),
            (0, bad)
        );
    }
    // the name is locked: the right password from a fresh address is refused too
    assert_eq!(
        login_from(&mut st, 144, "198.51.100.40:6000", "Erika", "hunter22"),
        (0, bad),
        "locked name"
    );
    // other names from those addresses still log in (each address has one failure only)
    assert_eq!(
        login_from(&mut st, 145, "198.51.100.30:6000", "Kim", "kimpass1").0,
        1
    );
    // one address guessing ten names (unknown ones included) is locked for every name
    let guesser = "203.0.113.7:5000";
    for (i, n) in [
        "Kim", "Kim", "nobody", "Ghost", "Kim", "a1", "a2", "a3", "a4", "a5",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            login_from(&mut st, 120 + i as u64, guesser, n, "guess"),
            (0, bad)
        );
    }
    assert_eq!(
        login_from(&mut st, 140, guesser, "Kim", "kimpass1"),
        (0, bad),
        "locked address"
    );
    // Kim's name has three failures only: from another address Kim logs in
    assert_eq!(
        login_from(&mut st, 141, "198.51.100.50:6000", "Kim", "kimpass1").0,
        1
    );
    // loopback logins on a public server are exempt from the account gate and from the lockout
    assert_eq!(login_from(&mut st, 142, "127.0.0.1:5000", "Erika", "").0, 1);
    // after the lockout (the guard forgets it; its timing is tested in limits::tests) the password works again
    st.limits = Default::default();
    assert_eq!(
        login_from(&mut st, 143, "198.51.100.40:6000", "Erika", "hunter22").0,
        1
    );
}

/// LG-65: a mode change puts the goal on the new mode's `DefaultGoal` (echoed as 0x3035 type 8), so an
/// Elimination room switched to One Man Army no longer carries its 100 kills into the FFA match; a goal the host
/// picks afterwards is the one the game server gets.
#[test]
fn a_mode_change_resets_the_goal_to_the_new_modes_default() {
    // synthetic room table: Elimination-like row 0 (default goal 100), One Man Army-like row 1 (default goal 30)
    const TABLE: &str = r"[Map0]
Enable=TRUE
GMS_TDM=a
GMS_FFA=b
[Mode0]
Key=TDM
GameType=1
DefaultGoal=100
[Mode1]
Key=FFA
GameType=6
DefaultGoal=30
";
    let mut st = state();
    std::sync::Arc::get_mut(&mut st.rom).unwrap().game_room =
        dc_bute::GameRoomData::from_doc(dc_bute::ButeDoc::parse(TABLE));
    let (mut a, _) = player(&mut st, 1, 2, "lg65", "GoalHost");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    assert_eq!(st.rooms[&key].goal, 100);
    a.got.clear();
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 4, 1]));
    let ups = |a: &mut Client| -> Vec<(u32, u32)> {
        a.all(ROOM_UPDATE_PUSH)
            .iter()
            .map(|d| (u32_at(d, 0xa), u32_at(d, 0xe)))
            .collect()
    };
    assert!(ups(&mut a).contains(&(8, 30)), "the goal echo");
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    assert_eq!(st.rooms[&key].goal, 30, "the FFA row's default goal");
    a.got.clear();
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[1, 8, 15]));
    assert_eq!(st.rooms[&key].goal, 15);
    assert_eq!(ups(&mut a), vec![(8, 15)]);
    assert!(st
        .maps()
        .room_line(0, 1, st.rooms[&key].goal, 0, 0, &Default::default())
        .contains("goal=15"));
}

/// LG-100: a mode change to a mode the room's map cannot play (Team Deathmatch map -> Fireteam) moves the room to the
/// mode's default map and echoes it (0x3035 type 3); a mode the map can play keeps the map.
#[test]
fn a_mode_change_moves_the_map_to_one_with_a_gms_for_the_mode() {
    // synthetic room table: map 0 plays TDM and FFA, map 1 only COOP
    const TABLE: &str = r"[Map0]
Enable=TRUE
GMS_TDM=a
GMS_FFA=b
[Map1]
Enable=TRUE
GMS_COOP=c
[Mode0]
Key=TDM
GameType=1
DefaultGoal=100
[Mode1]
Key=FFA
GameType=6
DefaultGoal=30
[Mode2]
Key=COOP
GameType=5
DefaultGoal=1
";
    let mut st = state();
    std::sync::Arc::get_mut(&mut st.rom).unwrap().game_room =
        dc_bute::GameRoomData::from_doc(dc_bute::ButeDoc::parse(TABLE));
    let (mut a, _) = player(&mut st, 1, 2, "lg100", "MapHost");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    assert_eq!(st.rooms[&key].map, 0);
    let ups = |a: &mut Client| -> Vec<(u32, u32)> {
        a.all(ROOM_UPDATE_PUSH)
            .iter()
            .map(|d| (u32_at(d, 0xa), u32_at(d, 0xe)))
            .collect()
    };
    // TDM -> FFA: map 0 has GMS_FFA, kept
    a.got.clear();
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 4, 1]));
    assert!(ups(&mut a).contains(&(3, 0)), "the map echo");
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    assert_eq!(st.rooms[&key].map, 0);
    // FFA -> COOP: map 0 has no GMS_COOP, the room moves to map 1
    a.got.clear();
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 4, 2]));
    let u = ups(&mut a);
    assert!(u.contains(&(4, 2)) && u.contains(&(3, 1)), "{u:?}");
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    assert_eq!(st.rooms[&key].map, 1, "the first map with GMS_COOP");
    assert_eq!(st.rooms[&key].goal, 1);
}

/// LG-66: in a "Sniper Only" room (weapon limit 6) a member without an equipped sniper rifle cannot mark ready
/// (0x393 {0}, reason LG-342); with one equipped he can. The host's Start passes the same check.
#[test]
fn a_sniper_only_room_refuses_ready_without_an_equipped_sniper_rifle() {
    let mut st = state();
    {
        let rom = std::sync::Arc::get_mut(&mut st.rom).unwrap();
        // synthetic weapon rows: 10 assault rifle (type 0), 20 pistol (4), 30 sniper rifle (2)
        rom.weapon_kinds.extend([(10, 0), (20, 4), (30, 2)]);
    }
    let (mut a, na) = player(&mut st, 1, 2, "lg66a", "ArmsHost");
    let (mut b, nb) = player(&mut st, 3, 4, "lg66b", "ArmsGuest");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    let give = |st: &Core, user: u32, item: i64, slot: i64| {
        crate::db::add_item(
            &st.db,
            &crate::db::NewItem {
                user_no: user,
                item_no: item,
                start_time: 0,
                end_time: 0,
                slot,
                default_type: 0,
            },
        )
        .unwrap()
    };
    let rifle = give(&st, nb, 10, 0);
    give(&st, nb, 20, 1);
    give(&st, na, 30, 0);
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 0xe, 6]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    let r = b.take(ROOM_USER_FLAG_REPLY);
    assert_eq!(u32_at(&r, 6), 0, "no sniper rifle");
    // LG-342: the refusal names the weapon limit (our reason, kind 0), not a start refusal
    assert_eq!(
        (
            u16::from_le_bytes([r[0xa], r[0xb]]),
            i16::from_le_bytes([r[0xc], r[0xd]])
        ),
        (
            dc_wire::flagrefusal::REASON_WEAPON_LIMIT,
            dc_wire::flagrefusal::KIND_WEAPON_LIMIT
        )
    );
    let key = RoomKey { channel: 1, no };
    assert_eq!(st.rooms[&key].member(nb).unwrap().state, 0);
    crate::db::set_slot(&st.db, rifle, -1).unwrap();
    give(&st, nb, 30, 0);
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(
        u32_at(&b.take(ROOM_USER_FLAG_REPLY), 6),
        1,
        "sniper rifle equipped"
    );
    assert_eq!(st.rooms[&key].member(nb).unwrap().state, 1);
    // DSV-180: the limit reaches the game server's room line
    assert!(st
        .maps()
        .room_line(0, 0, 100, 0, 6, &Default::default())
        .contains("arms=6"));
}

/// LG-67: in a "Sniper Only" room a member whose slot-0 primary is a rifle but who owns a sniper rifle elsewhere
/// (a backpack slot, or unequipped) may mark ready, and the game server's loadout names the sniper rifle in slot 0
/// for the match; the stored slots are unchanged. A member without any sniper rifle is still refused.
#[test]
fn a_sniper_only_room_puts_the_first_owned_sniper_rifle_in_the_primary_slot() {
    let mut st = state();
    {
        let rom = std::sync::Arc::get_mut(&mut st.rom).unwrap();
        // synthetic weapon rows: 10 assault rifle (type 0), 20 pistol (4), 30 / 31 sniper rifles (2), 40 knife (7)
        rom.weapon_kinds
            .extend([(10, 0), (20, 4), (30, 2), (31, 2), (40, 7)]);
    }
    let (mut a, na) = player(&mut st, 1, 2, "lg67a", "ArmsHost");
    let (mut b, nb) = player(&mut st, 3, 4, "lg67b", "ArmsGuest");
    let (mut c, nc) = player(&mut st, 5, 6, "lg67c", "ArmsPoor");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    assert_eq!(enter(&mut st, &mut c, no, "").0, 1);
    let give = |st: &Core, user: u32, item: i64, slot: i64, end_time: i64| {
        crate::db::add_item(
            &st.db,
            &crate::db::NewItem {
                user_no: user,
                item_no: item,
                start_time: 0,
                end_time,
                slot,
                default_type: 0,
            },
        )
        .unwrap()
    };
    // the starter kit leaves the slots: only the items below are equipped
    for user in [nb, nc] {
        for i in crate::db::items(&st.db, user).unwrap() {
            crate::db::set_slot(&st.db, i.uid, -1).unwrap();
        }
    }
    give(&st, na, 30, 0, 0);
    give(&st, nb, 10, 0, 0);
    give(&st, nb, 20, 1, 0);
    give(&st, nb, 40, 2, 0);
    give(&st, nb, 30, -1, 1); // expired long ago: skipped
    give(&st, nb, 31, 11, 0); // the first live sniper rifle in inventory order
    give(&st, nb, 30, -1, 0);
    give(&st, nc, 10, 0, 0);
    give(&st, nc, 20, 1, 0);
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 0xe, 6]));
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 1);
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(
        u32_at(&b.take(ROOM_USER_FLAG_REPLY), 6),
        1,
        "a sniper rifle is owned"
    );
    c.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(
        u32_at(&c.take(ROOM_USER_FLAG_REPLY), 6),
        0,
        "no sniper rifle at all"
    );
    let ch = st.character(nb).unwrap_or_default();
    let tok = game::loadout_tokens(&st, nb, &ch);
    // LG-419: the pistol, which the game server does not grant under Sniper Only (DSV-476), is not sent either
    assert!(tok.starts_with(" loadout=0:31,2:40 "), "{tok}");
    let stored: Vec<(i64, i64)> = crate::db::items(&st.db, nb)
        .unwrap()
        .iter()
        .map(|i| (i.slot, i.item_no))
        .collect();
    assert!(
        stored.contains(&(0, 10)) && stored.contains(&(11, 31)),
        "{stored:?}"
    );
    // without the limit the stored loadout goes out as it is
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 0xe, 0]));
    let tok = game::loadout_tokens(&st, nb, &ch);
    assert!(tok.starts_with(" loadout=0:10,1:20,2:40,11:31 "), "{tok}");
}

/// LG-419: under each of the nine rows of the room's weapon list the game server's `loadout=` token names only the
/// weapons the rule lets into the match (DSV-610's grant rule; an "only" rule's primary is first replaced by an owned
/// passing one, LG-67), so a backpack weapon under No Backpack and the primaries under Pistols Only stay home. The
/// stored loadout is not changed.
#[test]
fn every_weapon_rule_sends_only_its_weapons_to_the_game_server() {
    let mut st = state();
    {
        let rom = std::sync::Arc::get_mut(&mut st.rom).unwrap();
        // synthetic weapon rows: 10 rifle (0), 20 pistol (4), 30 sniper rifle (2), 40 knife (7), 50 grenade (6),
        // 60 shotgun (0x1a), 70 launcher (8)
        rom.weapon_kinds.extend([
            (10, 0),
            (20, 4),
            (30, 2),
            (40, 7),
            (50, 6),
            (60, 0x1a),
            (70, 8),
        ]);
    }
    let (mut a, _) = player(&mut st, 1, 2, "lg419a", "RuleHost");
    let (mut b, nb) = player(&mut st, 3, 4, "lg419b", "RuleGuest");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    for i in crate::db::items(&st.db, nb).unwrap() {
        crate::db::set_slot(&st.db, i.uid, -1).unwrap();
    }
    for (item, slot) in [
        (10, 0),
        (20, 1),
        (40, 2),
        (50, 3),
        (30, 10),
        (60, 11),
        (70, 12),
    ] {
        crate::db::add_item(
            &st.db,
            &crate::db::NewItem {
                user_no: nb,
                item_no: item,
                start_time: 0,
                end_time: 0,
                slot,
                default_type: 0,
            },
        )
        .unwrap();
    }
    let ch = st.character(nb).unwrap_or_default();
    for (rule, want) in [
        (0, "0:10,1:20,2:40,3:50,10:30,11:60,12:70"),
        (1, "1:20,2:40"),
        (2, "2:40"),
        (3, "0:10,1:20,2:40,3:50,11:60,12:70"),
        (4, "0:10,1:20,2:40,10:30,11:60"),
        (5, "0:10,1:20,2:40,3:50"),
        (6, "0:30,2:40"),
        (7, "0:60,2:40"),
        (8, "0:10,1:20,2:40,3:50,10:30,12:70"),
    ] {
        a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[0, 0xe, rule]));
        a.take(ROOM_SETTING_CHANGE_REPLY);
        assert_eq!(st.rooms[&RoomKey { channel: 1, no }].weapon_limit, rule);
        let tok = game::loadout_tokens(&st, nb, &ch);
        assert!(
            tok.starts_with(&format!(" loadout={want} ")),
            "rule {rule}: {tok}"
        );
    }
    let mut stored: Vec<(i64, i64)> = crate::db::items(&st.db, nb)
        .unwrap()
        .iter()
        .filter(|i| i.slot >= 0)
        .map(|i| (i.slot, i.item_no))
        .collect();
    stored.sort();
    assert_eq!(
        stored,
        [
            (0, 10),
            (1, 20),
            (2, 40),
            (3, 50),
            (10, 30),
            (11, 60),
            (12, 70)
        ]
    );
}

fn bare_game(id: u64, key: RoomKey) -> game::Game {
    game::Game {
        id,
        room: key,
        port: 27015,
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
    }
}

#[test]
fn the_last_player_leaving_a_running_match_closes_the_room_and_sends_the_bots_back() {
    // DC-DEFAULT: LG-132 (proposed)
    let mut st = state();
    let (mut a, na) = player(&mut st, 1, 2, "solo", "Solo");
    let (mut b1, nb1) = player(&mut st, 3, 4, "bot1", "Bot1");
    let (mut b2, _) = player(&mut st, 5, 6, "Bot2", "Bot2");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b1, no, "").0, 1);
    assert_eq!(enter(&mut st, &mut b2, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    b1.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    b2.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    countdown(&mut st, key);
    st.games.insert(9, bare_game(9, key));
    st.rooms.get_mut(&key).unwrap().game = Some(9);
    for c in [&mut a, &mut b1, &mut b2] {
        c.pump();
        c.got.clear();
    }
    // a bot leaving keeps the room
    b2.send(&mut st, ROOM_LEAVE, &[]);
    assert!(st.rooms.contains_key(&key));
    // the player leaves the room in mid match: the match ends, the bot is sent back, the room is gone
    a.send(&mut st, ROOM_LEAVE, &[]);
    assert!(!st.rooms.contains_key(&key), "room closed");
    assert!(st.games[&9].over, "the game server was told to quit");
    let removes: Vec<(u32, u32)> = b1
        .all(ROOM_USER_REMOVE_PUSH)
        .iter()
        .map(|d| (u32_at(d, 6), u32_at(d, 0xa)))
        .collect();
    assert!(
        removes.contains(&(nb1, crate::wire::REMOVE_KICKED)),
        "the bot is told it left the room: {removes:?}"
    );
    assert!(st.sessions[&b1.sid].room.is_none());
    let removed: Vec<u32> = a
        .all(LOBBY_ROOM_REMOVE_PUSH)
        .iter()
        .map(|d| u32_at(d, 6))
        .collect();
    assert!(removed.contains(&no), "{removed:?}");
    let _ = na;
}

#[test]
fn the_last_player_disconnecting_from_a_waiting_room_closes_it_but_a_second_player_keeps_it() {
    // DC-DEFAULT: LG-132 (proposed)
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "pa", "Pa");
    let (mut p, _) = player(&mut st, 3, 4, "pb", "Pb");
    let (mut b, _) = player(&mut st, 5, 6, "bot7", "Bot7");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut p, no, "").0, 1);
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    st.scope(|st| crate::lobby::disconnect(st, a.sid)).unwrap();
    assert_eq!(st.rooms[&key].members.len(), 2, "one player is still there");
    st.scope(|st| crate::lobby::disconnect(st, p.sid)).unwrap();
    assert!(!st.rooms.contains_key(&key));
    assert!(st.sessions[&b.sid].room.is_none());
}

#[test]
fn a_room_of_bots_only_is_left_alone() {
    // LG-132 acts on a player's departure only: a bot-only room (soak runs) keeps its bots
    let mut st = state();
    let (mut b1, _) = player(&mut st, 1, 2, "bot1", "Bot1");
    let (mut b2, _) = player(&mut st, 3, 4, "bot2", "Bot2");
    let (mut b3, _) = player(&mut st, 5, 6, "bot3", "Bot3");
    let no = open_room(&mut st, &mut b1, "");
    assert_eq!(enter(&mut st, &mut b2, no, "").0, 1);
    assert_eq!(enter(&mut st, &mut b3, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    b3.send(&mut st, ROOM_LEAVE, &[]);
    assert_eq!(st.rooms[&key].members.len(), 2);
}

// ---------------------------------------------------------------------------------------------------------------
// GM accounts on the lobby (LG-170 .. LG-175)
// ---------------------------------------------------------------------------------------------------------------

fn set_gm(st: &Core, account: u32, gm: i64) {
    st.db
        .execute(
            "UPDATE accounts SET gm = ?2 WHERE user_no = ?1",
            rusqlite::params![account as i64, gm],
        )
        .unwrap();
}

/// One auth login of `login`; returns the 0x201 payload and the open auth connection.
fn auth_login(st: &mut Core, sid: Sid, login: &str) -> (Client, Vec<u8>) {
    let mut a = Client {
        sid,
        rx: connect(st, sid, Listener::Auth),
        got: Vec::new(),
    };
    let mut body = wstr(&format!("LOCAL:{login}"), 4096);
    body.extend(wstr("pw", 17));
    body.extend(wstr("00-11", 20));
    body.extend(0i32.to_le_bytes());
    a.send(st, LOGIN_REQUEST, &body);
    let r = a.take(LOGIN_REPLY);
    (a, r)
}

fn name_free(st: &mut Core, a: &mut Client, nick: &str) -> u32 {
    a.send(st, NAME_CHECK, &wstr(nick, 17));
    u32_at(&a.take(NAME_CHECK_REPLY), 6)
}

fn create(st: &mut Core, a: &mut Client, nick: &str) -> (u32, u32) {
    let mut b = 6002i32.to_le_bytes().to_vec();
    b.extend(wstr(nick, 17));
    a.send(st, NEW_CHAR, &b);
    let r = a.take(NEW_CHAR_REPLY);
    (u32_at(&r, 6), u32_at(&r, 0xa))
}

#[test]
fn gm_account_gets_privilege_2_and_the_gm_name_prefix_is_reserved() {
    let mut st = state();
    // a player account: no GM name, privilege 0 (LG-170 / LG-172)
    let (mut a, r) = auth_login(&mut st, 1, "erika");
    assert_eq!(u32_at(&r, 6), 0, "no character yet");
    assert_eq!(name_free(&mut st, &mut a, "GMErika"), 0);
    assert_eq!(
        create(&mut st, &mut a, "GMErika"),
        (0, 0xe),
        "bad-name code"
    );
    assert_eq!(
        name_free(&mut st, &mut a, "gmErika"),
        1,
        "case as the 2009 client"
    );
    assert_eq!(create(&mut st, &mut a, "Erika"), (1, 0));
    st.sessions.remove(&1);
    let (_, r) = auth_login(&mut st, 2, "erika");
    assert_eq!((u32_at(&r, 6), u32_at(&r, 0x26)), (1, 0));
    st.sessions.remove(&2);
    // a GM account: may take a GM name and logs in with privilege 2
    let acc = crate::db::create_account(&st.db, "quinn", 0).unwrap();
    set_gm(&st, acc, 1);
    let (mut g, r) = auth_login(&mut st, 3, "quinn");
    assert_eq!(u32_at(&r, 6), 0);
    assert_eq!(name_free(&mut st, &mut g, "GMquinn"), 1);
    assert_eq!(create(&mut st, &mut g, "GMquinn"), (1, 0));
    st.sessions.remove(&3);
    let (mut g, r) = auth_login(&mut st, 4, "quinn");
    assert_eq!(
        (u32_at(&r, 6), u32_at(&r, 0x12), u32_at(&r, 0x26)),
        (1, acc, 2)
    );
    // the server list carries ten operator slots (LG-175)
    let list = g.take(SERVER_LIST_PUSH);
    assert_eq!(u32_at(&list, 0x16 + 0x98), 10);
    // the flag off again: privilege 0 at the next login
    st.sessions.remove(&4);
    set_gm(&st, acc, 0);
    let (_, r) = auth_login(&mut st, 5, "quinn");
    assert_eq!(u32_at(&r, 0x26), 0);
}

#[test]
fn autochar_never_hands_a_gm_name_to_a_player() {
    let mut st = state();
    st.opts.autochar = true;
    let (_, r) = auth_login(&mut st, 1, "GMsneaky");
    assert_eq!(u32_at(&r, 6), 1);
    let acc = u32_at(&r, 0x12);
    let nick = st.character(acc).unwrap().nick;
    assert!(!nick.starts_with("GM"), "{nick}");
}

// LG-203: without a room badge item the host neither sets the room badge level nor kicks
#[test]
fn a_host_without_a_room_badge_cannot_kick_or_set_the_badge_level() {
    let mut st = state();
    let (mut a, na) = player(&mut st, 1, 2, "alice", "Alice");
    let (mut b, nb) = player(&mut st, 3, 4, "bob", "Bob");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    a.got.clear();
    a.send(
        &mut st,
        ROOM_SETTING_CHANGE,
        &u32s(&[0, field::HOST_BADGE, 1]),
    );
    assert_eq!(u32_at(&a.take(ROOM_SETTING_CHANGE_REPLY), 6), 0);
    a.send(&mut st, ROOM_KICK, &u32s(&[nb, nb]));
    assert_eq!(u32_at(&a.take(ROOM_KICK_REPLY), 6), 0);
    // the operator can give the rule up (room.kick_needs_badge = false): the host kicks as in CD-27
    st.rules.kick_needs_badge = false;
    a.send(&mut st, ROOM_KICK, &u32s(&[nb, nb]));
    assert_eq!(u32_at(&a.take(ROOM_KICK_REPLY), 6), 1);
    let _ = na;
}

#[test]
fn gm_kick_immunity_gm_kicks_and_operator_slots() {
    let mut st = state();
    st.rules.kick_needs_badge = false; // LG-203 has its own test
    let (mut a, _na) = player(&mut st, 1, 2, "alice", "Alice");
    let (mut b, nb) = player(&mut st, 3, 4, "bob", "Bob");
    let (mut m, nm) = player(&mut st, 5, 6, "mod", "Mod");
    // a GM-named character on a player account (a name from before LG-172): the 2009 client shows it as a GM
    let (mut c, nc) = player(&mut st, 7, 8, "carol", "Carol");
    st.db
        .execute(
            "UPDATE characters SET nick = 'GMCarol' WHERE user_no = ?1",
            [nc as i64],
        )
        .unwrap();
    set_gm(&st, nm, 1);
    // channel list: one GM in channel 1 (LG-175)
    m.send(&mut st, CHANNEL_LIST, &[]);
    let l = m.take(CHANNEL_LIST_REPLY);
    assert_eq!((u32_at(&l, 0x12 + 0x52), u32_at(&l, 0x12 + 0x56)), (10, 1));
    let no = open_room(&mut st, &mut a, "");
    for p in [&mut b, &mut m, &mut c] {
        assert_eq!(enter(&mut st, p, no, "").0, 1);
    }
    for p in [&mut a, &mut b, &mut m, &mut c] {
        p.got.clear();
    }
    // the host cannot kick a GM account or a GM-named character: 0x38f {0, 0x82} -> res 0x1845 (LG-173)
    for t in [nm, nc] {
        a.send(&mut st, ROOM_KICK, &u32s(&[t, t]));
        let r = a.take(ROOM_KICK_REPLY);
        assert_eq!((u32_at(&r, 6), u32_at(&r, 0xa)), (0, 0x82));
    }
    assert!(!m.has(ROOM_USER_REMOVE_PUSH));
    // a guest who is no GM still cannot kick
    b.send(&mut st, ROOM_KICK, &u32s(&[nc, nc]));
    assert_eq!(u32_at(&b.take(ROOM_KICK_REPLY), 6), 0);
    // a GM guest kicks: the target is told with reason 2 (Kicked by GM.) and may not come back
    m.send(&mut st, ROOM_KICK, &u32s(&[nb, nb]));
    assert_eq!(u32_at(&m.take(ROOM_KICK_REPLY), 6), 1);
    let k = b.take(ROOM_USER_REMOVE_PUSH);
    assert_eq!((u32_at(&k, 6), u32_at(&k, 0xa)), (nb, 2));
    let k = a.take(ROOM_USER_REMOVE_PUSH);
    assert_eq!((u32_at(&k, 6), u32_at(&k, 0xa)), (nb, 2));
    assert_eq!(enter(&mut st, &mut b, no, "").0, 0);
}

#[test]
fn a_gm_enters_a_full_channel_on_an_operator_slot() {
    let mut st = state();
    st.opts.channel_max = 1;
    let (_a, _) = player(&mut st, 1, 2, "alice", "Alice");
    // a second player finds channel 1 full; the same account as a GM gets in
    let (mut au, r) = auth_login(&mut st, 3, "mod");
    assert_eq!(create(&mut st, &mut au, "Mod"), (1, 0));
    let nm = u32_at(&r, 0x12);
    st.sessions.remove(&3);
    let lobby = |st: &mut Core, sid: Sid, acc: u32| {
        let rx = connect(st, sid, Listener::Lobby);
        let s = st.sessions.get_mut(&sid).unwrap();
        s.account = Some(acc);
        s.in_lobby = true;
        Client {
            sid,
            rx,
            got: Vec::new(),
        }
    };
    let mut l = lobby(&mut st, 4, nm);
    let mut b = u32s(&[nm, 1]);
    b.extend(wstr("Channel 1", 32));
    l.send(&mut st, CHANNEL_ENTER, &b);
    assert_eq!(u32_at(&l.take(CHANNEL_ENTER_REPLY), 6), 0, "full");
    set_gm(&st, nm, 1);
    l.send(&mut st, CHANNEL_ENTER, &b);
    assert_eq!(u32_at(&l.take(CHANNEL_ENTER_REPLY), 6), 1, "operator slot");
}

// LG-179: the client's start watchdog (0x394 phase 1) takes a start that never got its address back.
#[test]
fn a_start_failed_notice_resets_a_starting_room() {
    let mut st = state();
    let (mut a, na) = player(&mut st, 1, 2, "sw1a", "WatchA");
    let (mut b, nb) = player(&mut st, 3, 4, "sw1b", "WatchB");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(st.rooms[&key].state, RoomPhase::Starting);
    a.pump();
    b.pump();
    a.got.clear();
    b.got.clear();
    // Bob's watchdog ran out before any address: the whole start is taken back
    b.send(&mut st, MATCH_EXIT, &u32s(&[1]));
    assert_eq!(st.rooms[&key].state, RoomPhase::Waiting);
    assert!(st.rooms[&key].members.iter().all(|m| m.state == 0));
    for c in [&mut a, &mut b] {
        let ups: Vec<(u32, u32)> = c
            .all(ROOM_UPDATE_PUSH)
            .iter()
            .map(|d| (u32_at(d, 0xa), u32_at(d, 0xe)))
            .collect();
        assert!(ups.contains(&(7, 0)), "{ups:?}");
        let states: Vec<u32> = c
            .all(ROOM_USER_STATE_PUSH)
            .iter()
            .map(|d| u32_at(d, 6))
            .collect();
        assert!(states.contains(&na) && states.contains(&nb), "{states:?}");
    }
}

// LG-179: a game server that cannot be started leaves the room waiting, with one log line naming the cause.
#[test]
fn a_game_server_that_cannot_start_leaves_the_room_waiting_and_logs_why() {
    let dir = std::env::temp_dir().join(format!("dc-lg179-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let logfile = dir.join("lobby.log");
    let mut st = state();
    st.log = std::sync::Arc::new(crate::logs::Logger::open(Some(&logfile), None).0);
    st.opts.dserver = true;
    st.opts.dserver_world = "world".into();
    st.opts.dserver_exe = dir.join("missing-dserver.exe");
    let (mut a, _) = player(&mut st, 1, 2, "sw2a", "SpawnA");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    a.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    let r = a.take(ROOM_USER_FLAG_REPLY);
    assert_eq!(u32_at(&r, 6), 0, "start refused");
    assert_eq!(st.rooms[&key].state, RoomPhase::Waiting);
    assert!(st.rooms[&key].members.iter().all(|m| m.state == 0));
    assert!(st.rooms[&key].game.is_none());
    let log = std::fs::read_to_string(&logfile).unwrap_or_default();
    assert!(
        log.contains("game server start failed: cannot start")
            && log.contains("missing-dserver.exe"),
        "{log}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------------------------------------------
// The shopping state of a waiting-room member (0x392 kind 1, 0x3042 sub-state; CLI-1560, LG-260)
// ---------------------------------------------------------------------------------------------------------------

/// The (id, state, sub-state) of every member record of a 0x38b.
fn member_rows(d: &[u8]) -> Vec<(u32, u32, u32)> {
    let n = u32_at(d, 0xe) as usize;
    (0..n)
        .map(|i| {
            let o = 0x12 + i * 0xb8;
            (u32_at(d, o + 4), u32_at(d, o + 0x3a), u32_at(d, o + 0x3e))
        })
        .collect()
}

#[test]
fn a_member_who_goes_shopping_is_shown_shopping_to_the_room_until_he_is_back() {
    // SPEC: PROTOCOL_2009.md §3.7 (0x392 (1, 1) shopping -> 0x3042 {id, state, 1}; the 0x38b record +0x3a / +0x3e)
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "sa", "Sa");
    let (mut b, nb) = player(&mut st, 3, 4, "sb", "Sb");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(u32_at(&b.take(ROOM_USER_FLAG_REPLY), 6), 1);
    a.pump();
    a.got.clear();
    // the 2009 client leaving for the shop: not ready, then shopping
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 0]));
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[1, 1]));
    let pushes: Vec<(u32, u32, u32)> = a
        .all(ROOM_USER_STATE_PUSH)
        .iter()
        .map(|d| (u32_at(d, 6), u32_at(d, 0xa), u32_at(d, 0xe)))
        .collect();
    assert_eq!(pushes, vec![(nb, 0, 0), (nb, 0, 1)]);
    // the ready change is answered, the shopping flag is not
    assert_eq!(b.all(ROOM_USER_FLAG_REPLY).len(), 1);
    // the private state of the shop screen is only remembered
    b.send(&mut st, PRIVATE_STATE, &u32s(&[1]));
    assert_eq!(u32_at(&b.take(PRIVATE_STATE_REPLY), 6), 1);
    assert_eq!(st.sessions[&b.sid].private_state, 1);
    // a member list fetched now carries the sub-state in Sb's record
    a.pump();
    a.got.clear();
    a.send(&mut st, ROOM_USERS, &[]);
    let rows = member_rows(&a.take(ROOM_USERS_REPLY));
    assert!(rows.contains(&(nb, 0, 1)), "{rows:?}");
    // back in the room: 0x30c, then present (1, 0)
    a.pump();
    a.got.clear();
    b.send(&mut st, PRIVATE_STATE_END, &[]);
    assert_eq!(st.sessions[&b.sid].private_state, 0);
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[1, 0]));
    let d = a.take(ROOM_USER_STATE_PUSH);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0xa), u32_at(&d, 0xe)),
        (nb, 0, 0)
    );
    assert_eq!(st.rooms[&key].member(nb).unwrap().sub, 0);
}

#[test]
fn a_ready_member_who_goes_shopping_is_no_longer_ready() {
    // DC-DEFAULT: LG-260 (proposed)
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "ra", "Ra");
    let (mut b, nb) = player(&mut st, 3, 4, "rb", "Rb");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    a.pump();
    a.got.clear();
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[1, 1]));
    let d = a.take(ROOM_USER_STATE_PUSH);
    assert_eq!(
        (u32_at(&d, 6), u32_at(&d, 0xa), u32_at(&d, 0xe)),
        (nb, 0, 1)
    );
    assert_eq!(st.rooms[&key].member(nb).unwrap().state, 0);
}

#[test]
fn the_shopping_state_ends_with_the_members_room_leave_or_disconnect() {
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "la", "La");
    let (mut b, nb) = player(&mut st, 3, 4, "lb", "Lb");
    let (mut c, nc) = player(&mut st, 5, 6, "lc", "Lc");
    let no = open_room(&mut st, &mut a, "");
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    assert_eq!(enter(&mut st, &mut c, no, "").0, 1);
    let key = RoomKey { channel: 1, no };
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[1, 1]));
    c.send(&mut st, ROOM_USER_FLAG, &u32s(&[1, 1]));
    // leaving the room removes the record; entering again starts without the sub-state
    b.send(&mut st, ROOM_LEAVE, &[]);
    assert_eq!(u32_at(&b.take(ROOM_LEAVE_REPLY), 6), 1);
    assert!(st.rooms[&key].member(nb).is_none());
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    assert_eq!(st.rooms[&key].member(nb).unwrap().sub, 0);
    a.pump();
    a.got.clear();
    a.send(&mut st, ROOM_USERS, &[]);
    let rows = member_rows(&a.take(ROOM_USERS_REPLY));
    assert!(rows.contains(&(nb, 0, 0)), "{rows:?}");
    assert!(rows.contains(&(nc, 0, 1)), "{rows:?}");
    // a disconnect removes the shopper from the room (0x3034 to the others)
    a.pump();
    a.got.clear();
    st.scope(|st| crate::lobby::disconnect(st, c.sid)).unwrap();
    a.pump();
    assert!(st.rooms[&key].member(nc).is_none());
    let k = a.take(ROOM_USER_REMOVE_PUSH);
    assert_eq!(u32_at(&k, 6), nc);
}

/// LG-272: a `[[lobby.servers]]` entry naming another lobby goes out as our push 0x3f02 before 0x2000; the 2009
/// entries of 0x2000 stay exactly as without it (this lobby's entries and the clan entry), and a lobby without such
/// an entry sends no 0x3f02 at all.
#[test]
fn another_lobby_entry_rides_in_our_push_before_the_unchanged_server_list() {
    fn login(st: &mut Core, sid: Sid) -> Vec<(u16, Vec<u8>)> {
        let mut a = Client {
            sid,
            rx: connect(st, sid, Listener::Auth),
            got: Vec::new(),
        };
        let mut body = wstr("LOCAL:listed", 4096);
        body.extend(wstr("pw", 17));
        body.extend(wstr("00-11", 20));
        body.extend(0i32.to_le_bytes());
        a.send(st, LOGIN_REQUEST, &body);
        if u32_at(&a.take(LOGIN_REPLY), 6) == 0 {
            let mut b = 6002i32.to_le_bytes().to_vec();
            b.extend(wstr("Listed", 17));
            a.send(st, NEW_CHAR, &b);
            a.take(NEW_CHAR_REPLY);
        }
        a.pump();
        st.sessions.remove(&sid);
        a.got
    }
    let mut st = state();
    let plain = login(&mut st, 70);
    assert!(plain
        .iter()
        .all(|(id, _)| *id != dc_wire::moddedlist::MODDED_SERVERS_PUSH));
    let list_before = plain
        .iter()
        .find(|(id, _)| *id == SERVER_LIST_PUSH)
        .unwrap()
        .1
        .clone();

    let mut st = state();
    st.opts.other_lobbies = vec![dc_config::LobbyServerCfg {
        name: "US Central Modded".into(),
        capacity: 0,
        host: String::new(),
        channels: 3,
        features: Vec::new(),
        mods: Vec::new(),
        client_visual_allowed: false,
        client_fov: None,
        lobby: "203.0.113.7:11000".into(),
        fingerprint: "ab".repeat(32),
        policy: String::new(),
        region: "US Central".into(),
    }];
    let got = login(&mut st, 71);
    let ids: Vec<u16> = got.iter().map(|(id, _)| *id).collect();
    let push = ids
        .iter()
        .position(|i| *i == dc_wire::moddedlist::MODDED_SERVERS_PUSH);
    let list = ids.iter().position(|i| *i == SERVER_LIST_PUSH);
    assert!(
        push.is_some() && push < list,
        "0x3f02 comes before 0x2000: {ids:x?}"
    );
    let listed = dc_wire::moddedlist::parse(&got[push.unwrap()].1);
    assert_eq!(
        listed,
        vec![dc_wire::moddedlist::ListedLobby {
            name: "US Central Modded".into(),
            region: "US Central".into(),
            lobby: "203.0.113.7:11000".into(),
            fingerprint: "ab".repeat(32),
            policy: "modded".into(),
        }]
    );
    // the 2009 list is byte for byte the one without the entry
    assert_eq!(got[list.unwrap()].1, list_before);
    assert_eq!(
        u32_at(&list_before, 0x12),
        2,
        "this lobby's entry and the clan entry only"
    );
}

/// A mod client over an empty folder whose set offers the given Hacks bits (LG-274).
fn hacks_mods(tag: &str, offered: u32) -> crate::modhost::ModClient {
    let dir = std::env::temp_dir().join(format!("ca_lobby_hacks_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = crate::modhost::ModClient::start(
        &dir,
        std::sync::Arc::new(crate::logs::Logger::none()),
        None,
    )
    .unwrap();
    if offered & dc_wire::roomhacks::WALLHACKS != 0 {
        c.culling_off_by = Some("xray".into());
    }
    if offered & dc_wire::roomhacks::NO_RECOIL != 0 {
        c.no_recoil_by = Some("xray".into());
    }
    c
}

fn hacks_push(c: &mut Client) -> (u32, u32) {
    let d = c.take(dc_wire::roomhacks::ROOM_HACKS_PUSH);
    dc_wire::roomhacks::parse_push(&d).unwrap()
}

/// LG-274 / DSV-479: on a set that offers both, a room starts on wallhacks (culling off), the host's request moves
/// it to wallhacks + no-recoil (`-visibility off -combat cone_check=off`) and to no-recoil only (`-visibility on`),
/// every member sees the value after its room info and on each change, and a guest's request or a value outside
/// the choices is refused.
#[test]
fn room_hacks_follow_the_set_and_reach_the_game_server_flags() {
    use dc_wire::roomhacks::{NO_RECOIL, ROOM_HACKS_PUSH, ROOM_HACKS_REQUEST, WALLHACKS};
    let mut st = state();
    st.mods = Some(hacks_mods("both", WALLHACKS | NO_RECOIL));
    let (mut a, _) = player(&mut st, 1, 2, "h1", "Host");
    let (mut b, _) = player(&mut st, 3, 4, "h2", "Guest");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    a.send(&mut st, ROOM_INFO, &[]);
    a.take(ROOM_INFO_REPLY);
    assert_eq!(hacks_push(&mut a), (3, WALLHACKS));
    assert_eq!(game::room_hacks_args(&st, key), ["-visibility", "off"]);
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    b.send(&mut st, ROOM_INFO, &[]);
    b.take(ROOM_INFO_REPLY);
    assert_eq!(hacks_push(&mut b), (3, WALLHACKS));
    // the host picks wallhacks + no-recoil: both members get it
    a.send(&mut st, ROOM_HACKS_REQUEST, &u32s(&[WALLHACKS | NO_RECOIL]));
    assert_eq!(hacks_push(&mut a), (3, 3));
    assert_eq!(hacks_push(&mut b), (3, 3));
    assert_eq!(
        game::room_hacks_args(&st, key),
        ["-visibility", "off", "-combat", "cone_check=off"]
    );
    // no-recoil only: culling stays on
    a.send(&mut st, ROOM_HACKS_REQUEST, &u32s(&[NO_RECOIL]));
    assert_eq!(hacks_push(&mut b), (3, NO_RECOIL));
    assert_eq!(
        game::room_hacks_args(&st, key),
        ["-visibility", "on", "-combat", "cone_check=off"]
    );
    // a guest's request and a value outside the choices: refused, the sender gets the room's value back
    b.send(&mut st, ROOM_HACKS_REQUEST, &u32s(&[WALLHACKS]));
    assert_eq!(hacks_push(&mut b), (3, NO_RECOIL));
    a.send(&mut st, ROOM_HACKS_REQUEST, &u32s(&[0]));
    assert_eq!(hacks_push(&mut a), (3, NO_RECOIL));
    a.send(&mut st, ROOM_HACKS_REQUEST, &u32s(&[4]));
    assert_eq!(hacks_push(&mut a), (3, NO_RECOIL));
    assert!(!b.has(ROOM_HACKS_PUSH));
    assert_eq!(st.rooms[&key].hacks, NO_RECOIL);
}

/// LG-274: a set with wallhacks only offers that one choice (no-recoil is refused); a set with no-recoil only starts
/// its rooms on no-recoil with culling on.
#[test]
fn room_hacks_refuse_a_choice_the_set_does_not_offer() {
    use dc_wire::roomhacks::{NO_RECOIL, ROOM_HACKS_REQUEST, WALLHACKS};
    let mut st = state();
    st.mods = Some(hacks_mods("wall", WALLHACKS));
    let (mut a, _) = player(&mut st, 1, 2, "h3", "Wall");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    a.send(&mut st, ROOM_HACKS_REQUEST, &u32s(&[WALLHACKS | NO_RECOIL]));
    assert_eq!(hacks_push(&mut a), (WALLHACKS, WALLHACKS));
    assert_eq!(game::room_hacks_args(&st, key), ["-visibility", "off"]);

    let mut st = state();
    st.mods = Some(hacks_mods("nr", NO_RECOIL));
    let (mut a, _) = player(&mut st, 1, 2, "h4", "Steady");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    assert_eq!(st.rooms[&key].hacks, NO_RECOIL);
    a.send(&mut st, ROOM_HACKS_REQUEST, &u32s(&[WALLHACKS]));
    assert_eq!(hacks_push(&mut a), (NO_RECOIL, NO_RECOIL));
    assert_eq!(
        game::room_hacks_args(&st, key),
        ["-visibility", "on", "-combat", "cone_check=off"]
    );
}

/// LG-274: a vanilla lobby sends no Hacks push, ignores the request and passes no flag.
#[test]
fn a_vanilla_room_has_no_hacks() {
    use dc_wire::roomhacks::{ROOM_HACKS_PUSH, ROOM_HACKS_REQUEST};
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "h5", "Plain");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    a.send(&mut st, ROOM_INFO, &[]);
    a.take(ROOM_INFO_REPLY);
    a.send(&mut st, ROOM_HACKS_REQUEST, &u32s(&[1]));
    assert!(!a.has(ROOM_HACKS_PUSH));
    assert_eq!(st.rooms[&key].hacks, 0);
    assert!(game::room_hacks_args(&st, key).is_empty());
}

/// A synthetic ROM for Gun Game: every mode row free-for-all, a shop with rifles 4, 12, a pistol 1, a knife 58, a
/// launcher 29, a specialist 171, and rifle 13 that no shown product sells.
fn gun_rom(st: &mut Core) {
    let mut rom = crate::rom::RomData::empty();
    let modes: String = (0..9)
        .map(|i| {
            [
                format!("[Mode{i}]"),
                "GameType = 6".into(),
                "DefaultGoal = 2".into(),
            ]
            .join("\n")
                + "\n"
        })
        .collect();
    rom.game_room = dc_bute::GameRoomData::from_doc(dc_bute::ButeDoc::parse(&modes));
    let mut shop = String::new();
    for (i, (id, kind)) in [
        (4, 0),
        (12, 0),
        (1, 4),
        (58, 7),
        (29, 8),
        (171, 0x1d),
        (13, 0),
    ]
    .iter()
    .enumerate()
    {
        let lines = [
            format!("[ItemInfo{i}]"),
            format!("Itemindex = {i}"),
            format!("Itemid = {id}"),
            format!("Itemtype = {kind}"),
            "GP_DisplayNum = 5".to_string(),
            format!("[Product{i}]"),
            format!("ProductID = {i}"),
            format!("Shopview = {}", (*id != 13) as u32),
            "Itemnum = 1".to_string(),
            format!("Itemindex0 = {i}"),
            "ProductName = \"p\"".to_string(),
        ];
        shop += &(lines.join("\n") + "\n");
        rom.weapon_kinds.insert(*id as i64, *kind);
    }
    rom.shop.catalog = Some(dc_bute::ShopCatalog::from_doc(&dc_bute::ButeDoc::parse(
        &shop,
    )));
    st.rom = std::sync::Arc::new(rom);
}

fn two_entries(st: &mut Core, second_features: &[&str]) {
    let e = |name: &str, features: &[&str]| dc_config::LobbyServerCfg {
        name: name.into(),
        capacity: 100,
        host: String::new(),
        channels: 3,
        features: features.iter().map(|f| f.to_string()).collect(),
        mods: Vec::new(),
        client_visual_allowed: false,
        client_fov: None,
        lobby: String::new(),
        fingerprint: String::new(),
        policy: String::new(),
        region: String::new(),
    };
    st.opts.servers = vec![e("US West", &[]), e("Vanilla++ (US West)", second_features)];
}

/// `player` on the second entry's first channel (4).
fn on_second_entry(st: &mut Core, a: Sid, l: Sid, login: &str) -> (Client, u32) {
    let (mut c, account) = player(st, a, l, login, login);
    st.sessions.get_mut(&c.sid).unwrap().server = 2;
    let mut b = u32s(&[account, 4]);
    b.extend(wstr("Channel 4", 32));
    c.send(st, CHANNEL_ENTER, &b);
    assert_eq!(u32_at(&c.take(CHANNEL_ENTER_REPLY), 6), 1);
    (c, account)
}

/// LG-305: everything a featured entry adds: the offer, the request, the channel's and the members' pushes, the drawn
/// ladder and the game server's flag. The host alone changes it; a mode change ends it.
#[test]
fn gun_game_on_a_featured_entry() {
    use dc_wire::gungame::{parse_room, OFFER, REQUEST, ROOM};
    let mut st = state();
    gun_rom(&mut st);
    two_entries(&mut st, &["gun_game"]);
    let (mut a, _) = on_second_entry(&mut st, 1, 2, "g1");
    assert!(a.has(OFFER));
    let (mut b, _) = on_second_entry(&mut st, 3, 4, "g2");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 4, no };
    a.send(&mut st, REQUEST, &u32s(&[1]));
    assert!(st.rooms[&key].gun_game);
    assert_eq!(
        parse_room(&b.take(ROOM)),
        Some((no, true, vec![])),
        "the channel hears"
    );
    assert_eq!(parse_room(&a.take(ROOM)), Some((no, true, vec![])));
    // a later arrival: the room list, then our push
    b.send(&mut st, ROOM_LIST, &[]);
    b.take(ROOM_LIST_REPLY);
    assert_eq!(parse_room(&b.take(ROOM)), Some((no, true, vec![])));
    // a guest's request: refused, he gets the value back
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    b.send(&mut st, REQUEST, &u32s(&[0]));
    assert!(st.rooms[&key].gun_game);
    assert_eq!(parse_room(&b.take(ROOM)), Some((no, true, vec![])));
    // the start: goal 2 from the three guns on sale: no knife, launcher, specialist, unsold rifle
    st.rooms.get_mut(&key).unwrap().goal = 2;
    st.scope(|st| {
        crate::gungame::prepare_start(st, key).unwrap();
        Ok(())
    })
    .unwrap();
    let ladder = st.rooms[&key].ladder.clone();
    assert_eq!(ladder.len(), 2);
    assert!(ladder.iter().all(|w| [1, 4, 12].contains(w)), "{ladder:?}");
    assert_ne!(ladder[0], ladder[1]);
    let flag = crate::gungame::args(&st, key);
    assert_eq!(flag[0], "-gungame");
    assert!(
        flag[1].ends_with(&format!("{},{}", ladder[0], ladder[1])),
        "{flag:?}"
    );
    assert_eq!(parse_room(&b.take(ROOM)), Some((no, true, ladder.clone())));
    // a goal above the list: the goal becomes the list and the room is told
    st.rooms.get_mut(&key).unwrap().goal = 9;
    st.scope(|st| {
        crate::gungame::prepare_start(st, key).unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(st.rooms[&key].goal, 3);
    assert_eq!(st.rooms[&key].ladder.len(), 3);
    assert!(b.has(CHAT_PUSH));
    // the host's mode change ends it
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[8, 4, 3, 0]));
    assert!(!st.rooms[&key].gun_game);
    assert!(crate::gungame::args(&st, key).is_empty());
}

/// LG-304 / LG-305: an entry without the feature refuses the request and its wire is byte for byte what it was: the
/// same script on a lobby with a featured sibling entry and on one without any feature.
#[test]
fn gun_game_is_refused_elsewhere_and_other_entries_stay_byte_identical() {
    use dc_wire::gungame::{OFFER, REQUEST, ROOM};
    let script = |features: &[&str]| -> Vec<(u16, Vec<u8>)> {
        let mut st = state();
        gun_rom(&mut st);
        two_entries(&mut st, features);
        let (mut a, _) = player(&mut st, 1, 2, "same", "Same");
        let no = open_room(&mut st, &mut a, "");
        a.send(&mut st, REQUEST, &u32s(&[1]));
        let key = RoomKey { channel: 1, no };
        assert!(!st.rooms[&key].gun_game, "refused");
        assert!(crate::gungame::args(&st, key).is_empty());
        a.send(&mut st, ROOM_INFO, &[]);
        a.send(&mut st, ROOM_LIST, &[]);
        assert!(!a.has(OFFER) && !a.has(ROOM));
        st.scope(|st| {
            crate::gungame::prepare_start(st, key).unwrap();
            Ok(())
        })
        .unwrap();
        a.pump();
        assert!(!a.has(ROOM));
        a.got.clone()
    };
    assert_eq!(script(&["gun_game"]), script(&[]));
}

/// LG-306: Hardcore on a featured entry: the host's request reaches every member and the game server's flag; a guest is
/// refused; on an entry without the feature nothing is sent and nothing changes.
#[test]
fn hardcore_on_a_featured_entry_only() {
    use dc_wire::roomhardcore::{parse, PUSH, REQUEST};
    let mut st = state();
    two_entries(&mut st, &["hardcore"]);
    let (mut a, _) = on_second_entry(&mut st, 1, 2, "h1");
    let (mut b, _) = on_second_entry(&mut st, 3, 4, "h2");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 4, no };
    a.send(&mut st, ROOM_INFO, &[]);
    assert_eq!(parse(&a.take(PUSH)), Some(false));
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    b.send(&mut st, REQUEST, &u32s(&[1]));
    assert_eq!(parse(&b.take(PUSH)), Some(false), "a guest is refused");
    assert!(game::hardcore_args(&st, key).is_empty());
    a.send(&mut st, REQUEST, &u32s(&[1]));
    assert_eq!(parse(&b.take(PUSH)), Some(true));
    assert_eq!(parse(&a.take(PUSH)), Some(true));
    assert_eq!(
        game::hardcore_args(&st, key),
        ["-combat", "health_scale=0.2"]
    );
    // it stands beside a weapon limit: sniper only stays
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[8, 0xe, 6, 0]));
    assert_eq!(st.rooms[&key].weapon_limit, 6);
    assert_eq!(game::hardcore_args(&st, key).len(), 2);

    // an entry without it: the request is refused and the wire is byte for byte the same as a lobby without features
    let script = |features: &[&str]| -> Vec<(u16, Vec<u8>)> {
        let mut st = state();
        two_entries(&mut st, features);
        let (mut a, _) = player(&mut st, 1, 2, "same", "Same");
        let no = open_room(&mut st, &mut a, "");
        a.send(&mut st, REQUEST, &u32s(&[1]));
        let key = RoomKey { channel: 1, no };
        assert!(!st.rooms[&key].hardcore);
        assert!(game::hardcore_args(&st, key).is_empty());
        a.send(&mut st, ROOM_INFO, &[]);
        assert!(!a.has(PUSH));
        a.got.clone()
    };
    assert_eq!(script(&["hardcore"]), script(&[]));
}

/// LG-307: the Possession Arms: a featured entry's host only, refused in Fireteam, Spy Hunt, Quarantine and with Gun Game;
/// a ROM pick or a refused mode ends it; a vanilla entry's wire is byte for byte what it was.
#[test]
fn possession_on_a_featured_entry_only() {
    use dc_wire::possession::{parse, PUSH, REQUEST};
    let mut st = state();
    gun_rom(&mut st);
    // rows: 0 TDM, 1 Fireteam, 2 FFA, 3 Spy Hunt, 4 Quarantine
    let modes: String = [1, 5, 6, 9, 11]
        .iter()
        .enumerate()
        .map(|(i, c)| {
            [
                format!("[Mode{i}]"),
                format!("GameType = {c}"),
                "DefaultGoal = 2".into(),
            ]
            .join("\n")
                + "\n"
        })
        .collect();
    let mut rom = crate::rom::RomData::empty();
    rom.game_room = dc_bute::GameRoomData::from_doc(dc_bute::ButeDoc::parse(&modes));
    st.rom = rom.into();
    two_entries(&mut st, &["possession", "gun_game"]);
    let (mut a, _) = on_second_entry(&mut st, 1, 2, "p1");
    let (mut b, _) = on_second_entry(&mut st, 3, 4, "p2");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 4, no };
    a.send(&mut st, ROOM_INFO, &[]);
    assert_eq!(parse(&a.take(PUSH)), Some(false));
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    let set_mode = |st: &mut Core, a: &mut Client, m: u32| {
        a.send(st, ROOM_SETTING_CHANGE, &u32s(&[8, 4, m, 0]));
        assert_eq!(st.rooms[&key].mode, m);
    };
    set_mode(&mut st, &mut a, 0);
    b.send(&mut st, REQUEST, &u32s(&[1]));
    assert_eq!(parse(&b.take(PUSH)), Some(false), "a guest is refused");
    // refused modes
    for m in [1, 3, 4] {
        set_mode(&mut st, &mut a, m);
        a.send(&mut st, REQUEST, &u32s(&[1]));
        assert!(!st.rooms[&key].possession, "mode row {m}");
        assert_eq!(parse(&a.take(PUSH)), Some(false));
        assert!(game::possession_args(&st, key).is_empty());
    }
    set_mode(&mut st, &mut a, 0);
    // it replaces a ROM limit
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[8, 0xe, 6, 0]));
    assert_eq!(st.rooms[&key].weapon_limit, 6);
    a.send(&mut st, REQUEST, &u32s(&[1]));
    assert!(st.rooms[&key].possession);
    assert_eq!(st.rooms[&key].weapon_limit, 0);
    assert_eq!(parse(&b.take(PUSH)), Some(true));
    assert_eq!(game::possession_args(&st, key), ["-possession"]);
    // CRATE-2 (amended 2026-11-07): the operator's crate share reaches the game server only when set
    assert!(game::hisec_args(&st).is_empty());
    assert!(st
        .rules
        .apply("hisec.camo_share", &dc_mod_lua::JsonValue::Number(40.0)));
    assert_eq!(game::hisec_args(&st), ["-hisec", "camo_share=40"]);
    // a ROM pick replaces it
    a.send(&mut st, ROOM_SETTING_CHANGE, &u32s(&[8, 0xe, 6, 0]));
    assert!(!st.rooms[&key].possession);
    assert_eq!(st.rooms[&key].weapon_limit, 6);
    assert_eq!(parse(&b.take(PUSH)), Some(false));
    // a mode without it ends it
    a.send(&mut st, REQUEST, &u32s(&[1]));
    assert!(st.rooms[&key].possession);
    set_mode(&mut st, &mut a, 1);
    assert!(!st.rooms[&key].possession);
    // with Gun Game: neither way round
    set_mode(&mut st, &mut a, 2);
    a.send(&mut st, REQUEST, &u32s(&[1]));
    assert!(st.rooms[&key].possession);
    a.send(&mut st, dc_wire::gungame::REQUEST, &u32s(&[1]));
    assert!(!st.rooms[&key].gun_game, "refused in a Possession room");
    a.send(&mut st, REQUEST, &u32s(&[0]));
    a.send(&mut st, dc_wire::gungame::REQUEST, &u32s(&[1]));
    assert!(st.rooms[&key].gun_game);
    a.send(&mut st, REQUEST, &u32s(&[1]));
    assert!(!st.rooms[&key].possession, "refused in a Gun Game room");

    // an entry without it: refused, and the same bytes as a lobby without the feature
    let script = |features: &[&str]| -> Vec<(u16, Vec<u8>)> {
        let mut st = state();
        two_entries(&mut st, features);
        let (mut a, _) = player(&mut st, 1, 2, "same", "Same");
        let no = open_room(&mut st, &mut a, "");
        a.send(&mut st, REQUEST, &u32s(&[1]));
        let key = RoomKey { channel: 1, no };
        assert!(!st.rooms[&key].possession);
        assert!(game::possession_args(&st, key).is_empty());
        a.send(&mut st, ROOM_INFO, &[]);
        assert!(!a.has(PUSH));
        a.got.clone()
    };
    assert_eq!(script(&["possession"]), script(&[]));
}

// ---------------------------------------------------------------- LG-318 / LG-319: bot seats, the kept seat

const POOL_FIELD: &str = "LOCAL:;b=1";

fn seat_login_body() -> Vec<u8> {
    let mut body = wstr(POOL_FIELD, 4096);
    body.extend(wstr("", 17));
    body.extend(wstr("00-11", 20));
    body.extend(0i32.to_le_bytes());
    body
}

fn pool_state(size: u32) -> Core {
    let mut st = state();
    st.rules.bot_pool_size = size;
    st.scope(crate::botpool::ensure).unwrap();
    st
}

fn set_peer(st: &mut Core, sid: Sid, peer: &str) {
    st.sessions.get_mut(&sid).unwrap().peer = peer.into();
}

/// The auth half of a seat: the client and the login reply's (ok, reason, account).
fn seat_auth(st: &mut Core, sid: Sid, peer: &str) -> (Client, u32, u32, u32) {
    let mut a = Client {
        sid,
        rx: connect(st, sid, Listener::Auth),
        got: Vec::new(),
    };
    set_peer(st, sid, peer);
    a.send(st, LOGIN_REQUEST, &seat_login_body());
    let r = a.take(LOGIN_REPLY);
    (a, u32_at(&r, 6), u32_at(&r, 0xa), u32_at(&r, 0x12))
}

/// A whole seat: auth (closed after the pick, as a bot's may), lobby, channel.
fn seat(st: &mut Core, sid: Sid) -> (Client, u32) {
    let (mut a, ok, _, account) = seat_auth(st, sid, "127.0.0.1:1");
    assert_eq!(ok, 1, "a seat is free");
    a.take(SERVER_LIST_PUSH);
    a.send(st, SERVER_PICK, &u32s(&[1]));
    let d = a.take(SERVER_PICK_REPLY);
    let mut b = d[0xe..0x1e].to_vec();
    st.scope(|st| crate::lobby::disconnect(st, sid)).unwrap();
    let lsid = sid + 1;
    let mut l = Client {
        sid: lsid,
        rx: connect(st, lsid, Listener::Lobby),
        got: Vec::new(),
    };
    b.extend(account.to_le_bytes());
    l.send(st, LOBBY_LOGIN, &b);
    assert_eq!(u32_at(&l.take(LOBBY_LOGIN_REPLY), 6), 1);
    let mut b = u32s(&[account, 1]);
    b.extend(wstr("Channel 1", 32));
    l.send(st, CHANNEL_ENTER, &b);
    l.take(CHANNEL_ENTER_REPLY);
    (l, account)
}

fn seat_room(st: &mut Core, c: &mut Client, max: i32) -> u32 {
    let mut b = max.to_le_bytes().to_vec();
    b.extend(wstr("r", 33));
    b.extend(wstr("", 9));
    b.extend(1i32.to_le_bytes());
    c.send(st, ROOM_CREATE, &b);
    let r = c.take(ROOM_CREATE_REPLY);
    assert_eq!(u32_at(&r, 6), 1);
    u32_at(&r, 0xe)
}

/// Enters a room; true when the lobby took the member.
fn seat_enter(st: &mut Core, c: &mut Client, no: u32) -> bool {
    let mut b = (no as i32).to_le_bytes().to_vec();
    b.extend(wstr("", 9));
    b.extend(wstr("", 65));
    c.send(st, ROOM_ENTER, &b);
    u32_at(&c.take(ROOM_ENTER_REPLY), 6) == 1
}

/// `n` people online (not in a room).
fn people(st: &mut Core, n: u32, first_sid: Sid) -> Vec<(Client, u32)> {
    (0..n)
        .map(|i| {
            let login = format!("person{first_sid}x{i}");
            player(
                st,
                first_sid + 10 * i as u64,
                first_sid + 10 * i as u64 + 1,
                &login,
                &login,
            )
        })
        .collect()
}

fn room_at(st: &Core, no: u32) -> &crate::room::Room {
    &st.rooms[&RoomKey { channel: 1, no }]
}

fn teams(st: &Core, no: u32) -> (usize, usize) {
    let r = room_at(st, no);
    (r.count_team(1), r.count_team(2))
}

#[test]
fn the_pool_is_made_once_and_passes_over_a_name_somebody_holds() {
    let mut st = state();
    crate::db::create_account_from(&st.db, "poolbot2", 0, st.rules.first_account_no).unwrap();
    st.rules.bot_pool_size = 3;
    st.scope(crate::botpool::ensure).unwrap();
    let count = |st: &Core| -> i64 {
        st.db
            .query_row(
                "SELECT COUNT(*) FROM accounts WHERE pool_bot = 1",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(count(&st), 3);
    st.scope(crate::botpool::ensure).unwrap();
    assert_eq!(count(&st), 3, "idempotent");
    let other = crate::db::account_by_login(&st.db, "poolbot2")
        .unwrap()
        .unwrap();
    assert!(!st.pool.has(other.user_no));
    let four = crate::db::account_by_login(&st.db, "poolbot4")
        .unwrap()
        .unwrap();
    assert!(st.pool.has(four.user_no));
    assert!(!crate::lifecycle::is_human(&st, four.user_no));
    assert_eq!(st.character(four.user_no).unwrap().nick, "PoolBot4");
}

#[test]
fn a_seat_is_refused_without_a_pool_when_full_and_over_the_address_cap() {
    use dc_wire::loginmods::*;
    let mut st = pool_state(0);
    let (_a, ok, reason, _) = seat_auth(&mut st, 1, "203.0.113.1:1");
    assert_eq!((ok, reason), (0, REASON_NO_POOL));

    let mut st = pool_state(3);
    st.rules.bot_pool_per_ip = 2;
    let (_a, ok, _, a1) = seat_auth(&mut st, 1, "203.0.113.1:1");
    let (_b, ok2, _, a2) = seat_auth(&mut st, 2, "203.0.113.1:2");
    assert_eq!((ok, ok2), (1, 1));
    assert_ne!(a1, a2, "two seats, two accounts");
    let (_c, ok, reason, _) = seat_auth(&mut st, 3, "203.0.113.1:3");
    assert_eq!((ok, reason), (0, REASON_POOL_ADDRESS));
    let (_d, ok, _, _) = seat_auth(&mut st, 4, "203.0.113.2:3");
    assert_eq!(ok, 1, "another address");
    let (_e, ok, reason, _) = seat_auth(&mut st, 5, "203.0.113.3:3");
    assert_eq!((ok, reason), (0, REASON_POOL_FULL));

    // this machine has no cap
    let mut st = pool_state(3);
    st.rules.bot_pool_per_ip = 1;
    for sid in 1..=3 {
        assert_eq!(seat_auth(&mut st, sid, "127.0.0.1:1").1, 1);
    }
}

#[test]
fn a_seat_comes_back_with_its_connection_and_waits_for_its_key() {
    let mut st = pool_state(1);
    // auth dropped before a pick: free at once
    let (_a, ok, _, _) = seat_auth(&mut st, 1, "127.0.0.1:1");
    assert_eq!(ok, 1);
    assert_eq!(seat_auth(&mut st, 2, "127.0.0.1:1").1, 0, "taken");
    st.scope(|st| crate::lobby::disconnect(st, 1)).unwrap();
    st.scope(|st| crate::lobby::disconnect(st, 2)).unwrap();
    // auth dropped after a pick: held for the lobby connection
    let (mut a, ok, _, _) = seat_auth(&mut st, 3, "127.0.0.1:1");
    assert_eq!(ok, 1);
    a.send(&mut st, SERVER_PICK, &u32s(&[1]));
    st.scope(|st| crate::lobby::disconnect(st, 3)).unwrap();
    assert_eq!(
        seat_auth(&mut st, 4, "127.0.0.1:1").1,
        0,
        "the key holds it"
    );
    st.scope(|st| crate::lobby::disconnect(st, 4)).unwrap();
    // an unused key runs out: the bot crashed between the connections
    let mut st = pool_state(1);
    let (mut a, ok, _, _) = seat_auth(&mut st, 5, "127.0.0.1:1");
    assert_eq!(ok, 1);
    st.rules.session_ttl_secs = 0;
    a.send(&mut st, SERVER_PICK, &u32s(&[1]));
    st.scope(|st| crate::lobby::disconnect(st, 5)).unwrap();
    assert_eq!(seat_auth(&mut st, 6, "127.0.0.1:1").1, 1, "the key ran out");
    st.scope(|st| crate::lobby::disconnect(st, 6)).unwrap();
    st.rules.session_ttl_secs = 60;
    // a full login, then the lobby connection crashes: free at once, room left
    let (mut l, account) = seat(&mut st, 10);
    assert_eq!(seat_auth(&mut st, 20, "127.0.0.1:1").1, 0);
    st.scope(|st| crate::lobby::disconnect(st, 20)).unwrap();
    assert!(
        st.tickets.values().all(|t| t.account != account),
        "no key left"
    );
    let _ = &mut l;
    st.scope(|st| crate::lobby::disconnect(st, l.sid)).unwrap();
    assert_eq!(
        seat_auth(&mut st, 21, "127.0.0.1:1").3,
        account,
        "the same seat again"
    );
}

#[test]
fn a_seat_has_no_password_login_and_no_way_to_shop_chat_or_make_a_room() {
    let mut st = pool_state(1);
    let acc = crate::db::account_by_login(&st.db, "poolbot1")
        .unwrap()
        .unwrap();
    // by name, also from this machine, with or without the mark
    for (i, field) in ["LOCAL:poolbot1", "LOCAL:poolbot1;b=1"].iter().enumerate() {
        let sid = 40 + i as u64;
        let mut a = Client {
            sid,
            rx: connect(&mut st, sid, Listener::Auth),
            got: Vec::new(),
        };
        set_peer(&mut st, sid, "127.0.0.1:1");
        let mut body = wstr(field, 4096);
        body.extend(wstr("anything", 17));
        body.extend(wstr("00-11", 20));
        body.extend(0i32.to_le_bytes());
        a.send(&mut st, LOGIN_REQUEST, &body);
        let r = a.take(LOGIN_REPLY);
        assert_eq!(
            (u32_at(&r, 6), u32_at(&r, 0xa)),
            (0, crate::wire::REASON_BAD_PASSWORD)
        );
    }
    assert!(st.sessions.values().all(|s| s.account != Some(acc.user_no)));
    // the prefix is reserved for the nickname screen
    assert!(crate::gm::name_reserved_for(&st, None, "poolBot77"));
    // a seat's other messages go nowhere
    let (mut l, _) = seat(&mut st, 50);
    let mut b = 3i32.to_le_bytes().to_vec();
    b.extend(wstr("r", 33));
    b.extend(wstr("", 9));
    b.extend(1i32.to_le_bytes());
    l.send(&mut st, ROOM_CREATE, &b);
    assert!(!l.has(ROOM_CREATE_REPLY));
    l.send(&mut st, CHAT_SEND, &[0u8; 64]);
    assert!(st.rooms.is_empty());
    assert!(crate::botpool::message_refused(&st, l.sid, CHAT_SEND));
    assert!(crate::botpool::message_refused(&st, l.sid, TEAM_CHANGE));
    assert!(!crate::botpool::message_refused(&st, l.sid, ROOM_ENTER));
    // the clan server is refused at the pick
    let mut st = pool_state(1);
    let (mut a, ok, _, _) = seat_auth(&mut st, 60, "127.0.0.1:1");
    assert_eq!(ok, 1);
    let clan = crate::servers::clan_id(&st);
    a.send(&mut st, SERVER_PICK, &u32s(&[clan]));
    assert_eq!(u32_at(&a.take(SERVER_PICK_REPLY), 6), 0);
}

/// LG-320: a seat in a match keeps no record and earns nothing, wherever it connects from; the person with it is paid.
#[test]
fn a_seat_earns_nothing_and_keeps_no_record() {
    let (mut st, _a, _b, na, nb, id) = leaver_match(true);
    st.db
        .execute(
            "UPDATE accounts SET pool_bot = 1, self_bot = 1 WHERE user_no = ?1",
            [nb as i64],
        )
        .unwrap();
    st.rules.bot_pool_size = 0;
    st.scope(crate::botpool::ensure).unwrap();
    assert!(st.pool.has(nb));
    let (ea, eb) = (
        crate::db::character(&st.db, na).unwrap().unwrap(),
        crate::db::character(&st.db, nb).unwrap().unwrap(),
    );
    result_line_for(&mut st, id);
    let (a2, b2) = (
        crate::db::character(&st.db, na).unwrap().unwrap(),
        crate::db::character(&st.db, nb).unwrap().unwrap(),
    );
    assert!(a2.exp > ea.exp && a2.gp > ea.gp, "the person is paid");
    assert_eq!(
        (b2.exp, b2.gp, b2.kills, b2.deaths),
        (eb.exp, eb.gp, eb.kills, eb.deaths)
    );
}

/// Under the threshold bots fill a room, and a person who wants in still gets a bot's seat.
#[test]
fn under_the_threshold_bots_fill_and_a_person_takes_a_bots_seat() {
    let mut st = pool_state(5);
    st.rules.bot_keep_seat_online = 8;
    let mut host = people(&mut st, 1, 100).pop().unwrap();
    let no = seat_room(&mut st, &mut host.0, 3);
    let (mut b1, a1) = seat(&mut st, 200);
    let (mut b2, a2) = seat(&mut st, 210);
    assert!(seat_enter(&mut st, &mut b1, no));
    assert!(
        seat_enter(&mut st, &mut b2, no),
        "the last seat, under the threshold"
    );
    assert_eq!(room_at(&st, no).members.len(), 3);
    let mut guest = people(&mut st, 1, 300).pop().unwrap();
    assert!(seat_enter(&mut st, &mut guest.0, no), "a person gets in");
    let r = room_at(&st, no);
    assert_eq!(
        r.members.len(),
        3,
        "full again: nobody more leaves under the threshold"
    );
    assert!(r.member(guest.1).is_some());
    // the bot that left heard of its removal, the others saw a leave
    let gone = if r.member(a1).is_none() {
        (&mut b1, a1)
    } else {
        (&mut b2, a2)
    };
    let d = gone.0.all(ROOM_USER_REMOVE_PUSH);
    assert_eq!(
        (u32_at(&d[0], 6), u32_at(&d[0], 0xa)),
        (gone.1, crate::wire::REMOVE_KICKED)
    );
    let seen = host.0.all(ROOM_USER_REMOVE_PUSH);
    assert_eq!(u32_at(seen.last().unwrap(), 0xa), crate::wire::REMOVE_LEFT);
    // a full room of people refuses, and with the rule off a bot keeps its seat
    let mut st = pool_state(2);
    st.rules.bot_keep_seat_online = 0;
    let mut host = people(&mut st, 1, 100).pop().unwrap();
    let no = seat_room(&mut st, &mut host.0, 2);
    let (mut b, _) = seat(&mut st, 200);
    assert!(seat_enter(&mut st, &mut b, no));
    let mut guest = people(&mut st, 1, 300).pop().unwrap();
    assert!(!seat_enter(&mut st, &mut guest.0, no), "0 = never");
}

/// At the threshold: no bot takes the last seat, a person's join that fills the room sends one bot out at once, the room
/// keeps the seat, and the bot's way back in is refused: no rejoin loop.
#[test]
fn at_the_threshold_the_last_seat_stays_open_and_the_bot_does_not_come_back() {
    let mut st = pool_state(5);
    st.rules.bot_keep_seat_online = 3;
    let mut ps = people(&mut st, 3, 100);
    let no = seat_room(&mut st, &mut ps[0].0, 4);
    let (mut b1, a1) = seat(&mut st, 200);
    let (mut b2, _) = seat(&mut st, 210);
    assert!(seat_enter(&mut st, &mut b1, no));
    assert_eq!(teams(&st, no), (1, 1));
    assert!(seat_enter(&mut st, &mut b2, no), "not the last seat");
    let (mut b3, _) = seat(&mut st, 220);
    assert!(!seat_enter(&mut st, &mut b3, no), "the last seat is kept");
    assert_eq!(room_at(&st, no).members.len(), 3);
    // a person fills it: the bot on the larger team, the newest of it, leaves
    let (t1, t2) = teams(&st, no);
    assert_eq!((t1, t2), (2, 1));
    let (c, acc) = &mut ps[1];
    assert!(seat_enter(&mut st, c, no));
    let r = room_at(&st, no);
    assert_eq!(r.members.len(), 3, "the seat stays open");
    assert!(r.member(*acc).is_some());
    let (t1, t2) = teams(&st, no);
    assert!(t1.abs_diff(t2) <= 1, "balanced: {t1} {t2}");
    // whoever left, it is refused now, and asks no more of the room
    let (gone, _other) = if r.member(a1).is_none() {
        (&mut b1, 0)
    } else {
        (&mut b2, 0)
    };
    assert!(!seat_enter(&mut st, gone, no));
    assert_eq!(room_at(&st, no).members.len(), 3);
    // the people went below the threshold: the bot may fill again
    let sid = ps[2].0.sid;
    st.scope(|st| crate::lobby::disconnect(st, sid)).unwrap();
    assert!(seat_enter(&mut st, &mut b3, no));
    assert_eq!(room_at(&st, no).members.len(), 4);
}

/// Above the threshold a person into a full room: one bot for the seat, one more to keep the next seat open.
#[test]
fn a_person_into_a_full_room_at_the_threshold_opens_two_seats() {
    let mut st = pool_state(5);
    st.rules.bot_keep_seat_online = 8;
    let mut host = people(&mut st, 1, 100).pop().unwrap();
    let no = seat_room(&mut st, &mut host.0, 4);
    let mut bots: Vec<_> = (0..3).map(|i| seat(&mut st, 200 + 10 * i)).collect();
    for b in bots.iter_mut() {
        assert!(seat_enter(&mut st, &mut b.0, no));
    }
    assert_eq!(room_at(&st, no).members.len(), 4);
    let _more = people(&mut st, 6, 400);
    let mut guest = people(&mut st, 1, 900).pop().unwrap();
    assert!(seat_enter(&mut st, &mut guest.0, no));
    assert_eq!(room_at(&st, no).members.len(), 3, "host, guest and one bot");
    let (t1, t2) = teams(&st, no);
    assert_eq!(t1.abs_diff(t2), 1, "as even as three can be");
}

/// A bot host is handed over to a person, and the bots go first by team: the host only when no other bot is left.
#[test]
fn a_bot_host_leaves_last_and_the_room_goes_to_a_person() {
    let mut st = pool_state(0);
    st.rules.bot_keep_seat_online = 1;
    // an operator bot on its own account: it may host
    let (mut hb, hb_acc) = player(&mut st, 1, 2, "VpsBot1", "VpsBot1");
    let no = seat_room(&mut st, &mut hb, 2);
    let mut guest = people(&mut st, 1, 100).pop().unwrap();
    assert!(
        seat_enter(&mut st, &mut guest.0, no),
        "the second seat fills it"
    );
    let r = room_at(&st, no);
    assert_eq!(r.members.len(), 1, "the host bot was the only bot: it left");
    assert_eq!(r.host, guest.1, "the person holds the room");
    assert!(r.member(hb_acc).is_none());
    assert!(guest.0.has(ROOM_UPDATE_PUSH), "the hand-over push");
    // a GM is no bot to send away, whatever his name
    let mut st = pool_state(0);
    st.rules.bot_keep_seat_online = 1;
    let (mut gm, gm_acc) = player(&mut st, 1, 2, "VpsBot2", "VpsBot2");
    st.db
        .execute(
            "UPDATE accounts SET gm = 1 WHERE user_no = ?1",
            [gm_acc as i64],
        )
        .unwrap();
    let no = seat_room(&mut st, &mut gm, 1);
    let mut guest = people(&mut st, 1, 100).pop().unwrap();
    assert!(!seat_enter(&mut st, &mut guest.0, no));
    assert!(room_at(&st, no).member(gm_acc).is_some());
}

/// A running match: the bot goes with the normal pushes, the match runs on for the people.
#[test]
fn a_bot_leaves_a_running_match_for_a_person_and_the_match_goes_on() {
    let (mut st, mut a, _b, _na, nb, id) = leaver_match(false);
    st.db
        .execute(
            "UPDATE accounts SET self_bot = 1 WHERE user_no = ?1",
            [nb as i64],
        )
        .unwrap();
    let key = st.games[&id].room;
    let phase = st.rooms[&key].state;
    assert_ne!(phase, RoomPhase::Waiting);
    st.rules.bot_keep_seat_online = 1;
    st.rooms.get_mut(&key).unwrap().max_players = 2;
    st.rooms.get_mut(&key).unwrap().mid_join = 1;
    st.rules.allow_late_join = true;
    let mut guest = people(&mut st, 1, 700).pop().unwrap();
    assert!(seat_enter(&mut st, &mut guest.0, key.no));
    let r = &st.rooms[&key];
    assert!(r.member(nb).is_none(), "the bot left");
    assert!(r.member(guest.1).is_some());
    assert_eq!(r.state, phase, "the match still runs");
    assert!(a.has(ROOM_USER_REMOVE_PUSH));
}

// ---- join in progress: the host's option through the wire, entry into a playing room, refusals logged ----

/// The `join_refused` events of the monitor, oldest first (their detail text).
fn join_refusals(st: &Core) -> Vec<String> {
    let mut q = st
        .db
        .prepare("SELECT detail FROM ops_events WHERE kind = 'join_refused' ORDER BY id")
        .unwrap();
    q.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// The host's 0x38c for field `field` (mode 0); true when the lobby answers ok.
fn set_option(st: &mut Core, host: &mut Client, field: u32, value: u32) -> bool {
    host.send(st, ROOM_SETTING_CHANGE, &u32s(&[0, field, value]));
    u32_at(&host.take(ROOM_SETTING_CHANGE_REPLY), 6) == 1
}

/// The join in progress value of the room view (0x389 +0x78) as `c` reads it.
fn room_info_mid_join(st: &mut Core, c: &mut Client) -> u32 {
    c.send(st, ROOM_INFO, &[]);
    u32_at(&c.take(ROOM_INFO_REPLY), 0x78)
}

/// The host starts the room and the countdown runs out: the room plays.
fn start_and_play(st: &mut Core, host: &mut Client, key: RoomKey) {
    host.send(st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    assert_eq!(u32_at(&host.take(ROOM_USER_FLAG_REPLY), 6), 1);
    countdown(st, key);
    assert_eq!(st.rooms[&key].state, RoomPhase::Playing);
}

/// The host turns join in progress on through the real 0x38c, the room view says so, the room plays and a member
/// joins from the room list: entry, Ready, the match address.
#[test]
fn host_enables_join_in_progress_and_a_member_joins_late() {
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "jp1", "JpHost");
    let (mut b, nb) = player(&mut st, 3, 4, "jp2", "JpLate");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    // off and on again, the way the host's two check boxes send it (0x38c field 9, value 0 / 1)
    assert_eq!(
        room_info_mid_join(&mut st, &mut a),
        1,
        "a new room allows it"
    );
    assert!(set_option(&mut st, &mut a, 9, 0));
    assert_eq!(st.rooms[&key].mid_join, 0);
    assert_eq!(room_info_mid_join(&mut st, &mut a), 0);
    assert!(set_option(&mut st, &mut a, 9, 1));
    assert_eq!(st.rooms[&key].mid_join, 1);
    assert_eq!(room_info_mid_join(&mut st, &mut a), 1);
    // friendly fire (field 10) and kill cam (13) do not touch it
    assert!(set_option(&mut st, &mut a, 10, 0));
    assert!(set_option(&mut st, &mut a, 13, 0));
    assert_eq!(st.rooms[&key].mid_join, 1);
    start_and_play(&mut st, &mut a, key);
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    assert_eq!(
        room_info_mid_join(&mut st, &mut b),
        1,
        "the member reads the same"
    );
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(u32_at(&b.take(ROOM_USER_FLAG_REPLY), 6), 1);
    assert!(b.has(MATCH_ADDRESS_PUSH), "the address follows the ok");
    assert_eq!(st.rooms[&key].member(nb).unwrap().state, 3);
    assert!(join_refusals(&st).is_empty());
}

/// Join in progress off in the room: the entry from the list is refused (generic reason) and logged.
#[test]
fn entry_into_a_playing_room_with_join_in_progress_off_is_refused_and_logged() {
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "jq1", "JqHost");
    let (mut b, nb) = player(&mut st, 3, 4, "jq2", "JqLate");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    assert!(set_option(&mut st, &mut a, 9, 0));
    start_and_play(&mut st, &mut a, key);
    assert_eq!(
        enter(&mut st, &mut b, no, ""),
        (0, 1),
        "reason 1: the generic box"
    );
    assert!(st.rooms[&key].member(nb).is_none());
    let seen = join_refusals(&st);
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].contains("playing, join in progress off (room option)"),
        "{seen:?}"
    );
    // the option on, the server rule off: another cause in words
    st.rooms.get_mut(&key).unwrap().mid_join = 1;
    st.rules.allow_late_join = false;
    assert_eq!(enter(&mut st, &mut b, no, "").0, 0);
    assert!(join_refusals(&st)[1].contains("late join disabled on this server"));
    // and back on: the same member comes in
    st.rules.allow_late_join = true;
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
}

/// A member who sits in the waiting room of a room that started without him cannot Ready into it with the option off:
/// 0x393 {fail, 0x8e, join kind} as before, and the cause is logged.
#[test]
fn ready_into_a_playing_room_with_join_in_progress_off_is_refused_and_logged() {
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "jr1", "JrHost");
    let (mut b, nb) = player(&mut st, 3, 4, "jr2", "JrLate");
    let no = open_room(&mut st, &mut a, "");
    let key = RoomKey { channel: 1, no };
    assert_eq!(enter(&mut st, &mut b, no, "").0, 1);
    assert!(set_option(&mut st, &mut a, 9, 0));
    start_and_play(&mut st, &mut a, key);
    assert_eq!(
        st.rooms[&key].member(nb).unwrap().state,
        0,
        "left in the room"
    );
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    let d = b.take(ROOM_USER_FLAG_REPLY);
    assert_eq!(u32_at(&d, 6), 0);
    assert_eq!(
        u16::from_le_bytes([d[0xa], d[0xb]]),
        crate::wire::REASON_JOIN_FAILED
    );
    assert!(!b.has(MATCH_ADDRESS_PUSH));
    let seen = join_refusals(&st);
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].contains("late join refused: join in progress off (room option)"),
        "{seen:?}"
    );
    // settings need a waiting room: the option cannot change now
    assert!(!set_option(&mut st, &mut a, 9, 1));
    st.rooms.get_mut(&key).unwrap().mid_join = 1;
    st.rules.allow_late_join = false;
    b.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 1]));
    assert_eq!(u32_at(&b.take(ROOM_USER_FLAG_REPLY), 6), 0);
    assert!(join_refusals(&st)[1].contains("late join disabled on this server"));
}

/// A playing room that is full: a person comes in when a bot makes way; a room of people (or of a GM) is closed to
/// him, and the refusal says "full" with the count.
#[test]
fn entry_into_a_full_playing_room_needs_a_bot_that_yields_and_a_refusal_is_logged() {
    // full of bots: the person is taken, the pool bot goes first
    let mut st = pool_state(5);
    st.rules.bot_keep_seat_online = 1;
    let (mut hb, hb_acc) = player(&mut st, 1, 2, "VpsBot3", "VpsBot3");
    let no = seat_room(&mut st, &mut hb, 2);
    let key = RoomKey { channel: 1, no };
    let (mut pb, pb_acc) = seat(&mut st, 200);
    assert!(seat_enter(&mut st, &mut pb, no));
    start_and_play(&mut st, &mut hb, key);
    assert_eq!(room_at(&st, no).members.len(), 2);
    let mut guest = people(&mut st, 1, 400).pop().unwrap();
    assert!(seat_enter(&mut st, &mut guest.0, no), "a bot makes way");
    assert!(
        room_at(&st, no).member(pb_acc).is_none(),
        "the pool bot went"
    );
    // the room is full again with the guest in it: the keep-a-seat rule sends the host bot out too
    assert!(room_at(&st, no).member(hb_acc).is_none());
    assert!(room_at(&st, no).member(guest.1).is_some());
    assert!(join_refusals(&st).is_empty());
    // full of people: nobody yields
    let mut st = pool_state(0);
    st.rules.bot_keep_seat_online = 1;
    let mut host = people(&mut st, 1, 100).pop().unwrap();
    let no = seat_room(&mut st, &mut host.0, 1);
    let key = RoomKey { channel: 1, no };
    start_and_play(&mut st, &mut host.0, key);
    let mut guest = people(&mut st, 1, 300).pop().unwrap();
    assert!(!seat_enter(&mut st, &mut guest.0, no));
    let seen = join_refusals(&st);
    assert_eq!(seen.len(), 1);
    assert!(seen[0].contains("full (1/1, no bot yields)"), "{seen:?}");
    // full of a GM bot: no one yields either
    let mut st = pool_state(0);
    st.rules.bot_keep_seat_online = 1;
    let (mut gm, gm_acc) = player(&mut st, 1, 2, "VpsBot4", "VpsBot4");
    st.db
        .execute(
            "UPDATE accounts SET gm = 1 WHERE user_no = ?1",
            [gm_acc as i64],
        )
        .unwrap();
    let no = seat_room(&mut st, &mut gm, 1);
    let key = RoomKey { channel: 1, no };
    start_and_play(&mut st, &mut gm, key);
    let mut guest = people(&mut st, 1, 100).pop().unwrap();
    assert!(!seat_enter(&mut st, &mut guest.0, no));
    assert!(join_refusals(&st)[0].contains("full (1/1"));
}

/// A wrong password is refused with its own reason and logged with its cause.
#[test]
fn a_wrong_password_entry_is_logged_with_its_cause() {
    let mut st = state();
    let (mut a, _) = player(&mut st, 1, 2, "jk1", "JkHost");
    let (mut b, _) = player(&mut st, 3, 4, "jk2", "JkGuest");
    let no = open_room(&mut st, &mut a, "pw1");
    assert_eq!(enter(&mut st, &mut b, no, "bad").0, 0);
    let seen = join_refusals(&st);
    assert!(seen[0].contains("wrong password"), "{seen:?}");
}
