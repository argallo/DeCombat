//! CLI-1170 (playtest report: the shop character "seems to be rendering at like 5fps"): every pose of the shop clip
//! parsed the body's model file twice (the shop clip's name, the weapon's grip socket), about 160 ms of a 170 ms
//! render at 2560 x 1440 (measured in release: 5-6 poses a second; after the fix about 3.5 ms a pose). The body
//! file is now read once per body model: stepping the clip parses nothing.
//!
//! ROM-gated (skipped without `CA_ROM`) and GPU-gated (no figure without an adapter). Its own test binary: the
//! parse counter is process-wide.

use std::sync::atomic::Ordering;

use dc_client::menu::link::LinkOptions;
use dc_client::menu::modelview::BODY_FILE_PARSES;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::steps::{resolve, Resolved};

fn click(m: &mut Menu, name: &str) {
    let Resolved::Part { at, .. } = resolve(m, name) else {
        panic!("{name}: {:?}", resolve(m, name));
    };
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
}

#[test]
fn rom_stepping_the_shop_clip_reads_no_model_file() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: 2560,
        height: 1440,
    });
    m.detach_link();
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    m.figure_time_ms = Some(0.0);
    let _ = m.image();
    if m.figure_renders() == 0 {
        eprintln!("skipped: no character view (no GPU adapter)");
        return;
    }
    let first = BODY_FILE_PARSES.load(Ordering::Relaxed);
    assert!(first >= 1, "the first pose reads the body file");
    for i in 1..6 {
        m.figure_time_ms = Some(i as f32 * 33.0);
        let _ = m.image();
    }
    assert!(m.figure_renders() >= 6, "every pose rendered");
    assert_eq!(
        BODY_FILE_PARSES.load(Ordering::Relaxed),
        first,
        "a pose step of the same body parses no model file"
    );
}
