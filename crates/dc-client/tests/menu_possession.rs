//! LG-307 / CLI-1641: the Possession row of the Arms list. The push 0x3f0a offers it; it goes away on the refused modes
//! and in a Gun Game room; a room without the push has the ROM's rows alone. Needs `CA_ROM` (skipped without it).

use dc_client::menu::link::{LinkEvent, LinkOptions, RoomView};
use dc_client::menu::room::{Room, POSSESSION_ROW};
use dc_client::menu::{Go, Menu, MenuOptions};

#[test]
fn rom_the_arms_row_follows_the_push_and_the_mode() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: CA_ROM unset");
        return;
    };
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "PUser".into(),
        link: LinkOptions::new("possession"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.data.profile.my_id = 1;
    m.data.room = RoomView {
        host_id: 1,
        ..RoomView::default()
    };
    m.open(Go::Room);
    m.update();
    let has = |m: &Menu| {
        Room::list_rows(&m.cat, &m.data, "arms")
            .iter()
            .any(|r| r.1 == POSSESSION_ROW && r.0 == "Possession")
    };
    assert!(!has(&m), "no row without the push");
    let plain = Room::list_rows(&m.cat, &m.data, "arms").len();
    m.feed_event(LinkEvent::RoomPossession(false));
    m.update();
    let row_of = |m: &Menu, class: u8| (0..32).find(|i| m.cat.mode_class(*i) == Some(class));
    for (class, offered) in [(1, true), (6, true), (5, false), (9, false), (11, false)] {
        let row = row_of(&m, class).expect("a ROM row of the class");
        m.data.room.mode = row;
        assert_eq!(has(&m), offered, "class {class}");
    }
    m.data.room.mode = row_of(&m, 6).unwrap();
    assert_eq!(Room::list_rows(&m.cat, &m.data, "arms").len(), plain + 1);
    m.data.room.gun_game = true;
    assert!(!has(&m), "not in a Gun Game room");
}
