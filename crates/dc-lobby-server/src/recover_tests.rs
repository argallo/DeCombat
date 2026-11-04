//! Password and login-ID recovery (LG-240..LG-245): the dry-run mail, the code rules, no enumeration, the hourly
//! cap, the not-set-up answer, and a login over the auth port with the new password.

use super::*;
use crate::db;
use crate::logs::Listener;
use crate::mail::Mailer;
use crate::msg::{LOGIN_REPLY, LOGIN_REQUEST};
use crate::state::testing::*;
use crate::state::Shared;
use dc_wire::PktWriter;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const T0: i64 = 1_800_000_000;
const OLD_PW: &str = "old-horse-41";
const NEW_PW: &str = "battery-staple-7";

/// A lobby with the dry-run mailer and the account Robin (address Robin.Doe@Example.org).
fn lobby() -> Core {
    let mut st = state();
    st.mail = Mailer::dry_run("[T]");
    st.opts.pw_check = true;
    st.opts.autochar = true;
    st.scope(|st| {
        let no = crate::auth::ensure_account(st, "Erika", OLD_PW)?;
        db::set_email(&st.db, no, "Erika.Doe@Example.org")?;
        Ok(())
    })
    .unwrap();
    st
}

fn call(st: &mut Core, method: &str, path: &str, form: &str, peer: &str, unix: i64) -> (u16, String) {
    call_as(st, method, path, form, peer, unix, true)
}

fn call_as(
    st: &mut Core,
    method: &str,
    path: &str,
    form: &str,
    peer: &str,
    unix: i64,
    json: bool,
) -> (u16, String) {
    let mut out = (0, String::new());
    st.scope(|st| {
        out = route_at(st, method, path, form.as_bytes(), peer, Instant::now(), unix, json)?;
        Ok(())
    })
    .unwrap();
    out
}

fn ok(m: &str) -> String {
    format!("{{\"ok\":true,\"message\":\"{m}\"}}")
}

fn refused(m: &str) -> String {
    format!("{{\"ok\":false,\"message\":\"{m}\"}}")
}

/// The code of the last dry-run mail.
fn last_code(st: &Core) -> String {
    let m = st.mail.dry.last().expect("a mail");
    let at = m.body.find("Your code: ").expect("code line") + "Your code: ".len();
    m.body[at..at + CODE_LEN].to_string()
}

fn peer(i: u32) -> String {
    format!("198.51.100.{i}:4000")
}

fn request(st: &mut Core, i: u32, unix: i64) -> (u16, String) {
    call(
        st,
        "POST",
        "/recover/password",
        "name=erika&email=erika.doe%40example.ORG",
        &peer(i),
        unix,
    )
}

fn confirm(st: &mut Core, i: u32, code: &str, pw: &str, unix: i64) -> (u16, String) {
    call(
        st,
        "POST",
        "/recover/password/confirm",
        &format!("name=Erika&code={code}&pw={pw}&pw2={pw}"),
        &peer(i),
        unix,
    )
}

fn pw_is(st: &Core, pw: &str) -> bool {
    let a = db::account_by_login(&st.db, "erika").unwrap().unwrap();
    crate::auth::pw_verify(&a, pw) == crate::auth::PwCheck::Ok
}

#[test]
fn without_a_mail_server_recovery_says_it_is_not_set_up() {
    let mut st = state();
    assert_eq!(call(&mut st, "GET", "/recover", "", "1.2.3.4:5", T0), (200, "{\"mail\":false}".into()));
    let (code, body) = call(&mut st, "POST", "/recover/password", "name=a&email=a%40b.cd", "1.2.3.4:5", T0);
    assert_eq!((code, body), (200, refused(MSG_NOT_SET_UP)));
    let (_, body) = call(&mut st, "POST", "/recover/password/confirm", "name=a&code=x", "1.2.3.4:5", T0);
    assert_eq!(body, refused(MSG_NOT_SET_UP));
    let (_, body) = call(&mut st, "POST", "/recover/id", "email=a%40b.cd", "1.2.3.4:5", T0);
    assert_eq!(body, refused(MSG_ID_NOT_SET_UP));
    let (code, page) = call_as(&mut st, "GET", "/recover", "", "1.2.3.4:5", T0, false);
    assert_eq!(code, 200);
    assert!(page.contains(MSG_NOT_SET_UP) && !page.contains("<form"), "{page}");
    // the registration page offers the recovery link only when mail is set up
    st.opts.register = true;
    let mut out = (0, String::new());
    st.scope(|st| {
        out = crate::register::route(st, "GET", b"", "1.2.3.4:5", None, Instant::now())?;
        Ok(())
    })
    .unwrap();
    assert!(!out.1.contains("href=\"/recover\""));
    st.mail = Mailer::dry_run("");
    assert_eq!(call(&mut st, "GET", "/recover", "", "1.2.3.4:5", T0).1, "{\"mail\":true}");
    st.scope(|st| {
        out = crate::register::route(st, "GET", b"", "1.2.3.4:5", None, Instant::now())?;
        Ok(())
    })
    .unwrap();
    assert!(out.1.contains("href=\"/recover\""));
}

#[test]
fn a_mailed_code_sets_the_new_password_and_the_auth_port_takes_it() {
    let mut st = lobby();
    let (code, body) = request(&mut st, 1, T0);
    assert_eq!((code, body), (200, ok(MSG_SENT)));
    assert_eq!(st.mail.dry.len(), 1);
    let m = st.mail.dry[0].clone();
    assert_eq!(m.to, "Erika.Doe@Example.org", "sent to the stored address");
    assert_eq!(m.subject, "[T] password reset code");
    assert!(m.body.contains("login Erika") && m.body.contains("ignore this mail"), "{}", m.body);
    let c = last_code(&st);
    assert!(c.bytes().all(|b| crate::accounts::CODE_ALPHABET.contains(&b)), "{c}");
    // only the hash is stored
    let stored: String = st
        .db
        .query_row("SELECT code_hash FROM recovery_codes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored.len(), 64);
    assert!(!stored.contains(&c) && !stored.to_uppercase().contains(&c));
    // the password rules come first and do not use up the code
    let (_, body) = confirm(&mut st, 2, &c, "short", T0 + 10);
    assert!(body.starts_with("{\"ok\":false,\"message\":\"Cannot change the password: ") && body.contains("8 to 16"), "{body}");
    let (_, body) = call(
        &mut st,
        "POST",
        "/recover/password/confirm",
        &format!("name=Erika&code={c}&pw={NEW_PW}&pw2=other-pass-99"),
        &peer(2),
        T0 + 10,
    );
    assert_eq!(body, refused("Cannot change the password: the two passwords differ."));
    // the right code in lower case with a dash
    let typed = format!("{}-{}", c[..4].to_lowercase(), &c[4..]);
    assert_eq!(confirm(&mut st, 3, &typed, NEW_PW, T0 + 20), (200, ok(MSG_CHANGED)));
    assert!(pw_is(&st, NEW_PW) && !pw_is(&st, OLD_PW));
    // single use
    assert_eq!(confirm(&mut st, 4, &c, "another-pass-5", T0 + 30).1, refused(MSG_BAD_CODE));
    assert!(pw_is(&st, NEW_PW));
    // the auth port: the new password logs in, the old one does not
    let shared: Shared = Arc::new(Mutex::new(st));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    crate::net::spawn_listener(Arc::clone(&shared), l, Listener::Auth);
    assert_eq!(login(port, "Erika", NEW_PW), 1, "the new password logs in");
    assert_eq!(login(port, "Erika", OLD_PW), 0, "the old one is refused");
}

fn login(port: u16, name: &str, pw: &str) -> u32 {
    let mut w = PktWriter::new(LOGIN_REQUEST);
    w.wstr(&format!("LOCAL:{name}"), 4096)
        .wstr(pw, 17)
        .wstr("00-11", 20)
        .i32(0);
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(&w.to_frame()).unwrap();
    while let Ok(Some((id, p))) = dc_wire::read_tcp_frame(&mut s) {
        if id as u16 == LOGIN_REPLY {
            return u32::from_le_bytes(p[6..10].try_into().unwrap());
        }
    }
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);
    panic!("no login reply");
}

#[test]
fn unknown_accounts_and_other_addresses_get_the_same_answer_and_no_mail() {
    let mut st = lobby();
    for form in [
        "name=Nobody&email=erika.doe%40example.org",
        "name=Erika&email=someone%40example.org",
    ] {
        let (code, body) = call(&mut st, "POST", "/recover/password", form, &peer(1), T0);
        assert_eq!((code, body), (200, ok(MSG_SENT)), "{form}");
    }
    assert!(st.mail.dry.is_empty());
    // a code entered for an unknown name is the plain wrong-code answer
    let (_, body) = call(
        &mut st,
        "POST",
        "/recover/password/confirm",
        &format!("name=Nobody&code=ABCDEFGH&pw={NEW_PW}&pw2={NEW_PW}"),
        &peer(2),
        T0,
    );
    assert_eq!(body, refused(MSG_BAD_CODE));
    // malformed input is refused without looking anything up
    let (_, body) = call(&mut st, "POST", "/recover/password", "name=Erika&email=erika", &peer(3), T0);
    assert_eq!(body, refused("Cannot send: that is not a valid e-mail address."));
    let (_, body) = call(&mut st, "POST", "/recover/password", "name=&email=a%40b.cd", &peer(3), T0);
    assert_eq!(body, refused("Enter your login name."));
}

#[test]
fn five_wrong_codes_burn_it_and_an_old_code_expires() {
    let mut st = lobby();
    request(&mut st, 1, T0);
    let c = last_code(&st);
    let wrong = if c.starts_with('A') { "BBBBBBBB" } else { "AAAAAAAA" };
    for i in 0..CODE_TRIES as u32 {
        assert_eq!(confirm(&mut st, 10 + i, wrong, NEW_PW, T0 + 1).1, refused(MSG_BAD_CODE));
    }
    assert_eq!(confirm(&mut st, 20, &c, NEW_PW, T0 + 2).1, refused(MSG_BAD_CODE), "burnt");
    assert!(pw_is(&st, OLD_PW));
    // expiry: 15 minutes
    request(&mut st, 2, T0 + 100);
    let c = last_code(&st);
    assert_eq!(
        confirm(&mut st, 21, &c, NEW_PW, T0 + 100 + CODE_TTL_SECS).1,
        refused(MSG_BAD_CODE),
        "expired"
    );
    request(&mut st, 3, T0 + 2000);
    let c = last_code(&st);
    assert_eq!(confirm(&mut st, 22, &c, NEW_PW, T0 + 2000 + CODE_TTL_SECS - 1).1, ok(MSG_CHANGED));
}

#[test]
fn three_codes_an_hour_and_a_new_code_replaces_the_old() {
    let mut st = lobby();
    for i in 0..3 {
        assert_eq!(request(&mut st, i, T0 + i as i64).1, ok(MSG_SENT));
    }
    assert_eq!(st.mail.dry.len(), 3);
    let first = st.mail.dry[0].body.clone();
    let newest = last_code(&st);
    // the fourth within the hour: the same answer, no mail
    assert_eq!(request(&mut st, 4, T0 + 3000).1, ok(MSG_SENT));
    assert_eq!(st.mail.dry.len(), 3);
    // only the newest code is open
    let at = first.find("Your code: ").unwrap() + 11;
    let old = first[at..at + CODE_LEN].to_string();
    if old != newest {
        assert_eq!(confirm(&mut st, 30, &old, NEW_PW, T0 + 10).1, refused(MSG_BAD_CODE));
    }
    // an hour after the first, a code is sent again
    assert_eq!(request(&mut st, 5, T0 + 3601).1, ok(MSG_SENT));
    assert_eq!(st.mail.dry.len(), 4);
}

#[test]
fn login_ids_are_mailed_to_the_address() {
    let mut st = lobby();
    st.scope(|st| {
        let no = db::create_account(&st.db, "Erikaie", 0)?;
        db::set_email(&st.db, no, "ERIKA.DOE@example.org")?;
        let other = db::create_account(&st.db, "Kim", 0)?;
        db::set_email(&st.db, other, "kim@example.org")?;
        Ok(())
    })
    .unwrap();
    let (code, body) = call(&mut st, "POST", "/recover/id", "email=erika.doe%40example.org", &peer(1), T0);
    assert_eq!((code, body), (200, ok(MSG_ID_SENT)));
    assert_eq!(st.mail.dry.len(), 1);
    let m = &st.mail.dry[0];
    assert_eq!(m.subject, "[T] your login IDs");
    assert!(m.body.contains("  Erika\n") && m.body.contains("  Erikaie\n") && !m.body.contains("Kim"), "{}", m.body);
    assert_eq!(m.to, "erika.doe@example.org");
    // an unused address: the same answer, no mail
    let (_, body) = call(&mut st, "POST", "/recover/id", "email=nobody%40example.org", &peer(2), T0);
    assert_eq!(body, ok(MSG_ID_SENT));
    assert_eq!(st.mail.dry.len(), 1);
    let (_, body) = call(&mut st, "POST", "/recover/id", "email=", &peer(3), T0);
    assert!(body.starts_with("{\"ok\":false,\"message\":\"Cannot send: enter your e-mail address"), "{body}");
}

#[test]
fn the_address_throttle_and_the_html_pages() {
    let mut st = lobby();
    for i in 0..crate::limits::REGISTER_ATTEMPTS {
        let (code, _) = call(&mut st, "POST", "/recover/id", "email=x%40y.zz", "203.0.113.9:1", T0 + i as i64);
        assert_eq!(code, 200);
    }
    let (code, body) = call(&mut st, "POST", "/recover/password", "name=Erika&email=erika.doe%40example.org", "203.0.113.9:2", T0);
    assert_eq!((code, body), (429, refused(MSG_SLOW)));
    assert!(st.mail.dry.is_empty());
    // HTML: the form pages and the answers in the page frame, no script
    let (code, page) = call_as(&mut st, "GET", "/recover", "", &peer(1), T0, false);
    assert_eq!(code, 200);
    assert!(
        page.contains("action=\"/recover/password\"") && page.contains("action=\"/recover/id\"") && !page.contains("<script"),
        "{page}"
    );
    let (_, page) = call_as(&mut st, "GET", "/recover/password/confirm", "", &peer(1), T0, false);
    assert!(page.contains("action=\"/recover/password/confirm\"") && page.contains("name=\"pw2\""));
    let (_, page) = call_as(
        &mut st,
        "POST",
        "/recover/password",
        "name=Erika&email=erika.doe%40example.org",
        &peer(1),
        T0,
        false,
    );
    assert!(page.contains(MSG_SENT) && page.contains("action=\"/recover/password/confirm\""), "{page}");
    assert!(!page.contains(&last_code(&st)), "the code is only in the mail");
    let (code, _) = call_as(&mut st, "PUT", "/recover/id", "", &peer(1), T0, false);
    assert_eq!(code, 405);
    assert!(handles("/recover/") && handles("/recover/password/confirm") && !handles("/recovery"));
}
