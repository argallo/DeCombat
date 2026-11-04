//! CLI-1628 / CLI-1629: the shop page's x-ray controls next to the nickname and the preview on the shop character.
//! A click on the cycler steps the style (box, chams, skeleton) into `settings.xray.mode`, a swatch opens its palette
//! and a cell sets that tone; the change is saved in the profile and read back by a new menu; the shop's character
//! request carries the style; the character view draws each style on the model (pixels of the chosen colours).
//!
//! ROM-gated (skipped without `CA_ROM`); the pixel test is also GPU-gated (skipped without an adapter).

use std::path::Path;
use std::time::Duration;

use dc_client::hud::items::DrawItem;
use dc_client::menu::figure::{FigureTables, LookIds};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::modelview::{ModelView, ViewReq, PANE_RECT, PLACE};
use dc_client::menu::shop::NICK_BUTTON_CMD;
use dc_client::menu::shopxray::{XrayRects, PALETTE};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::settings::{ProfileStore, XrayMode, XrayStyle};
use dc_client::steps::{resolve, Resolved};

fn menu(rom: &Path, w: u32, h: u32) -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom.to_path_buf()),
        font: None,
        width: w,
        height: h,
    });
    m.detach_link();
    m
}

fn store(dir: &Path) -> ProfileStore {
    let file = dir.join("player.txt");
    ProfileStore {
        read: vec![file.clone()],
        write: Some(file),
        key: None,
    }
}

fn tap(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

fn centre(r: [i32; 4]) -> [f32; 2] {
    [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32]
}

fn open_shop(m: &mut Menu) {
    open_shop_on(m, true);
}

/// The shop of a lobby that does (`offered`) or does not offer wallhacks (CLI-1628).
fn open_shop_on(m: &mut Menu, offered: bool) {
    m.data.xray_offered = offered;
    m.open(Go::Lobby);
    let Resolved::Part { at, .. } = resolve(m, "lobby.shop") else {
        panic!("no lobby.shop");
    };
    tap(m, at);
    assert_eq!(m.screen_name(), "shop");
}

/// `DC_XRAY_SHOT=<dir>`: saves the shop page as `<name>.png` (a look at the controls and the preview).
fn shot(m: &mut Menu, name: &str) {
    if let Some(dir) = std::env::var_os("DC_XRAY_SHOT") {
        let i = m.image();
        let f = dc_render::FrameImage {
            width: i.width,
            height: i.height,
            rgba: i.rgba,
        };
        let _ = f.save_png(&Path::new(&dir).join(format!("{name}.png")));
    }
}

fn labels(m: &mut Menu) -> Vec<String> {
    m.items()
        .into_iter()
        .filter_map(|i| match i {
            DrawItem::Label { text, .. } | DrawItem::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn rom_the_cycler_and_the_palette_write_the_style_and_it_is_saved() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let dir = std::env::temp_dir().join(format!("dc_shop_xray_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut m = menu(&rom, 1024, 768);
    m.load_settings(store(&dir));
    open_shop(&mut m);
    assert_eq!(m.data.settings.xray, XrayStyle::default());
    assert!(labels(&mut m).iter().any(|t| t == "Wallhack: Box"));
    shot(&mut m, "shop_xray_box");
    let b = m.sheet().button(NICK_BUTTON_CMD).expect("the nickname box");
    let r = XrayRects::of_nick(b.pos, b.size);
    // the controls lie right of the nickname box, inside the left pane
    assert!(r.cycler[0] >= b.pos[0] + b.size[0] && r.cycler[0] + r.cycler[2] <= 460);
    tap(&mut m, centre(r.cycler));
    assert_eq!(m.data.settings.xray.mode, XrayMode::Chams);
    assert!(labels(&mut m).iter().any(|t| t == "Wallhack: Chams"));
    shot(&mut m, "shop_xray_chams");
    assert_eq!(
        m.figure_request().and_then(|q| q.xray).map(|s| s.mode),
        Some(XrayMode::Chams),
        "the shop's character previews the style"
    );
    tap(&mut m, centre(r.cycler));
    assert_eq!(m.data.settings.xray.mode, XrayMode::Skeleton);
    // the primary swatch's palette: cell 7 (cyan); the secondary: cell 3 (yellow)
    tap(&mut m, centre(r.swatch[0]));
    tap(&mut m, centre(XrayRects::cell(r.palette(0), 7)));
    tap(&mut m, centre(r.swatch[1]));
    shot(&mut m, "shop_xray_palette");
    tap(&mut m, centre(XrayRects::cell(r.palette(1), 3)));
    shot(&mut m, "shop_xray_skeleton");
    let want = XrayStyle {
        mode: XrayMode::Skeleton,
        primary: PALETTE[7],
        secondary: PALETTE[3],
        ..XrayStyle::default()
    };
    assert_eq!(m.data.settings.xray, want);
    assert!(labels(&mut m).iter().any(|t| t == "Bones"));
    // CLI-1630: the switch row above the cycler: name tags, then health bars
    assert!(labels(&mut m).iter().any(|t| t == "Nametags: Off"));
    assert!(r.toggles[1] + r.toggles[3] <= r.cycler[1]);
    tap(&mut m, centre(r.toggle[0]));
    tap(&mut m, centre(r.toggle[1]));
    shot(&mut m, "shop_xray_switches");
    let want = XrayStyle {
        nametags: true,
        health: true,
        ..want
    };
    assert_eq!(m.data.settings.xray, want);
    let l = labels(&mut m);
    assert!(l.iter().any(|t| t == "Nametags: On") && l.iter().any(|t| t == "Health: On"));
    // CLI-1635: the team mates' switch above them, on by default
    assert!(l.iter().any(|t| t == "Teammates: On"));
    assert!(r.mates[1] + r.mates[3] <= r.toggles[1]);
    assert!(r.mates[1] >= 0);
    tap(&mut m, centre(r.mates));
    shot(&mut m, "shop_xray_teammates");
    let want = XrayStyle {
        teammates: false,
        ..want
    };
    assert_eq!(m.data.settings.xray, want);
    assert!(labels(&mut m).iter().any(|t| t == "Teammates: Off"));
    // saved: a new menu reads it back from the profile
    let mut n = menu(&rom, 1024, 768);
    n.load_settings(store(&dir));
    assert_eq!(n.data.settings.xray, want);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pane render of the default look with `xray`, or `None` without a GPU.
fn figure(m: &mut Menu, rom: &Path, xray: Option<XrayStyle>) -> Option<Vec<[u8; 4]>> {
    let t = FigureTables::cached(rom);
    let ids = LookIds::from_equipped(&[-1; 15]);
    let (look, pieces) = t.look(&ids)?;
    let mut v = ModelView::new(rom.to_path_buf());
    let req = ViewReq {
        look: Some(look),
        pieces,
        lights: t.lights(&ids),
        place: PLACE,
        rect: PANE_RECT,
        xray,
    };
    v.want(Some(req), (1024, 768), Some(0.0), 0.0);
    v.wait(&mut m.cat.assets, Duration::from_secs(120));
    let DrawItem::Image { tex, .. } = v.item()? else {
        return None;
    };
    let t = m.cat.assets.generated(&tex)?;
    Some(t.rgba.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect())
}

fn count(px: &[[u8; 4]], c: [u8; 4]) -> usize {
    px.iter()
        .filter(|p| p[3] > 200 && (0..3).all(|i| p[i].abs_diff(c[i]) <= 12))
        .count()
}

#[test]
fn rom_the_preview_draws_each_style_on_the_shop_character() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let mut m = menu(&rom, 1024, 768);
    let Some(bare) = figure(&mut m, &rom, None) else {
        eprintln!("skipped: no character view (no GPU adapter)");
        return;
    };
    let (green, blue, magenta, yellow) = (
        [0x00, 0xff, 0x00, 0xff],
        [0x00, 0x00, 0xff, 0xff],
        [0xff, 0x00, 0xff, 0xff],
        [0xff, 0xff, 0x00, 0xff],
    );
    let style = |mode, primary, secondary| XrayStyle {
        mode,
        primary,
        secondary,
        ..XrayStyle::default()
    };
    for c in [green, blue, magenta, yellow] {
        assert!(count(&bare, c) < 20, "the bare figure has no {c:?}");
    }
    // box: the border in the secondary colour (2 px around the body)
    let boxed = figure(&mut m, &rom, Some(style(XrayMode::Box, green, blue))).unwrap();
    let border = count(&boxed, blue);
    assert!(border > 400, "box border pixels {border}");
    // chams: the body in two flat tones, mostly the facing (primary) one
    let chams = figure(&mut m, &rom, Some(style(XrayMode::Chams, magenta, blue))).unwrap();
    let (front, rim) = (count(&chams, magenta), count(&chams, blue));
    assert!(front > 3000 && rim > 50, "chams front {front} rim {rim}");
    // skeleton: bones in the primary colour, joints in the secondary
    let bones = figure(&mut m, &rom, Some(style(XrayMode::Skeleton, green, yellow))).unwrap();
    let (lines, joints) = (count(&bones, green), count(&bones, yellow));
    assert!(lines > 100 && joints > 50, "bones {lines} joints {joints}");
}

#[test]
fn rom_a_lobby_without_wallhacks_has_the_plain_shop_page() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let dir = std::env::temp_dir().join(format!("dc_shop_xray_van_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut m = menu(&rom, 1024, 768);
    m.load_settings(store(&dir));
    open_shop_on(&mut m, false);
    let b = m.sheet().button(NICK_BUTTON_CMD).expect("the nickname box");
    let r = XrayRects::of_nick(b.pos, b.size);
    assert!(!labels(&mut m).iter().any(|t| t.starts_with("Wallhack") || t.starts_with("Nametags")));
    assert!(m.figure_request().is_some_and(|q| q.xray.is_none()), "no preview");
    let before = m.data.settings.xray;
    tap(&mut m, centre(r.cycler));
    tap(&mut m, centre(r.swatch[0]));
    assert_eq!(m.data.settings.xray, before, "the click targets do nothing");
}
