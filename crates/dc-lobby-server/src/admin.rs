//! The operator endpoint `--admin ADDR:PORT`: plain HTTP/1.1, one request per connection, no body needed.
//!
//! - `GET /status`: JSON (uptime, logins, connections, rooms with mode / map / players / bots / game port / state,
//!   DB and ROM paths, addresses).
//! - `GET /net`: JSON, every player of every running game with the newest 10 s link window of its game server
//!   (round trip mean / min / max, input jitter, resends, flow drops, datagrams) and its lobby connection age
//!   (LG-185).
//! - `POST /stop`: the graceful stop (LG-130); answers 202 before it starts.
//! - `POST /rooms/<channel>-<no>/close`: ends that room's match and closes the room.
//! - `GET /ops` and `/ops/...`: the operator dashboard and its JSON (opsweb.rs; loopback peers only, always).
//!
//! DC-DEFAULT: LG-131 (proposed) loopback only unless `--admin-public`, which needs `--admin-token`; with a token
//! every request carries it (`X-Admin-Token: <t>`, `Authorization: Bearer <t>` or `?token=<t>`). The request
//! reader and the request-line parser are the clan web server's (clanweb.rs).

use crate::clanweb::{parse_request, read_body, read_head_and_rest};
use crate::room::RoomKey;
use crate::state::Shared;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};

/// `--admin` text -> socket address: a bare port means 127.0.0.1:<port>.
pub fn parse_addr(spec: &str) -> Result<SocketAddr, String> {
    let spec = spec.trim();
    if let Ok(port) = spec.parse::<u16>() {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
    }
    spec.parse::<SocketAddr>().map_err(|_| {
        format!("--admin {spec}: give ADDR:PORT (for example 127.0.0.1:10090) or a port")
    })
}

/// The address to bind, or why the flags are refused.
pub fn resolve(spec: &str, public: bool, token: Option<&str>) -> Result<SocketAddr, String> {
    let a = parse_addr(spec)?;
    if !a.ip().is_loopback() {
        if !public {
            return Err(format!(
                "--admin {a} is not a loopback address; add --admin-public (and --admin-token) to allow it"
            ));
        }
        if token.is_none() {
            return Err(format!(
                "--admin {a} with --admin-public needs --admin-token"
            ));
        }
    }
    Ok(a)
}

fn reply(status: u16, ctype: &str, body: &str) -> Vec<u8> {
    let text = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    // MON-4: the dashboard page loads nothing from elsewhere and never sends its address (with the token) on
    let extra = if ctype == crate::opsweb::HTML {
        "Content-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
         connect-src 'self'; img-src data:\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n"
    } else {
        ""
    };
    let mut v = format!(
        "HTTP/1.1 {status} {text}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    v.extend_from_slice(body.as_bytes());
    v
}

fn msg(ok: bool, text: &str) -> String {
    let mut o = std::collections::BTreeMap::new();
    o.insert("ok".to_string(), dc_mod_lua::JsonValue::Bool(ok));
    o.insert(
        "message".to_string(),
        dc_mod_lua::JsonValue::String(text.to_string()),
    );
    dc_mod_lua::JsonValue::Object(o).to_json_pretty()
}

/// The token a request carries, from its headers or its query.
fn request_token(head: &str, query: Option<&String>) -> Option<String> {
    for line in head.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("x-admin-token") {
            return Some(value.to_string());
        }
        if name.eq_ignore_ascii_case("authorization") {
            if let Some(t) = value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
            {
                return Some(t.trim().to_string());
            }
        }
    }
    query.cloned()
}

/// True when no token is set, or the request carries it.
fn token_ok(head: &str, token: Option<&str>) -> bool {
    let Some(t) = token else {
        return true;
    };
    let q = parse_request(head).and_then(|r| r.query.get("token").cloned());
    request_token(head, q.as_ref()).is_some_and(|g| crate::hosts::token_eq(&g, t))
}

/// `<channel>-<no>` -> room key.
pub fn room_key(id: &str) -> Option<RoomKey> {
    let (c, n) = id.split_once('-')?;
    Some(RoomKey {
        channel: c.parse().ok()?,
        no: n.parse().ok()?,
    })
}

/// One request: (status, JSON body).
pub fn handle(
    shared: &Shared,
    head: &str,
    peer: Option<IpAddr>,
    token: Option<&str>,
) -> (u16, String) {
    let Some(req) = parse_request(head) else {
        return (400, msg(false, "bad request"));
    };
    match token {
        Some(t) => {
            let got = request_token(head, req.query.get("token"));
            if !got.is_some_and(|g| crate::hosts::token_eq(&g, t)) {
                return (401, msg(false, "admin token missing or wrong"));
            }
        }
        None => {
            if !peer.map(|p| p.is_loopback()).unwrap_or(false) {
                return (403, msg(false, "loopback only"));
            }
        }
    }
    let path = req.path.trim_end_matches('/');
    let method = req.method.as_str();
    match (method, path) {
        ("GET", "/status") | ("GET", "") => {
            let g = crate::net::lock(shared);
            (200, crate::lifecycle::status_json(&g))
        }
        // DC-DEFAULT: LG-185 (proposed) the players' link windows of every running game
        ("GET", "/net") => {
            let g = crate::net::lock(shared);
            (200, crate::lifecycle::net_json(&g))
        }
        ("POST", "/stop") => {
            crate::net::lock(shared).note("admin: stop requested");
            crate::lifecycle::request_stop();
            (202, msg(true, "stopping"))
        }
        (_, "/status") | (_, "/stop") | (_, "/net") => (405, msg(false, "method not allowed")),
        _ => {
            let close = path
                .strip_prefix("/rooms/")
                .and_then(|r| r.strip_suffix("/close"));
            match close {
                Some(id) if method != "POST" => {
                    let _ = id;
                    (405, msg(false, "method not allowed"))
                }
                Some(id) => {
                    let Some(key) = room_key(id) else {
                        return (400, msg(false, "room id is <channel>-<no>"));
                    };
                    let mut g = crate::net::lock(shared);
                    if !g.rooms.contains_key(&key) {
                        return (404, msg(false, &format!("no room {id}")));
                    }
                    let r =
                        g.scope(|st| crate::lifecycle::close_room_now(st, key, "admin request"));
                    crate::net::run_deferred(shared, &mut g);
                    match r {
                        Ok(()) => (200, msg(true, &format!("room {id} closed"))),
                        Err(e) => (500, msg(false, &format!("room {id}: {e}"))),
                    }
                }
                None => (404, msg(false, "not found")),
            }
        }
    }
}

fn serve(shared: Shared, token: Option<String>, mut s: TcpStream) {
    let peer = s.peer_addr().ok().map(|a| a.ip());
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let Some((head, rest)) = read_head_and_rest(&mut s) else {
        return;
    };
    // LG-375: the beta page's writes carry a JSON body (capped); every other request's body is dropped as before
    let req_body = match parse_request(&head) {
        Some(r)
            if r.method == "POST"
                && (crate::betaweb::admin_handles(&r.path)
                    || crate::botgroups::admin_handles(&r.path)
                    || crate::playeradmin::handles(&r.path)
                    || crate::deploy::handles(&r.path)) =>
        {
            match read_body(&mut s, &head, rest, crate::betaweb::ADMIN_MAX_BODY) {
                Ok(b) => b,
                Err(code) => {
                    let _ = s.write_all(&reply(
                        code,
                        crate::opsweb::JSON,
                        "{\"ok\": false, \"message\": \"bad or too large body\"}",
                    ));
                    let _ = s.shutdown(std::net::Shutdown::Both);
                    return;
                }
            }
        }
        _ => Vec::new(),
    };
    let tk = token.as_deref();
    let (status, ctype, body) = match crate::opsweb::handle_with_body(
        &shared,
        &head,
        &req_body,
        peer,
        tk,
        token_ok(&head, tk),
    ) {
        Some(r) => r,
        None => {
            let (status, body) = handle(&shared, &head, peer, tk);
            (status, crate::opsweb::JSON, body)
        }
    };
    let _ = s.write_all(&reply(status, ctype, &body));
    let _ = s.flush();
    let _ = s.shutdown(std::net::Shutdown::Both);
}

/// Serves the admin endpoint on an already bound listener.
pub fn spawn(shared: Shared, l: TcpListener, token: Option<String>) {
    let _ = std::thread::Builder::new()
        .name("admin-accept".into())
        .spawn(move || {
            for s in l.incoming() {
                match s {
                    Ok(stream) => {
                        let sh = shared.clone();
                        let tk = token.clone();
                        let _ = std::thread::Builder::new()
                            .name("admin".into())
                            .spawn(move || serve(sh, tk, stream));
                    }
                    Err(e) => crate::net::lock(&shared).note(&format!("admin accept: {}", e)),
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn shared() -> Shared {
        Arc::new(Mutex::new(crate::state::testing::state()))
    }

    const LO: Option<IpAddr> = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));

    #[test]
    fn addresses_default_to_loopback_and_public_needs_flag_and_token() {
        assert_eq!(parse_addr("10090").unwrap().to_string(), "127.0.0.1:10090");
        assert!(resolve("127.0.0.1:10090", false, None).is_ok());
        assert!(resolve("0.0.0.0:10090", false, None).is_err());
        assert!(resolve("0.0.0.0:10090", true, None).is_err());
        assert!(resolve("0.0.0.0:10090", true, Some("t")).is_ok());
        assert!(parse_addr("nonsense").is_err());
    }

    #[test]
    fn status_answers_and_unknown_paths_are_404() {
        let sh = shared();
        let (st, body) = handle(&sh, "GET /status HTTP/1.1\r\n\r\n", LO, None);
        assert_eq!(st, 200);
        assert!(body.contains("\"rooms\""));
        assert_eq!(handle(&sh, "GET /nope HTTP/1.1\r\n\r\n", LO, None).0, 404);
        assert_eq!(handle(&sh, "GET /stop HTTP/1.1\r\n\r\n", LO, None).0, 405);
        assert_eq!(
            handle(&sh, "POST /rooms/1-9/close HTTP/1.1\r\n\r\n", LO, None).0,
            404
        );
        assert_eq!(
            handle(&sh, "POST /rooms/x/close HTTP/1.1\r\n\r\n", LO, None).0,
            400
        );
    }

    #[test]
    fn a_token_is_required_when_set_and_remote_peers_need_one() {
        let sh = shared();
        let remote = Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)));
        assert_eq!(
            handle(&sh, "GET /status HTTP/1.1\r\n\r\n", remote, None).0,
            403
        );
        let t = Some("s3cret");
        assert_eq!(handle(&sh, "GET /status HTTP/1.1\r\n\r\n", LO, t).0, 401);
        assert_eq!(
            handle(
                &sh,
                "GET /status HTTP/1.1\r\nX-Admin-Token: s3cret\r\n\r\n",
                remote,
                t
            )
            .0,
            200
        );
        assert_eq!(
            handle(
                &sh,
                "GET /status HTTP/1.1\r\nAuthorization: Bearer s3cret\r\n\r\n",
                LO,
                t
            )
            .0,
            200
        );
        assert_eq!(
            handle(&sh, "GET /status?token=s3cret HTTP/1.1\r\n\r\n", LO, t).0,
            200
        );
    }
}
