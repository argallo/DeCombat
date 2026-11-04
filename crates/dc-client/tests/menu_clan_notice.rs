//! lobby20 item 2: the clan Announcement box. Leader view: the field shows what is typed (white, wrapped) with the
//! "n / 80" counter and takes 80 characters; member view: read only (the refusal box 0x4cb, no edit window).
//!
//! ROM-gated (skipped without `CA_ROM`), no lobby.

use dc_client::hud::items::DrawItem;
use dc_client::menu::clan::{CMD_NOTICE, NOTICE_MAX, NOTICE_WINDOW};
use dc_client::menu::link::{LinkOptions, CLAN_ROLE_LEADER};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_ui_data::WidgetKind;

fn menu(role: u32) -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
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
    m.open(Go::Lobby);
    m.data.char_extras.clan_id = 7;
    m.data.char_extras.clan_role = role;
    m.open(Go::Clan);
    Some(m)
}

fn shown(m: &Menu) -> bool {
    m.sheet()
        .part(NOTICE_WINDOW, NOTICE_WINDOW, WidgetKind::Window)
        .is_some_and(|p| p.shown)
}

#[test]
fn rom_leader_notice_field_draws_text_and_counter_and_stops_at_80() {
    let Some(mut m) = menu(CLAN_ROLE_LEADER) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.input(MenuInput::Command(CMD_NOTICE));
    assert!(shown(&m));
    for c in "QRSTUVWXYZ".repeat(12).chars() {
        m.input(MenuInput::Char(c));
    }
    let items = m.items_at(0.0);
    let labels: Vec<(&String, [u8; 4])> = items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Label { text, style, .. } => Some((text, style.colour)),
            _ => None,
        })
        .collect();
    let typed: String = labels
        .iter()
        .filter(|(t, _)| t.starts_with(|c: char| "QRSTUVWXYZ".contains(c)) && !t.contains(" / "))
        .map(|(t, _)| t.as_str())
        .collect();
    assert_eq!(typed.chars().count(), NOTICE_MAX, "80 characters drawn");
    assert!(
        labels.iter().any(|(t, c)| t.starts_with('Q') && c[3] != 0),
        "the typed text is visible (not a transparent colour)"
    );
    assert!(
        labels
            .iter()
            .any(|(t, _)| **t == format!("{NOTICE_MAX} / {NOTICE_MAX}")),
        "counter: {:?}",
        labels.iter().map(|l| l.0).collect::<Vec<_>>()
    );
}

#[test]
fn rom_member_notice_button_is_read_only() {
    let Some(mut m) = menu(1) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.input(MenuInput::Command(CMD_NOTICE));
    assert!(!shown(&m), "no edit window for a member");
    assert!(m.popup().is_some(), "the refusal box");
}
