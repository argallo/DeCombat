//! modes-r13: the Capture the Flag top block drawn from the user's ROM at 1024 x 768 and 2560 x 1440 (playtest report:
//! "capture the flag the top text shows max points is too squished").
//!
//! ROM-gated (skipped without `CA_ROM`); regions come from the loaded layout. PNGs (ROM-derived, never committed) go
//! to `modes-r13/ctf_top/` when that folder's parent exists, else to the render out dir.
// SPEC: GAME_MODES_2009.md §5.4
// EVIDENCE: cshell+0x8ebe0 (the goal digit items `MaxScoresHunPos` / `MaxScoresTenPos` / `MaxScoresOnePos` built like
// the round-limit digits)

use dc_botclient::shell::RosterRecord;
use dc_client::hud::state::HudEvent;
use dc_client::hud::{Hud, HudState};
use dc_client::overlay::text::RgbaImage;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let o = dc_render::default_out_dir().join("modes-r13-ctf");
    let _ = std::fs::create_dir_all(&o);
    o
}

fn save(img: &RgbaImage, name: &str) {
    let mut rgb = Vec::with_capacity((img.width * img.height * 3) as usize);
    for px in img.rgba.as_chunks::<4>().0 {
        let a = px[3] as u32;
        for c in &px[..3] {
            rgb.push(((*c as u32 * a + 48 * (255 - a)) / 255) as u8);
        }
    }
    if let Some(b) = image::RgbImage::from_raw(img.width, img.height, rgb) {
        let _ = b.save(out_dir().join(format!("{name}.png")));
    }
}

fn rec(pid: u8, name: &str, team: u8) -> RosterRecord {
    RosterRecord {
        name: name.into(),
        pid,
        team,
        complete: true,
        ..RosterRecord::default()
    }
}

/// A Capture the Flag match to 5: the own player 0 and a mate on team 1, two enemies.
fn ctf_state() -> HudState {
    let mut s = HudState::default();
    for e in [
        HudEvent::Own {
            pid: Some(0),
            mode_class: Some(7),
        },
        HudEvent::Roster(rec(0, "user", 1)),
        HudEvent::Roster(rec(1, "bot1", 1)),
        HudEvent::Roster(rec(2, "bot2", 2)),
        HudEvent::Roster(rec(3, "bot3", 2)),
        HudEvent::Life(1),
        HudEvent::Status {
            kind: 2,
            value: 100.0,
        },
        HudEvent::RoundBegin(540),
    ] {
        s.apply(&e, 1000, 0.0);
    }
    s.goal = Some(5);
    s.tick(4000, 60_000);
    s.now_ms = 4000;
    s
}

/// CLI-1212: the goal digits are whole score-digit cells shrunk into the goal digit size, as the round-limit digits
/// of classes 2 / 12 (CLI-1102). Before, each drew only the top-left goal-size texels of the larger digit image.
#[test]
fn rom_capture_the_flag_top_block_frames() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
        let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
        let f = hud.layout.top.flag.clone();
        let cell = hud.layout.top.round.digit_cell;
        let s = ctf_state();
        let items = hud.items(&s);
        let texs: Vec<(String, [f32; 2], [f32; 4])> = items
            .iter()
            .filter_map(|it| match it {
                dc_client::hud::DrawItem::Image { tex, src, dst, .. } => {
                    Some((tex.clone(), *src, *dst))
                }
                _ => None,
            })
            .collect();
        let places = f.goal.clone().expect("goal places in the layout");
        for p in places.iter() {
            let d = texs
                .iter()
                .find(|t| {
                    t.2[0] == p.pos[0] as f32
                        && t.2[1] == p.pos[1] as f32
                        && t.0.starts_with(&f.digit_prefix)
                })
                .expect("digit at its place");
            println!("{w}: {} src {:?} dst {:?}", d.0, d.1, d.2);
            assert_eq!(d.1, [cell[0] as f32, cell[1] as f32]);
            assert_eq!(d.2[2..], [p.size[0] as f32, p.size[1] as f32]);
        }
        let img = hud.render(&s);
        save(&img, &format!("ctf_top_{w}x{h}"));
    }
}
