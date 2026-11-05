//! free-pass-tab (OWNER REPORT 2026-11-05 "the weapon free pass, there's no free pass tab on storage to benefit from
//! it"): the storage page's Free Pass tab (window 4 control 7, command 1038). While a Weapon Free Pass (function item
//! kind 16) is in use the tab shows on the shelf of its weapons; its page lists the pass's weapons (ROM
//! `FreePassItemID<n>`), Equip sends 0x35e {weapon, pass, slot}, Equip on the equipped one 0x360 (CLI-1888).
//!
//! ROM-gated (skipped without `CA_ROM`). Headless, detached link: the requests are read back from the link's last-sent
//! hook, the lobby's answers fed in.
// EVIDENCE: cshell+0x141060 (the tab), cshell+0x14ad00 (the page), cshell+0x147e40 (0x35e), cshell+0x145ed0 (0x360),
// cshell+0x229ca0 (0x323 +0x4e)

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{InvRecord, LinkEvent, LinkOptions};
use dc_client::menu::shop::{
    grid_cells, CMD_STORE_FREE_PASS, CMD_STORE_GENERAL, CMD_WEAR, FREE_PASS_TAB, GENERAL_TAB,
    LEFT_WINDOW, SPECIAL_TAB, STORAGE_GRID, STORAGE_WINDOW, WEAPON_LIST,
};
use dc_client::menu::storage::{
    own_slot, shelf_of, stored_free, Wares, SUB_FREE_PASS, TAB_WEAPONS,
};
use dc_client::menu::tuning::EQUIPPED_MARK;
use dc_client::menu::{pages, Go, Menu, MenuInput, MenuOptions};
use dc_ui_data::WidgetKind;

fn menu() -> Option<Menu> {
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
    Some(m)
}

fn tap(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

fn mid(c: [i32; 4]) -> [f32; 2] {
    [(c[0] + c[2] / 2) as f32, (c[1] + c[3] / 2) as f32]
}

fn inside(r: [i32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] as f32
        && p[1] >= r[1] as f32
        && p[0] < (r[0] + r[2]) as f32
        && p[1] < (r[1] + r[3]) as f32
}

fn button_mid(m: &Menu, cmd: i32) -> [f32; 2] {
    let b = m
        .sheet()
        .button(cmd)
        .filter(|b| b.shown)
        .unwrap_or_else(|| panic!("no shown button {cmd}"));
    mid([b.pos[0], b.pos[1], b.size[0], b.size[1]])
}

/// (shown, selected, place) of storage strip control `id`.
fn strip(m: &Menu, id: i32) -> (bool, bool, [i32; 2]) {
    let b = m
        .sheet()
        .part(STORAGE_WINDOW, id, WidgetKind::Button)
        .expect("strip button");
    (b.shown, b.selected, b.pos)
}

/// A Weapon Free Pass of the ROM whose weapons lie on the primary shelf: its item number and its weapons.
fn primary_pass(wares: &Wares) -> (u32, Vec<i32>) {
    (9000..10000)
        .filter_map(|n| {
            let a = wares.art_of(n)?;
            (a.sub_type == SUB_FREE_PASS
                && a.free_pass_items.len() > 6
                && a.free_pass_items
                    .iter()
                    .all(|w| wares.kind_of(*w).and_then(shelf_of) == Some((TAB_WEAPONS, 0))))
            .then(|| (n as u32, a.free_pass_items.clone()))
        })
        .next()
        .expect("a Weapon Free Pass of primaries in the item table")
}

/// The storage page of the primary slot, the given records held.
fn storage_page(m: &mut Menu, items: Vec<InvRecord>) {
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = items[0].item_no as i32;
    m.data.store.items = items;
    m.open(Go::Shop);
    let l = m
        .sheet()
        .part(LEFT_WINDOW, WEAPON_LIST, WidgetKind::List)
        .expect("weapon list")
        .pos;
    tap(m, [l[0] as f32 + 10.0, l[1] as f32 + 10.0]);
}

fn labels_in(m: &mut Menu, cell: [i32; 4]) -> Vec<String> {
    m.input(MenuInput::Move([2.0, 2.0]));
    m.items_at(500.0)
        .into_iter()
        .filter_map(|d| match d {
            DrawItem::Text { text, pos, .. } | DrawItem::Label { text, pos, .. }
                if inside(cell, pos) =>
            {
                Some(text)
            }
            _ => None,
        })
        .collect()
}

fn equipped_mark_in(m: &mut Menu, cell: [i32; 4]) -> bool {
    m.input(MenuInput::Move([2.0, 2.0]));
    m.items_at(500.0).iter().any(|d| {
        matches!(d, DrawItem::Image { tex, dst, .. }
            if tex == EQUIPPED_MARK.0 && inside(cell, [dst[0], dst[1]]))
    })
}

/// CLI-1888: without a pass in use the strip shows General alone; with one the Free Pass tab shows at the hidden
/// Special Item tab's place, its page lists the pass's weapons (six a page) instead of the owned items, Equip sends
/// 0x35e for the weapon's own slot with the pass's item number, the flagged slot of 0x323 marks the cell equipped and
/// Equip on it sends 0x360; General returns to the owned items, and the tab goes with the pass.
#[test]
fn rom_the_free_pass_tab_lists_the_pass_weapons_and_equips_them_for_free() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let (pass, weapons) = primary_pass(&wares);
    let owned = (0..1000u32)
        .find(|n| {
            wares.kind_of(*n as i32).and_then(shelf_of) == Some((TAB_WEAPONS, 0))
                && own_slot(&wares, *n) == Some(0)
                && !weapons.contains(&(*n as i32))
        })
        .unwrap();
    let gun = InvRecord {
        uid: 7,
        item_no: owned,
        slot_kind: 1,
        ..Default::default()
    };
    let mut ticket = InvRecord {
        uid: 8,
        item_no: pass,
        end: 1,
        ..Default::default()
    };
    // not in use: no tab
    storage_page(&mut m, vec![gun.clone(), ticket.clone()]);
    assert!(
        strip(&m, GENERAL_TAB).0 && strip(&m, GENERAL_TAB).1,
        "General"
    );
    assert!(
        !strip(&m, FREE_PASS_TAB).0,
        "no Free Pass tab without a pass in use"
    );
    // in use: the tab at the Special Item tab's place
    ticket.in_use = true;
    storage_page(&mut m, vec![gun.clone(), ticket.clone()]);
    let special = strip(&m, SPECIAL_TAB);
    assert!(!special.0, "Special Item stays hidden");
    assert_eq!(strip(&m, FREE_PASS_TAB), (true, false, special.2));
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    let gun_name = wares.label_of(owned as i32);
    assert!(
        labels_in(&mut m, cells[0]).contains(&gun_name),
        "the general page"
    );
    let at = button_mid(&m, CMD_STORE_FREE_PASS);
    tap(&mut m, at);
    assert!(strip(&m, FREE_PASS_TAB).1 && !strip(&m, GENERAL_TAB).1);
    let free = stored_free(&m.data.store, &wares, TAB_WEAPONS, 0);
    let mut listed: Vec<i32> = free.iter().map(|r| r.item_no as i32).collect();
    listed.sort_unstable();
    let mut want = weapons.clone();
    want.sort_unstable();
    want.dedup();
    assert_eq!(listed, want, "the page holds the pass's weapons");
    assert!(free.iter().all(|r| r.free_pass == pass));
    for (k, cell) in cells.iter().enumerate() {
        let name = wares.label_of(free[k].item_no as i32);
        assert!(labels_in(&mut m, *cell).contains(&name), "cell {k}: {name}");
    }
    assert!(!labels_in(&mut m, cells[0]).contains(&gun_name));
    // Equip on the second cell: 0x35e into the weapon's own slot
    tap(&mut m, mid(cells[1]));
    let w = free[1].item_no;
    let at = button_mid(&m, CMD_WEAR);
    tap(&mut m, at);
    assert_eq!(
        m.link_latest_request(),
        format!("FreePassEquip({w}, {pass}, 0)")
    );
    // the lobby's answer and the reread 0x323 with slot 0 flagged
    m.feed_event(LinkEvent::EquipDone {
        ok: true,
        unequip: false,
    });
    let mut slots = [-1; 15];
    slots[0] = w as i32;
    let mut flags = [false; 15];
    flags[0] = true;
    m.feed_event(LinkEvent::Equipment(slots));
    m.feed_event(LinkEvent::FreePassSlots(flags));
    m.update();
    assert!(
        equipped_mark_in(&mut m, cells[1]),
        "the free weapon's cell is equipped"
    );
    assert!(!equipped_mark_in(&mut m, cells[0]));
    assert_eq!(
        m.data.store.in_slot(0).map(|r| r.item_no),
        Some(w),
        "the slot's record"
    );
    // Equip again takes it off: 0x360 with its slot
    tap(&mut m, mid(cells[1]));
    let at = button_mid(&m, CMD_WEAR);
    tap(&mut m, at);
    assert_eq!(
        m.link_latest_request(),
        format!("FreePassUnequip({w}, {pass}, 0)")
    );
    // General: the owned items again, the owned gun not equipped while the slot holds the free weapon
    let at = button_mid(&m, CMD_STORE_GENERAL);
    tap(&mut m, at);
    assert!(strip(&m, GENERAL_TAB).1 && !strip(&m, FREE_PASS_TAB).1);
    assert!(labels_in(&mut m, cells[0]).contains(&gun_name));
    assert!(
        !equipped_mark_in(&mut m, cells[0]),
        "slot 0 holds the free weapon"
    );
    // the pass ends: the tab goes, and the Free Pass page falls back to the general one
    let at = button_mid(&m, CMD_STORE_FREE_PASS);
    tap(&mut m, at);
    m.feed_event(LinkEvent::Inventory(vec![gun.clone()]));
    m.update();
    m.input(MenuInput::Move([3.0, 3.0]));
    assert!(!strip(&m, FREE_PASS_TAB).0, "no tab after the pass");
    assert!(strip(&m, GENERAL_TAB).1);
    assert!(labels_in(&mut m, cells[0]).contains(&gun_name));
}
