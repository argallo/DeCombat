//! Diagnostic reports (beta): the lobby side of `docs/DIAGNOSTICS.md` sections 2..6. Our own design, no 2009
//! counterpart.
//!
//! - `POST /diag/report` on the web port takes the client's gzip JSON document (`format` `dcdiag1`), stores the
//!   uploaded bytes unchanged under `<DB folder>/diag_reports/<YYYY-MM>/<N>.dcdiag.gz` and a row in the
//!   `diag_reports` table (made here, `CREATE TABLE IF NOT EXISTS`), and mails a short summary to `mail.reports_to`.
//! - A small UDP echo on the web port number answers the client's connection test.
//! - `dc-lobby-server reports ...` reads the stored reports on the server.
//!
//! The upload is gzip-bomb safe (the inflate is capped at 16 MiB), counted per account / per address / per lobby from
//! the stored rows (so a restart keeps the counts) and stored under a total size cap. No player text reaches a log
//! line beyond a one-line summary, and none reaches a mail header: the subject is fixed words, the id and a kind from
//! a fixed list.
// DC-DEFAULT: LG-360 (proposed, project decision) upload endpoint, limits, storage, mail
// DC-DEFAULT: LG-361 (proposed, project decision) UDP echo of the connection test
// DC-DEFAULT: LG-362 (proposed, project decision) `reports` CLI and retention

use crate::clock;
use crate::logs::Logger;
use crate::mail::Mail;
use crate::reportmail::{self, one_line, utc_text, BODY_MAX, PUMP_EVERY, RETRY_SECS};
use crate::state::{Core, Fault, HResult, Shared};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The upload path.
pub const PATH: &str = "/diag/report";
/// Largest upload body (compressed).
pub const MAX_BODY: usize = 1 << 20;
/// Largest inflated document; more answers 413.
pub const MAX_INFLATED: u64 = 16 << 20;
/// The document's format tag.
pub const FORMAT: &str = "dcdiag1";
/// The kinds a report can have (also the only words of the mail subject that come from the report).
pub const KINDS: [&str; 10] = [
    "link_lost",
    "server_gone",
    "stall",
    "crash",
    "panic",
    "gpu_lost",
    "join_failed",
    "manual",
    "bug_report",
    "nettest",
];
/// A mail send without an answer after this long counts as failed.
const LOST_SECS: i64 = 180;
/// Folder of the stored reports, next to the DB file.
const DIR_NAME: &str = "diag_reports";
/// Rows in the mail's file list.
const MAIL_FILES: usize = 40;

// limits (LG-360): (window seconds, maximum)
const ACCOUNT_HOUR: (i64, i64) = (3600, 6);
const ACCOUNT_DAY: (i64, i64) = (86_400, 20);
const ANON_HOUR: (i64, i64) = (3600, 3);
const ANON_DAY: (i64, i64) = (86_400, 10);
const IP_DAY: (i64, i64) = (86_400, 30);
const LOBBY_DAY: (i64, i64) = (86_400, 300);
const DEFAULT_MAX_TOTAL_MB: u64 = 2048;
const DEFAULT_RETENTION_DAYS: i64 = 30;

// ---------------------------------------------------------------------------------------------
// the service
// ---------------------------------------------------------------------------------------------

struct Pump {
    tx: Sender<(i64, bool)>,
    rx: Receiver<(i64, bool)>,
    /// The row being sent and when it left.
    inflight: Option<(i64, i64)>,
    next_try: i64,
    cap_noted: bool,
}

/// What the endpoint, the mail pump and the purge share: where the files go, the switches and the mail state.
pub struct Service {
    /// `<DB folder>/diag_reports`
    pub dir: PathBuf,
    /// False with `DC_DIAG=off`: the endpoint answers 404 and the echo stays closed.
    pub enabled: bool,
    /// Size cap of all stored reports together (bytes).
    pub max_total: u64,
    pub retention_days: i64,
    pump: Mutex<Pump>,
}

static SERVICE: OnceLock<Service> = OnceLock::new();

/// Sets the process-wide service up (the folder of the lobby DB is `data_dir`); later calls change nothing.
pub fn configure(data_dir: &Path) {
    let _ = SERVICE.get_or_init(|| Service::new(data_dir, &|k| std::env::var(k).ok()));
}

/// The process-wide service; None before [`configure`].
pub fn service() -> Option<&'static Service> {
    SERVICE.get()
}

impl Service {
    pub fn new(data_dir: &Path, env: &dyn Fn(&str) -> Option<String>) -> Service {
        let off = env("DC_DIAG").is_some_and(|v| v.trim().eq_ignore_ascii_case("off"));
        let num = |k: &str| env(k).and_then(|v| v.trim().parse::<u64>().ok());
        let (tx, rx) = channel();
        Service {
            dir: data_dir.join(DIR_NAME),
            enabled: !off,
            max_total: num("DC_DIAG_MAX_TOTAL_MB").unwrap_or(DEFAULT_MAX_TOTAL_MB) << 20,
            retention_days: num("DC_DIAG_RETENTION_DAYS")
                .map_or(DEFAULT_RETENTION_DAYS, |d| d.min(36_500) as i64),
            pump: Mutex::new(Pump {
                tx,
                rx,
                inflight: None,
                next_try: 0,
                cap_noted: false,
            }),
        }
    }
}

/// The report table (made when missing).
pub fn ensure_schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS diag_reports(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            created INTEGER NOT NULL,
            client_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            summary TEXT NOT NULL,
            account TEXT NOT NULL,
            login TEXT NOT NULL,
            ip TEXT NOT NULL,
            version TEXT NOT NULL,
            client_commit TEXT NOT NULL,
            os TEXT NOT NULL,
            gpu TEXT NOT NULL,
            link TEXT NOT NULL,
            bytes INTEGER NOT NULL,
            rel TEXT NOT NULL,
            files TEXT NOT NULL,
            mailed INTEGER NOT NULL DEFAULT 0,
            mailed_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS diag_reports_created ON diag_reports(created);
        CREATE INDEX IF NOT EXISTS diag_reports_account ON diag_reports(account, created);
        CREATE INDEX IF NOT EXISTS diag_reports_ip ON diag_reports(ip, created);
        CREATE INDEX IF NOT EXISTS diag_reports_client ON diag_reports(client_id);",
    )
}

// ---------------------------------------------------------------------------------------------
// the document
// ---------------------------------------------------------------------------------------------

/// Why an upload was not taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Reject {
    /// Not gzip, or the stream is damaged.
    BadGzip,
    /// More than [`MAX_INFLATED`] after inflating.
    TooLarge,
    /// Inflated, but not a JSON object.
    NotJson,
    /// A JSON object that is not a `dcdiag1` report.
    WrongFormat,
    /// `id` is not 16 hex digits.
    BadId,
}

impl Reject {
    pub fn status(&self) -> u16 {
        if *self == Reject::TooLarge {
            413
        } else {
            400
        }
    }
    pub fn text(&self) -> &'static str {
        match self {
            Reject::BadGzip => "not a gzip document",
            Reject::TooLarge => "the document is too large",
            Reject::NotJson => "not a JSON object",
            Reject::WrongFormat => "not a dcdiag1 report",
            Reject::BadId => "bad report id",
        }
    }
}

/// Inflates `gz`, at most `cap` bytes out (zip-bomb safe: the reader stops one byte past the cap).
pub fn inflate(gz: &[u8], cap: u64) -> Result<Vec<u8>, Reject> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .take(cap + 1)
        .read_to_end(&mut out)
        .map_err(|_| Reject::BadGzip)?;
    if out.len() as u64 > cap {
        return Err(Reject::TooLarge);
    }
    Ok(out)
}

/// What the lobby keeps of a report besides the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    pub client_id: String,
    pub kind: String,
    pub summary: String,
    pub version: String,
    pub commit: String,
    pub os: String,
    pub gpu: String,
    pub login: String,
    pub link: String,
    /// File names and their text sizes in bytes.
    pub files: Vec<(String, usize)>,
}

fn text_at(v: &Value, path: &[&str], max: usize) -> String {
    let mut cur = v;
    for k in path {
        match cur.get(k) {
            Some(n) => cur = n,
            None => return String::new(),
        }
    }
    let raw = match cur {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    };
    one_line(&raw).trim().chars().take(max).collect()
}

/// The link object as one line of `key=value` pairs (scalars only).
fn link_line(v: &Value) -> String {
    let Some(o) = v.get("link").and_then(|l| l.as_object()) else {
        return String::new();
    };
    let mut out = String::new();
    for (k, val) in o {
        let t = match val {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            _ => continue,
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("{}={}", one_line(k), one_line(&t)));
    }
    out.chars().take(300).collect()
}

/// Reads the inflated bytes: a JSON object with `format == "dcdiag1"` and a 16-hex-digit `id`.
pub fn parse_report(raw: &[u8]) -> Result<Meta, Reject> {
    let v: Value = serde_json::from_slice(raw).map_err(|_| Reject::NotJson)?;
    if !v.is_object() {
        return Err(Reject::NotJson);
    }
    if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(Reject::WrongFormat);
    }
    let id = v.get("id").and_then(Value::as_str).unwrap_or("");
    if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Reject::BadId);
    }
    let kind = text_at(&v, &["kind"], 40);
    let kind = if KINDS.contains(&kind.as_str()) {
        kind
    } else {
        "other".to_string()
    };
    let files = v
        .get("files")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|f| {
                    (
                        text_at(f, &["name"], 100),
                        f.get("text").and_then(Value::as_str).map_or(0, str::len),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Meta {
        client_id: id.to_ascii_lowercase(),
        kind,
        summary: text_at(&v, &["summary"], 200),
        version: text_at(&v, &["client", "version"], 40),
        commit: text_at(&v, &["client", "commit"], 40),
        os: text_at(&v, &["system", "os"], 80),
        gpu: text_at(&v, &["system", "gpu"], 120),
        login: text_at(&v, &["login"], 64),
        link: link_line(&v),
        files,
    })
}

// ---------------------------------------------------------------------------------------------
// the HTTP side
// ---------------------------------------------------------------------------------------------

/// True for the upload path.
pub fn handles(path: &str) -> bool {
    path.trim_end_matches('/') == PATH
}

fn reply(status: u16, msg: &str, retry: Option<i64>) -> (u16, String) {
    let j = match retry {
        Some(r) => serde_json::json!({"error": msg, "retry_after": r}),
        None => serde_json::json!({"error": msg}),
    };
    (status, j.to_string())
}

fn http(r: (u16, String)) -> Vec<u8> {
    crate::clanweb::response_bytes(r.0, "application/json", r.1.as_bytes())
}

/// The answer that needs no body: 404 when the endpoint is off, 405 for a method other than POST. None = read the
/// body and call [`respond`].
pub fn gate(method: &str) -> Option<Vec<u8>> {
    match service() {
        Some(s) if s.enabled => {}
        _ => return Some(http(reply(404, "not found", None))),
    }
    (method != "POST").then(|| http(reply(405, "use POST", None)))
}

/// The answer to a body that could not be read (413 above the cap, else 400).
pub fn body_error(code: u16) -> Vec<u8> {
    if code == 413 {
        http(reply(413, "the document is too large", None))
    } else {
        http(reply(400, "bad request", None))
    }
}

/// The whole answer (HTTP bytes) to an upload: `head` the request head, `body` the gzip document, `peer` the
/// source `ip:port`.
pub fn respond(shared: &Shared, head: &str, body: &[u8], peer: &str) -> Vec<u8> {
    match service() {
        Some(svc) => http(handle(shared, svc, head, body, peer, clock::unix_now())),
        None => http(reply(404, "not found", None)),
    }
}

/// The source address without its port.
pub fn peer_ip(peer: &str) -> String {
    match peer.parse::<SocketAddr>() {
        Ok(a) => a.ip().to_string(),
        Err(_) => peer.rsplit_once(':').map_or(peer, |x| x.0).to_string(),
    }
}

/// The value of header `name` of a request head.
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// Who sent a report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Who {
    /// `<issuer>:<account>` of a token this lobby accepts; empty = anonymous.
    pub account: String,
    pub login: String,
}

fn who_of(auth: &crate::identity::Authority, head: &str, now: i64) -> Who {
    let token = header(head, "authorization")
        .and_then(|v| {
            let (scheme, rest) = v.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
        })
        .filter(|t| !t.is_empty() && t.len() <= crate::identity::MAX_TOKEN);
    match token {
        Some(t) if auth.accepts() => match auth.check(t, now) {
            Ok(c) => Who {
                account: format!("{}:{}", c.issuer, c.account),
                login: one_line(&c.login).chars().take(64).collect(),
            },
            Err(_) => Who::default(),
        },
        _ => Who::default(),
    }
}

/// A refused upload.
#[derive(Debug, PartialEq, Eq)]
pub struct Denied {
    pub status: u16,
    pub msg: &'static str,
    pub retry: Option<i64>,
}

enum Outcome {
    Stored(i64),
    Same(i64),
    Denied(Denied),
}

/// Handles one upload. `now` is unix seconds. Inflating and parsing happen outside the state lock.
pub fn handle(
    shared: &Shared,
    svc: &Service,
    head: &str,
    body: &[u8],
    peer: &str,
    now: i64,
) -> (u16, String) {
    if !svc.enabled {
        return reply(404, "not found", None);
    }
    if body.is_empty() {
        return reply(400, Reject::BadGzip.text(), None);
    }
    let ip = peer_ip(peer);
    let (who, pre) = {
        let g = crate::net::lock(shared);
        let who = who_of(&g.identity, head, now);
        // cheap refusal before the expensive inflate
        let pre = ensure_schema(&g.db).and_then(|_| svc.admit(&g.db, &who, &ip, body.len(), now));
        (who, pre)
    };
    match pre {
        Err(_) => return reply(500, "store error", None),
        Ok(Some(d)) => return reply(d.status, d.msg, d.retry),
        Ok(None) => {}
    }
    let meta = match inflate(body, MAX_INFLATED).and_then(|raw| parse_report(&raw)) {
        Ok(m) => m,
        Err(r) => return reply(r.status(), r.text(), None),
    };
    let mut g = crate::net::lock(shared);
    let mut out = None;
    let r = g.scope(|st| {
        let stored = svc.store(st, &meta, &who, &ip, body, now)?;
        // MON-3: a new report shows in the uploader's timeline (`<issuer>:<account>`; anonymous ones do not)
        if let Outcome::Stored(id) = &stored {
            if let Some(user) = who.account.rsplit(':').next().and_then(|a| a.parse::<u32>().ok()) {
                crate::opsmon::diagnostics_uploaded(st, user, &id.to_string(), &meta.kind);
            }
        }
        out = Some(stored);
        Ok(())
    });
    crate::net::report(&g, r);
    match out {
        Some(Outcome::Stored(id)) => {
            g.note(&format!(
                "diag: report #{id} stored ({}, {} bytes): {}",
                meta.kind,
                body.len(),
                meta.summary
            ));
            (200, format!("{{\"id\":{id}}}"))
        }
        Some(Outcome::Same(id)) => (200, format!("{{\"id\":{id}}}")),
        Some(Outcome::Denied(d)) => reply(d.status, d.msg, d.retry),
        None => reply(500, "store error", None),
    }
}

impl Service {
    /// The limit, if any, that refuses another report of `who` from `ip` with `len` more bytes.
    pub fn admit(
        &self,
        db: &Connection,
        who: &Who,
        ip: &str,
        len: usize,
        now: i64,
    ) -> rusqlite::Result<Option<Denied>> {
        let mut hits: Vec<(&'static str, i64)> = Vec::new();
        let mut check = |msg: &'static str, cond: &str, key: &str, lim: (i64, i64)| -> rusqlite::Result<()> {
            if let Some(t) = limit_hit(db, cond, key, lim, now)? {
                hits.push((msg, t));
            }
            Ok(())
        };
        if who.account.is_empty() {
            check("too many reports from this address this hour", "account = '' AND ip = ?1", ip, ANON_HOUR)?;
            check("too many reports from this address today", "account = '' AND ip = ?1", ip, ANON_DAY)?;
        } else {
            check("too many reports from this account this hour", "account = ?1", &who.account, ACCOUNT_HOUR)?;
            check("too many reports from this account today", "account = ?1", &who.account, ACCOUNT_DAY)?;
        }
        check("too many reports from this address today", "ip = ?1", ip, IP_DAY)?;
        check("the lobby takes no more reports today", "?1 = ?1", "", LOBBY_DAY)?;
        if let Some(first) = hits.first() {
            let retry = hits.iter().map(|h| h.1).max().unwrap_or(first.1);
            return Ok(Some(Denied {
                status: 429,
                msg: first.0,
                retry: Some(retry),
            }));
        }
        let total: i64 = db.query_row("SELECT COALESCE(SUM(bytes), 0) FROM diag_reports", [], |r| r.get(0))?;
        if (total.max(0) as u64).saturating_add(len as u64) > self.max_total {
            return Ok(Some(Denied {
                status: 503,
                msg: "the report store is full",
                retry: None,
            }));
        }
        Ok(None)
    }

    /// Dedupe, limits, the row and the file, in the caller's transaction.
    fn store(
        &self,
        st: &mut Core,
        m: &Meta,
        who: &Who,
        ip: &str,
        body: &[u8],
        now: i64,
    ) -> Result<Outcome, Fault> {
        ensure_schema(&st.db)?;
        // a repeated upload (a lost answer) is the first row
        let same: Option<i64> = if who.account.is_empty() {
            st.db
                .query_row(
                    "SELECT id FROM diag_reports WHERE client_id = ?1 AND account = '' AND ip = ?2 ORDER BY id LIMIT 1",
                    params![m.client_id, ip],
                    |r| r.get(0),
                )
                .optional()?
        } else {
            st.db
                .query_row(
                    "SELECT id FROM diag_reports WHERE client_id = ?1 AND account = ?2 ORDER BY id LIMIT 1",
                    params![m.client_id, who.account],
                    |r| r.get(0),
                )
                .optional()?
        };
        if let Some(id) = same {
            return Ok(Outcome::Same(id));
        }
        if let Some(d) = self.admit(&st.db, who, ip, body.len(), now)? {
            return Ok(Outcome::Denied(d));
        }
        let login = if who.login.is_empty() { &m.login } else { &who.login };
        let mailed = if reportmail::enabled(st) { 0 } else { 2 };
        let files: Vec<Value> = m
            .files
            .iter()
            .map(|(n, s)| serde_json::json!({"name": n, "size": s}))
            .collect();
        st.db.execute(
            "INSERT INTO diag_reports(created, client_id, kind, summary, account, login, ip, version, client_commit, os, gpu, link, bytes, rel, files, mailed) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, '', ?14, ?15)",
            params![
                now,
                m.client_id,
                m.kind,
                m.summary,
                who.account,
                login,
                ip,
                m.version,
                m.commit,
                m.os,
                m.gpu,
                m.link,
                body.len() as i64,
                Value::Array(files).to_string(),
                mailed
            ],
        )?;
        let id = st.db.last_insert_rowid();
        let month = utc_text(now).chars().take(7).collect::<String>();
        let rel = format!("{month}/{id}.dcdiag.gz");
        let path = self.dir.join(&month).join(format!("{id}.dcdiag.gz"));
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| Fault::Other(format!("diag: {}: {e}", d.display())))?;
        }
        std::fs::write(&path, body).map_err(|e| Fault::Other(format!("diag: {}: {e}", path.display())))?;
        st.db
            .execute("UPDATE diag_reports SET rel = ?1 WHERE id = ?2", params![rel, id])?;
        Ok(Outcome::Stored(id))
    }
}

/// When `max` reports matching `cond` are already inside the last `win` seconds: the unix time the oldest of them
/// leaves the window, as seconds from `now` (at least 1). None = room left.
fn limit_hit(
    db: &Connection,
    cond: &str,
    key: &str,
    (win, max): (i64, i64),
    now: i64,
) -> rusqlite::Result<Option<i64>> {
    let sql = format!(
        "SELECT created FROM diag_reports WHERE ({cond}) AND created > ?2 ORDER BY created DESC LIMIT 1 OFFSET ?3"
    );
    let t: Option<i64> = db
        .query_row(&sql, params![key, now - win, max - 1], |r| r.get(0))
        .optional()?;
    Ok(t.map(|t| (t + win - now).max(1)))
}

// ---------------------------------------------------------------------------------------------
// mail, retention, background thread
// ---------------------------------------------------------------------------------------------

/// A stored row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stored {
    pub id: i64,
    pub created: i64,
    pub client_id: String,
    pub kind: String,
    pub summary: String,
    pub account: String,
    pub login: String,
    pub ip: String,
    pub version: String,
    pub commit: String,
    pub os: String,
    pub gpu: String,
    pub link: String,
    pub bytes: i64,
    pub rel: String,
    pub files: String,
    pub mailed: i64,
}

const COLS: &str =
    "id, created, client_id, kind, summary, account, login, ip, version, client_commit, os, gpu, link, bytes, rel, files, mailed";

fn stored_of(r: &rusqlite::Row) -> rusqlite::Result<Stored> {
    Ok(Stored {
        id: r.get(0)?,
        created: r.get(1)?,
        client_id: r.get(2)?,
        kind: r.get(3)?,
        summary: r.get(4)?,
        account: r.get(5)?,
        login: r.get(6)?,
        ip: r.get(7)?,
        version: r.get(8)?,
        commit: r.get(9)?,
        os: r.get(10)?,
        gpu: r.get(11)?,
        link: r.get(12)?,
        bytes: r.get(13)?,
        rel: r.get(14)?,
        files: r.get(15)?,
        mailed: r.get(16)?,
    })
}

/// The file list of a row: names and sizes.
fn file_list(files_json: &str) -> Vec<(String, i64)> {
    serde_json::from_str::<Vec<Value>>(files_json)
        .unwrap_or_default()
        .iter()
        .map(|f| {
            (
                f.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                f.get("size").and_then(Value::as_i64).unwrap_or(0),
            )
        })
        .collect()
}

/// The mail subject: fixed words, the id and the kind (from the fixed list).
pub fn mail_subject(r: &Stored) -> String {
    let kind = if KINDS.contains(&r.kind.as_str()) { r.kind.as_str() } else { "other" };
    format!("Diagnostic report #{}: {kind}", r.id)
}

/// The mail body, at most [`BODY_MAX`] characters, every player text as one clean line.
pub fn mail_body(r: &Stored) -> String {
    let who = if r.account.is_empty() {
        format!("anonymous (login typed: {})", one_line(&r.login))
    } else {
        format!("{} (login {})", one_line(&r.account), one_line(&r.login))
    };
    let mut out = format!(
        "Diagnostic report #{}\n\nTime (UTC): {}\nKind: {}\nSummary: {}\nAccount: {}\nClient: version {} commit {}\nOS: {}\nGPU: {}\nLink: {}\nSize: {} bytes (gzip)\n\nFiles:\n",
        r.id,
        utc_text(r.created),
        one_line(&r.kind),
        one_line(&r.summary),
        who,
        one_line(&r.version),
        one_line(&r.commit),
        one_line(&r.os),
        one_line(&r.gpu),
        one_line(&r.link),
        r.bytes,
    );
    let files = file_list(&r.files);
    for (n, s) in files.iter().take(MAIL_FILES) {
        out.push_str(&format!("  {} ({} bytes)\n", one_line(n), s));
    }
    if files.len() > MAIL_FILES {
        out.push_str(&format!("  ... {} more\n", files.len() - MAIL_FILES));
    }
    if files.is_empty() {
        out.push_str("  (none)\n");
    }
    let tail = format!("\nRead it on the server: dc-lobby-server reports show {}\n", r.id);
    if out.chars().count() + tail.len() > BODY_MAX {
        out = out.chars().take(BODY_MAX - tail.len() - 7).collect();
        out.push_str("\n[cut]\n");
    }
    out.push_str(&tail);
    out
}

impl Service {
    /// Takes the mail worker's answers, then hands the oldest waiting row to it unless one is on its way, the last
    /// send failed a moment ago or the hour's mails are used up. Never waits on the network.
    pub fn pump(&self, st: &mut Core) -> HResult {
        if !self.enabled || !reportmail::enabled(st) {
            return Ok(());
        }
        ensure_schema(&st.db)?;
        let mut p = self.pump.lock().unwrap_or_else(|e| e.into_inner());
        let now = clock::unix_now();
        while let Ok((id, ok)) = p.rx.try_recv() {
            if p.inflight.map(|x| x.0) == Some(id) {
                p.inflight = None;
            }
            if ok {
                st.db.execute(
                    "UPDATE diag_reports SET mailed = 1, mailed_at = ?1 WHERE id = ?2",
                    params![now, id],
                )?;
            } else {
                p.next_try = now + RETRY_SECS;
                st.note(&format!(
                    "diag report #{id}: the mail failed; it stays in the table, next try in {RETRY_SECS} s"
                ));
            }
        }
        if let Some((id, since)) = p.inflight {
            if now - since < LOST_SECS {
                return Ok(());
            }
            st.note(&format!("diag report #{id}: no answer from the mail thread; will retry"));
            p.inflight = None;
            p.next_try = now + RETRY_SECS;
        }
        if now < p.next_try {
            return Ok(());
        }
        let cap = st.mail.opts.reports_per_hour as i64;
        let sent: i64 = st.db.query_row(
            "SELECT COUNT(*) FROM diag_reports WHERE mailed = 1 AND mailed_at > ?1",
            [now - 3600],
            |r| r.get(0),
        )?;
        if sent >= cap {
            if !p.cap_noted {
                p.cap_noted = true;
                st.note(&format!(
                    "diag report mails: hourly cap {cap} reached; the rest wait in the table"
                ));
            }
            return Ok(());
        }
        p.cap_noted = false;
        let row = st
            .db
            .query_row(
                &format!("SELECT {COLS} FROM diag_reports WHERE mailed = 0 ORDER BY id LIMIT 1"),
                [],
                stored_of,
            )
            .optional()?;
        let Some(row) = row else {
            return Ok(());
        };
        let mail = Mail {
            to: st.mail.opts.reports_to.trim().to_string(),
            subject: st.mail.opts.subject(&mail_subject(&row)),
            body: mail_body(&row),
            what: format!("diag report #{}", row.id),
            ..Default::default()
        };
        let ack = (row.id, p.tx.clone());
        if st.mail.send_with(mail, Some(ack)) {
            p.inflight = Some((row.id, now));
        } else {
            p.next_try = now + RETRY_SECS;
        }
        Ok(())
    }

    /// Deletes the rows and files older than the retention. Returns how many reports went.
    pub fn purge(&self, db: &Connection, now: i64) -> rusqlite::Result<usize> {
        ensure_schema(db)?;
        let cut = now - self.retention_days * 86_400;
        let old: Vec<(i64, String)> = {
            let mut q = db.prepare("SELECT id, rel FROM diag_reports WHERE created < ?1")?;
            let rows = q.query_map([cut], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, rel) in &old {
            if let Some(path) = self.file_of(rel) {
                let _ = std::fs::remove_file(&path);
                if let Some(d) = path.parent() {
                    let _ = std::fs::remove_dir(d); // only when it is empty now
                }
            }
            db.execute("DELETE FROM diag_reports WHERE id = ?1", [id])?;
        }
        Ok(old.len())
    }

    /// The file of a stored relative name (None for a name that would leave the folder).
    fn file_of(&self, rel: &str) -> Option<PathBuf> {
        file_under(&self.dir, rel)
    }
}

fn file_under(dir: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.split('/').any(|p| p.is_empty() || p == "." || p == ".." || p.contains('\\') || p.contains(':')) {
        return None;
    }
    Some(rel.split('/').fold(dir.to_path_buf(), |a, p| a.join(p)))
}

/// The retention purge at the start and every hour, and the mail pump every [`PUMP_EVERY`]. No thread when the
/// service is off.
pub fn spawn_background(shared: Shared) {
    let Some(svc) = service().filter(|s| s.enabled) else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("diag".into())
        .spawn(move || {
            let mut next_purge = 0i64;
            loop {
                {
                    let mut g = crate::net::lock(&shared);
                    let now = clock::unix_now();
                    if now >= next_purge {
                        next_purge = now + 3600;
                        match svc.purge(&g.db, now) {
                            Ok(0) => {}
                            Ok(n) => g.note(&format!("diag: {n} reports past {} days deleted", svc.retention_days)),
                            Err(e) => g.note(&format!("diag: purge failed: {e}")),
                        }
                    }
                    let r = g.scope(|st| svc.pump(st));
                    crate::net::report(&g, r);
                }
                std::thread::sleep(PUMP_EVERY);
            }
        });
}

// ---------------------------------------------------------------------------------------------
// the UDP echo (LG-361)
// ---------------------------------------------------------------------------------------------

/// Smallest and largest request.
pub const ECHO_MIN: usize = 16;
pub const ECHO_MAX: usize = 8192;
const PER_IP_DGRAMS: f64 = 40.0;
const PER_IP_BYTES: f64 = 160_000.0;
const BURST_SECS: f64 = 2.0;
const GLOBAL_DGRAMS: f64 = 4000.0;
/// Most source addresses tracked at once.
const MAX_TRACKED: usize = 4096;

/// The reply to a request, None = drop it: 16..=8192 bytes, `DCPR`, version 1; the reply is `DCPA`, bytes 4..16
/// copied and zeros, exactly as long as the request.
pub fn echo_reply(req: &[u8]) -> Option<Vec<u8>> {
    if !(ECHO_MIN..=ECHO_MAX).contains(&req.len()) || &req[..4] != b"DCPR" || req[4] != 1 {
        return None;
    }
    let mut out = vec![0u8; req.len()];
    out[..4].copy_from_slice(b"DCPA");
    out[4..16].copy_from_slice(&req[4..16]);
    Some(out)
}

struct Bucket {
    dgrams: f64,
    bytes: f64,
    last: Duration,
}

/// Token buckets per source address (40 datagrams and 160 KB a second, a burst of 2 s) and one for all sources
/// (4000 datagrams a second). The caller supplies the clock (time since any fixed start).
pub struct RateLimiter {
    per_ip: HashMap<IpAddr, Bucket>,
    global: Bucket,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub fn new() -> RateLimiter {
        RateLimiter {
            per_ip: HashMap::new(),
            global: Bucket {
                dgrams: GLOBAL_DGRAMS,
                bytes: 0.0,
                last: Duration::ZERO,
            },
        }
    }

    /// Number of addresses tracked now (tests).
    #[cfg(test)]
    pub fn tracked(&self) -> usize {
        self.per_ip.len()
    }

    /// Whether a datagram of `bytes` from `ip` at `now` may be answered; counts it when so.
    pub fn allow(&mut self, ip: IpAddr, bytes: usize, now: Duration) -> bool {
        let bytes = bytes as f64;
        if !self.per_ip.contains_key(&ip) {
            if self.per_ip.len() >= MAX_TRACKED {
                // idle for longer than the burst = full again = the same as a new address
                self.per_ip
                    .retain(|_, b| now.saturating_sub(b.last).as_secs_f64() < BURST_SECS);
                if self.per_ip.len() >= MAX_TRACKED {
                    return false;
                }
            }
            self.per_ip.insert(
                ip,
                Bucket {
                    dgrams: PER_IP_DGRAMS * BURST_SECS,
                    bytes: PER_IP_BYTES * BURST_SECS,
                    last: now,
                },
            );
        }
        let b = self.per_ip.get_mut(&ip).expect("inserted");
        let dt = now.saturating_sub(b.last).as_secs_f64();
        b.last = b.last.max(now);
        b.dgrams = (b.dgrams + dt * PER_IP_DGRAMS).min(PER_IP_DGRAMS * BURST_SECS);
        b.bytes = (b.bytes + dt * PER_IP_BYTES).min(PER_IP_BYTES * BURST_SECS);
        let g = &mut self.global;
        let gdt = now.saturating_sub(g.last).as_secs_f64();
        g.last = g.last.max(now);
        g.dgrams = (g.dgrams + gdt * GLOBAL_DGRAMS).min(GLOBAL_DGRAMS);
        if b.dgrams < 1.0 || b.bytes < bytes || g.dgrams < 1.0 {
            return false;
        }
        b.dgrams -= 1.0;
        b.bytes -= bytes;
        g.dgrams -= 1.0;
        true
    }
}

/// The whole decision for one datagram: the reply, or None to drop it.
pub fn echo_decide(
    limiter: &mut RateLimiter,
    req: &[u8],
    src: SocketAddr,
    now: Duration,
) -> Option<Vec<u8>> {
    if src.port() < 1024 {
        return None;
    }
    let rep = echo_reply(req)?;
    limiter.allow(src.ip(), req.len(), now).then_some(rep)
}

/// Serves the echo on `sock` until the process ends.
pub fn echo_serve(sock: UdpSocket) {
    let start = Instant::now();
    let mut limiter = RateLimiter::new();
    // a datagram bigger than the buffer would be an error on Windows; leave room for the largest possible one
    let mut buf = vec![0u8; 65_536];
    loop {
        match sock.recv_from(&mut buf) {
            Ok((n, src)) => {
                if let Some(rep) = echo_decide(&mut limiter, &buf[..n], src, start.elapsed()) {
                    let _ = sock.send_to(&rep, src);
                }
            }
            // Windows reports an ICMP "port unreachable" of an earlier reply here; nothing to do
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// Starts the echo on `bind:port` over UDP (the web port number). Not when the service is off or unconfigured.
pub fn spawn_echo(bind: &str, port: u16, log: Arc<Logger>) {
    if !service().is_some_and(|s| s.enabled) {
        return;
    }
    match UdpSocket::bind((bind, port)) {
        Ok(sock) => {
            log.line(&format!("diag: connection-test echo on {bind}:{port}/udp"));
            let _ = std::thread::Builder::new()
                .name("diag-echo".into())
                .spawn(move || echo_serve(sock));
        }
        Err(e) => log.line(&format!(
            "diag: cannot open the connection-test echo on {bind}:{port}/udp: {e}"
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// the CLI (LG-362)
// ---------------------------------------------------------------------------------------------

const USAGE: &str = "usage: dc-lobby-server reports [--db FILE] list [--limit N] [--kind KIND]\n       dc-lobby-server reports [--db FILE] show ID [--files]\n       dc-lobby-server reports [--db FILE] fetch ID [--out DIR]";

/// A text for a terminal: no control characters but line breaks and tabs (no escape sequences from player text).
fn terminal_safe(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .map(|c| if c.is_control() && c != '\n' && c != '\t' { ' ' } else { c })
        .collect()
}

/// A file name that stays inside the folder: `[A-Za-z0-9._-]`, no `..`, not hidden, not a device name.
pub fn safe_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect();
    while s.contains("..") {
        s = s.replace("..", ".");
    }
    let mut s: String = s.trim_start_matches('.').chars().take(100).collect();
    if s.is_empty() {
        s = "file".into();
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.ends_with(|c: char| c.is_ascii_digit()));
    if reserved {
        s.insert(0, '_');
    }
    s
}

fn row_by_id(db: &Connection, id: i64) -> Result<Stored, String> {
    db.query_row(&format!("SELECT {COLS} FROM diag_reports WHERE id = ?1"), [id], stored_of)
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no report #{id}"))
}

/// The stored document of a row, parsed.
fn read_doc(base: &Path, r: &Stored) -> Result<Value, String> {
    let path = file_under(base, &r.rel).ok_or_else(|| format!("report #{}: bad file name in the table", r.id))?;
    let gz = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let raw = inflate(&gz, MAX_INFLATED).map_err(|e| format!("{}: {}", path.display(), e.text()))?;
    serde_json::from_slice(&raw).map_err(|e| format!("{}: {e}", path.display()))
}

/// `list`: newest first.
pub fn list_text(db: &Connection, limit: i64, kind: Option<&str>) -> Result<String, String> {
    let mut q = db
        .prepare(&format!(
            "SELECT {COLS} FROM diag_reports WHERE (?1 IS NULL OR kind = ?1) ORDER BY id DESC LIMIT ?2"
        ))
        .map_err(|e| e.to_string())?;
    let rows = q
        .query_map(params![kind, limit], stored_of)
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let mut out = String::new();
    for r in &rows {
        let who = if r.account.is_empty() {
            format!("-/{}", one_line(&r.login))
        } else {
            format!("{}/{}", one_line(&r.account), one_line(&r.login))
        };
        out.push_str(&format!(
            "#{}  {}  {}  {}  {}  {} B  {}\n",
            r.id,
            utc_text(r.created),
            r.kind,
            who,
            if r.version.is_empty() { "-" } else { &r.version },
            r.bytes,
            r.summary
        ));
    }
    if rows.is_empty() {
        out.push_str("no reports\n");
    }
    Ok(out)
}

/// `show`: the row, the document's meta keys and the file list; the files' text with `with_files`.
pub fn show_text(db: &Connection, base: &Path, id: i64, with_files: bool) -> Result<String, String> {
    let r = row_by_id(db, id)?;
    let mut out = format!(
        "Report #{}\ntime (UTC): {}\nclient id:  {}\nkind:       {}\nsummary:    {}\naccount:    {}\nlogin:      {}\nfrom:       {}\nversion:    {} {}\nOS:         {}\nGPU:        {}\nlink:       {}\nsize:       {} bytes\nmailed:     {}\nfile:       {}\n",
        r.id,
        utc_text(r.created),
        r.client_id,
        r.kind,
        r.summary,
        if r.account.is_empty() { "(anonymous)" } else { &r.account },
        r.login,
        r.ip,
        r.version,
        r.commit,
        r.os,
        r.gpu,
        r.link,
        r.bytes,
        match r.mailed {
            1 => "yes",
            2 => "no (mail was off)",
            _ => "waiting",
        },
        r.rel,
    );
    match read_doc(base, &r) {
        Ok(doc) => {
            for key in ["client", "system", "settings", "match", "link", "nettest", "note"] {
                if let Some(v) = doc.get(key) {
                    let t = serde_json::to_string_pretty(v).unwrap_or_default();
                    out.push_str(&format!("\n{key}:\n{t}\n"));
                }
            }
            let files = doc.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
            out.push_str(&format!("\nfiles ({}):\n", files.len()));
            for f in &files {
                let n = f.get("name").and_then(Value::as_str).unwrap_or("");
                let t = f.get("text").and_then(Value::as_str).unwrap_or("");
                out.push_str(&format!("  {} ({} bytes)\n", one_line(n), t.len()));
            }
            if with_files {
                for f in &files {
                    let n = f.get("name").and_then(Value::as_str).unwrap_or("");
                    let t = f.get("text").and_then(Value::as_str).unwrap_or("");
                    out.push_str(&format!("\n==== {} ====\n{}\n", one_line(n), t));
                }
            }
        }
        Err(e) => out.push_str(&format!("\n(the document cannot be read: {e})\n")),
    }
    Ok(terminal_safe(&out))
}

/// `fetch`: writes `<out>/report_<ID>/` with `report.json`, one file per entry and the original `.dcdiag.gz`.
pub fn fetch_to(db: &Connection, base: &Path, id: i64, out: &Path) -> Result<String, String> {
    let r = row_by_id(db, id)?;
    let doc = read_doc(base, &r)?;
    let dir = out.join(format!("report_{id}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut meta = doc.clone();
    if let Some(o) = meta.as_object_mut() {
        o.remove("files");
    }
    let put = |name: &str, data: &[u8]| -> Result<(), String> {
        let p = dir.join(name);
        std::fs::write(&p, data).map_err(|e| format!("{}: {e}", p.display()))
    };
    put("report.json", serde_json::to_string_pretty(&meta).unwrap_or_default().as_bytes())?;
    let mut used = std::collections::HashSet::new();
    used.insert("report.json".to_string());
    let gz_name = format!("{id}.dcdiag.gz");
    used.insert(gz_name.to_ascii_lowercase());
    let mut n = 2;
    for f in doc.get("files").and_then(Value::as_array).into_iter().flatten() {
        let raw = f.get("name").and_then(Value::as_str).unwrap_or("");
        let mut name = safe_name(raw);
        while !used.insert(name.to_ascii_lowercase()) {
            name = format!("{}_{n}", safe_name(raw));
            n += 1;
        }
        put(&name, f.get("text").and_then(Value::as_str).unwrap_or("").as_bytes())?;
    }
    let src = file_under(base, &r.rel).ok_or("bad file name in the table")?;
    std::fs::copy(&src, dir.join(&gz_name)).map_err(|e| format!("{}: {e}", src.display()))?;
    Ok(format!("wrote {} ({} files)", dir.display(), used.len()))
}

/// `dc-lobby-server reports ...`; the exit code.
pub fn run_subcommand(args: &[String]) -> i32 {
    let mut db: Option<PathBuf> = None;
    let mut limit = 20i64;
    let mut kind: Option<String> = None;
    let mut files = false;
    let mut out_dir: Option<PathBuf> = None;
    let mut pos: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (k, inline) = match a.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        let mut val = || inline.clone().or_else(|| it.next().cloned());
        match k {
            "--db" => db = val().map(PathBuf::from),
            "--limit" => match val().and_then(|v| v.parse::<i64>().ok()).filter(|n| *n > 0) {
                Some(n) => limit = n,
                None => {
                    eprintln!("dc-lobby-server reports: --limit needs a positive number");
                    return 2;
                }
            },
            "--kind" => kind = val(),
            "--out" => out_dir = val().map(PathBuf::from),
            "--files" => files = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            o if o.starts_with("--") => {
                eprintln!("dc-lobby-server reports: unknown argument {o:?}\n{USAGE}");
                return 2;
            }
            _ => pos.push(a.clone()),
        }
    }
    let path = db.unwrap_or_else(crate::cli::default_db);
    let id_arg = |pos: &[String]| -> Result<i64, String> {
        pos.get(1)
            .and_then(|s| s.trim_start_matches('#').parse::<i64>().ok())
            .ok_or_else(|| format!("{} needs a report number\n{USAGE}", pos[0]))
    };
    let Some(cmd) = pos.first().map(String::as_str) else {
        eprintln!("{USAGE}");
        return 2;
    };
    if !matches!(cmd, "list" | "show" | "fetch") {
        eprintln!("dc-lobby-server reports: unknown command {cmd:?}\n{USAGE}");
        return 2;
    }
    if !path.is_file() {
        eprintln!(
            "dc-lobby-server reports: no DB file {} (start the lobby once, or give --db)",
            path.display()
        );
        return 1;
    }
    let conn = match crate::db::open(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("dc-lobby-server reports: cannot open {}: {e}", path.display());
            return 1;
        }
    };
    if let Err(e) = ensure_schema(&conn) {
        eprintln!("dc-lobby-server reports: {e}");
        return 1;
    }
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        .join(DIR_NAME);
    let result = match cmd {
        "list" => list_text(&conn, limit, kind.as_deref()),
        "show" => id_arg(&pos).and_then(|id| show_text(&conn, &base, id, files)),
        _ => id_arg(&pos).and_then(|id| fetch_to(&conn, &base, id, &out_dir.unwrap_or_else(|| PathBuf::from(".")))),
    };
    match result {
        Ok(t) => {
            print!("{t}");
            if !t.ends_with('\n') {
                println!();
            }
            0
        }
        Err(e) => {
            eprintln!("dc-lobby-server reports: {e}");
            1
        }
    }
}

#[cfg(test)]
#[path = "diag_tests.rs"]
mod tests;
