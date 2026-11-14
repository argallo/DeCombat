//! A lock next to a store file: two game instances on one profile take turns, so they never spend the same secret
//! (the second would read as reuse and sign the player out) and never write the store at once.
//!
//! The lock is an operating-system lock on an open lock file (`<store>.lock`): `LockFileEx` on Windows, `flock` on
//! Unix, through the standard library's `File::try_lock`. The system releases it when the holding handle closes,
//! and so when the holding process dies, so there is no staleness rule, no takeover and nothing that can outlive a
//! holder or be released by anyone but the holder: a long refresh keeps the lock as long as it runs. The lock file is
//! never deleted (deleting it would let a newcomer lock a new file while the holder still holds the old one); it
//! only carries the holder's process id and a random token for a person who looks. A waiter polls for a bounded time
//! and gives up, so a stuck holder costs the waiter its refresh, not its credential.
//!
//! Re-entrant within one thread (a refresh holds it and saves under it); other threads of the same process wait like
//! other processes do (each acquisition opens its own handle).
// DC-DEFAULT: SES-8 the store lock: an OS file lock, a bounded wait of 30 s by default

use std::fs::{File, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// How long a caller waits for another holder by default.
pub const WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(15);

/// The locks this process holds: path, holding thread, depth, and the locked handle (closing it releases the lock).
static HELD: Mutex<Vec<(PathBuf, ThreadId, usize, File)>> = Mutex::new(Vec::new());

/// A held lock; released when the last guard of the holding thread is dropped. A guard stays on the thread that
/// took it (it is not `Send`), so its drop can only release that thread's own hold, never another holder's:
///
/// ```compile_fail
/// fn needs_send<T: Send>() {}
/// needs_send::<dc_session::lock::FileLock>();
/// ```
#[derive(Debug)]
pub struct FileLock {
    path: PathBuf,
    _thread_bound: std::marker::PhantomData<*const ()>,
}

fn held() -> std::sync::MutexGuard<'static, Vec<(PathBuf, ThreadId, usize, File)>> {
    HELD.lock().unwrap_or_else(|e| e.into_inner())
}

/// A random token for the lock file's diagnostics (not used for any decision).
fn token() -> String {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 8];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl FileLock {
    /// Takes the lock of `path`, waiting at most `wait` for another holder.
    pub fn acquire(path: &Path, wait: Duration) -> io::Result<FileLock> {
        let me = std::thread::current().id();
        {
            let mut h = held();
            if let Some(e) = h.iter_mut().find(|e| e.0 == path && e.1 == me) {
                e.2 += 1;
                return Ok(FileLock {
                    path: path.to_path_buf(),
                    _thread_bound: std::marker::PhantomData,
                });
            }
        }
        if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(d)?;
        }
        let mut o = std::fs::OpenOptions::new();
        o.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let deadline = Instant::now() + wait;
        loop {
            let mut f = o.open(path)?;
            match f.try_lock() {
                Ok(()) => {
                    // diagnostics only: who holds it
                    let _ = f.set_len(0);
                    let _ = writeln!(f, "pid {} token {}", std::process::id(), token());
                    held().push((path.to_path_buf(), me, 1, f));
                    return Ok(FileLock {
                        path: path.to_path_buf(),
                        _thread_bound: std::marker::PhantomData,
                    });
                }
                Err(TryLockError::WouldBlock) => {
                    drop(f);
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "the device store is in use by another game instance",
                        ));
                    }
                    std::thread::sleep(POLL);
                }
                Err(TryLockError::Error(e)) => return Err(e),
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let me = std::thread::current().id();
        let mut h = held();
        // only the holding thread's own entry; closing its handle releases the system lock
        if let Some(i) = h.iter().position(|e| e.0 == self.path && e.1 == me) {
            h[i].2 -= 1;
            if h[i].2 == 0 {
                h.remove(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-session-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_lock_is_reentrant_in_a_thread_and_exclusive_across_threads() {
        let d = dir("excl");
        let p = d.join("s.lock");
        let a = FileLock::acquire(&p, WAIT).unwrap();
        let b = FileLock::acquire(&p, Duration::ZERO).unwrap();
        drop(b);
        let p2 = p.clone();
        let other =
            std::thread::spawn(move || FileLock::acquire(&p2, Duration::from_millis(50)).is_err());
        assert!(other.join().unwrap(), "another thread must not get it");
        drop(a);
        assert!(
            p.exists(),
            "the lock file stays; only the system lock is released"
        );
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.starts_with(&format!("pid {} token ", std::process::id())),
            "{text}"
        );
        let p2 = p.clone();
        let other = std::thread::spawn(move || FileLock::acquire(&p2, WAIT).is_ok());
        assert!(other.join().unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_waiter_gets_the_lock_only_when_it_is_released() {
        let d = dir("wait");
        let p = d.join("s.lock");
        let a = FileLock::acquire(&p, WAIT).unwrap();
        let released = Arc::new(AtomicBool::new(false));
        let (p2, r2) = (p.clone(), released.clone());
        let t = std::thread::spawn(move || {
            let g = FileLock::acquire(&p2, WAIT).unwrap();
            let after = r2.load(Ordering::SeqCst);
            drop(g);
            after
        });
        std::thread::sleep(Duration::from_millis(100));
        released.store(true, Ordering::SeqCst);
        drop(a);
        assert!(
            t.join().unwrap(),
            "it got the lock only after the holder let go"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Dropping guards that do not hold the lock (a nested one, one of another path) never releases the holder's
    /// lock, and a hold far longer than any wait is kept: no other attempt can take it over.
    #[test]
    fn only_the_holders_last_guard_releases_and_a_long_hold_is_kept() {
        let d = dir("own");
        let p = d.join("s.lock");
        let other_path = d.join("t.lock");
        let a = FileLock::acquire(&p, WAIT).unwrap();
        drop(FileLock::acquire(&p, WAIT).unwrap());
        drop(FileLock::acquire(&other_path, WAIT).unwrap());
        for _ in 0..3 {
            let p2 = p.clone();
            let refused = std::thread::spawn(move || {
                FileLock::acquire(&p2, Duration::from_millis(100)).is_err()
            });
            assert!(refused.join().unwrap(), "still held");
            std::thread::sleep(Duration::from_millis(150));
        }
        drop(a);
        // the failed attempts wrote nothing: the note still names this holder (Windows refuses reads of a held lock
        // file, so it is read after the release)
        let note = std::fs::read_to_string(&p).unwrap();
        assert!(
            note.starts_with(&format!("pid {} token ", std::process::id())),
            "{note}"
        );
        let p2 = p.clone();
        assert!(
            std::thread::spawn(move || FileLock::acquire(&p2, WAIT).is_ok())
                .join()
                .unwrap()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Many threads competing for one lock: at most one holds it at any moment.
    #[test]
    fn competing_threads_never_hold_it_together() {
        let d = dir("compete");
        let p = d.join("s.lock");
        let inside = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ts: Vec<_> = (0..6)
            .map(|_| {
                let (p, inside) = (p.clone(), inside.clone());
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        let g = FileLock::acquire(&p, WAIT).unwrap();
                        assert_eq!(inside.fetch_add(1, Ordering::SeqCst), 0, "two holders");
                        std::thread::sleep(Duration::from_millis(1));
                        inside.fetch_sub(1, Ordering::SeqCst);
                        drop(g);
                    }
                })
            })
            .collect();
        for t in ts {
            t.join().unwrap();
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The other process of the next test: holds the lock named in `DC_SESSION_LOCK_CHILD` until it is killed.
    #[test]
    #[ignore = "run by a_holder_that_dies_releases_the_lock as a child process"]
    fn child_holder() {
        let Ok(path) = std::env::var("DC_SESSION_LOCK_CHILD") else {
            return;
        };
        let _g = FileLock::acquire(Path::new(&path), WAIT).unwrap();
        println!("held");
        std::io::stdout().flush().unwrap();
        std::thread::sleep(Duration::from_secs(120));
    }

    /// Another process holds the lock: this one waits; the holder dies (killed, no clean-up of its own): the lock is
    /// free at once, with no staleness rule.
    #[test]
    fn a_holder_that_dies_releases_the_lock() {
        use std::io::BufRead;
        let d = dir("dies");
        let p = d.join("s.lock");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "lock::tests::child_holder",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("DC_SESSION_LOCK_CHILD", &p)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.contains("held") {
            line.clear();
            assert!(
                out.read_line(&mut line).unwrap() > 0,
                "the child ended early"
            );
        }
        assert!(
            FileLock::acquire(&p, Duration::from_millis(200)).is_err(),
            "the other process holds it"
        );
        child.kill().unwrap();
        child.wait().unwrap();
        let g = FileLock::acquire(&p, Duration::from_secs(5));
        assert!(g.is_ok(), "the dead holder's lock is free");
        drop(g);
        let _ = std::fs::remove_dir_all(&d);
    }
}
