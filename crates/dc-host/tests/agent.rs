//! The agent binary against a fake lobby (HOST_AGENT.md §8): a TLS listener with a throw-away certificate and the
//! stub game server of examples/stub_dserver.rs. Checks hello, start / started, event, line, ping, quit, exited, the
//! grace kill, the reconnect with `running`, the refused pin and the refused config.

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TOKEN: &str = "test-token-0123456789abcdef";

fn exe_suffix() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else {
        ""
    }
}

/// The stub game server: target/<profile>/examples/stub_dserver (cargo test builds the examples; built here when a
/// filtered run did not).
fn stub() -> PathBuf {
    let deps = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let p = deps
        .parent()
        .unwrap()
        .join("examples")
        .join(format!("stub_dserver{}", exe_suffix()));
    if !p.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args(["build", "-p", "dc-host", "--example", "stub_dserver"])
            .status()
            .unwrap();
        assert!(st.success(), "building the stub game server failed");
    }
    assert!(p.is_file(), "no stub game server at {}", p.display());
    p
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc-host-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fake lobby: a TLS 1.3 listener on loopback with a fresh self-signed certificate.
struct Lobby {
    listener: TcpListener,
    config: Arc<ServerConfig>,
    fingerprint: String,
}

impl Lobby {
    fn new() -> Lobby {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let der = ck.cert.der().to_vec();
        let fingerprint = dc_host::tls::fingerprint(&der);
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ck.key_pair.serialize_der()));
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![CertificateDer::from(der)], key)
                .unwrap();
        Lobby {
            listener: TcpListener::bind("127.0.0.1:0").unwrap(),
            config: Arc::new(config),
            fingerprint,
        }
    }

    fn addr(&self) -> String {
        self.listener.local_addr().unwrap().to_string()
    }

    /// The next agent connection, its TLS handshake done.
    fn accept(&self, within: Duration) -> Conn {
        self.listener.set_nonblocking(true).unwrap();
        let end = Instant::now() + within;
        let sock = loop {
            match self.listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == ErrorKind::WouldBlock && Instant::now() < end => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(e) => panic!("no agent connection: {e}"),
            }
        };
        sock.set_nonblocking(false).unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let conn = ServerConnection::new(Arc::clone(&self.config)).unwrap();
        Conn {
            s: StreamOwned::new(conn, sock),
            buf: Vec::new(),
        }
    }
}

struct Conn {
    s: StreamOwned<ServerConnection, TcpStream>,
    buf: Vec<u8>,
}

impl Conn {
    fn send(&mut self, line: &str) {
        self.s.write_all(format!("{line}\n").as_bytes()).unwrap();
        self.s.flush().unwrap();
    }

    /// The next line within `within`; None on a time-out or the end of the stream.
    fn line(&mut self, within: Duration) -> Option<String> {
        let end = Instant::now() + within;
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let l: Vec<u8> = self.buf.drain(..=i).collect();
                return Some(String::from_utf8_lossy(&l[..l.len() - 1]).into_owned());
            }
            if Instant::now() > end {
                return None;
            }
            let mut b = [0u8; 4096];
            match self.s.read(&mut b) {
                Ok(0) => return None,
                Ok(n) => self.buf.extend_from_slice(&b[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => return None,
            }
        }
    }

    /// The first line starting with `prefix` (others are kept in `skipped`).
    fn expect(&mut self, prefix: &str, within: Duration, skipped: &mut Vec<String>) -> String {
        let end = Instant::now() + within;
        while Instant::now() < end {
            if let Some(l) = self.line(end.saturating_duration_since(Instant::now())) {
                if l.starts_with(prefix) {
                    return l;
                }
                skipped.push(l);
            }
        }
        panic!("no line starting with {prefix:?} within {within:?}; seen {skipped:?}");
    }
}

fn field<'a>(line: &'a str, key: &str) -> &'a str {
    line.split(' ')
        .find_map(|w| w.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
        .unwrap_or_else(|| panic!("no {key}= in {line:?}"))
}

/// A free range of three loopback UDP ports.
fn port_range() -> (u16, u16) {
    for _ in 0..50 {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let lo = s.local_addr().unwrap().port();
        drop(s);
        if lo < 60000 && (lo..lo + 3).all(|p| UdpSocket::bind(("127.0.0.1", p)).is_ok()) {
            return (lo, lo + 2);
        }
    }
    // the system's next ephemeral port can sit above 60000 for a long while (Windows hands them out in order)
    for lo in (20000..60000u16).step_by(97) {
        if (lo..lo + 3).all(|p| UdpSocket::bind(("127.0.0.1", p)).is_ok()) {
            return (lo, lo + 2);
        }
    }
    panic!("no three free UDP ports in a row");
}

struct Agent(Child);

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn write_config(dir: &Path, lobby: &str, fp: &str, ports: (u16, u16)) -> PathBuf {
    let rom = dir.join("rom");
    std::fs::create_dir_all(&rom).unwrap();
    let text = format!(
        "name = \"us-west\"\nlobby = \"{lobby}\"\nfingerprint = \"{fp}\"\ntoken = \"{TOKEN}\"\n\
         dserver_exe = {exe:?}\nrom = {rom:?}\nport_range = \"{}-{}\"\nbind_ip = \"127.0.0.1\"\n\
         log_dir = {logs:?}\ndserver_args = \"-maxplayers 16\"\n",
        ports.0,
        ports.1,
        exe = stub().display().to_string(),
        rom = rom.display().to_string(),
        logs = dir.join("logs").display().to_string(),
    );
    let p = dir.join("host.toml");
    std::fs::write(&p, text).unwrap();
    p
}

fn agent(config: &Path) -> Agent {
    Agent(
        Command::new(env!("CARGO_BIN_EXE_dc-host"))
            .arg("--config")
            .arg(config)
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

const T: Duration = Duration::from_secs(10);

#[test]
fn the_agent_runs_game_servers_for_the_lobby() {
    let dir = tmpdir("main");
    let lobby = Lobby::new();
    let ports = port_range();
    let cfg = write_config(&dir, &lobby.addr(), &lobby.fingerprint, ports);
    let _agent = agent(&cfg);
    let mut c = lobby.accept(T);
    let mut seen = Vec::new();

    // hello
    let hello = c.line(T).expect("hello");
    assert!(hello.starts_with("hello "), "{hello}");
    assert_eq!(field(&hello, "name"), "us-west");
    assert_eq!(field(&hello, "token"), TOKEN);
    assert_eq!(field(&hello, "ports"), format!("{}-{}", ports.0, ports.1));
    assert_eq!(field(&hello, "running"), "0");
    assert!(!field(&hello, "version").is_empty());
    assert_eq!(field(&hello, "content"), "1");
    assert_eq!(field(&hello, "packs"), "1");
    c.send("ok name=us-west lobby=test");

    // start: the first port of the range, the lobby's args filtered, the stub's lines come back as events
    c.send(r#"start 7 {"port":0,"world":"Worlds/Test A","guid":"ca","idle":30,"log":"g7.log","args":["-tickhz","30","rm","-rf","-log","/etc/x"]}"#);
    let st = c.expect("started 7 ", T, &mut seen);
    let port7: u16 = field(&st, "port").parse().unwrap();
    assert_eq!(port7, ports.0);
    assert!(field(&st, "pid").parse::<u32>().unwrap() > 0);
    let ready = c.expect("event 7 @ready", T, &mut seen);
    assert_eq!(
        ready,
        format!("event 7 @ready port={port7} world=Worlds/Test A")
    );
    let args = c.expect("event 7 @args ", T, &mut seen);
    let logs = dir.join("logs");
    assert_eq!(
        args,
        format!(
            "event 7 @args -port|{port7}|-world|Worlds/Test A|-guid|ca|-idle|30|-log|{}|-bindip|127.0.0.1|-maxplayers|16|-tickhz|30|-rf",
            logs.join("g7.log").display()
        )
    );
    let env = c.expect("event 7 @env ", T, &mut seen);
    assert!(
        env.contains(&format!("rom={}", dir.join("rom").display())),
        "{env}"
    );
    let cwd = std::fs::canonicalize(&logs).unwrap();
    let shown = PathBuf::from(env.split(" cwd=").nth(1).unwrap());
    assert_eq!(std::fs::canonicalize(shown).unwrap(), cwd, "{env}");

    // line -> stdin, verbatim
    c.send("line 7 hello  there ");
    assert_eq!(
        c.expect("event 7 echo:", T, &mut seen),
        "event 7 echo:hello  there "
    );

    // ping
    c.send("ping 42");
    assert_eq!(c.expect("pong ", T, &mut seen), "pong 42");

    // a second start of the same gid answers `started` again with the same port
    c.send(r#"start 7 {"world":"W"}"#);
    assert_eq!(c.expect("started 7 ", T, &mut seen), st);

    // a requested port that one of ours holds is not given out twice; the ignoreeof server for the grace kill
    c.send(&format!(
        r#"start 8 {{"port":{port7},"world":"W8","args":["-ignoreeof"]}}"#
    ));
    let st8 = c.expect("started 8 ", T, &mut seen);
    let port8: u16 = field(&st8, "port").parse().unwrap();
    assert_eq!(port8, ports.0 + 1);
    c.expect("event 8 @ready", T, &mut seen);

    // a bad request and a log name outside log_dir fail
    c.send(r#"start 9 {"world":"W","log":"../escape.log"}"#);
    assert!(c.expect("failed 9 ", T, &mut seen).contains("log"));

    // a server ending by itself reports its code
    c.send(r#"start 10 {"world":"W","args":["-exit","3"]}"#);
    c.expect("started 10 ", T, &mut seen);
    assert_eq!(c.expect("exited 10 ", T, &mut seen), "exited 10 code=3");

    // quit: stdin closed, the stub ends with code 0
    c.send("quit 7");
    assert_eq!(c.expect("exited 7 ", T, &mut seen), "exited 7 code=0");

    // the reconnect: the stream drops, the agent says hello again within the backoff and lists game 8
    drop(c);
    let mut c = lobby.accept(T);
    let hello = c.line(T).expect("second hello");
    assert_eq!(field(&hello, "running"), "1");
    c.send("ok name=us-west lobby=test");
    let run = c.expect("running ", T, &mut seen);
    assert_eq!(
        run,
        format!("running 8 port={port8} pid={}", field(&st8, "pid"))
    );

    // quit of a server that ignores stdin EOF: killed after the 5 s grace
    let t0 = Instant::now();
    c.send("quit 8");
    assert_eq!(
        c.expect("exited 8 ", Duration::from_secs(15), &mut seen),
        // a killed child has no exit code on Unix (-1); Windows' TerminateProcess leaves code 1 (the Windows test lane)
        if cfg!(windows) {
            "exited 8 code=1"
        } else {
            "exited 8 code=-1"
        }
    );
    assert!(t0.elapsed() >= Duration::from_secs(4), "{:?}", t0.elapsed());

    let _ = std::fs::remove_dir_all(&dir);
}

/// MAP-84 over the real stream: bytes that do not hash to the pack's name start nothing and say `failed <gid> pack
/// <sha>: ...`; a pack nobody sent says the same, and no game server runs for either.
#[test]
fn a_bad_map_pack_fails_the_start_and_names_the_pack() {
    let dir = tmpdir("badpack");
    let lobby = Lobby::new();
    let cfg = write_config(&dir, &lobby.addr(), &lobby.fingerprint, port_range());
    let _agent = agent(&cfg);
    let mut c = lobby.accept(T);
    assert!(c.line(T).expect("hello").starts_with("hello "));
    c.send("ok name=us-west lobby=test");
    let mut seen = Vec::new();
    let sha = "ab".repeat(32);
    let start = |gid: u32, size: u32| {
        format!(
            r#"start {gid} {{"world":"@mod:m/assets/maps/m.dat","world_pack":{{"sha256":"{sha}","size":{size},"url":"/mods/{sha}.zip"}},"world_file":"assets/maps/m.dat","announce":"@mod:m/assets/maps/m.dat"}}"#
        )
    };
    c.send(&format!("pack {sha} Zm9v"));
    c.send(&start(5, 3));
    let f = c.expect("failed 5 ", T, &mut seen);
    assert!(
        f.starts_with(&format!("failed 5 pack {sha}: ")) && f.contains("hash"),
        "{f}"
    );
    c.send(&start(6, 3));
    let f = c.expect("failed 6 ", T, &mut seen);
    assert!(f.contains("does not hold"), "{f}");
    // the stream carries on, and the bad bytes were not kept
    c.send("ping 9");
    c.expect("pong 9", T, &mut seen);
    assert!(!seen.iter().any(|l| l.starts_with("started ")), "{seen:?}");
    let packs = dir.join("packs");
    assert!(!packs.join(&sha).exists() && !packs.join(format!("{sha}.zip")).exists());
}

#[test]
fn a_refused_hello_is_retried_and_a_wrong_certificate_never_gets_the_hello() {
    let dir = tmpdir("refuse");
    let lobby = Lobby::new();
    let cfg = write_config(&dir, &lobby.addr(), &lobby.fingerprint, port_range());
    let _agent = agent(&cfg);
    let mut c = lobby.accept(T);
    assert!(c.line(T).expect("hello").starts_with("hello "));
    c.send("refuse bad token");
    drop(c);
    let mut c = lobby.accept(T);
    assert!(c.line(T).expect("hello again").starts_with("hello "));
    drop(c);
    drop(_agent);

    // a lobby with another certificate: the handshake fails on the agent's side, no hello (no token) is sent
    let other = Lobby::new();
    let pinned_elsewhere = Lobby::new().fingerprint;
    let cfg = write_config(&dir, &other.addr(), &pinned_elsewhere, port_range());
    let _agent = agent(&cfg);
    let mut c = other.accept(T);
    assert_eq!(c.line(Duration::from_secs(3)), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_config_without_fingerprint_or_token_is_refused() {
    let dir = tmpdir("nofp");
    let p = dir.join("host.toml");
    std::fs::write(
        &p,
        format!("name = \"a\"\nlobby = \"127.0.0.1:1\"\ntoken = \"{TOKEN}\"\n"),
    )
    .unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_dc-host"))
        .arg("--config")
        .arg(&p)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("fingerprint is missing"));
    let fp = Lobby::new().fingerprint;
    std::fs::write(
        &p,
        format!("name = \"a\"\nlobby = \"127.0.0.1:1\"\nfingerprint = \"{fp}\"\n"),
    )
    .unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_dc-host"))
        .arg("--config")
        .arg(&p)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("token is missing"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn version_prints_a_sha_or_unknown() {
    let o = Command::new(env!("CARGO_BIN_EXE_dc-host"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(o.status.success());
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.starts_with("dc-host "), "{s}");
    assert!(!s.trim().ends_with("dc-host"), "{s}");
}
