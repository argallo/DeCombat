//! The run's event stream (`--events FILE`): one JSON object per line with the time since the stream opened
//! (`t_ms`), a sequence number (`seq`) and the event name (`ev`), written as it happens (a line per event, flushed,
//! so a stalled or killed process keeps everything up to the stall).
//!
//! Events (the fields besides `t_ms` / `seq` / `ev`):
//! - `screen {name}`: the menu screen shown (`login`, `servers`, ... ) or `game` / `match_end` for the match window;
//! - `click {widget, x, y, handled, handler, effects, sound, enabled, via, screen, screen_after}`: one click of a
//!   step (or of the user in a window run); `handled` is true when the click changed anything (screen, box, a
//!   lobby request, the drawn screen); `sound` is the hash of a sound started by the click (`null`: silent);
//! - `hover {widget, x, y}`, `cursor {image, visible, grabbed}`, `dropdown {layer, items}` (a window the click
//!   opened, with its buttons and list rows);
//! - `sound {state: start|stop, voice, file_hash, trigger, loop, dur_ms, played_ms}` and `music {cue | track}`;
//! - `damage {damage, zone}`, `kill {victim, killer, weapon, zone}`, `death {killer}`, `respawn`, `life {state}`,
//!   `round {phase, ...}`;
//! - `viewmodel {row, offset, model}`: the own first-person weapon row and its eye-space offset when it changes;
//! - `corpse {obj, model, placeholder}`: a corpse seen (placeholder = drawn without its model);
//! - `step {id, verb, status}`, `step_fail {id, verb, why}`, `mark {label}`, `shot {name, path}` from `--steps`.
//!
//! Nothing here is read from the 2009 client: the stream is our own test instrument.
// DC-DEFAULT: CLI-GATE-T2-1 (staged) the scenario event stream (gate T2), see reports/gate-steps_DC_DEFAULTS.md

use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

/// Most events kept in memory for the step runner's `on_event` / `expect` (older ones drop first).
const KEEP: usize = 20_000;

/// One event as the step runner sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub seq: u64,
    pub t_ms: u64,
    pub ev: String,
    pub body: Map<String, Value>,
}

impl Event {
    /// True when every field of `want` is equal in this event (`ev` compares with the event name).
    pub fn matches(&self, ev: &str, want: &Map<String, Value>) -> bool {
        if self.ev != ev {
            return false;
        }
        want.iter().all(|(k, v)| self.body.get(k) == Some(v))
    }
}

/// A voice whose end the stream still waits for.
struct OpenVoice {
    started: Instant,
    file_hash: Option<String>,
    looped: bool,
    dur_ms: Option<u32>,
}

struct Bus {
    start: Instant,
    file: Option<std::io::LineWriter<std::fs::File>>,
    recent: VecDeque<Event>,
    seq: u64,
    screen: Option<String>,
    music: Option<Option<String>>,
    voices: BTreeMap<u32, OpenVoice>,
    sound_starts: u64,
    last_sound: Option<String>,
}

static ON: AtomicBool = AtomicBool::new(false);
static BUS: OnceLock<Mutex<Bus>> = OnceLock::new();

fn bus() -> MutexGuard<'static, Bus> {
    BUS.get_or_init(|| {
        Mutex::new(Bus {
            start: Instant::now(),
            file: None,
            recent: VecDeque::new(),
            seq: 0,
            screen: None,
            music: None,
            voices: BTreeMap::new(),
            sound_starts: 0,
            last_sound: None,
        })
    })
    .lock()
    .unwrap_or_else(|e| e.into_inner())
}

/// Turns the stream on: in memory always (the step runner reads it), into `path` when given.
pub fn open(path: Option<&Path>) -> std::io::Result<()> {
    let file = match path {
        Some(p) => {
            if let Some(d) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(d)?;
            }
            Some(std::io::LineWriter::new(std::fs::File::create(p)?))
        }
        None => None,
    };
    {
        let mut b = bus();
        b.start = Instant::now();
        b.file = file;
    }
    ON.store(true, Ordering::Release);
    Ok(())
}

/// The stream is on (`--events` or `--steps`).
pub fn on() -> bool {
    ON.load(Ordering::Acquire)
}

/// Milliseconds since the stream opened.
pub fn now_ms() -> u64 {
    bus().start.elapsed().as_millis() as u64
}

fn push(b: &mut Bus, ev: &str, body: Value) -> u64 {
    b.seq += 1;
    let seq = b.seq;
    let t_ms = b.start.elapsed().as_millis() as u64;
    let body = match body {
        Value::Object(m) => m,
        Value::Null => Map::new(),
        other => {
            let mut m = Map::new();
            m.insert("value".into(), other);
            m
        }
    };
    if let Some(f) = b.file.as_mut() {
        let mut line = Map::new();
        line.insert("t_ms".into(), json!(t_ms));
        line.insert("seq".into(), json!(seq));
        line.insert("ev".into(), json!(ev));
        for (k, v) in &body {
            line.insert(k.clone(), v.clone());
        }
        let _ = writeln!(f, "{}", Value::Object(line));
    }
    b.recent.push_back(Event {
        seq,
        t_ms,
        ev: ev.to_string(),
        body,
    });
    while b.recent.len() > KEEP {
        b.recent.pop_front();
    }
    seq
}

/// Writes one event (nothing while the stream is off); returns its sequence number (0: off).
pub fn emit(ev: &str, body: Value) -> u64 {
    if !on() {
        return 0;
    }
    push(&mut bus(), ev, body)
}

/// The sequence number of the last event.
pub fn last_seq() -> u64 {
    if !on() {
        return 0;
    }
    bus().seq
}

/// The events after `seq`, oldest first.
pub fn since(seq: u64) -> Vec<Event> {
    if !on() {
        return Vec::new();
    }
    bus()
        .recent
        .iter()
        .filter(|e| e.seq > seq)
        .cloned()
        .collect()
}

/// The newest event named `ev` after `seq`.
pub fn last_of(ev: &str, seq: u64) -> Option<Event> {
    if !on() {
        return None;
    }
    bus()
        .recent
        .iter()
        .rev()
        .take_while(|e| e.seq > seq)
        .find(|e| e.ev == ev)
        .cloned()
}

/// The screen now (menu screen name, `game`, `match_end`): an event when it changed.
pub fn screen(name: &str) {
    if !on() {
        return;
    }
    let mut b = bus();
    if b.screen.as_deref() == Some(name) {
        return;
    }
    b.screen = Some(name.to_string());
    push(&mut b, "screen", json!({ "name": name }));
}

/// The screen the stream last reported.
pub fn current_screen() -> Option<String> {
    if !on() {
        return None;
    }
    bus().screen.clone()
}

/// Sound starts so far (a click compares the count before and after it).
pub fn sound_starts() -> (u64, Option<String>) {
    if !on() {
        return (0, None);
    }
    let b = bus();
    (b.sound_starts, b.last_sound.clone())
}

/// A sound start of the sound director (`voice`: the mixer's voice, `None` when nothing plays; `dur_ms`: the
/// file's length).
pub fn sound_start(
    voice: Option<u32>,
    file_hash: Option<&str>,
    trigger: &str,
    looped: bool,
    dur_ms: Option<u32>,
) {
    if !on() {
        return;
    }
    let mut b = bus();
    b.sound_starts += 1;
    b.last_sound = file_hash.map(str::to_string);
    if let Some(v) = voice {
        b.voices.insert(
            v,
            OpenVoice {
                started: Instant::now(),
                file_hash: file_hash.map(str::to_string),
                looped,
                dur_ms,
            },
        );
    }
    push(
        &mut b,
        "sound",
        json!({ "state": "start", "voice": voice, "file_hash": file_hash, "trigger": trigger, "loop": looped,
                "dur_ms": dur_ms }),
    );
}

/// Once per frame with the director's mixer: a `sound stop` for every voice that ended, a `music` event when the
/// music track changed.
pub fn sound_poll(mixer: &dc_audio::mixer::Mixer) {
    if !on() {
        return;
    }
    let ended: Vec<u32> = {
        let b = bus();
        b.voices
            .keys()
            .copied()
            .filter(|v| mixer.is_finished(dc_audio::mixer::VoiceId(*v)))
            .collect()
    };
    let track = mixer.bgm.current_key().map(crate::sound::path_hash);
    let playing = mixer.bgm.is_playing();
    let mut b = bus();
    for v in ended {
        if let Some(o) = b.voices.remove(&v) {
            let played = o.started.elapsed().as_millis() as u64;
            push(
                &mut b,
                "sound",
                json!({ "state": "stop", "voice": v, "file_hash": o.file_hash, "loop": o.looped,
                        "dur_ms": o.dur_ms, "played_ms": played }),
            );
        }
    }
    let now = if playing { track } else { None };
    if b.music.as_ref() != Some(&now) {
        b.music = Some(now.clone());
        push(&mut b, "music", json!({ "track": now, "playing": playing }));
    }
}

/// Voices started and not ended yet that loop (the gate's `voices_looping`).
pub fn voices_looping() -> usize {
    if !on() {
        return 0;
    }
    bus().voices.values().filter(|v| v.looped).count()
}

/// A music cue the menu or the match gave the director.
pub fn music_cue(cue: &str) {
    emit("music", json!({ "cue": cue }));
}

/// What one game window or headless match saw so far, for the events it derives from the frame state.
#[derive(Debug, Default)]
pub struct GameWatch {
    dead: bool,
    view: Option<(Option<u16>, [i32; 3])>,
    corpses: BTreeMap<u16, bool>,
}

impl GameWatch {
    /// One frame: damage, kills, deaths, respawns, rounds and the own weapon row ([`GameWatch::corpses`] on the
    /// frames that draw).
    pub fn frame(&mut self, fs: &crate::world::FrameState) {
        if !on() {
            return;
        }
        use crate::hud::state::HudEvent as H;
        for (_, e) in &fs.hud_events {
            match e {
                H::Hit { damage, zone } => {
                    emit("damage", json!({ "damage": damage, "zone": zone }));
                }
                H::Kill(k) => {
                    emit(
                        "kill",
                        json!({ "victim": k.victim, "killer": k.killer, "weapon": k.weapon, "zone": k.hit_zone }),
                    );
                }
                H::Life(s) => {
                    emit("life", json!({ "state": s }));
                }
                H::RoundBegin(secs) => {
                    emit("round", json!({ "phase": "begin", "secs": secs }));
                }
                H::MatchBegin(_) => {
                    emit("round", json!({ "phase": "match_begin" }));
                }
                H::RoundResult {
                    round_secs,
                    win_team,
                    wins,
                } => {
                    emit(
                        "round",
                        json!({ "phase": "result", "secs": round_secs, "win_team": win_team, "wins": wins }),
                    );
                }
                H::ResultScreen { flag, value } => {
                    emit(
                        "round",
                        json!({ "phase": "result_screen", "flag": flag, "value": value }),
                    );
                }
                _ => {}
            }
        }
        match (&fs.death, self.dead) {
            (Some(d), false) => {
                self.dead = true;
                emit("death", json!({ "killer": d.killer }));
            }
            (None, true) if fs.life_state.is_none_or(|s| s == 1) => {
                self.dead = false;
                emit("respawn", json!({}));
            }
            _ => {}
        }
        let vm = fs.view_weapon.as_ref().map_or((None, [0; 3]), |w| {
            (Some(w.weapon), w.pos.map(|c| (c * 1000.0).round() as i32))
        });
        if self.view != Some(vm) {
            self.view = Some(vm);
            let (row, offset, model) = match &fs.view_weapon {
                Some(w) => (
                    Some(w.weapon),
                    Some(w.pos),
                    w.model.as_ref().map(|m| crate::sound::path_hash(&m.model)),
                ),
                None => (None, None, None),
            };
            emit(
                "viewmodel",
                json!({ "row": row, "offset": offset, "model": model }),
            );
        }
    }

    /// The corpses of a drawn frame (`drawn`: the objects drawn with a model this frame or loading one).
    pub fn corpses(&mut self, fs: &crate::world::FrameState, drawn: &[u16]) {
        if !on() {
            return;
        }
        for c in &fs.anims.corpses {
            let model = c
                .look
                .as_ref()
                .and_then(|l| l.models.first())
                .map(|m| crate::sound::path_hash(&m.model));
            let placeholder = model.is_none() || !drawn.contains(&c.obj);
            if self.corpses.get(&c.obj) != Some(&placeholder) {
                self.corpses.insert(c.obj, placeholder);
                emit(
                    "corpse",
                    json!({ "obj": c.obj, "model": model, "placeholder": placeholder }),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_compares_the_name_and_every_wanted_field() {
        let mut body = Map::new();
        body.insert("handled".into(), json!(false));
        body.insert("widget".into(), json!("lobby.shop"));
        let e = Event {
            seq: 1,
            t_ms: 0,
            ev: "click".into(),
            body,
        };
        let mut want = Map::new();
        assert!(e.matches("click", &want));
        want.insert("handled".into(), json!(false));
        assert!(e.matches("click", &want));
        assert!(!e.matches("hover", &want));
        want.insert("widget".into(), json!("lobby.clan"));
        assert!(!e.matches("click", &want));
    }
}
