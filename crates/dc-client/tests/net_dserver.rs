//! The client plays a match of our own dedicated server over loopback UDP: `dc-dserver` on an ephemeral port with
//! the made-up world of `dc_client::testkit`, the roster through its stdin (DSERVER_PIPE.md §3.1), a dc-botclient
//! walker as the second player, and the client headless (frames paced to the wall clock, rendered offscreen).
//! Checked: S1-S12 and the in-world state 1, at least 50 player updates, the walker's object moving on the
//! client's screen (stream samples interpolated and drawn as a box), a 0xa0 correction provoked mid-game and
//! applied (new `move_seq` echoed, updates go on), no NaN in the simulation, the frames or the remote boxes, the
//! HUD state from 0x66 / 0x67, and the round end 0x3f (no 0x99, no server bye: every client leaves on its own and
//! the server ends when the last one has left, DSV-35). The walker leaves first, so its track is read from the
//! session's record of departed players.
// SPEC: DSERVER_PIPE.md §2-§4; UDP_SESSION_2009_A.md §9.2; GAME_MESSAGES_2009_C2S.md §2.1; MOVEMENT_2009.md §12.4;
// GAME_MESSAGES_2009_S2C.md §2.1, §4.1, §4.3, §6.1

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ViewAngles,
    WalkTest, WorldFile,
};
use support::*;

/// Updates sent before the client shifts one report (the server answers with a teleport back).
const GLITCH_AFTER: u64 = 60;

fn world_file() -> WorldFile {
    let s = synth_source();
    WorldFile {
        bytes: s.bytes,
        stand_dims: SYNTH_DIMS,
        textures: s.textures,
    }
}

fn walker(port: u16, world: std::path::PathBuf, secs: u64) -> std::thread::JoinHandle<BotSession> {
    std::thread::spawn(move || {
        let mut cfg = GameConfig::new("WalkBot");
        cfg.hello_tag = "walkbot".into();
        cfg.motion = MotionKind::Circle;
        cfg.world_file = Some(world);
        cfg.load_ms = 300;
        let mut bot = BotSession::new(cfg, None, Recorder::null("WalkBot"));
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        run_game(
            &mut bot,
            server,
            Duration::from_secs(secs),
            &Recorder::null("WalkBot"),
        )
        .expect("bot udp");
        bot
    })
}

#[test]
fn client_joins_dserver_plays_and_sees_the_walker() {
    let dir = scratch("dserver");
    let world = synth_world_file(&dir);
    // a short round (16 s) with short holds so the whole match fits in the test; 20 Hz stream
    let (mut srv, port) = start_dserver(
        &world,
        &[
            "-tick",
            "20",
            "-roundsecs",
            "16",
            "-holds",
            "1500,800",
            // the round ends by its time limit: that hold has its own option (DSV-35 amendment 2, default 3 s)
            "-timeout-hold",
            "1500",
            "-seed",
            "7",
        ],
    );
    srv.line("user 1 1 1 0 0 NetClient");
    srv.line("user 2 2 2 1 0 WalkBot");
    let bot = walker(port, world.clone(), 30);

    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "NetClient",
    );
    o.world_file = Some(world_file());
    o.glitch_after_updates = Some(GLITCH_AFTER);
    o.log = Some(dir.join("client_events.jsonl"));
    // a short look at the result screen (the 2009-like 8 s would outlast the frames of this test)
    o.result_hold = Duration::from_millis(2000);
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 22,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 6,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    let mut walk = WalkTest::new(ViewAngles::default());
    let r = run_headless(&mut net, Some(&mut walk as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    let bot = bot.join().expect("bot thread");
    srv.drain();
    let code = srv.finish(Duration::from_secs(5));
    srv.drain();
    println!("{}", r.summary());
    for l in &r.last_lines {
        println!("  | {l}");
    }
    println!(
        "net: phase {:?} stage {} end {:?}\n  milestones {:?}\n  updates {} teleports {:?} taken {} life {:?} last seq {:?}",
        st.phase, st.stage, st.end, st.milestones.iter().map(|m| &m.0).collect::<Vec<_>>(), st.updates_sent,
        st.teleports, st.corrections_taken, st.life_states, st.last_seq_reported
    );
    for (obj, t) in &st.tracks {
        println!(
            "  remote {} pid {} team {} object {obj}: {} samples travel {:.1} gap {:.1} ms first {:?} last {:?}",
            t.name, t.pid, t.team, t.sample_count, t.travel, t.gap_ms, t.first, t.last
        );
    }
    println!("  hud {:?}", st.hud);
    println!(
        "  bot: stage {:?} updates {} end {:?}",
        bot.stage(),
        bot.obs.updates_sent,
        bot.obs.end
    );
    println!("  server exit {code:?}; lines:");
    for l in srv.seen.iter().filter(|l| !l.contains("ping")) {
        println!("    {l}");
    }

    // start-up S1-S12 and the in-world state
    for m in [
        "joined",
        "S2_version",
        "S4_file_list",
        "S6_load_world",
        "S6a_world_loaded_sent",
        "S10_view_object",
        "S11_switch_state",
        "S12_player_state",
        "world_ready_sent",
        "spawned",
        "first_update_sent",
    ] {
        assert!(st.reached(m), "milestone {m}: {:?}", st.milestones);
    }
    assert!(
        st.life_states.contains(&1),
        "player state 1 reached: {:?}",
        st.life_states
    );
    // w2-loading: the loading page's stage reached "in world" and the match ended normally
    // (the result screen was left: no disconnect box)
    assert_eq!(handle.load_stage(), dc_client::loading::stage::IN_WORLD);
    assert_eq!(handle.session_end(), dc_client::loading::SessionEnd::Played);
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert_eq!(st.bad_check, 0);
    // updates
    assert!(st.updates_sent >= 50, "updates sent {}", st.updates_sent);
    assert!(srv.has("first player update"), "the server parsed our 0x64");
    // the walker on the client's screen
    let t = st
        .tracks
        .values()
        .find(|t| t.name == "WalkBot")
        .unwrap_or_else(|| panic!("no remote track for the walker: {:?}", st.tracks));
    assert_eq!(t.team, 2, "team from 0x6b");
    assert!(t.sample_count > 40, "stream samples {}", t.sample_count);
    assert!(
        t.travel > 50.0,
        "the walker moved on the client's screen: travel {}",
        t.travel
    );
    assert!(
        r.remote_frames > 100,
        "frames with a remote player {}",
        r.remote_frames
    );
    // a correction mid-game: spawn teleport + the teleport back after the shifted report
    assert!(st.glitch_sent, "the shifted report went out");
    assert!(st.teleports.len() >= 2, "teleports {:?}", st.teleports);
    assert!(
        srv.has("update rejected") && srv.has("teleport back"),
        "the server corrected the client"
    );
    assert!(
        r.corrections >= 2 && st.corrections_taken >= 2,
        "corrections applied {}",
        r.corrections
    );
    let last_seq = st.teleports.last().map(|t| t.0);
    assert_eq!(
        st.last_seq_reported, last_seq,
        "the client echoes the newest move_seq"
    );
    assert!(
        st.updates_sent > GLITCH_AFTER + 30,
        "updates went on after the correction"
    );
    // simulation and frames
    assert_eq!(r.nan_ticks, 0);
    assert_eq!(r.nan_frames, 0);
    assert_eq!(r.nan_remotes, 0);
    assert!(r.end.is_finite());
    // HUD from 0x66 / 0x67 and the round end
    assert_eq!(st.hud.max_health, Some(100.0));
    assert_eq!(st.hud.health, Some(100.0));
    assert!(st.hud.result_screen.is_some(), "0x3f shown");
    // DC-DEFAULT: DSV-35 (supersedes DSV-13), see DC_DEFAULTS.md: never 0x99, no server bye after 0x3f
    assert_eq!(st.hud.level_exit, None, "no 0x99 after the result screen");
    assert!(st.reached("result_screen") && !st.reached("level_exit"));
    assert!(
        r.session_over_frame.is_some(),
        "the session ended after the result screen"
    );
    if r.adapter.is_some() {
        // the walker is drawn as its model when the ROM resolves the character, else as a box (CLI-39)
        assert!(
            r.boxes_drawn + r.characters_drawn > 10,
            "remote drawn: boxes {} models {}",
            r.boxes_drawn,
            r.characters_drawn
        );
    } else {
        println!("no adapter: rendering skipped ({:?})", r.render_skipped);
    }
    assert_eq!(
        code,
        Some(0),
        "the server ended by itself once every client had left"
    );
}

/// A ROM world: dc-dserver loads the first world of the user's archive (`-world`), announces its listed path, and
/// the client resolves that name in its own ROM, loads and renders it, spawns where the server teleports it and
/// walks on the ROM collision. ROM-gated (skipped without `CA_ROM`); nothing of the ROM is asserted by value.
// SPEC: UDP_SESSION_2009_A_addendum_disasm.md A9 (name form); MOVEMENT_2009.md §12.4
#[test]
fn client_resolves_and_renders_a_rom_world_announced_by_dserver() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let maps = dc_client::RomWorld::map_names(&rom).expect("ROM world archive");
    let Some(map) = maps.first().cloned() else {
        panic!("no world in the ROM archive");
    };
    let mut cmd = std::process::Command::new(dserver_exe());
    cmd.args([
        "-port",
        "0",
        "-world",
        &format!("Worlds/{map}"),
        "-idle",
        "30",
        "-tick",
        "20",
    ]);
    let mut srv = Proc::spawn(cmd);
    let ready = srv
        .wait_line("@ready", Duration::from_secs(60))
        .unwrap_or_else(|| panic!("no @ready; server said {:?}", srv.seen));
    let port: u16 = ready
        .split_whitespace()
        .find_map(|t| t.strip_prefix("port="))
        .and_then(|p| p.parse().ok())
        .expect("port");
    srv.line("user 5 5 5 0 0 RomClient");
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "RomClient",
    );
    o.rom = Some(rom.clone());
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 8,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 30,
        realtime: true,
        ..HeadlessOptions::default()
    };
    let mut walk = WalkTest::new(ViewAngles::default());
    let r = run_headless(&mut net, Some(&mut walk as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    srv.quit();
    let code = srv.finish(Duration::from_secs(5));
    srv.drain();
    println!("{}", r.summary());
    println!(
        "net: world {:?} phase {:?} updates {} teleports {} life {:?} fatal {:?}; server exit {code:?}",
        st.world,
        st.phase,
        st.updates_sent,
        st.teleports.len(),
        st.life_states,
        st.fatal
    );
    assert_eq!(
        st.world.as_deref(),
        Some(map.as_str()),
        "the announced listed path resolved to the map"
    );
    assert!(
        st.reached("world_ready_sent") && st.life_states.contains(&1),
        "{:?}",
        st.milestones
    );
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert!(st.updates_sent >= 50, "updates {}", st.updates_sent);
    assert!(r.corrections >= 1, "spawn teleport applied");
    assert_eq!(r.nan_ticks + r.nan_frames, 0);
    assert!(!r.fell_out, "stayed in the ROM world");
    // the body stands where the server put it, give or take the walk
    let tp = st.teleports.first().map(|t| t.1).unwrap();
    let d = ((r.end.x - tp[0]).powi(2) + (r.end.z - tp[2]).powi(2)).sqrt();
    assert!(d < 3000.0, "end {:?} vs spawn teleport {tp:?}", r.end);
}
