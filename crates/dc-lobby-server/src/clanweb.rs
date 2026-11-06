//! Clan web pages (CD-47): a minimal HTTP/1.1 server on `--clanweb` (default `port + 8080`, 0 = off) that serves the
//! create / join / leave / manage pages the 2009 Clan screen opens in its embedded browser.
//!
//! The client opens three URL pointers (cs+0x4475d8 / +0x4475dc / +0x4475e0) with the login token appended; our
//! launcher points them at this server (SPEC: PROTOCOL_2009.md §5.2). The pages, their paths and their HTML are our
//! own. Every request is one state lock and one transaction (`Core::scope`), so a leave pushes 0x3064 to the user
//! exactly as the chat and packet paths do.
//!
//! Paths:
//! - `GET /clan?url=<page>&p=<passport>`: pages `guild/list` (find a clan), `manager/guildmember` (members),
//!   `guild/create` (create form), `guild/leave` (leave form); anything else shows the home page.
//! - `GET /clan/create?p=&name=&emblem=&bg=`, `/clan/join?p=&c=<clan number>`, `/clan/leave?p=`,
//!   `/clan/kick?p=&m=<character number>`, `/clan/notice?p=&text=`: actions; the reply carries one message line.
//!
//! - `GET /mods/<sha256>.zip`, `GET /server_list.json`: mod hosting (`hosting.rs`), no account needed.
//!
//! `p` (also `strPassport` / `passport`) is the login token, with or without the `LOCAL:` prefix.
//!
//! `fmt=text` on any clan path answers with tab-separated lines instead of HTML (the DeCombat client draws the pages
//! itself, it has no browser): `me<TAB>clan number<TAB>1 when leader`, `msg<TAB>1 ok / 0 refused<TAB>text` for an
//! action, and `clan<TAB>number<TAB>name<TAB>members<TAB>leader<TAB>emblem<TAB>background` rows for the clan list
//! (`url=guild/list`) and after a join / leave, `member<TAB>character number<TAB>1 when leader<TAB>name<TAB>exp`
//! rows for the member page (`url=manager/guildmember`) and after a kick. `q=<text>` on the clan list keeps the
//! clans whose name holds the text (any case); an unknown account gets `me 0 0` and a refused `msg` line (LG-414).
// DC-DEFAULT: LG-414 (proposed) the find page's search and the text form's unknown-account answer
// DC-DEFAULT: CD-47 clan web pages served by our own server; a leave makes the lobby push 0x3064 to that user
// SPEC: LOBBY_HARNESS.md §5 (query parameters and the page fragments the harness matches)

use crate::clan::{self, Refusal};
use crate::hosting::Hosting;
use crate::state::{Core, Shared};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

/// Largest request head we read.
const MAX_HEAD: usize = 16 * 1024;

pub type Query = BTreeMap<String, String>;

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `application/x-www-form-urlencoded` decoding (RFC 3986 percent escapes, `+` = space), UTF-8 with replacement.
pub fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encodes everything but the RFC 3986 unreserved characters.
pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || b"-._~".contains(&c) {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{:02X}", c));
        }
    }
    out
}

pub fn parse_query(q: &str) -> Query {
    let mut m = Query::new();
    for part in q.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        m.entry(url_decode(k)).or_insert_with(|| url_decode(v));
    }
    m
}

/// HTML text escaping.
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c if c.is_control() => {}
            c => o.push(c),
        }
    }
    o
}

/// A parsed request line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Query,
}

/// Parses the head of an HTTP/1.x request (request line only; headers are not needed).
pub fn parse_request(head: &str) -> Option<Request> {
    let line = head.lines().next()?;
    let mut it = line.split_whitespace();
    let method = it.next()?.to_string();
    let target = it.next()?;
    let version = it.next()?;
    if !version.starts_with("HTTP/1.") {
        return None;
    }
    let target = target
        .strip_prefix("http://")
        .map(|t| t.find('/').map(|i| &t[i..]).unwrap_or("/"))
        .unwrap_or(target);
    let (path, q) = target.split_once('?').unwrap_or((target, ""));
    let q = q.split('#').next().unwrap_or("");
    Some(Request {
        method,
        path: url_decode(path),
        query: parse_query(q),
    })
}

pub(crate) fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        415 => "Unsupported Media Type",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        405 => "Method Not Allowed",
        403 => "Forbidden",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    }
}

fn hosting_route(h: &Hosting, req: &Request) -> Option<crate::hosting::Reply> {
    crate::hosting::route(h, &req.path)
}

/// The account a clan page request names is the sender's. The 2009 page URLs carry the login name and no secret, so on a
/// public lobby a request counts only from the address that has this account logged in (a remote client keeps its
/// lobby connection open while the pages are up); anybody else gets no page. Loopback (the host's tools) and a
/// request that names nobody pass.
// DC-DEFAULT: LG-321 (proposed) clan pages of a public lobby only from the account's logged-in address
pub fn passport_allowed(st: &Core, q: &Query, peer: &str) -> rusqlite::Result<bool> {
    if !st.opts.public || crate::announce::is_loopback_peer(peer) {
        return Ok(true);
    }
    let raw = q
        .get("p")
        .or_else(|| q.get("strPassport"))
        .or_else(|| q.get("passport"))
        .map(String::as_str)
        .unwrap_or_default();
    let login = crate::auth::normalize_login(raw);
    if login.is_empty() {
        return Ok(true);
    }
    let Some(acc) = crate::db::account_by_login(&st.db, &login)? else {
        return Ok(true);
    };
    let ip = crate::limits::peer_ip(peer);
    Ok(st
        .sessions
        .values()
        .any(|s| s.account == Some(acc.user_no) && crate::limits::peer_ip(&s.peer) == ip))
}

/// The clan action paths: they change a clan with a GET (the 2009 page links), so another site must not send them.
fn is_action(path: &str) -> bool {
    matches!(
        path.trim_end_matches('/'),
        "/clan/create" | "/clan/join" | "/clan/leave" | "/clan/kick" | "/clan/notice"
    )
}

/// LG-416: true for a browser request that another site started: `Sec-Fetch-Site` other than `same-origin` / `none`,
/// or an `Origin` whose host is not the request's `Host`. The 2009 embedded browser and the DeCombat client send
/// neither header, and a link followed inside these pages is same-origin, so only a forged request is caught. The
/// account check of LG-321 trusts the player's address, which a page the player has open in a browser shares.
// DC-DEFAULT: LG-416 (proposed) clan actions refuse another site's browser requests
pub fn cross_site(head: &str) -> bool {
    use crate::betaweb::header;
    if let Some(s) = header(head, "sec-fetch-site") {
        if !s.eq_ignore_ascii_case("same-origin") && !s.eq_ignore_ascii_case("none") {
            return true;
        }
    }
    let Some(origin) = header(head, "origin") else {
        return false;
    };
    let host = header(head, "host").unwrap_or("").to_ascii_lowercase();
    let o = origin.to_ascii_lowercase();
    let o_host = o
        .strip_prefix("http://")
        .or_else(|| o.strip_prefix("https://"))
        .unwrap_or("");
    host.is_empty() || o_host != host
}

/// [`route_from`] behind the cross-site check of the action paths (LG-416).
pub fn route_checked(
    st: &mut Core,
    req: &Request,
    head: &str,
    peer: &str,
) -> rusqlite::Result<(u16, String)> {
    if is_action(&req.path) && cross_site(head) {
        st.note(&format!(
            "clan web {}: refused, the request comes from another site (from {})",
            req.path,
            crate::limits::peer_ip(peer)
        ));
        return Ok((
            403,
            page("Clans", Some("Open the clan pages from the game."), ""),
        ));
    }
    route_from(st, req, peer)
}

/// [`route`] of a request from `peer` (LG-321).
pub fn route_from(st: &mut Core, req: &Request, peer: &str) -> rusqlite::Result<(u16, String)> {
    if !passport_allowed(st, &req.query, peer)? {
        return Ok((403, page("Clans", Some("Log in to the game first."), "")));
    }
    route(st, req)
}

/// A complete HTTP/1.1 response (the connection closes after it).
pub fn response(status: u16, body: &str) -> Vec<u8> {
    let text = status_text(status);
    let mut v = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        status,
        text,
        body.len()
    )
    .into_bytes();
    v.extend_from_slice(body.as_bytes());
    v
}

// ---------------------------------------------------------------------------------------------------------------
// pages
// ---------------------------------------------------------------------------------------------------------------

/// The viewer of a page: the account named by the passport.
#[derive(Clone, Debug)]
pub struct Viewer {
    pub account: u32,
    pub passport: String,
    pub nick: String,
    pub clan: Option<clan::ClanRow>,
}

fn viewer(st: &Core, q: &Query) -> rusqlite::Result<Option<Viewer>> {
    let raw = q
        .get("p")
        .or_else(|| q.get("strPassport"))
        .or_else(|| q.get("passport"))
        .cloned()
        .unwrap_or_default();
    let login = crate::auth::normalize_login(&raw);
    if login.is_empty() {
        return Ok(None);
    }
    let Some(acc) = crate::db::account_by_login(&st.db, &login)? else {
        return Ok(None);
    };
    let nick = st
        .character(acc.user_no)
        .map(|c| c.nick)
        .unwrap_or_default();
    Ok(Some(Viewer {
        account: acc.user_no,
        passport: raw,
        nick,
        clan: clan::clan_of(st, acc.user_no)?,
    }))
}

pub fn page(title: &str, msg: Option<&str>, body: &str) -> String {
    let mut s = String::new();
    s.push_str("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>");
    s.push_str(&esc(title));
    s.push_str(
        "</title><style>body{background:#1b1d22;color:#ddd;font:13px sans-serif;margin:12px}a{color:#9cf}\
         table{border-collapse:collapse}td,th{border:1px solid #444;padding:2px 8px}</style></head><body>\n",
    );
    s.push_str(&format!("<h3>{}</h3>\n", esc(title)));
    if let Some(m) = msg {
        s.push_str(&format!("<p style=\"color:#fc0\">{}</p>\n", esc(m)));
    }
    s.push_str(body);
    s.push_str("</body></html>\n");
    s
}

fn link(path: &str, v: &Viewer, extra: &str, text: &str) -> String {
    format!(
        "<a href=\"{}?p={}{}\">{}</a>",
        path,
        url_encode(&v.passport),
        extra,
        esc(text)
    )
}

// DC-DEFAULT: LG-408 (proposed) a member's pages carry no create link (the 2009 clan screen hides its create button
// for a member, EVIDENCE: cshell+0x104960); clan::create refuses a member's create in any case
fn nav(v: &Viewer) -> String {
    let create = if v.clan.is_none() {
        format!(
            " | {}",
            link("/clan", v, "&amp;url=guild/create", "Create a clan")
        )
    } else {
        String::new()
    };
    format!(
        "<p>{} | {}{}</p>\n",
        link("/clan", v, "&amp;url=guild/list", "Find a clan"),
        link("/clan", v, "&amp;url=manager/guildmember", "Members"),
        create,
    )
}

fn whoami(st: &Core, v: &Viewer) -> String {
    match &v.clan {
        Some(k) => {
            let role = if k.master == v.account {
                "leader"
            } else {
                "member"
            };
            let n = clan::members(&st.db, k.uid).map(|m| m.len()).unwrap_or(0);
            format!(
                "<p>{}: you are in clan <b>{}</b> ({}, {} members).</p>\n",
                esc(&v.nick),
                esc(&k.name),
                role,
                n
            )
        }
        None => format!("<p>{}: you are not in a clan.</p>\n", esc(&v.nick)),
    }
}

fn list_page(st: &Core, v: &Viewer, msg: Option<&str>) -> rusqlite::Result<String> {
    let mut b = whoami(st, v);
    b.push_str(&nav(v));
    b.push_str("<table><tr><th>Clan</th><th>Members</th><th>Leader</th><th></th></tr>\n");
    for k in clan::all_clans(&st.db)? {
        let m = clan::members(&st.db, k.uid)?;
        let leader = m
            .iter()
            .find(|x| x.user_no == k.master)
            .map(|x| x.nick.clone())
            .unwrap_or_default();
        let join = if v.clan.is_none() {
            link("/clan/join", v, &format!("&amp;c={}", k.uid), "[Join]")
        } else {
            String::new()
        };
        b.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            esc(&k.name),
            m.len(),
            esc(&leader),
            join
        ));
    }
    b.push_str("</table>\n");
    Ok(page("Clans", msg, &b))
}

fn members_page(st: &Core, v: &Viewer, msg: Option<&str>) -> rusqlite::Result<String> {
    let mut b = whoami(st, v);
    b.push_str(&nav(v));
    let Some(k) = &v.clan else {
        return Ok(page("Clan members", msg, &b));
    };
    let leader = k.master == v.account;
    b.push_str("<table><tr><th>Role</th><th>Name</th><th>Exp</th><th>Online</th><th></th></tr>\n");
    for m in clan::member_rows(st, k.uid)? {
        let role = if m.char_no == k.master {
            "Leader"
        } else {
            "Member"
        };
        let kick = if leader && m.char_no != v.account {
            link(
                "/clan/kick",
                v,
                &format!("&amp;m={}", m.char_no),
                "[Remove]",
            )
        } else {
            String::new()
        };
        b.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            role,
            esc(&m.name),
            m.exp,
            if m.online != 0 { "yes" } else { "no" },
            kick
        ));
    }
    b.push_str("</table>\n");
    b.push_str(&format!("<p>Notice: {}</p>\n", esc(&k.notice)));
    if leader {
        b.push_str(&format!(
            "<form action=\"/clan/notice\" method=\"get\"><input type=\"hidden\" name=\"p\" value=\"{}\">\
             <input name=\"text\" maxlength=\"120\" value=\"{}\"> <input type=\"submit\" value=\"Set notice\"></form>\n",
            esc(&v.passport),
            esc(&k.notice)
        ));
    }
    b.push_str(&format!(
        "<p>{}</p>\n",
        link("/clan", v, "&amp;url=guild/leave", "Leave the clan")
    ));
    Ok(page("Clan members", msg, &b))
}

fn create_page(st: &Core, v: &Viewer, msg: Option<&str>) -> String {
    let mut b = whoami(st, v);
    b.push_str(&nav(v));
    if v.clan.is_none() {
        b.push_str(&format!(
            "<form action=\"/clan/create\" method=\"get\"><input type=\"hidden\" name=\"p\" value=\"{}\">\
             Name <input name=\"name\" maxlength=\"16\"> Emblem <input name=\"emblem\" size=\"3\" value=\"0\"> \
             Background <input name=\"bg\" size=\"3\" value=\"0\"> <input type=\"submit\" value=\"Create\"></form>\n",
            esc(&v.passport)
        ));
    }
    page("Create a clan", msg, &b)
}

fn leave_page(st: &Core, v: &Viewer, msg: Option<&str>) -> String {
    let mut b = whoami(st, v);
    b.push_str(&nav(v));
    if v.clan.is_some() {
        b.push_str(&format!(
            "<p>{}</p>\n",
            link("/clan/leave", v, "", "Leave the clan now")
        ));
    }
    page("Leave the clan", msg, &b)
}

fn home_page(st: &Core, v: &Viewer, msg: Option<&str>) -> rusqlite::Result<String> {
    match v.clan {
        Some(_) => members_page(st, v, msg),
        None => Ok(create_page(st, v, msg)),
    }
}

fn need_session(st: &Core, v: &Viewer) -> Option<Refusal> {
    (st.rules.clan.web_requires_session && st.sids_of(v.account).is_empty())
        .then_some(Refusal::NotOnline)
}

fn num(q: &Query, k: &str) -> i64 {
    q.get(k)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

fn refused(what: &str, r: &Refusal) -> String {
    format!("Cannot {}: {}.", what, r.text())
}

/// One field of a text line: tabs and line breaks become spaces.
fn field(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// The `fmt=text` answer: the viewer's clan, the action's message and (with `list`) the clan rows.
// DC-DEFAULT: CLI-790 (proposed) the clan pages' text form for the DeCombat client's own page drawing
fn text_reply(
    st: &Core,
    v: &Viewer,
    msg: Option<(bool, &str)>,
    list: bool,
    members: bool,
    search: &str,
) -> rusqlite::Result<String> {
    let mut b = String::new();
    let (uid, lead) = match &v.clan {
        Some(k) => (k.uid, (k.master == v.account) as u8),
        None => (0, 0),
    };
    b.push_str(&format!("me\t{}\t{}\n", uid, lead));
    if let Some((ok, m)) = msg {
        b.push_str(&format!("msg\t{}\t{}\n", ok as u8, field(m)));
    }
    if let (true, Some(k)) = (members, &v.clan) {
        for m in clan::member_rows(st, k.uid)? {
            b.push_str(&format!(
                "member\t{}\t{}\t{}\t{}\n",
                m.char_no,
                (m.char_no == k.master) as u8,
                field(&m.name),
                m.exp
            ));
        }
    }
    if list {
        // LG-414: `q` keeps the clans whose name holds it (any case); without it every clan
        let q = search.trim().to_lowercase();
        for k in clan::all_clans(&st.db)? {
            if !q.is_empty() && !k.name.to_lowercase().contains(&q) {
                continue;
            }
            let m = clan::members(&st.db, k.uid)?;
            let leader = m
                .iter()
                .find(|x| x.user_no == k.master)
                .map(|x| x.nick.clone())
                .unwrap_or_default();
            b.push_str(&format!(
                "clan\t{}\t{}\t{}\t{}\t{}\t{}\n",
                k.uid,
                field(&k.name),
                m.len(),
                field(&leader),
                k.emblem,
                k.background
            ));
        }
    }
    Ok(b)
}

/// Routes one request inside the caller's scope; returns (status, HTML or the `fmt=text` lines).
pub fn route(st: &mut Core, req: &Request) -> rusqlite::Result<(u16, String)> {
    if req.method != "GET" && req.method != "HEAD" {
        return Ok((405, page("Clans", Some("Only GET is served."), "")));
    }
    let path = req.path.trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };
    let known = matches!(
        path,
        "/" | "/clan"
            | "/clan/create"
            | "/clan/join"
            | "/clan/leave"
            | "/clan/kick"
            | "/clan/notice"
    );
    if !known {
        return Ok((404, page("Clans", Some("No such page."), "")));
    }
    let Some(v) = viewer(st, &req.query)? else {
        // LG-414: the text form answers an unknown account in its own lines (the client shows the line; an HTML
        // page in its place showed an empty list)
        if req.query.get("fmt").is_some_and(|f| f == "text") {
            return Ok((
                200,
                "me\t0\t0\nmsg\t0\tCannot open the clan pages: unknown account.\n".to_string(),
            ));
        }
        return Ok((
            200,
            page(
                "Clans",
                Some("Cannot open the clan pages: unknown account."),
                "",
            ),
        ));
    };
    let q = &req.query;
    if q.get("fmt").is_some_and(|f| f == "text") {
        return route_text(st, path, v, q);
    }
    Ok(match path {
        "/clan/create" => {
            let r = match need_session(st, &v) {
                Some(r) => Err(r),
                None => clan::create(
                    st,
                    v.account,
                    q.get("name").map(|s| s.as_str()).unwrap_or(""),
                    num(q, "emblem"),
                    num(q, "bg"),
                )?,
            };
            let msg = match &r {
                Ok(k) => format!("Clan {} created.", k.name),
                Err(e) => refused("create the clan", e),
            };
            let v = viewer(st, q)?.unwrap_or(v);
            (200, home_page(st, &v, Some(&msg))?)
        }
        "/clan/join" => {
            let r = match need_session(st, &v) {
                Some(r) => Err(r),
                None => clan::join(st, v.account, num(q, "c"))?,
            };
            let msg = match &r {
                Ok(k) => format!("You joined {}.", k.name),
                Err(e) => refused("join", e),
            };
            let v = viewer(st, q)?.unwrap_or(v);
            (200, list_page(st, &v, Some(&msg))?)
        }
        "/clan/leave" => {
            let r = match need_session(st, &v) {
                Some(r) => Err(r),
                None => clan::leave(st, v.account)?,
            };
            let msg = match &r {
                Ok(l) => format!("You left {}.", l.clan().name),
                Err(e) => refused("leave", e),
            };
            let v = viewer(st, q)?.unwrap_or(v);
            (200, list_page(st, &v, Some(&msg))?)
        }
        "/clan/kick" => {
            let r = match need_session(st, &v) {
                Some(r) => Err(r),
                None => clan::kick(st, v.account, num(q, "m").clamp(0, u32::MAX as i64) as u32)?,
            };
            let msg = match &r {
                Ok((_, n)) => format!("{} was removed from the clan.", n),
                Err(e) => refused("remove the member", e),
            };
            let v = viewer(st, q)?.unwrap_or(v);
            (200, members_page(st, &v, Some(&msg))?)
        }
        "/clan/notice" => {
            let r = match need_session(st, &v) {
                Some(r) => Err(r),
                None => clan::set_notice(
                    st,
                    v.account,
                    q.get("text").map(|s| s.as_str()).unwrap_or(""),
                )?,
            };
            let msg = match &r {
                Ok(_) => "Notice saved.".to_string(),
                Err(e) => refused("set the notice", e),
            };
            let v = viewer(st, q)?.unwrap_or(v);
            (200, members_page(st, &v, Some(&msg))?)
        }
        _ => {
            let which = q
                .get("url")
                .map(|s| s.trim_matches('/').to_ascii_lowercase())
                .unwrap_or_default();
            let html = match which.as_str() {
                "guild/list" => list_page(st, &v, None)?,
                "manager/guildmember" | "guild/member" | "guild/members" => {
                    members_page(st, &v, None)?
                }
                "guild/create" => create_page(st, &v, None),
                "guild/leave" => leave_page(st, &v, None),
                _ => home_page(st, &v, None)?,
            };
            (200, html)
        }
    })
}

/// The `fmt=text` form of [`route`] (same actions, same refusals).
fn route_text(st: &mut Core, path: &str, v: Viewer, q: &Query) -> rusqlite::Result<(u16, String)> {
    let mut list = false;
    let mut members = false;
    let msg = match path {
        "/clan/create" => {
            let r = match need_session(st, &v) {
                Some(r) => Err(r),
                None => clan::create(
                    st,
                    v.account,
                    q.get("name").map(|s| s.as_str()).unwrap_or(""),
                    num(q, "emblem"),
                    num(q, "bg"),
                )?,
            };
            Some(match &r {
                Ok(k) => (true, format!("Clan {} created.", k.name)),
                Err(e) => (false, refused("create the clan", e)),
            })
        }
        "/clan/join" => {
            list = true;
            let r = match need_session(st, &v) {
                Some(r) => Err(r),
                None => clan::join(st, v.account, num(q, "c"))?,
            };
            Some(match &r {
                Ok(k) => (true, format!("You joined {}.", k.name)),
                Err(e) => (false, refused("join", e)),
            })
        }
        "/clan/leave" => {
            list = true;
            let r = match need_session(st, &v) {
                Some(r) => Err(r),
                None => clan::leave(st, v.account)?,
            };
            Some(match &r {
                Ok(l) => (true, format!("You left {}.", l.clan().name)),
                Err(e) => (false, refused("leave", e)),
            })
        }
        "/clan/kick" => {
            members = true;
            let r = match need_session(st, &v) {
                Some(r) => Err(r),
                None => clan::kick(st, v.account, num(q, "m").clamp(0, u32::MAX as i64) as u32)?,
            };
            Some(match &r {
                Ok((_, n)) => (true, format!("{} was removed from the clan.", n)),
                Err(e) => (false, refused("remove the member", e)),
            })
        }
        "/clan/notice" => {
            let r = match need_session(st, &v) {
                Some(r) => Err(r),
                None => clan::set_notice(
                    st,
                    v.account,
                    q.get("text").map(|s| s.as_str()).unwrap_or(""),
                )?,
            };
            Some(match &r {
                Ok(_) => (true, "Notice saved.".to_string()),
                Err(e) => (false, refused("set the notice", e)),
            })
        }
        _ => {
            let url = q
                .get("url")
                .map(|u| u.trim_matches('/').to_ascii_lowercase())
                .unwrap_or_default();
            list = url == "guild/list";
            members = matches!(
                url.as_str(),
                "manager/guildmember" | "guild/member" | "guild/members"
            );
            None
        }
    };
    let v = viewer(st, q)?.unwrap_or(v);
    let body = text_reply(
        st,
        &v,
        msg.as_ref().map(|(ok, m)| (*ok, m.as_str())),
        list,
        members,
        q.get("q").map_or("", |s| s.as_str()),
    )?;
    Ok((200, body))
}

// ---------------------------------------------------------------------------------------------------------------
// server
// ---------------------------------------------------------------------------------------------------------------

/// Reads the request head; returns it with the body bytes that came in the same reads.
pub(crate) fn read_head_and_rest<S: Read>(s: &mut S) -> Option<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if let Some(i) = buf.windows(2).position(|w| w == b"\n\n") {
            break i + 2;
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break buf.len(),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    if buf.is_empty() {
        return None;
    }
    let rest = buf.split_off(end);
    Some((String::from_utf8_lossy(&buf).into_owned(), rest))
}

/// Reads a request body of `Content-Length` bytes (none without one); `Err(413)` above `cap`, `Err(400)` when the
/// connection ends early.
pub(crate) fn read_body<S: Read>(
    s: &mut S,
    head: &str,
    mut got: Vec<u8>,
    cap: usize,
) -> Result<Vec<u8>, u16> {
    let len = head
        .lines()
        .skip(1)
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    if len > cap {
        return Err(413);
    }
    let mut chunk = [0u8; 2048];
    while got.len() < len {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(400),
            Ok(n) => got.extend_from_slice(&chunk[..n]),
        }
    }
    got.truncate(len);
    Ok(got)
}

/// A response with a binary body and its own content type (mod packages, the server-list document).
pub fn response_bytes(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut v = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        status,
        status_text(status),
        content_type,
        body.len()
    )
    .into_bytes();
    v.extend_from_slice(body);
    v
}

fn serve(
    shared: Shared,
    hosting: Arc<Hosting>,
    s: TcpStream,
    gate: Option<Arc<crate::tls::Gate>>,
    slot: Option<crate::tls::Slot>,
) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut s: crate::tls::Stream = match gate {
        None => crate::tls::Stream::Plain(s),
        Some(g) => match crate::tls::admit(s, &g, slot) {
            crate::tls::Admitted::Conn(c) => c.into(),
            crate::tls::Admitted::PlainRefused(s) => return redirect_to_tls(s),
            crate::tls::Admitted::Failed(_) => return,
        },
    };
    let _ = s.tcp().set_read_timeout(Some(Duration::from_secs(5)));
    let Some((head, rest)) = read_head_and_rest(&mut s) else {
        return;
    };
    let out = match parse_request(&head) {
        None => response(400, &page("Clans", Some("Bad request."), "")),
        // mod packages and the server list: no state lock, no account (SPEC: HOSTING_AND_MODS.md §4 items 3, 6)
        Some(req)
            if matches!(req.method.as_str(), "GET" | "HEAD")
                && hosting_route(&hosting, &req).is_some() =>
        {
            let r = hosting_route(&hosting, &req).expect("checked");
            let mut v = response_bytes(r.status, r.content_type, &r.body);
            if req.method == "HEAD" {
                if let Some(i) = v.windows(4).position(|w| w == b"\r\n\r\n") {
                    v.truncate(i + 4);
                }
            }
            v
        }
        // LG-360: a diagnostic report upload (1 MiB cap of its own, JSON answers)
        Some(req) if crate::diag::handles(&req.path) => match crate::diag::gate(&req.method) {
            Some(out) => out,
            None => match read_body(&mut s, &head, rest, crate::diag::MAX_BODY) {
                Ok(b) => {
                    let peer = s
                        .tcp()
                        .peer_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_default();
                    crate::diag::respond(&shared, &head, &b, &peer)
                }
                Err(code) => crate::diag::body_error(code),
            },
        },
        // LG-370: the beta signup endpoints (a small body cap of their own, JSON or HTML answers, CORS for the site)
        Some(req) if crate::betaweb::handles(&req.path) => {
            let body = if req.method == "POST" {
                match read_body(&mut s, &head, rest, crate::betaweb::MAX_BODY) {
                    Ok(b) => b,
                    Err(code) => {
                        let _ = s.write_all(&crate::betaweb::body_error(code));
                        s.close();
                        return;
                    }
                }
            } else {
                Vec::new()
            };
            let peer = s
                .tcp()
                .peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            crate::betaweb::respond(&shared, &req, &head, &body, &peer)
        }
        // the clan pages, and the registration page (LG-145: a capped form body, the source address for its rate
        // limit, the Host header for the success page)
        Some(req) => {
            let register = crate::register::handles(&req.path);
            // LG-241: the recovery forms share the registration page's body cap, JSON switch and rate limit
            let recover = crate::recover::handles(&req.path);
            // LG-68: the identity token endpoint (JSON in, JSON out)
            let ident = crate::identity::handles(&req.path);
            let form = register || recover;
            let body = if (form || ident) && req.method == "POST" {
                match read_body(
                    &mut s,
                    &head,
                    rest,
                    crate::identity::MAX_BODY.max(crate::register::MAX_BODY),
                ) {
                    Ok(b) => b,
                    Err(code) => {
                        let msg = if code == 413 {
                            "Request too large."
                        } else {
                            "Bad request."
                        };
                        let out = if ident {
                            // LG-68: the token endpoint answers in JSON, never the HTML page
                            response_bytes(
                                code,
                                "application/json",
                                format!("{{\"error\":\"{msg}\"}}").as_bytes(),
                            )
                        } else {
                            response(code, &page("Registration", Some(msg), ""))
                        };
                        let _ = s.write_all(&out);
                        s.close();
                        return;
                    }
                }
            } else {
                Vec::new()
            };
            let peer = s
                .tcp()
                .peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            let host = crate::register::host_of(&head);
            // LG-210: the game's registration dialog asks for JSON
            let json = ident || (form && crate::register::wants_json(&head, &body, &req.query));
            let mut g = crate::net::lock(&shared);
            let mut res = (500, String::new());
            // LG-68: the token endpoint hashes outside the lock: look up under it, verify without it, lock to issue
            let mut ident_done = false;
            if ident {
                let now = std::time::Instant::now();
                let mut step = None;
                let r1 = g.scope(|st| {
                    step = Some(crate::identity::begin(st, &req.method, &body, &peer, now)?);
                    Ok(())
                });
                match (r1, step) {
                    (Ok(()), Some(crate::identity::Step::Done(r))) => {
                        res = r;
                        ident_done = true;
                    }
                    (Ok(()), Some(crate::identity::Step::Verify(p))) => {
                        // DC-DEFAULT: LG-328 (proposed, project decision) on the password pool, its place counted
                        let keys = p.keys().clone();
                        if !g.limits.login.begin_attempt(&keys, now) {
                            res = (401, crate::identity::MSG_REFUSED.into());
                        } else {
                            drop(g);
                            let out = crate::pwpool::global().run(move || {
                                let c = p.check();
                                (p, c)
                            });
                            g = crate::net::lock(&shared);
                            g.limits.login.end_attempt(&keys);
                            let r2 = g.scope(|st| {
                                res = match out {
                                    Ok((p, check)) => crate::identity::finish(st, p, check, now)?,
                                    Err(_) => (503, "{\"error\":\"the server is busy\"}".into()),
                                };
                                Ok(())
                            });
                            if r2.is_err() {
                                res = (500, String::new());
                            }
                        }
                        ident_done = true;
                    }
                    _ => {
                        ident_done = true;
                    }
                }
            }
            // DC-DEFAULT: LG-328 (proposed, project decision) a registration's hash on the password pool, off the lock
            let mut reg_done = false;
            let mut reg_err = None;
            if register {
                reg_done = true;
                let mut step = None;
                reg_err = g
                    .scope(|st| {
                        step = Some(crate::register::begin(
                            st,
                            &req.method,
                            &body,
                            &peer,
                            host.as_deref(),
                            std::time::Instant::now(),
                            json,
                        )?);
                        Ok(())
                    })
                    .err();
                match step {
                    Some(crate::register::RegStep::Done(r)) => res = r,
                    Some(crate::register::RegStep::Hash(h)) => {
                        drop(g);
                        let phc = crate::pwpool::global().run(move || h.compute());
                        g = crate::net::lock(&shared);
                        match phc {
                            Err(_) => res = crate::register::busy(json),
                            Ok(phc) => {
                                reg_err = g
                                    .scope(|st| {
                                        res = crate::register::finish(
                                            st,
                                            &body,
                                            &peer,
                                            host.as_deref(),
                                            json,
                                            &phc,
                                        )?;
                                        Ok(())
                                    })
                                    .err();
                            }
                        }
                    }
                    None => {}
                }
            }
            let r = if let Some(e) = reg_err {
                Err(e)
            } else if ident_done || reg_done {
                Ok(())
            } else {
                g.scope(|st| {
                    res = if recover {
                        crate::recover::route(
                            st,
                            &req.method,
                            &req.path,
                            &body,
                            &peer,
                            std::time::Instant::now(),
                            json,
                        )?
                    } else {
                        route_checked(st, &req, &head, &peer)?
                    };
                    Ok(())
                })
            };
            if let Err(e) = r {
                g.note(&format!(
                    "clan web {}: error (rolled back): {}",
                    req.path, e
                ));
                res = (500, page("Clans", Some("Server error."), ""));
            }
            crate::net::run_deferred(&shared, &mut g);
            drop(g);
            let mut v = if json && res.0 != 500 {
                response_bytes(res.0, "application/json", res.1.as_bytes())
            } else {
                response(res.0, &res.1)
            };
            if req.method == "HEAD" {
                if let Some(i) = v.windows(4).position(|w| w == b"\r\n\r\n") {
                    v.truncate(i + 4);
                }
            }
            v
        }
    };
    let _ = s.write_all(&out);
    let _ = s.flush();
    s.close();
}

/// A plain request on a TLS-only web port (LG-224): GET / HEAD are sent to the same address over `https://`; other
/// methods (a form post) are refused without reading their body.
fn redirect_to_tls(mut s: TcpStream) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let Some((head, _)) = read_head_and_rest(&mut s) else {
        return;
    };
    // the Host header as sent (with its port), when it holds only host characters
    let host = head
        .lines()
        .skip(1)
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("host")
                .then(|| v.trim().to_string())
        })
        .filter(|h| {
            !h.is_empty()
                && h.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-.:[]".contains(c))
        })
        .or_else(|| s.local_addr().ok().map(|a| a.to_string()))
        .unwrap_or_default();
    let out = match parse_request(&head) {
        Some(req) if matches!(req.method.as_str(), "GET" | "HEAD") => {
            let target = head.split_whitespace().nth(1).unwrap_or("/");
            let target = if target.starts_with('/') { target } else { "/" };
            let url = format!("https://{host}{target}");
            format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: {url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .into_bytes()
        }
        _ => response(
            403,
            &page(
                "Encrypted connection required",
                Some("This server only takes forms over https://. Open the page again with https:// in front."),
                "",
            ),
        ),
    };
    let _ = s.write_all(&out);
    let _ = s.shutdown(std::net::Shutdown::Both);
}

/// Serves the clan pages on an already bound listener (main binds it so a failed bind stops the start-up).
/// Also serves `/mods/<sha256>.zip` and `/server_list.json` from `hosting`.
#[cfg(test)]
pub fn spawn_with(shared: Shared, l: TcpListener, hosting: Hosting) {
    spawn_with_tls(shared, l, hosting, None)
}

/// [`spawn_with`] behind the TLS gate of the web port (LG-223 / LG-224); None = plain only.
pub fn spawn_with_tls(
    shared: Shared,
    l: TcpListener,
    hosting: Hosting,
    gate: Option<Arc<crate::tls::Gate>>,
) {
    let hosting = Arc::new(hosting);
    let slots = crate::tls::Slots::default();
    let cap = crate::net::lock(&shared).limits.conns.clone();
    let mut refused_note = None;
    let _ = std::thread::Builder::new()
        .name("clanweb-accept".into())
        .spawn(move || {
            for s in l.incoming() {
                match s {
                    Ok(stream) => {
                        // LG-326: the place is held by this request's thread until it ends
                        let Some(place) = crate::net::admit_conn(
                            &shared,
                            &cap,
                            &stream,
                            "clan web",
                            &mut refused_note,
                        ) else {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                            continue;
                        };
                        // LG-223: the cap of handshakes in progress, as on the auth port
                        let slot = if gate.is_some() {
                            match slots.take(crate::tls::MAX_HANDSHAKES) {
                                Some(s) => Some(s),
                                None => {
                                    let _ = stream.shutdown(std::net::Shutdown::Both);
                                    if slots.first_of_burst() {
                                        crate::net::lock(&shared).note(
                                            "clan web accept: too many handshakes in progress, closing new connections at once (LG-223)",
                                        );
                                    }
                                    continue;
                                }
                            }
                        } else {
                            None
                        };
                        let sh = Arc::clone(&shared);
                        let hs = Arc::clone(&hosting);
                        let gate = gate.clone();
                        let _ = std::thread::Builder::new()
                            .name("clanweb".into())
                            .spawn(move || {
                                let _place = place;
                                serve(sh, hs, stream, gate, slot)
                            });
                    }
                    Err(e) => crate::net::accept_failed(&shared, "clan web", e),
                }
            }
        });
}

#[cfg(test)]
#[path = "clanweb_tests.rs"]
mod tests;
