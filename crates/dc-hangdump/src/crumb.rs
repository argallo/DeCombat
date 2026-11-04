//! Breadcrumbs (gate T1): every thread keeps a small lock-free ring of the last places it passed, so a hang report
//! can say what each thread was waiting on. A crumb is one relaxed atomic store of a pointer to a `static` site and
//! one store of its time: no allocation, no lock, no I/O, also on a thread's first crumb (it takes a slot of the
//! global table by one scan and stores only its thread id; the reader looks the thread's name up). A thread gives
//! its slot back when it ends.
//!
//! - [`crumb!`](crate::crumb!) marks a point (`crumb!("loader: job done")`).
//! - [`crumb_wait!`](crate::crumb_wait!) wraps a blocking expression (a lock, a `recv`, a `join`): it notes
//!   "enter" before and "leave" after, so a thread whose newest crumb is an "enter" is still inside that wait.
//!
//! [`snapshot`] reads every live slot (the watchdog, a panic hook, an exception filter); it never blocks the threads
//! it reads, and a read that races a write shows one entry torn at worst.
// ORIGINAL: our own diagnostics tooling (gate T1); no 2009 behaviour

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

/// One crumb site: what the thread is doing there and where in the source.
#[derive(Debug)]
pub struct Site {
    pub what: &'static str,
    pub file: &'static str,
    pub line: u32,
}

/// Threads with a slot at the same time.
pub const SLOTS: usize = 64;
/// Crumbs kept per thread.
pub const DEPTH: usize = 16;

/// How a crumb was set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Point,
    Enter,
    Leave,
}

struct Slot {
    used: AtomicBool,
    tid: AtomicU32,
    pos: AtomicU32,
    site: [AtomicPtr<Site>; DEPTH],
    /// time in ms since the epoch << 2 | kind
    at: [AtomicU64; DEPTH],
}

impl Slot {
    const fn new() -> Slot {
        Slot {
            used: AtomicBool::new(false),
            tid: AtomicU32::new(0),
            pos: AtomicU32::new(0),
            site: [const { AtomicPtr::new(std::ptr::null_mut()) }; DEPTH],
            at: [const { AtomicU64::new(0) }; DEPTH],
        }
    }
}

static TABLE: [Slot; SLOTS] = [const { Slot::new() }; SLOTS];
static EPOCH: OnceLock<Instant> = OnceLock::new();
/// Crumbs that found no free slot (more than [`SLOTS`] threads at once).
static NO_SLOT: AtomicU64 = AtomicU64::new(0);
/// `DC_CRUMBS=0` turns crumbs off (they are on by default: a crumb costs two relaxed stores).
static OFF: AtomicBool = AtomicBool::new(false);

/// Milliseconds since the first crumb of the process (the crumbs' clock).
pub fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

#[inline]
fn off() -> bool {
    OFF.load(Ordering::Relaxed)
}

/// Reads `DC_CRUMBS` (`0` / `off` / `false` / `no` turn crumbs off) once at start-up, so a crumb never reads the
/// environment (which allocates). Crumbs are on until this is called.
pub fn configure_from_env() {
    let off = matches!(
        std::env::var("DC_CRUMBS").ok().as_deref().map(str::trim),
        Some("0" | "off" | "false" | "no")
    );
    OFF.store(off, Ordering::Relaxed);
}

/// The calling thread's slot; given back when the thread ends.
struct Reg(Cell<usize>);

impl Drop for Reg {
    fn drop(&mut self) {
        let i = self.0.get();
        if i < SLOTS {
            let s = &TABLE[i];
            s.tid.store(0, Ordering::Relaxed);
            s.used.store(false, Ordering::Release);
        }
    }
}

thread_local! {
    static MINE: Reg = const { Reg(Cell::new(usize::MAX)) };
}

const UNSET: usize = usize::MAX;
const NONE: usize = usize::MAX - 1;

fn register() -> usize {
    // no allocation here (a crumb can run on the audio callback's thread): the name is looked up by the reader
    let tid = crate::current_thread_id();
    for (i, s) in TABLE.iter().enumerate() {
        if s.used
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            s.pos.store(0, Ordering::Relaxed);
            for k in 0..DEPTH {
                s.site[k].store(std::ptr::null_mut(), Ordering::Relaxed);
                s.at[k].store(0, Ordering::Relaxed);
            }
            s.tid.store(tid, Ordering::Release);
            return i;
        }
    }
    NO_SLOT.fetch_add(1, Ordering::Relaxed);
    NONE
}

/// Notes one crumb on the calling thread (use the macros).
#[inline]
pub fn push(site: &'static Site, kind: Kind) {
    if off() {
        return;
    }
    let at = (now_ms() << 2)
        | match kind {
            Kind::Point => 0,
            Kind::Enter => 1,
            Kind::Leave => 2,
        };
    let _ = MINE.try_with(|r| {
        let mut i = r.0.get();
        if i == UNSET {
            i = register();
            r.0.set(i);
        }
        if i >= SLOTS {
            return;
        }
        let s = &TABLE[i];
        let p = s.pos.fetch_add(1, Ordering::Relaxed) as usize % DEPTH;
        s.site[p].store(site as *const Site as *mut Site, Ordering::Relaxed);
        s.at[p].store(at, Ordering::Release);
    });
}

/// Leaves a [`crumb_wait!`](crate::crumb_wait!) when dropped (also on unwinding).
pub struct Wait(&'static Site);

impl Wait {
    #[inline]
    pub fn enter(site: &'static Site) -> Wait {
        push(site, Kind::Enter);
        Wait(site)
    }
}

impl Drop for Wait {
    #[inline]
    fn drop(&mut self) {
        push(self.0, Kind::Leave);
    }
}

/// Marks a point on the calling thread: `crumb!("loader: job done")`.
#[macro_export]
macro_rules! crumb {
    ($what:expr) => {{
        static SITE: $crate::crumb::Site = $crate::crumb::Site {
            what: $what,
            file: file!(),
            line: line!(),
        };
        $crate::crumb::push(&SITE, $crate::crumb::Kind::Point);
    }};
}

/// Wraps a blocking expression: `crumb_wait!("music: lock state", state.lock())` notes "enter" before and "leave"
/// after it (the value of the expression is returned).
#[macro_export]
macro_rules! crumb_wait {
    ($what:expr, $e:expr) => {{
        static SITE: $crate::crumb::Site = $crate::crumb::Site {
            what: $what,
            file: file!(),
            line: line!(),
        };
        let __crumb = $crate::crumb::Wait::enter(&SITE);
        let __value = $e;
        drop(__crumb);
        __value
    }};
}

/// One crumb as read back.
#[derive(Clone, Debug)]
pub struct Entry {
    pub what: &'static str,
    pub file: &'static str,
    pub line: u32,
    pub kind: Kind,
    pub at_ms: u64,
}

/// One thread's crumbs, oldest first.
#[derive(Clone, Debug)]
pub struct Thread {
    pub tid: u32,
    pub name: String,
    pub crumbs: Vec<Entry>,
}

impl Thread {
    /// The wait this thread is inside now (its newest crumb is an "enter"), with the time it entered.
    pub fn waiting(&self) -> Option<&Entry> {
        self.crumbs.last().filter(|e| e.kind == Kind::Enter)
    }
}

/// Every thread with a slot and its crumbs (oldest first). Never blocks.
pub fn snapshot() -> Vec<Thread> {
    let mut out = Vec::new();
    for s in TABLE.iter() {
        if !s.used.load(Ordering::Acquire) {
            continue;
        }
        let tid = s.tid.load(Ordering::Acquire);
        if tid == 0 && cfg!(windows) {
            continue;
        }
        let pos = s.pos.load(Ordering::Relaxed) as usize;
        let count = pos.min(DEPTH);
        let mut crumbs = Vec::with_capacity(count);
        for k in 0..count {
            let p = (pos - count + k) % DEPTH;
            let at = s.at[p].load(Ordering::Acquire);
            let site = s.site[p].load(Ordering::Relaxed);
            if site.is_null() {
                continue;
            }
            // SAFETY: every pointer stored in a slot comes from `push`, which takes a `&'static Site`.
            let site: &'static Site = unsafe { &*site };
            crumbs.push(Entry {
                what: site.what,
                file: site.file,
                line: site.line,
                kind: match at & 3 {
                    1 => Kind::Enter,
                    2 => Kind::Leave,
                    _ => Kind::Point,
                },
                at_ms: at >> 2,
            });
        }
        out.push(Thread {
            tid,
            name: crate::thread_description(tid).unwrap_or_else(|| format!("tid {tid}")),
            crumbs,
        });
    }
    out
}

/// Crumbs that found no free slot so far.
pub fn no_slot() -> u64 {
    NO_SLOT.load(Ordering::Relaxed)
}

/// The snapshot as report lines (`now` = [`now_ms`] at the report).
pub fn lines(threads: &[Thread], now: u64) -> Vec<String> {
    let mut out = Vec::new();
    for t in threads {
        let head = match t.waiting() {
            Some(e) => format!(
                "  thread {} \"{}\": WAITING in \"{}\" for {} ms ({}:{})",
                t.tid,
                t.name,
                e.what,
                now.saturating_sub(e.at_ms),
                e.file,
                e.line
            ),
            None => match t.crumbs.last() {
                Some(e) => format!(
                    "  thread {} \"{}\": last crumb \"{}\" {} ms ago",
                    t.tid,
                    t.name,
                    e.what,
                    now.saturating_sub(e.at_ms)
                ),
                None => format!("  thread {} \"{}\": no crumbs", t.tid, t.name),
            },
        };
        out.push(head);
        for e in t.crumbs.iter().rev() {
            let k = match e.kind {
                Kind::Point => "at   ",
                Kind::Enter => "enter",
                Kind::Leave => "leave",
            };
            out.push(format!(
                "      -{:>7} ms {k} {} ({}:{})",
                now.saturating_sub(e.at_ms),
                e.what,
                e.file,
                e.line
            ));
        }
    }
    out
}

// Both tests read thread names through the Windows `sys` (lib.rs); other platforms have the no-op `sys`.
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, Mutex};

    #[test]
    fn a_thread_inside_a_wait_is_named_with_its_crumb_and_its_slot_is_freed_at_its_end() {
        let lock = Arc::new(Mutex::new(()));
        let held = lock.lock().unwrap();
        let go = Arc::new(Barrier::new(2));
        let (l2, g2) = (lock.clone(), go.clone());
        let h = std::thread::Builder::new()
            .name("crumb-test-waiter".into())
            .spawn(move || {
                crate::crumb!("test: started");
                g2.wait();
                let _g = crate::crumb_wait!("test: lock the shared state", l2.lock());
            })
            .unwrap();
        go.wait();
        let mut seen = None;
        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let snap = snapshot();
            if let Some(t) = snap.iter().find(|t| t.name == "crumb-test-waiter") {
                if let Some(w) = t.waiting() {
                    seen = Some((w.what, lines(&snap, now_ms()).join("\n")));
                    break;
                }
            }
        }
        drop(held);
        h.join().unwrap();
        let (what, text) = seen.expect("the waiter shows its wait");
        assert_eq!(what, "test: lock the shared state");
        assert!(
            text.contains("WAITING in \"test: lock the shared state\""),
            "{text}"
        );
        // the slot is given back
        assert!(!snapshot().iter().any(|t| t.name == "crumb-test-waiter"));
    }

    #[test]
    fn the_ring_keeps_the_newest_crumbs_oldest_first() {
        let h = std::thread::Builder::new()
            .name("crumb-test-ring".into())
            .spawn(|| {
                for _ in 0..(DEPTH + 3) {
                    crate::crumb!("test: tick");
                }
                let r = crate::crumb_wait!("test: value", 7);
                assert_eq!(r, 7);
                let t = snapshot()
                    .into_iter()
                    .find(|t| t.name == "crumb-test-ring")
                    .unwrap();
                assert_eq!(t.crumbs.len(), DEPTH);
                assert_eq!(t.crumbs.last().unwrap().kind, Kind::Leave);
                assert_eq!(t.crumbs[DEPTH - 2].kind, Kind::Enter);
                assert!(t.waiting().is_none());
            })
            .unwrap();
        h.join().unwrap();
    }
}
