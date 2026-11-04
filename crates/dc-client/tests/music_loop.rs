//! The user's round on a579a3f: "the end of the round-start audio clip keeps looping the whole game". The looping
//! sound was the loading track (a 2 s sting in the ROM) cued as a loop at the lobby launch. This test runs the menu
//! -> loading -> match cue sequence through one sound director on the null backend, with the sound log on, and
//! checks the log the way `gate/audio_check.py` does: every cue has a `music` line, the music is not a
//! loop in the match, and the round-start voice plays once and is gone after its length.
// SPEC: AUDIO_2009.md §8.2; EVIDENCE: cshell+0x165650 -> cshell+0x1630a0 (stop, then the loading key once, no fade)

use std::time::{Duration, Instant};

use dc_audio::decode::Pcm;
use dc_client::music::{MusicCue, MusicDirector, MusicFiles};
use dc_client::sound::{AudioFrame, SoundCue, SoundDirector};

const RATE: u32 = 44_100;

fn pcm(seconds: f32) -> Pcm {
    let frames = (RATE as f32 * seconds) as usize;
    Pcm {
        rate: RATE,
        channels: 2,
        samples: vec![0.25; frames * 2],
    }
}

fn frame(d: &mut SoundDirector, now_ms: u32, cues: &[SoundCue]) {
    d.frame(&AudioFrame {
        now_ms,
        eye: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        cues,
        remotes: &[],
        own: None,
        world: None,
    });
}

/// Runs menu -> `launch` -> match and returns the sound log's lines (kept under `DC_MUSIC_LOOP_LOG_DIR` as
/// `<tag>.sound.jsonl` when that variable is set, for the gate tool).
fn run(tag: &str, launch: MusicCue) -> (String, Vec<serde_json::Value>, usize) {
    let root = std::env::temp_dir().join(format!("dc_music_loop_{tag}_{}", std::process::id()));
    let bgm = root.join("Game").join("BGM");
    std::fs::create_dir_all(&bgm).unwrap();
    for f in ["ui.mp3", "load.mp3"] {
        std::fs::write(bgm.join(f), b"x").unwrap();
    }
    let files = MusicFiles {
        front_end: "BGM\\ui.mp3".into(),
        waiting: "BGM\\ui.mp3".into(),
        loading: "BGM\\load.mp3".into(),
        ..MusicFiles::default()
    };
    let log = root.join("sound.jsonl");
    let mut d = SoundDirector::open(None);
    d.log_to(&log).unwrap();
    // the loading track is a 2 s sting like the ROM's; the menu track is long
    d.music = MusicDirector::none()
        .with_loader(|p| {
            let long = p
                .file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("ui.mp3"));
            Ok(pcm(if long { 30.0 } else { 2.0 }))
        })
        .with_files(root.clone(), files);
    d.set_sound("SND\\RS.WAV", pcm(1.5));

    // the menu: front end, then the waiting room (same file)
    let wait = |d: &mut SoundDirector| {
        let end = Instant::now() + Duration::from_secs(30);
        while d.music.loading() && Instant::now() < end {
            d.music_tick();
            std::thread::sleep(Duration::from_millis(2));
        }
        d.music_tick();
        assert!(!d.music.loading());
    };
    d.music_cue(MusicCue::FrontEnd);
    wait(&mut d);
    d.music_cue(MusicCue::WaitingRoom);
    // the lobby launch
    d.music_cue(launch);
    wait(&mut d);
    let st = d.mixer().bgm.status().expect("the loading track plays");
    assert_eq!(st.looped, launch == MusicCue::LoadingLocal);

    // the match: the first in-world update raises the match-start cue, the HUD the round-start voice
    let first = [
        SoundCue::MusicCue(MusicCue::MatchStart),
        SoundCue::Interface {
            file: Some("SND\\RS.WAV".into()),
            what: "round_start",
        },
    ];
    frame(&mut d, 1000, &first);
    for t in (1100..=8000).step_by(100) {
        frame(&mut d, t, &[]);
    }
    let interface = d.starts_of("interface").len();
    drop(d);
    let text = std::fs::read_to_string(&log).unwrap();
    if let Ok(dir) = std::env::var("DC_MUSIC_LOOP_LOG_DIR") {
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(
            std::path::Path::new(&dir).join(format!("{tag}.sound.jsonl")),
            &text,
        )
        .unwrap();
    }
    std::fs::remove_dir_all(root).ok();
    let lines = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (text, lines, interface)
}

#[test]
fn menu_loading_match_sequence_ends_with_nothing_looping() {
    let (text, lines, interface) = run("fixed", MusicCue::Loading);
    assert_eq!(interface, 1, "one round-start voice");
    // the log as the gate reads it
    let music: Vec<(&str, &str, bool)> = lines
        .iter()
        .filter(|v| v["trigger"] == "music")
        .map(|v| {
            (
                v["op"].as_str().unwrap(),
                v["cue"].as_str().unwrap(),
                v["loop"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        music,
        [
            ("cue", "FrontEnd", true),
            ("play", "FrontEnd", true),
            ("cue", "WaitingRoom", true),
            ("cue", "Loading", false),
            ("stop", "Loading", false),
            ("play", "Loading", false),
            ("cue", "MatchStart", false),
        ],
        "{text}"
    );
    let live: Vec<&serde_json::Value> = lines.iter().filter(|v| v["trigger"] == "live").collect();
    assert!(live.len() >= 7, "one live line per second: {}", live.len());
    for v in &live {
        for voice in v["voices"].as_array().unwrap() {
            assert!(!voice["loop"].as_bool().unwrap(), "{v}");
            let (age, dur) = (
                voice["age_ms"].as_u64().unwrap(),
                voice["dur_ms"].as_u64().unwrap(),
            );
            assert!(age <= dur + 500, "a voice outlived its length: {v}");
        }
        if let Some(m) = v["music"].as_object() {
            assert!(
                !m["loop"].as_bool().unwrap(),
                "looping music in the match: {v}"
            );
        }
    }
    // the last live line: nothing plays any more
    let last = live.last().unwrap();
    assert!(last["voices"].as_array().unwrap().is_empty(), "{last}");
    assert!(last["music"].is_null(), "{last}");
}

/// The negative control: the a579a3f rule (the lobby launch cued the loading track as a loop with a fade) keeps the
/// 2 s sting looping through the whole match, and the `live` lines show it.
#[test]
fn the_old_looping_launch_shows_in_the_live_lines() {
    let (_, lines, _) = run("prefix", MusicCue::LoadingLocal);
    let looping = lines
        .iter()
        .filter(|v| v["trigger"] == "live")
        .filter(|v| v["music"]["loop"] == true)
        .count();
    assert!(
        looping >= 7,
        "every live line of the match shows the looping track: {looping}"
    );
}
