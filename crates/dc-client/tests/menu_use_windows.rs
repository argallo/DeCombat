//! use-windows (CLI-1370..): the function-item windows beyond the rename window, opened from an opened inbox product
//! and from Equip (265) on the storage cell: the weapon change's question (SubType 12) and the record reset window
//! (SubType 5 player, 3 clan): Cancel, the 0x330 request with its payload, a refusal box, the success box and the
//! local update.
//!
//! ROM-gated (skipped without `CA_ROM`: the item table, the strings and the layout sections come from the user's ROM).
// SPEC: PROTOCOL_2009.md §4.3 (0x330, 0x331 window table), §5.1 (0x3c2 / 0x3c3)
// EVIDENCE: cshell+0x2450e0 / 0x2452f0 (weapon change), cshell+0x2427e0 / 0x244c00 / 0x244cf0 (record window)

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{
    ClanInfoView, InboxRow, InvRecord, LinkEvent, LinkOptions, OpenReply, UseReply,
};
use dc_client::menu::shop::CMD_WEAR;
use dc_client::menu::storage::Wares;
use dc_client::menu::usebox::{NOTICE_TITLE, WEAPON_CHANGE_ASK};
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};
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

fn click(m: &mut Menu, name: &str) {
    let Resolved::Part { at, .. } = resolve(m, name) else {
        panic!("{name}");
    };
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
}

fn texts(m: &mut Menu) -> Vec<String> {
    m.items()
        .iter()
        .filter_map(|i| match i {
            DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The window is up: its warning's first wrapped line is drawn.
fn up(m: &mut Menu, warning: &str) -> bool {
    texts(m)
        .iter()
        .any(|t| t.len() > 10 && warning.starts_with(t.as_str()))
}

fn settle(m: &mut Menu) {
    m.update();
    let ended = m
        .cat
        .text(dc_client::menu::popup::text_ids::CONNECTION_ENDED);
    while m.popup().is_some_and(|p| p.text == ended) {
        m.input(MenuInput::Key(MenuKey::Enter));
    }
}

fn shot(m: &mut Menu, name: &str) {
    let Ok(dir) = std::env::var("DC_USEBOX_PNG") else {
        return;
    };
    let i = m.image();
    let f = dc_render::FrameImage {
        width: i.width,
        height: i.height,
        rgba: i.rgba,
    };
    let _ = f.save_png(&std::path::Path::new(&dir).join(format!("{name}.png")));
}

/// A menu on the storage page with function item `item` (unique id `uid`) opened from the inbox; `None` without
/// the ROM or a product of the item.
fn opened(item: i32, uid: u32, prep: impl FnOnce(&mut Menu)) -> Option<Menu> {
    let mut m = menu()?;
    let wares = Wares::cached(m.cat.rom.as_deref());
    let pid = (0..6000u32).find(|p| {
        wares
            .ware(*p, &Default::default())
            .is_some_and(|w| w.item_no == item)
    })?;
    m.detach_link();
    prep(&mut m);
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
    m.input(MenuInput::Command(1027));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("InboxOpen(41, {pid})"));
    settle(&mut m);
    m.feed_event(LinkEvent::InboxOpened(OpenReply {
        ok: true,
        award_uid: uid,
        ..Default::default()
    }));
    settle(&mut m);
    m.feed_event(LinkEvent::Inventory(vec![InvRecord {
        uid,
        item_no: item as u32,
        ..Default::default()
    }]));
    settle(&mut m);
    Some(m)
}

fn cancelled(m: &mut Menu) {
    let q = m.popup().cloned().expect("cancelled box");
    assert_eq!((q.text, q.title), (m.cat.text(0x1807), 0x1811));
    m.input(MenuInput::Key(MenuKey::Enter));
}

#[test]
fn rom_weapon_change_asks_then_sends_the_change_item_and_converts_the_item() {
    const ITEM: i32 = 9015;
    let Some(rom) = dc_rom::resolve_rom().ok() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(Some(&rom));
    let Some(art) = wares.art_of(ITEM).cloned() else {
        eprintln!("skipped: no item {ITEM}");
        return;
    };
    assert_eq!(art.sub_type, 12, "item {ITEM} is a weapon change");
    assert!(art.change_item > 0, "its ChangeItemID");
    let change = art.change_item;
    let Some(mut m) = opened(ITEM, 600, |_| {}) else {
        eprintln!("skipped: no weapon change product");
        return;
    };
    // the question: the item's name and the change weapon's slot word, titled Notice, OK / Cancel
    let q = m.popup().cloned().expect("the 0x1876 question");
    assert_eq!(q.title, NOTICE_TITLE);
    assert!(q.text.contains(&wares.label_of(ITEM)), "{}", q.text);
    let form = m.cat.text(WEAPON_CHANGE_ASK);
    let head = form.split("%s").next().unwrap_or("");
    assert!(q.text.starts_with(head), "{}", q.text);
    assert!(!q.text.contains("%s"));
    shot(&mut m, "weapon_change_ask");
    // Cancel (the second button): the cancelled box, nothing sent
    let before = m.link_latest_request();
    m.input(MenuInput::Key(MenuKey::Escape));
    cancelled(&mut m);
    assert_eq!(m.link_latest_request(), before);
    // Equip on the cell asks again; OK sends 0x330 with the change item
    m.input(MenuInput::Command(CMD_WEAR));
    assert!(m.popup().is_some_and(|p| p.title == NOTICE_TITLE));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(
        m.link_latest_request(),
        format!("ItemUse(600, \"\", {change})")
    );
    // refused: the reason box
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: false,
        reason: 0x58,
        uid: 600,
        ..Default::default()
    }));
    settle(&mut m);
    assert_eq!(m.popup().map(|p| p.text.clone()), Some(m.cat.text(0x17f8)));
    m.input(MenuInput::Key(MenuKey::Enter));
    // again, ok with the replaced default 77: the success box, the item is the change weapon, 77 is gone
    m.data.store.items.push(InvRecord {
        uid: 77,
        item_no: change as u32,
        ..Default::default()
    });
    m.input(MenuInput::Command(CMD_WEAR));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(
        m.link_latest_request(),
        format!("ItemUse(600, \"\", {change})")
    );
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: true,
        uid: 600,
        removed: 77,
        ..Default::default()
    }));
    settle(&mut m);
    let q = m.popup().cloned().expect("success box");
    assert_eq!(q.title, NOTICE_TITLE);
    assert!(q.text.contains(&wares.label_of(change)), "{}", q.text);
    shot(&mut m, "weapon_changed");
    assert_eq!(m.link_latest_request(), "Inventory");
    assert!(!m.data.store.items.iter().any(|r| r.uid == 77));
    assert_eq!(
        m.data
            .store
            .items
            .iter()
            .find(|r| r.uid == 600)
            .map(|r| r.item_no),
        Some(change as u32)
    );
}

#[test]
fn rom_record_reset_shows_the_record_asks_and_clears_it() {
    const ITEM: i32 = 9005;
    let Some(mut m) = opened(ITEM, 700, |m| {
        m.data.char_extras.kills = 321;
        m.data.char_extras.deaths = 123;
        m.data.char_extras.headshots = 45;
    }) else {
        eprintln!("skipped: no CA_ROM or no record reset product");
        return;
    };
    let rec = texts(&mut m).join("|");
    assert!(
        rec.contains("321") && rec.contains("123") && rec.contains("45"),
        "{rec}"
    );
    let title = m.cat.text(6089);
    assert!(up(&mut m, &title), "the window's title");
    shot(&mut m, "record_open");
    // Cancel: the cancelled box, the window closes
    m.input(MenuInput::Key(MenuKey::Escape));
    cancelled(&mut m);
    assert!(!up(&mut m, &title));
    // Equip reopens; OK asks first, the check box's Cancel keeps the window and sends nothing
    m.input(MenuInput::Command(CMD_WEAR));
    assert!(up(&mut m, &title));
    let before = m.link_latest_request();
    m.input(MenuInput::Key(MenuKey::Enter));
    let q = m.popup().cloned().expect("the check box");
    assert_eq!(q.title, 6172);
    m.input(MenuInput::Key(MenuKey::Escape));
    assert_eq!(m.link_latest_request(), before);
    assert!(up(&mut m, &title));
    // OK, OK: 0x330 with the unique id alone; refused -> the reason box, the window stays
    m.input(MenuInput::Key(MenuKey::Enter));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), "ItemUse(700, \"\", 0)");
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: false,
        reason: 0x5f,
        uid: 700,
        ..Default::default()
    }));
    settle(&mut m);
    assert_eq!(m.popup().map(|p| p.text.clone()), Some(m.cat.text(0x1800)));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(up(&mut m, &title));
    // again and ok: the success box, the record cleared, the item gone, the window closed
    m.input(MenuInput::Key(MenuKey::Enter));
    m.input(MenuInput::Key(MenuKey::Enter));
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: true,
        uid: 700,
        ..Default::default()
    }));
    settle(&mut m);
    let q = m.popup().cloned().expect("success box");
    assert_eq!((q.text.clone(), q.title), (m.cat.text(6164), 6165));
    let x = &m.data.char_extras;
    assert_eq!((x.kills, x.deaths, x.headshots), (0, 0, 0));
    assert!(!m.data.store.items.iter().any(|r| r.uid == 700));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(!up(&mut m, &title));
}

#[test]
fn rom_clan_record_reset_asks_for_the_clan_record() {
    const ITEM: i32 = 9003;
    let Some(mut m) = opened(ITEM, 701, |_| {}) else {
        eprintln!("skipped: no CA_ROM or no clan record reset product");
        return;
    };
    assert_eq!(m.link_latest_request(), "ClanInfo");
    let title = m.cat.text(6091);
    assert!(up(&mut m, &title), "the clan window");
    m.feed_event(LinkEvent::ClanInfo(ClanInfoView {
        wins: 31,
        losses: 17,
        draws: 9,
        ..Default::default()
    }));
    settle(&mut m);
    let rec = texts(&mut m).join("|");
    assert!(
        rec.contains("31") && rec.contains("17") && rec.contains("9"),
        "{rec}"
    );
    // ok: the clan success box, the item gone
    m.input(MenuInput::Key(MenuKey::Enter));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), "ItemUse(701, \"\", 0)");
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: true,
        uid: 701,
        ..Default::default()
    }));
    settle(&mut m);
    assert_eq!(m.popup().map(|p| p.title), Some(6166));
    assert!(!m.data.store.items.iter().any(|r| r.uid == 701));
}
