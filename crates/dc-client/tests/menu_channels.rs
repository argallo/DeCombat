//! round 10 (CLI-910): the channel list's selected row shows the Click bar's arrow and no state icon (the icon drawn
//! over it hid the arrow); the other rows keep their icon.
//!
//! ROM-gated (skipped without `CA_ROM`).

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{ChannelRow, LinkOptions};
use dc_client::menu::{Go, Menu, MenuOptions};

#[test]
fn rom_channel_list_selected_row_hides_its_state_icon() {
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
    let row = |id: u32| ChannelRow {
        id,
        name: format!("Channel {id}"),
        open: 1,
        capacity: 100,
        current: 1,
        op_capacity: 0,
        op_current: 0,
    };
    m.data.channels = vec![row(1), row(2), row(3)];
    m.open(Go::Channels);
    let icon = m.cat.paths.state_ok.clone().expect("state icon texture");
    let icons = |m: &mut Menu| {
        m.items()
            .iter()
            .filter(|i| matches!(i, DrawItem::Image { tex, .. } if *tex == icon))
            .count()
    };
    // no row is selected on open: an icon on each of the three rows
    assert_eq!(icons(&mut m), 3);
    // the first row selected: two icons
    m.input(dc_client::menu::MenuInput::Key(dc_client::menu::MenuKey::Down));
    assert_eq!(icons(&mut m), 2);
}
