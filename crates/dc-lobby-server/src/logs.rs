//! The UTF-8 server log (`--log`) and the packet log (`--pktlog`).
//!
//! Both files are opened in append mode. A write error never stops the server. The console copy goes to stdout
//! and its errors are ignored too: the harness runs the server with a cp1252 pipe or with stdout closed
//! (SPEC: LOBBY_HARNESS.md §1 "Process-level behaviour").
//! The logger's mutexes are leaves: nothing takes the server state lock while holding one.

use crate::clock;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

/// Longest log line text, in characters: a login name field alone can be 4096 units.
const MAX_LINE_CHARS: usize = 2000;

/// What goes into a log line: player text (names, chat, a login field) cannot start a line of its own or send a terminal
/// a control sequence, and is cut at [`MAX_LINE_CHARS`]. The lobby's own multi-line notes keep their shape as ` | `.
// DC-DEFAULT: LG-324 (proposed) log lines hold no control characters
pub fn log_text(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len().min(MAX_LINE_CHARS));
    for (i, c) in msg.chars().enumerate() {
        if i == MAX_LINE_CHARS {
            out.push_str("...");
            break;
        }
        match c {
            '\n' | '\r' => out.push_str(" | "),
            '\t' => out.push(c),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
#[test]
fn player_text_cannot_forge_a_log_line() {
    assert_eq!(
        log_text("chat: hi\r\n2026-01-01 admin: stop"),
        "chat: hi |  | 2026-01-01 admin: stop"
    );
    assert_eq!(log_text("a\u{1b}[2Jb\tc"), "a [2Jb\tc");
    let long = log_text(&"x".repeat(5000));
    assert_eq!(long.chars().count(), MAX_LINE_CHARS + 3);
}

/// Which listener a connection came in on (pktlog column 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listener {
    Auth,
    Lobby,
    Clan,
}

impl Listener {
    pub fn tag(self) -> &'static str {
        match self {
            Listener::Auth => "auth",
            Listener::Lobby => "lobby",
            Listener::Clan => "clan",
        }
    }
}

pub struct Logger {
    file: Option<Mutex<File>>,
    pkt: Option<Mutex<File>>,
}

fn open_append(p: &Path) -> std::io::Result<File> {
    if let Some(dir) = p.parent() {
        if !dir.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(dir);
        }
    }
    OpenOptions::new().create(true).append(true).open(p)
}

/// Frame id of the login request (0x200) and where its password text sits (wstr[17] at +0x2006, wire.rs).
const LOGIN_REQUEST_ID: i32 = 0x200;
const LOGIN_PASSWORD_AT: usize = 0x2006;
const LOGIN_PASSWORD_UNITS: usize = 17;

/// LG-230: the packet log never holds a password: in a login request every typed UTF-16 unit of the password field
/// is written as `*` (the length stays), the rest of the frame is unchanged.
fn masked(frame_id: i32, to_client: bool, payload: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if to_client || frame_id != LOGIN_REQUEST_ID || payload.len() <= LOGIN_PASSWORD_AT {
        return std::borrow::Cow::Borrowed(payload);
    }
    let mut v = payload.to_vec();
    let end = (LOGIN_PASSWORD_AT + LOGIN_PASSWORD_UNITS * 2).min(v.len());
    for u in v[LOGIN_PASSWORD_AT..end].as_chunks_mut::<2>().0 {
        if *u != [0, 0] {
            *u = [b'*', 0];
        }
    }
    std::borrow::Cow::Owned(v)
}

/// One packet-log line: `<time> <kind>#<sid> <dir> <fid> <hex>`.
// SPEC: LOBBY_HARNESS.md §4
pub fn pkt_line(
    stamp: &str,
    kind: Listener,
    sid: u64,
    to_client: bool,
    frame_id: i32,
    payload: &[u8],
) -> String {
    let payload = masked(frame_id, to_client, payload);
    let mut hex = String::with_capacity(payload.len() * 2);
    for b in payload.iter() {
        hex.push_str(&format!("{:02x}", b));
    }
    format!(
        "{} {}#{} {} 0x{:x} {}",
        stamp,
        kind.tag(),
        sid,
        if to_client { "S->C" } else { "C->S" },
        frame_id,
        hex
    )
}

impl Logger {
    pub fn open(log: Option<&Path>, pktlog: Option<&Path>) -> (Self, Vec<String>) {
        let mut errs = Vec::new();
        let mut get = |p: Option<&Path>| match p {
            Some(p) => match open_append(p) {
                Ok(f) => Some(Mutex::new(f)),
                Err(e) => {
                    errs.push(format!("cannot open {}: {}", p.display(), e));
                    None
                }
            },
            None => None,
        };
        let file = get(log);
        let pkt = get(pktlog);
        (Self { file, pkt }, errs)
    }

    #[cfg(test)]
    pub fn none() -> Self {
        Self {
            file: None,
            pkt: None,
        }
    }

    /// Writes one timestamped line to the log file and to stdout.
    pub fn line(&self, msg: &str) {
        let text = format!("{} {}\n", clock::local_now().0.stamp(), log_text(msg));
        if let Some(f) = &self.file {
            if let Ok(mut g) = f.lock() {
                let _ = g.write_all(text.as_bytes());
                let _ = g.flush();
            }
        }
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    }

    pub fn has_pktlog(&self) -> bool {
        self.pkt.is_some()
    }

    pub fn packet(&self, kind: Listener, sid: u64, to_client: bool, frame_id: i32, payload: &[u8]) {
        if let Some(f) = &self.pkt {
            // one write per line: a hard kill must not glue two lines together
            let mut line = pkt_line(
                &clock::local_now().0.stamp(),
                kind,
                sid,
                to_client,
                frame_id,
                payload,
            );
            line.push('\n');
            if let Ok(mut g) = f.lock() {
                let _ = g.write_all(line.as_bytes());
                let _ = g.flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pktlog_columns() {
        let l = pkt_line(
            "2026-10-24 10:00:00.000",
            Listener::Lobby,
            7,
            true,
            0x301,
            &[0x01, 0x03, 0xab],
        );
        assert_eq!(l, "2026-10-24 10:00:00.000 lobby#7 S->C 0x301 0103ab");
        let l = pkt_line("t", Listener::Auth, 1, false, 0x206, &[]);
        assert_eq!(l, "t auth#1 C->S 0x206 ");
    }

    #[test]
    fn a_login_packet_log_line_never_holds_the_password() {
        let mut p = vec![0u8; 0x2054];
        for (i, u) in "LOCAL:erika".encode_utf16().enumerate() {
            p[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        for (i, u) in "hunter2pass".encode_utf16().enumerate() {
            p[0x2006 + i * 2..0x2006 + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        let l = pkt_line("t", Listener::Auth, 1, false, 0x200, &p);
        let enc = |s: &str| -> String {
            s.encode_utf16()
                .flat_map(|u| u.to_le_bytes())
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        assert!(!l.contains(&enc("hunter2")) && !l.contains(&enc("pass")));
        assert!(l.contains(&enc("LOCAL:erika")), "the token stays");
        assert!(l.contains(&enc("***********")), "masked, length kept");
        // other frames are untouched
        let l = pkt_line("t", Listener::Auth, 1, false, 0x206, &p);
        assert!(l.contains(&enc("hunter2pass")));
    }

    #[test]
    fn log_file_is_utf8_append() {
        let p = std::env::temp_dir().join(format!("ca_lobby_log_{}.log", std::process::id()));
        let _ = std::fs::remove_file(&p);
        {
            let (l, e) = Logger::open(Some(&p), None);
            assert!(e.is_empty());
            l.line("first");
            l.line("\u{c548}\u{b155}");
        }
        {
            let (l, _) = Logger::open(Some(&p), None);
            l.line("second");
        }
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.contains("first") && s.contains("\u{c548}\u{b155}") && s.contains("second"));
        assert_eq!(s.lines().count(), 3);
        std::fs::remove_file(&p).ok();
    }
}
