//! The client's version against the lobby's: before a login the client reads `GET /client_version.json` from the lobby's
//! web port (LG-322). Below `min_version` it shows why and does not log in; below `latest` it tells the player once and
//! goes on. Only a release build knows its version (the tag, `tools/ci_env.sh`); a development build asks nothing and is
//! never refused. No answer, or a lobby that names no version, changes nothing: the login's own errors speak for it.
// DC-DEFAULT: CLI-1651 (proposed, USER REQUEST 2026-11-02), see DC_DEFAULTS.md

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

/// The whole check may hold the login back this long at most.
const TIMEOUT: Duration = Duration::from_secs(3);
/// Larger than any answer of ours.
const MAX_DOC: u64 = 4096;
/// The start of the refusal text, how the login screen knows it.
const REFUSAL_START: &str = "This DeCombat version";

/// The version of this build, none for a development build.
pub fn current() -> Option<[u32; 3]> {
    parse(env!("DC_RELEASE_VERSION"))
}

/// `1.2.3`, also with a `-rc1` tail (a pre-release counts as its release).
pub fn parse(s: &str) -> Option<[u32; 3]> {
    let s = s.trim();
    let s = s.split_once('-').map_or(s, |(v, _)| v);
    let mut p = s.split('.').map(|n| n.parse::<u32>().ok());
    let v = [p.next()??, p.next()??, p.next()??];
    p.next().is_none().then_some(v)
}

fn show(v: [u32; 3]) -> String {
    format!("{}.{}.{}", v[0], v[1], v[2])
}

/// What the lobby's document means for a client of version `have`.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Fine,
    /// The text of the once-only notice.
    Newer(String),
    /// The text of the refusal.
    TooOld(String),
}

/// Reads the document (a broken one is no verdict: `Fine`). The address text is the lobby's, so it is cut and cleaned.
pub fn judge(doc: &str, have: [u32; 3]) -> Verdict {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(doc) else {
        return Verdict::Fine;
    };
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).and_then(parse);
    let url: String = v
        .get("update_url")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect();
    let url = if url.is_empty() { "decombat.com" } else { &url };
    if let Some(min) = field("min_version") {
        if have < min {
            return Verdict::TooOld(format!(
                "{REFUSAL_START} ({}) is too old for this server (it needs {} or newer). Get the new version at {url}.",
                show(have),
                show(min)
            ));
        }
    }
    if let Some(latest) = field("latest") {
        if have < latest {
            return Verdict::Newer(format!(
                "A new DeCombat version ({}) is available. Get it at {url}.",
                show(latest)
            ));
        }
    }
    Verdict::Fine
}

/// True for the text [`judge`] refuses with.
pub fn is_refusal(text: &str) -> bool {
    text.starts_with(REFUSAL_START)
}

fn pending() -> &'static Mutex<Option<String>> {
    static P: Mutex<Option<String>> = Mutex::new(None);
    &P
}

/// The notice waiting for the screen after the login, once.
pub fn take_notice() -> Option<String> {
    pending().lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The check of a login to the lobby at `auth`: `Err(text)` when this client is too old for it.
pub fn before_login(auth: SocketAddr) -> Result<(), String> {
    match current() {
        Some(have) => check(auth, have),
        None => Ok(()),
    }
}

/// [`before_login`] for a client of version `have`.
pub fn check(auth: SocketAddr, have: [u32; 3]) -> Result<(), String> {
    let Some(web) = crate::menu::clanpage::web_addr(Some(auth)) else {
        return Ok(());
    };
    let Ok(reply) = crate::mods::web_get(web, "/client_version.json", MAX_DOC, TIMEOUT, TIMEOUT, &mut |_, _| {}) else {
        return Ok(());
    };
    if reply.status != 200 {
        return Ok(());
    }
    match judge(&String::from_utf8_lossy(&reply.body), have) {
        Verdict::Fine => Ok(()),
        Verdict::TooOld(text) => Err(text),
        Verdict::Newer(text) => {
            static TOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !TOLD.swap(true, std::sync::atomic::Ordering::Relaxed) {
                *pending().lock().unwrap_or_else(|e| e.into_inner()) = Some(text);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    const DOC: &str = r#"{"latest":"0.4.0","min_version":"0.3.0","update_url":"decombat.com"}"#;

    #[test]
    fn versions_parse_and_order_by_number() {
        assert_eq!(parse("0.10.2"), Some([0, 10, 2]));
        assert_eq!(parse(" 1.2.3-rc1 "), Some([1, 2, 3]));
        for bad in ["", "1.2", "1.2.3.4", "v1.2.3", "a.b.c", "1.2.x"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert!(parse("0.10.0") > parse("0.9.9"));
    }

    #[test]
    fn the_document_decides_refusal_notice_or_nothing() {
        match judge(DOC, [0, 2, 9]) {
            Verdict::TooOld(t) => {
                assert!(is_refusal(&t), "{t}");
                assert!(t.contains("0.2.9") && t.contains("0.3.0") && t.contains("decombat.com"), "{t}");
            }
            v => panic!("{v:?}"),
        }
        match judge(DOC, [0, 3, 0]) {
            Verdict::Newer(t) => {
                assert!(!is_refusal(&t), "{t}");
                assert!(t.contains("0.4.0") && t.contains("decombat.com"), "{t}");
            }
            v => panic!("{v:?}"),
        }
        assert_eq!(judge(DOC, [0, 4, 0]), Verdict::Fine);
        assert_eq!(judge(DOC, [1, 0, 0]), Verdict::Fine);
    }

    #[test]
    fn a_broken_or_empty_document_stops_nobody() {
        for doc in ["", "not json", "{}", r#"{"latest":"soon","min_version":7}"#, "[]"] {
            assert_eq!(judge(doc, [0, 0, 1]), Verdict::Fine, "{doc}");
        }
        let hostile = "{\"min_version\":\"9.0.0\",\"update_url\":\"evil\\r\\nsite\"}";
        match judge(hostile, [0, 0, 1]) {
            Verdict::TooOld(t) => assert!(t.contains("evilsite"), "{t}"),
            v => panic!("{v:?}"),
        }
    }

    /// One answer on a loopback port (the auth port is 8080 below it); the client's own web request reads it.
    fn lobby_answering(status: &'static str, doc: &'static str) -> SocketAddr {
        let l = (20000..40000u16)
            .find_map(|p| std::net::TcpListener::bind(("127.0.0.1", p)).ok())
            .unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            // the client tries TLS first and falls back to plain on loopback when the server closes on its hello
            for s in l.incoming() {
                let Ok(mut s) = s else { continue };
                let mut first = [0u8; 1];
                if s.peek(&mut first).ok() != Some(1) || first[0] == 0x16 {
                    continue;
                }
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{doc}",
                    doc.len()
                );
                return;
            }
        });
        SocketAddr::from(([127, 0, 0, 1], port - 8080))
    }

    #[test]
    fn a_login_is_refused_told_or_let_through_by_the_lobbys_answer() {
        let e = check(lobby_answering("200 OK", DOC), [0, 1, 0]).unwrap_err();
        assert!(is_refusal(&e), "{e}");
        check(lobby_answering("200 OK", DOC), [0, 3, 5]).unwrap();
        let told = take_notice().expect("the notice waits for the next screen");
        assert!(told.contains("0.4.0"), "{told}");
        // once per run
        check(lobby_answering("200 OK", DOC), [0, 3, 5]).unwrap();
        assert_eq!(take_notice(), None);
        // a lobby with no version, or no web port at all, changes nothing
        check(lobby_answering("404 Not Found", ""), [0, 1, 0]).unwrap();
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        check(SocketAddr::new(closed.ip(), closed.port().wrapping_sub(8080)), [0, 1, 0]).unwrap();
    }

    #[test]
    fn a_development_build_claims_no_version() {
        if env!("DC_RELEASE_VERSION").is_empty() {
            assert_eq!(current(), None);
        }
    }
}
