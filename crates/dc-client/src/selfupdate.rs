//! Getting a newer client (project decision, no 2009 counterpart): the download page in the player's browser, and
//! on Windows an "Update now" that swaps the game folder's files and starts the game again.
//!
//! 1. What is offered: a release build hears of a newer release from its lobby ([`crate::clientver`], LG-322 /
//!    LG-404); a friend pack (a development build) reads `latest.json` in the folder its `pack.json` names.
//! 2. The download: the lobby's direct zip link when it is an official release download ([`OFFICIAL_DOWNLOAD`]),
//!    else the newest GitHub release's `DeCombat-win-<tag>.zip`, over HTTPS to GitHub's hosts only, and installed
//!    only when the same release's `SHA256SUMS` lists its sum. A friend pack's zip is a local file.
//! 3. The zip is unpacked (every name checked, every entry's CRC) into `dc-update/new` of the game folder; a copy of
//!    this exe in `dc-update/` is started as the helper (`--finish-update`) and this client quits.
//! 4. The helper waits for the client to end, moves the staged files over the game folder (the replaced ones go to
//!    `dc-update/old` and come back on a failure), keeps the player's own files, and starts the game again.
//!
//! macOS: the zip goes to the Downloads folder and Finder shows it (no swap). Any failure ends in a box with the
//! reason and the download page.
// DC-DEFAULT: CLI-1863..CLI-1868 (proposed, USER REQUEST 2026-11-04), see DC_DEFAULTS.md

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The download page when the lobby names none.
pub const DEFAULT_PAGE: &str = "https://github.com/argallo/DeCombat/releases/latest";
/// The public releases of the game (GitHub's REST API, public documentation).
pub const RELEASES_API: &str = "https://api.github.com/repos/argallo/DeCombat/releases";
/// The only place an update zip and its `SHA256SUMS` are taken from: `<this><tag>/<file>` (CLI-1864, LG-404).
// DC-DEFAULT: CLI-1864 (security rule: a lobby cannot name the client's code), see DC_DEFAULTS.md
pub const OFFICIAL_DOWNLOAD: &str = "https://github.com/argallo/DeCombat/releases/download/";
/// The hosts an update may talk to (port 443): the release API, the release download, and the asset storage GitHub
/// redirects a download to (`release-assets.` today, `objects.` before 2025).
pub const UPDATE_HOSTS: [&str; 4] = [
    "api.github.com",
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
];
/// The folder of an update inside the game folder: `new/` (staged), `old/` (replaced), the helper, the result.
pub const UPDATE_DIR: &str = "dc-update";
pub const STAGING_DIR: &str = "new";
pub const BACKUP_DIR: &str = "old";
pub const RESULT_FILE: &str = "result.txt";
/// The friend pack's own file next to the exe (`tools/friend_pack_win.py`): `{"commit", "update_dir"}`.
pub const PACK_FILE: &str = "pack.json";
/// The friend packs' update folder's file: `{"commit", "zip"}`.
pub const LATEST_FILE: &str = "latest.json";
/// The game's exe in the zip and in the game folder.
pub const EXE: &str = if cfg!(windows) {
    "dc-client.exe"
} else {
    "dc-client"
};
const HELPER: &str = if cfg!(windows) {
    "helper.exe"
} else {
    "helper"
};

const MAX_ZIP: u64 = 512 * 1024 * 1024;
const MAX_UNPACKED: u64 = 1024 * 1024 * 1024;
const MAX_API: u64 = 4 * 1024 * 1024;
const MAX_SUMS: u64 = 256 * 1024;
const MAX_LOCAL_DOC: u64 = 64 * 1024;
const MAX_REDIRECTS: usize = 5;
const NET_TIMEOUT: Duration = Duration::from_secs(20);
const NET_TOTAL: Duration = Duration::from_secs(30 * 60);
/// How long the helper waits for the game to end.
const EXIT_WAIT: Duration = Duration::from_secs(120);

// ------------------------------------------------------------------------------------------------ addresses

/// An address the browser may be handed: `https://` (or `http://`), at most 300 characters of a plain set (no
/// spaces, quotes or shell characters: the address goes to the OS opener as one argument).
pub fn safe_url(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    rest.is_some_and(|r| !r.is_empty() && !r.starts_with('/'))
        && s.len() <= 300
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~:/?#@!$&+,;=%".contains(&b))
}

/// The lobby's `update_url` as a page to open: kept when [`safe_url`], `https://` put before a bare `host/path`,
/// else [`DEFAULT_PAGE`].
pub fn page_url(raw: &str) -> String {
    let raw = raw.trim();
    if safe_url(raw) {
        return raw.to_string();
    }
    let with = format!("https://{raw}");
    if !raw.is_empty() && !raw.contains("://") && safe_url(&with) {
        return with;
    }
    DEFAULT_PAGE.to_string()
}

/// A direct zip link: `https://` and [`safe_url`] only.
pub fn download_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    (raw.starts_with("https://") && safe_url(raw)).then(|| raw.to_string())
}

/// `(tag, file)` of an official release download `<OFFICIAL_DOWNLOAD><tag>/<file>`: no query, no further folder,
/// each part 1..=100 of `A-Z a-z 0-9 . _ + -` and not starting with a dot (so no `..`, no escapes).
pub fn official_release(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix(OFFICIAL_DOWNLOAD)?;
    let (tag, file) = rest.split_once('/')?;
    let part = |s: &str| {
        (1..=100).contains(&s.len())
            && !s.starts_with('.')
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
    };
    (part(tag) && part(file)).then_some((tag, file))
}

/// The lobby's direct link when it is an official release download; another one is dropped with a log line and the
/// update looks the release up on GitHub instead.
pub fn official_link(url: &str) -> Option<&str> {
    if official_release(url).is_some() {
        return Some(url);
    }
    eprintln!(
        "dc-client update: the lobby's download link {url} is not under {OFFICIAL_DOWNLOAD}; ignored, the GitHub release is looked up instead"
    );
    None
}

#[cfg(test)]
pub(crate) static OPENED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Opens `url` in the player's browser (Windows `rundll32 url.dll,FileProtocolHandler`, macOS `open`, else
/// `xdg-open`). Only a [`safe_url`] is handed over. Tests record the address instead.
pub fn open_url(url: &str) -> Result<(), String> {
    if !safe_url(url) {
        return Err(format!("not an address the browser is given: {url}"));
    }
    #[cfg(test)]
    {
        OPENED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(url.to_string());
        Ok(())
    }
    #[cfg(not(test))]
    {
        let mut c = if cfg!(windows) {
            let mut c = std::process::Command::new("rundll32");
            c.arg("url.dll,FileProtocolHandler");
            c
        } else if cfg!(target_os = "macos") {
            std::process::Command::new("open")
        } else {
            std::process::Command::new("xdg-open")
        };
        c.arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("cannot open the browser: {e}"))
    }
}

/// Shows `file` in the OS file browser (macOS Finder, Windows Explorer), selected where the OS can.
pub fn reveal(file: &Path) {
    let r = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg("-R")
            .arg(file)
            .spawn()
    } else if cfg!(windows) {
        let mut a = OsString::from("/select,");
        a.push(file);
        std::process::Command::new("explorer").arg(a).spawn()
    } else {
        std::process::Command::new("xdg-open")
            .arg(file.parent().unwrap_or(file))
            .spawn()
    };
    if let Err(e) = r {
        eprintln!("dc-client update: cannot show {}: {e}", file.display());
    }
}

// ------------------------------------------------------------------------------------------------ the offer

/// The lobby document's `download` map (LG-404): a direct zip per platform, kept only when [`official_release`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Downloads {
    pub windows: Option<String>,
    pub macos: Option<String>,
}

impl Downloads {
    pub fn from_doc(doc: &serde_json::Value) -> Downloads {
        let at = |k: &str| {
            doc.get("download")
                .and_then(|d| d.get(k))
                .and_then(|x| x.as_str())
                .and_then(download_url)
                .filter(|u| official_link(u).is_some())
        };
        Downloads {
            windows: at("windows"),
            macos: at("macos"),
        }
    }

    /// The link of this platform.
    pub fn for_this(&self) -> Option<String> {
        match platform() {
            Platform::Windows => self.windows.clone(),
            Platform::MacOs => self.macos.clone(),
            Platform::Other => None,
        }
    }
}

/// Where the new client comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A release newer than `have`: the lobby's direct link `zip`, else the newest release of the download page.
    Release { have: [u32; 3], zip: Option<String> },
    /// A friend pack's update folder: `zip` of build `commit`.
    Local { zip: PathBuf, commit: String },
}

/// A newer client the player is told about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub text: String,
    /// The lobby refuses this client (the box's last button is Close, not Later).
    pub too_old: bool,
    /// The download page (empty for a friend pack).
    pub page: String,
    pub source: Source,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

static OFFER: Mutex<Option<Offer>> = Mutex::new(None);

/// Leaves `o` for the menu's next free moment (replacing an older one).
pub fn post_offer(o: Offer) {
    *lock(&OFFER) = Some(o);
}

pub fn take_offer() -> Option<Offer> {
    lock(&OFFER).take()
}

pub fn has_offer() -> bool {
    lock(&OFFER).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Other,
}

pub fn platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Other
    }
}

/// What "Update now" can do with `o` here: a swap (Windows), a download to the Downloads folder (a macOS release),
/// or nothing (the page only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Swap,
    Download,
    PageOnly,
}

pub fn action_for(o: &Offer) -> Action {
    match (platform(), &o.source) {
        (Platform::Windows, _) => Action::Swap,
        (Platform::MacOs, Source::Release { .. }) => Action::Download,
        _ => Action::PageOnly,
    }
}

// ------------------------------------------------------------------------------------------------ checksums

pub fn sha256_hex(b: &[u8]) -> String {
    dc_rom::sha256(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// The sum `SHA256SUMS` lists for `name` (`<64 hex>  <name>` or `<64 hex> *<name>`, as `sha256sum` writes), lower case.
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hex, file) = l.trim_end().split_once(char::is_whitespace)?;
        let file = file.trim_start();
        let file = file.strip_prefix('*').unwrap_or(file);
        (file == name && hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hex.to_ascii_lowercase())
    })
}

/// The download checked against the release's sums: `Ok` only when they list this file's sum. No sums (missing,
/// unreadable), no line for the file or another sum all refuse the install (CLI-1864; no unverified install).
pub fn check_sum(sums: Option<&str>, name: &str, bytes: &[u8]) -> Result<(), String> {
    const NOT: &str = "it could not be verified";
    let Some(sums) = sums else {
        return Err(format!("{NOT} (the release has no SHA256SUMS)."));
    };
    match sum_for(sums, name) {
        Some(want) if want == sha256_hex(bytes) => Ok(()),
        Some(_) => Err(format!(
            "{NOT} ({name} does not match the release's SHA256SUMS)."
        )),
        None => Err(format!(
            "{NOT} (the release's SHA256SUMS does not list {name})."
        )),
    }
}

// ------------------------------------------------------------------------------------------------ GitHub releases

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Option<[u32; 3]>,
    pub zip: Asset,
    pub sums: Option<Asset>,
}

/// The game zip of `p` among a release's assets: `DeCombat-win-<tag>.zip` / `DeCombat-mac-<tag>.zip`, never the
/// bots or the MapKit zips.
pub fn is_game_zip(name: &str, p: Platform) -> bool {
    let prefix = match p {
        Platform::Windows => "DeCombat-win-",
        Platform::MacOs => "DeCombat-mac-",
        Platform::Other => return false,
    };
    name.starts_with(prefix)
        && name.ends_with(".zip")
        && !name.contains("-bots")
        && !name.contains("MapKit")
        && !name.contains("i686")
}

/// The release to take from GitHub's answer: `/releases/latest` (one object) or `/releases` (a list, newest first:
/// the first that is not a draft and has the game zip).
pub fn pick_release(doc: &serde_json::Value, p: Platform) -> Option<Release> {
    let one = |r: &serde_json::Value| -> Option<Release> {
        if r.get("draft").and_then(|d| d.as_bool()) == Some(true) {
            return None;
        }
        let tag = r.get("tag_name")?.as_str()?.to_string();
        let assets: Vec<Asset> = r
            .get("assets")?
            .as_array()?
            .iter()
            .filter_map(|a| {
                Some(Asset {
                    name: a.get("name")?.as_str()?.to_string(),
                    url: download_url(a.get("browser_download_url")?.as_str()?)
                        .filter(|u| official_release(u).is_some())?,
                })
            })
            .collect();
        let zip = assets.iter().find(|a| is_game_zip(&a.name, p))?.clone();
        Some(Release {
            version: crate::clientver::parse(tag.trim_start_matches('v')),
            tag,
            sums: assets.iter().find(|a| a.name == "SHA256SUMS").cloned(),
            zip,
        })
    };
    match doc {
        serde_json::Value::Array(list) => list.iter().find_map(one),
        r => one(r),
    }
}

// ------------------------------------------------------------------------------------------------ HTTPS

/// An `https://host[:port]/path` address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsUrl {
    pub host: String,
    pub port: u16,
    pub path: String,
}

pub fn parse_https(url: &str) -> Result<HttpsUrl, String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| format!("not an https:// address: {url}"))?;
    let (auth, path) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_string()),
        Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
        None => (rest, "/".to_string()),
    };
    if auth.contains('@') || auth.is_empty() || path.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err(format!("not an address the update takes: {url}"));
    }
    let (host, port) = match auth.rsplit_once(':') {
        Some((h, p)) if !h.contains(']') || h.ends_with(']') => (
            h,
            p.parse::<u16>().map_err(|_| format!("bad port in {url}"))?,
        ),
        _ => (auth, 443),
    };
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-.:".contains(&b))
    {
        return Err(format!("bad host in {url}"));
    }
    Ok(HttpsUrl { host, port, path })
}

/// One reply: status, the redirect target, the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub location: Option<String>,
    pub body: Vec<u8>,
}

fn deadline_passed(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("the download took too long".into())
    } else {
        Ok(())
    }
}

/// Reads one HTTP/1.1 reply: a body of `Content-Length` bytes, chunked, or up to the close; at most `cap` bytes. A
/// redirect's body is not read.
// Public format: RFC 9112 (HTTP/1.1 message framing, section 6 and 7.1 chunked transfer coding)
pub fn read_http(
    r: &mut dyn Read,
    cap: u64,
    deadline: Instant,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Reply, String> {
    let mut rd = BufReader::with_capacity(64 * 1024, r);
    let mut line = String::new();
    let mut head_bytes = 0usize;
    let mut next_line =
        |rd: &mut BufReader<&mut dyn Read>, line: &mut String| -> Result<(), String> {
            line.clear();
            deadline_passed(deadline)?;
            let n = rd
                .by_ref()
                .take(16 * 1024)
                .read_line(line)
                .map_err(|e| format!("read: {e}"))?;
            head_bytes += n;
            if n == 0 {
                return Err("the connection closed before the reply".into());
            }
            if head_bytes > 64 * 1024 {
                return Err("the reply head is too long".into());
            }
            Ok(())
        };
    next_line(&mut rd, &mut line)?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or("not an HTTP reply")?;
    let mut length: Option<u64> = None;
    let mut chunked = false;
    let mut location = None;
    loop {
        next_line(&mut rd, &mut line)?;
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k.eq_ignore_ascii_case("content-length") {
            length = Some(v.parse().map_err(|_| "bad Content-Length")?);
        } else if k.eq_ignore_ascii_case("transfer-encoding") {
            chunked = v.to_ascii_lowercase().contains("chunked");
        } else if k.eq_ignore_ascii_case("location") {
            location = Some(v.to_string());
        }
    }
    let mut body = Vec::new();
    if (300..400).contains(&status) {
        return Ok(Reply {
            status,
            location,
            body,
        });
    }
    let mut buf = vec![0u8; 64 * 1024];
    let mut take = |rd: &mut BufReader<&mut dyn Read>,
                    want: Option<u64>,
                    body: &mut Vec<u8>|
     -> Result<(), String> {
        let mut left = want;
        loop {
            if left == Some(0) {
                return Ok(());
            }
            deadline_passed(deadline)?;
            let n = left.map_or(buf.len(), |l| l.min(buf.len() as u64) as usize);
            let got = rd.read(&mut buf[..n]).map_err(|e| format!("read: {e}"))?;
            if got == 0 {
                return match left {
                    None => Ok(()),
                    Some(_) => Err("the connection closed in the middle of the download".into()),
                };
            }
            if body.len() as u64 + got as u64 > cap {
                return Err(format!("the download is larger than {cap} bytes"));
            }
            body.extend_from_slice(&buf[..got]);
            if let Some(l) = left.as_mut() {
                *l -= got as u64;
            }
            progress(body.len() as u64, length.unwrap_or(0));
        }
    };
    if chunked {
        loop {
            line.clear();
            deadline_passed(deadline)?;
            rd.by_ref()
                .take(1024)
                .read_line(&mut line)
                .map_err(|e| format!("read: {e}"))?;
            let size = u64::from_str_radix(line.trim().split(';').next().unwrap_or("").trim(), 16)
                .map_err(|_| "bad chunk in the reply")?;
            if size == 0 {
                // the trailer lines up to the empty one
                loop {
                    line.clear();
                    let n = rd
                        .by_ref()
                        .take(16 * 1024)
                        .read_line(&mut line)
                        .map_err(|e| format!("read: {e}"))?;
                    if n == 0 || line.trim_end().is_empty() {
                        break;
                    }
                }
                break;
            }
            if body.len() as u64 + size > cap {
                return Err(format!("the download is larger than {cap} bytes"));
            }
            take(&mut rd, Some(size), &mut body)?;
            let mut crlf = [0u8; 2];
            rd.read_exact(&mut crlf).map_err(|e| format!("read: {e}"))?;
            if &crlf != b"\r\n" {
                return Err("bad chunk end in the reply".into());
            }
        }
    } else {
        if length.is_some_and(|n| n > cap) {
            return Err(format!("the download is larger than {cap} bytes"));
        }
        take(&mut rd, length, &mut body)?;
    }
    Ok(Reply {
        status,
        location,
        body,
    })
}

/// A connection the update reads through.
pub trait Conn: Read + Write {}
impl<T: Read + Write> Conn for T {}

/// How [`get_with`] opens a connection to a host.
pub type Connector<'a> = dyn FnMut(&HttpsUrl) -> Result<Box<dyn Conn>, String> + 'a;

/// The target of a redirect from `from`: an absolute `https://` address or a path on the same host.
fn redirect(from: &HttpsUrl, loc: &str) -> Result<String, String> {
    if loc.starts_with("https://") {
        Ok(loc.to_string())
    } else if loc.starts_with('/') && !loc.starts_with("//") {
        let port = if from.port == 443 {
            String::new()
        } else {
            format!(":{}", from.port)
        };
        Ok(format!("https://{}{port}{loc}", from.host))
    } else {
        Err(format!(
            "the download was sent to an address that is not https://: {loc}"
        ))
    }
}

/// GET of `url` following at most [`MAX_REDIRECTS`] `https://` redirects, through `connect`.
pub fn get_with(
    url: &str,
    cap: u64,
    accept: &str,
    progress: &mut dyn FnMut(u64, u64),
    connect: &mut Connector<'_>,
) -> Result<Reply, String> {
    let deadline = Instant::now() + NET_TOTAL;
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let u = parse_https(&url)?;
        let mut c = connect(&u)?;
        let host = if u.port == 443 {
            u.host.clone()
        } else {
            format!("{}:{}", u.host, u.port)
        };
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: DeCombat-client-update\r\nAccept: {accept}\r\nConnection: close\r\n\r\n",
            u.path
        );
        c.write_all(req.as_bytes())
            .map_err(|e| format!("send to {}: {e}", u.host))?;
        let reply = read_http(&mut c, cap, deadline, progress)?;
        if matches!(reply.status, 301 | 302 | 303 | 307 | 308) {
            let loc = reply.location.ok_or("a redirect without a target")?;
            url = redirect(&u, &loc)?;
            continue;
        }
        return Ok(reply);
    }
    Err("the download was redirected too often".into())
}

/// [`get_with`] over the OS-trusted TLS connection (`dc_botclient::tls::connect_public`).
pub fn get(
    url: &str,
    cap: u64,
    accept: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Reply, String> {
    get_with(url, cap, accept, progress, &mut |u| {
        dc_botclient::tls::connect_public(&u.host, u.port, NET_TIMEOUT)
            .map(|c| Box::new(c) as Box<dyn Conn>)
    })
}

fn mods_safe(name: &str) -> bool {
    !name.contains('/') && crate::mods::safe_entry_name(name)
}

/// The `SHA256SUMS` of the same official release as `url` (`<OFFICIAL_DOWNLOAD><tag>/SHA256SUMS`).
pub fn sums_url_of(url: &str) -> Option<String> {
    official_release(url).map(|(tag, _)| format!("{OFFICIAL_DOWNLOAD}{tag}/SHA256SUMS"))
}

// ------------------------------------------------------------------------------------------------ friend packs

/// A friend pack's `pack.json`: its build and the folder its updates appear in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub commit: String,
    pub update_dir: Option<PathBuf>,
}

fn read_small(path: &Path, cap: u64) -> Result<Option<String>, String> {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut s = String::new();
    f.take(cap)
        .read_to_string(&mut s)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(s))
}

fn is_commit(s: &str) -> bool {
    (4..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `pack.json` of the folder `dir` (`None`: no pack file, or a broken one).
pub fn read_pack(dir: &Path) -> Option<Pack> {
    let text = read_small(&dir.join(PACK_FILE), MAX_LOCAL_DOC).ok()??;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let commit = v.get("commit")?.as_str()?.trim().to_ascii_lowercase();
    if !is_commit(&commit) {
        return None;
    }
    let update_dir = v
        .get("update_dir")
        .and_then(|d| d.as_str())
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from);
    Some(Pack { commit, update_dir })
}

#[cfg(test)]
pub(crate) static PACK_OVERRIDE: Mutex<Option<Option<Pack>>> = Mutex::new(None);

/// This install's pack file (the exe's folder), read once.
pub fn pack() -> Option<Pack> {
    #[cfg(test)]
    if let Some(p) = lock(&PACK_OVERRIDE).clone() {
        return p;
    }
    static P: std::sync::OnceLock<Option<Pack>> = std::sync::OnceLock::new();
    P.get_or_init(|| install_dir().and_then(|d| read_pack(&d)))
        .clone()
}

/// A friend pack that looks in a local folder for its updates (and so never in the lobby's).
pub fn local_updates() -> bool {
    pack().is_some_and(|p| p.update_dir.is_some())
}

fn same_commit(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    !a.is_empty() && !b.is_empty() && (a.starts_with(&b) || b.starts_with(&a))
}

fn short(c: &str) -> &str {
    &c[..c.len().min(8)]
}

/// What the pack's update folder offers: `None` when its `latest.json` names this build (or there is none).
pub fn local_offer(pack: &Pack) -> Result<Option<Offer>, String> {
    let Some(dir) = &pack.update_dir else {
        return Ok(None);
    };
    let Some(text) = read_small(&dir.join(LATEST_FILE), MAX_LOCAL_DOC)? else {
        return Ok(None);
    };
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{LATEST_FILE} in {}: {e}", dir.display()))?;
    let commit = v
        .get("commit")
        .and_then(|c| c.as_str())
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| is_commit(c))
        .ok_or_else(|| format!("{LATEST_FILE} in {} names no commit", dir.display()))?;
    if same_commit(&commit, &pack.commit) {
        return Ok(None);
    }
    let zip = v
        .get("zip")
        .and_then(|z| z.as_str())
        .filter(|z| mods_safe(z) && z.ends_with(".zip"))
        .ok_or_else(|| format!("{LATEST_FILE} in {} names no zip file", dir.display()))?;
    let zip = dir.join(zip);
    if !zip.is_file() {
        return Err(format!("{} is missing", zip.display()));
    }
    Ok(Some(Offer {
        text: format!(
            "A new test build ({}) is ready. This one is {}.",
            short(&commit),
            short(&pack.commit)
        ),
        too_old: false,
        page: String::new(),
        source: Source::Local { zip, commit },
    }))
}

// ------------------------------------------------------------------------------------------------ staging and swap

/// The game folder: the exe's own.
pub fn install_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// The folder takes new files (a test file made and removed).
pub fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".dc-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"x").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Unpacks a downloaded zip into `<install>/dc-update/new` (replacing an older staging). One folder that holds
/// every entry (`DeCombat-win-<tag>/`) is taken off; the result must hold [`EXE`] at its top.
pub fn stage(zip: &[u8], install: &Path) -> Result<PathBuf, String> {
    let files = crate::mods::read_zip(zip, MAX_UNPACKED, true)
        .map_err(|e| format!("the download is damaged: {e}"))?;
    let top = files
        .first()
        .and_then(|f| f.name.split_once('/'))
        .map(|(t, _)| t.to_string());
    let strip = top.filter(|t| files.iter().all(|f| f.name.starts_with(&format!("{t}/"))));
    let staging = install.join(UPDATE_DIR).join(STAGING_DIR);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let write = || -> Result<(), String> {
        for f in &files {
            let name = match &strip {
                Some(t) => &f.name[t.len() + 1..],
                None => f.name.as_str(),
            };
            let name = name.trim_end_matches('/');
            if name.is_empty() {
                continue;
            }
            let path = name.split('/').fold(staging.clone(), |p, c| p.join(c));
            if f.name.ends_with('/') {
                std::fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                continue;
            }
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            std::fs::write(&path, &f.data).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if !staging.join(EXE).is_file() {
            return Err(format!("the download holds no {EXE}"));
        }
        Ok(())
    };
    write().inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&staging);
    })?;
    Ok(staging)
}

/// A file of the player's (or of the game's own writing) that an update never replaces: the launcher with its
/// server preset (`DeCombat.bat`), settings, trust and log files. A new version of one goes next to it as
/// `<name>.new`.
pub fn players_file(rel: &Path) -> bool {
    let name = rel
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let ext = rel
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        name.as_str(),
        "decombat.bat" | "bots.bat" | "known_servers.txt" | "launcher-name.txt"
    ) || name.starts_with("settings")
        || matches!(
            ext.as_str(),
            "toml" | "ini" | "cfg" | "log" | "out" | "jsonl"
        )
}

fn files_under(root: &Path, at: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let rd = std::fs::read_dir(at).map_err(|e| format!("{}: {e}", at.display()))?;
    for e in rd {
        let e = e.map_err(|e| format!("{}: {e}", at.display()))?;
        let t = e
            .file_type()
            .map_err(|err| format!("{}: {err}", at.display()))?;
        let p = e.path();
        if t.is_dir() {
            files_under(root, &p, out)?;
        } else if t.is_file() {
            out.push(
                p.strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

/// What a swap did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Applied {
    /// Files put in place (new or replaced).
    pub placed: Vec<PathBuf>,
    /// The player's files left as they were (their new version written as `<name>.new` when it differs).
    pub kept: Vec<PathBuf>,
}

/// Moves a file, a few tries for a file the OS still holds (the exe a moment after its process ended).
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(_) if tries < 40 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) => return Err(e),
        }
    }
}

/// The swap: every file of `staging` goes to the same place in `install`; a file it replaces goes to
/// `dc-update/old` first, and on any failure every replaced file comes back. Nothing is deleted; the player's files
/// ([`players_file`]) stay; a folder that holds a 2009 client (`Engine.exe`) is never written into.
pub fn apply_staged(staging: &Path, install: &Path) -> Result<Applied, String> {
    let mut files = Vec::new();
    files_under(staging, staging, &mut files)?;
    files.sort();
    let backup = install.join(UPDATE_DIR).join(BACKUP_DIR);
    let _ = std::fs::remove_dir_all(&backup);
    let mut out = Applied::default();
    // (target, its backup when one was replaced)
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let step = |rel: &Path,
                out: &mut Applied,
                done: &mut Vec<(PathBuf, Option<PathBuf>)>|
     -> Result<(), String> {
        let first = rel.components().next().map(|c| install.join(c));
        if rel.components().count() > 1 && first.is_some_and(|f| f.join("Engine.exe").exists()) {
            out.kept.push(rel.to_path_buf());
            return Ok(());
        }
        let from = staging.join(rel);
        let target = install.join(rel);
        if target.exists() && players_file(rel) {
            let same = std::fs::read(&target).ok() == std::fs::read(&from).ok();
            if !same {
                let mut n = target.clone().into_os_string();
                n.push(".new");
                std::fs::copy(&from, PathBuf::from(n))
                    .map_err(|e| format!("{}: {e}", target.display()))?;
            }
            out.kept.push(rel.to_path_buf());
            return Ok(());
        }
        if let Some(d) = target.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let saved = if target.exists() {
            let b = backup.join(rel);
            if let Some(d) = b.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            move_file(&target, &b)
                .map_err(|e| format!("{} cannot be replaced: {e}", target.display()))?;
            Some(b)
        } else {
            None
        };
        done.push((target.clone(), saved));
        move_file(&from, &target)
            .or_else(|_| std::fs::copy(&from, &target).map(|_| ()))
            .map_err(|e| format!("{}: {e}", target.display()))?;
        out.placed.push(rel.to_path_buf());
        Ok(())
    };
    for rel in &files {
        if let Err(e) = step(rel, &mut out, &mut done) {
            for (target, saved) in done.iter().rev() {
                let _ = std::fs::remove_file(target);
                if let Some(b) = saved {
                    let _ = move_file(b, target);
                }
            }
            return Err(e);
        }
    }
    Ok(out)
}

/// Starts the helper (a copy of this exe in `dc-update/`) that finishes the update once this process has ended;
/// `relaunch` = the arguments to start the game again with (`None`: not started again).
pub fn start_helper(
    install: &Path,
    staging: &Path,
    relaunch: Option<Vec<OsString>>,
) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| format!("this program's file: {e}"))?;
    let helper = install.join(UPDATE_DIR).join(HELPER);
    std::fs::copy(&me, &helper).map_err(|e| format!("{}: {e}", helper.display()))?;
    let mut c = std::process::Command::new(&helper);
    c.arg("--finish-update")
        .arg(staging)
        .arg(install)
        .arg(std::process::id().to_string());
    if let Some(args) = relaunch {
        c.arg("--relaunch").args(args);
    }
    c.spawn()
        .map(|_| ())
        .map_err(|e| format!("the update helper does not start: {e}"))
}

/// Whether process `pid` still runs (`None`: cannot tell). Windows asks `tasklist` (its CSV row names the id in
/// quotes; the text without a row is localised, so only the row counts), elsewhere `kill -0`.
fn still_running(pid: u32) -> Option<bool> {
    if cfg!(windows) {
        let out = std::process::Command::new("tasklist")
            .args(["/FO", "CSV", "/NH", "/FI", &format!("PID eq {pid}")])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).contains(&format!("\",\"{pid}\",")))
    } else {
        let st = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        Some(st.success())
    }
}

/// Waits up to `limit` for process `pid` to end (`false`: it still runs). When the OS cannot be asked, a few
/// seconds pass instead (the swap's moves retry a file that is still held).
pub fn wait_for_exit(pid: u32, limit: Duration) -> bool {
    let end = Instant::now() + limit;
    loop {
        match still_running(pid) {
            Some(false) => return true,
            None => {
                std::thread::sleep(Duration::from_secs(3));
                return true;
            }
            Some(true) if Instant::now() >= end => return false,
            Some(true) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

fn write_result(install: &Path, text: &str) {
    let _ = std::fs::write(install.join(UPDATE_DIR).join(RESULT_FILE), text);
}

/// The helper's run (`--finish-update <staging> <install> <pid> [--relaunch <args>...]`): the process code.
pub fn finish_update(args: &[OsString]) -> i32 {
    let usage = "--finish-update <staging> <install> <pid> [--relaunch <args>...]";
    let (Some(staging), Some(install), Some(pid)) = (args.first(), args.get(1), args.get(2)) else {
        eprintln!("dc-client: {usage}");
        return 2;
    };
    let Some(pid) = pid.to_str().and_then(|p| p.parse::<u32>().ok()) else {
        eprintln!("dc-client: {usage}");
        return 2;
    };
    let (staging, install) = (PathBuf::from(staging), PathBuf::from(install));
    let relaunch: Option<Vec<OsString>> = match args.get(3) {
        Some(a) if a == "--relaunch" => Some(args[4..].to_vec()),
        _ => None,
    };
    // only a staging of this install's own update folder is applied
    if staging != install.join(UPDATE_DIR).join(STAGING_DIR) || !staging.join(EXE).is_file() {
        eprintln!(
            "dc-client update: {} is not a staged update of {}",
            staging.display(),
            install.display()
        );
        return 2;
    }
    let outcome = if wait_for_exit(pid, EXIT_WAIT) {
        apply_staged(&staging, &install)
    } else {
        Err("the game did not close".to_string())
    };
    let code = match &outcome {
        Ok(a) => {
            eprintln!(
                "dc-client update: {} files placed, {} of yours kept",
                a.placed.len(),
                a.kept.len()
            );
            write_result(&install, "ok");
            let _ = std::fs::remove_dir_all(&staging);
            0
        }
        Err(e) => {
            eprintln!("dc-client update: {e}");
            write_result(&install, &format!("failed: {e}"));
            1
        }
    };
    if let Some(args) = relaunch {
        if let Err(e) = std::process::Command::new(install.join(EXE))
            .args(args)
            .spawn()
        {
            eprintln!("dc-client update: the game does not start again: {e}");
        }
    }
    code
}

/// `--update-from <zip>`: the same swap from a zip on disk, the game not started again. The helper finishes once
/// this process has ended.
pub fn update_from(zip: &Path) -> Result<String, String> {
    let install = install_dir().ok_or("this program's folder is unknown")?;
    if !writable(&install) {
        return Err(format!("{} cannot be written", install.display()));
    }
    let bytes = read_capped(zip)?;
    let staging = stage(&bytes, &install)?;
    start_helper(&install, &staging, None)?;
    Ok(format!(
        "staged {} into {}; the helper swaps the files once this process ends (result in {})",
        zip.display(),
        staging.display(),
        install.join(UPDATE_DIR).join(RESULT_FILE).display()
    ))
}

fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut b = Vec::new();
    f.take(MAX_ZIP + 1)
        .read_to_end(&mut b)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if b.len() as u64 > MAX_ZIP {
        return Err(format!("{} is larger than {MAX_ZIP} bytes", path.display()));
    }
    Ok(b)
}

static LAST_RESULT: Mutex<Option<String>> = Mutex::new(None);

/// At the start of a normal run: the last update's failure (for one box) is kept and its folder removed (the helper
/// may still be ending; what cannot go now goes next time).
pub fn cleanup_leftovers() {
    let Some(install) = install_dir() else {
        return;
    };
    let dir = install.join(UPDATE_DIR);
    if !dir.is_dir() {
        return;
    }
    if let Ok(Some(r)) = read_small(&dir.join(RESULT_FILE), 4096) {
        if let Some(why) = r.strip_prefix("failed: ") {
            *lock(&LAST_RESULT) = Some(why.trim().to_string());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The last update's failure, once.
pub fn take_last_failure() -> Option<String> {
    lock(&LAST_RESULT).take()
}

// ------------------------------------------------------------------------------------------------ the worker

/// Where an update or a check stands (one at a time per process).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// Working: what, bytes done and in all (0: unknown).
    Busy {
        what: String,
        done: u64,
        total: u64,
    },
    /// Staged and the helper started: the client quits now.
    Staged,
    /// macOS: the zip is saved here.
    Saved(PathBuf),
    /// The update failed for this reason.
    Failed(String),
    /// A check by hand found nothing new (or could not look: the reason).
    Note(String),
}

static PHASE: Mutex<Phase> = Mutex::new(Phase::Idle);

pub fn phase() -> Phase {
    lock(&PHASE).clone()
}

#[cfg(test)]
pub(crate) fn tests_phase() -> std::sync::MutexGuard<'static, Phase> {
    lock(&PHASE)
}

fn set_phase(p: Phase) {
    *lock(&PHASE) = p;
}

/// The phase once it has ended (`Staged`, `Saved`, `Failed`, `Note`), which leaves `Idle`.
pub fn take_ended() -> Option<Phase> {
    let mut p = lock(&PHASE);
    match *p {
        Phase::Idle | Phase::Busy { .. } => None,
        _ => Some(std::mem::replace(&mut *p, Phase::Idle)),
    }
}

fn busy(what: &str) {
    set_phase(Phase::Busy {
        what: what.into(),
        done: 0,
        total: 0,
    });
}

/// Starts the update of `o` on a worker thread (`false`: one runs already).
pub fn start(o: Offer) -> bool {
    {
        let mut p = lock(&PHASE);
        if matches!(*p, Phase::Busy { .. }) {
            return false;
        }
        *p = Phase::Busy {
            what: "Preparing the update".into(),
            done: 0,
            total: 0,
        };
    }
    let spawned = std::thread::Builder::new()
        .name("dc-update".into())
        .spawn(move || {
            let r = run(&o);
            set_phase(match r {
                Ok(p) => p,
                Err(e) => Phase::Failed(e),
            });
        });
    if spawned.is_err() {
        set_phase(Phase::Failed("the update cannot start".into()));
    }
    true
}

/// The friend pack's check of its update folder on a worker thread; a newer build becomes an offer, a check by
/// hand that finds none says so.
pub fn start_check(by_hand: bool) -> bool {
    let Some(p) = pack() else {
        return false;
    };
    if p.update_dir.is_none() || matches!(phase(), Phase::Busy { .. }) {
        return false;
    }
    std::thread::Builder::new()
        .name("dc-update-check".into())
        .spawn(move || match local_offer(&p) {
            Ok(Some(o)) => post_offer(o),
            Ok(None) if by_hand => set_phase(Phase::Note(format!(
                "This is the newest test build ({}).",
                short(&p.commit)
            ))),
            Err(e) if by_hand => set_phase(Phase::Note(format!("No update could be read: {e}"))),
            Err(e) => eprintln!("dc-client update: {e}"),
            Ok(None) => {}
        })
        .is_ok()
}

fn progress_of(what: &'static str) -> impl FnMut(u64, u64) {
    let mut last = Instant::now() - Duration::from_secs(1);
    move |done, total| {
        if last.elapsed() >= Duration::from_millis(100) || (total > 0 && done >= total) {
            last = Instant::now();
            set_phase(Phase::Busy {
                what: what.into(),
                done,
                total,
            });
        }
    }
}

/// The zip of a release (file name, bytes), checked against the same release's SHA256SUMS.
fn fetch_release(have: [u32; 3], direct: Option<&str>) -> Result<(String, Vec<u8>), String> {
    fetch_release_with(
        have,
        direct,
        &mut |u| {
            dc_botclient::tls::connect_public(&u.host, u.port, NET_TIMEOUT)
                .map(|c| Box::new(c) as Box<dyn Conn>)
        },
        &mut busy,
        &mut progress_of("Downloading the update"),
    )
}

/// `Ok` for a request an update may make: one of [`UPDATE_HOSTS`] on port 443.
pub fn on_github(u: &HttpsUrl) -> Result<(), String> {
    if u.port == 443 && UPDATE_HOSTS.contains(&u.host.as_str()) {
        return Ok(());
    }
    Err(format!(
        "the download was sent to {}, which is not GitHub's release download",
        u.host
    ))
}

/// [`fetch_release`] through `connect`: every request (redirects too) goes only to [`UPDATE_HOSTS`] on port 443; a
/// `direct` link that is not an [`official_release`] is ignored (the GitHub release is looked up as if none was
/// named); the zip is returned only when the same release's SHA256SUMS lists its sum.
pub(crate) fn fetch_release_with(
    have: [u32; 3],
    direct: Option<&str>,
    connect: &mut Connector<'_>,
    stage: &mut dyn FnMut(&str),
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(String, Vec<u8>), String> {
    let mut github = |u: &HttpsUrl| -> Result<Box<dyn Conn>, String> {
        on_github(u)?;
        connect(u)
    };
    let zip_url = match direct.and_then(official_link) {
        Some(u) => u.to_string(),
        None => {
            stage("Looking for the new version");
            let json = "application/vnd.github+json";
            let mut r = get_with(
                &format!("{RELEASES_API}/latest"),
                MAX_API,
                json,
                &mut |_, _| {},
                &mut github,
            )?;
            if r.status == 404 {
                // only pre-releases so far: the newest of the list
                r = get_with(
                    &format!("{RELEASES_API}?per_page=10"),
                    MAX_API,
                    json,
                    &mut |_, _| {},
                    &mut github,
                )?;
            }
            if r.status != 200 {
                return Err(format!("the release list answered {}", r.status));
            }
            let doc: serde_json::Value =
                serde_json::from_slice(&r.body).map_err(|e| format!("the release list: {e}"))?;
            let rel = pick_release(&doc, platform())
                .ok_or("no release has a download for this computer")?;
            if rel.version.is_none_or(|v| v <= have) {
                return Err(format!(
                    "the newest download ({}) is not newer than this version",
                    rel.tag
                ));
            }
            rel.zip.url
        }
    };
    let (_, name) = official_release(&zip_url).ok_or("the release's download is not official")?;
    let sums_url = sums_url_of(&zip_url).ok_or("the release's download is not official")?;
    let r = get_with(
        &zip_url,
        MAX_ZIP,
        "application/octet-stream",
        progress,
        &mut github,
    )?;
    if r.status != 200 {
        return Err(format!("the download answered {}", r.status));
    }
    stage("Checking the download");
    let sums = get_with(&sums_url, MAX_SUMS, "*/*", &mut |_, _| {}, &mut github)
        .ok()
        .filter(|s| s.status == 200)
        .map(|s| String::from_utf8_lossy(&s.body).into_owned());
    check_sum(sums.as_deref(), name, &r.body)?;
    Ok((name.to_string(), r.body))
}

fn run(o: &Offer) -> Result<Phase, String> {
    let action = action_for(o);
    if action == Action::PageOnly {
        return Err("this computer cannot update by itself".into());
    }
    let install = install_dir().ok_or("this program's folder is unknown")?;
    if action == Action::Swap && !writable(&install) {
        return Err(format!(
            "the game folder {} cannot be written",
            install.display()
        ));
    }
    let (name, bytes) = match &o.source {
        Source::Release { have, zip } => fetch_release(*have, zip.as_deref())?,
        Source::Local { zip, .. } => {
            busy("Reading the update");
            let name = zip
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, read_capped(zip)?)
        }
    };
    if action == Action::Download {
        let home = std::env::var_os("HOME").ok_or("no home folder")?;
        let dir = PathBuf::from(home).join("Downloads");
        let _ = std::fs::create_dir_all(&dir);
        let to = dir.join(&name);
        std::fs::write(&to, &bytes).map_err(|e| format!("{}: {e}", to.display()))?;
        reveal(&to);
        return Ok(Phase::Saved(to));
    }
    busy("Unpacking the update");
    let staging = stage(&bytes, &install)?;
    busy("Starting the update");
    start_helper(
        &install,
        &staging,
        Some(std::env::args_os().skip(1).collect()),
    )?;
    Ok(Phase::Staged)
}

#[cfg(test)]
#[path = "selfupdate_tests.rs"]
mod tests;
