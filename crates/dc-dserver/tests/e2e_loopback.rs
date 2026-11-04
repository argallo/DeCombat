//! End-to-end over real loopback UDP: spawns the `dc-dserver` binary on an ephemeral port with a synthetic world
//! file, feeds it the lobby's control lines on stdin, drives the synthetic client from a UDP socket, and checks
//! the pipe events and the exit. The child is killed if the test fails at any point.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig};
use dc_dserver::world::synthetic_world_bytes;
use dc_net_engine::socket_reader::SocketReader;

/// Ceiling (seconds) of every wait for the server or the clients to reach a state. UNDER LOAD (a dozen parallel
/// cargo builds on the test machine) the server process, the clients' pump and the datagrams are delayed by seconds;
/// these waits end as soon as the state is reached, so a generous ceiling costs nothing when the machine is idle and
/// only turns a slow run into a pass instead of a false failure. What the waits check is unchanged.
const LOAD_CEILING_S: u64 = 60;

struct Proc {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Proc {
    fn spawn(world: &PathBuf, extra: &[&str]) -> Proc {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dc-dserver"));
        cmd.args(["-port", "0", "-worldfile"])
            .arg(world)
            .args([
                "-announce",
                "Worlds\\Synth.dat",
                "-guid",
                "ca",
                "-idle",
                "30",
            ])
            .args(extra)
            .env_remove("CA_ROM")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().expect("spawn dc-dserver");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });
        Proc {
            stdin: child.stdin.take(),
            child: Some(child),
            lines: rx,
            seen: Vec::new(),
        }
    }

    /// Sends one control line; a server that is gone fails the test with its exit status and output (no
    /// broken-pipe panic).
    fn write(&mut self, line: &str) {
        let sent = match self.stdin.as_mut() {
            Some(s) => writeln!(s, "{line}").and_then(|_| s.flush()),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stdin already closed",
            )),
        };
        if let Err(e) = sent {
            self.drain();
            let status = self
                .child
                .as_mut()
                .and_then(|c| c.try_wait().ok().flatten());
            panic!(
                "control line {line:?} not delivered ({e}); dc-dserver exit status {status:?}; it said {:?}",
                self.seen
            );
        }
    }

    fn drain(&mut self) {
        while let Ok(l) = self.lines.try_recv() {
            self.seen.push(l);
        }
    }

    fn wait_line(&mut self, prefix: &str, timeout: Duration) -> Option<String> {
        let end = Instant::now() + timeout;
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

    fn events(&self) -> Vec<String> {
        self.seen
            .iter()
            .filter(|l| l.starts_with('@'))
            .cloned()
            .collect()
    }
}

fn world_file(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dc-dserver-e2e-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("synth.dat");
    std::fs::write(&p, synthetic_world_bytes()).unwrap();
    p
}

/// Drives the client over the socket until `done` or the timeout.
fn drive(
    sock: &UdpSocket,
    rd: &SocketReader,
    server: SocketAddr,
    client: &mut SynthClient,
    clock: Instant,
    timeout: Duration,
    mut done: impl FnMut(&SynthClient) -> bool,
) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        let now = clock.elapsed().as_millis() as u32;
        for d in client.poll(now) {
            sock.send_to(&d, server).unwrap();
        }
        loop {
            match rd.next_within(Duration::from_millis(1)) {
                Some(Ok((d, from))) if from == server => {
                    client.handle_datagram(clock.elapsed().as_millis() as u32, &d)
                }
                Some(_) => {}
                None => break,
            }
        }
        if done(client) {
            return true;
        }
    }
    false
}

#[test]
fn binary_runs_the_slice_over_loopback_and_reports_on_quit() {
    let world = world_file("slice");
    let mut p = Proc::spawn(&world, &[]);
    let ready = p
        .wait_line("@ready", Duration::from_secs(20))
        .unwrap_or_else(|| panic!("no @ready: {:?}", p.seen));
    let port: u16 = ready
        .split_whitespace()
        .find_map(|t| t.strip_prefix("port="))
        .and_then(|v| v.parse().ok())
        .expect("port");
    assert_ne!(port, 0);
    assert!(ready.contains("world=synth"), "{ready}");
    p.write("room key=TDM type=0 goal=2 ff=0 time=600");
    p.write("user 7 3 99 0 120 Dave");

    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let rd = SocketReader::start(&sock, 8192).unwrap();
    let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let clock = Instant::now();
    let mut cfg = SynthConfig::named("Dave");
    cfg.load_ms = 300;
    let mut client = SynthClient::new(cfg);
    assert!(
        drive(
            &sock,
            &rd,
            server,
            &mut client,
            clock,
            Duration::from_secs(20),
            |c| { c.state() == ClientState::InWorld }
        ),
        "state {:?}, seen {:?}",
        client.state(),
        client.seen.messages
    );
    // steady state: time entries and player updates. The tick rate is measured on the server's own clock (the
    // time values the entries carry), so a loaded machine that delays this process or the server's datagrams does
    // not change the result: the wait for 12 entries is retried up to 20 s, the rate assertion is the same as a
    // 1.2 s window of 8..=16 entries (a tick period of 75..=150 ms), taken as the median period between
    // consecutive entries (a stall of the server shows as one long gap, not as a slower rate)
    let t0 = clock.elapsed().as_millis() as u32;
    let since_t0 = |c: &SynthClient| c.seen.server_times.iter().filter(|t| t.0 >= t0).count();
    drive(
        &sock,
        &rd,
        server,
        &mut client,
        clock,
        Duration::from_millis(1200),
        |_| false,
    );
    if since_t0(&client) < 12 {
        drive(
            &sock,
            &rd,
            server,
            &mut client,
            clock,
            Duration::from_secs(20),
            |c| since_t0(c) >= 12,
        );
    }
    let times: Vec<f32> = client
        .seen
        .server_times
        .iter()
        .filter(|t| t.0 >= t0)
        .map(|t| t.1)
        .collect();
    assert!(times.len() >= 12, "{} time entries in 21 s", times.len());
    let mut gaps: Vec<f32> = times.windows(2).map(|w| (w[1] - w[0]) * 1000.0).collect();
    gaps.sort_by(f32::total_cmp);
    let median = gaps[gaps.len() / 2];
    assert!(
        (75.0..=150.0).contains(&median),
        "median tick period {median:.1} ms on the server clock (gaps {gaps:?})"
    );
    assert!(client.seen.server_times.windows(2).all(|w| w[1].1 > w[0].1));
    assert!(client.seen.updates_sent >= 10);
    assert!(client.seen.errors.is_empty(), "{:?}", client.seen.errors);
    p.drain();
    assert!(
        p.events().contains(&"@join 0 7 99 0 Dave".to_string()),
        "{:?}",
        p.seen
    );

    // the client leaves with C2S_BYE: bye code 5, @leave
    client.quit();
    assert!(drive(
        &sock,
        &rd,
        server,
        &mut client,
        clock,
        Duration::from_secs(LOAD_CEILING_S),
        |c| matches!(c.state(), ClientState::Dropped(_))
    ));
    assert_eq!(client.state(), ClientState::Dropped(5));
    assert!(
        p.wait_line("@leave 0", Duration::from_secs(LOAD_CEILING_S))
            .is_some(),
        "{:?}",
        p.seen
    );

    // the lobby ends the match
    p.write("quit");
    assert!(
        p.wait_line("@end", Duration::from_secs(LOAD_CEILING_S))
            .is_some(),
        "{:?}",
        p.seen
    );
    let ev = p.events();
    let tail: Vec<&str> = ev
        .iter()
        .map(String::as_str)
        .skip_while(|l| !l.starts_with("@player"))
        .collect();
    assert_eq!(
        tail,
        vec![
            "@player uid=7 kills=0 deaths=0 team=1 tk=0 hs=0 name=Dave",
            "@result mode=TDM win=0 alpha=0 bravo=0 limit=2 rounds=0:0",
            "@end"
        ]
    );
    let mut child = p.child.take().unwrap();
    let end = Instant::now() + Duration::from_secs(LOAD_CEILING_S);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > end {
            let _ = child.kill();
            panic!("dc-dserver did not exit after quit");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status:?}");
    // host log lines never start with '@'
    assert!(p
        .seen
        .iter()
        .any(|l| l.starts_with("dc-dserver: client 0: in world")));
}

#[test]
fn binary_answers_info_and_echo_and_stops_on_stdin_eof() {
    use dc_wire::udp::{OobDatagram, TODO_GAME_APP_ID_GUID_LAYOUT};
    let world = world_file("oob");
    let mut p = Proc::spawn(&world, &["-name", "slice-test"]);
    let ready = p
        .wait_line("@ready", Duration::from_secs(20))
        .expect("ready");
    let port: u16 = ready
        .split_whitespace()
        .find_map(|t| t.strip_prefix("port="))
        .and_then(|v| v.parse().ok())
        .unwrap();
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let rd = SocketReader::start(&sock, 8192).unwrap();
    let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let ask = |d: OobDatagram| -> OobDatagram {
        for _ in 0..10 {
            sock.send_to(&d.encode(), server).unwrap();
            if let Some(Ok((dg, _))) = rd.next_within(Duration::from_millis(500)) {
                return OobDatagram::decode(&dg).unwrap();
            }
        }
        panic!("no answer to {d:?}");
    };
    match ask(OobDatagram::InfoAsk {
        app_id: TODO_GAME_APP_ID_GUID_LAYOUT,
        probe_tag: 3,
    }) {
        OobDatagram::InfoReply(Some(r)) => {
            assert_eq!(r.session_name, b"slice-test");
            assert_eq!((r.probe_tag, r.link_count), (3, 0));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        ask(OobDatagram::EchoAsk { echo_id: 0x123456 }),
        OobDatagram::EchoReply { echo_id: 0x123456 }
    );
    // closing stdin is a quit: no player joined, so only @end
    drop(p.stdin.take());
    assert!(
        p.wait_line("@end", Duration::from_secs(5)).is_some(),
        "{:?}",
        p.seen
    );
    assert_eq!(p.events().last().map(String::as_str), Some("@end"));
    assert!(!p.events().iter().any(|l| l.starts_with("@result")));
}

#[test]
fn binary_without_a_world_prints_an_error_and_end() {
    let out = Command::new(env!("CARGO_BIN_EXE_dc-dserver"))
        .args([
            "-port",
            "0",
            "-world",
            "Worlds\\Nothing",
            "-rom",
            "Z:\\no\\such\\rom",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.lines().any(|l| l.starts_with("dc-dserver: error:")),
        "{text}"
    );
    assert_eq!(text.lines().last(), Some("@end"));
}

/// DSERVER_PIPE_addendum_e2e.md B3 / B1: the server binds exactly the port it was given or fails with an error
/// line and `@end` (exit 3); a negative `-idle` (the lobby passes its value as given) does not fail the launch.
#[test]
fn binary_on_a_busy_port_prints_an_error_and_end() {
    let holder = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = holder.local_addr().unwrap().port().to_string();
    let world = world_file("busy");
    let out = Command::new(env!("CARGO_BIN_EXE_dc-dserver"))
        .args(["-port", &port, "-bindip", "127.0.0.1", "-worldfile"])
        .arg(&world)
        .args(["-guid", "ca", "-idle", "-1"])
        .env_remove("CA_ROM")
        .env_remove("DC_CONFIG")
        .stdin(Stdio::null())
        .output()
        .expect("run");
    drop(holder);
    assert_eq!(out.status.code(), Some(3));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().any(|l| l.contains("cannot bind")), "{text}");
    assert!(
        text.lines().any(|l| l.contains("option ignored: -idle -1")),
        "{text}"
    );
    assert_eq!(text.lines().last(), Some("@end"));
}

/// Slice 2 over real loopback UDP: two synthetic clients of different teams reach player state 1 against the
/// binary, one kills the other, the round ends on the goal of one kill, and the server reports and ends by itself
/// (no `quit`): `@player` x 2, `@result`, `@leave` x 2, `@end`, exit code 0.
#[test]
fn binary_plays_a_tdm_round_to_its_end_over_loopback() {
    let world = world_file("tdm");
    let mut p = Proc::spawn(
        &world,
        &["-damage", "100", "-holds", "300,200", "-tick", "20"],
    );
    let ready = p
        .wait_line("@ready", Duration::from_secs(20))
        .unwrap_or_else(|| panic!("no @ready: {:?}", p.seen));
    let port: u16 = ready
        .split_whitespace()
        .find_map(|t| t.strip_prefix("port="))
        .and_then(|v| v.parse().ok())
        .expect("port");
    p.write("room key=TDM type=1 goal=1 ff=0");
    p.write("user 7 3 99 0 120 Dave");
    p.write("user 8 4 55 1 0 Erin");
    let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let socks: Vec<UdpSocket> = (0..2)
        .map(|_| UdpSocket::bind("127.0.0.1:0").unwrap())
        .collect();
    let rds: Vec<SocketReader> = socks
        .iter()
        .map(|s| SocketReader::start(s, 8192).unwrap())
        .collect();
    let clock = Instant::now();
    let mut clients: Vec<SynthClient> = ["Dave", "Erin"]
        .iter()
        .map(|n| {
            let mut c = SynthConfig::named(n);
            c.load_ms = 200;
            c.walk_step = 0.0;
            // the synthetic world's box (dc_dserver::world::synthetic_world_bytes)
            c.box_bounds = Some(([-512.0, -64.0, -512.0], [512.0, 256.0, 512.0]));
            // DSV-35: the server keeps the link until the client leaves after the result screen
            c.leave_after_result_ms = Some(200);
            SynthClient::new(c)
        })
        .collect();
    let pump = |clients: &mut Vec<SynthClient>,
                until: &dyn Fn(&[SynthClient]) -> bool,
                secs: u64|
     -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            for (i, c) in clients.iter_mut().enumerate() {
                let now = clock.elapsed().as_millis() as u32;
                for d in c.poll(now) {
                    socks[i].send_to(&d, server).unwrap();
                }
                while let Some(Ok((d, from))) = rds[i].next_within(Duration::from_millis(1)) {
                    if from == server {
                        c.handle_datagram(clock.elapsed().as_millis() as u32, &d);
                    }
                }
            }
            if until(clients) {
                return true;
            }
        }
        false
    };
    let erin_seen = |c: &[SynthClient]| {
        c.iter().all(|x| x.life_state() == 1)
            && c[1]
                .seen
                .view_object
                .is_some_and(|o| c[0].seen.remote.contains_key(&o))
    };
    assert!(
        pump(&mut clients, &erin_seen, LOAD_CEILING_S),
        "states {:?} {:?}",
        clients[0].state(),
        clients[1].state()
    );
    let target = clients[1].seen.view_object.unwrap();
    assert!(clients[0].fire_at(target, [0.0; 3], true));
    assert!(
        pump(
            &mut clients,
            &|c| c[0].seen.round_result.is_some(),
            LOAD_CEILING_S
        ),
        "no round result; kills {:?}",
        clients[0].seen.kills
    );
    assert_eq!(clients[0].seen.round_result, Some((1, 0, 0)));
    // the summary and the result screen follow after the two pauses; the clients leave 200 ms later
    assert!(
        pump(
            &mut clients,
            &|c| {
                c.iter()
                    .all(|x| matches!(x.state(), ClientState::Dropped(_)))
            },
            LOAD_CEILING_S,
        ),
        "the clients did not leave in {LOAD_CEILING_S} s: states {:?}, result screens {:?}; server {:?}",
        clients.iter().map(|c| c.state()).collect::<Vec<_>>(),
        clients
            .iter()
            .map(|c| c.seen.result_screen)
            .collect::<Vec<_>>(),
        {
            p.drain();
            p.seen.clone()
        }
    );
    assert!(
        p.wait_line("@end", Duration::from_secs(LOAD_CEILING_S))
            .is_some(),
        "{:?}",
        p.seen
    );
    for c in &clients {
        assert!(c.seen.errors.is_empty(), "{:?}", c.seen.errors);
        assert!(c.seen.result_screen.is_some(), "0x3f before the end");
        assert_eq!(c.seen.summary_rows.len(), 2);
        assert!(c.seen.level_exit.is_none(), "no 0x99");
    }
    let ev = p.events();
    // MON-1: each player's link summary follows its `@leave`; checked apart from the order of the rest
    let sums: Vec<&str> = ev
        .iter()
        .map(String::as_str)
        .filter(|l| l.starts_with("@linksum"))
        .collect();
    assert_eq!(sums.len(), 2, "{sums:?}");
    for l in &sums {
        assert!(
            l.contains(" how=bye ") && !l.contains(" updates=0 "),
            "a client that left by itself: {l}"
        );
    }
    let mut tail: Vec<&str> = ev
        .iter()
        .map(String::as_str)
        .skip_while(|l| !l.starts_with("@player"))
        .filter(|l| !l.starts_with("@linksum"))
        .collect();
    // each client leaves by itself: the two @leave lines come in either order
    if tail.len() == 6 {
        tail[3..5].sort();
    }
    assert_eq!(
        tail,
        vec![
            "@player uid=7 kills=1 deaths=0 team=1 tk=0 hs=0 name=Dave",
            "@player uid=8 kills=0 deaths=1 team=2 tk=0 hs=0 name=Erin",
            "@result mode=TDM win=1 alpha=1 bravo=0 limit=1 rounds=1:0",
            "@leave 0",
            "@leave 1",
            "@end"
        ],
        "{:?}",
        p.seen
    );
    let mut child = p.child.take().unwrap();
    let end = Instant::now() + Duration::from_secs(LOAD_CEILING_S);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > end {
            let _ = child.kill();
            panic!("dc-dserver did not exit after the match");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status:?}");
}

/// DSV-40 (proposed), ROM-gated: the binary on a ROM world that has a global light object sends `S2C_DIR_LIGHT` once
/// to a synthetic client after its world load (the world file is copied out of the user's archive to a temp file at
/// run time; nothing is kept). `-globallight off` sends none.
#[test]
fn binary_sends_the_global_light_after_the_world_load() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let arch = dc_rez::RezArchive::open(&rom.join("Game").join(dc_dserver::world::WORLD_ARCHIVE))
        .expect("world archive");
    // the first world that has the object (found through the server's own loader)
    let mut found = None;
    for (name, off, size) in arch.entries().iter() {
        if !name
            .to_ascii_uppercase()
            .ends_with(dc_dserver::world::WORLD_EXT)
        {
            continue;
        }
        let Ok(bytes) = arch.read_raw(*off, *size) else {
            continue;
        };
        let w = dc_dserver::world::world_from_bytes(&bytes, "Worlds/Synth.dat", "Lit");
        if w.is_ok_and(|w| w.global_light.is_some()) {
            found = Some(bytes);
            break;
        }
    }
    let bytes = found.expect("a ROM world with a global light");
    for (extra, expect) in [(&[][..], true), (&["-globallight", "off"][..], false)] {
        let dir = std::env::temp_dir().join(format!(
            "dc-dserver-e2e-{}-light{}",
            std::process::id(),
            u8::from(expect)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let world = dir.join("lit.dat");
        std::fs::write(&world, &bytes).unwrap();
        let mut p = Proc::spawn(&world, extra);
        let ready = p
            .wait_line("@ready", Duration::from_secs(30))
            .unwrap_or_else(|| panic!("no @ready: {:?}", p.seen));
        let port: u16 = ready
            .split_whitespace()
            .find_map(|t| t.strip_prefix("port="))
            .and_then(|v| v.parse().ok())
            .expect("port");
        p.write("room key=TDM type=0 goal=2 ff=0 time=600");
        p.write("user 7 3 99 0 120 Dave");
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let rd = SocketReader::start(&sock, 8192).unwrap();
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let clock = Instant::now();
        let mut cfg = SynthConfig::named("Dave");
        cfg.load_ms = 300;
        let mut client = SynthClient::new(cfg);
        assert!(
            drive(
                &sock,
                &rd,
                server,
                &mut client,
                clock,
                Duration::from_secs(30),
                |c| { c.state() == ClientState::InWorld }
            ),
            "state {:?}",
            client.state()
        );
        let n22 = client.seen.messages.iter().filter(|m| m.1 == 22).count();
        if expect {
            assert_eq!(n22, 1, "one S2C_DIR_LIGHT");
            let (_, dir, col, _) = client.seen.dir_light.expect("parsed");
            let len = dir.iter().map(|c| c * c).sum::<f32>().sqrt();
            assert!((len - 1.0).abs() < 1e-4 && dir[1] < 0.0, "{dir:?}");
            assert!(col.iter().all(|c| (0.0..=255.0).contains(c)), "{col:?}");
            let ids: Vec<u8> = client.seen.messages.iter().map(|m| m.1).collect();
            let first = |id: u8| ids.iter().position(|&i| i == id).unwrap();
            assert!(first(19) < first(22) && first(22) < first(7));
        } else {
            assert_eq!(n22, 0, "-globallight off sends none");
        }
        assert!(client.seen.errors.is_empty(), "{:?}", client.seen.errors);
        client.quit();
        drive(
            &sock,
            &rd,
            server,
            &mut client,
            clock,
            Duration::from_secs(5),
            |c| matches!(c.state(), ClientState::Dropped(_)),
        );
        p.write("quit");
        let _ = p.wait_line("@end", Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
