//! (task 24, CLI-951) The bar's Messenger button is a toggle: a press while the window is open must not close it on
//! the press (the outside-press rule) and reopen it on the release; the release shuts it. ROM-gated: without the
//! ROM layout the bar has no button rectangle to press (skipped).

use dc_botclient::lobby::LobbyProfile;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::messenger::BAR_FRIENDS_CMD;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;

fn menu(go: Go) -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("menu-messenger-toggle"),
        rom: dc_rom::resolve_rom().ok(),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.data.profile = LobbyProfile {
        nick: "tester".into(),
        char_no: 1,
        my_id: 1,
        chan_label: "Channel 1".into(),
        ..LobbyProfile::default()
    };
    m.open(go);
    m
}

/// The centre of the bar's messenger button on the current screen.
fn button_centre(m: &Menu) -> Option<[f32; 2]> {
    let s = m.sheet();
    let i = (0..s.parts.len()).find(|&i| {
        s.parts[i].command == BAR_FRIENDS_CMD
            && s.parts[i].kind == WidgetKind::Button
            && s.visible(i)
    })?;
    let p = &s.parts[i];
    let r = p.hit.unwrap_or([p.pos[0], p.pos[1], p.size[0], p.size[1]]);
    Some([(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32])
}

#[test]
fn a_press_on_the_messenger_button_while_open_closes_it_on_the_release_only() {
    for g in [Go::Lobby, Go::Shop, Go::Clan] {
        let mut m = menu(g);
        let Some(c) = button_centre(&m) else {
            eprintln!("skipped: no bar button without the ROM layout ({g:?})");
            return;
        };
        // press + release opens
        m.input(MenuInput::Move(c));
        m.input(MenuInput::Press(c));
        m.input(MenuInput::Release(c));
        assert!(m.messenger_open(), "opens on {g:?}");
        // the press alone keeps it open (it used to close here and the release reopened it)
        m.input(MenuInput::Press(c));
        assert!(
            m.messenger_open(),
            "still open while the button is held on {g:?}"
        );
        m.input(MenuInput::Release(c));
        assert!(!m.messenger_open(), "the release closes it on {g:?}");
        // and once more: the next click opens again
        m.input(MenuInput::Press(c));
        m.input(MenuInput::Release(c));
        assert!(m.messenger_open(), "opens again on {g:?}");
    }
}

#[test]
fn a_press_outside_the_window_and_the_button_still_closes_it() {
    let mut m = menu(Go::Lobby);
    let Some(c) = button_centre(&m) else {
        eprintln!("skipped: no bar button without the ROM layout");
        return;
    };
    m.toggle_messenger();
    assert!(m.messenger_open());
    // a corner of the screen: not the window, not the button
    let far = [4.0, 4.0];
    assert!((far[0] - c[0]).abs() > 1.0);
    m.input(MenuInput::Press(far));
    assert!(!m.messenger_open(), "the outside press closes the window");
}
