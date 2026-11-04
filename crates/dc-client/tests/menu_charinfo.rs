//! CLI-931: a profile reply opens the Character Info box (its own screen over the lobby), not a message box; the
//! box shows the rank, experience, kills / deaths / headshots and clan lines, and its tab switches to the fireteam
//! lines (cshell+0x35cf0, cshell+0x2412c0).

use std::path::PathBuf;

use dc_botclient::lobby::LobbyProfile;
use dc_client::hud::items::DrawItem;
use dc_client::menu::charinfo;
use dc_client::menu::link::{LinkEvent, LinkOptions, ProfileView};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::testkit::synth_font;

fn rom() -> Option<PathBuf> {
    std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

fn texts(m: &mut Menu) -> Vec<String> {
    m.items()
        .into_iter()
        .filter_map(|i| match i {
            DrawItem::Label { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn ordinals_and_ratio_follow_the_2009_rules() {
    assert_eq!(charinfo::ordinal(1), "1st");
    assert_eq!(charinfo::ordinal(2), "2nd");
    assert_eq!(charinfo::ordinal(3), "3rd");
    assert_eq!(charinfo::ordinal(4), "4th");
    assert_eq!(charinfo::ordinal(11), "11th");
    assert_eq!(charinfo::ordinal(12), "12th");
    assert_eq!(charinfo::ordinal(13), "13th");
    assert_eq!(charinfo::ordinal(21), "21st");
    assert_eq!(charinfo::ratio(0, 5), 1.0);
    assert_eq!(charinfo::ratio(7, 0), 7.0);
    assert_eq!(charinfo::ratio(3, 2), 1.5);
}

#[test]
fn rom_a_profile_reply_opens_the_character_info_box() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "casey".into(),
        link: LinkOptions::new("charinfo"),
        rom: Some(rom),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.data.profile = LobbyProfile {
        nick: "casey".into(),
        my_id: 1,
        exp: 225_000,
        ..LobbyProfile::default()
    };
    m.open(Go::Lobby);
    m.feed_event(LinkEvent::ProfileInfo(ProfileView {
        name: "Bot1".into(),
        exp: 225_000,
        kills: 120,
        deaths: 80,
        headshots: 30,
        rank: 2,
        clan: "Zed".into(),
        clan_id: 3,
        coop: [10, 20, 3, 4, 50, 40, 1, 2, 3, 4, 5, 6],
        ..ProfileView::default()
    }));
    m.update();
    assert_eq!(m.screen_name(), charinfo::NAME, "the reply opens the box");
    assert!(m.popup().is_none(), "no message box");
    let t = texts(&mut m);
    for want in [
        "Bot1",
        "Zed",
        "225000 exp",
        "2nd",
        "-",
        "120 Kills (1.50 K/D Ratio)",
        "80 Deaths",
        "30 / 120 (25.00 %)",
        "Default Info",
        "Fireteam Info",
        "OK",
    ] {
        assert!(t.iter().any(|x| x == want), "{want:?} not in {t:?}");
    }
    assert!(!t.iter().any(|x| x.contains("Participation")));
    // the fireteam tab: its lines replace the default page's
    let tab = [450.0, 132.0];
    m.input(MenuInput::Move(tab));
    m.input(MenuInput::Press(tab));
    m.input(MenuInput::Release(tab));
    let t = texts(&mut m);
    for want in [
        "Total 50 times / Cleared 40 times",
        "10 Kills",
        "20 Deaths",
        "3 times / 10 times (30.00%)",
        "4 Combos",
    ] {
        assert!(t.iter().any(|x| x == want), "{want:?} not in {t:?}");
    }
    assert!(!t.iter().any(|x| x == "120 Kills (1.50 K/D Ratio)"));
    // OK closes the box
    let ok = [513.0, 575.0];
    m.input(MenuInput::Move(ok));
    m.input(MenuInput::Press(ok));
    m.input(MenuInput::Release(ok));
    assert_eq!(m.screen_name(), "lobby");
}

/// CLI-961: the nation flag and its name sit after the player's name, the name takes the colour table's colour, and a
/// clan with both mark layers draws its mark.
#[test]
fn rom_the_box_draws_the_flag_the_name_colour_and_the_clan_mark() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "casey".into(),
        link: LinkOptions::new("charinfo"),
        rom: Some(rom),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.open(Go::Lobby);
    m.feed_event(LinkEvent::ProfileInfo(ProfileView {
        name: "Bot1".into(),
        exp: 225_000,
        clan: "Zed".into(),
        clan_id: 3,
        emblem: 1,
        background: 1,
        name_colour: 3,
        nation: 12,
        ..ProfileView::default()
    }));
    m.update();
    let items = m.items();
    let flags = items
        .iter()
        .filter(|i| matches!(i, DrawItem::Image { tex, .. } if tex.contains("Flag")))
        .count();
    assert_eq!(flags, 1, "the flag of the nation");
    let t = texts(&mut m);
    assert!(t.iter().any(|x| x.ends_with("(CZE)")), "{t:?}");
    // the name in the colour table's third colour (0, 128, 128)
    let name = items.iter().find_map(|i| match i {
        DrawItem::Label { style, text, .. } if text == "Bot1" => Some(style.colour),
        _ => None,
    });
    assert_eq!(name, Some(dc_client::menu::catalog::argb(0xff00_8080)));
    // the clan mark: two layers of the mark textures
    let marks = items
        .iter()
        .filter(
            |i| matches!(i, DrawItem::Image { dst, .. } if dst[..] == [451.0, 275.0, 26.0, 26.0]),
        )
        .count();
    assert_eq!(marks, 2, "background and emblem");
}
