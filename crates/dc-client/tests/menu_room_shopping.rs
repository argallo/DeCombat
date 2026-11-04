//! lobby-user-state (CLI-1560 / CLI-1561): a waiting-room member who opens the shop is shown "shopping" to the room.
//! 2009: the room's bar commands 0x3d / 0x8e / 0xa8 / 0xb8 un-ready the member (cshell+0x15d610) and send 0x392 (1, 1)
//! (cshell+0x164810); the shop screen's focus sends 0x306 {2} and, in a room, 0x30a {1}, its loss 0x30c
//! (cshell+0x14f300); the room's focus from screen 0x14 / 0x16 / 0x1c sends 0x306 {1} and 0x392 (1, 0)
//! (cshell+0x164020). The other members draw the state text `res 0x456` for state 0 with sub-state 1
//! (cshell+0x163d00). The channel lobby's user rows carry no state field (0x371 record, cshell+0x1220e0 ->
//! cshell+0x120d90), so the lobby list shows none. The commands run without the ROM; the drawn texts need `CA_ROM`.
//! With `DC_SHOT_DIR` set the ROM test saves the room and lobby pictures there (room-state and lobby-state PNGs).

use std::path::PathBuf;

use dc_botclient::lobby::{LobbyProfile, RoomMember};
use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{LinkEvent, LinkOptions, RoomView, UserRow};
use dc_client::menu::room::{member_state_text, SUB_SHOPPING};
use dc_client::menu::shop::{CMD_BAR_MARKET, CMD_BAR_SHOP, CMD_BAR_TUNING, CMD_EXIT};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::testkit::synth_font;

fn menu(rom: Option<PathBuf>) -> Menu {
    let font = if rom.is_some() {
        None
    } else {
        Some((synth_font(), 0))
    };
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "ShopUser".into(),
        link: LinkOptions::new("room-shopping"),
        rom,
        font,
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m
}

/// A room of two (the own player 1, a guest; the host 2), the own state `own`.
fn room(m: &mut Menu, own: i32) {
    let d = &mut m.data;
    d.profile = LobbyProfile {
        nick: "ShopUser".into(),
        my_id: 1,
        ..LobbyProfile::default()
    };
    d.room = RoomView {
        title: "shop".into(),
        host_id: 2,
        map: 2,
        mode: 1,
        players: 2,
        max_players: 8,
        goal: 50,
        ..RoomView::default()
    };
    for (id, team, state, name) in [(1u32, 1, own, "ShopUser"), (2, 2, 0, "Host")] {
        d.members.insert(
            id,
            RoomMember {
                char_no: id as i32,
                name: name.into(),
                team,
                state,
                exp: 0,
            },
        );
    }
    m.open(Go::Room);
}

/// The state requests the detached link kept (ready, shopping, location, private state; the screens' list
/// requests left out).
fn state_sent(m: &Menu) -> Vec<String> {
    m.take_link_sent()
        .into_iter()
        .filter(|s| {
            ["Ready", "Shopping", "Location", "PrivateState"]
                .iter()
                .any(|p| s.starts_with(p))
        })
        .collect()
}

/// Runs a command and the screen changes it asks for.
fn command(m: &mut Menu, c: i32) {
    m.input(MenuInput::Command(c));
    for _ in 0..4 {
        m.update();
    }
}

#[test]
fn a_ready_member_leaving_for_the_shop_unreadies_and_flags_shopping_and_clears_it_on_return() {
    let mut m = menu(None);
    room(&mut m, 1);
    m.take_link_sent();
    command(&mut m, CMD_BAR_SHOP);
    assert_eq!(m.screen_name(), "shop");
    assert_eq!(
        state_sent(&m),
        vec![
            "Ready(false)",
            "Shopping(true)",
            "Location(2)",
            "PrivateState(1)"
        ]
    );
    // the shop's Exit goes back to the room: the private state ends, location 1, present again
    command(&mut m, CMD_EXIT);
    assert_eq!(m.screen_name(), "room");
    assert_eq!(
        state_sent(&m),
        vec!["PrivateStateEnd", "Location(1)", "Shopping(false)"]
    );
}

#[test]
fn modification_flags_shopping_but_the_black_market_does_not() {
    let mut m = menu(None);
    room(&mut m, 0);
    m.take_link_sent();
    command(&mut m, CMD_BAR_TUNING);
    assert_eq!(
        state_sent(&m),
        vec!["Shopping(true)", "Location(3)", "PrivateState(3)"]
    );
    command(&mut m, CMD_EXIT);
    m.take_link_sent();
    // 622 is not one of the 2009 room's shopping commands; the shop screen's focus messages still go out
    command(&mut m, CMD_BAR_MARKET);
    assert_eq!(state_sent(&m), vec!["Location(2)", "PrivateState(1)"]);
}

#[test]
fn a_shopping_member_shows_the_shopping_state_text() {
    assert_eq!(member_state_text(0, 1), Some(SUB_SHOPPING));
    // a ready member shows ready, whatever the sub-state
    assert_ne!(member_state_text(1, 1), Some(SUB_SHOPPING));
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let mut m = menu(Some(rom));
    room(&mut m, 0);
    let shopping = m.cat.text(SUB_SHOPPING);
    assert!(!shopping.is_empty());
    let texts = |m: &mut Menu| -> Vec<String> {
        m.items()
            .iter()
            .filter_map(|i| match i {
                DrawItem::Label { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    assert!(!texts(&mut m).contains(&shopping));
    // the host goes shopping: 0x3042 {2, 0, 1}
    m.feed_event(LinkEvent::MemberState {
        id: 2,
        state: 0,
        sub: 1,
    });
    m.update();
    assert!(texts(&mut m).contains(&shopping), "{:?}", texts(&mut m));
    if let Some(dir) = std::env::var_os("DC_SHOT_DIR") {
        let save = |m: &mut Menu, name: &str| {
            let i = m.image();
            let f = dc_render::FrameImage {
                width: i.width,
                height: i.height,
                rgba: i.rgba,
            };
            let _ = f.save_png(&std::path::Path::new(&dir).join(name));
        };
        save(&mut m, "room-state.png");
        // the channel lobby: two users, one of them in the shop; 2009 draws no state in these rows
        m.data.users = vec![
            UserRow {
                account: 1,
                id: 1,
                name: "ShopUser".into(),
                ..UserRow::default()
            },
            UserRow {
                account: 3,
                id: 3,
                name: "InShop".into(),
                ..UserRow::default()
            },
        ];
        m.open(Go::Lobby);
        m.update();
        save(&mut m, "lobby-state.png");
    }
    // back: the sub-state 0 removes the text
    m.open(Go::Room);
    m.feed_event(LinkEvent::MemberState {
        id: 2,
        state: 0,
        sub: 0,
    });
    m.update();
    assert!(!texts(&mut m).contains(&shopping));
}
