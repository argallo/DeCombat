//! Renders every menu screen with made-up lobby data into PNGs (layout check without a server).
//!
//! ```text
//! cargo run -p dc-client --example menu_gallery -- --rom <CA_ROM> --out <dir> [--size WxH]
//! ```
//! The images are drawn from the user's ROM: local files, never committed.

use dc_botclient::lobby::{LobbyProfile, RoomMember, ServerEntry};
use dc_client::menu::link::{ChannelRow, LinkOptions, RoomRow, RoomView, UserRow};
use dc_client::menu::{Go, Menu, MenuOptions};
use std::path::PathBuf;

fn main() {
    let mut rom = None;
    let mut out = PathBuf::from("menu_gallery");
    let mut size = (1024u32, 768u32);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--rom" => rom = it.next().map(PathBuf::from),
            "--out" => out = PathBuf::from(it.next().unwrap_or_default()),
            "--size" => {
                let v = it.next().unwrap_or_default();
                if let Some((w, h)) = v.split_once('x') {
                    size = (w.parse().unwrap_or(1024), h.parse().unwrap_or(768));
                }
            }
            _ => {}
        }
    }
    let rom = rom.or_else(|| dc_rom::resolve_rom().ok());
    let mut menu = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "casey".into(),
        link: LinkOptions::new("gallery"),
        rom,
        font: Some((dc_client::testkit::synth_font(), 0)),
        width: size.0,
        height: size.1,
    });
    println!("{}", menu.source());
    let d = &mut menu.data;
    d.servers = (0..3)
        .map(|i| ServerEntry {
            id: i + 1,
            name: format!("DeCombat {}", i + 1),
            ip: "127.0.0.1".into(),
            port: 1,
            open: 1,
            capacity: 100,
            current: [10, 60, 97][i as usize],
            clan_only: 0,
            op_capacity: 0,
            op_current: 0,
        })
        .collect();
    d.server = d.servers.first().cloned();
    d.profile = LobbyProfile {
        nick: "casey".into(),
        char_no: 1,
        gp: 500000,
        exp: 225000,
        my_id: 1,
        chan_label: "Channel 1".into(),
        ..LobbyProfile::default()
    };
    d.channels = (0..3)
        .map(|i| ChannelRow {
            id: i,
            name: format!("Channel {}", i + 1),
            open: 1,
            capacity: 100,
            current: [5, 50, 90][i as usize],
            op_capacity: 0,
            op_current: 0,
        })
        .collect();
    d.rooms = vec![
        RoomRow {
            number: 0,
            title: "casey room".into(),
            map: 2,
            mode: 0,
            players: 2,
            max_players: 8,
            ..RoomRow::default()
        },
        RoomRow {
            number: 1,
            title: "locked room".into(),
            locked: true,
            map: 0,
            mode: 1,
            players: 6,
            max_players: 16,
            state: 2,
            ..RoomRow::default()
        },
    ];
    d.users = vec![
        UserRow {
            account: 1,
            id: 1,
            name: "casey".into(),
            exp: 0,
            ..UserRow::default()
        },
        UserRow {
            account: 2,
            id: 2,
            name: "Bot1".into(),
            exp: 0,
            ..UserRow::default()
        },
    ];
    d.room = RoomView {
        number: 0,
        title: "casey room".into(),
        host_id: 1,
        map: 2,
        mode: 0,
        players: 2,
        max_players: 8,
        goal: 100,
        ..RoomView::default()
    };
    d.members.insert(
        1,
        RoomMember {
            char_no: 1,
            name: "casey".into(),
            team: 1,
            state: 0,
            exp: 0,
        },
    );
    d.members.insert(
        2,
        RoomMember {
            char_no: 2,
            name: "Bot1".into(),
            team: 2,
            state: 1,
            exp: 0,
        },
    );
    d.chat = vec!["Bot1 : hello".into()];
    // the result screen's made-up match (dc_client::matchexit::MatchResult::sample)
    d.result = Some(dc_client::matchexit::MatchResult::sample());
    std::fs::create_dir_all(&out).unwrap();
    for (name, g) in [
        ("login", Go::Login),
        ("servers", Go::Servers),
        ("channels", Go::Channels),
        ("lobby", Go::Lobby),
        ("create", Go::Create),
        ("room", Go::Room),
        ("result", Go::Result),
    ] {
        menu.open(g);
        let img = menu.image();
        let p = out.join(format!("gallery_{name}.png"));
        dc_render::FrameImage {
            width: img.width,
            height: img.height,
            rgba: img.rgba,
        }
        .save_png(&p)
        .unwrap();
        println!("{}", p.display());
    }
    // CLI-1130 / CLI-1132: the message boxes over the lobby
    {
        use dc_client::menu::popup::{BoxTag, Popup};
        for (name, p) in [
            (
                "box_yesno",
                Popup::ask(
                    "A placeholder question that is long enough to wrap onto a second line of the box, to compare the wrap.".into(),
                    BoxTag::None,
                ),
            ),
            (
                "box_limit",
                Popup::ask(
                    "You do not have a Sniper equipped
Equip one in the shop".into(),
                    BoxTag::LimitBox,
                )
                .limit_buttons(),
            ),
        ] {
            menu.open(Go::Lobby);
            menu.show_popup(p);
            let img = menu.image();
            let p = out.join(format!("gallery_{name}.png"));
            dc_render::FrameImage {
                width: img.width,
                height: img.height,
                rgba: img.rgba,
            }
            .save_png(&p)
            .unwrap();
            println!("{}", p.display());
        }
    }
}
