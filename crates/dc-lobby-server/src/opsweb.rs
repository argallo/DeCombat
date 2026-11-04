//! The operator dashboard on the admin port: `GET /ops` (one plain HTML page, no external scripts) and the JSON it
//! reads, `/ops/live`, `/ops/events`, `/ops/matches`, `/ops/match`, `/ops/timeline`, `/ops/alerts` (opsmon.rs).
//!
//! DC-DEFAULT: MON-4 (proposed) these paths answer a loopback peer only, even on an admin port opened with
//! `--admin-public`, and only to a request whose `Host` is a loopback name (a web page that rebinds its own name to
//! 127.0.0.1 cannot read them through the operator's browser); with `--admin-token` the token is needed as well
//! (header, or `?token=` which the page passes on). The owner reaches the page through an SSH tunnel
//! (`docs/OPS.md`, `tools/vps/ops_tunnel.ps1`).
// Provenance: ORIGINAL (DeCombat operator tooling)

use crate::clanweb::parse_request;
use crate::state::Shared;
use std::net::IpAddr;

/// The page (inline CSS and JS; it fetches the JSON paths below with the token of its own address).
pub(crate) const PAGE: &str = include_str!("ops_dashboard.html");

pub const HTML: &str = "text/html; charset=utf-8";
pub const JSON: &str = "application/json; charset=utf-8";

/// True when the `Host` header names a loopback address (any port), or is absent (HTTP/1.0 tools).
fn host_is_loopback(head: &str) -> bool {
    let Some(v) = head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case("host").then(|| v.trim().to_ascii_lowercase())
    }) else {
        return true;
    };
    let name = if let Some(rest) = v.strip_prefix('[') {
        rest.split(']').next().unwrap_or("").to_string()
    } else {
        v.rsplit_once(':')
            .filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit()))
            .map_or(v.clone(), |(h, _)| h.to_string())
    };
    name == "localhost"
        || name
            .parse::<IpAddr>()
            .map(|a| a.is_loopback())
            .unwrap_or(false)
}

fn err(status: u16, text: &str) -> (u16, &'static str, String) {
    (
        status,
        JSON,
        format!("{{\"ok\": false, \"message\": {}}}", dc_mod_lua::JsonValue::String(text.into()).to_json_string()),
    )
}

/// One admin request under `/ops` without a body; `None` for any other path (admin.rs answers it).
#[cfg(test)]
pub fn handle(
    shared: &Shared,
    head: &str,
    peer: Option<IpAddr>,
    token: Option<&str>,
    token_ok: bool,
) -> Option<(u16, &'static str, String)> {
    handle_with_body(shared, head, &[], peer, token, token_ok)
}

/// [`handle`] with the request body (the beta page's writes, LG-375).
pub fn handle_with_body(
    shared: &Shared,
    head: &str,
    body: &[u8],
    peer: Option<IpAddr>,
    token: Option<&str>,
    token_ok: bool,
) -> Option<(u16, &'static str, String)> {
    let req = parse_request(head)?;
    let path = req.path.trim_end_matches('/');
    if path != "/ops" && !path.starts_with("/ops/") {
        return None;
    }
    if !peer.is_some_and(|p| p.is_loopback()) {
        return Some(err(403, "the dashboard answers loopback only (use an ssh tunnel)"));
    }
    if !host_is_loopback(head) {
        return Some(err(403, "Host must be a loopback name"));
    }
    if token.is_some() && !token_ok {
        return Some(err(401, "admin token missing or wrong"));
    }
    // LG-393: a deploy from the loopback port needs the admin token (`--admin-token` / DC_ADMIN_TOKEN), checked
    // above; a port without one may read the Updates tab but not deploy (the admin web deploys behind its key)
    if req.method == "POST" && crate::deploy::handles(path) && token.is_none() {
        return Some(err(
            403,
            "deploys on the admin port need its --admin-token; use the admin web, or start the lobby with a token",
        ));
    }
    Some(serve(shared, &req, head, body, None))
}

/// The dashboard's answer to a request under `/ops` whose caller is already authorised: the loopback admin port after
/// its peer / Host / token checks, or the admin web listener (adminweb.rs, LG-387) after its session check, which
/// passes the write guard of its own origin and session for the beta page.
pub fn serve(
    shared: &Shared,
    req: &crate::clanweb::Request,
    head: &str,
    body: &[u8],
    guard: Option<&crate::betaweb::WriteGuard>,
) -> (u16, &'static str, String) {
    let path = req.path.trim_end_matches('/');
    // LG-375: the beta list has its own methods and its own write guard (betaweb.rs)
    if crate::betaweb::admin_handles(path) {
        return crate::betaweb::admin_guarded(shared, req, head, body, guard);
    }
    // LG-390: the Bots tab's JSON and writes (botgroups.rs)
    if crate::botgroups::admin_handles(path) {
        return crate::botgroups::admin(shared, req, head, body, guard);
    }
    // LG-395..LG-398: the Players tab's account panel and its writes (playeradmin.rs)
    if crate::playeradmin::handles(path) {
        return crate::playeradmin::serve(shared, req, head, body, guard);
    }
    // LG-392: the Updates tab (deploys from the dashboard) has its own write
    if crate::deploy::handles(path) {
        return crate::deploy::serve(shared, req, head, body, guard);
    }
    if req.method != "GET" {
        return err(405, "method not allowed");
    }
    let q = &req.query;
    let num = |k: &str, d: u32| q.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(d);
    let json = |r: rusqlite::Result<dc_mod_lua::JsonValue>| match r {
        Ok(v) => (200, JSON, v.to_json_string()),
        Err(e) => err(500, &format!("db: {e}")),
    };
    match path {
        // LG-390: the page carries the token its writes send (the admin web's session token, else the per-process one)
        "/ops" => {
            let tok = match guard {
                Some(w) => w.csrf.clone(),
                None => {
                    let mut g = crate::net::lock(shared);
                    let st = &mut *g;
                    st.beta.csrf(&mut st.keys)
                }
            };
            (200, HTML, PAGE.replace("__CSRF__", &tok))
        }
        "/ops/live" => {
            let g = crate::net::lock(shared);
            (200, JSON, crate::opsmon::live_json(&g).to_json_string())
        }
        "/ops/events" => {
            let g = crate::net::lock(shared);
            json(crate::opsmon::events_json(
                &g.db,
                q.get("problems").is_some_and(|v| v == "1" || v == "true"),
                num("limit", 100),
            ))
        }
        "/ops/matches" => {
            let g = crate::net::lock(shared);
            json(crate::opsmon::matches_json(&g.db, num("limit", 50)))
        }
        "/ops/match" => {
            let Some(id) = q.get("id").and_then(|v| v.trim().parse::<i64>().ok()) else {
                return err(400, "id=<match id>");
            };
            let g = crate::net::lock(shared);
            match crate::opsmon::match_json(&g.db, id) {
                Ok(Some(v)) => (200, JSON, v.to_json_string()),
                Ok(None) => err(404, &format!("no match {id}")),
                Err(e) => err(500, &format!("db: {e}")),
            }
        }
        "/ops/players" => {
            let g = crate::net::lock(shared);
            let online: std::collections::BTreeSet<u32> = g
                .sessions
                .values()
                .filter(|x| x.in_lobby && !x.closing)
                .filter_map(|x| x.account)
                .collect();
            let text = |k: &str| q.get(k).cloned().unwrap_or_default();
            // LG-398: `banned=1` keeps the banned accounts
            match crate::opsmon::players_json_at(
                &g.db,
                &online,
                &text("q"),
                text("bots") == "1",
                text("banned") == "1",
                &text("sort"),
                num("offset", 0),
                num("limit", 50),
                (crate::clock::unix_now(), crate::clock::now_ticks()),
            ) {
                Ok(v) => (200, JSON, v.to_json_string()),
                Err(e) => err(500, &format!("db: {e}")),
            }
        }
        "/ops/timeline" => {
            let name = q.get("name").cloned().unwrap_or_default();
            if name.trim().is_empty() {
                return err(400, "name=<character or login>");
            }
            let g = crate::net::lock(shared);
            match crate::opsmon::timeline_json(&g.db, &name, num("limit", 300)) {
                Ok(Some(v)) => (200, JSON, v.to_json_string()),
                Ok(None) => err(404, "no account or character with that name"),
                Err(e) => err(500, &format!("db: {e}")),
            }
        }
        "/ops/alerts" => {
            let t = crate::opsmon::Thresholds::from_query(q);
            let g = crate::net::lock(shared);
            let mut checks = crate::opsmon::host_checks(&g);
            match crate::opsmon::db_checks(&g.db, crate::clock::unix_now(), &t) {
                Ok(c) => checks.extend(c),
                Err(e) => checks.push(("db".into(), false, format!("monitor query failed: {e}"))),
            }
            (200, JSON, crate::opsmon::alerts_json(&checks).to_json_string())
        }
        _ => err(404, "not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::sync::{Arc, Mutex};

    const LO: Option<IpAddr> = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));

    fn shared() -> Shared {
        Arc::new(Mutex::new(crate::state::testing::state()))
    }

    #[test]
    fn host_names() {
        assert!(host_is_loopback("GET / HTTP/1.1\r\nHost: 127.0.0.1:10090\r\n\r\n"));
        assert!(host_is_loopback("GET / HTTP/1.1\r\nhost: localhost\r\n\r\n"));
        assert!(host_is_loopback("GET / HTTP/1.1\r\nHost: [::1]:8\r\n\r\n"));
        assert!(host_is_loopback("GET / HTTP/1.0\r\n\r\n"));
        assert!(!host_is_loopback("GET / HTTP/1.1\r\nHost: evil.example:10090\r\n\r\n"));
        assert!(!host_is_loopback("GET / HTTP/1.1\r\nHost: 192.0.2.4\r\n\r\n"));
    }

    #[test]
    fn other_paths_are_not_ours() {
        assert!(handle(&shared(), "GET /status HTTP/1.1\r\n\r\n", LO, None, true).is_none());
        assert!(handle(&shared(), "GET /opsx HTTP/1.1\r\n\r\n", LO, None, true).is_none());
    }

    #[test]
    fn the_page_and_its_json_answer_loopback_only() {
        let sh = shared();
        let get = |p: &str, peer, host: &str, token, ok| {
            handle(&sh, &format!("GET {p} HTTP/1.1\r\nHost: {host}\r\n\r\n"), peer, token, ok).unwrap()
        };
        let (st, ct, body) = get("/ops", LO, "127.0.0.1:10090", None, true);
        assert_eq!((st, ct), (200, HTML));
        assert!(body.contains("<title>") && !body.contains("<script src") && !body.contains("<link"));
        for p in ["/ops/live", "/ops/events?problems=1", "/ops/matches", "/ops/alerts"] {
            let (st, ct, body) = get(p, LO, "localhost:10090", None, true);
            assert_eq!((st, ct), (200, JSON), "{p}: {body}");
        }
        // a remote peer is refused even with the right token (an admin port opened with --admin-public)
        let remote = Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)));
        assert_eq!(get("/ops/live", remote, "127.0.0.1", Some("t"), true).0, 403);
        assert_eq!(get("/ops", remote, "127.0.0.1", None, true).0, 403);
        // a rebound name, a missing token
        assert_eq!(get("/ops/live", LO, "attacker.example", None, true).0, 403);
        assert_eq!(get("/ops/live", LO, "127.0.0.1", Some("t"), false).0, 401);
        assert_eq!(get("/ops/live", LO, "127.0.0.1", Some("t"), true).0, 200);
        // bad arguments and writes
        assert_eq!(get("/ops/match", LO, "127.0.0.1", None, true).0, 400);
        assert_eq!(get("/ops/match?id=99", LO, "127.0.0.1", None, true).0, 404);
        assert_eq!(get("/ops/timeline?name=", LO, "127.0.0.1", None, true).0, 400);
        assert_eq!(get("/ops/timeline?name=nobody", LO, "127.0.0.1", None, true).0, 404);
        assert_eq!(get("/ops/nope", LO, "127.0.0.1", None, true).0, 404);
        let post = handle(&sh, "POST /ops/live HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", LO, None, true).unwrap();
        assert_eq!(post.0, 405);
    }
}
