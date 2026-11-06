//! CLI-1897: on a clan-only server entry the front end reads the ROM clan mode rows, and the waiting room greys the
//! room options a mode row locks. Every 2009 clan row locks friendly fire and mid-match join (their check buttons and
//! ON / OFF texts are disabled, a click sends nothing); kill cam and the weapon limit stay the host's; the goal list
//! holds the clan row's single goal and the room size starts at the clan row's minimum, in the waiting room and in
//! the room creation box. The same rule greys a plain row's locked option (One Man Army's friendly fire), and every
//! option for a guest.
//!
//! ROM-gated (skipped without `CA_ROM`).
// EVIDENCE: cshell+0x1677f0 (option controls enabled = mode row flag and host), cshell+0x264d90 / cshell+0x264c30
// (the clan list by the session's clan flag), cshell+0x163550 / cshell+0x1637a0 (goal / player lists of the row),
// cshell+0x1391e0 (room creation: the player step and limits of the clan row)

use std::path::PathBuf;

use dc_botclient::lobby::{LobbyProfile, RoomMember, ServerEntry};
use dc_client::menu::link::{LinkOptions, RoomView};
use dc_client::menu::{create, room, Go, Menu, MenuInput, MenuOptions};
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;

fn rom() -> Option<PathBuf> {
    std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

fn menu(rom: PathBuf, clan: bool, host: bool, mode: i32) -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "ClanUser".into(),
        link: LinkOptions::new("clan-room"),
        rom: Some(rom),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    let d = &mut m.data;
    d.server = Some(ServerEntry {
        id: 2,
        name: "clan".into(),
        ip: "127.0.0.1".into(),
        port: 1,
        open: 1,
        capacity: 100,
        current: 1,
        clan_only: clan as u32,
        op_capacity: 0,
        op_current: 0,
    });
    d.profile = LobbyProfile {
        nick: "ClanUser".into(),
        my_id: 1,
        ..LobbyProfile::default()
    };
    d.masks = (0..12).map(|i| (i, 0xffff)).collect();
    d.room = RoomView {
        title: "cw".into(),
        host_id: if host { 1 } else { 2 },
        map: 2,
        mode,
        players: 2,
        max_players: 16,
        goal: 100,
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
    m
}

fn part_enabled(m: &Menu, id: i32, kind: WidgetKind) -> bool {
    m.sheet()
        .part(room::SETTINGS_WINDOW, id, kind)
        .unwrap_or_else(|| panic!("(6, {id}) {kind:?}"))
        .enabled
}

/// The check buttons and ON / OFF texts of option `k`: all enabled or all disabled.
fn option_enabled(m: &Menu, k: usize) -> bool {
    let (on, off) = room::OPTION_CHECKS[k];
    let (ton, toff) = room::OPTION_TEXTS[k];
    let all = [
        part_enabled(m, on, WidgetKind::CheckButton),
        part_enabled(m, off, WidgetKind::CheckButton),
        part_enabled(m, ton, WidgetKind::Text),
        part_enabled(m, toff, WidgetKind::Text),
    ];
    assert!(all.iter().all(|e| *e == all[0]), "option {k}: {all:?}");
    all[0]
}

fn arms_enabled(m: &Menu) -> bool {
    part_enabled(m, room::ARMS_BUTTON, WidgetKind::Button)
}

fn press(m: &mut Menu, r: [i32; 4]) {
    let c = [
        r[0] as f32 + r[2] as f32 / 2.0,
        r[1] as f32 + r[3] as f32 / 2.0,
    ];
    m.input(MenuInput::Move(c));
    m.input(MenuInput::Press(c));
    m.input(MenuInput::Release(c));
}

fn check_rect(m: &Menu, id: i32) -> [i32; 4] {
    m.sheet()
        .part(room::SETTINGS_WINDOW, id, WidgetKind::CheckButton)
        .unwrap()
        .rect()
}

fn checked(m: &Menu, id: i32) -> bool {
    m.sheet()
        .part(room::SETTINGS_WINDOW, id, WidgetKind::CheckButton)
        .unwrap()
        .checked
}

fn row_int(m: &Menu, mode: i32, key: &str) -> i32 {
    m.cat
        .mode_row(mode)
        .and_then(|r| r.get(key))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("mode row {mode} {key}"))
}

const MID_JOIN: usize = 0;
const FRIENDLY_FIRE: usize = 1;
const KILL_CAM: usize = 2;

#[test]
fn rom_clan_waiting_room_greys_friendly_fire_and_mid_join() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    // the plain lobby's Elimination row: every option the host's
    let mut m = menu(rom.clone(), false, true, 0);
    m.open(Go::Room);
    assert!(!m.cat.clan_table());
    for k in [MID_JOIN, FRIENDLY_FIRE, KILL_CAM] {
        assert!(option_enabled(&m, k), "plain option {k}");
    }
    assert!(arms_enabled(&m));
    // the clan lobby's: friendly fire and mid-match join locked, kill cam and the weapon limit open
    let mut m = menu(rom, true, true, 0);
    m.open(Go::Room);
    assert!(m.cat.clan_table());
    assert!(!option_enabled(&m, MID_JOIN), "mid-match join locked");
    assert!(!option_enabled(&m, FRIENDLY_FIRE), "friendly fire locked");
    assert!(option_enabled(&m, KILL_CAM), "kill cam open");
    assert!(arms_enabled(&m), "weapon limit open");
    // a click on a locked check sends nothing and the room's value (off) stays shown
    let before = m.link_sent();
    let (ff_on, ff_off) = room::OPTION_CHECKS[FRIENDLY_FIRE];
    let r = check_rect(&m, ff_on);
    press(&mut m, r);
    assert_eq!(m.link_sent(), before, "no 0x38c for a locked option");
    assert!(
        !checked(&m, ff_on) && checked(&m, ff_off),
        "friendly fire shows off"
    );
    // an open option still sends
    let (kc_on, _) = room::OPTION_CHECKS[KILL_CAM];
    let r = check_rect(&m, kc_on);
    press(&mut m, r);
    assert_eq!(m.link_sent(), before + 1, "kill cam 0x38c");
    // the goal list holds the clan row's one goal, the player list runs over the clan row's sizes
    let (gmin, gmax) = (row_int(&m, 0, "GoalMin"), row_int(&m, 0, "GoalMax"));
    assert_eq!(gmin, gmax, "the clan row's goal is fixed");
    let (pmin, pmax, step) = m.cat.mode_players(0).unwrap();
    assert!(
        pmin > 2,
        "clan room size starts above the plain row's 2: {pmin}"
    );
    let b = m.sheet().button(room::CMD_GOAL).unwrap().rect();
    press(&mut m, b);
    assert_eq!(m.dropdown().expect("goal list").items.len(), 1);
    press(&mut m, b);
    assert!(m.dropdown().is_none());
    let b = m.sheet().button(room::CMD_PLAYERS).unwrap().rect();
    press(&mut m, b);
    let n = m.dropdown().expect("player list").items.len() as i32;
    assert_eq!(n, (pmax - pmin) / step.max(1) + 1);
}

#[test]
fn rom_locked_options_follow_the_row_and_the_host() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    // a guest changes nothing: every option and the weapon button disabled
    let mut m = menu(rom.clone(), false, false, 0);
    m.open(Go::Room);
    for k in [MID_JOIN, FRIENDLY_FIRE, KILL_CAM] {
        assert!(!option_enabled(&m, k), "guest option {k}");
    }
    assert!(!arms_enabled(&m));
    // the plain One Man Army row locks friendly fire, nothing else
    let ffa = m
        .cat
        .mode_rows()
        .iter()
        .position(|r| r.get("Key").is_some_and(|k| k.trim() == "FFA"))
        .expect("FFA row") as i32;
    let mut m = menu(rom, false, true, ffa);
    m.open(Go::Room);
    assert!(!option_enabled(&m, FRIENDLY_FIRE));
    assert!(option_enabled(&m, MID_JOIN) && option_enabled(&m, KILL_CAM));
}

#[test]
fn rom_clan_room_creation_box_uses_the_clan_row_sizes() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let fewest = |clan: bool| {
        let mut m = menu(rom.clone(), clan, true, 0);
        m.data.masks = vec![(2, 1)];
        m.open(Go::Create);
        for _ in 0..20 {
            m.input(MenuInput::Command(create::CMD_FEWER_PLAYERS));
        }
        let min = m.cat.mode_players(0).unwrap().0;
        let text = m
            .sheet()
            .part(create::WINDOW, create::PLAYER_COUNT_TEXT, WidgetKind::Text)
            .unwrap()
            .text
            .clone();
        (min, text)
    };
    let (plain_min, plain) = fewest(false);
    let (clan_min, clan) = fewest(true);
    assert!(clan_min > plain_min, "{clan_min} vs {plain_min}");
    assert_eq!(plain, format!("{0} vs {0}", plain_min / 2));
    assert_eq!(clan, format!("{0} vs {0}", clan_min / 2));
}
