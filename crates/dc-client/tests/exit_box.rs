//! Round 10 (CLI-920): the in-match exit box one Escape opens, drawn over the HUD centred on the window at its
//! real size (two sizes), with the hovered button in its over state, and the leave question behind its lobby
//! button; CLI-1907: our fourth button, Options, under the lobby button, inside the 2009 window. Made-up state only;
//! the ROM variant needs `CA_ROM` (nothing of the ROM in this file).

use dc_client::hud::compose::{resolve, Prim};
use dc_client::hud::{DrawItem, Hud, HudState};
use dc_client::loading::{LeavePage, LeaveView};
use dc_client::testkit::synth_font;

const SIZES: [(u32, u32); 2] = [(1024, 768), (1920, 1080)];

fn exit_view(hover: Option<usize>) -> HudState {
    HudState {
        leave_box: Some(LeaveView {
            page: LeavePage::Exit,
            hover,
            down: None,
        }),
        ..HudState::default()
    }
}

/// The box's window rectangle on the target (pixels) and the button rectangles.
fn box_rects(hud: &mut Hud, st: &HudState, w: u32, h: u32) -> ([f32; 4], Vec<[f32; 4]>) {
    hud.set_size(w, h);
    let items = hud.items(st);
    let win = hud.exit_box.window;
    let buttons = hud.exit_box.buttons;
    let sx = w as f32 / 1024.0;
    let sy = h as f32 / 768.0;
    let px = |r: [i32; 4]| {
        [
            r[0] as f32 * sx,
            r[1] as f32 * sy,
            (r[0] + r[2]) as f32 * sx,
            (r[1] + r[3]) as f32 * sy,
        ]
    };
    // the drawn rectangles of the box's items (the window first, then the buttons)
    let drawn: Vec<[f32; 4]> = resolve(&items, &mut hud.assets, w, h)
        .iter()
        .filter_map(|p| match p {
            Prim::Image { rect, .. } => Some(*rect),
            Prim::Fill { rect, .. } => Some(rect.map(|v| v as f32)),
            _ => None,
        })
        .collect();
    let want = px(win);
    let near = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1.0);
    assert!(
        drawn.iter().any(|r| near(*r, want)),
        "window drawn at {want:?} on {w}x{h}: {drawn:?}"
    );
    (want, buttons.iter().map(|b| px(*b)).collect())
}

fn check_centred(hud: &mut Hud) {
    for (w, h) in SIZES {
        let (win, buttons) = box_rects(hud, &exit_view(None), w, h);
        let c = [(win[0] + win[2]) / 2.0, (win[1] + win[3]) / 2.0];
        assert!(
            (c[0] - w as f32 / 2.0).abs() <= 0.01 * w as f32
                && (c[1] - h as f32 / 2.0).abs() <= 0.01 * h as f32,
            "box centred on {w}x{h}: centre {c:?}"
        );
        // CLI-1907: lobby, report, back to the game and our Options
        assert_eq!(buttons.len(), 4);
        for b in &buttons {
            assert!(
                b[0] >= win[0] && b[2] <= win[2] && b[1] >= win[1] && b[3] <= win[3],
                "button {b:?} inside the window {win:?}"
            );
        }
    }
}

fn image_names(items: &[DrawItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Image { tex, .. } => Some(tex.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_exit_box_is_centred_at_two_window_sizes() {
    let mut hud = Hud::fallback(640, 480, Some((synth_font(), 0)));
    check_centred(&mut hud);
}

#[test]
fn the_question_follows_the_lobby_button_with_hover() {
    let mut hud = Hud::fallback(1024, 768, Some((synth_font(), 0)));
    let st = HudState {
        leave_box: Some(LeaveView {
            page: LeavePage::Ask,
            hover: Some(0),
            down: None,
        }),
        ..HudState::default()
    };
    let labels: Vec<String> = hud
        .items(&st)
        .iter()
        .filter_map(|i| match i {
            DrawItem::Label { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    // title 0x371, text 0xbbf, Yes 0x35, No 0x36 (string ids: the fallback HUD writes `#id`)
    for want in ["#881", "#3007", "#53", "#54"] {
        assert!(labels.iter().any(|t| t == want), "{want}: {labels:?}");
    }
}

#[test]
fn rom_exit_box_is_centred_and_hover_shows_the_over_image() {
    let Ok(rom) = std::env::var("CA_ROM") else {
        return;
    };
    let mut hud = Hud::from_rom(std::path::Path::new(&rom), 1024, 768, None).expect("ROM HUD");
    let b = hud.exit_box.clone();
    assert!(b.window_tex.is_some(), "the 2009 window image resolves");
    assert!(b.button_tex.iter().flatten().all(|t| t.is_some()));
    // CLI-1907: Options is drawn with the message boxes' button picture, right under the lobby button; the 2009
    // buttons keep their order downwards
    assert!(
        b.options_art.iter().all(|t| t.is_some()),
        "the button picture resolves"
    );
    let o = b.buttons[dc_client::loading::EXIT_OPTIONS];
    let lobby = b.buttons[dc_client::loading::EXIT_LOBBY];
    let report = b.buttons[dc_client::loading::EXIT_REPORT];
    let game = b.buttons[dc_client::loading::EXIT_CANCEL];
    assert!(
        lobby[1] < o[1] && o[1] < report[1] && report[1] < game[1],
        "{:?}",
        b.buttons
    );
    assert!(
        o[1] - lobby[1] >= lobby[3] && report[1] - o[1] >= o[3],
        "no overlap: {:?}",
        b.buttons
    );
    let drawn = image_names(&hud.items(&exit_view(Some(dc_client::loading::EXIT_OPTIONS))));
    let lit = b.options_art[1].clone().unwrap();
    assert!(
        drawn.iter().any(|t| t.starts_with(&lit)),
        "hovered Options: the over picture: {drawn:?}"
    );
    check_centred(&mut hud);
    hud.set_size(1024, 768);
    let plain = image_names(&hud.items(&exit_view(None)));
    let over = image_names(&hud.items(&exit_view(Some(1))));
    let normal = b.button_tex[1][0].clone().unwrap();
    let lit = b.button_tex[1][1].clone().unwrap();
    assert!(plain.contains(&normal) && !plain.contains(&lit));
    assert!(
        over.contains(&lit) && !over.contains(&normal),
        "hovered: over image"
    );
    // the window's whole image is drawn at its texture-cut size (no stretch of the 512 x 256 picture)
    let dump = std::env::var("DC_EXITBOX_PNG").ok();
    if let Some(dir) = dump {
        for (w, h) in SIZES {
            hud.set_size(w, h);
            let img = hud.render(&exit_view(Some(0)));
            image::save_buffer(
                format!("{dir}/exitbox_{w}x{h}.png"),
                &img.rgba,
                img.width,
                img.height,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        }
    }
}

/// CLI-1012: a click on one of the exit box's buttons plays the interface select sound, as the menu's buttons do
/// (cshell+0x1aeb90); the fallback box (no ROM) has no file and stays silent.
#[test]
fn rom_exit_box_buttons_click_with_the_select_sound() {
    use dc_client::sound::SoundCue;
    assert!(Hud::fallback(1024, 768, None)
        .exit_box
        .select_cue()
        .is_none());
    let Ok(rom) = std::env::var("CA_ROM") else {
        return;
    };
    let hud = Hud::from_rom(std::path::Path::new(&rom), 1024, 768, None).expect("ROM HUD");
    match hud.exit_box.select_cue() {
        Some(SoundCue::Interface { file, what }) => {
            assert!(
                file.is_some_and(|f| !f.is_empty()),
                "the select sound's file"
            );
            assert_eq!(what, "select");
        }
        other => panic!("no select cue: {other:?}"),
    }
}
