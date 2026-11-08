//! The 2009 front end against the Rust lobby: `dc-lobby-server` on ephemeral loopback ports (temporary DB)
//! supervises `dc-dserver` with the made-up world; the menu (scripted player, headless) logs in, confirms the
//! server and the channel, then either creates a room and starts it once a dc-botclient walker joined and got ready,
//! or joins the walker's listed room and gets ready; the game session joins on the handed-over lobby connection and
//! plays until the server ends the round; the menu is back in the waiting room on the same lobby connection
//! afterwards. The typed password never reaches a file.
//!
//! With `CA_ROM` set the screens use the ROM's layouts (clicks on the layout's buttons) and `DC_MENU_SHOTS` names a
//! folder for one PNG per screen (ROM-derived: local files, never committed); without it the screens run on their
//! command numbers. Every process is stopped by its own PID.
// SPEC: PROTOCOL_2009.md §2-§3, §7; DSERVER_PIPE.md §2-§5
// HARNESS: lobby_harness::login, ::lobby, ::test_main_suite (the lobby flow and the 0x3043 port at +0x28)

mod support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::{run_bot, BotConfig, RoomChoice};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::pilot::{
    headless_runner, run as run_pilot, Pilot, PilotPlan, PilotReport, PilotRoom,
};
use dc_client::menu::{Menu, MenuOptions};
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{Connect, HeadlessOptions, NetOptions, WorldFile};
use support::*;

/// The password the scripted player types into the password box.
const PASSWORD: &str = "pw-secret-77";

/// UTF-16LE bytes of `s`.
fn wide(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
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
        Stack::start_with(tag, watchdog, &[])
    }

    /// [`Stack::start`] with extra environment variables for the lobby (config overrides `DC_<SECTION>__<KEY>`).
    fn start_with(tag: &str, watchdog: &Watchdog, env: &[(&str, &str)]) -> Stack {
        Stack::start_full(tag, watchdog, env, &[])
    }

    /// [`Stack::start_with`] with extra lobby flags.
    fn start_full(tag: &str, watchdog: &Watchdog, env: &[(&str, &str)], extra: &[&str]) -> Stack {
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
        .args(extra)
        .envs(env.iter().copied())
        .current_dir(&dir);
        let lob = Proc::spawn(cmd);
        watchdog.watch_pid(lob.pid());
        watchdog.watch_log(&log);
        let up = (0..600).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            std::net::TcpStream::connect(("127.0.0.1", lport)).is_ok()
                && std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
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

    /// Stops the lobby and its game servers by PID; returns the lobby log.
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

/// What one scripted menu run saw.
struct Outcome {
    report: PilotReport,
    lines: Vec<String>,
    screen_after: &'static str,
    members_after: usize,
    rom: bool,
}

/// The menu as `nick` with `room` while the walker plays `bot_room` in the same lobby; `sub` names the image
/// folder.
fn scenario(
    stack: &Stack,
    nick: &str,
    room: PilotRoom,
    bot_room: RoomChoice,
    sub: &str,
) -> Outcome {
    let auth = stack.auth;
    let dir = &stack.dir;
    let mut base = NetOptions::new(
        Connect::Lobby {
            auth,
            room: RoomChoice::Any,
            password: String::new(),
            channel: None,
        },
        nick,
    );
    base.world_file = Some({
        let s = synth_source();
        WorldFile {
            bytes: s.bytes,
            stand_dims: SYNTH_DIMS,
            textures: s.textures,
        }
    });
    base.match_wait = Duration::from_secs(30);
    base.result_hold = Duration::from_millis(1000);
    base.log = Some(dir.join("client_events.jsonl"));
    base.capture = Some(dir.join("client_capture.jsonl"));
    let mut link = LinkOptions::new(nick);
    link.log = Some(dir.join("menu_events.jsonl"));
    link.capture = Some(dir.join("menu_capture.jsonl"));
    let rom = std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir());
    let shots = std::env::var_os("DC_MENU_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("shots"))
        .join(sub);
    let mut menu = Menu::new(MenuOptions {
        auth,
        user: nick.into(),
        link,
        rom: rom.clone(),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    println!("menu: {}", menu.source());
    let bot = std::thread::spawn(move || {
        let mut cfg = BotConfig::new(auth, "MenuWalker");
        cfg.room = bot_room;
        cfg.motion = MotionKind::Circle;
        cfg.duration = Duration::from_secs(30);
        cfg.use_rom = false;
        cfg.load_ms = 300;
        cfg.match_wait = Duration::from_secs(40);
        run_bot(&cfg, &Recorder::null("MenuWalker"))
    });
    let mut pilot = Pilot::new(PilotPlan {
        password: PASSWORD.into(),
        room,
        matches: 1,
        shots: Some(shots.clone()),
        timeout: Duration::from_secs(120),
    });
    let hopts = HeadlessOptions {
        frames: 60 * 30,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 12,
        font_bytes: Some(synth_font()),
        realtime: true,
        stop_after_over: Some(60),
        ..HeadlessOptions::default()
    };
    let mut lines = Vec::new();
    let report = {
        let mut play = headless_runner(&base, nick.into(), &hopts, &mut lines);
        run_pilot(&mut menu, &mut pilot, &mut play)
    };
    let out = Outcome {
        report,
        lines,
        screen_after: menu.screen_name(),
        members_after: menu.data.members.len(),
        rom: rom.is_some(),
    };
    drop(menu);
    let bot = bot.join().expect("bot thread");
    println!("trail: {}", out.report.trail.join(" -> "));
    for l in &out.lines {
        println!("  {l}");
    }
    for b in &out.report.boxes {
        println!("  box: {b}");
    }
    for b in &out.report.notices {
        println!("  notice (closes by itself, not an error): {b}");
    }
    println!("  box open at the end: {:?}", out.report.open_at_end);
    println!(
        "  end: {} | shots {} in {}",
        out.report.end,
        out.report.shots.len(),
        shots.display()
    );
    println!(
        "  after: screen {} members {}",
        out.screen_after, out.members_after
    );
    match &bot {
        Ok(v) => println!("  bot: stage {} end {}", v["stage"], v["end"]),
        Err(e) => println!("  bot error: {e}"),
    }
    assert!(bot.is_ok(), "the walker played: {bot:?}");
    out
}

fn check(o: &Outcome, want: &[&str], dir: &Path, text: &str) {
    assert_eq!(o.report.matches, 1, "one match played; lobby log:\n{text}");
    // error boxes (those with buttons) fail the run; a notice (the connecting wait box) is fine while it closes by
    // itself, so no box of any kind may still be open at the end
    assert!(
        o.report.boxes.is_empty(),
        "no error box: {:?} (notices {:?})",
        o.report.boxes,
        o.report.notices
    );
    assert_eq!(
        o.report.open_at_end, None,
        "no box left open at the end (notices {:?})",
        o.report.notices
    );
    let mut at = 0;
    for step in &o.report.trail {
        if at < want.len() && step == want[at] {
            at += 1;
        }
    }
    assert_eq!(at, want.len(), "screen flow {:?}", o.report.trail);
    assert!(
        o.lines.iter().any(|l| l.contains("world Some")),
        "the game session loaded the announced world: {:?}",
        o.lines
    );
    assert_eq!(o.screen_after, "room", "back in the waiting room");
    assert!(o.members_after >= 1, "the waiting room's members came back");
    // the password box text went out once in the login request and is in no file the client wrote
    for f in [
        "menu_events.jsonl",
        "menu_capture.jsonl",
        "client_events.jsonl",
        "client_capture.jsonl",
    ] {
        let b = std::fs::read(dir.join(f)).unwrap_or_default();
        assert!(
            !contains(&b, PASSWORD.as_bytes()) && !contains(&b, &wide(PASSWORD)),
            "{f} holds the password"
        );
    }
    // CLI-1941: both event logs open with the run stamp, before any other event
    for f in ["menu_events.jsonl", "client_events.jsonl"] {
        let text = std::fs::read_to_string(dir.join(f)).unwrap_or_default();
        let first = text.lines().next().unwrap_or_default();
        let ev: serde_json::Value = serde_json::from_str(first).unwrap_or_default();
        assert_eq!(ev["ev"], "run", "{f} first event: {first}");
        assert_eq!(
            ev["pid"],
            serde_json::json!(std::process::id()),
            "{f}: {first}"
        );
        for key in ["run", "version", "commit", "target", "profile", "started"] {
            assert!(!ev[key].is_null(), "{f}: {key} missing in {first}");
        }
    }
    if o.rom {
        assert!(
            o.report.shots.len() + 2 >= want.len(),
            "one image per screen: {:?}",
            o.report.shots
        );
    }
    // the lobby locations of the match end: 0xb when the client left the game server (the result screen took the
    // focus), then 1 with the waiting room's requests when the result screen closed
    // OBSERVED: capture 20261025_204341_realclient (684.047 0x306 {0xb}; 746.047 0x306 {1},
    // 0x388, 0x38a)
    let mut frames: Vec<(f64, u16, u32)> = Vec::new();
    for f in ["menu_capture.jsonl", "client_capture.jsonl"] {
        if let Ok(d) = dc_botclient::capdecode::decode_file(&dir.join(f)) {
            for (t, _, c2s, id, bytes) in d.tcp {
                if c2s && matches!(id, 0x306 | 0x388 | 0x38a) {
                    let v = bytes
                        .get(14..18)
                        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                    frames.push((t, id, v));
                }
            }
        }
    }
    frames.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!("  lobby location frames: {frames:?}");
    let result_at = frames
        .iter()
        .position(|f| f.1 == 0x306 && f.2 == 0xb)
        .expect("0x306 {0xb} at the leave");
    let room_at = frames[result_at..]
        .iter()
        .position(|f| f.1 == 0x306 && f.2 == 1)
        .map(|i| i + result_at)
        .expect("0x306 {1} after the result screen");
    let after: Vec<u16> = frames[room_at..].iter().map(|f| f.1).collect();
    assert!(
        after.starts_with(&[0x306, 0x388, 0x38a]),
        "the waiting room's requests after the result screen: {after:x?}"
    );
}

#[test]
fn menu_logs_in_creates_a_room_starts_plays_and_returns_to_the_waiting_room() {
    // a hard limit: a run takes about 30 s alone; a lost peer must fail it, never hang it
    let watchdog = Watchdog::arm("menu-host", Duration::from_secs(300));
    let stack = Stack::start("menu-host", &watchdog);
    // the host names TDM: since Elimination Pro became playable (LG-70) the creation box opens on mode row 8 as the
    // 2009 box does (`menu::create::START_MODE_ROW`), a 7-round match that outlasts this 30 s run, so the result screen
    // never came; TDM ends on its short test clock. Without the ROM the box has no mode list and keeps its own mode.
    let tdm = std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .and_then(|rom| dc_bute::GameRoomData::from_rom(&rom).ok())
        .and_then(|g| {
            dc_client::menu::pilot::RoomNames::from_rows(&g.maps(), &g.modes(false))
                .mode("TDM")
                .ok()
        });
    let room = PilotRoom::Create {
        title: "menu test".into(),
        mode: tdm,
        map: None,
        start_after: 2,
    };
    let o = scenario(&stack, "MenuHost", room, RoomChoice::Any, "host");
    let dir = stack.dir.clone();
    let text = stack.stop();
    check(
        &o,
        &[
            "login", "servers", "channels", "lobby", "create", "room", "match", "back", "result",
            "room",
        ],
        &dir,
        &text,
    );
}

#[test]
fn menu_joins_a_listed_room_gets_ready_plays_and_returns_to_the_waiting_room() {
    // a hard limit: a run takes about 30 s alone; a lost peer must fail it, never hang it
    let watchdog = Watchdog::arm("menu-guest", Duration::from_secs(300));
    // LG-225 / CLI-1470: `--tls loopback` refuses plain logins, so the menu and the walker log in over TLS
    let stack = Stack::start_full("menu-guest", &watchdog, &[], &["--tls", "loopback"]);
    let bot_room = RoomChoice::New {
        max_players: 8,
        title: "walker room".into(),
        mode: 0,
        start_after: 2,
    };
    let o = scenario(
        &stack,
        "MenuGuest",
        PilotRoom::Join(None),
        bot_room,
        "guest",
    );
    let dir = stack.dir.clone();
    let text = stack.stop();
    assert!(
        text.contains("TLS: players pin this server with: --server-fingerprint "),
        "the fingerprint line:\n{text}"
    );
    assert!(
        text.lines().filter(|l| l.contains("auth connection") && l.contains("(TLS)")).count() >= 2,
        "the menu and the walker log in over TLS:\n{text}"
    );
    assert!(!text.contains("refused: plain"), "{text}");
    check(
        &o,
        &[
            "login", "servers", "channels", "lobby", "room", "match", "back", "result", "room",
        ],
        &dir,
        &text,
    );
}

/// launcher-mapmode: the scripted host names its mode and map (`--mode FFA --pilot-map JunkFlea`): it picks FFA in
/// the creation box's mode list and the map in the waiting room's map list; the lobby offers FFA through the
/// `modes.playable` environment override. Needs the ROM (the lists open under the layout's controls).
#[test]
fn the_pilot_creates_a_room_with_the_named_mode_and_map() {
    let Some(rom) = std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
    else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let g = dc_bute::GameRoomData::from_rom(&rom).expect("room table");
    let names = dc_client::menu::pilot::RoomNames::from_rows(&g.maps(), &g.modes(false));
    let mode = names.mode("FFA").expect("an FFA row");
    let map = names.map("JunkFlea").expect("JunkFlea");
    let watchdog = Watchdog::arm("menu-mapmode", Duration::from_secs(120));
    let stack = Stack::start_with(
        "menu-mapmode",
        &watchdog,
        &[(
            "DC_MODES__PLAYABLE",
            r#"["TDM", "TDM_CLAN", "COOP", "FFA"]"#,
        )],
    );
    let nick = "MapModeHost";
    let mut link = LinkOptions::new(nick);
    link.log = Some(stack.dir.join("menu_events.jsonl"));
    let mut menu = Menu::new(MenuOptions {
        auth: stack.auth,
        user: nick.into(),
        link,
        rom: Some(rom),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    // nobody joins: the host never starts, the run ends at its timeout in the waiting room
    let mut pilot = Pilot::new(PilotPlan {
        password: PASSWORD.into(),
        room: PilotRoom::Create {
            title: "mapmode".into(),
            mode: Some(mode),
            map: Some(map),
            start_after: 9,
        },
        matches: 1,
        shots: None,
        timeout: Duration::from_secs(20),
    });
    let base = NetOptions::new(
        Connect::Lobby {
            auth: stack.auth,
            room: RoomChoice::Any,
            password: String::new(),
            channel: None,
        },
        nick,
    );
    let mut lines = Vec::new();
    let hopts = HeadlessOptions::default();
    let report = {
        let mut play = headless_runner(&base, nick.into(), &hopts, &mut lines);
        run_pilot(&mut menu, &mut pilot, &mut play)
    };
    let (got_mode, got_map) = (menu.data.room.mode, menu.data.room.map);
    let screen = menu.screen_name();
    drop(menu);
    let text = stack.stop();
    println!("trail: {} | end {}", report.trail.join(" -> "), report.end);
    assert_eq!(screen, "room", "in the waiting room; lobby log: {text}");
    assert!(pilot.warnings.is_empty(), "{:?}", pilot.warnings);
    assert_eq!((got_mode, got_map), (mode, map), "the room's mode and map");
}

/// CLI-902: the common bar's notice string control (1, 5) is hidden until a notice arrives (no placeholder text), which drew as a red "-" over
/// the Inbox button (cshell+0x19680 replaces it with the last notice).
#[test]
fn rom_common_bar_notice_slot_has_no_red_placeholder_over_the_inbox_button() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    use dc_client::menu::link::LinkOptions;
    use dc_client::menu::{Go, Menu, MenuOptions};
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    m.open(Go::Lobby);
    let t = m
        .sheet()
        .part(1, 5, dc_ui_data::WidgetKind::Text)
        .expect("notice string control");
    assert!(!t.shown, "the placeholder text is drawn: {:?}", t.text);
}
