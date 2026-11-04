//! Offline render of recorded sound starts through the mixer, before and after the client-audio3d changes: the hard
//! pan with every fire sound played to its end (before) against the CLI-200 pan law with the 2009 fire-voice cut
//! (after). The starts come from a session's `logs/dc-client.sound.jsonl` (our own client's log: file hashes,
//! positions, radii, volume, class), the listener from the session's `updates.csv` (capanalyze: the client's own 0x64
//! positions and aim), the sounds from the user's ROM (matched by the logged hash). Nothing is rendered to a device;
//! the table goes to stdout. ROM-gated (`CA_ROM`) and capture-gated (skipped without the session folder); no ROM name
//! is printed and nothing is asserted by ROM value.
// SPEC: AUDIO_2009.md §5.1 (listener from the camera, 10 Hz), §5.3 (distance law), §4.1 (stages B, C)
// OBSERVED: capture 20261025_205716_rustclient, 20261025_202211_sbs4-rust (OUR client, OUR stack)
// EVIDENCE: cshell+0x1a2b50 (fire and alternate-fire voices kept per emitter and weapon and cut by the next one)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dc_audio::decode::{decode_wav, Pcm};
use dc_audio::mixer::{Listener, Mixer, PanLaw, PlayRequest, VoiceId, VoiceKind};
use dc_client::sound::{path_hash, Emitter};
use dc_rez::RezArchive;

const SESSIONS: [&str; 2] = ["20261025_205716_rustclient", "20261025_202211_sbs4-rust"];
const RATE: u32 = 44_100;
const BLOCK: usize = 441;

fn capture_dir(folder: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DC_CAPTURES") {
        let c = PathBuf::from(p).join(folder);
        return c.is_dir().then_some(c);
    }
    None
}

/// `updates.csv` of a session: its own `analysis/` folder, or `$DC_REPLAY_ANALYSIS/<folder>/`.
fn updates_csv(cap: &Path, folder: &str) -> Option<PathBuf> {
    let own = cap.join("analysis/updates.csv");
    if own.is_file() {
        return Some(own);
    }
    let alt = PathBuf::from(std::env::var("DC_REPLAY_ANALYSIS").ok()?)
        .join(folder)
        .join("updates.csv");
    alt.is_file().then_some(alt)
}

#[derive(Debug, Clone)]
struct Start {
    t_ms: u32,
    trigger: String,
    detail: String,
    hash: Option<String>,
    pos: Option<[f32; 3]>,
    kind: VoiceKind,
    inner: f32,
    outer: f32,
    volume: u8,
    class: u8,
    looped: bool,
}

fn load_starts(path: &Path) -> Vec<Start> {
    let text = std::fs::read_to_string(path).expect("sound log");
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|v| Start {
            t_ms: v["t_ms"].as_u64().unwrap_or(0) as u32,
            trigger: v["trigger"].as_str().unwrap_or("").to_string(),
            detail: v["detail"].as_str().unwrap_or("").to_string(),
            hash: v["file_hash"].as_str().map(str::to_string),
            pos: v["pos"].as_array().map(|a| {
                let f = |i: usize| a.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                [f(0), f(1), f(2)]
            }),
            kind: match v["kind"].as_str() {
                Some("Ambient") => VoiceKind::Ambient,
                Some("Positional") => VoiceKind::Positional,
                _ => VoiceKind::Local,
            },
            inner: v["inner"].as_f64().unwrap_or(0.0) as f32,
            outer: v["outer"].as_f64().unwrap_or(0.0) as f32,
            volume: v["volume"].as_u64().unwrap_or(100).min(100) as u8,
            class: v["class"].as_u64().unwrap_or(0) as u8,
            looped: v["loop"].as_bool().unwrap_or(false),
        })
        .collect()
}

/// One listener sample: client time (s), eye, yaw and pitch (radians).
#[derive(Debug, Clone, Copy)]
struct Ear {
    t: f64,
    eye: [f32; 3],
    yaw: f32,
    pitch: f32,
}

fn load_ears(path: &Path) -> Vec<Ear> {
    let text = std::fs::read_to_string(path).expect("updates.csv");
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().unwrap_or("").split(',').collect();
    let col = |n: &str| head.iter().position(|h| *h == n).expect(n);
    let (ct, px, py, pz, ey, yw, pt) = (
        col("client_t"),
        col("pos_x"),
        col("pos_y"),
        col("pos_z"),
        col("eye_y"),
        col("aim_yaw_deg"),
        col("aim_pitch_deg"),
    );
    let mut out: Vec<Ear> = lines
        .filter_map(|l| {
            let c: Vec<&str> = l.split(',').collect();
            let f = |i: usize| c.get(i)?.parse::<f64>().ok();
            Some(Ear {
                t: f(ct)?,
                eye: [f(px)? as f32, (f(py)? + f(ey)?) as f32, f(pz)? as f32],
                yaw: (f(yw)? as f32).to_radians(),
                pitch: (f(pt)? as f32).to_radians(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    out
}

/// Every WAV entry of the ROM archives by the hash the sound log writes.
fn wav_index(rom: &Path) -> HashMap<String, (PathBuf, u64, u32)> {
    let mut out = HashMap::new();
    let Ok(dir) = std::fs::read_dir(rom.join("Game")) else {
        return out;
    };
    for e in dir.flatten() {
        let p = e.path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rez")) {
            continue;
        }
        let Ok(arch) = RezArchive::open(&p) else {
            continue;
        };
        for (name, off, size) in arch.entries() {
            if name.to_ascii_lowercase().ends_with(".wav") {
                out.entry(path_hash(name))
                    .or_insert((p.clone(), *off, *size));
            }
        }
    }
    out
}

/// The fire-voice key of a start, as the director keys it (the weapon by its fire file).
fn fire_key(s: &Start) -> Option<(Emitter, String)> {
    let num = |field: &str| -> Option<u32> {
        let i = s.detail.find(field)? + field.len();
        let digits: String = s.detail[i..]
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().ok()
    };
    let e = match s.trigger.as_str() {
        "own_fire" => Emitter::Own,
        "shot_fire" => Emitter::Object(num("shooter")? as u16),
        "relay_sound" if matches!(num("k:"), Some(6) | Some(8)) => Emitter::Player(num("player")?),
        "own_weapon" if matches!(num("k:"), Some(6) | Some(8)) => Emitter::Own,
        _ => return None,
    };
    Some((e, s.hash.clone()?))
}

fn track_of(s: &Start) -> Option<String> {
    s.detail.contains("track:").then(|| s.detail.clone())
}

#[derive(Debug, Default, Clone)]
struct Meter {
    frames: u64,
    clipped: u64,
    sum2: [f64; 2],
    peak_l: f32,
    peak_r: f32,
    loud_blocks: u64,
    one_ear_blocks: u64,
    max_fire_voices: usize,
    voices_sum: u64,
    blocks: u64,
}

impl Meter {
    fn block(&mut self, out: &[f32]) {
        let mut s2 = [0.0f64; 2];
        for f in out.as_chunks::<2>().0 {
            for (c, v) in f.iter().enumerate() {
                s2[c] += f64::from(*v) * f64::from(*v);
                if v.abs() >= 0.999 {
                    self.clipped += 1;
                }
            }
            self.peak_l = self.peak_l.max(f[0].abs());
            self.peak_r = self.peak_r.max(f[1].abs());
        }
        let n = (out.len() / 2) as f64;
        self.frames += n as u64;
        self.sum2[0] += s2[0];
        self.sum2[1] += s2[1];
        let (l, r) = ((s2[0] / n).sqrt(), (s2[1] / n).sqrt());
        let (hi, lo) = (l.max(r), l.min(r));
        // a block louder than -30 dBFS whose quiet ear is more than 26 dB down: "only in one ear"
        if hi > 0.0316 {
            self.loud_blocks += 1;
            if lo < hi * 0.05 {
                self.one_ear_blocks += 1;
            }
        }
    }
    fn rms_db(&self) -> f64 {
        let p = (self.sum2[0] + self.sum2[1]) / (2.0 * self.frames.max(1) as f64);
        10.0 * p.max(1e-12).log10()
    }
    fn clip_pct(&self) -> f64 {
        100.0 * self.clipped as f64 / (2 * self.frames.max(1)) as f64
    }
    fn one_ear_pct(&self) -> f64 {
        100.0 * self.one_ear_blocks as f64 / self.loud_blocks.max(1) as f64
    }
}

/// One way of mixing: the pan law, the level ceiling and whether fire sounds cut the previous one.
#[derive(Debug, Clone, Copy)]
struct Variant {
    name: &'static str,
    law: PanLaw,
    ceiling: Option<f32>,
    cut: bool,
}

/// Before (the mixer and director as merged), the steps of client-audio3d one by one, and the shipped defaults.
fn variants() -> [Variant; 4] {
    let wide = PanLaw {
        centre: 1.0,
        ..PanLaw::default()
    };
    [
        Variant {
            name: "before (hard pan, 1.27, no cut)",
            law: PanLaw::HARD,
            ceiling: None,
            cut: false,
        },
        Variant {
            name: "+ fire cut (CLI-201)",
            law: PanLaw::HARD,
            ceiling: None,
            cut: true,
        },
        Variant {
            name: "+ pan floor 0.3, centre 1.0",
            law: wide,
            ceiling: None,
            cut: true,
        },
        Variant {
            name: "after: + centre 0.707, ceiling 1.0 (defaults)",
            law: PanLaw::default(),
            ceiling: Some(dc_audio::mixer::LEVEL_CEILING),
            cut: true,
        },
    ]
}

/// Renders the starts of `[t0, t1)` (ms) in one variant.
fn render(
    starts: &[Start],
    ears: &[Ear],
    pcm: &HashMap<String, Arc<Pcm>>,
    t0: u32,
    t1: u32,
    v: Variant,
    keep: &dyn Fn(&Start) -> bool,
) -> Meter {
    let mut m = Mixer::new();
    // the shell's class gains (CLI-96): default 0.75, weapons 1.0, speech 1.0 (speech not master-scaled)
    m.classes.set_gain(0, 0.75);
    m.classes.set_master_scaled(2, false);
    m.set_pan_law(v.law);
    m.set_level_ceiling(v.ceiling);
    let after = v.cut;
    let mut fire: HashMap<(Emitter, String), VoiceId> = HashMap::new();
    let mut tracks: HashMap<String, VoiceId> = HashMap::new();
    let mut fire_ids: Vec<VoiceId> = Vec::new();
    let mut meter = Meter::default();
    let mut out = vec![0.0f32; 2 * BLOCK];
    let mut next = starts.partition_point(|s| s.t_ms < t0);
    let mut ear_i = 0;
    let mut t = t0 as f64 / 1000.0;
    let end = t1 as f64 / 1000.0;
    while t < end {
        while ear_i + 1 < ears.len() && ears[ear_i + 1].t <= t {
            ear_i += 1;
        }
        let ear = ears.get(ear_i).copied().unwrap_or(Ear {
            t: 0.0,
            eye: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
        });
        let fwd = [
            ear.yaw.sin() * ear.pitch.cos(),
            -ear.pitch.sin(),
            ear.yaw.cos() * ear.pitch.cos(),
        ];
        m.set_camera(Some(Listener::from_camera(ear.eye, fwd, [0.0, 1.0, 0.0])));
        while next < starts.len() && f64::from(starts[next].t_ms) / 1000.0 <= t {
            let s = &starts[next];
            next += 1;
            if !keep(s) {
                continue;
            }
            let Some(p) = s.hash.as_ref().and_then(|h| pcm.get(h)) else {
                continue;
            };
            let is_fire = fire_key(s).is_some();
            let key = fire_key(s).filter(|_| after);
            if let Some(k) = &key {
                if let Some(old) = fire.remove(k) {
                    m.release(old);
                }
            }
            if let Some(tr) = track_of(s) {
                if let Some(old) = tracks.remove(&tr) {
                    m.release(old);
                }
            }
            let req = PlayRequest {
                pcm: p.clone(),
                kind: s.kind,
                looped: s.looped,
                want_handle: false,
                volume: s.volume,
                inner_radius: s.inner,
                outer_radius: s.outer,
                position: s.pos.unwrap_or(ear.eye),
                class: s.class,
                reverb: true,
            };
            if let Ok(id) = m.play(req) {
                if is_fire {
                    fire_ids.push(id);
                }
                if let Some(k) = key {
                    fire.insert(k, id);
                }
                if let Some(tr) = track_of(s) {
                    tracks.insert(tr, id);
                }
            }
        }
        m.update(t);
        m.render(&mut out, BLOCK);
        meter.block(&out);
        fire_ids.retain(|id| !m.is_finished(*id));
        meter.max_fire_voices = meter.max_fire_voices.max(fire_ids.len());
        meter.voices_sum += m.playing_voices() as u64;
        meter.blocks += 1;
        t += BLOCK as f64 / f64::from(RATE);
    }
    meter
}

/// The `secs`-long window with the most starts of the triggers `want` names.
fn busiest(starts: &[Start], secs: u32, want: &dyn Fn(&str) -> bool) -> (u32, u32) {
    let fires: Vec<u32> = starts
        .iter()
        .filter(|s| want(&s.trigger))
        .map(|s| s.t_ms)
        .collect();
    let first = starts.first().map_or(0, |s| s.t_ms);
    let mut best = (0usize, first);
    for &t in &fires {
        let n = fires
            .iter()
            .filter(|&&x| x >= t && x < t + secs * 1000)
            .count();
        if n > best.0 {
            best = (n, t);
        }
    }
    (best.1, best.1 + secs * 1000)
}

#[test]
fn replay_of_recorded_sessions_before_and_after() {
    let rom = dc_rom::rom_or_skip!();
    let secs: u32 = std::env::var("DC_REPLAY_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let index = wav_index(&rom);
    let mut ran = 0;
    for folder in SESSIONS {
        let Some(cap) = capture_dir(folder) else {
            eprintln!("skip {folder}: no capture");
            continue;
        };
        let log = cap.join("logs/dc-client.sound.jsonl");
        let Some(upd) = updates_csv(&cap, folder) else {
            eprintln!("skip {folder}: no updates.csv (run capanalyze, set DC_REPLAY_ANALYSIS)");
            continue;
        };
        if !log.is_file() {
            continue;
        }
        let starts = load_starts(&log);
        let ears = load_ears(&upd);
        let mut pcm = HashMap::new();
        let mut missing = 0;
        for h in starts.iter().filter_map(|s| s.hash.clone()) {
            if pcm.contains_key(&h) {
                continue;
            }
            let Some((rez, off, size)) = index.get(&h) else {
                missing += 1;
                continue;
            };
            let raw = RezArchive::open(rez)
                .and_then(|a| a.read_raw(*off, *size))
                .expect("read wav");
            if let Ok(p) = decode_wav(&raw) {
                pcm.insert(h, Arc::new(p));
            }
        }
        type Want = fn(&str) -> bool;
        let windows: [(&str, Want); 2] = [
            ("most fire", |t| t == "own_fire" || t == "shot_fire"),
            ("most own fire", |t| t == "own_fire"),
        ];
        for (wname, want) in windows {
            let (t0, t1) = busiest(&starts, secs, &want);
            let n_fire = |pred: &dyn Fn(&Start) -> bool| {
                starts
                    .iter()
                    .filter(|s| s.t_ms >= t0 && s.t_ms < t1 && pred(s))
                    .count()
            };
            println!(
                "session {folder}, window with the {wname}: {} starts, {} sounds decoded, {missing} hashes not in the ROM index; {:.1}..{:.1} s: own fire {}, remote fire {}",
                starts.len(),
                pcm.len(),
                t0 as f64 / 1000.0,
                t1 as f64 / 1000.0,
                n_fire(&|s| s.trigger == "own_fire"),
                n_fire(&|s| s.trigger == "shot_fire"),
            );
            if n_fire(&|s| want(&s.trigger)) == 0 {
                continue;
            }
            println!("| scene | variant | peak L | peak R | clipped % | RMS dBFS | one-ear blocks % | max fire voices | mean voices |");
            println!("|---|---|---|---|---|---|---|---|---|");
            type Keep = fn(&Start) -> bool;
            let scenes: [(&str, Keep); 3] = [
                ("all sounds", |_| true),
                ("own fire only", |s| s.trigger == "own_fire"),
                ("remote fire only", |s| s.trigger == "shot_fire"),
            ];
            for (name, keep) in scenes {
                let mut rows = Vec::new();
                for v in variants() {
                    let mtr = render(&starts, &ears, &pcm, t0, t1, v, &keep);
                    println!(
                        "| {name} | {} | {:.3} | {:.3} | {:.3} | {:.1} | {:.1} | {} | {:.2} |",
                        v.name,
                        mtr.peak_l,
                        mtr.peak_r,
                        mtr.clip_pct(),
                        mtr.rms_db(),
                        mtr.one_ear_pct(),
                        mtr.max_fire_voices,
                        mtr.voices_sum as f64 / mtr.blocks.max(1) as f64,
                    );
                    rows.push(mtr);
                }
                // the shipped defaults never make it louder or more one-sided than before
                let (b, a) = (&rows[0], &rows[rows.len() - 1]);
                if b.frames > 0 && b.loud_blocks > 0 {
                    assert!(a.clipped <= b.clipped, "{name}: clipping grew");
                    assert!(
                        a.one_ear_blocks <= b.one_ear_blocks,
                        "{name}: one-ear blocks grew"
                    );
                    assert!(a.rms_db() <= b.rms_db() + 0.05, "{name}: louder");
                }
            }
        }
        ran += 1;
    }
    if ran == 0 {
        eprintln!("no session rendered (captures or analysis missing)");
    }
}
