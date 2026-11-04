//! Background music of the front end, the waiting room, the loading screen and the infection mode.
//!
//! The music player itself (two slots, linear crossfade, hard stop) is `dc_audio::bgm::BgmPlayer`, owned by the
//! mixer. This module is the caller the original shell has: it maps a screen state or a mode event to one of the
//! five keys of the client attribute file, resolves the key to a file of the user's ROM, decodes the MP3 on a worker
//! thread (never on the frame thread) and hands the track to the player.
//!
//! Provenance: the rules below are 2009 behaviour; the bounds are our own (docs/RENDER_PERF_GATES.md).
// SPEC: AUDIO_2009.md §8.1 (engine player), §8.2 (who plays what)
// DC-DEFAULT: AUDIO-1 bounded music path: one worker thread, a single-slot request (latest wins), a single
// result slot, files over MAX_FILE_BYTES refused, decoded audio cut at MAX_SAMPLES, two decoded tracks kept
//
// Bounds (the overflow column of the ledger in docs/RENDER_PERF_GATES.md):
//   decode thread        1 worker, never the frame thread
//   request queue        1 slot; a newer cue replaces an older one that has not started decoding (latest wins)
//   result queue         1 slot; a result of a cue that was replaced meanwhile is dropped
//   file size            MAX_FILE_BYTES (8 MiB, the ROM's largest file is 3.4 MB): larger is refused, no music
//   decoded size         MAX_SAMPLES f32 samples (96 MB): longer audio is cut at that length (a looped track
//                        then loops the cut part)
//   resident tracks      TRACK_CACHE (2): the track in the player and the one before it
//   log                  LOG_KEEP lines, oldest dropped

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use dc_audio::bgm::PlayOutcome;
use dc_audio::decode::{decode_mp3, Pcm};
use dc_audio::mixer::Mixer;
use dc_audio::KeyId;
use dc_bute::ButeDoc;

/// Largest music file read (bytes).
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Most decoded samples (all channels) kept of one track: 96 MB of f32.
pub const MAX_SAMPLES: usize = 24 * 1024 * 1024;
/// Decoded tracks kept resident.
pub const TRACK_CACHE: usize = 2;
/// Most log lines kept.
pub const LOG_KEEP: usize = 64;
/// Most music events waiting for the sound log (oldest dropped).
pub const EVENT_KEEP: usize = 64;

/// The section of the client attribute file that holds the music keys (the interface sound section).
// EVIDENCE: cshell+0x1b2c14 (the section literal pushed by the lookup the music callers use), sha c26b3ed4ce70e202
pub const SECTION: KeyId = KeyId::new("c26b3ed4ce70e202", 9);

/// The five music keys of the client attribute file's interface section.
// EVIDENCE: cshell+0x1fba00 (the waiting-room and front-end keys), cshell+0x118bc0 / cshell+0x11a300 /
// cshell+0x1630a0 (loading),
// cshell+0x28dd40 / cshell+0xbe0c0 (infection start), cshell+0xbe570 (infection end)
// SPEC: AUDIO_2009.md §8.2 (key hashes and lengths)
pub const KEY_LOADING: KeyId = KeyId::new("da12ca4109dab384", 16);
pub const KEY_WAITING: KeyId = KeyId::new("2537f0c81ab4149f", 14);
pub const KEY_FRONT_END: KeyId = KeyId::new("00d4060b7bde3a74", 10);
pub const KEY_INFECTION: KeyId = KeyId::new("c8fb4e436daa42f9", 17);
pub const KEY_INFECTION_END: KeyId = KeyId::new("4e474505112a31a9", 20);

/// CLI-1599: the classic front-end track (the login screen, the server and channel lists, the shop, the black
/// market: every screen state but the waiting room).
// OBSERVED: user's ROM Game/BGM (the attribute file's front-end and waiting-room keys both name one other file,
// AUDIO_2009.md §8.2; UIBGM_LOOP and WAITINGROOM_LOOP are among its unreferenced files)
// DC-DEFAULT: CLI-1599 (proposed, USER REQUEST 2026-10-31) the "Classic" BGM style
pub const CLASSIC_FRONT_END: &str = "BGM\\UIBGM_LOOP.mp3";
/// CLI-1599: the classic waiting-room track (screen state 9: a room); the front end's loop comes back on leaving.
pub const CLASSIC_WAITING: &str = "BGM\\WAITINGROOM_LOOP.mp3";

/// What the shell asks the music player to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicCue {
    /// Any front-end menu screen: loop, 1 s fade.
    FrontEnd,
    /// The waiting room (screen state 9): loop, 1 s fade.
    WaitingRoom,
    /// Game launch from the lobby server's game-start order (the online room path): stop, then the loading track
    /// once, no fade. The ROM's loading track is a 2 s sting, so it ends by itself before the round starts.
    Loading,
    /// The other two launch paths of the shell (the local launch and the direct join): loop, 1 s fade. Kept for the
    /// record; this client launches only through the lobby, so nothing issues it.
    LoadingLocal,
    /// The match's first in-world update. 2009 changes no music there (no stop, no play; in-match music in normal
    /// modes: none found), so this cue only records the music state in the sound log.
    MatchStart,
    /// The infection-style mode's start: loop, 1 s fade.
    InfectionStart,
    /// The end of that mode's round: stop, then the end track once, no fade.
    InfectionEnd,
    /// Stop at once (screen state 0).
    Stop,
}

/// A server's music order (S2C 0xd9 `GS_MUSIC`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MusicOrder {
    /// op 0: stop at once.
    Stop,
    /// op 1: play `file` (relative to the game folder); `looped` from the order's flag; `p` is the order's 32-bit
    /// word, kept for the log (its use is not traced).
    Play { file: String, looped: bool, p: u32 },
}

/// Longest file name of a 0xd9 order (the reader's buffer of 261 units, the terminator included).
pub const ORDER_FILE_MAX: usize = 260;

/// Reads the body of a 0xd9 message (after the shell id). `None` for an op other than 0 / 1 or a short body.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.2 row 0xd9: `[op:u8]` then op 1: `[loop:bool1][p:u32][file:zstr8(261)]`
// (cshell+0x5de30)
pub fn read_music_order(r: &mut dc_wire::udp::BitReader<'_>) -> Option<MusicOrder> {
    if r.remaining_bits() < 8 {
        return None;
    }
    match r.read_u8() {
        0 => Some(MusicOrder::Stop),
        1 => {
            if r.remaining_bits() < 1 + 32 + 8 {
                return None;
            }
            let looped = r.read_bit();
            let p = r.read_u32();
            let mut units = r.read_zstr8();
            units.truncate(ORDER_FILE_MAX);
            let file = units.iter().map(|&u| u as char).collect();
            Some(MusicOrder::Play { file, looped, p })
        }
        _ => None,
    }
}

/// One thing the music did, for the sound log (the sound director drains these into `music` lines).
// DC-DEFAULT: AUDIO-3 (staged) every music cue, server order, start and stop is one sound-log line
#[derive(Debug, Clone, PartialEq)]
pub struct MusicEvent {
    /// `cue` (a shell cue), `order` (a 0xd9 order), `play` (a track started in the player), `stop` (both slots
    /// closed).
    pub op: &'static str,
    /// The cue's name (`Loading`, `MatchStart`, ...) or `server order`.
    pub label: String,
    /// The resource path (memory only; the log writes its hash).
    pub key: Option<String>,
    pub looped: bool,
    pub fade_s: f32,
    /// The track's length (`play` only).
    pub dur_s: Option<f64>,
}

/// How a cue plays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CueRule {
    /// The attribute key (`None`: the cue only stops).
    pub key: Option<KeyId>,
    pub looped: bool,
    pub fade_s: f32,
    /// Stop both slots before the new track (`stop`, then `play`).
    pub stop_first: bool,
}

impl MusicCue {
    // EVIDENCE: cshell+0x1fba00 (loop, fade 1.0), cshell+0x118bc0 / cshell+0x11a300 (loop, fade 1.0),
    // cshell+0x165650 -> cshell+0x1630a0 (the lobby game-start handler: music stop, then the loading key with
    // loop 0 and fade 0), cshell+0x28dd40 / cshell+0xbe0c0 (loop, fade 1.0), cshell+0xbe570 (stop, then once with
    // no fade)
    // SPEC: AUDIO_2009.md §8.2 table
    pub fn rule(self) -> CueRule {
        let looped = |key| CueRule {
            key: Some(key),
            looped: true,
            fade_s: 1.0,
            stop_first: false,
        };
        match self {
            MusicCue::FrontEnd => looped(KEY_FRONT_END),
            MusicCue::WaitingRoom => looped(KEY_WAITING),
            MusicCue::Loading => CueRule {
                key: Some(KEY_LOADING),
                looped: false,
                fade_s: 0.0,
                stop_first: true,
            },
            MusicCue::LoadingLocal => looped(KEY_LOADING),
            MusicCue::InfectionStart => looped(KEY_INFECTION),
            MusicCue::InfectionEnd => CueRule {
                key: Some(KEY_INFECTION_END),
                looped: false,
                fade_s: 0.0,
                stop_first: true,
            },
            MusicCue::Stop => CueRule {
                key: None,
                looped: false,
                fade_s: 0.0,
                stop_first: true,
            },
            MusicCue::MatchStart => CueRule {
                key: None,
                looped: false,
                fade_s: 0.0,
                stop_first: false,
            },
        }
    }
}

/// The cue a screen state gives: state 0 stops, state 7 (the result screen) keeps the current music (`None`),
/// state 9 is the waiting room, every other state the front end.
// EVIDENCE: cshell+0x1fba00 (the state switch: 0 stop, 7 nothing, 9 the waiting-room key, others the front-end key)
// SPEC: AUDIO_2009.md §8.2 table rows 3 and 4
pub fn cue_for_screen_state(state: u32) -> Option<MusicCue> {
    match state {
        0 => Some(MusicCue::Stop),
        7 => None,
        9 => Some(MusicCue::WaitingRoom),
        _ => Some(MusicCue::FrontEnd),
    }
}

/// The five tracks of the attribute file as resource paths (empty where the key is missing).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MusicFiles {
    pub loading: String,
    pub waiting: String,
    pub front_end: String,
    pub infection: String,
    pub infection_end: String,
}

impl MusicFiles {
    /// Finds the interface section and the five keys by their hashes (the names stay in the ROM).
    pub fn from_doc(doc: &ButeDoc) -> MusicFiles {
        let mut f = MusicFiles::default();
        let Some((_, sec)) = doc.iter().find(|(name, _)| SECTION.matches(name)) else {
            return f;
        };
        for (k, v) in sec.iter() {
            let v = v.trim().to_string();
            if KEY_LOADING.matches(k) {
                f.loading = v;
            } else if KEY_WAITING.matches(k) {
                f.waiting = v;
            } else if KEY_FRONT_END.matches(k) {
                f.front_end = v;
            } else if KEY_INFECTION.matches(k) {
                f.infection = v;
            } else if KEY_INFECTION_END.matches(k) {
                f.infection_end = v;
            }
        }
        f
    }

    pub fn path_of(&self, key: KeyId) -> &str {
        if key == KEY_LOADING {
            &self.loading
        } else if key == KEY_WAITING {
            &self.waiting
        } else if key == KEY_FRONT_END {
            &self.front_end
        } else if key == KEY_INFECTION {
            &self.infection
        } else {
            &self.infection_end
        }
    }
}

/// Finds `rel` (backslash or slash separated, any case) under `root`, trying `root` itself and then its `Game`
/// folder (the attribute values are resource paths; the music files sit in the game folder's BGM folder).
// OBSERVED: user's ROM: the 14 music files are loose files in <ROM>/Game/BGM (not in a rez archive)
pub fn resolve_file(root: &Path, rel: &str) -> Option<PathBuf> {
    fn walk(base: &Path, parts: &[&str]) -> Option<PathBuf> {
        let mut cur = base.to_path_buf();
        for p in parts {
            let direct = cur.join(p);
            if direct.exists() {
                cur = direct;
                continue;
            }
            let hit = std::fs::read_dir(&cur).ok()?.flatten().find(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| dc_bute::eq_fold(n, p))
            })?;
            cur = hit.path();
        }
        cur.is_file().then_some(cur)
    }
    let parts: Vec<&str> = rel
        .split(['\\', '/'])
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .collect();
    if parts.is_empty() {
        return None;
    }
    walk(root, &parts).or_else(|| walk(&root.join("Game"), &parts))
}

/// Reads and decodes one music file within the bounds above.
pub fn load_track(path: &Path) -> Result<Pcm, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_FILE_BYTES {
        return Err(format!(
            "file of {len} bytes over the {MAX_FILE_BYTES} byte bound"
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut pcm = decode_mp3(&bytes).map_err(|e| e.to_string())?;
    if pcm.samples.len() > MAX_SAMPLES {
        let ch = usize::from(pcm.channels.max(1));
        pcm.samples.truncate(MAX_SAMPLES / ch * ch);
    }
    if pcm.samples.is_empty() {
        return Err("no samples".into());
    }
    Ok(pcm)
}

type Loader = Arc<dyn Fn(&Path) -> Result<Pcm, String> + Send + Sync>;

struct Req {
    gen: u64,
    path: PathBuf,
}

struct Done {
    gen: u64,
    result: Result<Pcm, String>,
}

#[derive(Default)]
struct Slots {
    req: Option<Req>,
    done: Option<Done>,
    shutdown: bool,
}

struct Shared {
    slots: Mutex<Slots>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Slots> {
        dc_hangdump::crumb_wait!("music: lock the hand-over slots", self.slots.lock())
            .unwrap_or_else(|e| e.into_inner())
    }
}

/// A cue waiting for its track.
#[derive(Debug, Clone)]
struct Pending {
    gen: u64,
    label: String,
    key: String,
    rule: CueRule,
}

/// Plays the cues. One per sound director; every call is cheap and never decodes.
pub struct MusicDirector {
    root: Option<PathBuf>,
    pub files: MusicFiles,
    /// CLI-1599: the classic style (the default): the front-end and waiting-room keys play [`CLASSIC_FRONT_END`]
    /// and [`CLASSIC_WAITING`] instead of the attribute file's tracks.
    classic: bool,
    shared: Arc<Shared>,
    worker: bool,
    loader: Loader,
    next_gen: u64,
    pending: Option<Pending>,
    cache: VecDeque<(String, Arc<Pcm>)>,
    /// What happened, newest last (bounded by [`LOG_KEEP`]).
    pub log: VecDeque<String>,
    /// Events for the sound log, oldest first (bounded by [`EVENT_KEEP`]); see [`MusicDirector::take_events`].
    pub events: VecDeque<MusicEvent>,
}

impl Default for MusicDirector {
    fn default() -> Self {
        Self::none()
    }
}

impl Drop for MusicDirector {
    fn drop(&mut self) {
        self.shared.lock().shutdown = true;
        self.shared.cv.notify_all();
    }
}

impl MusicDirector {
    /// A director without files: every cue is logged and does nothing (no ROM).
    pub fn none() -> MusicDirector {
        MusicDirector {
            root: None,
            files: MusicFiles::default(),
            classic: crate::settings::BGM_DEFAULT == crate::settings::BGM_CLASSIC,
            shared: Arc::new(Shared {
                slots: Mutex::new(Slots::default()),
                cv: Condvar::new(),
            }),
            worker: false,
            loader: Arc::new(load_track),
            next_gen: 0,
            pending: None,
            cache: VecDeque::new(),
            log: VecDeque::new(),
            events: VecDeque::new(),
        }
    }

    /// The director of a ROM: its root folder and the client attribute file's music keys.
    pub fn from_rom(root: &Path, client_doc: &ButeDoc) -> MusicDirector {
        let mut d = MusicDirector::none();
        d.root = Some(root.to_path_buf());
        d.files = MusicFiles::from_doc(client_doc);
        d
    }

    /// Replaces the file loader (tests: synthetic tracks without MP3 files).
    pub fn with_loader(
        mut self,
        l: impl Fn(&Path) -> Result<Pcm, String> + Send + Sync + 'static,
    ) -> MusicDirector {
        self.loader = Arc::new(l);
        self
    }

    /// Sets the file table directly (tests).
    pub fn with_files(mut self, root: PathBuf, files: MusicFiles) -> MusicDirector {
        self.root = Some(root);
        self.files = files;
        self
    }

    /// CLI-1599: switches the BGM style; true when it changed (the caller cues the current screen again, so the
    /// track crossfades to the other style's).
    pub fn set_classic(&mut self, on: bool) -> bool {
        let changed = self.classic != on;
        self.classic = on;
        changed
    }

    pub fn classic(&self) -> bool {
        self.classic
    }

    /// The resource path a key plays: the classic track for the front-end and waiting-room keys in the classic
    /// style (when the ROM has the file; else the attribute file's track, so a ROM without the loops still plays),
    /// else the attribute file's.
    pub fn track_of(&self, key: KeyId) -> String {
        let classic = if !self.classic {
            None
        } else if key == KEY_FRONT_END {
            Some(CLASSIC_FRONT_END)
        } else if key == KEY_WAITING {
            Some(CLASSIC_WAITING)
        } else {
            None
        };
        match classic {
            Some(c)
                if self
                    .root
                    .as_deref()
                    .is_some_and(|r| resolve_file(r, c).is_some()) =>
            {
                c.to_string()
            }
            _ => self.files.path_of(key).to_string(),
        }
    }

    fn note(&mut self, s: String) {
        if self.log.len() >= LOG_KEEP {
            self.log.pop_front();
        }
        self.log.push_back(s);
    }

    fn event(
        &mut self,
        op: &'static str,
        label: &str,
        key: Option<&str>,
        rule: CueRule,
        dur_s: Option<f64>,
    ) {
        if self.events.len() >= EVENT_KEEP {
            self.events.pop_front();
        }
        self.events.push_back(MusicEvent {
            op,
            label: label.to_string(),
            key: key.map(str::to_string),
            looped: rule.looped,
            fade_s: rule.fade_s,
            dur_s,
        });
    }

    /// The events since the last call, oldest first.
    pub fn take_events(&mut self) -> Vec<MusicEvent> {
        self.events.drain(..).collect()
    }

    /// True while a track is being decoded for a cue.
    pub fn loading(&self) -> bool {
        self.pending.is_some()
    }

    /// Runs one cue against the mixer's player.
    // SPEC: AUDIO_2009.md §8.1 `play` step 1: a track that is playing under the same path is not restarted
    pub fn cue(&mut self, mixer: &mut Mixer, cue: MusicCue, now_s: f64) {
        let rule = cue.rule();
        let label = format!("{cue:?}");
        let path = rule.key.map(|k| self.track_of(k));
        self.event(
            "cue",
            &label,
            path.as_deref().filter(|p| !p.is_empty()),
            rule,
            None,
        );
        if rule.stop_first {
            // the stop closes both slots at once and clears the path; a waiting request is void as well
            let was = mixer.bgm.current_key().map(str::to_string);
            mixer.bgm.stop();
            self.pending = None;
            self.shared.lock().req = None;
            self.event("stop", &label, was.as_deref(), rule, None);
        }
        let Some(path) = path else {
            let what = if rule.stop_first { "stop" } else { "no change" };
            self.note(format!("{cue:?}: {what}"));
            return;
        };
        if path.is_empty() {
            self.note(format!("{cue:?}: no file for the key"));
            return;
        }
        self.request(mixer, &format!("{cue:?}"), path, rule, now_s);
    }

    /// Runs a server's music order (S2C 0xd9): stop at once, or play a file of the game folder.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.2 row 0xd9 (op 0: stop; op 1: the file with its loop flag; the play
    // reading of op 1 is PLAUSIBLE there)
    // DC-DEFAULT: AUDIO-2 (staged) a server-named track starts without a fade; the order's `p` word is not used
    pub fn order(&mut self, mixer: &mut Mixer, order: &MusicOrder, now_s: f64) {
        match order {
            MusicOrder::Stop => {
                self.event("order", "server order", None, MusicCue::Stop.rule(), None);
                self.cue(mixer, MusicCue::Stop, now_s)
            }
            MusicOrder::Play { file, looped, .. } => {
                let rule = CueRule {
                    key: None,
                    looped: *looped,
                    fade_s: 0.0,
                    stop_first: false,
                };
                self.event("order", "server order", Some(file.as_str()), rule, None);
                if file.is_empty() {
                    self.note("server order: no file".into());
                    return;
                }
                self.request(mixer, "server order", file.clone(), rule, now_s);
            }
        }
    }

    /// Plays `path` (a ROM-relative file) by `rule`: no restart of the same track, a cached track at once, else one
    /// decode request (latest wins).
    fn request(&mut self, mixer: &mut Mixer, label: &str, path: String, rule: CueRule, now_s: f64) {
        if mixer.bgm.is_playing()
            && mixer
                .bgm
                .current_key()
                .is_some_and(|c| dc_bute::eq_fold(c, &path))
        {
            // the same file under another cue (front end and waiting room share one) keeps playing
            self.pending = None;
            self.note(format!("{label}: already playing"));
            return;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|p| dc_bute::eq_fold(&p.key, &path))
        {
            if let Some(p) = &mut self.pending {
                p.rule = rule;
                p.label = label.to_string();
            }
            return;
        }
        if let Some(pcm) = self.cached(&path) {
            self.pending = None;
            self.install(mixer, label, &path, pcm, rule, now_s);
            return;
        }
        let Some(file) = self.root.as_deref().and_then(|r| resolve_file(r, &path)) else {
            self.note(format!("{label}: file not found"));
            self.pending = None;
            return;
        };
        self.next_gen += 1;
        let gen = self.next_gen;
        self.pending = Some(Pending {
            gen,
            label: label.to_string(),
            key: path,
            rule,
        });
        self.spawn_worker();
        // latest wins: a request that has not started is replaced
        self.shared.lock().req = Some(Req { gen, path: file });
        self.shared.cv.notify_all();
    }

    fn cached(&mut self, key: &str) -> Option<Arc<Pcm>> {
        let i = self
            .cache
            .iter()
            .position(|(k, _)| dc_bute::eq_fold(k, key))?;
        let e = self.cache.remove(i)?;
        let pcm = e.1.clone();
        self.cache.push_back(e);
        Some(pcm)
    }

    fn install(
        &mut self,
        mixer: &mut Mixer,
        label: &str,
        key: &str,
        pcm: Arc<Pcm>,
        rule: CueRule,
        now_s: f64,
    ) {
        let dur_s = pcm.duration_s();
        let out = mixer.bgm.play(key, pcm, rule.looped, rule.fade_s, now_s);
        if out != PlayOutcome::Ignored {
            self.event("play", label, Some(key), rule, Some(dur_s));
        }
        let word = match out {
            PlayOutcome::Ignored => "ignored",
            PlayOutcome::Started => "started",
            PlayOutcome::Crossfading => "crossfading",
        };
        self.note(format!(
            "play {word} loop {} fade {}",
            rule.looped, rule.fade_s
        ));
    }

    fn spawn_worker(&mut self) {
        if self.worker {
            return;
        }
        self.worker = true;
        let shared = self.shared.clone();
        let loader = self.loader.clone();
        let spawned = std::thread::Builder::new()
            .name("dc-music-decode".into())
            .spawn(move || loop {
                let req = {
                    let mut g = shared.lock();
                    loop {
                        if g.shutdown {
                            return;
                        }
                        if let Some(r) = g.req.take() {
                            break r;
                        }
                        g = dc_hangdump::crumb_wait!(
                            "music decode: wait for a request",
                            shared.cv.wait(g)
                        )
                        .unwrap_or_else(|e| e.into_inner());
                    }
                };
                let result = loader(&req.path);
                let mut g = shared.lock();
                g.done = Some(Done {
                    gen: req.gen,
                    result,
                });
            });
        if spawned.is_err() {
            self.worker = false;
            self.note("decode thread failed to start".into());
        }
    }

    /// Takes a finished decode and starts it in the player. Call once per frame (menu or match).
    pub fn poll(&mut self, mixer: &mut Mixer, now_s: f64) {
        let Some(done) = self.shared.lock().done.take() else {
            return;
        };
        let Some(p) = self.pending.clone().filter(|p| p.gen == done.gen) else {
            return; // the cue was replaced or stopped meanwhile: the result is dropped
        };
        self.pending = None;
        match done.result {
            Ok(pcm) => {
                let pcm = Arc::new(pcm);
                if self.cache.len() >= TRACK_CACHE {
                    self.cache.pop_front();
                }
                self.cache.push_back((p.key.clone(), pcm.clone()));
                self.install(mixer, &p.label, &p.key, pcm, p.rule, now_s);
            }
            Err(e) => self.note(format!("decode failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_audio::OUTPUT_RATE;
    use std::time::{Duration, Instant};

    fn tone() -> Pcm {
        Pcm {
            rate: OUTPUT_RATE,
            channels: 2,
            samples: vec![0.25; 4000],
        }
    }

    fn files() -> MusicFiles {
        MusicFiles {
            loading: "BGM\\load.mp3".into(),
            waiting: "BGM\\ui.mp3".into(),
            front_end: "bgm\\UI.MP3".into(),
            infection: "BGM\\zed.mp3".into(),
            infection_end: "BGM\\end.mp3".into(),
        }
    }

    /// A scratch ROM folder with tiny files (the injected loader never decodes them).
    fn rom(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc_music_{tag}_{}", std::process::id()));
        let b = d.join("Game").join("BGM");
        std::fs::create_dir_all(&b).unwrap();
        for f in [
            "load.mp3",
            "ui.mp3",
            "zed.mp3",
            "end.mp3",
            "UIBGM_LOOP.mp3",
            "WAITINGROOM_LOOP.mp3",
        ] {
            std::fs::write(b.join(f), b"x").unwrap();
        }
        d
    }

    /// CLI-1599: the classic style plays the UI loop for the front end and crossfades to the waiting-room loop in
    /// a room (and back on leaving); the 2009 style keeps the attribute file's tracks.
    #[test]
    fn the_classic_style_swaps_the_front_end_and_waiting_room_tracks() {
        let root = rom("classic");
        let mut d = MusicDirector::none()
            .with_loader(|_| Ok(tone()))
            .with_files(root.clone(), files());
        let mut m = Mixer::new();
        assert!(d.classic(), "Classic is the default");
        assert!(!d.set_classic(true), "no change");
        d.cue(&mut m, MusicCue::FrontEnd, 0.0);
        pump(&mut d, &mut m, 0.0);
        assert!(m
            .bgm
            .current_key()
            .is_some_and(|k| dc_bute::eq_fold(k, CLASSIC_FRONT_END)));
        d.cue(&mut m, MusicCue::WaitingRoom, 1.0);
        pump(&mut d, &mut m, 1.0);
        assert!(m
            .bgm
            .current_key()
            .is_some_and(|k| dc_bute::eq_fold(k, CLASSIC_WAITING)));
        let (a, b) = m.bgm.gains();
        assert!(a.is_some() && b.is_some(), "a 1 s crossfade: {a:?} {b:?}");
        d.cue(&mut m, MusicCue::FrontEnd, 3.0);
        pump(&mut d, &mut m, 3.0);
        assert!(m
            .bgm
            .current_key()
            .is_some_and(|k| dc_bute::eq_fold(k, CLASSIC_FRONT_END)));
        // back to 2009: the attribute file's front-end track
        assert!(d.set_classic(false));
        d.cue(&mut m, MusicCue::FrontEnd, 5.0);
        pump(&mut d, &mut m, 5.0);
        assert!(m
            .bgm
            .current_key()
            .is_some_and(|k| dc_bute::eq_fold(k, "bgm\\UI.MP3")));
        std::fs::remove_dir_all(root).ok();
    }

    fn pump(d: &mut MusicDirector, m: &mut Mixer, t: f64) {
        let end = Instant::now() + Duration::from_secs(30);
        while d.loading() && Instant::now() < end {
            d.poll(m, t);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!d.loading(), "decode did not finish: {:?}", d.log);
    }

    #[test]
    fn server_orders_are_read_by_the_0xd9_layout() {
        use dc_wire::udp::{BitReader, BitWriter};
        let mut w = BitWriter::new();
        w.write_u8(1)
            .write_bit(true)
            .write_u32(7)
            .write_zstr8(b"BGM/x.mp3");
        let mut r = BitReader::with_bit_len(w.as_bytes(), w.bit_len());
        assert_eq!(
            read_music_order(&mut r),
            Some(MusicOrder::Play {
                file: "BGM/x.mp3".into(),
                looped: true,
                p: 7
            })
        );
        let mut w = BitWriter::new();
        w.write_u8(0);
        let mut r = BitReader::with_bit_len(w.as_bytes(), w.bit_len());
        assert_eq!(read_music_order(&mut r), Some(MusicOrder::Stop));
        // an unknown op and a short op 1 are not orders
        for body in [vec![2u8], vec![1u8, 0x80]] {
            let mut r = BitReader::with_bit_len(&body, body.len() * 8);
            assert_eq!(read_music_order(&mut r), None, "{body:?}");
        }
        // a name longer than the reader's buffer is cut
        let long = vec![b'a'; 400];
        let mut w = BitWriter::new();
        w.write_u8(1)
            .write_bit(false)
            .write_u32(0)
            .write_zstr8(&long);
        let mut r = BitReader::with_bit_len(w.as_bytes(), w.bit_len());
        match read_music_order(&mut r) {
            Some(MusicOrder::Play { file, looped, .. }) => {
                assert_eq!((file.len(), looped), (ORDER_FILE_MAX, false))
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_server_order_plays_its_file_without_a_fade_and_stops() {
        let root = rom("so");
        let mut d = MusicDirector::none()
            .with_loader(|_| Ok(tone()))
            .with_files(root.clone(), files());
        let mut m = Mixer::new();
        let play = MusicOrder::Play {
            file: "bgm/ZED.mp3".into(),
            looped: true,
            p: 0,
        };
        d.order(&mut m, &play, 0.0);
        pump(&mut d, &mut m, 0.0);
        assert!(m.bgm.is_playing());
        assert_eq!(m.bgm.gains(), (Some(1.0), None), "no fade");
        // the same file again: no restart
        d.order(&mut m, &play, 1.0);
        assert!(!d.loading());
        assert_eq!(
            d.log.back().map(String::as_str),
            Some("server order: already playing")
        );
        // a file the ROM does not have changes nothing
        d.order(
            &mut m,
            &MusicOrder::Play {
                file: "BGM/none.mp3".into(),
                looped: false,
                p: 0,
            },
            2.0,
        );
        assert!(m.bgm.is_playing());
        d.order(&mut m, &MusicOrder::Stop, 3.0);
        assert!(!m.bgm.is_playing());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn screen_states_map() {
        assert_eq!(cue_for_screen_state(0), Some(MusicCue::Stop));
        assert_eq!(cue_for_screen_state(7), None);
        assert_eq!(cue_for_screen_state(9), Some(MusicCue::WaitingRoom));
        for s in [0x11, 0xb, 0xc, 0xf, 0x8, 1, 14] {
            assert_eq!(cue_for_screen_state(s), Some(MusicCue::FrontEnd), "{s}");
        }
    }

    #[test]
    fn cue_rules() {
        let r = MusicCue::InfectionEnd.rule();
        assert!(r.stop_first && !r.looped && r.fade_s == 0.0 && r.key == Some(KEY_INFECTION_END));
        for c in [
            MusicCue::FrontEnd,
            MusicCue::WaitingRoom,
            MusicCue::LoadingLocal,
            MusicCue::InfectionStart,
        ] {
            let r = c.rule();
            assert!(r.looped && r.fade_s == 1.0 && !r.stop_first, "{c:?}");
        }
        // the lobby launch: stop, then the loading track once with no fade (cshell+0x1630a0)
        let r = MusicCue::Loading.rule();
        assert!(r.stop_first && !r.looped && r.fade_s == 0.0 && r.key == Some(KEY_LOADING));
        assert_eq!(MusicCue::Stop.rule().key, None);
        let r = MusicCue::MatchStart.rule();
        assert!(
            r.key.is_none() && !r.stop_first,
            "the match start changes nothing"
        );
    }

    #[test]
    fn front_end_to_waiting_room_keeps_one_track() {
        let root = rom("fw");
        let mut d = MusicDirector::none()
            .with_loader(|_| Ok(tone()))
            .with_files(root.clone(), files());
        // the 2009 style: the attribute file's tracks (Classic is the default, CLI-1599)
        d.set_classic(false);
        let mut m = Mixer::new();
        d.cue(&mut m, MusicCue::FrontEnd, 0.0);
        assert!(d.loading());
        pump(&mut d, &mut m, 0.1);
        assert!(m.bgm.is_playing());
        m.bgm.update(0.6);
        let (g, _) = m.bgm.gains();
        assert!(
            g.unwrap() > 0.3 && g.unwrap() < 0.7,
            "fade in from silence: {g:?}"
        );
        // same file under another case: no restart, no new decode
        d.cue(&mut m, MusicCue::WaitingRoom, 5.0);
        assert!(!d.loading());
        assert_eq!(
            d.log.back().map(String::as_str),
            Some("WaitingRoom: already playing")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn crossfade_loading_then_stop_and_end_track() {
        let root = rom("cf");
        let mut d = MusicDirector::none()
            .with_loader(|_| Ok(tone()))
            .with_files(root.clone(), files());
        let mut m = Mixer::new();
        d.cue(&mut m, MusicCue::FrontEnd, 0.0);
        pump(&mut d, &mut m, 0.0);
        m.bgm.update(2.0);
        d.cue(&mut m, MusicCue::LoadingLocal, 3.0);
        pump(&mut d, &mut m, 3.0);
        m.bgm.update(3.5);
        let (c, n) = m.bgm.gains();
        assert!((c.unwrap() - 0.5).abs() < 1e-4 && (n.unwrap() - 0.5).abs() < 1e-4);
        m.bgm.update(4.5);
        assert_eq!(m.bgm.gains(), (Some(1.0), None));
        // the end of an infection round: hard stop, then the end track once at full gain
        d.cue(&mut m, MusicCue::InfectionEnd, 9.0);
        assert!(!m.bgm.is_playing());
        pump(&mut d, &mut m, 9.0);
        assert!(m.bgm.is_playing());
        assert_eq!(m.bgm.gains(), (Some(1.0), None));
        d.cue(&mut m, MusicCue::Stop, 10.0);
        assert!(!m.bgm.is_playing() && m.bgm.current_key().is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn latest_cue_wins_and_stale_result_is_dropped() {
        let root = rom("lw");
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let g2 = gate.clone();
        let mut d = MusicDirector::none()
            .with_loader(move |_| {
                let _g = g2.lock().unwrap_or_else(|e| e.into_inner());
                Ok(tone())
            })
            .with_files(root.clone(), files());
        let mut m = Mixer::new();
        d.cue(&mut m, MusicCue::FrontEnd, 0.0); // the worker takes this one and blocks in the loader
        std::thread::sleep(Duration::from_millis(200));
        d.cue(&mut m, MusicCue::InfectionStart, 0.1); // waits in the single request slot
        d.cue(&mut m, MusicCue::Loading, 0.2); // replaces it
        drop(held);
        pump(&mut d, &mut m, 0.3);
        assert_eq!(m.bgm.current_key(), Some("BGM\\load.mp3"));
        assert_eq!(d.log.iter().filter(|l| l.starts_with("play")).count(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn stop_voids_a_pending_decode() {
        let root = rom("sp");
        let mut d = MusicDirector::none()
            .with_loader(|_| Ok(tone()))
            .with_files(root.clone(), files());
        let mut m = Mixer::new();
        d.cue(&mut m, MusicCue::FrontEnd, 0.0);
        d.cue(&mut m, MusicCue::Stop, 0.0);
        assert!(!d.loading());
        std::thread::sleep(Duration::from_millis(100));
        d.poll(&mut m, 0.1);
        assert!(!m.bgm.is_playing());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_files_and_oversize_are_refused() {
        let mut d = MusicDirector::none();
        d.set_classic(false);
        let mut m = Mixer::new();
        d.cue(&mut m, MusicCue::FrontEnd, 0.0);
        assert_eq!(
            d.log.back().map(String::as_str),
            Some("FrontEnd: no file for the key")
        );
        let root = rom("ov");
        let big = root.join("Game").join("BGM").join("big.mp3");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        let e = load_track(&big).unwrap_err();
        assert!(e.contains("over the"), "{e}");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn lobby_launch_plays_the_loading_track_once_and_the_match_start_keeps_it() {
        let root = rom("ll");
        let mut d = MusicDirector::none()
            .with_loader(|_| Ok(tone()))
            .with_files(root.clone(), files());
        let mut m = Mixer::new();
        d.cue(&mut m, MusicCue::WaitingRoom, 0.0);
        pump(&mut d, &mut m, 0.0);
        d.take_events();
        d.cue(&mut m, MusicCue::Loading, 5.0);
        // the room music is cut at once (no fade-out), the loading track starts at full gain
        assert!(!m.bgm.is_playing());
        pump(&mut d, &mut m, 5.0);
        let st = m.bgm.status().unwrap();
        assert!(!st.looped && st.gain == 1.0, "{st:?}");
        let ops: Vec<(&str, String)> = d
            .take_events()
            .into_iter()
            .map(|e| (e.op, e.label))
            .collect();
        assert_eq!(
            ops,
            [
                ("cue", "Loading".to_string()),
                ("stop", "Loading".to_string()),
                ("play", "Loading".to_string())
            ]
        );
        // the track runs out by itself
        let mut buf = vec![0.0f32; 2 * 4096];
        m.bgm.render_add(&mut buf, 4096);
        assert!(!m.bgm.is_playing());
        // the match start changes nothing and logs one cue
        d.cue(&mut m, MusicCue::MatchStart, 6.0);
        assert!(!m.bgm.is_playing() && !d.loading());
        let ev = d.take_events();
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].op, ev[0].label.as_str()), ("cue", "MatchStart"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn events_are_bounded() {
        let mut d = MusicDirector::none();
        let mut m = Mixer::new();
        for _ in 0..(EVENT_KEEP * 2) {
            d.cue(&mut m, MusicCue::MatchStart, 0.0);
        }
        assert_eq!(d.events.len(), EVENT_KEEP);
    }

    #[test]
    fn log_is_bounded() {
        let mut d = MusicDirector::none();
        for i in 0..(LOG_KEEP * 2) {
            d.note(format!("{i}"));
        }
        assert_eq!(d.log.len(), LOG_KEEP);
    }

    /// The user's ROM: the attribute file names all five keys, the files resolve, and every track decodes within
    /// the bounds (counts only, no ROM names printed).
    #[test]
    fn rom_keys_resolve_and_decode() {
        let Ok(rom) = std::env::var("CA_ROM") else {
            return;
        };
        let rom = PathBuf::from(rom);
        let vfs = dc_rez::RezVfs::new(&rom);
        let doc = ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\CLIENTBUTES.TXT")
            .expect("client attribute file");
        let f = MusicFiles::from_doc(&doc);
        let mut distinct = std::collections::BTreeSet::new();
        for (k, name) in [
            (KEY_LOADING, "loading"),
            (KEY_WAITING, "waiting"),
            (KEY_FRONT_END, "front end"),
            (KEY_INFECTION, "infection"),
            (KEY_INFECTION_END, "infection end"),
        ] {
            let p = f.path_of(k);
            assert!(!p.is_empty(), "{name}: key missing");
            let file = resolve_file(&rom, p).unwrap_or_else(|| panic!("{name}: file not found"));
            let pcm = load_track(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
            println!(
                "{name}: {:.1} s {} Hz {} ch {} samples",
                pcm.duration_s(),
                pcm.rate,
                pcm.channels,
                pcm.samples.len()
            );
            assert!(pcm.samples.len() <= MAX_SAMPLES && pcm.duration_s() > 1.0);
            distinct.insert(file);
        }
        // SPEC: AUDIO_2009.md §8.2 COUNT: five keys, four distinct files (waiting room = front end)
        assert_eq!(distinct.len(), 4);
        assert!(dc_bute::eq_fold(&f.waiting, &f.front_end));
    }
}
