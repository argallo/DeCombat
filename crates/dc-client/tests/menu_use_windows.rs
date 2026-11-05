//! use-windows (CLI-1370..): the function-item windows beyond the rename window, opened from an opened inbox product
//! and from Equip (265) on the storage cell: the weapon change's question (SubType 12) and the record reset window
//! (SubType 5 player, 3 clan): Cancel, the 0x330 request with its payload, a refusal box, the success box and the
//! local update.
//!
//! ROM-gated (skipped without `CA_ROM`: the item table, the strings and the layout sections come from the user's ROM).
// SPEC: PROTOCOL_2009.md §4.3 (0x330, 0x331 window table), §5.1 (0x3c2 / 0x3c3)
// EVIDENCE: cshell+0x2450e0 / 0x2452f0 (weapon change), cshell+0x2427e0 / 0x244c00 / 0x244cf0 (record window)

use dc_client::hud::items::DrawItem;
use dc_client::menu::clan::ClanLits;
use dc_client::menu::colorbox::{self, ColourLayout, ColourTables, ColourTexts};
use dc_client::menu::countrybox::{CountryLayout, ROW_HEIGHT};
use dc_client::menu::facebox::{self, FaceLayout, FaceTexts};
use dc_client::menu::link::QueueReply;
use dc_client::menu::link::{
    ClanInfoView, InboxRow, InvRecord, LinkEvent, LinkOptions, OpenReply, UseReply,
};
use dc_client::menu::markbox::{self, MarkLayout};
use dc_client::menu::megabox::{self, MegaBox};
use dc_client::menu::shop::CMD_WEAR;
use dc_client::menu::storage::Wares;
use dc_client::menu::usebox;
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
    opened_with(item, uid, prep, Vec::new())
}

/// [`opened`] with `extra` inventory records next to the opened one (the inventory answer holds them too).
fn opened_with(
    item: i32,
    uid: u32,
    prep: impl FnOnce(&mut Menu),
    extra: Vec<InvRecord>,
) -> Option<Menu> {
    let mut m = menu()?;
    let wares = Wares::cached(m.cat.rom.as_deref());
    let pid = (0..6000u32).find(|p| {
        wares
            .ware(*p, &Default::default())
            .is_some_and(|w| w.item_no == item)
    })?;
    m.detach_link();
    prep(&mut m);
    m.data.store.items.extend(extra.iter().cloned());
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
    let mut inv = vec![InvRecord {
        uid,
        item_no: item as u32,
        ..Default::default()
    }];
    inv.extend(extra);
    m.feed_event(LinkEvent::Inventory(inv));
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
        format!("ItemUse(600, \"\", {change}, 0)")
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
        format!("ItemUse(600, \"\", {change}, 0)")
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
    assert_eq!(m.link_latest_request(), "ItemUse(700, \"\", 0, 0)");
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
    assert_eq!(m.link_latest_request(), "ItemUse(701, \"\", 0, 0)");
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: true,
        uid: 701,
        ..Default::default()
    }));
    settle(&mut m);
    assert_eq!(m.popup().map(|p| p.title), Some(6166));
    assert!(!m.data.store.items.iter().any(|r| r.uid == 701));
}

/// The first function item of `SubType` `sub` in the ROM item table.
fn item_of(sub: i32) -> Option<i32> {
    let rom = dc_rom::resolve_rom().ok()?;
    let wares = Wares::cached(Some(&rom));
    (9000..10000).find(|&n| wares.art_of(n).is_some_and(|a| a.sub_type == sub))
}

fn press(m: &mut Menu, at: [i32; 2]) {
    let p = [(at[0] + 5) as f32, (at[1] + 5) as f32];
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

/// use-items-emblem (CLI-1884): the clan emblem item opens the mark window: Next wants an emblem, Check a background,
/// an ok check enables OK, the confirmation's OK sends the pair and the answer makes it the clan's mark.
#[test]
fn rom_clan_mark_window_checks_then_sends_the_pair() {
    let Some(item) = item_of(markbox::SUB_CLAN_MARK) else {
        eprintln!("skipped: no CA_ROM or no clan emblem item");
        return;
    };
    let Some(mut m) = opened(item, 702, |_| {}) else {
        eprintln!("skipped: no clan emblem product");
        return;
    };
    let title = m.cat.text(markbox::TITLE_TEXT);
    assert!(texts(&mut m).contains(&title), "the mark window");
    let l = MarkLayout::read(&m.cat);
    let w = l.window_at();
    let at = |p: [i32; 2]| [w[0] + p[0], w[1] + p[1]];
    let marks = ClanLits::from_rom(m.cat.rom.as_deref().unwrap()).marks;
    let first = |cats: &[i32]| cats.iter().position(|&c| c >= 1).unwrap() as u32;
    let (e, g) = (first(&marks.emblem_cats), first(&marks.background_cats));
    // Next without an emblem: the box titled 0x346
    m.input(MenuInput::Key(MenuKey::Enter));
    let q = m.popup().cloned().expect("no emblem box");
    assert_eq!(
        (q.text, q.title),
        (m.cat.text(markbox::NO_EMBLEM_TEXT), 0x346)
    );
    m.input(MenuInput::Key(MenuKey::Enter));
    press(&mut m, at(l.list_at));
    shot(&mut m, "mark_emblem");
    m.input(MenuInput::Key(MenuKey::Enter));
    press(&mut m, at(l.list_at));
    let before = m.link_latest_request();
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("MarkCheck({e}, {g})"));
    assert_ne!(before, m.link_latest_request());
    m.feed_event(LinkEvent::MarkChecked {
        ok: true,
        reason: 0,
    });
    settle(&mut m);
    let q = m.popup().cloned().expect("check box");
    assert_eq!(q.title, markbox::CHECKED_TITLE);
    m.input(MenuInput::Key(MenuKey::Enter));
    shot(&mut m, "mark_background");
    // OK: the confirmation; its OK sends
    m.input(MenuInput::Key(MenuKey::Enter));
    shot(&mut m, "mark_confirm");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("MarkUse(702, {e}, {g})"));
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: true,
        uid: 702,
        ..Default::default()
    }));
    settle(&mut m);
    assert_eq!(m.popup().map(|p| p.title), Some(markbox::DONE_TITLE));
    let x = &m.data.char_extras;
    assert_eq!((x.clan_mark, x.clan_mark_ground), (e, g));
    assert!(m.data.store.items.iter().any(|r| r.uid == 702 && r.in_use));
}

/// use-items-emblem (CLI-1885): the base character item lists the own gender's other characters, OK confirms, the
/// confirmation's OK sends 6000 + the picked face and the answer sets the own face; Cancel shows the cancelled box.
#[test]
fn rom_face_window_confirms_then_sends_the_face() {
    let Some(item) = item_of(facebox::SUB_FACE) else {
        eprintln!("skipped: no CA_ROM or no base character item");
        return;
    };
    let Some(mut m) = opened(item, 703, |m| m.data.profile.face = 6002) else {
        eprintln!("skipped: no base character product");
        return;
    };
    let texts_of = FaceTexts::read(&m.cat, facebox::SUB_FACE);
    let title = m.cat.text(texts_of.title);
    assert!(texts(&mut m).contains(&title), "the face window");
    let rows = facebox::rows_from_rom(m.cat.rom.as_deref());
    let list = facebox::choices(&rows, facebox::SUB_FACE, 2);
    let l = FaceLayout::read(&m.cat);
    let w = l.window_at();
    press(&mut m, [w[0] + l.list_at[0], w[1] + l.list_at[1]]);
    shot(&mut m, "face_pick");
    m.input(MenuInput::Key(MenuKey::Enter));
    shot(&mut m, "face_confirm");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(
        m.link_latest_request(),
        format!("FaceUse(703, {})", 6000 + list[0].face)
    );
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: true,
        uid: 703,
        ..Default::default()
    }));
    settle(&mut m);
    assert_eq!(m.data.profile.face, 6000 + list[0].face);
    assert!(!m.data.store.items.iter().any(|r| r.uid == 703));
    // the gender item: Escape cancels
    let Some(item) = item_of(facebox::SUB_GENDER) else {
        return;
    };
    let Some(mut m) = opened(item, 704, |m| m.data.profile.face = 6002) else {
        return;
    };
    let title = m
        .cat
        .text(FaceTexts::read(&m.cat, facebox::SUB_GENDER).title);
    assert!(texts(&mut m).contains(&title), "the gender window");
    shot(&mut m, "gender");
    m.input(MenuInput::Key(MenuKey::Escape));
    cancelled(&mut m);
}

/// The pictures drawn, in order.
fn pictures(m: &mut Menu) -> Vec<(String, [f32; 4], [u8; 4])> {
    m.items()
        .iter()
        .filter_map(|i| match i {
            DrawItem::Image { tex, dst, tint, .. } => Some((tex.clone(), *dst, *tint)),
            _ => None,
        })
        .collect()
}

fn labels(m: &mut Menu) -> Vec<(String, [u8; 4])> {
    m.items()
        .iter()
        .filter_map(|i| match i {
            DrawItem::Label { text, style, .. } => Some((text.clone(), style.colour)),
            _ => None,
        })
        .collect()
}

/// colour-window-2009 (CLI-1876, owner report 2026-11-05 "the custom reticle screen still doesn't look exactly the
/// same"): the ring reticle's window on the ROM layout: the reticle colour table from its last entry in one row at
/// the reticle list's point, this item's own picture as "before" in green, the page links 2..5 and both arrows
/// disabled; a pick marks its slot and shows "after"; the confirmation's question is one line; the use enters the
/// item in a function slot, the match takes its shape and colour, and a reticle taken out of its slot (Unequip) no
/// longer counts (CLI-1878). PNGs with `DC_USEBOX_PNG` set.
#[test]
fn rom_reticle_window_draws_the_2009_page_and_its_unequip_ends_it() {
    const RING: i32 = 9014;
    let Some(rom) = dc_rom::resolve_rom().ok() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(Some(&rom));
    let Some(art) = wares.art_of(RING).cloned() else {
        eprintln!("skipped: no item {RING}");
        return;
    };
    assert_eq!((art.sub_type, art.shape), (colorbox::SUB_RETICLE, 4));
    // point 3: reticles may be taken out, the name and clan colours not (the ROM rows' IsCanUnEquip)
    assert!(wares.can_unequip(RING));
    for n in [9008, 9009] {
        if wares.art_of(n).is_some() {
            assert!(!wares.can_unequip(n), "{n}");
        }
    }
    let Some(mut m) = opened(RING, 801, |_| {}) else {
        eprintln!("skipped: no reticle product");
        return;
    };
    let texts_of = ColourTexts::read(&m.cat, colorbox::SUB_RETICLE);
    let title = m.cat.text(texts_of.title);
    assert!(texts(&mut m).contains(&title), "the reticle window");
    let tables = ColourTables::from_rom(Some(&rom));
    let swatches: Vec<String> = tables.reticles.iter().map(|s| s.tex.clone()).collect();
    assert_eq!(swatches.len(), 8);
    let l = ColourLayout::read(&m.cat);
    let w = l.window_at();
    let slot = |k: i32| {
        [
            w[0] + l.reticle_list_at[0] + k * (l.slot_size[0] + l.column_gap),
            w[1] + l.reticle_list_at[1],
        ]
    };
    let pics = pictures(&mut m);
    let shown: Vec<&(String, [f32; 4], [u8; 4])> =
        pics.iter().filter(|p| swatches.contains(&p.0)).collect();
    let want: Vec<String> = swatches.iter().rev().cloned().collect();
    assert_eq!(
        shown.iter().map(|p| p.0.clone()).collect::<Vec<_>>(),
        want,
        "the table from its last colour"
    );
    for (k, p) in shown.iter().enumerate() {
        let at = slot(k as i32);
        assert_eq!([p.1[0], p.1[1]], [at[0] as f32, at[1] as f32], "slot {k}");
    }
    let before = pics
        .iter()
        .find(|p| p.0 == art.preview_icon)
        .expect("this item's picture as before");
    assert_eq!(before.2, [0, 255, 0, 255]);
    assert_eq!(
        [before.1[0], before.1[1]],
        [
            (w[0] + l.before_at[0]) as f32,
            (w[1] + l.before_at[1]) as f32
        ]
    );
    let ls = labels(&mut m);
    // the window draws over the storage page (its own page numbers): the last label of a text
    let colour_of = |t: &str| ls.iter().rev().find(|(s, _)| s == t).map(|(_, c)| *c);
    assert_eq!(colour_of("1"), Some([255, 255, 255, 255]));
    for t in ["<", "2", "5", ">"] {
        assert_eq!(colour_of(t), Some([0x66, 0x66, 0x66, 255]), "{t}");
    }
    shot(&mut m, "reticle_window");
    // a pick on the first slot: the table's last colour, the after picture in it
    press(&mut m, slot(0));
    let after = pictures(&mut m)
        .into_iter()
        .rfind(|p| p.0 == art.preview_icon)
        .expect("after");
    let last = tables.reticles.last().unwrap().argb;
    assert_eq!(
        after.2,
        [(last >> 16) as u8, (last >> 8) as u8, last as u8, 255]
    );
    shot(&mut m, "reticle_window_picked");
    // OK: the confirmation, its question one label
    m.input(MenuInput::Key(MenuKey::Enter));
    let notice = m.cat.text(texts_of.confirm_notice);
    assert!(texts(&mut m).contains(&notice), "the question on one line");
    shot(&mut m, "reticle_confirm");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), "ColourUse(801, 8, 0)");
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: true,
        uid: 801,
        ..Default::default()
    }));
    settle(&mut m);
    let r = m.data.store.items.iter().find(|r| r.uid == 801).cloned();
    assert_eq!(r.map(|r| (r.in_use, r.function)), Some((true, 8)));
    assert_eq!(
        colorbox::match_reticle(Some(&rom), &m.data.store),
        Some((dc_client::spread::mark::shape_of(4), last))
    );
    // Unequip (0x36b ok): out of its function slot, the match no longer takes it
    m.feed_event(LinkEvent::FunctionSlotDone {
        put: false,
        ok: true,
        uid: 801,
    });
    settle(&mut m);
    assert_eq!(colorbox::match_reticle(Some(&rom), &m.data.store), None);
}

/// colour-window-2009 (CLI-1877, owner report 2026-11-05 "the player name color change doesn't let you change colors
/// after you've picked one"): a second Player Name Color item while the first is in use: the question's New Color
/// closes only the question, a slot press picks a new colour and OK sends it naming the item in use, whose record
/// goes with the answer.
#[test]
fn rom_new_color_picks_a_new_name_colour_over_the_one_in_use() {
    const NAME: i32 = 9008;
    let Some(rom) = dc_rom::resolve_rom().ok() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    if Wares::cached(Some(&rom)).art_of(NAME).map(|a| a.sub_type) != Some(colorbox::SUB_NAME_COLOUR)
    {
        eprintln!("skipped: no item {NAME}");
        return;
    }
    let in_use = InvRecord {
        uid: 810,
        item_no: NAME as u32,
        in_use: true,
        function: 3,
        ..Default::default()
    };
    let Some(mut m) = opened_with(
        NAME,
        811,
        |m| {
            m.data.profile.nick = "Alice".into();
            m.data.char_extras.name_colour = 3;
        },
        vec![in_use],
    ) else {
        eprintln!("skipped: no name colour product");
        return;
    };
    let q = m.popup().cloned().expect("the in-use question");
    assert_eq!(
        (q.text.clone(), q.title),
        (m.cat.text(colorbox::IN_USE_TEXT), 0x371)
    );
    let buttons = q.buttons();
    assert_eq!(buttons.len(), 3, "Extend / New Color / Cancel");
    shot(&mut m, "name_colour_in_use");
    // New Color: the question goes, the window stays
    let b = buttons[1];
    press(&mut m, [b[0] + b[2] / 2 - 5, b[1] + b[3] / 2 - 5]);
    settle(&mut m);
    assert!(m.popup().is_none(), "only the question closes");
    let title = m
        .cat
        .text(ColourTexts::read(&m.cat, colorbox::SUB_NAME_COLOUR).title);
    assert!(texts(&mut m).contains(&title), "the window stays");
    let n = ColourTables::from_rom(Some(&rom)).names.len() as u32;
    let l = ColourLayout::read(&m.cat);
    let w = l.window_at();
    // the first slot holds the table's last colour, the second the one before it
    press(&mut m, [w[0] + l.list_at[0], w[1] + l.list_at[1]]);
    press(
        &mut m,
        [
            w[0] + l.list_at[0] + l.slot_size[0] + l.column_gap,
            w[1] + l.list_at[1],
        ],
    );
    assert!(
        texts(&mut m).contains(&"Alice".to_string()),
        "the preview name"
    );
    shot(&mut m, "name_colour_new");
    press(&mut m, [w[0] + l.ok_at[0], w[1] + l.ok_at[1]]);
    assert_eq!(
        m.link_latest_request(),
        format!("ColourUse(811, {}, 810)", n - 1)
    );
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: true,
        uid: 811,
        removed: 810,
        ..Default::default()
    }));
    settle(&mut m);
    assert_eq!(m.data.char_extras.name_colour, n - 1);
    assert!(!m.data.store.items.iter().any(|r| r.uid == 810));
    assert!(m
        .data
        .store
        .items
        .iter()
        .any(|r| r.uid == 811 && r.in_use && r.function == n - 1));
}

/// megaphone-country (CLI-1873, owner report 2026-11-05 "no country code for America"): the country window's list
/// offers the United States after the ROM's rows, and picking it sends its nation number.
#[test]
fn rom_country_window_offers_the_united_states() {
    let Some(item) = item_of(usebox::SUB_COUNTRY) else {
        eprintln!("skipped: no CA_ROM or no country code item");
        return;
    };
    let Some(mut m) = opened(item, 705, |m| m.data.char_extras.nation = 2) else {
        eprintln!("skipped: no country code product");
        return;
    };
    let rows = dc_client::menu::charinfo::Tables::cached(m.cat.rom.as_deref().unwrap()).countries;
    let usa = rows
        .iter()
        .position(|r| r.0 == "United States" && r.1 == "USA")
        .expect("the United States is a country row");
    assert!(usa >= 1, "the list shows rows 1..");
    let l = CountryLayout::read(&m.cat);
    let abs = |p: [i32; 2]| [l.window_at[0] + p[0], l.window_at[1] + p[1]];
    // the drop-down opens; the wheel scrolls its list to the end, where the United States is the last row
    press(&mut m, abs(l.button_at));
    let wheel_at = [
        (abs(l.list_at)[0] + 5) as f32,
        (abs(l.list_at)[1] + 5) as f32,
    ];
    m.input(MenuInput::Wheel(wheel_at, -200));
    assert!(
        texts(&mut m).iter().any(|t| t.trim() == "United States"),
        "the list offers the United States"
    );
    shot(&mut m, "country_list_usa");
    let last = (l.list_rows() as i32 - 1).min(rows.len() as i32 - 2);
    press(
        &mut m,
        [abs(l.list_at)[0], abs(l.list_at)[1] + last * ROW_HEIGHT],
    );
    m.input(MenuInput::Key(MenuKey::Enter));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(
        m.link_latest_request(),
        format!("ItemUse(705, \"\", 0, {usa})")
    );
}

/// megaphone-country (CLI-1874, owner report 2026-11-05 "missing the cursor ... can't type spaces or give it colors"):
/// the composer takes a space from the Space key, a pick on the colour grid colours what is typed after it, the field
/// draws its caret, and the send carries the colour runs (ROM only; `DC_USEBOX_PNG` writes the composer's picture).
#[test]
fn rom_megaphone_composer_types_spaces_and_colours() {
    let Some(item) = item_of(usebox::SUB_MEGAPHONE) else {
        eprintln!("skipped: no CA_ROM or no megaphone item");
        return;
    };
    let Some(mut m) = opened(item, 706, |m| m.data.profile.nick = "Alice".into()) else {
        eprintln!("skipped: no megaphone product");
        return;
    };
    assert_eq!(m.link_latest_request(), "ItemUse(706, \"\", 0, 0)");
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: true,
        uid: 706,
        count: 3,
        ..Default::default()
    }));
    settle(&mut m);
    let title = m.cat.text(megabox::TITLE_TEXT);
    assert!(texts(&mut m).contains(&title), "the composer");
    let names = ColourTables::from_rom(m.cat.rom.as_deref()).names;
    assert!(!names.is_empty(), "the ROM name colours");
    let probe = MegaBox::open(&m.cat, 706, 3, "Alice", names.clone());
    assert!(probe.slots() > 0, "the colour grid has slots");
    for c in "hi".chars() {
        m.input(MenuInput::Char(c));
    }
    m.input(MenuInput::Key(MenuKey::Space));
    // the first slot shows the table's last colour
    let k = 0;
    press(&mut m, probe.slot_at(k));
    for c in "all".chars() {
        m.input(MenuInput::Char(c));
    }
    let n = probe.slot_colour(k).unwrap();
    let picked = names[n as usize - 1].argb;
    let items = m.items_at(0.1);
    let caret = items.iter().any(|d| {
        matches!(d, DrawItem::Fill { rect, .. } if rect[2] == dc_client::menu::sheet::CARET_WIDTH as f32)
    });
    assert!(caret, "the field draws its caret");
    let coloured = items.iter().any(|d| {
        matches!(d, DrawItem::Label { text, style, .. }
            if text == "all" && style.colour == dc_client::menu::catalog::argb(picked))
    });
    assert!(coloured, "the text after the pick is in the picked colour");
    assert!(texts(&mut m).iter().any(|t| t == "6/65"));
    shot(&mut m, "megaphone_window");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), "MegaphoneQueue");
    m.feed_event(LinkEvent::MegaphoneQueued(QueueReply {
        ok: true,
        secs: 30,
        on_air: 1,
        last: 2,
        waiting: 1,
        ..Default::default()
    }));
    settle(&mut m);
    shot(&mut m, "megaphone_confirm");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), "MegaphoneSend(706, \"hi all\")");
}
