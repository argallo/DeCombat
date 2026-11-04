//! shop-r11a (playtest report): the shop's left pane rows answer the pointer as the 2009 list items do: the row under
//! the pointer shows its _On image (CLI-971); a backpack slot row slides out a drawer with Primary / Secondary /
//! Throw buttons, a press on one opens that storage page for the row and Equip there fills the backpack slot
//! (CLI-972).
//!
//! ROM-gated (skipped without `CA_ROM`).
// EVIDENCE: cshell+0x1425d0, cshell+0x1d9ad0, cshell+0x1da230, cshell+0x1daa10, cshell+0x1d94f0
// DISASM: cshell+0x1509ba (10000 .. 10002), cshell+0x140590 (storage buttons), cshell+0x14fa39 (Equip into +0x55a8)

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{InvRecord, LinkOptions};
use dc_client::menu::pane;
use dc_client::menu::shop::{grid_cells, STORAGE_GRID, STORAGE_WINDOW};
use dc_client::menu::slotpick;
use dc_client::menu::storage::{own_slot, shelf_of, Wares, TAB_WEAPONS};
use dc_client::menu::{pages, Go, Menu, MenuInput, MenuOptions};
use dc_client::steps::{resolve, Resolved};
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

fn click(m: &mut Menu, name: &str) {
    let Resolved::Part { at, .. } = resolve(m, name) else {
        panic!("{name}: {:?}", resolve(m, name));
    };
    tap(m, at);
}

fn tap(m: &mut Menu, at: [f32; 2]) {
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
}

fn mid(r: [i32; 4]) -> [f32; 2] {
    [
        r[0] as f32 + r[2] as f32 / 2.0,
        r[1] as f32 + r[3] as f32 / 2.0,
    ]
}

fn drawn(items: &[DrawItem], tex: &str) -> bool {
    items.iter().any(|i| match i {
        DrawItem::Image { tex: t, .. } => t.eq_ignore_ascii_case(tex),
        _ => false,
    })
}

fn primaries(wares: &Wares) -> Vec<u32> {
    (0..1000u32)
        .filter(|n| {
            wares.kind_of(*n as i32).and_then(shelf_of) == Some((TAB_WEAPONS, 0))
                && own_slot(wares, *n) == Some(0)
                && wares.art_of(*n as i32).is_some()
        })
        .collect()
}

/// CLI-971: the equipment row under the pointer draws its _On image, the others their _Off image.
#[test]
fn rom_the_row_under_the_pointer_shows_its_on_image() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let items = m.items();
    assert!(drawn(&items, pane::WEAPON_ROW_IMAGES[1]));
    assert!(!drawn(&items, pane::WEAPON_ROW_ON_IMAGES[1]));
    let list = m
        .sheet()
        .part(2, 2, WidgetKind::List)
        .expect("weapons list")
        .pos;
    // the second row
    let at = [
        list[0] as f32 + 50.0,
        list[1] as f32 + 0x47 as f32 + 3.0 + 20.0,
    ];
    m.input(MenuInput::Move(at));
    let items = m.items();
    assert!(drawn(&items, pane::WEAPON_ROW_ON_IMAGES[1]), "row 2 lit");
    assert!(!drawn(&items, pane::WEAPON_ROW_IMAGES[1]));
    assert!(drawn(&items, pane::WEAPON_ROW_IMAGES[0]), "row 1 not lit");
    m.input(MenuInput::Move([700.0, 700.0]));
    assert!(drawn(&m.items(), pane::WEAPON_ROW_IMAGES[1]), "off again");
}

/// CLI-972: a backpack slot row slides its drawer out; Primary opens the primary storage page for that row (Mod at
/// Backpack's place, Backpack hidden) and Equip puts the item into backpack slot 10 + row.
#[test]
fn rom_a_backpack_row_drawer_opens_the_storage_page_that_equips_into_the_backpack() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let guns = primaries(&wares);
    let (worn, spare) = (guns[0], guns[1]);
    let bp = (7000..8000)
        .find(|n| wares.backpack_slots(*n) >= 1)
        .expect("no backpack with slots");
    m.data.store.items = vec![InvRecord {
        uid: 8,
        item_no: spare,
        slot_kind: 1,
        ..Default::default()
    }];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = worn as i32;
    m.data.store.equipped[pane::PACK_SLOT_NO] = bp;
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let t0 = m.secs() + 1.0;
    let items = m.items_at(t0);
    assert!(
        drawn(&items, pane::SLIDE_ARROW),
        "a closed row shows its arrow"
    );
    let at = m
        .sheet()
        .part(2, pane::BACKPACK_LIST, WidgetKind::List)
        .expect("backpack list")
        .pos;
    let row = pane::pack_row_rect(at, 0);
    m.input(MenuInput::Move(mid(row)));
    let items = m.items_at(t0);
    assert!(drawn(&items, pane::BACKPACK_SLOT_ON_IMAGES[0]), "row A lit");
    // the slide runs: after the 50 ms wait 12 px a step, open after 0xa0 / 12 steps
    let items = m.items_at(t0 + 0.1);
    assert!(
        items.iter().any(|i| matches!(i, DrawItem::Image { tex, .. }
            if tex.starts_with(pane::SLIDE_IMAGE) && tex.contains('#'))),
        "the drawer slides"
    );
    let items = m.items_at(t0 + 0.5);
    let primary_a = format!("{}a.dtx", pane::SLIDE_BUTTONS[0].0);
    assert!(drawn(&items, &primary_a), "open drawer shows Primary");
    // the pointer on the Primary button keeps the drawer open and lights it
    let b = pane::drawer_button_rect(row, 0);
    m.input(MenuInput::Move(mid(b)));
    let items = m.items_at(t0 + 0.6);
    assert!(drawn(&items, &format!("{}b.dtx", pane::SLIDE_BUTTONS[0].0)));
    tap(&mut m, mid(b));
    let _ = m.items_at(t0 + 0.7);
    let s = m.sheet();
    let storage = s.part(4, 4, WidgetKind::Window).expect("storage window");
    assert!(storage.shown, "the storage page is up");
    // cshell+0x140590 with a drawer row on a primary page: Discard at Mod's place, Mod at Backpack's, Backpack hidden
    let backpack = s.part(4, 2, WidgetKind::Button).unwrap();
    assert!(!backpack.shown, "Backpack hidden");
    let modify = s.part(4, 1, WidgetKind::Button).unwrap();
    assert_eq!(modify.pos, [812, 430]);
    assert_eq!(s.part(4, 0, WidgetKind::Button).unwrap().pos, [713, 430]);
    // Equip (265) on the spare primary fills backpack slot 10 (row A)
    let equip = s.button(265).expect("Equip");
    let e = mid([equip.pos[0], equip.pos[1], equip.size[0], equip.size[1]]);
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    tap(&mut m, mid(cells[0]));
    tap(&mut m, e);
    assert_eq!(m.link_latest_request(), format!("Equip(8, {spare}, 10)"));
}

/// CLI-972: without a drawer row the storage page's buttons stay at their places (primary page) or drop Mod
/// (melee and support pages).
#[test]
fn rom_storage_buttons_follow_the_category() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let list = m.sheet().part(2, 2, WidgetKind::List).unwrap().pos;
    // the first row opens the primary storage page
    tap(&mut m, [list[0] as f32 + 50.0, list[1] as f32 + 20.0]);
    let s = m.sheet();
    assert!(s.part(4, 1, WidgetKind::Button).unwrap().shown);
    assert!(s.part(4, 2, WidgetKind::Button).unwrap().shown);
    assert_eq!(s.part(4, 0, WidgetKind::Button).unwrap().pos, [614, 430]);
    // the third row (melee): Mod hidden, Discard at its place
    tap(
        &mut m,
        [
            list[0] as f32 + 50.0,
            list[1] as f32 + 2.0 * 0x4a as f32 + 20.0,
        ],
    );
    let s = m.sheet();
    assert!(!s.part(4, 1, WidgetKind::Button).unwrap().shown);
    assert_eq!(s.part(4, 0, WidgetKind::Button).unwrap().pos, [713, 430]);
}

/// CLI-974 (playtest report): a knife cannot go into the backpack: no backpack check pass for a melee weapon, no
/// Backpack button on the melee storage page.
#[test]
fn rom_the_backpack_takes_no_melee_weapon() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let knives: Vec<u32> = (0..1000u32)
        .filter(|n| own_slot(&wares, *n) == Some(2))
        .collect();
    let guns = primaries(&wares);
    let bp = (7000..8000)
        .find(|n| wares.backpack_slots(*n) >= 1)
        .expect("no backpack with slots");
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = guns[0] as i32;
    m.data.store.equipped[2] = knives[0] as i32;
    m.data.store.equipped[pane::PACK_SLOT_NO] = bp;
    assert_eq!(
        slotpick::pack_check(&m.data.store, &wares, Some(knives[1])),
        Err(None),
        "a knife is refused"
    );
    assert_eq!(
        slotpick::pack_check(&m.data.store, &wares, Some(guns[1])),
        Ok(())
    );
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let list = m.sheet().part(2, 2, WidgetKind::List).unwrap().pos;
    tap(
        &mut m,
        [
            list[0] as f32 + 50.0,
            list[1] as f32 + 2.0 * 0x4a as f32 + 20.0,
        ],
    );
    assert!(
        !m.sheet().part(4, 2, WidgetKind::Button).unwrap().shown,
        "no Backpack on the melee page"
    );
    tap(
        &mut m,
        [
            list[0] as f32 + 50.0,
            list[1] as f32 + 3.0 * 0x4a as f32 + 20.0,
        ],
    );
    assert!(
        m.sheet().part(4, 2, WidgetKind::Button).unwrap().shown,
        "Backpack on the support page"
    );
}

/// CLI-973: a gun in a backpack slot is drawn as the 2009 code sizes it: the slot image's 0xfe x 0x47 sprite scaled
/// by 0.9, 0x48 left of and 0xb above the row (cshell+0x1da7f0).
#[test]
fn rom_a_backpack_gun_is_drawn_at_the_2009_scale() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let guns = primaries(&wares);
    let bp = (7000..8000)
        .find(|n| wares.backpack_slots(*n) >= 1)
        .expect("no backpack with slots");
    m.data.store.items = vec![InvRecord {
        uid: 9,
        item_no: guns[1],
        slot_kind: 1,
        ..Default::default()
    }];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = guns[0] as i32;
    m.data.store.equipped[pane::PACK_SLOT_NO] = bp;
    m.data.store.equipped[pane::FIRST_PACK_SLOT] = guns[1] as i32;
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let icon = wares.art_of(guns[1] as i32).unwrap().icon.clone();
    let at = m
        .sheet()
        .part(2, pane::BACKPACK_LIST, WidgetKind::List)
        .unwrap()
        .pos;
    let items = m.items();
    let (src, dst) = items
        .iter()
        .find_map(|i| match i {
            DrawItem::Image { tex, src, dst, .. }
                if tex.eq_ignore_ascii_case(&icon) && dst[1] > at[1] as f32 - 20.0 =>
            {
                Some((*src, *dst))
            }
            _ => None,
        })
        .expect("the backpack gun is drawn");
    assert_eq!(src, [254.0, 71.0]);
    // CLI-1091: scaled about the sprite's centre (12.7 / 3.55 px in from the sprite's corner)
    assert!(
        (dst[0] - ((at[0] - 0x48) as f32 + 12.7)).abs() < 0.01,
        "{dst:?}"
    );
    assert!(
        (dst[1] - ((at[1] - 0xb) as f32 + 3.55)).abs() < 0.01,
        "{dst:?}"
    );
    assert!(
        (dst[2] - 254.0 * 0.9).abs() < 0.01 && (dst[3] - 71.0 * 0.9).abs() < 0.01,
        "{dst:?}"
    );
}

/// CLI-1090 (playtest report): the pointer moving from the row onto the drawer crosses the arrow, the 2 px before the
/// first button and the slide while it still moves; the drawer stays out and a press on a button lands.
#[test]
fn rom_the_drawer_stays_out_while_the_pointer_crosses_onto_its_buttons() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let bp = (7000..8000)
        .find(|n| wares.backpack_slots(*n) >= 1)
        .expect("no backpack with slots");
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[pane::PACK_SLOT_NO] = bp;
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let t0 = m.secs() + 1.0;
    let _ = m.items_at(t0);
    let at = m
        .sheet()
        .part(2, pane::BACKPACK_LIST, WidgetKind::List)
        .expect("backpack list")
        .pos;
    let row = pane::pack_row_rect(at, 0);
    let y = row[1] as f32 + 20.0;
    m.input(MenuInput::Move(mid(row)));
    let _ = m.items_at(t0 + 0.1);
    // still sliding: the pointer on the arrow, the gap and the slide keeps it coming out
    for x in [0x9f + 2, 0xa0, 0xa1 + 30] {
        m.input(MenuInput::Move([(row[0] + x) as f32 + 0.5, y]));
        let _ = m.items_at(t0 + 0.12);
    }
    // the pointer stayed on the slide, over the Primary button: open, Primary lit
    let primary_b = format!("{}b.dtx", pane::SLIDE_BUTTONS[0].0);
    assert!(
        drawn(&m.items_at(t0 + 0.6), &primary_b),
        "the drawer opened"
    );
    // open: back to the row, then across the gap onto the Secondary button
    m.input(MenuInput::Move(mid(row)));
    for x in [0x9f, 0xa0, 0xa1 + 10, 0xd4 + 10] {
        m.input(MenuInput::Move([(row[0] + x) as f32 + 0.5, y]));
        let _ = m.items_at(t0 + 0.65);
    }
    let items = m.items_at(t0 + 0.7);
    assert!(
        drawn(&items, &format!("{}b.dtx", pane::SLIDE_BUTTONS[1].0)),
        "Secondary lit, the drawer still out"
    );
    tap(&mut m, mid(pane::drawer_button_rect(row, 1)));
    let _ = m.items_at(t0 + 0.8);
    let storage = m
        .sheet()
        .part(4, 4, WidgetKind::Window)
        .expect("storage window");
    assert!(storage.shown, "the press opened the storage page");
}
