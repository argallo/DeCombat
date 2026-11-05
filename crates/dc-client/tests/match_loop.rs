//! The match loop end to end against our own servers over loopback UDP: `dc-dserver` (the workspace binary) with
//! the made-up world of `dc_client::testkit`, a dc-botclient shooter on the other team, and the client headless
//! (frames paced to the wall clock, rendered offscreen, the HUD on: the ROM layout when `CA_ROM` is set, else ours).
//! The client stands still; the shooter hits it (0x74 + 0x66: health falls, the aim cross flashes; no damage sector
//! lights with Reality off), kills it (0x6d naming the shooter, 0x67 state 3 / 2: the death camera turns to the killer, the
//! respawn countdown runs), the client asks for its respawn itself once, more than 7000 ms after the death (0x89),
//! the server places it again (the spawn set: 0x67 state 1, teleport, full health: the own view returns), the
//! round ends by its time limit, the result screen opens (0x3f) and the client leaves it the 2009 way (no
//! `C2S_BYE`: the plain transport bye; the server sees the peer's bye and ends once every client has left).
//! The HUD state is asserted at each step from the report's HUD marks; the PNGs of the death, respawn and
//! result states are written (local files under cargo's temporary directory, never committed).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (0x6d), §4.1 (0x67), §4.3 (0x66, 0x74), §2.1 (0x3f);
// GAME_MESSAGES_2009_C2S.md §3.3, §3.4 (own respawn wait and 0x89), Appendix A steps A8-A13; DSERVER_PIPE.md §3.1
// OBSERVED: capture 20261025_065016_tdm6 (the real client's 0x89 7.015-7.031 s after 0x67 [2])

mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_botclient::shooter::ShooterConfig;
use dc_client::deathcam::DeathCamMode;
use dc_client::hud::HudChoice;
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, HudMark, InputSource, NetOptions, NetWorld, ViewAngles,
    WorldFile,
};
use dc_sim::Commands;
use support::*;

fn world_file() -> WorldFile {
    let s = synth_source();
    WorldFile {
        bytes: s.bytes,
        stand_dims: SYNTH_DIMS,
        textures: s.textures,
    }
}

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
        // the shooter comes in late, once the client's frame loop runs (its first HUD frame loads the textures)
        cfg.load_ms = 7000;
        // one kill: the shooter takes the target up again only after a minute
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

fn first<'a>(marks: &'a [HudMark], what: &str, f: impl Fn(&HudMark) -> bool) -> &'a HudMark {
    marks
        .iter()
        .find(|m| f(m))
        .unwrap_or_else(|| panic!("no HUD mark for {what}: {marks:#?}"))
}

fn log_events(text: &str, kind: &str) -> Vec<serde_json::Value> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["ev"] == kind)
        .collect()
}

#[test]
fn client_takes_damage_dies_respawns_and_leaves_the_result_screen() {
    let dir = scratch("matchloop");
    let world = synth_world_file(&dir);
    // a 22 s round ended by its time limit (scoreboard set at once, 0x3f 800 ms later), 20 Hz stream, a weapon
    // table index without the ROM (the server's default clip and damage, 34 per hit: three hits kill)
    let (mut srv, port) = start_dserver(
        &world,
        &[
            "-tick",
            "20",
            "-roundsecs",
            "22",
            "-holds",
            "1500,800",
            "-timeout-hold",
            "0",
            "-seed",
            "13",
            "-loadout",
            "3,4",
            "-damage",
            "34",
            "-nomovecheck",
        ],
    );
    // a TDM room (mode class 1): the client arms its own respawn wait and asks; the server keeps no timer (DSV-32)
    srv.line("room key=TDM type=1 goal=50 ff=0 time=22");
    srv.line("user 1 1 1 0 0 MatchClient");
    srv.line("user 2 2 2 1 0 Shooter");
    let bot = shooter(port, world.clone(), 38);

    let log = dir.join("client_events.jsonl");
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "MatchClient",
    );
    o.world_file = Some(world_file());
    o.log = Some(log.clone());
    o.capture = Some(dir.join("client_capture.jsonl"));
    o.result_hold = Duration::from_millis(1500);
    // the HUD checks below run without the Reality jar and arcs (always on in play since CLI-1847)
    o.reality = false;
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let rom = dc_rom::resolve_rom().ok();
    let shots = dir.join("shots");
    std::fs::create_dir_all(&shots).unwrap();
    let opts = HeadlessOptions {
        frames: 60 * 38,
        frame_us: 16_667,
        width: 640,
        height: 480,
        out: Some(shots.clone()),
        prefix: "matchloop_".into(),
        // only the state pictures (and the last frame)
        save_every: u32::MAX,
        // the ROM's HUD fonts when there is a ROM (the made-up font has two glyphs only)
        font_bytes: rom.is_none().then(synth_font),
        font_rom: rom.clone(),
        realtime: true,
        stop_after_over: Some(60),
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
    let code = srv.finish(Duration::from_secs(8));
    srv.drain();
    let text = std::fs::read_to_string(&log).unwrap_or_default();

    println!("{}", r.summary());
    println!("  {}", r.stall_note());
    println!(
        "hud {:?}; death camera frames {}; state images {:?}",
        r.hud, r.death_cam_frames, r.state_images
    );
    for m in &r.hud_marks {
        println!("  mark {m:?}");
    }
    println!(
        "net: phase {:?} end {:?} life {:?} damage {} kills {:?} killer {:?} respawn requests {} leave {:?} result {:?} -> {:?} mover notes {}",
        st.phase, st.end, st.life_states, st.damage_taken, st.kill_pairs, st.last_killer, st.respawn_requests,
        st.leave_kind, st.result_at, st.result_left_at, st.mover_notes
    );
    println!(
        "  bot: stage {:?} kills {:?} end {:?}",
        bot.stage(),
        bot.obs.kills,
        bot.obs.end
    );
    println!("  server exit {code:?}; lines:");
    for l in srv.seen.iter().filter(|l| !l.contains("ping")) {
        println!("    {l}");
    }

    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert_eq!(st.bad_check, 0);
    // DSV-568 / CLI-1855: the spawn and the respawn both hold up the slot-0 weapon 3 of `-loadout 3,4`
    let held: Vec<Option<u16>> = r.view_weapon_marks.iter().map(|m| m.2).collect();
    let drawn: Vec<u16> = held.iter().flatten().copied().collect();
    assert!(
        drawn.len() >= 2 && drawn.iter().all(|w| *w == 3),
        "first-person weapons at the spawn and the respawn: {held:?}"
    );
    let me = st.client_id.expect("own client id") as u8;
    let killer = bot
        .obs
        .client_id
        .map(|c| c.0 as u8)
        .expect("the shooter's id");
    if r.adapter.is_none() {
        println!("no GPU adapter: the HUD checks are skipped");
        return;
    }
    let marks = &r.hud_marks;
    assert!(r.hud.is_some(), "a HUD ran");

    // 1. spawned: alive with full health
    let spawn = first(marks, "the spawn", |m| {
        m.life_state == 1 && m.health == Some(100.0)
    });
    // 2. damage: health below 100 while alive, the aim cross's hit flash; no damage sector lights, because the
    // arcs light only inside the Reality gate and this client runs with the default options (Reality off)
    let hit = first(marks, "the first damage", |m| {
        m.life_state == 1 && m.health.is_some_and(|h| h < 100.0)
    });
    assert!(
        marks.iter().all(|m| !m.damage_lit),
        "no damage arc outside the Reality gate"
    );
    assert!(hit.frame >= spawn.frame);
    assert!(
        marks.iter().any(|m| m.flash && m.life_state == 1),
        "the hit flash ran"
    );
    assert!(st.damage_taken >= 2, "0x74 received: {}", st.damage_taken);
    // 3. death: the kill names the shooter, the death camera looks at him, the respawn countdown runs
    assert!(
        st.kill_pairs.contains(&(me, killer)),
        "0x6d (victim {me}, killer {killer}): {:?}",
        st.kill_pairs
    );
    assert_eq!(st.last_killer, Some(killer));
    let dead = first(marks, "the death", |m| matches!(m.life_state, 2 | 3));
    assert!(dead.frame > hit.frame);
    assert!(
        marks
            .iter()
            .any(|m| m.killer == Some(Some("Shooter".into()))),
        "the HUD keeps the killer's name"
    );
    assert!(
        marks
            .iter()
            .any(|m| m.death_cam == Some(DeathCamMode::Killer(killer))),
        "the killer camera ran"
    );
    let counting: Vec<u32> = marks.iter().filter_map(|m| m.respawn_secs).collect();
    assert!(
        counting.contains(&1),
        "respawn countdown 7..1: {counting:?}"
    );
    if !counting.contains(&7) {
        // under load: the wait's first second (7) can only be missing when the frame loop did not run during it,
        // i.e. a logged stall ends right at the countdown's first frame; any other gap is a failure
        let first_count = marks
            .iter()
            .find(|m| m.respawn_secs.is_some())
            .expect("a countdown mark");
        let held = r.stalls.iter().find(|s| s.frame == first_count.frame);
        assert!(
            held.is_some(),
            "respawn countdown 7..1: {counting:?}; no frame stall before the countdown's first frame {} ({})",
            first_count.frame,
            r.stall_note()
        );
        println!(
            "UNDER LOAD: the countdown's 7 was skipped by a frame stall {held:?}; countdown {counting:?}"
        );
    }
    // the countdown never runs backwards
    assert!(
        counting.windows(2).all(|w| w[1] <= w[0]),
        "the countdown runs down: {counting:?}"
    );
    assert!(
        r.death_cam_frames > 60 * 5,
        "death camera frames {}",
        r.death_cam_frames
    );
    // the death panel (client-deathhud): not during the killer camera (3500 ms after 0x67 [2]), then drawn while the
    // own state is 2, with the last 0x74 the client took alive (dc-dserver sends `dmg_kind` 3)
    // (marks carry frame times; the respawn wait of 7000 ms counts from the same death time as the dying time of
    // 3500 ms, so the panel may show only with at most 4 whole seconds of the wait left)
    let panel: Vec<&HudMark> = marks.iter().filter(|m| m.death_panel).collect();
    assert!(!panel.is_empty(), "the death panel was drawn");
    for m in &panel {
        assert_eq!(m.life_state, 2);
        assert!(
            m.respawn_secs.is_none_or(|s| s <= 4),
            "panel too early: {m:?}"
        );
        assert!(
            m.wound.is_some_and(|(d, k)| d > 0 && k == 3),
            "{:?}",
            m.wound
        );
    }
    assert!(
        marks
            .iter()
            .any(|m| m.life_state == 2 && !m.death_panel && m.death_cam.is_some()),
        "the killer camera ran without the panel"
    );
    assert!(
        r.state_images.iter().any(|(w, _)| w == "death_panel"),
        "the death panel picture: {:?}",
        r.state_images
    );
    // 4. respawn: one request, after the 2009 wait, then the spawn set: alive, full health, own view, no killer
    assert_eq!(st.respawn_requests, 1, "one 0x89 per death");
    let req = log_events(&text, "respawn_requested");
    assert_eq!(req.len(), 1, "{req:?}");
    assert_eq!(req[0]["shell"], "0x89");
    let waited = req[0]["waited_ms"].as_u64().unwrap_or(0);
    assert!(
        waited > 7000 && waited < 7300,
        "the request {waited} ms after the death"
    );
    let i = st
        .life_states
        .iter()
        .position(|s| *s == 2)
        .expect("state 2");
    assert!(
        st.life_states[i..].contains(&1),
        "state 1 after the death: {:?}",
        st.life_states
    );
    let back = first(marks, "the respawn", |m| {
        m.frame > dead.frame && m.life_state == 1 && m.killer.is_none() && m.death_cam.is_none()
    });
    assert!(
        marks
            .iter()
            .any(|m| m.frame >= back.frame && m.life_state == 1 && m.health == Some(100.0)),
        "full health after the respawn"
    );
    assert!(
        st.teleports.len() >= 2,
        "spawn and respawn teleports: {:?}",
        st.teleports
    );
    // 5. the result screen and the 2009 leave: plain bye, no C2S_BYE, the server ends by itself
    let res = first(marks, "the result screen", |m| m.result_screen);
    assert!(res.frame > back.frame);
    assert!(st.reached("result_screen"));
    assert_eq!(st.leave_kind.as_deref(), Some("result"));
    let (open, left) = (
        st.result_at.unwrap(),
        st.result_left_at.expect("left the result screen"),
    );
    assert!(
        left - open >= 1500,
        "stayed {} ms on the result screen",
        left - open
    );
    assert!(st.reached("leave_plain"), "{:?}", st.milestones);
    assert!(log_events(&text, "quit").is_empty(), "no C2S_BYE");
    assert!(r.session_over_frame.is_some(), "the session ended");
    assert!(
        srv.has(&format!("client {me} gone (PeerBye")),
        "the server saw the plain bye"
    );
    assert_eq!(code, Some(0), "the server ended once every client had left");
    assert_eq!(r.nan_ticks + r.nan_frames + r.nan_remotes, 0);
    // the pictures of the states (rendering needs an adapter)
    if r.adapter.is_some() {
        let names: Vec<&str> = r.state_images.iter().map(|(n, _)| n.as_str()).collect();
        // no "damage" picture: the headless runner takes it when a damage arc lights, and the arcs stay dark with
        // Reality off (the default here)
        for want in ["death", "respawn", "result"] {
            assert!(names.contains(&want), "state picture {want}: {names:?}");
        }
        for (_, p) in &r.state_images {
            assert!(p.is_file(), "{}", p.display());
        }
    } else {
        println!("no adapter: rendering skipped ({:?})", r.render_skipped);
    }
}
