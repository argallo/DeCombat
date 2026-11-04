//! What the bot saw, on disk: a capture in the captap format (format 1, one JSON object per line: `meta`,
//! `open`, `close`, `frame` for TCP, `dgram` for UDP) and a JSONL event log (one object per decoded message,
//! milestone and statistics snapshot). Both sinks are optional; a [`Recorder`] can be shared between threads.
//!
//! The capture keys follow the captap rules: TCP `0x<frame_id>/0x<msg_id>`, UDP `u:<hex of the first byte>`.
//! Datagram lines carry one extra field, `blocks`, with the link blocks of the datagram decoded (sequence numbers,
//! entry sizes and the first bytes of each entry); captap ignores fields it does not know.
// HARNESS: captap.py (capture format 1), capture README.md §1
// SPEC: REFERENCE_TESTING.md §3 (tool and formats)

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{json, Map, Value};

use crate::logring::LogWriter;

/// Version of the capture format written here.
// HARNESS: captap.py (CAPTURE_FORMAT = 1)
pub const CAPTURE_FORMAT: u32 = 1;
/// Bytes of a datagram used as its capture key.
// HARNESS: captap.py (`--udp-key-bytes`, default 1)
pub const UDP_KEY_BYTES: usize = 1;
/// Bytes of a message kept as its `head` in the event log.
pub const HEAD_BYTES: usize = 24;

struct Inner {
    start: Instant,
    seq: u64,
    next_conn: u32,
    /// Both files go through bounded writer threads (perf-hitch): the session thread that records a datagram or an
    /// event holds the recorder lock, and often the session lock the frame thread waits for, so no file I/O may
    /// happen under it. A full queue drops the newest line (counted, see [`Recorder::log_stats`]); capture lines keep
    /// their `seq`, so a reader sees the gap.
    capture: Option<LogWriter>,
    log: Option<LogWriter>,
    /// Log lines kept in memory as well (tests read them back), bounded.
    memory: Option<Vec<Value>>,
}

/// Shared capture + event-log sink.
#[derive(Clone)]
pub struct Recorder {
    inner: Arc<Mutex<Inner>>,
    bot: String,
}

/// Wall-clock seconds since 1970 (lets captures of several bot processes be merged in time order).
pub fn wall_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs_f64() * 1e6).round() / 1e6)
        .unwrap_or(0.0)
}

/// Lower-case hex of `b`.
pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

/// Lower-case hex of at most [`HEAD_BYTES`] bytes of `b`.
pub fn head(b: &[u8]) -> String {
    hex(&b[..b.len().min(HEAD_BYTES)])
}

impl Recorder {
    /// A recorder that writes the capture to `capture` and the event log to `log` (each optional).
    pub fn new(capture: Option<&Path>, log: Option<&Path>, bot: &str) -> io::Result<Recorder> {
        let open = |p: Option<&Path>| -> io::Result<Option<LogWriter>> {
            p.map(LogWriter::create).transpose()
        };
        let r = Recorder {
            inner: Arc::new(Mutex::new(Inner {
                start: Instant::now(),
                seq: 0,
                next_conn: 0,
                capture: open(capture)?,
                log: open(log)?,
                memory: None,
            })),
            bot: bot.to_string(),
        };
        r.write_meta();
        Ok(r)
    }

    /// A recorder that writes nothing.
    pub fn null(bot: &str) -> Recorder {
        Recorder {
            inner: Arc::new(Mutex::new(Inner {
                start: Instant::now(),
                seq: 0,
                next_conn: 0,
                capture: None,
                log: None,
                memory: None,
            })),
            bot: bot.to_string(),
        }
    }

    /// Keeps the event-log lines in memory too (for tests; see [`Recorder::memory`]).
    pub fn keep_in_memory(self) -> Recorder {
        if let Ok(mut g) = self.inner.lock() {
            g.memory = Some(Vec::new());
        }
        self
    }

    /// The same sinks under another bot name (several bots in one process share one capture).
    pub fn for_bot(&self, bot: &str) -> Recorder {
        Recorder {
            inner: self.inner.clone(),
            bot: bot.to_string(),
        }
    }

    pub fn bot(&self) -> &str {
        &self.bot
    }

    /// The event-log lines kept in memory so far.
    pub fn memory(&self) -> Vec<Value> {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.memory.clone())
            .unwrap_or_default()
    }

    /// How many event-log lines of `kind` the in-memory log holds (0 without [`Recorder::keep_in_memory`]).
    pub fn count(&self, kind: &str) -> usize {
        self.inner
            .lock()
            .ok()
            .and_then(|g| {
                g.memory
                    .as_ref()
                    .map(|m| m.iter().filter(|e| e["ev"] == kind).count())
            })
            .unwrap_or(0)
    }

    fn write_meta(&self) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        if g.capture.is_none() {
            return;
        }
        g.seq += 1;
        let line = json!({
            "seq": g.seq, "t": 0.0, "ev": "meta", "format": CAPTURE_FORMAT, "udp_key_bytes": UDP_KEY_BYTES,
            "ports": {"auth": {"proto": "tcp"}, "lobby": {"proto": "tcp"}, "game": {"proto": "udp"}},
            "tool": "dc-botclient"
        });
        if let Some(c) = g.capture.as_ref() {
            c.push(line.to_string());
        }
    }

    fn capture_line(&self, mut fields: Map<String, Value>) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        if g.capture.is_none() {
            return;
        }
        g.seq += 1;
        let t = (g.start.elapsed().as_secs_f64() * 1e6).round() / 1e6;
        fields.insert("seq".into(), json!(g.seq));
        fields.insert("t".into(), json!(t));
        fields.insert("wall".into(), json!(wall_secs()));
        fields.insert("bot".into(), json!(self.bot));
        let v = Value::Object(fields);
        if let Some(c) = g.capture.as_ref() {
            c.push(v.to_string());
        }
    }

    /// Registers a connection (TCP socket or UDP peer) and returns its capture id.
    pub fn open_conn(&self, port: &str, proto: &str, peer: &str) -> u32 {
        let conn = {
            let Ok(mut g) = self.inner.lock() else {
                return 0;
            };
            g.next_conn += 1;
            g.next_conn
        };
        let mut m = Map::new();
        m.insert("ev".into(), json!("open"));
        m.insert("conn".into(), json!(conn));
        m.insert("port".into(), json!(port));
        m.insert("proto".into(), json!(proto));
        m.insert("peer".into(), json!(peer));
        self.capture_line(m);
        conn
    }

    /// Records a connection close (`dir` = the side that closed).
    pub fn close_conn(&self, conn: u32, port: &str, proto: &str, dir: &str) {
        let mut m = Map::new();
        m.insert("ev".into(), json!("close"));
        m.insert("conn".into(), json!(conn));
        m.insert("port".into(), json!(port));
        m.insert("proto".into(), json!(proto));
        m.insert("dir".into(), json!(dir));
        self.capture_line(m);
    }

    /// Records one whole TCP frame (8-byte header included).
    // HARNESS: captap.py (frame_key: 0x<frame_id>/0x<msg_id>)
    pub fn tcp_frame(&self, conn: u32, port: &str, dir: &str, frame: &[u8]) {
        let frame_id = frame
            .get(0..4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .unwrap_or(0);
        let msg_id = frame.get(8..10).map(|b| u16::from_le_bytes([b[0], b[1]]));
        let key = format!(
            "0x{:x}/{}",
            frame_id as u32,
            msg_id.map_or("-".to_string(), |m| format!("0x{m:x}"))
        );
        let mut m = Map::new();
        m.insert("ev".into(), json!("frame"));
        m.insert("conn".into(), json!(conn));
        m.insert("port".into(), json!(port));
        m.insert("proto".into(), json!("tcp"));
        m.insert("dir".into(), json!(dir));
        m.insert("key".into(), json!(key));
        m.insert("frame_id".into(), json!(frame_id));
        m.insert("msg_id".into(), msg_id.map_or(Value::Null, |x| json!(x)));
        m.insert("len".into(), json!(frame.len()));
        m.insert("hex".into(), json!(hex(frame)));
        self.capture_line(m);
    }

    /// Records one UDP datagram with its decoded link blocks (`blocks` may be `Null`).
    pub fn udp_dgram(&self, conn: u32, dir: &str, bytes: &[u8], blocks: Value) {
        let mut m = Map::new();
        m.insert("ev".into(), json!("dgram"));
        m.insert("conn".into(), json!(conn));
        m.insert("port".into(), json!("game"));
        m.insert("proto".into(), json!("udp"));
        m.insert("dir".into(), json!(dir));
        m.insert(
            "key".into(),
            json!(format!(
                "u:{}",
                hex(&bytes[..bytes.len().min(UDP_KEY_BYTES)])
            )),
        );
        m.insert("len".into(), json!(bytes.len()));
        m.insert("hex".into(), json!(hex(bytes)));
        if !blocks.is_null() {
            m.insert("blocks".into(), blocks);
        }
        self.capture_line(m);
    }

    /// One event-log line: `{"t", "bot", "ev": kind, ...fields}`.
    pub fn event(&self, kind: &str, fields: Value) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        let t = (g.start.elapsed().as_secs_f64() * 1e3).round() / 1e3;
        let mut m = Map::new();
        m.insert("t".into(), json!(t));
        m.insert("wall".into(), json!(wall_secs()));
        m.insert("bot".into(), json!(self.bot));
        m.insert("ev".into(), json!(kind));
        if let Value::Object(f) = fields {
            for (k, v) in f {
                m.insert(k, v);
            }
        }
        let v = Value::Object(m);
        // the writer thread flushes whenever its queue runs empty, so the log stays near live for readers
        if let Some(l) = g.log.as_ref() {
            l.push(v.to_string());
        }
        if let Some(mem) = g.memory.as_mut() {
            if mem.len() < 200_000 {
                mem.push(v);
            }
        }
    }

    /// Waits until both files hold every line recorded so far (BLOCKS: end of a session, tests).
    pub fn flush(&self) {
        if let Ok(g) = self.inner.lock() {
            if let Some(c) = g.capture.as_ref() {
                c.flush();
            }
            if let Some(l) = g.log.as_ref() {
                l.flush();
            }
        }
    }

    /// The writer counters of both files (lines written, dropped, queue high-water marks), for exit reports.
    pub fn log_stats(&self) -> String {
        let Ok(g) = self.inner.lock() else {
            return String::new();
        };
        let one = |n: &str, w: &Option<LogWriter>| {
            w.as_ref().map(|w| format!("{n} {}", w.stats().line()))
        };
        [one("capture", &g.capture), one("event log", &g.log)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_lines_follow_format_1() {
        let dir = std::env::temp_dir().join(format!("dc-botclient-rec-{}", std::process::id()));
        let cap = dir.join("c.jsonl");
        let log = dir.join("l.jsonl");
        let r = Recorder::new(Some(&cap), Some(&log), "Bot1").unwrap();
        let c = r.open_conn("lobby", "tcp", "127.0.0.1:1");
        // frame 0x301 with a 6-byte inner header
        let f = [0x01, 0x03, 0, 0, 6, 0, 0, 0, 0x01, 0x03, 6, 0, 0, 0];
        r.tcp_frame(c, "lobby", "s2c", &f);
        r.udp_dgram(2, "c2s", &[0x10, 0x7f, 0x33, 0x01], Value::Null);
        r.event("milestone", json!({"step": "joined"}));
        r.flush();
        drop(r);
        let lines: Vec<Value> = std::fs::read_to_string(&cap)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["ev"], "meta");
        assert_eq!(lines[0]["format"], 1);
        assert_eq!(lines[1]["ev"], "open");
        assert_eq!(lines[2]["key"], "0x301/0x301");
        assert_eq!(lines[2]["len"], 14);
        assert_eq!(lines[3]["key"], "u:10");
        let seqs: Vec<u64> = lines.iter().map(|l| l["seq"].as_u64().unwrap()).collect();
        assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1));
        let ev = std::fs::read_to_string(&log).unwrap();
        assert!(ev.contains("\"step\":\"joined\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
