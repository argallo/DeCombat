//! lobby-leftovers11: the messenger window's friend tree as 2009 builds it: the top group row (open mark + name,
//! cshell+0xf0000 / 0xecb40), the tree's scroll bar (cshell+0xf0d90) and the disabled Invite Chat caption
//! (cshell+0xf1100). ROM-gated (skipped without `CA_ROM`).

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::messenger::{Messenger, GROUP_NAME, LIST_AT, ROW_NAME_AT, TREE_AT, WINDOW_AT};
use dc_client::menu::{Go, Menu, MenuOptions};

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
fn rom_messenger_tree_has_the_group_row_and_a_scroll_bar() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.open(Go::Lobby);
    m.toggle_messenger();
    assert!(m.messenger_open());
    let items = m.items();
    let tree = [
        WINDOW_AT[0] + LIST_AT[0] + TREE_AT[0],
        WINDOW_AT[1] + LIST_AT[1] + TREE_AT[1],
    ];
    // the group row: its name at the tree's first row, its open mark (Group_Close) left of it
    let name = items.iter().find_map(|i| match i {
        DrawItem::Label { text, pos, .. } if text == GROUP_NAME => Some(*pos),
        _ => None,
    });
    assert_eq!(
        name,
        Some([
            (tree[0] + ROW_NAME_AT[0]) as f32,
            (tree[1] + ROW_NAME_AT[1]) as f32
        ])
    );
    // the window is drawn over the screen: its items come last
    let tex = |part: &str| {
        items.iter().rev().find_map(|i| match i {
            DrawItem::Image { tex, dst, .. } if tex.contains(part) => Some(*dst),
            _ => None,
        })
    };
    let mark = tex("Group_Close").expect("open group mark drawn");
    assert!(mark[0] < (tree[0] + ROW_NAME_AT[0]) as f32 && mark[1] < (tree[1] + 4) as f32);
    // the scroll bar's arrows at the bar's place
    let bar = Messenger::bar_rect();
    let up = tex("Scroll_UP_A").expect("up arrow drawn");
    assert_eq!([up[0], up[1]], [(bar[0] + 1) as f32, bar[1] as f32]);
    assert!(tex("Scroll_DOWN_A").is_some());
    // Invite Chat is drawn in another colour than the enabled Add Friend
    let colour = |s: &str| {
        let want = m.cat.text(match s {
            "invite" => 0x589,
            _ => 0x13f7,
        });
        items.iter().find_map(|i| match i {
            DrawItem::Label { text, style, .. } if *text == want => Some(style.colour),
            _ => None,
        })
    };
    assert!(colour("invite").is_some());
    assert_ne!(colour("invite"), colour("add"));
}
