//! snd-r12: the Search and Destroy top block drawn from the user's ROM at 1024 x 768 and 2560 x 1440 (playtest report:
//! "round number and number of alive enemies / players are mis-scaled").
//!
//! ROM-gated (skipped without `CA_ROM`); regions come from the loaded layout. PNGs (ROM-derived, never committed) go
//! to `reports/snd-r12/` when that folder's parent exists, else to the render out dir.
// SPEC: GAME_MODES_2009.md §5.2
// EVIDENCE: cshell+0x8ebe0 (items and places), cshell+0x92570 (classes 2 / 3)

use dc_botclient::shell::RosterRecord;
use dc_client::hud::state::HudEvent;
use dc_client::hud::{Hud, HudState};
use dc_client::overlay::text::RgbaImage;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let o = dc_render::default_out_dir().join("snd-r12");
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

/// A Search and Destroy round of 5: the own player 0 and a mate on team 1 (the mate dead), two enemies alive.
fn snd_state() -> HudState {
    let mut s = HudState::default();
    for e in [
        HudEvent::Own {
            pid: Some(0),
            mode_class: Some(2),
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
        HudEvent::RoundBegin(75),
    ] {
        s.apply(&e, 1000, 0.0);
    }
    s.goal = Some(5);
    if let Some(r) = s.roster.get_mut(&1) {
        r.dead = true;
    }
    s.tick(4000, 60_000);
    s.now_ms = 4000;
    s
}

/// CLI-1102: the round-limit and live digits are whole score-digit cells shrunk into their place sizes.
#[test]
fn rom_search_and_destroy_top_block_frames() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
        let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
        let r = hud.layout.top.round.clone();
        let s = snd_state();
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
        let places: Vec<_> = r
            .limit
            .iter()
            .flatten()
            .chain(r.live.iter().flatten())
            .cloned()
            .collect();
        assert_eq!(places.len(), 4, "limit and live places in the layout");
        for p in places {
            let d = texs
                .iter()
                .find(|t| {
                    t.2[0] == p.pos[0] as f32
                        && t.2[1] == p.pos[1] as f32
                        && t.0.starts_with(&r.digit_prefix)
                })
                .expect("digit at its place");
            assert_eq!(d.1, [r.digit_cell[0] as f32, r.digit_cell[1] as f32]);
            assert_eq!(d.2[2..], [p.size[0] as f32, p.size[1] as f32]);
        }
        let img = hud.render(&s);
        save(&img, &format!("snd_top_{w}x{h}"));
    }
}
