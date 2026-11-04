//! storage-r11 (playtest report): the body tab's equipped default items draw a slot picture on their left-pane rows
//! (CLI-1004), and every body row opens its own storage page: Character the characters' page, the others their
//! category (CLI-1003).
//!
//! ROM-gated (skipped without `CA_ROM`).
// EVIDENCE: cshell+0x149d20 (body list rows: slot 0xe type 0x16, slots 4 / 6 / 7 / 8 / 9 of types 0xb / 0xc / 0xe / 0xf
// / 0x10), cshell+0x14a700 (page of tab 2 / category 3 under the Body tab)

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{InvRecord, LinkOptions};
use dc_client::menu::storage::{shelf_of, Wares, TAB_BODY};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::steps::{resolve, Resolved};

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

fn tap(m: &mut Menu, at: [f32; 2]) {
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
}

fn click(m: &mut Menu, name: &str) {
    let Resolved::Part { at, .. } = resolve(m, name) else {
        panic!("{name}: {:?}", resolve(m, name));
    };
    tap(m, at);
}

fn rec(uid: u32, item_no: u32) -> InvRecord {
    InvRecord {
        uid,
        item_no,
        slot_kind: 1,
        ..Default::default()
    }
}

fn labels(m: &mut Menu) -> Vec<String> {
    m.items()
        .iter()
        .filter_map(|i| match i {
            DrawItem::Label { text, .. } | DrawItem::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// CLI-1004: the default head has no `InventoryIcon` in the item table; its pane row draws the stand-in slot picture
/// all the same. CLI-1093: the default uniform row draws none (2009 and the reference client), no woodland stand-in.
#[test]
fn rom_equipped_default_body_items_draw_a_row_picture() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.data.store.items = vec![rec(1, 2000), rec(2, 3000), rec(3, 4000)];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[7] = 2000;
    m.data.store.equipped[8] = 3000;
    m.data.store.equipped[4] = 4000;
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    click(&mut m, "shop.body");
    let items = m.items().to_vec();
    for tex in ["Uniform\\WoodlandCamo_Slot", "Vest\\Normal_AR_Slot"] {
        assert!(
            !items
                .iter()
                .any(|i| matches!(i, DrawItem::Image { tex: t, .. }
                if t.to_lowercase().contains(&tex.to_lowercase()))),
            "{tex} not drawn"
        );
    }
    for tex in ["Head\\Helmet_Default_Slot"] {
        assert!(
            items
                .iter()
                .any(|i| matches!(i, DrawItem::Image { tex: t, .. }
                if t.to_lowercase().contains(&tex.to_lowercase()))),
            "{tex} drawn on its row"
        );
    }
}

/// CLI-1003: each body row opens the page of its own item type: Head, Face, Uniform, Vest and Backpack their body
/// categories 0 .. 4, Character the characters' page (not the Head page).
#[test]
fn rom_every_body_row_opens_its_own_storage_page() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    // one owned item of every body category (a table item of that shelf), and a character item
    let pick = |cat: usize| {
        (2000..8000u32)
            .find(|n| {
                shelf_of(wares.kind_of(*n as i32).unwrap_or(-1)) == Some((TAB_BODY, cat))
                    && wares.art_of(*n as i32).is_some()
            })
            .unwrap_or_else(|| panic!("no body item of category {cat}"))
    };
    let character = (10000..11000u32).find(|n| wares.is_person_item(*n as i32));
    let mut items: Vec<InvRecord> = (0..5).map(|c| rec(c as u32 + 1, pick(c))).collect();
    if let Some(c) = character {
        items.push(rec(20, c));
    }
    m.data.store.items = items.clone();
    m.data.store.equipped = [-1; 15];
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    click(&mut m, "shop.body");
    // body rows in list order: Character, Head, Face, Uniform, Vest, Backpack
    let l = m
        .sheet()
        .part(
            dc_client::menu::shop::LEFT_WINDOW,
            dc_client::menu::shop::BODY_LIST,
            dc_ui_data::WidgetKind::List,
        )
        .unwrap()
        .pos;
    let row = |i: i32| {
        [
            l[0] as f32 + 10.0,
            (l[1] + i * dc_client::menu::shop::ROW_PITCH) as f32 + 10.0,
        ]
    };
    for (i, want) in [(1, 0usize), (2, 1), (3, 2), (4, 3), (5, 4)] {
        click(&mut m, "shop.body");
        tap(&mut m, row(i));
        let name = wares.label_of(items[want].item_no as i32);
        let shown = labels(&mut m);
        assert!(
            shown.contains(&name),
            "row {i} opens category {want}: {name}"
        );
        for (k, it) in items.iter().enumerate().take(5) {
            if k != want {
                assert!(
                    !shown.contains(&wares.label_of(it.item_no as i32))
                        || wares.label_of(it.item_no as i32) == name,
                    "row {i} shows only its own category"
                );
            }
        }
    }
    // Character: the head item is not listed
    click(&mut m, "shop.body");
    tap(&mut m, row(0));
    let shown = labels(&mut m);
    let head = wares.label_of(items[0].item_no as i32);
    let own_label = |it: &InvRecord| wares.label_of(it.item_no as i32);
    assert!(
        !shown.contains(&head),
        "the Character row does not open the Head page"
    );
    if let Some(c) = character {
        assert!(
            shown.contains(&own_label(&rec(0, c))),
            "the Character page lists the character item"
        );
    }
}
