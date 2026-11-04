//! Writes the draw list of every front-end screen with its provenance (`drawlist_<screen>.jsonl`), the structural
//! mismatches against the ROM layout pages (`mismatch_<screen>.jsonl`), a per-screen count (`summary.json`) and the
//! screen's CPU image (`shot_<screen>.png`), with the same made-up lobby data as `menu_gallery`.
//!
//! ```text
//! cargo run -p dc-client --example menu_drawlist -- --rom <CA_ROM> --out <dir> [--size WxH] [--with-text]
//!     [--pointer X,Y]
//! ```
//! Screens: login, servers, channels, lobby, create, room, result, popup (a box over the lobby), clan (w3-clan) and
//! messenger (the messenger window over the lobby, w3-clan).
//! The menu's cursor image is drawn at `--pointer` (virtual pixels, given as a pointer move before every screen),
//! else at the menu's default pointer (the screen centre, DC-DEFAULT CLI-482) with no pointer input.
//! The rows hold no ROM text unless `--with-text` is given (local diagnosis; never commit such output).

use dc_botclient::lobby::{LobbyProfile, RoomMember, ServerEntry};
use dc_client::menu::drawlist::{check, mismatch_json, row_json, FontTable, TraceOptions};
use dc_client::menu::link::{
    ChannelRow, ClanInfoView, ClanMemberRow, LinkOptions, RoomRow, RoomView, UserRow,
};
use dc_client::menu::popup::{BoxTag, Popup};
use dc_client::menu::{Go, Menu, MenuOptions};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let mut rom = None;
    let mut out = PathBuf::from("menu_drawlist");
    let mut size = (1024u32, 768u32);
    let mut opts = TraceOptions::default();
    let mut pointer: Option<[f32; 2]> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--rom" => rom = it.next().map(PathBuf::from),
            "--out" => out = PathBuf::from(it.next().unwrap_or_default()),
            "--with-text" => opts.with_text = true,
            "--pointer" => {
                let v = it.next().unwrap_or_default();
                pointer = v
                    .split_once(',')
                    .and_then(|(x, y)| Some([x.parse().ok()?, y.parse().ok()?]));
            }
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
    // w3-clan: a made-up clan (record, two members, notice, a chat line) and a friend list
    d.char_extras.clan_id = 7;
    d.char_extras.clan_label = "Testers".into();
    d.char_extras.clan_role = 5;
    d.clan.info = Some(ClanInfoView {
        number: 7,
        max_members: 120,
        members: 2,
        prev_rank: 4,
        rank: 3,
        level: 2,
        wins: 5,
        losses: 2,
        draws: 1,
        exp: 1500,
        points: 40,
        name: "Testers".into(),
        ..ClanInfoView::default()
    });
    d.clan.members = vec![
        ClanMemberRow {
            char_no: 1,
            role: 5,
            exp: 225000,
            name: "casey".into(),
            online: true,
        },
        ClanMemberRow {
            char_no: 2,
            role: 1,
            exp: 0,
            name: "Bot1".into(),
            online: false,
        },
    ];
    d.clan.notice = "clan night at eight".into();
    d.clan.say("Bot1", "hello clan");
    for line in [
        vec!["friends", "casey", "Bot1", "Bot2"],
        vec!["online", "Bot1", "1"],
        vec!["request", "Bot3"],
    ] {
        d.friends
            .apply(&line.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    }
    // the result screen's made-up match (dc_client::matchexit::MatchResult::sample)
    d.result = Some(dc_client::matchexit::MatchResult::sample());
    // w3-shop: the shop shows the catalogue's own list (no lobby here); a made-up inventory (two weapons, one
    // equipped), balance and inbox
    let wares = dc_client::menu::storage::Wares::cached(menu.cat.rom.as_deref());
    let d = &mut menu.data;
    d.store.view = Some(wares.catalogue_ids());
    d.store.lists_asked = true;
    let weapon = |uid: u32, item_no: u32| dc_client::menu::link::InvRecord {
        uid,
        item_no,
        slot_kind: 1,
        ..Default::default()
    };
    d.store.items = vec![weapon(1, 1), weapon(2, 2)];
    d.store.equipped = [-1; 15];
    d.store.equipped[0] = 1;
    d.balance = (1200, 300);
    d.store.inbox = vec![dc_client::menu::link::InboxRow {
        entry: 1,
        product: wares.catalogue_ids().first().copied().unwrap_or(0),
        gift_kind: 1,
        sender: "Bot1".into(),
        message: "gg".into(),
        ..Default::default()
    }];
    d.store.inbox_total = 1;
    d.store.inbox_fetched = true;
    std::fs::create_dir_all(&out).unwrap();
    let fonts = FontTable::from_catalog(&menu.cat);
    std::fs::write(
        out.join("fonts.json"),
        serde_json::to_string_pretty(&fonts.json(opts)).unwrap(),
    )
    .unwrap();
    let mut summary = BTreeMap::new();
    d.masks = (0..8).map(|i| (i, 0xffff)).collect();
    // the last two: a screen with an open drop-down list (w2-uisound), opened by a click on its button
    let screens: [(&str, Go, bool, Option<i32>); 20] = [
        ("login", Go::Login, false, None),
        ("servers", Go::Servers, false, None),
        ("channels", Go::Channels, false, None),
        ("lobby", Go::Lobby, false, None),
        ("create", Go::Create, false, None),
        ("room", Go::Room, false, None),
        ("result", Go::Result, false, None),
        ("popup", Go::Lobby, true, None),
        (
            "room_drop",
            Go::Room,
            false,
            Some(dc_client::menu::room::CMD_GOAL),
        ),
        (
            "create_drop",
            Go::Create,
            false,
            Some(dc_client::menu::create::CMD_MODE_LIST),
        ),
        // w3-shop: the shop & storage screen on its weapons tab, black market tab, storage page and inbox page; the
        // modification screen
        ("shop", Go::Shop, false, None),
        ("market", Go::Market, false, None),
        ("storage", Go::Shop, false, None),
        ("inbox", Go::Inbox, false, None),
        ("tuning", Go::Tuning, false, None),
        // w3-options: the boxes the top bar opens over the lobby
        ("options", Go::Options, false, None),
        ("report", Go::Report, false, None),
        ("help", Go::Help, false, None),
        // w3-clan: the clan screen and the messenger window over the lobby
        ("clan", Go::Clan, false, None),
        ("messenger", Go::Lobby, false, None),
    ];
    for (name, g, popup, open_list) in screens {
        if g.is_overlay() {
            menu.open(Go::Lobby);
        }
        // the clan mark (emblem 1 over background 1) only on the clan screen: the other screens keep their data
        let mark = u32::from(name == "clan");
        menu.data.char_extras.clan_mark = mark;
        menu.data.char_extras.clan_mark_ground = mark;
        menu.open(g);
        if popup {
            let t = menu.cat.text(1);
            menu.show_popup(Popup::ok(t, BoxTag::None));
        }
        if let Some(b) = open_list.and_then(|c| menu.sheet().button(c).map(|b| b.rect())) {
            let c = [
                b[0] as f32 + b[2] as f32 / 2.0,
                b[1] as f32 + b[3] as f32 / 2.0,
            ];
            menu.input(dc_client::menu::MenuInput::Move(c));
            menu.input(dc_client::menu::MenuInput::Press(c));
            menu.input(dc_client::menu::MenuInput::Release(c));
            menu.take_sound_cues();
        }
        if name == "storage" {
            // a press on the left pane's first equipment row opens the storage page
            if let Some(row) = menu
                .sheet()
                .part(
                    dc_client::menu::shop::LEFT_WINDOW,
                    dc_client::menu::shop::WEAPON_LIST,
                    dc_ui_data::WidgetKind::List,
                )
                .map(|l| [l.pos[0] as f32 + 10.0, l.pos[1] as f32 + 10.0])
            {
                menu.input(dc_client::menu::MenuInput::Press(row));
                menu.input(dc_client::menu::MenuInput::Release(row));
            }
        }
        if (name == "messenger") != menu.messenger_open() {
            menu.toggle_messenger();
        }
        if let Some(p) = pointer {
            menu.input(dc_client::menu::MenuInput::Move(p));
        }
        let rows = menu.draw_trace();
        let sheet = menu.trace_sheet().clone();
        let bad = check(&rows, &sheet, &fonts, &mut menu.cat);
        let mut f = std::fs::File::create(out.join(format!("drawlist_{name}.jsonl"))).unwrap();
        for r in &rows {
            writeln!(f, "{}", row_json(name, r, opts)).unwrap();
        }
        let mut f = std::fs::File::create(out.join(format!("mismatch_{name}.jsonl"))).unwrap();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for m in &bad {
            writeln!(f, "{}", mismatch_json(name, m)).unwrap();
            *counts.entry(m.class).or_default() += 1;
        }
        let img = menu.image();
        dc_render::FrameImage {
            width: img.width,
            height: img.height,
            rgba: img.rgba,
        }
        .save_png(&out.join(format!("shot_{name}.png")))
        .unwrap();
        println!(
            "{name}: {} items, {} mismatches {:?}",
            rows.len(),
            bad.len(),
            counts
        );
        summary.insert(
            name.to_string(),
            serde_json::json!({"items": rows.len(), "mismatches": bad.len(), "classes": counts}),
        );
    }
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_string_pretty(
            &serde_json::json!({"size": [size.0, size.1], "screens": summary}),
        )
        .unwrap(),
    )
    .unwrap();
}
