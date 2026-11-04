//! The client's trust rules (CLI-1470..1475) against a small TLS echo server of the test's own: a pinned
//! fingerprint, a wrong pin, trust on first use and a changed certificate, the plain fallback on loopback, and the
//! framed `TcpLink` over TLS.

use super::*;
use crate::lobby::TcpLink;
use crate::record::Recorder;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::PrivateKeyDer;
use rustls::{ServerConfig, ServerConnection};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU32, Ordering};

/// A TLS server on 127.0.0.1 that echoes what it reads; returns its address and fingerprint.
fn mirror_server() -> (SocketAddr, String) {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let fp = fingerprint(ck.cert.der().as_ref());
    let key = PrivateKeyDer::from_pem_slice(ck.key_pair.serialize_pem().as_bytes()).unwrap();
    let cfg = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![ck.cert.der().clone()], key)
        .unwrap();
    let cfg = Arc::new(cfg);
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let cfg = Arc::clone(&cfg);
            std::thread::spawn(move || {
                let c = ServerConnection::new(cfg).unwrap();
                let mut t = StreamOwned::new(c, s);
                let mut b = [0u8; 4096];
                while let Ok(n) = t.read(&mut b) {
                    if n == 0 || t.write_all(&b[..n]).and_then(|_| t.flush()).is_err() {
                        break;
                    }
                }
            });
        }
    });
    (addr, fp)
}

/// A plain echo server (a loopback server without TLS, like the reference client).
fn plain_server() -> SocketAddr {
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut b = [0u8; 4096];
                while let Ok(n) = s.read(&mut b) {
                    if n == 0 || s.write_all(&b[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

static N: AtomicU32 = AtomicU32::new(0);

fn temp_store() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "dc-botclient-tls-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    d.join(KNOWN_SERVERS_FILE)
}

fn echo(s: &mut ClientStream, msg: &[u8]) -> Vec<u8> {
    s.write_all(msg).unwrap();
    s.flush().unwrap();
    let _ = s.tcp().set_read_timeout(Some(Duration::from_secs(5)));
    let mut out = vec![0u8; msg.len()];
    s.read_exact(&mut out).unwrap();
    out
}

#[test]
fn fingerprints_are_read_in_every_common_spelling() {
    let hex = "ab".repeat(32);
    let want = vec!["AB"; 32].join(":");
    assert_eq!(normalize_fingerprint(&hex).unwrap(), want);
    assert_eq!(normalize_fingerprint(&want.to_lowercase()).unwrap(), want);
    assert_eq!(normalize_fingerprint(&format!("SHA256:{hex}")).unwrap(), want);
    assert_eq!(normalize_fingerprint(&format!("sha256 Fingerprint={want}")).unwrap(), want);
    assert!(normalize_fingerprint("AB:CD").is_err());
    assert!(normalize_fingerprint(&"zz".repeat(32)).is_err());
    assert!(is_trust_message(&format!("{T_CHANGED} since ...")));
    assert!(!is_trust_message("connect refused"));
}

#[test]
fn a_pinned_server_is_accepted_and_a_wrong_pin_refused() {
    let (addr, fp) = mirror_server();
    let t = Trust {
        pin: Some(fp.clone()),
        ..Trust::default()
    };
    let mut s = connect_with(addr, Duration::from_secs(5), &t).unwrap();
    assert!(s.is_tls());
    assert_eq!(echo(&mut s, b"hello"), b"hello");
    let t = Trust {
        pin: Some(normalize_fingerprint(&"00".repeat(32)).unwrap()),
        ..Trust::default()
    };
    let e = connect_with(addr, Duration::from_secs(5), &t).err().unwrap();
    assert!(e.starts_with(T_MISMATCH), "{e}");
    assert!(e.contains(&fp), "{e}");
}

#[test]
fn trust_on_first_use_remembers_and_refuses_a_changed_certificate() {
    // the verifier's non-loopback path, driven directly: the test servers live on loopback
    let store = temp_store();
    let t = Trust {
        store: Some(store.clone()),
        ..Trust::default()
    };
    let v = |fp_cert: &CertificateDer<'_>, t: &Trust| {
        let failure = Arc::new(Mutex::new(None));
        let keep = Arc::new(Mutex::new(None));
        let v = Verifier {
            trust: t.clone(),
            key: "203.0.113.5:10000".into(),
            loopback: false,
            failure: Arc::clone(&failure),
            keep: Arc::clone(&keep),
        };
        let r = v.verify_server_cert(
            fp_cert,
            &[],
            &ServerName::try_from("203.0.113.5").unwrap(),
            &[],
            UnixTime::now(),
        );
        if let Some(fp) = keep.lock().unwrap().take() {
            remember(t, "203.0.113.5:10000", &fp);
        }
        let why = failure.lock().unwrap().take();
        (r.is_ok(), why)
    };
    let a = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let b = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let (fa, fb) = (fingerprint(a.cert.der()), fingerprint(b.cert.der()));
    // first use: accepted and written to the profile file
    assert_eq!(v(a.cert.der(), &t), (true, None));
    assert_eq!(read_known(&store).unwrap().get("203.0.113.5:10000"), Some(&fa));
    // the same certificate again: accepted
    assert!(v(a.cert.der(), &t).0);
    // another certificate: refused with the identity box text, the file keeps the first one
    let (ok, why) = v(b.cert.der(), &t);
    assert!(!ok);
    let why = why.unwrap();
    assert!(why.starts_with(T_CHANGED) && why.contains("--server-fingerprint"), "{why}");
    assert!(why.contains(&fa) && why.contains(&fb));
    assert_eq!(read_known(&store).unwrap().get("203.0.113.5:10000"), Some(&fa));
    // continuing needs the pin of the new certificate, which then replaces the remembered one
    let pinned = Trust {
        pin: Some(fb.clone()),
        ..t.clone()
    };
    assert_eq!(v(b.cert.der(), &pinned), (true, None));
    assert_eq!(read_known(&store).unwrap().get("203.0.113.5:10000"), Some(&fb));
    assert!(v(b.cert.der(), &t).0);
    let _ = std::fs::remove_dir_all(store.parent().unwrap());
}

#[test]
fn a_loopback_server_without_tls_gets_plain_and_off_stays_plain() {
    let addr = plain_server();
    let t = Trust {
        loopback_handshake: Duration::from_millis(300),
        ..Trust::default()
    };
    let mut s = connect_with(addr, Duration::from_secs(5), &t).unwrap();
    assert!(!s.is_tls());
    assert_eq!(echo(&mut s, b"plain"), b"plain");
    let t = Trust {
        mode: Mode::Off,
        ..Trust::default()
    };
    let s = connect_with(addr, Duration::from_secs(5), &t).unwrap();
    assert!(!s.is_tls());
    // off never goes plain to a non-loopback address
    let e = connect_with("203.0.113.5:10000".parse().unwrap(), Duration::from_millis(50), &t)
        .err()
        .unwrap();
    assert!(e.starts_with(T_NO_TLS), "{e}");
}

#[test]
fn a_loopback_tls_server_needs_no_pin_and_tcplink_frames_over_it() {
    let (addr, _) = mirror_server();
    let rec = Recorder::null("tls-test");
    let mut link =
        TcpLink::connect_tls(addr, "auth", &rec, Duration::from_secs(5), &Trust::default()).unwrap();
    assert!(link.is_tls());
    // the echo server sends the frame back; the link cuts it like any server frame
    let mut w = dc_wire::PktWriter::new(0x206);
    w.pad(32);
    link.send(&w).unwrap();
    let d = link.recv_until(0x206, Duration::from_secs(5)).unwrap();
    assert!(d.len() >= 32);
    link.close();
}

/// Runs the verifier's trust-on-first-use path for one certificate against `t`; returns (accepted, refusal text).
fn verify_once(t: &Trust, der: &CertificateDer<'_>) -> (bool, Option<String>) {
    let failure = Arc::new(Mutex::new(None));
    let v = Verifier {
        trust: t.clone(),
        key: "203.0.113.5:10000".into(),
        loopback: false,
        failure: Arc::clone(&failure),
        keep: Arc::new(Mutex::new(None)),
    };
    let r = v.verify_server_cert(der, &[], &ServerName::try_from("203.0.113.5").unwrap(), &[], UnixTime::now());
    let why = failure.lock().unwrap().take();
    (r.is_ok(), why)
}

#[test]
fn a_malformed_or_truncated_store_refuses_instead_of_trusting_a_first_use() {
    let a = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let fa = fingerprint(a.cert.der());
    for (name, body) in [
        ("malformed", "# header
203.0.113.5:10000 not-a-fingerprint
".to_string()),
        ("single field", "203.0.113.5:10000
".to_string()),
        ("truncated", format!("203.0.113.5:10000 {}
", &fa[..40])),
    ] {
        let store = temp_store();
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        std::fs::write(&store, body).unwrap();
        let t = Trust {
            store: Some(store.clone()),
            ..Trust::default()
        };
        assert!(read_known(&store).is_err(), "{name}");
        let (ok, why) = verify_once(&t, a.cert.der());
        let why = why.unwrap_or_default();
        assert!(!ok, "{name}");
        assert!(is_trust_message(&why), "{name}: {why}");
        assert!(why.contains("saved server list is damaged: delete") && why.contains(KNOWN_SERVERS_FILE), "{why}");
        let _ = std::fs::remove_dir_all(store.parent().unwrap());
    }
    // a missing file is a first use; comments and blank lines are fine
    let store = temp_store();
    assert_eq!(read_known(&store).unwrap().len(), 0);
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    std::fs::write(&store, format!("# c

203.0.113.5:10000 {fa}
")).unwrap();
    assert_eq!(read_known(&store).unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(store.parent().unwrap());
}

#[test]
fn the_store_is_written_through_a_temporary_file_and_a_failure_is_reported() {
    let store = temp_store();
    assert_eq!(temp_path(&store).file_name().unwrap().to_str().unwrap(), "known_servers.txt.tmp");
    let fp = fingerprint(b"x");
    let mut m = BTreeMap::new();
    m.insert("h:1".to_string(), fp.clone());
    write_known(&store, &m).unwrap();
    assert!(!temp_path(&store).exists(), "the temporary file is renamed away");
    assert_eq!(read_known(&store).unwrap().get("h:1"), Some(&fp));
    // a leftover temporary file never replaces the store; the next write overwrites it
    std::fs::write(temp_path(&store), "garbage").unwrap();
    m.insert("h:2".to_string(), fp.clone());
    write_known(&store, &m).unwrap();
    assert_eq!(read_known(&store).unwrap().len(), 2);
    // an unwritable location: the parent folder is a file
    let blocker = store.parent().unwrap().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let bad = blocker.join(KNOWN_SERVERS_FILE);
    assert!(write_known(&bad, &m).is_err());
    let t = Trust {
        store: Some(bad),
        ..Trust::default()
    };
    let _ = take_store_warning();
    remember(&t, "h:3", &fp);
    let w = take_store_warning().expect("the failed save is reported");
    assert!(w.starts_with("Warning:") && w.contains("could not save"), "{w}");
    assert!(take_store_warning().is_none());
    let _ = std::fs::remove_dir_all(store.parent().unwrap());
}
