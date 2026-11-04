//! Tests of the beta list over HTTP: the public endpoints (CORS, preflight, no enumeration, double opt-in, unsubscribe,
//! limits, the Cloudflare header) and the operator page (loopback, CSRF, the budget).

use super::*;
use crate::beta::tests::{beta_state, token_in};
use crate::clanweb::parse_request;
use rusqlite::params;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};

const NOW: i64 = 1_793_620_800;
const SITE: &str = "https://decombat.com";
const PEER: &str = "198.51.100.7:4444";

fn shared() -> Shared {
    Arc::new(Mutex::new(beta_state()))
}

/// (status, head, body) of a raw response.
fn split(raw: Vec<u8>) -> (u16, String, String) {
    let text = String::from_utf8(raw).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, head.to_string(), body.to_string())
}

fn public(
    sh: &Shared,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &str,
    peer: &str,
    unix: i64,
) -> (u16, String, String) {
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: beta.example.com\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let req = parse_request(&head).unwrap();
    split(respond_at(sh, &req, &head, body.as_bytes(), peer, unix))
}

fn post_json(
    sh: &Shared,
    origin: Option<&str>,
    body: &str,
    peer: &str,
    unix: i64,
    extra: &[(&str, &str)],
) -> (u16, String, String) {
    let mut h = vec![("Content-Type", "application/json")];
    if let Some(o) = origin {
        h.push(("Origin", o));
    }
    h.extend_from_slice(extra);
    public(sh, "POST", "/beta/signup", &h, body, peer, unix)
}

fn rows(sh: &Shared) -> i64 {
    crate::net::lock(sh)
        .db
        .query_row("SELECT COUNT(*) FROM beta_signups", [], |r| r.get(0))
        .unwrap()
}

fn mails(sh: &Shared) -> Vec<crate::mail::Mail> {
    crate::net::lock(sh).mail.dry.clone()
}

fn signup_body(email: &str) -> String {
    json!({"email": email, "platform": "windows", "consent": true}).to_string()
}

#[test]
fn the_endpoints_are_404_while_the_beta_is_off() {
    let sh = shared();
    crate::net::lock(&sh).opts.beta.enabled = false;
    for (m, p) in [
        ("POST", "/beta/signup"),
        ("GET", "/beta/confirm?t=x"),
        ("GET", "/beta/unsubscribe?t=x"),
        ("OPTIONS", "/beta/signup"),
    ] {
        assert_eq!(
            public(&sh, m, p, &[("Origin", SITE)], "", PEER, NOW).0,
            404,
            "{m} {p}"
        );
    }
    assert_eq!(rows(&sh), 0);
}

#[test]
fn cors_answers_the_preflight_and_the_post_for_the_site_only() {
    let sh = shared();
    // the preflight
    let (st, head, _) = public(
        &sh,
        "OPTIONS",
        "/beta/signup",
        &[("Origin", SITE), ("Access-Control-Request-Method", "POST")],
        "",
        PEER,
        NOW,
    );
    assert_eq!(st, 204);
    assert!(
        head.contains(&format!("Access-Control-Allow-Origin: {SITE}\r\n")),
        "{head}"
    );
    assert!(
        head.contains("Access-Control-Allow-Methods: POST, OPTIONS")
            && head.contains("Access-Control-Allow-Headers: Content-Type")
    );
    assert!(head.contains("Vary: Origin") && !head.contains("Access-Control-Allow-Origin: *"));
    // another origin (or none) gets no CORS header and no permission
    for o in [
        Some("https://evil.example"),
        Some("null"),
        Some("http://decombat.com"),
        None,
    ] {
        let h: Vec<(&str, &str)> = o.map(|o| vec![("Origin", o)]).unwrap_or_default();
        let (st, head, _) = public(&sh, "OPTIONS", "/beta/signup", &h, "", PEER, NOW);
        assert_eq!(st, 403, "{o:?}");
        assert!(!head.contains("Access-Control-Allow-Origin"), "{head}");
    }
    // a POST from another site is refused and stores nothing
    let (st, head, body) = post_json(
        &sh,
        Some("https://evil.example"),
        &signup_body("a@example.com"),
        PEER,
        NOW,
        &[],
    );
    assert_eq!(st, 403, "{body}");
    assert!(!head.contains("Access-Control-Allow-Origin"));
    assert_eq!(rows(&sh), 0);
    // a POST from the site works and carries the CORS header on the answer
    let (st, head, body) = post_json(
        &sh,
        Some(SITE),
        &signup_body("a@example.com"),
        PEER,
        NOW,
        &[],
    );
    assert_eq!(st, 200, "{body}");
    assert!(
        head.contains(&format!("Access-Control-Allow-Origin: {SITE}"))
            && head.contains("application/json")
    );
    assert_eq!(rows(&sh), 1);
    // a tool without an Origin header is allowed (the limits are its fence); other methods are not
    assert_eq!(
        post_json(&sh, None, &signup_body("b@example.com"), PEER, NOW + 1, &[]).0,
        200
    );
    assert_eq!(
        public(&sh, "GET", "/beta/signup", &[], "", PEER, NOW).0,
        405
    );
}

#[test]
fn every_address_gets_the_same_answer_whatever_its_state() {
    let sh = shared();
    let answer = |email: &str, ip: &str, unix: i64| {
        let (st, _, body) = post_json(
            &sh,
            Some(SITE),
            &signup_body(email),
            &format!("{ip}:1"),
            unix,
            &[],
        );
        (st, body)
    };
    let new = answer("a@example.com", "203.0.113.1", NOW);
    assert_eq!(new.0, 200);
    assert_eq!(new.1, json!({"ok": true, "message": MSG_SENT}).to_string());
    // pending, confirmed, invited and unsubscribed addresses: byte-identical
    assert_eq!(answer("a@example.com", "203.0.113.2", NOW + 1), new);
    let (conf, unsub) = {
        let g = crate::net::lock(&sh);
        let r: (String, String) =
            g.db.query_row(
                "SELECT confirm_token, unsub_token FROM beta_signups",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        r
    };
    assert_eq!(
        public(
            &sh,
            "GET",
            &format!("/beta/confirm?t={conf}"),
            &[],
            "",
            PEER,
            NOW + 2
        )
        .0,
        200
    );
    assert_eq!(answer("A@example.com", "203.0.113.3", NOW + 3), new);
    crate::net::lock(&sh)
        .db
        .execute("UPDATE beta_signups SET status = 'invited'", [])
        .unwrap();
    assert_eq!(answer("a@example.com", "203.0.113.4", NOW + 4), new);
    assert_eq!(
        public(
            &sh,
            "GET",
            &format!("/beta/unsubscribe?t={unsub}"),
            &[],
            "",
            PEER,
            NOW + 5
        )
        .0,
        200
    );
    assert_eq!(answer("a@example.com", "203.0.113.5", NOW + 6), new);
    // and none of those repeats mailed the address more than the first confirmation (+ the unsubscribed re-signup's)
    assert_eq!(mails(&sh).len(), 1);
    // bad input is told apart from success, but not by anything about other addresses
    assert_eq!(answer("nonsense", "203.0.113.6", NOW + 7).0, 400);
}

#[test]
fn json_and_form_posts_and_their_refusals() {
    let sh = shared();
    // a form post answers with a page
    let (st, head, body) = public(
        &sh,
        "POST",
        "/beta/signup",
        &[
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Origin", SITE),
        ],
        "email=form%40example.com&platform=mac&consent=on",
        PEER,
        NOW,
    );
    assert_eq!(st, 200, "{body}");
    assert!(
        head.contains("text/html")
            && body.contains("confirmation link")
            && !body.contains("<script")
    );
    assert_eq!(
        crate::net::lock(&sh)
            .db
            .query_row("SELECT platform FROM beta_signups", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "mac"
    );
    // no consent, a bad address, a broken body
    let refused = |body: &str| post_json(&sh, Some(SITE), body, "203.0.113.50:1", NOW + 1, &[]);
    assert_eq!(
        refused(&json!({"email": "c@example.com", "consent": false}).to_string()).0,
        400
    );
    assert_eq!(
        refused(&json!({"email": "c@example.com"}).to_string()).0,
        400
    );
    assert_eq!(
        refused(&json!({"email": "bad", "consent": true}).to_string()).0,
        400
    );
    assert_eq!(refused("[1,2]").0, 400);
    assert_eq!(refused("{").0, 400);
    assert_eq!(
        refused(&json!({"email": "c@example.com", "consent": "true"}).to_string()).0,
        200
    );
    assert_eq!(rows(&sh), 2);
    // the mail sender being off is a 503, not a success
    crate::net::lock(&sh).mail = crate::mail::Mailer::off(Arc::new(crate::logs::Logger::none()));
    assert_eq!(
        post_json(
            &sh,
            Some(SITE),
            &signup_body("d@example.com"),
            "203.0.113.51:1",
            NOW + 2,
            &[]
        )
        .0,
        503
    );
    assert_eq!(rows(&sh), 2);
}

#[test]
fn the_links_in_the_mails_confirm_and_unsubscribe() {
    let sh = shared();
    assert_eq!(
        post_json(
            &sh,
            Some(SITE),
            &signup_body("e@example.com"),
            PEER,
            NOW,
            &[]
        )
        .0,
        200
    );
    let m = mails(&sh).remove(0);
    let conf = token_in(&m.body);
    let unsub = m.headers[0]
        .1
        .trim_matches(|c| c == '<' || c == '>')
        .rsplit("t=")
        .next()
        .unwrap()
        .to_string();
    let status = |sh: &Shared| {
        crate::net::lock(sh)
            .db
            .query_row("SELECT status FROM beta_signups", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap()
    };
    // a HEAD (a mail scanner's look) changes nothing; a wrong token says so
    let (st, _, _) = public(
        &sh,
        "HEAD",
        &format!("/beta/confirm?t={conf}"),
        &[],
        "",
        PEER,
        NOW + 1,
    );
    assert_eq!((st, status(&sh).as_str()), (200, "pending"));
    let (st, _, body) = public(
        &sh,
        "GET",
        &format!("/beta/confirm?t={}", "0".repeat(32)),
        &[],
        "",
        PEER,
        NOW + 1,
    );
    assert!(st == 200 && body.contains("Link not valid"), "{body}");
    // the link confirms; the page does not leak the token into a Referer or a search index
    let (st, head, body) = public(
        &sh,
        "GET",
        &format!("/beta/confirm?t={conf}"),
        &[],
        "",
        PEER,
        NOW + 2,
    );
    assert!(st == 200 && body.contains("You are on the list"), "{body}");
    assert!(
        head.contains("Referrer-Policy: no-referrer") && head.contains("X-Robots-Tag: noindex")
    );
    assert_eq!(status(&sh), "confirmed");
    assert_eq!(
        public(
            &sh,
            "POST",
            &format!("/beta/confirm?t={conf}"),
            &[],
            "",
            PEER,
            NOW + 3
        )
        .0,
        405
    );
    // one click: GET from the page, POST from a mail client's button
    let (st, _, body) = public(
        &sh,
        "POST",
        &format!("/beta/unsubscribe?t={unsub}"),
        &[("Content-Type", "application/x-www-form-urlencoded")],
        "List-Unsubscribe=One-Click",
        PEER,
        NOW + 4,
    );
    assert_eq!((st, body.trim()), (200, "unsubscribed"));
    assert_eq!(status(&sh), "unsubscribed");
    let (st, _, body) = public(
        &sh,
        "GET",
        &format!("/beta/unsubscribe?t={unsub}"),
        &[],
        "",
        PEER,
        NOW + 5,
    );
    assert!(st == 200 && body.contains("Unsubscribed"), "{body}");
    let (st, _, body) = public(
        &sh,
        "GET",
        "/beta/unsubscribe?t=nope",
        &[],
        "",
        PEER,
        NOW + 5,
    );
    assert!(st == 200 && body.contains("not valid"));
    assert_eq!(
        public(
            &sh,
            "POST",
            "/beta/unsubscribe?t=nope",
            &[],
            "",
            PEER,
            NOW + 5
        )
        .0,
        404
    );
    // a page never echoes anything the visitor sent
    let (_, _, body) = public(
        &sh,
        "GET",
        "/beta/confirm?t=%3Cscript%3Ealert(1)%3C/script%3E",
        &[],
        "",
        PEER,
        NOW + 6,
    );
    assert!(!body.contains("<script>alert"));
}

#[test]
fn signups_are_limited_per_client_address_and_the_cloudflare_header_only_counts_when_trusted() {
    // not trusted: the header is ignored, every request counts against the TCP peer
    let sh = shared();
    {
        let mut g = crate::net::lock(&sh);
        g.opts.beta.ip_per_hour = 2;
        g.opts.beta.trust_cf_header = false;
    }
    let try_cf = |n: u32, cf: &str, peer: &str| {
        post_json(
            &sh,
            Some(SITE),
            &signup_body(&format!("u{n}@example.com")),
            peer,
            NOW + i64::from(n),
            &[("CF-Connecting-IP", cf)],
        )
        .0
    };
    assert_eq!(try_cf(1, "192.0.2.1", "104.16.0.1:1"), 200);
    assert_eq!(try_cf(2, "192.0.2.2", "104.16.0.1:2"), 200);
    assert_eq!(
        try_cf(3, "192.0.2.3", "104.16.0.1:3"),
        429,
        "all three came from the one peer"
    );
    // trusted: the header names the client, so distinct clients behind one Cloudflare address are not lumped together
    let sh = shared();
    {
        let mut g = crate::net::lock(&sh);
        g.opts.beta.ip_per_hour = 2;
        g.opts.beta.trust_cf_header = true;
    }
    let try_cf = |n: u32, cf: &str, peer: &str| {
        post_json(
            &sh,
            Some(SITE),
            &signup_body(&format!("u{n}@example.com")),
            peer,
            NOW + i64::from(n),
            &[("CF-Connecting-IP", cf)],
        )
        .0
    };
    assert_eq!(try_cf(1, "192.0.2.1", "104.16.0.1:1"), 200);
    assert_eq!(try_cf(2, "192.0.2.2", "104.16.0.1:2"), 200);
    assert_eq!(try_cf(3, "192.0.2.3", "104.16.0.1:3"), 200);
    // ... and one client behind it is still limited
    assert_eq!(try_cf(4, "192.0.2.1", "104.16.0.1:4"), 200);
    assert_eq!(try_cf(5, "192.0.2.1", "104.16.0.1:5"), 429);
    // a header that is no address falls back to the peer
    assert_eq!(try_cf(6, "not-an-ip", "104.16.0.9:1"), 200);
    assert_eq!(try_cf(7, "not-an-ip", "104.16.0.9:2"), 200);
    assert_eq!(try_cf(8, "not-an-ip", "104.16.0.9:3"), 429);
    // the 429 says when to come back
    let (st, head, _) = post_json(
        &sh,
        Some(SITE),
        &signup_body("z@example.com"),
        "104.16.0.9:9",
        NOW + 20,
        &[("CF-Connecting-IP", "junk")],
    );
    assert!(st == 429 && head.contains("Retry-After: 3600"), "{head}");
}

#[test]
fn client_addresses_are_grouped_for_the_limits() {
    assert!(
        is_cloudflare("104.16.0.1")
            && is_cloudflare("172.71.255.255")
            && is_cloudflare("::ffff:162.159.1.1")
    );
    assert!(
        !is_cloudflare("104.15.255.255")
            && !is_cloudflare("10.0.0.1")
            && !is_cloudflare("2001:db8::1")
    );
    assert!(is_cloudflare("2a06:98c7::1") && !is_cloudflare("2a06:98c8::1"));
    let h = |ip: &str| format!("POST / HTTP/1.1\r\nCF-Connecting-IP: {ip}\r\n\r\n");
    assert_eq!(
        client_ip(&h("2001:db8:1:2:3:4:5:6"), "104.16.0.1:1", true),
        "2001:db8:1:2::/64"
    );
    assert_eq!(
        client_ip(&h("2001:db8:1:2:ffff::1"), "104.16.0.1:1", true),
        "2001:db8:1:2::/64"
    );
    assert_eq!(
        client_ip(&h("::ffff:192.0.2.5"), "104.16.0.1:1", true),
        "192.0.2.5"
    );
    assert_eq!(client_ip(&h("192.0.2.5"), "10.0.0.1:1", false), "10.0.0.1");
    // trusted, but the peer is not Cloudflare: anyone can reach the port directly and write the header
    assert_eq!(client_ip(&h("192.0.2.5"), "10.0.0.1:1", true), "10.0.0.1");
    assert_eq!(
        client_ip(&h("192.0.2.5"), "[2606:4700::1]:1", true),
        "192.0.2.5"
    );
    assert_eq!(
        client_ip("POST / HTTP/1.1\r\n\r\n", "10.0.0.1:1", true),
        "10.0.0.1"
    );
    assert_eq!(
        client_ip("POST / HTTP/1.1\r\n\r\n", "[2001:db8::7]:1", false),
        "2001:db8:0:0::/64"
    );
}

// ------------------------------------------------------------------------------------------------- operator page

const LO: Option<IpAddr> = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
const HOST: &str = "127.0.0.1:10090";

fn ops(
    sh: &Shared,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &str,
    peer: Option<IpAddr>,
    host: &str,
) -> (u16, String) {
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: {host}\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let (st, _, b) =
        crate::opsweb::handle_with_body(sh, &head, body.as_bytes(), peer, None, true).unwrap();
    (st, b)
}

fn csrf_of(sh: &Shared) -> String {
    let (st, page) = ops(sh, "GET", "/ops/beta", &[], "", LO, HOST);
    assert_eq!(st, 200);
    assert!(!page.contains("__CSRF__") && !page.contains("<script src") && !page.contains("<link"));
    let i = page.find("name=\"csrf\" content=\"").unwrap() + 21;
    page[i..i + 64].to_string()
}

fn write(sh: &Shared, path: &str, body: &Value, csrf: &str) -> (u16, Value) {
    let origin = format!("http://{HOST}");
    let (st, b) = ops(
        sh,
        "POST",
        path,
        &[
            ("Content-Type", "application/json"),
            ("Origin", &origin),
            ("X-Beta-CSRF", csrf),
        ],
        &body.to_string(),
        LO,
        HOST,
    );
    (st, serde_json::from_str(&b).unwrap())
}

fn with_confirmed(sh: &Shared, n: usize) {
    let g = crate::net::lock(sh);
    for i in 0..n {
        g.db.execute(
            "INSERT INTO beta_signups (email, platform, status, created, confirmed, confirm_token, unsub_token) \
             VALUES (?1, 'windows', 'confirmed', ?2, ?2, ?3, ?4)",
            params![format!("t{i}@example.com"), NOW + i as i64, format!("{i:032x}"), format!("{:032x}", i + 500)],
        )
        .unwrap();
    }
}

#[test]
fn the_operator_page_is_loopback_only_and_lists_the_signups() {
    let sh = shared();
    with_confirmed(&sh, 3);
    let remote = Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)));
    assert_eq!(ops(&sh, "GET", "/ops/beta", &[], "", remote, HOST).0, 403);
    assert_eq!(
        ops(&sh, "GET", "/ops/beta/list", &[], "", remote, HOST).0,
        403
    );
    assert_eq!(
        ops(
            &sh,
            "GET",
            "/ops/beta/list",
            &[],
            "",
            LO,
            "attacker.example"
        )
        .0,
        403,
        "a rebound name"
    );
    let (st, body) = ops(
        &sh,
        "GET",
        "/ops/beta/list?status=confirmed&platform=windows",
        &[],
        "",
        LO,
        HOST,
    );
    assert_eq!(st, 200, "{body}");
    let j: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(j["rows"].as_array().unwrap().len(), 3);
    assert_eq!(j["counts"]["confirmed"], 3);
    assert_eq!(j["budget"]["max"], 90);
    assert_eq!(j["budget"]["admin_remaining"], 80);
    assert_eq!(j["mail_on"], true);
    assert!(j["feedback_default"]["body"]
        .as_str()
        .unwrap()
        .contains("{name}"));
    let (_, body) = ops(
        &sh,
        "GET",
        "/ops/beta/list?after=2999-01-01",
        &[],
        "",
        LO,
        HOST,
    );
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["rows"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(ops(&sh, "GET", "/ops/beta/nope", &[], "", LO, HOST).0, 404);
    assert_eq!(
        ops(&sh, "DELETE", "/ops/beta/list", &[], "", LO, HOST).0,
        405
    );
    // the dashboard links to it
    assert!(include_str!("ops_dashboard.html").contains("/ops/beta"));
}

#[test]
fn a_write_from_a_browser_page_of_another_origin_is_refused() {
    let sh = shared();
    with_confirmed(&sh, 2);
    let csrf = csrf_of(&sh);
    assert_eq!(csrf, csrf_of(&sh), "one token per process");
    let body = json!({"ids": [1]}).to_string();
    let origin = format!("http://{HOST}");
    let attempt =
        |headers: &[(&str, &str)]| ops(&sh, "POST", "/ops/beta/invite", headers, &body, LO, HOST).0;
    // no token, a wrong token, no origin, another origin, a form content type (a cross-site form post)
    assert_eq!(
        attempt(&[("Content-Type", "application/json"), ("Origin", &origin)]),
        403
    );
    assert_eq!(
        attempt(&[
            ("Content-Type", "application/json"),
            ("Origin", &origin),
            ("X-Beta-CSRF", &"0".repeat(64))
        ]),
        403
    );
    assert_eq!(
        attempt(&[("Content-Type", "application/json"), ("X-Beta-CSRF", &csrf)]),
        403
    );
    assert_eq!(
        attempt(&[
            ("Content-Type", "application/json"),
            ("Origin", "http://evil.example"),
            ("X-Beta-CSRF", &csrf)
        ]),
        403
    );
    assert_eq!(
        attempt(&[
            ("Content-Type", "application/json"),
            ("Origin", "http://127.0.0.1:1"),
            ("X-Beta-CSRF", &csrf)
        ]),
        403
    );
    assert_eq!(
        attempt(&[
            ("Content-Type", "text/plain"),
            ("Origin", &origin),
            ("X-Beta-CSRF", &csrf)
        ]),
        415
    );
    assert_eq!(
        attempt(&[
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Origin", &origin),
            ("X-Beta-CSRF", &csrf)
        ]),
        415
    );
    assert!(mails(&sh).is_empty());
    assert_eq!(
        crate::net::lock(&sh)
            .db
            .query_row(
                "SELECT COUNT(*) FROM beta_signups WHERE status = 'invited'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    // the page's own request is taken
    let (st, j) = write(&sh, "/ops/beta/invite", &json!({"ids": [1]}), &csrf);
    assert_eq!(st, 200, "{j}");
    assert_eq!(
        (j["sent"].clone(), j["results"][0]["message"].clone()),
        (json!(1), json!("queued"))
    );
    assert_eq!(mails(&sh).len(), 1);
    // a write needs a body that names rows
    assert_eq!(write(&sh, "/ops/beta/invite", &json!({}), &csrf).0, 400);
}

#[test]
fn the_page_refuses_a_batch_over_the_mail_budget_and_shows_what_is_left() {
    let sh = shared();
    with_confirmed(&sh, 6);
    {
        let mut g = crate::net::lock(&sh);
        g.opts.beta.max_mail_per_day = 5;
        g.opts.beta.confirm_reserve = 1;
    }
    let csrf = csrf_of(&sh);
    let (st, j) = write(&sh, "/ops/beta/invite", &json!({"next": 5}), &csrf);
    assert_eq!(st, 409, "{j}");
    assert!(
        j["message"]
            .as_str()
            .unwrap()
            .contains("5 mails are over the budget"),
        "{j}"
    );
    assert!(mails(&sh).is_empty());
    // a preview of the same batch is refused the same way; 4 fit
    assert_eq!(
        write(
            &sh,
            "/ops/beta/invite",
            &json!({"next": 5, "dry_run": true}),
            &csrf
        )
        .0,
        409
    );
    let (st, j) = write(
        &sh,
        "/ops/beta/invite",
        &json!({"next": 4, "dry_run": true}),
        &csrf,
    );
    assert_eq!(
        (st, j["dry_run"].clone(), j["sent"].clone()),
        (200, json!(true), json!(4))
    );
    assert!(mails(&sh).is_empty());
    let (st, j) = write(&sh, "/ops/beta/invite", &json!({"next": 4}), &csrf);
    assert_eq!(st, 200);
    assert_eq!(
        j["budget"],
        json!({"max": 5, "used": 4, "remaining": 1, "reserve": 1, "admin_remaining": 0})
    );
    // the list shows the rest of the budget and each row's mail outcome
    let (_, body) = ops(&sh, "GET", "/ops/beta/list", &[], "", LO, HOST);
    let l: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(l["budget"]["admin_remaining"], 0);
    let invited: Vec<_> = l["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["status"] == "invited")
        .collect();
    assert_eq!(invited.len(), 4);
    assert!(
        invited
            .iter()
            .all(|r| r["mail"]["kind"] == "invite" && r["mail"]["state"] == 1),
        "the dry run answers at once"
    );
    // feedback: over the budget too, and a note is set through its own write
    let fb = json!({"ids": [1, 2], "subject": "How is it {name}?", "body": "Hi {name}"});
    assert_eq!(write(&sh, "/ops/beta/feedback", &fb, &csrf).0, 409);
    let (st, _) = write(
        &sh,
        "/ops/beta/note",
        &json!({"id": 1, "note": "friend of a friend"}),
        &csrf,
    );
    assert_eq!(st, 200);
    assert_eq!(
        write(
            &sh,
            "/ops/beta/note",
            &json!({"id": 99, "note": "x"}),
            &csrf
        )
        .0,
        400
    );
    let (_, body) = ops(&sh, "GET", "/ops/beta/list?q=t0@", &[], "", LO, HOST);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["rows"][0]["note"],
        "friend of a friend"
    );
}

#[test]
fn a_feedback_batch_goes_to_the_selected_testers() {
    let sh = shared();
    with_confirmed(&sh, 3);
    let csrf = csrf_of(&sh);
    assert_eq!(
        write(&sh, "/ops/beta/invite", &json!({"ids": [1, 2]}), &csrf).0,
        200
    );
    let (st, j) = write(
        &sh,
        "/ops/beta/feedback",
        &json!({"ids": [1, 3], "subject": "Hello {name}", "body": "Tell us, {name}."}),
        &csrf,
    );
    assert_eq!(st, 200, "{j}");
    assert_eq!(j["results"][0]["ok"], true);
    assert_eq!(j["results"][1]["ok"], false, "row 3 is only confirmed");
    let m = mails(&sh);
    assert_eq!(m.len(), 3);
    assert_eq!(m[2].subject, "[T] Hello t0");
    // no subject, no mail
    assert_eq!(
        write(
            &sh,
            "/ops/beta/feedback",
            &json!({"ids": [1], "subject": "", "body": "x"}),
            &csrf
        )
        .0,
        400
    );
    // a registered account shows up with its login in the list
    {
        let g = crate::net::lock(&sh);
        let u = crate::db::create_account(&g.db, "Tester", NOW).unwrap();
        let code: String =
            g.db.query_row(
                "SELECT invite_code FROM beta_signups WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(crate::accounts::consume_invite(&g.db, &code, u, NOW).unwrap());
    }
    let (_, body) = ops(
        &sh,
        "GET",
        "/ops/beta/list?status=registered",
        &[],
        "",
        LO,
        HOST,
    );
    let l: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(l["rows"][0]["status"], "registered");
    assert_eq!(l["rows"][0]["account"]["login"], "Tester");
}

#[test]
fn a_registration_with_the_mailed_code_marks_the_signup_registered() {
    let sh = shared();
    with_confirmed(&sh, 1);
    let csrf = csrf_of(&sh);
    assert_eq!(
        write(&sh, "/ops/beta/invite", &json!({"next": 1}), &csrf).0,
        200
    );
    let code = mails(&sh)[0]
        .body
        .split("Your invite code: ")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    let mut g = crate::net::lock(&sh);
    g.opts.register = true;
    // the registration page (the game's sign-up dialog posts the same fields to the same function)
    let form = format!(
        "name=Newbie&pw=secret12&pw2=secret12&email=t0%40example.com&code={}",
        crate::clanweb::url_encode(&code)
    );
    let mut out = (0, String::new());
    g.scope(|st| {
        out = crate::register::route(
            st,
            "POST",
            form.as_bytes(),
            "198.51.100.30:1",
            Some("play.example.org"),
            std::time::Instant::now(),
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(out.0, 200, "{}", out.1);
    let (status, user_no): (String, i64) =
        g.db.query_row(
            "SELECT status, user_no FROM beta_signups WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let want = crate::db::account_by_login(&g.db, "Newbie")
        .unwrap()
        .unwrap()
        .user_no;
    assert_eq!((status.as_str(), user_no), ("registered", i64::from(want)));
}

#[test]
fn the_page_deletes_selected_signups_through_the_same_guard() {
    let sh = shared();
    with_confirmed(&sh, 3);
    let csrf = csrf_of(&sh);
    let (st, page) = ops(&sh, "GET", "/ops/beta", &[], "", LO, HOST);
    assert!(st == 200 && page.contains("id=\"del-do\"") && page.contains("confirm("));
    let count = || {
        crate::net::lock(&sh)
            .db
            .query_row("SELECT COUNT(*) FROM beta_signups", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
    };
    // the guard of every other write: no token, a wrong origin, a form type
    let body = json!({"ids": [1]}).to_string();
    let origin = format!("http://{HOST}");
    let attempt =
        |headers: &[(&str, &str)]| ops(&sh, "POST", "/ops/beta/delete", headers, &body, LO, HOST).0;
    assert_eq!(
        attempt(&[("Content-Type", "application/json"), ("Origin", &origin)]),
        403
    );
    assert_eq!(
        attempt(&[
            ("Content-Type", "application/json"),
            ("Origin", "http://evil.example"),
            ("X-Beta-CSRF", &csrf)
        ]),
        403
    );
    assert_eq!(
        attempt(&[
            ("Content-Type", "text/plain"),
            ("Origin", &origin),
            ("X-Beta-CSRF", &csrf)
        ]),
        415
    );
    assert_eq!(count(), 3, "a refused request deletes nothing");
    // it needs ids: "next" (the invite batch's shortcut) is not taken for a delete
    assert_eq!(
        write(&sh, "/ops/beta/delete", &json!({"next": 3}), &csrf).0,
        400
    );
    assert_eq!(write(&sh, "/ops/beta/delete", &json!({}), &csrf).0, 400);
    assert_eq!(count(), 3);
    // a dry run changes nothing
    let (st, j) = write(
        &sh,
        "/ops/beta/delete",
        &json!({"ids": [1], "dry_run": true}),
        &csrf,
    );
    assert_eq!((st, j["sent"].clone()), (200, json!(1)), "{j}");
    assert_eq!(count(), 3);
    // the real one, with a missing id reported per row; a GET is not allowed
    let (st, j) = write(&sh, "/ops/beta/delete", &json!({"ids": [1, 99]}), &csrf);
    assert_eq!(st, 200, "{j}");
    assert_eq!(j["results"][0]["ok"], true);
    assert_eq!(j["results"][1]["message"], "no such signup");
    assert_eq!(count(), 2);
    assert_eq!(
        ops(&sh, "GET", "/ops/beta/delete", &[], "", LO, HOST).0,
        405
    );
}
