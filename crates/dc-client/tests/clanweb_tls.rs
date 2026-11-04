//! The client's clan page fetch over TLS (CLI-1470) against `dc-lobby-server --tls loopback` on ephemeral loopback
//! ports: the fetch gets the page, a plain request gets the redirect to https (LG-224), and `dc-lobby-server tls`
//! prints the fingerprint the lobby logged (LG-222).

mod support;

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::process::Command;
use std::time::Duration;

use dc_client::menu::link::clan_web_get;
use support::*;

#[test]
fn the_clan_page_fetch_speaks_tls_and_the_fingerprint_matches_the_log() {
    let watchdog = Watchdog::arm("clanweb-tls", Duration::from_secs(120));
    let lobby = lobby_exe();
    let dir = scratch("clanweb-tls");
    let log = dir.join("lobby.log");
    let (port, lport, cport, web) = loop {
        let p = (free_tcp_port(), free_tcp_port(), free_tcp_port(), free_tcp_port());
        let mut v = vec![p.0, p.1, p.2, p.3];
        v.sort();
        v.dedup();
        if v.len() == 4 {
            break p;
        }
    };
    let mut cmd = Command::new(&lobby);
    cmd.args(["--port", &port.to_string(), "--lobbyport", &lport.to_string()])
        .args(["--clanport", &cport.to_string(), "--clanweb", &web.to_string()])
        .arg("--db")
        .arg(dir.join("lobby.db"))
        .arg("--log")
        .arg(&log)
        .args(["--tls", "loopback"])
        .current_dir(&dir);
    let lob = Proc::spawn(cmd);
    watchdog.watch_pid(lob.pid());
    let up = (0..600).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        std::net::TcpStream::connect(("127.0.0.1", web)).is_ok()
    });
    assert!(up, "lobby did not listen");
    let addr: SocketAddr = format!("127.0.0.1:{web}").parse().unwrap();
    let body = clan_web_get(addr, "/clan?fmt=text");
    assert!(
        body.is_some(),
        "the clan page over TLS; log: {}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    // plain HTTP on the TLS-only port: a redirect, never the page
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET /clan HTTP/1.1\r\nHost: 127.0.0.1:{web}\r\n\r\n").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 301"), "{out}");
    assert!(out.contains(&format!("Location: https://127.0.0.1:{web}/clan")), "{out}");
    // the subcommand prints the fingerprint of the pair the lobby made next to its DB
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let printed = Command::new(&lobby)
        .args(["tls", "--db"])
        .arg(dir.join("lobby.db"))
        .output()
        .unwrap();
    assert!(printed.status.success());
    let fp = String::from_utf8_lossy(&printed.stdout).trim().to_string();
    assert_eq!(fp.len(), 95, "{fp}");
    assert!(
        text.contains(&format!("--server-fingerprint {fp}")),
        "the log names the same fingerprint:\n{text}"
    );
    drop(lob);
}
