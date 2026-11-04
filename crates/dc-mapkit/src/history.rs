//! The shared undo (MAPKIT_PLAN.md 5.2 `snapshot` / `history` / `revert`; DC-DEFAULT: MAP-14): snapshots of
//! `map.json` + `terrain.r16` under `.mapkit/history/<n>/` with a `meta.json` (number, label, unix time). `apply`
//! and the GUI take one before every change, so the CLI, the GUI and the MCP server share one history. At most
//! [`KEEP`] entries are kept (the oldest go first); `revert` snapshots the current state before restoring.

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::project::{write_atomic, HISTORY_DIR, MAP_FILE, TERRAIN_FILE};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Entries kept per project.
pub const KEEP: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub n: u32,
    pub label: String,
    /// Unix seconds when the snapshot was taken.
    pub time: u64,
    /// Whether the entry holds a terrain file.
    pub terrain: bool,
}

fn history_dir(dir: &Path) -> PathBuf {
    let mut p = dir.to_path_buf();
    for seg in HISTORY_DIR.split('/') {
        p.push(seg);
    }
    p
}

fn io(path: &Path, what: &str, e: impl std::fmt::Display) -> Diagnostics {
    Diagnostics::one(Diagnostic::new(
        Code::HistoryIo,
        path.display().to_string(),
        format!("{what}: {e}"),
    ))
}

/// Every entry, oldest first.
pub fn list(dir: &Path) -> Result<Vec<Entry>, Diagnostics> {
    let hd = history_dir(dir);
    if !hd.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for e in fs::read_dir(&hd).map_err(|e| io(&hd, "read", e))? {
        let e = e.map_err(|e| io(&hd, "read", e))?;
        let meta = e.path().join("meta.json");
        let Ok(text) = fs::read_to_string(&meta) else {
            continue;
        };
        if let Ok(entry) = serde_json::from_str::<Entry>(&text) {
            out.push(entry);
        }
    }
    out.sort_by_key(|e| e.n);
    Ok(out)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Copy the current `map.json` (and `terrain.r16` when present) into a new entry and prune to [`KEEP`].
pub fn snapshot(dir: &Path, label: &str) -> Result<Entry, Diagnostics> {
    let map = dir.join(MAP_FILE);
    let map_bytes = fs::read(&map).map_err(|e| io(&map, "read", e))?;
    let terrain = dir.join(TERRAIN_FILE);
    let terrain_bytes = if terrain.exists() {
        Some(fs::read(&terrain).map_err(|e| io(&terrain, "read", e))?)
    } else {
        None
    };
    let entries = list(dir)?;
    let n = entries.last().map_or(1, |e| e.n + 1);
    let entry = Entry {
        n,
        label: label.to_string(),
        time: now(),
        terrain: terrain_bytes.is_some(),
    };
    let ed = history_dir(dir).join(n.to_string());
    fs::create_dir_all(&ed).map_err(|e| io(&ed, "create", e))?;
    write_atomic(&ed.join(MAP_FILE), &map_bytes).map_err(|e| io(&ed, "write", e))?;
    if let Some(t) = terrain_bytes {
        write_atomic(&ed.join(TERRAIN_FILE), &t).map_err(|e| io(&ed, "write", e))?;
    }
    let meta = serde_json::to_string_pretty(&entry).expect("entry serialises");
    write_atomic(&ed.join("meta.json"), meta.as_bytes()).map_err(|e| io(&ed, "write", e))?;
    // prune
    let mut all = entries;
    all.push(entry.clone());
    while all.len() > KEEP {
        let old = all.remove(0);
        let od = history_dir(dir).join(old.n.to_string());
        let _ = fs::remove_dir_all(od);
    }
    Ok(entry)
}

/// Restore entry `n` (after snapshotting the current state as `before revert to <n>`).
pub fn revert(dir: &Path, n: u32) -> Result<Entry, Diagnostics> {
    let entries = list(dir)?;
    let Some(entry) = entries.iter().find(|e| e.n == n).cloned() else {
        return Err(Diagnostics::one(
            Diagnostic::new(
                Code::HistoryMissing,
                format!("{HISTORY_DIR}/{n}"),
                format!("no history entry {n}"),
            )
            .with_fix("run `dc-mapkit history <dir>`"),
        ));
    };
    snapshot(dir, &format!("before revert to {n}"))?;
    let ed = history_dir(dir).join(n.to_string());
    let map = fs::read(ed.join(MAP_FILE)).map_err(|e| io(&ed, "read", e))?;
    write_atomic(&dir.join(MAP_FILE), &map).map_err(|e| io(dir, "write", e))?;
    let t = ed.join(TERRAIN_FILE);
    if t.exists() {
        let bytes = fs::read(&t).map_err(|e| io(&t, "read", e))?;
        write_atomic(&dir.join(TERRAIN_FILE), &bytes).map_err(|e| io(dir, "write", e))?;
    } else {
        let _ = fs::remove_file(dir.join(TERRAIN_FILE));
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_number_up_prune_and_revert() {
        let dir = std::env::temp_dir().join(format!("dc-mapkit-history-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MAP_FILE), b"{\"v\":1}").unwrap();
        fs::write(dir.join(TERRAIN_FILE), [1u8, 0, 2, 0]).unwrap();
        assert!(list(&dir).unwrap().is_empty());
        let e1 = snapshot(&dir, "first").unwrap();
        assert_eq!(e1.n, 1);
        assert!(e1.terrain);
        fs::write(dir.join(MAP_FILE), b"{\"v\":2}").unwrap();
        let e2 = snapshot(&dir, "second").unwrap();
        assert_eq!(e2.n, 2);
        fs::write(dir.join(MAP_FILE), b"{\"v\":3}").unwrap();
        let r = revert(&dir, 1).unwrap();
        assert_eq!(r.label, "first");
        assert_eq!(fs::read(dir.join(MAP_FILE)).unwrap(), b"{\"v\":1}");
        let entries = list(&dir).unwrap();
        assert_eq!(entries.len(), 3, "the revert snapshotted v3 first");
        assert_eq!(entries[2].label, "before revert to 1");
        assert_eq!(
            fs::read(history_dir(&dir).join("3").join(MAP_FILE)).unwrap(),
            b"{\"v\":3}"
        );
        let e = revert(&dir, 99).unwrap_err();
        assert_eq!(e.items[0].code, Code::HistoryMissing);
        for i in 0..KEEP + 5 {
            snapshot(&dir, &format!("bulk {i}")).unwrap();
        }
        let entries = list(&dir).unwrap();
        assert_eq!(entries.len(), KEEP);
        assert!(entries[0].n > 3, "the oldest went first");
        assert!(!history_dir(&dir).join("1").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
