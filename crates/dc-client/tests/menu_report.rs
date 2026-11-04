//! inbox-clan: the report box opens on the category table's first entry: its general and detail names stand on the
//! two boxes (cshell+0x273b70 -> 0x186a5 / 0x186a6 with item 0), not the "Select ... Type" placeholders.
//!
//! ROM-gated (skipped without `CA_ROM`: the names are the binary's literals).

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::report::{self, LIST_TEXT_IDS, TARGET_ID, WINDOW};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions, CMD_BAR_ABUSE};
use dc_ui_data::WidgetKind;

#[test]
fn rom_report_opens_on_the_first_category_entry() {
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
    m.open(Go::Lobby);
    m.input(MenuInput::Command(CMD_BAR_ABUSE));
    assert_eq!(m.screen_name(), "report");
    let first = m.cat.paths.overlay.report_first_entry.clone();
    let placeholders = m.cat.paths.overlay.report_texts[1..3].to_vec();
    for (i, want) in first.iter().enumerate() {
        let want = want.clone().expect("the binary's first entry");
        let shown = m
            .sheet()
            .part(WINDOW, LIST_TEXT_IDS[i], WidgetKind::Text)
            .map(|p| p.text.clone())
            .unwrap_or_default();
        assert_eq!(shown, want, "box {i}");
        assert_ne!(Some(shown), placeholders[i]);
    }
    let drawn: Vec<String> = m
        .items()
        .iter()
        .filter_map(|i| match i {
            DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    for t in first.iter().flatten() {
        assert!(drawn.contains(t));
    }
}

/// report-box: the three category kinds, their detail lists and the target box that only some kinds ask for
/// (cshell+0x273080 table, cshell+0x272850 state, cshell+0x273d50 commands).
#[test]
fn rom_report_lists_hold_the_table_and_the_target_follows_the_kind() {
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
    m.open(Go::Lobby);
    m.input(MenuInput::Command(CMD_BAR_ABUSE));
    let general = m.cat.paths.overlay.report_general.clone();
    let detail = m.cat.paths.overlay.report_detail.clone();
    let target_shown = |m: &Menu| {
        m.sheet()
            .part(WINDOW, TARGET_ID, WidgetKind::EditBox)
            .is_some_and(|p| p.shown)
    };
    assert!(target_shown(&m), "harassment asks for a target");
    // the general list: one row per kind
    m.input(MenuInput::Command(report::CMD_GENERAL));
    let d = m.dropdown().expect("general list").clone();
    assert_eq!(
        d.items,
        general
            .iter()
            .map(|g| g.clone().unwrap())
            .collect::<Vec<_>>()
    );
    // the second kind (bugs): its first detail on the box, the target box hidden
    let p = d.row_point(1).unwrap();
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
    assert!(m.dropdown().is_none());
    let shown = |m: &Menu, i: usize| {
        m.sheet()
            .part(WINDOW, LIST_TEXT_IDS[i], WidgetKind::Text)
            .map(|p| p.text.clone())
            .unwrap_or_default()
    };
    assert_eq!(shown(&m, 0), general[1].clone().unwrap());
    assert_eq!(shown(&m, 1), detail[7].clone().unwrap());
    assert!(!target_shown(&m), "a bug report needs no target");
    // the detail list holds the six bug kinds
    m.input(MenuInput::Command(report::CMD_DETAIL));
    let d = m.dropdown().expect("detail list");
    assert_eq!(d.items.len(), 6);
    assert_eq!(d.items[5], detail[12].clone().unwrap());
    // the second press of the command hides the list again
    m.input(MenuInput::Command(report::CMD_DETAIL));
    assert!(m.dropdown().is_none());
}
