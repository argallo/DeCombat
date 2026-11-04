//! The sound director's decoded-WAV cache: bounded in bytes, filled ahead of use by a decode thread (perf-hitch).
//!
//! Why: every first use of a sound file read and decoded it on the frame thread (0.5 ms median, 13.7 ms p99, 58 ms
//! for the largest looped file, sndbench 2026-10-26), the server's preload list was decoded inside one frame (moving
//! it to the thread did NOT measurably change frame 0 of the headless network run: sound stage 145.5 ms before,
//! 146.9 ms after; that frame's cost was the footstep rig, 131 ms measured, see `SoundDirector::rig`), and the cache
//! had no size limit (all ROM sound files decode to about 396 MB of f32, a real risk for the 32-bit client).
//!
//! Design (RENDER_PERF_GATES.md rule: declared bounds, deterministic overflow, reported high-water marks):
//! - [`PcmCache::get`] answers from the cache; a miss decodes INLINE, exactly as before. The answer never depends
//!   on the decode thread's timing: the thread and the inline path run the same read + decoder on the same bytes,
//!   so a sound plays the same samples whichever decoded it. The thread only moves work off the frame thread.
//! - [`PcmCache::prefetch`] (async mode only) queues a file for the decode thread: at most [`PREFETCH_PENDING`]
//!   names wait (overflow: the newest name is dropped and counted; it decodes inline when first played) and at
//!   most [`DECODE_INFLIGHT`] are handed to the thread at a time, so the result channel holds at most that many
//!   decoded files.
//! - The cache holds at most [`CACHE_BYTES`] of samples. Inserting past the cap evicts the least recently used
//!   entries that no voice is playing (only the cache holds their `Arc`); if that is not enough the new file is
//!   not cached (it still plays; counted as a reject) and a prefetched one is dropped (counted). Evicted files
//!   decode again on their next use, to the same samples. Names of missing files stay as 0-byte entries.
//! - Inline mode (the default of headless runs and tests, as for the model loader) queues nothing: the decode
//!   order and every counter are those of the frame sequence alone.
//!
//! Provenance: ORIGINAL (our own engineering; the 2009 sound layer's own caching is not modelled).
// DC-DEFAULT: CLI-440 (proposed) sound files are decoded ahead on a thread (preload lists, footstep and jump
// sounds, the sounds of a weapon once one of them plays) and cached up to a byte cap, see DC_DEFAULTS.md

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Instant;

use dc_audio::decode::{decode_wav, Pcm};
use dc_rez::RezVfs;

/// Bytes of decoded samples the cache keeps (f32 samples; placeholder until the G1 budget pins it).
pub const CACHE_BYTES: usize = 128 << 20;
/// Names waiting for the decode thread.
pub const PREFETCH_PENDING: usize = 1024;
/// Names handed to the decode thread and not yet taken back.
pub const DECODE_INFLIGHT: usize = 8;

/// The cache key of a resource path (upper case, backslashes).
pub fn key_of(path: &str) -> String {
    path.trim().replace('/', "\\").to_ascii_uppercase()
}

fn bytes_of(p: &Option<Arc<Pcm>>) -> usize {
    p.as_ref()
        .map_or(0, |p| p.samples.len() * std::mem::size_of::<f32>())
}

/// Reads and decodes one sound file (the frame thread and the decode thread run exactly this).
pub fn decode_path(vfs: &RezVfs, path: &str) -> Option<Arc<Pcm>> {
    // CLI-1612: the installed mod layer first, then the ROM
    let bytes = crate::modassets::read_entry(vfs, path).ok()?;
    decode_wav(&bytes).ok().map(Arc::new)
}

struct Entry {
    pcm: Option<Arc<Pcm>>,
    bytes: usize,
    last: u64,
}

/// Counters and high-water marks of the cache (exit reports, tests).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    /// Misses decoded on the caller's (frame) thread.
    pub inline_decodes: u64,
    /// Longest inline decode, µs.
    pub inline_max_us: u64,
    pub prefetch_queued: u64,
    /// Prefetched files that arrived and were cached.
    pub prefetch_done: u64,
    /// Prefetch names dropped: pending list full, decode thread gone, or the file did not fit the cap.
    pub prefetch_dropped: u64,
    pub evictions: u64,
    /// Inline decodes that did not fit the cap (played, not cached).
    pub rejects: u64,
    pub bytes: usize,
    pub hwm_bytes: usize,
    pub hwm_pending: usize,
    pub hwm_inflight: usize,
}

impl CacheStats {
    pub fn line(&self) -> String {
        format!(
            "pcm cache {:.1} MB (high-water {:.1}/{} MB) hits {} inline decodes {} (longest {:.1} ms) prefetched {}/{} dropped {} evictions {} rejects {} (pending high-water {}/{}, in flight {}/{})",
            self.bytes as f64 / 1048576.0,
            self.hwm_bytes as f64 / 1048576.0,
            CACHE_BYTES >> 20,
            self.hits,
            self.inline_decodes,
            self.inline_max_us as f64 / 1000.0,
            self.prefetch_done,
            self.prefetch_queued,
            self.prefetch_dropped,
            self.evictions,
            self.rejects,
            self.hwm_pending,
            PREFETCH_PENDING,
            self.hwm_inflight,
            DECODE_INFLIGHT
        )
    }
}

struct Worker {
    tx: SyncSender<String>,
    rx: Receiver<(String, Option<Arc<Pcm>>)>,
}

/// The decoded-sound cache (see the module doc).
pub struct PcmCache {
    map: HashMap<String, Entry>,
    cap: usize,
    clock: u64,
    async_on: bool,
    worker: Option<Worker>,
    /// Keys waiting for the thread, and every key pending or in flight (no double requests).
    pending: VecDeque<String>,
    queued: HashSet<String>,
    inflight: usize,
    pub stats: CacheStats,
}

impl Default for PcmCache {
    fn default() -> Self {
        Self::new(CACHE_BYTES)
    }
}

impl PcmCache {
    pub fn new(cap: usize) -> PcmCache {
        PcmCache {
            map: HashMap::new(),
            cap,
            clock: 0,
            async_on: false,
            worker: None,
            pending: VecDeque::new(),
            queued: HashSet::new(),
            inflight: 0,
            stats: CacheStats::default(),
        }
    }

    /// Switches the decode thread on (it starts with the first prefetch) or off (inline mode; queued names are
    /// forgotten).
    pub fn set_async(&mut self, on: bool) {
        self.async_on = on;
        if !on {
            self.pending.clear();
            self.queued.clear();
        }
    }

    pub fn is_async(&self) -> bool {
        self.async_on
    }

    /// Whether `path` is cached (decoded or known missing).
    pub fn contains(&self, path: &str) -> bool {
        self.map.contains_key(&key_of(path))
    }

    /// The decoded file, from the cache or decoded inline now (`None`: no such file or not a WAV the decoder
    /// reads). The answer does not depend on the decode thread.
    pub fn get(&mut self, vfs: Option<&Arc<RezVfs>>, path: &str) -> Option<Arc<Pcm>> {
        self.absorb();
        let key = key_of(path);
        self.clock += 1;
        if let Some(e) = self.map.get_mut(&key) {
            e.last = self.clock;
            self.stats.hits += 1;
            return e.pcm.clone();
        }
        let t = Instant::now();
        // perf-g1: a sound read and decoded on the frame thread (its read counts as I/O, the WAV decode as parse)
        dc_render::perf::add(dc_render::perf::Counter::SoundLoads, 1);
        let p = vfs.and_then(|v| {
            let bytes = crate::modassets::read_entry(v, path).ok()?;
            let _parse = dc_render::perf::span(dc_render::perf::Counter::ParseNs);
            decode_wav(&bytes).ok().map(Arc::new)
        });
        self.stats.inline_decodes += 1;
        self.stats.inline_max_us = self.stats.inline_max_us.max(t.elapsed().as_micros() as u64);
        crate::watchdog::busy();
        if !self.insert(key, p.clone()) {
            self.stats.rejects += 1;
        }
        p
    }

    /// Installs a decoded sound directly (tests, synthetic sounds).
    pub fn set(&mut self, path: &str, pcm: Pcm) {
        let p = Some(Arc::new(pcm));
        if !self.insert(key_of(path), p) {
            self.stats.rejects += 1;
        }
    }

    /// Queues `path` for the decode thread (async mode only; cached, queued or empty names are skipped). Returns
    /// whether the name was queued.
    pub fn prefetch(&mut self, path: &str) -> bool {
        if !self.async_on || path.trim().is_empty() {
            return false;
        }
        let key = key_of(path);
        if self.map.contains_key(&key) || self.queued.contains(&key) {
            return false;
        }
        if self.pending.len() >= PREFETCH_PENDING {
            self.stats.prefetch_dropped += 1;
            return false;
        }
        self.queued.insert(key.clone());
        self.pending.push_back(key);
        self.stats.prefetch_queued += 1;
        self.stats.hwm_pending = self.stats.hwm_pending.max(self.pending.len());
        true
    }

    /// Takes the finished decodes and hands waiting names to the thread (once per frame; `rom` starts the thread).
    pub fn pump(&mut self, vfs: Option<&Arc<RezVfs>>) {
        self.absorb();
        if !self.async_on || self.pending.is_empty() {
            return;
        }
        if self.worker.is_none() {
            let Some(vfs) = vfs else {
                return;
            };
            self.worker = start_worker(vfs.clone());
            if self.worker.is_none() {
                // no thread: forget the queue (counted), the files decode inline when played
                self.stats.prefetch_dropped += self.pending.len() as u64;
                self.pending.clear();
                self.queued.clear();
                return;
            }
        }
        while self.inflight < DECODE_INFLIGHT {
            let Some(key) = self.pending.pop_front() else {
                break;
            };
            if self.map.contains_key(&key) {
                self.queued.remove(&key);
                continue;
            }
            let Some(w) = self.worker.as_ref() else {
                break;
            };
            match w.tx.try_send(key) {
                Ok(()) => {
                    self.inflight += 1;
                    self.stats.hwm_inflight = self.stats.hwm_inflight.max(self.inflight);
                }
                Err(TrySendError::Full(k)) => {
                    self.pending.push_front(k);
                    break;
                }
                Err(TrySendError::Disconnected(k)) => {
                    self.queued.remove(&k);
                    self.stats.prefetch_dropped += 1 + self.pending.len() as u64;
                    for k in self.pending.drain(..) {
                        self.queued.remove(&k);
                    }
                    self.worker = None;
                    break;
                }
            }
        }
    }

    /// Waits until every queued prefetch is decoded and taken (tests and reports; BLOCKS).
    pub fn settle(&mut self, vfs: Option<&Arc<RezVfs>>) {
        loop {
            self.pump(vfs);
            if self.inflight == 0 && (self.pending.is_empty() || self.worker.is_none()) {
                return;
            }
            let got = self.worker.as_ref().map(|w| {
                dc_hangdump::crumb_wait!("sound cache: settle, wait for a decode", w.rx.recv())
            });
            match got {
                Some(Ok((k, p))) => self.arrived(k, p),
                _ => {
                    self.inflight = 0;
                    self.worker = None;
                }
            }
        }
    }

    fn absorb(&mut self) {
        loop {
            let got = match self.worker.as_ref() {
                Some(w) => w.rx.try_recv(),
                None => return,
            };
            match got {
                Ok((k, p)) => self.arrived(k, p),
                Err(_) => return,
            }
        }
    }

    fn arrived(&mut self, key: String, p: Option<Arc<Pcm>>) {
        self.inflight = self.inflight.saturating_sub(1);
        self.queued.remove(&key);
        if self.map.contains_key(&key) {
            // decoded inline meanwhile: the same samples
            return;
        }
        if self.insert(key, p) {
            self.stats.prefetch_done += 1;
        } else {
            self.stats.prefetch_dropped += 1;
        }
    }

    /// Inserts within the cap (LRU eviction of entries no voice holds); false when it did not fit.
    fn insert(&mut self, key: String, pcm: Option<Arc<Pcm>>) -> bool {
        let bytes = bytes_of(&pcm);
        if self.stats.bytes + bytes > self.cap {
            self.evict(self.stats.bytes + bytes - self.cap);
        }
        if self.stats.bytes + bytes > self.cap {
            return false;
        }
        self.clock += 1;
        self.stats.bytes += bytes;
        self.stats.hwm_bytes = self.stats.hwm_bytes.max(self.stats.bytes);
        self.map.insert(
            key,
            Entry {
                pcm,
                bytes,
                last: self.clock,
            },
        );
        true
    }

    /// Frees at least `need` bytes, least recently used first, skipping files a voice still plays. Ties in `last`
    /// cannot happen (every use takes a new clock value), so the order is deterministic.
    fn evict(&mut self, need: usize) {
        let mut idle: Vec<(u64, String)> = self
            .map
            .iter()
            .filter(|(_, e)| {
                e.bytes > 0 && e.pcm.as_ref().is_some_and(|p| Arc::strong_count(p) == 1)
            })
            .map(|(k, e)| (e.last, k.clone()))
            .collect();
        idle.sort_unstable();
        let mut freed = 0;
        for (_, k) in idle {
            if freed >= need {
                break;
            }
            if let Some(e) = self.map.remove(&k) {
                freed += e.bytes;
                self.stats.bytes -= e.bytes;
                self.stats.evictions += 1;
            }
        }
    }
}

fn start_worker(vfs: Arc<RezVfs>) -> Option<Worker> {
    let (tx, req) = sync_channel::<String>(DECODE_INFLIGHT);
    // at most DECODE_INFLIGHT results exist at a time (the cache sends no more than that)
    let (done, rx) = sync_channel::<(String, Option<Arc<Pcm>>)>(DECODE_INFLIGHT);
    std::thread::Builder::new()
        .name("dc-sound-decode".into())
        .spawn(move || {
            while let Ok(key) =
                dc_hangdump::crumb_wait!("sound decode: wait for a request", req.recv())
            {
                let p = decode_path(&vfs, &key);
                if dc_hangdump::crumb_wait!(
                    "sound decode: hand the samples back",
                    done.send((key, p))
                )
                .is_err()
                {
                    return;
                }
            }
        })
        .ok()?;
    Some(Worker { tx, rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(n: usize) -> Pcm {
        Pcm {
            rate: 22050,
            channels: 1,
            samples: vec![0.25; n],
        }
    }

    #[test]
    fn the_byte_cap_evicts_least_recent_idle_entries_first() {
        // room for three files of 100 samples (400 bytes each)
        let mut c = PcmCache::new(1200);
        c.set("a.wav", pcm(100));
        c.set("b.wav", pcm(100));
        c.set("c.wav", pcm(100));
        // a used again: b is the oldest now
        assert!(c.get(None, "A.WAV").is_some());
        c.set("d.wav", pcm(100));
        assert!(c.contains("a.wav") && !c.contains("b.wav") && c.contains("c.wav"));
        assert_eq!(c.stats.evictions, 1);
        assert!(c.stats.hwm_bytes <= 1200);
        assert_eq!(c.stats.bytes, 1200);
    }

    #[test]
    fn a_playing_file_is_never_evicted_and_an_overflow_is_not_cached() {
        let mut c = PcmCache::new(800);
        c.set("a.wav", pcm(100));
        c.set("b.wav", pcm(100));
        // both are "playing": a voice holds each Arc
        let held = (c.get(None, "a.wav"), c.get(None, "b.wav"));
        c.set("c.wav", pcm(100));
        assert!(c.contains("a.wav") && c.contains("b.wav") && !c.contains("c.wav"));
        assert_eq!((c.stats.rejects, c.stats.evictions), (1, 0));
        drop(held);
        c.set("c.wav", pcm(100));
        assert!(c.contains("c.wav"));
        assert_eq!(c.stats.evictions, 1);
    }

    #[test]
    fn a_file_larger_than_the_cap_plays_but_is_not_kept() {
        let mut c = PcmCache::new(100);
        c.set("big.wav", pcm(1000));
        assert!(!c.contains("big.wav"));
        assert_eq!(c.stats.rejects, 1);
        assert_eq!(c.stats.bytes, 0);
    }

    #[test]
    fn inline_mode_queues_nothing_and_the_pending_list_is_bounded() {
        let mut c = PcmCache::new(1 << 20);
        assert!(!c.prefetch("a.wav"));
        assert_eq!(c.stats.prefetch_queued, 0);
        c.set_async(true);
        for i in 0..PREFETCH_PENDING + 10 {
            c.prefetch(&format!("f{i}.wav"));
        }
        // a name already queued is not queued twice
        assert!(!c.prefetch("f0.wav"));
        assert_eq!(c.stats.prefetch_queued, PREFETCH_PENDING as u64);
        assert_eq!(c.stats.prefetch_dropped, 10);
        assert_eq!(c.stats.hwm_pending, PREFETCH_PENDING);
        // without a ROM no thread starts: pump leaves the queue alone and a miss answers None inline
        c.pump(None);
        assert!(c.get(None, "f1.wav").is_none());
        assert_eq!(c.stats.inline_decodes, 1);
    }

    #[test]
    fn missing_files_are_remembered_without_bytes() {
        let mut c = PcmCache::new(1000);
        assert!(c.get(None, "nothing.wav").is_none());
        assert!(c.contains("NOTHING.WAV"));
        assert!(c.get(None, "nothing.wav").is_none());
        assert_eq!(
            (c.stats.inline_decodes, c.stats.hits, c.stats.bytes),
            (1, 1, 0)
        );
    }

    /// With the ROM: the decode thread and the inline path give the same samples, and settled prefetches are hits.
    #[test]
    fn rom_prefetch_matches_inline_decode() {
        let Some(rom) = std::env::var_os("CA_ROM") else {
            return;
        };
        let vfs = Arc::new(RezVfs::new(std::path::PathBuf::from(rom)));
        let doc = dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", crate::sound::SURFACE_ENTRY);
        let Some(doc) = doc else {
            return;
        };
        let t = crate::sound::SurfaceTable::from_doc(&doc);
        let files: Vec<String> = t
            .rows
            .values()
            .flat_map(|r| r.right.iter().chain(r.left.iter()).cloned())
            .filter(|f| !f.is_empty())
            .take(12)
            .collect();
        assert!(!files.is_empty());
        let mut a = PcmCache::new(CACHE_BYTES);
        a.set_async(true);
        for f in &files {
            a.prefetch(f);
        }
        a.settle(Some(&vfs));
        let mut b = PcmCache::new(CACHE_BYTES);
        for f in &files {
            let pa = a.get(Some(&vfs), f);
            let pb = b.get(Some(&vfs), f);
            assert_eq!(
                pa.as_ref().map(|p| (&p.samples, p.rate, p.channels)),
                pb.as_ref().map(|p| (&p.samples, p.rate, p.channels))
            );
        }
        assert_eq!(a.stats.inline_decodes, 0, "{}", a.stats.line());
        assert!(a.stats.prefetch_done > 0);
        eprintln!("rom prefetch: {} files, {}", files.len(), a.stats.line());
    }
}
