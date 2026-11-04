//! inbox-clan: the shop screen's inbox page draws the 2009 page: four GiftBox rows (the empty slot art for a row
//! without an entry), the pager, the info box, and the left pane's function item list (type 0x13 items) in place of
//! the weapon rows.
//!
//! ROM-gated (skipped without `CA_ROM`: the rectangles come from the user's layout pages, the item numbers from the
//! user's item table).
// EVIDENCE: cshell+0x14b8a0, cshell+0x148980, cshell+0x149a20, cshell+0x1482b0

use dc_client::hud::items::DrawItem;
use dc_client::menu::inbox::{KEY_SUB_TYPE, SLOT_EMPTY, SLOT_OFF, SLOT_ON};
use dc_client::menu::link::{InboxRow, InvRecord, LinkOptions};
use dc_client::menu::pane::{BODY_ROW_IMAGE, WEAPON_ROW_IMAGES};
use dc_client::menu::shop::{DETAIL_FRAME, PAGE_IDS};
use dc_client::menu::storage::Wares;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
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
        panic!("{name}");
    };
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
}

fn texes(items: &[DrawItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Image { tex, .. } => Some(tex.clone()),
            _ => None,
        })
        .collect()
}

fn texts(items: &[DrawItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn rom_inbox_page_draws_empty_slots_pager_info_box_and_function_items() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let key = (9000..10000)
        .find(|n| wares.art_of(*n).is_some_and(|a| a.sub_type == KEY_SUB_TYPE))
        .expect("a function item");
    // CLI-901: an owned clip option (type 0x13, e.g. an extended magazine) is not a key and is not listed
    let clip = (1000..2000)
        .find(|n| wares.kind_of(*n) == Some(0x13) && wares.label_of(*n) != wares.label_of(key))
        .expect("a clip option");
    m.data.store.items = vec![
        InvRecord {
            uid: 8,
            item_no: clip as u32,
            ..Default::default()
        },
        InvRecord {
            uid: 7,
            item_no: key as u32,
            ..Default::default()
        },
    ];
    m.open(Go::Lobby);
    click(&mut m, "lobby.inbox");
    assert_eq!(m.screen_name(), "shop");
    let items = m.items();
    let t = texes(&items);
    let count = |x: &str| t.iter().filter(|y| y.as_str() == x).count();
    // an empty inbox: four empty slots and the info box
    assert_eq!(count(SLOT_EMPTY), 4);
    assert_eq!(count(DETAIL_FRAME.0), 1);
    // the left pane: the function item row, no weapon rows
    assert_eq!(count(BODY_ROW_IMAGE), 1);
    assert!(WEAPON_ROW_IMAGES.iter().all(|w| count(w) == 0));
    assert!(texts(&items).contains(&wares.label_of(key)));
    assert!(!texts(&items).contains(&wares.label_of(clip)));
    // the pager is up (every number disabled while the inbox is empty)
    for id in PAGE_IDS {
        let b = m
            .sheet()
            .part(-1, id, WidgetKind::Button)
            .expect("page button");
        assert!(b.shown && !b.enabled, "page button {id}");
    }

    // one held row: a slot, three empty
    m.data.store.inbox = vec![InboxRow {
        entry: 1,
        product: 1,
        ..InboxRow::default()
    }];
    m.data.store.inbox_total = 1;
    m.data.store.inbox_fetched = true;
    let t = texes(&m.items());
    let count = |x: &str| t.iter().filter(|y| y.as_str() == x).count();
    assert_eq!(count(SLOT_EMPTY), 3);
    assert_eq!(count(SLOT_OFF) + count(SLOT_ON), 1);
}

/// CLI-763: inbox rows draw their item image, the period mark after the name (`<n>Day`, `1X` + PERMANENT for a
/// product without a period), the received date line, the HiSec slot forms for an entry with an end, and the left
/// pane's function items in item order.
#[test]
fn rom_inbox_rows_draw_image_period_marks_date_and_hisec_forms() {
    use dc_client::menu::inbox::{filetime_text, HISEC_OFF, HISEC_ON};
    use dc_client::menu::shop::PERMANENT_MARK;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let shop = wares.catalog().expect("catalogue");
    // a product with a 30-day period and one without a period
    let ids = wares.catalogue_ids();
    let day30 = *ids
        .iter()
        .find(|p| {
            shop.product(**p as i32)
                .is_some_and(|x| x.period == 30 && x.consume == 0)
        })
        .expect("a 30-day product");
    let permanent = *ids
        .iter()
        .find(|p| shop.product(**p as i32).is_some_and(|x| x.period == 0))
        .expect("a permanent product");
    let received = 134_374_736_000_000_000u64;
    let now = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 11_644_473_600)
        * 10_000_000;
    let row = |entry, product, end| InboxRow {
        entry,
        product,
        received,
        end,
        ..InboxRow::default()
    };
    m.data.store.inbox = vec![
        row(1, day30, 0),
        row(2, permanent, 0),
        row(3, day30, now + 7 * 86_400 * 10_000_000),
    ];
    m.data.store.inbox_total = 3;
    m.data.store.inbox_fetched = true;
    // two function items held in reverse item order
    let keys: Vec<i32> = (9000..10000)
        .filter(|n| wares.art_of(*n).is_some_and(|a| a.sub_type == KEY_SUB_TYPE))
        .take(2)
        .collect();
    m.data.store.items = keys
        .iter()
        .rev()
        .enumerate()
        .map(|(k, n)| InvRecord {
            uid: 7 + k as u32,
            item_no: *n as u32,
            ..Default::default()
        })
        .collect();
    m.open(Go::Lobby);
    click(&mut m, "lobby.inbox");
    let items = m.items();
    let t = texes(&items);
    let has = |x: &str| t.iter().any(|y| y == x);
    assert!(
        has(r"UI_SCR_Page_Shop\Gift\GiftBox_30Day_Mark.dtx"),
        "{t:?}"
    );
    assert!(has(r"UI_SCR_Page_Shop\Gift\GiftBox_1X_Mark.dtx"));
    assert!(has(PERMANENT_MARK.0));
    assert!(has(HISEC_OFF) || has(HISEC_ON), "the row with an end");
    let w = wares.ware(day30, &Default::default()).unwrap();
    assert!(has(w.icon.as_deref().unwrap()), "item image");
    let tx = texts(&items);
    assert!(tx.contains(&filetime_text(received)), "{tx:?}");
    assert_eq!(filetime_text(received), "10/26/2026 07:33:20");
    // the function items in item order
    if keys.len() == 2 {
        let a = tx.iter().position(|s| *s == wares.label_of(keys[0]));
        let b = tx.iter().position(|s| *s == wares.label_of(keys[1]));
        if wares.label_of(keys[0]) != wares.label_of(keys[1]) {
            assert!(a < b, "sorted by item number");
        }
    }
}

/// CLI-833: an inbox row of a HiSec case (type 0x14 whose first `FunctionValue0` is not -1) draws the HiSec mark
/// (cshell+0x69ff0 / 0x69e80); a plain supply case does not.
#[test]
fn rom_inbox_keyed_case_row_draws_the_hisec_mark() {
    use dc_client::menu::inbox::HISEC_MARK;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let ids: Vec<u32> = (0..6000).collect();
    let case_of = |hisec: bool| {
        ids.iter().copied().find(|p| {
            wares
                .ware(*p, &Default::default())
                .filter(|w| w.kind_of == Some(0x14))
                .and_then(|w| wares.art_of(w.item_no).map(|a| a.keyed_case == hisec))
                .unwrap_or(false)
        })
    };
    let (Some(hi), Some(plain)) = (case_of(true), case_of(false)) else {
        eprintln!("skipped: the catalogue has no HiSec / plain case product");
        return;
    };
    let mark_drawn = |m: &mut Menu, product: u32| {
        m.data.store.inbox = vec![InboxRow {
            entry: 1,
            product,
            received: 134_374_736_000_000_000,
            ..InboxRow::default()
        }];
        m.data.store.inbox_total = 1;
        m.data.store.inbox_fetched = true;
        m.open(Go::Lobby);
        click(m, "lobby.inbox");
        texes(&m.items()).iter().any(|t| t == HISEC_MARK.0)
    };
    assert!(mark_drawn(&mut m, hi), "HiSec case row");
    assert!(!mark_drawn(&mut m, plain), "plain case row");
}

/// round 10 (CLI-900): the inbox page's Activate (1027) on a HiSec case row asks for the key (nothing sent without
/// one), then with a key owned asks to open it and Yes sends 0x32e (entry, product); the answer 0x32f removes the
/// row (HISEC_2009.md 4.2 / 4.3, cshell+0x14cc00 / 0x150333).
#[test]
fn rom_inbox_activate_opens_a_hisec_case_with_its_key() {
    use dc_client::menu::link::LinkEvent;
    use dc_client::menu::popup::BoxKind;
    use dc_client::menu::MenuKey;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let hi = (0..6000u32).find(|p| {
        wares
            .ware(*p, &Default::default())
            .filter(|w| w.kind_of == Some(0x14))
            .and_then(|w| {
                wares
                    .art_of(w.item_no)
                    .map(|a| a.keyed_case && wares.key_of(w.item_no).is_some())
            })
            .unwrap_or(false)
    });
    let Some(hi) = hi else {
        eprintln!("skipped: no HiSec case product with a key");
        return;
    };
    let case_no = wares.ware(hi, &Default::default()).unwrap().item_no;
    let key = wares.key_of(case_no).unwrap();
    m.data.store.inbox = vec![InboxRow {
        entry: 77,
        product: hi,
        received: 134_374_736_000_000_000,
        ..InboxRow::default()
    }];
    m.data.store.inbox_total = 1;
    m.data.store.inbox_fetched = true;
    m.open(Go::Lobby);
    click(&mut m, "lobby.inbox");
    let sent = m.link_sent();
    // no key: the key-required box, nothing sent
    m.input(MenuInput::Command(1027));
    let q = m.popup().cloned().expect("key-required box");
    assert_eq!(q.kind, BoxKind::Ok);
    assert!(q.text.contains(&wares.label_of(key)), "{}", q.text);
    // CLI-960: window 10 art, the case and key images, no keys-left line
    let h = q.hisec.clone().expect("window 10");
    assert!(h.bg.contains("Hisec"), "{}", h.bg);
    assert_eq!(h.case.0, wares.art_of(case_no).unwrap().icon);
    assert_eq!(h.key.0, wares.art_of(key).unwrap().icon);
    assert!(h.keys_text.is_none());
    let t = texes(&m.items());
    assert!(t.contains(&h.bg) && t.contains(&h.case.0) && t.contains(&h.key.0));
    assert_eq!(m.link_sent(), sent);
    m.input(MenuInput::Key(MenuKey::Enter));
    // a key owned: the opening box, Yes sends 0x32e
    m.data.store.items = vec![InvRecord {
        uid: 9,
        item_no: key as u32,
        count: 2,
        ..Default::default()
    }];
    m.input(MenuInput::Command(1027));
    let q = m.popup().cloned().expect("opening box");
    assert_eq!(q.kind, BoxKind::YesNo);
    assert_eq!(
        q.hisec
            .as_ref()
            .and_then(|h| h.keys_text.clone())
            .as_deref(),
        Some("^0xffe7ae54^2^ Keys Remain")
    );
    assert!(texts(&m.items()).iter().any(|t| t.contains("Keys Remain")));
    assert_eq!(m.link_sent(), sent);
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("InboxOpen(77, {hi})"));
    // the answer removes the row
    m.feed_event(LinkEvent::InboxOpened(dc_client::menu::link::OpenReply {
        ok: true,
        award_item: case_no as u32,
        key_uid: 9,
        ..Default::default()
    }));
    m.update();
    assert!(m.data.store.inbox.is_empty());
}

fn shown_window(m: &Menu, id: i32) -> bool {
    m.sheet()
        .parts
        .iter()
        .position(|p| p.kind == WidgetKind::Window && p.layer == id)
        .is_some_and(|i| m.sheet().visible(i))
}

/// CLI-1345 (playtest report: "activated inbox items just say M9 no matter the item"): opening a bought product (not
/// a supply case) answers 0x32f with its item count and unique ids, +0x12 = 0; the client named item 0 (the M9) in an
/// award pop-up. 2009 shows no pop-up for a product: the item joins the inventory and the storage page of its tab and
/// category opens with the new item selected (cshell+0x14e010 product branch, cshell+0x14c7e0).
#[test]
fn rom_inbox_activate_of_a_product_opens_its_storage_page_not_an_m9_box() {
    use dc_client::menu::link::LinkEvent;
    use dc_client::menu::shop::{INBOX_WINDOW, STORAGE_WINDOW};
    use dc_client::menu::storage::{shelf_of, TAB_WEAPONS};
    use dc_client::menu::MenuKey;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    // a credit primary-weapon product (the black market's weapons), not a case
    let Some(pid) = (0..6000u32).find(|p| {
        wares.ware(*p, &Default::default()).is_some_and(|w| {
            w.sale_type == 1
                && w.kind_of.and_then(shelf_of) == Some((TAB_WEAPONS, 0))
                && w.item_no > 0
        })
    }) else {
        eprintln!("skipped: no credit primary product");
        return;
    };
    let item_no = wares.ware(pid, &Default::default()).unwrap().item_no;
    m.data.store.inbox = vec![InboxRow {
        entry: 41,
        product: pid,
        received: 134_374_736_000_000_000,
        ..InboxRow::default()
    }];
    m.data.store.inbox_total = 1;
    m.data.store.inbox_fetched = true;
    m.open(Go::Lobby);
    click(&mut m, "lobby.inbox");
    assert!(shown_window(&m, INBOX_WINDOW));
    m.input(MenuInput::Command(1027));
    assert!(m.popup().is_some(), "the generic open question");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("InboxOpen(41, {pid})"));
    m.update();
    let boxes = m.popup_count();
    // the product answer: +0x12 is 0, the new unique id at +0x22
    m.feed_event(LinkEvent::InboxOpened(dc_client::menu::link::OpenReply {
        ok: true,
        award_item: 0,
        award_uid: 555,
        ..Default::default()
    }));
    m.update();
    assert!(m.data.store.inbox.is_empty());
    // the old client queued a case-award box naming item 0 (the M9); 2009 shows none for a product
    assert_eq!(
        m.popup_count(),
        boxes,
        "a box named item 0 ({})",
        wares.label_of(0)
    );
    assert_eq!(m.link_latest_request(), "Inventory");
    assert!(shown_window(&m, STORAGE_WINDOW), "the storage page opens");
    // the inventory reread holds the new item after eight others of another weapon: page 2, its third cell, selected
    // (its name on the cell and on the detail panel)
    let other = (1..2000)
        .find(|n| {
            *n != item_no
                && wares.kind_of(*n).and_then(shelf_of) == Some((TAB_WEAPONS, 0))
                && wares.label_of(*n) != wares.label_of(item_no)
        })
        .expect("a second primary");
    let rec = |uid, n: i32| InvRecord {
        uid,
        item_no: n as u32,
        slot_kind: 1,
        ..Default::default()
    };
    let mut items: Vec<InvRecord> = (0..8u32).map(|k| rec(100 + k, other)).collect();
    items.push(rec(555, item_no));
    m.feed_event(LinkEvent::Inventory(items));
    m.update();
    assert!(shown_window(&m, STORAGE_WINDOW));
    let name = wares.label_of(item_no);
    let shown = texts(&m.items()).iter().filter(|t| **t == name).count();
    assert_eq!(shown, 2, "{name}: cell + detail panel");
}

/// CLI-1430: an opened option item goes to the Modification screen: the tab of its weapon class, the option
/// storage of its kind, the new option picked (cshell+0x14c7e0 -> cshell+0x1596f0).
#[test]
fn rom_inbox_activate_of_an_option_opens_the_modification_storage_with_it_picked() {
    use dc_client::menu::link::LinkEvent;
    use dc_client::menu::tuning::{class_tab, CMD_TABS, STORAGE_WINDOW};
    use dc_client::menu::MenuKey;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    // a muzzle product of the SMG class (class 1: the fourth tab, so the class switch is exercised)
    let Some(pid) = (0..6000u32).find(|p| {
        wares.ware(*p, &Default::default()).is_some_and(|w| {
            w.kind_of == Some(0x11)
                && wares.art_of(w.item_no as i32).and_then(|a| a.attach_class) == Some(1)
        })
    }) else {
        eprintln!("skipped: no SMG muzzle product");
        return;
    };
    let opt_no = wares.ware(pid, &Default::default()).unwrap().item_no as u32;
    let weapon = (0..1000u32)
        .find(|n| wares.kind_of(*n as i32) == Some(1))
        .expect("an SMG");
    m.data.store.inbox = vec![InboxRow {
        entry: 42,
        product: pid,
        received: 134_374_736_000_000_000,
        ..InboxRow::default()
    }];
    m.data.store.inbox_total = 1;
    m.data.store.inbox_fetched = true;
    // the screen change's focus requests (CLI-1561) come after the reread: the requests are read from a detached link
    m.detach_link();
    m.open(Go::Lobby);
    click(&mut m, "lobby.inbox");
    m.input(MenuInput::Command(1027));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("InboxOpen(42, {pid})"));
    m.update();
    m.feed_event(LinkEvent::InboxOpened(dc_client::menu::link::OpenReply {
        ok: true,
        award_uid: 777,
        ..Default::default()
    }));
    m.update();
    assert_eq!(m.screen_name(), "tuning", "the Modification screen");
    let sent = m.take_link_sent();
    assert!(sent.contains(&"Inventory".to_string()), "{sent:?}");
    // the reread brings the option (attached to the SMG, so Equip reads Detach whatever the fit)
    let rec = |uid, item_no, parent_uid| InvRecord {
        uid,
        item_no,
        parent_uid,
        ..Default::default()
    };
    m.feed_event(LinkEvent::Inventory(vec![
        rec(10, weapon, 0),
        rec(700, opt_no, 0),
        rec(777, opt_no, 10),
    ]));
    m.update();
    assert!(shown_window(&m, STORAGE_WINDOW), "the option storage");
    assert_eq!(class_tab(1), 3);
    assert!(
        m.sheet().button(CMD_TABS[3]).is_some_and(|b| b.selected),
        "the SMG tab"
    );
    // the picked option is the new, attached one (uid 777): Equip reads Detach (the loose uid 700 would read Attach)
    let detach = m.cat.text(0x1565);
    let attach = m.cat.text(0x1564);
    let t = texts(&m.items());
    assert!(
        t.contains(&detach) && !t.contains(&attach),
        "the new option is the picked one"
    );
}

/// CLI-1433: opening a product with bonus GP (a mercenary's 7-day product: +10,000 GP) adds the bonus to the shown GP
/// (cshell+0x14e010); the opened character goes to the body tab's character page (cshell+0x14c7e0).
#[test]
fn rom_inbox_activate_of_a_mercenary_adds_its_bonus_gp() {
    use dc_client::menu::link::LinkEvent;
    use dc_client::menu::MenuKey;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let Some(cat) = wares.catalog() else {
        eprintln!("skipped: no shop list");
        return;
    };
    let Some((pid, bonus)) = (0..6000u32).find_map(|p| {
        let w = wares.ware(p, &Default::default())?;
        let b = cat.product(p as i32)?.bonus;
        ((10000..11000).contains(&w.item_no) && b > 0).then_some((p, b as u32))
    }) else {
        eprintln!("skipped: no mercenary product with bonus GP");
        return;
    };
    m.data.profile.gp = 1000;
    m.data.store.inbox = vec![InboxRow {
        entry: 43,
        product: pid,
        received: 134_374_736_000_000_000,
        ..InboxRow::default()
    }];
    m.data.store.inbox_total = 1;
    m.data.store.inbox_fetched = true;
    m.open(Go::Lobby);
    click(&mut m, "lobby.inbox");
    m.input(MenuInput::Command(1027));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("InboxOpen(43, {pid})"));
    m.update();
    m.feed_event(LinkEvent::InboxOpened(dc_client::menu::link::OpenReply {
        ok: true,
        award_uid: 888,
        ..Default::default()
    }));
    m.update();
    assert_eq!(m.data.profile.gp, 1000 + bonus);
}

/// crate-result-window: opening a supply crate from the inbox shows the 2009 award window: the closed-crate art and the
/// title for one second, then the opened art with the awarded gun's slot picture, its name and class tag, the
/// "<name> / <period>" line (1 day, 7 days, permanent from the ROM's own forms) and the OK button
/// (cshell+0x286500 / 0x286d40 / 0x285a80).
#[test]
fn rom_inbox_opened_crate_shows_the_award_window_with_picture_and_period() {
    use dc_client::menu::award::{self, Variant};
    use dc_client::menu::link::LinkEvent;
    use dc_client::menu::MenuKey;
    use std::time::{Duration, Instant};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.detach_link();
    if !m.cat.paths.award.complete() {
        eprintln!("skipped: the ROM's shell lacks the award window literals");
        return;
    }
    let wares = Wares::cached(m.cat.rom.as_deref());
    let catalog = wares.catalog().expect("catalogue");
    // a supply crate (consume type 6) if the catalogue sells one, else a plain supply case without a key
    let cases: Vec<(u32, i32)> = (0..0x10000u32)
        .filter_map(|p| {
            let w = wares.ware(p, &Default::default())?;
            let keyed = wares.art_of(w.item_no).is_some_and(|a| a.keyed_case);
            (w.kind_of == Some(0x14) && !keyed).then_some((p, w.consume))
        })
        .collect();
    eprintln!("case products (product, consume): {cases:?}");
    let crate_product = cases.iter().find(|c| c.1 == 6).or(cases.first()).copied();
    let Some((crate_product, crate_consume)) = crate_product else {
        eprintln!("skipped: no supply case product");
        return;
    };
    // a gun with a slot picture and a class tag, its product counted in days
    let gun = (0..1000)
        .find(|n| {
            wares.kind_of(*n).is_some_and(|k| (0..=4).contains(&k))
                && !wares.icons_of(*n).is_empty()
                && catalog
                    .first_product_of_item(*n)
                    .is_some_and(|p| !matches!(p.consume, 2 | 3 | 6))
        })
        .expect("a gun");
    let name = wares.label_of(gun);
    let mut texts_by_days = Vec::new();
    for (round, days) in [7u32, 1, 0].into_iter().enumerate() {
        m.data.store.inbox = vec![InboxRow {
            entry: 50 + round as u32,
            product: crate_product,
            received: 134_374_736_000_000_000,
            ..InboxRow::default()
        }];
        m.data.store.inbox_total = 1;
        m.data.store.inbox_fetched = true;
        m.open(Go::Lobby);
        click(&mut m, "lobby.inbox");
        m.input(MenuInput::Command(1027));
        assert!(m.popup().is_some(), "the open question");
        m.input(MenuInput::Key(MenuKey::Enter));
        m.feed_event(LinkEvent::InboxOpened(dc_client::menu::link::OpenReply {
            ok: true,
            award_item: gun as u32,
            days,
            award_uid: 4242,
            ..Default::default()
        }));
        m.update();
        let ended = m
            .cat
            .text(dc_client::menu::popup::text_ids::CONNECTION_ENDED);
        assert!(m.popup().is_some_and(|p| p.text != ended), "detached link");
        let q = m.popup().cloned().expect("award window");
        let a = q
            .award
            .clone()
            .unwrap_or_else(|| panic!("plain box {:?} {:?}", q.title, q.text));
        let want = if crate_consume == 6 {
            Variant::Crate
        } else {
            Variant::Case
        };
        assert_eq!(a.variant, want);
        assert_eq!(a.item, gun);
        assert_eq!(a.name, name);
        assert_eq!(
            a.picture.as_deref(),
            wares.icons_of(gun).first().map(|s| s.as_str())
        );
        assert!(a.tag.is_some(), "class tag of a gun");
        assert!(a.period.starts_with(&name), "{}", a.period);
        texts_by_days.push(a.period.clone());
        // the first second: closed art, title, no picture; OK does nothing
        let items = m.items();
        let t = texes(&items);
        assert!(
            t.contains(&a.closed_art) && !t.contains(&a.opened_art),
            "{t:?}"
        );
        assert!(!t.contains(a.picture.as_ref().unwrap()));
        m.input(MenuInput::Key(MenuKey::Enter));
        assert!(
            m.popup().is_some_and(|p| p.award.is_some()),
            "OK disabled before the reveal"
        );
        // after the second: the opened art, the slot frame, the picture, the name and the period line
        m.tick_popup_at(Instant::now() + Duration::from_millis(award::REVEAL_MS + 10));
        let items = m.items();
        let t = texes(&items);
        assert!(t.contains(&a.opened_art) && t.contains(&a.slot), "{t:?}");
        assert!(t.contains(a.picture.as_ref().unwrap()), "{t:?}");
        let s = texts(&items);
        assert!(s.contains(&name) && s.contains(&a.period), "{s:?}");
        if round == 0 {
            if let Some(dir) = std::env::var_os("DC_AWARD_SHOT") {
                let i = m.image();
                let f = dc_render::FrameImage {
                    width: i.width,
                    height: i.height,
                    rgba: i.rgba,
                };
                let _ = f.save_png(&std::path::Path::new(&dir).join("crate_result.png"));
            }
        }
        m.input(MenuInput::Key(MenuKey::Enter));
        assert!(m.popup().is_none(), "OK closes the window");
    }
    // 7 days, 1 day and permanent read differently, each with the number where it has one
    assert!(texts_by_days[0].contains('7'), "{texts_by_days:?}");
    assert!(texts_by_days[1].contains('1'), "{texts_by_days:?}");
    assert_ne!(texts_by_days[0], texts_by_days[1]);
    assert_ne!(texts_by_days[1], texts_by_days[2]);
    eprintln!("award periods: {texts_by_days:?}");
}
