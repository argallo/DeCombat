//! CLI-1263 (playtest report: "my character body should be able to be rotated around if you click and drag on them"):
//! a left-button drag on the shop pane's character turns it, 1/60 rad per screen pixel, its front following the
//! pointer; the request the character view renders carries the new yaw. Headless, ROM-gated.

use dc_client::menu::link::LinkOptions;
use dc_client::menu::modelview::PLACE;
use dc_client::menu::shop::FIGURE_TURN_PER_PIXEL;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};

fn menu(width: u32, height: u32) -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width,
        height,
    });
    m.detach_link();
    m.open(Go::Shop);
    Some(m)
}

fn yaw(m: &Menu) -> f32 {
    m.figure_request().expect("the shop shows the character").place.yaw
}

fn close(a: f32, b: f32) -> bool {
    let d = (a - b).rem_euclid(std::f32::consts::TAU);
    d.min(std::f32::consts::TAU - d) < 1e-4
}

#[test]
fn rom_a_drag_on_the_character_turns_it_by_the_screen_pixels() {
    for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
        let Some(mut m) = menu(w, h) else {
            eprintln!("skipped: no CA_ROM");
            return;
        };
        assert!(close(yaw(&m), PLACE.yaw));
        let sx = w as f32 / 1024.0;
        m.input(MenuInput::Move([100.0, 300.0]));
        m.input(MenuInput::Press([100.0, 300.0]));
        m.input(MenuInput::Move([130.0, 310.0]));
        m.input(MenuInput::Move([160.0, 300.0]));
        let want = PLACE.yaw - 60.0 * sx * FIGURE_TURN_PER_PIXEL;
        assert!(close(yaw(&m), want), "{w}x{h}: {} != {want}", yaw(&m));
        m.input(MenuInput::Release([160.0, 300.0]));
        // after the release the pointer no longer turns it
        m.input(MenuInput::Move([300.0, 300.0]));
        assert!(close(yaw(&m), want));
        // back to the left turns it back
        m.input(MenuInput::Press([160.0, 300.0]));
        m.input(MenuInput::Move([100.0, 300.0]));
        m.input(MenuInput::Release([100.0, 300.0]));
        assert!(close(yaw(&m), PLACE.yaw));
    }
}

#[test]
fn rom_a_press_outside_the_character_does_not_turn_it() {
    let Some(mut m) = menu(1024, 768) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    // the equipment rows right of the character and the bar above it
    for p in [[300.0, 300.0], [100.0, 60.0], [100.0, 600.0]] {
        m.input(MenuInput::Move(p));
        m.input(MenuInput::Press(p));
        m.input(MenuInput::Move([p[0] + 50.0, p[1]]));
        m.input(MenuInput::Release([p[0] + 50.0, p[1]]));
        assert!(close(yaw(&m), PLACE.yaw), "press at {p:?}");
    }
}
