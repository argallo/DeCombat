//! Deploys from the dashboard (docs/DEPLOY_FROM_ADMIN.md): the lobby reads the state root keeps in `[deploy] dir`
//! (`tools/vps/deploy_setup.sh`, `tools/vps/deploy_from_git.sh`) and writes one thing, a request file in `requests/`;
//! root's `decombat-deploy.path` unit picks it up and does everything else. The lobby never holds the GitHub key, never
//! runs git or cargo, and is restarted by the deploy it asked for.
//!
//! - `GET /ops/updates`: the running builds (this lobby, each host agent), the commits of master since the running
//!   build (`available.json`), the deploy in progress (`status.json`), the last deploys (`history.jsonl`), the build
//!   offered to the hosts (`host/desired.txt`), the running matches.
//! - `POST /ops/updates/deploy {"sha", "targets": ["vps", "hosts"], "end_matches"}`: the write guard of the beta list
//!   (JSON, `Origin`, the page token in `X-Admin-CSRF`); 409 while a deploy runs or waits, or while matches run unless
//!   `end_matches`; then `requests/<unix>-<8 hex>.json`. Every request and every outcome is an ops event of kind
//!   `deploy`.
//! - `POST /ops/updates/fetch`: the same write guard; "Check now": writes `requests/fetch-<unix>.json`, which root's
//!   unit answers with one fetch of the mirror (a fetch request never starts a deploy). 409 while a deploy runs or
//!   waits (the deploy fetches itself), 429 within [`FETCH_GAP`] seconds of the last check; an ops event of kind `deploy`.
//! - Host agents (`hosts.rs`, HA-2): [`host_build`] is the build the lobby offers them.
// Provenance: ORIGINAL (DeCombat operator tooling)
// DC-DEFAULT: LG-392 (proposed) the state files and the request; LG-393 (proposed) the write guard and refusals;
// LG-394 (proposed) ops events of kind `deploy`; LG-402 (proposed) the fetch request ("Check now")

use crate::clanweb::Request;
use crate::state::{Core, Shared};
use rusqlite::params;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The event kind of a deploy request and outcome.
pub const KIND: &str = "deploy";
/// A state file larger than this is not read (root writes small ones).
const MAX_STATE: u64 = 1024 * 1024;
/// A deploy whose status has not moved for this long is taken as dead (root's unit times out after 120 min).
const STALE_SECS: i64 = 3 * 3600;
/// LG-402: the lobby asks for no check within this many seconds of the last one (root keeps its own limit).
const FETCH_GAP: i64 = 30;
/// The last deploys shown.
const HISTORY: usize = 10;

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
    p == "/ops/updates" || p.starts_with("/ops/updates/")
}

fn hex(s: &str, lo: usize, hi: usize) -> bool {
    (lo..=hi).contains(&s.len()) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn read_capped(p: &Path) -> Option<String> {
    let m = std::fs::metadata(p).ok()?;
    if !m.is_file() || m.len() > MAX_STATE {
        return None;
    }
    std::fs::read_to_string(p).ok()
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&read_capped(p)?).ok()
}

/// The build offered to the host agents: `host/desired.txt` and its tarball.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostBuild {
    pub commit: String,
    pub tarball: String,
    pub sha256: String,
    pub size: u64,
    pub path: PathBuf,
    /// When root wrote `desired.txt`.
    pub published: Option<std::time::SystemTime>,
    /// HA-2: root's sequence number and its signature (base64 of the `ssh-keygen -Y sign` file), relayed unchanged:
    /// the lobby holds no key and cannot make either.
    pub seq: u64,
    pub sig: String,
}

/// True for the base64 of a signature file: 16 to 8192 characters of the standard alphabet.
pub fn sig_ok(s: &str) -> bool {
    (16..=8192).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// `host/desired.txt` of `dir`, checked (40 hex commit, a tarball name `update.sh` takes, 64 hex SHA-256, its size
/// equal to the file's, a sequence number and a signature of the right shape); None when any of it is off. The
/// signature itself is checked by the host's updater, against the key it pinned.
pub fn host_build(dir: &Path) -> Option<HostBuild> {
    let pointer = dir.join("host").join("desired.txt");
    let text = read_capped(&pointer)?;
    let published = std::fs::metadata(&pointer).and_then(|m| m.modified()).ok();
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
            .map(|v| v.trim().to_string())
            .unwrap_or_default()
    };
    let (commit, tarball, sha256) = (get("commit"), get("tarball"), get("sha256"));
    let size: u64 = get("size").parse().ok()?;
    let seq: u64 = get("seq").parse().ok()?;
    let sig = get("sig");
    if !sig_ok(&sig) {
        return None;
    }
    let name_ok = tarball
        .strip_prefix("decombat-servers-linux-x86_64-")
        .and_then(|r| r.strip_suffix(".tar.gz"))
        .is_some_and(|m| {
            !m.is_empty()
                && !m.starts_with('.')
                && m.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        });
    if !hex(&commit, 40, 40) || !hex(&sha256, 64, 64) || !name_ok {
        return None;
    }
    let path = dir.join("host").join(&tarball);
    let m = std::fs::metadata(&path).ok()?;
    (m.is_file() && m.len() == size).then_some(HostBuild {
        commit,
        tarball,
        sha256,
        size,
        path,
        published,
        seq,
        sig,
    })
}

/// True while `status` is a deploy that has not finished and moved within [`STALE_SECS`].
fn in_progress(status: Option<&Value>, now: i64) -> bool {
    status.is_some_and(|s| {
        s.get("finished").is_none_or(Value::is_null)
            && s.get("updated")
                .and_then(Value::as_i64)
                .is_some_and(|u| now - u < STALE_SECS)
    })
}

/// A request file waiting for root's unit (a deploy's; `fetch-*.json` is a check request, LG-402).
fn pending(dir: &Path) -> bool {
    waiting(dir, false)
}

/// A fetch request waiting for root's unit.
fn fetch_pending(dir: &Path) -> bool {
    waiting(dir, true)
}

fn waiting(dir: &Path, fetch: bool) -> bool {
    std::fs::read_dir(dir.join("requests")).is_ok_and(|rd| {
        rd.flatten().any(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.ends_with(".json") && n.starts_with("fetch-") == fetch
        })
    })
}

fn history(dir: &Path) -> Vec<Value> {
    let Some(text) = read_capped(&dir.join("history.jsonl")) else {
        return Vec::new();
    };
    let mut v: Vec<Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();
    v.reverse();
    v.truncate(HISTORY);
    v
}

fn matches_running(st: &Core) -> usize {
    st.games.values().filter(|g| !g.over).count()
}

fn event(st: &Core, problem: bool, detail: &str) {
    let d: String = crate::logs::log_text(detail).chars().take(500).collect();
    let _ = st.db.execute(
        "INSERT INTO ops_events (at, user_no, nick, kind, problem, match_id, detail) VALUES (?1, 0, '', ?2, ?3, 0, ?4)",
        params![crate::clock::unix_now(), KIND, problem as i64, d],
    );
    st.note(&format!("deploy: {detail}"));
}

/// LG-394: a finished deploy root wrote is an ops event once (the lobby that sees it first records it; after a
/// deploy that restarted the lobby, that is the new one).
fn note_outcome(st: &Core, status: &Value) {
    let (Some(id), Some(ok)) = (
        status.get("id").and_then(Value::as_str),
        status.get("ok").and_then(Value::as_bool),
    ) else {
        return;
    };
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return;
    }
    let head = format!("deploy {id} finished");
    let seen: bool = st
        .db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM ops_events WHERE kind = ?1 AND detail LIKE ?2)",
            params![KIND, format!("{head}%")],
            |r| r.get(0),
        )
        .unwrap_or(true);
    if seen {
        return;
    }
    let s = |k: &str| {
        status
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    event(
        st,
        !ok,
        &format!(
            "{head}: {} {}: {}",
            if ok { "ok" } else { "FAILED" },
            s("short"),
            s("message")
        ),
    );
}

/// `GET /ops/updates`.
pub fn updates_json(st: &Core, now: i64) -> Value {
    let dir = st.opts.deploy_dir.clone();
    let set_up = dir.as_ref().is_some_and(|d| d.join("requests").is_dir());
    let hb = dir.as_deref().and_then(host_build);
    let lobby_build = crate::opsmon::build_sha();
    let hosts: Vec<Value> = st
        .opts
        .host_agents
        .iter()
        .map(|c| {
            let a = st.hosts.online.get(&c.name);
            let games = st
                .games
                .values()
                .filter(|g| !g.over && g.link.remote().is_some_and(|r| r.agent == c.name))
                .count();
            json!({
                "name": c.name,
                "online": a.is_some(),
                "version": a.map(|a| a.version.clone()),
                "takes_updates": a.is_some_and(|a| a.takes_update),
                "on_offered": match (a, &hb) {
                    (Some(a), Some(b)) => Value::Bool(crate::opsmon::same_build(&a.version, &b.commit)),
                    _ => Value::Null,
                },
                "on_lobby": a.map(|a| crate::opsmon::same_build(&a.version, lobby_build)),
                "games": games,
            })
        })
        .collect();
    let status = dir.as_ref().and_then(|d| read_json(&d.join("status.json")));
    if let Some(s) = &status {
        note_outcome(st, s);
    }
    json!({
        "ok": true,
        "set_up": set_up,
        "message": match &dir {
            None => "deploys from the dashboard are not set up: [deploy] dir in server.toml (docs/DEPLOY_FROM_ADMIN.md)".to_string(),
            Some(d) if !set_up => format!("{} has no requests/ folder: run tools/vps/deploy_setup.sh as root", d.display()),
            Some(_) => String::new(),
        },
        "now": now,
        "lobby": {"build": lobby_build},
        "hosts": hosts,
        "host_build": hb.as_ref().map(|b| json!({
            "commit": b.commit, "short": &b.commit[..8], "tarball": b.tarball, "size": b.size,
        })),
        "matches": matches_running(st),
        "available": dir.as_ref().and_then(|d| read_json(&d.join("available.json"))),
        "deploying": in_progress(status.as_ref(), now),
        "pending": dir.as_deref().is_some_and(pending),
        "fetching": dir.as_deref().is_some_and(fetch_pending),
        "status": status,
        "history": dir.as_deref().map(history).unwrap_or_default(),
    })
}

/// The subject of `sha` in `available.json` (new or older commits), if listed.
fn subject_of(avail: Option<&Value>, sha: &str) -> Option<String> {
    let a = avail?;
    ["new", "older"].iter().find_map(|k| {
        a.get(*k)?.as_array()?.iter().find_map(|c| {
            let full = c.get("sha")?.as_str()?;
            full.starts_with(sha).then(|| {
                c.get("subject")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            })
        })
    })
}

/// `POST /ops/updates/deploy` after the write guard: the checks and the request file.
pub fn request(st: &mut Core, body: &[u8], by: &str, now: i64) -> Reply {
    let Some(dir) = st.opts.deploy_dir.clone() else {
        return refuse(
            503,
            "deploys from the dashboard are not set up on this server ([deploy] dir)",
        );
    };
    let req_dir = dir.join("requests");
    if !req_dir.is_dir() {
        return refuse(
            503,
            "the deploy folder has no requests/: run tools/vps/deploy_setup.sh as root",
        );
    }
    let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(body) else {
        return refuse(400, "send a JSON object");
    };
    let sha = m
        .get("sha")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !hex(&sha, 7, 40) {
        return refuse(400, "sha: 7 to 40 hex digits of a commit on master");
    }
    let targets: Vec<String> = match m.get("targets") {
        None => vec!["vps".into(), "hosts".into()],
        Some(Value::Array(a)) => {
            let t: Vec<String> = a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if t.is_empty() || t.len() != a.len() || t.iter().any(|x| x != "vps" && x != "hosts") {
                return refuse(400, "targets: a non-empty list of \"vps\" and \"hosts\"");
            }
            ["vps", "hosts"]
                .iter()
                .filter(|x| t.iter().any(|y| y == *x))
                .map(|x| x.to_string())
                .collect()
        }
        Some(_) => return refuse(400, "targets: a non-empty list of \"vps\" and \"hosts\""),
    };
    let end_matches = match m.get("end_matches") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return refuse(400, "end_matches: true or false"),
    };
    let short = &sha[..8.min(sha.len())];
    let status = read_json(&dir.join("status.json"));
    if in_progress(status.as_ref(), now) || pending(&dir) {
        event(
            st,
            false,
            &format!("deploy of {short} refused: another deploy runs or waits (by {by})"),
        );
        return refuse(
            409,
            "a deploy is already running or waiting; watch its status",
        );
    }
    let n = matches_running(st);
    if n > 0 && !end_matches && targets.iter().any(|t| t == "vps") {
        event(
            st,
            false,
            &format!("deploy of {short} refused: {n} match(es) running (by {by})"),
        );
        return refuse(
            409,
            &format!("{n} match(es) running: the restart would end them. Tick \"end running matches\" to deploy anyway."),
        );
    }
    let id = format!("{now}-{}", &crate::beta::new_token(&mut st.keys)[..8]);
    let doc = json!({
        "sha": sha, "targets": targets, "end_matches": end_matches, "by": by, "at": now,
    });
    let path = req_dir.join(format!("{id}.json"));
    // one write of a new file: root's unit reads it after a short settle (deploy_from_git.sh)
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
                "deploy of {short} could not be requested: {}: {e}",
                path.display()
            ),
        );
        return refuse(500, &format!("could not write the request: {e}"));
    }
    let avail = read_json(&dir.join("available.json"));
    let subject = subject_of(avail.as_ref(), &sha).unwrap_or_default();
    event(
        st,
        false,
        &format!(
            "deploy {id} requested: {short} {subject:?} to {}{} by {by}",
            targets.join(" + "),
            if end_matches {
                ", ending running matches"
            } else {
                ""
            }
        ),
    );
    reply(
        200,
        json!({"ok": true, "id": id, "message": format!("deploy of {short} requested; the server starts it within seconds")}),
    )
}

/// `POST /ops/updates/fetch` after the write guard (LG-402): one `requests/fetch-<unix>.json` for root's unit to
/// answer with a fetch. Cheap: one pending request at a time, none within [`FETCH_GAP`] s of the last check, none
/// while a deploy runs or waits (a deploy fetches first anyway).
pub fn fetch_request(st: &mut Core, by: &str, now: i64) -> Reply {
    let Some(dir) = st.opts.deploy_dir.clone() else {
        return refuse(
            503,
            "deploys from the dashboard are not set up on this server ([deploy] dir)",
        );
    };
    let req_dir = dir.join("requests");
    if !req_dir.is_dir() {
        return refuse(
            503,
            "the deploy folder has no requests/: run tools/vps/deploy_setup.sh as root",
        );
    }
    let status = read_json(&dir.join("status.json"));
    if in_progress(status.as_ref(), now) || pending(&dir) {
        return refuse(
            409,
            "a deploy is running or waiting; it fetches master itself",
        );
    }
    if fetch_pending(&dir) {
        return reply(
            200,
            json!({"ok": true, "message": "a check is already requested"}),
        );
    }
    let checked = read_json(&dir.join("available.json"))
        .and_then(|a| a.get("checked").and_then(Value::as_i64))
        .unwrap_or(0);
    if now - checked < FETCH_GAP {
        return refuse(
            429,
            &format!(
                "checked {} s ago; wait {FETCH_GAP} s between checks",
                (now - checked).max(0)
            ),
        );
    }
    let path = req_dir.join(format!("fetch-{now}.json"));
    let doc = json!({"by": by, "at": now});
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
                "check for updates could not be requested: {}: {e}",
                path.display()
            ),
        );
        return refuse(500, &format!("could not write the request: {e}"));
    }
    event(st, false, &format!("check for updates requested by {by}"));
    reply(
        200,
        json!({"ok": true, "message": "check requested; the server looks within seconds"}),
    )
}

/// The write guard: a JSON body, an `Origin` that is this very page's, and the page token in `X-Admin-CSRF`.
fn guard_ok(head: &str, origin: &str, csrf: &str) -> Result<(), Reply> {
    let h = |n: &str| crate::betaweb::header(head, n);
    if !h("content-type")
        .unwrap_or("")
        .to_ascii_lowercase()
        .starts_with("application/json")
    {
        return Err(refuse(415, "send application/json"));
    }
    if !h("origin").is_some_and(|o| !origin.is_empty() && o.eq_ignore_ascii_case(origin)) {
        return Err(refuse(
            403,
            "the request does not come from the dashboard (Origin)",
        ));
    }
    if csrf.is_empty() || !crate::hosts::token_eq(h("x-admin-csrf").unwrap_or(""), csrf) {
        return Err(refuse(403, "missing or wrong page token: reload the page"));
    }
    Ok(())
}

/// One request under `/ops/updates` (the caller checked loopback, Host and token, or the admin web session).
pub fn serve(
    shared: &Shared,
    req: &Request,
    head: &str,
    body: &[u8],
    guard: Option<&crate::betaweb::WriteGuard>,
) -> Reply {
    serve_at(shared, req, head, body, guard, crate::clock::unix_now())
}

/// [`serve`] at unix time `now`.
pub fn serve_at(
    shared: &Shared,
    req: &Request,
    head: &str,
    body: &[u8],
    guard: Option<&crate::betaweb::WriteGuard>,
    now: i64,
) -> Reply {
    let path = req.path.trim_end_matches('/');
    let mut g = crate::net::lock(shared);
    let st = &mut *g;
    match (req.method.as_str(), path) {
        ("GET", "/ops/updates") => reply(200, updates_json(st, now)),
        ("POST", "/ops/updates/deploy" | "/ops/updates/fetch") => {
            // LG-393: on the loopback port the page's origin is http://<Host> and its token the per-process one; on the
            // admin web its own origin and the session's token
            let host = crate::betaweb::header(head, "host").unwrap_or("");
            let (origin, csrf, by) = match guard {
                Some(w) => (
                    w.origin.clone(),
                    w.csrf.clone(),
                    "the admin web".to_string(),
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
            if let Err(r) = guard_ok(head, &origin, &csrf) {
                return r;
            }
            if path.ends_with("/fetch") {
                fetch_request(st, &by, now)
            } else {
                request(st, body, &by, now)
            }
        }
        (_, "/ops/updates" | "/ops/updates/deploy" | "/ops/updates/fetch") => {
            refuse(405, "method not allowed")
        }
        _ => refuse(404, "not found"),
    }
}

#[cfg(test)]
#[path = "deploy_tests.rs"]
mod tests;
