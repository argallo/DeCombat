//! TLS on the auth and web ports (LG-220..226): the pair on disk, the gate rules, a login over TLS on a loopback
//! auth listener, plain refused where TLS is required, and the web port's TLS page and plain redirect.

use super::*;
use crate::logs::Listener;
use crate::msg::{LOGIN_REPLY, LOGIN_REQUEST};
use crate::state::testing::*;
use crate::state::Shared;
use dc_wire::PktWriter;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU32, Ordering};

static N: AtomicU32 = AtomicU32::new(0);

fn temp_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "dc-tls-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// A test client verifier: the certificate must have the pinned fingerprint.
#[derive(Debug)]
struct Pin(String);

impl ServerCertVerifier for Pin {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if fingerprint(end_entity.as_ref()) == self.0 {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("pin".into()))
        }
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2".into()))
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            m,
            c,
            d,
            &provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn tls_client(port: u16, pin: &str) -> rustls::StreamOwned<ClientConnection, TcpStream> {
    let cfg = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pin(pin.to_string())))
        .with_no_client_auth();
    let c = ClientConnection::new(Arc::new(cfg), ServerName::try_from("localhost").unwrap()).unwrap();
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    rustls::StreamOwned::new(c, s)
}

fn login_frame(login: &str, pw: &str) -> Vec<u8> {
    let mut w = PktWriter::new(LOGIN_REQUEST);
    w.wstr(&format!("LOCAL:{login}"), 4096)
        .wstr(pw, 17)
        .wstr("00-11", 20)
        .i32(0);
    w.to_frame()
}

fn reply_of<R: Read>(r: &mut R, want: u16) -> Option<Vec<u8>> {
    while let Ok(Some((id, p))) = dc_wire::read_tcp_frame(r) {
        if id as u16 == want {
            return Some(p);
        }
    }
    None
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

#[test]
fn the_pair_is_made_once_and_keeps_its_fingerprint() {
    let d = temp_dir();
    let a = load_or_create(&d, &["0.0.0.0".into(), "203.0.113.5".into()]).unwrap();
    assert!(a.created);
    assert_eq!(a.fingerprint.len(), 32 * 3 - 1);
    assert!(d.join(CERT_FILE).is_file() && d.join(KEY_FILE).is_file());
    let b = load_or_create(&d, &[]).unwrap();
    assert!(!b.created);
    assert_eq!(a.fingerprint, b.fingerprint);
    // the Debug form names the file and the fingerprint, never the key
    let dbg = format!("{b:?}");
    assert!(dbg.contains(&b.fingerprint) && !dbg.contains("PRIVATE"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(d.join(KEY_FILE)).unwrap().permissions().mode();
        assert_eq!(m & 0o777, 0o600);
    }
    // one file without the other: an error, nothing overwritten
    std::fs::remove_file(d.join(KEY_FILE)).unwrap();
    assert!(load_or_create(&d, &[]).is_err());
    assert!(d.join(CERT_FILE).is_file());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn operator_pairs_by_file_name_are_used_first() {
    let d = temp_dir();
    let own = load_or_create(&d, &[]).unwrap();
    // a second pair under the Let's Encrypt names
    let other = temp_dir();
    let o = load_or_create(&other, &[]).unwrap();
    std::fs::copy(other.join(CERT_FILE), d.join(LE_CERT_FILE)).unwrap();
    std::fs::copy(other.join(KEY_FILE), d.join(LE_KEY_FILE)).unwrap();
    let used = load_or_create(&d, &[]).unwrap();
    assert_eq!(used.fingerprint, o.fingerprint);
    assert_ne!(used.fingerprint, own.fingerprint);
    assert!(used.cert_path.ends_with(LE_CERT_FILE));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn the_gate_refuses_plain_logins_from_remote_peers_of_a_public_lobby() {
    let remote: SocketAddr = "198.51.100.4:6000".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:6000".parse().unwrap();
    let mapped: SocketAddr = "[::ffff:127.0.0.1]:6000".parse().unwrap();
    // a loopback lobby: plain from anyone that can reach it
    let g = Gate::new(None, TlsMode::Auto, false, false);
    assert!(g.plain_ok(Some(remote)) && g.plain_ok(Some(local)));
    // public: remote peers must use TLS, loopback peers (the host's own client and bots) may stay plain
    let g = Gate::new(None, TlsMode::Auto, true, false);
    assert!(!g.plain_ok(Some(remote)));
    assert!(g.plain_ok(Some(local)) && g.plain_ok(Some(mapped)));
    assert!(!g.plain_ok(None));
    // the override
    let g = Gate::new(None, TlsMode::Auto, true, true);
    assert!(g.plain_ok(Some(remote)));
    // --tls loopback: TLS from everyone
    let g = Gate::new(None, TlsMode::Loopback, false, false);
    assert!(!g.plain_ok(Some(local)));
    assert!(is_loopback_bind("127.0.0.1") && is_loopback_bind("localhost") && is_loopback_bind("[::1]"));
    assert!(!is_loopback_bind("0.0.0.0") && !is_loopback_bind("192.168.1.5"));
    assert_eq!(TlsMode::parse("loopback"), Ok(TlsMode::Loopback));
    assert!(TlsMode::parse("sometimes").is_err());
    // the frames of the 2009 auth port never look like a handshake
    assert!(!looks_like_tls(&[0x06, 0x02]) && !looks_like_tls(&[0x00, 0x02]));
    assert!(looks_like_tls(&[0x16, 0x03]));
}

#[test]
fn the_flag_parses_and_a_bad_mode_is_a_config_error() {
    let o = crate::cli::parse_full(["t", "--tls", "loopback", "--plain-auth-public"], &[])
        .ok()
        .unwrap()
        .opts;
    assert_eq!(o.tls, "loopback");
    assert!(o.plain_auth_public);
    assert!(crate::cli::parse_full(["t", "--tls", "maybe"], &[]).is_err());
}

fn auth_lobby(mode: TlsMode) -> (u16, String, PathBuf) {
    let d = temp_dir();
    let id = Arc::new(load_or_create(&d, &[]).unwrap());
    let fp = id.fingerprint.clone();
    let mut st = state();
    st.opts.pw_check = true;
    st.opts.autochar = true;
    st.scope(|st| {
        crate::auth::ensure_account(st, "alice", "s3cret-pw")?;
        Ok(())
    })
    .unwrap();
    let shared: Shared = Arc::new(std::sync::Mutex::new(st));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let gate = Gate::new(Some(id), mode, false, false);
    crate::net::spawn_listener_tls(shared, l, Listener::Auth, Some(Arc::new(gate)));
    (port, fp, d)
}

#[test]
fn a_login_over_tls_on_the_auth_port() {
    let (port, fp, d) = auth_lobby(TlsMode::Loopback);
    let mut c = tls_client(port, &fp);
    c.write_all(&login_frame("alice", "s3cret-pw")).unwrap();
    let r = reply_of(&mut c, LOGIN_REPLY).expect("0x201 over TLS");
    assert_eq!(u32_at(&r, 6), 1, "the right password logs in");
    assert!(c.conn.protocol_version() == Some(rustls::ProtocolVersion::TLSv1_3));
    // a wrong password over TLS is the normal refusal
    let mut c = tls_client(port, &fp);
    c.write_all(&login_frame("alice", "wrong")).unwrap();
    let r = reply_of(&mut c, LOGIN_REPLY).expect("0x201 over TLS");
    assert_eq!(u32_at(&r, 6), 0);
    // a client pinning another fingerprint never gets to send anything
    let mut c = tls_client(port, "00:11");
    assert!(c.write_all(&login_frame("alice", "s3cret-pw")).and_then(|_| c.flush()).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn plain_is_refused_where_tls_is_required_and_kept_on_a_loopback_lobby() {
    // --tls loopback: a plain login is closed before any answer
    let (port, _, d) = auth_lobby(TlsMode::Loopback);
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = s.write_all(&login_frame("alice", "s3cret-pw"));
    assert!(reply_of(&mut s, LOGIN_REPLY).is_none());
    let _ = std::fs::remove_dir_all(&d);
    // auto on loopback: plain (the 2009 client, loopback bots) and TLS both work
    let (port, fp, d) = auth_lobby(TlsMode::Auto);
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(&login_frame("alice", "s3cret-pw")).unwrap();
    assert_eq!(u32_at(&reply_of(&mut s, LOGIN_REPLY).unwrap(), 6), 1);
    let mut c = tls_client(port, &fp);
    c.write_all(&login_frame("alice", "s3cret-pw")).unwrap();
    assert_eq!(u32_at(&reply_of(&mut c, LOGIN_REPLY).unwrap(), 6), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_web_port_serves_tls_and_redirects_plain_requests() {
    let d = temp_dir();
    let id = Arc::new(load_or_create(&d, &[]).unwrap());
    let fp = id.fingerprint.clone();
    let shared: Shared = Arc::new(std::sync::Mutex::new(state()));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let gate = Gate::new(Some(id), TlsMode::Loopback, false, false);
    crate::clanweb::spawn_with_tls(shared, l, crate::hosting::Hosting::none(), Some(Arc::new(gate)));
    let mut c = tls_client(port, &fp);
    write!(c, "GET /missing HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
    let mut out = Vec::new();
    let _ = c.read_to_end(&mut out);
    let out = String::from_utf8_lossy(&out);
    assert!(out.starts_with("HTTP/1.1 404"), "{out}");
    // plain GET: a redirect to the same address over https; a plain POST is refused
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET /register?x=1 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 301"), "{out}");
    assert!(out.contains(&format!("Location: https://127.0.0.1:{port}/register?x=1\r\n")), "{out}");
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "POST /register HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 3\r\n\r\na=b").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 403"), "{out}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_client_that_trickles_bytes_is_dropped_at_the_overall_deadline() {
    let d = temp_dir();
    let id = Arc::new(load_or_create(&d, &[]).unwrap());
    let gate = Gate::new(Some(id), TlsMode::Auto, false, false);
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let t0 = std::time::Instant::now();
        let r = admit_within(s, &gate, Duration::from_millis(700), None);
        let _ = tx.send((matches!(r, Admitted::Failed(_)), t0.elapsed()));
    });
    // the TLS prefix, then one byte every 150 ms (the real limit is 10 s against one byte every 2 s)
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.write_all(&[0x16, 0x03]).unwrap();
    let t = std::thread::spawn(move || {
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(150));
            if c.write_all(&[0x01]).is_err() {
                break;
            }
        }
    });
    let (failed, took) = rx.recv_timeout(Duration::from_secs(5)).expect("admit returned");
    assert!(failed, "a trickling handshake fails");
    assert!(took >= Duration::from_millis(600) && took < Duration::from_millis(2500), "{took:?}");
    let _ = t.join();
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_handshake_cap_closes_the_next_pending_connection_and_frees_places() {
    // the counter itself
    let slots = Slots::default();
    let held: Vec<_> = (0..MAX_HANDSHAKES).map(|_| slots.take(MAX_HANDSHAKES).unwrap()).collect();
    assert!(slots.take(MAX_HANDSHAKES).is_none(), "the 65th is refused");
    drop(held);
    assert!(slots.take(MAX_HANDSHAKES).is_some(), "places are freed");
    // a listener with a cap of 3: three silent sockets hold their places, the fourth is closed at once
    let d = temp_dir();
    let id = Arc::new(load_or_create(&d, &[]).unwrap());
    let shared: Shared = Arc::new(std::sync::Mutex::new(state()));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let gate = Gate::new(Some(id), TlsMode::Auto, false, false);
    crate::net::spawn_listener_capped(Arc::clone(&shared), l, Listener::Auth, Some(Arc::new(gate)), 3);
    let idle: Vec<TcpStream> = (0..3).map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap()).collect();
    std::thread::sleep(Duration::from_millis(300));
    let mut fourth = TcpStream::connect(("127.0.0.1", port)).unwrap();
    fourth.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut b = [0u8; 1];
    // closed by the server: end of stream (or a reset), not a timeout
    match fourth.read(&mut b) {
        Ok(0) => {}
        Err(e) if e.kind() != io::ErrorKind::WouldBlock && e.kind() != io::ErrorKind::TimedOut => {}
        o => panic!("the fourth connection stayed open: {o:?}"),
    }
    drop(idle);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_new_pair_leaves_no_temporary_files_and_the_proxy_note_is_logged_for_public_lobbies() {
    let d = temp_dir();
    let id = load_or_create(&d, &[]).unwrap();
    assert!(d.join(CERT_FILE).is_file() && d.join(KEY_FILE).is_file());
    assert!(!d.join(format!("{CERT_FILE}.tmp")).exists() && !d.join(format!("{KEY_FILE}.tmp")).exists());
    let public = Gate::new(None, TlsMode::Auto, true, false);
    let lines = startup_lines(&id, &public);
    assert!(lines.iter().any(|l| l.contains("--tls loopback") && l.contains("proxy")), "{lines:?}");
    let strict = Gate::new(None, TlsMode::Loopback, true, false);
    assert!(!startup_lines(&id, &strict).iter().any(|l| l.contains("proxy")));
    let local = Gate::new(None, TlsMode::Auto, false, false);
    assert!(!startup_lines(&id, &local).iter().any(|l| l.contains("proxy")));
    let _ = std::fs::remove_dir_all(&d);
}

/// LG-300: the host port speaks TLS only, with the lobby's own certificate; a plain hello gets no answer.
#[test]
fn the_host_port_is_tls_only_and_greets_a_known_agent() {
    let d = temp_dir();
    let id = Arc::new(load_or_create(&d, &[]).unwrap());
    let fp = id.fingerprint.clone();
    let mut st = state();
    st.opts.host_agents = vec![dc_config::HostAgentCfg {
        name: "us-west".into(),
        token: "0123456789abcdef01234567".into(),
        public_address: String::new(),
    }];
    let shared: Shared = Arc::new(std::sync::Mutex::new(st));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    crate::hosts::spawn_listener(
        Arc::clone(&shared),
        l,
        Arc::new(crate::hosts::gate(Arc::clone(&id))),
    );
    let hello =
        b"hello name=us-west token=0123456789abcdef01234567 version=t ports=1-2 running=0\n";
    // plain, even from loopback: closed without an answer
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = s.write_all(hello);
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    assert!(buf.is_empty(), "no answer to a plain hello: {buf:?}");
    // TLS with the pinned certificate: ok
    let mut c = tls_client(port, &fp);
    c.write_all(hello).unwrap();
    c.flush().unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(&mut c), &mut line).unwrap();
    assert!(line.starts_with("ok name=us-west lobby="), "{line:?}");
    assert!(crate::net::lock(&shared)
        .hosts
        .online
        .contains_key("us-west"));
    let _ = std::fs::remove_dir_all(&d);
}
