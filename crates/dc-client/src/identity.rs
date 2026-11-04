//! The client side of the identity authority (CLI-1610, project decision: one identity, many ledgers). Our
//! own design, no 2009 counterpart.
//!
//! The player types name and password on the login screen as before. With the identity on, the client first asks the
//! authority (a lobby that issues tokens, its TLS web port) for a token: `POST /identity/token` with
//! `{"login","password"}` (dc-lobby-server LG-68). A 200 answer's token is kept in memory and in the profile folder
//! (`identity_token.json`) and the login request to the chosen lobby carries `tok:<token>` as its login name with an
//! empty password, so the password never reaches that lobby. A later start with an unexpired cached token logs in
//! without the password; a lobby refusing the token (wrong-password reply) drops the cache and the screen asks for the
//! password again.
//!
//! The authority: `--identity HOST:PORT` (its web port), `--identity-fingerprint SHA256` (its certificate), or by
//! default the lobby's host at the auth port + 8080 with the lobby's pinned fingerprint. `--identity off` keeps the
//! plain password login (a standalone local account). The default authority falls back to the plain password login
//! when it answers that it does not issue (404) or, on a loopback lobby, when it is not reachable (the password goes
//! to that same lobby in both cases); an authority named with `--identity` never falls back.
// DC-DEFAULT: CLI-1610 (proposed, project decision) the identity flow, the cache file, expiry and fallbacks

use dc_botclient::tls::{self, Trust};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// The cache file in the profile folder.
pub const CACHE_FILE: &str = "identity_token.json";
/// The issuing endpoint of the authority's web port (dc-lobby-server LG-68).
pub const TOKEN_PATH: &str = "/identity/token";
/// The login-name prefix of a token login (dc-lobby-server LG-68).
pub const LOGIN_PREFIX: &str = "tok:";
/// The web port of the default authority: the lobby's auth port + 8080 (the clan / registration port, CLI-790).
pub const WEB_PORT_OFFSET: u16 = 8080;
/// The 0x201 refusal a wrong password gets (and a refused token, LG-68).
// DC-DEFAULT: LG-68 (a refused token gets the wrong-password reply 0x00030025)
pub const REASON_WRONG_PASSWORD: u32 = 0x0003_0025;
/// Time for the connection and each read / write of the token request.
// DC-DEFAULT: CLI-1610 (proposed) 5 s
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// A cached token this close to its expiry counts as expired (the lobby allows 5 minutes of clock skew).
// DC-DEFAULT: CLI-1610 (proposed) 60 s
pub const EXPIRY_MARGIN: i64 = 60;
/// Largest answer read.
const MAX_ANSWER: u64 = 1 << 16;

/// Where the client asks for its token.
#[derive(Clone, Debug)]
pub struct Authority {
    /// The authority's web port.
    pub addr: SocketAddr,
    /// `host:port` as configured: the cache records it, so a token of another authority is not offered.
    pub key: String,
    /// How the authority's certificate is trusted.
    pub trust: Trust,
    /// True for the default authority (derived from the lobby address): it may fall back to the password login.
    pub derived: bool,
    /// The cache file; None = memory only.
    pub cache: Option<PathBuf>,
}

/// Picks the authority from the flags. `flag` is `--identity` (None: the default; `off`: no authority), `pin` the
/// `--identity-fingerprint`, `lobby` the auth address, `lobby_trust` the lobby's trust (its pin), `profile` the
/// profile folder of the cache.
pub fn resolve(
    flag: Option<&str>,
    pin: Option<&str>,
    lobby: SocketAddr,
    lobby_trust: &Trust,
    profile: Option<&Path>,
) -> Result<Option<Authority>, String> {
    let pin = match pin.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => Some(tls::normalize_fingerprint(p).map_err(|e| format!("--identity-fingerprint: {e}"))?),
        None => None,
    };
    let mut trust = lobby_trust.clone();
    let cache = profile.map(|d| d.join(CACHE_FILE));
    let (addr, key, derived) = match flag.map(str::trim) {
        Some(f) if f.eq_ignore_ascii_case("off") => return Ok(None),
        Some(f) if !f.is_empty() => {
            let addr = f
                .to_socket_addrs()
                .map_err(|e| format!("--identity {f}: {e}"))?
                .next()
                .ok_or_else(|| format!("--identity {f}: no address"))?;
            // CLI-1473: a typed host name is what a CA-issued certificate names
            if let Some((host, _)) = f.rsplit_once(':') {
                let host = host.trim_start_matches('[').trim_end_matches(']');
                if host.parse::<std::net::IpAddr>().is_err() && !host.is_empty() {
                    trust.names.insert(addr.ip(), host.to_string());
                }
            }
            (addr, f.to_string(), false)
        }
        _ => {
            let port = lobby
                .port()
                .checked_add(WEB_PORT_OFFSET)
                .ok_or("the lobby port leaves no room for the identity web port (+8080): use --identity")?;
            let addr = SocketAddr::new(lobby.ip(), port);
            (addr, addr.to_string(), true)
        }
    };
    // the lobby's pin serves an authority on the lobby's host (the same certificate on both ports)
    trust.pin = pin.or_else(|| (addr.ip() == lobby.ip()).then(|| lobby_trust.pin.clone()).flatten());
    Ok(Some(Authority {
        addr,
        key,
        trust,
        derived,
        cache,
    }))
}

// ------------------------------------------------------------------------------------------------ cache

/// A token the authority issued. Its `Debug` form never shows the token.
#[derive(Clone, PartialEq, Eq)]
pub struct Cached {
    pub token: String,
    /// Unix seconds.
    pub expires: i64,
    pub account: u32,
    pub nick: String,
    /// The login name typed (the cache is offered for it).
    pub login: String,
    /// [`Authority::key`] of the issuer.
    pub authority: String,
}

impl std::fmt::Debug for Cached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Cached({}, account {}, expires {}, at {}, token hidden)",
            self.login, self.account, self.expires, self.authority
        )
    }
}

impl Cached {
    /// True while the token can still be used at `now` (unix seconds).
    pub fn usable(&self, now: i64) -> bool {
        self.expires > now + EXPIRY_MARGIN && !self.token.is_empty()
    }

    /// True when this token was issued by `a` for `login`.
    pub fn fits(&self, a: &Authority, login: &str) -> bool {
        self.authority == a.key && self.login.eq_ignore_ascii_case(login.trim())
    }

    fn to_json(&self) -> String {
        serde_json::json!({
            "token": self.token,
            "expires": self.expires,
            "account": self.account,
            "nick": self.nick,
            "login": self.login,
            "authority": self.authority,
        })
        .to_string()
    }

    fn from_json(text: &str) -> Option<Cached> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        Some(Cached {
            token: v.get("token")?.as_str()?.to_string(),
            expires: v.get("expires")?.as_i64()?,
            account: u32::try_from(v.get("account")?.as_u64()?).ok()?,
            nick: v.get("nick").and_then(|n| n.as_str()).unwrap_or_default().to_string(),
            login: v.get("login")?.as_str()?.to_string(),
            authority: v.get("authority")?.as_str()?.to_string(),
        })
    }
}

/// Unix seconds now.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Writes the cache (a temporary file renamed over the old one; owner-only on Unix).
pub fn store(path: &Path, c: &Cached) -> io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(c.to_json().as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// The cached token at `now`: None when the file is missing, unreadable or the token expired.
pub fn load(path: &Path, now: i64) -> Option<Cached> {
    let text = std::fs::read_to_string(path).ok()?;
    Cached::from_json(&text).filter(|c| c.usable(now))
}

/// Removes the cache.
pub fn clear(path: &Path) {
    let _ = std::fs::remove_file(path);
}

// ------------------------------------------------------------------------------------------------ request

/// The authority's answer.
#[derive(Debug)]
pub enum Issued {
    Token(Cached),
    /// 401: wrong login or password (or a locked / disabled account).
    Refused,
    /// 404: this lobby does not issue tokens.
    NotIssuing,
    /// No answer, a trust refusal (the text is the TLS module's) or an answer that is not the endpoint's.
    Unreachable(String),
}

/// Asks the authority for a token. The password goes only into the TLS request body.
pub fn request_token(a: &Authority, login: &str, password: &str) -> Issued {
    let body = serde_json::json!({"login": login, "password": password}).to_string();
    let (status, answer) = match post_json(a.addr, &a.key, &a.trust, TOKEN_PATH, &body) {
        Ok(r) => r,
        Err(e) => return Issued::Unreachable(e),
    };
    match status {
        200 => {
            let v: serde_json::Value = match serde_json::from_str(&answer) {
                Ok(v) => v,
                Err(_) => return Issued::Unreachable(format!("{}: not a token answer", a.key)),
            };
            let token = v.get("token").and_then(|t| t.as_str()).unwrap_or_default();
            let expires = v.get("expires").and_then(|t| t.as_i64());
            match expires {
                Some(expires) if !token.is_empty() => Issued::Token(Cached {
                    token: token.to_string(),
                    expires,
                    account: v
                        .get("account")
                        .and_then(|n| n.as_u64())
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(0),
                    nick: v.get("nick").and_then(|n| n.as_str()).unwrap_or_default().to_string(),
                    login: login.trim().to_string(),
                    authority: a.key.clone(),
                }),
                _ => Issued::Unreachable(format!("{}: not a token answer", a.key)),
            }
        }
        401 => Issued::Refused,
        404 => Issued::NotIssuing,
        s => Issued::Unreachable(format!("{}: answer {s}", a.key)),
    }
}

/// A minimal HTTP/1.1 POST of a JSON body over [`tls::connect_with`] (plain only to a loopback server without TLS):
/// (status, body). Shared helper candidate (the coordinator may fold it with the other web requests of the client).
pub fn post_json(
    addr: SocketAddr,
    host: &str,
    trust: &Trust,
    path: &str,
    body: &str,
) -> Result<(u16, String), String> {
    let mut c = tls::connect_with(addr, TIMEOUT, trust)?;
    let _ = c.tcp().set_read_timeout(Some(TIMEOUT));
    let _ = c.tcp().set_write_timeout(Some(TIMEOUT));
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    c.write_all(req.as_bytes())
        .map_err(|e| format!("{host}: send: {e}"))?;
    let mut buf = Vec::new();
    let r = (&mut c).take(MAX_ANSWER).read_to_end(&mut buf);
    c.close();
    if buf.is_empty() {
        return Err(match r {
            Err(e) => format!("{host}: receive: {e}"),
            Ok(_) => format!("{host}: no answer"),
        });
    }
    let text = String::from_utf8_lossy(&buf);
    let (head, rest) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("{host}: not an HTTP answer"))?;
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("{host}: not an HTTP answer"))?;
    Ok((status, rest.to_string()))
}

// ------------------------------------------------------------------------------------------------ process-wide

fn global() -> &'static Mutex<Option<Authority>> {
    static G: OnceLock<Mutex<Option<Authority>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(None))
}

/// The client's authority for the account pages (registration, recovery): set once at start.
pub fn set_current(a: Option<Authority>) {
    *global().lock().unwrap_or_else(|e| e.into_inner()) = a;
}

/// The configured authority.
pub fn current() -> Option<Authority> {
    global().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The trust for a web request to `addr`: the authority's for its web port, else the process-wide one.
pub fn trust_for(addr: SocketAddr) -> Trust {
    match current() {
        Some(a) if a.addr == addr => a.trust,
        _ => tls::current(),
    }
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;
