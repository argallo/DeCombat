//! LG-306 / CLI-1640: a Hardcore entry's room row: the push 0x3f08 shows the row (the Hacks row's widgets) with the label
//! "Hardcore"; a room without the push keeps the 2009 layout. The layout part needs `CA_ROM` (skipped without it).

use dc_client::menu::link::{LinkEvent, LinkOptions, RoomView};
use dc_client::menu::room::{hacks_row, Room, SETTINGS_WINDOW};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_ui_data::WidgetKind;

#[test]
fn rom_the_row_follows_the_push_and_only_the_host_opens_it() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: CA_ROM unset");
        return;
    };
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "HUser".into(),
        link: LinkOptions::new("hardcore"),
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
    let text = |m: &Menu, id: i32| {
        let s = m.sheet();
        let i = s.find(SETTINGS_WINDOW, id, WidgetKind::Text).unwrap();
        (s.visible(i), s.parts[i].text.clone())
    };
    assert!(!text(&m, hacks_row::LABEL).0, "no row without the push");
    m.feed_event(LinkEvent::RoomHardcore(false));
    m.update();
    assert_eq!(text(&m, hacks_row::LABEL), (true, "Hardcore".to_string()));
    assert_eq!(text(&m, hacks_row::VALUE), (true, "Off".to_string()));
    m.feed_event(LinkEvent::RoomHardcore(true));
    m.update();
    assert_eq!(text(&m, hacks_row::VALUE).1, "On");
    let rows = Room::list_rows(&m.cat, &m.data, "hardcore");
    assert_eq!(rows, vec![("Off".to_string(), 0), ("On".to_string(), 1)]);
    m.input(MenuInput::Command(hacks_row::CMD));
    assert!(m.dropdown().is_some(), "the host's list");
    // a new room info clears it until the next push
    m.feed_event(LinkEvent::RoomInfo(RoomView {
        host_id: 2,
        ..RoomView::default()
    }));
    m.feed_event(LinkEvent::RoomHardcore(true));
    m.update();
    m.input(MenuInput::Command(hacks_row::CMD));
    m.update();
    assert_eq!(text(&m, hacks_row::VALUE).1, "On", "a guest sees it");
}
