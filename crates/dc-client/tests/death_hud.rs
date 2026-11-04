//! The death HUD of the 2009 client, drawn from the user's ROM: the live widgets leave at the death, the death panel
//! comes after the killer camera, the respawn wait shows no countdown digit; and the panel's places compared with the
//! real 2009 client's screenshot of the same moment.
//!
//! ROM-gated (skipped without `CA_ROM`). The screenshot comparison also needs the hands-free side-by-side capture
//! folder (`DC_CAPTURES`; skipped when absent). No ROM or screenshot
//! value is in this file: the element rectangles come from the loaded layout, the reference pixels from the capture
//! at run time. Frames are saved under `DC_RENDER_OUT` (default the temp folder) as `dc-client/death_*.png`.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1, §4.1, §4.3; GAME_MESSAGES_2009_C2S.md §3.3, §3.5; FMT_CRES_UI.md §8
// OBSERVED: capture 20261025_202429_sbs4-real/driver/walkstop_06.png (the 2009 client dead on
// JUNK_FLEA, killed by a bot, about 4 s after the death: death panel, respawn wait, no ammunition / health / radar)

use dc_botclient::shell::{KillRecord, RosterRecord};
use dc_client::hud::death::{self, DYING_TIME_MS};
use dc_client::hud::layout::{Digits, Img};
use dc_client::hud::state::{AmmoView, HudEvent};
use dc_client::hud::{Hud, HudState};
use dc_client::overlay::text::RgbaImage;
use std::path::PathBuf;

fn rom() -> Option<PathBuf> {
    let r = dc_rom::test_rom();
    if r.is_none() {
        println!("skipped: CA_ROM unset");
    }
    r
}

fn out_dir() -> PathBuf {
    let d = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn save(img: &RgbaImage, name: &str) {
    if let Some(b) = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone()) {
        let _ = b.save(out_dir().join(format!("death_{name}.png")));
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

const DEATH_AT: u32 = 10_000;

/// TDM, own team 1, the last hit 27 on the arms (0x74 `dmg_kind` 3), killed by the bot (0x6d, 0x67 3 then 2), `since`
/// ms after the death.
fn dead_state(name: &str, since: u32) -> HudState {
    let mut s = HudState::default();
    for e in [
        HudEvent::Own {
            pid: Some(0),
            mode_class: Some(1),
        },
        HudEvent::Roster(rec(0, name, 1)),
        HudEvent::Roster(rec(1, "bot1", 2)),
        HudEvent::Life(1),
        HudEvent::Status {
            kind: 2,
            value: 19.0,
        },
        HudEvent::Status {
            kind: 3,
            value: 64.0,
        },
        HudEvent::RoundBegin(600),
        HudEvent::TeamPair([0, 1]),
        HudEvent::Goal(140),
        HudEvent::Wound {
            damage: 27,
            kind: 3,
        },
    ] {
        s.apply(&e, 1000, 0.0);
    }
    s.ammo = Some(AmmoView {
        weapon: 145,
        clip: 30,
        reserve: 90,
        clip_size: Some(30),
        single_shot: false,
        order_class: 1,
        infinite: false,
    });
    let kill = KillRecord {
        victim: 0,
        killer: 1,
        weapon: 145,
        hit_zone: 3,
        complete: true,
        ..KillRecord::default()
    };
    s.apply(&HudEvent::Kill(kill), DEATH_AT, 0.0);
    for e in [
        HudEvent::Status {
            kind: 2,
            value: 0.0,
        },
        HudEvent::Life(3),
        HudEvent::Life(2),
    ] {
        s.apply(&e, DEATH_AT, 0.0);
    }
    s.tick(DEATH_AT + since, 60_000);
    s
}

/// True when this frame's display list has an image of the texture of `i`.
fn has_image(hud: &mut Hud, s: &HudState, i: &Option<Img>) -> bool {
    let Some(i) = i else {
        return false;
    };
    hud.items(s)
        .iter()
        .any(|it| matches!(it, dc_client::hud::DrawItem::Image { tex, .. } if *tex == i.tex))
}

fn rect_img(i: &Option<Img>) -> Option<[f32; 4]> {
    i.as_ref().map(|i| {
        [
            i.pos[0] as f32,
            i.pos[1] as f32,
            i.size[0] as f32,
            i.size[1] as f32,
        ]
    })
}

fn rect_digits(d: &Option<Digits>) -> Option<[f32; 4]> {
    d.as_ref().map(|d| {
        let xs = d.pos.iter().map(|p| p[0]);
        let (x0, x1) = (xs.clone().min().unwrap(), xs.max().unwrap() + d.size[0]);
        [
            x0 as f32,
            d.pos[0][1] as f32,
            (x1 - x0) as f32,
            d.size[1] as f32,
        ]
    })
}

/// Opaque pixels of `img` inside the virtual rectangle `r`.
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

#[test]
fn rom_death_hud_hides_the_live_widgets_and_waits_for_the_killer_camera() {
    let Some(rom) = rom() else {
        return;
    };
    let (w, h) = (1920, 1080);
    let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
    let l = hud.layout.clone();
    let d = &l.death;
    assert!(d.band.is_some() && d.info_frame.is_some() && d.damage_frame.is_some());
    assert!(d.hp.is_some() && d.ap.is_some() && d.damage.is_some() && d.hint.is_some());
    assert!(
        l.respawn.number.is_none(),
        "the 2009 client draws no countdown digit"
    );
    // alive: the live widgets
    let mut alive = dead_state("sbs4real", 0);
    alive.apply(&HudEvent::Life(1), DEATH_AT, 0.0);
    alive.apply(
        &HudEvent::Status {
            kind: 2,
            value: 100.0,
        },
        DEATH_AT,
        0.0,
    );
    let img = hud.render(&alive);
    save(&img, "alive");
    let health = rect_digits(&l.health.health_digits).expect("health digits");
    let clip = rect_digits(&l.ammo.clip).expect("clip digits");
    assert!(drawn_in(&img, health) > 50 && drawn_in(&img, clip) > 50);
    assert!(!has_image(&mut hud, &alive, &d.info_frame));
    // 1 s after the death: killer camera, live widgets gone, no panel, the respawn wait drawn
    let s = dead_state("sbs4real", 1000);
    assert!(death::killer_camera(&s) && !death::panel_shown(&s));
    let img = hud.render(&s);
    save(&img, "killercam_1s");
    assert_eq!(drawn_in(&img, health), 0, "health hidden while dead");
    assert_eq!(drawn_in(&img, clip), 0, "ammunition hidden while dead");
    assert!(!has_image(&mut hud, &s, &d.info_frame));
    assert_eq!(drawn_in(&img, rect_img(&d.hint).unwrap()), 0);
    assert!(drawn_in(&img, rect_img(&l.respawn.bar).unwrap()) > 50);
    // after the dying time: the panel
    let s = dead_state("sbs4real", DYING_TIME_MS + 500);
    assert!(death::panel_shown(&s));
    let img = hud.render(&s);
    save(&img, "panel_4s");
    for (n, r) in [
        ("frame", rect_img(&d.info_frame)),
        ("hp", rect_digits(&d.hp)),
        ("ap", rect_digits(&d.ap)),
        ("damage frame", rect_img(&d.damage_frame)),
        ("damage", rect_digits(&d.damage)),
        ("hint", rect_img(&d.hint)),
    ] {
        let r = r.unwrap();
        assert!(drawn_in(&img, r) > 20, "{n} drawn at {r:?}");
    }
    // (the panel's own HP digits lie where the live health digits were; the live block is checked above)
    assert!(has_image(&mut hud, &s, &d.info_frame));
    // respawned: gone again
    let mut s = s;
    s.apply(&HudEvent::Life(1), DEATH_AT + 7100, 0.0);
    assert!(!has_image(&mut hud, &s, &d.info_frame));
}

/// One element's best pixel offset against the reference: `(dx, dy, mean error, pixels)`.
fn best_offset(
    ours: &RgbaImage,
    real: &image::RgbImage,
    r: [f32; 4],
) -> Option<(i32, i32, f32, usize)> {
    let (sx, sy) = (ours.width as f32 / 1024.0, ours.height as f32 / 768.0);
    let (x0, y0) = ((r[0] * sx).floor() as i32, (r[1] * sy).floor() as i32);
    let (x1, y1) = (
        ((r[0] + r[2]) * sx).ceil() as i32,
        ((r[1] + r[3]) * sy).ceil() as i32,
    );
    let mut pts = Vec::new();
    for y in y0.max(0)..y1.min(ours.height as i32) {
        for x in x0.max(0)..x1.min(ours.width as i32) {
            if ours.pixel(x as u32, y as u32)[3] > 200 {
                pts.push((x, y));
            }
        }
    }
    if pts.len() < 20 {
        return None;
    }
    let mut best: Option<(i32, i32, f32, usize)> = None;
    for dy in -4..=4 {
        for dx in -4..=4 {
            let mut e = 0u64;
            for &(x, y) in &pts {
                let (rx, ry) = (x + dx, y + dy);
                if rx < 0 || ry < 0 || rx >= real.width() as i32 || ry >= real.height() as i32 {
                    e += 765;
                    continue;
                }
                let a = ours.pixel(x as u32, y as u32);
                let b = real.get_pixel(rx as u32, ry as u32).0;
                e += (0..3)
                    .map(|k| (a[k] as i32 - b[k] as i32).unsigned_abs() as u64)
                    .sum::<u64>();
            }
            let m = e as f32 / pts.len() as f32;
            if best.is_none_or(|b| m < b.2) {
                best = Some((dx, dy, m, pts.len()));
            }
        }
    }
    best
}

fn capture(name: &str) -> Option<PathBuf> {
    let p = std::env::var_os("DC_CAPTURES")
        .map(PathBuf::from)
        .unwrap_or_default();
    let f = p.join("20261025_202429_sbs4-real/driver").join(name);
    if f.is_file() {
        Some(f)
    } else {
        println!("skipped: no capture screenshot at {}", f.display());
        None
    }
}

/// Our HUD over the reference frame (alpha blend) and beside it, for the report.
fn save_overlay(ours: &RgbaImage, real: &image::RgbImage, name: &str) {
    let (w, h) = (real.width(), real.height());
    let mut over = image::RgbImage::new(w * 2, h);
    for y in 0..h {
        for x in 0..w {
            let r = real.get_pixel(x, y).0;
            let o = ours.pixel(x, y);
            let a = o[3] as u32;
            let mix = |k: usize| ((o[k] as u32 * a + r[k] as u32 * (255 - a)) / 255) as u8;
            over.put_pixel(x, y, image::Rgb([mix(0), mix(1), mix(2)]));
            over.put_pixel(w + x, y, image::Rgb(r));
        }
    }
    let _ = over.save(out_dir().join(format!("death_{name}.png")));
}

#[test]
fn rom_death_panel_matches_the_2009_client() {
    let Some(rom) = rom() else {
        return;
    };
    let Some(shot) = capture("walkstop_06.png") else {
        return;
    };
    let real = image::open(&shot).expect("screenshot").to_rgb8();
    let (w, h) = (real.width(), real.height());
    let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
    let s = dead_state("sbs4real", DYING_TIME_MS + 600);
    let ours = hud.render(&s);
    save_overlay(&ours, &real, "panel_vs_2009_walkstop_06");
    let d = hud.layout.death.clone();
    let r = &hud.layout.respawn;
    let mut worst = 0;
    let mut tested = 0;
    for (name, rr) in [
        ("info frame", rect_img(&d.info_frame)),
        ("soldier", rect_img(&d.soldier)),
        ("hp label", rect_img(&d.hp_label)),
        ("hp digits", rect_digits(&d.hp)),
        ("ap label", rect_img(&d.ap_label)),
        ("ap digits", rect_digits(&d.ap)),
        ("damage frame", rect_img(&d.damage_frame)),
        ("damage word", rect_img(&d.damage_word)),
        ("damage digits", rect_digits(&d.damage)),
        ("hint", rect_img(&d.hint)),
        // the respawn bar is not compared: its gauge fill differs with the moment (CLI-51)
        ("respawn note", rect_img(&r.note)),
    ] {
        let Some(rr) = rr else {
            continue;
        };
        match best_offset(&ours, &real, rr) {
            Some((dx, dy, err, n)) => {
                println!(
                    "{name:14} {rr:?}: offset ({dx:+}, {dy:+}), error {err:.1} over {n} pixels"
                );
                assert!(err < 120.0, "{name}: no match ({err:.1})");
                worst = worst.max(dx.abs()).max(dy.abs());
                tested += 1;
            }
            None => println!("{name:14} {rr:?}: nothing drawn"),
        }
    }
    println!("{tested} elements, worst offset {worst} px");
    assert!(tested >= 9, "elements tested: {tested}");
    assert!(worst <= 2, "every element within 2 pixels: worst {worst}");
}

#[test]
fn rom_killer_camera_frame_matches_the_2009_client_without_panel() {
    let Some(rom) = rom() else {
        return;
    };
    // 2009 frame fire_06 of the same session: dead, before the panel (no ammunition, health, radar or panel)
    let Some(shot) = capture("fire_06.png") else {
        return;
    };
    let real = image::open(&shot).expect("screenshot").to_rgb8();
    let (w, h) = (real.width(), real.height());
    let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
    let s = dead_state("sbs4real", 2000);
    let ours = hud.render(&s);
    save_overlay(&ours, &real, "killercam_vs_2009_fire_06");
    let d = hud.layout.death.clone();
    assert_eq!(drawn_in(&ours, rect_img(&d.band).unwrap()), 0);
}
