//! shop-r10a item 2 (CLI-880): a mod bought on the Modification screen is attached to the shown weapon at once. The
//! Rust client against the Rust lobby: log in, open Modification, pick the first shop cell, Buy, the purchase window's
//! OK (CLI-1172: the window with the period list, as 2009); the lobby's 0x329
//! brings the record (inventory reread) and the client then sends 0x336 for the shown weapon.
//!
//! ROM-gated (skipped without `CA_ROM`).
// SPEC: PROTOCOL_2009.md §4.2 (0x328 / 0x329, 0x336 / 0x337)
// EVIDENCE: cshell+0x1588a0 (the purchase reply on the Modification page), cshell+0x15a110 (0x3f2 / 0x3f3),
// cshell+0x154e90 (the attach of the bought item)

mod support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use dc_client::menu::link::LinkOptions;
use dc_client::menu::pilot::{Pilot, PilotPlan, PilotRoom};
use dc_client::menu::{Menu, MenuInput, MenuOptions};
use dc_client::steps::{resolve, Resolved};
use dc_client::testkit::synth_font;
use support::*;

const PASSWORD: &str = "pw-tuning-buy";

fn tap(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

fn pump(m: &mut Menu, t: Duration, mut done: impl FnMut(&Menu) -> bool) -> bool {
    let end = Instant::now() + t;
    while Instant::now() < end {
        m.update();
        if done(m) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn start_lobby(dir: &Path, watchdog: &Watchdog) -> (Proc, SocketAddr, PathBuf) {
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
    let mut cmd = Command::new(lobby_exe());
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
    .args(["--gameport", &free_udp_port().to_string()])
    .args(["--start-gp", "1000000"])
    .arg("--log")
    .arg(&log)
    .current_dir(dir);
    let lob = Proc::spawn(cmd);
    watchdog.watch_pid(lob.pid());
    watchdog.watch_log(&log);
    let up = (0..600).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        std::net::TcpStream::connect(("127.0.0.1", lport)).is_ok()
            && std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    });
    assert!(up, "lobby did not listen");
    (lob, format!("127.0.0.1:{port}").parse().unwrap(), log)
}

#[test]
fn rom_a_mod_bought_on_modification_is_attached_to_the_shown_weapon() {
    let Some(rom) = dc_rom::resolve_rom().ok() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let watchdog = Watchdog::arm("menu-tuning-buy", Duration::from_secs(240));
    let dir = scratch("menu-tuning-buy");
    let (mut lob, auth, log) = start_lobby(&dir, &watchdog);
    let nick = "ModBuyer";
    let mut m = Menu::new(MenuOptions {
        auth,
        user: nick.into(),
        link: LinkOptions::new(nick),
        rom: Some(rom),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.set_idle_rules(false);
    let mut pilot = Pilot::new(PilotPlan {
        password: PASSWORD.into(),
        room: PilotRoom::Create {
            title: "mod buy".into(),
            mode: None,
            map: None,
            start_after: 2,
        },
        matches: 1,
        shots: None,
        timeout: Duration::from_secs(120),
    });
    let end = Instant::now() + Duration::from_secs(60);
    while m.screen_name() != "lobby" {
        assert!(Instant::now() < end, "no lobby; at {}", m.screen_name());
        m.update();
        for i in pilot.next(&m) {
            m.input(i);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(pump(&mut m, Duration::from_secs(5), |m| !m
        .data
        .store
        .items
        .is_empty()));
    let Resolved::Part { at, .. } = resolve(&m, "lobby.modification") else {
        panic!("no modification button");
    };
    tap(&mut m, at);
    assert!(pump(&mut m, Duration::from_secs(10), |m| m.screen_name()
        == "tuning"));
    pump(&mut m, Duration::from_millis(500), |_| false);
    let mods_before = m
        .data
        .store
        .items
        .iter()
        .filter(|r| (1000..2000).contains(&r.item_no))
        .count();
    // the first shop cell, Buy, the purchase window's OK
    tap(&mut m, [100.0, 480.0]);
    let Resolved::Part { at, .. } = resolve(&m, "cmd:1007") else {
        panic!("no Buy button");
    };
    tap(&mut m, at);
    use dc_client::menu::buybox::{BUTTON_SIZE, OK_AT, WINDOW_AT};
    tap(
        &mut m,
        [
            (WINDOW_AT[0] + OK_AT[0] + BUTTON_SIZE[0] / 2) as f32,
            (WINDOW_AT[1] + OK_AT[1] + BUTTON_SIZE[1] / 2) as f32,
        ],
    );
    // the purchase brings a new mod, and it ends up on a weapon without any further input
    let attached = pump(&mut m, Duration::from_secs(10), |m| {
        let mods: Vec<_> = m
            .data
            .store
            .items
            .iter()
            .filter(|r| (1000..2000).contains(&r.item_no))
            .collect();
        mods.len() > mods_before && mods.iter().any(|r| r.parent_uid != 0)
    });
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    lob.drain();
    if let Some(p) = lob.pid() {
        kill_pid(p);
    }
    let _ = lob.finish(Duration::from_secs(2));
    assert!(attached, "the bought mod was attached; lobby log:\n{text}");
}
