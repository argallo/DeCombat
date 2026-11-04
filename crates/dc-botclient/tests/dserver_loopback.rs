//! Loopback test against our own dedicated server binary: `dc-dserver` is spawned on an ephemeral UDP port with a
//! world file, the roster goes in through its stdin (`user <uid> <sid> <key> <team> <exp> <nick>`, DSERVER_PIPE.md
//! §3.1), and the bot joins over real UDP. Slice 1 of the server takes a client to S11; the in-world assertions
//! (S12 player state, 0x87, updates under the 2009 gate) run only when the server sends shell 0x67, and the test
//! prints the stage it reached.
//!
//! The server binary is found next to this test's target directory (built by `cargo test --workspace` or `cargo
//! build -p dc-dserver`), or through `DC_DSERVER_EXE`; without it the test is skipped with a message.
// SPEC: DSERVER_PIPE.md §2 (launch line), §3.1 (user line), §4 (events: @ready, @join, @leave, @player, @end)

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::{run_game, session_report};
use dc_botclient::session::{BotSession, GameConfig, Stage};

fn dserver_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DC_DSERVER_EXE") {
        return Some(PathBuf::from(p)).filter(|p| p.exists());
    }
    // <target>/<profile>/deps/<this test>.exe -> <target>/<profile>/dc-dserver(.exe)
    let me = std::env::current_exe().ok()?;
    let dir = me.parent()?.parent()?;
    let p = dir.join(format!("dc-dserver{}", std::env::consts::EXE_SUFFIX));
    p.exists().then_some(p)
}

macro_rules! dserver_or_skip {
    () => {
        match dserver_exe() {
            Some(p) => p,
            None => {
                println!("skipped: dc-dserver binary not built (cargo build -p dc-dserver, or set DC_DSERVER_EXE)");
                return;
            }
        }
    };
}

/// The spawned server; killed when dropped.
struct Server {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Server {
    fn spawn(exe: &PathBuf, args: &[String]) -> Server {
        let mut child = Command::new(exe)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn dc-dserver");
        let out = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines() {
                match l {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });
        Server {
            stdin: child.stdin.take(),
            child: Some(child),
            lines: rx,
            seen: Vec::new(),
        }
    }

    fn line(&mut self, l: &str) {
        let s = self.stdin.as_mut().expect("stdin");
        writeln!(s, "{l}").unwrap();
        s.flush().unwrap();
    }

    fn drain(&mut self) {
        while let Ok(l) = self.lines.try_recv() {
            self.seen.push(l);
        }
    }

    fn wait(&mut self, prefix: &str, t: Duration) -> Option<String> {
        let end = Instant::now() + t;
        loop {
            if let Some(l) = self.seen.iter().find(|l| l.starts_with(prefix)) {
                return Some(l.clone());
            }
            let left = end.checked_duration_since(Instant::now())?;
            match self.lines.recv_timeout(left.min(Duration::from_millis(50))) {
                Ok(l) => self.seen.push(l),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return self.seen.iter().find(|l| l.starts_with(prefix)).cloned()
                }
            }
        }
    }

    fn has(&self, part: &str) -> bool {
        self.seen.iter().any(|l| l.contains(part))
    }

    fn exit(mut self) -> Option<i32> {
        self.stdin.take();
        let mut c = self.child.take()?;
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if let Ok(Some(s)) = c.try_wait() {
                return s.code();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = c.kill();
        let _ = c.wait();
        None
    }
}

/// A made-up world file (no ROM content): a box from (-512, -64, -512) to (512, 256, 512) and one model.
fn synthetic_world() -> PathBuf {
    use dc_world::synth::{SynthModel, SynthWorld};
    let dir = std::env::temp_dir().join(format!("dc-botclient-loopback-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("synth.dat");
    let bytes = SynthWorld {
        info: b"botclient synthetic".to_vec(),
        extent_min: [-512.0, -64.0, -512.0],
        extent_max: [512.0, 256.0, 512.0],
        tree_shape: vec![false],
        models: vec![SynthModel {
            name: b"main".to_vec(),
            root: -1,
            ..SynthModel::default()
        }],
        ..SynthWorld::default()
    }
    .build();
    std::fs::write(&p, bytes).unwrap();
    p
}

fn ready_port(s: &mut Server) -> u16 {
    let l = s
        .wait("@ready", Duration::from_secs(20))
        .unwrap_or_else(|| panic!("no @ready; server said {:?}", s.seen));
    l.split_whitespace()
        .find_map(|t| t.strip_prefix("port="))
        .and_then(|p| p.parse().ok())
        .expect("port in @ready")
}

/// Runs one bot named `nick` against the server for `secs` seconds; returns the session.
fn play(
    port: u16,
    nick: &str,
    force: bool,
    motion: MotionKind,
    secs: u64,
    world: Option<PathBuf>,
) -> BotSession {
    let mut cfg = GameConfig::new(nick);
    cfg.world_file = world;
    cfg.hello_tag = nick.to_ascii_lowercase();
    cfg.force_updates = force;
    cfg.motion = motion;
    cfg.load_ms = 400;
    let rec = Recorder::null(nick);
    let mut bot = BotSession::new(cfg, None, rec);
    let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    run_game(
        &mut bot,
        server,
        Duration::from_secs(secs),
        &Recorder::null(nick),
    )
    .expect("udp run");
    bot
}

fn stage_reached(b: &BotSession) -> &'static str {
    let o = &b.obs;
    if o.reached("world_ready_sent") {
        "S12+ (in world, 0x87 sent)"
    } else if o.reached("S12_player_state") {
        "S12 (player state received)"
    } else if o.reached("S11_switch_state") {
        "S11"
    } else if o.reached("S10_view_object") {
        "S10"
    } else if o.reached("S6a_world_loaded_sent") {
        "S6a"
    } else if o.reached("joined") {
        "joined"
    } else {
        "not joined"
    }
}

#[test]
fn bot_reaches_s11_on_our_dserver_and_its_updates_are_parsed() {
    let exe = dserver_or_skip!();
    let world = synthetic_world();
    let args: Vec<String> = ["-port", "0", "-worldfile"]
        .iter()
        .map(|s| s.to_string())
        .chain([world.to_string_lossy().into_owned()])
        .chain(
            [
                "-announce",
                "Worlds\\Synth.dat",
                "-guid",
                "ca",
                "-idle",
                "30",
            ]
            .iter()
            .map(|s| s.to_string()),
        )
        .collect();
    let mut srv = Server::spawn(&exe, &args);
    let port = ready_port(&mut srv);
    srv.line("user 7 1 1 0 0 LoopBot");
    // force updates so a server without shell 0x67 still receives 0x64
    let bot = play(
        port,
        "LoopBot",
        true,
        MotionKind::Circle,
        6,
        Some(world.clone()),
    );
    srv.drain();
    println!("stage reached against dc-dserver: {}", stage_reached(&bot));
    println!(
        "{}",
        serde_json::to_string(&session_report(&bot)["milestones"]).unwrap()
    );
    let o = &bot.obs;
    for m in [
        "joined",
        "S1_hello_sent",
        "S2_version",
        "S3_client_id",
        "S4_file_list",
        "S4a_file_ack_sent",
        "S6_load_world",
        "S6a_world_loaded_sent",
        "S7_preload_end",
        "S8a_preload_done_sent",
        "S9_own_object",
        "S10_view_object",
        "S11_switch_state",
        "rate_sent",
        "S13_first_time_entry",
    ] {
        assert!(o.reached(m), "milestone {m} missing: {:?}", o.milestones);
    }
    assert!(o.fatal.is_empty(), "{:?}", o.fatal);
    assert_eq!(o.version.map(|v| v.0), Some(7));
    assert_eq!(
        o.files.get(&o.load_world.unwrap().1).map(|f| f.1.as_str()),
        Some("Worlds\\Synth.dat")
    );
    assert!(o.updates_sent > 30, "updates sent {}", o.updates_sent);
    assert!(
        srv.wait("@join", Duration::from_secs(2)).is_some(),
        "{:?}",
        srv.seen
    );
    assert!(
        srv.has("first player update"),
        "the server parsed our 0x64: {:?}",
        srv.seen
    );
    assert!(
        srv.wait("@leave", Duration::from_secs(5)).is_some(),
        "{:?}",
        srv.seen
    );
    assert_eq!(bot.stage(), Stage::Closed, "{:?}", bot.obs.end);
    let s = bot.link_stats().unwrap();
    assert_eq!(s.bad_check, 0);
    // loopback loses nothing: at most 3 resends, plus at most 2 for every tick the server itself missed (a gap of
    // its own clock in the time stream, a loaded machine); an unloaded machine has no such gap
    let stalls = bot.table.server_stalls;
    assert!(
        s.resends <= 3 + 2 * stalls,
        "loopback resends {} with {stalls} server stalls",
        s.resends
    );
    assert!(
        bot.table.time_entries > 20,
        "time stream {}",
        bot.table.time_entries
    );
    assert_eq!(bot.table.stale_time_entries, 0);
    if o.reached("S12_player_state") {
        assert!(o.reached("world_ready_sent"));
    }
    srv.line("quit");
    assert!(
        srv.wait("@end", Duration::from_secs(5)).is_some(),
        "{:?}",
        srv.seen
    );
    assert_eq!(srv.exit(), Some(0));
}

#[test]
fn two_bots_under_the_2009_gate_see_each_other_when_the_server_plays() {
    let exe = dserver_or_skip!();
    let world = synthetic_world();
    let args: Vec<String> = vec![
        "-port".into(),
        "0".into(),
        "-worldfile".into(),
        world.to_string_lossy().into_owned(),
        "-announce".into(),
        "Worlds\\Synth.dat".into(),
        "-idle".into(),
        "30".into(),
    ];
    let mut srv = Server::spawn(&exe, &args);
    let port = ready_port(&mut srv);
    srv.line("user 1 1 1 0 0 BotA");
    srv.line("user 2 2 2 1 0 BotB");
    let (wa, wb) = (world.clone(), world.clone());
    let a = std::thread::spawn(move || play(port, "BotA", false, MotionKind::Circle, 6, Some(wa)));
    let b = std::thread::spawn(move || play(port, "BotB", false, MotionKind::Stand, 6, Some(wb)));
    let (a, b) = (a.join().unwrap(), b.join().unwrap());
    srv.drain();
    println!(
        "stage reached: BotA {}, BotB {}",
        stage_reached(&a),
        stage_reached(&b)
    );
    for bot in [&a, &b] {
        assert!(
            bot.obs.reached("S11_switch_state"),
            "{:?}",
            bot.obs.milestones
        );
        assert!(bot.obs.fatal.is_empty(), "{:?}", bot.obs.fatal);
        if bot.obs.reached("S12_player_state") {
            // a server that plays: 0x87, updates under the 2009 gate
            assert!(bot.obs.reached("world_ready_sent"));
            if bot.life_state() == 1 {
                assert!(bot.obs.updates_sent > 0);
            }
        } else {
            // slice 1: the client stays on its loading screen and sends no update
            assert!(!bot.obs.reached("world_ready_sent"));
            assert_eq!(bot.obs.updates_sent, 0);
        }
    }
    // with another player's object on the stream, the walking bot moves on the other bot's screen
    for (n, x) in [("A", &a), ("B", &b)] {
        println!(
            "Bot{n}: view {:?} objects {:?} removals {} roster {:?} teleports {:?} life {:?}",
            x.obs.view_object,
            x.table.objects.keys().collect::<Vec<_>>(),
            x.table.removals,
            x.obs
                .roster
                .values()
                .map(|p| (p.pid, p.character, p.team))
                .collect::<Vec<_>>(),
            x.obs.teleports,
            x.obs.life_states
        );
    }
    if a.obs.reached("other_player_object") && b.obs.reached("other_player_object") {
        let a_obj = a.obs.view_object.unwrap();
        let b_obj = b.obs.view_object.unwrap();
        // the first bot to quit is removed from the other's world: look at its last state too
        let a_seen = b.table.seen(a_obj).cloned().unwrap_or_default();
        let b_seen = a.table.seen(b_obj).cloned().unwrap_or_default();
        println!(
            "BotB saw BotA's object {a_obj}: {} stream entries, travel {:.1}, last {:?}; BotA saw BotB travel {:.1}",
            a_seen.stream_entries, a_seen.travel, a_seen.pos, b_seen.travel
        );
        // BotA walks a circle, BotB stands: what each sees of the other follows that
        if a.obs.updates_sent > 20 {
            assert!(
                a_seen.travel > 20.0,
                "BotA's walk is visible to BotB: {a_seen:?}"
            );
        }
        assert!(b_seen.travel < 5.0, "BotB stands: {b_seen:?}");
    }
    srv.line("quit");
    let _ = srv.wait("@end", Duration::from_secs(5));
    let _ = srv.exit();
}

#[test]
fn rom_world_name_resolves_and_the_box_decodes() {
    let Some(rom) = dc_rez::dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let exe = dserver_or_skip!();
    let arch =
        dc_rez::RezArchive::open(&rom.join("Game").join("WORLDS.rez")).expect("world archive");
    let Some(listed) = arch
        .entries()
        .iter()
        .map(|e| e.0.clone())
        .find(|n| n.to_ascii_uppercase().ends_with(".DAT"))
    else {
        panic!("no world in the archive");
    };
    let args: Vec<String> = vec![
        "-port".into(),
        "0".into(),
        "-world".into(),
        listed.clone(),
        "-idle".into(),
        "30".into(),
    ];
    let mut srv = Server::spawn(&exe, &args);
    let port = ready_port(&mut srv);
    srv.line("user 3 3 3 0 0 RomBot");
    let mut cfg = GameConfig::new("RomBot");
    cfg.force_updates = true;
    cfg.load_ms = 200;
    let rom_worlds = dc_botclient::romworld::RomWorlds::open(&rom).ok();
    let mut bot = BotSession::new(cfg, rom_worlds, Recorder::null("RomBot"));
    let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    run_game(
        &mut bot,
        server,
        Duration::from_secs(4),
        &Recorder::null("RomBot"),
    )
    .unwrap();
    srv.drain();
    let o = &bot.obs;
    println!("stage reached on a ROM world: {}", stage_reached(&bot));
    assert!(
        matches!(o.world_resolved, Some(Ok(_))),
        "announced {:?} -> {:?}",
        o.announced_world,
        o.world_resolved
    );
    assert!(o.reached("S11_switch_state"), "{:?}", o.milestones);
    assert!(bot.table.extents.is_some());
    let body = bot.body().expect("body spawned");
    println!(
        "ROM world resolved {}: box {:?}, own object {:?} at {:?}, body {:?} (ROM collision {})",
        matches!(o.world_resolved, Some(Ok(_))),
        bot.table.extents,
        o.view_object,
        bot.own_object()
            .and_then(|x| bot.table.objects.get(&x))
            .map(|x| x.pos),
        body.pos(),
        body.rom_world
    );
    assert!(body.rom_world, "ROM collision used");
    assert!(body.pos().iter().all(|c| c.is_finite()));
    srv.line("quit");
    let _ = srv.wait("@end", Duration::from_secs(5));
    let _ = srv.exit();
}
