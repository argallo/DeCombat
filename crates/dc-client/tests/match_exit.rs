//! The 2009 match end over loopback UDP: `dc-dserver` (kill goal 1, one-shot damage) with the made-up world, a
//! dc-botclient shooter on the other team, and the client headless (frames paced to the wall clock, the HUD on).
//! The shooter's kill ends the round, so the client dies at the round result 0x46 and arms its 7000 ms respawn
//! wait; the result screen message 0x3f comes 6 s later. Asserted on the session clock and the client's own
//! capture:
//! - the result board (scoreboard + lose header) is not drawn in the first 3000 ms after the result, then it is;
//! - after 0x3f the client sends no player update and no respawn request at all (the in-game frame ended), and
//!   none of the death's respawn requests ever goes out;
//! - the client leaves the result screen after its hold with the plain transport bye (no `C2S_BYE`);
//! - the match's result (rows, winner, the own row) is kept for the result screen.
//!
//! PNGs of the round-over and the result-board states are written when an adapter exists (local files under the
//! test's scratch folder or `DC_MATCHEXIT_SHOTS`, ROM-derived with `CA_ROM`, never committed).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x46, 0x3f); GAME_MESSAGES_2009_C2S.md §3.3, §3.4 (own respawn wait, 0x89)
// DISASM: cshell+0x4ac5c (0x3f ends the in-game frame), cshell+0xc02e0 (the board waits 3000 ms after the result)
// OBSERVED: capture 20261025_204341_realclient (no 0x64 / 0x89 after 0x3f)

mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use dc_botclient::capdecode::{decode_file, unbundle};
use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_botclient::shooter::ShooterConfig;
use dc_client::hud::HudChoice;
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ViewAngles,
    WorldFile,
};
use dc_sim::Commands;
use support::*;

/// A player who does nothing (the target).
struct Idle;

impl InputSource for Idle {
    fn frame(&mut self, _view: &mut ViewAngles) {}
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands::default()
    }
    fn label(&self) -> &'static str {
        "idle"
    }
}

fn shooter(port: u16, world: PathBuf, secs: u64) -> std::thread::JoinHandle<BotSession> {
    std::thread::spawn(move || {
        let mut cfg = GameConfig::new("Shooter");
        cfg.hello_tag = "shooter".into();
        cfg.motion = MotionKind::Stand;
        cfg.world_file = Some(world);
        // the shooter comes in once the client's frame loop runs
        cfg.load_ms = 5000;
        cfg.shooter = Some(ShooterConfig {
            shot_period_ms: 150,
            rearm_ms: 60_000,
            ..ShooterConfig::default()
        });
        let mut bot = BotSession::new(cfg, None, Recorder::null("Shooter"));
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        run_game(
            &mut bot,
            server,
            Duration::from_secs(secs),
            &Recorder::null("Shooter"),
        )
        .expect("bot udp");
        bot
    })
}

#[test]
fn the_match_ends_the_2009_way_without_a_respawn_request_after_the_result() {
    let dir = scratch("matchexit");
    let world = synth_world_file(&dir);
    // kill goal 1 and one-shot damage: the shooter's kill is the round result; the end set at the default pauses
    // (0x48 + the summary 3 s after 0x46, 0x3f 3 s later)
    let (mut srv, port) = start_dserver(
        &world,
        &[
            "-tick",
            "20",
            "-roundsecs",
            "60",
            "-seed",
            "13",
            "-loadout",
            "3,4",
            "-damage",
            "150",
            "-nomovecheck",
        ],
    );
    srv.line("room key=TDM type=1 goal=1 ff=0 time=60");
    srv.line("user 1 1 1 0 0 ExitClient");
    srv.line("user 2 2 2 1 0 Shooter");
    let bot = shooter(port, world.clone(), 45);

    let log = dir.join("client_events.jsonl");
    let capture = dir.join("client_capture.jsonl");
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "ExitClient",
    );
    o.world_file = Some({
        let s = synth_source();
        WorldFile {
            bytes: s.bytes,
            stand_dims: SYNTH_DIMS,
            textures: s.textures,
        }
    });
    o.log = Some(log.clone());
    o.capture = Some(capture.clone());
    o.result_hold = Duration::from_millis(1500);
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let rom = dc_rom::resolve_rom().ok();
    let shots = std::env::var_os("DC_MATCHEXIT_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("shots"));
    std::fs::create_dir_all(&shots).unwrap();
    let opts = HeadlessOptions {
        frames: 60 * 45,
        frame_us: 16_667,
        width: 640,
        height: 480,
        out: Some(shots.clone()),
        prefix: "matchexit_".into(),
        save_every: u32::MAX,
        font_bytes: rom.is_none().then(synth_font),
        font_rom: rom.clone(),
        realtime: true,
        stop_after_over: Some(30),
        hud: match &rom {
            Some(r) => HudChoice::Rom(r.clone()),
            None => HudChoice::Fallback,
        },
        debug_panel: false,
        state_shots: true,
        ..HeadlessOptions::default()
    };
    let mut idle = Idle;
    let r = run_headless(&mut net, Some(&mut idle as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    let bot = bot.join().expect("bot thread");
    srv.drain();
    let code = srv.finish(Duration::from_secs(10));
    srv.drain();
    let text = std::fs::read_to_string(&log).unwrap_or_default();

    println!("{}", r.summary());
    println!("  {}", r.stall_note());
    for m in &r.hud_marks {
        println!("  mark {m:?}");
    }
    println!(
        "net: end {:?} life {:?} kills {:?} respawn requests {} leave {:?} result {:?} -> {:?}; milestones {:?}",
        st.end,
        st.life_states,
        st.kill_pairs,
        st.respawn_requests,
        st.leave_kind,
        st.result_at,
        st.result_left_at,
        st.milestones
    );
    println!(
        "  bot: stage {:?} kills {:?} end {:?}",
        bot.stage(),
        bot.obs.kills,
        bot.obs.end
    );
    println!("  server exit {code:?}");
    for l in srv.seen.iter().filter(|l| !l.contains("ping")) {
        println!("    {l}");
    }
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    let me = st.client_id.expect("own client id") as u8;
    let killer = bot.obs.client_id.map(|c| c.0 as u8).expect("shooter id");

    // 1. the shooter's kill is the round result: the death and 0x46 in the same moment
    assert!(
        st.kill_pairs.contains(&(me, killer)),
        "the kill: {:?}",
        st.kill_pairs
    );
    let at = |m: &str| {
        st.milestones
            .iter()
            .find(|x| x.0 == m)
            .map(|x| x.1)
            .unwrap_or_else(|| panic!("milestone {m}: {:?}", st.milestones))
    };
    let (result_ms, screen_ms) = (at("round_result"), at("result_screen"));
    assert!(st.life_states.contains(&2), "{:?}", st.life_states);
    assert!(
        screen_ms - result_ms >= 5500,
        "0x3f {} ms after 0x46",
        screen_ms - result_ms
    );

    // 2. no respawn request for the death at the result: the 7000 ms wait ran past 0x3f, where the in-game frame
    //    ended
    assert_eq!(st.respawn_requests, 0, "no 0x89 after the result");
    let asked: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("\"respawn_requested\""))
        .collect();
    assert!(asked.is_empty(), "{asked:?}");

    // 3. the capture: nothing of the in-game frame after 0x3f
    let d = decode_file(&capture).expect("client capture");
    let t_screen = d
        .msgs
        .iter()
        .filter(|m| !m.c2s)
        .find(|m| {
            unbundle(&m.bits)
                .iter()
                .any(|b| b.as_bytes().first() == Some(&13) && b.as_bytes().get(1) == Some(&0x3f))
        })
        .map(|m| m.t)
        .expect("0x3f in the capture");
    let after: Vec<(f64, u8)> = d
        .msgs
        .iter()
        .filter(|m| m.c2s && m.t > t_screen)
        .filter_map(|m| m.shell().map(|s| (m.t - t_screen, s)))
        .collect();
    let updates_before = d
        .msgs
        .iter()
        .filter(|m| m.c2s && m.t <= t_screen && m.shell() == Some(0x64))
        .count();
    println!("c2s shell messages after 0x3f: {after:?}; updates before {updates_before}");
    assert!(
        after.is_empty(),
        "no game message after 0x3f (0x64 / 0x89 / 0x76 / 0xca): {after:?}"
    );
    assert!(updates_before > 0, "the client did send its updates before");
    assert!(
        !d.msgs
            .iter()
            .any(|m| m.c2s && matches!(m.shell(), Some(0x89 | 0xc5 | 0xd2))),
        "no respawn request in the whole capture"
    );

    // 4. the leave: after the hold, the plain transport bye, no C2S_BYE
    assert_eq!(st.leave_kind.as_deref(), Some("result"));
    let (open, left) = (
        st.result_at.expect("result screen"),
        st.result_left_at.expect("left the result screen"),
    );
    assert!(left - open >= 1500, "stayed {} ms", left - open);
    assert!(st.reached("leave_plain"), "{:?}", st.milestones);
    assert!(
        text.lines()
            .filter(|l| l.contains("\"ev\":\"quit\""))
            .count()
            == 0,
        "no C2S_BYE"
    );
    assert!(
        srv.has(&format!("client {me} gone (PeerBye")),
        "the server saw the plain bye"
    );

    // 5. the result the result screen shows
    let res = st.match_result.as_ref().expect("the match's result");
    println!("result: {res:?}");
    assert_eq!(res.own_pid, Some(me));
    assert_eq!(res.own_won(), Some(false), "the shooter's team won");
    let own = res.own_row().expect("own row");
    assert_eq!(own.deaths, 1);
    let theirs = res
        .rows
        .iter()
        .find(|r| r.pid == killer)
        .expect("shooter row");
    assert_eq!(theirs.kills, 1);
    assert_eq!(
        (theirs.team, res.score_of(theirs.team)),
        (2, 1),
        "bravo won 0:1"
    );

    // 6. the HUD: no board in the first 3000 ms after the result, then the board with the header
    if r.hud.is_some() {
        // the HUD marks carry the network side's clock (the one of `result_at`); the milestones the session's
        let result_hud = open - (screen_ms - result_ms);
        let board = r
            .hud_marks
            .iter()
            .find(|m| m.result_board)
            .expect("the result board was drawn");
        let dt = board.at_ms as i64 - result_hud as i64;
        println!(
            "result board {dt} ms after the result (longest frame {} ms)",
            r.longest_frame_ms
        );
        assert!(
            dt >= 3000 && dt <= 3200 + r.longest_frame_ms as i64,
            "the board {dt} ms after the result"
        );
        assert!(
            r.hud_marks
                .iter()
                .filter(|m| m.at_ms < result_hud + 3000)
                .all(|m| !m.result_board),
            "no board before 3000 ms"
        );
        assert!(
            r.hud_marks
                .iter()
                .filter(|m| m.result_screen)
                .all(|m| !m.result_board),
            "after 0x3f the result board of the HUD (CLI-80), not the 0x46 board"
        );
        if r.adapter.is_some() {
            println!("state images {:?}", r.state_images);
            assert!(r.state_images.iter().any(|(n, _)| n == "result"));
        }
    } else {
        println!("no HUD (no adapter): {:?}", r.render_skipped);
    }
    assert_eq!(r.nan_ticks + r.nan_frames + r.nan_remotes, 0);
    let _ = code;
}
