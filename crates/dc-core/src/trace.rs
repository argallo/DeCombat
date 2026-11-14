//! ROM read trace for the server ROM measurement (`docs/SERVER_ROM.md`): with `DC_ROM_TRACE=<file>` set, every
//! archive entry and loose file the readers of this workspace open is appended to `<file>` as one tab-separated
//! line `process  kind  file  entry  bytes` (kind `rez` = archive entry, `dir` = archive directory, `file` = loose
//! file, `list` = a directory listing). Several processes may append to the same file (each line is one write
//! in append mode). Off when the variable is unset: one `OnceLock` read per note.
// Provenance: ORIGINAL (our own tooling; no 2009 behaviour)

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static SINK: OnceLock<Option<(String, Mutex<File>)>> = OnceLock::new();

fn sink() -> Option<&'static (String, Mutex<File>)> {
    SINK.get_or_init(|| {
        // the second environment read of this crate (beside `resolve_rom`): a measurement switch, never a path
        // the readers use
        let path = std::env::var_os("DC_ROM_TRACE")?;
        if path.is_empty() {
            return None;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        let me = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "?".to_string());
        Some((me, Mutex::new(file)))
    })
    .as_ref()
}

/// The trace is on (`DC_ROM_TRACE` names a writable file).
pub fn is_on() -> bool {
    sink().is_some()
}

/// Appends one read: `kind`, the file (archive or loose file), the archive entry (empty for loose files) and the
/// byte count.
pub fn note(kind: &str, file: &Path, entry: &str, bytes: u64) {
    let Some((me, f)) = sink() else {
        return;
    };
    let line = format!(
        "{me}\t{kind}\t{}\t{entry}\t{bytes}\n",
        file.display().to_string().replace('\\', "/")
    );
    let mut f = f.lock().unwrap_or_else(|p| p.into_inner());
    let _ = f.write_all(line.as_bytes());
}
