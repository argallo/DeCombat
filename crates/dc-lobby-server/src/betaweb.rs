//! The HTTP side of the beta list (`beta.rs`):
//!
//! - **Public, on the lobby's web port** (only with `beta.enabled`; 404 otherwise): `POST /beta/signup` (JSON or form;
//!   CORS only for `beta.site_origin`; `OPTIONS` answers the preflight), `GET /beta/confirm?t=` and
//!   `GET /beta/unsubscribe?t=` (also `POST` for the one-click form of `List-Unsubscribe-Post`).
//! - **Operator, on the loopback admin port** (`opsweb.rs` runs the same Host / token checks first): `GET /ops/beta`
//!   (the page), `GET /ops/beta/list`, `POST /ops/beta/invite`, `/ops/beta/feedback`, `/ops/beta/note`, `/ops/beta/delete`. A write needs a
//!   JSON body, an `Origin` equal to `http://<Host>` and the per-process token the page carries in `X-Beta-CSRF`.
//!
//! The reply to a signup is the same whether the address is new, pending, confirmed or unsubscribed.
// Provenance: ORIGINAL (DeCombat server/web code)
// DC-DEFAULT: LG-370 endpoint, CORS and replies; LG-371 the client address; LG-375 operator page and CSRF

use crate::beta::{self, BetaErr, Confirmed, Pick};
use crate::clanweb::{esc, parse_query, status_text, Request};
use crate::state::{Fault, Shared};
use serde_json::{json, Value};
use std::net::IpAddr;

/// Largest public request body.
pub const MAX_BODY: usize = 4096;
/// Largest operator request body.
pub const ADMIN_MAX_BODY: usize = 64 * 1024;

const JSON_TYPE: &str = "application/json; charset=utf-8";

/// The reply to every accepted signup.
pub const MSG_SENT: &str = "Almost done: if this address can join the beta, a mail with a confirmation link is on its way. \
Check your inbox and also the spam or junk folder: while our domain is new the mail can land there. Open the link to finish signing up.";

/// True for the public paths.
pub fn handles(path: &str) -> bool {
    matches!(
        path.trim_end_matches('/'),
        "/beta/signup" | "/beta/confirm" | "/beta/unsubscribe"
    )
}

/// The value of header `name` of a request head.
pub fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// Cloudflare's published edge ranges (https://www.cloudflare.com/ips/, 2026-11). Only a connection from one of these
/// may name the client in `CF-Connecting-IP`: the web port stays open to everyone for the game client's own pages.
const CLOUDFLARE_V4: &[(&str, u8)] = &[
    ("173.245.48.0", 20),
    ("103.21.244.0", 22),
    ("103.22.200.0", 22),
    ("103.31.4.0", 22),
    ("141.101.64.0", 18),
    ("108.162.192.0", 18),
    ("190.93.240.0", 20),
    ("188.114.96.0", 20),
    ("197.234.240.0", 22),
    ("198.41.128.0", 17),
    ("162.158.0.0", 15),
    ("104.16.0.0", 13),
    ("104.24.0.0", 14),
    ("172.64.0.0", 13),
    ("131.0.72.0", 22),
];
const CLOUDFLARE_V6: &[(&str, u8)] = &[
    ("2400:cb00::", 32),
    ("2606:4700::", 32),
    ("2803:f800::", 32),
    ("2405:b500::", 32),
    ("2405:8100::", 32),
    ("2a06:98c0::", 29),
    ("2c0f:f248::", 32),
];

/// True when `ip` is one of Cloudflare's edge addresses.
pub fn is_cloudflare(ip: &str) -> bool {
    let within = |a: u128, net: u128, bits: u8, width: u8| {
        let shift = u32::from(width - bits);
        (a >> shift) == (net >> shift)
    };
    match ip.parse::<IpAddr>() {
        Ok(IpAddr::V4(a)) => CLOUDFLARE_V4.iter().any(|(n, b)| {
            n.parse::<std::net::Ipv4Addr>()
                .is_ok_and(|n| within(u32::from(a).into(), u32::from(n).into(), *b, 32))
        }),
        Ok(IpAddr::V6(a)) => match a.to_ipv4_mapped() {
            Some(v4) => is_cloudflare(&v4.to_string()),
            None => CLOUDFLARE_V6.iter().any(|(n, b)| {
                n.parse::<std::net::Ipv6Addr>()
                    .is_ok_and(|n| within(u128::from(a), u128::from(n), *b, 128))
            }),
        },
        Err(_) => false,
    }
}

/// The client address the limits count: `CF-Connecting-IP` when `trust_cf`, the TCP peer is a Cloudflare edge address
/// and the header parses, else the TCP peer. An IPv6 address counts as its /64, an IPv4-mapped one as the IPv4 address.
pub fn client_ip(head: &str, peer: &str, trust_cf: bool) -> String {
    let from_peer = crate::limits::peer_ip(peer);
    let chosen = if trust_cf && is_cloudflare(&from_peer) {
        header(head, "cf-connecting-ip")
            .filter(|v| v.parse::<IpAddr>().is_ok())
            .map_or(from_peer, str::to_string)
    } else {
        from_peer
    };
    group_ip(&chosen)
}

/// The key a client address is counted under: an IPv6 address counts as its /64, an IPv4-mapped one as the IPv4
/// address, anything that is not an address as itself.
pub fn group_ip(chosen: &str) -> String {
    match chosen.parse::<IpAddr>() {
        Ok(IpAddr::V4(a)) => a.to_string(),
        Ok(IpAddr::V6(a)) => match a.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => {
                let s = a.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        },
        Err(_) => chosen.to_string(),
    }
}

fn http(status: u16, ctype: &str, extra: &str, body: &str) -> Vec<u8> {
    let mut v = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n{extra}Connection: close\r\n\r\n",
        status,
        status_text(status),
        ctype,
        body.len()
    )
    .into_bytes();
    v.extend_from_slice(body.as_bytes());
    v
}

fn jreply(ok: bool, msg: &str) -> String {
    json!({"ok": ok, "message": msg}).to_string()
}

/// The answer to a body that could not be read (413 above the cap, else 400).
pub fn body_error(code: u16) -> Vec<u8> {
    let (status, msg) = if code == 413 {
        (413, "the request is too large")
    } else {
        (400, "bad request")
    };
    http(status, JSON_TYPE, "", &jreply(false, msg))
}

/// A small page in the style of the other lobby pages (no script, no external file).
fn page(title: &str, msg: &str, ok: bool, back: &str) -> String {
    let back = if back.is_empty() {
        String::new()
    } else {
        format!("<p><a href=\"{0}\">Back to {0}</a></p>", esc(back))
    };
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" \
         content=\"width=device-width,initial-scale=1\"><meta name=\"referrer\" content=\"no-referrer\">\
         <title>DeCombat beta</title><style>\
         :root{{--bg:#f6f7f9;--panel:#fff;--text:#1c2128;--muted:#5b6672;--ok:#1a7f37;--bad:#b42318;--link:#2457c5}}\
         @media (prefers-color-scheme:dark){{:root{{--bg:#14181d;--panel:#1c2229;--text:#e6e9ed;--muted:#9aa5b1;\
         --ok:#5fd37e;--bad:#ff8a80;--link:#6c9bff}}}}\
         body{{margin:0;background:var(--bg);color:var(--text);font:16px/1.5 system-ui,sans-serif}}\
         main{{max-width:34rem;margin:12vh auto;padding:0 16px}}\
         .card{{background:var(--panel);border-radius:10px;padding:8px 20px 12px;border:1px solid rgba(128,128,128,.3)}}\
         .msg{{color:var(--{cls});font-weight:600}}a{{color:var(--link)}}\
         </style></head><body><main><div class=\"card\"><h1>{title}</h1><p class=\"msg\">{msg}</p>{back}</div></main></body></html>\n",
        cls = if ok { "ok" } else { "bad" },
        title = esc(title),
        msg = esc(msg),
    )
}

fn site_back(site: &str) -> String {
    site.trim().to_string()
}

/// The CORS headers for a request whose `Origin` is the configured site.
fn cors(origin: Option<&str>, site: &str) -> String {
    match origin {
        Some(o) if !site.is_empty() && o == site => {
            format!("Access-Control-Allow-Origin: {o}\r\nVary: Origin\r\n")
        }
        _ => "Vary: Origin\r\n".to_string(),
    }
}

/// A truthy form or JSON value.
fn truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// The whole answer (HTTP bytes) to a public request. `body` is the (capped) request body.
pub fn respond(shared: &Shared, req: &Request, head: &str, body: &[u8], peer: &str) -> Vec<u8> {
    respond_at(shared, req, head, body, peer, crate::clock::unix_now())
}

/// [`respond`] at unix time `unix` (tests move the clock).
pub fn respond_at(
    shared: &Shared,
    req: &Request,
    head: &str,
    body: &[u8],
    peer: &str,
    unix: i64,
) -> Vec<u8> {
    let mut g = crate::net::lock(shared);
    let cfg = g.opts.beta.clone();
    if !cfg.enabled {
        return http(404, JSON_TYPE, "", &jreply(false, "not found"));
    }
    let origin = header(head, "origin");
    let ip = client_ip(head, peer, cfg.trust_cf_header);
    let path = req.path.trim_end_matches('/');
    let method = req.method.as_str();
    let site = cfg.site_origin.trim().to_string();
    let back = site_back(&site);

    if path == "/beta/signup" {
        let cors_h = cors(origin, &site);
        if method == "OPTIONS" {
            return match origin {
                Some(o) if o == site => http(
                    204,
                    "text/plain",
                    &format!(
                        "{cors_h}Access-Control-Allow-Methods: POST, OPTIONS\r\n\
                         Access-Control-Allow-Headers: Content-Type\r\nAccess-Control-Max-Age: 86400\r\n"
                    ),
                    "",
                ),
                _ => http(403, JSON_TYPE, "Vary: Origin\r\n", &jreply(false, "this page may not sign you up")),
            };
        }
        if method != "POST" {
            return http(
                405,
                JSON_TYPE,
                &format!("{cors_h}Allow: POST, OPTIONS\r\n"),
                &jreply(false, "use POST"),
            );
        }
        // a browser sends Origin on a cross-site POST: any other site is refused (a tool without one is limited by the rates)
        if origin.is_some_and(|o| o != site) {
            return http(
                403,
                JSON_TYPE,
                "Vary: Origin\r\n",
                &jreply(false, "this page may not sign you up"),
            );
        }
        let ct = header(head, "content-type")
            .unwrap_or("")
            .to_ascii_lowercase();
        let accept = header(head, "accept").unwrap_or("").to_ascii_lowercase();
        let json_in = ct.contains("json");
        let json_out = json_in || accept.starts_with("application/json");
        let (email, platform, consent) = if json_in {
            match serde_json::from_slice::<Value>(body) {
                Ok(Value::Object(m)) => {
                    let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    let consent = match m.get("consent") {
                        Some(Value::Bool(b)) => *b,
                        Some(Value::String(t)) => truthy(t),
                        _ => false,
                    };
                    (s("email"), s("platform"), consent)
                }
                _ => {
                    return http(
                        400,
                        JSON_TYPE,
                        &cors_h,
                        &jreply(false, "send a JSON object"),
                    )
                }
            }
        } else {
            let f = parse_query(&String::from_utf8_lossy(body));
            let g = |k: &str| f.get(k).cloned().unwrap_or_default();
            (g("email"), g("platform"), truthy(&g("consent")))
        };
        let mut res = None;
        let r = g.scope(|st| {
            let mut c = beta::ctx(st, unix);
            match beta::signup(&mut c, &email, &platform, consent, &ip) {
                Err(BetaErr::Db(e)) => return Err(Fault::Db(e)),
                other => res = Some(other),
            }
            Ok(())
        });
        crate::net::report(&g, r);
        let (status, ok, msg, extra) = match res {
            Some(Ok(())) => (200, true, MSG_SENT.to_string(), String::new()),
            Some(Err(BetaErr::Bad(m))) => (400, false, m, String::new()),
            Some(Err(BetaErr::Slow)) => (
                429,
                false,
                "Too many signups from your address. Try again in an hour.".into(),
                "Retry-After: 3600\r\n".into(),
            ),
            Some(Err(BetaErr::NotSetUp)) => (
                503,
                false,
                "Signups are not open right now. Please try again later.".into(),
                String::new(),
            ),
            _ => (
                500,
                false,
                "Server error. Please try again later.".into(),
                String::new(),
            ),
        };
        let extra = format!("{cors_h}{extra}");
        return if json_out {
            http(status, JSON_TYPE, &extra, &jreply(ok, &msg))
        } else {
            http(
                status,
                "text/html; charset=utf-8",
                &extra,
                &page("DeCombat beta", &msg, ok, &back),
            )
        };
    }

    // the two link endpoints: no CORS, never indexed, the token never leaves in a Referer
    let extra = "Referrer-Policy: no-referrer\r\nX-Robots-Tag: noindex\r\n";
    let token = req.query.get("t").map(String::as_str).unwrap_or("");
    let html = "text/html; charset=utf-8";
    // a HEAD request (a mail scanner's look) never changes a row
    if method == "HEAD" {
        return http(200, html, extra, "");
    }
    let slow = |back: &str| {
        http(
            429,
            html,
            extra,
            &page(
                "Slow down",
                "Too many requests from your address. Try again in an hour.",
                false,
                back,
            ),
        )
    };
    if path == "/beta/confirm" {
        if method != "GET" {
            return http(
                405,
                html,
                extra,
                &page("DeCombat beta", "Open the link from the mail.", false, ""),
            );
        }
        let mut res = None;
        let r = g.scope(|st| {
            let mut c = beta::ctx(st, unix);
            match beta::confirm(&mut c, token, &ip) {
                Err(BetaErr::Db(e)) => return Err(Fault::Db(e)),
                other => res = Some(other),
            }
            Ok(())
        });
        crate::net::report(&g, r);
        return match res {
            Some(Ok(Confirmed::Yes)) => http(
                200,
                html,
                extra,
                &page(
                    "You are on the list",
                    "Your address is confirmed. We invite people in small groups: when it is your turn, the invite code arrives by mail.",
                    true,
                    &back,
                ),
            ),
            Some(Ok(Confirmed::No)) => http(
                200,
                html,
                extra,
                &page(
                    "Link not valid",
                    "This confirmation link is wrong or has expired. Sign up again on the website to get a new one.",
                    false,
                    &back,
                ),
            ),
            Some(Err(BetaErr::Slow)) => slow(&back),
            _ => http(500, html, extra, &page("DeCombat beta", "Server error. Please try again later.", false, "")),
        };
    }
    // /beta/unsubscribe: GET from the mail's link, POST from a mail client's one-click button (RFC 8058)
    if method != "GET" && method != "POST" {
        return http(
            405,
            html,
            extra,
            &page("DeCombat beta", "Open the link from the mail.", false, ""),
        );
    }
    let mut res = None;
    let r = g.scope(|st| {
        let mut c = beta::ctx(st, unix);
        match beta::unsubscribe(&mut c, token, &ip) {
            Err(BetaErr::Db(e)) => return Err(Fault::Db(e)),
            other => res = Some(other),
        }
        Ok(())
    });
    crate::net::report(&g, r);
    match res {
        Some(Ok(known)) if method == "POST" => {
            http(if known { 200 } else { 404 }, "text/plain; charset=utf-8", extra, if known { "unsubscribed\n" } else { "not found\n" })
        }
        Some(Ok(true)) => http(
            200,
            html,
            extra,
            &page(
                "Unsubscribed",
                "This address will not get any more beta mail from us. (If it was a mistake, sign up again on the website.)",
                true,
                &back,
            ),
        ),
        Some(Ok(false)) => http(
            200,
            html,
            extra,
            &page("Link not valid", "This unsubscribe link is not valid.", false, &back),
        ),
        Some(Err(BetaErr::Slow)) => slow(&back),
        _ => http(500, html, extra, &page("DeCombat beta", "Server error. Please try again later.", false, "")),
    }
}

// ------------------------------------------------------------------------------------------------- operator

/// The operator page.
pub(crate) const ADMIN_PAGE: &str = include_str!("beta_admin.html");

type Reply = (u16, &'static str, String);

fn jerr(status: u16, msg: &str) -> Reply {
    (status, crate::opsweb::JSON, jreply(false, msg))
}

fn err_reply(e: BetaErr) -> Reply {
    match e {
        BetaErr::NotSetUp => jerr(503, "mail is not set up: set [mail] in server.toml"),
        BetaErr::Bad(m) => jerr(400, &m),
        BetaErr::Slow => jerr(429, "slow down"),
        BetaErr::Budget(m) => jerr(409, &m),
        BetaErr::Db(e) => jerr(500, &format!("db: {e}")),
    }
}

/// True when `path` belongs to the operator beta page.
pub fn admin_handles(path: &str) -> bool {
    let p = path.trim_end_matches('/');
    p == "/ops/beta" || p.starts_with("/ops/beta/")
}

/// The Host header as sent.
fn host_of(head: &str) -> &str {
    header(head, "host").unwrap_or("")
}

/// Who may write through the operator page when it is not served on the loopback admin port: the admin web listener
/// (adminweb.rs, LG-387) passes its public origin and the session's CSRF token, which the page then carries.
#[derive(Debug, Clone)]
pub struct WriteGuard {
    pub origin: String,
    pub csrf: String,
    /// Who writes, for the logs of the writes that name a person (LG-397): the signed-in key and its address.
    pub by: String,
}

/// The write guard: a JSON body, an `Origin` that is this very page's origin, and the page's token.
fn post_guard(head: &str, origin: &str, csrf: &str) -> Result<(), Reply> {
    let ct = header(head, "content-type")
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ct.starts_with("application/json") {
        return Err(jerr(415, "send application/json"));
    }
    let ok_origin = header(head, "origin")
        .is_some_and(|o| !origin.is_empty() && o.eq_ignore_ascii_case(origin));
    if !ok_origin {
        return Err(jerr(
            403,
            "the request does not come from the operator page (Origin)",
        ));
    }
    let tok = header(head, "x-beta-csrf").unwrap_or("");
    if csrf.is_empty() || !crate::hosts::token_eq(tok, csrf) {
        return Err(jerr(403, "missing or wrong page token: reload the page"));
    }
    Ok(())
}

/// One operator request under `/ops/beta` (the caller has checked loopback, Host and the admin token, or the admin web
/// session), with the write guard of another listener (`None`: the loopback admin port's own).
pub fn admin_guarded(
    shared: &Shared,
    req: &Request,
    head: &str,
    body: &[u8],
    guard: Option<&WriteGuard>,
) -> Reply {
    admin_at_guarded(shared, req, head, body, crate::clock::unix_now(), guard)
}

/// [`admin_guarded`] at unix time `unix`.
pub fn admin_at_guarded(
    shared: &Shared,
    req: &Request,
    head: &str,
    body: &[u8],
    unix: i64,
    guard: Option<&WriteGuard>,
) -> Reply {
    let path = req.path.trim_end_matches('/');
    let mut g = crate::net::lock(shared);
    let st = &mut *g;
    match (req.method.as_str(), path) {
        ("GET", "/ops/beta") => {
            let tok = match guard {
                Some(w) => w.csrf.clone(),
                None => st.beta.csrf(&mut st.keys),
            };
            (
                200,
                crate::opsweb::HTML,
                ADMIN_PAGE.replace("__CSRF__", &tok),
            )
        }
        ("GET", "/ops/beta/list") => {
            st.beta.drain(&st.db, unix);
            let q = &req.query;
            let day = |k: &str| q.get(k).and_then(|v| beta::parse_day(v));
            let f = beta::Filter {
                status: q.get("status").cloned(),
                platform: q.get("platform").cloned(),
                before: day("before"),
                after: day("after"),
                q: q.get("q").cloned(),
                limit: q.get("limit").and_then(|v| v.parse().ok()),
            };
            let mail_on = st.mail.enabled();
            let c = beta::ctx(st, unix);
            let out = (|| -> rusqlite::Result<Value> {
                Ok(json!({
                    "ok": true, "now": unix, "mail_on": mail_on, "signups_on": c.cfg.enabled,
                    "budget": beta::budget(&c)?.json(), "counts": beta::counts(&c)?,
                    "rows": beta::list(&c, &f)?,
                    "feedback_default": {"subject": beta::DEFAULT_FEEDBACK_SUBJECT, "body": beta::DEFAULT_FEEDBACK_BODY},
                }))
            })();
            match out {
                Ok(v) => (200, crate::opsweb::JSON, v.to_string()),
                Err(e) => jerr(500, &format!("db: {e}")),
            }
        }
        (
            "POST",
            "/ops/beta/invite" | "/ops/beta/feedback" | "/ops/beta/note" | "/ops/beta/delete",
        ) => {
            // LG-375: on the loopback port the page's origin is http://<Host>; LG-387: the admin web's is its own
            let (origin, csrf) = match guard {
                Some(w) => (w.origin.clone(), w.csrf.clone()),
                None => (
                    format!("http://{}", host_of(head)),
                    st.beta.csrf(&mut st.keys),
                ),
            };
            if host_of(head).is_empty() && guard.is_none() {
                return jerr(
                    403,
                    "the request does not come from the operator page (Origin)",
                );
            }
            if let Err(r) = post_guard(head, &origin, &csrf) {
                return r;
            }
            let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(body) else {
                return jerr(400, "send a JSON object");
            };
            let dry = m.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
            let pick = if path == "/ops/beta/note" {
                None
            } else if let Some(ids) = m.get("ids").and_then(Value::as_array) {
                Some(Pick::Ids(ids.iter().filter_map(Value::as_i64).collect()))
            } else if path == "/ops/beta/delete" {
                return jerr(400, "name the rows to delete: ids");
            } else if let Some(n) = m.get("next").and_then(Value::as_u64) {
                Some(Pick::Next(n as usize))
            } else {
                return jerr(400, "name the rows: ids or next");
            };
            let mut res: Option<Result<Value, BetaErr>> = None;
            let r = st.scope(|st| {
                let mut c = beta::ctx(st, unix);
                let out = match &pick {
                    None => {
                        let id = m.get("id").and_then(Value::as_i64).unwrap_or(0);
                        let note = m.get("note").and_then(Value::as_str).unwrap_or("");
                        beta::set_note(&c, id, note).map(|_| json!({"ok": true}))
                    }
                    Some(pk) => {
                        let results = if path == "/ops/beta/invite" {
                            beta::invite(&mut c, pk, dry)
                        } else if path == "/ops/beta/delete" {
                            beta::delete(&c, pk, dry)
                        } else {
                            let s = m.get("subject").and_then(Value::as_str).unwrap_or("");
                            let b = m.get("body").and_then(Value::as_str).unwrap_or("");
                            beta::feedback(&mut c, pk, s, b, dry)
                        };
                        results.and_then(|rs| {
                            let sent = rs.iter().filter(|r| r.ok).count();
                            let budget = beta::budget(&c)?.json();
                            Ok(json!({"ok": true, "dry_run": dry, "sent": sent,
                                      "results": rs.iter().map(|r| r.json()).collect::<Vec<_>>(), "budget": budget}))
                        })
                    }
                };
                match out {
                    Err(BetaErr::Db(e)) => return Err(Fault::Db(e)),
                    other => res = Some(other),
                }
                Ok(())
            });
            crate::net::report(st, r);
            match res {
                Some(Ok(v)) => (200, crate::opsweb::JSON, v.to_string()),
                Some(Err(e)) => err_reply(e),
                None => jerr(500, "server error"),
            }
        }
        (
            _,
            "/ops/beta" | "/ops/beta/list" | "/ops/beta/invite" | "/ops/beta/feedback"
            | "/ops/beta/note" | "/ops/beta/delete",
        ) => jerr(405, "method not allowed"),
        _ => jerr(404, "not found"),
    }
}

#[cfg(test)]
#[path = "betaweb_tests.rs"]
mod tests;
