//! The diagnostic report's document (CLI-1840): the facts of one failure, every text redacted, gzip-compressed, at
//! most [`MAX_COMPRESSED`] bytes. The format is docs/DIAGNOSTICS.md §2 (`dcdiag1`).
// DC-DEFAULT: CLI-1840 (proposed, USER REQUEST 2026-11-02) the document, the file tails and their caps

use super::redact::Redactor;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Map, Value};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "dcdiag1";
/// The compressed document's cap (the lobby refuses more).
pub const MAX_COMPRESSED: usize = 1 << 20;
/// Tails of the log files.
pub const OUT_TAIL: u64 = 256 * 1024;
pub const NET_TAIL: u64 = 512 * 1024;
pub const MENU_TAIL: u64 = 128 * 1024;
pub const WATCHDOG_TAIL: u64 = 64 * 1024;
pub const DIR_FILE_TAIL: u64 = 128 * 1024;
/// The player's note and the summary line.
pub const NOTE_MAX: usize = 1000;
pub const SUMMARY_MAX: usize = 200;
/// Text files of a stall / crash folder that go in (minidumps never do).
pub const DIR_FILES: &[&str] = &["stall.json", "threads.txt", "threads2.txt", "events_tail.jsonl", "crash.txt"];

/// One file of the report: its name and its (redacted) text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileText {
    pub name: String,
    pub text: String,
}

/// The last `max` bytes of `path`, starting at a line start when cut; `None` when the file cannot be read or is
/// empty.
pub fn tail(path: &Path, max: u64) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    let from = len.saturating_sub(max);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::with_capacity((len - from) as usize);
    f.take(max).read_to_end(&mut buf).ok()?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if from > 0 {
        // drop the cut first line
        if let Some(i) = text.find('\n') {
            text = text[i + 1..].to_string();
        }
        text.insert_str(0, &format!("[first {from} bytes cut]\n"));
    }
    Some(text)
}

/// The report files of a stall or crash folder, or a `panic_*.txt` file, named `<folder>/<file>`.
pub fn artifact_files(path: &Path) -> Vec<(String, PathBuf)> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if path.is_dir() {
        DIR_FILES
            .iter()
            .map(|f| (format!("{name}/{f}"), path.join(f)))
            .filter(|(_, p)| p.is_file())
            .collect()
    } else if path.is_file() {
        vec![(name, path.to_path_buf())]
    } else {
        Vec::new()
    }
}

/// Characters of `s` up to `max`, one line (no control characters).
pub fn one_line(s: &str, max: usize) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Everything of one report before it is packed.
#[derive(Debug, Clone, Default)]
pub struct Parts {
    pub id: String,
    pub created: i64,
    pub kind: &'static str,
    pub summary: String,
    pub client: Value,
    pub system: Value,
    pub settings: Value,
    pub match_info: Option<Value>,
    pub link: Option<Value>,
    pub nettest: Option<Value>,
    pub note: Option<String>,
    pub login: Option<String>,
    pub files: Vec<FileText>,
}

/// Redacts every string of a JSON value.
pub fn redact_value(v: &Value, r: &Redactor) -> Value {
    match v {
        Value::String(s) => Value::String(r.text(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| redact_value(x, r)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), redact_value(x, r)))
                .collect::<Map<String, Value>>(),
        ),
        x => x.clone(),
    }
}

/// The document (redacted).
pub fn document(p: &Parts, r: &Redactor) -> Value {
    let mut o = Map::new();
    o.insert("format".into(), json!(FORMAT));
    o.insert("id".into(), json!(p.id));
    o.insert("created".into(), json!(p.created));
    o.insert("kind".into(), json!(p.kind));
    o.insert("summary".into(), json!(r.text(&one_line(&p.summary, SUMMARY_MAX))));
    o.insert("client".into(), redact_value(&p.client, r));
    o.insert("system".into(), redact_value(&p.system, r));
    o.insert("settings".into(), redact_value(&p.settings, r));
    if let Some(m) = &p.match_info {
        o.insert("match".into(), redact_value(m, r));
    }
    if let Some(l) = &p.link {
        o.insert("link".into(), redact_value(l, r));
    }
    if let Some(n) = &p.nettest {
        o.insert("nettest".into(), redact_value(n, r));
    }
    if let Some(n) = p.note.as_deref().filter(|n| !n.trim().is_empty()) {
        let n: String = n.chars().take(NOTE_MAX).collect();
        o.insert("note".into(), json!(r.text(&n)));
    }
    if let Some(l) = p.login.as_deref().filter(|l| !l.trim().is_empty()) {
        o.insert("login".into(), json!(one_line(l, 64)));
    }
    o.insert(
        "files".into(),
        Value::Array(
            p.files
                .iter()
                .map(|f| json!({"name": f.name, "text": r.text(&f.text)}))
                .collect(),
        ),
    );
    Value::Object(o)
}

pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), Compression::best());
    let _ = e.write_all(bytes);
    e.finish().unwrap_or_default()
}

/// The packed report: the document gzip-compressed; the largest file is cut to its later half (and dropped below
/// 1 KiB) until the result fits [`MAX_COMPRESSED`].
pub fn pack(p: &Parts, r: &Redactor) -> Vec<u8> {
    let mut p = p.clone();
    loop {
        let doc = document(&p, r);
        let gz = gzip(doc.to_string().as_bytes());
        if gz.len() <= MAX_COMPRESSED {
            return gz;
        }
        let Some(i) = (0..p.files.len()).max_by_key(|&i| p.files[i].text.len()) else {
            return gz;
        };
        let t = &p.files[i].text;
        if t.len() < 1024 {
            p.files.remove(i);
            continue;
        }
        let mut cut = t.len() / 2;
        while !t.is_char_boundary(cut) {
            cut += 1;
        }
        p.files[i].text = format!("[cut to fit the report]\n{}", &t[cut..]);
    }
}

/// Reads a packed report back (tests, the queue's look at its entries).
pub fn unpack(gz: &[u8]) -> Option<Value> {
    let mut d = flate2::read::GzDecoder::new(gz).take(16 << 20);
    let mut s = String::new();
    d.read_to_string(&mut s).ok()?;
    serde_json::from_str(&s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::redact::{Redactor, USER};

    fn parts() -> Parts {
        Parts {
            id: "00112233aabbccdd".into(),
            created: 1_800_000_000,
            kind: "link_lost",
            summary: "The match link was lost\n(idle)".into(),
            client: json!({"version": "dev"}),
            system: json!({"os": "Windows 11", "gpu": "RTX"}),
            settings: json!({"fps_cap": 360}),
            link: Some(json!({"rtt_mean": 38.0, "peer": "81.2.69.160:27015"})),
            note: Some("it froze, my password is hunter22".into()),
            login: Some("quinn".into()),
            files: vec![FileText {
                name: "dc-client.out".into(),
                text: format!(r"log C:\Users\{}\x --password hunter22 from 81.2.69.160", "Quinn"),
            }],
            ..Parts::default()
        }
    }

    #[test]
    fn the_packed_report_is_redacted_everywhere() {
        let mut r = Redactor::new(vec![]);
        r.add_name("quinn", USER);
        let gz = pack(&parts(), &r);
        let v = unpack(&gz).unwrap();
        let text = v.to_string();
        assert_eq!(v["format"], FORMAT);
        assert_eq!(v["kind"], "link_lost");
        assert_eq!(v["summary"], "The match link was lost (idle)");
        assert!(!text.contains("hunter22"), "{text}");
        assert!(!text.contains("81.2.69.160"), "{text}");
        assert!(!text.contains(r"\Quinn"), "{text}");
        assert!(text.contains("%USER%"), "{text}");
        // the login is the one thing about the player that stays (the owner needs to know who)
        assert_eq!(v["login"], "quinn");
    }

    #[test]
    fn a_large_report_is_cut_to_the_cap() {
        let mut p = parts();
        // incompressible text, 3 MiB
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let noise: String = (0..3 << 20)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (b'!' + (x % 90) as u8) as char
            })
            .collect();
        p.files.push(FileText {
            name: "dc-client.net.jsonl".into(),
            text: noise,
        });
        let gz = pack(&p, &Redactor::new(vec![]));
        assert!(gz.len() <= MAX_COMPRESSED, "{}", gz.len());
        let v = unpack(&gz).unwrap();
        let files = v["files"].as_array().unwrap();
        assert!(files.iter().any(|f| f["name"] == "dc-client.out"));
    }

    #[test]
    fn tails_start_at_a_line() {
        let dir = std::env::temp_dir().join(format!("dc-diag-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.out");
        std::fs::write(&p, "first line\nsecond line\nthird\n").unwrap();
        assert_eq!(tail(&p, 1000).unwrap(), "first line\nsecond line\nthird\n");
        let t = tail(&p, 14).unwrap();
        assert!(t.ends_with("third\n") && !t.contains("second"), "{t}");
        assert!(t.starts_with("[first"), "{t}");
        std::fs::create_dir_all(dir.join("stall_1_1")).unwrap();
        std::fs::write(dir.join("stall_1_1/stall.json"), "{}").unwrap();
        std::fs::write(dir.join("stall_1_1/dump1.dmp"), "MDMP").unwrap();
        let f = artifact_files(&dir.join("stall_1_1"));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0, "stall_1_1/stall.json");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
