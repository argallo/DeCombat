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
pub const JS: &str = "text/javascript; charset=utf-8";
pub const CSS: &str = "text/css; charset=utf-8";
pub const TEXT: &str = "text/plain; charset=utf-8";

// DC-DEFAULT: LG-441 (proposed) the dashboard's app: a page without inline script and the lobby's own script and
// stylesheet, built from web/admin and embedded here (the servers build with cargo alone)
/// The page of the dashboard's app (`GET /ops`, LG-444): no inline script; it loads [`app_file`]'s script and stylesheet.
pub(crate) const APP_SHELL: &str = include_str!("admin_app/shell.html");
const APP_JS: &str = include_str!("admin_app/app.js");
const APP_CSS: &str = include_str!("admin_app/app.css");
const APP_NOTICES: &str = include_str!("admin_app/THIRD_PARTY_NOTICES.txt");

/// The boot data of the app's page on the loopback admin port: one lobby, no session, no further instance.
pub const LOOPBACK_BOOT: &str = r#"{"web":false,"label":"","current":null,"instances":[]}"#;

/// The app's page: `csrf` is the page token its writes send, `asset_query` what follows the addresses of its two files
/// (the loopback port's `?token=...`, else empty), `boot` the JSON the app reads from `<body data-boot>` (who serves
/// the page, which lobbies there are). Each value is HTML-escaped; the template is read once, so nothing put into the
/// page is searched for a placeholder again.
pub fn app_shell(csrf: &str, asset_query: &str, boot: &str) -> String {
    use crate::clanweb::esc;
    let fills = [
        ("__CSRF__", esc(csrf)),
        ("__ASSETQ__", esc(asset_query)),
        ("__BOOT__", esc(boot)),
    ];
    let mut out = String::with_capacity(APP_SHELL.len() + 512);
    let mut rest = APP_SHELL;
    while let Some(i) = rest.find("__") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match fills.iter().find(|(name, _)| tail.starts_with(name)) {
            Some((name, value)) => {
                out.push_str(value);
                rest = &tail[name.len()..];
            }
            None => {
                out.push_str("__");
                rest = &tail[2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// True for the request of the app's page (`GET /ops`): its answer carries the page policy without inline script
/// (admin.rs, LG-442).
pub fn is_app_page(head: &str) -> bool {
    parse_request(head)
        .is_some_and(|r| r.method == "GET" && r.path.trim_end_matches('/') == APP_PAGE_PATH)
}

// DC-DEFAULT: LG-444 (proposed) /ops is the dashboard app; the earlier page stays at /ops/classic
/// LG-444: the address of the dashboard app's page.
pub const APP_PAGE_PATH: &str = "/ops";
/// LG-444: the address of the earlier dashboard page (inline script and style), kept until every screen has moved
/// into the app.
pub const CLASSIC_PAGE_PATH: &str = "/ops/classic";

/// The app's embedded files by their exact path: the script, the stylesheet, and the licence texts of the packages
/// the build bundled. Nothing is read from disk and no part of the path names a file.
pub fn app_file(path: &str) -> Option<(&'static str, &'static str)> {
    match path {
        "/ops/app.js" => Some((JS, APP_JS)),
        "/ops/app.css" => Some((CSS, APP_CSS)),
        "/ops/licenses.txt" => Some((TEXT, APP_NOTICES)),
        _ => None,
    }
}

// DC-DEFAULT: LG-443 (proposed) the admin web lets a browser keep the app's script and stylesheet of one build
/// The name of this build's script and stylesheet: 16 hex digits of the SHA-256 over both. The admin web's page names
/// its two files with it (`?v=`), so another build is another address.
pub fn app_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let mut all = Vec::with_capacity(APP_JS.len() + APP_CSS.len() + 1);
        all.extend_from_slice(APP_JS.as_bytes());
        all.push(0);
        all.extend_from_slice(APP_CSS.as_bytes());
        crate::webauthn::hex(&crate::webauthn::sha256(&all))[..16].to_string()
    })
}

/// What follows the addresses of the app's two files on the admin web's page (LG-443).
pub fn app_asset_query() -> String {
    format!("?v={}", app_version())
}

/// The `Cache-Control` of every answer but the one [`app_cache_control`] names.
pub const NO_STORE: &str = "no-store";
/// LG-443: the browser alone (no shared cache) may keep the answer, and need not ask again.
pub const APP_FILE_CACHE: &str = "private, max-age=31536000, immutable";

/// LG-443: the `Cache-Control` of the admin web's answer to `req`. Only the app's script and stylesheet, asked by this
/// build's address and answered with 200, may be kept; a refusal, another build's address, the address without a
/// version and everything else stay `no-store`.
pub fn app_cache_control(req: &crate::clanweb::Request, status: u16) -> &'static str {
    let keeps = status == 200
        && req.method == "GET"
        && matches!(req.path.as_str(), "/ops/app.js" | "/ops/app.css")
        && req.query.get("v").is_some_and(|v| v == app_version());
    if keeps {
        APP_FILE_CACHE
    } else {
        NO_STORE
    }
}

/// The value of header `name` (any case), trimmed and lower case.
fn header_lower(head: &str, name: &str) -> Option<String> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_ascii_lowercase())
    })
}

/// True when the `Host` header names a loopback address (any port), or is absent (HTTP/1.0 tools).
pub(crate) fn host_is_loopback(head: &str) -> bool {
    match header_lower(head, "host") {
        Some(v) => loopback_name(&v),
        None => true,
    }
}

/// LG-415: true when the request carries no `Origin` (curl, the tools, a same-origin GET) or one whose host is a
/// loopback name (`http://localhost:10090`, `http://127.0.0.1:10090`); `null` and every other site are refused.
// DC-DEFAULT: LG-415 (proposed) the loopback admin port's own paths refuse another site's browser requests
pub(crate) fn origin_is_loopback(head: &str) -> bool {
    let Some(o) = header_lower(head, "origin") else {
        return true;
    };
    o.strip_prefix("http://")
        .or_else(|| o.strip_prefix("https://"))
        .is_some_and(|rest| !rest.contains('/') && loopback_name(rest))
}

/// `name[:port]` / `[v6][:port]`: a loopback name or address.
fn loopback_name(v: &str) -> bool {
    let v = v.to_string();
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
        format!(
            "{{\"ok\": false, \"message\": {}}}",
            dc_mod_lua::JsonValue::String(text.into()).to_json_string()
        ),
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
        return Some(err(
            403,
            "the dashboard answers loopback only (use an ssh tunnel)",
        ));
    }
    if !host_is_loopback(head) {
        return Some(err(403, "Host must be a loopback name"));
    }
    if token.is_some() && !token_ok {
        return Some(err(401, "admin token missing or wrong"));
    }
    // LG-393: a deploy from the loopback port needs the admin token (`--admin-token` / DC_ADMIN_TOKEN), checked
    // above; a port without one may read the Updates tab but not deploy (the admin web deploys behind its key)
    if req.method == "POST"
        && (crate::deploy::handles(path) || crate::triage::handles(path))
        && token.is_none()
    {
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
    // OPS-26: the Updates tab's bug reports (triage.rs): the list and the done mark
    if crate::triage::handles(path) {
        return crate::triage::serve_at(
            shared,
            req,
            head,
            body,
            guard,
            None,
            crate::clock::unix_now(),
        );
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
    if let Some((ctype, body)) = app_file(path) {
        return (200, ctype, body.to_string());
    }
    // LG-390: the page carries the token its writes send (the admin web's session token, else the per-process one)
    let page_token = || match guard {
        Some(w) => w.csrf.clone(),
        None => {
            let mut g = crate::net::lock(shared);
            let st = &mut *g;
            st.beta.csrf(&mut st.keys)
        }
    };
    match path {
        // LG-444: the earlier page at its own address
        CLASSIC_PAGE_PATH => (200, HTML, PAGE.replace("__CSRF__", &page_token())),
        // LG-441, LG-444: the app's page as the loopback admin port serves it (the admin web fills in its own boot
        // data, adminweb.rs); the admin token of the page's address goes on to the page's two files
        APP_PAGE_PATH => {
            let asset_query = match q.get("token") {
                Some(t) if guard.is_none() && !t.is_empty() => {
                    format!("?token={}", crate::clanweb::url_encode(t))
                }
                _ => String::new(),
            };
            (
                200,
                HTML,
                app_shell(&page_token(), &asset_query, LOOPBACK_BOOT),
            )
        }
        // OPS-14: the weapons the loadout lists offer
        "/ops/guns" => {
            let g = crate::net::lock(shared);
            (200, JSON, crate::opsmon::guns_json(&g).to_json_string())
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
            (
                200,
                JSON,
                crate::opsmon::alerts_json(&checks).to_json_string(),
            )
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
        assert!(host_is_loopback(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:10090\r\n\r\n"
        ));
        assert!(host_is_loopback(
            "GET / HTTP/1.1\r\nhost: localhost\r\n\r\n"
        ));
        assert!(host_is_loopback("GET / HTTP/1.1\r\nHost: [::1]:8\r\n\r\n"));
        assert!(host_is_loopback("GET / HTTP/1.0\r\n\r\n"));
        assert!(!host_is_loopback(
            "GET / HTTP/1.1\r\nHost: evil.example:10090\r\n\r\n"
        ));
        assert!(!host_is_loopback(
            "GET / HTTP/1.1\r\nHost: 192.0.2.4\r\n\r\n"
        ));
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
            handle(
                &sh,
                &format!("GET {p} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
                peer,
                token,
                ok,
            )
            .unwrap()
        };
        let (st, ct, body) = get("/ops/classic", LO, "127.0.0.1:10090", None, true);
        assert_eq!((st, ct), (200, HTML));
        assert!(
            body.contains("<title>") && !body.contains("<script src") && !body.contains("<link")
        );
        // OPS-13 / OPS-14: the page names a match's map by `map_name`, lists the guns by name, and starts a new group
        // with one channel ticked
        assert!(
            body.contains("m.map_name")
                && body.contains("/ops/guns")
                && body.contains("loadoutPicker")
                && body.contains("i === 0")
                && !body.contains("m.map_world")
        );
        for p in [
            "/ops/live",
            "/ops/events?problems=1",
            "/ops/matches",
            "/ops/alerts",
            "/ops/guns",
        ] {
            let (st, ct, body) = get(p, LO, "localhost:10090", None, true);
            assert_eq!((st, ct), (200, JSON), "{p}: {body}");
        }
        // a remote peer is refused even with the right token (an admin port opened with --admin-public)
        let remote = Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)));
        assert_eq!(
            get("/ops/live", remote, "127.0.0.1", Some("t"), true).0,
            403
        );
        assert_eq!(get("/ops", remote, "127.0.0.1", None, true).0, 403);
        // a rebound name, a missing token
        assert_eq!(get("/ops/live", LO, "attacker.example", None, true).0, 403);
        assert_eq!(get("/ops/live", LO, "127.0.0.1", Some("t"), false).0, 401);
        assert_eq!(get("/ops/live", LO, "127.0.0.1", Some("t"), true).0, 200);
        // bad arguments and writes
        assert_eq!(get("/ops/match", LO, "127.0.0.1", None, true).0, 400);
        assert_eq!(get("/ops/match?id=99", LO, "127.0.0.1", None, true).0, 404);
        assert_eq!(
            get("/ops/timeline?name=", LO, "127.0.0.1", None, true).0,
            400
        );
        assert_eq!(
            get("/ops/timeline?name=nobody", LO, "127.0.0.1", None, true).0,
            404
        );
        assert_eq!(get("/ops/nope", LO, "127.0.0.1", None, true).0, 404);
        let post = handle(
            &sh,
            "POST /ops/live HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            LO,
            None,
            true,
        )
        .unwrap();
        assert_eq!(post.0, 405);
    }
    // LG-441: the app's page (no inline script, the page token, the admin token passed on to its two files) and the
    // embedded files, by exact path, behind the same loopback / Host / token checks as the JSON
    #[test]
    fn the_app_page_and_its_embedded_files() {
        let sh = shared();
        let get = |p: &str, peer, token, ok| {
            handle(
                &sh,
                &format!("GET {p} HTTP/1.1\r\nHost: 127.0.0.1:10090\r\n\r\n"),
                peer,
                token,
                ok,
            )
            .unwrap()
        };
        // LG-444: /ops is the app's page, with or without the closing slash; /ops/next is gone from this port
        let (st, ct, page) = get("/ops", LO, None, true);
        assert_eq!((st, ct), (200, HTML));
        assert_eq!(get("/ops/", LO, None, true).2, page);
        assert_eq!(get("/ops/next", LO, None, true).0, 404);
        for hole in ["__CSRF__", "__ASSETQ__", "__BOOT__"] {
            assert!(!page.contains(hole), "{hole} was not filled in");
        }
        assert!(
            page.contains("<link rel=\"stylesheet\" href=\"/ops/app.css\">")
                && page.contains("<script type=\"module\" src=\"/ops/app.js\"></script>")
        );
        // no inline script: the one script element names a file
        assert_eq!(page.matches("<script").count(), 1);
        assert!(crate::adminweb::inline_script_hashes(&page).is_empty());
        // the page token sits in the same meta tag as on the earlier page (the instance forwarder reads it there)
        let tok = |h: &str| {
            h.split("<meta name=\"csrf\" content=\"")
                .nth(1)
                .and_then(|r| r.split('"').next())
                .map(str::to_string)
        };
        let (st, _, old) = get("/ops/classic", LO, None, true);
        assert_eq!(st, 200);
        assert!(tok(&page).is_some_and(|t| t.len() >= 16) && tok(&page) == tok(&old));
        // the earlier page is the one with the inline script, and it links the app's page
        assert!(old.contains("<script>") && !old.contains("/ops/app.js"));
        assert!(old.contains("id=\"new-link\""));
        assert!(page.contains("data-boot=\"{&quot;web&quot;:false,"));

        // the admin token of the page's address goes on to its files, percent-encoded
        let (_, _, page) = get("/ops?token=a%20b%22%3E", LO, Some("a b\">"), true);
        assert!(
            page.contains("href=\"/ops/app.css?token=a%20b%22%3E\"")
                && page.contains("src=\"/ops/app.js?token=a%20b%22%3E\""),
            "{page}"
        );

        for (p, want) in [
            ("/ops/app.js", JS),
            ("/ops/app.css", CSS),
            ("/ops/licenses.txt", TEXT),
        ] {
            let (st, ct, body) = get(p, LO, None, true);
            assert_eq!((st, ct), (200, want), "{p}");
            assert!(body.len() > 1000, "{p}");
            assert!(!body.contains("sourceMappingURL"), "{p}");
            // a missing or wrong admin token, another machine
            assert_eq!(get(p, LO, Some("t"), false).0, 401, "{p}");
            let remote = Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)));
            assert_eq!(get(p, remote, None, true).0, 403, "{p}");
        }
        // exact paths only: no other file of the folder, nothing below or beside a file
        for p in [
            "/ops/app.js.map",
            "/ops/app",
            "/ops/app.css/x",
            "/ops/shell.html",
            "/ops/admin_app/app.js",
            "/ops/THIRD_PARTY_NOTICES.txt",
            "/ops/classic/app.js",
            "/ops/classic/x",
        ] {
            assert_eq!(get(p, LO, None, true).0, 404, "{p}");
        }
        let post = handle(
            &sh,
            "POST /ops/app.js HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            LO,
            None,
            true,
        )
        .unwrap();
        assert_eq!(post.0, 405);
    }

    #[test]
    fn the_apps_files_may_be_kept_by_their_builds_address_only() {
        let v = app_version();
        assert!(
            v.len() == 16 && v.bytes().all(|b| b.is_ascii_hexdigit()),
            "{v}"
        );
        assert_eq!(app_asset_query(), format!("?v={v}"));
        let cc = |line: &str, status| {
            let r = parse_request(&format!("{line} HTTP/1.1\r\nHost: h\r\n\r\n")).unwrap();
            app_cache_control(&r, status)
        };
        assert_eq!(cc(&format!("GET /ops/app.js?v={v}"), 200), APP_FILE_CACHE);
        assert_eq!(cc(&format!("GET /ops/app.css?v={v}"), 200), APP_FILE_CACHE);
        assert_eq!(
            cc(&format!("GET /ops/app.css?x=1&v={v}"), 200),
            APP_FILE_CACHE
        );
        // a refusal is never kept, nor another build's address, nor the address without a version
        assert_eq!(cc(&format!("GET /ops/app.js?v={v}"), 401), NO_STORE);
        assert_eq!(cc(&format!("GET /ops/app.js?v={v}"), 404), NO_STORE);
        assert_eq!(cc("GET /ops/app.js?v=0123456789abcdef", 200), NO_STORE);
        assert_eq!(cc(&format!("GET /ops/app.js?v={v}0"), 200), NO_STORE);
        assert_eq!(cc("GET /ops/app.js?v=", 200), NO_STORE);
        assert_eq!(cc("GET /ops/app.js", 200), NO_STORE);
        // nothing else: not the page, not the licence texts, not JSON, not a write
        for p in [
            "/ops/classic",
            "/ops/licenses.txt",
            "/ops/live",
            "/ops",
            "/ops/app.js/",
        ] {
            assert_eq!(cc(&format!("GET {p}?v={v}"), 200), NO_STORE, "{p}");
        }
        assert_eq!(cc(&format!("POST /ops/app.js?v={v}"), 200), NO_STORE);
    }

    // a value that holds a placeholder's name is put in as text, never filled in again; every value is escaped
    #[test]
    fn the_app_page_is_filled_in_one_pass() {
        let page = app_shell(
            "__BOOT__",
            "?token=__CSRF__",
            "{\"label\":\"__ASSETQ__ \\\" <b>\"}",
        );
        assert!(page.contains("content=\"__BOOT__\""), "{page}");
        assert!(page.contains("/ops/app.js?token=__CSRF__\""), "{page}");
        assert!(
            page.contains(
                "data-boot=\"{&quot;label&quot;:&quot;__ASSETQ__ \\&quot; &lt;b&gt;&quot;}\""
            ),
            "{page}"
        );
    }
}
