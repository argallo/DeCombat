//! CLI-1132: the weapon-limit box of a room has the 2009 "Go to Shop" button (cshell+0x160190: Yes / No box with the
//! captions 0x177e / 0x43; its callback cshell+0x15bb40 sends the bar's shop command 0x3d). ROM-gated.

use dc_client::menu::link::LinkOptions;
use dc_client::menu::room::CMD_START;
use dc_client::menu::storage::Wares;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};

#[test]
fn rom_the_weapon_limit_box_offers_go_to_shop_and_opens_the_shop() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(Some(&rom));
    let Some(rifle) = (0..1000).find(|&n| wares.kind_of(n) == Some(0)) else {
        eprintln!("skipped: no assault rifle in the item table");
        return;
    };
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    // no lobby thread: the room's shopping flag (CLI-1560) and the shop's requests would come back from a link without
    // a connection as a "connection ended" box racing the asserts
    m.detach_link();
    m.data.profile.my_id = 7;
    m.data.room.host_id = 7;
    m.data.room.map = 2;
    m.data.room.weapons = 6; // Sniper Only
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = rifle;
    m.open(Go::Room);
    for _ in 0..40 {
        m.update();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    m.input(MenuInput::Command(CMD_START));
    let q = m.popup().cloned().expect("the weapon-limit box");
    assert_eq!(q.captions, Some([0x177e, 0x43]), "Go to Shop / No");
    assert!(m.cat.text(0x177e).contains("Shop"), "{}", m.cat.text(0x177e));
    // No closes it and stays in the room
    let b = q.buttons();
    let at = |r: [i32; 4]| [(r[0] + 5) as f32, (r[1] + 5) as f32];
    m.input(MenuInput::Press(at(b[1])));
    m.input(MenuInput::Release(at(b[1])));
    assert_eq!((m.popup_count(), m.screen_name()), (0, "room"));
    // Yes (Go to Shop) closes it and opens the shop
    m.input(MenuInput::Command(CMD_START));
    m.input(MenuInput::Press(at(b[0])));
    m.input(MenuInput::Release(at(b[0])));
    m.update();
    assert_eq!(m.popup_count(), 0);
    assert_eq!(m.screen_name(), "shop");
}

/// A member of a "Sniper Only" room (the room's host is someone else) with a rifle equipped.
fn sniper_room_member(rom: std::path::PathBuf, rifle: i32) -> Menu {
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
    m.data.profile.my_id = 7;
    m.data.room.host_id = 8;
    m.data.room.map = 2;
    m.data.room.weapons = 6; // Sniper Only
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = rifle;
    m.open(Go::Room);
    for _ in 0..40 {
        m.update();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    m
}

/// LG-342: the lobby's weapon-limit refusal (our 0x393 reason) opens the 2009 weapon-limit box with the limit's two ROM
/// strings and Go to Shop, not the "need players" notice. ROM-gated.
#[test]
fn rom_the_lobby_weapon_limit_refusal_opens_the_limit_box() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let mut m = sniper_room_member(rom, -1);
    m.feed_event(dc_client::menu::link::LinkEvent::FlagReply {
        ok: false,
        reason: dc_wire::flagrefusal::REASON_WEAPON_LIMIT,
        kind: dc_wire::flagrefusal::KIND_WEAPON_LIMIT,
    });
    m.update();
    let q = m.popup().cloned().expect("the weapon-limit box");
    assert_eq!(q.tag, dc_client::menu::popup::BoxTag::LimitBox);
    assert_eq!(q.captions, Some([0x177e, 0x43]), "Go to Shop / Cancel");
    assert_eq!(q.text, format!("{}\n{}", m.cat.text(0x1795), m.cat.text(0x1796)));
    assert!(!q.text.contains(&m.cat.text(0x444)), "not the need-players notice");
}

/// LG-342: the host turning the limit to one the member's equipped weapons fail opens the box for the member at once.
/// ROM-gated.
#[test]
fn rom_a_limit_change_the_member_fails_opens_the_limit_box() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(Some(&rom));
    let Some(rifle) = (0..1000).find(|&n| wares.kind_of(n) == Some(0)) else {
        eprintln!("skipped: no assault rifle in the item table");
        return;
    };
    let mut m = sniper_room_member(rom, rifle);
    m.data.room.weapons = 0;
    let set = |m: &mut Menu, value: u32| {
        m.feed_event(dc_client::menu::link::LinkEvent::RoomSetting {
            kind: dc_client::menu::room::SET_WEAPONS,
            value,
            text: String::new(),
        });
        m.update();
    };
    // a limit the rifle passes: no box
    set(&mut m, 3);
    assert_eq!(m.popup_count(), 0);
    set(&mut m, 6);
    let q = m.popup().cloned().expect("the weapon-limit box");
    assert_eq!(q.tag, dc_client::menu::popup::BoxTag::LimitBox);
    assert_eq!(q.text, format!("{}\n{}", m.cat.text(0x1795), m.cat.text(0x1796)));
}
