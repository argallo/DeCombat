//! The round loop over real loopback UDP (GMD-20..GMD-23): the `dc-dserver` binary on an ephemeral port with the
//! synthetic world file, two synthetic clients on their own sockets, an elimination room (class 12) with goal 2. The
//! first kill ends round 1 and the dead client comes back in round 2 (0x44 with round 2); the second kill ends the
//! match with the result screen and one `@result` carrying the round wins. The child is killed if the test fails.
// SPEC: GAME_MODES_2009.md §4.1, §4.2, §5.8; DSERVER_PIPE.md §3.2 (room and user lines)
// DC-DEFAULT: GMD-5, GMD-13 (proposed); GMD-20..GMD-23 (staged)

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig};
use dc_dserver::world::synthetic_world_bytes;
use dc_net_engine::socket_reader::SocketReader;

/// Ceiling (seconds) of every wait: the machine is shared with parallel builds, so the waits end as soon as the state
/// is reached and only a slow run uses the ceiling.
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
                "60",
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
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
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

    fn write(&mut self, line: &str) {
        let s = self.stdin.as_mut().expect("stdin");
        writeln!(s, "{line}")
            .and_then(|_| s.flush())
            .expect("control line");
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
}

fn world_file() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dc-dserver-e2e-{}-roundloop", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("synth.dat");
    std::fs::write(&p, synthetic_world_bytes()).unwrap();
    p
}

#[test]
fn binary_plays_an_elimination_match_of_two_rounds_over_loopback() {
    let world = world_file();
    let mut p = Proc::spawn(
        &world,
        &["-damage", "100", "-holds", "300,200", "-tick", "20"],
    );
    let ready = p
        .wait_line("@ready", Duration::from_secs(LOAD_CEILING_S))
        .unwrap_or_else(|| panic!("no @ready: {:?}", p.seen));
    let port: u16 = ready
        .split_whitespace()
        .find_map(|t| t.strip_prefix("port="))
        .and_then(|v| v.parse().ok())
        .expect("port");
    p.write("room key=ELIM type=12 goal=2 ff=0");
    p.write("user 7 3 99 0 120 Dave");
    p.write("user 8 4 55 1 0 Erin");
    let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let socks: Vec<UdpSocket> = (0..2)
        .map(|_| UdpSocket::bind("127.0.0.1:0").unwrap())
        .collect();
    // a reader thread per socket (Windows read timeouts lose datagrams; master 80381ff / b436750)
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
            c.box_bounds = Some(([-512.0, -64.0, -512.0], [512.0, 256.0, 512.0]));
            c.leave_after_result_ms = Some(200);
            SynthClient::new(c)
        })
        .collect();
    let pump = |clients: &mut Vec<SynthClient>, until: &dyn Fn(&[SynthClient]) -> bool| -> bool {
        let end = Instant::now() + Duration::from_secs(LOAD_CEILING_S);
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
    let both_seen = |c: &[SynthClient]| {
        c.iter().all(|x| x.life_state() == 1)
            && c[1]
                .seen
                .view_object
                .is_some_and(|o| c[0].seen.remote.contains_key(&o))
    };
    assert!(pump(&mut clients, &both_seen), "no start");
    let count = |c: &SynthClient, id: u8| c.seen.shell_ids.iter().filter(|x| x.1 == id).count();
    let begins = count(&clients[1], 0x44);

    // round 1
    let target = clients[1].seen.view_object.unwrap();
    assert!(clients[0].fire_at(target, [0.0; 3], true));
    assert!(
        pump(&mut clients, &|c| c[0].seen.round_result.is_some()),
        "no round result; kills {:?}",
        clients[0].seen.kills
    );
    // DSV-400: the round wins after the round
    assert_eq!(clients[0].seen.round_result, Some((1, 1, 0)));
    // the dead client comes back with the next round (0x44 again), the stream shows both again
    assert!(
        pump(&mut clients, &|c| count(&c[1], 0x44) > begins
            && both_seen(c)),
        "no second round"
    );
    assert!(clients
        .iter()
        .all(|c| c.seen.result_screen.is_none() && c.seen.summary_rows.is_empty()));

    // round 2 decides the match
    let target = clients[1].seen.view_object.unwrap();
    assert!(clients[0].fire_at(target, [0.0; 3], true));
    assert!(
        pump(&mut clients, &|c| c
            .iter()
            .all(|x| matches!(x.state(), ClientState::Dropped(_)))),
        "the clients did not leave: {:?}",
        {
            p.drain();
            p.seen.clone()
        }
    );
    for c in &clients {
        assert!(c.seen.errors.is_empty(), "{:?}", c.seen.errors);
        assert_eq!(c.seen.round_result, Some((1, 2, 0)));
        assert_eq!(c.seen.result_value, Some((false, 1)));
        assert_eq!(c.seen.summary_rows.len(), 2);
    }
    assert!(
        p.wait_line("@end", Duration::from_secs(LOAD_CEILING_S))
            .is_some(),
        "{:?}",
        p.seen
    );
    let results: Vec<&String> = p.seen.iter().filter(|l| l.starts_with("@result")).collect();
    assert_eq!(
        results,
        vec!["@result mode=ELIM win=1 alpha=2 bravo=0 limit=2 rounds=2:0"],
        "{:?}",
        p.seen
    );
    assert!(
        p.seen.iter().any(|l| l.contains("round 2 started")),
        "{:?}",
        p.seen
    );
}
