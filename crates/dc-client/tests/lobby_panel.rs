//! w1-lobbypanel: rank badges by experience in the lobby user list and the waiting room, and the Character Info
//! panel (rank icon, level, progress, clan, NX prepaid / credit, GP, grade) against the user's ROM.
//!
//! ROM-gated (skipped without `CA_ROM`); nothing of the ROM is asserted by value: the expectations are computed
//! from the tables read at run time.

use std::collections::BTreeMap;

use dc_botclient::lobby::{LobbyProfile, RoomMember};
use dc_client::hud::items::DrawItem;
use dc_client::menu::catalog::Catalog;
use dc_client::menu::link::{CharExtras, LinkEvent, UserRow};
use dc_client::menu::lobby::{LobbyScreen, PanelArt, CHAR_WINDOW};
use dc_client::menu::room::Room;
use dc_client::menu::{MenuData, Screen};
use dc_ui_data::WidgetKind;

fn images(items: &[DrawItem], tex: &str) -> usize {
    items
        .iter()
        .filter(|i| matches!(i, DrawItem::Image { tex: t, .. } if t == tex))
        .count()
}

fn text(s: &dyn Screen, id: i32) -> String {
    s.sheet()
        .part(CHAR_WINDOW, id, WidgetKind::Text)
        .map(|p| p.text.clone())
        .unwrap_or_default()
}

/// Two experience values that fall in levels with different icons.
fn two_exps(art: &PanelArt) -> (u32, u32) {
    let small = &art.levels.small;
    let a = 0;
    let level_a = art.levels.level_of(a);
    let b = art
        .levels
        .bounds
        .iter()
        .copied()
        .find(|e| {
            art.levels.icon_of_exp(*e, "x") != small.get(level_a as usize).map(String::as_str)
        })
        .expect("a level with another icon");
    (a, b)
}

#[test]
fn rom_lobby_user_rows_pick_the_rank_badge_by_experience() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let art = PanelArt::from_rom(&rom).expect("panel art");
    let (ea, eb) = two_exps(&art);
    let ia = art.levels.icon_of_exp(ea, "x").unwrap().to_string();
    let ib = art.levels.icon_of_exp(eb, "x").unwrap().to_string();
    assert_ne!(ia, ib, "two experience values, two badges");
    let mut cat = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
    let data = MenuData {
        profile: LobbyProfile {
            nick: "Me".into(),
            exp: eb,
            my_id: 2,
            ..LobbyProfile::default()
        },
        users: vec![
            UserRow {
                account: 1,
                id: 1,
                name: "A".into(),
                exp: ea,
                ..Default::default()
            },
            UserRow {
                account: 2,
                id: 2,
                name: "Me".into(),
                exp: eb,
                ..Default::default()
            },
            UserRow {
                account: 3,
                id: 3,
                name: "B".into(),
                exp: eb,
                ..Default::default()
            },
        ],
        ..MenuData::default()
    };
    let screen = LobbyScreen::new(&cat, &data);
    let mut items = Vec::new();
    screen.draw_extra(&mut cat, &data, 0.0, &mut items);
    assert_eq!(images(&items, &ia), 1, "one user row with the low badge");
    assert!(
        images(&items, &ib) >= 2,
        "two user rows with the high badge"
    );
}

#[test]
fn rom_character_panel_fills_rank_level_clan_balance_gp_and_grade() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let art = PanelArt::from_rom(&rom).expect("panel art");
    assert!(art.grades.len() >= 2, "grade table read");
    assert!(!art.short_names.is_empty(), "short level names read");
    let (_, eb) = two_exps(&art);
    let cat = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
    let data = MenuData {
        profile: LobbyProfile {
            nick: "Me".into(),
            exp: eb,
            gp: 1_234_567,
            ..LobbyProfile::default()
        },
        ..MenuData::default()
    };
    let mut screen = LobbyScreen::new(&cat, &data);
    assert_eq!(text(&screen, 3), "Me");
    assert_eq!(text(&screen, 13), "1,234,567", "GP with separators");
    assert_eq!(text(&screen, 9), "0", "no balance yet");
    assert_eq!(text(&screen, 11), "0");
    assert_eq!(text(&screen, 7), cat.text(0x41a), "no clan: string 0x41a");
    assert_eq!(text(&screen, 4), art.short_name(eb));
    let pr = text(&screen, 5);
    assert!(pr.contains('/') && pr.ends_with(" %)"), "progress {pr:?}");
    // own rank icon image
    let icon = art.levels.icon_of_exp(eb, "Me").unwrap();
    let tex = |s: &LobbyScreen, id: i32| match s
        .sheet()
        .part(CHAR_WINDOW, id, WidgetKind::Image)
        .map(|p| p.look.clone())
    {
        Some(dc_client::menu::sheet::Look::Panel { tex, .. }) => tex,
        _ => String::new(),
    };
    assert_eq!(tex(&screen, 0), icon);

    // the link delivered the extras and the balance before the screen existed
    let extras = CharExtras {
        grade: 1,
        clan_id: 7,
        clan_label: "Foxes".into(),
        ..CharExtras::default()
    };
    screen.apply_char_data(&cat, &data, &extras, (1_000, 25_000));
    assert_eq!(text(&screen, 7), "Foxes");
    assert_eq!(text(&screen, 9), "25,000", "prepaid");
    assert_eq!(text(&screen, 11), "1,000", "credit");
    let g = art.grade(1).unwrap();
    assert_eq!(text(&screen, 15), g.name);
    assert_eq!(tex(&screen, 2), g.icon);
    // live events: the grade push and the balance reply
    let mut data2 = MenuData {
        profile: data.profile.clone(),
        ..MenuData::default()
    };
    let mut go = None;
    let mut quit = false;
    let mut popups = Vec::new();
    let mut sounds = Vec::new();
    let link = dc_client::menu::link::Link::start(dc_client::menu::link::LinkOptions::new("t"));
    let mut ctx = dc_client::menu::Ctx {
        cat: &cat,
        data: &mut data2,
        link: &link,
        popups: &mut popups,
        go: &mut go,
        quit: &mut quit,
        sounds: &mut sounds,
    };
    screen.event(&LinkEvent::Grade(0), &mut ctx);
    screen.event(
        &LinkEvent::Balance {
            credit: 5,
            prepaid: 6,
        },
        &mut ctx,
    );
    assert_eq!(text(&screen, 15), art.grade(0).unwrap().name);
    assert_eq!(text(&screen, 9), "6");
    assert_eq!(text(&screen, 11), "5");
}

#[test]
fn rom_waiting_room_member_rows_pick_the_rank_badge_by_experience() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let art = PanelArt::from_rom(&rom).expect("panel art");
    let (ea, eb) = two_exps(&art);
    let ia = art.levels.icon_of_exp(ea, "x").unwrap().to_string();
    let ib = art.levels.icon_of_exp(eb, "x").unwrap().to_string();
    let mut cat = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
    let member = |name: &str, exp: u32| RoomMember {
        char_no: 1,
        name: name.into(),
        team: 1,
        state: 0,
        exp,
    };
    let data = MenuData {
        profile: LobbyProfile {
            nick: "Me".into(),
            exp: eb,
            my_id: 2,
            ..LobbyProfile::default()
        },
        users: vec![UserRow {
            account: 1,
            id: 1,
            name: "A".into(),
            exp: ea,
            ..Default::default()
        }],
        members: BTreeMap::from([(1, member("A", ea)), (2, member("Me", eb))]),
        ..MenuData::default()
    };
    let room = Room::new(&cat, &data);
    let mut items = Vec::new();
    room.draw_extra(&mut cat, &data, 0.0, &mut items);
    assert_eq!(images(&items, &ia), 1, "member A: the low badge");
    assert_eq!(
        images(&items, &ib),
        1,
        "own row: the badge of the own experience"
    );
}

/// The room list screen's chat: the Enter button at the screen's own place with its own textures and caption
/// place, the chat lists' scroll bars at x 520; the room creation screen (the lobby underneath) the same; the
/// waiting room keeps the page's places. The character info numbers are white, the grade name in the grade colour.
#[test]
fn rom_lobby_narrows_the_chat_and_colours_the_character_numbers() {
    use dc_client::menu::create::Create;
    use dc_client::menu::lobby::{
        CHAR_NUMBER_COLOUR, CHAT_ENTER, CHAT_LISTS, CHAT_PANEL, LOBBY_CHAT_SCROLL_X,
        LOBBY_ENTER_AT, LOBBY_ENTER_CAPTION_AT,
    };
    use dc_client::menu::sheet::{Look, Sheet};
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let cat = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
    let prefix = cat
        .paths
        .lobby_enter_prefix()
        .expect("the four lobby Enter textures resolve");
    let data = MenuData::default();
    let check = |sheet: &Sheet| {
        let b = sheet
            .part(CHAT_PANEL, CHAT_ENTER, WidgetKind::Button)
            .expect("chat Enter button");
        assert_eq!(b.pos, LOBBY_ENTER_AT);
        assert_eq!(b.text_at, LOBBY_ENTER_CAPTION_AT);
        assert!(b.placed);
        match &b.look {
            Look::Button {
                prefix: p, caption, ..
            } => {
                assert_eq!(p, &prefix);
                assert_eq!(caption.as_ref().map(|c| c.at), Some(LOBBY_ENTER_CAPTION_AT));
            }
            other => panic!("not a button look: {other:?}"),
        }
        for id in CHAT_LISTS {
            let l = sheet
                .part(CHAT_PANEL, id, WidgetKind::List)
                .expect("chat list");
            match &l.look {
                Look::List { scroll_at, .. } => {
                    assert_eq!(l.pos[0] + scroll_at[0], LOBBY_CHAT_SCROLL_X)
                }
                other => panic!("not a list look: {other:?}"),
            }
        }
    };
    let lobby = LobbyScreen::new(&cat, &data);
    check(lobby.sheet());
    let create = Create::new(&cat, &data);
    check(create.sheet());
    // the waiting room keeps the chat page's own Enter place
    let room = Room::new(&cat, &data);
    let b = room
        .sheet()
        .part(CHAT_PANEL, CHAT_ENTER, WidgetKind::Button)
        .expect("room chat Enter");
    assert_ne!(b.pos, LOBBY_ENTER_AT);
    assert!(!b.placed);
    // character numbers white, grade name in the grade row's colour
    let art = PanelArt::from_rom(&rom).expect("panel art");
    // the room creation box fills the same panel with the same code (fill_char_panel)
    for sheet in [lobby.sheet(), create.sheet()] {
        let colour = |id: i32| match sheet
            .part(CHAR_WINDOW, id, WidgetKind::Text)
            .map(|p| (p.look.clone(), p.colour_set))
        {
            Some((Look::Text { colour, .. }, true)) => colour,
            other => panic!("text {id}: {other:?}"),
        };
        for id in [9, 11, 13] {
            assert_eq!(colour(id), CHAR_NUMBER_COLOUR);
        }
        assert_eq!(colour(15), art.grade(0).map_or(0xffff_ffff, |r| r.colour));
    }
}

/// The rank threshold of rank 5 (the experience bound of row 4) and its icon, asserted to differ from rank 0's.
fn rank5(art: &PanelArt) -> (u32, String) {
    let e = art.levels.bounds[4];
    assert_eq!(art.levels.level_of(e), 5);
    let icon = art.levels.icon_of_exp(e, "x").unwrap().to_string();
    assert_ne!(Some(icon.as_str()), art.levels.icon_of_exp(0, "x"));
    (e, icon)
}

/// CLI-665: a room slot record (table 3.7b) with the rank word +8 = 0 and the exp +0x42 at rank 5's threshold
/// draws rank 5's icon, also when the member is not in the channel user list (the reference client joining the room).
#[test]
fn rom_room_slot_rank_icon_comes_from_the_slot_exp_not_the_rank_word() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let art = PanelArt::from_rom(&rom).expect("panel art");
    let (e5, i5) = rank5(&art);
    let mut rec = vec![0u8; 0xb8];
    rec[4..8].copy_from_slice(&7u32.to_le_bytes());
    for (k, c) in "Reference".encode_utf16().enumerate() {
        rec[0x10 + 2 * k..0x12 + 2 * k].copy_from_slice(&c.to_le_bytes());
    }
    rec[0x36..0x3a].copy_from_slice(&1i32.to_le_bytes());
    rec[0x42..0x46].copy_from_slice(&e5.to_le_bytes());
    let (id, m) = dc_client::menu::link::member_row(&rec, 0);
    assert_eq!((id, m.exp), (7, e5));
    let mut cat = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
    let data = MenuData {
        profile: LobbyProfile {
            nick: "Me".into(),
            my_id: 2,
            ..LobbyProfile::default()
        },
        members: BTreeMap::from([(id, m)]),
        ..MenuData::default()
    };
    let room = Room::new(&cat, &data);
    let mut items = Vec::new();
    room.draw_extra(&mut cat, &data, 0.0, &mut items);
    assert_eq!(images(&items, &i5), 1, "the member row: rank 5's icon");
}

/// CLI-665: a lobby user record (table 3.6b) with exp +0x2a at rank 5's threshold draws rank 5's icon in the
/// channel user list.
#[test]
fn rom_channel_user_rank_icon_comes_from_the_record_exp() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let art = PanelArt::from_rom(&rom).expect("panel art");
    let (e5, i5) = rank5(&art);
    let mut rec = vec![0u8; 0x42];
    rec[0..4].copy_from_slice(&3u32.to_le_bytes());
    rec[4..8].copy_from_slice(&7u32.to_le_bytes());
    for (k, c) in "Reference".encode_utf16().enumerate() {
        rec[8 + 2 * k..10 + 2 * k].copy_from_slice(&c.to_le_bytes());
    }
    rec[0x2a..0x2e].copy_from_slice(&e5.to_le_bytes());
    let u = dc_client::menu::link::user_row(&rec, 0);
    assert_eq!(u.exp, e5);
    let mut cat = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
    let data = MenuData {
        profile: LobbyProfile {
            nick: "Me".into(),
            my_id: 2,
            ..LobbyProfile::default()
        },
        users: vec![u],
        ..MenuData::default()
    };
    let screen = LobbyScreen::new(&cat, &data);
    let mut items = Vec::new();
    screen.draw_extra(&mut cat, &data, 0.0, &mut items);
    assert_eq!(images(&items, &i5), 1, "the user row: rank 5's icon");
}

/// CLI-1024: the ROM layout carries the user row's NationalFlagPos and the country window's FlagSize (the lobby
/// user list's flag position and size come from them).
#[test]
fn rom_user_row_style_has_the_flag_keys() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let cat = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
    let st = dc_client::menu::lobby::RowStyle::load(&cat);
    println!("flag at {:?} size {:?}", st.user_flag_at, st.flag_size);
    assert_ne!(st.user_flag_at, [0, 0], "NationalFlagPos");
    assert!(st.flag_size[0] > 0 && st.flag_size[1] > 0, "FlagSize");
}
