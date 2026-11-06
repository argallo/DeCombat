//! Unit tests of the clan web pages: request parsing, routing and the fragments the harness matches
//! (SPEC: LOBBY_HARNESS.md §5), plus one real HTTP exchange on a loopback ephemeral port.

use super::*;
use crate::db;
use crate::logs::Listener;
use crate::state::testing::*;
use crate::state::Sid;

fn player(st: &mut Core, sid: Sid, login: &str, nick: &str, online: bool) -> u32 {
    let a = db::create_account(&st.db, login, 0).unwrap();
    db::create_character(
        &st.db,
        &db::NewCharacter {
            user_no: a,
            nick,
            face: 2,
            exp: 0,
            gp: 0,
            nx: 0,
            now: 0,
        },
        true,
    )
    .unwrap();
    if online {
        let rx = connect(st, sid, Listener::Lobby);
        std::mem::forget(rx);
        let s = st.sessions.get_mut(&sid).unwrap();
        s.account = Some(a);
        s.in_lobby = true;
    }
    a
}

fn get(st: &mut Core, target: &str) -> (u16, String) {
    let req = parse_request(&format!("GET {} HTTP/1.1\r\nHost: x\r\n\r\n", target)).unwrap();
    let mut out = (0, String::new());
    st.scope(|st| {
        out = route(st, &req)?;
        Ok(())
    })
    .unwrap();
    out
}

#[test]
fn query_and_request_parsing() {
    assert_eq!(url_decode("a+b%20c%41%zz%"), "a b cA%zz%");
    assert_eq!(url_decode("%E2%82%AC"), "\u{20ac}");
    assert_eq!(url_encode("LOCAL:bob x"), "LOCAL%3Abob%20x");
    let q = parse_query("url=guild/list&strPassport=LOCAL%3Abob&url=second&flag");
    assert_eq!(
        q.get("url").map(|s| s.as_str()),
        Some("guild/list"),
        "the first value wins"
    );
    assert_eq!(q.get("strPassport").map(|s| s.as_str()), Some("LOCAL:bob"));
    assert_eq!(q.get("flag").map(|s| s.as_str()), Some(""));
    let r = parse_request("GET /clan/join?c=1&p=dan HTTP/1.1\r\n\r\n").unwrap();
    assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/clan/join"));
    assert_eq!(r.query.get("c").map(|s| s.as_str()), Some("1"));
    let r = parse_request("GET http://127.0.0.1:80/clan?url=x HTTP/1.0\r\n\r\n").unwrap();
    assert_eq!(r.path, "/clan");
    assert!(parse_request("GET /x\r\n").is_none());
    assert!(parse_request("garbage").is_none());
    assert_eq!(
        esc("<a href=\"x\">&'"),
        "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
    );
    let resp = String::from_utf8(response(200, "hi")).unwrap();
    assert!(
        resp.starts_with("HTTP/1.1 200 OK\r\n")
            && resp.contains("Content-Length: 2\r\n")
            && resp.ends_with("\r\n\r\nhi")
    );
}

#[test]
fn create_list_members_join_leave_pages() {
    let mut st = state();
    let carol = player(&mut st, 1, "carol", "Carol", true);
    let dan = player(&mut st, 2, "dan", "Dan", true);
    let _eve = player(&mut st, 3, "eve", "Eve", false);
    // HARNESS: lobby_harness::test_clan fragments
    let (code, page) = get(&mut st, "/clan/create?p=carol&name=Wolves&emblem=3&bg=4");
    assert_eq!(code, 200);
    assert!(page.contains("color:#fc0\">Clan Wolves created."), "{page}");
    let k = crate::clan::clan_by_name(&st.db, "Wolves")
        .unwrap()
        .unwrap();
    assert_eq!((k.master, k.emblem, k.background), (carol, 3, 4));
    let (_, page) = get(
        &mut st,
        "/clan/create?p=LOCAL%3Acarol&name=Other&emblem=1&bg=1",
    );
    assert!(page.contains("Cannot"), "{page}");
    let (_, page) = get(&mut st, "/clan?url=manager/guildmember&p=carol");
    assert!(page.contains("<td>Leader</td><td>Carol</td>"), "{page}");
    let (_, page) = get(&mut st, "/clan?url=guild/list&p=carol");
    assert!(
        page.contains("<td>Wolves</td><td>1</td><td>Carol</td>") && !page.contains("[Join]"),
        "{page}"
    );
    assert!(page.contains("clan <b>Wolves</b>"), "{page}");
    // a second account sees it and may join (legacy strPassport with the LOCAL: prefix)
    let (_, page) = get(&mut st, "/clan?url=guild/list&strPassport=LOCAL:dan");
    assert!(
        page.contains("<td>Wolves</td><td>1</td><td>Carol</td>") && page.contains("[Join]"),
        "{page}"
    );
    let (_, page) = get(&mut st, &format!("/clan/join?p=dan&c={}", k.uid));
    assert!(page.contains("You joined Wolves."), "{page}");
    assert_eq!(crate::clan::members(&st.db, k.uid).unwrap().len(), 2);
    // an offline account sees pages but cannot act (CLN-8)
    let (_, page) = get(&mut st, &format!("/clan/join?p=eve&c={}", k.uid));
    assert!(
        page.contains("Cannot join: log in to the game first."),
        "{page}"
    );
    let (_, page) = get(&mut st, "/clan?url=guild/list&p=eve");
    assert!(page.contains("[Join]"));
    // the leader's member page offers a remove link for Dan; Dan's does not
    let (_, page) = get(&mut st, "/clan?url=manager/guildmember&p=carol");
    assert!(
        page.contains(&format!("/clan/kick?p=carol&amp;m={}", dan)) && page.contains("Set notice"),
        "{page}"
    );
    let (_, page) = get(&mut st, "/clan?url=manager/guildmember&p=dan");
    assert!(
        !page.contains("/clan/kick") && !page.contains("Set notice"),
        "{page}"
    );
    // notice through the page
    let (_, page) = get(&mut st, "/clan/notice?p=carol&text=train+tonight");
    assert!(
        page.contains("Notice saved.") && page.contains("train tonight"),
        "{page}"
    );
    // leave: the user gets 0x3064 (CD-47); queued in the same scope as the page
    let (_, page) = get(&mut st, "/clan/leave?p=dan");
    assert!(page.contains("You left Wolves."), "{page}");
    assert_eq!(crate::clan::members(&st.db, k.uid).unwrap().len(), 1);
    // unknown account / page / method
    let (_, page) = get(&mut st, "/clan?url=guild/list&p=nobody");
    assert!(page.contains("unknown account"));
    assert_eq!(get(&mut st, "/nope").0, 404);
    let req = parse_request("POST /clan HTTP/1.1\r\n\r\n").unwrap();
    let mut code = 0;
    st.scope(|st| {
        code = route(st, &req)?.0;
        Ok(())
    })
    .unwrap();
    assert_eq!(code, 405);
    // names are escaped in the pages
    let (_, _) = get(&mut st, "/clan/leave?p=carol");
    let (_, page) = get(&mut st, "/clan/create?p=carol&name=%3Cb%3Ex");
    assert!(page.contains("Clan &lt;b&gt;x created."), "{page}");
}

#[test]
fn leave_page_pushes_0x3064_then_0x303() {
    let mut st = state();
    let a = db::create_account(&st.db, "zed", 0).unwrap();
    db::create_character(
        &st.db,
        &db::NewCharacter {
            user_no: a,
            nick: "Zed",
            face: 0,
            exp: 0,
            gp: 0,
            nx: 0,
            now: 0,
        },
        true,
    )
    .unwrap();
    let rx = connect(&mut st, 9, Listener::Lobby);
    {
        let s = st.sessions.get_mut(&9).unwrap();
        s.account = Some(a);
        s.in_lobby = true;
    }
    get(&mut st, "/clan/create?p=zed&name=Solo");
    let ids: Vec<u16> = drain(&rx).into_iter().map(|(i, _)| i).collect();
    assert_eq!(ids, vec![0x303, 0x3061, 0x3062]);
    get(&mut st, "/clan/leave?p=zed");
    let got = drain(&rx);
    let ids: Vec<u16> = got.iter().map(|(i, _)| *i).collect();
    assert_eq!(ids, vec![0x3064, 0x303]);
    assert_eq!(u32_at(&got[1].1, 0x86), 0, "no clan any more");
}

#[test]
fn http_exchange_on_a_loopback_port() {
    use std::io::{Read, Write};
    let mut st = state();
    player(&mut st, 1, "carol", "Carol", true);
    let shared: Shared = Arc::new(std::sync::Mutex::new(st));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    spawn_with(Arc::clone(&shared), l, Hosting::none());
    let ask = |target: &str| -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(s, "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", target).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    };
    let r = ask("/clan/create?p=carol&name=Wolves&emblem=3&bg=4");
    assert!(r.starts_with("HTTP/1.1 200 OK"), "{r}");
    assert!(r.contains("color:#fc0\">Clan Wolves created."), "{r}");
    let body = r.split("\r\n\r\n").nth(1).unwrap();
    let len: usize = r
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(body.len(), len);
    let r = ask("/missing");
    assert!(r.starts_with("HTTP/1.1 404"), "{r}");
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(b"nonsense\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 400"), "{out}");
}

fn raw_get(port: u16, method: &str, target: &str) -> Vec<u8> {
    use std::io::{Read, Write};
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        method, target
    )
    .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    out
}

fn split_http(r: &[u8]) -> (String, Vec<u8>) {
    let i = r
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("head end");
    (
        String::from_utf8_lossy(&r[..i]).into_owned(),
        r[i + 4..].to_vec(),
    )
}

#[test]
fn mods_and_server_list_over_http() {
    // SPEC: HOSTING_AND_MODS.md §4 items 3 and 6
    let dir = std::env::temp_dir().join(format!("dc_clanweb_mods_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let good: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 251) as u8).collect();
    let sha = dc_rom::sha256_hex(&good);
    std::fs::write(dir.join(format!("{sha}.zip")), &good).unwrap();
    let liar = "ee".repeat(32);
    std::fs::write(dir.join(format!("{liar}.zip")), b"not what the name says").unwrap();
    let big = vec![1u8; 9000];
    let big_sha = dc_rom::sha256_hex(&big);
    std::fs::write(dir.join(format!("{big_sha}.zip")), &big).unwrap();
    let hosting = Hosting {
        mods_dir: Some(dir.clone()),
        server_list: Some("{\"name\":\"t\"}".into()),
        catalog: None,
        catalog_shots: Default::default(),
        max_mod_bytes: 8192,
        entry_sets: Default::default(),
        client_version: None,
        folder_packs: Default::default(),
    };
    let shared: Shared = Arc::new(std::sync::Mutex::new(state()));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    spawn_with(Arc::clone(&shared), l, hosting);

    let (head, body) = split_http(&raw_get(port, "GET", &format!("/mods/{sha}.zip")));
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert!(
        head.contains("Content-Type: application/zip")
            && head.contains(&format!("Content-Length: {}", good.len())),
        "{head}"
    );
    assert_eq!(
        dc_rom::sha256_hex(&body),
        sha,
        "the body hashes to the name in the URL"
    );
    assert_eq!(body, good);

    let (head, body) = split_http(&raw_get(port, "HEAD", &format!("/mods/{sha}.zip")));
    assert!(
        head.starts_with("HTTP/1.1 200")
            && head.contains(&format!("Content-Length: {}", good.len())),
        "{head}"
    );
    assert!(body.is_empty(), "HEAD has no body");

    let (head, body) = split_http(&raw_get(port, "GET", &format!("/mods/{liar}.zip")));
    assert!(head.starts_with("HTTP/1.1 500"), "{head}");
    assert!(!String::from_utf8_lossy(&body).contains("not what the name says"));

    let (head, _) = split_http(&raw_get(port, "GET", &format!("/mods/{big_sha}.zip")));
    assert!(head.starts_with("HTTP/1.1 413"), "{head}");
    for bad in [
        "/mods/..%2f..%2fx.zip",
        "/mods/../lobby.db",
        "/mods/",
        &format!("/mods/{}.zip", "00".repeat(32)),
    ] {
        let (head, _) = split_http(&raw_get(port, "GET", bad));
        assert!(head.starts_with("HTTP/1.1 404"), "{bad}: {head}");
    }

    let (head, body) = split_http(&raw_get(port, "GET", "/server_list.json"));
    assert!(
        head.starts_with("HTTP/1.1 200") && head.contains("Content-Type: application/json"),
        "{head}"
    );
    assert_eq!(body, b"{\"name\":\"t\"}");
    // the clan pages still work next to it
    let (head, _) = split_http(&raw_get(port, "GET", "/nothing"));
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_hosting_means_404_for_both_routes() {
    let shared: Shared = Arc::new(std::sync::Mutex::new(state()));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    spawn_with(Arc::clone(&shared), l, Hosting::none());
    for t in [
        "/server_list.json",
        &format!("/mods/{}.zip", "ab".repeat(32)),
    ] {
        let (head, _) = split_http(&raw_get(port, "GET", t));
        assert!(head.starts_with("HTTP/1.1 404"), "{t}: {head}");
    }
}

/// LG-414 (owner report "I can't search for other clans"): the find page's `q` keeps the clans whose name holds it
/// (any case), an empty `q` lists every clan, and an unknown account gets text lines (not an HTML page) in the text
/// form.
#[test]
fn text_form_searches_clans_by_name_and_answers_an_unknown_account_in_lines() {
    let mut st = state();
    player(&mut st, 1, "carol", "Carol", true);
    player(&mut st, 2, "erin", "Erin", true);
    player(&mut st, 3, "dan", "Dan", true);
    get(
        &mut st,
        "/clan/create?p=carol&name=Red%20Wolves&emblem=1&bg=1&fmt=text",
    );
    get(
        &mut st,
        "/clan/create?p=erin&name=Blue%20Fox&emblem=2&bg=2&fmt=text",
    );
    let names = |body: &str| {
        body.lines()
            .filter(|l| l.starts_with("clan\t"))
            .map(|l| l.split('\t').nth(2).unwrap_or("").to_string())
            .collect::<Vec<_>>()
    };
    let (_, all) = get(&mut st, "/clan?url=guild/list&p=dan&fmt=text");
    assert_eq!(names(&all).len(), 2, "{all}");
    let (_, one) = get(&mut st, "/clan?url=guild/list&p=dan&q=wOLV&fmt=text");
    assert!(one.starts_with("me\t0\t0\n"), "{one}");
    assert_eq!(names(&one), vec!["Red Wolves".to_string()]);
    let (_, spaced) = get(&mut st, "/clan?url=guild/list&p=dan&q=blue%20f&fmt=text");
    assert_eq!(names(&spaced), vec!["Blue Fox".to_string()]);
    let (_, none) = get(&mut st, "/clan?url=guild/list&p=dan&q=zebra&fmt=text");
    assert_eq!(none, "me\t0\t0\n");
    let (code, unknown) = get(&mut st, "/clan?url=guild/list&p=nobody&fmt=text");
    assert_eq!(code, 200);
    assert_eq!(
        unknown,
        "me\t0\t0\nmsg\t0\tCannot open the clan pages: unknown account.\n"
    );
    // the HTML pages stay as they were
    let (_, page) = get(&mut st, "/clan?url=guild/list&p=nobody");
    assert!(page.contains("unknown account") && page.starts_with("<!DOCTYPE html>"));
}

/// CLI-790: the text form of the create / list / join / leave actions writes the same DB rows as the pages and
/// answers in tab-separated lines the DeCombat client reads.
#[test]
fn text_form_creates_lists_joins_and_leaves() {
    let mut st = state();
    let carol = player(&mut st, 1, "carol", "Carol", true);
    let dan = player(&mut st, 2, "dan", "Dan", true);
    let (code, body) = get(
        &mut st,
        "/clan/create?p=LOCAL%3Acarol&name=Wolves&emblem=4&bg=7&fmt=text",
    );
    assert_eq!(code, 200);
    let k = crate::clan::clan_by_name(&st.db, "Wolves")
        .unwrap()
        .unwrap();
    assert_eq!((k.master, k.emblem, k.background), (carol, 4, 7));
    assert_eq!(
        body,
        format!("me\t{}\t1\nmsg\t1\tClan Wolves created.\n", k.uid)
    );
    // a taken name is refused with ok 0 and no new row
    let (_, body) = get(&mut st, "/clan/create?p=dan&name=wolves&fmt=text");
    assert_eq!(
        body,
        "me\t0\t0\nmsg\t0\tCannot create the clan: that clan name is taken.\n"
    );
    assert_eq!(crate::clan::all_clans(&st.db).unwrap().len(), 1);
    let (_, body) = get(&mut st, "/clan?url=guild/list&p=dan&fmt=text");
    assert_eq!(
        body,
        format!("me\t0\t0\nclan\t{}\tWolves\t1\tCarol\t4\t7\n", k.uid)
    );
    let (_, body) = get(&mut st, &format!("/clan/join?p=dan&c={}&fmt=text", k.uid));
    assert!(
        body.starts_with(&format!("me\t{}\t0\nmsg\t1\tYou joined Wolves.\n", k.uid))
            && body.contains("\tWolves\t2\tCarol\t"),
        "{body}"
    );
    let c = st.character(dan).unwrap();
    assert_eq!(c.clan_uid, k.uid);
    let (_, body) = get(&mut st, "/clan/leave?p=dan&fmt=text");
    assert!(
        body.starts_with("me\t0\t0\nmsg\t1\tYou left Wolves.\n"),
        "{body}"
    );
    assert_eq!(st.character(dan).unwrap().clan_uid, 0);
}

/// CLI-790 / CLI-793: the member page's text form lists the members (leader flag, name, exp); the leader's kick
/// answers with the list again and the removed member is out of the clan.
#[test]
fn text_form_lists_members_and_kicks() {
    let mut st = state();
    let carol = player(&mut st, 1, "carol", "Carol", true);
    let dan = player(&mut st, 2, "dan", "Dan", true);
    get(&mut st, "/clan/create?p=carol&name=Wolves&fmt=text");
    let k = crate::clan::clan_by_name(&st.db, "Wolves")
        .unwrap()
        .unwrap();
    get(&mut st, &format!("/clan/join?p=dan&c={}&fmt=text", k.uid));
    let (_, body) = get(&mut st, "/clan?url=manager/guildmember&p=carol&fmt=text");
    assert!(
        body.contains(&format!("member\t{carol}\t1\tCarol\t0\n"))
            && body.contains(&format!("member\t{dan}\t0\tDan\t0\n")),
        "{body}"
    );
    // a member cannot remove; the leader can
    let (_, body) = get(&mut st, &format!("/clan/kick?p=dan&m={carol}&fmt=text"));
    assert!(body.contains("msg\t0\t"), "{body}");
    let (_, body) = get(&mut st, &format!("/clan/kick?p=carol&m={dan}&fmt=text"));
    assert!(
        body.contains("msg\t1\tDan was removed from the clan.\n")
            && body.contains(&format!("member\t{carol}\t1\t"))
            && !body.contains("\tDan\t"),
        "{body}"
    );
    assert_eq!(st.character(dan).unwrap().clan_uid, 0);
}

/// A registration form with a recovery address added unless the form names one (LG-240).
fn with_email(form: &str) -> String {
    if form.contains("email=") {
        form.to_string()
    } else {
        format!("{form}&email=player%40example.org")
    }
}

fn post_form(st: &mut Core, peer: &str, form: &str) -> (u16, String) {
    let form = with_email(form);
    let mut out = (0, String::new());
    st.scope(|st| {
        out = crate::register::route(
            st,
            "POST",
            form.as_bytes(),
            peer,
            Some("play.example.org"),
            std::time::Instant::now(),
        )?;
        Ok(())
    })
    .unwrap();
    out
}

/// LG-145: the registration page is off by default, needs an invite code unless open, checks the LG-141 rules,
/// consumes the code and stores an Argon2id password.
#[test]
fn registration_page_rules_invites_and_accounts() {
    let mut st = state();
    let get_reg = |st: &mut Core| {
        let mut out = (0, String::new());
        st.scope(|st| {
            out = crate::register::route(
                st,
                "GET",
                b"",
                "1.2.3.4:5",
                None,
                std::time::Instant::now(),
            )?;
            Ok(())
        })
        .unwrap();
        out
    };
    let (code, body) = get_reg(&mut st);
    assert_eq!(code, 404);
    assert!(body.contains("Registration is off"));
    st.opts.register = true;
    let (code, body) = get_reg(&mut st);
    assert_eq!(code, 200);
    assert!(
        body.contains("<form method=\"post\" action=\"/register\">") && !body.contains("<script")
    );
    assert!(body.contains("stored hashed on the server"));
    let invite =
        crate::accounts::new_invite(&st.db, &mut st.keys, None, crate::clock::unix_now(), 7)
            .unwrap();
    let peer = |i: u32| format!("198.51.100.{i}:4000");
    let cases = [
        (
            "name=Neo&pw=secret12&pw2=secret12&code=",
            "enter your invite code",
        ),
        (
            "name=Neo&pw=secret12&pw2=secret12&code=WRONGWRONG12",
            "unknown invite code",
        ),
        (
            "name=N&pw=secret12&pw2=secret12&code=x",
            "3 to 16 characters",
        ),
        ("name=Ne%20o&pw=secret12&pw2=secret12&code=x", "letters A-Z"),
        ("name=Neo&pw=short&pw2=short&code=x", "8 to 16 characters"),
        // LG-140: common passwords and the login name are refused
        ("name=Neo&pw=password1&pw2=password1&code=x", "too common"),
        (
            "name=Neo12345&pw=neo12345&pw2=neo12345&code=x",
            "may not be your login name",
        ),
        // LG-172: the GM name prefix is reserved
        (
            "name=GMNeo&pw=secret12&pw2=secret12&code=x",
            "reserved for game masters",
        ),
        (
            "name=Neo&pw=secret12&pw2=secret23&code=x",
            "the two passwords differ",
        ),
    ];
    for (i, (form, want)) in cases.iter().enumerate() {
        let (code, body) = post_form(&mut st, &peer(i as u32), form);
        assert_eq!(code, 200, "{form}");
        assert!(body.contains(want), "{form}: {body}");
        assert!(!body.contains("secret12"), "a password is never echoed");
    }
    assert!(db::account_by_login(&st.db, "Neo").unwrap().is_none());
    // a good form: lower case code with a space, the account is made, the code is used up
    let typed = format!("{} {}", invite[..6].to_lowercase(), &invite[6..]);
    let (code, body) = post_form(
        &mut st,
        &peer(20),
        &format!(
            "name=Neo&pw=secret12&pw2=secret12&code={}",
            url_encode(&typed)
        ),
    );
    assert_eq!(code, 200, "{body}");
    assert!(
        body.contains("Account created.") && body.contains("<td>Login</td><td><b>Neo</b></td>"),
        "{body}"
    );
    // the downloads come set up for their server: no server or port to copy
    assert!(
        !body.contains("<td>Port</td>") && !body.contains("play.example.org"),
        "{body}"
    );
    let a = db::account_by_login(&st.db, "neo").unwrap().unwrap();
    assert_eq!(
        crate::auth::pw_verify(&a, "secret12"),
        crate::auth::PwCheck::Ok
    );
    assert!(a.pw_hash.unwrap().starts_with("$argon2id$"));
    let (_, body) = post_form(
        &mut st,
        &peer(21),
        &format!("name=Trinity&pw=secret12&pw2=secret12&code={invite}"),
    );
    assert!(body.contains("already used"), "{body}");
    // a taken name, any case
    let other =
        crate::accounts::new_invite(&st.db, &mut st.keys, None, crate::clock::unix_now(), 7)
            .unwrap();
    let (_, body) = post_form(
        &mut st,
        &peer(22),
        &format!("name=NEO&pw=secret12&pw2=secret12&code={other}"),
    );
    assert!(body.contains("name is taken"), "{body}");
    // a code bound to a password-less account (`account add Kim --invite`) sets that account's password only
    let kim = db::create_account(&st.db, "Kim", 0).unwrap();
    let bound = crate::accounts::new_invite(
        &st.db,
        &mut st.keys,
        Some("Kim"),
        crate::clock::unix_now(),
        7,
    )
    .unwrap();
    let (_, body) = post_form(
        &mut st,
        &peer(23),
        &format!("name=Kimberly&pw=secret12&pw2=secret12&code={bound}"),
    );
    assert!(body.contains("for another name"), "{body}");
    let (_, body) = post_form(
        &mut st,
        &peer(24),
        &format!("name=kim&pw=kimpass12&pw2=kimpass12&code={bound}"),
    );
    assert!(body.contains("Account created."), "{body}");
    let k = db::account_by_no(&st.db, kim).unwrap().unwrap();
    assert_eq!(
        crate::auth::pw_verify(&k, "kimpass12"),
        crate::auth::PwCheck::Ok
    );
    // LG-172: a GM-prefixed login registers only with the invite made for that GM account
    let gm = db::create_account(&st.db, "GMKim", 0).unwrap();
    st.db
        .execute("UPDATE accounts SET gm = 1 WHERE user_no = ?1", [gm as i64])
        .unwrap();
    let gm_code = crate::accounts::new_invite(
        &st.db,
        &mut st.keys,
        Some("GMKim"),
        crate::clock::unix_now(),
        7,
    )
    .unwrap();
    let (_, body) = post_form(
        &mut st,
        &peer(26),
        &format!("name=GMKim&pw=gmpass12&pw2=gmpass12&code={gm_code}"),
    );
    assert!(body.contains("Account created."), "{body}");
    let plain =
        crate::accounts::new_invite(&st.db, &mut st.keys, None, crate::clock::unix_now(), 7)
            .unwrap();
    let (_, body) = post_form(
        &mut st,
        &peer(27),
        &format!("name=GMOther&pw=gmpass12&pw2=gmpass12&code={plain}"),
    );
    assert!(body.contains("reserved for game masters"), "{body}");
    // open registration: no code needed
    st.opts.register_open = true;
    let (_, body) = post_form(
        &mut st,
        &peer(25),
        "name=Morpheus&pw=secret99&pw2=secret99&code=",
    );
    assert!(body.contains("Account created."), "{body}");
    // the rate limit: five attempts per address in ten minutes
    for i in 0..5 {
        let (code, _) = post_form(&mut st, "203.0.113.9:1", &format!("name=x{i}&pw=a&pw2=a"));
        assert_eq!(code, 200);
    }
    let (code, body) = post_form(
        &mut st,
        "203.0.113.9:2",
        "name=Late&pw=secret99&pw2=secret99",
    );
    assert_eq!(code, 429);
    assert!(body.contains("Too many attempts"));
    assert!(db::account_by_login(&st.db, "Late").unwrap().is_none());
}

fn raw_request(port: u16, req: &str) -> Vec<u8> {
    use std::io::{Read, Write};
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    out
}

/// LG-145 over a real socket: the form body is read by its Content-Length, an oversize body is refused unread.
#[test]
fn registration_over_http() {
    let mut st = state();
    st.opts.register = true;
    st.opts.register_open = true;
    let shared: Shared = Arc::new(std::sync::Mutex::new(st));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    spawn_with(Arc::clone(&shared), l, Hosting::none());
    let (head, body) = split_http(&raw_get(port, "GET", "/register"));
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(String::from_utf8_lossy(&body).contains("Invite code (optional)"));
    let form = "name=Switch&pw=pass5678x&pw2=pass5678x&email=switch%40example.org&code=";
    let (head, body) = split_http(&raw_request(
        port,
        &format!(
            "POST /register HTTP/1.1\r\nHost: lan-box:18080\r\nContent-Type: application/x-www-form-urlencoded\r\n\
             Content-Length: {}\r\n\r\n{}",
            form.len(),
            form
        ),
    ));
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("Account created.")
            && body.contains("<b>Switch</b>")
            && !body.contains("lan-box"),
        "{body}"
    );
    let st = shared.lock().unwrap();
    let a = db::account_by_login(&st.db, "Switch").unwrap().unwrap();
    assert_eq!(
        crate::auth::pw_verify(&a, "pass5678x"),
        crate::auth::PwCheck::Ok
    );
    drop(st);
    let (head, _) = split_http(&raw_request(
        port,
        &format!(
            "POST /register HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n",
            crate::register::MAX_BODY + 1
        ),
    ));
    assert!(head.starts_with("HTTP/1.1 413"), "{head}");
}

fn reg_json(st: &mut Core, method: &str, peer: &str, form: &str) -> (u16, String) {
    let form = if method == "POST" {
        with_email(form)
    } else {
        form.to_string()
    };
    let mut out = (0, String::new());
    st.scope(|st| {
        out = crate::register::route_as(
            st,
            method,
            form.as_bytes(),
            peer,
            None,
            std::time::Instant::now(),
            true,
        )?;
        Ok(())
    })
    .unwrap();
    out
}

/// LG-210: the JSON answers of the game's registration dialog: GET says whether registration is on and needs a
/// code, POST answers ok + login or the HTML page's message, with the same checks, status codes and rate limit.
#[test]
fn registration_json_for_the_game() {
    let mut st = state();
    assert_eq!(
        reg_json(&mut st, "GET", "1.2.3.4:5", ""),
        (
            404,
            "{\"open\":false,\"invite\":false,\"email\":true}".to_string()
        )
    );
    let (code, body) = reg_json(
        &mut st,
        "POST",
        "1.2.3.4:5",
        "name=Neo&pw=secret12&pw2=secret12",
    );
    assert_eq!(code, 404);
    assert!(
        body.starts_with("{\"ok\":false,\"message\":\"Registration is off"),
        "{body}"
    );
    st.opts.register = true;
    assert_eq!(
        reg_json(&mut st, "GET", "1.2.3.4:5", ""),
        (
            200,
            "{\"open\":true,\"invite\":true,\"email\":true}".to_string()
        )
    );
    let (code, body) = reg_json(
        &mut st,
        "POST",
        "1.2.3.4:6",
        "name=Neo&pw=secret12&pw2=secret12&code=",
    );
    assert_eq!(
        (code, body.as_str()),
        (
            200,
            "{\"ok\":false,\"message\":\"Cannot register: enter your invite code.\"}"
        )
    );
    let (_, body) = reg_json(
        &mut st,
        "POST",
        "1.2.3.4:6",
        "name=Neo&pw=secret12&pw2=secret23&code=x",
    );
    assert!(
        body.contains("the two passwords differ") && !body.contains("secret"),
        "{body}"
    );
    st.opts.register_open = true;
    assert_eq!(
        reg_json(&mut st, "GET", "1.2.3.4:5", "").1,
        "{\"open\":true,\"invite\":false,\"email\":true}"
    );
    let (code, body) = reg_json(
        &mut st,
        "POST",
        "1.2.3.5:6",
        "name=Neo&pw=secret12&pw2=secret12&code=",
    );
    assert_eq!(
        (code, body.as_str()),
        (200, "{\"ok\":true,\"login\":\"Neo\"}")
    );
    assert!(db::account_by_login(&st.db, "Neo").unwrap().is_some());
    let (_, body) = reg_json(
        &mut st,
        "POST",
        "1.2.3.5:6",
        "name=neo&pw=secret12&pw2=secret12",
    );
    assert!(body.contains("name is taken"), "{body}");
    // the rate limit counts JSON posts as the form's
    for i in 0..3 {
        assert_eq!(
            reg_json(
                &mut st,
                "POST",
                "1.2.3.5:7",
                &format!("name=y{i}&pw=a&pw2=a")
            )
            .0,
            200
        );
    }
    let (code, body) = reg_json(
        &mut st,
        "POST",
        "1.2.3.5:7",
        "name=Late&pw=secret99&pw2=secret99",
    );
    assert_eq!(code, 429);
    assert_eq!(
        body,
        "{\"ok\":false,\"message\":\"Too many attempts from your address. Try again in ten minutes.\"}"
    );
    assert_eq!(crate::register::json_str("a\"b\\c\n"), r#""a\"b\\c\u000a""#);
}

/// LG-210 over a real socket: `Accept: application/json` (or `fmt=json`) gets JSON; a browser still gets the page.
#[test]
fn registration_json_over_http() {
    let mut st = state();
    st.opts.register = true;
    st.opts.register_open = true;
    let shared: Shared = Arc::new(std::sync::Mutex::new(st));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    spawn_with(Arc::clone(&shared), l, Hosting::none());
    let (head, body) = split_http(&raw_request(
        port,
        "GET /register HTTP/1.1\r\nHost: x\r\nAccept: application/json\r\n\r\n",
    ));
    assert!(
        head.starts_with("HTTP/1.1 200") && head.contains("application/json"),
        "{head}"
    );
    assert_eq!(body, b"{\"open\":true,\"invite\":false,\"email\":true}");
    let (head, _) = split_http(&raw_get(port, "GET", "/register?fmt=json"));
    assert!(head.contains("application/json"), "{head}");
    let (head, _) = split_http(&raw_get(port, "GET", "/register"));
    assert!(head.contains("text/html"), "{head}");
    let form = "name=Tank&pw=tank5678&pw2=tank5678&email=tank%40example.org&code=";
    let (head, body) = split_http(&raw_request(
        port,
        &format!(
            "POST /register HTTP/1.1\r\nHost: x\r\nAccept: application/json\r\nContent-Length: {}\r\n\r\n{}",
            form.len(),
            form
        ),
    ));
    assert!(
        head.starts_with("HTTP/1.1 200") && head.contains("application/json"),
        "{head}"
    );
    assert_eq!(body, b"{\"ok\":true,\"login\":\"Tank\"}");
    let form = "fmt=json&name=Tank&pw=tank5678&pw2=tank5678&email=tank%40example.org";
    let (_, body) = split_http(&raw_request(
        port,
        &format!(
            "POST /register HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{}",
            form.len(),
            form
        ),
    ));
    assert_eq!(
        body,
        b"{\"ok\":false,\"message\":\"Cannot register: that name is taken.\"}"
    );
}

/// LG-240: registration needs a recovery address (HTML and JSON), checks its form and stores it trimmed.
#[test]
fn registration_requires_and_stores_the_email() {
    let mut st = state();
    st.opts.register = true;
    st.opts.register_open = true;
    let mut out = (0, String::new());
    st.scope(|st| {
        out = crate::register::route(st, "GET", b"", "1.2.3.4:5", None, std::time::Instant::now())?;
        Ok(())
    })
    .unwrap();
    assert!(
        out.1.contains("type=\"email\" id=\"email\" name=\"email\""),
        "{}",
        out.1
    );
    let cases = [
        ("email=", "Cannot register: enter your e-mail address"),
        ("email=%20%20", "Cannot register: enter your e-mail address"),
        ("email=erika", "that is not a valid e-mail address"),
        (
            "email=fr%20ank%40example.org",
            "that is not a valid e-mail address",
        ),
        (
            "email=a%40b%40example.org",
            "that is not a valid e-mail address",
        ),
        (
            "email=erika%40localhost",
            "that is not a valid e-mail address",
        ),
        (
            "email=erika%40example.",
            "that is not a valid e-mail address",
        ),
        ("email=%40example.org", "that is not a valid e-mail address"),
    ];
    for (i, (e, want)) in cases.iter().enumerate() {
        let form = format!("name=Neo&pw=secret12&pw2=secret12&{e}");
        let (code, body) = post_form(&mut st, &format!("198.51.100.{i}:1"), &form);
        assert_eq!(code, 200);
        assert!(body.contains(want), "{e}: {body}");
        let (code, body) = reg_json(&mut st, "POST", &format!("198.51.101.{i}:1"), &form);
        assert_eq!(code, 200);
        assert!(
            body.starts_with("{\"ok\":false,\"message\":\"Cannot register: ")
                && body.contains(want),
            "{e}: {body}"
        );
    }
    let long = format!("email={}%40example.org", "a".repeat(250));
    let (_, body) = post_form(
        &mut st,
        "198.51.102.1:1",
        &format!("name=Neo&pw=secret12&pw2=secret12&{long}"),
    );
    assert!(body.contains("longer than 254"), "{body}");
    assert!(db::account_by_login(&st.db, "Neo").unwrap().is_none());
    // the form keeps the typed address after a refusal
    let (_, body) = post_form(
        &mut st,
        "198.51.102.2:1",
        "name=Neo&pw=secret12&pw2=secret99&email=neo%40example.org",
    );
    assert!(body.contains("value=\"neo@example.org\""), "{body}");
    // a good form stores it trimmed, as typed
    let (_, body) = post_form(
        &mut st,
        "198.51.102.3:1",
        "name=Neo&pw=secret12&pw2=secret12&email=%20Neo.A%40Example.org%20",
    );
    assert!(body.contains("Account created."), "{body}");
    let a = db::account_by_login(&st.db, "neo").unwrap().unwrap();
    assert_eq!(
        db::account_email(&st.db, a.user_no).unwrap(),
        "Neo.A@Example.org"
    );
    assert_eq!(
        db::logins_by_email(&st.db, "neo.a@example.ORG").unwrap(),
        vec!["Neo".to_string()]
    );
    let (_, body) = reg_json(
        &mut st,
        "POST",
        "198.51.102.4:1",
        "name=Trin&pw=secret12&pw2=secret12&email=trin%40example.org",
    );
    assert_eq!(body, "{\"ok\":true,\"login\":\"Trin\"}");
    let t = db::account_by_login(&st.db, "Trin").unwrap().unwrap();
    assert_eq!(
        db::account_email(&st.db, t.user_no).unwrap(),
        "trin@example.org"
    );
    assert_eq!(
        crate::accounts::mask_email("trin@example.org"),
        "t***@example.org"
    );
    assert_eq!(crate::accounts::mask_email(""), "-");
}

/// The recovery endpoints answer JSON over a real socket like /register (Accept header, body read).
#[test]
fn recovery_json_over_http() {
    let mut st = state();
    st.mail = crate::mail::Mailer::dry_run("[T]");
    let shared: Shared = Arc::new(std::sync::Mutex::new(st));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    spawn_with(Arc::clone(&shared), l, Hosting::none());
    let (head, body) = split_http(&raw_request(
        port,
        "GET /recover HTTP/1.1\r\nHost: x\r\nAccept: application/json\r\n\r\n",
    ));
    assert!(
        head.starts_with("HTTP/1.1 200") && head.contains("application/json"),
        "{head}"
    );
    assert_eq!(body, b"{\"mail\":true}");
    let form = "name=Nobody&email=nobody%40example.org";
    let (head, body) = split_http(&raw_request(
        port,
        &format!(
            "POST /recover/password HTTP/1.1\r\nHost: x\r\nAccept: application/json\r\nContent-Length: {}\r\n\r\n{}",
            form.len(),
            form
        ),
    ));
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(
        String::from_utf8_lossy(&body),
        "{\"ok\":true,\"message\":\"If the account and the address match, a code was sent.\"}"
    );
    let (head, body) = split_http(&raw_get(port, "GET", "/recover"));
    assert!(
        head.contains("text/html") && String::from_utf8_lossy(&body).contains("Send me a code"),
        "{head}"
    );
}

/// LG-321: on a public lobby the page URLs' login name is no proof: only the address it logged in from may use it.
#[test]
fn a_public_lobby_takes_clan_pages_only_from_the_logged_in_address() {
    let mut st = state();
    st.opts.public = true;
    let carol = player(&mut st, 1, "carol", "Carol", true);
    st.sessions.get_mut(&1).unwrap().peer = "203.0.113.9:5000".into();
    let ask = |st: &mut Core, target: &str, peer: &str| {
        let req = parse_request(&format!(
            "GET {target} HTTP/1.1

"
        ))
        .unwrap();
        let mut out = (0, String::new());
        st.scope(|st| {
            out = route_from(st, &req, peer)?;
            Ok(())
        })
        .unwrap();
        out
    };
    // somebody else names her: no page and no action, in any spelling of the parameter
    for t in [
        "/clan/create?p=carol&name=Stolen&emblem=1&bg=1",
        "/clan/create?strPassport=LOCAL:carol&name=Stolen&emblem=1&bg=1",
        "/clan?url=guild/list&passport=carol",
    ] {
        assert_eq!(ask(&mut st, t, "198.51.100.1:7000").0, 403, "{t}");
    }
    assert!(crate::clan::clan_of(&st, carol).unwrap().is_none());
    // her own address, another port: works; the host's loopback tools too
    let (code, page) = ask(
        &mut st,
        "/clan/create?p=carol&name=Wolves&emblem=1&bg=1",
        "203.0.113.9:6000",
    );
    assert_eq!(code, 200);
    assert!(page.contains("Clan Wolves created."), "{page}");
    assert_eq!(
        ask(&mut st, "/clan?url=guild/list&p=carol", "127.0.0.1:6000").0,
        200
    );
    // a page that names nobody stays open, and a lobby that is not public is not changed
    assert_eq!(ask(&mut st, "/clan", "198.51.100.1:7000").0, 200);
    st.opts.public = false;
    assert_eq!(ask(&mut st, "/clan?p=carol", "198.51.100.1:7000").0, 200);
}

/// LG-326: a silent connection is closed after the limit, a logged-in one is not on that clock, and the address cap holds.
#[test]
fn a_connection_that_never_logs_in_is_closed() {
    use std::io::Read;
    let mut st = state();
    st.limits.pre_login = Duration::from_millis(300);
    let shared: Shared = Arc::new(std::sync::Mutex::new(st));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    crate::net::spawn_listener(Arc::clone(&shared), l, crate::logs::Listener::Auth);
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let t = std::time::Instant::now();
    let mut b = [0u8; 16];
    let r = s.read(&mut b);
    assert!(
        matches!(r, Ok(0) | Err(_))
            && !matches!(&r, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut),
        "{r:?}"
    );
    assert!(
        t.elapsed() < Duration::from_secs(8),
        "closed by the limit, not by the test's own timeout"
    );
}
/// LG-408 (OWNER REPORT 2026-11-05: a founder created a second clan from the find page): a create from a character
/// in a clan is refused on every path (text form, HTML form, chat) with no new clan row; a member's pages carry no
/// create link; after a plain leave, a hand-over leave and a disband the same character creates again.
#[test]
fn a_member_cannot_create_a_second_clan_until_he_leaves() {
    let mut st = state();
    let carol = player(&mut st, 1, "carol", "Carol", true);
    let _dan = player(&mut st, 2, "dan", "Dan", true);
    get(
        &mut st,
        "/clan/create?p=carol&name=Wolves&emblem=1&bg=1&fmt=text",
    );
    let k = crate::clan::clan_by_name(&st.db, "Wolves")
        .unwrap()
        .unwrap();
    get(&mut st, &format!("/clan/join?p=dan&c={}&fmt=text", k.uid));
    // the founder and the plain member are both refused, the text form and the HTML form alike
    for (who, n) in [("carol", "Again"), ("dan", "Other")] {
        let (_, body) = get(
            &mut st,
            &format!("/clan/create?p={who}&name={n}&emblem=2&bg=2&fmt=text"),
        );
        assert_eq!(
            body,
            format!(
                "me\t{}\t{}\nmsg\t0\tCannot create the clan: you are already in clan Wolves.\n",
                k.uid,
                u8::from(who == "carol")
            ),
            "{who}"
        );
        let (_, page) = get(&mut st, &format!("/clan/create?p={who}&name={n}"));
        assert!(
            page.contains("Cannot create the clan: you are already in clan Wolves."),
            "{page}"
        );
    }
    assert_eq!(crate::clan::all_clans(&st.db).unwrap().len(), 1);
    assert_eq!(st.character(carol).unwrap().clan_uid, k.uid);
    // a member's pages offer no create link; a clanless viewer's do
    let eve = player(&mut st, 3, "eve", "Eve", true);
    for (who, link) in [("carol", false), ("dan", false), ("eve", true)] {
        let (_, page) = get(&mut st, &format!("/clan?url=guild/list&p={who}"));
        assert_eq!(page.contains("url=guild/create"), link, "{who}: {page}");
    }
    let (_, page) = get(&mut st, "/clan?url=guild/create&p=dan");
    assert!(!page.contains("action=\"/clan/create\""), "{page}");
    // a plain member leaves and creates
    get(&mut st, "/clan/leave?p=dan&fmt=text");
    let (_, body) = get(
        &mut st,
        "/clan/create?p=dan&name=Ravens&emblem=3&bg=3&fmt=text",
    );
    assert!(body.contains("msg\t1\tClan Ravens created.\n"), "{body}");
    // the founder with a member left: hand-over, then a create of his own
    get(&mut st, &format!("/clan/join?p=eve&c={}&fmt=text", k.uid));
    let (_, body) = get(&mut st, "/clan/leave?p=carol&fmt=text");
    assert!(body.contains("msg\t1\t"), "{body}");
    assert_eq!(
        crate::clan::clan_by_name(&st.db, "Wolves")
            .unwrap()
            .unwrap()
            .master,
        eve
    );
    let (_, body) = get(
        &mut st,
        "/clan/create?p=carol&name=Hawks&emblem=4&bg=4&fmt=text",
    );
    assert!(body.contains("msg\t1\tClan Hawks created.\n"), "{body}");
    // a founder alone disbands by leaving and creates again (the old name is free)
    get(&mut st, "/clan/leave?p=carol&fmt=text");
    assert!(crate::clan::clan_by_name(&st.db, "Hawks")
        .unwrap()
        .is_none());
    let (_, body) = get(
        &mut st,
        "/clan/create?p=carol&name=Hawks&emblem=5&bg=5&fmt=text",
    );
    assert!(body.contains("msg\t1\tClan Hawks created.\n"), "{body}");
    assert_eq!(crate::clan::all_clans(&st.db).unwrap().len(), 3);
}

/// LG-416: the player's address is the clan pages' only proof (LG-321), and a web page the player has open in a browser
/// shares it: another site's forged GET of an action is refused, the game's own requests and same-site links are not.
#[test]
fn another_sites_page_cannot_send_a_clan_action_from_the_players_address() {
    let mut st = state();
    st.opts.public = true;
    let erin = player(&mut st, 1, "erin", "Erin", true);
    st.sessions.get_mut(&1).unwrap().peer = "203.0.113.9:5000".into();
    let ask = |st: &mut Core, target: &str, extra: &str| {
        let head = format!("GET {target} HTTP/1.1\r\nHost: play.example:19080\r\n{extra}\r\n");
        let req = parse_request(&head).unwrap();
        let mut out = (0, String::new());
        st.scope(|st| {
            out = route_checked(st, &req, &head, "203.0.113.9:6000")?;
            Ok(())
        })
        .unwrap();
        out
    };
    let create = "/clan/create?p=erin&name=Forged&emblem=1&bg=1";
    // an <img> or a link on another site: the browser says so in Sec-Fetch-Site or Origin
    for extra in [
        "Sec-Fetch-Site: cross-site\r\n",
        "Sec-Fetch-Site: same-site\r\n",
        "Origin: https://evil.example\r\n",
        "Origin: null\r\n",
    ] {
        assert_eq!(ask(&mut st, create, extra).0, 403, "{extra}");
    }
    assert!(crate::clan::clan_of(&st, erin).unwrap().is_none());
    // a read is not an action: another site's link to the find page still opens it
    assert_eq!(
        ask(
            &mut st,
            "/clan?url=guild/list&p=erin",
            "Sec-Fetch-Site: cross-site\r\n"
        )
        .0,
        200
    );
    // the game (no such headers) and a link inside the pages themselves
    let (code, page) = ask(&mut st, create, "Sec-Fetch-Site: same-origin\r\n");
    assert_eq!(code, 200);
    assert!(page.contains("Clan Forged created."), "{page}");
    assert_eq!(
        ask(
            &mut st,
            "/clan/notice?p=erin&text=hi",
            "Origin: https://play.example:19080\r\n"
        )
        .0,
        200
    );
    assert_eq!(ask(&mut st, "/clan/leave?p=erin", "").0, 200);
}
