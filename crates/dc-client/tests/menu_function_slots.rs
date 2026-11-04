//! function-unequip (CLI-1395 .. CLI-1397): the function slots of 0x323 +0x8a kept client-side, Equip on an in-use
//! function item whose row has `IsCanUnEquip` toggling it out of (0x36a) / into (0x368) its function slot, the
//! storage cell's Equiped mark and the Equip button's caption 0x160e / 0x160f, the lobby's answers 0x369 / 0x36b.
//!
//! The ROM tests are ROM-gated (skipped without `CA_ROM`: the item table, the strings and the layout come from the
//! user's ROM). Headless, no lobby server: requests are read back from the link's last-sent hook, replies fed in.
// SPEC: PROTOCOL_2009.md §1.2 (0x368 / 0x36a in frame 0x324), §3.3 (0x323 +0x8a), §4.2 (0x369 / 0x36b)
// EVIDENCE: cshell+0x14f830 (Equip 265 on a type 0x15 cell, DISASM 0x14f96a..0x14f9b4), cshell+0x1413c0 / 0x141480,
// cshell+0x18f7b0 / 0x18f890, cshell+0x229ca0, cshell+0x2292e0, cshell+0x226800, cshell+0x14a700, cshell+0x147470,
// cshell+0x14cea0 / 0x14d010

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{
    function_slot_message, function_slots, InboxRow, InvRecord, LinkEvent, LinkOptions, OpenReply,
};
use dc_client::menu::shop::{
    CMD_WEAR, FUNCTION_IDLE_CAPTION, FUNCTION_PUT_REFUSED, FUNCTION_TAKE_REFUSED, STORAGE_WINDOW,
    UNWEAR_CAPTION, WEAR_CAPTION,
};
use dc_client::menu::sheet::Look;
use dc_client::menu::storage::Wares;
use dc_client::menu::tuning::EQUIPPED_MARK;
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};
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

fn settle(m: &mut Menu) {
    m.update();
    let ended = m
        .cat
        .text(dc_client::menu::popup::text_ids::CONNECTION_ENDED);
    while m.popup().is_some_and(|p| p.text == ended) {
        m.input(MenuInput::Key(MenuKey::Enter));
    }
}

/// The storage page of function item `item` (opened from an inbox product of it), holding `items`; `None` without a
/// product of the item.
fn storage_of(m: &mut Menu, item: i32, items: Vec<InvRecord>) -> Option<()> {
    let wares = Wares::cached(m.cat.rom.as_deref());
    let pid = (0..6000u32).find(|p| {
        wares
            .ware(*p, &Default::default())
            .is_some_and(|w| w.item_no == item)
    })?;
    m.detach_link();
    m.data.store.inbox = vec![InboxRow {
        entry: 41,
        product: pid,
        received: 134_374_736_000_000_000,
        ..InboxRow::default()
    }];
    m.data.store.inbox_total = 1;
    m.data.store.inbox_fetched = true;
    m.open(Go::Lobby);
    let dc_client::steps::Resolved::Part { at, .. } = dc_client::steps::resolve(m, "lobby.inbox")
    else {
        panic!("lobby.inbox");
    };
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
    m.input(MenuInput::Command(1027));
    m.input(MenuInput::Key(MenuKey::Enter));
    settle(m);
    m.feed_event(LinkEvent::InboxOpened(OpenReply {
        ok: true,
        award_uid: items[0].uid,
        ..Default::default()
    }));
    settle(m);
    m.feed_event(LinkEvent::Inventory(items));
    settle(m);
    // the item-use window an opening may show is not part of these tests
    for _ in 0..4 {
        if m.popup().is_none() {
            break;
        }
        m.input(MenuInput::Key(MenuKey::Escape));
        settle(m);
    }
    Some(())
}

fn rec(uid: u32, item_no: u32, in_use: bool) -> InvRecord {
    InvRecord {
        uid,
        item_no,
        in_use,
        ..Default::default()
    }
}

/// The storage Equip button's caption string id and its enabled flag.
fn wear(m: &Menu) -> (i32, bool) {
    let p = m
        .sheet()
        .parts
        .iter()
        .find(|p| p.layer == STORAGE_WINDOW && p.kind == WidgetKind::Button && p.command == CMD_WEAR)
        .expect("the storage Equip button");
    let c = match &p.look {
        Look::Button {
            caption: Some(c), ..
        } => c.string_id,
        _ => -1,
    };
    (c, p.enabled)
}

fn marks(m: &mut Menu) -> usize {
    m.items()
        .iter()
        .filter(|i| matches!(i, DrawItem::Image { tex, .. } if tex == EQUIPPED_MARK.0))
        .count()
}

#[test]
fn equipment_reply_function_slots_and_the_0x368_message() {
    let mut d = vec![0u8; 0xa6];
    d[6] = 1;
    for (i, v) in [9113i32, -1, 9019, -1, -1, -1, -1].iter().enumerate() {
        d[0x8a + 4 * i..0x8e + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    assert_eq!(function_slots(&d), Some([9113, -1, 9019, -1, -1, -1, -1]));
    d[6] = 0;
    assert_eq!(function_slots(&d), None, "a failed reply");
    assert_eq!(function_slots(&[1, 0, 0, 0, 0, 0, 1, 0, 0, 0]), None, "short");
    // 0x368 / 0x36a: +6 item number, +0xa unique id, 0xe bytes; frame 0x324 carries them (inner-id override)
    for (put, id) in [(true, 0x368u16), (false, 0x36a)] {
        let p = function_slot_message(put, 600, 9019).to_payload();
        assert_eq!(p.len(), 0xe);
        assert_eq!(u16::from_le_bytes([p[0], p[1]]), id);
        assert_eq!(u32::from_le_bytes(p[2..6].try_into().unwrap()), 0xe);
        assert_eq!(u32::from_le_bytes(p[6..10].try_into().unwrap()), 9019);
        assert_eq!(u32::from_le_bytes(p[10..14].try_into().unwrap()), 600);
        assert_eq!(dc_wire::effective_packet_id(0x324, &p), id);
    }
}

/// CLI-1395: Equip on an in-use Bonus EXP Pass (IsCanUnEquip) in a function slot sends 0x36a, the ok answer takes it
/// out (mark gone, caption Equip), Equip again sends 0x368, a refusal shows box 0x506, the ok answer puts it back; the
/// next 0x323 replaces the held slots.
#[test]
fn rom_equip_on_an_in_use_pass_takes_it_out_and_puts_it_back() {
    const PASS: u32 = 9019;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    if !wares.can_unequip(PASS as i32) {
        eprintln!("skipped: item {PASS} has no IsCanUnEquip");
        return;
    }
    if storage_of(&mut m, PASS as i32, vec![rec(600, PASS, true)]).is_none() {
        eprintln!("skipped: no product of {PASS}");
        return;
    }
    // the slot list of 0x323 +0x8a: the pass sits in a function slot
    m.feed_event(LinkEvent::FunctionSlots([-1, PASS as i32, -1, -1, -1, -1, -1]));
    settle(&mut m);
    assert_eq!(m.data.store.function_slots, vec![-1, PASS as i32, -1, -1, -1, -1, -1]);
    assert_eq!(wear(&m), (UNWEAR_CAPTION, true), "Unequip on an item in its slot");
    assert_eq!(marks(&mut m), 1, "the Equiped mark");
    assert_eq!(m.cat.text(UNWEAR_CAPTION), "Unequip");
    // Equip: take it out (0x36a)
    m.input(MenuInput::Command(CMD_WEAR));
    assert_eq!(m.link_latest_request(), format!("FunctionUnequip(600, {PASS})"));
    m.feed_event(LinkEvent::FunctionSlotDone {
        put: false,
        ok: true,
        uid: 600,
    });
    settle(&mut m);
    assert!(m.popup().is_none());
    assert!(!m.data.store.function_slots.contains(&(PASS as i32)));
    assert_eq!(marks(&mut m), 0, "no mark out of its slot");
    assert_eq!(wear(&m), (WEAR_CAPTION, true));
    // Equip again: put it back (0x368); refused first
    m.input(MenuInput::Command(CMD_WEAR));
    assert_eq!(m.link_latest_request(), format!("FunctionEquip(600, {PASS})"));
    m.feed_event(LinkEvent::FunctionSlotDone {
        put: true,
        ok: false,
        uid: 600,
    });
    settle(&mut m);
    let q = m.popup().cloned().expect("the refusal box");
    assert_eq!((q.text, q.title), (m.cat.text(FUNCTION_PUT_REFUSED), 0x371));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(marks(&mut m), 0);
    m.feed_event(LinkEvent::FunctionSlotDone {
        put: true,
        ok: true,
        uid: 600,
    });
    settle(&mut m);
    assert!(m.data.store.function_slots.contains(&(PASS as i32)));
    assert_eq!(marks(&mut m), 1);
    assert_eq!(wear(&m), (UNWEAR_CAPTION, true));
    // a refused take-out: box 0x4dd
    m.feed_event(LinkEvent::FunctionSlotDone {
        put: false,
        ok: false,
        uid: 600,
    });
    settle(&mut m);
    assert_eq!(
        m.popup().map(|p| p.text.clone()),
        Some(m.cat.text(FUNCTION_TAKE_REFUSED))
    );
    m.input(MenuInput::Key(MenuKey::Enter));
    // the reread 0x323 is the lobby's word
    m.feed_event(LinkEvent::FunctionSlots([-1; 7]));
    settle(&mut m);
    assert_eq!(marks(&mut m), 0);
    assert_eq!(wear(&m), (WEAR_CAPTION, true));
}

/// CLI-1395: an in-use item whose row has no IsCanUnEquip (Elite Moderator) gets no request; it is in the function
/// list (mark shown) and the Equip button reads Unequip, disabled. CLI-1396: an item not in use reads Activate (0x1640).
#[test]
fn rom_an_item_without_is_can_unequip_sends_nothing() {
    const ELITE: u32 = 9010;
    const PASS: u32 = 9019;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    if wares.can_unequip(ELITE as i32) || wares.art_of(ELITE as i32).is_none() {
        eprintln!("skipped: item {ELITE} missing or unequippable");
        return;
    }
    if storage_of(
        &mut m,
        ELITE as i32,
        vec![rec(700, ELITE, true), rec(701, PASS, false)],
    )
    .is_none()
    {
        eprintln!("skipped: no product of {ELITE}");
        return;
    }
    m.feed_event(LinkEvent::FunctionSlots([-1; 7]));
    settle(&mut m);
    let shown: Vec<u32> = m.data.store.items.iter().map(|r| r.uid).collect();
    assert_eq!(shown, vec![700, 701]);
    // in the function list (cshell+0x2292e0), so Unequip, but disabled (row +0x241 clear, cshell+0x147470)
    assert_eq!(wear(&m), (UNWEAR_CAPTION, false), "disabled for an item that stays on");
    assert_eq!(marks(&mut m), 1, "an item that stays on is in the function list");
    let before = m.link_latest_request();
    m.input(MenuInput::Command(CMD_WEAR));
    assert_eq!(m.link_latest_request(), before, "no request");
    assert!(m.popup().is_none());
    // the pass not in use: Activate
    let cells = dc_client::menu::shop::grid_cells(
        &m.cat,
        dc_client::menu::pages::SHOP,
        STORAGE_WINDOW,
        dc_client::menu::shop::STORAGE_GRID,
    );
    let c = cells[1];
    let p = [(c[0] + c[2] / 2) as f32, (c[1] + c[3] / 2) as f32];
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
    assert_eq!(wear(&m), (FUNCTION_IDLE_CAPTION, true));
    assert_eq!(m.cat.text(FUNCTION_IDLE_CAPTION), "Activate");
}
