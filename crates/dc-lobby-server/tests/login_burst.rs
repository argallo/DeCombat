//! LG-328, over real sockets: a burst of bad logins and registrations from many addresses runs while a logged-in
//! player times a lobby request. Prints the timings; `DC_LOBBY_EXE` runs the same load against another lobby binary.

mod support;

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::*;

/// A TCP connection from a source address of its own on the loopback network.
fn connect_from(src: Ipv4Addr, port: u16) -> std::io::Result<TcpStream> {
    let s = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    s.bind(&SockAddr::from(SocketAddrV4::new(src, 0)))?;
    s.connect(&SockAddr::from(SocketAddr::from((Ipv4Addr::LOCALHOST, port))))?;
    let t: TcpStream = s.into();
    t.set_read_timeout(Some(Duration::from_secs(30)))?;
    Ok(t)
}

/// One source: seven bad logins of its own name, one after the other on one connection.
fn bad_logins(src: Ipv4Addr, port: u16, n: usize, done: &AtomicUsize, stop: &AtomicBool) {
    let Ok(mut t) = connect_from(src, port) else {
        return;
    };
    let name = format!("burst{n}");
    for i in 0..8 {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // the first one sets the password of the new account, the others are wrong
        let f = frame(0x200, |w| {
            w.wstr(&format!("LOCAL:{name}"), 4096)
                .wstr(&format!("guess{}", i.min(1) * i), 17)
                .wstr("00-11-22-33-44-55", 20)
                .i32(0);
        });
        if t.write_all(&f).is_err() {
            return;
        }
        match dc_wire::read_tcp_frame(&mut t) {
            Ok(Some(_)) => {
                done.fetch_add(1, Ordering::Relaxed);
            }
            _ => return,
        }
    }
}

/// One registration over the web port from `src`; true when it answered 200.
fn register(src: Ipv4Addr, web: u16, n: usize) -> bool {
    let Ok(mut t) = connect_from(src, web) else {
        return false;
    };
    let body = format!(
        "name=webuser{n}&pw=correct-horse{n}&pw2=correct-horse{n}&email=u{n}%40example.org&code="
    );
    let req = format!(
        "POST /register HTTP/1.1\r\nHost: test\r\nContent-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    if t.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut out = String::new();
    let _ = t.read_to_string(&mut out);
    out.starts_with("HTTP/1.1 200")
}

fn pct(v: &[f64], p: f64) -> f64 {
    v[((v.len() - 1) as f64 * p) as usize]
}

#[test]
fn a_login_burst_does_not_stall_lobby_play() {
    let l = Lobby::start(
        "login_burst",
        LobbyOpts {
            extra: vec![
                "--check-password".into(),
                "--register-open".into(),
            ],
            ..Default::default()
        },
    );
    let (mut c, _) = player(&l, "timer", "Timer");
    let time_requests = |c: &mut Client, ms: u64| -> Vec<f64> {
        let end = Instant::now() + Duration::from_millis(ms);
        let mut v = Vec::new();
        while Instant::now() < end {
            let t = Instant::now();
            c.send(0x376, |_| {});
            c.recv_until(0x377, Duration::from_secs(60));
            v.push(t.elapsed().as_secs_f64() * 1000.0);
            std::thread::sleep(Duration::from_millis(20));
        }
        v.sort_by(f64::total_cmp);
        v
    };
    let quiet = time_requests(&mut c, 1000);

    let done = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let registered = Arc::new(AtomicUsize::new(0));
    let t0 = Instant::now();
    let mut burst = Vec::new();
    let port = l.port;
    let web = l.web;
    for n in 0..200usize {
        let (done, stop, registered) = (done.clone(), stop.clone(), registered.clone());
        let src = Ipv4Addr::new(127, 1, (n / 250) as u8, (n % 250 + 1) as u8);
        burst.push(std::thread::spawn(move || {
            if n % 10 == 0 && register(src, web, n) {
                registered.fetch_add(1, Ordering::Relaxed);
            }
            bad_logins(src, port, n, &done, &stop);
        }));
    }
    let loaded = time_requests(&mut c, 6000);
    stop.store(true, Ordering::Relaxed);
    let during = done.load(Ordering::Relaxed);
    for b in burst {
        let _ = b.join();
    }
    let reg = registered.load(Ordering::Relaxed);
    println!(
        "lobby request ms: quiet p50 {:.1} p95 {:.1} max {:.1} ({} requests); burst p50 {:.1} p95 {:.1} max {:.1} ({} \
         requests); {during} login answers in {:.1} s, {reg} registrations",
        pct(&quiet, 0.5),
        pct(&quiet, 0.95),
        quiet[quiet.len() - 1],
        quiet.len(),
        pct(&loaded, 0.5),
        pct(&loaded, 0.95),
        loaded[loaded.len() - 1],
        loaded.len(),
        t0.elapsed().as_secs_f64(),
    );
    assert!(during > 20, "the burst ran");
    assert!(reg > 0, "registrations went through the pool");
    let count: i64 = l
        .db()
        .query_row(
            "SELECT COUNT(*) FROM accounts WHERE login LIKE 'webuser%' AND pw_hash LIKE '$argon2id$%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count as usize, reg);
    if std::env::var_os("DC_LOBBY_EXE").is_none() {
        // far above the p95 this gives; with the hashes under the lock a request waits for dozens of them
        assert!(
            pct(&loaded, 0.95) < 250.0,
            "p95 {:.1} ms",
            pct(&loaded, 0.95)
        );
    }
    // the player is still fine
    c.send(0x376, |_| {});
    c.recv_until(0x377, Duration::from_secs(10));
}

/// LG-329 over a real connection: a flood of chat is cut to its burst and the connection stays usable.
#[test]
fn a_chat_flood_is_cut_and_the_player_stays() {
    let l = Lobby::start("chat_flood", LobbyOpts::default());
    let (mut c, _) = player(&l, "talker", "Talker");
    let (mut d, _) = player(&l, "listener", "Listener");
    for _ in 0..50 {
        c.send(0x3a0, |w| {
            w.wstr("spam", 121);
        });
    }
    // what arrives at the other player is within the burst plus what the time of the test refilled
    let end = Instant::now() + Duration::from_millis(800);
    let mut lines = 0;
    while Instant::now() < end {
        if d.saw_within(0x3051, Duration::from_millis(50)) {
            d.recv_until(0x3051, Duration::from_millis(50));
            lines += 1;
        }
    }
    assert!((8..=12).contains(&lines), "{lines} lines");
    c.send(0x376, |_| {});
    c.recv_until(0x377, Duration::from_secs(5));
    assert!(l.wait_log("over the message rate", Duration::from_secs(5)));
}
