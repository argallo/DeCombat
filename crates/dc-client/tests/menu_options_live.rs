//! CLI-1140 / CLI-1141: the Graphic tab's gamma slider moves the frame's gamma at once (Cancel puts it back, OK keeps and
//! saves it), and the Other tab's Block Whispers / Block Invitations switches are acted on. ROM-gated where noted.

use dc_client::menu::link::{LinkEvent, LinkOptions};
use dc_client::menu::options::{CMD_CANCEL, CMD_OK, ROW_CMD_BASE};
use dc_client::menu::{Go, MenuData, MenuInput};
use dc_client::menu::{Menu, MenuOptions};
use dc_client::settings::{ProfileStore, Settings};
use dc_client::testkit::synth_font;

fn menu() -> (Menu, std::path::PathBuf) {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("menu-options-live"),
        rom: dc_rom::resolve_rom().ok(),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    let dir = std::env::temp_dir().join(format!("dc-menu-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("player.txt");
    m.load_settings(ProfileStore {
        read: vec![file.clone()],
        write: Some(file.clone()),
        key: None,
    });
    m.open(Go::Lobby);
    (m, file)
}

#[test]
fn the_gamma_slider_is_live_cancel_restores_and_ok_saves() {
    let (mut m, file) = menu();
    if m.cat.rom.is_none() {
        println!("skipped: the graphic page's rows need the ROM layout");
        return;
    }
    let g = || dc_client::settings::published_gamma().unwrap();
    assert_eq!(m.data.settings.gamma, 22);
    assert!((g() - 1.22).abs() < 1e-4);
    m.input(MenuInput::Command(dc_client::menu::CMD_BAR_PREFS));
    // row 3 (the gamma slider): the minus button
    m.input(MenuInput::Command(ROW_CMD_BASE + 16 * 3));
    assert!(g() < 1.22 - 1e-3, "the slider moved the gamma before OK: {}", g());
    m.input(MenuInput::Command(CMD_CANCEL));
    assert!((g() - 1.22).abs() < 1e-4, "Cancel restores it");
    m.input(MenuInput::Command(dc_client::menu::CMD_BAR_PREFS));
    m.input(MenuInput::Command(ROW_CMD_BASE + 16 * 3));
    let moved = g();
    m.input(MenuInput::Command(CMD_OK));
    assert!((g() - moved).abs() < 1e-6);
    assert_eq!(m.data.settings.gamma_value(), moved);
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(text.contains(&format!("{moved:.2}")), "saved: {text}");
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

#[test]
fn block_whispers_and_invitations_read_the_other_tab_switches() {
    let mut s = Settings::default();
    assert!(!s.block_whispers() && !s.block_invites());
    s.inert.insert("quick.2000".into(), 1);
    s.inert.insert("quick.2002".into(), 1);
    assert!(s.block_whispers() && s.block_invites());
    // the whisper line of another player is dropped, an own one is kept
    let mut d = MenuData::default();
    d.friends.me = "Me".into();
    let whisper = |from: &str, to: &str| {
        LinkEvent::Messenger(
            ["whisper", from, to, "hi"]
                .iter()
                .map(|x| x.to_string())
                .collect(),
        )
    };
    dc_client::menu::messenger::shared_event(&mut d, &whisper("Ann", "Me"));
    assert_eq!(d.chat.len(), 1, "no block: heard");
    d.settings = s;
    dc_client::menu::messenger::shared_event(&mut d, &whisper("Bo", "Me"));
    assert_eq!(d.chat.len(), 1, "blocked");
    dc_client::menu::messenger::shared_event(&mut d, &whisper("Me", "Bo"));
    assert_eq!(d.chat.len(), 2, "the own line goes on");
}

#[test]
fn the_profile_keeps_the_gamma() {
    use dc_client::settings::{CommandTable, ProfileDoc};
    let mut s = Settings::default();
    s.gamma = 40;
    let mut doc = ProfileDoc::default();
    let t = CommandTable::default();
    s.to_doc(&mut doc, &t, &|_| String::new());
    let back = Settings::from_doc(&doc, &t, &|_| String::new());
    assert_eq!(back.gamma, 40);
}
