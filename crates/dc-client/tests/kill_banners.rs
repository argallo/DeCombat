//! The own kill banners (`hud::banners`): the kill-count, head-shot and multi-kill banners from the 0x6d level and
//! the 0xc8 combo form, drawn from the user's ROM layout with their voice lines from the interface sound table; and
//! the headless chain without a ROM (HUD event -> banner -> sound cue -> sound log).
//!
//! The ROM tests are skipped without `CA_ROM`. No ROM value is in this file: places, sizes and files come from the
//! loaded layout and tables at run time.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3, §3.3.1
// EVIDENCE: cshell+0xa5fe0 (the announcement), cshell+0xa52a0 (combo form), cshell+0xa1320 (layout)

use dc_botclient::shell::{KillRecord, RosterRecord};
use dc_client::hud::banners::{Slot, Voice};
use dc_client::hud::state::HudEvent;
use dc_client::hud::{Hud, HudDriver, HudState};
use dc_client::overlay::text::RgbaImage;
use dc_client::sound::{AudioFrame, SoundCue, SoundDirector};
use std::path::PathBuf;

const W: u32 = 1024;
const H: u32 = 768;

fn rom() -> Option<PathBuf> {
    let r = dc_rom::test_rom();
    if r.is_none() {
        println!("skipped: CA_ROM unset");
    }
    r
}

fn save(img: &RgbaImage, name: &str) {
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    if let Some(b) = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone()) {
        let _ = b.save(out.join(format!("kill_banner_{name}.png")));
    }
}

fn rec(pid: u8, team: u8) -> RosterRecord {
    RosterRecord {
        name: format!("player{pid}"),
        pid,
        team,
        complete: true,
        ..RosterRecord::default()
    }
}

fn kill(victim: u8, killer: u8, level: u8, zone: u8) -> KillRecord {
    KillRecord {
        victim,
        killer,
        kill_kind: level,
        weapon: 145,
        hit_zone: zone,
        feed_value: 2,
        complete: true,
        ..KillRecord::default()
    }
}

fn base_events() -> Vec<HudEvent> {
    vec![
        HudEvent::Own {
            pid: Some(0),
            mode_class: Some(1),
        },
        HudEvent::Roster(rec(0, 1)),
        HudEvent::Roster(rec(1, 2)),
        HudEvent::Roster(rec(2, 1)),
        HudEvent::Life(1),
    ]
}

fn state() -> HudState {
    let mut s = HudState::default();
    for e in base_events() {
        s.apply(&e, 1000, 0.0);
    }
    s
}

/// Opaque pixels of `img` inside the virtual rectangle `r` (x, y, w, h).
fn drawn_in(img: &RgbaImage, r: [f32; 4]) -> usize {
    let (sx, sy) = (img.width as f32 / 1024.0, img.height as f32 / 768.0);
    let (x0, y0) = ((r[0] * sx).floor() as u32, (r[1] * sy).floor() as u32);
    let (x1, y1) = (
        ((r[0] + r[2]) * sx).ceil().min(img.width as f32) as u32,
        ((r[1] + r[3]) * sy).ceil().min(img.height as f32) as u32,
    );
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|&(x, y)| img.pixel(x, y)[3] > 100)
        .count()
}

/// The ROM layout has every banner slot and every voice line; a double kill, a head shot, a count banner and the
/// combo form draw in the banner band and start their voice lines; a team kill (level 0) draws nothing.
#[test]
fn rom_kill_banners_draw_and_voice() {
    let Some(rom) = rom() else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    println!("{}", hud.source);
    let b = hud.banners.clone();
    assert_eq!(b.slots.len(), 12, "every banner slot has an image");
    // the 2009 table has no file for the one-kill line: that banner is silent on the 2009 client too
    // the first twelve lines of `Voice::ALL` are the kill banner lines; the round and case lines follow them
    let kill_lines = b
        .voices
        .keys()
        .filter(|v| Voice::ALL[..12].contains(v))
        .count();
    assert_eq!(
        kill_lines, 11,
        "every kill voice line but the one-kill line resolves to a file"
    );
    let round_lines: Vec<_> = Voice::ALL[12..]
        .iter()
        .map(|&v| (v, b.voice_file(v).is_some()))
        .collect();
    println!("round / case voice lines resolved: {round_lines:?}");
    // every 2009 round / case line resolves (the draw line is our own option and is not required)
    assert!(
        round_lines
            .iter()
            .all(|&(v, ok)| ok || v == Voice::RoundDraw),
        "every round and case voice line resolves to a file"
    );
    assert!(b.voice_file(Voice::OneKill).is_none());
    assert!(b.timing.is_some() && b.digits.is_some());
    let half = b.timing.as_ref().map_or(0, |t| (t.end * 500.0) as u32);
    // the screen's middle half around the banner place (the kill feed lines are at the left edge)
    let band = [256.0, b.text_pos[1] as f32 - 40.0, 512.0, 200.0];
    let mut s = state();
    s.tick(1000, 20_000);
    let empty = drawn_in(&hud.render(&s), band);
    let mut t = 5000;
    let cases: [(&str, HudEvent, Option<Slot>, Option<Voice>); 5] = [
        (
            "friendly_kill",
            HudEvent::Kill(kill(2, 0, 0, 1)),
            None,
            None,
        ),
        (
            "single_kill",
            HudEvent::Kill(kill(1, 0, 1, 2)),
            Some(Slot::OneKill),
            Some(Voice::OneKill),
        ),
        (
            "pair_kill",
            HudEvent::Kill(kill(1, 0, 2, 2)),
            Some(Slot::Double),
            Some(Voice::Double),
        ),
        (
            "head_shot_triple",
            HudEvent::Kill(kill(1, 0, 3, 1)),
            Some(Slot::Multi),
            Some(Voice::Multi),
        ),
        (
            "combo_2",
            HudEvent::KillRun {
                kills: 0,
                head: false,
                combo: 2,
            },
            Some(Slot::Combo),
            None,
        ),
    ];
    for (name, e, slot, voice) in cases {
        s.banner.voices.clear();
        s.apply(&e, t, 0.0);
        assert_eq!(s.banner.show.map(|x| x.slot), slot, "{name}");
        assert_eq!(s.banner.voices.first().copied(), voice, "{name}");
        s.tick(t + half, 20_000);
        let img = hud.render(&s);
        save(&img, name);
        let n = drawn_in(&img, band);
        println!("{name}: {n} pixels in the band (empty {empty})");
        if slot.is_some() {
            assert!(n > empty + 500, "{name}: drawn");
        } else {
            assert!(n <= empty + 20, "{name}: nothing drawn");
        }
        // gone after the total time
        let total = b.timing.as_ref().map_or(0, |x| (x.total * 1000.0) as u32);
        s.tick(t + total + 10, 20_000);
        let late = hud.render(&s);
        save(&late, &format!("{name}_late"));
        let left = drawn_in(&late, band);
        assert!(
            left <= empty + 20,
            "{name}: gone ({left}, {:?})",
            s.banner.show
        );
        t += total + 100;
    }
}

/// The voice file of the double kill plays as a local voice from the ROM (decoded, no missing file).
#[test]
fn rom_pair_kill_voice_plays() {
    let Some(rom) = rom() else {
        return;
    };
    let hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    let file = hud.banners.voice_file(Voice::Double).map(str::to_string);
    assert!(file.is_some());
    let mut d = SoundDirector::open(Some(&rom));
    let cues = [SoundCue::Interface {
        file,
        what: Voice::Double.label(),
    }];
    d.frame(&AudioFrame {
        now_ms: 100,
        eye: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        cues: &cues,
        remotes: &[],
        own: None,
        world: None,
    });
    let s = d.starts_of("interface");
    assert_eq!(s.len(), 1);
    assert_eq!(d.stats.missing_files, 0, "{}", d.summary());
    assert_eq!(d.stats.voices, 1);
}

/// Headless, no ROM: a kill event through the HUD driver starts the banner state and hands its voice line to the
/// sound director as an interface cue (logged without a file); another killer's kill and a team kill hand nothing.
#[test]
fn headless_kill_banner_voice_reaches_the_sound_log() {
    let mut drv = HudDriver::new(Hud::fallback(W, H, None));
    let mut d = SoundDirector::open(None);
    let mut fs = dc_client::world::FrameState::default();
    let mut evs: Vec<(u32, HudEvent)> = base_events().into_iter().map(|e| (100, e)).collect();
    evs.push((200, HudEvent::Kill(kill(2, 0, 0, 1)))); // team kill
    evs.push((300, HudEvent::Kill(kill(0, 1, 2, 2)))); // another killer's double kill
    evs.push((400, HudEvent::Kill(kill(1, 0, 1, 2))));
    evs.push((500, HudEvent::Kill(kill(1, 0, 2, 2))));
    fs.hud_events = evs;
    drv.update(&fs, 600, 0.0, false, false);
    assert_eq!(drv.state.banner.show.map(|b| b.slot), Some(Slot::Double));
    let cues = drv.take_sound_cues();
    assert!(drv.take_sound_cues().is_empty(), "drained");
    d.frame(&AudioFrame {
        now_ms: 600,
        eye: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        cues: &cues,
        remotes: &[],
        own: None,
        world: None,
    });
    let lines: Vec<String> = d
        .starts_of("interface")
        .iter()
        .map(|e| format!("{:?}", e.trigger))
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("single_kill") && lines[1].contains("pair_kill"));
}
