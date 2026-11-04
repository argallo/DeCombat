//! The w2-audiocues sound cues in a scripted headless match against our own dedicated server over loopback UDP: the
//! made-up world, the client running away from the shared spawn point for 2.5 s and then standing, its sound log on
//! (`--sound-log`), a shooter bot of the other team that fires the rifle at it with a loose aim (so bullets pass it)
//! and throws a grenade every 3 s. Checked in the client's
//! sound log: the bot's bullets that pass the client play the fly-by at the nearest point of their line (m42), a
//! team mate bot's throws play the team callout and the enemy shooter's throws play none (m39: the relay names the
//! thrower's team, DSV-260, and the client plays it for its own team only, cshell+0x2cbd30), the own steps of the view-bob cycle stay silent while standing (m38), and the JSONL log holds the same starts with
//! hashes only. ROM-gated (`CA_ROM`: the callout table, the fly-by record and the ammunition rows are ROM data).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (type 0x19 k 0x12, type 0x06); AUDIO_2009.md §9; DSERVER_PIPE.md §2-§4
// EVIDENCE: cshell+0x2c73a0 (fly-by), cshell+0x2cbd30 (k 0x12 callout)

mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_botclient::shooter::ShooterConfig;
use dc_botclient::skill::Skill;
use dc_client::sound::Trigger;
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ViewAngles,
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

/// Waits `wait_s` once alive, runs forward for `run_s` (away from the shared spawn point, so the bot's bullets end at
/// the far wall, well behind the client), then stands still. The run starts at once: the server holds the body from
/// about 2 s on (the round-start hold of `-holds`), and the run lies inside the spawn note's window.
struct RunThenStand {
    t: f64,
    alive_at: Option<f64>,
    wait_s: f64,
    run_s: f64,
    ran: u32,
}

impl RunThenStand {
    fn running(&self) -> bool {
        self.alive_at.is_some_and(|a| {
            let d = self.t - a;
            d >= self.wait_s && d < self.wait_s + self.run_s
        })
    }
}

impl InputSource for RunThenStand {
    fn frame(&mut self, _view: &mut ViewAngles) {}
    fn server(&mut self, v: &dc_client::ServerHold) {
        if v.life_state == Some(1) && self.alive_at.is_none() {
            self.alive_at = Some(self.t);
        }
    }
    fn tick(&mut self, tick: u64, hz: u32, _view: &mut ViewAngles) -> Commands {
        self.t = tick as f64 / hz.max(1) as f64;
        if self.running() {
            self.ran += 1;
        }
        Commands {
            forward: self.running(),
            run: self.running(),
            ..Commands::default()
        }
    }
    fn label(&self) -> &'static str {
        "run then stand"
    }
}

/// A standing bot named `name` that loads the world for `load_ms`, fires every `shot_period_ms` at the nearest enemy
/// and throws every 3 s.
fn shooter(
    name: &'static str,
    shot_period_ms: u32,
    load_ms: u32,
    port: u16,
    world: PathBuf,
    secs: u64,
) -> std::thread::JoinHandle<BotSession> {
    std::thread::spawn(move || {
        let mut cfg = GameConfig::new(name);
        cfg.hello_tag = name.to_lowercase();
        cfg.motion = MotionKind::Stand;
        cfg.world_file = Some(world);
        cfg.load_ms = load_ms;
        cfg.shooter = Some(ShooterConfig {
            shot_period_ms,
            // the loosest aim (4 degrees per axis): many bullets pass the client instead of hitting it
            skill: Some(Skill::new(0)),
            throw_every_ms: Some(3000),
            ..ShooterConfig::default()
        });
        let mut bot = BotSession::new(cfg, None, Recorder::null(name));
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        run_game(
            &mut bot,
            server,
            Duration::from_secs(secs),
            &Recorder::null(name),
        )
        .expect("bot udp");
        bot
    })
}

#[test]
fn flyby_and_team_callout_in_a_scripted_match() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: needs CA_ROM (callout table, fly-by record, ammunition rows)");
        return;
    };
    let _wd = Watchdog::arm("audio_cues", Duration::from_secs(300));
    let dir = scratch("audio_cues");
    let world = synth_world_file(&dir);
    let (mut srv, port) = start_dserver(
        &world,
        &[
            "-tick",
            "20",
            "-roundsecs",
            "60",
            "-holds",
            "1500,800",
            "-seed",
            "23",
            "-loadout",
            "145,164,83,7",
            "-combat",
            "start_health=100000,max_health=100000",
            "-nomovecheck",
            "-verbose",
        ],
    );
    // DSV-260 (amends CV2-3, 2026-10-28): the throw relay names the thrower's team, and the client plays the callout
    // for its own team only; so the team mate's throws are heard and the enemy shooter's are not. The mate fires
    // (almost) never: its shots are not part of the check. It loads 8 s, so it spawns after the shooter's bullets
    // passed the running client (the shooter aims at the nearest enemy: the mate at the spawn point once it is in)
    // and throws at about 13 s and 16 s of the client's 20 s run.
    srv.line("user 1 1 1 0 0 CueClient");
    srv.line("user 2 2 2 1 0 Shooter");
    srv.line("user 3 3 3 0 0 Mate");
    let bot = shooter("Shooter", 200, 2000, port, world.clone(), 26);
    let mate = shooter("Mate", 3_600_000, 8000, port, world.clone(), 26);
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "CueClient",
    );
    o.world_file = Some(world_file());
    o.rom = Some(rom.clone());
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let log = dir.join("sounds.jsonl");
    let mut opts = HeadlessOptions {
        frames: 60 * 20,
        frame_us: 16_667,
        width: 160,
        height: 120,
        out: None,
        save_every: 1_000_000,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    opts.sound.log = Some(log.clone());
    let mut input = RunThenStand {
        t: 0.0,
        alive_at: None,
        wait_s: 0.0,
        run_s: 2.5,
        ran: 0,
    };
    let r = run_headless(&mut net, Some(&mut input as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    let _ = bot.join();
    let _ = mate.join();
    srv.quit();
    srv.finish(Duration::from_secs(5));
    srv.drain();
    println!(
        "input: alive at {:?} s, last tick time {:.1} s, running ticks {}",
        input.alive_at, input.t, input.ran
    );
    println!("{}", r.summary());
    println!("  {}", r.sound_note.as_deref().unwrap_or(""));
    for l in srv.seen.iter().filter(|l| l.contains("throw")).take(6) {
        println!("    {l}");
    }
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    let flybys: Vec<_> = r
        .sound_log
        .iter()
        .filter(|e| matches!(e.trigger, Trigger::Flyby { .. }))
        .collect();
    let callouts: Vec<_> = r
        .sound_log
        .iter()
        .filter(|e| matches!(e.trigger, Trigger::Callout { .. }))
        .collect();
    let shots = r.sound_stats.shot_fires;
    println!(
        "  bot shots heard {shots}, flybys {}, callouts {} (first at {:?} ms)",
        flybys.len(),
        callouts.len(),
        callouts.first().map(|e| e.t_ms)
    );
    for e in flybys.iter().take(4) {
        println!(
            "  flyby t {} dist {:?} gain {:?} outer {} file {}",
            e.t_ms,
            e.distance.map(|d| d.round()),
            e.gain,
            e.outer,
            e.file_hash.as_deref().unwrap_or("-")
        );
    }
    assert!(
        shots >= 10,
        "the bot fired at the client: {:?}",
        r.sound_stats
    );
    // m42: bullets passed the client: the fly-by plays at the nearest point of the line, within the fly-by radius,
    // from a file of the record, positional at the record's radii, priority 0
    assert!(!flybys.is_empty(), "a bullet passed the client");
    for e in &flybys {
        assert!(
            e.distance
                .is_some_and(|d| d < dc_client::sound::cues::FLYBY_RADIUS),
            "{e:?}"
        );
        assert!(e.file_hash.is_some() && e.gain.is_some(), "{e:?}");
        assert_eq!((e.prio, e.class), (0, 0));
        assert!(e.outer > 0.0 && e.inner <= e.outer);
    }
    // m39 + DSV-260: the relay's player is the thrower's dserver client number; the mate's throws play one callout
    // each, positional at the relay position, priority 2, from the row's files; the enemy shooter's throws none
    let pid_of = |name: &str| -> u32 {
        let tag = format!(": hello name \"{name}\"");
        srv.seen
            .iter()
            .find_map(|l| {
                let i = l.find(&tag)?;
                l[..i].rsplit("client ").next()?.trim().parse().ok()
            })
            .unwrap_or_else(|| panic!("{name} joined"))
    };
    let (shooter_pid, mate_pid) = (pid_of("Shooter"), pid_of("Mate"));
    // the server numbers the throws of the match ("throw N")
    let threw = |pid: u32| {
        let tag = format!("client {pid}: throw ");
        srv.seen
            .iter()
            .any(|l| l.contains(&tag) && l.contains("(weapon 7 "))
    };
    assert!(threw(shooter_pid), "the shooter threw");
    assert!(threw(mate_pid), "the mate threw");
    assert!(!callouts.is_empty(), "the team mate's throw relay's callout");
    for e in &callouts {
        assert!(
            matches!(e.trigger, Trigger::Callout { player, .. } if player == mate_pid),
            "only the own team's thrower is heard (DSV-260): {e:?}"
        );
        assert!(e.file_hash.is_some() && e.gain.is_some(), "{e:?}");
        assert_eq!((e.prio, e.class), (2, 0));
        assert!(e.outer > 0.0);
    }
    // m38: the own steps of the view-bob cycle while running, also inside the spawn window (CLI-680)
    let steps: Vec<u32> = r
        .sound_log
        .iter()
        .filter(|e| matches!(e.trigger, Trigger::OwnStep { .. }))
        .map(|e| e.t_ms)
        .collect();
    println!(
        "  own steps {} spawn notes {:?}",
        steps.len(),
        r.sound_spawn_log
    );
    // CLI-680: the spawn window (0x1f .. 0x25) does not mute the steps: the run starts inside it and steps
    let mut open: std::collections::BTreeMap<u16, u32> = std::collections::BTreeMap::new();
    let mut windows = 0;
    let mut inside = 0;
    for &(t, obj, on) in &r.sound_spawn_log {
        if on {
            open.insert(obj, t);
        } else if let Some(a) = open.remove(&obj) {
            windows += 1;
            inside += steps.iter().filter(|s| **s >= a && **s < t).count();
        }
    }
    assert!(
        windows >= 2,
        "both characters' spawn windows: {:?}",
        r.sound_spawn_log
    );
    assert!(
        inside > 0,
        "own steps inside the spawn window ({:?})",
        r.sound_stats
    );
    // the JSONL log carries the same starts by label, hashes only
    let text = std::fs::read_to_string(&log).expect("sound log");
    let count = |label: &str| {
        // start lines only (the `live` lines of AUDIO-3 list the playing voices with their labels too)
        text.lines()
            .filter(|l| !l.contains(r#""trigger":"live""#) && !l.contains(r#""trigger":"music""#))
            .filter(|l| l.contains(&format!(r#""trigger":"{label}""#)))
            .count()
    };
    println!(
        "  jsonl: {} lines, flyby {} throw_callout {}",
        text.lines().count(),
        count("flyby"),
        count("throw_callout")
    );
    assert_eq!(count("flyby"), flybys.len());
    assert_eq!(count("throw_callout"), callouts.len());
    for e in flybys.iter().chain(callouts.iter()) {
        let name = e.file.as_deref().unwrap();
        assert!(!text.contains(name), "no ROM name in the log");
    }
}
