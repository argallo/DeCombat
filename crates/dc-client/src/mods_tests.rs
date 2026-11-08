//! Mod delivery without a network: a fake lobby web port on loopback (plain HTTP; the client's connector takes plain
//! on loopback) serves `server_list.json` and the packages.

use super::*;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Instant;

type Routes = Arc<Mutex<BTreeMap<String, (u16, Vec<u8>)>>>;

/// A web port answering GETs from `routes`; a TLS hello is closed at once (the client then speaks plain).
fn fake_lobby(routes: Routes) -> SocketAddr {
    fake_lobby_slow(routes, Duration::ZERO)
}

/// [`fake_lobby`] whose every reply waits `delay` first (a stand-in for the round trips of a far server).
fn fake_lobby_slow(routes: Routes, delay: Duration) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let routes = Arc::clone(&routes);
            std::thread::spawn(move || {
                let mut first = [0u8; 1];
                if s.peek(&mut first).ok() != Some(1) || first[0] == 0x16 {
                    return;
                }
                let mut buf = Vec::new();
                let mut c = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut c) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&c[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).into_owned();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, body) = routes
                    .lock()
                    .unwrap()
                    .get(&path)
                    .cloned()
                    .unwrap_or((404, b"No such package.".to_vec()));
                std::thread::sleep(delay);
                let _ = s.write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                );
                let _ = s.write_all(&body);
            });
        }
    });
    addr
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc_client_mods_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A tiny mod folder.
fn mod_dir(root: &Path, id: &str, kind: &str) -> PathBuf {
    let d = root.join(id);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("manifest.json"),
        format!(
            r#"{{"id": "{id}", "name": "T", "version": "1.0.0", "entry": "main.lua", "kind": "{kind}", "capabilities": ["content.text"], "description": "A test mod for the client's mod delivery.", "author": "tests", "screenshots": ["screenshots/shot.png"]}}"#
        ),
    )
    .unwrap();
    // a PNG signature and four more bytes: the lint checks the signature
    std::fs::create_dir_all(d.join("screenshots")).unwrap();
    std::fs::write(
        d.join("screenshots").join("shot.png"),
        [0x89u8, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 0],
    )
    .unwrap();
    std::fs::write(
        d.join("main.lua"),
        "return function(mod) mod.content.text:override(\"IDS_T\", \"t\") end",
    )
    .unwrap();
    d
}

fn entry_json(e: &dc_modkit::ModSetEntry) -> serde_json::Value {
    serde_json::json!({
        "id": e.id, "version": e.version.to_string(), "sha256": e.sha256, "kind": e.kind.as_str(),
        "requires_client": e.requires_client, "capabilities": e.capabilities,
    })
}

/// The server-list document as the lobby writes it (modset::server_list_json): fragment plus hash.
fn server_list(mods: &[serde_json::Value], cv: bool, hash: &str) -> Vec<u8> {
    serde_json::json!({
        "name": "t",
        "mod_set": {"policy": "modded", "client_visual_allowed": cv, "mods": mods},
        "mod_set_hash": hash,
    })
    .to_string()
    .into_bytes()
}

fn cfg(data: &Path) -> ModConfig {
    ModConfig {
        data_dir: Some(data.to_path_buf()),
        ..ModConfig::default()
    }
}

#[test]
fn a_set_of_one_mod_downloads_verifies_unpacks_loads_and_hashes_like_the_lobby() {
    let root = temp("one");
    let (zip, e) =
        dc_modkit::pack_mod_bytes(&mod_dir(&root.join("src"), "tmod", "shared")).unwrap();
    assert!(e.requires_client);
    // the lobby's reference: SHA-256 over `id@version:sha256\n` (HOSTING_AND_MODS.md §4 item 6)
    let reference = dc_rom::sha256_hex(format!("tmod@1.0.0:{}\n", e.sha256).as_bytes());
    let routes: Routes = Arc::default();
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, server_list(&[entry_json(&e)], false, &reference)),
    );
    routes
        .lock()
        .unwrap()
        .insert(format!("/mods/{}.zip", e.sha256), (200, zip.clone()));
    let web = fake_lobby(Arc::clone(&routes));
    let data = root.join("data");
    let mut cm = ClientMods::new();
    let mut seen = Vec::new();
    let h = cm
        .join(Some(web), None, &cfg(&data), &mut |s| seen.push(s))
        .unwrap();
    assert_eq!(h, reference);
    assert_eq!(cm.status.hash.as_deref(), Some(reference.as_str()));
    assert_eq!(cm.status.state, ModState::Ready);
    assert_eq!(cm.loaded(), vec!["tmod".to_string()]);
    // CLI-1619: the recipe rebuilds the same host (a server mod stays a server mod)
    let r = cm
        .status
        .recipe
        .clone()
        .expect("a recipe after a modded join");
    assert_eq!(r.mods.len(), 1);
    assert_eq!(
        (r.mods[0].id.as_str(), r.mods[0].origin),
        ("tmod", ModOrigin::Server)
    );
    assert_eq!(r.load().unwrap().load_order(), cm.loaded());
    let paths = ModPaths::new(&data);
    assert_eq!(std::fs::read(paths.zip(&e.sha256)).unwrap(), zip);
    assert!(paths.unpacked(&e.sha256).join("manifest.json").is_file());
    assert!(seen.contains(&ModState::FetchingSet) && seen.contains(&ModState::Loading));
    assert!(seen.iter().any(|s| matches!(
        s,
        ModState::Downloading {
            index: 1,
            count: 1,
            ..
        }
    )));
    // a second join takes the cached package (the server no longer has it)
    routes
        .lock()
        .unwrap()
        .remove(&format!("/mods/{}.zip", e.sha256));
    assert_eq!(
        cm.join(Some(web), None, &cfg(&data), &mut |_| {}).unwrap(),
        reference
    );
    // a server_rules mod is declared (and hashed) but never fetched by the client
    let mut rules = entry_json(&e);
    rules["id"] = "rules".into();
    rules["kind"] = "server_rules".into();
    rules["requires_client"] = false.into();
    rules["sha256"] = "ab".repeat(32).into();
    let both = [rules, entry_json(&e)];
    let h2 = dc_rom::sha256_hex(
        format!("rules@1.0.0:{}\ntmod@1.0.0:{}\n", "ab".repeat(32), e.sha256).as_bytes(),
    );
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, server_list(&both, false, &h2)),
    );
    assert_eq!(
        cm.join(Some(web), None, &cfg(&data), &mut |_| {}).unwrap(),
        h2
    );
    assert_eq!(cm.loaded(), vec!["tmod".to_string()]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_byte_swapped_package_is_refused_and_nothing_is_unpacked() {
    let root = temp("swap");
    let (mut zip, e) =
        dc_modkit::pack_mod_bytes(&mod_dir(&root.join("src"), "tmod", "shared")).unwrap();
    let n = zip.len();
    zip.swap(n / 2, n / 2 + 1);
    zip[n / 2] ^= 0x55;
    let routes: Routes = Arc::default();
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, server_list(&[entry_json(&e)], false, &"0".repeat(64))),
    );
    routes
        .lock()
        .unwrap()
        .insert(format!("/mods/{}.zip", e.sha256), (200, zip));
    let web = fake_lobby(routes.clone());
    let data = root.join("data");
    let mut cm = ClientMods::new();
    // a declared hash that disagrees with the list is refused first
    let err = cm
        .join(Some(web), None, &cfg(&data), &mut |_| {})
        .unwrap_err();
    assert!(err.contains("mod-set hash"), "{err}");
    let set_hash = ServerModSet::from_fragment(
        &serde_json::json!({"policy": "modded", "mods": [entry_json(&e)]}),
    )
    .unwrap()
    .hash();
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, server_list(&[entry_json(&e)], false, &set_hash)),
    );
    let err = cm
        .join(Some(web), None, &cfg(&data), &mut |_| {})
        .unwrap_err();
    assert!(err.contains("join refused"), "{err}");
    assert!(matches!(cm.status.state, ModState::Failed(_)));
    assert!(cm.status.hash.is_none() && cm.loaded().is_empty());
    let paths = ModPaths::new(&data);
    assert!(!paths.zip(&e.sha256).exists());
    assert!(!paths.unpacked(&e.sha256).exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_package_with_a_parent_path_is_refused() {
    let zip = dc_modkit::write_deterministic_zip(&[
        ("manifest.json".into(), b"{}".to_vec()),
        ("../x".into(), b"escape".to_vec()),
    ])
    .unwrap();
    let err = read_package(&zip, 1 << 20).unwrap_err();
    assert!(err.contains("unsafe entry name '../x'"), "{err}");
    let root = temp("slip");
    let dest = root.join("unpacked").join("m");
    assert!(unpack_package(&zip, &dest, 1 << 20).is_err());
    assert!(
        !dest.exists() && !root.join("unpacked").join("x").exists() && !root.join("x").exists()
    );
    for bad in ["/abs", "a/../b", "C:/x", "a\\b", "./a", "a//b", ""] {
        assert!(!safe_entry_name(bad), "{bad}");
    }
    for ok in ["manifest.json", "assets/a.dtx", "assets/"] {
        assert!(safe_entry_name(ok), "{ok}");
    }
    // the same refusal through a join: the hash matches, the unpack refuses
    let sha = dc_rom::sha256_hex(&zip);
    let m = serde_json::json!({"id": "slip", "version": "1.0.0", "sha256": sha, "kind": "shared",
        "requires_client": true, "capabilities": []});
    let set =
        ServerModSet::from_fragment(&serde_json::json!({"policy": "modded", "mods": [m.clone()]}))
            .unwrap();
    let routes: Routes = Arc::default();
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, server_list(&[m], false, &set.hash())),
    );
    routes
        .lock()
        .unwrap()
        .insert(format!("/mods/{sha}.zip"), (200, zip));
    let web = fake_lobby(routes);
    let data = root.join("data");
    let err = ClientMods::new()
        .join(Some(web), None, &cfg(&data), &mut |_| {})
        .unwrap_err();
    assert!(err.contains("unsafe entry name"), "{err}");
    assert!(!ModPaths::new(&data).unpacked(&sha).exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn vanilla_unloads_a_loaded_local_mod_and_yields_the_empty_set_hash() {
    let root = temp("vanilla");
    let data = root.join("data");
    let paths = ModPaths::new(&data);
    mod_dir(&paths.local(), "green", "client_visual");
    mod_dir(&paths.local(), "sneaky", "shared");
    let routes: Routes = Arc::default();
    let empty_modded = ServerModSet {
        policy: Policy::Modded {
            client_visual_allowed: true,
        },
        mods: Vec::new(),
        community_map_base: None,
        client_fov: None,
    };
    assert_eq!(empty_modded.hash(), EMPTY_SET_HASH);
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, server_list(&[], true, EMPTY_SET_HASH)),
    );
    let web = fake_lobby(Arc::clone(&routes));
    let mut cm = ClientMods::new();
    cm.join(Some(web), None, &cfg(&data), &mut |_| {}).unwrap();
    assert_eq!(cm.loaded(), vec!["green".to_string()]);
    // CLI-1619: the local mod stays local in the recipe
    let r = cm.host_recipe().unwrap();
    assert_eq!(r.mods.len(), 1);
    assert_eq!(
        (r.mods[0].id.as_str(), r.mods[0].origin),
        ("green", ModOrigin::Local)
    );
    assert_eq!(r.load().unwrap().load_order(), vec!["green".to_string()]);
    // the shared mod is refused as a local mod, with the host's message
    assert_eq!(cm.status.local_refused.len(), 1);
    assert!(
        cm.status.local_refused[0].contains("must be kind 'client_visual'"),
        "{:?}",
        cm.status.local_refused
    );
    // the same lobby turns vanilla
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (
            200,
            br#"{"mod_set": {"policy": "vanilla", "client_visual_allowed": false, "mods": []}}"#
                .to_vec(),
        ),
    );
    let h = cm.join(Some(web), None, &cfg(&data), &mut |_| {}).unwrap();
    assert_eq!(h, EMPTY_SET_HASH);
    assert!(cm.loaded().is_empty() && cm.host().is_none());
    // vanilla: no recipe, so the match builds no host and installs nothing
    assert!(cm.host_recipe().is_none() && cm.status.recipe.is_none());
    assert!(crate::modassets::MatchMods::open(cm.status.recipe.as_ref()).is_none());
    // a lobby without a server list (404) and an unreachable web port with no entry or a plain vanilla entry: vanilla
    routes.lock().unwrap().clear();
    assert_eq!(
        cm.join(Some(web), None, &cfg(&data), &mut |_| {}).unwrap(),
        EMPTY_SET_HASH
    );
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let vanilla = ServerModSet::vanilla();
    assert_eq!(
        cm.join(Some(closed), Some(&vanilla), &cfg(&data), &mut |_| {})
            .unwrap(),
        EMPTY_SET_HASH
    );
    assert_eq!(
        cm.join(None, None, &ModConfig::default(), &mut |_| {})
            .unwrap(),
        EMPTY_SET_HASH
    );
    // a modded entry with the web port unreachable fails
    assert!(cm
        .join(None, Some(&empty_modded), &cfg(&data), &mut |_| {})
        .is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_cache_is_trimmed_oldest_first_and_keeps_the_current_set() {
    let root = temp("lru");
    let paths = ModPaths::new(&root);
    std::fs::create_dir_all(paths.by_hash()).unwrap();
    let now = SystemTime::now();
    let mut shas = Vec::new();
    for i in 0..4u8 {
        let body = vec![i; 100];
        let sha = dc_rom::sha256_hex(&body);
        std::fs::write(paths.zip(&sha), &body).unwrap();
        std::fs::create_dir_all(paths.unpacked(&sha)).unwrap();
        let f = std::fs::OpenOptions::new()
            .append(true)
            .open(paths.zip(&sha))
            .unwrap();
        f.set_modified(now - Duration::from_secs(1000 - i as u64 * 100))
            .unwrap();
        shas.push(sha);
    }
    // a stray file is left alone
    std::fs::write(paths.by_hash().join("notes.txt"), b"x").unwrap();
    // 400 bytes, cap 200: the two oldest go, but the oldest is in use and stays
    let removed = trim_cache(&paths, 200, &[&shas[0]]);
    assert_eq!(removed, vec![shas[1].clone(), shas[2].clone()]);
    assert!(paths.zip(&shas[0]).is_file() && paths.zip(&shas[3]).is_file());
    assert!(!paths.unpacked(&shas[1]).exists() && paths.unpacked(&shas[3]).exists());
    assert!(paths.by_hash().join("notes.txt").is_file());
    assert!(trim_cache(&paths, 1000, &[]).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn replies_over_the_cap_are_refused() {
    let mut r: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789";
    assert!(read_reply(
        &mut r,
        9,
        Instant::now() + Duration::from_secs(5),
        &mut |_, _| {}
    )
    .unwrap_err()
    .contains("exceeds"));
    let mut r: &[u8] = b"HTTP/1.1 200 OK\r\n\r\n0123456789";
    assert!(read_reply(
        &mut r,
        9,
        Instant::now() + Duration::from_secs(5),
        &mut |_, _| {}
    )
    .is_err());
    let mut r: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabcd";
    assert_eq!(
        read_reply(
            &mut r,
            9,
            Instant::now() + Duration::from_secs(5),
            &mut |_, _| {}
        )
        .unwrap()
        .body,
        b"abcd"
    );
}

/// An auth port P with a fake web port at P + 8080 (the client's default), retried until both bind. P is drawn from
/// 20000..40000 (a clock-, pid- and call-seeded start): port 0 hands out the dynamic range above 49152, parts of
/// which Windows reserves, and on a CI runner 50 draws of that range found no pair at all (windows-i686).
fn auth_and_web(routes: Routes) -> (TcpListener, SocketAddr) {
    static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        ^ std::process::id().wrapping_mul(2_654_435_761)
        ^ call.wrapping_mul(40_503);
    let start = (seed % 20000) as u16;
    for i in 0..20000u16 {
        let p = 20000 + (start.wrapping_add(i * 7)) % 20000;
        let wp = p + 8080;
        let Ok(a) = TcpListener::bind(("127.0.0.1", p)) else {
            continue;
        };
        let Ok(w) = TcpListener::bind(("127.0.0.1", wp)) else {
            continue;
        };
        drop(w);
        fake_lobby_on(routes.clone(), wp);
        return (a, SocketAddr::from(([127, 0, 0, 1], p)));
    }
    panic!("no port pair");
}

fn fake_lobby_on(routes: Routes, port: u16) -> SocketAddr {
    // same server as fake_lobby, on a given port
    let l = TcpListener::bind(("127.0.0.1", port)).unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut first = [0u8; 1];
            if s.peek(&mut first).ok() != Some(1) || first[0] == 0x16 {
                continue;
            }
            let mut buf = Vec::new();
            let mut c = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut c) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&c[..n]),
                }
            }
            let head = String::from_utf8_lossy(&buf).into_owned();
            let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (status, body) = routes
                .lock()
                .unwrap()
                .get(&path)
                .cloned()
                .unwrap_or((404, Vec::new()));
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = s.write_all(&body);
        }
    });
    addr
}

#[test]
fn a_failed_mod_step_keeps_the_reason_and_sends_no_login() {
    use crate::menu::link::{Link, LinkCmd, LinkEvent, LinkOptions};
    let root = temp("link");
    let routes: Routes = Arc::default();
    // a modded set whose package the server does not have
    let m = serde_json::json!({"id": "gone", "version": "1.0.0", "sha256": "cd".repeat(32), "kind": "shared",
        "requires_client": true, "capabilities": []});
    let set =
        ServerModSet::from_fragment(&serde_json::json!({"policy": "modded", "mods": [m.clone()]}))
            .unwrap();
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, server_list(&[m], false, &set.hash())),
    );
    let (auth_l, auth) = auth_and_web(Arc::clone(&routes));
    auth_l.set_nonblocking(true).unwrap();
    let mut opts = LinkOptions::new("mods-test");
    opts.mods.data_dir = Some(root.clone());
    let link = Link::start(opts);
    let login = || LinkCmd::Login {
        auth,
        user: "u".into(),
        password: "p".into(),
        nick: "n".into(),
    };
    link.send(login());
    let wait_refusal = |link: &Link| -> String {
        let t = std::time::Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            for e in link.poll() {
                if let LinkEvent::LoginRefused { text, .. } = e {
                    return text;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no refusal");
    };
    let text = wait_refusal(&link);
    assert!(text.contains("Cannot load the server's mods"), "{text}");
    assert!(
        matches!(link.join_report().state, ModState::Failed(ref r) if r.contains("404")),
        "{:?}",
        link.join_report()
    );
    assert!(
        auth_l.accept().is_err(),
        "the login was attempted after a failed mod step"
    );
    // the same lobby turned vanilla: the mod step passes and the login goes to the auth port
    routes.lock().unwrap().clear();
    link.send(login());
    let t = std::time::Instant::now();
    let accepted = loop {
        if auth_l.accept().is_ok() {
            break true;
        }
        if t.elapsed() > Duration::from_secs(20) {
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(accepted, "no login after a vanilla mod step");
    let st = link.join_report();
    assert_eq!(
        (st.state, st.hash.as_deref()),
        (ModState::Ready, Some(EMPTY_SET_HASH))
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn local_mods_load_only_when_the_player_enabled_them() {
    // CLI-1617: the Mods screen's enable list; None = every local mod (scripted runs)
    let root = temp("enabled");
    mod_dir(&root, "lmod", "client_visual");
    let load = |enabled: Option<&[String]>| {
        let mut host = ModHost::new();
        host.set_policy(ModPolicy::Modded {
            client_visual_allowed: true,
        });
        let refused = load_local(&mut host, &root, enabled);
        (host.load_order(), refused)
    };
    assert_eq!(load(None).0, vec!["lmod".to_string()]);
    assert_eq!(load(Some(&[])).0, Vec::<String>::new());
    assert_eq!(
        load(Some(&["lmod".to_string()])).0,
        vec!["lmod".to_string()]
    );
    assert_eq!(load(Some(&["other".to_string()])).0, Vec::<String>::new());
    let _ = std::fs::remove_dir_all(&root);
}

/// CLI-1611 / LG-271: a set whose Lua patches a weapon yields that content (hash for the login, not `0`), and the
/// match's weapon table gets exactly it; vanilla yields none and applies nothing.
#[test]
fn the_set_content_comes_from_the_hosts_patches_and_reaches_the_weapon_table() {
    let root = temp("content");
    let src = mod_dir(&root.join("src"), "wmod", "shared");
    let man = std::fs::read_to_string(src.join("manifest.json"))
        .unwrap()
        .replace(r#"["content.text"]"#, r#"["content.weapons"]"#);
    std::fs::write(src.join("manifest.json"), man).unwrap();
    std::fs::write(
        src.join("main.lua"),
        "return function(mod) mod.content.weapons:patch(3, { ShotsPerClip = 7 }) end",
    )
    .unwrap();
    let (zip, e) = dc_modkit::pack_mod_bytes(&src).unwrap();
    let reference = dc_rom::sha256_hex(format!("wmod@1.0.0:{}\n", e.sha256).as_bytes());
    let routes: Routes = Arc::default();
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, server_list(&[entry_json(&e)], false, &reference)),
    );
    routes
        .lock()
        .unwrap()
        .insert(format!("/mods/{}.zip", e.sha256), (200, zip));
    let web = fake_lobby(Arc::clone(&routes));
    let mut cm = ClientMods::new();
    cm.join(Some(web), None, &cfg(&root.join("data")), &mut |_| {})
        .unwrap();
    let content = cm.status.content.clone();
    assert_eq!(
        content,
        set_content(cm.host()).unwrap(),
        "the host's own patches"
    );
    assert_eq!(content.weapons.len(), 1);
    assert_eq!(content_hash(&content), content.hash());
    assert_ne!(content_hash(&content), "0");

    let text = [
        "[Weapon3]",
        "ShotsPerClip = 3",
        "[Weapon5]",
        "ShotsPerClip = 4",
    ]
    .join("\n");
    let mut table = crate::arms::WeaponTable::from_doc(&dc_bute::ButeDoc::parse(&text));
    assert_eq!(
        crate::net::apply_set_content(&mut table, Some(&content)),
        Ok(true)
    );
    assert_eq!(
        (table.rows[&3].clip, table.rows[&5].clip),
        (Some(7), Some(4))
    );

    // vanilla: no host, no content, `0` at login, the table stays the ROM's
    routes
        .lock()
        .unwrap()
        .insert("/server_list.json".into(), (404, Vec::new()));
    cm.join(Some(web), None, &cfg(&root.join("data")), &mut |_| {})
        .unwrap();
    assert!(cm.host().is_none() && cm.status.content.is_empty());
    assert_eq!(content_hash(&cm.status.content), "0");
    let mut table = crate::arms::WeaponTable::from_doc(&dc_bute::ButeDoc::parse(&text));
    assert_eq!(
        crate::net::apply_set_content(&mut table, Some(&cm.status.content)),
        Ok(false)
    );
    assert_eq!(crate::net::apply_set_content(&mut table, None), Ok(false));
    assert_eq!(table.rows[&3].clip, Some(3));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_dribbling_server_hits_the_total_deadline() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let mut first = [0u8; 1];
            if c.peek(&mut first).ok() != Some(1) || first[0] == 0x16 {
                continue;
            }
            let mut b = [0u8; 512];
            let _ = c.read(&mut b);
            let _ = c.write_all(
                b"HTTP/1.1 200 OK
Content-Length: 1000

",
            );
            for _ in 0..100 {
                if c.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            break;
        }
    });
    let t = Instant::now();
    let e = web_get(
        addr,
        "/x",
        1 << 20,
        Duration::from_secs(3),
        Duration::from_millis(400),
        &mut |_, _| {},
    )
    .unwrap_err();
    assert!(e.contains("too long"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(3));
}

#[test]
fn reserved_and_trailing_dot_names_are_unsafe() {
    for n in [
        "con",
        "a/NUL.txt",
        "Com1.lua",
        "lpt9",
        "x/aux.",
        "a.",
        "b /c",
        "a /b",
    ] {
        assert!(!safe_entry_name(n), "{n}");
    }
    for n in ["com0", "console.lua", "a/b.lua", "lpt10", "dir/"] {
        assert!(safe_entry_name(n), "{n}");
    }
}

#[test]
fn a_host_recipe_round_trips_order_origins_and_policy() {
    let root = temp("recipe");
    let srv = mod_dir(&root.join("srv"), "shared_one", "shared");
    let loc = mod_dir(&root.join("loc"), "green", "client_visual");
    let mut host = ModHost::new();
    host.set_policy(ModPolicy::Modded {
        client_visual_allowed: true,
    });
    host.load_mod_dir_as(ModOrigin::Server, &srv).unwrap();
    host.load_mod_dir_as(ModOrigin::Local, &loc).unwrap();
    let r = HostRecipe::of(&host, |id| {
        if id == "shared_one" {
            ModOrigin::Server
        } else {
            ModOrigin::Local
        }
    });
    assert_eq!(
        r.mods
            .iter()
            .map(|m| (m.id.as_str(), m.origin, m.kind.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("shared_one", ModOrigin::Server, "shared"),
            ("green", ModOrigin::Local, "client_visual")
        ]
    );
    let again = r.load().unwrap();
    assert_eq!(again.load_order(), host.load_order());
    assert_eq!(again.policy(), host.policy());
    assert_eq!(
        HostRecipe::of(&again, |id| r
            .mods
            .iter()
            .find(|m| m.id == id)
            .unwrap()
            .origin),
        r
    );
    // the origin is applied, not just recorded: the shared mod named as a local one is refused (skipped)
    let mut wrong = r.clone();
    wrong.mods[0].origin = ModOrigin::Local;
    assert_eq!(
        wrong.load().unwrap().load_order(),
        vec!["green".to_string()]
    );
    // a server folder that changed its version fails the recipe
    let mut moved = r.clone();
    moved.mods[0].version = "9.9.9".into();
    assert!(moved.load().is_err());
    let _ = std::fs::remove_dir_all(&root);
}

/// CLI-1632: a cached package `id@version` (zip bytes from `seed`, unpacked manifest); its sha256.
fn cached_pkg(paths: &ModPaths, id: &str, version: &str, seed: u8) -> String {
    let body = vec![seed; 64];
    let sha = dc_rom::sha256_hex(&body);
    std::fs::create_dir_all(paths.by_hash()).unwrap();
    std::fs::write(paths.zip(&sha), &body).unwrap();
    let dir = paths.unpacked(&sha);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{"id": "{id}", "name": "T", "version": "{version}", "entry": "main.lua", "kind": "client_visual", "capabilities": ["content.text"], "description": "A test mod for the client's mod delivery.", "author": "tests", "screenshots": ["screenshots/shot.png"]}}"#
        ),
    )
    .unwrap();
    sha
}

fn declared(mods: &[(&str, &str, &str)]) -> ServerModSet {
    ServerModSet {
        policy: Policy::Modded {
            client_visual_allowed: true,
        },
        community_map_base: None,
        client_fov: None,
        mods: mods
            .iter()
            .map(|(id, version, sha)| ServerMod {
                id: id.to_string(),
                version: version.to_string(),
                sha256: sha.to_string(),
                kind: Some("client_visual".into()),
                requires_client: true,
                capabilities: Vec::new(),
                source_repo: None,
                name: None,
                author: None,
                description: None,
                homepage: None,
                screenshots: Vec::new(),
                tags: Vec::new(),
                commands: Vec::new(),
                preview: None,
            })
            .collect(),
    }
}

#[test]
fn an_updated_mod_supersedes_its_old_packages_unless_another_server_declares_them() {
    let root = temp("prune");
    let paths = ModPaths::new(&root);
    // the user's cache of 2026-11-01: two green crosshairs, the blue one, three x-ray packages
    let green_a = cached_pkg(&paths, "green_crosshair", "1.0.0", 1);
    let green_b = cached_pkg(&paths, "green_crosshair", "1.1.0", 2);
    let blue = cached_pkg(&paths, "test_crosshair", "1.0.0", 3);
    let xray_old = cached_pkg(&paths, "hacking_mod", "1.0.0", 4);
    let xray_kept = cached_pkg(&paths, "hacking_mod", "1.0.0", 5);
    let xray_new = cached_pkg(&paths, "hacking_mod", "1.0.0", 6);
    // a local cosmetic mod of the same id is never touched
    let local = mod_dir(&paths.local(), "test_crosshair", "client_visual");
    let joined = declared(&[
        ("hacking_mod", "1.0.0", &xray_new),
        ("test_crosshair", "1.0.0", &blue),
    ]);
    let other = declared(&[("hacking_mod", "1.0.0", &xray_kept)]);
    let now = SystemTime::now();

    let removed = prune_cache(&paths, Some(&joined), &[&other], &[], PruneMode::Auto, now);
    assert_eq!(
        removed,
        vec![Pruned {
            id: "hacking_mod".into(),
            version: "1.0.0".into(),
            sha256: xray_old.clone(),
            reason: PruneReason::Superseded(xray_new.clone()),
        }]
    );
    assert_eq!(
        removed[0].log_line(),
        format!(
            "mods: cache: removed hacking_mod 1.0.0 {} (superseded by {})",
            &xray_old[..8],
            &xray_new[..8]
        )
    );
    assert!(!paths.zip(&xray_old).exists() && !paths.unpacked(&xray_old).exists());
    for sha in [&green_a, &green_b, &blue, &xray_kept, &xray_new] {
        assert!(
            paths.zip(sha).is_file() && paths.unpacked(sha).is_dir(),
            "{sha} stays"
        );
    }
    assert!(local.join("manifest.json").is_file());

    // the retired id goes once a week passed without use; the declared ones stay at any age
    let later = now + UNUSED_MAX_AGE + Duration::from_secs(3600);
    let removed = prune_cache(
        &paths,
        Some(&joined),
        &[&other],
        &[],
        PruneMode::Auto,
        later,
    );
    let mut gone: Vec<(String, PruneReason)> = removed
        .iter()
        .map(|p| (p.sha256.clone(), p.reason.clone()))
        .collect();
    gone.sort_by(|a, b| a.0.cmp(&b.0));
    let mut want = vec![
        (green_a.clone(), PruneReason::Unused),
        (green_b.clone(), PruneReason::Unused),
    ];
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(gone, want);
    assert!(removed.iter().all(|p| p.log_line().ends_with("(unused)")));
    assert!(paths.zip(&xray_kept).is_file() && paths.zip(&blue).is_file());

    // Clean up: every package no known server declares, at once; the loaded ones (keep) stay
    let green_c = cached_pkg(&paths, "green_crosshair", "1.1.0", 7);
    let removed = prune_cache(
        &paths,
        None,
        &[&joined],
        &[&xray_kept],
        PruneMode::CleanUp,
        now,
    );
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].sha256, green_c);
    let removed = prune_cache(&paths, None, &[&joined], &[], PruneMode::CleanUp, now);
    assert_eq!(
        removed.iter().map(|p| p.sha256.clone()).collect::<Vec<_>>(),
        vec![xray_kept.clone()]
    );
    assert!(paths.zip(&blue).is_file() && paths.zip(&xray_new).is_file());
    assert!(local.join("manifest.json").is_file());
    let _ = std::fs::remove_dir_all(&root);
}

/// CLI-1632: only a real answer of the row's web port is a set; nothing else reads as "declares nothing".
#[test]
fn a_rows_set_is_read_from_its_web_port_and_no_answer_is_not_a_set() {
    let set = declared(&[("hacking_mod", "1.0.0", &"ab".repeat(32))]);
    let routes: Routes = Arc::default();
    let web = fake_lobby(Arc::clone(&routes));
    let Some(auth_port) = web.port().checked_sub(crate::identity::WEB_PORT_OFFSET) else {
        return; // the system's port lies under the offset
    };
    let auth = SocketAddr::new(web.ip(), auth_port);
    // the web port answers 404 (no route): not "vanilla"
    assert!(read_row_set(auth, None).is_err());
    routes
        .lock()
        .unwrap()
        .insert("/server_list.json".into(), (200, b"{not json".to_vec()));
    assert!(read_row_set(auth, None).is_err());
    let body = serde_json::json!({"mod_set": set.to_fragment(), "mod_set_hash": set.hash()});
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, body.to_string().into_bytes()),
    );
    assert_eq!(read_row_set(auth, None).unwrap(), set);
    // nothing listens: not "vanilla"
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    assert!(read_row_set(closed, None).is_err());
}

/// A copy of the repository's optional mod in `<data>/mods/local`, as a download leaves it.
fn install_drawn_crosshair(paths: &ModPaths) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/drawn_crosshair");
    let (bytes, e) = dc_modkit::pack_mod_bytes(&src).unwrap();
    let entry = crate::modcatalog::CatalogEntry {
        id: e.id.clone(),
        version: e.version.to_string(),
        sha256: e.sha256.clone(),
        name: e.id.clone(),
        author: String::new(),
        description: String::new(),
        size: bytes.len() as u64,
        screenshot: false,
        commands: Vec::new(),
        preview: None,
    };
    crate::modcatalog::install_bytes(&bytes, paths, &entry, DOWNLOAD_MAX_BYTES).unwrap();
}

/// CLI-1634 / CLI-1609: downloaded and enabled, the mod still loads nothing on vanilla (a lobby with a catalogue is
/// still vanilla), is off until enabled, and leaves the modded login hash as it was.
#[test]
fn a_downloaded_mod_loads_only_enabled_on_modded_and_leaves_every_hash_alone() {
    let root = temp("optional");
    let data = root.join("data");
    let paths = ModPaths::new(&data);
    install_drawn_crosshair(&paths);
    let on = ModConfig {
        local_enabled: Some(vec!["drawn_crosshair".to_string()]),
        ..cfg(&data)
    };
    let off = ModConfig {
        local_enabled: Some(Vec::new()),
        ..cfg(&data)
    };
    let routes: Routes = Arc::default();
    // the same lobby offers a catalogue, whatever its policy
    routes.lock().unwrap().insert(
        "/mods/catalog.json".into(),
        (200, br#"{"version":1,"mods":[]}"#.to_vec()),
    );
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (
            200,
            serde_json::json!({"name": "v", "mod_set": {"policy": "vanilla", "client_visual_allowed": false, "mods": []},
                "mod_set_hash": EMPTY_SET_HASH})
            .to_string()
            .into_bytes(),
        ),
    );
    let web = fake_lobby(Arc::clone(&routes));
    let mut cm = ClientMods::new();
    let h = cm.join(Some(web), None, &on, &mut |_| {}).unwrap();
    assert_eq!(h, EMPTY_SET_HASH, "vanilla stays the empty set");
    assert!(
        cm.loaded().is_empty() && cm.host_recipe().is_none(),
        "vanilla loads no mods"
    );
    // modded, local mods allowed
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, server_list(&[], true, EMPTY_SET_HASH)),
    );
    let h_off = cm.join(Some(web), None, &off, &mut |_| {}).unwrap();
    assert!(cm.loaded().is_empty(), "off until enabled");
    let h_on = cm.join(Some(web), None, &on, &mut |_| {}).unwrap();
    assert_eq!(cm.loaded(), vec!["drawn_crosshair".to_string()]);
    assert_eq!(h_on, h_off, "a client-visual mod is not part of the hash");
    // modded, local mods not allowed
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, server_list(&[], false, EMPTY_SET_HASH)),
    );
    cm.join(Some(web), None, &on, &mut |_| {}).unwrap();
    assert!(cm.loaded().is_empty(), "client_visual_allowed false");
    let _ = std::fs::remove_dir_all(&root);
}

/// CLI-1633: the sandbox gives the mod its settings; its package asks for no hook and no world state.
#[test]
fn the_drawn_crosshair_sees_only_its_settings() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/drawn_crosshair");
    let m = dc_mod_lua::ModManifest::from_file(&src.join("manifest.json")).unwrap();
    assert_eq!(m.capabilities, vec!["content.hud".to_string()]);
    assert_eq!(m.kind, dc_mod_lua::ModKind::ClientVisual);
    let mut host = ModHost::new();
    host.set_policy(ModPolicy::Modded {
        client_visual_allowed: true,
    });
    host.load_mod_dir_as(ModOrigin::Local, &src).unwrap();
    assert!(!host.has_hook("hud.draw") && !host.has_hook("render.entity_visibility"));
    let ov = crate::modassets::hud_overrides(&host).unwrap();
    assert_eq!(ov.crosshair_colour, Some([0x40, 0x80, 0xff, 0xff]));
    assert_eq!(
        ov.crosshair_style,
        Some(crate::hud::crosshair::CrosshairStyle::Circle)
    );
    assert_eq!(
        ov.crosshair_recoil,
        Some(crate::hud::crosshair::RecoilMode::Dynamic)
    );
    // the defaults keep the first version's look
    assert_eq!(ov.cross_look(), crate::hud::crosshair::CrossLook::default());
}

/// MOD-27: a picture is cached only when it hashes to its name and is a PNG; a dead port or a bad answer leaves nothing.
#[test]
fn an_operators_picture_is_cached_only_when_it_is_what_the_set_says() {
    let root = std::env::temp_dir().join(format!("dc_mods_preview_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = ModPaths::new(&root);
    let good = b"\x89PNG\r\n\x1a\nimage".to_vec();
    let sha = dc_rom::sha256_hex(&good);
    let reply = |status: u16, body: &[u8]| {
        let body = body.to_vec();
        move |_: &str| {
            Ok(HttpReply {
                status,
                body: body.clone(),
            })
        }
    };
    store_preview(&paths, &sha, &mut |_| Err("dead".to_string()));
    store_preview(&paths, &sha, &mut reply(404, &good));
    store_preview(&paths, &sha, &mut reply(200, b"\x89PNG\r\n\x1a\nother"));
    let junk = b"junk".to_vec();
    store_preview(&paths, &dc_rom::sha256_hex(&junk), &mut reply(200, &junk));
    store_preview(&paths, "../evil", &mut reply(200, &good));
    assert!(
        !root.join("catalog").exists()
            || std::fs::read_dir(root.join("catalog"))
                .unwrap()
                .next()
                .is_none()
    );
    let mut asked = Vec::new();
    store_preview(&paths, &sha, &mut |t| {
        asked.push(t.to_string());
        Ok(HttpReply {
            status: 200,
            body: good.clone(),
        })
    });
    assert_eq!(asked, [format!("/mods/{sha}.png")]);
    assert_eq!(std::fs::read(paths.catalog_shot(&sha)).unwrap(), good);
    store_preview(&paths, &sha, &mut |_| {
        panic!("the cached picture is not asked for again")
    });
    let _ = std::fs::remove_dir_all(&root);
}

/// MOD-27: an old document has neither field, and a new one's fields survive the known-sets round trip and leave the
/// set's hash alone.
#[test]
fn the_set_entrys_picture_and_commands_change_no_hash() {
    let sha = "ab".repeat(32);
    let plain = serde_json::json!({"policy": "modded", "mods": [{"id": "m", "version": "1.0.0", "sha256": sha, "kind": "shared"}]});
    let old = ServerModSet::from_fragment(&plain).unwrap();
    assert_eq!(old.mods[0].preview, None);
    assert!(old.mods[0].commands.is_empty());
    let fresh = serde_json::json!({"policy": "modded", "mods": [{"id": "m", "version": "1.0.0", "sha256": sha, "kind": "shared",
        "preview": "cd".repeat(32),
        "commands": [{"command": "/go", "description": "Goes."}, {"command": "bad", "description": "dropped"}]}]});
    let new = ServerModSet::from_fragment(&fresh).unwrap();
    assert_eq!(new.mods[0].preview, Some("cd".repeat(32)));
    assert_eq!(new.mods[0].commands.len(), 1);
    assert_eq!(new.hash(), old.hash());
    assert_eq!(
        ServerModSet::from_fragment(&new.to_fragment()).unwrap(),
        new
    );
    let odd = serde_json::json!({"policy": "modded", "mods": [{"id": "m", "version": "1.0.0", "sha256": sha, "preview": "../x"}]});
    assert_eq!(
        ServerModSet::from_fragment(&odd).unwrap().mods[0].preview,
        None
    );
}

/// C6 / MAP-32: the server list's `community_map_base` is read into the set (and from there into the join
/// report); a list without it (an older lobby) leaves `None`, as does a value that is no index.
#[test]
fn the_server_lists_community_map_base_is_read_when_published() {
    let without = server_list(&[], true, EMPTY_SET_HASH);
    let set = ServerModSet::from_server_list(&without).unwrap();
    assert_eq!(set.community_map_base, None);
    let with = serde_json::json!({
        "name": "t",
        "mod_set": {"policy": "modded", "client_visual_allowed": true, "mods": []},
        "mod_set_hash": EMPTY_SET_HASH,
        "community_map_base": 12,
    })
    .to_string();
    let set = ServerModSet::from_server_list(with.as_bytes()).unwrap();
    assert_eq!(set.community_map_base, Some(12));
    assert_eq!(set.hash(), EMPTY_SET_HASH, "not part of the hash");
    let odd = with.replace("\"community_map_base\":12", "\"community_map_base\":-3");
    assert_ne!(odd, with, "the key was rewritten");
    let set = ServerModSet::from_server_list(odd.as_bytes()).unwrap();
    assert_eq!(set.community_map_base, None);
    // a vanilla list (no mod_set) may still name it: a vanilla lobby's base is its ROM map count
    let vanilla = serde_json::json!({"name": "t", "community_map_base": 19}).to_string();
    let set = ServerModSet::from_server_list(vanilla.as_bytes()).unwrap();
    assert!(set.is_plain_vanilla());
    assert_eq!(set.community_map_base, Some(19));
}

/// One row of an entry document's `maps`, in the shape the lobby writes (every field under its own name, `None` as
/// null, `goals` as pairs).
fn doc_map_row(mod_id: &str, id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "name": format!("{id} name"), "description": null,
        "world": format!("@mod:{mod_id}/assets/maps/{id}.dat"), "modes": ["TDM", "FFA"],
        "goals": [["FFA", 30], ["TDM", 50]], "round_seconds": 300,
        "picture": format!("@mod:{mod_id}/assets/pictures/{id}.dtx"), "loading": null,
        "radar": {"picture": format!("@mod:{mod_id}/assets/pictures/{id}_radar.dtx"),
                  "min": [-10.5, -20.0], "max": [10.5, 20.0]},
        "players": {"min": 2, "max": 16}, "mod_id": mod_id, "patched_by": ["later_mod"],
    })
}

/// An entry document as the lobby writes it (`entry_set_json`), with `extra` keys.
fn entry_doc(mods: &[serde_json::Value], hash: &str, extra: serde_json::Value) -> Vec<u8> {
    let mut v = serde_json::json!({
        "entry": 7, "name": "t", "curated": true,
        "mod_set": {"policy": "modded", "client_visual_allowed": false, "mods": mods},
        "mod_set_hash": hash,
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v.to_string().into_bytes()
}

/// MAP-86: the entry document's records and base; no `maps` key (an older lobby) is `None`, an empty list is the
/// lobby saying the entry has none.
#[test]
fn the_entry_document_reads_its_map_records_and_base() {
    let extra =
        serde_json::json!({"community_map_base": 19, "maps": [doc_map_row("pack", "alpha")]});
    let doc = EntryDoc::parse(&entry_doc(&[], EMPTY_SET_HASH, extra)).unwrap();
    assert_eq!(doc.set.community_map_base, Some(19));
    let maps = doc.maps.unwrap();
    assert_eq!(maps.len(), 1);
    let m = &maps[0];
    assert_eq!(
        (m.id.as_str(), m.mod_id.as_str(), m.name.as_str()),
        ("alpha", "pack", "alpha name")
    );
    assert_eq!(m.description, None);
    assert_eq!(m.world, "@mod:pack/assets/maps/alpha.dat");
    assert_eq!(m.modes, ["TDM", "FFA"]);
    assert_eq!(m.goals, [("FFA".to_string(), 30), ("TDM".to_string(), 50)]);
    assert_eq!(m.round_seconds, Some(300));
    assert_eq!(
        m.picture.as_deref(),
        Some("@mod:pack/assets/pictures/alpha.dtx")
    );
    assert_eq!(m.loading, None);
    let radar = m.radar.as_ref().unwrap();
    assert_eq!((radar.min, radar.max), ([-10.5, -20.0], [10.5, 20.0]));
    assert_eq!(m.players, Some(dc_mod_lua::PlayerRange { min: 2, max: 16 }));
    assert_eq!(m.patched_by, ["later_mod"]);

    let older = EntryDoc::parse(&entry_doc(&[], EMPTY_SET_HASH, serde_json::json!({}))).unwrap();
    assert_eq!((older.maps, older.set.community_map_base), (None, None));
    let null = EntryDoc::parse(&entry_doc(
        &[],
        EMPTY_SET_HASH,
        serde_json::json!({"maps": null}),
    ))
    .unwrap();
    assert_eq!(null.maps, None);
    let none = EntryDoc::parse(&entry_doc(
        &[],
        EMPTY_SET_HASH,
        serde_json::json!({"maps": []}),
    ))
    .unwrap();
    assert_eq!(none.maps, Some(Vec::new()));
    // goals as an object come out sorted, as the host keeps them
    let mut row = doc_map_row("pack", "alpha");
    row["goals"] = serde_json::json!({"TDM": 50, "FFA": 30});
    let doc = EntryDoc::parse(&entry_doc(
        &[],
        EMPTY_SET_HASH,
        serde_json::json!({"maps": [row]}),
    ))
    .unwrap();
    assert_eq!(doc.maps.unwrap()[0].goals[0], ("FFA".to_string(), 30));
}

/// MAP-86: a row the client cannot turn into a record fails the document, as a malformed set does.
#[test]
fn a_bad_map_row_fails_the_entry_document() {
    let parse = |row: serde_json::Value| {
        EntryDoc::parse(&entry_doc(
            &[],
            EMPTY_SET_HASH,
            serde_json::json!({"maps": [row]}),
        ))
    };
    let with = |k: &str, v: serde_json::Value| {
        let mut r = doc_map_row("pack", "alpha");
        r[k] = v;
        r
    };
    assert!(parse(doc_map_row("pack", "alpha")).is_ok());
    for (k, v) in [
        ("id", serde_json::json!("Not-A-Key")),
        ("id", serde_json::json!(4)),
        ("mod_id", serde_json::json!("Bad Id")),
        ("name", serde_json::json!(null)),
        ("world", serde_json::json!("Worlds/x.dat")),
        ("modes", serde_json::json!("TDM")),
        ("goals", serde_json::json!([["TDM", -1]])),
        ("goals", serde_json::json!([["TDM"]])),
        ("round_seconds", serde_json::json!("long")),
        ("picture", serde_json::json!(3)),
        (
            "radar",
            serde_json::json!({"picture": "x", "min": [1.0], "max": [1.0, 2.0]}),
        ),
        ("players", serde_json::json!({"min": 2})),
    ] {
        let e = parse(with(k, v.clone())).expect_err(&format!("{k} = {v}"));
        assert!(e.contains("map record 0"), "{k}: {e}");
    }
    assert!(parse(serde_json::json!("alpha")).is_err());
    let not_a_list = entry_doc(&[], EMPTY_SET_HASH, serde_json::json!({"maps": {"a": 1}}));
    assert!(EntryDoc::parse(&not_a_list).is_err());
}

/// One entry document and its package behind a fake web port; the set's hash, the port and the data folder.
fn entry_routes(tag: &str, src: &Path, extra: serde_json::Value) -> (String, SocketAddr, PathBuf) {
    let root = temp(tag);
    let (zip, e) = dc_modkit::pack_mod_bytes(src).unwrap();
    let set = ServerModSet::from_fragment(&serde_json::json!({
        "policy": "modded", "client_visual_allowed": false, "mods": [entry_json(&e)],
    }))
    .unwrap();
    let hash = set.hash();
    let routes: Routes = Arc::default();
    routes.lock().unwrap().insert(
        "/entry_mods/7.json".into(),
        (200, entry_doc(&[entry_json(&e)], &hash, extra)),
    );
    routes
        .lock()
        .unwrap()
        .insert(format!("/mods/{}.zip", e.sha256), (200, zip));
    (hash, fake_lobby(routes), root)
}

/// MAP-86: the document's records and base are the report's; a leave clears both.
#[test]
fn join_entry_reports_the_documents_maps_and_base_and_a_leave_clears_them() {
    let src = temp("entry-doc-src");
    let dir = mod_dir(&src, "tpack", "shared");
    let extra =
        serde_json::json!({"community_map_base": 19, "maps": [doc_map_row("tpack", "alpha")]});
    let (hash, web, root) = entry_routes("entry-doc", &dir, extra);
    let mut mods = ClientMods::new();
    let got = mods
        .join_entry(Some(web), 7, &hash, &cfg(&root), &mut |_| {})
        .unwrap();
    assert_eq!(got, hash);
    assert!(mods.status.curated);
    assert_eq!(mods.status.community_map_base, Some(19));
    assert_eq!(
        mods.status.maps.len(),
        1,
        "the pack registers none: the document's row stands"
    );
    assert_eq!(mods.status.maps[0].id, "alpha");
    assert_eq!(
        mods.status.recipe.as_ref().map(|r| r.policy),
        Some(ModPolicy::Curated {
            client_visual_allowed: false
        })
    );

    // what `entry_leave` does
    mods.join_set(None, ServerModSet::vanilla(), &cfg(&root), &mut |_| {})
        .unwrap();
    assert!(mods.status.maps.is_empty());
    assert_eq!(mods.status.community_map_base, None);
    assert_eq!(mods.status.recipe, None);
    assert!(!mods.status.curated);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&src);
}

/// LG-426: a curated entry whose fragment allows local mods loads the player's client_visual one after its pack (the
/// recipe keeps it local) and refuses a local shared one; the same entry without the flag loads no local mod.
#[test]
fn a_curated_entry_that_allows_local_mods_loads_them_after_its_pack() {
    let src = temp("entry-cv-src");
    let dir = mod_dir(&src, "tpack", "shared");
    for allowed in [true, false] {
        let root = temp(if allowed {
            "entry-cv-on"
        } else {
            "entry-cv-off"
        });
        let (zip, e) = dc_modkit::pack_mod_bytes(&dir).unwrap();
        let frag = serde_json::json!({
            "policy": "modded", "client_visual_allowed": allowed, "mods": [entry_json(&e)],
        });
        let hash = ServerModSet::from_fragment(&frag).unwrap().hash();
        let doc = serde_json::json!({
            "entry": 7, "name": "t", "curated": true, "mod_set": frag, "mod_set_hash": hash,
        });
        let routes: Routes = Arc::default();
        routes.lock().unwrap().insert(
            "/entry_mods/7.json".into(),
            (200, doc.to_string().into_bytes()),
        );
        routes
            .lock()
            .unwrap()
            .insert(format!("/mods/{}.zip", e.sha256), (200, zip));
        let web = fake_lobby(routes);
        let paths = ModPaths::new(&root);
        mod_dir(&paths.local(), "green", "client_visual");
        mod_dir(&paths.local(), "sneaky", "shared");
        let mut mods = ClientMods::new();
        let got = mods
            .join_entry(Some(web), 7, &hash, &cfg(&root), &mut |_| {})
            .unwrap();
        assert_eq!(got, hash, "the local mod is no part of the claim");
        assert!(mods.status.curated);
        let recipe = mods.status.recipe.clone().unwrap();
        assert_eq!(
            recipe.policy,
            ModPolicy::Curated {
                client_visual_allowed: allowed
            }
        );
        if allowed {
            assert_eq!(
                mods.loaded(),
                vec!["tpack".to_string(), "green".to_string()]
            );
            assert_eq!(recipe.mods[1].origin, ModOrigin::Local);
            assert_eq!(mods.status.local_refused.len(), 1);
            assert!(
                mods.status.local_refused[0].contains("must be kind 'client_visual'"),
                "{:?}",
                mods.status.local_refused
            );
        } else {
            assert_eq!(mods.loaded(), vec!["tpack".to_string()]);
            assert!(mods.status.local_refused.is_empty(), "no local load at all");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
    let _ = std::fs::remove_dir_all(&src);
}

/// MAP-86: a bad row refuses the entry: nothing loaded, no rows.
#[test]
fn join_entry_refuses_a_document_with_a_bad_map_row() {
    let src = temp("entry-bad-src");
    let dir = mod_dir(&src, "tpack", "shared");
    let mut row = doc_map_row("tpack", "alpha");
    row["world"] = serde_json::json!("Worlds/x.dat");
    let (hash, web, root) = entry_routes("entry-bad", &dir, serde_json::json!({"maps": [row]}));
    let mut mods = ClientMods::new();
    let e = mods
        .join_entry(Some(web), 7, &hash, &cfg(&root), &mut |_| {})
        .unwrap_err();
    assert!(e.contains("map record 0"), "{e}");
    assert!(matches!(mods.status.state, ModState::Failed(_)));
    assert!(mods.status.maps.is_empty() && mods.status.recipe.is_none() && !mods.status.curated);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&src);
}

/// MAP-86: a document without `maps` (an older lobby): the loaded host's records stand in, and no base is claimed; a
/// document with `maps` wins over the host (the lobby may have left a record out, so its list is what the indices
/// mean). The map pack needs `content.maps` in the curated list (contract item (a), the lobby slice's).
#[test]
fn join_entry_falls_back_to_the_hosts_records_and_the_document_wins_over_them() {
    if !dc_mod_lua::CURATED_CAPABILITIES.contains(&"content.maps") {
        eprintln!("skipped: content.maps is not a curated capability in this tree yet");
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/starter_map");
    let (hash, web, root) = entry_routes("entry-host", &dir, serde_json::json!({}));
    let mut mods = ClientMods::new();
    mods.join_entry(Some(web), 7, &hash, &cfg(&root), &mut |_| {})
        .unwrap();
    let ids: Vec<&str> = mods.status.maps.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["starter_map"], "the host's records stand in");
    assert_eq!(mods.status.community_map_base, None);
    let _ = std::fs::remove_dir_all(&root);

    let (hash, web, root) = entry_routes("entry-host2", &dir, serde_json::json!({"maps": []}));
    let mut mods = ClientMods::new();
    mods.join_entry(Some(web), 7, &hash, &cfg(&root), &mut |_| {})
        .unwrap();
    assert!(mods.status.maps.is_empty(), "the lobby left it out: no row");
    let _ = std::fs::remove_dir_all(&root);
}

/// LG-429: the mod-set fragment's `client_fov` is read as `[min, max]` for a modded set (kept by the fragment round
/// trip); a vanilla set, a missing key and a broken range publish none.
#[test]
fn the_fragment_carries_the_field_of_view_range_of_a_modded_set_only() {
    let read = |v: serde_json::Value| ServerModSet::from_fragment(&v).unwrap();
    let modded = read(serde_json::json!({"policy": "modded", "client_fov": [70, 120], "mods": []}));
    assert_eq!(modded.fov_range(), Some((70, 120)));
    assert_eq!(read(modded.to_fragment()).fov_range(), Some((70, 120)), "round trip");
    assert_eq!(
        read(serde_json::json!({"policy": "modded", "client_fov": [90, 90], "mods": []})).fov_range(),
        Some((90, 90)),
        "a locked range is still a range"
    );
    for none in [
        serde_json::json!({"policy": "modded", "mods": []}),
        serde_json::json!({"policy": "modded", "client_fov": [120, 70], "mods": []}),
        serde_json::json!({"policy": "modded", "client_fov": [70], "mods": []}),
        serde_json::json!({"policy": "modded", "client_fov": "wide", "mods": []}),
        serde_json::json!({"policy": "vanilla", "client_fov": [70, 120]}),
    ] {
        assert_eq!(read(none.clone()).fov_range(), None, "{none}");
    }
    assert_eq!(ServerModSet::vanilla().fov_range(), None);
}

/// `n` tiny packages `m0 .. m{n-1}` with their set entries; the lobby's set hash over them.
fn many_packs(root: &Path, n: usize) -> (Vec<(Vec<u8>, dc_modkit::ModSetEntry)>, String) {
    let packs: Vec<_> = (0..n)
        .map(|i| {
            dc_modkit::pack_mod_bytes(&mod_dir(&root.join("src"), &format!("m{i}"), "shared"))
                .unwrap()
        })
        .collect();
    let lines: String = packs
        .iter()
        .enumerate()
        .map(|(i, (_, e))| format!("m{i}@1.0.0:{}
", e.sha256))
        .collect();
    (packs, dc_rom::sha256_hex(lines.as_bytes()))
}

fn serve_packs(packs: &[(Vec<u8>, dc_modkit::ModSetEntry)], hash: &str) -> Routes {
    let routes: Routes = Arc::default();
    let entries: Vec<_> = packs.iter().map(|(_, e)| entry_json(e)).collect();
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (200, server_list(&entries, false, hash)),
    );
    for (zip, e) in packs {
        routes
            .lock()
            .unwrap()
            .insert(format!("/mods/{}.zip", e.sha256), (200, zip.clone()));
    }
    routes
}

// CLI-1608 / CLI-1940: a long set downloads several packages at a time and reports one line for the whole fetch
#[test]
fn a_long_set_reports_pack_index_count_and_bytes_and_loads_in_the_servers_order() {
    let root = temp("many");
    let (packs, hash) = many_packs(&root, 12);
    let web = fake_lobby(serve_packs(&packs, &hash));
    let data = root.join("data");
    let mut cm = ClientMods::new();
    let mut seen = Vec::new();
    cm.join(Some(web), None, &cfg(&data), &mut |s| seen.push(s))
        .unwrap();
    let want: Vec<String> = (0..12).map(|i| format!("m{i}")).collect();
    assert_eq!(cm.loaded(), want, "the load order is the set's order");
    let lines: Vec<(usize, usize, u64, u64)> = seen
        .iter()
        .filter_map(|s| match s {
            ModState::Downloading {
                index,
                count,
                done,
                total,
            } => Some((*index, *count, *done, *total)),
            _ => None,
        })
        .collect();
    assert_eq!(lines[0], (1, 12, 0, 0), "the first line names the count");
    assert!(lines.iter().all(|l| l.1 == 12 && l.0 >= 1 && l.0 <= 12));
    assert!(
        lines.windows(2).all(|w| w[0].0 <= w[1].0 && w[0].2 <= w[1].2),
        "pack index and bytes never go back: {lines:?}"
    );
    let bytes: u64 = packs.iter().map(|(z, _)| z.len() as u64).sum();
    let last = lines.last().unwrap();
    assert_eq!(last.2, bytes, "every byte is counted at the end: {last:?}");
    assert_eq!(last.3, bytes, "every size was told: {last:?}");
    // a second join takes all of them from the cache and shows no download
    let mut again = Vec::new();
    let mut cm = ClientMods::new();
    cm.join(Some(web), None, &cfg(&data), &mut |s| again.push(s))
        .unwrap();
    assert!(!again
        .iter()
        .any(|s| matches!(s, ModState::Downloading { .. })));
    let _ = std::fs::remove_dir_all(&root);
}

// CLI-1608: one bad package among many refuses the join, is never written, and nothing of the set is unpacked
#[test]
fn one_bad_package_among_many_refuses_the_join_and_unpacks_nothing() {
    let root = temp("manybad");
    let (packs, hash) = many_packs(&root, 12);
    let routes = serve_packs(&packs, &hash);
    let bad = &packs[7].1.sha256;
    let mut swapped = packs[7].0.clone();
    let n = swapped.len();
    swapped[n / 2] ^= 0x55;
    routes
        .lock()
        .unwrap()
        .insert(format!("/mods/{bad}.zip"), (200, swapped));
    let web = fake_lobby(routes);
    let data = root.join("data");
    let mut cm = ClientMods::new();
    let err = cm
        .join(Some(web), None, &cfg(&data), &mut |_| {})
        .unwrap_err();
    assert!(err.contains("m7") && err.contains("join refused"), "{err}");
    let paths = ModPaths::new(&data);
    assert!(!paths.zip(bad).exists(), "the bad package was written");
    for (_, e) in &packs {
        assert!(
            !paths.unpacked(&e.sha256).exists(),
            "{} unpacked",
            e.sha256
        );
    }
    assert!(cm.loaded().is_empty() && cm.status.hash.is_none());
    assert!(matches!(cm.status.state, ModState::Failed(_)));
    let _ = std::fs::remove_dir_all(&root);
}

// CLI-1608: 65 packs from a server that answers each request after 30 ms (a stand-in for the connection set-up of a
// far server): the parallel fetch against the same downloads one after the other
#[test]
fn sixty_five_packs_download_faster_in_parallel_than_one_after_the_other() {
    let root = temp("sixtyfive");
    let (packs, hash) = many_packs(&root, 65);
    let web = fake_lobby_slow(serve_packs(&packs, &hash), Duration::from_millis(30));
    let c = cfg(&root.join("seq"));
    let paths = ModPaths::new(&root.join("seq"));
    let t = Instant::now();
    for (_, e) in &packs {
        let m = ServerMod {
            id: String::new(),
            version: "1.0.0".into(),
            sha256: e.sha256.clone(),
            ..declared(&[("x", "1.0.0", "")]).mods[0].clone()
        };
        download_package(web, &paths, &m, &c, &mut |_, _| {}).unwrap();
    }
    let sequential = t.elapsed();
    let mut cm = ClientMods::new();
    let t = Instant::now();
    cm.join(Some(web), None, &cfg(&root.join("par")), &mut |_| {})
        .unwrap();
    let parallel = t.elapsed();
    eprintln!("65 packs, 30 ms per request: one after the other {sequential:?}, joined {parallel:?}");
    assert_eq!(cm.loaded().len(), 65);
    assert!(
        parallel * 2 < sequential,
        "{parallel:?} is not under half of {sequential:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// CLI-1943: the set fragment's `tags` reach the client and survive a round trip; a fragment without them reads as none.
#[test]
fn the_set_fragment_keeps_the_tags() {
    let frag = serde_json::json!({"policy": "modded", "mods": [
        {"id": "variant_x", "version": "1.0.0", "sha256": "ab".repeat(32), "kind": "shared", "tags": ["variant", "camo"]},
        {"id": "plain_y", "version": "1.0.0", "sha256": "cd".repeat(32), "kind": "shared"}
    ]});
    let set = ServerModSet::from_fragment(&frag).unwrap();
    assert_eq!(set.mods[0].tags, vec!["variant".to_string(), "camo".to_string()]);
    assert!(set.mods[1].tags.is_empty());
    let again = ServerModSet::from_fragment(&set.to_fragment()).unwrap();
    assert_eq!(again, set);
    assert_eq!(again.hash(), set.hash(), "the tags are not part of the set's hash");
}
