//! CLI-1915: on a surface wider than 16:9 the 2D layer keeps the 16:9 stretch in a centred band. With `CA_ROM` set:
//! the front-end screens at 3440 x 1440 and 5120 x 1440 draw the same display list as at 2560 x 1440, their picture
//! is the 2560 x 1440 picture in the centred band with black side bands, their records still follow the layout rules,
//! and a click at a band-mapped position hits the control it hits at 16:9; the match HUD at 3440 x 1440 is the
//! 2560 x 1440 HUD moved into the band, with the radar, the ammunition block and the crosshair inside it and the
//! crosshair on the screen centre. `DC_ULTRAWIDE_PNG=<dir>` writes the 3440 x 1440 lobby and HUD pictures there.
// DC-DEFAULT: CLI-1915 (proposed)

use dc_client::hud::compose::{compose, resolve};
use dc_client::hud::state::{AmmoView, HudEvent};
use dc_client::hud::{HudChoice, HudDriver};
use dc_client::menu::drawlist::{check, FontTable};
use dc_client::menu::{cursor, Go, Menu, MenuInput, MenuOptions};
use dc_client::overlay::text::RgbaImage;
use dc_ui_data::VirtualScaler;
use std::path::PathBuf;

const SCREENS: [Go; 6] = [
    Go::Login,
    Go::Servers,
    Go::Channels,
    Go::Lobby,
    Go::Create,
    Go::Room,
];
const WIDE: [(u32, u32); 2] = [(3440, 1440), (5120, 1440)];
const BASE: (u32, u32) = (2560, 1440);
const RECORD_RULES: [&str; 7] = ["POS", "ALIGN", "FONT", "FSIZE", "DUP", "ORDER", "RANGE"];

fn rom() -> Option<PathBuf> {
    let r = dc_rom::test_rom();
    if r.is_none() {
        eprintln!("CA_ROM not set: skipped");
    }
    r
}

fn rom_menu(rom: &std::path::Path, (w, h): (u32, u32)) -> Menu {
    let mut menu = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: dc_client::menu::link::LinkOptions::new("ultrawide-test"),
        rom: Some(rom.to_path_buf()),
        font: None,
        width: w,
        height: h,
    });
    menu.detach_link();
    menu
}

/// Row `y` of `img` from column `x`, `n` pixels.
fn row(img: &RgbaImage, x: u32, y: u32, n: u32) -> &[u8] {
    let o = ((y * img.width + x) * 4) as usize;
    &img.rgba[o..o + (n * 4) as usize]
}

fn save(img: &RgbaImage, name: &str) {
    let Some(dir) = std::env::var_os("DC_ULTRAWIDE_PNG").map(PathBuf::from) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(format!("{name}.png"));
    let b = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone()).expect("image");
    b.save(&p).expect("png written");
    println!("wrote {}", p.display());
}

#[test]
fn rom_menu_screens_draw_the_16_9_band_centred() {
    let Some(rom) = rom() else {
        return;
    };
    // one menu resized from 2560 x 1440 to each wide size (the window's resize path), so a screen's own state (the
    // create box's random title) is the same in every list
    let mut m = rom_menu(&rom, BASE);
    for g in SCREENS {
        m.set_size(BASE.0, BASE.1);
        m.open(g);
        let want = m.items_at(0.0);
        let want_img = m.cat.compose(&want);
        for (w, h) in WIDE {
            m.set_size(w, h);
            let items = m.items_at(0.0);
            // the display list is the same in virtual coordinates: nothing of the layout changes
            if let Some(i) = (0..items.len().max(want.len())).find(|&i| items.get(i) != want.get(i)) {
                panic!(
                    "{w}x{h} {g:?}: the display list differs at item {i} of {}: {:?} vs 16:9 {:?}",
                    items.len(),
                    items.get(i),
                    want.get(i)
                );
            }
            let sc = VirtualScaler::new(w, h);
            let [bx, _, bw, _] = sc.band();
            assert_eq!((bw, bx * 2 + bw), (2560, w), "{w}x{h}: a centred 2560-pixel band");
            // the picture: the 2560 x 1440 one in the band, plain black beside it (every item but the cursor is
            // clipped to the band: a 2009 layout part reaching past 1024, off screen at 16:9, stays hidden)
            let img = m.cat.compose(&items);
            let black: Vec<u8> = [0, 0, 0, 255].repeat(bx as usize);
            for y in 0..h {
                assert_eq!(row(&img, bx, y, bw), row(&want_img, 0, y, bw), "{w}x{h} {g:?}: band row {y}");
                assert_eq!(row(&img, 0, y, bx), black.as_slice(), "{w}x{h} {g:?}: left band row {y}");
                assert_eq!(row(&img, bx + bw, y, bx), black.as_slice(), "{w}x{h} {g:?}: right band row {y}");
            }
            if g == Go::Lobby && w == 3440 {
                save(&img, "ultrawide_lobby_3440x1440");
            }
            // the record rules still hold
            let fonts = FontTable::from_catalog(&m.cat);
            let rows = m.draw_trace();
            let sheet = m.trace_sheet().clone();
            let bad: Vec<_> = check(&rows, &sheet, &fonts, &mut m.cat)
                .into_iter()
                .filter(|r| RECORD_RULES.contains(&r.class))
                .collect();
            assert!(bad.is_empty(), "{w}x{h} {g:?}: {bad:#?}");
        }
    }
}

#[test]
fn rom_a_click_in_the_band_hits_what_it_hits_at_16_9() {
    let Some(rom) = rom() else {
        return;
    };
    let base_sc = VirtualScaler::new(BASE.0, BASE.1);
    for (w, h) in WIDE {
        let sc = VirtualScaler::new(w, h);
        let mut base = rom_menu(&rom, BASE);
        let mut m = rom_menu(&rom, (w, h));
        for g in [Go::Login, Go::Lobby, Go::Create, Go::Room] {
            base.open(g);
            m.open(g);
            let (a, b) = (base.trace_sheet().clone(), m.trace_sheet().clone());
            let mut hits = 0;
            for p in a.parts.iter().filter(|p| p.shown && p.size[0] > 0 && p.size[1] > 0) {
                let c = [p.pos[0] + p.size[0] / 2, p.pos[1] + p.size[1] / 2];
                // the window pixel of the control's centre on each surface, mapped back as the windows do
                let px16 = base_sc.point(c);
                let pxw = sc.point(c);
                assert_eq!(pxw, [px16[0] + sc.ox, px16[1]], "{w}x{h}: the band's origin");
                let v16 = base_sc.to_virtual(px16);
                let vw = sc.to_virtual(pxw);
                let hit = a.hit(v16);
                assert_eq!(b.hit(vw), hit, "{w}x{h} {g:?}: the control at {c:?}");
                #[cfg(feature = "window")]
                {
                    use dc_client::menu::window::pointer_to_virtual;
                    let m16 = pointer_to_virtual([px16[0] as f64, px16[1] as f64], BASE);
                    let mw = pointer_to_virtual([pxw[0] as f64, pxw[1] as f64], (w, h));
                    assert_eq!(b.hit(mw), a.hit(m16), "{w}x{h} {g:?}: menu window at {c:?}");
                }
                hits += hit.is_some() as usize;
            }
            assert!(hits > 0, "{w}x{h} {g:?}: some control is hit");
            // a point in a side band is left of the virtual screen and hits nothing
            for x in [sc.ox * 0.5, w as f32 - sc.ox * 0.5] {
                let v = sc.to_virtual([x, h as f32 * 0.5]);
                assert!(!(0.0..=1024.0).contains(&v[0]), "{w}x{h}: {v:?}");
                assert_eq!(b.hit(v), None, "{w}x{h} {g:?}: side band at x {x}");
            }
        }
        // the same click on both surfaces leaves the same screen
        base.open(Go::Lobby);
        m.open(Go::Lobby);
        let sheet = base.trace_sheet().clone();
        let i = (0..sheet.parts.len())
            .rev()
            .find(|&i| {
                let p = &sheet.parts[i];
                let c = [p.pos[0] + p.size[0] / 2, p.pos[1] + p.size[1] / 2];
                sheet.hit([c[0] as f32, c[1] as f32]) == Some(i)
            })
            .expect("a clickable lobby control");
        let p = &sheet.parts[i];
        let c = [p.pos[0] + p.size[0] / 2, p.pos[1] + p.size[1] / 2];
        let at16 = base_sc.to_virtual(base_sc.point(c));
        let atw = sc.to_virtual(sc.point(c));
        for (menu, at) in [(&mut base, at16), (&mut m, atw)] {
            menu.input(MenuInput::Move(at));
            menu.input(MenuInput::Press(at));
            menu.input(MenuInput::Release(at));
        }
        assert_eq!(m.items_at(0.0), base.items_at(0.0), "{w}x{h}: the screen after the click");
    }
}

fn hud(rom: &std::path::Path, (w, h): (u32, u32)) -> HudDriver {
    let (mut d, _) = HudDriver::open(
        &HudChoice::Rom(rom.to_path_buf()),
        w,
        h,
        Some((dc_client::testkit::synth_font(), 0)),
    )
    .expect("ROM HUD");
    let rec = |pid: u8, name: &str, team: u8| dc_botclient::shell::RosterRecord {
        name: name.into(),
        pid,
        team,
        complete: true,
        ..Default::default()
    };
    for e in [
        HudEvent::Own {
            pid: Some(0),
            mode_class: Some(1),
        },
        HudEvent::Roster(rec(0, "AB", 1)),
        HudEvent::Roster(rec(1, "BA", 2)),
        HudEvent::Life(1),
        HudEvent::Status {
            kind: 2,
            value: 100.0,
        },
        HudEvent::RoundBegin(180),
    ] {
        d.state.apply(&e, 1000, 0.0);
    }
    d.state.ammo = Some(AmmoView {
        weapon: 1,
        clip: 24,
        reserve: 90,
        clip_size: Some(30),
        single_shot: false,
        order_class: 1,
        infinite: false,
    });
    d.state.aim_mark = Some(dc_client::spread::AimMarkView {
        shape: dc_client::spread::mark::SHAPE_PLAIN,
        distance: 10.0,
        since_shot_s: None,
        colour: dc_client::spread::mark::COLOUR,
        rest: 10.0,
    });
    let (fx, fy) = dc_render::fov_pair_2009(75.0, w, h);
    let cam = dc_render::Camera::looking(glam::Vec3::new(0.0, 60.0, 0.0), 0.0, 0.0, fx, fy);
    d.set_radar(&[], &cam);
    d
}

#[test]
fn rom_hud_at_ultrawide_is_the_16_9_hud_in_the_band() {
    let Some(rom) = rom() else {
        return;
    };
    let mut base = hud(&rom, BASE);
    let (want, _) = base.hud.items_spanned(&base.state.clone());
    let want_img = compose(&want, &mut base.hud.assets, BASE.0, BASE.1);
    for (w, h) in WIDE {
        let mut d = hud(&rom, (w, h));
        let state = d.state.clone();
        let (items, spans) = d.hud.items_spanned(&state);
        assert_eq!(items, want, "{w}x{h}: the HUD's display list");
        let sc = VirtualScaler::new(w, h);
        let [bx, _, bw, _] = sc.band();
        // radar, ammunition and crosshair resolve inside the band; the crosshair on the screen centre
        for name in ["radar", "ammo", "crosshair"] {
            let r = spans
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, r)| r.clone())
                .unwrap_or_default();
            assert!(!r.is_empty(), "{w}x{h}: {name} drawn (spans {spans:?})");
            let prims = resolve(&items[r.clone()], &mut d.hud.assets, w, h);
            assert!(!prims.is_empty(), "{w}x{h}: {name} resolves");
            // the drawn pixels of the element (a turned image's bounding box may reach further, transparent)
            let img = compose(&items[r], &mut d.hud.assets, w, h);
            let mut all = [u32::MAX, u32::MAX, 0, 0];
            for y in 0..h {
                for x in 0..w {
                    if img.pixel(x, y)[3] > 0 {
                        all = [all[0].min(x), all[1].min(y), all[2].max(x + 1), all[3].max(y + 1)];
                    }
                }
            }
            assert!(all[0] < all[2], "{w}x{h}: {name} draws pixels");
            assert!(
                all[0] >= bx && all[2] <= bx + bw,
                "{w}x{h}: {name} at {all:?} outside the band {bx}..{}",
                bx + bw
            );
            if name == "crosshair" {
                let all = all.map(|v| v as f32);
                let c = [(all[0] + all[2]) * 0.5, (all[1] + all[3]) * 0.5];
                assert!(
                    (c[0] - w as f32 / 2.0).abs() <= 1.5 && (c[1] - h as f32 / 2.0).abs() <= 1.5,
                    "{w}x{h}: the crosshair's centre {c:?}"
                );
            }
        }
        // the picture: the 2560 x 1440 HUD in the band, nothing beside it
        let img = compose(&items, &mut d.hud.assets, w, h);
        let clear = vec![0u8; (bx * 4) as usize];
        for y in 0..h {
            assert_eq!(row(&img, bx, y, bw), row(&want_img, 0, y, bw), "{w}x{h}: band row {y}");
            assert_eq!(row(&img, 0, y, bx), clear.as_slice(), "{w}x{h}: left band row {y}");
            assert_eq!(row(&img, bx + bw, y, bx), clear.as_slice(), "{w}x{h}: right band row {y}");
        }
        if w == 3440 {
            // over a mid grey so the transparent HUD shows (the band edges marked by a darker tone)
            let mut shot = RgbaImage::new(w, h, [90, 90, 90, 255]);
            for y in 0..h {
                for x in (0..bx).chain(bx + bw..w) {
                    let o = ((y * w + x) * 4) as usize;
                    shot.rgba[o..o + 3].copy_from_slice(&[60, 60, 60]);
                }
            }
            for (o, px) in shot.rgba.chunks_mut(4).zip(img.rgba.chunks(4)) {
                let a = px[3] as f32 / 255.0;
                for k in 0..3 {
                    o[k] = (px[k] as f32 * a + o[k] as f32 * (1.0 - a)).round() as u8;
                }
            }
            save(&shot, "ultrawide_hud_3440x1440");
        }
    }
}

/// The menu's own clipped composition leaves the cursor free: a pointer in a side band still shows it there.
#[test]
fn rom_the_cursor_shows_in_a_side_band() {
    let Some(rom) = rom() else {
        return;
    };
    let (w, h) = WIDE[0];
    let mut m = rom_menu(&rom, (w, h));
    m.open(Go::Login);
    let sc = VirtualScaler::new(w, h);
    let at = sc.to_virtual([sc.ox * 0.5, h as f32 * 0.5]);
    m.input(MenuInput::Move(at));
    let items = m.items_at(0.0);
    assert!(items.last().is_some_and(cursor::is_cursor), "the cursor is last");
    let img = m.cat.compose(&items);
    let x = (sc.ox * 0.5) as u32;
    let y = h / 2;
    let lit = (y..y + 20)
        .flat_map(|yy| (x..x + 12).map(move |xx| (xx, yy)))
        .filter(|&(xx, yy)| img.pixel(xx, yy) != [0, 0, 0, 255])
        .count();
    assert!(lit > 0, "the cursor image is drawn in the side band");
}
