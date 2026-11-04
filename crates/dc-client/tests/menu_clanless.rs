//! inbox-clan: a clanless account's clan page shows the 2009 clanless texts ("(Unaffiliated)" and "-" values,
//! cshell+0x10c5c0), and the lobby's Character Info panel draws the empty mark frame shrunk to the mark's 26 x 26
//! (cshell+0x121663..0x121693) instead of its full size over the clan row.
//!
//! ROM-gated (skipped without `CA_ROM`).

use dc_client::hud::items::DrawItem;
use dc_client::menu::clan::{
    CHAR_MARK_FRAME, CHAR_WINDOW, EXP_TEXT, INFO_WINDOW, LEADER_TEXT, MARK_SIZE, MEMBERS_TEXT,
    NAME_TEXT, TEXT_NO_VALUE, TEXT_UNAFFILIATED,
};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::{Go, Menu, MenuOptions};
use dc_ui_data::WidgetKind;

fn menu() -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
    Some(Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    }))
}

#[test]
fn rom_clanless_page_shows_unaffiliated_and_dashes() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.open(Go::Lobby);
    m.open(Go::Clan);
    assert_eq!(m.screen_name(), "clan");
    let text = |m: &Menu, id| {
        m.sheet()
            .part(INFO_WINDOW, id, WidgetKind::Text)
            .map(|p| p.text.clone())
            .unwrap_or_default()
    };
    let none = m.cat.text(TEXT_UNAFFILIATED);
    let dash = m.cat.text(TEXT_NO_VALUE);
    assert!(!none.is_empty() && !dash.is_empty());
    assert_eq!(text(&m, NAME_TEXT), none);
    for id in [LEADER_TEXT, MEMBERS_TEXT, EXP_TEXT] {
        assert_eq!(text(&m, id), dash, "text {id}");
    }
    // the drawn page carries them
    let drawn: Vec<String> = m
        .items()
        .iter()
        .filter_map(|i| match i {
            DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(drawn.contains(&none));
    assert!(drawn.iter().filter(|t| **t == dash).count() >= 6);
}

#[test]
fn rom_lobby_mark_frame_is_drawn_at_the_mark_size() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.open(Go::Lobby);
    let frame = m
        .sheet()
        .part(CHAR_WINDOW, CHAR_MARK_FRAME, WidgetKind::Image)
        .expect("mark frame")
        .clone();
    assert!(frame.shown);
    let at = [frame.pos[0] as f32, frame.pos[1] as f32];
    let hit = m.items().into_iter().find_map(|i| match i {
        DrawItem::Image { dst, .. } if dst[0] == at[0] && dst[1] == at[1] => Some(dst),
        _ => None,
    });
    let dst = hit.expect("frame drawn");
    assert_eq!([dst[2], dst[3]], [MARK_SIZE[0] as f32, MARK_SIZE[1] as f32]);
}
