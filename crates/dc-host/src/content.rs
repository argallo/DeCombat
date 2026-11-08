//! The weapon content of a featured entry on this machine (HOST_AGENT.md section 3, DC_DEFAULTS.md LG-431): the
//! lobby sends the document as `content <hash> <base64>` lines before the `start` that needs it; the agent collects
//! them in memory and, at that `start`, checks that the bytes hash to the name, writes them to
//! `<log_dir>/content/<hash>.json` (a temporary file renamed, so a file of that name is always complete) and
//! passes that file as `-content`. The files stay across starts and restarts; after every `ok` the agent lists them
//! as `have <hash>`.
// Provenance: ORIGINAL (our own protocol)

use crate::packs::{b64_decode, sha_ok};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// The most bytes one content stream may hold. A set of 65 packs is about 25 KB; this stops a stream early enough that
/// memory is not filled first.
// DC-DEFAULT: LG-431 (proposed) 4 MiB
pub const MAX_CONTENT_BYTES: usize = 4 * 1024 * 1024;

/// The stream in progress.
struct Open {
    hash: String,
    bytes: Vec<u8>,
    /// `Some` once it broke (how); the rest of the stream is ignored.
    why: Option<String>,
}

/// The content folder of this agent.
pub struct Contents {
    root: PathBuf,
    open: Mutex<Option<Open>>,
}

impl Contents {
    /// `<log_dir>/content`: inside the log folder, which the agent's user owns (a folder beside it may be root's, as
    /// `/opt/decombat` is). The setup scripts make it; the agent makes it on demand and names the path if it cannot.
    pub fn new(log_dir: &Path) -> Contents {
        let root = log_dir.join("content");
        Contents {
            root: std::path::absolute(&root).unwrap_or(root),
            open: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Open>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn file(&self, hash: &str) -> PathBuf {
        self.root.join(format!("{hash}.json"))
    }

    /// A new control connection: a stream cut off by the old one starts over.
    pub fn reset(&self) {
        self.lock().take();
    }

    /// The complete documents here (`have <hash>`), in name order. A file is trusted by its name only here; `path`
    /// checks the bytes again before a game server gets it.
    pub fn cached(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let names: BTreeSet<String> = rd
            .flatten()
            .filter(|e| e.path().is_file())
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.strip_suffix(".json").map(str::to_string)
            })
            .filter(|n| sha_ok(n))
            .collect();
        names.into_iter().collect()
    }

    /// One `content <hash> <base64>` line. `Err` at the first fault of a stream only (the caller logs it): its later
    /// lines are ignored, and the `start` that follows answers `failed` with the same reason.
    pub fn chunk(&self, hash: &str, b64: &str) -> Result<(), String> {
        if !sha_ok(hash) {
            return Err(format!("content line with a bad name {hash:?}"));
        }
        if self.file(hash).is_file() {
            return Ok(());
        }
        let mut guard = self.lock();
        if guard.as_ref().is_none_or(|o| o.hash != hash) {
            *guard = Some(Open {
                hash: hash.to_string(),
                bytes: Vec::new(),
                why: None,
            });
        }
        let o = guard.as_mut().expect("just set");
        if o.why.is_some() {
            return Ok(());
        }
        let put = match b64_decode(b64) {
            None => Err("a content line is not base64".to_string()),
            Some(d) if o.bytes.len() + d.len() > MAX_CONTENT_BYTES => Err(format!(
                "over the {MAX_CONTENT_BYTES} byte cap of a content document"
            )),
            Some(d) => {
                o.bytes.extend_from_slice(&d);
                Ok(())
            }
        };
        if let Err(e) = put {
            o.bytes = Vec::new();
            o.why = Some(e.clone());
            return Err(e);
        }
        Ok(())
    }

    /// The absolute path of document `hash`, finishing the stream that brought it when that is still open. `Err`
    /// says why the agent cannot give it; wrong bytes are never kept.
    pub fn path(&self, hash: &str) -> Result<PathBuf, String> {
        if !sha_ok(hash) {
            return Err("bad content hash".to_string());
        }
        let file = self.file(hash);
        if file.is_file() {
            match std::fs::read(&file) {
                Ok(b) if dc_rom::sha256_hex(&b) == hash => return Ok(file),
                // a file that no longer hashes to its name is dropped; the lobby sends it again
                Ok(_) | Err(_) => {
                    let _ = std::fs::remove_file(&file);
                }
            }
        }
        let mut guard = self.lock();
        let Some(o) = guard.take().filter(|o| o.hash == hash) else {
            return Err("this agent does not hold it".to_string());
        };
        drop(guard);
        if let Some(why) = o.why {
            return Err(why);
        }
        if dc_rom::sha256_hex(&o.bytes) != hash {
            return Err("the bytes do not hash to its name".to_string());
        }
        std::fs::create_dir_all(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let tmp = self.root.join(format!("{hash}.json.part"));
        std::fs::write(&tmp, &o.bytes)
            .and_then(|()| std::fs::rename(&tmp, &file))
            .map_err(|e| format!("{}: {e}", file.display()))?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-host-content-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("logs")).unwrap();
        d
    }

    fn b64(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in data.chunks(3) {
            let n = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            s.push(T[(n >> 18) as usize & 63] as char);
            s.push(T[(n >> 12) as usize & 63] as char);
            s.push(if c.len() > 1 {
                T[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            s.push(if c.len() > 2 {
                T[n as usize & 63] as char
            } else {
                '='
            });
        }
        s
    }

    /// A document well over one line, as the Vanilla++ set's is.
    fn doc() -> (Vec<u8>, String) {
        let rows: Vec<String> = (0..900)
            .map(|i| format!("{{\"weapon\":{i},\"op\":\"patch\"}}"))
            .collect();
        let text = format!(
            "{{\"version\":1,\"weapons\":[{}],\"ammo\":[]}}",
            rows.join(",")
        );
        let bytes = text.into_bytes();
        assert!(bytes.len() > 25_000);
        let hash = dc_rom::sha256_hex(&bytes);
        (bytes, hash)
    }

    #[test]
    fn a_document_streams_verifies_and_stays_for_the_next_start() {
        let d = tmp("ok");
        let (bytes, hash) = doc();
        let c = Contents::new(&d.join("logs"));
        assert_eq!(
            c.root,
            std::path::absolute(d.join("logs").join("content")).unwrap()
        );
        assert!(c.cached().is_empty());
        for p in bytes.chunks(12_000) {
            let line = b64(p);
            // a line of the lobby's size stays under the agent's limit
            assert!("content ".len() + 64 + 1 + line.len() <= crate::proto::MAX_LINE);
            c.chunk(&hash, &line).unwrap();
        }
        let f = c.path(&hash).unwrap();
        assert!(f.is_absolute());
        assert_eq!(std::fs::read(&f).unwrap(), bytes);
        assert_eq!(c.cached(), std::slice::from_ref(&hash));
        assert!(!c.root.join(format!("{hash}.json.part")).exists());
        // a restarted agent: cached, a second stream of it is ignored, and the file is the same
        let again = Contents::new(&d.join("logs"));
        assert_eq!(again.cached(), std::slice::from_ref(&hash));
        again.chunk(&hash, &b64(b"junk")).unwrap();
        assert_eq!(again.path(&hash).unwrap(), f);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_folder_that_cannot_be_made_names_its_path() {
        let d = tmp("nodir");
        // the content folder's name taken by a file: the stand-in for a parent the agent may not write
        std::fs::write(d.join("logs").join("content"), b"in the way").unwrap();
        let (bytes, hash) = doc();
        let c = Contents::new(&d.join("logs"));
        c.chunk(&hash, &b64(&bytes)).unwrap();
        let e = c.path(&hash).unwrap_err();
        assert!(
            e.contains(&d.join("logs").join("content").display().to_string()),
            "{e}"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn wrong_bytes_a_missing_stream_and_a_corrupt_file_are_refused() {
        let d = tmp("bad");
        let (bytes, hash) = doc();
        let c = Contents::new(&d.join("logs"));
        // nothing here: the lobby is told, and sends it the next time
        assert!(c.path(&hash).unwrap_err().contains("does not hold"));
        // the bytes of another document under this name
        c.chunk(&hash, &b64(b"not the document")).unwrap();
        assert!(c.path(&hash).unwrap_err().contains("hash"));
        assert!(c.cached().is_empty(), "wrong bytes are not kept");
        // a truncated stream is wrong bytes too
        c.chunk(&hash, &b64(&bytes[..12_000])).unwrap();
        assert!(c.path(&hash).unwrap_err().contains("hash"));
        // a bad name, bad base64
        assert!(c.chunk("../x", "Zm9v").is_err());
        assert!(c.path("../x").is_err());
        assert!(c.chunk(&hash, "Zm9v!").is_err());
        assert!(c.path(&hash).unwrap_err().contains("base64"));
        // a stream of another name replaces the one before it
        let other = "ab".repeat(32);
        c.chunk(&hash, &b64(&bytes[..3])).unwrap();
        c.chunk(&other, &b64(b"x")).unwrap();
        assert!(c.path(&hash).unwrap_err().contains("does not hold"));
        // a file on disk that no longer hashes to its name is dropped, not served
        std::fs::create_dir_all(&c.root).unwrap();
        std::fs::write(c.file(&hash), b"tampered").unwrap();
        assert!(c.path(&hash).is_err());
        assert!(!c.file(&hash).exists());
        // a new connection forgets a half stream
        c.chunk(&hash, &b64(&bytes[..12_000])).unwrap();
        c.reset();
        assert!(c.path(&hash).unwrap_err().contains("does not hold"));
        std::fs::remove_dir_all(&d).ok();
    }
}
