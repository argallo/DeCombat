//! w3-shop: the lobby bar's Shop / Storage, Black Market, Modification and Inbox buttons, driven by presses on the
//! ROM layout's real widget rectangles (headless, no lobby server: the requests are read back from the link's
//! last-sent hook). Each opens its screen with the select sound, draws its page, and Escape (the scripted
//! "dismiss", the screens' Exit) comes back to the lobby; the shop asks for its lists and prices once, the inbox
//! asks for its first page, and the storage page's Equip sends equip / unequip for the selected item.
//!
//! ROM-gated (skipped without `CA_ROM`: the rectangles come from the user's layout pages, the item numbers from the
//! user's item table).
// SPEC: PROTOCOL_2009.md §4.1 (0x350 .. 0x358), §4.2 (0x324 / 0x326), §4.3 (0x32c)
// EVIDENCE: cshell+0x1bf20 (lobby bar 61 / 622 / 619 -> screen 0x16, 184 -> screen 0x14)

use dc_client::menu::frontfx::SOUND_SELECT;
use dc_client::menu::link::{InvRecord, LinkOptions};
use dc_client::menu::shop::{grid_cells, LEFT_WINDOW, STORAGE_GRID, STORAGE_WINDOW, WEAPON_LIST};
use dc_client::menu::storage::{own_slot, shelf_of, Wares, TAB_BODY, TAB_WEAPONS};
use dc_client::menu::{pages, Go, Menu, MenuInput, MenuKey, MenuOptions};
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

fn tap(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

/// Presses widget `name` of the open screen at its layout rectangle's centre.
fn click(m: &mut Menu, name: &str) {
    let Resolved::Part { at, enabled, .. } = resolve(m, name) else {
        panic!("{name}: {:?}", resolve(m, name));
    };
    assert!(enabled, "{name} disabled");
    tap(m, at);
}

fn shown_window(m: &Menu, id: i32) -> bool {
    m.sheet()
        .parts
        .iter()
        .position(|p| p.kind == WidgetKind::Window && p.layer == id)
        .is_some_and(|i| m.sheet().visible(i))
}

#[test]
fn rom_lobby_bar_opens_shop_market_modification_inbox_and_escape_comes_back() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.open(Go::Lobby);
    for (widget, screen, asks) in [
        ("lobby.shop", "shop", Some("ShopLists")),
        ("lobby.market", "shop", None),
        ("lobby.modification", "tuning", None),
        ("lobby.inbox", "shop", Some("InboxPage(1)")),
    ] {
        assert_eq!(m.screen_name(), "lobby");
        let sent = m.link_sent();
        let _ = m.take_sound_cues();
        click(&mut m, widget);
        assert_eq!(m.screen_name(), screen, "{widget}");
        assert!(
            m.pending_sounds().contains(&SOUND_SELECT),
            "{widget}: no click cue"
        );
        let items = m.items();
        assert!(items.len() > 40, "{widget}: {} items", items.len());
        match asks {
            Some(a) => {
                assert!(m.link_sent() > sent, "{widget}: nothing asked");
                assert_eq!(m.link_latest_request(), a, "{widget}");
            }
            // only the screen's focus notices (CLI-1561: 0x306 and 0x30a), no list request
            None => assert_eq!(m.link_sent(), sent + 2, "{widget}: asked again"),
        }
        if widget == "lobby.market" {
            // the black market tab: its first category button (control 0x1b) is up, the weapons' (0x10) is not
            let up = |id| {
                m.sheet()
                    .part(-1, id, WidgetKind::Button)
                    .is_some_and(|p| p.shown)
            };
            assert!(up(0x1b) && !up(0x10));
        }
        if widget == "lobby.inbox" {
            assert!(shown_window(&m, dc_client::menu::shop::INBOX_WINDOW));
        }
        m.input(MenuInput::Key(MenuKey::Escape));
        assert_eq!(m.screen_name(), "lobby", "{widget}: Escape");
    }
}

#[test]
fn rom_storage_equips_and_unequips_the_selected_item() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    // two primary weapons of the user's item table (shelf weapons / category 0, equipment slot 0)
    let wares = Wares::cached(m.cat.rom.as_deref());
    let primaries: Vec<u32> = (0..1000)
        .filter(|n| {
            wares.kind_of(*n as i32).and_then(shelf_of) == Some((TAB_WEAPONS, 0))
                && own_slot(&wares, *n) == Some(0)
        })
        .take(2)
        .collect();
    assert_eq!(primaries.len(), 2, "no primary weapons in the item table");
    let rec = |uid, item_no| InvRecord {
        uid,
        item_no,
        slot_kind: 1,
        ..Default::default()
    };
    m.data.store.items = vec![rec(7, primaries[0]), rec(8, primaries[1])];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = primaries[0] as i32;
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    assert!(!shown_window(&m, STORAGE_WINDOW));
    // the left pane's first equipment row (slot 0) opens the storage page
    let l = m
        .sheet()
        .part(LEFT_WINDOW, WEAPON_LIST, WidgetKind::List)
        .expect("weapon list")
        .pos;
    tap(&mut m, [l[0] as f32 + 10.0, l[1] as f32 + 10.0]);
    assert!(shown_window(&m, STORAGE_WINDOW));
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    assert!(cells.len() >= 2);
    let mid = |c: [i32; 4]| [(c[0] + c[2] / 2) as f32, (c[1] + c[3] / 2) as f32];
    let equip = m
        .sheet()
        .button(dc_client::menu::shop::CMD_WEAR)
        .filter(|b| b.shown)
        .map(|b| {
            [
                (b.pos[0] + b.size[0] / 2) as f32,
                (b.pos[1] + b.size[1] / 2) as f32,
            ]
        });
    let equip = equip.expect("no Equip button on the storage page");
    let press_equip = |m: &mut Menu| tap(m, equip);
    // the equipped one: unequip from its slot
    tap(&mut m, mid(cells[0]));
    press_equip(&mut m);
    assert_eq!(
        m.link_latest_request(),
        format!("Unequip(7, {}, 0)", primaries[0])
    );
    // the other one: equip into its own slot
    tap(&mut m, mid(cells[1]));
    press_equip(&mut m);
    assert_eq!(
        m.link_latest_request(),
        format!("Equip(8, {}, 0)", primaries[1])
    );
    m.input(MenuInput::Key(MenuKey::Escape));
    assert_eq!(m.screen_name(), "lobby");
}

/// LG-12 / CLI-695: every category of the weapons and body tabs lists the GP products of the shown list by the
/// display number of their item row, in that order: the cell counts the 2009 screen showed against our lobby
/// (reference client shots 2026-10-27: Primary 47, Secondary 9, Melee 5, Support 9, Other 7, Special 13; Head 12, Face 9,
/// Uniform 4, Vest 3, Backpack 2) and the modification tabs (AR 9, SR 6, MG 3, SMG 9, SHOTGUN 3, PISTOL 9).
#[test]
fn rom_shop_categories_list_every_gp_item_in_display_order() {
    use dc_client::menu::storage::{Store, TAB_BODY, TAB_OPTIONS};
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(Some(&rom));
    let store = Store {
        view: Some(wares.catalogue_ids()),
        ..Store::default()
    };
    let counts = |tab, n| -> Vec<usize> {
        (0..n)
            .map(|c| {
                let shelf = wares.shelf(&store, tab, c);
                assert!(
                    shelf.windows(2).all(|p| p[0].display <= p[1].display),
                    "tab {tab} category {c} out of display order"
                );
                shelf.len()
            })
            .collect()
    };
    assert_eq!(counts(TAB_WEAPONS, 6), [47, 9, 5, 9, 7, 13]);
    assert_eq!(counts(TAB_BODY, 5), [12, 9, 4, 3, 2]);
    let options = counts(TAB_OPTIONS, 18);
    let per_class: Vec<usize> = options.chunks(3).map(|c| c.iter().sum()).collect();
    assert_eq!(per_class, [9, 6, 3, 9, 3, 9]);
}

/// CLI-695 / CLI-697: the shop page of the weapons tab's first category draws six product cells: the cell frame,
/// the item's slot image from the item table, the GP mark; the left pane's equipped row its item's image.
#[test]
fn rom_shop_page_draws_frames_item_images_and_marks() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{MARK_GP, SHOP_FRAMES};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    let shelf = wares.shelf(&m.data.store, TAB_WEAPONS, 0);
    let first = shelf[0].item_no;
    m.data.store.items = vec![InvRecord {
        uid: 1,
        item_no: first as u32,
        slot_kind: 1,
        ..Default::default()
    }];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = first;
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let texes: Vec<String> = m
        .items()
        .iter()
        .filter_map(|i| match i {
            DrawItem::Image { tex, .. } => Some(tex.clone()),
            _ => None,
        })
        .collect();
    let count = |t: &str| texes.iter().filter(|x| x.as_str() == t).count();
    assert_eq!(count(SHOP_FRAMES[0]) + count(SHOP_FRAMES[1]), 6);
    assert_eq!(count(MARK_GP), 6);
    for w in &shelf[..6] {
        let icon = w.icon.as_deref().expect("item image");
        let want = if w.item_no == first { 2 } else { 1 };
        assert_eq!(count(icon), want, "{}", w.item_no);
    }
}

/// The draw items of the shop's first weapons page (every catalogue product shown, account experience `exp`).
fn first_page(exp: u32) -> Option<(Menu, Vec<dc_client::hud::items::DrawItem>)> {
    let mut m = menu()?;
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    m.data.profile.exp = exp;
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let items = m.items().to_vec();
    Some((m, items))
}

/// CLI-720: a cell shows the rank badge of its required experience at (8, 0x2a), 30 x 22 (the reference stack's first
/// Primary page: none on M4A1 / AK-74U / K2, badges on M14 / AK-47 / FAMAS).
#[test]
fn rom_shop_cells_show_the_rank_badge_of_their_required_experience() {
    use dc_client::hud::board::LevelTable;
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{badge_of, BADGE_AT, BADGE_SIZE, SHOP_GRID, SHOP_WINDOW};
    let Some((m, items)) = first_page(0) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let rom = m.cat.rom.clone().unwrap();
    let levels = LevelTable::from_rom(&rom).expect("level table");
    let wares = Wares::cached(Some(&rom));
    let shelf = wares.shelf(&m.data.store, TAB_WEAPONS, 0);
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    let badge_at = |r: [i32; 4]| -> Option<String> {
        items.iter().find_map(|i| match i {
            DrawItem::Image { tex, dst, .. }
                if dst[0] == (r[0] + BADGE_AT[0]) as f32
                    && dst[1] == (r[1] + BADGE_AT[1]) as f32
                    && dst[2] == BADGE_SIZE[0] as f32
                    && dst[3] == BADGE_SIZE[1] as f32 =>
            {
                Some(tex.clone())
            }
            _ => None,
        })
    };
    let got: Vec<bool> = cells.iter().map(|r| badge_at(*r).is_some()).collect();
    assert_eq!(got, [false, false, false, true, true, true]);
    for (w, r) in shelf.iter().zip(&cells) {
        assert_eq!(
            badge_at(*r).as_deref(),
            badge_of(&levels, w.unlock_exp),
            "{}",
            w.name
        );
    }
}

/// CLI-721: a cell whose required experience is above the character's draws its item image black and the LOCKED
/// tag under the name; with enough experience neither shows.
#[test]
fn rom_shop_cells_above_the_character_draw_a_silhouette_and_the_locked_tag() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{LOCKED_TAG, LOCKED_TINT};
    for exp in [0, u32::MAX] {
        let Some((m, items)) = first_page(exp) else {
            eprintln!("skipped: no CA_ROM");
            return;
        };
        let wares = Wares::cached(m.cat.rom.as_deref());
        let shelf = wares.shelf(&m.data.store, TAB_WEAPONS, 0);
        let want: Vec<bool> = shelf[..6].iter().map(|w| exp < w.unlock_exp).collect();
        if exp == 0 {
            assert!(want.iter().any(|l| *l) && want.iter().any(|l| !*l));
        } else {
            assert!(want.iter().all(|l| !*l));
        }
        let tags = items
            .iter()
            .filter(|i| matches!(i, DrawItem::Image { tex, .. } if tex == LOCKED_TAG))
            .count();
        assert_eq!(tags, want.iter().filter(|l| **l).count(), "exp {exp}");
        for (w, l) in shelf[..6].iter().zip(&want) {
            let icon = w.icon.as_deref().unwrap();
            let tint = items.iter().find_map(|i| match i {
                DrawItem::Image { tex, tint, .. } if tex == icon => Some(*tint),
                _ => None,
            });
            assert_eq!(tint == Some(LOCKED_TINT), *l, "{} exp {exp}", w.name);
        }
    }
}

/// CLI-722: each weapon cell shows its class tag under the name at (5, 0x10) and the LOCKED tag 2 px after it; a
/// cell whose product is in the New list (0x355) shows the New mark at its top right.
#[test]
fn rom_shop_cells_show_the_class_tag_and_the_new_mark() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{class_tag, LOCKED_TAG, NEW_MARK, SHOP_GRID, SHOP_WINDOW, TAG_Y};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    let shelf = wares.shelf(&m.data.store, TAB_WEAPONS, 0);
    m.data.store.highlight[1] = [shelf[4].product].into_iter().collect();
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let items = m.items().to_vec();
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    let at = |tex: &str| -> Vec<[f32; 2]> {
        items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Image { tex: t, dst, .. } if t == tex => Some([dst[0], dst[1]]),
                _ => None,
            })
            .collect()
    };
    let (ar, size) = class_tag(0).unwrap();
    let want: Vec<[f32; 2]> = cells
        .iter()
        .map(|r| [(r[0] + 5) as f32, (r[1] + TAG_Y) as f32])
        .collect();
    // the six cells, then the detail panel's tag of the selected cell
    assert_eq!(at(ar)[..6], want, "every first-page cell is an AR");
    let c = cells[4];
    assert_eq!(
        at(NEW_MARK.0),
        [[(c[0] + c[2] - NEW_MARK.1[0]) as f32, c[1] as f32]]
    );
    // exp 0: the locked cells' tag sits after the class tag
    for p in at(LOCKED_TAG) {
        assert!(cells
            .iter()
            .any(|r| p == [(r[0] + 7 + size[0]) as f32, (r[1] + TAG_Y) as f32]));
    }
}

/// CLI-723: the shop page opens with its first cell selected (the selected frame) and the detail panel under the
/// grid shows that item (frame, detail image, name, five gauges); a press on another cell selects it and the panel
/// follows.
#[test]
fn rom_shop_selects_the_first_cell_and_the_detail_panel_follows_the_selection() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{
        DETAIL_AT, DETAIL_FRAME, DETAIL_NAME_AT, GAUGE_OUTLINE, SHOP_FRAMES, SHOP_GRID, SHOP_WINDOW,
    };
    let Some((mut m, items)) = first_page(u32::MAX) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let shelf = wares.shelf(&m.data.store, TAB_WEAPONS, 0);
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    let image_at = |items: &[DrawItem], tex: &str, at: [i32; 2]| {
        items.iter().any(|i| {
            matches!(i, DrawItem::Image { tex: t, dst, .. }
                if t == tex && dst[0] == at[0] as f32 && dst[1] == at[1] as f32)
        })
    };
    let name_in_panel = |items: &[DrawItem]| -> Vec<String> {
        let at = [
            (DETAIL_AT[0] + DETAIL_NAME_AT[0]) as f32,
            (DETAIL_AT[1] + DETAIL_NAME_AT[1]) as f32,
        ];
        items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Label {
                    text, pos, style, ..
                } if *pos == at && style.font == 0xe => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    assert!(image_at(&items, SHOP_FRAMES[1], [cells[0][0], cells[0][1]]));
    assert!(image_at(&items, DETAIL_FRAME.0, DETAIL_AT));
    let art = wares.art_of(shelf[0].item_no).unwrap();
    assert!(!art.panel_picture.is_empty());
    assert!(image_at(&items, &art.panel_picture, DETAIL_AT));
    let outlines = items
        .iter()
        .filter(|i| matches!(i, DrawItem::Image { tex, .. } if tex == GAUGE_OUTLINE.0))
        .count();
    assert_eq!(outlines, 5);
    assert_eq!(name_in_panel(&items), [shelf[0].name.clone()]);
    let c = cells[3];
    tap(&mut m, [(c[0] + 20) as f32, (c[1] + 20) as f32]);
    let items = m.items().to_vec();
    assert!(image_at(&items, SHOP_FRAMES[1], [c[0], c[1]]));
    assert!(image_at(&items, SHOP_FRAMES[0], [cells[0][0], cells[0][1]]));
    assert_eq!(name_in_panel(&items), [shelf[3].name.clone()]);
}

/// The body tab's left pane lists the 2009 body slots in their order; each empty slot shows its slot name.
#[test]
fn rom_body_tab_rows_name_their_empty_slots() {
    use dc_client::hud::items::DrawItem;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.data.store.equipped = [-1; 15];
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    click(&mut m, "shop.body");
    let texts: Vec<(String, f32)> = m
        .items()
        .iter()
        .filter_map(|i| match i {
            // the left pane only (the category buttons on the right carry some of the same words)
            DrawItem::Text { text, pos, .. } | DrawItem::Label { text, pos, .. }
                if pos[0] < 470.0 =>
            {
                Some((text.clone(), pos[1]))
            }
            _ => None,
        })
        .collect();
    // the string table's slot names, in the list's order
    let names: Vec<String> = [0x5b6, 0x33d, 0x1634, 0x33e, 0x33f, 0x340]
        .iter()
        .map(|id| m.cat.text(*id))
        .collect();
    let mut last = f32::MIN;
    for n in &names {
        assert!(!n.is_empty());
        let (_, y) = texts
            .iter()
            .find(|(t, _)| t == n)
            .unwrap_or_else(|| panic!("empty row {n:?} drawn"));
        assert!(*y > last, "{n:?} below the row before");
        last = *y;
    }
}

/// CLI-724: the detail panel's description (font 0x17: Tahoma bold, cell 14) is drawn at the whole pixels per em GDI
/// rasterises it with (12, not 11.6) and wraps at the observed width, so the first Primary item's description wraps
/// where the reference stack wraps it: two words on the second line (OBSERVED
/// shop-guns/cpp/shop_bb00_weapons_c0_p1.png).
#[test]
fn rom_the_detail_description_wraps_where_the_reference_wraps_it() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{DETAIL_AT, DETAIL_DESC_AT};
    let Some((_m, items)) = first_page(u32::MAX) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let x = (DETAIL_AT[0] + DETAIL_DESC_AT[0]) as f32;
    let lines: Vec<String> = items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Label {
                text, pos, style, ..
            } if pos[0] == x
                && style.font == 0x17
                && pos[1] >= (DETAIL_AT[1] + DETAIL_DESC_AT[1]) as f32 =>
            {
                Some(text.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[1].split_whitespace().count(), 2, "{lines:?}");
}

/// CLI-725: Gift is enabled only while the selected cell's product is a credit product (the first Primary page's
/// GP cells: disabled, as the reference stack draws it; a category whose first cell sells for credit: enabled).
#[test]
fn rom_gift_is_enabled_only_for_a_selected_credit_product() {
    use dc_client::menu::shop::{CATEGORY_COMMANDS, GIFT_ID, SHOP_WINDOW};
    use dc_client::menu::storage::TAB_MARKET;
    let Some((mut m, _)) = first_page(u32::MAX) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let gift = |m: &Menu| {
        m.sheet()
            .parts
            .iter()
            .find(|p| p.kind == WidgetKind::Button && p.layer == SHOP_WINDOW && p.id == GIFT_ID)
            .map(|p| p.enabled)
            .expect("gift button")
    };
    assert!(!gift(&m), "GP cell selected: gift enabled");
    // drawn disabled: the caption in the record's third colour (cshell+0x18930)
    let (text, colour) = m
        .sheet()
        .parts
        .iter()
        .find_map(|p| match &p.look {
            dc_client::menu::sheet::Look::Button {
                caption: Some(c), ..
            } if p.kind == WidgetKind::Button && p.layer == SHOP_WINDOW && p.id == GIFT_ID => {
                Some((m.cat.text(c.string_id), c.colours[2]))
            }
            _ => None,
        })
        .expect("gift caption");
    let rgba = [
        (colour >> 16) as u8,
        (colour >> 8) as u8,
        colour as u8,
        (colour >> 24) as u8,
    ];
    assert!(m.items().iter().any(|i| matches!(i,
        dc_client::hud::items::DrawItem::Label { text: t, style, .. } if *t == text && style.colour == rgba)));
    let wares = Wares::cached(m.cat.rom.as_deref());
    let cat = (0..CATEGORY_COMMANDS[TAB_MARKET].len())
        .find(|c| {
            wares
                .shelf(&m.data.store, TAB_MARKET, *c)
                .first()
                .is_some_and(|w| w.sale_type == 1)
        })
        .expect("a credit category");
    click(&mut m, "shop.market");
    let cmd = CATEGORY_COMMANDS[TAB_MARKET][cat];
    let b = m
        .sheet()
        .parts
        .iter()
        .position(|p| p.kind == WidgetKind::Button && p.command == cmd)
        .unwrap();
    let p = &m.sheet().parts[b];
    let at = [
        (p.pos[0] + p.size[0] / 2) as f32,
        (p.pos[1] + p.size[1] / 2) as f32,
    ];
    tap(&mut m, at);
    assert!(gift(&m), "credit cell selected: gift disabled");
}

/// CLI-726: a gear cell shows its type tag (the backpack shelf: BACKPACK) under the name at (5, 0x10) and, when
/// locked, the LOCKED tag 2 px after it; the detail panel shows the tag under the item name.
#[test]
fn rom_gear_cells_show_their_type_tag() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{
        type_tag, CATEGORY_COMMANDS, DETAIL_AT, DETAIL_TAG_AT, LOCKED_TAG, SHOP_GRID, SHOP_WINDOW,
        TAG_X, TAG_Y,
    };
    use dc_client::menu::storage::TAB_BODY;
    let Some((mut m, _)) = first_page(0) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    click(&mut m, "shop.body");
    let cmd = CATEGORY_COMMANDS[TAB_BODY][4];
    let b = m
        .sheet()
        .parts
        .iter()
        .position(|p| p.kind == WidgetKind::Button && p.command == cmd)
        .unwrap();
    let p = &m.sheet().parts[b];
    let at = [
        (p.pos[0] + p.size[0] / 2) as f32,
        (p.pos[1] + p.size[1] / 2) as f32,
    ];
    tap(&mut m, at);
    let items = m.items().to_vec();
    let wares = Wares::cached(m.cat.rom.as_deref());
    let shelf = wares.shelf(&m.data.store, TAB_BODY, 4);
    assert!(!shelf.is_empty());
    let (tag, size) = type_tag(0x10).unwrap();
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    let image_at = |tex: &str, x: i32, y: i32| {
        items.iter().any(|i| {
            matches!(i, DrawItem::Image { tex: t, dst, .. } if t == tex && dst[0] == x as f32 && dst[1] == y as f32)
        })
    };
    for (w, r) in shelf.iter().zip(&cells) {
        assert_eq!(w.kind_of, Some(0x10), "{}", w.name);
        assert!(
            image_at(tag, r[0] + TAG_X, r[1] + TAG_Y),
            "{}: no tag",
            w.name
        );
        if w.unlock_exp > 0 {
            assert!(
                image_at(LOCKED_TAG, r[0] + 7 + size[0], r[1] + TAG_Y),
                "{}",
                w.name
            );
        }
    }
    assert!(image_at(
        tag,
        DETAIL_AT[0] + DETAIL_TAG_AT[0],
        DETAIL_AT[1] + DETAIL_TAG_AT[1]
    ));
    // the 10000s: a character row (ItemType 22) tags CHARACTER, a specialist row (30) SPECIALIST
    let tags: Vec<Option<i32>> = (0..6)
        .flat_map(|c| wares.shelf(&m.data.store, dc_client::menu::storage::TAB_MARKET, c))
        .filter(|w| (10000..11000).contains(&w.item_no))
        .map(|w| w.tag_kind)
        .collect();
    assert!(
        tags.contains(&Some(0x16)) && tags.contains(&Some(0x1e)),
        "{tags:?}"
    );
}

/// CLI-727: a modification cell decorates like a shop cell (rank badge, LOCKED + silhouette for a character below
/// the requirement) and shows its price with the unit as text at (0xbd, 0x48), without the mark image.
#[test]
fn rom_modification_cells_show_badge_locked_and_text_price() {
    use dc_client::hud::board::LevelTable;
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{
        badge_of, price_text, BADGE_AT, LOCKED_TAG, MARK_GP, TAG_X, TAG_Y,
    };
    use dc_client::menu::tuning::{tab_wares, SHOP_GRID, SHOP_WINDOW, WARE_PRICE_AT};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    m.data.profile.exp = 0;
    m.open(Go::Lobby);
    click(&mut m, "lobby.modification");
    let items = m.items().to_vec();
    let shown = tab_wares(&wares, &m.data.store, 0);
    assert!(!shown.is_empty());
    let cells = grid_cells(&m.cat, pages::TUNING, SHOP_WINDOW, SHOP_GRID);
    let levels = LevelTable::from_rom(m.cat.rom.as_deref().unwrap()).unwrap();
    let image_at = |tex: &str, x: i32, y: i32| {
        items.iter().any(|i| {
            matches!(i, DrawItem::Image { tex: t, dst, .. } if t == tex && dst[0] == x as f32 && dst[1] == y as f32)
        })
    };
    let (mut locked, mut badges) = (0, 0);
    for (w, r) in shown.iter().zip(&cells) {
        let price = items.iter().any(|i| {
            matches!(i, DrawItem::Label { text, pos, .. } if *text == price_text(w)
                && pos[0] == (r[0] + WARE_PRICE_AT[0]) as f32 && pos[1] == (r[1] + WARE_PRICE_AT[1]) as f32)
        });
        assert!(price, "{}: no text price", w.name);
        if w.unlock_exp > 0 {
            assert!(
                image_at(LOCKED_TAG, r[0] + TAG_X, r[1] + TAG_Y),
                "{}",
                w.name
            );
            locked += 1;
        }
        if let Some(b) = badge_of(&levels, w.unlock_exp) {
            assert!(
                image_at(b, r[0] + BADGE_AT[0], r[1] + BADGE_AT[1]),
                "{}",
                w.name
            );
            badges += 1;
        }
    }
    assert!(locked > 0 && badges > 0, "locked {locked} badges {badges}");
    assert!(!items
        .iter()
        .any(|i| matches!(i, DrawItem::Image { tex, .. } if tex == MARK_GP)));
}

/// CLI-728: the detail panel's attachment marks take the form of the weapon row's socket states (0 `_disable`,
/// 1 `_D`, 2 `_Equip`), and every form resolves in the ROM.
#[test]
fn rom_detail_marks_follow_the_weapon_socket_states() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{mark_form, DETAIL_MODS};
    let Some((mut m, items)) = first_page(u32::MAX) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let shelf = wares.shelf(&m.data.store, TAB_WEAPONS, 0);
    let states = wares.art_of(shelf[0].item_no).unwrap().socket_states;
    for ((base, _), s) in DETAIL_MODS.iter().zip(states) {
        let t = mark_form(base, s).unwrap();
        assert!(
            items
                .iter()
                .any(|i| matches!(i, DrawItem::Image { tex, .. } if *tex == t)),
            "{t}"
        );
        for k in 0..3 {
            let f = mark_form(base, k).unwrap();
            assert!(m.cat.tex_size(&f).is_some(), "{f}");
        }
    }
    // the ROM has weapons with every state
    let all: Vec<i32> = (0..1000)
        .filter_map(|n| wares.art_of(n))
        .flat_map(|a| a.socket_states)
        .collect();
    for s in 0..3 {
        assert!(all.contains(&s), "state {s}: {all:?}");
    }
}

/// CLI-761 / CLI-762: a product without a period shows PERMANENT 17 px under its cell's top (no tag), the detail
/// panel draws the item's function lines in their colour runs (base 0xffebb14d), and the current tab button is
/// selected (the others not).
#[test]
fn rom_permanent_mark_function_lines_and_the_selected_tab() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{
        grid_cells, FUNCTION_COLOUR, PERMANENT_MARK, PERMANENT_UNDER_NAME, SHOP_GRID, SHOP_WINDOW,
        TAB_IDS, TAG_X,
    };
    let Some((mut m, _)) = first_page(u32::MAX) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    click(&mut m, "shop.market_tab");
    click(&mut m, "cmd:1029");
    let items = m.items().to_vec();
    let shown = wares.shelf(&m.data.store, dc_client::menu::storage::TAB_MARKET, 2);
    let mut permanent = 0;
    for (w, r) in shown.iter().zip(&cells) {
        let drawn = items.iter().any(|i| {
            matches!(i, DrawItem::Image { tex, dst, .. } if tex == PERMANENT_MARK.0
                && dst[0] == (r[0] + TAG_X) as f32 && dst[1] == (r[1] + PERMANENT_UNDER_NAME) as f32)
        });
        assert_eq!(drawn, w.permanent(), "{}", w.name);
        permanent += drawn as usize;
    }
    assert!(permanent > 0, "no permanent product on the page");
    let selected: Vec<bool> = TAB_IDS
        .iter()
        .map(|id| {
            m.sheet()
                .part(-1, *id, WidgetKind::Button)
                .unwrap()
                .selected
        })
        .collect();
    assert_eq!(selected, [false, false, true, false]);
    // the backpack page: the Advanced Backpack's function line
    click(&mut m, "shop.body");
    click(&mut m, "cmd:263");
    let items = m.items().to_vec();
    let line = wares
        .shelf(&m.data.store, TAB_BODY, 4)
        .first()
        .and_then(|w| wares.art_of(w.item_no))
        .and_then(|a| a.functions.first().cloned())
        .expect("a backpack function line");
    assert!(items.iter().any(|i| {
        matches!(i, DrawItem::Label { text, style, .. }
            if *text == line && style.colour == dc_client::menu::catalog::argb(FUNCTION_COLOUR))
    }));
}

/// CLI-854: the shop detail panel of a keyed (HiSec) supply case draws the HiSec mark 1 px under the class tag
/// (cshell+0xd7ec0 +0x138); a plain case draws none.
#[test]
fn rom_detail_panel_of_a_keyed_case_draws_the_hisec_mark() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::inbox::HISEC_MARK;
    use dc_client::menu::shop::{draw_detail, DETAIL_AT, DETAIL_TAG_AT};
    let Some(m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let mut m = m;
    let wares = Wares::cached(m.cat.rom.as_deref());
    let case_of = |hisec: bool| {
        (0..6000u32).find_map(|p| {
            let w = wares.ware(p, &Default::default())?;
            let ok = w.kind_of == Some(0x14)
                && wares
                    .art_of(w.item_no)
                    .is_some_and(|a| a.keyed_case == hisec);
            ok.then_some(w)
        })
    };
    let (Some(hi), Some(plain)) = (case_of(true), case_of(false)) else {
        eprintln!("skipped: the catalogue has no HiSec / plain case product");
        return;
    };
    let mark = |m: &mut Menu, w: &dc_client::menu::storage::Ware| {
        let mut out = Vec::new();
        draw_detail(
            &mut m.cat,
            w,
            wares.art_of(w.item_no),
            Default::default(),
            &mut out,
        );
        out.into_iter().find_map(|i| match i {
            DrawItem::Image { tex, dst, .. } if tex == HISEC_MARK.0 => Some(dst),
            _ => None,
        })
    };
    let at = mark(&mut m, &hi).expect("the HiSec case panel draws the mark");
    assert_eq!(at[0], (DETAIL_AT[0] + DETAIL_TAG_AT[0]) as f32);
    assert!(at[1] >= (DETAIL_AT[1] + DETAIL_TAG_AT[1]) as f32, "{at:?}");
    assert!(mark(&mut m, &plain).is_none(), "a plain case draws none");
}

/// CLI-860: the detail panel's mark stack (cshell+0xd7ec0): LOCKED (+0x148) for a ware above the character, then
/// TEMPORARY (+0x13c) for an owned item with an end date, PERMANENT (+0x140) for a product without a period.
#[test]
fn rom_detail_panel_draws_permanent_temporary_and_locked_marks() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::inbox::TEMPORARY_MARK;
    use dc_client::menu::shop::{
        draw_detail, DetailMarks, DETAIL_AT, DETAIL_TAG_AT, LOCKED_TAG, PERMANENT_MARK,
    };
    let Some(m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let mut m = m;
    let wares = Wares::cached(m.cat.rom.as_deref());
    let find = |perm: bool| {
        (0..6000u32).find_map(|p| {
            let w = wares.ware(p, &Default::default())?;
            (w.permanent() == perm).then_some(w)
        })
    };
    let (Some(perm), Some(dated)) = (find(true), find(false)) else {
        eprintln!("skipped: no permanent / dated product");
        return;
    };
    let at = |m: &mut Menu, w: &dc_client::menu::storage::Ware, marks: DetailMarks, tex: &str| {
        let mut out = Vec::new();
        draw_detail(&mut m.cat, w, wares.art_of(w.item_no), marks, &mut out);
        out.into_iter().find_map(|i| match i {
            DrawItem::Image { tex: t, dst, .. } if t == tex => Some(dst),
            _ => None,
        })
    };
    let none = DetailMarks::default();
    let p = at(&mut m, &perm, none, PERMANENT_MARK.0).expect("PERMANENT on a permanent product");
    assert_eq!(p[0], (DETAIL_AT[0] + DETAIL_TAG_AT[0]) as f32);
    assert!(at(&mut m, &dated, none, PERMANENT_MARK.0).is_none());
    assert!(at(&mut m, &dated, none, TEMPORARY_MARK.0).is_none());
    let t = DetailMarks {
        locked: false,
        temporary: true,
    };
    assert!(at(&mut m, &dated, t, TEMPORARY_MARK.0).is_some());
    let l = DetailMarks {
        locked: true,
        temporary: false,
    };
    let lk = at(&mut m, &perm, l, LOCKED_TAG).expect("LOCKED mark");
    assert!(at(&mut m, &perm, none, LOCKED_TAG).is_none());
    // the stack starts under the LOCKED mark
    let p2 = at(&mut m, &perm, l, PERMANENT_MARK.0).unwrap();
    assert!(p2[1] > lk[1], "{p2:?} {lk:?}");
}

/// CLI-962: a locked ware's detail panel also shows the level icon of the experience it asks for, 30 x 22 at (8, 100)
/// of the panel (cshell+0xd7ec0 / +0x14c).
#[test]
fn rom_locked_detail_panel_draws_the_rank_icon() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{draw_detail, DetailMarks, DETAIL_AT, RANK_ICON_AT};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let Some(w) = (0..6000u32)
        .filter_map(|p| wares.ware(p, &Default::default()))
        .find(|w| w.unlock_exp > 100_000)
    else {
        eprintln!("skipped: no ware with an experience requirement");
        return;
    };
    let icon = |locked: bool, m: &mut Menu| {
        let mut out = Vec::new();
        let marks = DetailMarks {
            locked,
            temporary: false,
        };
        draw_detail(&mut m.cat, &w, wares.art_of(w.item_no), marks, &mut out);
        out.into_iter().find_map(|i| match i {
            DrawItem::Image { tex, dst, .. }
                if dst
                    == [
                        (DETAIL_AT[0] + RANK_ICON_AT[0]) as f32,
                        (DETAIL_AT[1] + RANK_ICON_AT[1]) as f32,
                        30.0,
                        22.0,
                    ] =>
            {
                Some(tex)
            }
            _ => None,
        })
    };
    assert!(
        icon(true, &mut m).is_some(),
        "the rank icon of a locked ware"
    );
    assert!(icon(false, &mut m).is_none());
}

/// CLI-942: the storage page's selected cell shows the detail panel (frame, picture, name, gauges); an owned item
/// with a real end date also shows the period line above the panel's right edge.
#[test]
fn rom_storage_selected_cell_draws_the_detail_panel_and_the_period_line() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{
        DETAIL_AT, DETAIL_FRAME, DETAIL_NAME_AT, EXPIRES_AT, GAUGE_OUTLINE,
    };
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let n = (0..1000u32)
        .find(|n| {
            wares.kind_of(*n as i32).and_then(shelf_of) == Some((TAB_WEAPONS, 0))
                && own_slot(&wares, *n) == Some(0)
        })
        .expect("a primary weapon");
    // a FILETIME a year from now
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let end = (now + 365 * 86_400 + 11_644_473_600) * 10_000_000;
    m.data.store.items = vec![InvRecord {
        uid: 7,
        item_no: n,
        slot_kind: 1,
        end,
        ..Default::default()
    }];
    m.data.store.equipped = [-1; 15];
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let l = m
        .sheet()
        .part(LEFT_WINDOW, WEAPON_LIST, WidgetKind::List)
        .unwrap()
        .pos;
    tap(&mut m, [l[0] as f32 + 10.0, l[1] as f32 + 10.0]);
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    let c = cells[0];
    let has_frame = |m: &mut Menu| {
        m.items()
            .iter()
            .any(|i| matches!(i, DrawItem::Image { tex, .. } if tex == DETAIL_FRAME.0))
    };
    // CLI-1265: the storage page opens with its first cell selected (the 2009 page shows its panel at once)
    assert!(has_frame(&mut m), "the first cell's panel on open");
    tap(&mut m, [(c[0] + 20) as f32, (c[1] + 20) as f32]);
    assert!(has_frame(&mut m), "the selected cell's panel");
    let items = m.items().to_vec();
    let outlines = items
        .iter()
        .filter(|i| matches!(i, DrawItem::Image { tex, .. } if tex == GAUGE_OUTLINE.0))
        .count();
    assert_eq!(outlines, 5, "a weapon's five gauges");
    let name_at = [
        (DETAIL_AT[0] + DETAIL_NAME_AT[0]) as f32,
        (DETAIL_AT[1] + DETAIL_NAME_AT[1]) as f32,
    ];
    assert!(items
        .iter()
        .any(|i| matches!(i, DrawItem::Label { text, pos, .. }
        if *pos == name_at && *text == wares.label_of(n as i32))));
    let line_at = [
        (DETAIL_AT[0] + EXPIRES_AT[0]) as f32,
        (DETAIL_AT[1] + EXPIRES_AT[1]) as f32,
    ];
    // a year ahead: 364 whole days and some hours
    assert!(
        items
            .iter()
            .any(|i| matches!(i, DrawItem::Label { text, pos, .. }
            if *pos == line_at && text.starts_with("Remaining use : 36"))),
        "the remaining-use line"
    );
    // a record without an end date: Permanent Item, or Default Item with a default slot kind
    for (slot_kind, want) in [(0, "Permanent Item"), (1, "Default Item")] {
        m.data.store.items[0].end = 0;
        m.data.store.items[0].slot_kind = slot_kind;
        let items = m.items().to_vec();
        assert!(
            items
                .iter()
                .any(|i| matches!(i, DrawItem::Label { text, pos, .. }
                if *pos == line_at && text.ends_with(want))),
            "{want}"
        );
    }
}

#[test]
fn filetime_text_is_the_utc_date_and_time() {
    use dc_client::menu::shop::filetime_text;
    // 2026-10-28 12:34:56 UTC
    let unix: u64 = 1_793_190_896;
    assert_eq!(
        filetime_text((unix + 11_644_473_600) * 10_000_000),
        "28/10/2026 12:34:56"
    );
}

/// CLI-1000: a storage cell shows the PERMANENT mark of an item without an end date under its class tag; CLI-1264: a
/// temporary one shows the days / hours left as a label in the mark's place (top left, inside the panel), in the
/// short form when the words do not fit left of the item picture.
#[test]
fn rom_storage_cells_show_the_period_under_the_class_tag() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{
        class_tag, CELL_FONT, PERIOD_LABEL_COLOUR, PERIOD_LABEL_ROOM, PERIOD_LABEL_SIZE,
        PERMANENT_MARK,
    };
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let prim: Vec<u32> = (0..1000u32)
        .filter(|n| {
            wares.kind_of(*n as i32).and_then(shelf_of) == Some((TAB_WEAPONS, 0))
                && own_slot(&wares, *n) == Some(0)
        })
        .take(2)
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // half an hour past 3 d 5 h, so a second elapsing in the test does not turn it into 3 d 4 h
    let end = (now + 3 * 86_400 + 5 * 3600 + 1800 + 11_644_473_600) * 10_000_000;
    m.data.store.items = vec![
        InvRecord {
            uid: 7,
            item_no: prim[0],
            slot_kind: 1,
            end,
            ..Default::default()
        },
        InvRecord {
            uid: 8,
            item_no: prim[1],
            slot_kind: 1,
            end: 0,
            ..Default::default()
        },
    ];
    m.data.store.equipped = [-1; 15];
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let l = m
        .sheet()
        .part(LEFT_WINDOW, WEAPON_LIST, WidgetKind::List)
        .unwrap()
        .pos;
    tap(&mut m, [l[0] as f32 + 10.0, l[1] as f32 + 10.0]);
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    let items = m.items().to_vec();
    // CLI-1266: the 2009 places in the cell (virtual units): class tag at (5, 0x11), the period under it at (5, 26)
    // (the tag's y + its 8-texel texture height + 1), as the reference stack draws them at 1024 x 768
    let label_at = |c: [i32; 4]| [(c[0] + 5) as f32, (c[1] + 26) as f32];
    let tags: Vec<&str> = prim
        .iter()
        .filter_map(|n| wares.kind_of(*n as i32).and_then(class_tag))
        .map(|t| t.0)
        .collect();
    for c in &cells[..2] {
        assert!(
            items.iter().any(|i| matches!(i, DrawItem::Image { tex, dst, .. }
                if tags.contains(&tex.as_str()) && dst[0] == (c[0] + 5) as f32 && dst[1] == (c[1] + 0x11) as f32)),
            "class tag at (5, 0x11)"
        );
    }
    let text = items
        .iter()
        .find_map(|i| match i {
            DrawItem::Label {
                text, pos, style, ..
            } if *pos == label_at(cells[0]) => {
                let c = PERIOD_LABEL_COLOUR.to_be_bytes();
                assert_eq!(style.colour, [c[1], c[2], c[3], c[0]], "{text}");
                Some(text.clone())
            }
            _ => None,
        })
        .expect("the period label under the class tag");
    assert!(text == "3d 5h" || text.contains("3 days"), "{text}");
    let w = m
        .cat
        .measure_text_font_units(CELL_FONT, PERIOD_LABEL_SIZE, &text)
        .unwrap();
    assert!(w <= PERIOD_LABEL_ROOM as f32, "{text} is {w} wide");
    assert!(
        !items
            .iter()
            .any(|i| matches!(i, DrawItem::Label { pos, .. } if *pos == label_at(cells[1]))),
        "no period text on a permanent item"
    );
    let mark_in = |c: [i32; 4]| {
        items.iter().any(|i| {
            matches!(i, DrawItem::Image { tex, dst, .. }
            if tex == PERMANENT_MARK.0 && [dst[0], dst[1]] == label_at(c))
        })
    };
    assert!(
        !mark_in(cells[0]) && mark_in(cells[1]),
        "PERMANENT mark on the item without an end"
    );
}

/// CLI-1001 / CLI-1002: the storage page's top strip shows General alone, selected (Special Item and Free Pass are
/// enabled only with such items), and an equipped item's name keeps the cell colour (the Equipped mark tells it).
#[test]
fn rom_storage_top_strip_is_general_alone_and_names_keep_the_cell_colour() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::CELL_COLOUR;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let n = (0..1000u32)
        .find(|n| {
            wares.kind_of(*n as i32).and_then(shelf_of) == Some((TAB_WEAPONS, 0))
                && own_slot(&wares, *n) == Some(0)
        })
        .unwrap();
    m.data.store.items = vec![InvRecord {
        uid: 7,
        item_no: n,
        slot_kind: 1,
        ..Default::default()
    }];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = n as i32;
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let l = m
        .sheet()
        .part(LEFT_WINDOW, WEAPON_LIST, WidgetKind::List)
        .unwrap()
        .pos;
    tap(&mut m, [l[0] as f32 + 10.0, l[1] as f32 + 10.0]);
    let tab = |m: &Menu, id| {
        let b = m
            .sheet()
            .part(STORAGE_WINDOW, id, WidgetKind::Button)
            .unwrap();
        (b.shown, b.selected)
    };
    assert_eq!(tab(&m, 5), (true, true), "General");
    assert!(!tab(&m, 6).0 && !tab(&m, 7).0, "Free Pass / Special Item");
    let name = wares.label_of(n as i32);
    let items = m.items().to_vec();
    assert!(items
        .iter()
        .any(|i| matches!(i, DrawItem::Label { text, style, .. }
        if *text == name && {
            let mut c = style.colour;
            let mut w = CELL_COLOUR.to_le_bytes();
            c.sort();
            w.sort();
            c == w
        })));
}

/// CLI-552 amended (playtest report: the NX shown stayed at 10,000 after buying): a credit product's
/// successful purchase reply lowers the balance the shop shows by the product's price, the credit balance for
/// payment option 0 (the lobby sends no new 0x345 after a purchase).
#[test]
fn rom_a_credit_purchase_lowers_the_shown_balance() {
    use dc_client::menu::buybox::{BUTTON_SIZE, CANCEL_AT, WINDOW_AT};
    use dc_client::menu::link::{BuyReply, LinkEvent};
    use dc_client::menu::shop::{CATEGORY_COMMANDS, CMD_BUY};
    use dc_client::menu::storage::TAB_MARKET;
    let Some((mut m, _)) = first_page(u32::MAX) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let cat = (0..CATEGORY_COMMANDS[TAB_MARKET].len())
        .find(|c| {
            wares
                .shelf(&m.data.store, TAB_MARKET, *c)
                .first()
                .is_some_and(|w| w.sale_type == 1)
        })
        .expect("a credit category");
    let price = wares.shelf(&m.data.store, TAB_MARKET, cat)[0].price as u32;
    assert!(price > 0);
    click(&mut m, "shop.market");
    let cmd = CATEGORY_COMMANDS[TAB_MARKET][cat];
    let b = m
        .sheet()
        .parts
        .iter()
        .position(|p| p.kind == WidgetKind::Button && p.command == cmd)
        .unwrap();
    let at = {
        let p = &m.sheet().parts[b];
        [
            (p.pos[0] + p.size[0] / 2) as f32,
            (p.pos[1] + p.size[1] / 2) as f32,
        ]
    };
    m.data.balance = (1_200, 10_000);
    // the category's first cell is selected with the category (as the gift test relies on)
    tap(&mut m, at);
    let buy = {
        let b = m
            .sheet()
            .button(CMD_BUY)
            .filter(|b| b.shown)
            .expect("a shown Buy button");
        [
            (b.pos[0] + b.size[0] / 2) as f32,
            (b.pos[1] + b.size[1] / 2) as f32,
        ]
    };
    tap(&mut m, buy);
    // the purchase window, cancelled (headless: the link ends at its first request); the reply comes in anyway
    tap(
        &mut m,
        [
            (WINDOW_AT[0] + CANCEL_AT[0] + BUTTON_SIZE[0] / 2) as f32,
            (WINDOW_AT[1] + CANCEL_AT[1] + BUTTON_SIZE[1] / 2) as f32,
        ],
    );
    m.feed_event(LinkEvent::Bought(BuyReply {
        ok: true,
        reason: 0,
        uid: 0,
        gp: 0,
        award: -1,
    }));
    m.update();
    assert_eq!(
        m.data.balance,
        (1_200 - price, 10_000),
        "the credit balance paid (option 0)"
    );
    // a refused reply changes nothing
    m.feed_event(LinkEvent::Bought(BuyReply {
        ok: false,
        reason: 0x3b,
        uid: 0,
        gp: 0,
        award: -1,
    }));
    m.update();
    assert_eq!(m.data.balance, (1_200 - price, 10_000));
}
