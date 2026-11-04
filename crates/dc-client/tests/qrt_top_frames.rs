//! qrt-ctf-r12: the Quarantine top block drawn from the user's ROM at 1024 x 768 and 2560 x 1440 (playtest report:
//! "the human and infected logos also seem messed up").
//!
//! ROM-gated (skipped without `CA_ROM`); regions come from the loaded layout. PNGs (ROM-derived, never committed) go
//! to `reports/qrt-ctf-r12/` when that folder's parent exists, else to the render out dir.
// SPEC: GAME_MODES_2009.md §5.7 (0xd3)
// EVIDENCE: cshell+0xb29c0 (the infection element's items and places)

use dc_botclient::shell::RosterRecord;
use dc_client::hud::qrt::QrtEvent;
use dc_client::hud::state::HudEvent;
use dc_client::hud::{Hud, HudState};
use dc_client::overlay::text::RgbaImage;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let o = dc_render::default_out_dir().join("qrt-ctf-r12");
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

/// A Quarantine round: the own player 0 and three others human, round 1 of 7, the countdown running.
fn qrt_state() -> HudState {
    let mut s = HudState::default();
    for e in [
        HudEvent::Own {
            pid: Some(0),
            mode_class: Some(11),
        },
        HudEvent::Roster(rec(0, "user", 1)),
        HudEvent::Roster(rec(1, "bot1", 1)),
        HudEvent::Roster(rec(2, "bot2", 1)),
        HudEvent::Roster(rec(3, "bot3", 1)),
        HudEvent::Life(1),
        HudEvent::Status {
            kind: 2,
            value: 100.0,
        },
        HudEvent::RoundBegin(180),
        HudEvent::TeamPair([4, 0]),
        HudEvent::Qrt(QrtEvent::Countdown {
            a: 1,
            b: 7,
            secs: 20.0,
        }),
    ] {
        s.apply(&e, 1000, 0.0);
    }
    s.tick(4000, 60_000);
    s.now_ms = 4000;
    s
}

#[test]
fn rom_quarantine_top_block_frames() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
        let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
        let q = hud.layout.top.qrt.clone();
        let marks = [
            hud.layout.score.team1_mark.clone(),
            hud.layout.score.team2_mark.clone(),
            hud.layout.top.team1_base.clone(),
            hud.layout.top.team2_base.clone(),
        ];
        let s = qrt_state();
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
        // CLI-1110: the Alpha / Bravo marks are not drawn under the Quarantine team marks
        for m in marks.into_iter().flatten() {
            assert!(
                !texs.iter().any(|t| t.0 == m.tex),
                "{} drawn in class 11",
                m.tex
            );
        }
        for m in [&q.humans, &q.infected].into_iter().flatten() {
            assert!(texs.iter().any(|t| t.0 == m.tex), "{} missing", m.tex);
        }
        // CLI-1111: the round digits are whole score-digit cells shrunk into the goal digit size
        for p in q.digits.iter().flatten() {
            let d = texs
                .iter()
                .find(|t| t.2[0] == p[0] as f32 && t.2[1] == p[1] as f32)
                .expect("round digit");
            assert_eq!(d.1, [q.digit_cell[0] as f32, q.digit_cell[1] as f32]);
            assert_eq!(d.2[2..], [q.digit_size[0] as f32, q.digit_size[1] as f32]);
            assert!(q.digit_cell[0] > q.digit_size[0], "the cell is shrunk");
        }
        // CLI-1210: the countdown cells take the whole 16 x 28 texel digit, scaled by the layout's HostTimerScale
        println!("HostTimerScale {}", q.timer_scale);
        let cells: Vec<_> = texs
            .iter()
            .filter(|t| t.2[1] == dc_client::hud::qrt::TIMER_Y as f32)
            .collect();
        assert_eq!(cells.len(), 5, "{texs:?}");
        for c in cells {
            assert_eq!(c.1, [16.0, 28.0]);
            assert_eq!(c.2[2..], [16.0 * q.timer_scale, 28.0 * q.timer_scale]);
        }
        let img = hud.render(&s);
        save(&img, &format!("qrt_top_{w}x{h}"));
    }
}
