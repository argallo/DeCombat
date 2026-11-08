//! CLI-1303 (amended): the Character Info entry of the waiting room's member menu opens the Character Info box over
//! the waiting room when the profile reply comes (owner report 2026-11-07: it only showed in the channel lobby).
//! Runs without the ROM.

use dc_botclient::lobby::{LobbyProfile, RoomMember};
use dc_client::menu::charinfo;
use dc_client::menu::link::{LinkEvent, LinkOptions, ProfileView, RoomView};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::testkit::synth_font;

fn room_menu() -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "casey".into(),
        link: LinkOptions::new("room-charinfo"),
        rom: None,
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    let d = &mut m.data;
    d.profile = LobbyProfile {
        nick: "casey".into(),
        my_id: 1,
        ..LobbyProfile::default()
    };
    d.room = RoomView {
        title: "r".into(),
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
    m.open(Go::Room);
    m
}

#[test]
fn a_profile_reply_opens_the_box_over_the_waiting_room_and_ok_returns_to_it() {
    let mut m = room_menu();
    assert_eq!(m.screen_name(), "room");
    m.feed_event(LinkEvent::ProfileInfo(ProfileView {
        name: "P2".into(),
        ..ProfileView::default()
    }));
    m.update();
    assert_eq!(m.screen_name(), charinfo::NAME, "the reply opens the box");
    assert_eq!(
        m.data.char_view.as_ref().map(|v| v.name.as_str()),
        Some("P2")
    );
    // the room's state is kept underneath
    assert_eq!(m.data.members.len(), 2);
    assert_eq!(m.data.room.title, "r");
    // OK closes the box: back on the waiting room
    let ok = [513.0, 575.0];
    m.input(MenuInput::Move(ok));
    m.input(MenuInput::Press(ok));
    m.input(MenuInput::Release(ok));
    assert_eq!(m.screen_name(), "room");
    assert_eq!(m.data.members.len(), 2);
}
