//! clan-functions: the clan screen's functions against a real `dc-lobby-server` (clan pages on the auth port + 8080,
//! CLI-790), driven through the menu's inputs.
//!
//! Item 1: Create Clan opens the web window with the client's own create page (name field, emblem / background
//! grids, create button, CLI-791); a taken name shows the server's refusal on the page; the create writes the clan
//! (leader, mark) and the screen becomes the master's (name, leader, member row, the info values), with the
//! server's line in a box. On the old code the window stays empty and nothing is sent.
//!
//! ROM-gated (skipped without `CA_ROM`). Every process is stopped by its own PID.
// SPEC: PROTOCOL_2009.md §5.1, §5.2

mod support;

use dc_client::hud::items::DrawItem;
use dc_client::menu::clan::{
    CMD_CREATE, CMD_FIND, CMD_LEAVE, CMD_MANAGE, INFO_WINDOW, LEADER_TEXT, MEMBERS_TEXT, NAME_TEXT,
    TALK_FIELD, TALK_WINDOW,
};
use dc_client::menu::clanpage::{
    CELL, CELL_STEP, CREATE_BUTTON, GRID_AT, GRID_DY, LEAVE_BUTTON, NAME_FIELD, PAGE, ROW_BUTTON,
    ROW_STEP, ROW_Y, T_JOIN, T_LEAVE_QUESTION, T_REMOVE,
};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::steps::{parse, Here, StepRunner};
use dc_ui_data::WidgetKind;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use support::*;

fn rom() -> Option<PathBuf> {
    std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// dc-lobby-server with its clan pages on the auth port + 8080 (the client's default, CLI-790).
struct Stack {
    lob: Proc,
    dir: PathBuf,
    log: PathBuf,
    auth: SocketAddr,
}

impl Stack {
    fn start(tag: &str, watchdog: &Watchdog) -> Stack {
        let dir = scratch(tag);
        let log = dir.join("lobby.log");
        let (port, lport, cport) = loop {
            let p = (free_tcp_port(), free_tcp_port(), free_tcp_port());
            let web = p.0.checked_add(8080);
            let free = web.is_some_and(|w| std::net::TcpListener::bind(("127.0.0.1", w)).is_ok());
            let mut v = vec![p.0, p.1, p.2, web.unwrap_or(0)];
            v.sort();
            v.dedup();
            if free && v.len() == 4 {
                break p;
            }
        };
        let mut cmd = Command::new(lobby_exe());
        // LG-310: the daily bonus box would sit over the screen these tests drive
        cmd.env("DC_REWARDS__NX_ENABLED", "false");
        cmd.args([
            "--port",
            &port.to_string(),
            "--lobbyport",
            &lport.to_string(),
        ])
        .args([
            "--clanport",
            &cport.to_string(),
            "--clanweb",
            &(port + 8080).to_string(),
        ])
        .arg("--db")
        .arg(dir.join("lobby.db"))
        .args(["--gameport", &free_udp_port().to_string()])
        .arg("--log")
        .arg(&log)
        .current_dir(&dir);
        let lob = Proc::spawn(cmd);
        watchdog.watch_pid(lob.pid());
        watchdog.watch_log(&log);
        let up = (0..600).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            [port, lport, cport, port + 8080]
                .iter()
                .all(|p| std::net::TcpStream::connect(("127.0.0.1", *p)).is_ok())
        });
        assert!(
            up,
            "lobby did not listen; log:\n{}",
            std::fs::read_to_string(&log).unwrap_or_default()
        );
        Stack {
            lob,
            auth: format!("127.0.0.1:{port}").parse().unwrap(),
            dir,
            log,
        }
    }

    fn stop(mut self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        self.lob.drain();
        if let Some(p) = self.lob.pid() {
            kill_pid(p);
        }
        let _ = self.lob.finish(Duration::from_secs(2));
        text
    }
}

const PASSWORD: &str = "pw-clan-2";

/// Logs in through the real widgets up to the lobby.
fn login(rom: PathBuf, stack: &Stack, user: &str) -> Menu {
    let mut link = LinkOptions::new(user);
    link.log = Some(stack.dir.join(format!("{user}_events.jsonl")));
    let mut m = Menu::new(MenuOptions {
        auth: stack.auth,
        user: user.into(),
        link,
        rom: Some(rom),
        font: Some((dc_client::testkit::synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    for c in PASSWORD.chars() {
        m.input(MenuInput::Char(c));
    }
    let failed = run(
        &mut m,
        r#"{"do":"click","widget":"login.login","handled":true}
{"do":"wait_screen","screen":"servers","timeout":20}
{"do":"click","widget":"servers.enter","after":1.0,"handled":true}
{"do":"wait_screen","screen":"channels","timeout":20}
{"do":"key","key":"down"}
{"do":"click","widget":"channels.enter","after":1.0,"handled":true}
{"do":"wait_screen","screen":"lobby","timeout":20}"#,
    );
    assert!(failed.is_empty(), "to the lobby: {failed:?}");
    m
}

/// Runs `script` against the menu (frames of 5 ms) until the steps are done; returns the failures.
fn run(m: &mut Menu, script: &str) -> Vec<String> {
    let mut r = StepRunner::new(parse(script).expect("steps parse"), true);
    let end = Instant::now() + Duration::from_secs(30);
    while !r.done() && Instant::now() < end {
        m.update();
        r.poll(&mut Here::Menu(m));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(r.done(), "steps did not finish: {:?}", r.failed);
    r.failed.clone()
}

/// Updates the menu until `ok` holds (at most `secs`).
fn until(m: &mut Menu, secs: u64, mut ok: impl FnMut(&mut Menu) -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        m.update();
        if ok(m) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn click(m: &mut Menu, r: [i32; 4]) {
    let p = [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32];
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

fn texts(m: &mut Menu) -> Vec<String> {
    m.items_at(0.5)
        .into_iter()
        .filter_map(|i| match i {
            DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

fn info(m: &Menu, id: i32) -> String {
    m.sheet()
        .part(INFO_WINDOW, id, WidgetKind::Text)
        .map(|p| p.text.clone())
        .unwrap_or_default()
}

fn cell(g: i32, k: i32) -> [i32; 4] {
    [
        GRID_AT[0] + k * CELL_STEP[0],
        GRID_AT[1] + g * GRID_DY,
        CELL,
        CELL,
    ]
}

/// Creates clan `name` through the create page (emblem `e` / background `b` on the grids' first pages).
fn create(m: &mut Menu, name: &str, e: i32, b: i32) {
    m.input(MenuInput::Command(CMD_CREATE));
    click(m, NAME_FIELD);
    for c in name.chars() {
        m.input(MenuInput::Char(c));
    }
    click(m, cell(0, e - 1));
    click(m, cell(1, b - 1));
    click(m, CREATE_BUTTON);
}

fn to_clan(m: &mut Menu) {
    m.open(Go::Clan);
    assert!(until(m, 10, |m| m.screen_name() == "clan" && m.fading().is_none()));
}

fn page_shown(m: &mut Menu) -> bool {
    let page = [
        PAGE[0] as f32,
        PAGE[1] as f32,
        PAGE[2] as f32,
        PAGE[3] as f32,
    ];
    m.items_at(0.5)
        .iter()
        .any(|i| matches!(i, DrawItem::Fill { rect, .. } if *rect == page))
}

#[test]
fn rom_create_clan_through_the_page_makes_the_master_screen() {
    let Some(rom) = rom() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let watchdog = Watchdog::arm("menu-clan-create", Duration::from_secs(240));
    let stack = Stack::start("menu-clan-create", &watchdog);
    let mut a = login(rom.clone(), &stack, "Founder");
    let mut b = login(rom, &stack, "Second");
    // b takes the name first (the server's refusal must reach a's page)
    to_clan(&mut b);
    create(&mut b, "Alpha", 2, 2);
    assert!(
        until(&mut b, 10, |m| m.popup().is_some()),
        "b's create answered"
    );
    b.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));

    to_clan(&mut a);
    a.input(MenuInput::Command(CMD_CREATE));
    assert!(
        page_shown(&mut a),
        "the create page is drawn in the web window"
    );
    a.input(MenuInput::Key(dc_client::menu::MenuKey::Escape));
    create(&mut a, "Alpha", 4, 7);
    let refused = "Cannot create the clan: that clan name is taken.";
    assert!(
        until(&mut a, 10, |m| texts(m).iter().any(|t| t == refused)),
        "the refusal is shown on the page"
    );
    // another name: created, the box carries the server's line, the screen is the master's
    for _ in 0..5 {
        a.input(MenuInput::Key(dc_client::menu::MenuKey::Backspace));
    }
    for c in "Bravo".chars() {
        a.input(MenuInput::Char(c));
    }
    click(&mut a, CREATE_BUTTON);
    assert!(
        until(&mut a, 10, |m| m.popup().map(|p| p.text.as_str())
            == Some("Clan Bravo created.")),
        "created; log:\n{}",
        std::fs::read_to_string(&stack.log).unwrap_or_default()
    );
    assert!(!page_shown(&mut a), "the web window closed");
    assert!(
        until(&mut a, 10, |m| info(m, NAME_TEXT) == "Bravo"
            && info(m, LEADER_TEXT) == "Founder"
            && info(m, MEMBERS_TEXT).starts_with('1')),
        "master screen: name {:?} leader {:?} members {:?}",
        info(&a, NAME_TEXT),
        info(&a, LEADER_TEXT),
        info(&a, MEMBERS_TEXT)
    );
    assert_eq!(a.data.char_extras.clan_mark, 4);
    assert_eq!(a.data.char_extras.clan_mark_ground, 7);
    assert!(texts(&mut a).iter().any(|t| t == "Founder"), "member row");
    let log = stack.stop();
    assert!(log.contains("'Bravo' created by account"), "{log}");
}

/// Item 2: a clanless player finds the clan on the find page, joins it with the row's Join button (the member view
/// follows: two rows, the member's own role, Clan Setting disabled), the leader's open screen refreshes to two
/// members; then the member leaves through the leave page (Leave Clan -> the page's confirmation -> Leave Clan) and
/// is clanless again, the leader back to one member.
#[test]
fn rom_find_join_and_leave_through_the_pages() {
    let Some(rom) = rom() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let watchdog = Watchdog::arm("menu-clan-join", Duration::from_secs(240));
    let stack = Stack::start("menu-clan-join", &watchdog);
    let mut a = login(rom.clone(), &stack, "Boss");
    let mut b = login(rom, &stack, "Recruit");
    to_clan(&mut a);
    create(&mut a, "Charlie", 1, 1);
    assert!(until(&mut a, 10, |m| m.popup().is_some()), "created");
    a.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
    assert!(until(&mut a, 10, |m| info(m, MEMBERS_TEXT).starts_with('1')));

    to_clan(&mut b);
    b.input(MenuInput::Command(CMD_FIND));
    assert!(
        until(&mut b, 10, |m| {
            let t = texts(m);
            t.iter().any(|x| x == "Charlie") && t.iter().any(|x| x == T_JOIN)
        }),
        "the find page lists the clan with Join"
    );
    let join = [
        ROW_BUTTON[0],
        ROW_Y - ROW_BUTTON[3] / 2,
        ROW_BUTTON[2],
        ROW_BUTTON[3],
    ];
    click(&mut b, join);
    assert!(
        until(&mut b, 10, |m| m.popup().map(|p| p.text.as_str())
            == Some("You joined Charlie.")),
        "joined"
    );
    b.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
    assert!(
        until(&mut b, 10, |m| info(m, NAME_TEXT) == "Charlie"
            && info(m, LEADER_TEXT) == "Boss"
            && info(m, MEMBERS_TEXT).starts_with('2')),
        "member view: {:?} {:?}",
        info(&b, NAME_TEXT),
        info(&b, MEMBERS_TEXT)
    );
    assert!(texts(&mut b).iter().any(|t| t == "Recruit"));
    assert_eq!(b.data.clan.members.len(), 2);
    // the leader's screen follows the pushes (0x3061 / 0x3062)
    assert!(until(&mut a, 10, |m| info(m, MEMBERS_TEXT).starts_with('2')));

    // leave: the button opens the confirmation page, its Leave Clan sends the leave
    b.input(MenuInput::Command(CMD_LEAVE));
    assert!(texts(&mut b).iter().any(|t| t == T_LEAVE_QUESTION));
    click(&mut b, LEAVE_BUTTON);
    assert!(
        until(&mut b, 10, |m| m.popup().map(|p| p.text.as_str())
            == Some("You left Charlie.")),
        "left"
    );
    b.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
    assert!(until(&mut b, 10, |m| m.data.char_extras.clan_id == 0
        && info(m, NAME_TEXT)
            == m.cat.text(dc_client::menu::clan::TEXT_UNAFFILIATED)));
    assert!(until(&mut a, 10, |m| info(m, MEMBERS_TEXT).starts_with('1')));
    let log = stack.stop();
    assert!(
        log.contains("joined clan") && log.contains("left clan"),
        "{log}"
    );
}

/// Item 3: clan chat typed in the chat window's field reaches the other online member's clan chat list (DeCombat
/// relay, CLI-563); the leader's Clan Setting page lists both members with Remove on the other one, and Remove takes
/// the member out (the page shows the server's line and one row; the removed member's screen turns clanless).
#[test]
fn rom_clan_chat_reaches_members_and_the_leader_removes_one() {
    let Some(rom) = rom() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let watchdog = Watchdog::arm("menu-clan-kick", Duration::from_secs(240));
    let stack = Stack::start("menu-clan-kick", &watchdog);
    let mut a = login(rom.clone(), &stack, "Chief");
    let mut b = login(rom, &stack, "Grunt");
    to_clan(&mut a);
    create(&mut a, "Delta", 1, 1);
    assert!(until(&mut a, 10, |m| m.popup().is_some()), "created");
    a.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
    to_clan(&mut b);
    b.input(MenuInput::Command(CMD_FIND));
    assert!(until(&mut b, 10, |m| texts(m).iter().any(|x| x == T_JOIN)));
    click(
        &mut b,
        [
            ROW_BUTTON[0],
            ROW_Y - ROW_BUTTON[3] / 2,
            ROW_BUTTON[2],
            ROW_BUTTON[3],
        ],
    );
    assert!(until(&mut b, 10, |m| m.popup().is_some()), "joined");
    b.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
    assert!(until(&mut a, 10, |m| info(m, MEMBERS_TEXT).starts_with('2')));

    // chat: the field of the chat window, Enter sends
    let field = a
        .sheet()
        .part(TALK_WINDOW, TALK_FIELD, WidgetKind::EditBox)
        .map(|p| [p.pos[0], p.pos[1], p.size[0].max(4), p.size[1].max(4)])
        .expect("chat field");
    click(&mut a, field);
    for c in "push left".chars() {
        a.input(MenuInput::Char(c));
    }
    a.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
    assert!(
        until(&mut b, 10, |m| texts(m)
            .iter()
            .any(|t| t == "Chief : push left")),
        "the member's clan chat shows the line"
    );
    assert!(until(&mut a, 10, |m| texts(m)
        .iter()
        .any(|t| t == "Chief : push left")));

    // the leader's member page: Remove on Grunt's row
    a.input(MenuInput::Command(CMD_MANAGE));
    assert!(
        until(&mut a, 10, |m| {
            let t = texts(m);
            t.iter().any(|x| x == "Grunt") && t.iter().any(|x| x == T_REMOVE)
        }),
        "manage page with Remove"
    );
    let row = a
        .data
        .clan
        .members
        .iter()
        .position(|m| m.name == "Grunt")
        .unwrap_or(1) as i32;
    let y = ROW_Y + row * ROW_STEP;
    click(
        &mut a,
        [
            ROW_BUTTON[0],
            y - ROW_BUTTON[3] / 2,
            ROW_BUTTON[2],
            ROW_BUTTON[3],
        ],
    );
    assert!(
        until(&mut a, 10, |m| {
            let t = texts(m);
            t.iter().any(|x| x == "Grunt was removed from the clan.")
                && !t.iter().any(|x| x == T_REMOVE)
        }),
        "removed: {:?}",
        texts(&mut a)
    );
    assert!(until(&mut b, 10, |m| m.data.char_extras.clan_id == 0));
    assert!(until(&mut a, 10, |m| info(m, MEMBERS_TEXT).starts_with('1')));
    stack.stop();
}

/// CLI-1020: after the create the lobby screen shows the new clan: the Character Info panel's clan text, and the
/// user list row of the character (its own and the other client's copy) carries the clan and mark.
#[test]
fn rom_lobby_shows_the_new_clan_after_the_create() {
    let Some(rom) = rom() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let watchdog = Watchdog::arm("menu-clan-lobby", Duration::from_secs(240));
    let stack = Stack::start("menu-clan-lobby", &watchdog);
    let mut a = login(rom.clone(), &stack, "Founder");
    let mut b = login(rom, &stack, "Second");
    to_clan(&mut a);
    create(&mut a, "Bravo", 4, 7);
    assert!(until(&mut a, 10, |m| m.popup().is_some()), "created");
    a.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
    assert!(until(&mut a, 10, |m| info(m, NAME_TEXT) == "Bravo"));
    a.open(Go::Lobby);
    assert!(until(&mut a, 10, |m| m.screen_name() == "lobby"
        && m.fading().is_none()));
    let panel = |m: &Menu| {
        m.sheet()
            .part(dc_client::menu::lobby::CHAR_WINDOW, 7, WidgetKind::Text)
            .map(|p| p.text.clone())
            .unwrap_or_default()
    };
    assert_eq!(panel(&a), "Bravo", "the panel's clan text");
    let me = a.data.profile.char_no;
    let row = |m: &Menu| m.data.users.iter().find(|u| u.account == me).cloned();
    assert!(
        until(&mut a, 10, |m| row(m)
            .is_some_and(|u| u.clan != 0 && u.mark == 4 && u.ground == 7)),
        "own row: {:?}",
        row(&a)
    );
    let other = |m: &Menu| m.data.users.iter().find(|u| u.account == me).cloned();
    assert!(
        until(&mut b, 10, |m| other(m)
            .is_some_and(|u| u.clan != 0 && u.mark == 4 && u.ground == 7)),
        "the other client's row: {:?}",
        other(&b)
    );
    stack.stop();
}
