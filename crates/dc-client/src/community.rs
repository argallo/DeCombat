//! The community directory in the client (CLI-1615, project decision): one static JSON file on the project site
//! (`community/community.json` in the repository), read over HTTPS from a fixed URL. The client takes only hashes
//! and badges from it (`dc_config::directory::badges`), never executable content. Our own design, no 2009
//! counterpart. The browser screen (`menu::modbrowser`, CLI-1616) reads it through [`load_view`].
// DC-DEFAULT: CLI-1615 (proposed) fixed URL, 5 s limit, 1 MiB cap

use dc_botclient::tls::{self, Trust};
use dc_config::directory::{load_community, validate_community, Community, KnownMod, ServerEntry};
use std::io::{Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// The default URL of the community file.
pub const DEFAULT_URL: &str = "https://decombat.com/community.json";
/// Time for the connection and each read / write.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// Largest file read.
const MAX_BODY: u64 = 1 << 20;

fn slot() -> &'static Mutex<String> {
    static G: OnceLock<Mutex<String>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(DEFAULT_URL.to_string()))
}

/// `--community URL`: sets the URL for this run; empty switches the community list off.
pub fn set_url(url: &str) {
    *slot().lock().unwrap_or_else(|e| e.into_inner()) = url.trim().to_string();
}

/// The configured URL, None when switched off (empty) or still the placeholder.
pub fn url() -> Option<String> {
    let u = slot().lock().unwrap_or_else(|e| e.into_inner()).clone();
    (!u.is_empty() && !u.contains('<')).then_some(u)
}

/// Splits `https://host[:port]/path` (`http://` only reaches a loopback test server) into (host, port, path).
fn split_url(url: &str) -> Result<(String, u16, String), String> {
    let (rest, port) = if let Some(r) = url.strip_prefix("https://") {
        (r, 443)
    } else if let Some(r) = url.strip_prefix("http://") {
        (r, 80)
    } else {
        return Err(format!("{url}: not an http(s) URL"));
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => {
            (h, p.parse::<u16>().map_err(|_| format!("{url}: bad port"))?)
        }
        _ => (authority, port),
    };
    if host.is_empty() {
        return Err(format!("{url}: no host"));
    }
    Ok((host.to_string(), port, path.to_string()))
}

/// A minimal HTTP/1.1 GET over [`tls::connect_with`] (CA-chain trust; plain only to a loopback server): the body of a 200.
pub fn http_get(url: &str) -> Result<String, String> {
    let (host, port, path) = split_url(url)?;
    let bare = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let addr = (bare.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("{host}: {e}"))?
        .next()
        .ok_or_else(|| format!("{host}: no address"))?;
    let mut trust = Trust::default();
    trust.names.insert(addr.ip(), bare);
    let mut c = tls::connect_with(addr, TIMEOUT, &trust)?;
    let _ = c.tcp().set_read_timeout(Some(TIMEOUT));
    let _ = c.tcp().set_write_timeout(Some(TIMEOUT));
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n"
    );
    c.write_all(req.as_bytes()).map_err(|e| format!("{host}: send: {e}"))?;
    let mut buf = Vec::new();
    let r = (&mut c).take(MAX_BODY).read_to_end(&mut buf);
    c.close();
    if buf.is_empty() {
        return Err(match r {
            Err(e) => format!("{host}: receive: {e}"),
            Ok(_) => format!("{host}: no answer"),
        });
    }
    parse_response(&host, &buf)
}

fn parse_response(host: &str, buf: &[u8]) -> Result<String, String> {
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| format!("{host}: not an HTTP answer"))?;
    let head = String::from_utf8_lossy(&buf[..split]).to_string();
    let body = &buf[split + 4..];
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("{host}: not an HTTP answer"))?;
    if status != 200 {
        return Err(format!("{host}: answer {status}"));
    }
    let chunked = head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    let bytes = if chunked {
        dechunk(body).ok_or_else(|| format!("{host}: bad chunked body"))?
    } else {
        body.to_vec()
    };
    String::from_utf8(bytes).map_err(|_| format!("{host}: body is not UTF-8"))
}

fn dechunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n")?;
        let line = std::str::from_utf8(&b[..eol]).ok()?;
        let size = usize::from_str_radix(line.split(';').next()?.trim(), 16).ok()?;
        b = &b[eol + 2..];
        if size == 0 {
            return Some(out);
        }
        if b.len() < size + 2 {
            return None;
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

/// Fetches, parses and validates the community file at `url`.
pub fn fetch_community(url: &str) -> Result<Community, String> {
    parse_community(&http_get(url)?)
}

/// Parses and validates the text of a community file (an invalid document is refused as a whole).
pub fn parse_community(text: &str) -> Result<Community, String> {
    let say = |i: &dc_config::Issue| format!("{}: {}", i.path, i.message);
    let doc = load_community(text).map_err(|e| e.first().map(say).unwrap_or_default())?;
    match validate_community(&doc).iter().find(|i| i.is_error()) {
        Some(i) => Err(say(i)),
        None => Ok(doc),
    }
}

/// CLI-1616: the cache file of the community directory in the profile folder, and how long a cached copy is used
/// without asking the network again.
// DC-DEFAULT: CLI-1616 (proposed) one hour; a stale copy is still shown when the network fails
pub const CACHE_FILE: &str = "community_cache.json";
pub const CACHE_FOR: Duration = Duration::from_secs(3600);

/// The community directory as the server browser shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommunityView {
    /// The document (`None`: switched off, or neither the network nor the cache had one).
    pub doc: Option<Community>,
    /// Each server's lobby address resolved (same order as `doc.servers`; `None`: the name did not resolve).
    pub addrs: Vec<Option<SocketAddr>>,
    /// The document came from the disk cache.
    pub cached: bool,
    /// Why the network copy is missing (the cache may still have served one).
    pub error: Option<String>,
}

/// Reads the directory for the browser: the cache when younger than [`CACHE_FOR`], else the network (written to
/// the cache), else a stale cache. Runs on a thread of its own (the link spawns it); `url = None`: switched off.
pub fn load_view(url: Option<&str>, cache: Option<&Path>) -> CommunityView {
    let Some(url) = url else {
        return CommunityView::default();
    };
    let read_cache = |fresh_only: bool| -> Option<Community> {
        let p = cache?;
        let age = std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .unwrap_or(Duration::MAX);
        if fresh_only && age > CACHE_FOR {
            return None;
        }
        parse_community(&std::fs::read_to_string(p).ok()?).ok()
    };
    let (doc, cached, error) = match read_cache(true) {
        Some(d) => (Some(d), true, None),
        None => match http_get(url).and_then(|t| parse_community(&t).map(|d| (d, t))) {
            Ok((d, text)) => {
                if let Some(p) = cache {
                    if let Some(dir) = p.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(p, text);
                }
                (Some(d), false, None)
            }
            Err(e) => {
                let stale = read_cache(false);
                let cached = stale.is_some();
                (stale, cached, Some(e))
            }
        },
    };
    let addrs = doc
        .as_ref()
        .map(|d| d.servers.iter().map(|s| resolve(&s.address)).collect())
        .unwrap_or_default();
    CommunityView {
        doc,
        addrs,
        cached,
        error,
    }
}

/// `host:port` -> the first address (IPv4 first).
pub fn resolve(address: &str) -> Option<SocketAddr> {
    let all: Vec<SocketAddr> = address.to_socket_addrs().ok()?.collect();
    all.iter().find(|a| a.is_ipv4()).or(all.first()).copied()
}

/// The pinned set of a directory entry ([`crate::menu::link::LinkCmd::ExpectModSet`]): the lobby's own set must have
/// the same policy and hash.
pub fn expected_set(s: &ServerEntry) -> crate::mods::ServerModSet {
    use crate::mods::{Policy, ServerMod, ServerModSet};
    if s.policy != "modded" {
        return ServerModSet::vanilla();
    }
    ServerModSet {
        policy: Policy::Modded {
            client_visual_allowed: s.client_visual_allowed,
        },
        community_map_base: None,
        client_fov: None,
        mods: s
            .mods
            .iter()
            .map(|m| ServerMod {
                id: m.id.clone(),
                version: m.version.clone(),
                sha256: m.sha256.clone(),
                kind: None,
                requires_client: true,
                capabilities: Vec::new(),
                source_repo: None,
                name: None,
                author: None,
                description: None,
                homepage: None,
                screenshots: Vec::new(),
                tags: Vec::new(),
                commands: Vec::new(),
                preview: None,
            })
            .collect(),
    }
}

/// The mod keys the directory verifies (its known-mods list), [`crate::mods::mod_key`] form.
pub fn verified_keys(doc: &Community) -> Vec<String> {
    doc.known_mods
        .iter()
        .map(|k| crate::mods::mod_key(&k.id, &k.version, &k.sha256))
        .collect()
}

/// The known-mods row of a mod key, when the directory lists it.
pub fn known<'a>(doc: &'a Community, key: &str) -> Option<&'a KnownMod> {
    doc.known_mods
        .iter()
        .find(|k| crate::mods::mod_key(&k.id, &k.version, &k.sha256) == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_file_parses_through_the_client_path() {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../community/community.json");
        let doc = parse_community(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(doc.servers.len(), 2);
        assert!(!doc.known_mods.is_empty());
    }

    #[test]
    fn urls_split_and_a_placeholder_is_off() {
        assert_eq!(
            split_url("https://a.b/community.json").unwrap(),
            ("a.b".to_string(), 443, "/community.json".to_string())
        );
        assert_eq!(
            split_url("http://127.0.0.1:8000").unwrap(),
            ("127.0.0.1".to_string(), 8000, "/".to_string())
        );
        assert!(split_url("ftp://x/y").is_err());
        set_url("https://<host>/c.json");
        assert_eq!(url(), None);
        set_url("");
        assert_eq!(url(), None);
        set_url("https://x.example/c.json");
        assert!(url().is_some());
        set_url(DEFAULT_URL);
    }

    #[test]
    fn responses_decode_plain_and_chunked() {
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(parse_response("h", ok).unwrap(), "{}");
        let ch = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n{\r\n1\r\n}\r\n0\r\n\r\n";
        assert_eq!(parse_response("h", ch).unwrap(), "{}");
        assert!(parse_response("h", b"HTTP/1.1 404 Not Found\r\n\r\n").is_err());
    }

    /// Reads the published file: needs the network, so by hand (`--ignored`).
    #[test]
    #[ignore]
    fn the_published_directory_loads() {
        let doc = fetch_community(DEFAULT_URL).expect("fetch");
        assert!(validate_community(&doc).is_empty());
        println!("{} servers, {} known mods", doc.servers.len(), doc.known_mods.len());
    }
}
