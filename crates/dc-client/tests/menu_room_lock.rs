//! countdown-lock (CLI-1310 / CLI-1311 / CLI-1312): the waiting room during the start countdown. A 0x3044 tick locks
//! a player whose own state is 2 (the host, the players ready at the start, cshell+0x15f2b0): no exit (button,
//! Escape), ready, team, setting list or bar command and no pointer input (cshell+0x15e970 / 0x15ec10 / 0x160b00);
//! the chat goes on (cshell+0x160c90). The room in game (0x3035 type 7 value 2) unlocks (cshell+0x168680). A player
//! who was not ready stays free. Runs without the ROM (the commands); the pointer part needs `CA_ROM`.

use std::path::PathBuf;

use dc_botclient::lobby::{LobbyProfile, RoomMember};
use dc_client::menu::link::{LinkEvent, LinkOptions, RoomView};
use dc_client::menu::room::{CMD_EXIT, CMD_READY, CMD_TEAM, HOST_CHANGED_WHILE_STARTING};
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};
use dc_client::testkit::synth_font;

/// The menu over a detached link: the test has no lobby, and a live link thread answers every request (the
/// watchdog's 0x394, the exit) with an asynchronous "no lobby connection" close, whose "connection ended" box
/// raced the asserts on the room's own boxes.
fn menu(rom: Option<PathBuf>) -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "LockUser".into(),
        link: LinkOptions::new("countdown-lock"),
        rom,
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m
}

/// A room of two: member 1 (the own player) and member 2; `host` = the own player hosts.
fn room(m: &mut Menu, host: bool) {
    let d = &mut m.data;
    d.profile = LobbyProfile {
        nick: "LockUser".into(),
        my_id: 1,
        ..LobbyProfile::default()
    };
    d.room = RoomView {
        title: "lock".into(),
        host_id: if host { 1 } else { 2 },
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
    m.open(Go::Room);
}

fn feed(m: &mut Menu, evs: Vec<LinkEvent>) {
    for e in evs {
        m.feed_event(e);
    }
    m.update();
}

fn state(id: u32, state: i32) -> LinkEvent {
    LinkEvent::MemberState { id, state, sub: 0 }
}

fn room_push(v: u32) -> LinkEvent {
    LinkEvent::RoomSetting {
        kind: 7,
        value: v,
        text: String::new(),
    }
}

/// The start as the lobby sends it: room starting, own state 2 (when `own_starting`), the first tick.
fn start(m: &mut Menu, own_starting: bool) {
    let mut evs = vec![room_push(1), state(2, 2)];
    if own_starting {
        evs.push(state(1, 2));
    }
    evs.push(LinkEvent::Countdown(0));
    feed(m, evs);
}

/// Nothing goes to the lobby for these inputs.
#[track_caller]
fn sends_nothing(m: &mut Menu, inputs: &[MenuInput]) {
    for i in inputs {
        let before = m.link_sent();
        m.input(i.clone());
        assert_eq!(
            m.link_sent(),
            before,
            "{i:?} sent {}",
            m.link_latest_request()
        );
    }
}

fn exits() -> Vec<MenuInput> {
    vec![
        MenuInput::Key(MenuKey::Escape),
        MenuInput::Command(CMD_EXIT),
        MenuInput::Command(CMD_READY),
        MenuInput::Command(CMD_TEAM),
        // the bar's shop
        MenuInput::Command(0x3d),
    ]
}

#[test]
fn the_host_is_locked_from_the_first_tick_until_the_room_plays() {
    let mut m = menu(None);
    room(&mut m, true);
    start(&mut m, true);
    assert_eq!(m.screen_name(), "room");
    sends_nothing(&mut m, &exits());
    assert_eq!(
        m.screen_name(),
        "room",
        "no bar command opened another screen"
    );
    // the rest of the countdown keeps the lock
    feed(&mut m, (1..5).map(LinkEvent::Countdown).collect());
    sends_nothing(&mut m, &exits());
    // the room in game ends it
    feed(&mut m, vec![room_push(2), state(1, 3)]);
    let before = m.link_sent();
    m.input(MenuInput::Command(CMD_EXIT));
    assert_eq!(m.link_sent(), before + 1);
    assert_eq!(m.link_latest_request(), "QuitRoom");
}

#[test]
fn a_ready_guest_is_locked_and_a_host_change_says_so_in_red() {
    let mut m = menu(None);
    room(&mut m, false);
    feed(&mut m, vec![state(1, 1)]);
    start(&mut m, true);
    sends_nothing(&mut m, &exits());
    let line = m.cat.text(HOST_CHANGED_WHILE_STARTING);
    let lines = m.data.chat.len();
    feed(
        &mut m,
        vec![LinkEvent::RoomSetting {
            kind: 2,
            value: 1,
            text: String::new(),
        }],
    );
    assert_eq!(m.data.chat.len(), lines + 1);
    assert_eq!(m.data.chat.last(), Some(&line));
    // still locked after the host change
    sends_nothing(&mut m, &exits());
    // a room back to waiting unlocks too (CLI-1311)
    feed(&mut m, vec![room_push(0), state(1, 0)]);
    let before = m.link_sent();
    m.input(MenuInput::Key(MenuKey::Escape));
    assert_eq!(m.link_sent(), before + 1);
}

#[test]
fn a_guest_who_was_not_ready_stays_free() {
    let mut m = menu(None);
    room(&mut m, false);
    start(&mut m, false);
    let before = m.link_sent();
    m.input(MenuInput::Key(MenuKey::Escape));
    assert_eq!(m.link_sent(), before + 1);
    assert_eq!(m.link_latest_request(), "QuitRoom");
}

fn rom() -> Option<PathBuf> {
    std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

#[test]
fn rom_a_locked_room_takes_no_click() {
    let Some(rom) = rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut m = menu(Some(rom));
    room(&mut m, true);
    start(&mut m, true);
    let b = m.sheet().button(CMD_EXIT).expect("exit button");
    let r = b.rect();
    let p = [
        r[0] as f32 + r[2] as f32 / 2.0,
        r[1] as f32 + r[3] as f32 / 2.0,
    ];
    sends_nothing(
        &mut m,
        &[
            MenuInput::Move(p),
            MenuInput::Press(p),
            MenuInput::Release(p),
        ],
    );
    assert_eq!(m.screen_name(), "room");
    // the chat still works (the room's chat box has the keyboard)
    for c in "hi".chars() {
        m.input(MenuInput::Char(c));
    }
    let before = m.link_sent();
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(m.link_sent() > before, "the chat line went out");
    feed(&mut m, vec![room_push(2)]);
    let before = m.link_sent();
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
    assert_eq!(m.link_sent(), before + 1, "unlocked: the exit click leaves");
}

/// CLI-1314: the 2009 start watchdog (cshell+0x1608b0): 15 s after the last tick without a match address the room
/// shows box 0x473, sends 0x394 {1} and is free again.
#[test]
fn the_start_watchdog_frees_a_room_whose_match_never_came() {
    use std::time::Duration;
    let mut m = menu(None);
    room(&mut m, true);
    start(&mut m, true);
    sends_nothing(&mut m, &exits());
    // a tick restarts the clock: 14 s after it nothing happens
    m.age_start_watch(Duration::from_secs(14));
    m.update();
    assert_eq!(m.popup_count(), 0);
    feed(&mut m, vec![LinkEvent::Countdown(1)]);
    m.age_start_watch(Duration::from_secs(14));
    m.update();
    assert_eq!(m.popup_count(), 0, "the new tick restarted the clock");
    m.age_start_watch(Duration::from_secs(2));
    m.update();
    assert_eq!(m.popup_count(), 1, "box 0x473");
    assert_eq!(m.link_latest_request(), "StartFailed");
    // the box closes and the room works again
    m.input(MenuInput::Key(MenuKey::Enter));
    m.update();
    assert_eq!(m.popup_count(), 0);
    m.input(MenuInput::Command(CMD_EXIT));
    assert_eq!(m.link_latest_request(), "QuitRoom");
}

#[test]
fn the_watchdog_outlives_the_in_game_push_and_stops_when_the_lobby_takes_the_start_back() {
    use std::time::Duration;
    // room state 2 (in game) unlocks, but the wait for the match address still ends in the watchdog
    let mut m = menu(None);
    room(&mut m, true);
    start(&mut m, true);
    feed(&mut m, vec![room_push(2)]);
    m.age_start_watch(Duration::from_secs(16));
    m.update();
    assert_eq!(m.popup_count(), 1);
    assert_eq!(m.link_latest_request(), "StartFailed");
    // the lobby taking the start back (room state 0) disarms it
    let mut m = menu(None);
    room(&mut m, true);
    start(&mut m, true);
    feed(&mut m, vec![room_push(0)]);
    m.age_start_watch(Duration::from_secs(60));
    m.update();
    assert_eq!(m.popup_count(), 0);
}
