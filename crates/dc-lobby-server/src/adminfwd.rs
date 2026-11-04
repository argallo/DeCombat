//! Further lobby instances on the admin web (`[admin_web] instances`, LG-399 / LG-400): the default lobby's admin web
//! serves another instance's dashboard and beta list under `/i/<name>/ops...`, behind its own sign-in, by forwarding the
//! request to that instance's loopback admin port (admin.rs, opsweb.rs) after its session, Origin and CSRF checks.
//!
//! - Only configured instances, only `/ops` and `/ops/*`, never the deploys (`/ops/updates*`: they are box-wide and
//!   stay on the default instance's own Updates tab).
//! - The forwarded request satisfies the target's own loopback guard itself: `Host` and `Origin` are the target's
//!   address, the admin token comes from the instance's env file, the page token is read from the target's `/ops` page
//!   just before each write. The browser sees none of them: the pages it gets carry the admin web session's token.
//! - Writes name who made them in `X-Admin-Via` (the key and the client address), which the target's account writes
//!   record in place of "the admin port" (playeradmin.rs).
//! - One request per connection, 3 s to connect, 20 s for the whole answer, answers up to 8 MiB, no redirects
//!   followed (a 1xx or 3xx answer is a 502).
// Provenance: ORIGINAL (DeCombat operator tooling)
// DC-DEFAULT: LG-399 instances and forwarding; LG-400 pages under the prefix, the switcher and the writer's name

use crate::clanweb::esc;
use dc_config::{AdminWebInstance, AdminWebSection};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// The path prefix of a further instance: `/i/<name>/ops...`.
pub const PREFIX: &str = "/i/";
/// The header that names the writer on the target (trusted there only behind its loopback guard).
pub const VIA: &str = "X-Admin-Via";
const CONNECT_WAIT: Duration = Duration::from_secs(3);
const ANSWER_DEADLINE: Duration = Duration::from_secs(20);
/// The largest answer taken from an instance.
const MAX_ANSWER: usize = 8 * 1024 * 1024;
/// The largest env file read for the token.
const MAX_ENV_FILE: u64 = 64 * 1024;
const META: &str = "<meta name=\"csrf\" content=\"";

/// An instance's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub status: u16,
    pub html: bool,
    pub body: String,
}

/// `/i/<name>/rest` -> (`name`, `/rest`, possibly empty); `None` outside the prefix.
pub fn split(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix(PREFIX)?;
    let (name, inner) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    (!name.is_empty()).then_some((name, inner))
}

/// True for the paths an instance serves through the admin web: `/ops` and below, not the deploys.
pub fn forwardable(inner: &str) -> bool {
    let p = inner.trim_end_matches('/');
    (p == "/ops" || p.starts_with("/ops/")) && !crate::deploy::handles(p)
}

/// The request target for the instance: the raw target of the request line without `/i/<name>`. The path must be
/// plain (no percent escapes, no `.` / `..` / empty segments, so it is the path the admin web checked); the query
/// passes as sent. `None` for anything else.
pub fn inner_target(raw: &str, name: &str) -> Option<String> {
    if !raw.bytes().all(|b| (0x21..0x7f).contains(&b) && b != b'#' && b != b'\\') {
        return None;
    }
    let rest = raw.strip_prefix(PREFIX)?.strip_prefix(name)?;
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    if !path.starts_with("/ops") || path.contains('%') {
        return None;
    }
    let segs: Vec<&str> = path[1..].split('/').collect();
    for (i, s) in segs.iter().enumerate() {
        if *s == "." || *s == ".." || (s.is_empty() && i + 1 != segs.len()) {
            return None;
        }
    }
    if !forwardable(path) {
        return None;
    }
    Some(match query {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    })
}

/// `DC_ADMIN_TOKEN` of a `KEY=VALUE` file (`export` and quotes allowed); `Ok(None)` when the file does not set it.
pub fn token_from_env_file(path: &str) -> Result<Option<String>, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("cannot read the instance's env_file: {e}"))?;
    let mut text = String::new();
    f.take(MAX_ENV_FILE)
        .read_to_string(&mut text)
        .map_err(|e| format!("cannot read the instance's env_file: {e}"))?;
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != "DC_ADMIN_TOKEN" {
            continue;
        }
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|x| x.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
            .unwrap_or(v);
        if v.bytes().any(|b| !(0x21..0x7f).contains(&b)) {
            return Err("the instance's DC_ADMIN_TOKEN holds spaces or control characters".into());
        }
        return Ok((!v.is_empty()).then(|| v.to_string()));
    }
    Ok(None)
}

/// The page token in a dashboard or beta page (`<meta name="csrf" content="...">`).
pub fn page_token(html: &str) -> Option<&str> {
    let i = html.find(META)? + META.len();
    let t = html[i..].split('"').next()?;
    let ok = (1..=128).contains(&t.len())
        && t.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'=' | b'+' | b'/'));
    ok.then_some(t)
}

/// A header value as sent: control characters dropped, at most 160 characters.
pub fn clean_via(v: &str) -> String {
    v.chars().filter(|c| !c.is_control()).take(160).collect::<String>().trim().to_string()
}

/// One request to the instance's admin port and its answer.
fn exchange(addr: SocketAddr, head: &str, body: &[u8]) -> Result<Answer, (u16, String)> {
    let down = |e: std::io::Error| (502, format!("the instance's admin port {addr} does not answer: {e}"));
    let mut s = TcpStream::connect_timeout(&addr, CONNECT_WAIT).map_err(down)?;
    let until = Instant::now() + ANSWER_DEADLINE;
    s.set_write_timeout(Some(ANSWER_DEADLINE)).map_err(down)?;
    s.write_all(head.as_bytes()).map_err(down)?;
    s.write_all(body).map_err(down)?;
    let _ = s.flush();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err((504, format!("the instance at {addr} took too long to answer")));
        }
        s.set_read_timeout(Some(left)).map_err(down)?;
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_ANSWER {
                    return Err((502, "the instance's answer is too large".into()));
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err((504, format!("the instance at {addr} took too long to answer")));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(down(e)),
        }
    }
    parse_answer(&buf)
}

/// The status, kind and body of an HTTP answer; a redirect, an informational answer or another content type is a 502.
pub fn parse_answer(buf: &[u8]) -> Result<Answer, (u16, String)> {
    let bad = |m: &str| (502, format!("the instance's answer {m}"));
    let end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| bad("has no end of head"))?;
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| bad("head is not text"))?;
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|l| l.strip_prefix("HTTP/1."))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .filter(|c| (100..600).contains(c))
        .ok_or_else(|| bad("has no status"))?;
    if status < 200 || (300..400).contains(&status) {
        return Err(bad(&format!("is a {status} (redirects are not followed)")));
    }
    let mut ctype = String::new();
    let mut length: Option<usize> = None;
    for l in lines {
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        let k = k.trim();
        if k.eq_ignore_ascii_case("content-type") {
            ctype = v.trim().to_ascii_lowercase();
        } else if k.eq_ignore_ascii_case("content-length") {
            length = Some(v.trim().parse().map_err(|_| bad("has a bad length"))?);
        }
    }
    let mut body = &buf[end + 4..];
    if let Some(n) = length {
        if body.len() < n {
            return Err(bad("ended early"));
        }
        body = &body[..n];
    }
    let html = if ctype.starts_with("text/html") {
        true
    } else if ctype.starts_with("application/json") {
        false
    } else {
        return Err(bad("is neither HTML nor JSON"));
    };
    let body = String::from_utf8(body.to_vec()).map_err(|_| bad("is not UTF-8"))?;
    Ok(Answer { status, html, body })
}

/// The instance's loopback admin address; anything else is refused (the config check refuses it too).
fn address(inst: &AdminWebInstance) -> Result<SocketAddr, (u16, String)> {
    match inst.admin.trim().parse::<SocketAddr>() {
        Ok(a) if a.ip().is_loopback() => Ok(a),
        _ => Err((502, format!("instance {}: admin must be a loopback ADDR:PORT", inst.name))),
    }
}

fn request_head(addr: SocketAddr, method: &str, target: &str, token: Option<&str>) -> String {
    let mut h = format!(
        "{method} {target} HTTP/1.1\r\nHost: {addr}\r\nUser-Agent: dc-lobby-server admin web\r\nConnection: close\r\n"
    );
    if let Some(t) = token {
        h.push_str(&format!("X-Admin-Token: {t}\r\n"));
    }
    h
}

/// Forwards one request (`method` GET or POST, `target` from [`inner_target`]) to the instance. A write first reads the
/// target's page token from its `/ops` page and carries it, the target's own Origin and `via` (who writes).
pub fn forward(
    inst: &AdminWebInstance,
    method: &str,
    target: &str,
    body: &[u8],
    via: &str,
) -> Result<Answer, (u16, String)> {
    let addr = address(inst)?;
    let token = if inst.env_file.trim().is_empty() {
        None
    } else {
        token_from_env_file(inst.env_file.trim()).map_err(|e| (502, e))?
    };
    let token = token.as_deref();
    if method == "GET" {
        let mut h = request_head(addr, "GET", target, token);
        h.push_str("\r\n");
        return exchange(addr, &h, &[]);
    }
    if method != "POST" {
        return Err((405, "method not allowed".into()));
    }
    let mut h = request_head(addr, "GET", "/ops", token);
    h.push_str("\r\n");
    let page = exchange(addr, &h, &[])?;
    let csrf = match page_token(&page.body) {
        Some(t) if page.status == 200 && page.html => t.to_string(),
        _ => {
            return Err((
                502,
                format!("the instance's dashboard did not give its page token (answer {})", page.status),
            ))
        }
    };
    let mut h = request_head(addr, "POST", target, token);
    h.push_str(&format!(
        "Content-Type: application/json\r\nOrigin: http://{addr}\r\nX-Admin-CSRF: {csrf}\r\nX-Beta-CSRF: {csrf}\r\n\
         {VIA}: {}\r\nContent-Length: {}\r\n\r\n",
        clean_via(via),
        body.len()
    ));
    exchange(addr, &h, body)
}

/// A dashboard or beta page as the admin web serves it (`current`: the instance, `None` the default lobby): the page
/// token is the session's; with instances configured, the top bar names the lobby and switches between them; on a
/// further instance the Updates tab is gone; the dashboard links the key page.
pub fn dress(html: &str, cfg: &AdminWebSection, current: Option<&AdminWebInstance>, csrf: &str, beta: bool) -> String {
    let mut out = match page_token(html) {
        Some(t) => html.replacen(&format!("{META}{t}\""), &format!("{META}{csrf}\""), 1),
        None => html.to_string(),
    };
    if !beta {
        out = out.replacen(
            "id=\"beta-link\">Beta list</a>",
            "id=\"beta-link\">Beta list</a><a href=\"/keys\">Security keys</a>",
            1,
        );
    }
    if cfg.instances.is_empty() {
        return out;
    }
    let label = current.map_or(cfg.label.as_str(), |i| i.label.as_str());
    let page = if beta { "/ops/beta" } else { "/ops" };
    let link = |href: String, text: &str, on: bool| {
        format!(
            "<option value=\"{href}\"{}>{}</option>",
            if on { " selected" } else { "" },
            esc(text)
        )
    };
    let mut switch = link(page.to_string(), &cfg.label, current.is_none());
    for i in &cfg.instances {
        switch.push_str(&link(
            format!("{PREFIX}{}{page}", i.name),
            &i.label,
            current.is_some_and(|c| c.name == i.name),
        ));
    }
    out = out.replacen(
        "<div class=\"mark\">DeCombat <span>operations</span></div>",
        &format!(
            "<div class=\"mark\">DeCombat <span>operations</span></div><label class=\"inst\"><select data-inst \
             aria-label=\"Lobby\">{switch}</select></label>"
        ),
        1,
    );
    out = out.replacen("<title>DeCombat ", &format!("<title>{} | DeCombat ", esc(label)), 1);
    if current.is_some() {
        out = out.replacen("<body>", "<body class=\"other-lobby\">", 1);
        out = out.replacen("<a href=\"#updates\">Updates</a>", "", 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_split_and_only_plain_ops_paths_are_forwarded() {
        assert_eq!(split("/i/modded/ops/live"), Some(("modded", "/ops/live")));
        assert_eq!(split("/i/modded"), Some(("modded", "")));
        assert_eq!(split("/i/"), None);
        assert_eq!(split("/ops"), None);
        assert!(forwardable("/ops") && forwardable("/ops/") && forwardable("/ops/player/ban"));
        assert!(!forwardable("/ops/updates") && !forwardable("/ops/updates/deploy") && !forwardable("/opsx"));
        assert!(!forwardable("/status") && !forwardable(""));
        let t = |raw: &str| inner_target(raw, "modded");
        assert_eq!(t("/i/modded/ops/live").as_deref(), Some("/ops/live"));
        assert_eq!(t("/i/modded/ops/players?q=a%20b&limit=5").as_deref(), Some("/ops/players?q=a%20b&limit=5"));
        assert_eq!(t("/i/modded/ops/").as_deref(), Some("/ops/"));
        for bad in [
            "/i/modded/status",
            "/i/modded/ops/../status",
            "/i/modded/ops/./live",
            "/i/modded/ops//live",
            "/i/modded/ops/%2e%2e/status",
            "/i/modded/ops/updates/deploy",
            "/i/moddedx/ops/live",
            "/i/other/ops/live",
            "/i/modded/ops/live#x",
            "/i/modded/ops\\live",
        ] {
            assert_eq!(t(bad), None, "{bad}");
        }
    }

    #[test]
    fn answers_are_taken_only_as_json_or_html_and_never_as_redirects() {
        let a = parse_answer(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}junk")
            .unwrap();
        assert_eq!((a.status, a.html, a.body.as_str()), (200, false, "{}"));
        let a = parse_answer(b"HTTP/1.1 403 Forbidden\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>").unwrap();
        assert_eq!((a.status, a.html), (403, true));
        for bad in [
            &b"HTTP/1.1 302 Found\r\nLocation: http://192.0.2.1/\r\nContent-Type: text/html\r\n\r\n"[..],
            b"HTTP/1.1 101 Switching\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\n\r\nx",
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 9\r\n\r\n{}",
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json",
            b"nonsense\r\n\r\n",
        ] {
            assert_eq!(parse_answer(bad).unwrap_err().0, 502, "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn the_token_comes_from_the_env_file_and_the_page_token_from_the_meta() {
        let p = std::env::temp_dir().join(format!("dc_adminfwd_unit_{}", std::process::id()));
        let read = |text: &str| {
            std::fs::write(&p, text).unwrap();
            token_from_env_file(&p.to_string_lossy())
        };
        assert_eq!(read("DC_ADMIN=127.0.0.1:10091\nDC_ADMIN_TOKEN=abc\n").unwrap().as_deref(), Some("abc"));
        assert_eq!(read("export DC_ADMIN_TOKEN='q-1'\n").unwrap().as_deref(), Some("q-1"));
        assert_eq!(read("DC_ADMIN=127.0.0.1:10091\n").unwrap(), None);
        assert_eq!(read("DC_ADMIN_TOKEN=\n").unwrap(), None);
        assert!(read("DC_ADMIN_TOKEN=a b\n").is_err());
        let _ = std::fs::remove_file(&p);
        assert!(token_from_env_file(&p.to_string_lossy()).is_err());
        assert_eq!(page_token("<meta name=\"csrf\" content=\"Ab-_9\">"), Some("Ab-_9"));
        assert_eq!(page_token("<meta name=\"csrf\" content=\"\">"), None);
        assert_eq!(page_token("<meta name=\"csrf\" content=\"a<b\">"), None);
        assert_eq!(clean_via("key #1\r\nX-Evil: 1"), "key #1X-Evil: 1");
    }

    #[test]
    fn the_switcher_escapes_labels_and_marks_the_current_lobby() {
        let page = "<title>DeCombat ops</title><meta name=\"csrf\" content=\"old\"><body><div class=\"mark\">\
                    DeCombat <span>operations</span></div><nav><a href=\"#updates\">Updates</a>\
                    <a href=\"/ops/beta\" id=\"beta-link\">Beta list</a></nav>";
        let mut cfg = AdminWebSection::default();
        assert_eq!(
            dress(page, &cfg, None, "new", false),
            page.replace("content=\"old\"", "content=\"new\"").replace(
                "Beta list</a>",
                "Beta list</a><a href=\"/keys\">Security keys</a>"
            ),
            "without instances only the token and the key link change"
        );
        cfg.label = "Vanilla <b>".into();
        cfg.instances = vec![AdminWebInstance {
            name: "modded".into(),
            label: "Hacking & Mod".into(),
            admin: "127.0.0.1:10091".into(),
            env_file: String::new(),
        }];
        let other = dress(page, &cfg, Some(&cfg.instances[0]), "new", false);
        assert!(other.contains("DeCombat <span>operations</span>") && other.contains("<option value=\"/i/modded/ops\" selected>Hacking &amp; Mod</option>"));
        assert!(other.contains("<option value=\"/ops\">Vanilla &lt;b&gt;</option>"));
        assert!(other.contains("<title>Hacking &amp; Mod | DeCombat ops</title>"));
        assert!(other.contains("<body class=\"other-lobby\">") && !other.contains("#updates"));
        let beta = dress(page, &cfg, None, "new", true);
        assert!(beta.contains("<option value=\"/i/modded/ops/beta\">Hacking &amp; Mod</option>"));
        assert!(!beta.contains("Security keys") && beta.contains("#updates"));
    }
}
