//! The map packs of a featured entry on this machine (HOST_AGENT.md section 3, DC_DEFAULTS.md MAP-84): the lobby sends
//! the zip as `pack <sha> <base64>` lines before the `start` that needs it; the agent appends them to
//! `<log_dir>/../packs/<sha>.zip.part`, and at that `start` checks size and SHA-256, renames the file to `<sha>.zip` and
//! unpacks it into `<sha>/` (built beside it, then renamed, so a folder of that name is always complete). The folders
//! stay across starts and restarts; after every `ok` the agent lists them as `have <sha>`.
// Provenance: ORIGINAL (our own protocol)

use crate::proto::WorldPack;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// The most bytes one pack may stream. `dc_modkit::read_stored_zip` refuses more than 32 MiB unpacked anyway; this
/// stops a stream early enough that the disk is not filled first.
// DC-DEFAULT: MAP-84 (proposed) 64 MiB
pub const MAX_PACK_BYTES: u64 = 64 * 1024 * 1024;

/// True for a pack name: 64 lower-case hex digits (the form `[[lobby.servers]] mods` requires).
pub fn sha_ok(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Standard base64 with optional padding; `None` for anything else.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let body = s.trim_end_matches('=');
    if s.len() - body.len() > 2 || body.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(body.len() / 4 * 3 + 2);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in body.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The stream in progress.
struct Open {
    sha: String,
    /// `None` once it broke (`why` says how) or was closed.
    file: Option<File>,
    written: u64,
    why: Option<String>,
}

/// The pack folder of this agent.
pub struct Packs {
    root: PathBuf,
    open: Mutex<Option<Open>>,
}

impl Packs {
    /// `<log_dir>/../packs`: beside the log folder, which the install gives the agent's user (tools/host/push.sh).
    pub fn new(log_dir: &Path) -> Packs {
        let parent = log_dir.parent().filter(|p| !p.as_os_str().is_empty());
        let root = parent.unwrap_or(log_dir).join("packs");
        Packs {
            root: std::path::absolute(&root).unwrap_or(root),
            open: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Open>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn folder(&self, sha: &str) -> PathBuf {
        self.root.join(sha)
    }

    fn zip(&self, sha: &str) -> PathBuf {
        self.root.join(format!("{sha}.zip"))
    }

    fn part(&self, sha: &str) -> PathBuf {
        self.root.join(format!("{sha}.zip.part"))
    }

    /// A new control connection: a stream cut off by the old one starts over.
    pub fn reset(&self) {
        if let Some(o) = self.lock().take() {
            let _ = std::fs::remove_file(self.part(&o.sha));
        }
    }

    /// The complete packs here (`have <sha>`), in name order.
    pub fn cached(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let names: BTreeSet<String> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| sha_ok(n))
            .collect();
        names.into_iter().collect()
    }

    /// One `pack <sha> <base64>` line. `Err` at the first fault of a stream only (the caller logs it): its later lines
    /// are ignored, and the `start` that follows answers `failed` with the same reason.
    pub fn chunk(&self, sha: &str, b64: &str) -> Result<(), String> {
        if !sha_ok(sha) {
            return Err(format!("pack line with a bad name {sha:?}"));
        }
        if self.folder(sha).is_dir() {
            return Ok(());
        }
        let mut guard = self.lock();
        if guard.as_ref().is_none_or(|o| o.sha != sha) {
            if let Some(o) = guard.take() {
                let _ = std::fs::remove_file(self.part(&o.sha));
            }
            let opened = std::fs::create_dir_all(&self.root)
                .and_then(|()| File::create(self.part(sha)))
                .map_err(|e| format!("{}: {e}", self.part(sha).display()));
            let (file, why) = match opened {
                Ok(f) => (Some(f), None),
                Err(e) => (None, Some(e)),
            };
            let first = why.clone();
            *guard = Some(Open {
                sha: sha.to_string(),
                file,
                written: 0,
                why,
            });
            if let Some(e) = first {
                return Err(e);
            }
        }
        let o = guard.as_mut().expect("just set");
        if o.why.is_some() {
            return Ok(());
        }
        let Some(file) = o.file.as_mut() else {
            return Ok(());
        };
        let put = match b64_decode(b64) {
            None => Err("a pack line is not base64".to_string()),
            Some(d) if o.written + d.len() as u64 > MAX_PACK_BYTES => {
                Err(format!("over the {MAX_PACK_BYTES} byte cap of a pack"))
            }
            Some(d) => file
                .write_all(&d)
                .map(|()| o.written += d.len() as u64)
                .map_err(|e| format!("{}: {e}", self.part(sha).display())),
        };
        if let Err(e) = put {
            o.file = None;
            o.why = Some(e.clone());
            let _ = std::fs::remove_file(self.part(sha));
            return Err(e);
        }
        Ok(())
    }

    /// The absolute path of `world_file` in pack `p`, finishing the stream that brought it when that is still open.
    pub fn world(&self, p: &WorldPack, world_file: &str) -> Result<PathBuf, String> {
        if !self.folder(&p.sha256).is_dir() {
            self.finish(p)?;
        }
        let file = world_file
            .split('/')
            .fold(self.folder(&p.sha256), |d, part| d.join(part));
        if !file.is_file() {
            return Err(format!("{world_file} is not in the pack"));
        }
        Ok(file)
    }

    /// The stream (or a `<sha>.zip` left from before) becomes the folder.
    fn finish(&self, p: &WorldPack) -> Result<(), String> {
        let sha = &p.sha256;
        let mut guard = self.lock();
        if guard.as_ref().is_some_and(|o| o.sha == *sha) {
            let o = guard.take().expect("checked");
            if let Some(why) = o.why {
                return Err(why);
            }
            if let Some(mut f) = o.file {
                f.flush()
                    .map_err(|e| format!("{}: {e}", self.part(sha).display()))?;
            }
            std::fs::rename(self.part(sha), self.zip(sha))
                .map_err(|e| format!("{}: {e}", self.zip(sha).display()))?;
        }
        drop(guard);
        let zip = self.zip(sha);
        if !zip.is_file() {
            return Err("this agent does not hold it".to_string());
        }
        let checked = std::fs::read(&zip)
            .map_err(|e| format!("{}: {e}", zip.display()))
            .and_then(|bytes| {
                if bytes.len() as u64 != p.size {
                    return Err(format!(
                        "{} bytes, the request says {}",
                        bytes.len(),
                        p.size
                    ));
                }
                if dc_rom::sha256_hex(&bytes) != *sha {
                    return Err("the bytes do not hash to its name".to_string());
                }
                dc_modkit::read_stored_zip(&bytes)
            });
        let files = match checked {
            Ok(f) => f,
            Err(e) => {
                // never kept: the next start asks for it again
                let _ = std::fs::remove_file(&zip);
                return Err(e);
            }
        };
        let tmp = self.root.join(format!("{sha}.unpack"));
        let _ = std::fs::remove_dir_all(&tmp);
        for (name, data) in files {
            // read_stored_zip refuses absolute paths, `..`, backslashes and drive letters
            let out = tmp.join(&name);
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&out, data).map_err(|e| format!("{}: {e}", out.display()))?;
        }
        std::fs::rename(&tmp, self.folder(sha))
            .map_err(|e| format!("{}: {e}", self.folder(sha).display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-host-packs-{}-{tag}", std::process::id()));
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

    /// A real stored zip of a mod folder, from the pack tool the lobby's operator uses.
    fn zip_bytes(dir: &Path) -> (Vec<u8>, String) {
        let src = dir.join("src").join("m");
        std::fs::create_dir_all(src.join("assets/maps")).unwrap();
        std::fs::create_dir_all(src.join("screenshots")).unwrap();
        std::fs::write(
            src.join("manifest.json"),
            r#"{"id": "m", "name": "m", "version": "1.0.0", "entry": "main.lua", "kind": "shared",
               "description": "A test fixture pack for the unit tests.", "author": "tests",
               "screenshots": ["screenshots/main.png"], "capabilities": ["content.maps", "assets"]}"#,
        )
        .unwrap();
        std::fs::write(src.join("main.lua"), "return function(mod) end").unwrap();
        std::fs::write(
            src.join("screenshots/main.png"),
            dc_modkit::placeholder_png(),
        )
        .unwrap();
        std::fs::write(
            src.join("assets/maps/m.dat"),
            dc_modkit::placeholder_world(),
        )
        .unwrap();
        let (bytes, e) = dc_modkit::pack_mod_bytes(&src).unwrap();
        (bytes, e.sha256)
    }

    fn request(sha: &str, size: usize) -> WorldPack {
        WorldPack {
            sha256: sha.to_string(),
            size: size as u64,
            url: String::new(),
        }
    }

    #[test]
    fn base64_reads_the_rfc_vectors_and_refuses_the_rest() {
        for (t, want) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9vYmE", "fooba"),
        ] {
            assert_eq!(b64_decode(t).unwrap(), want.as_bytes(), "{t}");
        }
        assert!(b64_decode("Z").is_none());
        assert!(b64_decode("Zm9v!").is_none());
        assert!(b64_decode("Zg===").is_none());
    }

    #[test]
    fn a_pack_streams_verifies_unpacks_and_stays_for_the_next_start() {
        let d = tmp("ok");
        let (bytes, sha) = zip_bytes(&d);
        let packs = Packs::new(&d.join("logs"));
        assert_eq!(packs.root, std::path::absolute(d.join("packs")).unwrap());
        assert!(packs.cached().is_empty());
        for c in bytes.chunks(12_000) {
            packs.chunk(&sha, &b64(c)).unwrap();
        }
        let w = packs
            .world(&request(&sha, bytes.len()), "assets/maps/m.dat")
            .unwrap();
        assert!(w.is_absolute(), "{}", w.display());
        assert_eq!(
            w,
            packs
                .root
                .join(&sha)
                .join("assets")
                .join("maps")
                .join("m.dat")
        );
        assert_eq!(std::fs::read(&w).unwrap(), dc_modkit::placeholder_world());
        assert!(packs.root.join(format!("{sha}.zip")).is_file());
        assert!(!packs.root.join(format!("{sha}.zip.part")).exists());
        assert_eq!(packs.cached(), std::slice::from_ref(&sha));
        // a restarted agent: cached, and a second stream of it writes nothing
        let again = Packs::new(&d.join("logs"));
        assert_eq!(again.cached(), std::slice::from_ref(&sha));
        again.chunk(&sha, &b64(b"junk")).unwrap();
        assert!(!again.root.join(format!("{sha}.zip.part")).exists());
        assert_eq!(
            again
                .world(&request(&sha, bytes.len()), "assets/maps/m.dat")
                .unwrap(),
            w
        );
        assert!(again
            .world(&request(&sha, bytes.len()), "assets/maps/none.dat")
            .is_err());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn wrong_bytes_are_refused_and_not_kept() {
        let d = tmp("bad");
        let (bytes, sha) = zip_bytes(&d);
        let packs = Packs::new(&d.join("logs"));
        // the bytes of another file under this name
        packs.chunk(&sha, &b64(b"not the pack")).unwrap();
        let e = packs
            .world(&request(&sha, 12), "assets/maps/m.dat")
            .unwrap_err();
        assert!(e.contains("hash"), "{e}");
        assert!(packs.cached().is_empty());
        assert!(!packs.root.join(format!("{sha}.zip")).exists());
        // the size the request names is checked too
        packs.chunk(&sha, &b64(&bytes)).unwrap();
        let e = packs
            .world(&request(&sha, bytes.len() + 1), "assets/maps/m.dat")
            .unwrap_err();
        assert!(e.contains("bytes"), "{e}");
        assert!(packs.cached().is_empty());
        // nothing here at all: the lobby is told, and sends it the next time
        let e = packs
            .world(&request(&sha, bytes.len()), "assets/maps/m.dat")
            .unwrap_err();
        assert!(e.contains("does not hold"), "{e}");
        // a good stream after that one works
        packs.chunk(&sha, &b64(&bytes)).unwrap();
        packs
            .world(&request(&sha, bytes.len()), "assets/maps/m.dat")
            .unwrap();
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_broken_or_cut_off_stream_starts_over() {
        let d = tmp("cut");
        let (bytes, sha) = zip_bytes(&d);
        let packs = Packs::new(&d.join("logs"));
        let half = bytes.len() / 2;
        // cut off by a reconnect: the part is gone, the full stream then is the whole file
        packs.chunk(&sha, &b64(&bytes[..half - half % 3])).unwrap();
        packs.reset();
        assert!(!packs.root.join(format!("{sha}.zip.part")).exists());
        packs.chunk(&sha, &b64(&bytes)).unwrap();
        packs
            .world(&request(&sha, bytes.len()), "assets/maps/m.dat")
            .unwrap();
        // a line that is no base64: one Err, the rest ignored, the start names the reason
        let other = "ab".repeat(32);
        assert!(packs.chunk(&other, "!!!").is_err());
        packs.chunk(&other, "Zm9v").unwrap();
        let e = packs.world(&request(&other, 3), "x.dat").unwrap_err();
        assert!(e.contains("base64"), "{e}");
        assert!(packs.chunk("../evil", "Zm9v").is_err());
        std::fs::remove_dir_all(&d).ok();
    }
}
