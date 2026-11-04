//! Tests of the Players tab's account panel and writes: each action's rows and event, the caps, the guard, the delete
//! cascade, the balance a logged-in player sees after a grant.
// Provenance: ORIGINAL (DeCombat tests)

use super::*;
use crate::clanweb::parse_request;
use crate::logs::Listener;
use crate::msg::{BALANCE, BALANCE_REPLY, LOGIN_REPLY, LOGIN_REQUEST, PROGRESS_PUSH};
use crate::state::testing::{connect, u32_at};
use crate::state::{Outgoing, Sid};
use dc_wire::PktWriter;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

const HOST: &str = "127.0.0.1:10090";
const ORIGIN: &str = "http://127.0.0.1:10090";

struct Conn {
    rx: Receiver<Outgoing>,
    got: Vec<(u16, Vec<u8>)>,
    closed: bool,
}

impl Conn {
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

    fn last(&mut self, id: u16) -> Option<Vec<u8>> {
        self.pump();
        self.got
            .iter()
            .rev()
            .find(|(i, _)| *i == id)
            .map(|(_, d)| d.clone())
    }
}

fn shared() -> Shared {
    Arc::new(Mutex::new(crate::state::testing::state()))
}

/// An account with a character; `sid` Some = logged into the lobby there, in channel 1.
fn player(sh: &Shared, login: &str, nick: &str, sid: Option<Sid>) -> (u32, Option<Conn>) {
    let mut g = sh.lock().unwrap();
    let st = &mut *g;
    let acc = db::create_account(&st.db, login, 0).unwrap();
    st.scope(|st| crate::auth::create_character(st, acc, nick, 2))
        .unwrap();
    let conn = sid.map(|sid| {
        let rx = connect(st, sid, Listener::Lobby);
        let s = st.sessions.get_mut(&sid).unwrap();
        s.account = Some(acc);
        s.in_lobby = true;
        s.channel = Some(1);
        Conn {
            rx,
            got: Vec::new(),
            closed: false,
        }
    });
    (acc, conn)
}

fn page_token(sh: &Shared) -> String {
    let mut g = sh.lock().unwrap();
    let st = &mut *g;
    st.beta.csrf(&mut st.keys)
}

fn post_with(
    sh: &Shared,
    action: &str,
    headers: &[(&str, &str)],
    body: &Value,
    guard: Option<&WriteGuard>,
) -> (u16, Value) {
    let mut head = format!("POST /ops/player/{action} HTTP/1.1\r\nHost: {HOST}\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let req = parse_request(&head).unwrap();
    let (status, _, text) = serve(sh, &req, &head, body.to_string().as_bytes(), guard);
    (status, serde_json::from_str(&text).unwrap())
}

fn post(sh: &Shared, action: &str, body: Value) -> (u16, Value) {
    let tok = page_token(sh);
    post_with(
        sh,
        action,
        &[
            ("Content-Type", "application/json"),
            ("Origin", ORIGIN),
            ("X-Admin-CSRF", &tok),
        ],
        &body,
        None,
    )
}

fn panel(sh: &Shared, account: u32) -> Value {
    let head = format!("GET /ops/player?account={account} HTTP/1.1\r\nHost: {HOST}\r\n\r\n");
    let req = parse_request(&head).unwrap();
    let (status, _, text) = serve(sh, &req, &head, &[], None);
    assert_eq!(status, 200, "{text}");
    serde_json::from_str(&text).unwrap()
}

fn count(sh: &Shared, sql: &str, a: u32) -> i64 {
    sh.lock()
        .unwrap()
        .db
        .query_row(sql, [a as i64], |r| r.get(0))
        .unwrap()
}

fn events(sh: &Shared, a: u32) -> Vec<String> {
    let g = sh.lock().unwrap();
    let mut q = g
        .db
        .prepare("SELECT detail FROM ops_events WHERE user_no = ?1 AND kind = 'admin' AND problem = 0 ORDER BY id")
        .unwrap();
    let rows = q.query_map([a as i64], |r| r.get(0)).unwrap();
    rows.map(Result::unwrap).collect()
}

fn balance(sh: &Shared, a: u32) -> (i64, i64, i64) {
    let c = db::character(&sh.lock().unwrap().db, a).unwrap().unwrap();
    (c.nx, c.gp, c.exp)
}

/// The 2009 auth login of `login` on a fresh connection: the 0x201 reply.
fn auth_login(sh: &Shared, sid: Sid, login: &str) -> Vec<u8> {
    let mut g = sh.lock().unwrap();
    let st = &mut *g;
    let rx = connect(st, sid, Listener::Auth);
    let mut w = PktWriter::new(LOGIN_REQUEST);
    w.wstr(&format!("LOCAL:{login}"), 4096)
        .wstr("pw", 17)
        .wstr("00-11", 20)
        .i32(0);
    let p = w.into_payload();
    st.scope(|st| crate::net::route(st, sid, LOGIN_REQUEST as i32, &p))
        .unwrap();
    st.sessions.remove(&sid);
    let mut c = Conn {
        rx,
        got: Vec::new(),
        closed: false,
    };
    c.last(LOGIN_REPLY).expect("a login reply")
}

#[test]
fn a_grant_changes_the_balances_logs_it_and_reaches_the_online_game() {
    let sh = shared();
    let (a, conn) = player(&sh, "anna", "Anna", Some(1));
    let mut conn = conn.unwrap();
    let (_, gp0, exp0) = balance(&sh, a);
    let (s, v) = post(
        &sh,
        "grant",
        json!({"account": a, "nx": 500, "gp": "1,000", "reason": "event prize"}),
    );
    assert_eq!(s, 200, "{v}");
    assert!(v["message"].as_str().unwrap().contains("online"), "{v}");
    let (nx, gp, _) = balance(&sh, a);
    assert_eq!((nx, gp), (500, gp0 + 1000));
    // the pushes: 0x345 with the new prepaid balance, 0x3004 with the totals and no popup
    let b = conn.last(BALANCE_REPLY).expect("balance pushed");
    assert_eq!((u32_at(&b, 6), u32_at(&b, 0x12)), (1, 500));
    let p = conn.last(PROGRESS_PUSH).expect("progress pushed");
    assert_eq!((u32_at(&p, 0xa), u32_at(&p, 0xe)), ((gp0 + 1000) as u32, 0));
    // the shop's balance request (0x344) answers the new amount on the same session
    {
        let mut g = sh.lock().unwrap();
        let p = PktWriter::new(BALANCE).into_payload();
        g.scope(|st| crate::net::route(st, 1, BALANCE as i32, &p))
            .unwrap();
    }
    conn.got.clear();
    let b = conn.last(BALANCE_REPLY).expect("balance reply");
    assert_eq!(u32_at(&b, 0x12), 500);
    // taking back, logged with who did it
    let (s, v) = post(
        &sh,
        "grant",
        json!({"account": a, "nx": -200, "exp": 50, "reason": "correction"}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(balance(&sh, a), (300, gp0 + 1000, exp0 + 50));
    let ev = events(&sh, a);
    assert_eq!(ev.len(), 2);
    assert!(
        ev[0].contains("NX +500 (now 500)")
            && ev[0].contains("GP +1,000 (now")
            && ev[0].contains("the admin port"),
        "{ev:?}"
    );
    assert!(
        ev[1].contains("NX -200 (now 300)") && ev[1].contains("correction"),
        "{ev:?}"
    );
    assert_eq!(
        count(
            &sh,
            "SELECT COUNT(*) FROM op_actions WHERE target = ?1 AND action = 'grant' AND by_no = 0",
            a
        ),
        2
    );
}

#[test]
fn grants_are_capped_need_a_reason_and_never_go_below_zero() {
    let sh = shared();
    let (a, _) = player(&sh, "bert", "Bert", None);
    sh.lock().unwrap().rules.ops.admin_max_nx = 1000;
    let start = balance(&sh, a);
    let refused = |body: Value| post(&sh, "grant", body).0;
    assert_eq!(
        refused(json!({"account": a, "nx": 1001, "reason": "x"})),
        400,
        "over the cap"
    );
    assert_eq!(
        refused(json!({"account": a, "nx": -1001, "reason": "x"})),
        400,
        "over the cap, taking"
    );
    assert_eq!(refused(json!({"account": a, "nx": 10})), 400, "no reason");
    assert_eq!(
        refused(json!({"account": a, "reason": "x"})),
        400,
        "no amount"
    );
    assert_eq!(
        refused(json!({"account": a, "nx": 1.5, "reason": "x"})),
        400,
        "not whole"
    );
    assert_eq!(
        refused(json!({"account": a, "nx": -1, "reason": "x"})),
        409,
        "below zero"
    );
    let gp_below = -(start.1 + 1);
    assert_eq!(
        refused(json!({"account": a, "gp": gp_below, "reason": "x"})),
        409,
        "below zero"
    );
    assert_eq!(
        refused(json!({"account": 999, "nx": 1, "reason": "x"})),
        404
    );
    assert_eq!(balance(&sh, a), start, "nothing changed");
    assert!(events(&sh, a).is_empty());
    assert_eq!(
        post(
            &sh,
            "grant",
            json!({"account": a, "nx": 1000, "reason": "x"})
        )
        .0,
        200
    );
    assert_eq!(
        post(
            &sh,
            "grant",
            json!({"account": a, "nx": -1000, "reason": "x"})
        )
        .0,
        200
    );
    assert_eq!(balance(&sh, a).0, 0);
}

#[test]
fn a_ban_drops_the_connections_and_refuses_the_login_until_lifted() {
    let sh = shared();
    let (a, conn) = player(&sh, "carla", "Carla", Some(1));
    let mut conn = conn.unwrap();
    assert_eq!(
        u32_at(&auth_login(&sh, 20, "carla"), 6),
        1,
        "logs in before"
    );
    assert_eq!(
        post(&sh, "ban", json!({"account": a, "days": 2, "reason": ""})).0,
        400,
        "a reason"
    );
    assert_eq!(
        post(&sh, "ban", json!({"account": a, "reason": "cheating"})).0,
        400,
        "a length"
    );
    let before = clock::unix_now();
    let (s, v) = post(
        &sh,
        "ban",
        json!({"account": a, "days": 2, "hours": 3, "reason": "cheating"}),
    );
    assert_eq!(s, 200, "{v}");
    conn.pump();
    assert!(conn.closed, "the lobby connection is dropped");
    assert!(!sh.lock().unwrap().sessions.contains_key(&1));
    let (until, by): (i64, String) = sh
        .lock()
        .unwrap()
        .db
        .query_row(
            "SELECT until, by_name FROM op_bans WHERE user_no = ?1",
            [a as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(
        (before + 51 * 3600..=before + 51 * 3600 + 5).contains(&until),
        "{until}"
    );
    assert_eq!(by, "the admin port");
    let d = auth_login(&sh, 21, "carla");
    assert_eq!(
        (u32_at(&d, 0xa), u32_at(&d, 0x2a)),
        (gm::REASON_LOGIN_BANNED, 3),
        "2 days 3 h -> 3 days"
    );
    let p = panel(&sh, a);
    assert_eq!(p["ban"]["permanent"], false);
    assert_eq!(p["ban"]["reason"], "cheating");
    assert_eq!(p["ban"]["by"], "the admin port");
    assert!(
        events(&sh, a)[0].contains("banned for 51 hours"),
        "{:?}",
        events(&sh, a)
    );
    // permanent, then lifted
    assert_eq!(
        post(
            &sh,
            "ban",
            json!({"account": a, "permanent": true, "reason": "again"})
        )
        .0,
        200
    );
    assert_eq!(panel(&sh, a)["ban"]["permanent"], true);
    assert_eq!(post(&sh, "unban", json!({"account": a})).0, 200);
    assert_eq!(
        post(&sh, "unban", json!({"account": a})).0,
        409,
        "not banned now"
    );
    assert!(panel(&sh, a)["ban"].is_null());
    assert_eq!(u32_at(&auth_login(&sh, 22, "carla"), 6), 1, "logs in again");
}

#[test]
fn kick_mute_disable_and_the_reset_mail() {
    let sh = shared();
    let (a, conn) = player(&sh, "dora", "Dora", Some(1));
    let mut conn = conn.unwrap();
    // mute: stored, pushed, shown; unmute
    assert_eq!(
        post(&sh, "mute", json!({"account": a, "minutes": 0})).0,
        400
    );
    let (s, v) = post(
        &sh,
        "mute",
        json!({"account": a, "minutes": 30, "reason": "spam"}),
    );
    assert_eq!(s, 200, "{v}");
    assert!(conn.last(gm::MUTE_UNTIL_PUSH).is_some());
    assert!(gm::mute_end(&sh.lock().unwrap(), a) > 0);
    assert_eq!(panel(&sh, a)["mute"]["minutes_left"], 30);
    assert_eq!(post(&sh, "unmute", json!({"account": a})).0, 200);
    assert_eq!(gm::mute_end(&sh.lock().unwrap(), a), 0);
    assert_eq!(post(&sh, "unmute", json!({"account": a})).0, 409);
    // disable / enable (LG-142)
    assert_eq!(post(&sh, "disable", json!({"account": a})).0, 200);
    assert!(
        db::account_by_no(&sh.lock().unwrap().db, a)
            .unwrap()
            .unwrap()
            .disabled
    );
    assert_eq!(
        post(&sh, "disable", json!({"account": a})).0,
        409,
        "already"
    );
    assert_eq!(panel(&sh, a)["disabled"], true);
    assert_eq!(post(&sh, "enable", json!({"account": a})).0, 200);
    assert!(
        !db::account_by_no(&sh.lock().unwrap().db, a)
            .unwrap()
            .unwrap()
            .disabled
    );
    // kick needs a reason and a connection
    assert_eq!(post(&sh, "kick", json!({"account": a})).0, 400);
    assert_eq!(
        post(&sh, "kick", json!({"account": a, "reason": "afk"})).0,
        200
    );
    conn.pump();
    assert!(conn.closed);
    assert_eq!(
        post(&sh, "kick", json!({"account": a, "reason": "afk"})).0,
        409,
        "offline now"
    );
    // the reset mail: needs mail and an address
    assert_eq!(
        post(&sh, "reset_mail", json!({"account": a})).0,
        409,
        "mail off"
    );
    sh.lock().unwrap().mail = crate::mail::Mailer::dry_run("");
    assert_eq!(
        post(&sh, "reset_mail", json!({"account": a})).0,
        409,
        "no address"
    );
    db::set_email(&sh.lock().unwrap().db, a, "dora@example.com").unwrap();
    let (s, v) = post(&sh, "reset_mail", json!({"account": a}));
    assert_eq!(s, 200, "{v}");
    assert!(v["message"].as_str().unwrap().contains("d***@example.com"));
    assert_eq!(sh.lock().unwrap().mail.dry.len(), 1);
    assert_eq!(
        count(
            &sh,
            "SELECT COUNT(*) FROM recovery_codes WHERE user_no = ?1",
            a
        ),
        1
    );
    let ev = events(&sh, a);
    for word in [
        "muted for 30 min",
        "mute lifted",
        "login disabled",
        "login enabled",
        "kicked",
        "password reset code",
    ] {
        assert!(ev.iter().any(|e| e.contains(word)), "{word}: {ev:?}");
    }
}

#[test]
fn the_write_guard_refuses_without_the_token_origin_or_json() {
    let sh = shared();
    let (a, _) = player(&sh, "erik", "Erik", None);
    let tok = page_token(&sh);
    let body = json!({"account": a, "nx": 5, "reason": "x"});
    let send = |h: &[(&str, &str)]| post_with(&sh, "grant", h, &body, None).0;
    assert_eq!(send(&[("Origin", ORIGIN), ("X-Admin-CSRF", &tok)]), 415);
    assert_eq!(
        send(&[("Content-Type", "application/json"), ("X-Admin-CSRF", &tok)]),
        403
    );
    assert_eq!(
        send(&[
            ("Content-Type", "application/json"),
            ("Origin", "http://evil.example"),
            ("X-Admin-CSRF", &tok)
        ]),
        403
    );
    assert_eq!(
        send(&[("Content-Type", "application/json"), ("Origin", ORIGIN)]),
        403
    );
    assert_eq!(
        send(&[
            ("Content-Type", "application/json"),
            ("Origin", ORIGIN),
            ("X-Admin-CSRF", "wrong")
        ]),
        403
    );
    assert_eq!(balance(&sh, a).0, 0, "nothing written");
    // the admin web: its own origin and session token, and the key named in the event
    let w = WriteGuard {
        origin: "https://admin.example.com".into(),
        csrf: "session-token".into(),
        by: "the admin web, key #3 from 192.0.2.10".into(),
    };
    let via_web = |origin: &str, t: &str| {
        post_with(
            &sh,
            "grant",
            &[
                ("Content-Type", "application/json"),
                ("Origin", origin),
                ("X-Admin-CSRF", t),
            ],
            &body,
            Some(&w),
        )
        .0
    };
    assert_eq!(
        via_web(ORIGIN, &tok),
        403,
        "the loopback page's token does not pass there"
    );
    assert_eq!(via_web("https://admin.example.com", "session-token"), 200);
    assert!(events(&sh, a)[0].contains("key #3"), "{:?}", events(&sh, a));
    // methods and paths
    let get = |p: &str| {
        let head = format!("GET {p} HTTP/1.1\r\nHost: {HOST}\r\n\r\n");
        serve(&sh, &parse_request(&head).unwrap(), &head, &[], None).0
    };
    assert_eq!(get("/ops/player/grant"), 405);
    assert_eq!(get("/ops/player/nope"), 404);
    assert_eq!(get("/ops/player"), 400);
    assert_eq!(get("/ops/player?account=999"), 404);
    assert!(handles("/ops/player") && handles("/ops/player/ban") && !handles("/ops/players"));
}

#[test]
fn the_panel_and_the_player_list_show_the_account_state() {
    let sh = shared();
    let (a, _) = player(&sh, "fay", "Fay", Some(1));
    let (b, _) = player(&sh, "gus", "Gus", None);
    let p = panel(&sh, a);
    assert_eq!(
        (
            p["login"].as_str(),
            p["nick"].as_str(),
            p["online"].as_bool()
        ),
        (Some("fay"), Some("Fay"), Some(true))
    );
    assert_eq!(p["where"], "channel 1 lobby");
    assert_eq!(
        (
            p["gm"].as_bool(),
            p["disabled"].as_bool(),
            p["bot"].as_bool()
        ),
        (Some(false), Some(false), Some(false))
    );
    assert!(p["ban"].is_null() && p["mute"].is_null());
    assert_eq!(p["caps"]["nx"], 100_000);
    assert_eq!(panel(&sh, b)["online"], false);
    assert_eq!(
        post(&sh, "ban", json!({"account": b, "hours": 5, "reason": "x"})).0,
        200
    );
    assert_eq!(
        post(&sh, "mute", json!({"account": a, "minutes": 5})).0,
        200
    );
    let g = sh.lock().unwrap();
    let none = std::collections::BTreeSet::new();
    let now = (clock::unix_now(), clock::now_ticks());
    let all = crate::opsmon::players_json_at(&g.db, &none, "", false, false, "", 0, 50, now)
        .unwrap()
        .to_json_string();
    assert!(
        all.contains("\"login\":\"gus\"") && all.contains("\"login\":\"fay\""),
        "{all}"
    );
    assert!(
        all.contains("\"banned\":true") && all.contains("\"muted\":true"),
        "{all}"
    );
    let banned = crate::opsmon::players_json_at(&g.db, &none, "", false, true, "", 0, 50, now)
        .unwrap()
        .to_json_string();
    assert!(
        banned.contains("\"login\":\"gus\"") && !banned.contains("\"login\":\"fay\""),
        "{banned}"
    );
    // an ended ban is not shown
    let later = (now.0 + 6 * 3600, now.1);
    let after = crate::opsmon::players_json_at(&g.db, &none, "", false, true, "", 0, 50, later)
        .unwrap()
        .to_json_string();
    assert!(after.contains("\"total\":0"), "{after}");
}

/// Every table with an account column, and the column (LG-396's per-table decisions are in docs/OPS.md).
const EMPTIED: &[(&str, &str)] = &[
    ("accounts", "user_no"),
    ("characters", "user_no"),
    ("items", "user_no"),
    ("function_slots", "user_no"),
    ("expiry_notes", "user_no"),
    ("inbox", "user_no"),
    ("notices", "user_no"),
    ("friends", "user_no"),
    ("friends", "friend_no"),
    ("friend_requests", "from_no"),
    ("friend_requests", "to_no"),
    ("recommends", "user_no"),
    ("coupon_uses", "user_no"),
    ("megaphone", "user_no"),
    ("clan_joins", "user_no"),
    ("op_bans", "user_no"),
    ("nx_daily", "user_no"),
    ("nx_clan_wins", "user_no"),
    ("nx_events", "user_no"),
    ("identity_links", "user_no"),
    ("recovery_codes", "user_no"),
    ("beta_signups", "user_no"),
];

fn seed(sh: &Shared, a: u32, login: &str, other: u32) {
    let g = sh.lock().unwrap();
    let n = a as i64;
    let o = other as i64;
    for (sql, p) in [
        ("INSERT INTO items (user_no, item_no) VALUES (?1, 101)", vec![n]),
        ("INSERT INTO function_slots (uid, user_no, slot) VALUES (?1 * 1000, ?1, 1)", vec![n]),
        ("INSERT INTO expiry_notes (user_no, item_no) VALUES (?1, 101)", vec![n]),
        ("INSERT INTO inbox (user_no, product_id) VALUES (?1, 7)", vec![n]),
        ("INSERT INTO notices (user_no, text) VALUES (?1, 'hi')", vec![n]),
        ("INSERT INTO friends (user_no, friend_no) VALUES (?1, ?2)", vec![n, o]),
        ("INSERT INTO friends (user_no, friend_no) VALUES (?1, 777)", vec![n]),
        ("INSERT INTO friend_requests (from_no, to_no) VALUES (?1, 777)", vec![n]),
        ("INSERT INTO recommends (user_no, recommender) VALUES (?1, 'x')", vec![n]),
        ("INSERT INTO coupon_uses (code, user_no) VALUES ('C1', ?1)", vec![n]),
        ("INSERT INTO megaphone (user_no, text) VALUES (?1, 'shout')", vec![n]),
        ("INSERT INTO op_bans (user_no, until) VALUES (?1, 1)", vec![n]),
        ("INSERT INTO nx_daily (user_no, last_day) VALUES (?1, 1)", vec![n]),
        ("INSERT INTO nx_clan_wins (user_no, day, wins) VALUES (?1, 1, 1)", vec![n]),
        ("INSERT INTO nx_events (event_id, user_no, amount, label, created) VALUES ('e', ?1, 1, 'l', 0)", vec![n]),
        ("INSERT INTO identity_links (issuer, remote_no, user_no) VALUES ('iss', ?1, ?1)", vec![n]),
        ("INSERT INTO recovery_codes (user_no, code_hash, created, expires) VALUES (?1, 'h', 0, 0)", vec![n]),
        ("INSERT INTO beta_signups (email, status, user_no) VALUES ('s' || ?1 || '@example.com', 'registered', ?1)", vec![n]),
        ("INSERT INTO shop_log (user_no, product_id) VALUES (?1, 7)", vec![n]),
    ] {
        g.db.execute(sql, rusqlite::params_from_iter(p)).unwrap();
    }
    g.db.execute(
        "INSERT INTO bot_loadouts (login, spec) VALUES (?1, '0:27')",
        [login],
    )
    .unwrap();
}

#[test]
fn delete_removes_everything_of_the_account_and_hands_its_clan_over() {
    let sh = shared();
    let (a, _) = player(&sh, "hank", "Hank", None);
    let (b, _) = player(&sh, "ines", "Ines", None);
    seed(&sh, a, "hank", b);
    seed(&sh, b, "ines", a);
    // Hank leads a clan, Ines is in it
    {
        let g = sh.lock().unwrap();
        g.db.execute(
            "INSERT INTO clans (name, master) VALUES ('Wolves', ?1)",
            [a as i64],
        )
        .unwrap();
        let uid = g.db.last_insert_rowid();
        for (who, role, joined) in [(a, crate::clan::ROLE_LEADER, 1), (b, 1, 2)] {
            g.db.execute(
                "UPDATE characters SET clan_uid = ?2, clan_name = 'Wolves', clan_type = ?3 WHERE user_no = ?1",
                params![who as i64, uid, role],
            )
            .unwrap();
            g.db.execute(
                "INSERT INTO clan_joins (user_no, clan_uid, joined) VALUES (?1, ?2, ?3)",
                params![who as i64, uid, joined],
            )
            .unwrap();
        }
    }
    assert_eq!(
        post(&sh, "delete", json!({"account": a, "confirm": "nope"})).0,
        400,
        "the login to confirm"
    );
    // refused while connected and for a GM
    {
        let mut g = sh.lock().unwrap();
        let _rx = connect(&mut g, 9, Listener::Lobby);
        g.sessions.get_mut(&9).unwrap().account = Some(a);
        g.sessions.get_mut(&9).unwrap().in_lobby = true;
    }
    assert_eq!(
        post(&sh, "delete", json!({"account": a, "confirm": "hank"})).0,
        409,
        "online"
    );
    sh.lock().unwrap().sessions.remove(&9);
    sh.lock()
        .unwrap()
        .db
        .execute("UPDATE accounts SET gm = 1 WHERE user_no = ?1", [a as i64])
        .unwrap();
    assert_eq!(
        post(&sh, "delete", json!({"account": a, "confirm": "hank"})).0,
        409,
        "a GM"
    );
    sh.lock()
        .unwrap()
        .db
        .execute("UPDATE accounts SET gm = 0 WHERE user_no = ?1", [a as i64])
        .unwrap();
    assert_eq!(
        count(&sh, "SELECT COUNT(*) FROM accounts WHERE user_no = ?1", a),
        1,
        "still there"
    );
    let (s, v) = post(&sh, "delete", json!({"account": a, "confirm": "HANK"}));
    assert_eq!(s, 200, "{v}");
    for (t, col) in EMPTIED {
        assert_eq!(
            count(
                &sh,
                &format!("SELECT COUNT(*) FROM {t} WHERE {col} = ?1"),
                a
            ),
            0,
            "{t}.{col}"
        );
    }
    assert_eq!(
        count(
            &sh,
            "SELECT COUNT(*) FROM bot_loadouts WHERE login = 'hank' AND ?1 > 0",
            a
        ),
        0
    );
    assert_eq!(count(&sh, "SELECT COUNT(*) FROM beta_signups WHERE status = 'registered' AND user_no = 0 AND ?1 > 0", a), 1, "row kept");
    assert_eq!(
        count(&sh, "SELECT COUNT(*) FROM shop_log WHERE user_no = ?1", a),
        1,
        "the sales record stays"
    );
    // Ines keeps her things (her friend row naming Hank is gone with him)
    for (t, col) in EMPTIED
        .iter()
        .filter(|(t, c)| !matches!((*t, *c), ("friends", _) | ("friend_requests", "to_no")))
    {
        assert!(
            count(
                &sh,
                &format!("SELECT COUNT(*) FROM {t} WHERE {col} = ?1"),
                b
            ) >= 1,
            "{t}.{col} of the other account"
        );
    }
    assert_eq!(
        count(&sh, "SELECT COUNT(*) FROM friends WHERE user_no = ?1", b),
        1,
        "her other friend"
    );
    assert_eq!(
        count(
            &sh,
            "SELECT COUNT(*) FROM bot_loadouts WHERE login = 'ines' AND ?1 > 0",
            b
        ),
        1
    );
    // the clan went to Ines
    assert_eq!(
        count(
            &sh,
            "SELECT master FROM clans WHERE name = 'Wolves' AND ?1 > 0",
            b
        ),
        b as i64
    );
    assert_eq!(
        count(
            &sh,
            "SELECT clan_type FROM characters WHERE user_no = ?1",
            b
        ),
        crate::clan::ROLE_LEADER
    );
    // the ops rows keep the number; the deletion is the last event
    let ev = events(&sh, a);
    assert!(
        ev.last()
            .unwrap()
            .contains("account deleted (login hank, character Hank)"),
        "{ev:?}"
    );
    assert_eq!(
        count(
            &sh,
            "SELECT COUNT(*) FROM op_actions WHERE target = ?1 AND action = 'delete'",
            a
        ),
        1
    );
    assert_eq!(
        post(&sh, "delete", json!({"account": a, "confirm": "hank"})).0,
        404,
        "gone"
    );
}

#[test]
fn a_leader_alone_disbands_the_clan_when_deleted() {
    let sh = shared();
    let (a, _) = player(&sh, "jon", "Jon", None);
    {
        let g = sh.lock().unwrap();
        g.db.execute(
            "INSERT INTO clans (name, master) VALUES ('Solo', ?1)",
            [a as i64],
        )
        .unwrap();
        let uid = g.db.last_insert_rowid();
        g.db.execute(
            "UPDATE characters SET clan_uid = ?2, clan_name = 'Solo', clan_type = ?3 WHERE user_no = ?1",
            params![a as i64, uid, crate::clan::ROLE_LEADER],
        )
        .unwrap();
        g.db.execute("INSERT INTO clan_records (clan_uid) VALUES (?1)", [uid])
            .unwrap();
    }
    assert_eq!(
        post(&sh, "delete", json!({"account": a, "confirm": "jon"})).0,
        200
    );
    assert_eq!(count(&sh, "SELECT COUNT(*) FROM clans WHERE ?1 > 0", a), 0);
    assert_eq!(
        count(&sh, "SELECT COUNT(*) FROM clan_records WHERE ?1 > 0", a),
        0
    );
}

#[test]
fn numbers_read_with_separators() {
    assert_eq!(grouped(1_234_567), "1,234,567");
    assert_eq!(grouped(-1000), "-1,000");
    assert_eq!(grouped(12), "12");
    assert_eq!(signed(5), "+5");
    assert_eq!(hours_text(48), "2 days");
    assert_eq!(hours_text(5), "5 hours");
    assert_eq!(mute_minutes_left(0, 10), 0);
    assert_eq!(mute_minutes_left(10 + 60 * clock::TICKS_PER_SEC + 1, 10), 2);
}
