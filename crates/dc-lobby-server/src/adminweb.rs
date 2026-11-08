//! The operator pages on a public name (`[admin_web]`, LG-382..LG-387): a second loopback listener of the lobby that a
//! Cloudflare Tunnel (`cloudflared` on the same machine) maps to `https://admin.<domain>`. Signing in takes a WebAuthn
//! assertion of an enrolled hardware security key (webauthn.rs); there is no password and no other way in from the
//! outside. Behind the session it serves the operator dashboard and the beta list (opsweb.rs, betaweb.rs, the same
//! handlers as the loopback admin port) and the key management page; nothing else of the admin port.
//!
//! - Every request must name the configured host. The TCP peer is always the tunnel, so it is never trusted for
//!   anything: no loopback shortcut, no admin token. The client address for the limits is `CF-Connecting-IP`.
//! - `GET /login`, `/enroll`, `/admin-web.js` and the four ceremony POSTs under `/auth/login/` and `/auth/enroll/` work
//!   without a session; every other path needs one (a page is redirected to `/login`, anything else is 401).
//! - Every POST needs `Content-Type: application/json` and `Origin` equal to `public_origin`; with a session also the
//!   session's CSRF token (`X-Admin-CSRF`, or `X-Beta-CSRF` from the beta page).
//! - Keys are enrolled with a one-time link (`dc-lobby-server admin-web enroll`, 15 minutes, one key) or from the key
//!   page after a fresh assertion with a key already enrolled.
//! - Further lobby instances on the same box (`instances`, LG-399): their dashboards under `/i/<name>/ops`, behind
//!   the same session and checks, forwarded to their loopback admin ports (adminfwd.rs).
// Provenance: ORIGINAL (DeCombat operator tooling)
// DC-DEFAULT: LG-382 listener and transport; LG-383 hardware keys; LG-384 enrolment; LG-385 sessions; LG-386 limits and
// log; LG-387 pages and write guard; LG-399 further instances

use crate::betaweb::{group_ip, header, WriteGuard};
use crate::clanweb::{parse_request, read_body, read_head_and_rest};
use crate::state::Shared;
use crate::webauthn::{self, b64, Expect, NewCredential, Trust};
use dc_config::AdminWebSection;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The session cookie (the `__Host-` prefix: Secure, Path=/, no Domain).
pub const COOKIE: &str = "__Host-dcadmin";
/// How long a ceremony's challenge can be answered.
const CEREMONY_SECS: i64 = 180;
/// The browser's own timeout for the key prompt.
const CLIENT_TIMEOUT_MS: u64 = 120_000;
/// How long a fresh assertion allows adding or removing a key.
const STEP_UP_SECS: i64 = 300;
/// How long an enrolment link works.
pub const ENROLL_SECS: i64 = 15 * 60;
/// The window of the attempt limits.
const WINDOW_SECS: i64 = 600;
const MAX_SESSIONS: usize = 16;
const MAX_CEREMONIES: usize = 64;
const MAX_BODY: usize = 64 * 1024;
const MAX_LABEL: usize = 40;
/// Connections served at once (each is a thread; the tunnel is open to everybody).
const MAX_CONNECTIONS: usize = 64;
/// The one admin account's WebAuthn user handle: not a secret, the same for every key.
const USER_ID: &[u8] = b"decombat-admin";

const PAGE: &str = include_str!("admin_web.html");
const SCRIPT: &str = include_str!("admin_web.js");

const JSON_TYPE: &str = "application/json; charset=utf-8";
const HTML_TYPE: &str = "text/html; charset=utf-8";

// ------------------------------------------------------------------------------------------------- state

#[derive(Debug, Clone)]
struct Session {
    key_id: i64,
    created: i64,
    seen: i64,
    csrf: String,
    step_up_until: i64,
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Login,
    Enroll { token: String },
    StepUp { session: String },
    AddKey { session: String },
}

#[derive(Debug, Clone)]
struct Ceremony {
    kind: Kind,
    challenge: Vec<u8>,
    expires: i64,
}

/// The listener's memory: sessions, open ceremonies and attempt windows. A lobby restart signs everybody out.
pub struct Runtime {
    pub cfg: AdminWebSection,
    pub trust: Arc<Trust>,
    /// By the SHA-256 (hex) of the cookie value.
    sessions: HashMap<String, Session>,
    ceremonies: HashMap<String, Ceremony>,
    /// LG-386: attempts per client address in the window (each queue at most `attempts_per_ip` long, at most
    /// [`MAX_IPS`] addresses).
    per_ip: HashMap<String, IpWindow>,
    /// Challenges issued in the window, all addresses together (at most `attempts_global`).
    issued: VecDeque<i64>,
    /// When the global limit was last logged.
    global_logged: i64,
    /// Refusals and limit hits not logged since the last limit line.
    unlogged: u64,
}

/// The attempts of one client address.
#[derive(Debug)]
struct IpWindow {
    hits: VecDeque<i64>,
    /// When this address's limit was last logged (once per window).
    limit_logged: i64,
}

impl Default for IpWindow {
    fn default() -> Self {
        IpWindow {
            hits: VecDeque::new(),
            limit_logged: i64::MIN / 2,
        }
    }
}

/// Client addresses tracked at once; above, the one quiet the longest is forgotten.
const MAX_IPS: usize = 4096;

/// What the limiter decided.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Gate {
    Open,
    /// Over a limit; `true` the first time in this window (log it).
    Shut(bool),
}

pub type Web = Arc<Mutex<Runtime>>;

fn random(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    // the OS generator; it does not fail on the platforms the lobby runs on
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut v);
    v
}

fn lock(web: &Web) -> std::sync::MutexGuard<'_, Runtime> {
    web.lock().unwrap_or_else(|p| p.into_inner())
}

impl Runtime {
    pub fn new(cfg: AdminWebSection, trust: Trust) -> Runtime {
        Runtime {
            cfg,
            trust: Arc::new(trust),
            sessions: HashMap::new(),
            ceremonies: HashMap::new(),
            per_ip: HashMap::new(),
            issued: VecDeque::new(),
            global_logged: i64::MIN / 2,
            unlogged: 0,
        }
    }

    pub fn shared(cfg: AdminWebSection, trust: Trust) -> Web {
        Arc::new(Mutex::new(Runtime::new(cfg, trust)))
    }

    /// The host part of `public_origin`, lower case (what `Host` must say).
    pub fn host(&self) -> String {
        self.cfg
            .public_origin
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    }

    fn prune(&mut self, now: i64) {
        let old = |t: &i64| now - *t >= WINDOW_SECS;
        while self.issued.front().is_some_and(old) {
            self.issued.pop_front();
        }
        for w in self.per_ip.values_mut() {
            while w.hits.front().is_some_and(old) {
                w.hits.pop_front();
            }
        }
        self.per_ip
            .retain(|_, w| !w.hits.is_empty() || now - w.limit_logged < WINDOW_SECS);
    }

    fn ip_entry(&mut self, ip: &str) -> &mut IpWindow {
        if !self.per_ip.contains_key(ip) && self.per_ip.len() >= MAX_IPS {
            let quiet = self
                .per_ip
                .iter()
                .min_by_key(|(_, w)| w.hits.back().copied().unwrap_or(w.limit_logged))
                .map(|(k, _)| k.clone());
            if let Some(k) = quiet {
                self.per_ip.remove(&k);
            }
        }
        self.per_ip.entry(ip.to_string()).or_default()
    }

    /// Whether `ip` is under its own limit (nothing is counted). Over it, the first refusal in a window is logged.
    fn ip_gate(&mut self, ip: &str, now: i64) -> Gate {
        self.prune(now);
        let limit = self.cfg.attempts_per_ip as usize;
        let w = self.ip_entry(ip);
        if w.hits.len() < limit {
            return Gate::Open;
        }
        if now - w.limit_logged >= WINDOW_SECS {
            w.limit_logged = now;
            return Gate::Shut(true);
        }
        self.unlogged += 1;
        Gate::Shut(false)
    }

    /// LG-386: a new challenge for `ip`, when both its own limit and the global one allow it; counted in both.
    fn allow(&mut self, ip: &str, now: i64) -> Gate {
        let g = self.ip_gate(ip, now);
        if g != Gate::Open {
            return g;
        }
        if self.issued.len() >= self.cfg.attempts_global as usize {
            if now - self.global_logged >= WINDOW_SECS {
                self.global_logged = now;
                return Gate::Shut(true);
            }
            self.unlogged += 1;
            return Gate::Shut(false);
        }
        self.ip_entry(ip).hits.push_back(now);
        self.issued.push_back(now);
        Gate::Open
    }

    /// A refused answer counts against its address only, and only while the address is under its limit; `true`
    /// when it should be logged (an address over its limit is not logged again, only counted).
    fn note_refusal(&mut self, ip: &str, now: i64) -> bool {
        self.prune(now);
        let limit = self.cfg.attempts_per_ip as usize;
        let w = self.ip_entry(ip);
        if w.hits.len() < limit {
            w.hits.push_back(now);
            return true;
        }
        self.unlogged += 1;
        false
    }

    /// The count of refusals not logged since the last call.
    fn take_unlogged(&mut self) -> u64 {
        std::mem::take(&mut self.unlogged)
    }

    fn new_ceremony(&mut self, kind: Kind, now: i64) -> (String, Vec<u8>) {
        self.ceremonies.retain(|_, c| c.expires >= now);
        if self.ceremonies.len() >= MAX_CEREMONIES {
            if let Some(k) = self
                .ceremonies
                .iter()
                .min_by_key(|(_, c)| c.expires)
                .map(|(k, _)| k.clone())
            {
                self.ceremonies.remove(&k);
            }
        }
        let id = b64::url(&random(16));
        let challenge = random(32);
        self.ceremonies.insert(
            id.clone(),
            Ceremony {
                kind,
                challenge: challenge.clone(),
                expires: now + CEREMONY_SECS,
            },
        );
        (id, challenge)
    }

    /// The ceremony `id`, removed (one answer per challenge); `None` when unknown or expired.
    fn take_ceremony(&mut self, id: &str, now: i64) -> Option<Ceremony> {
        self.ceremonies.remove(id).filter(|c| c.expires >= now)
    }

    /// A new session for `key_id`; the cookie value.
    fn new_session(&mut self, key_id: i64, now: i64) -> String {
        if self.sessions.len() >= MAX_SESSIONS {
            if let Some(k) = self
                .sessions
                .iter()
                .min_by_key(|(_, s)| s.seen)
                .map(|(k, _)| k.clone())
            {
                self.sessions.remove(&k);
            }
        }
        let value = b64::url(&random(32));
        self.sessions.insert(
            webauthn::hex(&webauthn::sha256(value.as_bytes())),
            Session {
                key_id,
                created: now,
                seen: now,
                csrf: b64::url(&random(32)),
                step_up_until: 0,
            },
        );
        value
    }

    /// LG-385: the session of a cookie value, still within its idle and absolute limits (touched); its id.
    fn session(&mut self, cookie: &str, now: i64) -> Option<(String, Session)> {
        let sid = webauthn::hex(&webauthn::sha256(cookie.as_bytes()));
        let idle = i64::from(self.cfg.idle_minutes) * 60;
        let total = i64::from(self.cfg.session_hours) * 3600;
        let s = self.sessions.get_mut(&sid)?;
        if now - s.seen > idle || now - s.created > total {
            self.sessions.remove(&sid);
            return None;
        }
        s.seen = now;
        Some((sid, s.clone()))
    }

    fn drop_sessions_of(&mut self, key_id: i64) {
        self.sessions.retain(|_, s| s.key_id != key_id);
    }
}

// ------------------------------------------------------------------------------------------------- DB

/// An enrolled security key.
#[derive(Debug, Clone)]
pub struct KeyRow {
    pub id: i64,
    pub cred_id: Vec<u8>,
    pub public_key: Vec<u8>,
    pub alg: i64,
    pub counter: u32,
    pub aaguid: String,
    pub root: String,
    pub transports: String,
    pub label: String,
    pub created: i64,
    pub last_used: i64,
}

impl KeyRow {
    /// A short, harmless name of the credential for logs and pages.
    pub fn short(&self) -> String {
        webauthn::hex(&self.cred_id[..self.cred_id.len().min(6)])
    }
}

const KEY_COLS: &str =
    "id, cred_id, public_key, alg, counter, aaguid, root, transports, label, created, last_used";

fn key_row(r: &rusqlite::Row) -> rusqlite::Result<KeyRow> {
    Ok(KeyRow {
        id: r.get(0)?,
        cred_id: r.get(1)?,
        public_key: r.get(2)?,
        alg: r.get(3)?,
        counter: r.get::<_, i64>(4)?.clamp(0, i64::from(u32::MAX)) as u32,
        aaguid: r.get(5)?,
        root: r.get(6)?,
        transports: r.get(7)?,
        label: r.get(8)?,
        created: r.get(9)?,
        last_used: r.get(10)?,
    })
}

pub fn keys(c: &Connection) -> rusqlite::Result<Vec<KeyRow>> {
    let mut s = c.prepare(&format!(
        "SELECT {KEY_COLS} FROM admin_web_keys ORDER BY id"
    ))?;
    let rows = s.query_map([], key_row)?;
    rows.collect()
}

pub fn key_by_cred(c: &Connection, cred_id: &[u8]) -> rusqlite::Result<Option<KeyRow>> {
    c.query_row(
        &format!("SELECT {KEY_COLS} FROM admin_web_keys WHERE cred_id = ?1"),
        [cred_id],
        key_row,
    )
    .optional()
}

pub fn key_by_id(c: &Connection, id: i64) -> rusqlite::Result<Option<KeyRow>> {
    c.query_row(
        &format!("SELECT {KEY_COLS} FROM admin_web_keys WHERE id = ?1"),
        [id],
        key_row,
    )
    .optional()
}

pub fn add_key(
    c: &Connection,
    k: &NewCredential,
    label: &str,
    transports: &str,
    now: i64,
) -> rusqlite::Result<i64> {
    c.execute(
        "INSERT INTO admin_web_keys (cred_id, public_key, alg, counter, aaguid, root, transports, label, created, last_used) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)",
        params![
            k.cred_id,
            k.public_key,
            k.alg,
            i64::from(k.counter),
            webauthn::hex(&k.aaguid),
            k.root,
            transports,
            label,
            now
        ],
    )?;
    Ok(c.last_insert_rowid())
}

/// Stores the new counter if the stored one is still `old`; false when another sign-in moved it or the key is gone.
fn set_counter(
    c: &Connection,
    id: i64,
    old: u32,
    counter: u32,
    now: i64,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE admin_web_keys SET counter = ?3, last_used = ?4 WHERE id = ?1 AND counter = ?2",
        params![id, i64::from(old), i64::from(counter), now],
    )? == 1)
}

pub fn rename_key(c: &Connection, id: i64, label: &str) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE admin_web_keys SET label = ?2 WHERE id = ?1",
        params![id, label],
    )? == 1)
}

pub fn remove_key(c: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(c.execute("DELETE FROM admin_web_keys WHERE id = ?1", [id])? == 1)
}

fn token_hash(token: &str) -> String {
    webauthn::hex(&webauthn::sha256(token.as_bytes()))
}

/// LG-384: a new one-time enrolment link token (only its hash is stored), valid [`ENROLL_SECS`].
pub fn new_enroll_link(c: &Connection, label: &str, now: i64) -> rusqlite::Result<String> {
    c.execute(
        "DELETE FROM admin_web_enroll WHERE expires < ?1",
        [now - 86_400],
    )?;
    let token = b64::url(&random(32));
    c.execute(
        "INSERT INTO admin_web_enroll (token_hash, label, created, expires, used) VALUES (?1, ?2, ?3, ?4, 0)",
        params![token_hash(&token), label, now, now + ENROLL_SECS],
    )?;
    Ok(token)
}

/// The label of a link that is still open.
fn enroll_link(c: &Connection, token: &str, now: i64) -> rusqlite::Result<Option<String>> {
    if token.is_empty() || token.len() > 100 {
        return Ok(None);
    }
    c.query_row(
        "SELECT label FROM admin_web_enroll WHERE token_hash = ?1 AND used = 0 AND expires >= ?2",
        params![token_hash(token), now],
        |r| r.get(0),
    )
    .optional()
}

fn consume_enroll_link(c: &Connection, token: &str, now: i64) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE admin_web_enroll SET used = 1 WHERE token_hash = ?1 AND used = 0 AND expires >= ?2",
        params![token_hash(token), now],
    )? == 1)
}

/// A label as typed: trimmed, control characters dropped, at most [`MAX_LABEL`] characters.
fn clean_label(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_LABEL)
        .collect()
}

const TRANSPORTS: &[&str] = &["usb", "nfc", "ble", "hybrid", "smart-card", "internal"];

fn clean_transports(v: Option<&Value>) -> String {
    let mut out: Vec<&str> = Vec::new();
    if let Some(Value::Array(a)) = v {
        for t in a.iter().filter_map(Value::as_str) {
            if let Some(k) = TRANSPORTS.iter().find(|k| **k == t) {
                if !out.contains(k) {
                    out.push(k);
                }
            }
        }
    }
    out.join(",")
}

// ------------------------------------------------------------------------------------------------- responses

/// One answer of the listener.
#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    pub ctype: &'static str,
    /// Extra header lines, each ending in CRLF.
    pub headers: String,
    pub body: String,
}

fn status_text(s: u16) -> &'static str {
    match s {
        200 => "OK",
        303 => "See Other",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        421 => "Misdirected Request",
        429 => "Too Many Requests",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Internal Server Error",
    }
}

/// `'sha256-...'` sources for the inline `<script>` blocks of a page (the dashboard and the beta page carry theirs
/// inline; this listener's own pages load `/admin-web.js`). A browser hashes the script text after the HTML parser's
/// newline normalisation (CR LF and a lone CR become LF), so the template is normalised the same way first: a checkout
/// with CRLF line ends would otherwise give hashes no browser matches, and every inline script would be blocked.
pub fn inline_script_hashes(html: &str) -> Vec<String> {
    let html = html.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = Vec::new();
    let mut rest = html.as_str();
    while let Some(i) = rest.find("<script>") {
        let after = &rest[i + "<script>".len()..];
        let Some(j) = after.find("</script>") else {
            break;
        };
        out.push(format!(
            "'sha256-{}'",
            b64::std(&webauthn::sha256(&after.as_bytes()[..j]))
        ));
        rest = &after[j..];
    }
    out
}

/// `'self'` and the hashes of the inline scripts of the operator page templates.
fn template_script_sources() -> &'static str {
    static SOURCES: OnceLock<String> = OnceLock::new();
    SOURCES.get_or_init(|| {
        let mut v = vec!["'self'".to_string()];
        for page in [crate::opsweb::PAGE, crate::betaweb::ADMIN_PAGE] {
            for h in inline_script_hashes(page) {
                if !v.contains(&h) {
                    v.push(h);
                }
            }
        }
        v.join(" ")
    })
}

impl Resp {
    fn new(status: u16, ctype: &'static str, body: String) -> Resp {
        Resp {
            status,
            ctype,
            headers: String::new(),
            body,
        }
    }

    fn json(status: u16, v: Value) -> Resp {
        Resp::new(status, JSON_TYPE, v.to_string())
    }

    fn err(status: u16, msg: &str) -> Resp {
        Resp::json(status, json!({"ok": false, "message": msg}))
    }

    fn redirect(to: &str) -> Resp {
        let mut r = Resp::new(303, HTML_TYPE, String::new());
        r.headers.push_str(&format!("Location: {to}\r\n"));
        r
    }

    fn with(mut self, line: &str) -> Resp {
        self.headers.push_str(line);
        self.headers.push_str("\r\n");
        self
    }

    /// The Content-Security-Policy of this answer. The allowed inline scripts are those of the static templates
    /// (the dashboard's and the beta page's), hashed once: nothing rendered into a response can add one.
    pub fn csp(&self) -> String {
        if self.ctype.starts_with("text/html") {
            format!(
                "default-src 'none'; script-src {}; style-src 'unsafe-inline'; connect-src 'self'; img-src data:; \
                 base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
                template_script_sources()
            )
        } else {
            "default-src 'none'; frame-ancestors 'none'".to_string()
        }
    }

    /// The HTTP bytes, with the security headers every answer of this listener carries (LG-387).
    pub fn bytes(&self) -> Vec<u8> {
        let mut v = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
             X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\n\
             Cross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Resource-Policy: same-origin\r\n\
             Strict-Transport-Security: max-age=31536000\r\nX-Robots-Tag: noindex\r\n\
             Content-Security-Policy: {}\r\n{}Connection: close\r\n\r\n",
            self.status,
            status_text(self.status),
            self.ctype,
            self.body.len(),
            self.csp(),
            self.headers,
        )
        .into_bytes();
        v.extend_from_slice(self.body.as_bytes());
        v
    }
}

fn page(which: &str, csrf: &str) -> Resp {
    Resp::new(
        200,
        HTML_TYPE,
        PAGE.replace("__PAGE__", which).replace("__CSRF__", csrf),
    )
}

fn session_cookie(value: &str, max_age: i64) -> String {
    format!("Set-Cookie: {COOKIE}={value}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={max_age}")
}

/// The value of cookie `name`.
fn cookie<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if !k.trim().eq_ignore_ascii_case("cookie") {
            return None;
        }
        v.split(';').find_map(|p| {
            let (n, val) = p.trim().split_once('=')?;
            (n == name && !val.is_empty()).then_some(val)
        })
    })
}

/// LG-386: the client address: `CF-Connecting-IP` (the peer is always the tunnel), else the peer.
fn client_ip(head: &str, peer: Option<IpAddr>) -> String {
    match header(head, "cf-connecting-ip").filter(|v| v.parse::<IpAddr>().is_ok()) {
        Some(v) => group_ip(v),
        None => peer.map_or_else(|| "unknown".to_string(), |p| group_ip(&p.to_string())),
    }
}

/// LG-387: the guard of every POST: JSON and this listener's own origin.
fn post_guard(head: &str, origin: &str) -> Result<(), Resp> {
    let ct = header(head, "content-type")
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ct.starts_with("application/json") {
        return Err(Resp::err(415, "send application/json"));
    }
    if header(head, "origin") != Some(origin) {
        return Err(Resp::err(
            403,
            "the request does not come from this site (Origin)",
        ));
    }
    Ok(())
}

fn event(shared: &Shared, problem: bool, detail: &str) {
    let g = crate::net::lock(shared);
    crate::opsmon::admin_web_event(&g.db, problem, detail);
    g.note(&format!("admin-web: {detail}"));
}

/// LG-430: the signed-in admin's display name: the label of the session's key, else `key #<id>`.
fn admin_name(c: &Ctx, s: &Session) -> String {
    let label = key_by_id(&crate::net::lock(c.shared).db, s.key_id)
        .ok()
        .flatten()
        .map(|k| k.label.trim().to_string())
        .unwrap_or_default();
    if label.is_empty() {
        format!("key #{}", s.key_id)
    } else {
        label
    }
}

fn key_name(k: &KeyRow) -> String {
    format!("key #{} {:?} (cred {})", k.id, k.label, k.short())
}

fn body_json(body: &[u8]) -> Result<serde_json::Map<String, Value>, Resp> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(m)) => Ok(m),
        _ => Err(Resp::err(400, "send a JSON object")),
    }
}

fn field_b64(v: &Value, path: &[&str]) -> Option<Vec<u8>> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    b64::decode(cur.as_str()?)
}

// ------------------------------------------------------------------------------------------------- ceremonies

fn request_options(cfg: &AdminWebSection, challenge: &[u8], keys: &[KeyRow]) -> Value {
    json!({
        "challenge": b64::url(challenge),
        "rpId": cfg.rp_id,
        "timeout": CLIENT_TIMEOUT_MS,
        "userVerification": "required",
        "allowCredentials": keys.iter().map(descriptor).collect::<Vec<_>>(),
        "hints": ["security-key"],
    })
}

fn descriptor(k: &KeyRow) -> Value {
    let mut d = json!({"type": "public-key", "id": b64::url(&k.cred_id)});
    if !k.transports.is_empty() {
        d["transports"] = json!(k.transports.split(',').collect::<Vec<_>>());
    }
    d
}

/// LG-383: the creation options: attestation `direct`, a roaming authenticator, the PIN required, ES256 or EdDSA.
fn creation_options(cfg: &AdminWebSection, challenge: &[u8], keys: &[KeyRow]) -> Value {
    json!({
        "rp": {"id": cfg.rp_id, "name": "DeCombat admin"},
        "user": {"id": b64::url(USER_ID), "name": "admin", "displayName": "DeCombat admin"},
        "challenge": b64::url(challenge),
        "pubKeyCredParams": [{"type": "public-key", "alg": webauthn::ALG_ES256}, {"type": "public-key", "alg": webauthn::ALG_EDDSA}],
        "timeout": CLIENT_TIMEOUT_MS,
        "attestation": "direct",
        "authenticatorSelection": {
            "authenticatorAttachment": "cross-platform",
            "residentKey": "discouraged",
            "requireResidentKey": false,
            "userVerification": "required",
        },
        "excludeCredentials": keys.iter().map(descriptor).collect::<Vec<_>>(),
        "hints": ["security-key"],
    })
}

/// Everything a handler needs.
struct Ctx<'a> {
    shared: &'a Shared,
    web: &'a Web,
    cfg: AdminWebSection,
    trust: Arc<Trust>,
    ip: String,
    now: i64,
}

impl Ctx<'_> {
    fn expect<'c>(&'c self, challenge: &'c [u8]) -> Expect<'c> {
        Expect {
            rp_id: &self.cfg.rp_id,
            origin: &self.cfg.public_origin,
            challenge,
        }
    }

    /// A refused attempt: counted against its address and logged as a problem while the address is under its limit
    /// (a flood writes at most `attempts_per_ip` rows per address and window).
    fn refuse(&self, status: u16, what: &str, msg: &str) -> Resp {
        let log = lock(self.web).note_refusal(&self.ip, self.now);
        if log {
            event(
                self.shared,
                true,
                &format!("{what} refused from {}: {msg}", self.ip),
            );
        }
        Resp::err(status, msg)
    }

    fn too_many(&self, gate: Gate, whose: &str) -> Resp {
        if gate == Gate::Shut(true) {
            let n = lock(self.web).take_unlogged();
            event(
                self.shared,
                true,
                &format!("attempt limit reached {whose}; {n} refusals since the last such line were not logged"),
            );
        }
        Resp::err(429, "too many attempts: wait ten minutes").with("Retry-After: 600")
    }

    /// The limit of a ceremony start (a new challenge).
    fn limited(&self) -> Option<Resp> {
        let (gate, over_ip) = {
            let mut w = lock(self.web);
            let g = w.allow(&self.ip, self.now);
            let limit = w.cfg.attempts_per_ip as usize;
            (
                g,
                w.per_ip
                    .get(&self.ip)
                    .is_some_and(|x| x.hits.len() >= limit),
            )
        };
        match gate {
            Gate::Open => None,
            g => {
                let whose = if over_ip {
                    format!("for {}", self.ip)
                } else {
                    "in all".to_string()
                };
                Some(self.too_many(g, &whose))
            }
        }
    }

    /// The limit of an answer (a finish POST): the address's own limit only, checked before any work.
    fn finish_gate(&self) -> Option<Resp> {
        let gate = lock(self.web).ip_gate(&self.ip, self.now);
        match gate {
            Gate::Open => None,
            g => Some(self.too_many(g, &format!("for {}", self.ip))),
        }
    }

    fn all_keys(&self) -> Result<Vec<KeyRow>, Resp> {
        keys(&crate::net::lock(self.shared).db).map_err(|e| Resp::err(500, &format!("db: {e}")))
    }

    /// Options for an assertion with any enrolled key.
    fn assertion_options(&self, kind: Kind) -> Resp {
        if let Some(r) = self.limited() {
            return r;
        }
        let ks = match self.all_keys() {
            Ok(k) => k,
            Err(r) => return r,
        };
        if ks.is_empty() {
            return Resp::err(
                409,
                "no security key is enrolled yet: run `dc-lobby-server admin-web enroll` on the server",
            );
        }
        let (id, ch) = lock(self.web).new_ceremony(kind, self.now);
        Resp::json(
            200,
            json!({"ok": true, "ceremony": id, "publicKey": request_options(&self.cfg, &ch, &ks)}),
        )
    }

    /// Checks an assertion answer for a ceremony of `kind`; the key it came from (its counter already stored).
    fn finish_assertion(&self, body: &[u8], kind: &Kind, what: &str) -> Result<KeyRow, Resp> {
        if let Some(r) = self.finish_gate() {
            return Err(r);
        }
        let m = body_json(body)?;
        let cer_id = m.get("ceremony").and_then(Value::as_str).unwrap_or("");
        let Some(cer) = lock(self.web).take_ceremony(cer_id, self.now) else {
            return Err(self.refuse(
                400,
                what,
                "the request expired or was already used: start again",
            ));
        };
        if &cer.kind != kind {
            return Err(self.refuse(
                400,
                what,
                "the request does not belong to this step: start again",
            ));
        }
        let cred = m.get("credential").cloned().unwrap_or(Value::Null);
        let (Some(raw), Some(cdj), Some(ad), Some(sig)) = (
            field_b64(&cred, &["rawId"]),
            field_b64(&cred, &["response", "clientDataJSON"]),
            field_b64(&cred, &["response", "authenticatorData"]),
            field_b64(&cred, &["response", "signature"]),
        ) else {
            return Err(self.refuse(400, what, "the key's answer is incomplete"));
        };
        // the lobby lock is held for the row only, not while the answer is parsed and checked
        let found = key_by_cred(&crate::net::lock(self.shared).db, &raw);
        let key = match found {
            Ok(Some(k)) => k,
            Ok(None) => return Err(self.refuse(403, what, "this security key is not enrolled")),
            Err(e) => return Err(Resp::err(500, &format!("db: {e}"))),
        };
        let counter = match webauthn::verify_assertion(
            &cdj,
            &ad,
            &sig,
            &self.expect(&cer.challenge),
            &key.public_key,
            key.counter,
        ) {
            Ok(c) => c,
            Err(msg) => return Err(self.refuse(403, what, &format!("{msg} ({})", key_name(&key)))),
        };
        // the counter moves only from the value that was checked (a parallel sign-in with the same key loses)
        let stored = set_counter(
            &crate::net::lock(self.shared).db,
            key.id,
            key.counter,
            counter,
            self.now,
        );
        match stored {
            Ok(true) => Ok(KeyRow { counter, ..key }),
            Ok(false) => Err(self.refuse(
                409,
                what,
                &format!(
                    "the key was used for another sign-in at the same time, or removed: try again ({})",
                    key_name(&key)
                ),
            )),
            Err(e) => Err(Resp::err(500, &format!("db: {e}"))),
        }
    }

    /// Checks a registration answer for a ceremony of `kind` and stores the key (after `before_store`, which may
    /// refuse, under the same DB lock).
    fn finish_registration(
        &self,
        body: &[u8],
        kind: &Kind,
        what: &str,
        default_label: &str,
        before_store: &dyn Fn(&Connection) -> Result<(), String>,
    ) -> Result<KeyRow, Resp> {
        if let Some(r) = self.finish_gate() {
            return Err(r);
        }
        let m = body_json(body)?;
        let cer_id = m.get("ceremony").and_then(Value::as_str).unwrap_or("");
        let Some(cer) = lock(self.web).take_ceremony(cer_id, self.now) else {
            return Err(self.refuse(
                400,
                what,
                "the request expired or was already used: start again",
            ));
        };
        if &cer.kind != kind {
            return Err(self.refuse(
                400,
                what,
                "the request does not belong to this step: start again",
            ));
        }
        let cred = m.get("credential").cloned().unwrap_or(Value::Null);
        let (Some(cdj), Some(ao)) = (
            field_b64(&cred, &["response", "clientDataJSON"]),
            field_b64(&cred, &["response", "attestationObject"]),
        ) else {
            return Err(self.refuse(400, what, "the key's answer is incomplete"));
        };
        let nc = match webauthn::verify_registration(
            &cdj,
            &ao,
            &self.expect(&cer.challenge),
            &self.trust,
            self.now,
        ) {
            Ok(c) => c,
            Err(msg) => return Err(self.refuse(400, what, &msg)),
        };
        let transports = clean_transports(cred.get("response").and_then(|r| r.get("transports")));
        let typed = clean_label(m.get("label").and_then(Value::as_str).unwrap_or(""));
        // checked and stored under one DB lock; a refusal is logged after the lock is let go
        let stored: Result<KeyRow, (u16, String)> = {
            let g = crate::net::lock(self.shared);
            (|| {
                let ks = keys(&g.db).map_err(|e| (500, format!("db: {e}")))?;
                if ks.len() >= self.cfg.max_keys as usize {
                    return Err((
                        409,
                        format!(
                            "{} keys are enrolled already (admin_web.max_keys)",
                            ks.len()
                        ),
                    ));
                }
                if ks.iter().any(|k| k.cred_id == nc.cred_id) {
                    return Err((409, "this security key is enrolled already".to_string()));
                }
                before_store(&g.db).map_err(|m| (403, m))?;
                let label = if !typed.is_empty() {
                    typed.clone()
                } else if !default_label.is_empty() {
                    clean_label(default_label)
                } else {
                    format!("Security key {}", ks.len() + 1)
                };
                let id = add_key(&g.db, &nc, &label, &transports, self.now)
                    .map_err(|e| (500, format!("db: {e}")))?;
                key_by_id(&g.db, id)
                    .ok()
                    .flatten()
                    .ok_or((500, "db: the new key is gone".to_string()))
            })()
        };
        let row = match stored {
            Ok(r) => r,
            Err((500, m)) => return Err(Resp::err(500, &m)),
            Err((status, m)) => return Err(self.refuse(status, what, &m)),
        };
        event(
            self.shared,
            false,
            &format!(
                "{what}: {} enrolled (model {}, attested by {}) from {}",
                key_name(&row),
                row.aaguid,
                row.root,
                self.ip
            ),
        );
        Ok(row)
    }
}

// ------------------------------------------------------------------------------------------------- routing

/// The answer to one request on the admin web listener. `peer` is the TCP peer (the tunnel); `now` unix seconds.
pub fn handle(
    shared: &Shared,
    web: &Web,
    head: &str,
    body: &[u8],
    peer: Option<IpAddr>,
    now: i64,
) -> Resp {
    let Some(req) = parse_request(head) else {
        return Resp::err(400, "bad request");
    };
    let (cfg, trust, host) = {
        let w = lock(web);
        (w.cfg.clone(), Arc::clone(&w.trust), w.host())
    };
    // LG-382: only the configured name (a page that rebinds another name to this machine gets nothing)
    if !header(head, "host").is_some_and(|h| h.eq_ignore_ascii_case(&host)) {
        return Resp::err(421, "this listener answers its configured host only");
    }
    let c = Ctx {
        shared,
        web,
        ip: client_ip(head, peer),
        cfg,
        trust,
        now,
    };
    let path = match req.path.trim_end_matches('/') {
        "" => "/",
        p => p,
    };
    let method = req.method.as_str();
    if method == "GET" && path == "/admin-web.js" {
        return Resp::new(200, "text/javascript; charset=utf-8", SCRIPT.to_string());
    }

    // the session, if any; one whose key was removed (here or by the command line) ends now
    let mut sess = cookie(head, COOKIE).and_then(|v| lock(web).session(v, now));
    if let Some((sid, s)) = &sess {
        let alive = key_by_id(&crate::net::lock(shared).db, s.key_id)
            .ok()
            .flatten()
            .is_some();
        if !alive {
            lock(web).sessions.remove(sid);
            sess = None;
        }
    }

    // pages and ceremonies without a session
    match (method, path) {
        ("GET", "/") => return Resp::redirect(if sess.is_some() { "/ops" } else { "/login" }),
        ("GET", "/login") => {
            return if sess.is_some() {
                Resp::redirect("/ops")
            } else {
                page("login", "")
            }
        }
        ("GET", "/enroll") => return page("enroll", ""),
        (
            "POST",
            "/auth/login/options"
            | "/auth/login/finish"
            | "/auth/enroll/options"
            | "/auth/enroll/finish",
        ) => {
            if let Err(r) = post_guard(head, &c.cfg.public_origin) {
                return r;
            }
            return match path {
                "/auth/login/options" => c.assertion_options(Kind::Login),
                "/auth/login/finish" => login_finish(&c, body),
                "/auth/enroll/options" => enroll_options(&c, body),
                _ => enroll_finish(&c, body),
            };
        }
        _ => {}
    }

    // LG-399: a further instance's pages sit under /i/<name>; the checks below see the path the instance serves
    let inner = crate::adminfwd::split(path).map_or(path, |(_, p)| p);
    let Some((sid, s)) = sess else {
        return if method == "GET" && matches!(inner, "/ops" | "/ops/beta" | "/keys") {
            Resp::redirect("/login")
        } else {
            Resp::err(401, "sign in first")
        };
    };
    if method == "POST" {
        if let Err(r) = post_guard(head, &c.cfg.public_origin) {
            return r;
        }
        let name = if crate::betaweb::admin_handles(inner) {
            "x-beta-csrf"
        } else {
            "x-admin-csrf"
        };
        if !crate::hosts::token_eq(header(head, name).unwrap_or(""), &s.csrf) {
            return Resp::err(403, "missing or wrong page token: reload the page");
        }
    }
    match (method, path) {
        ("GET", "/keys") => page("keys", &s.csrf),
        ("GET", "/keys/list") => keys_list(&c, &s),
        ("POST", "/auth/stepup/options") => c.assertion_options(Kind::StepUp { session: sid }),
        ("POST", "/auth/stepup/finish") => match c.finish_assertion(
            body,
            &Kind::StepUp {
                session: sid.clone(),
            },
            "confirmation",
        ) {
            Ok(k) => {
                if let Some(x) = lock(web).sessions.get_mut(&sid) {
                    x.step_up_until = now + STEP_UP_SECS;
                }
                event(
                    shared,
                    false,
                    &format!("confirmed with {} from {}", key_name(&k), c.ip),
                );
                Resp::json(200, json!({"ok": true}))
            }
            Err(r) => r,
        },
        ("POST", "/keys/enroll/options") => {
            if s.step_up_until < now {
                return Resp::err(403, "confirm with a key you already have first");
            }
            add_key_options(&c, Kind::AddKey { session: sid })
        }
        ("POST", "/keys/enroll/finish") => {
            if let Some(r) = c.finish_gate() {
                return r;
            }
            if s.step_up_until < now {
                return Resp::err(403, "confirm with a key you already have first");
            }
            match c.finish_registration(
                body,
                &Kind::AddKey {
                    session: sid.clone(),
                },
                "key added",
                "",
                &|_| Ok(()),
            ) {
                Ok(k) => {
                    // one key per confirmation
                    if let Some(x) = lock(web).sessions.get_mut(&sid) {
                        x.step_up_until = 0;
                    }
                    Resp::json(
                        200,
                        json!({"ok": true, "id": k.id, "message": format!("{} is enrolled.", k.label)}),
                    )
                }
                Err(r) => r,
            }
        }
        ("POST", "/keys/rename") => key_rename(&c, body),
        ("POST", "/keys/remove") => key_remove(&c, body, &sid, &s),
        ("POST", "/logout") => {
            lock(web).sessions.remove(&sid);
            event(shared, false, &format!("signed out from {}", c.ip));
            Resp::json(200, json!({"ok": true, "redirect": "/login"})).with(&session_cookie("", 0))
        }
        (_, p) if p == "/ops" || p.starts_with("/ops/") => {
            // LG-387: the loopback port's dashboard and beta list, behind this session and its write guard
            let guard = WriteGuard {
                origin: c.cfg.public_origin.clone(),
                csrf: s.csrf.clone(),
                by: format!("the admin web, key #{} from {}", s.key_id, c.ip),
            };
            // LG-430: a release request names the signed-in admin (the key's label) in its file and its event
            if crate::release::handles(p) {
                let name = admin_name(&c, &s);
                let (status, ctype, out) =
                    crate::release::serve_at(shared, &req, head, Some(&guard), Some(&name), now);
                if method == "POST" {
                    event(
                        shared,
                        false,
                        &format!(
                            "release request by {name} with key #{} from {} (answer {status})",
                            s.key_id, c.ip
                        ),
                    );
                }
                return Resp::new(status, ctype, out);
            }
            // OPS-26: a done mark on a bug report names the signed-in admin too
            if crate::triage::handles(p) {
                let name = admin_name(&c, &s);
                let (status, ctype, out) = crate::triage::serve_at(
                    shared,
                    &req,
                    head,
                    body,
                    Some(&guard),
                    Some(&name),
                    now,
                );
                return Resp::new(status, ctype, out);
            }
            let (status, ctype, mut out) =
                crate::opsweb::serve(shared, &req, head, body, Some(&guard));
            // LG-394: who asked for a deploy (the request's own event names the admin web only)
            if method == "POST" && crate::deploy::handles(p) {
                event(
                    shared,
                    false,
                    &format!(
                        "deploy request with key #{} from {} (answer {status})",
                        s.key_id, c.ip
                    ),
                );
            }
            if matches!(p, "/ops" | "/ops/beta") && status == 200 && ctype == crate::opsweb::HTML {
                out = crate::adminfwd::dress(&out, &c.cfg, None, &s.csrf, p == "/ops/beta");
            }
            Resp::new(status, ctype, out)
        }
        (_, p) if p.starts_with(crate::adminfwd::PREFIX) => instance(&c, &s, method, head, p, body),
        _ => Resp::err(404, "not found"),
    }
}

/// LG-399: a request under `/i/<name>` (the session, Origin and CSRF are checked): forwarded to that instance's
/// loopback admin port when the instance is configured and the path is one of its `/ops` paths, deploys excepted.
fn instance(c: &Ctx, s: &Session, method: &str, head: &str, path: &str, body: &[u8]) -> Resp {
    let Some((name, inner)) = crate::adminfwd::split(path) else {
        return Resp::err(404, "not found");
    };
    let Some(inst) = c.cfg.instances.iter().find(|i| i.name == name) else {
        return Resp::err(404, "no such lobby instance");
    };
    if inner.is_empty() && method == "GET" {
        return Resp::redirect(&format!("{}{name}/ops", crate::adminfwd::PREFIX));
    }
    if crate::deploy::handles(inner) {
        return Resp::err(
            403,
            &format!(
                "deploys are for the whole box: use the Updates tab of {}",
                c.cfg.label
            ),
        );
    }
    if !crate::adminfwd::forwardable(inner) {
        return Resp::err(404, "not found");
    }
    if method != "GET" && method != "POST" {
        return Resp::err(405, "method not allowed");
    }
    let raw = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("");
    let Some(target) = crate::adminfwd::inner_target(raw, name) else {
        return Resp::err(400, "bad path");
    };
    let via = format!("the admin web, key #{} from {}", s.key_id, c.ip);
    let res = crate::adminfwd::forward(inst, method, &target, body, &via);
    if method == "POST" {
        let answer = match &res {
            Ok(a) => a.status.to_string(),
            Err((st, m)) => format!("{st}: {m}"),
        };
        event(
            c.shared,
            false,
            &format!(
                "key #{} from {}: POST {inner} on instance {name} (answer {answer})",
                s.key_id, c.ip
            ),
        );
    }
    match res {
        Ok(a) => {
            let page = inner.trim_end_matches('/');
            if a.html && a.status == 200 && matches!(page, "/ops" | "/ops/beta") {
                let out = crate::adminfwd::dress(
                    &a.body,
                    &c.cfg,
                    Some(inst),
                    &s.csrf,
                    page == "/ops/beta",
                );
                Resp::new(200, HTML_TYPE, out)
            } else if a.html {
                // only the two pages are HTML; anything else of that kind is not passed on
                Resp::err(502, "the instance answered with an unexpected page")
            } else {
                Resp::new(a.status, JSON_TYPE, a.body)
            }
        }
        Err((st, m)) => Resp::err(st, &m),
    }
}

fn login_finish(c: &Ctx, body: &[u8]) -> Resp {
    match c.finish_assertion(body, &Kind::Login, "sign-in") {
        Ok(k) => {
            let value = lock(c.web).new_session(k.id, c.now);
            event(
                c.shared,
                false,
                &format!("signed in with {} from {}", key_name(&k), c.ip),
            );
            let max_age = i64::from(c.cfg.session_hours) * 3600;
            Resp::json(200, json!({"ok": true, "redirect": "/ops"}))
                .with(&session_cookie(&value, max_age))
        }
        Err(r) => r,
    }
}

fn enroll_options(c: &Ctx, body: &[u8]) -> Resp {
    let m = match body_json(body) {
        Ok(m) => m,
        Err(r) => return r,
    };
    if let Some(r) = c.limited() {
        return r;
    }
    let token = m
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let (link, ks) = {
        let g = crate::net::lock(c.shared);
        (enroll_link(&g.db, &token, c.now), keys(&g.db))
    };
    let (label, ks) = match (link, ks) {
        (Ok(Some(l)), Ok(k)) => (l, k),
        (Ok(None), _) => {
            return c.refuse(
                403,
                "enrolment",
                "this enrolment link is not valid (used, expired or mistyped): run `dc-lobby-server admin-web enroll` again",
            )
        }
        (Err(e), _) | (_, Err(e)) => return Resp::err(500, &format!("db: {e}")),
    };
    if ks.len() >= c.cfg.max_keys as usize {
        return Resp::err(
            409,
            &format!(
                "{} keys are enrolled already (admin_web.max_keys)",
                ks.len()
            ),
        );
    }
    let (id, ch) = lock(c.web).new_ceremony(Kind::Enroll { token }, c.now);
    Resp::json(
        200,
        json!({"ok": true, "ceremony": id, "label": label, "publicKey": creation_options(&c.cfg, &ch, &ks)}),
    )
}

fn enroll_finish(c: &Ctx, body: &[u8]) -> Resp {
    if let Some(r) = c.finish_gate() {
        return r;
    }
    let token = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("token").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    // the link is used up only by a key that passed every check
    let label = enroll_link(&crate::net::lock(c.shared).db, &token, c.now)
        .ok()
        .flatten()
        .unwrap_or_default();
    let consume = |db: &Connection| {
        match consume_enroll_link(db, &token, c.now) {
        Ok(true) => Ok(()),
        Ok(false) => Err("this enrolment link is not valid any more: run `dc-lobby-server admin-web enroll` again".into()),
        Err(e) => Err(format!("db: {e}")),
    }
    };
    match c.finish_registration(
        body,
        &Kind::Enroll {
            token: token.clone(),
        },
        "enrolment",
        &label,
        &consume,
    ) {
        Ok(k) => Resp::json(
            200,
            json!({"ok": true, "id": k.id, "redirect": "/login",
                   "message": format!("{} is enrolled. Sign in with it now.", k.label)}),
        ),
        Err(r) => r,
    }
}

fn add_key_options(c: &Ctx, kind: Kind) -> Resp {
    if let Some(r) = c.limited() {
        return r;
    }
    let ks = match c.all_keys() {
        Ok(k) => k,
        Err(r) => return r,
    };
    if ks.len() >= c.cfg.max_keys as usize {
        return Resp::err(
            409,
            &format!(
                "{} keys are enrolled already (admin_web.max_keys)",
                ks.len()
            ),
        );
    }
    let (id, ch) = lock(c.web).new_ceremony(kind, c.now);
    Resp::json(
        200,
        json!({"ok": true, "ceremony": id, "publicKey": creation_options(&c.cfg, &ch, &ks)}),
    )
}

fn keys_list(c: &Ctx, s: &Session) -> Resp {
    let ks = match c.all_keys() {
        Ok(k) => k,
        Err(r) => return r,
    };
    let rows: Vec<Value> = ks
        .iter()
        .map(|k| {
            json!({"id": k.id, "label": k.label, "cred": k.short(), "aaguid": k.aaguid, "root": k.root,
                   "alg": if k.alg == webauthn::ALG_EDDSA { "EdDSA" } else { "ES256" },
                   "transports": k.transports, "counter": k.counter, "created": k.created, "last_used": k.last_used,
                   "current": k.id == s.key_id})
        })
        .collect();
    Resp::json(
        200,
        json!({"ok": true, "now": c.now, "max_keys": c.cfg.max_keys, "confirmed_until": s.step_up_until, "keys": rows}),
    )
}

fn key_rename(c: &Ctx, body: &[u8]) -> Resp {
    let m = match body_json(body) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let id = m.get("id").and_then(Value::as_i64).unwrap_or(0);
    let label = clean_label(m.get("label").and_then(Value::as_str).unwrap_or(""));
    if label.is_empty() {
        return Resp::err(400, "give the key a name");
    }
    let r = rename_key(&crate::net::lock(c.shared).db, id, &label);
    match r {
        Ok(true) => {
            event(
                c.shared,
                false,
                &format!("key #{id} renamed to {label:?} from {}", c.ip),
            );
            Resp::json(200, json!({"ok": true}))
        }
        Ok(false) => Resp::err(404, "no such key"),
        Err(e) => Resp::err(500, &format!("db: {e}")),
    }
}

/// LG-384: a key can be removed after a fresh confirmation, never the last one.
fn key_remove(c: &Ctx, body: &[u8], sid: &str, s: &Session) -> Resp {
    let m = match body_json(body) {
        Ok(m) => m,
        Err(r) => return r,
    };
    if s.step_up_until < c.now {
        return Resp::err(403, "confirm with a key you already have first");
    }
    let id = m.get("id").and_then(Value::as_i64).unwrap_or(0);
    let g = crate::net::lock(c.shared);
    let ks = match keys(&g.db) {
        Ok(k) => k,
        Err(e) => return Resp::err(500, &format!("db: {e}")),
    };
    let Some(k) = ks.iter().find(|k| k.id == id).cloned() else {
        return Resp::err(404, "no such key");
    };
    if ks.len() <= 1 {
        return Resp::err(
            409,
            "the last key cannot be removed: enrol another key first (or use `admin-web keys remove` on the server)",
        );
    }
    if let Err(e) = remove_key(&g.db, id) {
        return Resp::err(500, &format!("db: {e}"));
    }
    drop(g);
    let signed_out = s.key_id == id;
    {
        let mut w = lock(c.web);
        w.drop_sessions_of(id);
        if let Some(x) = w.sessions.get_mut(sid) {
            x.step_up_until = 0;
        }
    }
    event(
        c.shared,
        false,
        &format!("{} removed from {}", key_name(&k), c.ip),
    );
    Resp::json(200, json!({"ok": true, "signed_out": signed_out}))
}

// ------------------------------------------------------------------------------------------------- listener

/// LG-382: binds `admin_web.bind`, a loopback address only.
pub fn bind(cfg: &AdminWebSection) -> Result<(SocketAddr, TcpListener), String> {
    let a: SocketAddr = cfg.bind.trim().parse().map_err(|_| {
        format!(
            "admin_web.bind {:?}: give ADDR:PORT, for example 127.0.0.1:10096",
            cfg.bind
        )
    })?;
    if !a.ip().is_loopback() {
        return Err(format!(
            "admin_web.bind {a} is not a loopback address (the tunnel connects locally)"
        ));
    }
    TcpListener::bind(a)
        .map(|l| (a, l))
        .map_err(|e| format!("cannot listen on {a} (admin_web.bind): {e}"))
}

/// A request read with one deadline for the whole request (L2: a client that trickles bytes cannot hold a slot past
/// it); each read waits at most [`READ_WAIT`] and never past the deadline.
struct Deadline<'a> {
    s: &'a TcpStream,
    until: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.s.set_read_timeout(Some(left.min(READ_WAIT)))?;
        let mut s = self.s;
        s.read(buf)
    }
}

/// Longest wait of one read.
const READ_WAIT: Duration = Duration::from_secs(10);
/// Longest time to receive one whole request.
const REQUEST_DEADLINE: Duration = Duration::from_secs(15);

fn serve(shared: Shared, web: Web, mut s: TcpStream, deadline: Duration) {
    let peer = s.peer_addr().ok().map(|a| a.ip());
    let until = Instant::now() + deadline;
    let mut rd = Deadline { s: &s, until };
    let Some((head, rest)) = read_head_and_rest(&mut rd) else {
        return;
    };
    let body = if head.starts_with("POST ") {
        match read_body(&mut rd, &head, rest, MAX_BODY) {
            Ok(b) => b,
            Err(code) => {
                if Instant::now() < until {
                    let _ = s.write_all(&Resp::err(code, "bad or too large body").bytes());
                }
                let _ = s.shutdown(std::net::Shutdown::Both);
                return;
            }
        }
    } else {
        Vec::new()
    };
    // a request still incomplete at the deadline is dropped without an answer
    if Instant::now() >= until {
        let _ = s.shutdown(std::net::Shutdown::Both);
        return;
    }
    let r = handle(&shared, &web, &head, &body, peer, crate::clock::unix_now());
    let _ = s.write_all(&r.bytes());
    let _ = s.flush();
    let _ = s.shutdown(std::net::Shutdown::Both);
}

/// Serves the admin web on a bound listener.
pub fn spawn(shared: Shared, web: Web, l: TcpListener) {
    spawn_with(shared, web, l, REQUEST_DEADLINE)
}

/// [`spawn`] with another request deadline (tests).
fn spawn_with(shared: Shared, web: Web, l: TcpListener, deadline: Duration) {
    let busy = Arc::new(AtomicUsize::new(0));
    let _ = std::thread::Builder::new()
        .name("admin-web-accept".into())
        .spawn(move || {
            for s in l.incoming() {
                match s {
                    Ok(mut stream) => {
                        if busy.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                            let _ = stream.write_all(&Resp::err(503, "busy").bytes());
                            continue;
                        }
                        busy.fetch_add(1, Ordering::SeqCst);
                        let (sh, w, b) = (shared.clone(), web.clone(), Arc::clone(&busy));
                        let spawned =
                            std::thread::Builder::new()
                                .name("admin-web".into())
                                .spawn(move || {
                                    serve(sh, w, stream, deadline);
                                    b.fetch_sub(1, Ordering::SeqCst);
                                });
                        if spawned.is_err() {
                            busy.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    Err(e) => crate::net::lock(&shared).note(&format!("admin-web accept: {e}")),
                }
            }
        });
}

// ------------------------------------------------------------------------------------------------- command line

const USAGE: &str = "usage: dc-lobby-server admin-web enroll [--label NAME]\n\
       dc-lobby-server admin-web keys list\n\
       dc-lobby-server admin-web keys remove ID [--force]\n\
  (also --config FILE / --db FILE as for the lobby itself)";

/// The words, our flags and the lobby's own flags of an `admin-web` command line.
#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub words: Vec<String>,
    pub label: String,
    pub force: bool,
    pub rest: Vec<String>,
}

pub fn split_args(args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut i = 0;
    while i < args.len() {
        let raw = &args[i];
        i += 1;
        let Some(flag) = raw.strip_prefix("--") else {
            a.words.push(raw.clone());
            continue;
        };
        let (name, inline) = match flag.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (flag, None),
        };
        match name {
            "label" => {
                a.label = match inline {
                    Some(v) => v,
                    None => {
                        let v = args.get(i).ok_or("--label needs a value")?.clone();
                        i += 1;
                        v
                    }
                }
            }
            "force" => a.force = true,
            _ => {
                a.rest.push(raw.clone());
                if inline.is_none() && args.get(i).is_some_and(|v| !v.starts_with("--")) {
                    a.rest.push(args[i].clone());
                    i += 1;
                }
            }
        }
    }
    Ok(a)
}

fn when(unix: i64) -> String {
    if unix <= 0 {
        "never".into()
    } else {
        crate::reportmail::utc_text(unix)
    }
}

/// `dc-lobby-server admin-web ...`; the exit code.
pub fn run_cli(args: &[String], env: &[(String, String)]) -> i32 {
    let a = match split_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("dc-lobby-server admin-web: {e}\n{USAGE}");
            return 2;
        }
    };
    let words: Vec<&str> = a.words.iter().map(String::as_str).collect();
    if !matches!(
        words.as_slice(),
        ["enroll"] | ["keys", "list"] | ["keys", "remove", _]
    ) {
        eprintln!("{USAGE}");
        return 2;
    }
    let argv = std::iter::once("dc-lobby-server".to_string()).chain(a.rest.iter().cloned());
    let opts = match crate::cli::parse_full(argv, env) {
        Ok(p) => p.opts,
        Err(crate::cli::CliError::Usage(e)) => {
            let _ = e.print();
            return 2;
        }
        Err(crate::cli::CliError::Config(errs)) => {
            for e in errs {
                eprintln!("dc-lobby-server: config: {e}");
            }
            return 2;
        }
    };
    let db = match crate::db::open(&opts.db) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot open the DB {}: {e}", opts.db.display());
            return 1;
        }
    };
    let cfg = &opts.admin_web;
    let now = crate::clock::unix_now();
    let ks = match keys(&db) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("db: {e}");
            return 1;
        }
    };
    match words.as_slice() {
        ["enroll"] => {
            if ks.len() >= cfg.max_keys as usize {
                eprintln!(
                    "{} keys are enrolled already (admin_web.max_keys = {}): remove one first (admin-web keys remove ID)",
                    ks.len(),
                    cfg.max_keys
                );
                return 1;
            }
            let label = clean_label(&a.label);
            let token = match new_enroll_link(&db, &label, now) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("db: {e}");
                    return 1;
                }
            };
            crate::opsmon::admin_web_event(&db, false, "enrolment link made on the command line");
            println!(
                "One-time enrolment link (one key, valid {} minutes):",
                ENROLL_SECS / 60
            );
            println!();
            println!(
                "  {}/enroll?t={token}",
                cfg.public_origin.trim_end_matches('/')
            );
            println!();
            println!("Open it in a browser with the security key plugged in, press Enrol, allow the browser to show");
            println!("the key's make and model, enter the key's PIN and touch it. Then sign in at {}/login.", cfg.public_origin);
            if !cfg.enabled {
                println!("note: [admin_web] enabled is false in this configuration: the link works once the lobby runs with it on");
            }
            0
        }
        ["keys", "list"] => {
            if ks.is_empty() {
                println!("no security key is enrolled (dc-lobby-server admin-web enroll)");
            }
            for k in &ks {
                println!(
                    "#{:<3} {:<24} cred {}  model {}  {}  attested by {}  enrolled {}  last used {}",
                    k.id,
                    k.label,
                    k.short(),
                    k.aaguid,
                    if k.alg == webauthn::ALG_EDDSA { "EdDSA" } else { "ES256" },
                    k.root,
                    when(k.created),
                    when(k.last_used)
                );
            }
            0
        }
        ["keys", "remove", id] => {
            let Ok(id) = id.trim_start_matches('#').parse::<i64>() else {
                eprintln!("{USAGE}");
                return 2;
            };
            let Some(k) = ks.iter().find(|k| k.id == id) else {
                eprintln!("no key #{id} (admin-web keys list)");
                return 1;
            };
            if ks.len() == 1 && !a.force {
                eprintln!(
                    "#{id} is the last key: nobody could sign in afterwards until a new `admin-web enroll`. Add --force to remove it anyway."
                );
                return 1;
            }
            match remove_key(&db, id) {
                Ok(_) => {
                    crate::opsmon::admin_web_event(
                        &db,
                        false,
                        &format!("{} removed on the command line", key_name(k)),
                    );
                    println!(
                        "removed {} (its sessions end with their next request)",
                        key_name(k)
                    );
                    0
                }
                Err(e) => {
                    eprintln!("db: {e}");
                    1
                }
            }
        }
        _ => 2,
    }
}

#[cfg(test)]
#[path = "adminweb_tests.rs"]
mod tests;
