//! lobby-leftovers11: the room creation box starts with one of the 2009 default titles (strings 0x414 .. 0x418,
//! picked by the tick count, cshell+0x139770 / 0x1399e0), not the nickname. ROM-gated (skipped without `CA_ROM`).

use dc_client::menu::create::{default_title, TITLES, TITLE_BOX, WINDOW};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::{Go, Menu, MenuOptions};
use dc_ui_data::WidgetKind;

#[test]
fn the_tick_picks_each_title_in_turn() {
    for (i, id) in TITLES.iter().enumerate() {
        assert_eq!(default_title(i as u64 + 5 * 1234), *id);
    }
}

#[test]
fn rom_create_box_starts_with_a_default_title() {
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
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.data.profile.nick = "Nick".into();
    m.open(Go::Create);
    let title = m
        .sheet()
        .part(WINDOW, TITLE_BOX, WidgetKind::EditBox)
        .map(|p| p.text.clone())
        .unwrap();
    let list: Vec<String> = TITLES.iter().map(|id| m.cat.text(*id)).collect();
    assert!(list.contains(&title), "{title:?} not in {list:?}");
    assert!(list.iter().any(|t| t == "Nowhere to Hide"));
}
