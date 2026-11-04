//! CLI-1600: the server screen's Character Info panel shows the login reply's record (level, progress, kills, deaths,
//! clan) from the first entry on, and the lobby login's values (nickname, NX, grade, GP) once they are in.
//!
//! ROM-gated (skipped without `CA_ROM`); nothing of the ROM is asserted by value: the expectations are computed
//! from the tables read at run time.

use std::time::{Duration, Instant};

use dc_botclient::lobby::{LobbyProfile, ServerEntry};
use dc_client::menu::link::{CharExtras, LinkEvent, LinkOptions};
use dc_client::menu::lobby::PanelArt;
use dc_client::menu::servers::CHAR_WINDOW;
use dc_client::menu::{Menu, MenuOptions};
use dc_ui_data::WidgetKind;

fn text(m: &Menu, id: i32) -> String {
    m.sheet()
        .part(CHAR_WINDOW, id, WidgetKind::Text)
        .map(|p| p.text.clone())
        .unwrap_or_default()
}

fn wait_screen(m: &mut Menu, name: &str) {
    let end = Instant::now() + Duration::from_secs(5);
    while m.screen_name() != name {
        assert!(Instant::now() < end, "screen {name} not reached");
        m.update();
        std::thread::sleep(Duration::from_millis(5));
    }
    m.update();
}

#[test]
fn rom_the_server_screen_character_info_shows_the_login_record_then_the_lobby_values() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let art = PanelArt::from_rom(&rom).expect("panel art");
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("server-charinfo"),
        rom: Some(rom),
        font: Some((dc_client::testkit::synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    // a level past the first, with a part of the level done
    let exp = art.levels.bounds[1] + 1;
    m.feed_event(LinkEvent::LoginRecord {
        exp,
        clan_id: 0,
        kills: 1234,
        deaths: 56,
    });
    m.feed_event(LinkEvent::Servers(vec![ServerEntry {
        id: 1,
        name: "Server 1".into(),
        ip: "127.0.0.1".into(),
        port: 1,
        open: 1,
        capacity: 100,
        current: 10,
        clan_only: 0,
        op_capacity: 0,
        op_current: 0,
    }]));
    wait_screen(&mut m, "servers");
    // the login reply's record, before any lobby login
    assert_eq!(text(&m, 17), "1234");
    assert_eq!(text(&m, 18), "56");
    assert_eq!(text(&m, 4), art.short_name(exp));
    let (part, span) = art.part_span(exp);
    let pct = f64::from(part) * 100.0 / f64::from(span);
    assert_eq!(text(&m, 16), format!("{pct:3.2} %"));
    assert_eq!(text(&m, 7), m.cat.text(0x41a), "no clan");
    let icon = art.levels.icon_of_exp(exp, "").unwrap().to_string();
    let rank = m.sheet().part(CHAR_WINDOW, 0, WidgetKind::Image).unwrap();
    assert!(
        matches!(&rank.look, dc_client::menu::sheet::Look::Panel { tex, .. } if *tex == icon),
        "rank icon of the experience"
    );
    assert_eq!(text(&m, 3), "", "no nickname before the lobby login");
    // the lobby login's values come in (back on the screen from the channel list, they are there at once)
    m.feed_event(LinkEvent::Balance {
        credit: 3000,
        prepaid: 1500,
    });
    m.feed_event(LinkEvent::CharInfo(CharExtras {
        grade: 1,
        clan_id: 7,
        clan_label: "Clan".into(),
        kills: 1300,
        deaths: 60,
        ..CharExtras::default()
    }));
    m.feed_event(LinkEvent::Profile(LobbyProfile {
        char_no: 9,
        nick: "Hero".into(),
        gp: 123456,
        exp,
        ..LobbyProfile::default()
    }));
    m.update();
    assert_eq!(text(&m, 3), "Hero");
    assert_eq!(text(&m, 15), "123,456");
    assert_eq!(text(&m, 9), "1,500");
    assert_eq!(text(&m, 11), "3,000");
    assert_eq!(text(&m, 7), "Clan");
    assert_eq!(text(&m, 17), "1300");
    assert_eq!(
        text(&m, 13),
        art.grade(1).map_or("", |r| r.name.as_str()),
        "grade name"
    );
}
