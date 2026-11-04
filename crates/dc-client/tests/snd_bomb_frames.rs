//! snd-r13 / snd-hud-r14: the Search and Destroy bomb images drawn from the user's ROM at 1024 x 768 and 2560 x 1440
//! (playtest report: "there's a cutoff bomb icon in top left of screen"; round 14: "the bomb icon in the top left
//! shouldn't be there", "when planting the bomb the progress seems to be mis-scaled").
//!
//! ROM-gated (skipped without `CA_ROM`). PNGs (ROM-derived, never committed) go to
//! `reports/snd-r13/` when that folder's parent exists, else to the render out dir.
// EVIDENCE: cshell+0x343860 / cshell+0x342d10 (an image control samples its own size in texels from the texture's
// top-left corner), cshell+0x6ffd0 (the bomb timer icon and digits at their control sizes), cshell+0xafe30 (the
// gauge frames and labels at the layout sizes)

use dc_client::hud::bomb::{self, BombEvent};
use dc_client::hud::state::HudEvent;
use dc_client::hud::{Hud, HudState};
use dc_client::overlay::text::RgbaImage;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let o = dc_render::default_out_dir().join("snd-r13");
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

/// The own player carries the bomb (0x56 on the own character) and a bomb timer runs.
fn snd_state() -> HudState {
    let mut s = HudState::default();
    for e in [
        HudEvent::Own {
            pid: Some(0),
            mode_class: Some(2),
        },
        HudEvent::Life(1),
        HudEvent::RoundBegin(75),
    ] {
        s.apply(&e, 1000, 0.0);
    }
    s.view_object = Some(7);
    s.apply(&HudEvent::Bomb(BombEvent::Carrier(Some(7))), 1000, 0.0);
    s.apply(&HudEvent::Bomb(BombEvent::Planted(40)), 1000, 0.0);
    s.tick(4000, 60_000);
    s.now_ms = 4000;
    s
}

/// CLI-1241: every bomb image samples its control's size in texels 1:1 (the planting gauge 145 x 137 of its 256 x 256
/// frame, not the whole frame squeezed in), and the picture of each sits inside that rect. CLI-1240: no held bomb.
#[test]
fn rom_search_and_destroy_bomb_images_sample_their_control_size() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
        let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
        let b = hud.layout.top.bomb.clone();
        let gauge = b.gauge.clone().expect("gauge keys in the ROM");
        let label = b.labels[0].clone().expect("plant label keys in the ROM");
        let mut s = snd_state();
        s.on_bomb(
            &BombEvent::Track {
                obj: 5,
                kind: 1,
                secs: 4.0,
            },
            2000,
        );
        let items = hud.items(&s);
        let mut seen = 0;
        let mut gauge_seen = false;
        for it in &items {
            let dc_client::hud::DrawItem::Image { tex, src, dst, .. } = it else {
                continue;
            };
            assert!(
                Some(tex) != b.held.as_ref().map(|h| &h.tex),
                "no held-bomb image"
            );
            let want = if tex.contains("Gage_0") {
                gauge_seen = true;
                gauge.size
            } else if *tex == label.tex {
                label.size
            } else if *tex == b.icon {
                bomb::ICON_SIZE
            } else if tex.contains("BOMBTIMER") {
                bomb::DIGIT_SIZE
            } else {
                continue;
            };
            let t = hud.assets.texture(tex).expect("bomb texture in the ROM");
            println!(
                "{w}x{h} {tex} src {src:?} dst {dst:?} tex {}x{}",
                t.width, t.height
            );
            assert_eq!(*src, [want[0] as f32, want[1] as f32], "{tex}: control size 1:1");
            assert!(
                src[0] <= t.width as f32 && src[1] <= t.height as f32,
                "{tex}: inside the texture"
            );
            // the picture (alpha > 8) lies inside the sampled rect
            for y in 0..t.height {
                for x in 0..t.width {
                    if t.rgba[((y * t.width + x) * 4 + 3) as usize] > 8 {
                        assert!(
                            (x as f32) < src[0] && (y as f32) < src[1],
                            "{tex}: texel {x},{y} outside the {src:?} rect"
                        );
                    }
                }
            }
            seen += 1;
        }
        assert!(gauge_seen, "the planting gauge is drawn");
        // CLI-1242: the round clock is hidden while the bomb timer runs (both sit at the top centre)
        let clock = &hud.layout.clock;
        let clock_tex: Vec<String> = (0..10)
            .map(|d| clock.digit_form.replacen("%d", &d.to_string(), 1))
            .chain([clock.separator.clone()])
            .collect();
        assert!(
            !items.iter().any(|it| matches!(it, dc_client::hud::DrawItem::Image { tex, .. }
                if clock_tex.contains(tex))),
            "no round clock digits beside the bomb timer"
        );
        assert!(seen >= 11, "gauge, label, timer icon, digits and separators drawn ({seen})");
        let img = hud.render(&s);
        save(&img, &format!("snd_bomb_{w}x{h}"));
    }
}
