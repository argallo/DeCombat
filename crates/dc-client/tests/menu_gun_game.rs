//! LG-305 / CLI-1638 / CLI-1639: Gun Game on a featured server entry. The push 0x3f05 offers the row, 0x3f07 marks rooms;
//! an entry without the offer sees today's lists. The list part needs `CA_ROM` (skipped without it).

use dc_client::hud::state::HudState;
use dc_client::menu::create::{free_for_all_row, row_name, GUN_GAME_ROW};
use dc_client::menu::link::{LinkEvent, LinkOptions, RoomView};
use dc_client::menu::room::{Room, MODE_NAME, SETTINGS_WINDOW};
use dc_client::menu::{Go, Menu, MenuOptions};
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;

fn menu(rom: bool) -> Option<Menu> {
    let rom = if rom {
        Some(dc_rom::resolve_rom().ok()?)
    } else {
        None
    };
    let font = rom.is_none().then(|| (synth_font(), 0));
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "GUser".into(),
        link: LinkOptions::new("gun-game"),
        rom,
        font,
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.data.profile.my_id = 1;
    m.data.room = RoomView {
        number: 3,
        host_id: 1,
        ..RoomView::default()
    };
    Some(m)
}

#[test]
fn the_pushes_fill_the_data_and_a_channel_enter_clears_the_offer() {
    let mut m = menu(false).unwrap();
    assert!(!m.data.gun_offered);
    m.feed_event(LinkEvent::GunGameOffer);
    m.update();
    assert!(m.data.gun_offered);
    m.feed_event(LinkEvent::GunGameRoom {
        room: 3,
        on: true,
        ladder: vec![4, 12],
    });
    m.feed_event(LinkEvent::GunGameRoom {
        room: 9,
        on: true,
        ladder: vec![],
    });
    m.update();
    assert!(m.data.gun_rooms.contains(&9) && m.data.gun_rooms.contains(&3));
    assert!(m.data.room.gun_game, "the own room");
    assert_eq!(m.data.room.ladder, vec![4, 12]);
    m.feed_event(LinkEvent::RoomRemoved(9));
    m.update();
    assert!(!m.data.gun_rooms.contains(&9), "a reused number starts clean");
    m.feed_event(LinkEvent::GunGameRoom {
        room: 3,
        on: false,
        ladder: vec![],
    });
    m.update();
    assert!(!m.data.room.gun_game);
    m.feed_event(LinkEvent::Rooms(Vec::new()));
    m.update();
    assert!(m.data.gun_rooms.is_empty());
}

#[test]
fn rom_the_row_shows_only_on_an_offering_entry_and_runs_on_the_free_for_all_row() {
    let Some(mut m) = menu(true) else {
        eprintln!("skipped: CA_ROM unset");
        return;
    };
    let names = |m: &Menu| -> Vec<String> {
        Room::list_rows(&m.cat, &m.data, "mode")
            .into_iter()
            .map(|r| r.0)
            .collect()
    };
    assert!(!names(&m).iter().any(|n| n == "Gun Game"));
    let base = free_for_all_row(&m.cat, &m.data).expect("a free-for-all row");
    assert_eq!(m.cat.mode_class(base), Some(6));
    assert_eq!(row_name(&m.cat, GUN_GAME_ROW), "Gun Game");
    m.feed_event(LinkEvent::GunGameOffer);
    m.update();
    assert!(names(&m).iter().any(|n| n == "Gun Game"));
    // the waiting room names it, on the free-for-all row
    m.data.room.mode = base;
    m.feed_event(LinkEvent::GunGameRoom {
        room: 3,
        on: true,
        ladder: vec![],
    });
    m.open(Go::Room);
    m.update();
    let s = m.sheet();
    let i = s
        .find(SETTINGS_WINDOW, MODE_NAME, WidgetKind::Text)
        .expect("mode text");
    assert_eq!(s.parts[i].text, "Gun Game");
}

#[test]
fn rom_the_ladder_names_come_from_the_shop() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: CA_ROM unset");
        return;
    };
    // weapon row 4 is the shop's M4A1 in the user's ROM; an unknown row keeps its number
    let names = dc_client::hud::gun_ladder_names(Some(&rom), &[4, 60000]);
    assert!(!names[0].is_empty() && !names[0].starts_with('#'), "{names:?}");
    assert_eq!(names[1], "#60000");
}

/// The rung comes from the own 0x6b record alone (a kill feed line counts nothing): a rise shows the next gun, a drop
/// the knife line; both lines go after a moment and the board's numbers stay the server's.
#[test]
fn the_hud_follows_the_own_record() {
    use dc_client::hud::state::{GunGameHud, GUN_LINE_MS};
    use dc_botclient::shell::RosterRecord;
    let mut h = HudState {
        gun_game: Some(GunGameHud::new(vec!["A".into(), "B".into(), "C".into()])),
        own_pid: Some(0),
        ..HudState::default()
    };
    let rec = |kills: i16| RosterRecord {
        pid: 0,
        kills,
        ..RosterRecord::default()
    };
    h.now_ms = 1000;
    h.on_roster(&rec(1));
    let g = h.gun_game.clone().unwrap();
    assert_eq!(g.rung, 1);
    assert_eq!(g.line, Some(("Next gun: B".to_string(), false, 1000)));
    h.now_ms = 1000 + GUN_LINE_MS;
    h.on_roster(&rec(0));
    let g = h.gun_game.clone().unwrap();
    assert_eq!(g.rung, 0);
    assert_eq!(
        g.line,
        Some(("Knifed! Back to A".to_string(), true, 1000 + GUN_LINE_MS))
    );
    assert_eq!(h.roster[&0].kills, 0);
}
