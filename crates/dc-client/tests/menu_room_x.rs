//! lobby-x-after-leave (CLI-1316): the top-right X of the bar asks the quit question on the waiting room too. A match
//! (left early or ended) hands the player back to the waiting room (`Menu::back_from_game` / the result screen's
//! exit), where the X did nothing; Escape left the room for the lobby, where it worked (user, build 3af05e0c).
//! The command part runs without the ROM; the pointer part needs `CA_ROM` (skipped without it).

use dc_botclient::lobby::{LobbyProfile, RoomMember};
use dc_client::menu::link::{LinkOptions, RoomView};
use dc_client::menu::shop::CMD_BAR_QUIT;
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;

fn menu(rom: bool) -> Option<Menu> {
    let rom = if rom {
        Some(dc_rom::resolve_rom().ok()?)
    } else {
        None
    };
    let font = rom.is_none().then(|| (synth_font(), 0));
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "XUser".into(),
        link: LinkOptions::new("lobby-x-after-leave"),
        rom,
        font,
        width: 1024,
        height: 768,
    });
    m.detach_link();
    let d = &mut m.data;
    d.profile = LobbyProfile {
        nick: "XUser".into(),
        my_id: 1,
        ..LobbyProfile::default()
    };
    d.room = RoomView {
        title: "x".into(),
        host_id: 1,
        players: 2,
        max_players: 8,
        goal: 50,
        ..RoomView::default()
    };
    for (id, team) in [(1u32, 1), (2, 2)] {
        d.members.insert(
            id,
            RoomMember {
                char_no: id as i32,
                name: format!("P{id}"),
                team,
                state: 0,
                exp: 0,
            },
        );
    }
    // the screen a match hands back to (`Menu::back_from_game` switches to it)
    m.open(Go::Room);
    Some(m)
}

#[test]
fn the_bar_x_command_asks_to_quit_in_the_waiting_room() {
    let mut m = menu(false).unwrap();
    assert_eq!(m.screen_name(), "room");
    m.input(MenuInput::Command(CMD_BAR_QUIT));
    assert_eq!(m.screen_name(), "room");
    assert_eq!(m.popup_count(), 1, "the quit box is up");
}

#[test]
fn rom_a_click_on_the_bar_x_asks_to_quit_in_the_waiting_room() {
    let Some(mut m) = menu(true) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let s = m.sheet();
    let i = (0..s.parts.len())
        .find(|&i| {
            s.parts[i].command == CMD_BAR_QUIT
                && s.parts[i].kind == WidgetKind::Button
                && s.visible(i)
        })
        .expect("the waiting room shows the bar's X");
    let p = &s.parts[i];
    let r = p.hit.unwrap_or([p.pos[0], p.pos[1], p.size[0], p.size[1]]);
    let c = [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32];
    m.input(MenuInput::Move(c));
    m.input(MenuInput::Press(c));
    m.input(MenuInput::Release(c));
    assert_eq!(m.screen_name(), "room");
    assert_eq!(m.popup_count(), 1, "the X's click opened the quit box");
    // Escape closes the box and leaves the player in the room (the box's No)
    m.input(MenuInput::Key(MenuKey::Escape));
    assert_eq!(m.popup_count(), 0);
    assert_eq!(m.screen_name(), "room");
}

#[test]
fn the_bar_x_command_asks_to_quit_on_the_create_page() {
    let mut m = menu(false).unwrap();
    m.open(Go::Create);
    m.input(MenuInput::Command(CMD_BAR_QUIT));
    assert_eq!(m.screen_name(), "create");
    assert_eq!(m.popup_count(), 1, "the quit box is up");
}

/// CLI-1310 stays: while the start countdown holds the room the X does nothing.
#[test]
fn the_start_lock_still_holds_the_x() {
    use dc_client::menu::link::LinkEvent;
    let mut m = menu(false).unwrap();
    for e in [
        LinkEvent::MemberState {
            id: 1,
            state: 2,
            sub: 0,
        },
        LinkEvent::Countdown(0),
    ] {
        m.feed_event(e);
    }
    m.update();
    m.input(MenuInput::Command(CMD_BAR_QUIT));
    assert_eq!(m.popup_count(), 0, "locked: no quit box");
}
