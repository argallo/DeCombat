//! CLI-1610: the token cache and the authority resolution.

use super::*;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc_identity_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sample(expires: i64) -> Cached {
    Cached {
        token: "cGF5.c2ln".into(),
        expires,
        account: 42,
        nick: "Robin".into(),
        login: "robin".into(),
        authority: "203.0.113.5:18080".into(),
    }
}

#[test]
fn the_cache_stores_loads_and_expires() {
    let d = dir("cache");
    let p = d.join(CACHE_FILE);
    assert!(load(&p, 1000).is_none(), "no file, no token");
    let c = sample(10_000);
    store(&p, &c).unwrap();
    assert_eq!(load(&p, 1000), Some(c.clone()));
    // expired, and within the margin of its expiry
    assert!(load(&p, 10_000).is_none());
    assert!(load(&p, 10_000 - EXPIRY_MARGIN).is_none());
    assert!(load(&p, 10_000 - EXPIRY_MARGIN - 1).is_some());
    // a damaged file is no token
    std::fs::write(&p, "{not json").unwrap();
    assert!(load(&p, 1000).is_none());
    store(&p, &c).unwrap();
    clear(&p);
    assert!(!p.exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_debug_form_hides_the_token() {
    let text = format!("{:?}", sample(1));
    assert!(!text.contains("cGF5"), "{text}");
    assert!(text.contains("token hidden"));
}

#[test]
fn the_default_authority_is_the_lobby_host_at_port_plus_8080_with_its_pin() {
    let lobby: SocketAddr = "203.0.113.5:10000".parse().unwrap();
    let mut t = Trust::default();
    t.pin = Some("AB:CD".into());
    let a = resolve(None, None, lobby, &t, Some(Path::new("prof")))
        .unwrap()
        .unwrap();
    assert_eq!(a.addr, "203.0.113.5:18080".parse().unwrap());
    assert!(a.derived);
    assert_eq!(a.trust.pin.as_deref(), Some("AB:CD"));
    assert_eq!(a.cache.as_deref(), Some(Path::new("prof").join(CACHE_FILE).as_path()));
    assert!(resolve(Some("off"), None, lobby, &t, None).unwrap().is_none());
    // another host does not inherit the lobby's pin
    let b = resolve(Some("198.51.100.9:18080"), None, lobby, &t, None)
        .unwrap()
        .unwrap();
    assert!(!b.derived && b.trust.pin.is_none());
    assert_eq!(b.key, "198.51.100.9:18080");
    assert!(resolve(Some("198.51.100.9:18080"), Some("zz"), lobby, &t, None).is_err());
}

#[test]
fn an_unreachable_authority_is_reported() {
    let addr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let a = Authority {
        addr,
        key: addr.to_string(),
        trust: Trust::default(),
        derived: false,
        cache: None,
    };
    assert!(matches!(request_token(&a, "robin", "pw"), Issued::Unreachable(_)));
}
