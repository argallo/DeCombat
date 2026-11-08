//! The Release card of the dashboard's Updates tab (docs/DEPLOY_FROM_ADMIN.md "Release card (dashboard)"): the same
//! pattern as the deploys of deploy.rs. Root's release job on the VPS keeps `<[deploy] dir>/release/status.json`; the
//! lobby only reads it and writes one thing, a request file in `release/requests/`. The lobby never holds a GitHub
//! key and never runs git.
//!
//! - `GET /ops/updates/release`: the release state, every field of root's file checked and cut to size (it is
//!   untrusted text), with the headline and which buttons the card offers.
//! - `POST /ops/updates/release/<check|prepare|tag>`: the write guard of the deploys (JSON, `Origin`, the page token
//!   in `X-Admin-CSRF`; the admin port also its `--admin-token`, opsweb.rs), then
//!   `release/requests/<unix>-<action>.json` = `{"action", "by", "at"}`. One request waits at a time; `prepare` only
//!   from `idle` / `published` / `failed` with commits since the last tag, `tag` only from `ready`. Every request and
//!   refusal of a busy state is an ops event of kind `release`.
// Provenance: ORIGINAL (DeCombat operator tooling)
// DC-DEFAULT: LG-430 (proposed) the release card, its state file and its request

use crate::clanweb::Request;
use crate::state::{Core, Shared};
use rusqlite::params;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The event kind of a release request.
pub const KIND: &str = "release";
/// The path prefix of this module (under deploy.rs's `/ops/updates`, so its guards and refusals apply here too).
pub const PREFIX: &str = "/ops/updates/release";
/// The actions a request may name.
pub const ACTIONS: [&str; 3] = ["check", "prepare", "tag"];
/// The phases root's job writes; anything else reads as `unknown`.
const PHASES: [&str; 9] = [
    "idle",
    "exporting",
    "pushed",
    "ci_running",
    "ready",
    "tagging",
    "publishing",
    "published",
    "failed",
];
/// The phases during which root's job is at work.
const BUSY: [&str; 5] = ["exporting", "pushed", "ci_running", "tagging", "publishing"];
/// The only links the card makes clickable: the public repository's pages.
const LINK_BASE: &str = "https://github.com/argallo/DeCombat/";
/// A state file larger than this is not read.
const MAX_STATE: u64 = 256 * 1024;
/// The log lines shown, and the length of each.
const TAIL_LINES: usize = 40;
const TAIL_CHARS: usize = 300;

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

fn release_dir(st: &Core) -> Option<PathBuf> {
    st.opts.deploy_dir.as_ref().map(|d| d.join("release"))
}

/// One line of untrusted text: control characters become spaces, at most `max` characters.
fn text(v: Option<&Value>, max: usize) -> String {
    v.and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// A commit id: 7 to 40 hex digits, else empty.
fn sha(v: Option<&Value>) -> String {
    let s = text(v, 40).to_ascii_lowercase();
    if (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        s
    } else {
        String::new()
    }
}

/// A tag name: up to 40 letters, digits, `.`, `_`, `-` and `+`, else empty.
fn tag(v: Option<&Value>) -> String {
    let s = text(v, 41);
    if s.len() <= 40
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
    {
        s
    } else {
        String::new()
    }
}

/// `u` when it is a page of the public repository made of plain URL characters (no quote, space, angle bracket or
/// backslash), else None: such a value is shown as text only.
pub fn github_link(u: &str) -> Option<String> {
    let rest = u.strip_prefix(LINK_BASE)?;
    (u.len() <= 300
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

/// A URL field: `{"link": <clickable or null>, "text": <what it says>}`.
fn url(v: Option<&Value>) -> Value {
    let t = text(v, 300);
    json!({"link": github_link(&t), "text": t})
}

/// Root's `status.json`, every field checked: the only shape the page ever sees.
pub fn clean_status(raw: &Value) -> Value {
    let get = |k: &str| raw.get(k);
    let phase = text(get("phase"), 20);
    let phase = if PHASES.contains(&phase.as_str()) {
        phase
    } else {
        "unknown".to_string()
    };
    let ci = get("ci").cloned().unwrap_or(Value::Null);
    let tail: Vec<String> = get("log_tail")
        .and_then(Value::as_array)
        .map(|a| {
            let lines: Vec<String> = a
                .iter()
                .map(|l| text(Some(l), TAIL_CHARS))
                .filter(|l| !l.is_empty())
                .collect();
            lines[lines.len().saturating_sub(TAIL_LINES)..].to_vec()
        })
        .unwrap_or_default();
    json!({
        "phase": phase,
        "updated": get("updated").and_then(Value::as_i64).filter(|u| *u > 0),
        "private_sha": sha(get("private_sha")),
        "public_sha": sha(get("public_sha")),
        "last_tag": tag(get("last_tag")),
        "next_tag": tag(get("next_tag")),
        "commits_since": get("commits_since").and_then(Value::as_u64),
        "ci": {
            "status": text(ci.get("status"), 40),
            "conclusion": text(ci.get("conclusion"), 40),
            "url": url(ci.get("url")),
        },
        "release_url": url(get("release_url")),
        "message": text(get("message"), 300),
        "log_tail": tail,
        "ok": get("ok").and_then(Value::as_bool),
        "by": text(get("by"), 80),
    })
}

fn short(s: &str) -> &str {
    &s[..s.len().min(8)]
}

/// The headline, its level (`good` / `warn` / `bad`) and the buttons the card offers for a cleaned status.
pub fn view(s: &Value, pending: Option<&str>) -> Value {
    let f = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("");
    let phase = f("phase");
    let n = s.get("commits_since").and_then(Value::as_u64).unwrap_or(0);
    let (last, next, msg) = (f("last_tag"), f("next_tag"), f("message"));
    let since = if last.is_empty() {
        "the last release".to_string()
    } else {
        last.to_string()
    };
    let public = short(f("public_sha"));
    let headline = match phase {
        "idle" | "published" if n > 0 => format!(
            "{n} commit{} since {since} — ready to prepare",
            if n == 1 { "" } else { "s" }
        ),
        "idle" => format!("Nothing new since {since}"),
        "published" => format!("Published {}", if last.is_empty() { next } else { last }),
        "exporting" => format!(
            "Exporting {} to the public repository…",
            short(f("private_sha"))
        ),
        "pushed" => format!("Pushed {public}, waiting for CI"),
        "ci_running" => format!("Pushed {public}, CI running"),
        "ready" => format!("Ready to tag {next} — CI green"),
        "tagging" => format!("Tagging {next}…"),
        "publishing" => format!("Publishing {next}…"),
        "failed" if msg.is_empty() => "Failed".to_string(),
        "failed" => format!("Failed: {msg}"),
        _ => "The release job wrote a state this lobby does not know".to_string(),
    };
    let level = match phase {
        "failed" | "unknown" => "bad",
        p if BUSY.contains(&p) => "warn",
        _ => "good",
    };
    let free = pending.is_none();
    json!({
        "headline": headline,
        "level": level,
        "busy": BUSY.contains(&phase),
        "can_check": free,
        "can_prepare": free && matches!(phase, "idle" | "published" | "failed") && n > 0,
        "can_tag": free && phase == "ready" && !next.is_empty(),
    })
}

/// The action of the newest request file waiting in `requests/`, if any.
fn waiting(req_dir: &Path) -> Option<String> {
    let rd = std::fs::read_dir(req_dir).ok()?;
    rd.flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let (at, action) = n.strip_suffix(".json")?.split_once('-')?;
            let at: i64 = at.parse().ok()?;
            ACTIONS.contains(&action).then(|| (at, action.to_string()))
        })
        .max()
        .map(|(_, a)| a)
}

fn read_status(dir: &Path) -> Option<Value> {
    let p = dir.join("status.json");
    let m = std::fs::metadata(&p).ok()?;
    if !m.is_file() || m.len() > MAX_STATE {
        return None;
    }
    serde_json::from_str::<Value>(&std::fs::read_to_string(p).ok()?)
        .ok()
        .filter(Value::is_object)
        .map(|v| clean_status(&v))
}

/// `GET /ops/updates/release`.
pub fn release_json(st: &Core, now: i64) -> Value {
    let dir = release_dir(st);
    let req_dir = dir.as_ref().map(|d| d.join("requests"));
    let set_up = req_dir.as_ref().is_some_and(|d| d.is_dir());
    let pending = req_dir.as_deref().filter(|_| set_up).and_then(waiting);
    let status = dir.as_deref().filter(|_| set_up).and_then(read_status);
    let message = match &dir {
        None => "releases from the dashboard are not set up: [deploy] dir in server.toml (docs/DEPLOY_FROM_ADMIN.md)"
            .to_string(),
        Some(d) if !set_up => format!(
            "{} has no requests/ folder: the release job is not set up on this server",
            d.display()
        ),
        Some(_) if status.is_none() => {
            "no release state yet: press Check now and the release job writes one".to_string()
        }
        Some(_) => String::new(),
    };
    let view = status.as_ref().map(|s| view(s, pending.as_deref()));
    json!({
        "ok": true,
        "set_up": set_up,
        "message": message,
        "now": now,
        "pending": pending,
        "status": status,
        "view": view,
    })
}

fn event(st: &Core, problem: bool, detail: &str) {
    let d: String = crate::logs::log_text(detail).chars().take(500).collect();
    let _ = st.db.execute(
        "INSERT INTO ops_events (at, user_no, nick, kind, problem, match_id, detail) VALUES (?1, 0, '', ?2, ?3, 0, ?4)",
        params![crate::clock::unix_now(), KIND, problem as i64, d],
    );
    st.note(&format!("release: {detail}"));
}

/// `POST /ops/updates/release/<action>` after the write guard: the checks and the request file.
pub fn request(st: &mut Core, action: &str, by: &str, now: i64) -> Reply {
    if !ACTIONS.contains(&action) {
        return refuse(404, "no such release action (check, prepare or tag)");
    }
    let Some(dir) = release_dir(st) else {
        return refuse(
            503,
            "releases from the dashboard are not set up on this server ([deploy] dir)",
        );
    };
    let req_dir = dir.join("requests");
    if !req_dir.is_dir() {
        return refuse(
            503,
            "the release folder has no requests/: the release job is not set up on this server",
        );
    }
    if let Some(w) = waiting(&req_dir) {
        if w == action && action == "check" {
            return reply(
                200,
                json!({"ok": true, "message": "a check is already requested"}),
            );
        }
        return refuse(
            409,
            &format!("a release {w} request is waiting; the server picks it up within seconds"),
        );
    }
    if action != "check" {
        let status = read_status(&dir);
        let v = status.as_ref().map(|s| view(s, None));
        let allowed = v
            .as_ref()
            .and_then(|v| {
                v.get(if action == "tag" {
                    "can_tag"
                } else {
                    "can_prepare"
                })
            })
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !allowed {
            let phase = status
                .as_ref()
                .and_then(|s| s.get("phase"))
                .and_then(Value::as_str)
                .unwrap_or("no state")
                .to_string();
            event(
                st,
                false,
                &format!("release {action} refused: the release job is at {phase} (by {by})"),
            );
            return refuse(
                409,
                &if action == "tag" {
                    format!("nothing is ready to tag (the release job is at {phase}); prepare a release first")
                } else {
                    format!("nothing to prepare (the release job is at {phase}); press Check now")
                },
            );
        }
    }
    let path = req_dir.join(format!("{now}-{action}.json"));
    let doc = json!({"action": action, "by": by, "at": now});
    // one write of a new file, as the deploy requests
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, doc.to_string().as_bytes()));
    if let Err(e) = written {
        event(
            st,
            true,
            &format!(
                "release {action} could not be requested: {}: {e}",
                path.display()
            ),
        );
        return refuse(500, &format!("could not write the request: {e}"));
    }
    event(st, false, &format!("release {action} requested by {by}"));
    reply(
        200,
        json!({"ok": true, "message": format!("release {action} requested; the server starts it within seconds")}),
    )
}

/// One request under [`PREFIX`] (the caller checked loopback, Host and token, or the admin web session). `name` is
/// the signed-in admin's display name on the admin web; on the admin port the writer is "the admin port".
pub fn serve_at(
    shared: &Shared,
    req: &Request,
    head: &str,
    guard: Option<&crate::betaweb::WriteGuard>,
    name: Option<&str>,
    now: i64,
) -> Reply {
    let path = req.path.trim_end_matches('/');
    let mut g = crate::net::lock(shared);
    let st = &mut *g;
    match (
        req.method.as_str(),
        path.strip_prefix(PREFIX).unwrap_or(path),
    ) {
        ("GET", "") => reply(200, release_json(st, now)),
        ("POST", a) if a.starts_with('/') => {
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
            request(st, &a[1..], &by, now)
        }
        ("GET", _) => refuse(405, "method not allowed"),
        _ => refuse(404, "not found"),
    }
}
