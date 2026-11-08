//! The player's view of mods (CLI-1616 .. CLI-1618), headless: the community browser shows a vanilla and a modded
//! entry with their badges from a fixture document and its Join pins the entry; the consent prompt appears before any
//! download for an unverified server mod, Cancel sends no login and downloads nothing, Accept is remembered in the
//! profile; the Mods window lists the server set of the last join and toggles a local mod (profile round trip).
//! A fake lobby web port on loopback serves `server_list.json`. Nothing of the ROM is in this file.

use dc_client::community::{parse_community, CommunityView};
use dc_client::menu::link::{LinkEvent, LinkOptions};
use dc_client::menu::modbrowser::{
    frame_art_size, frame_title_y, BUTTON_ART, CMD_CLEAN_UP, FRAME_BAR_H, FRAME_BAR_TOP, FRAME_HEAD_H, CMD_DOWNLOAD_BASE, CMD_FORGET_BASE, CMD_REMOVE_BASE, CMD_REMOVE_LOCAL_BASE,
    CMD_SETTINGS_BASE, CMD_TAB_BROWSE, CMD_TAB_MINE, DETAIL_RECT, FOOT_Y, LIST_RECT, MODS_RECT, MODS_ROWS, MODS_ROW_H,
    MODS_ROW_STEP, T_BROWSE, T_CTX_MODDED, T_CTX_VANILLA, T_DOWNLOAD, T_INSTALLED, T_MY_MODS, T_NOTHING_INSTALLED,
    T_SETTINGS,
};
use dc_client::menu::modsettings::{
    CMD_PAGE_BACK, CMD_PAGE_COLOUR_BASE, CMD_PAGE_COPY, CMD_PAGE_NUDGE_BASE, CMD_PAGE_PASTE, CMD_PAGE_RESET,
    CMD_PAGE_STEP_BASE, T_APPLIES, T_COPY, T_PASTE, T_RESET,
};
use dc_client::menu::modbrowser::{
    CMD_AGREE, CMD_CLOSE, CMD_DECLINE, CMD_GROUP_BASE, CMD_JOIN, CMD_MODS, CMD_ROW_BASE, CMD_TOGGLE_BASE,
    T_CAMOS, T_CAMOS_KIND, T_CAMOS_NOTE, T_CONSENT_TITLE, T_HIDE_CAMOS, T_ORIGIN_OPTIONAL, T_ORIGIN_SERVER,
    T_ORIGIN_YOURS, T_SHOW_CAMOS,
};
use dc_client::menu::{login, Menu, MenuInput, MenuOptions};
use dc_client::hud::items::DrawItem;
use dc_client::settings::ProfileStore;
use dc_client::testkit::synth_font;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc_menu_mods_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn menu(data: Option<&Path>, auth: SocketAddr) -> Menu {
    // the published list stays out of these tests; a test that wants one feeds it
    dc_client::community::set_url("");
    let mut link = LinkOptions::new("menu-mods");
    link.mods.data_dir = data.map(Path::to_path_buf);
    link.mods.timeout = Duration::from_secs(2);
    Menu::new(MenuOptions {
        auth,
        user: "tester".into(),
        link,
        rom: dc_rom::resolve_rom().ok(),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    })
}

fn store(dir: &Path) -> ProfileStore {
    let file = dir.join("player.txt");
    ProfileStore {
        read: vec![file.clone()],
        write: Some(file),
        key: None,
    }
}

fn texts(m: &mut Menu) -> Vec<String> {
    m.items()
        .into_iter()
        .filter_map(|i| match i {
            DrawItem::Label { text, .. } | DrawItem::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

/// The panel's text from its top and from its end (a long mod's detail scrolls), then back at its top.
fn detail_text(m: &mut Menu) -> String {
    let [x, y, w, h] = DETAIL_RECT;
    let centre = [(x + w / 2) as f32, (y + h / 2) as f32];
    let mut all = texts(m).join(" ");
    m.input(MenuInput::Wheel(centre, -30));
    all.push(' ');
    all.push_str(&texts(m).join(" "));
    m.input(MenuInput::Wheel(centre, 30));
    all
}

fn has(t: &[String], s: &str) -> bool {
    t.iter().any(|x| x.contains(s))
}

/// The tests share the process-wide certificate pin: one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

const SHA_A: &str = "8dd4370211a2686be7c16ccfdfd881a8154b0fe9db24249768042a21d8ad799e";

fn fixture() -> CommunityView {
    let doc = parse_community(&format!(
        r#"{{"schema_version": 1, "updated": "2026-11-01T12:00:00Z",
        "servers": [
          {{"id": "plain-one", "name": "Plain One", "region": "eu", "address": "127.0.0.1:10000",
            "policy": "vanilla", "players": {{"current": 3, "max": 16}}, "mods": []}},
          {{"id": "mod-one", "name": "Mod One", "region": "eu", "address": "127.0.0.1:11000",
            "policy": "modded", "fingerprint": "{fp}",
            "mods": [{{"id": "hacking_mod", "version": "1.1.1", "sha256": "{SHA_A}"}},
                     {{"id": "odd_rules", "version": "2.0.0", "sha256": "{odd}"}}]}}
        ],
        "known_mods": [{{"id": "hacking_mod", "name": "Hacking Mod", "version": "1.1.1", "sha256": "{SHA_A}",
          "source_repo": "https://github.com/example/xray", "author": "DeCombat",
          "capabilities": ["world.full_visibility"]}}]}}"#,
        fp = "ab".repeat(32),
        odd = "cd".repeat(32),
    ))
    .expect("the fixture document is valid");
    CommunityView {
        addrs: vec![
            Some("127.0.0.1:10000".parse().unwrap()),
            Some("127.0.0.1:11000".parse().unwrap()),
        ],
        doc: Some(doc),
        cached: false,
        error: None,
    }
}

#[test]
fn the_browser_shows_vanilla_and_modded_entries_with_badges_and_join_pins_the_entry() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = menu(None, "127.0.0.1:1".parse().unwrap());
    m.feed_event(LinkEvent::Community(fixture()));
    m.update();
    assert_eq!(m.screen_name(), "login");
    m.input(MenuInput::Command(login::CMD_HOMEPAGE));
    assert_eq!(m.screen_name(), "community");
    let t = texts(&mut m);
    for want in [
        "Configured lobby",
        "Plain One",
        "VANILLA",
        "3 / 16 players",
        "Mod One",
        "MODDED",
        "Hacking Mod",
        " [Verified]",
        "odd_rules",
        " [Unverified]",
    ] {
        assert!(has(&t, want), "missing {want:?} in {t:?}");
    }
    // row 2 = Mod One (row 0 is the configured lobby): Join sets the address and pins the entry's set
    m.input(MenuInput::Command(CMD_ROW_BASE + 2));
    m.input(MenuInput::Command(CMD_JOIN));
    assert_eq!(m.screen_name(), "login");
    assert_eq!(m.data.auth, Some("127.0.0.1:11000".parse().unwrap()));
    assert_eq!(m.data.picked_server.as_deref(), Some("Mod One"));
    assert_eq!(m.link_latest_request(), "ExpectModSet(true)");
    assert_eq!(
        dc_botclient::tls::current().pin.map(|p| p.replace(':', "").to_lowercase()),
        Some("ab".repeat(32))
    );
    assert!(has(&texts(&mut m), "Server: Mod One"));
    // the configured lobby again: no pin, no expected set
    m.input(MenuInput::Command(login::CMD_HOMEPAGE));
    m.input(MenuInput::Command(CMD_ROW_BASE));
    m.input(MenuInput::Command(CMD_JOIN));
    assert_eq!(m.data.auth, Some("127.0.0.1:1".parse().unwrap()));
    assert_eq!(m.link_latest_request(), "ExpectModSet(false)");
    assert_eq!(dc_botclient::tls::current().pin, None);
}

#[test]
fn with_the_list_off_the_browser_shows_the_configured_lobby_only() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = menu(None, "127.0.0.1:1".parse().unwrap());
    m.input(MenuInput::Command(login::CMD_HOMEPAGE));
    let t = texts(&mut m);
    assert!(has(&t, "The community list is off"), "{t:?}");
    assert!(has(&t, "Configured lobby"));
    assert!(!has(&t, "MODDED"));
    m.input(MenuInput::Command(CMD_CLOSE));
    assert_eq!(m.screen_name(), "login");
}

type Routes = Arc<Mutex<BTreeMap<String, (u16, Vec<u8>)>>>;

/// A plain-HTTP web port answering from `routes` and noting every path asked for.
fn web(routes: Routes, seen: Arc<Mutex<Vec<String>>>, port: u16) {
    let l = TcpListener::bind(("127.0.0.1", port)).unwrap();
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
            seen.lock().unwrap().push(path.clone());
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
}

/// An auth port P (never answering) with the web port at P + 8080.
fn lobby(routes: Routes, seen: Arc<Mutex<Vec<String>>>) -> (TcpListener, SocketAddr) {
    for _ in 0..50 {
        let w = TcpListener::bind("127.0.0.1:0").unwrap();
        let wp = w.local_addr().unwrap().port();
        let Some(p) = wp.checked_sub(8080).filter(|p| *p > 1024) else {
            continue;
        };
        let Ok(a) = TcpListener::bind(("127.0.0.1", p)) else {
            continue;
        };
        drop(w);
        web(routes, seen, wp);
        a.set_nonblocking(true).unwrap();
        return (a, SocketAddr::from(([127, 0, 0, 1], p)));
    }
    panic!("no port pair");
}

fn local_mod(root: &Path, id: &str, kind: &str) {
    let d = root.join("mods").join("local").join(id);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("manifest.json"),
        format!(
            r#"{{"id": "{id}", "name": "{id} look", "version": "1.0.0", "kind": "{kind}", "capabilities": ["content.text"], "description": "A local test mod for the Mods window.", "author": "tests", "screenshots": ["screenshots/main.png"]}}"#
        ),
    )
    .unwrap();
    std::fs::write(d.join("main.lua"), "return function(mod) end").unwrap();
}

/// Updates the menu until `done` holds (20 s at most).
fn until(m: &mut Menu, what: &str, mut done: impl FnMut(&mut Menu) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(20) {
        m.update();
        if done(m) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}: {:?}", texts(m));
}

#[test]
fn the_consent_prompt_comes_before_any_download_and_the_mods_window_lists_the_set() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("consent");
    local_mod(&root, "glow", "client_visual");
    local_mod(&root, "rules", "server_rules");
    let sha = "cd".repeat(32);
    let fragment = serde_json::json!({"policy": "modded", "client_visual_allowed": true, "mods": [{
        "id": "tmod", "version": "1.0.0", "sha256": sha, "kind": "shared", "requires_client": true,
        "capabilities": ["content.weapons", "world.full_visibility"], "author": "Alice",
        "source_repo": "https://github.com/alice/tmod", "description": "A test mod the server declares.",
        "screenshots": []}]});
    let set = dc_client::mods::ServerModSet::from_fragment(&fragment).unwrap();
    let routes: Routes = Arc::default();
    routes.lock().unwrap().insert(
        "/server_list.json".into(),
        (
            200,
            serde_json::json!({"name": "t", "mod_set": fragment, "mod_set_hash": set.hash()})
                .to_string()
                .into_bytes(),
        ),
    );
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let (auth_l, auth) = lobby(routes, Arc::clone(&seen));
    let downloads = |seen: &Arc<Mutex<Vec<String>>>| {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with("/mods/"))
            .count()
    };
    let mut m = menu(Some(&root), auth);
    m.load_settings(store(&root));
    m.update();

    // the first login: the prompt shows the author, the source and the capabilities in plain words
    m.input(MenuInput::Command(login::CMD_LOGIN));
    until(&mut m, "the consent prompt", |m| {
        has(&texts(m), T_CONSENT_TITLE)
    });
    let t = texts(&mut m);
    for want in [
        "tmod 1.0.0",
        "Author: Alice",
        "Source: https://github.com/alice/tmod",
        "It changes weapon stats, everyone is visible through walls",
        "Unverified",
    ] {
        assert!(has(&t, want), "missing {want:?} in {t:?}");
    }
    assert_eq!(downloads(&seen), 0, "a package was asked for before the answer");

    // Cancel: back to the browser, no login, no download
    m.input(MenuInput::Command(CMD_DECLINE));
    assert_eq!(m.screen_name(), "community");
    assert_eq!(m.link_latest_request(), "ModConsent(false)");
    std::thread::sleep(Duration::from_millis(300));
    m.update();
    assert!(auth_l.accept().is_err(), "a login followed the cancelled prompt");
    assert_eq!(downloads(&seen), 0);
    assert!(m.data.settings.mods_accepted.is_empty());

    // the Mods window opens on My Mods (something is installed): no paragraph, no path
    m.input(MenuInput::Command(CMD_MODS));
    assert_eq!(m.screen_name(), "mods");
    let t = texts(&mut m);
    for want in ["glow look", "rules look", "Enable", "Off", T_BROWSE, T_MY_MODS, "A local test mod"] {
        assert!(has(&t, want), "missing {want:?} in {t:?}");
    }
    assert!(!has(&t, "A mod is a package") && !has(&t, "folder:") && !has(&t, "dc_menu_mods"), "{t:?}");
    // a mod that is merely off shows no warning; the one that cannot load says why, in full, when selected
    assert!(!has(&t, "did not load") && !has(&t, "Only client-visual"), "{t:?}");
    m.input(MenuInput::Command(CMD_ROW_BASE + 1));
    let all = texts(&mut m).join(" ");
    assert!(all.contains("Only client-visual mods can be") && all.contains("loads only from a server"), "{all}");
    // Browse: the server's mod that is not on this PC, its words in plain lines
    m.input(MenuInput::Command(CMD_TAB_BROWSE));
    let t = texts(&mut m);
    for want in [
        "tmod",
        "With server",
        "Version 1.0.0 by Alice",
        "Changes weapon stats",
        "Everyone is visible through walls",
        "Unverified",
    ] {
        assert!(has(&t, want), "missing {want:?} in {t:?}");
    }
    assert!(!has(&t, "glow look"), "{t:?}");
    // glow is the first local folder: enabled, saved, shown as on
    m.input(MenuInput::Command(CMD_TAB_MINE));
    m.input(MenuInput::Command(CMD_ROW_BASE));
    m.input(MenuInput::Command(CMD_TOGGLE_BASE));
    assert_eq!(m.data.settings.mods_local_enabled, vec!["glow".to_string()]);
    assert!(m.link_latest_request().starts_with("ModPrefs(0, [\"glow\"])"), "{}", m.link_latest_request());
    let t = texts(&mut m);
    assert!(t.iter().any(|x| x == "On"), "{t:?}");
    assert!(has(&t, "Disable"));
    let profile = std::fs::read_to_string(root.join("player.txt")).unwrap();
    assert!(profile.contains("mods.local_enabled = glow"), "{profile}");
    let mut again = menu(Some(&root), auth);
    again.load_settings(store(&root));
    assert_eq!(again.data.settings.mods_local_enabled, vec!["glow".to_string()]);
    drop(again);
    m.input(MenuInput::Command(CMD_CLOSE));
    assert_eq!(m.screen_name(), "login");

    // Accept: remembered per (id, version, sha256); the download follows (the fake lobby has no package)
    m.input(MenuInput::Command(login::CMD_LOGIN));
    until(&mut m, "the second prompt", |m| has(&texts(m), T_CONSENT_TITLE));
    m.input(MenuInput::Command(CMD_AGREE));
    let key = format!("tmod@1.0.0:{sha}");
    assert_eq!(m.data.settings.mods_accepted, vec![key.clone()]);
    let profile = std::fs::read_to_string(root.join("player.txt")).unwrap();
    assert!(profile.contains(&format!("mods.accepted = {key}")), "{profile}");
    until(&mut m, "the download after Accept", |_| downloads(&seen) >= 1);
    // the next join of the same set asks nothing: straight to the download
    let asked_before = downloads(&seen);
    for _ in 0..50 {
        m.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    m.open(dc_client::menu::Go::Login);
    m.input(MenuInput::Command(login::CMD_LOGIN));
    until(&mut m, "the third join's download", |_| downloads(&seen) > asked_before);
    assert!(!has(&texts(&mut m), T_CONSENT_TITLE), "an accepted mod was asked again");
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------ LG-272 / CLI-1622 / CLI-1623: the server list

/// True when `s` holds an IPv4 literal (four dot-separated groups of 1..3 digits).
fn has_ipv4(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len()).any(|start| {
        if start > 0 && (b[start - 1].is_ascii_digit() || b[start - 1] == b'.') {
            return false;
        }
        let mut i = start;
        for group in 0..4 {
            let d0 = i;
            while i < b.len() && b[i].is_ascii_digit() && i - d0 < 3 {
                i += 1;
            }
            if i == d0 {
                return false;
            }
            if group < 3 {
                if i >= b.len() || b[i] != b'.' {
                    return false;
                }
                i += 1;
            }
        }
        i >= b.len() || !b[i].is_ascii_digit()
    })
}

fn vanilla_list() -> Vec<dc_botclient::lobby::ServerEntry> {
    ["US Central", "US West", "US Central Clan"]
        .iter()
        .enumerate()
        .map(|(i, n)| dc_botclient::lobby::ServerEntry {
            id: i as u32 + 1,
            name: n.to_string(),
            ip: "203.0.113.10".into(),
            port: 10000,
            open: 1,
            capacity: 100,
            current: 3,
            clan_only: (i == 2) as u32,
            op_capacity: 0,
            op_current: 0,
        })
        .collect()
}

fn other(name: &str, region: &str, addr: SocketAddr) -> dc_client::menu::link::OtherLobby {
    dc_client::menu::link::OtherLobby {
        listed: dc_wire::moddedlist::ListedLobby {
            name: name.into(),
            region: region.into(),
            lobby: addr.to_string(),
            fingerprint: String::new(),
            policy: "modded".into(),
        },
        addr: Some(addr),
    }
}

#[test]
fn the_server_list_shows_modded_rows_with_badges_mods_and_the_mods_button_and_no_address() {
    // needs the ROM's screen layouts (or body models): CI has none
    if dc_rom::resolve_rom().is_err() {
        println!("skipped: CA_ROM unset");
        return;
    }
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = menu(None, "127.0.0.1:1".parse().unwrap());
    m.feed_event(LinkEvent::Community(fixture()));
    // the lobby's own entry has the community's Mod One address: one row, the lobby's name, the document's mods
    m.feed_event(LinkEvent::OtherLobbies(vec![
        other("Central Modded", "US Central", "127.0.0.1:11000".parse().unwrap()),
        other("West Modded", "US West", "203.0.113.9:12000".parse().unwrap()),
    ]));
    m.feed_event(LinkEvent::Servers(vanilla_list()));
    m.update();
    m.open(dc_client::menu::Go::Servers);
    assert_eq!(m.screen_name(), "servers");
    let t = texts(&mut m);
    for want in [
        "US Central",
        "US West",
        "Central Modded (US Central)",
        "West Modded (US West)",
        "MODDED",
        "Modded servers",
        "Hacking Mod",
        " [Verified]",
        "odd_rules",
        " [Unverified]",
        "Mods",
    ] {
        assert!(has(&t, want), "missing {want:?} in {t:?}");
    }
    assert!(!has(&t, "Mod One"), "the document's entry of the same address is merged: {t:?}");
    let shown: Vec<&String> = t.iter().filter(|s| has_ipv4(s)).collect();
    assert!(shown.is_empty(), "an address is on the server screen: {shown:?}");
    // the Mods button opens the Mods window over the list
    m.input(MenuInput::Command(CMD_MODS));
    assert_eq!(m.screen_name(), "mods");
    m.input(MenuInput::Key(dc_client::menu::MenuKey::Escape));
    assert_eq!(m.screen_name(), "servers");
    // the browser before the login shows no address either
    m.open(dc_client::menu::Go::Login);
    m.input(MenuInput::Command(login::CMD_HOMEPAGE));
    assert_eq!(m.screen_name(), "community");
    for row in 0..3 {
        m.input(MenuInput::Command(CMD_ROW_BASE + row));
        let t = texts(&mut m);
        let shown: Vec<&String> = t.iter().filter(|s| has_ipv4(s)).collect();
        assert!(shown.is_empty(), "an address is in the browser (row {row}): {shown:?}");
    }
    assert!(has_ipv4("at 203.0.113.10 now") && !has_ipv4("v1.2.3 build 2026.11.01"));
}

/// A fake auth port that sends the login request's (name, password) and closes.
fn fake_auth(l: TcpListener) -> std::sync::mpsc::Receiver<(String, String)> {
    let (tx, rx) = std::sync::mpsc::channel();
    l.set_nonblocking(false).unwrap();
    std::thread::spawn(move || {
        let wstr = |d: &[u8], o: usize, n: usize| {
            let u: Vec<u16> = d[o.min(d.len())..(o + 2 * n).min(d.len())]
                .chunks(2)
                .filter(|c| c.len() == 2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|u| *u != 0)
                .collect();
            String::from_utf16_lossy(&u)
        };
        loop {
            let Ok((mut c, _)) = l.accept() else { return };
            let _ = c.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buf = Vec::new();
            let mut tmp = [0u8; 65536];
            match c.read(&mut tmp) {
                Ok(n) if n > 0 && tmp[0] != 0x16 => buf.extend_from_slice(&tmp[..n]),
                _ => continue,
            }
            loop {
                let (frames, used) = dc_wire::tcp::parse_tcp_frames(&buf).unwrap();
                buf.drain(..used);
                for (id, d) in frames {
                    if id as u16 == 0x206 {
                        let mut w = dc_wire::PktWriter::new(0x207);
                        w.u32(1);
                        c.write_all(&w.to_frame()).unwrap();
                    } else if id as u16 == 0x200 {
                        let _ = tx.send((wstr(&d, 6, 4096), wstr(&d, 0x2006, 17)));
                        return;
                    }
                }
                match c.read(&mut tmp) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
        }
    });
    rx
}

#[test]
fn picking_a_modded_row_runs_the_mod_step_and_logs_in_with_the_token_and_leaving_goes_home() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("token");
    let cache = root.join(dc_client::identity::CACHE_FILE);
    let authority: SocketAddr = "127.0.0.1:9".parse().unwrap();
    dc_client::identity::store(
        &cache,
        &dc_client::identity::Cached {
            token: "dG9r.c2ln".into(),
            expires: dc_client::identity::now() + 3600,
            account: 5,
            nick: "Tester".into(),
            login: "tester".into(),
            authority: authority.to_string(),
        },
    )
    .unwrap();
    // the modded lobby: its web port answers 404 (a vanilla set), its auth port notes the login
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let (auth_l, modded) = lobby(Arc::default(), Arc::clone(&seen));
    let logins = fake_auth(auth_l);
    let home: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let mut link = LinkOptions::new("menu-mods-token");
    link.mods.timeout = Duration::from_secs(2);
    link.identity = Some(dc_client::identity::Authority {
        addr: authority,
        key: authority.to_string(),
        trust: dc_botclient::tls::Trust::default(),
        derived: false,
        cache: Some(cache.clone()),
    });
    let mut m = Menu::new(MenuOptions {
        auth: home,
        user: "tester".into(),
        link,
        rom: dc_rom::resolve_rom().ok(),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.set_identity(Some(authority), Some("tester".into()));
    m.feed_event(LinkEvent::OtherLobbies(vec![other("Central Modded", "US Central", modded)]));
    m.feed_event(LinkEvent::Servers(vanilla_list()));
    m.update();
    m.open(dc_client::menu::Go::Servers);
    // the fourth row (after the three entries of this lobby), Enter
    for _ in 0..3 {
        m.input(MenuInput::Key(dc_client::menu::MenuKey::Down));
    }
    m.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
    until(&mut m, "the login screen", |m| m.screen_name() == "login");
    assert_eq!(m.data.picked_server.as_deref(), Some("Central Modded"));
    assert_eq!(m.data.home.as_ref().and_then(|h| h.auth), Some(home));
    let (name, password) = logins
        .recv_timeout(Duration::from_secs(10))
        .expect("a login at the modded lobby");
    assert_eq!(
        name,
        format!("LOCAL:tok:dG9r.c2ln;m={};c=0", dc_wire::loginmods::EMPTY_SET_HASH)
    );
    assert_eq!(password, "", "no password goes to the modded lobby");
    assert!(
        seen.lock().unwrap().iter().any(|p| p == "/server_list.json"),
        "the mod step read the lobby's set first"
    );
    // the fake closed without an answer: the refusal puts the login back on the lobby the player came from
    until(&mut m, "the refusal", |m| m.data.modded_join.is_none());
    assert_eq!(m.data.auth, Some(home));

    // leaving a joined lobby: the server list again logs into the first lobby with the token
    m.data.modded_join = Some("Central Modded".into());
    m.data.home = Some(dc_client::menu::moddedrows::Home {
        auth: Some(home),
        pin: None,
        picked: None,
    });
    m.data.auth = Some(modded);
    m.data.lobby_up = true;
    m.open(dc_client::menu::Go::Servers);
    until(&mut m, "the login at home", |m| {
        m.link_latest_request().starts_with(&format!("Login({home}, tester"))
    });
    assert_eq!(m.data.auth, Some(home));
    assert!(m.data.modded_join.is_none() && m.data.home.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_popup_buttons_lie_inside_their_frame() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let inside = |f: [i32; 4], b: [i32; 4]| {
        b[0] >= f[0] + 8
            && b[1] >= f[1] + 8
            && b[0] + b[2] <= f[0] + f[2] - 8
            && b[1] + b[3] <= f[1] + f[3] - 8
    };
    let mut m = menu(None, "127.0.0.1:1".parse().unwrap());
    m.input(MenuInput::Command(login::CMD_HOMEPAGE));
    save_shot(&mut m, "server_browser_1024x768.png");
    let (frame, buttons) = m.layout_rects().expect("the browser's layout");
    assert_eq!(buttons.len(), 3);
    for b in &buttons {
        assert!(inside(frame, *b), "browser button {b:?} outside {frame:?}");
    }
    m.input(MenuInput::Key(dc_client::menu::MenuKey::Escape));
    let fragment = serde_json::json!({"policy": "modded", "mods": [{
        "id": "tmod", "version": "1.0.0", "sha256": "cd".repeat(32), "kind": "shared", "requires_client": true,
        "capabilities": ["content.weapons"]}]});
    let set = dc_client::mods::ServerModSet::from_fragment(&fragment).unwrap();
    m.feed_event(LinkEvent::Mods(dc_client::mods::ModState::NeedsConsent(set)));
    m.update();
    assert!(has(&texts(&mut m), T_CONSENT_TITLE));
    save_shot(&mut m, "consent_1024x768.png");
    let (frame, buttons) = m.layout_rects().expect("the prompt's layout");
    assert_eq!(buttons.len(), 2);
    for b in &buttons {
        assert!(inside(frame, *b), "prompt button {b:?} outside {frame:?}");
    }
}

// ------------------------------------------------------------------ CLI-1617 / CLI-1623 amended: the Mods screen

/// Packs example mod `id` with dc-modkit into the by-hash cache of `root` and unpacks it there; its sha256.
fn cached_example(root: &Path, id: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods").join(id);
    let (zip, entry) = dc_modkit::pack_mod_bytes(&src).expect("the example packs");
    let paths = dc_client::mods::ModPaths::new(root);
    std::fs::create_dir_all(paths.by_hash()).unwrap();
    std::fs::write(paths.zip(&entry.sha256), &zip).unwrap();
    dc_client::mods::unpack_package(&zip, &paths.unpacked(&entry.sha256), 1 << 24).unwrap();
    entry.sha256
}

/// Builds an in-test client_visual crosshair mod (id `test_crosshair`, name "Test Crosshair") with a generated
/// texture, packs it into the by-hash cache of `root` and unpacks it there; its sha256.
fn cached_crosshair(root: &Path) -> String {
    let src = root.join("src_test_crosshair");
    std::fs::create_dir_all(src.join("assets")).unwrap();
    std::fs::create_dir_all(src.join("screenshots")).unwrap();
    std::fs::write(
        src.join("manifest.json"),
        r#"{"id": "test_crosshair", "name": "Test Crosshair", "version": "1.0.0", "kind": "client_visual",
        "capabilities": ["content.hud", "assets"], "description": "A test mod: a crosshair for the HUD override keys.",
        "author": "DeCombat", "tags": ["hud", "test"], "screenshots": ["screenshots/main.png"]}"#,
    )
    .unwrap();
    std::fs::write(
        src.join("main.lua"),
        "return function(mod)
    mod.content.hud:override(\"crosshair\", mod.assets:path(\"assets/crosshair.dtx\"))
end
",
    )
    .unwrap();
    let rgba: Vec<u8> = (0..32 * 32).flat_map(|_| [255u8, 255, 255, 255]).collect();
    std::fs::write(src.join("assets/crosshair.dtx"), dc_tex::write_bgra8(32, 32, &rgba).unwrap()).unwrap();
    image::RgbaImage::from_pixel(16, 9, image::Rgba([30, 40, 60, 255]))
        .save(src.join("screenshots/main.png"))
        .unwrap();
    let (zip, entry) = dc_modkit::pack_mod_bytes(&src).expect("the fixture packs");
    let paths = dc_client::mods::ModPaths::new(root);
    std::fs::create_dir_all(paths.by_hash()).unwrap();
    std::fs::write(paths.zip(&entry.sha256), &zip).unwrap();
    dc_client::mods::unpack_package(&zip, &paths.unpacked(&entry.sha256), 1 << 24).unwrap();
    entry.sha256
}

#[test]
fn the_mods_screen_shows_the_selected_servers_set_the_downloads_and_the_own_mods_without_addresses() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("screen");
    let xray = cached_example(&root, "hacking_mod");
    let green = cached_crosshair(&root);
    let doc = parse_community(&format!(
        r#"{{"schema_version": 1, "updated": "2026-11-01T12:00:00Z",
        "servers": [
          {{"id": "vps", "name": "DeCombat VPS", "region": "na", "address": "127.0.0.1:10000",
            "policy": "vanilla", "mods": []}},
          {{"id": "vps-modded", "name": "DeCombat VPS Modded", "region": "na", "address": "127.0.0.1:11000",
            "policy": "modded",
            "mods": [{{"id": "hacking_mod", "version": "1.1.1", "sha256": "{xray}"}},
                     {{"id": "test_crosshair", "version": "1.0.0", "sha256": "{green}"}}]}}
        ], "known_mods": []}}"#
    ))
    .expect("the fixture document is valid");
    let view = CommunityView {
        addrs: vec![
            Some("127.0.0.1:10000".parse().unwrap()),
            Some("127.0.0.1:11000".parse().unwrap()),
        ],
        doc: Some(doc),
        cached: false,
        error: None,
    };
    let mut m = menu(Some(&root), "127.0.0.1:10000".parse().unwrap());
    m.load_settings(store(&root));
    let xray_key = format!("hacking_mod@1.1.1:{xray}");
    m.data.settings.mods_accepted = vec![xray_key.clone()];
    m.feed_event(LinkEvent::Community(view));
    m.feed_event(LinkEvent::OtherLobbies(vec![other(
        "Modded One",
        "US West",
        "127.0.0.1:11000".parse().unwrap(),
    )]));
    m.feed_event(LinkEvent::Servers(vanilla_list()));
    m.update();
    let no_address = |t: &[String], what: &str| {
        let shown: Vec<&String> = t.iter().filter(|s| has_ipv4(s)).collect();
        assert!(shown.is_empty(), "an address is on the Mods screen ({what}): {shown:?}");
    };

    // the vanilla lobby's own entry highlighted (the automatic pick): the context line, one row per package
    m.open(dc_client::menu::Go::Servers);
    m.input(MenuInput::Command(CMD_MODS));
    assert_eq!(m.screen_name(), "mods");
    let t = texts(&mut m);
    for want in [
        "US Central (North America)",
        T_CTX_VANILLA,
        "Test Crosshair",
        "Hacking Mod",
        "Required by",
        "Version 1.1.1 by DeCombat",
        "Open folder",
        "Remove",
    ] {
        assert!(has(&t, want), "missing {want:?} in {t:?}");
    }
    no_address(&t, "vanilla");
    m.input(MenuInput::Command(CMD_ROW_BASE + 1));
    let t = texts(&mut m);
    assert!(!has(&t, "Revoke trust"), "only in the detail of an accepted mod: {t:?}");
    m.input(MenuInput::Command(CMD_ROW_BASE));
    let t = texts(&mut m);
    assert!(has(&t, "Revoke trust") && has(&t, "Accepted"), "{t:?}");
    assert!(detail_text(&mut m).contains("Required by DeCombat VPS Modded."));
    m.input(MenuInput::Key(dc_client::menu::MenuKey::Escape));
    assert_eq!(m.screen_name(), "servers");

    // the modded row (after the three entries of this lobby): marks, capabilities, the description wrapped in full
    for _ in 0..3 {
        m.input(MenuInput::Key(dc_client::menu::MenuKey::Down));
    }
    m.input(MenuInput::Command(CMD_MODS));
    let t = texts(&mut m);
    for want in ["Modded One (US West)", T_CTX_MODDED] {
        assert!(has(&t, want), "missing {want:?} in {t:?}");
    }
    assert!(!has(&t, T_CTX_VANILLA));
    no_address(&t, "modded");
    m.input(MenuInput::Command(CMD_ROW_BASE + 1));
    assert!(has(&texts(&mut m), "Unverified"), "the crosshair's mark");
    m.input(MenuInput::Command(CMD_ROW_BASE));
    let all = detail_text(&mut m);
    for want in [
        "Accepted",
        "What it does",
        "Lets rooms switch recoil and spread off",
        "Everyone is visible through walls",
        "Everyone on this server hacks",
        "nobody is surprised",
    ] {
        assert!(all.contains(want), "missing {want:?} in {all}");
    }

    // Revoke trust: the pref is gone (saved), the mark falls back to Unverified
    m.input(MenuInput::Command(CMD_FORGET_BASE));
    assert!(m.data.settings.mods_accepted.is_empty());
    assert!(m.link_latest_request().starts_with("ModPrefs("), "{}", m.link_latest_request());
    let profile = std::fs::read_to_string(root.join("player.txt")).unwrap();
    assert!(!profile.contains(&xray_key), "{profile}");
    let t = texts(&mut m);
    assert!(!has(&t, "Accepted") && !has(&t, "Revoke trust") && has(&t, "Unverified"), "{t:?}");

    // Remove: the second package (Test Crosshair by name) loses its zip and unpacked folder
    let paths = dc_client::mods::ModPaths::new(&root);
    m.input(MenuInput::Command(CMD_REMOVE_BASE + 1));
    assert!(!paths.zip(&green).exists() && !paths.unpacked(&green).exists());
    assert!(paths.zip(&xray).exists() && paths.unpacked(&xray).exists());
    let t = texts(&mut m);
    assert!(!has(&t, "Test Crosshair"), "{t:?}");
    no_address(&t, "after the removal");
    // the server still names it: it is on Browse now, to come with the join
    m.input(MenuInput::Command(CMD_TAB_BROWSE));
    let t = texts(&mut m);
    assert!(has(&t, "test_crosshair") && has(&t, "With server"), "the set still names it: {t:?}");
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------ CLI-1632: superseded and unused packages

/// An older package of mod `id`: the unpacked copy of cached package `from` with its id (and name) renamed, under
/// the hash of dummy zip bytes from `seed`; its sha256.
fn older_package(root: &Path, from: &str, from_id: &str, id: &str, seed: u8) -> String {
    let paths = dc_client::mods::ModPaths::new(root);
    let body = vec![seed; 128];
    let sha = dc_rom::sha256_hex(&body);
    std::fs::write(paths.zip(&sha), &body).unwrap();
    let dir = paths.unpacked(&sha);
    std::fs::create_dir_all(dir.join("screenshots")).unwrap();
    let man = std::fs::read_to_string(paths.unpacked(from).join("manifest.json")).unwrap();
    let man = man.replace(from_id, id).replace("Test Crosshair", "Green Crosshair");
    std::fs::write(dir.join("manifest.json"), man).unwrap();
    sha
}

#[test]
fn an_updated_mod_prunes_its_old_package_and_clean_up_removes_the_retired_one() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("prune");
    let xray = cached_example(&root, "hacking_mod");
    let blue = cached_crosshair(&root);
    let xray_old = older_package(&root, &xray, "hacking_mod", "hacking_mod", 1);
    let xray_other = older_package(&root, &xray, "hacking_mod", "hacking_mod", 2);
    let green = older_package(&root, &blue, "test_crosshair", "green_crosshair", 3);
    let local = root.join("mods").join("local");
    local_mod(&root, "glow", "client_visual");
    let doc = parse_community(&format!(
        r#"{{"schema_version": 1, "updated": "2026-11-01T12:00:00Z",
        "servers": [
          {{"id": "vps-modded", "name": "DeCombat VPS Modded", "region": "na", "address": "127.0.0.1:11000",
            "policy": "modded",
            "mods": [{{"id": "hacking_mod", "version": "1.1.1", "sha256": "{xray}"}},
                     {{"id": "test_crosshair", "version": "1.0.0", "sha256": "{blue}"}}]}},
          {{"id": "other", "name": "Other Modded", "region": "eu", "address": "127.0.0.1:12000",
            "policy": "modded",
            "mods": [{{"id": "hacking_mod", "version": "1.1.1", "sha256": "{xray_other}"}}]}}
        ], "known_mods": []}}"#
    ))
    .expect("the fixture document is valid");
    let view = CommunityView {
        addrs: vec![
            Some("127.0.0.1:11000".parse().unwrap()),
            Some("127.0.0.1:12000".parse().unwrap()),
        ],
        doc: Some(doc),
        cached: false,
        error: None,
    };
    let mut m = menu(Some(&root), "127.0.0.1:11000".parse().unwrap());
    m.load_settings(store(&root));
    let old_key = format!("hacking_mod@1.1.1:{xray_old}");
    let new_key = format!("hacking_mod@1.1.1:{xray}");
    m.data.settings.mods_accepted = vec![old_key.clone(), new_key.clone()];
    m.feed_event(LinkEvent::Community(view));
    m.feed_event(LinkEvent::OtherLobbies(Vec::new()));
    m.update();
    let paths = dc_client::mods::ModPaths::new(&root);
    let set = dc_client::mods::ServerModSet::from_fragment(&serde_json::json!({
        "policy": "modded", "client_visual_allowed": true,
        "mods": [
          {"id": "hacking_mod", "version": "1.1.1", "sha256": xray, "kind": "shared", "requires_client": true},
          {"id": "test_crosshair", "version": "1.0.0", "sha256": blue, "kind": "client_visual", "requires_client": true}
        ]}))
    .unwrap();
    let report = dc_client::mods::JoinReport {
        state: dc_client::mods::ModState::Ready,
        set: Some(set),
        ..Default::default()
    };

    // the join of the modded lobby: its old x-ray package goes with its consent key; the one another server still
    // declares, the retired green one (not a week old) and the local mod stay
    let now = std::time::SystemTime::now();
    m.prune_mod_cache(&report, now);
    assert!(!paths.zip(&xray_old).exists() && !paths.unpacked(&xray_old).exists());
    for sha in [&xray, &blue, &xray_other, &green] {
        assert!(paths.zip(sha).is_file(), "{sha} stays");
    }
    assert!(local.join("glow").join("manifest.json").is_file());
    assert_eq!(m.data.settings.mods_accepted, vec![new_key.clone()]);
    assert!(m.link_latest_request().starts_with("ModPrefs("), "{}", m.link_latest_request());
    let profile = std::fs::read_to_string(root.join("player.txt")).unwrap();
    assert!(!profile.contains(&old_key) && profile.contains(&new_key), "{profile}");

    // a week later the retired id goes too
    m.prune_mod_cache(&report, now + dc_client::mods::UNUSED_MAX_AGE + Duration::from_secs(60));
    assert!(!paths.zip(&green).exists() && !paths.unpacked(&green).exists());
    assert!(paths.zip(&xray_other).is_file());

    // Clean up on the Mods screen: a retired package goes at once, the declared ones and the own mod stay
    let green = older_package(&root, &blue, "test_crosshair", "green_crosshair", 4);
    m.open(dc_client::menu::Go::Mods);
    assert_eq!(m.screen_name(), "mods");
    let t = texts(&mut m);
    assert!(has(&t, "Green Crosshair") && has(&t, "Remove unused (1)"), "{t:?}");
    m.input(MenuInput::Command(CMD_CLEAN_UP));
    assert!(!paths.zip(&green).exists() && !paths.unpacked(&green).exists());
    for sha in [&xray, &blue, &xray_other] {
        assert!(paths.zip(sha).is_file(), "{sha} stays");
    }
    let t = texts(&mut m);
    assert!(!has(&t, "Green Crosshair"), "{t:?}");
    assert!(!has(&t, "Remove unused"), "shown only when there is something to remove: {t:?}");
    assert!(has(&t, "Test Crosshair"), "{t:?}");
    assert!(has(&t, "glow"), "the own mod is listed: {t:?}");
    // the two copies of the x-ray mod are one row, naming both servers that need them
    assert_eq!(t.iter().filter(|x| x.as_str() == "Hacking Mod").count(), 1, "{t:?}");
    m.input(MenuInput::Command(CMD_ROW_BASE + 1));
    let all = detail_text(&mut m);
    assert!(all.contains("DeCombat VPS Modded") && all.contains("Other Modded"), "{all}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A set declaring the cached package `sha` of `id`.
fn set_of(id: &str, sha: &str) -> dc_client::mods::ServerModSet {
    dc_client::mods::ServerModSet::from_fragment(&serde_json::json!({
        "policy": "modded", "client_visual_allowed": false,
        "mods": [{"id": id, "version": "1.0.0", "sha256": sha, "kind": "shared", "requires_client": true}]}))
    .unwrap()
}

/// `<data>/known_mod_sets.json` holding one server, seen `age` ago.
fn persisted(root: &Path, addr: SocketAddr, name: &str, set: &dc_client::mods::ServerModSet, age: Duration) {
    dc_client::knownsets::save(
        root,
        &[dc_client::knownsets::KnownSet {
            addr,
            name: name.into(),
            set: set.clone(),
            seen: std::time::SystemTime::now() - age,
        }],
    );
}

fn mine(m: &mut Menu) -> String {
    m.open(dc_client::menu::Go::Mods);
    m.input(MenuInput::Command(CMD_TAB_MINE));
    texts(m).join(" | ")
}

/// USER REPORT 2026-11-01: a fresh run with a kept set says Required by; Remove unused leaves the required package.
#[test]
fn a_fresh_run_with_a_persisted_set_shows_required_by_and_remove_unused_keeps_it() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("kept_set");
    let xray = cached_example(&root, "hacking_mod");
    let old = older_package(&root, &xray, "hacking_mod", "old_retired", 7);
    let server: SocketAddr = "127.0.0.1:11000".parse().unwrap();
    persisted(&root, server, "VPS Modded", &set_of("hacking_mod", &xray), Duration::from_secs(60));
    let mut m = menu(Some(&root), "127.0.0.1:1".parse().unwrap());
    m.load_settings(store(&root));
    m.feed_event(LinkEvent::OtherLobbies(vec![other("VPS Modded", "", server)]));
    m.update();
    let t = mine(&mut m);
    assert!(t.contains("Required by VPS Modded") && t.contains("Remove unused (1)"), "{t}");
    assert!(!t.contains("127.0.0.1"), "{t}");
    m.input(MenuInput::Command(CMD_CLEAN_UP));
    let paths = dc_client::mods::ModPaths::new(&root);
    assert!(paths.zip(&xray).is_file(), "the required package stays");
    assert!(!paths.zip(&old).exists(), "a package no known server declares is still removable");
    let _ = std::fs::remove_dir_all(&root);
}

/// The server list's background read fills a modded row with no join; an unreachable row keeps what was known.
#[test]
fn opening_the_server_list_reads_the_rows_sets_and_no_answer_keeps_the_old_one() {
    // needs the ROM's screen layouts (or body models): CI has none
    if dc_rom::resolve_rom().is_err() {
        println!("skipped: CA_ROM unset");
        return;
    }
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("row_read");
    let xray = cached_example(&root, "hacking_mod");
    let set = set_of("hacking_mod", &xray);
    let routes: Routes = Arc::default();
    let body = serde_json::json!({"name": "t", "mod_set": set.to_fragment(), "mod_set_hash": set.hash()});
    routes.lock().unwrap().insert("/server_list.json".into(), (200, body.to_string().into_bytes()));
    let (_a, live) = lobby(routes, Arc::default());
    // a port nothing listens on: the row is unreachable, its package known from an earlier run
    let dead: SocketAddr = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let dead_set = set_of("other_mod", &"ee".repeat(32));
    persisted(&root, dead, "Dead Row", &dead_set, Duration::from_secs(60));

    let mut m = menu(Some(&root), "127.0.0.1:1".parse().unwrap());
    m.load_settings(store(&root));
    m.feed_event(LinkEvent::OtherLobbies(vec![other("Live Row", "", live), other("Dead Row", "", dead)]));
    m.feed_event(LinkEvent::Servers(vanilla_list()));
    m.update();
    // before any answer the package is not called unused and cannot be removed
    let t = mine(&mut m);
    assert!(t.contains("Kept") && !t.contains("Unused") && !t.contains("Remove unused"), "{t}");
    m.open(dc_client::menu::Go::Servers);
    until(&mut m, "the live row's mods in the panel", |m| has(&texts(m), "hacking_mod"));
    let file = dc_client::knownsets::load(&root, std::time::SystemTime::now());
    assert_eq!(file.iter().find(|k| k.addr == live).map(|k| k.set.clone()), Some(set));
    // the failed read of the dead row changes nothing, whenever it ends
    m.feed_event(LinkEvent::RowSet { addr: dead, name: "Dead Row".into(), set: Err("no answer".into()) });
    m.update();
    let file = dc_client::knownsets::load(&root, std::time::SystemTime::now());
    assert_eq!(file.iter().find(|k| k.addr == dead).map(|k| k.set.clone()), Some(dead_set));
    let t = mine(&mut m);
    assert!(t.contains("Required by Live Row"), "{t}");
    assert!(!t.contains("127.0.0.1"), "{t}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A row with no answer and nothing kept: nothing is pruned or removable; once known the rules apply again, also the
/// supersede rule against the joined lobby's own stale note; a set past 30 days stops counting.
#[test]
fn an_unknown_row_holds_every_prune_and_a_stale_or_expired_note_does_not() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("hold");
    let new = cached_example(&root, "hacking_mod");
    let old = older_package(&root, &new, "hacking_mod", "hacking_mod", 8);
    let joined_addr: SocketAddr = "127.0.0.1:11000".parse().unwrap();
    let row: SocketAddr = "127.0.0.1:12000".parse().unwrap();
    // the joined lobby's own note from an earlier run still names the old package
    persisted(&root, joined_addr, "Joined", &set_of("hacking_mod", &old), Duration::from_secs(60));
    let mut m = menu(Some(&root), joined_addr);
    m.load_settings(store(&root));
    let paths = dc_client::mods::ModPaths::new(&root);
    let report = dc_client::mods::JoinReport {
        state: dc_client::mods::ModState::Ready,
        set: Some(set_of("hacking_mod", &new)),
        ..Default::default()
    };
    let now = std::time::SystemTime::now();
    // the list of other lobbies is not read yet, then a row is unknown: the old package stays
    m.prune_mod_cache(&report, now);
    assert!(paths.zip(&old).is_file());
    m.feed_event(LinkEvent::OtherLobbies(vec![other("Row", "", row)]));
    m.update();
    m.prune_mod_cache(&report, now);
    assert!(paths.zip(&old).is_file());
    // the row is known and declares something else: the joined lobby's stale note keeps nothing alive
    let other_sha = "dd".repeat(32);
    m.feed_event(LinkEvent::RowSet { addr: row, name: "Row".into(), set: Ok(set_of("other_mod", &other_sha)) });
    m.update();
    m.prune_mod_cache(&report, now);
    assert!(!paths.zip(&old).exists(), "superseded");
    assert!(paths.zip(&new).is_file());
    // 31 days on, the row's set stopped counting: unknown again, so nothing goes even past the week's age
    let retired = older_package(&root, &new, "hacking_mod", "retired_mod", 9);
    let later = now + Duration::from_secs(31 * 24 * 3600);
    m.prune_mod_cache(&report, later);
    assert!(paths.zip(&retired).is_file());
    // and an unlisted server's set past 30 days no longer keeps its package
    m.feed_event(LinkEvent::RowSet { addr: row, name: "Row".into(), set: Ok(set_of("retired_mod", &retired)) });
    m.feed_event(LinkEvent::OtherLobbies(Vec::new()));
    m.update();
    m.prune_mod_cache(&report, now);
    assert!(paths.zip(&retired).is_file(), "declared");
    m.prune_mod_cache(&report, later);
    assert!(!paths.zip(&retired).exists(), "expired and a week unused");
    let _ = std::fs::remove_dir_all(&root);
}

/// A corrupt file changes nothing but the knowledge: the row is unknown, nothing is called unused.
#[test]
fn a_corrupt_known_sets_file_is_ignored() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("corrupt_sets");
    let _xray = cached_example(&root, "hacking_mod");
    std::fs::write(root.join(dc_client::knownsets::FILE), "{broken").unwrap();
    let mut m = menu(Some(&root), "127.0.0.1:1".parse().unwrap());
    m.load_settings(store(&root));
    assert!(m.data.known_sets.is_empty());
    m.feed_event(LinkEvent::OtherLobbies(vec![other("Row", "", "127.0.0.1:12000".parse().unwrap())]));
    m.update();
    let t = mine(&mut m);
    assert!(t.contains("Kept") && !t.contains("Unused"), "{t}");
    let _ = std::fs::remove_dir_all(&root);
}

fn crosshair_package() -> (Vec<u8>, dc_modkit::ModSetEntry) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/drawn_crosshair");
    dc_modkit::pack_mod_bytes(&src).unwrap()
}

fn catalog_doc(e: &dc_modkit::ModSetEntry, sha: &str, size: usize) -> Vec<u8> {
    serde_json::json!({"version": 1, "mods": [{"id": e.id, "version": e.version.to_string(), "sha256": sha,
        "name": "Custom Crosshair", "author": "DeCombat", "description": "A drawn crosshair for the test.",
        "size": size, "screenshot": false}]})
    .to_string()
    .into_bytes()
}

/// CLI-1634: Get mods lists the lobby's optional mod; Download installs it disabled; Settings persists; Remove deletes.
#[test]
fn get_mods_downloads_disabled_settings_persist_and_remove_deletes() {
    // needs the ROM's screen layouts (or body models): CI has none
    if dc_rom::resolve_rom().is_err() {
        println!("skipped: CA_ROM unset");
        return;
    }
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("get");
    let (zip, entry) = crosshair_package();
    let routes: Routes = Arc::default();
    routes.lock().unwrap().insert(
        "/mods/catalog.json".into(),
        (200, catalog_doc(&entry, &entry.sha256, zip.len())),
    );
    routes
        .lock()
        .unwrap()
        .insert(format!("/mods/{}.zip", entry.sha256), (200, zip));
    let (_auth_l, auth) = lobby(routes, Arc::default());
    let mut m = menu(Some(&root), auth);
    m.load_settings(store(&root));
    m.update();
    m.input(MenuInput::Command(login::CMD_HOMEPAGE));
    m.input(MenuInput::Command(CMD_MODS));
    until(&mut m, "the list", |m| texts(m).iter().any(|x| x == T_DOWNLOAD));
    let t = texts(&mut m);
    assert!(has(&t, T_BROWSE) && has(&t, "Custom Crosshair") && has(&t, "Version 1.1.0 by DeCombat"), "{t:?}");
    assert!(has(&t, "Install") && has(&t, "KB"), "{t:?}");
    assert!(has(&t, T_ORIGIN_OPTIONAL) && !has(&t, T_ORIGIN_YOURS), "a catalogue entry not installed yet: {t:?}");
    assert!(!t.iter().any(|x| x.contains("127.0.0.1")), "no address on screen: {t:?}");
    let dir = root.join("mods/local/drawn_crosshair");
    assert!(!dir.exists(), "nothing is installed by itself");

    m.input(MenuInput::Command(CMD_DOWNLOAD_BASE));
    until(&mut m, "the install", |m| has(&texts(m), T_INSTALLED));
    assert!(dir.join("manifest.json").is_file());
    assert!(m.data.settings.mods_local_enabled.is_empty(), "installed disabled");
    let t = texts(&mut m);
    assert!(has(&t, "Enable") && has(&t, T_SETTINGS) && has(&t, "Remove"), "{t:?}");
    assert!(!t.iter().any(|x| x == "Install" || x == T_DOWNLOAD), "{t:?}");
    assert!(has(&t, T_ORIGIN_YOURS) && !has(&t, T_ORIGIN_OPTIONAL), "installed: it is the player's own now: {t:?}");

    // the page: the style cycler, the recoil toggle, a palette cell; saved in the profile, a reload keeps them
    m.input(MenuInput::Command(CMD_SETTINGS_BASE));
    let t = texts(&mut m);
    for want in ["Shape", "Colour", "Recoil", "Style", "Length", "Thickness", "Gap", "Circle", "Dynamic", "Opacity", T_APPLIES, T_RESET, T_COPY, T_PASTE] {
        assert!(has(&t, want), "missing {want:?} in {t:?}");
    }
    let drawn = |m: &mut Menu| {
        m.items_at(0.5)
            .into_iter()
            .find_map(|i| match i {
                DrawItem::Crosshair(c) => Some(c),
                _ => None,
            })
            .expect("the preview")
    };
    let burst = drawn(&mut m);
    let calm = m
        .items_at(2.9)
        .into_iter()
        .find_map(|i| match i {
            DrawItem::Crosshair(c) => Some(c),
            _ => None,
        })
        .unwrap();
    assert!(burst.ring_radius() > calm.ring_radius(), "the preview fires a burst");
    if let Some(dir) = std::env::var_os("DC_CROSSHAIR_SHOTS") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img = m.image();
        image::save_buffer(dir.join("settings_page.png"), &img.rgba, img.width, img.height, image::ColorType::Rgba8).unwrap();
    }
    // the rows' places: 0 style, 1 length, 2 thickness, 3 gap, 4 dot, 5 T, 6 outline, 7 its width, 8 colour, 9 recoil
    m.input(MenuInput::Command(CMD_PAGE_STEP_BASE));
    assert_eq!(drawn(&mut m).style.name(), "dot");
    m.input(MenuInput::Command(CMD_PAGE_STEP_BASE + 9));
    m.input(MenuInput::Command(CMD_PAGE_COLOUR_BASE + 8 * 100)); // the palette's first cell, red
    assert_eq!(drawn(&mut m).colour, [0xff, 0x30, 0x30, 0xff]);
    let t = texts(&mut m);
    assert!(has(&t, "Static") && has(&t, "Dot"), "{t:?}");
    m.input(MenuInput::Command(CMD_PAGE_STEP_BASE + 9));
    assert!(has(&texts(&mut m), "On fire only"));
    m.input(MenuInput::Command(CMD_PAGE_STEP_BASE + 9));
    m.input(MenuInput::Command(CMD_PAGE_STEP_BASE + 9));
    assert!(has(&texts(&mut m), "Static"));

    // the sliders: 0 length, 1 thickness, 2 gap, 3 outline width, 4..7 red green blue opacity, 8 recoil scale
    let sliders = m.slider_rects();
    assert_eq!(sliders.len(), 9);
    // a click on the track's middle, and a drag past its end
    let mid = |r: [i32; 4], f: f32| [r[0] as f32 + r[2] as f32 * f, r[1] as f32 + r[3] as f32 / 2.0];
    m.input(MenuInput::Press(mid(sliders[0], 0.5)));
    m.input(MenuInput::Release(mid(sliders[0], 0.5)));
    assert_eq!(m.data.settings.mods_settings["drawn_crosshair"]["length"], "20.5");
    assert_eq!(drawn(&mut m).length, 20.5 + 6.0 * (1.0 - (0.5f32 % 0.1) / 0.1));
    m.input(MenuInput::Press(mid(sliders[2], 0.0)));
    assert_eq!(m.data.settings.mods_settings["drawn_crosshair"]["gap"], "0", "a held drag stores at its end");
    m.input(MenuInput::Move(mid(sliders[2], 0.25)));
    m.input(MenuInput::Move([0.0, 0.0]));
    assert_eq!(texts(&mut m).iter().filter(|x| *x == "-20").count(), 1, "the value shows at once");
    m.input(MenuInput::Release([0.0, 0.0]));
    assert_eq!(m.data.settings.mods_settings["drawn_crosshair"]["gap"], "-20");
    // the fine step buttons
    m.input(MenuInput::Command(CMD_PAGE_NUDGE_BASE + 2 * 2 + 1));
    assert_eq!(m.data.settings.mods_settings["drawn_crosshair"]["gap"], "-19.5");
    m.input(MenuInput::Command(CMD_PAGE_NUDGE_BASE + 2 * 7));
    assert_eq!(m.data.settings.mods_settings["drawn_crosshair"]["colour"], "#ff3030fe");
    assert_eq!(drawn(&mut m).colour, [0xff, 0x30, 0x30, 0xfe]);
    m.input(MenuInput::Command(CMD_PAGE_COLOUR_BASE + 8 * 100 + 1));
    assert_eq!(drawn(&mut m).colour[3], 0xfe, "a preset keeps the opacity");

    // the share code: copy through the button and the chord, a bad code changes nothing, a good one is applied
    #[derive(Default)]
    struct Clip(Option<String>);
    impl dc_client::menu::clipboard::Clipboard for Clip {
        fn get(&mut self) -> Option<String> {
            self.0.clone()
        }
        fn set(&mut self, text: &str) {
            self.0 = Some(text.to_string());
        }
    }
    let mut clip = Clip::default();
    m.input(MenuInput::Command(CMD_PAGE_COPY));
    m.pump_clipboard(&mut clip);
    let code = clip.0.clone().expect("the button copies");
    assert!(code.starts_with("DCS-") && has(&texts(&mut m), "copied"), "{code}");
    assert_eq!(m.copy_text().as_deref(), Some(code.as_str()), "Ctrl+C copies it too");
    let saved = m.data.settings.mods_settings["drawn_crosshair"].clone();
    clip.0 = Some("DCS-HELLO-WORLD".into());
    m.input(MenuInput::Command(CMD_PAGE_PASTE));
    m.pump_clipboard(&mut clip);
    assert!(has(&texts(&mut m), "Nothing changed"));
    assert_eq!(m.data.settings.mods_settings["drawn_crosshair"], saved);
    // reset gives the first version's look and the stored defaults
    m.input(MenuInput::Command(CMD_PAGE_RESET));
    let c = drawn(&mut m);
    assert_eq!((c.style.name(), c.colour, c.outline, c.t_style, c.dot), ("circle", [0x40, 0x80, 0xff, 0xff], 0.0, false, false));
    assert_eq!(m.data.settings.mods_settings["drawn_crosshair"]["length"], "7");
    clip.0 = Some(code.clone());
    m.input(MenuInput::Command(CMD_PAGE_PASTE));
    m.pump_clipboard(&mut clip);
    assert_eq!(m.data.settings.mods_settings["drawn_crosshair"], saved, "the code brings every value back");
    assert!(has(&texts(&mut m), "applied"));
    m.input(MenuInput::Paste("nonsense".into())); // Ctrl+V
    assert_eq!(m.data.settings.mods_settings["drawn_crosshair"], saved);
    // set the values the rest of this test expects
    m.input(MenuInput::Command(CMD_PAGE_NUDGE_BASE + 2 * 7 + 1));
    m.input(MenuInput::Command(CMD_PAGE_COLOUR_BASE + 8 * 100));
    m.input(MenuInput::Command(CMD_PAGE_BACK));
    let profile = std::fs::read_to_string(root.join("player.txt")).unwrap();
    assert!(profile.contains("mods.setting.drawn_crosshair.style = dot"), "{profile}");
    let mut again = menu(Some(&root), auth);
    again.load_settings(store(&root));
    let got = &again.data.settings.mods_settings["drawn_crosshair"];
    assert_eq!(got["recoil"], "false");
    assert_eq!(got["colour"], "#ff3030");
    assert_eq!(got["gap"], "-19.5");
    drop(again);

    m.input(MenuInput::Command(CMD_TOGGLE_BASE));
    assert_eq!(m.data.settings.mods_local_enabled, vec!["drawn_crosshair".to_string()]);
    m.input(MenuInput::Command(CMD_REMOVE_LOCAL_BASE));
    assert!(!dir.exists(), "Remove deletes the folder");
    assert!(m.data.settings.mods_local_enabled.is_empty(), "and its enable entry");
    assert!(texts(&mut m).iter().any(|x| x == T_DOWNLOAD));
}

/// CLI-1634: a package that does not hash to its entry shows the reason and installs nothing; no catalogue is a line.
#[test]
fn a_mismatched_download_installs_nothing_and_no_catalogue_is_a_plain_line() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("mismatch");
    let (zip, entry) = crosshair_package();
    let wrong = "ab".repeat(32);
    let routes: Routes = Arc::default();
    routes.lock().unwrap().insert(
        "/mods/catalog.json".into(),
        (200, catalog_doc(&entry, &wrong, zip.len())),
    );
    routes.lock().unwrap().insert(format!("/mods/{wrong}.zip"), (200, zip));
    let (_auth_l, auth) = lobby(Arc::clone(&routes), Arc::default());
    let mut m = menu(Some(&root), auth);
    m.load_settings(store(&root));
    m.input(MenuInput::Command(login::CMD_HOMEPAGE));
    m.input(MenuInput::Command(CMD_MODS));
    until(&mut m, "the list", |m| texts(m).iter().any(|x| x == T_DOWNLOAD));
    m.input(MenuInput::Command(CMD_DOWNLOAD_BASE));
    until(&mut m, "the refusal", |m| has(&texts(m), "does not match the hash"));
    assert!(!root.join("mods/local/drawn_crosshair").exists());
    drop(m);

    routes.lock().unwrap().clear();
    let mut m = menu(Some(&root), auth);
    m.load_settings(store(&root));
    m.input(MenuInput::Command(login::CMD_HOMEPAGE));
    m.input(MenuInput::Command(CMD_MODS));
    until(&mut m, "the line", |m| has(&texts(m), "offers no optional mods"));
}

// ------------------------------------------------------------------ CLI-1617 redesign: the window's geometry

/// A text item's box in virtual pixels, its anchor point and its text.
struct Words {
    text: String,
    anchor: [f32; 2],
    pos: [f32; 2],
    bx: [f32; 4],
}

fn words(m: &mut Menu) -> Vec<Words> {
    m.items()
        .into_iter()
        .filter_map(|i| match i {
            DrawItem::Label { style, pos, text, anchor } => {
                let (tw, th) = m
                    .cat
                    .measure_text(style.font, style.size.unwrap_or(0), &text)
                    .unwrap_or((text.chars().count() as f32 * 7.0, 16.0));
                Some(Words {
                    bx: [pos[0] - anchor[0] * tw, pos[1] - anchor[1] * th, tw, th],
                    text,
                    anchor,
                    pos,
                })
            }
            _ => None,
        })
        .collect()
}

fn within(b: [f32; 4], r: [i32; 4]) -> bool {
    b[0] >= r[0] as f32 - 0.5
        && b[1] >= r[1] as f32 - 2.0
        && b[0] + b[2] <= (r[0] + r[2]) as f32 + 0.5
        && b[1] + b[3] <= (r[1] + r[3]) as f32 + 2.0
}

/// Every button's caption is centred on its rectangle within 1 px and fits; every text lies in a row, the detail
/// panel, a button or the header band; no mod has two rows.
fn check_geometry(m: &mut Menu, what: &str, rows: &[&str]) {
    let (frame, buttons) = m.layout_rects().expect("the window's layout");
    let all = words(m);
    for b in &buttons {
        let caption = all.iter().find(|w| {
            w.anchor == [0.5, 0.5]
                && w.pos[0] >= b[0] as f32
                && w.pos[1] >= b[1] as f32
                && w.pos[0] <= (b[0] + b[2]) as f32
                && w.pos[1] <= (b[1] + b[3]) as f32
        });
        let c = caption.unwrap_or_else(|| panic!("{what}: button {b:?} has no caption"));
        assert!((c.pos[0] - (b[0] as f32 + b[2] as f32 / 2.0)).abs() <= 1.0, "{what}: {:?} on {b:?}", c.text);
        assert!((c.pos[1] - (b[1] as f32 + b[3] as f32 / 2.0)).abs() <= 1.0, "{what}: {:?} on {b:?}", c.text);
        assert!(c.bx[2] <= b[2] as f32, "{what}: caption {:?} wider than {b:?}", c.text);
        assert!(b[0] >= frame[0] && b[1] >= frame[1], "{what}: {b:?} outside {frame:?}");
        assert!(b[0] + b[2] <= frame[0] + frame[2] && b[1] + b[3] <= frame[1] + frame[3], "{what}: {b:?}");
    }
    let row_rects: Vec<[i32; 4]> = (0..MODS_ROWS)
        .map(|k| [LIST_RECT[0], LIST_RECT[1] + k as i32 * MODS_ROW_STEP, LIST_RECT[2], MODS_ROW_H])
        .collect();
    let header = [frame[0], frame[1], frame[2], LIST_RECT[1] - frame[1]];
    // the title and the server line: centred on the frame art's title bar, on the grid's left edge; nothing else in
    // the bar, and the tabs and the content under its divider
    let bar = [frame[0], frame[1] + FRAME_BAR_TOP, frame[2], FRAME_BAR_H];
    let in_bar: Vec<&Words> = all.iter().filter(|w| w.bx[1] < (frame[1] + FRAME_HEAD_H) as f32).collect();
    assert!(!in_bar.is_empty(), "{what}: a title");
    for w in &in_bar {
        assert_eq!(w.anchor[1], 0.5, "{what}: {:?}", w.text);
        assert_eq!(w.pos[1], frame_title_y(frame[1]) as f32, "{what}: {:?}", w.text);
        assert_eq!(frame_title_y(frame[1]), bar[1] + bar[3] / 2);
        assert!(within(w.bx, bar), "{what}: {:?} at {:?} leaves the bar {bar:?}", w.text, w.bx);
    }
    assert_eq!(in_bar[0].bx[0], LIST_RECT[0] as f32, "{what}: {:?}", in_bar[0].text);
    assert!(buttons.iter().all(|b| b[1] >= frame[1] + FRAME_HEAD_H), "{what}: a button in the header");
    for w in &all {
        let ok = within(w.bx, DETAIL_RECT)
            || within(w.bx, header)
            || row_rects.iter().any(|r| within(w.bx, *r))
            || buttons.iter().any(|b| within(w.bx, *b))
            || within(w.bx, [frame[0], LIST_RECT[1], frame[2], LIST_RECT[3]]) && !rows.contains(&w.text.as_str());
        assert!(ok, "{what}: text {:?} at {:?} lies in no row or panel", w.text, w.bx);
    }
    for name in rows {
        let n = all
            .iter()
            .filter(|w| w.text == *name && within(w.bx, LIST_RECT))
            .count();
        assert_eq!(n, 1, "{what}: {name:?} has {n} rows");
    }
}

fn save_shot(m: &mut Menu, name: &str) {
    if let Some(dir) = std::env::var_os("DC_MODS_SHOTS") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img = m.image();
        image::save_buffer(dir.join(name), &img.rgba, img.width, img.height, image::ColorType::Rgba8).unwrap();
    }
}

#[test]
fn the_mods_window_is_one_aligned_grid_at_every_size() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("grid");
    let xray = cached_example(&root, "hacking_mod");
    let (zip, entry) = crosshair_package();
    let routes: Routes = Arc::default();
    let doc = serde_json::json!({"version": 1, "mods": [
        {"id": entry.id, "version": entry.version.to_string(), "sha256": entry.sha256,
         "name": "Custom Crosshair", "author": "DeCombat",
         "description": "A drawn crosshair: pick its shape and colour, with an option for recoil kickback.",
         "size": zip.len(), "screenshot": false},
        {"id": "night_sky", "version": "2.1.0", "sha256": "ef".repeat(32), "name": "Night Sky",
         "author": "Someone With A Rather Long Name",
         "description": "Turns every sky dark. A long description follows so that the row must cut it down to one line while the detail panel wraps it in full over several lines without losing a word of it.",
         "size": 2_400_000, "screenshot": false}]});
    routes.lock().unwrap().insert("/mods/catalog.json".into(), (200, doc.to_string().into_bytes()));
    routes.lock().unwrap().insert(format!("/mods/{}.zip", entry.sha256), (200, zip));
    let (_auth_l, auth) = lobby(routes, Arc::default());

    // nothing installed but the package: opens on My Mods; an empty PC opens on Browse
    let empty = temp("grid_empty");
    let mut m = menu(Some(&empty), auth);
    m.load_settings(store(&empty));
    m.open(dc_client::menu::Go::Mods);
    until(&mut m, "Browse first on an empty PC", |m| texts(m).iter().any(|x| x == T_DOWNLOAD));
    m.input(MenuInput::Command(CMD_TAB_MINE));
    assert!(has(&texts(&mut m), T_NOTHING_INSTALLED));
    save_shot(&mut m, "empty_1024x768.png");
    drop(m);

    let mut m = menu(Some(&root), auth);
    m.load_settings(store(&root));
    m.data.settings.mods_accepted = vec![format!("hacking_mod@1.1.1:{xray}")];
    local_mod(&root, "rules", "server_rules");
    m.open(dc_client::menu::Go::Mods);
    until(&mut m, "the catalogue", |m| {
        m.input(MenuInput::Command(CMD_TAB_BROWSE));
        let got = texts(m).iter().any(|x| x == T_DOWNLOAD);
        got
    });
    // the crosshair arrives through the real download, the other stays to get
    m.input(MenuInput::Command(CMD_DOWNLOAD_BASE));
    until(&mut m, "the install", |m| has(&texts(m), T_INSTALLED));
    m.input(MenuInput::Command(CMD_TAB_MINE));
    m.input(MenuInput::Command(CMD_TOGGLE_BASE));

    for (w, h) in [(1024, 768), (2560, 1440), (2000, 1125), (1920, 1080), (1280, 720)] {
        m.set_size(w, h);
        let at = format!("{w}x{h}");
        m.input(MenuInput::Command(CMD_TAB_MINE));
        for row in 0..3 {
            m.input(MenuInput::Command(CMD_ROW_BASE + row));
            check_geometry(&mut m, &format!("my mods row {row} at {at}"), &["rules look", "Hacking Mod"]);
            save_shot(&mut m, &format!("mine_row{row}_{at}.png"));
        }
        m.input(MenuInput::Command(CMD_TAB_BROWSE));
        for row in 0..2 {
            m.input(MenuInput::Command(CMD_ROW_BASE + row));
            check_geometry(&mut m, &format!("browse row {row} at {at}"), &["Custom Crosshair", "Night Sky"]);
            save_shot(&mut m, &format!("browse_row{row}_{at}.png"));
        }
        m.input(MenuInput::Command(CMD_TAB_MINE));
        m.input(MenuInput::Command(CMD_ROW_BASE));
        m.input(MenuInput::Command(CMD_SETTINGS_BASE));
        let (frame, buttons) = m.layout_rects().expect("the page's layout");
        let all = words(&mut m);
        for b in &buttons {
            let c = all
                .iter()
                .find(|x| x.anchor == [0.5, 0.5] && x.pos[0] >= b[0] as f32 && x.pos[0] <= (b[0] + b[2]) as f32
                    && x.pos[1] >= b[1] as f32 && x.pos[1] <= (b[1] + b[3]) as f32)
                .expect("a caption");
            assert!((c.pos[0] - (b[0] as f32 + b[2] as f32 / 2.0)).abs() <= 1.0);
            assert!((c.pos[1] - (b[1] as f32 + b[3] as f32 / 2.0)).abs() <= 1.0);
        }
        for x in &all {
            assert!(within(x.bx, frame), "settings text {:?} at {at}", x.text);
        }
        for r in m.slider_rects() {
            assert!(r[0] >= frame[0] && r[1] >= frame[1] && r[0] + r[2] <= frame[0] + frame[2] && r[1] + r[3] <= frame[1] + frame[3], "slider {r:?} at {at}");
        }
        for b in &buttons {
            for o in buttons.iter().filter(|o| *o != b) {
                let apart = b[0] + b[2] <= o[0] || o[0] + o[2] <= b[0] || b[1] + b[3] <= o[1] || o[1] + o[3] <= b[1];
                assert!(apart, "buttons {b:?} and {o:?} at {at}");
            }
        }
        save_shot(&mut m, &format!("settings_{at}.png"));
        m.input(MenuInput::Command(CMD_PAGE_BACK));
    }
    m.set_size(1024, 768);

    // a disabled button stays legible and unclipped: the caption centred on full art
    m.data.catalog_busy = Some("night_sky".into());
    m.input(MenuInput::Command(CMD_TAB_BROWSE));
    m.input(MenuInput::Command(CMD_ROW_BASE + 1));
    assert!(texts(&mut m).iter().filter(|x| x.as_str() == "Wait...").count() >= 2);
    check_geometry(&mut m, "busy", &["Custom Crosshair", "Night Sky"]);
    save_shot(&mut m, "disabled_1024x768.png");
    m.data.catalog_busy = None;
    m.input(MenuInput::Command(CMD_TAB_MINE));

    // a long list scrolls with the wheel; the footer stays where it is and every row stays in the list
    for k in 0..14 {
        local_mod(&root, &format!("extra{k:02}"), "client_visual");
    }
    m.input(MenuInput::Key(dc_client::menu::MenuKey::Escape));
    m.open(dc_client::menu::Go::Mods);
    m.input(MenuInput::Command(CMD_TAB_MINE));
    let before = words(&mut m);
    assert!(before.iter().any(|w| w.text == "extra00 look"));
    let centre = [LIST_RECT[0] as f32 + 50.0, LIST_RECT[1] as f32 + 50.0];
    m.input(MenuInput::Wheel(centre, -30));
    let after = words(&mut m);
    assert!(!after.iter().any(|w| w.text == "extra00 look"), "the list scrolled");
    assert!(after.iter().any(|w| w.text == "Hacking Mod"), "to its end");
    assert!(MODS_RECT[1] + MODS_RECT[3] > FOOT_Y + 25);
    let (_, buttons) = m.layout_rects().unwrap();
    assert!(buttons.iter().all(|b| b[1] + b[3] <= MODS_RECT[1] + MODS_RECT[3]));
    save_shot(&mut m, "scrolled_1024x768.png");

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&empty);
}

/// The button texture's visible art is the size the nine parts are cut from (only with the ROM present).
#[test]
fn the_button_art_constant_is_the_textures_visible_part() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = menu(None, "127.0.0.1:1".parse().unwrap());
    for t in m.cat.paths.popup_buttons.clone().iter().flatten() {
        let Some((w, h)) = m.cat.tex_size(t) else { continue };
        let tex = m.cat.assets.texture(t).unwrap();
        let (mut ax, mut ay) = (0, 0);
        for y in 0..h {
            for x in 0..w {
                if tex.rgba[((y * w + x) * 4 + 3) as usize] > 20 {
                    ax = ax.max(x + 1);
                    ay = ay.max(y + 1);
                }
            }
        }
        assert_eq!((ax, ay), BUTTON_ART, "{t}");
    }
}

/// The frame's title bar constants are the art's (only with the ROM present): at the art's middle column the bar is
/// the run under the bright top border, ended by the divider, a line brighter than both its neighbours.
#[test]
fn the_frame_bar_constants_are_the_arts() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = menu(None, "127.0.0.1:1".parse().unwrap());
    let rom = dc_rom::resolve_rom().ok();
    let Some(t) = dc_client::menu::usebox::NameArt::from_rom(rom.as_deref()).window else { return };
    let Some((tw, _)) = m.cat.tex_size(&t) else { return };
    let (aw, ah) = frame_art_size(&mut m.cat, &t).unwrap();
    assert!(FRAME_HEAD_H <= (ah / 4) as i32, "the bar lies in the corners' row, drawn at the art's own size");
    let tex = m.cat.assets.texture(&t).unwrap();
    let lum = |y: u32| tex.rgba[((y * tw + aw / 2) * 4) as usize] as i32;
    let bright = |y: u32| lum(y) > lum(y - 1) + 15 && lum(y) > lum(y + 1) + 15;
    let lines: Vec<u32> = (1..ah / 4).filter(|y| bright(*y)).collect();
    assert_eq!(lines, vec![FRAME_BAR_TOP as u32 - 1, (FRAME_BAR_TOP + FRAME_BAR_H) as u32], "{t}");
}

/// MOD-27: the detail panel shows the entry's Commands section and the operator's picture instead of the package's
/// screenshot; a picture the lobby cannot deliver leaves the panel as it was with no line about it.
#[test]
fn the_panel_shows_the_commands_and_the_operators_picture() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (zip, entry) = crosshair_package();
    let picture = {
        let mut out = Vec::new();
        image::RgbaImage::from_pixel(64, 36, image::Rgba([200, 30, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    };
    let pic_sha = dc_rom::sha256_hex(&picture);
    let doc = |preview: bool| {
        let mut e = serde_json::json!({"id": entry.id, "version": entry.version.to_string(), "sha256": entry.sha256,
            "name": "Custom Crosshair", "author": "DeCombat", "description": "A drawn crosshair for the test.",
            "size": zip.len(), "screenshot": false,
            "commands": [{"command": "/wallhack", "description": "Next style."}]});
        if preview {
            e["preview"] = serde_json::json!(pic_sha);
        }
        serde_json::json!({"version": 1, "mods": [e]}).to_string().into_bytes()
    };
    let shown = |m: &mut Menu, sha: &str| {
        m.items().into_iter().any(|i| match i {
            DrawItem::Image { tex, .. } => tex.starts_with("modshot:") && tex.contains(sha),
            _ => false,
        })
    };
    let open = |tag: &str, routes: Routes| {
        let root = temp(tag);
        let (l, auth) = lobby(routes, Arc::default());
        let mut m = menu(Some(&root), auth);
        m.load_settings(store(&root));
        m.update();
        m.input(MenuInput::Command(login::CMD_HOMEPAGE));
        m.input(MenuInput::Command(CMD_MODS));
        until(&mut m, "the list", |m| texts(m).iter().any(|x| x == T_DOWNLOAD));
        (m, root, l)
    };

    // delivered: the picture and the section
    let routes: Routes = Arc::default();
    routes.lock().unwrap().insert("/mods/catalog.json".into(), (200, doc(true)));
    routes.lock().unwrap().insert(format!("/mods/{pic_sha}.png"), (200, picture.clone()));
    let (mut m, root, _l) = open("preview_ok", routes);
    let t = texts(&mut m);
    assert!(has(&t, "Commands") && has(&t, "/wallhack - Next style."), "{t:?}");
    assert!(shown(&mut m, &pic_sha), "the operator's picture is drawn");
    let _ = std::fs::remove_dir_all(&root);

    // flagged but the route fails: no picture, no error line, the section stays
    let routes: Routes = Arc::default();
    routes.lock().unwrap().insert("/mods/catalog.json".into(), (200, doc(true)));
    let (mut m, root, _l) = open("preview_dead", routes);
    let t = texts(&mut m);
    assert!(has(&t, "Commands"), "{t:?}");
    assert!(!shown(&mut m, &pic_sha));
    assert!(!t.iter().any(|x| x.contains("could not") || x.contains("picture")), "{t:?}");
    let _ = std::fs::remove_dir_all(&root);

    // a set entry: the picture over the unpacked package's own screenshot, by the set's entry
    let root = temp("preview_set");
    let hack = cached_example(&root, "hacking_mod");
    let paths = dc_client::mods::ModPaths::new(&root);
    std::fs::create_dir_all(paths.catalog_shot(&pic_sha).parent().unwrap()).unwrap();
    std::fs::write(paths.catalog_shot(&pic_sha), &picture).unwrap();
    let mut set = set_of("hacking_mod", &hack);
    let server: SocketAddr = "127.0.0.1:12001".parse().unwrap();
    set.mods[0].preview = Some(pic_sha.clone());
    persisted(&root, server, "Hacking Mod (US Central)", &set, Duration::from_secs(60));
    let mut m = menu(Some(&root), server);
    m.load_settings(store(&root));
    m.update();
    let rows = dc_client::menu::modbrowser::set_rows(&m.data, &set);
    assert_eq!(rows[0].preview, Some(paths.catalog_shot(&pic_sha)));
    assert!(rows[0].shot.as_ref().is_some_and(|s| s.is_file()), "the package's own stays behind it");
    assert_eq!(rows[0].commands.len(), 4, "from the unpacked manifest");
    set.mods[0].preview = None;
    assert_eq!(dc_client::menu::modbrowser::set_rows(&m.data, &set)[0].preview, None);
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------ CLI-1940: the download shows in the wait box

/// The login's wait box takes the pack download's line (pack count and bytes) while the join works, and gets its own
/// text back when the join is past the download.
#[test]
fn the_login_wait_box_shows_the_pack_download_and_gives_its_text_back() {
    use dc_client::mods::ModState;
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = menu(None, "127.0.0.1:1".parse().unwrap());
    m.detach_link();
    m.update();
    m.input(MenuInput::Command(login::CMD_LOGIN));
    let before = texts(&mut m);
    assert!(
        !has(&before, "Downloading server packs"),
        "the plain wait box was expected: {before:?}"
    );
    m.feed_event(LinkEvent::Mods(ModState::FetchingSet));
    m.update();
    assert!(has(&texts(&mut m), "Reading the server's pack list"));
    m.feed_event(LinkEvent::Mods(ModState::Downloading {
        index: 3,
        count: 65,
        done: 3 * 1024 * 1024,
        total: 0,
    }));
    m.update();
    let t = texts(&mut m);
    assert!(
        has(&t, "Downloading server packs: 3 of 65 (3.0 MB so far)"),
        "{t:?}"
    );
    m.feed_event(LinkEvent::Mods(ModState::Downloading {
        index: 4,
        count: 65,
        done: 5 * 1024 * 1024,
        total: 0,
    }));
    m.update();
    assert!(has(&texts(&mut m), "4 of 65 (5.0 MB so far)"));
    m.feed_event(LinkEvent::Mods(ModState::Verifying));
    m.update();
    assert!(has(&texts(&mut m), "Checking the packs"));
    m.feed_event(LinkEvent::Mods(ModState::Ready));
    m.update();
    let after = texts(&mut m);
    assert!(!has(&after, "packs"), "{after:?}");
    assert_eq!(after, before, "the box's own text did not come back");
}

// ------------------------------------------------------------------ CLI-1943: the camo packages fold into one row

/// An in-test weapon camo `test_camo_<n>` ("Test Camo <nn>", kind shared, tags variant + camo, a generated pattern)
/// packed into the by-hash cache of `root` and unpacked there; its sha256.
fn cached_camo(root: &Path, n: usize) -> String {
    let src = root.join(format!("src_test_camo_{n}"));
    std::fs::create_dir_all(src.join("assets")).unwrap();
    std::fs::create_dir_all(src.join("screenshots")).unwrap();
    std::fs::write(
        src.join("manifest.json"),
        format!(
            r#"{{"id": "test_camo_{n}", "name": "Test Camo {n:02}", "version": "1.0.0", "kind": "shared",
            "requires_client": true, "capabilities": ["content.weapons", "assets"],
            "description": "A test camo: row {row} is the SCAR-L with a new look and nothing else.",
            "author": "DeCombat", "tags": ["variant", "camo"], "screenshots": ["screenshots/main.png"]}}"#,
            row = 510 + n
        ),
    )
    .unwrap();
    std::fs::write(
        src.join("main.lua"),
        format!(
            "return function(mod)
    local art = mod.assets:path(\"assets/pattern.dtx\")
    mod.content.weapons:register({{
        id = \"Weapon{row}\",
        variant_of = 33,
        variant_name = \"Test Camo {n:02}\",
        PVSkin1 = \"@camo:paint:33:PVSkin1:\" .. art,
    }})
end
",
            row = 510 + n
        ),
    )
    .unwrap();
    let rgba: Vec<u8> = (0..16 * 16).flat_map(|k| [(k * 7 % 256) as u8, 90, 160, 255]).collect();
    std::fs::write(src.join("assets/pattern.dtx"), dc_tex::write_bgra8(16, 16, &rgba).unwrap()).unwrap();
    image::RgbaImage::from_pixel(16, 9, image::Rgba([60, 30, 90, 255]))
        .save(src.join("screenshots/main.png"))
        .unwrap();
    let (zip, entry) = dc_modkit::pack_mod_bytes(&src).expect("the camo fixture packs");
    let paths = dc_client::mods::ModPaths::new(root);
    std::fs::create_dir_all(paths.by_hash()).unwrap();
    std::fs::write(paths.zip(&entry.sha256), &zip).unwrap();
    dc_client::mods::unpack_package(&zip, &paths.unpacked(&entry.sha256), 1 << 24).unwrap();
    entry.sha256
}

#[test]
fn camo_packages_fold_into_one_row_that_opens_to_list_them() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("camos");
    let xray = cached_example(&root, "hacking_mod");
    let camos: Vec<String> = (1..=12).map(|n| cached_camo(&root, n)).collect();
    let mut mods = vec![format!(r#"{{"id": "hacking_mod", "version": "1.1.1", "sha256": "{xray}"}}"#)];
    for (k, sha) in camos.iter().enumerate() {
        mods.push(format!(r#"{{"id": "test_camo_{}", "version": "1.0.0", "sha256": "{sha}"}}"#, k + 1));
    }
    let doc = parse_community(&format!(
        r#"{{"schema_version": 1, "updated": "2026-11-08T12:00:00Z",
        "servers": [
          {{"id": "vps", "name": "DeCombat VPS", "region": "na", "address": "127.0.0.1:10000",
            "policy": "vanilla", "mods": []}},
          {{"id": "camo-land", "name": "Camo Land", "region": "na", "address": "127.0.0.1:11000",
            "policy": "modded", "mods": [{}]}}
        ], "known_mods": []}}"#,
        mods.join(", ")
    ))
    .expect("the fixture document is valid");
    let view = CommunityView {
        addrs: vec![
            Some("127.0.0.1:10000".parse().unwrap()),
            Some("127.0.0.1:11000".parse().unwrap()),
        ],
        doc: Some(doc),
        cached: false,
        error: None,
    };
    let mut m = menu(Some(&root), "127.0.0.1:10000".parse().unwrap());
    m.load_settings(store(&root));
    m.feed_event(LinkEvent::Community(view));
    m.feed_event(LinkEvent::OtherLobbies(vec![other(
        "Camo Land",
        "US West",
        "127.0.0.1:11000".parse().unwrap(),
    )]));
    m.feed_event(LinkEvent::Servers(vanilla_list()));
    m.update();

    // the modded row (after the three entries of this lobby): My Mods lists the Hacking Mod and ONE camo row
    m.open(dc_client::menu::Go::Servers);
    for _ in 0..3 {
        m.input(MenuInput::Key(dc_client::menu::MenuKey::Down));
    }
    m.input(MenuInput::Command(CMD_MODS));
    assert_eq!(m.screen_name(), "mods");
    let t = texts(&mut m);
    for want in ["Hacking Mod", T_CAMOS, "12 camos", "Test Camo 01, Test Camo 02"] {
        assert!(has(&t, want), "missing {want:?} in {t:?}");
    }
    // CLI-1943 (amended): both rows come from the server; nothing here is the player's own
    assert_eq!(t.iter().filter(|s| *s == T_ORIGIN_SERVER).count(), 2, "{t:?}");
    assert!(!has(&t, T_ORIGIN_YOURS) && !has(&t, T_ORIGIN_OPTIONAL), "{t:?}");
    for n in 4..=12 {
        assert!(!has(&t, &format!("Test Camo {n:02}")), "camo {n} listed while the group is closed: {t:?}");
    }

    // the group's detail: the kind, the shared author, the notes once, the count with the size, Show camos
    m.input(MenuInput::Command(CMD_ROW_BASE + 1));
    let all = detail_text(&mut m);
    for want in [T_CAMOS_KIND, "By DeCombat", T_CAMOS_NOTE, "Test Camo 03 and 9 more", "Required by Camo Land.", "12 camos,", T_SHOW_CAMOS] {
        assert!(all.contains(want), "missing {want:?} in {all}");
    }
    assert!(!all.contains("Version"), "a group has no version: {all}");
    let once = texts(&mut m).join(" ");
    assert_eq!(once.matches("Required by Camo Land.").count(), 1, "the shared note once: {once}");

    // Show camos: the camos follow the group row in order, each with its own row; the group stays selected
    m.input(MenuInput::Command(CMD_GROUP_BASE));
    let t = texts(&mut m);
    assert!(has(&t, T_HIDE_CAMOS), "{t:?}");
    for n in 1..=7 {
        assert!(has(&t, &format!("Test Camo {n:02}")), "camo {n} not listed while the group is open: {t:?}");
    }
    assert!(!has(&t, "Test Camo 08"), "nine rows show at a time: {t:?}");
    m.input(MenuInput::Command(CMD_ROW_BASE + 2));
    let all = detail_text(&mut m);
    for want in ["Test Camo 01", "Version 1.0.0 by DeCombat", "Required by Camo Land.", "Remove"] {
        assert!(all.contains(want), "missing {want:?} in the camo's own detail: {all}");
    }

    // Hide camos (from the group row): closed again
    m.input(MenuInput::Command(CMD_ROW_BASE + 1));
    m.input(MenuInput::Command(CMD_GROUP_BASE));
    let t = texts(&mut m);
    assert!(has(&t, T_SHOW_CAMOS) && !has(&t, "Test Camo 04"), "{t:?}");

    // Browse: everything is on this PC, so the server adds no rows there and no group either
    m.input(MenuInput::Command(CMD_TAB_BROWSE));
    let t = texts(&mut m);
    assert!(!has(&t, T_CAMOS), "{t:?}");
    let _ = std::fs::remove_dir_all(&root);
}
