//! A stand-in dedicated server for the lobby tests: a tiny stdin/stdout program that follows the line contract of
//! `specs/DSERVER_PIPE.md` and nothing else. The tests compile this file with `rustc` at run time (std only), so
//! it is never part of the lobby crate. It is our own test tooling.
//!
//! Environment (inherited from the lobby, which inherits it from the test):
//! - `DC_FAKE_DIR`: directory for the records: `launch_<pid>.txt` (one argument per line), `stdin_<pid>.txt` (every
//!   control line as received), and `mode` (read once at launch: `normal`, `linger` or `record`; default `normal`).
//!
//! Modes:
//! - `normal`: binds the UDP game port on 127.0.0.1 (or `-bindip`) like a real server, prints `@ready`, waits until at least one
//!   `user` line arrived and 1.0 s passed since the last one, then prints `@connect`/`@join` per player, a `@player`
//!   per player (team 0 = Alpha: 2 kills, 1 headshot, reported exp=3 gp=1; others: 2 deaths), `@result` TWICE
//!   (the lobby must store it once), `@leave` per player and `@end`, and exits 0. `quit` or stdin EOF stops it.
//! - `linger`: like `normal`, but after `@end` it keeps running and keeps the port (ignores `quit` and stdin EOF)
//!   for up to 30 s: the "our own stale server still holds the port" case.
//! - `record`: binds, prints `@ready`, prints nothing else until `quit` / stdin EOF, then `@end`.
//! - `hisec`: like `normal`, plus, after the `@join` lines, the HiSec pickup line of the first player TWICE with the
//!   same case key (the lobby must deliver one case) and once for an account that is not in the match.
// SPEC: DSERVER_PIPE.md §2 (launch), §3 (control lines), §4 (events), §5 (event order of the stand-in)
// HARNESS: lobby_harness FAKE_DSERVER (same event shape; the settle time and modes are ours)

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn say(line: &str) {
    let out = std::io::stdout();
    let mut o = out.lock();
    let _ = writeln!(o, "{line}");
    let _ = o.flush();
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::env::var("DC_FAKE_DIR").ok().map(PathBuf::from);
    let pid = std::process::id();
    let mode = dir
        .as_ref()
        .and_then(|d| std::fs::read_to_string(d.join("mode")).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "normal".into());
    if let Some(d) = &dir {
        let _ = std::fs::write(d.join(format!("launch_{pid}.txt")), args.join("\n"));
    }
    let port: u16 = arg(&args, "-port").and_then(|p| p.parse().ok()).unwrap_or(0);
    // `-bindip` as the real server takes it (default 127.0.0.1)
    let ip = arg(&args, "-bindip").unwrap_or_else(|| "127.0.0.1".into());
    // an address this host lacks (a forwarded setup's advertised address) falls back to 127.0.0.1
    let sock = match std::net::UdpSocket::bind((ip.as_str(), port)).or_else(|e| {
        if e.kind() == std::io::ErrorKind::AddrNotAvailable {
            std::net::UdpSocket::bind(("127.0.0.1", port))
        } else {
            Err(e)
        }
    }) {
        Ok(s) => s,
        Err(e) => {
            say(&format!("fake-dserver: cannot bind {ip}:{port}: {e}"));
            say("@end");
            std::process::exit(3);
        }
    };
    let bound = sock.local_addr().map(|a| a.port()).unwrap_or(port);
    say(&format!("fake-dserver: mode {mode} pid {pid}"));
    say(&format!("@ready port={bound} world=fake"));

    // control lines: Some(line) or None on EOF
    let (tx, rx) = mpsc::channel::<Option<String>>();
    let log = dir.as_ref().map(|d| d.join(format!("stdin_{pid}.txt")));
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(l) = line else { break };
            if let Some(p) = &log {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                    let _ = writeln!(f, "{l}");
                }
            }
            if tx.send(Some(l)).is_err() {
                return;
            }
        }
        let _ = tx.send(None);
    });

    let mut users: Vec<Vec<String>> = Vec::new();
    let mut last_user = Instant::now();
    let settle = Duration::from_millis(1000);
    let mut stop = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Some(l)) => {
                let t: Vec<String> = l.split_whitespace().map(String::from).collect();
                match t.first().map(String::as_str) {
                    Some("user") if t.len() >= 7 => {
                        users.push(t);
                        last_user = Instant::now();
                    }
                    Some("quit") => stop = true,
                    _ => {}
                }
            }
            Ok(None) => stop = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => stop = true,
        }
        if stop && mode != "linger" {
            say("@end");
            return;
        }
        if mode != "record" && !users.is_empty() && last_user.elapsed() >= settle {
            break;
        }
    }

    for (i, u) in users.iter().enumerate() {
        say(&format!("@connect {i}"));
        // header form: @join <client id> <uid> <key> <team> <name>
        say(&format!("@join {i} {} {} {} {}", u[1], u[3], u[4], u[6]));
    }
    if mode == "hisec" {
        if let Some(u) = users.first() {
            // DeCombat addition HS-13: @hisec uid= case= key= name= (name last)
            say(&format!("@hisec uid={} case=0 key=1 name={}", u[1], u[6]));
            say(&format!("@hisec uid={} case=0 key=1 name={}", u[1], u[6]));
            say("@hisec uid=4000000000 case=0 key=2 name=Stranger");
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    for u in &users {
        let alpha = u[4] == "0";
        let team: u32 = u[4].parse::<u32>().unwrap_or(0) + 1;
        say(&format!(
            "@player uid={} kills={} deaths={} team={} exp={} gp={} tk=0 hs={} name={}",
            u[1],
            if alpha { 2 } else { 0 },
            if alpha { 0 } else { 2 },
            team,
            if alpha { 3 } else { 0 },
            if alpha { 1 } else { 0 },
            if alpha { 1 } else { 0 },
            u[6]
        ));
    }
    say("@result mode=TDM win=1 alpha=2 bravo=0 limit=2 rounds=0:0");
    // a second result for the same game: the lobby stores exactly one (DSERVER_PIPE.md §4.5 exactly-once)
    say("@result mode=TDM win=1 alpha=2 bravo=0 limit=2 rounds=0:0");
    std::thread::sleep(Duration::from_millis(200));
    for i in 0..users.len() {
        say(&format!("@leave {i}"));
    }
    say("@end");
    if mode == "linger" {
        // keep the port and ignore every control line or EOF, bounded so a failed test leaves no orphan for long
        let end = Instant::now() + Duration::from_secs(30);
        while Instant::now() < end {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    drop(sock);
}
