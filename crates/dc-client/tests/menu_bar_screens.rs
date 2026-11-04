//! (r10a) The bars on every front-end screen: messenger, option, report, help; the chat combos.
//! Was: The top bar's option, report and help buttons (w3-options): each opens its box over the screen it was pressed on
//! (the lobby here), the box closes again, the options' OK saves the profile and changes the settings in effect, the
//! report box refuses empty entries and sends one report, and a key box takes the next key. Headless: the menu is
//! driven by inputs; with `CA_ROM` the screens use the ROM layouts and the buttons are pressed by their rectangles,
//! without it by their command numbers. Nothing of the ROM is in this file.

use dc_botclient::lobby::LobbyProfile;
use dc_client::menu::help;
use dc_client::menu::link::{LinkOptions, UserRow};
use dc_client::menu::options::{CMD_CANCEL, CMD_OK, CMD_TABS, ROW_CMD_BASE};
use dc_client::menu::report;
use dc_client::menu::{
    Go, Menu, MenuInput, MenuKey, MenuOptions, CMD_BAR_ABUSE, CMD_BAR_ASSIST, CMD_BAR_PREFS,
};
use dc_client::settings::{ProfileStore, Settings};
use dc_client::sound::SoundCue;
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;
use std::path::PathBuf;

fn menu() -> (Menu, PathBuf) {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("menu-options"),
        rom: dc_rom::resolve_rom().ok(),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.data.profile = LobbyProfile {
        nick: "tester".into(),
        char_no: 1,
        my_id: 1,
        chan_label: "Channel 1".into(),
        ..LobbyProfile::default()
    };
    m.data.users = vec![
        UserRow {
            account: 1,
            id: 1,
            name: "tester".into(),
            exp: 0,
            ..Default::default()
        },
        UserRow {
            account: 2,
            id: 2,
            name: "Bot1".into(),
            exp: 0,
            ..Default::default()
        },
    ];
    let dir = std::env::temp_dir().join(format!(
        "dc-menu-options-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("player.txt");
    // the command table comes from the ROM here, as it does at start-up
    m.load_settings(ProfileStore {
        read: vec![file.clone()],
        write: Some(file.clone()),
        key: None,
    });
    m.open(Go::Lobby);
    (m, file)
}

/// Presses the button with `cmd` on the current screen: by its rectangle when the screen has one, else by command.
fn press(m: &mut Menu, cmd: i32) {
    let s = m.sheet();
    let at = (0..s.parts.len()).find(|&i| {
        s.parts[i].command == cmd && s.parts[i].kind == WidgetKind::Button && s.visible(i)
    });
    match at {
        Some(i) => {
            let r = m.sheet().parts[i].hit.unwrap_or_else(|| {
                let p = &m.sheet().parts[i];
                [p.pos[0], p.pos[1], p.size[0], p.size[1]]
            });
            let c = [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32];
            m.input(MenuInput::Move(c));
            m.input(MenuInput::Press(c));
            m.input(MenuInput::Release(c));
        }
        None => m.input(MenuInput::Command(cmd)),
    }
}

fn overlay_names(m: &Menu) -> &'static str {
    m.screen_name()
}

#[test]
fn the_messenger_button_opens_the_window_on_every_page_that_has_it() {
    for g in [
        Go::Lobby,
        Go::Create,
        Go::Room,
        Go::Shop,
        Go::Market,
        Go::Inbox,
        Go::Tuning,
        Go::Clan,
    ] {
        let (mut m, _) = menu();
        m.open(g);
        let base = m.screen_name();
        press(&mut m, 310);
        assert!(m.messenger_open(), "messenger opens on {g:?}");
        assert_eq!(m.screen_name(), base);
        m.input(MenuInput::Key(MenuKey::Escape));
        assert!(!m.messenger_open(), "and closes again on {g:?}");
    }
}

#[test]
fn the_option_report_and_help_buttons_open_on_every_page_that_has_them() {
    for g in [
        Go::Channels,
        Go::Lobby,
        Go::Create,
        Go::Room,
        Go::Shop,
        Go::Market,
        Go::Inbox,
        Go::Tuning,
        Go::Clan,
    ] {
        for (cmd, name) in [
            (CMD_BAR_PREFS, "options"),
            (CMD_BAR_ABUSE, "report"),
            (CMD_BAR_ASSIST, "help"),
        ] {
            let (mut m, _) = menu();
            m.open(g);
            let base = m.screen_name();
            press(&mut m, cmd);
            assert_eq!(overlay_names(&m), name, "button {cmd} on {g:?}");
            assert!(m.overlay_open());
            m.input(MenuInput::Key(MenuKey::Escape));
            assert_eq!(m.screen_name(), base, "{name} closes back to {g:?}");
        }
    }
}

#[test]
fn the_chat_combos_open_their_lists_and_take_a_pick() {
    // needs the ROM's screen layouts (or body models): CI has none
    if dc_rom::resolve_rom().is_err() {
        println!("skipped: CA_ROM unset");
        return;
    }
    use dc_client::menu::chatdrop;
    let (mut m, _) = menu();
    m.open(Go::Lobby);
    // the filter combo: a press on its arrow shows three rows, the third is "Clan Chat Only"
    press(&mut m, chatdrop::CMD_FILTER);
    let d = m.dropdown().expect("the filter list is open");
    assert_eq!(d.items.len(), 3);
    assert_eq!(d.items[2], "Clan Chat Only");
    let at = d.row_point(2).unwrap();
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
    assert!(m.dropdown().is_none());
    assert_eq!(m.data.chat_filter, chatdrop::FILTER_CLAN);
    let s = m.sheet();
    assert!(s.parts.iter().any(|p| p.kind == WidgetKind::Text
        && p.command == chatdrop::CMD_FILTER
        && p.text == "Clan Chat Only"));
    // pressing the caption toggles the list (open, closed)
    let a = s
        .parts
        .iter()
        .find(|p| p.kind == WidgetKind::Text && p.command == chatdrop::CMD_FILTER)
        .unwrap()
        .pos;
    let c = [(a[0] + 20) as f32, (a[1] + 5) as f32];
    for open in [true, false] {
        m.input(MenuInput::Press(c));
        m.input(MenuInput::Release(c));
        assert_eq!(m.dropdown().is_some(), open);
    }
    // the target combo: the second row is the clan chat
    press(&mut m, chatdrop::CMD_TARGET);
    let d = m.dropdown().expect("the target list is open");
    assert_eq!(d.items.len(), 2);
    let at = d.row_point(1).unwrap();
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
    assert_eq!(m.data.chat_target, chatdrop::TARGET_CLAN);
    // the caption is drawn from the picked row
    let shown = format!("{:?}", m.items());
    assert!(
        shown.contains("Clan chat (F3)"),
        "the target caption shows the pick"
    );
    assert!(
        shown.contains("Clan Chat Only"),
        "the filter caption shows the pick"
    );
}

/// CLI-1173 (playtest report: "switching to clan or channel chat does not change"): F2 / F3 set the chat target in the
/// lobby and the waiting room, as the target combo's rows do (cshell+0x205e0), and the combo caption follows; with
/// the filter on one kind of line the keys do nothing.
#[test]
fn f2_and_f3_switch_the_chat_target() {
    // needs the ROM's screen layouts (or body models): CI has none
    if dc_rom::resolve_rom().is_err() {
        println!("skipped: CA_ROM unset");
        return;
    }
    use dc_client::menu::{chatdrop, MenuKey};
    for go in [Go::Lobby, Go::Room] {
        let (mut m, _) = menu();
        m.open(go);
        m.input(MenuInput::Key(MenuKey::F3));
        assert_eq!(m.data.chat_target, chatdrop::TARGET_CLAN, "{go:?}");
        let caption = |m: &Menu| {
            m.sheet()
                .parts
                .iter()
                .find(|p| p.kind == WidgetKind::Text && p.command == chatdrop::CMD_TARGET)
                .map(|p| p.text.clone())
                .unwrap_or_default()
        };
        let clan = caption(&m);
        m.input(MenuInput::Key(MenuKey::F2));
        assert_eq!(m.data.chat_target, chatdrop::TARGET_CHANNEL, "{go:?}");
        assert_ne!(caption(&m), clan, "the caption follows the key");
        m.data.chat_filter = chatdrop::FILTER_CLAN;
        m.input(MenuInput::Key(MenuKey::F3));
        assert_eq!(
            m.data.chat_target,
            chatdrop::TARGET_CHANNEL,
            "filtered: no change"
        );
    }
}

/// CLI-941: the waiting room's chat combos open their lists and take a pick like the lobby's (they were drawn only).
#[test]
fn the_waiting_room_chat_combos_open_their_lists_and_take_a_pick() {
    // needs the ROM's screen layouts (or body models): CI has none
    if dc_rom::resolve_rom().is_err() {
        println!("skipped: CA_ROM unset");
        return;
    }
    use dc_client::menu::chatdrop;
    let (mut m, _) = menu();
    m.open(Go::Room);
    press(&mut m, chatdrop::CMD_FILTER);
    let d = m.dropdown().expect("the filter list is open in the room");
    assert_eq!(d.items.len(), 3);
    let at = d.row_point(1).unwrap();
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
    assert!(m.dropdown().is_none());
    assert_eq!(m.data.chat_filter, chatdrop::FILTER_CHANNEL);
    // the caption is the picked row's; a second press on the same combo hides its list
    press(&mut m, chatdrop::CMD_TARGET);
    assert!(m.dropdown().is_some());
    press(&mut m, chatdrop::CMD_TARGET);
    assert!(
        m.dropdown().is_none(),
        "a press on the shown combo hides it"
    );
    press(&mut m, chatdrop::CMD_TARGET);
    let d = m.dropdown().expect("the target list is open");
    assert_eq!(d.items.len(), 2);
    let at = d.row_point(1).unwrap();
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
    assert_eq!(m.data.chat_target, chatdrop::TARGET_CLAN);
    let shown = format!("{:?}", m.items());
    assert!(
        shown.contains("Clan chat (F3)"),
        "the room's caption shows the pick"
    );
}

/// CLI-1021: the bar's clan button (168) opens the clan screen from the shop family's pages and the tuning page too.
#[test]
fn the_clan_button_opens_the_clan_screen_on_the_shop_family_pages() {
    for g in [Go::Lobby, Go::Shop, Go::Market, Go::Inbox, Go::Tuning] {
        let (mut m, _) = menu();
        m.open(g);
        press(&mut m, dc_client::menu::clan::BAR_CLAN_CMD);
        for _ in 0..200 {
            m.update();
            if m.screen_name() == "clan" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(m.screen_name(), "clan", "the clan button on {g:?}");
    }
}
