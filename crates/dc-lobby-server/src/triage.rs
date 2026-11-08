//! The Bug reports card of the dashboard's Updates tab (docs/DEPLOY_FROM_ADMIN.md "Bug reports from Discord"): the
//! VPS poller (tools/vps/discord_triage.sh, root) keeps `<[deploy] dir>/triage/reports.jsonl` and `state.json`; the
//! lobby only reads them, and writes one thing, `triage/done.json`, the ids the admin marked as handled. The lobby never
//! holds the Discord token.
//!
//! - `GET /ops/triage`: the poller's state, the counts and the newest 50 reports (newest first), every field of the
//!   poller's files checked and cut to size (they carry what strangers typed into a Discord channel).
//! - `POST /ops/triage/done` `{"id": "<report id>", "done": true|false}`: the write guard of the deploys (JSON, `Origin`,
//!   the page token in `X-Admin-CSRF`; the admin port also its `--admin-token`, opsweb.rs), then the id is added to or
//!   removed from `done.json`. Only an id of a stored report is accepted. Every mark is an ops event of kind `triage`.
// Provenance: ORIGINAL (DeCombat operator tooling)
// DC-DEFAULT: OPS-26 (proposed) the bug-report list, its files and the done mark

use crate::clanweb::Request;
use crate::state::{Core, Shared};
use rusqlite::params;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// The event kind of a done mark.
pub const KIND: &str = "triage";
/// The paths of this module.
pub const PREFIX: &str = "/ops/triage";
const DONE_PATH: &str = "/ops/triage/done";
/// The reports a listing shows.
pub const LIST_MAX: usize = 50;
/// The text of one report the lobby passes on (the card shows the first 600 and a "more").
const CONTENT_MAX: usize = 4000;
/// Attachment links per report, and the longest one.
const ATT_MAX: usize = 10;
const URL_MAX: usize = 500;
/// A file larger than this is read from its end (reports.jsonl) or not at all (state.json, done.json).
const MAX_FILE: u64 = 4 * 1024 * 1024;
const MAX_SMALL: u64 = 256 * 1024;
/// Done marks kept: the newest by time.
const DONE_KEEP: usize = 5000;
/// The only hosts whose attachment links the card makes clickable (Discord's own file servers).
const LINK_HOSTS: [&str; 2] = [
    "https://cdn.discordapp.com/",
    "https://media.discordapp.net/",
];

type Reply = (u16, &'static str, String);

fn reply(status: u16, v: Value) -> Reply {
    (status, crate::opsweb::JSON, v.to_string())
}

fn refuse(status: u16, msg: &str) -> Reply {
    reply(status, json!({"ok": false, "message": msg}))
}

/// True for the paths of this module.
pub fn handles(path: &str) -> bool {
    let p = path.trim_end_matches('/');
    p == PREFIX || p.strip_prefix(PREFIX).is_some_and(|r| r.starts_with('/'))
}

fn triage_dir(st: &Core) -> Option<PathBuf> {
    st.opts.deploy_dir.as_ref().map(|d| d.join("triage"))
}

/// Untrusted text: control characters become spaces (a newline stays when `lines`), at most `max` characters.
fn text(v: Option<&Value>, max: usize, lines: bool) -> String {
    v.and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .map(|c| match c {
            '\n' if lines => c,
            '\t' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// A report id: 1 to 25 digits, else empty.
fn id(v: Option<&Value>) -> String {
    let s = text(v, 26, false);
    if (1..=25).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()) {
        s
    } else {
        String::new()
    }
}

/// `u` when it is a file of Discord's servers made of plain URL characters (no quote, space, angle bracket or
/// backslash), else None: such a value is shown as text only.
pub fn discord_link(u: &str) -> Option<String> {
    let rest = LINK_HOSTS.iter().find_map(|h| u.strip_prefix(h))?;
    (u.len() <= URL_MAX
        && !rest.is_empty()
        && !rest.contains("..")
        && rest.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'-' | b'.' | b'_' | b'~' | b'/' | b'?' | b'=' | b'&' | b'%' | b'#' | b'+'
                )
        }))
    .then(|| u.to_string())
}

/// One stored report, every field checked: the only shape the page ever sees. None for a line that is no report.
pub fn clean_report(raw: &Value) -> Option<Value> {
    let id = id(raw.get("id"));
    if id.is_empty() {
        return None;
    }
    let author = raw.get("author").cloned().unwrap_or(Value::Null);
    let (user, name) = match &author {
        Value::Object(_) => (text(author.get("username"), 80, false), {
            text(author.get("name"), 80, false)
        }),
        Value::String(_) => (text(Some(&author), 80, false), String::new()),
        _ => (String::new(), String::new()),
    };
    let atts: Vec<Value> = raw
        .get("attachments")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(ATT_MAX)
                .map(|u| text(Some(u), URL_MAX, false))
                .filter(|u| !u.is_empty())
                .map(|u| json!({"link": discord_link(&u), "text": u}))
                .collect()
        })
        .unwrap_or_default();
    Some(json!({
        "id": id,
        "at": raw.get("at").and_then(Value::as_i64).filter(|t| *t > 0),
        "author": {"username": user, "name": if name.is_empty() { user.clone() } else { name }},
        "content": text(raw.get("content"), CONTENT_MAX, true),
        "attachments": atts,
    }))
}

/// The poller's `state.json`, every field checked.
pub fn clean_state(raw: &Value) -> Value {
    json!({
        "last_id": id(raw.get("last_id")),
        "updated": raw.get("updated").and_then(Value::as_i64).filter(|u| *u > 0),
        "fetched": raw.get("fetched").and_then(Value::as_u64),
        "ok": raw.get("ok").and_then(Value::as_bool),
        "message": text(raw.get("message"), 300, false),
    })
}

fn read_small(dir: &std::path::Path, name: &str) -> Option<Value> {
    let p = dir.join(name);
    let m = std::fs::metadata(&p).ok()?;
    if !m.is_file() || m.len() > MAX_SMALL {
        return None;
    }
    serde_json::from_str::<Value>(&std::fs::read_to_string(p).ok()?)
        .ok()
        .filter(Value::is_object)
}

/// Every report of `reports.jsonl` (the last [`MAX_FILE`] bytes of a larger file), each id once, newest id first.
pub fn read_reports(dir: &std::path::Path) -> Vec<Value> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(dir.join("reports.jsonl")) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut buf = Vec::new();
    let from = len.saturating_sub(MAX_FILE);
    if from > 0 && f.seek(SeekFrom::Start(from)).is_err() {
        return Vec::new();
    }
    if f.take(MAX_FILE).read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let s = String::from_utf8_lossy(&buf);
    let mut lines = s.lines();
    if from > 0 {
        lines.next(); // the first line of a cut file is a fragment
    }
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<(u128, Value)> = lines
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| clean_report(&v))
        .filter(|r| seen.insert(r["id"].as_str().unwrap_or("").to_string()))
        .map(|r| (r["id"].as_str().unwrap_or("0").parse().unwrap_or(0), r))
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.0));
    out.into_iter().map(|(_, r)| r).collect()
}

/// `done.json` as `{id: {"at", "by"}}`; anything else reads as empty.
fn read_done(dir: &std::path::Path) -> Map<String, Value> {
    read_small(dir, "done.json")
        .and_then(|v| v.get("done").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}

/// `GET /ops/triage`.
pub fn triage_json(st: &Core, now: i64) -> Value {
    let dir = triage_dir(st);
    let set_up = dir.as_ref().is_some_and(|d| d.is_dir());
    let message = match &dir {
        None => {
            "bug reports are not set up: [deploy] dir in server.toml (docs/DEPLOY_FROM_ADMIN.md)"
                .to_string()
        }
        Some(d) if !set_up => format!(
            "{} does not exist: the Discord poller is not set up on this server",
            d.display()
        ),
        Some(_) => String::new(),
    };
    let dir = dir.filter(|_| set_up);
    let state = dir
        .as_deref()
        .and_then(|d| read_small(d, "state.json"))
        .map(|v| clean_state(&v));
    let done = dir.as_deref().map(read_done).unwrap_or_default();
    let all = dir.as_deref().map(read_reports).unwrap_or_default();
    let open = all
        .iter()
        .filter(|r| !done.contains_key(r["id"].as_str().unwrap_or("")))
        .count();
    let reports: Vec<Value> = all
        .iter()
        .take(LIST_MAX)
        .map(|r| {
            let mut r = r.clone();
            let d = done.get(r["id"].as_str().unwrap_or(""));
            r["done"] = json!(d.is_some());
            r["done_by"] = json!(text(d.and_then(|d| d.get("by")), 80, false));
            r["done_at"] = json!(d.and_then(|d| d.get("at")).and_then(Value::as_i64));
            r
        })
        .collect();
    json!({
        "ok": true,
        "set_up": set_up,
        "message": message,
        "now": now,
        "state": state,
        "total": all.len(),
        "open": open,
        "reports": reports,
    })
}

fn event(st: &Core, problem: bool, detail: &str) {
    let d: String = crate::logs::log_text(detail).chars().take(500).collect();
    let _ = st.db.execute(
        "INSERT INTO ops_events (at, user_no, nick, kind, problem, match_id, detail) VALUES (?1, 0, '', ?2, ?3, 0, ?4)",
        params![crate::clock::unix_now(), KIND, problem as i64, d],
    );
    st.note(&format!("triage: {detail}"));
}

/// `POST /ops/triage/done` after the write guard: the checks and the write of `done.json`.
pub fn mark(st: &mut Core, body: &[u8], by: &str, now: i64) -> Reply {
    if body.len() > 1024 {
        return refuse(413, "the request is too large");
    }
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return refuse(
            400,
            "send JSON {\"id\": \"<report id>\", \"done\": true|false}",
        );
    };
    let rid = id(v.get("id"));
    let Some(want) = v.get("done").and_then(Value::as_bool) else {
        return refuse(400, "\"done\" must be true or false");
    };
    if rid.is_empty() {
        return refuse(400, "\"id\" must be a report id (digits)");
    }
    let Some(dir) = triage_dir(st).filter(|d| d.is_dir()) else {
        return refuse(
            503,
            "bug reports are not set up on this server (the poller's triage/ folder)",
        );
    };
    if !read_reports(&dir).iter().any(|r| r["id"] == rid.as_str()) {
        return refuse(404, "no such report");
    }
    let mut done = read_done(&dir);
    if want {
        done.insert(rid.clone(), json!({"at": now, "by": by}));
    } else {
        done.remove(&rid);
    }
    if done.len() > DONE_KEEP {
        let mut by_time: Vec<(i64, String)> = done
            .iter()
            .map(|(k, v)| (v.get("at").and_then(Value::as_i64).unwrap_or(0), k.clone()))
            .collect();
        by_time.sort();
        for (_, k) in by_time.iter().take(done.len() - DONE_KEEP) {
            done.remove(k);
        }
    }
    let doc = json!({"done": done});
    // in place: done.json is the one file the unit may write (a bind of the file itself, no rename)
    let path = dir.join("done.json");
    if let Err(e) = std::fs::write(&path, doc.to_string()) {
        event(
            st,
            true,
            &format!("report {rid} could not be marked: {}: {e}", path.display()),
        );
        return refuse(500, &format!("could not write done.json: {e}"));
    }
    event(
        st,
        false,
        &format!(
            "report {rid} {} by {by}",
            if want { "marked done" } else { "reopened" }
        ),
    );
    reply(
        200,
        json!({"ok": true, "id": rid, "done": want, "message": if want { "marked done" } else { "reopened" }}),
    )
}

/// One request under [`PREFIX`] (the caller checked loopback, Host and token, or the admin web session). `name` is
/// the signed-in admin's display name on the admin web; on the admin port the writer is "the admin port".
pub fn serve_at(
    shared: &Shared,
    req: &Request,
    head: &str,
    body: &[u8],
    guard: Option<&crate::betaweb::WriteGuard>,
    name: Option<&str>,
    now: i64,
) -> Reply {
    let path = req.path.trim_end_matches('/');
    let mut g = crate::net::lock(shared);
    let st = &mut *g;
    match (req.method.as_str(), path) {
        ("GET", PREFIX) => reply(200, triage_json(st, now)),
        ("POST", DONE_PATH) => {
            let host = crate::betaweb::header(head, "host").unwrap_or("");
            let (origin, csrf, by) = match guard {
                Some(w) => (
                    w.origin.clone(),
                    w.csrf.clone(),
                    name.unwrap_or("the admin web").to_string(),
                ),
                None if host.is_empty() => {
                    return refuse(403, "the request does not come from the dashboard (Origin)")
                }
                None => (
                    format!("http://{host}"),
                    st.beta.csrf(&mut st.keys),
                    "the admin port".to_string(),
                ),
            };
            if let Err(r) = crate::deploy::guard_ok(head, &origin, &csrf) {
                return r;
            }
            mark(st, body, &by, now)
        }
        (_, PREFIX | DONE_PATH) => refuse(405, "method not allowed"),
        _ => refuse(404, "not found"),
    }
}
