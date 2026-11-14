//! The supervisor against the real `dc-dserver` binary over loopback UDP: a synthetic world file, the player joined
//! by the server's own synthetic client with the player's ticket, and a stand-in bot program (tests/support/fake_bot.rs)
//! that records its arguments and ticket and waits for its stdin to close. Checks the start order, the seats, the
//! logs and that every child is gone after the teardown.

use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig};
use dc_localhost::supervise::{Error, Event, LocalHost, Options, Programs};
use dc_localhost::{Bots, Hosting, LocalMatch, Mode, Practice, Seat, World};
use dc_net_engine::socket_reader::SocketReader;

/// Ceiling of every wait: a loaded test machine delays the server by seconds; the waits end as soon as they can.
const WAIT: Duration = Duration::from_secs(60);

fn exe_name(stem: &str) -> String {
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}

/// `dc-dserver`: `DC_DSERVER_EXE`, else the one in this profile's target folder, else built with the same cargo.
fn dserver_exe() -> PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| {
        if let Some(p) = std::env::var_os("DC_DSERVER_EXE").filter(|p| !p.is_empty()) {
            return PathBuf::from(p);
        }
        let me = std::env::current_exe().unwrap();
        let profile = me.parent().and_then(Path::parent).unwrap().to_path_buf();
        let exe = profile.join(exe_name("dc-dserver"));
        if !exe.is_file() {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let ok = Command::new(cargo)
                .args(["build", "-p", "dc-dserver", "--bin", "dc-dserver"])
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .status()
                .expect("run cargo build -p dc-dserver");
            assert!(ok.success(), "cargo build -p dc-dserver failed");
        }
        assert!(exe.is_file(), "no dc-dserver at {}", exe.display());
        exe
    })
    .clone()
}

/// The stand-in bot, compiled with the toolchain's rustc once per source hash.
fn fake_bot_exe() -> PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| {
        let src = include_str!("support/fake_bot.rs");
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in src.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake-bot");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join(exe_name(&format!("fake_bot_{h:016x}")));
        if out.is_file() {
            return out;
        }
        let rustc = std::env::var("CARGO")
            .ok()
            .map(|c| Path::new(&c).with_file_name(exe_name("rustc")))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("rustc"));
        let tmp = dir.join(format!("build-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let src_path = tmp.join("fake_bot.rs");
        std::fs::write(&src_path, src).unwrap();
        let tmp_out = tmp.join(exe_name("fake_bot"));
        let o = Command::new(&rustc)
            .args(["--edition", "2021", "-C", "debuginfo=0", "-o"])
            .arg(&tmp_out)
            .arg(&src_path)
            .output()
            .expect("run rustc");
        assert!(
            o.status.success(),
            "rustc fake_bot.rs failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        if std::fs::rename(&tmp_out, &out).is_err() && !out.is_file() {
            return tmp_out;
        }
        out
    })
    .clone()
}

fn temp_dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("localhost-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn spec(dir: &Path, bots: u8) -> LocalMatch {
    let world = dir.join("synth.dat");
    std::fs::write(&world, dc_dserver::world::synthetic_world_bytes()).unwrap();
    LocalMatch {
        world: World::File {
            path: world,
            announce: Some("Worlds\\Synth.dat".into()),
        },
        rom: None,
        content: None,
        mode: Mode {
            key: "TDM".into(),
            kind: 0,
            goal: 50,
            time_secs: 600,
            friendly_fire: false,
            teams: true,
        },
        player: Seat {
            name: "Tester".into(),
            loadout: Vec::new(),
        },
        bots: Bots {
            count: bots,
            skill: 40,
            name_prefix: "Bot".into(),
        },
        practice: Practice::default(),
        server_loadout: Vec::new(),
        tick_hz: None,
        hosting: Hosting::Solo,
        idle_secs: Some(60),
        team_empty_secs: None,
        log: None,
        verbose: false,
    }
}

fn start(dir: &Path, bots: u8) -> LocalHost {
    let programs = Programs {
        server: dserver_exe(),
        bot: fake_bot_exe(),
    };
    let options = Options {
        log_dir: Some(dir.join("logs")),
        bot_args: vec!["--hunt".into()],
        env: vec![("DC_FAKE_BOT_DIR".into(), dir.display().to_string())],
        ..Options::default()
    };
    LocalHost::start(&spec(dir, bots), &programs, options).unwrap_or_else(|e| panic!("start: {e}"))
}

/// The player's client: the server's synthetic client over a loopback socket, with the player's ticket.
fn join_player(host: &mut LocalHost) -> SynthClient {
    join_player_at(host.port(), host.player_ticket().0)
}

fn join_player_at(port: u16, ticket: dc_wire::seatticket::Ticket) -> SynthClient {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let rd = SocketReader::start(&sock, 8192).unwrap();
    let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let clock = Instant::now();
    let mut cfg = SynthConfig::named("Tester");
    cfg.seat_ticket = Some(ticket);
    let mut c = SynthClient::new(cfg);
    let end = Instant::now() + WAIT;
    while Instant::now() < end && c.state() != ClientState::InWorld {
        for d in c.poll(clock.elapsed().as_millis() as u32) {
            sock.send_to(&d, server).unwrap();
        }
        while let Some(Ok((d, from))) = rd.next_within(Duration::from_millis(1)) {
            if from == server {
                c.handle_datagram(clock.elapsed().as_millis() as u32, &d);
            }
        }
    }
    assert_eq!(c.state(), ClientState::InWorld);
    c
}

fn wait_file(p: &Path, has: &str) -> String {
    let end = Instant::now() + WAIT;
    loop {
        let t = std::fs::read_to_string(p).unwrap_or_default();
        if t.contains(has) || Instant::now() > end {
            return t;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_solo_match_seats_the_player_starts_the_bots_and_tears_everything_down() {
    let dir = temp_dir("solo");
    let mut host = start(&dir, 2);
    assert_ne!(host.port(), 0, "-port 0 gave a free port");
    let _player = join_player(&mut host);
    let (joined, seen) = host.wait_for(
        WAIT,
        |e| matches!(e, Event::Joined { uid: 1, nick, .. } if nick == "Tester"),
    );
    assert!(joined, "{seen:?}");

    host.start_bots().unwrap();
    assert!(host.grouped(), "every child in the kill-on-close group");
    assert!(
        !host.poll().iter().any(|e| matches!(e, Event::Warning(_))),
        "no warning when every child is grouped"
    );
    for (i, name) in ["Bot1", "Bot2"].iter().enumerate() {
        let rec = wait_file(&dir.join(format!("{name}.txt")), "ticket ");
        let args = rec.lines().next().unwrap_or_default();
        assert_eq!(args, format!("args {}", host.bot_args(i).join(" ")));
        assert!(
            args.contains(&format!(
                "--udp 127.0.0.1:{} --name {name} --skill 40 --parent-pipe --hunt",
                host.port()
            )),
            "{args}"
        );
        let ticket = rec
            .lines()
            .nth(1)
            .unwrap_or_default()
            .trim_start_matches("ticket ")
            .to_string();
        assert_eq!(ticket.len(), 32, "the bot's ticket is in its environment");
        assert!(!args.contains(&ticket), "never on the command line");
    }

    host.shutdown();
    assert!(!host.running(), "the server stopped");
    for name in ["Bot1", "Bot2"] {
        let rec = wait_file(&dir.join(format!("{name}.txt")), "eof");
        assert!(rec.contains("eof"), "{name} saw its stdin close: {rec}");
    }
    let log = std::fs::read_to_string(dir.join("logs").join("dc-dserver.log")).unwrap();
    assert!(log.contains("@ready port="), "{log}");
    assert!(log.contains("@join 0 1 1 0 Tester"), "{log}");
    let player = dc_wire::seatticket::to_hex(&host.player_ticket().0);
    assert!(!log.contains(&player), "no ticket in the log");
    assert!(dir.join("logs").join("dc-botclient-1.log").is_file());
}

#[test]
fn a_server_that_ends_stops_the_bots() {
    let dir = temp_dir("ends");
    let mut host = start(&dir, 1);
    host.start_bots().unwrap();
    wait_file(&dir.join("Bot1.txt"), "ticket ");
    host.send_line("quit").unwrap();
    let (ended, seen) = host.wait_for(WAIT, |e| matches!(e, Event::ServerExited(_)));
    assert!(ended, "{seen:?}");
    let rec = wait_file(&dir.join("Bot1.txt"), "eof");
    assert!(rec.contains("eof"), "{rec}");
    assert!(!host.running());
}

#[test]
fn a_server_that_never_gets_ready_fails_the_start_and_is_stopped() {
    let dir = temp_dir("never");
    // the stand-in bot never prints `@ready`: the start fails after the timeout and the child is gone
    let programs = Programs {
        server: fake_bot_exe(),
        bot: fake_bot_exe(),
    };
    let options = Options {
        ready_timeout: Duration::from_millis(800),
        env: vec![
            ("DC_FAKE_BOT_DIR".into(), dir.display().to_string()),
            ("DC_FAKE_SERVER".into(), "silent".into()),
        ],
        ..Options::default()
    };
    match LocalHost::start(&spec(&dir, 0), &programs, options) {
        Err(Error::NotReady(_)) => {}
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("started"),
    }
    // the stand-in records its stdin closing when the failed start stopped it
    let rec = wait_file(&dir.join("server.txt"), "end");
    assert!(rec.contains("end"), "{rec}");
    let missing = Programs {
        server: dir.join("no-such-server"),
        bot: fake_bot_exe(),
    };
    assert!(matches!(
        LocalHost::start(&spec(&dir, 0), &missing, Options::default()),
        Err(Error::Start(..))
    ));
}

#[test]
fn in_the_background_the_bots_start_after_the_players_join_and_stop_with_the_handle() {
    let dir = temp_dir("background");
    let host = start(&dir, 1);
    let port = host.port();
    let ticket = host.player_ticket();
    let bg = host.run_in_background(Some(1)).expect("background thread");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!dir.join("Bot1.txt").exists(), "no bot before the player");
    let _player = join_player_at(port, ticket.0);
    assert!(bg.wait_for(WAIT, |e| matches!(e, Event::Joined { uid: 1, .. })));
    let rec = wait_file(&dir.join("Bot1.txt"), "ticket ");
    assert!(rec.contains("--name Bot1"), "{rec}");
    bg.stop();
    let rec = wait_file(&dir.join("Bot1.txt"), "eof");
    assert!(rec.contains("eof"), "{rec}");
}

/// A supervised match whose server is the stand-in in `mode` (DC_FAKE_SERVER).
fn fake_server(dir: &Path, mode: &str, extra: &[(&str, &str)]) -> Result<LocalHost, Error> {
    let programs = Programs {
        server: fake_bot_exe(),
        bot: fake_bot_exe(),
    };
    let mut env = vec![
        ("DC_FAKE_BOT_DIR".to_string(), dir.display().to_string()),
        ("DC_FAKE_SERVER".to_string(), mode.to_string()),
    ];
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let options = Options {
        ready_timeout: Duration::from_secs(20),
        log_dir: Some(dir.join("logs")),
        env,
        ..Options::default()
    };
    LocalHost::start(&spec(dir, 3), &programs, options)
}

/// A line that is not UTF-8 before `@ready`: the start still sees `@ready` (it used to stop reading the server and
/// then wait for it forever).
#[test]
fn a_bad_line_before_ready_does_not_stop_the_start() {
    let dir = temp_dir("bad-before");
    let t = Instant::now();
    let host = fake_server(&dir, "bad-before", &[]).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(host.port(), 40000);
    assert!(t.elapsed() < Duration::from_secs(15));
}

/// A line that is not UTF-8 after `@ready`: later events still arrive, the bad line as text with replacements.
#[test]
fn events_after_a_bad_line_still_arrive() {
    let dir = temp_dir("bad-after");
    let mut host = fake_server(&dir, "bad-after", &[]).unwrap_or_else(|e| panic!("{e}"));
    let (joined, seen) = host.wait_for(WAIT, |e| matches!(e, Event::Joined { uid: 7, .. }));
    assert!(joined, "{seen:?}");
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::Server(l) if l.contains('\u{fffd}'))),
        "{seen:?}"
    );
}

/// A server that floods its stdout (4 MB) is drained: the event after the flood arrives and nothing blocks.
#[test]
fn a_flooding_server_is_drained() {
    let dir = temp_dir("flood");
    let mut host = fake_server(&dir, "flood", &[]).unwrap_or_else(|e| panic!("{e}"));
    let (joined, _) = host.wait_for(WAIT, |e| matches!(e, Event::Joined { uid: 8, .. }));
    assert!(joined, "the line after the flood");
}

/// LOC-11: a hosted match's join secret reaches the server in its environment, never on its command line.
#[test]
fn the_join_secret_is_not_on_the_servers_command_line() {
    let dir = temp_dir("secret-env");
    let programs = Programs {
        server: fake_bot_exe(),
        bot: fake_bot_exe(),
    };
    let mut m = spec(&dir, 0);
    m.hosting = Hosting::Host {
        port: 29977,
        max_players: 4,
    };
    let options = Options {
        env: vec![("DC_FAKE_BOT_DIR".into(), dir.display().to_string())],
        ..Options::default()
    };
    let host = LocalHost::start(&m, &programs, options).unwrap_or_else(|e| panic!("{e}"));
    let hex = dc_wire::seatticket::to_hex(&host.join_secret().expect("hosted").0);
    let rec = wait_file(&dir.join("server.txt"), "secret ");
    let args = rec.lines().next().unwrap_or_default();
    assert!(args.starts_with("args -port 29977"), "{rec}");
    assert!(!args.contains(&hex), "no secret in the argv: {args}");
    assert!(!args.contains("joinsecret"), "{args}");
    assert!(
        rec.contains(&format!("secret {hex}")),
        "in the environment: {rec}"
    );
}

/// Bots that ignore the end of their stdin are stopped together: the teardown takes one grace (2 s), not one per bot.
#[test]
fn stubborn_bots_are_stopped_within_one_grace() {
    let dir = temp_dir("stubborn");
    let mut host = fake_server(&dir, "ready", &[("DC_FAKE_BOT", "stubborn")])
        .unwrap_or_else(|e| panic!("{e}"));
    host.start_bots().unwrap();
    for n in 1..=3 {
        wait_file(&dir.join(format!("Bot{n}.txt")), "ticket ");
    }
    let t = Instant::now();
    host.shutdown();
    let took = t.elapsed();
    assert!(
        took < dc_localhost::supervise::BOT_QUIT_GRACE + Duration::from_secs(2),
        "{took:?}"
    );
    assert!(!host.running());
}

/// `dc-botclient` in this profile's target folder, else built with the same cargo.
fn botclient_exe() -> PathBuf {
    let exe = dserver_exe().with_file_name(exe_name("dc-botclient"));
    if !exe.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let ok = Command::new(cargo)
            .args(["build", "-p", "dc-botclient", "--bin", "dc-botclient"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .expect("run cargo build -p dc-botclient");
        assert!(ok.success(), "cargo build -p dc-botclient failed");
    }
    exe
}

/// The real bot client is seated by the ticket the supervisor gives it in DC_SEAT_TICKET (the server prints its
/// `@join` at the hello, before any world load); a bot without its ticket would be refused.
#[test]
fn the_real_bot_client_is_seated_by_its_ticket() {
    let dir = temp_dir("realbot");
    let programs = Programs {
        server: dserver_exe(),
        bot: botclient_exe(),
    };
    let options = Options {
        log_dir: Some(dir.join("logs")),
        ..Options::default()
    };
    let mut host = LocalHost::start(&spec(&dir, 1), &programs, options)
        .unwrap_or_else(|e| panic!("start: {e}"));
    host.start_bots().unwrap();
    let (joined, seen) = host.wait_for(
        WAIT,
        |e| matches!(e, Event::Joined { uid: 2, nick, .. } if nick == "Bot1"),
    );
    let bot_log =
        std::fs::read_to_string(dir.join("logs").join("dc-botclient-1.log")).unwrap_or_default();
    assert!(joined, "{seen:?}\nbot log: {bot_log}");
    host.shutdown();
    assert!(!host.running());
}

/// LOC-11: a bot never gets the match's join secret, also when the caller's environment holds one.
#[test]
fn bots_never_get_the_join_secret() {
    let dir = temp_dir("bot-secret");
    let leaked = dc_wire::seatticket::to_hex(&[0x42; 16]);
    let mut host = fake_server(&dir, "ready", &[("DC_JOIN_SECRET", leaked.as_str())])
        .unwrap_or_else(|e| panic!("{e}"));
    host.start_bots().unwrap();
    let rec = wait_file(&dir.join("Bot1.txt"), "joinsecret");
    assert!(rec.lines().any(|l| l == "joinsecret "), "{rec}");
    assert!(!rec.contains(&leaked), "{rec}");
}

/// A line longer than LINE_MAX is cut there; the output after it still arrives.
#[test]
fn an_overlong_line_is_cut_and_the_next_one_arrives() {
    let dir = temp_dir("long");
    let mut host = fake_server(&dir, "long", &[]).unwrap_or_else(|e| panic!("{e}"));
    let (joined, seen) = host.wait_for(WAIT, |e| matches!(e, Event::Joined { uid: 9, .. }));
    assert!(joined, "the line after the long one");
    let longest = seen
        .iter()
        .filter_map(|e| match e {
            Event::Server(l) => Some(l.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    assert_eq!(longest, dc_localhost::supervise::LINE_MAX);
}
