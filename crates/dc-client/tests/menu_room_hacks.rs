//! LG-274 / CLI-1626 / CLI-1627: the waiting room's Hacks row of a modded lobby. The lobby's push 0x3f03 (choices and
//! the room's setting) fills the room's data; the row (bar, label, value, dropdown button) shows only when a choice
//! is offered, the settings window's art grows its Game Settings section by one row and the map picture and the
//! description move down (the description cut to four lines); the host's pick goes out as 0x3f04. A vanilla room
//! keeps the 2009 layout. The layout part needs `CA_ROM` (skipped without it).

use dc_botclient::lobby::{LobbyProfile, RoomMember};
use dc_client::menu::link::{LinkEvent, LinkOptions, RoomView};
use dc_client::menu::room::{
    hacks_row, HACKS_DESCRIPTION_LINES, MAP_DESCRIPTION, MAP_PICTURE, SETTINGS_WINDOW,
};
use dc_client::menu::sheet::Look;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;

fn menu(rom: bool, host: bool) -> Option<Menu> {
    let rom = if rom {
        Some(dc_rom::resolve_rom().ok()?)
    } else {
        None
    };
    let font = rom.is_none().then(|| (synth_font(), 0));
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "HUser".into(),
        link: LinkOptions::new("room-hacks"),
        rom,
        font,
        width: 1024,
        height: 768,
    });
    m.detach_link();
    let d = &mut m.data;
    d.profile = LobbyProfile {
        nick: "HUser".into(),
        my_id: 1,
        ..LobbyProfile::default()
    };
    d.room = RoomView {
        title: "h".into(),
        host_id: if host { 1 } else { 2 },
        players: 2,
        max_players: 8,
        goal: 50,
        ..RoomView::default()
    };
    for (id, team) in [(1u32, 1), (2, 2)] {
        d.members.insert(
            id,
            RoomMember {
                char_no: id as i32,
                name: format!("P{id}"),
                team,
                state: 0,
                exp: 0,
            },
        );
    }
    m.open(Go::Room);
    Some(m)
}

fn hacks(m: &mut Menu, offered: u32, value: u32) {
    m.feed_event(LinkEvent::RoomHacks { offered, value });
    m.update();
}

fn shown(m: &Menu, id: i32, kind: WidgetKind) -> bool {
    let s = m.sheet();
    s.find(SETTINGS_WINDOW, id, kind).is_some_and(|i| s.visible(i))
}

const ROW: [(i32, WidgetKind); 6] = [
    (hacks_row::BAR, WidgetKind::Image),
    (hacks_row::BUTTON, WidgetKind::Button),
    (hacks_row::LABEL, WidgetKind::Text),
    (hacks_row::VALUE, WidgetKind::Text),
    (hacks_row::ART_MIDDLE, WidgetKind::Image),
    (hacks_row::ART_LOWER, WidgetKind::Image),
];

#[test]
fn the_push_fills_the_room_and_only_the_host_picks() {
    let mut m = menu(false, true).unwrap();
    assert_eq!((m.data.room.hacks_offered, m.data.room.hacks), (0, 0));
    // vanilla: the row's command does nothing
    m.input(MenuInput::Command(hacks_row::CMD));
    assert!(m.dropdown().is_none());
    hacks(&mut m, 3, 1);
    assert_eq!((m.data.room.hacks_offered, m.data.room.hacks), (3, 1));
    // a new room info clears the setting until the next push
    m.feed_event(LinkEvent::RoomInfo(RoomView {
        host_id: 1,
        ..RoomView::default()
    }));
    m.update();
    assert_eq!(m.data.room.hacks_offered, 0);
    // a guest cannot open the list
    let mut g = menu(false, false).unwrap();
    hacks(&mut g, 3, 1);
    g.input(MenuInput::Command(hacks_row::CMD));
    assert!(g.dropdown().is_none());
}

#[test]
fn the_match_bridge_follows_the_wallhacks_bit() {
    use dc_client::window::room_wallhacks;
    assert!(
        room_wallhacks(None),
        "no Hacks choice: the set decides (CLI-1620)"
    );
    assert!(room_wallhacks(Some(1)) && room_wallhacks(Some(3)));
    assert!(!room_wallhacks(Some(2)), "no-recoil only: no x-ray bridge");
}

#[test]
fn rom_the_hacks_row_shows_only_with_a_choice_and_moves_the_map_block() {
    let Some(mut m) = menu(true, true) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let pos = |m: &Menu, id: i32, kind: WidgetKind| {
        let s = m.sheet();
        s.parts[s.find(SETTINGS_WINDOW, id, kind).unwrap()].pos
    };
    let window_tex = |m: &Menu| {
        let s = m.sheet();
        let w = s
            .find(SETTINGS_WINDOW, SETTINGS_WINDOW, WidgetKind::Window)
            .unwrap();
        match &s.parts[w].look {
            Look::Panel { tex, .. } => tex.clone(),
            _ => String::new(),
        }
    };
    // vanilla: no row, the 2009 places, the whole art
    for (id, kind) in ROW {
        assert!(!shown(&m, id, kind), "{id}");
    }
    let picture = pos(&m, MAP_PICTURE, WidgetKind::Image);
    let description = pos(&m, MAP_DESCRIPTION, WidgetKind::Text);
    let art = window_tex(&m);
    assert!(!art.contains('#'));
    // a choice offered: the row, one row lower than the weapon limit's, the map block one row down
    hacks(&mut m, 3, 3);
    for (id, kind) in ROW {
        assert!(shown(&m, id, kind), "{id}");
    }
    let arms_bar = pos(&m, 5, WidgetKind::Image);
    assert_eq!(
        pos(&m, hacks_row::BAR, WidgetKind::Image),
        [arms_bar[0], arms_bar[1] + hacks_row::PITCH]
    );
    assert_eq!(
        pos(&m, MAP_PICTURE, WidgetKind::Image),
        [picture[0], picture[1] + hacks_row::PITCH]
    );
    assert_eq!(
        pos(&m, MAP_DESCRIPTION, WidgetKind::Text),
        [description[0], description[1] + hacks_row::PITCH]
    );
    assert!(
        window_tex(&m).starts_with(&format!("{art}#")),
        "the art's top piece"
    );
    {
        let s = m.sheet();
        let value = &s.parts[s
            .find(SETTINGS_WINDOW, hacks_row::VALUE, WidgetKind::Text)
            .unwrap()];
        assert_eq!(value.text, "Wallhacks + no-recoil");
        // centred about the same x as the Arms value, one row lower (the other setting fields' rule)
        let arms = &s.parts[s.find(SETTINGS_WINDOW, 21, WidgetKind::Text).unwrap()];
        assert_eq!(value.pos, [arms.pos[0], arms.pos[1] + hacks_row::PITCH]);
        match (&value.look, &arms.look) {
            (Look::Text { align: a, .. }, Look::Text { align: b, .. }) => {
                assert_eq!((a.first, a.second), (b.first, b.second));
                assert_eq!(a.first, 4, "centre");
            }
            _ => panic!("text records"),
        }
        let desc = &s.parts[s
            .find(SETTINGS_WINDOW, MAP_DESCRIPTION, WidgetKind::Text)
            .unwrap()];
        assert_eq!(desc.wrap_max, HACKS_DESCRIPTION_LINES);
    }
    // the description draws at most four lines
    let rows = m.draw_trace();
    let sheet = m.trace_sheet().clone();
    let i = sheet
        .find(SETTINGS_WINDOW, MAP_DESCRIPTION, WidgetKind::Text)
        .unwrap();
    let lines = rows
        .iter()
        .filter(|r| matches!(r.origin, dc_client::menu::drawlist::Origin::Part { part, .. } if part == i))
        .count();
    assert!((1..=4).contains(&lines), "{lines}");
    // the host's list: the set's three choices; a pick sends 0x3f04
    m.take_link_sent();
    m.input(MenuInput::Command(hacks_row::CMD));
    let d = m.dropdown().expect("the hacks list is open").clone();
    assert_eq!((d.list, d.values.clone()), ("hacks", vec![1, 3, 2]));
    assert_eq!(
        d.items,
        ["Wallhacks", "Wallhacks + no-recoil", "No-recoil only"]
    );
    let r = d.rect();
    let x = (r[0] + r[2] / 2) as f32;
    let y = (r[1]..r[1] + r[3])
        .map(|y| y as f32)
        .find(|&y| d.row_at([x, y]) == Some(2))
        .unwrap();
    m.input(MenuInput::Move([x, y]));
    m.input(MenuInput::Press([x, y]));
    m.input(MenuInput::Release([x, y]));
    let sent = m.take_link_sent();
    assert!(sent.iter().any(|c| c == "RoomHacks(2)"), "{sent:?}");
    // back to a room without a choice: the 2009 layout again
    hacks(&mut m, 0, 0);
    assert!(!shown(&m, hacks_row::BAR, WidgetKind::Image));
    assert_eq!(pos(&m, MAP_PICTURE, WidgetKind::Image), picture);
    assert_eq!(window_tex(&m), art);
}
