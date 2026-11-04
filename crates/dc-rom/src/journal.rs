//! Read journal for the render determinism manifest (`docs/RENDER_DETERMINISM_CONTRACT.md`, gate G0 of
//! `docs/RENDER_PERF_GATES.md`): while it is on, every read of the user's client folder through the readers of this
//! workspace (`.rez` archive entries, mod override files, PE images, profile files) notes the read's key and the
//! SHA-256 of the bytes read, so an exact-lane run can name its exact inputs by hash.
//!
//! Off by default. Off costs one relaxed atomic load per read and keeps nothing. The journal is process-global:
//! only one exact-lane run may use it at a time ([`lock`]). Keys name files of the user's folder and stay inside
//! the process: the manifest writes only digests of them (never names, CLEANROOM.md section 4).
// Provenance: ORIGINAL (our own tooling; no 2009 behaviour)

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

static ON: AtomicBool = AtomicBool::new(false);
static LOG: Mutex<BTreeMap<String, [u8; 32]>> = Mutex::new(BTreeMap::new());
static RUN: Mutex<()> = Mutex::new(());

/// Serialises the users of the journal (one exact-lane run at a time in a process); hold it for the whole run.
pub fn lock() -> MutexGuard<'static, ()> {
    RUN.lock().unwrap_or_else(|p| p.into_inner())
}

/// Starts (`true`) or stops noting reads. Starting clears what an earlier run noted.
pub fn enable(on: bool) {
    if on {
        LOG.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
    ON.store(on, Ordering::Relaxed);
}

/// The journal is noting reads.
#[inline]
pub fn is_on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Notes one read: `key` names the file (and range) read, `bytes` what was read. The key is built only when the
/// journal is on.
#[inline]
pub fn note(key: impl FnOnce() -> String, bytes: &[u8]) {
    if !is_on() {
        return;
    }
    note_slow(key(), bytes);
}

#[cold]
fn note_slow(key: String, bytes: &[u8]) {
    let digest = crate::sha256(bytes);
    LOG.lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, digest);
}

/// Every read noted since [`enable`], ordered by key (a read repeated with the same key keeps its last digest).
pub fn snapshot() -> Vec<(String, [u8; 32])> {
    LOG.lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_notes_only_while_on() {
        let _g = lock();
        enable(false);
        note(|| "a".into(), b"x");
        enable(true);
        assert!(
            snapshot().is_empty(),
            "enable clears and nothing was noted while off"
        );
        note(|| "b".into(), b"abc");
        note(|| "a".into(), b"x");
        let s = snapshot();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].0, "a", "ordered by key");
        assert_eq!(s[1].1, crate::sha256(b"abc"));
        enable(false);
        note(|| "c".into(), b"y");
        assert_eq!(snapshot().len(), 2);
    }
}
