//! Waiting room option pairs (mid-match join, friendly fire, kill cam), live incident 2026-11-06: each pair shows the
//! room's value on the right check, sends the clicked value (ON = 1, OFF = 0), and a mode change sends the mode only.
//! ROM-gated (skipped without `CA_ROM`).
// EVIDENCE: cshell+0x1677f0 (each option's four controls: check buttons (6, 31) / (6, 32) and texts (6, 23) / (6, 24)
// carry the commands 234 / 235). Read from the ROM's window records: text 23 "ON" and check 31 carry 234, text 24
// "OFF" and check 32 carry 235; the friendly fire pair is 236 / 237 (texts 26 / 27, checks 33 / 34), the kill cam
// pair 601 / 602 (texts 29 / 30, checks 35 / 36)
// DISASM: cshell+0x168680 (0x3035 types 9 / 0xa / 0xd update the option's checks)

use std::path::PathBuf;

use dc_botclient::lobby::{LobbyProfile, RoomMember, ServerEntry};
use dc_client::menu::link::{LinkEvent, LinkOptions, RoomView};
use dc_client::menu::sheet::Part;
use dc_client::menu::{room, Go, Menu, MenuInput, MenuOptions};
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;

fn rom() -> Option<PathBuf> {
    std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

fn host_room(rom: PathBuf, options: [i32; 3]) -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "Host".into(),
        link: LinkOptions::new("room-options"),
        rom: Some(rom),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    let d = &mut m.data;
    d.server = Some(ServerEntry {
        id: 2,
        name: "plain".into(),
        ip: "127.0.0.1".into(),
        port: 1,
        open: 1,
        capacity: 100,
        current: 1,
        clan_only: 0,
        op_capacity: 0,
        op_current: 0,
    });
    d.profile = LobbyProfile {
        nick: "Host".into(),
        my_id: 1,
        ..LobbyProfile::default()
    };
    d.masks = (0..24).map(|i| (i, 0xffff)).collect();
    d.room = RoomView {
        title: "opts".into(),
        host_id: 1,
        map: 2,
        mode: 0,
        players: 2,
        max_players: 16,
        goal: 100,
        options,
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
    m.update();
    m
}

fn part(m: &Menu, id: i32, kind: WidgetKind) -> &Part {
    m.sheet()
        .part(room::SETTINGS_WINDOW, id, kind)
        .unwrap_or_else(|| panic!("(6, {id}) {kind:?}"))
}

fn checked(m: &Menu, id: i32) -> bool {
    part(m, id, WidgetKind::CheckButton).checked
}

fn click(m: &mut Menu, r: [i32; 4]) {
    let c = [
        r[0] as f32 + r[2] as f32 / 2.0,
        r[1] as f32 + r[3] as f32 / 2.0,
    ];
    m.input(MenuInput::Move(c));
    m.input(MenuInput::Press(c));
    m.input(MenuInput::Release(c));
}

fn set(m: &mut Menu, kind: u32, value: u32) {
    m.feed_event(LinkEvent::RoomSetting {
        kind,
        value,
        text: String::new(),
    });
    m.update();
}

/// One option pair of the settings window.
struct Pair {
    c_on: i32,
    c_off: i32,
    t_on: i32,
    t_off: i32,
    kind: u32,
    cmd_on: i32,
    cmd_off: i32,
}

fn pairs() -> Vec<Pair> {
    (0..3)
        .map(|k| Pair {
            c_on: room::OPTION_CHECKS[k].0,
            c_off: room::OPTION_CHECKS[k].1,
            t_on: room::OPTION_TEXTS[k].0,
            t_off: room::OPTION_TEXTS[k].1,
            kind: room::OPTION_COMMANDS[k].2,
            cmd_on: room::OPTION_COMMANDS[k].0,
            cmd_off: room::OPTION_COMMANDS[k].1,
        })
        .collect()
}

fn shows_on(m: &Menu, p: &Pair) -> bool {
    let (on, off) = (checked(m, p.c_on), checked(m, p.c_off));
    assert!(on != off, "exactly one of the pair is checked: {on} {off}");
    on
}

#[test]
fn rom_option_records_carry_on_then_off() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let m = host_room(rom, [1, 0, 0]);
    for p in pairs() {
        // the ROM's own labels: the text carrying the "on" command reads ON, the "off" one OFF
        assert_eq!(part(&m, p.t_on, WidgetKind::Text).command, p.cmd_on);
        assert_eq!(part(&m, p.t_off, WidgetKind::Text).command, p.cmd_off);
        assert_eq!(part(&m, p.t_on, WidgetKind::Text).text, "ON");
        assert_eq!(part(&m, p.t_off, WidgetKind::Text).text, "OFF");
        assert_eq!(part(&m, p.c_on, WidgetKind::CheckButton).command, p.cmd_on);
        assert_eq!(part(&m, p.c_off, WidgetKind::CheckButton).command, p.cmd_off);
        // the ON check sits left of its ON text, the OFF check left of the OFF text, on the same row
        for (c, t) in [(p.c_on, p.t_on), (p.c_off, p.t_off)] {
            let (a, b) = (
                part(&m, c, WidgetKind::CheckButton),
                part(&m, t, WidgetKind::Text),
            );
            assert!(a.pos[0] < b.pos[0] && (a.pos[1] - b.pos[1]).abs() < 12);
        }
    }
    // the mid-match join pair is commands 234 / 235, field 9
    assert_eq!(room::OPTION_COMMANDS[0], (234, 235, 9));
}

#[test]
fn rom_option_checks_show_the_rooms_value_and_follow_pushes() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = host_room(rom, [1, 1, 1]);
    for p in pairs() {
        assert!(shows_on(&m, &p), "room value 1 = ON");
    }
    // 0x3035 type 9 value 0: mid-match join shows OFF, the others stay ON
    set(&mut m, 9, 0);
    assert_eq!(m.data.room.options, [0, 1, 1]);
    let all = pairs();
    assert!(!shows_on(&m, &all[0]), "type 9 value 0 = OFF");
    assert!(shows_on(&m, &all[1]) && shows_on(&m, &all[2]));
    // and back on
    set(&mut m, 9, 1);
    assert!(shows_on(&m, &all[0]));
    // friendly fire (type 0xa) and kill cam (type 0xd) off
    set(&mut m, 0xa, 0);
    set(&mut m, 0xd, 0);
    assert!(shows_on(&m, &all[0]));
    assert!(!shows_on(&m, &all[1]) && !shows_on(&m, &all[2]));
}

#[test]
fn rom_option_clicks_send_on_as_one_and_off_as_zero() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = host_room(rom, [0, 0, 0]);
    let _ = m.take_link_sent();
    for p in pairs() {
        let r = part(&m, p.c_on, WidgetKind::CheckButton).rect();
        click(&mut m, r);
        let sent = m.take_link_sent();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(
            sent[0].contains(&format!("Setting({}, 1)", p.kind)),
            "ON click: {sent:?}"
        );
        // the room still holds 0 until the lobby confirms: the display goes back to OFF
        assert!(!shows_on(&m, &p));
        let r = part(&m, p.c_off, WidgetKind::CheckButton).rect();
        click(&mut m, r);
        let sent = m.take_link_sent();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(
            sent[0].contains(&format!("Setting({}, 0)", p.kind)),
            "OFF click: {sent:?}"
        );
    }
    // with the room ON, an OFF click sends 0 and the display stays ON until the lobby echoes
    set(&mut m, 9, 1);
    let all = pairs();
    let r = part(&m, all[0].c_off, WidgetKind::CheckButton).rect();
    click(&mut m, r);
    let sent = m.take_link_sent();
    assert!(
        sent.len() == 1 && sent[0].contains("Setting(9, 0)"),
        "{sent:?}"
    );
    assert!(shows_on(&m, &all[0]));
}

#[test]
fn rom_mode_changes_send_the_mode_only() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = host_room(rom, [1, 0, 1]);
    let _ = m.take_link_sent();
    let mut picked = 0;
    for row in [3usize, 2, 1, 2, 0, 1] {
        let b = m.sheet().button(room::CMD_MODE).unwrap().rect();
        click(&mut m, b);
        let Some(d) = m.dropdown() else { continue };
        assert_eq!(d.list, "mode");
        let (Some(p), Some(&v)) = (d.row_point(row), d.values.get(row)) else {
            continue;
        };
        let was = m.data.room.mode;
        m.input(MenuInput::Move(p));
        m.input(MenuInput::Press(p));
        m.input(MenuInput::Release(p));
        let sent = m.take_link_sent();
        if v == was {
            assert!(sent.is_empty(), "{sent:?}");
            continue;
        }
        picked += 1;
        assert_eq!(sent.len(), 1, "one request per mode pick: {sent:?}");
        assert!(sent[0].contains(&format!("Setting(4, {v})")), "{sent:?}");
        // the lobby answers with the new mode; the options and the display stay and nothing is sent
        set(&mut m, 4, v as u32);
        assert_eq!(m.data.room.options, [1, 0, 1]);
        assert!(m.take_link_sent().is_empty());
    }
    assert!(picked >= 3, "picked {picked}");
    let all = pairs();
    assert!(shows_on(&m, &all[0]) && !shows_on(&m, &all[1]) && shows_on(&m, &all[2]));
}
