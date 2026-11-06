//! M11 / M27 (w2-uisound): the front end's interface sounds and its drop-down lists, driven only through pointer
//! input on the ROM layout's real rectangles.
//!
//! - a hover change onto a button plays the select-change sound, a click the select sound (the server screen's
//!   common window: the navigation sounds), a message box button the select sound; the cues reach the sound log
//!   as `interface` starts (cshell+0x1af490 / cshell+0x1aeb90 -> cshell+0x1b6470 -> cshell+0x1b3960);
//! - the waiting room's map / mode / goal / players / weapon buttons and the creation box's mode button open a
//!   list 3 px under their control (cshell+0x164810, cshell+0x1391e0, cshell+0x33c350); a row is picked by a click,
//!   which closes the list and changes the setting (cshell+0x33cbb0, cshell+0x1614f0); the same button closes it;
//! - the result screen opens with the win / lose sound (cshell+0x1326b0).

use std::path::PathBuf;

use dc_botclient::lobby::{LobbyProfile, RoomMember, ServerEntry};
use dc_client::menu::frontfx::{self, FrontLits};
use dc_client::menu::link::{LinkOptions, RoomView};
use dc_client::menu::{create, dropdown, pages, room, Go, Menu, MenuInput, MenuOptions};
use dc_client::sound::{SoundCue, SoundDirector};
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;

fn rom() -> Option<PathBuf> {
    std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

fn menu(rom: Option<PathBuf>) -> Menu {
    Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "SoundUser".into(),
        link: LinkOptions::new("w2-uisound"),
        rom,
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    })
}

/// The waiting room of a host with one guest, all maps offering every mode.
fn host_room(m: &mut Menu) {
    let d = &mut m.data;
    d.profile = LobbyProfile {
        nick: "SoundUser".into(),
        my_id: 1,
        ..LobbyProfile::default()
    };
    d.masks = (0..12).map(|i| (i, 0xffff)).collect();
    d.room = RoomView {
        title: "w2".into(),
        host_id: 1,
        map: 0,
        mode: 0,
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
}

fn centre(r: [i32; 4]) -> [f32; 2] {
    [
        r[0] as f32 + r[2] as f32 / 2.0,
        r[1] as f32 + r[3] as f32 / 2.0,
    ]
}

fn press_at(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

#[track_caller]
fn click(m: &mut Menu, command: i32) {
    let b = m
        .sheet()
        .button(command)
        .unwrap_or_else(|| panic!("{} has no button {command}", m.screen_name()));
    let c = centre(b.rect());
    press_at(m, c);
}

/// The centre of shown row `k` of the open list (from the list's own geometry, the 2009 rule).
fn row_point(m: &Menu, k: usize) -> [f32; 2] {
    let d = m.dropdown().expect("a list is open");
    let w = dropdown::rule_row(d.list, d.anchor, k, d.font_size);
    [w.text_at[0], w.text_at[1] + d.font_size as f32 / 2.0]
}

#[test]
fn rom_interface_sound_keys_resolve_to_files() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let l = FrontLits::from_rom(&rom);
    assert!(l.rom);
    let found: Vec<&str> = frontfx::SOUND_KEYS
        .iter()
        .filter(|(id, _, _)| l.sounds.contains_key(id))
        .map(|(_, n, _)| *n)
        .collect();
    println!("interface keys with a file: {found:?}");
    for id in [
        frontfx::SOUND_SELECT,
        frontfx::SOUND_START_COUNT,
        frontfx::SOUND_MESSENGER_TWINKLE,
    ] {
        assert!(
            l.sounds.contains_key(&id),
            "{} has a file",
            frontfx::cue_name(id)
        );
    }
    assert!(
        l.drop_art.frame.is_some(),
        "the list frame texture is named"
    );
}

#[test]
fn rom_room_setting_lists_open_hover_pick_and_close() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = menu(Some(rom));
    host_room(&mut m);
    m.open(Go::Room);
    for (cmd, list) in [
        (room::CMD_MAP, "map"),
        (room::CMD_MODE, "mode"),
        (room::CMD_GOAL, "goal"),
        (room::CMD_PLAYERS, "players"),
        (room::CMD_ARMS, "arms"),
    ] {
        click(&mut m, cmd);
        let d = m
            .dropdown()
            .unwrap_or_else(|| panic!("{list}: no list after the click"))
            .clone();
        assert_eq!(d.list, list);
        assert!(d.items.len() >= 2, "{list}: rows {}", d.items.len());
        // 3 px under its control, as wide as it
        let id = room::LIST_ANCHORS
            .iter()
            .find(|(l, _)| *l == list)
            .unwrap()
            .1;
        let a = m
            .sheet()
            .anchor_rect(room::SETTINGS_WINDOW, id)
            .expect("anchor");
        let r = d.rect();
        assert_eq!([r[0], r[1], r[2]], [a[0], a[1] + a[3] + 3, a[2]], "{list}");
        // hover a row, then pick the last shown row whose value is not the room's
        let shown: Vec<usize> = d.shown().collect();
        let k = shown.len() - 1;
        m.input(MenuInput::Move(row_point(&m, k)));
        assert_eq!(m.dropdown().unwrap().hover, Some(shown[k]), "{list} hover");
        let before = m.link_sent();
        let p = row_point(&m, k);
        m.input(MenuInput::Press(p));
        m.input(MenuInput::Release(p));
        assert!(m.dropdown().is_none(), "{list}: a pick closes the list");
        assert_eq!(m.link_sent(), before + 1, "{list}: the pick sends 0x38c");
        // the button opens it again and closes it again
        click(&mut m, cmd);
        assert!(m.dropdown().is_some());
        click(&mut m, cmd);
        assert!(m.dropdown().is_none(), "{list}: the button closes its list");
    }
    // a guest's buttons open nothing
    m.data.room.host_id = 2;
    m.open(Go::Room);
    if m.sheet().button(room::CMD_GOAL).is_some() {
        click(&mut m, room::CMD_GOAL);
        assert!(m.dropdown().is_none(), "no list for a guest");
    }
}

#[test]
fn rom_map_list_scrolls_by_its_arrows() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = menu(Some(rom));
    host_room(&mut m);
    m.open(Go::Room);
    click(&mut m, room::CMD_MAP);
    let d = m.dropdown().unwrap().clone();
    if d.items.len() <= dropdown::MAX_ROWS {
        println!("skipped: {} maps", d.items.len());
        return;
    }
    assert_eq!(d.shown().len(), dropdown::MAX_ROWS);
    let r = d.rect();
    let down = [
        (r[0] + r[2] - dropdown::SCROLL_FROM_RIGHT + 2) as f32,
        (r[1] + r[3] - 2) as f32,
    ];
    m.input(MenuInput::Press(down));
    m.input(MenuInput::Release(down));
    let d = m.dropdown().expect("an arrow keeps the list open");
    assert_eq!(d.first, 1);
    // the first shown row is now the second map: picking it sends that map
    let p = row_point(&m, 0);
    let want = m.dropdown().unwrap().values[1];
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
    assert!(m.dropdown().is_none());
    assert_ne!(want, 0);
}

/// Review fix (w2-uisound): an open map list follows a room change that alters its rows (a 0x3035 mode change, the
/// 0x311 masks) instead of keeping the rows of the old mode until it is reopened.
#[test]
fn rom_an_open_map_list_follows_the_room_mode() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = menu(Some(rom));
    host_room(&mut m);
    // mode 0 on every map, mode 1 on the first three only
    m.data.masks = (0..12)
        .map(|i| (i, if i < 3 { 0b11 } else { 0b01 }))
        .collect();
    m.open(Go::Room);
    click(&mut m, room::CMD_MAP);
    assert_eq!(m.dropdown().expect("map list").values.len(), 12);
    let mut view = m.data.room.clone();
    view.mode = 1;
    m.feed_event(dc_client::menu::link::LinkEvent::RoomInfo(view));
    m.update();
    let d = m.dropdown().expect("the list stays open");
    assert_eq!(d.list, "map");
    assert_eq!(d.values, vec![0, 1, 2], "rows of the new mode");
    // the masks change too: map 5 now offers mode 1
    let masks: Vec<(u32, u32)> = (0..12)
        .map(|i| (i, if i < 3 || i == 5 { 0b11 } else { 0b01 }))
        .collect();
    m.feed_event(dc_client::menu::link::LinkEvent::RoomSettings(masks));
    m.update();
    assert_eq!(m.dropdown().unwrap().values, vec![0, 1, 2, 5]);
    // an unrelated event keeps the list as it is (scroll position included)
    m.feed_event(dc_client::menu::link::LinkEvent::RoomSettings(
        m.data.masks.clone(),
    ));
    m.update();
    assert_eq!(m.dropdown().unwrap().values, vec![0, 1, 2, 5]);
}

/// Review fix (w2-uisound): a press inside the open list's frame but between two rows is not the list's; the
/// window's hit test goes on (cshell+0x33cbb0 answers 0), so the list stays open and no row is picked.
#[test]
fn rom_a_press_between_list_rows_is_not_a_pick() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = menu(Some(rom));
    host_room(&mut m);
    m.open(Go::Room);
    click(&mut m, room::CMD_GOAL);
    let d = m.dropdown().expect("goal list").clone();
    let a = row_point(&m, 0);
    let b = row_point(&m, 1);
    // the spacing between rows 0 and 1, inside the frame
    let gap = (a[1] as i32..b[1] as i32)
        .map(|y| [a[0], y as f32])
        .find(|p| d.contains(*p) && !d.takes(*p))
        .expect("a gap between two rows");
    assert!(d.takes(a) && d.takes(b));
    let before = m.link_sent();
    m.input(MenuInput::Press(gap));
    m.input(MenuInput::Release(gap));
    assert_eq!(m.link_sent(), before, "no pick");
    assert!(m.dropdown().is_some(), "the list stays open");
    // a gap or frame border over an enabled screen button: the press reaches that button (before the fix the list
    // swallowed it)
    let r = d.rect();
    let over_button = (r[1]..r[1] + r[3])
        .flat_map(|y| (r[0]..r[0] + r[2]).map(move |x| [x as f32, y as f32]))
        .find_map(|p| {
            let i = m.sheet().hit(p)?;
            let q = &m.sheet().parts[i];
            (!d.takes(p) && q.enabled && q.kind == WidgetKind::Button && m.sheet().visible(i))
                .then_some((p, i))
        });
    match over_button {
        Some((p, i)) => {
            println!("gap over control {i} at {p:?}");
            m.input(MenuInput::Move(p));
            let n = m.pending_sounds().len();
            m.input(MenuInput::Press(p));
            m.input(MenuInput::Release(p));
            assert!(
                m.pending_sounds()[n..].contains(&frontfx::SOUND_SELECT),
                "the button under the gap clicked: {:?}",
                m.pending_sounds()
            );
        }
        None => println!("no enabled button under the open goal list's gaps"),
    }
}

#[test]
fn rom_create_box_mode_list_picks_a_mode() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = menu(Some(rom));
    host_room(&mut m);
    m.open(Go::Create);
    click(&mut m, create::CMD_MODE_LIST);
    let d = m.dropdown().expect("the mode list opens").clone();
    assert_eq!(d.list, "create_mode");
    assert_eq!(d.font_size, dropdown::CREATE_ROW_SIZE);
    let a = m
        .sheet()
        .anchor_rect(create::WINDOW, create::MODE_LIST_ANCHOR)
        .unwrap();
    assert_eq!(d.rect()[1], a[1] + a[3] + 3);
    let k = d.shown().len() - 1;
    let want = d.items[d.shown().nth(k).unwrap()].clone();
    let p = row_point(&m, k);
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
    assert!(m.dropdown().is_none());
    let label = m
        .sheet()
        .part(create::WINDOW, create::MODE_LABEL_TEXT, WidgetKind::Text)
        .unwrap()
        .text
        .clone();
    assert_eq!(label, want, "the picked mode is the box's mode");
    // the button toggles
    click(&mut m, create::CMD_MODE_LIST);
    click(&mut m, create::CMD_MODE_LIST);
    assert!(m.dropdown().is_none());
}

/// The cues of `m` played through a sound director (the window's per-frame hook) logging to `log`; returns the log's interface lines' names.
fn played(m: &mut Menu, rom: &std::path::Path, log: &std::path::Path) -> Vec<String> {
    let mut sd = SoundDirector::open(Some(rom));
    sd.log_to(log).unwrap();
    let n = m.pending_sounds().len();
    // the window's per-frame hand-over (menu::window::menu_music)
    m.sound_frame(Some(&mut sd));
    assert!(m.pending_sounds().is_empty());
    let cues = sd.starts_of("interface");
    assert_eq!(cues.len(), n, "every cue started");
    assert!(cues
        .iter()
        .all(|e| e.file.as_deref().is_some_and(|f| !f.is_empty())));
    drop(sd);
    let text = std::fs::read_to_string(log).unwrap();
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["trigger"] == "interface")
        .map(|v| {
            assert!(v["file_hash"].is_string(), "a played file: {v}");
            assert_eq!(v["loop"], false);
            v["detail"].as_str().unwrap_or_default().to_string()
        })
        .collect()
}

#[test]
fn rom_clicks_and_hovers_reach_the_sound_log() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let dir = std::env::temp_dir().join(format!("dc_w2_uisound_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut m = menu(Some(rom.clone()));
    host_room(&mut m);
    m.open(Go::Room);
    m.take_sound_cues();
    // pointer onto the goal button: one select-change; moving on it again: nothing; click: select
    let b = centre(m.sheet().button(room::CMD_GOAL).unwrap().rect());
    m.input(MenuInput::Move(b));
    m.input(MenuInput::Move([b[0] + 1.0, b[1]]));
    assert_eq!(m.pending_sounds(), &[frontfx::SOUND_SELECT_CHANGE]);
    m.input(MenuInput::Press(b));
    m.input(MenuInput::Release(b));
    assert_eq!(
        m.pending_sounds(),
        &[frontfx::SOUND_SELECT_CHANGE, frontfx::SOUND_SELECT]
    );
    // a pick in the open list plays the select click (CLI-930; cshell+0x33cbb0 asks for none itself)
    let p = row_point(&m, 0);
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
    assert_eq!(m.pending_sounds().len(), 3, "{:?}", m.pending_sounds());
    // the check buttons (room options) click with the select sound too
    let check = m
        .sheet()
        .parts
        .iter()
        .find(|q| q.kind == WidgetKind::CheckButton && q.layer == room::SETTINGS_WINDOW)
        .map(|q| centre(q.rect()));
    if let Some(c) = check {
        press_at(&mut m, c);
        assert_eq!(m.pending_sounds().last(), Some(&frontfx::SOUND_SELECT));
    }
    let lines = played(&mut m, &rom, &dir.join("room.sound.jsonl"));
    println!("room: {lines:?}");
    assert!(lines.iter().any(|l| l.contains("\"select_change\"")));
    assert!(lines.iter().filter(|l| l.contains("\"select\"")).count() >= 1);

    // the server screen: its common window plays the navigation sounds, the rest the select sounds
    m.data.servers = vec![ServerEntry {
        id: 1,
        name: "DeCombat".into(),
        ip: "127.0.0.1".into(),
        port: 1,
        open: 1,
        capacity: 100,
        current: 10,
        clan_only: 0,
        op_capacity: 0,
        op_current: 0,
    }];
    m.open(Go::Servers);
    assert!(m.popup().is_none());
    m.take_sound_cues();
    let nav = m
        .sheet()
        .parts
        .iter()
        .enumerate()
        .find(|(i, q)| {
            q.kind == WidgetKind::Button
                && q.enabled
                && q.page == pages::COMMON
                && q.layer == 0
                && m.sheet().visible(*i)
        })
        .map(|(_, q)| centre(q.rect()));
    match nav {
        Some(c) => {
            m.input(MenuInput::Move(c));
            m.input(MenuInput::Press(c));
            assert_eq!(m.pending_sounds()[0], frontfx::SOUND_NAV_CHANGE);
            m.input(MenuInput::Release(c));
            assert!(m.pending_sounds().contains(&frontfx::SOUND_NAV_SELECT));
        }
        None => println!("no common window button on the server screen"),
    }
}

#[test]
fn a_message_box_button_and_the_result_screen_play_their_sounds() {
    // without a ROM: the cues stay (no file to look up)
    let mut m = menu(None);
    m.open(Go::Lobby);
    m.take_sound_cues();
    let b = dc_client::menu::popup::Popup::ok("x".into(), dc_client::menu::popup::BoxTag::None);
    // the box's OK button: press and release on it
    let ok = centre(b.buttons()[0]);
    m.show_popup(b);
    m.input(MenuInput::Press(ok));
    m.input(MenuInput::Release(ok));
    assert!(m.popup().is_none(), "the box closed");
    assert_eq!(m.pending_sounds(), &[frontfx::SOUND_SELECT]);
    m.take_sound_cues();
    // the result screen of a lost match: the lose sound
    let mut r = dc_client::matchexit::MatchResult::sample();
    r.own_team = Some(if r.winner == 1 { 2 } else { 1 });
    m.data.result = Some(r);
    m.open(Go::Result);
    assert_eq!(m.pending_sounds(), &[frontfx::SOUND_RESULT_LOSE]);
    let cues = m.take_sound_cues();
    assert!(matches!(
        cues.as_slice(),
        [SoundCue::Interface {
            what: "match_lost",
            ..
        }]
    ));
}

#[test]
fn rom_the_countdown_line_is_drawn_red_in_the_room_chat() {
    // EVIDENCE: cshell+0x15f2b0 (the countdown line goes to the chat with 0xffff0000)
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = menu(Some(rom));
    host_room(&mut m);
    let text = m.cat.text(frontfx::COUNTDOWN_TEXT);
    let line = FrontLits::from_rom(&rom_path())
        .countdown_line(&text, 2)
        .expect("a countdown line");
    m.data.say("P2 : hi".into());
    m.data.say_in(line.clone(), frontfx::COUNTDOWN_COLOUR);
    m.open(Go::Room);
    let red = dc_client::menu::catalog::argb(frontfx::COUNTDOWN_COLOUR);
    let items = m.items();
    let colour_of = |t: &str| {
        items.iter().find_map(|i| match i {
            dc_client::hud::items::DrawItem::Label { text, style, .. } if text == t => {
                Some(style.colour)
            }
            _ => None,
        })
    };
    assert_eq!(colour_of(&line), Some(red), "the countdown line is red");
    assert_ne!(
        colour_of("P2 : hi"),
        Some(red),
        "a chat line keeps the chat colour"
    );
}

fn rom_path() -> PathBuf {
    rom().expect("CA_ROM")
}

#[test]
fn rom_a_picked_list_row_plays_the_select_click() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = menu(Some(rom));
    host_room(&mut m);
    m.open(Go::Room);
    click(&mut m, room::CMD_MODE);
    assert!(m.dropdown().is_some());
    let _ = m.take_sound_cues();
    let k = m.dropdown().unwrap().shown().count() - 1;
    let p = row_point(&m, k);
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
    assert!(m.dropdown().is_none());
    assert!(
        m.pending_sounds().contains(&frontfx::SOUND_SELECT),
        "pick sounds: {:?}",
        m.pending_sounds()
    );
}
