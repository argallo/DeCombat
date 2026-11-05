//! shop-pane: the shop screen's left pane (window 2) against the 2009 list items: the weapons and body rows draw
//! their row images, the item's slot image and a white name; the weapons tab draws the backpack row ("A").
//!
//! ROM-gated (skipped without `CA_ROM`).
// EVIDENCE: cshell+0x1425d0 (row images), cshell+0xda610 (item image), cshell+0xda950 (row text)

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{InvRecord, LinkOptions};
use dc_client::menu::pane;
use dc_client::menu::storage::{own_slot, shelf_of, Wares, TAB_WEAPONS};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::steps::{resolve, Resolved};

fn menu() -> Option<Menu> {
    menu_at(1024, 768)
}

fn menu_at(width: u32, height: u32) -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
    Some(Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width,
        height,
    }))
}

fn click(m: &mut Menu, name: &str) {
    let Resolved::Part { at, enabled, .. } = resolve(m, name) else {
        panic!("{name}: {:?}", resolve(m, name));
    };
    assert!(enabled, "{name} disabled");
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
}

fn images(items: &[DrawItem]) -> Vec<(String, [f32; 4])> {
    items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Image { tex, dst, .. } => Some((tex.to_ascii_lowercase(), *dst)),
            _ => None,
        })
        .collect()
}

#[test]
fn rom_weapon_rows_draw_their_slot_images_a_backpack_row_and_white_names() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let primary = (0..1000u32)
        .find(|n| {
            wares.kind_of(*n as i32).and_then(shelf_of) == Some((TAB_WEAPONS, 0))
                && own_slot(&wares, *n) == Some(0)
                && wares.art_of(*n as i32).is_some()
        })
        .expect("no primary weapon with a slot image");
    m.data.store.items = vec![InvRecord {
        uid: 7,
        item_no: primary,
        slot_kind: 1,
        ..Default::default()
    }];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = primary as i32;
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let items = m.items();
    let imgs = images(&items);
    // the four row images, one under the other, 0xc2 x 0x47 at the list's x
    let mut last_y = f32::MIN;
    let mut row_x = None;
    for t in pane::WEAPON_ROW_IMAGES {
        let (_, d) = imgs
            .iter()
            .find(|(n, _)| *n == t.to_ascii_lowercase())
            .unwrap_or_else(|| panic!("row image {t} not drawn"));
        assert_eq!([d[2], d[3]], [0xc2 as f32, 0x47 as f32], "{t}");
        assert!(d[1] > last_y);
        last_y = d[1];
        row_x = Some(d[0]);
    }
    let row_x = row_x.unwrap();
    // the backpack row below them
    let (_, bp) = imgs
        .iter()
        .find(|(n, _)| *n == pane::BACKPACK_ROW_IMAGE.to_ascii_lowercase())
        .expect("backpack row not drawn");
    assert!(bp[1] > last_y);
    // the item's slot image 0x3e px left of the first row
    let icon = wares
        .art_of(primary as i32)
        .unwrap()
        .icon
        .to_ascii_lowercase();
    let (_, id) = imgs
        .iter()
        .find(|(n, _)| *n == icon)
        .expect("equipped item's image not drawn");
    assert_eq!(id[0], row_x - 0x3e as f32);
    // the equipped name in white, font 0x15
    let name = wares.label_of(primary as i32);
    let style = items
        .iter()
        .find_map(|i| match i {
            DrawItem::Text {
                text, style, pos, ..
            }
            | DrawItem::Label {
                text, style, pos, ..
            } if *text == name && pos[0] < 470.0 => Some(*style),
            _ => None,
        })
        .expect("equipped name not drawn in the pane");
    assert_eq!(style.colour, [255, 255, 255, 255]);
    assert_eq!(style.font, 0x15);
    // a backpack with extra slots: one lettered row per slot instead of the default row
    let bp = (7000..8000)
        .find(|n| wares.backpack_slots(*n) >= 1)
        .expect("no backpack with slots");
    m.data.store.equipped[pane::PACK_SLOT_NO] = bp;
    let imgs = images(&m.items());
    let n = (wares.backpack_slots(bp) % 5) as usize;
    for t in &pane::BACKPACK_SLOT_IMAGES[..n] {
        assert!(
            imgs.iter().any(|(x, _)| *x == t.to_ascii_lowercase()),
            "{t}"
        );
    }
    assert!(!imgs
        .iter()
        .any(|(x, _)| *x == pane::BACKPACK_ROW_IMAGE.to_ascii_lowercase()));
}

#[test]
fn rom_body_rows_draw_the_item_slot_image() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.data.store.equipped = [-1; 15];
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    click(&mut m, "shop.body");
    let imgs = images(&m.items());
    let rows = imgs
        .iter()
        .filter(|(n, _)| *n == pane::BODY_ROW_IMAGE.to_ascii_lowercase())
        .count();
    assert_eq!(rows, 6);
    assert!(!imgs
        .iter()
        .any(|(n, _)| *n == pane::BACKPACK_ROW_IMAGE.to_ascii_lowercase()));
}

#[test]
fn rom_black_market_tab_has_its_window_title_and_the_function_row() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.data.store.equipped = [-1; 15];
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let has = |imgs: &[(String, [f32; 4])], t: &str| {
        imgs.iter().any(|(n, _)| *n == t.to_ascii_lowercase())
    };
    let imgs = images(&m.items());
    assert!(has(&imgs, pane::SHOP_WINDOW_IMAGE));
    assert!(!has(&imgs, pane::MARKET_WINDOW_IMAGE));
    click(&mut m, "shop.market");
    let items = m.items();
    let imgs = images(&items);
    assert!(
        has(&imgs, pane::MARKET_WINDOW_IMAGE),
        "black market window not drawn"
    );
    assert!(!has(&imgs, pane::SHOP_WINDOW_IMAGE));
    // the empty function list: one row named by string 0x17cc
    let name = m.cat.text(pane::MARKET_EMPTY_TEXT);
    assert!(!name.is_empty());
    assert!(items.iter().any(|i| matches!(i,
        DrawItem::Text { text, pos, .. } | DrawItem::Label { text, pos, .. } if *text == name && pos[0] < 470.0)));
    assert_eq!(
        imgs.iter()
            .filter(|(n, d)| *n == pane::BODY_ROW_IMAGE.to_ascii_lowercase() && d[0] < 470.0)
            .count(),
        1
    );
    // back on the weapons tab the shop window returns
    click(&mut m, "shop.weapons");
    assert!(has(&images(&m.items()), pane::SHOP_WINDOW_IMAGE));
}

/// CLI-741 (amended; OWNER REPORT 2026-11-04 "activating all of them causes them to clip off screen"): twelve active
/// function items on the black market tab: the list shows the rows that fit its record (six), its scroll bar is up,
/// and nothing of the list is drawn outside the list record's rows or the pane, at 1024 x 768 and 2560 x 1440; the
/// wheel brings the later rows in, still inside.
// EVIDENCE: cshell+0x1496e0 (bar on from 7 records), cshell+0x2292e0 (the function list)
#[test]
fn rom_market_list_of_twelve_active_items_scrolls_inside_its_record() {
    for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
        let Some(mut m) = menu_at(w, h) else {
            eprintln!("skipped: no CA_ROM");
            return;
        };
        let wares = Wares::cached(m.cat.rom.as_deref());
        // function items that stay in the function list while in use (no IsCanUnEquip)
        let active: Vec<u32> = (9000..10000u32)
            .filter(|n| wares.kind_of(*n as i32) == Some(0x15) && wares.art_of(*n as i32).is_some())
            .filter(|n| !wares.can_unequip(*n as i32))
            .take(12)
            .collect();
        assert_eq!(
            active.len(),
            12,
            "the ROM has fewer than 12 such function items"
        );
        m.data.store.items = active
            .iter()
            .enumerate()
            .map(|(k, n)| InvRecord {
                uid: 100 + k as u32,
                item_no: *n,
                in_use: true,
                ..Default::default()
            })
            .collect();
        m.data.store.equipped = [-1; 15];
        m.open(Go::Lobby);
        click(&mut m, "lobby.shop");
        click(&mut m, "shop.market");
        let list = m
            .sheet()
            .parts
            .iter()
            .find(|p| {
                p.layer == 2 && p.id == pane::MARKET_LIST && p.kind == dc_ui_data::WidgetKind::List
            })
            .expect("market list record")
            .clone();
        let window = m
            .sheet()
            .parts
            .iter()
            .find(|p| p.layer == 2 && p.kind == dc_ui_data::WidgetKind::Window)
            .expect("pane window")
            .clone();
        let dc_client::menu::sheet::Look::List { height, .. } = list.look else {
            panic!("not a list");
        };
        let (top, bottom) = (list.pos[1] as f32, (list.pos[1] + height) as f32);
        let (left, right) = (
            window.pos[0] as f32,
            (window.pos[0] + window.size[0]) as f32,
        );
        let labels: Vec<String> = active.iter().map(|n| wares.label_of(*n as i32)).collect();
        let icons: Vec<String> = active
            .iter()
            .filter_map(|n| wares.art_of(*n as i32).map(|a| a.icon.to_ascii_lowercase()))
            .collect();
        let row_tex = pane::BODY_ROW_IMAGE.to_ascii_lowercase();
        let check = |m: &mut Menu, step: &str| -> Vec<String> {
            let items = m.items();
            let mut rows = 0;
            for (t, d) in images(&items) {
                if d[0] >= 470.0 || !(t == row_tex || icons.contains(&t)) {
                    continue;
                }
                rows += (t == row_tex) as usize;
                assert!(
                    d[1] >= top && d[1] + d[3] <= bottom && d[0] >= left && d[0] + d[2] <= right,
                    "{w}x{h} {step}: {t} at {d:?} outside the list ({left}..{right} x {top}..{bottom})"
                );
            }
            assert_eq!(rows, 6, "{w}x{h} {step}: rows shown");
            let mut shown = Vec::new();
            for i in &items {
                if let DrawItem::Text { text, pos, .. } | DrawItem::Label { text, pos, .. } = i {
                    if pos[0] < 470.0 && labels.contains(text) {
                        assert!(
                            pos[1] >= top && pos[1] < bottom,
                            "{w}x{h} {step}: {text} at {pos:?} outside the list"
                        );
                        shown.push(text.clone());
                    }
                }
            }
            shown
        };
        let first = check(&mut m, "top");
        let i = m
            .sheet()
            .parts
            .iter()
            .position(|p| {
                p.layer == 2 && p.id == pane::MARKET_LIST && p.kind == dc_ui_data::WidgetKind::List
            })
            .unwrap();
        let bar = m
            .sheet()
            .list_bar(i)
            .expect("the list's scroll bar")
            .clone();
        assert!(bar.top() > 0, "{w}x{h}: the bar scrolls");
        assert!(
            (bar.rect[1] as f32) >= top && ((bar.rect[1] + bar.rect[3]) as f32) <= bottom,
            "{w}x{h}: bar {:?}",
            bar.rect
        );
        // the wheel over the list moves the rows: the end of the list comes in
        let over = [(list.pos[0] + 100) as f32, (list.pos[1] + 100) as f32];
        m.input(MenuInput::Wheel(over, -20));
        let last = check(&mut m, "end");
        assert_ne!(first, last, "{w}x{h}: the wheel scrolled nothing");
        let order = dc_client::menu::pane::market_records(&wares, &m.data.store);
        let last_label = wares.label_of(order.last().unwrap().item_no as i32);
        assert!(
            last.contains(&last_label),
            "{w}x{h}: the last row is not shown after scrolling"
        );
    }
}
