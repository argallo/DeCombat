//! A bounded, non-blocking line log: the calling thread hands each finished line to a writer thread through a
//! fixed-capacity queue and never touches the file itself.
//!
//! Why (perf-hitch, 2026-10-26): the client's sound log wrote one unbuffered line per sound start on the frame thread,
//! and the session recorder flushed its event log after every line while the network thread held the session lock
//! that the frame thread waits for. On a busy disk single writes stalled the frame by 30-800 ms (window runs of the
//! client-fps worker). With this writer the producer's cost is one string move and two atomic adds.
//!
//! Bounds (RENDER_PERF_GATES.md rule): at most `cap_records` lines and `cap_bytes` bytes wait in the queue. A line
//! that would exceed either cap is DROPPED (drop-newest: the lines already queued are kept, so a reader sees a gap
//! at the end of a burst, never a reordering) and counted. The writer flushes its buffer whenever the queue runs
//! empty, so the file stays close to live for readers that tail it. [`LogWriter::stats`] reports the lines written,
//! dropped and the high-water marks of both caps.
//!
//! Provenance: ORIGINAL (our own tooling; no 2009 behaviour involved, the file formats are unchanged).
// DC-DEFAULT: CLI-441 (proposed) session logs are written by bounded writer threads, see DC_DEFAULTS.md

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Default cap of queued lines.
pub const DEFAULT_RECORDS: usize = 8192;
/// Default cap of queued bytes (4 MiB; a capture line of a full datagram is under 3 KB).
pub const DEFAULT_BYTES: usize = 4 << 20;
/// Buffer of the writer thread's file.
const WRITE_BUFFER: usize = 64 << 10;

enum Msg {
    Line(String),
    Flush(SyncSender<()>),
}

#[derive(Default)]
struct Counters {
    queued_records: AtomicUsize,
    queued_bytes: AtomicUsize,
    hwm_records: AtomicUsize,
    hwm_bytes: AtomicUsize,
    written: AtomicU64,
    dropped: AtomicU64,
    write_errors: AtomicU64,
    /// Longest single write or flush on the writer thread, µs (the stall the producer no longer sees).
    max_io_us: AtomicU64,
}

/// What a [`LogWriter`] did so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogStats {
    pub written: u64,
    pub dropped: u64,
    pub write_errors: u64,
    pub hwm_records: usize,
    pub hwm_bytes: usize,
    pub cap_records: usize,
    pub cap_bytes: usize,
    /// Longest single write or flush call of the writer thread, µs.
    pub max_io_us: u64,
}

impl LogStats {
    /// One short text for exit reports: `written W dropped D (queue high-water R/CR lines, B/CB KB)`.
    pub fn line(&self) -> String {
        format!(
            "written {} dropped {} write errors {} longest write {:.1} ms (queue high-water {}/{} lines, {}/{} KB)",
            self.written,
            self.dropped,
            self.write_errors,
            self.max_io_us as f64 / 1000.0,
            self.hwm_records,
            self.cap_records,
            self.hwm_bytes.div_ceil(1024),
            self.cap_bytes / 1024
        )
    }
}

/// The producer side of a bounded line log (see the module doc).
pub struct LogWriter {
    tx: Option<SyncSender<Msg>>,
    c: Arc<Counters>,
    thread: Option<JoinHandle<()>>,
    cap_records: usize,
    cap_bytes: usize,
}

impl LogWriter {
    /// Creates (truncates) `path`, its parent directories, and the writer thread, with the default caps.
    pub fn create(path: &Path) -> io::Result<LogWriter> {
        Self::create_with(path, DEFAULT_RECORDS, DEFAULT_BYTES)
    }

    /// As [`LogWriter::create`] with explicit caps.
    pub fn create_with(path: &Path, cap_records: usize, cap_bytes: usize) -> io::Result<LogWriter> {
        if let Some(d) = path.parent() {
            if !d.as_os_str().is_empty() {
                std::fs::create_dir_all(d)?;
            }
        }
        let f = Stall::new(File::create(path)?);
        Ok(Self::from_writer(
            BufWriter::with_capacity(WRITE_BUFFER, f),
            cap_records,
            cap_bytes,
        ))
    }

    /// A log over any sink (tests use slow or failing sinks).
    pub fn from_writer<W: Write + Send + 'static>(
        w: W,
        cap_records: usize,
        cap_bytes: usize,
    ) -> LogWriter {
        let cap_records = cap_records.max(1);
        let cap_bytes = cap_bytes.max(1);
        let (tx, rx) = sync_channel(cap_records);
        let c = Arc::new(Counters::default());
        let cc = c.clone();
        let thread = std::thread::Builder::new()
            .name("dc-log-writer".into())
            .spawn(move || writer_main(w, rx, &cc))
            .ok();
        LogWriter {
            tx: thread.as_ref().map(|_| tx),
            c,
            thread,
            cap_records,
            cap_bytes,
        }
    }

    /// Queues one line (the newline is added by the writer). Never blocks and never does I/O; returns false when
    /// the line was dropped because a cap was reached (counted).
    pub fn push(&self, line: String) -> bool {
        let Some(tx) = self.tx.as_ref() else {
            self.c.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        let len = line.len() + 1;
        let bytes = self.c.queued_bytes.fetch_add(len, Ordering::AcqRel) + len;
        if bytes > self.cap_bytes {
            self.c.queued_bytes.fetch_sub(len, Ordering::AcqRel);
            self.c.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let records = self.c.queued_records.fetch_add(1, Ordering::AcqRel) + 1;
        match tx.try_send(Msg::Line(line)) {
            Ok(()) => {
                self.c.hwm_bytes.fetch_max(bytes, Ordering::Relaxed);
                self.c
                    .hwm_records
                    .fetch_max(records.min(self.cap_records), Ordering::Relaxed);
                true
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.c.queued_bytes.fetch_sub(len, Ordering::AcqRel);
                self.c.queued_records.fetch_sub(1, Ordering::AcqRel);
                self.c.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Waits until every line queued before this call is written and the file is flushed. BLOCKS: for the end
    /// of a session and for tests, never for a frame or network loop.
    pub fn flush(&self) {
        let Some(tx) = self.tx.as_ref() else {
            return;
        };
        let (ack_tx, ack_rx) = sync_channel(1);
        if dc_hangdump::crumb_wait!("log writer: queue the flush", tx.send(Msg::Flush(ack_tx)))
            .is_ok()
        {
            let _ = dc_hangdump::crumb_wait!("log writer: wait for the flush", ack_rx.recv());
        }
    }

    /// The counters so far.
    pub fn stats(&self) -> LogStats {
        LogStats {
            written: self.c.written.load(Ordering::Relaxed),
            dropped: self.c.dropped.load(Ordering::Relaxed),
            write_errors: self.c.write_errors.load(Ordering::Relaxed),
            hwm_records: self.c.hwm_records.load(Ordering::Relaxed),
            hwm_bytes: self.c.hwm_bytes.load(Ordering::Relaxed),
            cap_records: self.cap_records,
            cap_bytes: self.cap_bytes,
            max_io_us: self.c.max_io_us.load(Ordering::Relaxed),
        }
    }
}

impl Drop for LogWriter {
    /// Closes the queue; the writer writes what is queued, flushes and ends (joined here).
    fn drop(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = dc_hangdump::crumb_wait!("log writer: join the writer thread", t.join());
        }
    }
}

fn writer_main<W: Write>(mut w: W, rx: Receiver<Msg>, c: &Counters) {
    // every write and flush is timed: the longest is the stall the producers no longer see
    let timed = |f: &mut dyn FnMut() -> io::Result<()>| {
        let t = std::time::Instant::now();
        let r = f();
        c.max_io_us
            .fetch_max(t.elapsed().as_micros() as u64, Ordering::Relaxed);
        if r.is_err() {
            c.write_errors.fetch_add(1, Ordering::Relaxed);
        }
        r.is_ok()
    };
    let handle = |w: &mut W, m: Msg| match m {
        Msg::Line(line) => {
            c.queued_records.fetch_sub(1, Ordering::AcqRel);
            c.queued_bytes.fetch_sub(line.len() + 1, Ordering::AcqRel);
            if timed(&mut || {
                w.write_all(line.as_bytes())
                    .and_then(|_| w.write_all(b"\n"))
            }) {
                c.written.fetch_add(1, Ordering::Relaxed);
            }
        }
        Msg::Flush(ack) => {
            timed(&mut || w.flush());
            let _ = ack.send(());
        }
    };
    while let Ok(m) = dc_hangdump::crumb_wait!("log writer: wait for lines", rx.recv()) {
        handle(&mut w, m);
        // write the whole burst, then flush once the queue is empty (the file stays near live for readers)
        while let Ok(m) = rx.try_recv() {
            handle(&mut w, m);
        }
        timed(&mut || w.flush());
    }
    let _ = w.flush();
}

/// Fault injection for hitch measurements (perf-hitch tooling, off unless asked for): with
/// `DC_LOG_STALL=<period_ms>:<stall_ms>` a log sink wrapped in [`Stall`] sleeps `stall_ms` in the first write call
/// made at least `period_ms` after its previous stall (a disk that stalls now and then: an almost full volume, a
/// virus scanner). The same schedule applies whichever thread writes, so a run with the old synchronous log and one
/// with [`LogWriter`] meet the same stalls.
pub fn stall_from_env() -> Option<(u64, u64)> {
    let v = std::env::var("DC_LOG_STALL").ok()?;
    let (p, s) = v.trim().split_once(':')?;
    let (p, s) = (p.trim().parse().ok()?, s.trim().parse().ok()?);
    (s > 0).then_some((p, s))
}

/// A sink that stalls on the [`stall_from_env`] schedule (a plain pass-through without it).
pub struct Stall<W> {
    inner: W,
    plan: Option<(u64, u64)>,
    last: std::time::Instant,
}

impl<W: Write> Stall<W> {
    pub fn new(inner: W) -> Stall<W> {
        Stall {
            inner,
            plan: stall_from_env(),
            last: std::time::Instant::now(),
        }
    }
}

impl<W: Write> Write for Stall<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if let Some((period, ms)) = self.plan {
            if self.last.elapsed().as_millis() as u64 >= period {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                self.last = std::time::Instant::now();
            }
        }
        self.inner.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};

    /// A sink that blocks every write until released (a stalled disk).
    #[derive(Clone, Default)]
    struct Gate {
        open: Arc<(Mutex<bool>, Condvar)>,
        out: Arc<Mutex<Vec<u8>>>,
    }

    impl Gate {
        fn release(&self) {
            *self.open.0.lock().unwrap() = true;
            self.open.1.notify_all();
        }
    }

    impl Write for Gate {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            let (m, cv) = &*self.open;
            let mut g = m.lock().unwrap();
            while !*g {
                g = cv.wait(g).unwrap();
            }
            self.out.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn lines_arrive_in_order_and_flush_waits_for_them() {
        let g = Gate::default();
        g.release();
        let out = g.out.clone();
        let w = LogWriter::from_writer(g, 64, 1 << 20);
        for i in 0..50 {
            assert!(w.push(format!("line {i}")));
        }
        w.flush();
        let text = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 50);
        assert_eq!(lines[0], "line 0");
        assert_eq!(lines[49], "line 49");
        let s = w.stats();
        assert_eq!((s.written, s.dropped), (50, 0));
    }

    #[test]
    fn a_stalled_sink_never_blocks_the_producer_and_overflow_drops_the_newest() {
        let g = Gate::default();
        let out = g.out.clone();
        let w = LogWriter::from_writer(g.clone(), 8, 1 << 20);
        let t = Instant::now();
        let mut kept = 0;
        for i in 0..1000 {
            if w.push(format!("{i}")) {
                kept += 1;
            }
        }
        // 1000 pushes against a sink that never returns: bounded time, bounded queue
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "{:?}",
            t.elapsed()
        );
        let s = w.stats();
        // the writer may hold one line in its hand (blocked in write) on top of the 8 queued
        assert!((8..=9).contains(&kept), "kept {kept}");
        assert_eq!(s.dropped, 1000 - kept as u64);
        assert!(s.hwm_records <= 8);
        g.release();
        w.flush();
        let text = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        // drop-newest: the first 8 lines survive and the order is kept (a slot freed by the writer taking a line may
        // let one later line in after the gap)
        let got: Vec<usize> = text.lines().map(|l| l.parse().unwrap()).collect();
        assert_eq!(got.len(), kept);
        assert_eq!(got[..8], (0..8).collect::<Vec<_>>()[..]);
        assert!(got.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn the_byte_cap_drops_and_counts() {
        let g = Gate::default();
        let w = LogWriter::from_writer(g.clone(), 1000, 100);
        let mut kept = 0;
        for _ in 0..100 {
            if w.push("x".repeat(19)) {
                kept += 1;
            }
        }
        // 20 bytes per line (newline included); 100 bytes queued at most, plus the one in the writer's hand
        assert!((5..=6).contains(&kept), "kept {kept}");
        let s = w.stats();
        assert!(s.hwm_bytes <= 100);
        assert_eq!(s.dropped, 100 - kept as u64);
        g.release();
    }

    #[test]
    fn drop_writes_everything_queued() {
        let dir = std::env::temp_dir().join(format!("dc-logring-{}", std::process::id()));
        let p = dir.join("a.jsonl");
        {
            let w = LogWriter::create(&p).unwrap();
            for i in 0..100 {
                w.push(format!("{{\"i\":{i}}}"));
            }
        }
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), 100);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
