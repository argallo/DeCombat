//! Background music wiring: the menu's screen changes give the music cues of AUDIO_2009 §8.2, and a sound director
//! turns a cue into audible output on the null backend through the decode thread (synthetic track, no ROM).

use std::time::{Duration, Instant};

use dc_audio::decode::Pcm;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::{Go, Menu, MenuOptions};
use dc_client::music::{MusicCue, MusicDirector, MusicFiles};
use dc_client::sound::SoundDirector;
use dc_client::testkit::synth_font;

fn menu() -> Menu {
    Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("music-cues"),
        rom: None,
        font: Some((synth_font(), 0)),
        width: 800,
        height: 600,
    })
}

#[test]
fn screen_changes_give_the_state_cues() {
    let mut m = menu();
    // the first screen (login, state 0x11) is a front-end state
    assert_eq!(m.take_music(), Some(MusicCue::FrontEnd));
    assert_eq!(m.take_music(), None, "a cue is taken once");
    m.open(Go::Lobby);
    assert_eq!(m.take_music(), Some(MusicCue::FrontEnd));
    m.open(Go::Room);
    assert_eq!(m.take_music(), Some(MusicCue::WaitingRoom));
    // state 7 (the result screen) keeps whatever plays: no cue
    m.open(Go::Result);
    assert_eq!(m.take_music(), None);
    m.open(Go::Room);
    assert_eq!(m.take_music(), Some(MusicCue::WaitingRoom));
}

#[test]
fn cue_through_the_director_is_audible_and_bounded() {
    let root = std::env::temp_dir().join(format!("dc_music_cues_{}", std::process::id()));
    let bgm = root.join("Game").join("BGM");
    std::fs::create_dir_all(&bgm).unwrap();
    std::fs::write(bgm.join("ui.mp3"), b"x").unwrap();
    let files = MusicFiles {
        front_end: "BGM\\ui.mp3".into(),
        waiting: "bgm\\UI.mp3".into(),
        ..MusicFiles::default()
    };
    let mut d = SoundDirector::open(None);
    d.music = MusicDirector::none()
        .with_loader(|_| {
            Ok(Pcm {
                rate: 44_100,
                channels: 2,
                samples: vec![0.5; 2 * 44_100 * 4],
            })
        })
        .with_files(root.clone(), files);
    d.music_cue(MusicCue::FrontEnd);
    let end = Instant::now() + Duration::from_secs(30);
    while !d.mixer().bgm.is_playing() && Instant::now() < end {
        d.music_tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(d.mixer().bgm.is_playing(), "{:?}", d.music.log);
    // the waiting room names the same file: nothing restarts
    d.music_cue(MusicCue::WaitingRoom);
    assert!(!d.music.loading());
    // render after the fade: the music reaches the output
    std::thread::sleep(Duration::from_millis(1200));
    d.music_tick();
    let mut out = vec![0.0f32; 2 * 4410];
    d.mixer().render(&mut out, 4410);
    let peak = out.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    assert!(peak > 0.05, "music is silent: peak {peak}");
    d.music_cue(MusicCue::Stop);
    assert!(!d.mixer().bgm.is_playing());
    std::fs::remove_dir_all(root).ok();
}
