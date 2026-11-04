//! w3-clan: the lobby bar's clan and messenger buttons, driven headless by the scripted steps (the same verbs and
//! widget names as the gate's `ui_lobby_buttons` / `ui_lobby_strict_walk` scenarios, strict hit-tests on the ROM
//! layout's rectangles): the clan button opens the clan screen with the select sound, the screen draws its page
//! (record, member rows, notice, the clan mark) and its exit / Escape go back to the lobby; the messenger button opens
//! the messenger window over the lobby with the friend rows, its close button and Escape close it. On the old code
//! both lobby buttons are dead and the steps fail.
//!
//! ROM-gated (skipped without `CA_ROM`). `dc-lobby-server` (+ `dc-dserver` with the made-up world) on ephemeral
//! loopback ports with a temporary DB; the fresh user has no clan, so the clan screen shows made-up clan data put in
//! the menu once the lobby is up. Every process is stopped by its own PID.
// SPEC: PROTOCOL_2009.md §5.1 (clan info / members / notice), §3.4 (0x306 location)

mod support;

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{ClanInfoView, ClanMemberRow, LinkOptions, CLAN_ROLE_LEADER};
use dc_client::menu::{Menu, MenuInput, MenuOptions};
use dc_client::sound::SoundCue;
use dc_client::steps::{parse, Here, StepRunner};
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

const PASSWORD: &str = "pw-clan-1";

/// Logs in through the real widgets (the steps' strict clicks) up to the lobby, then puts made-up clan and friend
/// data in the menu (the test user has no clan on the fresh server).
fn menu(rom: PathBuf, stack: &Stack, user: &str) -> Menu {
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
    let d = &mut m.data;
    let me = d.profile.char_no;
    d.char_extras.clan_id = 7;
    d.char_extras.clan_role = CLAN_ROLE_LEADER;
    d.char_extras.clan_mark = 1;
    d.char_extras.clan_mark_ground = 1;
    d.clan.info = Some(ClanInfoView {
        number: 7,
        max_members: 120,
        members: 2,
        rank: 3,
        level: 2,
        wins: 5,
        losses: 2,
        draws: 1,
        exp: 1500,
        name: "Testers".into(),
        ..ClanInfoView::default()
    });
    d.clan.members = vec![
        ClanMemberRow {
            char_no: me,
            role: CLAN_ROLE_LEADER,
            exp: 0,
            name: user.into(),
            online: true,
        },
        ClanMemberRow {
            char_no: me + 1000,
            role: 1,
            exp: 0,
            name: "Mate".into(),
            online: false,
        },
    ];
    d.clan.notice = "notice".into();
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

fn texts(items: &[DrawItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn clan_button_opens_the_clan_screen_and_messenger_button_the_window() {
    let Some(rom) = rom() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let watchdog = Watchdog::arm("menu-clan", Duration::from_secs(240));
    let stack = Stack::start("menu-clan", &watchdog);
    let mut m = menu(rom, &stack, "Kinsman");
    let sent = m.link_sent();
    let failed = run(
        &mut m,
        r#"{"id":"L8-clan","do":"click","widget":"lobby.clan","handled":true}
{"do":"expect","screen":"clan","within":5}
{"do":"wait_screen","screen":"clan","timeout":5}"#,
    );
    assert!(failed.is_empty(), "{failed:?}");
    assert!(
        m.link_sent() > sent,
        "the clan screen asked the lobby (0x3c2)"
    );
    assert!(m.trail.iter().any(|t| t == "clan"), "trail {:?}", m.trail);
    // the page and the screen's own rows: record, member rows, notice; the mark's two layers at the frame
    let items = m.items_at(0.0);
    let t = texts(&items);
    for want in ["Testers", "Kinsman", "Mate", "notice"] {
        assert!(t.iter().any(|x| x == want), "{want} drawn: {t:?}");
    }
    let marks = items
        .iter()
        // clan-functions (CLI-792): on the clan screen the mark covers the 51 x 51 frame
        .filter(|i| matches!(i, DrawItem::Image { dst, .. } if dst[2] == 51.0 && dst[3] == 51.0))
        .count();
    assert_eq!(marks, 2, "the clan mark's background and emblem");
    // exit button, then the lobby button again and Escape (the scenario's `dismiss`)
    let failed = run(
        &mut m,
        r#"{"id":"clan-exit","do":"click","widget":"clan.exit","handled":true}
{"do":"expect","screen":"lobby","within":5}
{"do":"wait_screen","screen":"lobby","timeout":5}
{"id":"L8-clan","do":"click","widget":"lobby.clan","handled":true}
{"do":"wait_screen","screen":"clan","timeout":5}
{"do":"dismiss"}
{"do":"expect","screen":"lobby","within":5}
{"do":"wait_screen","screen":"lobby","timeout":5}"#,
    );
    assert!(failed.is_empty(), "{failed:?}");
    let cues = m.take_sound_cues();
    assert!(
        cues.iter()
            .any(|c| matches!(c, SoundCue::Interface { what: "select", .. })),
        "the clicks' select sound: {cues:?}"
    );

    // the messenger window over the lobby: it asks the relay for the list, the `friends` line names the user
    let failed = run(
        &mut m,
        r#"{"id":"L9-messenger","do":"click","widget":"lobby.messenger","handled":true}"#,
    );
    assert!(failed.is_empty(), "{failed:?}");
    assert!(m.messenger_open());
    assert_eq!(m.screen_name(), "lobby");
    let deadline = Instant::now() + Duration::from_secs(10);
    while m.data.friends.me.is_empty() && Instant::now() < deadline {
        m.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(m.data.friends.me, "Kinsman", "the relay's friend list");
    let failed = run(&mut m, r#"{"do":"dismiss"}"#);
    assert!(failed.is_empty(), "{failed:?}");
    assert!(!m.messenger_open());
    // the button again; a press outside the window closes it and goes on to the screen
    let failed = run(
        &mut m,
        r#"{"id":"L9-messenger","do":"click","widget":"lobby.messenger","handled":true}"#,
    );
    assert!(failed.is_empty(), "{failed:?}");
    assert!(m.messenger_open());
    m.input(MenuInput::Press([10.0, 400.0]));
    m.input(MenuInput::Release([10.0, 400.0]));
    assert!(!m.messenger_open());
    let log = stack.stop();
    assert!(
        !log.contains("panicked"),
        "lobby log:
{log}"
    );
}
