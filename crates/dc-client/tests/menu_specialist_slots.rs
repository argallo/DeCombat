//! CLI-772 amendment (owner report 2026-11-08: "there are no special slots displayed"): a worn specialist turns the
//! weapons tab's backpack rows into its special slots S1.. (one from the specialist's row, one more with a specialist
//! slot license in the function list); a special item (`EquipOnlyBackpack`) is equipped into a special slot through
//! window 6, never into its own (primary) slot.
//!
//! ROM-gated (skipped without `CA_ROM`).
// EVIDENCE: cshell+0x227240 (the slot count and kind), cshell+0x1425d0 (the Special_Slot rows), cshell+0x13fed0 (the
// rows "S1 <0x399>"), cshell+0x148020 (the special-item check, boxes 0x189e / 0x189f)

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{InvRecord, LinkOptions};
use dc_client::menu::shop::{grid_cells, CMD_WEAR, LEFT_WINDOW, STORAGE_GRID, STORAGE_WINDOW};
use dc_client::menu::slotpick::{CMD_LIST, CMD_OK, PACK_WINDOW};
use dc_client::menu::storage::{own_slot, shelf_of, Wares, TAB_WEAPONS};
use dc_client::menu::{pages, pane, Go, Menu, MenuInput, MenuKey, MenuOptions};
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

fn tap(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

fn mid(c: [i32; 4]) -> [f32; 2] {
    [(c[0] + c[2] / 2) as f32, (c[1] + c[3] / 2) as f32]
}

fn button_mid(m: &Menu, cmd: i32) -> [f32; 2] {
    let b = m
        .sheet()
        .button(cmd)
        .filter(|b| b.shown)
        .unwrap_or_else(|| panic!("no shown button {cmd}"));
    mid([b.pos[0], b.pos[1], b.size[0], b.size[1]])
}

fn window_shown(m: &Menu, id: i32) -> bool {
    m.sheet()
        .parts
        .iter()
        .position(|p| p.kind == WidgetKind::Window && p.layer == id)
        .is_some_and(|i| m.sheet().visible(i))
}

fn images(m: &mut Menu) -> Vec<String> {
    m.items()
        .iter()
        .filter_map(|i| match i {
            DrawItem::Image { tex, .. } => Some(tex.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

fn rec(uid: u32, item_no: u32) -> InvRecord {
    InvRecord {
        uid,
        item_no,
        end: 1,
        ..Default::default()
    }
}

/// The ROM's items: a primary gun, a special item, a specialist and a specialist slot license.
struct Kit {
    primary: u32,
    gear: u32,
    specialist: u32,
    licence: u32,
}

fn kit(m: &Menu) -> Kit {
    let w = Wares::cached(m.cat.rom.as_deref());
    let primary = (0..1000u32)
        .find(|n| {
            w.kind_of(*n as i32).and_then(shelf_of) == Some((TAB_WEAPONS, 0))
                && own_slot(&w, *n) == Some(0)
        })
        .expect("a primary gun");
    let gear = (0..1000u32)
        .find(|n| w.pack_only(*n as i32))
        .expect("a special item (EquipOnlyBackpack)");
    let specialist = (10000..11000u32)
        .find(|n| w.art_of(*n as i32).and_then(|a| a.row_type) == Some(30))
        .expect("a specialist row (ItemType 30)");
    let licence = (9000..10000u32)
        .find(|n| {
            w.art_of(*n as i32)
                .is_some_and(|a| a.sub_type == dc_bute::SUB_SLOT_LICENSE)
        })
        .expect("a specialist slot license (SubType 18)");
    Kit {
        primary,
        gear,
        specialist,
        licence,
    }
}

/// The character's items: the primary equipped, the special item, optionally the worn specialist and the license in
/// its function slot (SubType 18: function slot 4).
fn equip(m: &mut Menu, k: &Kit, specialist: bool, licence: bool) {
    let mut items = vec![rec(7, k.primary), rec(20, k.gear), rec(21, k.specialist)];
    let mut slots = vec![-1; 7];
    if licence {
        let mut l = rec(22, k.licence);
        l.in_use = true;
        items.push(l);
        slots[4] = k.licence as i32;
    }
    m.data.store.items = items;
    m.data.store.function_slots = slots;
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = k.primary as i32;
    if specialist {
        m.data.store.equipped[14] = k.specialist as i32;
    }
}

/// The weapons tab's backpack rows: a specialist shows S1 (the arrow, no drawer), the license adds S2; without one the
/// default backpack row.
#[test]
fn rom_a_specialist_shows_special_slot_rows_and_the_licence_adds_one() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let k = kit(&m);
    let low = |t: &str| t.to_ascii_lowercase();
    equip(&mut m, &k, false, false);
    m.open(Go::Shop);
    let imgs = images(&mut m);
    assert!(imgs.contains(&low(pane::BACKPACK_ROW_IMAGE)), "no specialist: the default row");
    assert!(!imgs.contains(&low(pane::SPECIAL_SLOT_IMAGES[0])));
    equip(&mut m, &k, true, false);
    let imgs = images(&mut m);
    assert!(imgs.contains(&low(pane::SPECIAL_SLOT_IMAGES[0])), "S1 drawn");
    assert!(!imgs.contains(&low(pane::SPECIAL_SLOT_IMAGES[1])), "one slot without a license");
    assert!(imgs.contains(&low(pane::SPECIAL_ARROW)));
    assert!(!imgs.contains(&low(pane::SLIDE_ARROW)), "no backpack drawer");
    assert!(!imgs.contains(&low(pane::BACKPACK_ROW_IMAGE)));
    equip(&mut m, &k, true, true);
    let imgs = images(&mut m);
    assert!(imgs.contains(&low(pane::SPECIAL_SLOT_IMAGES[0])));
    assert!(imgs.contains(&low(pane::SPECIAL_SLOT_IMAGES[1])), "the license adds S2");
    assert!(!imgs.contains(&low(pane::SPECIAL_SLOT_IMAGES[2])));
}

/// The black market storage page (through its function list's first row) with the special item's cell selected.
fn market_storage(m: &mut Menu) {
    m.open(Go::Market);
    let l = m
        .sheet()
        .part(LEFT_WINDOW, pane::MARKET_LIST, WidgetKind::List)
        .expect("function list")
        .pos;
    let r = [
        l[0] + pane::MARKET_ROW_DX,
        l[1],
        pane::ROW_SIZE[0],
        pane::ROW_SIZE[1],
    ];
    tap(m, mid(r));
    assert!(window_shown(m, STORAGE_WINDOW), "the storage page is up");
    // the market's records in inventory order: the special item is the first cell
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    tap(m, mid(cells[0]));
}

/// Equip on the special item: without a specialist the 0x189e box and no request; with one, window 6 lists S1 (and S2
/// with the license) and OK equips into slot 10 + the row, never the primary slot.
#[test]
fn rom_a_special_item_equips_into_a_special_slot_never_the_primary_slot() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let k = kit(&m);
    equip(&mut m, &k, false, false);
    market_storage(&mut m);
    let sent = m.link_sent();
    m.input(MenuInput::Command(CMD_WEAR));
    let q = m.popup().cloned().expect("the special-item box");
    assert_eq!((q.text, q.title), (m.cat.text(0x189e), 0x371));
    assert_eq!(m.link_sent(), sent, "nothing sent: no primary-slot equip");
    m.input(MenuInput::Key(MenuKey::Enter));
    for (licence, rows) in [(false, 1usize), (true, 2)] {
        equip(&mut m, &k, true, licence);
        market_storage(&mut m);
        m.input(MenuInput::Command(CMD_WEAR));
        assert_eq!(m.popup_count(), 0);
        assert!(window_shown(&m, PACK_WINDOW), "window 6 is up");
        let arrow = button_mid(&m, CMD_LIST);
        tap(&mut m, arrow);
        let got: Vec<String> = m.dropdown().expect("slot list").items.clone();
        let word = m.cat.text(0x399);
        let want: Vec<String> = ["S1", "S2"][..rows]
            .iter()
            .map(|l| format!("{l} {word}"))
            .collect();
        assert_eq!(got, want);
        tap(&mut m, arrow);
        let ok = button_mid(&m, CMD_OK);
        tap(&mut m, ok);
        assert_eq!(m.link_latest_request(), format!("Equip(20, {}, 10)", k.gear));
        assert!(!window_shown(&m, PACK_WINDOW));
    }
}
