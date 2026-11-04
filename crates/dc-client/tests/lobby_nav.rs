//! lobby-nav: the front end's navigation against the Rust lobby, driven only by presses on the ROM layout's real
//! widget rectangles (never `MenuInput::Command`): the channel screen's server list button, the server screen
//! opened again over the lobby connection (0x3b0), the current server back to its channels, a server move (0x3b2 /
//! 0x3b3 and the new lobby login), the lobby's change channel / change server buttons and their black fade, the user
//! list's row menu with the profile request (0x378 / 0x379), the untouched "users online" text and the waiting
//! room's red countdown line with its start-count sound.
//!
//! ROM-gated (skipped without `CA_ROM`: the real rectangles come from the user's layout pages). `dc-lobby-server`
//! (+ `dc-dserver` with the made-up world) runs on ephemeral loopback ports with a temporary DB; every process is
//! stopped by its own PID.
// SPEC: PROTOCOL_2009.md §3.4 (0x3b0..0x3b3, 0x374 / 0x375), §6.1 (0x378 / 0x379), §7 (0x3044)

mod support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::{run_bot, BotConfig, RoomChoice};
use dc_client::hud::items::DrawItem;
use dc_client::menu::frontfx::{COUNTDOWN_COLOUR, SOUND_START_COUNT};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::lobby::{
    lobby_sheet, RowStyle, UserMenu, USER_COUNT, USER_LIST, USER_MENU_PROFILE, USER_MENU_TEXTS,
    USER_WINDOW,
};
use dc_client::menu::servers::{BarStyle, LIST_WINDOW, SERVER_LIST};
use dc_client::menu::sheet::list_row;
use dc_client::menu::{channels, charinfo, create, lobby, login, room, servers};
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions, MenuOutcome};
use dc_client::sound::{SoundCue, SoundDirector};
use dc_client::testkit::synth_font;
use dc_ui_data::{UiScreen, WidgetKind};
use support::*;

const PASSWORD: &str = "pw-nav-1";

/// dc-lobby-server (+ dc-dserver for the made-up world) on loopback ports.
struct Stack {
    lob: Proc,
    dir: PathBuf,
    log: PathBuf,
    auth: SocketAddr,
}

impl Stack {
    fn start(tag: &str, watchdog: &Watchdog) -> Stack {
        let lobby = lobby_exe();
        let dserver = dserver_exe();
        let dir = scratch(tag);
        let world = synth_world_file(&dir);
        let log = dir.join("lobby.log");
        let (port, lport, cport, web) = loop {
            let p = (
                free_tcp_port(),
                free_tcp_port(),
                free_tcp_port(),
                free_tcp_port(),
            );
            let mut v = vec![p.0, p.1, p.2, p.3, p.1.wrapping_add(1)];
            v.sort();
            v.dedup();
            if v.len() == 5 {
                break p;
            }
        };
        let gameport = free_udp_port();
        let dargs = format!(
            "-worldfile {} -announce Worlds\\Synth.dat -tick 20 -roundsecs 12 -holds 1500,800",
            world.display()
        );
        let mut cmd = Command::new(&lobby);
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
            &web.to_string(),
        ])
        .arg("--db")
        .arg(dir.join("lobby.db"))
        .args(["--countdown", "0.05", "--gameport", &gameport.to_string()])
        .arg("--log")
        .arg(&log)
        .arg("--dserver")
        .arg("--dserver-exe")
        .arg(&dserver)
        .arg("--dserver-logdir")
        .arg(&dir)
        .args(["--dserver-world", "Worlds\\Synth", "--dserver-args", &dargs])
        .current_dir(&dir);
        let lob = Proc::spawn(cmd);
        watchdog.watch_pid(lob.pid());
        watchdog.watch_log(&log);
        let up = (0..600).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            [port, lport, cport]
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
        for p in logged_pids(&text) {
            kill_pid(p);
        }
        text
    }
}

/// Move, press and release at `p`.
fn press_at(menu: &mut Menu, p: [f32; 2]) {
    menu.input(MenuInput::Move(p));
    menu.input(MenuInput::Press(p));
    menu.input(MenuInput::Release(p));
}

/// A click on the centre of the visible, enabled button with `command` of the current screen's layout (the test
/// fails when the screen has no such button: no command shortcut).
#[track_caller]
fn click(menu: &mut Menu, command: i32) {
    let b = menu.sheet().button(command).unwrap_or_else(|| {
        panic!(
            "screen {} has no visible button {command}",
            menu.screen_name()
        )
    });
    assert!(
        b.enabled,
        "button {command} of {} is disabled",
        menu.screen_name()
    );
    let r = b.rect();
    let c = [
        r[0] as f32 + r[2] as f32 / 2.0,
        r[1] as f32 + r[3] as f32 / 2.0,
    ];
    press_at(menu, c);
}

/// Updates the menu until `done` holds (false at the time-out).
fn pump(menu: &mut Menu, secs: u64, mut done: impl FnMut(&Menu) -> bool) -> bool {
    let end = Instant::now() + Duration::from_millis(if secs == 1 { 500 } else { secs * 1000 });
    while Instant::now() < end {
        menu.update();
        if done(menu) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// Half a second of updates (the reply to a screen's request arrives).
fn settle(menu: &mut Menu) {
    pump(menu, 1, |_| false);
}

#[track_caller]
fn wait_screen(menu: &mut Menu, name: &str) {
    let ok = pump(menu, 20, |m| {
        m.screen_name() == name && m.fading().is_none() && m.popup().is_none()
    });
    assert!(
        ok,
        "screen {name} expected, on {} (box {:?}, trail {:?})",
        menu.screen_name(),
        menu.popup().map(|p| p.text.clone()),
        menu.trail
    );
}

/// The fade of a screen change: the old screen stays up under a rising black, the new one opens at black after
/// 600 ms, the black lifts over 600 ms more; the fader is the last thing drawn.
#[track_caller]
fn check_fade(menu: &mut Menu, from: &str, to: Go, to_name: &str) {
    assert!(
        pump(menu, 10, |m| m.fading().is_some()),
        "a fade {from} -> {to_name}: trail {:?}",
        menu.trail
    );
    let t0 = Instant::now();
    let (target, a0) = menu.fading().unwrap();
    assert_eq!(target, to);
    assert_eq!(
        menu.screen_name(),
        from,
        "the old screen stays during the fade out"
    );
    assert!(a0 < 200, "the black rises from 0 ({a0})");
    // the fader covers the screen; only the cursor image (L1, drawn after everything) is over it
    let items = menu.items();
    let mut top = items
        .iter()
        .rev()
        .skip_while(|i| dc_client::menu::cursor::is_cursor(i));
    match top.next() {
        Some(DrawItem::Image { dst, .. }) => assert_eq!(dst[2..], [1024.0, 768.0]),
        Some(DrawItem::Fill { rect, .. }) => assert_eq!(rect[2..], [1024.0, 768.0]),
        other => panic!("the fader is drawn last: {other:?}"),
    }
    assert!(pump(menu, 5, |m| m.screen_name() == to_name));
    let switched = t0.elapsed();
    assert!(
        switched >= Duration::from_millis(500),
        "the screen changes at black, not at once ({switched:?})"
    );
    let a1 = menu.fading().map(|f| f.1);
    assert!(a1.is_some_and(|a| a > 150), "black at the change: {a1:?}");
    assert!(pump(menu, 5, |m| m.fading().is_none()));
    assert!(
        t0.elapsed() >= Duration::from_millis(1000),
        "two 600 ms halves"
    );
}

/// The centre of row `i` of the server list.
fn server_row(menu: &Menu, i: usize) -> [f32; 2] {
    let list = menu
        .sheet()
        .part(LIST_WINDOW, SERVER_LIST, WidgetKind::List)
        .expect("server list")
        .clone();
    let st = BarStyle::load(&menu.cat, UiScreen::Screen11);
    let at = list_row(&list, st.size[1], i, 0).expect("row in the list");
    [
        (at[0] + st.size[0] / 2) as f32,
        (at[1] + st.size[1] / 2) as f32,
    ]
}

/// The centre of row `i` of the lobby's user list.
fn user_row(menu: &Menu, i: usize) -> [f32; 2] {
    let list = menu
        .sheet()
        .part(USER_WINDOW, USER_LIST, WidgetKind::List)
        .expect("user list")
        .clone();
    let st = RowStyle::load(&menu.cat);
    let at = list_row(&list, st.user_size[1], i, 0).expect("row in the list");
    [
        (at[0] + st.user_size[0] / 2) as f32,
        (at[1] + st.user_size[1] / 2) as f32,
    ]
}

/// C2S lobby frames of the menu's capture: (id, the u32 at +6 of the frame; the capture keeps 8 bytes before it).
fn c2s_frames(path: &Path) -> Vec<(u16, u32)> {
    let d = dc_botclient::capdecode::decode_file(path).expect("menu capture");
    d.tcp
        .into_iter()
        .filter(|f| f.2)
        .map(|(_, _, _, id, b)| {
            let v = b
                .get(14..18)
                .map_or(0, |x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]));
            (id, v)
        })
        .collect()
}

#[test]
fn rom_lobby_navigation_by_the_real_widgets() {
    let Some(rom) = std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
    else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let watchdog = Watchdog::arm("lobby-nav", Duration::from_secs(300));
    let stack = Stack::start("lobby-nav", &watchdog);
    let auth = stack.auth;
    let dir = stack.dir.clone();
    let mut link = LinkOptions::new("NavUser");
    link.capture = Some(dir.join("menu_capture.jsonl"));
    link.log = Some(dir.join("menu_events.jsonl"));
    let mut menu = Menu::new(MenuOptions {
        auth,
        user: "NavUser".into(),
        link,
        rom: Some(rom),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });

    // login (the remembered ID puts the keyboard in the password box)
    for c in PASSWORD.chars() {
        menu.input(MenuInput::Char(c));
    }
    click(&mut menu, login::CMD_LOGIN);
    wait_screen(&mut menu, "servers");
    assert!(pump(&mut menu, 5, |m| !m.data.servers.is_empty()));
    click(&mut menu, servers::CMD_ENTER);
    wait_screen(&mut menu, "channels");
    assert!(menu.data.lobby_up);
    let first_server = menu.data.server.clone().expect("current server").id;

    // L3 / L6: the channel screen's server list button is live and opens the server screen, which asks the lobby
    // for the list again
    click(&mut menu, channels::CMD_SERVER_LIST);
    assert_eq!(
        menu.screen_name(),
        "servers",
        "server list at once (no fade)"
    );
    assert!(menu.fading().is_none());
    wait_screen(&mut menu, "servers");
    assert!(pump(&mut menu, 5, |m| m.data.servers.len() >= 2));
    // the list request's answer ends the screen's busy state (a pick waits for it, as the 2009 screen does)
    settle(&mut menu);
    // the current server is highlighted: Enter goes back to its channels without a move
    click(&mut menu, servers::CMD_ENTER);
    assert_eq!(menu.screen_name(), "channels");
    wait_screen(&mut menu, "channels");
    assert!(pump(&mut menu, 5, |m| !m.data.channels.is_empty()));

    // L7: entering a channel fades to the lobby
    menu.input(MenuInput::Key(MenuKey::Down));
    click(&mut menu, channels::CMD_ENTER);
    check_fade(&mut menu, "channels", Go::Lobby, "lobby");
    wait_screen(&mut menu, "lobby");

    // L10: the user list's text 0 keeps the layout's own text (no user count over the window's caption)
    assert!(pump(&mut menu, 5, |m| !m.data.users.is_empty()));
    let layout_text = lobby_sheet(&menu.cat)
        .part(USER_WINDOW, USER_COUNT, WidgetKind::Text)
        .map(|p| p.text.clone());
    let shown = menu
        .sheet()
        .part(USER_WINDOW, USER_COUNT, WidgetKind::Text)
        .map(|p| p.text.clone());
    assert_eq!(shown, layout_text, "users-online text untouched");

    // w2-lobbychat (L13 / M6): typed text and Enter go to dc-lobby-server as 0x3a0; its echo 0x3051 is the line
    let nick0 = menu.data.profile.nick.clone();
    for c in "nav says hi".chars() {
        menu.input(MenuInput::Char(c));
    }
    menu.input(MenuInput::Key(MenuKey::Enter));
    let line = format!("{nick0} : nav says hi");
    assert!(
        pump(&mut menu, 5, |m| m.data.chat.contains(&line)),
        "echoed chat line {line:?}: {:?}",
        menu.data.chat
    );
    assert_eq!(
        menu.screen_name(),
        "lobby",
        "Enter with chat text does not join a room"
    );

    // L11: a press on the own user row selects it and opens the user menu; its profile entry asks 0x378 and the
    // 0x379 answer opens a box with the name
    let me = menu.data.profile.my_id;
    let nick = menu.data.profile.nick.clone();
    let row = menu
        .data
        .users
        .iter()
        .position(|u| u.id == me)
        .expect("own user row");
    let p = user_row(&menu, row);
    menu.input(MenuInput::Move(p));
    let before_bar = menu.items().len();
    press_at(&mut menu, p);
    let items = menu.items();
    let profile_label = menu.cat.text(USER_MENU_TEXTS[USER_MENU_PROFILE]);
    let has_label = |items: &[DrawItem], t: &str| {
        items.iter().any(|i| match i {
            DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => text == t,
            _ => false,
        })
    };
    assert!(
        has_label(&items, &profile_label),
        "the user menu shows its profile entry ({profile_label:?})"
    );
    assert!(items.len() > before_bar, "menu box drawn");
    let m = UserMenu {
        at: [p[0] as i32, p[1] as i32],
        row,
        id: me,
    };
    let e = m.entry_rect(USER_MENU_PROFILE);
    press_at(
        &mut menu,
        [(e[0] + e[2] / 2) as f32, (e[1] + e[3] / 2) as f32],
    );
    // CLI-931 (56c2c28; amended by CLI-961): the profile reply 0x379 opens the Character Info box with the user's
    // profile (cshell+0x35cf0 / cshell+0x2412c0), no longer an OK message box; Escape closes it back to the lobby
    assert!(
        pump(&mut menu, 5, |m| m.screen_name() == charinfo::NAME
            && m.data.char_view.as_ref().is_some_and(|v| v.name == nick)),
        "Character Info with the name {nick:?}: on {}, view {:?}",
        menu.screen_name(),
        menu.data.char_view.as_ref().map(|v| v.name.clone())
    );
    menu.input(MenuInput::Key(MenuKey::Escape));
    wait_screen(&mut menu, "lobby");
    assert!(
        !has_label(&menu.items(), &profile_label),
        "the menu closed with its entry"
    );

    // L12: change channel -> the channel screen, change server -> the server screen, both through the fade
    click(&mut menu, lobby::CMD_CHANGE_CHANNEL);
    check_fade(&mut menu, "lobby", Go::Channels, "channels");
    wait_screen(&mut menu, "channels");
    assert!(pump(&mut menu, 5, |m| !m.data.channels.is_empty()));
    menu.input(MenuInput::Key(MenuKey::Down));
    click(&mut menu, channels::CMD_ENTER);
    check_fade(&mut menu, "channels", Go::Lobby, "lobby");
    wait_screen(&mut menu, "lobby");
    click(&mut menu, lobby::CMD_EXIT);
    check_fade(&mut menu, "lobby", Go::Servers, "servers");
    wait_screen(&mut menu, "servers");
    assert!(pump(&mut menu, 5, |m| m.data.servers.len() >= 2));
    // the list request's answer ends the screen's busy state (a pick waits for it, as the 2009 screen does)
    settle(&mut menu);

    // the server move: the other server's row, Enter -> 0x3b2, the new lobby login, its channels
    let other = menu
        .data
        .servers
        .iter()
        .position(|e| e.id != first_server)
        .expect("a second server");
    let other_id = menu.data.servers[other].id;
    let p = server_row(&menu, other);
    press_at(&mut menu, p);
    // the other server is the clan server: without a clan a box, no move
    click(&mut menu, servers::CMD_ENTER);
    assert_eq!(menu.screen_name(), "servers");
    assert!(menu.popup().is_some(), "the clan server's box");
    menu.input(MenuInput::Key(MenuKey::Enter));
    menu.data.char_extras.clan_id = 1;
    click(&mut menu, servers::CMD_ENTER);
    wait_screen(&mut menu, "channels");
    assert_eq!(menu.data.server.as_ref().map(|s| s.id), Some(other_id));
    // and back to the first server for the room (the walker plays there)
    click(&mut menu, channels::CMD_SERVER_LIST);
    wait_screen(&mut menu, "servers");
    assert!(pump(&mut menu, 5, |m| m.data.servers.len() >= 2));
    // the list request's answer ends the screen's busy state (a pick waits for it, as the 2009 screen does)
    settle(&mut menu);
    let first = menu
        .data
        .servers
        .iter()
        .position(|e| e.id == first_server)
        .unwrap();
    let p = server_row(&menu, first);
    press_at(&mut menu, p);
    click(&mut menu, servers::CMD_ENTER);
    wait_screen(&mut menu, "channels");
    assert_eq!(menu.data.server.as_ref().map(|s| s.id), Some(first_server));
    assert!(pump(&mut menu, 5, |m| !m.data.channels.is_empty()));
    menu.input(MenuInput::Key(MenuKey::Down));
    click(&mut menu, channels::CMD_ENTER);
    wait_screen(&mut menu, "lobby");

    // L16: a room with the walker, Start, the lobby's countdown: red lines and one start-count sound per tick
    click(&mut menu, lobby::CMD_CREATE);
    wait_screen(&mut menu, "create");
    click(&mut menu, create::CMD_OK);
    wait_screen(&mut menu, "room");
    let bot = std::thread::spawn(move || {
        let mut cfg = BotConfig::new(auth, "NavWalker");
        cfg.room = RoomChoice::Any;
        cfg.motion = MotionKind::Circle;
        cfg.duration = Duration::from_secs(10);
        cfg.use_rom = false;
        cfg.load_ms = 300;
        cfg.match_wait = Duration::from_secs(40);
        cfg.lobby_timeout = Duration::from_secs(40);
        run_bot(&cfg, &Recorder::null("NavWalker"))
    });
    assert!(
        pump(&mut menu, 40, |m| {
            m.data.members.len() >= 2
                && m.data
                    .members
                    .iter()
                    .filter(|(id, _)| **id != m.data.profile.my_id)
                    .all(|(_, x)| x.state == 1)
        }),
        "the walker joined and got ready: {:?}",
        menu.data.members
    );
    // w2-uisound (M11): every click of the walk so far played the select sound (the server screen's common window
    // the navigation sounds) and the pointer moves onto buttons the select-change sound; all of them reach the sound
    // log as interface starts (written next to the stack's logs for gate/audio_check.py)
    // (handed over by the window's own per-frame hook, Menu::sound_frame: the screen's music cue too, so every
    // client log has its music lines)
    let sound_log = dir.join("menu.sound.jsonl");
    let mut sd = SoundDirector::open(menu.cat.rom.as_deref());
    sd.log_to(&sound_log).unwrap();
    menu.sound_frame(Some(&mut sd));
    assert!(
        menu.pending_sounds().is_empty(),
        "the frame took the walk's cues"
    );
    let chat_before = menu.data.chat.len();
    click(&mut menu, room::CMD_START);
    // w3-clan: a click on a button plays the select sound (interface sound 0x14); the countdown's sounds follow
    let clicked = menu.take_sound_cues();
    assert!(
        matches!(
            clicked.last(),
            Some(SoundCue::Interface { what: "select", .. })
        ),
        "the start click's select sound: {clicked:?}"
    );
    let started = pump(&mut menu, 20, |m| {
        let red = (0..m.data.chat.len())
            .filter(|i| m.data.chat_colour_of(*i) == COUNTDOWN_COLOUR)
            .count();
        red >= 5 || m.has_outcome()
    });
    assert!(started, "countdown lines: {:?}", menu.data.chat);
    let red: Vec<String> = (chat_before..menu.data.chat.len())
        .filter(|i| menu.data.chat_colour_of(*i) == COUNTDOWN_COLOUR)
        .map(|i| menu.data.chat[i].clone())
        .collect();
    println!("countdown lines: {red:?}");
    assert!(!red.is_empty(), "red countdown lines");
    let countdown_text = menu.cat.text(0x57a);
    assert!(red.iter().all(|l| l.starts_with(&countdown_text)));
    let sounds = menu.pending_sounds().to_vec();
    assert_eq!(
        sounds.iter().filter(|s| **s == SOUND_START_COUNT).count(),
        red.len(),
        "one start-count sound per line: {sounds:?}"
    );
    menu.sound_frame(Some(&mut sd));
    assert!(menu.pending_sounds().is_empty());
    drop(sd);
    let names: Vec<String> = std::fs::read_to_string(&sound_log)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["trigger"] == "interface" && v["file_hash"].is_string())
        .map(|v| v["detail"].as_str().unwrap_or_default().to_string())
        .collect();
    let count = |w: &str| {
        names
            .iter()
            .filter(|n| n.contains(&format!("\"{w}\"")))
            .count()
    };
    println!(
        "sound log {}: select {} select_change {} nav_select {} start_count {}",
        sound_log.display(),
        count("select"),
        count("select_change"),
        count("nav_select"),
        count("start_count")
    );
    assert!(count("select") >= 10, "the walk's clicks: {names:?}");
    assert!(count("select_change") >= 1);
    assert_eq!(count("start_count"), red.len());
    // the match starts: the menu hands its lobby connection over (not played here)
    if let Some(MenuOutcome::Play { lobby, .. }) = {
        pump(&mut menu, 20, |m| m.has_outcome());
        menu.take_outcome()
    } {
        drop(lobby);
    }
    drop(menu);
    drop(bot);
    let text = stack.stop();

    // the wire: 0x3b0 at each server screen after the first, one 0x3b2 per move with the entry's id, a lobby login
    // per connection, 0x378 with the own in-channel id, 0x374 for each channel leave
    let f = c2s_frames(&dir.join("menu_capture.jsonl"));
    let count = |id: u16| f.iter().filter(|x| x.0 == id).count();
    println!(
        "c2s: 3b0 {} 3b2 {:?} 300 {} 378 {:?} 374 {}",
        count(0x3b0),
        f.iter().filter(|x| x.0 == 0x3b2).collect::<Vec<_>>(),
        count(0x300),
        f.iter().filter(|x| x.0 == 0x378).collect::<Vec<_>>(),
        count(0x374)
    );
    assert_eq!(
        count(0x3b0),
        3,
        "server list asked by each later server screen"
    );
    let moves: Vec<u32> = f.iter().filter(|x| x.0 == 0x3b2).map(|x| x.1).collect();
    assert_eq!(
        moves,
        vec![other_id, first_server],
        "two moves; lobby log:\n{text}"
    );
    assert_eq!(count(0x300), 3, "a lobby login per lobby connection");
    assert!(
        f.contains(&(0x378, me)),
        "profile asked with the in-channel id"
    );
    assert_eq!(count(0x374), 2, "two channel leaves");
}
