//! Reports waiting to be sent (CLI-1842): `<profile>/diag_queue/<created>_<id>.dcdiag` (the packed report) with
//! `<created>_<id>.target` (the lobby web port it goes to). A report whose upload fails stays until the next try;
//! at most [`MAX_ENTRIES`] / [`MAX_BYTES`] wait (the oldest goes first) and none older than [`MAX_AGE_SECS`].
// DC-DEFAULT: CLI-1842 (proposed, USER REQUEST 2026-11-02) the queue's caps

use std::path::{Path, PathBuf};

pub const DIR: &str = "diag_queue";
pub const MAX_ENTRIES: usize = 10;
pub const MAX_BYTES: u64 = 5 << 20;
pub const MAX_AGE_SECS: i64 = 14 * 86_400;

/// One waiting report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub created: i64,
    pub id: String,
    pub target: String,
    pub size: u64,
}

impl Entry {
    pub fn bytes(&self) -> Option<Vec<u8>> {
        std::fs::read(&self.path).ok()
    }

    /// Removes the report and its target file.
    pub fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.path.with_extension("target"));
    }
}

/// The queue folder under `profile`.
pub fn dir(profile: &Path) -> PathBuf {
    profile.join(DIR)
}

/// The waiting reports, oldest first.
pub fn list(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<Entry> = rd
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("dcdiag") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_string();
            let (created, id) = stem.split_once('_')?;
            let created = created.parse().ok()?;
            let target = std::fs::read_to_string(path.with_extension("target"))
                .unwrap_or_default()
                .trim()
                .to_string();
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            Some(Entry {
                path,
                created,
                id: id.to_string(),
                target,
                size,
            })
        })
        .collect();
    v.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
    v
}

/// Drops the reports over the age, then the oldest while over the caps; returns how many went.
pub fn trim(dir: &Path, now: i64) -> usize {
    let mut v = list(dir);
    let mut gone = 0;
    v.retain(|e| {
        let old = now - e.created > MAX_AGE_SECS;
        if old {
            e.remove();
            gone += 1;
        }
        !old
    });
    while v.len() > MAX_ENTRIES || v.iter().map(|e| e.size).sum::<u64>() > MAX_BYTES {
        v.remove(0).remove();
        gone += 1;
    }
    gone
}

/// Adds a packed report for `target`; the caps then hold (the new one stays unless it alone is over them).
pub fn push(dir: &Path, created: i64, id: &str, target: &str, packed: &[u8], now: i64) -> std::io::Result<Entry> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{created}_{id}.dcdiag"));
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, packed)?;
    std::fs::write(path.with_extension("target"), target)?;
    std::fs::rename(&tmp, &path)?;
    trim(dir, now);
    Ok(Entry {
        path,
        created,
        id: id.to_string(),
        target: target.to_string(),
        size: packed.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-diag-queue-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn reports_wait_in_order_and_the_caps_drop_the_oldest() {
        let d = tmp("caps");
        let now = 2_000_000_000;
        for i in 0..12 {
            push(&d, now - 100 + i, &format!("{i:016x}"), "1.2.3.4:18080", b"x", now).unwrap();
        }
        let v = list(&d);
        assert_eq!(v.len(), MAX_ENTRIES);
        assert_eq!(v[0].id, format!("{:016x}", 2));
        assert_eq!(v[0].target, "1.2.3.4:18080");
        // a big one pushes the others out
        let big = vec![0u8; (MAX_BYTES - 3) as usize];
        push(&d, now, "ffffffffffffffff", "t", &big, now).unwrap();
        let v = list(&d);
        assert_eq!(v.last().unwrap().id, "ffffffffffffffff");
        assert!(v.iter().map(|e| e.size).sum::<u64>() <= MAX_BYTES);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn old_reports_are_dropped_and_a_sent_one_is_removed() {
        let d = tmp("age");
        let now = 2_000_000_000;
        push(&d, now - MAX_AGE_SECS - 10, "0000000000000001", "t", b"a", now - MAX_AGE_SECS - 10).unwrap();
        let e = push(&d, now, "0000000000000002", "t", b"b", now).unwrap();
        assert_eq!(list(&d).len(), 1);
        assert_eq!(e.bytes().unwrap(), b"b");
        e.remove();
        assert!(list(&d).is_empty());
        assert!(!d.join(format!("{now}_0000000000000002.target")).exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
