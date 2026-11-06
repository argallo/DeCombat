//! room-password (CLI-1905): the waiting room's Change password button (team window control 8, command 20). The
//! host's button opens the lock box (title 0x10be, text 0x1036, masked, 4 characters; cshell+0x15fc80); OK sends
//! 0x38c type 1 in the password form (cshell+0x15baf0 -> command 0x53 -> cshell+0x18a9c0), Cancel nothing. A guest
//! does not see the button (cshell+0x15db30). Runs without the ROM except the last test (string ids read back as
//! `#<id>`). Passwords are fixed dummy values.

use dc_botclient::lobby::{LobbyProfile, RoomMember};
use dc_client::menu::link::{room_password_packet, LinkOptions, RoomView};
use dc_client::menu::popup::{BoxKind, BoxTag, LOCK_ROOM_MAX, LOCK_ROOM_TEXT, LOCK_ROOM_TITLE};
use dc_client::menu::room::{CHANGE_PASSWORD_BUTTON, CMD_CHANGE_PASSWORD, TEAM_WINDOW};
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;

fn menu_with(rom: Option<std::path::PathBuf>) -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "LockUser".into(),
        link: LinkOptions::new("room-password"),
        rom,
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m
}

/// A two-player room in mode 3; the own player (id 1) is the host when `host`.
fn room(m: &mut Menu, host: bool) {
    let d = &mut m.data;
    d.profile = LobbyProfile {
        nick: "LockUser".into(),
        my_id: 1,
        ..LobbyProfile::default()
    };
    d.room = RoomView {
        title: "lock".into(),
        host_id: if host { 1 } else { 2 },
        players: 2,
        max_players: 8,
        mode: 3,
        goal: 50,
        ..RoomView::default()
    };
    for id in 1..=2u32 {
        d.members.insert(
            id,
            RoomMember {
                char_no: id as i32,
                name: format!("P{id}"),
                team: id as i32,
                state: 0,
                exp: 0,
            },
        );
    }
    m.open(Go::Room);
}

fn button_shown(m: &Menu) -> bool {
    let s = m.sheet();
    s.find(TEAM_WINDOW, CHANGE_PASSWORD_BUTTON, WidgetKind::Button)
        .is_some_and(|i| s.parts[i].shown)
}

#[test]
fn the_hosts_button_opens_the_lock_box_and_ok_sends_the_password() {
    let mut m = menu_with(None);
    room(&mut m, true);
    let _ = m.take_link_sent();
    m.input(MenuInput::Command(CMD_CHANGE_PASSWORD));
    let p = m.popup().expect("the lock box");
    assert_eq!(p.tag, BoxTag::LockRoom);
    assert_eq!(p.kind, BoxKind::Input);
    assert_eq!(p.title, LOCK_ROOM_TITLE);
    assert_eq!(p.text, m.cat.text(LOCK_ROOM_TEXT));
    assert!(!p.plain, "the password is masked");
    for c in "ab12xyz".chars() {
        m.input(MenuInput::Char(c));
    }
    assert_eq!(m.popup().unwrap().input.chars().count(), LOCK_ROOM_MAX);
    assert!(m.take_link_sent().is_empty(), "nothing goes out before OK");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(m.popup().is_none());
    assert_eq!(
        m.take_link_sent(),
        vec!["RoomPassword(locked true)".to_string()],
        "the Debug form never shows the password"
    );
}

#[test]
fn an_empty_ok_clears_the_password_and_cancel_sends_nothing() {
    let mut m = menu_with(None);
    room(&mut m, true);
    let _ = m.take_link_sent();
    m.input(MenuInput::Command(CMD_CHANGE_PASSWORD));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(
        m.take_link_sent(),
        vec!["RoomPassword(locked false)".to_string()]
    );
    m.input(MenuInput::Command(CMD_CHANGE_PASSWORD));
    m.input(MenuInput::Char('q'));
    m.input(MenuInput::Key(MenuKey::Escape));
    assert!(m.popup().is_none());
    assert!(m.take_link_sent().is_empty());
}

#[test]
fn a_guest_has_no_button_and_its_command_opens_nothing() {
    let mut m = menu_with(None);
    room(&mut m, false);
    let _ = m.take_link_sent();
    m.input(MenuInput::Command(CMD_CHANGE_PASSWORD));
    assert!(m.popup().is_none());
    assert!(m.take_link_sent().is_empty());
}

#[test]
fn the_password_form_is_0x58_bytes_with_type_1_the_length_and_the_text() {
    let b = room_password_packet(3, "ab12").payload();
    assert_eq!(b.len(), 0x58);
    let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    assert_eq!(u16_at(0), 0x38c);
    assert_eq!(u32_at(2), 0x58);
    assert_eq!(u32_at(6), 3, "the room's mode");
    assert_eq!(u32_at(0xa), 1, "type 1: locked");
    assert_eq!(u32_at(0xe), 0);
    assert_eq!(u16_at(0x12), 4);
    let text: Vec<u16> = (0..5).map(|i| u16_at(0x16 + 2 * i)).collect();
    assert_eq!(
        text,
        vec![b'a' as u16, b'b' as u16, b'1' as u16, b'2' as u16, 0]
    );
    // empty: length 0, the field all zero
    let e = room_password_packet(0, "").payload();
    assert_eq!(e.len(), 0x58);
    assert!(e[0x12..].iter().all(|&x| x == 0));
}

/// With the ROM: the host sees the ROM's Change password button (command 20), a guest does not. ROM-gated.
#[test]
fn rom_the_change_password_button_is_the_hosts() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let mut m = menu_with(Some(rom.clone()));
    room(&mut m, true);
    assert!(m.sheet().button(CMD_CHANGE_PASSWORD).is_some());
    assert!(button_shown(&m));
    let mut g = menu_with(Some(rom));
    room(&mut g, false);
    assert!(g.sheet().button(CMD_CHANGE_PASSWORD).is_none());
    assert!(!button_shown(&g));
}
