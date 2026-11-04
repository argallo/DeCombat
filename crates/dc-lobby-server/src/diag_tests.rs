//! Tests of the diagnostic reports: upload, limits, dedupe, identity, mail, retention, the UDP echo and the CLI.

use super::*;
use crate::identity::Authority;
use crate::logs::Logger;
use crate::mail::{MailOpts, Mailer};
use crate::state::testing::*;
use ring::signature::Ed25519KeyPair;
use std::io::Write;

const NOW: i64 = 1_793_620_800; // 2026-11-02 12:00:00 UTC

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("dc_diag_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn svc(tag: &str) -> Service {
    Service::new(&tmp(tag), &|_| None)
}

fn gz(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn hex_id(n: u64) -> String {
    format!("{n:016x}")
}

fn doc_json(id: &str, kind: &str, summary: &str) -> serde_json::Value {
    serde_json::json!({
        "format": "dcdiag1", "id": id, "created": NOW, "kind": kind, "summary": summary,
        "client": {"version": "0.9.1", "commit": "abc1234", "target": "x86_64", "profile": "release"},
        "system": {"os": "Windows 11", "gpu": "Test GPU 9000", "driver": "1.2"},
        "settings": {"fps_cap": 1000},
        "match": {"map": "m1"},
        "link": {"rtt_mean": 41, "loss_in": 0.5},
        "note": "it froze",
        "login": "zoe",
        "files": [
            {"name": "dc-client.out", "text": "line one\nline two\n"},
            {"name": "../evil.txt", "text": "x"},
            {"name": "dc-client.out", "text": "dup"},
        ],
    })
}

fn report(n: u64, kind: &str, summary: &str) -> Vec<u8> {
    gz(doc_json(&hex_id(n), kind, summary).to_string().as_bytes())
}

fn shared_state() -> Shared {
    Arc::new(Mutex::new(state()))
}

fn head(token: Option<&str>) -> String {
    let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
    format!("POST /diag/report HTTP/1.1\r\nHost: x\r\nContent-Type: application/x-dcdiag\r\n{auth}\r\n")
}

fn post(sh: &Shared, s: &Service, token: Option<&str>, peer: &str, body: &[u8], now: i64) -> (u16, String) {
    handle(sh, s, &head(token), body, peer, now)
}

fn count(sh: &Shared) -> i64 {
    let g = crate::net::lock(sh);
    ensure_schema(&g.db).unwrap();
    g.db.query_row("SELECT COUNT(*) FROM diag_reports", [], |r| r.get(0)).unwrap()
}

fn issuing() -> Authority {
    let doc = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
    Authority::issuing("dc.main", Ed25519KeyPair::from_pkcs8(doc.as_ref()).unwrap(), 24)
}

fn dry_mail(cap: u32) -> Mailer {
    Mailer::start(
        &MailOpts {
            dry_run: true,
            reports_to: "owner@test.invalid".into(),
            reports_per_hour: cap,
            subject_prefix: "[T]".into(),
            ..Default::default()
        },
        Arc::new(Logger::none()),
    )
}

fn put_row(sh: &Shared, created: i64, account: &str, ip: &str, bytes: i64) {
    let g = crate::net::lock(sh);
    ensure_schema(&g.db).unwrap();
    g.db.execute(
        "INSERT INTO diag_reports(created, client_id, kind, summary, account, login, ip, version, client_commit, os, gpu, link, bytes, rel, files) \
         VALUES (?1, ?2, 'manual', '', ?3, '', ?4, '', '', '', '', '', ?5, '', '[]')",
        params![created, format!("seed{created}{ip}{account}"), account, ip, bytes],
    )
    .unwrap();
}

// ---------------------------------------------------------------------------------------------
// upload
// ---------------------------------------------------------------------------------------------

#[test]
fn an_upload_is_stored_as_a_row_and_a_file() {
    let sh = shared_state();
    let s = svc("stored");
    let body = report(1, "link_lost", "the link dropped");
    let (st, json) = post(&sh, &s, None, "203.0.113.5:50000", &body, NOW);
    assert_eq!((st, json.as_str()), (200, "{\"id\":1}"));
    let g = crate::net::lock(&sh);
    let r: Stored = g
        .db
        .query_row(&format!("SELECT {COLS} FROM diag_reports"), [], stored_of)
        .unwrap();
    assert_eq!((r.kind.as_str(), r.summary.as_str()), ("link_lost", "the link dropped"));
    assert_eq!((r.account.as_str(), r.login.as_str(), r.ip.as_str()), ("", "zoe", "203.0.113.5"));
    assert_eq!((r.version.as_str(), r.commit.as_str(), r.os.as_str()), ("0.9.1", "abc1234", "Windows 11"));
    assert_eq!(r.gpu, "Test GPU 9000");
    assert!(r.link.contains("rtt_mean=41"), "{}", r.link);
    assert_eq!(r.rel, "2026-11/1.dcdiag.gz");
    assert_eq!(r.bytes, body.len() as i64);
    assert_eq!(r.mailed, 2, "no mail set up here");
    assert_eq!(file_list(&r.files).len(), 3);
    // the bytes are kept unchanged
    let path = s.dir.join("2026-11").join("1.dcdiag.gz");
    assert_eq!(std::fs::read(path).unwrap(), body);
}

#[test]
fn things_that_are_not_reports_are_400_and_a_bomb_is_413() {
    let sh = shared_state();
    let s = svc("bad");
    let p = "198.51.100.1:4000";
    assert_eq!(post(&sh, &s, None, p, b"", NOW).0, 400);
    assert_eq!(post(&sh, &s, None, p, b"this is not gzip at all", NOW).0, 400);
    assert_eq!(post(&sh, &s, None, p, &gz(b"plain text"), NOW).0, 400);
    assert_eq!(post(&sh, &s, None, p, &gz(b"[1,2]"), NOW).0, 400);
    let other = serde_json::json!({"format": "other", "id": hex_id(1)}).to_string();
    let (st, json) = post(&sh, &s, None, p, &gz(other.as_bytes()), NOW);
    assert_eq!(st, 400);
    assert!(json.contains("dcdiag1"), "{json}");
    let bad_id = serde_json::json!({"format": "dcdiag1", "id": "xyz"}).to_string();
    assert_eq!(post(&sh, &s, None, p, &gz(bad_id.as_bytes()), NOW).0, 400);
    // a truncated gzip stream
    let mut cut = report(1, "manual", "x");
    cut.truncate(cut.len() / 2);
    assert_eq!(post(&sh, &s, None, p, &cut, NOW).0, 400);
    // a bomb: 17 MiB of zeros in a few KB
    let bomb = gz(&vec![0u8; 17 << 20]);
    assert!(bomb.len() < MAX_BODY, "the bomb fits the body cap: {}", bomb.len());
    assert_eq!(post(&sh, &s, None, p, &bomb, NOW).0, 413);
    assert_eq!(count(&sh), 0);
    assert_eq!(inflate(&gz(&[1u8; 100]), 50), Err(Reject::TooLarge));
    assert_eq!(inflate(&gz(&[1u8; 100]), 100).unwrap().len(), 100);
}

#[test]
fn an_unknown_kind_is_kept_as_other() {
    let sh = shared_state();
    let s = svc("kind");
    assert_eq!(post(&sh, &s, None, "198.51.100.1:4000", &report(1, "from_the_future", "x"), NOW).0, 200);
    let g = crate::net::lock(&sh);
    let k: String = g.db.query_row("SELECT kind FROM diag_reports", [], |r| r.get(0)).unwrap();
    assert_eq!(k, "other");
}

#[test]
fn the_body_cap_and_methods_through_the_web_port() {
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    let dir = tmp("http");
    configure(&dir);
    let sh = shared_state();
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    crate::clanweb::spawn_with(Arc::clone(&sh), l, crate::hosting::Hosting::none());
    let ask = |req: &[u8]| -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(req).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    };
    let post_req = |body: &[u8]| -> Vec<u8> {
        let mut v = format!(
            "POST /diag/report HTTP/1.1\r\nHost: x\r\nContent-Type: application/x-dcdiag\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        v.extend_from_slice(body);
        v
    };
    let r = ask(&post_req(&report(7, "stall", "a stall")));
    assert!(r.starts_with("HTTP/1.1 200 OK"), "{r}");
    assert!(r.contains("application/json") && r.ends_with("{\"id\":1}"), "{r}");
    // over 1 MiB: refused from the header, before the body is read
    let r = ask(b"POST /diag/report HTTP/1.1\r\nHost: x\r\nContent-Length: 1048577\r\n\r\n");
    assert!(r.starts_with("HTTP/1.1 413"), "{r}");
    // exactly the cap is read (and is not a report)
    let r = ask(&post_req(&vec![7u8; MAX_BODY]));
    assert!(r.starts_with("HTTP/1.1 400"), "{r}");
    let r = ask(b"GET /diag/report HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(r.starts_with("HTTP/1.1 405"), "{r}");
    assert_eq!(count(&sh), 1);
}

#[test]
fn diag_off_closes_the_endpoint() {
    let sh = shared_state();
    let s = Service::new(&tmp("off"), &|k| (k == "DC_DIAG").then(|| "off".to_string()));
    assert!(!s.enabled);
    assert_eq!(post(&sh, &s, None, "198.51.100.1:4000", &report(1, "manual", "x"), NOW).0, 404);
    assert_eq!(count(&sh), 0);
    let on = Service::new(&tmp("on"), &|k| match k {
        "DC_DIAG_MAX_TOTAL_MB" => Some("5".into()),
        "DC_DIAG_RETENTION_DAYS" => Some("3".into()),
        _ => None,
    });
    assert!(on.enabled);
    assert_eq!((on.max_total, on.retention_days), (5 << 20, 3));
    let d = Service::new(&tmp("def"), &|_| None);
    assert_eq!((d.max_total, d.retention_days), (2048 << 20, 30));
}

// ---------------------------------------------------------------------------------------------
// identity, dedupe, limits
// ---------------------------------------------------------------------------------------------

#[test]
fn a_valid_token_names_the_account_and_a_bad_one_is_anonymous() {
    let sh = shared_state();
    let s = svc("tok");
    let auth = issuing();
    let (token, _) = auth.issue(7, "ann", "Ann", NOW).unwrap();
    crate::net::lock(&sh).identity = auth;
    assert_eq!(post(&sh, &s, Some(&token), "198.51.100.1:4000", &report(1, "crash", "a"), NOW).0, 200);
    assert_eq!(post(&sh, &s, Some("tok.garbage"), "198.51.100.1:4000", &report(2, "crash", "b"), NOW).0, 200);
    assert_eq!(post(&sh, &s, None, "198.51.100.1:4000", &report(3, "crash", "c"), NOW).0, 200);
    // an expired token is anonymous too
    assert_eq!(post(&sh, &s, Some(&token), "198.51.100.1:4000", &report(4, "crash", "d"), NOW + 90_000).0, 200);
    let g = crate::net::lock(&sh);
    let rows: Vec<(String, String)> = g
        .db
        .prepare("SELECT account, login FROM diag_reports ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(rows[0], ("dc.main:7".to_string(), "ann".to_string()));
    assert_eq!(rows[1], (String::new(), "zoe".to_string()));
    assert_eq!(rows[2].0, "");
    assert_eq!(rows[3].0, "");
}

#[test]
fn a_lobby_without_identity_takes_a_token_as_anonymous() {
    let sh = shared_state();
    let s = svc("noid");
    let (token, _) = issuing().issue(7, "ann", "Ann", NOW).unwrap();
    assert_eq!(post(&sh, &s, Some(&token), "198.51.100.1:4000", &report(1, "crash", "a"), NOW).0, 200);
    let g = crate::net::lock(&sh);
    let a: String = g.db.query_row("SELECT account FROM diag_reports", [], |r| r.get(0)).unwrap();
    assert_eq!(a, "");
}

#[test]
fn a_repeated_client_id_is_stored_once() {
    let sh = shared_state();
    let s = svc("dedupe");
    let auth = issuing();
    let (token, _) = auth.issue(7, "ann", "Ann", NOW).unwrap();
    crate::net::lock(&sh).identity = auth;
    let body = report(5, "manual", "x");
    // anonymous: the same address
    assert_eq!(post(&sh, &s, None, "198.51.100.1:4000", &body, NOW).1, "{\"id\":1}");
    assert_eq!(post(&sh, &s, None, "198.51.100.1:4999", &body, NOW + 5).1, "{\"id\":1}");
    assert_eq!(count(&sh), 1);
    // another address is another sender
    assert_eq!(post(&sh, &s, None, "198.51.100.2:4000", &body, NOW + 6).1, "{\"id\":2}");
    // an account: from any address
    assert_eq!(post(&sh, &s, Some(&token), "198.51.100.3:4000", &body, NOW + 7).1, "{\"id\":3}");
    assert_eq!(post(&sh, &s, Some(&token), "198.51.100.9:4000", &body, NOW + 8).1, "{\"id\":3}");
    assert_eq!(count(&sh), 3);
}

#[test]
fn anonymous_senders_are_limited_per_address() {
    let sh = shared_state();
    let s = svc("anon");
    let p = "198.51.100.1:4000";
    for i in 0..3 {
        assert_eq!(post(&sh, &s, None, p, &report(i, "manual", "x"), NOW + i as i64).0, 200);
    }
    let (st, json) = post(&sh, &s, None, p, &report(3, "manual", "x"), NOW + 10);
    assert_eq!(st, 429, "{json}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let retry = v["retry_after"].as_i64().unwrap();
    assert!((1..=3600).contains(&retry), "{retry}");
    assert!(v["error"].as_str().unwrap().contains("hour"));
    // another address is not held back
    assert_eq!(post(&sh, &s, None, "198.51.100.2:4000", &report(3, "manual", "x"), NOW + 10).0, 200);
    // after the hour the first address may send again, until the day's 10
    let mut t = NOW + 3700;
    let mut n = 10;
    let mut sent = 3;
    loop {
        let r = post(&sh, &s, None, p, &report(n, "manual", "x"), t);
        if r.0 == 429 {
            break;
        }
        assert_eq!(r.0, 200);
        sent += 1;
        n += 1;
        t += 1300;
        assert!(sent <= 10, "the day's limit never held");
    }
    assert_eq!(sent, 10);
}

#[test]
fn an_account_is_limited_per_hour_and_per_day_from_any_address() {
    let sh = shared_state();
    let s = svc("acct");
    let auth = issuing();
    let (token, _) = auth.issue(7, "ann", "Ann", NOW).unwrap();
    crate::net::lock(&sh).identity = auth;
    for i in 0..6 {
        let peer = format!("198.51.100.{}:4000", i + 1);
        assert_eq!(post(&sh, &s, Some(&token), &peer, &report(i, "manual", "x"), NOW + i as i64).0, 200);
    }
    let (st, json) = post(&sh, &s, Some(&token), "198.51.100.77:4000", &report(6, "manual", "x"), NOW + 10);
    assert_eq!(st, 429, "{json}");
    assert!(json.contains("account") && json.contains("retry_after"), "{json}");
    // spaced out: the day's 20 hold
    let mut sent = 6;
    let mut t = NOW + 4000;
    loop {
        let peer = format!("198.51.101.{}:4000", sent);
        let r = post(&sh, &s, Some(&token), &peer, &report(100 + sent, "manual", "x"), t);
        if r.0 == 429 {
            assert!(r.1.contains("today"), "{}", r.1);
            break;
        }
        assert_eq!(r.0, 200, "{}", r.1);
        sent += 1;
        t += 700;
        assert!(sent <= 20, "the day's limit never held");
    }
    assert_eq!(sent, 20);
}

#[test]
fn the_address_lobby_and_store_limits_hold() {
    let sh = shared_state();
    let mut s = svc("caps");
    // 30 reports of various accounts from one address in the day
    for i in 0..30 {
        put_row(&sh, NOW - 100 - i, &format!("x:{i}"), "203.0.113.9", 10);
    }
    let (st, json) = post(&sh, &s, None, "203.0.113.9:4000", &report(1, "manual", "x"), NOW);
    assert_eq!(st, 429, "{json}");
    assert!(json.contains("address"));
    // the whole lobby: 300 in the day
    for i in 0..270 {
        put_row(&sh, NOW - 200 - i, &format!("y:{i}"), &format!("10.1.{}.{}", i / 200, i % 200), 10);
    }
    let (st, json) = post(&sh, &s, None, "203.0.113.50:4000", &report(2, "manual", "x"), NOW);
    assert_eq!(st, 429, "{json}");
    assert!(json.contains("lobby"), "{json}");
    // a day later the lobby is open again
    assert_eq!(post(&sh, &s, None, "203.0.113.50:4000", &report(2, "manual", "x"), NOW + 86_400).0, 200);
    // the store: 503 when the new report does not fit
    s.max_total = 4000;
    put_row(&sh, NOW + 90_000, "z:1", "10.9.9.9", 4000);
    let (st, json) = post(&sh, &s, None, "203.0.113.51:4000", &report(3, "manual", "x"), NOW + 90_000);
    assert_eq!(st, 503, "{json}");
}

// ---------------------------------------------------------------------------------------------
// mail and retention
// ---------------------------------------------------------------------------------------------

fn pump(sh: &Shared, s: &Service) {
    let mut g = crate::net::lock(sh);
    g.scope(|st| s.pump(st)).unwrap();
}

#[test]
fn one_mail_per_report_with_a_fixed_subject_and_clean_text() {
    let sh = shared_state();
    let s = svc("mail");
    crate::net::lock(&sh).mail = dry_mail(10);
    let summary = "boom\u{7}\r\nBcc: evil@test.invalid\u{202e}";
    assert_eq!(post(&sh, &s, None, "198.51.100.1:4000", &report(1, "gpu_lost", summary), NOW).0, 200);
    for _ in 0..3 {
        pump(&sh, &s);
    }
    let g = crate::net::lock(&sh);
    assert_eq!(g.mail.dry.len(), 1, "one mail, however often the pump runs");
    let m = &g.mail.dry[0];
    assert_eq!(m.to, "owner@test.invalid");
    assert_eq!(m.subject, "[T] Diagnostic report #1: gpu_lost");
    assert!(m.body.contains("Read it on the server: dc-lobby-server reports show 1"), "{}", m.body);
    assert!(m.body.contains("dc-client.out (18 bytes)") && m.body.contains("Test GPU 9000"), "{}", m.body);
    assert!(m.body.contains("Time (UTC): 2026-11-02 12:00:00") && m.body.contains("Kind: gpu_lost"));
    assert!(m.body.contains("anonymous (login typed: zoe)") && m.body.contains("0.9.1"), "{}", m.body);
    assert!(
        !m.body.chars().any(|c| (c.is_control() && c != '\n') || c == '\u{202e}'),
        "{:?}",
        m.body
    );
    assert!(!m.subject.contains("evil") && !m.subject.contains('\r'));
    let mailed: (i64, i64) = g
        .db
        .query_row("SELECT mailed, mailed_at FROM diag_reports", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(mailed.0, 1);
    assert!(mailed.1 > 0);
}

#[test]
fn the_hourly_cap_of_the_diag_mails_holds_the_rest_back() {
    let sh = shared_state();
    let s = svc("cap");
    crate::net::lock(&sh).mail = dry_mail(2);
    for i in 0..4 {
        let peer = format!("198.51.100.{}:4000", i + 1);
        assert_eq!(post(&sh, &s, None, &peer, &report(i, "manual", "x"), NOW + i as i64).0, 200);
    }
    for _ in 0..8 {
        pump(&sh, &s);
    }
    let g = crate::net::lock(&sh);
    assert_eq!(g.mail.dry.len(), 2);
    let waiting: i64 = g
        .db
        .query_row("SELECT COUNT(*) FROM diag_reports WHERE mailed = 0", [], |r| r.get(0))
        .unwrap();
    assert_eq!(waiting, 2);
}

#[test]
fn with_the_mail_off_rows_are_kept_and_never_mailed() {
    let sh = shared_state();
    let s = svc("nomail");
    assert_eq!(post(&sh, &s, None, "198.51.100.1:4000", &report(1, "manual", "x"), NOW).0, 200);
    {
        let g = crate::net::lock(&sh);
        let m: i64 = g.db.query_row("SELECT mailed FROM diag_reports", [], |r| r.get(0)).unwrap();
        assert_eq!(m, 2);
    }
    // the mail turned on later does not send what was stored while it was off
    crate::net::lock(&sh).mail = dry_mail(10);
    pump(&sh, &s);
    pump(&sh, &s);
    assert!(crate::net::lock(&sh).mail.dry.is_empty());
}

#[test]
fn the_retention_purge_removes_rows_and_files() {
    let sh = shared_state();
    let s = Service::new(&tmp("purge"), &|k| (k == "DC_DIAG_RETENTION_DAYS").then(|| "10".to_string()));
    let old = NOW - 12 * 86_400;
    assert_eq!(post(&sh, &s, None, "198.51.100.1:4000", &report(1, "manual", "old"), old).0, 200);
    assert_eq!(post(&sh, &s, None, "198.51.100.2:4000", &report(2, "manual", "new"), NOW - 86_400).0, 200);
    let old_dir = s.dir.join(&utc_text(old)[..7]);
    let old_file = old_dir.join("1.dcdiag.gz");
    assert!(old_file.is_file());
    let g = crate::net::lock(&sh);
    assert_eq!(s.purge(&g.db, NOW).unwrap(), 1);
    assert!(!old_file.exists());
    assert!(s.dir.join("2026-11").join("2.dcdiag.gz").is_file());
    let left: Vec<i64> = g
        .db
        .prepare("SELECT id FROM diag_reports")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(left, vec![2]);
    assert_eq!(s.purge(&g.db, NOW).unwrap(), 0);
    // ids are not reused, so a purged number never names another file
    drop(g);
    assert_eq!(post(&sh, &s, None, "198.51.100.3:4000", &report(3, "manual", "x"), NOW).1, "{\"id\":3}");
}

// ---------------------------------------------------------------------------------------------
// the echo
// ---------------------------------------------------------------------------------------------

fn request(len: usize) -> Vec<u8> {
    let mut r = vec![0xAAu8; len];
    let head = [b'D', b'C', b'P', b'R', 1, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];
    let n = len.min(16);
    r[..n].copy_from_slice(&head[..n]);
    r
}

#[test]
fn the_echo_replies_with_exactly_the_request_length() {
    for len in (16..=8192).step_by(7).chain([16, 17, 512, 1024, 1200, 1472, 2000, 4000, 8000, 8191, 8192]) {
        let req = request(len);
        let rep = echo_reply(&req).unwrap();
        assert_eq!(rep.len(), len);
        assert_eq!(&rep[..4], b"DCPA");
        assert_eq!(&rep[4..16], &req[4..16]);
        assert!(rep[16..].iter().all(|b| *b == 0));
    }
}

#[test]
fn the_echo_drops_what_is_not_a_probe() {
    assert!(echo_reply(&request(15)).is_none());
    assert!(echo_reply(&[]).is_none());
    assert!(echo_reply(&request(8193)).is_none());
    assert!(echo_reply(&request(60_000)).is_none());
    let mut bad = request(100);
    bad[0] = b'X';
    assert!(echo_reply(&bad).is_none());
    let mut v2 = request(100);
    v2[4] = 2;
    assert!(echo_reply(&v2).is_none());
    let mut reply_shaped = request(100);
    reply_shaped[..4].copy_from_slice(b"DCPA");
    assert!(echo_reply(&reply_shaped).is_none(), "a reply is not answered (no echo loop)");
    // source ports below 1024
    let mut lim = RateLimiter::new();
    let ip: IpAddr = "203.0.113.5".parse().unwrap();
    assert!(echo_decide(&mut lim, &request(100), SocketAddr::new(ip, 1023), Duration::ZERO).is_none());
    assert!(echo_decide(&mut lim, &request(100), SocketAddr::new(ip, 53), Duration::ZERO).is_none());
    assert!(echo_decide(&mut lim, &request(100), SocketAddr::new(ip, 1024), Duration::ZERO).is_some());
}

#[test]
fn the_rate_limiter_drops_after_the_budget_and_refills() {
    let mut lim = RateLimiter::new();
    let ip: IpAddr = "203.0.113.5".parse().unwrap();
    let t = |ms: u64| Duration::from_millis(ms);
    // a burst of 2 s of 40 per second
    let ok = (0..200).filter(|_| lim.allow(ip, 64, t(0))).count();
    assert_eq!(ok, 80);
    // half a second later: 20 more
    let ok = (0..200).filter(|_| lim.allow(ip, 64, t(500))).count();
    assert_eq!(ok, 20);
    // idle for a long time: back to the burst, not more
    let ok = (0..200).filter(|_| lim.allow(ip, 64, t(60_000))).count();
    assert_eq!(ok, 80);
    // another address has its own budget
    let other: IpAddr = "203.0.113.6".parse().unwrap();
    assert!(lim.allow(other, 64, t(60_000)));
}

#[test]
fn the_byte_budget_holds_for_big_probes() {
    let mut lim = RateLimiter::new();
    let ip: IpAddr = "203.0.113.5".parse().unwrap();
    // 160 KB a second with a 2 s burst: 320000 / 8192 = 39 datagrams
    let ok = (0..100).filter(|_| lim.allow(ip, 8192, Duration::ZERO)).count();
    assert_eq!(ok, 39);
    // a second later: 160000 / 8192 = 19 more
    let ok = (0..100).filter(|_| lim.allow(ip, 8192, Duration::from_secs(1))).count();
    assert_eq!(ok, 19);
}

#[test]
fn all_sources_together_get_4000_a_second_and_the_table_is_bounded() {
    let ip = |i: u32| IpAddr::from([10, (i >> 16) as u8, (i >> 8) as u8, i as u8]);
    let mut lim = RateLimiter::new();
    let ok = (0..5000).filter(|i| lim.allow(ip(*i), 32, Duration::ZERO)).count();
    assert_eq!(ok, 4000);
    // a second later the next 4000
    let ok = (5000..9000).filter(|i| lim.allow(ip(*i), 32, Duration::from_secs(3))).count();
    assert_eq!(ok, 4000);
    // the table never grows past its bound; idle entries make room
    let mut lim = RateLimiter::new();
    for i in 0..4096 {
        lim.allow(ip(i), 32, Duration::from_secs(10));
    }
    assert_eq!(lim.tracked(), 4096);
    assert!(!lim.allow(ip(99_999), 32, Duration::from_secs(10)), "full of fresh entries: dropped");
    assert_eq!(lim.tracked(), 4096);
    assert!(lim.allow(ip(99_999), 32, Duration::from_secs(20)), "the idle ones were evicted");
    assert_eq!(lim.tracked(), 1);
}

#[test]
fn the_echo_answers_over_a_real_socket() {
    let sock = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    let addr = sock.local_addr().unwrap();
    std::thread::spawn(move || echo_serve(sock));
    let c = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    c.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    let mut buf = vec![0u8; 70_000];
    for len in [16usize, 100, 1472, 8192] {
        c.send_to(&request(len), addr).unwrap();
        let (n, from) = c.recv_from(&mut buf).unwrap();
        assert_eq!((n, from), (len, addr));
        assert_eq!(&buf[..4], b"DCPA");
    }
    // a short one and a long one get nothing
    c.send_to(&request(15), addr).unwrap();
    assert!(c.recv_from(&mut buf).is_err());
    c.send_to(&request(9000), addr).unwrap();
    assert!(c.recv_from(&mut buf).is_err());
}

// ---------------------------------------------------------------------------------------------
// the CLI
// ---------------------------------------------------------------------------------------------

/// A lobby whose DB is a file in a new folder, with two stored reports.
fn file_lobby(tag: &str) -> (Shared, Service, PathBuf) {
    let dir = tmp(tag);
    let path = dir.join("ca.db");
    let sh = shared_state();
    crate::net::lock(&sh).db = crate::db::open(&path).unwrap();
    let s = Service::new(&dir, &|_| None);
    assert_eq!(post(&sh, &s, None, "198.51.100.1:4000", &report(1, "link_lost", "dropped\u{1b}[31m"), NOW).0, 200);
    assert_eq!(post(&sh, &s, None, "198.51.100.2:4000", &report(2, "crash", "crashed"), NOW + 60).0, 200);
    (sh, s, path)
}

#[test]
fn the_cli_lists_shows_and_fetches() {
    let (sh, s, path) = file_lobby("cli");
    let g = crate::net::lock(&sh);
    let list = list_text(&g.db, 10, None).unwrap();
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("#2  2026-11-02 12:01:00  crash"), "{list}");
    assert!(lines[1].contains("link_lost") && lines[1].contains("0.9.1") && lines[1].contains("dropped"), "{list}");
    assert!(!list.contains('\u{1b}'));
    let only = list_text(&g.db, 10, Some("crash")).unwrap();
    assert_eq!(only.lines().count(), 1);
    assert_eq!(list_text(&g.db, 1, None).unwrap().lines().count(), 1);
    assert_eq!(list_text(&g.db, 10, Some("stall")).unwrap(), "no reports\n");

    let show = show_text(&g.db, &s.dir.parent().unwrap().join(DIR_NAME), 1, false).unwrap();
    assert!(show.contains("Report #1") && show.contains("Test GPU 9000") && show.contains("it froze"), "{show}");
    assert!(show.contains("dc-client.out (18 bytes)") && !show.contains("line one"), "{show}");
    let full = show_text(&g.db, &s.dir, 1, true).unwrap();
    assert!(full.contains("==== dc-client.out ====\nline one\nline two"), "{full}");
    assert!(!full.contains('\u{1b}'), "no escape sequences reach the terminal");
    assert!(show_text(&g.db, &s.dir, 99, false).is_err());

    let out = tmp("cli_out");
    let msg = fetch_to(&g.db, &s.dir, 1, &out).unwrap();
    assert!(msg.contains("report_1"), "{msg}");
    let dir = out.join("report_1");
    let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("report.json")).unwrap()).unwrap();
    assert!(meta.get("files").is_none());
    assert_eq!(meta["kind"], "link_lost");
    assert_eq!(meta["client"]["version"], "0.9.1");
    assert_eq!(std::fs::read_to_string(dir.join("dc-client.out")).unwrap(), "line one\nline two\n");
    assert_eq!(std::fs::read_to_string(dir.join("dc-client.out_2")).unwrap(), "dup");
    assert_eq!(std::fs::read_to_string(dir.join("_evil.txt")).unwrap(), "x");
    assert!(!out.join("evil.txt").exists());
    assert_eq!(
        std::fs::read(dir.join("1.dcdiag.gz")).unwrap(),
        report(1, "link_lost", "dropped\u{1b}[31m")
    );
    drop(g);
    // through the argument parser
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let p = path.to_string_lossy().into_owned();
    assert_eq!(run_subcommand(&a(&["--db", &p, "list", "--limit", "5"])), 0);
    assert_eq!(run_subcommand(&a(&["list", &format!("--db={p}"), "--kind", "crash"])), 0);
    assert_eq!(run_subcommand(&a(&["--db", &p, "show", "1", "--files"])), 0);
    let out2 = tmp("cli_out2");
    assert_eq!(run_subcommand(&a(&["--db", &p, "fetch", "2", "--out", &out2.to_string_lossy()])), 0);
    assert!(out2.join("report_2").join("report.json").is_file());
    assert_eq!(run_subcommand(&a(&["--db", &p, "show", "99"])), 1);
    assert_eq!(run_subcommand(&a(&["--db", &p, "show"])), 1);
    assert_eq!(run_subcommand(&a(&["--db", &p, "bogus"])), 2);
    assert_eq!(run_subcommand(&a(&["--db", &p, "list", "--limit", "0"])), 2);
    assert_eq!(run_subcommand(&a(&["--db", "/no/such/file.db", "list"])), 1);
}

#[test]
fn file_names_stay_inside_the_folder() {
    assert_eq!(safe_name("dc-client.out"), "dc-client.out");
    assert_eq!(safe_name("../../etc/passwd"), "_._etc_passwd");
    assert_eq!(safe_name("..\\..\\boot.ini"), "_._boot.ini");
    assert_eq!(safe_name("stall_1/log.txt"), "stall_1_log.txt");
    assert_eq!(safe_name(".."), "file");
    assert_eq!(safe_name(""), "file");
    assert_eq!(safe_name("a..b"), "a.b");
    assert_eq!(safe_name("C:\\x"), "C__x");
    assert_eq!(safe_name("CON.txt"), "_CON.txt");
    assert_eq!(safe_name("naïve file"), "na_ve_file");
    assert!(safe_name(&"a".repeat(500)).len() <= 100);
    for n in ["a/b", "..", "x\0y", "a\\b", "./c"] {
        let s = safe_name(n);
        assert!(!s.contains('/') && !s.contains('\\') && !s.contains("..") && !s.is_empty(), "{n} -> {s}");
    }
}
