//! A server build the lobby offers this agent (specs/HOST_AGENT.md "Updates", DC_DEFAULTS.md HA-2): the lobby says
//! `update <commit> <sha256> <size> <tarball>`; an idle agent that does not hold it answers `getupdate <commit>`, gets
//! the tarball as `updpart <commit> <base64>` lines and `updend <commit>`, checks size and SHA-256 and leaves it in
//! `<log_dir>/../incoming/` with `desired.txt` (`commit=`, `tarball=`, `sha256=`, `size=`, `seq=`, `sig=`), the pointer
//! form `tools/host/update.sh` reads with `SOURCE=lobby`. The offer carries root's sequence number and signature from
//! the VPS (`update <commit> <sha256> <size> <tarball> <seq> <sig>`), passed on unchanged: neither the lobby nor this
//! agent can make them, and the updater (root, its own timer) trusts nothing before the signature verifies against
//! the key it pinned. The agent installs nothing itself.
// Provenance: ORIGINAL (our own protocol)

use crate::packs::b64_decode;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// The largest tarball the agent takes.
// DC-DEFAULT: HA-2 (proposed) 256 MiB
pub const MAX_UPDATE_BYTES: u64 = 256 * 1024 * 1024;

/// One offer of the lobby, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub commit: String,
    pub sha256: String,
    pub size: u64,
    pub tarball: String,
    /// Root's sequence number and signature (base64), relayed to update.sh unchanged.
    pub seq: u64,
    pub sig: String,
}

fn hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The tarball names `tools/host/update.sh` takes: `decombat-servers-linux-x86_64-<[0-9A-Za-z._-]+>.tar.gz`.
pub fn tarball_ok(s: &str) -> bool {
    s.strip_prefix("decombat-servers-linux-x86_64-")
        .and_then(|r| r.strip_suffix(".tar.gz"))
        .is_some_and(|m| {
            !m.is_empty()
                && !m.starts_with('.')
                && m.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

impl Offer {
    /// `<commit> <sha256> <size> <tarball> <seq> <sig>` (the words after `update`).
    pub fn parse(rest: &str) -> Option<Offer> {
        let w: Vec<&str> = rest.split_whitespace().collect();
        let [commit, sha, size, tarball, seq, sig] = w.as_slice() else {
            return None;
        };
        let size: u64 = size.parse().ok()?;
        let seq: u64 = seq.parse().ok()?;
        let sig_ok = (16..=8192).contains(&sig.len())
            && sig
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='));
        (hex(commit, 40)
            && hex(sha, 64)
            && size > 0
            && size <= MAX_UPDATE_BYTES
            && tarball_ok(tarball)
            && sig_ok)
            .then(|| Offer {
                commit: commit.to_string(),
                sha256: sha.to_string(),
                size,
                tarball: tarball.to_string(),
                seq,
                sig: sig.to_string(),
            })
    }

    fn pointer(&self) -> String {
        format!(
            "commit={}\ntarball={}\nsha256={}\nsize={}\nseq={}\nsig={}\n",
            self.commit, self.tarball, self.sha256, self.size, self.seq, self.sig
        )
    }
}

/// The transfer in progress.
struct Open {
    offer: Offer,
    file: Option<File>,
    hash: dc_rom::Sha256,
    written: u64,
    why: Option<String>,
}

/// What [`Incoming::offer`] decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Ask for it: the line to send.
    Fetch(String),
    /// Held here already (complete, checked).
    Have,
    /// A transfer of it is under way.
    Busy,
    /// A game server runs: not now (the lobby offers again).
    Matches,
    /// A malformed offer, or the incoming folder cannot be written.
    Refused(String),
}

/// The incoming folder of this agent.
pub struct Incoming {
    root: PathBuf,
    open: Mutex<Option<Open>>,
}

impl Incoming {
    /// `<log_dir>/../incoming`, beside `packs/`.
    pub fn new(log_dir: &Path) -> Incoming {
        let parent = log_dir.parent().filter(|p| !p.as_os_str().is_empty());
        let root = parent.unwrap_or(log_dir).join("incoming");
        Incoming {
            root: std::path::absolute(&root).unwrap_or(root),
            open: Mutex::new(None),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn lock(&self) -> MutexGuard<'_, Option<Open>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn part(&self, o: &Offer) -> PathBuf {
        self.root.join(format!(".{}.part", o.tarball))
    }

    /// A new control connection: a transfer the old one cut off starts over.
    pub fn reset(&self) {
        if let Some(o) = self.lock().take() {
            let _ = std::fs::remove_file(self.part(&o.offer));
        }
    }

    /// True when `desired.txt` names this offer and its tarball is there with the offered size.
    fn holds(&self, o: &Offer) -> bool {
        std::fs::read_to_string(self.root.join("desired.txt")).is_ok_and(|t| t == o.pointer())
            && std::fs::metadata(self.root.join(&o.tarball))
                .is_ok_and(|m| m.is_file() && m.len() == o.size)
    }

    /// One `update ...` line. `busy`: a game server runs here.
    pub fn offer(&self, rest: &str, busy: bool) -> Decision {
        let Some(o) = Offer::parse(rest) else {
            return Decision::Refused(format!("a malformed update offer {rest:?}"));
        };
        if self.holds(&o) {
            return Decision::Have;
        }
        let mut guard = self.lock();
        if guard
            .as_ref()
            .is_some_and(|x| x.offer == o && x.why.is_none())
        {
            return Decision::Busy;
        }
        if busy {
            return Decision::Matches;
        }
        if let Some(x) = guard.take() {
            let _ = std::fs::remove_file(self.part(&x.offer));
        }
        let file = std::fs::create_dir_all(&self.root)
            .and_then(|()| File::create(self.part(&o)))
            .map_err(|e| format!("{}: {e}", self.part(&o).display()));
        let file = match file {
            Ok(f) => f,
            Err(e) => return Decision::Refused(e),
        };
        let line = format!("getupdate {}", o.commit);
        *guard = Some(Open {
            offer: o,
            file: Some(file),
            hash: dc_rom::Sha256::new(),
            written: 0,
            why: None,
        });
        Decision::Fetch(line)
    }

    /// One `updpart <commit> <base64>` line. `Err` at the first fault of a transfer only (its later lines are ignored).
    pub fn part_line(&self, commit: &str, b64: &str) -> Result<(), String> {
        let mut guard = self.lock();
        let Some(o) = guard.as_mut().filter(|o| o.offer.commit == commit) else {
            return Ok(());
        };
        if o.why.is_some() {
            return Ok(());
        }
        let put = match b64_decode(b64) {
            None => Err("an update line is not base64".to_string()),
            Some(d) if o.written + d.len() as u64 > o.offer.size => {
                Err(format!("more than the offered {} bytes", o.offer.size))
            }
            Some(d) => match o.file.as_mut() {
                Some(f) => f
                    .write_all(&d)
                    .map(|()| {
                        o.hash.update(&d);
                        o.written += d.len() as u64;
                    })
                    .map_err(|e| format!("write: {e}")),
                None => Ok(()),
            },
        };
        if let Err(e) = put {
            o.file = None;
            o.why = Some(e.clone());
            let _ = std::fs::remove_file(self.part(&o.offer));
            return Err(e);
        }
        Ok(())
    }

    /// `updend <commit>`: the checked tarball and `desired.txt`; earlier tarballs here go.
    pub fn end(&self, commit: &str) -> Result<Offer, String> {
        let o = {
            let mut guard = self.lock();
            match guard.take() {
                Some(o) if o.offer.commit == commit => o,
                other => {
                    *guard = other;
                    return Err(format!("no transfer of {commit} under way"));
                }
            }
        };
        let part = self.part(&o.offer);
        let checked = (|| {
            if let Some(why) = o.why {
                return Err(why);
            }
            let mut f = o.file.ok_or("the transfer broke")?;
            f.flush().map_err(|e| format!("write: {e}"))?;
            drop(f);
            if o.written != o.offer.size {
                return Err(format!(
                    "{} bytes, the offer says {}",
                    o.written, o.offer.size
                ));
            }
            let got: String = o
                .hash
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if got != o.offer.sha256 {
                return Err("the bytes do not hash to the offered SHA-256".to_string());
            }
            Ok(())
        })();
        if let Err(e) = checked {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
        let dest = self.root.join(&o.offer.tarball);
        std::fs::rename(&part, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        if let Ok(rd) = std::fs::read_dir(&self.root) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n != o.offer.tarball && tarball_ok(&n) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let tmp = self.root.join(".desired.txt.tmp");
        std::fs::write(&tmp, o.offer.pointer())
            .and_then(|()| std::fs::rename(&tmp, self.root.join("desired.txt")))
            .map_err(|e| format!("desired.txt: {e}"))?;
        Ok(o.offer)
    }

    /// `updateno <commit> <reason>`: the lobby will not send it; the transfer is dropped.
    pub fn refused(&self, commit: &str) {
        let mut guard = self.lock();
        if guard.as_ref().is_some_and(|o| o.offer.commit == commit) {
            if let Some(o) = guard.take() {
                let _ = std::fs::remove_file(self.part(&o.offer));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: &str = "2222222222222222222222222222222222222222";
    /// The words root's deploy adds: a sequence number and the base64 of a signature (its shape only).
    const T: &str = " 7 LS0tLS1CRUdJTiBTU0ggU0lHTkFUVVJFLS0tLS0K";
    const SIG: &str = "LS0tLS1CRUdJTiBTU0ggU0lHTkFUVVJFLS0tLS0K";

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-host-update-{}-{tag}", std::process::id()));
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

    fn offer_of(data: &[u8], name: &str) -> String {
        format!("{C} {} {} {name}{T}", dc_rom::sha256_hex(data), data.len())
    }

    #[test]
    fn offers_are_checked() {
        let sha = "a".repeat(64);
        assert!(Offer::parse(&format!(
            "{C} {sha} 10 decombat-servers-linux-x86_64-22222222.tar.gz{T}"
        ))
        .is_some());
        for bad in [
            format!("{C} {sha} 10 ../decombat-servers-linux-x86_64-x.tar.gz{T}"),
            format!("{C} {sha} 10 decombat-servers-linux-x86_64-.tar.gz{T}"),
            format!("{C} {sha} 10 decombat-servers-linux-x86_64-a/b.tar.gz{T}"),
            format!("{C} {sha} 10 other.tar.gz{T}"),
            format!(
                "{} {sha} 10 decombat-servers-linux-x86_64-x.tar.gz{T}",
                &C[..8]
            ),
            format!(
                "{C} {} 10 decombat-servers-linux-x86_64-x.tar.gz{T}",
                sha.to_uppercase()
            ),
            format!("{C} {sha} 0 decombat-servers-linux-x86_64-x.tar.gz{T}"),
            format!(
                "{C} {sha} {} decombat-servers-linux-x86_64-x.tar.gz{T}",
                MAX_UPDATE_BYTES + 1
            ),
            format!("{C} {sha} 10"),
            // HA-2: root's sequence number and signature are required and checked for shape
            format!("{C} {sha} 10 decombat-servers-linux-x86_64-x.tar.gz"),
            format!("{C} {sha} 10 decombat-servers-linux-x86_64-x.tar.gz x {SIG}"),
            format!("{C} {sha} 10 decombat-servers-linux-x86_64-x.tar.gz 7 short"),
            format!("{C} {sha} 10 decombat-servers-linux-x86_64-x.tar.gz 7 {SIG}!"),
        ] {
            assert!(Offer::parse(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_build_streams_checks_and_leaves_the_pointer_update_sh_reads() {
        let d = tmp("good");
        let inc = Incoming::new(&d.join("logs"));
        let data: Vec<u8> = (0..30_000u32).map(|i| (i * 7) as u8).collect();
        let name = "decombat-servers-linux-x86_64-22222222.tar.gz";
        let rest = offer_of(&data, name);
        assert_eq!(
            inc.offer(&rest, true),
            Decision::Matches,
            "not while a match runs"
        );
        assert_eq!(
            inc.offer(&rest, false),
            Decision::Fetch(format!("getupdate {C}"))
        );
        assert_eq!(inc.offer(&rest, false), Decision::Busy);
        // an older build left from before goes once the new one is complete
        std::fs::write(
            inc.root()
                .join("decombat-servers-linux-x86_64-11111111.tar.gz"),
            b"old",
        )
        .unwrap();
        for c in data.chunks(12_000) {
            inc.part_line(C, &b64(c)).unwrap();
        }
        let got = inc.end(C).unwrap();
        assert_eq!(got.tarball, name);
        assert_eq!(std::fs::read(inc.root().join(name)).unwrap(), data);
        let ptr = std::fs::read_to_string(inc.root().join("desired.txt")).unwrap();
        assert!(
            ptr.starts_with(&format!(
                "commit={C}\ntarball={name}\nsha256={}\n",
                dc_rom::sha256_hex(&data)
            )),
            "{ptr}"
        );
        // root's sequence number and signature go on to update.sh unchanged
        assert!(ptr.ends_with(&format!("seq=7\nsig={SIG}\n")), "{ptr}");
        assert!(!inc
            .root()
            .join("decombat-servers-linux-x86_64-11111111.tar.gz")
            .exists());
        assert!(!inc.root().join(format!(".{name}.part")).exists());
        // the next offer of the same build: held, nothing asked
        assert_eq!(inc.offer(&rest, false), Decision::Have);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn wrong_or_extra_bytes_are_refused_and_nothing_is_kept() {
        let d = tmp("bad");
        let inc = Incoming::new(&d.join("logs"));
        let data = vec![5u8; 9_000];
        let name = "decombat-servers-linux-x86_64-22222222.tar.gz";
        // the offer names other bytes
        let rest = format!("{C} {} {} {name}{T}", "b".repeat(64), data.len());
        assert!(matches!(inc.offer(&rest, false), Decision::Fetch(_)));
        inc.part_line(C, &b64(&data)).unwrap();
        assert!(inc.end(C).unwrap_err().contains("hash"));
        assert!(!inc.root().join(name).exists() && !inc.root().join("desired.txt").exists());
        // more than offered
        let rest = format!("{C} {} 10 {name}{T}", dc_rom::sha256_hex(&data[..10]));
        assert!(matches!(inc.offer(&rest, false), Decision::Fetch(_)));
        assert!(inc
            .part_line(C, &b64(&data))
            .unwrap_err()
            .contains("more than"));
        assert!(inc.end(C).is_err());
        // not base64; a cut-off transfer; an end without a transfer
        let rest = offer_of(&data, name);
        assert!(matches!(inc.offer(&rest, false), Decision::Fetch(_)));
        assert!(inc.part_line(C, "!!!").is_err());
        assert!(inc.end(C).is_err());
        assert!(matches!(inc.offer(&rest, false), Decision::Fetch(_)));
        inc.part_line(C, &b64(&data[..3000])).unwrap();
        inc.reset();
        assert!(inc.end(C).is_err());
        assert!(matches!(inc.offer(&rest, false), Decision::Fetch(_)));
        inc.refused(C);
        assert!(inc.end(C).is_err());
        assert!(!inc.root().join(name).exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
