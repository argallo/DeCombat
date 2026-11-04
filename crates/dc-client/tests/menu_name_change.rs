//! name-change (CLI-1365..1368): the player name change item (function item 9004, SubType 4) is used from the
//! storage page: an opened inbox product of it, and Equip (265) on its storage cell, open the 2009 rename window;
//! Check asks the lobby (0x346), OK sends 0x330 with the checked name, the 0x331 answers show the reason box (0x53:
//! the name is in use) or the success box, and the client takes the new name without a relogin.
//!
//! ROM-gated (skipped without `CA_ROM`: the item table, the strings and the layout section come from the user's ROM).
// SPEC: PROTOCOL_2009.md §4.3 (0x330, 0x331, 0x346 / 0x347), §4.5 (reason 0x53 -> res 0x17f7)
// DISASM: cshell+0x14f8a7..0x14f965 (Equip 265 on a function item opens its use window)
// EVIDENCE: cshell+0x14e010 (an opened function item opens its window), cshell+0x2441f0 (Check / OK), cshell+0x249d80

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{InboxRow, InvRecord, LinkEvent, LinkOptions, OpenReply, UseReply};
use dc_client::menu::shop::{CMD_WEAR, STORAGE_WINDOW};
use dc_client::menu::storage::Wares;
use dc_client::menu::usebox::{CHECK_TEXT, NAME_OK_TEXT, PROMPT_TEXT};
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};
use dc_client::steps::{resolve, Resolved};
use dc_ui_data::WidgetKind;

const RENAME_ITEM: i32 = 9004;

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

fn shown_window(m: &Menu, id: i32) -> bool {
    let s = m.sheet();
    s.parts
        .iter()
        .position(|p| p.kind == WidgetKind::Window && p.layer == id)
        .is_some_and(|i| s.visible(i))
}

/// The detached test link has no lobby: its "connection ended" boxes are closed (the inbox test does not look at
/// boxes; this one does).
fn settle(m: &mut Menu) {
    m.update();
    let ended = m
        .cat
        .text(dc_client::menu::popup::text_ids::CONNECTION_ENDED);
    while m.popup().is_some_and(|p| p.text == ended) {
        m.input(MenuInput::Key(MenuKey::Enter));
    }
}

fn shot(m: &mut Menu, dir: &str, name: &str) {
    let i = m.image();
    let f = dc_render::FrameImage {
        width: i.width,
        height: i.height,
        rgba: i.rgba,
    };
    let _ = f.save_png(&std::path::Path::new(dir).join(format!("{name}.png")));
}

fn type_text(m: &mut Menu, s: &str) {
    for c in s.chars() {
        m.input(MenuInput::Char(c));
    }
}

fn window_up(m: &mut Menu) -> bool {
    let check = m.cat.text(CHECK_TEXT);
    let ok = m.cat.text(NAME_OK_TEXT);
    texts(m).iter().any(|t| *t == check || *t == ok)
}

#[test]
fn rom_name_change_item_opens_the_rename_window_and_sends_the_checked_name() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    assert_eq!(
        wares.art_of(RENAME_ITEM).map(|a| a.sub_type),
        Some(4),
        "item 9004 is the player name change"
    );
    let Some(pid) = (0..6000u32).find(|p| {
        wares
            .ware(*p, &Default::default())
            .is_some_and(|w| w.item_no == RENAME_ITEM)
    }) else {
        eprintln!("skipped: no name change product");
        return;
    };
    m.detach_link();
    m.data.profile.nick = "Casey".into();
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
    // the opened function item: the storage page of the black market's function items and the rename window
    m.feed_event(LinkEvent::InboxOpened(OpenReply {
        ok: true,
        award_uid: 555,
        ..Default::default()
    }));
    settle(&mut m);
    m.feed_event(LinkEvent::Inventory(vec![InvRecord {
        uid: 555,
        item_no: RENAME_ITEM as u32,
        ..Default::default()
    }]));
    settle(&mut m);
    assert!(shown_window(&m, STORAGE_WINDOW), "the storage page");
    assert!(
        window_up(&mut m),
        "the rename window opens with the opened item"
    );
    let all = texts(&mut m);
    let prompt = m.cat.text(PROMPT_TEXT);
    assert!(all.contains(&prompt), "the field's prompt");
    assert!(all.iter().any(|t| t.contains("Casey")), "the current name");
    settle(&mut m);
    // Cancel (Escape): the 2009 cancelled box, the window closes
    m.input(MenuInput::Key(MenuKey::Escape));
    let q = m.popup().cloned().expect("cancelled box");
    assert_eq!((q.text, q.title), (m.cat.text(0x1807), 0x1811));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(!window_up(&mut m));
    // Equip (265) on the selected storage cell opens the window again
    m.input(MenuInput::Command(CMD_WEAR));
    assert!(
        window_up(&mut m),
        "Equip on the function item opens its window"
    );
    // a look at the window (headless PNG) when asked for
    if let Ok(dir) = std::env::var("DC_NAMEBOX_PNG") {
        shot(&mut m, &dir, "namebox_open");
    }
    // too short: no request, the length message
    type_text(&mut m, "Ab");
    let before = m.link_latest_request();
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), before);
    type_text(&mut m, "cdef");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), "NameCheck(\"Abcdef\")");
    // taken: OK stays off
    m.feed_event(LinkEvent::NameChecked(false));
    settle(&mut m);
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), "NameCheck(\"Abcdef\")");
    // free: "Name OK!", then Enter sends the use with the name
    m.feed_event(LinkEvent::NameChecked(true));
    settle(&mut m);
    let ok = m.cat.text(NAME_OK_TEXT);
    assert!(texts(&mut m).contains(&ok), "Name OK! caption");
    if let Ok(dir) = std::env::var("DC_NAMEBOX_PNG") {
        shot(&mut m, &dir, "namebox_checked");
    }
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(
        m.link_latest_request(),
        "ItemUse(555, \"Abcdef\", 0)",
        "0x330 with the checked name"
    );
    // refused with 0x53 (the name is in use, a GM prefix for a non-GM account too): the reason box, window stays
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: false,
        reason: 0x53,
        uid: 555,
        ..Default::default()
    }));
    settle(&mut m);
    let q = m.popup().cloned().expect("reason box");
    assert_eq!(q.text, m.cat.text(0x17f7));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(window_up(&mut m), "the window stays after a refusal");
    assert_eq!(m.data.profile.nick, "Casey");
    // check again, use, ok: the success box with the name, the own name changed, the item gone
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), "NameCheck(\"Abcdef\")");
    m.feed_event(LinkEvent::NameChecked(true));
    settle(&mut m);
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), "ItemUse(555, \"Abcdef\", 0)");
    m.feed_event(LinkEvent::ItemUsed(UseReply {
        ok: true,
        uid: 555,
        ..Default::default()
    }));
    settle(&mut m);
    let q = m.popup().cloned().expect("success box");
    assert!(q.text.contains("Abcdef"), "{}", q.text);
    assert_eq!(q.title, 6100);
    assert_eq!(m.link_latest_request(), "OwnName(\"Abcdef\")");
    assert_eq!(m.data.profile.nick, "Abcdef");
    assert!(!m.data.store.items.iter().any(|r| r.uid == 555));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(!window_up(&mut m), "the window closes after the rename");
}
