//! The upload of a packed report (CLI-1842): `POST /diag/report` on the lobby's web port over TLS (the pinned
//! certificate), with the identity token as `Authorization: Bearer` when the player has one (docs/DIAGNOSTICS.md §3).
// DC-DEFAULT: CLI-1842 (proposed, USER REQUEST 2026-11-02) what the client does with each answer

use dc_botclient::tls::{self, Trust};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::time::Duration;

pub const PATH: &str = "/diag/report";
pub const CONTENT_TYPE: &str = "application/x-dcdiag";
/// Time for the connection and each read / write (a 1 MiB body on a slow line takes a while).
pub const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_ANSWER: u64 = 4096;

/// What became of an upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Stored as report `N` on the lobby.
    Sent(i64),
    /// Refused for good (not a report, too large): the queue drops it.
    Rejected(String),
    /// Not now (no answer, a limit, the store full): it stays queued.
    Later(String),
}

/// The outcome of an HTTP answer.
pub fn judge(status: u16, body: &str) -> Outcome {
    let v: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let err = v
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|e| e.as_str())
        .map(|e| e.chars().take(200).collect::<String>())
        .unwrap_or_else(|| format!("answer {status}"));
    match status {
        200 => match v.as_ref().and_then(|v| v.get("id")).and_then(|i| i.as_i64()) {
            Some(id) => Outcome::Sent(id),
            None => Outcome::Later("the lobby's answer has no report number".into()),
        },
        400 | 413 => Outcome::Rejected(err),
        _ => Outcome::Later(err),
    }
}

/// The request head for a body of `len` bytes.
pub fn head(host: &str, len: usize, token: Option<&str>) -> String {
    let auth = token
        .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_graphic()))
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    format!(
        "POST {PATH} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nContent-Type: {CONTENT_TYPE}\r\n\
         {auth}Content-Length: {len}\r\nConnection: close\r\n\r\n"
    )
}

/// Sends `packed` to the web port `addr`.
pub fn send(addr: SocketAddr, trust: &Trust, packed: &[u8], token: Option<&str>) -> Outcome {
    let host = addr.to_string();
    let mut c = match tls::connect_with(addr, TIMEOUT, trust) {
        Ok(c) => c,
        Err(e) => return Outcome::Later(e),
    };
    let _ = c.tcp().set_read_timeout(Some(TIMEOUT));
    let _ = c.tcp().set_write_timeout(Some(TIMEOUT));
    let mut req = head(&host, packed.len(), token).into_bytes();
    req.extend_from_slice(packed);
    if let Err(e) = c.write_all(&req) {
        return Outcome::Later(format!("{host}: send: {e}"));
    }
    let _ = c.flush();
    let mut buf = Vec::new();
    let _ = (&mut c).take(MAX_ANSWER).read_to_end(&mut buf);
    c.close();
    let text = String::from_utf8_lossy(&buf);
    let Some((h, body)) = text.split_once("\r\n\r\n") else {
        return Outcome::Later(format!("{host}: no answer"));
    };
    match h
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
    {
        Some(status) => judge(status, body),
        None => Outcome::Later(format!("{host}: not an HTTP answer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_decide_whether_the_report_stays_queued() {
        assert_eq!(judge(200, "{\"id\":42}"), Outcome::Sent(42));
        assert!(matches!(judge(200, "{}"), Outcome::Later(_)));
        assert_eq!(judge(400, "{\"error\":\"not a report\"}"), Outcome::Rejected("not a report".into()));
        assert!(matches!(judge(413, ""), Outcome::Rejected(_)));
        assert_eq!(
            judge(429, "{\"error\":\"too many reports\",\"retry_after\":60}"),
            Outcome::Later("too many reports".into())
        );
        assert!(matches!(judge(503, ""), Outcome::Later(_)));
        assert!(matches!(judge(404, ""), Outcome::Later(_)));
    }

    #[test]
    fn the_token_goes_only_into_its_header_and_only_when_clean() {
        let h = head("1.2.3.4:18080", 10, Some("abc.def"));
        assert!(h.contains("Authorization: Bearer abc.def\r\n"));
        assert!(h.contains("Content-Length: 10\r\n"));
        assert!(!head("h", 1, Some("bad\r\nX: y")).contains("Authorization"));
        assert!(!head("h", 1, None).contains("Authorization"));
    }

    /// A plain loopback lobby stand-in: the request arrives whole with its header, the answer is judged.
    #[test]
    fn a_report_reaches_a_loopback_server() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = s.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    if buf.len() >= i + 4 + 5 {
                        break;
                    }
                }
            }
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"id\":7}").unwrap();
            String::from_utf8_lossy(&buf).into_owned()
        });
        let plain = tls::Trust {
            mode: tls::Mode::Off,
            ..tls::Trust::default()
        };
        let out = send(addr, &plain, b"hello", Some("t0k.en"));
        let got = t.join().unwrap();
        assert_eq!(out, Outcome::Sent(7));
        assert!(got.starts_with("POST /diag/report HTTP/1.1\r\n"), "{got}");
        assert!(got.contains("Authorization: Bearer t0k.en"), "{got}");
        assert!(got.ends_with("hello"), "{got}");
    }
}
