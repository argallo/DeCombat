//! Tests of adminweb.rs with the software security key of webauthn_tests.rs and a TEST attestation root.
// Provenance: ORIGINAL (DeCombat test code)

use super::*;
use crate::webauthn::tests::{SoftKey, TestCa, ORIGIN, RP};
use std::net::Ipv4Addr;

/// What a test key answers with: three byte strings.
type Answer = (Vec<u8>, Vec<u8>, Vec<u8>);

const LO: Option<IpAddr> = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
const HOST: &str = "admin.example.com";
const T0: i64 = 1_800_000_000;

struct T {
    sh: Shared,
    web: Web,
    ca: TestCa,
    now: i64,
    ip: String,
}

fn setup_with(f: impl FnOnce(&mut AdminWebSection)) -> T {
    let sh: Shared = Arc::new(Mutex::new(crate::state::testing::state()));
    let mut cfg = AdminWebSection {
        enabled: true,
        public_origin: ORIGIN.into(),
        rp_id: RP.into(),
        attempts_per_ip: 1000,
        attempts_global: 1000,
        ..Default::default()
    };
    f(&mut cfg);
    let ca = TestCa::new("Test FIDO Root");
    let web = Runtime::shared(cfg, ca.trust());
    T {
        sh,
        web,
        ca,
        now: T0,
        ip: "192.0.2.10".into(),
    }
}

fn setup() -> T {
    setup_with(|_| {})
}

struct Req<'a> {
    method: &'a str,
    path: &'a str,
    cookie: Option<&'a str>,
    csrf: Option<&'a str>,
    origin: &'a str,
    ctype: &'a str,
    host: &'a str,
    extra: &'a str,
}

fn req<'a>(method: &'a str, path: &'a str) -> Req<'a> {
    Req {
        method,
        path,
        cookie: None,
        csrf: None,
        origin: ORIGIN,
        ctype: "application/json",
        host: HOST,
        extra: "",
    }
}

impl T {
    fn send(&self, r: &Req, body: &Value) -> Resp {
        let b = if r.method == "POST" {
            body.to_string().into_bytes()
        } else {
            Vec::new()
        };
        let mut h = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nCF-Connecting-IP: {}\r\n{}",
            r.method, r.path, r.host, self.ip, r.extra
        );
        if r.method == "POST" {
            h += &format!(
                "Content-Type: {}\r\nOrigin: {}\r\nContent-Length: {}\r\n",
                r.ctype,
                r.origin,
                b.len()
            );
        }
        if let Some(c) = r.cookie {
            h += &format!("Cookie: theme=dark; {COOKIE}={c}\r\n");
        }
        if let Some(t) = r.csrf {
            h += &format!("X-Admin-CSRF: {t}\r\n");
        }
        h += "\r\n";
        handle(&self.sh, &self.web, &h, &b, LO, self.now)
    }

    fn post(&self, path: &str, s: Option<&Sess>, body: Value) -> (u16, Value) {
        let mut r = req("POST", path);
        r.cookie = s.map(|s| s.cookie.as_str());
        r.csrf = s.map(|s| s.csrf.as_str());
        let out = self.send(&r, &body);
        (
            out.status,
            serde_json::from_str(&out.body).unwrap_or(Value::Null),
        )
    }

    fn get(&self, path: &str, s: Option<&Sess>) -> Resp {
        let mut r = req("GET", path);
        r.cookie = s.map(|s| s.cookie.as_str());
        self.send(&r, &Value::Null)
    }

    fn link(&self, label: &str) -> String {
        new_enroll_link(&crate::net::lock(&self.sh).db, label, self.now).unwrap()
    }

    fn key_count(&self) -> usize {
        keys(&crate::net::lock(&self.sh).db).unwrap().len()
    }

    /// The enrolment of `k` through the one-time link `token`.
    fn enroll_with(&self, k: &mut SoftKey, token: &str, label: &str) -> (u16, Value) {
        let (st, o) = self.post(
            "/auth/enroll/options",
            None,
            json!({"token": token, "label": label}),
        );
        if st != 200 {
            return (st, o);
        }
        assert_eq!(o["publicKey"]["attestation"], "direct");
        assert_eq!(
            o["publicKey"]["authenticatorSelection"]["authenticatorAttachment"],
            "cross-platform"
        );
        assert_eq!(
            o["publicKey"]["authenticatorSelection"]["userVerification"],
            "required"
        );
        let ch = b64::decode(o["publicKey"]["challenge"].as_str().unwrap()).unwrap();
        let (cdj, ao) = k.create(RP, ORIGIN, &ch);
        self.post(
            "/auth/enroll/finish",
            None,
            json!({"ceremony": o["ceremony"], "token": token, "label": label, "credential": att_json(k, &cdj, &ao)}),
        )
    }

    fn enroll(&self, k: &mut SoftKey, label: &str) {
        let token = self.link("");
        let (st, v) = self.enroll_with(k, &token, label);
        assert_eq!(st, 200, "{v}");
    }

    /// Options, then the key's answer made by `answer` (so a test can tamper with it), then finish.
    fn login_with(
        &self,
        k: &mut SoftKey,
        answer: &dyn Fn(&mut SoftKey, &[u8]) -> Answer,
    ) -> (Resp, Value) {
        let (st, o) = self.post("/auth/login/options", None, json!({}));
        assert_eq!(st, 200, "{o}");
        assert_eq!(o["publicKey"]["userVerification"], "required");
        let ch = b64::decode(o["publicKey"]["challenge"].as_str().unwrap()).unwrap();
        let (cdj, ad, sig) = answer(k, &ch);
        let body = json!({"ceremony": o["ceremony"], "credential": get_json(k, &cdj, &ad, &sig)});
        let out = self.send(&req("POST", "/auth/login/finish"), &body);
        (out, body)
    }

    fn login(&self, k: &mut SoftKey) -> Sess {
        let (out, _) = self.login_with(k, &|k, ch| k.get(RP, ORIGIN, ch));
        assert_eq!(out.status, 200, "{}", out.body);
        let cookie = out
            .headers
            .split("\r\n")
            .find_map(|l| l.strip_prefix(&format!("Set-Cookie: {COOKIE}=")))
            .and_then(|v| v.split(';').next())
            .unwrap()
            .to_string();
        let page = self.get(
            "/keys",
            Some(&Sess {
                cookie: cookie.clone(),
                csrf: String::new(),
            }),
        );
        assert_eq!(page.status, 200);
        let csrf = page
            .body
            .split("name=\"csrf\" content=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string();
        assert!(!csrf.is_empty());
        Sess { cookie, csrf }
    }

    /// A fresh confirmation with `k` in session `s`.
    fn step_up(&self, k: &mut SoftKey, s: &Sess) -> u16 {
        let (st, o) = self.post("/auth/stepup/options", Some(s), json!({}));
        assert_eq!(st, 200, "{o}");
        let ch = b64::decode(o["publicKey"]["challenge"].as_str().unwrap()).unwrap();
        let (cdj, ad, sig) = k.get(RP, ORIGIN, &ch);
        self.post(
            "/auth/stepup/finish",
            Some(s),
            json!({"ceremony": o["ceremony"], "credential": get_json(k, &cdj, &ad, &sig)}),
        )
        .0
    }
}

struct Sess {
    cookie: String,
    csrf: String,
}

fn att_json(k: &SoftKey, cdj: &[u8], ao: &[u8]) -> Value {
    json!({"id": b64::url(&k.cred_id), "rawId": b64::url(&k.cred_id), "type": "public-key",
           "response": {"clientDataJSON": b64::url(cdj), "attestationObject": b64::url(ao), "transports": ["usb", "nfc", "bogus"]}})
}

fn get_json(k: &SoftKey, cdj: &[u8], ad: &[u8], sig: &[u8]) -> Value {
    json!({"id": b64::url(&k.cred_id), "rawId": b64::url(&k.cred_id), "type": "public-key",
           "response": {"clientDataJSON": b64::url(cdj), "authenticatorData": b64::url(ad), "signature": b64::url(sig)}})
}

#[test]
fn a_key_enrols_through_the_one_time_link_and_signs_in() {
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    let token = t.link("YubiKey blue");
    // no key yet: nobody can sign in
    assert_eq!(t.post("/auth/login/options", None, json!({})).0, 409);
    let (st, v) = t.enroll_with(&mut k, &token, "");
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["redirect"], "/login");
    let ks = keys(&crate::net::lock(&t.sh).db).unwrap();
    assert_eq!(ks.len(), 1);
    assert_eq!(
        ks[0].label, "YubiKey blue",
        "the link's label is the default"
    );
    assert_eq!(ks[0].root, "Test FIDO Root");
    assert_eq!(ks[0].transports, "usb,nfc");
    assert_eq!(ks[0].cred_id, k.cred_id);
    // the link is used up
    let mut k2 = SoftKey::new(&t.ca);
    assert_eq!(t.enroll_with(&mut k2, &token, "").0, 403);
    // signing in gives a session for the dashboard
    let s = t.login(&mut k);
    let ops = t.get("/ops", Some(&s));
    assert_eq!(ops.status, 200);
    assert!(
        ops.body.contains("<title>DeCombat ops</title>") && ops.body.contains("href=\"/keys\"")
    );
    assert_eq!(t.get("/ops/live", Some(&s)).status, 200);
    assert_eq!(t.get("/ops/beta", Some(&s)).status, 200);
    assert_eq!(t.get("/", Some(&s)).headers, "Location: /ops\r\n");
    assert_eq!(t.get("/login", Some(&s)).status, 303);
    // the counter and the time of use are stored
    let row = key_by_cred(&crate::net::lock(&t.sh).db, &k.cred_id)
        .unwrap()
        .unwrap();
    assert_eq!(row.counter, k.counter);
    assert_eq!(row.last_used, T0);
}

#[test]
fn the_session_cookie_has_the_host_prefix_and_strict_flags() {
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    let (out, _) = t.login_with(&mut k, &|k, ch| k.get(RP, ORIGIN, ch));
    let line = out
        .headers
        .lines()
        .find(|l| l.starts_with("Set-Cookie:"))
        .unwrap();
    assert!(line.starts_with("Set-Cookie: __Host-dcadmin="), "{line}");
    assert!(
        line.ends_with("; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=43200"),
        "{line}"
    );
    assert!(!line.contains("Domain"));
    let value = line.split('=').nth(1).unwrap().split(';').next().unwrap();
    assert_eq!(b64::decode(value).unwrap().len(), 32);
    // the security headers of every answer
    let bytes = String::from_utf8(out.bytes()).unwrap();
    for h in [
        "Cache-Control: no-store",
        "X-Frame-Options: DENY",
        "Referrer-Policy: no-referrer",
        "X-Content-Type-Options: nosniff",
        "frame-ancestors 'none'",
    ] {
        assert!(bytes.contains(h), "{h}");
    }
    // logout ends the session and clears the cookie
    let s = t.login(&mut k);
    let mut r = req("POST", "/logout");
    r.cookie = Some(&s.cookie);
    r.csrf = Some(&s.csrf);
    let out = t.send(&r, &json!({}));
    assert_eq!(out.status, 200);
    assert!(out.headers.contains(
        "Set-Cookie: __Host-dcadmin=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0"
    ));
    assert_eq!(t.get("/ops/live", Some(&s)).status, 401);
}

#[test]
fn pages_carry_a_csp_without_inline_script_except_the_hashed_dashboard_scripts() {
    let t = setup();
    let login = t.get("/login", None);
    assert_eq!(login.status, 200);
    assert!(
        login.body.contains("data-page=\"login\"") && login.body.contains("src=\"/admin-web.js\"")
    );
    assert!(!login.body.contains("<script>"));
    // the allowed inline scripts are the two templates' own, hashed once
    let want: Vec<String> = [crate::opsweb::PAGE, crate::betaweb::ADMIN_PAGE]
        .iter()
        .flat_map(|p| inline_script_hashes(p))
        .collect();
    assert_eq!(want.len(), 2);
    let csp = login.csp();
    assert!(
        csp.contains(&format!("script-src 'self' {} {};", want[0], want[1])),
        "{csp}"
    );
    assert!(!csp
        .split(';')
        .any(|d| d.trim().starts_with("script-src") && d.contains("unsafe-inline")));
    // a script that reached a response body is not allowed by it
    let injected = Resp::new(200, HTML_TYPE, "<p>x</p><script>alert(1)</script>".into());
    for h in inline_script_hashes(&injected.body) {
        assert!(!injected.csp().contains(&h));
    }
    let js = t.get("/admin-web.js", None);
    assert_eq!(
        (js.status, js.ctype),
        (200, "text/javascript; charset=utf-8")
    );
    // the dashboard's and the beta page's served scripts are exactly the templates' ones
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    let s = t.login(&mut k);
    let ops = t.get("/ops", Some(&s));
    assert_eq!(inline_script_hashes(&ops.body), vec![want[0].clone()]);
    let beta = t.get("/ops/beta", Some(&s));
    assert!(
        beta.body.contains(&format!("content=\"{}\"", s.csrf)),
        "the beta page carries the session's token"
    );
    assert_eq!(inline_script_hashes(&beta.body), vec![want[1].clone()]);
}
#[test]
fn enrolment_refuses_keys_that_are_not_attested_yubikeys_and_keeps_the_link() {
    let t = setup();
    let token = t.link("");
    let mut k = SoftKey::new(&t.ca);
    k.fmt = "none";
    let (st, v) = t.enroll_with(&mut k, &token, "");
    assert_eq!(st, 400);
    assert!(
        v["message"].as_str().unwrap().contains("no attestation"),
        "{v}"
    );
    let mut k = SoftKey::new(&t.ca);
    k.self_attest = true;
    let (st, v) = t.enroll_with(&mut k, &token, "");
    assert_eq!(st, 400);
    assert!(
        v["message"].as_str().unwrap().contains("self attestation"),
        "{v}"
    );
    // a packed attestation from a root that is not trusted
    let stranger = TestCa::new("Some Other Vendor Root");
    let mut k = SoftKey::new(&stranger);
    let (st, v) = t.enroll_with(&mut k, &token, "");
    assert_eq!(st, 400);
    assert!(
        v["message"].as_str().unwrap().contains("Yubico root"),
        "{v}"
    );
    assert_eq!(t.key_count(), 0);
    // the link was not used up by the refusals; a real key still enrols with it, once
    let mut good = SoftKey::new(&t.ca);
    assert_eq!(t.enroll_with(&mut good, &token, "").0, 200);
    assert_eq!(t.key_count(), 1);
    // the refusals are in the problem events
    let n: i64 = crate::net::lock(&t.sh)
        .db
        .query_row(
            "SELECT COUNT(*) FROM ops_events WHERE kind = 'admin_web' AND problem = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(n >= 3);
}

#[test]
fn enrolment_links_expire_and_wrong_ones_are_refused() {
    let mut t = setup();
    let token = t.link("");
    let mut k = SoftKey::new(&t.ca);
    assert_eq!(t.enroll_with(&mut k, "not-a-token", "").0, 403);
    t.now += ENROLL_SECS + 1;
    assert_eq!(t.enroll_with(&mut k, &token, "").0, 403);
    assert_eq!(t.key_count(), 0);
}

#[test]
fn sign_in_refuses_wrong_origin_rp_id_reused_challenge_missing_uv_and_counter_regression() {
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    let bad = |out: &Resp, what: &str| {
        assert_eq!(out.status, 403, "{what}: {}", out.body);
        assert!(!out.headers.contains("Set-Cookie"), "{what}");
    };
    let (out, _) = t.login_with(&mut k, &|k, ch| k.get(RP, "https://evil.example.com", ch));
    bad(&out, "origin");
    let (out, _) = t.login_with(&mut k, &|k, ch| k.get("evil.example.com", ORIGIN, ch));
    bad(&out, "rp id");
    let (out, _) = t.login_with(&mut k, &|k, ch| {
        k.flags = webauthn::FLAG_UP;
        let a = k.get(RP, ORIGIN, ch);
        k.flags = webauthn::FLAG_UP | webauthn::FLAG_UV;
        a
    });
    bad(&out, "uv");
    assert!(out.body.contains("PIN"));
    // a good answer works once; the same ceremony again is refused
    let (out, body) = t.login_with(&mut k, &|k, ch| k.get(RP, ORIGIN, ch));
    assert_eq!(out.status, 200);
    let again = t.send(&req("POST", "/auth/login/finish"), &body);
    assert_eq!(again.status, 400);
    assert!(again.body.contains("expired or was already used"));
    // an answer to another (older) challenge
    let (out, _) = t.login_with(&mut k, &|k, _| {
        k.get(RP, ORIGIN, b"an old challenge of another ceremony")
    });
    bad(&out, "challenge");
    // a counter that went back (a cloned key)
    let (out, _) = t.login_with(&mut k, &|k, ch| {
        k.counter = 0;
        k.get(RP, ORIGIN, ch)
    });
    bad(&out, "counter");
    assert!(out.body.contains("counter"));
    // an expired ceremony
    let mut t2 = setup();
    let mut k2 = SoftKey::new(&t2.ca);
    t2.enroll(&mut k2, "a");
    let (_, o) = t2.post("/auth/login/options", None, json!({}));
    let ch = b64::decode(o["publicKey"]["challenge"].as_str().unwrap()).unwrap();
    let (cdj, ad, sig) = k2.get(RP, ORIGIN, &ch);
    t2.now += CEREMONY_SECS + 1;
    let (st, _) = t2.post(
        "/auth/login/finish",
        None,
        json!({"ceremony": o["ceremony"], "credential": get_json(&k2, &cdj, &ad, &sig)}),
    );
    assert_eq!(st, 400);
    // a key that is not enrolled
    let mut stranger = SoftKey::new(&t.ca);
    let (out, _) = t.login_with(&mut stranger, &|k, ch| k.get(RP, ORIGIN, ch));
    bad(&out, "unknown key");
}

#[test]
fn sessions_end_after_the_idle_and_the_absolute_limit() {
    let mut t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    let s = t.login(&mut k);
    t.now += 29 * 60;
    assert_eq!(t.get("/ops/live", Some(&s)).status, 200);
    t.now += 31 * 60;
    assert_eq!(t.get("/ops/live", Some(&s)).status, 401, "idle 31 minutes");
    assert_eq!(t.get("/ops", Some(&s)).status, 303);
    // a session kept busy still ends after 12 hours
    let s = t.login(&mut k);
    let start = t.now;
    while t.now - start < 12 * 3600 - 20 * 60 {
        t.now += 20 * 60;
        assert_eq!(t.get("/ops/live", Some(&s)).status, 200);
    }
    t.now = start + 12 * 3600 + 1;
    assert_eq!(t.get("/ops/live", Some(&s)).status, 401, "absolute limit");
}

#[test]
fn requests_without_a_session_get_nothing_of_the_dashboard() {
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    for p in ["/ops", "/ops/beta", "/keys"] {
        let r = t.get(p, None);
        assert_eq!(
            (r.status, r.headers.as_str()),
            (303, "Location: /login\r\n"),
            "{p}"
        );
    }
    for p in [
        "/ops/live",
        "/ops/events",
        "/ops/matches",
        "/ops/alerts",
        "/ops/beta/list",
        "/keys/list",
        "/status",
        "/net",
    ] {
        let r = t.get(p, None);
        assert_eq!(r.status, 401, "{p}");
        assert!(!r.body.contains("rooms"), "{p}");
    }
    for p in [
        "/ops/beta/invite",
        "/ops/beta/delete",
        "/stop",
        "/keys/remove",
        "/auth/stepup/options",
        "/logout",
    ] {
        assert_eq!(t.post(p, None, json!({"ids": [1]})).0, 401, "{p}");
    }
    // a made-up cookie, the loopback admin token, a loopback peer: none of them is a way in
    let mut r = req("GET", "/ops/live");
    r.cookie = Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    r.extra = "X-Admin-Token: s3cret\r\nAuthorization: Bearer s3cret\r\n";
    assert_eq!(t.send(&r, &Value::Null).status, 401);
    let mut r = req("GET", "/ops/live?token=s3cret");
    r.host = "127.0.0.1:10096";
    assert_eq!(t.send(&r, &Value::Null).status, 421);
    // the admin port's own endpoints are not served even with a session
    let s = t.login(&mut k);
    assert_eq!(t.get("/status", Some(&s)).status, 404);
    assert_eq!(t.post("/stop", Some(&s), json!({})).0, 404);
    assert_eq!(t.post("/rooms/1-1/close", Some(&s), json!({})).0, 404);
}

#[test]
fn every_request_must_name_the_configured_host() {
    let t = setup();
    for h in [
        "evil.example.com",
        "admin.example.com.evil.example",
        "127.0.0.1",
        "",
    ] {
        let mut r = req("GET", "/login");
        r.host = h;
        assert_eq!(t.send(&r, &Value::Null).status, 421, "{h:?}");
    }
    let mut r = req("GET", "/login");
    r.host = "ADMIN.example.com";
    assert_eq!(t.send(&r, &Value::Null).status, 200);
}

#[test]
fn posts_need_json_the_own_origin_and_the_session_token() {
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    let s = t.login(&mut k);
    let id = keys(&crate::net::lock(&t.sh).db).unwrap()[0].id;
    let rename = |cookie: Option<&str>, csrf: Option<&str>, origin: &str, ctype: &str| {
        let mut r = req("POST", "/keys/rename");
        r.cookie = cookie;
        r.csrf = csrf;
        r.origin = origin;
        r.ctype = ctype;
        t.send(&r, &json!({"id": id, "label": "renamed"})).status
    };
    assert_eq!(
        rename(Some(&s.cookie), None, ORIGIN, "application/json"),
        403,
        "no CSRF token"
    );
    assert_eq!(
        rename(Some(&s.cookie), Some("wrong"), ORIGIN, "application/json"),
        403,
        "wrong CSRF token"
    );
    assert_eq!(
        rename(
            Some(&s.cookie),
            Some(&s.csrf),
            "https://evil.example.com",
            "application/json"
        ),
        403,
        "origin"
    );
    assert_eq!(
        rename(Some(&s.cookie), Some(&s.csrf), "null", "application/json"),
        403,
        "null origin"
    );
    assert_eq!(
        rename(
            Some(&s.cookie),
            Some(&s.csrf),
            ORIGIN,
            "application/x-www-form-urlencoded"
        ),
        415,
        "a form post"
    );
    assert_eq!(
        rename(Some(&s.cookie), Some(&s.csrf), ORIGIN, "application/json"),
        200
    );
    assert_eq!(
        keys(&crate::net::lock(&t.sh).db).unwrap()[0].label,
        "renamed"
    );
    // the ceremonies before a session need the origin and JSON too
    let mut r = req("POST", "/auth/login/options");
    r.origin = "https://evil.example.com";
    assert_eq!(t.send(&r, &json!({})).status, 403);
    let mut r = req("POST", "/auth/login/options");
    r.ctype = "text/plain";
    assert_eq!(t.send(&r, &json!({})).status, 415);
    // the beta page's writes: the session's token in X-Beta-CSRF (the beta list itself then answers)
    let beta = |tok: &str| {
        let mut r = req("POST", "/ops/beta/note");
        r.cookie = Some(&s.cookie);
        r.extra = Box::leak(format!("X-Beta-CSRF: {tok}\r\n").into_boxed_str());
        t.send(&r, &json!({"id": 999, "note": "x"}))
    };
    let ok = beta(&s.csrf);
    assert_eq!(ok.status, 400, "{}", ok.body);
    assert!(ok.body.contains("no such signup"));
    let loopback_token = {
        let mut g = crate::net::lock(&t.sh);
        let st = &mut *g;
        st.beta.csrf(&mut st.keys)
    };
    assert_eq!(
        beta(&loopback_token).status,
        403,
        "the loopback page's token is not this session's"
    );
}

#[test]
fn keys_are_added_and_removed_after_a_fresh_confirmation_but_never_the_last() {
    let t = setup();
    let mut a = SoftKey::new(&t.ca);
    t.enroll(&mut a, "first");
    let s = t.login(&mut a);
    let first = keys(&crate::net::lock(&t.sh).db).unwrap()[0].id;
    // remove-last-key refused, even after a confirmation
    assert_eq!(
        t.post("/keys/remove", Some(&s), json!({"id": first})).0,
        403,
        "no confirmation yet"
    );
    assert_eq!(t.step_up(&mut a, &s), 200);
    let (st, v) = t.post("/keys/remove", Some(&s), json!({"id": first}));
    assert_eq!(st, 409, "{v}");
    assert!(v["message"].as_str().unwrap().contains("last key"));
    assert_eq!(t.key_count(), 1);
    // adding a key needs a confirmation too
    let s2 = t.login(&mut a);
    assert_eq!(t.post("/keys/enroll/options", Some(&s2), json!({})).0, 403);
    assert_eq!(t.step_up(&mut a, &s2), 200);
    let (st, o) = t.post(
        "/keys/enroll/options",
        Some(&s2),
        json!({"label": "second"}),
    );
    assert_eq!(st, 200, "{o}");
    assert_eq!(
        o["publicKey"]["excludeCredentials"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut b = SoftKey::new_eddsa(&t.ca);
    let ch = b64::decode(o["publicKey"]["challenge"].as_str().unwrap()).unwrap();
    let (cdj, ao) = b.create(RP, ORIGIN, &ch);
    let (st, v) = t.post(
        "/keys/enroll/finish",
        Some(&s2),
        json!({"ceremony": o["ceremony"], "label": "second", "credential": att_json(&b, &cdj, &ao)}),
    );
    assert_eq!(st, 200, "{v}");
    assert_eq!(t.key_count(), 2);
    // the confirmation was used up by that key
    assert_eq!(t.post("/keys/enroll/options", Some(&s2), json!({})).0, 403);
    // the new key signs in; the list shows both
    let sb = t.login(&mut b);
    let list: Value = serde_json::from_str(&t.get("/keys/list", Some(&sb)).body).unwrap();
    assert_eq!(list["keys"].as_array().unwrap().len(), 2);
    assert_eq!(list["keys"][1]["alg"], "EdDSA");
    assert_eq!(list["keys"][1]["current"], true);
    // removing the first key (now not the last) ends its sessions, not the others
    assert_eq!(t.step_up(&mut b, &sb), 200);
    let (st, v) = t.post("/keys/remove", Some(&sb), json!({"id": first}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["signed_out"], false);
    assert_eq!(
        t.get("/ops/live", Some(&s2)).status,
        401,
        "a session of the removed key ends"
    );
    assert_eq!(t.get("/ops/live", Some(&sb)).status, 200);
    assert_eq!(
        t.post("/auth/login/options", None, json!({})).1["publicKey"]["allowCredentials"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // and the removed key cannot sign in again
    let (out, _) = t.login_with(&mut a, &|k, ch| k.get(RP, ORIGIN, ch));
    assert_eq!(out.status, 403);
}

#[test]
fn max_keys_is_enforced_at_the_link_and_at_the_end() {
    let t = setup_with(|c| c.max_keys = 1);
    let mut a = SoftKey::new(&t.ca);
    let token = t.link("");
    let (_, o) = t.post("/auth/enroll/options", None, json!({"token": token}));
    t.enroll(&mut a, "a");
    // a ceremony opened before the first key was stored cannot add a second one
    let mut b = SoftKey::new(&t.ca);
    let ch = b64::decode(o["publicKey"]["challenge"].as_str().unwrap()).unwrap();
    let (cdj, ao) = b.create(RP, ORIGIN, &ch);
    let (st, _) = t.post(
        "/auth/enroll/finish",
        None,
        json!({"ceremony": o["ceremony"], "token": token, "credential": att_json(&b, &cdj, &ao)}),
    );
    assert_eq!(st, 409);
    let token2 = t.link("");
    assert_eq!(
        t.post("/auth/enroll/options", None, json!({"token": token2}))
            .0,
        409
    );
    assert_eq!(t.key_count(), 1);
}

#[test]
fn attempts_are_limited_per_client_address_and_in_all() {
    let mut t = setup_with(|c| {
        c.attempts_per_ip = 4;
        c.attempts_global = 5;
    });
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a"); // one challenge
    for _ in 0..2 {
        assert_eq!(t.post("/auth/login/options", None, json!({})).0, 200);
    }
    // a refused answer counts against its address
    assert_eq!(
        t.post("/auth/login/finish", None, json!({"ceremony": "nope"}))
            .0,
        400
    );
    let (st, _) = t.post("/auth/login/options", None, json!({}));
    assert_eq!(st, 429);
    // and over the limit even an answer is not looked at
    assert_eq!(
        t.post("/auth/login/finish", None, json!({"ceremony": "nope"}))
            .0,
        429
    );
    // other addresses get challenges until the global count of issued challenges is reached
    t.ip = "198.51.100.7".into();
    assert_eq!(t.post("/auth/login/options", None, json!({})).0, 200);
    t.ip = "198.51.100.8".into();
    assert_eq!(t.post("/auth/login/options", None, json!({})).0, 200);
    t.ip = "203.0.113.9".into();
    assert_eq!(t.post("/auth/login/options", None, json!({})).0, 429);
    // ten minutes later the window is empty again
    t.now += WINDOW_SECS;
    assert_eq!(t.post("/auth/login/options", None, json!({})).0, 200);
}

#[test]
fn junk_answers_from_one_address_do_not_lock_out_the_others() {
    let mut t = setup_with(|c| {
        c.attempts_per_ip = 10;
        c.attempts_global = 60;
    });
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    t.ip = "192.0.2.66".into();
    let mut codes = std::collections::BTreeMap::new();
    for path in ["/auth/login/finish", "/auth/enroll/finish"] {
        for _ in 0..100 {
            *codes
                .entry(t.post(path, None, json!({"ceremony": "x"})).0)
                .or_insert(0) += 1;
        }
    }
    // the first ten are refused one by one, the rest are cut off before any work
    assert_eq!(codes.get(&400), Some(&10), "{codes:?}");
    assert_eq!(codes.get(&429), Some(&190), "{codes:?}");
    // the session answers are limited the same way
    t.ip = "192.0.2.10".into();
    let s = t.login(&mut k);
    t.ip = "192.0.2.66".into();
    assert_eq!(
        t.post("/auth/stepup/finish", Some(&s), json!({"ceremony": "x"}))
            .0,
        429
    );
    assert_eq!(
        t.post("/keys/enroll/finish", Some(&s), json!({"ceremony": "x"}))
            .0,
        429
    );
    // another address still signs in
    t.ip = "198.51.100.20".into();
    assert_eq!(t.post("/auth/login/options", None, json!({})).0, 200);
    let s2 = t.login(&mut k);
    assert_eq!(t.get("/ops/live", Some(&s2)).status, 200);
}

#[test]
fn a_flood_writes_a_bounded_number_of_event_rows() {
    let mut t = setup_with(|c| {
        c.attempts_per_ip = 10;
        c.attempts_global = 60;
    });
    let rows = |t: &T| -> i64 {
        crate::net::lock(&t.sh)
            .db
            .query_row(
                "SELECT COUNT(*) FROM ops_events WHERE kind = 'admin_web'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let before = rows(&t);
    for i in 0..20 {
        t.ip = format!("192.0.2.{}", 100 + i);
        for _ in 0..50 {
            t.post("/auth/login/finish", None, json!({"ceremony": "x"}));
            t.post("/auth/login/options", None, json!({}));
        }
    }
    // per address: 10 refusals and one limit line, whatever the number of requests
    let n = rows(&t) - before;
    assert!(n <= 20 * 11, "{n} rows");
    let last: String = crate::net::lock(&t.sh)
        .db
        .query_row(
            "SELECT detail FROM ops_events WHERE kind = 'admin_web' AND detail LIKE 'attempt limit%' ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(last.contains("were not logged"), "{last}");
}

#[test]
fn the_address_table_stays_bounded() {
    let mut rt = Runtime::new(
        AdminWebSection::default(),
        Trust {
            roots: vec![],
            intermediates: vec![],
        },
    );
    for i in 0..(MAX_IPS + 500) {
        let ip = format!("10.{}.{}.{}", i / 65536, (i / 256) % 256, i % 256);
        assert!(rt.note_refusal(&ip, T0));
    }
    assert!(rt.per_ip.len() <= MAX_IPS);
    for _ in 0..1000 {
        rt.note_refusal("192.0.2.1", T0);
    }
    assert_eq!(
        rt.per_ip["192.0.2.1"].hits.len(),
        10,
        "a queue never grows past the limit"
    );
}
#[test]
fn the_loopback_admin_port_is_unchanged() {
    let t = setup();
    let get = |p: &str, host: &str, peer, token: Option<&str>, ok| {
        crate::opsweb::handle(
            &t.sh,
            &format!("GET {p} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
            peer,
            token,
            ok,
        )
        .unwrap()
    };
    assert_eq!(get("/ops/live", "127.0.0.1:10090", LO, None, true).0, 200);
    assert_eq!(
        get("/ops/live", HOST, LO, None, true).0,
        403,
        "the admin web's name is not a loopback name"
    );
    assert_eq!(
        get(
            "/ops/live",
            "127.0.0.1",
            Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 4))),
            None,
            true
        )
        .0,
        403
    );
    assert_eq!(get("/ops/live", "127.0.0.1", LO, Some("t"), false).0, 401);
    // its beta page still carries its own per-process token and checks http://<Host>
    let page = get("/ops/beta", "127.0.0.1:10090", LO, None, true);
    let tok = {
        let mut g = crate::net::lock(&t.sh);
        let st = &mut *g;
        st.beta.csrf(&mut st.keys)
    };
    assert!(page.2.contains(&tok));
    let post = |origin: &str, tok: &str| {
        crate::opsweb::handle_with_body(
            &t.sh,
            &format!(
                "POST /ops/beta/note HTTP/1.1\r\nHost: 127.0.0.1:10090\r\nOrigin: {origin}\r\n\
                 Content-Type: application/json\r\nX-Beta-CSRF: {tok}\r\n\r\n"
            ),
            br#"{"id": 999, "note": "x"}"#,
            LO,
            None,
            true,
        )
        .unwrap()
        .0
    };
    assert_eq!(
        post("http://127.0.0.1:10090", &tok),
        400,
        "guard passed, no such signup"
    );
    assert_eq!(post(ORIGIN, &tok), 403);
    assert_eq!(post("http://127.0.0.1:10090", "wrong"), 403);
    // and the admin endpoint itself
    assert_eq!(
        crate::admin::handle(&t.sh, "GET /status HTTP/1.1\r\n\r\n", LO, None).0,
        200
    );
}

#[test]
fn bind_takes_loopback_addresses_only() {
    let mut cfg = AdminWebSection {
        bind: "0.0.0.0:0".into(),
        ..Default::default()
    };
    assert!(bind(&cfg).unwrap_err().contains("loopback"));
    cfg.bind = "nonsense".into();
    assert!(bind(&cfg).is_err());
    cfg.bind = "127.0.0.1:0".into();
    assert!(bind(&cfg).is_ok());
}

#[test]
fn the_listener_answers_over_tcp() {
    use std::io::Read;
    let t = setup();
    let cfg = lock(&t.web).cfg.clone();
    let (addr, l) = bind(&AdminWebSection {
        bind: "127.0.0.1:0".into(),
        ..cfg
    })
    .unwrap();
    let addr = l.local_addr().unwrap_or(addr);
    spawn(t.sh.clone(), t.web.clone(), l);
    let ask = |raw: &str| {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    };
    let page = ask(&format!("GET /login HTTP/1.1\r\nHost: {HOST}\r\n\r\n"));
    assert!(page.starts_with("HTTP/1.1 200 OK\r\n"), "{page}");
    assert!(
        page.contains("Content-Security-Policy: default-src 'none'; script-src 'self' 'sha256-")
    );
    let body = "{}";
    let post = ask(&format!(
        "POST /auth/login/options HTTP/1.1\r\nHost: {HOST}\r\nOrigin: {ORIGIN}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    ));
    assert!(post.starts_with("HTTP/1.1 409 "), "no key yet: {post}");
    let big = ask(&format!(
        "POST /auth/login/options HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: {}\r\n\r\n",
        MAX_BODY + 1
    ));
    assert!(big.starts_with("HTTP/1.1 413 "), "{big}");
}

#[test]
fn the_command_line_splits_its_flags_from_the_lobby_flags() {
    let a =
        split_args(&["keys", "remove", "3", "--force", "--db", "x.db"].map(String::from)).unwrap();
    assert_eq!(a.words, vec!["keys", "remove", "3"]);
    assert!(a.force);
    assert_eq!(a.rest, vec!["--db", "x.db"]);
    let a = split_args(&["enroll", "--label=blue key", "--config", "s.toml"].map(String::from))
        .unwrap();
    assert_eq!(
        (a.words.as_slice(), a.label.as_str()),
        (&["enroll".to_string()][..], "blue key")
    );
    assert_eq!(a.rest, vec!["--config", "s.toml"]);
    assert!(split_args(&["enroll".to_string(), "--label".to_string()]).is_err());
}

#[test]
fn the_command_line_makes_links_lists_and_keeps_the_last_key() {
    let dir = std::env::temp_dir().join(format!("dc_adminweb_cli_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let db = dir.join("ca.db");
    let _ = std::fs::remove_file(&db);
    let dbs = db.to_string_lossy().to_string();
    let run = |args: &[&str]| run_cli(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &[]);
    assert_eq!(run(&["enroll", "--label", "blue", "--db", &dbs]), 0);
    assert_eq!(run(&["keys", "list", "--db", &dbs]), 0);
    assert_eq!(
        run(&["keys", "remove", "1", "--db", &dbs]),
        1,
        "no such key"
    );
    assert_eq!(run(&["bogus"]), 2);
    // one key in the DB: removing it needs --force
    let c = crate::db::open(&db).unwrap();
    let open: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM admin_web_enroll WHERE used = 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(open, 1);
    let ca = TestCa::new("Test FIDO Root");
    let mut k = SoftKey::new(&ca);
    let (cdj, ao) = k.create(RP, ORIGIN, b"challenge");
    let e = Expect {
        rp_id: RP,
        origin: ORIGIN,
        challenge: b"challenge",
    };
    let nc = webauthn::verify_registration(&cdj, &ao, &e, &ca.trust(), T0).unwrap();
    let id = add_key(&c, &nc, "blue", "", T0).unwrap();
    drop(c);
    assert_eq!(run(&["keys", "remove", &id.to_string(), "--db", &dbs]), 1);
    assert_eq!(
        run(&["keys", "remove", &id.to_string(), "--force", "--db", &dbs]),
        0
    );
    let c = crate::db::open(&db).unwrap();
    assert!(keys(&c).unwrap().is_empty());
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_trickled_request_is_dropped_at_the_deadline() {
    use std::io::Read;
    let t = setup();
    let cfg = lock(&t.web).cfg.clone();
    let (_, l) = bind(&AdminWebSection {
        bind: "127.0.0.1:0".into(),
        ..cfg
    })
    .unwrap();
    let addr = l.local_addr().unwrap();
    spawn_with(t.sh.clone(), t.web.clone(), l, Duration::from_millis(600));
    let mut s = TcpStream::connect(addr).unwrap();
    let mut w = s.try_clone().unwrap();
    // one byte every 100 ms: no single read waits long, the request never ends
    let writer = std::thread::spawn(move || {
        for b in format!(
            "GET /login HTTP/1.1\r\nHost: {HOST}\r\nX-Slow: {}",
            "a".repeat(60)
        )
        .bytes()
        {
            if w.write_all(&[b]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    let start = Instant::now();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let took = start.elapsed();
    assert!(
        took < Duration::from_secs(3),
        "the server held the slot for {took:?}"
    );
    assert!(out.is_empty(), "no answer to an incomplete request");
    drop(s);
    let _ = writer.join();
}

#[test]
fn script_hashes_follow_the_browser_newline_rule() {
    // a browser hashes the script after CR LF / CR -> LF; a CRLF checkout of the templates must give the same hashes
    let lf = "<p>x</p><script>\nlet a = 1;\nlet b = 2;\n</script>";
    let crlf = lf.replace('\n', "\r\n");
    let cr = lf.replace('\n', "\r");
    let want = format!(
        "'sha256-{}'",
        b64::std(&webauthn::sha256("\nlet a = 1;\nlet b = 2;\n".as_bytes()))
    );
    assert_eq!(inline_script_hashes(lf), vec![want.clone()]);
    assert_eq!(inline_script_hashes(&crlf), vec![want.clone()]);
    assert_eq!(inline_script_hashes(&cr), vec![want]);
}

// ------------------------------------------------------------------------------------------------- LG-399 / LG-400

/// A second lobby (its own state and DB) with its loopback admin port, as the modded instance runs it.
fn instance_port(token: Option<&str>) -> (Shared, String) {
    let sh: Shared = Arc::new(Mutex::new(crate::state::testing::state()));
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    crate::admin::spawn(sh.clone(), l, token.map(str::to_string));
    (sh, addr)
}

fn inst(name: &str, label: &str, admin: &str, env_file: &str) -> dc_config::AdminWebInstance {
    dc_config::AdminWebInstance {
        name: name.into(),
        label: label.into(),
        admin: admin.into(),
        env_file: env_file.into(),
    }
}

#[test]
fn a_further_instance_is_served_under_its_prefix_behind_the_same_session() {
    let (ish, iaddr) = instance_port(Some("inst-s3cret"));
    let env = std::env::temp_dir().join(format!("dc_adminfwd_env_{}", std::process::id()));
    std::fs::write(
        &env,
        "DC_ADMIN=127.0.0.1:10091\nexport DC_ADMIN_TOKEN=\"inst-s3cret\"\n",
    )
    .unwrap();
    let env_s = env.to_string_lossy().to_string();
    // a closed port: bind one and drop it
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let t = setup_with(|c| {
        c.label = "Vanilla lobby".into();
        c.instances = vec![
            inst("modded", "Hacking Mod lobby", &iaddr, &env_s),
            inst("notoken", "No token", &iaddr, ""),
            inst("down", "Down", &closed, ""),
        ];
    });
    let acc = {
        let mut g = ish.lock().unwrap();
        let st = &mut *g;
        let a = crate::db::create_account(&st.db, "modonly", 0).unwrap();
        st.scope(|st| crate::auth::create_character(st, a, "ModOnly", 2))
            .unwrap();
        a
    };
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");

    // no session: the pages go to the sign-in, everything else is 401; nothing reaches the instance
    for p in ["/i/modded/ops", "/i/modded/ops/beta"] {
        let r = t.get(p, None);
        assert_eq!(
            (r.status, r.headers.as_str()),
            (303, "Location: /login\r\n"),
            "{p}"
        );
    }
    assert_eq!(t.get("/i/modded/ops/live", None).status, 401);
    assert_eq!(
        t.post(
            "/i/modded/ops/player/disable",
            None,
            json!({"account": acc})
        )
        .0,
        401
    );

    let s = t.login(&mut k);
    let tgt_token = {
        let mut g = ish.lock().unwrap();
        let st = &mut *g;
        st.beta.csrf(&mut st.keys)
    };
    // the page: the session's token in place of the instance's, the switcher, the lobby named, no Updates tab
    let page = t.get("/i/modded/ops", Some(&s));
    assert_eq!(page.status, 200, "{}", page.body);
    assert!(page.ctype.starts_with("text/html"));
    assert!(page
        .body
        .contains(&format!("<meta name=\"csrf\" content=\"{}\">", s.csrf)));
    assert!(
        !page.body.contains(&tgt_token),
        "the instance's page token never reaches the browser"
    );
    assert!(!page.body.contains("inst-s3cret"));
    assert!(page.body.contains("DeCombat <span>operations</span>"));
    assert!(page
        .body
        .contains("<select data-inst aria-label=\"Lobby\">"));
    assert!(page
        .body
        .contains("<option value=\"/ops\">Vanilla lobby</option>"));
    assert!(page
        .body
        .contains("<option value=\"/i/modded/ops\" selected>Hacking Mod lobby</option>"));
    assert!(page.body.contains("<body class=\"other-lobby\">"));
    assert!(!page.body.contains("href=\"#updates\""));
    assert!(page.body.contains("<a href=\"/keys\">Security keys</a>"));
    assert!(page
        .body
        .contains("<title>Hacking Mod lobby | DeCombat ops</title>"));
    let beta = t.get("/i/modded/ops/beta", Some(&s));
    assert_eq!(beta.status, 200);
    assert!(beta.body.contains(&s.csrf) && !beta.body.contains(&tgt_token));
    assert!(beta
        .body
        .contains("<option value=\"/i/modded/ops/beta\" selected>"));
    // the inline scripts are the templates' own, so the CSP hashes still allow them
    let csp = page.csp();
    for h in inline_script_hashes(&page.body) {
        assert!(csp.contains(&h), "{h}");
    }
    // the default lobby's page names itself and keeps its Updates tab
    let own = t.get("/ops", Some(&s));
    assert!(own
        .body
        .contains("<title>Vanilla lobby | DeCombat ops</title>"));
    assert!(own
        .body
        .contains("<option value=\"/ops\" selected>Vanilla lobby</option>"));
    assert!(
        own.body.contains("href=\"#updates\"")
            && !own.body.contains("<body class=\"other-lobby\">")
    );

    // the JSON is the instance's: its account is there and not in the default lobby
    let tl = t.get("/i/modded/ops/timeline?name=modonly", Some(&s));
    assert_eq!(tl.status, 200, "{}", tl.body);
    assert_eq!(t.get("/ops/timeline?name=modonly", Some(&s)).status, 404);
    assert_eq!(t.get("/i/modded/ops/live", Some(&s)).status, 200);
    let players = t.get("/i/modded/ops/players?q=modonly", Some(&s));
    assert!(players.body.contains("ModOnly"), "{}", players.body);

    // the instance's own guard is satisfied by the forwarder: without the token from its env file it says 401
    assert_eq!(t.get("/i/notoken/ops/live", Some(&s)).status, 401);
    assert_eq!(t.get("/i/down/ops/live", Some(&s)).status, 502);
    // only configured instances, only /ops paths, no deploys, no path tricks
    assert_eq!(t.get("/i/other/ops/live", Some(&s)).status, 404);
    assert_eq!(t.get("/i/modded/status", Some(&s)).status, 404);
    assert_eq!(t.get("/i/modded/keys", Some(&s)).status, 404);
    assert_eq!(t.get("/i/modded/opsx", Some(&s)).status, 404);
    assert_eq!(t.get("/i/modded/ops/../status", Some(&s)).status, 400);
    assert_eq!(t.get("/i/modded/ops/%2e%2e/status", Some(&s)).status, 400);
    assert_eq!(t.get("/i/modded/ops/updates", Some(&s)).status, 403);
    let (st, v) = t.post(
        "/i/modded/ops/updates/deploy",
        Some(&s),
        json!({"sha": "abcdef1"}),
    );
    assert_eq!(st, 403, "{v}");
    assert!(v["message"].as_str().unwrap().contains("Vanilla lobby"));
    let r = t.get("/i/modded", Some(&s));
    assert_eq!(
        (r.status, r.headers.as_str()),
        (303, "Location: /i/modded/ops\r\n")
    );

    // writes: the admin web's Origin and the session's token first, then the instance's own guard (passed by the
    // forwarder: "no such signup" is the beta list's answer after its guard)
    let beta_note = |tok: &str, origin: &str| {
        let mut r = req("POST", "/i/modded/ops/beta/note");
        r.cookie = Some(&s.cookie);
        r.origin = origin;
        r.extra = Box::leak(format!("X-Beta-CSRF: {tok}\r\n").into_boxed_str());
        t.send(&r, &json!({"id": 999, "note": "x"}))
    };
    assert_eq!(beta_note("wrong", ORIGIN).status, 403);
    assert_eq!(
        beta_note(&tgt_token, ORIGIN).status,
        403,
        "the instance's token is not the session's"
    );
    assert_eq!(beta_note(&s.csrf, "https://evil.example.com").status, 403);
    let ok = beta_note(&s.csrf, ORIGIN);
    assert_eq!(ok.status, 400, "{}", ok.body);
    assert!(ok.body.contains("no such signup"), "{}", ok.body);
    // an account write lands on the instance, named after the key and the client address
    let mut r = req("POST", "/i/modded/ops/player/disable");
    r.cookie = Some(&s.cookie);
    assert_eq!(
        t.send(&r, &json!({"account": acc})).status,
        403,
        "no page token"
    );
    let (st, v) = t.post(
        "/i/modded/ops/player/disable",
        Some(&s),
        json!({"account": acc}),
    );
    assert_eq!(st, 200, "{v}");
    let detail: String = ish
        .lock()
        .unwrap()
        .db
        .query_row(
            "SELECT detail FROM ops_events WHERE user_no = ?1 AND kind = 'admin' ORDER BY id DESC LIMIT 1",
            [acc as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        detail.contains("the admin web, key #1 from 192.0.2.10"),
        "{detail}"
    );
    assert!(!detail.contains("the admin port"), "{detail}");
    // and the default lobby's own events name the forwarded write
    let n: i64 =
        t.sh.lock()
            .unwrap()
            .db
            .query_row(
                "SELECT COUNT(*) FROM ops_events WHERE kind = 'admin_web' \
             AND detail LIKE '%POST /ops/player/disable on instance modded (answer 200)%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(n, 1);
    let _ = std::fs::remove_file(&env);
}

#[test]
fn the_pages_take_their_base_from_the_path() {
    // LG-400: every request of both pages goes through one helper below BASE, and BASE comes from location.pathname
    for (name, page) in [
        ("dashboard", crate::opsweb::PAGE),
        ("beta", crate::betaweb::ADMIN_PAGE),
    ] {
        assert!(
            page.contains(
                r#"const BASE = (location.pathname.match(/^\/i\/[a-z0-9][a-z0-9-]*(?=\/ops(\/|$))/) || [""])[0];"#
            ),
            "{name}"
        );
        let fetches = page.matches("fetch(").count();
        assert!(fetches >= 1, "{name}");
        assert_eq!(
            page.matches("fetch(BASE + path").count(),
            fetches,
            "{name}: a fetch without the base"
        );
        assert!(
            page.contains("<div class=\"mark\">DeCombat <span>operations</span></div>"),
            "{name}: the switcher's anchor"
        );
        assert!(page.contains(".inst select"), "{name}");
        assert!(
            page.contains("select[data-inst]"),
            "{name}: the switcher's change handler"
        );
    }
    assert!(crate::opsweb::PAGE
        .contains(r#"document.getElementById("beta-link").href = BASE + "/ops/beta""#));
    assert!(crate::betaweb::ADMIN_PAGE.contains("a.href = BASE + path"));
    assert!(crate::opsweb::PAGE.contains("<a href=\"#updates\">Updates</a>"));
}

// ---- LG-430: the Release card (release.rs) behind the admin web

/// A deploy folder with `release/requests/` and, when given, root's `release/status.json`.
fn release_dir(t: &T, tag: &str, status: Option<Value>) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("dc-release-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("release").join("requests")).unwrap();
    if let Some(s) = status {
        std::fs::write(d.join("release").join("status.json"), s.to_string()).unwrap();
    }
    crate::net::lock(&t.sh).opts.deploy_dir = Some(d.clone());
    d
}

fn release_status(phase: &str, commits: u64) -> Value {
    json!({"phase": phase, "updated": T0 - 120, "private_sha": "f451b21e0000000000000000000000000000beef",
           "public_sha": "9e10335a", "last_tag": "v0.5.0-beta6", "next_tag": "v0.6.0-beta7", "commits_since": commits,
           "ci": {"status": "completed", "conclusion": "success", "url": "https://github.com/argallo/DeCombat/actions/runs/42"},
           "release_url": "", "message": "CI green on 9e10335a", "log_tail": ["export ok", "push ok"], "ok": null, "by": "Owner key"})
}

fn request_files(d: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d.join("release").join("requests"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn release_events(t: &T) -> Vec<String> {
    let g = crate::net::lock(&t.sh);
    let mut q =
        g.db.prepare("SELECT detail FROM ops_events WHERE kind = 'release' ORDER BY id")
            .unwrap();
    let v = q.query_map([], |r| r.get(0)).unwrap().flatten().collect();
    v
}

#[test]
fn a_release_request_is_a_file_named_for_its_action_with_the_admins_name() {
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "Owner key");
    let s = t.login(&mut k);
    let d = release_dir(&t, "request", Some(release_status("idle", 3)));
    let (st, v) = t.post("/ops/updates/release/prepare", Some(&s), json!({}));
    assert_eq!(st, 200, "{v}");
    let name = format!("{T0}-prepare.json");
    assert_eq!(request_files(&d), vec![name.clone()]);
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(d.join("release").join("requests").join(&name)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        file,
        json!({"action": "prepare", "by": "Owner key", "at": T0})
    );
    assert_eq!(
        release_events(&t),
        vec!["release prepare requested by Owner key".to_string()]
    );
    let admin: i64 = crate::net::lock(&t.sh)
        .db
        .query_row(
            "SELECT COUNT(*) FROM ops_events WHERE kind = 'admin_web' AND detail LIKE 'release request by Owner key with key #%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(admin, 1, "the admin web names the key and the address too");
    // one request waits at a time; the page shows it and offers no second prepare
    assert_eq!(
        t.post("/ops/updates/release/check", Some(&s), json!({})).0,
        409
    );
    let v: Value = serde_json::from_str(&t.get("/ops/updates/release", Some(&s)).body).unwrap();
    assert_eq!(v["pending"], "prepare");
    assert_eq!(v["view"]["can_prepare"], false);
    // root took it; a check lands, and a second press does not add a file
    std::fs::remove_file(d.join("release").join("requests").join(&name)).unwrap();
    assert_eq!(
        t.post("/ops/updates/release/check", Some(&s), json!({})).0,
        200
    );
    let (st, v) = t.post("/ops/updates/release/check", Some(&s), json!({}));
    assert_eq!(
        (st, v["message"].as_str()),
        (200, Some("a check is already requested"))
    );
    assert_eq!(request_files(&d), vec![format!("{T0}-check.json")]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn release_requests_need_a_session_the_page_token_and_an_allowed_action() {
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    let s = t.login(&mut k);
    let d = release_dir(&t, "refused", Some(release_status("idle", 3)));
    let send = |path: &str, cookie: Option<&str>, csrf: Option<&str>, origin: &str, ctype: &str| {
        let mut r = req("POST", path);
        r.cookie = cookie;
        r.csrf = csrf;
        r.origin = origin;
        r.ctype = ctype;
        t.send(&r, &json!({})).status
    };
    let ok = |path: &str| {
        send(
            path,
            Some(&s.cookie),
            Some(&s.csrf),
            ORIGIN,
            "application/json",
        )
    };
    let p = "/ops/updates/release/check";
    let j = "application/json";
    assert_eq!(send(p, None, None, ORIGIN, j), 401, "no session");
    assert_eq!(send(p, Some(&s.cookie), None, ORIGIN, j), 403, "no token");
    assert_eq!(
        send(p, Some(&s.cookie), Some("wrong"), ORIGIN, j),
        403,
        "wrong token"
    );
    assert_eq!(
        send(
            p,
            Some(&s.cookie),
            Some(&s.csrf),
            "https://evil.example.com",
            j
        ),
        403,
        "another origin"
    );
    assert_eq!(
        send(p, Some(&s.cookie), Some(&s.csrf), ORIGIN, "text/plain"),
        415,
        "a form post"
    );
    assert_eq!(
        t.get("/ops/updates/release", None).status,
        401,
        "no state without a session"
    );
    // unknown actions, and actions the state does not allow
    for bad in ["deploy", "Tag", "check/x", "tag.json"] {
        assert_eq!(ok(&format!("/ops/updates/release/{bad}")), 404, "{bad}");
    }
    assert_eq!(
        ok("/ops/updates/release/tag"),
        409,
        "nothing is ready to tag"
    );
    let write =
        |st: Value| std::fs::write(d.join("release").join("status.json"), st.to_string()).unwrap();
    write(release_status("ci_running", 3));
    assert_eq!(
        ok("/ops/updates/release/prepare"),
        409,
        "a release is under way"
    );
    write(release_status("published", 0));
    assert_eq!(
        ok("/ops/updates/release/prepare"),
        409,
        "nothing new since the last tag"
    );
    assert!(
        request_files(&d).is_empty(),
        "no refused request leaves a file"
    );
    assert_eq!(release_events(&t).len(), 3, "the state refusals are events");
    // a further instance's prefix takes no release request (releases are for the whole box)
    assert_ne!(ok("/i/modded/ops/updates/release/check"), 200);
    // the loopback port: reading needs nothing more, a request needs its admin token
    let lo = |head: String, token: Option<&str>| {
        crate::opsweb::handle_with_body(&t.sh, &head, b"{}", LO, token, true)
            .unwrap()
            .0
    };
    let post = |tok: &str| {
        format!(
            "POST {p} HTTP/1.1\r\nHost: 127.0.0.1:10090\r\nContent-Type: application/json\r\nOrigin: http://127.0.0.1:10090\r\nX-Admin-CSRF: {tok}\r\n\r\n"
        )
    };
    let tok = {
        let mut g = crate::net::lock(&t.sh);
        let st = &mut *g;
        st.beta.csrf(&mut st.keys)
    };
    let get = "GET /ops/updates/release HTTP/1.1\r\nHost: 127.0.0.1:10090\r\n\r\n";
    assert_eq!(lo(get.into(), None), 200);
    assert_eq!(lo(post(&tok), None), 403, "no --admin-token");
    assert_eq!(lo(post("wrong"), Some("t")), 403, "the port's page token");
    assert_eq!(lo(post(&tok), Some("t")), 200);
    let files = request_files(&d);
    assert!(
        files.len() == 1 && files[0].ends_with("-check.json"),
        "{files:?}"
    );
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(d.join("release").join("requests").join(&files[0])).unwrap(),
    )
    .unwrap();
    assert_eq!(file["by"], "the admin port");
    let _ = std::fs::remove_dir_all(&d);
}

// LG-434: Sync packs behind the admin web: a session's POST writes the request with the admin web's name; without a
// session it is refused; the Updates JSON lists the sources
#[test]
fn sync_packs_behind_the_admin_web_needs_a_session_and_names_the_admin_web() {
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    let s = t.login(&mut k);
    let d = release_dir(&t, "packs", None);
    {
        let mut g = crate::net::lock(&t.sh);
        g.opts.pack_sources = vec![("argallo/DeCombat-mods".into(), "v1".into())];
        g.opts.mods_dir = Some(std::path::PathBuf::from("/opt/decombat/data/mods"));
    }
    std::fs::create_dir_all(d.join("requests")).unwrap();
    // no session: refused (401, sign in) before anything is written
    let (st, _) = t.post("/ops/updates/packs", None, json!({}));
    assert_eq!(st, 401);
    assert!(std::fs::read_dir(d.join("requests"))
        .unwrap()
        .next()
        .is_none());
    let v: Value = serde_json::from_str(&t.get("/ops/updates", Some(&s)).body).unwrap();
    assert_eq!(v["packs"]["sources"][0]["repo"], "argallo/DeCombat-mods");
    assert_eq!(v["packs"]["pending"], false);
    let (st, out) = t.post("/ops/updates/packs", Some(&s), json!({}));
    assert_eq!(st, 200, "{out}");
    let files: Vec<String> = std::fs::read_dir(d.join("requests"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        files.len() == 1 && files[0].starts_with("packs-"),
        "{files:?}"
    );
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("requests").join(&files[0])).unwrap())
            .unwrap();
    assert_eq!(file["by"], "the admin web");
    assert_eq!(file["sources"][0]["tag"], "v1");
    assert_eq!(file["mods_dir"], "/opt/decombat/data/mods");
    let v: Value = serde_json::from_str(&t.get("/ops/updates", Some(&s)).body).unwrap();
    assert_eq!(v["packs"]["pending"], true);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_release_state_is_read_checked_and_turned_into_the_cards_headline_and_buttons() {
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    let s = t.login(&mut k);
    let state = || -> Value {
        serde_json::from_str(&t.get("/ops/updates/release", Some(&s)).body).unwrap()
    };
    // no [deploy] dir, then no release folder, then no status file
    let v = state();
    assert_eq!(
        (v["set_up"].as_bool(), v["status"].is_null()),
        (Some(false), true)
    );
    assert!(v["message"].as_str().unwrap().contains("[deploy] dir"));
    let d = release_dir(&t, "state", None);
    std::fs::remove_dir_all(d.join("release")).unwrap();
    assert_eq!(state()["set_up"], false);
    std::fs::create_dir_all(d.join("release").join("requests")).unwrap();
    let v = state();
    assert_eq!(
        (v["set_up"].as_bool(), v["status"].is_null()),
        (Some(true), true)
    );
    assert!(v["message"].as_str().unwrap().contains("Check now"));
    // a ready release: the tag button, the CI link
    std::fs::write(
        d.join("release").join("status.json"),
        release_status("ready", 3).to_string(),
    )
    .unwrap();
    let v = state();
    assert_eq!(
        v["view"]["headline"],
        "Ready to tag v0.6.0-beta7 — CI green"
    );
    assert_eq!(v["view"]["level"], "good");
    assert_eq!(
        (
            v["view"]["can_tag"].as_bool(),
            v["view"]["can_prepare"].as_bool()
        ),
        (Some(true), Some(false))
    );
    assert_eq!(
        v["status"]["ci"]["url"]["link"],
        "https://github.com/argallo/DeCombat/actions/runs/42"
    );
    assert_eq!(
        v["status"]["private_sha"],
        "f451b21e0000000000000000000000000000beef"
    );
    // each phase's headline
    let head = |phase: &str, n: u64, msg: &str| {
        let mut st = release_status(phase, n);
        st["message"] = json!(msg);
        crate::release::view(&crate::release::clean_status(&st), None)
    };
    assert_eq!(
        head("idle", 0, "")["headline"],
        "Nothing new since v0.5.0-beta6"
    );
    let h = head("idle", 3, "");
    assert_eq!(
        h["headline"],
        "3 commits since v0.5.0-beta6 — ready to prepare"
    );
    assert_eq!(
        (h["can_prepare"].as_bool(), h["can_tag"].as_bool()),
        (Some(true), Some(false))
    );
    assert_eq!(
        head("idle", 1, "")["headline"],
        "1 commit since v0.5.0-beta6 — ready to prepare"
    );
    let h = head("ci_running", 3, "");
    assert_eq!(h["headline"], "Pushed 9e10335a, CI running");
    assert_eq!(
        (
            h["level"].as_str(),
            h["busy"].as_bool(),
            h["can_prepare"].as_bool()
        ),
        (Some("warn"), Some(true), Some(false))
    );
    assert_eq!(head("tagging", 3, "")["headline"], "Tagging v0.6.0-beta7…");
    let mut p = release_status("published", 0);
    p["last_tag"] = json!("v0.6.0-beta7");
    assert_eq!(
        crate::release::view(&crate::release::clean_status(&p), None)["headline"],
        "Published v0.6.0-beta7"
    );
    let h = head("failed", 2, "CI failed on 9e10335a");
    assert_eq!(
        (h["headline"].as_str(), h["level"].as_str()),
        (Some("Failed: CI failed on 9e10335a"), Some("bad"))
    );
    assert_eq!(
        h["can_prepare"], true,
        "a failed release can be prepared again"
    );
    // a waiting request turns the buttons off
    let c = crate::release::clean_status(&release_status("ready", 3));
    let h = crate::release::view(&c, Some("tag"));
    assert_eq!(
        (h["can_tag"].as_bool(), h["can_check"].as_bool()),
        (Some(false), Some(false))
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn untrusted_release_fields_are_cut_to_text_and_only_public_repository_urls_are_links() {
    let mut st = release_status("ready", 3);
    st["message"] = json!("<img src=x onerror=alert(1)>\u{7}\nline two");
    st["by"] = json!(42);
    st["private_sha"] = json!("not-a-sha");
    st["next_tag"] = json!("v1;rm -rf /");
    st["ci"]["url"] = json!("javascript:alert(1)");
    st["release_url"] = json!("https://github.com/argallo/DeCombat.evil.example/releases");
    st["log_tail"] = json!((0..100)
        .map(|i| format!("line {i}\u{1b}[31m"))
        .collect::<Vec<_>>());
    let c = crate::release::clean_status(&st);
    assert_eq!(
        c["message"], "<img src=x onerror=alert(1)>  line two",
        "kept as text, controls gone"
    );
    assert_eq!(c["by"], "");
    assert_eq!(c["private_sha"], "");
    assert_eq!(c["next_tag"], "");
    assert_eq!(
        c["ci"]["url"],
        json!({"link": null, "text": "javascript:alert(1)"})
    );
    assert!(c["release_url"]["link"].is_null());
    let tail = c["log_tail"].as_array().unwrap();
    assert_eq!((tail.len(), tail[0].as_str()), (40, Some("line 60 [31m")));
    assert_eq!(
        crate::release::view(&c, None)["can_tag"],
        false,
        "no tag name, no tag button"
    );
    let mut bad = release_status("<b>ready</b>", 3);
    bad["commits_since"] = json!(-4);
    let c = crate::release::clean_status(&bad);
    assert_eq!(
        (c["phase"].as_str(), c["commits_since"].is_null()),
        (Some("unknown"), true)
    );
    assert_eq!(crate::release::view(&c, None)["level"], "bad");
    for (u, ok) in [
        (
            "https://github.com/argallo/DeCombat/releases/tag/v0.6.0-beta7",
            true,
        ),
        (
            "https://github.com/argallo/DeCombat/actions/runs/42?check_suite_focus=true",
            true,
        ),
        ("https://github.com/argallo/DeCombat/", false),
        ("http://github.com/argallo/DeCombat/releases", false),
        ("https://github.com/argallo/DeCombatX/releases", false),
        (
            "https://github.com/argallo/DeCombat/\"onmouseover=alert(1)",
            false,
        ),
        ("https://github.com/argallo/DeCombat/a b", false),
        ("https://github.com/argallo/DeCombat/../../evil", false),
    ] {
        assert_eq!(crate::release::github_link(u).is_some(), ok, "{u}");
    }
    // the card puts every field in as text: the dashboard has no HTML sink
    let page = crate::opsweb::PAGE;
    assert!(page.contains("function releaseCard(r)"));
    for sink in [
        "innerHTML",
        "outerHTML",
        "insertAdjacentHTML",
        "document.write",
    ] {
        assert!(!page.contains(sink), "{sink}");
    }
    // a state file over its cap is not read
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    let s = t.login(&mut k);
    let mut big = release_status("ready", 3);
    big["message"] = json!("x".repeat(300 * 1024));
    let d = release_dir(&t, "big", Some(big));
    let v: Value = serde_json::from_str(&t.get("/ops/updates/release", Some(&s)).body).unwrap();
    assert!(v["status"].is_null());
    let _ = std::fs::remove_dir_all(&d);
}

// ---- OPS-26: the Bug reports card (triage.rs) behind the admin web

/// A deploy folder with `triage/` holding the given report lines (as the poller writes them), a state and a done.json.
fn triage_dir(t: &T, tag: &str, lines: &[String]) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("dc-triage-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let tr = d.join("triage");
    std::fs::create_dir_all(&tr).unwrap();
    std::fs::write(tr.join("reports.jsonl"), lines.join("\n") + "\n").unwrap();
    std::fs::write(
        tr.join("state.json"),
        json!({"last_id": "1010", "updated": T0 - 60, "fetched": 2, "ok": true, "message": "2 new reports"}).to_string(),
    )
    .unwrap();
    std::fs::write(tr.join("done.json"), r#"{"done": {}}"#).unwrap();
    crate::net::lock(&t.sh).opts.deploy_dir = Some(d.clone());
    d
}

fn report(id: u64, content: &str) -> String {
    json!({"id": id.to_string(), "at": T0 - 1000 + id as i64, "author": {"username": "alice", "name": "Alice A"},
           "content": content, "attachments": []})
    .to_string()
}

fn triage_events(t: &T) -> Vec<String> {
    let g = crate::net::lock(&t.sh);
    let mut q =
        g.db.prepare("SELECT detail FROM ops_events WHERE kind = 'triage' ORDER BY id")
            .unwrap();
    let v = q.query_map([], |r| r.get(0)).unwrap().flatten().collect();
    v
}

#[test]
fn bug_reports_are_listed_newest_first_checked_and_cut_to_size() {
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "a");
    let s = t.login(&mut k);
    let get = || -> Value { serde_json::from_str(&t.get("/ops/triage", Some(&s)).body).unwrap() };
    // no [deploy] dir
    let v = get();
    assert_eq!(
        (v["set_up"].as_bool(), v["reports"].as_array().map(Vec::len)),
        (Some(false), Some(0))
    );
    assert!(v["message"].as_str().unwrap().contains("[deploy] dir"));
    let hostile = json!({
        "id": "1005", "at": T0 - 5, "author": {"username": "<img src=x onerror=alert(1)>", "name": "mallory\u{1b}[31m"},
        "content": format!("<script>alert(1)</script>\u{7}line two\nline three{}", "x".repeat(6000)),
        "attachments": [
            "https://cdn.discordapp.com/attachments/1/2/shot.png?ex=1",
            "https://cdn.discordapp.com/attachments/1/2/a b",
            "https://evil.example.com/cdn.discordapp.com/x",
            "https://cdn.discordapp.com/\"onmouseover=alert(1)",
            "https://media.discordapp.net/attachments/1/2/../../x",
            "javascript:alert(1)"
        ]
    })
    .to_string();
    let mut lines = vec![
        report(1001, "oldest"),
        "{ this line is not JSON".to_string(),
        report(1003, "third"),
        report(1003, "third again: the same id is listed once"),
        json!({"id": "not-a-number", "content": "no report"}).to_string(),
        json!({"content": "no id"}).to_string(),
        hostile,
        report(1010, "newest"),
    ];
    let d = triage_dir(&t, "list", &lines);
    let v = get();
    assert_eq!(v["set_up"], true);
    let ids: Vec<&str> = v["reports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["1010", "1005", "1003", "1001"],
        "newest first, each id once"
    );
    assert_eq!(
        (v["total"].as_u64(), v["open"].as_u64()),
        (Some(4), Some(4))
    );
    assert_eq!(v["state"]["ok"], true);
    assert_eq!(v["state"]["message"], "2 new reports");
    let r = &v["reports"][1];
    let content = r["content"].as_str().unwrap();
    assert_eq!(content.chars().count(), 4000, "cut to 4000 characters");
    assert!(
        content.starts_with("<script>alert(1)</script> line two\nline three"),
        "{content:?}"
    );
    assert_eq!(r["author"]["username"], "<img src=x onerror=alert(1)>");
    assert_eq!(
        r["author"]["name"], "mallory [31m",
        "control characters become spaces"
    );
    let links: Vec<(bool, &str)> = r["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["link"].is_string(), a["text"].as_str().unwrap()))
        .collect();
    assert_eq!(
        links.iter().map(|l| l.0).collect::<Vec<_>>(),
        [true, false, false, false, false, false],
        "only plain URLs of the Discord file servers are links: {links:?}"
    );
    // the newest 50 of many
    lines = (1..=60u64).map(|i| report(2000 + i, "r")).collect();
    std::fs::write(
        d.join("triage").join("reports.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();
    std::fs::write(
        d.join("triage").join("done.json"),
        json!({"done": {"2060": {"at": T0, "by": "Owner"}, "2001": {"at": T0, "by": "Owner"}}})
            .to_string(),
    )
    .unwrap();
    let v = get();
    let list = v["reports"].as_array().unwrap();
    assert_eq!(list.len(), 50);
    assert_eq!(list[0]["id"], "2060");
    assert_eq!(
        (list[0]["done"].as_bool(), list[0]["done_by"].as_str()),
        (Some(true), Some("Owner"))
    );
    assert_eq!(list[1]["done"], false);
    assert_eq!(
        (v["total"].as_u64(), v["open"].as_u64()),
        (Some(60), Some(58))
    );
    // a poller that failed: the state says so
    std::fs::write(
        d.join("triage").join("state.json"),
        json!({"last_id": "2060", "updated": T0, "fetched": 0, "ok": false, "message": "Discord refused the token (HTTP 401)\u{1b}"}).to_string(),
    )
    .unwrap();
    let v = get();
    assert_eq!(v["state"]["ok"], false);
    assert_eq!(
        v["state"]["message"],
        "Discord refused the token (HTTP 401)"
    );
    // no state file yet
    std::fs::remove_file(d.join("triage").join("state.json")).unwrap();
    assert!(get()["state"].is_null());
    // no folder at all
    std::fs::remove_dir_all(d.join("triage")).unwrap();
    let v = get();
    assert_eq!(v["set_up"], false);
    assert!(v["message"].as_str().unwrap().contains("not set up"));
    // the card is in the page, and bug reports are the default lobby's, box-wide
    assert!(crate::opsweb::PAGE.contains("function triageCard(r)"));
    assert!(crate::adminfwd::forwardable("/ops/live"));
    assert!(
        !crate::adminfwd::forwardable("/ops/triage")
            && !crate::adminfwd::forwardable("/ops/triage/done")
    );
    assert_eq!(
        t.get("/ops/triage", None).status,
        401,
        "no list without a session"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_done_mark_needs_a_session_the_page_token_and_a_stored_report() {
    let t = setup();
    let mut k = SoftKey::new(&t.ca);
    t.enroll(&mut k, "Owner key");
    let s = t.login(&mut k);
    let p = "/ops/triage/done";
    let send = |path: &str,
                cookie: Option<&str>,
                csrf: Option<&str>,
                origin: &str,
                ctype: &str,
                body: Value| {
        let mut r = req("POST", path);
        r.cookie = cookie;
        r.csrf = csrf;
        r.origin = origin;
        r.ctype = ctype;
        t.send(&r, &body).status
    };
    let j = "application/json";
    let ok = |body: Value| send(p, Some(&s.cookie), Some(&s.csrf), ORIGIN, j, body);
    let body = json!({"id": "1003", "done": true});
    // no [deploy] dir
    assert_eq!(ok(body.clone()), 503, "bug reports not set up");
    let d = triage_dir(&t, "mark", &[report(1001, "a"), report(1003, "b")]);
    assert_eq!(
        send(p, None, None, ORIGIN, j, body.clone()),
        401,
        "no session"
    );
    assert_eq!(
        send(p, Some(&s.cookie), None, ORIGIN, j, body.clone()),
        403,
        "no token"
    );
    assert_eq!(
        send(p, Some(&s.cookie), Some("wrong"), ORIGIN, j, body.clone()),
        403,
        "wrong token"
    );
    assert_eq!(
        send(
            p,
            Some(&s.cookie),
            Some(&s.csrf),
            "https://evil.example.com",
            j,
            body.clone()
        ),
        403,
        "another origin"
    );
    assert_eq!(
        send(
            p,
            Some(&s.cookie),
            Some(&s.csrf),
            ORIGIN,
            "text/plain",
            body.clone()
        ),
        415,
        "a form post"
    );
    let done_file = d.join("triage").join("done.json");
    assert_eq!(
        std::fs::read_to_string(&done_file).unwrap(),
        r#"{"done": {}}"#,
        "no refused request writes"
    );
    assert_eq!(
        ok(json!({"id": "9999", "done": true})),
        404,
        "a report that is not stored"
    );
    assert_eq!(ok(json!({"id": "10x3", "done": true})), 400, "digits only");
    assert_eq!(ok(json!({"id": "1003"})), 400, "done must be a boolean");
    assert_eq!(ok(json!({"id": "1003", "done": "yes"})), 400);
    assert_eq!(
        ok(json!({"id": "1003", "done": true, "pad": "x".repeat(2000)})),
        413
    );
    assert_eq!(ok(json!([1])), 400, "no id");
    // the mark, then the list shows it and counts one less open
    let (st, v) = t.post(p, Some(&s), body.clone());
    assert_eq!(st, 200, "{v}");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&done_file).unwrap()).unwrap();
    assert_eq!(
        file,
        json!({"done": {"1003": {"at": T0, "by": "Owner key"}}})
    );
    let list: Value = serde_json::from_str(&t.get("/ops/triage", Some(&s)).body).unwrap();
    assert_eq!(
        (list["total"].as_u64(), list["open"].as_u64()),
        (Some(2), Some(1))
    );
    assert_eq!(list["reports"][0]["id"], "1003");
    assert_eq!(list["reports"][0]["done"], true);
    assert_eq!(list["reports"][0]["done_by"], "Owner key");
    // marking twice is the same mark; unmarking removes it and leaves the other marks
    assert_eq!(ok(json!({"id": "1001", "done": true})), 200);
    assert_eq!(ok(body), 200);
    let (st, _) = t.post(p, Some(&s), json!({"id": "1003", "done": false}));
    assert_eq!(st, 200);
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&done_file).unwrap()).unwrap();
    assert_eq!(
        file,
        json!({"done": {"1001": {"at": T0, "by": "Owner key"}}})
    );
    assert_eq!(
        triage_events(&t),
        vec![
            "report 1003 marked done by Owner key",
            "report 1001 marked done by Owner key",
            "report 1003 marked done by Owner key",
            "report 1003 reopened by Owner key",
        ]
    );
    // a done.json that cannot be written is an answer 500 and an event that is a problem
    std::fs::remove_file(&done_file).unwrap();
    std::fs::create_dir(&done_file).unwrap();
    assert_eq!(ok(json!({"id": "1003", "done": true})), 500);
    let problems: i64 = crate::net::lock(&t.sh)
        .db
        .query_row(
            "SELECT COUNT(*) FROM ops_events WHERE kind = 'triage' AND problem = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(problems, 1);
    // a further instance's prefix takes no mark
    assert_ne!(
        send(
            "/i/modded/ops/triage/done",
            Some(&s.cookie),
            Some(&s.csrf),
            ORIGIN,
            j,
            json!({"id": "1003", "done": true})
        ),
        200
    );
    // the loopback port: reading needs nothing more, a mark needs its admin token
    let lo = |head: String, token: Option<&str>| {
        crate::opsweb::handle_with_body(
            &t.sh,
            &head,
            br#"{"id":"1001","done":false}"#,
            LO,
            token,
            true,
        )
        .unwrap()
        .0
    };
    let tok = {
        let mut g = crate::net::lock(&t.sh);
        let st = &mut *g;
        st.beta.csrf(&mut st.keys)
    };
    let post = |tok: &str| {
        format!(
            "POST {p} HTTP/1.1\r\nHost: 127.0.0.1:10090\r\nContent-Type: application/json\r\nOrigin: http://127.0.0.1:10090\r\nX-Admin-CSRF: {tok}\r\n\r\n"
        )
    };
    std::fs::remove_dir_all(&done_file).unwrap();
    std::fs::write(&done_file, r#"{"done": {}}"#).unwrap();
    let get = "GET /ops/triage HTTP/1.1\r\nHost: 127.0.0.1:10090\r\n\r\n";
    assert_eq!(lo(get.into(), None), 200);
    assert_eq!(lo(post(&tok), None), 403, "no --admin-token");
    assert_eq!(lo(post("wrong"), Some("t")), 403, "the port's page token");
    assert_eq!(lo(post(&tok), Some("t")), 200);
    let _ = std::fs::remove_dir_all(&d);
}
