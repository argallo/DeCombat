//! The client through the Rust lobby: `dc-lobby-server` on ephemeral loopback ports with a temporary DB supervises
//! `dc-dserver` (the made-up world forced through `--dserver-world` / `--dserver-args -worldfile`); the client logs
//! in (`LOCAL:<id>`, nickname creation), creates a room, waits for a dc-botclient walker to join and get ready,
//! starts, receives 0x3043, joins the game server and plays until the server ends the match.
//! Every process is stopped by its own PID when the test ends.
// SPEC: PROTOCOL_2009.md §2-§3, §7 (0x3043); DSERVER_PIPE.md §2-§5
// HARNESS: lobby_harness::login, ::lobby, ::test_main_suite (the lobby flow and the 0x3043 port at +0x28)

mod support;

use std::net::SocketAddr;
use std::process::Command;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::{run_bot, BotConfig, RoomChoice};
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ViewAngles,
    WalkTest, WorldFile,
};
use support::*;

#[test]
fn client_logs_in_hosts_a_room_and_plays_through_the_rust_lobby() {
    // a hard limit: the run takes about 35 s alone; a lost peer must fail it, never hang it (one run hung 1252 s)
    let watchdog = Watchdog::arm("net_lobby", Duration::from_secs(300));
    let lobby = lobby_exe();
    let dserver = dserver_exe();
    let dir = scratch("lobby");
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
        "-worldfile {} -announce Worlds\\Synth.dat -tick 20 -roundsecs 14 -holds 1500,800",
        world.display()
    );
    let mut cmd = Command::new(&lobby);
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
    let mut lob = Proc::spawn(cmd);
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
    let auth: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    // the client hosts: room created, waits for the walker, starts
    let mut o = NetOptions::new(
        Connect::Lobby {
            auth,
            room: RoomChoice::New {
                max_players: 8,
                title: "net test".into(),
                mode: 0,
                start_after: 2,
            },
            password: String::new(),
            channel: None,
        },
        "NetHost",
    );
    o.world_file = Some({
        let s = synth_source();
        WorldFile {
            bytes: s.bytes,
            stand_dims: SYNTH_DIMS,
            textures: s.textures,
        }
    });
    o.match_wait = Duration::from_secs(30);
    o.log = Some(dir.join("client_events.jsonl"));
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    // the walker joins the first listed room and gets ready (no ROM world check: the world is a made-up file)
    let bot = std::thread::spawn(move || {
        let mut cfg = BotConfig::new(auth, "LobbyWalker");
        cfg.room = RoomChoice::Any;
        cfg.motion = MotionKind::Circle;
        cfg.duration = Duration::from_secs(30);
        cfg.use_rom = false;
        cfg.load_ms = 300;
        cfg.match_wait = Duration::from_secs(30);
        run_bot(&cfg, &Recorder::null("LobbyWalker"))
    });
    let opts = HeadlessOptions {
        frames: 60 * 24,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 12,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    let mut walk = WalkTest::new(ViewAngles::default());
    let r = run_headless(&mut net, Some(&mut walk as &mut dyn InputSource), &opts);
    let st = handle.shutdown(Duration::from_secs(5));
    let bot = bot.join().expect("bot thread");
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let pids = logged_pids(&text);
    lob.drain();
    if let Some(p) = lob.pid() {
        kill_pid(p);
    }
    let _ = lob.finish(Duration::from_secs(2));
    for p in &pids {
        kill_pid(*p);
    }
    let r = r.unwrap_or_else(|e| {
        panic!(
            "headless run: {e}; phase {:?}; lobby log:\n{text}",
            st.phase
        )
    });
    println!("{}", r.summary());
    for l in &r.last_lines {
        println!("  | {l}");
    }
    println!(
        "net: phase {:?} stage {} end {:?} updates {} teleports {} life {:?}",
        st.phase,
        st.stage,
        st.end,
        st.updates_sent,
        st.teleports.len(),
        st.life_states
    );
    for (obj, t) in &st.tracks {
        println!(
            "  remote {} pid {} team {} object {obj}: {} samples travel {:.1}",
            t.name, t.pid, t.team, t.sample_count, t.travel
        );
    }
    match &bot {
        Ok(v) => println!("  bot: stage {} end {}", v["stage"], v["end"]),
        Err(e) => println!("  bot error: {e}"),
    }
    let launch: Vec<&str> = text
        .lines()
        .filter(|l| l.contains(" server pid "))
        .collect();
    println!("  lobby launch lines {launch:?}");

    assert_eq!(
        launch.len(),
        1,
        "the lobby started one game server; log:\n{text}"
    );
    assert!(bot.is_ok(), "the walker played: {bot:?}");
    for m in [
        "joined",
        "S6a_world_loaded_sent",
        "S12_player_state",
        "world_ready_sent",
        "spawned",
    ] {
        assert!(st.reached(m), "milestone {m}: {:?}", st.milestones);
    }
    assert!(st.life_states.contains(&1));
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert!(st.updates_sent >= 50, "updates {}", st.updates_sent);
    let t = st
        .tracks
        .values()
        .find(|t| t.name == "LobbyWalker")
        .unwrap_or_else(|| panic!("no remote track for the walker: {:?}", st.tracks));
    assert!(
        t.travel > 50.0,
        "the walker moved on the client's screen: {}",
        t.travel
    );
    assert!(r.corrections >= 1, "the spawn teleport was applied");
    assert_eq!(r.nan_ticks + r.nan_frames + r.nan_remotes, 0);
    // DC-DEFAULT: DSV-35 (supersedes DSV-13), see DC_DEFAULTS.md: the round end ends with 0x3f, never 0x99
    assert!(
        st.hud.result_screen.is_some() && st.hud.level_exit.is_none(),
        "round end shown: {:?}",
        st.hud
    );
}
